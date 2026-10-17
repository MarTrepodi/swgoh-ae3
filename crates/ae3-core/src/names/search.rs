//! Matching display names, and joining the matches to the Asset Index.
//!
//! Names and queries are compared through [`fold_name`]: lowercase, with
//! punctuation dropped, so `qira` finds "Qi'ra" and `ig88` finds "IG-88".
//! A match ranks by where the query sits in the name: the whole name, its
//! start, the start of a later word, or anywhere.

use std::cell::OnceCell;
use std::collections::HashMap;

use serde::{Serialize, Serializer};

use super::{GameEntity, NameIndex};
use crate::index::{AssetIndex, IndexedPackage, SearchResults};

/// How well a name matched a query, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchRank {
    /// The name is the query.
    Exact,
    /// The name starts with the query.
    Prefix,
    /// A later word of the name starts with the query.
    WordPrefix,
    /// The query is somewhere inside the name.
    Substring,
}

/// A name as search compares it: lowercase, letters and digits only, words
/// separated by single spaces.
#[must_use]
pub fn fold_name(name: &str) -> String {
    let mut folded = String::with_capacity(name.len());
    let mut space = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            if space && !folded.is_empty() {
                folded.push(' ');
            }
            space = false;
            folded.extend(c.to_lowercase());
        } else if c.is_whitespace() {
            space = true;
        }
    }
    folded
}

/// How `name` (folded) matches `query` (folded, not empty), if it does.
fn rank(name: &str, query: &str) -> Option<MatchRank> {
    if name == query {
        return Some(MatchRank::Exact);
    }
    let mut best = None;
    for (at, _) in name.match_indices(query) {
        let rank = if at == 0 {
            MatchRank::Prefix
        } else if name[..at].ends_with(' ') {
            MatchRank::WordPrefix
        } else {
            MatchRank::Substring
        };
        best = Some(best.map_or(rank, |b: MatchRank| b.min(rank)));
        if rank == MatchRank::Prefix {
            break;
        }
    }
    best
}

/// One Game Entity whose display name matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameMatch<'a> {
    pub entity: &'a GameEntity,
    pub rank: MatchRank,
}

impl NameIndex {
    /// Every Game Entity whose display name contains `query` (compared
    /// through [`fold_name`]), best match first: by [`MatchRank`], then
    /// kind, then shorter and alphabetically earlier names. An empty query
    /// matches nothing.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<NameMatch<'_>> {
        let query = fold_name(query);
        if query.is_empty() {
            return Vec::new();
        }
        let mut matches: Vec<(MatchRank, usize)> = self
            .folded
            .iter()
            .enumerate()
            .filter_map(|(i, name)| rank(name, &query).map(|r| (r, i)))
            .collect();
        matches.sort_by(|&(ra, a), &(rb, b)| {
            let (ea, eb) = (&self.entities[a], &self.entities[b]);
            ra.cmp(&rb)
                .then(ea.kind.cmp(&eb.kind))
                .then(self.folded[a].len().cmp(&self.folded[b].len()))
                .then(ea.name.cmp(&eb.name))
                .then(ea.context.cmp(&eb.context))
                .then(ea.id.cmp(&eb.id))
        });
        matches
            .into_iter()
            .map(|(rank, i)| NameMatch {
                entity: &self.entities[i],
                rank,
            })
            .collect()
    }
}

/// A Game Entity in [`SearchResults::entities`], with the Bundles it links
/// to that the Manifest has.
#[derive(Debug, Clone)]
pub struct EntityHit<'a> {
    pub entity: &'a GameEntity,
    pub rank: MatchRank,
    /// The linked Bundles present in the Manifest, in the entity's order.
    pub bundles: Vec<&'a IndexedPackage>,
}

/// Serialized flat: `{kind, id, name, context?, rank, bundles: [name]}`.
impl Serialize for EntityHit<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Out<'a> {
            kind: &'static str,
            id: &'a str,
            name: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            context: Option<&'a str>,
            rank: MatchRank,
            bundles: Vec<&'a str>,
        }
        Out {
            kind: self.entity.kind.name(),
            id: &self.entity.id,
            name: &self.entity.name,
            context: self.entity.context.as_deref(),
            rank: self.rank,
            bundles: self.bundles.iter().map(|p| p.name.as_str()).collect(),
        }
        .serialize(s)
    }
}

impl AssetIndex {
    /// [`AssetIndex::search`], plus the Game Entities of `names` whose
    /// display names match `query`, in [`SearchResults::entities`] (best
    /// first), each with the linked Bundles this Manifest has. Bundle names
    /// are matched ignoring case.
    #[must_use]
    pub fn search_with_names<'a>(&'a self, query: &str, names: &'a NameIndex) -> SearchResults<'a> {
        let mut results = self.search(query);
        // Manifest names are lowercase in practice; a case-folded lookup is
        // built only for a Manifest with a name that is not.
        let folded: OnceCell<Option<HashMap<String, &IndexedPackage>>> = OnceCell::new();
        let find = |name: &str| {
            self.package(name).or_else(|| {
                folded
                    .get_or_init(|| {
                        let packages = self.packages();
                        packages
                            .iter()
                            .any(|p| p.name.chars().any(char::is_uppercase))
                            .then(|| {
                                packages
                                    .iter()
                                    .map(|p| (p.name.to_lowercase(), p))
                                    .collect()
                            })
                    })
                    .as_ref()?
                    .get(&name.to_lowercase())
                    .copied()
            })
        };
        results.entities = names
            .search(query)
            .into_iter()
            .map(|m| EntityHit {
                entity: m.entity,
                rank: m.rank,
                bundles: m.entity.bundles.iter().filter_map(|b| find(b)).collect(),
            })
            .collect();
        results
    }
}
