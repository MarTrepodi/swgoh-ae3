//! Shared pieces of the AE3 window tests: a fake CDN, a harness that runs
//! the real [`ae3_gui::ui::show`] headless, and the fixture lookup.
//!
//! The tests never reach the network. [`FakeCdn`] answers the requests the
//! app makes: the community `meta.json` (so the latest Asset Version
//! resolves), Manifests and Packages from the fixture directory, and anything
//! a test registers with [`FakeCdn::put`]. With [`FakeCdn::offline`] every
//! request fails instead.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ae3_core::http::{HttpError, RangeResponse, Transport};
use ae3_core::version::{CDN_ROOT, META_URL};
use ae3_core::{AssetVersion, Config, ConfigFile};
use ae3_gui::Ae3App;
use ae3_gui::tasks::{Net, Services};
use eframe::egui;
use egui_kittest::Harness;

/// The Asset Version the fixtures come from.
pub const FIXTURE_VERSION: AssetVersion = AssetVersion(100_054);

/// The window size the tests draw at.
pub const WINDOW: egui::Vec2 = egui::vec2(1280.0, 860.0);

// ---- Fixtures --------------------------------------------------------------

/// Whether `AE3_REQUIRE_FIXTURES` is set (to anything but `0`), as CI does
/// after fetching them: a missing fixture then fails instead of skipping.
#[must_use]
pub fn fixtures_required() -> bool {
    std::env::var_os("AE3_REQUIRE_FIXTURES").is_some_and(|v| v != "0")
}

/// The Windows fixtures of Asset Version 100054: `$AE3_FIXTURES/100054/Windows`,
/// defaulting to `<workspace>/fixtures`.
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    std::env::var_os("AE3_FIXTURES")
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
            PathBuf::from,
        )
        .join(FIXTURE_VERSION.to_string())
        .join("Windows")
}

/// The path of fixture `name`, or `None` (after a note on stderr) when it is
/// absent, so the test can skip.
///
/// # Panics
///
/// When it is absent and [`fixtures_required`].
#[must_use]
pub fn fixture(name: &str) -> Option<PathBuf> {
    let path = fixtures_dir().join(name);
    if path.is_file() {
        return Some(path);
    }
    assert!(
        !fixtures_required(),
        "fixture {} is missing; run scripts/fetch-fixtures.sh",
        path.display()
    );
    eprintln!("skipping: fixture {} not found", path.display());
    None
}

/// A directory under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    /// # Panics
    ///
    /// When the directory cannot be created.
    #[must_use]
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ae3-gui-tests-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating a temporary directory");
        Self(dir)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- A fake CDN ------------------------------------------------------------

/// The CDN base URL of the PROD Environment.
#[must_use]
pub fn prod_base() -> String {
    format!("{CDN_ROOT}PROD/")
}

/// The URL of `file` of `version` for Windows on the PROD Environment.
#[must_use]
pub fn cdn_url(version: AssetVersion, file: &str) -> String {
    format!("{}{version}/Windows/ETC/{file}", prod_base())
}

/// A `meta.json` naming `version` on the PROD Environment.
#[must_use]
pub fn meta_json(version: AssetVersion) -> String {
    format!(
        r#"{{"version":"0.0.0:test","data":{{"assetVersion":{version},"assetPath":"{}","assetSubpath":"{version}/Windows/ETC"}}}}"#,
        prod_base()
    )
}

/// An in-memory CDN; see the crate documentation.
pub struct FakeCdn {
    files: Mutex<HashMap<String, Vec<u8>>>,
    /// Files of `version` not registered are read from here.
    serve_dir: Option<(AssetVersion, PathBuf)>,
    offline: bool,
    requests: Mutex<Vec<String>>,
}

impl FakeCdn {
    /// Answers nothing: every request fails as if the network were down.
    #[must_use]
    pub fn offline() -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::default(),
            serve_dir: None,
            offline: true,
            requests: Mutex::default(),
        })
    }

    /// Answers only what is [`put`](Self::put); everything else is a 404.
    #[must_use]
    pub fn empty() -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::default(),
            serve_dir: None,
            offline: false,
            requests: Mutex::default(),
        })
    }

    /// Publishes the fixture directory as Asset Version 100054, the latest:
    /// `meta.json` names it and its Manifest and Packages are served from
    /// `dir`.
    #[must_use]
    pub fn serving(dir: PathBuf) -> Arc<Self> {
        let cdn = Self {
            files: Mutex::default(),
            serve_dir: Some((FIXTURE_VERSION, dir)),
            offline: false,
            requests: Mutex::default(),
        };
        cdn.put(META_URL, meta_json(FIXTURE_VERSION).as_bytes());
        Arc::new(cdn)
    }

    /// Answers `url` with `body`.
    pub fn put(&self, url: &str, body: &[u8]) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(url.to_owned(), body.to_vec());
    }

    /// This CDN as the app's transport.
    #[must_use]
    pub fn net(self: &Arc<Self>) -> Net {
        Arc::clone(self) as Net
    }

    /// How many requests asked for a URL containing `part`.
    #[must_use]
    pub fn requests_containing(&self, part: &str) -> usize {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|u| u.contains(part))
            .count()
    }

    fn lookup(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(url.to_owned());
        if self.offline {
            return Err(HttpError::Transport {
                url: url.to_owned(),
                message: "the network is unreachable (test)".to_owned(),
            });
        }
        if let Some(body) = self
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(url)
        {
            return Ok(body.clone());
        }
        if let Some((version, dir)) = &self.serve_dir
            && let Some(file) = url.strip_prefix(&format!("{}{version}/", prod_base()))
            && let Some((_, name)) = file.rsplit_once('/')
            && let Ok(body) = std::fs::read(dir.join(name))
        {
            return Ok(body);
        }
        Err(HttpError::Status {
            url: url.to_owned(),
            status: 404,
        })
    }
}

