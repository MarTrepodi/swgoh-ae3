//! Asset access and export against the real Asset Version 100054 fixtures,
//! served by an offline Transport. Skips when the fixtures are absent.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ae3_core::config::{AudioFormat, ImageFormat, Layout};
use ae3_core::http::{HttpError, RangeResponse, Transport};
use ae3_core::{
    AssetKind, AssetVersion, CdnLocation, Downloader, Environment, ExportEvent, ExportOptions,
    ExportReport, LoadedBundle, Manifest, PackageCache, PackageStore, Platform, Selection,
    SkipReason, VersionSource, export_bundle,
};

const BASE_URL: &str = "https://cdn.test/PROD/";

fn fixtures_dir() -> PathBuf {
    std::env::var_os("AE3_FIXTURES")
        .map_or_else(
            || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            PathBuf::from,
        )
        .join("100054")
        .join("Windows")
}

/// Serves `<base>/100054/Windows/ETC/<file>` from the fixtures directory.
struct FixtureTransport {
    dir: PathBuf,
    requests: Mutex<Vec<String>>,
}

impl FixtureTransport {
    fn read(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.requests.lock().unwrap().push(url.to_owned());
        let name = url.rsplit('/').next().unwrap_or_default();
        fs::read(self.dir.join(name)).map_err(|_| HttpError::Status {
            url: url.to_owned(),
            status: 404,
        })
    }

    fn requested(&self, file: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|u| u.ends_with(&format!("/{file}")))
            .count()
    }
}

impl Transport for FixtureTransport {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.read(url)
    }

    fn get_range(
        &self,
        url: &str,
        start: u64,
        end_inclusive: u64,
    ) -> Result<RangeResponse, HttpError> {
        let body = self.read(url)?;
        let len = body.len() as u64;
        let start = usize::try_from(start.min(len)).unwrap();
        let end = usize::try_from(end_inclusive.saturating_add(1).min(len)).unwrap();
        Ok(RangeResponse {
            bytes: body[start..end].to_vec(),
            total_len: Some(len),
        })
    }

    fn exists(&self, url: &str) -> Result<bool, HttpError> {
        Ok(self.read(url).is_ok())
    }
}

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ae3-export-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    store: PackageStore<Arc<FixtureTransport>>,
    transport: Arc<FixtureTransport>,
    _cache_dir: TempDir,
    out: TempDir,
}

impl Fixture {
    fn options(&self) -> ExportOptions {
        ExportOptions {
            parallel: 2,
            ..ExportOptions::new(self.out.path())
        }
    }

    fn out(&self, rel: &str) -> PathBuf {
        self.out.path().join(rel)
    }
}

/// The store over the fixtures, or `None` (skip) when they are absent.
fn fixture(name: &str, needed: &[&str]) -> Option<Fixture> {
    let dir = fixtures_dir();
    let missing: Vec<&&str> = std::iter::once(&"manifest.data")
        .chain(needed)
        .filter(|f| !dir.join(f).exists())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "skipping: fixtures {missing:?} not found in {}",
            dir.display()
        );
        return None;
    }
    let manifest = Manifest::decode(&fs::read(dir.join("manifest.data")).unwrap()).unwrap();
    let transport = Arc::new(FixtureTransport {
        dir,
        requests: Mutex::default(),
    });
    let cache_dir = TempDir::new(&format!("{name}-cache"));
    let cache = PackageCache::open(
        cache_dir.path().to_path_buf(),
        u64::MAX,
        Downloader::new(Arc::clone(&transport)),
    )
    .unwrap();
    let location = CdnLocation {
        asset_version: AssetVersion(100_054),
        environment: Environment::new("PROD").unwrap(),
        base_url: BASE_URL.to_owned(),
        source: VersionSource::Explicit,
    };
    Some(Fixture {
        store: PackageStore::new(Arc::new(cache), location, Platform::Windows, manifest),
        transport,
        _cache_dir: cache_dir,
        out: TempDir::new(&format!("{name}-out")),
    })
}

fn export(f: &Fixture, selection: &Selection, options: &ExportOptions) -> ExportReport {
    let report = f
        .store
        .export(selection, options, &|_| {}, &AtomicBool::new(false))
        .unwrap();
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    report
}

fn packages(names: &[&str]) -> Selection {
    Selection::Packages(names.iter().map(|&n| n.to_owned()).collect())
}

/// Width and height from a PNG's IHDR chunk.
fn png_size(path: &Path) -> (u32, u32) {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{}", path.display());
    let be = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    (be(16), be(20))
}

