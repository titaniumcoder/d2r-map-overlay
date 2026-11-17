//! Reads one snapshot of the game state (player, level layout, presets, monsters, ground drops,
//! nearby shrines and wells).
//!
//! Lists in game memory change under us while we read, so a single bad pointer deep in a
//! list only skips that entry and is recorded in `Snapshot::warnings`; only a missing
//! player (or its level) fails the snapshot.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::items::{self, DropKind};
use crate::objects_data::{SHRINE_OBJECTS, WELL_OBJECTS};
use crate::offsets::{
    ROOM_TILE_SCALE, act, act_misc, inventory, item_data, item_mode, level, level_preset, monster_data,
    object_data, object_mode, path, player_data, preset, room_tile, room1, room2, skill, skill_txt, skills,
    stash_tab_node, stat_list, state, static_path, unit, unit_table,
};
use crate::process::ReadMem;
use crate::render::is_consumable_quest_object;
use crate::skills_data;
use crate::stash::{self, StashTab};
use crate::valuables;

/// Most units walked per unit type (over all hash slots).
const MAX_UNITS_PER_TYPE: usize = 1000;
/// Most rooms walked per level.
const MAX_ROOMS: usize = 5000;
/// Most presets walked per room.
const MAX_PRESETS_PER_ROOM: usize = 500;
/// Most stat-list entries read per item (a socketed rare has a few dozen).
const MAX_ITEM_STATS: u64 = 512;
/// Most room-tile links walked per room.
const MAX_LINKS_PER_ROOM: usize = 100;
/// Most near-room entries read per room (`room2::NEAR_COUNT`); live counts are around 9.
const MAX_NEAR_ROOMS: u32 = 64;
/// Warnings kept in the snapshot; the rest are only counted.
const MAX_WARNINGS: usize = 50;
/// Living desecrated monsters needed to call a level terrorized. Two rather than one, so a
/// single misread unit can't flip the info line; the player's summons never carry the state.
pub const TERROR_MIN_MONSTERS: usize = 2;
/// Most items read per shared-stash tab (a tab holds 10×10; the gems/runes/materials tab had
/// 49 stacks live); a runaway chain means a misread, and the warnings tell the rest.
const MAX_STASH_ITEMS_PER_TAB: usize = 100;
/// Most nodes walked in the player's shared-stash tab list (live: 6).
const MAX_STASH_TABS: usize = 16;
/// Most nodes walked in the player's skill list (live: 19; a class has 30 skills, plus the
/// basic ones and gear-granted skills of other classes).
const MAX_SKILLS: usize = 128;

/// Agreeing owner ids (`item_data::OWNER`) after which a storage page stops voting.
const OWNER_VOTES: u32 = 8;

/// Shrine types (shrines.txt codes, see `config::SHRINE_NAMES`) of a real shrine; 0 is "none".
const SHRINE_TYPES: std::ops::RangeInclusive<u8> = 1..=22;

/// Level ids of the five towns (levels.txt): Rogue Encampment, Lut Gholein, Kurast Docks,
/// Pandemonium Fortress, Harrogath.
pub const TOWN_LEVELS: [u32; 5] = [1, 40, 75, 103, 109];

/// True if `level_id` is a town (the overlay is hidden there).
pub fn is_town(level_id: u32) -> bool {
    TOWN_LEVELS.contains(&level_id)
}

/// A game address, serialized as a hex string (JSON numbers lose precision above 2^53).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Addr(pub u64);

impl Serialize for Addr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:#x}", self.0))
    }
}

impl<'de> Deserialize<'de> for Addr {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let hex = s.strip_prefix("0x").unwrap_or(&s);
        u64::from_str_radix(hex, 16)
            .map(Self)
            .map_err(|e| serde::de::Error::custom(format!("bad address {s:?}: {e}")))
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub player: Player,
    /// The local player's equipped `(body slot, quality)` pairs: the `BiS` per-slot "already
    /// better" check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equipped: Vec<(u8, u32)>,
    /// The mercenary's equipped `(body slot, quality)` pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merc_equipped: Vec<(u8, u32)>,
    /// The player's equipped items, enriched for the dashboard's gear grid: per body slot
    /// the item code, the runeword flag and the identified unique/set name + row
    /// ([`Equipped`]). Kept next to [`Snapshot::equipped`], which other code consumes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equipped_items: Vec<Equipped>,
    /// Display names of identified unique/set items the player owns (equipped, carried,
    /// stashed): the `BiS` list marks owned pieces with a ✓.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owned_names: Vec<String>,
    /// The shared-stash tabs as read this snapshot. Empty once the tab units vanish (stash UI
    /// closed); the session keeps the last capture ([`crate::session::Session`]). Left out of
    /// the JSON when empty, so older dumps stay loadable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stash: Vec<StashTab>,
    /// Display names of the identified unique/set items in [`Snapshot::stash`]: the `BiS` ✓
    /// for pieces sitting in the shared stash, kept apart from [`Snapshot::owned_names`] so
    /// the dashboard can show ✓(stash) distinctly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stash_names: Vec<String>,
    pub player_candidates: Vec<Candidate>,
    /// Which rule picked `player` among `player_candidates`. Left out of the JSON when it is the
    /// default (first candidate), so older dumps round-trip.
    #[serde(default, skip_serializing_if = "ChosenBy::is_first_candidate")]
    pub player_chosen_by: ChosenBy,
    pub level: Level,
    pub presets: Vec<Preset>,
    pub room_links: Vec<RoomLink>,
    /// Rooms of other levels in this level's near-room lists (`room2::NEAR`): walk-through
    /// borders to adjacent levels, which have no tile preset or room tile. Deduplicated; left
    /// out of the JSON when empty, so older dumps and fixtures stay unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub border_links: Vec<BorderLink>,
    pub monsters: Vec<Monster>,
    /// Item units lying on the ground (shown or not). Defaults keep older dumps loadable.
    #[serde(default)]
    pub ground_items: u32,
    /// The ground items worth showing.
    #[serde(default)]
    pub drops: Vec<ItemDrop>,
    /// Shrines near the player (objects are loaded only in range). Live mode keeps only the
    /// unused ones of the configured types for drawing ([`crate::config::Shrines::retain_shown`]).
    /// Left out of the JSON when empty, like the fields below.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shrines: Vec<Shrine>,
    /// Wells near the player.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wells: Vec<Well>,
    /// Operated consumable quest objects near the player (Diablo seals; see
    /// [`crate::render::CONSUMABLE_QUEST_OBJECTS`]). The session hides their presets for the game.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operated_objects: Vec<OperatedObject>,
    /// Items in the player's inventory (shown or not); live-mode only, not in the JSON.
    #[serde(skip)]
    pub inventory_items: u32,
    /// Inventory items worth keeping (S and A tiers, whatever the config; see
    /// [`valuables::keep_list`]). Shown on the dashboard (keep panel).
    /// Left out of the JSON when empty, so older dumps and fixtures stay unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inventory_keep: Vec<valuables::Keep>,
    /// The player's stored items to sell (inventory, cube, personal and shared stash; see
    /// [`valuables::spare_lists`]). Grey lines (dashboard). Left out of the JSON when
    /// empty, like `give_away`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sell: Vec<valuables::Spare>,
    /// The player's stored items to give away (best rule is tier B, or a further copy of a
    /// keep-one item). Yellow lines below `sell`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub give_away: Vec<valuables::Spare>,
    /// 0 Normal, 1 Nightmare, 2 Hell; `None` if unreadable (and in old fixtures).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u8>,
    /// The level is a Terror Zone ([`is_terrorized`] over the live monsters). The session keeps
    /// it set for the level for the rest of the game, once seen. Left out of the JSON when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terrorized: bool,
    /// The cow counter ([`crate::cows`]), set by the session in the Moo Moo Farm only; it
    /// replaces the monster levels in the info line. Left out of the JSON when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cows: Option<crate::cows::Tally>,
    /// Where the player has been in this game's Moo Moo Farm (world tiles, oldest first), set
    /// by the session there only and drawn as a faint path under the markers. Left out of the
    /// JSON when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trail: Vec<(u16, u16)>,
    /// The player's skill allocation ([`Skills`]); empty when unreadable. Left out of the JSON
    /// when empty, so older dumps stay loadable.
    #[serde(default, skip_serializing_if = "Skills::is_empty")]
    pub skills: Skills,
    pub warnings: Vec<String>,
}

/// The player's skill allocation, read from the skill list ([`skill`]): the class skills with
/// hard (allocated) points and the skills on the two mouse buttons. Bonus levels from gear
/// are not included (the skill nodes don't carry them).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skills {
    /// `(skills.txt id, hard points)` of the player's own class skills with points, in list
    /// order ([`skills_data::SKILLS`] has their tree places).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<(u16, u8)>,
    /// The skill id on the left mouse button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<u16>,
    /// The skill id on the right mouse button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<u16>,
}

impl Skills {
    pub const fn is_empty(&self) -> bool {
        self.points.is_empty() && self.left.is_none() && self.right.is_none()
    }
}

/// True if at least [`TERROR_MIN_MONSTERS`] living monsters carry the desecrated state.
pub fn is_terrorized(monsters: &[Monster]) -> bool {
    monsters.iter().filter(|m| m.desecrated && !m.is_dead()).count() >= TERROR_MIN_MONSTERS
}

/// True if state `state` is set in the state bitfield `flags` (see
/// [`stat_list::STATE_FLAGS`]: little-endian u32 dwords, bit `n % 32` of dword `n / 32`). A
/// state past the end of `flags` is clear.
pub fn has_state(flags: &[u8], state: u32) -> bool {
    let (dwords, _) = flags.as_chunks::<4>();
    usize::try_from(state / 32)
        .ok()
        .and_then(|i| dwords.get(i))
        .is_some_and(|&d| u32::from_le_bytes(d) & (1 << (state % 32)) != 0)
}

/// The difficulty from the raw `ActMisc` value; anything above Hell (2) is unknown.
pub const fn difficulty_from_raw(raw: u16) -> Option<u8> {
    match raw {
        0 => Some(0),
        1 => Some(1),
        2 => Some(2),
        _ => None,
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Player {
    pub addr: Addr,
    pub id: u32,
    pub name: String,
    /// Character class (txt row).
    pub class: u32,
    pub x: u16,
    pub y: u16,
    pub level_id: u32,
    /// The player's act; changes with a new game (and on act travel). `None` in old fixtures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<Addr>,
    /// Character level (stat [`stat_list::LEVEL`], see [`char_level_from_raw`]); `None` if
    /// unreadable (and in old fixtures, where it is left out of the JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_level: Option<u8>,
    /// Speed stats and elemental resistances from the player's stat list, for the
    /// breakpoint/gear guide (user request, 2026-11-17). `None` if the stat list is unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<SpeedStats>,
    /// The character's build from the config ([`crate::config::Config::damages_for_player`]), primary
    /// damage first; colours the info line's monster levels ([`crate::build::rate`]). Not read
    /// from the game nor kept in the JSON: set from the config after reading or loading.
    #[serde(skip)]
    pub build: Vec<crate::build::Damage>,
    /// The character's `BiS` build and gear list from the config ([`crate::config::Bis`]); shown
    /// as a chase list under the guide. Config-set like `build`, not kept in the JSON.
    #[serde(skip)]
    pub bis: Option<crate::config::Bis>,
    /// The equipped weapon's WSM (weapon speed modifier, weapons.txt `speed` column); shown
    /// next to IAS in the guide (its frames need the weapon speed). `None` without a readable
    /// weapon or items table. Set in `snapshot`, not kept in the JSON.
    #[serde(skip)]
    pub wsm: Option<i32>,
}

/// The player's speed stats and elemental resistances as granted by gear (stat ids from
/// itemstatcost.txt, summed over the equipped items' affix lists and carried charms;
/// difficulty penalties are NOT included — they are applied when the guide displays them).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeedStats {
    /// `item_fastercastrate` (FCR, %).
    pub fcr: u32,
    /// `item_fasterattackrate` (IAS, %).
    pub ias: u32,
    /// `item_fasterhitrecovery` (FHR, %).
    pub fhr: u32,
    /// The four elemental resistances as granted by gear (fire, cold, lightning, poison).
    pub res_fire: u32,
    pub res_cold: u32,
    pub res_lightning: u32,
    pub res_poison: u32,
}

/// The character level from the raw stat value: 1..=99, or the same shifted left by 8 (some D2
/// stats, like life, are stored in 1/256 units). Anything else is unknown.
pub fn char_level_from_raw(raw: u32) -> Option<u8> {
    let level = if raw > 99 && raw.trailing_zeros() >= 8 {
        raw >> 8
    } else {
        raw
    };
    u8::try_from(level).ok().filter(|l| (1..=99).contains(l))
}

/// The plain value of a raw stat: most D2 stats are stored as-is, but some (life, mana, level…)
/// in 1/256 units — a value far above the stat's realistic maximum with 8 trailing zeros is such
/// an encoding. Good enough for the guide stats (FCR/IAS/FHR/resists, all ≤ 255 in practice).
const fn plain_stat_from_raw(raw: u32) -> u32 {
    if raw > 255 && raw.trailing_zeros() >= 8 {
        raw >> 8
    } else {
        raw
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub addr: Addr,
    /// Unit id; 0 in older dumps (and then left out of the JSON).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub id: u32,
    pub name: String,
}

/// How [`snapshot`] picked the player among the candidates. In a group game the other players'
/// units are candidates too; the owner of the stash items is the local player.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChosenBy {
    /// `--player NAME`.
    Name,
    /// The player chosen on the previous tick (or saved in `game.json`) is still a candidate:
    /// kept, so a group game's owner vote can't flip the choice and reset the session.
    Previous,
    /// The majority owner id of the stash items.
    StashOwner,
    /// No stash items (or none of a candidate): the majority owner id of the inventory items.
    InventoryOwner,
    /// No owner vote matched: the first candidate in unit-table order.
    #[default]
    FirstCandidate,
}

/// For `skip_serializing_if` (serde passes a reference).
#[allow(clippy::trivially_copy_pass_by_ref)] // reason: serde's skip_serializing_if signature
const fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl ChosenBy {
    #[allow(clippy::trivially_copy_pass_by_ref)] // reason: serde's skip_serializing_if signature
    const fn is_first_candidate(&self) -> bool {
        matches!(self, Self::FirstCandidate)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Previous => "previous",
            Self::StashOwner => "stash owner",
            Self::InventoryOwner => "inventory owner",
            Self::FirstCandidate => "first candidate",
        }
    }
}

/// Owner unit ids of stored items on one storage page, counted per id.
#[derive(Debug, Default)]
struct OwnerVotes(Vec<(u32, u32)>);

impl OwnerVotes {
    /// Counts `owner`; [`item_data::NO_OWNER`] and 0 (unset or unreadable-looking) don't vote.
    fn add(&mut self, owner: u32) {
        if owner == item_data::NO_OWNER || owner == 0 {
            return;
        }
        match self.0.iter_mut().find(|(id, _)| *id == owner) {
            Some((_, n)) => *n += 1,
            None => self.0.push((owner, 1)),
        }
    }

    /// True once one id has [`OWNER_VOTES`] votes: more items can't change the answer much.
    fn done(&self) -> bool {
        self.0.iter().any(|&(_, n)| n >= OWNER_VOTES)
    }

    /// The id with the most votes (the first seen on a tie); `None` without votes.
    fn winner(&self) -> Option<u32> {
        let mut best: Option<(u32, u32)> = None;
        for &(id, n) in &self.0 {
            if best.is_none_or(|(_, b)| n > b) {
                best = Some((id, n));
            }
        }
        best.map(|(id, _)| id)
    }
}

/// What one walk over the item units found.
#[derive(Debug, Default)]
struct ItemScan {
    /// Item units on the ground (shown or not).
    ground: u32,
    /// Ground items worth showing.
    drops: Vec<ItemDrop>,
    /// All items on the inventory page, with their owner id (`None` if unreadable).
    inventory: Vec<(ItemDrop, Option<u32>)>,
    /// Stored items on the cube and stash pages that may be spare ([`valuables::may_spare`]),
    /// any owner: the owner filter needs the chosen player. Inventory items are in `inventory`.
    stored: Vec<Stored>,
    stash_owners: OwnerVotes,
    inventory_owners: OwnerVotes,
    /// Speed/resist stats summed over each owner's equipped items and carried charms (the
    /// guide's numbers; the player's own stat list is base-only, no item stats).
    gear: HashMap<u32, SpeedStats>,
    /// The equipped weapon's WSM per owner (the guide's IAS note).
    weapons: HashMap<u32, i32>,
    /// Display names of identified unique/set items per owning candidate (equipped, carried,
    /// stashed): the `BiS` list marks owned pieces with a ✓. Keyed by owner because the
    /// player is chosen only after the scan — in a group game every candidate's items are
    /// units (live 2026-11-17: other players' gear showed in the grid).
    owned_names: HashMap<u32, Vec<String>>,
    /// Each candidate's equipped `(body slot, quality)` pairs (the `BiS` "already better"
    /// check: set/unique quality there hides the slot).
    equipped: HashMap<u32, Vec<(u8, u32)>>,
    /// The same items, enriched for the dashboard's gear grid ([`game::Equipped`]): one per
    /// body slot (weapon and swap weapon share body 4, like the pairs), per candidate. The
    /// mercenary's gear has no grid.
    equipped_items: HashMap<u32, Vec<Equipped>>,
    /// The same for the mercenary's gear (owner `NO_OWNER`).
    merc_equipped: Vec<(u8, u32)>,
}

/// One live shared-stash tab of the player ([`Reader::shared_tabs`]).
#[derive(Debug)]
struct TabRef {
    /// The tab unit's id: the `item_data::OWNER` of its normal-page items ([`place`]).
    id: u32,
    /// The tab unit's address; `None` while the unit isn't in the unit table (the stash not
    /// loaded yet), which reads as an empty tab.
    addr: Option<u64>,
    kind: stash::TabKind,
}

/// A stored item with its `item_data::PAGE` and `item_data::OWNER`.
#[derive(Debug)]
struct Stored {
    item: ItemDrop,
    page: u8,
    owner: u32,
}

/// Where a stored item on `page` with `owner` is, for the player with unit id `player`; `None`
/// if it isn't the player's. Pages 0 (inventory), 3 (cube) and 4 (stash) count when the player
/// owns the item; page 4 also when one of the player's `shared_tabs` owns it.
fn place(page: u8, owner: u32, player: u32, shared_tabs: &[u32]) -> Option<valuables::Place> {
    use valuables::Place;
    match page {
        item_data::PAGE_INVENTORY if owner == player => Some(Place::Inventory),
        item_data::PAGE_CUBE if owner == player => Some(Place::Cube),
        item_data::PAGE_STASH if owner == player => Some(Place::Stash),
        // Page-4 items of other owners: only the player's own shared-stash tab units count
        // ([`Reader::shared_tabs`]). Stale tab units from other characters or the offline shared
        // stash are left out (live 2026-11-15: BigWolf's Gravenspine showed up while playing Vex).
        item_data::PAGE_STASH if shared_tabs.contains(&owner) => Some(Place::Shared),
        _ => None,
    }
}

/// The items of unit id `player` with their place, the one set both the keep list and the spare
/// lists are computed from (a keep-one item is kept once across all places): the `inventory`
/// items it owns (in a group game the other players' inventory items are units too) and the
/// `stored` cube and stash items that are its own ([`place`], with the unit ids of its
/// `shared_tabs`).
fn player_items(
    inventory: Vec<(ItemDrop, Option<u32>)>,
    stored: Vec<Stored>,
    player: u32,
    shared_tabs: &[u32],
) -> Vec<(ItemDrop, valuables::Place)> {
    let inv = inventory
        .into_iter()
        .filter_map(|(d, owner)| (owner == Some(player)).then_some((d, valuables::Place::Inventory)));
    let stashed = stored
        .into_iter()
        .filter_map(|s| place(s.page, s.owner, player, shared_tabs).map(|p| (s.item, p)));
    inv.chain(stashed).collect()
}

/// The candidate to use as the player and the rule that picked it; `None` if no candidate fits
/// (none named `player_name`, or no candidates at all). Precedence: `player_name`, then the
/// `previous` unit id while it is still a candidate, then the owner votes, then the first.
fn choose_player<'a>(
    candidates: &'a [Candidate],
    player_name: Option<&str>,
    previous: Option<u32>,
    scan: &ItemScan,
) -> Option<(&'a Candidate, ChosenBy)> {
    if let Some(name) = player_name {
        return candidates
            .iter()
            .find(|c| c.name == name)
            .map(|c| (c, ChosenBy::Name));
    }
    if let Some(c) = previous.and_then(|id| candidates.iter().find(|c| c.id == id)) {
        return Some((c, ChosenBy::Previous));
    }
    let by_owner = |votes: &OwnerVotes, by: ChosenBy| {
        let owner = votes.winner()?;
        candidates.iter().find(|c| c.id == owner).map(|c| (c, by))
    };
    by_owner(&scan.stash_owners, ChosenBy::StashOwner)
        .or_else(|| by_owner(&scan.inventory_owners, ChosenBy::InventoryOwner))
        .or_else(|| candidates.first().map(|c| (c, ChosenBy::FirstCandidate)))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Level {
    pub id: u32,
    /// `None` if the level has no readable rooms.
    pub bounds: Option<Bounds>,
    pub rooms: Vec<Room>,
}

/// World-tile bounding box; `max` is exclusive (room x + w).
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub struct Bounds {
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
}

/// A room in world tiles.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Room {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub loaded: bool,
    /// Level-preset id (lvlprest row) of the room; `None` if unreadable (and in old fixtures).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PresetKind {
    Monster,
    Object,
    Tile,
    /// Unknown kind, serialized as the raw number.
    #[serde(untagged)]
    Other(u32),
}

