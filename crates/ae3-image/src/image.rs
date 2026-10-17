use crate::error::{Error, Result};

/// The largest width or height an [`RgbaImage`] may have.
///
/// It matches Unity's maximum texture size and the WebP format's limit, and
/// stops a corrupt header from requesting a multi-gigabyte allocation (which
/// would abort the process rather than return an error).
pub const MAX_DIMENSION: u32 = 16_384;

/// An 8-bit RGBA image with rows stored top-down and no row padding.
///
/// Width and height are always between 1 and [`MAX_DIMENSION`], and the pixel
/// buffer is always exactly `width * height * 4` bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl std::fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RgbaImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Checks `width` and `height` and returns them as `usize`.
pub(crate) fn checked_dimensions(width: u32, height: u32) -> Result<(usize, usize)> {
    let invalid = Error::InvalidDimensions { width, height };
    if !(1..=MAX_DIMENSION).contains(&width) || !(1..=MAX_DIMENSION).contains(&height) {
        return Err(invalid);
    }
    match (usize::try_from(width), usize::try_from(height)) {
        (Ok(w), Ok(h)) => Ok((w, h)),
        _ => Err(invalid),
    }
}

impl RgbaImage {
    /// Wraps an RGBA8 pixel buffer of `width * height * 4` bytes, rows top-down.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        let (w, h) = checked_dimensions(width, height)?;
        let expected = w * h * 4;
        if pixels.len() != expected {
            return Err(Error::PixelBufferSize {
                width,
                height,
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// A fully transparent image.
    pub fn transparent(width: u32, height: u32) -> Result<Self> {
        let (w, h) = checked_dimensions(width, height)?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; w * h * 4],
        })
    }

    /// Builds an image from a buffer whose size the caller has already checked.
    pub(crate) fn from_parts(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        debug_assert_eq!(
            Some(pixels.len()),
            usize::try_from(u64::from(width) * u64::from(height) * 4).ok()
        );
        Self {
            width,
            height,
            pixels,
        }
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA8 pixels, rows top-down.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Mutable access to the pixels; the length cannot change.
    #[must_use]
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// The pixel at (`x`, `y`) with a top-left origin, or `None` outside the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y as usize * self.row_bytes() + x as usize * 4;
        self.pixels.get(i..i + 4)?.try_into().ok()
    }

    /// Whether every pixel has alpha 255.
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[3] == u8::MAX)
    }

    fn row_bytes(&self) -> usize {
        self.width as usize * 4
    }
}

/// Copies the `width` x `height` region whose top-left corner is (`x`, `y`).
///
/// The region must lie entirely inside the image and must not be empty.
pub fn crop(image: &RgbaImage, x: u32, y: u32, width: u32, height: u32) -> Result<RgbaImage> {
    let out_of_bounds = Error::OutOfBounds {
        x,
        y,
        width,
        height,
        image_width: image.width,
        image_height: image.height,
    };
    let fits =
        |start: u32, len: u32, limit: u32| start.checked_add(len).is_some_and(|end| end <= limit);
    if !fits(x, width, image.width) || !fits(y, height, image.height) {
        return Err(out_of_bounds);
    }
    let (w, h) = checked_dimensions(width, height)?;
    let (x, y) = (x as usize, y as usize);
    let src_row = image.row_bytes();
    let mut pixels = Vec::with_capacity(w * h * 4);
    for row in image.pixels.chunks_exact(src_row).skip(y).take(h) {
        pixels.extend_from_slice(&row[x * 4..(x + w) * 4]);
    }
    Ok(RgbaImage::from_parts(width, height, pixels))
}

