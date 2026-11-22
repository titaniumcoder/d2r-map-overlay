//! The gear panel: the 2×5 `BiS` grid with the mercenary's slots and the build's extras, the
//! upgrades the character can make now, the attention flag that opens the panel, and the vendor
//! panel's notes.

use d2r_core::runewords_data::Runeword;
use d2r_core::socket_bases::{MercAct, Wearer};
use d2r_core::web::ShopNote;

use super::recipes::{Input, need_totals, owned_materials};
use super::{
    PanelInfo, base_hint, equipped_level, is_worn, merc_base_hint, outgrown, piece_state, piece_window,
    slot_bodies,
};
use crate::config::{Bis, Recipe};

/// The gear grid's columns in the user's two rows of five (the Ring column goes in at
/// [`RING_COLUMN_AT`]): title, build slot, `ItemData::BODY` slots of its header. Swap shares
/// body 4 with Weapons: only the active weapon set is read.
const GRID_COLUMNS: [(&str, &str, &[u8]); 9] = [
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

const RING_COLUMN_AT: usize = 6;

/// The mercenary's grid columns: title, build slot, body slot.
const MERC_COLUMNS: [(&str, &str, u8); 4] = [
    ("Merc weapon", "weapon", 4),
    ("Merc off-hand", "offhand", 5),
    ("Merc body", "body", 3),
    ("Merc helm", "helm", 1),
];

/// The dashboard column of a build slot: both ring slots share one.
fn column_name(slot: &str) -> &str {
    if slot.starts_with("ring") { "ring" } else { slot }
}

/// The pieces of a mercenary slot (best first) ranked above the first one the merc wears.
#[must_use]
pub fn merc_open<'p>(pieces: &'p [String], info: &PanelInfo) -> &'p [String] {
    pieces
        .split(|p| is_merc_worn(d2r_core::info::piece_name(p), info))
        .next()
        .unwrap_or(pieces)
}

#[must_use]
pub fn is_merc_worn(name: &str, info: &PanelInfo) -> bool {
    info.live
        .merc_worn
        .iter()
        .any(|n| d2r_core::names::same_name(n, name))
}

/// The vendor panel's notes, one per item of the open trade window: the build slot it fills
/// and why its stats beat what is worn.
#[must_use]
pub fn shop_notes(info: &PanelInfo) -> Vec<ShopNote> {
    use crate::gear_score::{Piece, verdict};
    let Some(shop) = info.live.shop else {
        return Vec::new();
    };
    let profile = score_profile(info);
    let plan = shop_plan(info);
    let worn = worn_pieces(info);
    shop.items
        .iter()
        .map(|item| {
            let piece = Piece {
                code: item.code.as_deref(),
                stats: &item.stats,
            };
            ShopNote {
                slot: shop_match(item, info).map(|(slot, _)| slot),
                why: verdict(piece, item.req_level, &worn, &profile, plan.as_ref()),
                mark: None,
            }
        })
        .collect()
}

/// The equipped items as `(body slot, piece)` for [`crate::gear_score`].
fn worn_pieces<'i>(info: &PanelInfo<'i>) -> Vec<(u8, crate::gear_score::Piece<'i>)> {
    info.live
        .equipped_items
        .iter()
        .map(|e| {
            let piece = crate::gear_score::Piece {
                code: e.code.as_deref(),
                stats: &e.stats,
            };
            (e.body, piece)
        })
        .collect()
}

/// An owned piece measured against what it would replace.
enum Versus {
    /// Worth wearing ([`crate::gear_score::verdict`]), with its gains.
    Better(String),
    /// Scores [`crate::gear_score::MIN_GAIN`] or more below what it would replace.
    Worse,
    Even,
}

