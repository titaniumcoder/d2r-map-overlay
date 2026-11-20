//! The gear panel: the 2×5 `BiS` grid with the mercenary's slots and the build's extras, the
//! upgrades the character can make now (the page chimes on new ones), the attention flag that
//! opens the panel, and the vendor panel's notes ([`shop_notes`]).

use d2r_core::socket_bases::{MercAct, Wearer};
use d2r_core::web::ShopNote;

use super::recipes::{Input, need_totals, owned_materials};
use super::{
    PanelInfo, base_hint, equipped_level, is_worn, merc_base_hint, outgrown, piece_state, piece_window,
    slot_bodies,
};

/// The part of a mercenary slot's `pieces` (stored best→worst) still worth getting: the pieces
/// ranked above the first one the merc wears ([`d2r_core::web::LiveInfo::merc_worn`]; all of
/// them when it wears none, or in a group game where that list is empty).
#[must_use]
pub fn merc_open<'p>(pieces: &'p [String], info: &PanelInfo) -> &'p [String] {
    // `split` always yields a first part: the pieces before the first worn one (all of them
    // when none is worn).
    pieces
        .split(|p| is_merc_worn(d2r_core::info::piece_name(p), info))
        .next()
        .unwrap_or(pieces)
}

/// Whether the mercenary wears a piece named `name` ([`d2r_core::web::LiveInfo::merc_worn`]).
#[must_use]
pub fn is_merc_worn(name: &str, info: &PanelInfo) -> bool {
    info.live
        .merc_worn
        .iter()
        .any(|n| d2r_core::names::same_name(n, name))
}

