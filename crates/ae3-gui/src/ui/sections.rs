//! The three sections at the top: Manifest, Output and Asset Search.

use std::fmt::Write as _;

use std::time::Instant;

use ae3_core::Platform;
use ae3_core::config::{AudioFormat, ImageFormat, Layout, MeshFormat, StructuredMode};
use eframe::egui::{self, RichText};

use super::{error_label, icons, small_icon_button, warn_label};
use crate::app::{Ae3App, SettingChange};
use crate::model::jobs::JobPhase;
use crate::model::names::{EntityMatch, NamesState};

const SECTION_WIDTH: f32 = 96.0;

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_sized(
        [SECTION_WIDTH, 18.0],
        egui::Label::new(RichText::new(text).strong()),
    );
}

fn platform_name(p: Platform) -> &'static str {
    p.name()
}

/// Platform, Asset Version, Fetch Latest / Load Manifest and the
/// newer-version notice.
pub fn manifest(app: &mut Ae3App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        heading(ui, "Manifest");
        let busy = app.model.manifest_job.is_active();
        let mut platform = app.config.platform;
        ui.add_enabled_ui(!busy, |ui| {
            egui::ComboBox::from_id_salt("platform")
                .selected_text(platform_name(platform))
                .show_ui(ui, |ui| {
                    for p in Platform::ALL {
                        ui.selectable_value(&mut platform, p, platform_name(p));
                    }
                })
                .response
                .accessible_name("Platform");
        });
        if platform != app.config.platform {
            app.change_setting(SettingChange::Platform(platform));
        }
        ui.label("Asset Version");
        let edit = ui
            .add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut app.model.version_text)
                    .desired_width(72.0)
                    .hint_text("latest"),
            )
            .accessible_name("Asset Version");
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui
            .add_enabled(!busy, egui::Button::new("Fetch Latest"))
            .on_hover_text("Find the newest Asset Version and load its Manifest")
            .clicked()
        {
            app.load_manifest(None);
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Load Manifest"))
            .on_hover_text("Load the Manifest of the Asset Version entered")
            .clicked()
            || (enter && !busy)
        {
            match crate::model::diff::parse_version(&app.model.version_text) {
                Ok(version) => app.load_manifest(version),
                Err(e) => app.last_load_error = Some(e),
            }
        }
        if busy {
            ui.spinner();
            ui.label(app.model.manifest_job.progress().clone());
        } else if let Some(e) = &app.last_load_error {
            error_label(ui, e.clone());
        } else if let (Some(manifest), Some(store)) = (&app.model.manifest, &app.store) {
            let bundles = manifest
                .packages
                .iter()
                .filter(|p| p.kind == ae3_core::PackageKind::Bundle)
                .count();
            let audio = manifest.packages.len() - bundles;
            ui.label(format!(
                "Asset Version {} · {} · {} · {bundles} Bundles, {audio} Audio Packages",
                manifest.asset_version,
                store.platform(),
                store.location().environment,
            ));
            if app.model.startup.is_pending() {
                // No spinner: it would repaint the window until GitHub answers.
                ui.label(RichText::new("· looking for a newer Asset Version").weak());
            }
        }
        if let Some(newer) = app.model.update.newer() {
            ui.separator();
            warn_label(ui, format!("Asset Version {newer} available"));
            if ui.add_enabled(!busy, egui::Button::new("Load")).clicked() {
                app.model.version_text = newer.to_string();
                app.load_manifest(Some(newer));
            }
            if small_icon_button(ui, icons::CLOSE, "Dismiss Asset Version notice").clicked() {
                app.model.update.dismiss();
            }
        }
    });
}

