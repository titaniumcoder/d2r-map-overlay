//! The playstyle panels' tests, through the whole dashboard JSON where they check its shape.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow};

use super::config_page::{builds_json, config_json, remove_character, set_bis_selection};
use super::farm::{FarmStatus, farm_status};
use super::gear::{gear_json, grid_pieces, score_profile, upgrades_json};
use super::recipes::{Input, runeword_slots, socket_count};
use super::*;
use crate::config;
use d2r_core::build::Damage;
use d2r_core::web::fixtures::{EMPTY_MARKS, EMPTY_SKILLS, lines_of, stash_item, test_stash};
use d2r_core::web::{GearInfo, LiveInfo};

static NO_ROUTE: crate::route::Route = crate::route::Route { bosses: Vec::new() };

static NO_MARKS: crate::not_needed::Marks = BTreeMap::new();

fn live_info<'a>(
    class: u32,
    blocks: &'a d2r_core::render::TextBlocks,
    equipped: &'a [(u8, u32)],
    stash: &'a [d2r_core::stash::StashTab],
    stash_owned: &'a [String],
) -> PanelInfo<'a> {
    PanelInfo {
        live: d2r_core::web::fixtures::live_info(class, blocks, equipped, stash, stash_owned),
        bis: None,
        build: None,
        stage: None,
        farm: &[],
        route: &NO_ROUTE,
        stage_chain: &[],
        not_needed: &NO_MARKS,
    }
}

fn live_json(info: &PanelInfo) -> String {
    d2r_core::web::live_json(&info.live, panels(info))
}

fn shop_json(info: &PanelInfo) -> serde_json::Value {
    d2r_core::web::vendor::shop_json(&info.live, &gear::shop_notes(info))
}

fn settings(text: &str) -> Result<(crate::config::Settings, Vec<String>), toml::de::Error> {
    crate::config::parse_tables(text).map(|(tables, warnings)| (tables.into_settings(), warnings))
}

#[test]
fn set_bis_selection_preserves_comments_and_round_trips() -> Result<()> {
    let text = "# top\n[bis.Hork]\n# build line\nbuild = \"Blood Boil\"\n";
    let out = set_bis_selection(text, "Hork", Some("blood boil")).ok_or_else(|| anyhow!("no edit"))?;
    assert!(out.contains("# top") && out.contains("# build line"), "{out}");
    let (config, _) = settings(&out)?;
    assert_eq!(
        config.bis.get("hork").map(|b| b.build.as_str()),
        Some("blood boil")
    );
    Ok(())
}

#[test]
fn set_bis_selection_creates_missing_tables_and_names_with_spaces() -> Result<()> {
    let out = set_bis_selection("", "Demon Master", Some("blood boil")).ok_or_else(|| anyhow!("no edit"))?;
    let (config, warnings) = settings(&out)?;
    assert_eq!(
        config.bis.get("demon master").map(|b| b.build.as_str()),
        Some("blood boil")
    );
    assert_eq!(warnings, Vec::<String>::new());
    Ok(())
}

#[test]
fn clearing_a_selection_removes_the_build_and_empty_entries() -> Result<()> {
    // A selection-only entry vanishes entirely; an inline one only loses its build.
    let out =
        set_bis_selection("[bis.Hork]\nbuild = \"b\"\n", "Hork", None).ok_or_else(|| anyhow!("no edit"))?;
    assert_eq!(out, "", "the emptied entry is removed: {out}");
    let inline = "[bis.Hork]\nbuild = \"b\"\n[bis.Hork.slots]\nweapon = [\"Insight@27\"]\n";
    let out = set_bis_selection(inline, "Hork", None).ok_or_else(|| anyhow!("no edit"))?;
    let (config, _) = settings(&out)?;
    let bis = config.bis.get("hork").ok_or_else(|| anyhow!("gone"))?;
    assert_eq!(bis.build, "");
    assert_eq!(
        bis.slots.get("weapon").map(Vec::as_slice),
        Some(&["Insight@27".to_owned()][..])
    );
    Ok(())
}

#[test]
fn set_bis_selection_rejects_non_table_bis() {
    // A bare value and a non-table entry can't be saved.
    assert_eq!(set_bis_selection("bis = \"oops\"\n", "Hork", Some("b")), None);
    assert_eq!(set_bis_selection("[bis]\nHork = 5\n", "Hork", Some("b")), None);
}

/// One build definition, a selection-only character and a builds-only character.
fn resolved_config() -> crate::config::Settings {
    let mut config = crate::config::Settings::default();
    config.build_defs.insert(
        "blood boil".to_owned(),
        crate::config::Build {
            name: "Blood Boil".to_owned(),
            damages: vec![Damage::Physical, Damage::Fire],
            notes: Some("guide".to_owned()),
            slots: [("weapon".to_owned(), vec!["Heart of the Oak@55".to_owned()])].into(),
            merc_slots: [("weapon".to_owned(), vec!["Insight@27".to_owned()])].into(),
            ..crate::config::Build::default()
        },
    );
    config.bis.insert(
        "demonmaster".to_owned(),
        crate::config::Bis {
            build: "blood boil".to_owned(),
            slots: BTreeMap::new(),
            merc_slots: BTreeMap::new(),
        },
    );
    config
}

fn played_entry(name: &str, t: u64, class: Option<u32>) -> (String, d2r_core::played::Entry) {
    (
        name.to_lowercase(),
        d2r_core::played::Entry {
            name: name.to_owned(),
            t,
            class,
            stage: None,
        },
    )
}

fn names_of(json: &serde_json::Value) -> Vec<String> {
    json.get("characters")
        .and_then(|v| v.as_array())
        .map_or_else(Vec::new, |cs| {
            cs.iter()
                .filter_map(|c| Some(c.get("name")?.as_str()?.to_owned()))
                .collect()
        })
}

#[test]
fn config_json_lists_every_character_once_recently_played_first() {
    let mut config = crate::config::Settings::default();
    for name in ["demonmaster", "elemental", "fulmina", "vex"] {
        let bis = crate::config::Bis {
            build: String::new(),
            slots: BTreeMap::new(),
            merc_slots: BTreeMap::new(),
        };
        config.bis.insert(name.to_owned(), bis);
    }
    // Builds-only, and one in both tables (listed once).
    config.builds.names.insert("hork".to_owned(), vec![Damage::Fire]);
    config.builds.names.insert("vex".to_owned(), vec![Damage::Cold]);
    let played = d2r_core::played::Played::from([
        played_entry("Fulmina", 30, None),
        played_entry("Elemental", 20, None),
        played_entry("Other", 99, None),
    ]);
    let json = config_json(&config, &GearInfo::default(), &played);
    assert_eq!(
        names_of(&json),
        ["Other", "Fulmina", "Elemental", "demonmaster", "vex", "hork"],
        "recent first with played display names, the never-seen in config order"
    );
    assert_eq!(
        json.pointer("/characters/0/last_played"),
        Some(&serde_json::json!(99))
    );
    assert_eq!(
        json.pointer("/characters/0/build_name"),
        Some(&serde_json::Value::Null),
        "played-only: no build selected"
    );
    assert_eq!(
        json.pointer("/characters/3/last_played"),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(
        json.pointer("/characters/4/damages"),
        Some(&serde_json::json!(["cold"]))
    );
}

#[test]
fn config_json_lists_every_not_needed_mark_by_build_the_stale_ones_too() {
    let mut config = resolved_config();
    let build = |name: &str, next: Option<&str>| crate::config::Build {
        name: name.to_owned(),
        slots: [("weapon".to_owned(), vec!["Grief".to_owned()])].into(),
        next: next.map(str::to_owned),
        ..crate::config::Build::default()
    };
    config.build_defs.insert(
        "summoner druid".to_owned(),
        build("Summoner Druid", Some("fury druid")),
    );
    config
        .build_defs
        .insert("fury druid".to_owned(), build("Fury Druid", None));
    config.not_needed = mark(
        &[
            ("summoner druid", &["Grief", "No Such Piece"]),
            ("old build", &["Gone"]),
            ("demonmaster", &["Shako"]),
        ],
        &mut config.build_defs,
        &mut [],
    );
    assert!(config.not_needed.contains_key("fury druid"), "inherited there");
    let json = config_json(&config, &GearInfo::default(), &d2r_core::played::Played::new());
    assert_eq!(
        json.get("not_needed"),
        Some(&serde_json::json!([
            { "build": "demonmaster", "known": true, "names": ["Shako"] },
            { "build": "old build", "known": false, "names": ["Gone"] },
            { "build": "Summoner Druid", "known": true, "names": ["Grief", "No Such Piece"] },
        ])),
        "own marks only, a character key and an unknown build included"
    );
    config.not_needed.clear();
    let json = config_json(&config, &GearInfo::default(), &d2r_core::played::Played::new());
    assert_eq!(json.get("not_needed"), Some(&serde_json::json!([])));
}

#[test]
fn config_json_shows_the_class_default_of_a_played_class() {
    let mut config = resolved_config();
    if let Some(def) = config.build_defs.get_mut("blood boil") {
        def.class = Some(7);
        def.default_for_class = true;
    }
    let played = d2r_core::played::Played::from([
        played_entry("Warly", 10, Some(7)),
        played_entry("Amy", 5, Some(0)),
        played_entry("Demonmaster", 1, Some(7)),
    ]);
    let json = config_json(&config, &GearInfo::default(), &played);
    assert_eq!(names_of(&json), ["Warly", "Amy", "Demonmaster"]);
    let field = |i: usize, key: &str| {
        json.pointer(&format!("/characters/{i}/{key}"))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(field(0, "class"), serde_json::json!("warlock"));
    assert_eq!(field(0, "build"), serde_json::json!(""));
    assert_eq!(field(0, "build_name"), serde_json::json!("Blood Boil"));
    assert_eq!(field(0, "class_default"), serde_json::json!(true));
    assert_eq!(
        json.pointer("/characters/0/slots/weapon/0/name"),
        Some(&serde_json::json!("Heart of the Oak"))
    );
    assert_eq!(field(1, "class"), serde_json::json!("amazon"));
    assert_eq!(field(1, "build_name"), serde_json::Value::Null);
    assert_eq!(field(2, "build"), serde_json::json!("blood boil"));
    assert_eq!(field(2, "class_default"), serde_json::json!(false), "a selection");
}

#[test]
fn remove_character_drops_every_spelling_and_keeps_the_rest() -> Result<()> {
    let text = "# top comment\n\
        [builds]\nFulmina = [\"lightning\"]\n# damages\nvex = [\"cold\"] # vex trailing\n\
        [builds.class]\nfulmina = [\"fire\"]\nsorceress = [\"cold\"]\n\n\
        # demon comment\n[bis.Demonmaster]\nbuild = \"blood boil\"\n\n\
        [bis.Fulmina]\nbuild = \"a\"\n\n[bis.fulmina]\nbuild = \"b\"\n\
        [bis.fulmina.slots]\nweapon = [\"Insight@27\"]\n\n\
        # vex comment\n[bis.Vex]\nbuild = \"c\"\n";
    let out = remove_character(text, "FULMINA").ok_or_else(|| anyhow!("no edit"))?;
    for kept in [
        "# top comment",
        "# damages",
        "# vex trailing",
        "# demon comment",
        "# vex comment",
    ] {
        assert!(out.contains(kept), "{kept} kept: {out}");
    }
    assert!(!out.to_lowercase().contains("bis.fulmina"), "{out}");
    assert!(!out.contains("Insight"), "{out}");
    let (config, _) = settings(&out)?;
    assert_eq!(config.bis.keys().collect::<Vec<_>>(), ["demonmaster", "vex"]);
    assert_eq!(config.builds.names.keys().collect::<Vec<_>>(), ["vex"]);
    assert_eq!(config.builds.classes.len(), 1, "the class table is untouched");
    assert!(
        out.contains("fulmina = [\"fire\"]"),
        "not a character entry: {out}"
    );
    // Nothing to remove: unchanged; invalid TOML: an error.
    assert_eq!(remove_character(text, "nobody").as_deref(), Some(text));
    assert_eq!(remove_character("[bis", "x"), None);
    Ok(())
}

#[test]
fn set_bis_selection_writes_every_spelling() -> Result<()> {
    let text = "[bis.Fulmina]\nbuild = \"a\"\n[bis.fulmina]\nbuild = \"b\"\n";
    let out = set_bis_selection(text, "FULMINA", Some("c")).ok_or_else(|| anyhow!("no edit"))?;
    assert_eq!(out.matches("build = \"c\"").count(), 2, "{out}");
    let out = set_bis_selection(&out, "Fulmina", None).ok_or_else(|| anyhow!("no edit"))?;
    assert_eq!(out, "", "{out}");
    // Clearing a character that has no entry is no change, not an error.
    assert_eq!(
        set_bis_selection("[bis.Vex]\nbuild = \"x\"\n", "Other", None).as_deref(),
        Some("[bis.Vex]\nbuild = \"x\"\n")
    );
    Ok(())
}

#[test]
fn config_json_resolves_the_selected_build() {
    // The selection-only character gets the build's name, damages and pieces.
    let json = config_json(
        &resolved_config(),
        &GearInfo::default(),
        &d2r_core::played::Played::new(),
    );
    let chars = json
        .get("characters")
        .and_then(|v| v.as_array())
        .map_or_else(Vec::new, Vec::clone);
    assert_eq!(chars.len(), 1, "one character: {chars:?}");
    let char = chars.first().map_or(serde_json::Value::Null, Clone::clone);
    assert_eq!(char.get("name"), Some(&serde_json::json!("demonmaster")));
    assert_eq!(char.get("build"), Some(&serde_json::json!("blood boil")));
    assert_eq!(char.get("build_name"), Some(&serde_json::json!("Blood Boil")));
    assert_eq!(
        char.get("damages"),
        Some(&serde_json::json!(["physical", "fire"]))
    );
    let weapon = char
        .pointer("/slots/weapon/0")
        .map_or(serde_json::Value::Null, Clone::clone);
    assert_eq!(
        weapon.get("piece"),
        Some(&serde_json::json!("Heart of the Oak@55"))
    );
    assert_eq!(weapon.get("name"), Some(&serde_json::json!("Heart of the Oak")));
    assert_eq!(weapon.get("level"), Some(&serde_json::json!(55)));
    assert_eq!(weapon.get("tick"), Some(&serde_json::Value::Null));
    assert_eq!(weapon.get("better"), Some(&serde_json::json!(false)));
    assert_eq!(
        weapon.get("runes"),
        Some(&serde_json::json!(["Ko", "Vex", "Pul", "Thul"]))
    );
    assert_eq!(
        char.pointer("/merc_slots/weapon/0/name"),
        Some(&serde_json::json!("Insight"))
    );
}

#[test]
fn config_json_marks_owned_stash_and_better_pieces() {
    // Owned ✓, stash ✓(stash), and an equipped unique strikes the chase pieces through.
    let gear = GearInfo {
        worn: vec![],
        owned: vec!["Heart of the Oak".to_owned()],
        stash: vec!["Insight".to_owned()],
        equipped: vec![(4, 7)], // body slot 4 = weapon, quality 7 = unique
        merc_equipped: vec![],
    };
    let json = config_json(&resolved_config(), &gear, &d2r_core::played::Played::new());
    let char = json
        .pointer("/characters/0")
        .map_or(serde_json::Value::Null, Clone::clone);
    assert_eq!(
        char.pointer("/slots/weapon/0/tick"),
        Some(&serde_json::json!("owned"))
    );
    assert_eq!(
        char.pointer("/slots/weapon/0/better"),
        Some(&serde_json::json!(true)),
        "a unique equipped in the weapon slot beats the chase pieces"
    );
    assert_eq!(
        char.pointer("/merc_slots/weapon/0/tick"),
        Some(&serde_json::json!("stash"))
    );
    // Worn beats owned.
    let wearing = GearInfo {
        worn: vec!["Heart of the Oak".to_owned()],
        ..gear
    };
    let json = config_json(&resolved_config(), &wearing, &d2r_core::played::Played::new());
    assert_eq!(
        json.pointer("/characters/0/slots/weapon/0/tick"),
        Some(&serde_json::json!("worn"))
    );
}

fn column_of<'a>(gear: &'a serde_json::Value, title: &str) -> &'a serde_json::Value {
    gear.get("columns")
        .and_then(|c| c.as_array())
        .and_then(|cols| {
            cols.iter()
                .find(|c| c.get("title").and_then(|t| t.as_str()) == Some(title))
        })
        .unwrap_or(&serde_json::Value::Null)
}

