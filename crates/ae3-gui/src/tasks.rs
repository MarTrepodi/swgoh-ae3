//! The work done off the UI thread, and the messages it sends back.
//!
//! Each function here runs on a pool or job thread, calls `ae3-core`, and
//! reports through an [`Outbox`]. Stale work is skipped early: tasks carry
//! the generation they were started for and compare it with a shared
//! counter before each expensive step.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use ae3_core::ae3_image::{self, RgbaImage};
use ae3_core::http::UreqTransport;
use ae3_core::{
    AssetIndex, AssetInfo, AssetKind, AssetKinds, AssetVersion, Atlas, CdnLocation, Config, Diff,
    ExportOptions, IndexBuilder, IndexProgress, LoadedBundle, PackageCache, PackageKind,
    PackageStore, Platform, Resolver, SoundInfo, Value,
};
use eframe::egui::ColorImage;

use crate::model::IndexProgressView;
use crate::model::extract::{ExtractProgress, ExtractRequest, ExtractSummary};
use crate::model::jobs::{Generation, Throttle};
use crate::model::preview::THUMBNAIL_PX;
use crate::model::table::SearchMatches;
use crate::model::viewer::Channels;
use crate::worker::Outbox;

pub type Cache = PackageCache<UreqTransport>;
pub type Store = PackageStore<UreqTransport>;

/// How often progress messages may wake the UI.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Structured Assets larger than this are summarised instead of read into
/// a tree, which costs about 32 bytes per array element.
pub const MAX_TREE_BYTES: u64 = 8 << 20;
/// Text Assets longer than this are shown truncated.
pub const MAX_TEXT_BYTES: usize = 1 << 20;

/// What background work needs that lives for the whole session.
pub struct Services {
    /// The configuration the session started with (cache settings).
    pub config: Config,
    cache: OnceLock<Result<Arc<Cache>, String>>,
}

impl Services {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            cache: OnceLock::new(),
        }
    }

    /// The download cache, opened on first use (it reads its directory).
    pub fn cache(&self) -> Result<Arc<Cache>, String> {
        self.cache
            .get_or_init(|| {
                PackageCache::from_config(&self.config, UreqTransport::new())
                    .map(Arc::new)
                    .map_err(|e| format!("cannot open the download cache: {e}"))
            })
            .clone()
    }
}

/// A preview Package, opened.
#[derive(Clone)]
pub enum Opened {
    Bundle {
        bundle: Arc<LoadedBundle>,
        assets: Vec<AssetInfo>,
        kinds: AssetKinds,
    },
    Audio {
        sounds: Vec<SoundInfo>,
    },
}

/// What the detail viewer shows for one Asset.
#[expect(
    clippy::large_enum_variant,
    reason = "one value at a time, moved once from the worker"
)]
pub enum DetailContent {
    Image(DetailImage),
    Atlas {
        atlas: Atlas,
        image: Option<DetailImage>,
        error: Option<String>,
    },
    Tree(Value),
    Text {
        text: String,
        extension: &'static str,
        truncated: bool,
    },
    /// Nothing to show beyond a description.
    Info(String),
}

/// A decoded image ready to upload, with the full pixels kept for
/// channel masking.
pub struct DetailImage {
    /// What is uploaded: the decode, downscaled when the GPU cannot hold
    /// it at full size.
    pub shown: Arc<RgbaImage>,
    pub color: ColorImage,
    /// The decoded size, which may be larger than `shown`.
    pub width: u32,
    pub height: u32,
}

pub struct IndexOutcome {
    pub index: Arc<AssetIndex>,
    pub cancelled: bool,
    pub save_error: Option<String>,
}

