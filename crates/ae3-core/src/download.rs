//! Downloading Packages to disk: retries with exponential backoff, a size
//! check against the Manifest, a temp file renamed into place, and a bounded
//! pool of std threads (ADR-0004: no async runtime).
//!
//! A [`Downloader`] shares one cancellation flag with whoever drives it, such
//! as the GUI's Cancel button or a Ctrl-C handler. Setting the flag stops
//! in-flight transfers at their next chunk and skips queued ones.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::http::{HttpError, RangeResponse, Transport};

/// How many downloads run at once unless configured otherwise.
pub const DEFAULT_PARALLEL_DOWNLOADS: usize = 8;
/// How many times a request is tried before its failure is reported.
pub const DEFAULT_ATTEMPTS: u32 = 3;
/// The pause before the first retry; each later pause doubles it.
pub const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// How cancellation is checked during a backoff pause.
const CANCEL_POLL: Duration = Duration::from_millis(25);

/// How a [`Downloader`] behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOptions {
    /// Downloads run at once by [`Downloader::download_all`]; at least 1.
    pub parallel: usize,
    /// Tries per request, including the first; at least 1.
    pub attempts: u32,
    /// The pause before the first retry, doubled for each later one.
    pub initial_backoff: Duration,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            parallel: DEFAULT_PARALLEL_DOWNLOADS,
            attempts: DEFAULT_ATTEMPTS,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
        }
    }
}

/// One file to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadJob {
    pub url: String,
    /// Where the finished file goes. It appears only once complete and of
    /// the expected size; partial data never sits at this path.
    pub dest: PathBuf,
    /// The size the Manifest gives the Package, checked after download.
    pub expected_size: Option<u64>,
}

impl DownloadJob {
    #[must_use]
    pub fn new(url: impl Into<String>, dest: impl Into<PathBuf>) -> Self {
        Self {
            url: url.into(),
            dest: dest.into(),
            expected_size: None,
        }
    }

    #[must_use]
    pub fn with_expected_size(mut self, size: Option<u64>) -> Self {
        self.expected_size = size;
        self
    }
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error(transparent)]
    Http(#[from] HttpError),
    /// The body was not the size the Manifest lists. `actual` is a lower
    /// bound when the transfer was stopped for running past `expected`.
    #[error("{url}: expected {expected} bytes, received {actual}")]
    SizeMismatch {
        url: String,
        expected: u64,
        actual: u64,
    },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("download cancelled")]
    Cancelled,
}

impl DownloadError {
    /// Whether another attempt may succeed.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http(e) => e.is_transient(),
            Self::SizeMismatch { .. } => true,
            Self::Io { .. } | Self::Cancelled => false,
        }
    }

    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }
}

/// A snapshot of a [`Downloader::download_all`] batch, passed to its
/// progress callback after every chunk and every finished job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub jobs_total: usize,
    pub jobs_done: usize,
    pub jobs_failed: usize,
    /// Bytes written so far. Bytes from a failed attempt are taken back out.
    pub bytes_done: u64,
    /// The sum of the jobs' expected sizes; jobs without one count as 0.
    pub bytes_total: u64,
}

/// Downloads files over a [`Transport`].
///
/// `T` is any Transport, including `&T` and `Arc<T>`, so a Downloader can
/// borrow or share one. [`Downloader::download_all`] needs `T: Sync`.
pub struct Downloader<T> {
    transport: T,
    options: DownloadOptions,
    cancel: Arc<AtomicBool>,
}

/// Byte accounting shared by the jobs of one batch.
struct Tally<'a> {
    bytes: &'a AtomicU64,
    notify: &'a (dyn Fn() + Sync),
}

