//! `ae3 serve`: swgoh-ae2's HTTP API on a small synchronous server
//! (ADR-0001, ADR-0004).
//!
//! Routes (paths and parameter names are case-insensitive, as in ASP.NET):
//!
//! - `GET /Asset/list?version=&assetOS=`: the Bundle names of the Manifest.
//! - `GET /Asset/listDiff?version=&diffVersion=&diffType=&prefix=&assetOS=`:
//!   Bundle names added and changed (`diffType` 0), added (1) or changed (2)
//!   since `diffVersion`, optionally only of the Category `prefix`.
//! - `GET /Asset/single?assetName=&version=&forceReDownload=&assetOS=`: the
//!   PNG of `Texture2D` `tex.{assetName}` from Bundle `{assetName}`, or of its
//!   first `Texture2D`.
//!
//! `assetOS` is 0 Windows, 1 Android, 2 iOS (names work too); `version`
//! defaults to the Asset Version resolved at start-up. Fixes over swgoh-ae2:
//! the Diff uses the requested Platform and the resolved Environment rather
//! than hardcoded Android and PROD; errors are JSON with a proper status
//! (400, 404, 502) instead of a null body; responses are `image/png` and
//! `application/json`.
//!
//! Manifests are kept per (Asset Version, Platform) and rendered PNGs in a
//! bounded in-memory cache; concurrent requests for the same thing do the
//! work once.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::hash::Hash;
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ae3_core::ae3_image::{self, ImageFormat, PngCompression};
use ae3_core::cache::CacheError;
use ae3_core::download::DownloadError;
use ae3_core::http::{HttpError, Transport};
use ae3_core::{
    AssetError, AssetKind, AssetVersion, CdnLocation, Diff, DiffFilter, LoadedBundle, PackageKind,
    PackageStore, Platform,
};
use serde::Serialize;

use crate::app::{App, CliError, CmdResult, Status, Store, write_json};
use crate::cli::ServeArgs;

/// Manifests kept in memory, one per (Asset Version, Platform).
const MAX_STORES: usize = 8;
/// Bytes of rendered PNGs kept in memory.
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// How often idle workers look at the cancellation flag.
const POLL: Duration = Duration::from_millis(250);

/// A finished response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Arc<Vec<u8>>,
    /// For a PNG: the file name offered to clients that save it.
    pub file_name: Option<String>,
}

impl Reply {
    fn json(status: u16, value: &impl Serialize) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: Arc::new(serde_json::to_vec(value).unwrap_or_default()),
            file_name: None,
        }
    }

    fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(status, &serde_json::json!({ "error": message.into() }))
    }
}

type Handled = Result<Reply, Reply>;

/// Runs `work` for `key` at most once at a time; callers for the same key
/// wait, then usually find the result already cached.
struct SingleFlight<K> {
    locks: Mutex<HashMap<K, Arc<Mutex<()>>>>,
}

impl<K: Eq + Hash + Clone> SingleFlight<K> {
    fn new() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }

    fn run<R>(&self, key: &K, work: impl FnOnce() -> R) -> R {
        let flight = Arc::clone(lock(&self.locks).entry(key.clone()).or_default());
        let guard = flight.lock().unwrap_or_else(PoisonError::into_inner);
        let result = work();
        drop(guard);
        let mut locks = lock(&self.locks);
        // Only the map and this call hold it: nobody else is waiting.
        if Arc::strong_count(&flight) == 2 {
            locks.remove(key);
        }
        result
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

type StoreKey = (AssetVersion, Platform);
type ImageKey = (AssetVersion, Platform, String);
/// The open stores, and their keys oldest first.
type Stores<T> = Mutex<(HashMap<StoreKey, Store<T>>, VecDeque<StoreKey>)>;

#[derive(Default)]
struct ImageCache {
    entries: HashMap<ImageKey, Arc<Vec<u8>>>,
    order: VecDeque<ImageKey>,
    bytes: usize,
}

impl ImageCache {
    fn insert(&mut self, key: ImageKey, png: Arc<Vec<u8>>) {
        let len = png.len();
        if len > MAX_IMAGE_BYTES {
            return;
        }
        if let Some(old) = self.entries.insert(key.clone(), png) {
            self.bytes -= old.len();
            self.order.retain(|k| *k != key);
        }
        self.bytes += len;
        self.order.push_back(key);
        while self.bytes > MAX_IMAGE_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.entries.remove(&oldest) {
                self.bytes -= old.len();
            }
        }
    }

    fn remove(&mut self, key: &ImageKey) {
        if let Some(old) = self.entries.remove(key) {
            self.bytes -= old.len();
            self.order.retain(|k| k != key);
        }
    }
}

