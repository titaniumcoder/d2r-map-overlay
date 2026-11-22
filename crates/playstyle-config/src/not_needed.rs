//! The pieces the player marked "not needed" on the dashboard, per build: `not-needed.toml` in
//! the data dir (`[not_needed]` `"<build name>" = ["<piece>", …]`). The build files stay as
//! they are; the load takes the marked pieces out of the build, the later stages of its chain
//! and their farm groups ([`marks`], [`apply`]), so no part of the playstyle wants them.

use std::collections::BTreeMap;

use d2r_core::guidance::Bis;
use d2r_core::info::piece_name;
use d2r_core::names::same_name;
use serde::Serialize;
use toml_edit::DocumentMut;

use crate::config::{Build, FarmGroup};

/// Lowercased build name → the piece names marked "not needed" for it, as in the file.
pub type NotNeeded = BTreeMap<String, Vec<String>>;

/// One mark in force for a build: the piece and the build it was marked for (its display name,
/// the key the undo writes back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Marked {
    pub name: String,
    pub build: String,
}

/// Lowercased build name (or `[bis.<name>]` key) → the marks in force for it.
pub type Marks = BTreeMap<String, Vec<Marked>>;

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
/// the file. A `not_needed` that isn't a table (an inline table, a stray value) becomes one,
/// keeping an inline table's entries. `None` when `text` is not valid TOML.
#[must_use]
pub fn set(text: &str, build: &str, piece: &str, needed: bool) -> Option<String> {
    let mut file = text.parse::<DocumentMut>().ok()?;
    if !file.get(TABLE).is_some_and(toml_edit::Item::is_table) {
        let table = match file.remove(TABLE) {
            Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(inline))) => inline.into_table(),
            _ => toml_edit::Table::new(),
        };
        file.insert(TABLE, toml_edit::Item::Table(table));
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

/// The marks in force per build: each build's own, plus those of every earlier stage whose
/// chain (`next` links) reaches it. The dashboard shows one stage, but the stash plan keeps
/// for the whole chain: a piece marked while playing the Summoner must not stay wanted
/// through the Fury stage after it.
#[must_use]
pub fn marks(marked: &NotNeeded, builds: &BTreeMap<String, Build>) -> Marks {
    let mut out = Marks::new();
    for (key, names) in marked {
        let shown = builds.get(key).map_or(key.as_str(), |b| b.name.as_str());
        for slug in chain(key, builds) {
            let list = out.entry(slug).or_default();
            for name in names {
                if !list.iter().any(|m| m.name.eq_ignore_ascii_case(name)) {
                    list.push(Marked {
                        name: name.clone(),
                        build: shown.to_owned(),
                    });
                }
            }
        }
    }
    out
}

/// `from` and the slugs after it along `next`, each once.
fn chain(from: &str, builds: &BTreeMap<String, Build>) -> Vec<String> {
    let mut slugs = vec![from.to_owned()];
    let mut slug = from;
    while let Some(next) = builds.get(slug).and_then(|b| b.next.as_deref()) {
        if slugs.iter().any(|s| s == next) {
            break;
        }
        slugs.push(next.to_owned());
        slug = next;
    }
    slugs
}

/// Takes the marked pieces out of `builds` (keyed by slug) and of the `farm` groups for them.
/// A runeword recipe leaves only once no piece of the build still names that runeword
/// ("Hustle (weapon)" marked, "Hustle (armor)" kept: the recipe stays). `switch_requires` is
/// left alone, so a stage switch keeps its conditions; the stash plan skips the marked items
/// there ([`covers`]). Afterwards `marks` keeps, per build,
/// its own marks and the inherited ones that took something there (for the dashboard's
/// undo); a name that matches nothing has no effect.
pub fn apply(marks: &mut Marks, builds: &mut BTreeMap<String, Build>, farm: &mut [FarmGroup]) {
    let mut took: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (slug, build) in builds.iter_mut() {
        if let Some(list) = marks.get(slug) {
            build_without(build, list, took.entry(slug.clone()).or_default());
        }
    }
    for group in farm {
        let key = group.target.trim().to_lowercase();
        if let Some(list) = marks.get(&key) {
            let took = took.entry(key).or_default();
            group.items.retain(|item| kept(list, &item.name, took));
        }
    }
    for (key, list) in marks.iter_mut() {
        let took = took.get(key).map_or(&[][..], Vec::as_slice);
        list.retain(|m| m.build.eq_ignore_ascii_case(key) || took.contains(&m.name));
    }
    marks.retain(|_, list| !list.is_empty());
}

