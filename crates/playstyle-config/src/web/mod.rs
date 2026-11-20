//! The playstyle's dashboard panels, as JSON for the core page ([`d2r_core::web`]): the item
//! plan (sell / move / keep / give-away), the `BiS` lines, the gear grid with its upgrades and
//! attention flag ([`gear`]), the guide stage, the recipes ([`recipes`]) and the farm list
//! ([`farm`]); the vendor panel's slot / better notes and the skill plan for the core panels;
//! and the `/config` page's JSON and `config.toml` edits ([`config_page`]).
//!
//! Its sections of the live JSON ([`panels`]): `sell`, `move`, `keep_own`, `keep_trade`,
//! `give` (the item plan's groups, [`crate::stash_plan`], one structured entry each,
//! [`advice_lines`]; the page joins each group into one wrapping run in the Sell / Keep panel),
//! `bis` (the chase list, each line with a structured `"tick": "worn" | "owned" | "stash" |
//! null` and, for a runeword piece, `"runes"` and `"bases"`, [`bis_line`]), `gear` (the 2×5
//! `BiS` grid, [`gear::gear_json`]), `gear_attention` (bool) and `gear_attention_why` (text or
//! `null`, [`gear::gear_attention`]), `upgrades` ([`gear::upgrades_json`]), `stage`
//! ([`stage_json`]), `recipes` ([`recipes::recipes_json`]) and `farm` (the cross-character farm
//! list, [`farm::farm_json`], `null` without one) and `next` (the route's current step,
//! [`crate::route::Advice::json`], `null` without one).

pub mod config_page;
pub mod farm;
pub mod gear;
pub mod recipes;
#[cfg(test)]
mod tests;

use crate::config::{Bis, Build, FarmGroup, Stage};
use d2r_core::socket_bases::Wearer;
use d2r_core::web::{LiveInfo, Panels, hex};

/// What the playstyle's panels read for one tick: the core's dashboard data plus the
/// evaluation's build state.
#[derive(Debug, Clone, Copy)]
pub struct PanelInfo<'a> {
    pub live: LiveInfo<'a>,
    /// The character's resolved `BiS` slots (`None` without a build): the gear grid lists
    /// each slot's pieces.
    pub bis: Option<&'a Bis>,
    /// The current stage's build definition (`None` without one): the per-piece details, the
    /// targets and the extras (charms, jewels, runes) the gear panel shows.
    pub build: Option<&'a Build>,
    /// The character's guide stage ([`crate::config::Settings::stage`]; `None` without a
    /// build): the `stage` section (not rendered), and `build` is its build.
    pub stage: Option<Stage<'a>>,
    /// The cross-character farm list (`farm.toml`, [`d2r_core::config::load_farm`]): the Farm
    /// panel ([`farm::farm_json`]).
    pub farm: &'a [FarmGroup],
    /// The build names of the character's current stage and every later one
    /// ([`crate::config::Settings::stage_chain`]): a farm entry for one of them that the
    /// character wears enough copies of is hidden ([`farm::farm_worn`]).
    pub stage_chain: &'a [String],
    /// The route's current step ([`crate::route::current`]): the `next` section.
    pub next: Option<&'a crate::route::Advice>,
}

/// The playstyle's part of one tick's dashboard (see the module docs for its sections).
#[must_use]
pub fn panels<'a>(info: &PanelInfo<'a>) -> Panels<'a> {
    let blocks = info.live.blocks;
    let upgrades = gear::upgrades_json(info);
    let attention = gear::gear_attention(info, &upgrades);
    let sections = serde_json::json!({
        "sell": advice_lines(&blocks.sell, d2r_core::info::SELL_TEXT),
        "move": advice_lines(&blocks.moves, d2r_core::info::MOVE_TEXT),
        "keep_own": advice_lines(&blocks.keep_own, d2r_core::info::KEEP_TEXT),
        "keep_trade": advice_lines(&blocks.keep_trade, d2r_core::info::TRADE_TEXT),
        "give": advice_lines(&blocks.give, d2r_core::info::GIVE_TEXT),
        "bis": blocks.bis.iter().map(bis_line).collect::<Vec<_>>(),
        "gear": gear::gear_json(info),
        "stage": stage_json(info.stage.as_ref(), &|item| info.live.holdings().count(item)),
        "recipes": recipes::recipes_json(info),
        "gear_attention": attention.is_some(),
        "gear_attention_why": attention,
        "upgrades": upgrades,
        "farm": farm::farm_json(info),
        "next": info.next.map(crate::route::Advice::json),
    });
    let serde_json::Value::Object(sections) = sections else {
        // `json!` with braces always builds an object.
        return Panels::default();
    };
    Panels {
        sections,
        skill_plan: info.build.map_or(&[][..], |b| b.skill_plan.as_slice()),
        shop: gear::shop_notes(info),
    }
}

