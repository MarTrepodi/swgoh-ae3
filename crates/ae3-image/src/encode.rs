use std::borrow::Cow;

use crate::error::{Error, Result};
use crate::image::RgbaImage;

/// How hard the PNG encoder works; every level is lossless.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PngCompression {
    /// Fastest encoding, largest files.
    Fastest,
    /// Very fast with a decent ratio (the `fdeflate` fast path). The default.
    #[default]
    Fast,
    /// Slower, roughly 10-20% smaller files than `Fast`.
    Balanced,
    /// Slowest, slightly smaller again than `Balanced`.
    High,
}

impl From<PngCompression> for png::Compression {
    fn from(c: PngCompression) -> Self {
        match c {
            PngCompression::Fastest => Self::Fastest,
            PngCompression::Fast => Self::Fast,
            PngCompression::Balanced => Self::Balanced,
            PngCompression::High => Self::High,
        }
    }
}

/// An output image file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    /// PNG at the given compression level.
    Png(PngCompression),
    /// Lossless WebP (VP8L).
    WebP,
}

impl Default for ImageFormat {
    fn default() -> Self {
        Self::Png(PngCompression::default())
    }
}

impl ImageFormat {
    /// The file extension, without a dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png(_) => "png",
            Self::WebP => "webp",
        }
    }

    /// The MIME type, for HTTP responses.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Png(_) => "image/png",
            Self::WebP => "image/webp",
        }
    }
}

/// Encodes the image as a complete PNG or lossless WebP file.
///
/// Fully opaque images are written without an alpha channel. Encoding into
/// memory only fails if an encoder rejects the image, which the invariants of
/// [`RgbaImage`] rule out in practice.
pub fn encode(image: &RgbaImage, format: ImageFormat) -> Result<Vec<u8>> {
    let opaque = image.is_opaque();
    let data: Cow<'_, [u8]> = if opaque {
        Cow::Owned(
            image
                .pixels()
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[r, g, b, _]| [r, g, b])
                .collect(),
        )
    } else {
        Cow::Borrowed(image.pixels())
    };
    let mut out = Vec::with_capacity(data.len() / 4);
    match format {
        ImageFormat::Png(compression) => {
            let mut encoder = png::Encoder::new(&mut out, image.width(), image.height());
            encoder.set_color(if opaque {
                png::ColorType::Rgb
            } else {
                png::ColorType::Rgba
            });
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(compression.into());
            let png_error = |e: png::EncodingError| Error::Encode(e.to_string());
            let mut writer = encoder.write_header().map_err(png_error)?;
            writer.write_image_data(&data).map_err(png_error)?;
            writer.finish().map_err(png_error)?;
        }
        ImageFormat::WebP => {
            let color = if opaque {
                image_webp::ColorType::Rgb8
            } else {
                image_webp::ColorType::Rgba8
            };
            image_webp::WebPEncoder::new(&mut out)
                .encode(&data, image.width(), image.height(), color)
                .map_err(|e| Error::Encode(e.to_string()))?;
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, reason = "test data generators")]
mod tests {
    use super::*;

    fn sample(width: u32, height: u32, alpha: impl Fn(u32, u32) -> u8) -> RgbaImage {
        let pixels = (0..height)
            .flat_map(|y| {
                let alpha = &alpha;
                (0..width)
                    .flat_map(move |x| [(x * 7) as u8, (y * 13) as u8, (x * y) as u8, alpha(x, y)])
            })
            .collect();
        RgbaImage::new(width, height, pixels).unwrap()
    }

    fn decode_png(bytes: &[u8]) -> (u32, u32, png::ColorType, Vec<u8>) {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, info.color_type, buf)
    }

    fn to_rgba(color: png::ColorType, data: Vec<u8>) -> Vec<u8> {
        match color {
            png::ColorType::Rgba => data,
            png::ColorType::Rgb => data
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            other => panic!("unexpected colour type {other:?}"),
        }
    }

    #[test]
    fn png_round_trips_at_every_level() {
        let translucent = sample(37, 21, |x, y| ((x + y) * 5) as u8);
        let opaque = sample(16, 9, |_, _| 255);
        for compression in [
            PngCompression::Fastest,
            PngCompression::Fast,
            PngCompression::Balanced,
            PngCompression::High,
        ] {
            for (image, expected_color) in [
                (&translucent, png::ColorType::Rgba),
                (&opaque, png::ColorType::Rgb),
            ] {
                let bytes = encode(image, ImageFormat::Png(compression)).unwrap();
                assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
                let (w, h, color, data) = decode_png(&bytes);
                assert_eq!(
                    (w, h, color),
                    (image.width(), image.height(), expected_color)
                );
                assert_eq!(to_rgba(color, data), image.pixels());
            }
        }
    }

    #[test]
    fn webp_round_trips_losslessly() {
        for image in [
            sample(37, 21, |x, y| ((x ^ y) * 9) as u8),
            sample(5, 40, |_, _| 255),
        ] {
            let bytes = encode(&image, ImageFormat::WebP).unwrap();
            assert_eq!((&bytes[..4], &bytes[8..12]), (&b"RIFF"[..], &b"WEBP"[..]));
            let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(&bytes)).unwrap();
            assert_eq!(decoder.dimensions(), (image.width(), image.height()));
            assert_eq!(decoder.has_alpha(), !image.is_opaque());
            let mut buf = vec![0; decoder.output_buffer_size().unwrap()];
            decoder.read_image(&mut buf).unwrap();
            let rgba = if decoder.has_alpha() {
                buf
            } else {
                buf.as_chunks::<3>()
                    .0
                    .iter()
                    .flat_map(|p| [p[0], p[1], p[2], 255])
                    .collect()
            };
            assert_eq!(rgba, image.pixels());
        }
    }

    #[test]
    fn format_metadata() {
        assert_eq!(
            ImageFormat::default(),
            ImageFormat::Png(PngCompression::Fast)
        );
        assert_eq!(ImageFormat::WebP.extension(), "webp");
        assert_eq!(
            ImageFormat::Png(PngCompression::High).mime_type(),
            "image/png"
        );
    }
}
