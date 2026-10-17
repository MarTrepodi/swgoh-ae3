//! The Asset Index through the public API, over the real Asset Version 100054
//! Manifest entries of the fixture Bundles, served by an offline Transport
//! that only answers Range requests. Skips when the fixtures are absent.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ae3_core::http::{HttpError, RangeResponse, Transport};
use ae3_core::{
    AssetIndex, AssetKind, AssetVersion, CdnLocation, Downloader, Environment, IndexBuilder,
    Manifest, PackageCache, PackageKind, PackageStore, Platform, SearchHit, VersionSource,
};

const BUNDLES: &[&str] = &[
    "charui_bossk",
    "abilityui_passive_summon",
    "vanity_bossk",
    "relic_rfl_bossk",
    "territoryui_view_standard_atlas",
    "unit_details_atlas",
    "shared_uicontainer",
    "char_bossk_pre",
];

fn fixtures_dir() -> PathBuf {
    std::env::var_os("AE3_FIXTURES")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            PathBuf::from,
        )
        .join("100054")
        .join("Windows")
}

/// Serves Range requests from the fixtures and refuses whole-file GETs, so
/// a test fails if indexing ever downloads a full Bundle.
struct RangeOnlyCdn {
    dir: PathBuf,
    fetched: AtomicU64,
    ranges: Mutex<Vec<(String, u64, u64)>>,
}

impl Transport for RangeOnlyCdn {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        Err(HttpError::Transport {
            url: url.to_owned(),
            message: "whole-file GET while indexing".to_owned(),
        })
    }

    fn get_range(&self, url: &str, start: u64, end: u64) -> Result<RangeResponse, HttpError> {
        let name = url.rsplit('/').next().unwrap_or_default();
        let body = fs::read(self.dir.join(name)).map_err(|_| HttpError::Status {
            url: url.to_owned(),
            status: 404,
        })?;
        self.ranges
            .lock()
            .unwrap()
            .push((name.to_owned(), start, end));
        let len = body.len() as u64;
        let from = usize::try_from(start.min(len)).unwrap();
        let to = usize::try_from(end.saturating_add(1).min(len)).unwrap();
        self.fetched
            .fetch_add((to - from) as u64, Ordering::Relaxed);
        Ok(RangeResponse {
            bytes: body[from..to].to_vec(),
            total_len: Some(len),
        })
    }

    fn exists(&self, url: &str) -> Result<bool, HttpError> {
        Ok(self.get_range(url, 0, 0).is_ok())
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    store: PackageStore<Arc<RangeOnlyCdn>>,
    cdn: Arc<RangeOnlyCdn>,
    total_size: u64,
    _cache_dir: TempDir,
}

/// A store over the fixture Bundles' real Manifest entries plus one Audio
/// Package, or `None` (skip) when fixtures are absent.
fn fixture() -> Option<Fixture> {
    let dir = fixtures_dir();
    let needed: Vec<String> = std::iter::once("manifest.data".to_owned())
        .chain(BUNDLES.iter().map(|b| format!("{b}.bundle")))
        .collect();
    if let Some(missing) = needed.iter().find(|f| !dir.join(f).exists()) {
        eprintln!("skipping: fixture {missing} not found in {}", dir.display());
        return None;
    }
    let full = Manifest::decode(&fs::read(dir.join("manifest.data")).unwrap()).unwrap();
    let mut manifest = Manifest {
        packages: full
            .packages
            .iter()
            .filter(|p| BUNDLES.contains(&p.name.as_str()) || p.name == "audio_dataprimer")
            .cloned()
            .collect(),
        ..full
    };
    // Sizes come from the files, in case the Manifest's differ.
    for p in &mut manifest.packages {
        if let Ok(meta) = fs::metadata(dir.join(p.file_name().unwrap())) {
            p.size = meta.len();
        }
    }
    let bundle_count = manifest
        .packages
        .iter()
        .filter(|p| p.kind == PackageKind::Bundle)
        .count();
    assert_eq!(bundle_count, BUNDLES.len());
    let total_size: u64 = manifest.packages.iter().map(|p| p.size).sum();

    let cdn = Arc::new(RangeOnlyCdn {
        dir: dir.clone(),
        fetched: AtomicU64::new(0),
        ranges: Mutex::new(Vec::new()),
    });
    let cache_dir =
        TempDir(std::env::temp_dir().join(format!("ae3-index-it-{}", std::process::id())));
    let cache = PackageCache::open(
        cache_dir.0.clone(),
        u64::MAX,
        Downloader::new(Arc::clone(&cdn)),
    )
    .unwrap();
    let location = CdnLocation {
        asset_version: AssetVersion(100_054),
        environment: Environment::new("PROD").unwrap(),
        base_url: "https://cdn.test/PROD/".into(),
        source: VersionSource::Explicit,
    };
    let store = PackageStore::new(Arc::new(cache), location, Platform::Windows, manifest);
    Some(Fixture {
        store,
        cdn,
        total_size,
        _cache_dir: cache_dir,
    })
}

