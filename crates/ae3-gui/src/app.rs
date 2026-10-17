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

use ae3_core::config::{AudioFormat, ImageFormat, Layout, StructuredMode};
use ae3_core::{AssetIndex, AssetInfo, AssetVersion, Config, ConfigFile, ExportOptions, Platform};
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::model::Model;
use crate::model::extract::{ExtractRequest, ExtractSummary};
use crate::model::lru::{ByteLru, rgba_bytes};
use crate::model::mesh::OrbitCamera;
use crate::model::table::SearchMatches;
use crate::model::viewer::{Channels, ViewTransform};
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
    /// Draws the detail viewer's Mesh; shared with its paint callback.
    pub mesh_renderer: Arc<Mutex<MeshRenderer>>,
    extract_cancel: Option<Arc<AtomicBool>>,
    pub diff_store: Option<Store>,
    checker: Option<TextureHandle>,
    max_texture_side: u32,
    pub last_load_error: Option<String>,
}

impl Ae3App {
    /// Builds the app; `config` and `file` come from `Config::load`.
    pub fn new(
        ctx: &egui::Context,
        config: Config,
        config_file: ConfigFile,
        config_path: Option<PathBuf>,
        notice: Option<String>,
    ) -> Self {
        crate::ui::install_fonts(ctx);
        let repaint = ctx.clone();
        let (outbox, inbox) = Outbox::new(move || repaint.request_repaint());
        let mut model = Model::default();
        model.notice = notice;
        model.version_text = config_file
            .gui
            .last_asset_version
            .map(|v| v.to_string())
            .unwrap_or_default();
        Self {
            model,
            services: Arc::new(Services::new(config.clone())),
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
            mesh_renderer: Arc::default(),
            extract_cancel: None,
            diff_store: None,
            checker: None,
            max_texture_side: 2048,
            last_load_error: None,
        }
    }

    // ---- Manifest -------------------------------------------------------

    /// Resolves and loads a Manifest: the latest when `version` is `None`.
    pub fn load_manifest(&mut self, version: Option<AssetVersion>) {
        if self.model.manifest_job.is_active() {
            return;
        }
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
        self.start_index();
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
        let bundle = Arc::clone(bundle);
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
            Msg::SearchDone { matches } => {
                self.search_in_flight = false;
                self.model.apply_search(matches);
                if self.search_wanted {
                    self.request_search();
                }
            }
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
                    Opened::Audio { sounds } => preview.set_sounds(sounds.clone()),
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

impl eframe::App for Ae3App {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
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
        crate::ui::show(self, ui);
        // Selection changes made while drawing take effect here.
        self.sync_preview();
        self.flush_thumbnail_requests();
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(gl) = gl {
            mesh::lock(&self.mesh_renderer).destroy(gl);
        }
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
}