/// Messages to the UI thread.
#[expect(
    clippy::large_enum_variant,
    reason = "a handful in flight; boxing would only add allocations"
)]
pub enum Msg {
    ManifestLoaded {
        generation: Generation,
        result: Result<Store, String>,
    },
    UpdateChecked {
        latest: Option<AssetVersion>,
    },
    IndexSnapshot {
        generation: Generation,
        index: Arc<AssetIndex>,
    },
    IndexProgress {
        generation: Generation,
        view: IndexProgressView,
        paused: bool,
    },
    IndexDone {
        generation: Generation,
        result: Result<IndexOutcome, String>,
    },
    SearchDone {
        matches: SearchMatches,
    },
    PreviewStatus {
        generation: Generation,
        text: String,
    },
    PreviewOpened {
        generation: Generation,
        result: Result<Opened, String>,
    },
    Thumbnail {
        generation: Generation,
        package: String,
        path_id: i64,
        result: Result<ColorImage, String>,
    },
    Detail {
        generation: Generation,
        result: Result<DetailContent, String>,
    },
    DetailMasked {
        generation: Generation,
        color: ColorImage,
    },
    ExtractProgress {
        generation: Generation,
        progress: ExtractProgress,
    },
    ExtractDone {
        generation: Generation,
        summary: ExtractSummary,
    },
    DiffDone {
        generation: Generation,
        result: Result<(Arc<Diff>, Store), String>,
    },
    ConfigSaved(Result<(), String>),
}

pub type Mailbox = Outbox<Msg>;

fn color_image(image: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.pixels(),
    )
}

/// Resolves the Asset Version (the latest when `explicit` is `None`) and
/// opens a store over its Manifest.
pub fn load_manifest(
    services: &Services,
    platform: Platform,
    explicit: Option<AssetVersion>,
) -> Result<Store, String> {
    let cache = services.cache()?;
    let location = Resolver::new(
        cache.downloader().transport(),
        platform,
        services.config.effective_cache_dir(),
    )
    .resolve(explicit)
    .map_err(|e| e.to_string())?;
    Store::open(cache, location, platform).map_err(|e| e.to_string())
}

/// The latest Asset Version, or `None` when it cannot be found right now.
pub fn latest_version(services: &Services, platform: Platform) -> Option<AssetVersion> {
    let cache = services.cache().ok()?;
    Resolver::new(
        cache.downloader().transport(),
        platform,
        services.config.effective_cache_dir(),
    )
    .resolve(None)
    .ok()
    .map(|l| l.asset_version)
}

/// Builds the Asset Index for the store's Manifest, sending the stored
/// index first, progress while probing, a fresh snapshot after every save,
/// and the final index.
pub fn build_index(
    store: &Store,
    generation: Generation,
    pause: Arc<AtomicBool>,
    cancel: &AtomicBool,
    outbox: &Mailbox,
) {
    let builder = IndexBuilder::for_store(store).with_pause_flag(pause);
    if let Ok(index) = builder.index() {
        outbox.send(Msg::IndexSnapshot {
            generation,
            index: Arc::new(index),
        });
    }
    let save_every = builder.options().save_every.max(1);
    let state = Mutex::new((Throttle::new(PROGRESS_INTERVAL), false, 0usize));
    let progress = |p: &IndexProgress<'_>| {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        let (throttle, was_paused, last_bucket) = &mut *guard;
        let pause_changed = p.paused != *was_paused;
        *was_paused = p.paused;
        let bucket = p.done / save_every;
        // The builder saved just before reporting this Bundle (or saved on
        // pause): publish what search can now see.
        if bucket > *last_bucket || (pause_changed && p.paused) {
            *last_bucket = bucket;
            if let Ok(index) = builder.index() {
                outbox.send(Msg::IndexSnapshot {
                    generation,
                    index: Arc::new(index),
                });
            }
        }
        if pause_changed || p.done == p.total || throttle.ready(Instant::now()) {
            outbox.send(Msg::IndexProgress {
                generation,
                view: IndexProgressView {
                    done: p.done,
                    total: p.total,
                    failed: p.failed,
                    reused: p.reused,
                    bytes_fetched: p.bytes_fetched,
                },
                paused: p.paused,
            });
        }
    };
    let result = builder
        .run(&progress, cancel)
        .map(|run| IndexOutcome {
            index: Arc::new(run.index),
            cancelled: run.report.cancelled,
            save_error: run.report.save_error,
        })
        .map_err(|e| e.to_string());
    outbox.send(Msg::IndexDone { generation, result });
}

