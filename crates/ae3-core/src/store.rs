//! The facade the CLI and GUI open Packages and run exports through.

use std::collections::HashMap;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use ae3_unity::Bundle;

use crate::audio::LoadedAudioPackage;
use crate::bundle::{BundlePool, DependencySource, LoadedBundle};
use crate::cache::PackageCache;
use crate::download::Progress;
use crate::error::AssetError;
use crate::export::{Batch, ExportOptions, ExportReport, ProgressFn, SkipReason, SkippedAsset};
use crate::http::Transport;
use crate::manifest::{Manifest, Package, PackageKind};
use crate::platform::Platform;
use crate::version::CdnLocation;

/// What a batch export covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Every Package in the Manifest: Bundles and Audio Packages.
    All,
    /// Every Bundle.
    AllBundles,
    /// Every Audio Package.
    AllAudio,
    /// Every Package of a Category, such as `charui` (case-insensitive).
    Category(String),
    /// These Packages, by name (`charui_bossk`, `charui_bossk.bundle` or
    /// `audio_x.wwpkg`).
    Packages(Vec<String>),
    /// Only these Assets of one Bundle.
    Assets { package: String, path_ids: Vec<i64> },
    /// Only the sounds with these Wwise ids of one Audio Package.
    Sounds { package: String, ids: Vec<u64> },
}

