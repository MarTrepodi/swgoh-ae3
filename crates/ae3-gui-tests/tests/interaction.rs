//! The window driven through its AccessKit tree, as a user would: find
//! widgets by role and label, click, type and press keys, then check what
//! the window shows and what the app did. Everything runs offline against
//! a [`FakeCdn`]; tests that need game fixtures skip without them unless
//! `AE3_REQUIRE_FIXTURES=1`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ae3_core::{AssetKind, AssetVersion, ConfigFile, PackageKind};
use ae3_gui::model::playback::{PlayState, SoundRef};
use ae3_gui::tasks::DetailContent;
use ae3_gui_tests::{
    FIXTURE_VERSION, FakeCdn, Gui, Played, Recorder, Spec, TempDir, fixture, fixtures_dir, harness,
    package, synthetic_manifest, wait_for,
};
use eframe::egui::accesskit::{Role, Toggled};
use eframe::egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

type H = Harness<'static, Gui>;

const SYNTHETIC: AssetVersion = AssetVersion(200_001);

/// A cache directory holding the fixture Manifest as Asset Version 100054
/// of Windows on PROD, as an earlier session left it.
fn seed_cache(dir: &TempDir, manifest: &Path) {
    let target = dir.path().join("cache/manifests/PROD/Windows");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::copy(manifest, target.join(format!("{FIXTURE_VERSION}.data"))).unwrap();
}

/// Clicks the widget labelled `label` and lets the window settle.
#[track_caller]
fn click(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.run_ok();
}

/// Scrolls the widget labelled `label` into view, as a user would, and
/// clicks it.
#[track_caller]
fn scroll_and_click(h: &mut H, label: &str) {
    h.get_by_label(label).scroll_to_me();
    h.run_ok();
    click(h, label);
}

/// The text field labelled `label`.
#[track_caller]
fn text_field<'a>(h: &'a H, label: &'a str) -> egui_kittest::Node<'a> {
    h.get_by_role_and_label(Role::TextInput, label)
}

/// Types `text` into the text field labelled `label`.
#[track_caller]
fn type_into(h: &mut H, label: &str, text: &str) {
    text_field(h, label).focus();
    h.run_ok();
    text_field(h, label).type_text(text);
    h.run_ok();
}

/// Clicks the Bundles table row of `name` (its name label, the leftmost
/// of that text: the preview pane's heading may repeat it) with
/// `modifiers` held.
#[track_caller]
fn click_row(h: &mut H, name: &str, modifiers: Modifiers) {
    h.get_all_by_label(name)
        .min_by(|a, b| a.rect().min.x.total_cmp(&b.rect().min.x))
        .unwrap_or_else(|| panic!("no row {name}"))
        .click_modifiers(modifiers);
    h.run_ok();
}

/// Double-clicks the widget labelled `label`.
#[track_caller]
fn double_click(h: &mut H, label: &str) {
    let node = h.get_by_label(label);
    node.click();
    node.click();
    h.run_ok();
}

/// The labels of the Bundles table's Name column now on screen, among
/// `names`.
fn shown(h: &H, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter(|n| h.query_by_label(n).is_some())
        .map(|n| (*n).to_owned())
        .collect()
}

/// A window over a [`FakeCdn`] publishing the synthetic Manifest, loaded with
/// Fetch latest.
fn synthetic_window(name: &str) -> (TempDir, Arc<FakeCdn>, H) {
    let dir = TempDir::new(name);
    let cdn = FakeCdn::empty();
    cdn.publish_latest(&synthetic_manifest(SYNTHETIC));
    let mut h = harness(Spec::new(dir.path(), cdn.net()));
    fetch_latest(&mut h);
    (dir, cdn, h)
}

/// Presses Fetch latest and waits for the Manifest.
fn fetch_latest(h: &mut H) {
    click(h, "Fetch latest");
    wait_for(h, "the Manifest", |app| !app.model.manifest_job.is_active());
}

/// A window over the fixture CDN, with the fixture Manifest loaded and the
/// table narrowed to `bundle`, whose row is clicked; waits for the
/// preview.
fn fixture_preview(name: &str, bundle: &str) -> Option<(TempDir, H)> {
    fixture("manifest.data")?;
    let file = if fixtures_dir().join(format!("{bundle}.wwpkg")).exists() {
        format!("{bundle}.wwpkg")
    } else {
        format!("{bundle}.bundle")
    };
    fixture(&file)?;
    let dir = TempDir::new(name);
    let mut h = harness(Spec::new(
        dir.path(),
        FakeCdn::serving(fixtures_dir()).net(),
    ));
    fetch_latest(&mut h);
    assert_eq!(h.state().app().model.asset_version(), Some(FIXTURE_VERSION));
    type_into(&mut h, "Asset Search", bundle);
    click_row(&mut h, bundle, Modifiers::NONE);
    wait_for(&mut h, "the preview", |app| {
        app.model
            .preview
            .as_ref()
            .is_some_and(|p| !matches!(p.status, ae3_gui::model::preview::PreviewStatus::Loading))
    });
    Some((dir, h))
}