/// The `(name, level, state, tick)` of a column's pieces.
fn piece_states(col: &serde_json::Value) -> Vec<(String, serde_json::Value, String, serde_json::Value)> {
    col.get("pieces")
        .and_then(|p| p.as_array())
        .map_or_else(Vec::new, |pieces| {
            pieces
                .iter()
                .map(|p| {
                    (
                        p.get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        p.get("level").cloned().unwrap_or_default(),
                        p.get("state")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        p.get("tick").cloned().unwrap_or_default(),
                    )
                })
                .collect()
        })
}

#[test]
fn gear_grid_states_and_equipped_headers() {
    use d2r_core::offsets::item_data;
    // clvl 42: the gate is at 47.
    let equipped = [
        // A magic Shako (base uap, levelreq 43): unnameable, but comparable.
        d2r_core::game::Equipped {
            body: 1,
            quality: item_data::QUALITY_MAGIC,
            code: Some("uap".to_owned()),
            name: None,
            file_index: None,
            runeword: false,
            stats: Vec::new(),
        },
        // A finished runeword on a Crystal Sword: uncomparable.
        d2r_core::game::Equipped {
            body: 4,
            quality: item_data::QUALITY_NORMAL,
            code: Some("crs".to_owned()),
            name: None,
            file_index: None,
            runeword: true,
            stats: Vec::new(),
        },
        // An identified Harlequin Crest: exact name and level.
        equipped_unique(2),
    ];
    let slots = [
        (
            "helm",
            vec!["Peasant Crown@25".to_owned(), "Valkyrie Wing@71".to_owned()],
        ),
        ("amulet", vec!["Harlequin Crest@62".to_owned()]),
        ("weapon", vec!["Heart of the Oak@55".to_owned()]),
        ("ring1", vec!["Naj's Rite@30".to_owned()]),
        ("ring2", vec!["Dwarf Star@45".to_owned()]),
        ("offhand", vec!["Spirit Monarch@35".to_owned()]),
    ];
    let gear = gear_grid(&slots, &equipped, &["Heart of the Oak".to_owned()]);
    let titles: Vec<&str> = gear.get("columns").map_or_default(|v| {
        v.as_array()
            .iter()
            .flat_map(|pieces| pieces.iter())
            .filter_map(|x| x.get("title").and_then(|val| val.as_str()))
            .collect()
    });
    assert_eq!(
        titles,
        [
            "Helm", "Body", "Hands", "Belt", "Foot", "Amulet", "Ring", "Weapons", "Off-Hand", "Swap"
        ]
    );
    // Helm: a magic piece is unnameable but comparable (base level 43); 71 is above the gate.
    let helm = column_of(&gear, "Helm");
    assert_eq!(
        helm.get("equipped").cloned().unwrap_or_default(),
        serde_json::json!([{ "name": "(unknown)", "level": 43 }])
    );
    assert_eq!(
        piece_states(helm),
        vec![
            (
                "Peasant Crown".to_owned(),
                serde_json::json!(25),
                "worse".to_owned(),
                serde_json::Value::Null
            ),
            (
                "Valkyrie Wing".to_owned(),
                serde_json::json!(71),
                "too_high".to_owned(),
                serde_json::Value::Null
            ),
        ]
    );
    let amulet = column_of(&gear, "Amulet");
    assert_eq!(
        amulet.get("equipped").cloned().unwrap_or_default(),
        serde_json::json!([{ "name": "Harlequin Crest", "level": 62 }])
    );
    // 62 ≤ 62, but the piece is the worn item itself: never struck.
    assert_eq!(
        piece_states(amulet).first().map(|p| p.2.as_str()),
        Some("neutral"),
    );
    // Weapons: an equipped runeword is uncomparable; 55 is above the 47 gate.
    let weapons = column_of(&gear, "Weapons");
    assert_eq!(
        weapons.get("equipped").cloned().unwrap_or_default(),
        serde_json::json!([{ "name": "runeword (unnamed)", "level": serde_json::Value::Null }])
    );
    assert_eq!(
        piece_states(weapons),
        vec![(
            "Heart of the Oak".to_owned(),
            serde_json::json!(55),
            "too_high".to_owned(),
            serde_json::json!("owned")
        )]
    );
    // Off-hand: nothing equipped, and the Crystal Sword is one-handed: no note.
    let offhand = column_of(&gear, "Off-Hand");
    assert_eq!(
        piece_states(offhand).first().map(|p| p.2.as_str()),
        Some("better"),
    );
    assert_eq!(offhand.get("note"), Some(&serde_json::Value::Null));
}

#[test]
fn gear_grid_merges_the_ring_slots() {
    // Dwarf Star (lvl 45) and a magic ring: the merged column compares against the higher.
    let equipped = [
        d2r_core::game::Equipped {
            body: 6,
            quality: d2r_core::offsets::item_data::QUALITY_UNIQUE,
            code: Some("rin".to_owned()),
            name: Some("Dwarf Star".to_owned()),
            file_index: Some(274),
            runeword: false,
            stats: Vec::new(),
        },
        d2r_core::game::Equipped {
            body: 7,
            quality: d2r_core::offsets::item_data::QUALITY_MAGIC,
            code: Some("rin".to_owned()),
            name: None,
            file_index: None,
            runeword: false,
            stats: Vec::new(),
        },
    ];
    let slots = [
        ("ring1", vec!["Dwarf Star@45".to_owned(), "Ring A@20".to_owned()]),
        ("ring2", vec!["Ring A@20".to_owned(), "Ring C@46".to_owned()]),
    ];
    let gear = gear_grid(&slots, &equipped, &[]);
    let ring = column_of(&gear, "Ring");
    assert_eq!(
        ring.get("equipped").map(|e| e.as_array().map_or(0, Vec::len)),
        Some(2)
    );
    let names: Vec<&str> = ring.get("pieces").map_or_default(|v| {
        v.as_array()
            .iter()
            .flat_map(|pieces| pieces.iter())
            .filter_map(|x| x.get("name").and_then(|val| val.as_str()))
            .collect()
    });
    assert_eq!(names, ["Dwarf Star", "Ring A", "Ring C"]);
    let states: Vec<&str> = ring.get("pieces").map_or_default(|v| {
        v.as_array()
            .iter()
            .flat_map(|pieces| pieces.iter())
            .filter_map(|x| x.get("state").and_then(|val| val.as_str()))
            .collect()
    });
    assert_eq!(
        states,
        ["neutral", "worse", "better"],
        "the worn Dwarf Star is not struck"
    );
}

#[test]
fn gear_grid_notes_two_handed_weapons() {
    // Elder Staff (6cs) is two-handed: the off-hand gets the note.
    for (code, expected) in [("6cs", Some("(two-handed weapon equipped)")), ("crs", None)] {
        let equipped = [d2r_core::game::Equipped {
            body: 4,
            quality: d2r_core::offsets::item_data::QUALITY_NORMAL,
            code: Some(code.to_owned()),
            name: None,
            file_index: None,
            runeword: false,
            stats: Vec::new(),
        }];
        let gear = gear_grid(
            &[("offhand", vec!["Spirit Monarch@35".to_owned()])],
            &equipped,
            &[],
        );
        assert_eq!(
            column_of(&gear, "Off-Hand").get("note").and_then(|n| n.as_str()),
            expected,
            "weapon code {code}"
        );
    }
}

#[test]
fn builds_json_lists_slug_name_damages_and_counts() {
    let json = builds_json(&resolved_config());
    let builds = json
        .get("builds")
        .and_then(|v| v.as_array())
        .map_or_else(Vec::new, Vec::clone);
    assert_eq!(builds.len(), 1, "{builds:?}");
    let build = builds.first().map_or(serde_json::Value::Null, Clone::clone);
    assert_eq!(build.get("slug"), Some(&serde_json::json!("blood boil")));
    assert_eq!(build.get("name"), Some(&serde_json::json!("Blood Boil")));
    assert_eq!(
        build.get("damages"),
        Some(&serde_json::json!(["physical", "fire"]))
    );
    assert_eq!(build.get("notes"), Some(&serde_json::json!("guide")));
    assert_eq!(
        build.get("slots"),
        Some(&serde_json::json!({"weapon": 1})),
        "per-slot piece counts"
    );
    assert_eq!(build.get("merc_slots"), Some(&serde_json::json!({"weapon": 1})));
}

/// A gear grid over a one-slot build, one equipped piece and one owned name.
fn gear_grid<'a>(
    slots: &[(&str, Vec<String>)],
    equipped_items: &'a [d2r_core::game::Equipped],
    owned: &'a [String],
) -> serde_json::Value {
    let bis = crate::config::Bis {
        build: "test".to_owned(),
        slots: slots
            .iter()
            .map(|(slot, pieces)| ((*slot).to_owned(), pieces.clone()))
            .collect(),
        merc_slots: BTreeMap::new(),
    };
    let info = PanelInfo {
        live: LiveInfo {
            player_name: "Hork",
            class: 0,
            char_level: Some(42),
            difficulty: Some(2),
            area: "test",
            blocks: &d2r_core::render::TextBlocks::default(),
            equipped: &[],
            equipped_items,
            owned_items: &[],
            owned,
            runeword_copies: &[],
            merc_worn: &[],
            stash: &[],
            stash_owned: &[],
            skills: &EMPTY_SKILLS,
            held_materials: &[],
            white_bases: &[],
            quests: &[],
            shop: None,
            strength: None,
            dexterity: None,
            marks: &EMPTY_MARKS,
            updated_at: 1_700_000_000,
        },
        bis: Some(&bis),
        build: None,
        stage: None,
        farm: &[],
        route: &NO_ROUTE,
        stage_chain: &[],
        not_needed: &NO_MARKS,
    };
    gear_json(&info)
}

/// Harlequin Crest's live-verified row (248 → lvl req 62).
fn equipped_unique(body: u8) -> d2r_core::game::Equipped {
    d2r_core::game::Equipped {
        body,
        quality: d2r_core::offsets::item_data::QUALITY_UNIQUE,
        code: Some("uap".to_owned()),
        name: Some("Harlequin Crest".to_owned()),
        file_index: Some(248),
        runeword: false,
        stats: Vec::new(),
    }
}

fn advice(
    name: &str,
    spot: crate::stash_plan::Spot,
    action: crate::stash_plan::Action,
) -> crate::stash_plan::Advice {
    crate::stash_plan::Advice {
        cells: Vec::new(),
        name: name.to_owned(),
        spot,
        action,
        reason: None,
        count: 1,
    }
}

#[test]
fn live_json_has_the_documented_shape() -> Result<()> {
    use crate::stash_plan::{Action, Side, Spot};
    let blocks = d2r_core::render::TextBlocks {
        guide: vec![d2r_core::info::Band::plain(
            "fcr (7 / 105)",
            d2r_core::info::GUIDE_TEXT,
        )],
        sell: vec![advice("Gravenspine", Spot::Inventory, Action::Sell)],
        moves: vec![advice("Hunter's Bow 4os", Spot::Page(4), Action::Move(Side::Own))],
        keep_own: vec![advice("Spirit", Spot::Page(1), Action::Keep(Side::Own))],
        keep_trade: vec![advice(
            "Harlequin Crest",
            Spot::Page(3),
            Action::Keep(Side::Trade),
        )],
        give: vec![advice("Magefist", Spot::Stash, Action::GiveAway)],
        bis: vec![d2r_core::info::BisLine {
            text: "helm: Harlequin Crest (82)".to_owned(),
            tick: None,
            runeword: None,
        }],
    };
    let equipped = [(1, 7)];
    let stash = test_stash();
    let stash_owned = ["Harlequin Crest".to_owned()];
    let json = serde_json::from_str::<serde_json::Value>(&live_json(&live_info(
        1,
        &blocks,
        &equipped,
        &stash,
        &stash_owned,
    )))
    .map_err(|e| anyhow!("valid json: {e}"))?;
    assert_eq!(json.get("live"), Some(&serde_json::json!(true)));
    assert_eq!(json.get("updated_at"), Some(&serde_json::json!(1_700_000_000)));
    assert_eq!(
        json.get("player"),
        Some(&serde_json::json!({
            "name": "Hork", "class": "sorceress", "level": 42, "difficulty": "Hell",
        }))
    );
    assert_eq!(
        json.get("area").and_then(|v| v.as_str()),
        Some("Durance of Hate Level 3")
    );
    let colour_of = |key: &str| -> Vec<(String, String)> { lines_of(&json, key) };
    assert_eq!(
        colour_of("guide"),
        vec![("fcr (7 / 105)".to_owned(), "#c882ff".to_owned())]
    );
    assert_eq!(json.get("levels"), None, "the level band is gone");
    assert_eq!(
        colour_of("sell"),
        vec![("Gravenspine (inv)".to_owned(), "#afafaf".to_owned())]
    );
    assert_eq!(
        colour_of("move"),
        vec![("Hunter's Bow 4os (page 4 → 1–2)".to_owned(), "#ffa03c".to_owned())]
    );
    assert_eq!(
        colour_of("keep_own"),
        vec![("Spirit (page 1)".to_owned(), "#5ae65a".to_owned())]
    );
    assert_eq!(
        colour_of("keep_trade"),
        vec![("Harlequin Crest (page 3)".to_owned(), "#6ebeff".to_owned())]
    );
    assert_eq!(json.get("keep"), None, "the old keep list is gone");
    assert_eq!(
        colour_of("give"),
        vec![("Magefist (stash)".to_owned(), "#ffe13c".to_owned())]
    );
    assert_eq!(
        colour_of("bis"),
        vec![("helm: Harlequin Crest (82)".to_owned(), "#ffd278".to_owned())]
    );
    assert_eq!(
        colour_of("equipped"),
        vec![("helm: unique".to_owned(), "#ffffff".to_owned())]
    );
    assert_eq!(
        json.get("stash_owned"),
        Some(&serde_json::json!(["Harlequin Crest"]))
    );
    Ok(())
}

#[test]
fn live_json_bis_lines_carry_structural_ticks() -> Result<()> {
    let blocks = d2r_core::render::TextBlocks {
        bis: vec![
            d2r_core::info::BisLine {
                text: "helm: Harlequin Crest (82)".to_owned(),
                tick: Some(d2r_core::info::Tick::Owned),
                runeword: None,
            },
            d2r_core::info::BisLine {
                text: "merc body: ✓ Treachery (23)".to_owned(),
                tick: Some(d2r_core::info::Tick::Stash),
                runeword: None,
            },
            d2r_core::info::BisLine {
                text: "weapon: Mang Song's Lesson (82)".to_owned(),
                tick: None,
                runeword: None,
            },
        ],
        ..d2r_core::render::TextBlocks::default()
    };
    let json = serde_json::from_str::<serde_json::Value>(&live_json(&live_info(1, &blocks, &[], &[], &[])))
        .map_err(|e| anyhow!("valid json: {e}"))?;
    let ticks: Vec<String> = json
        .get("bis")
        .and_then(|v| v.as_array())
        .map_or_else(Vec::new, |lines| {
            lines
                .iter()
                .map(|l| {
                    l.get("tick").map_or_else(
                        || "missing".to_owned(),
                        |t| {
                            if t.is_null() {
                                String::new()
                            } else {
                                t.as_str().unwrap_or("not a string").to_owned()
                            }
                        },
                    )
                })
                .collect()
        });
    assert_eq!(ticks, ["owned", "stash", ""]);
    Ok(())
}

