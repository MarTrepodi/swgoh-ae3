//! The Name Index: the game's display names, such as "Bossk" or "Hunting
//! Party", for the Game Entities that use them, each with the Bundles it
//! links to (ADR-0008, superseding the deferral in ADR-0007).
//!
//! It is derived from the community gamedata mirror on GitHub
//! ([`GAMEDATA_URL`]): Units, Abilities (reached through Skills), Gear,
//! Materials and Player Portraits, with their names in one [`Language`]. The
//! texture and prefab references there (`tex.charui_bossk`,
//! `unit.char_bossk_pre`) name Bundles once their prefix is stripped.
//!
//! The source files are large (about 25 MB, one of them 18 MB of JSON), so
//! they are stream-parsed as they download and only the compact derived
//! index (a few hundred kilobytes) is kept, at [`default_name_index_path`].
//! [`NameIndex::open`] rebuilds it once per gamedata version and falls back
//! to the cached index when GitHub cannot be reached.
//!
//! [`AssetIndex::search_with_names`](crate::AssetIndex::search_with_names)
//! puts the matching Game Entities, with the linked Bundles the Manifest
//! has, above the Asset and Bundle matches.

mod gamedata;
mod search;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::Language;
use crate::http::{HttpError, Transport};

pub use search::{EntityHit, MatchRank, NameMatch, fold_name};

/// Where the community gamedata files are published.
pub const GAMEDATA_URL: &str = "https://raw.githubusercontent.com/swgoh-utils/gamedata/main/";
/// The mirror's version file: the gamedata and localization versions, which
/// the mirror advances only once every file of that version is uploaded.
pub const VERSIONS_FILE: &str = "allVersions.json";
/// The directory inside the cache root that holds the Name Index files.
pub const NAMES_DIR: &str = "names";
/// The version of the Name Index file layout.
const FORMAT: u32 = 1;

/// Where the Name Index for `language` is kept under a cache root:
/// `<root>/names/<language>.json`, such as `names/ENG_US.json`.
#[must_use]
pub fn default_name_index_path(cache_root: &Path, language: Language) -> PathBuf {
    cache_root
        .join(NAMES_DIR)
        .join(format!("{}.json", language.name()))
}

/// Why the Name Index could not be read, written or built.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NamesError {
    #[error(transparent)]
    Http(#[from] HttpError),
    /// A gamedata file could not be parsed.
    #[error("{file}: {detail}")]
    Gamedata { file: String, detail: String },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The cached file is not a Name Index this build can read.
    #[error("{}: not a readable Name Index: {detail}", path.display())]
    Corrupt { path: PathBuf, detail: String },
}

/// The kind of a Game Entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntityKind {
    /// A character or ship.
    Unit,
    Ability,
    Gear,
    Material,
    PlayerPortrait,
}

impl EntityKind {
    pub const ALL: [Self; 5] = [
        Self::Unit,
        Self::Ability,
        Self::Gear,
        Self::Material,
        Self::PlayerPortrait,
    ];

    /// The name JSON output uses, such as `player-portrait`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Ability => "ability",
            Self::Gear => "gear",
            Self::Material => "material",
            Self::PlayerPortrait => "player-portrait",
        }
    }

    /// The name people read, such as `Player Portrait`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Unit => "Unit",
            Self::Ability => "Ability",
            Self::Gear => "Gear",
            Self::Material => "Material",
            Self::PlayerPortrait => "Player Portrait",
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A thing in the game with a display name: a Unit, Ability, Gear, Material
/// or Player Portrait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameEntity {
    pub kind: EntityKind,
    /// The gamedata id: a Unit's base id (`BOSSK`), an Ability's id
    /// (`specialability_bossk01`), and so on.
    pub id: String,
    /// The display name in the index's language, markup removed.
    pub name: String,
    /// Who the entity belongs to, when that tells same-named entities apart:
    /// the display name of an Ability's Unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// The names of the Bundles the entity links to, lowercase, most
    /// specific first. A name may be missing from a given Manifest.
    pub bundles: Vec<String>,
}

/// The versions of the gamedata a Name Index was built from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GamedataVersions {
    /// The gamedata version, as `meta.json`'s `latestGamedataVersion`
    /// spells it, such as `0.40.6:i9YYIijBSxuHHNoSmCMBoQ`.
    pub gamedata: String,
    /// The localization bundle version, as `latestLocalizationBundleVersion`.
    pub localization: String,
}

/// The display names of the Game Entities in one language, ready to search.
#[derive(Debug, Clone)]
pub struct NameIndex {
    language: Language,
    versions: GamedataVersions,
    entities: Vec<GameEntity>,
    /// Each entity's name through [`fold_name`], in entity order.
    folded: Vec<String>,
}

/// The persisted form.
#[derive(Serialize, Deserialize)]
struct NameIndexFile {
    format: u32,
    language: Language,
    versions: GamedataVersions,
    entities: Vec<GameEntity>,
}

impl NameIndex {
    /// An index over `entities`, kept in the order given.
    #[must_use]
    pub fn new(language: Language, versions: GamedataVersions, entities: Vec<GameEntity>) -> Self {
        let folded = entities.iter().map(|e| fold_name(&e.name)).collect();
        Self {
            language,
            versions,
            entities,
            folded,
        }
    }

    #[must_use]
    pub fn language(&self) -> Language {
        self.language
    }

    #[must_use]
    pub fn versions(&self) -> &GamedataVersions {
        &self.versions
    }

