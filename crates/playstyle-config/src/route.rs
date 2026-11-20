//! The leveling and farming route (PLAN.md, plugin refactor step 9): `route.toml` in the
//! playstyle folder (format: the shipped file's header) and the engine picking the current
//! step for the dashboard's "Next" panel ([`current`]).
//!
//! The engine is pure: it gets the character level and difficulty, the quests done per
//! difficulty ([`d2r_core::quest_log`]), the bosses worth farming ([`worth_for`], the route's one
//! call of [`crate::worth::worth`]) and the max areas ([`max_areas`]).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::FarmGroup;
use d2r_core::quest_log::QuestsDone;
use d2r_core::stash::Holdings;

use crate::worth::{self, Why};
use d2r_core::build::{Damage, Rating};

/// The route file: switches, quest sets and the steps in order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Route {
    /// `[options]`: named switches the steps' conditions test.
    pub options: BTreeMap<String, bool>,
    /// `[quest_sets]`: set name → quest names.
    pub quest_sets: BTreeMap<String, Vec<String>>,
    /// `[[step]]`, in order.
    pub step: Vec<Step>,
}

/// One step of the route.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub title: String,
    /// When the step applies.
    pub when: Cond,
    /// When it is done (`None`: never; the next step takes over by its own `when` only).
    pub done_when: Option<Cond>,
    /// Areas to run.
    pub farm: Vec<String>,
    /// Quests to do (the missing ones are listed).
    pub quests: Option<Quests>,
    /// Rush to this target.
    pub rush: Option<String>,
    /// Bosses to kill.
    pub boss: Vec<Boss>,
    /// The level-85 Hell areas for the build ([`max_areas`]).
    pub max_areas: bool,
    pub note: String,
}

/// A quest set's name or a list of quest names.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Quests {
    Set(String),
    List(Vec<String>),
}

/// A step condition: every field given must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cond {
    pub difficulty: Option<Difficulty>,
    pub level_min: Option<u8>,
    pub level_below: Option<u8>,
    pub quests_done: Option<Quests>,
    /// Option names that must be on; `"!name"` must be off.
    pub options: Vec<String>,
    /// None of the step's `worth` bosses is worth farming.
    pub not_worth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Normal,
    Nightmare,
    Hell,
}

impl Difficulty {
    const fn index(self) -> u8 {
        self as u8
    }
}

/// A boss of a step.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boss {
    pub name: String,
    /// Shown only while worth farming (and counted by `not_worth`).
    #[serde(default = "yes")]
    pub worth: bool,
    /// From this character level on, [`Boss::late_note`] is added.
    #[serde(default)]
    pub late_from: Option<u8>,
    #[serde(default)]
    pub late_note: String,
}

const fn yes() -> bool {
    true
}

/// Parses `route.toml`; the warnings name unknown quests, quest sets, bosses and options, and
/// duplicate step ids (the steps are kept).
///
/// # Errors
/// Invalid TOML or a field of the wrong shape.
pub fn parse_route(text: &str) -> Result<(Route, Vec<String>), toml::de::Error> {
    let route: Route = toml::from_str(text)?;
    let mut warnings = Vec::new();
    for (set, quests) in &route.quest_sets {
        for quest in quests {
            if d2r_core::quests::word_of(quest).is_none() {
                warnings.push(format!("quest set {set}: unknown quest {quest:?}"));
            }
        }
    }
    let mut ids: Vec<&str> = Vec::new();
    for step in &route.step {
        let at = format!("step {:?}", step.id);
        if ids.contains(&step.id.as_str()) {
            warnings.push(format!("{at}: duplicate id"));
        }
        ids.push(&step.id);
        let conds = std::iter::once(&step.when).chain(&step.done_when);
        let quest_refs = conds
            .clone()
            .filter_map(|c| c.quests_done.as_ref())
            .chain(&step.quests);
        for quests in quest_refs {
            match quests {
                Quests::Set(set) if !route.quest_sets.contains_key(set) => {
                    warnings.push(format!("{at}: unknown quest set {set:?}"));
                }
                Quests::Set(_) => {}
                Quests::List(list) => warnings.extend(
                    list.iter()
                        .filter(|q| d2r_core::quests::word_of(q).is_none())
                        .map(|q| format!("{at}: unknown quest {q:?}")),
                ),
            }
        }
        for option in conds.flat_map(|c| &c.options) {
            if !route.options.contains_key(option.trim_start_matches('!')) {
                warnings.push(format!("{at}: unknown option {option:?} (read as off)"));
            }
        }
        for boss in &step.boss {
            if worth::boss_drops(&boss.name, 0).is_none() {
                warnings.push(format!(
                    "{at}: no drop data for boss {:?} (never worth farming)",
                    boss.name
                ));
            }
        }
    }
    Ok((route, warnings))
}

