//! The leveling and farming route (`route.toml`, format in the shipped file's header) and the
//! pure engine picking the current step for the dashboard's "Next" panel ([`current`]).

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
        if ids.contains(&step.id.as_str()) {
            warnings.push(format!("step {:?}: duplicate id", step.id));
        }
        ids.push(&step.id);
        step_warnings(&route, step, &mut warnings);
    }
    Ok((route, warnings))
}

/// The unknown quests, quest sets, options and bosses `step` names.
fn step_warnings(route: &Route, step: &Step, warnings: &mut Vec<String>) {
    let at = format!("step {:?}", step.id);
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

/// What the engine reads besides the route.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub level: u8,
    /// 0 Normal, 1 Nightmare, 2 Hell.
    pub difficulty: u8,
    /// `None` while the quest record wasn't read this game.
    pub quests: Option<&'a QuestsDone>,
    /// [`worth_for`].
    pub worth: &'a [(String, Vec<String>)],
    pub max_areas: &'a [&'static str],
}

/// The dashboard's "Next" panel: the current step and the one after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Advice {
    pub id: String,
    pub title: String,
    /// What to do, one line each (quests missing, areas, bosses and what for).
    pub todo: Vec<String>,
    pub note: String,
    /// The following step's title.
    pub then: Option<String>,
}

impl Advice {
    /// The live JSON's `next` section.
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

/// The current step: the first whose `when` holds and whose `done_when` doesn't; with none left
/// in this difficulty, a pointer to the next difficulty's first step.
///
/// With the quests unknown, a condition that can't be decided, or a current step listing
/// quests, gives `None`: no advice rather than a false one ("rescue Cain" long after).
#[must_use]
pub fn current(route: &Route, facts: &Facts) -> Option<Advice> {
    let mut at = None;
    for (i, step) in route.step.iter().enumerate() {
        if !eval(route, step, &step.when, facts)? {
            continue;
        }
        let done = step
            .done_when
            .as_ref()
            .map_or(Some(false), |c| eval(route, step, c, facts))?;
        if !done {
            if step.quests.is_some() && facts.quests.is_none() {
                return None;
            }
            at = Some(i);
            break;
        }
    }
    let Some(at) = at else {
        return move_on(route, facts);
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

/// The advice pointing to the next difficulty's first step.
fn move_on(route: &Route, facts: &Facts) -> Option<Advice> {
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
    Some(Advice {
        id: "move-on".to_owned(),
        title: format!("{here} done: on to {name}"),
        todo: Vec::new(),
        note: String::new(),
        then: Some(next.title.clone()),
    })
}

/// Whether `cond` of `step` holds; `None` when its `quests_done` can't be told and no known
/// part fails.
fn eval(route: &Route, step: &Step, cond: &Cond, facts: &Facts) -> Option<bool> {
    let known = cond.difficulty.is_none_or(|d| d.index() == facts.difficulty)
        && cond.level_min.is_none_or(|min| facts.level >= min)
        && cond.level_below.is_none_or(|below| facts.level < below)
        && options_hold(route, cond)
        && (!cond.not_worth
            || !step
                .boss
                .iter()
                .any(|b| b.worth && worth_items(b, facts).is_some()));
    if !known {
        return Some(false);
    }
    let Some(quests) = &cond.quests_done else {
        return Some(true);
    };
    let difficulty = cond.difficulty.map_or(facts.difficulty, Difficulty::index);
    facts
        .quests
        .map(|done| missing(route, quests, done, difficulty).is_empty())
}

/// Whether `cond`'s options are as it wants them; unknown ones are off.
fn options_hold(route: &Route, cond: &Cond) -> bool {
    let on = |name: &str| route.options.get(name).copied().unwrap_or(false);
    cond.options
        .iter()
        .all(|o| o.strip_prefix('!').map_or_else(|| on(o), |off| !on(off)))
}

/// Whether a later step can still come after the current one: not outleveled, its options on,
/// and in this difficulty not known to be done (a later difficulty's can't be judged yet).
fn may_come(route: &Route, step: &Step, facts: &Facts) -> bool {
    let later = step.when.difficulty.is_some_and(|d| d.index() > facts.difficulty);
    step.when.difficulty.is_none_or(|d| d.index() >= facts.difficulty)
        && step.when.level_below.is_none_or(|below| facts.level < below)
        && options_hold(route, &step.when)
        && (later
            || step
                .done_when
                .as_ref()
                .is_none_or(|c| eval(route, step, c, facts) != Some(true)))
}

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
    let left = match (&step.quests, facts.quests) {
        (Some(q), Some(done)) => missing(route, q, done, facts.difficulty),
        _ => Vec::new(),
    };
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
    out.extend(step.boss.iter().filter_map(|boss| boss_line(boss, facts)));
    if step.max_areas {
        out.push(if facts.max_areas.is_empty() {
            "Max areas: none without immunes to the build's damage".to_owned()
        } else {
            format!("Max areas: {}", facts.max_areas.join(" · "))
        });
    }
    out
}

/// A boss's what-to-do line: "Mephisto (59) for: Bloodfist"; the Countess's one line of
/// missing runes. `None` for a `worth` boss not worth farming.
fn boss_line(boss: &Boss, facts: &Facts) -> Option<String> {
    if boss.worth && worth::is_countess(&boss.name) {
        let items = worth_items(boss, facts)?;
        return Some(format!("{}: {}", boss.name, items.join(" · ")));
    }
    let level = worth::boss_drops(&boss.name, facts.difficulty).map(|(l, _)| l);
    let mut line = level.map_or_else(|| boss.name.clone(), |l| format!("{} ({l})", boss.name));
    if boss.late_from.is_some_and(|from| facts.level >= from) && !boss.late_note.is_empty() {
        line = format!("{line}, {}", boss.late_note);
    }
    if boss.worth {
        let items = worth_items(boss, facts)?;
        line = format!("{line} for: {}", items.join(", "));
    }
    Some(line)
}

/// The route's bosses, each once, in route order with the Countess first.
#[must_use]
pub fn bosses(route: &Route) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for boss in route.step.iter().flat_map(|s| &s.boss) {
        if !out.iter().any(|b| d2r_core::names::same_name(b, &boss.name)) {
            out.push(&boss.name);
        }
    }
    out.sort_by_key(|b| !worth::is_countess(b));
    out
}

/// Every `worth` boss of the route with the items it is worth farming for in `difficulty`, as
/// shown ("Heart of the Oak (Ko, Pul)"; the Countess: her missing runes).
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
        let items = if worth::is_countess(&boss.name) {
            if worth.is_empty() {
                Vec::new()
            } else {
                let missing = worth::countess_missing(difficulty, farm, holdings, materials);
                vec![worth::countess_next(difficulty, &missing)]
            }
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

/// The Hell areas of monster level 85 that rate good for a build of `damages` (none:
/// physical), in area order.
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

    /// Every quest of these sets / names done, per difficulty.
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
        advise_maybe(route, level, difficulty, Some(quests), worth).context("no advice")
    }

