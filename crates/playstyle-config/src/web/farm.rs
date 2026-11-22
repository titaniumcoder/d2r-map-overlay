//! The farm list panel: the cross-character farm list (`farm.toml`) with the copies held and
//! the runes still needed.

use crate::config::{FarmGroup, FarmItem};
use crate::worth::Why;
use d2r_core::game::Place;
use d2r_core::socket_bases::Wearer;
use d2r_core::web::LiveInfo;

use super::recipes::{Input, need_totals, owned_materials, rune_count};
use super::{PanelInfo, base_hint, merc_base_hint};

/// Rune → total need over the farm list, with the runewords needing it.
type RuneNeeds = Vec<(&'static str, u32, Vec<String>)>;

/// Where a farm-list entry stands, across characters: `Done` once the shared stash holds
/// `count` copies, `Found` while the played character carries a copy outside it.
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

/// The copies of farm entry `item`: `(in the shared stash, carried by the character outside it
/// or worn by its mercenary)`.
#[must_use]
pub fn farm_copies(item: &FarmItem, info: &PanelInfo) -> (u32, u32) {
    item.kind.item_kind().map_or_else(
        || rolled_copies(item, &info.live),
        |kind| info.live.holdings().copies(&item.name, Some(kind)),
    )
}

/// The copies of rolled entry `item`: `(in the shared stash, on the character)`. The
/// mercenary's gear has no stats in the snapshot, so it never counts.
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

/// Whether farm entry `item` of the group for `target` leaves the panel: worn, or a Teleport
/// entry once Enigma is held.
#[must_use]
pub fn farm_hidden(target: &str, item: &FarmItem, info: &PanelInfo) -> bool {
    farm_worn(target, item, info)
        || (item.wants_teleport() && !crate::gear_score::teleport_wanted(&info.live.holdings()))
}

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

/// The farm list section, `null` without one: its visible groups, the runes still needed over
/// the whole list (in rune order), and the bosses worth farming.
#[must_use]
pub fn farm_json(info: &PanelInfo) -> serde_json::Value {
    if info.farm.is_empty() {
        return serde_json::Value::Null;
    }
    let materials = owned_materials(info);
    let mut wanted = RuneNeeds::new();
    let groups: Vec<serde_json::Value> = info
        .farm
        .iter()
        .filter_map(|group| group_json(group, info, &materials, &mut wanted))
        .collect();
    let runes = rune_needs_json(wanted, &materials, &stash_materials(info));
    serde_json::json!({ "groups": groups, "runes": runes, "worth": worth_json(info, &materials) })
}

/// One farm group's visible entries; `None` when every entry is hidden.
fn group_json(
    group: &FarmGroup,
    info: &PanelInfo,
    materials: &[(String, u32)],
    wanted: &mut RuneNeeds,
) -> Option<serde_json::Value> {
    let build = info
        .build
        .filter(|b| d2r_core::names::same_name(&b.name, &group.target));
    let items: Vec<serde_json::Value> = group
        .items
        .iter()
        .filter(|item| !farm_hidden(&group.target, item, info))
        .map(|item| farm_item_json(item, info, build, materials, wanted))
        .collect();
    (!items.is_empty()).then(|| serde_json::json!({ "for": group.target, "items": items }))
}

/// The materials of the shared stash's materials tab alone.
fn stash_materials(info: &PanelInfo) -> Vec<(String, u32)> {
    owned_materials(&PanelInfo {
        live: LiveInfo {
            held_materials: &[],
            ..info.live
        },
        ..*info
    })
}

/// The farm list's rune needs in rune order, each with its owned (`have`) and stash-tab
/// (`stash`) counts.
fn rune_needs_json(
    mut wanted: RuneNeeds,
    materials: &[(String, u32)],
    stash_only: &[(String, u32)],
) -> Vec<serde_json::Value> {
    wanted.sort_by_key(|(rune, _, _)| d2r_core::stash::RUNE_NAMES.iter().position(|r| r == rune));
    wanted
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
                "have": count(materials),
                "stash": count(stash_only),
                "for": words,
            })
        })
        .collect()
}

