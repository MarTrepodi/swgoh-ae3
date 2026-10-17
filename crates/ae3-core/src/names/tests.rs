use std::fs;

use super::gamedata::{bundle_name, clean_display_name};
use super::*;
use crate::http::UreqTransport;
use crate::index::{AssetIndex, IndexFile};
use crate::manifest::{Manifest, Package, PackageKind};
use crate::platform::Platform;
use crate::test_support::{FakeTransport, TempDir, transient};
use crate::version::AssetVersion;

/// `data` as a Brotli stream of uncompressed meta-blocks (RFC 7932 §9.2),
/// which any decoder must accept; enough to test the `.br` path without
/// an encoder.
fn brotli_stored(data: &[u8]) -> Vec<u8> {
    struct Bits {
        out: Vec<u8>,
        acc: u64,
        len: u32,
    }
    impl Bits {
        fn push(&mut self, value: u64, bits: u32) {
            self.acc |= value << self.len;
            self.len += bits;
            while self.len >= 8 {
                self.out.push(self.acc.to_le_bytes()[0]);
                self.acc >>= 8;
                self.len -= 8;
            }
        }
        fn align(&mut self) {
            if self.len > 0 {
                self.out.push(self.acc.to_le_bytes()[0]);
                self.acc = 0;
                self.len = 0;
            }
        }
    }
    let mut w = Bits {
        out: Vec::new(),
        acc: 0,
        len: 0,
    };
    w.push(0, 1); // WBITS 16
    for chunk in data.chunks(1 << 16) {
        w.push(0, 1); // ISLAST
        w.push(0, 2); // MNIBBLES: 4
        w.push(chunk.len() as u64 - 1, 16); // MLEN - 1
        w.push(1, 1); // ISUNCOMPRESSED
        w.align();
        w.out.extend_from_slice(chunk);
    }
    w.push(1, 1); // ISLAST
    w.push(1, 1); // ISLASTEMPTY
    w.align();
    w.out
}

const GAME: &str = "0.40.6:abc";
const LOC: &str = "loc1";

fn url(file: &str) -> String {
    format!("{GAMEDATA_URL}{file}")
}