#[test]
fn exports_a_portrait_as_png() {
    let Some(f) = fixture("portrait", &["charui_bossk.bundle"]) else {
        return;
    };
    let report = export(&f, &packages(&["charui_bossk"]), &f.options());
    let png = f.out("charui/charui_bossk/tex.charui_bossk.png");
    assert_eq!(png_size(&png), (128, 128));
    assert_eq!(report.written.len(), 1);
    assert_eq!(report.written[0].path, png);
    assert_eq!(report.written[0].bytes, fs::metadata(&png).unwrap().len());
    // The AssetBundle object is plumbing, not output.
    assert_eq!(report.ignored, 1);
    assert_eq!(report.packages_done, 1);
    assert!(report.is_success());
}

#[test]
fn lists_assets_with_metadata_and_decodes_lazily() {
    let Some(f) = fixture(
        "listing",
        &["charui_bossk.bundle", "unit_details_atlas.bundle"],
    ) else {
        return;
    };
    let bundle = f.store.open_bundle("charui_bossk").unwrap();
    assert_eq!(bundle.category(), "charui");
    let assets = bundle.assets().unwrap();
    let texture = assets
        .iter()
        .find(|a| a.kind == AssetKind::Texture)
        .unwrap();
    assert_eq!(texture.name, "tex.charui_bossk");
    let info = texture.texture.unwrap();
    assert_eq!((info.width, info.height, info.format_id), (128, 128, 10));
    assert_eq!(info.format.unwrap().name(), "DXT1");
    // 8 KiB of DXT1 data streamed from the .resS node.
    assert_eq!(texture.byte_size, u64::from(216u32) + 8192);
    assert!(
        texture
            .container_path
            .as_deref()
            .unwrap()
            .contains("charui_bossk")
    );

    let image = bundle.decode_texture(texture.path_id).unwrap();
    assert_eq!((image.width(), image.height()), (128, 128));
    let thumb = bundle.thumbnail(texture.path_id, 32).unwrap();
    assert_eq!((thumb.width(), thumb.height()), (32, 32));
    let json = bundle.structured_json(texture.path_id).unwrap();
    assert!(json.contains("\"m_Width\": 128"));
    let wrong = bundle.atlas(texture.path_id).unwrap_err();
    assert!(
        wrong
            .to_string()
            .contains("is of kind texture, expected atlas"),
        "{wrong}"
    );

    let atlas_bundle = f.store.open_bundle("unit_details_atlas").unwrap();
    let assets = atlas_bundle.assets().unwrap();
    let atlas = assets.iter().find(|a| a.kind == AssetKind::Atlas).unwrap();
    assert_eq!(atlas.script_class.as_deref(), Some("NGUIAtlas"));
    assert_eq!(atlas.sprite_count, Some(39));
    let atlas_thumb = atlas_bundle.thumbnail(atlas.path_id, 100).unwrap();
    assert_eq!((atlas_thumb.width(), atlas_thumb.height()), (100, 100));
}

