//! The farm list panel: the cross-character farm list (`farm.toml`) with the copies held and
//! the runes still needed.

use crate::config::FarmItem;
use crate::worth::Why;
use d2r_core::game::Place;
use d2r_core::socket_bases::Wearer;
use d2r_core::web::LiveInfo;

use super::recipes::{Input, need_totals, owned_materials, rune_count};
use super::{PanelInfo, base_hint, merc_base_hint};

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
/// outside it or worn by its mercenary)` ([`d2r_core::stash::Holdings::copies`]); a rolled
/// entry's by its stats ([`rolled_copies`]).
#[must_use]
pub fn farm_copies(item: &FarmItem, info: &PanelInfo) -> (u32, u32) {
    item.kind.item_kind().map_or_else(
        || rolled_copies(item, &info.live),
        |kind| info.live.holdings().copies(&item.name, Some(kind)),
    )
}

/// The copies of rolled entry `item` ([`FarmItem::rolled_match`]): `(in the shared stash, on
/// the played character: worn, inventory, cube, personal stash)`. The mercenary's gear has no
/// stats in the snapshot, so it never counts.
#[must_use]
pub fn rolled_copies(item: &FarmItem, live: &LiveInfo<'_>) -> (u32, u32) {
    let stashed = live
        .stash
        .iter()
        .flat_map(|tab| &tab.items)
        .filter(|i| item.rolled_match(i.code.as_deref(), &i.stats))
        .count();
    let carried = live
        .owned_items
        .iter()
        .filter(|(_, place)| *place != Place::Shared)
        .map(|(i, _)| (i.code.as_deref(), &i.stats))
        .chain(live.equipped_items.iter().map(|e| (e.code.as_deref(), &e.stats)))
        .filter(|(code, stats)| item.rolled_match(*code, stats))
        .count();
    let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    (clamp(stashed), clamp(carried))
}

/// Whether farm entry `item` of the group for `target` leaves the panel: worn ([`farm_worn`]),
/// or a rolled entry needing Teleport once Enigma is held (it gives Teleport; user,
/// 2026-11-20: "very high for the summoner until he gets enigma"; held anywhere the copy
/// counts see: [`crate::gear_score::teleport_wanted`]).
#[must_use]
pub fn farm_hidden(target: &str, item: &FarmItem, info: &PanelInfo) -> bool {
    farm_worn(target, item, info)
        || (item.wants_teleport() && !crate::gear_score::teleport_wanted(&info.live.holdings()))
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
/// wears for its own build (and Teleport entries once Enigma is held) are left out, and a
/// group left empty with them ([`farm_hidden`]):
/// `{ "groups": [{ "for", "items": [{ "name", "kind", "count", "stashed", "carried",
/// "status", "runes", "bases", "slot", "needs", "note" }] }], "runes": [{ "rune", "code",
/// "need", "have", "stash", "for" }], "worth": … }` (`worth`: [`worth_json`]). `slot` is the
/// entry's slot name or `null`, `needs` a rolled entry's stat names, `note` its free text or
/// `null`.
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
            // The group's mercenaries: the current stage's acts when the group is for it, else
            // Act 2 (the panel has no other build file at hand; armor bases don't care).
            let build = info
                .build
                .filter(|b| d2r_core::names::same_name(&b.name, &group.target));
            let items: Vec<serde_json::Value> = group
                .items
                .iter()
                .filter(|item| !farm_hidden(&group.target, item, info))
                .map(|item| farm_item_json(item, info, build, &materials, &mut wanted))
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
pub fn farm_worn(target: &str, item: &FarmItem, info: &PanelInfo) -> bool {
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
/// `wanted` (rune → total need, runewords needing it). A mercenary entry's base hint is for
/// the mercenary of `build` that carries it ([`merc_base_hint`]; Act 2 without a build).
pub fn farm_item_json(
    item: &FarmItem,
    info: &PanelInfo,
    build: Option<&crate::config::Build>,
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
        "slot": item.slot.map(crate::config::FarmSlot::name),
        "needs": item.needs.iter().map(|n| n.name()).collect::<Vec<_>>(),
        "note": item.note,
        "base_hint": word.and_then(|w| {
            let slot = item.slot.map(crate::config::FarmSlot::body);
            if item.merc {
                merc_base_hint(w, info, slot, build)
            } else {
                base_hint(w, info, slot, Wearer::Player(info.live.class))
            }
        }),
    })
}
