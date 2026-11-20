//! The game state model: one snapshot of what the reader found (player, level layout, presets,
//! monsters, ground drops, nearby shrines and wells, the player's items), serializable as the
//! `--dump` JSON. The reader plugin fills it (`d2r_reader_mem::game`); the map, the session, the
//! journal, the dashboard and the playstyle read it.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::items::DropKind;
use crate::objects_data::{SHRINE_OBJECTS, WELL_OBJECTS};
use crate::offsets::{item_data, preset};
use crate::stash::{self, StashTab};

/// Living desecrated monsters needed to call a level terrorized. Two rather than one, so a
/// single misread unit can't flip the info line; the player's summons never carry the state.
pub const TERROR_MIN_MONSTERS: usize = 2;

/// Level ids of the five towns (levels.txt): Rogue Encampment, Lut Gholein, Kurast Docks,
/// Pandemonium Fortress, Harrogath.
pub const TOWN_LEVELS: [u32; 5] = [1, 40, 75, 103, 109];

/// True if `level_id` is a town (the overlay is hidden there).
#[must_use]
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
    /// Display names of identified unique/set items and named runewords the player owns
    /// (equipped, carried, cubed, privately stashed): the `BiS` list marks owned pieces with a
    /// ✓ (the worn ones apart: [`worn_names`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owned_names: Vec<String>,
    /// One name per runeword copy the player has outside the shared stash (worn, carried,
    /// cubed, privately stashed) or the mercenary wears (solo games only: see
    /// [`Snapshot::merc_worn`]), duplicates kept: the Recipes panel
    /// keeps a runeword recipe until every slot of the stage that wants it can be filled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runeword_copies: Vec<String>,
    /// Display names of the identified unique/set items and the runewords the mercenary wears,
    /// one per item: its uniques/sets count as owned for a stage switch and the farm list
    /// ([`crate::stash::Holdings`]; its runewords count there through
    /// [`Snapshot::runeword_copies`]), and every name counts as worn for the farm list's
    /// hiding rule (`playstyle::web::farm::farm_worn`); none are in [`Snapshot::owned_names`] (the `BiS` ✓ of
    /// the character's own pieces). Empty when other players are in the game: their mercs'
    /// gear can't be told from the local player's own merc's (PLAN.md, "Merc gear caveat").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merc_worn: Vec<String>,
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
    /// The ground items worth showing: uniques, sets, gems, runes and charms, plus every base
    /// with a known code. The playstyle keeps only the bases it marks
    /// (`d2r_playstyle_config::valuables::apply`) before anything draws or records them.
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
    /// [`crate::objects::CONSUMABLE_QUEST_OBJECTS`]). The session hides their presets for the game.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operated_objects: Vec<OperatedObject>,
    /// Items in the player's inventory (shown or not); live-mode only, not in the JSON.
    #[serde(skip)]
    pub inventory_items: u32,
    /// The player's own items with their [`Place`]: every inventory item, and the cube and
    /// stash items that are identified uniques or sets ([`identified_unique_or_set`]), magic or
    /// better items with their stats, or white runeword bases (only these are read in full;
    /// `d2r_reader_mem::game::stored_kept`). The playstyle's keep / sell / give-away lists are computed from
    /// them (`d2r_playstyle_config::stash_plan::held`). Left out of the JSON when empty;
    /// older dumps carry the computed lists instead, which are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owned_items: Vec<(ItemDrop, Place)>,
    /// The runes and gems the player holds in inventory, cube and stash pages (the shared
    /// materials tab is in [`Snapshot::stash`]), `(name, count)` ([`stash::held_materials`]):
    /// the dashboard's recipe ✓. Live-mode only, not in the JSON.
    #[serde(skip)]
    pub held_materials: Vec<(String, u32)>,
    /// The player's white socketed bases without a runeword ([`stash::white_bases`]): the
    /// dashboard's recipe base hint. Live-mode only, not in the JSON.
    #[serde(skip)]
    pub white_bases: Vec<String>,
    /// 0 Normal, 1 Nightmare, 2 Hell; `None` if unreadable (and in old fixtures).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u8>,
    /// The real Tal Rasha's Tomb of this game (level 66..=72, [`crate::tombs`]), read in Act 2;
    /// `None` elsewhere, if unreadable, and in old fixtures. Left out of the JSON when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staff_tomb: Option<u32>,
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
    /// The vendor trade window open right now ([`crate::shop`]); read in towns only. Left
    /// out of the JSON when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop: Option<crate::shop::Shop>,
    pub warnings: Vec<String>,
}

