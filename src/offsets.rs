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
    /// Objects (shrines, wells, chests, ...); only those near the player are loaded. Live 2026-11-15.
    pub const OBJECT: u32 = 2;
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
    /// ptr to the unit's `StatList` (verified live on items, 2026-11-15).
    pub const STATS: u64 = 0x88;
    /// ptr to the next unit in the hash chain (same slot = same `id & 127`). Verified live
    /// 2026-11-15 for items (43 chained units), monsters (24) and objects (1): every chained unit
    /// had the slot's type and `id & 127 == slot`. The earlier 0x150 only ever reached slot heads,
    /// so a unit sharing a slot was invisible (a dropped sapphire never showed).
    pub const NEXT: u64 = 0x158;
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
    /// ptr to an array of [`NEAR_COUNT`] `Room2` pointers: the "near rooms", which include rooms
    /// of adjacent levels. Live 2026-11-15 (python probe, Outer Cloister 27): the top-row rooms
    /// had near rooms in Barracks (28), the bottom-row ones in Monastery Gate (26), so a
    /// walk-through border to another level shows up here without any tile preset or room tile.
    pub const NEAR: u64 = 0x10;
    /// u32 number of entries in [`NEAR`]. Live 2026-11-15: +0x20 sometimes read 9 with null
    /// entries after the real ones; that is a different field, this one is the count.
    pub const NEAR_COUNT: u64 = 0x18;
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

/// `Act` (the unit's act; `unit::ACT`).
pub mod act {
    /// ptr to `ActMisc`. Live-verified 2026-11-15; older builds had it at +0x78 (now points into code).
    pub const MISC: u64 = 0x70;
}

/// `ActMisc` (per-act game data).
pub mod act_misc {
    /// u16 difficulty: 0 Normal, 1 Nightmare, 2 Hell. Live-verified 2026-11-15 (Normal only).
    pub const DIFFICULTY: u64 = 0x830;
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

/// `StaticPath`: the path of a unit that doesn't move (items and objects; `path` is for moving
/// units). Objects live-verified 2026-11-15.
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
    /// In a storage page: inventory, stash, cube (`item_data::PAGE` tells which). Live 2026-11-15.
    pub const STORED: u32 = 0;
}

/// Object unit modes (`unit::MODE` of an object). An untouched shrine is in mode 0 (live
/// 2026-11-15).
pub mod object_mode {
    /// Neutral: an untouched shrine. Any other mode (1 = operating, the click animation; 2 =
    /// operated, seen live on a clicked Health shrine) means it is used. Wells refill, so a well
    /// is drawn in any mode.
    pub const NEUTRAL: u32 = 0;
}

/// `ObjectData` (unit data of an object). `+0x00` points to the object's objects.txt record (not
/// read: the unit's txt id says the same).
pub mod object_data {
    /// u8 shrine type = shrines.txt code (1..=22) of a shrine object. Live 2026-11-15: a Health
    /// shrine read 2.
    pub const SHRINE_TYPE: u64 = 0x08;
}