/// What part of one Package a batch exports.
enum Part<'s> {
    Whole,
    Assets(&'s [i64]),
    Sounds(&'s [u64]),
}

/// Opens the Packages of one Asset Version and Platform through the
/// [`PackageCache`], and exports them.
///
/// Dependency Bundles are shared by every Bundle opened through one store and
/// stay parsed until [`PackageStore::release_dependencies`].
pub struct PackageStore<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Clone for PackageStore<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct Inner<T> {
    cache: Arc<PackageCache<T>>,
    location: CdnLocation,
    platform: Platform,
    manifest: Arc<Manifest>,
    by_name: HashMap<String, usize>,
    pool: BundlePool,
}

impl<T: Transport + Send + Sync + 'static> PackageStore<T> {
    /// A store over an already fetched Manifest.
    pub fn new(
        cache: Arc<PackageCache<T>>,
        location: CdnLocation,
        platform: Platform,
        manifest: impl Into<Arc<Manifest>>,
    ) -> Self {
        let manifest = manifest.into();
        let by_name = manifest
            .packages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), i))
            .collect();
        Self {
            inner: Arc::new(Inner {
                cache,
                location,
                platform,
                manifest,
                by_name,
                pool: BundlePool::default(),
            }),
        }
    }

    /// A store for `location`'s Asset Version on `platform`, fetching the
    /// Manifest through the cache.
    pub fn open(
        cache: Arc<PackageCache<T>>,
        location: CdnLocation,
        platform: Platform,
    ) -> Result<Self, AssetError> {
        let manifest = cache.get_or_fetch_manifest(&location, platform)?;
        Ok(Self::new(cache, location, platform, manifest))
    }

    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.inner.manifest
    }

    #[must_use]
    pub fn location(&self) -> &CdnLocation {
        &self.inner.location
    }

    #[must_use]
    pub fn platform(&self) -> Platform {
        self.inner.platform
    }

    #[must_use]
    pub fn cache(&self) -> &Arc<PackageCache<T>> {
        &self.inner.cache
    }

    /// The Package named `name`; a `.bundle` or `.wwpkg` suffix is allowed.
    #[must_use]
    pub fn package(&self, name: &str) -> Option<&Package> {
        self.inner.package(name)
    }

    /// Opens a Bundle Package. It and its direct Manifest dependencies are
    /// fetched (in parallel) now; dependencies are parsed when a reference
    /// first needs them, and their own dependencies fetched then.
    pub fn open_bundle(&self, name: &str) -> Result<LoadedBundle, AssetError> {
        self.open_bundle_with_progress(name, &|_| {})
    }

    /// [`PackageStore::open_bundle`] with download progress.
    pub fn open_bundle_with_progress(
        &self,
        name: &str,
        progress: &(dyn Fn(&Progress) + Sync),
    ) -> Result<LoadedBundle, AssetError> {
        let inner = &self.inner;
        let package = inner.package_of_kind(name, PackageKind::Bundle)?;
        let mut wanted = vec![package];
        wanted.extend(
            package
                .dependencies
                .iter()
                .filter_map(|d| inner.package(d))
                .filter(|p| p.kind == PackageKind::Bundle),
        );
        // A dependency that fails to download is reported when (and if) a
        // reference needs it.
        let mut fetched = inner
            .cache
            .get_or_fetch_all(&inner.location, inner.platform, &wanted, progress)
            .into_iter();
        let path = fetched
            .next()
            .ok_or_else(|| AssetError::PackageNotFound(name.to_owned()))??;
        let bytes = fs::read(&path).map_err(AssetError::io(&path))?;
        let loaded = LoadedBundle::from_bytes(package.name.clone(), &bytes)?;
        let source: Arc<dyn DependencySource> = Arc::clone(&self.inner) as _;
        Ok(loaded.with_dependencies(package.dependencies.clone(), source))
    }

    /// Opens an Audio Package.
    pub fn open_audio_package(&self, name: &str) -> Result<LoadedAudioPackage, AssetError> {
        let inner = &self.inner;
        let package = inner.package_of_kind(name, PackageKind::AudioPackage)?;
        let path = inner
            .cache
            .get_or_fetch(&inner.location, inner.platform, package)?;
        let bytes = fs::read(&path).map_err(AssetError::io(&path))?;
        LoadedAudioPackage::from_bytes(package.name.clone(), &bytes)
    }

    /// Drops the parsed dependency Bundles (and remembered failures) to free
    /// memory; they are loaded again when next needed.
    pub fn release_dependencies(&self) {
        self.inner.pool.clear();
    }

    /// The Packages a selection covers, in Manifest order (or the order
    /// given). Unknown names are an error.
    pub fn select(&self, selection: &Selection) -> Result<Vec<&Package>, AssetError> {
        Ok(self.jobs(selection)?.into_iter().map(|(p, _)| p).collect())
    }

    fn jobs<'s, 'q>(
        &'s self,
        selection: &'q Selection,
    ) -> Result<Vec<(&'s Package, Part<'q>)>, AssetError> {
        let packages = &self.inner.manifest.packages;
        let whole = |filter: &dyn Fn(&Package) -> bool| {
            packages
                .iter()
                .filter(|p| filter(p))
                .map(|p| (p, Part::Whole))
                .collect()
        };
        Ok(match selection {
            Selection::All => whole(&|_| true),
            Selection::AllBundles => whole(&|p| p.kind == PackageKind::Bundle),
            Selection::AllAudio => whole(&|p| p.kind == PackageKind::AudioPackage),
            Selection::Category(category) => {
                whole(&|p| p.category().eq_ignore_ascii_case(category))
            }
            Selection::Packages(names) => {
                let mut seen = std::collections::HashSet::new();
                let mut jobs = Vec::with_capacity(names.len());
                for name in names {
                    let package = self
                        .package(name)
                        .ok_or_else(|| AssetError::PackageNotFound(name.clone()))?;
                    if seen.insert(&package.name) {
                        jobs.push((package, Part::Whole));
                    }
                }
                jobs
            }
            Selection::Assets { package, path_ids } => {
                let package = self.inner.package_of_kind(package, PackageKind::Bundle)?;
                vec![(package, Part::Assets(path_ids))]
            }
            Selection::Sounds { package, ids } => {
                let package = self
                    .inner
                    .package_of_kind(package, PackageKind::AudioPackage)?;
                vec![(package, Part::Sounds(ids))]
            }
        })
    }

    /// Exports a selection. Packages are processed `options.parallel` at a
    /// time; failures of single Packages or Assets are recorded in the report
    /// and do not stop the batch. Setting `cancel` stops it at the next
    /// Asset. Only an unknown Package name in the selection is an error.
    pub fn export(
        &self,
        selection: &Selection,
        options: &ExportOptions,
        progress: ProgressFn<'_>,
        cancel: &AtomicBool,
    ) -> Result<ExportReport, AssetError> {
        let jobs = self.jobs(selection)?;
        let batch = Batch::new(options, progress, cancel, jobs.len());
        let report = Mutex::new(ExportReport::default());
        let next = AtomicUsize::new(0);
        let work = || {
            while !batch.is_cancelled() {
                let Some((package, part)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                let done = self.export_one(&batch, package, part);
                report
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .merge(done);
            }
        };
        let workers = options.parallel.clamp(1, jobs.len().max(1));
        if workers == 1 {
            work();
        } else {
            thread::scope(|s| {
                for _ in 0..workers {
                    s.spawn(work);
                }
            });
        }
        let mut report = report.into_inner().unwrap_or_else(PoisonError::into_inner);
        report.cancelled |= report.packages_done < jobs.len();
        report.sort();
        Ok(report)
    }

    fn export_one(&self, batch: &Batch<'_>, package: &Package, part: &Part<'_>) -> ExportReport {
        let name = package.name.as_str();
        batch.package_started(name);
        let mut report = match package.kind {
            PackageKind::Bundle => match self.open_bundle(name) {
                Ok(bundle) => batch.export_bundle(
                    &bundle,
                    match part {
                        Part::Assets(ids) => Some(*ids),
                        _ => None,
                    },
                ),
                Err(e) => failed(name, &e),
            },
            PackageKind::AudioPackage => match self.open_audio_package(name) {
                Ok(audio) => batch.export_audio(
                    &audio,
                    match part {
                        Part::Sounds(ids) => Some(*ids),
                        _ => None,
                    },
                ),
                Err(e) => failed(name, &e),
            },
            PackageKind::Other(kind) => ExportReport {
                skipped: vec![SkippedAsset {
                    package: name.to_owned(),
                    asset: name.to_owned(),
                    path_id: None,
                    reason: SkipReason::Unsupported {
                        detail: format!("unknown Package kind {kind}"),
                    },
                }],
                ..ExportReport::default()
            },
        };
        if !report.cancelled {
            batch.package_finished(name, &mut report);
        }
        report
    }
}