/// The best-scoring owned copy (carried, cube, stash, shared stash) of piece `name` with read
/// stats against the worn gear; `None` without such a copy or when it can't be compared. The
/// vendor's rules without its build plan: no lost FCR/FHR breakpoint, [`MIN_GAIN`] to be better.
///
/// [`MIN_GAIN`]: crate::gear_score::MIN_GAIN
fn owned_versus_worn(
    name: &str,
    req: Option<u32>,
    worn: &[(u8, crate::gear_score::Piece<'_>)],
    profile: &crate::gear_score::Profile,
    info: &PanelInfo,
) -> Option<Versus> {
    use crate::gear_score::{MIN_GAIN, Piece, replaced_score, scores, verdict};
    let points = |p: Piece<'_>| scores(p, profile).0;
    let copy = info
        .live
        .owned_items
        .iter()
        .map(|(item, _)| item)
        .filter(|item| {
            !item.stats.is_empty() && d2r_core::names::same_name(&d2r_core::names::display_name(item), name)
        })
        .map(|item| Piece {
            code: item.code.as_deref(),
            stats: &item.stats,
        })
        .max_by(|a, b| points(*a).total_cmp(&points(*b)))?;
    if let Some(why) = verdict(copy, req, worn, profile, None) {
        return Some(Versus::Better(why));
    }
    let old = replaced_score(copy, worn, profile)?;
    Some(if points(copy) + MIN_GAIN <= old {
        Versus::Worse
    } else {
        Versus::Even
    })
}

/// Whether build piece `name` is one of the guides' `setup = "alternative"` pieces.
fn is_alternative(name: &str, info: &PanelInfo) -> bool {
    info.build
        .and_then(|b| b.pieces.get(name))
        .is_some_and(|p| p.setup == "alternative")
}

/// What the stage's pieces allow a store item to replace; `None` without a build.
fn shop_plan(info: &PanelInfo) -> Option<crate::gear_score::Plan> {
    use crate::gear_score::piece_kind;
    let bis = info.bis?;
    let current = |slot: &str| -> Vec<&str> {
        bis.slots
            .get(slot)
            .into_iter()
            .flatten()
            .filter(|p| !outgrown(p, info))
            .map(|p| d2r_core::info::piece_name(p))
            .collect()
    };
    let entry = |name: &str| info.build.and_then(|b| b.pieces.get(name));
    // A piece without a `[pieces]` entry is generic unless it names a unique, set or runeword.
    let generic = |name: &str| {
        entry(name).map_or_else(
            || !is_fixed_name(name),
            |e| matches!(e.kind.as_str(), "magic" | "rare" | "crafted" | "base"),
        )
    };
    let weapon_kinds = current("weapon")
        .into_iter()
        .filter(|n| generic(n))
        .filter_map(|n| piece_kind(n).or_else(|| piece_kind(&entry(n)?.base)))
        .collect();
    let pieces: Vec<&str> = bis
        .slots
        .values()
        .flatten()
        .map(|p| d2r_core::info::piece_name(p))
        .filter(|n| !generic(n))
        .collect();
    let build_pieces = info
        .live
        .equipped_items
        .iter()
        .filter(|e| {
            e.name
                .as_deref()
                .is_some_and(|n| pieces.iter().any(|p| d2r_core::names::same_name(n, p)))
        })
        .map(|e| e.body)
        .collect();
    Some(crate::gear_score::Plan {
        weapon_kinds,
        offhand: !current("offhand").is_empty(),
        build_pieces,
    })
}

/// Whether `name` is a unique, set or runeword.
fn is_fixed_name(name: &str) -> bool {
    use d2r_core::item_names_data::{SET_NAMES, UNIQUE_NAMES};
    let same = |n: &str| d2r_core::names::same_name(n, name);
    d2r_core::runewords_data::lookup(name).is_some()
        || UNIQUE_NAMES.iter().any(|&(_, n)| same(n))
        || SET_NAMES.iter().any(|&(_, n)| same(n))
}

/// What the vendor panel's stat comparison values for this character.
pub fn score_profile(info: &PanelInfo) -> crate::gear_score::Profile {
    use crate::gear_score::{Profile, is_caster};
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
    let weapon = worn_weapon_kind(info).or_else(|| planned_weapon_kind(info));
    let attribute = |base: Option<u32>, id: u16| Some(i32::try_from(base?).ok()? + gear(id));
    Profile {
        class: info.live.class,
        level: info.live.char_level.map(u32::from),
        strength: attribute(info.live.strength, stat::STRENGTH),
        dexterity: attribute(info.live.dexterity, stat::DEXTERITY),
        skills: planned_skills(info),
        weapon,
        caster: is_caster(info.live.class, weapon),
        teleport: crate::gear_score::teleport_wanted(&info.live.holdings()),
    }
}

/// The allocated skills plus the stage's skill plan.
fn planned_skills(info: &PanelInfo) -> Vec<u16> {
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
    skills
}

fn worn_weapon_kind(info: &PanelInfo) -> Option<&'static str> {
    info.live
        .equipped_items
        .iter()
        .find(|e| e.body == 4)
        .and_then(|e| d2r_core::bases_data::base(e.code.as_deref()?))
        .map(|b| b.kind)
}

/// The weapon kind of the best current weapon piece wearable now, else of any current one.
fn planned_weapon_kind(info: &PanelInfo) -> Option<&'static str> {
    use crate::gear_score::piece_kind;
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
}

