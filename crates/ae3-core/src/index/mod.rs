//! The Asset Index: which Assets, by name and kind, each Bundle contains, so
//! that searching for an Asset by name finds its Bundles (ADR-0007).
//!
//! The index is built by range-probing every Bundle of a Manifest: only the
//! start of each file is fetched, as much as ae3-unity's
//! [`probe_prefix_filtered`](ae3_unity::probe_prefix_filtered) needs to list
//! its Assets and the names of the kinds search matches. That is a few
//! kilobytes for texture and icon Bundles, but most of the SerializedFile for
//! prefabs, whose named objects are spread all through it: over a whole
//! Manifest a little under half of the bytes. How much to fetch first is
//! predicted from earlier probes, so most Bundles take one request. Results
//! are kept per Bundle with the Content Hash and size they were probed at, in
//! one compact file per Platform under the cache directory
//! ([`default_index_path`]). A new Asset Version therefore only probes the
//! Bundles whose Content Hash changed; after an Engine Rebuild that is nearly
//! all of them, each predicted from what it needed before. Audio Packages
//! hold no Unity Assets and are indexed by name only.
//!
//! - [`IndexBuilder`] probes what is missing, in parallel, saving as it goes
//!   so that a cancelled or crashed run resumes where it stopped.
//! - [`AssetIndex`] is the index of one Manifest, loaded from that file, and
//!   answers [`AssetIndex::search`].
//! - [`IndexFile`] is the persisted form, for callers that manage it directly.

mod build;
mod file;
mod predict;
mod search;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ae3_unity::{CLASS_MONO_BEHAVIOUR, CLASS_MONO_SCRIPT, ObjectInfo, ProbeListing};
use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::bundle::{AssetKind, kind_of_class};
use crate::cache::PackageCache;
use crate::http::Transport;
use crate::manifest::{Manifest, PackageKind, category_of};
use crate::platform::Platform;
use crate::version::AssetVersion;

pub use build::{
    DEFAULT_FIRST_PROBE_LEN, DEFAULT_SAVE_EVERY, IndexBuilder, IndexFailure, IndexOptions,
    IndexPlan, IndexProgress, IndexReport, IndexRun,
};
pub use file::{BundleRecord, IndexFile};
pub use search::{Coverage, SearchHit, SearchResults};

use search::SearchTable;

/// The directory inside the cache root that holds the index files.
pub const INDEX_DIR: &str = "index";

/// Where the Asset Index of `platform` is kept under a cache root:
/// `<root>/index/<Platform>.ae3index`.
#[must_use]
pub fn default_index_path(cache_root: &Path, platform: Platform) -> PathBuf {
    cache_root
        .join(INDEX_DIR)
        .join(format!("{}.ae3index", platform.name()))
}

/// Why an index file could not be read or written.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum IndexError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The file is not an index file, or is damaged.
    #[error("{}: not a readable Asset Index: {detail}", path.display())]
    Corrupt { path: PathBuf, detail: String },
    /// The file indexes another Platform.
    #[error("{}: indexes {found}, not {expected}", path.display())]
    PlatformMismatch {
        path: PathBuf,
        expected: Platform,
        found: String,
    },
}

/// A set of [`AssetKind`]s: which kinds of Asset a Bundle holds.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct AssetKinds(u8);

impl AssetKinds {
    /// No kinds.
    pub const EMPTY: Self = Self(0);

    fn bit(kind: AssetKind) -> u8 {
        1 << kind_code(kind)
    }

    /// The set from its bits, as [`AssetKinds::bits`] returns them; bits of
    /// no known kind are dropped.
    #[must_use]
    pub fn from_bits(bits: u8) -> Self {
        let known = AssetKind::ALL.iter().fold(0, |acc, &k| acc | Self::bit(k));
        Self(bits & known)
    }

    /// One bit per kind, in [`AssetKind::ALL`] order.
    #[must_use]
    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn insert(&mut self, kind: AssetKind) {
        self.0 |= Self::bit(kind);
    }

    #[must_use]
    pub fn contains(self, kind: AssetKind) -> bool {
        self.0 & Self::bit(kind) != 0
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// The kinds in the set, in [`AssetKind::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = AssetKind> {
        AssetKind::ALL
            .into_iter()
            .filter(move |&k| self.contains(k))
    }
}

