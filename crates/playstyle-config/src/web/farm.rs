//! The farm list panel: what the cross-character farm list (`farm.toml`) still needs, as
//! cards by where it drops.

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

/// The farm list section, `null` without one: the still-missing entries as cards
/// ([`cards_json`]), the entries held (`held`, for the pickup sound), the runes still needed
/// over the whole list (in rune order) and the bosses worth farming.
#[must_use]
pub fn farm_json(info: &PanelInfo) -> serde_json::Value {
    if info.farm.is_empty() {
        return serde_json::Value::Null;
    }
    let materials = owned_materials(info);
    let mut wanted = RuneNeeds::new();
    let mut needed: Vec<Needed> = Vec::new();
    let mut held: Vec<serde_json::Value> = Vec::new();
    for group in info.farm {
        collect_group(group, info, &materials, &mut wanted, &mut needed, &mut held);
    }
    needed.sort_by_cached_key(|n| (build_rank(&n.builds, info.farm), n.item.name.to_lowercase()));
    let cards = cards_json(info, &needed, &materials, &wanted);
    let runes = rune_needs_json(wanted, &materials, &stash_materials(info));
    serde_json::json!({
        "cards": cards,
        "held": held,
        "runes": runes,
        "worth": worth_json(info, &materials),
    })
}

/// One still-missing farm entry, merged over the groups (builds) needing it.
struct Needed<'a> {
    item: &'a FarmItem,
    /// The farm group targets it is for, in farm-list order.
    builds: Vec<&'a str>,
    word: Option<&'static d2r_core::runewords_data::Runeword>,
    /// Its runes still short: `(rune, how many)`, in socket order.
    short: Vec<(&'static str, u32)>,
    base_hint: Option<String>,
}

/// Sorts group `group`'s visible entries: a still-missing one joins `needed` (merged by name
/// and wearer, its runes added to `wanted`), a held one `held` (the runes of copies still to
/// build added to `wanted` all the same).
fn collect_group<'a>(
    group: &'a FarmGroup,
    info: &PanelInfo,
    materials: &[(String, u32)],
    wanted: &mut RuneNeeds,
    needed: &mut Vec<Needed<'a>>,
    held: &mut Vec<serde_json::Value>,
) {
    let build = info
        .build
        .filter(|b| d2r_core::names::same_name(&b.name, &group.target));
    for item in group
        .items
        .iter()
        .filter(|item| !farm_hidden(&group.target, item, info))
    {
        let (stashed, carried) = farm_copies(item, info);
        let to_build = item.count.saturating_sub(stashed.saturating_add(carried));
        if farm_status(item.count, stashed, carried) != FarmStatus::Missing {
            let copies = stashed.saturating_add(carried).min(item.count.max(1));
            held.push(serde_json::json!({ "for": group.target, "name": item.name, "copies": copies }));
            if let Some(word) = runeword_of(item).filter(|_| to_build > 0) {
                runeword_short(word, to_build, &item.name, materials, wanted);
            }
        } else if let Some(entry) = needed
            .iter_mut()
            .find(|n| n.item.merc == item.merc && d2r_core::names::same_name(&n.item.name, &item.name))
        {
            if !entry.builds.contains(&group.target.as_str()) {
                entry.builds.push(&group.target);
            }
        } else {
            needed.push(needed_entry(
                item,
                &group.target,
                to_build,
                info,
                build,
                materials,
                wanted,
            ));
        }
    }
}

/// Still-missing entry `item` of the group for `target`, `to_build` copies to go; a runeword's
/// runes join `wanted`. A mercenary entry's base hint is for the mercenary of `build`.
fn needed_entry<'a>(
    item: &'a FarmItem,
    target: &'a str,
    to_build: u32,
    info: &PanelInfo,
    build: Option<&crate::config::Build>,
    materials: &[(String, u32)],
    wanted: &mut RuneNeeds,
) -> Needed<'a> {
    let word = runeword_of(item);
    let short = word.map_or_else(Vec::new, |word| {
        runeword_short(word, to_build, &item.name, materials, wanted)
    });
    let base_hint = word.and_then(|w| {
        let slot = item.slot.map(crate::config::FarmSlot::body);
        if item.merc {
            merc_base_hint(w, info, slot, build)
        } else {
            base_hint(w, info, slot, Wearer::Player(info.live.class))
        }
    });
    Needed {
        item,
        builds: vec![target],
        word,
        short,
        base_hint,
    }
}

fn runeword_of(item: &FarmItem) -> Option<&'static d2r_core::runewords_data::Runeword> {
    (item.kind == crate::config::FarmKind::Runeword)
        .then(|| d2r_core::runewords_data::lookup(&item.name))
        .flatten()
}

