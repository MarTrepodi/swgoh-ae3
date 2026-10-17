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
