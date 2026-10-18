//! The About window: the icon, name and version, where the project lives,
//! and a "Check for updates" button that asks GitHub straight away. The
//! header's About button opens it, and on macOS so does the menu bar's
//! "About swgoh-ae3" ([`crate::menu`]).

use eframe::egui::{self, RichText, vec2};

use super::widgets::{self, Tone};
use super::{icons, theme};
use crate::APP_NAME;
use crate::app::Ae3App;
use crate::model::release::{self, ManualCheck};

/// The window itself, while open.
pub fn window(app: &mut Ae3App, ctx: &egui::Context) {
    if !app.model.windows.about {
        return;
    }
    let mut open = true;
    let icon = app.app_icon(ctx);
    egui::Window::new(format!("About {APP_NAME}"))
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .show(ctx, |ui| {
            let p = theme::palette(ui);
            ui.horizontal(|ui| {
                if let Some(icon) = &icon {
                    ui.add(egui::Image::new(icon).fit_to_exact_size(vec2(88.0, 88.0)));
                }
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    ui.heading(APP_NAME);
                    ui.label(
                        RichText::new(format!("Version {}", app.version_label()))
                            .color(p.text_muted),
                    );
                    ui.label("Star Wars: Galaxy of Heroes asset extractor");
                    ui.hyperlink_to("Source code and releases", release::REPOSITORY);
                });
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "MIT License. An unofficial community tool, not affiliated with or \
                     endorsed by Electronic Arts, Capital Games or Lucasfilm.",
                )
                .size(theme::size::SMALL)
                .color(p.text_muted),
            );
            ui.separator();
            update_row(app, ui);
        });
    app.model.windows.about = open;
}

fn update_row(app: &mut Ae3App, ui: &mut egui::Ui) {
    let check = app.model.manual_check.clone();
    ui.horizontal_wrapped(|ui| {
        let checking = check == ManualCheck::Checking;
        if widgets::button(ui, Some(icons::RETRY), "Check for updates", !checking)
            .on_hover_text("Ask GitHub Releases now for a newer version")
            .clicked()
        {
            app.check_for_updates_now();
        }
        match check {
            ManualCheck::Idle => {}
            ManualCheck::Checking => {
                ui.spinner();
                widgets::muted(ui, "Checking…");
            }
            ManualCheck::UpToDate => {
                widgets::status(
                    ui,
                    Tone::Ok,
                    format!("{APP_NAME} {} is the latest release.", release::CURRENT),
                );
            }
            ManualCheck::Newer { version, url } => {
                ui.label(RichText::new(icons::UPDATE).color(theme::palette(ui).accent_text));
                ui.hyperlink_to(format!("{APP_NAME} v{version} is available"), url)
                    .on_hover_text("Open the release page to download it");
            }
            ManualCheck::NotPublished => {
                widgets::muted(ui, "No release has been published yet.");
            }
            ManualCheck::Failed(e) => {
                widgets::status(ui, Tone::Error, format!("Could not check for updates: {e}"));
            }
        }
    });
}
