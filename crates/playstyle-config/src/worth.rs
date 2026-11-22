//! "Worth farming": a boss is worth farming while a still-missing farm-list item can drop from
//! it at its level ([`d2r_core::drops_data`]). Runes count only above the Countess's highest
//! rune in the difficulty; she is the farm for the low ones.

use crate::config::{FarmGroup, FarmItem, FarmKind};
use d2r_core::item_levels_data::{set_qlvl, unique_qlvl};
use d2r_core::item_names_data::{SET_CODES, SET_NAMES, UNIQUE_CODES, UNIQUE_NAMES};
use d2r_core::names::{same_name, shown};
use d2r_core::stash::{Holdings, RUNE_NAMES};

use crate::web::farm::{FarmStatus, farm_status};
use crate::web::recipes::rune_count;

/// Why a farm entry makes a boss worth farming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// The unique or set item itself can drop.
    Drops,
    /// The runeword's still-missing runes that can drop, in socket order.
    Runes(Vec<&'static str>),
}

/// The bosses with drop data: act bosses, quest bosses, then the farmed superuniques.
pub fn bosses() -> impl Iterator<Item = &'static str> {
    d2r_core::drops_data::BOSS_DROPS.iter().map(|&(name, _, _)| name)
}

/// Boss `boss`'s level (the item level of its drops) and sorted drop codes in `difficulty`.
#[must_use]
pub fn boss_drops(boss: &str, difficulty: u8) -> Option<(u32, &'static [&'static str])> {
    let d = usize::from(difficulty);
    let &(_, levels, lists) = d2r_core::drops_data::BOSS_DROPS
        .iter()
        .find(|(name, _, _)| same_name(name, boss))?;
    Some((*levels.get(d)?, *lists.get(d)?))
}

/// The still-missing farm entries boss `boss` can drop in `difficulty`, each once, in farm-list
/// order, with why. `materials` are the runes the character owns. A rolled entry counts for no
/// boss: any boss dropping its slot could drop it.
#[must_use]
pub fn worth(
    boss: &str,
    difficulty: u8,
    farm: &[FarmGroup],
    holdings: &Holdings,
    materials: &[(String, u32)],
) -> Vec<(String, Why)> {
    let Some((ilvl, drops)) = boss_drops(boss, difficulty) else {
        return Vec::new();
    };
    let rune_floor = rune_floor(boss, difficulty);
    let counts = |rune: &str| rune_rank(rune).is_some_and(|rank| rune_floor.is_none_or(|floor| rank > floor));
    worth_in(ilvl, drops, &counts, farm, holdings, materials)
}

/// The still-missing farm entries any monster of a Hell area of level 85 can drop
/// ([`d2r_core::drops_data::AREA_85`]), as [`worth`] gives them; the runes the Countess drops
/// reliably in `difficulty` are left to her (none without a difficulty).
#[must_use]
pub fn area_85(
    difficulty: Option<u8>,
    farm: &[FarmGroup],
    holdings: &Holdings,
    materials: &[(String, u32)],
) -> Vec<(String, Why)> {
    let (ilvl, drops) = d2r_core::drops_data::AREA_85;
    let counts = |rune: &str| difficulty.is_none_or(|d| countess_rune_chance(rune, d).is_none());
    worth_in(ilvl, drops, &counts, farm, holdings, materials)
}

/// The still-missing farm entries a drop of item level `ilvl` from the sorted codes `drops`
/// can be, a runeword only for its missing runes that `counts`.
fn worth_in(
    ilvl: u32,
    drops: &[&str],
    counts: &dyn Fn(&str) -> bool,
    farm: &[FarmGroup],
    holdings: &Holdings,
    materials: &[(String, u32)],
) -> Vec<(String, Why)> {
    let drops_code = |code: &str| drops.binary_search(&code).is_ok();
    let farmed_here = |rune: &str| counts(rune) && rune_code(rune).is_some_and(|c| drops_code(&c));
    let mut out: Vec<(String, Why)> = Vec::new();
    for item in farm.iter().flat_map(|g| &g.items) {
        if out.iter().any(|(name, _)| same_name(name, &item.name)) {
            continue;
        }
        let Some(to_build) = copies_to_build(item, holdings) else {
            continue;
        };
        let why = match item.kind {
            FarmKind::Unique | FarmKind::Set => {
                unique_set_drops(item, ilvl, &drops_code).then_some(Why::Drops)
            }
            FarmKind::Runeword => {
                let dropping: Vec<&'static str> = missing_runes(&item.name, to_build, materials)
                    .into_iter()
                    .filter(|r| farmed_here(r))
                    .collect();
                (!dropping.is_empty()).then_some(Why::Runes(dropping))
            }
            FarmKind::Rolled => None,
        };
        if let Some(why) = why {
            out.push((item.name.clone(), why));
        }
    }
    out
}

