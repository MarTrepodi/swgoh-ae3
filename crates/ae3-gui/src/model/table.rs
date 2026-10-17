//! The Bundles table: one row per Package, filtered by the Asset Index
//! search and the type filter, and sorted.

use std::fmt::Write as _;

use std::cmp::Ordering;
use std::collections::HashMap;

use ae3_core::{AssetIndex, AssetKind, AssetKinds, Manifest, PackageKind};

/// The Asset kinds the type filter and the Types column show. `Other`
/// (scene plumbing) is never shown.
pub const SHOWN_KINDS: [AssetKind; 7] = [
    AssetKind::Texture,
    AssetKind::Atlas,
    AssetKind::Sprite,
    AssetKind::Text,
    AssetKind::Structured,
    AssetKind::Mesh,
    AssetKind::Audio,
];

/// One Package in the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRow {
    /// The Package's index in the Manifest; the row's stable id.
    pub id: usize,
    pub name: String,
    pub kind: PackageKind,
    pub size: u64,
    /// What the Package holds, once the Asset Index (or opening it) has
    /// told; `None` while unknown.
    pub kinds: Option<AssetKinds>,
}

impl PackageRow {
    /// Rows for every Package of `manifest`. Audio Packages are known to
    /// hold audio without being indexed.
    #[must_use]
    pub fn from_manifest(manifest: &Manifest) -> Vec<Self> {
        manifest
            .packages
            .iter()
            .enumerate()
            .map(|(id, p)| Self {
                id,
                name: p.name.clone(),
                kind: p.kind,
                size: p.size,
                kinds: (p.kind == PackageKind::AudioPackage)
                    .then(|| [AssetKind::Audio].into_iter().collect()),
            })
            .collect()
    }
}

/// Sets each Bundle row's kinds from the Asset Index; rows the index has
/// not reached keep what they had (for example from opening the Bundle).
pub fn apply_index_kinds(rows: &mut [PackageRow], index: &AssetIndex) {
    for row in rows.iter_mut().filter(|r| r.kind == PackageKind::Bundle) {
        if let Some(package) = index.package(&row.name).filter(|p| p.is_indexed()) {
            row.kinds = Some(shown(package.kinds()));
        }
    }
}

/// `kinds` without `Other`.
#[must_use]
pub fn shown(kinds: AssetKinds) -> AssetKinds {
    kinds.iter().filter(|k| *k != AssetKind::Other).collect()
}

/// Which Asset kinds a row must contain (at least one of) to be shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TypeFilter {
    /// Empty means no restriction.
    pub kinds: AssetKinds,
}

impl TypeFilter {
    #[must_use]
    pub fn is_active(self) -> bool {
        !self.kinds.is_empty()
    }

    pub fn toggle(&mut self, kind: AssetKind) {
        let mut one = AssetKinds::EMPTY;
        one.insert(kind);
        self.kinds = AssetKinds::from_bits(self.kinds.bits() ^ one.bits());
    }

    /// Whether `row` passes. With an active filter, rows whose contents are
    /// not known yet are hidden.
    #[must_use]
    pub fn accepts(self, row: &PackageRow) -> bool {
        if !self.is_active() {
            return true;
        }
        row.kinds
            .is_some_and(|kinds| kinds.iter().any(|k| self.kinds.contains(k)))
    }
}

/// The outcome of an Asset Index search, owned so it outlives the index
/// snapshot it came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchMatches {
    /// The query as typed.
    pub query: String,
    /// Package name -> the path ids of its Assets whose names matched
    /// (empty when only the Package name matched).
    pub packages: HashMap<String, Vec<i64>>,
    /// Matching Assets, summed over Packages.
    pub asset_matches: usize,
    /// Bundles the index has not covered yet; they can match by name only.
    pub pending_bundles: usize,
}

impl SearchMatches {
    /// Runs `query` against `index`.
    #[must_use]
    pub fn search(index: &AssetIndex, query: &str) -> Self {
        let results = index.search(query);
        let asset_matches = results.asset_match_count();
        let packages = results
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.package_name().to_owned(),
                    hit.assets.iter().map(|a| a.path_id).collect(),
                )
            })
            .collect();
        Self {
            query: query.to_owned(),
            packages,
            asset_matches,
            pending_bundles: results.coverage.pending_bundles(),
        }
    }

    /// Matches from Package names alone, for when no index is loaded yet.
    #[must_use]
    pub fn by_name(rows: &[PackageRow], query: &str) -> Self {
        let needle = query.trim().to_lowercase();
        let packages = if needle.is_empty() {
            HashMap::new()
        } else {
            rows.iter()
                .filter(|r| r.name.to_lowercase().contains(&needle))
                .map(|r| (r.name.clone(), Vec::new()))
                .collect()
        };
        Self {
            query: query.to_owned(),
            packages,
            asset_matches: 0,
            pending_bundles: rows
                .iter()
                .filter(|r| r.kind == PackageKind::Bundle)
                .count(),
        }
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    /// The matched Asset path ids of a Package.
    #[must_use]
    pub fn assets_of(&self, name: &str) -> &[i64] {
        self.packages.get(name).map_or(&[], Vec::as_slice)
    }

    /// "N Bundles contain 'x'".
    #[must_use]
    pub fn status_line(&self) -> String {
        let n = self.packages.len();
        let noun = if n == 1 {
            "Bundle contains"
        } else {
            "Bundles contain"
        };
        let mut line = format!("{n} {noun} '{}'", self.query.trim());
        if self.asset_matches > 0 {
            let s = if self.asset_matches == 1 { "" } else { "s" };
            let _ = write!(line, " ({} matching Asset{s})", self.asset_matches);
        }
        if self.pending_bundles > 0 {
            let _ = write!(
                line,
                "; {} Bundles not indexed yet match by name only",
                self.pending_bundles
            );
        }
        line
    }
}

