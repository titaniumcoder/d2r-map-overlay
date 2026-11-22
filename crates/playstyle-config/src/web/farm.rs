//! The farm list panel: what the cross-character farm list (`farm.toml`) still needs, per
//! build, each entry with where to farm it.

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

/// The farm list section, `null` without one: per farm group (build, in farm-list order) its
/// still-missing entries, each with where to farm it ([`where_text`]); the entries held
/// (`held`, for the pickup sound), the runes still needed over the whole list (in rune order)
/// and the bosses worth farming.
#[must_use]
pub fn farm_json(info: &PanelInfo) -> serde_json::Value {
    if info.farm.is_empty() {
        return serde_json::Value::Null;
    }
    let materials = owned_materials(info);
    let mut wanted = RuneNeeds::new();
    let mut held: Vec<serde_json::Value> = Vec::new();
    let mut counted: Vec<(&str, bool)> = Vec::new();
    let groups: Vec<(&str, Vec<Needed>)> = info
        .farm
        .iter()
        .map(|group| {
            let needed = collect_group(group, info, &materials, &mut wanted, &mut counted, &mut held);
            (group.target.as_str(), needed)
        })
        .collect();
    let spots = Spots::new(info, &materials);
    let groups: Vec<serde_json::Value> = groups
        .into_iter()
        .filter(|(_, needed)| !needed.is_empty())
        .map(|(target, needed)| {
            let items: Vec<serde_json::Value> = needed.iter().map(|n| needed_json(n, &spots)).collect();
            serde_json::json!({ "for": target, "items": items })
        })
        .collect();
    let runes = rune_needs_json(wanted, &materials, &stash_materials(info));
    serde_json::json!({
        "groups": groups,
        "held": held,
        "runes": runes,
        "worth": worth_json(info, &materials),
    })
}

/// One still-missing farm entry of a group.
struct Needed<'a> {
    item: &'a FarmItem,
    stashed: u32,
    word: Option<&'static d2r_core::runewords_data::Runeword>,
    /// Its runes still short: `(rune, need, have)`, in socket order.
    short: Vec<(&'static str, u32, u32)>,
    base_hint: Option<String>,
}

/// Group `group`'s visible entries still missing, in farm-list order; a held one joins `held`.
/// The runes of the copies still to build join `wanted` once per entry over the groups
/// (`counted`: name and wearer already counted).
fn collect_group<'a>(
    group: &'a FarmGroup,
    info: &PanelInfo,
    materials: &[(String, u32)],
    wanted: &mut RuneNeeds,
    counted: &mut Vec<(&'a str, bool)>,
    held: &mut Vec<serde_json::Value>,
) -> Vec<Needed<'a>> {
    let build = info
        .build
        .filter(|b| d2r_core::names::same_name(&b.name, &group.target));
    let mut needed = Vec::new();
    for item in group
        .items
        .iter()
        .filter(|item| !farm_hidden(&group.target, item, info))
    {
        let (stashed, carried) = farm_copies(item, info);
        let to_build = item.count.saturating_sub(stashed.saturating_add(carried));
        let first = !counted
            .iter()
            .any(|&(name, merc)| merc == item.merc && d2r_core::names::same_name(name, &item.name));
        if first {
            counted.push((&item.name, item.merc));
        }
        let mut scratch = RuneNeeds::new();
        let sink = if first { &mut *wanted } else { &mut scratch };
        let word = runeword_of(item);
        let short = word.filter(|_| to_build > 0).map_or_else(Vec::new, |word| {
            runeword_short(word, to_build, &item.name, materials, sink)
        });
        if farm_status(item.count, stashed, carried) != FarmStatus::Missing {
            let copies = stashed.saturating_add(carried).min(item.count.max(1));
            held.push(serde_json::json!({ "for": group.target, "name": item.name, "copies": copies }));
            continue;
        }
        let base_hint = word.and_then(|w| {
            let slot = item.slot.map(crate::config::FarmSlot::body);
            if item.merc {
                merc_base_hint(w, info, slot, build)
            } else {
                base_hint(w, info, slot, Wearer::Player(info.live.class))
            }
        });
        needed.push(Needed {
            item,
            stashed,
            word,
            short,
            base_hint,
        });
    }
    needed
}