/// The guide stage (`live_json`'s `stage` section): `{ "slug", "name", "class_default",
/// "next": { "slug", "name", "level", "auto", "note", "ready", "requires": [{ "label", "count",
/// "have", "met", "any": [{ "name", "owned" }] }] } | null }`, `null` without a build.
/// `ready`: every `requires` group is met (the level is not part of it; with `auto` the stage
/// switches by itself once `level` is reached too). The page sounds when it turns true (user
/// request, 2026-11-20: "ready to switch to Fury").
/// `requires` lists the current stage's `switch_requires` groups ([`crate::config::SwitchGroup`]):
/// `have` copies of the group's items (`copies` per item, [`d2r_core::stash::Holdings::count`]),
/// met at `count`; `owned` = at least one copy of that item.
pub fn stage_json(stage: Option<&Stage>, copies: &dyn Fn(&str) -> u32) -> serde_json::Value {
    let Some(stage) = stage else {
        return serde_json::Value::Null;
    };
    let next = stage.next.map(|(slug, next)| {
        serde_json::json!({
            "slug": slug,
            "name": next.name,
            "level": stage.build.switch_level,
            "auto": stage.build.auto_switch,
            "note": stage.build.switch_note,
            "ready": stage.build.switch_requires.iter().all(|group| group.met(copies)),
            "requires": stage
                .build
                .switch_requires
                .iter()
                .map(|group| {
                    let any = group
                        .any
                        .iter()
                        .map(|item| serde_json::json!({ "name": item, "owned": copies(item) > 0 }))
                        .collect::<Vec<_>>();
                    serde_json::json!({
                        "label": group.label,
                        "count": group.count,
                        "have": group.have(copies),
                        "met": group.met(copies),
                        "any": any,
                    })
                })
                .collect::<Vec<_>>(),
        })
    });
    serde_json::json!({
        "slug": stage.slug,
        "name": stage.build.name,
        "class_default": stage.class_default,
        "next": next,
    })
}

/// The natural-socket base hint for runeword `word` ([`d2r_core::socket_bases::hint`]): bases
/// that can drop with its socket count up to the highest monster level the character farms
/// ([`d2r_core::info::reachable_level`], the cap [`gear_attention`] uses), for `wearer` (the
/// character's class, or the build's mercenary: [`Wearer::Merc`]) and, with `slot`
/// set, worn in that `ItemData::BODY` slot. `None` for an unknown character level or difficulty
/// or when nothing qualifies: the dashboard then shows the runeword's `bases` categories.
#[must_use]
pub fn base_hint(
    word: &d2r_core::runewords_data::Runeword,
    info: &PanelInfo,
    slot: Option<u8>,
    wearer: Wearer,
) -> Option<String> {
    let max_mlvl = u32::from(d2r_core::info::reachable_level(
        info.live.char_level,
        info.live.difficulty,
    )?);
    d2r_core::socket_bases::hint(word, max_mlvl, wearer, slot)
}

/// The `ItemData::BODY` slots a build slot's equipped piece sits in (the merged ring column
/// reads both rings).
#[must_use]
pub fn slot_bodies(slot: &str) -> &'static [u8] {
    match slot {
        "helm" => &[1],
        "amulet" => &[2],
        "body" => &[3],
        "weapon" | "swap" => &[4],
        "offhand" => &[5],
        "ring1" | "ring2" => &[6, 7],
        "belt" => &[8],
        "boots" => &[9],
        "gloves" => &[10],
        _ => &[],
    }
}

/// The comparison base of the equipped piece(s) in `bodies`: the strongest one's required
/// level (two rings compare against the higher one), `None` when nothing comparable is
/// equipped.
pub fn equipped_level(bodies: &[u8], info: &PanelInfo) -> Option<u32> {
    bodies
        .iter()
        .filter_map(|&body| info.live.equipped_items.iter().find(|e| e.body == body))
        .filter_map(d2r_core::game::Equipped::required_level)
        .max()
}

