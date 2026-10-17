//! Offline doubles for the command tests: a CDN served from the fixtures
//! directory, temporary directories, and an [`App`] wired to both.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ae3_core::http::{HttpError, RangeResponse, Transport};
use ae3_core::version::META_URL;
use ae3_core::{Config, Manifest, Package};

use crate::app::{App, CmdResult};
use crate::cli::Cli;
use crate::commands;
use crate::ui::{Ui, Verbosity};

pub const BASE_URL: &str = "https://cdn.test/PROD/";
pub const FIXTURE_VERSION: u64 = 100_054;

/// `<AE3_FIXTURES or workspace/fixtures>/100054/Windows`, if present.
pub fn fixtures_dir() -> Option<PathBuf> {
    let root = std::env::var_os("AE3_FIXTURES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
        PathBuf::from,
    );
    let dir = root.join(FIXTURE_VERSION.to_string()).join("Windows");
    dir.join("manifest.data").is_file().then_some(dir)
}

/// Skips the calling test (with a note) when the fixtures are absent.
macro_rules! require_fixtures {
    () => {
        match $crate::test_support::fixtures_dir() {
            Some(dir) => dir,
            None => {
                eprintln!("skipping: fixtures not found (set AE3_FIXTURES)");
                return;
            }
        }
    };
}
pub(crate) use require_fixtures;

/// A CDN: `meta.json` names Asset Version 100054 under [`BASE_URL`];
/// `<BASE_URL><version>/Windows/ETC/<file>` is served from `extra` first and
/// otherwise, for 100054, from the fixtures directory.
pub struct FixtureCdn {
    dir: Option<PathBuf>,
    extra: Mutex<HashMap<String, Vec<u8>>>,
    requests: Mutex<Vec<String>>,
}

