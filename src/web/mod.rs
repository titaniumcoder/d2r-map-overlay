//! The companion web app, core half: a small HTTP server on `localhost` ([`server`]) serving
//! the live dashboard (`/`) and the build selection page (`/config`, [`page`]).
//!
//! The dashboard's JSON ([`live_json`]) is the core's panels — character bar, breakpoint guide,
//! equipped list, shared stash ([`stash`]), skill trees ([`skills`]) and vendor window
//! ([`vendor`]) — plus the playstyle's ([`Panels`], built by
//! [`crate::playstyle::Playstyle::panels`]: sell / keep, gear grid, recipes, farm list, …).
//! The `/config` JSON and the build selection writes go through the playstyle too
//! ([`crate::playstyle::Playstyle::config_json`], [`crate::playstyle::Playstyle::select_build`]).
//! `live` publishes the dashboard JSON each tick ([`publish_live`]), and the gear state the
//! config page annotates from ([`publish_gear`]).

pub mod page;
mod server;
mod skills;
mod stash;
#[cfg(test)]
pub mod tests;
pub mod vendor;

pub use server::{GearInfo, clear_live, publish_gear, publish_live, serve};
pub use skills::SkillTarget;
pub use vendor::ShopNote;

/// The character classes by txt row id, in [`crate::breakpoints`] per-class table order.
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

/// One tick's dashboard data, gathered by `live` from the snapshot and the dashboard's text
/// blocks ([`crate::render::TextBlocks`]) and serialized by [`live_json`]. The playstyle's
/// panels read it too ([`crate::playstyle::Playstyle::panels`]).
#[derive(Debug, Clone, Copy)]
pub struct LiveInfo<'a> {
    pub player_name: &'a str,
    /// Character class txt row (see [`CLASS_NAMES`]).
    pub class: u32,
    /// `None` when the level stat was unreadable.
    pub char_level: Option<u8>,
    /// `None` when unreadable (in the menu no snapshot exists at all).
    pub difficulty: Option<u8>,
    /// The current area's name (or a `Level <id>` fallback).
    pub area: &'a str,
    pub blocks: &'a crate::render::TextBlocks,
    /// The equipped `(body slot, quality)` pairs, shown under the `BiS` chase list.
    pub equipped: &'a [(u8, u32)],
    /// The equipped items enriched for the gear grid's column headers
    /// ([`crate::game::Equipped`]): per body slot code, name and runeword flag.
    pub equipped_items: &'a [crate::game::Equipped],
    /// Display names of the identified unique/set items the character owns (equipped,
    /// carried, personally stashed): the grid pieces' ✓ ticks.
    pub owned: &'a [String],
    /// One name per runeword copy outside the shared stash, the mercenary's worn ones included
    /// ([`crate::game::Snapshot::runeword_copies`]): the Recipes panel's built count.
    pub runeword_copies: &'a [String],
    /// The identified unique/set and runeword names the mercenary wears
    /// ([`crate::game::Snapshot::merc_worn`]): counted copies (farm list, stage switch) and
    /// worn ones (the playstyle's farm list).
    pub merc_worn: &'a [String],
    /// The (session-cached) shared-stash tabs, shown in their own panel.
    pub stash: &'a [crate::stash::StashTab],
    /// Names of the identified unique/set items in the shared stash, emitted as
    /// `stash_owned` for reference; the per-`BiS`-line `tick` ([`info::Tick`]) is what the
    /// page marks ✓(stash) from.
    pub stash_owned: &'a [String],
    /// The character's skill allocation, shown as the class's three skill trees.
    pub skills: &'a crate::game::Skills,
    /// Runes and gems held outside the shared materials tab ([`crate::game::Snapshot`]).
    pub held_materials: &'a [(String, u32)],
    /// White socketed bases the character holds ([`crate::stash::white_bases`]).
    pub white_bases: &'a [String],
    /// The quest flags of the current game ([`crate::quests::read`]); empty when unread.
    pub quests: &'a [u16],
    /// The vendor trade window open right now ([`crate::game::Snapshot::shop`]).
    pub shop: Option<&'a crate::shop::Shop>,
    /// Base strength and dexterity ([`crate::game::Player::strength`]); `None` if unread.
    pub strength: Option<u32>,
    pub dexterity: Option<u32>,
    /// Unix seconds of the tick that produced this data.
    pub updated_at: u64,
}

