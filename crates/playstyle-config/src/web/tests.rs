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

/// The core fixture ([`d2r_core::web::fixtures::live_info`]) without a build, stage or farm list.
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
        stage_chain: &[],
        next: None,
    }
}

/// The whole dashboard JSON: the core panels plus this playstyle's ([`panels`]).
fn live_json(info: &PanelInfo) -> String {
    d2r_core::web::live_json(&info.live, panels(info))
}

/// The vendor panel with this playstyle's notes ([`gear::shop_notes`]).
fn shop_json(info: &PanelInfo) -> serde_json::Value {
    d2r_core::web::vendor::shop_json(&info.live, &gear::shop_notes(info))
}

/// The settings of a `bis.toml` / old `config.toml` text, with its warnings.
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
    // A bare value, and a non-table entry: the save would silently do nothing, so the
    // caller must get `None` and answer the browser with an error instead.
    assert_eq!(set_bis_selection("bis = \"oops\"\n", "Hork", Some("b")), None);
    assert_eq!(set_bis_selection("[bis]\nHork = 5\n", "Hork", Some("b")), None);
}

/// A config with one build definition and a selection-only character, plus a builds-only
/// character, for the resolved-view tests.
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

/// A played entry keyed by its lowercased name.
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
    // A `[builds]`-only character, and one in both tables (listed once).
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
    // The resolved view: the selection-only character gets the build's display name,
    // damages and annotated pieces; a builds-only character rides along for its select.
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
    // The piece carries its level, tick and runeword info.
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
    // The live gear state annotates the pieces: owned ✓, stash ✓(stash), and a set/unique
    // equipped piece strikes the slot's chase pieces through.
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
    // Worn beats owned (owned_names include the worn pieces): the green ✓.
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

/// The gear-grid JSON of one column, by title.
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
    // 10 columns in the user's two rows of five.
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
    // Helm: a magic equipped piece is unnameable but comparable — the base level 43
    // strikes the below pieces; the 71 one is better but above the 47 gate.
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
    // Amulet: the unique equipped piece is named with its exact level; a below piece
    // is worse, one above (and inside the gate) is better.
    let amulet = column_of(&gear, "Amulet");
    assert_eq!(
        amulet.get("equipped").cloned().unwrap_or_default(),
        serde_json::json!([{ "name": "Harlequin Crest", "level": 62 }])
    );
    // 62 ≤ 62 → worse even though the piece is the same item.
    assert_eq!(piece_states(amulet).first().map(|p| p.2.as_str()), Some("worse"),);
    // Weapons: the equipped runeword is uncomparable, so nothing is struck through
    // (and the page's hint says so); the 55 piece would be better but sits above the
    // 47 gate — too_high; the owned piece is ticked.
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
    // Off-hand: nothing equipped, so the piece is better; no two-handed note (the
    // equipped Crystal Sword is one-handed).
    let offhand = column_of(&gear, "Off-Hand");
    assert_eq!(
        piece_states(offhand).first().map(|p| p.2.as_str()),
        Some("better"),
    );
    assert_eq!(offhand.get("note"), Some(&serde_json::Value::Null));
}

#[test]
fn gear_grid_merges_the_ring_slots() {
    // Two equipped rings: the unique Dwarf Star (row 274, lvl req 45) and a magic one;
    // the merged column compares against the higher of the two requirements.
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
    // Both equipped rings in the header, deduplicated pieces from both slots in order.
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
    // Equipped base: the stronger ring's level (45); the 46 piece is better (the 47
    // gate still holds at clvl 42), the rest worse.
    let states: Vec<&str> = ring.get("pieces").map_or_default(|v| {
        v.as_array()
            .iter()
            .flat_map(|pieces| pieces.iter())
            .filter_map(|x| x.get("state").and_then(|val| val.as_str()))
            .collect()
    });
    assert_eq!(states, ["worse", "worse", "better"]);
}

#[test]
fn gear_grid_notes_two_handed_weapons() {
    // An equipped Elder Staff (code 6cs, weapons.txt `2handed` 1): the off-hand is
    // unusable, and the note says so. With the one-handed Crystal Sword: no note.
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

/// A gear grid over a one-slot build, one equipped piece and one owned name; the
/// pieces' per-column state is what the tests assert on.
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
        stage_chain: &[],
        next: None,
    };
    gear_json(&info)
}