/// Output folder, image and audio formats, Atlas Cutting, padding and
/// layout.
pub fn output(app: &mut Ae3App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        heading(ui, "Output");
        let dir = app.config.output_dir.display().to_string();
        ui.add(
            egui::Label::new(RichText::new(&dir).monospace())
                .truncate()
                .show_tooltip_when_elided(true),
        )
        .on_hover_text(&dir);
        if ui.button("Choose…").clicked()
            && let Some(dir) = rfd::FileDialog::new()
                .set_directory(&app.config.output_dir)
                .pick_folder()
        {
            app.set_output_dir(dir);
        }
        ui.separator();

        let mut image = app.config.image_format;
        egui::ComboBox::from_id_salt("image-format")
            .selected_text(image_label(image))
            .show_ui(ui, |ui| {
                for &f in ImageFormat::ALL {
                    ui.selectable_value(&mut image, f, image_label(f));
                }
            })
            .response
            .on_hover_text("Image format for textures and Sprites");
        if image != app.config.image_format {
            app.change_setting(SettingChange::ImageFormat(image));
        }

        let mut cutting = app.config.atlas_cutting;
        if ui
            .checkbox(&mut cutting, "Split sprite atlases")
            .on_hover_text("Atlas Cutting: write each Sprite of an Atlas as its own image")
            .changed()
        {
            app.change_setting(SettingChange::AtlasCutting(cutting));
        }
        let mut padding = app.config.padding_restoration;
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

        let mut audio = app.config.audio_format;
        egui::ComboBox::from_id_salt("audio-format")
            .selected_text(audio_label(audio))
            .show_ui(ui, |ui| {
                for &f in AudioFormat::ALL {
                    ui.selectable_value(&mut audio, f, audio_label(f));
                }
            })
            .response
            .on_hover_text("Audio format for sounds from Audio Packages");
        if audio != app.config.audio_format {
            app.change_setting(SettingChange::AudioFormat(audio));
        }

        structured_choice(app, ui);
        mesh_choice(app, ui);

        let mut layout = app.config.layout;
        egui::ComboBox::from_id_salt("layout")
            .selected_text(layout_label(layout))
            .show_ui(ui, |ui| {
                for &l in Layout::ALL {
                    ui.selectable_value(&mut layout, l, layout_label(l));
                }
            })
            .response
            .on_hover_text("default: {category}/{bundle}/ · ae2: {category}/ like swgoh-ae2");
        if layout != app.config.layout {
            app.change_setting(SettingChange::Layout(layout));
        }

        let mut skip = app.config.skip_existing;
        if ui
            .checkbox(&mut skip, "Skip existing")
            .on_hover_text("Leave files that already exist alone")
            .changed()
        {
            app.change_setting(SettingChange::SkipExisting(skip));
        }
    });
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

/// How whole-Bundle extractions write Structured Assets.
fn structured_choice(app: &mut Ae3App, ui: &mut egui::Ui) {
    let mut structured = app.config.structured;
    egui::ComboBox::from_id_salt("structured")
        .selected_text(structured_label(structured))
        .show_ui(ui, |ui| {
            for &m in StructuredMode::ALL {
                ui.selectable_value(&mut structured, m, structured_label(m))
                    .on_hover_text(structured_tip(m));
            }
        })
        .response
        .on_hover_text(
            "How whole-Bundle extractions write Structured Assets (MonoBehaviours and \
             other data). Assets checked in the preview are always written as their own files.",
        );
    if structured != app.config.structured {
        app.change_setting(SettingChange::Structured(structured));
    }
}

/// The file format Meshes are extracted in.
fn mesh_choice(app: &mut Ae3App, ui: &mut egui::Ui) {
    let mut format = app.config.mesh_format;
    egui::ComboBox::from_id_salt("mesh-format")
        .selected_text(mesh_label(format))
        .show_ui(ui, |ui| {
            for &f in MeshFormat::ALL {
                ui.selectable_value(&mut format, f, mesh_label(f));
            }
        })
        .response
        .on_hover_text(
            "Mesh format: Wavefront OBJ (geometry only), binary glTF with the main \
             textures embedded, or both",
        );
    if format != app.config.mesh_format {
        app.change_setting(SettingChange::MeshFormat(format));
    }
}

fn mesh_label(f: MeshFormat) -> &'static str {
    match f {
        MeshFormat::Obj => "Meshes: OBJ",
        MeshFormat::Glb => "Meshes: glb",
        MeshFormat::Both => "Meshes: OBJ + glb",
    }
}

fn structured_label(m: StructuredMode) -> &'static str {
    match m {
        StructuredMode::Combined => "Data: one JSON per Bundle",
        StructuredMode::Separate => "Data: one JSON per Asset",
        StructuredMode::None => "Data: skip",
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

fn layout_label(l: Layout) -> &'static str {
    match l {
        Layout::Default => "Layout: default",
        Layout::Ae2 => "Layout: ae2",
    }
}

/// How many matching Game Entities are listed before "N more".
const ENTITIES_SHOWN: usize = 8;
/// How many are listed at most once expanded; a query matching more is
/// better refined than scrolled.
const ENTITIES_MAX: usize = 200;