#[test]
fn live_json_bis_lines_carry_runeword_runes_and_bases() -> Result<()> {
    let treachery = d2r_core::runewords_data::lookup("Treachery").copied();
    let blocks = d2r_core::render::TextBlocks {
        bis: vec![
            d2r_core::info::BisLine {
                text: "merc body: Treachery (43)".to_owned(),
                tick: None,
                runeword: treachery,
            },
            d2r_core::info::BisLine {
                text: "helm: Harlequin Crest (82)".to_owned(),
                tick: None,
                runeword: None,
            },
        ],
        ..d2r_core::render::TextBlocks::default()
    };
    let json = serde_json::from_str::<serde_json::Value>(&live_json(&live_info(7, &blocks, &[], &[], &[])))
        .map_err(|e| anyhow!("valid json: {e}"))?;
    let bis = json
        .get("bis")
        .and_then(|v| v.as_array())
        .map_or_else(Vec::new, Vec::clone);
    assert_eq!(bis.len(), 2, "{bis:?}");
    let first = bis.first().map_or(serde_json::Value::Null, Clone::clone);
    assert_eq!(
        first.get("runes"),
        Some(&serde_json::json!(["Shael", "Thul", "Lem"]))
    );
    assert_eq!(first.get("bases"), Some(&serde_json::json!(["armor"])));
    let second = bis.get(1).map_or(serde_json::Value::Null, Clone::clone);
    assert!(second.get("runes").is_none(), "no runes for uniques: {second:?}");
    assert!(second.get("bases").is_none(), "no bases for uniques: {second:?}");
    Ok(())
}

#[test]
fn gear_carries_build_details_merc_and_extras() {
    let build = crate::config::Build {
        name: "Blood Boil".to_owned(),
        targets: vec!["75% FCR".to_owned()],
        extras: [("charms".to_owned(), vec!["Annihilus@70".to_owned()])].into(),
        pieces: [(
            "Harlequin Crest".to_owned(),
            crate::config::PieceInfo {
                kind: "unique".to_owned(),
                base: "Shako".to_owned(),
                stats: vec!["+2 to All Skills".to_owned()],
                ..crate::config::PieceInfo::default()
            },
        )]
        .into(),
        ..crate::config::Build::default()
    };
    let bis = crate::config::Bis {
        build: "Blood Boil".to_owned(),
        slots: [("helm".to_owned(), vec!["Harlequin Crest@62".to_owned()])].into(),
        merc_slots: [("weapon".to_owned(), vec!["Insight@27".to_owned()])].into(),
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(0, &blocks, &[], &[], &[]);
    info.bis = Some(&bis);
    info.build = Some(&build);
    let gear = gear_json(&info);
    let at = |path: &str| gear.pointer(path).cloned().unwrap_or_default();
    assert_eq!(at("/build"), "Blood Boil");
    assert_eq!(at("/targets"), serde_json::json!(["75% FCR"]));
    let helm = at("/columns/0/pieces/0");
    assert_eq!(
        (
            at("/columns/0/pieces/0/name"),
            at("/columns/0/pieces/0/kind"),
            at("/columns/0/pieces/0/base")
        ),
        (
            serde_json::json!("Harlequin Crest"),
            serde_json::json!("unique"),
            serde_json::json!("Shako")
        )
    );
    assert_eq!(
        at("/columns/0/pieces/0/stats"),
        serde_json::json!(["+2 to All Skills"])
    );
    assert!(helm.get("why").is_none(), "unset details are left out");
    assert_eq!(at("/merc/0/title"), "Merc weapon");
    assert_eq!(at("/merc/0/pieces/0/name"), "Insight");
    assert_eq!(at("/extras/0/title"), "charms");
    assert_eq!(at("/extras/0/pieces/0/level"), 70);
}

#[test]
fn outgrown_leveling_pieces_leave_and_runes_never_expire() {
    let window = |kind: &str, setup: &str| crate::config::PieceInfo {
        kind: kind.to_owned(),
        setup: setup.to_owned(),
        ..crate::config::PieceInfo::default()
    };
    let build = crate::config::Build {
        name: "Druid leveling".to_owned(),
        pieces: [
            ("Old Pelt".to_owned(), window("base", "lvl 1-13")),
            ("Stealth".to_owned(), window("runeword", "lvl 13-26")),
            ("Lore".to_owned(), window("runeword", "lvl 26-38")),
            ("Tal Rune".to_owned(), window("rune", "lvl 13-26")),
            ("Shako".to_owned(), window("unique", "standard")),
        ]
        .into(),
        ..crate::config::Build::default()
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(0, &blocks, &[], &[], &[]);
    info.build = Some(&build);
    info.live.char_level = Some(21);
    let pieces = ["Old Pelt", "Stealth@17", "Lore@27", "Tal Rune@17", "Shako@62"].map(str::to_owned);
    let json = grid_pieces(&pieces, None, &[], (None, Wearer::Player(info.live.class)), &info);
    let names: Vec<_> = json.iter().filter_map(|p| p.get("name")?.as_str()).collect();
    assert_eq!(
        names,
        ["Stealth", "Lore", "Tal Rune", "Shako"],
        "lvl 1-13 is past at 21"
    );
    let field = |name: &str, key: &str| {
        json.iter()
            .find(|p| p.get("name").and_then(|n| n.as_str()) == Some(name))
            .and_then(|p| p.get(key).cloned())
    };
    assert_eq!(field("Stealth", "now"), Some(serde_json::json!(true)));
    assert_eq!(field("Lore", "now"), None, "not yet");
    // In Lore's window (26-38) but below its own level 27: still not "wear now".
    info.live.char_level = Some(26);
    let json26 = grid_pieces(&pieces, None, &[], (None, Wearer::Player(info.live.class)), &info);
    let lore = json26
        .iter()
        .find(|p| p.get("name").and_then(|n| n.as_str()) == Some("Lore"));
    assert_eq!(lore.and_then(|p| p.get("now")), None, "level 27 needed");
    info.live.char_level = Some(27);
    let json27 = grid_pieces(&pieces, None, &[], (None, Wearer::Player(info.live.class)), &info);
    let lore = json27
        .iter()
        .find(|p| p.get("name").and_then(|n| n.as_str()) == Some("Lore"));
    assert_eq!(lore.and_then(|p| p.get("now")), Some(&serde_json::json!(true)));
    info.live.char_level = Some(21);
    assert_eq!(field("Tal Rune", "setup"), Some(serde_json::json!("lvl 17+")));
    info.live.char_level = Some(60);
    let json = grid_pieces(&pieces, None, &[], (None, Wearer::Player(info.live.class)), &info);
    let names: Vec<_> = json.iter().filter_map(|p| p.get("name")?.as_str()).collect();
    assert_eq!(names, ["Tal Rune", "Shako"], "a rune never runs out");
    assert_eq!(level_range("lvl 13-26"), Some((13, 26)));
    assert_eq!(level_range("standard"), None);
}

#[test]
fn recipe_inputs_parse() {
    let material = |name: &str, need| Input::Material {
        name: name.to_owned(),
        need,
    };
    assert_eq!(Input::parse("Tal"), material("Tal Rune", 1));
    assert_eq!(Input::parse("3 Tal Rune"), material("Tal Rune", 3));
    assert_eq!(Input::parse("1-2 Ral Rune"), material("Ral Rune", 1));
    assert_eq!(Input::parse("Ort Rune x2"), material("Ort Rune", 2));
    assert_eq!(Input::parse("1 Chipped Topaz"), material("Chipped Topaz", 1));
    assert_eq!(Input::parse("Perfect skull"), material("Perfect Skull", 1));
    assert_eq!(
        Input::parse("1 Chipped Gem (any)"),
        Input::AnyGem {
            grade: "chipped".to_owned(),
            need: 1
        }
    );
    assert_eq!(
        Input::parse("non-magic Body Armor with exactly 2 sockets"),
        Input::Base
    );
    assert_eq!(Input::parse("Weapon"), Input::Base);
    assert_eq!(socket_count("non-magic Helm with exactly 2 sockets"), Some(2));
    assert_eq!(socket_count("3-socket shield"), Some(3));
    assert_eq!(socket_count("Socketed (2)"), None);
}

/// Stealth (body, 17) and Leaf (weapon, 19), Tal → Ral and Ort → Thul upgrades, a cube
/// recipe, and one unique per slot.
fn guide_build() -> config::Build {
    let recipe = |name: &str, kind: &str, inputs: &[&str], result: &str| config::Recipe {
        name: name.to_owned(),
        kind: kind.to_owned(),
        inputs: inputs.iter().map(|&i| i.to_owned()).collect(),
        result: result.to_owned(),
        ..config::Recipe::default()
    };
    config::Build {
        name: "Leveling".to_owned(),
        slots: [
            (
                "body".to_owned(),
                vec!["Stealth@17".to_owned(), "2os Body Armor".to_owned()],
            ),
            ("weapon".to_owned(), vec!["Leaf@19".to_owned()]),
            (
                "helm".to_owned(),
                vec!["Harlequin Crest@62".to_owned(), "Biggin's Bonnet@3".to_owned()],
            ),
            ("ring1".to_owned(), vec!["Nagelring@7".to_owned()]),
            ("ring2".to_owned(), vec!["Nagelring@7".to_owned()]),
        ]
        .into(),
        recipes: vec![
            recipe(
                "Stealth",
                "runeword",
                &[
                    "Tal",
                    "Eth",
                    "non-magic Hard Leather Armor with exactly 2 sockets",
                ],
                "Stealth",
            ),
            recipe(
                "Leaf",
                "runeword",
                &["Tir", "Ral", "non-magic Staff with 2 sockets"],
                "Leaf",
            ),
            recipe("3 Tal -> Ral", "upgrade", &["3 Tal Rune"], "Ral Rune"),
            recipe("3 Ort -> Thul", "upgrade", &["3 Ort Rune"], "Thul Rune"),
            recipe("Imbue", "upgrade", &["white Druid Pelt"], "Rare Druid Pelt"),
            recipe(
                "Recharge",
                "cube",
                &["1 Ort Rune", "1 Chipped Gem (any)", "Weapon"],
                "recharged",
            ),
        ],
        ..config::Build::default()
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "test fixture builder; each argument is one LiveInfo field"
)]
fn guide_json(
    build: &config::Build,
    level: u8,
    stash: &[d2r_core::stash::StashTab],
    held: &[(String, u32)],
    bases: &[String],
    owned: &[String],
    equipped_items: &[d2r_core::game::Equipped],
) -> Result<serde_json::Value> {
    let bis = config::Bis {
        build: build.name.clone(),
        slots: build.slots.clone(),
        merc_slots: BTreeMap::new(),
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], stash, &[]);
    info.live.char_level = Some(level);
    info.bis = Some(&bis);
    info.build = Some(build);
    info.live.held_materials = held;
    info.live.white_bases = bases;
    info.live.owned = owned;
    info.live.equipped_items = equipped_items;
    Ok(serde_json::from_str(&live_json(&info))?)
}

/// The materials tab with `stacks` of `(name, code, qty)`.
fn materials_tab(stacks: &[(&str, &str, u32)]) -> Vec<d2r_core::stash::StashTab> {
    vec![d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Materials,
        items: stacks
            .iter()
            .map(|&(name, code, qty)| {
                stash_item(
                    name,
                    code,
                    d2r_core::offsets::item_data::QUALITY_NORMAL,
                    Some(qty),
                )
            })
            .collect(),
    }]
}

