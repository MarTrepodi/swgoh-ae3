//! The application: owns the model, the background workers and the GPU
//! textures, turns user actions into jobs and job results into model
//! updates. Drawing lives in [`crate::ui`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ae3_core::config::{AudioFormat, ImageFormat, Layout, MeshFormat, StructuredMode};
use ae3_core::{AssetIndex, AssetInfo, AssetVersion, Config, ConfigFile, ExportOptions, Platform};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::model::Model;
use crate::model::extract::{ExtractRequest, ExtractSummary};
use crate::model::lru::{ByteLru, rgba_bytes};
use crate::model::mesh::OrbitCamera;
use crate::model::names::NamesState;
use crate::model::playback::{Action, Playback, SoundRef, unplayable_sound};
use crate::model::release::ReleaseState;
use crate::model::startup::LatestDecision;
use crate::model::table::SearchMatches;
use crate::model::viewer::{Channels, ViewTransform};
use crate::output::{DeviceOutput, Output};
use crate::tasks::{self, DetailContent, Mailbox, Msg, Opened, Services, Store};
use crate::ui::mesh::{self, MeshRenderer};
use crate::worker::{Outbox, Pool, pool_size, spawn_long};

/// Budget for decoded textures held on the GPU (thumbnails and previews).
pub const TEXTURE_BUDGET: u64 = 512 << 20;
/// How long after the last settings change the file is written.
const SAVE_DELAY: Duration = Duration::from_secs(1);
/// Thumbnails decoded per pool task; several tasks run in parallel.
const THUMBNAILS_PER_TASK: usize = 4;

/// A texture in the cache: a Package's Asset, as a thumbnail or full size.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TexKey {
    pub package: String,
    pub path_id: i64,
    pub full: bool,
}

/// The detail viewer's state.
pub struct Detail {
    pub asset: AssetInfo,
    pub package: String,
    pub generation: u64,
    /// `None` while loading.
    pub content: Option<Result<DetailContent, String>>,
    pub transform: ViewTransform,
    /// Whether the transform has been fitted to the view yet.
    pub fitted: bool,
    pub channels: Channels,
    /// Channels of the uploaded texture (a remask may be in flight).
    pub shown_channels: Channels,
    pub selected_sprite: Option<usize>,
    /// The 3D view's camera, for a Mesh.
    pub camera: OrbitCamera,
    pub open: bool,
}

/// Settings the user changed that the configuration file should keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingChange {
    Platform(Platform),
    ImageFormat(ImageFormat),
    AtlasCutting(bool),
    PaddingRestoration(bool),
    AudioFormat(AudioFormat),
    Layout(Layout),
    SkipExisting(bool),
    Structured(StructuredMode),
    MeshFormat(MeshFormat),
}

pub struct Ae3App {
    pub model: Model,
    pub config: Config,
    config_file: ConfigFile,
    config_path: Option<PathBuf>,
    config_dirty_since: Option<Instant>,
    services: Arc<Services>,
    pool: Pool,
    outbox: Mailbox,
    inbox: Receiver<Msg>,
    pub store: Option<Store>,
    pub index: Option<Arc<AssetIndex>>,
    index_pause: Arc<AtomicBool>,
    index_cancel: Arc<AtomicBool>,
    index_thread: Option<JoinHandle<()>>,
    /// Whether a loaded Manifest starts an Asset Index build; tests turn
    /// it off to stay offline.
    index_on_load: bool,
    /// The query a background search is running for, and whether another
    /// is wanted after it.
    search_in_flight: bool,
    search_wanted: bool,
    preview_generation: Arc<AtomicU64>,
    pub opened: Option<Opened>,
    pub preview_status: Option<String>,
    pub textures: ByteLru<TexKey, TextureHandle>,
    pub thumb_requested: HashSet<i64>,
    pub thumb_failed: HashMap<i64, String>,
    thumb_queue: Vec<i64>,
    pub detail: Option<Detail>,
    detail_counter: u64,
    /// Which sound of the preview plays, and which cannot.
    pub playback: Playback,
    /// The playback generation decodes are for; a decode started for an
    /// older one is skipped.
    playback_current: Arc<AtomicU64>,
    audio: Box<dyn Output>,
    /// Draws the detail viewer's Mesh; shared with its paint callback.
    pub mesh_renderer: Arc<Mutex<MeshRenderer>>,
    extract_cancel: Option<Arc<AtomicBool>>,
    pub diff_store: Option<Store>,
    checker: Option<TextureHandle>,
    /// The application icon, for the About window; loaded on first use.
    about_icon: Option<TextureHandle>,
    max_texture_side: u32,
    pub last_load_error: Option<String>,
}

impl Ae3App {
    /// Builds the app; `config` and `file` come from `Config::load`.
    #[must_use]
    pub fn new(
        ctx: &egui::Context,
        config: Config,
        config_file: ConfigFile,
        config_path: Option<PathBuf>,
        notice: Option<String>,
    ) -> Self {
        let services = Services::new(config.clone());
        Self::with_services(ctx, config, config_file, config_path, notice, services)
    }

    /// Builds the app over `services`, which decide how the network is
    /// reached ([`Services::with_transport`]) and where the download cache
    /// lives (`services.config`). Tests pass a fake transport here.
    #[must_use]
    pub fn with_services(
        ctx: &egui::Context,
        config: Config,
        config_file: ConfigFile,
        config_path: Option<PathBuf>,
        notice: Option<String>,
        services: Services,
    ) -> Self {
        crate::ui::install(ctx);
        let repaint = ctx.clone();
        let (outbox, inbox) = Outbox::new(move || repaint.request_repaint());
        let ended = outbox.clone();
        let audio = DeviceOutput::new(Arc::new(move |generation, error| {
            ended.send(Msg::PlaybackEnded { generation, error });
        }));
        let mut model = Model::default();
        model.notice = notice;
        model.version_text = config_file
            .gui
            .last_asset_version
            .map(|v| v.to_string())
            .unwrap_or_default();
        Self {
            model,
            services: Arc::new(services),
            config,
            config_file,
            config_path,
            config_dirty_since: None,
            pool: Pool::new(pool_size()),
            outbox,
            inbox,
            store: None,
            index: None,
            index_pause: Arc::new(AtomicBool::new(false)),
            index_cancel: Arc::new(AtomicBool::new(false)),
            index_thread: None,
            index_on_load: true,
            search_in_flight: false,
            search_wanted: false,
            preview_generation: Arc::new(AtomicU64::new(0)),
            opened: None,
            preview_status: None,
            textures: ByteLru::new(TEXTURE_BUDGET),
            thumb_requested: HashSet::new(),
            thumb_failed: HashMap::new(),
            thumb_queue: Vec::new(),
            detail: None,
            detail_counter: 0,
            playback: Playback::default(),
            playback_current: Arc::new(AtomicU64::new(0)),
            audio: Box::new(audio),
            mesh_renderer: Arc::default(),
            extract_cancel: None,
            diff_store: None,
            checker: None,
            about_icon: None,
            max_texture_side: 2048,
            last_load_error: None,
        }
    }

    /// Starts what the window does by itself: loading the latest Manifest
    /// and the Name Index and, unless turned off, looking for a newer AE3
    /// release. Kept out of [`Ae3App::new`] so tests build an app that stays
    /// offline.
    pub fn start(&mut self) {
        self.start_up();
        self.start_names();
        if self.config.check_for_updates {
            self.start_release_check();
        }
    }

    // ---- Seams for tests and the smoke run -----------------------------

    /// Plays sounds through `output` instead of the default device.
    pub fn set_audio_output(&mut self, output: Box<dyn Output>) {
        self.audio = output;
    }

    /// What an [`Output`] calls when a sound ended or could not play.
    #[must_use]
    pub fn playback_end_reporter(&self) -> crate::output::OnEnd {
        let ended = self.outbox.clone();
        Arc::new(move |generation, error| {
            ended.send(Msg::PlaybackEnded { generation, error });
        })
    }

    /// Whether a loaded Manifest starts an Asset Index build (it does by
    /// default). The build probes every Bundle over the network.
    pub fn set_index_on_load(&mut self, on: bool) {
        self.index_on_load = on;
    }

    /// The services background work runs with.
    #[must_use]
    pub fn services(&self) -> &Services {
        &self.services
    }

    /// The settings as the configuration file will be saved.
    #[must_use]
    pub fn config_file(&self) -> &ConfigFile {
        &self.config_file
    }

    /// Shows `manifest` as if it had been loaded for the configured
    /// Platform from the PROD Environment. Its Packages are fetched through
    /// the download cache when opened.
    pub fn show_manifest(&mut self, manifest: ae3_core::Manifest) -> Result<(), String> {
        let cache = self.services.cache()?;
        let environment = ae3_core::Environment::new("PROD").ok_or("no PROD Environment")?;
        let location = ae3_core::CdnLocation {
            asset_version: manifest.asset_version,
            base_url: format!("{}{environment}/", ae3_core::version::CDN_ROOT),
            environment,
            source: ae3_core::VersionSource::Explicit,
        };
        let store = Store::new(cache, location, self.config.platform, manifest);
        self.manifest_loaded(store);
        Ok(())
    }