/// Everything the request handlers share.
pub struct ServeState<'a, T> {
    app: &'a App<T>,
    /// The location resolved at start-up: its Environment and base URL serve
    /// every Asset Version, and its version is the default.
    base: CdnLocation,
    stores: Stores<T>,
    store_flight: SingleFlight<StoreKey>,
    images: Mutex<ImageCache>,
    image_flight: SingleFlight<ImageKey>,
}

/// The query parameters, with lowercase names.
struct Query(HashMap<String, String>);

impl Query {
    fn parse(query: &str) -> Self {
        Self(
            query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| {
                    let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                    (
                        percent_decode(name).to_ascii_lowercase(),
                        percent_decode(value),
                    )
                })
                .collect(),
        )
    }

    /// A parameter; an empty value counts as absent.
    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
            .filter(|v| !v.trim().is_empty())
    }
}

/// Decodes `%XX` escapes and `+` as space; invalid escapes stay as they are.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = s
                    .get(i + 1..i + 3)
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(byte) = hex {
                    out.push(byte);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_platform(value: Option<&str>, default: Platform) -> Result<Platform, Reply> {
    match value.map(str::trim) {
        None => Ok(default),
        Some("0") => Ok(Platform::Windows),
        Some("1") => Ok(Platform::Android),
        Some("2") => Ok(Platform::Ios),
        Some(other) => other.parse().map_err(|_| {
            Reply::error(
                400,
                format!("assetOS {other:?}: expected 0 (Windows), 1 (Android) or 2 (iOS)"),
            )
        }),
    }
}

fn parse_version(name: &str, value: Option<&str>) -> Result<Option<AssetVersion>, Reply> {
    value
        .map(|v| {
            v.parse()
                .map_err(|_| Reply::error(400, format!("{name} {v:?} is not an Asset Version")))
        })
        .transpose()
}

fn parse_bool(name: &str, value: Option<&str>) -> Result<bool, Reply> {
    match value.map(str::to_ascii_lowercase).as_deref() {
        None | Some("false" | "0") => Ok(false),
        Some("true" | "1") => Ok(true),
        Some(other) => Err(Reply::error(
            400,
            format!("{name} {other:?}: expected true or false"),
        )),
    }
}

/// The HTTP status for a failure to open a Manifest or Package.
fn status_of(err: &AssetError) -> u16 {
    match err {
        // The CDN answers 403 for files it does not have.
        AssetError::PackageNotFound(_)
        | AssetError::WrongPackageKind { .. }
        | AssetError::Cache(CacheError::Download(DownloadError::Http(HttpError::Status {
            status: 403 | 404,
            ..
        }))) => 404,
        AssetError::Cache(CacheError::Download(_)) => 502,
        _ => 500,
    }
}

fn asset_error(err: &AssetError) -> Reply {
    Reply::error(status_of(err), err.to_string())
}

impl<'a, T: Transport + Send + Sync + 'static> ServeState<'a, T> {
    pub fn new(app: &'a App<T>, base: CdnLocation) -> Self {
        Self {
            app,
            base,
            stores: Mutex::new((HashMap::new(), VecDeque::new())),
            store_flight: SingleFlight::new(),
            images: Mutex::new(ImageCache::default()),
            image_flight: SingleFlight::new(),
        }
    }

    /// Answers one request.
    pub fn handle(&self, method: &str, url: &str) -> Reply {
        if !method.eq_ignore_ascii_case("GET") {
            return Reply::error(405, "only GET is supported");
        }
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        let query = Query::parse(query);
        let route = path.trim_end_matches('/').to_ascii_lowercase();
        let handled = match route.as_str() {
            "/asset/list" => self.list(&query),
            "/asset/listdiff" => self.list_diff(&query),
            "/asset/single" => self.single(&query),
            _ => Err(Reply::error(
                404,
                "unknown route; expected /Asset/list, /Asset/listDiff or /Asset/single",
            )),
        };
        handled.unwrap_or_else(|reply| reply)
    }

    fn store(&self, version: Option<AssetVersion>, platform: Platform) -> Result<Store<T>, Reply> {
        let version = version.unwrap_or(self.base.asset_version);
        let key = (version, platform);
        let cached = |stores: &Stores<T>| lock(stores).0.get(&key).cloned();
        if let Some(store) = cached(&self.stores) {
            return Ok(store);
        }
        self.store_flight.run(&key, || {
            if let Some(store) = cached(&self.stores) {
                return Ok(store);
            }
            let cache = self
                .app
                .cache()
                .map_err(|e| Reply::error(500, e.to_string()))?;
            let location = CdnLocation {
                asset_version: version,
                ..self.base.clone()
            };
            let store = PackageStore::open(Arc::clone(cache), location, platform).map_err(|e| {
                let status = status_of(&e);
                let message = if status == 404 {
                    format!("no Manifest for Asset Version {version} ({platform})")
                } else {
                    e.to_string()
                };
                Reply::error(status, message)
            })?;
            let mut stores = lock(&self.stores);
            let (map, order) = &mut *stores;
            map.insert(key, store.clone());
            order.push_back(key);
            while order.len() > MAX_STORES {
                if let Some(oldest) = order.pop_front() {
                    map.remove(&oldest);
                }
            }
            Ok(store)
        })
    }

    fn platform(&self, query: &Query) -> Result<Platform, Reply> {
        parse_platform(query.get("assetOS"), self.app.platform())
    }

    fn list(&self, query: &Query) -> Handled {
        let platform = self.platform(query)?;
        let version = parse_version("version", query.get("version"))?;
        let store = self.store(version, platform)?;
        let names: Vec<&str> = store
            .manifest()
            .packages
            .iter()
            .filter(|p| p.kind == PackageKind::Bundle)
            .map(|p| p.name.as_str())
            .collect();
        Ok(Reply::json(200, &names))
    }

    fn list_diff(&self, query: &Query) -> Handled {
        let platform = self.platform(query)?;
        let version = parse_version("version", query.get("version"))?;
        let old = parse_version("diffVersion", query.get("diffVersion"))?
            .ok_or_else(|| Reply::error(400, "diffVersion is required"))?;
        let filter = match query.get("diffType").map(str::trim) {
            None | Some("0" | "All" | "all") => DiffFilter::All,
            Some("1" | "New" | "new") => DiffFilter::Added,
            Some("2" | "Changed" | "changed") => DiffFilter::Changed,
            Some(other) => {
                return Err(Reply::error(
                    400,
                    format!("diffType {other:?}: expected 0 (All), 1 (New) or 2 (Changed)"),
                ));
            }
        };
        let prefix = query.get("prefix");
        let new_store = self.store(version, platform)?;
        let old_store = self.store(Some(old), platform)?;
        let diff = Diff::compute(old_store.manifest(), new_store.manifest());
        let names: Vec<&str> = diff
            .packages(filter)
            .filter(|p| p.kind == PackageKind::Bundle)
            .filter(|p| prefix.is_none_or(|c| p.category().eq_ignore_ascii_case(c)))
            .map(|p| p.name.as_str())
            .collect();
        Ok(Reply::json(200, &names))
    }

    fn single(&self, query: &Query) -> Handled {
        let platform = self.platform(query)?;
        let version = parse_version("version", query.get("version"))?;
        let force = parse_bool("forceReDownload", query.get("forceReDownload"))?;
        let name = query
            .get("assetName")
            .ok_or_else(|| Reply::error(400, "assetName is required"))?;
        let store = self.store(version, platform)?;
        let package = store
            .package(name)
            .filter(|p| p.kind == PackageKind::Bundle)
            .ok_or_else(|| {
                Reply::error(
                    404,
                    format!(
                        "no Bundle named {name:?} in Asset Version {} ({platform})",
                        store.manifest().asset_version
                    ),
                )
            })?;
        let name = package.name.clone();
        let key = (store.manifest().asset_version, platform, name.clone());
        let png = self.image_flight.run(&key, || {
            if force {
                lock(&self.images).remove(&key);
                // Drop the cached download so it is fetched again.
                if let Ok(path) = store.cache().path_for(platform, package) {
                    let _ = fs::remove_file(path);
                }
            } else if let Some(png) = lock(&self.images).entries.get(&key) {
                return Ok(Arc::clone(png));
            }
            let png = Arc::new(render(&store, package)?);
            lock(&self.images).insert(key.clone(), Arc::clone(&png));
            Ok::<_, Reply>(png)
        })?;
        Ok(Reply {
            status: 200,
            content_type: "image/png",
            body: png,
            file_name: Some(format!("tex.{name}.png")),
        })
    }
}

