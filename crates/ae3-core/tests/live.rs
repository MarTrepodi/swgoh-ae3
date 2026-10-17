//! End-to-end checks against the real CDN. They need the network, so they are
//! ignored by default: run with `cargo test -- --ignored`.

use ae3_core::http::{Transport, UreqTransport};
use ae3_core::{Manifest, PackageKind, Platform, Resolver};

#[test]
#[ignore = "needs network access"]
fn resolves_and_decodes_the_live_manifest() {
    let transport = UreqTransport::new();
    let location = Resolver::new(&transport, Platform::Windows, None)
        .resolve(None)
        .expect("resolve the latest Asset Version");
    let buf = transport
        .get_bytes(&location.manifest_url(Platform::Windows))
        .expect("download manifest.data");
    let manifest = Manifest::decode(&buf).expect("decode manifest.data");

    assert_eq!(manifest.asset_version, location.asset_version);
    assert_eq!(manifest.platform, "Windows");
    assert!(
        manifest.packages.len() > 1_000,
        "{}",
        manifest.packages.len()
    );
    assert!(
        manifest
            .packages
            .iter()
            .any(|p| p.kind == PackageKind::AudioPackage)
    );
    // Live Manifests omit the Entry lists, which is why the Asset Index has to
    // be built by probing Bundles (ADR-0007). Dependencies are present.
    assert!(manifest.packages.iter().any(|p| !p.dependencies.is_empty()));
}

#[test]
#[ignore = "needs network access"]
fn range_probe_tells_published_from_unpublished_versions() {
    let transport = UreqTransport::new();
    let mut location = Resolver::new(&transport, Platform::Windows, None)
        .resolve(None)
        .expect("resolve the latest Asset Version");
    assert!(
        transport
            .exists(&location.manifest_url(Platform::Windows))
            .unwrap()
    );
    location.asset_version.0 += 1_000;
    assert!(
        !transport
            .exists(&location.manifest_url(Platform::Windows))
            .unwrap()
    );
}

#[test]
#[ignore = "needs network access"]
fn range_get_reads_a_window_and_the_total_length() {
    let transport = UreqTransport::new();
    let location = Resolver::new(&transport, Platform::Windows, None)
        .resolve(None)
        .expect("resolve the latest Asset Version");
    let url = location.manifest_url(Platform::Windows);
    let whole = transport.get_bytes(&url).expect("download manifest.data");
    let window = transport.get_range(&url, 2, 9).expect("range GET");
    assert_eq!(window.bytes, whole[2..=9]);
    assert_eq!(window.total_len, Some(whole.len() as u64));
}

#[test]
#[ignore = "needs network access"]
fn cache_downloads_a_small_package_once() {
    use ae3_core::{Downloader, PackageCache};

    let transport = UreqTransport::new();
    let location = Resolver::new(&transport, Platform::Windows, None)
        .resolve(None)
        .expect("resolve the latest Asset Version");
    let root = std::env::temp_dir().join(format!("ae3-live-cache-{}", std::process::id()));
    let cache =
        PackageCache::open(root.clone(), 1 << 30, Downloader::new(&transport)).expect("open cache");
    let manifest = cache
        .get_or_fetch_manifest(&location, Platform::Windows)
        .expect("manifest");
    let small = manifest
        .packages
        .iter()
        .filter(|p| p.kind == PackageKind::Bundle && p.size > 0)
        .min_by_key(|p| p.size)
        .expect("a Bundle");
    let path = cache
        .get_or_fetch(&location, Platform::Windows, small)
        .expect("download");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), small.size);
    assert!(cache.cached(Platform::Windows, small).is_some());
    drop(cache);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "needs network access"]
fn indexes_fifty_live_bundles_from_prefixes() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use ae3_core::{Downloader, IndexBuilder, PackageCache};

    let transport = Arc::new(UreqTransport::new());
    let location = Resolver::new(&*transport, Platform::Windows, None)
        .resolve(None)
        .expect("resolve the latest Asset Version");
    let root = std::env::temp_dir().join(format!("ae3-live-index-{}", std::process::id()));
    let cache = Arc::new(
        PackageCache::open(
            root.clone(),
            1 << 30,
            Downloader::new(Arc::clone(&transport)),
        )
        .expect("open cache"),
    );
    let full = cache
        .get_or_fetch_manifest(&location, Platform::Windows)
        .expect("manifest");
    // Every n-th Bundle, so the sample spans Categories and sizes.
    let bundles: Vec<_> = full
        .packages
        .iter()
        .filter(|p| p.kind == PackageKind::Bundle)
        .collect();
    let step = (bundles.len() / 50).max(1);
    let sample = Manifest {
        packages: bundles
            .iter()
            .step_by(step)
            .take(50)
            .map(|p| (*p).clone())
            .collect(),
        ..full.clone()
    };
    let total_size: u64 = sample.packages.iter().map(|p| p.size).sum();

    let builder = IndexBuilder::new(
        Arc::clone(&cache),
        location.clone(),
        Platform::Windows,
        sample.clone(),
    );
    let run = builder
        .run(&|_| {}, &AtomicBool::new(false))
        .expect("index run");
    let report = &run.report;
    eprintln!(
        "indexed {} Bundles in {:.2?} ({:.1}/s), {} requests, {} KiB fetched of {} KiB total; {} failed",
        report.probed,
        report.elapsed,
        report.bundles_per_second(),
        report.requests,
        report.bytes_fetched / 1024,
        total_size / 1024,
        report.failed.len(),
    );
    for failure in &report.failed {
        eprintln!("  {}: {}", failure.bundle, failure.error);
    }
    let index_size = std::fs::metadata(&report.path).map_or(0, |m| m.len());
    eprintln!("index file: {index_size} bytes");
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.probed, sample.packages.len());
    assert!(run.index.coverage().is_complete());
    assert!(
        report.bytes_fetched < total_size,
        "only prefixes are fetched"
    );
    let assets: usize = run.index.packages().iter().map(|p| p.assets().len()).sum();
    assert!(assets >= sample.packages.len(), "{assets} Assets");
    let first = &sample.packages[0].name;
    assert!(!run.index.search(first).is_empty());
    drop(cache);
    let _ = std::fs::remove_dir_all(root);
}