/// The player's skill allocation, read from the skill list ([`crate::offsets::skill`]): the
/// class skills with hard (allocated) points and the skills on the two mouse buttons. Bonus
/// levels from gear are not included (the skill nodes don't carry them).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skills {
    /// `(skills.txt id, hard points)` of the player's own class skills with points, in list
    /// order ([`crate::skills_data::SKILLS`] has their tree places).
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
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.points.is_empty() && self.left.is_none() && self.right.is_none()
    }
}

/// True if at least [`TERROR_MIN_MONSTERS`] living monsters carry the desecrated state.
#[must_use]
pub fn is_terrorized(monsters: &[Monster]) -> bool {
    monsters.iter().filter(|m| m.desecrated && !m.is_dead()).count() >= TERROR_MIN_MONSTERS
}

/// True if state `state` is set in the state bitfield `flags` (see
/// [`crate::offsets::stat_list::STATE_FLAGS`]: little-endian u32 dwords, bit `n % 32` of dword `n / 32`). A
/// state past the end of `flags` is clear.
#[must_use]
pub fn has_state(flags: &[u8], state: u32) -> bool {
    let (dwords, _) = flags.as_chunks::<4>();
    usize::try_from(state / 32)
        .ok()
        .and_then(|i| dwords.get(i))
        .is_some_and(|&d| u32::from_le_bytes(d) & (1 << (state % 32)) != 0)
}

/// The difficulty from the raw `ActMisc` value; anything above Hell (2) is unknown.
#[must_use]
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
    /// Character level (stat [`crate::offsets::stat_list::LEVEL`], see [`char_level_from_raw`]); `None` if
    /// unreadable (and in old fixtures, where it is left out of the JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_level: Option<u8>,
    /// Speed stats and elemental resistances from the player's stat list, for the
    /// breakpoint/gear guide (user request, 2026-11-17). `None` if the stat list is unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<SpeedStats>,
    /// The equipped weapon's WSM (weapon speed modifier, weapons.txt `speed` column); shown
    /// next to IAS in the guide (its frames need the weapon speed). `None` without a readable
    /// weapon or items table. Set in `snapshot`, not kept in the JSON.
    #[serde(skip)]
    pub wsm: Option<i32>,
    /// Base strength and dexterity (stats [`crate::offsets::stat_list::STRENGTH`] / [`crate::offsets::stat_list::DEXTERITY`]
    /// of the player's base-only stat list, so without gear bonuses); `None` if unreadable.
    /// The vendor panel checks store items' requirements against them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dexterity: Option<u32>,
}

/// The player's speed stats and elemental resistances as granted by gear (stat ids from
/// itemstatcost.txt, summed over the equipped items' and carried charms' aggregated stat arrays
/// (`stat_list::FULL_ENTRIES`: base, affixes, runeword and socketed runes/gems);
/// difficulty penalties are NOT included — they are applied when the guide displays them).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeedStats {
    /// `item_fastercastrate` (FCR, %).
    pub fcr: i32,
    /// `item_fasterattackrate` (IAS, %).
    pub ias: i32,
    /// `item_fasterhitrecovery` (FHR, %).
    pub fhr: i32,
    /// The four elemental resistances as granted by gear (fire, cold, lightning, poison).
    /// Signed like every field here: gear can carry negative values (e.g. -10 all resistances).
    pub res_fire: i32,
    pub res_cold: i32,
    pub res_lightning: i32,
    pub res_poison: i32,
}

