//! Offline doubles shared by the unit tests.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::http::{HttpError, RangeResponse, Transport};

/// A thread-safe in-memory CDN. URLs not registered answer 404; queued
/// failures are returned, one per request, before the real body.
#[derive(Default)]
pub struct FakeTransport {
    files: Mutex<HashMap<String, Vec<u8>>>,
    failures: Mutex<HashMap<String, VecDeque<HttpError>>>,
    requests: Mutex<Vec<String>>,
    ranges: Mutex<Vec<(String, u64, u64)>>,
    before_request: Mutex<Option<RequestHook>>,
}

type RequestHook = Arc<dyn Fn(&str) + Send + Sync>;

impl FakeTransport {
    pub fn with_file(self, url: &str, body: &[u8]) -> Self {
        self.put(url, body);
        self
    }

    pub fn put(&self, url: &str, body: &[u8]) {
        self.files
            .lock()
            .unwrap()
            .insert(url.to_owned(), body.to_vec());
    }

    /// Makes the next request for `url` fail with `err`.
    pub fn fail_next(&self, url: &str, err: HttpError) {
        self.failures
            .lock()
            .unwrap()
            .entry(url.to_owned())
            .or_default()
            .push_back(err);
    }

    pub fn requests_for(&self, url: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|u| *u == url)
            .count()
    }

    /// The `(start, end_inclusive)` of every Range GET of `url`, in order.
    pub fn ranges_for(&self, url: &str) -> Vec<(u64, u64)> {
        self.ranges
            .lock()
            .unwrap()
            .iter()
            .filter(|(u, ..)| u == url)
            .map(|&(_, start, end)| (start, end))
            .collect()
    }

    pub fn total_requests(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// Calls `hook` with the URL of every later request before answering
    /// it, on the requesting thread; it may block to hold that request.
    pub fn before_request(&self, hook: impl Fn(&str) + Send + Sync + 'static) {
        *self.before_request.lock().unwrap() = Some(Arc::new(hook));
    }

    fn lookup(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        // Cloned out so a blocking hook does not hold the lock.
        let hook = self.before_request.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(url);
        }
        self.requests.lock().unwrap().push(url.to_owned());
        if let Some(err) = self
            .failures
            .lock()
            .unwrap()
            .get_mut(url)
            .and_then(VecDeque::pop_front)
        {
            return Err(err);
        }
        self.files
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| HttpError::Status {
                url: url.to_owned(),
                status: 404,
            })
    }
}

impl Transport for FakeTransport {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.lookup(url)
    }

    fn get_range(
        &self,
        url: &str,
        start: u64,
        end_inclusive: u64,
    ) -> Result<RangeResponse, HttpError> {
        self.ranges
            .lock()
            .unwrap()
            .push((url.to_owned(), start, end_inclusive));
        let body = self.lookup(url)?;
        let len = body.len() as u64;
        if start >= len {
            return Err(HttpError::Status {
                url: url.to_owned(),
                status: 416,
            });
        }
        let end = end_inclusive.min(len - 1);
        let (start, end) = (
            usize::try_from(start).unwrap(),
            usize::try_from(end).unwrap(),
        );
        Ok(RangeResponse {
            bytes: body[start..=end].to_vec(),
            total_len: Some(len),
        })
    }

    fn exists(&self, url: &str) -> Result<bool, HttpError> {
        match self.lookup(url) {
            Ok(_) => Ok(true),
            Err(HttpError::Status {
                status: 403 | 404, ..
            }) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// A unique directory under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ae3-unit-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn transient(url: &str) -> HttpError {
    HttpError::Transport {
        url: url.to_owned(),
        message: "connection reset".to_owned(),
    }
}

/// The Asset Version 100054 Windows fixtures: `$AE3_FIXTURES/100054/Windows`,
/// defaulting to `<workspace>/fixtures`.
pub fn fixtures_dir() -> PathBuf {
    std::env::var_os("AE3_FIXTURES")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            PathBuf::from,
        )
        .join("100054")
        .join("Windows")
}

/// A fixture file's bytes, or `None` (after a note on stderr) when absent,
/// so tests that need it can skip.
pub fn fixture(name: &str) -> Option<Vec<u8>> {
    let path = fixtures_dir().join(name);
    if let Ok(bytes) = fs::read(&path) {
        Some(bytes)
    } else {
        eprintln!("skipping: fixture {} not found", path.display());
        None
    }
}