/// The column (`gloves`, `ring`, `merc helm`, …) and piece name of the first current-stage
/// piece the store `item` is: the character's slots first, then the mercenary's open ones;
/// outgrown pieces are skipped.
#[must_use]
pub fn shop_match(item: &d2r_core::shop::ShopItem, info: &PanelInfo) -> Option<(String, String)> {
    let bis = info.bis?;
    let piece_in = |pieces: &[String]| {
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
            Some((column_name(slot).to_owned(), name))
        })
        .or_else(|| {
            bis.merc_slots
                .iter()
                .find_map(|(slot, pieces)| Some((format!("merc {slot}"), piece_in(merc_open(pieces, info))?)))
        })
}

/// The upgrades the character can make right now; the page chimes when a new `key` appears.
/// The mercenary's slots only get `"shop"` entries: its items' required levels are unread.
#[must_use]
pub fn upgrades_json(info: &PanelInfo) -> Vec<serde_json::Value> {
    let (Some(bis), Some(lvl)) = (info.bis, info.live.char_level) else {
        return Vec::new();
    };
    let lvl = u32::from(lvl);
    let mut out = equip_upgrades(bis, lvl, info);
    craft_upgrades(bis, lvl, info, &mut out);
    shop_upgrades(info, lvl, &mut out);
    out
}

/// The `"equip"` upgrades: per character slot, the best owned, wearable, better piece not
/// equipped yet.
fn equip_upgrades(bis: &Bis, lvl: u32, info: &PanelInfo) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for slot in d2r_core::config::SLOT_NAMES {
        // The swap set is unread: its column mirrors the active weapon.
        if slot == "swap" {
            continue;
        }
        let Some(pieces) = bis.slots.get(slot) else {
            continue;
        };
        let column = column_name(slot);
        if let Some((name, why)) = owned_upgrade(pieces, slot, lvl, info) {
            let key = format!("equip:{column}:{name}");
            if !seen.contains(&key) {
                seen.push(key.clone());
                out.push(serde_json::json!({
                    "key": key, "kind": "equip", "slot": column, "name": name, "why": why,
                }));
            }
        }
    }
    out
}

/// The first of `pieces` that is owned, wearable at `lvl` and better than what is equipped in
/// `slot`, with where it is. A guide alternative is better by its owned copy's stats
/// ([`owned_versus_worn`]); any other piece by level, and only above the worn one (the list is
/// ranked; ring slots, with two rings worn, go by level only and have no alternatives judged).
fn owned_upgrade<'p>(
    pieces: &'p [String],
    slot: &str,
    lvl: u32,
    info: &PanelInfo,
) -> Option<(&'p str, String)> {
    let bodies = slot_bodies(slot);
    let eq_level = equipped_level(bodies, info);
    let one_body = bodies.len() == 1;
    let worn_at = pieces
        .iter()
        .position(|p| one_body && is_worn(d2r_core::info::piece_name(p), info));
    let profile = score_profile(info);
    let worn = worn_pieces(info);
    let versus = eq_level.map_or_else(String::new, |eq| format!(" (equipped: lvl {eq})"));
    pieces
        .iter()
        .enumerate()
        .filter(|(_, p)| !outgrown(p, info))
        .find_map(|(i, piece)| {
            let name = d2r_core::info::piece_name(piece);
            let req = d2r_core::info::piece_level(piece)?;
            let tick = d2r_core::info::piece_tick(piece, &[], info.live.owned, info.live.stash_owned)?;
            if req > lvl {
                return None;
            }
            let gain = if one_body && is_alternative(name, info) {
                if is_worn(name, info) {
                    return None;
                }
                match owned_versus_worn(name, Some(req), &worn, &profile, info)? {
                    Versus::Better(why) => format!(" · {why}"),
                    Versus::Worse | Versus::Even => return None,
                }
            } else if worn_at.is_none_or(|w| i < w) && piece_state(Some(req), eq_level, lvl) == "better" {
                versus.clone()
            } else {
                return None;
            };
            let place = match tick {
                d2r_core::info::Tick::Worn | d2r_core::info::Tick::Owned => "you have it",
                d2r_core::info::Tick::Stash => "in the shared stash",
            };
            Some((name, format!("{place} · lvl {req}{gain}")))
        })
}