#[test]
fn recipes_tick_owned_inputs_and_fold_upgrades() -> Result<()> {
    let build = guide_build();
    let stash = materials_tab(&[("Tal Rune", "r07", 1), ("Ral Rune", "r08", 1)]);
    let held = [("Eth Rune".to_owned(), 1), ("Chipped Ruby".to_owned(), 2)];
    let bases = ["Hard Leather Armor 2os".to_owned()];
    let json = guide_json(&build, 18, &stash, &held, &bases, &[], &[])?;
    let recipes = json
        .get("recipes")
        .and_then(|r| r.as_array())
        .context("recipes")?;
    let names: Vec<&str> = recipes.iter().filter_map(|r| r.get("name")?.as_str()).collect();
    // Tir is missing but no upgrade makes it; Thul is needed nowhere; Ral is owned.
    assert_eq!(names, ["Stealth", "Leaf", "Imbue", "Recharge"]);
    let statuses = |i: usize| -> Vec<String> {
        recipes
            .get(i)
            .and_then(|r| r.get("inputs")?.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|l| {
                l.get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    };
    assert_eq!(
        statuses(0),
        ["have", "have", "likely"],
        "the 2os Hard Leather Armor hint"
    );
    assert_eq!(
        recipes.first().and_then(|r| r.pointer("/inputs/2/hint")),
        Some(&serde_json::json!("Hard Leather Armor 2os"))
    );
    assert_eq!(statuses(1), ["missing", "have", "check"]);
    assert_eq!(
        recipes.get(1).and_then(|r| r.pointer("/inputs/0/have")),
        Some(&serde_json::json!(0))
    );
    assert_eq!(
        statuses(3),
        ["missing", "have", "check"],
        "any chipped gem counts"
    );
    assert_eq!(
        recipes.get(2).and_then(|r| r.get("material")),
        Some(&serde_json::json!(false))
    );
    // With Ral missing too (a two-Ral runeword), Tal → Ral appears, flagged material.
    let mut two_ral = guide_build();
    if let Some(leaf) = two_ral.recipes.get_mut(1) {
        leaf.inputs = vec!["Ral".to_owned(), "Ral".to_owned()];
    }
    let json = guide_json(&two_ral, 18, &stash, &held, &bases, &[], &[])?;
    let leaf = json.pointer("/recipes/1/inputs/0").cloned().unwrap_or_default();
    assert_eq!(
        (leaf.get("have"), leaf.get("need")),
        (Some(&serde_json::json!(1)), Some(&serde_json::json!(2)))
    );
    let upgrade = json
        .get("recipes")
        .and_then(|r| r.as_array())
        .and_then(|r| {
            r.iter()
                .find(|r| r.get("kind") == Some(&"upgrade".into()) && r.get("material") == Some(&true.into()))
        })
        .and_then(|r| r.get("name").cloned());
    assert_eq!(upgrade, Some(serde_json::json!("3 Tal -> Ral")));
    Ok(())
}

fn recipe_names(build: &config::Build, copies: &[String], stashed: &[&str]) -> Result<Vec<String>> {
    let blocks = d2r_core::render::TextBlocks::default();
    let stash = vec![d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(1),
        items: stashed
            .iter()
            .map(|&name| d2r_core::stash::StashItem {
                runeword: true,
                ..stash_item(name, "aar", d2r_core::offsets::item_data::QUALITY_NORMAL, None)
            })
            .collect(),
    }];
    let mut info = live_info(5, &blocks, &[], &stash, &[]);
    info.build = Some(build);
    info.live.runeword_copies = copies;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    Ok(json
        .get("recipes")
        .and_then(|r| r.as_array())
        .context("recipes")?
        .iter()
        .filter_map(|r| r.get("name").and_then(|n| n.as_str()).map(str::to_owned))
        .collect())
}

#[test]
fn recipes_hide_runewords_already_built() -> Result<()> {
    let build = guide_build();
    let all = recipe_names(&build, &[], &[])?;
    assert!(
        all.contains(&"Stealth".to_owned()) && all.contains(&"Leaf".to_owned()),
        "{all:?}"
    );
    // Held anywhere, any case: Stealth goes, Leaf stays.
    let got = recipe_names(&build, &["stealth".to_owned()], &[])?;
    assert!(
        !got.contains(&"Stealth".to_owned()) && got.contains(&"Leaf".to_owned()),
        "{got:?}"
    );
    let got = recipe_names(&build, &[], &["Leaf"])?;
    assert!(
        !got.contains(&"Leaf".to_owned()) && got.contains(&"Stealth".to_owned()),
        "{got:?}"
    );
    let got = recipe_names(&build, &["Recharge".to_owned(), "Imbue".to_owned()], &[])?;
    assert!(
        got.contains(&"Recharge".to_owned()) && got.contains(&"Imbue".to_owned()),
        "{got:?}"
    );
    Ok(())
}

#[test]
fn recipes_keep_runewords_until_every_slot_has_a_copy() -> Result<()> {
    let mut build = guide_build();
    build
        .merc_slots
        .insert("body".to_owned(), vec!["Stealth@17".to_owned()]);
    assert_eq!(runeword_slots(&build, "Stealth"), 2);
    assert_eq!(runeword_slots(&build, "Leaf"), 1);
    assert_eq!(runeword_slots(&build, "Spirit"), 1, "listed in no slot: one copy");
    let stealth = |copies: &[String], stashed: &[&str]| -> Result<bool> {
        Ok(recipe_names(&build, copies, stashed)?.contains(&"Stealth".to_owned()))
    };
    assert!(stealth(&[], &[])?);
    assert!(stealth(&["Stealth".to_owned()], &[])?);
    assert!(stealth(&[], &["Stealth"])?);
    assert!(!stealth(&["Stealth".to_owned(), "Stealth".to_owned()], &[])?);
    assert!(!stealth(&["Stealth".to_owned()], &["Stealth"])?);
    assert!(stealth(&["Stealth".to_owned(), "Leaf".to_owned()], &[])?);
    Ok(())
}

#[test]
fn recipes_hide_used_quest_rewards() -> Result<()> {
    let mut build = guide_build();
    if let Some(r) = build.recipes.iter_mut().find(|r| r.name == "Imbue") {
        r.quest = "Tools of the Trade".to_owned();
    }
    let quest_recipe = |name: &str, quest: &str| config::Recipe {
        name: name.to_owned(),
        kind: "socket".to_owned(),
        quest: quest.to_owned(),
        ..config::Recipe::default()
    };
    build.recipes.push(quest_recipe("Larzuk", "Siege on Harrogath"));
    build.recipes.push(quest_recipe("Respec", "Akara respec"));
    let blocks = d2r_core::render::TextBlocks::default();
    let quest_view = |quests: &[u16]| -> Result<Vec<(String, serde_json::Value)>> {
        let mut info = live_info(5, &blocks, &[], &[], &[]);
        info.build = Some(&build);
        info.live.quests = quests;
        let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
        Ok(json
            .get("recipes")
            .and_then(|r| r.as_array())
            .context("recipes")?
            .iter()
            .map(|r| {
                (
                    r.get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    r.get("quest").cloned().unwrap_or_default(),
                )
            })
            .filter(|(name, _)| ["Imbue", "Larzuk", "Respec"].contains(&name.as_str()))
            .collect())
    };
    let null = serde_json::Value::Null;
    assert_eq!(
        quest_view(&[])?,
        [
            ("Imbue".to_owned(), null.clone()),
            ("Larzuk".to_owned(), null.clone()),
            ("Respec".to_owned(), null)
        ]
    );
    // Elemental's live record: imbue used, Act 5 not reached, respec pending.
    let mut live = vec![0u16; 48];
    for (word, value) in [(3, 0x1049), (41, 0x8002)] {
        if let Some(w) = live.get_mut(word) {
            *w = value;
        }
    }
    assert_eq!(
        quest_view(&live)?,
        [
            ("Larzuk".to_owned(), serde_json::json!("todo")),
            ("Respec".to_owned(), serde_json::json!("ready"))
        ]
    );
    Ok(())
}

/// The `shop` section's items as `name|slot|high|count` lines.
fn shop_rows(json: &serde_json::Value) -> Vec<String> {
    let items = json.get("items").and_then(|i| i.as_array());
    items.map_or_else(Vec::new, |items| {
        items
            .iter()
            .map(|i| {
                let field = |k: &str| i.get(k).map_or_else(String::new, ToString::to_string);
                format!(
                    "{}|{}|{}|{}",
                    field("name"),
                    field("slot"),
                    field("high"),
                    field("count")
                )
            })
            .collect()
    })
}

/// A trade window with two build pieces, one other item and two identical potions.
#[test]
fn shop_marks_build_pieces_and_chimes_them_as_upgrades() {
    use d2r_core::shop::shop_item;
    let bis = crate::config::Bis {
        build: "test".to_owned(),
        slots: BTreeMap::from([
            ("gloves".to_owned(), vec!["Coral Heavy Gloves@13".to_owned()]),
            (
                "swap".to_owned(),
                vec!["Battle Staff of Teleportation@24".to_owned()],
            ),
        ]),
        merc_slots: BTreeMap::from([("helm".to_owned(), vec!["Full Helm".to_owned()])]),
    };
    let coral = d2r_core::affixes_data::PREFIXES
        .iter()
        .find(|&&(_, n, l)| n == "Coral" && l == 13)
        .map_or(0, |&(i, _, _)| i);
    let shop = d2r_core::shop::Shop {
        npc_txt: 255,
        npc: Some("Ormus".to_owned()),
        items: vec![
            shop_item(Some("vgl"), 4, 0, coral, 0),
            shop_item(Some("fhl"), 2, 0, 0, 0),
            shop_item(Some("bst"), 4, 1, 0, 532),
            shop_item(Some("bst"), 2, 1, 0, 0),
            shop_item(Some("hp1"), 2, 3, 0, 0),
            shop_item(Some("hp1"), 2, 3, 0, 0),
        ],
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.live.char_level = Some(15);
    info.live.difficulty = Some(0);
    info.live.area = "Kurast Docks";
    info.live.shop = Some(&shop);
    info.bis = Some(&bis);
    let json = shop_json(&info);
    assert_eq!(json.get("npc"), Some(&"Ormus".into()));
    let rows = shop_rows(&json);
    assert_eq!(
        rows,
        [
            r#""Coral Heavy Gloves"|"gloves"|false|1"#,
            r#""Full Helm"|"merc helm"|false|1"#,
            r#""Battle Staff of Teleportation"|"swap"|true|1"#,
            r#""Battle Staff"|null|false|1"#,
            r#""Minor Healing Potion"|null|false|2"#,
        ]
    );
    let ups: Vec<_> = upgrades_json(&info)
        .iter()
        .filter_map(|u| {
            Some((
                u.get("key")?.as_str()?.to_owned(),
                u.get("why")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    assert_eq!(
        ups,
        vec![
            (
                "shop:gloves:Coral Heavy Gloves".to_owned(),
                "Ormus sells it · lvl 13".to_owned()
            ),
            ("shop:merc helm:Full Helm".to_owned(), "Ormus sells it".to_owned()),
            (
                "shop:swap:Battle Staff of Teleportation".to_owned(),
                "Ormus sells it · lvl 24 — above your level".to_owned()
            ),
        ]
    );
    let closed = PanelInfo {
        live: LiveInfo {
            shop: None,
            ..info.live
        },
        ..info
    };
    assert_eq!(shop_json(&closed), serde_json::Value::Null);
}

/// The `shop` section's items as `name|better|why` lines.
fn shop_better_rows(json: &serde_json::Value) -> Vec<String> {
    let items = json.get("items").and_then(|i| i.as_array());
    items.map_or_else(Vec::new, |items| {
        items
            .iter()
            .map(|i| {
                let field = |k: &str| i.get(k).map_or_else(String::new, ToString::to_string);
                format!("{}|{}|{}", field("name"), field("better"), field("why"))
            })
            .collect()
    })
}

/// A level-12 bow amazon wearing a Short Bow and a fire ring: only the Long Bow and the
/// better ring are marked.
#[test]
fn shop_marks_items_better_than_the_worn_ones() {
    use d2r_core::item_stats::{ItemStat, stat};
    use d2r_core::shop::shop_item;
    let base = |id, value| ItemStat {
        id,
        param: 0,
        value,
        base: true,
    };
    let affix = |id, value| ItemStat {
        id,
        param: 0,
        value,
        base: false,
    };
    let worn_item = |body: u8, code: &str, stats: Vec<ItemStat>| d2r_core::game::Equipped {
        body,
        quality: 2,
        code: Some(code.to_owned()),
        name: None,
        file_index: None,
        runeword: false,
        stats,
    };
    let equipped_items = [
        worn_item(
            4,
            "sbw",
            vec![base(stat::MIN_DAMAGE_2H, 1), base(stat::MAX_DAMAGE_2H, 4)],
        ),
        worn_item(6, "rin", vec![affix(stat::RES_FIRE, 20)]),
    ];
    let with = |mut item: d2r_core::shop::ShopItem, stats: Vec<ItemStat>| {
        item.stats = stats;
        item
    };
    let long_bow = vec![
        base(stat::MIN_DAMAGE_2H, 3),
        base(stat::MAX_DAMAGE_2H, 10),
        affix(stat::RES_FIRE, 15),
    ];
    let shop = d2r_core::shop::Shop {
        npc_txt: 148,
        npc: Some("Charsi".to_owned()),
        items: vec![
            with(shop_item(Some("lbw"), 2, 1, 0, 0), long_bow.clone()),
            with(shop_item(Some("6lb"), 2, 1, 0, 0), long_bow),
            with(
                shop_item(Some("jav"), 2, 1, 0, 0),
                vec![base(stat::THROW_MIN, 6), base(stat::THROW_MAX, 14)],
            ),
            with(
                shop_item(Some("rin"), 2, 3, 0, 0),
                vec![affix(stat::MAX_LIFE, 30)],
            ),
            with(shop_item(Some("rin"), 4, 3, 0, 0), vec![affix(stat::MAX_LIFE, 5)]),
        ],
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let info = PanelInfo {
        live: LiveInfo {
            player_name: "Fulmina",
            char_level: Some(12),
            difficulty: Some(0),
            area: "Rogue Encampment",
            equipped_items: &equipped_items,
            shop: Some(&shop),
            strength: Some(30),
            dexterity: Some(40),
            ..d2r_core::web::fixtures::live_info(0, &blocks, &[], &[], &[])
        },
        bis: None,
        build: None,
        stage: None,
        farm: &[],
        route: &NO_ROUTE,
        stage_chain: &[],
        not_needed: &NO_MARKS,
    };
    let rows = shop_better_rows(&shop_json(&info));
    assert_eq!(
        rows,
        [
            r#""Long Bow"|true|"+15 fire res, +4 avg dmg""#,
            r#""Shadow Bow"|false|null"#,
            r#""Javelin"|false|null"#,
            r#""Ring"|true|"+30 life""#,
            r#""Ring"|false|null"#,
        ]
    );
    assert!(upgrades_json(&info).is_empty(), "better items don't chime");
}

/// The user's cases (2026-11-22): a summoner druid at 20 FCR, weapon pieces all uniques and
/// runewords. A magic wand, an armor without the FCR breakpoint's FCR and a second Teleport
/// staff aren't worth buying; a staff with more Teleport than the worn swap one is.
#[test]
fn shop_marks_only_what_the_summoner_build_can_use() {
    use crate::gear_score::TELEPORT;
    use d2r_core::item_stats::{ItemStat, stat};
    use d2r_core::shop::shop_item;
    let affix = |id, param, value| ItemStat {
        id,
        param,
        value,
        base: false,
    };
    let worn_item =
        |body: u8, code: &str, name: Option<&str>, stats: Vec<ItemStat>| d2r_core::game::Equipped {
            body,
            quality: if name.is_some() { 7 } else { 4 },
            code: Some(code.to_owned()),
            name: name.map(str::to_owned),
            file_index: None,
            runeword: false,
            stats,
        };
    let teleport = |level: u16| affix(stat::CHARGED_SKILL, (TELEPORT << 6) | level, 20);
    let equipped_items = [
        worn_item(4, "spc", Some("Stoutnail"), vec![affix(stat::VITALITY, 0, 7)]),
        worn_item(3, "qui", None, vec![affix(stat::FCR, 0, 20)]),
        worn_item(11, "bst", None, vec![teleport(1)]),
    ];
    let with = |mut item: d2r_core::shop::ShopItem, stats: Vec<ItemStat>| {
        item.stats = stats;
        item
    };
    let shop = d2r_core::shop::Shop {
        npc_txt: 148,
        npc: Some("Akara".to_owned()),
        items: vec![
            with(
                shop_item(Some("wnd"), 4, 1, 0, 0),
                vec![affix(stat::MAX_MANA, 0, 40), affix(stat::FCR, 0, 10)],
            ),
            with(
                shop_item(Some("lea"), 4, 0, 0, 0),
                vec![affix(stat::MAX_LIFE, 0, 60)],
            ),
            with(shop_item(Some("bst"), 4, 1, 0, 0), vec![teleport(3)]),
            with(shop_item(Some("bst"), 4, 1, 0, 0), vec![teleport(1)]),
        ],
    };
    let bis = crate::config::Bis {
        build: "summoner".to_owned(),
        slots: BTreeMap::from([
            (
                "weapon".to_owned(),
                vec![
                    "Spirit@25".to_owned(),
                    "Stoutnail@5".to_owned(),
                    "Suicide Branch@33".to_owned(),
                ],
            ),
            ("offhand".to_owned(), vec!["Ancients' Pledge@21".to_owned()]),
        ]),
        merc_slots: BTreeMap::new(),
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let info = PanelInfo {
        live: LiveInfo {
            char_level: Some(20),
            difficulty: Some(0),
            area: "Rogue Encampment",
            equipped_items: &equipped_items,
            shop: Some(&shop),
            ..d2r_core::web::fixtures::live_info(5, &blocks, &[], &[], &[])
        },
        bis: Some(&bis),
        ..live_info(5, &blocks, &[], &[], &[])
    };
    let rows = shop_better_rows(&shop_json(&info));
    assert_eq!(
        rows,
        [
            r#""Wand"|false|null"#,
            r#""Leather Armor"|false|null"#,
            r#""Battle Staff"|true|"Teleport 3 (worn 1)""#,
            r#""Battle Staff"|false|null"#,
        ]
    );
}

#[test]
fn score_profile_takes_the_weapon_kind_from_gear_or_build() {
    static CODELESS: [d2r_core::game::Equipped; 1] = [d2r_core::game::Equipped {
        body: 4,
        quality: 2,
        code: None,
        name: None,
        file_index: None,
        runeword: false,
        stats: Vec::new(),
    }];
    let bis = crate::config::Bis {
        build: "amazon leveling".to_owned(),
        slots: BTreeMap::from([(
            "weapon".to_owned(),
            vec![
                "Titan's Revenge@42".to_owned(),
                "Javelin of Swiftness@26".to_owned(),
                "Hunter's Bow".to_owned(),
            ],
        )]),
        merc_slots: BTreeMap::new(),
    };
    let info = |lvl: u8, equipped_items: &'static [d2r_core::game::Equipped]| {
        let profile = score_profile(&PanelInfo {
            live: LiveInfo {
                player_name: "Fulmina",
                class: 0,
                char_level: Some(lvl),
                difficulty: Some(0),
                area: "Rogue Encampment",
                blocks: &d2r_core::render::TextBlocks::default(),
                equipped: &[],
                equipped_items,
                owned_items: &[],
                owned: &[],
                runeword_copies: &[],
                merc_worn: &[],
                stash: &[],
                stash_owned: &[],
                skills: &EMPTY_SKILLS,
                held_materials: &[],
                white_bases: &[],
                quests: &[],
                shop: None,
                strength: None,
                dexterity: None,
                marks: &EMPTY_MARKS,
                updated_at: 1_700_000_000,
            },
            bis: Some(&bis),
            build: None,
            stage: None,
            farm: &[],
            route: &NO_ROUTE,
            stage_chain: &[],
            not_needed: &NO_MARKS,
        });
        (profile.weapon, profile.caster)
    };
    assert_eq!(
        info(12, &[]),
        (Some("bow"), false),
        "the javelin piece needs level 26"
    );
    assert_eq!(info(30, &[]), (Some("jave"), false));
    assert_eq!(
        info(12, &CODELESS),
        (Some("bow"), false),
        "a worn weapon without a code"
    );
}

#[test]
fn upgrades_list_wearable_owned_pieces_and_craftable_runewords() -> Result<()> {
    let build = guide_build();
    let keys = |json: &serde_json::Value| -> Vec<String> {
        json.get("upgrades")
            .and_then(|u| u.as_array())
            .map_or_else(Vec::new, |u| {
                u.iter()
                    .filter_map(|x| x.get("key")?.as_str().map(str::to_owned))
                    .collect()
            })
    };
    let owned = ["Biggin's Bonnet".to_owned(), "Nagelring".to_owned()];
    let json = guide_json(&build, 20, &[], &[], &[], &owned, &[])?;
    assert_eq!(
        keys(&json),
        ["equip:ring:Nagelring", "equip:helm:Biggin's Bonnet"],
        "slot order, one ring entry"
    );
    let worn = [d2r_core::game::Equipped {
        body: 1,
        quality: d2r_core::offsets::item_data::QUALITY_UNIQUE,
        code: Some("bhm".to_owned()),
        name: Some("Biggin's Bonnet".to_owned()),
        file_index: None,
        runeword: false,
        stats: Vec::new(),
    }];
    let json = guide_json(&build, 6, &[], &[], &[], &owned, &worn)?;
    assert_eq!(keys(&json), Vec::<String>::new());
    let json = guide_json(&build, 20, &[], &[], &[], &owned[..1], &[equipped_unique(1)])?;
    assert_eq!(keys(&json), Vec::<String>::new(), "already beaten: worse");
    // Leaf misses Tir; the craft's base is not checked.
    let stash = materials_tab(&[("Tal Rune", "r07", 1), ("Ral Rune", "r08", 1)]);
    let held = [("Eth Rune".to_owned(), 1)];
    let json = guide_json(&build, 17, &stash, &held, &[], &[], &[])?;
    assert_eq!(keys(&json), ["craft:body:Stealth"]);
    assert_eq!(
        json.pointer("/upgrades/0/why").and_then(|w| w.as_str()),
        Some("all runes owned · base: non-magic Hard Leather Armor with exactly 2 sockets")
    );
    let json = guide_json(&build, 16, &stash, &held, &[], &[], &[])?;
    assert_eq!(keys(&json), Vec::<String>::new());
    // The reader names a worn runeword by its socketed runes: no craft, no equip.
    let stealth = [rune_worded(3, "lea", "Stealth")];
    let owned = ["Stealth".to_owned()];
    let json = guide_json(&build, 17, &stash, &held, &[], &owned, &stealth)?;
    assert_eq!(keys(&json), Vec::<String>::new());
    Ok(())
}

#[test]
fn no_equip_upgrade_below_the_worn_piece_of_the_list() -> Result<()> {
    // The Fury's guide alternatives sit below the user's core pick: an owned one with a higher
    // level requirement is no upgrade over a worn piece ranked above it.
    let build = config::Build {
        name: "Fury".to_owned(),
        slots: [(
            "helm".to_owned(),
            vec!["Harlequin Crest@62".to_owned(), "Crown of Ages@82".to_owned()],
        )]
        .into(),
        ..config::Build::default()
    };
    let owned = ["Crown of Ages".to_owned()];
    let json = guide_json(&build, 85, &[], &[], &[], &owned, &[equipped_unique(1)])?;
    assert_eq!(
        json.get("upgrades")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(0),
        "{json}"
    );
    assert_eq!(
        json.pointer("/gear/columns/0/pieces/1/state")
            .and_then(serde_json::Value::as_str),
        Some("worse"),
        "the grid strikes it too"
    );
    // Worn below it, the core pick is still an upgrade.
    let build = config::Build {
        slots: [(
            "helm".to_owned(),
            vec!["Crown of Ages@82".to_owned(), "Harlequin Crest@62".to_owned()],
        )]
        .into(),
        ..build
    };
    let json = guide_json(&build, 85, &[], &[], &[], &owned, &[equipped_unique(1)])?;
    assert_eq!(
        json.pointer("/upgrades/0/key")
            .and_then(serde_json::Value::as_str),
        Some("equip:helm:Crown of Ages")
    );
    Ok(())
}

/// A build with `slot`'s `pieces`, those named in `alternatives` as guide alternatives.
fn build_with_alternatives(slot: &str, pieces: &[&str], alternatives: &[&str]) -> config::Build {
    config::Build {
        name: "Test".to_owned(),
        slots: [(slot.to_owned(), pieces.iter().map(|&p| p.to_owned()).collect())].into(),
        pieces: alternatives
            .iter()
            .map(|&name| {
                let info = config::PieceInfo {
                    setup: "alternative".to_owned(),
                    ..config::PieceInfo::default()
                };
                (name.to_owned(), info)
            })
            .collect(),
        ..config::Build::default()
    }
}

/// An identified unique or set item of the character's with its stats, in the shared stash.
fn owned_named(
    quality: u32,
    index: u32,
    code: &str,
    stats: Vec<d2r_core::item_stats::ItemStat>,
) -> (d2r_core::game::ItemDrop, d2r_core::game::Place) {
    let item = d2r_core::game::ItemDrop {
        cell: None,
        id: index,
        code: Some(code.to_owned()),
        kind: d2r_core::items::DropKind::Base,
        quality,
        flags: d2r_core::offsets::item_data::FLAG_IDENTIFIED,
        ethereal: false,
        sockets: 0,
        unique_index: Some(index),
        stats,
        item_data_head: String::new(),
        x: 0,
        y: 0,
    };
    (item, d2r_core::game::Place::Shared)
}

const fn affix(id: u16, value: i32) -> d2r_core::item_stats::ItemStat {
    d2r_core::item_stats::ItemStat {
        id,
        param: 0,
        value,
        base: false,
    }
}

/// The live JSON of a druid at `level` wearing `equipped`, owning `owned_items` (names in
/// `owned`), on `build`.
fn owned_gear_json(
    build: &config::Build,
    level: u8,
    owned: &[String],
    owned_items: &[(d2r_core::game::ItemDrop, d2r_core::game::Place)],
    equipped: &[d2r_core::game::Equipped],
) -> Result<serde_json::Value> {
    let bis = config::Bis {
        build: build.name.clone(),
        slots: build.slots.clone(),
        merc_slots: BTreeMap::new(),
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.live.char_level = Some(level);
    info.bis = Some(&bis);
    info.build = Some(build);
    info.live.owned = owned;
    info.live.owned_items = owned_items;
    info.live.equipped_items = equipped;
    Ok(serde_json::from_str(&live_json(&info))?)
}

/// Whether grid piece `p` is not a guide alternative (those are never struck for their place).
fn not_alternative(p: &serde_json::Value) -> bool {
    p.get("setup").and_then(serde_json::Value::as_str) != Some("alternative")
}

fn column_states(json: &serde_json::Value, column: usize) -> Vec<(String, String)> {
    json.pointer(&format!("/gear/columns/{column}/pieces"))
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|p| {
            let text = |k: &str| {
                p.get(k)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            (text("name"), text("state"))
        })
        .collect()
}

fn upgrade_keys(json: &serde_json::Value) -> Vec<String> {
    json.get("upgrades")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|u| Some(u.get("key")?.as_str()?.to_owned()))
        .collect()
}

#[test]
fn guide_alternatives_are_judged_by_owned_stats_not_by_list_position() -> Result<()> {
    use d2r_core::item_stats::stat;
    use d2r_core::offsets::item_data::{QUALITY_NORMAL, QUALITY_UNIQUE};
    // The summoner wears Lore; the guide's helms follow the stage's own pieces.
    let build = build_with_alternatives(
        "helm",
        &[
            "Jalal's Mane@42",
            "Lore@27",
            "Druid pelt@3",
            "Harlequin Crest@62",
            "Ravenlore@74",
            "Tarnhelm@15",
        ],
        &["Harlequin Crest", "Ravenlore", "Tarnhelm"],
    );
    let lore = d2r_core::game::Equipped {
        stats: vec![
            affix(stat::ALL_SKILLS, 1),
            affix(stat::ENERGY, 10),
            affix(stat::RES_LIGHTNING, 30),
        ],
        quality: QUALITY_NORMAL,
        ..rune_worded(1, "dr4", "Lore")
    };
    let worn = [lore];
    let json = owned_gear_json(&build, 80, &[], &[], &worn)?;
    let states = |json: &serde_json::Value| {
        column_states(json, 0)
            .into_iter()
            .map(|(n, s)| format!("{n}: {s}"))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        states(&json),
        [
            "Jalal's Mane: better",
            "Lore: neutral",
            "Druid pelt: worse",
            "Harlequin Crest: neutral",
            "Ravenlore: neutral",
            "Tarnhelm: neutral",
        ],
        "unowned alternatives are neither struck nor upgrades"
    );
    assert_eq!(upgrade_keys(&json), Vec::<String>::new());
    // An owned Shako beats Lore; an owned weak Ravenlore is struck.
    let owned = ["Harlequin Crest".to_owned(), "Ravenlore".to_owned()];
    let items = [
        owned_named(
            QUALITY_UNIQUE,
            248,
            "uap",
            vec![
                affix(stat::ALL_SKILLS, 2),
                affix(stat::MAX_LIFE, 100),
                affix(stat::MAX_MANA, 100),
                affix(stat::MAGIC_FIND, 50),
            ],
        ),
        owned_named(QUALITY_UNIQUE, 350, "uap", vec![affix(stat::RES_LIGHTNING, 5)]),
    ];
    let json = owned_gear_json(&build, 80, &owned, &items, &worn)?;
    let shown = states(&json);
    assert!(shown.contains(&"Harlequin Crest: better".to_owned()), "{shown:?}");
    assert!(shown.contains(&"Ravenlore: worse".to_owned()), "{shown:?}");
    assert!(shown.contains(&"Tarnhelm: neutral".to_owned()), "{shown:?}");
    assert_eq!(upgrade_keys(&json), ["equip:helm:Harlequin Crest"]);
    let why = json
        .pointer("/upgrades/0/why")
        .and_then(serde_json::Value::as_str);
    assert!(
        why.is_some_and(|w| w.starts_with("you have it · lvl 62 · ")),
        "{why:?}"
    );
    // Not wearable yet: no upgrade, not struck either.
    let json = owned_gear_json(&build, 60, &owned, &items, &worn)?;
    assert_eq!(upgrade_keys(&json), Vec::<String>::new());
    assert!(states(&json).contains(&"Harlequin Crest: neutral".to_owned()));
    Ok(())
}

#[test]
fn an_owned_alternative_scoring_lower_is_no_upgrade_over_the_worn_core_piece() -> Result<()> {
    use d2r_core::item_stats::stat;
    use d2r_core::offsets::item_data::{QUALITY_SET, QUALITY_UNIQUE};
    // The Fury wears Laying of Hands; Soul Drainer (an alternative ranked below) is owned.
    let build = build_with_alternatives(
        "gloves",
        &["Laying of Hands@63", "Soul Drainer@74"],
        &["Soul Drainer"],
    );
    let laying = d2r_core::game::Equipped {
        body: 10,
        quality: QUALITY_SET,
        code: Some("ulg".to_owned()),
        name: Some("Laying of Hands".to_owned()),
        file_index: Some(96),
        runeword: false,
        stats: vec![affix(stat::IAS, 20), affix(stat::RES_FIRE, 50)],
    };
    let soul_drainer = owned_named(
        QUALITY_UNIQUE,
        312,
        "umg",
        vec![affix(stat::LIFE_LEECH, 7), affix(stat::MANA_LEECH, 7)],
    );
    let owned = ["Soul Drainer".to_owned()];
    let json = owned_gear_json(&build, 80, &owned, &[soul_drainer], &[laying])?;
    assert_eq!(upgrade_keys(&json), Vec::<String>::new(), "{json}");
    assert_eq!(
        column_states(&json, 2),
        [
            ("Laying of Hands".to_owned(), "neutral".to_owned()),
            ("Soul Drainer".to_owned(), "worse".to_owned())
        ]
    );
    Ok(())
}

#[test]
fn ring_column_alternatives_keep_the_level_rule() -> Result<()> {
    use d2r_core::offsets::item_data::QUALITY_UNIQUE;
    let build = build_with_alternatives("ring1", &["Raven Frost@45", "Nagelring@7"], &["Nagelring"]);
    let raven = d2r_core::game::Equipped {
        body: 6,
        quality: QUALITY_UNIQUE,
        code: Some("rin".to_owned()),
        name: Some("Raven Frost".to_owned()),
        file_index: Some(275),
        runeword: false,
        stats: Vec::new(),
    };
    let json = owned_gear_json(&build, 80, &[], &[], &[raven])?;
    let ring = json
        .pointer("/gear/columns")
        .and_then(serde_json::Value::as_array)
        .and_then(|c| {
            c.iter()
                .position(|c| c.get("title").and_then(serde_json::Value::as_str) == Some("Ring"))
        })
        .context("no Ring column")?;
    assert_eq!(
        column_states(&json, ring),
        [
            ("Raven Frost".to_owned(), "neutral".to_owned()),
            ("Nagelring".to_owned(), "worse".to_owned())
        ]
    );
    Ok(())
}

fn rune_worded(body: u8, code: &str, name: &str) -> d2r_core::game::Equipped {
    d2r_core::game::Equipped {
        body,
        quality: d2r_core::offsets::item_data::QUALITY_SUPERIOR,
        code: Some(code.to_owned()),
        name: Some(name.to_owned()),
        file_index: None,
        runeword: true,
        stats: Vec::new(),
    }
}

#[test]
fn worn_check_ignores_case_and_apostrophe_placement() {
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    let worn = [
        rune_worded(5, "buc", "Ancient's Pledge"),
        rune_worded(4, "sst", "Leaf"),
    ];
    info.live.equipped_items = &worn;
    assert!(is_worn("Ancients' Pledge", &info), "the builds' spelling");
    assert!(is_worn("ancient's pledge", &info));
    assert!(is_worn(" Leaf ", &info));
    assert!(!is_worn("Stealth", &info));
    assert!(!is_worn("Lea", &info), "whole names only");
    info.live.equipped_items = &[];
    assert!(!is_worn("Leaf", &info));
}

#[test]
fn stage_json_shows_current_and_next() {
    let leveling = config::Build {
        name: "Leveling".to_owned(),
        next: Some("tornado".to_owned()),
        switch_level: Some(38),
        auto_switch: false,
        switch_note: Some("you have Spirit".to_owned()),
        switch_requires: vec![
            crate::config::SwitchGroup {
                label: "Cannot Be Frozen".to_owned(),
                any: vec!["Raven Frost".to_owned(), "Nosferatu's Coil".to_owned()],
                count: 1,
            },
            crate::config::SwitchGroup {
                label: "Physical immunes".to_owned(),
                any: vec!["Lawbringer".to_owned()],
                count: 2,
            },
            crate::config::SwitchGroup::item("Enigma"),
        ],
        ..config::Build::default()
    };
    let tornado = config::Build {
        name: "Tornado".to_owned(),
        ..config::Build::default()
    };
    let stage = config::Stage {
        slug: "leveling",
        build: &leveling,
        class_default: true,
        next: Some(("tornado", &tornado)),
    };
    let owns = |item: &str| match item {
        "Raven Frost" | "Lawbringer" => 1,
        _ => 0,
    };
    assert_eq!(
        stage_json(Some(&stage), &owns),
        serde_json::json!({
            "slug": "leveling", "name": "Leveling", "class_default": true,
            "next": {
                "slug": "tornado", "name": "Tornado", "level": 38, "auto": false, "note": "you have Spirit",
                "requires": [
                    {
                        "label": "Cannot Be Frozen", "count": 1, "have": 1, "met": true,
                        "any": [{ "name": "Raven Frost", "owned": true }, { "name": "Nosferatu's Coil", "owned": false }],
                    },
                    {
                        "label": "Physical immunes", "count": 2, "have": 1, "met": false,
                        "any": [{ "name": "Lawbringer", "owned": true }],
                    },
                    {
                        "label": "Enigma", "count": 1, "have": 0, "met": false,
                        "any": [{ "name": "Enigma", "owned": false }],
                    },
                ],
            },
        })
    );
    // The switch level doesn't count.
    let owns_all = |item: &str| match item {
        "Raven Frost" | "Enigma" => 1,
        "Lawbringer" => 2,
        _ => 0,
    };
    let all = stage_json(Some(&stage), &owns_all);
    let requires = all
        .pointer("/next/requires")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    assert!(
        !requires.is_empty()
            && requires
                .iter()
                .all(|g| g.get("met") == Some(&serde_json::json!(true))),
        "{all}"
    );
    let last = config::Build {
        next: None,
        ..leveling.clone()
    };
    let stage = config::Stage {
        build: &last,
        next: None,
        ..stage
    };
    assert_eq!(
        stage_json(Some(&stage), &owns).get("next"),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(stage_json(None, &owns), serde_json::Value::Null);
}

fn fury_farm() -> Result<Vec<config::FarmGroup>> {
    let (groups, warnings) = crate::config::parse_farm(
        "[[farm]]\nfor = \"Fury Druid\"\nitems = [\n\
         { name = \"Fortitude\", kind = \"runeword\" },\n\
         { name = \"Raven Frost\", kind = \"unique\" },\n\
         { name = \"Ribcracker\", kind = \"unique\" },\n\
         { name = \"Lawbringer\", kind = \"runeword\", count = 2 },\n\
         { name = \"Smoke\", kind = \"runeword\" },\n\
         { name = \"Bulwark\", kind = \"runeword\" },\n]\n",
    )?;
    assert_eq!(warnings, Vec::<String>::new());
    Ok(groups)
}

/// The farm groups as `(for, [item names])`.
fn farm_groups(json: &serde_json::Value) -> Vec<(String, Vec<String>)> {
    let str_of =
        |v: &serde_json::Value, k: &str| v.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
    json.pointer("/farm/groups")
        .and_then(serde_json::Value::as_array)
        .map_or_default(|groups| {
            groups
                .iter()
                .map(|g| {
                    let items = g
                        .get("items")
                        .and_then(serde_json::Value::as_array)
                        .map_or_default(|i| i.iter().map(|i| str_of(i, "name")).collect());
                    (str_of(g, "for"), items)
                })
                .collect()
        })
}

/// The still-needed entries over every group as `(for, name)`.
fn farm_needed(json: &serde_json::Value) -> Vec<(String, String)> {
    farm_groups(json)
        .into_iter()
        .flat_map(|(target, items)| items.into_iter().map(move |name| (target.clone(), name)))
        .collect()
}

/// Every entry named `name`, in group order.
fn farm_items<'j>(json: &'j serde_json::Value, name: &str) -> Vec<&'j serde_json::Value> {
    json.pointer("/farm/groups")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| g.get("items")?.as_array())
        .flatten()
        .filter(|i| i.get("name") == Some(&serde_json::json!(name)))
        .collect()
}

/// The first entry named `name`.
fn farm_item<'j>(json: &'j serde_json::Value, name: &str) -> Option<&'j serde_json::Value> {
    farm_items(json, name).into_iter().next()
}

/// The where line of the first entry named `name`; `None` without one.
fn farm_where(json: &serde_json::Value, name: &str) -> Option<String> {
    Some(farm_item(json, name)?.get("where")?.as_str()?.to_owned())
}

/// The base hint of the first entry named `name`.
fn farm_hint(json: &serde_json::Value, name: &str) -> Result<String> {
    Ok(farm_item(json, name)
        .and_then(|i| i.get("base_hint")?.as_str())
        .with_context(|| format!("no hint for {name}"))?
        .to_owned())
}

/// The held entries as `(for, name, copies)`.
fn farm_held(json: &serde_json::Value) -> Vec<(String, String, u64)> {
    json.pointer("/farm/held")
        .and_then(serde_json::Value::as_array)
        .map_or_default(|held| {
            held.iter()
                .map(|h| {
                    let s = |k: &str| h.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                    (
                        s("for"),
                        s("name"),
                        h.get("copies").and_then(serde_json::Value::as_u64).unwrap_or(99),
                    )
                })
                .collect()
        })
}

/// The farm section as `(needed (for, name), held (for, name, copies))`.
type FarmRows = (Vec<(String, String)>, Vec<(String, String, u64)>);

fn held_row(target: &str, name: &str, copies: u64) -> (String, String, u64) {
    (target.to_owned(), name.to_owned(), copies)
}

fn shipped_route() -> Result<crate::route::Route> {
    let (route, warnings) =
        crate::route::parse_route(include_str!("../../../../playstyles/summoner-fury/route.toml"))?;
    assert_eq!(warnings, Vec::<String>::new());
    Ok(route)
}

#[test]
fn farm_leaves_held_entries_out_and_says_where_to_farm_the_rest() -> Result<()> {
    use d2r_core::offsets::item_data::{QUALITY_NORMAL, QUALITY_UNIQUE};
    let farm = fury_farm()?;
    let route = shipped_route()?;
    let blocks = d2r_core::render::TextBlocks::default();
    let mut lawbringer = stash_item("Lawbringer", "flc", QUALITY_NORMAL, None);
    lawbringer.runeword = true;
    let mut stash = vec![d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(1),
        items: vec![
            stash_item("Raven Frost", "rin", QUALITY_UNIQUE, None),
            lawbringer,
            d2r_core::stash::StashItem {
                identified: false,
                ..stash_item("Ribcracker", "8cs", QUALITY_UNIQUE, None)
            },
        ],
    }];
    stash.extend(materials_tab(&[("El Rune", "r01", 8), ("Io Rune", "r16", 1)]));
    let owned = ["Ribcracker".to_owned(), "Lawbringer".to_owned()];
    let copies = ["Lawbringer".to_owned()];
    let held = [("Sol Rune".to_owned(), 1)];
    let mut info = live_info(5, &blocks, &[], &stash, &[]);
    info.live.char_level = None;
    info.live.difficulty = Some(2);
    info.farm = &farm;
    info.route = &route;
    info.live.owned = &owned;
    info.live.runeword_copies = &copies;
    info.live.held_materials = &held;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        farm_held(&json),
        [
            held_row("Fury Druid", "Raven Frost", 1),
            held_row("Fury Druid", "Ribcracker", 1),
            held_row("Fury Druid", "Lawbringer", 2),
        ],
        "done or found: off the list"
    );
    assert_eq!(
        farm_needed(&json),
        [
            ("Fury Druid".to_owned(), "Fortitude".to_owned()),
            ("Fury Druid".to_owned(), "Smoke".to_owned()),
            ("Fury Druid".to_owned(), "Bulwark".to_owned()),
        ]
    );
    // Owned runes leave: El ×8 owned, the held Sol covers one copy, Io is in the stash.
    assert_eq!(
        farm_item(&json, "Fortitude").and_then(|i| i.get("runes")),
        Some(&serde_json::json!([
            { "rune": "Dol", "need": 1, "have": 0 }, { "rune": "Lo", "need": 1, "have": 0 },
        ]))
    );
    assert_eq!(
        farm_item(&json, "Fortitude").and_then(|i| i.get("bases")),
        Some(&serde_json::json!(["weapons", "armor"]))
    );
    assert_eq!(
        farm_item(&json, "Fortitude").and_then(|i| i.get("base_hint")),
        Some(&serde_json::Value::Null),
        "unknown character level: no item level to check bases by"
    );
    // Hell: the Countess drops Dol, Nef, Lum and Shael reliably; Lo comes from any level-85
    // area. The runes are grouped by spot, the Countess's with their odds.
    assert_eq!(
        farm_where(&json, "Fortitude").as_deref(),
        Some("Countess: Dol 1 in 13 · Lo: any level-85 area")
    );
    assert_eq!(
        farm_where(&json, "Smoke").as_deref(),
        Some("Countess: Nef 1 in 12, Lum 1 in 28")
    );
    assert_eq!(
        farm_where(&json, "Bulwark").as_deref(),
        Some("Countess: Shael 1 in 9")
    );
    assert_fury_rune_needs(&json)
}

/// The farm runes of [`fury_farm`] with El ×8 and Io in the stash and a Sol held: held runes
/// count as had, but not as in the stash.
fn assert_fury_rune_needs(json: &serde_json::Value) -> Result<()> {
    let runes = json
        .pointer("/farm/runes")
        .and_then(serde_json::Value::as_array)
        .context("farm runes")?;
    let names: Vec<&str> = runes.iter().filter_map(|r| r.get("rune")?.as_str()).collect();
    assert_eq!(
        names,
        ["El", "Nef", "Sol", "Shael", "Dol", "Io", "Lum", "Lo"],
        "rune order"
    );
    let sol = runes
        .iter()
        .find(|r| r.get("rune").and_then(|v| v.as_str()) == Some("Sol"))
        .context("Sol")?;
    assert_eq!(
        sol,
        &serde_json::json!({
            "rune": "Sol", "code": "r12", "need": 2, "have": 1, "stash": 0,
            "for": ["Fortitude", "Bulwark"],
        })
    );
    Ok(())
}

#[test]
fn farm_lists_per_build_and_names_where_each_entry_drops() -> Result<()> {
    // Highlord's Wrath qlvl 73 (any level-85 area); Arachnid Mesh qlvl 87 (Hell Mephisto 87,
    // Baal 99; not Andariel 75); Lawbringer's Amn / Lem / Ko: the Hell Countess.
    let (farm, warnings) = crate::config::parse_farm(
        "[[farm]]\nfor = \"Summoner Druid\"\nitems = [\n\
         { name = \"Teleport Amulet\", kind = \"rolled\", slot = \"amulet\", needs = [\"Teleport\"] },\n\
         { name = \"Arachnid Mesh\", kind = \"unique\" },\n]\n\
         [[farm]]\nfor = \"Fury Druid\"\nitems = [\n\
         { name = \"Teleport Amulet\", kind = \"rolled\", slot = \"amulet\", needs = [\"Teleport\"] },\n\
         { name = \"Highlord's Wrath\", kind = \"unique\" },\n\
         { name = \"Lawbringer\", kind = \"runeword\", count = 2 },\n]\n",
    )?;
    assert_eq!(warnings, Vec::<String>::new());
    let route = shipped_route()?;
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.farm = &farm;
    info.route = &route;
    info.live.difficulty = Some(2);
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    let group = |target: &str, items: &[&str]| {
        (
            target.to_owned(),
            items.iter().map(|&i| i.to_owned()).collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        farm_groups(&json),
        [
            group("Summoner Druid", &["Teleport Amulet", "Arachnid Mesh"]),
            group(
                "Fury Druid",
                &["Teleport Amulet", "Highlord's Wrath", "Lawbringer"]
            ),
        ],
        "per build in farm-list order, an entry of both builds under each"
    );
    let wheres = |json: &serde_json::Value| -> Vec<Option<String>> {
        [
            "Teleport Amulet",
            "Arachnid Mesh",
            "Highlord's Wrath",
            "Lawbringer",
        ]
        .iter()
        .map(|name| farm_where(json, name))
        .collect()
    };
    let some = |s: &str| Some(s.to_owned());
    assert_eq!(
        wheres(&json),
        [
            some("any level-85 area"),
            some("Mephisto, Baal"),
            some("any level-85 area"),
            some("Countess: Amn 1 in 5, Lem 1 in 82, Ko 1 in 43"),
        ],
        "Andariel can't drop the Mesh"
    );
    // Nightmare has no level-85 areas: the bosses carry it; no boss reaches qlvl 87. Ko is
    // only a rare Countess drop there (her item drop: no odds).
    info.live.difficulty = Some(1);
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        wheres(&json),
        [
            None,
            None,
            some("Baal"),
            some("Countess: Amn 1 in 5 · Lem: Mephisto, Baal · Ko: Countess"),
        ]
    );
    // Out of a game: every rune is a level-85 drop and the bosses wait.
    info.live.difficulty = None;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        wheres(&json),
        [
            some("any level-85 area"),
            None,
            some("any level-85 area"),
            some("any level-85 area"),
        ]
    );
    Ok(())
}

