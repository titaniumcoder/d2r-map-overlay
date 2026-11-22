//! The playstyle's dashboard panels as JSON sections for the core page ([`d2r_core::web`]),
//! and the `/config` page ([`config_page`]).

pub mod config_page;
pub mod farm;
pub mod gear;
pub mod next_gear;
pub mod recipes;
#[cfg(test)]
mod tests;

use crate::config::{Bis, Build, FarmGroup, Stage, SwitchGroup};
use d2r_core::socket_bases::Wearer;
use d2r_core::web::{LiveInfo, Panels, hex};

/// What the playstyle's panels read for one tick: the core's dashboard data plus the
/// evaluation's build state.
#[derive(Debug, Clone, Copy)]
pub struct PanelInfo<'a> {
    pub live: LiveInfo<'a>,
    pub bis: Option<&'a Bis>,
    /// The current stage's build definition.
    pub build: Option<&'a Build>,
    pub stage: Option<Stage<'a>>,
    pub farm: &'a [FarmGroup],
    /// The route, whose bosses say where the farm list's entries drop.
    pub route: &'a crate::route::Route,
    /// The build names of the current stage and every later one.
    pub stage_chain: &'a [String],
    /// The marks in force per build ([`crate::not_needed::Marks`]), for the undo lists.
    pub not_needed: &'a crate::not_needed::Marks,
}

impl PanelInfo<'_> {
    /// The marks in force for build (or `[bis]` key) `build`.
    #[must_use]
    pub fn marks_of(&self, build: &str) -> &[crate::not_needed::Marked] {
        self.not_needed
            .get(&build.trim().to_lowercase())
            .map_or(&[], Vec::as_slice)
    }
}

/// The playstyle's part of one tick's dashboard.
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
        "next_gear": next_gear::next_gear_json(info),
        "stage": stage_json(info.stage.as_ref(), &|item| info.live.holdings().count(item)),
        "recipes": recipes::recipes_json(info),
        "gear_attention": attention.is_some(),
        "gear_attention_why": attention,
        "upgrades": upgrades,
        "farm": farm::farm_json(info),
    });
    let serde_json::Value::Object(sections) = sections else {
        return Panels::default();
    };
    Panels {
        sections,
        skill_plan: info.build.map_or(&[][..], |b| b.skill_plan.as_slice()),
        shop: gear::shop_notes(info),
    }
}

/// The guide stage section, `null` without a build; `copies` counts the held copies of an item.
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
            "requires": stage
                .build
                .switch_requires
                .iter()
                .map(|group| switch_group_json(group, copies))
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

fn switch_group_json(group: &SwitchGroup, copies: &dyn Fn(&str) -> u32) -> serde_json::Value {
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
}

/// The natural-socket bases for runeword `word` that can drop where the character farms now,
/// for `wearer`, worn in body `slot` when set.
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

/// [`base_hint`] for the mercenary act of `build` that carries `word`, prefixed `"Act N merc: "`
/// when that isn't the build's first act.
#[must_use]
pub fn merc_base_hint(
    word: &d2r_core::runewords_data::Runeword,
    info: &PanelInfo,
    slot: Option<u8>,
    build: Option<&crate::config::Build>,
) -> Option<String> {
    use d2r_core::socket_bases::MercAct;
    let act = build.map_or_else(MercAct::default, |b| b.merc_wearer(word.name, slot));
    let hint = base_hint(word, info, slot, Wearer::Merc(act))?;
    Some(if build.is_some_and(|b| b.merc_act() != act) {
        format!("Act {} merc: {hint}", act.number())
    } else {
        hint
    })
}

/// The `ItemData::BODY` slots of a build slot.
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

/// The highest required level of the comparable pieces equipped in `bodies`.
pub fn equipped_level(bodies: &[u8], info: &PanelInfo) -> Option<u32> {
    bodies
        .iter()
        .filter_map(|&body| info.live.equipped_items.iter().find(|e| e.body == body))
        .filter_map(d2r_core::game::Equipped::required_level)
        .max()
}

/// A build piece's grid state: `"worse"`, `"too_high"` (above clvl + 5) or `"better"`.
#[must_use]
pub fn piece_state(req: Option<u32>, equipped_level: Option<u32>, lvl: u32) -> &'static str {
    match req {
        Some(req) if equipped_level.is_some_and(|eq| req <= eq) => "worse",
        Some(req) if req > lvl + 5 => "too_high",
        _ => "better",
    }
}

/// Whether a piece named `name` is equipped in any slot.
#[must_use]
pub fn is_worn(name: &str, info: &PanelInfo) -> bool {
    info.live.equipped_items.iter().any(|e| {
        e.name
            .as_deref()
            .is_some_and(|n| d2r_core::names::same_name(n, name))
    })
}

/// One `BiS` line; a runeword piece also carries its `runes` and `bases`.
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

/// Whether the character is past the piece's leveling window (never for a rune).
#[must_use]
pub fn outgrown(piece: &str, info: &PanelInfo) -> bool {
    let (range, rune) = piece_window(piece, info);
    match (range, info.live.char_level) {
        (Some((_, to)), Some(lvl)) => !rune && u32::from(lvl) > to,
        _ => false,
    }
}

/// The character-level range of a piece's `setup` ("lvl 13-26" → (13, 26)).
#[must_use]
pub fn level_range(setup: &str) -> Option<(u32, u32)> {
    let (from, to) = setup.strip_prefix("lvl ")?.split_once('-')?;
    Some((from.trim().parse().ok()?, to.trim().parse().ok()?))
}

/// The item plan's entries in the block's `colour`; an item with a move target is in the
/// wrong place.
#[must_use]
pub fn advice_lines(list: &[crate::stash_plan::Advice], colour: [u8; 4]) -> Vec<serde_json::Value> {
    use crate::stash_plan::Action;
    let hex = hex(colour);
    list.iter()
        .map(|a| {
            let target = match a.action {
                Action::Move(side) => Some(side.pages()),
                Action::Stow(tab) => Some(tab.label()),
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
