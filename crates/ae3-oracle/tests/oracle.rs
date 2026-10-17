//! The differential oracle (ADR-0006): every `Texture2D` of every fixture
//! Bundle, on every Platform, decoded by `ae3-core` and by `unity-rs-core`,
//! must agree pixel for pixel.
//!
//! The one declared tolerance is ASTC: `ae3-image` uses `texture2ddecoder`,
//! which converts an interpolated 16-bit channel to 8 bits by rounding
//! `* 255 / 65535` (the `AssetStudio` lineage), while `unity-rs-core` takes
//! the top byte as the ASTC specification and Khronos `astcenc` do. The two
//! differ by at most one per channel.
//!
//! Needs the `oracle` feature (which links C zstd) and is ignored by default:
//!
//! ```text
//! cargo test -p ae3-oracle --features oracle -- --ignored
//! ```

#![cfg(feature = "oracle")]

use std::collections::BTreeMap;

use ae3_core::Platform;
use ae3_core::ae3_image::TextureFormat;
use ae3_oracle::unity_rs::decode_with_unity_rs;
use ae3_oracle::{PixelDiff, decode_with_ae3, fixtures};

/// The largest per-channel difference allowed for `format`.
fn tolerance(format: Option<TextureFormat>) -> u8 {
    match format.map(TextureFormat::name) {
        Some(name) if name.starts_with("ASTC_") => 1,
        _ => 0,
    }
}

#[test]
#[ignore = "differential oracle: needs --features oracle, run with --ignored"]
fn every_fixture_texture_matches_unity_rs() {
    let mut failures = Vec::new();
    // (Platform, format) -> (textures compared, of which bit-exact).
    let mut compared = BTreeMap::<(Platform, String), (usize, usize)>::new();
    for platform in Platform::ALL {
        for path in fixtures::bundles(platform) {
            let bundle = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let ours = decode_with_ae3(&path).unwrap_or_else(|e| panic!("{bundle}: ae3: {e}"));
            let mut theirs =
                decode_with_unity_rs(&path).unwrap_or_else(|e| panic!("{bundle}: unity-rs: {e}"));
            for texture in ours {
                let what = format!(
                    "{platform} {bundle} {} ({})",
                    texture.name,
                    texture.format_name()
                );
                let Some(other) = theirs.remove(&texture.path_id) else {
                    failures.push(format!("{what}: unity-rs does not list it"));
                    continue;
                };
                match (&texture.image, &other) {
                    (Ok(a), Ok(b)) => match PixelDiff::between(a, b) {
                        Some(diff) if diff.max <= tolerance(texture.format) => {
                            let entry = compared
                                .entry((platform, texture.format_name()))
                                .or_default();
                            entry.0 += 1;
                            entry.1 += usize::from(diff.is_exact());
                        }
                        Some(diff) => failures.push(format!(
                            "{what}: max difference {}, mean {:.3}, {} of {} pixels differ",
                            diff.max, diff.mean, diff.differing, diff.pixels
                        )),
                        None => failures.push(format!(
                            "{what}: ae3 {}x{}, unity-rs {}x{}",
                            a.width(),
                            a.height(),
                            b.width(),
                            b.height()
                        )),
                    },
                    // Both refuse a texture stored empty (dynamic font textures).
                    (Err(_), Err(_)) if texture.width == 0 || texture.height == 0 => {}
                    (ours, theirs) => failures.push(format!(
                        "{what}: ae3 {:?}, unity-rs {:?}",
                        ours.as_ref().err(),
                        theirs.as_ref().err()
                    )),
                }
            }
            failures.extend(theirs.keys().map(|path_id| {
                format!("{platform} {bundle}: ae3 does not list Texture2D {path_id}")
            }));
        }
    }
    for ((platform, format), (count, exact)) in &compared {
        eprintln!("{platform} {format}: {count} textures agree ({exact} bit-exact)");
    }
    assert!(
        failures.is_empty(),
        "{} disagreements:\n{}",
        failures.len(),
        failures.join("\n")
    );
    if compared.is_empty() {
        assert!(
            !fixtures::required(),
            "no fixture Bundles found under {}",
            fixtures::root().display()
        );
        eprintln!("skipping: no fixture Bundles found");
    }
}