impl<T: Transport> Downloader<T> {
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self::with_options(transport, DownloadOptions::default())
    }

    #[must_use]
    pub fn with_options(transport: T, options: DownloadOptions) -> Self {
        Self {
            transport,
            options,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Uses `flag` as the cancellation flag, so the caller can cancel by
    /// setting it.
    #[must_use]
    pub fn with_cancel_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel = flag;
        self
    }

    /// The shared cancellation flag; store `true` to cancel.
    #[must_use]
    pub fn cancel_flag(&self) -> &Arc<AtomicBool> {
        &self.cancel
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    #[must_use]
    pub fn options(&self) -> &DownloadOptions {
        &self.options
    }

    /// Downloads one file with retries, returning its size.
    pub fn download(&self, job: &DownloadJob) -> Result<u64, DownloadError> {
        let bytes = AtomicU64::new(0);
        let tally = Tally {
            bytes: &bytes,
            notify: &|| {},
        };
        self.download_tallied(job, &tally)
    }

    /// GETs `url` into memory with retries, checking its size if known.
    pub fn fetch(&self, url: &str, expected_size: Option<u64>) -> Result<Vec<u8>, DownloadError> {
        self.with_retries(|| {
            let bytes = self.transport.get_bytes(url)?;
            check_size(url, expected_size, bytes.len() as u64)?;
            Ok(bytes)
        })
    }

    /// A Range GET of bytes `start..=end_inclusive` of `url`, with retries.
    pub fn fetch_range(
        &self,
        url: &str,
        start: u64,
        end_inclusive: u64,
    ) -> Result<RangeResponse, DownloadError> {
        self.with_retries(|| Ok(self.transport.get_range(url, start, end_inclusive)?))
    }

    fn download_tallied(&self, job: &DownloadJob, tally: &Tally<'_>) -> Result<u64, DownloadError> {
        self.with_retries(|| self.attempt(job, tally))
    }

    fn with_retries<R>(
        &self,
        mut attempt: impl FnMut() -> Result<R, DownloadError>,
    ) -> Result<R, DownloadError> {
        let attempts = self.options.attempts.max(1);
        let mut delay = self.options.initial_backoff;
        let mut tried = 0;
        loop {
            if self.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
            tried += 1;
            match attempt() {
                Ok(value) => return Ok(value),
                Err(_) if self.is_cancelled() => return Err(DownloadError::Cancelled),
                Err(err) if tried >= attempts || !err.is_transient() => return Err(err),
                Err(_) => {
                    if !self.pause(delay) {
                        return Err(DownloadError::Cancelled);
                    }
                    delay = delay.saturating_mul(2);
                }
            }
        }
    }

    /// Sleeps for `delay` unless cancelled first; returns whether it was not.
    fn pause(&self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        loop {
            if self.is_cancelled() {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            thread::sleep(CANCEL_POLL.min(deadline - now));
        }
    }

    fn attempt(&self, job: &DownloadJob, tally: &Tally<'_>) -> Result<u64, DownloadError> {
        if let Some(dir) = job.dest.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|e| DownloadError::io(dir, e))?;
        }
        let tmp = temp_path(&job.dest);
        let file = File::create(&tmp).map_err(|e| DownloadError::io(&tmp, e))?;
        let mut writer = GuardedWriter {
            inner: BufWriter::with_capacity(256 * 1024, file),
            cancel: &self.cancel,
            limit: job.expected_size,
            written: 0,
            overflow: None,
            failure: None,
            tally,
        };
        let result = self.transport.get_to_writer(&job.url, &mut writer);
        let finished = result.map_err(DownloadError::from).and_then(|_| {
            writer
                .inner
                .flush()
                .map_err(|e| DownloadError::io(&tmp, e))?;
            check_size(&job.url, job.expected_size, writer.written)
        });
        let GuardedWriter {
            inner,
            written,
            overflow,
            failure,
            ..
        } = writer;
        drop(inner);
        let outcome = finished
            .map_err(|err| match (overflow, failure) {
                (Some(actual), _) => DownloadError::SizeMismatch {
                    url: job.url.clone(),
                    expected: job.expected_size.unwrap_or_default(),
                    actual,
                },
                (None, Some(source)) if !self.is_cancelled() => DownloadError::io(&tmp, source),
                _ => err,
            })
            .and_then(|()| {
                fs::rename(&tmp, &job.dest).map_err(|e| DownloadError::io(&job.dest, e))
            });
        match outcome {
            Ok(()) => Ok(written),
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                tally.bytes.fetch_sub(written, Ordering::Relaxed);
                (tally.notify)();
                Err(err)
            }
        }
    }
}

impl<T: Transport + Sync> Downloader<T> {
    /// Downloads every job on a pool of at most [`DownloadOptions::parallel`]
    /// threads. Results are in job order. Once cancelled, queued jobs fail
    /// with [`DownloadError::Cancelled`] without being started.
    ///
    /// # Panics
    ///
    /// If `progress` or the transport panics on a worker thread.
    pub fn download_all(
        &self,
        jobs: &[DownloadJob],
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Vec<Result<u64, DownloadError>> {
        let jobs_total = jobs.len();
        let bytes_total = jobs.iter().filter_map(|j| j.expected_size).sum();
        let bytes = AtomicU64::new(0);
        let done = AtomicUsize::new(0);
        let failed = AtomicUsize::new(0);
        let next = AtomicUsize::new(0);
        let notify = || {
            progress(&Progress {
                jobs_total,
                jobs_done: done.load(Ordering::Relaxed),
                jobs_failed: failed.load(Ordering::Relaxed),
                bytes_done: bytes.load(Ordering::Relaxed),
                bytes_total,
            });
        };
        let tally = Tally {
            bytes: &bytes,
            notify: &notify,
        };
        let workers = self.options.parallel.clamp(1, jobs_total.max(1));
        let mut results: Vec<Option<Result<u64, DownloadError>>> =
            std::iter::repeat_with(|| None).take(jobs_total).collect();
        thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(job) = jobs.get(i) else { break };
                            let result = if self.is_cancelled() {
                                Err(DownloadError::Cancelled)
                            } else {
                                self.download_tallied(job, &tally)
                            };
                            let counter = if result.is_ok() { &done } else { &failed };
                            counter.fetch_add(1, Ordering::Relaxed);
                            notify();
                            out.push((i, result));
                        }
                        out
                    })
                })
                .collect();
            for handle in handles {
                for (i, result) in handle.join().expect("download worker panicked") {
                    results[i] = Some(result);
                }
            }
        });
        results
            .into_iter()
            .map(|r| r.unwrap_or(Err(DownloadError::Cancelled)))
            .collect()
    }
}