fn runeword_of(item: &FarmItem) -> Option<&'static d2r_core::runewords_data::Runeword> {
    (item.kind == crate::config::FarmKind::Runeword)
        .then(|| d2r_core::runewords_data::lookup(&item.name))
        .flatten()
}

fn needed_json(n: &Needed, spots: &Spots) -> serde_json::Value {
    let runes: Vec<serde_json::Value> = n
        .short
        .iter()
        .map(|&(rune, need, have)| serde_json::json!({ "rune": rune, "need": need, "have": have }))
        .collect();
    serde_json::json!({
        "name": n.item.name,
        "kind": n.item.kind.name(),
        "count": n.item.count,
        "stashed": n.stashed,
        "runes": runes,
        "bases": n.word.map_or(&[][..], |w| w.bases),
        "base_hint": n.base_hint,
        "slot": n.item.slot.map(crate::config::FarmSlot::name),
        "needs": n.item.needs.iter().map(|n| n.name()).collect::<Vec<_>>(),
        "note": n.item.note,
        "where": where_text(n, spots),
    })
}

/// Where the still-missing entries drop in the played difficulty: any level-85 area
/// ([`crate::worth::area_85`]) and the route's bosses ([`crate::route::bosses`]) with what
/// each drops ([`crate::worth::worth`]).
struct Spots<'a> {
    difficulty: Option<u8>,
    area_85: Vec<(String, Why)>,
    bosses: Vec<(&'a str, Vec<(String, Why)>)>,
}

impl<'a> Spots<'a> {
    fn new(info: &PanelInfo<'a>, materials: &[(String, u32)]) -> Self {
        let holdings = info.live.holdings();
        let difficulty = info.live.difficulty;
        let bosses = difficulty.map_or_else(Vec::new, |d| {
            crate::route::bosses(info.route)
                .into_iter()
                .map(|boss| {
                    let drops = crate::worth::worth(boss, d, info.farm, &holdings, materials);
                    (boss, drops)
                })
                .collect()
        });
        Spots {
            difficulty,
            area_85: crate::worth::area_85(difficulty, info.farm, &holdings, materials),
            bosses,
        }
    }

    fn why<'s>(list: &'s [(String, Why)], name: &str) -> Option<&'s Why> {
        list.iter()
            .find(|(n, _)| d2r_core::names::same_name(n, name))
            .map(|(_, why)| why)
    }

    /// Where unique or set `name` drops: any level-85 area, else the bosses dropping it.
    fn item(&self, name: &str) -> Option<Spot<'a>> {
        if Self::why(&self.area_85, name).is_some() {
            return Some(Spot::Area85);
        }
        let bosses: Vec<&str> = self
            .bosses
            .iter()
            .filter(|(_, drops)| Self::why(drops, name) == Some(&Why::Drops))
            .map(|&(boss, _)| boss_name(boss))
            .collect();
        (!bosses.is_empty()).then_some(Spot::Bosses(bosses))
    }

    /// Where rune `rune` of runeword `word` drops: the Countess when her rune drop gives it
    /// reliably, else any level-85 area, else the bosses dropping it (the Countess too: her
    /// item drop, without odds).
    fn rune(&self, word: &str, rune: &str) -> Option<Spot<'a>> {
        let has = |list: &[(String, Why)]| matches!(Self::why(list, word), Some(Why::Runes(r)) if r.contains(&rune));
        if let Some(chance) = self
            .difficulty
            .and_then(|d| crate::worth::countess_rune_chance(rune, d))
        {
            return Some(Spot::Countess((100.0 / chance).round()));
        }
        if has(&self.area_85) {
            return Some(Spot::Area85);
        }
        let bosses: Vec<&str> = self
            .bosses
            .iter()
            .filter(|(_, drops)| has(drops))
            .map(|&(boss, _)| boss_name(boss))
            .collect();
        (!bosses.is_empty()).then_some(Spot::Bosses(bosses))
    }
}