/// The character level from the raw stat value: 1..=99, or the same shifted left by 8 (some D2
/// stats, like life, are stored in 1/256 units). Anything else is unknown.
#[must_use]
pub fn char_level_from_raw(raw: u32) -> Option<u8> {
    let level = if raw > 99 && raw.trailing_zeros() >= 8 {
        raw >> 8
    } else {
        raw
    };
    u8::try_from(level).ok().filter(|l| (1..=99).contains(l))
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub addr: Addr,
    /// Unit id; 0 in older dumps (and then left out of the JSON).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub id: u32,
    pub name: String,
}

/// How `d2r_reader_mem::game::snapshot` picked the player among the candidates. In a group game the other players'
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

    #[must_use]
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

impl Snapshot {
    /// What the account holds, for copy counts ([`crate::stash::Holdings`]): the
    /// (session-cached) shared stash, the player's own names and runeword copies, and the
    /// mercenary's worn uniques and sets.
    #[must_use]
    pub fn holdings(&self) -> crate::stash::Holdings<'_> {
        crate::stash::Holdings {
            stash: &self.stash,
            owned: &self.owned_names,
            runeword_copies: &self.runeword_copies,
            merc_worn: &self.merc_worn,
        }
    }
}

/// Where one of the player's own items is ([`Snapshot::owned_items`]; see `d2r_reader_mem::game::place` for how
/// page and owner map to it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Inventory,
    Cube,
    Stash,
    Shared,
}

/// Cheap pre-check on the raw `ItemData` fields: an identified unique or set item, not a
/// runeword. Of the cube and stash items only these are read in full (only they can be spare,
/// `d2r_playstyle_config::valuables::verdict`).
#[must_use]
pub const fn identified_unique_or_set(quality: u32, flags: u32) -> bool {
    (quality == item_data::QUALITY_UNIQUE || quality == item_data::QUALITY_SET)
        && flags & item_data::FLAG_IDENTIFIED != 0
        && flags & item_data::FLAG_RUNEWORD == 0
}

/// Cheap pre-check on the raw `ItemData` fields: a white (low, normal or superior quality)
/// item without a runeword. Only its code tells a runeword base from a potion or a rune
/// (`d2r_reader_mem::game::stored_kept`).
#[must_use]
pub const fn maybe_white_base(quality: u32, flags: u32) -> bool {
    quality >= item_data::QUALITY_LOW
        && quality <= item_data::QUALITY_SUPERIOR
        && flags & item_data::FLAG_RUNEWORD == 0
}

/// A magic, rare or crafted item: the player's own ones get their stats read
/// ([`ItemDrop::stats`]) for the playstyle's keep-or-sell scoring.
#[must_use]
pub const fn magic_or_better(quality: u32) -> bool {
    matches!(
        quality,
        item_data::QUALITY_MAGIC | item_data::QUALITY_RARE | item_data::QUALITY_CRAFTED
    )
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
    /// The room was loaded at some point in this game, so its preset list was complete then (a
    /// landmark candidate piece of rooms all seen loaded without the quest object is ruled out:
    /// [`crate::render::landmarks`]). Set by `Session::apply` for candidate rooms only, never
    /// read from memory. Omitted from JSON when false, so dumps and fixtures stay unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seen_loaded: bool,
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
    #[must_use]
    pub const fn from_raw(t: u32) -> Self {
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
    /// Carries [`crate::offsets::state::DESECRATED`] (a Terror Zone monster). Left out of the JSON when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub desecrated: bool,
}

/// Monster modes of a dead or dying unit.
const DEAD_MODES: [u32; 2] = [0, 12];

impl Monster {
    #[must_use]
    pub fn is_dead(&self) -> bool {
        DEAD_MODES.contains(&self.mode)
    }
}

/// A shrine object (txt id in [`SHRINE_OBJECTS`]) with a shrine type in `d2r_reader_mem::game::SHRINE_TYPES`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shrine {
    /// objects.txt row.
    pub txt: u32,
    /// Shrine type: the shrines.txt code (`ObjectData` +0x08), 2 health, 12 skill, ...
    pub code: u8,
    /// Already clicked (any unit mode other than [`crate::offsets::object_mode::NEUTRAL`]).
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

/// A consumable quest object (txt id in [`crate::objects::CONSUMABLE_QUEST_OBJECTS`]) that has
/// been operated (unit mode other than [`crate::offsets::object_mode::NEUTRAL`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatedObject {
    /// objects.txt row.
    pub txt: u32,
    pub x: u32,
    pub y: u32,
}