/// What the engine reads besides the route.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub level: u8,
    /// 0 Normal, 1 Nightmare, 2 Hell.
    pub difficulty: u8,
    pub quests: &'a QuestsDone,
    /// The bosses worth farming in this difficulty, each with the items it is worth it for
    /// (as shown: `"Heart of the Oak (Ko, Pul)"`; the Countess's are the parts of
    /// [`worth::countess_parts`]); bosses not worth it are absent.
    pub worth: &'a [(String, Vec<String>)],
    /// The max areas' names ([`max_areas`]).
    pub max_areas: &'a [&'static str],
}

/// The dashboard's "Next" panel: the current step and the one after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Advice {
    pub id: String,
    pub title: String,
    /// What to do, one line each (quests missing, areas, bosses and what for).
    pub todo: Vec<String>,
    /// Why / hints.
    pub note: String,
    /// The following step's title.
    pub then: Option<String>,
}

impl Advice {
    /// The live JSON's `next` section: `{ "id", "title", "todo": [..], "note", "then" }`.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "title": self.title,
            "todo": self.todo,
            "note": self.note,
            "then": self.then,
        })
    }
}

/// The current step: the first whose `when` holds and whose `done_when` doesn't. With none
/// left in this difficulty, a "move on" advice pointing to the next difficulty's first step;
/// `None` for a route without steps.
#[must_use]
pub fn current(route: &Route, facts: &Facts) -> Option<Advice> {
    let at = route.step.iter().position(|step| {
        holds(route, step, &step.when, facts)
            && !step
                .done_when
                .as_ref()
                .is_some_and(|c| holds(route, step, c, facts))
    });
    let Some(at) = at else {
        let next = route
            .step
            .iter()
            .find(|s| s.when.difficulty.is_some_and(|d| d.index() > facts.difficulty))?;
        let name = next
            .when
            .difficulty
            .and_then(|d| d2r_core::info::difficulty_name(d.index()))
            .unwrap_or("the next difficulty");
        let here = d2r_core::info::difficulty_name(facts.difficulty).unwrap_or("This difficulty");
        return Some(Advice {
            id: "move-on".to_owned(),
            title: format!("{here} done: on to {name}"),
            todo: Vec::new(),
            note: String::new(),
            then: Some(next.title.clone()),
        });
    };
    let step = route.step.get(at)?;
    let then = route
        .step
        .iter()
        .skip(at.saturating_add(1))
        .find(|s| may_come(route, s, facts))
        .map(|s| s.title.clone());
    Some(Advice {
        id: step.id.clone(),
        title: step.title.clone(),
        todo: todo(route, step, facts),
        note: step.note.clone(),
        then,
    })
}

/// Whether `cond` of `step` holds.
fn holds(route: &Route, step: &Step, cond: &Cond, facts: &Facts) -> bool {
    let difficulty = cond.difficulty.map_or(facts.difficulty, Difficulty::index);
    cond.difficulty.is_none_or(|d| d.index() == facts.difficulty)
        && cond.level_min.is_none_or(|min| facts.level >= min)
        && cond.level_below.is_none_or(|below| facts.level < below)
        && cond
            .quests_done
            .as_ref()
            .is_none_or(|q| missing(route, q, facts.quests, difficulty).is_empty())
        && options_hold(route, cond)
        && (!cond.not_worth
            || !step
                .boss
                .iter()
                .any(|b| b.worth && worth_items(b, facts).is_some()))
}

/// Whether `cond`'s options are as it wants them (unknown ones are off).
fn options_hold(route: &Route, cond: &Cond) -> bool {
    let on = |name: &str| route.options.get(name).copied().unwrap_or(false);
    cond.options
        .iter()
        .all(|o| o.strip_prefix('!').map_or_else(|| on(o), |off| !on(off)))
}