/// The `"craft"` upgrades: runeword recipes with every rune owned whose piece is wearable and
/// better in its slot.
fn craft_upgrades(bis: &Bis, lvl: u32, info: &PanelInfo, out: &mut Vec<serde_json::Value>) {
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
        let has_runes = inputs.iter().any(|i| matches!(i, Input::Material { .. }));
        if has_runes
            && all_owned(&inputs, &materials)
            && !is_worn(&recipe.name, info)
            && req.is_some_and(|r| r <= lvl)
            && piece_state(req, equipped_level(slot_bodies(slot), info), lvl) == "better"
        {
            let column = column_name(slot);
            out.push(serde_json::json!({
                "key": format!("craft:{column}:{}", recipe.name),
                "kind": "craft",
                "slot": column,
                "name": recipe.name,
                "why": craft_why(recipe, &inputs),
            }));
        }
    }
}

/// Whether `materials` hold every material `inputs` need.
fn all_owned(inputs: &[Input], materials: &[(String, u32)]) -> bool {
    need_totals(inputs).iter().all(|(key, need)| {
        inputs
            .iter()
            .find(|i| i.need().is_some_and(|(k, _)| k == *key))
            .and_then(|i| i.have(materials))
            .is_some_and(|have| have >= *need)
    })
}

fn craft_why(recipe: &Recipe, inputs: &[Input]) -> String {
    let base = recipe
        .inputs
        .iter()
        .zip(inputs)
        .filter(|(_, i)| **i == Input::Base)
        .map(|(t, _)| t.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    if base.is_empty() {
        "all runes owned".to_owned()
    } else {
        format!("all runes owned · base: {base}")
    }
}

/// Appends the `"shop"` upgrades to `out`: one per current-stage piece an open-window item is,
/// keyed per build piece so differently-affixed copies chime once.
pub fn shop_upgrades(info: &PanelInfo, lvl: u32, out: &mut Vec<serde_json::Value>) {
    for item in info.live.shop.map_or(&[][..], |shop| shop.items.as_slice()) {
        let Some((slot, piece)) = shop_match(item, info) else {
            continue;
        };
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

/// Why the Gear panel should open on its own, `None` to keep it collapsed: an owned piece to
/// wear now, else findable upgrades. Only the character's own slots count, never the swap.
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

/// Whether `piece` is an unowned upgrade wearable at `lvl` that drops up to monster level
/// `max_mlvl`: a unique or set item by its quality level, a runeword by its unowned runes' (the
/// base counts as findable). Any other piece has no item level to check and never is, nor is a
/// guide alternative: unowned, it is neither better nor worse than what is worn.
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
    if !unowned
        || req > lvl
        || is_alternative(name, info)
        || piece_state(Some(req), eq_level, lvl) != "better"
    {
        return false;
    }
    if let Some(word) = d2r_core::runewords_data::lookup(name) {
        return runes_findable(word, max_mlvl, materials);
    }
    unique_set_qlvl(name).is_some_and(|q| q <= max_mlvl)
}

/// Whether every rune of `word` is owned in the count it needs or drops up to `max_mlvl`.
fn runes_findable(word: &Runeword, max_mlvl: u32, materials: &[(String, u32)]) -> bool {
    word.runes.iter().all(|rune| {
        let need = word.runes.iter().filter(|r| *r == rune).count();
        let have = materials
            .iter()
            .filter(|(n, _)| same_rune(n, rune))
            .map(|(_, q)| *q)
            .sum::<u32>();
        usize::try_from(have).is_ok_and(|have| have >= need)
            || d2r_core::item_levels_data::rune_qlvl(rune).is_some_and(|q| q <= max_mlvl)
    })
}

/// Whether material name `material` (`"Tal Rune"`) is rune `rune` (`"Tal"`).
#[must_use]
pub fn same_rune(material: &str, rune: &str) -> bool {
    material
        .trim()
        .strip_suffix(" Rune")
        .is_some_and(|bare| bare.eq_ignore_ascii_case(rune))
}

/// The lowest quality level of the unique or set item shown in game as `name`. The tables
/// name some items differently (Biggin's Bonnet is `War Bonnet`), mapped by `DISPLAY_NAMES`.
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

/// The gear grid section: the character's columns, the mercenary's and the build's extras, and
/// the build's pieces marked not needed (`not_needed`, for the undo).
#[must_use]
pub fn gear_json(info: &PanelInfo) -> serde_json::Value {
    let mut columns: Vec<_> = GRID_COLUMNS
        .iter()
        .map(|&(title, slot, bodies)| {
            grid_column(title, info.bis.and_then(|b| b.slots.get(slot)), bodies, info)
        })
        .collect();
    let ring_pieces = ring_pieces(info);
    columns.insert(
        RING_COLUMN_AT,
        grid_column("Ring", Some(&ring_pieces), &[6, 7], info),
    );
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
        "merc": merc_columns(info),
        "extras": extras,
        "charms": charms_json(info),
        "not_needed": info.bis.map_or(&[][..], |b| info.marks_of(&b.build)),
    })
}

