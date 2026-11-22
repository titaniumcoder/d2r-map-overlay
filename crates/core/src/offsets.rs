//! Verified D2R struct offsets: byte offsets from each struct's base address, pointers are 8 bytes.
//! Provenance and live-verification notes are in `PLAN.md` "Memory layer".

/// Unit hash table: per unit type, `SLOTS` chain heads of 8 bytes each; type `t` starts at
/// `table + t * SLOTS * SLOT_SIZE`.
pub mod unit_table {
    pub const SLOTS: u64 = 128;
    pub const SLOT_SIZE: u64 = 8;
    /// Unit type ids (also the value of `unit::TYPE`).
    pub const PLAYER: u32 = 0;
    pub const MONSTER: u32 = 1;
    /// Objects (shrines, wells, chests, ...); only those near the player are loaded.
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
    /// ptr to the unit's `StatList`.
    pub const STATS: u64 = 0x88;
    /// ptr to the unit's `Inventory` (see [`super::inventory`]); on a socketed item it holds the
    /// socketed runes/gems/jewels.
    pub const INVENTORY: u64 = 0x90;
    /// ptr to the player's [`super::skills`] header.
    pub const SKILLS: u64 = 0x100;
    /// ptr to the next unit in the hash chain (same slot = same `id & 127`).
    pub const NEXT: u64 = 0x158;
}

/// The player's skill list header ([`unit::SKILLS`]).
pub mod skills {
    /// ptr to the first [`super::skill`] node; the rest follow [`super::skill::NEXT`].
    pub const FIRST: u64 = 0x00;
    /// ptr to the skill node on the left mouse button.
    pub const LEFT: u64 = 0x08;
    /// ptr to the skill node on the right mouse button.
    pub const RIGHT: u64 = 0x10;
}

/// One node of the player's skill list (`Skill`). Holds base points only: gear-granted skills
/// sit in the list with 0 hard points and their bonus lives in item stats.
pub mod skill {
    /// ptr to the skill's skills.txt record ([`super::skill_txt`]).
    pub const TXT: u64 = 0x00;
    /// ptr to the next node; null ends the list.
    pub const NEXT: u64 = 0x08;
    /// u32 hard (allocated) points; 1 on Attack/Kick/Throw and the scrolls/tomes.
    pub const HARD_POINTS: u64 = 0x38;
}

/// The game's skills.txt record a [`skill::TXT`] points to.
pub mod skill_txt {
    /// u16 skills.txt row (`*Id`).
    pub const ID: u64 = 0x00;
}

/// `Inventory` of a unit.
pub mod inventory {
    /// u32 unit id of the player the inventory belongs to; stale shared-stash tab units left from
    /// other characters or the offline stash carry another id.
    pub const OWNER_ID: u64 = 0x48;
    /// ptr to the first item unit stored in this inventory (null when empty); the rest follow
    /// through [`super::item_data::NEXT_IN_INVENTORY`].
    pub const FIRST_ITEM: u64 = 0x10;
    /// Candidate offsets of the player's shared-stash tab list header (ptr to the first
    /// [`super::stash_tab_node`], nodes in UI order). It moves between games, so the reader takes
    /// the first candidate whose first → last walk matches [`TAB_LIST_COUNT`].
    pub const TAB_LIST: [u64; 2] = [0x60, 0x68];
    /// ptr to the last node, relative to a [`TAB_LIST`] header.
    pub const TAB_LIST_LAST: u64 = 0x08;
    /// u32 node count, relative to a [`TAB_LIST`] header.
    pub const TAB_LIST_COUNT: u64 = 0x10;
}

/// A node of the player inventory's shared-stash tab list ([`inventory::TAB_LIST`]).
pub mod stash_tab_node {
    /// u32 unit id of the tab unit (a player-type unit without an act).
    pub const TAB_ID: u64 = 0x00;
    /// u8: 1 on the gems/runes/materials ("advanced") tab, 0 on the normal pages (a hint, not a
    /// proven kind field).
    pub const ADVANCED: u64 = 0x0A;
    /// ptr to the next node; null ends the list.
    pub const NEXT: u64 = 0x10;
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
    /// of adjacent levels, so a walk-through border to another level shows up here.
    pub const NEAR: u64 = 0x10;
    /// u32 number of entries in [`NEAR`] (`+0x20` is a different field despite looking similar).
    pub const NEAR_COUNT: u64 = 0x18;
    /// ptr to the room's level-preset record (`level_preset`).
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
    /// ptr to `ActMisc`.
    pub const MISC: u64 = 0x70;
}