/// A place to farm something.
#[derive(Debug, PartialEq)]
enum Spot<'a> {
    Area85,
    /// The Countess's rune drop, one in this many kills.
    Countess(f64),
    Bosses(Vec<&'a str>),
}

impl Spot<'_> {
    fn text(&self) -> String {
        match self {
            Spot::Area85 => "any level-85 area".to_owned(),
            Spot::Countess(_) => "Countess".to_owned(),
            Spot::Bosses(bosses) => bosses.join(", "),
        }
    }

    /// Whether `other` goes into the same part of a where line: the Countess's rune drop
    /// whatever the odds, else the same spot.
    fn joins(&self, other: &Spot) -> bool {
        matches!((self, other), (Spot::Countess(_), Spot::Countess(_))) || self == other
    }
}

fn boss_name(boss: &str) -> &str {
    if crate::worth::is_countess(boss) {
        "Countess"
    } else {
        boss
    }
}

/// Where entry `n` is farmed, one short line; `None` when unknown. A runeword's short runes
/// are grouped by spot: one spot alone (`"any level-85 area"`), else a part per spot, the
/// Countess's with each rune's odds (`"Countess: Ko 1 in 43, Pul 1 in 108 · Vex: any level-85
/// area"`); runes with no known spot are left out. A rolled entry drops in any level-85 area.
fn where_text(n: &Needed, spots: &Spots) -> Option<String> {
    match n.item.kind {
        crate::config::FarmKind::Unique | crate::config::FarmKind::Set => {
            spots.item(&n.item.name).map(|s| s.text())
        }
        crate::config::FarmKind::Rolled => spots
            .difficulty
            .is_none_or(|d| d == crate::worth::HELL)
            .then(|| Spot::Area85.text()),
        crate::config::FarmKind::Runeword => {
            let mut parts: Vec<(Spot, Vec<(&str, f64)>)> = Vec::new();
            let mut unknown = false;
            for &(rune, _, _) in &n.short {
                let Some(spot) = spots.rune(&n.item.name, rune) else {
                    unknown = true;
                    continue;
                };
                let odds = if let Spot::Countess(one_in) = spot {
                    one_in
                } else {
                    0.0
                };
                match parts.iter_mut().find(|(s, _)| s.joins(&spot)) {
                    Some((_, runes)) => runes.push((rune, odds)),
                    None => parts.push((spot, vec![(rune, odds)])),
                }
            }
            if let [(spot, _)] = parts.as_slice()
                && !unknown
                && !matches!(spot, Spot::Countess(_))
            {
                return Some(spot.text());
            }
            let text: Vec<String> = parts
                .iter()
                .map(|(spot, runes)| {
                    if matches!(spot, Spot::Countess(_)) {
                        let runes: Vec<String> =
                            runes.iter().map(|(rune, n)| format!("{rune} 1 in {n}")).collect();
                        format!("Countess: {}", runes.join(", "))
                    } else {
                        let names: Vec<&str> = runes.iter().map(|&(rune, _)| rune).collect();
                        format!("{}: {}", names.join(", "), spot.text())
                    }
                })
                .collect();
            (!text.is_empty()).then(|| text.join(" · "))
        }
    }
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

/// The runes of `to_build` copies of `word` the owned `materials` are short of, each distinct
/// rune once in socket order as `(rune, need, have)`; every rune's need also joins `wanted`
/// for runeword `name`.
fn runeword_short(
    word: &d2r_core::runewords_data::Runeword,
    to_build: u32,
    name: &str,
    materials: &[(String, u32)],
    wanted: &mut RuneNeeds,
) -> Vec<(&'static str, u32, u32)> {
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
    let mut short = Vec::new();
    for &rune in word.runes {
        if seen.contains(&rune) {
            continue;
        }
        seen.push(rune);
        let key = format!("{rune} rune").to_lowercase();
        let need = totals.iter().find(|(k, _)| *k == key).map_or(0, |(_, n)| *n);
        let have = rune_count(rune, materials);
        if have < need {
            short.push((rune, need, have));
        }
        add_wanted(wanted, rune, need, name);
    }
    short
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
