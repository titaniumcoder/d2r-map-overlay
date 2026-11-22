//! The shipped `playstyles/trapsin/` folder parses without warnings: the Fire Wake Assassin →
//! Trapsin chain, its farm list, route and loot filter name.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use d2r_core::item_names_data::{DISPLAY_NAMES, SET_NAMES, UNIQUE_NAMES};
use d2r_core::names::same_name;
use d2r_playstyle_config::config::{Build, parse_build, parse_farm, parse_playstyle};
use d2r_playstyle_config::route::parse_route;

/// The assassin's class id (charstats row).
const ASSASSIN: u32 = 6;

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../playstyles/trapsin")
}

fn read(rel: &str) -> Result<String> {
    let path = folder().join(rel);
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

fn build(stem: &str) -> Result<(String, Build)> {
    let (slug, def, warnings) = parse_build(&read(&format!("builds/{stem}.toml"))?)?;
    ensure!(warnings.is_empty(), "{stem}: {warnings:?}");
    Ok((slug, def))
}

/// Whether `name` is a unique, set item or runeword of the game tables.
fn named_item(name: &str) -> bool {
    d2r_core::runewords_data::lookup(name).is_some()
        || UNIQUE_NAMES.iter().any(|&(_, n)| same_name(n, name))
        || SET_NAMES.iter().any(|&(_, n)| same_name(n, name))
        || DISPLAY_NAMES.iter().any(|&(_, n)| same_name(n, name))
}

/// Every slot piece of `def` is a named game item, or a piece the file calls magic.
fn pieces_known(def: &Build) -> Result<()> {
    for (slot, pieces) in def.slots.iter().chain(&def.merc_slots).chain(&def.extras) {
        for piece in pieces {
            let name = d2r_core::info::piece_name(piece);
            let magic = def.pieces.get(name).is_some_and(|p| p.kind == "magic");
            ensure!(
                named_item(name) || magic,
                "{}: {slot} piece {name:?} unknown",
                def.name
            );
        }
    }
    Ok(())
}

#[test]
fn playstyle_names_the_chain_and_its_filter() -> Result<()> {
    let ((name, tables), warnings) = parse_playstyle(&read("playstyle.toml")?)?;
    ensure!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(name, "Fire Wake Assassin → Trapsin");
    assert_eq!(tables.filter_name.as_deref(), Some("Trapsin"));
    let filter: serde_json::Value = serde_json::from_str(&read("loot-filter.json")?)?;
    assert_eq!(
        filter.get("name").and_then(serde_json::Value::as_str),
        Some("Trapsin")
    );
    Ok(())
}

#[test]
fn fire_wake_switches_to_the_trapsin_at_38() -> Result<()> {
    let (slug, fire) = build("fire-wake-assassin")?;
    let (next_slug, trapsin) = build("trapsin")?;
    assert_eq!(slug, "fire wake assassin");
    assert_eq!(next_slug, "trapsin");
    assert_eq!(
        (fire.name.as_str(), trapsin.name.as_str()),
        ("Fire Wake Assassin", "Trapsin")
    );
    assert_eq!(fire.next.as_deref(), Some(next_slug.as_str()));
    assert_eq!(fire.switch_level, Some(38));
    assert!(fire.auto_switch);
    assert!(fire.switch_requires.is_empty(), "the level alone switches");
    assert!(fire.default_for_class && !trapsin.default_for_class);
    assert_eq!((fire.class, trapsin.class), (Some(ASSASSIN), Some(ASSASSIN)));
    assert_eq!(trapsin.next, None);
    // The plans start where the user takes over (13) and at the reset (38).
    assert_eq!(fire.skill_plan.iter().map(|t| t.by_level).min(), Some(13));
    assert_eq!(trapsin.skill_plan.iter().map(|t| t.by_level).min(), Some(38));
    pieces_known(&fire)?;
    pieces_known(&trapsin)?;
    for stem in ["fire-wake-assassin", "trapsin"] {
        let source = folder().join(format!("builds/{stem}.source.json"));
        ensure!(source.is_file(), "{} missing", source.display());
    }
    Ok(())
}

#[test]
fn farm_list_and_route_parse_without_warnings() -> Result<()> {
    let (groups, warnings) = parse_farm(&read("farm.toml")?)?;
    ensure!(warnings.is_empty(), "{warnings:?}");
    let targets: Vec<&str> = groups.iter().map(|g| g.target.as_str()).collect();
    assert_eq!(targets, ["Fire Wake Assassin", "Trapsin"]);
    let (route, warnings) = parse_route(&read("route.toml")?)?;
    ensure!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        route.bosses,
        ["The Countess", "Andariel", "Mephisto", "Diablo", "Baal"]
    );
    Ok(())
}
