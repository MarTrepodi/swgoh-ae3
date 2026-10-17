//! The work done off the UI thread, and the messages it sends back.
//!
//! Each function here runs on a pool or job thread, calls `ae3-core`, and
//! reports through an [`Outbox`]. Stale work is skipped early: tasks carry
//! the generation they were started for and compare it with a shared
//! counter before each expensive step.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use ae3_core::ae3_audio::{self, Pcm};
use ae3_core::ae3_image::{self, RgbaImage};
use ae3_core::http::{HttpError, Transport, UreqTransport};
use ae3_core::version::CDN_ROOT;
use ae3_core::{
    AssetIndex, AssetInfo, AssetKind, AssetKinds, AssetVersion, Atlas, CdnLocation, Config, Diff,
    Environment, ExportOptions, IndexBuilder, IndexProgress, Language, LoadedAudioPackage,
    LoadedBundle, NameIndex, NameIndexLoad, NameIndexStatus, NamesEvent, PackageCache, PackageKind,
    PackageStore, Platform, Resolver, SoundInfo, Value, VersionSource,
};
use eframe::egui::ColorImage;

use crate::model::IndexProgressView;
use crate::model::extract::{ExtractProgress, ExtractRequest, ExtractSummary};
use crate::model::jobs::{Generation, Throttle};
use crate::model::mesh::MeshPreview;
use crate::model::names::NamesState;
use crate::model::playback::{SoundRef, unplayable_clip};
use crate::model::preview::THUMBNAIL_PX;
use crate::model::release::{self, Release, ReleaseState};
use crate::model::table::SearchMatches;
use crate::model::viewer::Channels;
use crate::worker::Outbox;

/// The HTTP client every network call goes through: [`UreqTransport`] in
/// the app, a fake in tests.
pub type Net = Arc<dyn Transport + Send + Sync>;
pub type Cache = PackageCache<Net>;
pub type Store = PackageStore<Net>;

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
    transport: Net,
    cache: OnceLock<Result<Arc<Cache>, String>>,
}