/// The rank of the highest rune left to the Countess when farming `boss`; `None`: every rune
/// counts (the Countess herself, or no Countess data).
fn rune_floor(boss: &str, difficulty: u8) -> Option<usize> {
    if is_countess(boss) {
        None
    } else {
        countess_top_rune(difficulty)
    }
}

/// How many copies of a still-missing farm entry are left to get; `None` when it is done or
/// held, as the farm panel counts it.
fn copies_to_build(item: &FarmItem, holdings: &Holdings) -> Option<u32> {
    let (stashed, carried) = holdings.copies(&item.name, item.kind.item_kind());
    (farm_status(item.count, stashed, carried) == FarmStatus::Missing)
        .then(|| item.count.saturating_sub(stashed.saturating_add(carried)))
}

/// Whether any unique or set row named like `item` (several can share a name) has a base that
/// drops and a quality level of at most `ilvl`.
fn unique_set_drops(item: &FarmItem, ilvl: u32, drops_code: &impl Fn(&str) -> bool) -> bool {
    let (names, codes, qlvl): (_, _, fn(u32) -> Option<u32>) = match item.kind {
        FarmKind::Set => (SET_NAMES, SET_CODES, set_qlvl),
        FarmKind::Unique | FarmKind::Runeword | FarmKind::Rolled => (UNIQUE_NAMES, UNIQUE_CODES, unique_qlvl),
    };
    names
        .iter()
        .filter(|&&(_, index)| same_name(index, &item.name) || same_name(shown(index), &item.name))
        .any(|&(row, _)| {
            let code = codes
                .binary_search_by_key(&row, |&(r, _)| r)
                .ok()
                .and_then(|i| codes.get(i))
                .map(|&(_, code)| code);
            code.is_some_and(drops_code) && qlvl(row).is_some_and(|q| q <= ilvl)
        })
}

/// The distinct runes of runeword `name`, in socket order, of which the owned `materials` hold
/// fewer than `to_build` copies need.
fn missing_runes(name: &str, to_build: u32, materials: &[(String, u32)]) -> Vec<&'static str> {
    let Some(word) = d2r_core::runewords_data::lookup(name) else {
        return Vec::new();
    };
    let mut out: Vec<&'static str> = Vec::new();
    for &rune in word.runes {
        if out.contains(&rune) {
            continue;
        }
        let per_copy = word.runes.iter().filter(|&&r| r == rune).count();
        let need = u32::try_from(per_copy)
            .unwrap_or(u32::MAX)
            .saturating_mul(to_build);
        if rune_count(rune, materials) < need {
            out.push(rune);
        }
    }
    out
}

/// The boss whose rune drops set the floor for every other boss's runes.
pub const COUNTESS: &str = "The Countess";

#[must_use]
pub fn is_countess(boss: &str) -> bool {
    same_name(boss, COUNTESS)
}

/// One rune the Countess is worth farming for ([`countess_runes`]).
#[derive(Debug, Clone, PartialEq)]
pub struct CountessRune {
    pub rune: &'static str,
    /// Percent chance of at least one per kill from her rune TC; `None`: rare (item TC only).
    pub chance: Option<f64>,
    pub entries: Vec<String>,
}

impl CountessRune {
    /// Kills per drop, rounded ("1 in 43"); `None` for a rare rune.
    #[must_use]
    pub fn one_in(&self) -> Option<f64> {
        self.chance.map(|c| (100.0 / c).round())
    }
}

