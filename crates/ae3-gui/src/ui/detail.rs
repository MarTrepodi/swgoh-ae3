//! The in-app detail viewer: textures with zoom, pan and channel toggles;
//! Atlases with their Sprites outlined; Meshes in an orbiting 3D view;
//! Structured Assets as a tree; text with highlighting; AudioClips with a
//! play control.

use std::fmt::Write as _;
use std::sync::Arc;

use ae3_core::{SpriteRect, Value};
use eframe::egui::{
    self, Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, TextFormat, Vec2, vec2,
};

use super::{error_label, icons, kind_label, theme, widgets};
use crate::app::{Ae3App, Detail};
use crate::highlight::{Language, TokenKind, tokenize};
use crate::model::playback::SoundRef;
use crate::model::table::format_bytes;
use crate::model::viewer::{ViewTransform, sprite_at};
use crate::tasks::{DetailContent, DetailImage};

/// Array elements shown per collapsible group in the tree.
const TREE_CHUNK: usize = 100;

pub fn window(app: &mut Ae3App, ctx: &egui::Context) {
    let Some(detail) = &app.detail else { return };
    let title = format!("{} — {}", detail.asset.display_name(), detail.package);
    let mut open = detail.open;
    let texture = app.detail_texture();
    let checker = app.checker(ctx);
    let renderer = Arc::clone(&app.mesh_renderer);
    let mut remask = false;
    let mut play: Option<SoundRef> = None;
    egui::Window::new(title)
        .id(egui::Id::unique("ae3-detail"))
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .default_size([780.0, 600.0])
        .min_size([320.0, 240.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            let Some(detail) = &mut app.detail else {
                return;
            };
            header(ui, detail);
            ui.separator();
            match &detail.content {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Decoding…");
                    });
                }
                Some(Err(e)) => error_label(ui, e.clone()),
                Some(Ok(DetailContent::Image(image))) => {
                    let size = vec2(image.width as f32, image.height as f32);
                    let info = image_info(image);
                    remask |= image_toolbar(ui, detail, &info);
                    if let Some(texture) = &texture {
                        canvas(ui, detail, texture, &checker, size, None);
                    }
                }
                Some(Ok(DetailContent::Atlas { .. })) => {
                    remask |= atlas_view(ui, detail, texture.as_ref(), &checker);
                }
                Some(Ok(DetailContent::Tree(value))) => {
                    egui::ScrollArea::both()
                        .id_salt("detail-tree")
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            node(ui, "root", value, egui::Id::unique("ae3-detail-tree"), true);
                        });
                }
                Some(Ok(DetailContent::Text {
                    text,
                    extension,
                    truncated,
                })) => text_view(ui, text, extension, *truncated),
                Some(Ok(DetailContent::Mesh(preview))) => {
                    super::mesh::view(
                        ui,
                        &mut detail.camera,
                        preview,
                        detail.generation,
                        &renderer,
                    );
                }
                Some(Ok(DetailContent::Info(text))) => {
                    ui.label(text);
                }
                Some(Ok(DetailContent::Audio { text, .. })) => {
                    ui.label(text);
                    let sound = SoundRef::Clip(detail.asset.path_id);
                    ui.horizontal(|ui| {
                        if super::preview::play_control(ui, app.playback.state(sound)) {
                            play = Some(sound);
                        }
                    });
                }
            }
        });
    if let Some(sound) = play {
        app.toggle_sound(sound);
    }
    if let Some(detail) = &mut app.detail {
        detail.open = open;
    }
    if remask {
        app.remask_detail();
    }
}

fn header(ui: &mut egui::Ui, detail: &Detail) {
    let a = &detail.asset;
    let p = theme::palette(ui);
    ui.horizontal_wrapped(|ui| {
        widgets::with_icon(
            ui,
            RichText::new(icons::kind(a.kind)).color(p.accent_text),
            |ui| ui.label(RichText::new(kind_label(a.kind)).family(theme::medium())),
        );
        widgets::muted(
            ui,
            format!(
                "{} · path id {} · {}",
                a.script_class.as_deref().unwrap_or(&a.class_name),
                a.path_id,
                format_bytes(a.byte_size)
            ),
        );
    });
    if let Some(path) = &a.container_path {
        ui.label(
            RichText::new(path)
                .monospace()
                .size(theme::size::SMALL)
                .color(p.text_muted),
        );
    }
}