/// Surrounds the image with transparent borders of the given widths.
///
/// Atlas Cutting uses this to restore the padding NGUI trimmed from a Sprite.
pub fn pad(image: &RgbaImage, left: u32, right: u32, top: u32, bottom: u32) -> Result<RgbaImage> {
    let too_big = || Error::InvalidDimensions {
        width: image.width.saturating_add(left).saturating_add(right),
        height: image.height.saturating_add(top).saturating_add(bottom),
    };
    let width = image
        .width
        .checked_add(left)
        .and_then(|v| v.checked_add(right))
        .ok_or_else(too_big)?;
    let height = image
        .height
        .checked_add(top)
        .and_then(|v| v.checked_add(bottom))
        .ok_or_else(too_big)?;
    let mut out = RgbaImage::transparent(width, height)?;
    if width == image.width && height == image.height {
        out.pixels.copy_from_slice(&image.pixels);
        return Ok(out);
    }
    let dst_row = out.row_bytes();
    let src_row = image.row_bytes();
    let left_bytes = left as usize * 4;
    for (src, dst) in image
        .pixels
        .chunks_exact(src_row)
        .zip(out.pixels.chunks_exact_mut(dst_row).skip(top as usize))
    {
        dst[left_bytes..left_bytes + src_row].copy_from_slice(src);
    }
    Ok(out)
}

/// Shrinks the image so neither side exceeds `max_dim`, preserving the aspect ratio.
///
/// Each output pixel is the alpha-weighted average of the source pixels it
/// covers, so transparent pixels do not darken edges. Images already within
/// `max_dim` are returned unchanged; a `max_dim` of 0 is treated as 1.
#[must_use]
pub fn thumbnail(image: &RgbaImage, max_dim: u32) -> RgbaImage {
    let max_dim = max_dim.max(1);
    let (width, height) = (image.width, image.height);
    if width <= max_dim && height <= max_dim {
        return image.clone();
    }
    let scaled = |short: u32, long: u32| -> u32 {
        let v = (u64::from(short) * u64::from(max_dim) + u64::from(long) / 2) / u64::from(long);
        u32::try_from(v).unwrap_or(max_dim).clamp(1, max_dim)
    };
    let (out_w, out_h) = if width >= height {
        (max_dim, scaled(height, width))
    } else {
        (scaled(width, height), max_dim)
    };

    let (src_w, src_h) = (width as usize, height as usize);
    let (dst_w, dst_h) = (out_w as usize, out_h as usize);
    // Box edges: output column `i` covers source columns xs[i]..xs[i + 1]; every
    // box is at least one pixel wide because the output is never larger.
    let xs: Vec<usize> = (0..=dst_w).map(|i| i * src_w / dst_w).collect();
    let src_row = image.row_bytes();
    let mut pixels = Vec::with_capacity(dst_w * dst_h * 4);
    let mut sums = vec![[0u64; 4]; dst_w];

    for oy in 0..dst_h {
        let (y0, y1) = (oy * src_h / dst_h, (oy + 1) * src_h / dst_h);
        sums.fill([0; 4]);
        for row in image.pixels.chunks_exact(src_row).skip(y0).take(y1 - y0) {
            for (sum, edge) in sums.iter_mut().zip(xs.windows(2)) {
                for p in row[edge[0] * 4..edge[1] * 4].as_chunks::<4>().0 {
                    let a = u64::from(p[3]);
                    sum[0] += u64::from(p[0]) * a;
                    sum[1] += u64::from(p[1]) * a;
                    sum[2] += u64::from(p[2]) * a;
                    sum[3] += a;
                }
            }
        }
        for (sum, edge) in sums.iter().zip(xs.windows(2)) {
            let count = ((edge[1] - edge[0]) * (y1 - y0)) as u64;
            let alpha = sum[3];
            let channel = |c: u64| {
                (c + alpha / 2)
                    .checked_div(alpha)
                    .map_or(0, |v| u8::try_from(v).unwrap_or(u8::MAX))
            };
            pixels.extend_from_slice(&[
                channel(sum[0]),
                channel(sum[1]),
                channel(sum[2]),
                u8::try_from((alpha + count / 2) / count.max(1)).unwrap_or(u8::MAX),
            ]);
        }
    }
    RgbaImage::from_parts(out_w, out_h, pixels)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation, reason = "test data generators")]
mod tests {
    use super::*;