/// The vendor panel's notes ([`ShopNote`]), one per item of the open trade window (empty
/// when none is open): the build slot whose current-stage piece the item is ([`shop_match`])
/// and why its stats beat what is worn in its body slot ([`crate::gear_score::verdict`] for
/// [`score_profile`]).
#[must_use]
pub fn shop_notes(info: &PanelInfo) -> Vec<ShopNote> {
    use crate::gear_score::{Piece, verdict};
    let Some(shop) = info.live.shop else {
        return Vec::new();
    };
    let profile = score_profile(info);
    let worn: Vec<(u8, Piece<'_>)> = info
        .live
        .equipped_items
        .iter()
        .map(|e| {
            let piece = Piece {
                code: e.code.as_deref(),
                stats: &e.stats,
            };
            (e.body, piece)
        })
        .collect();
    shop.items
        .iter()
        .map(|item| {
            let piece = Piece {
                code: item.code.as_deref(),
                stats: &item.stats,
            };
            ShopNote {
                slot: shop_match(item, info).map(|(slot, _)| slot),
                why: verdict(piece, item.req_level, &worn, &profile),
            }
        })
        .collect()
}

/// What the vendor panel's stat comparison values for this character
/// ([`crate::gear_score::Profile`]): its skills are the allocated ones plus the stage's skill
/// plan; its weapon kind is the worn weapon's, else that of the best current weapon piece
/// wearable at this level that names a base ([`crate::gear_score::piece_kind`]), else of any
/// current piece naming one; strength and dexterity are the base
/// values plus the worn gear's; Teleport is wanted until Enigma is held
/// ([`crate::gear_score::teleport_wanted`]).
pub fn score_profile(info: &PanelInfo) -> crate::gear_score::Profile {
    use crate::gear_score::{Profile, is_caster, piece_kind};
    use d2r_core::item_stats::stat;
    let gear = |id: u16| -> i32 {
        info.live
            .equipped_items
            .iter()
            .flat_map(|e| &e.stats)
            .filter(|s| s.id == id)
            .map(|s| s.value)
            .sum()
    };
    let mut skills: Vec<u16> = info.live.skills.points.iter().map(|&(id, _)| id).collect();
    for target in info.build.map_or(&[][..], |b| b.skill_plan.as_slice()) {
        let planned = d2r_core::skills_data::SKILLS.iter().find(|s| {
            u32::from(s.class) == info.live.class && d2r_core::names::same_name(s.name, &target.skill)
        });
        if let Some(s) = planned
            && !skills.contains(&s.id)
        {
            skills.push(s.id);
        }
    }
    let worn = info
        .live
        .equipped_items
        .iter()
        .find(|e| e.body == 4)
        .and_then(|e| d2r_core::bases_data::base(e.code.as_deref()?))
        .map(|b| b.kind);
    // The best current weapon piece wearable now names the kind; any current one otherwise.
    let planned = || {
        let pieces: Vec<&str> = info
            .bis?
            .slots
            .get("weapon")?
            .iter()
            .filter(|p| !outgrown(p, info))
            .map(String::as_str)
            .collect();
        let wearable = |p: &&str| {
            d2r_core::info::piece_level(p)
                .zip(info.live.char_level)
                .is_none_or(|(req, lvl)| req <= u32::from(lvl))
        };
        let kind = |p: &str| piece_kind(d2r_core::info::piece_name(p));
        pieces
            .iter()
            .copied()
            .filter(wearable)
            .find_map(kind)
            .or_else(|| pieces.iter().copied().find_map(kind))
    };
    let weapon = worn.or_else(planned);
    let attribute = |base: Option<u32>, id: u16| Some(i32::try_from(base?).ok()? + gear(id));
    Profile {
        class: info.live.class,
        level: info.live.char_level.map(u32::from),
        strength: attribute(info.live.strength, stat::STRENGTH),
        dexterity: attribute(info.live.dexterity, stat::DEXTERITY),
        skills,
        weapon,
        caster: is_caster(info.live.class, weapon),
        teleport: crate::gear_score::teleport_wanted(&info.live.holdings()),
    }
}

/// The build slot (`gloves`, `ring`, `merc helm`, …) and bare piece name of the first
/// current-stage piece the store `item` is ([`d2r_core::shop::matches`]): the character's slots
/// in [`d2r_core::config::SLOT_NAMES`] order, then the mercenary's; outgrown pieces are skipped,
/// and so are the mercenary's worn pieces and those ranked below them ([`merc_open`]).
#[must_use]
pub fn shop_match(item: &d2r_core::shop::ShopItem, info: &PanelInfo) -> Option<(String, String)> {
    let bis = info.bis?;
    let piece_in = |pieces: &[String]| {
        // `pieces` may be a merc slot cut at the merc's worn piece ([`merc_open`]).
        pieces
            .iter()
            .filter(|p| !outgrown(p, info))
            .map(|p| d2r_core::info::piece_name(p))
            .find(|name| d2r_core::shop::matches(item, name))
            .map(str::to_owned)
    };
    d2r_core::config::SLOT_NAMES
        .iter()
        .find_map(|slot| {
            let name = piece_in(bis.slots.get(*slot)?)?;
            let column = if slot.starts_with("ring") { "ring" } else { slot };
            Some((column.to_owned(), name))
        })
        .or_else(|| {
            bis.merc_slots
                .iter()
                .find_map(|(slot, pieces)| Some((format!("merc {slot}"), piece_in(merc_open(pieces, info))?)))
        })
}

/// The upgrades the character can make right now (`live_json`'s `upgrades` section), each
/// `{ "key", "kind", "slot", "name", "why" }`; the page chimes when a new `key` appears:
/// - `"equip"`: a current-stage piece of a character slot that is owned (✓ or ✓ stash),
///   wearable (required level ≤ character level), "better" in its slot ([`piece_state`]) and
///   not equipped already — the best such piece per slot;
/// - `"craft"`: a current-stage runeword recipe whose runes are all owned, whose piece is
///   wearable and "better" in its slot.
/// - `"shop"`: an item of the open trade window that is a current-stage piece
///   ([`shop_match`]; mercenary slots included, any required level) — buying ahead is fine.
///
/// The mercenary's slots get no `"equip"` or `"craft"` entries: its items' required levels are
/// not read (only their names, and only in solo games), so there is nothing to compare an
/// owned piece against; a `"shop"` entry never names a piece the merc wears or one ranked
/// below it ([`shop_match`]).
#[must_use]
pub fn upgrades_json(info: &PanelInfo) -> Vec<serde_json::Value> {
    let (Some(bis), Some(lvl)) = (info.bis, info.live.char_level) else {
        return Vec::new();
    };
    let lvl = u32::from(lvl);
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for slot in d2r_core::config::SLOT_NAMES {
        // The swap set is unread (its column mirrors the active weapon), so a swap piece
        // would be compared against the main weapon: no equip upgrades for it (review).
        if slot == "swap" {
            continue;
        }
        let Some(pieces) = bis.slots.get(slot) else {
            continue;
        };
        let column = if slot.starts_with("ring") { "ring" } else { slot };
        let eq_level = equipped_level(slot_bodies(slot), info);
        let found = pieces.iter().filter(|p| !outgrown(p, info)).find_map(|piece| {
            let name = d2r_core::info::piece_name(piece);
            let req = d2r_core::info::piece_level(piece)?;
            // Worn pieces are filtered by `is_worn` below, so no worn names here.
            let tick = d2r_core::info::piece_tick(piece, &[], info.live.owned, info.live.stash_owned)?;
            (req <= lvl && !is_worn(name, info) && piece_state(Some(req), eq_level, lvl) == "better").then(
                || {
                    let place = match tick {
                        d2r_core::info::Tick::Worn | d2r_core::info::Tick::Owned => "you have it",
                        d2r_core::info::Tick::Stash => "in the shared stash",
                    };
                    let versus = eq_level.map_or_else(String::new, |eq| format!(" (equipped: lvl {eq})"));
                    (name, format!("{place} · lvl {req}{versus}"))
                },
            )
        });
        if let Some((name, why)) = found {
            let key = format!("equip:{column}:{name}");
            if !seen.contains(&key) {
                seen.push(key.clone());
                out.push(serde_json::json!({
                    "key": key, "kind": "equip", "slot": column, "name": name, "why": why,
                }));
            }
        }
    }
    let materials = owned_materials(info);
    for recipe in info.build.map_or(&[][..], |b| b.recipes.as_slice()) {
        if recipe.kind != "runeword" {
            continue;
        }
        let slot_piece = d2r_core::config::SLOT_NAMES.iter().find_map(|slot| {
            bis.slots.get(*slot)?.iter().find_map(|p| {
                d2r_core::names::same_name(d2r_core::info::piece_name(p), &recipe.name).then_some((*slot, p))
            })
        });
        let Some((slot, piece)) = slot_piece.filter(|(_, p)| !outgrown(p, info)) else {
            continue;
        };
        let req = d2r_core::info::piece_level(piece);
        let inputs: Vec<Input> = recipe.inputs.iter().map(|t| Input::parse(t)).collect();
        let runes_owned = need_totals(&inputs).iter().all(|(key, need)| {
            inputs
                .iter()
                .find(|i| i.need().is_some_and(|(k, _)| k == *key))
                .and_then(|i| i.have(&materials))
                .is_some_and(|have| have >= *need)
        });
        let has_runes = inputs.iter().any(|i| matches!(i, Input::Material { .. }));
        let column = if slot.starts_with("ring") { "ring" } else { slot };
        if has_runes
            && runes_owned
            && !is_worn(&recipe.name, info)
            && req.is_some_and(|r| r <= lvl)
            && piece_state(req, equipped_level(slot_bodies(slot), info), lvl) == "better"
        {
            let base = recipe
                .inputs
                .iter()
                .zip(&inputs)
                .filter(|(_, i)| **i == Input::Base)
                .map(|(t, _)| t.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            let why = if base.is_empty() {
                "all runes owned".to_owned()
            } else {
                format!("all runes owned · base: {base}")
            };
            out.push(serde_json::json!({
                "key": format!("craft:{column}:{}", recipe.name),
                "kind": "craft",
                "slot": column,
                "name": recipe.name,
                "why": why,
            }));
        }
    }
    shop_upgrades(info, lvl, &mut out);
    out
}

/// Appends the `"shop"` upgrades ([`upgrades_json`]) to `out`: one per current-stage piece
/// that an open-window item is (the first such item names the entry) ([`shop_match`]), with the vendor and the required level (`lvl` =
/// the character level) in `why`.
pub fn shop_upgrades(info: &PanelInfo, lvl: u32, out: &mut Vec<serde_json::Value>) {
    for item in info.live.shop.map_or(&[][..], |shop| shop.items.as_slice()) {
        let Some((slot, piece)) = shop_match(item, info) else {
            continue;
        };
        // Keyed per build piece, not per item: differently-affixed sashes chime once.
        let key = format!("shop:{slot}:{piece}");
        if out
            .iter()
            .any(|u| u.get("key").and_then(|k| k.as_str()) == Some(key.as_str()))
        {
            continue;
        }
        let vendor = info
            .live
            .shop
            .and_then(|s| s.npc.as_deref())
            .unwrap_or("the vendor");
        let req = item.req_level.filter(|&r| r > 0).map_or_else(String::new, |r| {
            let note = if r > lvl { " — above your level" } else { "" };
            format!(" · lvl {r}{note}")
        });
        out.push(serde_json::json!({
            "key": key, "kind": "shop", "slot": slot, "name": item.name,
            "why": format!("{vendor} sells it{req}"),
        }));
    }
}

/// Why the dashboard's Gear panel should open on its own (`live_json`'s `gear_attention` and
/// `gear_attention_why`; user request, 2026-11-20), `None` to keep it collapsed. Only the
/// character's own slots count (the swap slot is unread, the mercenary's never trigger it):
/// - an owned or shared-stash piece to wear now: the `"equip"` entries of `upgrades`
///   ([`upgrades_json`]) → `"▲ upgrade to wear: <name>"`;
/// - else unowned unique, set or runeword pieces findable at the character's level
///   ([`findable_upgrade`]) → `"▲ findable: <names>"`.
pub fn gear_attention(info: &PanelInfo, upgrades: &[serde_json::Value]) -> Option<String> {
    let wear: Vec<&str> = upgrades
        .iter()
        .filter(|u| u.get("kind").and_then(serde_json::Value::as_str) == Some("equip"))
        .filter_map(|u| u.get("name")?.as_str())
        .collect();
    if !wear.is_empty() {
        return Some(format!("▲ upgrade to wear: {}", wear.join(", ")));
    }
    let (Some(bis), Some(lvl)) = (info.bis, info.live.char_level) else {
        return None;
    };
    // The highest monster level the character farms right now.
    let max_mlvl = u32::from(d2r_core::info::reachable_level(
        info.live.char_level,
        info.live.difficulty,
    )?);
    let materials = owned_materials(info);
    let mut found: Vec<&str> = Vec::new();
    for slot in d2r_core::config::SLOT_NAMES.iter().filter(|&&s| s != "swap") {
        let eq_level = equipped_level(slot_bodies(slot), info);
        for piece in bis.slots.get(*slot).map_or(&[][..], Vec::as_slice) {
            let name = d2r_core::info::piece_name(piece);
            if !found.iter().any(|f| d2r_core::names::same_name(f, name))
                && !outgrown(piece, info)
                && findable_upgrade(piece, u32::from(lvl), max_mlvl, eq_level, &materials, info)
            {
                found.push(name);
            }
        }
    }
    (!found.is_empty()).then(|| format!("▲ findable: {}", found.join(", ")))
}

/// Whether build piece `piece` is an unowned upgrade findable now: not worn, owned or in the
/// shared stash; its required level known and ≤ `lvl`; better than the equipped piece
/// ([`piece_state`] against `eq_level`); and droppable where the character farms (`max_mlvl`
/// = the highest monster level it farms, [`d2r_core::info::reachable_level`]):
/// - a unique or set item: its quality level ≤ `max_mlvl` ([`unique_set_qlvl`]);
/// - a runeword: every rune not already owned (`materials`, [`owned_materials`]: the shared
///   materials tab plus inventory, cube and stash) has a quality level ≤ `max_mlvl`
///   (misc.txt `level`); the base can't be checked and counts as findable.
///
/// Anything else (a magic, rare or white base described in words) is never "findable": there is
/// no item level to check it by.
#[must_use]
pub fn findable_upgrade(
    piece: &str,
    lvl: u32,
    max_mlvl: u32,
    eq_level: Option<u32>,
    materials: &[(String, u32)],
    info: &PanelInfo,
) -> bool {
    let name = d2r_core::info::piece_name(piece);
    let Some(req) = d2r_core::info::piece_level(piece) else {
        return false;
    };
    let unowned = d2r_core::info::piece_tick(piece, &[], info.live.owned, info.live.stash_owned).is_none()
        && !is_worn(name, info);
    if !unowned || req > lvl || piece_state(Some(req), eq_level, lvl) != "better" {
        return false;
    }
    if let Some(word) = d2r_core::runewords_data::lookup(name) {
        return word.runes.iter().all(|rune| {
            let need = word.runes.iter().filter(|r| *r == rune).count();
            let have = materials
                .iter()
                .filter(|(n, _)| same_rune(n, rune))
                .map(|(_, q)| *q)
                .sum::<u32>();
            usize::try_from(have).is_ok_and(|have| have >= need)
                || d2r_core::item_levels_data::rune_qlvl(rune).is_some_and(|q| q <= max_mlvl)
        });
    }
    unique_set_qlvl(name).is_some_and(|q| q <= max_mlvl)
}

/// Whether material name `material` (`"Tal Rune"`) is rune `rune` (`"Tal"`).
#[must_use]
pub fn same_rune(material: &str, rune: &str) -> bool {
    material
        .trim()
        .strip_suffix(" Rune")
        .is_some_and(|bare| bare.eq_ignore_ascii_case(rune))
}

/// The quality level of the unique or set item shown in game as `name` (the lowest when
/// several rows share it), `None` for any other name. The in-game name maps back to the
/// tables' `index` name through `DISPLAY_NAMES` (Biggin's Bonnet is `War Bonnet` there).
pub fn unique_set_qlvl(name: &str) -> Option<u32> {
    use d2r_core::item_names_data::{DISPLAY_NAMES, SET_NAMES, UNIQUE_NAMES};
    let index = DISPLAY_NAMES
        .iter()
        .find(|(_, shown)| d2r_core::names::same_name(shown, name))
        .map_or(name, |&(index, _)| index);
    let rows = |table: &'static [(u32, &'static str)]| {
        table
            .iter()
            .filter(move |(_, n)| d2r_core::names::same_name(n, index))
            .map(|&(row, _)| row)
    };
    rows(UNIQUE_NAMES)
        .filter_map(d2r_core::item_levels_data::unique_qlvl)
        .chain(rows(SET_NAMES).filter_map(d2r_core::item_levels_data::set_qlvl))
        .min()
}