/// The PNG of `tex.{name}` in the Bundle, or of its first `Texture2D`. Only
/// the Bundle itself is fetched: textures never live in dependencies.
fn render<T: Transport + Send + Sync + 'static>(
    store: &Store<T>,
    package: &ae3_core::Package,
) -> Result<Vec<u8>, Reply> {
    let path = store
        .cache()
        .get_or_fetch(store.location(), store.platform(), package)
        .map_err(|e| asset_error(&AssetError::Cache(e)))?;
    let bytes = fs::read(&path).map_err(|e| Reply::error(500, e.to_string()))?;
    let bundle =
        LoadedBundle::from_bytes(package.name.clone(), &bytes).map_err(|e| asset_error(&e))?;
    let view = bundle.view().map_err(|e| asset_error(&e))?;
    let textures: Vec<_> = view
        .assets()
        .into_iter()
        .filter(|a| a.kind == AssetKind::Texture && a.class_name == "Texture2D")
        .collect();
    let wanted = format!("tex.{}", package.name);
    let texture = textures
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(&wanted))
        .or_else(|| textures.first())
        .ok_or_else(|| Reply::error(404, format!("{} holds no Texture2D", package.name)))?;
    let image = view
        .decode_texture(texture.path_id)
        .map_err(|e| asset_error(&e))?;
    ae3_image::encode(&image, ImageFormat::Png(PngCompression::default()))
        .map_err(|e| Reply::error(500, e.to_string()))
}