impl Transport for FakeCdn {
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.lookup(url)
    }

    fn get_range(
        &self,
        url: &str,
        start: u64,
        end_inclusive: u64,
    ) -> Result<RangeResponse, HttpError> {
        let body = self.lookup(url)?;
        let len = body.len() as u64;
        if start >= len {
            return Err(HttpError::Status {
                url: url.to_owned(),
                status: 416,
            });
        }
        let start = usize::try_from(start).unwrap_or(usize::MAX);
        let end = usize::try_from(end_inclusive.min(len - 1)).unwrap_or(usize::MAX);
        Ok(RangeResponse {
            bytes: body[start..=end].to_vec(),
            total_len: Some(len),
        })
    }

    fn exists(&self, url: &str) -> Result<bool, HttpError> {
        match self.lookup(url) {
            Ok(_) => Ok(true),
            Err(HttpError::Status {
                status: 403 | 404, ..
            }) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

// ---- The app under test -----------------------------------------------------

/// How to build the app under test. The defaults keep it offline, with its
/// download cache, output and settings in `dir`.
pub struct Spec {
    pub config: Config,
    pub file: ConfigFile,
    /// Where settings are saved; `None` to save nothing.
    pub config_path: Option<PathBuf>,
    pub transport: Net,
    /// Whether to start what the window starts by itself (the startup load,
    /// the Name Index and the release check).
    pub start: bool,
    /// Whether a loaded Manifest starts an Asset Index build.
    pub index: bool,
    /// The platform egui behaves as (keyboard modifiers, shortcuts).
    pub os: egui::os::OperatingSystem,
}

impl Spec {
    /// An app whose download cache and output folder are under `dir`,
    /// reaching the network through `transport`.
    #[must_use]
    pub fn new(dir: &Path, transport: Net) -> Self {
        Self {
            config: Config {
                cache_dir: Some(dir.join("cache")),
                output_dir: dir.join("out"),
                check_for_updates: false,
                ..Config::default()
            },
            file: ConfigFile::default(),
            config_path: None,
            transport,
            start: false,
            index: false,
            os: egui::os::OperatingSystem::Nix,
        }
    }

    /// Builds the app for `ctx`.
    #[must_use]
    pub fn build(self, ctx: &egui::Context) -> Ae3App {
        let services = Services::with_transport(self.config.clone(), self.transport);
        let mut app = Ae3App::with_services(
            ctx,
            self.config,
            self.file,
            self.config_path,
            None,
            services,
        );
        app.set_index_on_load(self.index);
        if self.start {
            app.start();
        }
        app
    }
}

type Make = Box<dyn FnOnce(&egui::Context) -> Ae3App>;

/// The harness state: the app, built on the first frame so it shares the
/// harness's context.
pub struct Gui {
    app: Option<Ae3App>,
    make: Option<Make>,
}

impl Gui {
    /// # Panics
    ///
    /// Before the first frame, which the harness runs when it is built.
    #[must_use]
    pub fn app(&self) -> &Ae3App {
        self.app
            .as_ref()
            .expect("the app is built on the first frame")
    }

    /// # Panics
    ///
    /// Before the first frame.
    pub fn app_mut(&mut self) -> &mut Ae3App {
        self.app
            .as_mut()
            .expect("the app is built on the first frame")
    }

    fn frame(&mut self, ui: &mut egui::Ui) {
        if let Some(make) = self.make.take() {
            self.app = Some(make(ui.ctx()));
        }
        if let Some(app) = &mut self.app {
            app.frame(ui);
        }
    }
}

/// A harness running the app `spec` builds in a [`WINDOW`]-sized window.
#[must_use]
pub fn harness(spec: Spec) -> Harness<'static, Gui> {
    harness_with(spec, WINDOW, egui::Theme::Dark)
}

/// A harness at `size` with `theme`.
#[must_use]
pub fn harness_with(spec: Spec, size: egui::Vec2, theme: egui::Theme) -> Harness<'static, Gui> {
    let os = spec.os;
    harness_from(Box::new(move |ctx| spec.build(ctx)), size, theme, os)
}

