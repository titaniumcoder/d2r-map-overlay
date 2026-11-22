//! Reads one snapshot of the game state into the core's model ([`d2r_core::game`]). Lists change
//! while we read, so a bad entry is only a warning; only a missing player (or level) fails.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use d2r_core::game::{
    Addr, BorderLink, Bounds, Candidate, ChosenBy, Equipped, ItemDrop, Level, Monster, OperatedObject, Place,
    Player, Preset, PresetKind, Room, RoomLink, RoomPos, RoomRect, Shrine, Skills, Snapshot, SpeedStats,
    Well, char_level_from_raw, difficulty_from_raw, has_state, identified_unique_or_set, is_shrine_object,
    is_terrorized, is_town, is_well_object, magic_or_better, maybe_white_base,
};
use d2r_core::items::{self, DropKind};
use d2r_core::objects::is_consumable_quest_object;
use d2r_core::offsets::{
    ROOM_TILE_SCALE, act, act_misc, inventory, item_data, item_mode, level, level_preset, monster_data,
    object_data, object_mode, path, player_data, preset, room_tile, room1, room2, skill, skill_txt, skills,
    stash_tab_node, stat_list, state, static_path, unit, unit_table,
};
use d2r_core::reader::ReadMem;
use d2r_core::skills_data;
use d2r_core::stash::{self, StashTab};

const MAX_UNITS_PER_TYPE: usize = 1000;
const MAX_ROOMS: usize = 5000;
const MAX_PRESETS_PER_ROOM: usize = 500;
/// A socketed rare has a few dozen.
const MAX_ITEM_STATS: u64 = 512;
/// An item's own stat lists (base, affixes, ...) walked along [`stat_list::NEXT`].
const MAX_ITEM_STAT_LISTS: usize = 8;
const MAX_LINKS_PER_ROOM: usize = 100;
/// Seen: around 9.
const MAX_NEAR_ROOMS: u32 = 64;
const MAX_WARNINGS: usize = 50;
/// A tab holds 10×10; the materials tab had 49 stacks.
const MAX_STASH_ITEMS_PER_TAB: usize = 100;
/// Seen: 6.
const MAX_STASH_TABS: usize = 16;
const MAX_SOCKETS: usize = 6;
/// Seen: ~60 at Ormus.
const MAX_SHOP_ITEMS: usize = 300;
/// Seen: 19 (a class's 30 skills plus basic and gear-granted ones).
const MAX_SKILLS: usize = 128;

/// Agreeing owner ids (`item_data::OWNER`) after which a storage page stops voting.
const OWNER_VOTES: u32 = 8;

/// Shrine types (shrines.txt codes) of a real shrine; 0 is "none".
const SHRINE_TYPES: std::ops::RangeInclusive<u8> = 1..=22;

/// `item_data::BODY` of the right-hand weapon.
const WEAPON_BODY_SLOT: u8 = 4;
const CHARM_CODES: [&str; 3] = ["cm1", "cm2", "cm3"];

/// True for an `item_data::BODY` value of a worn item (0 and 255 are "not worn").
const fn is_worn(body: u8) -> bool {
    body != 0 && body != 255
}

/// The plain value of a raw stat word (two's-complement `i32`): some stats are stored in 1/256
/// units, recognised as a positive value above 255 with 8 trailing zeros.
const fn plain_stat_from_raw(raw: u32) -> i32 {
    let value = raw.cast_signed();
    if value > 255 && value.trailing_zeros() >= 8 {
        value >> 8
    } else {
        value
    }
}

/// Adds the resists on the player's own list ([`Reader::own_resists`]: the quest resists) to
/// `player`'s gear sums, and warns when the player's aggregated array disagrees with the result.
fn add_own_resists(r: &mut Reader<'_, impl ReadMem>, player: &mut Player, addr: u64) {
    let (own, aggregated) = r.own_resists(addr);
    if own != [0; 4] {
        add_resists(player.speed.get_or_insert_default(), own);
    }
    let summed = player.speed.map_or([0; 4], |s| {
        [s.res_fire, s.res_cold, s.res_lightning, s.res_poison]
    });
    if let Some(aggregated) = aggregated
        && aggregated != summed
    {
        r.warn(format!(
            "player resists: aggregated array {aggregated:?} vs gear + own list {summed:?} \
             (fire/cold/lightning/poison)"
        ));
    }
}

const fn add_resists(speed: &mut SpeedStats, [fire, cold, lightning, poison]: [i32; 4]) {
    speed.res_fire = speed.res_fire.saturating_add(fire);
    speed.res_cold = speed.res_cold.saturating_add(cold);
    speed.res_lightning = speed.res_lightning.saturating_add(lightning);
    speed.res_poison = speed.res_poison.saturating_add(poison);
}