impl Services {
    /// Services that reach the network through [`UreqTransport`].
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self::with_transport(config, Arc::new(UreqTransport::new()))
    }

    /// Services that reach the network through `transport`.
    #[must_use]
    pub fn with_transport(config: Config, transport: Net) -> Self {
        Self {
            config,
            transport,
            cache: OnceLock::new(),
        }
    }

    /// The HTTP client, for requests outside the download cache.
    #[must_use]
    pub fn transport(&self) -> &Net {
        &self.transport
    }

    /// The download cache, opened on first use (it reads its directory).
    pub fn cache(&self) -> Result<Arc<Cache>, String> {
        self.cache
            .get_or_init(|| {
                PackageCache::from_config(&self.config, Arc::clone(&self.transport))
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
        /// Kept for playback, which decodes from it.
        package: Arc<LoadedAudioPackage>,
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
    /// A Mesh, ready to upload for the 3D view.
    Mesh(Arc<MeshPreview>),
    /// An AudioClip: a description, and why it cannot be played (`None`
    /// when it can be tried).
    Audio {
        text: String,
        unplayable: Option<String>,
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

/// The latest Asset Version, as the startup load found it.
pub enum Latest {
    /// Not newer than the cached Manifest the startup opened, so only
    /// where it is served from.
    Located(CdnLocation),
    /// Newer (or nothing was cached): a store over its Manifest.
    Opened(Store),
}

impl Latest {
    #[must_use]
    pub fn asset_version(&self) -> AssetVersion {
        match self {
            Self::Located(location) => location.asset_version,
            Self::Opened(store) => store.location().asset_version,
        }
    }
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
    /// The startup load opened the last Asset Version shown from the
    /// download cache, or found none there.
    StartupCached {
        generation: Generation,
        store: Option<Store>,
    },
    /// The startup load resolved the latest Asset Version.
    StartupLatest {
        generation: Generation,
        result: Result<Latest, String>,
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
    /// A sound to play was decoded (playback generation).
    SoundDecoded {
        generation: Generation,
        result: Result<Pcm, String>,
    },
    /// The sound of a playback generation finished, or could not play.
    PlaybackEnded {
        generation: Generation,
        error: Option<String>,
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
    /// What the "swgoh-ae3 vX available" check knows.
    ReleaseChecked(ReleaseState),
    /// The About window's check: the saved state and what GitHub said.
    ManualReleaseChecked {
        state: ReleaseState,
        result: Result<Option<Release>, String>,
    },
    /// What the Name Index load is doing.
    NamesProgress(String),
    /// The Name Index as loaded, and how it came to be.
    NamesLoaded {
        index: Option<Arc<NameIndex>>,
        state: NamesState,
    },
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

/// Where [`PackageCache`] keeps Manifests under its root:
/// `manifests/{Environment}/{Platform}/{Asset Version}.data`.
const CACHED_MANIFESTS_DIR: &str = "manifests";

/// Opens `version`'s Manifest from the download cache without touching the
/// network, or `None` when it is not cached. The Environment is the one the
/// cached copy was filed under (the most recent, if several), and the base
/// URL is built from the one CDN host, as the ADR-0005 fallback does; the
/// startup load replaces it with the resolved one when that differs.
pub fn open_cached(
    services: &Services,
    platform: Platform,
    version: AssetVersion,
) -> Option<Store> {
    let cache = services.cache().ok()?;
    let file = format!("{version}.data");
    let environment = std::fs::read_dir(cache.root().join(CACHED_MANIFESTS_DIR))
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let environment = Environment::new(entry.file_name().to_str()?)?;
            let path = entry.path().join(platform.name()).join(&file);
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
            Some((modified, environment))
        })
        .max_by_key(|(modified, _)| *modified)?
        .1;
    let location = CdnLocation {
        asset_version: version,
        base_url: format!("{CDN_ROOT}{environment}/"),
        environment,
        source: VersionSource::Explicit,
    };
    // The copy is on disk, so this reads it (a corrupt copy is fetched again).
    Store::open(cache, location, platform).ok()
}

/// The startup load: opens `last_shown` from the cache and reports it at
/// once, then resolves the latest Asset Version and opens its Manifest when
/// it is newer than what was reported.
pub fn startup(
    services: &Services,
    platform: Platform,
    last_shown: Option<AssetVersion>,
    generation: Generation,
    outbox: &Mailbox,
) {
    let cached = last_shown.and_then(|v| open_cached(services, platform, v));
    let shown = cached.as_ref().map(|s| s.location().asset_version);
    outbox.send(Msg::StartupCached {
        generation,
        store: cached,
    });
    let result = services.cache().and_then(|cache| {
        let location = Resolver::new(
            cache.downloader().transport(),
            platform,
            services.config.effective_cache_dir(),
        )
        .resolve(None)
        .map_err(|e| e.to_string())?;
        if shown.is_some_and(|v| v >= location.asset_version) {
            Ok(Latest::Located(location))
        } else {
            Store::open(cache, location, platform)
                .map(Latest::Opened)
                .map_err(|e| e.to_string())
        }
    });
    outbox.send(Msg::StartupLatest { generation, result });
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
                package: Arc::new(package),
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
            DetailContent::Audio {
                text: format!(
                    "AudioClip, {} of {} data.",
                    crate::model::table::format_bytes(clip.bytes.len() as u64),
                    clip.extension
                ),
                unplayable: unplayable_clip(clip.extension),
            }
        }
        AssetKind::Mesh => {
            DetailContent::Mesh(Arc::new(MeshPreview::new(&view.mesh(id).map_err(err)?)))
        }
        AssetKind::Structured | AssetKind::Other => {
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

/// What a sound to play is decoded from: the preview's Package, shared.
#[derive(Clone)]
pub enum SoundFrom {
    Package(Arc<LoadedAudioPackage>),
    Bundle(Arc<LoadedBundle>),
}

impl Opened {
    /// The Package to decode the preview's sounds from.
    #[must_use]
    pub fn sound_source(&self) -> SoundFrom {
        match self {
            Self::Audio { package, .. } => SoundFrom::Package(Arc::clone(package)),
            Self::Bundle { bundle, .. } => SoundFrom::Bundle(Arc::clone(bundle)),
        }
    }
}

/// Decodes a sound of the preview to samples for playback: an Audio
/// Package's sound through `ae3-audio`, or a Bundle's WAV AudioClip. A
/// panic in a decoder is reported as an error.
pub fn decode_sound(from: &SoundFrom, sound: SoundRef) -> Result<Pcm, String> {
    let decode = || match (from, sound) {
        (SoundFrom::Package(package), SoundRef::Sound(index)) => {
            let sound = package
                .sounds()
                .get(index)
                .ok_or_else(|| format!("no sound {index} in {}", package.name()))?;
            package
                .package()
                .decode_pcm(sound)
                .map_err(|e| e.to_string())
        }
        (SoundFrom::Bundle(bundle), SoundRef::Clip(path_id)) => {
            let clip = bundle
                .view()
                .and_then(|view| view.audio_clip(path_id))
                .map_err(|e| e.to_string())?;
            if let Some(why) = unplayable_clip(clip.extension) {
                return Err(why);
            }
            ae3_audio::decode_wem(&clip.bytes).map_err(|e| e.to_string())
        }
        _ => Err("the sound is not in the opened Package".to_owned()),
    };
    ae3_core::guard::catch_panic(decode)
        .unwrap_or_else(|panic| Err(format!("the decoder failed: {panic}")))
}

/// Re-applies a channel mask to the detail image.
#[must_use]
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

/// Loads the Name Index for `language` from the cache under `cache_root`:
/// the cached copy first, so display names match at once, then the copy
/// brought up to date with the community gamedata mirror. An update
/// downloads about 25 MB, so this runs on a thread of its own. It never
/// fails: a problem (or a panic while parsing the gamedata) is reported in
/// the final [`Msg::NamesLoaded`].
pub fn load_names<T: Transport + ?Sized>(
    transport: &T,
    cache_root: &Path,
    language: Language,
    outbox: &Mailbox,
) {
    let open = |update: bool, on_event: &mut dyn FnMut(NamesEvent<'_>)| {
        ae3_core::guard::catch_panic(|| {
            NameIndex::open(transport, cache_root, language, update, on_event)
        })
        .unwrap_or_else(|panic| NameIndexLoad {
            index: None,
            status: NameIndexStatus::Unavailable,
            problem: Some(format!("the Name Index could not be read: {panic}")),
            path: ae3_core::names::default_name_index_path(cache_root, language),
        })
    };
    let cached = open(false, &mut |_| {});
    let shown = cached
        .index
        .is_some()
        .then(|| NamesState::from_load(&cached, true));
    let fallback = cached.index.map(Arc::new);
    if let Some(state) = shown {
        outbox.send(Msg::NamesLoaded {
            index: fallback.clone(),
            state,
        });
    }
    let mut load = open(true, &mut |event| {
        let step = match event {
            NamesEvent::Building { versions } => format!(
                "Building display names from gamedata {}…",
                short_version(&versions.gamedata)
            ),
            NamesEvent::Downloading { file } => format!("Downloading display names: {file}…"),
            _ => return,
        };
        outbox.send(Msg::NamesProgress(step));
    });
    let index = match load.index.take() {
        Some(index) => Some(Arc::new(index)),
        // A panic during the update must not take away the copy shown.
        None if fallback.is_some() => {
            load.status = NameIndexStatus::Stale;
            fallback
        }
        None => None,
    };
    let state = NamesState::new(index.as_deref(), load.status, load.problem, false);
    outbox.send(Msg::NamesLoaded { index, state });
}

/// A gamedata version without its hash: `0.40.6:i9YY…` -> `0.40.6`.
fn short_version(version: &str) -> &str {
    version.split(':').next().unwrap_or(version)
}

/// The "swgoh-ae3 vX available" check: reads the saved state at `path`, asks
/// GitHub for the latest release when a day has passed since the last
/// check, and saves the outcome. Failures (a 404 while the repository is
/// not public, no network, an unwritable cache) are silent.
pub fn check_release<T: Transport + ?Sized>(
    transport: &T,
    path: Option<&Path>,
    now: u64,
) -> ReleaseState {
    let mut state = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| ReleaseState::parse(&text))
        .unwrap_or_default();
    if !state.is_due(now) {
        return state;
    }
    let found = release::latest_release_api(release::REPOSITORY)
        .and_then(|url| transport.get_text(&url).ok())
        .and_then(|body| release::parse_latest_release(&body, release::REPOSITORY));
    state.checked(now, found);
    if let Some(path) = path {
        save_release_state(&state, path);
    }
    state
}

/// Asks GitHub for the latest release. `Ok(None)` when the repository has
/// none published (a 404, as while it is private or before a first
/// release); `Err` when GitHub could not be asked or answered oddly.
pub fn fetch_latest_release<T: Transport + ?Sized>(
    transport: &T,
) -> Result<Option<Release>, String> {
    let url = release::latest_release_api(release::REPOSITORY)
        .ok_or_else(|| format!("{} is not a GitHub repository", release::REPOSITORY))?;
    match transport.get_text(&url) {
        Ok(body) => release::parse_latest_release(&body, release::REPOSITORY)
            .map(Some)
            .ok_or_else(|| "GitHub's answer was not a release".to_owned()),
        Err(HttpError::Status { status: 404, .. }) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The About window's "Check for updates": asks GitHub now, whatever the
/// once-a-day limit says, and records the answer like the daily check so
/// the notice and the next daily check agree with it.
pub fn check_release_now<T: Transport + ?Sized>(
    transport: &T,
    path: Option<&Path>,
    now: u64,
) -> (ReleaseState, Result<Option<Release>, String>) {
    let mut state = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| ReleaseState::parse(&text))
        .unwrap_or_default();
    let result = fetch_latest_release(transport);
    state.checked(now, result.clone().ok().flatten());
    if let Some(path) = path {
        save_release_state(&state, path);
    }
    (state, result)
}

/// Saves the release-check state, best effort: losing it only means one
/// more check.
pub fn save_release_state(state: &ReleaseState, path: &Path) {
    let tmp = path.with_extension("json.tmp");
    let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&tmp, state.to_json()))
        .and_then(|()| std::fs::rename(&tmp, path));
    if saved.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Seconds since the Unix epoch.
#[must_use]
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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

    #[test]
    fn detail_prepares_meshes_for_the_3d_view() {
        let Some(path) = fixture("char_bossk_pre.bundle") else {
            return;
        };
        let bundle = LoadedBundle::open_file(&path).unwrap();
        let assets = bundle.assets().unwrap();
        let mesh = assets
            .iter()
            .find(|a| a.kind == AssetKind::Mesh && a.name == "bossk_mesh")
            .unwrap();
        match detail(&bundle, mesh, 16384).unwrap() {
            DetailContent::Mesh(preview) => {
                assert_eq!(preview.vertex_count, 1693);
                assert_eq!(preview.triangle_count, 2502);
                assert_eq!(preview.vertices.len(), 1693 * 6);
                assert_eq!(preview.indices.len(), 2502 * 3);
                assert!(preview.has_normals && preview.has_uvs);
                // Bossk stands about 1.9 units tall around y = 0.94.
                assert!(
                    (preview.center[1] - 0.935).abs() < 0.01,
                    "{:?}",
                    preview.center
                );
                assert!(
                    preview.radius > 0.9 && preview.radius < 1.2,
                    "{}",
                    preview.radius
                );
            }
            _ => panic!("expected a mesh"),
        }
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

    /// The startup load against the real CDN: with an empty cache it opens
    /// the latest Manifest; on the next start that Manifest comes from the
    /// cache and the latest is only located.
    #[test]
    #[ignore = "needs the network"]
    fn live_startup_uses_the_cache_on_the_second_run() {
        let dir = std::env::temp_dir().join(format!("ae3-gui-live-start-{}", std::process::id()));
        let services = Services::new(Config {
            cache_dir: Some(dir.clone()),
            ..Config::default()
        });
        let run = |last_shown| {
            let (outbox, rx) = outbox();
            startup(&services, Platform::Windows, last_shown, 7, &outbox);
            let Ok(Msg::StartupCached {
                generation: 7,
                store,
            }) = rx.try_recv()
            else {
                panic!("no cached message first");
            };
            let Ok(Msg::StartupLatest {
                generation: 7,
                result,
            }) = rx.try_recv()
            else {
                panic!("no latest message");
            };
            (store, result.unwrap())
        };
        let (cached, latest) = run(Some(AssetVersion(100_054)));
        assert!(cached.is_none(), "the cache starts empty");
        let Latest::Opened(store) = latest else {
            panic!("the latest Manifest is opened");
        };
        let version = store.location().asset_version;
        let (cached, latest) = run(Some(version));
        let cached = cached.expect("the Manifest is cached now");
        assert_eq!(cached.manifest(), store.manifest());
        assert_eq!(cached.location().base_url, store.location().base_url);
        assert!(matches!(latest, Latest::Located(l) if l.asset_version == version));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Answers GitHub's latest-release URL with `body`, or 404 without one,
    /// and counts the requests.
    #[test]
    fn check_now_ignores_the_daily_limit_and_says_what_it_found() {
        let dir = std::env::temp_dir().join(format!("ae3-gui-release-now-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(ReleaseState::FILE);
        let t0 = 1_800_000_000;

        let missing = FakeGitHub {
            body: None,
            requests: std::cell::Cell::new(0),
        };
        let (_, result) = check_release_now(&missing, Some(&path), t0);
        assert_eq!(result, Ok(None), "a 404 means nothing is published");
        // A second click the same minute still asks.
        let _ = check_release_now(&missing, Some(&path), t0 + 1);
        assert_eq!(missing.requests.get(), 2);

        let published = FakeGitHub {
            body: Some(
                r#"{"tag_name":"v99.0.0","html_url":"https://github.com/swgoh-utils/swgoh-ae3/releases/tag/v99.0.0"}"#,
            ),
            requests: std::cell::Cell::new(0),
        };
        let (state, result) = check_release_now(&published, Some(&path), t0 + 2);
        assert_eq!(result.unwrap().unwrap().tag, "v99.0.0");
        assert_eq!(
            state.latest.unwrap().tag,
            "v99.0.0",
            "the notice learns it too"
        );
        // The daily check now waits a day from this one.
        check_release(&published, Some(&path), t0 + 3);
        assert_eq!(published.requests.get(), 1);

        let garbled = FakeGitHub {
            body: Some("not json"),
            requests: std::cell::Cell::new(0),
        };
        assert!(check_release_now(&garbled, None, t0).1.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct FakeGitHub {
        body: Option<&'static str>,
        requests: std::cell::Cell<usize>,
    }

    impl Transport for FakeGitHub {
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ae3_core::http::HttpError> {
            self.requests.set(self.requests.get() + 1);
            assert_eq!(
                url,
                "https://api.github.com/repos/swgoh-utils/swgoh-ae3/releases/latest"
            );
            self.body.map(|b| b.as_bytes().to_vec()).ok_or_else(|| {
                ae3_core::http::HttpError::Status {
                    url: url.to_owned(),
                    status: 404,
                }
            })
        }

        fn get_range(
            &self,
            url: &str,
            _start: u64,
            _end_inclusive: u64,
        ) -> Result<ae3_core::http::RangeResponse, ae3_core::http::HttpError> {
            panic!("unexpected Range request for {url}");
        }

        fn exists(&self, url: &str) -> Result<bool, ae3_core::http::HttpError> {
            panic!("unexpected probe of {url}");
        }
    }

    #[test]
    fn release_check_asks_github_once_a_day_and_survives_a_404() {
        let dir = std::env::temp_dir().join(format!("ae3-gui-release-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(ReleaseState::FILE);
        let day = release::CHECK_INTERVAL.as_secs();
        let t0 = 1_800_000_000;

        // Not published there yet: silent, but remembered as checked.
        let missing = FakeGitHub {
            body: None,
            requests: std::cell::Cell::new(0),
        };
        let state = check_release(&missing, Some(&path), t0);
        assert_eq!(missing.requests.get(), 1);
        assert_eq!(state.latest, None);
        assert_eq!(state.checked_at, Some(t0));
        assert!(path.exists(), "the check is remembered");
        check_release(&missing, Some(&path), t0 + day - 1);
        assert_eq!(missing.requests.get(), 1, "not again the same day");

        let published = FakeGitHub {
            body: Some(
                r#"{"tag_name":"v99.0.0","html_url":"https://github.com/swgoh-utils/swgoh-ae3/releases/tag/v99.0.0"}"#,
            ),
            requests: std::cell::Cell::new(0),
        };
        let state = check_release(&published, Some(&path), t0 + day);
        assert_eq!(published.requests.get(), 1);
        let (version, found) = state.notice(release::ReleaseVersion::current()).unwrap();
        assert_eq!(version.major, 99);
        assert!(found.url.ends_with("/v99.0.0"));
        // The next start shows it without asking.
        let again = check_release(&published, Some(&path), t0 + day + 60);
        assert_eq!(published.requests.get(), 1);
        assert_eq!(again, state);

        // Without a cache directory nothing is remembered, but it works.
        let state = check_release(&published, None, t0);
        assert!(state.latest.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Asks the real GitHub. While the repository is not public there the
    /// answer is a 404, which must leave no notice and no error.
    #[test]
    #[ignore = "needs the network"]
    fn live_release_check_is_quiet() {
        let state = check_release(&UreqTransport::new(), None, unix_now());
        assert!(state.checked_at.is_some());
        eprintln!("latest release: {:?}", state.latest);
    }

    /// No network at all.
    struct Offline;

    impl Transport for Offline {
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, ae3_core::http::HttpError> {
            Err(ae3_core::http::HttpError::Transport {
                url: url.to_owned(),
                message: "offline".to_owned(),
            })
        }

        fn get_range(
            &self,
            url: &str,
            _start: u64,
            _end_inclusive: u64,
        ) -> Result<ae3_core::http::RangeResponse, ae3_core::http::HttpError> {
            self.get_bytes(url).map(|_| unreachable!())
        }

        fn exists(&self, url: &str) -> Result<bool, ae3_core::http::HttpError> {
            self.get_bytes(url).map(|_| true)
        }
    }

    /// Offline, a cached Name Index is shown at once and kept; with none
    /// cached, display names are reported unavailable. Either way the load
    /// ends with one final message.
    #[test]
    fn name_index_loads_from_the_cache_and_survives_offline() {
        let dir = std::env::temp_dir().join(format!("ae3-gui-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let language = Language::default();
        let collect = || {
            let (outbox, rx) = Outbox::new(|| {});
            load_names(&Offline, &dir, language, &outbox);
            rx.try_iter()
                .filter_map(|msg| match msg {
                    Msg::NamesLoaded { index, state } => Some((index.map(|i| i.len()), state)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        let none = collect();
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].0, None);
        assert!(matches!(none[0].1, NamesState::Unavailable { .. }));

        let entity = ae3_core::GameEntity {
            kind: ae3_core::EntityKind::Unit,
            id: "BOSSK".into(),
            name: "Bossk".into(),
            context: None,
            bundles: vec!["charui_bossk".into()],
        };
        NameIndex::new(
            language,
            ae3_core::GamedataVersions::default(),
            vec![entity],
        )
        .save(&ae3_core::names::default_name_index_path(&dir, language))
        .unwrap();
        let cached = collect();
        assert_eq!(cached.len(), 2, "the cached copy, then the update");
        assert_eq!(cached[0].0, Some(1));
        assert!(matches!(
            cached[0].1,
            NamesState::Ready {
                status: NameIndexStatus::Unchecked,
                checking: true,
                ..
            }
        ));
        assert_eq!(cached[1].0, Some(1));
        assert!(matches!(
            &cached[1].1,
            NamesState::Ready {
                status: NameIndexStatus::Stale,
                problem: Some(_),
                checking: false,
                ..
            }
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Builds the Name Index from the real gamedata mirror and finds Bossk.
    #[test]
    #[ignore = "needs the network"]
    fn live_name_index_finds_bossk() {
        let dir = std::env::temp_dir().join(format!("ae3-gui-live-names-{}", std::process::id()));
        let (outbox, rx) = Outbox::new(|| {});
        load_names(&UreqTransport::new(), &dir, Language::default(), &outbox);
        let mut steps = 0;
        let mut last = None;
        for msg in rx.try_iter() {
            match msg {
                Msg::NamesProgress(_) => steps += 1,
                Msg::NamesLoaded { index, state } => last = Some((index, state)),
                _ => {}
            }
        }
        let (index, state) = last.expect("a final message");
        eprintln!("{steps} progress steps; {state:?}");
        let index = index.expect("an index");
        let names: Vec<_> = index
            .search("bossk")
            .iter()
            .map(|m| m.entity.name.clone())
            .collect();
        eprintln!("bossk: {names:?}");
        assert!(names.iter().any(|n| n == "Bossk"));
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
