//! Drawing. The window has a header bar (name, Asset Version, About), a
//! sidebar of cards (Game data, Output, Asset Index), the main area and an
//! action bar at the bottom. The main area opens with a banner that
//! switches between Bundles and Diff and says what is shown; on Bundles,
//! the search field, the matching Game Entities and the Asset type filters
//! sit above the Bundles table, with the preview pane on the right.
//! Settings, About and the detail viewer are windows.
//!
//! [`theme`] holds the palette and type, [`widgets`] the shared pieces.

mod about;
mod detail;
mod diff;
mod extract;
mod header;
pub mod icons;
pub mod mesh;
mod preview;
mod search;
mod settings;
mod sidebar;
mod table;
pub mod theme;
pub mod widgets;

use std::fmt::Write as _;

use ae3_core::AssetKind;
use eframe::egui::{self, Frame, Margin, RichText, Stroke};

use self::widgets::Tone;
use crate::app::Ae3App;
use crate::model::Tab;
use crate::model::diff::summary_line;

/// Installs the fonts and the light and dark styles; see [`theme`].
pub fn install(ctx: &egui::Context) {
    theme::install(ctx);
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

/// An error message with its icon.
pub fn error_label(ui: &mut egui::Ui, text: impl Into<String>) {
    widgets::status(ui, Tone::Error, text);
}

/// A warning with its icon.
pub fn warn_label(ui: &mut egui::Ui, text: impl Into<String>) {
    widgets::status(ui, Tone::Warning, text);
}

/// The colour of search matches.
#[must_use]
pub fn match_color(ui: &egui::Ui) -> egui::Color32 {
    theme::palette(ui).amber
}

/// The narrowest the Bundles table gets beside the preview pane.
const TABLE_MIN_WIDTH: f32 = 380.0;

/// The widest the preview pane gets in a main area `available` wide.
fn preview_max_width(available: f32) -> f32 {
    (available * 0.6)
        .min(available - TABLE_MIN_WIDTH)
        .max(280.0)
}

/// The space between the main area's blocks.
const GAP: f32 = 10.0;

/// Draws the whole window.
pub fn show(app: &mut Ae3App, ui: &mut egui::Ui) {
    header::bar(app, ui);
    extract::bar(app, ui);
    sidebar::panel(app, ui);
    let p = theme::palette(ui);
    egui::CentralPanel::default()
        .frame(Frame::new().fill(p.canvas).inner_margin(Margin::same(14)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = GAP;
            notice(app, ui);
            banner(app, ui);
            match app.model.tab {
                Tab::Bundles => bundles(app, ui),
                Tab::Diff => diff::tab(app, ui),
            }
        });
    detail::window(app, ui.ctx());
    settings::window(app, ui.ctx());
    about::window(app, ui.ctx());
}

/// The last problem worth telling, with a dismiss button.
fn notice(app: &mut Ae3App, ui: &mut egui::Ui) {
    let Some(notice) = app.model.notice.clone() else {
        return;
    };
    let p = theme::palette(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.amber.gamma_multiply(0.6)))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::icon_button(ui, icons::CLOSE, "Dismiss notice").clicked() {
                    app.model.notice = None;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    widgets::status(ui, Tone::Warning, notice);
                });
            });
        });
}

/// The banner: Bundles or Diff, and what is shown.
fn banner(app: &mut Ae3App, ui: &mut egui::Ui) {
    widgets::banner_frame(ui).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_width(ui.available_width());
            widgets::segmented(
                ui,
                &mut app.model.tab,
                &[
                    (Tab::Bundles, Some(icons::BUNDLES), "Bundles"),
                    (Tab::Diff, Some(icons::DIFF), "Diff"),
                ],
                false,
            );
            ui.add_space(6.0);
            match app.model.tab {
                Tab::Bundles => bundles_context(app, ui),
                Tab::Diff => diff_context(app, ui),
            }
        });
    });
}

fn bundles_context(app: &mut Ae3App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    if app.model.manifest.is_none() {
        widgets::muted(ui, "No Manifest loaded");
        return;
    }
    let shown = app.model.visible.len();
    let total = app.model.rows.len();
    let mut text = if shown == total {
        format!("{total} Packages")
    } else {
        format!("{shown} of {total} Packages")
    };
    if !app.model.selection.is_empty() {
        let _ = write!(text, " · {} selected", app.model.selection.len());
    }
    ui.label(
        RichText::new(text)
            .family(theme::medium())
            .color(p.text_strong),
    );
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if !app.model.selection.is_empty()
            && widgets::button(ui, None, "Clear selection", true).clicked()
        {
            app.model.selection.clear();
        }
        if !app.model.visible.is_empty()
            && widgets::button(ui, Some(icons::SELECT_ALL), "Select all", true)
                .on_hover_text("Select every Package the table shows")
                .clicked()
        {
            let visible = app.model.visible.clone();
            app.model.selection.select_all(&visible);
        }
        if let Some(status) = app.model.search_status() {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(RichText::new(icons::SEARCH).color(p.amber));
                ui.add(egui::Label::new(RichText::new(status).color(p.text)).truncate());
            });
        }
    });
}

fn diff_context(app: &Ae3App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    if let Some(diff) = &app.model.diff.diff {
        ui.add(
            egui::Label::new(
                RichText::new(summary_line(diff))
                    .family(theme::medium())
                    .color(p.text_strong),
            )
            .truncate(),
        );
    } else {
        widgets::muted(ui, "Compare two Asset Versions of this Platform");
    }
}

/// The Bundles view: search, Game Entities, type filters, then the table
/// and the preview pane.
fn bundles(app: &mut Ae3App, ui: &mut egui::Ui) {
    search::field(app, ui);
    search::entities(app, ui);
    if app.model.manifest.is_some() {
        search::type_filters(app, ui);
    }
    if app.model.preview.is_some() {
        egui::Panel::right("ae3-preview")
            .resizable(true)
            .default_size(460.0)
            // Never more than 60% of the main area, whatever the pane's
            // contents ask for, and never so wide the table gets cramped.
            .size_range(280.0..=preview_max_width(ui.available_width()))
            .show_separator_line(false)
            .frame(
                widgets::card_frame(ui)
                    .inner_margin(Margin::same(12))
                    .outer_margin(Margin {
                        left: 10,
                        ..Margin::ZERO
                    }),
            )
            .show(ui, |ui| preview::pane(app, ui));
    }
    table::bundles(app, ui);
}