/// Whether a later step can still come after the current one: the same or a later
/// difficulty, not outleveled, its options on, and, in this difficulty, not done (a later
/// difficulty's step can't be judged from this one's quests and worth).
fn may_come(route: &Route, step: &Step, facts: &Facts) -> bool {
    let later = step.when.difficulty.is_some_and(|d| d.index() > facts.difficulty);
    step.when.difficulty.is_none_or(|d| d.index() >= facts.difficulty)
        && step.when.level_below.is_none_or(|below| facts.level < below)
        && options_hold(route, &step.when)
        && (later
            || !step
                .done_when
                .as_ref()
                .is_some_and(|c| holds(route, step, c, facts)))
}

/// The quests of `quests` not done in `difficulty`, in their listed order.
fn missing<'r>(route: &'r Route, quests: &'r Quests, done: &QuestsDone, difficulty: u8) -> Vec<&'r str> {
    let list: &[String] = match quests {
        Quests::Set(set) => route.quest_sets.get(set).map_or(&[], Vec::as_slice),
        Quests::List(list) => list,
    };
    list.iter()
        .map(String::as_str)
        .filter(|q| !done.is_done(difficulty, q))
        .collect()
}

/// The items boss `boss` is worth farming for, if it is.
fn worth_items<'f>(boss: &Boss, facts: &Facts<'f>) -> Option<&'f [String]> {
    facts
        .worth
        .iter()
        .find(|(name, items)| d2r_core::names::same_name(name, &boss.name) && !items.is_empty())
        .map(|(_, items)| items.as_slice())
}

/// The step's what-to-do lines.
fn todo(route: &Route, step: &Step, facts: &Facts) -> Vec<String> {
    let mut out = Vec::new();
    let left = step
        .quests
        .as_ref()
        .map_or_else(Vec::new, |q| missing(route, q, facts.quests, facts.difficulty));
    if let Some(target) = &step.rush {
        out.push(if left.is_empty() {
            format!("Rush to {target}")
        } else {
            format!("Rush to {target}: {}", left.join(", "))
        });
    } else if !left.is_empty() {
        out.push(format!("Quests left: {}", left.join(", ")));
    }
    if !step.farm.is_empty() {
        out.push(format!("Farm: {}", step.farm.join(" · ")));
    }
    for boss in &step.boss {
        let level = worth::boss_drops(&boss.name, facts.difficulty).map(|(l, _)| l);
        let mut line = level.map_or_else(|| boss.name.clone(), |l| format!("{} ({l})", boss.name));
        if boss.late_from.is_some_and(|from| facts.level >= from) && !boss.late_note.is_empty() {
            line = format!("{line}, {}", boss.late_note);
        }
        if boss.worth {
            let Some(items) = worth_items(boss, facts) else {
                continue;
            };
            line = if worth::is_countess(&boss.name) {
                // Parts "reliable: …", "rare: …", "items: …" ([`worth::countess_parts`]).
                format!("{line} — {}", items.join("; "))
            } else {
                format!("{line} for: {}", items.join(", "))
            };
        }
        out.push(line);
    }
    if step.max_areas {
        out.push(if facts.max_areas.is_empty() {
            "Max areas: none without immunes to the build's damage".to_owned()
        } else {
            format!("Max areas: {}", facts.max_areas.join(" · "))
        });
    }
    out
}

/// The worth-farming items of every `worth` boss the route names, in `difficulty`, as
/// [`Facts::worth`] wants them: the route's single call of [`worth::worth`], so a change of the
/// rule lands here and in the farm panel alike.
#[must_use]
pub fn worth_for(
    route: &Route,
    difficulty: u8,
    farm: &[FarmGroup],
    holdings: &Holdings,
    materials: &[(String, u32)],
) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for boss in route.step.iter().flat_map(|s| &s.boss).filter(|b| b.worth) {
        if out
            .iter()
            .any(|(name, _)| d2r_core::names::same_name(name, &boss.name))
        {
            continue;
        }
        let worth = worth::worth(&boss.name, difficulty, farm, holdings, materials);
        // The Countess: her runes split into reliable and rare ([`worth::countess_parts`]).
        let items = if worth::is_countess(&boss.name) {
            worth::countess_parts(difficulty, &worth)
        } else {
            worth
                .into_iter()
                .map(|(name, why)| match why {
                    Why::Drops => name,
                    Why::Runes(runes) => format!("{name} ({})", runes.join(", ")),
                })
                .collect()
        };
        out.push((boss.name.clone(), items));
    }
    out
}

