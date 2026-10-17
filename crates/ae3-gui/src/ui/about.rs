//! The About window: the icon, name and version, where the project lives,
//! and a "Check for updates" button that asks GitHub straight away.

use eframe::egui::{self, RichText, vec2};

use super::error_label;
use crate::APP_NAME;
use crate::app::Ae3App;
use crate::model::release::{self, ManualCheck};

/// The button that opens the window, for the top of the window.
pub fn button(app: &mut Ae3App, ui: &mut egui::Ui) {
    if ui
        .button("About")
        .on_hover_text(format!("Version, license and updates for {APP_NAME}"))
        .clicked()
    {
        app.model.about_open = true;
    }
}

/// The window itself, while open.
pub fn window(app: &mut Ae3App, ctx: &egui::Context) {
    if !app.model.about_open {
        return;
    }
    let mut open = true;
    let icon = app.about_icon(ctx);
    egui::Window::new(format!("About {APP_NAME}"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(icon) = &icon {
                    ui.add(egui::Image::new(icon).fit_to_exact_size(vec2(96.0, 96.0)));
                }
                ui.vertical(|ui| {
                    ui.heading(APP_NAME);
                    ui.label(format!("Version {}", release::CURRENT));
                    ui.label("Star Wars: Galaxy of Heroes asset extractor");
                    ui.hyperlink_to("Source code and releases", release::REPOSITORY);
                });
            });
            ui.label(
                RichText::new(
                    "MIT License. An unofficial community tool, not affiliated with or \
                     endorsed by Electronic Arts, Capital Games or Lucasfilm.",
                )
                .small()
                .weak(),
            );
            ui.separator();
            update_row(app, ui);
        });
    app.model.about_open = open;
}

fn update_row(app: &mut Ae3App, ui: &mut egui::Ui) {
    let check = app.model.manual_check.clone();
    ui.horizontal_wrapped(|ui| {
        let checking = check == ManualCheck::Checking;
        if ui
            .add_enabled(!checking, egui::Button::new("Check for updates"))
            .on_hover_text("Ask GitHub Releases now for a newer version")
            .clicked()
        {
            app.check_for_updates_now();
        }
        match check {
            ManualCheck::Idle => {}
            ManualCheck::Checking => {
                ui.spinner();
                ui.label("Checking…");
            }
            ManualCheck::UpToDate => {
                ui.label(format!(
                    "{APP_NAME} {} is the latest release.",
                    release::CURRENT
                ));
            }
            ManualCheck::Newer { version, url } => {
                ui.hyperlink_to(format!("{APP_NAME} v{version} is available"), url)
                    .on_hover_text("Open the release page to download it");
            }
            ManualCheck::NotPublished => {
                ui.label(RichText::new("No release has been published yet.").weak());
            }
            ManualCheck::Failed(e) => error_label(ui, format!("Could not check for updates: {e}")),
        }
    });
}
