//! Texture decoding and image encoding for AE3.
//!
//! [`decode`] turns the raw image data of a Unity `Texture2D` Asset into an
//! [`RgbaImage`] (8-bit RGBA, rows top-down), [`encode`] writes that image as
//! PNG or lossless WebP, and [`crop`], [`pad`] and [`thumbnail`] serve Atlas
//! Cutting and previews.
//!
//! Everything here is pure Rust and synchronous. Decoding never panics on
//! malformed input: buffer lengths and dimensions are checked before any
//! decoder runs, and compressed blocks that could drive `texture2ddecoder`
//! out of bounds are replaced with the ASTC error colour instead.

mod astc;
mod crunch;
mod decode;
mod encode;
mod error;
mod format;
mod image;
#[cfg(test)]
mod test_util;

pub use decode::decode;
pub use encode::{ImageFormat, PngCompression, encode};
pub use error::{Error, Result};
pub use format::TextureFormat;
pub use image::{MAX_DIMENSION, RgbaImage, crop, pad, thumbnail};