/// The 2×5 gear grid (`live_json`'s `gear` section): `{ "columns": [...] }` — the dashboard
/// lays them out two rows of five, in the given order (Helm, Body, Hands, Belt, Foot /
/// Amulet, Ring, Weapons, Off-Hand, Swap; the user's row split). Each column is
/// `{ "title", "equipped": [...], "note": null, "pieces": [...] }` where `equipped` holds
/// the equipped piece(s) in that slot — two for the merged Ring column — as
/// `{ "name", "level" }` ([`equipped_name`], [`d2r_core::game::Equipped::required_level`]),
/// `note` is the off-hand's two-handed remark, and `pieces` are the build's pieces for that
/// slot ([`grid_pieces`]).
#[must_use]
pub fn gear_json(info: &PanelInfo) -> serde_json::Value {
    // (column title, build slot name, `ItemData::BODY` slots the header reads). The Swap
    // column shares body 4 with Weapons: only the active weapon set is equipped, and its
    // items carry body 4 — the inactive set's pieces are unread.
    const COLUMNS: [(&str, &str, &[u8]); 9] = [
        ("Helm", "helm", &[1]),
        ("Body", "body", &[3]),
        ("Hands", "gloves", &[10]),
        ("Belt", "belt", &[8]),
        ("Foot", "boots", &[9]),
        ("Amulet", "amulet", &[2]),
        ("Weapons", "weapon", &[4]),
        ("Off-Hand", "offhand", &[5]),
        ("Swap", "swap", &[4]),
    ];
    let mut columns: Vec<_> = COLUMNS
        .iter()
        .map(|&(title, slot, bodies)| {
            grid_column(title, info.bis.and_then(|b| b.slots.get(slot)), bodies, info)
        })
        .collect();
    // The Ring column merges ring1 and ring2 (the build stores them as two slots): pieces
    // from both, deduplicated, in stored order (best→worst).
    let mut ring_pieces: Vec<String> = Vec::new();
    for slot in ["ring1", "ring2"] {
        for piece in info
            .bis
            .and_then(|b| b.slots.get(slot))
            .map_or(&[][..], Vec::as_slice)
        {
            if !ring_pieces.contains(piece) {
                ring_pieces.push(piece.clone());
            }
        }
    }
    let ring = grid_column("Ring", Some(&ring_pieces), &[6, 7], info);
    columns.insert(6, ring);
    // The mercenary's slots (no header: its items' levels and slots are not read) and the
    // build's extras (charms, jewels, runes), with the same per-piece state and details. The
    // merc's worn names (solo games only) tick its pieces ✓; the worn piece's build level
    // stands in for the equipped requirement the player's columns compare against, and the
    // pieces ranked below it are beaten as well.
    let merc: Vec<_> = [
        ("Merc weapon", "weapon", 4),
        ("Merc off-hand", "offhand", 5),
        ("Merc body", "body", 3),
        ("Merc helm", "helm", 1),
    ]
    .iter()
    .filter_map(|&(title, slot, body)| {
        let pieces = info.bis.and_then(|b| b.merc_slots.get(slot))?;
        // The mercenary of the stage's first act (Act 2 without a build file): no class-only
        // bases but its act's, its act's weapons only; a runeword's hint is for the act that
        // carries it ([`merc_base_hint`]).
        let act = info
            .build
            .map_or_else(MercAct::default, crate::config::Build::merc_act);
        let worn_level = pieces
            .iter()
            .filter(|p| is_merc_worn(d2r_core::info::piece_name(p), info))
            .filter_map(|p| d2r_core::info::piece_level(p))
            .max();
        let mut json = grid_pieces(
            pieces,
            worn_level,
            info.live.merc_worn,
            (Some(body), Wearer::Merc(act)),
            info,
        );
        beat_below_worn(&mut json);
        Some(serde_json::json!({ "title": title, "pieces": json }))
    })
    .collect();
    let extras: Vec<_> = info
        .build
        .map(|b| &b.extras)
        .into_iter()
        .flatten()
        .map(|(group, pieces)| {
            serde_json::json!({
                "title": group,
                "pieces": grid_pieces(pieces, None, &[], (None, Wearer::Player(info.live.class)), info),
            })
        })
        .collect();
    serde_json::json!({
        "columns": columns,
        "build": info.bis.map(|b| b.build.as_str()),
        "targets": info.build.map_or(&[][..], |b| b.targets.as_slice()),
        "stats_plan": info.build.map_or(&[][..], |b| b.stats_plan.as_slice()),
        "tips": tips_json(info),
        "merc": merc,
        "extras": extras,
    })
}

