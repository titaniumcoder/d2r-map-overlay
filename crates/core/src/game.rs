//! The game state model: one snapshot of what the reader found, serializable as the `--dump`
//! JSON. Optional fields are left out of the JSON when empty so older dumps stay loadable.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::items::DropKind;
use crate::objects_data::{SHRINE_OBJECTS, WELL_OBJECTS};
use crate::offsets::{item_data, preset};
use crate::stash::{self, StashTab};

/// Living desecrated monsters needed to call a level terrorized; two so one misread unit can't.
pub const TERROR_MIN_MONSTERS: usize = 2;

/// Level ids of the five towns (levels.txt).
pub const TOWN_LEVELS: [u32; 5] = [1, 40, 75, 103, 109];

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
    /// The local player's equipped `(body slot, quality)` pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equipped: Vec<(u8, u32)>,
    /// The mercenary's equipped `(body slot, quality)` pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merc_equipped: Vec<(u8, u32)>,
    /// The player's equipped items with code, runeword flag and unique/set name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub equipped_items: Vec<Equipped>,
    /// Names of identified uniques/sets and named runewords the player owns (worn ones included).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owned_names: Vec<String>,
    /// One name per runeword copy outside the shared stash (or worn by the merc), duplicates kept.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runeword_copies: Vec<String>,
    /// Names of uniques/sets and runewords the mercenary wears; empty in multiplayer games, where
    /// other players' mercs can't be told apart (PLAN.md, "Merc gear caveat").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merc_worn: Vec<String>,
    /// The shared-stash tabs read this snapshot; empty once the stash UI closes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stash: Vec<StashTab>,
    /// Names of the identified uniques/sets in [`Snapshot::stash`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stash_names: Vec<String>,
    pub player_candidates: Vec<Candidate>,
    #[serde(default, skip_serializing_if = "ChosenBy::is_first_candidate")]
    pub player_chosen_by: ChosenBy,
    pub level: Level,
    pub presets: Vec<Preset>,
    pub room_links: Vec<RoomLink>,
    /// Rooms of other levels in this level's near-room lists: walk-through borders, which have no
    /// tile preset or room tile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub border_links: Vec<BorderLink>,
    pub monsters: Vec<Monster>,
    /// Item units lying on the ground, shown or not.
    #[serde(default)]
    pub ground_items: u32,
    /// Ground items worth showing; the playstyle filters the bases before anything draws them.
    #[serde(default)]
    pub drops: Vec<ItemDrop>,
    /// Shrines near the player (objects are loaded only in range).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shrines: Vec<Shrine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wells: Vec<Well>,
    /// Operated consumable quest objects near the player (Diablo seals).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operated_objects: Vec<OperatedObject>,
    /// Items in the player's inventory, shown or not.
    #[serde(skip)]
    pub inventory_items: u32,
    /// The player's own items: inventory items, plus cube/stash items that are identified
    /// uniques/sets, magic or better, or white runeword bases (only these are read in full).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owned_items: Vec<(ItemDrop, Place)>,
    /// Runes and gems held in inventory, cube and stash pages, `(name, count)`.
    #[serde(skip)]
    pub held_materials: Vec<(String, u32)>,
    /// White socketed bases without a runeword.
    #[serde(skip)]
    pub white_bases: Vec<String>,
    /// 0 Normal, 1 Nightmare, 2 Hell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u8>,
    /// The real Tal Rasha's Tomb level id of this game, read in Act 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staff_tomb: Option<u32>,
    /// Two or more living desecrated monsters seen (the session keeps it for the level).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terrorized: bool,
    /// The cow counter, set by the session in the Moo Moo Farm only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cows: Option<crate::cows::Tally>,
    /// Where the player has been in this game's Moo Moo Farm (world tiles, oldest first).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trail: Vec<(u16, u16)>,
    #[serde(default, skip_serializing_if = "Skills::is_empty")]
    pub skills: Skills,
    /// The vendor trade window open right now; read in towns only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop: Option<crate::shop::Shop>,
    #[serde(skip)]
    pub buff_research: Option<BuffResearch>,
    pub warnings: Vec<String>,
}

/// The player's hard skill points and mouse-button skills; gear bonus levels aren't in the nodes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skills {
    /// `(skills.txt id, hard points)` of the class skills with points, in list order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<(u16, u8)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<u16>,
}

impl Skills {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.points.is_empty() && self.left.is_none() && self.right.is_none()
    }
}

#[must_use]
pub fn is_terrorized(monsters: &[Monster]) -> bool {
    monsters.iter().filter(|m| m.desecrated && !m.is_dead()).count() >= TERROR_MIN_MONSTERS
}

