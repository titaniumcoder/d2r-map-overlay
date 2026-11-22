//! The pieces the player marked "not needed" on the dashboard, per build: `not-needed.toml` in
//! the data dir (`[not_needed]` `"<build name>" = ["<piece>", …]`). The build files stay as they are; the load
//! takes the marked pieces out of the build's slots, extras and runeword recipes and out of its
//! farm group ([`apply`]), so no part of the playstyle wants them.

use std::collections::BTreeMap;

use d2r_core::info::piece_name;
use d2r_core::names::same_name;
use toml_edit::DocumentMut;

use crate::config::{Build, FarmGroup};

/// Lowercased build name → the piece names marked "not needed" for it.
pub type NotNeeded = BTreeMap<String, Vec<String>>;

/// A new `not-needed.toml`; the comment heads the table so it stays on top.
pub const HEADER: &str = "# Build pieces marked \"not needed\" on the dashboard: \"<build name>\" = [\"<piece>\", ...].\n\
                          # Written by the overlay's dashboard (the x on a piece; \"show hidden\" undoes it).\n\
                          [not_needed]\n";

const TABLE: &str = "not_needed";

/// Parses `not-needed.toml`; anything but a list of names under `[not_needed]` is skipped
/// with a warning.
///
/// # Errors
/// Invalid TOML.
pub fn parse(text: &str) -> Result<(NotNeeded, Vec<String>), toml::de::Error> {
    let mut file: toml::Table = toml::from_str(text)?;
    let mut warnings = Vec::new();
    let table = match file.remove(TABLE) {
        Some(toml::Value::Table(table)) => table,
        Some(_) => {
            warnings.push(format!("{TABLE} is not a table; ignored"));
            toml::Table::new()
        }
        None => toml::Table::new(),
    };
    warnings.extend(
        file.keys()
            .map(|k| format!("{k:?}: not under [{TABLE}]; ignored")),
    );
    let mut out = NotNeeded::new();
    for (build, value) in table {
        let names = value.as_array().and_then(|list| {
            list.iter()
                .map(|v| v.as_str().map(|s| s.trim().to_owned()))
                .collect::<Option<Vec<_>>>()
        });
        match names {
            Some(names) => out
                .entry(build.trim().to_lowercase())
                .or_default()
                .extend(names.into_iter().filter(|n| !n.is_empty())),
            None => warnings.push(format!("{build:?}: not a list of piece names; ignored")),
        }
    }
    Ok((out, warnings))
}

