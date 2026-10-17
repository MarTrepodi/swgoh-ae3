//! Development-only checks of AE3's texture decoding against real game data.
//!
//! This crate is never shipped. It holds what the fixture tests in its
//! `tests/` directory share:
//!
//! - [`fixtures`]: where the fixture Packages fetched by
//!   `scripts/fetch-fixtures.sh` live (`$AE3_FIXTURES`, default
//!   `<workspace>/fixtures`, then `<Asset Version>/<Platform>/`).
//! - [`decode_with_ae3`]: every `Texture2D` of a Bundle file decoded through
//!   `ae3-core`, the same path the binaries use.
//! - [`ImageStats`] and [`PixelDiff`]: what the tests assert about pixels.
//! - With the `oracle` feature, the `unity_rs` module: the same textures decoded by
//!   `unity-rs-core`, the differential oracle ADR-0006 names. That crate links
//!   C zstd, which is why the feature is off by default and why no shipped
//!   binary depends on this crate.

pub mod fixtures;
mod stats;
#[cfg(feature = "oracle")]
pub mod unity_rs;

use std::path::Path;

use ae3_core::ae3_image::TextureFormat;
use ae3_core::{AssetError, AssetKind, LoadedBundle, RgbaImage, TextureInfo, guard};

pub use stats::{ImageStats, PixelDiff, coarse_diff};

/// One `Texture2D` of a Bundle and the outcome of decoding it.
#[derive(Debug)]
pub struct DecodedTexture {
    pub path_id: i64,
    /// `m_Name`.
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Unity's `TextureFormat` number.
    pub format_id: i32,
    /// The format, when AE3 knows the number.
    pub format: Option<TextureFormat>,
    /// The decoded top mip level, rows top-down, or why decoding failed
    /// (including a contained decoder panic).
    pub image: Result<RgbaImage, String>,
}

impl DecodedTexture {
    /// Unity's name for the format, or its number when AE3 does not know it.
    #[must_use]
    pub fn format_name(&self) -> String {
        self.format.map_or_else(
            || format!("format {}", self.format_id),
            |f| f.name().to_owned(),
        )
    }
}

/// Decodes every `Texture2D` in the Bundle file at `path` through `ae3-core`,
/// in `SerializedFile` order.
///
/// Each decode runs under [`guard::catch_panic`], as the export path does, so
/// one bad texture is reported rather than ending the run.
pub fn decode_with_ae3(path: &Path) -> Result<Vec<DecodedTexture>, AssetError> {
    let bundle = LoadedBundle::open_file(path)?;
    let view = bundle.view()?;
    Ok(view
        .assets()
        .into_iter()
        .filter(|asset| asset.kind == AssetKind::Texture)
        .map(|asset| {
            // A texture whose header could not be read still gets a row, so
            // the failure shows up instead of the texture going missing.
            let info = asset.texture.unwrap_or(TextureInfo {
                width: 0,
                height: 0,
                format_id: -1,
                format: None,
            });
            let image = guard::catch_panic(|| view.decode_texture(asset.path_id))
                .and_then(|decoded| decoded.map_err(|e| e.to_string()));
            DecodedTexture {
                path_id: asset.path_id,
                name: asset.name,
                width: info.width,
                height: info.height,
                format_id: info.format_id,
                format: info.format,
                image,
            }
        })
        .collect())
}