/// The charm wishlist: the current stage's `[[charms]]`, then the next stage's whose name it
/// doesn't list (a same-named wish of the next stage is left out, whatever its count; `stage` =
/// that build's name: the summoner sees the Fury's), each with `have` = the owned
/// charms that fill it ([`crate::charms::found`]: carried, cube, stash, shared stash pages),
/// `need` = its stat conditions, and a unique's build-file details.
fn charms_json(info: &PanelInfo) -> Vec<serde_json::Value> {
    use crate::charms::CharmWish;
    let mut wishes: Vec<(&CharmWish, Option<&crate::config::Build>)> = info
        .build
        .map_or(&[][..], |b| b.charms.as_slice())
        .iter()
        .map(|w| (w, None))
        .collect();
    if let Some((_, next)) = info.stage.and_then(|s| s.next) {
        for wish in &next.charms {
            if !wishes
                .iter()
                .any(|(w, _)| d2r_core::names::same_name(&w.name, &wish.name))
            {
                wishes.push((wish, Some(next)));
            }
        }
    }
    let held = crate::stash_plan::held_from(info.live.owned_items, info.live.stash);
    let refs: Vec<&CharmWish> = wishes.iter().map(|&(w, _)| w).collect();
    let have = crate::charms::found(&refs, held.iter().map(|h| &h.item), Some(info.live.class));
    wishes
        .iter()
        .zip(have)
        .map(|(&(wish, later), have)| {
            let mut json = serde_json::json!({
                "name": wish.name,
                "kind": if wish.unique { "unique" } else { "magic" },
                "size": wish.size.map(|s| match s {
                    d2r_core::items::CharmSize::Small => "Small Charm",
                    d2r_core::items::CharmSize::Large => "Large Charm",
                    d2r_core::items::CharmSize::Grand => "Grand Charm",
                }),
                "level": wish.level,
                "count": wish.count,
                "have": have,
                "need": wish.describe(),
                "stage": later.map(|b| b.name.as_str()),
            });
            if wish.unique {
                add_piece_details(&mut json, &wish.name, later.or(info.build));
            }
            if let Some(obj) = json.as_object_mut() {
                // A unique charm's piece is a guide alternative: here it is simply wanted.
                obj.remove("setup");
                if !wish.why.is_empty() {
                    obj.insert("why".to_owned(), serde_json::json!(wish.why));
                }
            }
            json
        })
        .collect()
}

/// The pieces of both ring slots, deduplicated, best first.
fn ring_pieces(info: &PanelInfo) -> Vec<String> {
    let mut pieces: Vec<String> = Vec::new();
    for slot in ["ring1", "ring2"] {
        for piece in info
            .bis
            .and_then(|b| b.slots.get(slot))
            .map_or(&[][..], Vec::as_slice)
        {
            if !pieces.contains(piece) {
                pieces.push(piece.clone());
            }
        }
    }
    pieces
}

