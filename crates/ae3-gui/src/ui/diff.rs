//! The Diff tab: compare two Asset Versions and extract what changed. The
//! action bar carries the extractions.

use ae3_core::DiffFilter;
use eframe::egui::{self, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use super::table::{HEADER_HEIGHT, ROW_HEIGHT, click_modifiers, column_title, empty_state};
use super::widgets::{self, Tone};
use super::{icons, theme};
use crate::app::Ae3App;
use crate::model::diff::{Change, engine_rebuild_warning};
use crate::model::jobs::JobPhase;
use crate::model::table::format_bytes;

pub fn tab(app: &mut Ae3App, ui: &mut egui::Ui) {
    widgets::card(ui, |ui| compare(app, ui));
    ui.add_space(8.0);
    let Some(diff) = app.model.diff.diff.clone() else {
        widgets::card_frame(ui).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            empty_state(
                ui,
                icons::DIFF,
                "Nothing compared yet",
                "Enter an older Asset Version to see the Packages added, changed or removed since.",
            );
        });
        return;
    };
    ui.horizontal_wrapped(|ui| {
        let mut filter = app.model.diff.filter;
        let labels = DiffFilter::ALL.map(|f| {
            let label = match f {
                DiffFilter::Added => format!("Added ({})", diff.added.len()),
                DiffFilter::Changed => format!("Changed ({})", diff.changed.len()),
                DiffFilter::All => format!("All ({})", diff.added.len() + diff.changed.len()),
                DiffFilter::Removed => format!("Removed ({})", diff.removed.len()),
            };
            (f, label)
        });
        let options: Vec<_> = labels
            .iter()
            .map(|(f, label)| (*f, None, label.as_str()))
            .collect();
        widgets::segmented(ui, &mut filter, &options, false);
        app.model.diff.set_filter(filter);
    });
    if let Some(warning) = engine_rebuild_warning(&diff) {
        widgets::status(ui, Tone::Warning, warning);
    }
    ui.add_space(4.0);
    widgets::card_frame(ui)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            table(app, ui);
        });
}

/// The two Asset Versions and Compare.
fn compare(app: &mut Ae3App, ui: &mut egui::Ui) {
    let loaded = app.model.asset_version();
    let busy = app.model.diff.job.is_active();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.vertical(|ui| {
            widgets::group_label(ui, "Old Asset Version");
            ui.add(
                egui::TextEdit::singleline(&mut app.model.diff.old_text)
                    .desired_width(110.0)
                    .hint_text("100052"),
            )
            .accessible_name("Old Asset Version");
        });
        ui.vertical(|ui| {
            widgets::group_label(ui, "New Asset Version");
            ui.add(
                egui::TextEdit::singleline(&mut app.model.diff.new_text)
                    .desired_width(110.0)
                    .hint_text(loaded.map_or_else(|| "loaded".to_owned(), |v| v.to_string())),
            )
            .accessible_name("New Asset Version");
        });
        ui.vertical(|ui| {
            widgets::group_label(ui, " ");
            if widgets::button(ui, Some(icons::DIFF), "Compare", !busy && loaded.is_some())
                .on_hover_text(
                    "Download both Manifests and list what was added, changed or removed",
                )
                .clicked()
            {
                app.compute_diff();
            }
        });
        ui.vertical(|ui| {
            widgets::group_label(ui, " ");
            ui.horizontal(|ui| {
                ui.set_min_height(ui.spacing().interact_size.y);
                if busy {
                    ui.spinner();
                    widgets::muted(ui, "Fetching Manifests…");
                } else if let JobPhase::Failed(e) = app.model.diff.job.phase() {
                    widgets::status(ui, Tone::Error, e.clone());
                }
            });
        });
    });
}

fn table(app: &mut Ae3App, ui: &mut egui::Ui) {
    let mods = click_modifiers(ui);
    let mut clicked = None;
    let p = theme::palette(ui);
    let rows = &app.model.diff.rows;
    let selection = &app.model.diff.selection;
    // A click on a cell's text selects its row. Selectable labels would
    // take the click for text selection, so the row never saw it.
    ui.style_mut().interaction.selectable_labels = false;
    TableBuilder::new(ui)
        .id_salt("diff")
        .striped(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::remainder().at_least(160.0).clip(true))
        .column(Column::initial(96.0))
        .column(Column::initial(84.0))
        .auto_shrink(false)
        .header(HEADER_HEIGHT, |mut h| {
            h.col(|ui| {
                column_title(ui, "Name");
            });
            h.col(|ui| {
                column_title(ui, "Change");
            });
            h.col(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    column_title(ui, "Size");
                });
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
                    let color = match r.change {
                        Change::Added => p.green,
                        Change::Changed => p.amber,
                        Change::Removed => p.red,
                    };
                    ui.label(RichText::new(r.change.name()).color(color));
                });
                row.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format_bytes(r.size)).color(p.text_muted));
                    });
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
