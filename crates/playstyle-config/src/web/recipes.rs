//! The recipes panel: the current stage's runewords, crafts, cube and socket recipes with their
//! inputs counted against the runes and gems the character holds.

use super::PanelInfo;
use crate::config::Recipe;
use d2r_core::quests::Reward;

/// Gem grade prefixes; the normal grade has none.
pub const GEM_GRADES: [&str; 4] = ["chipped", "flawed", "flawless", "perfect"];

pub const GEM_COLOURS: [&str; 7] = [
    "amethyst", "topaz", "sapphire", "emerald", "ruby", "diamond", "skull",
];

/// One recipe input line: a counted rune or gem, any gem of one grade, or a base item the
/// reader can't check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Material { name: String, need: u32 },
    AnyGem { grade: String, need: u32 },
    Base,
}

impl Input {
    /// Reads `"Tal"`, `"3 Tal Rune"`, `"1-2 Ral Rune"` (the minimum), `"Ort Rune x2"`,
    /// `"1 Chipped Topaz"`, `"1 Chipped Gem (any)"`; anything else is a base.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        let mut need = 1;
        let mut rest = text;
        if let Some((count, after)) = text.split_once(' ') {
            let low = count.split_once('-').map_or(count, |(low, _)| low);
            if let Ok(n) = low.parse::<u32>() {
                need = n;
                rest = after.trim();
            }
        }
        if let Some((before, times)) = rest.rsplit_once(" x")
            && let Ok(n) = times.trim().parse::<u32>()
        {
            need = need.saturating_mul(n);
            rest = before.trim();
        }
        let lower = rest.to_lowercase();
        let bare = lower
            .strip_suffix(" runes")
            .or_else(|| lower.strip_suffix(" rune"))
            .unwrap_or(&lower);
        if let Some(rune) = d2r_core::stash::RUNE_NAMES
            .iter()
            .find(|r| r.eq_ignore_ascii_case(bare))
        {
            return Self::Material {
                name: format!("{rune} Rune"),
                need,
            };
        }
        let words: Vec<&str> = lower.split_whitespace().collect();
        match words.as_slice() {
            [grade, colour] if GEM_GRADES.contains(grade) && GEM_COLOURS.contains(colour) => Self::Material {
                name: title_case(&lower),
                need,
            },
            [colour] if GEM_COLOURS.contains(colour) => Self::Material {
                name: title_case(&lower),
                need,
            },
            [grade, "gem" | "gems", ..] if GEM_GRADES.contains(grade) => Self::AnyGem {
                grade: (*grade).to_owned(),
                need,
            },
            _ => Self::Base,
        }
    }

    /// How many of this input the merged material counts hold; `None` for a base item.
    #[must_use]
    pub fn have(&self, materials: &[(String, u32)]) -> Option<u32> {
        match self {
            Self::Material { name, .. } => Some(
                materials
                    .iter()
                    .filter(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, q)| q)
                    .sum(),
            ),
            Self::AnyGem { grade, .. } => Some(
                materials
                    .iter()
                    .filter(|(n, _)| {
                        let n = n.to_lowercase();
                        n.split_once(' ')
                            .is_some_and(|(g, colour)| g == grade && GEM_COLOURS.contains(&colour))
                    })
                    .map(|(_, q)| q)
                    .sum(),
            ),
            Self::Base => None,
        }
    }

    #[must_use]
    pub fn need(&self) -> Option<(String, u32)> {
        match self {
            Self::Material { name, need } => Some((name.to_lowercase(), *need)),
            Self::AnyGem { grade, need } => Some((format!("{grade} gem"), *need)),
            Self::Base => None,
        }
    }
}

#[must_use]
pub fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            chars
                .next()
                .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The runes and gems of the materials tab plus those held in inventory, cube and stash
/// pages, merged by name.
#[must_use]
pub fn owned_materials(info: &PanelInfo) -> Vec<(String, u32)> {
    materials_of(info.live.held_materials, info.live.stash)
}