/// The per-kill chance in percent of rune `name` from the Countess's rune TC.
#[must_use]
pub fn countess_rune_chance(name: &str, difficulty: u8) -> Option<f64> {
    let code = rune_code(name)?;
    let runes = d2r_core::drops_data::COUNTESS_RUNES.get(usize::from(difficulty))?;
    runes.iter().find(|(c, _)| *c == code).map(|&(_, chance)| chance)
}

/// The runes of the Countess's [`worth`] items, each once with the entries needing it:
/// reliable ones first, then rare ones, each by rune rank.
#[must_use]
pub fn countess_runes(difficulty: u8, worth: &[(String, Why)]) -> Vec<CountessRune> {
    let mut out: Vec<CountessRune> = Vec::new();
    for (name, why) in worth {
        let Why::Runes(runes) = why else { continue };
        for &rune in runes {
            match out.iter_mut().find(|r| r.rune == rune) {
                Some(r) => r.entries.push(name.clone()),
                None => out.push(CountessRune {
                    rune,
                    chance: countess_rune_chance(rune, difficulty),
                    entries: vec![name.clone()],
                }),
            }
        }
    }
    out.sort_by_key(|r| (r.chance.is_none(), rune_rank(r.rune)));
    out
}

/// The Countess's worth line as parts, empty ones left out:
/// `"reliable: Ko (1 in 43: Heart of the Oak, Lawbringer), Lem (1 in 82: Wealth)"`,
/// `"rare: Vex (Heart of the Oak)"` and `"items: Bloodfist"` (uniques and sets that drop).
#[must_use]
pub fn countess_parts(difficulty: u8, worth: &[(String, Why)]) -> Vec<String> {
    let runes = countess_runes(difficulty, worth);
    let shown = |r: &CountessRune| {
        let odds = r.one_in().map_or_else(String::new, |n| format!("1 in {n}: "));
        format!("{} ({odds}{})", r.rune, r.entries.join(", "))
    };
    let reliable: Vec<String> = runes.iter().filter(|r| r.chance.is_some()).map(shown).collect();
    let rare: Vec<String> = runes.iter().filter(|r| r.chance.is_none()).map(shown).collect();
    let items: Vec<&str> = worth
        .iter()
        .filter(|(_, why)| *why == Why::Drops)
        .map(|(name, _)| name.as_str())
        .collect();
    [
        ("reliable", reliable.join(", ")),
        ("rare", rare.join(", ")),
        ("items", items.join(", ")),
    ]
    .into_iter()
    .filter(|(_, list)| !list.is_empty())
    .map(|(label, list)| format!("{label}: {list}"))
    .collect()
}

