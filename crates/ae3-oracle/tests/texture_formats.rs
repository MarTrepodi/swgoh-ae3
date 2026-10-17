//! Per-format checks of texture decoding against the real Asset Version
//! 100054 Bundles of every Platform.
//!
//! The same Bundle ships with different texture formats per Platform:
//! Windows uses DXT1, DXT5 and BC7, Android `ETC_RGB4` and `ETC2_RGBA8` (and
//! `ASTC_4x4` for a lightmap), iOS ASTC 4x4, 6x6 and 8x8 and `ETC2_RGB`; UI
//! Atlases and icons stay RGBA32 or RGB24 everywhere. That gives three
//! checks:
//!
//! - [`pinned_textures_per_format`]: one or two textures per format with
//!   their size, format and mean colour pinned.
//! - [`platforms_agree`]: every texture that ships on several Platforms
//!   looks the same on each, whatever the format, which catches a swapped
//!   channel, a missing flip or a wrong block layout in any one decoder.
//! - [`every_fixture_texture_decodes`]: every texture of every fixture
//!   Bundle decodes to its stated size.
//!
//! Agreement with the `unity-rs-core` oracle is in `tests/oracle.rs`.
//!
//! Fixtures come from `scripts/fetch-fixtures.sh`; tests skip what is absent
//! unless `AE3_REQUIRE_FIXTURES` is set.

use std::collections::{BTreeMap, BTreeSet};

use ae3_core::Platform;
use ae3_core::ae3_image::TextureFormat;
use ae3_oracle::{DecodedTexture, ImageStats, coarse_diff, decode_with_ae3, fixtures};

/// One texture whose decoded form is pinned.
struct Pinned {
    platform: Platform,
    bundle: &'static str,
    texture: &'static str,
    format: TextureFormat,
    size: (u32, u32),
    /// Mean R, G, B and A of the decoded image.
    mean: [f64; 4],
}

const fn pin(
    platform: Platform,
    bundle: &'static str,
    texture: &'static str,
    format: TextureFormat,
    size: (u32, u32),
    mean: [f64; 4],
) -> Pinned {
    Pinned {
        platform,
        bundle,
        texture,
        format,
        size,
        mean,
    }
}

/// How far a pinned mean may drift, per channel. Decoding is deterministic,
/// so this only absorbs rounding changes such as ASTC's 8-bit conversion.
const MEAN_TOLERANCE: f64 = 1.0;

#[rustfmt::skip]
const PINNED: &[Pinned] = {
    use Platform::{Android, Ios, Windows};
    use TextureFormat as F;
    &[
        pin(Windows, "charui_bossk", "tex.charui_bossk", F::Dxt1, (128, 128), [59.6, 52.3, 37.9, 255.0]),
        pin(Windows, "relic_rfl_bossk", "tex.relic_rfl_bossk", F::Dxt5, (256, 256), [121.9, 117.0, 129.0, 57.3]),
        pin(Windows, "char_bossk_pre", "bossk_tex", F::Bc7, (256, 256), [77.5, 62.0, 37.9, 254.7]),
        pin(Windows, "unit_details_atlas", "unit_details_atlas", F::Rgba32, (1024, 1024), [49.3, 76.0, 82.8, 45.3]),
        pin(Windows, "guild_icon_blast_small", "tex.guild_icon_blast_small", F::Rgb24, (64, 64), [35.6, 79.6, 176.4, 255.0]),
        pin(Android, "charui_bossk", "tex.charui_bossk", F::EtcRgb4, (128, 128), [59.3, 52.5, 38.3, 255.0]),
        pin(Android, "relic_rfl_bossk", "tex.relic_rfl_bossk", F::Etc2Rgba8, (256, 256), [122.1, 117.4, 129.4, 57.3]),
        pin(Android, "env_tat_dunes_jawa_pre", "Lightmap-0_comp_light", F::Astc4x4, (512, 512), [36.8, 32.2, 31.9, 255.0]),
        pin(Android, "unit_details_atlas", "unit_details_atlas", F::Rgba32, (1024, 1024), [49.3, 76.0, 82.8, 45.3]),
        pin(Android, "guild_icon_blast_small", "tex.guild_icon_blast_small", F::Rgb24, (64, 64), [35.6, 79.6, 176.4, 255.0]),
        pin(Ios, "charui_bossk", "tex.charui_bossk", F::Astc6x6, (128, 128), [59.7, 52.6, 38.3, 255.0]),
        pin(Ios, "char_bossk_pre", "bossk_tex", F::Astc4x4, (256, 256), [77.5, 62.0, 37.9, 255.0]),
        pin(Ios, "territory_backdrop_hoth_atat", "tex.territory_backdrop_hoth_atat", F::Astc8x8, (1024, 256), [124.6, 128.8, 142.4, 255.0]),
        pin(Ios, "pack_jp_ship_gacha", "tex.pack_jp_ship_gacha", F::Etc2Rgb, (256, 512), [61.6, 93.5, 124.1, 255.0]),
        pin(Ios, "unit_details_atlas", "unit_details_atlas", F::Rgba32, (1024, 1024), [49.3, 76.0, 82.8, 45.3]),
        pin(Ios, "guild_icon_blast_small", "tex.guild_icon_blast_small", F::Rgb24, (64, 64), [35.6, 79.6, 176.4, 255.0]),
    ]
};

/// Formats that carry no alpha: every decoded pixel must be opaque.
fn has_no_alpha(format: TextureFormat) -> bool {
    matches!(
        format,
        TextureFormat::Rgb24 | TextureFormat::EtcRgb4 | TextureFormat::Etc2Rgb
    )
}

