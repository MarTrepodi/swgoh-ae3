//! Drawing. The window follows the Unity Bundle Extractor's single-window
//! layout, top to bottom: Manifest, Output, Asset Search, the Bundles table
//! (with the preview pane on the right) or the Diff tab, and the
//! extraction bar.

mod about;
mod detail;
mod diff;
mod extract;
pub mod mesh;
mod preview;
mod sections;
mod table;

use ae3_core::{AssetKind, EntityKind};
use eframe::egui::{self, Color32, RichText};

use crate::app::Ae3App;
use crate::model::Tab;
use crate::model::release::ReleaseVersion;

/// A small glyph for each Asset kind. They come from the Hack font that
/// egui already embeds (see [`install_fonts`]), so no emoji font is
/// linked into the binary.
#[must_use]
pub fn glyph(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Texture => "▣",
        AssetKind::Atlas => "▦",
        AssetKind::Sprite => "◪",
        AssetKind::Text => "¶",
        AssetKind::Structured => "≡",
        AssetKind::Mesh => "▲",
        AssetKind::Audio => "♪",
        AssetKind::Other => "○",
    }
}

/// A small glyph for each kind of Game Entity, from the same fonts as
/// [`glyph`].
#[must_use]
pub fn entity_glyph(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Unit => "◉",
        EntityKind::Ability => "❖",
        EntityKind::Gear => "⊛",
        EntityKind::Material => "◊",
        EntityKind::PlayerPortrait => "◙",
    }
}

/// How much larger than egui's defaults AE3 draws text: body text is 15
/// points instead of 12.5.
pub const TEXT_SCALE: f32 = 1.2;

/// A font size of `base` points at AE3's text scale, for text painted
/// directly rather than through a text style.
#[must_use]
pub fn text_size(base: f32) -> f32 {
    base * TEXT_SCALE
}

/// Lets proportional text fall back to Hack, egui's embedded monospace
/// font, which has the geometric shapes the glyphs use. egui 0.37 ships
/// its emoji fonts only behind a feature that would add about 740 KB.
/// Also enlarges every text style by [`TEXT_SCALE`].
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        for font in style.text_styles.values_mut() {
            font.size = text_size(font.size);
        }
    });
}

/// A capitalised kind name for labels.
#[must_use]
pub fn kind_label(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Texture => "Texture",
        AssetKind::Atlas => "Atlas",
        AssetKind::Sprite => "Sprite",
        AssetKind::Text => "Text",
        AssetKind::Structured => "Structured",
        AssetKind::Mesh => "Mesh",
        AssetKind::Audio => "Audio",
        AssetKind::Other => "Other",
    }
}

/// An error-coloured label.
pub fn error_label(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).color(ui.visuals().error_fg_color));
}

/// A warning-coloured label.
pub fn warn_label(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).color(ui.visuals().warn_fg_color));
}

/// The accent used to mark search matches.
#[must_use]
pub fn match_color(ui: &egui::Ui) -> Color32 {
    ui.visuals().selection.stroke.color
}

/// "swgoh-ae3 vX available", linking to the release page, with a dismiss button.
/// AE3 never updates itself.
fn release_notice(app: &mut Ae3App, ui: &mut egui::Ui) {
    let Some((version, url)) = app.model.release.as_ref().and_then(|state| {
        state
            .notice(ReleaseVersion::current())
            .map(|(version, release)| (version, release.url.clone()))
    }) else {
        return;
    };
    ui.separator();
    ui.hyperlink_to(format!("{} v{version} available", crate::APP_NAME), url)
        .on_hover_text("Open the release page to download it");
    if ui.small_button("×").on_hover_text("Dismiss").clicked() {
        app.dismiss_release();
    }
}

/// Draws the whole window.
pub fn show(app: &mut Ae3App, ui: &mut egui::Ui) {
    egui::Panel::top("ae3-top").show(ui, |ui| {
        ui.add_space(4.0);
        sections::manifest(app, ui);
        ui.separator();
        sections::output(app, ui);
        ui.separator();
        sections::search(app, ui);
        ui.add_space(2.0);
    });
    egui::Panel::bottom("ae3-extract").show(ui, |ui| {
        ui.add_space(4.0);
        extract::bar(app, ui);
        ui.add_space(4.0);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut app.model.tab, Tab::Bundles, "Bundles");
            ui.selectable_value(&mut app.model.tab, Tab::Diff, "Diff");
            release_notice(app, ui);
            if let Some(notice) = app.model.notice.clone() {
                ui.separator();
                warn_label(ui, notice);
                if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                    app.model.notice = None;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                about::button(app, ui);
            });
        });
        ui.separator();
        match app.model.tab {
            Tab::Bundles => {
                if app.model.preview.is_some() {
                    egui::Panel::right("ae3-preview")
                        .resizable(true)
                        .default_size(440.0)
                        // Never more than 60% of the window, whatever the
                        // pane's contents ask for.
                        .size_range(260.0..=(ui.available_width() * 0.6).max(260.0))
                        .show(ui, |ui| preview::pane(app, ui));
                }
                table::bundles(app, ui);
            }
            Tab::Diff => diff::tab(app, ui),
        }
    });
    detail::window(app, ui.ctx());
    about::window(app, ui.ctx());
}

#[cfg(test)]
mod tests {
    use eframe::egui::{FontId, RawInput};

    use super::*;

    /// Text is drawn larger than egui's defaults.
    #[test]
    fn text_styles_are_enlarged() {
        let ctx = egui::Context::default();
        let body = |theme| ctx.style_of(theme).text_styles[&egui::TextStyle::Body].size;
        let themes = [egui::Theme::Dark, egui::Theme::Light];
        let before = themes.map(body);
        install_fonts(&ctx);
        for (theme, before) in themes.into_iter().zip(before) {
            let after = body(theme);
            assert!(
                (after - before * TEXT_SCALE).abs() < 1e-3,
                "{before} -> {after}"
            );
            assert!(after >= 15.0, "body text is {after} points");
        }
    }

    /// Every glyph must render with the bundled fonts; a missing glyph
    /// would show as a box.
    #[test]
    fn glyphs_exist_in_the_default_fonts() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mut output = ctx.run_ui(RawInput::default(), |_| {});
        output.textures_delta.clear();
        for kind in AssetKind::ALL {
            let g = glyph(kind);
            let ok = ctx.fonts_mut(|f| f.has_glyphs(&FontId::proportional(14.0), g));
            assert!(ok, "no glyph for {kind}: {g}");
        }
        for kind in EntityKind::ALL {
            let g = entity_glyph(kind);
            let ok = ctx.fonts_mut(|f| f.has_glyphs(&FontId::proportional(14.0), g));
            assert!(ok, "no glyph for {kind}: {g}");
        }
        // Symbols the UI uses besides the kind glyphs.
        for extra in ["×", "▶", "■", "↻", "▴", "▾", "…", "—", "●"] {
            let ok = ctx.fonts_mut(|f| f.has_glyphs(&FontId::proportional(14.0), extra));
            assert!(ok, "no glyph {extra}");
        }
    }
}