/// `ActMisc` (per-act game data).
pub mod act_misc {
    /// u16 difficulty: 0 Normal, 1 Nightmare, 2 Hell.
    pub const DIFFICULTY: u64 = 0x830;
    /// u32 level id of the real Tal Rasha's Tomb (66..=72, the one with the orifice), set in
    /// Act 2 (`dwStaffTombLevel`).
    pub const STAFF_TOMB_LEVEL: u64 = 0x120;
}

/// The client's quest flags of the current game: a `.data` global points to a `BitBuffer`
/// header, which points to 48 little-endian u16 words, one per quest (classic d2s order, see
/// [`crate::quests::QUESTS`]). The per-difficulty copies in `PlayerData` stay zero on the client.
pub mod quest_flags {
    /// ptr to the word buffer.
    pub const BUFFER: u64 = 0x00;
    /// u32 size in bits.
    pub const BITS: u64 = 0x08;
    /// u64 size in bytes.
    pub const BYTES: u64 = 0x28;
    /// The header's size (what the structural search reads).
    pub const HEADER_LEN: usize = 0x30;
    /// Expected [`BITS`] and [`BYTES`] values.
    pub const BITS_VALUE: u32 = 0x300;
    pub const BYTES_VALUE: u64 = 0x60;
    /// Bit of a quest word: done and its reward taken (D2MOO `QFLAG_REWARDGRANTED`).
    pub const REWARD_GRANTED: u16 = 0x0001;
    /// Bit of a quest word: done, reward not yet taken, e.g. Charsi's imbue (`QFLAG_REWARDPENDING`).
    pub const REWARD_PENDING: u16 = 0x0002;
}

/// A `.data` byte array of open-panel flags (0 or 1) in d2go's open-menus layout
/// (2026-11-22 UI recordings, PLAN.md "In-game UI marks"). Its rva is known for one build.
pub mod ui_panels {
    /// The build (`"<TimeDateStamp>-<SizeOfImage>"`) [`RVA`] was recorded on.
    pub const BUILD: &str = "6ad81202-279c000";
    pub const RVA: u64 = 0x01ec_e2d8;
    /// Bytes compared by the UI probe (d2go reads 0x16D).
    pub const LEN: usize = 0x170;
    /// Bytes read for the flags below (`+0x20` is a frame counter).
    pub const FLAGS: usize = 0x20;
    /// u8 1 while the inventory is shown (alone, beside the stash or a trade window).
    pub const INVENTORY: usize = 0x01;
    /// u8 1 while talking to an NPC (the trade window included).
    pub const NPC_INTERACT: usize = 0x08;
    /// u8 1 while a trade window is open.
    pub const NPC_SHOP: usize = 0x0B;
    /// u8 1 while the stash window is open.
    pub const STASH: usize = 0x18;
    /// u8 1 while the Horadric Cube is open (d2go; recording 1's +0x19 changes).
    pub const CUBE: usize = 0x19;
}

/// `PlayerData` (unit data of a player).
pub mod player_data {
    /// Character name, NUL-terminated.
    pub const NAME: u64 = 0x00;
    pub const NAME_LEN: usize = 16;
}

/// `MonsterData` (unit data of a monster).
pub mod monster_data {
    /// u32 owner type (hypothesis, not live-verified; the journal logs it for the merc check).
    pub const OWNER_TYPE: u64 = 0x00;
    /// u32 owner unit id: the local player's id on their merc/pets (hypothesis).
    pub const OWNER_ID: u64 = 0x04;
    /// u16 name seed (unique/superunique names).
    pub const NAME_SEED: u64 = 0x18;
    /// u8 flag byte (classic bits: superunique 0x02, champion 0x04, unique 0x08, minion 0x10).
    pub const FLAGS: u64 = 0x1A;
    /// `FLAGS` bit of a superunique.
    pub const SUPERUNIQUE: u8 = 0x02;
}