/// The Bundles fetched for every Platform; see `scripts/fetch-fixtures.sh`.
const SHARED_BUNDLES: &[&str] = &[
    "abilityui_passive_summon",
    "char_bossk_pre",
    "charui_bossk",
    "env_space_crait_01_pre",
    "env_tat_dunes_jawa_pre",
    "guild_icon_blast_small",
    "pack_jp_ship_gacha",
    "relic_rfl_bossk",
    "territory_backdrop_hoth_atat",
    "territoryui_view_standard_atlas",
    "unit_details_atlas",
    "vanity_bossk",
];

fn decode(path: &std::path::Path) -> Vec<DecodedTexture> {
    decode_with_ae3(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn pinned_textures_per_format() {
    let mut verified = BTreeMap::<Platform, BTreeSet<&str>>::new();
    for p in PINNED {
        let Some(path) = fixtures::bundle(p.platform, p.bundle) else {
            continue;
        };
        let what = format!("{} {} {}", p.platform, p.bundle, p.texture);
        let texture = decode(&path)
            .into_iter()
            .find(|t| t.name == p.texture)
            .unwrap_or_else(|| panic!("{what}: no such texture"));
        assert_eq!(texture.format, Some(p.format), "{what}: format");
        assert_eq!((texture.width, texture.height), p.size, "{what}: size");
        let image = texture
            .image
            .unwrap_or_else(|e| panic!("{what}: decode failed: {e}"));
        assert_eq!(
            (image.width(), image.height()),
            p.size,
            "{what}: image size"
        );

        let stats = ImageStats::of(&image);
        for (channel, (actual, expected)) in ["R", "G", "B", "A"]
            .iter()
            .zip(stats.mean.iter().zip(p.mean))
        {
            assert!(
                (actual - expected).abs() <= MEAN_TOLERANCE,
                "{what}: mean {channel} is {actual:.2}, expected {expected:.1}"
            );
        }
        assert!(
            stats.distinct > 16,
            "{what}: only {} colours",
            stats.distinct
        );
        if has_no_alpha(p.format) {
            assert!(image.is_opaque(), "{what}: {} has no alpha", p.format);
        }
        verified
            .entry(p.platform)
            .or_default()
            .insert(p.format.name());
    }
    for (platform, formats) in &verified {
        eprintln!("verified on {platform}: {formats:?}");
    }
}

/// Lightmaps are encoded per Platform (RGBM on Windows, plain LDR on
/// mobile), so they legitimately differ.
fn comparable(name: &str) -> bool {
    !name.starts_with("Lightmap-")
}

/// The largest [`coarse_diff`] allowed between two Platforms' decodes of one
/// texture on a 16 x 16 raster. Measured on the fixtures: iOS ASTC stays
/// under 1 against Windows, Android `ETC_RGB4` under 4 (on small masks); a
/// vertically flipped or channel-swapped decode of an asymmetric texture
/// lands above 7, mostly far above.
const PLATFORM_TOLERANCE: f64 = 6.0;
const GRID: u32 = 16;

#[test]
fn platforms_agree() {
    let mut compared = BTreeMap::<String, usize>::new();
    let mut worst = (0.0f64, String::new());
    for &bundle in SHARED_BUNDLES {
        let Some(reference) = fixtures::bundle(Platform::Windows, bundle) else {
            continue;
        };
        let reference = decode(&reference);
        for platform in [Platform::Android, Platform::Ios] {
            let Some(path) = fixtures::bundle(platform, bundle) else {
                continue;
            };
            for texture in decode(&path) {
                if !comparable(&texture.name) {
                    continue;
                }
                let mut same_name = reference.iter().filter(|r| r.name == texture.name);
                let (Some(windows), None) = (same_name.next(), same_name.next()) else {
                    continue;
                };
                let what = format!(
                    "{bundle} {}: Windows {} vs {platform} {}",
                    texture.name,
                    windows.format_name(),
                    texture.format_name()
                );
                let (Ok(a), Ok(b)) = (&windows.image, &texture.image) else {
                    assert!(
                        windows.width == 0 && texture.width == 0,
                        "{what}: {:?} / {:?}",
                        windows.image.as_ref().err(),
                        texture.image.as_ref().err()
                    );
                    continue;
                };
                let diff = coarse_diff(a, b, GRID);
                assert!(
                    diff <= PLATFORM_TOLERANCE,
                    "{what}: they differ by {diff:.2}"
                );
                if diff > worst.0 {
                    worst = (diff, what);
                }
                *compared
                    .entry(format!(
                        "{} vs {}",
                        windows.format_name(),
                        texture.format_name()
                    ))
                    .or_default() += 1;
            }
        }
    }
    eprintln!("compared across Platforms: {compared:?}");
    eprintln!("largest difference {:.2}: {}", worst.0, worst.1);
}

#[test]
fn every_fixture_texture_decodes() {
    let mut formats = BTreeMap::<Platform, BTreeMap<String, usize>>::new();
    for platform in Platform::ALL {
        for path in fixtures::bundles(platform) {
            for texture in decode(&path) {
                let what = format!("{} {}", path.display(), texture.name);
                if texture.width == 0 || texture.height == 0 {
                    // Dynamic font textures are stored empty and filled at run time.
                    assert!(texture.image.is_err(), "{what}: an empty texture decoded");
                    continue;
                }
                let image = texture
                    .image
                    .as_ref()
                    .unwrap_or_else(|e| panic!("{what}: {e}"));
                assert_eq!(
                    (image.width(), image.height()),
                    (texture.width, texture.height),
                    "{what}"
                );
                *formats
                    .entry(platform)
                    .or_default()
                    .entry(texture.format_name())
                    .or_default() += 1;
            }
        }
    }
    for (platform, counts) in &formats {
        eprintln!("decoded on {platform}: {counts:?}");
    }
}