/// Opens the fixture Bundle `bundle` in the preview straight from its file,
/// over the fixture Manifest (no download).
fn fixture_bundle_file(name: &str, bundle: &str) -> Option<(TempDir, H)> {
    let manifest = fixture("manifest.data")?;
    let path = fixture(&format!("{bundle}.bundle"))?;
    let dir = TempDir::new(name);
    let mut h = harness(Spec::new(dir.path(), FakeCdn::offline().net()));
    let bytes = std::fs::read(manifest).unwrap();
    let app = h.state_mut().app_mut();
    app.show_manifest(ae3_core::Manifest::decode(&bytes).unwrap())
        .unwrap();
    app.open_bundle_file(&path).unwrap();
    h.run_ok();
    Some((dir, h))
}

/// Waits until the detail viewer has its content.
fn wait_for_detail(h: &mut H) {
    wait_for(h, "the detail viewer", |app| {
        app.detail.as_ref().is_some_and(|d| d.content.is_some())
    });
    assert!(
        matches!(
            h.state()
                .app()
                .detail
                .as_ref()
                .and_then(|d| d.content.as_ref()),
            Some(Ok(_))
        ),
        "the detail viewer shows an error"
    );
}

// ---- Startup ----------------------------------------------------------------

#[test]
fn startup_opens_the_cached_asset_version_while_offline() {
    let Some(manifest) = fixture("manifest.data") else {
        return;
    };
    let dir = TempDir::new("startup");
    seed_cache(&dir, &manifest);
    let cdn = FakeCdn::offline();
    let mut spec = Spec::new(dir.path(), cdn.net());
    spec.file.gui.last_asset_version = Some(FIXTURE_VERSION);
    spec.start = true;
    let mut h = harness(spec);
    wait_for(&mut h, "the startup load", |app| {
        !app.model.manifest_job.is_active() && !app.model.startup.is_pending()
    });
    h.get_by_label_contains("Asset Version 100054 · Windows · PROD");
    h.get_by_label_contains("the latest could not be found");
    assert_eq!(h.state().app().model.asset_version(), Some(FIXTURE_VERSION));
    let rows = h.state().app().model.rows.len();
    assert!(rows > 1000, "{rows} rows");
    // The Bundles shown first are on screen.
    let first = h.state().app().model.visible[0];
    let name = h.state().app().model.rows[first].name.clone();
    h.get_by_label(&name);
}

// ---- The Bundles table ------------------------------------------------------

#[test]
fn typing_a_search_filters_the_bundles_table() {
    let (_dir, _cdn, mut h) = synthetic_window("search");
    let all = [
        "charui_testpilot",
        "charui_testgunner",
        "vanity_testpilot",
        "ability_testpilot_strike",
        "audio_test_ui",
    ];
    assert_eq!(shown(&h, &all), all);
    h.get_by_label("9 Packages");

    type_into(&mut h, "Asset Search", "testpilot");
    assert_eq!(
        shown(&h, &all),
        [
            "charui_testpilot",
            "vanity_testpilot",
            "ability_testpilot_strike"
        ]
    );
    h.get_by_label("3 of 9 Packages");
    h.get_by_label_contains("3 Bundles contain 'testpilot'");
    assert_eq!(
        text_field(&h, "Asset Search").value().as_deref(),
        Some("testpilot")
    );

    // Backspace narrows the query back down; the clear button empties it.
    text_field(&h, "Asset Search").focus();
    for _ in 0.."pilot".len() {
        h.key_press(Key::Backspace);
    }
    h.run_ok();
    assert_eq!(h.state().app().model.query, "test");
    click(&mut h, "Clear search");
    assert_eq!(shown(&h, &all), all);
}

