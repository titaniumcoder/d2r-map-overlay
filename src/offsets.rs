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
    /// ptr to the unit's `Inventory` (see [`super::inventory`]); live 2026-11-15 on player units,
    /// 2026-11-18 on socketed item units (the chain holds the socketed runes/gems/jewels).
    pub const INVENTORY: u64 = 0x90;
    /// ptr to the player's [`super::skills`] header. Live-verified 2026-11-17 (Warlock, clvl 38,
    /// python probe `skillprobe.py`): the header's first-skill chain held 19 skills — Attack,
    /// Kick, Throw, Unsummon, the two scrolls and tomes, and the Warlock's skills with points
    /// plus the two granted only by gear.
    pub const SKILLS: u64 = 0x100;
    /// ptr to the next unit in the hash chain (same slot = same `id & 127`). Verified live
    /// 2026-11-15 for items (43 chained units), monsters (24) and objects (1): every chained unit
    /// had the slot's type and `id & 127 == slot`. The earlier 0x150 only ever reached slot heads,
    /// so a unit sharing a slot was invisible (a dropped sapphire never showed).
    pub const NEXT: u64 = 0x158;
}

/// The player's skill list header ([`unit::SKILLS`]). Live-verified 2026-11-17.
pub mod skills {
    /// ptr to the first [`super::skill`] node; the rest follow [`super::skill::NEXT`].
    pub const FIRST: u64 = 0x00;
    /// ptr to the skill node on the left mouse button. Live: pointed at the list's Attack node.
    pub const LEFT: u64 = 0x08;
    /// ptr to the skill node on the right mouse button. Live: pointed at the Death Mark (375)
    /// node of the same list. Left/right follow the community layout; not yet seen changing.
    pub const RIGHT: u64 = 0x10;
}

/// One node of the player's skill list (`Skill`). Live-verified 2026-11-17 on a level 38
/// Warlock: the class skills' [`HARD_POINTS`] were Summon Goatman 16, Demonic Mastery 10,
/// Death Mark 1, Summon Tainted 1, Summon Defiler 3, Blood Oath 1, Engorge 1, Blood Boil 1,
/// Bind Demon 7 = 41 = clvl 38 − 1 + 4 Normal quest points, with no unspent points (no
/// `newskills` stat on the player). Miasma Bolt and Enhanced Entropy were in the list with 0:
/// the gear grants them (+1 each, item stat 107 `item_singleskill` on weapon and off-hand).
/// No field in the first 0x100 bytes carried that +1 bonus (Summon Goatman, +1 from the
/// weapon, differs from Demonic Mastery only in the hard points and flags), so the node holds
/// base points only; the bonus is item stats, not read.
pub mod skill {
    /// ptr to the skill's skills.txt record ([`super::skill_txt`]).
    pub const TXT: u64 = 0x00;
    /// ptr to the next node; null ends the list.
    pub const NEXT: u64 = 0x08;
    /// u32 hard (allocated) points; 1 on Attack/Kick/Throw and the scrolls/tomes. (The u32 at
    /// `+0x40` is a tome's charge count, live 15 identify / 17 town portal; not read.)
    pub const HARD_POINTS: u64 = 0x38;
}

/// The game's skills.txt record a [`skill::TXT`] points to.
pub mod skill_txt {
    /// u16 skills.txt row (`*Id`). Live: 0 Attack, 217–220 scrolls/tomes, 373–402 Warlock.
    pub const ID: u64 = 0x00;
}

