//! Where the fixture Packages live.
//!
//! `scripts/fetch-fixtures.sh` downloads a pinned list of Packages into
//! `<root>/<Asset Version>/<Platform>/<name>`, where the root is
//! `$AE3_FIXTURES` or, when that is unset, `<workspace>/fixtures`. Tests skip
//! whatever is absent, so a partial fixture set still runs what it can.

use std::path::{Path, PathBuf};

use ae3_core::Platform;

/// The Asset Version every fixture comes from.
pub const ASSET_VERSION: u32 = 100_054;

/// `$AE3_FIXTURES`, or `<workspace>/fixtures` when it is unset.
#[must_use]
pub fn root() -> PathBuf {
    std::env::var_os("AE3_FIXTURES").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
        PathBuf::from,
    )
}

/// The directory holding one Platform's fixtures, whether or not it exists.
#[must_use]
pub fn platform_dir(platform: Platform) -> PathBuf {
    root().join(ASSET_VERSION.to_string()).join(platform.name())
}

/// Whether `AE3_REQUIRE_FIXTURES` is set (to anything but `0`), as CI does
/// after fetching them: a missing fixture is then a failure, not a skip.
#[must_use]
pub fn required() -> bool {
    std::env::var_os("AE3_REQUIRE_FIXTURES").is_some_and(|v| v != "0")
}

/// The path of one fixture Bundle (`<name>.bundle`), or `None` after a note
/// on stderr when it is absent.
///
/// # Panics
///
/// When the Bundle is absent and [`required`] is true.
#[must_use]
pub fn bundle(platform: Platform, name: &str) -> Option<PathBuf> {
    let path = platform_dir(platform).join(format!("{name}.bundle"));
    if path.is_file() {
        return Some(path);
    }
    assert!(
        !required(),
        "fixture {} is missing; run scripts/fetch-fixtures.sh",
        path.display()
    );
    eprintln!("skipping: fixture {} not found", path.display());
    None
}

/// Every `.bundle` file present for `platform`, sorted by name.
#[must_use]
pub fn bundles(platform: Platform) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(platform_dir(platform)) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "bundle"))
        .collect();
    paths.sort();
    paths
}
