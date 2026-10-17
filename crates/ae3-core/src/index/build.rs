//! Building the Asset Index by range-probing Bundles.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ae3_unity::{Probe, ProbeListing, probe_prefix};
use serde::Serialize;

use super::{AssetIndex, BundleContents, BundleRecord, IndexError, IndexFile, default_index_path};
use crate::cache::PackageCache;
use crate::diff::ENGINE_REBUILD_RATIO;
use crate::download::DEFAULT_PARALLEL_DOWNLOADS;
use crate::http::Transport;
use crate::manifest::{Manifest, Package, PackageKind};
use crate::platform::Platform;
use crate::store::PackageStore;
use crate::version::{AssetVersion, CdnLocation};

/// The first prefix fetched of each Bundle. Small Bundles (portraits, icons)
/// list in about 1.5 KB and single Atlases in under 4 KB, so one request
/// usually suffices; larger SerializedFiles ask for more.
pub const DEFAULT_FIRST_PROBE_LEN: u64 = 4 * 1024;
/// How many newly probed Bundles are kept in memory before the index file is
/// saved again.
pub const DEFAULT_SAVE_EVERY: usize = 500;
/// Prefix rounds per Bundle before giving up; the prober converges in three.
const MAX_ROUNDS: usize = 12;
/// How often idle threads look at the pause and cancel flags.
const POLL: Duration = Duration::from_millis(50);

/// How an index build runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexOptions {
    /// Bundles probed at once.
    pub parallel: usize,
    /// Bytes fetched first from each Bundle.
    pub first_probe_len: u64,
    /// Save the index file after this many newly probed Bundles (and always
    /// when the run ends, pauses or is cancelled).
    pub save_every: usize,
    /// Probe again Bundles whose earlier probe failed for good at the same
    /// Content Hash. Off by default, so a Bundle the prober cannot read is
    /// not re-fetched on every run.
    pub retry_failed: bool,
    /// Ignore every stored record and probe all Bundles again.
    pub reprobe_all: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            parallel: DEFAULT_PARALLEL_DOWNLOADS,
            first_probe_len: DEFAULT_FIRST_PROBE_LEN,
            save_every: DEFAULT_SAVE_EVERY,
            retry_failed: false,
            reprobe_all: false,
        }
    }
}

/// What a build would do, from the stored records and the Manifest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct IndexPlan {
    pub asset_version: AssetVersion,
    /// Bundles in the Manifest.
    pub total_bundles: usize,
    /// Audio Packages, indexed by name only.
    pub audio_packages: usize,
    /// Bundles already probed at their current Content Hash.
    pub indexed: usize,
    /// Bundles whose probe failed for good at their current Content Hash,
    /// and that are not retried.
    pub failed: usize,
    /// Bundles probed before at another Content Hash.
    pub changed: usize,
    /// Bundles never probed.
    pub new: usize,
    /// Bundles this build will probe.
    pub to_probe: usize,
    /// Their combined size on the CDN: an upper bound on what probing them
    /// downloads (far more than it actually fetches).
    pub to_probe_size: u64,
}

impl IndexPlan {
    /// Whether most Bundles the index knew changed, as after an Engine
    /// Rebuild, so nearly everything is probed again.
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "a ratio")]
    pub fn is_engine_rebuild(&self) -> bool {
        let known = self.indexed + self.failed + self.changed;
        known > 0 && self.changed as f64 / known as f64 >= ENGINE_REBUILD_RATIO
    }

    /// Whether there is nothing to probe.
    #[must_use]
    pub fn is_up_to_date(&self) -> bool {
        self.to_probe == 0
    }
}

/// Progress of a build, reported after every Bundle and on pause changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexProgress<'a> {
    /// Bundles finished in this run, failures included.
    pub done: usize,
    /// Bundles this run probes.
    pub total: usize,
    /// Of `done`, those that failed.
    pub failed: usize,
    /// Bundles reused from the stored index.
    pub reused: usize,
    /// Bytes downloaded so far.
    pub bytes_fetched: u64,
    /// Range requests made so far.
    pub requests: u64,
    /// Whether the build is paused.
    pub paused: bool,
    /// The Bundle just finished, if this report is for one.
    pub bundle: Option<&'a str>,
}

/// One Bundle a build could not index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexFailure {
    pub bundle: String,
    pub error: String,
    /// Whether the failure was stored, so the Bundle is not probed again at
    /// this Content Hash (unless [`IndexOptions::retry_failed`]). Network
    /// failures are not stored and are retried on the next run.
    pub stored: bool,
}