fn versions_json(game: &str, loc: &str) -> String {
    format!(r#"{{"meta.json":"{game}","gameVersion":"{game}","localeVersion":"{loc}"}}"#)
}

/// A small but faithful copy of the mirror: the real field names and
/// nesting, with the fields the index ignores present to be skipped.
fn gamedata() -> FakeTransport {
    let skills = r#"{"version":"0.40.6:abc","data":[
        {"tier":[{"x":1}],"id":"specialskill_BOSSK01","nameKey":"DEFENSE_UP_NAME_KEY",
         "iconKey":"tex.consumable_bacta_salvaged","abilityReference":"specialability_bossk01"},
        {"id":"basicskill_BOSSK","nameKey":"JUNK","abilityReference":"basicability_bossk"},
        {"id":"basicskill_HOUNDSTOOTH","abilityReference":"basicability_houndstooth"}]}"#;
    let unit = |id: &str| {
        format!(
            r#"{{"categoryId":["alignment_dark"],"skillReference":[{{"skillId":"specialskill_BOSSK01","requiredTier":1}},
            {{"skillId":"basicskill_BOSSK"}}],"unitTier":[{{"equipmentSet":["001"],"baseStat":{{"stat":[{{"unitStatId":2}}]}}}}],
            "id":"{id}","unitPrefab":"unit.char_bossk_pre","thumbnailName":"tex.charui_bossk",
            "nameKey":"UNIT_BOSSK_NAME","baseId":"BOSSK","combatType":1,"basicAttack":null,
            "relicDefinition":{{"relicTierDefinitionId":["A"],"texture":"tex.relic_rfl_bossk","nameKey":"X"}},
            "battlePortraitPrefab":"","eraThumbnailName":"","battlePortraitTexture":"","thumbImageTurnOrder":""}}"#
        )
    };
    let units = format!(
        r#"{{"data":[{},{},{{"id":"HOUNDSTOOTH:SEVEN_STAR","baseId":"HOUNDSTOOTH","nameKey":"UNIT_HOUNDSTOOTH_NAME",
        "unitPrefab":"unit.ship_houndstooth_pre","thumbnailName":"tex.charui_houndstooth","relicDefinition":null,
        "battlePortraitPrefab":"prefab.crewportrait_bossk_pre","skillReference":[{{"skillId":"basicskill_HOUNDSTOOTH"}}]}},
        {{"id":"NONAME:ONE_STAR","baseId":"NONAME","nameKey":"UNIT_NONAME_NAME","thumbnailName":"tex.charui_noname"}}],
        "version":"0.40.6:abc"}}"#,
        unit("BOSSK:ONE_STAR"),
        unit("BOSSK:SEVEN_STAR"),
    );
    let abilities = r#"{"version":"0.40.6:abc","data":[
        {"triggerCondition":[],"effectReference":[{"id":"x","contextIndex":[]}],"id":"summon_general_ships",
         "nameKey":"SUMMONUNIT_NAME","icon":"tex.abilityui_summon","prefabName":"prefab.placeholder"},
        {"id":"specialability_bossk01","nameKey":"SPECIALABILITY_BOSSK01_NAME","icon":"tex.ability_bossk_special01",
         "prefabName":"prefab.ability_char_netgun_atk","aiParams":{"deep":{"deeper":[1,2,3]}}},
        {"id":"basicability_bossk","nameKey":"BASICABILITY_BOSSK_NAME","icon":"tex.ability_bossk_basic",
         "prefabName":"prefab.placeholder"},
        {"id":"basicability_houndstooth","nameKey":"BASICABILITY_BOSSK_NAME","icon":"tex.ability_houndstooth_basic",
         "prefabName":""}]}"#;
    let equipment = r#"{"version":"0.40.6:abc","data":[
        {"lookupMission":[{"missionIdentifier":{"campaignId":"C01D"}}],"id":"001","nameKey":"EQUIPMENT_001_NAME",
         "iconKey":"tex.equip_weaponmod","tier":1}]}"#;
    let materials = r#"{"version":"0.40.6:abc","data":[
        {"id":"unitshard_BOSSK","nameKey":"UNIT_BOSSK_NAME","iconKey":"tex.charui_bossk","unitDefReference":"BOSSK:TWO_STAR"},
        {"id":"datacron_set_1_upgrade_1","nameKey":"ACUMEN_1_NAME","iconKey":"tex.icon_datacrons_upgrade_a_1","unitDefReference":""},
        {"id":"datacron_set_2_upgrade_1","nameKey":"ACUMEN_1_NAME","iconKey":"tex.icon_datacrons_upgrade_a_1"}]}"#;
    let portraits = r#"{"version":"0.40.6:abc","data":[
        {"id":"PLAYERPORTRAIT_BOSSK","nameKey":"PLAYERPORTRAIT_BOSSK_NAME","obtainable":true,"icon":"tex.vanity_bossk"}]}"#;
    let loc = r#"{"version":"loc1","data":{
        "000_UNRELATED":"Not wanted\\nat all",
        "UNIT_BOSSK_NAME":"Bossk",
        "UNIT_HOUNDSTOOTH_NAME":"Hound's Tooth",
        "SPECIALABILITY_BOSSK01_NAME":"[c][ffff33]Hunting  Party[-][/c]",
        "BASICABILITY_BOSSK_NAME":"Flamethrower",
        "EQUIPMENT_001_NAME":"Mk 1 BlasTech Weapon Mod",
        "ACUMEN_1_NAME":"Acumen Mk1",
        "PLAYERPORTRAIT_BOSSK_NAME":"Bossk",
        "SUMMONUNIT_NAME":"Summon"}}"#;
    FakeTransport::default()
        .with_file(&url(VERSIONS_FILE), versions_json(GAME, LOC).as_bytes())
        .with_file(&url("skill.json"), skills.as_bytes())
        .with_file(&url("units.json.br"), &brotli_stored(units.as_bytes()))
        .with_file(&url("ability.json"), abilities.as_bytes())
        .with_file(&url("equipment.json"), equipment.as_bytes())
        .with_file(&url("material.json"), materials.as_bytes())
        .with_file(&url("playerPortrait.json"), portraits.as_bytes())
        .with_file(
            &url("Loc_ENG_US.txt.json.br"),
            &brotli_stored(loc.as_bytes()),
        )
}