/// Marks the grid pieces ranked below the first worn one (`tick` `"worn"`) as beaten: state
/// `"worse"` and no `now` badge, whatever their level.
pub fn beat_below_worn(pieces: &mut [serde_json::Value]) {
    let Some(first) = pieces
        .iter()
        .position(|p| p.get("tick").and_then(serde_json::Value::as_str) == Some("worn"))
    else {
        return;
    };
    for obj in pieces
        .iter_mut()
        .skip(first + 1)
        .filter_map(serde_json::Value::as_object_mut)
    {
        obj.insert("state".to_owned(), serde_json::json!("worse"));
        obj.remove("now");
    }
}

/// The current stage's tips for the character level, as `{ "levels", "text" }` (all of them
/// while the level is unknown).
#[must_use]
pub fn tips_json(info: &PanelInfo) -> Vec<serde_json::Value> {
    info.build
        .map_or(&[][..], |b| b.tips.as_slice())
        .iter()
        .filter(|t| info.live.char_level.is_none_or(|l| (t.from..=t.to).contains(&l)))
        .map(|t| {
            let levels = if t.from == t.to {
                t.from.to_string()
            } else {
                format!("{}-{}", t.from, t.to)
            };
            serde_json::json!({ "levels": levels, "text": t.text })
        })
        .collect()
}