/// True if `state` is set in the state bitfield `flags`: little-endian u32 dwords, bit
/// `n % 32` of dword `n / 32`; states past the end are clear.
#[must_use]
pub fn has_state(flags: &[u8], state: u32) -> bool {
    let (dwords, _) = flags.as_chunks::<4>();
    usize::try_from(state / 32)
        .ok()
        .and_then(|i| dwords.get(i))
        .is_some_and(|&d| u32::from_le_bytes(d) & (1 << (state % 32)) != 0)
}

/// The states set in the state bitfield `flags`, ascending.
#[must_use]
pub fn active_states(flags: &[u8]) -> Vec<u32> {
    let (dwords, _) = flags.as_chunks::<4>();
    (0u32..)
        .zip(dwords)
        .flat_map(|(i, &d)| {
            let d = u32::from_le_bytes(d);
            (0..32)
                .filter(move |bit| d & (1 << bit) != 0)
                .map(move |bit| i * 32 + bit)
        })
        .collect()
}

/// Raw dump of the player's stat lists for the buff tracker research (journal `buffs` event).
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct BuffResearch {
    /// States decoded from the bitfield's second copy (`stat_list::STATE_FLAGS_COPY`).
    pub states_copy: Vec<u32>,
    /// Every list of the player's chain, in chain order (capped by the reader).
    pub lists: Vec<StatListDump>,
    /// The top list's aggregated array, `(layer, stat id, value)`.
    pub full: Vec<(u16, u16, u32)>,
    /// The chain or a list's entries were cut at the reader's caps.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StatListDump {
    pub addr: Addr,
    pub owner_type: u32,
    pub owner_id: u32,
    pub flags: u32,
    /// Hex of the list's first [`crate::offsets::stat_list::DUMP_LEN`] bytes (fewer if unreadable).
    pub head: String,
    pub count: u64,
    /// `(layer, stat id, value)` entries, at most the reader's per-list cap.
    pub entries: Vec<(u16, u16, u32)>,
}

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
    /// The player's act; changes with a new game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<Addr>,
    /// Character level, see [`char_level_from_raw`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_level: Option<u8>,
    /// Speed stats and resistances from gear and the player's own stat list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<SpeedStats>,
    /// The equipped weapon's weapon speed modifier (weapons.txt `speed`).
    #[serde(skip)]
    pub wsm: Option<i32>,
    /// Base strength and dexterity, without gear bonuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dexterity: Option<u32>,
    /// Active states (states.txt ids, ascending); `None` when unread, so the live reader can carry
    /// the last value over a failed read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub states: Option<Vec<u32>>,
    /// The state list most recently attached to the player's stats: a new one each time a
    /// state is (re)applied, which is how a refreshed buff shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_state: Option<StateList>,
}

/// A state's own stat list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateList {
    pub list: Addr,
    /// states.txt row.
    pub state: u32,
    /// `(stat id, value)`.
    pub stats: Vec<(u16, u32)>,
}

/// The player's speed stats and elemental resistances summed over equipped items and charms;
/// resists include permanent quest bonuses but not the difficulty penalty.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeedStats {
    /// Faster cast rate, %.
    pub fcr: i32,
    /// Increased attack speed, %.
    pub ias: i32,
    /// Faster hit recovery, %.
    pub fhr: i32,
    /// Gear resistances can be negative.
    pub res_fire: i32,
    pub res_cold: i32,
    pub res_lightning: i32,
    pub res_poison: i32,
}

/// The character level from the raw stat value: 1..=99, possibly stored shifted left by 8.
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
    /// Unit id; 0 in older dumps.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub id: u32,
    pub name: String,
}

/// How the player was picked among the candidates (in group games other players are too).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChosenBy {
    /// `--player NAME`.
    Name,
    /// The previous tick's player is still a candidate; kept so the owner vote can't flip it.
    Previous,
    /// Majority owner id of the stash items.
    StashOwner,
    /// Majority owner id of the inventory items.
    InventoryOwner,
    #[default]
    FirstCandidate,
}

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
    /// What the account holds, for copy counts.
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

/// Where one of the player's own items is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Inventory,
    Cube,
    Stash,
    Shared,
}

/// An identified unique or set item that is not a runeword (from raw `ItemData` fields).
#[must_use]
pub const fn identified_unique_or_set(quality: u32, flags: u32) -> bool {
    (quality == item_data::QUALITY_UNIQUE || quality == item_data::QUALITY_SET)
        && flags & item_data::FLAG_IDENTIFIED != 0
        && flags & item_data::FLAG_RUNEWORD == 0
}