#[test]
fn the_type_filter_shows_only_packages_holding_that_kind() {
    let (_dir, _cdn, mut h) = synthetic_window("type-filter");
    let names = ["charui_testpilot", "audio_test_ui", "audio_test_music"];
    let audio = h.get_by_label("Audio");
    assert_eq!(audio.accesskit_node().toggled(), Some(Toggled::False));
    click(&mut h, "Audio");
    assert_eq!(
        h.get_by_label("Audio").accesskit_node().toggled(),
        Some(Toggled::True)
    );
    assert_eq!(shown(&h, &names), ["audio_test_ui", "audio_test_music"]);
    h.get_by_label("2 of 9 Packages");

    // Bundles whose contents are not known yet are hidden by any filter.
    click(&mut h, "Texture");
    assert_eq!(shown(&h, &names), ["audio_test_ui", "audio_test_music"]);
    click(&mut h, "All types");
    assert_eq!(shown(&h, &names), names);
}

#[test]
fn shift_and_command_clicks_select_several_rows() {
    let (_dir, _cdn, mut h) = synthetic_window("multi-select");
    // Sorted by name: ability_…, audio_test_music, audio_test_ui, charui_…
    click_row(&mut h, "audio_test_music", Modifiers::NONE);
    assert_eq!(h.state().app().model.selection.len(), 1);
    assert!(h.state().app().model.preview.is_some(), "one row previews");

    click_row(&mut h, "charui_testmedic", Modifiers::SHIFT);
    let selected = |h: &H| {
        let model = &h.state().app().model;
        let mut names: Vec<String> = model
            .selection
            .iter()
            .map(|id| model.rows[id].name.clone())
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        selected(&h),
        [
            "audio_test_music",
            "audio_test_ui",
            "charui_testgunner",
            "charui_testmedic"
        ]
    );
    h.get_by_label("9 Packages · 4 selected");
    assert!(
        h.state().app().model.preview.is_none(),
        "several rows do not"
    );

    // Ctrl (Windows, Linux) toggles one row.
    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    click_row(&mut h, "audio_test_ui", ctrl);
    click_row(&mut h, "vanity_testpilot", ctrl);
    assert_eq!(
        selected(&h),
        [
            "audio_test_music",
            "charui_testgunner",
            "charui_testmedic",
            "vanity_testpilot"
        ]
    );
    h.get_by_label("Extract selected (4)");

    click(&mut h, "Clear selection");
    assert_eq!(h.state().app().model.selection.len(), 0);
}

#[test]
fn cmd_clicks_toggle_rows_on_macos() {
    let dir = TempDir::new("mac-select");
    let cdn = FakeCdn::empty();
    cdn.publish_latest(&synthetic_manifest(SYNTHETIC));
    let mut spec = Spec::new(dir.path(), cdn.net());
    spec.os = eframe::egui::os::OperatingSystem::Mac;
    let mut h = harness(spec);
    fetch_latest(&mut h);
    // As egui-winit reports ⌘ on a Mac.
    let cmd = Modifiers {
        mac_cmd: true,
        command: true,
        ..Modifiers::NONE
    };
    click_row(&mut h, "charui_testpilot", Modifiers::NONE);
    click_row(&mut h, "vanity_testpilot", cmd);
    click_row(&mut h, "audio_test_ui", cmd);
    assert_eq!(h.state().app().model.selection.len(), 3);
    click_row(&mut h, "charui_testpilot", cmd);
    assert_eq!(h.state().app().model.selection.len(), 2);
    // A plain click starts over.
    click_row(&mut h, "audio_test_ui", Modifiers::NONE);
    assert_eq!(h.state().app().model.selection.len(), 1);
}

// ---- Preview and extraction ---------------------------------------------------

#[test]
fn a_fixture_bundle_previews_with_thumbnails_and_extracts_checked_assets() {
    let Some((dir, mut h)) = fixture_preview("extract", "char_bossk_pre") else {
        return;
    };
    wait_for(&mut h, "the thumbnails", |app| {
        app.thumb_requested.is_empty() && !app.textures.is_empty()
    });
    // Thumbnails are decoded into GPU textures for the visible cells.
    assert!(h.state().app().thumb_failed.is_empty());
    // The pane is headed by the Bundle's name (also in the table).
    assert_eq!(h.get_all_by_label("char_bossk_pre").count(), 2);

    let check = |h: &H, name: &str| {
        h.get_by_label(&format!("Extract {name}"))
            .accesskit_node()
            .toggled()
    };
    assert_eq!(check(&h, "bossk_tex"), Some(Toggled::False));
    scroll_and_click(&mut h, "Extract bossk_tex");
    scroll_and_click(&mut h, "Extract rfl_bossk_tex");
    assert_eq!(check(&h, "bossk_tex"), Some(Toggled::True));
    h.get_by_label("2 checked");

    click(&mut h, "Extract 2 Assets");
    wait_for(&mut h, "the extraction", |app| {
        !app.model.extract_job.is_active() && app.model.summary.is_some()
    });
    let summary = h.state().app().model.summary.clone().unwrap();
    assert!(summary.failure.is_none(), "{summary:?}");
    assert!(summary.errors.is_empty(), "{:?}", summary.errors);
    h.get_by_label(&summary.headline());
    h.get_by_label("Reveal in folder");

    let mut written: Vec<PathBuf> = walk(&dir.path().join("out"))
        .into_iter()
        .map(|p| p.strip_prefix(dir.path().join("out")).unwrap().to_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        [
            Path::new("char/char_bossk_pre/bossk_tex.png"),
            Path::new("char/char_bossk_pre/rfl_bossk_tex.png"),
        ]
    );
    for file in &written {
        let bytes = std::fs::read(dir.path().join("out").join(file)).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"), "{} is a PNG", file.display());
    }
}