#[test]
fn cuts_an_atlas_into_its_sprites() {
    let needed = ["unit_details_atlas.bundle", "shared_uicontainer.bundle"];
    let Some(f) = fixture("atlas", &needed) else {
        return;
    };
    let bundle = f.store.open_bundle("unit_details_atlas").unwrap();
    // The direct Manifest dependency is fetched up front.
    assert_eq!(bundle.dependencies(), ["shared_uicontainer"]);
    assert_eq!(f.transport.requested("shared_uicontainer.bundle"), 1);
    let atlas_id = bundle
        .assets()
        .unwrap()
        .iter()
        .find(|a| a.kind == AssetKind::Atlas)
        .unwrap()
        .path_id;
    let atlas = bundle.atlas(atlas_id).unwrap();
    assert_eq!(atlas.name, "unit_details_atlas");
    assert_eq!(atlas.sprites.len(), 39);
    let texture = atlas.texture.as_ref().expect("atlas texture");
    assert_eq!(texture.name, "unit_details_atlas");
    assert_eq!((texture.info.width, texture.info.height), (1024, 1024));
    assert_eq!(texture.asset.package, "unit_details_atlas");
    let first = &atlas.sprites[0];
    assert_eq!(
        (
            first.name.as_str(),
            first.x,
            first.y,
            first.width,
            first.height
        ),
        ("active_prereq_default", 428, 540, 114, 114)
    );

    let report = export(&f, &packages(&["unit_details_atlas"]), &f.options());
    let sprites_dir = f.out("unit/unit_details_atlas/sprites/unit_details_atlas");
    let mut seen = HashSet::new();
    for sprite in &atlas.sprites {
        let path = sprites_dir.join(format!("{}.png", sprite.name));
        assert_eq!(png_size(&path), sprite.restored_size(), "{}", sprite.name);
        assert!(seen.insert(sprite.name.to_lowercase()));
    }
    let pngs = fs::read_dir(&sprites_dir).unwrap().count();
    assert_eq!(pngs, 39);
    let json: serde_json::Value = serde_json::from_slice(
        &fs::read(f.out("unit/unit_details_atlas/sprites/unit_details_atlas.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(json["sprites"].as_array().unwrap().len(), 39);
    assert_eq!(
        json["sprites"][0]["file"],
        "unit_details_atlas/active_prereq_default.png"
    );
    assert_eq!(json["texture"]["width"], 1024);
    // The atlas texture itself, the Material as JSON, the Sprites and the
    // Atlas JSON.
    assert!(
        f.out("unit/unit_details_atlas/unit_details_atlas.png")
            .exists()
    );
    assert!(
        f.out("unit/unit_details_atlas/unit_details_atlas.json")
            .exists()
    );
    assert_eq!(report.written.len(), 39 + 3);
}

#[test]
fn ae2_layout_drops_the_bundle_directory() {
    let Some(f) = fixture(
        "ae2",
        &[
            "charui_bossk.bundle",
            "unit_details_atlas.bundle",
            "shared_uicontainer.bundle",
        ],
    ) else {
        return;
    };
    let options = ExportOptions {
        layout: Layout::Ae2,
        ..f.options()
    };
    export(
        &f,
        &packages(&["charui_bossk", "unit_details_atlas"]),
        &options,
    );
    assert_eq!(png_size(&f.out("charui/tex.charui_bossk.png")), (128, 128));
    assert!(!f.out("charui/charui_bossk").exists());
    let sprite = f.out("unit/sprites/unit_details_atlas/active_prereq_default.png");
    assert_eq!(png_size(&sprite), (114, 114));
    assert!(f.out("unit/sprites/unit_details_atlas.json").exists());
}

#[test]
fn colliding_names_get_a_path_id_suffix() {
    let needed = ["unit_details_atlas.bundle", "shared_uicontainer.bundle"];
    let Some(f) = fixture("collide", &needed) else {
        return;
    };
    // Without Atlas Cutting both the Atlas and its Material, each named
    // unit_details_atlas, are written as JSON.
    let options = ExportOptions {
        atlas_cutting: false,
        ..f.options()
    };
    let report = export(&f, &packages(&["unit_details_atlas"]), &options);
    let dir = f.out("unit/unit_details_atlas");
    let atlas: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("unit_details_atlas.json")).unwrap()).unwrap();
    assert!(
        atlas.get("mSprites").is_some(),
        "the Atlas keeps the plain name"
    );
    let material = dir.join("unit_details_atlas_-8126108850209264937.json");
    let material: serde_json::Value = serde_json::from_slice(&fs::read(material).unwrap()).unwrap();
    assert!(material.get("m_SavedProperties").is_some());
    assert!(!dir.join("sprites").exists());
    // Deterministic: a second run writes the same paths.
    let again = export(&f, &packages(&["unit_details_atlas"]), &options);
    let paths = |r: &ExportReport| r.written.iter().map(|w| w.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&report), paths(&again));
}

#[test]
fn skip_existing_leaves_files_alone() {
    let Some(f) = fixture("skip", &["charui_bossk.bundle", "relic_rfl_bossk.bundle"]) else {
        return;
    };
    let selection = packages(&["charui_bossk", "relic_rfl_bossk"]);
    let first = export(&f, &selection, &f.options());
    assert_eq!(first.written.len(), 2);
    let options = ExportOptions {
        skip_existing: true,
        ..f.options()
    };
    let second = export(&f, &selection, &options);
    assert_eq!(second.written.len(), 0);
    assert_eq!(second.skipped.len(), 2);
    assert!(
        second
            .skipped
            .iter()
            .all(|s| matches!(&s.reason, SkipReason::Exists { path } if path.exists()))
    );
    // Without skip-existing the files are overwritten.
    let third = export(&f, &selection, &f.options());
    assert_eq!(third.written.len(), 2);
}

