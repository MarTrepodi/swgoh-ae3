//! The action bar at the bottom: the extraction's status on the left
//! (progress with Cancel, or the completion summary), the extractions on
//! the right. Exactly one is the primary action: the most specific one
//! available.

use eframe::egui::{self, Frame, Margin, Stroke};

use super::widgets::{self, Tone};
use super::{icons, theme};
use crate::app::Ae3App;
use crate::model::Tab;
use crate::model::diff::can_extract;
use crate::model::jobs::JobPhase;
use crate::model::table::format_bytes;

/// An extraction the bar offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extraction {
    /// Every Package the table shows.
    All,
    /// The selected Packages.
    Selected,
    /// The Assets or sounds checked in the preview.
    Checked,
}

/// Which extraction is the primary action: the checked Assets, else the
/// selected Packages, else everything.
#[must_use]
pub fn primary(selected: usize, checked: usize) -> Extraction {
    if checked > 0 {
        Extraction::Checked
    } else if selected > 0 {
        Extraction::Selected
    } else {
        Extraction::All
    }
}

pub fn bar(app: &mut Ae3App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    egui::Panel::bottom("ae3-actions")
        .frame(
            Frame::new()
                .fill(p.chrome)
                .inner_margin(Margin::symmetric(16, 10))
                .stroke(Stroke::new(1.0, p.border)),
        )
        .show(ui, |ui| {
            // Right to left: the primary action is the rightmost.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.set_min_height(34.0);
                match app.model.tab {
                    Tab::Bundles => bundle_actions(app, ui),
                    Tab::Diff => diff_actions(app, ui),
                }
            });
        });
}

/// The status, drawn at the left of the right-to-left bar: everything
/// added after the actions lands left of them, so the status goes into a
/// left-to-right child that takes the space that is left.
fn status(app: &mut Ae3App, ui: &mut egui::Ui, idle: impl FnOnce(&mut egui::Ui)) {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let phase = app.model.extract_job.phase().clone();
        if phase.is_active() {
            let progress = app.model.extract_job.progress().clone();
            widgets::progress(ui, progress.fraction(), 200.0);
            ui.label(format!(
                "{:.0}% · {} · {}",
                progress.fraction() * 100.0,
                app.model.extract_label,
                progress.label()
            ));
            let cancelling = phase == JobPhase::Cancelling;
            let text = if cancelling {
                "Cancelling…"
            } else {
                "Cancel"
            };
            if widgets::button(ui, Some(icons::CLOSE), text, !cancelling).clicked() {
                app.cancel_extract();
            }
        } else if let Some(summary) = app.summary().cloned() {
            let tone = if summary.failure.is_some() {
                Tone::Error
            } else if summary.errors.is_empty() && !summary.cancelled {
                Tone::Ok
            } else {
                Tone::Warning
            };
            widgets::status(ui, tone, summary.headline());
            if widgets::button(ui, Some(icons::FOLDER), "Reveal in folder", true)
                .on_hover_text(summary.output_dir.display().to_string())
                .clicked()
            {
                app.reveal_output();
            }
            if !summary.errors.is_empty() {
                ui.menu_button(format!("Errors ({})", summary.errors.len()), |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for e in &summary.errors {
                                ui.label(egui::RichText::new(e).small());
                            }
                        });
                });
            }
            if widgets::icon_button(ui, icons::CLOSE, "Dismiss summary").clicked() {
                app.model.summary = None;
            }
        } else {
            idle(ui);
        }
    });
}

