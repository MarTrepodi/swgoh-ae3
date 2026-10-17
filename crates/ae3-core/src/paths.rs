//! Per-user directories, resolved from the standard environment variables.

use std::env;
use std::path::PathBuf;

fn env_dir(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The file name of the configuration file inside [`default_config_dir`].
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The directory AE3 keeps its configuration in: `%APPDATA%\ae3` on Windows,
/// `~/Library/Application Support/ae3` on macOS, and `$XDG_CONFIG_HOME/ae3`
/// (default `~/.config/ae3`) elsewhere.
#[must_use]
pub fn default_config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env_dir("APPDATA")
    } else if cfg!(target_os = "macos") {
        env_dir("HOME").map(|home| home.join("Library/Application Support"))
    } else {
        env_dir("XDG_CONFIG_HOME").or_else(|| env_dir("HOME").map(|home| home.join(".config")))
    };
    base.map(|dir| dir.join("ae3"))
}

/// The configuration file shared by the CLI and the GUI.
#[must_use]
pub fn default_config_file() -> Option<PathBuf> {
    default_config_dir().map(|dir| dir.join(CONFIG_FILE_NAME))
}

/// The directory AE3 caches downloads and last-known values in:
/// `%LOCALAPPDATA%\ae3` on Windows, `~/Library/Caches/ae3` on macOS, and
/// `$XDG_CACHE_HOME/ae3` (default `~/.cache/ae3`) elsewhere.
#[must_use]
pub fn default_cache_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        env_dir("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        env_dir("HOME").map(|home| home.join("Library/Caches"))
    } else {
        env_dir("XDG_CACHE_HOME").or_else(|| env_dir("HOME").map(|home| home.join(".cache")))
    };
    base.map(|dir| dir.join("ae3"))
}
