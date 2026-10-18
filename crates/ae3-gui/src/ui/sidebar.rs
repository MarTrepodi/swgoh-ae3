//! The sidebar: the Game data card (Platform, Asset Version, Fetch latest
//! and Load Manifest), a summary of the Output settings, and the Asset
//! Index card with its build progress and the display names' status.

use std::fmt::Write as _;
use std::time::Instant;

use ae3_core::Platform;
use eframe::egui::{self, Frame, Margin, Stroke};

use super::widgets::{self, Tone};
use super::{icons, settings, theme};
use crate::app::{Ae3App, SettingChange};
use crate::model::jobs::JobPhase;
use crate::model::names::NamesState;

/// The sidebar's width: its default and the range it can be dragged to.
const WIDTH: f32 = 244.0;
const MIN_WIDTH: f32 = 220.0;
const MAX_WIDTH: f32 = 340.0;

pub fn panel(app: &mut Ae3App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    egui::Panel::left("ae3-sidebar")
        .resizable(true)
        .default_size(WIDTH)
        .size_range(MIN_WIDTH..=MAX_WIDTH)
        .frame(
            Frame::new()
                .fill(p.chrome)
                .inner_margin(Margin::same(12))
                .stroke(Stroke::new(1.0, p.border)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("ae3-sidebar-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 10.0;
                    widgets::card(ui, |ui| game_data(app, ui));
                    widgets::card(ui, |ui| output(app, ui));
                    widgets::card(ui, |ui| asset_index(app, ui));
                });
        });
}

/// Platform, Asset Version, Fetch latest and Load Manifest, what loaded,
/// and the newer-Asset-Version notice.
fn game_data(app: &mut Ae3App, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.y = 6.0;
    widgets::card_title(ui, "Game data");
    ui.add_space(2.0);
    let busy = app.model.manifest_job.is_active();

    widgets::group_label(ui, "Platform");
    let mut platform = app.config.platform;
    let options = Platform::ALL.map(|p| (p, None, p.name()));
    ui.add_enabled_ui(!busy, |ui| {
        widgets::segmented(ui, &mut platform, &options, true);
    });
    if platform != app.config.platform {
        app.change_setting(SettingChange::Platform(platform));
    }

    widgets::group_label(ui, "Asset Version");
    let edit = ui
        .add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut app.model.version_text)
                .desired_width(f32::INFINITY)
                .hint_text("latest"),
        )
        .accessible_name("Asset Version");
    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.add_space(2.0);
    if widgets::wide_button(ui, Some(icons::FETCH), "Fetch latest", !busy)
        .on_hover_text("Find the newest Asset Version and load its Manifest")
        .clicked()
    {
        app.load_manifest(None);
    }
    if widgets::wide_button(ui, Some(icons::LOAD), "Load Manifest", !busy)
        .on_hover_text("Load the Manifest of the Asset Version entered")
        .clicked()
        || (enter && !busy)
    {
        match crate::model::diff::parse_version(&app.model.version_text) {
            Ok(version) => app.load_manifest(version),
            Err(e) => app.last_load_error = Some(e),
        }
    }

    if busy {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            widgets::muted(ui, app.model.manifest_job.progress().clone());
        });
    } else if let Some(e) = &app.last_load_error {
        widgets::status(ui, Tone::Error, e.clone());
    } else if let Some(manifest) = &app.model.manifest {
        let bundles = manifest
            .packages
            .iter()
            .filter(|p| p.kind == ae3_core::PackageKind::Bundle)
            .count();
        let audio = manifest.packages.len() - bundles;
        widgets::caption(ui, format!("{bundles} Bundles · {audio} Audio Packages"));
        if app.model.startup.is_pending() {
            // No spinner: it would repaint the window until GitHub answers.
            widgets::caption(ui, "Looking for a newer Asset Version…");
        }
    }
    if let Some(newer) = app.model.update.newer() {
        let p = theme::palette(ui);
        Frame::new()
            .fill(p.inset)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(theme::CONTROL_RADIUS)
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::status(
                    ui,
                    Tone::Warning,
                    format!("Asset Version {newer} available"),
                );
                ui.horizontal(|ui| {
                    if widgets::button(ui, Some(icons::LOAD), "Load", !busy).clicked() {
                        app.model.version_text = newer.to_string();
                        app.load_manifest(Some(newer));
                    }
                    if widgets::button(ui, None, "Dismiss", true)
                        .accessible_name("Dismiss Asset Version notice")
                        .clicked()
                    {
                        app.model.update.dismiss();
                    }
                });
            });
    }
}

