//! The Diff tab: rows of one part of a Diff and its summary.

use ae3_core::{AssetVersion, Diff, DiffFilter, PackageKind};

use super::table::format_bytes;

/// How a Package differs between the two Asset Versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Changed,
    Removed,
}

impl Change {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Changed => "changed",
            Self::Removed => "removed",
        }
    }
}

/// One Package in the Diff table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub name: String,
    pub kind: PackageKind,
    /// Size in the newer Asset Version (the older one for removals).
    pub size: u64,
    pub change: Change,
}

/// The rows `filter` covers: added, then changed, then removed, as each
/// list is ordered in the Diff.
#[must_use]
pub fn rows(diff: &Diff, filter: DiffFilter) -> Vec<DiffRow> {
    let row = |p: &ae3_core::Package, change| DiffRow {
        name: p.name.clone(),
        kind: p.kind,
        size: p.size,
        change,
    };
    let added = || diff.added.iter().map(|p| row(p, Change::Added));
    let changed = || diff.changed.iter().map(|c| row(&c.new, Change::Changed));
    let removed = || diff.removed.iter().map(|p| row(p, Change::Removed));
    match filter {
        DiffFilter::Added => added().collect(),
        DiffFilter::Changed => changed().collect(),
        DiffFilter::All => added().chain(changed()).collect(),
        DiffFilter::Removed => removed().collect(),
    }
}

/// Whether the rows of `filter` can be extracted: removed Packages exist
/// only in the older Asset Version.
#[must_use]
pub fn can_extract(filter: DiffFilter) -> bool {
    filter != DiffFilter::Removed
}

/// "100052 → 100054: 3 added (1.2 MB) · 40 changed (55.0 MB) · 1 removed
/// (3.0 KB) · 9000 unchanged".
#[must_use]
pub fn summary_line(diff: &Diff) -> String {
    let unchanged = diff.shared - diff.changed.len();
    format!(
        "{} → {}: {} added ({}) · {} changed ({}) · {} removed ({}) · {unchanged} unchanged",
        diff.old_version,
        diff.new_version,
        diff.added.len(),
        format_bytes(diff.added_size()),
        diff.changed.len(),
        format_bytes(diff.changed_size()),
        diff.removed.len(),
        format_bytes(diff.removed_size()),
    )
}

/// The Engine Rebuild warning, when the Diff is one.
#[must_use]
pub fn engine_rebuild_warning(diff: &Diff) -> Option<String> {
    diff.is_engine_rebuild().then(|| {
        format!(
            "Engine Rebuild: {:.0}% of the {} Bundles in both Asset Versions changed, \
             probably because the game moved to a new Unity Version. Changed Bundles \
             mostly hold the same Assets.",
            diff.changed_ratio() * 100.0,
            diff.shared_bundles
        )
    })
}

/// Parses an Asset Version field; empty means "not given".
pub fn parse_version(text: &str) -> Result<Option<AssetVersion>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| format!("{text:?} is not an Asset Version (a number such as 100054)"))
}

#[cfg(test)]
mod tests {
    use ae3_core::{Manifest, Package};

    use super::*;

    fn manifest(version: u64, packages: &[(&str, u64, u64)]) -> Manifest {
        Manifest {
            asset_version: AssetVersion(version),
            packages: packages
                .iter()
                .map(|&(name, hash, size)| Package {
                    name: name.into(),
                    content_hash: hash,
                    size,
                    ..Package::default()
                })
                .collect(),
            ..Manifest::default()
        }
    }

    fn diff() -> Diff {
        let old = manifest(1, &[("a", 1, 10), ("b", 1, 20), ("gone", 1, 3000)]);
        let new = manifest(2, &[("a", 1, 10), ("b", 2, 25), ("new", 1, 1200)]);
        Diff::compute(&old, &new)
    }

    #[test]
    fn rows_per_filter() {
        let d = diff();
        let names = |f| rows(&d, f).into_iter().map(|r| r.name).collect::<Vec<_>>();
        assert_eq!(names(DiffFilter::Added), ["new"]);
        assert_eq!(names(DiffFilter::Changed), ["b"]);
        assert_eq!(names(DiffFilter::All), ["new", "b"]);
        assert_eq!(names(DiffFilter::Removed), ["gone"]);
        let changed = &rows(&d, DiffFilter::Changed)[0];
        assert_eq!((changed.size, changed.change), (25, Change::Changed));
        assert!(!can_extract(DiffFilter::Removed));
        assert!(can_extract(DiffFilter::All));
    }

    #[test]
    fn summary_and_warning() {
        let d = diff();
        assert_eq!(
            summary_line(&d),
            "1 → 2: 1 added (1.2 KB) · 1 changed (25 B) · 1 removed (3.0 KB) · 1 unchanged"
        );
        // 1 of 2 shared Bundles changed: not an Engine Rebuild.
        assert_eq!(engine_rebuild_warning(&d), None);
        let old = manifest(1, &[("a", 1, 1), ("b", 1, 1)]);
        let new = manifest(2, &[("a", 2, 1), ("b", 2, 1)]);
        let rebuild = Diff::compute(&old, &new);
        assert!(engine_rebuild_warning(&rebuild).is_some_and(|w| w.contains("100%")));
    }

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version(" 100054 "), Ok(Some(AssetVersion(100_054))));
        assert_eq!(parse_version(""), Ok(None));
        assert!(parse_version("abc").is_err());
    }
}