/// One gear-grid column: the equipped header piece(s) for `bodies`, the build's `pieces`
/// with their server-side state, and the off-hand's two-handed note.
pub fn grid_column(
    title: &str,
    pieces: Option<&Vec<String>>,
    bodies: &[u8],
    info: &PanelInfo,
) -> serde_json::Value {
    let equipped = |bodies: &[u8]| {
        bodies
            .iter()
            .filter_map(|&body| info.live.equipped_items.iter().find(|e| e.body == body))
            .collect::<Vec<&d2r_core::game::Equipped>>()
    };
    let header = equipped(bodies);
    // The weapon beside off-hand pieces must be one-handed (as in the item plan's base needs).
    let class = info.live.class;
    let offhand = info.bis.is_some_and(|b| b.slots.contains_key("offhand"));
    let wearer = if title == "Weapons" && offhand {
        Wearer::PlayerWithOffhand(class)
    } else {
        Wearer::Player(class)
    };
    // The column's comparison base: the strongest equipped piece's requirement. Two rings
    // with different levels compare against the higher one (a piece better than one ring
    // but worse than the other is still worth listing as better).
    let equipped_level = equipped_level(bodies, info);
    // The off-hand is unusable while a two-handed weapon is equipped (weapons.txt
    // `2handed`, looked up by the weapon's item code).
    let note = (title == "Off-Hand")
        .then(|| {
            equipped(&[4])
                .into_iter()
                .next()
                .and_then(|w| w.code.as_deref())
                .and_then(d2r_core::weapons_data::two_handed)
                .filter(|&two| two)
                .map(|_| "(two-handed weapon equipped)")
        })
        .flatten();
    serde_json::json!({
        "title": title,
        "equipped": header
            .iter()
            .map(|e| serde_json::json!({ "name": equipped_name(e), "level": e.required_level() }))
            .collect::<Vec<_>>(),
        "note": note,
        "pieces": grid_pieces(
            pieces.map_or(&[][..], Vec::as_slice),
            equipped_level,
            &d2r_core::game::worn_names(info.live.equipped_items),
            (bodies.first().copied(), wearer),
            info,
        ),
    })
}