fn image_info(image: &DetailImage) -> String {
    let shown = &image.shown;
    if shown.width() == image.width && shown.height() == image.height {
        format!("{}×{}", image.width, image.height)
    } else {
        format!(
            "{}×{} (shown at {}×{}, the GPU's limit)",
            image.width,
            image.height,
            shown.width(),
            shown.height()
        )
    }
}

/// Zoom buttons and channel toggles; returns true when the channels
/// changed.
fn image_toolbar(ui: &mut egui::Ui, detail: &mut Detail, info: &str) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        let format = detail.asset.texture.map(|t| {
            t.format.map_or_else(
                || format!("format {}", t.format_id),
                |f| f.name().to_owned(),
            )
        });
        let mut text = info.to_owned();
        if let Some(format) = format {
            let _ = write!(text, " · {format}");
        }
        ui.label(RichText::new(text).family(theme::medium()));
        ui.separator();
        if widgets::button(ui, Some(icons::FIT), "Fit", true)
            .on_hover_text("Fit the image to the view")
            .clicked()
        {
            detail.fitted = false;
        }
        if widgets::button(ui, None, "1:1", true)
            .on_hover_text("One image pixel per screen pixel")
            .clicked()
        {
            detail.transform = ViewTransform::default();
            detail.fitted = true;
        }
        widgets::muted(ui, format!("{:.0}%", detail.transform.zoom * 100.0));
        ui.separator();
        let ch = &mut detail.channels;
        for (on, name, color) in [
            (&mut ch.r, "R", Color32::from_rgb(230, 80, 80)),
            (&mut ch.g, "G", Color32::from_rgb(80, 200, 80)),
            (&mut ch.b, "B", Color32::from_rgb(90, 140, 240)),
            (&mut ch.a, "A", ui.visuals().text_color()),
        ] {
            if ui
                .toggle_value(on, RichText::new(name).strong().color(color))
                .on_hover_text(format!("Show the {name} channel"))
                .changed()
            {
                changed = true;
            }
        }
        if !detail.channels.is_all() && ui.small_button("All").clicked() {
            detail.channels = crate::model::viewer::Channels::ALL;
            changed = true;
        }
    });
    changed && detail.channels != detail.shown_channels
}

/// The zoomable, pannable image. `size` is the texture's true pixel size;
/// Sprite rectangles are in that space.
fn canvas(
    ui: &mut egui::Ui,
    detail: &mut Detail,
    texture: &egui::TextureHandle,
    checker: &egui::TextureHandle,
    size: Vec2,
    sprites: Option<&[SpriteRect]>,
) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if !detail.fitted {
        detail.transform = ViewTransform::fit(size, rect.size());
        detail.fitted = true;
    }
    if response.dragged() {
        detail.transform.pan += response.drag_delta();
    }
    if let Some(pos) = response.hover_pos() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        let factor = pinch * (scroll / 200.0).exp();
        if (factor - 1.0).abs() > 1e-4 {
            detail.transform.zoom_at(factor, pos - rect.center());
        }
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::ZERO, ui.visuals().extreme_bg_color);
    let image_rect = detail.transform.image_rect(size, rect);
    let tiles = image_rect.size() / 12.0;
    painter.image(
        checker.id(),
        image_rect,
        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(tiles.x, tiles.y)),
        Color32::WHITE,
    );
    painter.image(
        texture.id(),
        image_rect,
        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    if let Some(sprites) = sprites {
        let outline = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 220, 0, 160));
        let selected = Stroke::new(2.0, ui.visuals().selection.stroke.color);
        for (i, sprite) in sprites.iter().enumerate() {
            let r = detail.transform.sprite_rect(size, rect, sprite);
            if !rect.intersects(r) {
                continue;
            }
            if detail.selected_sprite == Some(i) {
                painter.rect_filled(
                    r,
                    CornerRadius::ZERO,
                    ui.visuals().selection.bg_fill.gamma_multiply(0.35),
                );
                painter.rect_stroke(r, CornerRadius::ZERO, selected, StrokeKind::Outside);
            } else {
                painter.rect_stroke(r, CornerRadius::ZERO, outline, StrokeKind::Inside);
            }
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            detail.selected_sprite = detail
                .transform
                .pixel_at(size, rect, pos)
                .and_then(|p| sprite_at(sprites, p));
        }
    }
    if let Some(pos) = response.hover_pos()
        && let Some(p) = detail.transform.pixel_at(size, rect, pos)
    {
        let mut text = format!("{}, {}", p.x.floor(), p.y.floor());
        if let Some(i) = sprites.and_then(|s| sprite_at(s, p)) {
            let _ = write!(text, "  {}", sprites.map_or("", |s| s[i].name.as_str()));
        }
        painter.text(
            rect.left_bottom() + vec2(6.0, -6.0),
            egui::Align2::LEFT_BOTTOM,
            text,
            egui::FontId::monospace(super::theme::size::SMALL),
            ui.visuals().strong_text_color(),
        );
    }
}