#[test]
fn a_rolled_teleport_amulet_is_missing_found_done_and_hidden_with_enigma() -> Result<()> {
    use d2r_core::item_stats::{ItemStat, stat};
    use d2r_core::offsets::item_data::QUALITY_MAGIC;
    let (farm, warnings) = crate::config::parse_farm(
        "[[farm]]\nfor = \"Summoner Druid\"\nitems = [\n\
         { name = \"Teleport Amulet\", kind = \"rolled\", slot = \"amulet\", needs = [\"Teleport\"], note = \"until Enigma\" },\n]\n",
    )?;
    assert_eq!(warnings, Vec::<String>::new());
    let teleport = vec![ItemStat {
        id: stat::CHARGED_SKILL,
        param: (crate::gear_score::TELEPORT << 6) | 1,
        value: 10,
        base: false,
    }];
    let blocks = d2r_core::render::TextBlocks::default();
    let rows = |info: &PanelInfo| -> Result<FarmRows> {
        let json: serde_json::Value = serde_json::from_str(&live_json(info))?;
        Ok((farm_needed(&json), farm_held(&json)))
    };
    let missing = (
        vec![("Summoner Druid".to_owned(), "Teleport Amulet".to_owned())],
        Vec::new(),
    );
    let held = (Vec::new(), vec![held_row("Summoner Druid", "Teleport Amulet", 1)]);
    let ring = d2r_core::stash::StashItem {
        stats: teleport.clone(),
        ..stash_item("Magic Ring", "rin", QUALITY_MAGIC, None)
    };
    let plain = stash_item("Magic Amulet", "amu", QUALITY_MAGIC, None);
    let tabs = [d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(1),
        items: vec![ring, plain],
    }];
    let mut info = live_info(5, &blocks, &[], &tabs, &[]);
    info.farm = &farm;
    assert_eq!(rows(&info)?, missing);
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        farm_where(&json, "Teleport Amulet").as_deref(),
        Some("any level-85 area"),
        "difficulty unknown"
    );
    let entry = farm_item(&json, "Teleport Amulet").context("entry")?;
    assert_eq!(entry.get("kind"), Some(&serde_json::json!("rolled")));
    assert_eq!(entry.get("slot"), Some(&serde_json::json!("amulet")));
    assert_eq!(entry.get("needs"), Some(&serde_json::json!(["Teleport"])));
    assert_eq!(entry.get("note"), Some(&serde_json::json!("until Enigma")));
    let worn = [d2r_core::game::Equipped {
        body: 2,
        quality: QUALITY_MAGIC,
        code: Some("amu".to_owned()),
        name: None,
        file_index: None,
        runeword: false,
        stats: teleport.clone(),
    }];
    info.live.equipped_items = &worn;
    assert_eq!(rows(&info)?, held, "found");
    info.live.equipped_items = &[];
    let carried = [(
        d2r_core::game::ItemDrop {
            cell: None,
            id: 1,
            code: Some("amu".to_owned()),
            kind: d2r_core::items::DropKind::Base,
            quality: QUALITY_MAGIC,
            flags: d2r_core::offsets::item_data::FLAG_IDENTIFIED,
            ethereal: false,
            sockets: 0,
            unique_index: None,
            stats: teleport.clone(),
            item_data_head: String::new(),
            x: 0,
            y: 0,
        },
        d2r_core::game::Place::Inventory,
    )];
    info.live.owned_items = &carried;
    assert_eq!(rows(&info)?, held, "found");
    let stashed = [d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(2),
        items: vec![d2r_core::stash::StashItem {
            stats: teleport,
            ..stash_item("Magic Amulet", "amu", QUALITY_MAGIC, None)
        }],
    }];
    info.live.stash = &stashed;
    assert_eq!(rows(&info)?, held, "done");
    let copies = ["Enigma".to_owned()];
    info.live.runeword_copies = &copies;
    assert_eq!(rows(&info)?, held, "done");
    info.live.owned = &copies;
    assert_eq!(rows(&info)?, (Vec::new(), Vec::new()), "hidden with Enigma");
    Ok(())
}

