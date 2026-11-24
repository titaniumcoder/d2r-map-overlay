//! The companion web app, core half: a `localhost` HTTP server for the live dashboard (`/`) and
//! the build selection page (`/config`), with the core's dashboard panels.

#[cfg(any(test, feature = "testutil"))]
pub mod fixtures;
pub mod page;
mod server;
mod skills;
mod stash;
#[cfg(test)]
mod tests;
pub mod vendor;

pub use server::{clear_live, publish_live, serve, write_atomic};
pub use skills::SkillTarget;
pub use vendor::ShopNote;

/// Class names by txt row id.
const CLASS_NAMES: [&str; 8] = [
    "amazon",
    "sorceress",
    "necromancer",
    "paladin",
    "barbarian",
    "druid",
    "assassin",
    "warlock",
];

/// One tick's dashboard data, serialized by [`live_json`] and read by the playstyle's panels.
#[derive(Debug, Clone, Copy)]
pub struct LiveInfo<'a> {
    pub player_name: &'a str,
    /// Character class txt row (see [`CLASS_NAMES`]).
    pub class: u32,
    pub char_level: Option<u8>,
    pub difficulty: Option<u8>,
    /// The current area's name (or a `Level <id>` fallback).
    pub area: &'a str,
    pub blocks: &'a crate::render::TextBlocks,
    /// `(body slot, quality)` pairs.
    pub equipped: &'a [(u8, u32)],
    pub equipped_items: &'a [crate::game::Equipped],
    /// The player's own inventory, cube and stash items with their stats.
    pub owned_items: &'a [(crate::game::ItemDrop, crate::game::Place)],
    /// Identified unique/set names owned (equipped, carried, personally stashed).
    pub owned: &'a [String],
    /// One name per runeword copy outside the shared stash, the mercenary's included.
    pub runeword_copies: &'a [String],
    pub merc_worn: &'a [String],
    pub stash: &'a [crate::stash::StashTab],
    /// Identified unique/set names in the shared stash.
    pub stash_owned: &'a [String],
    pub skills: &'a crate::game::Skills,
    /// Runes and gems held outside the shared materials tab.
    pub held_materials: &'a [(String, u32)],
    pub white_bases: &'a [String],
    /// Quest flags of the current game; empty when unread.
    pub quests: &'a [u16],
    pub shop: Option<&'a crate::shop::Shop>,
    /// Base strength.
    pub strength: Option<u32>,
    /// Base dexterity.
    pub dexterity: Option<u32>,
    /// Marked ground drops by unit id.
    pub marks: &'a crate::guidance::Marks,
    /// Unix seconds of the tick that produced this data.
    pub updated_at: u64,
}

impl LiveInfo<'_> {
    #[must_use]
    pub const fn holdings(&self) -> crate::stash::Holdings<'_> {
        crate::stash::Holdings {
            stash: self.stash,
            owned: self.owned,
            runeword_copies: self.runeword_copies,
            merc_worn: self.merc_worn,
        }
    }
}

/// What the playstyle adds to one tick's dashboard.
#[derive(Debug, Default)]
pub struct Panels<'a> {
    /// Top-level sections of the live JSON; a key the core sets itself is never overwritten.
    pub sections: serde_json::Map<String, serde_json::Value>,
    /// The skill plan the skills panel marks the trees against (empty: no plan).
    pub skill_plan: &'a [SkillTarget],
    /// One note per item of the open trade window, in window order.
    pub shop: Vec<ShopNote>,
}

/// Serializes one tick's dashboard: the core panels from `info` plus the playstyle's `panels`.
#[must_use]
pub fn live_json(info: &LiveInfo, panels: Panels) -> String {
    live_value(info, panels).to_string()
}

