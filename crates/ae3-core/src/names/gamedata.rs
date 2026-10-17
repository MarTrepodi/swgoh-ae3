//! Building the Name Index from the community gamedata files.
//!
//! Every file is `{"version": "...", "data": ...}`. The files are parsed
//! while they download: the HTTP body is piped to a parser thread through a
//! small bounded channel, Brotli-decoded when the file is `.br`, and read
//! with serde's streaming visitors, which keep only the few fields used and
//! skip the rest without building it. Peak memory therefore stays at the
//! derived data plus a few hundred kilobytes of buffers, even for the 18 MB
//! `ability.json` and the 67 MB that `units.json.br` decodes to.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::marker::PhantomData;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use serde::Deserialize;
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor,
};

use super::{
    EntityKind, GAMEDATA_URL, GameEntity, GamedataVersions, NameIndex, NamesError, NamesEvent,
    VERSIONS_FILE,
};
use crate::config::Language;
use crate::guard::panic_message;
use crate::http::{HttpError, Transport};

/// How many body chunks may wait between the download and the parser.
const PIPE_CHUNKS: usize = 8;
/// The read buffer in front of the JSON parser and the Brotli window buffer.
const BUFFER_LEN: usize = 64 * 1024;

pub(super) fn fetch_versions<T: Transport + ?Sized>(
    transport: &T,
) -> Result<GamedataVersions, NamesError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AllVersions {
        game_version: String,
        locale_version: String,
    }
    let url = format!("{GAMEDATA_URL}{VERSIONS_FILE}");
    let body = transport.get_bytes(&url)?;
    let parsed: AllVersions = serde_json::from_slice(&body).map_err(|e| NamesError::Gamedata {
        file: VERSIONS_FILE.to_owned(),
        detail: e.to_string(),
    })?;
    if parsed.game_version.is_empty() || parsed.locale_version.is_empty() {
        return Err(NamesError::Gamedata {
            file: VERSIONS_FILE.to_owned(),
            detail: "no gameVersion or localeVersion".to_owned(),
        });
    }
    Ok(GamedataVersions {
        gamedata: parsed.game_version,
        localization: parsed.locale_version,
    })
}