/// The mercenary's columns, without a header: its items' levels are unread. The worn piece's
/// build level stands in for the equipped requirement, and the pieces below it are beaten.
fn merc_columns(info: &PanelInfo) -> Vec<serde_json::Value> {
    let act = info
        .build
        .map_or_else(MercAct::default, crate::config::Build::merc_act);
    MERC_COLUMNS
        .iter()
        .filter_map(|&(title, slot, body)| {
            let pieces = info.bis.and_then(|b| b.merc_slots.get(slot))?;
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
            // The mercenary's items have no stats in the snapshot.
            judge_alternatives(&mut json, |_, _| None);
            Some(serde_json::json!({ "title": title, "pieces": json }))
        })
        .collect()
}

/// Marks the grid pieces ranked below the first worn one as `"worse"`, without a `now` badge;
/// the guides' alternatives follow the stage's own pieces, so their place says nothing.
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
        .filter(|obj| obj.get("setup").and_then(serde_json::Value::as_str) != Some("alternative"))
    {
        obj.insert("state".to_owned(), serde_json::json!("worse"));
        obj.remove("now");
    }
}

/// Sets the state of each guide alternative not worn from `versus` (name, required level):
/// `"better"` (an upgrade), `"worse"` (struck, no `now` badge), else `"too_high"` when its
/// level says so or `"neutral"`; its level alone never strikes it.
fn judge_alternatives(
    pieces: &mut [serde_json::Value],
    versus: impl Fn(&str, Option<u32>) -> Option<Versus>,
) {
    for obj in pieces.iter_mut().filter_map(serde_json::Value::as_object_mut) {
        let text = |key: &str| obj.get(key).and_then(serde_json::Value::as_str);
        if text("setup") != Some("alternative") || text("tick") == Some("worn") {
            continue;
        }
        let req = obj
            .get("level")
            .and_then(serde_json::Value::as_u64)
            .and_then(|l| u32::try_from(l).ok());
        let state = match versus(text("name").unwrap_or_default(), req) {
            Some(Versus::Better(_)) => "better",
            Some(Versus::Worse) => "worse",
            _ if text("state") == Some("too_high") => "too_high",
            _ => "neutral",
        };
        if state == "worse" {
            obj.remove("now");
        }
        obj.insert("state".to_owned(), serde_json::json!(state));
    }
}

/// The current stage's tips for the character level (all of them while it is unknown).
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

/// One gear-grid column: the pieces equipped in `bodies`, the build's `pieces` with their
/// state, and the off-hand's two-handed note.
pub fn grid_column(
    title: &str,
    pieces: Option<&Vec<String>>,
    bodies: &[u8],
    info: &PanelInfo,
) -> serde_json::Value {
    let header = equipped_in(bodies, info);
    let class = info.live.class;
    let offhand = info.bis.is_some_and(|b| b.slots.contains_key("offhand"));
    let wearer = if title == "Weapons" && offhand {
        Wearer::PlayerWithOffhand(class)
    } else {
        Wearer::Player(class)
    };
    let note = (title == "Off-Hand" && two_handed_equipped(info)).then_some("(two-handed weapon equipped)");
    let mut pieces = grid_pieces(
        pieces.map_or(&[][..], Vec::as_slice),
        equipped_level(bodies, info),
        &d2r_core::game::worn_names(info.live.equipped_items),
        (bodies.first().copied(), wearer),
        info,
    );
    // The Ring column holds two worn rings: a piece between them can still beat the second.
    if bodies.len() == 1 {
        beat_below_worn(&mut pieces);
        let profile = score_profile(info);
        let worn = worn_pieces(info);
        judge_alternatives(&mut pieces, |name, req| {
            owned_versus_worn(name, req, &worn, &profile, info)
        });
    }
    serde_json::json!({
        "title": title,
        "equipped": header
            .iter()
            .map(|e| serde_json::json!({ "name": equipped_name(e), "level": e.required_level() }))
            .collect::<Vec<_>>(),
        "note": note,
        "pieces": pieces,
    })
}

fn equipped_in<'i>(bodies: &[u8], info: &PanelInfo<'i>) -> Vec<&'i d2r_core::game::Equipped> {
    bodies
        .iter()
        .filter_map(|&body| info.live.equipped_items.iter().find(|e| e.body == body))
        .collect()
}

