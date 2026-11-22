//! The `/config` page's JSON (each character's resolved `BiS` view, the build definitions) and
//! its comment-preserving edits of `bis.toml` / an old `config.toml`.

use std::collections::BTreeMap;

use toml_edit::DocumentMut;

use d2r_core::web::GearInfo;

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

/// The config page's JSON: every character of the config and of `played` once, most recently
/// played first, the never-seen ones after in config order.
#[must_use]
pub fn config_json(
    config: &crate::config::Settings,
    gear: &GearInfo,
    played: &d2r_core::played::Played,
) -> serde_json::Value {
    let mut characters: Vec<_> = character_keys(config, played)
        .into_iter()
        .map(|key| {
            let seen = played.get(&key);
            (seen.map(|e| e.t), character_json(config, gear, &key, seen))
        })
        .collect();
    characters.sort_by_key(|&(t, _)| std::cmp::Reverse(t));
    let characters: Vec<_> = characters.into_iter().map(|(_, json)| json).collect();
    serde_json::json!({ "characters": characters })
}

/// The lowercased character names of the config, then of `played`, each once.
fn character_keys(config: &crate::config::Settings, played: &d2r_core::played::Played) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for name in config
        .bis
        .keys()
        .chain(config.builds.names.keys())
        .chain(played.keys())
    {
        let key = name.to_lowercase();
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// One character's resolved view without live data: level unknown and nothing owned, so no
/// automatic stage switch.
fn character_json(
    config: &crate::config::Settings,
    gear: &GearInfo,
    key: &str,
    seen: Option<&d2r_core::played::Entry>,
) -> serde_json::Value {
    let class = seen.and_then(|e| e.class);
    let owns = crate::config::owns_nothing;
    let stage = config.stage(key, class, None, &owns, None);
    let resolved = config.resolved_bis(key, class, None, &owns, None);
    let (slots, merc_slots) = resolved.map_or_else(
        || (serde_json::json!({}), serde_json::json!({})),
        |bis| {
            (
                slot_tables_json(&bis.slots, &gear.equipped, gear),
                slot_tables_json(&bis.merc_slots, &gear.merc_equipped, gear),
            )
        },
    );
    let class_name = class
        .and_then(|c| usize::try_from(c).ok())
        .and_then(|c| d2r_core::config::CLASS_NAMES.get(c));
    let damages: Vec<_> = config
        .damages_for_player(key, class, None, &owns, None)
        .iter()
        .map(|d| d.name())
        .collect();
    serde_json::json!({
        "name": seen.map_or(key, |e| e.name.as_str()),
        "class": class_name,
        "build": config.bis.get(key).map_or("", |b| b.build.as_str()),
        "build_name": stage.map(|s| s.build.name.as_str()),
        "class_default": stage.is_some_and(|s| s.class_default),
        "damages": damages,
        "slots": slots,
        "merc_slots": merc_slots,
        "last_played": seen.map(|e| e.t),
    })
}

/// A resolved slot table as JSON: slot name → annotated pieces.
#[must_use]
pub fn slot_tables_json(
    slots: &BTreeMap<String, Vec<String>>,
    equipped: &[(u8, u32)],
    gear: &GearInfo,
) -> serde_json::Value {
    slots
        .iter()
        .map(|(slot, pieces)| {
            (
                slot.clone(),
                serde_json::json!(
                    pieces
                        .iter()
                        .map(|piece| piece_json(piece, slot, equipped, gear))
                        .collect::<Vec<_>>()
                ),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into()
}

/// One resolved `BiS` piece with its level, tick, whether the equipped piece is better, and a
/// runeword's `runes` and `bases`.
pub fn piece_json(piece: &str, slot: &str, equipped: &[(u8, u32)], gear: &GearInfo) -> serde_json::Value {
    let name = d2r_core::info::piece_name(piece);
    let mut json = serde_json::json!({
        "piece": piece,
        "name": name,
        "level": d2r_core::info::piece_level(piece),
        "tick": d2r_core::info::piece_tick(piece, &gear.worn, &gear.owned, &gear.stash)
            .map(d2r_core::info::Tick::as_str),
        "better": d2r_core::info::slot_equipped_better(slot, equipped),
    });
    if let Some(rw) = d2r_core::runewords_data::lookup(name)
        && let Some(obj) = json.as_object_mut()
    {
        obj.insert("runes".to_owned(), serde_json::json!(rw.runes));
        obj.insert("bases".to_owned(), serde_json::json!(rw.bases));
    }
    json
}

/// The `/api/builds` JSON: every build definition with its per-slot piece counts, by slug.
#[must_use]
pub fn builds_json(config: &crate::config::Settings) -> serde_json::Value {
    let counts = |slots: &BTreeMap<String, Vec<String>>| {
        slots
            .iter()
            .map(|(slot, pieces)| (slot.clone(), serde_json::json!(pieces.len())))
            .collect::<serde_json::Map<String, serde_json::Value>>()
    };
    let builds: Vec<_> = config
        .build_defs
        .iter()
        .map(|(slug, def)| {
            serde_json::json!({
                "slug": slug,
                "name": def.name,
                "damages": def.damages.iter().map(|d| d.name()).collect::<Vec<_>>(),
                "notes": def.notes,
                "slots": counts(&def.slots),
                "merc_slots": counts(&def.merc_slots),
            })
        })
        .collect();
    serde_json::json!({ "builds": builds })
}