    /// Opens the Bundle file at `path` in the preview, selecting the row of
    /// the Manifest shown that has its name, without the download cache.
    pub fn open_bundle_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bundle = ae3_core::LoadedBundle::open_file(path).map_err(|e| e.to_string())?;
        let row = self
            .model
            .rows
            .iter()
            .find(|r| r.name == bundle.name())
            .map(|r| r.id)
            .ok_or_else(|| format!("{} is not in the Manifest shown", bundle.name()))?;
        let assets = bundle.view().map_err(|e| e.to_string())?.assets();
        let kinds = assets.iter().map(|a| a.kind).collect();
        self.model.selection.clear();
        self.model
            .click_row(row, crate::model::selection::ClickModifiers::default());
        self.sync_preview();
        // A newer generation, so the open the selection started is ignored.
        let generation = self.preview_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.preview_opened(
            generation,
            Ok(Opened::Bundle {
                bundle: Arc::new(bundle),
                assets,
                kinds,
            }),
        );
        Ok(())
    }

    /// Uploads `image` into the texture cache under `key`, as a decoded
    /// thumbnail (`key.full` false) or detail texture is.
    pub fn upload_texture(&mut self, ctx: &egui::Context, key: TexKey, image: ColorImage) {
        let nearest = key.full;
        self.upload(ctx, key, image, nearest);
    }

    /// One frame as the window runs it, without the OpenGL housekeeping:
    /// worker messages and timers, then drawing and what it changed.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        self.tick(ui.ctx());
        self.draw(ui);
    }

    // ---- Name Index -----------------------------------------------------

    /// Loads the display names search matches on a thread of its own:
    /// the cached Name Index at once, then one brought up to date with the
    /// gamedata mirror. Search works on Bundle and Asset names meanwhile.
    fn start_names(&mut self) {
        self.model.names_state = NamesState::Loading {
            step: "Loading display names…".to_owned(),
        };
        let services = Arc::clone(&self.services);
        let language = self.config.language;
        let outbox = self.outbox.clone();
        spawn_long("ae3-names", move || match services.cache() {
            Ok(cache) => tasks::load_names(
                cache.downloader().transport(),
                cache.root(),
                language,
                &outbox,
            ),
            Err(e) => outbox.send(Msg::NamesLoaded {
                index: None,
                state: NamesState::Unavailable { problem: e },
            }),
        });
    }

    // ---- AE3 release notice --------------------------------------------

    /// Where the release check remembers its last answer.
    fn release_state_path(&self) -> Option<PathBuf> {
        self.services
            .config
            .effective_cache_dir()
            .map(|dir| dir.join(ReleaseState::FILE))
    }

    /// Looks for a newer AE3 release on a thread of its own; GitHub is
    /// asked at most once a day.
    fn start_release_check(&mut self) {
        let path = self.release_state_path();
        let transport = Arc::clone(self.services.transport());
        let outbox = self.outbox.clone();
        spawn_long("ae3-release-check", move || {
            let state = tasks::check_release(&transport, path.as_deref(), tasks::unix_now());
            outbox.send(Msg::ReleaseChecked(state));
        });
    }

    /// The About window's "Check for updates": asks GitHub now, on a thread
    /// of its own, regardless of `check-for-updates` and the daily limit.
    pub fn check_for_updates_now(&mut self) {
        if self.model.manual_check == crate::model::release::ManualCheck::Checking {
            return;
        }
        self.model.manual_check = crate::model::release::ManualCheck::Checking;
        let path = self.release_state_path();
        let transport = Arc::clone(self.services.transport());
        let outbox = self.outbox.clone();
        spawn_long("ae3-release-check-now", move || {
            let (state, result) =
                tasks::check_release_now(&transport, path.as_deref(), tasks::unix_now());
            outbox.send(Msg::ManualReleaseChecked { state, result });
        });
    }

    /// The application icon as a texture, for the About window.
    pub fn about_icon(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        if self.about_icon.is_none() {
            self.about_icon = crate::icon::color_image()
                .map(|image| ctx.load_texture("app-icon", image, TextureOptions::LINEAR));
        }
        self.about_icon.clone()
    }

    /// The About window's check came back.
    fn manual_release_checked(
        &mut self,
        state: ReleaseState,
        result: &Result<Option<crate::model::release::Release>, String>,
    ) {
        self.model.manual_check = crate::model::release::ManualCheck::from_result(
            result,
            crate::model::release::ReleaseVersion::current(),
        );
        self.model.release = Some(state);
    }

    /// Hides the "swgoh-ae3 vX available" notice until a later release.
    pub fn dismiss_release(&mut self) {
        let path = self.release_state_path();
        let Some(state) = &mut self.model.release else {
            return;
        };
        state.dismiss();
        if let Some(path) = path {
            let state = state.clone();
            self.pool
                .spawn(move || tasks::save_release_state(&state, &path));
        }
    }

    // ---- Manifest -------------------------------------------------------

    /// The startup load: the last Asset Version shown, from the cache, at
    /// once; then the latest (ADR-0005). The UI never waits for either.
    fn start_up(&mut self) {
        if self.model.manifest_job.is_active() {
            return;
        }
        let generation = self.begin_startup();
        let last_shown = self.config_file.gui.last_asset_version;
        let services = Arc::clone(&self.services);
        let platform = self.config.platform;
        let outbox = self.outbox.clone();
        spawn_long("ae3-startup", move || {
            tasks::startup(&services, platform, last_shown, generation, &outbox);
        });
    }

    /// Starts the startup load's Manifest job; its messages carry the
    /// generation returned.
    fn begin_startup(&mut self) -> u64 {
        let generation = self.model.manifest_job.start(Instant::now());
        self.model.startup.begin(generation);
        self.model.manifest_job.update(
            generation,
            match self.config_file.gui.last_asset_version {
                Some(v) => format!("Opening Asset Version {v}…"),
                None => "Finding the latest Asset Version…".to_owned(),
            },
        );
        generation
    }

    /// The startup load's cached Manifest arrived (or there was none).
    fn startup_cached(&mut self, generation: u64, store: Option<Store>) {
        if !self.model.startup.is_for(generation) {
            return;
        }
        match store {
            Some(store) => {
                if self.model.manifest_job.finish(generation, Ok(false)) {
                    self.last_load_error = None;
                    self.manifest_loaded(store);
                }
            }
            None => {
                self.model
                    .manifest_job
                    .update(generation, "Finding the latest Asset Version…".to_owned());
            }
        }
    }

    /// The startup load resolved the latest Asset Version.
    fn startup_latest(&mut self, generation: u64, result: Result<tasks::Latest, String>) {
        let job = &self.model.manifest_job;
        if job.is_active() && job.generation() == generation {
            // Nothing was cached: this is the window's first Manifest.
            self.model.startup.abandon();
            let outcome = result.as_ref().map(|_| false).map_err(Clone::clone);
            if self.model.manifest_job.finish(generation, outcome) {
                match result {
                    Ok(tasks::Latest::Opened(store)) => {
                        self.last_load_error = None;
                        self.manifest_loaded(store);
                    }
                    Ok(tasks::Latest::Located(_)) => {}
                    Err(e) => self.last_load_error = Some(e),
                }
            }
            return;
        }
        let shown = self.model.asset_version();
        let latest = match result {
            Ok(latest) => latest,
            Err(e) => {
                if self.model.startup.is_for(generation) {
                    self.model.startup.abandon();
                    if let Some(v) = shown {
                        self.model.notice = Some(format!(
                            "Showing Asset Version {v} from the cache; the latest could not be found: {e}"
                        ));
                    }
                }
                return;
            }
        };
        let version = latest.asset_version();
        let extracting = self.model.extract_job.is_active();
        match (
            self.model
                .startup
                .decide(generation, shown, version, extracting),
            latest,
        ) {
            (LatestDecision::Show, tasks::Latest::Opened(store)) => self.manifest_loaded(store),
            (LatestDecision::Keep, tasks::Latest::Located(location)) => {
                self.adopt_location(location);
            }
            (LatestDecision::Offer, _) => self.model.update.finished(Some(version)),
            _ => {}
        }
    }

    /// The cached Manifest's base URL was built rather than resolved; use
    /// the resolved one if it differs. The Manifest itself is unchanged.
    fn adopt_location(&mut self, location: ae3_core::CdnLocation) {
        let Some(store) = &self.store else { return };
        let current = store.location();
        if current.asset_version != location.asset_version
            || (current.base_url == location.base_url
                && current.environment == location.environment)
        {
            return;
        }
        let store = Store::new(
            Arc::clone(store.cache()),
            location,
            store.platform(),
            store.shared_manifest(),
        );
        self.store = Some(store);
        // The running build probes through the old location.
        if self.model.index_job.is_active() {
            self.stop_index();
            self.start_index();
        }
    }

    /// Resolves and loads a Manifest: the latest when `version` is `None`.
    pub fn load_manifest(&mut self, version: Option<AssetVersion>) {
        if self.model.manifest_job.is_active() {
            return;
        }
        self.model.startup.abandon();
        let generation = self.model.manifest_job.start(Instant::now());
        self.model.manifest_job.update(
            generation,
            match version {
                Some(v) => format!("Loading Asset Version {v}…"),
                None => "Finding the latest Asset Version…".to_owned(),
            },
        );
        let services = Arc::clone(&self.services);
        let platform = self.config.platform;
        let outbox = self.outbox.clone();
        spawn_long("ae3-manifest", move || {
            let result = tasks::load_manifest(&services, platform, version);
            outbox.send(Msg::ManifestLoaded { generation, result });
        });
    }

    fn manifest_loaded(&mut self, store: Store) {
        self.stop_index();
        self.clear_preview_resources();
        self.textures.clear();
        self.index = None;
        self.diff_store = None;
        let manifest = store.shared_manifest();
        let version = manifest.asset_version;
        self.model.set_manifest(manifest, Instant::now());
        if self.config_file.gui.last_asset_version != Some(version) {
            self.config_file.gui.last_asset_version = Some(version);
            self.config_dirty_since = Some(Instant::now());
        }
        self.store = Some(store);
        if self.index_on_load {
            self.start_index();
        }
    }

    fn check_for_update(&mut self, now: Instant) {
        self.model.update.started(now);
        let services = Arc::clone(&self.services);
        let platform = self.config.platform;
        let outbox = self.outbox.clone();
        spawn_long("ae3-update-check", move || {
            let latest = tasks::latest_version(&services, platform);
            outbox.send(Msg::UpdateChecked { latest });
        });
    }

    // ---- Asset Index ----------------------------------------------------

    fn start_index(&mut self) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let generation = self.model.index_job.start(Instant::now());
        self.index_pause = Arc::new(AtomicBool::new(false));
        self.index_cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::clone(&self.index_pause);
        let cancel = Arc::clone(&self.index_cancel);
        let outbox = self.outbox.clone();
        // A cancelled earlier build still saves the index file when it
        // stops; wait for it so the two builds never write it at once.
        let previous = self.index_thread.take();
        self.index_thread = Some(spawn_long("ae3-index", move || {
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            tasks::build_index(&store, generation, pause, &cancel, &outbox);
        }));
    }

    /// Cancels the running build; its thread is joined by the next one.
    fn stop_index(&mut self) {
        self.index_cancel.store(true, Ordering::Relaxed);
        self.index_pause.store(false, Ordering::Relaxed);
        self.model.index_job.reset();
    }

    /// Pauses or resumes the Asset Index build.
    pub fn toggle_index_pause(&mut self) {
        let paused = !self.index_pause.load(Ordering::Relaxed);
        self.index_pause.store(paused, Ordering::Relaxed);
    }

    #[must_use]
    pub fn index_pause_requested(&self) -> bool {
        self.index_pause.load(Ordering::Relaxed)
    }

    /// Starts an index build again (after it finished or failed).
    pub fn restart_index(&mut self) {
        if !self.model.index_job.is_active() {
            self.start_index();
        }
    }

    fn apply_index(&mut self, index: Arc<AssetIndex>) {
        self.model.apply_index(&index);
        self.index = Some(index);
        if !self.model.query.trim().is_empty() {
            self.request_search();
        }
    }

    // ---- Search ---------------------------------------------------------

    /// The search box changed.
    pub fn set_query(&mut self, query: String) {
        self.model.set_query(query);
        if self.index.is_some() && !self.model.query.trim().is_empty() {
            self.request_search();
        }
    }

    fn request_search(&mut self) {
        if self.search_in_flight {
            self.search_wanted = true;
            return;
        }
        let Some(index) = self.index.clone() else {
            return;
        };
        self.search_in_flight = true;
        self.search_wanted = false;
        let query = self.model.query.clone();
        let outbox = self.outbox.clone();
        self.pool.spawn(move || {
            let matches = SearchMatches::search(&index, &query);
            outbox.send(Msg::SearchDone { matches });
        });
    }

    fn search_done(&mut self, matches: SearchMatches) {
        self.search_in_flight = false;
        self.model.apply_search(matches);
        if self.search_wanted {
            self.request_search();
        }
    }

    // ---- Preview --------------------------------------------------------

    fn clear_preview_resources(&mut self) {
        self.preview_generation.fetch_add(1, Ordering::Relaxed);
        // Full-size previews are large; thumbnails stay cached for a revisit.
        self.textures.retain(|k| !k.full);
        self.opened = None;
        self.preview_status = None;
        self.thumb_requested.clear();
        self.thumb_failed.clear();
        self.thumb_queue.clear();
        self.detail = None;
        let stop = self.playback.reset();
        self.apply_playback(stop);
    }

    /// Opens the preview for the selection, or closes it.
    fn sync_preview(&mut self) {
        if !self.model.preview_is_stale() {
            return;
        }
        self.clear_preview_resources();
        let Some(row) = self.model.wanted_preview().cloned() else {
            self.model.close_preview();
            self.release_unused_dependencies();
            return;
        };
        let Some(store) = self.store.clone() else {
            return;
        };
        let generation = self.preview_generation.load(Ordering::Relaxed);
        self.model.open_preview(row.id);
        // Before the new preview starts parsing its own dependencies.
        self.release_unused_dependencies();
        let current = Arc::clone(&self.preview_generation);
        let outbox = self.outbox.clone();
        self.pool.spawn(move || {
            tasks::open_package(&store, &row.name, row.kind, generation, &current, &outbox);
        });
    }

    /// Drops the dependency Bundles the stores keep parsed once the model
    /// says nothing needs them, so memory does not grow while browsing.
    fn release_unused_dependencies(&mut self) {
        if self.model.take_dependency_release() {
            for store in [&self.store, &self.diff_store].into_iter().flatten() {
                store.release_dependencies();
            }
        }
    }

    /// Asks for a thumbnail of the preview's Asset `path_id`; decoded in
    /// the background, batched per frame.
    pub fn want_thumbnail(&mut self, path_id: i64) {
        if self.thumb_requested.insert(path_id) {
            self.thumb_queue.push(path_id);
        }
    }

    fn flush_thumbnail_requests(&mut self) {
        if self.thumb_queue.is_empty() {
            return;
        }
        let Some(Opened::Bundle { bundle, .. }) = &self.opened else {
            self.thumb_queue.clear();
            return;
        };
        let generation = self.preview_generation.load(Ordering::Relaxed);
        for chunk in self.thumb_queue.chunks(THUMBNAILS_PER_TASK) {
            let bundle = Arc::clone(bundle);
            let ids = chunk.to_vec();
            let current = Arc::clone(&self.preview_generation);
            let outbox = self.outbox.clone();
            self.pool.spawn(move || {
                tasks::thumbnails(&bundle, &ids, generation, &current, &outbox);
            });
        }
        self.thumb_queue.clear();
    }

    /// The thumbnail texture of a preview Asset, if decoded.
    pub fn thumbnail(&mut self, package: &str, path_id: i64) -> Option<TextureHandle> {
        self.textures
            .get(&TexKey {
                package: package.to_owned(),
                path_id,
                full: false,
            })
            .cloned()
    }

    fn upload(&mut self, ctx: &egui::Context, key: TexKey, image: ColorImage, nearest: bool) {
        let bytes = rgba_bytes(image.width(), image.height());
        let options = if nearest {
            TextureOptions::NEAREST
        } else {
            TextureOptions::LINEAR
        };
        let name = format!("{}/{}", key.package, key.path_id);
        let handle = ctx.load_texture(name, image, options);
        // Evicted handles drop here, which frees their GPU textures.
        drop(self.textures.insert(key, handle, bytes));
    }

    // ---- Detail viewer --------------------------------------------------

    /// Opens the detail viewer on a preview Asset.
    pub fn open_detail(&mut self, asset: &AssetInfo) {
        let Some(Opened::Bundle { bundle, .. }) = &self.opened else {
            return;
        };
        let package = bundle.name().to_owned();
        let bundle = Arc::clone(bundle);
        if let Some(old) = &self.detail {
            let stop = self.playback.stop_sound(SoundRef::Clip(old.asset.path_id));
            self.apply_playback(stop);
        }
        self.detail_counter += 1;
        let generation = self.detail_counter;
        self.detail = Some(Detail {
            asset: asset.clone(),
            package: package.clone(),
            generation,
            content: None,
            transform: ViewTransform::default(),
            fitted: false,
            channels: Channels::ALL,
            shown_channels: Channels::ALL,
            selected_sprite: None,
            camera: OrbitCamera::default(),
            open: true,
        });
        let asset = asset.clone();
        let max_side = self.max_texture_side;
        let outbox = self.outbox.clone();
        self.pool.spawn(move || {
            let result = tasks::detail(&bundle, &asset, max_side);
            outbox.send(Msg::Detail { generation, result });
        });
    }

    /// The detail texture, if uploaded.
    pub fn detail_texture(&mut self) -> Option<TextureHandle> {
        let detail = self.detail.as_ref()?;
        let key = TexKey {
            package: detail.package.clone(),
            path_id: detail.asset.path_id,
            full: true,
        };
        self.textures.get(&key).cloned()
    }

    /// Forgets a detail viewer the user closed, freeing its texture.
    fn drop_closed_detail(&mut self) {
        if let Some(detail) = self.detail.take_if(|d| !d.open) {
            let stop = self
                .playback
                .stop_sound(SoundRef::Clip(detail.asset.path_id));
            self.apply_playback(stop);
            self.textures.remove(&TexKey {
                package: detail.package,
                path_id: detail.asset.path_id,
                full: true,
            });
        }
    }

    /// The channel toggles changed: remask in the background.
    pub fn remask_detail(&mut self) {
        let Some(detail) = &self.detail else { return };
        let image = match &detail.content {
            Some(Ok(
                DetailContent::Image(image)
                | DetailContent::Atlas {
                    image: Some(image), ..
                },
            )) => Arc::clone(&image.shown),
            _ => return,
        };
        let (generation, channels) = (detail.generation, detail.channels);
        let outbox = self.outbox.clone();
        self.pool.spawn(move || {
            let color = tasks::remask(&image, channels);
            outbox.send(Msg::DetailMasked { generation, color });
        });
        if let Some(detail) = &mut self.detail {
            detail.shown_channels = channels;
        }
    }

    // ---- Playback -------------------------------------------------------

    /// The play control of a preview sound was pressed: plays it, or stops
    /// it when it is the one playing.
    pub fn toggle_sound(&mut self, sound: SoundRef) {
        let action = self.playback.toggle(sound);
        self.apply_playback(action);
    }

    /// Carries out what the playback model asked for.
    fn apply_playback(&mut self, action: Action) {
        self.playback_current
            .store(self.playback.generation(), Ordering::Relaxed);
        match action {
            Action::None => {}
            Action::Stop => self.audio.stop(),
            Action::Decode { sound, generation } => {
                // The old sound stops now, not when the new one is ready.
                self.audio.stop();
                let Some(from) = self.opened.as_ref().map(Opened::sound_source) else {
                    self.playback
                        .decoded(generation, Err("nothing is open".to_owned()));
                    return;
                };
                let current = Arc::clone(&self.playback_current);
                let outbox = self.outbox.clone();
                self.pool.spawn(move || {
                    if current.load(Ordering::Relaxed) != generation {
                        return;
                    }
                    let result = tasks::decode_sound(&from, sound);
                    outbox.send(Msg::SoundDecoded { generation, result });
                });
            }
        }
    }

    fn sound_decoded(&mut self, generation: u64, result: Result<ae3_core::ae3_audio::Pcm, String>) {
        let (outcome, pcm) = match result {
            Ok(pcm) => (Ok(()), Some(pcm)),
            Err(e) => (Err(e), None),
        };
        if self.playback.decoded(generation, outcome)
            && let Some(pcm) = pcm
        {
            self.audio.play(generation, pcm);
        }
    }

    /// A checkerboard tile for transparent backgrounds.
    pub fn checker(&mut self, ctx: &egui::Context) -> TextureHandle {
        self.checker
            .get_or_insert_with(|| {
                let (a, b) = (egui::Color32::from_gray(200), egui::Color32::from_gray(150));
                let image = ColorImage::new([2, 2], vec![a, b, b, a]);
                ctx.load_texture("checker", image, TextureOptions::NEAREST_REPEAT)
            })
            .clone()
    }

    // ---- Extraction -----------------------------------------------------

    /// Starts an extraction of `request` from `store` (the session's or the
    /// Diff's newer Asset Version).
    pub fn extract(&mut self, request: ExtractRequest, from_diff: bool) {
        if self.model.extract_job.is_active() || request.is_empty() {
            return;
        }
        let store = if from_diff {
            self.diff_store.clone()
        } else {
            self.store.clone()
        };
        let Some(store) = store else { return };
        let generation = self.model.extract_job.start(Instant::now());
        self.model.extract_label = request.describe();
        self.model.summary = None;
        let cancel = Arc::new(AtomicBool::new(false));
        self.extract_cancel = Some(Arc::clone(&cancel));
        let options = ExportOptions::from_config(&self.config);
        let outbox = self.outbox.clone();
        spawn_long("ae3-extract", move || {
            tasks::extract(&store, &request, &options, generation, &cancel, &outbox);
        });
    }

    pub fn cancel_extract(&mut self) {
        if self.model.extract_job.request_cancel()
            && let Some(cancel) = &self.extract_cancel
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn reveal_output(&mut self) {
        let dir = self
            .model
            .summary
            .as_ref()
            .map_or_else(|| self.config.output_dir.clone(), |s| s.output_dir.clone());
        if let Err(e) = tasks::reveal(&dir) {
            self.model.notice = Some(e);
        }
    }

    // ---- Diff -----------------------------------------------------------

    pub fn compute_diff(&mut self) {
        let Some(store) = self.store.clone() else {
            self.model.notice = Some("Load a Manifest first.".into());
            return;
        };
        let old = match crate::model::diff::parse_version(&self.model.diff.old_text) {
            Ok(Some(v)) => v,
            Ok(None) => {
                self.model.notice = Some("Enter the older Asset Version to compare.".into());
                return;
            }
            Err(e) => {
                self.model.notice = Some(e);
                return;
            }
        };
        let new = match crate::model::diff::parse_version(&self.model.diff.new_text) {
            Ok(v) => v,
            Err(e) => {
                self.model.notice = Some(e);
                return;
            }
        };
        let generation = self.model.diff.job.start(Instant::now());
        let outbox = self.outbox.clone();
        spawn_long("ae3-diff", move || {
            let result = tasks::diff(&store, old, new);
            outbox.send(Msg::DiffDone { generation, result });
        });
    }

    // ---- Settings -------------------------------------------------------

    /// Applies a settings change and schedules saving it.
    pub fn change_setting(&mut self, change: SettingChange) {
        let s = &mut self.config_file.settings;
        match change {
            SettingChange::Platform(v) => {
                self.config.platform = v;
                s.platform = Some(v);
                // The startup load is for the Platform it started with.
                self.model.startup.abandon();
            }
            SettingChange::ImageFormat(v) => {
                self.config.image_format = v;
                s.image_format = Some(v);
            }
            SettingChange::AtlasCutting(v) => {
                self.config.atlas_cutting = v;
                s.atlas_cutting = Some(v);
            }
            SettingChange::PaddingRestoration(v) => {
                self.config.padding_restoration = v;
                s.padding_restoration = Some(v);
            }
            SettingChange::AudioFormat(v) => {
                self.config.audio_format = v;
                s.audio_format = Some(v);
            }
            SettingChange::Layout(v) => {
                self.config.layout = v;
                s.layout = Some(v);
            }
            SettingChange::SkipExisting(v) => {
                self.config.skip_existing = v;
                s.skip_existing = Some(v);
            }
            SettingChange::Structured(v) => {
                self.config.structured = v;
                s.structured = Some(v);
            }
            SettingChange::MeshFormat(v) => {
                self.config.mesh_format = v;
                s.mesh_format = Some(v);
            }
        }
        self.config_dirty_since = Some(Instant::now());
    }

    pub fn set_output_dir(&mut self, dir: PathBuf) {
        self.config.output_dir.clone_from(&dir);
        self.config_file.settings.output_dir = Some(dir);
        self.config_dirty_since = Some(Instant::now());
    }

    fn save_config(&mut self) {
        self.config_dirty_since = None;
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let file = self.config_file.clone();
        let outbox = self.outbox.clone();
        self.pool.spawn(move || {
            outbox.send(Msg::ConfigSaved(tasks::save_config(&file, &path)));
        });
    }

    // ---- Messages -------------------------------------------------------

    fn handle(&mut self, ctx: &egui::Context, msg: Msg) {
        let now = Instant::now();
        match msg {
            Msg::ManifestLoaded { generation, result } => {
                let outcome = result.as_ref().map(|_| false).map_err(Clone::clone);
                if self.model.manifest_job.finish(generation, outcome) {
                    match result {
                        Ok(store) => {
                            self.last_load_error = None;
                            self.manifest_loaded(store);
                        }
                        Err(e) => self.last_load_error = Some(e),
                    }
                }
            }
            Msg::StartupCached { generation, store } => self.startup_cached(generation, store),
            Msg::StartupLatest { generation, result } => self.startup_latest(generation, result),
            Msg::UpdateChecked { latest } => self.model.update.finished(latest),
            Msg::IndexSnapshot { generation, index } => {
                if generation == self.model.index_job.generation() {
                    self.apply_index(index);
                }
            }
            Msg::IndexProgress {
                generation,
                view,
                paused,
            } => self.index_progress(generation, view, paused, now),
            Msg::IndexDone { generation, result } => self.index_done(generation, result),
            Msg::SearchDone { matches } => self.search_done(matches),
            Msg::PreviewStatus { generation, text } => {
                if self.preview_generation.load(Ordering::Relaxed) == generation {
                    self.preview_status = Some(text);
                }
            }
            Msg::PreviewOpened { generation, result } => self.preview_opened(generation, result),
            Msg::Thumbnail {
                generation,
                package,
                path_id,
                result,
            } => self.thumbnail_arrived(ctx, generation, package, path_id, result),
            Msg::Detail { generation, result } => self.detail_arrived(ctx, generation, result),
            Msg::DetailMasked { generation, color } => {
                let Some(detail) = &self.detail else { return };
                if detail.generation != generation {
                    return;
                }
                let key = TexKey {
                    package: detail.package.clone(),
                    path_id: detail.asset.path_id,
                    full: true,
                };
                self.upload(ctx, key, color, true);
            }
            Msg::SoundDecoded { generation, result } => self.sound_decoded(generation, result),
            Msg::PlaybackEnded { generation, error } => self.playback.ended(generation, error),
            Msg::ExtractProgress {
                generation,
                progress,
            } => {
                self.model.extract_job.update(generation, progress);
            }
            Msg::ExtractDone {
                generation,
                summary,
            } => {
                let outcome = if summary.failure.is_some() {
                    Err(summary.headline())
                } else {
                    Ok(summary.cancelled)
                };
                if self.model.extract_job.finish(generation, outcome) {
                    self.extract_cancel = None;
                    self.model.summary = Some(summary);
                    self.model.extraction_ended();
                    self.release_unused_dependencies();
                }
            }
            Msg::DiffDone { generation, result } => {
                let outcome = result.as_ref().map(|_| false).map_err(Clone::clone);
                if self.model.diff.job.finish(generation, outcome)
                    && let Ok((diff, store)) = result
                {
                    self.model.diff.set_diff(diff);
                    self.diff_store = Some(store);
                }
            }
            Msg::ReleaseChecked(state) => self.model.release = Some(state),
            Msg::ManualReleaseChecked { state, result } => {
                self.manual_release_checked(state, &result);
            }
            Msg::NamesProgress(step) => self.model.names_state.progress(step),
            Msg::NamesLoaded { index, state } => self.model.set_names(index, state),
            Msg::ConfigSaved(result) => {
                if let Err(e) = result {
                    self.model.notice = Some(format!("Could not save settings: {e}"));
                }
            }
        }
    }

    fn index_progress(
        &mut self,
        generation: u64,
        view: crate::model::IndexProgressView,
        paused: bool,
        now: Instant,
    ) {
        self.model.index_job.update(generation, view);
        let phase_paused = self.model.index_job.phase() == &crate::model::jobs::JobPhase::Paused;
        if paused != phase_paused {
            self.model.index_job.set_paused(generation, paused, now);
        }
    }

    fn index_done(&mut self, generation: u64, result: Result<tasks::IndexOutcome, String>) {
        let outcome = match &result {
            Ok(o) => Ok(o.cancelled),
            Err(e) => Err(e.clone()),
        };
        if self.model.index_job.finish(generation, outcome)
            && let Ok(outcome) = result
        {
            if let Some(e) = outcome.save_error {
                self.model.notice = Some(format!("Could not save the Asset Index: {e}"));
            }
            self.apply_index(outcome.index);
        }
    }

    fn preview_opened(&mut self, generation: u64, result: Result<Opened, String>) {
        if self.preview_generation.load(Ordering::Relaxed) != generation {
            return;
        }
        self.preview_status = None;
        let Some(preview) = &mut self.model.preview else {
            return;
        };
        let row_id = preview.row_id;
        match result {
            Ok(opened) => {
                match &opened {
                    Opened::Bundle { assets, kinds, .. } => {
                        preview.set_assets(assets.clone());
                        let kinds = *kinds;
                        self.model.learn_kinds(row_id, kinds);
                    }
                    Opened::Audio { sounds, .. } => {
                        for sound in sounds {
                            if let Some(why) = unplayable_sound(sound) {
                                self.playback
                                    .set_unplayable(SoundRef::Sound(sound.index), why);
                            }
                        }
                        preview.set_sounds(sounds.clone());
                    }
                }
                self.opened = Some(opened);
            }
            Err(e) => preview.status = crate::model::preview::PreviewStatus::Failed(e),
        }
    }

    fn thumbnail_arrived(
        &mut self,
        ctx: &egui::Context,
        generation: u64,
        package: String,
        path_id: i64,
        result: Result<ColorImage, String>,
    ) {
        if self.preview_generation.load(Ordering::Relaxed) != generation {
            return;
        }
        // Allow a later re-request if this one is evicted.
        self.thumb_requested.remove(&path_id);
        match result {
            Ok(image) => {
                let key = TexKey {
                    package,
                    path_id,
                    full: false,
                };
                self.upload(ctx, key, image, false);
            }
            Err(e) => {
                self.thumb_failed.insert(path_id, e);
            }
        }
    }

    fn detail_arrived(
        &mut self,
        ctx: &egui::Context,
        generation: u64,
        result: Result<DetailContent, String>,
    ) {
        let Some(detail) = &mut self.detail else {
            return;
        };
        if detail.generation != generation {
            return;
        }
        let image = match &result {
            Ok(
                DetailContent::Image(image)
                | DetailContent::Atlas {
                    image: Some(image), ..
                },
            ) => Some(image.color.clone()),
            _ => None,
        };
        let key = TexKey {
            package: detail.package.clone(),
            path_id: detail.asset.path_id,
            full: true,
        };
        if let Ok(DetailContent::Audio {
            unplayable: Some(why),
            ..
        }) = &result
        {
            self.playback
                .set_unplayable(SoundRef::Clip(detail.asset.path_id), why.clone());
        }
        detail.content = Some(result);
        if let Some(image) = image {
            self.upload(ctx, key, image, true);
        }
    }

    /// The summary of the last extraction, failing ones included.
    #[must_use]
    pub fn summary(&self) -> Option<&ExtractSummary> {
        self.model.summary.as_ref()
    }
}

