//! The `/config` page's JSON (the player's characters and their build selection, the builds
//! to follow) and its comment-preserving edits of `bis.toml` / an old `config.toml`.

use toml_edit::DocumentMut;

use crate::config::{Settings, playstyle_of, split_selection};

/// Sets (`Some(slug)`) or clears (`None`) the `[bis.<name>]` build selection in `text`, under
/// every case spelling of `name`; clearing drops an entry left empty. `None` when `text` is not
/// valid TOML or an entry is not a table, so the caller can report the failed save.
#[must_use]
pub fn set_bis_selection(text: &str, name: &str, build: Option<&str>) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    if build.is_some() && doc.get("bis").is_none() {
        doc.insert("bis", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let Some(bis) = doc.get_mut("bis") else {
        return Some(doc.to_string());
    };
    let bis = bis.as_table_mut()?;
    let mut keys = matching_keys(bis.iter().map(|(k, _)| k), name);
    if let Some(slug) = build {
        if keys.is_empty() {
            bis.insert(name, toml_edit::Item::Table(toml_edit::Table::new()));
            keys.push(name.to_owned());
        }
        for key in &keys {
            let entry = bis.get_mut(key).and_then(|e| e.as_table_mut())?;
            entry["build"] = toml_edit::value(slug);
        }
    } else {
        for key in &keys {
            let entry = bis.get_mut(key).and_then(|e| e.as_table_mut())?;
            entry.remove("build");
            if entry.is_empty() {
                bis.remove(key);
            }
        }
    }
    Some(doc.to_string())
}

/// The keys equal to `name` ignoring case.
pub fn matching_keys<'a>(keys: impl Iterator<Item = &'a str>, name: &str) -> Vec<String> {
    let name = name.to_lowercase();
    keys.filter(|k| k.to_lowercase() == name)
        .map(str::to_owned)
        .collect()
}

/// Removes character `name` (any case) from `text`: its `[bis.<name>]` tables and its
/// `[builds]` damage list. `None` when `text` is not valid TOML.
#[must_use]
pub fn remove_character(text: &str, name: &str) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    if let Some(bis) = doc.get_mut("bis").and_then(toml_edit::Item::as_table_like_mut) {
        for key in matching_keys(bis.iter().map(|(k, _)| k), name) {
            bis.remove(&key);
        }
    }
    if let Some(builds) = doc.get_mut("builds").and_then(toml_edit::Item::as_table_like_mut) {
        let entries = builds.iter().filter(|(_, v)| v.is_array()).map(|(k, _)| k);
        for key in matching_keys(entries, name) {
            builds.remove(&key);
        }
    }
    Some(doc.to_string())
}

/// The config page's JSON: the player's own characters (those in `played`, recorded only for
/// the local player and seeded from the save files; not the other players of a game), most
/// recently played first, each with the playstyle it follows among `folders` (folder name and
/// settings, the selected playstyle first, see [`playstyle_of`]); the playstyles (for their
/// loot filters); every "not needed" mark; and the money farming switch.
#[must_use]
pub fn config_json(folders: &[(&str, &Settings)], played: &d2r_core::played::Played) -> serde_json::Value {
    let mut seen: Vec<_> = played.iter().collect();
    seen.sort_by_key(|&(_, entry)| std::cmp::Reverse(entry.t));
    let characters: Vec<_> = seen
        .into_iter()
        .filter_map(|(key, entry)| {
            let (folder, config) = playstyle_of(folders, key, entry.class)?;
            Some(character_json(folder, config, key, entry))
        })
        .collect();
    let playstyles: Vec<_> = folders
        .iter()
        .map(|(key, config)| serde_json::json!({ "key": key, "name": config.name }))
        .collect();
    let selected = folders.first().map(|(_, config)| *config);
    serde_json::json!({
        "characters": characters,
        "playstyles": playstyles,
        "not_needed": selected.map_or_else(|| serde_json::json!([]), |config| not_needed_json(config, folders)),
        "money_farming": selected.is_none_or(|config| config.valuables.money),
    })
}

