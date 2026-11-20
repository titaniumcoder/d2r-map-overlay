//! The recipes panel: the current stage's runewords, crafts, cube and socket recipes with their
//! inputs counted against the runes and gems the character holds.

use super::PanelInfo;

/// Gem grade prefixes (chipped … perfect; the normal grade has none) and colours, for
/// reading recipe inputs.
pub const GEM_GRADES: [&str; 4] = ["chipped", "flawed", "flawless", "perfect"];

pub const GEM_COLOURS: [&str; 7] = [
    "amethyst", "topaz", "sapphire", "emerald", "ruby", "diamond", "skull",
];

/// One recipe input line, read: a material the reader counts (rune or gem, by its
/// [`crate::stash::material_name`] name), any gem of one grade, or a base item the reader
/// can't check (free text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Material { name: String, need: u32 },
    AnyGem { grade: String, need: u32 },
    Base,
}

impl Input {
    /// Reads `"Tal"`, `"3 Tal Rune"`, `"1-2 Ral Rune"`, `"Ort Rune x2"`, `"1 Chipped Topaz"`,
    /// `"1 Chipped Gem (any)"`; everything else is a [`Input::Base`]. A count range takes its
    /// minimum.
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
        if let Some(rune) = crate::stash::RUNE_NAMES
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

    /// How many of this input the character has, from the merged material counts; `None`
    /// for a base item.
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

    /// The material key and count this input needs (`None` for a base item).
    pub fn need(&self) -> Option<(String, u32)> {
        match self {
            Self::Material { name, need } => Some((name.to_lowercase(), *need)),
            Self::AnyGem { grade, need } => Some((format!("{grade} gem"), *need)),
            Self::Base => None,
        }
    }
}

/// `"chipped topaz"` → `"Chipped Topaz"`.
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

/// The runes and gems the character has: the shared materials tab's stacks plus the ones
/// held in inventory, cube and stash pages, `(name, count)` merged by name.
pub fn owned_materials(info: &PanelInfo) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = info.live.held_materials.to_vec();
    let stacks = info
        .live
        .stash
        .iter()
        .filter(|t| t.kind == crate::stash::TabKind::Materials)
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

/// The socket count a base input names (`"with exactly 2 sockets"`, `"2-socket helm"`).
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

/// The white bases (`"Hard Leather Armor 2os"`) a base input line names by base name, with
/// the socket count it asks for when it names one: a cheap "you probably have it" hint.
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

/// How many copies of runeword `name` the character has: one per [`LiveInfo::runeword_copies`]
/// entry (worn, carried, cubed, private stash, the mercenary's worn) plus one per runeword
/// item in the shared-stash tabs. The two sources don't overlap: the shared stash's items are
/// owned by its tab units, never by the player.
pub fn runeword_copies(info: &PanelInfo, name: &str) -> usize {
    let held = info
        .live
        .runeword_copies
        .iter()
        .filter(|n| crate::names::same_name(n, name))
        .count();
    let stashed = info
        .live
        .stash
        .iter()
        .flat_map(|tab| &tab.items)
        .filter(|item| item.runeword && crate::names::same_name(&item.name, name))
        .count();
    held + stashed
}

/// How many copies of runeword `name` the stage `build` wants: the player and merc slots
/// whose piece list names it, at least one (a recipe for a runeword no slot lists).
pub fn runeword_slots(build: &crate::config::Build, name: &str) -> usize {
    build
        .slots
        .values()
        .chain(build.merc_slots.values())
        .filter(|pieces| {
            pieces
                .iter()
                .any(|p| crate::names::same_name(crate::info::piece_name(p), name))
        })
        .count()
        .max(1)
}