/// Repaint policy: the window repaints only when something changed. Worker
/// messages wake it ([`Outbox`]; progress at most every
/// [`tasks::PROGRESS_INTERVAL`]), input wakes it, and the only timers are the
/// Asset Version recheck (focused windows, every 15 minutes) and the
/// settings save a second after a change. Nothing animates while idle:
/// spinners are drawn only while a short job runs that the user waits for,
/// and the progress bars are not animated. `idle_frames_schedule_no_repaint`
/// checks this.
impl Ae3App {
    /// The per-frame work outside drawing: messages, timers, saving.
    fn tick(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.inbox.try_recv() {
            self.handle(ctx, msg);
        }
        let now = Instant::now();
        let focused = ctx.input(|i| i.focused);
        self.max_texture_side = u32::try_from(ctx.input(|i| i.max_texture_side)).unwrap_or(2048);
        if self.model.update.is_due(now, focused) {
            self.check_for_update(now);
        }
        if let Some(wait) = self.model.next_wakeup(now, focused) {
            ctx.request_repaint_after(wait);
        }
        if let Some(since) = self.config_dirty_since {
            let waited = now.saturating_duration_since(since);
            if waited >= SAVE_DELAY {
                self.save_config();
            } else {
                ctx.request_repaint_after(SAVE_DELAY.saturating_sub(waited));
            }
        }
        self.drop_closed_detail();
    }