/// Takes the pieces of `marks` out of a character's own `[bis.<name>]` slots.
pub fn bis_without(bis: &mut Bis, marks: &[Marked]) {
    let mut took = Vec::new();
    slots_without(&mut bis.slots, marks, &mut took);
    slots_without(&mut bis.merc_slots, marks, &mut took);
}

fn slots_without(slots: &mut BTreeMap<String, Vec<String>>, marks: &[Marked], took: &mut Vec<String>) {
    for pieces in slots.values_mut() {
        pieces.retain(|entry| kept(marks, piece_name(entry), took));
    }
    slots.retain(|_, pieces| !pieces.is_empty());
}

fn build_without(build: &mut Build, marks: &[Marked], took: &mut Vec<String>) {
    slots_without(&mut build.slots, marks, took);
    slots_without(&mut build.merc_slots, marks, took);
    slots_without(&mut build.extras, marks, took);
    build.charms.retain(|wish| kept(marks, &wish.name, took));
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
        r.kind != "runeword" || !took.iter().any(|n| same_name(n, &r.name)) || still_named(&r.name)
    });
}

/// Whether one of `marks` covers piece `name`.
#[must_use]
pub fn covers(marks: &[Marked], name: &str) -> bool {
    find(marks, name).is_some()
}

/// The mark `name` falls under, compared as item names are ([`same_name`]): "Hustle" marked
/// covers both Hustles, "Hustle (weapon)" only the weapon one.
fn find<'m>(marks: &'m [Marked], name: &str) -> Option<&'m str> {
    marks
        .iter()
        .map(|m| m.name.as_str())
        .find(|n| same_name(n, name) && qualifier_fits(n, name))
}

/// `same_name` lets a qualified name match a bare one either way; a marked bare name may cover
/// qualified entries, but a marked "Hustle (weapon)" must not take a bare "Hustle" entry that
/// may be the armor.
fn qualifier_fits(marked: &str, entry: &str) -> bool {
    !marked.trim().ends_with(')') || entry.trim().ends_with(')')
}

