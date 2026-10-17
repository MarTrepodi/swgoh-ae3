//! The `ae3` binary end to end: exit codes and stdout/stderr separation.
//! Each run gets an empty configuration file and its own cache directory, so
//! the user's settings never leak in. The `#[ignore]` tests talk to the real
//! CDN: `cargo test -p ae3 -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use ae3_core::config::ENV_VARS;

/// A unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ae3-bin-{name}-{}-{n}", std::process::id()));
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

/// Runs the binary with `args` and extra environment variables.
fn ae3(home: &TempDir, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ae3"));
    for var in ENV_VARS {
        command.env_remove(var);
    }
    command
        .args(args)
        .env("AE3_CONFIG", home.path().join("config.toml"))
        .env("AE3_CACHE_DIR", home.path().join("cache"))
        .envs(env.iter().copied());
    command.output().unwrap()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exited normally")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn help_and_version_exit_zero() {
    let home = TempDir::new("help");
    let help = ae3(&home, &["--help"], &[]);
    assert_eq!(code(&help), 0);
    assert!(stdout(&help).contains("extract"));
    let version = ae3(&home, &["--version"], &[]);
    assert_eq!(code(&version), 0);
    assert!(stdout(&version).starts_with("ae3 "));
}

#[test]
fn usage_errors_exit_two() {
    let home = TempDir::new("usage");
    for args in [
        &["extract"][..],
        &["extract", "--all", "--audio"],
        &["frobnicate"],
        &["list", "--platform", "linux"],
        &["diff", "1", "2", "--extract", "removed"],
    ] {
        let output = ae3(&home, args, &[]);
        assert_eq!(code(&output), 2, "{args:?}: {}", stderr(&output));
        assert!(stdout(&output).is_empty(), "{args:?}");
    }
}

#[test]
fn a_bad_setting_is_a_usage_error() {
    let home = TempDir::new("env");
    let output = ae3(&home, &["version", "--json"], &[("AE3_PLATFORM", "linux")]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("AE3_PLATFORM"),
        "{}",
        stderr(&output)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("AE3_PLATFORM")
    );

    fs::write(home.path().join("config.toml"), "platform = [").unwrap();
    let output = ae3(&home, &["version"], &[]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("config.toml"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn cache_stats_work_offline() {
    let home = TempDir::new("cache");
    let output = ae3(&home, &["cache", "--json"], &[]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["packages"], 0);
    assert_eq!(
        Path::new(json["root"].as_str().unwrap()),
        home.path().join("cache")
    );
    let text = ae3(&home, &["cache", "prune", "--max", "1GB"], &[]);
    assert_eq!(code(&text), 0, "{}", stderr(&text));
    assert!(
        stdout(&text).starts_with("Removed 0 files"),
        "{}",
        stdout(&text)
    );
}

#[test]
#[ignore = "needs the network"]
fn live_version() {
    let home = TempDir::new("live-version");
    let output = ae3(&home, &["version", "--json"], &[]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["asset_version"].as_u64().unwrap() >= 100_054, "{json}");
    assert!(
        json["manifest_url"]
            .as_str()
            .unwrap()
            .ends_with("/Windows/ETC/manifest.data")
    );
}

#[test]
#[ignore = "needs the network"]
fn live_search_by_name() {
    let home = TempDir::new("live-search");
    let output = ae3(
        &home,
        &["search", "charui_bossk", "--names-only", "--json"],
        &[],
    );
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hits = json["hits"].as_array().unwrap();
    assert!(
        hits.iter().any(|h| h["package"] == "charui_bossk"),
        "{json}"
    );
}

#[test]
#[ignore = "needs the network"]
fn live_extract_charui_bossk() {
    let home = TempDir::new("live-extract");
    let out = home.path().join("out");
    let output = ae3(
        &home,
        &[
            "extract",
            "charui_bossk",
            "--json",
            "--output",
            out.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["packages_done"], 1, "{json}");
    let png = out.join("charui/charui_bossk/tex.charui_bossk.png");
    assert!(fs::read(&png).unwrap().starts_with(b"\x89PNG"), "{json}");
}