impl LiveInfo<'_> {
    /// What the account holds, for copy counts ([`crate::stash::Holdings`]).
    pub const fn holdings(&self) -> crate::stash::Holdings<'_> {
        crate::stash::Holdings {
            stash: self.stash,
            owned: self.owned,
            runeword_copies: self.runeword_copies,
            merc_worn: self.merc_worn,
        }
    }
}

/// What the playstyle adds to one tick's dashboard ([`crate::playstyle::Playstyle::panels`]):
/// its own sections, and its part of the core skills and vendor panels.
#[derive(Debug, Default)]
pub struct Panels<'a> {
    /// The playstyle's top-level sections of the live JSON, by key (in this build: `sell`,
    /// `move`, `keep_own`, `keep_trade`, `give`, `bis`, `gear`, `stage`, `recipes`,
    /// `upgrades`, `gear_attention`, `gear_attention_why`, `farm`). Merged into the core's
    /// object; a key the core sets itself is never overwritten.
    pub sections: serde_json::Map<String, serde_json::Value>,
    /// The skill plan the skills panel marks the trees against (empty: no plan).
    pub skill_plan: &'a [SkillTarget],
    /// The vendor panel's notes, one per item of the open trade window in window order
    /// (missing ones: no slot, not better).
    pub shop: Vec<ShopNote>,
}

/// Serializes one tick's dashboard data: the core panels from `info` plus the playstyle's
/// `panels`. Pure: the same input gives the same JSON, so the tests need no game. The core's
/// part:
/// `{ "live": true, "updated_at": <unix secs>, "player": {"name", "class", "level",
/// "difficulty"}, "area": …, "guide": […], "equipped": […], "stash_pages": […],
/// "stash_materials": {…}, "stash_owned": […], "skills": {…} | null, "shop": {…} | null }`,
/// where each line is `{ "text": …, "colour": "#rrggbb" }` (the overlay line's colour as hex;
/// the guide's per-span colours are flattened to the base colour). The shared stash comes as
/// `stash_pages` (the normal tabs in the game's UI order, [`stash::stash_pages`]) and
/// `stash_materials` (the gems/runes/materials tab's stacks, [`stash::stash_materials`]). The
/// `skills` section is the class's three skill trees ([`skills::skills_json`]), `shop` the open
/// trade window ([`vendor::shop_json`]). The playstyle's sections are listed in
/// [`crate::playstyle::web`].
#[must_use]
pub fn live_json(info: &LiveInfo, panels: Panels) -> String {
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
    });
    if let Some(obj) = json.as_object_mut() {
        for (key, value) in panels.sections {
            obj.entry(key).or_insert(value);
        }
    }
    json.to_string()
}

/// `equipped` lines as `{ "text", "colour" }` objects in the block's colour.
fn text_lines(lines: &[String], colour: [u8; 4]) -> Vec<serde_json::Value> {
    let hex = hex(colour);
    lines
        .iter()
        .map(|text| serde_json::json!({ "text": text, "colour": hex }))
        .collect()
}

/// A guide band as a `{ "text", "colour" }` object: the band's base colour as hex, spans
/// flattened away (the dashboard keeps the values readable, not the per-span red).
fn band_line(band: &crate::info::Band) -> serde_json::Value {
    serde_json::json!({ "text": band.text, "colour": hex(band.colour) })
}

/// An overlay `[u8; 4]` RGBA colour as `#rrggbb` (the alpha is the same 255 everywhere).
pub fn hex([r, g, b, _]: [u8; 4]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The equipped items as dashboard lines: `helm: unique`, `ring2: rare`, … (the `BiS` "already
/// better" pairs; see [`crate::game::Snapshot::equipped`]). Unknown slots and qualities are
/// shown as such rather than dropped.
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

/// The classic quality name of an `ItemData` quality (see [`crate::offsets::item_data`]).
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