fn two_handed_equipped(info: &PanelInfo) -> bool {
    equipped_in(&[4], info)
        .into_iter()
        .next()
        .and_then(|w| w.code.as_deref())
        .and_then(d2r_core::weapons_data::two_handed)
        .is_some_and(|two| two)
}

/// The build's pieces of one grid column with their [`piece_state`] (a worn one `"neutral"`
/// instead of `"worse"`), best first. Outgrown pieces leave unless worn; a worn or `"worse"`
/// piece never gets the `now` ("wear now") badge.
#[must_use]
pub fn grid_pieces(
    pieces: &[String],
    equipped_level: Option<u32>,
    worn: &[String],
    (slot, wearer): (Option<u8>, Wearer),
    info: &PanelInfo,
) -> Vec<serde_json::Value> {
    pieces
        .iter()
        .filter_map(|piece| grid_piece(piece, equipped_level, worn, (slot, wearer), info))
        .collect()
}

fn grid_piece(
    piece: &str,
    equipped_level: Option<u32>,
    worn: &[String],
    (slot, wearer): (Option<u8>, Wearer),
    info: &PanelInfo,
) -> Option<serde_json::Value> {
    let lvl = u32::from(info.live.char_level.unwrap_or(0));
    let tick = d2r_core::info::piece_tick(piece, worn, info.live.owned, info.live.stash_owned);
    let is_worn = tick == Some(d2r_core::info::Tick::Worn);
    if outgrown(piece, info) && !is_worn {
        return None;
    }
    let name = d2r_core::info::piece_name(piece);
    let req = d2r_core::info::piece_level(piece);
    // The worn piece is never struck in its own column.
    let state = match piece_state(req, equipped_level, lvl) {
        "worse" if is_worn => "neutral",
        state => state,
    };
    let mut json = serde_json::json!({
        "piece": piece,
        "name": name,
        "level": req,
        "state": state,
        "tick": tick.map(d2r_core::info::Tick::as_str),
    });
    let runeword = d2r_core::runewords_data::lookup(name);
    if let Some(rw) = runeword {
        add_runeword_hint(&mut json, rw, (slot, wearer), info);
    }
    add_piece_details(&mut json, name, info.build);
    if is_worn
        && runeword.is_some()
        && let Some(obj) = json.as_object_mut()
    {
        // The missing-rune remark is moot once the runeword is made.
        obj.remove("notes");
    }
    add_level_window(&mut json, piece, req, state != "worse" && !is_worn, info);
    Some(json)
}

fn add_runeword_hint(
    json: &mut serde_json::Value,
    rw: &Runeword,
    (slot, wearer): (Option<u8>, Wearer),
    info: &PanelInfo,
) {
    let Some(obj) = json.as_object_mut() else { return };
    obj.insert("runes".to_owned(), serde_json::json!(rw.runes));
    obj.insert("bases".to_owned(), serde_json::json!(rw.bases));
    let hint = match wearer {
        Wearer::Merc(_) => merc_base_hint(rw, info, slot, info.build),
        _ => base_hint(rw, info, slot, wearer),
    };
    obj.insert("base_hint".to_owned(), serde_json::json!(hint));
}

/// A rune's `setup` becomes its minimum level; another piece inside its leveling window and
/// wearable now (`can_be_better`) gets the `now` badge.
fn add_level_window(
    json: &mut serde_json::Value,
    piece: &str,
    req: Option<u32>,
    can_be_better: bool,
    info: &PanelInfo,
) {
    let (range, rune) = piece_window(piece, info);
    let (Some((from, to)), Some(obj)) = (range, json.as_object_mut()) else {
        return;
    };
    let lvl = u32::from(info.live.char_level.unwrap_or(0));
    if rune {
        let min = req.unwrap_or(from);
        obj.insert("setup".to_owned(), serde_json::json!(format!("lvl {min}+")));
    } else if info.live.char_level.is_some()
        && can_be_better
        && (from..=to).contains(&lvl)
        && req.is_none_or(|r| r <= lvl)
    {
        obj.insert("now".to_owned(), serde_json::json!(true));
    }
}

/// Adds the build file's `[pieces."<name>"]` details that are set to a piece's JSON.
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

/// The name of an equipped piece in a column header: its unique, set or runeword name, else a
/// white base's name, else a placeholder (magic and rare names come from affixes).
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