#[test]
fn cancellation_stops_the_batch() {
    let names = [
        "charui_bossk",
        "relic_rfl_bossk",
        "vanity_bossk",
        "abilityui_passive_summon",
    ];
    let files: Vec<String> = names.iter().map(|n| format!("{n}.bundle")).collect();
    let needed: Vec<&str> = files.iter().map(String::as_str).collect();
    let Some(f) = fixture("cancel", &needed) else {
        return;
    };
    let cancel = AtomicBool::new(false);
    let finished = AtomicUsize::new(0);
    let progress = |p: &ae3_core::ExportProgress<'_>| {
        assert_eq!(p.packages_total, 4);
        if let ExportEvent::PackageFinished { .. } = p.event {
            finished.fetch_add(1, Ordering::Relaxed);
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let options = ExportOptions {
        parallel: 1,
        ..f.options()
    };
    let report = f
        .store
        .export(&packages(&names), &options, &progress, &cancel)
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.packages_done, 1);
    assert_eq!(finished.load(Ordering::Relaxed), 1);
    assert_eq!(report.written.len(), 1);
    // The remaining Packages were never even fetched.
    assert_eq!(f.transport.requested("abilityui_passive_summon.bundle"), 0);

    // Cancelled before it starts: nothing at all.
    let report = f
        .store
        .export(&packages(&names), &options, &|_| {}, &AtomicBool::new(true))
        .unwrap();
    assert!(report.cancelled && report.written.is_empty() && report.packages_done == 0);
}

#[test]
fn exports_assets_by_path_id_and_reports_progress() {
    let Some(f) = fixture("byid", &["char_bossk_pre.bundle"]) else {
        return;
    };
    let bundle = f.store.open_bundle("char_bossk_pre").unwrap();
    let assets = bundle.assets().unwrap();
    let meshes: Vec<i64> = assets
        .iter()
        .filter(|a| a.kind == AssetKind::Mesh)
        .map(|a| a.path_id)
        .collect();
    assert_eq!(meshes.len(), 3);
    let texture = assets.iter().find(|a| a.name == "rfl_bossk_tex").unwrap();
    assert_eq!(texture.kind, AssetKind::Texture);
    let transform = assets.iter().find(|a| a.class_name == "Transform").unwrap();
    assert_eq!(transform.kind, AssetKind::Other);

    let mut ids = meshes.clone();
    ids.extend([texture.path_id, transform.path_id, 12_345]);
    let events = Mutex::new(Vec::new());
    let report = f
        .store
        .export(
            &Selection::Assets {
                package: "char_bossk_pre".into(),
                path_ids: ids,
            },
            &f.options(),
            &|p| {
                if let ExportEvent::AssetFinished { done, total, .. } = p.event {
                    events.lock().unwrap().push((done, total));
                }
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    // Meshes are listed but skipped with a reason.
    assert_eq!(report.skipped.len(), 3);
    assert!(report.skipped.iter().all(|s| matches!(
        &s.reason,
        SkipReason::Unsupported { detail } if detail.contains("mesh")
    )));
    // An unknown path id is an error for that Asset only.
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].path_id, Some(12_345));
    // The texture as an image, and the Transform (asked for by id) as JSON.
    assert_eq!(report.written.len(), 2);
    assert!(f.out("char/char_bossk_pre/rfl_bossk_tex.png").exists());
    let transform_json = f.out(&format!(
        "char/char_bossk_pre/Transform_{}.json",
        transform.path_id
    ));
    assert!(transform_json.exists());
    let events = events.into_inner().unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(events.last(), Some(&(5, 5)));
}

#[test]
fn resolves_references_into_dependency_bundles() {
    let needed = [
        "territoryui_view_standard_atlas.bundle",
        "shared_uicontainer.bundle",
    ];
    let Some(f) = fixture("deps", &needed) else {
        return;
    };
    let bundle = f
        .store
        .open_bundle("territoryui_view_standard_atlas")
        .unwrap();
    let atlas_id = bundle
        .assets()
        .unwrap()
        .iter()
        .find(|a| a.kind == AssetKind::Atlas)
        .unwrap()
        .path_id;
    let atlas = bundle.atlas(atlas_id).unwrap();
    assert_ne!(atlas.sprites.len(), 0);
    assert!(atlas.texture.is_some(), "{:?}", atlas.texture_error);

    // A texture in the dependency, decoded through this Bundle.
    let shared = f.store.open_bundle("shared_uicontainer").unwrap();
    let shared_assets = shared.assets().unwrap();
    let misc = shared_assets
        .iter()
        .find(|a| a.kind == AssetKind::Texture && a.name == "misc_atlas")
        .unwrap();
    let reference = ae3_core::AssetRef {
        package: "shared_uicontainer".into(),
        file: shared
            .bundle()
            .serialized_nodes()
            .next()
            .unwrap()
            .path
            .clone(),
        path_id: misc.path_id,
    };
    let image = bundle.decode_texture_ref(&reference).unwrap();
    let info = misc.texture.unwrap();
    assert_eq!((image.width(), image.height()), (info.width, info.height));

    // Every Atlas of shared_uicontainer resolves, reference Atlases too.
    let atlases: Vec<_> = shared_assets
        .iter()
        .filter(|a| a.kind == AssetKind::Atlas)
        .collect();
    assert_eq!(atlases.len(), 14);
    for a in atlases {
        let atlas = shared.atlas(a.path_id).unwrap();
        assert!(!atlas.sprites.is_empty(), "{}", a.name);
        assert!(
            atlas.texture.is_some(),
            "{}: {:?}",
            a.name,
            atlas.texture_error
        );
        assert_eq!(
            atlas.replacement.is_some(),
            a.sprite_count == Some(0),
            "{}",
            a.name
        );
    }
}