impl FromIterator<AssetKind> for AssetKinds {
    fn from_iter<I: IntoIterator<Item = AssetKind>>(iter: I) -> Self {
        let mut set = Self::EMPTY;
        for kind in iter {
            set.insert(kind);
        }
        set
    }
}

impl fmt::Debug for AssetKinds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// Comma-separated kind names, such as `texture, atlas`.
impl fmt::Display for AssetKinds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, kind) in self.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(kind.name())?;
        }
        Ok(())
    }
}

/// Serialized as a list of kind names.
impl Serialize for AssetKinds {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.len()))?;
        for kind in self.iter() {
            seq.serialize_element(&kind)?;
        }
        seq.end()
    }
}

/// The on-disk code of a kind: its position in [`AssetKind::ALL`].
fn kind_code(kind: AssetKind) -> u8 {
    match kind {
        AssetKind::Texture => 0,
        AssetKind::Atlas => 1,
        AssetKind::Sprite => 2,
        AssetKind::Text => 3,
        AssetKind::Structured => 4,
        AssetKind::Mesh => 5,
        AssetKind::Audio => 6,
        AssetKind::Other => 7,
    }
}

fn kind_from_code(code: u64) -> Option<AssetKind> {
    AssetKind::ALL.get(usize::try_from(code).ok()?).copied()
}

/// One named Asset of an indexed Bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexedAsset {
    /// The Asset's path id, for [`Selection::Assets`](crate::Selection) and
    /// [`LoadedBundle::asset`](crate::LoadedBundle::asset).
    pub path_id: i64,
    pub kind: AssetKind,
    /// `m_Name`.
    pub name: String,
}

/// What probing one Bundle found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleContents {
    /// The Unity Version that built the Bundle.
    pub unity_version: String,
    /// The kinds of every Asset the Bundle holds, scene plumbing
    /// ([`AssetKind::Other`]) excepted.
    pub kinds: AssetKinds,
    /// How many objects the Bundle's SerializedFiles hold, of every kind.
    pub object_count: u32,
    /// How many bytes from the start of the file the listing needed.
    pub prefix_len: u64,
    /// The named Assets that search matches, sorted by name then path id.
    /// Unnamed Assets and scene plumbing (GameObjects, Transforms,
    /// MonoScripts, the AssetBundle object and so on) are left out.
    pub assets: Vec<IndexedAsset>,
}

/// MonoScript class names of NGUI Atlases.
const ATLAS_SCRIPTS: &[&str] = &["NGUIAtlas", "UIAtlas"];

impl BundleContents {
    /// The index entry for a probed Bundle.
    ///
    /// A probe has no TypeTrees at hand, so an Atlas is recognised by its
    /// name, as SWGOH names them: a MonoBehaviour named `…atlas` or
    /// `…atlas_ref`, in a Bundle whose MonoScripts (when it lists any)
    /// include `NGUIAtlas`. [`LoadedBundle`](crate::LoadedBundle) checks the
    /// fields instead and is authoritative.
    #[must_use]
    pub fn from_listing(listing: &ProbeListing) -> Self {
        let scripts = || {
            listing
                .assets
                .iter()
                .filter(|a| a.class_id == CLASS_MONO_SCRIPT)
        };
        let atlas_script = scripts().next().is_none()
            || scripts().any(|a| {
                a.name
                    .as_deref()
                    .is_some_and(|n| ATLAS_SCRIPTS.contains(&n))
            });

        let mut kinds = AssetKinds::EMPTY;
        let mut assets = Vec::new();
        for asset in &listing.assets {
            let mut kind = kind_of_class(asset.class_id, &asset.type_name);
            if asset.class_id == CLASS_MONO_BEHAVIOUR
                && atlas_script
                && asset.name.as_deref().is_some_and(is_atlas_name)
            {
                kind = AssetKind::Atlas;
            }
            if kind == AssetKind::Other {
                continue;
            }
            kinds.insert(kind);
            if let Some(name) = &asset.name {
                assets.push(IndexedAsset {
                    path_id: asset.path_id,
                    kind,
                    name: name.clone(),
                });
            }
        }
        assets.sort_by(|a, b| a.name.cmp(&b.name).then(a.path_id.cmp(&b.path_id)));
        Self {
            unity_version: listing.unity_version.clone(),
            kinds,
            object_count: u32::try_from(listing.assets.len()).unwrap_or(u32::MAX),
            prefix_len: listing.bytes_used,
            assets,
        }
    }
}