/// `Inventory` of a unit.
pub mod inventory {
    /// u32 unit id of the player the inventory belongs to. Live 2026-11-15: the player's own
    /// inventory and its shared-stash tab units (player-type units without an act, one per shared
    /// page) carry the player's id here; stale tab units left from other characters or the
    /// offline shared stash carry another id. Cathan's Seal moved page 1→5 through tab units
    /// 3537311729, 4184574968, 2092279292, 1046131454, 523057535, all with this field = Vex's id.
    pub const OWNER_ID: u64 = 0x48;
    /// ptr to the first item unit stored in this inventory (null when empty); the rest follow
    /// through [`super::item_data::NEXT_IN_INVENTORY`]. Live 2026-11-17 on the shared-stash tab
    /// units: page 5 chained Sigon's Visor and Shelter, the gems/runes/materials tab its 49
    /// stacks. `+0x18` is the last item and the u32 at `+0x4C` the item count (2 and 49 then).
    pub const FIRST_ITEM: u64 = 0x10;
    /// Candidate offsets of the player's shared-stash tab list header: ptr to the first node
    /// ([`super::stash_tab_node`]), then [`TAB_LIST_LAST`] and [`TAB_LIST_COUNT`] relative to
    /// it; the nodes are in the game's UI order. Live 2026-11-17 on the PLAYER unit's
    /// inventory, two games: the header was at `+0x60` in one (six nodes, the five normal
    /// pages then the gems/runes/materials tab; page 5's tab unit held Sigon's set, as in the
    /// UI) and at `+0x68` in the next (`+0x60` null), so the reader accepts the first
    /// candidate whose first → last walk matches the count.
    pub const TAB_LIST: [u64; 2] = [0x60, 0x68];
    /// ptr to the last node, relative to a [`TAB_LIST`] header.
    pub const TAB_LIST_LAST: u64 = 0x08;
    /// u32 node count (6 live), relative to a [`TAB_LIST`] header.
    pub const TAB_LIST_COUNT: u64 = 0x10;
}

/// A node of the player inventory's shared-stash tab list ([`inventory::TAB_LIST`]).
/// Live 2026-11-17: `+0x04` u32 was 0x10 on every node, `+0x08` u64 `0x6ffffb000100` on the
/// normal pages and `0x6ffffb010100` on the gems/runes/materials tab.
pub mod stash_tab_node {
    /// u32 unit id of the tab unit (a player-type unit without an act).
    pub const TAB_ID: u64 = 0x00;
    /// u8: 1 on the gems/runes/materials ("advanced") tab, 0 on the normal pages. A hint seen
    /// on one account (one advanced tab, five pages), not proven to be a kind field.
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

/// The client's quest flags of the current game: a `.data` global points to a `BitBuffer`
/// header, which points to 48 little-endian u16 words, one per quest (classic d2s order,
/// see [`crate::quests::QUESTS`]). Live-verified 2026-11-18 (druid "Elemental", clvl 21,
/// Normal, Acts 1–2 done, research/quests.py): the global sat at rva 0x1edaf78 (build
/// `6ad81202-279c000`), found by a structural search of `.data` (exactly two adjacent slots
/// point to a header of this shape; the first is this record, the second a record of
/// unknown meaning with bits 13/15 on finished quests). `PlayerData +0x40/+0x48/+0x50` hold
/// headers of the same shape per difficulty, but on the client they stay all zero.
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
    /// Bit 0 of a quest word: reward granted (D2MOO `QFLAG_REWARDGRANTED`): the quest is done
    /// and its reward taken. Live: set on every finished Act 1–2 quest.
    pub const REWARD_GRANTED: u16 = 0x0001;
    /// Bit 1: reward pending (`QFLAG_REWARDPENDING`): done, reward not yet taken (Charsi's
    /// imbue, Larzuk's socket); D2MOO clears it when the reward is granted. Live: set alone
    /// on word 41 (Akara's respec, unused), clear on Tools of the Trade (imbue used).
    pub const REWARD_PENDING: u16 = 0x0002;
}

/// `PlayerData` (unit data of a player).
pub mod player_data {
    /// Character name, NUL-terminated.
    pub const NAME: u64 = 0x00;
    pub const NAME_LEN: usize = 16;
}