/// Every file under `dir`.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

// ---- The detail viewer ------------------------------------------------------

#[test]
fn texture_channels_toggle_in_the_detail_viewer() {
    let Some((_dir, mut h)) = fixture_bundle_file("channels", "charui_bossk") else {
        return;
    };
    let selected = h.state().app().model.selection.clone();
    // The preview pane sits beside the Bundles table: a double click on a
    // cell opens it and leaves the table's selection alone.
    double_click(&mut h, "tex.charui_bossk");
    assert_eq!(h.state().app().model.selection, selected);
    wait_for_detail(&mut h);
    wait_for(&mut h, "the detail texture", |app| {
        app.textures.contains(&ae3_gui::app::TexKey {
            package: "charui_bossk".into(),
            path_id: app.detail.as_ref().unwrap().asset.path_id,
            full: true,
        })
    });
    h.get_by_label_contains("tex.charui_bossk — charui_bossk");
    let toggled = |h: &H, channel: &str| {
        h.get_by_role_and_label(Role::Button, channel)
            .accesskit_node()
            .toggled()
    };
    assert_eq!(toggled(&h, "R"), Some(Toggled::True));
    assert!(h.query_by_label("All").is_none(), "all channels shown");

    click(&mut h, "R");
    click(&mut h, "A");
    let detail = |h: &H| {
        let d = h.state().app().detail.as_ref().unwrap();
        (d.channels, d.shown_channels)
    };
    let (wanted, shown) = detail(&h);
    assert!(!wanted.r && wanted.g && wanted.b && !wanted.a);
    assert_eq!(wanted, shown, "remasked");
    assert_eq!(toggled(&h, "R"), Some(Toggled::False));

    click(&mut h, "All");
    assert!(detail(&h).0.is_all());
    assert_eq!(toggled(&h, "A"), Some(Toggled::True));
}

#[test]
fn atlas_sprites_are_selected_from_the_list() {
    let Some((_dir, mut h)) = fixture_bundle_file("atlas", "unit_details_atlas") else {
        return;
    };
    // The Atlas and its texture share a name; the Atlas cell says "sprites".
    let atlas = h
        .state()
        .app()
        .model
        .preview
        .as_ref()
        .unwrap()
        .cells()
        .into_iter()
        .find(|a| a.kind == AssetKind::Atlas)
        .cloned()
        .unwrap();
    h.state_mut().app_mut().open_detail(&atlas);
    wait_for_detail(&mut h);
    let sprites = match &h.state().app().detail.as_ref().unwrap().content {
        Some(Ok(DetailContent::Atlas { atlas, .. })) => atlas.sprites.clone(),
        _ => panic!("expected an Atlas"),
    };
    assert_ne!(sprites.len(), 0);
    h.get_by_label(&format!("{} Sprites", sprites.len()));

    let first = &sprites[0];
    let entry = format!("{} ({}×{})", first.name, first.width, first.height);
    click(&mut h, &entry);
    assert_eq!(
        h.state().app().detail.as_ref().unwrap().selected_sprite,
        Some(0)
    );
    assert_eq!(
        h.get_by_label(&entry).accesskit_node().toggled(),
        Some(Toggled::True)
    );
    // The selected Sprite's details head the list.
    h.get_by_label(&format!(
        "x {} y {} · {}×{}",
        first.x, first.y, first.width, first.height
    ));

    // Clicking it again clears the selection.
    click(&mut h, &entry);
    assert_eq!(
        h.state().app().detail.as_ref().unwrap().selected_sprite,
        None
    );
}

