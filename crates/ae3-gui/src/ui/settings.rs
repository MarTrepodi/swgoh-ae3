//! The Settings window: every output option, grouped (folder, images,
//! audio, data and meshes, files). Changes apply at once and are saved a
//! second later. The sidebar's Output card shows [`summary`].

use ae3_core::Config;
use ae3_core::config::{AudioFormat, ImageFormat, Layout, MeshFormat, StructuredMode};
use eframe::egui::{self, RichText};

use super::widgets;
use super::{icons, theme};
use crate::app::{Ae3App, SettingChange};

/// The Output card's lines: the formats and how files are laid out.
#[must_use]
pub fn summary(config: &Config) -> Vec<String> {
    let mut atlas = if config.atlas_cutting {
        "Atlas Cutting".to_owned()
    } else {
        "Atlases whole".to_owned()
    };
    if config.atlas_cutting && config.padding_restoration {
        atlas.push_str(", padding restored");
    }
    let mut files = format!("Layout {}", layout_name(config.layout));
    if config.skip_existing {
        files.push_str(" · skip existing");
    }
    vec![
        format!(
            "{} · {} · {}",
            image_label(config.image_format),
            audio_label(config.audio_format),
            mesh_label(config.mesh_format)
        ),
        format!("Data: {}", structured_label(config.structured)),
        atlas,
        files,
    ]
}

pub fn window(app: &mut Ae3App, ctx: &egui::Context) {
    if !app.model.windows.settings {
        return;
    }
    let mut open = true;
    egui::Window::new("Settings")
        .open(&mut open)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            folder(app, ui);
            ui.add_space(4.0);
            section(ui, "Images");
            images(app, ui);
            ui.add_space(4.0);
            section(ui, "Audio, data and meshes");
            formats(app, ui);
            ui.add_space(4.0);
            section(ui, "Files");
            files(app, ui);
        });
    app.model.windows.settings = open;
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).text_style(theme::section_style()));
}

/// A form row: `label` in a fixed-width column, then the control.
fn row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let width = 132.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(width, ui.spacing().interact_size.y),
            egui::Sense::hover(),
        );
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(theme::size::BODY),
            theme::palette(ui).text_muted,
        );
        add(ui)
    })
    .inner
}

fn folder(app: &mut Ae3App, ui: &mut egui::Ui) {
    section(ui, "Output folder");
    ui.horizontal(|ui| {
        let dir = app.config.output_dir.display().to_string();
        let choose = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let choose = widgets::button(ui, Some(icons::FOLDER), "Choose…", true)
                .on_hover_text("Pick the folder extractions are written to");
            let p = theme::palette(ui);
            egui::Frame::new()
                .fill(p.inset)
                .stroke(egui::Stroke::new(1.0, p.border))
                .corner_radius(theme::CONTROL_RADIUS)
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&dir).monospace())
                                .truncate()
                                .show_tooltip_when_elided(true),
                        )
                        .on_hover_text(&dir);
                    });
                });
            choose
        });
        if choose.inner.clicked()
            && let Some(dir) = rfd::FileDialog::new()
                .set_directory(&app.config.output_dir)
                .pick_folder()
        {
            app.set_output_dir(dir);
        }
    });
}

fn images(app: &mut Ae3App, ui: &mut egui::Ui) {
    let mut image = app.config.image_format;
    row(ui, "Format", |ui| {
        combo(
            ui,
            "Image format",
            &mut image,
            ImageFormat::ALL,
            image_label,
            None,
        )
        .on_hover_text("Image format for textures and Sprites");
    });
    if image != app.config.image_format {
        app.change_setting(SettingChange::ImageFormat(image));
    }
    let mut cutting = app.config.atlas_cutting;
    row(ui, "Atlases", |ui| {
        if ui
            .checkbox(&mut cutting, "Split sprite atlases")
            .on_hover_text("Atlas Cutting: write each Sprite of an Atlas as its own image")
            .changed()
        {
            app.change_setting(SettingChange::AtlasCutting(cutting));
        }
    });
    let mut padding = app.config.padding_restoration;
    row(ui, "", |ui| {
        if ui
            .add_enabled(
                cutting,
                egui::Checkbox::new(&mut padding, "Restore padding"),
            )
            .on_hover_text("Give cut Sprites back the transparent margin NGUI trimmed")
            .changed()
        {
            app.change_setting(SettingChange::PaddingRestoration(padding));
        }
    });
}