#[test]
fn farm_counts_split_and_multiply_rune_needs() -> Result<()> {
    let farm = fury_farm()?;
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.farm = &farm;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    let lawbringer = |json: &serde_json::Value| {
        farm_item(json, "Lawbringer")
            .and_then(|i| i.get("runes"))
            .cloned()
    };
    assert_eq!(
        lawbringer(&json),
        Some(serde_json::json!([
            { "rune": "Amn", "need": 2, "have": 0 },
            { "rune": "Lem", "need": 2, "have": 0 },
            { "rune": "Ko", "need": 2, "have": 0 },
        ]))
    );
    let copies = ["Lawbringer".to_owned()];
    info.live.runeword_copies = &copies;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(lawbringer(&json), None, "found: off the list");
    assert_eq!(farm_held(&json), [held_row("Fury Druid", "Lawbringer", 1)]);
    let runes = json.pointer("/farm/runes").map_or_default(ToString::to_string);
    assert!(
        runes.contains("\"Amn\""),
        "the second copy's runes stay marked: {runes}"
    );
    let mut stashed = stash_item("Lawbringer", "flc", 2, None);
    stashed.runeword = true;
    let stash = vec![d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(1),
        items: vec![stashed.clone(), stashed],
    }];
    info.live.stash = &stash;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(farm_held(&json), [held_row("Fury Druid", "Lawbringer", 2)]);
    let runes = json.pointer("/farm/runes").map_or_default(ToString::to_string);
    assert!(!runes.contains("\"Amn\""), "{runes}");
    Ok(())
}

#[test]
fn live_json_sends_every_sell_and_keep_entry() -> Result<()> {
    use crate::stash_plan::{Action, Side, Spot};
    let blocks = d2r_core::render::TextBlocks {
        sell: (0..20)
            .map(|i| advice(&format!("Item {i}"), Spot::Inventory, Action::Sell))
            .collect(),
        keep_trade: (0..7)
            .map(|i| advice(&format!("Rune {i}"), Spot::Page(3), Action::Keep(Side::Trade)))
            .collect(),
        ..d2r_core::render::TextBlocks::default()
    };
    let json: serde_json::Value = serde_json::from_str(&live_json(&live_info(5, &blocks, &[], &[], &[])))?;
    let sell = lines_of(&json, "sell");
    assert_eq!(sell.len(), 20, "one line per entry, no \"+N more\"");
    assert_eq!(sell.last().map(|(t, _)| t.as_str()), Some("Item 19 (inv)"));
    assert!(sell.iter().all(|(_, c)| c == "#afafaf"));
    assert_eq!(lines_of(&json, "keep_trade").len(), 7);
    Ok(())
}