/// The max areas for a build of `damages` (primary first; none: physical): the Hell areas of
/// monster level 85 where no monster type is immune to the primary damage
/// ([`d2r_core::build::rate`] = good), in area order.
#[must_use]
pub fn max_areas(damages: &[Damage]) -> Vec<&'static str> {
    let damages = if damages.is_empty() {
        &[Damage::Physical][..]
    } else {
        damages
    };
    d2r_core::levels_data::LEVELS
        .iter()
        .filter(|&&(id, _, [_, _, hell])| {
            hell == 85 && d2r_core::build::rate(id, Some(2), damages) == Rating::Good
        })
        .map(|&(_, name, _)| name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};

    const NORMAL: u8 = 0;
    const NIGHTMARE: u8 = 1;
    const HELL: u8 = 2;

    fn shipped() -> Result<Route> {
        let (route, warnings) = parse_route(include_str!("../../../playstyles/summoner-fury/route.toml"))?;
        assert_eq!(warnings, Vec::<String>::new());
        Ok(route)
    }

    /// Quests done: every quest of these sets / names, per difficulty.
    fn done(route: &Route, per_difficulty: [&[&str]; 3]) -> QuestsDone {
        let mut out = QuestsDone::default();
        for (d, names) in per_difficulty.iter().enumerate() {
            let set = match d {
                0 => &mut out.normal,
                1 => &mut out.nightmare,
                _ => &mut out.hell,
            };
            for name in *names {
                match route.quest_sets.get(*name) {
                    Some(quests) => set.extend(quests.iter().cloned()),
                    None => {
                        set.insert((*name).to_owned());
                    }
                }
            }
        }
        out
    }

    fn advise(
        route: &Route,
        level: u8,
        difficulty: u8,
        quests: &QuestsDone,
        worth: &[(String, Vec<String>)],
    ) -> Result<Advice> {
        let areas = max_areas(&[Damage::Physical]);
        current(
            route,
            &Facts {
                level,
                difficulty,
                quests,
                worth,
                max_areas: &areas,
            },
        )
        .context("no advice")
    }

    fn worth_of(entries: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        entries
            .iter()
            .map(|(boss, items)| ((*boss).to_owned(), items.iter().map(|&i| i.to_owned()).collect()))
            .collect()
    }

    #[test]
    fn the_shipped_route_parses_without_warnings() -> Result<()> {
        let route = shipped()?;
        assert_eq!(route.step.len(), 17);
        let (_, warnings) = parse_route(
            "[quest_sets]\nx = [\"Nope\"]\n[[step]]\nid = \"a\"\nquests = \"y\"\nwhen = { options = [\"z\"] }\n\
             [[step.boss]]\nname = \"Nobody\"\n[[step]]\nid = \"a\"",
        )?;
        assert_eq!(
            warnings,
            [
                "quest set x: unknown quest \"Nope\"",
                "step \"a\": unknown quest set \"y\"",
                "step \"a\": unknown option \"z\" (read as off)",
                "step \"a\": no drop data for boss \"Nobody\" (never worth farming)",
                "step \"a\": duplicate id",
            ]
        );
        Ok(())
    }

    #[test]
    fn walks_the_normal_route() -> Result<()> {
        let route = shipped()?;
        let none = QuestsDone::default();
        let a = advise(&route, 10, NORMAL, &none, &[])?;
        assert_eq!(a.id, "normal-tristram");
        assert_eq!(a.todo, ["Farm: Tristram · Forgotten Tower (the Countess)"]);
        assert_eq!(a.then.as_deref(), Some("Quests to open the Tombs"));
        // 16 with two canyon quests missing: those are listed.
        let some = done(&route, [&["Sisters to the Slaughter", "Tainted Sun"], &[], &[]]);
        let a = advise(&route, 16, NORMAL, &some, &[])?;
        assert_eq!(a.id, "normal-canyon");
        assert_eq!(a.todo, ["Quests left: Arcane Sanctuary, The Summoner"]);
        assert_eq!(a.then.as_deref(), Some("Tal Rasha's Tombs until 24"));
        let canyon = done(&route, [&["canyon"], &[], &[]]);
        assert_eq!(advise(&route, 20, NORMAL, &canyon, &[])?.id, "normal-tombs");
        let a = advise(&route, 24, NORMAL, &canyon, &[])?;
        assert_eq!(a.id, "normal-ancients");
        assert_eq!(
            a.todo,
            ["Quests left: The Horadric Staff, The Seven Tombs, Khalim's Will, The Guardian, Terror's End"]
        );
        let ancients = done(&route, [&["canyon", "ancients"], &[], &[]]);
        assert_eq!(advise(&route, 30, NORMAL, &ancients, &[])?.id, "normal-baal");
        let a = advise(&route, 35, NORMAL, &ancients, &[])?;
        assert_eq!(a.id, "normal-valuable");
        assert!(a.note.contains("Larzuk"));
        assert_eq!(a.then.as_deref(), Some("Nightmare: rescue Cain"));
        // Everything done in Normal: on to Nightmare.
        let all = done(&route, [&["canyon", "ancients", "valuable"], &[], &[]]);
        let a = advise(&route, 38, NORMAL, &all, &[])?;
        assert_eq!(a.title, "Normal done: on to Nightmare");
        assert_eq!(a.then.as_deref(), Some("Nightmare: rescue Cain"));
        Ok(())
    }

    #[test]
    fn walks_the_nightmare_route() -> Result<()> {
        let route = shipped()?;
        let normal: &[&str] = &["canyon", "ancients", "valuable"];
        let a = advise(&route, 38, NIGHTMARE, &done(&route, [normal, &[], &[]]), &[])?;
        assert_eq!(a.id, "nm-cain");
        let cain = done(&route, [normal, &["The Search for Cain"], &[]]);
        let a = advise(&route, 40, NIGHTMARE, &cain, &[])?;
        assert_eq!(a.id, "nm-countess");
        assert_eq!(a.then.as_deref(), Some("Rush to Mephisto"));
        let a = advise(&route, 45, NIGHTMARE, &cain, &[])?;
        assert_eq!(a.id, "nm-rush-mephisto");
        assert_eq!(
            a.todo,
            [
                "Rush to Mephisto: Sisters to the Slaughter, The Horadric Staff, Tainted Sun, \
                 Arcane Sanctuary, The Summoner, The Seven Tombs, Khalim's Will"
            ]
        );
        // NM 50, Mephisto worth it (Andariel not): Mephisto.
        let rushed = done(&route, [normal, &["The Search for Cain", "mephisto"], &[]]);
        let meph = worth_of(&[
            ("Mephisto", &["Bloodfist", "Heart of the Oak (Pul)"]),
            ("Andariel", &[]),
        ]);
        let a = advise(&route, 50, NIGHTMARE, &rushed, &meph)?;
        assert_eq!(a.id, "nm-bosses");
        assert_eq!(a.todo, ["Mephisto (59) for: Bloodfist, Heart of the Oak (Pul)"]);
        assert_eq!(a.then.as_deref(), Some("Finish Diablo"));
        // NM 56, both worth it: Andariel gets her note.
        let both = worth_of(&[("Mephisto", &["Bloodfist"]), ("Andariel", &["Ribcracker"])]);
        let a = advise(&route, 56, NIGHTMARE, &rushed, &both)?;
        assert_eq!(
            a.todo,
            [
                "Mephisto (59) for: Bloodfist",
                "Andariel (49), items only, little XP for: Ribcracker"
            ]
        );
        assert!(
            !advise(&route, 54, NIGHTMARE, &rushed, &both)?
                .todo
                .iter()
                .any(|t| t.contains("little XP"))
        );
        // Nothing worth it, or 57: Diablo.
        let a = advise(&route, 50, NIGHTMARE, &rushed, &[])?;
        assert_eq!(a.id, "nm-diablo");
        assert_eq!(a.todo, ["Quests left: The Guardian, Terror's End"]);
        let a = advise(&route, 57, NIGHTMARE, &rushed, &both)?;
        assert_eq!(a.id, "nm-diablo");
        assert_eq!(a.then.as_deref(), Some("The Ancients"));
        let diablo = done(
            &route,
            [normal, &["The Search for Cain", "mephisto", "ancients"], &[]],
        );
        assert_eq!(advise(&route, 58, NIGHTMARE, &diablo, &both)?.id, "nm-ancients");
        let ancients = done(
            &route,
            [
                normal,
                &["The Search for Cain", "mephisto", "ancients", "Rite of Passage"],
                &[],
            ],
        );
        let a = advise(&route, 60, NIGHTMARE, &ancients, &both)?;
        assert_eq!(a.id, "nm-baal");
        assert_eq!(a.then.as_deref(), Some("On to Hell"));
        // From 65 (user, 2026-11-20): on to Hell, with Baal's quest listed while it's missing.
        let a = advise(&route, 65, NIGHTMARE, &ancients, &both)?;
        assert_eq!(a.id, "nm-to-hell");
        assert_eq!(a.todo, ["Quests left: Eve of Destruction"]);
        assert_eq!(a.then.as_deref(), Some("Hell: the Countess"));
        Ok(())
    }

    #[test]
    fn walks_the_hell_route() -> Result<()> {
        let route = shipped()?;
        let none = QuestsDone::default();
        let countess = worth_of(&[("The Countess", &["Enigma (Ber)"])]);
        assert_eq!(advise(&route, 70, HELL, &none, &countess)?.id, "hell-countess");
        assert_eq!(advise(&route, 70, HELL, &none, &[])?.id, "hell-basic");
        let a = advise(&route, 76, HELL, &none, &[])?;
        assert_eq!(a.id, "hell-max-areas");
        let areas = a.todo.first().context("areas")?;
        assert!(areas.starts_with("Max areas: "), "{areas}");
        assert!(
            areas.contains("The Chaos Sanctuary") || areas.contains("Ancient Tunnels"),
            "{areas}"
        );
        assert!(a.note.contains("Terror zones"));
        Ok(())
    }

    #[test]
    fn the_countess_step_lists_her_reliable_runes() -> Result<()> {
        let route = shipped()?;
        // Heart of the Oak is Ko Vex Pul Thul: with Pul and Thul owned, Ko (her rune TC) and
        // Vex (only her item TC) are missing.
        let (farm, _) = crate::config::parse_farm(
            "[[farm]]\nfor = \"Test\"\nitems = [{ name = \"Heart of the Oak\", kind = \"runeword\" }]\n",
        )?;
        let holdings = Holdings {
            stash: &[],
            owned: &[],
            runeword_copies: &[],
            merc_worn: &[],
        };
        let owned = [("Pul Rune".to_owned(), 1), ("Thul Rune".to_owned(), 1)];
        let worth = worth_for(&route, HELL, &farm, &holdings, &owned);
        let a = advise(&route, 70, HELL, &QuestsDone::default(), &worth)?;
        assert_eq!(a.id, "hell-countess");
        assert_eq!(
            a.todo,
            ["The Countess (82) — reliable: Ko (1 in 43: Heart of the Oak); rare: Vex (Heart of the Oak)"]
        );
        // Nightmare: Ko is rare there (her rune TC stops at Io).
        let worth = worth_for(&route, NIGHTMARE, &farm, &holdings, &owned);
        let a = advise(
            &route,
            40,
            NIGHTMARE,
            &done(
                &route,
                [&["canyon", "ancients", "valuable"], &["The Search for Cain"], &[]],
            ),
            &worth,
        )?;
        assert_eq!(a.id, "nm-countess");
        assert_eq!(
            a.todo,
            [
                "Farm: Forgotten Tower (the Countess)",
                "The Countess (45) — rare: Ko (Heart of the Oak)"
            ]
        );
        Ok(())
    }

    #[test]
    fn options_switch_steps() -> Result<()> {
        let mut route = shipped()?;
        let all: &[&str] = &["canyon", "ancients", "valuable"];
        let nm: &[&str] = &["The Search for Cain", "mephisto", "ancients", "Rite of Passage"];
        let quests = done(&route, [all, nm, &[]]);
        assert_eq!(advise(&route, 60, NIGHTMARE, &quests, &[])?.id, "nm-baal");
        route.options.insert("nm_baal_waves".to_owned(), false);
        assert_eq!(
            advise(&route, 60, NIGHTMARE, &quests, &[])?.title,
            "Nightmare done: on to Hell"
        );
        Ok(())
    }

    #[test]
    fn max_areas_skip_physical_immunes() {
        let areas = max_areas(&[]);
        assert_ne!(areas, [] as [&str; 0]);
        for name in &areas {
            let id = d2r_core::levels_data::LEVELS
                .iter()
                .find(|(_, n, _)| n == name)
                .map(|&(id, _, _)| id);
            let immune = d2r_core::levels_data::IMMUNITIES
                .iter()
                .find(|(i, _)| Some(*i) == id)
                .and_then(|(_, per)| per.get(2))
                .and_then(|(_, counts, _)| counts.first().copied());
            assert_eq!(immune, Some(0), "{name}");
        }
        println!("max areas (physical): {}", areas.join(", "));
    }
}
