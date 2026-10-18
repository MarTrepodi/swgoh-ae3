//! The Bundles table: sortable columns, virtualized rows and multi-select,
//! in a card.

use ae3_core::{AssetKinds, PackageKind};
use eframe::egui::{self, RichText, Sense};
use egui_extras::{Column, TableBuilder};

use super::widgets;
use super::{icons, kind_label, theme};
use crate::app::Ae3App;
use crate::model::selection::ClickModifiers;
use crate::model::table::{SortKey, format_bytes};

/// Height of a table row: room for the descenders and underscores of
/// body text.
pub const ROW_HEIGHT: f32 = 26.0;
/// Height of a table's header row.
pub const HEADER_HEIGHT: f32 = 24.0;

/// The Types cell: one icon per kind, or a hint while unknown.
pub fn kinds_text(kind: PackageKind, kinds: Option<AssetKinds>) -> String {
    match (kind, kinds) {
        (_, Some(k)) if k.is_empty() => "—".to_owned(),
        (_, Some(k)) => k.iter().map(icons::kind).collect::<Vec<_>>().join(" "),
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

/// A column title: small, medium weight, muted.
pub fn column_title(ui: &mut egui::Ui, title: &str) -> egui::Response {
    let p = theme::palette(ui);
    ui.label(
        RichText::new(title)
            .family(theme::medium())
            .size(theme::size::SMALL)
            .color(p.text_muted),
    )
}

fn header_button(ui: &mut egui::Ui, app: &mut Ae3App, title: &str, key: SortKey) {
    let p = theme::palette(ui);
    let sort = app.model.sort;
    let arrow = match (sort.key == key, sort.descending) {
        (true, false) => icons::SORT_UP,
        (true, true) => icons::SORT_DOWN,
        _ => "",
    };
    let color = if sort.key == key {
        p.text
    } else {
        p.text_muted
    };
    let response = ui
        .add(
            egui::Label::new(
                RichText::new(format!("{title} {arrow}"))
                    .family(theme::medium())
                    .size(theme::size::SMALL)
                    .color(color),
            )
            .sense(Sense::click()),
        )
        .on_hover_text(format!("Sort by {}", title.to_lowercase()))
        .accessible_name(title);
    if response.clicked() {
        app.model.sort.click(key);
        app.model.refresh_visible();
    }
}

/// What the table shows before a Manifest is loaded.
pub fn empty_state(ui: &mut egui::Ui, icon: &str, title: &str, hint: &str) {
    let p = theme::palette(ui);
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 2.0 - 70.0).max(16.0));
        ui.label(RichText::new(icon).size(40.0).color(p.text_muted));
        ui.add_space(4.0);
        ui.label(RichText::new(title).text_style(theme::section_style()));
        ui.add(egui::Label::new(RichText::new(hint).color(p.text_muted)).wrap());
    });
}

/// The table of Packages, in a card filling the space left.
pub fn bundles(app: &mut Ae3App, ui: &mut egui::Ui) {
    widgets::card_frame(ui)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            if app.model.manifest.is_none() {
                empty_state(
                    ui,
                    icons::BUNDLES,
                    "No Manifest loaded",
                    "Choose a Platform and press Fetch latest, or enter an Asset Version and \
                     press Load Manifest.",
                );
                return;
            }
            table(app, ui);
        });
}

fn table(app: &mut Ae3App, ui: &mut egui::Ui) {
    let mods = click_modifiers(ui);
    let mut clicked: Option<usize> = None;
    let p = theme::palette(ui);
    // Borrowed out of the model while drawing; put back below.
    let visible = std::mem::take(&mut app.model.visible);
    // A click on a cell's text selects its row. Selectable labels would
    // take the click for text selection, so the row never saw it.
    ui.style_mut().interaction.selectable_labels = false;
    TableBuilder::new(ui)
        .id_salt("bundles")
        .striped(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::remainder().at_least(140.0).clip(true))
        .column(Column::initial(110.0).at_least(48.0).clip(true))
        .column(Column::initial(80.0).at_least(64.0).clip(true))
        .auto_shrink(false)
        .header(HEADER_HEIGHT, |mut header| {
            header.col(|ui| header_button(ui, app, "Name", SortKey::Name));
            header.col(|ui| header_button(ui, app, "Types", SortKey::Types));
            header.col(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    header_button(ui, app, "Size", SortKey::Size);
                });
            });
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
                                .size(theme::size::SMALL)
                                .color(p.amber),
                        );
                    }
                });
                row.col(|ui| {
                    ui.label(RichText::new(kinds_text(r.kind, r.kinds)).color(p.text_muted))
                        .on_hover_text(kinds_tooltip(r.kinds));
                });
                row.col(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format_bytes(r.size)).color(p.text_muted));
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
        assert_eq!(
            kinds_text(PackageKind::Bundle, Some(kinds)),
            format!("{} {}", icons::TEXTURE, icons::ATLAS)
        );
        assert_eq!(kinds_text(PackageKind::Bundle, None), "…");
        assert_eq!(
            kinds_text(PackageKind::Bundle, Some(AssetKinds::EMPTY)),
            "—"
        );
        assert_eq!(kinds_tooltip(Some(kinds)), "Texture, Atlas");
    }
}