/// [`live_json`] before serializing (the sounds read it too).
#[must_use]
pub fn live_value(info: &LiveInfo, panels: Panels) -> serde_json::Value {
    let class = CLASS_NAMES
        .get(usize::try_from(info.class).unwrap_or(usize::MAX))
        .map_or_else(|| format!("class {}", info.class), |name| (*name).to_owned());
    let difficulty = info.difficulty.and_then(crate::info::difficulty_name);
    let mut json = serde_json::json!({
        "live": true,
        "updated_at": info.updated_at,
        "player": {
            "name": info.player_name,
            "class": class,
            "level": info.char_level,
            "difficulty": difficulty,
        },
        "area": info.area,
        "guide": info.blocks.guide.iter().map(band_line).collect::<Vec<_>>(),
        "equipped": text_lines(&equipped_lines(info.equipped), [255, 255, 255, 255]),
        "stash_pages": stash::stash_pages(info.stash),
        "stash_materials": stash::stash_materials(info.stash),
        "stash_owned": info.stash_owned,
        "skills": skills::skills_json(info.class, info.char_level, info.skills, panels.skill_plan),
        "shop": vendor::shop_json(info, &panels.shop),
        "drops": drops_json(info.marks),
        "countess_runes": countess_runes_json(),
    });
    if let Some(obj) = json.as_object_mut() {
        for (key, value) in panels.sections {
            obj.entry(key).or_insert(value);
        }
    }
    json
}

/// The runes of the Countess's own rune drop per rune code: the per-kill chance in percent
/// per difficulty `[Normal, Nightmare, Hell]`, `null` where she doesn't drop it that way.
fn countess_runes_json() -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (difficulty, runes) in crate::drops_data::COUNTESS_RUNES.iter().enumerate() {
        for &(code, chance) in *runes {
            let entry = out
                .entry(code.to_owned())
                .or_insert_with(|| serde_json::json!([null, null, null]));
            if let Some(slot) = entry.get_mut(difficulty) {
                *slot = serde_json::json!(chance);
            }
        }
    }
    serde_json::Value::Object(out)
}

/// The marked ground drops sorted by unit id, so an unchanged frame gives the same JSON.
fn drops_json(marks: &crate::guidance::Marks) -> Vec<serde_json::Value> {
    let mut drops: Vec<_> = marks.iter().collect();
    drops.sort_unstable_by_key(|(id, _)| **id);
    drops
        .into_iter()
        .map(|(id, mark)| {
            serde_json::json!({
                "id": id,
                "kind": mark.kind,
                "tier": mark.trade.map(|(tier, _)| tier),
            })
        })
        .collect()
}

fn text_lines(lines: &[String], colour: [u8; 4]) -> Vec<serde_json::Value> {
    let hex = hex(colour);
    lines
        .iter()
        .map(|text| serde_json::json!({ "text": text, "colour": hex }))
        .collect()
}

/// A guide band in its base colour; the per-span colours are dropped on the dashboard.
fn band_line(band: &crate::info::Band) -> serde_json::Value {
    serde_json::json!({ "text": band.text, "colour": hex(band.colour) })
}

/// An RGBA colour as `#rrggbb`.
#[must_use]
pub fn hex([r, g, b, _]: [u8; 4]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `helm: unique`, `ring2: rare`, …
fn equipped_lines(equipped: &[(u8, u32)]) -> Vec<String> {
    equipped
        .iter()
        .map(|&(body, quality)| {
            let slot =
                crate::info::body_slot_name(body).map_or_else(|| format!("slot {body}"), str::to_owned);
            format!("{slot}: {}", quality_name(quality))
        })
        .collect()
}

fn quality_name(quality: u32) -> String {
    use crate::offsets::item_data;
    match quality {
        item_data::QUALITY_SET => "set".to_owned(),
        item_data::QUALITY_UNIQUE => "unique".to_owned(),
        item_data::QUALITY_RARE => "rare".to_owned(),
        item_data::QUALITY_MAGIC => "magic".to_owned(),
        item_data::QUALITY_SUPERIOR => "superior".to_owned(),
        item_data::QUALITY_NORMAL => "normal".to_owned(),
        _ => format!("quality {quality}"),
    }
}