impl PresetKind {
    const fn from_raw(t: u32) -> Self {
        match t {
            preset::MONSTER => Self::Monster,
            preset::OBJECT => Self::Object,
            preset::TILE => Self::Tile,
            other => Self::Other(other),
        }
    }
}

/// A preset unit in world tiles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Preset {
    pub kind: PresetKind,
    pub txt: u32,
    pub x: u32,
    pub y: u32,
    /// Tile preset the player entered the level by; set by `Session::apply`, never read from
    /// memory. Omitted from JSON when false, so dumps and fixtures stay unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub entry: bool,
}

/// World-tile origin of a room.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub struct RoomPos {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomLink {
    pub from_room: RoomPos,
    pub to_level_id: u32,
    pub to_room: RoomPos,
}

/// A room rectangle in world tiles.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy, Hash)]
pub struct RoomRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A room of this level next to a room of another level (see [`Snapshot::border_links`]).
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Hash)]
pub struct BorderLink {
    pub from_room: RoomRect,
    pub to_level_id: u32,
    pub to_room: RoomRect,
    /// `to_level_id` is the level the player came from; set by `Session::apply`, never read from
    /// memory. Omitted from JSON when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub entry: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Monster {
    pub addr: Addr,
    pub id: u32,
    pub txt: u32,
    pub mode: u32,
    pub x: u16,
    pub y: u16,
    pub flags: u8,
    pub name_seed: u16,
    /// Owner type/id from the `MonsterData` head (pets/mercs hypothesis, NOT yet verified):
    /// the local player's id here makes the unit their merc. Journal-logged once per unit id and
    /// game so the
    /// offset can be verified live; left out of the JSON when both are zero.
    #[serde(default, skip_serializing_if = "is_zero", alias = "owner")]
    pub owner_id: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub owner_type: u32,
    /// Carries [`state::DESECRATED`] (a Terror Zone monster). Left out of the JSON when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub desecrated: bool,
}

/// Monster modes of a dead or dying unit.
const DEAD_MODES: [u32; 2] = [0, 12];

impl Monster {
    pub fn is_dead(&self) -> bool {
        DEAD_MODES.contains(&self.mode)
    }
}

/// A shrine object (txt id in [`SHRINE_OBJECTS`]) with a shrine type in [`SHRINE_TYPES`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shrine {
    /// objects.txt row.
    pub txt: u32,
    /// Shrine type: the shrines.txt code (`ObjectData` +0x08), 2 health, 12 skill, ...
    pub code: u8,
    /// Already clicked (any unit mode other than [`object_mode::NEUTRAL`]).
    pub used: bool,
    pub x: u32,
    pub y: u32,
}

/// A well object (txt id in [`WELL_OBJECTS`]). Wells refill, so used or not doesn't matter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Well {
    /// objects.txt row.
    pub txt: u32,
    pub x: u32,
    pub y: u32,
}

/// A consumable quest object (txt id in [`crate::render::CONSUMABLE_QUEST_OBJECTS`]) that has
/// been operated (unit mode other than [`object_mode::NEUTRAL`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatedObject {
    /// objects.txt row.
    pub txt: u32,
    pub x: u32,
    pub y: u32,
}

/// True if objects.txt row `txt` is a shrine (see `research/gen_objects.py`).
pub fn is_shrine_object(txt: u32) -> bool {
    SHRINE_OBJECTS.binary_search(&txt).is_ok()
}

/// True if objects.txt row `txt` is a well (see `research/gen_objects.py`).
pub fn is_well_object(txt: u32) -> bool {
    WELL_OBJECTS.binary_search(&txt).is_ok()
}

/// A ground item worth showing (see [`items::classify`]), or a base a valuables rule matches.
/// The fields after `quality` default so older dumps stay loadable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemDrop {
    pub id: u32,
    /// Item code (`"gcg"`); `None` if the items table is unknown.
    pub code: Option<String>,
    pub kind: DropKind,
    /// Raw `ItemData` quality (2 normal, 5 set, 7 unique, ...); 0 in snapshots dumped before it existed.
    #[serde(default)]
    pub quality: u32,
    /// Raw `ItemData` flags (bits mostly unverified; see `item_data::FLAGS`).
    #[serde(default)]
    pub flags: u32,
    #[serde(default)]
    pub ethereal: bool,
    /// Socket count (stat 194); 0 if none or unreadable.
    #[serde(default)]
    pub sockets: u8,
    /// uniqueitems / setitems row of a unique or set item (`item_data::FILE_INDEX`).
    #[serde(default)]
    pub unique_index: Option<u32>,
    /// The first `item_data::HEAD_LEN` bytes of `ItemData` as hex, for checking offsets in the
    /// journal. Not part of the snapshot JSON.
    #[serde(skip)]
    pub item_data_head: String,
    /// Set by [`valuables::apply`] from the config, not by the reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<valuables::Highlight>,
    pub x: u32,
    pub y: u32,
}

/// One equipped body-slot item of the player, with what the dashboard's gear grid needs:
/// the quality the old `Snapshot::equipped` pairs carry, plus the item code, the runeword
/// flag and (for an identified unique/set) its name and table row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equipped {
    /// `item_data::BODY` slot (1 helm, 2 amulet, 3 body, 4 weapon, 5 off-hand, 6/7 rings,
    /// 8 belt, 9 boots, 10 gloves).
    pub body: u8,
    /// Raw `ItemData` quality (2 normal, 4 magic, 5 set, 7 unique, ...).
    pub quality: u32,
    /// 4-char item code ("uap"); `None` if the items table wasn't found yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Display name of an identified unique/set item (the `valuables::unique_set_name`
    /// identity path); `None` otherwise (unidentified, magic, rare, base).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// uniqueitems/setitems row (`item_data::FILE_INDEX`) of a named unique/set. Kept only
    /// with a `name`: the index is only filled once the item is identified, so without a
    /// name it is not trusted (see `item_data::FILE_INDEX`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_index: Option<u32>,
    /// `ItemData::FLAGS` runeword bit (classic; UNVERIFIED in D2R).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub runeword: bool,
}

impl Equipped {
    /// The item's required character level, as far as the reader can know it:
    /// - an identified unique/set: the exact `lvl req` of its table row;
    /// - a runeword: `None` — the socket contents (the runes) are unread, so neither the
    ///   word nor its level is known;
    /// - everything else (normal, magic, rare, crafted, ...): its BASE item's `levelreq`
    ///   (armor/weapons tables). APPROXIMATION: affixes can raise a magic/rare/crafted
    ///   item's real requirement above the base one.
    #[must_use]
    pub fn required_level(&self) -> Option<u32> {
        use crate::item_levels_data;
        use crate::offsets::item_data;
        let row = if self.name.is_some() {
            self.file_index
        } else {
            None
        };
        match self.quality {
            item_data::QUALITY_UNIQUE => row.and_then(item_levels_data::unique_level),
            item_data::QUALITY_SET => row.and_then(item_levels_data::set_level),
            _ if self.runeword => None,
            _ => {
                let code = self.code.as_deref()?;
                item_levels_data::armor_level(code).or_else(|| crate::weapons_data::level(code))
            }
        }
    }
}

/// Reads all live player candidates (unit table, `PLAYER` type); unreadable ones warn.
fn player_candidates<M: ReadMem>(r: &mut Reader<'_, M>, unit_table: u64) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for addr in r.units(unit_table, unit_table::PLAYER) {
        match r.player_candidate(addr) {
            Ok(Some(p)) => candidates.push(p),
            Ok(None) => {}
            Err(e) => r.warn(format!("player unit {addr:#x}: {e:#}")),
        }
    }
    candidates
}

/// Walks `level_addr`'s rooms and collects the level, its presets and links. The Room2 chain
/// must stay inside this level (as in research/rooms.py); a stop at the first room is an error.
/// The walk's collected data.
struct LevelWalk {
    level: Level,
    presets: Vec<Preset>,
    room_links: Vec<RoomLink>,
    border_links: Vec<BorderLink>,
}

fn walk_level(r: &mut Reader<'_, impl ReadMem>, level_addr: u64, level_id: u32) -> Result<LevelWalk> {
    let Some(first_room) = r.ptr(level_addr, level::FIRST_ROOM2)? else {
        bail!("level {level_addr:#x} has no first room");
    };
    let rooms = r.walk(Some(first_room), room2::NEXT, MAX_ROOMS, "room");
    let mut level = Level {
        id: level_id,
        bounds: None,
        rooms: Vec::new(),
    };
    let (mut presets, mut room_links, mut border_links) = (Vec::new(), Vec::new(), Vec::new());
    for (i, room_addr) in rooms.into_iter().enumerate() {
        // The Room2 chain must stay inside this level (as in research/rooms.py); stop where it
        // leaves.
        let owner = r.ptr(room_addr, room2::LEVEL);
        if !matches!(owner, Ok(Some(a)) if a == level_addr) {
            if i == 0 {
                bail!("first room {room_addr:#x} does not belong to level {level_addr:#x} ({owner:?})");
            }
            r.warn(format!(
                "room {room_addr:#x} belongs to another level ({owner:?}); room walk stopped"
            ));
            break;
        }
        let room = match r.room(room_addr) {
            Ok(room) => room,
            Err(e) => {
                r.warn(format!("room {room_addr:#x}: {e:#}"));
                continue;
            }
        };
        let pos = RoomPos { x: room.x, y: room.y };
        r.presets(room_addr, pos, &mut presets);
        r.room_links(room_addr, pos, &mut room_links);
        r.border_links(room_addr, level_addr, &room, &mut border_links);
        level.rooms.push(room);
    }
    Ok(LevelWalk {
        level,
        presets,
        room_links,
        border_links,
    })
}

/// Reads one snapshot. `unit_table` is the hash table address from [`crate::image::Image::unit_table`];
/// `items_table` is the item-code table from [`items::find_table`] (`None`: unique and set drops only).
/// `previous` is the unit id of the player chosen before (see [`ChosenBy::Previous`]).
///
/// # Errors
/// Fails if no live player unit is found (or none named `player_name`), or the player's level
/// can't be read. Unreadable entries elsewhere become warnings.
pub fn snapshot(
    mem: &impl ReadMem,
    unit_table: u64,
    items_table: Option<u64>,
    player_name: Option<&str>,
    previous: Option<u32>,
) -> Result<Snapshot> {
    let mut r = Reader {
        mem,
        warnings: Vec::new(),
        suppressed: 0,
    };

    let candidates = player_candidates(&mut r, unit_table);
    // Items first: their owner ids tell which candidate is the local player.
    let candidate_ids: Vec<u32> = candidates.iter().map(|c| c.id).collect();
    let scan = r.items(unit_table, items_table, &candidate_ids);
    let Some((chosen, chosen_by)) = choose_player(&candidates, player_name, previous, &scan) else {
        let names: Vec<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
        match player_name {
            Some(name) => bail!("no live player named {name:?} (candidates: {names:?})"),
            None => bail!("no live player unit found (not in a game?)"),
        }
    };
    let (mut player, level_addr) = r.player(chosen.addr.0).context("reading the player")?;
    // The guide's numbers: the chosen player's equipped items and carried charms.
    player.speed = scan.gear.get(&chosen.id).copied();
    player.wsm = scan.weapons.get(&chosen.id).copied();
    let equipped = scan.equipped.get(&chosen.id).cloned().unwrap_or_default();
    let merc_equipped = scan.merc_equipped.clone();
    let equipped_items = scan.equipped_items.get(&chosen.id).cloned().unwrap_or_default();
    let owned_names = scan.owned_names.get(&chosen.id).cloned().unwrap_or_default();

    let LevelWalk {
        mut level,
        mut presets,
        room_links,
        border_links,
    } = walk_level(&mut r, level_addr, player.level_id).context("walking the level's rooms")?;
    level.bounds = bounds(&level.rooms);
    dedup_presets(&mut presets);

    let mut monsters = Vec::new();
    for addr in r.units(unit_table, unit_table::MONSTER) {
        match r.monster(addr) {
            Ok(m) => monsters.push(m),
            Err(e) => r.warn(format!("monster {addr:#x}: {e:#}")),
        }
    }
    let (shrines, wells, operated_objects) = r.objects(unit_table);
    let inventory_items = u32::try_from(scan.inventory.len()).unwrap_or(u32::MAX);
    let tabs = r.shared_tabs(unit_table, chosen.addr.0, chosen.id);
    let stash = r.stash(&tabs, items_table);
    let stash_names = stash::stash_names(&stash);
    // The materials tab's stacks are owned by nobody (`NO_OWNER`), so only the normal pages'
    // items reach the spare lists through these ids; gems and runes are never spare anyway.
    let shared_tabs: Vec<u32> = tabs.iter().map(|t| t.id).collect();
    let mine = player_items(scan.inventory, scan.stored, chosen.id, &shared_tabs);
    let inventory_keep = valuables::keep_list(&mine);
    let (sell, give_away) = valuables::spare_lists(&mine);
    let difficulty = player.act.and_then(|a| r.difficulty(a.0));
    let terrorized = is_terrorized(&monsters);
    let skills = r.skills(chosen.addr.0, player.class);

    Ok(Snapshot {
        player,
        equipped,
        merc_equipped,
        equipped_items,
        owned_names,
        stash,
        stash_names,
        player_candidates: candidates,
        player_chosen_by: chosen_by,
        level,
        presets,
        room_links,
        border_links,
        monsters,
        ground_items: scan.ground,
        drops: scan.drops,
        shrines,
        wells,
        operated_objects,
        inventory_items,
        inventory_keep,
        sell,
        give_away,
        difficulty,
        terrorized,
        cows: None,
        trail: Vec::new(),
        skills,
        warnings: r.into_warnings(),
    })
}

/// Drops repeated presets (same kind, txt and position), keeping the first of each.
///
/// Live (Dry Hills, 2026-11-14) the same preset came back 3–5 times: a Room2's preset list is
/// reachable more than once or holds repeats.
fn dedup_presets(presets: &mut Vec<Preset>) {
    let mut seen = HashSet::new();
    presets.retain(|p| seen.insert(p.clone()));
}

/// Bounding box over rooms, or `None` if there are none (or a room's end overflows).
fn bounds(rooms: &[Room]) -> Option<Bounds> {
    let mut it = rooms.iter();
    let first = it.next()?;
    let mut b = Bounds {
        min_x: first.x,
        min_y: first.y,
        max_x: first.x.checked_add(first.w)?,
        max_y: first.y.checked_add(first.h)?,
    };
    for r in it {
        b.min_x = b.min_x.min(r.x);
        b.min_y = b.min_y.min(r.y);
        b.max_x = b.max_x.max(r.x.checked_add(r.w)?);
        b.max_y = b.max_y.max(r.y.checked_add(r.h)?);
    }
    Some(b)
}

/// Fields shared by all unit types.
#[derive(Clone, Debug)]
struct UnitHead {
    txt: u32,
    id: u32,
    mode: u32,
    unit_data: Option<u64>,
    act: Option<u64>,
    path: Option<u64>,
}

/// Memory access plus the warnings collected while reading one snapshot.
///
/// Generic rather than `&dyn ReadMem` because `ReadMem` has a const-generic method,
/// which makes it not object-safe.
struct Reader<'a, M: ReadMem> {
    mem: &'a M,
    warnings: Vec<String>,
    suppressed: usize,
}