/// Opens the preview Package, unless the preview moved on meanwhile.
pub fn open_package(
    store: &Store,
    name: &str,
    kind: PackageKind,
    generation: Generation,
    current: &AtomicU64,
    outbox: &Mailbox,
) {
    if current.load(Ordering::Relaxed) != generation {
        return;
    }
    let result = match kind {
        PackageKind::Bundle => {
            let throttle = Mutex::new(Throttle::new(PROGRESS_INTERVAL));
            let progress = |p: &ae3_core::Progress| {
                let ready = throttle
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .ready(Instant::now());
                if ready && p.bytes_total > 0 && p.bytes_done < p.bytes_total {
                    outbox.send(Msg::PreviewStatus {
                        generation,
                        text: format!(
                            "Downloading {} of {}…",
                            crate::model::table::format_bytes(p.bytes_done),
                            crate::model::table::format_bytes(p.bytes_total)
                        ),
                    });
                }
            };
            store
                .open_bundle_with_progress(name, &progress)
                .and_then(|bundle| {
                    let assets = bundle.view()?.assets();
                    let kinds = assets.iter().map(|a| a.kind).collect();
                    Ok(Opened::Bundle {
                        bundle: Arc::new(bundle),
                        assets,
                        kinds,
                    })
                })
                .map_err(|e| e.to_string())
        }
        PackageKind::AudioPackage => store
            .open_audio_package(name)
            .map(|package| Opened::Audio {
                sounds: package.sound_infos(),
            })
            .map_err(|e| e.to_string()),
        PackageKind::Other(code) => Err(format!("{name} is a Package of unknown kind {code}")),
    };
    outbox.send(Msg::PreviewOpened { generation, result });
}

/// Decodes thumbnails of `path_ids`, stopping when the preview changes.
pub fn thumbnails(
    bundle: &LoadedBundle,
    path_ids: &[i64],
    generation: Generation,
    current: &AtomicU64,
    outbox: &Mailbox,
) {
    let view = match bundle.view() {
        Ok(view) => view,
        Err(e) => {
            for &path_id in path_ids {
                outbox.send(Msg::Thumbnail {
                    generation,
                    package: bundle.name().to_owned(),
                    path_id,
                    result: Err(e.to_string()),
                });
            }
            return;
        }
    };
    for &path_id in path_ids {
        if current.load(Ordering::Relaxed) != generation {
            return;
        }
        let result = view
            .thumbnail(path_id, THUMBNAIL_PX)
            .map(|image| color_image(&image))
            .map_err(|e| e.to_string());
        outbox.send(Msg::Thumbnail {
            generation,
            package: bundle.name().to_owned(),
            path_id,
            result,
        });
    }
}

fn detail_image(image: RgbaImage, max_side: u32) -> DetailImage {
    let (width, height) = (image.width(), image.height());
    let shown = if width.max(height) > max_side {
        ae3_image::thumbnail(&image, max_side)
    } else {
        image
    };
    DetailImage {
        color: color_image(&shown),
        shown: Arc::new(shown),
        width,
        height,
    }
}

/// Reads one Asset for the detail viewer. `max_side` is the GPU's largest
/// texture side.
pub fn detail(
    bundle: &LoadedBundle,
    asset: &AssetInfo,
    max_side: u32,
) -> Result<DetailContent, String> {
    let view = bundle.view().map_err(|e| e.to_string())?;
    let id = asset.path_id;
    let err = |e: ae3_core::AssetError| e.to_string();
    Ok(match asset.kind {
        AssetKind::Texture => DetailContent::Image(detail_image(
            view.decode_texture(id).map_err(err)?,
            max_side,
        )),
        AssetKind::Sprite => {
            DetailContent::Image(detail_image(view.sprite_image(id).map_err(err)?, max_side))
        }
        AssetKind::Atlas => {
            let atlas = view.atlas(id).map_err(err)?;
            let (image, error) = match view.atlas_texture(&atlas) {
                Ok(image) => (Some(detail_image(image, max_side)), None),
                Err(e) => (
                    None,
                    Some(atlas.texture_error.clone().unwrap_or_else(|| e.to_string())),
                ),
            };
            DetailContent::Atlas {
                atlas,
                image,
                error,
            }
        }
        AssetKind::Text => {
            let text = view.text(id).map_err(err)?;
            let truncated = text.bytes.len() > MAX_TEXT_BYTES;
            let bytes = &text.bytes[..text.bytes.len().min(MAX_TEXT_BYTES)];
            DetailContent::Text {
                text: String::from_utf8_lossy(bytes).into_owned(),
                extension: text.extension,
                truncated,
            }
        }
        AssetKind::Audio => {
            let clip = view.audio_clip(id).map_err(err)?;
            DetailContent::Info(format!(
                "AudioClip, {} of {} data. Playback is not available yet; extract it to listen.",
                crate::model::table::format_bytes(clip.bytes.len() as u64),
                clip.extension
            ))
        }
        AssetKind::Structured | AssetKind::Mesh | AssetKind::Other => {
            if asset.byte_size > MAX_TREE_BYTES {
                DetailContent::Info(format!(
                    "{} is {}, too large to browse as a tree. Extract it as JSON instead.",
                    asset.display_name(),
                    crate::model::table::format_bytes(asset.byte_size)
                ))
            } else {
                DetailContent::Tree(view.structured(id).map_err(err)?)
            }
        }
    })
}