fn check_size(url: &str, expected: Option<u64>, actual: u64) -> Result<(), DownloadError> {
    match expected {
        Some(expected) if expected != actual => Err(DownloadError::SizeMismatch {
            url: url.to_owned(),
            expected,
            actual,
        }),
        _ => Ok(()),
    }
}

/// A hidden, unique sibling of `dest`, so the rename into place stays on one
/// file system and concurrent writers never share a temp file.
fn temp_path(dest: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dest.with_file_name(format!(".{name}.{}-{n}{PART_SUFFIX}", std::process::id()))
}

/// The suffix of in-progress download files.
pub(crate) const PART_SUFFIX: &str = ".part";

/// Counts bytes, enforces the expected size and checks for cancellation on
/// every chunk.
struct GuardedWriter<'a, W: Write> {
    inner: W,
    cancel: &'a AtomicBool,
    limit: Option<u64>,
    written: u64,
    /// Set when the body ran past `limit`, to the size it would have reached.
    overflow: Option<u64>,
    /// The disk error that stopped the transfer, if any.
    failure: Option<io::Error>,
    tally: &'a Tally<'a>,
}

impl<W: Write> Write for GuardedWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        let reached = self.written + buf.len() as u64;
        if self.limit.is_some_and(|limit| reached > limit) {
            self.overflow = Some(reached);
            return Err(io::Error::other("more bytes than the Manifest lists"));
        }
        match self.inner.write(buf) {
            Ok(n) => {
                self.written += n as u64;
                self.tally.bytes.fetch_add(n as u64, Ordering::Relaxed);
                (self.tally.notify)();
                Ok(n)
            }
            Err(e) => {
                let message = e.to_string();
                self.failure = Some(e);
                Err(io::Error::other(message))
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::test_support::{FakeTransport, TempDir, transient};

    const URL: &str = "https://cdn.test/a.bundle";

    fn fast(transport: &FakeTransport) -> Downloader<&FakeTransport> {
        Downloader::with_options(
            transport,
            DownloadOptions {
                parallel: 4,
                attempts: 3,
                initial_backoff: Duration::from_millis(1),
            },
        )
    }

    fn leftover_parts(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(PART_SUFFIX))
            .collect()
    }

    #[test]
    fn retries_transient_failures_then_succeeds() {
        let t = FakeTransport::default().with_file(URL, b"bundle");
        t.fail_next(URL, transient(URL));
        t.fail_next(
            URL,
            HttpError::Status {
                url: URL.into(),
                status: 503,
            },
        );
        let dir = TempDir::new("retry");
        let dest = dir.path().join("x/a.bundle");
        let job = DownloadJob::new(URL, &dest).with_expected_size(Some(6));
        assert_eq!(fast(&t).download(&job).unwrap(), 6);
        assert_eq!(fs::read(&dest).unwrap(), b"bundle");
        assert_eq!(t.requests_for(URL), 3);
        assert_eq!(leftover_parts(&dir.path().join("x")), Vec::<String>::new());
    }

    #[test]
    fn gives_up_after_three_attempts() {
        let t = FakeTransport::default().with_file(URL, b"bundle");
        for _ in 0..3 {
            t.fail_next(URL, transient(URL));
        }
        let dir = TempDir::new("give-up");
        let dest = dir.path().join("a.bundle");
        let err = fast(&t)
            .download(&DownloadJob::new(URL, &dest))
            .unwrap_err();
        assert!(
            matches!(err, DownloadError::Http(HttpError::Transport { .. })),
            "{err}"
        );
        assert_eq!(t.requests_for(URL), 3);
        assert!(!dest.exists());
        assert_eq!(leftover_parts(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn does_not_retry_not_found() {
        let t = FakeTransport::default();
        let dir = TempDir::new("404");
        let err = fast(&t)
            .download(&DownloadJob::new(URL, dir.path().join("a")))
            .unwrap_err();
        assert!(matches!(
            err,
            DownloadError::Http(HttpError::Status { status: 404, .. })
        ));
        assert_eq!(t.requests_for(URL), 1);
    }

    #[test]
    fn size_mismatch_is_retried_and_reported() {
        let t = FakeTransport::default().with_file(URL, b"short");
        let dir = TempDir::new("size");
        let dest = dir.path().join("a.bundle");
        let job = DownloadJob::new(URL, &dest).with_expected_size(Some(100));
        let err = fast(&t).download(&job).unwrap_err();
        assert!(
            matches!(
                err,
                DownloadError::SizeMismatch {
                    expected: 100,
                    actual: 5,
                    ..
                }
            ),
            "{err}"
        );
        assert_eq!(t.requests_for(URL), 3);
        assert!(!dest.exists());

        // A body longer than expected is stopped early.
        let job = DownloadJob::new(URL, &dest).with_expected_size(Some(2));
        let err = fast(&t).download(&job).unwrap_err();
        assert!(
            matches!(err, DownloadError::SizeMismatch { expected: 2, .. }),
            "{err}"
        );
        assert_eq!(leftover_parts(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn exponential_backoff_doubles() {
        let t = FakeTransport::default().with_file(URL, b"x");
        t.fail_next(URL, transient(URL));
        t.fail_next(URL, transient(URL));
        let d = Downloader::with_options(
            &t,
            DownloadOptions {
                parallel: 1,
                attempts: 3,
                initial_backoff: Duration::from_millis(20),
            },
        );
        let start = Instant::now();
        assert_eq!(d.fetch(URL, Some(1)).unwrap(), b"x");
        // 20 ms, then 40 ms.
        assert!(start.elapsed() >= Duration::from_millis(60));
    }

    #[test]
    fn fetch_range_slices_and_reports_total() {
        let t = FakeTransport::default().with_file(URL, b"0123456789");
        let r = fast(&t).fetch_range(URL, 2, 4).unwrap();
        assert_eq!(r.bytes, b"234");
        assert_eq!(r.total_len, Some(10));
        let r = fast(&t).fetch_range(URL, 8, 100).unwrap();
        assert_eq!(r.bytes, b"89");
    }

    #[test]
    fn download_all_runs_in_parallel_and_keeps_order() {
        let t = FakeTransport::default();
        let dir = TempDir::new("all");
        let jobs: Vec<_> = (0..20)
            .map(|i| {
                let url = format!("https://cdn.test/{i}.bundle");
                let body = vec![b'a'; i + 1];
                t.put(&url, &body);
                DownloadJob::new(url, dir.path().join(format!("{i}.bundle")))
                    .with_expected_size(Some(i as u64 + 1))
            })
            .collect();
        t.fail_next(&jobs[3].url, transient(&jobs[3].url));
        let last = Mutex::new(Progress::default());
        let results = fast(&t).download_all(&jobs, &|p| *last.lock().unwrap() = *p);
        for (i, r) in results.iter().enumerate() {
            assert_eq!(*r.as_ref().unwrap(), i as u64 + 1);
        }
        let last = *last.lock().unwrap();
        assert_eq!(last.jobs_done, 20);
        assert_eq!(last.jobs_failed, 0);
        assert_eq!(last.bytes_done, last.bytes_total);
        assert_eq!(last.bytes_total, (1..=20).sum::<u64>());
    }

    #[test]
    fn cancellation_skips_queued_jobs() {
        let t = FakeTransport::default();
        let dir = TempDir::new("cancel");
        let jobs: Vec<_> = (0..10)
            .map(|i| {
                let url = format!("https://cdn.test/{i}");
                t.put(&url, b"data");
                DownloadJob::new(url, dir.path().join(i.to_string()))
            })
            .collect();
        let flag = Arc::new(AtomicBool::new(false));
        let d = Downloader::with_options(
            &t,
            DownloadOptions {
                parallel: 1,
                ..DownloadOptions::default()
            },
        )
        .with_cancel_flag(Arc::clone(&flag));
        let results = d.download_all(&jobs, &|p| {
            if p.jobs_done == 2 {
                flag.store(true, Ordering::Relaxed);
            }
        });
        assert!(results[..2].iter().all(Result::is_ok));
        assert!(
            results[2..]
                .iter()
                .all(|r| matches!(r, Err(DownloadError::Cancelled)))
        );
        assert_eq!(t.total_requests(), 2);
        assert_eq!(leftover_parts(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn cancellation_interrupts_backoff() {
        let t = FakeTransport::default().with_file(URL, b"x");
        t.fail_next(URL, transient(URL));
        let d = Downloader::with_options(
            &t,
            DownloadOptions {
                parallel: 1,
                attempts: 3,
                initial_backoff: Duration::from_secs(30),
            },
        );
        let flag = Arc::clone(d.cancel_flag());
        let start = Instant::now();
        let err = thread::scope(|s| {
            let h = s.spawn(|| d.fetch(URL, None));
            thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::Relaxed);
            h.join().unwrap()
        })
        .unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