fn bundle_actions(app: &mut Ae3App, ui: &mut egui::Ui) {
    // Counts only; the requests are built when a button is pressed.
    let n_all = app.model.visible.len();
    let n_selected = app.model.selection.len();
    let preview = app.model.preview.as_ref();
    let n_checked = preview.map_or(0, crate::model::preview::Preview::checked_count);
    let audio = preview.is_some_and(crate::model::preview::Preview::is_audio);
    let ready = app.store.is_some() && !app.model.extract_job.is_active();
    let primary = primary(n_selected, n_checked);
    let checked_label = {
        let noun = match (audio, n_checked == 1) {
            (true, true) => "Sound",
            (true, false) => "Sounds",
            (false, true) => "Asset",
            (false, false) => "Assets",
        };
        format!("Extract {n_checked} {noun}")
    };
    let selected_label = format!("Extract selected ({n_selected})");
    let all_label = format!("Extract all ({n_all})");
    // Right to left: the primary first, then what else is on offer.
    let mut offered = vec![primary];
    if primary == Extraction::Checked && n_selected > 0 {
        offered.push(Extraction::Selected);
    }
    if primary != Extraction::All {
        offered.push(Extraction::All);
    }
    let mut pressed = None;
    for (i, extraction) in offered.into_iter().enumerate() {
        let (label, count, tip) = match extraction {
            Extraction::All => (&all_label, n_all, "Every Package the table shows"),
            Extraction::Selected => (&selected_label, n_selected, "The selected Packages"),
            Extraction::Checked => (
                &checked_label,
                n_checked,
                "The checked Assets in the preview",
            ),
        };
        let enabled = ready && count > 0;
        let response = if i == 0 {
            widgets::primary_button(ui, Some(icons::EXTRACT), label, enabled)
        } else {
            widgets::button(ui, None, label, enabled)
        };
        if response.on_hover_text(tip).clicked() {
            pressed = Some(extraction);
        }
    }
    let request = match pressed {
        Some(Extraction::All) => app.model.request_all(),
        Some(Extraction::Selected) => app.model.request_selected(),
        Some(Extraction::Checked) => app.model.request_checked(),
        None => None,
    };
    if let Some(request) = request {
        app.extract(request, false);
    }
    let dir = app.config.output_dir.display().to_string();
    status(app, ui, |ui| {
        let p = theme::palette(ui);
        widgets::with_icon(
            ui,
            egui::RichText::new(icons::FOLDER).color(p.text_muted),
            |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("Output: {dir}")).color(p.text_muted),
                    )
                    .truncate(),
                )
            },
        );
    });
}

fn diff_actions(app: &mut Ae3App, ui: &mut egui::Ui) {
    let filter = app.model.diff.filter;
    let selected = app.model.diff.selection.len();
    let total = app.model.diff.rows.len();
    let enabled = can_extract(filter) && !app.model.extract_job.is_active();
    let download = app
        .model
        .diff
        .diff
        .as_ref()
        .map(|d| format_bytes(d.download_size(filter)));
    let all_label = format!("Extract all ({total})");
    let selected_label = format!("Extract selected ({selected})");
    let mut pressed = None;
    if selected > 0 {
        if widgets::primary_button(ui, Some(icons::EXTRACT), &selected_label, enabled)
            .on_hover_text("The selected Packages of the newer Asset Version")
            .clicked()
        {
            pressed = Some(true);
        }
        if widgets::button(ui, None, &all_label, enabled && total > 0)
            .on_hover_text(
                download
                    .clone()
                    .map_or_else(String::new, |d| format!("{d} to download")),
            )
            .clicked()
        {
            pressed = Some(false);
        }
    } else if widgets::primary_button(ui, Some(icons::EXTRACT), &all_label, enabled && total > 0)
        .on_hover_text(download.map_or_else(String::new, |d| format!("{d} to download")))
        .clicked()
    {
        pressed = Some(false);
    }
    if let Some(selected_only) = pressed
        && let Some(request) = app.model.diff.request(selected_only)
    {
        app.extract(request, true);
    }
    let removed = !can_extract(filter);
    let has_diff = app.model.diff.diff.is_some();
    status(app, ui, |ui| {
        if removed {
            widgets::muted(
                ui,
                "Removed Packages exist only in the older Asset Version.",
            );
        } else if has_diff {
            widgets::muted(ui, "Extracts from the newer Asset Version.");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_specific_extraction_is_primary() {
        assert_eq!(primary(0, 0), Extraction::All);
        assert_eq!(primary(3, 0), Extraction::Selected);
        assert_eq!(primary(3, 2), Extraction::Checked);
        assert_eq!(primary(0, 1), Extraction::Checked);
    }
}