/// Whether the index needs an object's `m_Name`: every kind search lists,
/// plus MonoScripts, which tell NGUI Atlases apart. Skipping the rest (scene
/// plumbing such as GameObjects and Transforms) changes no index entry, and
/// lets a probe stop once the last needed name is decoded.
fn wants_name(object: &ObjectInfo) -> bool {
    object.class_id == CLASS_MONO_SCRIPT
        || kind_of_class(object.class_id, &object.type_name) != AssetKind::Other
}

fn is_atlas_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with("atlas") || lower.ends_with("atlas_ref")
}

/// Where a Package stands in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexStatus {
    /// An Audio Package (or a kind AE3 does not know): searchable by name
    /// only, and complete as such.
    NameOnly,
    /// A Bundle probed at its current Content Hash.
    Indexed(Arc<BundleContents>),
    /// A Bundle whose probe at its current Content Hash failed for good
    /// (it could not be parsed); the message says why.
    Failed(String),
    /// A Bundle not yet probed at its current Content Hash.
    Pending,
}

/// One Package of the Manifest, as the index knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedPackage {
    pub name: String,
    pub kind: PackageKind,
    pub content_hash: u64,
    /// Size on the CDN, in bytes.
    pub size: u64,
    pub status: IndexStatus,
}

impl IndexedPackage {
    #[must_use]
    pub fn category(&self) -> &str {
        category_of(&self.name)
    }

    /// The probe results, for an indexed Bundle.
    #[must_use]
    pub fn contents(&self) -> Option<&BundleContents> {
        match &self.status {
            IndexStatus::Indexed(contents) => Some(contents),
            _ => None,
        }
    }

    /// The kinds of Asset the Package holds; empty when unknown. An Audio
    /// Package reports [`AssetKind::Audio`].
    #[must_use]
    pub fn kinds(&self) -> AssetKinds {
        match (&self.status, self.kind) {
            (IndexStatus::Indexed(contents), _) => contents.kinds,
            (_, PackageKind::AudioPackage) => [AssetKind::Audio].into_iter().collect(),
            _ => AssetKinds::EMPTY,
        }
    }

    /// Whether search sees everything the index can know about the Package.
    #[must_use]
    pub fn is_indexed(&self) -> bool {
        matches!(self.status, IndexStatus::NameOnly | IndexStatus::Indexed(_))
    }

    /// The Package's named Assets; empty unless indexed.
    #[must_use]
    pub fn assets(&self) -> &[IndexedAsset] {
        self.contents().map_or(&[], |c| &c.assets)
    }
}

/// The lowercase name of a Package kind, as JSON output spells it.
#[must_use]
pub fn package_kind_name(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Bundle => "bundle",
        PackageKind::AudioPackage => "audio",
        PackageKind::Other(_) => "other",
    }
}

impl Serialize for IndexedPackage {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Out<'a> {
            name: &'a str,
            kind: &'static str,
            category: &'a str,
            content_hash: u64,
            size: u64,
            status: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<&'a str>,
            kinds: AssetKinds,
            #[serde(skip_serializing_if = "Option::is_none")]
            contents: Option<&'a BundleContents>,
        }
        let (status, error) = match &self.status {
            IndexStatus::NameOnly => ("name-only", None),
            IndexStatus::Indexed(_) => ("indexed", None),
            IndexStatus::Failed(e) => ("failed", Some(e.as_str())),
            IndexStatus::Pending => ("pending", None),
        };
        Out {
            name: &self.name,
            kind: package_kind_name(self.kind),
            category: self.category(),
            content_hash: self.content_hash,
            size: self.size,
            status,
            error,
            kinds: self.kinds(),
            contents: self.contents(),
        }
        .serialize(s)
    }
}