/// Marks `piece` of `build` not needed (`needed = false`) or needed again, in `text`, comments
/// kept. Build keys and piece names compare ignoring case; a build left without pieces leaves
/// the file. `None` when `text` is not valid TOML.
#[must_use]
pub fn set(text: &str, build: &str, piece: &str, needed: bool) -> Option<String> {
    let mut file = text.parse::<DocumentMut>().ok()?;
    if file.get(TABLE).is_none() {
        file.insert(TABLE, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let doc = file.get_mut(TABLE)?.as_table_mut()?;
    let key = doc
        .iter()
        .map(|(k, _)| k)
        .find(|k| k.eq_ignore_ascii_case(build))
        .unwrap_or(build)
        .to_owned();
    let mut names: Vec<String> =
        doc.get(&key)
            .and_then(toml_edit::Item::as_array)
            .map_or_else(Vec::new, |list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            });
    let same = |n: &String| n.trim().eq_ignore_ascii_case(piece.trim());
    if needed {
        names.retain(|n| !same(n));
    } else if !names.iter().any(same) {
        names.push(piece.trim().to_owned());
    }
    if names.is_empty() {
        doc.remove(&key);
    } else {
        doc[key.as_str()] = toml_edit::value(names.iter().collect::<toml_edit::Array>());
    }
    Some(file.to_string())
}

/// Takes the marked pieces out of `builds` (keyed by slug) and `farm`, noting on each build
/// and farm group the names it lost (`not_needed`, for the dashboard's undo). A marked name
/// that matches nothing is ignored. A runeword recipe leaves only once no piece of the build
/// still names that runeword ("Hustle (weapon)" marked, "Hustle (armor)" kept: the recipe
/// stays). `switch_requires` is left alone: a stage switch keeps wanting its items.
pub fn apply(marked: &NotNeeded, builds: &mut BTreeMap<String, Build>, farm: &mut [FarmGroup]) {
    for (slug, build) in builds.iter_mut() {
        if let Some(names) = marked.get(slug) {
            build_without(build, names);
        }
    }
    for group in farm {
        if let Some(names) = marked.get(&group.target.trim().to_lowercase()) {
            let mut lost = Vec::new();
            group.items.retain(|item| kept(names, &item.name, &mut lost));
            group.not_needed = lost;
        }
    }
}

fn build_without(build: &mut Build, names: &[String]) {
    let mut lost = Vec::new();
    let mut drop = |pieces: &mut Vec<String>| {
        pieces.retain(|entry| kept(names, piece_name(entry), &mut lost));
    };
    build.slots.values_mut().for_each(&mut drop);
    build.merc_slots.values_mut().for_each(&mut drop);
    build.extras.values_mut().for_each(&mut drop);
    build.slots.retain(|_, pieces| !pieces.is_empty());
    build.merc_slots.retain(|_, pieces| !pieces.is_empty());
    build.extras.retain(|_, pieces| !pieces.is_empty());
    let still_named = |name: &str| {
        build
            .slots
            .values()
            .chain(build.merc_slots.values())
            .chain(build.extras.values())
            .flatten()
            .any(|entry| same_name(piece_name(entry), name))
    };
    // Edition 2021 closures capture fields, so `still_named` reading the slots doesn't clash
    // with `retain` borrowing the recipes.
    build.recipes.retain(|r| {
        r.kind != "runeword" || !lost.iter().any(|n| same_name(n, &r.name)) || still_named(&r.name)
    });
    build.not_needed = lost;
}

/// The marked name `name` is, compared as item names are ([`same_name`]): "Hustle" marked
/// covers both Hustles, "Hustle (weapon)" only the weapon one.
fn find<'n>(names: &'n [String], name: &str) -> Option<&'n str> {
    names
        .iter()
        .find(|n| same_name(n, name) && qualifier_fits(n, name))
        .map(String::as_str)
}

/// `same_name` lets a qualified name match a bare one either way; a marked bare name may cover
/// qualified entries, but a marked "Hustle (weapon)" must not take a bare "Hustle" entry that
/// may be the armor.
fn qualifier_fits(marked: &str, entry: &str) -> bool {
    !marked.trim().ends_with(')') || entry.trim().ends_with(')')
}