fn failed(package: &str, error: &AssetError) -> ExportReport {
    ExportReport {
        errors: vec![crate::export::ExportFailure {
            package: package.to_owned(),
            asset: None,
            path_id: None,
            error: error.to_string(),
        }],
        ..ExportReport::default()
    }
}

impl<T> Inner<T> {
    fn package(&self, name: &str) -> Option<&Package> {
        let index = self.by_name.get(name).or_else(|| {
            let bare = name
                .strip_suffix(".bundle")
                .or_else(|| name.strip_suffix(".wwpkg"))?;
            self.by_name.get(bare)
        })?;
        self.manifest.packages.get(*index)
    }

    fn package_of_kind(&self, name: &str, kind: PackageKind) -> Result<&Package, AssetError> {
        let package = self
            .package(name)
            .ok_or_else(|| AssetError::PackageNotFound(name.to_owned()))?;
        if package.kind == kind {
            Ok(package)
        } else {
            Err(AssetError::WrongPackageKind {
                name: package.name.clone(),
                expected: match kind {
                    PackageKind::Bundle => "Bundle",
                    _ => "Audio Package",
                },
            })
        }
    }
}

impl<T: Transport + Send + Sync> DependencySource for Inner<T> {
    fn load(&self, name: &str) -> Result<Arc<Bundle>, AssetError> {
        self.pool.get_or_load(name, || {
            let package = self.package_of_kind(name, PackageKind::Bundle)?;
            let path = self
                .cache
                .get_or_fetch(&self.location, self.platform, package)?;
            let bytes = fs::read(&path).map_err(AssetError::io(&path))?;
            Bundle::parse(&bytes).map_err(AssetError::unity(name))
        })
    }