/// Sort rank of an entry for `builds`: one for several builds first, then by farm group.
fn build_rank(builds: &[&str], farm: &[FarmGroup]) -> usize {
    match builds {
        [one] => farm
            .iter()
            .position(|g| g.target == *one)
            .map_or(usize::MAX, |i| i.saturating_add(1)),
        _ => 0,
    }
}

/// The short build tag of an entry for `builds`: `"Fury"` for "Fury Druid", `"both"` for two.
fn build_tag(builds: &[&str]) -> String {
    let short = |name: &str| name.split_whitespace().next().unwrap_or(name).to_owned();
    match builds {
        [one] => short(one),
        [_, _] => "both".to_owned(),
        _ => builds.iter().map(|b| short(b)).collect::<Vec<_>>().join(" + "),
    }
}

/// The farm cards: "General drops" (what any level-85 area drops: uniques and sets of quality
/// level 85 at most, runes the Countess doesn't drop reliably, rolled entries), then one per
/// route boss in the played difficulty with what only it drops (higher uniques and sets; the
/// Countess: her reliable runes, with odds), then "Elsewhere" for entries on no card. A card
/// with nothing on it is left out.
fn cards_json(
    info: &PanelInfo,
    needed: &[Needed],
    materials: &[(String, u32)],
    wanted: &RuneNeeds,
) -> Vec<serde_json::Value> {
    let holdings = info.live.holdings();
    let difficulty = info.live.difficulty;
    let general = crate::worth::area_85(difficulty, info.farm, &holdings, materials);
    let mut placed: Vec<&str> = Vec::new();
    let mut cards: Vec<serde_json::Value> = Vec::new();
    cards.extend(general_card(needed, &general, &mut placed));
    if let Some(d) = difficulty {
        for boss in crate::route::bosses(info.route) {
            let special = boss_special(boss, d, info, &general, materials);
            cards.extend(boss_card(
                boss,
                d,
                needed,
                &special,
                materials,
                wanted,
                &mut placed,
            ));
        }
    }
    cards.extend(elsewhere_card(needed, &placed, difficulty));
    cards
}

fn card_json(
    kind: &str,
    title: &str,
    sub: &str,
    items: Vec<serde_json::Value>,
    runes: Vec<serde_json::Value>,
) -> serde_json::Value {
    let mut card = serde_json::json!({ "kind": kind, "title": title, "sub": sub });
    if let Some(map) = card.as_object_mut() {
        map.insert("items".to_owned(), items.into());
        map.insert("runes".to_owned(), runes.into());
    }
    card
}

fn why_runes(why: &Why) -> Vec<&'static str> {
    match why {
        Why::Drops => Vec::new(),
        Why::Runes(runes) => runes.clone(),
    }
}

/// The "General drops" card: the entries of `general` ([`crate::worth::area_85`]) and the
/// rolled entries.
fn general_card<'a>(
    needed: &'a [Needed],
    general: &[(String, Why)],
    placed: &mut Vec<&'a str>,
) -> Option<serde_json::Value> {
    let items: Vec<serde_json::Value> = needed
        .iter()
        .filter_map(|n| {
            let runes = if n.item.kind == crate::config::FarmKind::Rolled {
                Vec::new()
            } else {
                why_runes(
                    &general
                        .iter()
                        .find(|(name, _)| d2r_core::names::same_name(name, &n.item.name))?
                        .1,
                )
            };
            placed.push(&n.item.name);
            Some(card_item_json(n, &runes))
        })
        .collect();
    (!items.is_empty()).then(|| card_json("general", "General drops", "any level-85 area", items, Vec::new()))
}

/// The needed entries boss `boss` drops in `difficulty` that no level-85 area (`general`) can;
/// runewords only for bosses other than the Countess (her runes have their own lines).
fn boss_special(
    boss: &str,
    difficulty: u8,
    info: &PanelInfo,
    general: &[(String, Why)],
    materials: &[(String, u32)],
) -> Vec<(String, Why)> {
    crate::worth::worth(boss, difficulty, info.farm, &info.live.holdings(), materials)
        .into_iter()
        .filter(|(_, why)| *why == Why::Drops || !crate::worth::is_countess(boss))
        .filter(|(name, _)| !general.iter().any(|(g, _)| d2r_core::names::same_name(g, name)))
        .collect()
}

