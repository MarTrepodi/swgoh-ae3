//! Visual snapshots of the window, rendered with wgpu and compared with the
//! PNGs in `tests/snapshots/`. Run with `--features snapshots`; set
//! `UPDATE_SNAPSHOTS=1` to accept changes. A failing comparison writes
//! `<name>.diff.png` and `<name>.new.png` next to the reference.
//!
//! Everything drawn here is made up: invented Package and Asset names,
//! generated textures and sounds. Never put game assets, or anything derived
//! from them, into a snapshot.
#![cfg(feature = "snapshots")]
// Generated images are a few hundred pixels across, far below the 2^24
// that f32 represents exactly.
#![allow(
    clippy::cast_precision_loss,
    reason = "pixel coordinates of small generated images"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ae3_core::{
    AssetInfo, AssetKind, AssetRef, AssetVersion, Atlas, Diff, EntityKind, GameEntity,
    GamedataVersions, Language, NameIndex, NameIndexStatus, PackageKind, RgbaImage, SoundInfo,
    SpriteRect,
};
use ae3_gui::Ae3App;
use ae3_gui::app::{Detail, TexKey};
use ae3_gui::model::extract::{ExtractProgress, ExtractSummary};
use ae3_gui::model::names::NamesState;
use ae3_gui::model::selection::ClickModifiers;
use ae3_gui::model::viewer::{Channels, ViewTransform};
use ae3_gui::tasks::{DetailContent, DetailImage};
use ae3_gui_tests::{FakeCdn, Gui, Spec, TempDir, harness_with, package, synthetic_manifest};
use eframe::egui::{self, ColorImage, Theme, vec2};
use egui_kittest::kittest::Queryable as _;
use egui_kittest::{Harness, SnapshotOptions};

type H = Harness<'static, Gui>;

const VERSION: AssetVersion = AssetVersion(200_001);
const SIZE: egui::Vec2 = vec2(1280.0, 800.0);

/// The comparison tolerance. References may be generated on any platform
/// (macOS Metal, Linux lavapipe); anti-aliasing differs slightly between
/// rasterizers, which the per-pixel threshold absorbs.
fn options() -> SnapshotOptions {
    SnapshotOptions::new().threshold(1.0)
}

/// For snapshots showing magnified textures. With the smooth synthetic
/// pattern, GPUs that round texel edges differently disagree by about one
/// level on a few edge pixels; a real regression changes thousands.
fn texture_options() -> SnapshotOptions {
    options().max_failed_pixels(128)
}

/// A window showing the synthetic Manifest, offline.
fn window(name: &str, theme: Theme) -> (TempDir, H) {
    let dir = TempDir::new(name);
    let mut spec = Spec::new(dir.path(), FakeCdn::offline().net());
    // A fixed path: the temporary directory's name changes every run.
    spec.config.output_dir = PathBuf::from("ae3-output");
    let mut h = harness_with(spec, SIZE, theme);
    // Pinned: a release bumps the version, which must not change every
    // screenshot that shows the header.
    h.state_mut().app_mut().set_version_label("0.1.0");
    h.state_mut()
        .app_mut()
        .show_manifest(synthetic_manifest(VERSION))
        .unwrap();
    h.run_ok();
    (dir, h)
}

fn app(h: &mut H) -> &mut Ae3App {
    h.state_mut().app_mut()
}

/// The row id of the Package `name`.
fn row(h: &H, name: &str) -> usize {
    h.state()
        .app()
        .model
        .rows
        .iter()
        .find(|r| r.name == name)
        .map(|r| r.id)
        .unwrap()
}

/// Selects the row of `name` and opens its preview without fetching it.
fn select_for_preview(h: &mut H, name: &str) {
    let id = row(h, name);
    let model = &mut app(h).model;
    model.click_row(id, ClickModifiers::default());
    model.open_preview(id);
}