/// Re-applies a channel mask to the detail image.
pub fn remask(image: &RgbaImage, channels: Channels) -> ColorImage {
    let pixels = channels.apply(image.pixels());
    ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], &pixels)
}

/// Runs an extraction, reporting throttled progress and a summary.
pub fn extract(
    store: &Store,
    request: &ExtractRequest,
    options: &ExportOptions,
    generation: Generation,
    cancel: &AtomicBool,
    outbox: &Mailbox,
) {
    let state = Mutex::new((Throttle::new(PROGRESS_INTERVAL), ExtractProgress::default()));
    let progress = |p: &ae3_core::ExportProgress<'_>| {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        let (throttle, view) = &mut *guard;
        view.apply(p);
        if throttle.ready(Instant::now()) {
            outbox.send(Msg::ExtractProgress {
                generation,
                progress: view.clone(),
            });
        }
    };
    let output_dir = options.output_dir.clone();
    let summary = match store.export(&request.selection(), options, &progress, cancel) {
        Ok(report) => ExtractSummary::from_report(&report, output_dir),
        Err(e) => ExtractSummary::failed(output_dir, e.to_string()),
    };
    outbox.send(Msg::ExtractDone {
        generation,
        summary,
    });
}

/// Fetches two Manifests and computes their Diff, returning a store for
/// the newer Asset Version (to extract from). `current` is reused when it
/// is that version.
pub fn diff(
    current: &Store,
    old: AssetVersion,
    new: Option<AssetVersion>,
) -> Result<(Arc<Diff>, Store), String> {
    let cache = Arc::clone(current.cache());
    let platform = current.platform();
    let at = |version: AssetVersion| CdnLocation {
        asset_version: version,
        ..current.location().clone()
    };
    let new_store = match new {
        Some(v) if v != current.location().asset_version => {
            let manifest = cache
                .get_or_fetch_manifest(&at(v), platform)
                .map_err(|e| format!("Asset Version {v}: {e}"))?;
            Store::new(Arc::clone(&cache), at(v), platform, manifest)
        }
        _ => current.clone(),
    };
    let old_manifest = cache
        .get_or_fetch_manifest(&at(old), platform)
        .map_err(|e| format!("Asset Version {old}: {e}"))?;
    let diff = Diff::compute(&old_manifest, new_store.manifest());
    Ok((Arc::new(diff), new_store))
}

/// Writes the configuration file.
pub fn save_config(file: &ae3_core::ConfigFile, path: &std::path::Path) -> Result<(), String> {
    file.save(path).map_err(|e| e.to_string())
}