/// Every mark of `not-needed.toml` (as `config` read it) by the build (or character) it was
/// made for, in key order: `[{build, known, names}]`. A build or name that matches nothing in
/// any of `folders` any more is listed too (`known: false` for a build that is neither a
/// build nor a character), so it can be brought back. The inherited marks of later stages are
/// left out: they undo with their own build.
fn not_needed_json(config: &Settings, folders: &[(&str, &Settings)]) -> serde_json::Value {
    let groups: Vec<_> = config
        .not_needed
        .iter()
        .filter_map(|(key, marks)| {
            let own: Vec<_> = marks
                .iter()
                .filter(|m| m.build.eq_ignore_ascii_case(key))
                .collect();
            let build = &own.first()?.build;
            let known = folders.iter().any(|(_, c)| c.build_defs.contains_key(key))
                || config.bis.keys().any(|k| k.eq_ignore_ascii_case(key));
            let names: Vec<_> = own.iter().map(|m| m.name.as_str()).collect();
            Some(serde_json::json!({ "build": build, "known": known, "names": names }))
        })
        .collect();
    serde_json::json!(groups)
}

/// One character's playstyle (`folder`, `config`), build selection and current stage: the
/// stage it last reached live (`played.json`) when that lies on the selected chain, else the
/// chain's start. `build` is the selection as the build list's `value` names it
/// (`"<folder>/<slug>"`, [`builds_json`]) when the build is in its playstyle, else as saved.
fn character_json(
    folder: &str,
    config: &Settings,
    key: &str,
    seen: &d2r_core::played::Entry,
) -> serde_json::Value {
    let stage = config.stage(key, seen.class, None, &crate::config::owns_nothing, None);
    let reached = seen
        .stage
        .as_deref()
        .filter(|&reached| stage.is_some_and(|s| config.leads_to(s.slug, reached)))
        .and_then(|reached| config.build_defs.get(reached));
    let class_name = seen
        .class
        .and_then(|c| usize::try_from(c).ok())
        .and_then(|c| d2r_core::config::CLASS_NAMES.get(c));
    let saved = config.bis.get(key).map_or("", |b| b.build.as_str());
    let (_, slug) = split_selection(saved);
    let build = if config.build_defs.contains_key(&slug) {
        format!("{folder}/{slug}")
    } else {
        saved.to_owned()
    };
    serde_json::json!({
        "name": seen.name,
        "class": class_name,
        "playstyle": config.name,
        "build": build,
        "build_name": reached.or_else(|| stage.map(|s| s.build)).map(|b| b.name.as_str()),
        "class_default": stage.is_some_and(|s| s.class_default),
        "last_played": seen.t,
    })
}

/// The `/api/builds` JSON: every build a character can follow, per playstyle of `folders`
/// in order, by slug, with the `value` a selection saves (`"<folder>/<slug>"`), its class name
/// (`null`: any class), its stage chain's names (`["Fire Wake Assassin", "Trapsin"]`) and its
/// playstyle (`{key, name}`).
#[must_use]
pub fn builds_json(folders: &[(&str, &Settings)]) -> serde_json::Value {
    let builds: Vec<_> = folders
        .iter()
        .flat_map(|&(folder, config)| {
            config.build_defs.iter().map(move |(slug, def)| {
                let class = def
                    .class
                    .and_then(|c| usize::try_from(c).ok())
                    .and_then(|c| d2r_core::config::CLASS_NAMES.get(c));
                serde_json::json!({
                    "slug": slug,
                    "value": format!("{folder}/{slug}"),
                    "name": def.name,
                    "class": class,
                    "chain": config.stage_chain(slug),
                    "playstyle": { "key": folder, "name": config.name },
                })
            })
        })
        .collect();
    serde_json::json!({ "builds": builds })
}