#[test]
fn opens_local_bundle_files_with_sibling_dependencies() {
    let dir = fixtures_dir();
    let path = dir.join("unit_details_atlas.bundle");
    if !path.exists() || !dir.join("shared_uicontainer.bundle").exists() {
        eprintln!("skipping: fixtures not found in {}", dir.display());
        return;
    }
    let bundle = LoadedBundle::open_file(&path).unwrap();
    assert_eq!(bundle.name(), "unit_details_atlas");
    assert_eq!(bundle.dependencies(), ["shared_uicontainer"]);
    let out = TempDir::new("local");
    let options = ExportOptions {
        image_format: ImageFormat::Webp,
        padding_restoration: false,
        ..ExportOptions::new(out.path())
    };
    let report = export_bundle(&bundle, None, &options, &|_| {}, &AtomicBool::new(false));
    assert!(report.is_success(), "{:#?}", report.errors);
    let webp = out
        .path()
        .join("unit/unit_details_atlas/sprites/unit_details_atlas/active_prereq_default.webp");
    let bytes = fs::read(webp).unwrap();
    assert_eq!((&bytes[..4], &bytes[8..12]), (&b"RIFF"[..], &b"WEBP"[..]));
}

#[test]
fn exports_audio_packages() {
    let Some(f) = fixture("audio", &["audio_vo_placeholder.wwpkg"]) else {
        return;
    };
    let audio = f.store.open_audio_package("audio_vo_placeholder").unwrap();
    assert_eq!(audio.category(), "audio");
    let sounds = audio.sound_infos();

    let report = export(&f, &packages(&["audio_vo_placeholder.wwpkg"]), &f.options());
    let dir = f.out("audio/audio_vo_placeholder");
    assert_eq!(report.written.len(), 1);
    assert_eq!(sounds.len(), 1);
    let ogg = &report.written[0].path;
    assert_eq!(ogg, &dir.join(format!("{}.ogg", sounds[0].stem)));
    assert_eq!(&fs::read(ogg).unwrap()[..4], b"OggS");

    // Skip-existing predicts the converted extension without converting.
    let skip = ExportOptions {
        skip_existing: true,
        ..f.options()
    };
    let report = export(&f, &packages(&["audio_vo_placeholder"]), &skip);
    assert_eq!(report.written.len(), 0);
    assert_eq!(report.skipped.len(), 1);

    let raw = ExportOptions {
        audio_format: AudioFormat::Raw,
        ..f.options()
    };
    let report = export(
        &f,
        &Selection::Sounds {
            package: "audio_vo_placeholder".into(),
            ids: vec![sounds[0].id],
        },
        &raw,
    );
    assert_eq!(report.written.len(), 1);
    assert_eq!(report.written[0].path.extension().unwrap(), "wem");
}

#[test]
fn unknown_packages_are_rejected_up_front() {
    let Some(f) = fixture("unknown", &[]) else {
        return;
    };
    let err = f
        .store
        .export(
            &packages(&["no_such_bundle"]),
            &f.options(),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(matches!(err, ae3_core::AssetError::PackageNotFound(_)));
    assert!(f.store.open_audio_package("charui_bossk").is_err());
    assert!(f.store.package("charui_bossk.bundle").is_some());
    let category = f
        .store
        .select(&Selection::Category("charui".into()))
        .unwrap();
    assert!(category.iter().all(|p| p.category() == "charui"));
    assert!(category.len() > 100);
}