/// Whether `name` stays; a marked one joins `took` (once).
fn kept(marks: &[Marked], name: &str, took: &mut Vec<String>) -> bool {
    let Some(marked) = find(marks, name) else {
        return true;
    };
    if !took.iter().any(|n| n == marked) {
        took.push(marked.to_owned());
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
        let text = set("not_needed = 1\n", "a", "Oath", false).context("a stray value")?;
        assert_eq!(parse(&text)?.0.get("a"), Some(&list(&["Oath"])), "{text}");
        let text = set("not_needed = { b = [\"Grief\"] }\n", "a", "Oath", false).context("inline")?;
        let (marked, warnings) = parse(&text)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            marked.get("b"),
            Some(&list(&["Grief"])),
            "inline entries kept: {text}"
        );
        assert_eq!(marked.get("a"), Some(&list(&["Oath"])));
        assert_eq!(set("[not_needed\n", "a", "Oath", false), None, "invalid TOML");
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

    fn marked(name: &str, build: &str) -> Marked {
        Marked {
            name: name.to_owned(),
            build: build.to_owned(),
        }
    }

    #[test]
    fn apply_takes_a_marked_charm_wish_out_by_its_name() -> Result<()> {
        let mut fury = Build {
            name: "Fury Druid".to_owned(),
            ..Build::default()
        };
        fury.charms = toml::from_str::<BTreeMap<String, Vec<crate::charms::CharmWish>>>(
            "[[charms]]\nname = \"Bone Break\"\nunique = true\n\
             [[charms]]\nname = \"FHR small charm\"\nsize = \"small\"\nstats = [{ stat = \"fhr\", min = 5 }]\n",
        )?
        .remove("charms")
        .unwrap_or_default();
        let mut builds = BTreeMap::from([("fury druid".to_owned(), fury)]);
        let file = NotNeeded::from([("fury druid".to_owned(), list(&["fhr small charm"]))]);
        let mut marks = marks(&file, &builds);
        apply(&mut marks, &mut builds, &mut []);
        let fury = builds.get("fury druid").context("the build stays")?;
        let charms: Vec<&str> = fury.charms.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(charms, ["Bone Break"]);
        assert_eq!(
            marks.get("fury druid"),
            Some(&vec![marked("fhr small charm", "Fury Druid")]),
            "kept for the undo"
        );
        Ok(())
    }

    #[test]
    fn apply_takes_marked_pieces_out_of_the_build_its_later_stages_and_farm_groups() -> Result<()> {
        let mut summoner = Build {
            name: "Summoner Druid".to_owned(),
            next: Some("fury druid".to_owned()),
            ..Build::default()
        };
        summoner
            .slots
            .insert("weapon".to_owned(), list(&["Spirit@25", "Grief@59"]));
        let mut fury = Build {
            name: "Fury Druid".to_owned(),
            recipes: vec![recipe("Grief"), recipe("Hustle"), recipe("Oath")],
            ..Build::default()
        };
        fury.slots.insert(
            "weapon".to_owned(),
            list(&["Grief@59", "Hustle (weapon)@39", "Oath@49"]),
        );
        fury.slots.insert("body".to_owned(), list(&["Hustle (armor)@39"]));
        fury.extras
            .insert("charms".to_owned(), list(&["Gheed's Fortune@62"]));
        fury.merc_slots.insert("weapon".to_owned(), list(&["Oath@49"]));
        let mut builds = BTreeMap::from([
            ("summoner druid".to_owned(), summoner),
            ("fury druid".to_owned(), fury),
        ]);
        let group = |target: &str, items: &[&str]| FarmGroup {
            target: target.to_owned(),
            items: items.iter().map(|&n| farm_item(n)).collect(),
        };
        let mut farm = vec![
            group("Summoner Druid", &["Grief", "Gheed's Fortune"]),
            group("Fury Druid", &["Grief", "Oath"]),
        ];
        let file = NotNeeded::from([
            ("summoner druid".to_owned(), list(&["Grief"])),
            (
                "fury druid".to_owned(),
                list(&[
                    "Hustle (weapon)",
                    "Gheeds Fortune",
                    "Oath",
                    "Nothing Of This Name",
                ]),
            ),
        ]);
        let mut marks = marks(&file, &builds);
        apply(&mut marks, &mut builds, &mut farm);
        let slots = |slug: &str, slot: &str| builds.get(slug).and_then(|b| b.slots.get(slot)).cloned();
        assert_eq!(slots("summoner druid", "weapon"), Some(list(&["Spirit@25"])));
        assert_eq!(
            slots("fury druid", "weapon"),
            None,
            "Grief from the Summoner mark, the rest own"
        );
        assert_eq!(slots("fury druid", "body"), Some(list(&["Hustle (armor)@39"])));
        let fury = builds.get("fury druid").context("the build stays")?;
        assert!(fury.extras.is_empty() && fury.merc_slots.is_empty(), "{fury:?}");
        let recipes: Vec<&str> = fury.recipes.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(recipes, ["Hustle"], "the armor Hustle still wants its recipe");
        let farm_names: Vec<Vec<&str>> = farm
            .iter()
            .map(|g| g.items.iter().map(|i| i.name.as_str()).collect())
            .collect();
        assert_eq!(
            farm_names,
            [vec!["Gheed's Fortune"], vec![]],
            "a later stage's mark doesn't reach back"
        );
        assert_eq!(
            marks.get("summoner druid"),
            Some(&vec![marked("Grief", "Summoner Druid")])
        );
        assert_eq!(
            marks.get("fury druid"),
            Some(&vec![
                marked("Hustle (weapon)", "Fury Druid"),
                marked("Gheeds Fortune", "Fury Druid"),
                marked("Oath", "Fury Druid"),
                marked("Nothing Of This Name", "Fury Druid"),
                marked("Grief", "Summoner Druid"),
            ]),
            "own marks stay for the undo, the inherited one names where it was made"
        );
        let mut bis = Bis {
            build: String::new(),
            slots: BTreeMap::from([("weapon".to_owned(), list(&["Oath@49", "Spirit@25"]))]),
            merc_slots: BTreeMap::from([("weapon".to_owned(), list(&["Oath@49"]))]),
        };
        bis_without(&mut bis, &[marked("Oath", "vex")]);
        assert_eq!(bis.slots.get("weapon"), Some(&list(&["Spirit@25"])));
        assert!(bis.merc_slots.is_empty());
        Ok(())
    }
}