fn formats(app: &mut Ae3App, ui: &mut egui::Ui) {
    let mut audio = app.config.audio_format;
    row(ui, "Audio format", |ui| {
        combo(
            ui,
            "Audio format",
            &mut audio,
            AudioFormat::ALL,
            audio_label,
            None,
        )
        .on_hover_text("Audio format for sounds from Audio Packages");
    });
    if audio != app.config.audio_format {
        app.change_setting(SettingChange::AudioFormat(audio));
    }
    let mut structured = app.config.structured;
    row(ui, "Structured data", |ui| {
        combo(
            ui,
            "Structured data",
            &mut structured,
            StructuredMode::ALL,
            structured_label,
            Some(structured_tip),
        )
        .on_hover_text(
            "How whole-Bundle extractions write Structured Assets (MonoBehaviours and \
             other data). Assets checked in the preview are always written as their own files.",
        );
    });
    if structured != app.config.structured {
        app.change_setting(SettingChange::Structured(structured));
    }
    let mut mesh = app.config.mesh_format;
    row(ui, "Mesh format", |ui| {
        combo(
            ui,
            "Mesh format",
            &mut mesh,
            MeshFormat::ALL,
            mesh_label,
            None,
        )
        .on_hover_text(
            "Wavefront OBJ (geometry only), binary glTF with the main textures embedded, \
             or both",
        );
    });
    if mesh != app.config.mesh_format {
        app.change_setting(SettingChange::MeshFormat(mesh));
    }
}

fn files(app: &mut Ae3App, ui: &mut egui::Ui) {
    let mut layout = app.config.layout;
    row(ui, "Layout", |ui| {
        combo(ui, "Layout", &mut layout, Layout::ALL, layout_label, None)
            .on_hover_text("Where each Package's files go under the output folder");
    });
    if layout != app.config.layout {
        app.change_setting(SettingChange::Layout(layout));
    }
    let mut skip = app.config.skip_existing;
    row(ui, "Existing files", |ui| {
        if ui
            .checkbox(&mut skip, "Skip existing")
            .on_hover_text("Leave files that already exist alone")
            .changed()
        {
            app.change_setting(SettingChange::SkipExisting(skip));
        }
    });
}

/// A drop-down named `name` over `options`.
fn combo<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut T,
    options: &[T],
    label: fn(T) -> &'static str,
    tip: Option<fn(T) -> &'static str>,
) -> egui::Response {
    egui::ComboBox::from_id_salt(name)
        .selected_text(label(*value))
        .width(ui.available_width().min(260.0))
        .show_ui(ui, |ui| {
            for &option in options {
                let item = ui.selectable_value(value, option, label(option));
                if let Some(tip) = tip {
                    item.on_hover_text(tip(option));
                }
            }
        })
        .response
        .accessible_name(name)
}

fn image_label(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Png => "PNG",
        ImageFormat::Webp => "WebP (lossless)",
    }
}

fn audio_label(f: AudioFormat) -> &'static str {
    match f {
        AudioFormat::Ogg => "Ogg Vorbis",
        AudioFormat::Raw => "Raw (Wwise)",
    }
}

fn mesh_label(f: MeshFormat) -> &'static str {
    match f {
        MeshFormat::Obj => "OBJ",
        MeshFormat::Glb => "glb",
        MeshFormat::Both => "OBJ + glb",
    }
}

fn structured_label(m: StructuredMode) -> &'static str {
    match m {
        StructuredMode::Combined => "One JSON per Bundle",
        StructuredMode::Separate => "One JSON per Asset",
        StructuredMode::None => "Skip",
    }
}

fn structured_tip(m: StructuredMode) -> &'static str {
    match m {
        StructuredMode::Combined => {
            "{bundle}.structured.json holds every Structured Asset of the Bundle, keyed by name"
        }
        StructuredMode::Separate => {
            "One {asset}.json per Structured Asset, as earlier releases wrote"
        }
        StructuredMode::None => "Do not write Structured Assets",
    }
}

fn layout_name(l: Layout) -> &'static str {
    match l {
        Layout::Default => "default",
        Layout::Ae2 => "ae2",
    }
}

fn layout_label(l: Layout) -> &'static str {
    match l {
        Layout::Default => "Default: {category}/{bundle}/",
        Layout::Ae2 => "ae2: {category}/",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_names_every_choice() {
        let config = Config::default();
        let lines = summary(&config);
        assert_eq!(lines.len(), 4);
        assert!(
            lines[0].contains(image_label(config.image_format)),
            "{lines:?}"
        );
        assert!(lines[0].contains(audio_label(config.audio_format)));
        assert!(lines[0].contains(mesh_label(config.mesh_format)));
        let changed = Config {
            atlas_cutting: false,
            skip_existing: true,
            layout: Layout::Ae2,
            ..Config::default()
        };
        let lines = summary(&changed);
        assert_eq!(lines[2], "Atlases whole");
        assert_eq!(lines[3], "Layout ae2 · skip existing");
    }
}
