//! Reads one snapshot of the game state (player, level layout, presets, monsters, ground drops,
//! nearby shrines and wells).
//!
//! Lists in game memory change under us while we read, so a single bad pointer deep in a
//! list only skips that entry and is recorded in `Snapshot::warnings`; only a missing
//! player (or its level) fails the snapshot.

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::items::{self, DropKind};
use crate::objects_data::{SHRINE_OBJECTS, WELL_OBJECTS};
use crate::offsets::{
    ROOM_TILE_SCALE, act, act_misc, item_data, item_mode, level, level_preset, monster_data, object_data,
    object_mode, path, player_data, preset, room_tile, room1, room2, stat_list, static_path, unit,
    unit_table,
};
use crate::process::ReadMem;
use crate::render::is_consumable_quest_object;
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
/// Warnings kept in the snapshot; the rest are only counted.
const MAX_WARNINGS: usize = 50;

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
    pub player_candidates: Vec<Candidate>,
    pub level: Level,
    pub presets: Vec<Preset>,
    pub room_links: Vec<RoomLink>,
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
    /// [`valuables::keep_list`]). Shown as green lines below the info line, also in towns.
    /// Left out of the JSON when empty, so older dumps and fixtures stay unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inventory_keep: Vec<valuables::Keep>,
    /// 0 Normal, 1 Nightmare, 2 Hell; `None` if unreadable (and in old fixtures).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u8>,
    pub warnings: Vec<String>,
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
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub addr: Addr,
    pub name: String,
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

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomLink {
    pub from_room: RoomPos,
    pub to_level_id: u32,
    pub to_room: RoomPos,
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
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
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

/// Reads one snapshot. `unit_table` is the hash table address from [`crate::image::Image::unit_table`];
/// `items_table` is the item-code table from [`items::find_table`] (`None`: unique and set drops only).
///
/// # Errors
/// Fails if no live player unit is found (or none named `player_name`), or the player's level
/// can't be read. Unreadable entries elsewhere become warnings.
pub fn snapshot(
    mem: &impl ReadMem,
    unit_table: u64,
    items_table: Option<u64>,
    player_name: Option<&str>,
) -> Result<Snapshot> {
    let mut r = Reader {
        mem,
        warnings: Vec::new(),
        suppressed: 0,
    };

    let mut candidates = Vec::new();
    for addr in r.units(unit_table, unit_table::PLAYER) {
        match r.player_candidate(addr) {
            Ok(Some(p)) => candidates.push(p),
            Ok(None) => {}
            Err(e) => r.warn(format!("player unit {addr:#x}: {e:#}")),
        }
    }
    let Some(chosen) = candidates
        .iter()
        .find(|c| player_name.is_none_or(|n| c.name == n))
    else {
        let names: Vec<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
        match player_name {
            Some(name) => bail!("no live player named {name:?} (candidates: {names:?})"),
            None => bail!("no live player unit found (not in a game?)"),
        }
    };
    let (player, level_addr) = r.player(chosen.addr.0).context("reading the player")?;

    let rooms = r.walk(
        r.ptr(level_addr, level::FIRST_ROOM2)?,
        room2::NEXT,
        MAX_ROOMS,
        "room",
    );
    let mut level = Level {
        id: player.level_id,
        bounds: None,
        rooms: Vec::new(),
    };
    let mut presets = Vec::new();
    let mut room_links = Vec::new();
    for (i, room_addr) in rooms.into_iter().enumerate() {
        // The Room2 chain must stay inside this level (as in research/rooms.py); stop where it leaves.
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
        level.rooms.push(room);
    }
    level.bounds = bounds(&level.rooms);
    dedup_presets(&mut presets);

    let mut monsters = Vec::new();
    for addr in r.units(unit_table, unit_table::MONSTER) {
        match r.monster(addr) {
            Ok(m) => monsters.push(m),
            Err(e) => r.warn(format!("monster {addr:#x}: {e:#}")),
        }
    }
    let (ground_items, drops, inventory) = r.items(unit_table, items_table);
    let (shrines, wells, operated_objects) = r.objects(unit_table);
    let inventory_items = u32::try_from(inventory.len()).unwrap_or(u32::MAX);
    let inventory_keep = valuables::keep_list(&inventory);
    let difficulty = player.act.and_then(|a| r.difficulty(a.0));

    if r.suppressed > 0 {
        r.warnings.push(format!("... and {} more warnings", r.suppressed));
    }
    Ok(Snapshot {
        player,
        player_candidates: candidates,
        level,
        presets,
        room_links,
        monsters,
        ground_items,
        drops,
        shrines,
        wells,
        operated_objects,
        inventory_items,
        inventory_keep,
        difficulty,
        warnings: r.warnings,
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
        };
        Ok((player, level))
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
        })
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

    /// The number of items on the ground, those worth showing, and the items in the player's
    /// inventory (all of them, position 0,0; the keep list picks from them).
    fn items(&mut self, unit_table: u64, items_table: Option<u64>) -> (u32, Vec<ItemDrop>, Vec<ItemDrop>) {
        let mut ground = 0u32;
        let mut drops = Vec::new();
        let mut inventory = Vec::new();
        for addr in self.units(unit_table, unit_table::ITEM) {
            let head = match self.unit_head(addr, unit_table::ITEM) {
                Ok(head) => head,
                Err(e) => {
                    self.warn(format!("item {addr:#x}: {e:#}"));
                    continue;
                }
            };
            // Stash, cube, equipped, cursor and belt items are units too: only the ground and
            // the inventory count.
            let found = match head.mode {
                item_mode::ON_GROUND => {
                    ground = ground.saturating_add(1);
                    self.item_drop(addr, &head, items_table)
                        .map(|d| d.map(|d| (d, true)))
                }
                item_mode::STORED if self.in_inventory(&head) => self
                    .item_fields(addr, &head, items_table)
                    .map(|d| Some((d, false))),
                _ => Ok(None),
            };
            match found {
                Ok(Some((d, true))) => drops.push(d),
                Ok(Some((d, false))) => inventory.push(d),
                Ok(None) => {}
                Err(e) => self.warn(format!("item {addr:#x}: {e:#}")),
            }
        }
        (ground, drops, inventory)
    }

    /// True for a stored item on the inventory page (not stash or cube). An unreadable page is
    /// not the inventory.
    fn in_inventory(&self, head: &UnitHead) -> bool {
        head.unit_data
            .and_then(|data| self.u8(data, item_data::PAGE).ok())
            == Some(item_data::PAGE_INVENTORY)
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
        let Some(list) = self.ptr(unit_addr, unit::STATS)? else {
            return Ok(0);
        };
        let count = self.u64(list, stat_list::COUNT)?;
        if count == 0 {
            return Ok(0);
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
        let value = stats
            .iter()
            .find_map(|&[_, _, id0, id1, v0, v1, v2, v3]| {
                (u16::from_le_bytes([id0, id1]) == stat_list::NUM_SOCKETS)
                    .then(|| u32::from_le_bytes([v0, v1, v2, v3]))
            })
            .unwrap_or(0);
        Ok(u8::try_from(value).unwrap_or(u8::MAX))
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
        put(
            mem,
            addr,
            UNIT_SIZE,
            &[
                (unit::TYPE, &ty.to_le_bytes()),
                (unit::TXT, &txt.to_le_bytes()),
                (unit::ID, &(u32::try_from(addr >> 12)?).to_le_bytes()),
                (unit::MODE, &1u32.to_le_bytes()),
                (unit::UNIT_DATA, &data.to_le_bytes()),
                (unit::ACT, &act.to_le_bytes()),
                (unit::PATH, &path.to_le_bytes()),
                (unit::NEXT, &next.to_le_bytes()),
            ],
        )
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
        let s = snapshot(&world()?, TABLE, None, None)?;
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
                    name: "Alice".into()
                },
                Candidate {
                    addr: Addr(P_BOB),
                    name: "Bob".into()
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn room_walk_stops_at_foreign_level() -> Result<()> {
        let mut m = world()?;
        // R2 now claims to belong to another level: the walk keeps R0, R1 and stops with a warning.
        room(&mut m, R2, (10, 26, 4, 4), 0, 0, L_OTHER, 0, 0)?;
        let s = snapshot(&m, TABLE, None, None)?;
        assert_eq!(s.level.rooms.len(), 2);
        assert!(s.warnings.iter().any(|w| w.contains("room walk stopped")));
        // A foreign first room means the level offsets are wrong: that's an error, not an empty map.
        level_struct(&mut m, LEVEL, 21, R_OTHER)?;
        assert!(snapshot(&m, TABLE, None, None).is_err());
        Ok(())
    }

    #[test]
    fn level_rooms_in_world_tiles() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None, None)?;
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

    #[test]
    fn room_preset_ids() -> Result<()> {
        const LP0: u64 = 0x33_0000;
        let mut m = world()?;
        // R0 points at a record with id 525; R1 at an unmapped record; R2 keeps a null pointer.
        room_with_preset(&mut m, R0, (10, 20, 8, 6), R1, ROOM1_0, LEVEL, PR_OBJ, RT0, LP0)?;
        room_with_preset(&mut m, R1, (18, 20, 8, 8), R2, 0, LEVEL, PR_MON, 0, UNMAPPED)?;
        put(&mut m, LP0, 0x10, &[(level_preset::ID, &525u32.to_le_bytes())])?;
        let s = snapshot(&m, TABLE, None, None)?;
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
        let s = snapshot(&world()?, TABLE, None, None)?;
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
        let s = snapshot(&world()?, TABLE, None, None)?;
        let monster = |addr, txt, x, y, flags, name_seed| Monster {
            addr: Addr(addr),
            id: u32::try_from(addr >> 12).unwrap_or_default(),
            txt,
            mode: 1,
            x,
            y,
            flags,
            name_seed,
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
            0x60,
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

        let mut s = snapshot(&m, TABLE, Some(ITEMS), None)?;
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
        stored_item(&mut m, I5, (30, 2, id, 0), (stored, 0), &[(194, 3)], 0)?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), None)?;
        assert_eq!((s.ground_items, s.drops.len()), (0, 0));
        assert_eq!(s.inventory_items, 3, "page-0 stored items only");
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
        let s = snapshot(&m, TABLE, Some(ITEMS), None)?;
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
        let s = snapshot(&m, TABLE, None, None)?;
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
        let s = snapshot(&mem, TABLE, None, Some("Bob"))?;
        assert_eq!((s.player.addr, s.player.class, s.player.x), (Addr(P_BOB), 5, 95));
        let err = snapshot(&mem, TABLE, None, Some("Ghost"))
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
        let err = snapshot(&mem, TABLE, None, None)
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
        let s = snapshot(&mem, TABLE, None, None)?;
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
        let s = snapshot(&mem, TABLE, None, None)?;
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
        let s = snapshot(&m, TABLE, None, None)?;
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
        let v = serde_json::to_value(snapshot(&world()?, TABLE, None, None)?)?;
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
        let s = snapshot(&m, TABLE, None, None)?;
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
        let v = serde_json::to_value(snapshot(&world()?, TABLE, None, None)?)?;
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
        let s = snapshot(&m, TABLE, None, None)?;
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
        let s = snapshot(&m, TABLE, None, None)?;
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
            let s = snapshot(&m, TABLE, None, None)?;
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
        let s = snapshot(&world()?, TABLE, None, None)?;
        let v = serde_json::to_value(&s)?;
        let at = |p: &str| v.pointer(p).cloned().unwrap_or_default();
        assert_eq!(at("/player/addr"), "0x201000");
        assert_eq!(at("/presets/0/kind"), "object");
        assert_eq!(at("/presets/2/kind"), 9);
        let s = snapshot(&world_with_items()?, TABLE, Some(ITEMS), None)?;
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
