//! Diffs and cache keys over the real Manifest for Asset Version 100054.
//!
//! The fixture is located through `AE3_FIXTURES` (default `<workspace>/fixtures`)
//! and the tests skip when it is absent.

use std::path::PathBuf;

use ae3_core::cache::CacheKey;
use ae3_core::{AssetVersion, Diff, DiffFilter, Manifest, PackageKind, Platform};

fn fixture_manifest() -> Option<Manifest> {
    let root = std::env::var_os("AE3_FIXTURES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
        PathBuf::from,
    );
    let path = root.join("100054/Windows/manifest.data");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipping: fixture {} is absent", path.display());
        return None;
    };
    Some(Manifest::decode(&bytes).expect("decode fixture manifest"))
}

#[test]
fn identical_manifests_have_an_empty_diff() {
    let Some(m) = fixture_manifest() else { return };
    let d = Diff::compute(&m, &m);
    assert!(d.is_empty());
    assert_eq!(d.shared, m.packages.len());
    assert!(!d.is_engine_rebuild());
    assert_eq!(d.changed_ratio(), 0.0);
}

#[test]
fn mutated_copy_reports_each_change() {
    let Some(old) = fixture_manifest() else {
        return;
    };
    assert_eq!(old.asset_version, AssetVersion(100_054));
    let mut new = old.clone();
    new.asset_version = AssetVersion(100_055);

    let bundle_idx: Vec<usize> = new
        .packages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == PackageKind::Bundle)
        .map(|(i, _)| i)
        .collect();
    let audio_idx = new
        .packages
        .iter()
        .position(|p| p.kind == PackageKind::AudioPackage)
        .expect("the fixture lists Audio Packages");

    // Change five Bundles and one Audio Package.
    for &i in &bundle_idx[..5] {
        new.packages[i].content_hash ^= 1;
        new.packages[i].size += 100;
    }
    new.packages[audio_idx].content_hash ^= 1;
    // Remove three Bundles (from the end, so the indices above stay valid).
    let removed: Vec<_> = bundle_idx[bundle_idx.len() - 3..]
        .iter()
        .rev()
        .map(|&i| new.packages.remove(i))
        .collect();
    // Add two Packages.
    let mut added = old.packages[bundle_idx[10]].clone();
    added.name = "zz_new_bundle".into();
    added.size = 1_000;
    let mut added_audio = old.packages[audio_idx].clone();
    added_audio.name = "audio_zz_new".into();
    added_audio.size = 2_000;
    new.packages.extend([added, added_audio]);

    let d = Diff::compute(&old, &new);
    assert_eq!(d.added.len(), 2);
    assert_eq!(d.changed.len(), 6);
    assert_eq!(d.removed.len(), 3);
    assert_eq!(d.added_size(), 3_000);
    let changed_expected: u64 = bundle_idx[..5]
        .iter()
        .map(|&i| old.packages[i].size + 100)
        .sum::<u64>()
        + old.packages[audio_idx].size;
    assert_eq!(d.changed_size(), changed_expected);
    assert_eq!(
        d.removed_size(),
        removed.iter().map(|p| p.size).sum::<u64>()
    );
    assert_eq!(
        d.download_size(DiffFilter::All),
        d.added_size() + d.changed_size()
    );
    assert_eq!(d.shared, old.packages.len() - 3);
    assert_eq!(d.shared_bundles, bundle_idx.len() - 3);
    assert_eq!(d.changed_bundles, 5);
    assert!(!d.is_engine_rebuild());
    assert!(
        d.changed
            .iter()
            .any(|c| c.new.kind == PackageKind::AudioPackage)
    );
}

#[test]
fn rehashing_every_bundle_is_an_engine_rebuild() {
    let Some(old) = fixture_manifest() else {
        return;
    };
    let mut new = old.clone();
    for p in &mut new.packages {
        if p.kind == PackageKind::Bundle {
            p.content_hash = p.content_hash.wrapping_add(1);
        }
    }
    let d = Diff::compute(&old, &new);
    assert!(d.is_engine_rebuild());
    assert!((d.changed_ratio() - 1.0).abs() < f64::EPSILON);
    assert!(d.added.is_empty() && d.removed.is_empty());
}

#[test]
fn every_fixture_package_has_a_cache_key() {
    let Some(m) = fixture_manifest() else { return };
    for p in &m.packages {
        let key = CacheKey::for_package(Platform::Windows, p)
            .unwrap_or_else(|e| panic!("{}: {e}", p.name));
        assert!(
            matches!(
                key.file_name.rsplit_once('.'),
                Some((_, "bundle" | "wwpkg"))
            ),
            "{}",
            key.file_name
        );
    }
}