impl FixtureCdn {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            extra: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Serves `body` as `file` of Asset Version `version`.
    pub fn put(&self, version: u64, file: &str, body: Vec<u8>) {
        self.extra
            .lock()
            .unwrap()
            .insert(format!("{BASE_URL}{version}/Windows/ETC/{file}"), body);
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn read(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.requests.lock().unwrap().push(url.to_owned());
        let not_found = || HttpError::Status {
            url: url.to_owned(),
            status: 404,
        };
        if url == META_URL {
            return Ok(format!(
                r#"{{"data":{{"assetVersion":{FIXTURE_VERSION},"assetPath":"{BASE_URL}"}}}}"#
            )
            .into_bytes());
        }
        if let Some(body) = self.extra.lock().unwrap().get(url) {
            return Ok(body.clone());
        }
        let prefix = format!("{BASE_URL}{FIXTURE_VERSION}/Windows/ETC/");
        let name = url.strip_prefix(&prefix).ok_or_else(not_found)?;
        let dir = self.dir.as_ref().ok_or_else(not_found)?;
        fs::read(dir.join(name)).map_err(|_| not_found())
    }
}

impl Transport for FixtureCdn {
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
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ae3-cli-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A command run against the fixture CDN, with its own cache and output
/// directories.
pub struct Harness {
    pub cdn: Arc<FixtureCdn>,
    pub cache: TempDir,
    pub out: TempDir,
}

/// What one command run produced.
pub struct Ran {
    pub result: CmdResult,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn json(&self) -> serde_json::Value {
        assert!(self.result.is_ok(), "{:?}\n{}", self.result, self.stderr);
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("{e}: {}", self.stdout))
    }
}

impl Harness {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            cdn: Arc::new(FixtureCdn::new(dir)),
            cache: TempDir::new("cache"),
            out: TempDir::new("out"),
        }
    }

    /// Runs `f` with an [`App`] for `line`. The configuration is the
    /// defaults plus the command line; the environment and the user's
    /// configuration file are not read.
    pub fn with_app<R>(&self, line: &str, f: impl FnOnce(&App<FixtureCdn>) -> R) -> R {
        let cli = parse(line);
        let mut config = Config {
            cache_dir: Some(self.cache.path().to_owned()),
            output_dir: self.out.path().to_owned(),
            parallel_downloads: 2,
            ..Config::default()
        };
        config.apply(&cli.settings());
        let app = App::new(
            cli.global.clone(),
            config,
            Arc::clone(&self.cdn),
            Arc::new(AtomicBool::new(false)),
            Ui::captured(Verbosity::Verbose),
        );
        f(&app)
    }

    /// Runs `line` (such as `ae3 list --json`) in-process.
    pub fn run(&self, line: &str) -> Ran {
        let cli = parse(line);
        self.with_app(line, |app| {
            let mut stdout = Vec::new();
            let result = commands::dispatch(app, &cli.command, &mut stdout);
            Ran {
                result,
                stdout: String::from_utf8(stdout).unwrap(),
                stderr: app.ui.output(),
            }
        })
    }

    /// Replaces the 100054 Manifest by one listing only the Packages present
    /// in the fixtures (so indexing never meets a missing file), and serves
    /// an older Asset Version 100053 that lacks `charui_bossk`, has another
    /// Content Hash for `vanity_bossk` and also lists `charui_gone`.
    /// Returns the older Asset Version.
    pub fn install_mini_manifests(&self) -> u64 {
        let dir = self.cdn.dir.clone().expect("fixtures");
        let real = Manifest::decode(&fs::read(dir.join("manifest.data")).unwrap()).unwrap();
        let names: Vec<String> = real
            .packages
            .iter()
            .filter(|p| p.file_name().is_some_and(|f| dir.join(f).is_file()))
            .map(|p| p.name.clone())
            .collect();
        let mini: Vec<Package> = real
            .packages
            .into_iter()
            .filter(|p| names.contains(&p.name))
            .map(|mut p| {
                p.dependencies.retain(|d| names.contains(d));
                p
            })
            .collect();
        assert!(mini.iter().any(|p| p.name == "charui_bossk"));
        self.cdn.put(
            FIXTURE_VERSION,
            "manifest.data",
            encode_manifest(FIXTURE_VERSION, &mini),
        );

        let old_version = FIXTURE_VERSION - 1;
        let mut old: Vec<Package> = mini
            .into_iter()
            .filter(|p| p.name != "charui_bossk")
            .map(|mut p| {
                if p.name == "vanity_bossk" {
                    p.content_hash ^= 1;
                }
                p
            })
            .collect();
        old.push(Package {
            name: "charui_gone".into(),
            size: 5,
            ..Package::default()
        });
        self.cdn.put(
            old_version,
            "manifest.data",
            encode_manifest(old_version, &old),
        );
        old_version
    }
}

fn parse(line: &str) -> Cli {
    use clap::Parser;
    Cli::try_parse_from(line.split_whitespace()).unwrap_or_else(|e| panic!("{line}: {e}"))
}

/// Encodes a minimal `manifest.data` (protobuf) with these Packages:
/// name, Content Hash, size, kind and dependencies.
pub fn encode_manifest(version: u64, packages: &[Package]) -> Vec<u8> {
    fn varint(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(u8::try_from(v & 0x7f).unwrap() | 0x80);
            v >>= 7;
        }
        out.push(u8::try_from(v).unwrap());
    }
    fn bytes(out: &mut Vec<u8>, field: u64, data: &[u8]) {
        varint(out, field << 3 | 2);
        varint(out, data.len() as u64);
        out.extend_from_slice(data);
    }
    fn number(out: &mut Vec<u8>, field: u64, v: u64) {
        varint(out, field << 3);
        varint(out, v);
    }
    let mut out = Vec::new();
    number(&mut out, 1, version);
    for p in packages {
        let mut record = Vec::new();
        bytes(&mut record, 1, p.name.as_bytes());
        number(&mut record, 2, p.content_hash);
        number(&mut record, 3, p.size);
        number(&mut record, 7, p.kind.code());
        for d in &p.dependencies {
            bytes(&mut record, 9, d.as_bytes());
        }
        bytes(&mut out, 2, &record);
    }
    bytes(&mut out, 3, b"Windows");
    out
}