/// Where extractions go and how, in short, with the button that opens
/// every setting.
fn output(app: &mut Ae3App, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.y = 6.0;
    widgets::card_title(ui, "Output");
    ui.add_space(2.0);
    let p = theme::palette(ui);
    let dir = app.config.output_dir.display().to_string();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(icons::FOLDER).color(p.text_muted));
        ui.add(
            egui::Label::new(egui::RichText::new(&dir).monospace())
                .truncate()
                .show_tooltip_when_elided(true),
        )
        .on_hover_text(&dir);
    });
    for line in settings::summary(&app.config) {
        widgets::caption(ui, line);
    }
    ui.add_space(2.0);
    if widgets::wide_button(ui, Some(icons::SETTINGS), "Edit settings", true)
        .on_hover_text("Output folder, formats, Atlas Cutting and layout")
        .clicked()
    {
        app.model.windows.settings = true;
    }
}

/// The Asset Index build: progress with pause and resume, what it covers,
/// and how the display names stand.
fn asset_index(app: &mut Ae3App, ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.y = 6.0;
    widgets::card_title(ui, "Asset Index");
    ui.add_space(2.0);
    let phase = app.model.index_job.phase().clone();
    match phase {
        JobPhase::Running | JobPhase::Paused | JobPhase::Cancelling => {
            let fraction = app.model.index_job.progress().fraction();
            let label = app.model.index_label(Instant::now()).unwrap_or_default();
            widgets::progress(ui, fraction, ui.available_width());
            widgets::caption(ui, format!("{:.0}% · {label}", fraction * 100.0));
            let paused = app.index_pause_requested();
            let (icon, text, tip) = if paused {
                (icons::PLAY, "Resume", "Continue building the Asset Index")
            } else {
                (
                    icons::PAUSE,
                    "Pause",
                    "Stop fetching for now; progress is kept",
                )
            };
            if widgets::button(ui, Some(icon), text, true)
                .on_hover_text(tip)
                .clicked()
            {
                app.toggle_index_pause();
            }
        }
        JobPhase::Failed(e) => {
            widgets::status(ui, Tone::Error, format!("Asset Index build failed: {e}"));
            if widgets::button(ui, Some(icons::RETRY), "Retry", true).clicked() {
                app.restart_index();
            }
        }
        JobPhase::Finished | JobPhase::Cancelled | JobPhase::Idle => {
            coverage(app, ui);
        }
    }
    names(app, ui);
}

/// How much of the Manifest the Asset Index covers, with Retry when
/// Bundles are missing because the network failed.
fn coverage(app: &mut Ae3App, ui: &mut egui::Ui) {
    let Some(c) = app.model.coverage else {
        let text = if app.model.manifest.is_some() {
            "Not built yet."
        } else {
            "Built in the background once a Manifest is loaded."
        };
        widgets::caption(ui, text);
        return;
    };
    let mut text = format!(
        "{} of {} Bundles indexed",
        c.indexed_bundles, c.total_bundles
    );
    if c.failed_bundles > 0 {
        let _ = write!(text, ", {} unreadable", c.failed_bundles);
    }
    let tone = if c.is_complete() {
        Tone::Ok
    } else {
        Tone::Neutral
    };
    widgets::status(ui, tone, text);
    if !c.is_complete() && c.pending_bundles() > 0 && !app.model.index_job.is_active() {
        widgets::status(
            ui,
            Tone::Warning,
            format!(
                "{} Bundles could not be indexed (network); search them by name only.",
                c.pending_bundles()
            ),
        );
        if widgets::button(ui, Some(icons::RETRY), "Retry", true).clicked() {
            app.restart_index();
        }
    }
}

/// The display names' status line.
fn names(app: &Ae3App, ui: &mut egui::Ui) {
    let Some((line, warning)) = app.model.names_state.line() else {
        return;
    };
    ui.add_space(2.0);
    widgets::group_label(ui, "Display names");
    if warning {
        widgets::status(ui, Tone::Warning, line);
    } else {
        let loading = matches!(app.model.names_state, NamesState::Loading { .. });
        let p = theme::palette(ui);
        let color = if loading { p.text_muted } else { p.green };
        widgets::with_icon(ui, egui::RichText::new(icons::NAMES).color(color), |ui| {
            widgets::caption(ui, line)
        });
    }
}