#[test]
fn a_structured_asset_expands_as_a_tree() {
    let Some((_dir, mut h)) = fixture_bundle_file("tree", "unit_details_atlas") else {
        return;
    };
    let data = h
        .state()
        .app()
        .model
        .preview
        .as_ref()
        .unwrap()
        .cells()
        .into_iter()
        .find(|a| a.kind == AssetKind::Structured)
        .cloned()
        .unwrap();
    h.state_mut().app_mut().open_detail(&data);
    wait_for_detail(&mut h);
    let root = h.get_by_label_contains("root  {");
    assert_eq!(root.accesskit_node().data().is_expanded(), Some(true));

    // A nested map or array starts collapsed and opens on a click.
    let nested = h
        .get_all_by(|n| {
            n.label()
                .is_some_and(|l| !l.starts_with("root") && (l.contains("  {") || l.contains("  [")))
                && n.data().is_expanded() == Some(false)
        })
        .next()
        .map(|n| n.accesskit_node().label().unwrap_or_default())
        .expect("a collapsed container");
    let before = h.root().children_recursive().count();
    click(&mut h, &nested);
    assert_eq!(
        h.get_by_label(&nested)
            .accesskit_node()
            .data()
            .is_expanded(),
        Some(true)
    );
    assert!(
        h.root().children_recursive().count() > before,
        "children shown"
    );
}

#[test]
fn text_assets_show_highlighted_and_flag_truncation() {
    // No fixture Bundle holds a TextAsset, so the viewer is given one.
    let dir = TempDir::new("text");
    let mut h = harness(Spec::new(dir.path(), FakeCdn::offline().net()));
    let asset = ae3_core::AssetInfo {
        path_id: 7,
        name: "test_config".into(),
        class_name: "TextAsset".into(),
        script_class: None,
        kind: AssetKind::Text,
        byte_size: 64,
        texture: None,
        sprite_count: None,
        container_path: None,
    };
    let text = "{\"name\": \"test\", \"count\": 3}".to_owned();
    h.state_mut().app_mut().detail = Some(ae3_gui::app::Detail {
        asset,
        package: "test_bundle".into(),
        generation: 1,
        content: Some(Ok(DetailContent::Text {
            text: text.clone(),
            extension: "json",
            truncated: true,
        })),
        transform: ae3_gui::model::viewer::ViewTransform::default(),
        fitted: false,
        channels: ae3_gui::model::viewer::Channels::ALL,
        shown_channels: ae3_gui::model::viewer::Channels::ALL,
        selected_sprite: None,
        camera: ae3_gui::model::mesh::OrbitCamera::default(),
        open: true,
    });
    h.run_ok();
    h.get_by_label_contains("test_config — test_bundle");
    h.get_by_label(&format!("json · {} characters", text.chars().count()));
    h.get_by_label("(truncated; extract the Asset for the whole text)");
    h.get_by_label(&text);
}

#[test]
fn a_mesh_opens_in_the_3d_view() {
    let Some((_dir, mut h)) = fixture_bundle_file("mesh", "char_bossk_pre") else {
        return;
    };
    let mesh = h
        .state()
        .app()
        .model
        .preview
        .as_ref()
        .unwrap()
        .cells()
        .into_iter()
        .find(|a| a.kind == AssetKind::Mesh && a.name == "bossk_mesh")
        .cloned()
        .unwrap();
    h.state_mut().app_mut().open_detail(&mesh);
    wait_for_detail(&mut h);
    h.get_by_label("1693 vertices · 2502 triangles · 1 submesh · UVs");
    h.get_by_label("Reset view");
    assert!(matches!(
        h.state().app().detail.as_ref().unwrap().content,
        Some(Ok(DetailContent::Mesh(_)))
    ));

    // The window's close button closes the viewer.
    h.get_by_label("Close window").click();
    h.run_ok();
    assert!(h.state().app().detail.is_none());
}

// ---- Audio --------------------------------------------------------------------

/// The fixture Audio Package's preview, and the first sound that plays.
fn audio_preview(name: &str) -> Option<(TempDir, H, usize, String)> {
    let (dir, mut h) = fixture_preview(name, "audio_sharedui")?;
    let sounds = match &h.state().app().model.preview.as_ref().unwrap().status {
        ae3_gui::model::preview::PreviewStatus::Audio { sounds } => sounds.clone(),
        other => panic!("expected sounds, got {other:?}"),
    };
    let playable = sounds
        .iter()
        .find(|s| h.state().app().playback.state(SoundRef::Sound(s.index)) == PlayState::Idle)
        .expect("a playable sound");
    let (index, stem) = (playable.index, playable.stem.clone());
    h.run_ok();
    Some((dir, h, index, stem))
}

/// The play control in the row of the sound `stem`.
fn play_button<'a>(h: &'a H, stem: &str) -> egui_kittest::Node<'a> {
    let row = h.get_by_label(stem).rect();
    h.get_all_by_role(Role::Button)
        .find(|b| {
            let r = b.rect();
            (r.center().y - row.center().y).abs() < 4.0 && r.max.x <= row.min.x
        })
        .expect("a play control in the row")
}