fn versions() -> GamedataVersions {
    GamedataVersions {
        gamedata: GAME.into(),
        localization: LOC.into(),
    }
}

fn build(transport: &FakeTransport) -> NameIndex {
    NameIndex::build(transport, Language::EngUs, versions(), &mut |_| {}).unwrap()
}

fn entity<'a>(index: &'a NameIndex, kind: EntityKind, id: &str) -> &'a GameEntity {
    index
        .entities()
        .iter()
        .find(|e| e.kind == kind && e.id == id)
        .unwrap_or_else(|| panic!("no {kind} {id} in {:#?}", index.entities()))
}

#[test]
fn stored_brotli_round_trips() {
    use std::io::Read;
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    for data in [&data[..], b"", b"x"] {
        let encoded = brotli_stored(data);
        let mut decoded = Vec::new();
        brotli_decompressor::Decompressor::new(&encoded[..], 4096)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, data);
    }
}

#[test]
fn references_name_bundles() {
    assert_eq!(
        bundle_name("tex.charui_bossk").as_deref(),
        Some("charui_bossk")
    );
    assert_eq!(
        bundle_name("unit.char_CX-2_pre").as_deref(),
        Some("char_cx-2_pre")
    );
    assert_eq!(
        bundle_name("prefab.crewportrait_bossk_pre").as_deref(),
        Some("crewportrait_bossk_pre")
    );
    assert_eq!(bundle_name("ui.foo").as_deref(), Some("foo"));
    assert_eq!(bundle_name("charui_x").as_deref(), Some("charui_x"));
    for none in ["", "  ", "tex.", "prefab.placeholder", "tex.a/b", "tex.a b"] {
        assert_eq!(bundle_name(none), None, "{none:?}");
    }
}

#[test]
fn display_names_lose_markup() {
    assert_eq!(
        clean_display_name("[c][ffff33]Rebel  Trooper[-][/c]"),
        "Rebel Trooper"
    );
    assert_eq!(clean_display_name("[b]Bold[/b] move"), "Bold move");
    assert_eq!(clean_display_name("Squad [Leader]"), "Squad [Leader]");
    assert_eq!(clean_display_name("Open [ bracket"), "Open [ bracket");
    assert_eq!(clean_display_name("  "), "");
}

#[test]
fn folding_drops_case_and_punctuation() {
    assert_eq!(fold_name("Qi'ra"), "qira");
    assert_eq!(fold_name("  IG-88  "), "ig88");
    assert_eq!(fold_name("Darth   Vader"), "darth vader");
    assert_eq!(fold_name("Hound's Tooth"), "hounds tooth");
    assert_eq!(fold_name("БОССК"), "босск");
    assert_eq!(fold_name("!!"), "");
}

