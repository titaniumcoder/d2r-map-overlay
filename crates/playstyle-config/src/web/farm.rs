//! The farm list panel: the cross-character farm list (`farm.toml`) with the copies held and
//! the runes still needed.

use crate::worth::Why;
use d2r_core::socket_bases::Wearer;
use d2r_core::web::LiveInfo;

use super::recipes::{Input, need_totals, owned_materials, rune_count};
use super::{PanelInfo, base_hint};

/// Where a farm-list entry stands, across characters: `Done` once the shared stash holds
/// `count` copies, `Found` while the played character carries a copy outside it, else
/// `Missing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FarmStatus {
    Done,
    Found,
    Missing,
}

impl FarmStatus {
    const fn name(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Found => "found",
            Self::Missing => "missing",
        }
    }
}

/// The copies of farm entry `item`: `(in the shared stash, carried by the played character
/// outside it or worn by its mercenary)` ([`d2r_core::stash::Holdings::copies`]).
#[must_use]
pub fn farm_copies(item: &crate::config::FarmItem, info: &PanelInfo) -> (u32, u32) {
    info.live.holdings().copies(&item.name, Some(item.kind))
}

/// The status of a farm entry wanting `count` copies with `stashed` in the shared stash and
/// `carried` on the played character.
#[must_use]
pub const fn farm_status(count: u32, stashed: u32, carried: u32) -> FarmStatus {
    if stashed >= count {
        FarmStatus::Done
    } else if carried > 0 {
        FarmStatus::Found
    } else {
        FarmStatus::Missing
    }
}

/// The farm list (`live_json`'s `farm` section), `null` without one; entries the character
/// wears for its own build are left out, and a group left empty with them ([`farm_worn`]):
/// `{ "groups": [{ "for", "items": [{ "name", "kind", "count", "stashed", "carried",
/// "status", "runes", "bases" }] }], "runes": [{ "rune", "code", "need", "have", "stash",
/// "for" }], "worth": … }` (`worth`: [`worth_json`]).
/// `status` is `"done"` / `"found"` / `"missing"` ([`farm_status`]). A runeword not done
/// with copies still to build (`count` minus stashed and carried) lists its `runes` as
/// `{ "rune", "need", "have", "status" }` (need = per copy × copies to build, have from
/// [`owned_materials`], status `"have"`/`"missing"`) and its allowed `bases`; otherwise both
/// are empty. `base_hint` names concrete natural-socket bases for it ([`base_hint`]; `null`
/// when none qualify or the runeword is done). The top-level `runes` sums those needs per rune over the whole list, in rune
/// order, with the runewords needing it (`for`), the owned count (`have`) and the shared
/// materials tab's own count (`stash`): the stash panel marks its rune stacks from it.
#[must_use]
pub fn farm_json(info: &PanelInfo) -> serde_json::Value {
    if info.farm.is_empty() {
        return serde_json::Value::Null;
    }
    let materials = owned_materials(info);
    let stash_only = owned_materials(&PanelInfo {
        live: LiveInfo {
            held_materials: &[],
            ..info.live
        },
        ..*info
    });
    let mut wanted = Vec::new();
    let groups: Vec<serde_json::Value> = info
        .farm
        .iter()
        .filter_map(|group| {
            let items: Vec<serde_json::Value> = group
                .items
                .iter()
                .filter(|item| !farm_worn(&group.target, item, info))
                .map(|item| farm_item_json(item, info, &materials, &mut wanted))
                .collect();
            // A group whose entries are all worn leaves the list.
            (!items.is_empty()).then(|| serde_json::json!({ "for": group.target, "items": items }))
        })
        .collect();
    wanted.sort_by_key(|(rune, _, _)| d2r_core::stash::RUNE_NAMES.iter().position(|r| r == rune));
    let runes: Vec<serde_json::Value> = wanted
        .into_iter()
        .map(|(rune, need, words)| {
            let count = |m: &[(String, u32)]| rune_count(rune, m);
            let code = d2r_core::stash::RUNE_NAMES
                .iter()
                .position(|r| *r == rune)
                .map(|i| format!("r{:02}", i.saturating_add(1)));
            serde_json::json!({
                "rune": rune,
                "code": code,
                "need": need,
                "have": count(&materials),
                "stash": count(&stash_only),
                "for": words,
            })
        })
        .collect();
    serde_json::json!({ "groups": groups, "runes": runes, "worth": worth_json(info, &materials) })
}

