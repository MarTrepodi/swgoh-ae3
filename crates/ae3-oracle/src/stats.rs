//! Pixel statistics and comparisons for decoded textures.

use std::collections::HashSet;

use ae3_core::RgbaImage;

/// Summary statistics of one decoded image.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageStats {
    pub width: u32,
    pub height: u32,
    /// The mean of each channel, R, G, B and A, from 0 to 255.
    pub mean: [f64; 4],
    /// The share of pixels with alpha 255.
    pub opaque: f64,
    /// The share of pixels with alpha 0.
    pub transparent: f64,
    /// How many distinct RGBA values the image holds.
    pub distinct: usize,
}

impl ImageStats {
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "pixel counts stay far below 2^52"
    )]
    pub fn of(image: &RgbaImage) -> Self {
        let pixels = image.pixels().as_chunks::<4>().0;
        let count = pixels.len().max(1) as f64;
        let mut sums = [0u64; 4];
        let (mut opaque, mut transparent) = (0usize, 0usize);
        let mut distinct = HashSet::new();
        for p in pixels {
            for (sum, &c) in sums.iter_mut().zip(p) {
                *sum += u64::from(c);
            }
            opaque += usize::from(p[3] == u8::MAX);
            transparent += usize::from(p[3] == 0);
            distinct.insert(*p);
        }
        Self {
            width: image.width(),
            height: image.height(),
            mean: sums.map(|s| s as f64 / count),
            opaque: opaque as f64 / count,
            transparent: transparent as f64 / count,
            distinct: distinct.len(),
        }
    }
}

/// How two images of the same size differ, channel by channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelDiff {
    /// The largest difference in any channel of any pixel.
    pub max: u8,
    /// The mean absolute difference over every channel of every pixel.
    pub mean: f64,
    /// How many pixels differ in at least one channel.
    pub differing: usize,
    /// How many pixels the images have.
    pub pixels: usize,
}

impl PixelDiff {
    /// Compares `a` and `b`, or `None` when their sizes differ.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "pixel counts stay far below 2^52"
    )]
    pub fn between(a: &RgbaImage, b: &RgbaImage) -> Option<Self> {
        if (a.width(), a.height()) != (b.width(), b.height()) {
            return None;
        }
        let (pa, pb) = (a.pixels().as_chunks::<4>().0, b.pixels().as_chunks::<4>().0);
        let mut max = 0;
        let mut total = 0u64;
        let mut differing = 0;
        for (x, y) in pa.iter().zip(pb) {
            let mut any = false;
            for (&c, &d) in x.iter().zip(y) {
                let delta = c.abs_diff(d);
                max = max.max(delta);
                total += u64::from(delta);
                any |= delta != 0;
            }
            differing += usize::from(any);
        }
        Some(Self {
            max,
            mean: total as f64 / (pa.len().max(1) * 4) as f64,
            differing,
            pixels: pa.len(),
        })
    }

    /// Whether the images are identical.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.max == 0
    }
}

/// The mean absolute difference, from 0 to 255, between `a` and `b` reduced
/// to a `grid` x `grid` raster of premultiplied-alpha averages.
///
/// The images may differ in size and compression: this compares what they
/// show, not their exact pixels, so it can check that one texture decoded
/// from two Platforms' lossy formats (DXT, ETC, ASTC, PVRTC) agrees, and it
/// catches a swapped channel, a missing flip or a wrong block layout. The
/// colour of fully transparent pixels does not count.
#[must_use]
pub fn coarse_diff(a: &RgbaImage, b: &RgbaImage, grid: u32) -> f64 {
    let (ga, gb) = (coarse(a, grid), coarse(b, grid));
    let total: f64 = ga
        .iter()
        .zip(&gb)
        .flat_map(|(x, y)| x.iter().zip(y).map(|(c, d)| (c - d).abs()))
        .sum();
    total / f64::from(grid.max(1).pow(2) * 4)
}

/// Premultiplied RGB and alpha averaged over each cell of a `grid` x `grid` raster.
#[expect(
    clippy::cast_precision_loss,
    reason = "pixel counts stay far below 2^52"
)]
fn coarse(image: &RgbaImage, grid: u32) -> Vec<[f64; 4]> {
    let grid = grid.max(1) as usize;
    let (w, h) = (image.width() as usize, image.height() as usize);
    let mut sums = vec![([0f64; 4], 0usize); grid * grid];
    for (y, row) in image.pixels().chunks_exact(4 * w).enumerate() {
        let cy = y * grid / h;
        for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
            let cx = x * grid / w;
            let (sum, count) = &mut sums[cy * grid + cx];
            let alpha = f64::from(p[3]) / 255.0;
            for (s, &c) in sum.iter_mut().zip(&p[..3]) {
                *s += f64::from(c) * alpha;
            }
            sum[3] += f64::from(p[3]);
            *count += 1;
        }
    }
    sums.into_iter()
        .map(|(sum, count)| sum.map(|s| s / count.max(1) as f64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        let f = &f;
        let pixels = (0..h)
            .flat_map(|y| (0..w).flat_map(move |x| f(x, y)))
            .collect();
        RgbaImage::new(w, h, pixels).unwrap()
    }

    #[test]
    fn stats_count_alpha_and_colours() {
        let img = image(2, 2, |x, _| if x == 0 { [255, 0, 0, 255] } else { [0; 4] });
        let s = ImageStats::of(&img);
        assert_eq!(s.mean, [127.5, 0.0, 0.0, 127.5]);
        assert!((s.opaque - 0.5).abs() < f64::EPSILON);
        assert!((s.transparent - 0.5).abs() < f64::EPSILON);
        assert_eq!(s.distinct, 2);
    }

    #[test]
    fn pixel_diff_measures_channels() {
        let a = image(2, 1, |_, _| [10, 20, 30, 255]);
        let b = image(2, 1, |x, _| {
            if x == 0 {
                [10, 20, 30, 255]
            } else {
                [14, 20, 30, 255]
            }
        });
        let d = PixelDiff::between(&a, &b).unwrap();
        assert_eq!((d.max, d.differing, d.pixels), (4, 1, 2));
        assert!((d.mean - 0.5).abs() < f64::EPSILON);
        assert!(PixelDiff::between(&a, &a).unwrap().is_exact());
        assert!(PixelDiff::between(&a, &image(1, 2, |_, _| [0; 4])).is_none());
    }

    #[test]
    fn coarse_diff_ignores_size_and_hidden_colour() {
        let small = image(4, 4, |x, _| {
            if x < 2 {
                [200, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let big = image(8, 8, |x, _| {
            if x < 4 {
                [200, 0, 0, 255]
            } else {
                [0, 99, 0, 0]
            }
        });
        assert!(coarse_diff(&small, &big, 2) < 1e-9);
        // A vertical flip of a top/bottom split is far off.
        let top = image(4, 4, |_, y| if y < 2 { [255; 4] } else { [0, 0, 0, 255] });
        let bottom = image(4, 4, |_, y| if y < 2 { [0, 0, 0, 255] } else { [255; 4] });
        assert!(coarse_diff(&top, &bottom, 2) > 100.0);
    }
}
