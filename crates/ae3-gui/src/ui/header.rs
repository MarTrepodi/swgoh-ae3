//! The header bar: the icon, the name and version on the left; the Asset
//! Version shown, the "swgoh-ae3 vX available" notice and About on the
//! right.

use eframe::egui::{self, Frame, Margin, RichText, Stroke, vec2};

use super::widgets::{self, Tone};
use super::{icons, theme};
use crate::APP_NAME;
use crate::app::Ae3App;
use crate::model::release::{self, ReleaseVersion};

/// Height of the bar's contents.
const HEIGHT: f32 = 30.0;

pub fn bar(app: &mut Ae3App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let icon = app.app_icon(ui.ctx());
    egui::Panel::top("ae3-header")
        .frame(
            Frame::new()
                .fill(p.chrome)
                .inner_margin(Margin::symmetric(16, 8))
                .stroke(Stroke::new(1.0, p.border)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(HEIGHT);
                if let Some(icon) = &icon {
                    ui.add(egui::Image::new(icon).fit_to_exact_size(vec2(26.0, 26.0)));
                }
                ui.label(
                    RichText::new(APP_NAME)
                        .family(theme::medium())
                        .size(theme::size::BRAND)
                        .color(p.text_strong),
                );
                ui.label(
                    RichText::new(format!("v{}", release::CURRENT))
                        .size(theme::size::SMALL)
                        .color(p.text_muted),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button(ui, Some(icons::ABOUT), "About", true)
                        .on_hover_text(format!("Version, license and updates for {APP_NAME}"))
                        .clicked()
                    {
                        app.model.windows.about = true;
                    }
                    release_notice(app, ui);
                    asset_version(app, ui);
                });
            });
        });
}

/// "Asset Version 100054 · Windows · PROD", amber while a newer one waits.
fn asset_version(app: &Ae3App, ui: &mut egui::Ui) {
    let (Some(manifest), Some(store)) = (&app.model.manifest, &app.store) else {
        widgets::chip(ui, Tone::Neutral, icons::INDEX, "No Manifest loaded");
        return;
    };
    let text = format!(
        "Asset Version {} · {} · {}",
        manifest.asset_version,
        store.platform(),
        store.location().environment,
    );
    let newer = app.model.update.newer();
    let tone = if newer.is_some() {
        Tone::Warning
    } else {
        Tone::Ok
    };
    let chip = widgets::chip(ui, tone, icons::INDEX, &text);
    if let Some(newer) = newer {
        chip.on_hover_text(format!("Asset Version {newer} is available"));
    }
}

/// The newer release, linking to its page, with a dismiss button. AE3 never
/// updates itself.
fn release_notice(app: &mut Ae3App, ui: &mut egui::Ui) {
    let Some((version, url)) = app.model.release.as_ref().and_then(|state| {
        state
            .notice(ReleaseVersion::current())
            .map(|(version, release)| (version, release.url.clone()))
    }) else {
        return;
    };
    let p = theme::palette(ui);
    Frame::new()
        .fill(p.accent_tint)
        .stroke(Stroke::new(1.0, p.accent_border))
        .corner_radius(13)
        .inner_margin(Margin {
            left: 10,
            right: 4,
            top: 1,
            bottom: 1,
        })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            // Right to left: the dismiss button first.
            if widgets::icon_button(ui, icons::CLOSE, "Dismiss release notice").clicked() {
                app.dismiss_release();
            }
            ui.hyperlink_to(format!("{APP_NAME} v{version} available"), url)
                .on_hover_text("Open the release page to download it");
            ui.label(RichText::new(icons::UPDATE).color(p.accent_text));
        });
}