impl<M: ReadMem> Reader<'_, M> {
    fn warn(&mut self, msg: String) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(msg);
        } else {
            self.suppressed = self.suppressed.saturating_add(1);
        }
    }

    /// The warnings kept, plus a line counting the suppressed ones.
    fn into_warnings(mut self) -> Vec<String> {
        if self.suppressed > 0 {
            self.warnings
                .push(format!("... and {} more warnings", self.suppressed));
        }
        self.warnings
    }

    fn ptr(&self, base: u64, off: u64) -> Result<Option<u64>> {
        let a = field(base, off)?;
        self.mem
            .read_ptr(a)
            .with_context(|| format!("reading pointer at {a:#x}"))
    }

    /// Like [`Reader::ptr`], but a null pointer is an error.
    fn ptr_req(&self, base: u64, off: u64, what: &str) -> Result<u64> {
        self.ptr(base, off)?
            .with_context(|| format!("{what} pointer at {base:#x}+{off:#x} is null"))
    }

    fn u64(&self, base: u64, off: u64) -> Result<u64> {
        let a = field(base, off)?;
        self.mem
            .read_u64(a)
            .with_context(|| format!("reading u64 at {a:#x}"))
    }

    fn u32(&self, base: u64, off: u64) -> Result<u32> {
        let a = field(base, off)?;
        self.mem
            .read_u32(a)
            .with_context(|| format!("reading u32 at {a:#x}"))
    }

    fn u16(&self, base: u64, off: u64) -> Result<u16> {
        let a = field(base, off)?;
        self.mem
            .read_u16(a)
            .with_context(|| format!("reading u16 at {a:#x}"))
    }

    fn u8(&self, base: u64, off: u64) -> Result<u8> {
        let a = field(base, off)?;
        self.mem
            .read_u8(a)
            .with_context(|| format!("reading u8 at {a:#x}"))
    }

    fn bytes(&self, base: u64, off: u64, len: usize) -> Result<Vec<u8>> {
        let a = field(base, off)?;
        self.mem
            .read_bytes(a, len)
            .with_context(|| format!("reading {len:#x} bytes at {a:#x}"))
    }

    /// Follows a singly linked list from `head` via the pointer at `next_off`.
    /// Stops at null, a revisited node (cycle), an unreadable next pointer, or `cap` nodes;
    /// the last three add a warning.
    fn walk(&mut self, head: Option<u64>, next_off: u64, cap: usize, what: &str) -> Vec<u64> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        self.walk_into(head, next_off, cap, what, &mut seen, &mut out);
        out
    }

    /// [`Reader::walk`] with a caller-owned visited set, so several chains share one cap.
    fn walk_into(
        &mut self,
        head: Option<u64>,
        next_off: u64,
        cap: usize,
        what: &str,
        seen: &mut HashSet<u64>,
        out: &mut Vec<u64>,
    ) {
        let mut cur = head;
        while let Some(node) = cur {
            if !seen.insert(node) {
                self.warn(format!("{what} list: cycle at {node:#x}"));
                return;
            }
            if out.len() >= cap {
                self.warn(format!("{what} list: truncated at {cap} entries"));
                return;
            }
            out.push(node);
            cur = match self.ptr(node, next_off) {
                Ok(next) => next,
                Err(e) => {
                    self.warn(format!("{what} list: broken after {node:#x}: {e:#}"));
                    return;
                }
            };
        }
    }

    /// All units of type `ty` from the hash table (every slot's chain).
    fn units(&mut self, table: u64, ty: u32) -> Vec<u64> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let base = u64::from(ty)
            .checked_mul(unit_table::SLOTS * unit_table::SLOT_SIZE)
            .and_then(|o| table.checked_add(o));
        let Some(base) = base else {
            self.warn(format!("unit table {table:#x}: type {ty} block overflows"));
            return out;
        };
        for slot in 0..unit_table::SLOTS {
            let head = match self.ptr(base, slot * unit_table::SLOT_SIZE) {
                Ok(head) => head,
                Err(e) => {
                    // The table is one contiguous block: if one slot fails, the rest will too.
                    self.warn(format!("unit table type {ty} slot {slot}: {e:#}"));
                    break;
                }
            };
            let what = format!("unit type {ty}");
            self.walk_into(head, unit::NEXT, MAX_UNITS_PER_TYPE, &what, &mut seen, &mut out);
        }
        out
    }

    fn unit_head(&self, addr: u64, ty: u32) -> Result<UnitHead> {
        let actual = self.u32(addr, unit::TYPE)?;
        if actual != ty {
            bail!("unit type is {actual}, expected {ty}");
        }
        Ok(UnitHead {
            txt: self.u32(addr, unit::TXT)?,
            id: self.u32(addr, unit::ID)?,
            mode: self.u32(addr, unit::MODE)?,
            unit_data: self.ptr(addr, unit::UNIT_DATA)?,
            act: self.ptr(addr, unit::ACT)?,
            path: self.ptr(addr, unit::PATH)?,
        })
    }

    fn pos(&self, path: u64) -> Result<(u16, u16)> {
        Ok((self.u16(path, path::X)?, self.u16(path, path::Y)?))
    }

    fn player_name(&self, unit_data: Option<u64>) -> Result<String> {
        let Some(pd) = unit_data else {
            return Ok(String::new());
        };
        let raw = self.bytes(pd, player_data::NAME, player_data::NAME_LEN)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Ok(String::from_utf8_lossy(raw.get(..end).unwrap_or_default()).into_owned())
    }

    /// The player's live shared-stash tabs in the game's UI order: the tab list of the player
    /// unit's own inventory ([`Reader::tab_list`]), each node's unit id looked up among the
    /// tab units (other player-type units **without an act** whose inventory names `player`
    /// as owner, [`inventory::OWNER_ID`]; research note §3.1/§3.3). Normal pages are numbered
    /// 1-based in list order; the node's [`stash_tab_node::ADVANCED`] byte marks the
    /// gems/runes/materials tab. A node whose unit isn't (yet) in the table is an empty tab, so
    /// the page numbers never shift. If the list is unreadable or empty while tab units exist,
    /// the tab units are used in unit-table order (pages only) with a warning.
    fn shared_tabs(&mut self, unit_table: u64, player_addr: u64, player: u32) -> Vec<TabRef> {
        let units: Vec<(u32, u64)> = self
            .units(unit_table, unit_table::PLAYER)
            .into_iter()
            .filter_map(|addr| {
                // The act check is hardening: today nothing else player-type names the player
                // as inventory owner, but a phantom tab here would shift every page.
                let id = self.u32(addr, unit::ID).ok()?;
                let inv = self.ptr(addr, unit::INVENTORY).ok()??;
                (id != player
                    && self.ptr(addr, unit::ACT).ok()?.is_none()
                    && self.u32(inv, inventory::OWNER_ID).ok()? == player)
                    .then_some((id, addr))
            })
            .collect();
        let nodes = match self.tab_list(player_addr) {
            Ok(nodes) if !nodes.is_empty() => nodes,
            listed => {
                if !units.is_empty() {
                    let why = listed
                        .err()
                        .map_or_else(|| "empty".to_owned(), |e| format!("{e:#}"));
                    self.warn(format!(
                        "shared stash: tab list unreadable ({why}); pages in memory order, \
                         no materials tab"
                    ));
                }
                return units
                    .into_iter()
                    .enumerate()
                    .map(|(i, (id, addr))| TabRef {
                        id,
                        addr: Some(addr),
                        kind: stash::TabKind::Page(u8::try_from(i + 1).unwrap_or(u8::MAX)),
                    })
                    .collect();
            }
        };
        let mut page = 0u8;
        nodes
            .into_iter()
            .map(|(id, advanced)| {
                let kind = if advanced {
                    stash::TabKind::Materials
                } else {
                    page = page.saturating_add(1);
                    stash::TabKind::Page(page)
                };
                let addr = units.iter().find(|&&(u, _)| u == id).map(|&(_, a)| a);
                TabRef { id, addr, kind }
            })
            .collect()
    }

    /// The `(tab unit id, advanced)` nodes of the shared-stash tab list in the inventory of the
    /// player unit at `player_addr`, list order: the first [`inventory::TAB_LIST`] header whose
    /// walk checks out ([`Reader::tab_nodes`]); empty when none does (the stash not loaded, or
    /// the layout moved again).
    fn tab_list(&self, player_addr: u64) -> Result<Vec<(u32, bool)>> {
        let inv = self.ptr_req(player_addr, unit::INVENTORY, "player inventory")?;
        let Some(nodes) = inventory::TAB_LIST
            .iter()
            .find_map(|&header| self.tab_nodes(field(inv, header).ok()?))
        else {
            return Ok(Vec::new());
        };
        nodes
            .into_iter()
            .map(|node| {
                Ok((
                    self.u32(node, stash_tab_node::TAB_ID)?,
                    self.u8(node, stash_tab_node::ADVANCED)? == 1,
                ))
            })
            .collect()
    }

    /// The node addresses of the tab list whose header is at `header`, if it is one: a count
    /// of 1..=[`MAX_STASH_TABS`], and the walk from the first node reaches the last node in
    /// exactly that many steps and ends there. Silent: a candidate header that isn't one is
    /// expected, not news.
    fn tab_nodes(&self, header: u64) -> Option<Vec<u64>> {
        let first = self.ptr(header, 0).ok()??;
        let last = self.ptr(header, inventory::TAB_LIST_LAST).ok()??;
        let count = usize::try_from(self.u32(header, inventory::TAB_LIST_COUNT).ok()?).ok()?;
        if !(1..=MAX_STASH_TABS).contains(&count) {
            return None;
        }
        let mut nodes = vec![first];
        let mut cur = first;
        while nodes.len() < count {
            cur = self.ptr(cur, stash_tab_node::NEXT).ok()??;
            if nodes.contains(&cur) {
                return None;
            }
            nodes.push(cur);
        }
        let ends = self.ptr(cur, stash_tab_node::NEXT).ok()?.is_none();
        (cur == last && ends).then_some(nodes)
    }

    /// The shared-stash `tabs` with their items, read through each tab unit's inventory chain
    /// ([`inventory::FIRST_ITEM`], then [`item_data::NEXT_IN_INVENTORY`]); the materials tab's
    /// items also get their stack size ([`item_data::QUANTITY`]). Unreadable items are skipped
    /// without a warning (optional data); a broken or runaway chain warns once per tab
    /// ([`MAX_STASH_ITEMS_PER_TAB`]).
    fn stash(&mut self, tabs: &[TabRef], items_table: Option<u64>) -> Vec<StashTab> {
        tabs.iter()
            .map(|tab| {
                let materials = tab.kind == stash::TabKind::Materials;
                let mut items = Vec::new();
                for addr in tab.addr.map_or_default(|a| self.tab_items(a)) {
                    let Ok(head) = self.unit_head(addr, unit_table::ITEM) else {
                        continue;
                    };
                    let Ok(item) = self.item_fields(addr, &head, items_table) else {
                        continue;
                    };
                    let quantity = head
                        .unit_data
                        .filter(|_| materials)
                        .and_then(|d| self.u32(d, item_data::QUANTITY).ok());
                    items.push(stash::stash_item(&item, quantity));
                }
                StashTab {
                    kind: tab.kind,
                    items,
                }
            })
            .collect()
    }

    /// The item unit addresses of the tab unit at `tab`, in its inventory's chain order.
    fn tab_items(&mut self, tab: u64) -> Vec<u64> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let first = self
            .ptr(tab, unit::INVENTORY)
            .ok()
            .flatten()
            .and_then(|inv| self.ptr(inv, inventory::FIRST_ITEM).ok().flatten());
        let mut cur = first;
        while let Some(item) = cur {
            if !seen.insert(item) {
                self.warn(format!("stash tab {tab:#x}: item chain cycles at {item:#x}"));
                break;
            }
            if out.len() >= MAX_STASH_ITEMS_PER_TAB {
                self.warn(format!(
                    "stash tab {tab:#x}: over {MAX_STASH_ITEMS_PER_TAB} items, the rest are \
                     skipped (a runaway chain means a misread)"
                ));
                break;
            }
            out.push(item);
            // The next link lives in the item's `ItemData`, not in the unit itself.
            cur = match self
                .ptr_req(item, unit::UNIT_DATA, "item data")
                .and_then(|d| self.ptr(d, item_data::NEXT_IN_INVENTORY))
            {
                Ok(next) => next,
                Err(e) => {
                    self.warn(format!(
                        "stash tab {tab:#x}: item chain broken after {item:#x}: {e:#}"
                    ));
                    break;
                }
            };
        }
        out
    }

    /// `Some` if the unit is a live player: non-null act and path, non-zero position.
    fn player_candidate(&self, addr: u64) -> Result<Option<Candidate>> {
        let head = self.unit_head(addr, unit_table::PLAYER)?;
        let (Some(_), Some(path)) = (head.act, head.path) else {
            return Ok(None);
        };
        let (x, y) = self.pos(path)?;
        if x == 0 || y == 0 {
            return Ok(None);
        }
        Ok(Some(Candidate {
            addr: Addr(addr),
            id: head.id,
            name: self.player_name(head.unit_data)?,
        }))
    }

    /// The player and its level's address (path → room1 → room2 → level).
    fn player(&self, addr: u64) -> Result<(Player, u64)> {
        let head = self.unit_head(addr, unit_table::PLAYER)?;
        let path = head.path.context("player has no path")?;
        let (x, y) = self.pos(path)?;
        let room1 = self.ptr_req(path, path::ROOM1, "path room1")?;
        let room2 = self.ptr_req(room1, room1::ROOM2, "room1 room2")?;
        let level = self.ptr_req(room2, room2::LEVEL, "room2 level")?;
        let player = Player {
            addr: Addr(addr),
            id: head.id,
            name: self.player_name(head.unit_data)?,
            class: head.txt,
            x,
            y,
            level_id: self.u32(level, level::ID)?,
            act: head.act.map(Addr),
            // Optional data like `room_preset`: an unreadable stat list is no level, not an error.
            char_level: self
                .stat(addr, stat_list::LEVEL)
                .ok()
                .flatten()
                .and_then(char_level_from_raw),
            // Gear speed/resist stats are summed from the equipped items in `snapshot` (the
            // player's stat list is base-only, no item stats).
            speed: None,
            build: Vec::new(),
            bis: None,
            wsm: None,
        };
        Ok((player, level))
    }

    /// The player's skill allocation ([`Skills`]). A read error is a warning: an unreadable list
    /// leaves the allocation empty, an unreadable entry is skipped, so a bad skill list never
    /// fails the snapshot.
    fn skills(&mut self, player_addr: u64, class: u32) -> Skills {
        let header = match self.ptr(player_addr, unit::SKILLS) {
            Ok(Some(header)) => header,
            Ok(None) => return Skills::default(),
            Err(e) => {
                self.warn(format!("skill list: {e:#}"));
                return Skills::default();
            }
        };
        let first = match self.ptr(header, skills::FIRST) {
            Ok(first) => first,
            Err(e) => {
                self.warn(format!("skill list: {e:#}"));
                return Skills::default();
            }
        };
        let mut points = Vec::new();
        for node in self.walk(first, skill::NEXT, MAX_SKILLS, "skill") {
            match self.skill_points(node) {
                // Basic skills (Attack, scrolls) and gear-granted skills of other classes are
                // in the list too; only the class's own allocated skills count.
                Ok((id, hard)) => {
                    let own = skills_data::skill(id).is_some_and(|s| u32::from(s.class) == class);
                    if hard > 0 && own {
                        points.push((id, hard));
                    }
                }
                // One unreadable entry is skipped; the rest of the allocation still shows.
                Err(e) => self.warn(format!("skill {node:#x}: {e:#}")),
            }
        }
        // The buttons are a nice-to-have: an unreadable one is just not marked.
        let button = |off| {
            self.ptr(header, off)
                .ok()
                .flatten()
                .and_then(|node| self.skill_id(node).ok())
        };
        Skills {
            points,
            left: button(skills::LEFT),
            right: button(skills::RIGHT),
        }
    }

    /// The skills.txt id of a skill node.
    fn skill_id(&self, node: u64) -> Result<u16> {
        let txt = self.ptr_req(node, skill::TXT, "skill txt")?;
        self.u16(txt, skill_txt::ID)
    }

    /// `(id, hard points)` of a skill node; more than 255 points is a misread.
    fn skill_points(&self, node: u64) -> Result<(u16, u8)> {
        let id = self.skill_id(node)?;
        let hard = self.u32(node, skill::HARD_POINTS)?;
        let hard = u8::try_from(hard).with_context(|| format!("skill {id}: {hard} hard points"))?;
        Ok((id, hard))
    }

    /// World-tile origin of a `Room2`.
    fn room_pos(&self, room: u64) -> Result<RoomPos> {
        Ok(RoomPos {
            x: world(self.u32(room, room2::X)?)?,
            y: world(self.u32(room, room2::Y)?)?,
        })
    }

    fn room(&self, room: u64) -> Result<Room> {
        let pos = self.room_pos(room)?;
        Ok(Room {
            x: pos.x,
            y: pos.y,
            w: world(self.u32(room, room2::W)?)?,
            h: world(self.u32(room, room2::H)?)?,
            loaded: self.ptr(room, room2::ROOM1)?.is_some(),
            preset: self.room_preset(room),
        })
    }

    /// The room's level-preset id. Optional data: a null or unreadable record is `None`, not a
    /// warning (it would repeat for every room).
    fn room_preset(&self, room: u64) -> Option<u32> {
        let record = self.ptr(room, room2::LEVEL_PRESET).ok().flatten()?;
        self.u32(record, level_preset::ID).ok()
    }

    /// The game's difficulty via act → `ActMisc`. Optional data like [`Reader::room_preset`]: an
    /// unreadable chain is `None`, not a warning.
    fn difficulty(&self, act_addr: u64) -> Option<u8> {
        let misc = self.ptr(act_addr, act::MISC).ok().flatten()?;
        difficulty_from_raw(self.u16(misc, act_misc::DIFFICULTY).ok()?)
    }

    fn presets(&mut self, room: u64, pos: RoomPos, out: &mut Vec<Preset>) {
        let head = match self.ptr(room, room2::PRESETS) {
            Ok(head) => head,
            Err(e) => return self.warn(format!("room {room:#x} presets: {e:#}")),
        };
        for p in self.walk(head, preset::NEXT, MAX_PRESETS_PER_ROOM, "preset") {
            match self.preset(p, pos) {
                Ok(preset) => out.push(preset),
                Err(e) => self.warn(format!("preset {p:#x}: {e:#}")),
            }
        }
    }

    fn preset(&self, p: u64, room: RoomPos) -> Result<Preset> {
        let rel_x = self.u32(p, preset::X)?;
        let rel_y = self.u32(p, preset::Y)?;
        Ok(Preset {
            kind: PresetKind::from_raw(self.u32(p, preset::TYPE)?),
            txt: self.u32(p, preset::TXT)?,
            x: room.x.checked_add(rel_x).context("preset x overflow")?,
            y: room.y.checked_add(rel_y).context("preset y overflow")?,
            entry: false,
        })
    }

    fn room_links(&mut self, room: u64, from_room: RoomPos, out: &mut Vec<RoomLink>) {
        let head = match self.ptr(room, room2::ROOM_TILES) {
            Ok(head) => head,
            Err(e) => return self.warn(format!("room {room:#x} room tiles: {e:#}")),
        };
        for t in self.walk(head, room_tile::NEXT, MAX_LINKS_PER_ROOM, "room tile") {
            match self.room_link(t, from_room) {
                Ok(link) => out.push(link),
                Err(e) => self.warn(format!("room tile {t:#x}: {e:#}")),
            }
        }
    }

    fn room_link(&self, tile: u64, from_room: RoomPos) -> Result<RoomLink> {
        let to = self.ptr_req(tile, room_tile::ROOM2, "linked room2")?;
        let to_level = self.ptr_req(to, room2::LEVEL, "linked room2 level")?;
        Ok(RoomLink {
            from_room,
            to_level_id: self.u32(to_level, level::ID)?,
            to_room: self.room_pos(to)?,
        })
    }

    /// Records every near room (`room2::NEAR`) of `room` that belongs to another level than
    /// `level_addr`, skipping links already in `out`. Unreadable arrays and entries become
    /// warnings; a null array is no near rooms.
    fn border_links(&mut self, room: u64, level_addr: u64, from: &Room, out: &mut Vec<BorderLink>) {
        let from_room = RoomRect {
            x: from.x,
            y: from.y,
            w: from.w,
            h: from.h,
        };
        let array = match self.ptr(room, room2::NEAR) {
            Ok(Some(a)) => a,
            Ok(None) => return,
            Err(e) => return self.warn(format!("room {room:#x} near rooms: {e:#}")),
        };
        let count = match self.u32(room, room2::NEAR_COUNT) {
            Ok(n) => n,
            Err(e) => return self.warn(format!("room {room:#x} near count: {e:#}")),
        };
        if count > MAX_NEAR_ROOMS {
            self.warn(format!(
                "room {room:#x}: {count} near rooms, reading the first {MAX_NEAR_ROOMS}"
            ));
        }
        let len = usize::try_from(count.min(MAX_NEAR_ROOMS)).unwrap_or(0) * 8;
        let bytes = match self.bytes(array, 0, len) {
            Ok(b) => b,
            Err(e) => return self.warn(format!("room {room:#x} near rooms: {e:#}")),
        };
        for &raw in bytes.as_chunks::<8>().0 {
            let near = u64::from_le_bytes(raw);
            // Nulls (and garbage) after the real entries are skipped, like `ReadMem::read_ptr`.
            if !crate::process::is_user_ptr(near) {
                continue;
            }
            match self.border_link(near, level_addr, from_room) {
                // Many rooms share a neighbour pair's list entries; a level has a few hundred
                // links at most, so a linear check is cheap.
                Ok(Some(link)) if !out.contains(&link) => out.push(link),
                Ok(_) => {}
                Err(e) => self.warn(format!("near room {near:#x}: {e:#}")),
            }
        }
    }

    /// The link to near room `near`, or `None` if it is in this level.
    fn border_link(&self, near: u64, level_addr: u64, from_room: RoomRect) -> Result<Option<BorderLink>> {
        let to_level = self.ptr_req(near, room2::LEVEL, "near room level")?;
        if to_level == level_addr {
            return Ok(None);
        }
        let pos = self.room_pos(near)?;
        Ok(Some(BorderLink {
            from_room,
            to_level_id: self.u32(to_level, level::ID)?,
            to_room: RoomRect {
                x: pos.x,
                y: pos.y,
                w: world(self.u32(near, room2::W)?)?,
                h: world(self.u32(near, room2::H)?)?,
            },
            entry: false,
        }))
    }

    fn monster(&self, addr: u64) -> Result<Monster> {
        let head = self.unit_head(addr, unit_table::MONSTER)?;
        let (x, y) = self.pos(head.path.context("monster has no path")?)?;
        let md = head.unit_data.context("monster has no unit data")?;
        Ok(Monster {
            addr: Addr(addr),
            id: head.id,
            txt: head.txt,
            mode: head.mode,
            x,
            y,
            flags: self.u8(md, monster_data::FLAGS)?,
            name_seed: self.u16(md, monster_data::NAME_SEED)?,
            owner_id: self.u32(md, monster_data::OWNER_ID).unwrap_or(0),
            owner_type: self.u32(md, monster_data::OWNER_TYPE).unwrap_or(0),
            desecrated: self.has_state(addr, state::DESECRATED),
        })
    }

    /// True if the unit at `addr` has `state` set in its stat list's state bitfield. Optional
    /// data like [`Reader::room_preset`]: no stat list or an unreadable bitfield is no state, not
    /// a warning (it would repeat for every monster).
    fn has_state(&self, addr: u64, state: u32) -> bool {
        let Ok(Some(list)) = self.ptr(addr, unit::STATS) else {
            return false;
        };
        self.bytes(list, stat_list::STATE_FLAGS, stat_list::STATE_FLAG_DWORDS * 4)
            .is_ok_and(|flags| has_state(&flags, state))
    }

    /// The shrines, wells and operated consumable quest objects among the loaded object units;
    /// other objects are skipped unread.
    fn objects(&mut self, unit_table: u64) -> (Vec<Shrine>, Vec<Well>, Vec<OperatedObject>) {
        let mut shrines = Vec::new();
        let mut wells = Vec::new();
        let mut operated = Vec::new();
        for addr in self.units(unit_table, unit_table::OBJECT) {
            match self.object(addr) {
                Ok(Some(Object::Shrine(s))) => shrines.push(s),
                Ok(Some(Object::Well(w))) => wells.push(w),
                Ok(Some(Object::Operated(o))) => operated.push(o),
                Ok(None) => {}
                Err(e) => self.warn(format!("object {addr:#x}: {e:#}")),
            }
        }
        (shrines, wells, operated)
    }

    /// The object unit at `addr` as a shrine, well or operated consumable quest object; `None`
    /// for any other object, a shrine object without a shrine type in [`SHRINE_TYPES`], and an
    /// untouched consumable quest object.
    fn object(&self, addr: u64) -> Result<Option<Object>> {
        let head = self.unit_head(addr, unit_table::OBJECT)?;
        if is_consumable_quest_object(head.txt) {
            if head.mode == object_mode::NEUTRAL {
                return Ok(None);
            }
            let path = head.path.context("object has no path")?;
            return Ok(Some(Object::Operated(OperatedObject {
                txt: head.txt,
                x: self.u32(path, static_path::X)?,
                y: self.u32(path, static_path::Y)?,
            })));
        }
        let shrine = is_shrine_object(head.txt);
        if !shrine && !is_well_object(head.txt) {
            return Ok(None);
        }
        let shrine_type = if shrine {
            let data = head.unit_data.context("shrine has no object data")?;
            let t = self.u8(data, object_data::SHRINE_TYPE)?;
            if !SHRINE_TYPES.contains(&t) {
                return Ok(None);
            }
            Some(t)
        } else {
            None
        };
        // Objects have a static path, like items.
        let path = head.path.context("object has no path")?;
        let (x, y) = (self.u32(path, static_path::X)?, self.u32(path, static_path::Y)?);
        let txt = head.txt;
        Ok(Some(shrine_type.map_or(
            Object::Well(Well { txt, x, y }),
            |code| {
                Object::Shrine(Shrine {
                    txt,
                    code,
                    used: head.mode != object_mode::NEUTRAL,
                    x,
                    y,
                })
            },
        )))
    }

    /// The items on the ground, those worth showing, the items in the inventory (all of them,
    /// position 0,0; the keep list picks from them), and the owner votes of the stash and
    /// inventory items. Only owners among the player `candidates` (unit ids) vote: the shared
    /// stash tabs' pseudo-owners must never win the stash vote.
    fn items(&mut self, unit_table: u64, items_table: Option<u64>, candidates: &[u32]) -> ItemScan {
        let mut scan = ItemScan::default();
        for addr in self.units(unit_table, unit_table::ITEM) {
            let head = match self.unit_head(addr, unit_table::ITEM) {
                Ok(head) => head,
                Err(e) => {
                    self.warn(format!("item {addr:#x}: {e:#}"));
                    continue;
                }
            };
            // Stash, cube, equipped, cursor and belt items are units too: the ground and the
            // inventory count, the stash and inventory vote on the owner, and the inventory,
            // cube and stash give the spare lists.
            let page = if head.mode == item_mode::STORED {
                self.page(&head)
            } else {
                None
            };
            // An unreadable owner just doesn't vote, and its item can't be listed as spare.
            let owner = match page {
                Some(item_data::PAGE_INVENTORY | item_data::PAGE_CUBE | item_data::PAGE_STASH) => {
                    head.unit_data.and_then(|d| self.u32(d, item_data::OWNER).ok())
                }
                _ => None,
            };
            let votes = match page {
                Some(item_data::PAGE_STASH) => Some(&mut scan.stash_owners),
                Some(item_data::PAGE_INVENTORY) => Some(&mut scan.inventory_owners),
                _ => None,
            };
            if let (Some(votes), Some(owner)) = (votes.filter(|v| !v.done()), owner)
                && candidates.contains(&owner)
            {
                votes.add(owner);
            }
            // Cube and stash: only the may-be-spare items are read in full.
            if let (Some(p @ (item_data::PAGE_CUBE | item_data::PAGE_STASH)), Some(owner)) = (page, owner)
                && self.may_spare(&head)
            {
                match self.item_fields(addr, &head, items_table) {
                    Ok(item) => scan.stored.push(Stored { item, page: p, owner }),
                    Err(e) => self.warn(format!("item {addr:#x}: {e:#}")),
                }
            }
            let found = match head.mode {
                item_mode::ON_GROUND => {
                    scan.ground = scan.ground.saturating_add(1);
                    self.item_drop(addr, &head, items_table)
                        .map(|d| d.map(|d| (d, true)))
                }
                item_mode::STORED if page == Some(item_data::PAGE_INVENTORY) => self
                    .item_fields(addr, &head, items_table)
                    .map(|d| Some((d, false))),
                _ => Ok(None),
            };
            match found {
                Ok(Some((d, true))) => scan.drops.push(d),
                Ok(Some((d, false))) => scan.inventory.push((d, owner)),
                Ok(None) => {}
                Err(e) => self.warn(format!("item {addr:#x}: {e:#}")),
            }
            // The guide's speed/resist stats: equipped items and carried charms of a candidate
            // (the player's own stat list is base-only; item stats live on the items).
            if let Some(owner) = self.gear_owner(&head, page, items_table)
                && candidates.contains(&owner)
            {
                self.accumulate_gear(addr, &head, scan.gear.entry(owner).or_default());
                self.capture_weapon_wsm(&head, owner, items_table, &mut scan.weapons);
            }
            // Owned identified unique/set items: the `BiS` list marks owned pieces with a ✓.
            if let Some(owner) = head.unit_data.and_then(|d| self.u32(d, item_data::OWNER).ok())
                && candidates.contains(&owner)
            {
                let quality = self.equipped_quality(&head);
                if matches!(quality, Some(item_data::QUALITY_UNIQUE | item_data::QUALITY_SET)) {
                    let name = self
                        .item_fields(addr, &head, items_table)
                        .ok()
                        .and_then(|d| valuables::unique_set_name(&d).map(str::to_owned));
                    let names = scan.owned_names.entry(owner).or_default();
                    if let Some(name) = name
                        && !names.iter().any(|n| n.eq_ignore_ascii_case(&name))
                    {
                        names.push(name);
                    }
                }
            }
            // Equipped pieces by owner kind, for the `BiS` per-slot display.
            self.collect_equipped(&mut scan, &head, items_table, candidates);
        }
        scan
    }

    /// The equipped item of `head` into the `BiS` per-slot display lists: the
    /// `(body slot, quality)` pair of the player or the mercenary (the old list), and —
    /// for the player — the enriched [`Equipped`] for the dashboard's gear grid (code,
    /// runeword bit, unique/set name), one per body slot like the pairs.
    fn collect_equipped(
        &self,
        scan: &mut ItemScan,
        head: &UnitHead,
        items_table: Option<u64>,
        candidates: &[u32],
    ) {
        let Some(quality) = self.equipped_quality(head) else {
            return;
        };
        let Some(slot) = self.equipped_slot(head) else {
            return;
        };
        let owner = head.unit_data.and_then(|d| self.u32(d, item_data::OWNER).ok());
        let merc = owner == Some(item_data::NO_OWNER);
        let Some(owner) = owner.filter(|o| merc || candidates.contains(o)) else {
            return;
        };
        let list = if merc {
            &mut scan.merc_equipped
        } else {
            scan.equipped.entry(owner).or_default()
        };
        if !list.contains(&(slot, quality)) {
            list.push((slot, quality));
        }
        if !merc {
            let items = scan.equipped_items.entry(owner).or_default();
            if items.iter().all(|e| e.body != slot) {
                items.push(self.equipped_item(head, slot, quality, items_table));
            }
        }
    }

    /// The `item_data::BODY` slot of an equipped item (`None` when not equipped).
    fn head_body_slot(&self, head: &UnitHead) -> Option<u8> {
        head.unit_data
            .and_then(|d| self.u8(d, item_data::BODY).ok())
            .filter(|&b| b != 0 && b != 255)
    }

    /// The `item_data::BODY` slot of an equipped item (`None` when not equipped).
    fn equipped_slot(&self, head: &UnitHead) -> Option<u8> {
        let data = head.unit_data?;
        let body = self.u8(data, item_data::BODY).ok()?;
        (body != 0 && body != 255).then_some(body)
    }

    /// The `item_data::QUALITY` of an item (`None` if unreadable).
    fn equipped_quality(&self, head: &UnitHead) -> Option<u32> {
        head.unit_data.and_then(|d| self.u32(d, item_data::QUALITY).ok())
    }

    /// The enriched equipped item of `head` (body slot `slot`, quality `quality`) for the
    /// dashboard's gear grid: the item code from the items table, the `FLAGS` runeword bit
    /// and, for an identified unique/set, its display name and `FILE_INDEX` row. The
    /// individual reads are optional: a missing items table only leaves the code out.
    fn equipped_item(&self, head: &UnitHead, slot: u8, quality: u32, items_table: Option<u64>) -> Equipped {
        let flags = head
            .unit_data
            .and_then(|d| self.u32(d, item_data::FLAGS).ok())
            .unwrap_or(0);
        // `FILE_INDEX` is only filled once identified, so it is trusted (and kept) only
        // together with the name the identity path resolves from it.
        // The raw index: `*ID` 0 is a real row (The Gnasher), so no `!= 0` filter here —
        // `unique_set_name_parts` gates on FLAG_IDENTIFIED instead.
        let file_index = head
            .unit_data
            .and_then(|d| self.u32(d, item_data::FILE_INDEX).ok());
        let code = items_table
            .and_then(|t| items::read_code(self.mem, t, head.txt))
            .map(items::code_text);
        let name = valuables::unique_set_name_parts(quality, flags, file_index).map(str::to_owned);
        let named_index = if name.is_some() { file_index } else { None };
        Equipped {
            body: slot,
            quality,
            code,
            name,
            file_index: named_index,
            runeword: flags & item_data::FLAG_RUNEWORD != 0,
        }
    }

    /// The owner whose gear stats `item` contributes to: the player's equipped items (body slot)
    /// and carried charms (inventory page, charm codes), by `item_data::OWNER`.
    fn gear_owner(&self, head: &UnitHead, page: Option<u8>, items_table: Option<u64>) -> Option<u32> {
        let data = head.unit_data?;
        let owner = self.u32(data, item_data::OWNER).ok()?;
        let body = self.u8(data, item_data::BODY).ok()?;
        if body != 0 && body != 255 {
            return Some(owner);
        }
        if page == Some(item_data::PAGE_INVENTORY) {
            let code = items::read_code(self.mem, items_table?, head.txt)?;
            let code = items::code_text(code);
            if matches!(code.as_str(), "cm1" | "cm2" | "cm3") {
                return Some(owner);
            }
        }
        None
    }

    /// The equipped weapon's WSM per owner (the guide's IAS note): the weapon is the body-4
    /// item of `owner`, its code looked up in the items table, the WSM in `weapons_data`.
    fn capture_weapon_wsm(
        &self,
        head: &UnitHead,
        owner: u32,
        items_table: Option<u64>,
        out: &mut HashMap<u32, i32>,
    ) {
        let already = out.contains_key(&owner);
        let owned_by_player = head
            .unit_data
            .and_then(|d| self.u32(d, item_data::OWNER).ok())
            .is_some_and(|o| o == owner);
        if self.head_body_slot(head) != Some(4) || already || !owned_by_player {
            return;
        }
        let Some(code) = items_table.and_then(|t| items::read_code(self.mem, t, head.txt)) else {
            return;
        };
        if let Some(wsm) = crate::weapons_data::wsm(&items::code_text(code)) {
            out.insert(owner, wsm);
        }
    }

    /// Adds the speed/resist stats of `item`'s stat-list chain (base list at `unit::STATS`, then
    /// the affix lists chained by `stat_list::NEXT` while they still belong to the item) to
    /// `acc`. The chain is verified live (2026-11-17): an equipped armour's affix list carried
    /// its res; lists of other owners end the walk.
    fn accumulate_gear(&self, item: u64, head: &UnitHead, acc: &mut SpeedStats) {
        let Some(mut list) = self.ptr(item, unit::STATS).unwrap_or(None) else {
            return;
        };
        for _ in 0..8 {
            let (Some(ty), Some(owner)) = (self.u32(list, 0x08).ok(), self.u32(list, 0x0C).ok()) else {
                return;
            };
            if ty != unit_table::ITEM || owner != head.id {
                return;
            }
            for (_, id, value) in self.stat_entries(list).unwrap_or_default() {
                let value = plain_stat_from_raw(value);
                match id {
                    stat_list::RESIST_FIRE => acc.res_fire = acc.res_fire.saturating_add(value),
                    stat_list::RESIST_COLD => acc.res_cold = acc.res_cold.saturating_add(value),
                    stat_list::RESIST_LIGHTNING => {
                        acc.res_lightning = acc.res_lightning.saturating_add(value);
                    }
                    stat_list::RESIST_POISON => acc.res_poison = acc.res_poison.saturating_add(value),
                    stat_list::IAS => acc.ias = acc.ias.saturating_add(value),
                    stat_list::FHR => acc.fhr = acc.fhr.saturating_add(value),
                    stat_list::FCR => acc.fcr = acc.fcr.saturating_add(value),
                    _ => {}
                }
            }
            let Some(next) = self.u64(list, stat_list::NEXT).ok() else {
                return;
            };
            if next == 0 {
                return;
            }
            list = next;
        }
    }

    /// All `{layer, stat id, value}` entries of the stat list at `list` (a `StatList` address,
    /// not a unit: the entries sit at `stat_list::ENTRIES`, the count at `stat_list::COUNT`).
    fn stat_entries(&self, list: u64) -> Result<Vec<(u16, u16, u32)>> {
        let count = self.u64(list, stat_list::COUNT)?;
        if count > MAX_ITEM_STATS {
            bail!("stat list {list:#x}: {count} entries");
        }
        let entries = self.ptr_req(list, stat_list::ENTRIES, "stat entries")?;
        let len = usize::try_from(count * stat_list::STAT_SIZE)?;
        let bytes = self
            .mem
            .read_bytes(entries, len)
            .with_context(|| format!("reading {count} stats at {entries:#x}"))?;
        Ok(bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&[l0, l1, i0, i1, v0, v1, v2, v3]| {
                (
                    u16::from_le_bytes([l0, l1]),
                    u16::from_le_bytes([i0, i1]),
                    u32::from_le_bytes([v0, v1, v2, v3]),
                )
            })
            .collect())
    }

    /// True if the stored item `head` may be spare ([`valuables::may_spare`]); false if its
    /// quality or flags are unreadable.
    fn may_spare(&self, head: &UnitHead) -> bool {
        head.unit_data.is_some_and(|d| {
            matches!(
                (self.u32(d, item_data::QUALITY), self.u32(d, item_data::FLAGS)),
                (Ok(q), Ok(f)) if valuables::may_spare(q, f)
            )
        })
    }

    /// Storage page of a stored item (`item_data::PAGE`); `None` if unreadable.
    fn page(&self, head: &UnitHead) -> Option<u8> {
        head.unit_data
            .and_then(|data| self.u8(data, item_data::PAGE).ok())
    }

    /// The fields of item `head` (unit at `addr`) the valuables rules look at; no position.
    fn item_fields(&self, addr: u64, head: &UnitHead, items_table: Option<u64>) -> Result<ItemDrop> {
        let data = head.unit_data.context("item has no unit data")?;
        let quality = self.u32(data, item_data::QUALITY)?;
        let flags = self.u32(data, item_data::FLAGS)?;
        let code = items_table.and_then(|t| items::read_code(self.mem, t, head.txt));
        let unique_index = [item_data::QUALITY_UNIQUE, item_data::QUALITY_SET]
            .contains(&quality)
            .then(|| self.u32(data, item_data::FILE_INDEX).ok())
            .flatten();
        Ok(ItemDrop {
            id: head.id,
            code: code.map(items::code_text),
            kind: items::classify(quality, code).unwrap_or(DropKind::Base),
            quality,
            flags,
            ethereal: flags & item_data::FLAG_ETHEREAL != 0,
            // Optional data like `room_preset`: an unreadable stat list counts as no sockets.
            sockets: self.sockets(addr).unwrap_or(0),
            unique_index,
            item_data_head: String::new(),
            highlight: None,
            x: 0,
            y: 0,
        })
    }

    /// The ground item `head` (unit at `addr`) as a drop, or `None` if it isn't one worth
    /// showing: not a unique, set, gem, rune or charm, and no valuables rule of any tier matches
    /// it. Which tiers are highlighted is decided later ([`valuables::apply`]).
    fn item_drop(&self, addr: u64, head: &UnitHead, items_table: Option<u64>) -> Result<Option<ItemDrop>> {
        let mut drop = self.item_fields(addr, head, items_table)?;
        if drop.kind == DropKind::Base && !valuables::is_candidate(&drop) {
            return Ok(None);
        }
        // Items have a static path: u32 position at other offsets than a moving unit's path.
        let path = head.path.context("ground item has no path")?;
        drop.x = self.u32(path, static_path::X)?;
        drop.y = self.u32(path, static_path::Y)?;
        let data = head.unit_data.context("item has no unit data")?;
        drop.item_data_head = self
            .mem
            .read_bytes(data, item_data::HEAD_LEN)
            .map_or_else(|_| String::new(), |b| hex(&b));
        Ok(Some(drop))
    }

    /// The item's socket count: stat [`stat_list::NUM_SOCKETS`] of its stat list, 0 without one.
    fn sockets(&self, unit_addr: u64) -> Result<u8> {
        let value = self.stat(unit_addr, stat_list::NUM_SOCKETS)?.unwrap_or(0);
        Ok(u8::try_from(value).unwrap_or(u8::MAX))
    }

    /// The value of stat `id` in the unit's stat list (first entry with that id, any layer);
    /// `None` without a stat list or without the stat.
    fn stat(&self, unit_addr: u64, id: u16) -> Result<Option<u32>> {
        let Some(list) = self.ptr(unit_addr, unit::STATS)? else {
            return Ok(None);
        };
        let count = self.u64(list, stat_list::COUNT)?;
        if count == 0 {
            return Ok(None);
        }
        if count > MAX_ITEM_STATS {
            bail!("stat list {list:#x}: {count} entries");
        }
        let entries = self.ptr_req(list, stat_list::ENTRIES, "stat entries")?;
        // count <= MAX_ITEM_STATS, so the byte length fits any usize.
        let len = usize::try_from(count * stat_list::STAT_SIZE)?;
        let bytes = self
            .mem
            .read_bytes(entries, len)
            .with_context(|| format!("reading {count} stats at {entries:#x}"))?;
        let (stats, _) = bytes.as_chunks::<8>();
        Ok(stats.iter().find_map(|&[_, _, id0, id1, v0, v1, v2, v3]| {
            (u16::from_le_bytes([id0, id1]) == id).then(|| u32::from_le_bytes([v0, v1, v2, v3]))
        }))
    }
}

