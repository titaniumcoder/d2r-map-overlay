//! The `/config` page's JSON (each character's resolved `BiS` view, the build definitions) and
//! its text edits of `bis.toml` / an old `config.toml` (the `[bis.<name>]` build selection,
//! removing a character; the files are picked by [`crate::files`]).

use std::collections::BTreeMap;

use toml_edit::DocumentMut;

use d2r_core::web::GearInfo;

/// Writes the `[bis.<name>]` build selection into `text` (comment-preserving): `Some(slug)`
/// sets `build = "<slug>"` and creates the `[bis]`/`[bis.<name>]` tables when missing;
/// `None` removes the selection again — the `build` key, and the whole entry when that leaves
/// it empty (nothing to clear is no change). The name matches case-insensitively and every
/// spelling present is written (`[bis.Fulmina]` and `[bis.fulmina]` both, so they can't
/// disagree; [`crate::config::parse_tables`] lowercases the keys). `None` (the return) when `text` is
/// not valid TOML or the `bis` entry is not a table: the save would silently do nothing, so
/// the caller must answer the browser with an error instead.
#[must_use]
pub fn set_bis_selection(text: &str, name: &str, build: Option<&str>) -> Option<String> {
    let mut doc = text.parse::<DocumentMut>().ok()?;
    if build.is_some() && doc.get("bis").is_none() {
        doc.insert("bis", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let Some(bis) = doc.get_mut("bis") else {
        // Clearing without any `[bis]`: nothing to do.
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
            // Same contract as the set path: a non-table `[bis.<name>]` would silently no-op.
            let entry = bis.get_mut(key).and_then(|e| e.as_table_mut())?;
            entry.remove("build");
            if entry.is_empty() {
                bis.remove(key);
            }
        }
    }
    Some(doc.to_string())
}

/// The keys equal to `name` ignoring case (as [`crate::config::parse_tables`] folds
/// them).
pub fn matching_keys<'a>(keys: impl Iterator<Item = &'a str>, name: &str) -> Vec<String> {
    let name = name.to_lowercase();
    keys.filter(|k| k.to_lowercase() == name)
        .map(str::to_owned)
        .collect()
}

/// Removes character `name` from `text` (comment-preserving, case-insensitive): every
/// `[bis.<name>]` table (with its slot sub-tables) and its `[builds]` damage entry. The
/// `[builds.class]` table is never an entry (only damage lists are removed). `None` when
/// `text` is not valid TOML.
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

/// The config page's JSON ([`api_config`]), pure over the parsed config, the live gear
/// state and the last-played times so tests need no file and no game.
///
/// Every character is listed once (names compared ignoring case): those in the config
/// (`[bis.<name>]` selections and `[builds]` damage entries) and every one in `played`. Each
/// carries:
/// - `name`: the played display name, else the config's (lowercased) name,
/// - `class`: the class name when the tracker saw it live (`null` otherwise),
/// - `build`: the selected build's slug as written in `[bis.<name>]` (`""` when none),
/// - `build_name`: the shown build's display name — the selection, else the class default
///   when the class is known (`null` when neither),
/// - `class_default`: whether `build_name` is the class default rather than a selection,
/// - `damages`: the resolved damage types (character-level `[builds]` entry, else the shown
///   build's, else the class's), as lowercased names,
/// - `slots`/`merc_slots`: the resolved per-slot piece lists — inline slots when the config
///   has any, else the shown build's ([`crate::config::Settings::resolved_bis`]) — each piece
///   annotated with its required level, its ✓ (`tick`: `"worn"`/`"owned"`/`"stash"`/`null`,
///   [`d2r_core::info::piece_tick`]), whether the equipped piece is already better (`better`,
///   [`d2r_core::info::slot_equipped_better`]) and, for a runeword piece, its `runes` and
///   `bases`,
/// - `last_played`: unix seconds the character was last seen in a game (`null` when never,
///   [`d2r_core::played`]).
///
/// Characters are listed most recently played first; those never seen follow in config order.
#[must_use]
pub fn config_json(
    config: &crate::config::Settings,
    gear: &GearInfo,
    played: &d2r_core::played::Played,
) -> serde_json::Value {
    // Lowercased names, config order first, each once.
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
    let mut characters: Vec<_> = keys
        .into_iter()
        .map(|key| {
            let seen = played.get(&key);
            let class = seen.and_then(|e| e.class);
            // No live data here: level unknown and nothing owned, so no automatic switch.
            let owns = crate::config::owns_nothing;
            let stage = config.stage(&key, class, None, &owns, None);
            let resolved = config.resolved_bis(&key, class, None, &owns, None);
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
                .damages_for_player(&key, class, None, &owns, None)
                .iter()
                .map(|d| d.name())
                .collect();
            let json = serde_json::json!({
                "name": seen.map_or(key.as_str(), |e| e.name.as_str()),
                "class": class_name,
                "build": config.bis.get(&key).map_or("", |b| b.build.as_str()),
                "build_name": stage.map(|s| s.build.name.as_str()),
                "class_default": stage.is_some_and(|s| s.class_default),
                "damages": damages,
                "slots": slots,
                "merc_slots": merc_slots,
                "last_played": seen.map(|e| e.t),
            });
            (seen.map(|e| e.t), json)
        })
        .collect();
    // Stable: equal times and the never-seen (`None`, last under `Reverse`) keep config order.
    characters.sort_by_key(|&(t, _)| std::cmp::Reverse(t));
    let characters: Vec<_> = characters.into_iter().map(|(_, json)| json).collect();
    serde_json::json!({ "characters": characters })
}

/// A resolved slot table as JSON: slot name → annotated pieces ([`piece_json`]).
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

/// One resolved `BiS` piece as JSON (see [`config_json`] for the fields).
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

/// The `/api/builds` JSON ([`api_builds`]), pure over the parsed config: every build
/// definition with its slug, display name, damages, notes and per-slot piece counts, sorted by
/// slug.
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