#[test]
fn live_json_sends_the_place_of_each_plan_entry_apart() -> Result<()> {
    use crate::stash_plan::{Action, Side, Spot};
    let mut ring = advice("Rare Ring", Spot::Page(4), Action::Sell);
    ring.reason = Some("no build stats");
    let mut shako = advice("Harlequin Crest", Spot::Cube, Action::Move(Side::Trade));
    shako.count = 2;
    let blocks = d2r_core::render::TextBlocks {
        sell: vec![ring],
        moves: vec![
            advice("Hunter's Bow 4os", Spot::Page(4), Action::Move(Side::Own)),
            shako,
        ],
        keep_own: vec![advice("Spirit", Spot::Inventory, Action::Keep(Side::Own))],
        ..d2r_core::render::TextBlocks::default()
    };
    let json: serde_json::Value = serde_json::from_str(&live_json(&live_info(5, &blocks, &[], &[], &[])))?;
    let fields = |key: &str| -> Vec<serde_json::Value> {
        json.get(key)
            .and_then(|v| v.as_array())
            .map_or_else(Vec::new, |lines| {
                lines
                    .iter()
                    .map(|l| {
                        let pick = |k: &str| l.get(k).cloned().unwrap_or(serde_json::Value::Null);
                        serde_json::json!([
                            pick("text"),
                            pick("name"),
                            pick("count"),
                            pick("place"),
                            pick("target"),
                            pick("wrong_place"),
                            pick("reason"),
                        ])
                    })
                    .collect()
            })
    };
    assert_eq!(
        fields("move"),
        vec![
            serde_json::json!([
                "Hunter's Bow 4os (page 4 → 1–2)",
                "Hunter's Bow 4os",
                1,
                "page 4",
                "1–2",
                true,
                null
            ]),
            serde_json::json!([
                "Harlequin Crest ×2 (cube → 3–4)",
                "Harlequin Crest",
                2,
                "cube",
                "3–4",
                true,
                null
            ]),
        ],
        "a move is in the wrong place: its place goes red, the name doesn't"
    );
    assert_eq!(
        fields("sell"),
        vec![serde_json::json!([
            "Rare Ring (page 4, no build stats)",
            "Rare Ring",
            1,
            "page 4",
            null,
            false,
            "no build stats"
        ])]
    );
    assert_eq!(
        fields("keep_own"),
        vec![serde_json::json!([
            "Spirit (inv)",
            "Spirit",
            1,
            "inv",
            null,
            false,
            null
        ])],
        "an own keeper on the character is where it may be"
    );
    let page = d2r_core::web::page::DASHBOARD_PAGE;
    assert!(page.contains(r#"<span class="wrong">${esc(e.place)}</span>"#));
    assert!(page.contains(".line.run .wrong { color: var(--bad);"));
    assert!(page.contains("Keep · trade (pages 3–4)"));
    Ok(())
}

/// The live JSON's `gear_attention_why`; asserts `gear_attention` agrees with it.
fn attention(
    slots: &[(&str, &[&str])],
    merc: &[(&str, &[&str])],
    level: u8,
    difficulty: u8,
    owned: &[String],
    held: &[(String, u32)],
) -> Result<Option<String>> {
    let to_map = |list: &[(&str, &[&str])]| -> BTreeMap<String, Vec<String>> {
        list.iter()
            .map(|(slot, pieces)| ((*slot).to_owned(), pieces.iter().map(|&p| p.to_owned()).collect()))
            .collect()
    };
    let bis = config::Bis {
        build: "test".to_owned(),
        slots: to_map(slots),
        merc_slots: to_map(merc),
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.live.char_level = Some(level);
    info.live.difficulty = Some(difficulty);
    info.bis = Some(&bis);
    info.live.owned = owned;
    info.live.held_materials = held;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    let why = json
        .get("gear_attention_why")
        .and_then(|w| w.as_str())
        .map(str::to_owned);
    assert_eq!(
        json.get("gear_attention"),
        Some(&serde_json::json!(why.is_some()))
    );
    Ok(why)
}

#[test]
fn gear_attention_for_an_owned_upgrade() -> Result<()> {
    let slots: [(&str, &[&str]); 1] = [("helm", &["Harlequin Crest@62", "Biggin's Bonnet@3"])];
    let owned = ["Biggin's Bonnet".to_owned()];
    assert_eq!(
        attention(&slots, &[], 20, 0, &owned, &[])?.as_deref(),
        Some("▲ upgrade to wear: Biggin's Bonnet"),
        "owned and wearable, whatever the areas"
    );
    assert_eq!(attention(&slots[..], &[], 2, 0, &[], &[])?, None);
    Ok(())
}

#[test]
fn gear_attention_for_a_findable_unique() -> Result<()> {
    // Biggin's Bonnet (War Bonnet, qlvl 4) and Nagelring (qlvl 10).
    let slots: [(&str, &[&str]); 2] = [("ring1", &["Nagelring@7"]), ("helm", &["Biggin's Bonnet@3"])];
    assert_eq!(
        attention(&slots, &[], 10, 0, &[], &[])?.as_deref(),
        Some("▲ findable: Nagelring, Biggin's Bonnet"),
        "level 10 in Normal farms up to monster level 12"
    );
    assert_eq!(
        attention(&slots, &[], 7, 0, &[], &[])?.as_deref(),
        Some("▲ findable: Biggin's Bonnet"),
        "Nagelring's qlvl 10 is above the 9 a level-7 character farms"
    );
    assert_eq!(
        attention(&slots, &[], 6, 0, &[], &[])?.as_deref(),
        Some("▲ findable: Biggin's Bonnet"),
        "Nagelring needs level 7"
    );
    let words: [(&str, &[&str]); 1] = [("helm", &["Druid pelt with +summoning skills@3"])];
    assert_eq!(attention(&words, &[], 10, 0, &[], &[])?, None);
    Ok(())
}

#[test]
fn gear_attention_for_runewords_counts_owned_runes() -> Result<()> {
    // Stealth = Tal (qlvl 17) + Eth (15), Leaf = Tir (13) + Ral (19): a level-14 character
    // farming up to 16 can wear both but finds neither Tal nor Ral.
    let slots: [(&str, &[&str]); 2] = [("body", &["Stealth@14"]), ("weapon", &["Leaf@14"])];
    assert_eq!(
        attention(&slots, &[], 14, 0, &[], &[])?,
        None,
        "Tal and Ral drop only above monster level 16"
    );
    let tal = [("Tal Rune".to_owned(), 1)];
    assert_eq!(
        attention(&slots, &[], 14, 0, &[], &tal)?.as_deref(),
        Some("▲ findable: Stealth"),
        "Tal owned, Eth drops there"
    );
    assert_eq!(
        attention(&slots, &[], 17, 0, &[], &[])?.as_deref(),
        Some("▲ findable: Stealth, Leaf")
    );
    Ok(())
}

#[test]
fn runeword_pieces_and_farm_entries_name_natural_socket_bases() -> Result<()> {
    // Nightmare, level 43: farms up to monster level 45.
    let blocks = d2r_core::render::TextBlocks::default();
    let slot = |s: &str, p: &str| (s.to_owned(), vec![p.to_owned()]);
    let bis = config::Bis {
        build: "test".to_owned(),
        slots: [slot("weapon", "Spirit"), slot("body", "Fortitude")].into(),
        merc_slots: [slot("weapon", "Insight")].into(),
    };
    let farm = fury_farm()?;
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.live.char_level = Some(43);
    info.live.difficulty = Some(1);
    info.bis = Some(&bis);
    info.farm = &farm;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    let hint = |pointer: &str| -> Result<String> {
        Ok(json
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("no hint at {pointer}"))?
            .to_owned())
    };
    let column = |title: &str| {
        json.pointer("/gear/columns")
            .and_then(serde_json::Value::as_array)
            .and_then(|cs| {
                cs.iter()
                    .position(|c| c.get("title") == Some(&serde_json::json!(title)))
            })
            .context("column")
    };
    let spirit = hint(&format!(
        "/gear/columns/{}/pieces/0/base_hint",
        column("Weapons")?
    ))?;
    assert!(
        spirit.starts_with("4 sockets: Crystal Sword (43 Str)"),
        "{spirit}"
    );
    assert!(!spirit.contains("shields"), "{spirit}");
    let fort = hint(&format!("/gear/columns/{}/pieces/0/base_hint", column("Body")?))?;
    assert!(fort.starts_with("4 sockets: "), "{fort}");
    assert!(!fort.contains("Staff"), "{fort}");
    let insight = hint("/gear/merc/0/pieces/0/base_hint")?;
    assert!(
        !insight.contains("Staff") && !insight.contains("Bow") && !insight.contains("staves"),
        "{insight}"
    );
    assert!(insight.starts_with("4 sockets: Voulge (50 Str)"), "{insight}");
    let farm_fort = farm_hint(&json, "Fortitude")?;
    assert!(
        farm_fort.contains("weapons: ") && farm_fort.contains(" · armor: "),
        "{farm_fort}"
    );
    assert!(!format!("{spirit}{fort}{insight}{farm_fort}").contains("Larzuk"));
    Ok(())
}

#[test]
fn farm_merc_and_slot_fields_steer_the_base_hint() -> Result<()> {
    let blocks = d2r_core::render::TextBlocks::default();
    let (farm, warnings) = crate::config::parse_farm(
        "[[farm]]\nfor = \"Fury Druid\"\nitems = [\n\
         { name = \"Fortitude\", kind = \"runeword\", slot = \"body\" },\n\
         { name = \"Bulwark\", kind = \"runeword\", merc = true, slot = \"helm\" },\n\
         { name = \"Bulwark\", kind = \"runeword\" },\n]\n",
    )?;
    assert_eq!(warnings, Vec::<String>::new());
    let bis = config::Bis {
        build: "test".to_owned(),
        slots: BTreeMap::new(),
        merc_slots: [("helm".to_owned(), vec!["Bulwark".to_owned()])].into(),
    };
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.live.char_level = Some(43);
    info.live.difficulty = Some(1);
    info.bis = Some(&bis);
    info.farm = &farm;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    let hint = |pointer: &str| -> Result<String> {
        Ok(json
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("no hint at {pointer}"))?
            .to_owned())
    };
    let fort = farm_hint(&json, "Fortitude")?;
    assert!(fort.starts_with("4 sockets: "), "{fort}");
    assert!(!fort.contains("weapons: ") && !fort.contains("armor: "), "{fort}");
    let bulwarks: Vec<&str> = farm_items(&json, "Bulwark")
        .into_iter()
        .filter_map(|i| i.get("base_hint")?.as_str())
        .collect();
    let [merc, own] = bulwarks.as_slice() else {
        return Err(anyhow!(
            "two Bulwark entries, the mercenary's first: {bulwarks:?}"
        ));
    };
    assert!(
        !merc.contains("Wolf Head") && !merc.contains("Hawk Helm"),
        "{merc}"
    );
    let defs: Vec<u32> = merc
        .split("(def ")
        .skip(1)
        .map(|s| s.split(',').next().unwrap_or_default().parse())
        .collect::<Result<_, _>>()?;
    assert!(defs.len() >= 2, "{merc}");
    assert!(defs.windows(2).all(|w| w.first() >= w.get(1)), "{merc}");
    assert_eq!(hint("/gear/merc/0/pieces/0/base_hint")?, *merc);

    assert!(own.contains("Wolf Head") && !own.contains("(def "), "{own}");
    Ok(())
}

#[test]
fn gear_attention_ignores_the_mercenary() -> Result<()> {
    let merc: [(&str, &[&str]); 1] = [("weapon", &["Insight@27"])];
    let owned = ["Insight".to_owned()];
    assert_eq!(attention(&[], &merc, 60, 2, &[], &[])?, None, "findable");
    assert_eq!(attention(&[], &merc, 60, 2, &owned, &[])?, None, "owned");
    Ok(())
}

fn worn_item(body: u8, name: &str, runeword: bool) -> d2r_core::game::Equipped {
    d2r_core::game::Equipped {
        body,
        quality: if runeword {
            d2r_core::offsets::item_data::QUALITY_SUPERIOR
        } else {
            d2r_core::offsets::item_data::QUALITY_UNIQUE
        },
        code: None,
        name: Some(name.to_owned()),
        file_index: None,
        runeword,
        stats: Vec::new(),
    }
}

#[test]
fn farm_hides_entries_worn_for_the_characters_own_build() -> Result<()> {
    let (mut farm, _) = crate::config::parse_farm(
        "[[farm]]\nfor = \"Summoner Druid\"\nitems = [{ name = \"Spirit\", kind = \"runeword\" }]\n",
    )?;
    farm.extend(fury_farm()?);
    let blocks = d2r_core::render::TextBlocks::default();
    // Every entry still listed, needed or held, as sorted "build|name".
    let names = |json: &serde_json::Value| -> Vec<String> {
        let needed = farm_needed(json)
            .into_iter()
            .map(|(b, name)| format!("{b}|{name}"));
        let held = farm_held(json)
            .into_iter()
            .map(|(b, name, _)| format!("{b}|{name}"));
        let mut out: Vec<String> = needed.chain(held).collect();
        out.sort();
        out.dedup();
        out
    };
    let listed = |entries: &[(&str, &[&str])]| -> Vec<String> {
        let mut out: Vec<String> = entries
            .iter()
            .flat_map(|(build, names)| names.iter().map(move |n| format!("{build}|{n}")))
            .collect();
        out.sort();
        out
    };
    let fury_all: &[&str] = &[
        "Fortitude",
        "Raven Frost",
        "Ribcracker",
        "Lawbringer",
        "Smoke",
        "Bulwark",
    ];
    let fury_rest: &[&str] = &["Fortitude", "Ribcracker", "Lawbringer", "Smoke", "Bulwark"];
    let worn = [
        worn_item(4, "Spirit", true),
        worn_item(6, "Raven Frost", false),
        worn_item(5, "Lawbringer", true),
    ];
    let summoner_chain = ["Summoner Druid".to_owned(), "Fury Druid".to_owned()];
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.farm = &farm;
    info.live.equipped_items = &worn;
    info.stage_chain = &summoner_chain;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    // Spirit worn for the current build and Raven Frost for the later stage: hidden; one
    // Lawbringer of two: listed.
    assert_eq!(names(&json), listed(&[("Fury Druid", fury_rest)]));
    let other_chain = ["Other Build".to_owned()];
    info.stage_chain = &other_chain;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        names(&json),
        listed(&[("Summoner Druid", &["Spirit"]), ("Fury Druid", fury_all)])
    );
    let owned = ["Raven Frost".to_owned()];
    info.stage_chain = &summoner_chain;
    info.live.equipped_items = &[];
    info.live.owned = &owned;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        names(&json),
        listed(&[("Summoner Druid", &["Spirit"]), ("Fury Druid", fury_all)])
    );
    let merc = ["Raven Frost".to_owned(), "Spirit".to_owned()];
    info.live.owned = &[];
    info.live.merc_worn = &merc;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(names(&json), listed(&[("Fury Druid", fury_rest)]));
    // In a group game `merc_worn` is empty.
    info.live.merc_worn = &[];
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        names(&json),
        listed(&[("Summoner Druid", &["Spirit"]), ("Fury Druid", fury_all)])
    );
    Ok(())
}

/// Applies `file`'s marks to `builds` and `farm` as the load does.
fn mark(
    file: &[(&str, &[&str])],
    builds: &mut BTreeMap<String, config::Build>,
    farm: &mut [config::FarmGroup],
) -> crate::not_needed::Marks {
    let file: crate::not_needed::NotNeeded = file
        .iter()
        .map(|(build, names)| ((*build).to_owned(), names.iter().map(|&n| n.to_owned()).collect()))
        .collect();
    let mut marks = crate::not_needed::marks(&file, builds);
    crate::not_needed::apply(&mut marks, builds, farm);
    marks
}