/// A harness over the app `make` builds.
#[must_use]
pub fn harness_from(
    make: Make,
    size: egui::Vec2,
    theme: egui::Theme,
    os: egui::os::OperatingSystem,
) -> Harness<'static, Gui> {
    let builder = Harness::builder()
        .with_size(size)
        .with_os(os)
        .with_theme(theme)
        // Real time between frames, so a double click is one.
        .with_step_dt(1.0 / 60.0)
        .with_max_steps(12)
        // The fonts AE3 draws with, before the first layout.
        .with_context_setup(ae3_gui::ui::install);
    // Snapshots show the window, not the test's pointer.
    #[cfg(feature = "snapshots")]
    let builder = builder.with_render_cursor(false);
    builder.build_ui_state(
        |ui, gui: &mut Gui| gui.frame(ui),
        Gui {
            app: None,
            make: Some(make),
        },
    )
}

/// Runs frames until `done` holds for the app, as background work reports
/// in; panics with `what` after a minute.
///
/// # Panics
///
/// When `done` does not hold within a minute.
#[track_caller]
pub fn wait_for(h: &mut Harness<'_, Gui>, what: &str, done: impl Fn(&Ae3App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        h.step();
        if done(h.state().app()) {
            break;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Let what arrived lay out and settle.
    h.run_ok();
}

// ---- Manifests -------------------------------------------------------------

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).unwrap_or_default() | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).unwrap_or_default());
}

fn put_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    put_varint(out, field << 3);
    put_varint(out, value);
}

fn put_bytes_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// Encodes `manifest` as a `manifest.data` file: the Asset Version and, of
/// each Package, its name, Content Hash, size and kind; all the window and
/// the Diff read.
#[must_use]
pub fn encode_manifest(manifest: &ae3_core::Manifest) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint_field(&mut out, 1, manifest.asset_version.0);
    for package in &manifest.packages {
        let mut record = Vec::new();
        put_bytes_field(&mut record, 1, package.name.as_bytes());
        put_varint_field(&mut record, 2, package.content_hash);
        put_varint_field(&mut record, 3, package.size);
        put_varint_field(&mut record, 7, package.kind.code());
        put_bytes_field(&mut out, 2, &record);
    }
    out
}

/// A made-up Package.
#[must_use]
pub fn package(name: &str, kind: ae3_core::PackageKind, size: u64) -> ae3_core::Package {
    ae3_core::Package {
        name: name.to_owned(),
        content_hash: size.wrapping_mul(0x9e37_79b9),
        size,
        kind,
        ..ae3_core::Package::default()
    }
}

/// A small made-up Manifest: invented Bundles in a few Categories and two
/// Audio Packages. No name is a real game Package.
#[must_use]
pub fn synthetic_manifest(version: AssetVersion) -> ae3_core::Manifest {
    use ae3_core::PackageKind::{AudioPackage, Bundle};
    let packages = [
        ("charui_testpilot", Bundle, 48_213),
        ("charui_testgunner", Bundle, 51_877),
        ("charui_testmedic", Bundle, 39_004),
        ("vanity_testpilot", Bundle, 12_480),
        ("ability_testpilot_strike", Bundle, 8_196),
        ("unit_test_atlas", Bundle, 1_204_331),
        ("shared_testcontainer", Bundle, 3_051_999),
        ("audio_test_ui", AudioPackage, 245_760),
        ("audio_test_music", AudioPackage, 7_340_032),
    ];
    ae3_core::Manifest {
        asset_version: version,
        packages: packages
            .into_iter()
            .map(|(name, kind, size)| package(name, kind, size))
            .collect(),
        ..ae3_core::Manifest::default()
    }
}

impl FakeCdn {
    /// Publishes `manifest` as the latest Asset Version: `meta.json` names
    /// it and its `manifest.data` is served.
    pub fn publish_latest(&self, manifest: &ae3_core::Manifest) {
        self.put(META_URL, meta_json(manifest.asset_version).as_bytes());
        self.publish(manifest);
    }

    /// Serves `manifest` as the `manifest.data` of its Asset Version.
    pub fn publish(&self, manifest: &ae3_core::Manifest) {
        self.put(
            &cdn_url(manifest.asset_version, "manifest.data"),
            &encode_manifest(manifest),
        );
    }
}

// ---- Sound output ----------------------------------------------------------

/// What a [`Recorder`] was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Played {
    Play { generation: u64, frames: usize },
    Stop,
}

/// A sound output that records instead of opening a device.
#[derive(Clone, Default)]
pub struct Recorder(Arc<Mutex<Vec<Played>>>);

impl Recorder {
    /// What was asked since the last call.
    #[must_use]
    pub fn take(&self) -> Vec<Played> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl ae3_gui::output::Output for Recorder {
    fn play(&mut self, generation: u64, pcm: ae3_core::ae3_audio::Pcm) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Played::Play {
                generation,
                frames: pcm.frames(),
            });
    }

    fn stop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Played::Stop);
    }
}

/// A sound output whose device is missing, as on a machine without sound:
/// every sound ends at once with the error the real output reports.
pub struct NoDevice(pub ae3_gui::output::OnEnd);

impl ae3_gui::output::Output for NoDevice {
    fn play(&mut self, generation: u64, _pcm: ae3_core::ae3_audio::Pcm) {
        (self.0)(
            generation,
            Some("no audio output: no default device".to_owned()),
        );
    }

    fn stop(&mut self) {}
}