/// `MonsterData` (unit data of a monster).
pub mod monster_data {
    /// u32 owner type (pet chain hypothesis: classic layouts carry owner type/id in the first
    /// two dwords; NOT yet live-verified — journal logs both for the merc check).
    pub const OWNER_TYPE: u64 = 0x00;
    /// u32 owner unit id: the local player's id on their merc/pets (hypothesis, see above).
    pub const OWNER_ID: u64 = 0x04;
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
    /// `FLAGS` bit of a finished runeword. Live 2026-11-18: set (flags `0x0480_0810`) on an
    /// equipped Stealth armour and Leaf staff, clear on the other equipped items.
    pub const FLAG_RUNEWORD: u32 = 0x0400_0000;
    /// `FLAGS` bit of an identified item (classic; live: set on an identified quest unique).
    pub const FLAG_IDENTIFIED: u32 = 0x10;
    /// u32 uniqueitems / setitems row (`*ID`) of a unique / set item. Live-verified 2026-11-15 for
    /// an identified unique (Khalim's Flail: 127 = uniqueitems `KhalimFlail`). Sets verified
    /// live too (2026-11-17: the stash reader resolves Sigon's set pieces via this field),
    /// and whether it is filled in before the item is identified (so only trusted with
    /// [`FLAG_IDENTIFIED`]).
    pub const FILE_INDEX: u64 = 0x34;
    /// u8 body slot of an equipped item (0 none; 1 helm, 3 armour, 4 weapon…). Live 2026-11-17:
    /// the local player's equipped items carry their slot here, the mercenary's too.
    pub const BODY: u64 = 0x54;
    /// u8 storage page of a stored item: `PAGE_INVENTORY` 0, stash 4; 255 when equipped (body
    /// slot in the byte at `+0x54`), on the ground or on the cursor. Live-verified 2026-11-15
    /// over 74 items.
    pub const PAGE: u64 = 0x55;
    pub const PAGE_INVENTORY: u8 = 0;
    /// The Horadric Cube (live 2026-11-15, owner = the player's unit id).
    pub const PAGE_CUBE: u8 = 3;
    /// Personal stash (owner = the player's unit id) and the shared stash tabs (live
    /// 2026-11-15: each tab is its own pseudo-owner id, no player's). Also items of owner
    /// [`NO_OWNER`], all at (0,0) with quality 2: the stacks of the gems/runes/materials tab
    /// (live 2026-11-17, reached through that tab's [`super::inventory::FIRST_ITEM`] chain).
    pub const PAGE_STASH: u8 = 4;
    /// u32 stack size of an item on the shared stash's gems/runes/materials tab. Live
    /// 2026-11-17: rvs 50, rvl 43, gcy 19, r01 8, gzv 0 (an emptied stack stays in the tab);
    /// those items carry no stat list, [`OWNER`] [`NO_OWNER`] and [`PAGE`] [`PAGE_STASH`].
    pub const QUANTITY: u64 = 0x9C;
    /// ptr to the next item unit in the same `Inventory` (see
    /// [`super::inventory::FIRST_ITEM`]); null ends the chain. Live 2026-11-17 on the shared
    /// stash tabs (every item of each tab, the advanced tab's stacks included); live 2026-11-18
    /// also on a socketed item's own inventory (its runes, in socket order).
    pub const NEXT_IN_INVENTORY: u64 = 0xB0;
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
    /// Stat id `level` (itemstatcost.txt): the character level on a player unit. Not yet
    /// live-verified; the reader also accepts the value shifted left by 8.
    pub const LEVEL: u16 = 12;
    /// Stat ids of the four elemental resistances as granted by gear (itemstatcost.txt; the txt
    /// interleaves fire/light/cold/poison with their max counterparts, so light is 41, cold 43).
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
    /// ptr to the unit's next stat list (the chain: base list → magic-affix list → …; each
    /// list's `+0x08/+0x0C` owner (unit type, unit id) tells whose it is). Live 2026-11-17:
    /// an equipped item's base list chained to its affix list (res/FHR on it), so item-granted
    /// stats are readable by walking this chain per item — the player's own `+0x88` list is
    /// BASE-ONLY (level/strength/…, no item stats).
    pub const NEXT: u64 = 0xD0;
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