/// A generated RGBA image: smooth colour gradients over a soft alpha ramp,
/// so channel masking and transparency show. Neighbouring texels differ by
/// about one level, never by a hard edge: the viewer magnifies with nearest
/// sampling at fractional zooms, and GPUs may round a texel edge to
/// different screen pixels. With hard edges that moves whole rows of pixels
/// between colours and fails the comparison across renderers (macOS Metal
/// against Linux lavapipe); with gradients the same rounding costs a level.
fn pattern(width: u32, height: u32, seed: u8) -> RgbaImage {
    // Every value is reduced to 0..=255 before the conversion.
    let level = |value: u32| u8::try_from(value.min(255)).unwrap_or(u8::MAX);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    let (x_max, y_max) = (width.max(2) - 1, height.max(2) - 1);
    let shift = u32::from(seed) * 40;
    for y in 0..height {
        for x in 0..width {
            let red = (255 * x / x_max + shift) % 256;
            let green = 255 * y / y_max;
            let blue = 255 - (255 * (x + y) / (x_max + y_max));
            // 160..255: transparency shows, but the checkerboard behind
            // the texture stays faint.
            let alpha = 160 + 95 * x / x_max;
            pixels.extend_from_slice(&[level(red), level(green), level(blue), level(alpha)]);
        }
    }
    RgbaImage::new(width, height, pixels).unwrap()
}

fn color_image(image: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.pixels(),
    )
}

fn asset(path_id: i64, name: &str, kind: AssetKind, class: &str) -> AssetInfo {
    AssetInfo {
        path_id,
        name: name.into(),
        class_name: class.into(),
        script_class: None,
        kind,
        byte_size: 4_096 * u64::try_from(path_id).unwrap(),
        texture: None,
        sprite_count: (kind == AssetKind::Atlas).then_some(12),
        container_path: None,
    }
}

/// Opens the detail viewer on `asset` of `package` showing `content`.
fn show_detail(h: &mut H, package: &str, asset: AssetInfo, content: DetailContent) {
    let image = match &content {
        DetailContent::Image(image)
        | DetailContent::Atlas {
            image: Some(image), ..
        } => Some(image.color.clone()),
        _ => None,
    };
    let key = TexKey {
        package: package.into(),
        path_id: asset.path_id,
        full: true,
    };
    let ctx = h.ctx.clone();
    let app = app(h);
    app.detail = Some(Detail {
        asset,
        package: package.into(),
        generation: 1,
        content: Some(Ok(content)),
        transform: ViewTransform::default(),
        fitted: false,
        channels: Channels::ALL,
        shown_channels: Channels::ALL,
        selected_sprite: None,
        camera: ae3_gui::model::mesh::OrbitCamera::default(),
        open: true,
    });
    if let Some(image) = image {
        app.upload_texture(&ctx, key, image);
    }
}

fn detail_image(image: RgbaImage) -> DetailImage {
    DetailImage {
        color: color_image(&image),
        width: image.width(),
        height: image.height(),
        shown: Arc::new(image),
    }
}

fn main_window(theme: Theme, name: &str) {
    let (_dir, mut h) = window(name, theme);
    h.snapshot_options(name, &options());
}

#[test]
fn main_window_dark() {
    main_window(Theme::Dark, "main_window_dark");
}

#[test]
fn main_window_light() {
    main_window(Theme::Light, "main_window_light");
}

#[test]
fn settings_window() {
    let (_dir, mut h) = window("settings", Theme::Dark);
    app(&mut h).model.windows.settings = true;
    h.hover_at(egui::pos2(1270.0, 790.0));
    h.run_ok();
    h.snapshot_options("settings_window", &options());
}

#[test]
fn search_results_list_game_entities() {
    let (_dir, mut h) = window("search_entities", Theme::Dark);
    let entity = |kind, id: &str, name: &str, bundles: &[&str]| GameEntity {
        kind,
        id: id.into(),
        name: name.into(),
        context: None,
        bundles: bundles.iter().map(|b| (*b).to_owned()).collect(),
    };
    let index = NameIndex::new(
        Language::default(),
        GamedataVersions::default(),
        vec![
            entity(
                EntityKind::Unit,
                "TESTPILOT",
                "Test Pilot",
                &["charui_testpilot", "vanity_testpilot"],
            ),
            entity(
                EntityKind::Ability,
                "ability_testpilot01",
                "Test Strike",
                &["ability_testpilot_strike"],
            ),
            entity(EntityKind::Gear, "test_gear", "Test Visor", &[]),
        ],
    );
    let state = NamesState::new(Some(&index), NameIndexStatus::Built, None, false);
    app(&mut h).model.set_names(Some(Arc::new(index)), state);
    let field = h.get_by_role_and_label(egui::accesskit::Role::TextInput, "Asset Search");
    field.focus();
    field.type_text("test");
    h.run_ok();
    h.get_by_label_contains("Test Pilot · 2").click();
    // The pointer away from the buttons, so nothing is hovered.
    h.hover_at(egui::pos2(1270.0, 790.0));
    h.run_ok();
    h.snapshot_options("search_game_entities", &options());
}

