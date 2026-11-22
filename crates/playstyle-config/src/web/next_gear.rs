//! The next stage's gear section (user request 2026-11-22: "a fury gear list while I walk the
//! summoner ... the ones that I could craft right now or have active are shown there and the
//! rest is under 'alternative'"): per gear-grid slot of the next build, the pieces the account
//! holds or can make now up front, every other one folded.

use d2r_core::guidance::Spot;
use d2r_core::offsets::item_data;
use d2r_core::runewords_data::Runeword;
use d2r_core::socket_bases::Wearer;

use super::gear::{
    GRID_COLUMNS, MERC_COLUMNS, RING_COLUMN_AT, add_piece_details, charm_wishes_json, ring_pieces,
};
use super::recipes::{owned_materials, rune_count};
use super::{PanelInfo, base_hint, is_worn, level_range};
use crate::config::Build;
use crate::stash_plan::Held;

/// What every piece of the section is judged against. `held` and `materials` are a ledger: a
/// piece found craftable takes its runes and its base out, so the pieces after it (display
/// order) can't count them again.
struct Next<'a> {
    info: &'a PanelInfo<'a>,
    build: &'a Build,
    held: Vec<Held>,
    materials: Vec<(String, u32)>,
}

/// The section, `null` while the current stage has no next one: `build`, `level` (the
/// switch level), `columns` and `merc` (`{title, pieces}`, empty slots left out), the next
/// build's `charms` and its pieces marked `not_needed` (already gone from its slots).
#[must_use]
pub fn next_gear_json(info: &PanelInfo) -> serde_json::Value {
    let Some(stage) = info.stage else {
        return serde_json::Value::Null;
    };
    let Some((_, build)) = stage.next else {
        return serde_json::Value::Null;
    };
    let mut next = Next {
        info,
        build,
        held: crate::stash_plan::held_from(info.live.owned_items, info.live.stash),
        materials: owned_materials(info),
    };
    let class = info.live.class;
    let offhand = build.slots.contains_key("offhand");
    // The player's columns in display order (title, pieces, body, wearer), then the merc's.
    let mut player: Vec<(&str, Vec<String>, u8, Wearer)> = GRID_COLUMNS
        .iter()
        .map(|&(title, slot, bodies)| {
            let wearer = if slot == "weapon" && offhand {
                Wearer::PlayerWithOffhand(class)
            } else {
                Wearer::Player(class)
            };
            let pieces = build.slots.get(slot).cloned().unwrap_or_default();
            (title, pieces, bodies.first().copied().unwrap_or(0), wearer)
        })
        .collect();
    player.insert(
        RING_COLUMN_AT,
        ("Ring", ring_pieces(Some(&build.slots)), 6, Wearer::Player(class)),
    );
    let columns: Vec<_> = player
        .iter()
        .map(|(title, pieces, body, wearer)| {
            (*title, pieces_json(pieces, |_| (Some(*body), *wearer), &mut next))
        })
        .collect();
    let merc: Vec<_> = MERC_COLUMNS
        .iter()
        .filter_map(|&(title, slot, body)| {
            let pieces = build.merc_slots.get(slot)?;
            let wearer = |piece: &str| (Some(body), Wearer::Merc(build.merc_wearer(piece, Some(body))));
            Some((title, pieces_json(pieces, wearer, &mut next)))
        })
        .collect();
    let cards = |list: Vec<(&str, Vec<serde_json::Value>)>| {
        list.into_iter()
            .filter(|(_, pieces)| !pieces.is_empty())
            .map(|(title, pieces)| serde_json::json!({ "title": title, "pieces": pieces }))
            .collect::<Vec<_>>()
    };
    let wishes: Vec<_> = build.charms.iter().map(|w| (w, None)).collect();
    serde_json::json!({
        "build": build.name,
        "level": stage.build.switch_level,
        "columns": cards(columns),
        "merc": cards(merc),
        "charms": charm_wishes_json(&wishes, Some(build), info),
        "not_needed": info.marks_of(&build.name),
    })
}

fn pieces_json(
    pieces: &[String],
    wearer: impl Fn(&str) -> (Option<u8>, Wearer),
    next: &mut Next,
) -> Vec<serde_json::Value> {
    pieces
        .iter()
        .filter_map(|piece| piece_json(piece, wearer(piece), next))
        .collect()
}

