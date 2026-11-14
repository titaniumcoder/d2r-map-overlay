//! Verified D2R struct offsets (source: `PLAN.md` "Memory layer" and `research/*.py`).
//!
//! All values are byte offsets from the struct's base address. Pointers are 8 bytes.

/// Unit hash table: per unit type, `SLOTS` chain heads of 8 bytes each; type `t` starts at
/// `table + t * SLOTS * SLOT_SIZE`.
pub mod unit_table {
    pub const SLOTS: u64 = 128;
    pub const SLOT_SIZE: u64 = 8;
    /// Unit type ids (also the value of `unit::TYPE`).
    pub const PLAYER: u32 = 0;
    pub const MONSTER: u32 = 1;
    pub const ITEM: u32 = 4;
}

/// `UnitAny`: every unit type (player, monster, item, …).
pub mod unit {
    /// u32 unit type (see `unit_table`).
    pub const TYPE: u64 = 0x00;
    /// u32 txt file row (class for players, monstats row for monsters, item row for items).
    pub const TXT: u64 = 0x04;
    /// u32 unit id.
    pub const ID: u64 = 0x08;
    /// u32 mode (monsters: 0/12 dead).
    pub const MODE: u64 = 0x0C;
    /// ptr to `PlayerData` / `MonsterData` / `ItemData`.
    pub const UNIT_DATA: u64 = 0x10;
    /// ptr to the act; null on stale player units.
    pub const ACT: u64 = 0x20;
    /// ptr to `Path`.
    pub const PATH: u64 = 0x38;
    /// ptr to the next unit in the hash chain (verified for monsters only).
    pub const NEXT: u64 = 0x150;
}

/// `Path` (dynamic path of a moving unit).
pub mod path {
    /// u16 world x (tiles).
    pub const X: u64 = 0x02;
    /// u16 world y (tiles).
    pub const Y: u64 = 0x06;
    /// ptr to the `Room1` the unit stands in.
    pub const ROOM1: u64 = 0x20;
}

/// `Room1` (a loaded room).
pub mod room1 {
    /// ptr to the room's `Room2`.
    pub const ROOM2: u64 = 0x18;
}

/// `Room2` (level layout; present for the whole level).
pub mod room2 {
    /// ptr to the room's level-preset record (`level_preset`). Live-verified 2026-11-15 over 8 games.
    pub const LEVEL_PRESET: u64 = 0x40;
    /// ptr to the next `Room2` of the level.
    pub const NEXT: u64 = 0x48;
    /// ptr to the `Room1`; non-null if the room is loaded.
    pub const ROOM1: u64 = 0x58;
    /// u32 position and size in room units (×5 = world tiles).
    pub const X: u64 = 0x60;
    pub const Y: u64 = 0x64;
    pub const W: u64 = 0x68;
    pub const H: u64 = 0x6C;
    /// ptr to the first `RoomTile` (links to rooms of adjacent levels).
    pub const ROOM_TILES: u64 = 0x78;
    /// ptr to the owning `Level`.
    pub const LEVEL: u64 = 0x90;
    /// ptr to the first `PresetUnit`.
    pub const PRESETS: u64 = 0x98;
}

/// Level-preset record of a `Room2` (one lvlprest.txt row).
pub mod level_preset {
    /// u32 preset id (lvlprest row). Arcane Sanctuary arm ends: 525–528 hold the Summoner.
    pub const ID: u64 = 0x00;
}

/// Room units are 5 world tiles wide.
pub const ROOM_TILE_SCALE: u32 = 5;

/// `Level`.
pub mod level {
    /// ptr to the first `Room2`.
    pub const FIRST_ROOM2: u64 = 0x10;
    /// u32 level id (levels.txt row).
    pub const ID: u64 = 0x1F8;
}

/// `RoomTile`: one link from a room to a room of another level.
pub mod room_tile {
    /// ptr to the `Room2` of the linked level.
    pub const ROOM2: u64 = 0x00;
    /// ptr to the next `RoomTile`.
    pub const NEXT: u64 = 0x08;
}

/// `PresetUnit`: a fixed spawn (monster, object, tile/exit) of a room.
pub mod preset {
    /// u32 txt row.
    pub const TXT: u64 = 0x04;
    /// u32 x, room-relative, in world tiles.
    pub const X: u64 = 0x08;
    /// ptr to the next `PresetUnit`.
    pub const NEXT: u64 = 0x10;
    /// u32 kind: `MONSTER`, `OBJECT`, `TILE`.
    pub const TYPE: u64 = 0x20;
    /// u32 y, room-relative, in world tiles.
    pub const Y: u64 = 0x24;
    pub const MONSTER: u32 = 1;
    pub const OBJECT: u32 = 2;
    pub const TILE: u32 = 5;
}

/// `PlayerData` (unit data of a player).
pub mod player_data {
    /// Character name, NUL-terminated.
    pub const NAME: u64 = 0x00;
    pub const NAME_LEN: usize = 16;
}

/// `MonsterData` (unit data of a monster).
pub mod monster_data {
    /// u16 name seed (unique/superunique names).
    pub const NAME_SEED: u64 = 0x18;
    /// u8 flag byte (observed 0x08, 0x0C on elites; bits to confirm).
    pub const FLAGS: u64 = 0x1A;
    /// `FLAGS` bit of a superunique (classic D2 bit; champion 0x04, unique 0x08, minion 0x10).
    pub const SUPERUNIQUE: u8 = 0x02;
}

/// `StaticPath`: the path of a unit that doesn't move (items; `path` is for moving units).
pub mod static_path {
    /// u32 world x (tiles). Live-verified 2026-11-14 with a dropped gem next to the player.
    pub const X: u64 = 0x10;
    /// u32 world y (tiles).
    pub const Y: u64 = 0x14;
}

/// Item unit modes (`unit::MODE` of an item).
pub mod item_mode {
    /// Lying on the ground (live-verified 2026-11-14).
    pub const ON_GROUND: u32 = 3;
}

/// `ItemData` (unit data of an item).
pub mod item_data {
    /// u32 quality (classic D2: 1 low, 2 normal, 3 superior, 4 magic, 5 set, 6 rare, 7 unique,
    /// 8 crafted). Live-verified: a normal gem reads 2.
    pub const QUALITY: u64 = 0x00;
    /// Classic values; to verify live with a set and a unique drop.
    pub const QUALITY_SET: u32 = 5;
    pub const QUALITY_UNIQUE: u32 = 7;
}

/// The game's items table (one record per items.txt row, weapons then armor then misc; D2R row =
/// classic row + 15). Found at run time on the heap (`items::find_table`).
pub mod items_txt {
    /// Bytes per record.
    pub const STRIDE: u64 = 0x1C0;
    /// 4-byte item code, space-padded (`gcg `, `r01 `).
    pub const CODE: u64 = 0x00;
}