/// A low, normal or superior quality item without a runeword (from raw `ItemData` fields).
#[must_use]
pub const fn maybe_white_base(quality: u32, flags: u32) -> bool {
    quality >= item_data::QUALITY_LOW
        && quality <= item_data::QUALITY_SUPERIOR
        && flags & item_data::FLAG_RUNEWORD == 0
}

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

/// World-tile bounding box; `max` is exclusive.
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
    /// The room was loaded at some point this game, so its preset list was complete then; set by
    /// the session for landmark candidate rooms only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seen_loaded: bool,
    /// Level-preset id (lvlprest row).
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
    /// The tile preset the player entered the level by; set by the session.
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

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Hash)]
pub struct BorderLink {
    pub from_room: RoomRect,
    pub to_level_id: u32,
    pub to_room: RoomRect,
    /// `to_level_id` is the level the player came from; set by the session.
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
    /// Owner type/id from the `MonsterData` head; unverified hypothesis that the player's id marks
    /// their merc.
    #[serde(default, skip_serializing_if = "is_zero", alias = "owner")]
    pub owner_id: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub owner_type: u32,
    /// Carries the desecrated state (a Terror Zone monster).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub desecrated: bool,
}

const DEAD_MODES: [u32; 2] = [0, 12];

impl Monster {
    #[must_use]
    pub fn is_dead(&self) -> bool {
        DEAD_MODES.contains(&self.mode)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shrine {
    /// objects.txt row.
    pub txt: u32,
    /// shrines.txt code: 2 health, 12 skill, ...
    pub code: u8,
    pub used: bool,
    pub x: u32,
    pub y: u32,
}

/// A well object. Wells refill, so used or not doesn't matter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Well {
    /// objects.txt row.
    pub txt: u32,
    pub x: u32,
    pub y: u32,
}

/// A consumable quest object that has been operated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperatedObject {
    /// objects.txt row.
    pub txt: u32,
    pub x: u32,
    pub y: u32,
}

#[must_use]
pub fn is_shrine_object(txt: u32) -> bool {
    SHRINE_OBJECTS.binary_search(&txt).is_ok()
}

#[must_use]
pub fn is_well_object(txt: u32) -> bool {
    WELL_OBJECTS.binary_search(&txt).is_ok()
}

/// A ground item worth showing, or a base with a known code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemDrop {
    pub id: u32,
    /// Item code (`"gcg"`); `None` if the items table is unknown.
    pub code: Option<String>,
    pub kind: DropKind,
    /// Raw `ItemData` quality (2 normal, 5 set, 7 unique, ...).
    #[serde(default)]
    pub quality: u32,
    /// Raw `ItemData` flags (bits mostly unverified).
    #[serde(default)]
    pub flags: u32,
    #[serde(default)]
    pub ethereal: bool,
    /// Socket count (stat 194).
    #[serde(default)]
    pub sockets: u8,
    /// uniqueitems / setitems row of a unique or set item.
    #[serde(default)]
    pub unique_index: Option<u32>,
    /// The item's stats, read only for the player's own magic, rare and crafted items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<crate::item_stats::ItemStat>,
    /// The first `item_data::HEAD_LEN` bytes of `ItemData` as hex, for checking offsets.
    #[serde(skip)]
    pub item_data_head: String,
    pub x: u32,
    pub y: u32,
}

/// One equipped body-slot item of the player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equipped {
    /// Body slot: 1 helm, 2 amulet, 3 body, 4 weapon, 5 off-hand, 6/7 rings, 8 belt, 9 boots,
    /// 10 gloves.
    pub body: u8,
    /// Raw `ItemData` quality (2 normal, 4 magic, 5 set, 7 unique, ...).
    pub quality: u32,
    /// 4-char item code ("uap").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Name of an identified unique/set or of a runeword recognised by its socketed runes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// uniqueitems/setitems row; kept only with a `name`, since it is unreliable before identify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_index: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub runeword: bool,
    /// The item's full stat entries: base, affixes, runeword, sockets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<crate::item_stats::ItemStat>,
}

impl Equipped {
    /// The item's required level: exact for uniques, sets and named runewords (`None` for an
    /// unnamed runeword), otherwise the base's `levelreq`, which ignores affix requirements.
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

    /// The base item's `levelreq`.
    fn base_level(&self) -> Option<u32> {
        let code = self.code.as_deref()?;
        crate::item_levels_data::armor_level(code).or_else(|| crate::weapons_data::level(code))
    }
}

/// The names of the worn items that have one (identified unique/set, named runeword).
#[must_use]
pub fn worn_names(items: &[Equipped]) -> Vec<String> {
    items.iter().filter_map(|e| e.name.clone()).collect()
}
