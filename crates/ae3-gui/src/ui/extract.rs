//! The extraction bar: Extract All / Selected / checked Assets, progress
//! with cancel, and the completion summary.

use eframe::egui::{self, RichText};

use super::{error_label, warn_label};
use crate::app::Ae3App;
use crate::model::jobs::JobPhase;

#[expect(
    clippy::too_many_lines,
    reason = "lays out one widget in drawing order"
)]
pub fn bar(app: &mut Ae3App, ui: &mut egui::Ui) {
    let job_phase = app.model.extract_job.phase().clone();
    let active = job_phase.is_active();
    ui.horizontal_wrapped(|ui| {
        // Counts only; the requests are built when a button is pressed.
        let n_all = app.model.visible.len();
        let n_selected = app.model.selection.len();
        let n_checked = app
            .model
            .preview
            .as_ref()
            .map_or(0, crate::model::preview::Preview::checked_count);
        let ready = app.store.is_some() && !active;
        if ui
            .add_enabled(
                ready && n_all > 0,
                egui::Button::new(format!("Extract All ({n_all})")),
            )
            .on_hover_text("Every Package the table shows")
            .clicked()
            && let Some(r) = app.model.request_all()
        {
            app.extract(r, false);
        }
        if ui
            .add_enabled(
                ready && n_selected > 0,
                egui::Button::new(format!("Extract Selected ({n_selected})")),
            )
            .on_hover_text("The selected Packages")
            .clicked()
            && let Some(r) = app.model.request_selected()
        {
            app.extract(r, false);
        }
        let noun = if app
            .model
            .preview
            .as_ref()
            .is_some_and(crate::model::preview::Preview::is_audio)
        {
            if n_checked == 1 { "Sound" } else { "Sounds" }
        } else if n_checked == 1 {
            "Asset"
        } else {
            "Assets"
        };
        if ui
            .add_enabled(
                ready && n_checked > 0,
                egui::Button::new(format!("Extract {n_checked} {noun}")),
            )
            .on_hover_text("The checked Assets in the preview")
            .clicked()
            && let Some(r) = app.model.request_checked()
        {
            app.extract(r, false);
        }
        ui.separator();
        if active {
            let progress = app.model.extract_job.progress().clone();
            ui.add(
                egui::ProgressBar::new(progress.fraction())
                    .desired_width(220.0)
                    .show_percentage(),
            );
            ui.label(format!(
                "{} · {}",
                app.model.extract_label,
                progress.label()
            ));
            let cancelling = job_phase == JobPhase::Cancelling;
            if ui
                .add_enabled(
                    !cancelling,
                    egui::Button::new(if cancelling {
                        "Cancelling…"
                    } else {
                        "Cancel"
                    }),
                )
                .clicked()
            {
                app.cancel_extract();
            }
        } else if let Some(summary) = app.summary().cloned() {
            if summary.failure.is_some() {
                error_label(ui, summary.headline());
            } else if summary.errors.is_empty() && !summary.cancelled {
                ui.label(RichText::new(summary.headline()).strong());
            } else {
                warn_label(ui, summary.headline());
            }
            if ui
                .button("Reveal in folder")
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
                                ui.label(RichText::new(e).small());
                            }
                        });
                });
            }
            if super::small_icon_button(ui, super::icons::CLOSE, "Dismiss summary").clicked() {
                app.model.summary = None;
            }
        } else {
            ui.label(RichText::new(format!("Output: {}", app.config.output_dir.display())).weak());
        }
    });
}
