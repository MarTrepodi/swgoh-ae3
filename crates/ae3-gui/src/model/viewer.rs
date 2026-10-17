//! The texture viewer's pure parts: channel masking, zoom and pan, and
//! hit-testing Sprite rectangles.

use ae3_core::SpriteRect;
use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

/// Which colour channels the viewer shows.
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent toggles, one per channel"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channels {
    pub r: bool,
    pub g: bool,
    pub b: bool,
    pub a: bool,
}

impl Default for Channels {
    fn default() -> Self {
        Self::ALL
    }
}

impl Channels {
    pub const ALL: Self = Self {
        r: true,
        g: true,
        b: true,
        a: true,
    };

    #[must_use]
    pub fn is_all(self) -> bool {
        self == Self::ALL
    }

    fn count(self) -> usize {
        [self.r, self.g, self.b, self.a]
            .into_iter()
            .filter(|on| *on)
            .count()
    }

    /// Applies the mask to RGBA8 pixels (straight alpha). With all
    /// channels on the pixels are unchanged. With exactly one channel on it
    /// is shown as opaque greyscale. Otherwise the channels that are off
    /// are zeroed, and with alpha off the image is opaque.
    #[must_use]
    pub fn apply(self, pixels: &[u8]) -> Vec<u8> {
        if self.is_all() {
            return pixels.to_vec();
        }
        let single = (self.count() == 1).then(|| {
            [self.r, self.g, self.b, self.a]
                .iter()
                .position(|on| *on)
                .unwrap_or(0)
        });
        let mut out = Vec::with_capacity(pixels.len());
        for px in pixels.as_chunks::<4>().0 {
            match single {
                Some(c) => out.extend_from_slice(&[px[c], px[c], px[c], 255]),
                None => out.extend_from_slice(&[
                    if self.r { px[0] } else { 0 },
                    if self.g { px[1] } else { 0 },
                    if self.b { px[2] } else { 0 },
                    if self.a { px[3] } else { 255 },
                ]),
            }
        }
        out
    }
}

/// Smallest and largest zoom factors (screen points per texture pixel).
pub const MIN_ZOOM: f32 = 1.0 / 64.0;
pub const MAX_ZOOM: f32 = 64.0;

/// Zoom and pan of an image in a view. The image centre sits at the view
/// centre plus `pan`; one texture pixel is `zoom` points wide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    pub zoom: f32,
    pub pan: Vec2,
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }
}

impl ViewTransform {
    /// Fits an image of `image` pixels inside `view` points, centred, never
    /// magnifying beyond 8x (tiny icons stay crisp but not huge).
    #[must_use]
    pub fn fit(image: Vec2, view: Vec2) -> Self {
        if image.x <= 0.0 || image.y <= 0.0 || view.x <= 0.0 || view.y <= 0.0 {
            return Self::default();
        }
        let zoom = (view.x / image.x).min(view.y / image.y).min(8.0);
        Self {
            zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM),
            pan: Vec2::ZERO,
        }
    }

    /// Multiplies the zoom by `factor`, keeping the image point under
    /// `cursor` (relative to the view centre) where it is.
    pub fn zoom_at(&mut self, factor: f32, cursor: Vec2) {
        let new_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let image_point = (cursor - self.pan) / self.zoom;
        self.pan = cursor - image_point * new_zoom;
        self.zoom = new_zoom;
    }

    /// Where an image of `image` pixels is drawn in `view`.
    #[must_use]
    pub fn image_rect(&self, image: Vec2, view: Rect) -> Rect {
        Rect::from_center_size(view.center() + self.pan, image * self.zoom)
    }

    /// The texture pixel under screen point `pos`, if inside the image.
    #[must_use]
    pub fn pixel_at(&self, image: Vec2, view: Rect, pos: Pos2) -> Option<Pos2> {
        let rect = self.image_rect(image, view);
        if !rect.contains(pos) {
            return None;
        }
        let p = (pos - rect.min) / self.zoom;
        Some(pos2(p.x.min(image.x - 1.0), p.y.min(image.y - 1.0)))
    }

    /// The screen rectangle of a Sprite (top-left origin, texture pixels).
    #[must_use]
    pub fn sprite_rect(&self, image: Vec2, view: Rect, sprite: &SpriteRect) -> Rect {
        let origin = self.image_rect(image, view).min;
        Rect::from_min_size(
            origin + vec2(sprite.x as f32, sprite.y as f32) * self.zoom,
            vec2(sprite.width as f32, sprite.height as f32) * self.zoom,
        )
    }
}

