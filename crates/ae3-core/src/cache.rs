//! The on-disk download cache.
//!
//! Packages are stored content-addressed by (Platform, Package file name,
//! Content Hash), so a Package unchanged between Asset Versions is downloaded
//! once. Their total size is capped; the least recently used are evicted
//! first, with recency kept in a small persistent index. Manifests are kept
//! per (Environment, Asset Version, Platform) indefinitely and do not count
//! towards the cap.
//!
//! Layout under the cache root (by default [`paths::default_cache_dir`]):
//!
//! ```text
//! packages/index.json
//! packages/<Platform>/<name>.<content hash as 16 hex digits>.<bundle|wwpkg>
//! manifests/<Environment>/<Platform>/<Asset Version>.data
//! ```
//!
//! The index is rebuilt from the files on [`PackageCache::open`], so a lost
//! or stale index only loses recency, never cached data. In no-cache mode
//! ([`PackageCache::disabled`]) downloads go to a private temporary directory
//! that is deleted when the cache is dropped.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::Config;
use crate::download::{
    DownloadError, DownloadJob, DownloadOptions, Downloader, PART_SUFFIX, Progress,
};
use crate::http::Transport;
use crate::manifest::{Manifest, Package};
use crate::paths;
use crate::platform::Platform;
use crate::version::CdnLocation;
use crate::wire::DecodeError;

/// The default cap on cached Package bytes: 5 GB.
pub const DEFAULT_CACHE_CAP: u64 = 5_000_000_000;

const PACKAGES_DIR: &str = "packages";
const MANIFESTS_DIR: &str = "manifests";
const INDEX_FILE: &str = "index.json";
const INDEX_FORMAT: u32 = 1;
/// The index is written at most this often while Packages are fetched, and
/// always when the cache is flushed or dropped.
const PERSIST_INTERVAL: Duration = Duration::from_secs(2);
/// Leftover partial downloads older than this are deleted on open; younger
/// ones may belong to another running AE3.
const STALE_PART_AGE: Duration = Duration::from_hours(1);

#[derive(Debug, Error)]
pub enum CacheError {
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("Package {name:?} has unknown kind {kind}, so its file name is unknown")]
    UnknownPackageKind { name: String, kind: u64 },
    #[error("Package name {0:?} is not safe to use as a file name")]
    InvalidPackageName(String),
    #[error("{url}: not a valid Manifest: {source}")]
    Decode {
        url: String,
        #[source]
        source: DecodeError,
    },
    #[error("no cache directory: neither the configuration nor the environment names one")]
    NoCacheDir,
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> CacheError + '_ {
    move |source| CacheError::Io {
        path: path.to_owned(),
        source,
    }
}

/// What identifies one cached Package file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey {
    pub platform: Platform,
    /// The CDN file name, such as `charui_bossk.bundle`.
    pub file_name: String,
    pub content_hash: u64,
}

impl CacheKey {
    pub fn for_package(platform: Platform, package: &Package) -> Result<Self, CacheError> {
        if !is_safe_name(&package.name) {
            return Err(CacheError::InvalidPackageName(package.name.clone()));
        }
        let file_name = package
            .file_name()
            .ok_or_else(|| CacheError::UnknownPackageKind {
                name: package.name.clone(),
                kind: package.kind.code(),
            })?;
        Ok(Self {
            platform,
            file_name,
            content_hash: package.content_hash,
        })
    }

    /// The file's path relative to the `packages` directory.
    fn relative_path(&self) -> PathBuf {
        let (stem, ext) = self
            .file_name
            .rsplit_once('.')
            .unwrap_or((&self.file_name, ""));
        Path::new(self.platform.name()).join(format!("{stem}.{:016x}.{ext}", self.content_hash))
    }

    /// The inverse of [`CacheKey::relative_path`] for one stored file.
    fn parse(platform: Platform, stored: &str) -> Option<Self> {
        let (rest, ext) = stored.rsplit_once('.')?;
        let (stem, hash) = rest.rsplit_once('.')?;
        if hash.len() != 16 || !is_safe_name(stem) || ext.is_empty() {
            return None;
        }
        Some(Self {
            platform,
            file_name: format!("{stem}.{ext}"),
            content_hash: u64::from_str_radix(hash, 16).ok()?,
        })
    }
}