// --- Rows: only the fields the index uses. Every field defaults, so a row
// --- that lacks one is kept with it empty rather than failing the file.

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SkillRow {
    id: String,
    ability_reference: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct SkillRef {
    #[serde(rename = "skillId")]
    skill_id: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RelicDefinition {
    texture: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UnitRow {
    base_id: String,
    name_key: String,
    thumbnail_name: String,
    unit_prefab: String,
    relic_definition: Option<RelicDefinition>,
    era_thumbnail_name: String,
    battle_portrait_prefab: String,
    battle_portrait_texture: String,
    thumb_image_turn_order: String,
    skill_reference: Vec<SkillRef>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct AbilityRow {
    id: String,
    name_key: String,
    icon: String,
    prefab_name: String,
}

/// A row of `equipment.json` or `material.json`.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ItemRow {
    id: String,
    name_key: String,
    icon_key: String,
    /// Set on a Material that is a Unit's shards; the Unit covers it.
    unit_def_reference: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PortraitRow {
    id: String,
    name_key: String,
    icon: String,
}

/// A Game Entity before its name is looked up.
struct Draft {
    kind: EntityKind,
    id: String,
    name_key: String,
    /// The index (into the drafts) of the Unit an Ability belongs to.
    owner: Option<usize>,
    bundles: Vec<String>,
}

/// The Bundle name a gamedata reference such as `tex.charui_bossk` or
/// `unit.char_bossk_pre` points to: the reference without its prefix,
/// lowercase. `None` for an empty reference or a placeholder.
pub(super) fn bundle_name(reference: &str) -> Option<String> {
    let reference = reference.trim();
    let bare = ["tex.", "unit.", "prefab.", "ui."]
        .iter()
        .find_map(|p| reference.strip_prefix(p))
        .unwrap_or(reference);
    let valid = !bare.is_empty()
        && bare
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    (valid && !bare.eq_ignore_ascii_case("placeholder")).then(|| bare.to_ascii_lowercase())
}

/// The Bundle names of `references`, in order, without repeats.
fn bundle_names<'a>(references: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in references.into_iter().filter_map(bundle_name) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// A display name without the game's markup (`[c][ffff33]…[-][/c]`, `[b]`)
/// and with runs of whitespace collapsed.
pub(super) fn clean_display_name(raw: &str) -> String {
    fn is_markup(tag: &str) -> bool {
        let tag = tag.strip_prefix('/').unwrap_or(tag);
        matches!(tag, "" | "-" | "c" | "b" | "i" | "u" | "s" | "sub" | "sup")
            || (matches!(tag.len(), 6 | 8) && tag.bytes().all(|b| b.is_ascii_hexdigit()))
    }
    let mut text = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('[') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find(']') {
            Some(close) if is_markup(&after[..close]) => rest = &after[close + 1..],
            _ => {
                text.push('[');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn build<T: Transport + ?Sized>(
    transport: &T,
    language: Language,
    versions: GamedataVersions,
    on_event: &mut dyn FnMut(NamesEvent<'_>),
) -> Result<NameIndex, NamesError> {
    on_event(NamesEvent::Building {
        versions: &versions,
    });
    let mut files = Files {
        transport,
        on_event,
    };
    let mut drafts = unit_and_ability_drafts(&mut files)?;
    item_drafts(&mut files, &mut drafts)?;

    let keys: HashSet<String> = drafts
        .iter()
        .filter(|d| !d.name_key.is_empty())
        .map(|d| d.name_key.clone())
        .collect();
    let loc_file = format!("Loc_{}.txt.json.br", language.name());
    let texts = files.open(&loc_file).strings(&keys)?;
    drop(keys);
    Ok(NameIndex::new(
        language,
        versions,
        assemble(&drafts, &texts),
    ))
}

/// Opens gamedata files, reporting each.
struct Files<'a, T: ?Sized> {
    transport: &'a T,
    on_event: &'a mut dyn FnMut(NamesEvent<'_>),
}

impl<'a, T: Transport + ?Sized> Files<'a, T> {
    fn open(&mut self, file: &str) -> RowFetch<'a, T> {
        (self.on_event)(NamesEvent::Downloading { file });
        RowFetch {
            transport: self.transport,
            file: file.to_owned(),
        }
    }
}

/// The Units, then the Abilities their Skills use.
fn unit_and_ability_drafts<T: Transport + ?Sized>(
    files: &mut Files<'_, T>,
) -> Result<Vec<Draft>, NamesError> {
    // Skills lead to the Abilities players see, out of thousands of others.
    let mut skill_ability: HashMap<String, String> = HashMap::new();
    files.open("skill.json").each(|row: SkillRow| {
        if !row.ability_reference.is_empty() {
            skill_ability.insert(row.id, row.ability_reference);
        }
    })?;

    let mut drafts: Vec<Draft> = Vec::new();
    let mut seen_units: HashSet<String> = HashSet::new();
    // Ability id -> the draft of the first Unit that has it.
    let mut ability_owner: HashMap<String, usize> = HashMap::new();
    files.open("units.json.br").each(|row: UnitRow| {
        // units.json repeats each Unit once per star level.
        if row.base_id.is_empty() || !seen_units.insert(row.base_id.clone()) {
            return;
        }
        let unit = drafts.len();
        for skill in &row.skill_reference {
            if let Some(ability) = skill_ability.get(&skill.skill_id) {
                ability_owner.entry(ability.clone()).or_insert(unit);
            }
        }
        let relic = row
            .relic_definition
            .as_ref()
            .map_or("", |r| r.texture.as_str());
        drafts.push(Draft {
            kind: EntityKind::Unit,
            bundles: bundle_names([
                row.thumbnail_name.as_str(),
                &row.unit_prefab,
                relic,
                &row.era_thumbnail_name,
                &row.battle_portrait_prefab,
                &row.battle_portrait_texture,
                &row.thumb_image_turn_order,
            ]),
            id: row.base_id,
            name_key: row.name_key,
            owner: None,
        });
    })?;

    let wanted: HashSet<&str> = skill_ability.values().map(String::as_str).collect();
    let mut abilities: Vec<Draft> = Vec::new();
    files.open("ability.json").each(|row: AbilityRow| {
        if wanted.contains(row.id.as_str()) {
            abilities.push(Draft {
                kind: EntityKind::Ability,
                bundles: bundle_names([row.icon.as_str(), &row.prefab_name]),
                owner: ability_owner.get(&row.id).copied(),
                id: row.id,
                name_key: row.name_key,
            });
        }
    })?;
    drafts.append(&mut abilities);
    Ok(drafts)
}

/// Gear, Materials (not Unit shards) and Player Portraits.
fn item_drafts<T: Transport + ?Sized>(
    files: &mut Files<'_, T>,
    drafts: &mut Vec<Draft>,
) -> Result<(), NamesError> {
    let item = |kind: EntityKind| {
        move |row: ItemRow| Draft {
            kind,
            bundles: bundle_names([row.icon_key.as_str()]),
            id: row.id,
            name_key: row.name_key,
            owner: None,
        }
    };
    let to_gear = item(EntityKind::Gear);
    files
        .open("equipment.json")
        .each(|row: ItemRow| drafts.push(to_gear(row)))?;
    let to_material = item(EntityKind::Material);
    files.open("material.json").each(|row: ItemRow| {
        if row.unit_def_reference.is_empty() {
            drafts.push(to_material(row));
        }
    })?;
    files.open("playerPortrait.json").each(|row: PortraitRow| {
        drafts.push(Draft {
            kind: EntityKind::PlayerPortrait,
            bundles: bundle_names([row.icon.as_str()]),
            id: row.id,
            name_key: row.name_key,
            owner: None,
        });
    })?;
    Ok(())
}

/// The Game Entities of `drafts` that have a display name in `texts`, with
/// repeats dropped.
fn assemble(drafts: &[Draft], texts: &HashMap<String, String>) -> Vec<GameEntity> {
    let names: Vec<Option<String>> = drafts
        .iter()
        .map(|d| {
            texts
                .get(&d.name_key)
                .map(|t| clean_display_name(t))
                .filter(|n| !n.is_empty())
        })
        .collect();
    let mut seen: HashSet<(EntityKind, &str, Option<&str>, &[String])> = HashSet::new();
    let mut entities = Vec::with_capacity(drafts.len());
    for (draft, name) in drafts.iter().zip(&names) {
        // An entity without a display name cannot be found by one.
        let Some(name) = name else { continue };
        let context = draft.owner.and_then(|o| names[o].as_deref());
        // Materials repeat per source (nine "Acumen Mk1"); keep one.
        if !seen.insert((draft.kind, name, context, &draft.bundles)) {
            continue;
        }
        entities.push(GameEntity {
            kind: draft.kind,
            id: draft.id.clone(),
            name: name.clone(),
            context: context.map(str::to_owned),
            bundles: draft.bundles.clone(),
        });
    }
    entities
}

/// One gamedata file, about to be streamed.
struct RowFetch<'t, T: ?Sized> {
    transport: &'t T,
    file: String,
}

impl<T: Transport + ?Sized> RowFetch<'_, T> {
    /// Calls `f` with every row of the file's `data` array.
    fn each<R: DeserializeOwned>(self, f: impl FnMut(R) + Send) -> Result<String, NamesError> {
        self.parse(move |reader| {
            read_envelope(
                reader,
                Rows {
                    f,
                    rows: PhantomData,
                },
            )
        })
    }

    /// The values of `keys` in the file's `data` object of strings.
    fn strings(self, keys: &HashSet<String>) -> Result<HashMap<String, String>, NamesError> {
        let mut out = HashMap::with_capacity(keys.len());
        self.parse(|reader| {
            read_envelope(
                reader,
                WantedStrings {
                    keys,
                    out: &mut out,
                },
            )
        })?;
        Ok(out)
    }

    /// Streams the file through `parse`, Brotli-decoding a `.br` file, and
    /// returns what `parse` returned.
    fn parse<V: Send>(
        self,
        parse: impl FnOnce(&mut dyn Read) -> Result<V, serde_json::Error> + Send,
    ) -> Result<V, NamesError> {
        let url = format!("{GAMEDATA_URL}{}", self.file);
        let brotli = std::path::Path::new(&self.file)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("br"));
        let result = stream(self.transport, &url, move |reader| {
            let parsed = if brotli {
                let mut decoded = brotli_decompressor::Decompressor::new(reader, BUFFER_LEN);
                parse(&mut BufReader::with_capacity(BUFFER_LEN, &mut decoded))
            } else {
                parse(&mut BufReader::with_capacity(BUFFER_LEN, reader))
            };
            parsed.map_err(|e| e.to_string())
        });
        result.map_err(|e| match e {
            StreamError::Http(e) => NamesError::Http(e),
            StreamError::Parse(detail) => NamesError::Gamedata {
                file: self.file,
                detail,
            },
        })
    }
}

enum StreamError {
    Http(HttpError),
    Parse(String),
}

/// GETs `url` on this thread while `parse` reads the body on another, joined
/// by a bounded pipe. A parse error (or panic) wins over the write error it
/// causes on the download side.
fn stream<T: Transport + ?Sized, V: Send>(
    transport: &T,
    url: &str,
    parse: impl FnOnce(&mut dyn Read) -> Result<V, String> + Send,
) -> Result<V, StreamError> {
    let (tx, rx) = mpsc::sync_channel(PIPE_CHUNKS);
    thread::scope(|scope| {
        let parser = scope.spawn(move || {
            let mut reader = PipeReader {
                rx,
                chunk: Vec::new(),
                at: 0,
            };
            parse(&mut reader)
        });
        let mut writer = PipeWriter { tx };
        let fetched = transport.get_to_writer(url, &mut writer);
        drop(writer);
        let parsed = parser
            .join()
            .unwrap_or_else(|panic| Err(panic_message(&*panic)));
        match (fetched, parsed) {
            (Ok(_), Ok(value)) => Ok(value),
            (Err(HttpError::Sink { .. }) | Ok(_), Err(detail)) => Err(StreamError::Parse(detail)),
            (Err(e), _) => Err(StreamError::Http(e)),
        }
    })
}

struct PipeWriter {
    tx: SyncSender<Vec<u8>>,
}

impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !buf.is_empty() {
            self.tx
                .send(buf.to_vec())
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the parser stopped"))?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct PipeReader {
    rx: Receiver<Vec<u8>>,
    chunk: Vec<u8>,
    at: usize,
}

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.at == self.chunk.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.chunk = chunk;
                    self.at = 0;
                }
                // The download ended (or failed): end of input.
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.chunk.len() - self.at);
        buf[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Reads `{"version": "...", "data": <seed>}` (keys in any order, others
/// skipped) to the end, and returns the version.
fn read_envelope<S>(reader: &mut dyn Read, seed: S) -> Result<String, serde_json::Error>
where
    S: for<'de> DeserializeSeed<'de, Value = ()>,
{
    let mut de = serde_json::Deserializer::from_reader(reader);
    let version = de::Deserializer::deserialize_map(&mut de, Envelope(Some(seed)))?;
    de.end()?;
    Ok(version)
}

struct Envelope<S>(Option<S>);

impl<'de, S: DeserializeSeed<'de, Value = ()>> Visitor<'de> for Envelope<S> {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object with version and data")
    }

    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<String, A::Error> {
        let mut version = String::new();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "version" => version = map.next_value()?,
                "data" => {
                    let seed = self
                        .0
                        .take()
                        .ok_or_else(|| de::Error::duplicate_field("data"))?;
                    map.next_value_seed(seed)?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        if self.0.is_some() {
            return Err(de::Error::missing_field("data"));
        }
        Ok(version)
    }
}

/// Feeds each element of an array to `f`.
struct Rows<R, F> {
    f: F,
    rows: PhantomData<fn() -> R>,
}

impl<'de, R: Deserialize<'de>, F: FnMut(R)> DeserializeSeed<'de> for Rows<R, F> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, R: Deserialize<'de>, F: FnMut(R)> Visitor<'de> for Rows<R, F> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of rows")
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<(), A::Error> {
        while let Some(row) = seq.next_element::<R>()? {
            (self.f)(row);
        }
        Ok(())
    }
}

/// Keeps the string values of `keys` from an object, skipping the rest.
struct WantedStrings<'a> {
    keys: &'a HashSet<String>,
    out: &'a mut HashMap<String, String>,
}

impl<'de> DeserializeSeed<'de> for WantedStrings<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for WantedStrings<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object of strings")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if self.keys.contains(&key) {
                self.out.insert(key, map.next_value()?);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}