    fn dependencies_of(&self, name: &str) -> Vec<String> {
        self.package(name)
            .map(|p| p.dependencies.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::{DownloadOptions, Downloader};
    use crate::export::ExportEvent;
    use crate::test_support::{FakeTransport, TempDir};
    use crate::version::{AssetVersion, Environment, VersionSource};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn handles_are_shareable_across_threads() {
        assert_send_sync::<PackageStore<crate::http::UreqTransport>>();
        assert_send_sync::<LoadedBundle>();
        assert_send_sync::<LoadedAudioPackage>();
    }

    fn package(name: &str, kind: PackageKind) -> Package {
        Package {
            name: name.into(),
            kind,
            size: 4,
            content_hash: 1,
            ..Package::default()
        }
    }

    fn store(dir: &TempDir) -> PackageStore<Arc<FakeTransport>> {
        let location = CdnLocation {
            asset_version: AssetVersion(1),
            environment: Environment::new("PROD").unwrap(),
            base_url: "https://cdn.test/PROD/".into(),
            source: VersionSource::Explicit,
        };
        let transport = Arc::new(FakeTransport::default());
        // Not a Bundle, so opening it fails after a successful download.
        transport.put(
            &location.file_url(Platform::Windows, "charui_broken.bundle"),
            b"junk",
        );
        let downloader = Downloader::with_options(
            transport,
            DownloadOptions {
                attempts: 1,
                ..DownloadOptions::default()
            },
        );
        let cache = PackageCache::open(dir.path().to_path_buf(), u64::MAX, downloader).unwrap();
        let manifest = Manifest {
            packages: vec![
                package("charui_broken", PackageKind::Bundle),
                package("charui_missing", PackageKind::Bundle),
                package("charui_future", PackageKind::Other(9)),
                package("audio_x", PackageKind::AudioPackage),
            ],
            ..Manifest::default()
        };
        PackageStore::new(Arc::new(cache), location, Platform::Windows, manifest)
    }

    #[test]
    fn selections_pick_packages() {
        let dir = TempDir::new("store-select");
        let store = store(&dir);
        let names = |s: &Selection| -> Vec<String> {
            store
                .select(s)
                .unwrap()
                .iter()
                .map(|p| p.name.clone())
                .collect()
        };
        assert_eq!(names(&Selection::All).len(), 4);
        assert_eq!(names(&Selection::AllAudio), ["audio_x"]);
        assert_eq!(names(&Selection::AllBundles).len(), 2);
        assert_eq!(names(&Selection::Category("CHARUI".into())).len(), 3);
        assert_eq!(
            names(&Selection::Packages(vec![
                "audio_x.wwpkg".into(),
                "charui_broken.bundle".into(),
                "audio_x".into()
            ])),
            ["audio_x", "charui_broken"]
        );
        assert!(matches!(
            store.select(&Selection::Assets {
                package: "audio_x".into(),
                path_ids: vec![]
            }),
            Err(AssetError::WrongPackageKind { .. })
        ));
        assert!(matches!(
            store.select(&Selection::Packages(vec!["nope".into()])),
            Err(AssetError::PackageNotFound(_))
        ));
    }

    #[test]
    fn failing_packages_do_not_stop_the_batch() {
        let dir = TempDir::new("store-export");
        let out = TempDir::new("store-export-out");
        let store = store(&dir);
        let options = ExportOptions {
            parallel: 3,
            ..ExportOptions::new(out.path())
        };
        let started = AtomicUsize::new(0);
        let report = store
            .export(
                &Selection::Category("charui".into()),
                &options,
                &|p| {
                    if matches!(p.event, ExportEvent::PackageStarted { .. }) {
                        started.fetch_add(1, Ordering::Relaxed);
                    }
                },
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(started.load(Ordering::Relaxed), 3);
        assert_eq!(report.packages_done, 3);
        assert!(!report.cancelled);
        let failed: Vec<&str> = report.errors.iter().map(|e| e.package.as_str()).collect();
        assert_eq!(failed, ["charui_broken", "charui_missing"]);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].package, "charui_future");
        assert_eq!(report.written.len(), 0);
    }
}