#[test]
fn indexes_the_fixture_bundles_and_searches_them() {
    let Some(Fixture {
        store,
        cdn,
        total_size,
        _cache_dir,
    }) = fixture()
    else {
        return;
    };
    let builder = IndexBuilder::for_store(&store);
    assert_eq!(builder.plan().unwrap().to_probe, BUNDLES.len());

    let run = builder.run(&|_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(run.report.failed, []);
    assert_eq!(run.report.probed, BUNDLES.len());
    let fetched = cdn.fetched.load(Ordering::Relaxed);
    assert_eq!(run.report.bytes_fetched, fetched);
    assert!(fetched < total_size / 10, "{fetched} of {total_size}");

    let charui: Vec<_> = cdn
        .ranges
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.0 == "charui_bossk.bundle")
        .map(|r| (r.1, r.2))
        .collect();
    assert_eq!(charui, vec![(0, 4095)], "only a prefix of charui_bossk");

    // The index persists in the cache and reloads for search.
    let index = AssetIndex::open(store.cache(), Platform::Windows, store.manifest()).unwrap();
    let coverage = index.coverage();
    assert!(coverage.is_complete());
    assert_eq!(coverage.audio_packages, 1);

    let bossk = index.search("BOSSK");
    let names: Vec<_> = bossk.hits.iter().map(SearchHit::package_name).collect();
    assert_eq!(
        names,
        vec![
            "char_bossk_pre",
            "charui_bossk",
            "relic_rfl_bossk",
            "vanity_bossk"
        ]
    );
    assert!(bossk.hits.iter().all(|h| h.name_matched));
    let prefab = &bossk.hits[0];
    assert!(prefab.assets.iter().any(|a| a.name == "bossk_mesh"));

    // shared_uicontainer is found through its Assets only.
    let standard = index.search("standard_atlas");
    let shared = standard
        .hits
        .iter()
        .find(|h| h.package_name() == "shared_uicontainer")
        .expect("found by Asset name");
    assert!(!shared.name_matched);
    assert!(
        shared
            .assets
            .iter()
            .any(|a| a.name == "standard_atlas" && a.kind == AssetKind::Atlas)
    );
    assert!(
        standard
            .hits
            .iter()
            .any(|h| h.package_name() == "territoryui_view_standard_atlas" && h.name_matched)
    );
    assert!(!index.search("dataprimer").is_empty(), "Audio by name");
    assert!(
        index
            .kinds_of("unit_details_atlas")
            .unwrap()
            .contains(AssetKind::Atlas)
    );

    // A second run over the same Manifest fetches nothing.
    let before = cdn.fetched.load(Ordering::Relaxed);
    let again = IndexBuilder::for_store(&store)
        .run(&|_| {}, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(again.report.probed, 0);
    assert_eq!(cdn.fetched.load(Ordering::Relaxed), before);
}