/// The Sprite containing texture pixel `pixel`; the smallest one when
/// several overlap.
#[must_use]
pub fn sprite_at(sprites: &[SpriteRect], pixel: Pos2) -> Option<usize> {
    sprites
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let (x, y) = (pixel.x, pixel.y);
            x >= s.x as f32
                && y >= s.y as f32
                && x < (s.x + s.width) as f32
                && y < (s.y + s.height) as f32
        })
        .min_by_key(|(_, s)| u64::from(s.width) * u64::from(s.height))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PX: [u8; 8] = [10, 20, 30, 40, 50, 60, 70, 80];

    #[test]
    fn all_channels_is_identity() {
        assert_eq!(Channels::ALL.apply(&PX), PX);
    }

    #[test]
    fn alpha_only_is_greyscale_alpha() {
        let ch = Channels {
            r: false,
            g: false,
            b: false,
            a: true,
        };
        assert_eq!(ch.apply(&PX), [40, 40, 40, 255, 80, 80, 80, 255]);
    }

    #[test]
    fn single_colour_channel_is_greyscale() {
        let ch = Channels {
            r: false,
            g: true,
            b: false,
            a: false,
        };
        assert_eq!(ch.apply(&PX), [20, 20, 20, 255, 60, 60, 60, 255]);
    }

    #[test]
    fn masked_channels_are_zeroed_and_alpha_off_is_opaque() {
        let ch = Channels {
            r: true,
            g: false,
            b: true,
            a: false,
        };
        assert_eq!(ch.apply(&PX), [10, 0, 30, 255, 50, 0, 70, 255]);
        let ch = Channels {
            g: false,
            ..Channels::ALL
        };
        assert_eq!(ch.apply(&PX), [10, 0, 30, 40, 50, 0, 70, 80]);
    }

    #[test]
    fn fit_and_zoom_about_the_cursor() {
        let mut t = ViewTransform::fit(vec2(400.0, 200.0), vec2(200.0, 200.0));
        assert!((t.zoom - 0.5).abs() < 1e-6);
        // Tiny images are magnified at most 8x.
        assert!((ViewTransform::fit(vec2(4.0, 4.0), vec2(1000.0, 1000.0)).zoom - 8.0).abs() < 1e-6);

        let view = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0));
        let image = vec2(400.0, 200.0);
        let cursor_screen = pos2(150.0, 100.0);
        let before = t.pixel_at(image, view, cursor_screen).unwrap();
        t.zoom_at(2.0, cursor_screen - view.center());
        let after = t.pixel_at(image, view, cursor_screen).unwrap();
        assert!((before - after).length() < 1e-3, "{before:?} vs {after:?}");
        assert!((t.zoom - 1.0).abs() < 1e-6);
        t.zoom_at(1e9, Vec2::ZERO);
        assert!((t.zoom - MAX_ZOOM).abs() < 1e-3);
    }

    #[test]
    fn pixels_outside_the_image_are_none() {
        let t = ViewTransform::default();
        let view = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        assert_eq!(t.pixel_at(vec2(10.0, 10.0), view, pos2(0.0, 0.0)), None);
        assert_eq!(
            t.pixel_at(vec2(10.0, 10.0), view, pos2(45.0, 45.0)),
            Some(pos2(0.0, 0.0))
        );
    }

    fn sprite(name: &str, x: u32, y: u32, w: u32, h: u32) -> SpriteRect {
        SpriteRect {
            name: name.into(),
            x,
            y,
            width: w,
            height: h,
            border_left: 0,
            border_right: 0,
            border_top: 0,
            border_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            padding_top: 0,
            padding_bottom: 0,
        }
    }

    #[test]
    fn sprite_hit_test_prefers_the_smallest() {
        let sprites = [sprite("big", 0, 0, 100, 100), sprite("small", 10, 10, 5, 5)];
        assert_eq!(sprite_at(&sprites, pos2(12.0, 12.0)), Some(1));
        assert_eq!(sprite_at(&sprites, pos2(50.0, 50.0)), Some(0));
        assert_eq!(sprite_at(&sprites, pos2(100.0, 5.0)), None);

        let t = ViewTransform {
            zoom: 2.0,
            pan: Vec2::ZERO,
        };
        let view = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0));
        let r = t.sprite_rect(vec2(100.0, 100.0), view, &sprites[1]);
        assert_eq!(r, Rect::from_min_size(pos2(120.0, 120.0), vec2(10.0, 10.0)));
    }
}
