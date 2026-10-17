//! The differential oracle: textures decoded by `unity-rs-core` (ADR-0006).
//!
//! `unity-rs-core` has its own `UnityFS`, `SerializedFile` and `Texture2D`
//! readers, so agreeing with it checks AE3's container parsing, the choice of
//! image-data slice, the row flip and the channel order independently. It
//! also decodes DXT, ASTC, EAC and the uncompressed formats with its own
//! code. For BC7, ETC, ETC2 and PVRTC it calls `texture2ddecoder`, the crate
//! `ae3-image` uses too, so a bug inside that crate would not show here; the
//! per-Platform agreement in `tests/texture_formats.rs` covers those formats.

use std::collections::BTreeMap;
use std::path::Path;

use ae3_core::RgbaImage;
use unity_rs_core::studio::Studio;
use unity_rs_core::texture::TextureReadLimits;

/// Unity's class id for `Texture2D`.
const CLASS_TEXTURE_2D: i32 = 28;

/// Decodes the top mip level of every `Texture2D` in the Bundle file at
/// `path`, keyed by path id, rows top-down like [`crate::decode_with_ae3`].
///
/// A texture `unity-rs-core` cannot decode maps to its error message.
pub fn decode_with_unity_rs(
    path: &Path,
) -> Result<BTreeMap<i64, Result<RgbaImage, String>>, String> {
    let studio = Studio::open(path).map_err(|e| e.to_string())?;
    Ok(studio
        .objects()
        .filter(|object| object.class_id() == CLASS_TEXTURE_2D)
        .map(|object| {
            let image = object
                .decode_texture_mip(0, TextureReadLimits::default())
                .map_err(|e| e.to_string())
                .and_then(|image| top_down(image.width, image.height, &image.pixels));
            (object.path_id(), image)
        })
        .collect())
}

/// Reverses Unity's bottom-up row order.
fn top_down(width: u32, height: u32, pixels: &[u8]) -> Result<RgbaImage, String> {
    let row = width as usize * 4;
    if row == 0 {
        return Err("zero-width image".to_owned());
    }
    let flipped = pixels.chunks_exact(row).rev().flatten().copied().collect();
    RgbaImage::new(width, height, flipped).map_err(|e| e.to_string())
}
