//! Per-user directories, resolved from the standard environment variables.

use std::env;
use std::path::PathBuf;

/// The directory AE3 caches downloads and last-known values in:
/// `%LOCALAPPDATA%\ae3` on Windows, `~/Library/Caches/ae3` on macOS, and
/// `$XDG_CACHE_HOME/ae3` (default `~/.cache/ae3`) elsewhere.
#[must_use]
pub fn default_cache_dir() -> Option<PathBuf> {
    let var = |name| {
        env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))
    };
    base.map(|dir| dir.join("ae3"))
}