/// Opens `dir` in the system file manager, creating it if needed.
pub fn reveal(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dir: PathBuf = dir.canonicalize().unwrap_or_else(|_| dir.to_owned());
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(&dir)
        .spawn()
        .map(drop)
        .map_err(|e| format!("cannot run {program}: {e}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn fixture(name: &str) -> Option<PathBuf> {
        let root = std::env::var_os("AE3_FIXTURES").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            PathBuf::from,
        );
        let path = root.join("100054/Windows").join(name);
        if path.exists() {
            Some(path)
        } else {
            eprintln!("skipping: fixture {} not found", path.display());
            None
        }
    }

    fn outbox() -> (Mailbox, std::sync::mpsc::Receiver<Msg>) {
        Outbox::new(|| {})
    }

    #[test]
    fn thumbnails_decode_a_portrait_and_stop_when_stale() {
        let Some(path) = fixture("charui_bossk.bundle") else {
            return;
        };
        let bundle = LoadedBundle::open_file(&path).unwrap();
        let textures: Vec<i64> = bundle
            .assets()
            .unwrap()
            .into_iter()
            .filter(|a| a.kind == AssetKind::Texture)
            .map(|a| a.path_id)
            .collect();
        assert_ne!(textures, Vec::<i64>::new());
        let current = AtomicU64::new(3);
        let (outbox, rx) = outbox();
        thumbnails(&bundle, &textures, 3, &current, &outbox);
        let Ok(Msg::Thumbnail { result, .. }) = rx.try_recv() else {
            panic!("no thumbnail");
        };
        let image = result.unwrap();
        assert!(image.width() <= THUMBNAIL_PX as usize && image.height() <= THUMBNAIL_PX as usize);
        // A stale generation decodes nothing.
        thumbnails(&bundle, &textures, 2, &current, &outbox);
        while let Ok(msg) = rx.try_recv() {
            assert!(matches!(msg, Msg::Thumbnail { generation: 3, .. }));
        }
    }

    #[test]
    fn detail_reads_textures_atlases_and_trees() {
        let Some(path) = fixture("unit_details_atlas.bundle") else {
            return;
        };
        let bundle = LoadedBundle::open_file(&path).unwrap();
        let assets = bundle.assets().unwrap();
        let atlas = assets.iter().find(|a| a.kind == AssetKind::Atlas).unwrap();
        match detail(&bundle, atlas, 16384).unwrap() {
            DetailContent::Atlas { atlas, image, .. } => {
                assert_ne!(atlas.sprites.len(), 0);
                let image = image.expect("atlas texture");
                assert_eq!(image.color.width(), image.width as usize);
            }
            _ => panic!("expected an atlas"),
        }
        let texture = assets
            .iter()
            .find(|a| a.kind == AssetKind::Texture)
            .unwrap();
        match detail(&bundle, texture, 64).unwrap() {
            DetailContent::Image(image) => {
                // Downscaled to the GPU limit, true size kept.
                assert!(image.color.width() <= 64 && image.color.height() <= 64);
                assert!(image.width > 64 || image.height > 64);
            }
            _ => panic!("expected an image"),
        }
        let data = assets
            .iter()
            .find(|a| matches!(a.kind, AssetKind::Structured | AssetKind::Other))
            .unwrap();
        assert!(matches!(
            detail(&bundle, data, 16384).unwrap(),
            DetailContent::Tree(_) | DetailContent::Info(_)
        ));
    }

    /// Resolves the latest Asset Version on the real CDN, loads its
    /// Manifest and opens one Bundle, as Fetch Latest and a click do.
    #[test]
    #[ignore = "needs the network"]
    fn live_fetch_latest_and_open_a_bundle() {
        let dir = std::env::temp_dir().join(format!("ae3-gui-live-{}", std::process::id()));
        let services = Services::new(Config {
            cache_dir: Some(dir.clone()),
            ..Config::default()
        });
        let store = load_manifest(&services, Platform::Windows, None).unwrap();
        assert!(store.manifest().packages.len() > 1000);
        assert_eq!(
            latest_version(&services, Platform::Windows),
            Some(store.location().asset_version)
        );
        let current = AtomicU64::new(1);
        let (outbox, rx) = outbox();
        open_package(
            &store,
            "charui_bossk",
            PackageKind::Bundle,
            1,
            &current,
            &outbox,
        );
        let opened = loop {
            if let Msg::PreviewOpened { result, .. } = rx.recv().unwrap() {
                break result.unwrap();
            }
        };
        let Opened::Bundle { assets, kinds, .. } = opened else {
            panic!("expected a Bundle");
        };
        assert!(kinds.contains(AssetKind::Texture));
        assert!(assets.iter().any(|a| a.kind == AssetKind::Texture));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn remask_keeps_size() {
        let image = RgbaImage::new(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let masked = remask(
            &image,
            Channels {
                r: false,
                g: false,
                b: false,
                a: true,
            },
        );
        assert_eq!(masked.size, [2, 1]);
        assert_eq!(masked.pixels[0], eframe::egui::Color32::from_gray(4));
    }
}