/// The Atlas view: Sprite list on the left, the texture with Sprite
/// outlines on the right. Returns true when the channels changed.
fn atlas_view(
    ui: &mut egui::Ui,
    detail: &mut Detail,
    texture: Option<&egui::TextureHandle>,
    checker: &egui::TextureHandle,
) -> bool {
    // Moved out while drawing, so the view state can change alongside.
    let content = detail.content.take();
    let changed = match &content {
        Some(Ok(DetailContent::Atlas {
            atlas,
            image,
            error,
        })) => atlas_body(
            ui,
            detail,
            atlas,
            image.as_ref(),
            error.as_deref(),
            texture,
            checker,
        ),
        _ => false,
    };
    detail.content = content;
    changed
}

fn atlas_body(
    ui: &mut egui::Ui,
    detail: &mut Detail,
    atlas: &ae3_core::Atlas,
    image: Option<&DetailImage>,
    error: Option<&str>,
    texture: Option<&egui::TextureHandle>,
    checker: &egui::TextureHandle,
) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{} Sprites", atlas.sprites.len())).family(theme::medium()));
        if let Some(t) = &atlas.texture {
            widgets::muted(ui, format!("texture {} in {}", t.name, t.asset.package));
        }
        if atlas.replacement.is_some() {
            widgets::muted(ui, "(reference Atlas, showing its target)");
        }
        if (atlas.pixel_size - 1.0).abs() > f64::EPSILON {
            widgets::muted(ui, format!("pixel size {}", atlas.pixel_size));
        }
    });
    if let Some(e) = error {
        error_label(ui, format!("Atlas texture: {e}"));
    }
    let mut changed = false;
    if let Some(image) = image {
        changed = image_toolbar(ui, detail, &image_info(image));
    }
    egui::Panel::left("atlas-sprites")
        .resizable(true)
        .default_size(200.0)
        .show(ui, |ui| {
            if let Some(i) = detail.selected_sprite
                && let Some(s) = atlas.sprites.get(i)
            {
                sprite_info(ui, s);
                ui.separator();
            }
            egui::ScrollArea::vertical()
                .id_salt("atlas-sprite-list")
                .auto_shrink(false)
                .show_rows(ui, 18.0, atlas.sprites.len(), |ui, range| {
                    for i in range {
                        let s = &atlas.sprites[i];
                        let on = detail.selected_sprite == Some(i);
                        if ui
                            .selectable_label(on, format!("{} ({}×{})", s.name, s.width, s.height))
                            .clicked()
                        {
                            detail.selected_sprite = if on { None } else { Some(i) };
                        }
                    }
                });
        });
    if let (Some(texture), Some(image)) = (texture, image) {
        let size = vec2(image.width as f32, image.height as f32);
        canvas(ui, detail, texture, checker, size, Some(&atlas.sprites));
    }
    changed
}

fn sprite_info(ui: &mut egui::Ui, s: &SpriteRect) {
    ui.label(RichText::new(&s.name).family(theme::medium()));
    ui.label(format!("x {} y {} · {}×{}", s.x, s.y, s.width, s.height));
    let (w, h) = s.restored_size();
    if (w, h) != (s.width, s.height) {
        ui.label(format!(
            "padding l{} r{} t{} b{} → {w}×{h}",
            s.padding_left, s.padding_right, s.padding_top, s.padding_bottom
        ));
    }
    if s.border_left + s.border_right + s.border_top + s.border_bottom > 0 {
        ui.label(format!(
            "border l{} r{} t{} b{}",
            s.border_left, s.border_right, s.border_top, s.border_bottom
        ));
    }
}