/// The Asset Index of one Manifest: every Package with what is known of its
/// Assets, ready to search.
#[derive(Debug, Clone)]
pub struct AssetIndex {
    platform: Platform,
    asset_version: AssetVersion,
    /// Sorted by name, then kind.
    packages: Vec<IndexedPackage>,
    table: SearchTable,
}

impl AssetIndex {
    /// The index of `manifest` from the records in `file`. Bundles whose
    /// record is missing or of another Content Hash are
    /// [`IndexStatus::Pending`].
    #[must_use]
    pub fn new(platform: Platform, manifest: &Manifest, file: &IndexFile) -> Self {
        let mut packages: Vec<IndexedPackage> = manifest
            .packages
            .iter()
            .map(|p| {
                let status = if p.kind == PackageKind::Bundle {
                    match file.record_for(p) {
                        Some(BundleRecord {
                            result: Ok(contents),
                            ..
                        }) => IndexStatus::Indexed(Arc::clone(contents)),
                        Some(BundleRecord { result: Err(e), .. }) => IndexStatus::Failed(e.clone()),
                        None => IndexStatus::Pending,
                    }
                } else {
                    IndexStatus::NameOnly
                };
                IndexedPackage {
                    name: p.name.clone(),
                    kind: p.kind,
                    content_hash: p.content_hash,
                    size: p.size,
                    status,
                }
            })
            .collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name).then(a.kind.code().cmp(&b.kind.code())));
        let table = SearchTable::build(&packages);
        Self {
            platform,
            asset_version: manifest.asset_version,
            packages,
            table,
        }
    }

    /// The index of `manifest` from the index file at `path`; a missing file
    /// gives an index with every Bundle pending.
    pub fn load(path: &Path, platform: Platform, manifest: &Manifest) -> Result<Self, IndexError> {
        let file = IndexFile::load(path, platform)?;
        Ok(Self::new(platform, manifest, &file))
    }

    /// The index of `manifest` from the cache's index file
    /// ([`default_index_path`]).
    pub fn open<T: Transport>(
        cache: &PackageCache<T>,
        platform: Platform,
        manifest: &Manifest,
    ) -> Result<Self, IndexError> {
        Self::load(
            &default_index_path(cache.root(), platform),
            platform,
            manifest,
        )
    }

    #[must_use]
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// The Asset Version of the Manifest this index describes.
    #[must_use]
    pub fn asset_version(&self) -> AssetVersion {
        self.asset_version
    }

    /// Every Package, sorted by name.
    #[must_use]
    pub fn packages(&self) -> &[IndexedPackage] {
        &self.packages
    }

    /// The Package named `name`; a `.bundle` or `.wwpkg` suffix is allowed.
    #[must_use]
    pub fn package(&self, name: &str) -> Option<&IndexedPackage> {
        let bare = name
            .strip_suffix(".bundle")
            .or_else(|| name.strip_suffix(".wwpkg"))
            .unwrap_or(name);
        let at = self.packages.partition_point(|p| p.name.as_str() < bare);
        self.packages.get(at).filter(|p| p.name == bare)
    }

    /// The kinds of Asset the Package `name` holds, when it is indexed.
    #[must_use]
    pub fn kinds_of(&self, name: &str) -> Option<AssetKinds> {
        self.package(name)
            .filter(|p| p.is_indexed())
            .map(IndexedPackage::kinds)
    }

    /// How much of the Manifest is indexed.
    #[must_use]
    pub fn coverage(&self) -> Coverage {
        let mut c = Coverage::default();
        for p in &self.packages {
            match p.kind {
                PackageKind::Bundle => {
                    c.total_bundles += 1;
                    match p.status {
                        IndexStatus::Indexed(_) => c.indexed_bundles += 1,
                        IndexStatus::Failed(_) => c.failed_bundles += 1,
                        IndexStatus::NameOnly | IndexStatus::Pending => {}
                    }
                }
                PackageKind::AudioPackage => c.audio_packages += 1,
                PackageKind::Other(_) => c.other_packages += 1,
            }
        }
        c
    }

    /// Every Package whose name, or the name of one of whose indexed Assets,
    /// contains `query`, ignoring case. Hits are in name order. An empty
    /// query matches nothing.
    #[must_use]
    pub fn search(&self, query: &str) -> SearchResults<'_> {
        self.table.search(&self.packages, query, self.coverage())
    }
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;