/// Names come from the CDN's Manifest, so anything that could escape the
/// cache directory or confuse the file system is refused.
fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// A summary of the cache, for `ae3 cache`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub root: PathBuf,
    /// False in no-cache mode.
    pub persistent: bool,
    pub packages: usize,
    pub package_bytes: u64,
    pub cap_bytes: u64,
    /// Package count and bytes per Platform, in [`Platform::ALL`] order,
    /// omitting Platforms with nothing cached.
    pub by_platform: Vec<(Platform, usize, u64)>,
    pub manifests: usize,
    pub manifest_bytes: u64,
}

/// What a prune or clear removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub removed: usize,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    size: u64,
    /// A logical clock: higher is more recently used.
    used: u64,
}

struct State {
    entries: HashMap<CacheKey, Entry>,
    total: u64,
    clock: u64,
    dirty: bool,
    last_persist: Instant,
}

impl State {
    fn touch(&mut self, key: &CacheKey, size: u64) {
        self.clock += 1;
        let entry = Entry {
            size,
            used: self.clock,
        };
        if let Some(old) = self.entries.insert(key.clone(), entry) {
            self.total -= old.size;
        }
        self.total += size;
        self.dirty = true;
    }

    fn forget(&mut self, key: &CacheKey) {
        if let Some(old) = self.entries.remove(key) {
            self.total -= old.size;
            self.dirty = true;
        }
    }
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    format: u32,
    entries: Vec<IndexRecord>,
}

#[derive(Serialize, Deserialize)]
struct IndexRecord {
    platform: String,
    file: String,
    hash: u64,
    used: u64,
}

/// The download cache. See the module documentation.
pub struct PackageCache<T> {
    downloader: Downloader<T>,
    root: PathBuf,
    cap: u64,
    persistent: bool,
    state: Mutex<State>,
}

impl<T: Transport> PackageCache<T> {
    /// Opens (creating if needed) the cache at `root`, capped at `cap` bytes
    /// of Packages.
    pub fn open(root: PathBuf, cap: u64, downloader: Downloader<T>) -> Result<Self, CacheError> {
        let packages = root.join(PACKAGES_DIR);
        fs::create_dir_all(&packages).map_err(io_error(&packages))?;
        let state = load_state(&packages);
        let cache = Self {
            downloader,
            root,
            cap,
            persistent: true,
            state: Mutex::new(state),
        };
        // Enforce a cap that may have been lowered since the last run.
        let mut state = cache.lock();
        cache.evict_locked(&mut state, cap, &|_| false);
        drop(state);
        Ok(cache)
    }

    /// Opens the cache in [`paths::default_cache_dir`].
    pub fn open_default(cap: u64, downloader: Downloader<T>) -> Result<Self, CacheError> {
        let root = paths::default_cache_dir().ok_or(CacheError::NoCacheDir)?;
        Self::open(root, cap, downloader)
    }