#[test]
fn audio_package_sounds_play_and_stop() {
    let Some((_dir, mut h, index, stem)) = audio_preview("audio-play") else {
        return;
    };
    let recorder = Recorder::default();
    h.state_mut()
        .app_mut()
        .set_audio_output(Box::new(recorder.clone()));
    assert_eq!(
        play_button(&h, &stem).accesskit_node().label().as_deref(),
        Some("Play")
    );
    play_button(&h, &stem).click();
    let sound = SoundRef::Sound(index);
    wait_for(&mut h, "playback", |app| {
        app.playback.state(sound) == PlayState::Playing
    });
    assert!(matches!(
        recorder.take().as_slice(),
        [Played::Stop, Played::Play { frames, .. }] if *frames > 0
    ));
    assert_eq!(
        play_button(&h, &stem).accesskit_node().label().as_deref(),
        Some("Stop")
    );
    play_button(&h, &stem).click();
    h.run_ok();
    assert_eq!(h.state().app().playback.state(sound), PlayState::Idle);
    assert_eq!(recorder.take(), [Played::Stop]);

    // Ticking a sound offers to extract it.
    click(&mut h, &format!("Extract {stem}"));
    h.get_by_label("Extract 1 Sound");
}

#[test]
fn a_missing_sound_device_marks_the_sound_and_keeps_the_window_working() {
    let Some((_dir, mut h, index, stem)) = audio_preview("audio-no-device") else {
        return;
    };
    let report = h.state().app().playback_end_reporter();
    h.state_mut()
        .app_mut()
        .set_audio_output(Box::new(ae3_gui_tests::NoDevice(report)));
    play_button(&h, &stem).click();
    let sound = SoundRef::Sound(index);
    wait_for(&mut h, "the device error", |app| {
        matches!(app.playback.state(sound), PlayState::Unplayable(_))
    });
    let PlayState::Unplayable(why) = h.state().app().playback.state(sound) else {
        unreachable!()
    };
    assert!(why.contains("no audio output"), "{why}");
    // The row's play control turns into "can't play", the reason on hover.
    let row = h.get_by_label(&stem).rect();
    assert!(
        h.get_all_by_label("can't play")
            .any(|n| (n.rect().center().y - row.center().y).abs() < 4.0),
        "the row says it can't play"
    );
    // The window goes on working: the sound can still be ticked.
    click(&mut h, &format!("Extract {stem}"));
    h.get_by_label("Extract 1 Sound");
}

// ---- Diff ---------------------------------------------------------------------

#[test]
fn the_diff_tab_compares_two_manifests() {
    let Some(path) = fixture("manifest.data") else {
        return;
    };
    let new = ae3_core::Manifest::decode(&std::fs::read(path).unwrap()).unwrap();
    // An older Asset Version: two Packages were added since, one changed
    // and one removed.
    let mut old = new.clone();
    old.asset_version = AssetVersion(FIXTURE_VERSION.0 - 1);
    let added: Vec<String> = old.packages.drain(..2).map(|p| p.name).collect();
    old.packages[0].content_hash ^= 1;
    let changed = old.packages[0].name.clone();
    old.packages
        .push(package("charui_retired_test", PackageKind::Bundle, 4_096));

    let dir = TempDir::new("diff");
    let cdn = FakeCdn::serving(fixtures_dir());
    cdn.publish(&old);
    let mut h = harness(Spec::new(dir.path(), cdn.net()));
    fetch_latest(&mut h);

    click(&mut h, "Diff");
    h.get_by_label_contains("Enter an older Asset Version");
    type_into(&mut h, "Old Asset Version", "100053");
    click(&mut h, "Compare");
    wait_for(&mut h, "the Diff", |app| app.model.diff.diff.is_some());

    h.get_by_label("Added (2)");
    h.get_by_label("Changed (1)");
    h.get_by_label("Removed (1)");
    h.get_by_label_contains("100053 → 100054: 2 added");
    // All (added and changed) is listed first.
    for name in added.iter().chain([&changed]) {
        h.get_by_label(name);
    }
    click(&mut h, "Removed (1)");
    h.get_by_label("charui_retired_test");
    h.get_by_label("Removed Packages exist only in the older Asset Version.");
    assert!(
        h.get_by_label("Extract all (1)")
            .accesskit_node()
            .is_disabled()
    );
}

// ---- Settings -----------------------------------------------------------------

