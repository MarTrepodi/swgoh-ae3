//! Above the Bundles table: the search field, the Game Entities whose
//! display names match, and the Asset type filters.

use std::fmt::Write as _;

use eframe::egui::{self, FontId, Margin, Rect, RichText, vec2};

use super::widgets;
use super::{icons, kind_label, theme};
use crate::app::Ae3App;
use crate::model::names::EntityMatch;
use crate::model::table::{SHOWN_KINDS, TypeFilter};

/// How many matching Game Entities are listed before "N more".
const ENTITIES_SHOWN: usize = 8;
/// How many are listed at most once expanded; a query matching more is
/// better refined than scrolled.
const ENTITIES_MAX: usize = 200;
/// The search text's size.
const SEARCH_SIZE: f32 = 16.0;

/// The large search field, with a search icon and a clear button.
pub fn field(app: &mut Ae3App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let mut query = app.model.query.clone();
    let enabled = app.model.manifest.is_some();
    let edit = ui
        .add_enabled(
            enabled,
            egui::TextEdit::singleline(&mut query)
                .desired_width(f32::INFINITY)
                .font(FontId::proportional(SEARCH_SIZE))
                .margin(Margin {
                    left: 12,
                    right: 36,
                    top: 9,
                    bottom: 9,
                })
                .prefix(RichText::new(icons::SEARCH).size(18.0).color(p.text_muted))
                .hint_text("Search Assets, Bundles or display names"),
        )
        .accessible_name("Asset Search");
    if edit.changed() {
        app.set_query(query);
    }
    if !app.model.query.is_empty() {
        // Inside the field's right end. A child Ui, so the cursor stays put.
        let size = vec2(28.0, 28.0);
        let rect = Rect::from_center_size(edit.rect.right_center() - vec2(20.0, 0.0), size);
        let mut clear = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        if widgets::icon_button(&mut clear, icons::CLOSE, "Clear search").clicked() {
            app.set_query(String::new());
        }
    }
}

fn entity_tooltip(entity: &EntityMatch) -> String {
    let mut text = format!("{} · {}", entity.kind.label(), entity.id);
    if entity.bundles.is_empty() {
        text.push_str("\nNo linked Bundle in this Manifest");
    } else {
        for bundle in &entity.bundles {
            let _ = write!(text, "\n{bundle}");
        }
        text.push_str("\n\nClick to show only these Bundles; click again for every match.");
    }
    text
}

/// The Game Entities whose display names match, as chips that narrow the
/// Bundles table to their Bundles.
pub fn entities(app: &mut Ae3App, ui: &mut egui::Ui) {
    let Some(search) = &app.model.search else {
        return;
    };
    let total = search.entities.len();
    if total == 0 {
        return;
    }
    let expanded = app.model.entities_expanded;
    let shown = total.min(if expanded {
        ENTITIES_MAX
    } else {
        ENTITIES_SHOWN
    });
    let picked_key = app.model.entity.as_ref();
    let mut picked = None;
    let mut toggle_expanded = false;
    let mut list = |ui: &mut egui::Ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            widgets::group_label(ui, "Game Entities");
            for entity in &search.entities[..shown] {
                let on = picked_key.is_some_and(|k| entity.is(k));
                let n = entity.bundles.len();
                let text = icons::with(
                    icons::entity(entity.kind),
                    &format!("{} · {n}", entity.label()),
                );
                let name = format!("{} {} · {n}", entity.kind.label(), entity.label());
                let response = widgets::entity_chip(ui, on, n > 0, &text, &name)
                    .on_hover_ui(|ui| {
                        ui.label(entity_tooltip(entity));
                    })
                    .on_disabled_hover_ui(|ui| {
                        ui.label(entity_tooltip(entity));
                    });
                if response.clicked() {
                    picked = Some(entity.key());
                }
            }
            if expanded && total > shown {
                widgets::caption(
                    ui,
                    format!("and {} more; type more to narrow", total - shown),
                );
            }
            if total > ENTITIES_SHOWN {
                let label = if expanded {
                    "Fewer".to_owned()
                } else {
                    format!("{} more…", total - ENTITIES_SHOWN)
                };
                if ui.small_button(label).clicked() {
                    toggle_expanded = true;
                }
            }
        });
    };
    if expanded {
        egui::ScrollArea::vertical()
            .id_salt("entities")
            .max_height(132.0)
            .show(ui, |ui| list(ui));
    } else {
        list(ui);
    }
    if toggle_expanded {
        app.model.entities_expanded = !expanded;
    }
    if picked.is_some() {
        app.model.pick_entity(picked);
    }
}

/// The Asset type filters, as toggle chips.
pub fn type_filters(app: &mut Ae3App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        widgets::group_label(ui, "Asset types");
        let mut changed = false;
        for kind in SHOWN_KINDS {
            let on = app.model.filter.kinds.contains(kind);
            if widgets::toggle_chip(ui, on, Some(icons::kind(kind)), kind_label(kind))
                .on_hover_text(format!("Only Packages holding {} Assets", kind_label(kind)))
                .clicked()
            {
                app.model.filter.toggle(kind);
                changed = true;
            }
        }
        if app.model.filter.is_active()
            && widgets::button(ui, None, "All types", true)
                .on_hover_text("Show Packages of every type")
                .clicked()
        {
            app.model.filter = TypeFilter::default();
            changed = true;
        }
        if changed {
            app.model.refresh_visible();
        }
    });
}