#[must_use]
pub fn materials_of(held: &[(String, u32)], stash: &[d2r_core::stash::StashTab]) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = held.to_vec();
    let stacks = stash
        .iter()
        .filter(|t| t.kind == d2r_core::stash::TabKind::Materials)
        .flat_map(|t| &t.items);
    for item in stacks {
        let qty = item.quantity.unwrap_or(1);
        match out.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&item.name)) {
            Some((_, count)) => *count = count.saturating_add(qty),
            None => out.push((item.name.clone(), qty)),
        }
    }
    out
}

/// The socket count of `"with exactly 2 sockets"`, `"2-socket helm"`.
#[must_use]
pub fn socket_count(text: &str) -> Option<u8> {
    let lower = text.to_lowercase();
    lower.match_indices("socket").find_map(|(at, _)| {
        let before = lower.get(..at)?.trim_end_matches([' ', '-']);
        before
            .chars()
            .last()?
            .to_digit(10)
            .and_then(|d| u8::try_from(d).ok())
    })
}

/// The held white bases (`"Hard Leather Armor 2os"`) a base input line names, with its
/// socket count when it asks for one.
pub fn matching_bases<'a>(text: &str, bases: &'a [String]) -> Vec<&'a str> {
    let lower = text.to_lowercase();
    let sockets = socket_count(text);
    bases
        .iter()
        .filter(|b| {
            let Some((name, os)) = b.rsplit_once(' ') else {
                return false;
            };
            let name = name.strip_prefix("eth ").unwrap_or(name).to_lowercase();
            let has = os.strip_suffix("os").and_then(|n| n.parse::<u8>().ok());
            lower.contains(&name) && sockets.is_none_or(|want| has == Some(want))
        })
        .map(String::as_str)
        .collect()
}

/// How many copies of runeword `name` the character holds, its shared stash included.
#[must_use]
pub fn runeword_copies(info: &PanelInfo, name: &str) -> usize {
    let held = info
        .live
        .runeword_copies
        .iter()
        .filter(|n| d2r_core::names::same_name(n, name))
        .count();
    let stashed = info
        .live
        .stash
        .iter()
        .flat_map(|tab| &tab.items)
        .filter(|item| item.runeword && d2r_core::names::same_name(&item.name, name))
        .count();
    held + stashed
}

/// How many copies of runeword `name` `build` wants: one per player or merc slot listing it,
/// at least one.
#[must_use]
pub fn runeword_slots(build: &crate::config::Build, name: &str) -> usize {
    build
        .slots
        .values()
        .chain(build.merc_slots.values())
        .filter(|pieces| {
            pieces
                .iter()
                .any(|p| d2r_core::names::same_name(d2r_core::info::piece_name(p), name))
        })
        .count()
        .max(1)
}

/// The current stage's open recipes. Gone: a used quest reward, a runeword held for every slot
/// wanting it, and an `upgrade` recipe whose result no other recipe misses.
#[must_use]
pub fn recipes_json(info: &PanelInfo) -> Vec<serde_json::Value> {
    let Some(build) = info.build else {
        return Vec::new();
    };
    let already_made =
        |r: &Recipe| r.kind == "runeword" && runeword_copies(info, &r.name) >= runeword_slots(build, &r.name);
    let recipes: Vec<&Recipe> = build
        .recipes
        .iter()
        .filter(|r| quest_reward(r, info) != Some(Reward::Used) && !already_made(r))
        .collect();
    let materials = owned_materials(info);
    let missing = missing_materials(&recipes, &materials);
    recipes
        .into_iter()
        .filter(|recipe| {
            let makes = Input::parse(&recipe.result).need();
            recipe.kind != "upgrade" || makes.is_none_or(|(key, _)| missing.contains(&key))
        })
        .map(|recipe| recipe_json(recipe, info, &materials))
        .collect()
}