fn respond(request: tiny_http::Request, reply: &Reply) -> std::io::Result<()> {
    let header = |name: &str, value: &str| {
        tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
    };
    let mut response =
        tiny_http::Response::from_data(reply.body.as_slice()).with_status_code(reply.status);
    response.add_header(header("Content-Type", reply.content_type).expect("valid header"));
    if let Some(name) = &reply.file_name
        && let Some(h) = header(
            "Content-Disposition",
            &format!("inline; filename=\"{name}\""),
        )
    {
        response.add_header(h);
    }
    request.respond(response)
}

#[derive(Serialize)]
struct Listening<'a> {
    url: &'a str,
    asset_version: u64,
    platform: &'static str,
}

pub fn run<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    args: &ServeArgs,
    out: &mut dyn Write,
) -> CmdResult {
    let base = app.resolve_global()?;
    let server = tiny_http::Server::http(&args.bind)
        .map_err(|e| CliError::failed(format!("cannot listen on {}: {e}", args.bind)))?;
    let address = server
        .server_addr()
        .to_ip()
        .map_or_else(|| args.bind.clone(), |a| a.to_string());
    let url = format!("http://{address}");
    if app.json() {
        write_json(
            out,
            &Listening {
                url: &url,
                asset_version: base.asset_version.0,
                platform: app.platform().name(),
            },
        )?;
    } else {
        writeln!(
            out,
            "Listening on {url} (default Asset Version {}, {})",
            base.asset_version,
            app.platform()
        )?;
    }
    out.flush()?;
    serve(app, &server, base, usize::from(args.threads));
    app.ui.info("Stopped");
    Ok(Status::Success)
}