/// The search box, its status line, the matching Game Entities, and the
/// Asset Index build progress.
pub fn search(app: &mut Ae3App, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        heading(ui, "Asset Search");
        let mut query = app.model.query.clone();
        let edit = ui
            .add_enabled(
                app.model.manifest.is_some(),
                egui::TextEdit::singleline(&mut query)
                    .desired_width(260.0)
                    .hint_text("Asset, Bundle or display name"),
            )
            .accessible_name("Asset Search");
        if edit.changed() {
            app.set_query(query);
        }
        if !app.model.query.is_empty()
            && small_icon_button(ui, icons::CLOSE, "Clear search").clicked()
        {
            app.set_query(String::new());
        }
        let searching = app.model.search.is_some();
        if let Some(status) = app.model.search_status() {
            ui.label(status);
        } else if let Some(c) = &app.model.coverage {
            let mut text = format!(
                "Asset Index: {} of {} Bundles indexed",
                c.indexed_bundles, c.total_bundles
            );
            if c.failed_bundles > 0 {
                let _ = write!(text, ", {} unreadable", c.failed_bundles);
            }
            ui.label(RichText::new(text).weak());
        }
        // How the display names stand: always when there is a problem or
        // they are still loading, otherwise beside the coverage.
        if let Some((line, warning)) = app.model.names_state.line() {
            let loading = matches!(app.model.names_state, NamesState::Loading { .. });
            if warning {
                ui.separator();
                warn_label(ui, line);
            } else if loading || (!searching && app.model.manifest.is_some()) {
                ui.separator();
                ui.label(RichText::new(line).weak());
            }
        }
    });
    entities(app, ui);
    index_progress(app, ui);
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

/// The Game Entities whose display names match, as buttons that narrow the
/// Bundles table to their Bundles.
fn entities(app: &mut Ae3App, ui: &mut egui::Ui) {
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
            for entity in &search.entities[..shown] {
                let on = picked_key.is_some_and(|k| entity.is(k));
                let n = entity.bundles.len();
                let name = format!("{} {} · {n}", entity.kind.label(), entity.label());
                let text = icons::with(
                    icons::entity(entity.kind),
                    &format!("{} · {n}", entity.label()),
                );
                let response = ui
                    .add_enabled(n > 0, egui::Button::selectable(on, text))
                    .accessible_name(name)
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
                ui.label(
                    RichText::new(format!("and {} more; type more to narrow", total - shown))
                        .weak(),
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
    ui.horizontal(|ui| {
        ui.add_space(SECTION_WIDTH + ui.spacing().item_spacing.x);
        if expanded {
            egui::ScrollArea::vertical()
                .id_salt("entities")
                .max_height(132.0)
                .show(ui, |ui| list(ui));
        } else {
            list(ui);
        }
    });
    if toggle_expanded {
        app.model.entities_expanded = !expanded;
    }
    if picked.is_some() {
        app.model.pick_entity(picked);
    }
}

fn index_progress(app: &mut Ae3App, ui: &mut egui::Ui) {
    let job = &app.model.index_job;
    let phase = job.phase().clone();
    let now = Instant::now();
    match phase {
        JobPhase::Running | JobPhase::Paused | JobPhase::Cancelling => {
            let fraction = job.progress().fraction();
            let label = app.model.index_label(now).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.add_space(SECTION_WIDTH + ui.spacing().item_spacing.x);
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(320.0)
                        .show_percentage(),
                );
                ui.label(label);
                let paused = app.index_pause_requested();
                let (icon, text, tip) = if paused {
                    (icons::PLAY, "Resume", "Continue building the Asset Index")
                } else {
                    (
                        icons::PAUSE,
                        "Pause",
                        "Stop fetching for now; progress is kept",
                    )
                };
                if super::icon_button(ui, icon, text)
                    .on_hover_text(tip)
                    .clicked()
                {
                    app.toggle_index_pause();
                }
            });
        }
        JobPhase::Failed(e) => {
            ui.horizontal(|ui| {
                ui.add_space(SECTION_WIDTH + ui.spacing().item_spacing.x);
                error_label(ui, format!("Asset Index build failed: {e}"));
                if super::icon_button(ui, icons::RETRY, "Retry").clicked() {
                    app.restart_index();
                }
            });
        }
        JobPhase::Finished | JobPhase::Cancelled => {
            if let Some(c) = app.model.coverage
                && !c.is_complete()
                && c.pending_bundles() > 0
            {
                ui.horizontal(|ui| {
                    ui.add_space(SECTION_WIDTH + ui.spacing().item_spacing.x);
                    warn_label(
                        ui,
                        format!(
                            "{} Bundles could not be indexed (network); search them by name only.",
                            c.pending_bundles()
                        ),
                    );
                    if super::icon_button(ui, icons::RETRY, "Retry").clicked() {
                        app.restart_index();
                    }
                });
            }
        }
        JobPhase::Idle => {}
    }
}
