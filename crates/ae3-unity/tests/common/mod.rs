//! Locates the untracked game fixtures.

use std::path::PathBuf;

/// `$AE3_FIXTURES/100054/Windows`, defaulting to `<workspace>/fixtures`.
pub fn windows_dir() -> PathBuf {
    let root = std::env::var_os("AE3_FIXTURES").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
        PathBuf::from,
    );
    root.join("100054").join("Windows")
}

/// The bytes of a fixture, or `None` (and a note on stderr) when it is
/// absent, so tests skip instead of failing.
pub fn fixture(name: &str) -> Option<Vec<u8>> {
    let path = windows_dir().join(name);
    let bytes = std::fs::read(&path).ok();
    if bytes.is_none() {
        eprintln!("skipping: fixture {} not found", path.display());
    }
    bytes
}