/// The build's pieces of one grid column with their state, best→worst (they are stored
/// ordered). `state`:
/// - `"worse"`: the piece's required level is at or below the equipped piece's (grey +
///   strike-through) — not worth chasing even when above the level gate;
/// - `"too_high"`: it would be better but is above the clvl + 5 gate (grey, no strike);
/// - `"better"`: its level is above the equipped piece's, or the slot has no comparable
///   equipped piece (nothing equipped, or an unnameable one — a runeword or an
///   unidentified unique/set), or the piece's level is unknown; never struck through.
///
/// A column whose equipped piece is unnameable therefore shows no strike-throughs.
///
/// A worn piece (`worn`) is shown even when outgrown, and a worn runeword drops its build
/// `notes` (the missing-rune remark is moot once it is made). A `"worse"` piece never gets
/// the `now` ("wear now") badge.
///
/// A runeword piece also gets a `base_hint` ([`base_hint`]) filtered by the column's body
/// `slot` and its `wearer` ([`Wearer`]).
#[must_use]
pub fn grid_pieces(
    pieces: &[String],
    equipped_level: Option<u32>,
    worn: &[String],
    (slot, wearer): (Option<u8>, Wearer),
    info: &PanelInfo,
) -> Vec<serde_json::Value> {
    let lvl = u32::from(info.live.char_level.unwrap_or(0));
    pieces
        .iter()
        .filter_map(|piece| {
            let tick = d2r_core::info::piece_tick(piece, worn, info.live.owned, info.live.stash_owned);
            let is_worn = tick == Some(d2r_core::info::Tick::Worn);
            if outgrown(piece, info) && !is_worn {
                return None;
            }
            let (range, rune) = piece_window(piece, info);
            let req = d2r_core::info::piece_level(piece);
            let state = piece_state(req, equipped_level, lvl);
            let mut json = serde_json::json!({
                "piece": piece,
                "name": d2r_core::info::piece_name(piece),
                "level": req,
                "state": state,
                "tick": tick.map(d2r_core::info::Tick::as_str),
            });
            let runeword = d2r_core::runewords_data::lookup(d2r_core::info::piece_name(piece));
            if let Some(rw) = runeword
                && let Some(obj) = json.as_object_mut()
            {
                obj.insert("runes".to_owned(), serde_json::json!(rw.runes));
                obj.insert("bases".to_owned(), serde_json::json!(rw.bases));
                let hint = match wearer {
                    Wearer::Merc(_) => merc_base_hint(rw, info, slot, info.build),
                    _ => base_hint(rw, info, slot, wearer),
                };
                obj.insert("base_hint".to_owned(), serde_json::json!(hint));
            }
            add_piece_details(&mut json, d2r_core::info::piece_name(piece), info.build);
            if is_worn
                && runeword.is_some()
                && let Some(obj) = json.as_object_mut()
            {
                obj.remove("notes");
            }
            if let (Some((from, to)), Some(obj)) = (range, json.as_object_mut()) {
                if rune {
                    // The rune's own requirement when known; the guide window only says when
                    // the guide starts using it.
                    let min = req.unwrap_or(from);
                    obj.insert("setup".to_owned(), serde_json::json!(format!("lvl {min}+")));
                } else if info.live.char_level.is_some()
                    && state != "worse"
                    && (from..=to).contains(&lvl)
                    // In the guide's window but above the piece's own requirement: not yet.
                    && req.is_none_or(|r| r <= lvl)
                {
                    obj.insert("now".to_owned(), serde_json::json!(true));
                }
            }
            Some(json)
        })
        .collect()
}