/// A build piece's grid state against the equipped comparison level and the character
/// level `lvl` (see [`grid_pieces`] for the states).
#[must_use]
pub fn piece_state(req: Option<u32>, equipped_level: Option<u32>, lvl: u32) -> &'static str {
    match req {
        Some(req) if equipped_level.is_some_and(|eq| req <= eq) => "worse",
        Some(req) if req > lvl + 5 => "too_high",
        _ => "better",
    }
}

/// Whether a piece named `name` is equipped already (any slot), by the equipped items'
/// unique/set/runeword names ([`d2r_core::names::same_name`]).
#[must_use]
pub fn is_worn(name: &str, info: &PanelInfo) -> bool {
    info.live.equipped_items.iter().any(|e| {
        e.name
            .as_deref()
            .is_some_and(|n| d2r_core::names::same_name(n, name))
    })
}

/// One `BiS` line as `{ "text", "colour", "tick" }`: the tick is the structural ✓ source
/// ([`info::Tick`]) — `null` when the piece is owned by neither the character nor the stash.
/// A piece that is a known runeword also carries `"runes"` and `"bases"` (the dashboard renders
/// them as a dim hint line under the piece).
pub fn bis_line(line: &d2r_core::info::BisLine) -> serde_json::Value {
    let mut json = serde_json::json!({
        "text": line.text,
        "colour": hex(d2r_core::info::BIS_TEXT),
        "tick": line.tick.map(d2r_core::info::Tick::as_str),
    });
    if let Some(rw) = &line.runeword
        && let Some(obj) = json.as_object_mut()
    {
        obj.insert("runes".to_owned(), serde_json::json!(rw.runes));
        obj.insert("bases".to_owned(), serde_json::json!(rw.bases));
    }
    json
}

/// A build piece's level window from its `setup` ("lvl 13-26"), and whether it is a rune.
#[must_use]
pub fn piece_window(piece: &str, info: &PanelInfo) -> (Option<(u32, u32)>, bool) {
    let details = info
        .build
        .and_then(|b| b.pieces.get(d2r_core::info::piece_name(piece)));
    (
        details.and_then(|p| level_range(&p.setup)),
        details.is_some_and(|p| p.kind == "rune"),
    )
}

/// True when the character outgrew the piece's leveling window: it leaves the dashboard
/// (user request, 2026-11-18). A rune never runs out, it only has a minimum level.
#[must_use]
pub fn outgrown(piece: &str, info: &PanelInfo) -> bool {
    let (range, rune) = piece_window(piece, info);
    match (range, info.live.char_level) {
        (Some((_, to)), Some(lvl)) => !rune && u32::from(lvl) > to,
        _ => false,
    }
}

/// The character-level range of a build piece's `setup` ("lvl 13-26" → (13, 26)); `None` for
/// the other setups ("starter", "standard", "alternative") and anything unparsable.
#[must_use]
pub fn level_range(setup: &str) -> Option<(u32, u32)> {
    let (from, to) = setup.strip_prefix("lvl ")?.split_once('-')?;
    Some((from.trim().parse().ok()?, to.trim().parse().ok()?))
}

/// The item plan's entries as `{ "text", "colour", "name", "count", "place", "target",
/// "wrong_place", "reason" }` objects in the block's colour: `text` is the whole entry
/// ([`d2r_core::info::advice_entry`]); `place` the spot (`inv`, `page 4`); `target` the pages a
/// move goes to (`1–2`, else `null`); `wrong_place` whether the item sits where it doesn't
/// belong (every move: the page shows the place in red, user request 2026-11-20); `reason` why
/// a scored item is sold (else `null`).
#[must_use]
pub fn advice_lines(list: &[crate::stash_plan::Advice], colour: [u8; 4]) -> Vec<serde_json::Value> {
    use crate::stash_plan::Action;
    let hex = hex(colour);
    list.iter()
        .map(|a| {
            let target = match a.action {
                Action::Move(side) => Some(side.pages()),
                Action::Sell | Action::Keep(_) | Action::GiveAway => None,
            };
            serde_json::json!({
                "text": d2r_core::info::advice_entry(a),
                "colour": hex,
                "name": a.name,
                "count": a.count,
                "place": a.spot.label(),
                "target": target,
                "wrong_place": target.is_some(),
                "reason": a.reason,
            })
        })
        .collect()
}
