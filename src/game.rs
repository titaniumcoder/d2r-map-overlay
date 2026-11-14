//! Reads one snapshot of the game state (player, level layout, presets, monsters, items).
//!
//! Lists in game memory change under us while we read, so a single bad pointer deep in a
//! list only skips that entry and is recorded in `Snapshot::warnings`; only a missing
//! player (or its level) fails the snapshot.

use std::collections::HashSet;
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::offsets::{
    ROOM_TILE_SCALE, item_data, level, monster_data, path, player_data, preset, room_tile, room1, room2,
    unit, unit_table,
};
use crate::process::ReadMem;

/// Most units walked per unit type (over all hash slots).
const MAX_UNITS_PER_TYPE: usize = 1000;
/// Most rooms walked per level.
const MAX_ROOMS: usize = 5000;
/// Most presets walked per room.
const MAX_PRESETS_PER_ROOM: usize = 500;
/// Most room-tile links walked per room.
const MAX_LINKS_PER_ROOM: usize = 100;
/// Warnings kept in the snapshot; the rest are only counted.
const MAX_WARNINGS: usize = 50;

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
    pub items: Vec<Item>,
    pub warnings: Vec<String>,
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

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub addr: Addr,
    pub id: u32,
    pub txt: u32,
    pub mode: u32,
    /// `None` if the item has no path (position struct to verify live).
    pub x: Option<u16>,
    pub y: Option<u16>,
    /// Hex of the first `item_data::HEAD_LEN` bytes of the unit data; `None` if unreadable.
    pub unit_data_head: Option<String>,
}

/// Reads one snapshot. `unit_table` is the hash table address from [`crate::image::Image::unit_table`].
///
/// # Errors
/// Fails if no live player unit is found (or none named `player_name`), or the player's level
/// can't be read. Unreadable entries elsewhere become warnings.
pub fn snapshot(mem: &impl ReadMem, unit_table: u64, player_name: Option<&str>) -> Result<Snapshot> {
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
    let mut items = Vec::new();
    for addr in r.units(unit_table, unit_table::ITEM) {
        match r.item(addr) {
            Ok(i) => items.push(i),
            Err(e) => r.warn(format!("item {addr:#x}: {e:#}")),
        }
    }

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
        items,
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
        })
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

    fn item(&self, addr: u64) -> Result<Item> {
        let head = self.unit_head(addr, unit_table::ITEM)?;
        let pos = head.path.map(|p| self.pos(p)).transpose()?;
        // Unit data is raw diagnostics only: an unreadable one is reported as null, not skipped.
        let unit_data_head = head
            .unit_data
            .and_then(|d| self.bytes(d, 0, item_data::HEAD_LEN).ok())
            .map(|b| hex(&b));
        Ok(Item {
            addr: Addr(addr),
            id: head.id,
            txt: head.txt,
            mode: head.mode,
            x: pos.map(|p| p.0),
            y: pos.map(|p| p.1),
            unit_data_head,
        })
    }
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

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        // Writing to a String can't fail.
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
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

    const UNIT_SIZE: usize = 0x158;

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
        put(
            mem,
            addr,
            0xA0,
            &[
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
        let s = snapshot(&world()?, TABLE, None)?;
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
        let s = snapshot(&m, TABLE, None)?;
        assert_eq!(s.level.rooms.len(), 2);
        assert!(s.warnings.iter().any(|w| w.contains("room walk stopped")));
        // A foreign first room means the level offsets are wrong: that's an error, not an empty map.
        level_struct(&mut m, LEVEL, 21, R_OTHER)?;
        assert!(snapshot(&m, TABLE, None).is_err());
        Ok(())
    }

    #[test]
    fn level_rooms_in_world_tiles() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None)?;
        assert_eq!(s.level.id, 21);
        assert_eq!(
            s.level.rooms,
            vec![
                Room {
                    x: 50,
                    y: 100,
                    w: 40,
                    h: 30,
                    loaded: true
                },
                Room {
                    x: 90,
                    y: 100,
                    w: 40,
                    h: 40,
                    loaded: false
                },
                Room {
                    x: 50,
                    y: 130,
                    w: 20,
                    h: 20,
                    loaded: false
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
    fn presets_and_links() -> Result<()> {
        let s = snapshot(&world()?, TABLE, None)?;
        assert_eq!(
            s.presets,
            vec![
                Preset {
                    kind: PresetKind::Object,
                    txt: 119,
                    x: 53,
                    y: 104
                },
                Preset {
                    kind: PresetKind::Tile,
                    txt: 7,
                    x: 51,
                    y: 102
                },
                Preset {
                    kind: PresetKind::Other(9),
                    txt: 1,
                    x: 50,
                    y: 100
                },
                Preset {
                    kind: PresetKind::Monster,
                    txt: 853,
                    x: 92,
                    y: 103
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
        let s = snapshot(&world()?, TABLE, None)?;
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
            s.items,
            vec![Item {
                addr: Addr(I0),
                id: 0x800,
                txt: 600,
                mode: 1,
                x: None,
                y: None,
                unit_data_head: Some(
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".into()
                ),
            }]
        );
        Ok(())
    }

    #[test]
    fn player_by_name() -> Result<()> {
        let mem = world()?;
        let s = snapshot(&mem, TABLE, Some("Bob"))?;
        assert_eq!((s.player.addr, s.player.class, s.player.x), (Addr(P_BOB), 5, 95));
        let err = snapshot(&mem, TABLE, Some("Ghost"))
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
        let err = snapshot(&mem, TABLE, None).err().context("expected error")?;
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
        let s = snapshot(&mem, TABLE, None)?;
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
        let s = snapshot(&mem, TABLE, None)?;
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
        let s = snapshot(&m, TABLE, None)?;
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
        let s = snapshot(&world()?, TABLE, None)?;
        let v = serde_json::to_value(&s)?;
        let at = |p: &str| v.pointer(p).cloned().unwrap_or_default();
        assert_eq!(at("/player/addr"), "0x201000");
        assert_eq!(at("/presets/0/kind"), "object");
        assert_eq!(at("/presets/2/kind"), 9);
        assert_eq!(v.pointer("/items/0/x"), Some(&serde_json::Value::Null));
        Ok(())
    }
}