/// Boss `boss`'s card: the needed entries of `special`; the Countess adds the runes still
/// short that her rune drop gives in `difficulty`, each with its odds per kill.
fn boss_card<'a>(
    boss: &str,
    difficulty: u8,
    needed: &'a [Needed],
    special: &[(String, Why)],
    materials: &[(String, u32)],
    wanted: &RuneNeeds,
    placed: &mut Vec<&'a str>,
) -> Option<serde_json::Value> {
    let (level, _) = crate::worth::boss_drops(boss, difficulty)?;
    let items: Vec<serde_json::Value> = needed
        .iter()
        .filter_map(|n| {
            let (_, why) = special
                .iter()
                .find(|(s, _)| d2r_core::names::same_name(s, &n.item.name))?;
            placed.push(&n.item.name);
            Some(card_item_json(n, &why_runes(why)))
        })
        .collect();
    let runes = if crate::worth::is_countess(boss) {
        countess_runes_short(difficulty, wanted, materials, needed, placed)
    } else {
        Vec::new()
    };
    if items.is_empty() && runes.is_empty() {
        return None;
    }
    let name = d2r_core::info::difficulty_name(difficulty).unwrap_or_default();
    Some(card_json(
        "boss",
        boss,
        &format!("{name} · level {level}"),
        items,
        runes,
    ))
}

/// The runes over `wanted` still short that the Countess's rune drop gives in `difficulty`,
/// by rank: `{rune, short, one_in, for}`; the runewords needing them count as placed.
fn countess_runes_short<'a>(
    difficulty: u8,
    wanted: &RuneNeeds,
    materials: &[(String, u32)],
    needed: &'a [Needed],
    placed: &mut Vec<&'a str>,
) -> Vec<serde_json::Value> {
    let mut runes: Vec<(usize, serde_json::Value)> = Vec::new();
    for (rune, need, words) in wanted {
        let short = need.saturating_sub(rune_count(rune, materials));
        let Some(chance) = crate::worth::countess_rune_chance(rune, difficulty) else {
            continue;
        };
        if short == 0 {
            continue;
        }
        placed.extend(
            needed
                .iter()
                .filter(|n| words.iter().any(|w| w == &n.item.name))
                .map(|n| n.item.name.as_str()),
        );
        let rank = d2r_core::stash::RUNE_NAMES.iter().position(|r| r == rune);
        let one_in = (100.0 / chance).round();
        let json = serde_json::json!({ "rune": rune, "short": short, "one_in": one_in, "for": words });
        runes.push((rank.unwrap_or(usize::MAX), json));
    }
    runes.sort_by_key(|(rank, _)| *rank);
    runes.into_iter().map(|(_, json)| json).collect()
}

/// The "Elsewhere" card: the needed entries on no other card, with all their runes short.
fn elsewhere_card(needed: &[Needed], placed: &[&str], difficulty: Option<u8>) -> Option<serde_json::Value> {
    let items: Vec<serde_json::Value> = needed
        .iter()
        .filter(|n| !placed.contains(&n.item.name.as_str()))
        .map(|n| {
            let runes: Vec<&'static str> = n.short.iter().map(|&(rune, _)| rune).collect();
            card_item_json(n, &runes)
        })
        .collect();
    let sub = difficulty.and_then(d2r_core::info::difficulty_name).map_or_else(
        || "the bosses show in a game".to_owned(),
        |name| format!("not on a card above in {name}"),
    );
    (!items.is_empty()).then(|| card_json("other", "Elsewhere", &sub, items, Vec::new()))
}

/// One entry on a card, with the runes `runes` of its short ones.
fn card_item_json(n: &Needed, runes: &[&str]) -> serde_json::Value {
    let runes: Vec<serde_json::Value> = n
        .short
        .iter()
        .filter(|(rune, _)| runes.contains(rune))
        .map(|&(rune, short)| serde_json::json!({ "rune": rune, "short": short }))
        .collect();
    serde_json::json!({
        "name": n.item.name,
        "kind": n.item.kind.name(),
        "tag": build_tag(&n.builds),
        "for": n.builds,
        "runes": runes,
        "bases": n.word.map_or(&[][..], |w| w.bases),
        "base_hint": n.base_hint,
        "slot": n.item.slot.map(crate::config::FarmSlot::name),
        "needs": n.item.needs.iter().map(|n| n.name()).collect::<Vec<_>>(),
        "note": n.item.note,
    })
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
/// rune once in socket order; every rune's need also joins `wanted` for runeword `name`.
fn runeword_short(
    word: &d2r_core::runewords_data::Runeword,
    to_build: u32,
    name: &str,
    materials: &[(String, u32)],
    wanted: &mut RuneNeeds,
) -> Vec<(&'static str, u32)> {
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
        let missing = need.saturating_sub(rune_count(rune, materials));
        if missing > 0 {
            short.push((rune, missing));
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
