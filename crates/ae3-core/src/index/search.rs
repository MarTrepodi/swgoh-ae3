//! Case-insensitive substring search over Package and Asset names.
//!
//! Every searchable name is lowercased once, when the index is built, into
//! one string with a NUL after each name: for each Package (in name order)
//! its own name, then the names of its indexed Assets. A query is then one
//! substring scan over that string, whose matches map back to names by
//! binary search. That keeps search to a few milliseconds over the full game
//! (about 11.6k Bundles and their Assets) with no per-name allocations.

use serde::{Serialize, Serializer};

use super::{AssetKinds, IndexedAsset, IndexedPackage, package_kind_name};
use crate::names::EntityHit;

/// Separates names in [`SearchTable::text`]; never part of a query.
const SEPARATOR: char = '\0';

/// The precomputed lowercase names.
#[derive(Debug, Clone, Default)]
pub(super) struct SearchTable {
    /// Every name, lowercased and NUL-terminated.
    text: String,
    /// Where each name starts in `text`.
    starts: Vec<usize>,
    /// The index (into `starts`) of each Package's own name; its Assets'
    /// names follow it.
    package_starts: Vec<usize>,
}

impl SearchTable {
    pub(super) fn build(packages: &[IndexedPackage]) -> Self {
        let names = packages.len() + packages.iter().map(|p| p.assets().len()).sum::<usize>();
        let mut table = Self {
            text: String::new(),
            starts: Vec::with_capacity(names),
            package_starts: Vec::with_capacity(packages.len()),
        };
        for package in packages {
            table.package_starts.push(table.starts.len());
            table.push(&package.name);
            for asset in package.assets() {
                table.push(&asset.name);
            }
        }
        table.text.shrink_to_fit();
        table
    }

    fn push(&mut self, name: &str) {
        self.starts.push(self.text.len());
        // Lowercasing never yields a NUL from other characters, so dropping
        // NULs keeps the separators unambiguous.
        self.text.extend(
            name.chars()
                .filter(|&c| c != SEPARATOR)
                .flat_map(char::to_lowercase),
        );
        self.text.push(SEPARATOR);
    }

    pub(super) fn search<'a>(
        &self,
        packages: &'a [IndexedPackage],
        query: &str,
        coverage: Coverage,
    ) -> SearchResults<'a> {
        let needle: String = query
            .trim()
            .chars()
            .filter(|&c| c != SEPARATOR)
            .flat_map(char::to_lowercase)
            .collect();
        let mut hits: Vec<SearchHit<'a>> = Vec::new();
        let mut pos = 0;
        while !needle.is_empty() {
            let Some(found) = self.text[pos..].find(&needle) else {
                break;
            };
            let at = pos + found;
            let name = self.starts.partition_point(|&s| s <= at) - 1;
            let package = self.package_starts.partition_point(|&s| s <= name) - 1;
            let local = name - self.package_starts[package];
            let entry = &packages[package];
            if hits.last().is_none_or(|h| !std::ptr::eq(h.package, entry)) {
                hits.push(SearchHit {
                    package: entry,
                    name_matched: false,
                    assets: Vec::new(),
                });
            }
            let hit = hits.last_mut().expect("pushed above");
            match local.checked_sub(1) {
                None => hit.name_matched = true,
                Some(asset) => hit.assets.push(&entry.assets()[asset]),
            }
            // One match per name is enough: go on from the next name.
            pos = self
                .starts
                .get(name + 1)
                .copied()
                .unwrap_or(self.text.len());
        }
        SearchResults {
            query: query.to_owned(),
            entities: Vec::new(),
            hits,
            coverage,
        }
    }
}

/// How much of a Manifest the index covers. Audio Packages are indexed by
/// name only and always count as covered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Coverage {
    /// Bundles probed at their current Content Hash.
    pub indexed_bundles: usize,
    /// Bundles whose probe failed for good at their current Content Hash.
    pub failed_bundles: usize,
    /// Every Bundle in the Manifest.
    pub total_bundles: usize,
    pub audio_packages: usize,
    /// Packages of a kind AE3 does not know (searchable by name).
    pub other_packages: usize,
}

impl Coverage {
    /// Bundles not yet probed at their current Content Hash.
    #[must_use]
    pub fn pending_bundles(&self) -> usize {
        self.total_bundles
            .saturating_sub(self.indexed_bundles + self.failed_bundles)
    }

    /// The indexed fraction of the Bundles, from 0 to 1 (1 when there are
    /// none).
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "a ratio for display")]
    pub fn ratio(&self) -> f64 {
        if self.total_bundles == 0 {
            1.0
        } else {
            self.indexed_bundles as f64 / self.total_bundles as f64
        }
    }

    /// Whether no Bundle is waiting to be probed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.pending_bundles() == 0
    }
}

/// The result of [`AssetIndex::search`](super::AssetIndex::search).
#[derive(Debug, Clone, Serialize)]
pub struct SearchResults<'a> {
    pub query: String,
    /// Game Entities whose display names matched, best first, when the
    /// search was given a Name Index
    /// ([`AssetIndex::search_with_names`](super::AssetIndex::search_with_names));
    /// otherwise empty.
    pub entities: Vec<EntityHit<'a>>,
    /// Matching Packages, in name order.
    pub hits: Vec<SearchHit<'a>>,
    /// How much of the Manifest was searchable. Assets of Bundles not yet
    /// indexed cannot match, so results are partial until it is complete.
    pub coverage: Coverage,
}

impl SearchResults<'_> {
    /// How many Packages matched; [`SearchResults::entities`] are not
    /// counted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// Whether no Package matched; [`SearchResults::entities`] are not
    /// counted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// Whether nothing at all matched: no Game Entity and no Package.
    #[must_use]
    pub fn nothing_matched(&self) -> bool {
        self.entities.is_empty() && self.hits.is_empty()
    }

    /// How many Assets matched, over all hits.
    #[must_use]
    pub fn asset_match_count(&self) -> usize {
        self.hits.iter().map(|h| h.assets.len()).sum()
    }
}

/// One Package that matched.
#[derive(Debug, Clone)]
pub struct SearchHit<'a> {
    pub package: &'a IndexedPackage,
    /// Whether the Package's own name matched.
    pub name_matched: bool,
    /// The Package's Assets whose names matched, in name order.
    pub assets: Vec<&'a IndexedAsset>,
}

impl SearchHit<'_> {
    #[must_use]
    pub fn package_name(&self) -> &str {
        &self.package.name
    }
}

/// Serialized flat: `{package, kind, category, name_matched, indexed, kinds,
/// assets: [{path_id, kind, name}]}`.
impl Serialize for SearchHit<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Out<'a> {
            package: &'a str,
            kind: &'static str,
            category: &'a str,
            name_matched: bool,
            indexed: bool,
            kinds: AssetKinds,
            assets: &'a [&'a IndexedAsset],
        }
        Out {
            package: &self.package.name,
            kind: package_kind_name(self.package.kind),
            category: self.package.category(),
            name_matched: self.name_matched,
            indexed: self.package.is_indexed(),
            kinds: self.package.kinds(),
            assets: &self.assets,
        }
        .serialize(s)
    }
}
