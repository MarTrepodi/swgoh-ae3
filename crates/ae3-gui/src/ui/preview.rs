//! The preview pane: a grid of Asset cells with thumbnails, or the sound
//! list of an Audio Package.

use std::fmt::Write as _;

use ae3_core::AssetInfo;
use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, vec2};
use egui_extras::{Column, TableBuilder};

use super::{error_label, glyph, kind_label, match_color};
use crate::app::Ae3App;
use crate::model::preview::{PreviewStatus, has_image, summary};
use crate::model::table::format_bytes;

const CELL_W: f32 = 124.0;
const CELL_H: f32 = 158.0;
const THUMB: f32 = 112.0;

pub fn pane(app: &mut Ae3App, ui: &mut egui::Ui) {
    let Some(preview) = &app.model.preview else {
        return;
    };
    let size = app.model.rows.get(preview.row_id).map_or(0, |r| r.size);
    let textures = format!(
        "{} decoded textures in memory: {} of {}",
        app.textures.len(),
        format_bytes(app.textures.used_bytes()),
        format_bytes(app.textures.capacity())
    );
    ui.horizontal(|ui| {
        ui.heading(&preview.package);
        ui.label(RichText::new(format_bytes(size)).weak())
            .on_hover_text(textures);
    });
    match &preview.status {
        PreviewStatus::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(app.preview_status.as_deref().unwrap_or("Opening…"));
            });
        }
        PreviewStatus::Failed(e) => error_label(ui, e.clone()),
        PreviewStatus::Bundle { .. } => bundle_grid(app, ui),
        PreviewStatus::Audio { .. } => sound_list(app, ui),
    }
}

fn toolbar(app: &mut Ae3App, ui: &mut egui::Ui) {
    let Some(preview) = &mut app.model.preview else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        if ui.button("Select All").clicked() {
            preview.check_all();
        }
        if ui.button("Clear").clicked() {
            preview.clear_checks();
        }
        let n = preview.checked_count();
        ui.label(RichText::new(format!("{n} checked")).weak());
        if !preview.is_audio() {
            let hidden = preview.hidden_count();
            let mut show = preview.show_other;
            if (hidden > 0 || show)
                && ui
                    .checkbox(&mut show, format!("Scene objects ({hidden})"))
                    .on_hover_text("Also show GameObjects, Transforms, MonoScripts and the like")
                    .changed()
            {
                preview.set_show_other(show);
            }
            if !preview.highlighted.is_empty() {
                ui.label(
                    RichText::new(format!("{} matching", preview.highlighted.len()))
                        .color(match_color(ui)),
                );
            }
        }
    });
}

/// How many cells of `cell` points fit in `width`, with `gap` between
/// them; at least one.
fn columns(width: f32, cell: f32, gap: f32) -> usize {
    let fit = ((width + gap) / (cell + gap)).floor();
    if fit.is_finite() && fit >= 1.0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a finite, positive column count of a window's width"
        )]
        let n = fit as usize;
        n
    } else {
        1
    }
}

fn bundle_grid(app: &mut Ae3App, ui: &mut egui::Ui) {
    toolbar(app, ui);
    ui.separator();
    let count = app
        .model
        .preview
        .as_ref()
        .map_or(0, crate::model::preview::Preview::cell_count);
    if count == 0 {
        ui.label(RichText::new("No Assets to show.").weak());
        return;
    }
    let per_row = columns(ui.available_width(), CELL_W, ui.spacing().item_spacing.x);
    let rows = count.div_ceil(per_row);
    let mut open: Option<AssetInfo> = None;
    egui::ScrollArea::vertical()
        .id_salt("preview-grid")
        .auto_shrink(false)
        .show_rows(ui, CELL_H, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for col in 0..per_row {
                        let i = row * per_row + col;
                        let Some(asset) =
                            app.model.preview.as_ref().and_then(|p| p.cell(i)).cloned()
                        else {
                            break;
                        };
                        if cell(app, ui, &asset) {
                            open = Some(asset);
                        }
                    }
                });
            }
        });
    if let Some(asset) = open {
        app.open_detail(&asset);
    }
}