/// `ItemData` (unit data of an item).
pub mod item_data {
    /// u32 quality (classic D2: 1 low, 2 normal, 3 superior, 4 magic, 5 set, 6 rare, 7 unique,
    /// 8 crafted). Live-verified: a normal gem reads 2.
    pub const QUALITY: u64 = 0x00;
    /// Classic values; unique (7) live-verified 2026-11-15, set (5) still to see.
    pub const QUALITY_SET: u32 = 5;
    pub const QUALITY_UNIQUE: u32 = 7;
    /// Other classic quality values (2 and 4 seen live).
    pub const QUALITY_NORMAL: u32 = 2;
    pub const QUALITY_SUPERIOR: u32 = 3;
    pub const QUALITY_MAGIC: u32 = 4;
    pub const QUALITY_RARE: u32 = 6;
    /// u32 unit id of the player owning the item. Live 2026-11-15: every stash (page 4) and
    /// inventory (page 0) item carried the local player's unit id; ground items carry
    /// [`NO_OWNER`]. Only the local client's own stash is in memory, so the stash owner is the
    /// local player.
    pub const OWNER: u64 = 0x0C;
    /// `OWNER` of an item nobody owns (live: ground items).
    pub const NO_OWNER: u32 = 0xFFFF_FFFF;
    /// u32 item flags. Live: `0x0080_0000` on unidentified magic items. The bit values below are
    /// classic D2 and still to verify in D2R.
    pub const FLAGS: u64 = 0x18;
    /// `FLAGS` bit of an ethereal item (classic; to verify live).
    pub const FLAG_ETHEREAL: u32 = 0x0040_0000;
    /// `FLAGS` bit of a finished runeword (classic; UNVERIFIED in D2R, the journal logs `FLAGS`).
    pub const FLAG_RUNEWORD: u32 = 0x0400_0000;
    /// `FLAGS` bit of an identified item (classic; live: set on an identified quest unique).
    pub const FLAG_IDENTIFIED: u32 = 0x10;
    /// u32 uniqueitems / setitems row (`*ID`) of a unique / set item. Live-verified 2026-11-15 for
    /// an identified unique (Khalim's Flail: 127 = uniqueitems `KhalimFlail`). NOT verified: sets,
    /// and whether it is filled in before the item is identified (so only trusted with
    /// [`FLAG_IDENTIFIED`]).
    pub const FILE_INDEX: u64 = 0x34;
    /// u8 storage page of a stored item: `PAGE_INVENTORY` 0, stash 4; 255 when equipped (body
    /// slot in the byte at `+0x54`), on the ground or on the cursor. Live-verified 2026-11-15
    /// over 74 items.
    pub const PAGE: u64 = 0x55;
    pub const PAGE_INVENTORY: u8 = 0;
    pub const PAGE_STASH: u8 = 4;
    /// Bytes of `ItemData` copied into the journal's drop event for checking the offsets above.
    pub const HEAD_LEN: usize = 0x50;
}

/// `StatList` (unit `+0x88`). Verified live on items (damage 21/22, durability 72/73, defense 31).
pub mod stat_list {
    /// ptr to the array of [`STAT_SIZE`]-byte entries `{u16 layer, u16 stat id, u32 value}`.
    pub const ENTRIES: u64 = 0x30;
    /// u64 entry count.
    pub const COUNT: u64 = 0x38;
    pub const STAT_SIZE: u64 = 8;
    /// Stat id `item_numsockets` (itemstatcost.txt).
    pub const NUM_SOCKETS: u16 = 194;
    /// The unit's state bitfield: [`STATE_FLAG_DWORDS`] little-endian u32, state `n` is bit
    /// `n % 32` of dword `n / 32` (states.txt rows). Live 2026-11-15 on monsters (Moo Moo Farm,
    /// terrorized): all 24 cows had [`super::state::DESECRATED`] set here, the player and the
    /// druid's wolves not. A second copy at +0xB50 also had it (synced or previous; not read).
    pub const STATE_FLAGS: u64 = 0xB30;
    pub const STATE_FLAG_DWORDS: usize = 8;
}

/// State ids (states.txt rows), bits of [`stat_list::STATE_FLAGS`].
pub mod state {
    /// `desecrated`: the Terror Zone state of a monster. Live-verified 2026-11-15 on monsters of
    /// a terrorized area; NOT yet checked that monsters of a normal area have it clear.
    /// (State 56 `terror` is an unrelated fear effect.)
    pub const DESECRATED: u32 = 189;
}

/// The game's items table (one record per items.txt row, weapons then armor then misc; D2R row =
/// classic row + 15). Found at run time on the heap (`items::find_table`).
pub mod items_txt {
    /// Bytes per record.
    pub const STRIDE: u64 = 0x1C0;
    /// 4-byte item code, space-padded (`gcg `, `r01 `).
    pub const CODE: u64 = 0x00;
}