#[test]
fn bundle_preview_grid_with_thumbnails() {
    let (_dir, mut h) = window("preview_grid", Theme::Dark);
    select_for_preview(&mut h, "charui_testpilot");
    let assets = vec![
        asset(1, "testpilot_portrait", AssetKind::Texture, "Texture2D"),
        asset(2, "testpilot_atlas", AssetKind::Atlas, "MonoBehaviour"),
        asset(3, "testpilot_icon", AssetKind::Sprite, "Sprite"),
        asset(4, "testpilot_mesh", AssetKind::Mesh, "Mesh"),
        asset(5, "testpilot_data", AssetKind::Structured, "MonoBehaviour"),
        asset(6, "testpilot_notes", AssetKind::Text, "TextAsset"),
        asset(7, "testpilot_bark", AssetKind::Audio, "AudioClip"),
        asset(8, "testpilot_glow", AssetKind::Texture, "Texture2D"),
    ];
    let ctx = h.ctx.clone();
    let a = app(&mut h);
    a.model.preview.as_mut().unwrap().set_assets(assets);
    a.model.preview.as_mut().unwrap().toggle_asset(1);
    for (path_id, (w, hgt), seed) in [
        (1, (112, 112), 0),
        (2, (128, 64), 1),
        (3, (48, 48), 2),
        (8, (64, 96), 1),
    ] {
        let image = pattern(w, hgt, seed);
        a.upload_texture(
            &ctx,
            TexKey {
                package: "charui_testpilot".into(),
                path_id,
                full: false,
            },
            color_image(&image),
        );
    }
    h.hover_at(egui::pos2(1270.0, 790.0));
    h.run_ok();
    h.snapshot_options("bundle_preview_grid", &texture_options());
}

#[test]
fn audio_preview_keeps_its_width() {
    let (_dir, mut h) = window("audio_preview", Theme::Dark);
    select_for_preview(&mut h, "audio_test_ui");
    let sounds = (0..6_u64)
        .map(|i| SoundInfo {
            index: usize::try_from(i).unwrap(),
            id: 900_000 + i,
            stem: format!("{}", 900_000 + i),
            language: if i % 2 == 0 { "sfx" } else { "english(us)" }.into(),
            source: "stream",
            size: 20_000 + 7_000 * i,
            codec: Some(if i == 5 { "Opus" } else { "Wwise Vorbis" }),
            channels: Some(2),
            sample_rate: Some(48_000),
            duration_secs: Some(1.5 + i as f64),
        })
        .collect();
    let a = app(&mut h);
    a.model.preview.as_mut().unwrap().set_sounds(sounds);
    a.playback.set_unplayable(
        ae3_gui::model::playback::SoundRef::Sound(5),
        "Opus is not decoded",
    );
    // Laid out over several frames, the pane must not creep across the
    // table.
    for _ in 0..10 {
        h.step();
    }
    h.run_ok();
    let heading = h
        .get_all_by_label("audio_test_ui")
        .map(|n| n.rect().min.x)
        .fold(0.0_f32, f32::max);
    assert!(
        heading > SIZE.x / 2.0,
        "the preview pane starts at x={heading}, over most of the window"
    );
    h.snapshot_options("audio_preview", &options());
}

fn detail_texture(theme: Theme, name: &str) {
    let (_dir, mut h) = window(name, theme);
    show_detail(
        &mut h,
        "charui_testpilot",
        asset(1, "testpilot_portrait", AssetKind::Texture, "Texture2D"),
        DetailContent::Image(detail_image(pattern(256, 256, 0))),
    );
    h.hover_at(egui::pos2(1270.0, 790.0));
    h.run_ok();
    h.snapshot_options(name, &texture_options());
}

#[test]
fn detail_viewer_texture_dark() {
    detail_texture(Theme::Dark, "detail_texture_dark");
}

#[test]
fn detail_viewer_texture_light() {
    detail_texture(Theme::Light, "detail_texture_light");
}