fn marked_json(names: &[&str], build: &str) -> serde_json::Value {
    names
        .iter()
        .map(|n| serde_json::json!({ "name": n, "build": build }))
        .collect()
}

#[test]
fn farm_leaves_out_pieces_marked_not_needed_and_lists_them_for_the_undo() -> Result<()> {
    let mut farm = fury_farm()?;
    let marks = mark(
        &[("fury druid", &["Raven Frost", "Bulwark"])],
        &mut BTreeMap::new(),
        &mut farm,
    );
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.farm = &farm;
    info.not_needed = &marks;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    let names: Vec<String> = farm_needed(&json).into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["Fortitude", "Ribcracker", "Lawbringer", "Smoke"]);
    assert_eq!(
        json.pointer("/farm/groups/0/not_needed"),
        Some(&marked_json(&["Raven Frost", "Bulwark"], "fury druid"))
    );
    let rune_for: Vec<&str> = json
        .pointer("/farm/runes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|r| {
            r.get("for")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(!rune_for.contains(&"Bulwark"), "no runes for it: {rune_for:?}");
    // Every entry marked: the group stays, empty, so the undo is still there.
    let all: Vec<String> = fury_farm()?
        .iter()
        .flat_map(|g| g.items.iter().map(|i| i.name.clone()))
        .collect();
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let mut farm = fury_farm()?;
    let marks = mark(&[("fury druid", &all)], &mut BTreeMap::new(), &mut farm);
    info.farm = &farm;
    info.not_needed = &marks;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        farm_groups(&json),
        [("Fury Druid".to_owned(), Vec::<String>::new())]
    );
    Ok(())
}

#[test]
fn gear_grid_and_upgrades_leave_out_pieces_marked_not_needed() -> Result<()> {
    let mut build = crate::config::Build {
        name: "Fury Druid".to_owned(),
        slots: [(
            "helm".to_owned(),
            vec!["Jalal's Mane@42".to_owned(), "Harlequin Crest@62".to_owned()],
        )]
        .into(),
        ..crate::config::Build::default()
    };
    build
        .extras
        .insert("charms".to_owned(), vec!["Annihilus@70".to_owned()]);
    let mut builds = BTreeMap::from([("fury druid".to_owned(), build)]);
    let marks = mark(
        &[("fury druid", &["Harlequin Crest", "Annihilus"])],
        &mut builds,
        &mut [],
    );
    let build = builds.get("fury druid").context("the build stays")?;
    let bis = crate::config::Bis {
        build: build.name.clone(),
        slots: build.slots.clone(),
        merc_slots: build.merc_slots.clone(),
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.bis = Some(&bis);
    info.build = Some(build);
    info.not_needed = &marks;
    // An owned Harlequin Crest would be an upgrade to wear; marked, it isn't.
    let owned = ["Harlequin Crest".to_owned()];
    info.live.owned = &owned;
    info.live.char_level = Some(70);
    let gear = gear_json(&info);
    let helm: Vec<_> = gear
        .pointer("/columns/0/pieces")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("name")?.as_str())
        .collect();
    assert_eq!(helm, ["Jalal's Mane"]);
    assert_eq!(gear.get("extras"), Some(&serde_json::json!([])));
    assert_eq!(
        gear.get("not_needed"),
        Some(&marked_json(&["Harlequin Crest", "Annihilus"], "Fury Druid"))
    );
    let upgrades = upgrades_json(&info);
    assert!(upgrades.is_empty(), "{upgrades:?}");
    let unmarked = crate::config::Bis {
        slots: [("helm".to_owned(), vec!["Harlequin Crest@62".to_owned()])].into(),
        ..bis.clone()
    };
    info.bis = Some(&unmarked);
    assert_eq!(upgrades_json(&info).len(), 1, "the check above can fail");
    Ok(())
}

#[test]
fn a_characters_own_bis_slots_take_its_marks_too() -> Result<()> {
    let (mut config, warnings) =
        settings("[bis.Vex]\nslots = { helm = [\"Harlequin Crest@62\", \"Jalal's Mane@42\"] }\n")?;
    assert!(warnings.is_empty(), "{warnings:?}");
    config.not_needed = mark(&[("vex", &["Harlequin Crest"])], &mut BTreeMap::new(), &mut []);
    let bis = config
        .resolved_bis("Vex", Some(5), Some(70), &config::owns_nothing, None)
        .context("inline slots")?;
    assert_eq!(bis.build, "Vex", "the ✕ writes under the character");
    assert_eq!(bis.slots.get("helm"), Some(&vec!["Jalal's Mane@42".to_owned()]));
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.bis = Some(&bis);
    info.not_needed = &config.not_needed;
    assert_eq!(
        gear_json(&info).get("not_needed"),
        Some(&marked_json(&["Harlequin Crest"], "vex"))
    );
    Ok(())
}

#[test]
fn farm_status_rule() {
    assert_eq!(farm_status(1, 1, 0), FarmStatus::Done);
    assert_eq!(farm_status(1, 2, 1), FarmStatus::Done, "extra copies stay done");
    assert_eq!(farm_status(2, 1, 1), FarmStatus::Found);
    assert_eq!(
        farm_status(2, 1, 0),
        FarmStatus::Missing,
        "stashed short, none carried"
    );
    assert_eq!(farm_status(1, 0, 0), FarmStatus::Missing);
}

#[test]
fn no_farm_list_is_null() -> Result<()> {
    let blocks = d2r_core::render::TextBlocks::default();
    let info = live_info(5, &blocks, &[], &[], &[]);
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(json.get("farm"), Some(&serde_json::Value::Null));
    Ok(())
}

/// The summoner's merc (`merc_acts = [2, 1]`): Insight's hint names Act 2 polearms,
/// Harmony's the Act 1 rogue's bows.
#[test]
fn merc_hints_are_per_act() -> Result<()> {
    let config = crate::files::load(&d2r_core::playstyle::Files {
        shipped: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../playstyles/summoner-fury"),
        ),
        quiet: true,
        ..d2r_core::playstyle::Files::default()
    });
    let build = config
        .build_defs
        .get("summoner druid")
        .context("shipped summoner build")?;
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.live.char_level = Some(57);
    info.live.difficulty = Some(1);
    info.build = Some(build);
    let weapon = build.merc_slots.get("weapon").context("merc weapon")?;
    let pieces = grid_pieces(
        weapon,
        None,
        &[],
        (Some(4), Wearer::Merc(build.merc_act())),
        &info,
    );
    let hint = |name: &str| {
        pieces
            .iter()
            .find(|p| p.get("name") == Some(&serde_json::json!(name)))
            .and_then(|p| p.get("base_hint")?.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    assert!(
        hint("Insight").starts_with("4 sockets: Voulge"),
        "{}",
        hint("Insight")
    );
    assert!(
        hint("Harmony").starts_with("Act 1 merc: 4 sockets: Hunter's Bow"),
        "{}",
        hint("Harmony")
    );
    Ok(())
}

/// A level-57 druid whose merc wears Insight, Smoke and Bulwark: each is ticked (Insight past
/// its window), the pieces below are beaten, Smoke drops its rune note, nothing reaches upgrades.
#[test]
fn merc_columns_follow_the_mercs_worn_gear() -> Result<()> {
    let config = crate::files::load(&d2r_core::playstyle::Files {
        shipped: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../playstyles/summoner-fury"),
        ),
        quiet: true,
        ..d2r_core::playstyle::Files::default()
    });
    let build = config
        .build_defs
        .get("summoner druid")
        .context("shipped summoner build")?;
    let bis = config::Bis {
        build: build.name.clone(),
        slots: build.slots.clone(),
        merc_slots: build.merc_slots.clone(),
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let stash_owned = ["Soul Harvest".to_owned(), "The Battlebranch".to_owned()];
    let merc = ["Bulwark".to_owned(), "Insight".to_owned(), "Smoke".to_owned()];
    let mut info = live_info(5, &blocks, &[], &[], &stash_owned);
    info.live.char_level = Some(57);
    info.live.difficulty = Some(1);
    info.live.merc_worn = &merc;
    info.bis = Some(&bis);
    info.build = Some(build);
    let column = |gear: &serde_json::Value, title: &str| -> Vec<serde_json::Value> {
        gear.get("merc")
            .and_then(serde_json::Value::as_array)
            .and_then(|cols| {
                cols.iter()
                    .find(|c| c.get("title") == Some(&serde_json::json!(title)))
            })
            .and_then(|c| c.get("pieces")?.as_array().cloned())
            .unwrap_or_default()
    };
    let field = |p: &serde_json::Value, key: &str| p.get(key).cloned().unwrap_or(serde_json::Value::Null);
    let at = |v: &[serde_json::Value], i: usize| v.get(i).cloned().unwrap_or(serde_json::Value::Null);
    let gear = gear_json(&info);

    let names = |col: &[serde_json::Value]| col.iter().map(|p| field(p, "name")).collect::<Vec<_>>();
    let weapon = column(&gear, "Merc weapon");
    assert_eq!(
        names(&weapon),
        [
            "Insight",
            "Soul Harvest",
            "The Battlebranch",
            "The Reaper's Toll",
            "Hustle (weapon)",
            "Harmony"
        ],
        "{weapon:?}"
    );
    assert_eq!(field(&at(&weapon, 0), "tick"), "worn");
    for beaten in weapon.iter().skip(1).filter(|p| not_alternative(p)) {
        assert_eq!(field(beaten, "state"), "worse", "{beaten}");
        assert_eq!(field(beaten, "now"), serde_json::Value::Null, "{beaten}");
    }

    let body = column(&gear, "Merc body");
    assert_eq!(names(&body), ["Smoke", "Fortitude", "Treachery"], "{body:?}");
    assert_eq!(field(&at(&body, 0), "tick"), "worn");
    assert_eq!(
        field(&at(&body, 0), "notes"),
        serde_json::Value::Null,
        "no missing-rune note"
    );
    assert_eq!(field(&at(&body, 0), "now"), serde_json::Value::Null);

    let helm = column(&gear, "Merc helm");
    assert_eq!(names(&helm), ["Bulwark", "Guillaume's Face"], "{helm:?}");
    assert_eq!(field(&at(&helm, 0), "tick"), "worn");

    let ups = upgrades_json(&info);
    assert!(
        !ups.iter()
            .any(|u| field(u, "slot").as_str().is_some_and(|s| s.starts_with("merc"))),
        "{ups:?}"
    );
    assert!(
        !ups.iter().any(
            |u| ["Insight", "Soul Harvest", "The Battlebranch", "Smoke", "Bulwark"]
                .contains(&field(u, "name").as_str().unwrap_or_default())
        ),
        "{ups:?}"
    );
    assert!(
        super::gear::gear_attention(&info, &ups).is_none_or(|why| ![
            "Soul Harvest",
            "The Battlebranch",
            "Insight"
        ]
        .iter()
        .any(|n| why.contains(n))),
        "attention"
    );
    let weapons = bis.merc_slots.get("weapon").context("merc weapon slot")?;
    assert_eq!(super::gear::merc_open(weapons, &info), &[] as &[String]);

    // In a group game the merc's gear is unread.
    info.live.merc_worn = &[];
    let gear = gear_json(&info);
    let weapon = column(&gear, "Merc weapon");
    assert!(weapon.iter().all(|p| field(p, "tick") != "worn"), "{weapon:?}");
    assert_eq!(super::gear::merc_open(weapons, &info).len(), weapons.len());
    Ok(())
}

#[test]
fn merc_guide_alternatives_are_not_struck_below_the_worn_piece() {
    let build = config::Build {
        merc_slots: [(
            "weapon".to_owned(),
            vec![
                "Insight@27".to_owned(),
                "Harmony@39".to_owned(),
                "Soul Harvest@19".to_owned(),
            ],
        )]
        .into(),
        ..build_with_alternatives("helm", &[], &["Harmony"])
    };
    let bis = config::Bis {
        build: build.name.clone(),
        slots: build.slots.clone(),
        merc_slots: build.merc_slots.clone(),
    };
    let blocks = d2r_core::render::TextBlocks::default();
    let merc = ["Insight".to_owned()];
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.live.char_level = Some(57);
    info.live.merc_worn = &merc;
    info.bis = Some(&bis);
    info.build = Some(&build);
    let gear = gear_json(&info);
    let states: Vec<_> = gear
        .pointer("/merc/0/pieces")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|p| p.get("state")?.as_str())
        .collect();
    // Insight is worn: never struck; Harmony was struck before.
    assert_eq!(states, ["neutral", "neutral", "worse"], "{gear}");
}

/// The shipped playstyle (`playstyles/summoner-fury`).
fn shipped_settings() -> crate::config::Settings {
    crate::files::load(&d2r_core::playstyle::Files {
        shipped: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../playstyles/summoner-fury"),
        ),
        quiet: true,
        ..d2r_core::playstyle::Files::default()
    })
}

#[test]
fn gear_lists_the_charm_wishlist_with_what_is_found_and_the_next_stage_s_wishes() -> Result<()> {
    use crate::charms::tests::{magic, unique};
    use d2r_core::item_stats::stat;
    let config = shipped_settings();
    let build = |slug: &str| config.build_defs.get(slug).ok_or_else(|| anyhow!("no {slug}"));
    let (summoner, fury) = (build("summoner druid")?, build("fury druid")?);
    let stage = config::Stage {
        slug: "summoner druid",
        build: summoner,
        class_default: true,
        next: Some(("fury druid", fury)),
    };
    let summoning = || magic("cm3", &[(stat::SKILL_TAB, 5 * 8, 1)]);
    let owned = [
        (unique("cm1", 381, &[]), d2r_core::game::Place::Inventory),
        (summoning(), d2r_core::game::Place::Inventory),
        (summoning(), d2r_core::game::Place::Cube),
        // The live shared stash items carry no page: the pages below count instead.
        (summoning(), d2r_core::game::Place::Shared),
    ];
    let stash = [d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(3),
        items: vec![
            d2r_core::stash::stash_item(&magic("cm3", &[(stat::SKILL_TAB, 5 * 8 + 1, 1)]), None),
            // A Latent Bone Break (the Sunder as it drops) is the wished Bone Break.
            d2r_core::stash::stash_item(&unique("cm3", 431, &[]), None),
        ],
    }];
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &stash, &[]);
    info.live.owned_items = &owned;
    info.build = Some(summoner);
    info.stage = Some(stage);
    let gear = gear_json(&info);
    let rows: Vec<(String, u64, u64, Option<String>)> = gear
        .get("charms")
        .and_then(serde_json::Value::as_array)
        .context("charms")?
        .iter()
        .map(|c| {
            let text = |k: &str| c.get(k).and_then(serde_json::Value::as_str).map(str::to_owned);
            let num = |k: &str| c.get(k).and_then(serde_json::Value::as_u64).unwrap_or(99);
            (
                text("name").unwrap_or_default(),
                num("have"),
                num("count"),
                text("stage"),
            )
        })
        .collect();
    let row = |name: &str, have, count, stage: Option<&str>| {
        (name.to_owned(), have, count, stage.map(str::to_owned))
    };
    assert_eq!(
        rows,
        [
            row("Annihilus", 1, 1, None),
            row("Hellfire Torch", 0, 1, None),
            row("Gheed's Fortune", 0, 1, None),
            row("Bone Break", 1, 1, None),
            row("Summoning skiller", 2, 7, None),
            row("Life small charm", 0, 2, None),
            row("Resistance small charm", 0, 2, None),
            row("FHR small charm", 0, 1, None),
            row("Mana small charm", 0, 1, None),
            row("Shape Shifting skiller", 1, 6, Some("Fury Druid")),
            row("10/60 grand charm", 0, 1, Some("Fury Druid")),
            row("Max damage / AR / life small charm", 0, 2, Some("Fury Druid")),
        ]
    );
    let at = |path: &str| gear.pointer(path).cloned().unwrap_or_default();
    assert_eq!(at("/charms/0/base"), "Small Charm", "the unique's piece details");
    assert!(at("/charms/0/setup").is_null(), "no alternative fold");
    assert_eq!(at("/charms/4/size"), "Grand Charm");
    assert_eq!(at("/charms/4/need"), serde_json::json!(["+1 Summoning skills"]));
    assert_eq!(at("/charms/1/need"), serde_json::json!(["+3 class skills"]));
    assert!(
        gear.get("extras")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty),
        "the unique charms are no extras: {:?}",
        gear.get("extras")
    );
    Ok(())
}