/// True if objects.txt row `txt` is a shrine (see `research/gen_objects.py`).
#[must_use]
pub fn is_shrine_object(txt: u32) -> bool {
    SHRINE_OBJECTS.binary_search(&txt).is_ok()
}

/// True if objects.txt row `txt` is a well (see `research/gen_objects.py`).
#[must_use]
pub fn is_well_object(txt: u32) -> bool {
    WELL_OBJECTS.binary_search(&txt).is_ok()
}

/// A ground item worth showing (see [`crate::items::classify`]), or a base with a known code (the
/// playstyle decides which bases to show). The fields after `quality` default so older dumps
/// stay loadable; older dumps' `highlight` is ignored (marks live in
/// [`crate::guidance::Marks`]).
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
    /// The item's stats (`d2r_reader_mem::game::Reader::item_stats`), read only for the player's own magic, rare
    /// and crafted items (the playstyle scores them, `d2r_playstyle_config::stash_plan`); empty elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<crate::item_stats::ItemStat>,
    /// The first `item_data::HEAD_LEN` bytes of `ItemData` as hex, for checking offsets in the
    /// journal. Not part of the snapshot JSON.
    #[serde(skip)]
    pub item_data_head: String,
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
    /// Display name of an identified unique/set item (the `names::unique_set_name_parts`
    /// identity path) or of a runeword (its socketed runes in socket order,
    /// [`stash::runeword_by_runes`]); `None` otherwise (unidentified, magic, rare, base).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// uniqueitems/setitems row (`item_data::FILE_INDEX`) of a named unique/set. Kept only
    /// with a `name`: the index is only filled once the item is identified, so without a
    /// name it is not trusted (see `item_data::FILE_INDEX`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_index: Option<u32>,
    /// `ItemData::FLAGS` runeword bit ([`item_data::FLAG_RUNEWORD`], live-verified).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub runeword: bool,
    /// The item's full stat entries (`d2r_reader_mem::game::Reader::item_stats`: base, affixes, runeword, sockets):
    /// what the vendor panel compares store items against (`d2r_playstyle_config::gear_score`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<crate::item_stats::ItemStat>,
}

impl Equipped {
    /// The item's required character level, as far as the reader can know it:
    /// - an identified unique/set: the exact `lvl req` of its table row;
    /// - a named runeword: the higher of its runes' `levelreq` and its base's `levelreq`
    ///   (exact); an unnamed one (runes unreadable or unmatched): `None`;
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
            _ if self.runeword => {
                let word = crate::runewords_data::lookup(self.name.as_deref()?)?;
                let runes = stash::runes_level(word)?;
                Some(self.base_level().map_or(runes, |base| base.max(runes)))
            }
            _ => self.base_level(),
        }
    }

    /// The base item's `levelreq` (armor or weapons table); `None` for an unknown code.
    fn base_level(&self) -> Option<u32> {
        let code = self.code.as_deref()?;
        crate::item_levels_data::armor_level(code).or_else(|| crate::weapons_data::level(code))
    }
}

/// The names of the worn items that have one (identified unique/set, named runeword): the
/// `BiS` ✓ of a piece the character wears ([`crate::info::Tick::Worn`]), as opposed to one it
/// merely owns ([`Snapshot::owned_names`], which includes the worn ones).
#[must_use]
pub fn worn_names(items: &[Equipped]) -> Vec<String> {
    items.iter().filter_map(|e| e.name.clone()).collect()
}