#[test]
fn output_settings_persist_to_the_config_file() {
    let dir = TempDir::new("settings");
    let path = dir.path().join("config.toml");
    let mut spec = Spec::new(dir.path(), FakeCdn::offline().net());
    spec.config_path = Some(path.clone());
    let mut h = harness(spec);
    // The sidebar sums the settings up; they are edited in their window.
    h.get_by_label("Layout default");
    assert!(h.query_by_label("Skip existing").is_none());
    click(&mut h, "Edit settings");

    click(&mut h, "Skip existing");
    click(&mut h, "Split sprite atlases");
    // "Restore padding" only applies to cut Sprites.
    assert!(
        h.get_by_label("Restore padding")
            .accesskit_node()
            .is_disabled()
    );
    let layout = h.get_by_role_and_label(Role::ComboBox, "Layout");
    assert_eq!(
        layout.value().as_deref(),
        Some("Default: {category}/{bundle}/")
    );
    layout.click();
    h.run_ok();
    click(&mut h, "ae2: {category}/");
    assert_eq!(
        h.get_by_role_and_label(Role::ComboBox, "Layout")
            .value()
            .as_deref(),
        Some("ae2: {category}/")
    );
    // The summary follows.
    h.get_by_label("Layout ae2 · skip existing");
    h.get_by_label("Atlases whole");

    // Saved a second after the last change.
    wait_for(&mut h, "the settings file", |_| path.exists());
    let file = ConfigFile::load(&path).unwrap();
    assert_eq!(file.settings.skip_existing, Some(true));
    assert_eq!(file.settings.atlas_cutting, Some(false));
    assert_eq!(file.settings.layout, Some(ae3_core::config::Layout::Ae2));
    let app = h.state().app();
    assert!(app.config.skip_existing && !app.config.atlas_cutting);
}

// ---- Game Entities ------------------------------------------------------------

#[test]
fn picking_a_game_entity_narrows_the_table_to_its_bundles() {
    use ae3_core::{EntityKind, GameEntity, GamedataVersions, Language, NameIndex};

    let dir = TempDir::new("entities");
    let cdn = FakeCdn::empty();
    cdn.publish_latest(&synthetic_manifest(SYNTHETIC));
    // A Name Index cached by an earlier session; the update finds no
    // gamedata (404) and keeps it.
    let entity = |kind, id: &str, name: &str, bundles: &[&str]| GameEntity {
        kind,
        id: id.into(),
        name: name.into(),
        context: None,
        bundles: bundles.iter().map(|b| (*b).to_owned()).collect(),
    };
    let names = NameIndex::new(
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
        ],
    );
    names
        .save(&ae3_core::names::default_name_index_path(
            &dir.path().join("cache"),
            Language::default(),
        ))
        .unwrap();
    let mut spec = Spec::new(dir.path(), cdn.net());
    spec.start = true;
    let mut h = harness(spec);
    wait_for(&mut h, "the Manifest and display names", |app| {
        app.model.asset_version() == Some(SYNTHETIC) && app.model.names.is_some()
    });

    type_into(&mut h, "Asset Search", "test");
    h.get_by_label_contains("2 Game Entities");
    let unit = h.get_by_label_contains("Test Pilot · 2");
    assert_eq!(unit.accesskit_node().toggled(), Some(Toggled::False));
    let label = unit.accesskit_node().label().unwrap();
    click(&mut h, &label);
    let model = &h.state().app().model;
    let mut visible: Vec<&str> = model
        .visible
        .iter()
        .map(|&id| model.rows[id].name.as_str())
        .collect();
    visible.sort_unstable();
    assert_eq!(visible, ["charui_testpilot", "vanity_testpilot"]);
    h.get_by_label("Showing the 2 Bundles of Unit 'Test Pilot'");

    // Clicking it again shows every match.
    click(&mut h, &label);
    assert!(h.state().app().model.visible.len() > 2);
}

// ---- Failures and notices -------------------------------------------------------

#[test]
fn a_network_failure_is_shown_as_an_error() {
    let dir = TempDir::new("offline");
    let cdn = FakeCdn::offline();
    let mut h = harness(Spec::new(dir.path(), cdn.net()));
    fetch_latest(&mut h);
    h.get_by_label_contains("could not determine the Asset Version automatically");
    assert!(h.state().app().model.manifest.is_none());
    h.get_by_label_contains("Choose a Platform and press Fetch latest");

    // An explicit Asset Version, entered with the Enter key, fails too.
    type_into(&mut h, "Asset Version", "100055");
    h.key_press(Key::Enter);
    h.run_ok();
    wait_for(&mut h, "the explicit load", |app| {
        !app.model.manifest_job.is_active()
    });
    h.get_by_label_contains("could not determine the Environment");
    assert!(cdn.requests_containing("env-list.ini") >= 2);
}