/// The bosses worth farming in the current difficulty ([`crate::worth`]):
/// `{ "difficulty": "Nightmare", "bosses": [{ "boss", "level", "items": [{ "name", "runes" }] }] }`,
/// only bosses with at least one item, in the drop data's order; `runes` are a runeword's
/// missing runes that drop (empty for a unique / set). `null` with the difficulty unknown.
#[must_use]
pub fn worth_json(info: &PanelInfo, materials: &[(String, u32)]) -> serde_json::Value {
    let Some(difficulty) = info.live.difficulty else {
        return serde_json::Value::Null;
    };
    let holdings = info.live.holdings();
    let bosses: Vec<serde_json::Value> = crate::worth::bosses()
        .filter_map(|boss| {
            let (level, _) = crate::worth::boss_drops(boss, difficulty)?;
            let items = crate::worth::worth(boss, difficulty, info.farm, &holdings, materials);
            (!items.is_empty()).then(|| {
                let items: Vec<serde_json::Value> = items
                    .into_iter()
                    .map(|(name, why)| {
                        let runes = match why {
                            Why::Drops => Vec::new(),
                            Why::Runes(runes) => runes,
                        };
                        serde_json::json!({ "name": name, "runes": runes })
                    })
                    .collect();
                serde_json::json!({ "boss": boss, "level": level, "items": items })
            })
        })
        .collect();
    serde_json::json!({
        "difficulty": d2r_core::info::difficulty_name(difficulty),
        "bosses": bosses,
    })
}

/// Whether farm entry `item` of the group for build `target` is hidden (user request,
/// 2026-11-20): the played character is on that build — its current stage or a later one
/// along the `next` links ([`PanelInfo::stage_chain`]: it keeps the gear through a respec) —
/// and it or its mercenary wears at least `count` copies (its equipped items plus
/// [`LiveInfo::merc_worn`], which is empty with other players in the game: PLAN.md, "Merc gear
/// caveat"; carried, cubed and stashed copies don't count).
pub fn farm_worn(target: &str, item: &crate::config::FarmItem, info: &PanelInfo) -> bool {
    let on_build = info
        .stage_chain
        .iter()
        .any(|b| b.trim().eq_ignore_ascii_case(target.trim()));
    let worn = info
        .live
        .equipped_items
        .iter()
        .filter_map(|e| e.name.as_deref())
        .chain(info.live.merc_worn.iter().map(String::as_str))
        .filter(|n| d2r_core::names::same_name(n, &item.name))
        .count();
    on_build && u32::try_from(worn).is_ok_and(|w| w >= item.count.max(1))
}

/// One farm entry's JSON ([`farm_json`]); a runeword still to build adds its rune needs to
/// `wanted` (rune → total need, runewords needing it).
pub fn farm_item_json(
    item: &crate::config::FarmItem,
    info: &PanelInfo,
    materials: &[(String, u32)],
    wanted: &mut Vec<(&'static str, u32, Vec<String>)>,
) -> serde_json::Value {
    let (stashed, carried) = farm_copies(item, info);
    let status = farm_status(item.count, stashed, carried);
    let to_build = item.count.saturating_sub(stashed.saturating_add(carried));
    let word = (item.kind == crate::config::FarmKind::Runeword && to_build > 0)
        .then(|| d2r_core::runewords_data::lookup(&item.name))
        .flatten();
    let mut runes = Vec::new();
    if let Some(word) = word {
        let inputs: Vec<Input> = word
            .runes
            .iter()
            .map(|r| Input::Material {
                name: format!("{r} Rune"),
                need: to_build,
            })
            .collect();
        // Each distinct rune once, in socket order, with its merged need (a runeword naming
        // one rune twice needs two per copy).
        let totals = need_totals(&inputs);
        let mut seen: Vec<&str> = Vec::new();
        for &rune in word.runes {
            if seen.contains(&rune) {
                continue;
            }
            seen.push(rune);
            let key = format!("{rune} rune").to_lowercase();
            let need = totals.iter().find(|(k, _)| *k == key).map_or(0, |(_, n)| *n);
            let have = rune_count(rune, materials);
            runes.push(serde_json::json!({
                "rune": rune,
                "need": need,
                "have": have,
                "status": if have >= need { "have" } else { "missing" },
            }));
            match wanted.iter_mut().find(|(r, _, _)| *r == rune) {
                Some((_, total, words)) => {
                    *total = total.saturating_add(need);
                    if !words.contains(&item.name) {
                        words.push(item.name.clone());
                    }
                }
                None => wanted.push((rune, need, vec![item.name.clone()])),
            }
        }
    }
    serde_json::json!({
        "name": item.name,
        "kind": item.kind.name(),
        "count": item.count,
        "stashed": stashed,
        "carried": carried,
        "status": status.name(),
        "runes": runes,
        "bases": word.map_or(&[][..], |w| w.bases),
        "base_hint": word.and_then(|w| {
            let wearer = if item.merc { Wearer::Act2Merc } else { Wearer::Player(info.live.class) };
            base_hint(w, info, item.slot.map(crate::config::FarmSlot::body), wearer)
        }),
    })
}