/// What [`Reader::object`] found.
enum Object {
    Shrine(Shrine),
    Well(Well),
    Operated(OperatedObject),
}

/// Lowercase hex of `bytes`, no separators.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            // Writing to a String can't fail.
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// `base + off`, failing on overflow.
fn field(base: u64, off: u64) -> Result<u64> {
    base.checked_add(off)
        .with_context(|| format!("address overflow: {base:#x}+{off:#x}"))
}

/// Room units to world tiles.
fn world(room_units: u32) -> Result<u32> {
    room_units
        .checked_mul(ROOM_TILE_SCALE)
        .with_context(|| format!("room coordinate {room_units} overflows world tiles"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::testutil::FakeMem;

    // Synthetic world addresses (all distinct FakeMem regions).
    const TABLE: u64 = 0x10_0000;
    const P_STALE: u64 = 0x20_0000;
    const P_ALICE: u64 = 0x20_1000;
    const P_BOB: u64 = 0x20_2000;
    const PD_STALE: u64 = 0x21_0000;
    const PD_ALICE: u64 = 0x21_1000;
    const PD_BOB: u64 = 0x21_2000;
    const ACT: u64 = 0x22_0000;
    const LEVEL: u64 = 0x30_0000;
    const R0: u64 = 0x31_0000;
    const R1: u64 = 0x31_1000;
    const R2: u64 = 0x31_2000;
    const ROOM1_0: u64 = 0x32_0000;
    const PATH_ALICE: u64 = 0x40_0000;
    const PATH_BOB: u64 = 0x40_1000;
    const PATH_M0: u64 = 0x40_2000;
    const PATH_M1: u64 = 0x40_3000;
    const PATH_M2: u64 = 0x40_4000;
    const PR_OBJ: u64 = 0x45_0000;
    const PR_TILE: u64 = 0x45_1000;
    const PR_ODD: u64 = 0x45_2000;
    const PR_MON: u64 = 0x45_3000;
    const PR_DUP0: u64 = 0x45_4000;
    const PR_DUP1: u64 = 0x45_5000;
    const PR_DUP2: u64 = 0x45_6000;
    const RT0: u64 = 0x50_0000;
    const RT_BAD: u64 = 0x50_1000;
    const R_OTHER: u64 = 0x60_0000;
    const L_OTHER: u64 = 0x61_0000;
    const M0: u64 = 0x70_0000;
    const M1: u64 = 0x70_1000;
    const M2: u64 = 0x70_2000;
    const MD0: u64 = 0x71_0000;
    const MD1: u64 = 0x71_1000;
    const I0: u64 = 0x80_0000;
    const ID0: u64 = 0x81_0000;
    /// Plausible but unmapped.
    const UNMAPPED: u64 = 0xdead_0000;

    const UNIT_SIZE: usize = 0x160;

    /// Writes a zeroed struct of `size` bytes at `base` with the given little-endian fields.
    fn put(mem: &mut FakeMem, base: u64, size: usize, fields: &[(u64, &[u8])]) -> Result<()> {
        let mut b = vec![0u8; size];
        for &(off, val) in fields {
            let off = usize::try_from(off)?;
            b.get_mut(off..off + val.len())
                .context("field outside struct")?
                .copy_from_slice(val);
        }
        mem.add(base, b);
        Ok(())
    }

    /// Unit-table entries: (unit type, slot, chain head).
    const BASE_SLOTS: [(u32, u64, u64); 4] = [
        (unit_table::PLAYER, 3, P_STALE),
        (unit_table::PLAYER, 9, P_BOB),
        (unit_table::MONSTER, 0, M0),
        (unit_table::ITEM, 2, I0),
    ];

    fn table(mem: &mut FakeMem, slots: &[(u32, u64, u64)]) -> Result<()> {
        let fields: Vec<(u64, [u8; 8])> = slots
            .iter()
            .map(|&(ty, slot, head)| {
                let off = (u64::from(ty) * unit_table::SLOTS + slot) * unit_table::SLOT_SIZE;
                (off, head.to_le_bytes())
            })
            .collect();
        let fields: Vec<(u64, &[u8])> = fields.iter().map(|(o, b)| (*o, b.as_slice())).collect();
        put(mem, TABLE, 5 * 128 * 8, &fields)
    }

    #[allow(clippy::too_many_arguments)] // reason: test fixture mirroring the unit fields
    fn unit(
        mem: &mut FakeMem,
        addr: u64,
        ty: u32,
        txt: u32,
        data: u64,
        act: u64,
        path: u64,
        next: u64,
    ) -> Result<()> {
        unit_ext(mem, addr, (ty, txt, data, act, path, next), &[])
    }

    /// Like [`unit`] (fields `(ty, txt, data, act, path, next)`), plus `extra` fields.
    fn unit_ext(
        mem: &mut FakeMem,
        addr: u64,
        (ty, txt, data, act, path, next): (u32, u32, u64, u64, u64, u64),
        extra: &[(u64, &[u8])],
    ) -> Result<()> {
        let id = u32::try_from(addr >> 12)?.to_le_bytes();
        let (ty, txt, mode) = (ty.to_le_bytes(), txt.to_le_bytes(), 1u32.to_le_bytes());
        let (data, act, path, next) = (
            data.to_le_bytes(),
            act.to_le_bytes(),
            path.to_le_bytes(),
            next.to_le_bytes(),
        );
        let mut fields: Vec<(u64, &[u8])> = vec![
            (unit::TYPE, &ty),
            (unit::TXT, &txt),
            (unit::ID, &id),
            (unit::MODE, &mode),
            (unit::UNIT_DATA, &data),
            (unit::ACT, &act),
            (unit::PATH, &path),
            (unit::NEXT, &next),
        ];
        fields.extend_from_slice(extra);
        put(mem, addr, UNIT_SIZE, &fields)
    }

    fn unit_path(mem: &mut FakeMem, addr: u64, x: u16, y: u16, room1: u64) -> Result<()> {
        put(
            mem,
            addr,
            0x28,
            &[
                (path::X, &x.to_le_bytes()),
                (path::Y, &y.to_le_bytes()),
                (path::ROOM1, &room1.to_le_bytes()),
            ],
        )
    }

    fn name(mem: &mut FakeMem, addr: u64, name: &str) -> Result<()> {
        put(mem, addr, 0x20, &[(player_data::NAME, name.as_bytes())])
    }

    #[allow(clippy::too_many_arguments)] // reason: test fixture mirroring the Room2 fields
    fn room(
        mem: &mut FakeMem,
        addr: u64,
        (x, y, w, h): (u32, u32, u32, u32),
        next: u64,
        room1: u64,
        level: u64,
        presets: u64,
        tiles: u64,
    ) -> Result<()> {
        room_with_preset(mem, addr, (x, y, w, h), next, room1, level, presets, tiles, 0)
    }

    /// Like [`room`], with a level-preset record pointer at `room2::LEVEL_PRESET`.
    #[allow(clippy::too_many_arguments)] // reason: test fixture mirroring the Room2 fields
    fn room_with_preset(
        mem: &mut FakeMem,
        addr: u64,
        (x, y, w, h): (u32, u32, u32, u32),
        next: u64,
        room1: u64,
        level: u64,
        presets: u64,
        tiles: u64,
        level_preset: u64,
    ) -> Result<()> {
        put(
            mem,
            addr,
            0xA0,
            &[
                (room2::LEVEL_PRESET, &level_preset.to_le_bytes()),
                (room2::NEXT, &next.to_le_bytes()),
                (room2::ROOM1, &room1.to_le_bytes()),
                (room2::X, &x.to_le_bytes()),
                (room2::Y, &y.to_le_bytes()),
                (room2::W, &w.to_le_bytes()),
                (room2::H, &h.to_le_bytes()),
                (room2::ROOM_TILES, &tiles.to_le_bytes()),
                (room2::LEVEL, &level.to_le_bytes()),
                (room2::PRESETS, &presets.to_le_bytes()),
            ],
        )
    }

    fn preset_unit(
        mem: &mut FakeMem,
        addr: u64,
        kind: u32,
        txt: u32,
        x: u32,
        y: u32,
        next: u64,
    ) -> Result<()> {
        put(
            mem,
            addr,
            0x28,
            &[
                (preset::TXT, &txt.to_le_bytes()),
                (preset::X, &x.to_le_bytes()),
                (preset::NEXT, &next.to_le_bytes()),
                (preset::TYPE, &kind.to_le_bytes()),
                (preset::Y, &y.to_le_bytes()),
            ],
        )
    }

    fn level_struct(mem: &mut FakeMem, addr: u64, id: u32, first_room: u64) -> Result<()> {
        put(
            mem,
            addr,
            0x200,
            &[
                (level::FIRST_ROOM2, &first_room.to_le_bytes()),
                (level::ID, &id.to_le_bytes()),
            ],
        )
    }

    fn monster_data(mem: &mut FakeMem, addr: u64, seed: u16, flags: u8) -> Result<()> {
        put(
            mem,
            addr,
            0x20,
            &[
                (monster_data::NAME_SEED, &seed.to_le_bytes()),
                (monster_data::FLAGS, &[flags]),
            ],
        )
    }

    /// Level 21 with 3 rooms; players: stale → Alice (slot 3), Bob (slot 9); 2 monsters; 1 item.
    fn world() -> Result<FakeMem> {
        let m = &mut FakeMem::default();
        table(m, &BASE_SLOTS)?;
        // Players: the stale one has a null act.
        unit(m, P_STALE, 0, 1, PD_STALE, 0, 0, P_ALICE)?;
        unit(m, P_ALICE, 0, 3, PD_ALICE, ACT, PATH_ALICE, 0)?;
        unit(m, P_BOB, 0, 5, PD_BOB, ACT, PATH_BOB, 0)?;
        name(m, PD_STALE, "Ghost")?;
        name(m, PD_ALICE, "Alice")?;
        name(m, PD_BOB, "Bob")?;
        unit_path(m, PATH_ALICE, 55, 110, ROOM1_0)?;
        unit_path(m, PATH_BOB, 95, 105, ROOM1_0)?;
        put(m, ROOM1_0, 0x20, &[(room1::ROOM2, &R0.to_le_bytes())])?;
        // Level 21: R0 (loaded) → R1 → R2.
        level_struct(m, LEVEL, 21, R0)?;
        room(m, R0, (10, 20, 8, 6), R1, ROOM1_0, LEVEL, PR_OBJ, RT0)?;
        room(m, R1, (18, 20, 8, 8), R2, 0, LEVEL, PR_MON, 0)?;
        room(m, R2, (10, 26, 4, 4), 0, 0, LEVEL, 0, 0)?;
        preset_unit(m, PR_OBJ, preset::OBJECT, 119, 3, 4, PR_TILE)?;
        preset_unit(m, PR_TILE, preset::TILE, 7, 1, 2, PR_ODD)?;
        preset_unit(m, PR_ODD, 9, 1, 0, 0, 0)?;
        preset_unit(m, PR_MON, preset::MONSTER, 853, 2, 3, 0)?;
        // R0 links to level 22 (room at 40,60) and has one link to an unmapped room.
        put(
            m,
            RT0,
            0x10,
            &[
                (room_tile::ROOM2, &R_OTHER.to_le_bytes()),
                (room_tile::NEXT, &RT_BAD.to_le_bytes()),
            ],
        )?;
        put(m, RT_BAD, 0x10, &[(room_tile::ROOM2, &UNMAPPED.to_le_bytes())])?;
        room(m, R_OTHER, (40, 60, 8, 8), 0, 0, L_OTHER, 0, 0)?;
        level_struct(m, L_OTHER, 22, R_OTHER)?;
        // Monsters M0 → M1; M1 is an elite.
        unit(m, M0, 1, 40, MD0, ACT, PATH_M0, M1)?;
        unit(m, M1, 1, 41, MD1, ACT, PATH_M1, 0)?;
        unit_path(m, PATH_M0, 60, 112, ROOM1_0)?;
        unit_path(m, PATH_M1, 61, 113, ROOM1_0)?;
        monster_data(m, MD0, 5, 0)?;
        monster_data(m, MD1, 77, 0x0C)?;
        // Item without a path.
        unit(m, I0, 4, 600, ID0, ACT, 0, 0)?;
        m.add(ID0, (0u8..0x20).collect());
        Ok(std::mem::take(m))
    }

    #[test]
    fn player_selection() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None, None, None)?;
        assert_eq!(
            s.player.addr,
            Addr(P_ALICE),
            "stale player skipped, first candidate chosen"
        );
        assert_eq!(s.player.name, "Alice");
        assert_eq!(
            (s.player.class, s.player.x, s.player.y, s.player.level_id),
            (3, 55, 110, 21)
        );
        assert_eq!(
            s.player_candidates,
            vec![
                Candidate {
                    addr: Addr(P_ALICE),
                    id: ALICE_ID,
                    name: "Alice".into()
                },
                Candidate {
                    addr: Addr(P_BOB),
                    id: BOB_ID,
                    name: "Bob".into()
                },
            ]
        );
        assert_eq!(s.player_chosen_by, ChosenBy::FirstCandidate, "no items to vote");
        Ok(())
    }

    #[test]
    fn char_level_values() {
        assert_eq!(char_level_from_raw(1), Some(1));
        assert_eq!(char_level_from_raw(34), Some(34));
        assert_eq!(char_level_from_raw(99), Some(99));
        assert_eq!(char_level_from_raw(34 << 8), Some(34), "shifted by 8");
        assert_eq!(char_level_from_raw(99 << 8), Some(99));
        for raw in [0, 100, 255, 1 << 16, 100 << 8, 0x1234, u32::MAX] {
            assert_eq!(char_level_from_raw(raw), None, "{raw:#x}");
        }
    }

    /// [`world`] with Alice's unit given a stat list holding `stats` (stat id, value).
    fn world_with_player_stats(stats: &[(u16, u32)]) -> Result<FakeMem> {
        use crate::process::ReadMem;
        const LIST: u64 = 0x90_0000;
        const ENTRIES: u64 = 0x90_1000;
        let mut m = world()?;
        let mut alice = vec![0u8; UNIT_SIZE];
        m.read(P_ALICE, &mut alice)?;
        let off = usize::try_from(unit::STATS)?;
        alice
            .get_mut(off..off + 8)
            .context("stats field in unit")?
            .copy_from_slice(&LIST.to_le_bytes());
        m.add(P_ALICE, alice);
        let count = u64::try_from(stats.len())?;
        put(
            &mut m,
            LIST,
            0x40,
            &[
                (stat_list::ENTRIES, &ENTRIES.to_le_bytes()),
                (stat_list::COUNT, &count.to_le_bytes()),
            ],
        )?;
        let mut b = Vec::new();
        for &(id, value) in stats {
            b.extend(0u16.to_le_bytes());
            b.extend(id.to_le_bytes());
            b.extend(value.to_le_bytes());
        }
        m.add(ENTRIES, b);
        Ok(m)
    }

    #[test]
    fn player_char_level_from_stat_12() -> Result<()> {
        // Strength, energy, then level among the player's stats.
        let m = world_with_player_stats(&[(0, 25), (1, 15), (stat_list::LEVEL, 34), (13, 9000)])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(s.player.char_level, Some(34));
        assert_eq!(
            serde_json::to_value(&s)?.pointer("/player/char_level"),
            Some(&34.into())
        );

        let m = world_with_player_stats(&[(stat_list::LEVEL, 34 << 8)])?;
        assert_eq!(
            snapshot(&m, TABLE, None, None, None)?.player.char_level,
            Some(34),
            "shifted"
        );

        let m = world_with_player_stats(&[(0, 25)])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(s.player.char_level, None, "no level stat");

        // The base world has no stat list on the player: no level, left out of the JSON.
        let s = snapshot(&world()?, TABLE, None, None, None)?;
        assert_eq!(s.player.char_level, None);
        assert_eq!(serde_json::to_value(&s)?.pointer("/player/char_level"), None);
        Ok(())
    }

    /// Unit ids of Alice and Bob (the fixture's id is the address >> 12).
    const ALICE_ID: u32 = 0x201;
    const BOB_ID: u32 = 0x202;

    /// A normal item in `mode` on storage `page`, owned by unit id `owner` (`item_data::OWNER`).
    fn owned_item(
        mem: &mut FakeMem,
        addr: u64,
        (mode, page): (u32, u8),
        owner: u32,
        next: u64,
    ) -> Result<()> {
        item(mem, addr, (1, mode, 2), (30, 31), next)?;
        put(
            mem,
            addr + 0x200,
            0x60,
            &[
                (item_data::QUALITY, &2u32.to_le_bytes()),
                (item_data::OWNER, &owner.to_le_bytes()),
                (item_data::PAGE, &[page]),
            ],
        )
    }

    /// [`world`] with the item chain I0 → `items` (mode/page, owner), in order.
    fn world_with_owned(items: &[((u32, u8), u32)]) -> Result<FakeMem> {
        let mut m = world()?;
        let addrs = [I1, I2, I3, I4, I5, I6];
        let first = if items.is_empty() { 0 } else { I1 };
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, first)?;
        for (i, &(place, owner)) in items.iter().enumerate() {
            let addr = *addrs.get(i).context("at most 6 owned items")?;
            let next = if i + 1 < items.len() {
                *addrs.get(i + 1).context("at most 6 owned items")?
            } else {
                0
            };
            owned_item(&mut m, addr, place, owner, next)?;
        }
        Ok(m)
    }

    const STASH: (u32, u8) = (item_mode::STORED, item_data::PAGE_STASH);
    const INVENTORY: (u32, u8) = (item_mode::STORED, item_data::PAGE_INVENTORY);
    const GROUND: (u32, u8) = (item_mode::ON_GROUND, 255);

    #[test]
    fn stash_owner_picks_the_local_player() -> Result<()> {
        // Stash: Bob, Bob, Alice (majority Bob); the inventory says Alice but the stash wins.
        let m = world_with_owned(&[
            (STASH, BOB_ID),
            (INVENTORY, ALICE_ID),
            (STASH, ALICE_ID),
            (STASH, BOB_ID),
        ])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(
            (s.player.addr, s.player_chosen_by),
            (Addr(P_BOB), ChosenBy::StashOwner)
        );
        assert_eq!(s.player.name, "Bob");
        assert_eq!(
            s.inventory_items, 1,
            "the owner vote doesn't change the inventory scan"
        );
        Ok(())
    }

    #[test]
    fn inventory_owner_without_stash_items() -> Result<()> {
        let m = world_with_owned(&[(INVENTORY, BOB_ID), (INVENTORY, BOB_ID)])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(
            (s.player.addr, s.player_chosen_by),
            (Addr(P_BOB), ChosenBy::InventoryOwner)
        );
        // A stash owner that is no candidate falls through to the inventory vote.
        let m = world_with_owned(&[(STASH, 0x999), (INVENTORY, BOB_ID)])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(
            (s.player.addr, s.player_chosen_by),
            (Addr(P_BOB), ChosenBy::InventoryOwner)
        );
        Ok(())
    }

    #[test]
    fn shared_stash_pseudo_owners_do_not_vote() -> Result<()> {
        // Four items of a shared stash tab (pseudo-owner, no player) and one of Bob's: before,
        // the tab id won the stash vote and the choice fell through; now only Bob votes.
        let tab = 0x7777;
        let m = world_with_owned(&[
            (STASH, tab),
            (STASH, tab),
            (STASH, tab),
            (STASH, tab),
            (STASH, BOB_ID),
        ])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(
            (s.player.addr, s.player_chosen_by),
            (Addr(P_BOB), ChosenBy::StashOwner)
        );
        Ok(())
    }

    #[test]
    fn player_name_beats_the_owner_vote() -> Result<()> {
        let m = world_with_owned(&[(STASH, BOB_ID), (INVENTORY, BOB_ID)])?;
        let s = snapshot(&m, TABLE, None, Some("Alice"), None)?;
        assert_eq!(
            (s.player.addr, s.player_chosen_by),
            (Addr(P_ALICE), ChosenBy::Name)
        );
        Ok(())
    }

    #[test]
    fn previous_player_is_kept_while_a_candidate() -> Result<()> {
        // Alice was chosen first; the owner votes now alternate (here: all Bob's). Alice stays.
        let m = world_with_owned(&[(STASH, BOB_ID), (INVENTORY, BOB_ID)])?;
        let s = snapshot(&m, TABLE, None, None, Some(ALICE_ID))?;
        assert_eq!(
            (s.player.addr, s.player_chosen_by),
            (Addr(P_ALICE), ChosenBy::Previous)
        );
        // `--player` still wins over the previous choice.
        let s = snapshot(&m, TABLE, None, Some("Bob"), Some(ALICE_ID))?;
        assert_eq!((s.player.addr, s.player_chosen_by), (Addr(P_BOB), ChosenBy::Name));
        // The previous player left (no such candidate): the vote decides again.
        let s = snapshot(&m, TABLE, None, None, Some(0x999))?;
        assert_eq!(
            (s.player.addr, s.player_chosen_by),
            (Addr(P_BOB), ChosenBy::StashOwner)
        );
        Ok(())
    }

    #[test]
    fn alternating_owner_votes_keep_the_first_choice_and_the_game() -> Result<()> {
        // Group game: the owner vote flips between Bob and Alice from tick to tick.
        let bob = world_with_owned(&[(STASH, BOB_ID)])?;
        let alice = world_with_owned(&[(STASH, ALICE_ID)])?;
        let mut session = crate::session::Session::default();
        for (i, m) in [&bob, &alice, &bob, &alice].into_iter().enumerate() {
            let mut s = snapshot(m, TABLE, None, None, session.player_id())?;
            assert_eq!(s.player.addr, Addr(P_BOB), "tick {i}");
            assert!(!session.apply(&mut s).new_game, "tick {i}");
        }
        Ok(())
    }

    #[test]
    fn ground_and_unowned_items_dont_vote() -> Result<()> {
        // Ground items "owned" by Bob (live they carry NO_OWNER) and stash items without owner.
        let m = world_with_owned(&[
            (GROUND, BOB_ID),
            (GROUND, BOB_ID),
            (STASH, item_data::NO_OWNER),
            (STASH, item_data::NO_OWNER),
            (STASH, 0),
        ])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(
            (s.player.addr, s.player_chosen_by),
            (Addr(P_ALICE), ChosenBy::FirstCandidate)
        );
        Ok(())
    }

    /// `ItemData::BODY` of the item at `addr` (its `ItemData` region sits at `addr + 0x200`).
    fn set_body(mem: &mut FakeMem, addr: u64, body: u8) -> Result<()> {
        let data = addr + 0x200;
        let mut b = mem.read_bytes(data, 0x60)?;
        let at = usize::try_from(item_data::BODY)?;
        b.get_mut(at..at + 1)
            .context("body in ItemData")?
            .copy_from_slice(&[body]);
        mem.add(data, b);
        Ok(())
    }

    const SK_HEADER: u64 = 0x90_0000;
    const SK_NODES: u64 = 0x91_0000;
    const SK_TXT: u64 = 0x92_0000;

    /// Gives Alice (a paladin, class 3) a skill list of `(skills.txt id, hard points)` nodes
    /// in that order; `left` / `right` index the node on each mouse button.
    fn alice_skills(m: &mut FakeMem, nodes: &[(u16, u32)], left: usize, right: usize) -> Result<()> {
        unit_ext(
            m,
            P_ALICE,
            (0, 3, PD_ALICE, ACT, PATH_ALICE, 0),
            &[(unit::SKILLS, &SK_HEADER.to_le_bytes())],
        )?;
        let node = |i: usize| SK_NODES + u64::try_from(i).unwrap_or(0) * 0x1000;
        for (i, &(id, hard)) in nodes.iter().enumerate() {
            let txt = SK_TXT + u64::try_from(i)? * 0x1000;
            put(m, txt, 0x10, &[(skill_txt::ID, &id.to_le_bytes())])?;
            let next = if i + 1 < nodes.len() { node(i + 1) } else { 0 };
            put(
                m,
                node(i),
                0x60,
                &[
                    (skill::TXT, &txt.to_le_bytes()),
                    (skill::NEXT, &next.to_le_bytes()),
                    (skill::HARD_POINTS, &hard.to_le_bytes()),
                ],
            )?;
        }
        put(
            m,
            SK_HEADER,
            0x30,
            &[
                (skills::FIRST, &node(0).to_le_bytes()),
                (skills::LEFT, &node(left).to_le_bytes()),
                (skills::RIGHT, &node(right).to_le_bytes()),
            ],
        )
    }

    #[test]
    fn skills_keep_the_class_skills_with_points() -> Result<()> {
        let mut m = world()?;
        // Attack and a scroll (basic, 1 point each), Sacrifice 5, Might 3, Prayer 0 (a +skill
        // from gear), Fire Bolt 2 (another class's skill), Blessed Hammer 20.
        let nodes = [(0, 1), (219, 1), (96, 5), (98, 3), (99, 0), (36, 2), (112, 20)];
        alice_skills(&mut m, &nodes, 0, 3)?;
        let s = snapshot(&m, TABLE, None, Some("Alice"), None)?;
        assert_eq!(s.skills.points, vec![(96, 5), (98, 3), (112, 20)]);
        assert_eq!(
            (s.skills.left, s.skills.right),
            (Some(0), Some(98)),
            "Attack / Might"
        );
        Ok(())
    }

    #[test]
    fn a_broken_skill_entry_is_a_warning_and_skipped() -> Result<()> {
        let mut m = world()?;
        alice_skills(&mut m, &[(96, 5), (98, 3)], 0, 1)?;
        // The second node's skills.txt record is unmapped.
        put(
            &mut m,
            SK_NODES + 0x1000,
            0x60,
            &[(skill::TXT, &UNMAPPED.to_le_bytes())],
        )?;
        let s = snapshot(&m, TABLE, None, Some("Alice"), None)?;
        assert_eq!(s.skills.points, vec![(96, 5)], "the readable entry stays");
        assert!(
            s.warnings.iter().any(|w| w.starts_with("skill ")),
            "{:?}",
            s.warnings
        );
        Ok(())
    }

    #[test]
    fn no_skill_list_is_empty_skills_without_a_warning() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None, Some("Alice"), None)?;
        assert!(s.skills.is_empty());
        assert!(
            !s.warnings.iter().any(|w| w.contains("skill")),
            "{:?}",
            s.warnings
        );
        Ok(())
    }

    #[test]
    fn equipped_items_are_enriched_with_code_name_and_row() -> Result<()> {
        let mut m = world()?;
        let id = item_data::FLAG_IDENTIFIED;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        // Bob's equipped helm: an identified Shako (unique row 248, lvl req 62).
        stored_item(&mut m, I1, (40, 7, id, 248), (1, 255), &[], I2)?;
        set_owner(&mut m, I1, BOB_ID)?;
        set_body(&mut m, I1, 1)?;
        // Bob's equipped amulet: an identified set piece (Cleglaw's Claw, set row 7, lvl req 4).
        stored_item(&mut m, I2, (40, 5, id, 7), (1, 255), &[], I3)?;
        set_owner(&mut m, I2, BOB_ID)?;
        set_body(&mut m, I2, 2)?;
        // Bob's equipped weapon: a finished runeword on a Crystal Sword.
        stored_item(
            &mut m,
            I3,
            (30, 2, id | item_data::FLAG_RUNEWORD, 0),
            (1, 255),
            &[],
            I4,
        )?;
        set_owner(&mut m, I3, BOB_ID)?;
        set_body(&mut m, I3, 4)?;
        // Bob's equipped body armour: magic, so only the BASE requirement is knowable.
        stored_item(&mut m, I4, (40, 4, id, 0), (1, 255), &[], I5)?;
        set_owner(&mut m, I4, BOB_ID)?;
        set_body(&mut m, I4, 3)?;
        // The mercenary's equipped armour (NO_OWNER, body 3): pairs only, not enriched.
        stored_item(&mut m, I5, (40, 2, id, 0), (1, 255), &[], 0)?;
        set_owner(&mut m, I5, item_data::NO_OWNER)?;
        set_body(&mut m, I5, 3)?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!(s.equipped, vec![(1, 7), (2, 5), (4, 2), (3, 4)]);
        assert_eq!(s.merc_equipped, vec![(3, 2)], "the merc's piece stays a pair");
        let eq = &s.equipped_items;
        assert_eq!(
            eq.iter().map(|e| e.body).collect::<Vec<_>>(),
            vec![1, 2, 4, 3],
            "one enriched item per body slot: {eq:?}"
        );
        let helm = eq.first().context("helm")?;
        assert_eq!(helm.code.as_deref(), Some("uap"));
        assert_eq!(helm.name.as_deref(), Some("Harlequin Crest"));
        assert_eq!(helm.file_index, Some(248));
        assert!(!helm.runeword);
        assert_eq!(helm.required_level(), Some(62), "the unique's exact lvl req");
        let amulet = eq.get(1).context("amulet")?;
        assert_eq!(amulet.name.as_deref(), Some("Cleglaw's Claw"));
        assert_eq!(amulet.file_index, Some(7));
        assert_eq!(amulet.required_level(), Some(4), "the set row's lvl req");
        let weapon = eq.get(2).context("weapon")?;
        assert_eq!(weapon.code.as_deref(), Some("crs"));
        assert!(weapon.runeword);
        assert_eq!(weapon.name, None, "a runeword's runes are unread");
        assert_eq!(weapon.file_index, None);
        assert_eq!(weapon.required_level(), None, "runeword: level unknown");
        let armour = eq.get(3).context("armour")?;
        assert_eq!(armour.name, None, "magic: no name");
        assert_eq!(
            armour.required_level(),
            Some(43),
            "magic: the BASE item's levelreq (affixes can raise the real one)"
        );
        Ok(())
    }

    #[test]
    fn only_the_chosen_players_gear_counts_in_a_group_game() -> Result<()> {
        // Live 2026-11-17: in a public game the other players' equipped items filled the grid.
        let mut m = world()?;
        let id = item_data::FLAG_IDENTIFIED;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        // Bob wears a Shako; Alice wears a magic body armour.
        stored_item(&mut m, I1, (40, 7, id, 248), (1, 255), &[], I2)?;
        set_owner(&mut m, I1, BOB_ID)?;
        set_body(&mut m, I1, 1)?;
        stored_item(&mut m, I2, (40, 4, id, 0), (1, 255), &[], 0)?;
        set_owner(&mut m, I2, ALICE_ID)?;
        set_body(&mut m, I2, 3)?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Alice"), None)?;
        assert_eq!(s.equipped, vec![(3, 4)], "only Alice's armour");
        assert_eq!(
            s.equipped_items.iter().map(|e| e.body).collect::<Vec<_>>(),
            vec![3]
        );
        assert!(
            s.owned_names.is_empty(),
            "Bob's Shako is not Alice's: {:?}",
            s.owned_names
        );
        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!(s.equipped, vec![(1, 7)]);
        assert_eq!(s.owned_names, vec!["Harlequin Crest"]);
        Ok(())
    }

    #[test]
    fn owner_votes_majority_and_cap() {
        let mut v = OwnerVotes::default();
        assert_eq!(v.winner(), None);
        v.add(7);
        v.add(9);
        assert_eq!(v.winner(), Some(7), "a tie goes to the first seen");
        v.add(9);
        assert_eq!(v.winner(), Some(9));
        for _ in 0..6 {
            assert!(!v.done());
            v.add(9);
        }
        assert!(v.done(), "8 agreeing votes end the vote");
    }

    #[test]
    fn room_walk_stops_at_foreign_level() -> Result<()> {
        let mut m = world()?;
        // R2 now claims to belong to another level: the walk keeps R0, R1 and stops with a warning.
        room(&mut m, R2, (10, 26, 4, 4), 0, 0, L_OTHER, 0, 0)?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(s.level.rooms.len(), 2);
        assert!(s.warnings.iter().any(|w| w.contains("room walk stopped")));
        // A foreign first room means the level offsets are wrong: that's an error, not an empty map.
        level_struct(&mut m, LEVEL, 21, R_OTHER)?;
        assert!(snapshot(&m, TABLE, None, None, None).is_err());
        Ok(())
    }

    #[test]
    fn level_rooms_in_world_tiles() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None, None, None)?;
        assert_eq!(s.level.id, 21);
        assert_eq!(
            s.level.rooms,
            vec![
                Room {
                    x: 50,
                    y: 100,
                    w: 40,
                    h: 30,
                    loaded: true,
                    preset: None
                },
                Room {
                    x: 90,
                    y: 100,
                    w: 40,
                    h: 40,
                    loaded: false,
                    preset: None
                },
                Room {
                    x: 50,
                    y: 130,
                    w: 20,
                    h: 20,
                    loaded: false,
                    preset: None
                },
            ]
        );
        assert_eq!(
            s.level.bounds,
            Some(Bounds {
                min_x: 50,
                min_y: 100,
                max_x: 130,
                max_y: 150
            })
        );
        Ok(())
    }

    /// Points `room`'s near-room array at `array` with `count` entries (the Room2 must exist).
    fn set_near(mem: &mut FakeMem, room: u64, array: u64, count: u32) -> Result<()> {
        let mut b = mem.read_bytes(room, 0xA0)?;
        for (off, val) in [
            (room2::NEAR, array.to_le_bytes().to_vec()),
            (room2::NEAR_COUNT, count.to_le_bytes().to_vec()),
        ] {
            let off = usize::try_from(off)?;
            b.get_mut(off..off + val.len())
                .context("field outside struct")?
                .copy_from_slice(&val);
        }
        mem.add(room, b);
        Ok(())
    }

    /// A near-room array of `rooms` (8-byte pointers), exactly that long.
    fn near_array(mem: &mut FakeMem, addr: u64, rooms: &[u64]) {
        mem.add(addr, rooms.iter().flat_map(|r| r.to_le_bytes()).collect());
    }

    const NEAR0: u64 = 0x55_0000;
    const NEAR1: u64 = 0x55_1000;
    const R_OTHER2: u64 = 0x60_1000;

    const fn rect(x: u32, y: u32, w: u32, h: u32) -> RoomRect {
        RoomRect { x, y, w, h }
    }

    #[test]
    fn near_rooms_in_other_levels_become_border_links() -> Result<()> {
        let mut m = world()?;
        // R0 sees two rooms of level 22, its own neighbour R1 (same level: skipped) and nulls.
        room(&mut m, R_OTHER2, (48, 60, 8, 8), 0, 0, L_OTHER, 0, 0)?;
        near_array(&mut m, NEAR0, &[R_OTHER, R1, 0, R_OTHER2, 0]);
        set_near(&mut m, R0, NEAR0, 5)?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        let from = rect(50, 100, 40, 30);
        let link = |to_room| BorderLink {
            from_room: from,
            to_level_id: 22,
            to_room,
            entry: false,
        };
        assert_eq!(
            s.border_links,
            vec![link(rect(200, 300, 40, 40)), link(rect(240, 300, 40, 40))]
        );
        // Only the bad room-tile link warns.
        assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
        Ok(())
    }

    #[test]
    fn border_links_are_deduplicated() -> Result<()> {
        let mut m = world()?;
        // R0 lists R_OTHER twice; R1 lists it too (a different from-room: kept).
        near_array(&mut m, NEAR0, &[R_OTHER, R_OTHER]);
        near_array(&mut m, NEAR1, &[R_OTHER]);
        set_near(&mut m, R0, NEAR0, 2)?;
        set_near(&mut m, R1, NEAR1, 1)?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        let from: Vec<RoomRect> = s.border_links.iter().map(|l| l.from_room).collect();
        assert_eq!(from, vec![rect(50, 100, 40, 30), rect(90, 100, 40, 40)]);
        // Empty border links stay out of the JSON (fixtures unchanged); filled ones round-trip.
        assert!(
            serde_json::to_value(snapshot(&world()?, TABLE, None, None, None)?)?
                .get("border_links")
                .is_none()
        );
        let back: Snapshot = serde_json::from_str(&serde_json::to_string(&s)?)?;
        assert_eq!(back.border_links, s.border_links);
        Ok(())
    }

    #[test]
    fn near_room_count_is_capped() -> Result<()> {
        let mut m = world()?;
        // A garbage count: only MAX_NEAR_ROOMS entries are mapped, so reading more would fail.
        let cap = usize::try_from(MAX_NEAR_ROOMS)?;
        let mut rooms = vec![0; cap];
        if let Some(last) = rooms.last_mut() {
            *last = R_OTHER;
        }
        near_array(&mut m, NEAR0, &rooms);
        set_near(&mut m, R0, NEAR0, 100_000)?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(s.border_links.len(), 1, "the 64th entry is still read");
        assert!(
            s.warnings.iter().any(|w| w.contains("100000 near rooms")),
            "{:?}",
            s.warnings
        );
        Ok(())
    }

    #[test]
    fn unreadable_near_rooms_only_warn() -> Result<()> {
        let mut m = world()?;
        // R0's array is unmapped; R1's array holds an unmapped room pointer.
        set_near(&mut m, R0, UNMAPPED, 3)?;
        near_array(&mut m, NEAR1, &[UNMAPPED]);
        set_near(&mut m, R1, NEAR1, 1)?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(s.border_links, vec![]);
        assert_eq!(s.level.rooms.len(), 3, "the rooms are still read");
        assert!(
            s.warnings.iter().any(|w| w.contains("near rooms")),
            "{:?}",
            s.warnings
        );
        assert!(
            s.warnings.iter().any(|w| w.contains("near room 0x")),
            "{:?}",
            s.warnings
        );
        Ok(())
    }

    #[test]
    fn room_preset_ids() -> Result<()> {
        const LP0: u64 = 0x33_0000;
        let mut m = world()?;
        // R0 points at a record with id 525; R1 at an unmapped record; R2 keeps a null pointer.
        room_with_preset(&mut m, R0, (10, 20, 8, 6), R1, ROOM1_0, LEVEL, PR_OBJ, RT0, LP0)?;
        room_with_preset(&mut m, R1, (18, 20, 8, 8), R2, 0, LEVEL, PR_MON, 0, UNMAPPED)?;
        put(&mut m, LP0, 0x10, &[(level_preset::ID, &525u32.to_le_bytes())])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        let ids: Vec<Option<u32>> = s.level.rooms.iter().map(|r| r.preset).collect();
        assert_eq!(ids, vec![Some(525), None, None]);
        // Optional data: the unmapped record adds no warning (only the bad room-tile link does).
        assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
        // `None` is left out of the JSON, so old fixtures and dumps stay as they were.
        let v = serde_json::to_value(&s.level.rooms)?;
        assert_eq!(v.pointer("/0/preset"), Some(&serde_json::json!(525)));
        assert_eq!(v.pointer("/1/preset"), None);
        Ok(())
    }

    #[test]
    fn presets_and_links() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None, None, None)?;
        assert_eq!(
            s.presets,
            vec![
                Preset {
                    kind: PresetKind::Object,
                    txt: 119,
                    x: 53,
                    y: 104,
                    entry: false
                },
                Preset {
                    kind: PresetKind::Tile,
                    txt: 7,
                    x: 51,
                    y: 102,
                    entry: false
                },
                Preset {
                    kind: PresetKind::Other(9),
                    txt: 1,
                    x: 50,
                    y: 100,
                    entry: false
                },
                Preset {
                    kind: PresetKind::Monster,
                    txt: 853,
                    x: 92,
                    y: 103,
                    entry: false
                },
            ]
        );
        assert_eq!(
            s.room_links,
            vec![RoomLink {
                from_room: RoomPos { x: 50, y: 100 },
                to_level_id: 22,
                to_room: RoomPos { x: 200, y: 300 },
            }]
        );
        // The unmapped room-tile link is the only problem: a warning, not an error.
        assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
        let w = s.warnings.first().context("warning")?;
        assert!(w.starts_with(&format!("room tile {RT_BAD:#x}")), "{w}");
        Ok(())
    }

    #[test]
    fn monsters_and_items() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None, None, None)?;
        let monster = |addr, txt, x, y, flags, name_seed| Monster {
            addr: Addr(addr),
            id: u32::try_from(addr >> 12).unwrap_or_default(),
            txt,
            mode: 1,
            x,
            y,
            flags,
            name_seed,
            owner_id: 0,
            owner_type: 0,
            desecrated: false,
        };
        assert_eq!(
            s.monsters,
            vec![monster(M0, 40, 60, 112, 0, 5), monster(M1, 41, 61, 113, 0x0C, 77)],
            "M0 -> M1 chain via unit NEXT"
        );
        assert_eq!(
            (s.ground_items, s.drops.len()),
            (0, 0),
            "the only item is not on the ground"
        );
        Ok(())
    }

    /// A state bitfield with the given dwords.
    fn state_field(dwords: [u32; 8]) -> Vec<u8> {
        dwords.iter().flat_map(|d| d.to_le_bytes()).collect()
    }

    #[test]
    fn state_bits() {
        // State 189: dword 5, mask 0x2000_0000 (the live terrorized cows).
        let flags = state_field([0, 0, 0, 0, 0, 0x2000_0000, 0, 0]);
        assert!(has_state(&flags, state::DESECRATED));
        assert!(!has_state(&flags, 188));
        assert!(!has_state(&flags, 190));
        assert!(!has_state(&flags, 29), "same mask in dword 0");
        assert!(!has_state(&flags, 256), "past the end");
        // Every other bit set: 189 still clear.
        let mut others = [u32::MAX; 8];
        others[5] = !0x2000_0000;
        let others = state_field(others);
        assert!(!has_state(&others, state::DESECRATED));
        assert!(has_state(&others, 188) && has_state(&others, 0) && has_state(&others, 255));
    }

    /// `M0` → `M1` → `M2`, each with a stat list whose state bitfield has `bits` set (`None`: an
    /// unmapped stat list).
    fn world_with_states(bits: [Option<&[u32]>; 3]) -> Result<FakeMem> {
        let mut m = world()?;
        for (i, (addr, b)) in (0u64..).zip([M0, M1, M2].into_iter().zip(bits)) {
            let list = 0xA0_0000 + i * 0x1000;
            let path = [PATH_M0, PATH_M1, PATH_M2]
                .get(usize::try_from(i)?)
                .copied()
                .context("path")?;
            let next = [M1, M2, 0].get(usize::try_from(i)?).copied().context("next")?;
            put(
                &mut m,
                addr,
                UNIT_SIZE,
                &[
                    (unit::TYPE, &unit_table::MONSTER.to_le_bytes()),
                    (unit::TXT, &40u32.to_le_bytes()),
                    (unit::ID, &(u32::try_from(addr >> 12)?).to_le_bytes()),
                    (unit::MODE, &1u32.to_le_bytes()),
                    (unit::UNIT_DATA, &MD0.to_le_bytes()),
                    (unit::PATH, &path.to_le_bytes()),
                    (
                        unit::STATS,
                        &(if b.is_some() { list } else { UNMAPPED }).to_le_bytes(),
                    ),
                    (unit::NEXT, &next.to_le_bytes()),
                ],
            )?;
            let mut dwords = [0u32; 8];
            for &bit in b.unwrap_or_default() {
                *dwords
                    .get_mut(usize::try_from(bit / 32)?)
                    .context("bit in field")? |= 1 << (bit % 32);
            }
            put(
                &mut m,
                list,
                0xB60,
                &[(stat_list::STATE_FLAGS, &state_field(dwords))],
            )?;
        }
        unit_path(&mut m, PATH_M2, 70, 120, ROOM1_0)?;
        Ok(m)
    }

    #[test]
    fn monster_desecrated_state() -> Result<()> {
        let m = world_with_states([Some(&[189]), Some(&[56, 188, 190]), None])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        let flags: Vec<_> = s.monsters.iter().map(|m| (m.addr.0, m.desecrated)).collect();
        assert_eq!(flags, vec![(M0, true), (M1, false), (M2, false)]);
        assert!(
            !s.warnings.iter().any(|w| w.starts_with("monster")),
            "an unreadable state field is no warning: {:?}",
            s.warnings
        );
        assert!(!s.terrorized, "one desecrated monster is not enough");

        let m = world_with_states([Some(&[189]), Some(&[3, 189]), None])?;
        assert!(snapshot(&m, TABLE, None, None, None)?.terrorized);
        Ok(())
    }

    #[test]
    fn terrorized_needs_two_living_desecrated_monsters() {
        let mon = |desecrated, mode| Monster {
            addr: Addr(0),
            id: 1,
            txt: 1,
            mode,
            x: 0,
            y: 0,
            flags: 0,
            name_seed: 0,
            owner_id: 0,
            owner_type: 0,
            desecrated,
        };
        assert!(!is_terrorized(&[]));
        assert!(!is_terrorized(&[mon(true, 1), mon(false, 1)]));
        assert!(is_terrorized(&[mon(true, 1), mon(true, 2)]));
        assert!(
            !is_terrorized(&[mon(true, 1), mon(true, 0), mon(true, 12)]),
            "dead ones don't count"
        );
        assert!(!is_terrorized(&[mon(false, 1), mon(false, 1), mon(false, 1)]));
    }

    const ITEMS: u64 = 0x90_0000;
    const I1: u64 = 0x80_1000;
    const I2: u64 = 0x80_2000;
    const I3: u64 = 0x80_3000;
    const I4: u64 = 0x80_4000;
    const I5: u64 = 0x80_5000;
    const I6: u64 = 0x80_6000;

    /// An item unit with its `ItemData` (quality) and static path (position) at `addr + 0x200`/`+ 0x400`.
    fn item(
        mem: &mut FakeMem,
        addr: u64,
        (txt, mode, quality): (u32, u32, u32),
        (x, y): (u32, u32),
        next: u64,
    ) -> Result<()> {
        let (data, path) = (addr + 0x200, addr + 0x400);
        put(
            mem,
            addr,
            UNIT_SIZE,
            &[
                (unit::TYPE, &unit_table::ITEM.to_le_bytes()),
                (unit::TXT, &txt.to_le_bytes()),
                (unit::ID, &(u32::try_from(addr >> 12)?).to_le_bytes()),
                (unit::MODE, &mode.to_le_bytes()),
                (unit::UNIT_DATA, &data.to_le_bytes()),
                (unit::PATH, &path.to_le_bytes()),
                (unit::NEXT, &next.to_le_bytes()),
            ],
        )?;
        put(mem, data, 0x20, &[(item_data::QUALITY, &quality.to_le_bytes())])?;
        // A dynamic path's u16 x/y offsets hold something else on a static path (live: 22407, 0).
        put(
            mem,
            path,
            0x20,
            &[
                (path::X, &22407u16.to_le_bytes()),
                (static_path::X, &x.to_le_bytes()),
                (static_path::Y, &y.to_le_bytes()),
            ],
        )
    }

    /// Items table with codes for rows 1 (axe), 587 (chipped emerald) and 625 (El rune).
    fn items_table(mem: &mut FakeMem) -> Result<()> {
        let stride = usize::try_from(crate::offsets::items_txt::STRIDE)?;
        let mut b = vec![0u8; 700 * stride];
        for (row, code) in [(1, b"axe "), (587, b"gcg "), (625, b"r01 ")] {
            b.get_mut(row * stride..row * stride + 4)
                .context("row in table")?
                .copy_from_slice(code);
        }
        mem.add(ITEMS, b);
        Ok(())
    }

    /// I0 (no path, mode 1) → ground gem, ground normal axe, ground unique axe, ground rune,
    /// rune in the inventory (mode 0), ground set item.
    fn world_with_items() -> Result<FakeMem> {
        let mut m = world()?;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        item(&mut m, I1, (587, 3, 2), (5323, 5622), I2)?;
        item(&mut m, I2, (1, 3, 2), (10, 11), I3)?;
        item(&mut m, I3, (1, 3, 7), (12, 13), I4)?;
        item(&mut m, I4, (625, 3, 2), (14, 15), I5)?;
        item(&mut m, I5, (625, 0, 2), (16, 17), I6)?;
        item(&mut m, I6, (1, 3, 5), (18, 19), 0)?;
        items_table(&mut m)?;
        Ok(m)
    }

    fn drop_of(addr: u64, code: Option<&str>, kind: DropKind, (x, y): (u32, u32)) -> ItemDrop {
        let quality = match kind {
            DropKind::Unique => 7,
            DropKind::Set => 5,
            _ => 2,
        };
        ItemDrop {
            id: u32::try_from(addr >> 12).unwrap_or_default(),
            code: code.map(str::to_owned),
            kind,
            quality,
            flags: 0,
            ethereal: false,
            sockets: 0,
            unique_index: None,
            item_data_head: String::new(),
            highlight: None,
            x,
            y,
        }
    }

    /// [`item`] on the ground plus `ItemData` flags and file index, and a stat list with `stats`
    /// (stat id, value) at `addr + 0x600` (entries at `+ 0x700`).
    fn rich_item(
        mem: &mut FakeMem,
        addr: u64,
        fields: (u32, u32, u32, u32),
        stats: &[(u16, u32)],
        next: u64,
    ) -> Result<()> {
        stored_item(mem, addr, fields, (item_mode::ON_GROUND, 255), stats, next)
    }

    /// [`rich_item`] with a unit mode and `ItemData` storage page.
    fn stored_item(
        mem: &mut FakeMem,
        addr: u64,
        (txt, quality, flags, file_index): (u32, u32, u32, u32),
        (mode, page): (u32, u8),
        stats: &[(u16, u32)],
        next: u64,
    ) -> Result<()> {
        item(mem, addr, (txt, mode, quality), (20, 21), next)?;
        let (data, list, entries) = (addr + 0x200, addr + 0x600, addr + 0x700);
        put(
            mem,
            addr,
            UNIT_SIZE,
            &[
                (unit::TYPE, &unit_table::ITEM.to_le_bytes()),
                (unit::TXT, &txt.to_le_bytes()),
                (unit::ID, &(u32::try_from(addr >> 12)?).to_le_bytes()),
                (unit::MODE, &mode.to_le_bytes()),
                (unit::UNIT_DATA, &data.to_le_bytes()),
                (unit::PATH, &(addr + 0x400).to_le_bytes()),
                (unit::STATS, &list.to_le_bytes()),
                (unit::NEXT, &next.to_le_bytes()),
            ],
        )?;
        put(
            mem,
            data,
            ITEM_DATA_SIZE,
            &[
                (item_data::QUALITY, &quality.to_le_bytes()),
                (item_data::FLAGS, &flags.to_le_bytes()),
                (item_data::FILE_INDEX, &file_index.to_le_bytes()),
                (item_data::PAGE, &[page]),
            ],
        )?;
        let count = u64::try_from(stats.len())?;
        put(
            mem,
            list,
            0x40,
            &[
                (stat_list::ENTRIES, &entries.to_le_bytes()),
                (stat_list::COUNT, &count.to_le_bytes()),
            ],
        )?;
        let mut b = Vec::new();
        for &(id, value) in stats {
            b.extend(0u16.to_le_bytes());
            b.extend(id.to_le_bytes());
            b.extend(value.to_le_bytes());
        }
        mem.add(entries, b);
        Ok(())
    }

    /// Items table with `crs` (Crystal Sword) at row 30 and `uap` (Shako) at row 40.
    fn base_items_table(mem: &mut FakeMem) -> Result<()> {
        let stride = usize::try_from(crate::offsets::items_txt::STRIDE)?;
        let mut b = vec![0u8; 50 * stride];
        for (row, code) in [(30, b"crs "), (40, b"uap ")] {
            b.get_mut(row * stride..row * stride + 4)
                .context("row in table")?
                .copy_from_slice(code);
        }
        mem.add(ITEMS, b);
        Ok(())
    }

    #[test]
    fn item_fields_sockets_and_bases() -> Result<()> {
        let mut m = world()?;
        let eth = item_data::FLAG_ETHEREAL | 0x0080_0000;
        // I0 (no path) heads the item chain, as in `world_with_items`.
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        // White eth 4os Crystal Sword: durability and damage stats around the socket stat.
        rich_item(&mut m, I1, (30, 2, eth, 0), &[(72, 50), (194, 4), (21, 5)], I2)?;
        // White 2os Crystal Sword: no rule at any tier, not shown.
        rich_item(&mut m, I2, (30, 2, 0, 0), &[(194, 2)], I3)?;
        // Unique Shako, identified, row 248; no stats at all.
        rich_item(&mut m, I3, (40, 7, item_data::FLAG_IDENTIFIED, 248), &[], I4)?;
        // White 3os Crystal Sword: a B rule, kept by the reader, removed by the default config.
        rich_item(&mut m, I4, (30, 3, 0, 0), &[(194, 3)], 0)?;
        base_items_table(&mut m)?;

        let mut s = snapshot(&m, TABLE, Some(ITEMS), None, None)?;
        assert!(
            s.warnings.iter().all(|w| !w.starts_with("item")),
            "{:?}",
            s.warnings
        );
        let got: Vec<_> = s
            .drops
            .iter()
            .map(|d| {
                (
                    d.id,
                    d.kind,
                    d.code.as_deref(),
                    d.sockets,
                    d.ethereal,
                    d.flags,
                    d.unique_index,
                )
            })
            .collect();
        let id = |a: u64| u32::try_from(a >> 12).unwrap_or_default();
        assert_eq!(
            got,
            vec![
                (id(I1), DropKind::Base, Some("crs"), 4, true, eth, None),
                (
                    id(I3),
                    DropKind::Unique,
                    Some("uap"),
                    0,
                    false,
                    item_data::FLAG_IDENTIFIED,
                    Some(248)
                ),
                (id(I4), DropKind::Base, Some("crs"), 3, false, 0, None),
            ]
        );
        let head = s
            .drops
            .first()
            .map_or_else(String::new, |d| d.item_data_head.clone());
        assert_eq!(head.len(), 2 * item_data::HEAD_LEN);
        assert_eq!(s.inventory_keep, vec![], "nothing in the inventory");
        assert!(head.starts_with("02000000"), "quality first: {head}");
        assert!(
            s.drops.iter().all(|d| d.highlight.is_none()),
            "the reader doesn't highlight"
        );

        valuables::apply(&mut s.drops, &config::Valuables::default());
        let tiers: Vec<_> = s
            .drops
            .iter()
            .map(|d| d.highlight.as_ref().map(|h| (h.tier, h.uncertain)))
            .collect();
        assert_eq!(
            tiers,
            vec![
                Some((valuables::Tier::A, false)),
                Some((valuables::Tier::S, false))
            ],
            "4os sword A, identified Shako S (certain); the 3os sword is gone"
        );
        Ok(())
    }

    #[test]
    fn keep_list_reads_the_inventory_page_only() -> Result<()> {
        let mut m = world()?;
        let id = item_data::FLAG_IDENTIFIED;
        let stored = item_mode::STORED;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        // Identified Shako (Harlequin Crest) in the inventory: kept, certain, by name.
        stored_item(&mut m, I1, (40, 7, id, 248), (stored, 0), &[], I2)?;
        // The same in the stash (page 4), and equipped (mode 1, page 255): not listed.
        stored_item(&mut m, I2, (40, 7, id, 248), (stored, 4), &[], I3)?;
        stored_item(&mut m, I3, (40, 7, id, 248), (1, 255), &[], I4)?;
        // A white 4os Crystal Sword in the inventory: A twink.
        stored_item(&mut m, I4, (30, 2, id, 0), (stored, 0), &[(194, 4)], I5)?;
        // A 3os one (B only): not kept.
        stored_item(&mut m, I5, (30, 2, id, 0), (stored, 0), &[(194, 3)], I6)?;
        // Another player's Shako in an inventory (group game): not kept.
        stored_item(&mut m, I6, (40, 7, id, 248), (stored, 0), &[], 0)?;
        for addr in [I1, I2, I4, I5] {
            set_owner(&mut m, addr, BOB_ID)?;
        }
        set_owner(&mut m, I6, ALICE_ID)?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!((s.ground_items, s.drops.len()), (0, 0));
        assert_eq!(s.inventory_items, 4, "page-0 stored items, any owner");
        let keep: Vec<_> = s
            .inventory_keep
            .iter()
            .map(|k| (k.name.as_str(), k.tier, k.count))
            .collect();
        assert_eq!(
            keep,
            vec![
                ("Harlequin Crest", valuables::Tier::S, 1),
                ("Crystal Sword 4os", valuables::Tier::A, 1),
            ]
        );
        Ok(())
    }

    /// An identified unique of items.txt row `txt` and uniqueitems row `index`, stored on `page`
    /// with `owner` ([`stored_item`] plus `item_data::OWNER`).
    fn owned_unique(
        mem: &mut FakeMem,
        addr: u64,
        (txt, index): (u32, u32),
        (page, owner): (u8, u32),
        next: u64,
    ) -> Result<()> {
        let fields = (txt, 7, item_data::FLAG_IDENTIFIED, index);
        stored_item(mem, addr, fields, (item_mode::STORED, page), &[], next)?;
        set_owner(mem, addr, owner)
    }

    /// Bytes of the `ItemData` [`stored_item`] lays out (up to `NEXT_IN_INVENTORY`).
    const ITEM_DATA_SIZE: usize = 0xC0;

    /// Overwrites `bytes` at `base + off` inside the `len`-byte region at `base`.
    fn patch(mem: &mut FakeMem, base: u64, len: usize, off: u64, bytes: &[u8]) -> Result<()> {
        let mut b = mem.read_bytes(base, len)?;
        let at = usize::try_from(off)?;
        b.get_mut(at..at + bytes.len())
            .context("patched field outside region")?
            .copy_from_slice(bytes);
        mem.add(base, b);
        Ok(())
    }

    /// Sets `item_data::OWNER` of the item at `addr` (laid out by [`stored_item`]).
    fn set_owner(mem: &mut FakeMem, addr: u64, owner: u32) -> Result<()> {
        patch(
            mem,
            addr + 0x200,
            ITEM_DATA_SIZE,
            item_data::OWNER,
            &owner.to_le_bytes(),
        )
    }

    /// A shared-stash tab unit: a player-type unit without act or path whose `Inventory`
    /// names `owner` ([`inventory::OWNER_ID`]). The id is the address >> 12.
    fn tab_unit(mem: &mut FakeMem, addr: u64, inventory: u64, next: u64) -> Result<()> {
        put(
            mem,
            addr,
            UNIT_SIZE,
            &[
                (unit::TYPE, &unit_table::PLAYER.to_le_bytes()),
                (unit::TXT, &1u32.to_le_bytes()),
                (unit::ID, &(u32::try_from(addr >> 12)?).to_le_bytes()),
                (unit::MODE, &1u32.to_le_bytes()),
                (unit::INVENTORY, &inventory.to_le_bytes()),
                (unit::NEXT, &next.to_le_bytes()),
            ],
        )
    }

    const TAB0: u64 = 0x20_3000; // unit id 0x203, Bob's live tab: page 2 in the UI
    const TAB1: u64 = 0x20_4000; // unit id 0x204, a stale tab (another character's)
    const TAB2: u64 = 0x20_5000; // unit id 0x205, Bob's live tab: page 1 in the UI
    const TAB_M: u64 = 0x20_6000; // unit id 0x206, Bob's gems/runes/materials tab
    /// Unit id of a listed tab whose unit isn't in the unit table: page 3, empty.
    const TAB_GONE_ID: u32 = 0x207;
    const INV0: u64 = 0x23_0000;
    const INV1: u64 = 0x23_1000;
    const INV2: u64 = 0x23_2000;
    const INV_M: u64 = 0x23_3000;
    const INV_BOB: u64 = 0x23_4000;
    /// The tab-list nodes hang off Bob's inventory ([`inventory::TAB_LIST`]).
    const NODES: u64 = 0x24_0000;
    const INV_SIZE: usize = 0x80;

    /// A tab unit's `Inventory` naming `owner`, its item chain starting at `first` (0: none).
    fn tab_inventory(mem: &mut FakeMem, addr: u64, owner: u32, first: u64) -> Result<()> {
        put(
            mem,
            addr,
            INV_SIZE,
            &[
                (inventory::OWNER_ID, &owner.to_le_bytes()),
                (inventory::FIRST_ITEM, &first.to_le_bytes()),
            ],
        )
    }

    /// Links the [`stored_item`]s `items` into one inventory chain
    /// ([`item_data::NEXT_IN_INVENTORY`]) in order; returns the chain head (0 when empty).
    fn chain_items(mem: &mut FakeMem, items: &[u64]) -> Result<u64> {
        for pair in items.windows(2) {
            if let [item, next] = *pair {
                let next = next.to_le_bytes();
                patch(
                    mem,
                    item + 0x200,
                    ITEM_DATA_SIZE,
                    item_data::NEXT_IN_INVENTORY,
                    &next,
                )?;
            }
        }
        Ok(items.first().copied().unwrap_or(0))
    }

    /// Sets `item_data::QUANTITY` of the [`stored_item`] at `addr`.
    fn set_quantity(mem: &mut FakeMem, addr: u64, quantity: u32) -> Result<()> {
        patch(
            mem,
            addr + 0x200,
            ITEM_DATA_SIZE,
            item_data::QUANTITY,
            &quantity.to_le_bytes(),
        )
    }

    /// Bob's `Inventory` with the tab list `nodes` ((tab unit id, advanced byte), UI order),
    /// its header (first, last, `count`) at `header` (one of [`inventory::TAB_LIST`]).
    fn bob_tab_list(mem: &mut FakeMem, header: u64, nodes: &[(u32, u8)], count: u32) -> Result<()> {
        patch(mem, P_BOB, UNIT_SIZE, unit::INVENTORY, &INV_BOB.to_le_bytes())?;
        let at = |i: usize| -> Result<u64> { Ok(NODES + 0x100 * u64::try_from(i)?) };
        let (first, last) = match nodes.len() {
            0 => (0, 0),
            n => (NODES, at(n - 1)?),
        };
        put(
            mem,
            INV_BOB,
            INV_SIZE,
            &[
                (header, &first.to_le_bytes()),
                (header + inventory::TAB_LIST_LAST, &last.to_le_bytes()),
                (header + inventory::TAB_LIST_COUNT, &count.to_le_bytes()),
            ],
        )?;
        for (i, &(id, advanced)) in nodes.iter().enumerate() {
            let next = if i + 1 < nodes.len() { at(i + 1)? } else { 0 };
            put(
                mem,
                at(i)?,
                0x18,
                &[
                    (stash_tab_node::TAB_ID, &id.to_le_bytes()),
                    (stash_tab_node::ADVANCED, &[advanced]),
                    (stash_tab_node::NEXT, &next.to_le_bytes()),
                ],
            )?;
        }
        Ok(())
    }

    /// Bob's tab list in [`world_with_tabs`]: `TAB2`, `TAB0`, [`TAB_GONE_ID`], `TAB_M`.
    const BOB_TABS: [(u32, u8); 4] = [(0x205, 0), (0x203, 0), (TAB_GONE_ID, 0), (0x206, 1)];

    /// [`world`] plus the tab units: `TAB2`, `TAB0` and `TAB_M` name Bob their inventory
    /// owner (live), `TAB1` names Alice (stale while playing Bob). The unit table holds them
    /// in another order than Bob's tab list (`TAB2`, `TAB0`, [`TAB_GONE_ID`], `TAB_M`), which
    /// is the UI order. No tab has items; the tests chain them in ([`tab_inventory`]).
    fn world_with_tabs() -> Result<FakeMem> {
        let mut m = world()?;
        table(
            &mut m,
            &[
                (unit_table::PLAYER, 2, TAB_M),
                (unit_table::PLAYER, 3, P_STALE),
                (unit_table::PLAYER, 4, TAB0),
                (unit_table::PLAYER, 5, TAB2),
                (unit_table::PLAYER, 6, TAB1),
                (unit_table::PLAYER, 9, P_BOB),
                (unit_table::MONSTER, 0, M0),
                (unit_table::ITEM, 2, I0),
            ],
        )?;
        for (tab, inv, owner) in [
            (TAB0, INV0, BOB_ID),
            (TAB1, INV1, ALICE_ID),
            (TAB2, INV2, BOB_ID),
            (TAB_M, INV_M, BOB_ID),
        ] {
            tab_unit(&mut m, tab, inv, 0)?;
            tab_inventory(&mut m, inv, owner, 0)?;
        }
        bob_tab_list(&mut m, 0x60, &BOB_TABS, 4)?;
        Ok(m)
    }

    #[test]
    fn shared_stash_tabs_follow_the_ui_order_and_their_item_chains() -> Result<()> {
        use crate::stash::{StashItem, StashTab, TabKind};
        const I7: u64 = 0x80_7000;
        let mut m = world_with_tabs()?;
        // The unit-table item chain (I0 → …) is irrelevant to the stash: each tab chains its
        // own items. items.txt row 40 uap (Shako), uniqueitems 248 Harlequin Crest; row 30 crs.
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, 0)?;
        owned_unique(&mut m, I1, (40, 248), (4, 0x203), 0)?; // page 2: unique
        stored_item(&mut m, I2, (30, 2, 0, 0), (item_mode::STORED, 4), &[], 0)?;
        set_owner(&mut m, I2, 0x203)?; // page 2: normal base
        owned_unique(&mut m, I3, (40, 248), (4, 0x204), 0)?; // stale tab: out
        // The materials tab: stacks owned by nobody, with their stack sizes.
        for (addr, row, qty) in [(I4, 45, 8), (I5, 46, 19), (I6, 47, 0)] {
            stored_item(&mut m, addr, (row, 2, 0, 0), (item_mode::STORED, 4), &[], 0)?;
            set_owner(&mut m, addr, item_data::NO_OWNER)?;
            set_quantity(&mut m, addr, qty)?;
        }
        // A page-1 item with a stack field that must not be read as a quantity.
        stored_item(&mut m, I7, (30, 2, 0, 0), (item_mode::STORED, 4), &[], 0)?;
        set_owner(&mut m, I7, 0x205)?;
        set_quantity(&mut m, I7, 5)?;
        let page2 = chain_items(&mut m, &[I1, I2])?;
        tab_inventory(&mut m, INV0, BOB_ID, page2)?;
        let stale = chain_items(&mut m, &[I3])?;
        tab_inventory(&mut m, INV1, ALICE_ID, stale)?;
        let materials = chain_items(&mut m, &[I4, I5, I6])?;
        tab_inventory(&mut m, INV_M, BOB_ID, materials)?;
        tab_inventory(&mut m, INV2, BOB_ID, I7)?;
        let stride = usize::try_from(crate::offsets::items_txt::STRIDE)?;
        let mut b = vec![0u8; 50 * stride];
        for (row, code) in [
            (30, b"crs "),
            (40, b"uap "),
            (45, b"r01 "),
            (46, b"gcy "),
            (47, b"gzv "),
        ] {
            b.get_mut(row * stride..row * stride + 4)
                .context("row in table")?
                .copy_from_slice(code);
        }
        m.add(ITEMS, b);

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert!(
            s.warnings.iter().all(|w| !w.contains("stash")),
            "{:?}",
            s.warnings
        );
        let item = |name: &str, code: &str, quality: u8, identified: bool, quantity| StashItem {
            name: name.to_owned(),
            code: Some(code.to_owned()),
            quality,
            identified,
            ethereal: false,
            runeword: false,
            quantity,
        };
        assert_eq!(
            s.stash,
            vec![
                StashTab {
                    kind: TabKind::Page(1),
                    items: vec![item("Crystal Sword", "crs", 2, false, None)],
                },
                StashTab {
                    kind: TabKind::Page(2),
                    items: vec![
                        item("Harlequin Crest", "uap", 7, true, None),
                        item("Crystal Sword", "crs", 2, false, None),
                    ],
                },
                StashTab {
                    kind: TabKind::Page(3),
                    items: vec![],
                },
                StashTab {
                    kind: TabKind::Materials,
                    items: vec![
                        item("El Rune", "r01", 2, false, Some(8)),
                        item("Chipped Topaz", "gcy", 2, false, Some(19)),
                        item("Flawless Amethyst", "gzv", 2, false, Some(0)),
                    ],
                },
            ],
            "UI order from Bob's tab list; the stale tab is out"
        );
        assert_eq!(s.stash_names, vec!["Harlequin Crest"]);
        Ok(())
    }

    #[test]
    fn stash_tabs_without_items_still_show() -> Result<()> {
        use crate::stash::{StashTab, TabKind};
        let m = world_with_tabs()?;
        let s = snapshot(&m, TABLE, None, Some("Bob"), None)?;
        let empty = |kind| StashTab { kind, items: vec![] };
        assert_eq!(
            s.stash,
            vec![
                empty(TabKind::Page(1)),
                empty(TabKind::Page(2)),
                empty(TabKind::Page(3)),
                empty(TabKind::Materials),
            ],
            "every listed tab, the one without a unit too"
        );
        assert_eq!(s.stash_names, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn the_tab_list_header_is_found_at_either_offset_and_checked() -> Result<()> {
        use crate::stash::TabKind;
        let kinds = |m: &FakeMem| -> Result<Vec<TabKind>> {
            let s = snapshot(m, TABLE, None, Some("Bob"), None)?;
            Ok(s.stash.iter().map(|t| t.kind).collect())
        };
        let ui = vec![
            TabKind::Page(1),
            TabKind::Page(2),
            TabKind::Page(3),
            TabKind::Materials,
        ];
        // The header one qword later (live in the second game): `+0x60` is null.
        let mut m = world_with_tabs()?;
        bob_tab_list(&mut m, 0x68, &BOB_TABS, 4)?;
        assert_eq!(kinds(&m)?, ui);
        // A count that doesn't match the walk is no header: memory-order fallback.
        let mut m = world_with_tabs()?;
        bob_tab_list(&mut m, 0x60, &BOB_TABS, 3)?;
        assert_eq!(
            kinds(&m)?,
            vec![TabKind::Page(1), TabKind::Page(2), TabKind::Page(3)]
        );
        Ok(())
    }

    #[test]
    fn without_a_tab_list_the_tab_units_are_pages_in_memory_order() -> Result<()> {
        use crate::stash::{StashTab, TabKind};
        let mut m = world_with_tabs()?;
        bob_tab_list(&mut m, 0x60, &[], 0)?;
        let s = snapshot(&m, TABLE, None, Some("Bob"), None)?;
        let pages: Vec<_> = s.stash.iter().map(|t| t.kind).collect();
        assert_eq!(
            pages,
            vec![TabKind::Page(1), TabKind::Page(2), TabKind::Page(3)],
            "the three live tab units (stale one out), no materials tab"
        );
        assert!(
            s.warnings.iter().any(|w| w.contains("tab list unreadable")),
            "{:?}",
            s.warnings
        );

        // No tab list and no live tab units (the stash not loaded): nothing, no warning.
        let mut m = world()?;
        bob_tab_list(&mut m, 0x60, &[], 0)?;
        let s = snapshot(&m, TABLE, None, Some("Bob"), None)?;
        assert_eq!(s.stash, Vec::<StashTab>::new());
        assert!(
            s.warnings.iter().all(|w| !w.contains("stash")),
            "{:?}",
            s.warnings
        );
        Ok(())
    }

    #[test]
    fn place_counts_only_the_players_own_shared_tabs() {
        use valuables::Place;
        let (me, tab, stale) = (7, 70, 71);
        let tabs = [tab];
        assert_eq!(place(item_data::PAGE_STASH, me, me, &tabs), Some(Place::Stash));
        assert_eq!(place(item_data::PAGE_STASH, tab, me, &tabs), Some(Place::Shared));
        assert_eq!(
            place(item_data::PAGE_STASH, stale, me, &tabs),
            None,
            "stale tab unit"
        );
        assert_eq!(place(item_data::PAGE_STASH, item_data::NO_OWNER, me, &tabs), None);
        assert_eq!(
            place(item_data::PAGE_INVENTORY, tab, me, &tabs),
            None,
            "tabs only own stash pages"
        );
    }

    #[test]
    fn spare_lists_cover_the_players_inventory_cube_and_stashes() -> Result<()> {
        const I7: u64 = 0x80_7000;
        const I8: u64 = 0x80_8000;
        const SHARED_TAB: u32 = 0x7777;
        // items.txt rows 30 crs, 41 tgl, 42 hdm; uniqueitems 12 Gravenspine, 105 Magefist.
        let (grave, magefist) = ((30, 12), (41, 105));
        let mut m = world()?;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        owned_unique(&mut m, I1, grave, (0, BOB_ID), I2)?; // Bob's inventory: sell
        owned_unique(&mut m, I2, grave, (0, ALICE_ID), I3)?; // another player's inventory
        owned_unique(&mut m, I3, magefist, (4, SHARED_TAB), I4)?; // shared stash: give away
        owned_unique(&mut m, I4, grave, (4, item_data::NO_OWNER), I5)?; // unknown page-4 items
        owned_unique(&mut m, I5, grave, (3, BOB_ID), I6)?; // Bob's cube: sell
        owned_unique(&mut m, I6, grave, (4, ALICE_ID), I7)?; // another player's stash id
        owned_unique(&mut m, I7, magefist, (4, BOB_ID), I8)?; // Bob's stash: give away
        owned_unique(&mut m, I8, (42, 12), (4, BOB_ID), 0)?; // quest code hdm: never listed
        let stride = usize::try_from(crate::offsets::items_txt::STRIDE)?;
        let mut b = vec![0u8; 50 * stride];
        for (row, code) in [(30, b"crs "), (41, b"tgl "), (42, b"hdm ")] {
            b.get_mut(row * stride..row * stride + 4)
                .context("row in table")?
                .copy_from_slice(code);
        }
        m.add(ITEMS, b);

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        let names =
            |l: &[valuables::Spare]| -> Vec<String> { l.iter().map(crate::info::spare_entry).collect() };
        assert_eq!(names(&s.sell), vec!["Gravenspine (inv)", "Gravenspine (cube)"]);
        // Other-owner page-4 items are not the shared stash (stale units); left out for now.
        assert_eq!(names(&s.give_away), vec!["Magefist (stash)"]);
        assert_eq!(
            s.inventory_items, 2,
            "the keep scan still sees every inventory item"
        );
        // Empty lists stay out of the JSON.
        let mut quiet = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        quiet.sell.clear();
        quiet.give_away.clear();
        let json = serde_json::to_string(&quiet)?;
        assert!(!json.contains("sell") && !json.contains("give_away"), "{json}");
        Ok(())
    }

    #[test]
    fn keep_one_sees_inventory_and_stash_together() -> Result<()> {
        // items.txt row 43 ghm; setitems 36 Sigon's Visor (keep one).
        let visor = (43, 5, item_data::FLAG_IDENTIFIED, 36);
        let stored = item_mode::STORED;
        let mut m = world()?;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        stored_item(&mut m, I1, visor, (stored, 0), &[], I2)?; // inventory
        stored_item(&mut m, I2, visor, (stored, 4), &[], 0)?; // personal stash: the kept copy
        for addr in [I1, I2] {
            set_owner(&mut m, addr, BOB_ID)?;
        }
        let stride = usize::try_from(crate::offsets::items_txt::STRIDE)?;
        let mut b = vec![0u8; 50 * stride];
        b.get_mut(43 * stride..43 * stride + 4)
            .context("row in table")?
            .copy_from_slice(b"ghm ");
        m.add(ITEMS, b);

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        let names =
            |l: &[valuables::Spare]| -> Vec<String> { l.iter().map(crate::info::spare_entry).collect() };
        assert_eq!(s.inventory_keep, vec![], "the inventory copy is not kept");
        assert_eq!(names(&s.sell), Vec::<String>::new());
        assert_eq!(names(&s.give_away), vec!["Sigon's Visor (inv)"]);
        Ok(())
    }

    #[test]
    fn sockets_absent_or_unreadable_count_as_zero() -> Result<()> {
        let mut m = FakeMem::default();
        let r = Reader {
            mem: &m,
            warnings: Vec::new(),
            suppressed: 0,
        };
        assert!(r.sockets(UNMAPPED).is_err(), "unreadable unit");
        drop(r);
        put(&mut m, I1, UNIT_SIZE, &[])?;
        let r = Reader {
            mem: &m,
            warnings: Vec::new(),
            suppressed: 0,
        };
        assert_eq!(r.sockets(I1)?, 0, "null stat list");
        Ok(())
    }

    #[test]
    fn ground_drops() -> Result<()> {
        let m = world_with_items()?;
        let s = snapshot(&m, TABLE, Some(ITEMS), None, None)?;
        assert_eq!(
            s.ground_items, 5,
            "I0 (mode 1) and I5 (mode 0) are not on the ground"
        );
        assert_eq!(
            s.drops,
            vec![
                drop_of(
                    I1,
                    Some("gcg"),
                    DropKind::Gem(items::GemColour::Emerald),
                    (5323, 5622)
                ),
                drop_of(I3, Some("axe"), DropKind::Unique, (12, 13)),
                drop_of(I4, Some("r01"), DropKind::Rune(1), (14, 15)),
                drop_of(I6, Some("axe"), DropKind::Set, (18, 19)),
            ],
            "static-path positions; the normal axe and the inventory rune are left out"
        );
        assert!(
            s.warnings.iter().all(|w| !w.starts_with("item")),
            "{:?}",
            s.warnings
        );

        // Without the items table only quality tells: unique and set, no codes.
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(s.ground_items, 5);
        assert_eq!(
            s.drops,
            vec![
                drop_of(I3, None, DropKind::Unique, (12, 13)),
                drop_of(I6, None, DropKind::Set, (18, 19)),
            ]
        );
        Ok(())
    }

    #[test]
    fn player_by_name() -> Result<()> {
        let mem = world()?;
        let s = snapshot(&mem, TABLE, None, Some("Bob"), None)?;
        assert_eq!((s.player.addr, s.player.class, s.player.x), (Addr(P_BOB), 5, 95));
        let err = snapshot(&mem, TABLE, None, Some("Ghost"), None)
            .err()
            .context("stale player must not match")?;
        assert!(err.to_string().contains("\"Alice\", \"Bob\""), "{err}");
        Ok(())
    }

    #[test]
    fn no_player_is_an_error() -> Result<()> {
        let mut mem = world()?;
        table(&mut mem, &[(unit_table::PLAYER, 3, P_STALE)])?;
        unit(&mut mem, P_STALE, 0, 1, PD_STALE, 0, 0, 0)?;
        let err = snapshot(&mem, TABLE, None, None, None)
            .err()
            .context("expected error")?;
        assert!(err.to_string().contains("not in a game?"), "{err}");
        Ok(())
    }

    #[test]
    fn self_looping_chain_terminates() -> Result<()> {
        let mut mem = world()?;
        let mut slots = BASE_SLOTS.to_vec();
        slots.push((unit_table::MONSTER, 5, M2));
        table(&mut mem, &slots)?;
        unit(&mut mem, M2, 1, 42, MD0, ACT, PATH_M2, M2)?;
        unit_path(&mut mem, PATH_M2, 70, 120, ROOM1_0)?;
        let s = snapshot(&mem, TABLE, None, None, None)?;
        assert_eq!(
            s.monsters.iter().map(|m| m.txt).collect::<Vec<_>>(),
            vec![40, 41, 42]
        );
        assert!(s.warnings.iter().any(|w| w.contains("cycle")), "{:?}", s.warnings);
        Ok(())
    }

    #[test]
    fn unreadable_unit_is_a_warning() -> Result<()> {
        let mut mem = world()?;
        unit(&mut mem, M1, 1, 41, MD1, ACT, PATH_M1, UNMAPPED)?;
        let s = snapshot(&mem, TABLE, None, None, None)?;
        assert_eq!(s.monsters.len(), 2);
        assert!(
            s.warnings
                .iter()
                .any(|w| w.starts_with(&format!("monster {UNMAPPED:#x}"))),
            "{:?}",
            s.warnings
        );
        Ok(())
    }

    const O0: u64 = 0xA0_0000;
    const O1: u64 = 0xA0_1000;
    const O2: u64 = 0xA0_2000;
    const O3: u64 = 0xA0_3000;
    const O4: u64 = 0xA0_4000;
    const O5: u64 = 0xA0_5000;

    /// An object unit with its `ObjectData` (shrine type) at `addr + 0x200` and static path
    /// (position) at `addr + 0x400`.
    fn object(
        mem: &mut FakeMem,
        addr: u64,
        (txt, mode, shrine_type): (u32, u32, u8),
        (x, y): (u32, u32),
        next: u64,
    ) -> Result<()> {
        let (data, path) = (addr + 0x200, addr + 0x400);
        put(
            mem,
            addr,
            UNIT_SIZE,
            &[
                (unit::TYPE, &unit_table::OBJECT.to_le_bytes()),
                (unit::TXT, &txt.to_le_bytes()),
                (unit::ID, &(u32::try_from(addr >> 12)?).to_le_bytes()),
                (unit::MODE, &mode.to_le_bytes()),
                (unit::UNIT_DATA, &data.to_le_bytes()),
                (unit::PATH, &path.to_le_bytes()),
                (unit::NEXT, &next.to_le_bytes()),
            ],
        )?;
        put(mem, data, 0x10, &[(object_data::SHRINE_TYPE, &[shrine_type])])?;
        put(
            mem,
            path,
            0x20,
            &[
                (path::X, &22407u16.to_le_bytes()),
                (static_path::X, &x.to_le_bytes()),
                (static_path::Y, &y.to_le_bytes()),
            ],
        )
    }

    #[test]
    fn shrines_and_wells() -> Result<()> {
        let mut m = world()?;
        let mut slots = BASE_SLOTS.to_vec();
        slots.push((unit_table::OBJECT, 5, O0));
        table(&mut m, &slots)?;
        // O0 used health shrine (236, live) → O1 skill shrine → O2 used well → O3 chest →
        // O4 shrine object without a shrine type → O5 well with an unreadable path.
        object(&mut m, O0, (236, 2, 2), (5100, 5200), O1)?;
        object(&mut m, O1, (2, 0, 12), (5110, 5210), O2)?;
        object(&mut m, O2, (130, 2, 0), (5120, 5220), O3)?;
        object(&mut m, O3, (5, 0, 0), (5130, 5230), O4)?;
        object(&mut m, O4, (77, 0, 0), (5140, 5240), O5)?;
        put(
            &mut m,
            O5,
            UNIT_SIZE,
            &[
                (unit::TYPE, &unit_table::OBJECT.to_le_bytes()),
                (unit::TXT, &132u32.to_le_bytes()),
                (unit::PATH, &UNMAPPED.to_le_bytes()),
            ],
        )?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        let shrine = |txt, code, used, x, y| Shrine {
            txt,
            code,
            used,
            x,
            y,
        };
        assert_eq!(
            s.shrines,
            vec![shrine(236, 2, true, 5100, 5200), shrine(2, 12, false, 5110, 5210)],
            "mode 2 is used; type 0 skipped"
        );
        assert_eq!(
            s.wells,
            vec![Well {
                txt: 130,
                x: 5120,
                y: 5220
            }]
        );
        let object_warnings: Vec<_> = s.warnings.iter().filter(|w| w.starts_with("object")).collect();
        assert_eq!(
            object_warnings.len(),
            1,
            "only the unreadable well: {object_warnings:?}"
        );
        assert!(
            object_warnings
                .iter()
                .all(|w| w.starts_with(&format!("object {O5:#x}")))
        );
        // Older dumps without the fields still load; empty lists stay out of the JSON.
        let v = serde_json::to_value(snapshot(&world()?, TABLE, None, None, None)?)?;
        assert!(v.get("shrines").is_none() && v.get("wells").is_none());
        let v = serde_json::to_value(&s)?;
        assert_eq!(
            v.pointer("/shrines/1"),
            Some(&serde_json::json!({"txt": 2, "code": 12, "used": false, "x": 5110, "y": 5210}))
        );
        Ok(())
    }

    #[test]
    fn operated_seals() -> Result<()> {
        let mut m = world()?;
        let mut slots = BASE_SLOTS.to_vec();
        slots.push((unit_table::OBJECT, 5, O0));
        table(&mut m, &slots)?;
        // O0 operated seal 392 → O1 untouched seal 393 → O2 operating seal 396 (mode 1) →
        // O3 used shrine: a shrine, not an operated quest object.
        object(&mut m, O0, (392, 2, 0), (7800, 5300), O1)?;
        object(&mut m, O1, (393, 0, 0), (7820, 5310), O2)?;
        object(&mut m, O2, (396, 1, 0), (7700, 5290), O3)?;
        object(&mut m, O3, (236, 2, 2), (7710, 5280), 0)?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        let op = |txt, x, y| OperatedObject { txt, x, y };
        assert_eq!(
            s.operated_objects,
            vec![op(392, 7800, 5300), op(396, 7700, 5290)],
            "mode 0 is untouched"
        );
        assert_eq!(s.shrines.len(), 1);
        assert!(
            s.warnings.iter().all(|w| !w.starts_with("object")),
            "{:?}",
            s.warnings
        );
        // Empty stays out of the JSON; a listed one round-trips.
        let v = serde_json::to_value(snapshot(&world()?, TABLE, None, None, None)?)?;
        assert!(v.get("operated_objects").is_none());
        let v = serde_json::to_value(&s)?;
        assert_eq!(
            v.pointer("/operated_objects/0"),
            Some(&serde_json::json!({"txt": 392, "x": 7800, "y": 5300}))
        );
        Ok(())
    }

    #[test]
    fn object_classification_tables() {
        assert!(SHRINE_OBJECTS.len() > 50 && WELL_OBJECTS.len() > 5);
        assert!(SHRINE_OBJECTS.is_sorted_by(|a, b| a < b), "sorted, unique");
        assert!(WELL_OBJECTS.is_sorted_by(|a, b| a < b), "sorted, unique");
        // 236 "health wellforhell" was a Health shrine live (2026-11-15).
        for txt in [2, 236, 249, 81, 83] {
            assert!(is_shrine_object(txt) && !is_well_object(txt), "{txt}");
        }
        for txt in [130, 132, 137, 138, 111] {
            assert!(is_well_object(txt) && !is_shrine_object(txt), "{txt}");
        }
        // Waypoint, chest, an unusable fountain: neither.
        for txt in [119, 5, 31] {
            assert!(!is_well_object(txt) && !is_shrine_object(txt), "{txt}");
        }
        assert_eq!(
            usize::from(*SHRINE_TYPES.end()) + 1,
            crate::config::SHRINE_NAMES.len(),
            "every named shrine type"
        );
    }

    #[test]
    fn warnings_are_capped() {
        let mem = FakeMem::default();
        let mut r = Reader {
            mem: &mem,
            warnings: Vec::new(),
            suppressed: 0,
        };
        for i in 0..60 {
            r.warn(format!("w{i}"));
        }
        assert_eq!((r.warnings.len(), r.suppressed), (MAX_WARNINGS, 10));
    }

    #[test]
    fn repeated_presets_are_kept_once() -> Result<()> {
        let mut m = world()?;
        // R2 moves onto R0's origin and gets a list repeating R0's waypoint twice, plus a
        // monster preset at the same spot (different kind: kept).
        room(&mut m, R2, (10, 20, 4, 4), 0, 0, LEVEL, PR_DUP0, 0)?;
        preset_unit(&mut m, PR_DUP0, preset::OBJECT, 119, 3, 4, PR_DUP1)?;
        preset_unit(&mut m, PR_DUP1, preset::OBJECT, 119, 3, 4, PR_DUP2)?;
        preset_unit(&mut m, PR_DUP2, preset::MONSTER, 119, 3, 4, 0)?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        let got: Vec<(PresetKind, u32, u32, u32)> =
            s.presets.iter().map(|p| (p.kind, p.txt, p.x, p.y)).collect();
        assert_eq!(
            got,
            vec![
                (PresetKind::Object, 119, 53, 104),
                (PresetKind::Tile, 7, 51, 102),
                (PresetKind::Other(9), 1, 50, 100),
                (PresetKind::Monster, 853, 92, 103),
                (PresetKind::Monster, 119, 53, 104),
            ],
            "first occurrence order, repeats dropped"
        );
        Ok(())
    }

    #[test]
    fn difficulty_values() {
        assert_eq!(difficulty_from_raw(0), Some(0));
        assert_eq!(difficulty_from_raw(1), Some(1));
        assert_eq!(difficulty_from_raw(2), Some(2));
        for raw in [3, 7, 0x100, u16::MAX] {
            assert_eq!(difficulty_from_raw(raw), None, "{raw}");
        }
    }

    #[test]
    fn difficulty_via_act_misc() -> Result<()> {
        const ACT_MISC: u64 = 0x23_0000;
        let mut m = world()?;
        // The act isn't mapped in the base world: no difficulty, no warning, not in the JSON.
        let s = snapshot(&m, TABLE, None, None, None)?;
        assert_eq!(s.difficulty, None);
        assert_eq!(serde_json::to_value(&s)?.pointer("/difficulty"), None);
        put(&mut m, ACT, 0x80, &[(act::MISC, &ACT_MISC.to_le_bytes())])?;
        for (raw, want) in [(0u16, Some(0)), (2, Some(2)), (5, None)] {
            put(
                &mut m,
                ACT_MISC,
                0x840,
                &[(act_misc::DIFFICULTY, &raw.to_le_bytes())],
            )?;
            let s = snapshot(&m, TABLE, None, None, None)?;
            assert_eq!(s.difficulty, want, "raw {raw}");
        }
        Ok(())
    }

    #[test]
    fn towns() {
        for id in [1, 40, 75, 103, 109] {
            assert!(is_town(id), "{id}");
        }
        for id in [0, 2, 29, 39, 41, 108, 110] {
            assert!(!is_town(id), "{id}");
        }
    }

    #[test]
    fn json_shape() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None, None, None)?;
        let v = serde_json::to_value(&s)?;
        let at = |p: &str| v.pointer(p).cloned().unwrap_or_default();
        assert_eq!(at("/player/addr"), "0x201000");
        assert_eq!(at("/presets/0/kind"), "object");
        assert_eq!(at("/presets/2/kind"), 9);
        let s = snapshot(&world_with_items()?, TABLE, Some(ITEMS), None, None)?;
        let v = serde_json::to_value(&s)?;
        assert_eq!(
            v.pointer("/drops/0"),
            Some(
                &serde_json::json!({"id": 0x801, "code": "gcg", "kind": {"gem": "emerald"}, "quality": 2,
                    "flags": 0, "ethereal": false, "sockets": 0, "unique_index": null,
                    "x": 5323, "y": 5622})
            )
        );
        assert_eq!(v.pointer("/drops/1/kind"), Some(&serde_json::json!("unique")));
        // The ItemData hex is journal-only; a highlight shows once set, and parses back.
        let mut s = s;
        assert!(v.pointer("/drops/0/item_data_head").is_none());
        let Some(d) = s.drops.first_mut() else {
            bail!("no drop")
        };
        d.highlight = Some(valuables::Highlight {
            tier: valuables::Tier::A,
            category: valuables::Category::Twink,
            rule: "r".into(),
            uncertain: true,
        });
        let v = serde_json::to_value(&s)?;
        assert_eq!(
            v.pointer("/drops/0/highlight"),
            Some(&serde_json::json!({"tier": "A", "category": "twink", "rule": "r", "uncertain": true}))
        );
        let back: Snapshot = serde_json::from_value(v.clone())?;
        assert_eq!(
            back.drops.first().and_then(|d| d.highlight.clone()),
            s.drops.first().and_then(|d| d.highlight.clone())
        );
        assert_eq!(v.pointer("/drops/2/kind"), Some(&serde_json::json!({"rune": 1})));
        Ok(())
    }
}
