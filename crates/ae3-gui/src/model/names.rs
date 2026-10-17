//! Game Entities in the search: the ones whose display names match the
//! query, the one that narrows the Bundles table to its Bundles, and how far
//! the Name Index has loaded.

use std::fmt::Write as _;

use ae3_core::{EntityKind, NameIndex, NameIndexLoad, NameIndexStatus};

/// Identifies a Game Entity across searches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityKey {
    pub kind: EntityKind,
    pub id: String,
}

/// A Game Entity whose display name matched, owned so it outlives the Name
/// Index snapshot it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityMatch {
    pub kind: EntityKind,
    /// The gamedata id, such as `BOSSK`.
    pub id: String,
    /// The display name.
    pub name: String,
    /// Who it belongs to, such as an Ability's Unit.
    pub context: Option<String>,
    /// The linked Bundles the Manifest has, spelled as the Manifest spells
    /// them, most specific first.
    pub bundles: Vec<String>,
}

impl EntityMatch {
    #[must_use]
    pub fn key(&self) -> EntityKey {
        EntityKey {
            kind: self.kind,
            id: self.id.clone(),
        }
    }

    #[must_use]
    pub fn is(&self, key: &EntityKey) -> bool {
        self.kind == key.kind && self.id == key.id
    }

    /// The display name, with its context when it has one:
    /// "Prey On The Weak (Bossk)".
    #[must_use]
    pub fn label(&self) -> String {
        match &self.context {
            Some(context) => format!("{} ({context})", self.name),
            None => self.name.clone(),
        }
    }

    /// "Unit 'Bossk'".
    #[must_use]
    pub fn title(&self) -> String {
        format!("{} '{}'", self.kind.label(), self.label())
    }

    /// The Game Entities of `names` matching `query`, best first, each with
    /// the linked Bundles `find` knows: it maps a lowercase Bundle name to
    /// the Manifest's spelling, or `None` when the Manifest lacks it.
    #[must_use]
    pub fn search<'a>(
        names: &NameIndex,
        query: &str,
        find: impl Fn(&str) -> Option<&'a str>,
    ) -> Vec<Self> {
        names
            .search(query)
            .into_iter()
            .map(|m| Self {
                kind: m.entity.kind,
                id: m.entity.id.clone(),
                name: m.entity.name.clone(),
                context: m.entity.context.clone(),
                bundles: m
                    .entity
                    .bundles
                    .iter()
                    .filter_map(|b| find(b).map(str::to_owned))
                    .collect(),
            })
            .collect()
    }
}

/// How far the Name Index has got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NamesState {
    /// Not asked for (tests, or before startup).
    #[default]
    Idle,
    /// Being read from the cache or checked against the mirror; `step`
    /// says what it is doing.
    Loading { step: String },
    /// In use. `problem` is set when it could not be brought up to date (or
    /// saved), and `checking` while the cached copy shown is still being
    /// compared with the mirror.
    Ready {
        entities: usize,
        status: NameIndexStatus,
        problem: Option<String>,
        checking: bool,
    },
    /// No index: none is cached and none could be built.
    Unavailable { problem: String },
}

impl NamesState {
    /// The state after `load`; `checking` while an update is still to come.
    #[must_use]
    pub fn from_load(load: &NameIndexLoad, checking: bool) -> Self {
        Self::new(
            load.index.as_ref(),
            load.status,
            load.problem.clone(),
            checking,
        )
    }

    /// The state with `index` in use (or none), as [`NameIndex::open`]
    /// described it.
    #[must_use]
    pub fn new(
        index: Option<&NameIndex>,
        status: NameIndexStatus,
        problem: Option<String>,
        checking: bool,
    ) -> Self {
        match index {
            Some(index) => Self::Ready {
                entities: index.len(),
                status,
                problem,
                checking,
            },
            None if checking => Self::Loading {
                step: "Checking for display names…".to_owned(),
            },
            None => Self::Unavailable {
                problem: problem.unwrap_or_else(|| "unknown problem".to_owned()),
            },
        }
    }

    /// A step of the load was reported. It shows only while no index is in
    /// use; a cached one stays listed while it is brought up to date.
    pub fn progress(&mut self, step: String) {
        if matches!(self, Self::Idle | Self::Loading { .. }) {
            *self = Self::Loading { step };
        }
    }

    /// A line for under the search box, and whether it is a warning.
    #[must_use]
    pub fn line(&self) -> Option<(String, bool)> {
        match self {
            Self::Idle => None,
            Self::Loading { step } => Some((step.clone(), false)),
            Self::Ready {
                entities,
                status,
                problem,
                checking,
            } => {
                let s = if *entities == 1 { "" } else { "s" };
                let mut line = format!("{entities} display name{s}");
                match (status, problem) {
                    (NameIndexStatus::Stale, Some(p)) => {
                        let _ = write!(line, " (could not update: {p})");
                        return Some((line, true));
                    }
                    (NameIndexStatus::Built, Some(p)) => {
                        let _ = write!(line, " (could not save: {p})");
                        return Some((line, true));
                    }
                    _ if *checking => line.push_str(" · checking for newer gamedata…"),
                    _ => {}
                }
                Some((line, false))
            }
            Self::Unavailable { problem } => {
                Some((format!("Display names cannot match: {problem}"), true))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ae3_core::{GameEntity, GamedataVersions, Language};

    use super::*;

    fn index() -> NameIndex {
        NameIndex::new(
            Language::default(),
            GamedataVersions::default(),
            vec![GameEntity {
                kind: EntityKind::Ability,
                id: "specialability_bossk01".into(),
                name: "Prey On The Weak".into(),
                context: Some("Bossk".into()),
                bundles: vec!["abilityui_bossk".into(), "missing_bundle".into()],
            }],
        )
    }

    #[test]
    fn search_keeps_only_bundles_the_manifest_has() {
        let found = EntityMatch::search(&index(), "weak", |b| {
            (b == "abilityui_bossk").then_some("abilityui_bossk")
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bundles, ["abilityui_bossk"]);
        assert_eq!(found[0].label(), "Prey On The Weak (Bossk)");
        assert_eq!(found[0].title(), "Ability 'Prey On The Weak (Bossk)'");
        assert!(found[0].is(&found[0].key()));
    }

    #[test]
    fn state_lines() {
        let load = |index, status, problem: Option<&str>| NameIndexLoad {
            index,
            status,
            problem: problem.map(str::to_owned),
            path: PathBuf::new(),
        };
        let unchecked =
            NamesState::from_load(&load(Some(index()), NameIndexStatus::Unchecked, None), true);
        assert_eq!(
            unchecked.line(),
            Some((
                "1 display name · checking for newer gamedata…".to_owned(),
                false
            ))
        );
        let stale = NamesState::from_load(
            &load(Some(index()), NameIndexStatus::Stale, Some("offline")),
            false,
        );
        assert_eq!(
            stale.line(),
            Some((
                "1 display name (could not update: offline)".to_owned(),
                true
            ))
        );
        let none = load(None, NameIndexStatus::Unavailable, Some("offline"));
        assert!(matches!(
            NamesState::from_load(&none, true),
            NamesState::Loading { .. }
        ));
        assert_eq!(
            NamesState::from_load(&none, false).line(),
            Some(("Display names cannot match: offline".to_owned(), true))
        );
        assert_eq!(NamesState::Idle.line(), None);
    }
}