/// The runes the Countess drops in `difficulty` still missing over the whole farm list, with
/// how many (owned `materials` subtracted): reliable ones first, then rare ones, each by rank.
#[must_use]
pub fn countess_missing(
    difficulty: u8,
    farm: &[FarmGroup],
    holdings: &Holdings,
    materials: &[(String, u32)],
) -> Vec<(&'static str, u32)> {
    let Some((_, drops)) = boss_drops(COUNTESS, difficulty) else {
        return Vec::new();
    };
    let need = runes_to_build(farm, holdings);
    let mut out: Vec<(&'static str, u32)> = need
        .into_iter()
        .filter(|(rune, _)| rune_code(rune).is_some_and(|c| drops.binary_search(&c.as_str()).is_ok()))
        .map(|(rune, n)| (rune, n.saturating_sub(rune_count(rune, materials))))
        .filter(|&(_, n)| n > 0)
        .collect();
    out.sort_by_key(|&(rune, _)| (countess_rune_chance(rune, difficulty).is_none(), rune_rank(rune)));
    out
}

/// Every rune of the still-missing runeword entries (each name once) × the copies still to
/// build, summed per rune.
fn runes_to_build(farm: &[FarmGroup], holdings: &Holdings) -> Vec<(&'static str, u32)> {
    let mut seen: Vec<&str> = Vec::new();
    let mut need: Vec<(&'static str, u32)> = Vec::new();
    for item in farm.iter().flat_map(|g| &g.items) {
        if item.kind != FarmKind::Runeword || seen.iter().any(|n| same_name(n, &item.name)) {
            continue;
        }
        seen.push(&item.name);
        let Some(to_build) = copies_to_build(item, holdings) else {
            continue;
        };
        let Some(word) = d2r_core::runewords_data::lookup(&item.name) else {
            continue;
        };
        for &rune in word.runes {
            match need.iter_mut().find(|(r, _)| *r == rune) {
                Some((_, n)) => *n = n.saturating_add(to_build),
                None => need.push((rune, to_build)),
            }
        }
    }
    need
}

/// The Countess's Next-panel text from [`countess_missing`]: `"Ko ×3 (1 in 43), Pul (1 in
/// 108) · rare: Vex, Lo"` (×1 left out), or `"no runes missing"`.
#[must_use]
pub fn countess_next(difficulty: u8, missing: &[(&str, u32)]) -> String {
    let times = |rune: &str, n: u32| {
        if n > 1 {
            format!("{rune} ×{n}")
        } else {
            rune.to_owned()
        }
    };
    let mut reliable: Vec<String> = Vec::new();
    let mut rare: Vec<String> = Vec::new();
    for &(rune, n) in missing {
        match countess_rune_chance(rune, difficulty) {
            Some(chance) => reliable.push(format!("{} (1 in {})", times(rune, n), (100.0 / chance).round())),
            None => rare.push(times(rune, n)),
        }
    }
    let rare = (!rare.is_empty()).then(|| format!("rare: {}", rare.join(", ")));
    let reliable = (!reliable.is_empty()).then(|| reliable.join(", "));
    let parts: Vec<String> = [reliable, rare].into_iter().flatten().collect();
    if parts.is_empty() {
        "no runes missing".to_owned()
    } else {
        parts.join(" · ")
    }
}

/// The rank (0 = El) of the Countess's highest droppable rune in `difficulty`.
fn countess_top_rune(difficulty: u8) -> Option<usize> {
    let (_, drops) = boss_drops(COUNTESS, difficulty)?;
    (0..RUNE_NAMES.len())
        .rev()
        .find(|&i| drops.binary_search(&rune_code_at(i).as_str()).is_ok())
}

/// The rank of rune `name` (`"El"` → 0, `"Zod"` → 32).
fn rune_rank(name: &str) -> Option<usize> {
    RUNE_NAMES.iter().position(|r| r.eq_ignore_ascii_case(name))
}

/// The item code of the rune of rank `rank` (17 → `"r18"`).
fn rune_code_at(rank: usize) -> String {
    format!("r{:02}", rank.saturating_add(1))
}

/// The item code of rune `name` (`"Ko"` → `"r18"`).
fn rune_code(name: &str) -> Option<String> {
    rune_rank(name).map(rune_code_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};

    const NO_HOLDINGS: Holdings<'static> = Holdings {
        stash: &[],
        owned: &[],
        runeword_copies: &[],
        merc_worn: &[],
    };
    const NIGHTMARE: u8 = 1;
    const HELL: u8 = 2;

    fn shipped_farm() -> Result<Vec<FarmGroup>> {
        let text = include_str!("../../../playstyles/summoner-fury/farm.toml");
        let (groups, warnings) = crate::config::parse_farm(text)?;
        assert_eq!(warnings, Vec::<String>::new());
        Ok(groups)
    }

    fn farm_of(entries: &str) -> Result<Vec<FarmGroup>> {
        let text = format!("[[farm]]\nfor = \"Test\"\nitems = [\n{entries}\n]\n");
        Ok(crate::config::parse_farm(&text)?.0)
    }

    /// `worth` as `"name"` / `"name (Ko, Lum)"` strings.
    fn shown_worth(boss: &str, farm: &[FarmGroup], materials: &[(String, u32)]) -> Vec<String> {
        worth(boss, NIGHTMARE, farm, &NO_HOLDINGS, materials)
            .into_iter()
            .map(|(name, why)| match why {
                Why::Drops => name,
                Why::Runes(r) => format!("{name} ({})", r.join(", ")),
            })
            .collect()
    }

    #[test]
    fn a_rolled_entry_never_makes_a_boss_worth_farming() -> Result<()> {
        let farm = farm_of(
            "{ name = \"Teleport Amulet\", kind = \"rolled\", slot = \"amulet\", needs = [\"Teleport\"] },",
        )?;
        for boss in bosses() {
            for difficulty in 0..3 {
                assert_eq!(
                    worth(boss, difficulty, &farm, &NO_HOLDINGS, &[]),
                    Vec::new(),
                    "{boss} in difficulty {difficulty}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn drop_lists_are_sorted_and_cover_every_boss_and_difficulty() {
        for &(name, levels, lists) in d2r_core::drops_data::BOSS_DROPS {
            assert!(
                levels.windows(2).all(|w| matches!(w, [a, b] if a < b)),
                "{name}: levels rise"
            );
            for list in lists {
                assert!(list.is_sorted(), "{name}: binary_search needs sorted codes");
                assert!(!list.is_empty(), "{name}");
            }
        }
        for boss in [
            "Andariel",
            "Duriel",
            "Mephisto",
            "Diablo",
            "Baal",
            "Radament",
            "Summoner",
            "Izual",
            "Nihlathak",
            "Griswold",
            "Blood Raven",
            "The Countess",
            "Pindleskin",
            "Eldritch the Rectifier",
            "Shenk the Overseer",
            "Thresh Socket",
            "The Council",
            "Bishibosh",
        ] {
            assert!(bosses().any(|b| b == boss), "{boss}");
        }
    }

    #[test]
    fn boss_levels_match_the_game() {
        // Quest bosses: their fixed monstats level; superuniques: area level + 3.
        let level = |b: &str, d: u8| boss_drops(b, d).map(|(l, _)| l);
        assert_eq!(level("Andariel", NIGHTMARE), Some(49));
        assert_eq!(level("Mephisto", NIGHTMARE), Some(59));
        assert_eq!(level("Shenk the Overseer", NIGHTMARE), Some(61));
        assert_eq!(level("Eldritch the Rectifier", NIGHTMARE), Some(62));
        assert_eq!(level("The Countess", NIGHTMARE), Some(45));
        assert_eq!(level("The Countess", 2), Some(82));
        assert_eq!(level("Mephisto", 3), None, "no fourth difficulty");
        for &(name, _, levels) in d2r_core::levels_data::BOSSES {
            let ours = [0, 1, 2].map(|d| level(name, d).map(u8::try_from));
            assert_eq!(ours, levels.map(|l| Some(Ok(l))), "{name}");
        }
    }

    #[test]
    fn countess_nightmare_drops_runes_up_to_ko() -> Result<()> {
        // Her rune TC (Runes 8) stops at Io; her item TC's "Act 2 (N) Good" reaches Runes 9 (Ko).
        let (_, drops) = boss_drops("The Countess", NIGHTMARE).context("countess")?;
        let runes: Vec<&str> = RUNE_NAMES
            .iter()
            .copied()
            .filter(|r| rune_code(r).is_some_and(|c| drops.contains(&c.as_str())))
            .collect();
        assert_eq!(runes.first(), Some(&"El"));
        assert_eq!(runes.last(), Some(&"Ko"));
        assert_eq!(runes.len(), 18, "every rune El..Ko");
        Ok(())
    }

    #[test]
    fn countess_nightmare_is_worth_it_for_low_runes_only() -> Result<()> {
        let farm = farm_of(
            "{ name = \"Insight\", kind = \"runeword\" },\n{ name = \"Enigma\", kind = \"runeword\" },",
        )?;
        assert_eq!(
            shown_worth("The Countess", &farm, &[]),
            ["Insight (Ral, Tir, Tal, Sol)", "Enigma (Ith)"]
        );
        let owned: Vec<(String, u32)> = ["Ral", "Tir", "Tal", "Sol"]
            .iter()
            .map(|r| (format!("{r} Rune"), 1))
            .collect();
        assert_eq!(shown_worth("The Countess", &farm, &owned), ["Enigma (Ith)"]);
        Ok(())
    }

    #[test]
    fn nightmare_mephisto_leaves_the_countess_runes_to_her() -> Result<()> {
        let low = farm_of("{ name = \"Insight\", kind = \"runeword\" },")?;
        assert_eq!(shown_worth("Mephisto", &low, &[]), Vec::<String>::new());
        assert_eq!(
            shown_worth("The Countess", &low, &[]),
            ["Insight (Ral, Tir, Tal, Sol)"]
        );
        let lem = farm_of("{ name = \"Wealth\", kind = \"runeword\" },")?;
        assert_eq!(shown_worth("Mephisto", &lem, &[]), ["Wealth (Lem)"]);
        Ok(())
    }

    #[test]
    fn countess_counts_every_rune_she_drops() -> Result<()> {
        let farm = farm_of("{ name = \"Stealth\", kind = \"runeword\" },")?;
        assert_eq!(shown_worth("The Countess", &farm, &[]), ["Stealth (Tal, Eth)"]);
        let top = |d: u8| countess_top_rune(d).and_then(|i| RUNE_NAMES.get(i).copied());
        assert_eq!([0, 1, 2].map(top), [Some("Ral"), Some("Ko"), Some("Lo")]);
        assert_eq!(countess_top_rune(3), None, "no data: every rune counts");
        Ok(())
    }

    #[test]
    fn impossible_drops_give_nothing() -> Result<()> {
        let farm = farm_of(
            "{ name = \"Arachnid Mesh\", kind = \"unique\" },\n{ name = \"Jah\", kind = \"runeword\" },",
        )?;
        assert_eq!(worth("Andariel", 0, &farm, &NO_HOLDINGS, &[]), []);
        assert_eq!(
            worth("Nobody", NIGHTMARE, &farm, &NO_HOLDINGS, &[]),
            [],
            "unknown boss"
        );
        Ok(())
    }

    #[test]
    fn qlvl_above_the_boss_level_is_out() -> Result<()> {
        // Bloodfist qlvl 9, The Cat's Eye qlvl 58.
        let farm = farm_of(
            "{ name = \"Bloodfist\", kind = \"unique\" },\n{ name = \"The Cat's Eye\", kind = \"unique\" },",
        )?;
        assert_eq!(
            shown_worth("Andariel", &farm, &[]),
            ["Bloodfist"],
            "NM Andariel is 49"
        );
        assert_eq!(
            shown_worth("Mephisto", &farm, &[]),
            ["Bloodfist", "The Cat's Eye"]
        );
        Ok(())
    }

    #[test]
    fn nightmare_andariel_and_mephisto_for_the_shipped_farm_list() -> Result<()> {
        let farm = shipped_farm()?;
        let andariel = shown_worth("Andariel", &farm, &[]);
        let mephisto = shown_worth("Mephisto", &farm, &[]);
        println!("Nightmare Andariel: {}", andariel.join("; "));
        println!("Nightmare Mephisto: {}", mephisto.join("; "));
        println!(
            "Nightmare Countess: {}",
            shown_worth("The Countess", &farm, &[]).join("; ")
        );
        assert!(andariel.iter().any(|w| w == "Bloodfist"));
        assert!(mephisto.iter().any(|w| w == "Bloodfist"));
        assert!(!andariel.iter().any(|w| w == "Arachnid Mesh"), "qlvl 87");
        assert!(!andariel.iter().any(|w| w.contains('(')), "{andariel:?}");
        assert!(mephisto.iter().any(|w| w == "Heart of the Oak (Pul)"));
        Ok(())
    }

    #[test]
    fn a_level_85_area_drops_qlvl_85_and_leaves_reliable_runes_to_the_countess() -> Result<()> {
        // Highlord's Wrath qlvl 73, Arachnid Mesh qlvl 87; Hell Countess: Um reliable, Vex not.
        let farm = farm_of(
            "{ name = \"Highlord's Wrath\", kind = \"unique\" },\n\
             { name = \"Arachnid Mesh\", kind = \"unique\" },\n\
             { name = \"Heart of the Oak\", kind = \"runeword\" },\n\
             { name = \"Duress\", kind = \"runeword\" },",
        )?;
        let owned: Vec<(String, u32)> = ["Shael", "Thul"]
            .iter()
            .map(|r| (format!("{r} Rune"), 1))
            .collect();
        let shown = |d: Option<u8>| -> Vec<String> {
            area_85(d, &farm, &NO_HOLDINGS, &owned)
                .into_iter()
                .map(|(name, why)| match why {
                    Why::Drops => name,
                    Why::Runes(r) => format!("{name} ({})", r.join(", ")),
                })
                .collect()
        };
        assert_eq!(shown(Some(HELL)), ["Highlord's Wrath", "Heart of the Oak (Vex)"]);
        assert_eq!(
            shown(None),
            [
                "Highlord's Wrath",
                "Heart of the Oak (Ko, Vex, Pul)",
                "Duress (Um)"
            ]
        );
        Ok(())
    }

    #[test]
    fn countess_rune_tc_tops_out_at_ral_io_ist() {
        let top = |d: u8| {
            RUNE_NAMES
                .iter()
                .copied()
                .rfind(|r| countess_rune_chance(r, d).is_some())
        };
        assert_eq!([0, 1, 2].map(top), [Some("Ral"), Some("Io"), Some("Ist")]);
        assert_eq!(countess_rune_chance("El", 3), None, "no fourth difficulty");
        for d in 0..3 {
            for r in RUNE_NAMES.iter().filter(|r| countess_rune_chance(r, d).is_some()) {
                let (_, drops) = boss_drops(COUNTESS, d).unwrap_or_default();
                assert!(
                    rune_code(r).is_some_and(|c| drops.contains(&c.as_str())),
                    "{r} in {d}"
                );
            }
        }
    }

    #[test]
    fn hell_countess_ko_is_about_one_in_43() {
        let ko = CountessRune {
            rune: "Ko",
            chance: countess_rune_chance("Ko", HELL),
            entries: Vec::new(),
        };
        assert_eq!(ko.one_in().map(|n| n.to_string()), Some("43".to_owned()));
        let ist = countess_rune_chance("Ist", HELL).unwrap_or_default();
        assert!((0.4..0.43).contains(&ist), "Ist {ist}%");
    }

    #[test]
    fn countess_next_shows_counts_and_no_runes_missing() {
        assert_eq!(countess_next(HELL, &[]), "no runes missing");
        assert_eq!(
            countess_next(HELL, &[("Ko", 3), ("Vex", 1), ("Lo", 2)]),
            "Ko ×3 (1 in 43) · rare: Vex, Lo ×2"
        );
        assert_eq!(countess_next(NIGHTMARE, &[("Ko", 1)]), "rare: Ko");
    }

    #[test]
    fn hell_countess_splits_reliable_and_rare_runes() -> Result<()> {
        let farm = farm_of(
            "{ name = \"Heart of the Oak\", kind = \"runeword\" },\n{ name = \"Lawbringer\", kind = \"runeword\" },",
        )?;
        let owned: Vec<(String, u32)> = ["Pul", "Thul", "Amn", "Lem"]
            .iter()
            .map(|r| (format!("{r} Rune"), 1))
            .collect();
        let items = worth(COUNTESS, HELL, &farm, &NO_HOLDINGS, &owned);
        let runes = countess_runes(HELL, &items);
        let shown: Vec<(&str, bool, Vec<String>)> = runes
            .iter()
            .map(|r| (r.rune, r.chance.is_some(), r.entries.clone()))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "Ko",
                    true,
                    vec!["Heart of the Oak".to_owned(), "Lawbringer".to_owned()]
                ),
                ("Vex", false, vec!["Heart of the Oak".to_owned()]),
            ]
        );
        assert_eq!(
            countess_parts(HELL, &items),
            [
                "reliable: Ko (1 in 43: Heart of the Oak, Lawbringer)",
                "rare: Vex (Heart of the Oak)"
            ]
        );
        Ok(())
    }
}