/// What the table is sorted by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Types,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

impl Sort {
    /// Clicking a column header: the same column flips the direction, a
    /// new column sorts ascending.
    pub fn click(&mut self, key: SortKey) {
        if self.key == key {
            self.descending = !self.descending;
        } else {
            self.key = key;
            self.descending = false;
        }
    }

    fn compare(self, a: &PackageRow, b: &PackageRow) -> Ordering {
        let order = match self.key {
            SortKey::Name => a.name.cmp(&b.name),
            SortKey::Size => a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name)),
            SortKey::Types => a
                .kinds
                .map(AssetKinds::bits)
                .cmp(&b.kinds.map(AssetKinds::bits))
                .then_with(|| a.name.cmp(&b.name)),
        };
        if self.descending {
            order.reverse()
        } else {
            order
        }
    }
}

/// The ids of the rows to show, in display order.
#[must_use]
pub fn visible_rows(
    rows: &[PackageRow],
    filter: TypeFilter,
    search: Option<&SearchMatches>,
    sort: Sort,
) -> Vec<usize> {
    let mut shown: Vec<&PackageRow> = rows
        .iter()
        .filter(|r| filter.accepts(r))
        .filter(|r| search.is_none_or(|s| s.contains(&r.name)))
        .collect();
    shown.sort_by(|a, b| sort.compare(a, b));
    shown.into_iter().map(|r| r.id).collect()
}

/// Formats a byte count as `512 B`, `12.3 KB`, `4.5 MB` or `1.20 GB`.
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit >= 1 {
        format!("{value:.2} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use ae3_core::Package;

    use super::*;

    fn row(id: usize, name: &str, size: u64, kinds: Option<&[AssetKind]>) -> PackageRow {
        PackageRow {
            id,
            name: name.into(),
            kind: PackageKind::Bundle,
            size,
            kinds: kinds.map(|k| k.iter().copied().collect()),
        }
    }

    fn rows() -> Vec<PackageRow> {
        vec![
            row(0, "charui_bossk", 300, Some(&[AssetKind::Texture])),
            row(
                1,
                "unit_details_atlas",
                100,
                Some(&[AssetKind::Atlas, AssetKind::Texture]),
            ),
            row(
                2,
                "char_bossk_pre",
                900,
                Some(&[AssetKind::Mesh, AssetKind::Texture]),
            ),
            row(3, "pending_bundle", 50, None),
        ]
    }

    #[test]
    fn no_filter_shows_everything_sorted_by_name() {
        let visible = visible_rows(&rows(), TypeFilter::default(), None, Sort::default());
        assert_eq!(visible, [2, 0, 3, 1]);
    }

    #[test]
    fn type_filter_hides_unknown_rows_and_non_matching_kinds() {
        let mut filter = TypeFilter::default();
        filter.toggle(AssetKind::Atlas);
        filter.toggle(AssetKind::Mesh);
        assert!(filter.is_active());
        let visible = visible_rows(&rows(), filter, None, Sort::default());
        assert_eq!(visible, [2, 1]);
        filter.toggle(AssetKind::Atlas);
        filter.toggle(AssetKind::Mesh);
        assert!(!filter.is_active());
    }

    #[test]
    fn search_and_sort_combine() {
        let rows = rows();
        let search = SearchMatches::by_name(&rows, "BOSSK");
        let mut sort = Sort::default();
        sort.click(SortKey::Size);
        assert_eq!(
            visible_rows(&rows, TypeFilter::default(), Some(&search), sort),
            [0, 2]
        );
        sort.click(SortKey::Size);
        assert!(sort.descending);
        assert_eq!(
            visible_rows(&rows, TypeFilter::default(), Some(&search), sort),
            [2, 0]
        );
        sort.click(SortKey::Name);
        assert!(!sort.descending);
    }

    #[test]
    fn name_search_status_line() {
        let rows = rows();
        let search = SearchMatches::by_name(&rows, " bossk ");
        assert_eq!(search.packages.len(), 2);
        assert!(
            search
                .status_line()
                .starts_with("2 Bundles contain 'bossk'")
        );
        let none = SearchMatches::by_name(&rows, "   ");
        assert!(none.packages.is_empty());
    }

    #[test]
    fn rows_from_manifest_mark_audio_packages() {
        let manifest = Manifest {
            packages: vec![
                Package {
                    name: "charui_bossk".into(),
                    size: 10,
                    ..Package::default()
                },
                Package {
                    name: "audio_dataprimer".into(),
                    kind: PackageKind::AudioPackage,
                    ..Package::default()
                },
            ],
            ..Manifest::default()
        };
        let rows = PackageRow::from_manifest(&manifest);
        assert_eq!(rows[0].kinds, None);
        assert!(rows[1].kinds.is_some_and(|k| k.contains(AssetKind::Audio)));
        assert_eq!(rows[1].id, 1);
    }

    #[test]
    fn shown_drops_other() {
        let kinds: AssetKinds = [AssetKind::Other, AssetKind::Mesh].into_iter().collect();
        assert_eq!(shown(kinds).len(), 1);
    }

    #[test]
    fn bytes_format() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(12_300), "12.3 KB");
        assert_eq!(format_bytes(4_500_000), "4.50 MB");
        assert_eq!(format_bytes(1_200_000_000), "1.20 GB");
    }
}