/// One piece: `ready` when held (`tick`: worn, owned or in the shared stash) or `craft` (a
/// runeword whose runes and a fitting socketed base are all still in the ledger, the base
/// named; both then leave the ledger), `at` when
/// its level is above the character's. A runeword not ready lists its `missing` runes
/// (`"Ber 0/1"`) and the held base (`base_owned`) or else where bases drop (`base_hint`). A
/// piece past its leveling window leaves unless held.
fn piece_json(
    piece: &str,
    (slot, wearer): (Option<u8>, Wearer),
    next: &mut Next,
) -> Option<serde_json::Value> {
    let info = next.info;
    let name = d2r_core::info::piece_name(piece);
    let req = d2r_core::info::piece_level(piece);
    let tick = held_tick(name, wearer, info);
    if tick.is_none() && outgrown(piece, next) {
        return None;
    }
    let word = d2r_core::runewords_data::lookup(name);
    let found = word.and_then(|w| held_base(w, slot, wearer, &next.held));
    let missing = word.map_or_else(Vec::new, |w| missing_runes(w, &next.materials));
    let craft = match (tick, word, &found) {
        (None, Some(w), Some((at, base))) if missing.is_empty() => {
            next.held.remove(*at);
            take_runes(w, &mut next.materials);
            // Socket contents are unread outside runewords: a filled base looks empty.
            Some(format!("craft: {} in {base}, if its sockets are empty", w.name))
        }
        _ => None,
    };
    let base = found.map(|(_, base)| base);
    let lvl = info.live.char_level.map(u32::from);
    let mut json = serde_json::json!({
        "piece": piece,
        "name": name,
        "level": req,
        "at": req.zip(lvl).is_some_and(|(req, lvl)| req > lvl),
        "state": "neutral",
        "tick": tick,
        "ready": tick.is_some() || craft.is_some(),
        "craft": craft,
    });
    add_piece_details(&mut json, name, Some(next.build));
    if let (Some(w), Some(obj)) = (word, json.as_object_mut()) {
        obj.insert("runes".to_owned(), serde_json::json!(w.runes));
        obj.insert("bases".to_owned(), serde_json::json!(w.bases));
        if tick.is_some() {
            // The missing-rune remark is moot once the runeword is made.
            obj.remove("notes");
        } else if craft.is_none() {
            obj.insert("missing".to_owned(), serde_json::json!(missing));
            obj.insert("base_owned".to_owned(), serde_json::json!(base));
            if base.is_none() {
                obj.insert(
                    "base_hint".to_owned(),
                    serde_json::json!(base_hint(w, info, slot, wearer)),
                );
            }
        }
    }
    Some(json)
}

/// Where a copy of piece `name` is: `"worn"` by its wearer, `"owned"` on the character (or
/// the other wearer), `"stash"` in the shared stash.
fn held_tick(name: &str, wearer: Wearer, info: &PanelInfo) -> Option<&'static str> {
    let worn = match wearer {
        Wearer::Merc(_) => super::gear::is_merc_worn(name, info),
        Wearer::Player(_) | Wearer::PlayerWithOffhand(_) => is_worn(name, info),
    };
    if worn {
        return Some("worn");
    }
    let (stashed, carried) = info.live.holdings().copies(name, None);
    if carried > 0 {
        Some("owned")
    } else if stashed > 0 {
        Some("stash")
    } else {
        None
    }
}

/// Whether the character is past the piece's leveling window in the next build (never for a
/// rune).
fn outgrown(piece: &str, next: &Next) -> bool {
    let Some(details) = next.build.pieces.get(d2r_core::info::piece_name(piece)) else {
        return false;
    };
    match (level_range(&details.setup), next.info.live.char_level) {
        (Some((_, to)), Some(lvl)) => details.kind != "rune" && u32::from(lvl) > to,
        _ => false,
    }
}

/// The runes of `word` held short of what it needs, as `"Ber 0/1"`, in socket order.
fn missing_runes(word: &Runeword, materials: &[(String, u32)]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, rune) in word.runes.iter().enumerate() {
        if word.runes.get(..i).is_some_and(|before| before.contains(rune)) {
            continue;
        }
        let need = word.runes.iter().filter(|r| *r == rune).count();
        let have = rune_count(rune, materials);
        if usize::try_from(have).is_ok_and(|have| have < need) {
            out.push(format!("{rune} {have}/{need}"));
        }
    }
    out
}

/// The first held white or superior base with exactly `word`'s socket count that can carry it
/// for `wearer` in body `slot`: its index in `held` and `"Loricated Mail 4os (base on page 1)"`.
fn held_base(word: &Runeword, slot: Option<u8>, wearer: Wearer, held: &[Held]) -> Option<(usize, String)> {
    held.iter().enumerate().find_map(|(i, h)| {
        let item = &h.item;
        let code = item.code.as_deref()?;
        let fits = matches!(
            item.quality,
            item_data::QUALITY_NORMAL | item_data::QUALITY_SUPERIOR
        ) && item.flags & item_data::FLAG_RUNEWORD == 0
            && usize::from(item.sockets) == word.runes.len()
            && d2r_core::socket_bases::fits(word, code, wearer, slot);
        if !fits {
            return None;
        }
        let base =
            d2r_core::item_levels_data::armor_name(code).or_else(|| d2r_core::weapons_data::name(code))?;
        let eth = if item.ethereal { "eth " } else { "" };
        let place = match h.spot {
            Spot::Page(n) => format!("on page {n}"),
            Spot::Inventory => "in the inventory".to_owned(),
            Spot::Cube => "in the cube".to_owned(),
            Spot::Stash => "in the stash".to_owned(),
        };
        Some((i, format!("{eth}{base} {}os (base {place})", item.sockets)))
    })
}

/// Takes one of each rune of `word` (repeats counted) out of `materials`.
fn take_runes(word: &Runeword, materials: &mut [(String, u32)]) {
    for rune in word.runes {
        if let Some((_, qty)) = materials
            .iter_mut()
            .find(|(name, qty)| *qty > 0 && super::gear::same_rune(name, rune))
        {
            *qty = qty.saturating_sub(1);
        }
    }
}