    /// A `w` x `h` image whose pixel at (x, y) is (x, y, x ^ y, 255).
    fn gradient(w: u32, h: u32) -> RgbaImage {
        let pixels = (0..h)
            .flat_map(|y| (0..w).flat_map(move |x| [x as u8, y as u8, (x ^ y) as u8, 255]))
            .collect();
        RgbaImage::new(w, h, pixels).unwrap()
    }

    #[test]
    fn new_validates_dimensions_and_length() {
        assert!(RgbaImage::new(2, 2, vec![0; 16]).is_ok());
        assert!(matches!(
            RgbaImage::new(2, 2, vec![0; 15]),
            Err(Error::PixelBufferSize {
                expected: 16,
                actual: 15,
                ..
            })
        ));
        assert!(matches!(
            RgbaImage::new(0, 2, vec![]),
            Err(Error::InvalidDimensions { .. })
        ));
        assert!(matches!(
            RgbaImage::transparent(MAX_DIMENSION + 1, 1),
            Err(Error::InvalidDimensions { .. })
        ));
    }

    #[test]
    fn pixel_uses_top_left_origin() {
        let img = gradient(4, 3);
        assert_eq!(img.pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(img.pixel(3, 2), Some([3, 2, 1, 255]));
        assert_eq!(img.pixel(4, 0), None);
        assert_eq!(img.pixel(0, 3), None);
    }

    #[test]
    fn crop_copies_the_region() {
        let img = gradient(8, 6);
        let c = crop(&img, 2, 1, 3, 4).unwrap();
        assert_eq!((c.width(), c.height()), (3, 4));
        for y in 0..4 {
            for x in 0..3 {
                assert_eq!(c.pixel(x, y), img.pixel(x + 2, y + 1));
            }
        }
        assert_eq!(crop(&img, 0, 0, 8, 6).unwrap(), img);
    }

    #[test]
    fn crop_rejects_bad_regions() {
        let img = gradient(8, 6);
        for (x, y, w, h) in [
            (6, 0, 3, 1),
            (0, 5, 1, 2),
            (0, 0, 0, 1),
            (u32::MAX, 0, 2, 1),
        ] {
            assert!(crop(&img, x, y, w, h).is_err(), "{x},{y} {w}x{h}");
        }
    }

    #[test]
    fn pad_adds_transparent_borders() {
        let img = gradient(3, 2);
        let p = pad(&img, 1, 2, 3, 4).unwrap();
        assert_eq!((p.width(), p.height()), (6, 9));
        for y in 0..9 {
            for x in 0..6 {
                let inside = (1..4).contains(&x) && (3..5).contains(&y);
                let expected = if inside {
                    img.pixel(x - 1, y - 3)
                } else {
                    Some([0; 4])
                };
                assert_eq!(p.pixel(x, y), expected, "({x}, {y})");
            }
        }
        assert_eq!(pad(&img, 0, 0, 0, 0).unwrap(), img);
        assert!(pad(&img, MAX_DIMENSION, 0, 0, 0).is_err());
    }

    #[test]
    fn thumbnail_preserves_aspect_and_averages() {
        let img = gradient(200, 100);
        let t = thumbnail(&img, 50);
        assert_eq!((t.width(), t.height()), (50, 25));
        // Output (0, 0) averages source x 0..4, y 0..4: mean x and y are 1.5.
        assert_eq!(t.pixel(0, 0), Some([2, 2, 2, 255]));

        let tall = thumbnail(&gradient(10, 1000), 100);
        assert_eq!((tall.width(), tall.height()), (1, 100));

        let small = gradient(10, 10);
        assert_eq!(thumbnail(&small, 64), small);
        assert_eq!(thumbnail(&small, 0).width(), 1);
    }

    #[test]
    fn thumbnail_ignores_colour_of_transparent_pixels() {
        // Opaque red next to transparent green: the average stays pure red.
        let img = RgbaImage::new(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 0]).unwrap();
        assert_eq!(thumbnail(&img, 1).pixel(0, 0), Some([255, 0, 0, 128]));
        let clear = RgbaImage::transparent(4, 4).unwrap();
        assert_eq!(thumbnail(&clear, 2).pixels(), &[0; 16]);
    }
}