/// `StaticPath`: the path of a unit that doesn't move (items and objects; `path` is for moving
/// units).
pub mod static_path {
    /// u32 world x (tiles); a stored or store item's grid column (classic `dwPosX`, as d2go
    /// reads it; not yet seen live).
    pub const X: u64 = 0x10;
    /// u32 world y (tiles); a stored or store item's grid row.
    pub const Y: u64 = 0x14;
}

/// Item unit modes (`unit::MODE` of an item).
pub mod item_mode {
    /// Lying on the ground.
    pub const ON_GROUND: u32 = 3;
    /// In a storage page: inventory, stash, cube (`item_data::PAGE` tells which).
    pub const STORED: u32 = 0;
}

/// Object unit modes (`unit::MODE` of an object).
pub mod object_mode {
    /// Neutral: an untouched shrine; any other mode means used. Wells refill, so they ignore it.
    pub const NEUTRAL: u32 = 0;
}

/// `ObjectData` (unit data of an object).
pub mod object_data {
    /// u8 shrine type = shrines.txt code (1..=22); only meaningful on shrine objects.
    pub const SHRINE_TYPE: u64 = 0x08;
}

/// `ItemData` (unit data of an item).
pub mod item_data {
    /// u32 quality (classic D2: 1 low, 2 normal, 3 superior, 4 magic, 5 set, 6 rare, 7 unique,
    /// 8 crafted).
    pub const QUALITY: u64 = 0x00;
    pub const QUALITY_SET: u32 = 5;
    pub const QUALITY_UNIQUE: u32 = 7;
    pub const QUALITY_LOW: u32 = 1;
    pub const QUALITY_NORMAL: u32 = 2;
    pub const QUALITY_SUPERIOR: u32 = 3;
    pub const QUALITY_MAGIC: u32 = 4;
    pub const QUALITY_RARE: u32 = 6;
    pub const QUALITY_CRAFTED: u32 = 8;
    /// u32 unit id of the player owning the item; ground items carry [`NO_OWNER`]. Only the
    /// local client's own stash is in memory, so the stash owner is the local player.
    pub const OWNER: u64 = 0x0C;
    /// `OWNER` of an item nobody owns (ground items).
    pub const NO_OWNER: u32 = 0xFFFF_FFFF;
    /// u32 item flags (`0x0080_0000` seen on unidentified magic items).
    pub const FLAGS: u64 = 0x18;
    /// `FLAGS` bit of an ethereal item (classic; unverified).
    pub const FLAG_ETHEREAL: u32 = 0x0040_0000;
    /// `FLAGS` bit of a finished runeword.
    pub const FLAG_RUNEWORD: u32 = 0x0400_0000;
    /// `FLAGS` bit of an identified item.
    pub const FLAG_IDENTIFIED: u32 = 0x10;
    /// `FLAGS` bit of an item for sale in a vendor's trade window (classic `ITEMFLAG_INSTORE`).
    pub const FLAG_IN_STORE: u32 = 0x2000;
    /// u32 uniqueitems / setitems row (`*ID`) of a unique / set item; only trusted with
    /// [`FLAG_IDENTIFIED`].
    pub const FILE_INDEX: u64 = 0x34;
    /// u16 magic prefix id (first of three; 0 = none): `PREFIX_BASE` (785) + the magicprefix.txt
    /// row (`affixes_data::PREFIXES`).
    pub const MAGIC_PREFIX: u64 = 0x48;
    /// u16 magic suffix id (first of three, `+0x4E/+0x50/+0x52`; 0 taken as none) = the
    /// magicsuffix.txt row.
    pub const MAGIC_SUFFIX: u64 = 0x4E;
    /// u8 body slot of an equipped item (0 none; 1 helm, 3 armour, 4 weapon…), the merc's too.
    pub const BODY: u64 = 0x54;
    /// u8 storage page of a stored item: `PAGE_INVENTORY` 0, stash 4; 255 when equipped, on the
    /// ground or on the cursor. A store item's page is its trade tab (0 armor, 1–2 weapons, 3 misc).
    pub const PAGE: u64 = 0x55;
    pub const PAGE_INVENTORY: u8 = 0;
    /// The Horadric Cube.
    pub const PAGE_CUBE: u8 = 3;
    /// Personal stash and the shared stash tabs (each tab its own pseudo-owner id); also the
    /// [`NO_OWNER`] stacks of the gems/runes/materials tab.
    pub const PAGE_STASH: u8 = 4;
    /// u32 stack size of an item on the shared stash's gems/runes/materials tab (an emptied
    /// stack stays at 0); those items carry no stat list.
    pub const QUANTITY: u64 = 0x9C;
    /// ptr to the next item unit in the same `Inventory` (see
    /// [`super::inventory::FIRST_ITEM`]); null ends the chain. Socketed items chain in socket order.
    pub const NEXT_IN_INVENTORY: u64 = 0xB0;
    /// Bytes of `ItemData` copied into the journal's drop event for checking the offsets above.
    pub const HEAD_LEN: usize = 0x50;
}

