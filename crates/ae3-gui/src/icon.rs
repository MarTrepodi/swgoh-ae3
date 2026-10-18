//! The application icon, embedded in the binary: the window icon on every
//! platform and the picture in the About window. Platform packages carry
//! their own copies (packaging/icons/).

use eframe::egui;

/// A 128 px rendering of packaging/icons/swgoh-ae3.png. Window managers
/// scale it; larger sizes would only grow the binary.
const PNG: &[u8] = include_bytes!("../assets/icon-128.png");

/// The decoded icon as straight RGBA, or `None` if the embedded PNG is
/// unreadable (a build mistake, caught by the tests below).
#[must_use]
pub fn rgba() -> Option<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(PNG));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        _ => return None,
    };
    Some((rgba, info.width, info.height))
}

/// The window icon; an empty icon (the platform default) if decoding fails.
#[must_use]
pub fn window_icon() -> egui::IconData {
    rgba().map_or_else(egui::IconData::default, |(rgba, width, height)| {
        egui::IconData {
            rgba,
            width,
            height,
        }
    })
}

/// The icon as an egui image, for the About window.
#[must_use]
pub fn color_image() -> Option<egui::ColorImage> {
    let (rgba, width, height) = rgba()?;
    let size = [usize::try_from(width).ok()?, usize::try_from(height).ok()?];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, &rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_decodes() {
        let (rgba, width, height) = rgba().expect("the embedded icon is a readable PNG");
        assert_eq!((width, height), (128, 128));
        assert_eq!(rgba.len(), 128 * 128 * 4);
        // Rounded corners are transparent, the middle is opaque.
        assert_eq!(rgba[3], 0, "top-left corner is transparent");
        let centre = (64 * 128 + 64) * 4;
        assert_eq!(rgba[centre + 3], 255, "the middle is opaque");
        assert_eq!(window_icon().width, 128);
        assert!(color_image().is_some());
    }
}