/// Answers requests on `threads` workers until the cancellation flag is set.
pub fn serve<T: Transport + Send + Sync + 'static>(
    app: &App<T>,
    server: &tiny_http::Server,
    base: CdnLocation,
    threads: usize,
) {
    let state = ServeState::new(app, base);
    thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                while !app.cancel.load(Ordering::Relaxed) {
                    let request = match server.recv_timeout(POLL) {
                        Ok(Some(request)) => request,
                        Ok(None) => continue,
                        Err(err) => {
                            app.ui.warn(&format!("receiving a request failed: {err}"));
                            continue;
                        }
                    };
                    let started = Instant::now();
                    let method = request.method().to_string();
                    let url = request.url().to_owned();
                    let reply = state.handle(&method, &url);
                    app.ui.verbose(|| {
                        format!(
                            "{method} {url} -> {} ({} ms)",
                            reply.status,
                            started.elapsed().as_millis()
                        )
                    });
                    if let Err(err) = respond(request, &reply) {
                        app.ui
                            .debug(|| format!("writing the response failed: {err}"));
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::TcpStream;

    use super::*;
    use crate::test_support::{FIXTURE_VERSION, Harness, require_fixtures};

    #[test]
    fn query_parsing() {
        let q = Query::parse("assetName=charui%5Fbossk&Version=1&x&prefix=a+b&bad=%zz");
        assert_eq!(q.get("assetname"), Some("charui_bossk"));
        assert_eq!(q.get("VERSION"), Some("1"));
        assert_eq!(q.get("x"), None);
        assert_eq!(q.get("prefix"), Some("a b"));
        assert_eq!(q.get("bad"), Some("%zz"));
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn platforms_by_number_or_name() {
        assert_eq!(
            parse_platform(None, Platform::Windows),
            Ok(Platform::Windows)
        );
        assert_eq!(
            parse_platform(Some("1"), Platform::Windows),
            Ok(Platform::Android)
        );
        assert_eq!(
            parse_platform(Some("2"), Platform::Windows),
            Ok(Platform::Ios)
        );
        assert_eq!(
            parse_platform(Some("ios"), Platform::Windows),
            Ok(Platform::Ios)
        );
        assert_eq!(
            parse_platform(Some("7"), Platform::Windows)
                .unwrap_err()
                .status,
            400
        );
    }

    #[test]
    fn image_cache_evicts_oldest_beyond_its_budget() {
        let mut cache = ImageCache::default();
        let key = |n: &str| (AssetVersion(1), Platform::Windows, n.to_owned());
        let big = Arc::new(vec![0; MAX_IMAGE_BYTES / 2 + 1]);
        cache.insert(key("a"), Arc::clone(&big));
        cache.insert(key("b"), Arc::clone(&big));
        assert!(!cache.entries.contains_key(&key("a")));
        assert!(cache.entries.contains_key(&key("b")));
        assert_eq!(cache.bytes, big.len());
        cache.remove(&key("b"));
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn single_flight_runs_one_at_a_time_per_key() {
        let flight = SingleFlight::new();
        let running = std::sync::atomic::AtomicUsize::new(0);
        thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    flight.run(&1, || {
                        assert_eq!(running.fetch_add(1, Ordering::SeqCst), 0);
                        thread::sleep(Duration::from_millis(5));
                        running.fetch_sub(1, Ordering::SeqCst);
                    });
                });
            }
        });
        assert_eq!(lock(&flight.locks).len(), 0);
    }

    fn state_for(h: &Harness, f: impl FnOnce(&ServeState<'_, crate::test_support::FixtureCdn>)) {
        h.with_app("ae3 serve", |app| {
            let base = app.resolve_global().unwrap();
            f(&ServeState::new(app, base));
        });
    }

    #[test]
    fn bad_requests() {
        let h = Harness::new(None);
        state_for(&h, |state| {
            assert_eq!(state.handle("POST", "/Asset/list").status, 405);
            assert_eq!(state.handle("GET", "/nope").status, 404);
            assert_eq!(state.handle("GET", "/Asset/single").status, 400);
            assert_eq!(state.handle("GET", "/Asset/list?version=abc").status, 400);
            assert_eq!(state.handle("GET", "/Asset/list?assetOS=9").status, 400);
            assert_eq!(state.handle("GET", "/Asset/listDiff?version=1").status, 400);
            let reply = state.handle("GET", "/Asset/list?version=1");
            assert_eq!(
                reply.status,
                404,
                "{:?}",
                String::from_utf8_lossy(&reply.body)
            );
            let body: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
            assert!(body["error"].as_str().unwrap().contains("Asset Version 1"));
        });
    }

    #[test]
    fn list_list_diff_and_single() {
        let dir = require_fixtures!();
        let h = Harness::new(Some(dir));
        let old = h.install_mini_manifests();
        state_for(&h, |state| {
            let reply = state.handle("GET", "/asset/LIST?assetOS=0");
            assert_eq!(reply.status, 200);
            assert_eq!(reply.content_type, "application/json; charset=utf-8");
            let names: Vec<String> = serde_json::from_slice(&reply.body).unwrap();
            assert!(names.contains(&"charui_bossk".to_owned()));
            assert!(
                !names.iter().any(|n| n.starts_with("audio_")),
                "Bundles only"
            );

            let url = format!("/Asset/listDiff?version={FIXTURE_VERSION}&diffVersion={old}");
            let all: Vec<String> = serde_json::from_slice(&state.handle("GET", &url).body).unwrap();
            assert_eq!(all, ["charui_bossk", "vanity_bossk"]);
            let new: Vec<String> =
                serde_json::from_slice(&state.handle("GET", &format!("{url}&diffType=1")).body)
                    .unwrap();
            assert_eq!(new, ["charui_bossk"]);
            let changed: Vec<String> = serde_json::from_slice(
                &state
                    .handle("GET", &format!("{url}&diffType=2&prefix=charui"))
                    .body,
            )
            .unwrap();
            assert_eq!(changed.len(), 0);

            let reply = state.handle("GET", "/Asset/single?assetName=charui_bossk");
            assert_eq!(
                reply.status,
                200,
                "{:?}",
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(reply.content_type, "image/png");
            assert_eq!(reply.file_name.as_deref(), Some("tex.charui_bossk.png"));
            assert!(reply.body.starts_with(b"\x89PNG"));
            let fetches = || {
                h.cdn
                    .requests()
                    .iter()
                    .filter(|u| u.ends_with("/charui_bossk.bundle"))
                    .count()
            };
            assert_eq!(fetches(), 1);
            let again = state.handle("GET", "/Asset/single?assetName=charui_bossk&version=100054");
            assert_eq!(again.body, reply.body);
            assert_eq!(fetches(), 1, "served from memory");
            let forced = state.handle(
                "GET",
                "/Asset/single?assetName=charui_bossk&forceReDownload=true",
            );
            assert_eq!(forced.body, reply.body);
            assert_eq!(fetches(), 2, "forceReDownload fetches again");

            assert_eq!(
                state.handle("GET", "/Asset/single?assetName=nope_x").status,
                404
            );
            assert_eq!(
                state
                    .handle("GET", "/Asset/single?assetName=audio_dataprimer")
                    .status,
                404,
                "an Audio Package is not a Bundle"
            );
        });
    }

    #[test]
    fn serves_over_http_until_cancelled() {
        let h = Harness::new(None);
        h.with_app("ae3 serve", |app| {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let addr = server.server_addr().to_ip().unwrap();
            let base = app.resolve_global().unwrap();
            thread::scope(|s| {
                s.spawn(|| serve(app, &server, base, 2));
                let mut stream = TcpStream::connect(addr).unwrap();
                stream
                    .write_all(
                        b"GET /Asset/nothing HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
                let mut response = String::new();
                stream.read_to_string(&mut response).unwrap();
                assert!(response.starts_with("HTTP/1.1 404"), "{response}");
                assert!(response.contains("application/json"), "{response}");
                app.cancel.store(true, Ordering::Relaxed);
            });
        });
    }
}