/// `StatList` (unit `+0x88`).
pub mod stat_list {
    /// ptr to the array of [`STAT_SIZE`]-byte entries `{u16 layer, u16 stat id, u32 value}`.
    pub const ENTRIES: u64 = 0x30;
    /// u64 entry count.
    pub const COUNT: u64 = 0x38;
    pub const STAT_SIZE: u64 = 8;
    /// Stat id `item_numsockets` (itemstatcost.txt).
    pub const NUM_SOCKETS: u16 = 194;
    /// Stat id `level`: the character level on a player unit (the reader also accepts it << 8).
    pub const LEVEL: u16 = 12;
    /// Stat ids `strength` and `dexterity` (itemstatcost.txt), plain values.
    pub const STRENGTH: u16 = 0;
    pub const DEXTERITY: u16 = 2;
    /// Stat ids of the four elemental resistances (itemstatcost.txt interleaves them with their
    /// max counterparts, so light is 41, cold 43).
    pub const RESIST_FIRE: u16 = 39;
    pub const RESIST_LIGHTNING: u16 = 41;
    pub const RESIST_COLD: u16 = 43;
    pub const RESIST_POISON: u16 = 45;
    /// Stat id `item_fasterattackrate` (IAS, %).
    pub const IAS: u16 = 93;
    /// Stat id `item_fastergethitrate` (FHR, %).
    pub const FHR: u16 = 99;
    /// Stat id `item_fastercastrate` (FCR, %).
    pub const FCR: u16 = 105;
    /// ptr to the unit's next stat list (base list → magic-affix list → …; [`OWNER_TYPE`] /
    /// [`OWNER_ID`] tell whose). The player's own list is base-only, without item stats.
    pub const NEXT: u64 = 0xD0;
    /// u32 list flags.
    pub const FLAGS: u64 = 0x1C;
    /// [`FLAGS`] bit of a unit's top list, which also carries the aggregated [`FULL_ENTRIES`].
    pub const FLAG_FULL_STATS: u32 = 0x8000_0000;
    /// ptr / u64 count of the aggregated stat array (layout as [`ENTRIES`]): base, affix,
    /// runeword and socketed stats summed with modifiers applied (ED% on defence and damage).
    pub const FULL_ENTRIES: u64 = 0xE8;
    pub const FULL_COUNT: u64 = 0xF0;
    /// The unit's state bitfield: [`STATE_FLAG_DWORDS`] little-endian u32, state `n` is bit
    /// `n % 32` of dword `n / 32` (states.txt rows).
    pub const STATE_FLAGS: u64 = 0xB30;
    pub const STATE_FLAG_DWORDS: usize = 8;
    /// u32 owner unit type and unit id of a list (see [`NEXT`]).
    pub const OWNER_TYPE: u64 = 0x08;
    pub const OWNER_ID: u64 = 0x0C;
}

/// State ids (states.txt rows), bits of [`stat_list::STATE_FLAGS`].
pub mod state {
    /// `desecrated`: the Terror Zone state of a monster (state 56 `terror` is unrelated).
    /// Not yet checked that monsters of a normal area have it clear.
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