#[test]
fn builds_entities_from_streamed_gamedata() {
    let transport = gamedata();
    let mut events = Vec::new();
    let index = NameIndex::build(&transport, Language::EngUs, versions(), &mut |e| {
        events.push(match e {
            NamesEvent::Building { versions } => format!("build {}", versions.gamedata),
            NamesEvent::Downloading { file } => file.to_owned(),
        });
    })
    .unwrap();
    assert_eq!(
        events,
        [
            "build 0.40.6:abc",
            "skill.json",
            "units.json.br",
            "ability.json",
            "equipment.json",
            "material.json",
            "playerPortrait.json",
            "Loc_ENG_US.txt.json.br"
        ]
    );
    assert_eq!(index.versions(), &versions());
    assert_eq!(index.language(), Language::EngUs);

    // One Unit per base id, however many star levels units.json lists.
    let bossk = entity(&index, EntityKind::Unit, "BOSSK");
    assert_eq!(bossk.name, "Bossk");
    assert_eq!(
        bossk.bundles,
        ["charui_bossk", "char_bossk_pre", "relic_rfl_bossk"]
    );
    let ship = entity(&index, EntityKind::Unit, "HOUNDSTOOTH");
    assert_eq!(ship.name, "Hound's Tooth");
    assert_eq!(
        ship.bundles,
        [
            "charui_houndstooth",
            "ship_houndstooth_pre",
            "crewportrait_bossk_pre"
        ]
    );

    // Abilities come through Skills, named by ability.nameKey, with their
    // Unit as context; unreferenced ones are left out.
    let hunting = entity(&index, EntityKind::Ability, "specialability_bossk01");
    assert_eq!(hunting.name, "Hunting Party");
    assert_eq!(hunting.context.as_deref(), Some("Bossk"));
    assert_eq!(
        hunting.bundles,
        ["ability_bossk_special01", "ability_char_netgun_atk"]
    );
    assert_eq!(
        entity(&index, EntityKind::Ability, "basicability_bossk").bundles,
        ["ability_bossk_basic"]
    );
    assert_eq!(
        entity(&index, EntityKind::Ability, "basicability_houndstooth")
            .context
            .as_deref(),
        Some("Hound's Tooth")
    );
    assert!(
        index
            .entities()
            .iter()
            .all(|e| e.id != "summon_general_ships")
    );

    assert_eq!(
        entity(&index, EntityKind::Gear, "001").bundles,
        ["equip_weaponmod"]
    );
    // Shards are the Unit's; repeated Materials collapse to one.
    let materials: Vec<_> = index
        .entities()
        .iter()
        .filter(|e| e.kind == EntityKind::Material)
        .collect();
    assert_eq!(materials.len(), 1, "{materials:#?}");
    assert_eq!(materials[0].name, "Acumen Mk1");
    assert_eq!(
        entity(&index, EntityKind::PlayerPortrait, "PLAYERPORTRAIT_BOSSK").bundles,
        ["vanity_bossk"]
    );
    // No display name, no entity.
    assert!(index.entities().iter().all(|e| e.id != "NONAME"));
    assert_eq!(index.len(), 8);
}

