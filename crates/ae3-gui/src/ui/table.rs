//! The Bundles table: type filter, sortable columns, virtualized rows and
//! multi-select.

use std::fmt::Write as _;

use ae3_core::{AssetKinds, PackageKind};
use eframe::egui::{self, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use super::{glyph, kind_label};
use crate::app::Ae3App;
use crate::model::selection::ClickModifiers;
use crate::model::table::{SHOWN_KINDS, SortKey, format_bytes};

pub const ROW_HEIGHT: f32 = 20.0;

/// The Types cell: one glyph per kind, or a hint while unknown.
pub fn kinds_text(kind: PackageKind, kinds: Option<AssetKinds>) -> String {
    match (kind, kinds) {
        (_, Some(k)) if k.is_empty() => "—".to_owned(),
        (_, Some(k)) => k.iter().map(super::glyph).collect::<Vec<_>>().join(" "),
        (PackageKind::Other(_), None) => "?".to_owned(),
        (_, None) => "…".to_owned(),
    }
}

fn kinds_tooltip(kinds: Option<AssetKinds>) -> String {
    match kinds {
        Some(k) if k.is_empty() => "No exportable Assets".to_owned(),
        Some(k) => k.iter().map(kind_label).collect::<Vec<_>>().join(", "),
        None => "Not indexed yet".to_owned(),
    }
}

/// The current modifiers as a click modifier set.
pub fn click_modifiers(ui: &egui::Ui) -> ClickModifiers {
    ui.input(|i| ClickModifiers {
        shift: i.modifiers.shift,
        command: i.modifiers.command,
    })
}

fn type_filter(app: &mut Ae3App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Types");
        let mut changed = false;
        for kind in SHOWN_KINDS {
            let on = app.model.filter.kinds.contains(kind);
            let text = format!("{} {}", glyph(kind), kind_label(kind));
            if ui
                .selectable_label(on, text)
                .on_hover_text(format!("Only Packages holding {} Assets", kind_label(kind)))
                .clicked()
            {
                app.model.filter.toggle(kind);
                changed = true;
            }
        }
        if app.model.filter.is_active() && ui.small_button("All").clicked() {
            app.model.filter = crate::model::table::TypeFilter::default();
            changed = true;
        }
        if changed {
            app.model.refresh_visible();
        }
        ui.separator();
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
        ui.label(RichText::new(text).weak());
        if !app.model.visible.is_empty() && ui.small_button("Select all").clicked() {
            let visible = app.model.visible.clone();
            app.model.selection.select_all(&visible);
        }
        if !app.model.selection.is_empty() && ui.small_button("Clear").clicked() {
            app.model.selection.clear();
        }
    });
}

fn header_button(ui: &mut egui::Ui, app: &mut Ae3App, title: &str, key: SortKey) {
    let sort = app.model.sort;
    let arrow = match (sort.key == key, sort.descending) {
        (true, false) => " ▴",
        (true, true) => " ▾",
        _ => "",
    };
    let response = ui.add(
        egui::Label::new(RichText::new(format!("{title}{arrow}")).strong()).sense(Sense::click()),
    );
    if response.clicked() {
        app.model.sort.click(key);
        app.model.refresh_visible();
    }
}

/// The table of Packages.
pub fn bundles(app: &mut Ae3App, ui: &mut egui::Ui) {
    if app.model.manifest.is_none() {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("Choose a Platform and press Fetch Latest, or enter an Asset Version and press Load Manifest.")
                    .weak(),
            );
        });
        return;
    }
    type_filter(app, ui);
    ui.add_space(2.0);
    let mods = click_modifiers(ui);
    let mut clicked: Option<usize> = None;
    // Borrowed out of the model while drawing; put back below.
    let visible = std::mem::take(&mut app.model.visible);
    TableBuilder::new(ui)
        .id_salt("bundles")
        .striped(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::remainder().at_least(160.0).clip(true))
        .column(Column::initial(120.0).at_least(60.0).clip(true))
        .column(Column::initial(84.0).at_least(60.0))
        .auto_shrink(false)
        .header(ROW_HEIGHT, |mut header| {
            header.col(|ui| header_button(ui, app, "Name", SortKey::Name));
            header.col(|ui| header_button(ui, app, "Types", SortKey::Types));
            header.col(|ui| header_button(ui, app, "Size", SortKey::Size));
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, visible.len(), |mut row| {
                let Some(r) = visible
                    .get(row.index())
                    .and_then(|&id| app.model.rows.get(id))
                else {
                    return;
                };
                row.set_selected(app.model.selection.contains(r.id));
                let matched = app
                    .model
                    .search
                    .as_ref()
                    .map_or(0, |s| s.assets_of(&r.name).len());
                row.col(|ui| {
                    ui.label(&r.name);
                    if matched > 0 {
                        let s = if matched == 1 { "" } else { "es" };
                        ui.label(
                            RichText::new(format!("{matched} match{s}"))
                                .small()
                                .color(super::match_color(ui)),
                        );
                    }
                });
                row.col(|ui| {
                    ui.label(kinds_text(r.kind, r.kinds))
                        .on_hover_text(kinds_tooltip(r.kinds));
                });
                row.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format_bytes(r.size));
                    });
                });
                if row.response().clicked() {
                    clicked = Some(r.id);
                }
            });
        });
    app.model.visible = visible;
    if let Some(id) = clicked {
        app.model.click_row(id, mods);
    }
}

#[cfg(test)]
mod tests {
    use ae3_core::AssetKind;

    use super::*;

    #[test]
    fn types_cell_text() {
        let kinds: AssetKinds = [AssetKind::Texture, AssetKind::Atlas].into_iter().collect();
        assert_eq!(kinds_text(PackageKind::Bundle, Some(kinds)), "▣ ▦");
        assert_eq!(kinds_text(PackageKind::Bundle, None), "…");
        assert_eq!(
            kinds_text(PackageKind::Bundle, Some(AssetKinds::EMPTY)),
            "—"
        );
        assert_eq!(kinds_tooltip(Some(kinds)), "Texture, Atlas");
    }
}