/// What a build did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IndexReport {
    pub asset_version: AssetVersion,
    pub plan: IndexPlan,
    /// Bundles probed successfully in this run.
    pub probed: usize,
    /// Of `probed`, those read from the download cache instead of the CDN.
    pub from_cache: usize,
    /// Bundles this run could not index.
    pub failed: Vec<IndexFailure>,
    /// Bytes downloaded by Range requests.
    pub bytes_fetched: u64,
    /// Range requests made.
    pub requests: u64,
    pub elapsed: Duration,
    /// Whether the run was cancelled before probing everything.
    pub cancelled: bool,
    /// How many times the index file was written.
    pub saves: usize,
    /// The last failed save while the run went on (the final save's failure
    /// is returned as an error instead).
    pub save_error: Option<String>,
    /// Set when the stored index could not be read and was started afresh.
    pub discarded_index: Option<String>,
    /// The index file.
    pub path: PathBuf,
}

impl IndexReport {
    /// Bundles finished per second of the run.
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "a rate for display")]
    pub fn bundles_per_second(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            (self.probed + self.failed.len()) as f64 / secs
        } else {
            0.0
        }
    }
}

/// The outcome of [`IndexBuilder::run`].
#[derive(Debug, Clone)]
pub struct IndexRun {
    pub report: IndexReport,
    /// The index of the Manifest as it stands after the run.
    pub index: AssetIndex,
}

/// Builds or updates the Asset Index of one Manifest.
///
/// Probes run on [`IndexOptions::parallel`] threads, each Bundle read from
/// the download cache when a full copy is there and otherwise range-fetched
/// from the CDN, starting with [`IndexOptions::first_probe_len`] bytes and
/// growing to what the prober asks for (never beyond the Package's size).
/// Progress is saved every [`IndexOptions::save_every`] Bundles, on pause and
/// at the end, so a later run resumes where this one stopped.
pub struct IndexBuilder<T> {
    cache: Arc<PackageCache<T>>,
    location: CdnLocation,
    platform: Platform,
    manifest: Arc<Manifest>,
    options: IndexOptions,
    path: PathBuf,
    pause: Arc<AtomicBool>,
}

impl<T: Transport + Send + Sync> IndexBuilder<T> {
    /// A builder for `manifest`, fetching through `cache`'s downloader (with
    /// its retries) and keeping the index at [`default_index_path`] under
    /// the cache root. Parallelism follows the downloader's.
    pub fn new(
        cache: Arc<PackageCache<T>>,
        location: CdnLocation,
        platform: Platform,
        manifest: impl Into<Arc<Manifest>>,
    ) -> Self {
        let path = default_index_path(cache.root(), platform);
        let options = IndexOptions {
            parallel: cache.downloader().options().parallel.max(1),
            ..IndexOptions::default()
        };
        Self {
            cache,
            location,
            platform,
            manifest: manifest.into(),
            options,
            path,
            pause: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A builder for the store's Manifest, cache and location.
    #[must_use]
    pub fn for_store(store: &PackageStore<T>) -> Self
    where
        T: 'static,
    {
        Self::new(
            Arc::clone(store.cache()),
            store.location().clone(),
            store.platform(),
            store.shared_manifest(),
        )
    }

    #[must_use]
    pub fn with_options(mut self, options: IndexOptions) -> Self {
        self.options = options;
        self
    }

    /// Keeps the index in `path` instead of the default.
    #[must_use]
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = path;
        self
    }

    /// Uses `flag` to pause: while it is set, no new probe starts, and the
    /// progress so far is saved. Clearing it resumes.
    #[must_use]
    pub fn with_pause_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.pause = flag;
        self
    }

    #[must_use]
    pub fn pause_flag(&self) -> &Arc<AtomicBool> {
        &self.pause
    }