/// The bosses worth farming in the current difficulty ([`crate::worth`]), only those with at
/// least one item; `null` with the difficulty unknown.
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
            (!items.is_empty()).then(|| boss_json(boss, level, difficulty, items))
        })
        .collect();
    serde_json::json!({
        "difficulty": d2r_core::info::difficulty_name(difficulty),
        "bosses": bosses,
    })
}

/// One boss worth farming; the Countess also lists her `reliable` and `rare` runes.
fn boss_json(boss: &str, level: u32, difficulty: u8, items: Vec<(String, Why)>) -> serde_json::Value {
    let split = crate::worth::is_countess(boss).then(|| countess_runes_json(difficulty, &items));
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
    let mut out = serde_json::json!({ "boss": boss, "level": level, "items": items });
    if let (Some((reliable, rare)), Some(map)) = (split, out.as_object_mut()) {
        map.insert("reliable".to_owned(), reliable.into());
        map.insert("rare".to_owned(), rare.into());
    }
    out
}

/// The Countess's runes for `items`: `(from her rune TC, only from her item TC)`.
fn countess_runes_json(
    difficulty: u8,
    items: &[(String, Why)],
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let runes = crate::worth::countess_runes(difficulty, items);
    let part = |reliable: bool| -> Vec<serde_json::Value> {
        runes
            .iter()
            .filter(|r| r.chance.is_some() == reliable)
            .map(|r| serde_json::json!({ "rune": r.rune, "one_in": r.one_in(), "for": r.entries }))
            .collect()
    };
    (part(true), part(false))
}

/// Whether farm entry `item` of the group for build `target` is hidden: the character is on
/// that build (or an earlier stage of it) and it or its mercenary wears `count` copies.
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

/// One farm entry's JSON; a runeword still to build adds its rune needs to `wanted`. A
/// mercenary entry's base hint is for the mercenary of `build` that carries it.
pub fn farm_item_json(
    item: &FarmItem,
    info: &PanelInfo,
    build: Option<&crate::config::Build>,
    materials: &[(String, u32)],
    wanted: &mut RuneNeeds,
) -> serde_json::Value {
    let (stashed, carried) = farm_copies(item, info);
    let status = farm_status(item.count, stashed, carried);
    let to_build = item.count.saturating_sub(stashed.saturating_add(carried));
    let word = (item.kind == crate::config::FarmKind::Runeword && to_build > 0)
        .then(|| d2r_core::runewords_data::lookup(&item.name))
        .flatten();
    let runes = word.map_or_else(Vec::new, |word| {
        runeword_needs_json(word, to_build, &item.name, materials, wanted)
    });
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

/// The runes of `to_build` copies of `word`, each distinct rune once in socket order, also
/// added to `wanted` for runeword `name`.
fn runeword_needs_json(
    word: &d2r_core::runewords_data::Runeword,
    to_build: u32,
    name: &str,
    materials: &[(String, u32)],
    wanted: &mut RuneNeeds,
) -> Vec<serde_json::Value> {
    let inputs: Vec<Input> = word
        .runes
        .iter()
        .map(|r| Input::Material {
            name: format!("{r} Rune"),
            need: to_build,
        })
        .collect();
    let totals = need_totals(&inputs);
    let mut seen: Vec<&str> = Vec::new();
    let mut runes = Vec::new();
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
        add_wanted(wanted, rune, need, name);
    }
    runes
}

fn add_wanted(wanted: &mut RuneNeeds, rune: &'static str, need: u32, name: &str) {
    match wanted.iter_mut().find(|(r, _, _)| *r == rune) {
        Some((_, total, words)) => {
            *total = total.saturating_add(need);
            if !words.iter().any(|w| w == name) {
                words.push(name.to_owned());
            }
        }
        None => wanted.push((rune, need, vec![name.to_owned()])),
    }
}