/// A unique equipped piece: Harlequin Crest's live-verified row (248 → lvl req 62).
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

/// One item-plan entry, no reason.
fn advice(
    name: &str,
    spot: crate::stash_plan::Spot,
    action: crate::stash_plan::Action,
) -> crate::stash_plan::Advice {
    crate::stash_plan::Advice {
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
    // Each block becomes `{ "text", "colour" }` lines in the block's overlay colour.
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
    // A runeword piece carries its runes and bases for the dashboard's hint line; a
    // non-runeword piece carries neither field.
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

/// A leveling build: Stealth (body, 17) and Leaf (weapon, 19) runewords, a Tal → Ral and
/// an Ort → Thul upgrade, a cube recipe; one unique per slot for the equip upgrades.
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

/// The live JSON of a level-`level` character with `build`, the materials tab `stash`,
/// `held` materials and `bases`, `owned` names and `equipped_items`.
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
    // Tal in the shared materials tab, Eth and a chipped gem held in the inventory, a
    // white 2os Hard Leather Armor in the cube.
    let stash = materials_tab(&[("Tal Rune", "r07", 1), ("Ral Rune", "r08", 1)]);
    let held = [("Eth Rune".to_owned(), 1), ("Chipped Ruby".to_owned(), 2)];
    let bases = ["Hard Leather Armor 2os".to_owned()];
    let json = guide_json(&build, 18, &stash, &held, &bases, &[], &[])?;
    let recipes = json
        .get("recipes")
        .and_then(|r| r.as_array())
        .context("recipes")?;
    let names: Vec<&str> = recipes.iter().filter_map(|r| r.get("name")?.as_str()).collect();
    // Tir is missing for Leaf, but no upgrade makes Tir; Thul is needed nowhere, Ral is
    // owned: both rune upgrades stay out. The Imbue isn't a rune or gem: always shown.
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

/// The recipe names `live_json` lists for `build` with these runeword copies held and
/// these runewords in a shared-stash page.
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
    // Held (worn, carried, cubed, private stash or on the merc), any case: Stealth goes,
    // Leaf stays.
    let got = recipe_names(&build, &["stealth".to_owned()], &[])?;
    assert!(
        !got.contains(&"Stealth".to_owned()) && got.contains(&"Leaf".to_owned()),
        "{got:?}"
    );
    // In the shared stash counts too.
    let got = recipe_names(&build, &[], &["Leaf"])?;
    assert!(
        !got.contains(&"Leaf".to_owned()) && got.contains(&"Stealth".to_owned()),
        "{got:?}"
    );
    // Only runewords: an owned item named like another kind's recipe keeps it.
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
    // Stealth wanted twice: the body slot and a merc body slot.
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
    // Needed 2, owned 1: shown.
    assert!(stealth(&["Stealth".to_owned()], &[])?);
    assert!(stealth(&[], &["Stealth"])?);
    // Owned 2, held and stashed in any mix: hidden.
    assert!(!stealth(&["Stealth".to_owned(), "Stealth".to_owned()], &[])?);
    assert!(!stealth(&["Stealth".to_owned()], &["Stealth"])?);
    // Another runeword's copy doesn't count.
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
    // Unread flags: every quest recipe stays, unmarked.
    let null = serde_json::Value::Null;
    assert_eq!(
        quest_view(&[])?,
        [
            ("Imbue".to_owned(), null.clone()),
            ("Larzuk".to_owned(), null.clone()),
            ("Respec".to_owned(), null)
        ]
    );
    // Elemental's live record: imbue used (gone), Act 5 not reached, respec pending.
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

/// The druid-leveling build's slots (pieces as in the build file) and a trade window
/// with two of its pieces, one non-matching item, two identical potions.
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

/// A level-12 bow amazon wearing a Short Bow and one fire ring: the store's Long Bow and
/// a better ring are marked with why; a javelin (not her weapon kind), a ring below the
/// bar and a bow above her level are not; nothing joins the upgrade list.
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
        stage_chain: &[],
        next: None,
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

/// The weapon kind: the worn weapon's, else the best stage weapon piece wearable now.
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
            stage_chain: &[],
            next: None,
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
    // Biggin's Bonnet (helm, 3) and Nagelring (rings, 7) are owned; level 20.
    let owned = ["Biggin's Bonnet".to_owned(), "Nagelring".to_owned()];
    let json = guide_json(&build, 20, &[], &[], &[], &owned, &[])?;
    assert_eq!(
        keys(&json),
        ["equip:ring:Nagelring", "equip:helm:Biggin's Bonnet"],
        "slot order, one ring entry"
    );
    // Too low a level to wear Nagelring; Biggin's already equipped: neither.
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
    // An equipped unique of a higher level than the piece (Harlequin Crest, 62) beats it.
    let json = guide_json(&build, 20, &[], &[], &[], &owned[..1], &[equipped_unique(1)])?;
    assert_eq!(keys(&json), Vec::<String>::new(), "already beaten: worse");
    // Craft: Stealth's runes owned at 17+ (the base is not checked); Leaf misses Tir.
    let stash = materials_tab(&[("Tal Rune", "r07", 1), ("Ral Rune", "r08", 1)]);
    let held = [("Eth Rune".to_owned(), 1)];
    let json = guide_json(&build, 17, &stash, &held, &[], &[], &[])?;
    assert_eq!(keys(&json), ["craft:body:Stealth"]);
    assert_eq!(
        json.pointer("/upgrades/0/why").and_then(|w| w.as_str()),
        Some("all runes owned · base: non-magic Hard Leather Armor with exactly 2 sockets")
    );
    // Below the runeword's level: not yet.
    let json = guide_json(&build, 16, &stash, &held, &[], &[], &[])?;
    assert_eq!(keys(&json), Vec::<String>::new());
    // Stealth worn already (the reader names a runeword by its socketed runes): no
    // craft and no equip, even with its runes owned and the piece ticked as owned.
    let stealth = [rune_worded(3, "lea", "Stealth")];
    let owned = ["Stealth".to_owned()];
    let json = guide_json(&build, 17, &stash, &held, &[], &owned, &stealth)?;
    assert_eq!(keys(&json), Vec::<String>::new());
    Ok(())
}

/// A named runeword on a superior base `code`, equipped in `body`.
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
                "ready": false,
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
    // Every group met: ready to switch (the level doesn't count).
    let owns_all = |item: &str| match item {
        "Raven Frost" | "Enigma" => 1,
        "Lawbringer" => 2,
        _ => 0,
    };
    assert_eq!(
        stage_json(Some(&stage), &owns_all).pointer("/next/ready"),
        Some(&serde_json::json!(true))
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

/// The Fury Druid farm list of `farm.toml`, parsed like the real file.
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

/// The farm section's items as `(name, status, stashed, carried)`.
fn farm_rows(json: &serde_json::Value) -> Vec<(String, String, u64, u64)> {
    json.pointer("/farm/groups/0/items")
        .and_then(serde_json::Value::as_array)
        .map_or_default(|items| {
            items
                .iter()
                .map(|i| {
                    let s = |k: &str| i.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
                    let n = |k: &str| i.get(k).and_then(serde_json::Value::as_u64).unwrap_or(99);
                    (s("name"), s("status"), n("stashed"), n("carried"))
                })
                .collect()
        })
}

/// An item's runes as `(rune, need, have)`.
fn farm_runes(json: &serde_json::Value, item: usize) -> Vec<(String, u64, u64)> {
    json.pointer(&format!("/farm/groups/0/items/{item}/runes"))
        .and_then(serde_json::Value::as_array)
        .map_or_default(|runes| {
            runes
                .iter()
                .map(|r| {
                    (
                        r.get("rune")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        r.get("need").and_then(serde_json::Value::as_u64).unwrap_or(99),
                        r.get("have").and_then(serde_json::Value::as_u64).unwrap_or(99),
                    )
                })
                .collect()
        })
}

#[test]
fn farm_status_done_found_missing_and_rune_needs() -> Result<()> {
    use d2r_core::offsets::item_data::{QUALITY_NORMAL, QUALITY_UNIQUE};
    let farm = fury_farm()?;
    let blocks = d2r_core::render::TextBlocks::default();
    let mut lawbringer = stash_item("Lawbringer", "flc", QUALITY_NORMAL, None);
    lawbringer.runeword = true;
    let mut stash = vec![d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(1),
        items: vec![
            stash_item("Raven Frost", "rin", QUALITY_UNIQUE, None),
            lawbringer,
            // An unidentified unique is no copy yet.
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
    info.farm = &farm;
    info.live.owned = &owned;
    info.live.runeword_copies = &copies;
    info.live.held_materials = &held;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        json.pointer("/farm/groups/0/for"),
        Some(&serde_json::json!("Fury Druid"))
    );
    let row = |name: &str, status: &str, stashed: u64, carried: u64| {
        (name.to_owned(), status.to_owned(), stashed, carried)
    };
    assert_eq!(
        farm_rows(&json),
        [
            row("Fortitude", "missing", 0, 0),
            row("Raven Frost", "done", 1, 0),
            row("Ribcracker", "found", 0, 1),
            // 1 stashed + 1 carried of 2: not done, found, nothing left to build.
            row("Lawbringer", "found", 1, 1),
            row("Smoke", "missing", 0, 0),
            row("Bulwark", "missing", 0, 0),
        ]
    );
    let rune = |r: &str, need: u64, have: u64| (r.to_owned(), need, have);
    assert_eq!(
        farm_runes(&json, 0),
        [
            rune("El", 1, 8),
            rune("Sol", 1, 1),
            rune("Dol", 1, 0),
            rune("Lo", 1, 0)
        ]
    );
    assert_eq!(farm_runes(&json, 3), [], "Lawbringer: both copies exist");
    assert_eq!(farm_runes(&json, 1), [], "uniques have no runes");
    assert_eq!(
        json.pointer("/farm/groups/0/items/0/bases"),
        Some(&serde_json::json!(["weapons", "armor"]))
    );
    assert_eq!(
        json.pointer("/farm/groups/0/items/3/bases"),
        Some(&serde_json::json!([]))
    );
    assert_eq!(
        json.pointer("/farm/groups/0/items/0/base_hint"),
        Some(&serde_json::Value::Null),
        "unknown character level: no item level to check bases by"
    );
    // The rune list sums over the list: Sol for Fortitude and Bulwark; held runes count
    // as had, but not as in the stash.
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
    let rows = |info: &PanelInfo| -> Result<Vec<(String, String, u64, u64)>> {
        let json: serde_json::Value = serde_json::from_str(&live_json(info))?;
        Ok(farm_rows(&json))
    };
    let row = |status: &str, stashed: u64, carried: u64| {
        vec![("Teleport Amulet".to_owned(), status.to_owned(), stashed, carried)]
    };
    // Nothing held; a Teleport ring and a plain amulet are no copies.
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
    assert_eq!(rows(&info)?, row("missing", 0, 0));
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    let entry = json.pointer("/farm/groups/0/items/0").context("entry")?;
    assert_eq!(entry.get("kind"), Some(&serde_json::json!("rolled")));
    assert_eq!(entry.get("slot"), Some(&serde_json::json!("amulet")));
    assert_eq!(entry.get("needs"), Some(&serde_json::json!(["Teleport"])));
    assert_eq!(entry.get("note"), Some(&serde_json::json!("until Enigma")));
    // Worn, or in the inventory: found.
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
    assert_eq!(rows(&info)?, row("found", 0, 1));
    info.live.equipped_items = &[];
    let carried = [(
        d2r_core::game::ItemDrop {
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
    assert_eq!(rows(&info)?, row("found", 0, 1));
    // In the shared stash: done.
    let stashed = [d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(2),
        items: vec![d2r_core::stash::StashItem {
            stats: teleport,
            ..stash_item("Magic Amulet", "amu", QUALITY_MAGIC, None)
        }],
    }];
    info.live.stash = &stashed;
    assert_eq!(rows(&info)?, row("done", 1, 1));
    // An Enigma only the mercenary wears doesn't count; one the character holds does: the
    // entry (and its group, now empty) leaves the panel.
    let copies = ["Enigma".to_owned()];
    info.live.runeword_copies = &copies;
    assert_eq!(rows(&info)?, row("done", 1, 1));
    info.live.owned = &copies;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(json.pointer("/farm/groups"), Some(&serde_json::json!([])));
    Ok(())
}

#[test]
fn farm_counts_split_and_multiply_rune_needs() -> Result<()> {
    let farm = fury_farm()?;
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.farm = &farm;
    // No copy anywhere: two Lawbringers to build, two of each rune.
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    let rune = |r: &str, need: u64| (r.to_owned(), need, 0);
    assert_eq!(
        farm_runes(&json, 3),
        [rune("Amn", 2), rune("Lem", 2), rune("Ko", 2)]
    );
    // One carried: one left to build.
    let copies = ["Lawbringer".to_owned()];
    info.live.runeword_copies = &copies;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        farm_runes(&json, 3),
        [rune("Amn", 1), rune("Lem", 1), rune("Ko", 1)]
    );
    assert_eq!(
        json.pointer("/farm/groups/0/items/3/status"),
        Some(&serde_json::json!("found"))
    );
    // Two in the shared stash: done, no runes, gone from the rune list.
    let mut stashed = stash_item("Lawbringer", "flc", 2, None);
    stashed.runeword = true;
    let stash = vec![d2r_core::stash::StashTab {
        kind: d2r_core::stash::TabKind::Page(1),
        items: vec![stashed.clone(), stashed],
    }];
    info.live.stash = &stash;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        json.pointer("/farm/groups/0/items/3/status"),
        Some(&serde_json::json!("done"))
    );
    assert_eq!(farm_runes(&json, 3), []);
    let runes = json.pointer("/farm/runes").map_or_default(ToString::to_string);
    assert!(!runes.contains("\"Amn\""), "{runes}");
    Ok(())
}

#[test]
fn farm_names_the_bosses_worth_farming() -> Result<()> {
    let farm = fury_farm()?;
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    info.farm = &farm;
    info.live.difficulty = Some(0);
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        json.pointer("/farm/worth/difficulty"),
        Some(&serde_json::json!("Normal"))
    );
    // Normal Countess (level 11) drops runes up to Ral and keeps them all: Fortitude's El and
    // Smoke's Nef. Normal Andariel (12) drops no rune above Ral, so she is not listed;
    // Mephisto (26) is the first boss worth it, for runes above Ral.
    let boss = |i: usize| json.pointer(&format!("/farm/worth/bosses/{i}/boss")).cloned();
    assert!(
        (0..20).all(|i| boss(i) != Some(serde_json::json!("Andariel"))),
        "{json}"
    );
    assert_eq!(
        json.pointer("/farm/worth/bosses/0"),
        Some(&serde_json::json!({
            "boss": "Mephisto",
            "level": 26,
            "items": [
                { "name": "Fortitude", "runes": ["Sol"] },
                { "name": "Lawbringer", "runes": ["Amn"] },
                { "name": "Bulwark", "runes": ["Sol"] },
            ],
        }))
    );
    assert_eq!(
        json.pointer("/farm/worth/bosses/5"),
        Some(&serde_json::json!({
            "boss": "The Countess",
            "level": 11,
            "items": [
                { "name": "Fortitude", "runes": ["El"] },
                { "name": "Smoke", "runes": ["Nef"] },
            ],
        }))
    );
    // Unknown difficulty: no note.
    info.live.difficulty = None;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(json.pointer("/farm/worth"), Some(&serde_json::Value::Null));
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
    // The page renders the place in its own span, red only when wrong.
    let page = d2r_core::web::page::DASHBOARD_PAGE;
    assert!(page.contains(r#"<span class="wrong">${esc(e.place)}</span>"#));
    assert!(page.contains(".line.run .wrong { color: var(--bad);"));
    assert!(page.contains("Keep · trade (pages 3–4)"));
    Ok(())
}

/// The live JSON's `gear_attention_why` for a level-`level` character with player
/// `slots`, mercenary `merc` slots, in `difficulty` (it farms up to
/// [`d2r_core::info::reachable_level`]: in Normal the level +2), `owned` names and `held`
/// materials; asserts `gear_attention` agrees with it.
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
    // Harlequin Crest (lvl 62) and Biggin's Bonnet (lvl 3) are too high to wear at 2: closed.
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
    // A piece described in words has no item level to check: never findable.
    let words: [(&str, &[&str]); 1] = [("helm", &["Druid pelt with +summoning skills@3"])];
    assert_eq!(attention(&words, &[], 10, 0, &[], &[])?, None);
    Ok(())
}

#[test]
fn gear_attention_for_runewords_counts_owned_runes() -> Result<()> {
    // Stealth = Tal (qlvl 17) + Eth (15); Leaf = Tir (13) + Ral (19). The pieces' levels
    // are set below the runes' so a level-14 character (farming up to 16 in Normal) can
    // wear both but finds neither Tal nor Ral.
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
    // A level-43 druid (class 5) in Nightmare: it farms up to monster level 45.
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
    // The weapon column keeps Spirit's swords (not its shields).
    let spirit = hint(&format!(
        "/gear/columns/{}/pieces/0/base_hint",
        column("Weapons")?
    ))?;
    assert!(
        spirit.starts_with("4 sockets: Crystal Sword (43 Str)"),
        "{spirit}"
    );
    assert!(!spirit.contains("shields"), "{spirit}");
    // The body column keeps Fortitude's armor (not its weapons).
    let fort = hint(&format!("/gear/columns/{}/pieces/0/base_hint", column("Body")?))?;
    assert!(fort.starts_with("4 sockets: "), "{fort}");
    assert!(!fort.contains("Staff"), "{fort}");
    // The mercenary's Insight (taken as Act 2): polearms only, no staves or bows.
    let insight = hint("/gear/merc/0/pieces/0/base_hint")?;
    assert!(
        !insight.contains("Staff") && !insight.contains("Bow") && !insight.contains("staves"),
        "{insight}"
    );
    assert!(insight.starts_with("4 sockets: Voulge (50 Str)"), "{insight}");
    // The farm list has no slot: Fortitude lists weapons and armor separately.
    let farm_fort = hint("/farm/groups/0/items/0/base_hint")?;
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
    // Fortitude as body armor: armor only, no weapon group.
    let fort = hint("/farm/groups/0/items/0/base_hint")?;
    assert!(fort.starts_with("4 sockets: "), "{fort}");
    assert!(!fort.contains("weapons: ") && !fort.contains("armor: "), "{fort}");
    // The merc's Bulwark: no druid pelt, strongest first with its defense.
    let merc = hint("/farm/groups/0/items/1/base_hint")?;
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
    // The gear card's merc helm column ranks the same way.
    assert_eq!(hint("/gear/merc/0/pieces/0/base_hint")?, merc);
    // Without merc/slot the druid's own cheapest helms, pelts included.
    let own = hint("/farm/groups/0/items/2/base_hint")?;
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

/// A named item (unique or runeword) the character wears in `body`.
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
    let names = |json: &serde_json::Value| -> Vec<(String, Vec<String>)> {
        json.pointer("/farm/groups")
            .and_then(serde_json::Value::as_array)
            .map_or_default(|groups| {
                groups
                    .iter()
                    .map(|g| {
                        let items = g
                            .get("items")
                            .and_then(serde_json::Value::as_array)
                            .map_or_default(|i| {
                                i.iter()
                                    .filter_map(|i| Some(i.get("name")?.as_str()?.to_owned()))
                                    .collect()
                            });
                        let target = g.get("for").and_then(|f| f.as_str()).unwrap_or_default();
                        (target.to_owned(), items)
                    })
                    .collect()
            })
    };
    let fury_all: Vec<String> = [
        "Fortitude",
        "Raven Frost",
        "Ribcracker",
        "Lawbringer",
        "Smoke",
        "Bulwark",
    ]
    .map(str::to_owned)
    .to_vec();
    let fury_rest: Vec<String> = fury_all.iter().filter(|n| *n != "Raven Frost").cloned().collect();
    // The summoner wears Spirit and Raven Frost, and one of the two Lawbringers.
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
    // Spirit worn for the current build: its group (all hidden) disappears. Raven Frost
    // worn for the later Fury stage: hidden too. One Lawbringer of two: still listed.
    assert_eq!(names(&json), [("Fury Druid".to_owned(), fury_rest.clone())]);
    // Another build's character wearing the same items: everything stays.
    let other_chain = ["Other Build".to_owned()];
    info.stage_chain = &other_chain;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        names(&json),
        [
            ("Summoner Druid".to_owned(), vec!["Spirit".to_owned()]),
            ("Fury Druid".to_owned(), fury_all.clone()),
        ]
    );
    // A carried copy doesn't hide an entry.
    let owned = ["Raven Frost".to_owned()];
    info.stage_chain = &summoner_chain;
    info.live.equipped_items = &[];
    info.live.owned = &owned;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        names(&json),
        [
            ("Summoner Druid".to_owned(), vec!["Spirit".to_owned()]),
            ("Fury Druid".to_owned(), fury_all.clone()),
        ]
    );
    // Mercenary-worn copies (a unique and a runeword; `merc_worn` is filled only in solo
    // games, `game::held_copies`) hide an entry like the character's own.
    let merc = ["Raven Frost".to_owned(), "Spirit".to_owned()];
    info.live.owned = &[];
    info.live.merc_worn = &merc;
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(names(&json), [("Fury Druid".to_owned(), fury_rest)]);
    // In a group game `merc_worn` is empty: the merc's gear hides nothing.
    info.live.merc_worn = &[];
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        names(&json),
        [
            ("Summoner Druid".to_owned(), vec!["Spirit".to_owned()]),
            ("Fury Druid".to_owned(), fury_all),
        ]
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

#[test]
fn next_section_carries_the_route_step() -> Result<()> {
    let blocks = d2r_core::render::TextBlocks::default();
    let mut info = live_info(5, &blocks, &[], &[], &[]);
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(json.get("next"), Some(&serde_json::Value::Null), "no route");
    let advice = crate::route::Advice {
        id: "nm-diablo".to_owned(),
        title: "Finish Diablo".to_owned(),
        todo: vec!["Quests left: Terror's End".to_owned()],
        note: "Not worth farming.".to_owned(),
        then: Some("The Ancients".to_owned()),
    };
    info.next = Some(&advice);
    let json: serde_json::Value = serde_json::from_str(&live_json(&info))?;
    assert_eq!(
        json.get("next"),
        Some(&serde_json::json!({
            "id": "nm-diablo",
            "title": "Finish Diablo",
            "todo": ["Quests left: Terror's End"],
            "note": "Not worth farming.",
            "then": "The Ancients",
        }))
    );
    Ok(())
}

/// The summoner's merc weapon column (`merc_acts = [2, 1]`): Insight's hint names the Act 2
/// merc's polearms, Harmony's the Act 1 rogue's bows, marked as that act's.
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

/// The live case of 2026-11-20 on the shipped summoner build: a level-57 druid whose merc
/// wears Insight, Smoke and Bulwark (solo game, so `merc_worn` is filled). Each worn piece is
/// ticked ✓ (Insight although its leveling window ended at 40), the stash's Soul Harvest and
/// The Battlebranch are beaten, Smoke drops its "missing Lum" note, and no merc piece reaches
/// the upgrade bar.
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

    let weapon = column(&gear, "Merc weapon");
    let names: Vec<_> = weapon.iter().map(|p| field(p, "name")).collect();
    // Harmony last: the Act 1 rogue's (the build's second merc act), with that act's bows.
    assert_eq!(
        names,
        ["Insight", "Soul Harvest", "The Battlebranch", "Harmony"],
        "{weapon:?}"
    );
    assert_eq!(field(&at(&weapon, 0), "tick"), "worn");
    for beaten in weapon.iter().skip(1) {
        assert_eq!(field(beaten, "state"), "worse", "{beaten}");
        assert_eq!(field(beaten, "now"), serde_json::Value::Null, "{beaten}");
    }

    let body = column(&gear, "Merc body");
    assert_eq!(body.len(), 1, "{body:?}");
    assert_eq!(field(&at(&body, 0), "name"), "Smoke");
    assert_eq!(field(&at(&body, 0), "tick"), "worn");
    assert_eq!(
        field(&at(&body, 0), "notes"),
        serde_json::Value::Null,
        "no missing-rune note"
    );
    assert_eq!(field(&at(&body, 0), "now"), serde_json::Value::Null);

    let helm = column(&gear, "Merc helm");
    assert_eq!(helm.len(), 1, "{helm:?}");
    assert_eq!(field(&at(&helm, 0), "name"), "Bulwark");
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
    // Only the pieces ranked above the worn one stay open for the vendor match.
    let weapons = bis.merc_slots.get("weapon").context("merc weapon slot")?;
    assert_eq!(super::gear::merc_open(weapons, &info), &[] as &[String]);

    // In a group game the merc's gear is unread: no ticks, nothing beaten.
    info.live.merc_worn = &[];
    let gear = gear_json(&info);
    let weapon = column(&gear, "Merc weapon");
    assert!(weapon.iter().all(|p| field(p, "tick") != "worn"), "{weapon:?}");
    assert_eq!(super::gear::merc_open(weapons, &info).len(), weapons.len());
    Ok(())
}