    pub fn pause(&self) {
        self.pause.store(true, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.pause.store(false, Ordering::Relaxed);
    }

    #[must_use]
    pub fn options(&self) -> &IndexOptions {
        &self.options
    }

    /// The index file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// What [`IndexBuilder::run`] would probe, from the stored index.
    pub fn plan(&self) -> Result<IndexPlan, IndexError> {
        let file = IndexFile::load(&self.path, self.platform)?;
        Ok(self.plan_for(&file).0)
    }

    /// The index as stored, without probing anything.
    pub fn index(&self) -> Result<AssetIndex, IndexError> {
        AssetIndex::load(&self.path, self.platform, &self.manifest)
    }

    fn plan_for(&self, file: &IndexFile) -> (IndexPlan, Vec<&Package>) {
        let mut plan = IndexPlan {
            asset_version: self.manifest.asset_version,
            ..IndexPlan::default()
        };
        let mut jobs = Vec::new();
        for package in &self.manifest.packages {
            match package.kind {
                PackageKind::Bundle => plan.total_bundles += 1,
                PackageKind::AudioPackage => {
                    plan.audio_packages += 1;
                    continue;
                }
                PackageKind::Other(_) => continue,
            }
            let probe = match file.get(&package.name) {
                Some(r) if r.content_hash != package.content_hash => {
                    plan.changed += 1;
                    true
                }
                Some(r) if r.result.is_ok() => {
                    plan.indexed += 1;
                    self.options.reprobe_all
                }
                Some(_) => {
                    plan.failed += 1;
                    self.options.reprobe_all || self.options.retry_failed
                }
                None => {
                    plan.new += 1;
                    true
                }
            };
            if probe {
                jobs.push(package);
                plan.to_probe_size += package.size;
            }
        }
        plan.to_probe = jobs.len();
        (plan, jobs)
    }

    /// Probes every Bundle the stored index lacks at its current Content
    /// Hash, saving as it goes.
    ///
    /// `cancel` stops the run early: in-flight probes finish their current
    /// request, nothing new starts, and what was probed is saved. A Bundle
    /// that fails is reported and the run goes on. Errors are returned only
    /// when the final save fails.
    pub fn run(
        &self,
        progress: &(dyn Fn(&IndexProgress<'_>) + Sync),
        cancel: &AtomicBool,
    ) -> Result<IndexRun, IndexError> {
        let started = Instant::now();
        let (mut file, discarded_index) = match IndexFile::load(&self.path, self.platform) {
            Ok(file) => (file, None),
            Err(e) => (IndexFile::new(self.platform), Some(e.to_string())),
        };
        let (plan, jobs) = self.plan_for(&file);
        let pruned = file.retain_manifest(&self.manifest);
        let version_changed = file.asset_version() != self.manifest.asset_version;
        file.set_asset_version(self.manifest.asset_version);

        let mut report = IndexReport {
            asset_version: self.manifest.asset_version,
            plan,
            probed: 0,
            from_cache: 0,
            failed: Vec::new(),
            bytes_fetched: 0,
            requests: 0,
            elapsed: Duration::ZERO,
            cancelled: false,
            saves: 0,
            save_error: None,
            discarded_index,
            path: self.path.clone(),
        };
        let counters = Counters::default();
        let reused = plan.total_bundles - plan.to_probe;

        let mut unsaved = !jobs.is_empty()
            || pruned > 0
            || version_changed
            || report.discarded_index.is_some()
            || !self.path.exists();
        if !jobs.is_empty() {
            self.probe_all(
                &jobs,
                &mut file,
                &mut report,
                &counters,
                reused,
                &mut unsaved,
                progress,
                cancel,
            );
        }
        report.cancelled =
            cancel.load(Ordering::Relaxed) && report.probed + report.failed.len() < jobs.len();
        report.bytes_fetched = counters.bytes.load(Ordering::Relaxed);
        report.requests = counters.requests.load(Ordering::Relaxed);
        if unsaved {
            file.save(&self.path)?;
            report.saves += 1;
        }
        report.elapsed = started.elapsed();
        let index = AssetIndex::new(self.platform, &self.manifest, &file);
        Ok(IndexRun { report, index })
    }

    #[expect(clippy::too_many_arguments, reason = "one run's shared state")]
    fn probe_all(
        &self,
        jobs: &[&Package],
        file: &mut IndexFile,
        report: &mut IndexReport,
        counters: &Counters,
        reused: usize,
        unsaved: &mut bool,
        progress: &(dyn Fn(&IndexProgress<'_>) + Sync),
        cancel: &AtomicBool,
    ) {
        let next = AtomicUsize::new(0);
        let workers = self.options.parallel.clamp(1, jobs.len());
        let save_every = self.options.save_every.max(1);
        let (tx, rx) = mpsc::channel::<(usize, Result<Probed, Failure>)>();
        thread::scope(|scope| {
            for _ in 0..workers {
                let tx = tx.clone();
                let next = &next;
                scope.spawn(move || {
                    while self.wait_while_paused(cancel) {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(package) = jobs.get(i) else { break };
                        // A prober panic marks this Bundle unreadable
                        // instead of ending the run.
                        let result =
                            crate::guard::catch_panic(|| self.probe(package, counters, cancel))
                                .unwrap_or_else(|message| {
                                    Err(Failure::Unreadable(format!("prober panicked: {message}")))
                                });
                        if tx.send((i, result)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(tx);

            let mut since_save = 0usize;
            let mut was_paused = false;
            let emit = |report: &IndexReport, paused: bool, bundle: Option<&str>| {
                progress(&IndexProgress {
                    done: report.probed + report.failed.len(),
                    total: jobs.len(),
                    failed: report.failed.len(),
                    reused,
                    bytes_fetched: counters.bytes.load(Ordering::Relaxed),
                    requests: counters.requests.load(Ordering::Relaxed),
                    paused,
                    bundle,
                });
            };
            emit(report, false, None);
            loop {
                let paused = self.pause.load(Ordering::Relaxed);
                if paused != was_paused {
                    was_paused = paused;
                    if paused && *unsaved {
                        self.save(file, report, unsaved);
                        since_save = 0;
                    }
                    emit(report, paused, None);
                }
                let (i, result) = match rx.recv_timeout(POLL) {
                    Ok(message) => message,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let package = jobs[i];
                let record = |result| BundleRecord {
                    content_hash: package.content_hash,
                    result,
                };
                match result {
                    Ok(probed) => {
                        report.probed += 1;
                        report.from_cache += usize::from(probed.from_cache);
                        let contents = BundleContents::from_listing(&probed.listing);
                        file.insert(package.name.clone(), record(Ok(Arc::new(contents))));
                    }
                    Err(Failure::Cancelled) => continue,
                    Err(Failure::Unreadable(error)) => {
                        file.insert(package.name.clone(), record(Err(error.clone())));
                        report.failed.push(IndexFailure {
                            bundle: package.name.clone(),
                            error,
                            stored: true,
                        });
                    }
                    Err(Failure::Fetch(error)) => report.failed.push(IndexFailure {
                        bundle: package.name.clone(),
                        error,
                        stored: false,
                    }),
                }
                *unsaved = true;
                since_save += 1;
                if since_save >= save_every {
                    self.save(file, report, unsaved);
                    since_save = 0;
                }
                emit(report, was_paused, Some(&package.name));
            }
        });
    }

    fn save(&self, file: &IndexFile, report: &mut IndexReport, unsaved: &mut bool) {
        match file.save(&self.path) {
            Ok(()) => {
                report.saves += 1;
                *unsaved = false;
            }
            Err(e) => report.save_error = Some(e.to_string()),
        }
    }

    /// Blocks while paused; returns false once cancelled.
    fn wait_while_paused(&self, cancel: &AtomicBool) -> bool {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            if !self.pause.load(Ordering::Relaxed) {
                return true;
            }
            thread::sleep(POLL);
        }
    }

    /// Lists one Bundle's Assets from a prefix of it.
    fn probe(
        &self,
        package: &Package,
        counters: &Counters,
        cancel: &AtomicBool,
    ) -> Result<Probed, Failure> {
        if let Some(mut source) = self.cached_file(package) {
            match probe_file(&mut source, package.size, self.options.first_probe_len) {
                Ok(listing) => {
                    return Ok(Probed {
                        listing,
                        from_cache: true,
                    });
                }
                // A cached copy that cannot be read (evicted meanwhile, or
                // damaged) is no verdict on the Bundle: ask the CDN.
                Err(Failure::Fetch(_) | Failure::Unreadable(_) | Failure::Cancelled) => {}
            }
        }
        let url = self
            .location
            .package_url(self.platform, package)
            .ok_or_else(|| Failure::Unreadable("not a Bundle".to_owned()))?;
        let listing = probe_network(
            &url,
            package.size,
            self.options.first_probe_len,
            &|start, end| {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Failure::Cancelled);
                }
                let response = self
                    .cache
                    .downloader()
                    .fetch_range(&url, start, end)
                    .map_err(|e| Failure::Fetch(e.to_string()))?;
                counters.requests.fetch_add(1, Ordering::Relaxed);
                counters
                    .bytes
                    .fetch_add(response.bytes.len() as u64, Ordering::Relaxed);
                Ok((response.bytes, response.total_len))
            },
        )?;
        Ok(Probed {
            listing,
            from_cache: false,
        })
    }

    /// The download cache's full copy of `package`, if it has one. Looked up
    /// without counting as a use, so indexing does not reorder eviction.
    fn cached_file(&self, package: &Package) -> Option<File> {
        let path = self.cache.path_for(self.platform, package).ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        (meta.is_file() && (package.size == 0 || meta.len() == package.size))
            .then(|| File::open(&path).ok())
            .flatten()
    }
}

#[derive(Default)]
struct Counters {
    bytes: AtomicU64,
    requests: AtomicU64,
}

struct Probed {
    listing: ProbeListing,
    from_cache: bool,
}

enum Failure {
    /// The run was cancelled.
    Cancelled,
    /// The bytes could not be fetched; worth retrying later.
    Fetch(String),
    /// The Bundle's bytes could not be listed; retrying will not help.
    Unreadable(String),
}

/// Reads ever longer prefixes of a cached file until the prober lists it.
fn probe_file(file: &mut File, size: u64, first: u64) -> Result<ProbeListing, Failure> {
    let total = match size {
        0 => file
            .metadata()
            .map_err(|e| Failure::Fetch(e.to_string()))?
            .len(),
        n => n,
    };
    let mut buf = Vec::new();
    let mut want = first.clamp(1, total.max(1));
    for _ in 0..MAX_ROUNDS {
        let missing = want.saturating_sub(buf.len() as u64);
        let read = file
            .by_ref()
            .take(missing)
            .read_to_end(&mut buf)
            .map_err(|e| Failure::Fetch(e.to_string()))?;
        if read as u64 != missing {
            return Err(Failure::Fetch(
                "cached file is shorter than expected".to_owned(),
            ));
        }
        match next_round(&buf, total)? {
            Round::Done(listing) => return Ok(listing),
            Round::Grow(at_least) => want = at_least,
        }
    }
    Err(too_many_rounds())
}

/// Range-fetches ever longer prefixes of a Bundle until the prober lists it.
/// `fetch(start, end_inclusive)` returns the bytes and the resource's total
/// length when the server reports it.
fn probe_network(
    url: &str,
    size: u64,
    first: u64,
    fetch: &RangeFetch<'_>,
) -> Result<ProbeListing, Failure> {
    // The server's length, when it reports one, wins over the Manifest's.
    let mut total = (size > 0).then_some(size);
    let mut buf: Vec<u8> = Vec::new();
    let mut want = total.map_or(first, |t| first.clamp(1, t));
    for _ in 0..MAX_ROUNDS {
        let start = buf.len() as u64;
        let (bytes, reported) = fetch(start, want.max(start + 1) - 1)?;
        if let Some(t) = reported {
            total = Some(t);
        }
        let Some(total) = total else {
            return Err(Failure::Fetch(format!("{url}: size unknown")));
        };
        if bytes.is_empty() {
            return Err(Failure::Fetch(format!(
                "{url}: empty response at byte {start}"
            )));
        }
        buf.extend_from_slice(&bytes);
        buf.truncate(usize::try_from(total).unwrap_or(usize::MAX));
        match next_round(&buf, total)? {
            Round::Done(listing) => return Ok(listing),
            Round::Grow(at_least) => want = at_least,
        }
    }
    Err(too_many_rounds())
}

/// Fetches bytes `start..=end_inclusive`, returning them and the resource's
/// total length when the server reports it.
type RangeFetch<'a> = dyn Fn(u64, u64) -> Result<(Vec<u8>, Option<u64>), Failure> + 'a;

enum Round {
    Done(ProbeListing),
    Grow(u64),
}

fn next_round(prefix: &[u8], total: u64) -> Result<Round, Failure> {
    match probe_prefix(prefix, total) {
        Ok(Probe::Listed(listing)) => Ok(Round::Done(listing)),
        Ok(Probe::NeedMore { at_least }) if at_least > prefix.len() as u64 => {
            Ok(Round::Grow(at_least.min(total)))
        }
        Ok(Probe::NeedMore { .. }) => Err(Failure::Unreadable(
            "prober asked for no more bytes than it had".to_owned(),
        )),
        Err(e) => Err(Failure::Unreadable(e.to_string())),
    }
}

fn too_many_rounds() -> Failure {
    Failure::Unreadable(format!("not listed after {MAX_ROUNDS} prefix rounds"))
}