    /// No-cache mode: nothing is reused across runs. Packages are downloaded
    /// into a private temporary directory, which is deleted on drop, and
    /// Manifests are not stored.
    pub fn disabled(downloader: Downloader<T>) -> Result<Self, CacheError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir().join(format!(
            "ae3-nocache-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let packages = root.join(PACKAGES_DIR);
        fs::create_dir_all(&packages).map_err(io_error(&packages))?;
        Ok(Self {
            downloader,
            root,
            cap: u64::MAX,
            persistent: false,
            state: Mutex::new(empty_state()),
        })
    }

    /// Opens the cache the configuration describes: disabled when
    /// `no_cache` is set, otherwise at `cache_dir` (or the default) with
    /// `cache_cap`, downloading `parallel_downloads` at a time.
    pub fn from_config(config: &Config, transport: T) -> Result<Self, CacheError> {
        let downloader = Downloader::with_options(
            transport,
            DownloadOptions {
                parallel: config.parallel_downloads.max(1),
                ..DownloadOptions::default()
            },
        );
        if config.no_cache {
            return Self::disabled(downloader);
        }
        let root = config
            .cache_dir
            .clone()
            .or_else(paths::default_cache_dir)
            .ok_or(CacheError::NoCacheDir)?;
        Self::open(root, config.cache_cap, downloader)
    }

    #[must_use]
    pub fn downloader(&self) -> &Downloader<T> {
        &self.downloader
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// False in no-cache mode.
    #[must_use]
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// Where `package` is cached for `platform`, whether or not it is there.
    pub fn path_for(&self, platform: Platform, package: &Package) -> Result<PathBuf, CacheError> {
        Ok(self.key_path(&CacheKey::for_package(platform, package)?))
    }

    /// The cached file for `package`, if present and of the Manifest's size,
    /// without downloading. A hit counts as a use for eviction.
    pub fn cached(&self, platform: Platform, package: &Package) -> Option<PathBuf> {
        let key = CacheKey::for_package(platform, package).ok()?;
        let path = self.key_path(&key);
        self.check_hit(&key, &path, package.size).then_some(path)
    }

    /// The cached file for `package`, downloading it from `location` first if
    /// it is not cached. The returned path stays valid until the Package is
    /// evicted to make room for others, pruned, or (in no-cache mode) the
    /// cache is dropped.
    pub fn get_or_fetch(
        &self,
        location: &CdnLocation,
        platform: Platform,
        package: &Package,
    ) -> Result<PathBuf, CacheError> {
        let key = CacheKey::for_package(platform, package)?;
        let path = self.key_path(&key);
        if self.check_hit(&key, &path, package.size) {
            return Ok(path);
        }
        let job = download_job(location, platform, package, &path);
        let size = self.downloader.download(&job)?;
        let mut state = self.lock();
        state.touch(&key, size);
        self.evict_locked(&mut state, self.cap, &|k| *k == key);
        let _ = self.maybe_persist(&mut state, false);
        Ok(path)
    }

    /// The Manifest of `location`'s Asset Version for `platform`, from the
    /// cache or else downloaded and cached.
    pub fn get_or_fetch_manifest(
        &self,
        location: &CdnLocation,
        platform: Platform,
    ) -> Result<Manifest, CacheError> {
        let url = location.manifest_url(platform);
        let decode = |bytes: &[u8]| {
            Manifest::decode(bytes).map_err(|source| CacheError::Decode {
                url: url.clone(),
                source,
            })
        };
        let path = self.persistent.then(|| {
            self.root
                .join(MANIFESTS_DIR)
                .join(location.environment.as_str())
                .join(platform.name())
                .join(format!("{}.data", location.asset_version))
        });
        if let Some(path) = &path
            && let Ok(bytes) = fs::read(path)
        {
            match decode(&bytes) {
                Ok(manifest) => return Ok(manifest),
                // A corrupt copy is replaced by a fresh download.
                Err(_) => drop(fs::remove_file(path)),
            }
        }
        let bytes = self.downloader.fetch(&url, None)?;
        let manifest = decode(&bytes)?;
        if let Some(path) = &path {
            write_atomically(path, &bytes)?;
        }
        Ok(manifest)
    }
}

/// Bookkeeping that needs no Transport, so `Drop` can flush the index.
impl<T> PackageCache<T> {
    /// Evicts least recently used Packages until at most `max_bytes` remain.
    pub fn prune(&self, max_bytes: u64) -> Result<PruneReport, CacheError> {
        let mut state = self.lock();
        let report = self.evict_locked(&mut state, max_bytes, &|_| false);
        self.maybe_persist(&mut state, true)?;
        Ok(report)
    }

    /// Removes every cached Package and Manifest.
    pub fn clear(&self) -> Result<PruneReport, CacheError> {
        let mut report = self.prune(0)?;
        let manifests = self.root.join(MANIFESTS_DIR);
        for (path, size) in manifest_files(&manifests) {
            if fs::remove_file(&path).is_ok() {
                report.removed += 1;
                report.freed_bytes += size;
            }
        }
        let _ = fs::remove_dir_all(&manifests);
        Ok(report)
    }

    #[must_use]
    pub fn stats(&self) -> CacheStats {
        let state = self.lock();
        let by_platform = Platform::ALL
            .into_iter()
            .filter_map(|platform| {
                let (count, bytes) = state
                    .entries
                    .iter()
                    .filter(|(k, _)| k.platform == platform)
                    .fold((0, 0), |(c, b), (_, e)| (c + 1, b + e.size));
                (count > 0).then_some((platform, count, bytes))
            })
            .collect();
        let manifests = manifest_files(&self.root.join(MANIFESTS_DIR));
        CacheStats {
            root: self.root.clone(),
            persistent: self.persistent,
            packages: state.entries.len(),
            package_bytes: state.total,
            cap_bytes: self.cap,
            by_platform,
            manifests: manifests.len(),
            manifest_bytes: manifests.iter().map(|(_, size)| size).sum(),
        }
    }

    /// Writes the index now if anything changed since it was last written.
    pub fn flush(&self) -> Result<(), CacheError> {
        let mut state = self.lock();
        self.maybe_persist(&mut state, true)
    }

    fn key_path(&self, key: &CacheKey) -> PathBuf {
        self.root.join(PACKAGES_DIR).join(key.relative_path())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `path` holds a complete copy (`expected` 0 means the Manifest
    /// gives no size). Files placed by another AE3 process are adopted.
    fn check_hit(&self, key: &CacheKey, path: &Path, expected: u64) -> bool {
        let size = fs::metadata(path)
            .ok()
            .filter(|m| m.is_file() && (expected == 0 || m.len() == expected))
            .map(|m| m.len());
        let mut state = self.lock();
        if let Some(size) = size {
            state.touch(key, size);
            let _ = self.maybe_persist(&mut state, false);
            true
        } else {
            state.forget(key);
            false
        }
    }

    fn evict_locked(
        &self,
        state: &mut State,
        target: u64,
        protect: &dyn Fn(&CacheKey) -> bool,
    ) -> PruneReport {
        let mut report = PruneReport::default();
        if state.total <= target {
            return report;
        }
        let mut candidates: Vec<(u64, CacheKey)> = state
            .entries
            .iter()
            .filter(|(k, _)| !protect(k))
            .map(|(k, e)| (e.used, k.clone()))
            .collect();
        candidates.sort_unstable();
        for (_, key) in candidates {
            if state.total <= target {
                break;
            }
            match fs::remove_file(self.key_path(&key)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                // Locked or read-only: keep it and try the next one.
                Err(_) => continue,
            }
            let size = state.entries[&key].size;
            state.forget(&key);
            report.removed += 1;
            report.freed_bytes += size;
        }
        report
    }

    fn maybe_persist(&self, state: &mut State, force: bool) -> Result<(), CacheError> {
        if !self.persistent
            || !state.dirty
            || (!force && state.last_persist.elapsed() < PERSIST_INTERVAL)
        {
            return Ok(());
        }
        let mut entries: Vec<IndexRecord> = state
            .entries
            .iter()
            .map(|(k, e)| IndexRecord {
                platform: k.platform.name().to_owned(),
                file: k.file_name.clone(),
                hash: k.content_hash,
                used: e.used,
            })
            .collect();
        entries.sort_unstable_by_key(|r| r.used);
        let json = serde_json::to_vec(&IndexFile {
            format: INDEX_FORMAT,
            entries,
        })
        .map_err(|e| io_error(&self.root)(io::Error::other(e)))?;
        write_atomically(&self.root.join(PACKAGES_DIR).join(INDEX_FILE), &json)?;
        state.dirty = false;
        state.last_persist = Instant::now();
        Ok(())
    }

    /// Writes the index and closes the cache.
    pub fn close(self) -> Result<(), CacheError> {
        self.flush()
    }
}

impl<T: Transport + Sync> PackageCache<T> {
    /// [`PackageCache::get_or_fetch`] for many Packages, downloading the
    /// missing ones in parallel. Results are in input order. `progress`
    /// covers only the Packages that had to be downloaded.
    pub fn get_or_fetch_all(
        &self,
        location: &CdnLocation,
        platform: Platform,
        packages: &[&Package],
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Vec<Result<PathBuf, CacheError>> {
        let mut results: Vec<Option<Result<PathBuf, CacheError>>> =
            packages.iter().map(|_| None).collect();
        let mut jobs = Vec::new();
        let mut pending = Vec::new();
        for (i, package) in packages.iter().enumerate() {
            let key = match CacheKey::for_package(platform, package) {
                Ok(key) => key,
                Err(e) => {
                    results[i] = Some(Err(e));
                    continue;
                }
            };
            let path = self.key_path(&key);
            if self.check_hit(&key, &path, package.size) {
                results[i] = Some(Ok(path));
            } else {
                jobs.push(download_job(location, platform, package, &path));
                pending.push((i, key, path));
            }
        }
        let outcomes = self.downloader.download_all(&jobs, progress);
        let mut state = self.lock();
        let batch: HashSet<&CacheKey> = pending.iter().map(|(_, k, _)| k).collect();
        for ((i, key, path), outcome) in pending.iter().zip(outcomes) {
            results[*i] = Some(match outcome {
                Ok(size) => {
                    state.touch(key, size);
                    Ok(path.clone())
                }
                Err(e) => Err(e.into()),
            });
        }
        self.evict_locked(&mut state, self.cap, &|k| batch.contains(k));
        let _ = self.maybe_persist(&mut state, true);
        drop(state);
        results
            .into_iter()
            .map(|r| r.unwrap_or(Err(CacheError::Download(DownloadError::Cancelled))))
            .collect()
    }
}

impl<T> Drop for PackageCache<T> {
    fn drop(&mut self) {
        if self.persistent {
            let _ = self.flush();
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn download_job(
    location: &CdnLocation,
    platform: Platform,
    package: &Package,
    dest: &Path,
) -> DownloadJob {
    // CacheKey::for_package has already checked the kind has a file name.
    let url = location.package_url(platform, package).unwrap_or_default();
    DownloadJob::new(url, dest).with_expected_size((package.size > 0).then_some(package.size))
}

fn empty_state() -> State {
    State {
        entries: HashMap::new(),
        total: 0,
        clock: 0,
        dirty: false,
        last_persist: Instant::now(),
    }
}

/// Rebuilds the state from the files on disk, taking recency from the index.
fn load_state(packages: &Path) -> State {
    let recency: HashMap<CacheKey, u64> = fs::read(packages.join(INDEX_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<IndexFile>(&bytes).ok())
        .filter(|index| index.format == INDEX_FORMAT)
        .map(|index| {
            index
                .entries
                .into_iter()
                .filter_map(|r| {
                    let platform = r.platform.parse().ok()?;
                    let key = CacheKey {
                        platform,
                        file_name: r.file,
                        content_hash: r.hash,
                    };
                    Some((key, r.used))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut state = empty_state();
    for platform in Platform::ALL {
        let Ok(dir) = fs::read_dir(packages.join(platform.name())) else {
            continue;
        };
        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            if name.ends_with(PART_SUFFIX) {
                let is_old = meta
                    .modified()
                    .ok()
                    .and_then(|m| m.elapsed().ok())
                    .is_some_and(|age| age > STALE_PART_AGE);
                if is_old {
                    let _ = fs::remove_file(entry.path());
                }
                continue;
            }
            let Some(key) = CacheKey::parse(platform, name) else {
                continue;
            };
            let used = recency.get(&key).copied().unwrap_or(0);
            state.clock = state.clock.max(used);
            state.total += meta.len();
            state.entries.insert(
                key,
                Entry {
                    size: meta.len(),
                    used,
                },
            );
        }
    }
    state.dirty = state.entries.len() != recency.len();
    state
}

/// Every stored Manifest file and its size.
fn manifest_files(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if entry.path().extension().is_some_and(|e| e == "data") {
                out.push((entry.path(), meta.len()));
            }
        }
    }
    out
}

fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), CacheError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_error(dir))?;
    }
    let tmp = path.with_extension(format!("{}{PART_SUFFIX}", std::process::id()));
    fs::write(&tmp, contents).map_err(io_error(&tmp))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io_error(path)(e)
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::manifest::PackageKind;
    use crate::test_support::{FakeTransport, TempDir, transient};
    use crate::version::{AssetVersion, Environment, VersionSource};

    fn location(version: u64) -> CdnLocation {
        CdnLocation {
            asset_version: AssetVersion(version),
            environment: Environment::new("PROD").unwrap(),
            base_url: "https://cdn.test/PROD/".into(),
            source: VersionSource::Explicit,
        }
    }

    fn bundle(name: &str, hash: u64, size: u64) -> Package {
        Package {
            name: name.into(),
            content_hash: hash,
            size,
            ..Package::default()
        }
    }

    fn downloader(t: &FakeTransport) -> Downloader<&FakeTransport> {
        Downloader::with_options(
            t,
            DownloadOptions {
                parallel: 4,
                attempts: 3,
                initial_backoff: Duration::from_millis(1),
            },
        )
    }

    /// Publishes `package` with a body of its size at `version`.
    fn publish(t: &FakeTransport, version: u64, package: &Package) -> String {
        let url = location(version)
            .package_url(Platform::Windows, package)
            .unwrap();
        t.put(&url, &vec![b'x'; usize::try_from(package.size).unwrap()]);
        url
    }

    #[test]
    fn unchanged_package_downloads_once_across_asset_versions() {
        let t = FakeTransport::default();
        let p = bundle("charui_bossk", 0xabc, 10);
        let url1 = publish(&t, 1, &p);
        let url2 = publish(&t, 2, &p);
        let dir = TempDir::new("cache-once");
        let cache = PackageCache::open(dir.path().into(), 1_000, downloader(&t)).unwrap();
        let first = cache
            .get_or_fetch(&location(1), Platform::Windows, &p)
            .unwrap();
        let second = cache
            .get_or_fetch(&location(2), Platform::Windows, &p)
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(t.requests_for(&url1), 1);
        assert_eq!(t.requests_for(&url2), 0);
        assert!(first.ends_with("Windows/charui_bossk.0000000000000abc.bundle"));

        // A new Content Hash is a different cache entry.
        let changed = bundle("charui_bossk", 0xdef, 12);
        publish(&t, 2, &changed);
        let third = cache
            .get_or_fetch(&location(2), Platform::Windows, &changed)
            .unwrap();
        assert_ne!(first, third);
        assert_eq!(cache.stats().packages, 2);
        assert_eq!(cache.stats().package_bytes, 22);
    }

    #[test]
    fn retries_through_the_downloader() {
        let t = FakeTransport::default();
        let p = bundle("a", 1, 3);
        let url = publish(&t, 1, &p);
        t.fail_next(&url, transient(&url));
        let dir = TempDir::new("cache-retry");
        let cache = PackageCache::open(dir.path().into(), 1_000, downloader(&t)).unwrap();
        assert!(
            cache
                .get_or_fetch(&location(1), Platform::Windows, &p)
                .is_ok()
        );
        assert_eq!(t.requests_for(&url), 2);
    }

    #[test]
    fn evicts_least_recently_used_over_the_cap() {
        let t = FakeTransport::default();
        let pkgs: Vec<_> = (0..4).map(|i| bundle(&format!("b{i}"), i, 10)).collect();
        for p in &pkgs {
            publish(&t, 1, p);
        }
        let dir = TempDir::new("cache-lru");
        let cache = PackageCache::open(dir.path().into(), 30, downloader(&t)).unwrap();
        let loc = location(1);
        let fetch = |i: usize| {
            cache
                .get_or_fetch(&loc, Platform::Windows, &pkgs[i])
                .unwrap()
        };
        let p0 = fetch(0);
        let p1 = fetch(1);
        let p2 = fetch(2);
        // Use b0 again, so b1 becomes the least recently used.
        fetch(0);
        let p3 = fetch(3);
        assert!(p0.exists() && p2.exists() && p3.exists());
        assert!(!p1.exists());
        let stats = cache.stats();
        assert_eq!((stats.packages, stats.package_bytes), (3, 30));
        assert_eq!(stats.by_platform, vec![(Platform::Windows, 3, 30)]);

        let report = cache.prune(10).unwrap();
        assert_eq!(
            report,
            PruneReport {
                removed: 2,
                freed_bytes: 20
            }
        );
        assert!(p3.exists() && !p2.exists() && !p0.exists());
    }

    #[test]
    fn recency_and_contents_survive_reopening() {
        let t = FakeTransport::default();
        let pkgs: Vec<_> = (0..3).map(|i| bundle(&format!("b{i}"), i, 10)).collect();
        for p in &pkgs {
            publish(&t, 1, p);
        }
        let dir = TempDir::new("cache-reopen");
        let loc = location(1);
        {
            let cache = PackageCache::open(dir.path().into(), 100, downloader(&t)).unwrap();
            for i in [1, 0, 2] {
                cache
                    .get_or_fetch(&loc, Platform::Windows, &pkgs[i])
                    .unwrap();
            }
            cache.close().unwrap();
        }
        // A stray file and a stale index entry must not confuse the reopen.
        fs::write(dir.path().join("packages/Windows/notes.txt"), b"?").unwrap();
        let requests = t.total_requests();
        let cache = PackageCache::open(dir.path().into(), 100, downloader(&t)).unwrap();
        assert_eq!(cache.stats().packages, 3);
        assert!(cache.cached(Platform::Windows, &pkgs[2]).is_some());
        assert_eq!(t.total_requests(), requests);
        // b1 was used first, so it goes first.
        cache.prune(20).unwrap();
        assert!(cache.cached(Platform::Windows, &pkgs[1]).is_none());
        assert!(cache.cached(Platform::Windows, &pkgs[0]).is_some());
        drop(cache);

        // Reopening with a lower cap evicts immediately.
        let cache = PackageCache::open(dir.path().into(), 10, downloader(&t)).unwrap();
        assert_eq!(cache.stats().packages, 1);
    }

    #[test]
    fn index_is_rebuilt_from_files_when_lost() {
        let t = FakeTransport::default();
        let p = bundle("b", 7, 5);
        publish(&t, 1, &p);
        let dir = TempDir::new("cache-lost-index");
        {
            let cache = PackageCache::open(dir.path().into(), 100, downloader(&t)).unwrap();
            cache
                .get_or_fetch(&location(1), Platform::Windows, &p)
                .unwrap();
        }
        let _ = fs::remove_file(dir.path().join("packages").join(INDEX_FILE));
        let cache = PackageCache::open(dir.path().into(), 100, downloader(&t)).unwrap();
        assert_eq!(cache.stats().package_bytes, 5);
    }

    #[test]
    fn wrong_size_copy_is_refetched() {
        let t = FakeTransport::default();
        let p = bundle("b", 1, 4);
        let url = publish(&t, 1, &p);
        let dir = TempDir::new("cache-truncated");
        let cache = PackageCache::open(dir.path().into(), 100, downloader(&t)).unwrap();
        let path = cache
            .get_or_fetch(&location(1), Platform::Windows, &p)
            .unwrap();
        fs::write(&path, b"xx").unwrap();
        cache
            .get_or_fetch(&location(1), Platform::Windows, &p)
            .unwrap();
        assert_eq!(t.requests_for(&url), 2);
        assert_eq!(fs::read(&path).unwrap().len(), 4);
    }

    #[test]
    fn get_or_fetch_all_mixes_hits_misses_and_errors() {
        let t = FakeTransport::default();
        let pkgs: Vec<_> = (0..6).map(|i| bundle(&format!("b{i}"), i, 8)).collect();
        for p in &pkgs[..5] {
            publish(&t, 1, p);
        }
        let bad = bundle("../evil", 1, 1);
        let dir = TempDir::new("cache-all");
        let cache = PackageCache::open(dir.path().into(), 1_000, downloader(&t)).unwrap();
        let loc = location(1);
        cache
            .get_or_fetch(&loc, Platform::Windows, &pkgs[0])
            .unwrap();
        let mut refs: Vec<&Package> = pkgs.iter().collect();
        refs.push(&bad);
        let results = cache.get_or_fetch_all(&loc, Platform::Windows, &refs, &|_| {});
        assert!(results[..5].iter().all(Result::is_ok));
        assert!(matches!(results[5], Err(CacheError::Download(_))));
        assert!(matches!(results[6], Err(CacheError::InvalidPackageName(_))));
        assert_eq!(t.total_requests(), 6, "b0 was a hit, b5 missing");
        assert_eq!(cache.stats().packages, 5);
    }

    #[test]
    fn batch_larger_than_cap_keeps_the_batch() {
        let t = FakeTransport::default();
        let pkgs: Vec<_> = (0..3).map(|i| bundle(&format!("b{i}"), i, 10)).collect();
        for p in &pkgs {
            publish(&t, 1, p);
        }
        let dir = TempDir::new("cache-big-batch");
        let cache = PackageCache::open(dir.path().into(), 15, downloader(&t)).unwrap();
        let refs: Vec<&Package> = pkgs.iter().collect();
        let results = cache.get_or_fetch_all(&location(1), Platform::Windows, &refs, &|_| {});
        assert!(results.iter().all(|r| r.as_ref().is_ok_and(|p| p.exists())));
    }

    #[test]
    fn manifests_are_cached_per_version_and_platform() {
        let t = FakeTransport::default();
        // An empty buffer is a valid (empty) Manifest; field 1 sets the version.
        let body = [0x08, 0x05];
        let loc = location(5);
        let url = loc.manifest_url(Platform::Windows);
        t.put(&url, &body);
        let dir = TempDir::new("cache-manifest");
        let cache = PackageCache::open(dir.path().into(), 100, downloader(&t)).unwrap();
        let m = cache
            .get_or_fetch_manifest(&loc, Platform::Windows)
            .unwrap();
        assert_eq!(m.asset_version, AssetVersion(5));
        cache
            .get_or_fetch_manifest(&loc, Platform::Windows)
            .unwrap();
        assert_eq!(t.requests_for(&url), 1);
        assert!(
            cache
                .get_or_fetch_manifest(&loc, Platform::Android)
                .is_err()
        );
        let stats = cache.stats();
        assert_eq!((stats.manifests, stats.manifest_bytes), (1, 2));
        // Manifests are not subject to the Package cap.
        cache.prune(0).unwrap();
        assert_eq!(cache.stats().manifests, 1);
        let report = cache.clear().unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(cache.stats().manifests, 0);
    }

    #[test]
    fn corrupt_manifest_is_refetched() {
        let t = FakeTransport::default();
        let loc = location(5);
        let url = loc.manifest_url(Platform::Windows);
        t.put(&url, &[0x08, 0x05]);
        let dir = TempDir::new("cache-manifest-corrupt");
        let cache = PackageCache::open(dir.path().into(), 100, downloader(&t)).unwrap();
        cache
            .get_or_fetch_manifest(&loc, Platform::Windows)
            .unwrap();
        let stored = dir.path().join("manifests/PROD/Windows/5.data");
        fs::write(&stored, [0x08]).unwrap();
        let m = cache
            .get_or_fetch_manifest(&loc, Platform::Windows)
            .unwrap();
        assert_eq!(m.asset_version, AssetVersion(5));
        assert_eq!(t.requests_for(&url), 2);
    }

    #[test]
    fn no_cache_mode_keeps_nothing() {
        let t = FakeTransport::default();
        let p = bundle("b", 1, 3);
        publish(&t, 1, &p);
        let loc = location(1);
        t.put(&loc.manifest_url(Platform::Windows), &[]);
        let cache = PackageCache::disabled(downloader(&t)).unwrap();
        assert!(!cache.is_persistent());
        let root = cache.root().to_owned();
        let path = cache.get_or_fetch(&loc, Platform::Windows, &p).unwrap();
        assert!(path.starts_with(&root) && path.exists());
        cache
            .get_or_fetch_manifest(&loc, Platform::Windows)
            .unwrap();
        assert_eq!(cache.stats().manifests, 0);
        drop(cache);
        assert!(!root.exists());
    }

    #[test]
    fn audio_packages_and_unknown_kinds() {
        let audio = Package {
            kind: PackageKind::AudioPackage,
            ..bundle("audio_dataprimer", 1, 1)
        };
        let key = CacheKey::for_package(Platform::Ios, &audio).unwrap();
        assert_eq!(key.file_name, "audio_dataprimer.wwpkg");
        assert_eq!(
            CacheKey::parse(Platform::Ios, "audio_dataprimer.0000000000000001.wwpkg"),
            Some(key)
        );
        let other = Package {
            kind: PackageKind::Other(9),
            ..bundle("x", 1, 1)
        };
        assert!(matches!(
            CacheKey::for_package(Platform::Windows, &other),
            Err(CacheError::UnknownPackageKind { kind: 9, .. })
        ));
    }

    #[test]
    fn unsafe_names_are_refused() {
        for bad in ["", "..", "../x", "a/b", "a\\b", ".hidden", "a b", "c:x"] {
            assert!(!is_safe_name(bad), "{bad:?}");
        }
        assert!(is_safe_name("charui_bossk-2.v1"));
    }
}