/// The quest reward state of `recipe`'s quest; `None` without one or with the flags unread.
fn quest_reward(recipe: &Recipe, info: &PanelInfo) -> Option<Reward> {
    d2r_core::quests::word_of(&recipe.quest).and_then(|w| d2r_core::quests::reward(info.live.quests, w))
}

/// The material keys the non-`upgrade` recipes still miss.
fn missing_materials(recipes: &[&Recipe], materials: &[(String, u32)]) -> Vec<String> {
    recipes
        .iter()
        .filter(|r| r.kind != "upgrade")
        .flat_map(|r| {
            let inputs: Vec<Input> = r.inputs.iter().map(|t| Input::parse(t)).collect();
            need_totals(&inputs)
                .into_iter()
                .filter(|(key, need)| {
                    inputs
                        .iter()
                        .find(|i| i.need().is_some_and(|(k, _)| k == *key))
                        .and_then(|i| i.have(materials))
                        .is_some_and(|have| have < *need)
                })
                .map(|(key, _)| key)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn recipe_json(recipe: &Recipe, info: &PanelInfo, materials: &[(String, u32)]) -> serde_json::Value {
    let inputs: Vec<Input> = recipe.inputs.iter().map(|t| Input::parse(t)).collect();
    let totals = need_totals(&inputs);
    let lines: Vec<serde_json::Value> = recipe
        .inputs
        .iter()
        .zip(&inputs)
        .map(|(text, input)| input_json(text, input, &totals, info, materials))
        .collect();
    serde_json::json!({
        "name": recipe.name,
        "kind": recipe.kind,
        "material": Input::parse(&recipe.result).need().is_some(),
        "result": recipe.result,
        "req": recipe.req,
        "notes": recipe.notes,
        "quest": match quest_reward(recipe, info) {
            Some(Reward::Ready) => Some("ready"),
            Some(Reward::NotEarned) => Some("todo"),
            Some(Reward::Used) | None => None,
        },
        "inputs": lines,
    })
}

/// One input line: `status` is `"have"` / `"missing"` for a counted material (`need` summed
/// over the recipe's lines), `"likely"` for a base the character seems to hold, else `"check"`.
fn input_json(
    text: &str,
    input: &Input,
    totals: &[(String, u32)],
    info: &PanelInfo,
    materials: &[(String, u32)],
) -> serde_json::Value {
    let have = input.have(materials);
    let need = input
        .need()
        .and_then(|(key, _)| totals.iter().find(|(k, _)| *k == key))
        .map(|(_, n)| *n);
    let bases = if have.is_none() {
        matching_bases(text, info.live.white_bases)
    } else {
        Vec::new()
    };
    let status = match (have, need) {
        (Some(have), Some(need)) if have >= need => "have",
        (Some(_), _) => "missing",
        (None, _) if !bases.is_empty() => "likely",
        (None, _) => "check",
    };
    serde_json::json!({
        "text": text,
        "status": status,
        "have": have,
        "need": need,
        "hint": (!bases.is_empty()).then(|| bases.join(", ")),
    })
}

/// The material keys a recipe's inputs need, summed over its lines.
pub fn need_totals(inputs: &[Input]) -> Vec<(String, u32)> {
    let mut totals: Vec<(String, u32)> = Vec::new();
    for (key, n) in inputs.iter().filter_map(Input::need) {
        match totals.iter_mut().find(|(k, _)| *k == key) {
            Some((_, total)) => *total = total.saturating_add(n),
            None => totals.push((key, n)),
        }
    }
    totals
}

/// How many of `rune` (`"Sol"`) the merged material counts hold.
#[must_use]
pub fn rune_count(rune: &str, materials: &[(String, u32)]) -> u32 {
    Input::Material {
        name: format!("{rune} Rune"),
        need: 0,
    }
    .have(materials)
    .unwrap_or(0)
}