/// Whether `name` stays; a marked one joins `lost` (once).
fn kept(names: &[String], name: &str, lost: &mut Vec<String>) -> bool {
    let Some(marked) = find(names, name) else {
        return true;
    };
    if !lost.iter().any(|n| n == marked) {
        lost.push(marked.to_owned());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FarmItem, FarmKind, Recipe};
    use anyhow::{Context, Result};

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|&n| n.to_owned()).collect()
    }

    #[test]
    fn set_round_trips_through_parse() -> Result<()> {
        let text = set(HEADER, "Fury Druid", "Grief", false).context("valid")?;
        let text = set(&text, "fury druid", "Hustle (weapon)", false).context("valid")?;
        let text = set(&text, "FURY DRUID", "grief", false).context("valid")?;
        assert!(text.starts_with("# Build pieces"), "comments kept: {text}");
        let (marked, warnings) = parse(&text)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            marked.get("fury druid"),
            Some(&list(&["Grief", "Hustle (weapon)"])),
            "one key, no duplicate: {text}"
        );
        let text = set(&text, "Fury Druid", "GRIEF", true).context("valid")?;
        assert_eq!(
            parse(&text)?.0.get("fury druid"),
            Some(&list(&["Hustle (weapon)"]))
        );
        let text = set(&text, "Fury Druid", "Hustle (weapon)", true).context("valid")?;
        assert_eq!(parse(&text)?.0, NotNeeded::new(), "an empty build leaves: {text}");
        let text = set(&text, "Fury Druid", "Grief", true).context("valid")?;
        assert_eq!(parse(&text)?.0, NotNeeded::new(), "undoing nothing is fine");
        assert_eq!(set("not = [toml", "B", "P", false), None);
        Ok(())
    }

    #[test]
    fn odd_values_are_skipped_not_fatal() -> Result<()> {
        let text = "top = 1\n[not_needed]\n\"A\" = \"Grief\"\n\"B\" = [1]\n\"C\" = [\"Oath\", \"\"]\n";
        let (marked, warnings) = parse(text)?;
        assert_eq!(marked.len(), 1);
        assert_eq!(marked.get("c"), Some(&list(&["Oath"])));
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(parse("not_needed = 1\n")?.0.is_empty());
        assert_eq!(set("not_needed = 1\n", "a", "Oath", false), None, "not a table");
        let text = set("[not_needed]\n\"A\" = \"Grief\"\n", "a", "Oath", false).context("valid")?;
        assert_eq!(
            parse(&text)?.0.get("a"),
            Some(&list(&["Oath"])),
            "a non-list is replaced"
        );
        Ok(())
    }

    fn recipe(name: &str) -> Recipe {
        Recipe {
            name: name.to_owned(),
            kind: "runeword".to_owned(),
            inputs: Vec::new(),
            result: String::new(),
            req: String::new(),
            notes: String::new(),
            quest: String::new(),
        }
    }

    fn farm_item(name: &str) -> FarmItem {
        FarmItem {
            name: name.to_owned(),
            kind: FarmKind::Runeword,
            count: 1,
            merc: false,
            slot: None,
            needs: Vec::new(),
            note: None,
        }
    }

    #[test]
    fn apply_takes_marked_pieces_out_of_the_build_and_its_farm_group() -> Result<()> {
        let mut build = Build {
            name: "Fury Druid".to_owned(),
            recipes: vec![recipe("Grief"), recipe("Hustle"), recipe("Oath")],
            ..Build::default()
        };
        build.slots.insert(
            "weapon".to_owned(),
            list(&["Grief@59", "Hustle (weapon)@39", "Oath@49"]),
        );
        build
            .slots
            .insert("body".to_owned(), list(&["Hustle (armor)@39"]));
        build
            .extras
            .insert("charms".to_owned(), list(&["Gheed's Fortune@62"]));
        build.merc_slots.insert("weapon".to_owned(), list(&["Grief@59"]));
        let mut builds = BTreeMap::from([("fury druid".to_owned(), build)]);
        let mut farm = vec![
            FarmGroup {
                target: "Fury Druid".to_owned(),
                items: vec![farm_item("Grief"), farm_item("Oath")],
                not_needed: Vec::new(),
            },
            FarmGroup {
                target: "Summoner Druid".to_owned(),
                items: vec![farm_item("Grief")],
                not_needed: Vec::new(),
            },
        ];
        let marked = NotNeeded::from([(
            "fury druid".to_owned(),
            list(&[
                "Grief",
                "Hustle (weapon)",
                "Gheeds Fortune",
                "Nothing Of This Name",
            ]),
        )]);
        apply(&marked, &mut builds, &mut farm);
        let build = builds.get("fury druid").context("the build stays")?;
        assert_eq!(build.slots.get("weapon"), Some(&list(&["Oath@49"])));
        assert_eq!(build.slots.get("body"), Some(&list(&["Hustle (armor)@39"])));
        assert!(
            build.extras.is_empty() && build.merc_slots.is_empty(),
            "{build:?}"
        );
        let recipes: Vec<&str> = build.recipes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            recipes,
            ["Hustle", "Oath"],
            "the armor Hustle still wants its recipe"
        );
        assert_eq!(
            build.not_needed,
            list(&["Grief", "Hustle (weapon)", "Gheeds Fortune"]),
            "an unknown name is ignored"
        );
        let farm_names: Vec<Vec<&str>> = farm
            .iter()
            .map(|g| g.items.iter().map(|i| i.name.as_str()).collect())
            .collect();
        assert_eq!(farm_names, [vec!["Oath"], vec!["Grief"]], "per build only");
        assert_eq!(farm.first().map(|g| &g.not_needed), Some(&list(&["Grief"])));
        Ok(())
    }
}