#[test]
fn a_broken_file_names_itself() {
    let transport = gamedata().with_file(&url("ability.json"), br#"{"data":[{"id":"#);
    let err = NameIndex::build(&transport, Language::EngUs, versions(), &mut |_| {}).unwrap_err();
    assert!(
        matches!(&err, NamesError::Gamedata { file, .. } if file == "ability.json"),
        "{err}"
    );
    let transport = gamedata().with_file(&url("units.json.br"), b"not brotli at all");
    let err = NameIndex::build(&transport, Language::EngUs, versions(), &mut |_| {}).unwrap_err();
    assert!(
        matches!(&err, NamesError::Gamedata { file, .. } if file == "units.json.br"),
        "{err}"
    );
    let transport = gamedata();
    transport.fail_next(&url("material.json"), transient("material"));
    let err = NameIndex::build(&transport, Language::EngUs, versions(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, NamesError::Http(_)), "{err}");
}

#[test]
fn search_ranks_exact_then_prefix_then_words_then_substrings() {
    use MatchRank::{Exact, Prefix, Substring, WordPrefix};
    let e = |kind, id: &str, name: &str| GameEntity {
        kind,
        id: id.into(),
        name: name.into(),
        context: None,
        bundles: Vec::new(),
    };
    let index = NameIndex::new(
        Language::EngUs,
        versions(),
        vec![
            e(EntityKind::Material, "m", "Invader Kit"),
            e(EntityKind::Unit, "VADERSHIP", "Vader's TIE Advanced"),
            e(EntityKind::Unit, "VADER", "Darth Vader"),
            e(EntityKind::PlayerPortrait, "P", "Vader"),
            e(EntityKind::Unit, "LORDVADER", "Lord Vader"),
        ],
    );
    let ids = |q: &str| -> Vec<(String, MatchRank)> {
        index
            .search(q)
            .iter()
            .map(|m| (m.entity.id.clone(), m.rank))
            .collect()
    };
    assert_eq!(
        ids("VADER"),
        [
            ("P".to_owned(), Exact),
            ("VADERSHIP".to_owned(), Prefix),
            ("LORDVADER".to_owned(), WordPrefix),
            ("VADER".to_owned(), WordPrefix),
            ("m".to_owned(), Substring),
        ]
    );
    assert_eq!(ids("vaders tie")[0], ("VADERSHIP".to_owned(), Prefix));
    assert_eq!(ids("  "), []);
    assert_eq!(ids("yoda"), []);
}

fn manifest(names: &[&str]) -> Manifest {
    Manifest {
        asset_version: AssetVersion(100_054),
        packages: names
            .iter()
            .map(|&name| Package {
                name: name.into(),
                content_hash: 1,
                size: 1,
                kind: PackageKind::Bundle,
                ..Package::default()
            })
            .collect(),
        platform: "Windows".into(),
        ..Manifest::default()
    }
}

#[test]
fn search_with_names_puts_entities_above_bundles() {
    let names = build(&gamedata());
    let manifest = manifest(&[
        "charui_bossk",
        "char_bossk_pre",
        "vanity_bossk",
        "ability_bossk_special01",
        "bossk_extra",
    ]);
    let platform = Platform::Windows;
    let index = AssetIndex::new(platform, &manifest, &IndexFile::new(platform));

    let results = index.search_with_names("bossk", &names);
    let found: Vec<_> = results
        .entities
        .iter()
        .map(|h| (h.entity.kind, h.entity.name.as_str(), h.rank))
        .collect();
    assert_eq!(
        found,
        [
            (EntityKind::Unit, "Bossk", MatchRank::Exact),
            (EntityKind::PlayerPortrait, "Bossk", MatchRank::Exact),
        ]
    );
    // Only the linked Bundles this Manifest has.
    let unit: Vec<_> = results.entities[0]
        .bundles
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(unit, ["charui_bossk", "char_bossk_pre"]);
    assert_eq!(results.len(), 5, "the Bundle name matches stay");
    assert!(!results.nothing_matched());

    let json = serde_json::to_value(&results).unwrap();
    assert_eq!(
        json["entities"][0],
        serde_json::json!({
            "kind": "unit", "id": "BOSSK", "name": "Bossk", "rank": "exact",
            "bundles": ["charui_bossk", "char_bossk_pre"]
        })
    );

    let hunting = index.search_with_names("hunting", &names);
    assert_eq!(hunting.entities.len(), 1);
    assert_eq!(hunting.entities[0].rank, MatchRank::Prefix);
    assert_eq!(
        hunting.entities[0].bundles[0].name,
        "ability_bossk_special01"
    );
    assert!(hunting.hits.is_empty());
    let json = serde_json::to_value(&hunting).unwrap();
    assert_eq!(json["entities"][0]["context"], "Bossk");

    // A plain search leaves entities empty.
    assert!(index.search("bossk").entities.is_empty());
    assert!(index.search_with_names("zzz", &names).nothing_matched());
}

#[test]
fn bundle_links_ignore_case() {
    let names = NameIndex::new(
        Language::EngUs,
        versions(),
        vec![GameEntity {
            kind: EntityKind::Unit,
            id: "CX2".into(),
            name: "CX-2".into(),
            context: None,
            bundles: vec!["char_cx-2_pre".into()],
        }],
    );
    let manifest = manifest(&["char_CX-2_pre"]);
    let index = AssetIndex::new(
        Platform::Windows,
        &manifest,
        &IndexFile::new(Platform::Windows),
    );
    let results = index.search_with_names("cx2", &names);
    assert_eq!(results.entities[0].bundles[0].name, "char_CX-2_pre");
}

#[test]
fn saved_index_round_trips() {
    let dir = TempDir::new("names");
    let path = default_name_index_path(dir.path(), Language::EngUs);
    assert!(path.ends_with("names/ENG_US.json"));
    assert!(NameIndex::load(&path).unwrap().is_none());
    let index = build(&gamedata());
    index.save(&path).unwrap();
    let loaded = NameIndex::load(&path).unwrap().unwrap();
    assert_eq!(loaded.entities(), index.entities());
    assert_eq!(loaded.versions(), index.versions());
    assert_eq!(loaded.search("bossk").len(), 2);

    fs::write(&path, b"{\"format\":99}").unwrap();
    assert!(matches!(
        NameIndex::load(&path),
        Err(NamesError::Corrupt { .. })
    ));
}

#[test]
fn open_builds_once_per_gamedata_version_and_falls_back_to_the_cache() {
    let dir = TempDir::new("names-open");
    let transport = gamedata();
    let open =
        |update| NameIndex::open(&transport, dir.path(), Language::EngUs, update, &mut |_| {});

    // Nothing cached and not allowed to check: unavailable, no requests.
    let load = open(false);
    assert_eq!(load.status, NameIndexStatus::Unavailable);
    assert!(load.index.is_none());
    assert_eq!(transport.total_requests(), 0);

    let load = open(true);
    assert_eq!(load.status, NameIndexStatus::Built, "{:?}", load.problem);
    assert!(load.problem.is_none());
    assert!(load.path.is_file());
    assert_eq!(transport.requests_for(&url("ability.json")), 1);

    // Same versions: only the version file is fetched again.
    let before = transport.total_requests();
    let load = open(true);
    assert_eq!(load.status, NameIndexStatus::Current);
    assert_eq!(load.index.unwrap().len(), 8);
    assert_eq!(transport.total_requests(), before + 1);

    let load = open(false);
    assert_eq!(load.status, NameIndexStatus::Unchecked);
    assert_eq!(transport.total_requests(), before + 1);

    // GitHub down: the cached index, with the reason.
    transport.fail_next(&url(VERSIONS_FILE), transient("down"));
    let load = open(true);
    assert_eq!(load.status, NameIndexStatus::Stale);
    assert!(load.problem.is_some());
    assert_eq!(load.index.unwrap().len(), 8);

    // A new localization version rebuilds; a failed rebuild keeps the old.
    transport.put(&url(VERSIONS_FILE), versions_json(GAME, "loc2").as_bytes());
    transport.fail_next(&url("skill.json"), transient("skill"));
    let load = open(true);
    assert_eq!(load.status, NameIndexStatus::Stale);
    assert_eq!(load.index.unwrap().versions().localization, LOC);
    let load = open(true);
    assert_eq!(load.status, NameIndexStatus::Built);
    assert_eq!(load.index.unwrap().versions().localization, "loc2");
    assert_eq!(transport.requests_for(&url("ability.json")), 2);

    // Another language has its own file.
    let other = NameIndex::open(&transport, dir.path(), Language::FreFr, false, &mut |_| {});
    assert_eq!(other.status, NameIndexStatus::Unavailable);
}

#[test]
fn open_without_any_index_reports_why() {
    let dir = TempDir::new("names-down");
    let transport = FakeTransport::default();
    let load = NameIndex::open(&transport, dir.path(), Language::EngUs, true, &mut |_| {});
    assert_eq!(load.status, NameIndexStatus::Unavailable);
    assert!(load.problem.unwrap().contains(VERSIONS_FILE));
}

/// Builds the real Name Index from GitHub and finds Bossk's Bundles.
#[test]
#[ignore = "needs network access"]
fn live_name_index_finds_bossk() {
    let dir = TempDir::new("names-live");
    let transport = UreqTransport::new();
    let load = NameIndex::open(&transport, dir.path(), Language::EngUs, true, &mut |_| {});
    assert_eq!(load.status, NameIndexStatus::Built, "{:?}", load.problem);
    let index = load.index.unwrap();
    let size = fs::metadata(&load.path).unwrap().len();
    eprintln!(
        "{} entities, {size} bytes, gamedata {}",
        index.len(),
        index.versions().gamedata
    );
    assert!(size < 1_500_000, "{size} bytes");
    for kind in EntityKind::ALL {
        assert!(index.entities().iter().any(|e| e.kind == kind), "no {kind}");
    }
    let matches = index.search("Bossk");
    let first = matches[0].entity;
    assert_eq!(
        (first.kind, first.name.as_str()),
        (EntityKind::Unit, "Bossk")
    );
    for bundle in ["charui_bossk", "char_bossk_pre"] {
        assert!(first.bundles.iter().any(|b| b == bundle), "{first:?}");
    }
    let hunting = index.search("Hunting Party");
    assert!(
        hunting
            .iter()
            .any(|m| m.entity.context.as_deref() == Some("Bossk")),
        "{hunting:?}"
    );
}