/// Draws one cell; returns true when it was double-clicked.
#[expect(
    clippy::too_many_lines,
    reason = "lays out one widget in drawing order"
)]
fn cell(app: &mut Ae3App, ui: &mut egui::Ui, asset: &AssetInfo) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(CELL_W, CELL_H), Sense::click());
    let Some(preview) = &app.model.preview else {
        return false;
    };
    let package = preview.package.clone();
    let highlighted = preview.highlighted.contains(&asset.path_id);
    let mut checked = preview.checked_assets.contains(&asset.path_id);
    let visuals = ui.visuals().clone();
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let fill = if response.hovered() {
            visuals.widgets.hovered.weak_bg_fill
        } else {
            visuals.faint_bg_color
        };
        painter.rect_filled(rect, CornerRadius::same(4), fill);
        if highlighted {
            painter.rect_stroke(
                rect.shrink(1.0),
                CornerRadius::same(4),
                Stroke::new(2.0, match_color(ui)),
                StrokeKind::Inside,
            );
        } else if checked {
            painter.rect_stroke(
                rect.shrink(1.0),
                CornerRadius::same(4),
                visuals.selection.stroke,
                StrokeKind::Inside,
            );
        }
        let thumb_rect = Rect::from_min_size(
            rect.min + vec2((CELL_W - THUMB) / 2.0, 6.0),
            vec2(THUMB, THUMB),
        );
        let mut drew = false;
        if has_image(asset.kind) {
            if let Some(texture) = app.thumbnail(&package, asset.path_id) {
                let size = texture.size_vec2();
                let scale = (THUMB / size.x).min(THUMB / size.y).min(1.0);
                let image_rect = Rect::from_center_size(thumb_rect.center(), size * scale);
                let tile = app.checker(ui.ctx());
                let uv = Rect::from_min_max(
                    egui::Pos2::ZERO,
                    egui::pos2(image_rect.width() / 8.0, image_rect.height() / 8.0),
                );
                painter.image(tile.id(), image_rect, uv, Color32::WHITE);
                painter.image(
                    texture.id(),
                    image_rect,
                    Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                drew = true;
            } else if !app.thumb_failed.contains_key(&asset.path_id) {
                app.want_thumbnail(asset.path_id);
                painter.text(
                    thumb_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "…",
                    egui::FontId::proportional(24.0),
                    visuals.weak_text_color(),
                );
                drew = true;
            }
        }
        if !drew {
            painter.text(
                thumb_rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph(asset.kind),
                egui::FontId::proportional(40.0),
                visuals.weak_text_color(),
            );
        }
        let name = asset.display_name();
        let text_rect = Rect::from_min_size(
            egui::pos2(rect.min.x + 4.0, thumb_rect.max.y + 4.0),
            vec2(CELL_W - 8.0, 32.0),
        );
        let line = |text: String, size: f32, color: Color32| {
            let mut job = egui::text::LayoutJob::simple_singleline(
                text,
                egui::FontId::proportional(size),
                color,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(text_rect.width());
            painter.layout_job(job)
        };
        painter.galley(
            text_rect.min,
            line(name, 12.0, visuals.text_color()),
            visuals.text_color(),
        );
        painter.galley(
            egui::pos2(text_rect.min.x, text_rect.min.y + 16.0),
            line(summary(asset), 10.5, visuals.weak_text_color()),
            visuals.weak_text_color(),
        );
    }
    // The checkbox sits on top of the cell and takes its own clicks.
    let check_rect = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(18.0, 18.0));
    if ui
        .put(check_rect, egui::Checkbox::without_text(&mut checked))
        .changed()
        && let Some(preview) = &mut app.model.preview
    {
        preview.toggle_asset(asset.path_id);
    }
    let response = response.on_hover_ui(|ui| {
        ui.label(RichText::new(asset.display_name()).strong());
        ui.label(format!(
            "{} · {} · {}",
            kind_label(asset.kind),
            asset.script_class.as_deref().unwrap_or(&asset.class_name),
            format_bytes(asset.byte_size)
        ));
        ui.label(summary(asset));
        if let Some(path) = &asset.container_path {
            ui.label(RichText::new(path).monospace().small());
        }
        if let Some(e) = app.thumb_failed.get(&asset.path_id) {
            error_label(ui, format!("No preview: {e}"));
        }
        ui.label(RichText::new("Double-click to open").weak().small());
    });
    response.double_clicked()
}

fn sound_list(app: &mut Ae3App, ui: &mut egui::Ui) {
    toolbar(app, ui);
    ui.separator();
    let Some(preview) = &mut app.model.preview else {
        return;
    };
    let PreviewStatus::Audio { sounds } = &preview.status else {
        return;
    };
    if sounds.is_empty() {
        ui.label(RichText::new("This Audio Package holds no sounds.").weak());
        return;
    }
    let sounds = sounds.clone();
    let mut toggled: Option<u64> = None;
    ui.label(
        RichText::new("Playback is not available yet; extract sounds to listen.")
            .weak()
            .small(),
    );
    TableBuilder::new(ui)
        .id_salt("sounds")
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(22.0))
        .column(Column::remainder().at_least(100.0).clip(true))
        .column(Column::initial(60.0))
        .column(Column::initial(84.0))
        .column(Column::initial(64.0))
        .column(Column::initial(70.0))
        .auto_shrink(false)
        .header(18.0, |mut h| {
            h.col(|_| {});
            h.col(|ui| {
                ui.strong("Sound");
            });
            h.col(|ui| {
                ui.strong("Language");
            });
            h.col(|ui| {
                ui.strong("Codec");
            });
            h.col(|ui| {
                ui.strong("Duration");
            });
            h.col(|ui| {
                ui.strong("Size");
            });
        })
        .body(|body| {
            body.rows(20.0, sounds.len(), |mut row| {
                let s = &sounds[row.index()];
                let mut checked = preview.checked_sounds.contains(&s.id);
                row.col(|ui| {
                    if ui.checkbox(&mut checked, "").changed() {
                        toggled = Some(s.id);
                    }
                });
                row.col(|ui| {
                    ui.label(&s.stem)
                        .on_hover_text(format!("{} · {}", s.source, s.id));
                });
                row.col(|ui| {
                    ui.label(&s.language);
                });
                row.col(|ui| {
                    let mut codec = s.codec.unwrap_or("unknown").to_owned();
                    if let (Some(ch), Some(rate)) = (s.channels, s.sample_rate) {
                        let _ = write!(codec, " {ch}ch {}k", rate / 1000);
                    }
                    ui.label(codec);
                });
                row.col(|ui| {
                    ui.label(
                        s.duration_secs
                            .map_or_else(|| "—".to_owned(), |d| format!("{d:.2} s")),
                    );
                });
                row.col(|ui| {
                    ui.label(format_bytes(s.size));
                });
            });
        });
    if let Some(id) = toggled {
        preview.toggle_sound(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fit_the_width() {
        assert_eq!(columns(500.0, 124.0, 8.0), 3);
        assert_eq!(columns(10.0, 124.0, 8.0), 1);
        assert_eq!(columns(f32::NAN, 124.0, 8.0), 1);
        assert_eq!(columns(-50.0, 124.0, 8.0), 1);
    }
}