    #[must_use]
    pub fn entities(&self) -> &[GameEntity] {
        &self.entities
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Reads the index at `path`; `None` when there is no file.
    pub fn load(path: &Path) -> Result<Option<Self>, NamesError> {
        let text = match fs::read(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(NamesError::Io {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let corrupt = |detail: String| NamesError::Corrupt {
            path: path.to_owned(),
            detail,
        };
        let file: NameIndexFile =
            serde_json::from_slice(&text).map_err(|e| corrupt(e.to_string()))?;
        if file.format != FORMAT {
            return Err(corrupt(format!("format {} is not {FORMAT}", file.format)));
        }
        Ok(Some(Self::new(file.language, file.versions, file.entities)))
    }

    /// Writes the index to `path` atomically, creating its directory.
    pub fn save(&self, path: &Path) -> Result<(), NamesError> {
        #[derive(Serialize)]
        struct Out<'a> {
            format: u32,
            language: Language,
            versions: &'a GamedataVersions,
            entities: &'a [GameEntity],
        }
        let io_err = |path: &Path| {
            let path = path.to_owned();
            move |source| NamesError::Io { path, source }
        };
        let bytes = serde_json::to_vec(&Out {
            format: FORMAT,
            language: self.language,
            versions: &self.versions,
            entities: &self.entities,
        })
        .map_err(|e| io_err(path)(io::Error::other(e)))?;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, bytes).map_err(io_err(&tmp))?;
        fs::rename(&tmp, path).map_err(io_err(path))
    }

    /// The versions of the gamedata the mirror currently publishes.
    pub fn fetch_versions<T: Transport + ?Sized>(
        transport: &T,
    ) -> Result<GamedataVersions, NamesError> {
        gamedata::fetch_versions(transport)
    }

    /// Downloads the gamedata of `versions` and builds the index for
    /// `language`. The files are parsed as they stream in; none is kept.
    pub fn build<T: Transport + ?Sized>(
        transport: &T,
        language: Language,
        versions: GamedataVersions,
        on_event: &mut dyn FnMut(NamesEvent<'_>),
    ) -> Result<Self, NamesError> {
        gamedata::build(transport, language, versions, on_event)
    }

    /// The Name Index for `language` from the cache under `cache_root`,
    /// brought up to date first when `update` is set.
    ///
    /// Updating fetches the mirror's small version file and rebuilds only
    /// when the gamedata or localization version changed. When GitHub cannot
    /// be reached, or the build fails, the cached index is used as it is.
    /// This never fails: the result says what happened, and has no index
    /// only when none is cached and none could be built.
    pub fn open<T: Transport + ?Sized>(
        transport: &T,
        cache_root: &Path,
        language: Language,
        update: bool,
        on_event: &mut dyn FnMut(NamesEvent<'_>),
    ) -> NameIndexLoad {
        let path = default_name_index_path(cache_root, language);
        let (index, status, problem) = Self::open_at(&path, transport, language, update, on_event);
        NameIndexLoad {
            index,
            status,
            problem,
            path,
        }
    }

    fn open_at<T: Transport + ?Sized>(
        path: &Path,
        transport: &T,
        language: Language,
        update: bool,
        on_event: &mut dyn FnMut(NamesEvent<'_>),
    ) -> (Option<Self>, NameIndexStatus, Option<String>) {
        let (cached, load_problem) = match Self::load(path) {
            Ok(index) => (index.filter(|i| i.language == language), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let fallback = |cached: Option<Self>, problem: String| match cached {
            Some(index) => (Some(index), NameIndexStatus::Stale, Some(problem)),
            None => (None, NameIndexStatus::Unavailable, Some(problem)),
        };
        if !update {
            return match cached {
                Some(index) => (Some(index), NameIndexStatus::Unchecked, None),
                None => fallback(
                    None,
                    load_problem.unwrap_or_else(|| "no Name Index is cached yet".to_owned()),
                ),
            };
        }
        let versions = match Self::fetch_versions(transport) {
            Ok(versions) => versions,
            Err(e) => return fallback(cached, e.to_string()),
        };
        match cached {
            Some(index) if index.versions == versions => {
                (Some(index), NameIndexStatus::Current, None)
            }
            cached => match Self::build(transport, language, versions, on_event) {
                Ok(index) => {
                    let problem = index.save(path).err().map(|e| e.to_string());
                    (Some(index), NameIndexStatus::Built, problem)
                }
                Err(e) => fallback(cached, e.to_string()),
            },
        }
    }
}

/// Progress while [`NameIndex::build`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NamesEvent<'a> {
    /// A build of `versions` is starting.
    Building { versions: &'a GamedataVersions },
    /// The gamedata file `file` is being downloaded and parsed.
    Downloading { file: &'a str },
}

/// How [`NameIndex::open`] came by its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NameIndexStatus {
    /// The cached index matches the mirror's current gamedata.
    Current,
    /// The index was just built from the mirror (and cached, unless
    /// [`NameIndexLoad::problem`] says otherwise).
    Built,
    /// The cached index, used without checking the mirror.
    Unchecked,
    /// The cached index, used because updating it failed.
    Stale,
    /// No index: none is cached and none could be built.
    Unavailable,
}

/// The result of [`NameIndex::open`].
#[derive(Debug, Clone)]
pub struct NameIndexLoad {
    pub index: Option<NameIndex>,
    pub status: NameIndexStatus,
    /// What went wrong, for a [`NameIndexStatus::Stale`] or
    /// [`NameIndexStatus::Unavailable`] index, or a built one that could not
    /// be saved.
    pub problem: Option<String>,
    /// Where the index is cached.
    pub path: PathBuf,
}

#[cfg(test)]
mod tests;