/// Adds the build file's details of piece `name` (`[pieces."<name>"]`) to its JSON: `kind`,
/// `setup`, `base`, `stats`, `why`, `notes` — each only when set, so the page can test for
/// presence.
pub fn add_piece_details(json: &mut serde_json::Value, name: &str, build: Option<&crate::config::Build>) {
    let Some(info) = build.and_then(|b| b.pieces.get(name)) else {
        return;
    };
    let Some(obj) = json.as_object_mut() else { return };
    for (key, value) in [
        ("kind", &info.kind),
        ("setup", &info.setup),
        ("base", &info.base),
        ("why", &info.why),
        ("notes", &info.notes),
    ] {
        if !value.is_empty() {
            obj.insert(key.to_owned(), serde_json::json!(value));
        }
    }
    if !info.stats.is_empty() {
        obj.insert("stats".to_owned(), serde_json::json!(info.stats));
    }
}

/// The name shown for an equipped piece in a grid column header: the identified unique/set
/// or runeword name, else (normal/superior quality) the base item's name from the armor/weapons
/// tables, else — for a runeword whose runes couldn't be read or matched, and
/// for magic/rare (the real name comes from the affixes) — the placeholders below.
#[must_use]
pub fn equipped_name(equipped: &d2r_core::game::Equipped) -> String {
    if let Some(name) = &equipped.name {
        return name.clone();
    }
    if equipped.runeword {
        return "runeword (unnamed)".to_owned();
    }
    if matches!(
        equipped.quality,
        d2r_core::offsets::item_data::QUALITY_NORMAL | d2r_core::offsets::item_data::QUALITY_SUPERIOR
    ) && let Some(code) = equipped.code.as_deref()
    {
        let base =
            d2r_core::item_levels_data::armor_name(code).or_else(|| d2r_core::weapons_data::name(code));
        if let Some(name) = base {
            return name.to_owned();
        }
    }
    "(unknown)".to_owned()
}