/// The current stage's recipes (`live_json`'s `recipes` section; an `upgrade` recipe that
/// makes a rune or gem — `material` — only when another recipe still misses it), each
/// `{ "name", "kind", "material", "result", "req", "notes", "quest", "inputs": [...] }`
/// (`quest`: `"ready"` when the recipe's quest reward waits to be taken, `"todo"` while the
/// quest isn't done, else `null`; a recipe whose quest reward is used, or a runeword the
/// character has a copy of for every stage slot wanting it, is left out) with every input as
/// `{ "text", "status", "have", "need", "hint" }`: `status` is `"have"`/`"missing"` for a rune or gem (counted over
/// all of the recipe's lines naming it, against [`owned_materials`]), `"likely"` for a base
/// line naming a white socketed base the character holds (`hint` lists them), else
/// `"check"` (a base the reader can't verify).
pub fn recipes_json(info: &PanelInfo) -> Vec<serde_json::Value> {
    let Some(build) = info.build else {
        return Vec::new();
    };
    // A quest-reward recipe (Charsi's imbue, Larzuk's socket) is gone once its reward is
    // used (user request, 2026-11-18); with the flags unread it stays.
    let reward = |r: &crate::config::Recipe| {
        crate::quests::word_of(&r.quest).and_then(|w| crate::quests::reward(info.live.quests, w))
    };
    // A runeword is built once the character has a copy for every slot of the stage that
    // wants it (player and merc slots naming it, at least one): its recipe goes (user request,
    // 2026-11-18; Spirit sword plus Spirit shield stays until both exist). Other kinds have no
    // such signal: a craft's result is a rare with a random name, cube recipes (repair,
    // recharge) are repeatable, a socket recipe's base isn't told apart from a found one; only
    // their quest rewards (above) drop them.
    let already_made = |r: &crate::config::Recipe| {
        r.kind == "runeword" && runeword_copies(info, &r.name) >= runeword_slots(build, &r.name)
    };
    let recipes: Vec<&crate::config::Recipe> = build
        .recipes
        .iter()
        .filter(|r| reward(r) != Some(crate::quests::Reward::Used) && !already_made(r))
        .collect();
    let materials = owned_materials(info);
    // The runes and gems the other recipes still miss: an `upgrade` recipe (3 Ral → Ort,
    // gem upgrades) is shown only when its result is one of them (coordinator request,
    // 2026-11-17: the tornado guide lists ~60 of them).
    let missing: Vec<String> = recipes
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
                        .and_then(|i| i.have(&materials))
                        .is_some_and(|have| have < *need)
                })
                .map(|(key, _)| key)
                .collect::<Vec<_>>()
        })
        .collect();
    recipes
        .into_iter()
        .filter(|recipe| {
            let makes = Input::parse(&recipe.result).need();
            recipe.kind != "upgrade" || makes.is_none_or(|(key, _)| missing.contains(&key))
        })
        .map(|recipe| {
            let inputs: Vec<Input> = recipe.inputs.iter().map(|t| Input::parse(t)).collect();
            let totals = need_totals(&inputs);
            let lines: Vec<serde_json::Value> = recipe
                .inputs
                .iter()
                .zip(&inputs)
                .map(|(text, input)| {
                    let have = input.have(&materials);
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
                })
                .collect();
            serde_json::json!({
                "name": recipe.name,
                "kind": recipe.kind,
                "material": Input::parse(&recipe.result).need().is_some(),
                "result": recipe.result,
                "req": recipe.req,
                "notes": recipe.notes,
                "quest": match reward(recipe) {
                    Some(crate::quests::Reward::Ready) => Some("ready"),
                    Some(crate::quests::Reward::NotEarned) => Some("todo"),
                    Some(crate::quests::Reward::Used) | None => None,
                },
                "inputs": lines,
            })
        })
        .collect()
}

/// The material keys a recipe's inputs need, summed over its lines (a runeword naming one
/// rune twice needs two).
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
pub fn rune_count(rune: &str, materials: &[(String, u32)]) -> u32 {
    Input::Material {
        name: format!("{rune} Rune"),
        need: 0,
    }
    .have(materials)
    .unwrap_or(0)
}