    /// Draws the window and acts on what the drawing changed.
    fn draw(&mut self, ui: &mut egui::Ui) {
        crate::ui::show(self, ui);
        // Selection changes made while drawing take effect here.
        self.sync_preview();
        self.flush_thumbnail_requests();
    }
}

impl eframe::App for Ae3App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.tick(ctx);
        // GPU buffers are kept only while a Mesh is on screen.
        let showing_mesh = matches!(
            &self.detail,
            Some(Detail {
                content: Some(Ok(DetailContent::Mesh(_))),
                ..
            })
        );
        if !showing_mesh && let Some(gl) = frame.gl() {
            mesh::lock(&self.mesh_renderer).release_mesh(gl);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(gl) = gl {
            mesh::lock(&self.mesh_renderer).destroy(gl);
        }
        self.audio.stop();
        if self.config_dirty_since.is_some()
            && let Some(path) = &self.config_path
        {
            let _ = tasks::save_config(&self.config_file, path);
        }
        // Stop background jobs; the index build saves what it probed.
        self.index_cancel.store(true, Ordering::Relaxed);
        if let Some(cancel) = &self.extract_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.preview_generation.fetch_add(1, Ordering::Relaxed);
        if let Some(handle) = self.index_thread.take() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc::RecvTimeoutError;

    use ae3_core::{AssetKind, LoadedBundle, Manifest};
    use eframe::egui::{RawInput, Rect, pos2};

    use super::*;
    use crate::model::selection::ClickModifiers;

    fn fixtures() -> Option<PathBuf> {
        let root = std::env::var_os("AE3_FIXTURES").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            PathBuf::from,
        );
        let dir = root.join("100054/Windows");
        if dir.join("manifest.data").exists() {
            Some(dir)
        } else {
            eprintln!("skipping: no fixtures at {}", dir.display());
            None
        }
    }

    /// Draws one frame without a window.
    fn frame(ctx: &egui::Context, app: &mut Ae3App) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(1280.0, 860.0))),
            ..RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            crate::ui::show(app, ui);
            app.flush_thumbnail_requests();
        });
        // No renderer: drop the texture uploads.
        output.textures_delta.clear();
    }

    /// Handles messages until `done` holds or a timeout.
    fn pump(ctx: &egui::Context, app: &mut Ae3App, done: impl Fn(&Ae3App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done(app) {
            let left = deadline.saturating_duration_since(Instant::now());
            match app.inbox.recv_timeout(left) {
                Ok(msg) => app.handle(ctx, msg),
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for workers"),
                Err(RecvTimeoutError::Disconnected) => break,
            }
            frame(ctx, app);
        }
    }

    /// A download cache in the temporary directory, removed on drop.
    struct TempCache(PathBuf);

    impl TempCache {
        /// With the fixture Manifest filed as Asset Version 100054 of
        /// Windows in the PROD Environment, as `PackageCache` files it.
        fn with_manifest(name: &str, manifest: Option<&Path>) -> Self {
            let dir = std::env::temp_dir().join(format!("ae3-gui-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            if let Some(manifest) = manifest {
                let target = dir.join("manifests/PROD/Windows");
                std::fs::create_dir_all(&target).unwrap();
                std::fs::copy(manifest, target.join("100054.data")).unwrap();
            }
            Self(dir)
        }
    }

    impl Drop for TempCache {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An offline app over `cache` that last showed `last_shown`, with its
    /// startup load begun; returns the load's generation.
    fn starting_app(
        ctx: &egui::Context,
        cache: &TempCache,
        last_shown: Option<AssetVersion>,
    ) -> (Ae3App, u64) {
        let config = Config {
            cache_dir: Some(cache.0.clone()),
            ..Config::default()
        };
        let mut file = ConfigFile::default();
        file.gui.last_asset_version = last_shown;
        let mut app = Ae3App::new(ctx, config, file, None, None);
        app.index_on_load = false;
        let generation = app.begin_startup();
        (app, generation)
    }

    /// A store over the fixture Manifest relabelled as Asset Version
    /// `version`, as if the CDN had published it.
    fn published(app: &Ae3App, version: u64) -> Store {
        let cached = app.store.as_ref().expect("a cached Manifest is shown");
        let mut manifest = cached.manifest().clone();
        manifest.asset_version = AssetVersion(version);
        let location = ae3_core::CdnLocation {
            asset_version: AssetVersion(version),
            ..cached.location().clone()
        };
        Store::new(
            Arc::clone(cached.cache()),
            location,
            Platform::Windows,
            manifest,
        )
    }

    #[test]
    fn startup_opens_the_cached_manifest_then_switches_to_a_newer_one() {
        let Some(dir) = fixtures() else { return };
        let cache = TempCache::with_manifest("startup-newer", Some(&dir.join("manifest.data")));
        let ctx = egui::Context::default();
        let (mut app, generation) = starting_app(&ctx, &cache, Some(AssetVersion(100_054)));
        assert!(app.model.manifest_job.is_active());

        let cached = tasks::open_cached(&app.services, Platform::Windows, AssetVersion(100_054))
            .expect("the cached Manifest opens without the network");
        assert_eq!(cached.location().environment.as_str(), "PROD");
        assert!(
            tasks::open_cached(&app.services, Platform::Android, AssetVersion(100_054)).is_none()
        );
        assert!(
            tasks::open_cached(&app.services, Platform::Windows, AssetVersion(100_053)).is_none()
        );
        app.handle(
            &ctx,
            Msg::StartupCached {
                generation,
                store: Some(cached),
            },
        );
        assert!(!app.model.manifest_job.is_active(), "usable at once");
        assert_eq!(app.model.asset_version(), Some(AssetVersion(100_054)));
        assert!(app.model.startup.is_pending());
        frame(&ctx, &mut app);

        let newer = published(&app, 100_055);
        app.handle(
            &ctx,
            Msg::StartupLatest {
                generation,
                result: Ok(tasks::Latest::Opened(newer)),
            },
        );
        assert_eq!(app.model.asset_version(), Some(AssetVersion(100_055)));
        assert_eq!(app.model.version_text, "100055");
        assert!(!app.model.startup.is_pending());
        assert_eq!(app.model.update.newer(), None);
        assert_eq!(
            app.config_file.gui.last_asset_version,
            Some(AssetVersion(100_055)),
            "remembered for the next start"
        );
        frame(&ctx, &mut app);
    }

    #[test]
    fn startup_only_offers_a_newer_version_once_the_user_moved_on() {
        let Some(dir) = fixtures() else { return };
        let cache = TempCache::with_manifest("startup-offer", Some(&dir.join("manifest.data")));
        let ctx = egui::Context::default();
        let (mut app, generation) = starting_app(&ctx, &cache, Some(AssetVersion(100_054)));
        let cached = tasks::open_cached(&app.services, Platform::Windows, AssetVersion(100_054));
        app.handle(
            &ctx,
            Msg::StartupCached {
                generation,
                store: cached,
            },
        );
        // The user picks another Platform before the latest arrives.
        app.change_setting(SettingChange::Platform(Platform::Android));
        let newer = published(&app, 100_055);
        app.handle(
            &ctx,
            Msg::StartupLatest {
                generation,
                result: Ok(tasks::Latest::Opened(newer)),
            },
        );
        assert_eq!(app.model.asset_version(), Some(AssetVersion(100_054)));
        assert_eq!(app.model.update.newer(), Some(AssetVersion(100_055)));
    }

    #[test]
    fn startup_keeps_the_cached_manifest_when_offline() {
        let Some(dir) = fixtures() else { return };
        let cache = TempCache::with_manifest("startup-offline", Some(&dir.join("manifest.data")));
        let ctx = egui::Context::default();
        let (mut app, generation) = starting_app(&ctx, &cache, Some(AssetVersion(100_054)));
        let cached = tasks::open_cached(&app.services, Platform::Windows, AssetVersion(100_054));
        app.handle(
            &ctx,
            Msg::StartupCached {
                generation,
                store: cached,
            },
        );
        app.handle(
            &ctx,
            Msg::StartupLatest {
                generation,
                result: Err("no network".into()),
            },
        );
        assert_eq!(app.model.asset_version(), Some(AssetVersion(100_054)));
        assert!(!app.model.startup.is_pending());
        assert!(app.last_load_error.is_none());
        assert!(
            app.model
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("no network")),
            "{:?}",
            app.model.notice
        );

        // The same version resolved: the resolved location is adopted and
        // nothing is reloaded.
        let (mut app, generation) = starting_app(&ctx, &cache, Some(AssetVersion(100_054)));
        let cached = tasks::open_cached(&app.services, Platform::Windows, AssetVersion(100_054));
        app.handle(
            &ctx,
            Msg::StartupCached {
                generation,
                store: cached,
            },
        );
        app.model.click_row(0, ClickModifiers::default());
        let location = ae3_core::CdnLocation {
            base_url: "https://mirror.invalid/PROD/".into(),
            ..app.store.as_ref().unwrap().location().clone()
        };
        app.handle(
            &ctx,
            Msg::StartupLatest {
                generation,
                result: Ok(tasks::Latest::Located(location)),
            },
        );
        assert_eq!(app.model.selection.len(), 1, "the table was not reset");
        assert_eq!(
            app.store.as_ref().unwrap().location().base_url,
            "https://mirror.invalid/PROD/"
        );
    }

    /// Runs a few whole frames (as eframe does: `tick`, then `draw`) a
    /// quarter second apart, so egui's own short animations (a scroll bar
    /// appearing) end, and returns the repaint delay the last one asked
    /// for: `Duration::MAX` when nothing is scheduled, zero for a
    /// continuous repaint.
    fn settled_repaint_delay(ctx: &egui::Context, app: &mut Ae3App) -> Duration {
        let mut delay = Duration::ZERO;
        for _ in 0..6 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(1280.0, 860.0))),
                time: Some(ctx.cumulative_frame_nr() as f64 * 0.25),
                ..RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                app.tick(ui.ctx());
                app.draw(ui);
            });
            output.textures_delta.clear();
            delay = output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map_or(Duration::MAX, |v| v.repaint_delay);
        }
        delay
    }

    /// The window idles at no CPU: once a frame has settled, nothing asks
    /// for another before the 15-minute Asset Version recheck, whether
    /// nothing is loaded, a Manifest is shown, the Asset Index builds,
    /// pauses or has finished, the startup load looks for a newer Asset
    /// Version, or an extraction runs between progress messages.
    /// Longer than any wait a settled window may schedule, short of the
    /// Asset Version recheck.
    const IDLE: Duration = Duration::from_secs(60);

    /// An offline app showing a small made-up Manifest.
    fn app_with_manifest(ctx: &egui::Context) -> Ae3App {
        let mut app = Ae3App::new(ctx, Config::default(), ConfigFile::default(), None, None);
        app.index_on_load = false;
        let package = |name: &str, kind| ae3_core::Package {
            name: name.into(),
            kind,
            size: 1000,
            ..ae3_core::Package::default()
        };
        let manifest = Manifest {
            asset_version: AssetVersion(100_054),
            packages: (0..50)
                .map(|i| package(&format!("charui_{i}"), ae3_core::PackageKind::Bundle))
                .chain([package("audio_x", ae3_core::PackageKind::AudioPackage)])
                .collect(),
            ..Manifest::default()
        };
        app.model.set_manifest(Arc::new(manifest), Instant::now());
        app
    }

    #[test]
    fn idle_frames_schedule_no_repaint() {
        let ctx = egui::Context::default();
        let mut app = Ae3App::new(&ctx, Config::default(), ConfigFile::default(), None, None);
        assert!(
            settled_repaint_delay(&ctx, &mut app) >= IDLE,
            "empty window"
        );

        let mut app = app_with_manifest(&ctx);
        let now = Instant::now();
        let delay = settled_repaint_delay(&ctx, &mut app);
        assert!(
            delay >= IDLE && delay <= crate::model::update::RECHECK_INTERVAL,
            "Manifest shown: {delay:?}"
        );

        // The startup load still resolving the latest shows no spinner.
        let generation = app.begin_startup();
        assert!(app.model.manifest_job.finish(generation, Ok(false)));
        assert!(app.model.startup.is_pending());
        assert!(
            settled_repaint_delay(&ctx, &mut app) >= IDLE,
            "startup check"
        );
        app.model.startup.abandon();

        let generation = app.model.index_job.start(now);
        app.model.index_job.update(
            generation,
            crate::model::IndexProgressView {
                done: 10,
                total: 51,
                bytes_fetched: 1 << 20,
                ..Default::default()
            },
        );
        assert!(
            settled_repaint_delay(&ctx, &mut app) >= IDLE,
            "index building"
        );

        // A progress message wakes the window once, then it idles again.
        app.outbox.send(Msg::IndexProgress {
            generation,
            view: crate::model::IndexProgressView {
                done: 11,
                total: 51,
                ..Default::default()
            },
            paused: false,
        });
        assert!(ctx.has_requested_repaint(), "the message woke the window");
        assert!(settled_repaint_delay(&ctx, &mut app) >= IDLE);
        assert_eq!(app.model.index_job.progress().done, 11);

        assert!(app.model.index_job.set_paused(generation, true, now));
        assert!(
            settled_repaint_delay(&ctx, &mut app) >= IDLE,
            "index paused"
        );
        assert!(app.model.index_job.finish(generation, Ok(false)));
        assert!(settled_repaint_delay(&ctx, &mut app) >= IDLE, "index done");

        let extraction = app.model.extract_job.start(now);
        assert!(settled_repaint_delay(&ctx, &mut app) >= IDLE, "extracting");
        assert!(app.model.extract_job.finish(extraction, Ok(false)));

        // A spinner, by contrast, repaints continuously while its job runs:
        // the checks above would catch one left on screen.
        app.model.manifest_job.start(now);
        assert_eq!(settled_repaint_delay(&ctx, &mut app), Duration::ZERO);
    }

    /// A Bundle preview waiting for thumbnails draws placeholders, not
    /// spinners, so it idles too.
    #[test]
    fn an_idle_preview_schedules_no_repaint() {
        let ctx = egui::Context::default();
        let mut app = app_with_manifest(&ctx);
        let id = app.model.rows[0].id;
        app.model.click_row(id, ClickModifiers::default());
        app.model.open_preview(id);
        let asset = |path_id: i64, kind| AssetInfo {
            path_id,
            name: format!("asset_{path_id}"),
            class_name: "Texture2D".into(),
            script_class: None,
            kind,
            byte_size: 100,
            texture: None,
            sprite_count: None,
            container_path: None,
        };
        if let Some(preview) = &mut app.model.preview {
            preview.set_assets(vec![
                asset(1, AssetKind::Texture),
                asset(2, AssetKind::Structured),
                asset(3, AssetKind::Mesh),
            ]);
        }
        assert!(
            settled_repaint_delay(&ctx, &mut app) >= IDLE,
            "Bundle preview"
        );
        assert!(app.model.preview.is_some(), "the preview stayed open");
    }

    /// Every text painted in one frame.
    fn painted_texts(ctx: &egui::Context, app: &mut Ae3App) -> Vec<String> {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(1600.0, 900.0))),
            ..RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| crate::ui::show(app, ui));
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn release_notice_links_to_the_release_and_stays_dismissed() {
        let cache = TempCache::with_manifest("release-notice", None);
        let ctx = egui::Context::default();
        let config = Config {
            cache_dir: Some(cache.0.clone()),
            ..Config::default()
        };
        let mut app = Ae3App::new(&ctx, config, ConfigFile::default(), None, None);
        let notice = "swgoh-ae3 v99.0.0 available";
        assert!(!painted_texts(&ctx, &mut app).iter().any(|t| t == notice));
        let mut state = ReleaseState::default();
        state.checked(
            1,
            Some(crate::model::release::Release {
                tag: "v99.0.0".into(),
                url: "https://github.com/swgoh-utils/swgoh-ae3/releases/tag/v99.0.0".into(),
            }),
        );
        app.handle(&ctx, Msg::ReleaseChecked(state));
        assert!(painted_texts(&ctx, &mut app).iter().any(|t| t == notice));

        app.dismiss_release();
        assert!(!painted_texts(&ctx, &mut app).iter().any(|t| t == notice));
        let path = cache.0.join(ReleaseState::FILE);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let saved = ReleaseState::parse(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(
            saved.dismissed.as_deref(),
            Some("v99.0.0"),
            "kept for the next start"
        );
    }

    #[test]
    fn startup_without_a_cached_manifest_waits_for_the_latest() {
        let ctx = egui::Context::default();
        let cache = TempCache::with_manifest("startup-empty", None);
        let (mut app, generation) = starting_app(&ctx, &cache, Some(AssetVersion(100_054)));
        assert!(
            tasks::open_cached(&app.services, Platform::Windows, AssetVersion(100_054)).is_none()
        );
        app.handle(
            &ctx,
            Msg::StartupCached {
                generation,
                store: None,
            },
        );
        assert!(app.model.manifest_job.is_active());
        assert_eq!(
            app.model.manifest_job.progress(),
            "Finding the latest Asset Version…"
        );
        frame(&ctx, &mut app);
        app.handle(
            &ctx,
            Msg::StartupLatest {
                generation,
                result: Err("offline".into()),
            },
        );
        assert!(!app.model.manifest_job.is_active());
        assert_eq!(app.last_load_error.as_deref(), Some("offline"));
        assert!(!app.model.startup.is_pending());
    }

    /// The whole window renders headless: the Manifest table, a fixture
    /// Bundle's preview with decoded thumbnails, and the detail viewer on
    /// an Atlas.
    #[test]
    fn renders_a_fixture_bundle_headless() {
        let Some(dir) = fixtures() else { return };
        let ctx = egui::Context::default();
        let mut app = Ae3App::new(&ctx, Config::default(), ConfigFile::default(), None, None);
        frame(&ctx, &mut app);

        let bytes = std::fs::read(dir.join("manifest.data")).unwrap();
        let manifest = Manifest::decode(&bytes).unwrap();
        app.model.set_manifest(Arc::new(manifest), Instant::now());
        app.set_query("unit_details".into());
        frame(&ctx, &mut app);
        let id = app
            .model
            .rows
            .iter()
            .find(|r| r.name == "unit_details_atlas")
            .map(|r| r.id)
            .expect("unit_details_atlas is in the Manifest");
        assert!(app.model.visible.contains(&id));
        app.model.click_row(id, ClickModifiers::default());

        // Open the preview from the local fixture instead of the CDN.
        let bundle = LoadedBundle::open_file(&dir.join("unit_details_atlas.bundle")).unwrap();
        let assets = bundle.assets().unwrap();
        let kinds = assets.iter().map(|a| a.kind).collect();
        app.model.open_preview(id);
        app.model
            .preview
            .as_mut()
            .unwrap()
            .set_assets(assets.clone());
        app.opened = Some(Opened::Bundle {
            bundle: Arc::new(bundle),
            assets: assets.clone(),
            kinds,
        });
        frame(&ctx, &mut app);
        assert!(
            !app.thumb_requested.is_empty(),
            "visible cells ask for thumbnails"
        );
        pump(&ctx, &mut app, |a| a.thumb_requested.is_empty());
        assert_ne!(app.textures.len(), 0);
        assert!(app.textures.used_bytes() > 0);

        let atlas = assets.iter().find(|a| a.kind == AssetKind::Atlas).unwrap();
        app.open_detail(atlas);
        pump(&ctx, &mut app, |a| {
            a.detail.as_ref().is_some_and(|d| d.content.is_some())
        });
        assert!(app.detail_texture().is_some());
        frame(&ctx, &mut app);
        // Hide alpha; the remasked texture replaces the first.
        if let Some(detail) = &mut app.detail {
            detail.channels.a = false;
        }
        app.remask_detail();
        let before = app.textures.len();
        let msg = app.inbox.recv_timeout(Duration::from_secs(60)).unwrap();
        assert!(matches!(msg, Msg::DetailMasked { .. }));
        app.handle(&ctx, msg);
        assert_eq!(app.textures.len(), before);
        frame(&ctx, &mut app);

        // Closing the detail viewer frees its full-size texture.
        let full = app.textures.len();
        if let Some(detail) = &mut app.detail {
            detail.open = false;
        }
        app.drop_closed_detail();
        assert_eq!(app.textures.len(), full - 1);

        // The Diff tab renders without a Diff.
        app.model.tab = crate::model::Tab::Diff;
        frame(&ctx, &mut app);
    }

    /// The Name Index arriving in the background: progress shows while
    /// nothing is loaded, the matching Game Entities list under the search
    /// box (collapsed, then all of them), and picking one narrows the table.
    #[test]
    fn game_entities_render_and_narrow_the_table() {
        use ae3_core::{EntityKind, GameEntity, GamedataVersions, Language, NameIndex};

        let ctx = egui::Context::default();
        let mut app = Ae3App::new(&ctx, Config::default(), ConfigFile::default(), None, None);
        let package = |name: &str| ae3_core::Package {
            name: name.into(),
            size: 10,
            ..ae3_core::Package::default()
        };
        app.model.set_manifest(
            Arc::new(Manifest {
                asset_version: AssetVersion(100_054),
                packages: vec![package("charui_bossk"), package("vanity_bossk")],
                ..Manifest::default()
            }),
            Instant::now(),
        );
        app.handle(&ctx, Msg::NamesProgress("Downloading units.json…".into()));
        assert_eq!(
            app.model.names_state.line(),
            Some(("Downloading units.json…".to_owned(), false))
        );
        frame(&ctx, &mut app);

        // More matches than are listed at first, one with no Bundle here.
        let mut entities: Vec<GameEntity> = (0..12)
            .map(|i| GameEntity {
                kind: EntityKind::Gear,
                id: format!("gear{i}"),
                name: format!("Bossk Gear {i}"),
                context: None,
                bundles: vec!["missing".into()],
            })
            .collect();
        entities.insert(
            0,
            GameEntity {
                kind: EntityKind::PlayerPortrait,
                id: "PLAYERPORTRAIT_BOSSK".into(),
                name: "Bossk".into(),
                context: None,
                bundles: vec!["vanity_bossk".into()],
            },
        );
        let index = NameIndex::new(Language::default(), GamedataVersions::default(), entities);
        let state = NamesState::new(Some(&index), ae3_core::NameIndexStatus::Built, None, false);
        app.handle(
            &ctx,
            Msg::NamesLoaded {
                index: Some(Arc::new(index)),
                state,
            },
        );
        // A later progress report does not hide the index in use.
        app.handle(&ctx, Msg::NamesProgress("late".into()));
        assert!(matches!(app.model.names_state, NamesState::Ready { .. }));

        app.set_query("bossk".into());
        frame(&ctx, &mut app);
        assert_eq!(app.model.search.as_ref().unwrap().entities.len(), 13);
        app.model.entities_expanded = true;
        frame(&ctx, &mut app);
        let key = app.model.search.as_ref().unwrap().entities[0].key();
        app.model.pick_entity(Some(key));
        frame(&ctx, &mut app);
        assert_eq!(app.model.visible, [1]);
        assert_eq!(
            app.model.search_status().unwrap(),
            "Showing the Bundle of Player Portrait 'Bossk'"
        );
    }

    /// The Output section's Structured Asset choice reaches the export
    /// options and the settings file.
    #[test]
    fn structured_mode_setting_reaches_exports() {
        let ctx = egui::Context::default();
        let mut app = Ae3App::new(&ctx, Config::default(), ConfigFile::default(), None, None);
        let mode = |app: &Ae3App| ExportOptions::from_config(&app.config).structured;
        assert_eq!(mode(&app), StructuredMode::Combined);
        app.change_setting(SettingChange::Structured(StructuredMode::Separate));
        assert_eq!(mode(&app), StructuredMode::Separate);
        assert_eq!(
            app.config_file.settings.structured,
            Some(StructuredMode::Separate)
        );
        assert!(app.config_dirty_since.is_some(), "saved shortly");
        frame(&ctx, &mut app);
    }

    /// So does the Mesh format choice.
    #[test]
    fn mesh_format_setting_reaches_exports() {
        let ctx = egui::Context::default();
        let mut app = Ae3App::new(&ctx, Config::default(), ConfigFile::default(), None, None);
        let format = |app: &Ae3App| ExportOptions::from_config(&app.config).mesh_format;
        assert_eq!(format(&app), MeshFormat::Obj);
        app.change_setting(SettingChange::MeshFormat(MeshFormat::Both));
        assert_eq!(format(&app), MeshFormat::Both);
        assert_eq!(app.config_file.settings.mesh_format, Some(MeshFormat::Both));
        assert!(
            painted_texts(&ctx, &mut app)
                .iter()
                .any(|t| t == "Meshes: OBJ + glb"),
            "the Output section shows the choice"
        );
    }

    /// Opening an Audio Package keeps the preview pane at its own width
    /// instead of spreading over the Bundles table.
    #[test]
    fn audio_preview_keeps_its_width() {
        let ctx = egui::Context::default();
        let mut app = Ae3App::new(&ctx, Config::default(), ConfigFile::default(), None, None);
        let sounds = (0..5_u64)
            .map(|i| ae3_core::SoundInfo {
                index: usize::try_from(i).unwrap(),
                id: 300_573_292 + i,
                stem: format!("{}", 300_573_292 + i),
                language: "sfx".into(),
                source: "stream",
                size: 50_000,
                codec: Some("Wwise Vorbis"),
                channels: Some(2),
                sample_rate: Some(44_100),
                duration_secs: Some(7.0),
            })
            .collect();
        let mut preview = crate::model::preview::Preview::new(0, "audio_pkg_under_test".into());
        preview.set_sounds(sounds);
        app.model.preview = Some(preview);

        let width = 1600.0;
        let mut heading_x = None;
        for _ in 0..10 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(width, 900.0))),
                ..RawInput::default()
            };
            let mut output = ctx.run_ui(input, |ui| crate::ui::show(&mut app, ui));
            output.textures_delta.clear();
            heading_x = output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(t) if t.galley.text() == "audio_pkg_under_test" => {
                        Some(t.pos.x)
                    }
                    _ => None,
                });
        }
        let x = heading_x.expect("the preview heading is painted");
        assert!(
            x > width / 2.0,
            "the preview pane starts at x={x}, covering most of a {width}-wide window"
        );
    }

    /// Cells in a preview row sit side by side. Each cell's checkbox is
    /// placed with `ui.put`, which used to pull the row's cursor back to
    /// the checkbox, so the next cell was drawn almost on top of this one.
    #[test]
    fn preview_cells_do_not_overlap() {
        let ctx = egui::Context::default();
        let mut app = Ae3App::new(&ctx, Config::default(), ConfigFile::default(), None, None);
        let names: Vec<String> = (0..6).map(|i| format!("cell_asset_{i}")).collect();
        let assets = names
            .iter()
            .zip(1_i64..)
            .map(|(name, path_id)| AssetInfo {
                path_id,
                name: name.clone(),
                class_name: "Material".into(),
                script_class: None,
                kind: AssetKind::Structured,
                byte_size: 100,
                texture: None,
                sprite_count: None,
                container_path: None,
            })
            .collect();
        let mut preview = crate::model::preview::Preview::new(0, "test_bundle".into());
        preview.set_assets(assets);
        app.model.preview = Some(preview);

        let input = RawInput {
            screen_rect: Some(Rect::from_min_max(pos2(0.0, 0.0), pos2(1600.0, 900.0))),
            ..RawInput::default()
        };
        let mut output = ctx.run_ui(input, |ui| crate::ui::show(&mut app, ui));
        output.textures_delta.clear();

        // Where each cell's name label was painted.
        let mut labels: Vec<(String, egui::Pos2)> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if names.contains(&text.galley.text().to_owned()) => {
                    Some((text.galley.text().to_owned(), text.pos))
                }
                _ => None,
            })
            .collect();
        labels.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(labels.len(), names.len(), "every cell label is painted");
        for (i, (a, pa)) in labels.iter().enumerate() {
            for (b, pb) in &labels[i + 1..] {
                let same_row = (pa.y - pb.y).abs() < 1.0;
                assert!(
                    !same_row || (pa.x - pb.x).abs() >= 124.0,
                    "{a} at {pa:?} overlaps {b} at {pb:?}"
                );
            }
        }
    }

    /// What a test output was asked to do.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Played {
        Play { generation: u64, frames: usize },
        Stop,
    }

    /// An [`Output`] that records instead of opening a device.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<Played>>>);

    impl Recorder {
        fn take(&self) -> Vec<Played> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl Output for Recorder {
        fn play(&mut self, generation: u64, pcm: ae3_core::ae3_audio::Pcm) {
            self.0.lock().unwrap().push(Played::Play {
                generation,
                frames: pcm.frames(),
            });
        }

        fn stop(&mut self) {
            self.0.lock().unwrap().push(Played::Stop);
        }
    }

    /// Playback end to end without a sound device: Play decodes on the pool
    /// and hands the samples to the output, the control turns into Stop,
    /// nothing animates while the sound plays, the output's end report
    /// resets it, an undecodable codec shows "can't play", and a preview
    /// change stops playback and drops a decode still in flight.
    #[test]
    fn sounds_play_from_the_preview_and_stop_with_it() {
        use crate::model::playback::{PlayState, unplayable_sound};

        let Some(dir) = fixtures() else { return };
        let path = dir.join("audio_sharedui.wwpkg");
        if !path.exists() {
            eprintln!("skipping: no {}", path.display());
            return;
        }
        let ctx = egui::Context::default();
        let mut app = app_with_manifest(&ctx);
        let recorder = Recorder::default();
        app.audio = Box::new(recorder.clone());
        let row = app
            .model
            .rows
            .iter()
            .find(|r| r.name == "audio_x")
            .map(|r| r.id)
            .unwrap();
        app.model.click_row(row, ClickModifiers::default());
        app.model.open_preview(row);

        let package = Arc::new(ae3_core::LoadedAudioPackage::open_file(&path).unwrap());
        let mut sounds = package.sound_infos();
        // The first sound that decodes, so its row is on screen.
        let playable = sounds
            .iter()
            .filter(|s| unplayable_sound(s).is_none())
            .find(|s| {
                package
                    .package()
                    .decode_pcm(&package.sounds()[s.index])
                    .is_ok()
            })
            .map(|s| s.index)
            .expect("a playable sound");
        let opus = sounds.iter().position(|s| s.index != playable).unwrap();
        sounds[opus].codec = Some("Opus");
        let generation = app.preview_generation.load(Ordering::Relaxed);
        app.preview_opened(generation, Ok(Opened::Audio { package, sounds }));
        let texts = painted_texts(&ctx, &mut app);
        let play = crate::ui::icons::with(crate::ui::icons::PLAY, "Play");
        assert!(texts.contains(&play), "{texts:?}");
        assert!(texts.iter().any(|t| t == "can't play"), "{texts:?}");
        assert_eq!(
            app.playback.state(SoundRef::Sound(opus)),
            PlayState::Unplayable("Opus is not decoded")
        );

        let sound = SoundRef::Sound(playable);
        app.toggle_sound(sound);
        assert_eq!(
            app.playback.state(sound),
            PlayState::Decoding,
            "decoding happens on the pool, not in the click"
        );
        pump(&ctx, &mut app, |app| {
            app.playback.state(sound) == PlayState::Playing
        });
        let played = recorder.take();
        let generation = app.playback.generation();
        assert!(
            matches!(
                played.as_slice(),
                [Played::Stop, Played::Play { generation: g, frames }]
                    if *g == generation && *frames > 0
            ),
            "{played:?}"
        );
        let texts = painted_texts(&ctx, &mut app);
        let stop = crate::ui::icons::with(crate::ui::icons::STOP, "Stop");
        assert!(texts.contains(&stop), "{texts:?}");
        assert!(
            settled_repaint_delay(&ctx, &mut app) >= IDLE,
            "playing schedules no repaint; the end report wakes the UI"
        );

        app.handle(
            &ctx,
            Msg::PlaybackEnded {
                generation,
                error: None,
            },
        );
        assert_eq!(app.playback.state(sound), PlayState::Idle);

        // Changing the preview stops playback; the decode that was still
        // running is dropped when it arrives.
        app.toggle_sound(sound);
        let generation = app.playback.generation();
        recorder.take();
        app.clear_preview_resources();
        assert_eq!(recorder.take(), [Played::Stop]);
        app.sound_decoded(
            generation,
            Ok(ae3_core::ae3_audio::Pcm {
                channels: 1,
                sample_rate: 48_000,
                samples: vec![0; 480],
            }),
        );
        assert_eq!(recorder.take(), []);
        assert_eq!(app.playback.state(sound), PlayState::Idle);
    }

    /// A WAV AudioClip decodes through the same path as a WEM, and an FSB
    /// one is refused before trying.
    #[test]
    fn audio_clips_decode_only_as_wav() {
        let wav = ae3_core::ae3_audio::Pcm {
            channels: 2,
            sample_rate: 22_050,
            samples: vec![1, -1, 2, -2],
        }
        .to_wav()
        .unwrap();
        assert_eq!(ae3_core::sniff_audio_extension(&wav), "wav");
        let pcm = ae3_core::ae3_audio::decode_wem(&wav).unwrap();
        assert_eq!(
            (pcm.channels, pcm.sample_rate, pcm.frames()),
            (2, 22_050, 2)
        );
        assert!(crate::model::playback::unplayable_clip("fsb").is_some());
    }
}