/// Adds the speed and resist stats among `entries` (`{layer, id, raw}`) to `acc`.
fn add_speed_stats(acc: &mut SpeedStats, entries: &[(u16, u16, u32)]) {
    for &(_, id, raw) in entries {
        let value = plain_stat_from_raw(raw);
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
}

/// The four resists summed over stat `entries` (`{layer, id, raw}`), as
/// `[fire, cold, lightning, poison]`.
fn resists_of(entries: &[(u16, u16, u32)]) -> [i32; 4] {
    let mut out = [0i32; 4];
    for &(_, id, raw) in entries {
        let slot = match id {
            stat_list::RESIST_FIRE => 0,
            stat_list::RESIST_COLD => 1,
            stat_list::RESIST_LIGHTNING => 2,
            stat_list::RESIST_POISON => 3,
            _ => continue,
        };
        if let Some(v) = out.get_mut(slot) {
            *v = v.saturating_add(plain_stat_from_raw(raw));
        }
    }
    out
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

/// What one walk over the item units found. Per-owner maps are keyed by candidate unit id: the
/// player is chosen only after the scan, and in a group game every player's items are units.
#[derive(Debug, Default)]
struct ItemScan {
    /// Item units on the ground (shown or not).
    ground: u32,
    /// Ground items worth showing.
    drops: Vec<ItemDrop>,
    /// All items on the inventory page, with their owner id (`None` if unreadable).
    inventory: Vec<(ItemDrop, Option<u32>)>,
    /// Cube and stash items kept by [`stored_kept`], any owner.
    stored: Vec<Stored>,
    stash_owners: OwnerVotes,
    inventory_owners: OwnerVotes,
    /// Speed/resist stats summed over equipped items and carried charms.
    gear: HashMap<u32, SpeedStats>,
    /// The chain-walk fallback of [`Reader::accumulate_gear`] is warned once per scan.
    gear_fallback_warned: bool,
    /// The equipped weapon's WSM.
    weapons: HashMap<u32, i32>,
    /// Identified unique/set and runeword names (equipped, carried, stashed).
    owned_names: HashMap<u32, Vec<String>>,
    /// Equipped `(body slot, quality)` pairs.
    equipped: HashMap<u32, Vec<(u8, u32)>>,
    /// The equipped items for the gear grid, one per body slot.
    equipped_items: HashMap<u32, Vec<Equipped>>,
    /// The mercenary's (owner `NO_OWNER`) equipped `(body slot, quality)` pairs.
    merc_equipped: Vec<(u8, u32)>,
    /// One name per runeword copy; the mercenary's worn ones under `NO_OWNER`.
    runeword_copies: HashMap<u32, Vec<String>>,
    merc_worn: Vec<String>,
}

impl ItemScan {
    /// Counts `owner` of an item on storage `page` in that page's vote; only `candidates` vote,
    /// so a shared-stash tab's pseudo-owner never wins.
    fn vote(&mut self, page: Option<u8>, owner: Option<u32>, candidates: &[u32]) {
        let votes = match page {
            Some(item_data::PAGE_STASH) => &mut self.stash_owners,
            Some(item_data::PAGE_INVENTORY) => &mut self.inventory_owners,
            _ => return,
        };
        if let Some(owner) = owner
            && !votes.done()
            && candidates.contains(&owner)
        {
            votes.add(owner);
        }
    }
}

/// Appends `name` to `names` unless it is there already ([`d2r_core::names::same_name`]).
fn push_unique_name(names: &mut Vec<String>, name: String) {
    if !names.iter().any(|n| d2r_core::names::same_name(n, &name)) {
        names.push(name);
    }
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

/// The tab list's `(tab unit id, advanced)` `nodes` as tabs: normal pages numbered 1-based in
/// list order, the advanced node the materials tab, each with its unit from `units` if loaded.
fn tabs_in_list_order(nodes: Vec<(u32, bool)>, units: &[(u32, u64)]) -> Vec<TabRef> {
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

/// The tab `units` as normal pages in their order.
fn tabs_in_unit_order(units: Vec<(u32, u64)>) -> Vec<TabRef> {
    units
        .into_iter()
        .enumerate()
        .map(|(i, (id, addr))| TabRef {
            id,
            addr: Some(addr),
            kind: stash::TabKind::Page(u8::try_from(i + 1).unwrap_or(u8::MAX)),
        })
        .collect()
}

/// A stored item with its `item_data::PAGE` and `item_data::OWNER`.
#[derive(Debug)]
struct Stored {
    item: ItemDrop,
    page: u8,
    owner: u32,
}

/// Whether a stored item read in full is kept in [`Snapshot::owned_items`].
fn stored_kept(item: &ItemDrop) -> bool {
    identified_unique_or_set(item.quality, item.flags)
        || magic_or_better(item.quality)
        || (maybe_white_base(item.quality, item.flags)
            && item
                .code
                .as_deref()
                .is_some_and(d2r_core::socket_bases::socketable))
}

/// Where a stored item on `page` with `owner` is, for the player with unit id `player`; `None`
/// if it isn't the player's. Stale tab units of other characters own stash items too, so only
/// the player's own `shared_tabs` count.
fn place(page: u8, owner: u32, player: u32, shared_tabs: &[u32]) -> Option<Place> {
    match page {
        item_data::PAGE_INVENTORY if owner == player => Some(Place::Inventory),
        item_data::PAGE_CUBE if owner == player => Some(Place::Cube),
        item_data::PAGE_STASH if owner == player => Some(Place::Stash),
        item_data::PAGE_STASH if shared_tabs.contains(&owner) => Some(Place::Shared),
        _ => None,
    }
}

/// The items of unit id `player` with their place, from the scan's `inventory` and `stored`
/// items: the one set the keep list and the spare lists are computed from.
fn player_items(
    inventory: Vec<(ItemDrop, Option<u32>)>,
    stored: Vec<Stored>,
    player: u32,
    shared_tabs: &[u32],
) -> Vec<(ItemDrop, Place)> {
    let inv = inventory
        .into_iter()
        .filter_map(|(d, owner)| (owner == Some(player)).then_some((d, Place::Inventory)));
    let stashed = stored
        .into_iter()
        .filter_map(|s| place(s.page, s.owner, player, shared_tabs).map(|p| (s.item, p)));
    inv.chain(stashed).collect()
}

/// The candidate to use as the player and the rule that picked it, by precedence; `None` if no
/// candidate fits.
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

/// Whether the game is solo: one distinct player name (a stale duplicate unit doesn't count).
fn is_solo(candidates: &[Candidate]) -> bool {
    candidates
        .iter()
        .map(|c| c.name.as_str())
        .collect::<HashSet<_>>()
        .len()
        == 1
}

/// The chosen player's runeword copies and the merc's worn names. Merc gear counts only when
/// `solo`: nothing ties it to the local player's merc (PLAN.md, "Merc gear caveat").
fn held_copies(scan: &ItemScan, player: u32, solo: bool) -> (Vec<String>, Vec<String>) {
    let owners: &[u32] = if solo {
        &[player, item_data::NO_OWNER]
    } else {
        &[player]
    };
    let copies = owners
        .iter()
        .filter_map(|id| scan.runeword_copies.get(id))
        .flatten()
        .cloned()
        .collect();
    let merc_worn = if solo { scan.merc_worn.clone() } else { Vec::new() };
    (copies, merc_worn)
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

/// What [`walk_level`] collected.
struct LevelWalk {
    level: Level,
    presets: Vec<Preset>,
    room_links: Vec<RoomLink>,
    border_links: Vec<BorderLink>,
}

/// Walks `level_addr`'s Room2 chain while it stays inside this level (as in research/rooms.py);
/// a first room of another level is an error.
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
    let mut r = Reader::new(mem);

    let candidates = player_candidates(&mut r, unit_table);
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
    player.speed = scan.gear.get(&chosen.id).copied();
    add_own_resists(&mut r, &mut player, chosen.addr.0);
    player.wsm = scan.weapons.get(&chosen.id).copied();
    let equipped = scan.equipped.get(&chosen.id).cloned().unwrap_or_default();
    let merc_equipped = scan.merc_equipped.clone();
    let equipped_items = scan.equipped_items.get(&chosen.id).cloned().unwrap_or_default();
    let owned_names = scan.owned_names.get(&chosen.id).cloned().unwrap_or_default();
    let (runeword_copies, merc_worn) = held_copies(&scan, chosen.id, is_solo(&candidates));

    let LevelWalk {
        mut level,
        mut presets,
        room_links,
        border_links,
    } = walk_level(&mut r, level_addr, player.level_id).context("walking the level's rooms")?;
    level.bounds = bounds(&level.rooms);
    dedup_presets(&mut presets);

    let monsters = r.monsters(unit_table);
    let (shrines, wells, operated_objects) = r.objects(unit_table);
    let inventory_items = u32::try_from(scan.inventory.len()).unwrap_or(u32::MAX);
    let tabs = r.shared_tabs(unit_table, chosen.addr.0, chosen.id);
    let stash = r.stash(&tabs, items_table);
    let stash_names = stash::stash_names(&stash);
    let shared_tabs: Vec<u32> = tabs.iter().map(|t| t.id).collect();
    let owned_items = player_items(scan.inventory, scan.stored, chosen.id, &shared_tabs);
    let held_materials = stash::held_materials(owned_items.iter().map(|(item, _)| item));
    let white_bases = stash::white_bases(owned_items.iter().map(|(item, _)| item));
    let difficulty = player.act.and_then(|a| r.difficulty(a.0));
    let staff_tomb = player.act.and_then(|a| r.staff_tomb(a.0));
    let terrorized = is_terrorized(&monsters);
    let skills = r.skills(chosen.addr.0, player.class);
    let shop = if is_town(player.level_id) {
        r.shop(monsters.iter().map(|m| m.addr.0), items_table)
    } else {
        None
    };

    Ok(Snapshot {
        player,
        equipped,
        merc_equipped,
        equipped_items,
        owned_names,
        runeword_copies,
        merc_worn,
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
        owned_items,
        held_materials,
        white_bases,
        difficulty,
        staff_tomb,
        terrorized,
        cows: None,
        trail: Vec::new(),
        skills,
        shop,
        warnings: r.into_warnings(),
    })
}

/// Drops repeated presets, keeping the first of each: a Room2's preset list can hold repeats.
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

impl<'a, M: ReadMem> Reader<'a, M> {
    const fn new(mem: &'a M) -> Self {
        Self {
            mem,
            warnings: Vec::new(),
            suppressed: 0,
        }
    }

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

    /// The player's shared-stash tabs in the game's UI order ([`Reader::tab_list`]), each with
    /// its tab unit if loaded (a missing one is an empty tab, so page numbers never shift).
    /// Without a tab list, the tab units in unit-table order (pages only) with a warning.
    fn shared_tabs(&mut self, unit_table: u64, player_addr: u64, player: u32) -> Vec<TabRef> {
        let units = self.tab_units(unit_table, player);
        match self.tab_list(player_addr) {
            Ok(nodes) if !nodes.is_empty() => tabs_in_list_order(nodes, &units),
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
                tabs_in_unit_order(units)
            }
        }
    }

    /// `(unit id, address)` of the player's shared-stash tab units: other player-type units
    /// without an act whose inventory names `player` as owner ([`inventory::OWNER_ID`]).
    fn tab_units(&mut self, unit_table: u64, player: u32) -> Vec<(u32, u64)> {
        self.units(unit_table, unit_table::PLAYER)
            .into_iter()
            .filter_map(|addr| {
                let id = self.u32(addr, unit::ID).ok()?;
                let inv = self.ptr(addr, unit::INVENTORY).ok()??;
                (id != player
                    && self.ptr(addr, unit::ACT).ok()?.is_none()
                    && self.u32(inv, inventory::OWNER_ID).ok()? == player)
                    .then_some((id, addr))
            })
            .collect()
    }

    /// The `(tab unit id, advanced)` nodes of the shared-stash tab list in the player's
    /// inventory, from the first [`inventory::TAB_LIST`] header that checks out; empty if none.
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

    /// The node addresses of the tab list whose header is at `header`, if it is one: the walk
    /// from the first node reaches the last in exactly `count` (1..=[`MAX_STASH_TABS`]) steps.
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

    /// The shared-stash `tabs` with their items (stack sizes on the materials tab); unreadable
    /// items are skipped silently.
    fn stash(&mut self, tabs: &[TabRef], items_table: Option<u64>) -> Vec<StashTab> {
        tabs.iter()
            .map(|tab| {
                let materials = tab.kind == stash::TabKind::Materials;
                let mut items = Vec::new();
                for addr in tab.addr.map_or_default(|a| self.tab_items(a)) {
                    let Ok(head) = self.unit_head(addr, unit_table::ITEM) else {
                        continue;
                    };
                    let Ok(item) = self.owned_item_fields(addr, &head, items_table) else {
                        continue;
                    };
                    let quantity = head
                        .unit_data
                        .filter(|_| materials)
                        .and_then(|d| self.u32(d, item_data::QUANTITY).ok());
                    let mut entry = stash::stash_item(&item, quantity);
                    if let Some(word) = self.runeword_name(addr, &head, item.flags, items_table) {
                        entry.name = word;
                    }
                    items.push(entry);
                }
                StashTab {
                    kind: tab.kind,
                    items,
                }
            })
            .collect()
    }

    /// The first item in the inventory of the unit at `owner` (`None` if unreadable).
    fn first_inventory_item(&self, owner: u64) -> Option<u64> {
        let inv = self.ptr(owner, unit::INVENTORY).ok().flatten()?;
        self.ptr(inv, inventory::FIRST_ITEM).ok().flatten()
    }

    /// The item after `item` in its inventory chain (the link sits in its `ItemData`).
    fn next_in_inventory(&self, item: u64) -> Result<Option<u64>> {
        let data = self.ptr_req(item, unit::UNIT_DATA, "item data")?;
        self.ptr(data, item_data::NEXT_IN_INVENTORY)
    }

    /// The item unit addresses of the tab unit at `tab`, in its inventory's chain order; a
    /// broken, cyclic or runaway chain warns.
    fn tab_items(&mut self, tab: u64) -> Vec<u64> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = self.first_inventory_item(tab);
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
            cur = match self.next_in_inventory(item) {
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
            char_level: self
                .stat(addr, stat_list::LEVEL)
                .ok()
                .flatten()
                .and_then(char_level_from_raw),
            speed: None, // summed from the gear in [`snapshot`]

            wsm: None,
            strength: self.stat(addr, stat_list::STRENGTH).ok().flatten(),
            dexterity: self.stat(addr, stat_list::DEXTERITY).ok().flatten(),
        };
        Ok((player, level))
    }

    /// The player's skill allocation. Read errors are warnings: an unreadable list is no
    /// allocation, an unreadable entry is skipped.
    fn skills(&mut self, player_addr: u64, class: u32) -> Skills {
        let (header, first) = match self.skill_list(player_addr) {
            Ok(Some(list)) => list,
            Ok(None) => return Skills::default(),
            Err(e) => {
                self.warn(format!("skill list: {e:#}"));
                return Skills::default();
            }
        };
        Skills {
            points: self.class_skill_points(first, class),
            left: self.skill_button(header, skills::LEFT),
            right: self.skill_button(header, skills::RIGHT),
        }
    }

    /// The skill list header of the player and its first node; `None` without a list.
    fn skill_list(&self, player_addr: u64) -> Result<Option<(u64, Option<u64>)>> {
        let Some(header) = self.ptr(player_addr, unit::SKILLS)? else {
            return Ok(None);
        };
        Ok(Some((header, self.ptr(header, skills::FIRST)?)))
    }

    /// `(skills.txt id, hard points)` of the allocated skills of `class` in the list from
    /// `first`; basic skills and other classes' gear-granted skills are left out.
    fn class_skill_points(&mut self, first: Option<u64>, class: u32) -> Vec<(u16, u8)> {
        let mut points = Vec::new();
        for node in self.walk(first, skill::NEXT, MAX_SKILLS, "skill") {
            match self.skill_points(node) {
                Ok((id, hard)) => {
                    let own = skills_data::skill(id).is_some_and(|s| u32::from(s.class) == class);
                    if hard > 0 && own {
                        points.push((id, hard));
                    }
                }
                Err(e) => self.warn(format!("skill {node:#x}: {e:#}")),
            }
        }
        points
    }

    /// The skill on the mouse button at `off` of the list `header`; `None` if unreadable.
    fn skill_button(&self, header: u64, off: u64) -> Option<u16> {
        let node = self.ptr(header, off).ok().flatten()?;
        self.skill_id(node).ok()
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
            seen_loaded: false,
            preset: self.room_preset(room),
        })
    }

    /// The room's level-preset id; unreadable is `None`, silently (it would repeat every room).
    fn room_preset(&self, room: u64) -> Option<u32> {
        let record = self.ptr(room, room2::LEVEL_PRESET).ok().flatten()?;
        self.u32(record, level_preset::ID).ok()
    }

    /// The game's difficulty via act → `ActMisc`.
    fn difficulty(&self, act_addr: u64) -> Option<u8> {
        let misc = self.ptr(act_addr, act::MISC).ok().flatten()?;
        difficulty_from_raw(self.u16(misc, act_misc::DIFFICULTY).ok()?)
    }

    /// The real Tal Rasha's Tomb level via act → `ActMisc` (0 outside Act 2: `None`).
    fn staff_tomb(&self, act_addr: u64) -> Option<u32> {
        let misc = self.ptr(act_addr, act::MISC).ok().flatten()?;
        let level = self.u32(misc, act_misc::STAFF_TOMB_LEVEL).ok()?;
        d2r_core::tombs::TOMB_LEVELS.contains(&level).then_some(level)
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

    /// Records the link to every near room of `room` in another level than `level_addr`,
    /// skipping links already in `out` (a level has a few hundred at most).
    fn border_links(&mut self, room: u64, level_addr: u64, from: &Room, out: &mut Vec<BorderLink>) {
        let from_room = RoomRect {
            x: from.x,
            y: from.y,
            w: from.w,
            h: from.h,
        };
        for near in self.near_rooms(room) {
            match self.border_link(near, level_addr, from_room) {
                Ok(Some(link)) if !out.contains(&link) => out.push(link),
                Ok(_) => {}
                Err(e) => self.warn(format!("near room {near:#x}: {e:#}")),
            }
        }
    }

    /// The Room2 pointers in `room`'s near-room array (`room2::NEAR`), nulls and garbage
    /// skipped; unreadable arrays warn, a null array is none.
    fn near_rooms(&mut self, room: u64) -> Vec<u64> {
        let array = match self.ptr(room, room2::NEAR) {
            Ok(Some(a)) => a,
            Ok(None) => return Vec::new(),
            Err(e) => {
                self.warn(format!("room {room:#x} near rooms: {e:#}"));
                return Vec::new();
            }
        };
        let count = match self.u32(room, room2::NEAR_COUNT) {
            Ok(n) => n,
            Err(e) => {
                self.warn(format!("room {room:#x} near count: {e:#}"));
                return Vec::new();
            }
        };
        if count > MAX_NEAR_ROOMS {
            self.warn(format!(
                "room {room:#x}: {count} near rooms, reading the first {MAX_NEAR_ROOMS}"
            ));
        }
        let len = usize::try_from(count.min(MAX_NEAR_ROOMS)).unwrap_or(0) * 8;
        match self.bytes(array, 0, len) {
            Ok(bytes) => le_user_ptrs(&bytes),
            Err(e) => {
                self.warn(format!("room {room:#x} near rooms: {e:#}"));
                Vec::new()
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

    /// The loaded monster units; unreadable ones warn.
    fn monsters(&mut self, unit_table: u64) -> Vec<Monster> {
        let mut monsters = Vec::new();
        for addr in self.units(unit_table, unit_table::MONSTER) {
            match self.monster(addr) {
                Ok(m) => monsters.push(m),
                Err(e) => self.warn(format!("monster {addr:#x}: {e:#}")),
            }
        }
        monsters
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

    /// True if the unit at `addr` has `state` set in its stat list's state bitfield; unreadable
    /// is false, silently (it would repeat for every monster).
    fn has_state(&self, addr: u64, state: u32) -> bool {
        let Ok(Some(list)) = self.ptr(addr, unit::STATS) else {
            return false;
        };
        self.bytes(list, stat_list::STATE_FLAGS, stat_list::STATE_FLAG_DWORDS * 4)
            .is_ok_and(|flags| has_state(&flags, state))
    }

    /// The aggregated stat array of the list at `list` (base, affixes, runeword and socketed
    /// items, modifiers applied); `None` unless the list has [`stat_list::FLAG_FULL_STATS`].
    fn aggregated_entries(&self, list: u64) -> Option<Result<Vec<(u16, u16, u32)>>> {
        let flags = self.u32(list, stat_list::FLAGS).unwrap_or(0);
        (flags & stat_list::FLAG_FULL_STATS != 0)
            .then(|| self.stat_array(list, stat_list::FULL_ENTRIES, stat_list::FULL_COUNT))
    }

    /// True if the stat list at `list` belongs to the item unit with id `id`.
    fn is_item_list_of(&self, list: u64, id: u32) -> bool {
        matches!(
            (self.u32(list, stat_list::OWNER_TYPE), self.u32(list, stat_list::OWNER_ID)),
            (Ok(ty), Ok(owner)) if ty == unit_table::ITEM && owner == id
        )
    }

    /// The item `id`'s own stat lists: `list` (the base list), then [`stat_list::NEXT`] while
    /// the next one is still the item's (the chain then runs into other owners' lists).
    fn own_item_lists(&self, list: u64, id: u32) -> Vec<u64> {
        let mut out = Vec::new();
        let mut cur = list;
        while out.len() < MAX_ITEM_STAT_LISTS && self.is_item_list_of(cur, id) {
            out.push(cur);
            match self.u64(cur, stat_list::NEXT) {
                Ok(next) if next != 0 => cur = next,
                _ => break,
            }
        }
        out
    }

    /// The entries of item `id`'s own lists from `list` on; unreadable lists add none.
    fn own_item_chain_entries(&self, list: u64, id: u32) -> Vec<(u16, u16, u32)> {
        self.own_item_lists(list, id)
            .into_iter()
            .flat_map(|l| self.stat_entries(l).unwrap_or_default())
            .collect()
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

    /// One walk over all item units (ground, stored, equipped, cursor, belt) into an
    /// [`ItemScan`]. Only owners among the player `candidates` (unit ids) are collected.
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
            let page = self.stored_page(&head);
            let owner = self.storage_owner(&head, page);
            scan.vote(page, owner, candidates);
            if let Some(owner) = owner {
                self.collect_stored(&mut scan, addr, &head, page, owner, items_table);
            }
            self.collect_ground_or_inventory(&mut scan, addr, &head, page, owner, items_table);
            self.collect_gear_stats(&mut scan, addr, &head, page, items_table, candidates);
            self.collect_owned_names(&mut scan, addr, &head, items_table, candidates);
            self.collect_equipped(&mut scan, addr, &head, items_table, candidates);
        }
        scan
    }

    /// The storage page of a stored item; `None` for any other item mode.
    fn stored_page(&self, head: &UnitHead) -> Option<u8> {
        if head.mode == item_mode::STORED {
            self.page(head)
        } else {
            None
        }
    }

    /// The owner of an item on the inventory, cube or stash `page`; an unreadable owner is
    /// `None` (no vote, not spare).
    fn storage_owner(&self, head: &UnitHead, page: Option<u8>) -> Option<u32> {
        match page {
            Some(item_data::PAGE_INVENTORY | item_data::PAGE_CUBE | item_data::PAGE_STASH) => {
                head.unit_data.and_then(|d| self.u32(d, item_data::OWNER).ok())
            }
            _ => None,
        }
    }

    /// A cube or stash item into `scan.stored` if [`stored_kept`]; only the items the
    /// playstyle's lists can judge are read in full.
    fn collect_stored(
        &mut self,
        scan: &mut ItemScan,
        addr: u64,
        head: &UnitHead,
        page: Option<u8>,
        owner: u32,
        items_table: Option<u64>,
    ) {
        let Some(page @ (item_data::PAGE_CUBE | item_data::PAGE_STASH)) = page else {
            return;
        };
        if !self.is_read_in_full(head, items_table) {
            return;
        }
        match self.owned_item_fields(addr, head, items_table) {
            Ok(item) if stored_kept(&item) => scan.stored.push(Stored { item, page, owner }),
            Ok(_) => {}
            Err(e) => self.warn(format!("item {addr:#x}: {e:#}")),
        }
    }

    /// A ground item into `scan.drops` (if worth showing) and an inventory item into
    /// `scan.inventory`.
    fn collect_ground_or_inventory(
        &mut self,
        scan: &mut ItemScan,
        addr: u64,
        head: &UnitHead,
        page: Option<u8>,
        owner: Option<u32>,
        items_table: Option<u64>,
    ) {
        let read = match head.mode {
            item_mode::ON_GROUND => {
                scan.ground = scan.ground.saturating_add(1);
                self.item_drop(addr, head, items_table).map(|drop| {
                    scan.drops.extend(drop);
                })
            }
            item_mode::STORED if page == Some(item_data::PAGE_INVENTORY) => self
                .owned_item_fields(addr, head, items_table)
                .map(|item| scan.inventory.push((item, owner))),
            _ => Ok(()),
        };
        if let Err(e) = read {
            self.warn(format!("item {addr:#x}: {e:#}"));
        }
    }

    /// The speed/resist stats and weapon WSM of a candidate's equipped item or carried charm.
    fn collect_gear_stats(
        &mut self,
        scan: &mut ItemScan,
        addr: u64,
        head: &UnitHead,
        page: Option<u8>,
        items_table: Option<u64>,
        candidates: &[u32],
    ) {
        let Some(owner) = self
            .gear_owner(head, page, items_table)
            .filter(|o| candidates.contains(o))
        else {
            return;
        };
        let fell_back = self.accumulate_gear(addr, head, scan.gear.entry(owner).or_default());
        if fell_back && !scan.gear_fallback_warned {
            scan.gear_fallback_warned = true;
            self.warn(format!(
                "item {addr:#x}: stat list without the aggregated array, gear stats from \
                 the chain walk (runeword/socket stats missing)"
            ));
        }
        self.capture_weapon_wsm(head, owner, items_table, &mut scan.weapons);
    }

    /// The name of a candidate's identified unique/set or runeword item into its
    /// `owned_names` (a runeword also into its `runeword_copies`). What the mercenary wears
    /// (owner `NO_OWNER`, equipped) goes to `merc_worn` instead, its runewords under `NO_OWNER`.
    fn collect_owned_names(
        &self,
        scan: &mut ItemScan,
        addr: u64,
        head: &UnitHead,
        items_table: Option<u64>,
        candidates: &[u32],
    ) {
        let item_owner = head.unit_data.and_then(|d| self.u32(d, item_data::OWNER).ok());
        let merc_worn = item_owner == Some(item_data::NO_OWNER) && self.equipped_slot(head).is_some();
        let Some(owner) = item_owner.filter(|o| merc_worn || candidates.contains(o)) else {
            return;
        };
        let quality = self.equipped_quality(head);
        if matches!(quality, Some(item_data::QUALITY_UNIQUE | item_data::QUALITY_SET)) {
            match self.unique_or_set_name(addr, head, items_table) {
                Some(name) if merc_worn => scan.merc_worn.push(name),
                Some(name) => push_unique_name(scan.owned_names.entry(owner).or_default(), name),
                None => {}
            }
            return;
        }
        let flags = head.unit_data.and_then(|d| self.u32(d, item_data::FLAGS).ok());
        if let Some(name) = flags.and_then(|f| self.runeword_name(addr, head, f, items_table)) {
            scan.runeword_copies.entry(owner).or_default().push(name.clone());
            if merc_worn {
                scan.merc_worn.push(name);
            } else {
                push_unique_name(scan.owned_names.entry(owner).or_default(), name);
            }
        }
    }

    /// The display name of an identified unique or set item.
    fn unique_or_set_name(&self, addr: u64, head: &UnitHead, items_table: Option<u64>) -> Option<String> {
        let d = self.item_fields(addr, head, items_table).ok()?;
        d2r_core::names::unique_set_name_parts(d.quality, d.flags, d.unique_index).map(str::to_owned)
    }

    /// The equipped item of `head` into its owner's `(body slot, quality)` pairs (the
    /// mercenary's into `merc_equipped`) and, for a candidate, the gear grid's [`Equipped`].
    fn collect_equipped(
        &self,
        scan: &mut ItemScan,
        addr: u64,
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
                items.push(self.equipped_item(addr, head, slot, quality, items_table));
            }
        }
    }

    /// The `item_data::BODY` slot of an equipped item (`None` when not equipped or unreadable).
    fn equipped_slot(&self, head: &UnitHead) -> Option<u8> {
        let data = head.unit_data?;
        let body = self.u8(data, item_data::BODY).ok()?;
        is_worn(body).then_some(body)
    }

    /// The `item_data::QUALITY` of an item (`None` if unreadable).
    fn equipped_quality(&self, head: &UnitHead) -> Option<u32> {
        head.unit_data.and_then(|d| self.u32(d, item_data::QUALITY).ok())
    }

    /// The equipped item of `head` (body slot `slot`, quality `quality`) for the gear grid;
    /// unreadable parts are left out.
    fn equipped_item(
        &self,
        addr: u64,
        head: &UnitHead,
        slot: u8,
        quality: u32,
        items_table: Option<u64>,
    ) -> Equipped {
        let flags = head
            .unit_data
            .and_then(|d| self.u32(d, item_data::FLAGS).ok())
            .unwrap_or(0);
        // `FILE_INDEX` is filled only once identified, so it is kept only with the name resolved
        // from it; row 0 is real (The Gnasher), so no `!= 0` filter.
        let file_index = head
            .unit_data
            .and_then(|d| self.u32(d, item_data::FILE_INDEX).ok());
        let code = items_table
            .and_then(|t| items::read_code(self.mem, t, head.txt))
            .map(items::code_text);
        let name = d2r_core::names::unique_set_name_parts(quality, flags, file_index).map(str::to_owned);
        let named_index = if name.is_some() { file_index } else { None };
        let runeword = flags & item_data::FLAG_RUNEWORD != 0;
        let name = name.or_else(|| self.runeword_name(addr, head, flags, items_table));
        Equipped {
            body: slot,
            quality,
            code,
            name,
            file_index: named_index,
            runeword,
            stats: self.item_stats(addr, head.id),
        }
    }

    /// The open trade window: the first of the monster units at `npcs` whose inventory chain
    /// holds items flagged [`item_data::FLAG_IN_STORE`] (filled only while the window is open).
    fn shop(
        &self,
        mut npcs: impl Iterator<Item = u64>,
        items_table: Option<u64>,
    ) -> Option<d2r_core::shop::Shop> {
        npcs.find_map(|npc| {
            let items = self.store_items(npc, items_table);
            let npc_txt = self.u32(npc, unit::TXT).ok()?;
            (!items.is_empty()).then(|| d2r_core::shop::Shop {
                npc_txt,
                npc: d2r_core::affixes_data::vendor(npc_txt).map(str::to_owned),
                items,
            })
        })
    }

    /// The store items in the inventory chain of the monster unit at `npc`; unreadable links
    /// end the walk, a runaway chain is a misread (empty).
    fn store_items(&self, npc: u64, items_table: Option<u64>) -> Vec<d2r_core::shop::ShopItem> {
        let mut out = Vec::new();
        let mut cur = self.first_inventory_item(npc);
        let mut walked = 0;
        while let Some(item) = cur {
            walked += 1;
            let Ok(Some(data)) = self.ptr(item, unit::UNIT_DATA) else {
                break;
            };
            if walked > MAX_SHOP_ITEMS {
                return Vec::new();
            }
            if let Ok(flags) = self.u32(data, item_data::FLAGS)
                && flags & item_data::FLAG_IN_STORE != 0
                && let Some(entry) = self.store_item(item, data, items_table)
            {
                out.push(entry);
            }
            cur = self.ptr(data, item_data::NEXT_IN_INVENTORY).ok().flatten();
        }
        out
    }

    /// One store item (unit at `item`, `ItemData` at `data`) named by [`d2r_core::shop::shop_item`].
    fn store_item(&self, item: u64, data: u64, items_table: Option<u64>) -> Option<d2r_core::shop::ShopItem> {
        let quality = self.u32(data, item_data::QUALITY).ok()?;
        let page = self.u8(data, item_data::PAGE).ok()?;
        let prefix = self.u16(data, item_data::MAGIC_PREFIX).ok()?;
        let suffix = self.u16(data, item_data::MAGIC_SUFFIX).ok()?;
        let code = items_table
            .zip(self.u32(item, unit::TXT).ok())
            .and_then(|(t, txt)| items::read_code(self.mem, t, txt))
            .map(items::code_text);
        let mut entry = d2r_core::shop::shop_item(code.as_deref(), quality, page, prefix, suffix);
        if let Ok(id) = self.u32(item, unit::ID) {
            entry.stats = self.item_stats(item, id);
        }
        Some(entry)
    }

    /// The stat entries of the item unit at `item` (unit id `id`), for the gear score: its
    /// aggregated array, with the base list's stat ids to pick the base's damage pairs. Without
    /// the array (or a readable base list), the item's own list chain, which misses runeword
    /// and socket stats.
    fn item_stats(&self, item: u64, id: u32) -> Vec<d2r_core::item_stats::ItemStat> {
        let Some(list) = self.ptr(item, unit::STATS).unwrap_or(None) else {
            return Vec::new();
        };
        if self.is_item_list_of(list, id)
            && let Some(Ok(full)) = self.aggregated_entries(list)
            && let Ok(base) = self.stat_entries(list)
        {
            let base_ids: Vec<u16> = base.into_iter().map(|(_, stat, _)| stat).collect();
            return d2r_core::item_stats::ItemStat::from_totals(&full, &base_ids);
        }
        let mut out = Vec::new();
        for (i, list) in self.own_item_lists(list, id).into_iter().enumerate() {
            let Ok(entries) = self.stat_entries(list) else {
                break;
            };
            out.extend(entries.into_iter().map(|(param, stat, raw)| {
                d2r_core::item_stats::ItemStat::from_raw(param, stat, raw, i == 0)
            }));
        }
        out
    }

    /// The runeword name of the item at `addr` (flags `flags`): `None` unless it carries the
    /// [`item_data::FLAG_RUNEWORD`] bit and its socketed runes spell a known runeword. The
    /// base's code picks between runewords sharing their runes ("Hustle (weapon)" / "(armor)").
    fn runeword_name(
        &self,
        addr: u64,
        head: &UnitHead,
        flags: u32,
        items_table: Option<u64>,
    ) -> Option<String> {
        let table = items_table.filter(|_| flags & item_data::FLAG_RUNEWORD != 0)?;
        let runes = self.socketed_codes(addr, table);
        let base = items::read_code(self.mem, table, head.txt).map(items::code_text);
        stash::runeword_by_runes(&runes, base.as_deref()).map(|w| w.name.to_owned())
    }

    /// The item codes of what is socketed in the item at `addr`, in socket order (its own
    /// inventory chain). An unreadable link ends the list early; a runaway chain is empty.
    fn socketed_codes(&self, addr: u64, items_table: u64) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = self.first_inventory_item(addr);
        while let Some(item) = cur {
            if out.len() >= MAX_SOCKETS {
                return Vec::new();
            }
            let Ok(txt) = self.u32(item, unit::TXT) else {
                break;
            };
            let Some(code) = items::read_code(self.mem, items_table, txt) else {
                break;
            };
            out.push(items::code_text(code));
            cur = self.next_in_inventory(item).ok().flatten();
        }
        out
    }

    /// The owner whose gear stats the item contributes to, if it is equipped or a charm in
    /// the inventory.
    fn gear_owner(&self, head: &UnitHead, page: Option<u8>, items_table: Option<u64>) -> Option<u32> {
        let data = head.unit_data?;
        let owner = self.u32(data, item_data::OWNER).ok()?;
        let body = self.u8(data, item_data::BODY).ok()?;
        if is_worn(body) {
            return Some(owner);
        }
        if page == Some(item_data::PAGE_INVENTORY) {
            let code = items::read_code(self.mem, items_table?, head.txt)?;
            if CHARM_CODES.contains(&items::code_text(code).as_str()) {
                return Some(owner);
            }
        }
        None
    }

    /// Records the WSM of `owner`'s equipped weapon (`weapons_data`) in `out`, once per owner.
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
        if self.equipped_slot(head) != Some(WEAPON_BODY_SLOT) || already || !owned_by_player {
            return;
        }
        let Some(code) = items_table.and_then(|t| items::read_code(self.mem, t, head.txt)) else {
            return;
        };
        if let Some(wsm) = d2r_core::weapons_data::wsm(&items::code_text(code)) {
            out.insert(owner, wsm);
        }
    }

    /// Adds the speed/resist stats of `item` to `acc` from its aggregated array (the owner
    /// chain misses runeword and socket stats). Without the array it falls back to the item's
    /// own list chain and returns true, so the caller can warn.
    fn accumulate_gear(&self, item: u64, head: &UnitHead, acc: &mut SpeedStats) -> bool {
        let Some(list) = self.ptr(item, unit::STATS).unwrap_or(None) else {
            return false;
        };
        if !self.is_item_list_of(list, head.id) {
            return false;
        }
        let aggregated = self.aggregated_entries(list);
        let fell_back = aggregated.is_none();
        let entries = aggregated.map_or_else(
            || self.own_item_chain_entries(list, head.id),
            Result::unwrap_or_default,
        );
        add_speed_stats(acc, &entries);
        fell_back
    }

    /// The resists on the player unit's own stat list and on its aggregated array, if any. The
    /// own list holds no item stats, only permanent quest resists (Anya's scrolls); unreadable
    /// is `[0; 4]` / `None`.
    fn own_resists(&self, player: u64) -> ([i32; 4], Option<[i32; 4]>) {
        let Some(list) = self.ptr(player, unit::STATS).unwrap_or(None) else {
            return ([0; 4], None);
        };
        let own = self.stat_entries(list).map_or([0; 4], |e| resists_of(&e));
        let aggregated = self
            .aggregated_entries(list)
            .and_then(Result::ok)
            .map(|e| resists_of(&e));
        (own, aggregated)
    }

    /// All `{layer, stat id, value}` entries of the stat list at `list`; an empty list (count 0,
    /// e.g. a magic ring's base list) is `Ok(vec![])` without touching its entries pointer.
    fn stat_entries(&self, list: u64) -> Result<Vec<(u16, u16, u32)>> {
        self.stat_array(list, stat_list::ENTRIES, stat_list::COUNT)
    }

    /// The `{layer, stat id, value}` array whose pointer sits at `list + entries_at` and whose
    /// u64 count at `list + count_at`.
    fn stat_array(&self, list: u64, entries_at: u64, count_at: u64) -> Result<Vec<(u16, u16, u32)>> {
        let count = self.u64(list, count_at)?;
        if count == 0 {
            return Ok(Vec::new());
        }
        if count > MAX_ITEM_STATS {
            bail!("stat list {list:#x}: {count} entries");
        }
        let entries = self.ptr_req(list, entries_at, "stat entries")?;
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

    /// True if the stored item `head` may pass [`stored_kept`], judged before the full read from
    /// its quality, flags and code; false if unreadable.
    fn is_read_in_full(&self, head: &UnitHead, items_table: Option<u64>) -> bool {
        let white_base = || {
            items_table
                .and_then(|t| items::read_code(self.mem, t, head.txt))
                .is_some_and(|code| d2r_core::socket_bases::socketable(&items::code_text(code)))
        };
        head.unit_data.is_some_and(|d| {
            matches!(
                (self.u32(d, item_data::QUALITY), self.u32(d, item_data::FLAGS)),
                (Ok(q), Ok(f)) if identified_unique_or_set(q, f)
                    || magic_or_better(q)
                    || (maybe_white_base(q, f) && white_base())
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
            sockets: self.sockets(addr).unwrap_or(0),
            unique_index,
            stats: Vec::new(),
            item_data_head: String::new(),
            x: 0,
            y: 0,
        })
    }

    /// [`Reader::item_fields`] of one of the player's own items, with its stats when it is
    /// magic or better ([`magic_or_better`]; the playstyle scores those).
    fn owned_item_fields(&self, addr: u64, head: &UnitHead, items_table: Option<u64>) -> Result<ItemDrop> {
        let mut item = self.item_fields(addr, head, items_table)?;
        if magic_or_better(item.quality) {
            item.stats = self.item_stats(addr, head.id);
        }
        Ok(item)
    }

    /// The ground item `head` (unit at `addr`) as a drop, or `None` for a base without a known
    /// code, which no playstyle rule can judge.
    fn item_drop(&self, addr: u64, head: &UnitHead, items_table: Option<u64>) -> Result<Option<ItemDrop>> {
        let mut drop = self.item_fields(addr, head, items_table)?;
        if drop.kind == DropKind::Base && drop.code.is_none() {
            return Ok(None);
        }
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
        let entries = self.stat_entries(list)?;
        Ok(entries
            .into_iter()
            .find_map(|(_, stat, value)| (stat == id).then_some(value)))
    }
}

/// What [`Reader::object`] found.
enum Object {
    Shrine(Shrine),
    Well(Well),
    Operated(OperatedObject),
}

/// The plausible user pointers among the little-endian u64 words of `bytes`.
fn le_user_ptrs(bytes: &[u8]) -> Vec<u64> {
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|&w| u64::from_le_bytes(w))
        .filter(|&p| d2r_core::reader::is_user_ptr(p))
        .collect()
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
    use d2r_core::game::worn_names;
    use d2r_core::objects_data::{SHRINE_OBJECTS, WELL_OBJECTS};
    use d2r_core::testutil::FakeMem;
    // The reader's output checked against the playstyle's rules (a dev-dependency).
    use d2r_playstyle_config::{marks, stash_plan, valuables};

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
    fn plain_stat_values_are_signed() {
        // Positive cases: plain values as-is, 1/256-encoded ones decoded.
        assert_eq!(plain_stat_from_raw(25), 25);
        assert_eq!(plain_stat_from_raw(255), 255);
        assert_eq!(plain_stat_from_raw(20 << 8), 20, "1/256 units");
        assert_eq!(plain_stat_from_raw(0x1234), 0x1234, "not 8 trailing zeros");
        // Negative values are two's complement and never 1/256-decoded: -256 = 0xFFFF_FF00
        // has 8 trailing zeros but is a plain -256.
        assert_eq!(plain_stat_from_raw((-10i32).cast_unsigned()), -10);
        assert_eq!(plain_stat_from_raw((-256i32).cast_unsigned()), -256);
        assert_eq!(plain_stat_from_raw(u32::MAX), -1);
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
        use d2r_core::reader::ReadMem;
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
        // (Its resists: see `player_own_resists_add_to_gear`.)
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
        // Four items of a shared stash tab (pseudo-owner, no player) and one of Bob's.
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
        // Alice was chosen before; the owner votes say Bob.
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
        let mut session = d2r_core::session::Session::default();
        for (i, m) in [&bob, &alice, &bob, &alice].into_iter().enumerate() {
            let mut s = snapshot(m, TABLE, None, None, session.player_id())?;
            assert_eq!(s.player.addr, Addr(P_BOB), "tick {i}");
            assert!(!session.apply(&mut s).new_game, "tick {i}");
        }
        Ok(())
    }

    #[test]
    fn ground_and_unowned_items_dont_vote() -> Result<()> {
        // Ground items "owned" by Bob and stash items without owner.
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

    /// [`world`] with Alice's unit made stale (null act): Bob is the only player in the game.
    fn solo_bob(m: &mut FakeMem) -> Result<()> {
        unit(m, P_ALICE, 0, 3, PD_ALICE, 0, PATH_ALICE, 0)
    }

    #[test]
    fn merc_worn_uniques_are_listed_apart_from_the_players() -> Result<()> {
        let mut m = world()?;
        solo_bob(&mut m)?;
        let id = item_data::FLAG_IDENTIFIED;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        // The mercenary's identified unique helm (Shako, unique row 248) and an unidentified
        // unique: only the identified one is named.
        stored_item(&mut m, I1, (40, 7, id, 248), (1, 255), &[], I2)?;
        set_owner(&mut m, I1, item_data::NO_OWNER)?;
        set_body(&mut m, I1, 1)?;
        stored_item(&mut m, I2, (40, 7, 0, 248), (1, 255), &[], I3)?;
        set_owner(&mut m, I2, item_data::NO_OWNER)?;
        set_body(&mut m, I2, 4)?;
        // Bob's own worn set piece (Cleglaw's Claw, set row 7).
        stored_item(&mut m, I3, (40, 5, id, 7), (1, 255), &[], 0)?;
        set_owner(&mut m, I3, BOB_ID)?;
        set_body(&mut m, I3, 2)?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!(s.merc_worn, ["Harlequin Crest"]);
        assert_eq!(
            s.owned_names,
            ["Cleglaw's Claw"],
            "the merc's piece isn't Bob's own"
        );
        assert_eq!(s.holdings().count("Harlequin Crest"), 1);
        assert_eq!(s.player_candidates.len(), 1, "solo");
        Ok(())
    }

    #[test]
    fn a_duplicate_unit_of_the_same_player_is_still_solo() {
        let unit = |addr, id, name: &str| Candidate {
            addr: Addr(addr),
            id,
            name: name.to_owned(),
        };
        assert!(is_solo(&[unit(1, 7, "Farmer"), unit(2, 9, "Farmer")]));
        assert!(!is_solo(&[unit(1, 7, "Farmer"), unit(2, 9, "Fulmina")]));
    }

    #[test]
    fn merc_gear_is_ignored_with_other_players_in_the_game() -> Result<()> {
        let mut m = world()?;
        let id = item_data::FLAG_IDENTIFIED;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        // A merc-worn identified Shako and a merc-worn runeword (Leaf over Tir/Ral): with
        // Alice in the game too, either may be her merc's.
        stored_item(&mut m, I1, (40, 7, id, 248), (1, 255), &[], I2)?;
        set_owner(&mut m, I1, item_data::NO_OWNER)?;
        set_body(&mut m, I1, 1)?;
        stored_item(
            &mut m,
            I2,
            (40, 2, id | item_data::FLAG_RUNEWORD, 0),
            (1, 255),
            &[],
            0,
        )?;
        set_owner(&mut m, I2, item_data::NO_OWNER)?;
        set_body(&mut m, I2, 3)?;
        let (rune_tir, rune_ral) = (0x80_7000, 0x80_8000);
        stored_item(&mut m, rune_tir, (41, 2, 0, 0), (6, 255), &[], 0)?;
        stored_item(&mut m, rune_ral, (42, 2, 0, 0), (6, 255), &[], 0)?;
        let sockets = chain_items(&mut m, &[rune_tir, rune_ral])?;
        tab_inventory(&mut m, INV0, item_data::NO_OWNER, sockets)?;
        patch(&mut m, I2, UNIT_SIZE, unit::INVENTORY, &INV0.to_le_bytes())?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!(s.player_candidates.len(), 2, "Alice and Bob");
        assert!(s.merc_worn.is_empty(), "{:?}", s.merc_worn);
        assert!(s.runeword_copies.is_empty(), "{:?}", s.runeword_copies);
        assert_eq!(s.holdings().count("Harlequin Crest"), 0);
        assert_eq!(s.holdings().count("Leaf"), 0);
        assert_eq!(
            s.merc_equipped,
            vec![(1, 7), (3, 2)],
            "the merc slot display is untouched"
        );

        // The same gear with Bob alone in the game counts.
        solo_bob(&mut m)?;
        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!(s.merc_worn, ["Harlequin Crest", "Leaf"]);
        assert_eq!(s.runeword_copies, ["Leaf"]);
        assert_eq!(s.holdings().count("Harlequin Crest"), 1);
        assert_eq!(s.holdings().count("Leaf"), 1);
        Ok(())
    }

    #[test]
    fn equipped_items_are_enriched_with_code_name_and_row() -> Result<()> {
        let mut m = world()?;
        solo_bob(&mut m)?; // the merc's runeword copy counts only when solo
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
        // Bob's equipped weapon: a finished runeword on a Crystal Sword, Tir then Ral in its
        // sockets (Leaf, by runes only), chained through the weapon's own `Inventory`.
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
        let (rune_tir, rune_ral) = (0x80_7000, 0x80_8000);
        stored_item(&mut m, rune_tir, (41, 2, 0, 0), (6, 255), &[], 0)?;
        stored_item(&mut m, rune_ral, (42, 2, 0, 0), (6, 255), &[], 0)?;
        let sockets = chain_items(&mut m, &[rune_tir, rune_ral])?;
        tab_inventory(&mut m, INV0, item_data::NO_OWNER, sockets)?;
        patch(&mut m, I3, UNIT_SIZE, unit::INVENTORY, &INV0.to_le_bytes())?;
        // Bob's equipped body armour: magic, so only the BASE requirement is knowable.
        stored_item(&mut m, I4, (40, 4, id, 0), (1, 255), &[], I5)?;
        set_owner(&mut m, I4, BOB_ID)?;
        set_body(&mut m, I4, 3)?;
        // The mercenary's equipped armour (NO_OWNER, body 3): pairs only, not enriched. A
        // runeword over the same Tir/Ral socket chain, so a second copy of Leaf.
        stored_item(
            &mut m,
            I5,
            (40, 2, id | item_data::FLAG_RUNEWORD, 0),
            (1, 255),
            &[],
            I6,
        )?;
        set_owner(&mut m, I5, item_data::NO_OWNER)?;
        set_body(&mut m, I5, 3)?;
        patch(&mut m, I5, UNIT_SIZE, unit::INVENTORY, &INV0.to_le_bytes())?;
        // The same runeword owned by nobody but in no body slot (ground, shop): no copy.
        stored_item(
            &mut m,
            I6,
            (40, 2, id | item_data::FLAG_RUNEWORD, 0),
            (1, 255),
            &[],
            0,
        )?;
        set_owner(&mut m, I6, item_data::NO_OWNER)?;
        patch(&mut m, I6, UNIT_SIZE, unit::INVENTORY, &INV0.to_le_bytes())?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!(s.equipped, vec![(1, 7), (2, 5), (4, 2), (3, 4)]);
        assert_eq!(s.merc_equipped, vec![(3, 2)], "the merc's piece stays a pair");
        assert_eq!(s.merc_worn, ["Leaf"], "a merc-worn runeword is worn by the merc");
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
        assert_eq!(
            weapon.name.as_deref(),
            Some("Leaf"),
            "named by its socketed runes"
        );
        assert_eq!(weapon.file_index, None);
        assert_eq!(weapon.required_level(), Some(19), "Ral's 19 beats the base's");
        assert!(
            s.owned_names.iter().any(|n| n == "Leaf"),
            "a worn runeword is owned: {:?}",
            s.owned_names
        );
        // Bob's copy and the merc's, each counted; the unworn ownerless one not.
        assert_eq!(s.runeword_copies, ["Leaf", "Leaf"]);
        assert_eq!(
            worn_names(eq),
            ["Harlequin Crest", "Cleglaw's Claw", "Leaf"],
            "and worn"
        );
        let armour = eq.get(3).context("armour")?;
        assert_eq!(armour.name, None, "magic: no name");
        assert_eq!(
            armour.required_level(),
            Some(43),
            "magic: the BASE item's levelreq (affixes can raise the real one)"
        );
        Ok(())
    }

    /// Writes `{layer 0, id, value}` stat entries at `at`.
    fn entries(mem: &mut FakeMem, at: u64, stats: &[(u16, u32)]) {
        let mut b = Vec::new();
        for &(id, value) in stats {
            b.extend(0u16.to_le_bytes());
            b.extend(id.to_le_bytes());
            b.extend(value.to_le_bytes());
        }
        mem.add(at, b);
    }

    /// An item stat list at `at` owned by item `owner`: `flags`, its own entries
    /// (pointer, count), the aggregated array (pointer, count) and the next list.
    fn item_list(
        mem: &mut FakeMem,
        at: u64,
        owner: u32,
        flags: u32,
        own: (u64, u64),
        full: (u64, u64),
        next: u64,
    ) -> Result<()> {
        put(
            mem,
            at,
            0x100,
            &[
                (0x08, &unit_table::ITEM.to_le_bytes()),
                (0x0C, &owner.to_le_bytes()),
                (stat_list::FLAGS, &flags.to_le_bytes()),
                (stat_list::ENTRIES, &own.0.to_le_bytes()),
                (stat_list::COUNT, &own.1.to_le_bytes()),
                (stat_list::NEXT, &next.to_le_bytes()),
                (stat_list::FULL_ENTRIES, &full.0.to_le_bytes()),
                (stat_list::FULL_COUNT, &full.1.to_le_bytes()),
            ],
        )
    }

    #[test]
    fn player_own_resists_add_to_gear() -> Result<()> {
        // Two Anya scrolls (Normal + Nightmare): +20 all on the player's own list, no gear.
        let m = world_with_player_stats(&[
            (stat_list::LEVEL, 68),
            (stat_list::RESIST_FIRE, 20),
            (stat_list::RESIST_LIGHTNING, 20),
            (stat_list::RESIST_COLD, 20),
            (stat_list::RESIST_POISON, 20),
        ])?;
        let s = snapshot(&m, TABLE, None, None, None)?;
        let all = SpeedStats {
            res_fire: 20,
            res_cold: 20,
            res_lightning: 20,
            res_poison: 20,
            ..SpeedStats::default()
        };
        assert_eq!(s.player.speed, Some(all));
        // No resists on the list: nothing added (no gear: no speed stats at all).
        let m = world_with_player_stats(&[(stat_list::LEVEL, 68)])?;
        assert_eq!(snapshot(&m, TABLE, None, None, None)?.player.speed, None);
        // The summing itself: shifted and negative values, other stats ignored.
        let minus_five = (-5i32).cast_unsigned();
        assert_eq!(
            resists_of(&[
                (0, stat_list::RESIST_FIRE, 10 << 8),
                (0, stat_list::RESIST_FIRE, 10),
                (0, stat_list::RESIST_POISON, minus_five),
                (0, stat_list::FCR, 20),
            ]),
            [20, 0, 0, -5]
        );
        Ok(())
    }

    #[test]
    fn gear_stats_read_the_aggregated_array() -> Result<()> {
        // Runeword resists sit only in the aggregated array; the next list is a socketed rune's.
        let mut m = world()?;
        let full = stat_list::FLAG_FULL_STATS;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        // Bob's runeword shield (body 5).
        let rw = item_data::FLAG_RUNEWORD;
        stored_item(&mut m, I1, (40, 2, rw, 0), (1, 255), &[], I2)?;
        set_owner(&mut m, I1, BOB_ID)?;
        set_body(&mut m, I1, 5)?;
        let shield_id = u32::try_from(I1 >> 12)?;
        let (arr, rune_list, rune_arr) = (I1 + 0x900, I1 + 0xA00, I1 + 0xB00);
        item_list(&mut m, I1 + 0x600, shield_id, full, (0, 0), (arr, 6), rune_list)?;
        entries(
            &mut m,
            arr,
            &[
                (31, 25),
                (stat_list::RESIST_FIRE, 48),
                (stat_list::RESIST_LIGHTNING, 48),
                (stat_list::RESIST_COLD, 43),
                (stat_list::RESIST_POISON, 48),
                (stat_list::FCR, 25),
            ],
        );
        item_list(&mut m, rune_list, shield_id + 99, full, (0, 0), (rune_arr, 1), 0)?;
        entries(&mut m, rune_arr, &[(stat_list::RESIST_POISON, 35)]);
        // Bob's ring (body 6) without the flag: the chain walk (empty base list, affix list),
        // stopping at another owner's list.
        let id = item_data::FLAG_IDENTIFIED;
        stored_item(&mut m, I2, (40, 4, id, 0), (1, 255), &[], 0)?;
        set_owner(&mut m, I2, BOB_ID)?;
        set_body(&mut m, I2, 6)?;
        let ring_id = u32::try_from(I2 >> 12)?;
        let (affixes, affix_entries, foreign) = (I2 + 0x900, I2 + 0xA00, I2 + 0xB00);
        item_list(&mut m, I2 + 0x600, ring_id, 0x40, (0, 0), (0, 0), affixes)?;
        item_list(
            &mut m,
            affixes,
            ring_id,
            0x40,
            (affix_entries, 2),
            (0, 0),
            foreign,
        )?;
        // A negative affix (-10 fire resist, stored as its two's-complement word) lowers the sum.
        let minus_ten = (-10i32).cast_unsigned();
        entries(
            &mut m,
            affix_entries,
            &[(stat_list::FHR, 10), (stat_list::RESIST_FIRE, minus_ten)],
        );
        item_list(&mut m, foreign, ring_id + 1, 0x40, (affix_entries, 1), (0, 0), 0)?;

        let s = snapshot(&m, TABLE, None, Some("Bob"), None)?;
        let expected = SpeedStats {
            fcr: 25,
            fhr: 10,
            res_fire: 38,
            res_cold: 43,
            res_lightning: 48,
            res_poison: 48,
            ..SpeedStats::default()
        };
        assert_eq!(s.player.speed, Some(expected));
        let fallbacks = s.warnings.iter().filter(|w| w.contains("chain walk")).count();
        assert_eq!(fallbacks, 1, "{:?}", s.warnings);
        // The vendor comparison sees the same full stats: the shield's runeword resists and
        // its enhanced defense as the base value (no ED left to apply), not the rune's list.
        let stats = |body: u8| {
            s.equipped_items
                .iter()
                .find(|e| e.body == body)
                .map_or_default(|e| e.stats.clone())
        };
        let stat = |id: u16, value: i32, base: bool| d2r_core::item_stats::ItemStat {
            id,
            param: 0,
            value,
            base,
        };
        assert_eq!(
            stats(5),
            [
                stat(31, 25, true),
                stat(stat_list::RESIST_FIRE, 48, false),
                stat(stat_list::RESIST_LIGHTNING, 48, false),
                stat(stat_list::RESIST_COLD, 43, false),
                stat(stat_list::RESIST_POISON, 48, false),
                stat(stat_list::FCR, 25, false),
            ]
        );
        // The ring without the flag: the chain walk's affix list.
        assert_eq!(
            stats(6),
            [
                stat(stat_list::FHR, 10, false),
                stat(stat_list::RESIST_FIRE, -10, false),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_carried_charm_counts_once() -> Result<()> {
        // A cm1 in Bob's inventory (page 0, body 0): its poison res is in the base list, the
        // affix list and the aggregated array; only the array counts.
        let mut m = world()?;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        let id = item_data::FLAG_IDENTIFIED;
        let page = item_data::PAGE_INVENTORY;
        stored_item(&mut m, I1, (43, 4, id, 0), (item_mode::STORED, page), &[], 0)?;
        set_owner(&mut m, I1, BOB_ID)?;
        let charm_id = u32::try_from(I1 >> 12)?;
        let (poison, affixes, arr) = (I1 + 0x900, I1 + 0xA00, I1 + 0xB00);
        let full = stat_list::FLAG_FULL_STATS;
        item_list(&mut m, I1 + 0x600, charm_id, full, (poison, 1), (arr, 1), affixes)?;
        item_list(&mut m, affixes, charm_id, 0x40, (poison, 1), (0, 0), 0)?;
        entries(&mut m, poison, &[(stat_list::RESIST_POISON, 11)]);
        entries(&mut m, arr, &[(stat_list::RESIST_POISON, 11)]);
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        let speed = s.player.speed.context("Bob's gear stats")?;
        assert_eq!(speed.res_poison, 11);
        assert!(
            s.warnings.iter().all(|w| !w.contains("chain walk")),
            "{:?}",
            s.warnings
        );
        Ok(())
    }

    #[test]
    fn item_stats_walk_past_an_empty_base_list() -> Result<()> {
        // An empty base list, the affixes (+5 fire res) in the next, then another owner's list.
        let mut m = world()?;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        stored_item(
            &mut m,
            I1,
            (40, 4, item_data::FLAG_IDENTIFIED, 0),
            (1, 255),
            &[],
            0,
        )?;
        set_owner(&mut m, I1, BOB_ID)?;
        set_body(&mut m, I1, 6)?;
        let ring_id = u32::try_from(I1 >> 12)?;
        let (base_list, affixes, entries, foreign) = (I1 + 0x600, I1 + 0x800, I1 + 0x900, I1 + 0xA00);
        let list = |mem: &mut FakeMem, at: u64, owner: u32, entries: u64, count: u64, next: u64| {
            put(
                mem,
                at,
                0xE0,
                &[
                    (0x08, &unit_table::ITEM.to_le_bytes()),
                    (0x0C, &owner.to_le_bytes()),
                    (stat_list::ENTRIES, &entries.to_le_bytes()),
                    (stat_list::COUNT, &count.to_le_bytes()),
                    (stat_list::NEXT, &next.to_le_bytes()),
                ],
            )
        };
        // The empty base list: count 0 and a null entries pointer.
        list(&mut m, base_list, ring_id, 0, 0, affixes)?;
        list(&mut m, affixes, ring_id, entries, 1, foreign)?;
        let mut b = Vec::new();
        b.extend(0u16.to_le_bytes());
        b.extend(stat_list::RESIST_FIRE.to_le_bytes());
        b.extend(5u32.to_le_bytes());
        m.add(entries, b);
        list(&mut m, foreign, ring_id + 1, entries, 1, 0)?;

        let s = snapshot(&m, TABLE, None, Some("Bob"), None)?;
        let ring = s.equipped_items.first().context("ring")?;
        assert_eq!(
            ring.stats,
            [d2r_core::item_stats::ItemStat {
                id: stat_list::RESIST_FIRE,
                param: 0,
                value: 5,
                base: false,
            }],
            "the affix list after the empty base list, not the foreign list"
        );
        Ok(())
    }

    #[test]
    fn a_stashed_runeword_is_owned_but_not_worn() -> Result<()> {
        // Bob's private stash (page 4) holds a finished Leaf (Tir, Ral) on a Crystal Sword.
        let mut m = world()?;
        let id = item_data::FLAG_IDENTIFIED;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        stored_item(
            &mut m,
            I1,
            (30, 2, id | item_data::FLAG_RUNEWORD, 0),
            (item_mode::STORED, 4),
            &[],
            0,
        )?;
        set_owner(&mut m, I1, BOB_ID)?;
        let (rune_tir, rune_ral) = (0x80_7000, 0x80_8000);
        stored_item(&mut m, rune_tir, (41, 2, 0, 0), (6, 255), &[], 0)?;
        stored_item(&mut m, rune_ral, (42, 2, 0, 0), (6, 255), &[], 0)?;
        let sockets = chain_items(&mut m, &[rune_tir, rune_ral])?;
        tab_inventory(&mut m, INV0, item_data::NO_OWNER, sockets)?;
        patch(&mut m, I1, UNIT_SIZE, unit::INVENTORY, &INV0.to_le_bytes())?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!(s.owned_names, vec!["Leaf"]);
        assert!(s.equipped_items.is_empty(), "stashed, not worn");
        assert_eq!(worn_names(&s.equipped_items), Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn only_the_chosen_players_gear_counts_in_a_group_game() -> Result<()> {
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
        // R2 belongs to another level: the walk keeps R0, R1 and stops with a warning.
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
                    seen_loaded: false,
                    preset: None
                },
                Room {
                    x: 90,
                    y: 100,
                    w: 40,
                    h: 40,
                    loaded: false,
                    seen_loaded: false,
                    preset: None
                },
                Room {
                    x: 50,
                    y: 130,
                    w: 20,
                    h: 20,
                    loaded: false,
                    seen_loaded: false,
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
        // Empty border links stay out of the JSON; filled ones round-trip.
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
        // `None` is left out of the JSON.
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
        // State 189: dword 5, mask 0x2000_0000.
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
        // A dynamic path's u16 x/y offsets hold something else on a static path.
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
        let stride = usize::try_from(d2r_core::offsets::items_txt::STRIDE)?;
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
            stats: Vec::new(),
            item_data_head: String::new(),
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
                // The list's owner: an item unit with this unit's id ([`Reader::item_stats`]).
                (0x08, &unit_table::ITEM.to_le_bytes()),
                (0x0C, &(u32::try_from(addr >> 12)?).to_le_bytes()),
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

    /// Items table with `crs` (Crystal Sword) at row 30, `uap` (Shako) at row 40, the runes
    /// `r03`/`r08` at 41/42 and `cm1` (small charm) at 43.
    fn base_items_table(mem: &mut FakeMem) -> Result<()> {
        let stride = usize::try_from(d2r_core::offsets::items_txt::STRIDE)?;
        let mut b = vec![0u8; 50 * stride];
        let rows = [
            (30, b"crs "),
            (40, b"uap "),
            (41, b"r03 "),
            (42, b"r08 "),
            (43, b"cm1 "),
        ];
        for (row, code) in rows {
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
        // White 2os Crystal Sword: no rule at any tier; kept by the reader (it has a code), removed
        // by any config.
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
                (id(I2), DropKind::Base, Some("crs"), 2, false, 0, None),
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
        assert_eq!(s.owned_items, vec![], "nothing in the inventory");
        assert!(head.starts_with("02000000"), "quality first: {head}");

        let wants = stash_plan::Wants::default();
        let marks = marks::apply(
            &mut s.drops,
            &d2r_playstyle_config::config::Valuables::default(),
            &wants,
        );
        let tiers: Vec<_> = s
            .drops
            .iter()
            .map(|d| marks.get(&d.id).map(|m| (m.trade, m.uncertain)))
            .collect();
        assert_eq!(
            tiers,
            vec![Some((
                Some((valuables::Tier::S, valuables::Category::Trade)),
                false
            ))],
            "identified Shako S (certain); the swords are gone (the 4os one is a twink A base \
             no need wants: no trade good)"
        );
        Ok(())
    }

    #[test]
    fn item_plan_reads_the_players_own_items() -> Result<()> {
        const I7: u64 = 0x80_7000;
        const I8: u64 = 0x80_8000;
        let mut m = world()?;
        let id = item_data::FLAG_IDENTIFIED;
        let stored = item_mode::STORED;
        unit(&mut m, I0, 4, 600, ID0, ACT, 0, I1)?;
        // Identified Shako (Harlequin Crest) in the inventory: kept, certain, by name.
        stored_item(&mut m, I1, (40, 7, id, 248), (stored, 0), &[], I2)?;
        // The same in the stash (page 4), and equipped (mode 1, page 255): not listed.
        stored_item(&mut m, I2, (40, 7, id, 248), (stored, 4), &[], I3)?;
        stored_item(&mut m, I3, (40, 7, id, 248), (1, 255), &[], I4)?;
        // A white 4os Crystal Sword in the inventory: A twink, sold without a Spirit need.
        stored_item(&mut m, I4, (30, 2, id, 0), (stored, 0), &[(194, 4)], I5)?;
        // A 3os one (B only): not kept.
        stored_item(&mut m, I5, (30, 2, id, 0), (stored, 0), &[(194, 3)], I6)?;
        // Another player's Shako in an inventory (group game): not kept.
        stored_item(&mut m, I6, (40, 7, id, 248), (stored, 0), &[], I7)?;
        // Stored white bases and magic items are read in full too (the magic one with its
        // stats): a 4os Crystal Sword in the stash, a magic one with 20 fire res in the cube.
        stored_item(&mut m, I7, (30, 2, id, 0), (stored, 4), &[(194, 4)], I8)?;
        stored_item(&mut m, I8, (30, 4, id, 0), (stored, 3), &[(39, 20)], 0)?;
        for addr in [I1, I2, I4, I5, I7, I8] {
            set_owner(&mut m, addr, BOB_ID)?;
        }
        set_owner(&mut m, I6, ALICE_ID)?;
        base_items_table(&mut m)?;

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!((s.ground_items, s.drops.len()), (0, 0));
        assert_eq!(s.inventory_items, 4, "page-0 stored items, any owner");
        assert_eq!(
            item_plan(&s),
            vec![
                "Crystal Sword 3os (inv)",
                "Crystal Sword 4os (inv)",
                "Crystal Sword 4os (stash)",
                "Magic Crystal Sword (cube → 1–2)",
                "Harlequin Crest (inv → 3–4)",
                "Harlequin Crest (stash → 3–4)",
            ],
            "the swords (B, A twink) are sold with no base need; the equipped and Alice's Shakos \
             aren't Bob's; the magic sword is kept, but not in the cube"
        );
        Ok(())
    }

    /// The playstyle's item plan of `s` ([`stash_plan`]) without build needs, as text.
    fn item_plan(s: &Snapshot) -> Vec<String> {
        use stash_plan::{Needs, Scoring, held, plan};
        plan(&held(s), &Needs::default(), &Scoring::default())
            .0
            .iter()
            .map(d2r_core::info::advice_entry)
            .collect()
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
    /// owner, `TAB1` names Alice (stale). The unit table order differs from Bob's tab list (the
    /// UI order). No tab has items yet ([`tab_inventory`]).
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
        use d2r_core::stash::{StashItem, StashTab, TabKind};
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
        let stride = usize::try_from(d2r_core::offsets::items_txt::STRIDE)?;
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
            sockets: 0,
            unique_index: None,
            stats: Vec::new(),
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
                        StashItem {
                            unique_index: Some(248),
                            ..item("Harlequin Crest", "uap", 7, true, None)
                        },
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
        use d2r_core::stash::{StashTab, TabKind};
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
        use d2r_core::stash::TabKind;
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
        // The header one qword later: `+0x60` is null.
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
        use d2r_core::stash::{StashTab, TabKind};
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
        use super::Place;
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
        let stride = usize::try_from(d2r_core::offsets::items_txt::STRIDE)?;
        let mut b = vec![0u8; 50 * stride];
        for (row, code) in [(30, b"crs "), (41, b"tgl "), (42, b"hdm ")] {
            b.get_mut(row * stride..row * stride + 4)
                .context("row in table")?
                .copy_from_slice(code);
        }
        m.add(ITEMS, b);

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        // Page-4 items of other owners (stale tab units) are left out.
        assert_eq!(
            item_plan(&s),
            vec!["Gravenspine (inv)", "Gravenspine (cube)", "Magefist (stash)"]
        );
        assert_eq!(
            s.inventory_items, 2,
            "the keep scan still sees every inventory item"
        );
        let places: Vec<_> = s.owned_items.iter().map(|(_, p)| *p).collect();
        assert_eq!(
            places,
            vec![Place::Inventory, Place::Cube, Place::Stash, Place::Stash],
            "Bob's own items only (no tab unit, so no shared ones): inventory first, then cube and \
             stash in unit order"
        );
        // An empty list stays out of the JSON.
        let mut quiet = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        quiet.owned_items.clear();
        let json = serde_json::to_string(&quiet)?;
        assert!(!json.contains("owned_items"), "{json}");
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
        let stride = usize::try_from(d2r_core::offsets::items_txt::STRIDE)?;
        let mut b = vec![0u8; 50 * stride];
        b.get_mut(43 * stride..43 * stride + 4)
            .context("row in table")?
            .copy_from_slice(b"ghm ");
        m.add(ITEMS, b);

        let s = snapshot(&m, TABLE, Some(ITEMS), Some("Bob"), None)?;
        assert_eq!(
            item_plan(&s),
            vec!["Sigon's Visor (stash → 3–4)", "Sigon's Visor (inv)"],
            "the stashed copy is the one kept; the inventory one is a give-away"
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

    /// Store items of the open trade window ([`Reader::shop`]): hung off a monster's
    /// inventory chain, flagged [`item_data::FLAG_IN_STORE`], read in towns only.
    #[test]
    fn the_trade_window_lists_the_vendors_store_items_in_town() -> Result<()> {
        const S0: u64 = 0xB0_0000;
        const S1: u64 = 0xB0_1000;
        const S2: u64 = 0xB0_2000;
        const NPC_INV: u64 = 0xB8_0000;
        let store = item_data::FLAG_IN_STORE;
        let mut m = world()?;
        base_items_table(&mut m)?;
        // A magic Crystal Sword of Teleportation (weapons tab), the NPC's own gear (not for
        // sale), a white Shako (armor tab).
        stored_item(&mut m, S0, (30, 4, store, 0), (0, 1), &[], 0)?;
        patch(
            &mut m,
            S0 + 0x200,
            ITEM_DATA_SIZE,
            item_data::MAGIC_SUFFIX,
            &532u16.to_le_bytes(),
        )?;
        stored_item(&mut m, S1, (30, 2, 0, 0), (0, 255), &[], 0)?;
        stored_item(&mut m, S2, (40, 2, store, 0), (0, 0), &[], 0)?;
        let first = chain_items(&mut m, &[S0, S1, S2])?;
        tab_inventory(&mut m, NPC_INV, 0, first)?;
        // M1 (monstats 41 here) carries the store; M0 has no inventory.
        patch(&mut m, M1, UNIT_SIZE, unit::INVENTORY, &NPC_INV.to_le_bytes())?;

        let s = snapshot(&m, TABLE, Some(ITEMS), None, None)?;
        assert_eq!(s.shop, None, "level 21 is no town: the inventories aren't walked");

        level_struct(&mut m, LEVEL, 40, R0)?; // Lut Gholein
        let s = snapshot(&m, TABLE, Some(ITEMS), None, None)?;
        let shop = s.shop.context("a shop in town")?;
        assert_eq!((shop.npc_txt, shop.npc), (41, None), "not a vendor row");
        let names: Vec<_> = shop
            .items
            .iter()
            .map(|i| (i.name.as_str(), i.page, i.req_level))
            .collect();
        assert_eq!(
            names,
            [
                ("Crystal Sword of Teleportation", 1, Some(24)),
                ("Shako", 0, Some(43))
            ]
        );
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
                drop_of(I2, Some("axe"), DropKind::Base, (10, 11)),
                drop_of(I3, Some("axe"), DropKind::Unique, (12, 13)),
                drop_of(I4, Some("r01"), DropKind::Rune(1), (14, 15)),
                drop_of(I6, Some("axe"), DropKind::Set, (18, 19)),
            ],
            "static-path positions; the normal axe is kept for the playstyle to judge, the \
             inventory rune is left out"
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
        // O0 used health shrine (236) → O1 skill shrine → O2 used well → O3 chest →
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
        // Empty lists stay out of the JSON.
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
            d2r_core::config::SHRINE_NAMES.len(),
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
    fn staff_tomb_via_act_misc() -> Result<()> {
        const ACT_MISC: u64 = 0x23_0000;
        let mut m = world()?;
        assert_eq!(snapshot(&m, TABLE, None, None, None)?.staff_tomb, None);
        put(&mut m, ACT, 0x80, &[(act::MISC, &ACT_MISC.to_le_bytes())])?;
        // Only tomb levels count: 0 (outside Act 2) and other ids are `None`.
        for (raw, want) in [
            (72u32, Some(72)),
            (66, Some(66)),
            (0, None),
            (65, None),
            (73, None),
        ] {
            put(
                &mut m,
                ACT_MISC,
                0x840,
                &[(act_misc::STAFF_TOMB_LEVEL, &raw.to_le_bytes())],
            )?;
            let s = snapshot(&m, TABLE, None, None, None)?;
            assert_eq!(s.staff_tomb, want, "raw {raw}");
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
        assert_eq!(v.pointer("/drops/1/kind"), Some(&serde_json::json!("base")));
        assert_eq!(v.pointer("/drops/2/kind"), Some(&serde_json::json!("unique")));
        // The ItemData hex is journal-only.
        assert!(v.pointer("/drops/0/item_data_head").is_none());
        // A dump's legacy `highlight` field is ignored on load.
        let mut old = v.clone();
        if let Some(d) = old
            .pointer_mut("/drops/0")
            .and_then(serde_json::Value::as_object_mut)
        {
            d.insert(
                "highlight".into(),
                serde_json::json!({"tier": "A", "category": "twink", "rule": "r", "uncertain": true}),
            );
        }
        let back: Snapshot = serde_json::from_value(old)?;
        assert_eq!(back.drops, s.drops);
        assert_eq!(v.pointer("/drops/3/kind"), Some(&serde_json::json!({"rune": 1})));
        Ok(())
    }
}