#[test]
fn a_newer_ae3_release_is_announced_and_can_be_dismissed() {
    use ae3_gui::model::release::{REPOSITORY, ReleaseState, latest_release_api};

    let dir = TempDir::new("release");
    let cdn = FakeCdn::empty();
    cdn.publish_latest(&synthetic_manifest(SYNTHETIC));
    cdn.put(
        &latest_release_api(REPOSITORY).unwrap(),
        format!(r#"{{"tag_name":"v99.0.0","html_url":"{REPOSITORY}/releases/tag/v99.0.0"}}"#)
            .as_bytes(),
    );
    let mut spec = Spec::new(dir.path(), cdn.net());
    spec.config.check_for_updates = true;
    spec.start = true;
    let mut h = harness(spec);
    wait_for(&mut h, "the release check", |app| {
        app.model.release.is_some()
    });

    let link = h.get_by_label("swgoh-ae3 v99.0.0 available");
    assert_eq!(link.accesskit_node().role(), Role::Link);
    click(&mut h, "Dismiss release notice");
    assert!(h.query_by_label("swgoh-ae3 v99.0.0 available").is_none());
    let saved = dir.path().join("cache").join(ReleaseState::FILE);
    wait_for(&mut h, "the dismissal to be saved", |_| {
        std::fs::read_to_string(&saved)
            .is_ok_and(|s| ReleaseState::parse(&s).dismissed.as_deref() == Some("v99.0.0"))
    });
}

/// The macOS menu bar's "About swgoh-ae3" opens the app's About window, not
/// the system's standard panel.
#[test]
fn the_menu_bar_about_item_opens_the_about_window() {
    use ae3_gui::menu::Command;

    let dir = TempDir::new("menu-about");
    let mut h = harness(Spec::new(dir.path(), FakeCdn::offline().net()));
    assert!(h.query_by_label("Check for updates").is_none());
    h.state().app().menu_inbox().push(Command::About);
    // As eframe hands each frame's input to the app before running it.
    let mut input = eframe::egui::RawInput::default();
    h.state_mut().app_mut().take_menu_commands(&mut input);
    h.run_ok();
    h.get_by_label("About swgoh-ae3");
    h.get_by_label("Check for updates");
}

#[test]
fn about_shows_the_version_and_checks_for_updates_on_request() {
    use ae3_gui::model::release::{CURRENT, ManualCheck, REPOSITORY, latest_release_api};

    let dir = TempDir::new("about");
    let cdn = FakeCdn::empty();
    cdn.publish_latest(&synthetic_manifest(SYNTHETIC));
    // The daily check is off: only the button may ask GitHub.
    let mut spec = Spec::new(dir.path(), cdn.net());
    spec.config.check_for_updates = false;
    let mut h = harness(spec);
    let asks = || cdn.requests_containing("api.github.com");

    click(&mut h, "About");
    h.get_by_label("About swgoh-ae3");
    h.get_by_label(&format!("Version {CURRENT}"));
    assert_eq!(asks(), 0, "opening About asks nothing");

    let check = |h: &mut H, expected: &dyn Fn(&ManualCheck) -> bool| {
        click(h, "Check for updates");
        wait_for(h, "the update check", |app| {
            expected(&app.model.manual_check)
        });
    };

    // Nothing published yet: GitHub answers 404.
    check(&mut h, &|c| *c == ManualCheck::NotPublished);
    h.get_by_label("No release has been published yet.");

    // A newer release: a link to it.
    let api = latest_release_api(REPOSITORY).unwrap();
    let publish = |tag: &str| {
        cdn.put(
            &api,
            format!(r#"{{"tag_name":"{tag}","html_url":"{REPOSITORY}/releases/tag/{tag}"}}"#)
                .as_bytes(),
        );
    };
    publish("v99.0.0");
    check(&mut h, &|c| matches!(c, ManualCheck::Newer { .. }));
    let link = h.get_by_label("swgoh-ae3 v99.0.0 is available");
    assert_eq!(link.accesskit_node().role(), Role::Link);

    // The running version is the newest.
    publish(&format!("v{CURRENT}"));
    check(&mut h, &|c| *c == ManualCheck::UpToDate);
    h.get_by_label(&format!("swgoh-ae3 {CURRENT} is the latest release."));
    assert_eq!(asks(), 3, "each click asks once, ignoring the daily limit");

    // The window closes with its title-bar button.
    h.state_mut().app_mut().model.windows.about = false;
    h.run_ok();
    assert!(h.query_by_label("Check for updates").is_none());
}
