//! The Diff tab: compare two Asset Versions and extract what changed.

use ae3_core::DiffFilter;
use eframe::egui::{self, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use super::table::{ROW_HEIGHT, click_modifiers};
use super::{error_label, warn_label};
use crate::app::Ae3App;
use crate::model::diff::{can_extract, engine_rebuild_warning, summary_line};
use crate::model::jobs::JobPhase;
use crate::model::table::format_bytes;

#[expect(
    clippy::too_many_lines,
    reason = "lays out one widget in drawing order"
)]
pub fn tab(app: &mut Ae3App, ui: &mut egui::Ui) {
    let loaded = app.model.asset_version();
    ui.horizontal_wrapped(|ui| {
        ui.label("Old Asset Version");
        ui.add(
            egui::TextEdit::singleline(&mut app.model.diff.old_text)
                .desired_width(72.0)
                .hint_text("100052"),
        );
        ui.label("New Asset Version");
        ui.add(
            egui::TextEdit::singleline(&mut app.model.diff.new_text)
                .desired_width(72.0)
                .hint_text(loaded.map_or_else(|| "loaded".to_owned(), |v| v.to_string())),
        );
        let busy = app.model.diff.job.is_active();
        if ui
            .add_enabled(!busy && loaded.is_some(), egui::Button::new("Compare"))
            .on_hover_text("Download both Manifests and list what was added, changed or removed")
            .clicked()
        {
            app.compute_diff();
        }
        if busy {
            ui.spinner();
            ui.label("Fetching Manifests…");
        }
        if let JobPhase::Failed(e) = app.model.diff.job.phase() {
            error_label(ui, e.clone());
        }
    });
    let Some(diff) = app.model.diff.diff.clone() else {
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Enter an older Asset Version to see the Packages added, changed or removed since.",
            )
            .weak(),
        );
        return;
    };
    ui.horizontal_wrapped(|ui| {
        let mut filter = app.model.diff.filter;
        for f in DiffFilter::ALL {
            let label = match f {
                DiffFilter::Added => format!("Added ({})", diff.added.len()),
                DiffFilter::Changed => format!("Changed ({})", diff.changed.len()),
                DiffFilter::All => format!("All ({})", diff.added.len() + diff.changed.len()),
                DiffFilter::Removed => format!("Removed ({})", diff.removed.len()),
            };
            ui.selectable_value(&mut filter, f, label);
        }
        app.model.diff.set_filter(filter);
        ui.separator();
        ui.label(summary_line(&diff));
    });
    if let Some(warning) = engine_rebuild_warning(&diff) {
        warn_label(ui, warning);
    }
    let filter = app.model.diff.filter;
    ui.horizontal(|ui| {
        let selected = app.model.diff.selection.len();
        let total = app.model.diff.rows.len();
        let extracting = app.model.extract_job.is_active();
        let enabled = can_extract(filter) && !extracting;
        if ui
            .add_enabled(
                enabled && selected > 0,
                egui::Button::new(format!("Extract Selected ({selected})")),
            )
            .clicked()
            && let Some(request) = app.model.diff.request(true)
        {
            app.extract(request, true);
        }
        if ui
            .add_enabled(
                enabled && total > 0,
                egui::Button::new(format!("Extract All ({total})")),
            )
            .on_hover_text(format!(
                "{} to download",
                format_bytes(diff.download_size(filter))
            ))
            .clicked()
            && let Some(request) = app.model.diff.request(false)
        {
            app.extract(request, true);
        }
        if !can_extract(filter) {
            ui.label(
                RichText::new("Removed Packages exist only in the older Asset Version.").weak(),
            );
        }
    });
    ui.separator();
    let mods = click_modifiers(ui);
    let mut clicked = None;
    let rows = &app.model.diff.rows;
    let selection = &app.model.diff.selection;
    TableBuilder::new(ui)
        .id_salt("diff")
        .striped(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::remainder().at_least(160.0).clip(true))
        .column(Column::initial(80.0))
        .column(Column::initial(84.0))
        .auto_shrink(false)
        .header(ROW_HEIGHT, |mut h| {
            h.col(|ui| {
                ui.strong("Name");
            });
            h.col(|ui| {
                ui.strong("Change");
            });
            h.col(|ui| {
                ui.strong("Size");
            });
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                let i = row.index();
                let r = &rows[i];
                row.set_selected(selection.contains(i));
                row.col(|ui| {
                    ui.label(&r.name);
                });
                row.col(|ui| {
                    ui.label(r.change.name());
                });
                row.col(|ui| {
                    ui.label(format_bytes(r.size));
                });
                if row.response().clicked() {
                    clicked = Some(i);
                }
            });
        });
    if let Some(i) = clicked {
        let visible = app.model.diff.visible();
        app.model.diff.selection.click(i, &visible, mods);
    }
}