    fn advise_maybe(
        route: &Route,
        level: u8,
        difficulty: u8,
        quests: Option<&QuestsDone>,
        worth: &[(String, Vec<String>)],
    ) -> Option<Advice> {
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
        let rushed = done(&route, [normal, &["The Search for Cain", "mephisto"], &[]]);
        let meph = worth_of(&[
            ("Mephisto", &["Bloodfist", "Heart of the Oak (Pul)"]),
            ("Andariel", &[]),
        ]);
        let a = advise(&route, 50, NIGHTMARE, &rushed, &meph)?;
        assert_eq!(a.id, "nm-bosses");
        assert_eq!(a.todo, ["Mephisto (59) for: Bloodfist, Heart of the Oak (Pul)"]);
        assert_eq!(a.then.as_deref(), Some("Finish Diablo"));
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
        assert_eq!(a.todo, ["The Countess: Ko (1 in 43) · rare: Vex"]);
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
            ["Farm: Forgotten Tower (the Countess)", "The Countess: rare: Ko"]
        );
        Ok(())
    }

    #[test]
    fn the_countess_step_counts_her_missing_runes_over_the_farm_list() -> Result<()> {
        let route = shipped()?;
        let (farm, _) = crate::config::parse_farm(
            "[[farm]]\nfor = \"Test\"\nitems = [\n\
             { name = \"Heart of the Oak\", kind = \"runeword\" },\n\
             { name = \"Lawbringer\", kind = \"runeword\" },\n\
             { name = \"Harmony\", kind = \"runeword\" },\n\
             { name = \"Treachery\", kind = \"runeword\" },\n\
             { name = \"Duress\", kind = \"runeword\" },\n\
             { name = \"Fortitude\", kind = \"runeword\" },\n]\n",
        )?;
        let holdings = Holdings {
            stash: &[],
            owned: &[],
            runeword_copies: &[],
            merc_worn: &[],
        };
        let owned: Vec<(String, u32)> = [
            ("Thul", 3),
            ("Amn", 1),
            ("Tir", 1),
            ("Ith", 1),
            ("Sol", 2),
            ("Shael", 2),
            ("El", 1),
            ("Dol", 1),
        ]
        .iter()
        .map(|&(r, n)| (format!("{r} Rune"), n))
        .collect();
        let worth = worth_for(&route, HELL, &farm, &holdings, &owned);
        let a = advise(&route, 70, HELL, &QuestsDone::default(), &worth)?;
        assert_eq!(
            a.todo,
            [
                "The Countess: Ko ×3 (1 in 43), Lem ×2 (1 in 82), Pul (1 in 108), Um (1 in 162) · rare: Vex, Lo"
            ]
        );
        let mut more = owned;
        more.push(("Ko Rune".to_owned(), 1));
        let worth = worth_for(&route, HELL, &farm, &holdings, &more);
        let a = advise(&route, 70, HELL, &QuestsDone::default(), &worth)?;
        let line = a.todo.first().context("countess")?;
        assert!(line.starts_with("The Countess: Ko ×2 (1 in 43), "), "{line}");
        Ok(())
    }

    #[test]
    fn unknown_quests_show_only_the_steps_not_needing_them() -> Result<()> {
        let route = shipped()?;
        let countess = worth_of(&[("The Countess", &["Enigma (Ber)"])]);
        let a = advise_maybe(&route, 70, HELL, None, &countess).context("hell countess")?;
        assert_eq!(a.id, "hell-countess");
        assert_eq!(a.then.as_deref(), Some("Max areas"));
        let a = advise_maybe(&route, 70, HELL, None, &[]).context("hell basic")?;
        assert_eq!(a.id, "hell-basic");
        let a = advise_maybe(&route, 76, HELL, None, &[]).context("hell max")?;
        assert_eq!(a.id, "hell-max-areas");
        let a = advise_maybe(&route, 10, NORMAL, None, &[]).context("normal")?;
        assert_eq!(a.id, "normal-tristram");
        assert_eq!(a.then.as_deref(), Some("Quests to open the Tombs"));
        assert_eq!(
            advise_maybe(&route, 16, NORMAL, None, &[]),
            None,
            "done_when on quests"
        );
        assert_eq!(
            advise_maybe(&route, 60, NIGHTMARE, None, &countess),
            None,
            "done_when on quests"
        );
        Ok(())
    }

    #[test]
    fn unknown_quests_three_valued_conditions() -> Result<()> {
        let (route, _) = parse_route(
            "[[step]]\nid = \"q-fails\"\nwhen = { level_min = 50, quests_done = [\"Den of Evil\"] }\n             [[step]]\nid = \"list\"\nwhen = { level_min = 40 }\nquests = [\"Den of Evil\"]\n             [[step]]\nid = \"q-when\"\nwhen = { level_min = 30, quests_done = [\"Den of Evil\"] }\n             [[step]]\nid = \"plain\"\n",
        )?;
        assert_eq!(
            advise_maybe(&route, 10, NORMAL, None, &[])
                .map(|a| a.id)
                .as_deref(),
            Some("plain"),
            "a known part fails"
        );
        assert_eq!(
            advise_maybe(&route, 35, NORMAL, None, &[]),
            None,
            "undecidable when"
        );
        assert_eq!(
            advise_maybe(&route, 45, NORMAL, None, &[]),
            None,
            "lists unknown quests"
        );
        let a = advise(&route, 45, NORMAL, &QuestsDone::default(), &[])?;
        assert_eq!(a.id, "list");
        assert_eq!(a.todo, ["Quests left: Den of Evil"]);
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