fn leaf_text(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(d) => d.to_string(),
        Value::String(s) => format!("{s:?}"),
        Value::Bytes(b) => {
            let hex: Vec<String> = b.iter().take(32).map(|x| format!("{x:02x}")).collect();
            let more = if b.len() > 32 { " …" } else { "" };
            format!("<{} bytes> {}{more}", b.len(), hex.join(" "))
        }
        Value::Array(a) if a.is_empty() => "[]".to_owned(),
        Value::Map(m) if m.is_empty() => "{}".to_owned(),
        _ => return None,
    })
}

/// One field of the tree; containers are collapsible and only lay out
/// their children while open.
fn node(ui: &mut egui::Ui, key: &str, value: &Value, id: egui::Id, open: bool) {
    if let Some(text) = leaf_text(value) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(key).strong());
            ui.label(RichText::new(text).monospace());
        });
        return;
    }
    match value {
        Value::Map(map) => {
            egui::CollapsingHeader::new(format!("{key}  {{{}}}", map.len()))
                .id_salt(id)
                .default_open(open)
                .show(ui, |ui| {
                    for (k, v) in map.iter() {
                        node(ui, k, v, id.with(k), false);
                    }
                });
        }
        Value::Array(items) => {
            egui::CollapsingHeader::new(format!("{key}  [{}]", items.len()))
                .id_salt(id)
                .default_open(open)
                .show(ui, |ui| {
                    if items.len() <= TREE_CHUNK {
                        for (i, v) in items.iter().enumerate() {
                            node(ui, &format!("[{i}]"), v, id.with(i), false);
                        }
                    } else {
                        for (c, chunk) in items.chunks(TREE_CHUNK).enumerate() {
                            let start = c * TREE_CHUNK;
                            let end = start + chunk.len();
                            egui::CollapsingHeader::new(format!("[{start}..{end})"))
                                .id_salt(id.with(("chunk", c)))
                                .show(ui, |ui| {
                                    for (i, v) in chunk.iter().enumerate() {
                                        node(
                                            ui,
                                            &format!("[{}]", start + i),
                                            v,
                                            id.with(start + i),
                                            false,
                                        );
                                    }
                                });
                        }
                    }
                });
        }
        _ => {}
    }
}

fn token_color(kind: TokenKind, dark: bool, plain: Color32) -> Color32 {
    let pick = |d: (u8, u8, u8), l: (u8, u8, u8)| {
        let (r, g, b) = if dark { d } else { l };
        Color32::from_rgb(r, g, b)
    };
    match kind {
        TokenKind::Plain => plain,
        TokenKind::Key | TokenKind::Attribute => pick((156, 220, 254), (0, 84, 160)),
        TokenKind::String => pick((206, 145, 120), (163, 21, 21)),
        TokenKind::Number => pick((181, 206, 168), (9, 134, 88)),
        TokenKind::Keyword => pick((86, 156, 214), (0, 0, 255)),
        TokenKind::Punct => pick((150, 150, 150), (110, 110, 110)),
        TokenKind::Comment => pick((106, 153, 85), (0, 128, 0)),
        TokenKind::Tag => pick((78, 201, 176), (128, 0, 0)),
    }
}

fn text_view(ui: &mut egui::Ui, text: &str, extension: &str, truncated: bool) {
    let language = Language::from_extension(extension);
    ui.horizontal(|ui| {
        widgets::muted(
            ui,
            format!("{extension} · {} characters", text.chars().count()),
        );
        if truncated {
            super::warn_label(ui, "(truncated; extract the Asset for the whole text)");
        }
    });
    let dark = ui.visuals().dark_mode;
    let plain = ui.visuals().text_color();
    let font = egui::FontId::monospace(super::theme::size::MONO);
    let mut job = egui::text::LayoutJob::default();
    for (range, kind) in tokenize(text, language) {
        job.append(
            &text[range],
            0.0,
            TextFormat::simple(font.clone(), token_color(kind, dark, plain)),
        );
    }
    egui::ScrollArea::both()
        .id_salt("detail-text")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.add(egui::Label::new(job).selectable(true).extend());
        });
}