#[test]
fn detail_viewer_hides_channels() {
    // Red and alpha hidden: the remasked texture replaces the shown one.
    let (_dir, mut h) = window("detail_channels", Theme::Dark);
    show_detail(
        &mut h,
        "charui_testpilot",
        asset(1, "testpilot_portrait", AssetKind::Texture, "Texture2D"),
        DetailContent::Image(detail_image(pattern(256, 256, 0))),
    );
    h.run_ok();
    for channel in ["R", "A"] {
        h.get_by_role_and_label(egui::accesskit::Role::Button, channel)
            .click();
        h.run_ok();
    }
    let started = Instant::now();
    let masked = |h: &H| {
        let d = h.state().app().detail.as_ref().unwrap();
        !d.channels.r && !d.channels.a && d.shown_channels == d.channels
    };
    assert!(masked(&h));
    // The remask runs on a worker; give it a moment to arrive.
    while started.elapsed().as_millis() < 500 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.hover_at(egui::pos2(1270.0, 790.0));
    h.run_ok();
    h.snapshot_options("detail_channels_masked", &texture_options());
}

#[test]
fn atlas_overlay_outlines_the_sprites() {
    let (_dir, mut h) = window("atlas", Theme::Dark);
    let sprite = |name: &str, x, y, width, height| SpriteRect {
        name: name.into(),
        x,
        y,
        width,
        height,
        border_left: 0,
        border_right: 0,
        border_top: 0,
        border_bottom: 0,
        padding_left: 0,
        padding_right: 0,
        padding_top: 0,
        padding_bottom: 0,
    };
    let sprites = vec![
        sprite("test_icon_star", 0, 0, 64, 64),
        sprite("test_icon_shield", 64, 0, 64, 64),
        sprite("test_frame_wide", 0, 64, 128, 32),
        sprite("test_badge", 128, 0, 96, 96),
        sprite("test_dot", 224, 0, 32, 32),
    ];
    let atlas = Atlas {
        name: "unit_test_atlas".into(),
        asset: AssetRef {
            package: "unit_test_atlas".into(),
            file: "CAB-00000000000000000000000000000000".into(),
            path_id: 2,
        },
        replacement: None,
        pixel_size: 1.0,
        texture: None,
        texture_error: None,
        sprites,
    };
    show_detail(
        &mut h,
        "unit_test_atlas",
        asset(2, "unit_test_atlas", AssetKind::Atlas, "MonoBehaviour"),
        DetailContent::Atlas {
            atlas,
            image: Some(detail_image(pattern(256, 128, 2))),
            error: None,
        },
    );
    h.run_ok();
    h.get_by_label("test_icon_shield (64×64)").click();
    h.hover_at(egui::pos2(1270.0, 790.0));
    h.run_ok();
    assert_eq!(
        h.state().app().detail.as_ref().unwrap().selected_sprite,
        Some(1)
    );
    h.snapshot_options("atlas_overlay", &texture_options());
}

#[test]
fn diff_tab_lists_the_changes() {
    let (_dir, mut h) = window("diff", Theme::Dark);
    let new = synthetic_manifest(VERSION);
    let mut old = new.clone();
    old.asset_version = AssetVersion(VERSION.0 - 3);
    old.packages.retain(|p| p.name != "charui_testmedic");
    old.packages[0].content_hash ^= 1;
    old.packages[3].content_hash ^= 1;
    old.packages
        .push(package("charui_retiredtest", PackageKind::Bundle, 22_528));
    let a = app(&mut h);
    a.model.diff.old_text = old.asset_version.to_string();
    a.model.diff.set_diff(Arc::new(Diff::compute(&old, &new)));
    a.model.tab = ae3_gui::model::Tab::Diff;
    h.hover_at(egui::pos2(1270.0, 790.0));
    h.run_ok();
    h.snapshot_options("diff_tab", &options());
}

#[test]
fn extraction_progress_and_summary() {
    let (_dir, mut h) = window("extract", Theme::Dark);
    let a = app(&mut h);
    let generation = a.model.extract_job.start(Instant::now());
    a.model.extract_label = "3 Packages".into();
    a.model.extract_job.update(
        generation,
        ExtractProgress {
            packages_done: 1,
            packages_total: 3,
            current: Some("charui_testgunner".into()),
            assets_done: 2,
            assets_total: 5,
        },
    );
    h.hover_at(egui::pos2(1270.0, 790.0));
    h.run_ok();
    h.snapshot_options("extraction_progress", &options());

    let a = app(&mut h);
    assert!(a.model.extract_job.finish(generation, Ok(false)));
    a.model.summary = Some(ExtractSummary {
        output_dir: PathBuf::from("ae3-output"),
        files: 14,
        bytes: 1_234_567,
        skipped: 2,
        cancelled: false,
        errors: vec!["charui_testmedic / broken_tex: unsupported texture format 99".into()],
        failure: None,
    });
    h.run_ok();
    h.snapshot_options("extraction_summary", &options());
}
