//! State accumulated over snapshots of one game: what was seen in each level, superuniques
//! killed, seals operated and where the player arrived in each level.
//!
//! **Memory**: every preset (raw, before filtering), shrine, well, room link and border link
//! seen is remembered per level for the game and merged back into later snapshots of that level, so the map keeps what
//! was explored even after D2R unloads it. The filters below then run over live and remembered
//! presets alike. A shrine keeps its latest `used`; a border link's `entry` is recomputed per
//! snapshot. Rooms of landmark candidate pieces ([`crate::render::candidate_room`]) seen loaded
//! are remembered too and marked [`Room::seen_loaded`](crate::game::Room::seen_loaded), so a
//! clearing explored without the Gidbinn altar stays ruled out once it unloads. Monsters, items
//! and the player are live only;
//! a level seen terrorized (Terror Zone) stays so for the current visit (zones rotate hourly).
//! [`GameState`] is that per-game part; `game_file` keeps it on disk across overlay restarts.
//!
//! **Superunique presets**: a monster preset is drawn iff its txt id is in
//! [`SUPERUNIQUES`](crate::levels_data::SUPERUNIQUES) (751 + the superuniques.txt row, see
//! `research/gen_levels.py`) and it wasn't killed in this game. Other monster presets are
//! ordinary fixed spawns. Older versions learned the ids in play and kept them in a
//! `superuniques.txt` next to the offset cache; that file is no longer read and can be deleted.
//! Quest monster presets ([`QUEST_MONSTERS`](crate::render::QUEST_MONSTERS), the prison doors
//! of Frigid Highlands) are kept too, until a dead monster with the same txt id is seen
//! within [`QUEST_KILL_RADIUS`] of one; that preset then counts as killed for the game.
//!
//! Pure: no I/O. [`Session::apply`] folds each snapshot in and rewrites it for drawing, so the
//! renderer stays a function of the snapshot alone.
//!
//! **Game change**: the per-game part is cleared when the player's unit id changes. The choice
//! is sticky ([`Session::player_id`] feeds [`ChosenBy::Previous`](crate::game::ChosenBy::Previous)),
//! so in a group game the id only changes once the chosen player is no longer a candidate (or
//! `--player` names someone else), not when the item owner vote flips. Online the
//! server hands out a random-looking 32-bit player unit id per game, so a repeat is practically
//! impossible; offline games may reuse small ids, which then carries the old game's state over
//! (accepted: it only keeps markers hidden or remembered that a fresh look would redraw). The
//! act pointer is not part of the key on purpose: it changes on act travel, and kills, seals,
//! entries and explored levels must survive moving between acts within a game.
//!
//! **Entry**: the player's position when the level id changes is the level's arrival point (first
//! visit per game wins; a new game counts as a level change). For [`ARRIVAL_SETTLE`] snapshots it
//! follows the player while no tile is near it, so a stale first frame can't lock a wrong point.
//! The tile preset nearest to it, within [`ENTRY_RADIUS`], is the way in and gets
//! [`Preset::entry`](crate::game::Preset::entry); the other tiles stay exits. Arriving by waypoint or portal has no tile nearby, so no entry. The
//! very first snapshot of a session has no previous level (the overlay may start mid-level),
//! so it records no arrival. A walk-through border ([`Snapshot::border_links`]) to the level the
//! player came from is the entry if the arrival point lies within [`ENTRY_RADIUS`] of its rooms.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::cows;
use crate::game::{BorderLink, Monster, Preset, PresetKind, RoomLink, RoomRect, Shrine, Snapshot, Well};
use crate::levels_data::SUPERUNIQUES;
use crate::objects::is_consumable_quest_object;
use crate::offsets::monster_data;
use crate::render::{candidate_room, quest_monster};
use crate::stash::{self, StashTab};

/// Most distance in tiles between a dead superunique and the superunique preset it is credited
/// to. Generous: it only picks which marker to remove (the nearest), and superunique presets of
/// one level lie far apart.
const KILL_RADIUS: f64 = 30.0;
/// Most distance in tiles between a dead quest monster (live position) and the quest monster
/// preset of the same txt id it is credited to.
const QUEST_KILL_RADIUS: f64 = 10.0;
/// Most distance in tiles between a living superunique and the superunique preset its unit id is
/// bound to: it spawns on its preset, so a small radius suffices and keeps a passing
/// superunique from binding to another's preset.
const BIND_RADIUS: f64 = 15.0;
/// Most distance in tiles between an operated consumable quest object (live position) and the
/// object preset it hides: preset and live positions may differ by a tile or two.
const OPERATED_RADIUS: f64 = 5.0;
/// Most distance in tiles between the arrival point and the entry tile preset.
const ENTRY_RADIUS: f64 = 25.0;
/// Tile presets this close to the entry tile are part of the same entry (multi-tile stairs).
const ENTRY_CLUSTER: f64 = 5.0;

/// Least distance in tiles between two recorded trail points.
const TRAIL_STEP: f64 = 4.0;
/// Most trail points kept (about 20 000 tiles walked).
const TRAIL_CAP: usize = 5000;

/// (level id, x, y) in world tiles of a preset.
type LevelPos = (u32, u32, u32);

/// What [`Session::apply`] found besides rewriting the snapshot.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Update {
    /// The game changed since the previous snapshot; the per-game state was cleared.
    pub new_game: bool,
    /// The arrival point settled in this snapshot: (level id, x, y). Once per level per game.
    pub arrival: Option<(u32, u16, u16)>,
    /// First snapshot in the Moo Moo Farm in this game: a new cow run ([`crate::cows`]).
    pub cow_run: bool,
    /// Cows newly counted dead in this snapshot.
    pub new_cows: u32,
    /// The shared stash was captured for the first time this game: (tab count, item count).
    /// D2R fills the stash only once the stash was opened in the game. Fires only for a
    /// capture with at least one item, so the journal never records a torn under-count (see
    /// [`Session::stash_tick`]).
    pub stash_captured: Option<(u32, u32)>,
    /// The session's cached shared stash was dropped: a new game (player unit id changed).
    pub stash_invalidated: bool,
}

/// The per-game part of a [`Session`]: everything that must survive an overlay restart within
/// the game. Serialized as JSON by `game_file`. `player_id` is required (a file without it is
/// rejected, never restored as id 0); the other fields default, so older files still load.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameState {
    /// Player unit id of the game this state belongs to.
    pub player_id: u32,
    /// Superunique and quest monster presets whose monster was seen dead in this game, by
    /// (level id, preset x, y).
    #[serde(default)]
    killed: HashSet<LevelPos>,
    /// Consumable quest objects (Diablo seals) seen operated in this game, by (level id, live
    /// x, y). Their presets are hidden for the game, also once the live object unloads.
    #[serde(default)]
    operated: HashSet<LevelPos>,
    /// Player position on arriving in each level of this game, by level id.
    #[serde(default)]
    arrivals: HashMap<u32, (u16, u16)>,
    /// The level the player was in just before arriving in each level of this game, by level
    /// id (recorded with the arrival, first visit wins).
    #[serde(default)]
    came_from: HashMap<u32, u32>,
    /// Every preset seen per level id, raw (before filtering, `entry` false), first seen first.
    #[serde(default)]
    presets: HashMap<u32, Vec<Preset>>,
    /// Every shrine seen per level id, with its latest state.
    #[serde(default)]
    shrines: HashMap<u32, Vec<Shrine>>,
    /// Every well seen per level id.
    #[serde(default)]
    wells: HashMap<u32, Vec<Well>>,
    /// Every room-tile link seen per level id. Kept with the border links: a level with a
    /// room link draws tile exits, not a border arrow ([`crate::render::border_exits`]).
    #[serde(default)]
    room_links: HashMap<u32, Vec<RoomLink>>,
    /// Every walk-through border link seen per level id (`entry` false; it is set per snapshot).
    #[serde(default)]
    border_links: HashMap<u32, Vec<BorderLink>>,
    /// Origins (x, y) of landmark candidate rooms ([`crate::render::candidate_room`]) seen
    /// loaded in this game, per level id: their preset lists were complete then.
    #[serde(default)]
    loaded_rooms: HashMap<u32, HashSet<(u32, u32)>>,
    /// Levels seen terrorized in this game ([`Snapshot::terrorized`]). Kept so the info line
    /// doesn't drop back to the table monster levels once the desecrated monsters near the
    /// player are dead or unloaded.
    #[serde(default)]
    terrorized: HashSet<u32>,
    /// Cows (by unit id) seen dead in the Moo Moo Farm in this game: each counts once
    /// ([`crate::cows`]).
    #[serde(default)]
    cows: HashSet<u32>,
    /// The player was in the Moo Moo Farm in this game (the cow run is counted).
    #[serde(default)]
    cow_run: bool,
    /// The player's path through each level of this game, by level id (world tiles, oldest
    /// first): a point at least [`TRAIL_STEP`] from the previous one, at most [`TRAIL_CAP`] per
    /// level (the oldest go first). Every level since the user asked for it everywhere
    /// (2026-11-15); it started in the Moo Moo Farm.
    #[serde(default)]
    trails: HashMap<u32, Vec<(u16, u16)>>,
}

impl GameState {
    fn new(player_id: u32) -> Self {
        Self {
            player_id,
            ..Self::default()
        }
    }

    /// Remembers the live presets, shrines, wells and links of `snap`'s level and adds the
    /// remembered ones it lacks (same kind, txt and position for presets; same position for
    /// shrines and wells; equal links). Returns whether anything was newly remembered or a
    /// shrine changed.
    fn remember(&mut self, snap: &mut Snapshot) -> bool {
        let level = snap.level.id;
        let mut changed = false;

        let same = |a: &Preset, b: &Preset| (a.kind, a.txt, a.x, a.y) == (b.kind, b.txt, b.x, b.y);
        let seen = self.presets.entry(level).or_default();
        for p in &snap.presets {
            if !seen.iter().any(|s| same(s, p)) {
                seen.push(Preset {
                    entry: false,
                    ..p.clone()
                });
                changed = true;
            }
        }
        let missing: Vec<Preset> = seen
            .iter()
            .filter(|s| !snap.presets.iter().any(|p| same(p, s)))
            .cloned()
            .collect();
        snap.presets.extend(missing);

        let seen = self.shrines.entry(level).or_default();
        for live in &snap.shrines {
            match seen.iter_mut().find(|s| (s.x, s.y) == (live.x, live.y)) {
                Some(s) if s == live => {}
                Some(s) => {
                    s.clone_from(live);
                    changed = true;
                }
                None => {
                    seen.push(live.clone());
                    changed = true;
                }
            }
        }
        let missing: Vec<Shrine> = seen
            .iter()
            .filter(|s| !snap.shrines.iter().any(|l| (l.x, l.y) == (s.x, s.y)))
            .cloned()
            .collect();
        snap.shrines.extend(missing);

        let seen = self.wells.entry(level).or_default();
        for live in &snap.wells {
            if !seen.iter().any(|w| (w.x, w.y) == (live.x, live.y)) {
                seen.push(live.clone());
                changed = true;
            }
        }
        let missing: Vec<Well> = seen
            .iter()
            .filter(|w| !snap.wells.iter().any(|l| (l.x, l.y) == (w.x, w.y)))
            .cloned()
            .collect();
        snap.wells.extend(missing);

        changed |= merge(self.room_links.entry(level).or_default(), &mut snap.room_links);
        // Live links carry `entry` false here (it is set after `remember`), so equality is
        // the room pair.
        changed |= merge(
            self.border_links.entry(level).or_default(),
            &mut snap.border_links,
        );

        // Only levels with candidate rooms get an entry, so the game file stays small.
        for room in snap
            .level
            .rooms
            .iter_mut()
            .filter(|r| candidate_room(level, r.preset))
        {
            let at = (room.x, room.y);
            if room.loaded {
                changed |= self.loaded_rooms.entry(level).or_default().insert(at);
            }
            room.seen_loaded = self
                .loaded_rooms
                .get(&level)
                .is_some_and(|seen| seen.contains(&at));
        }
        changed
    }
}

/// Adds the `live` items `seen` lacks to `seen` and the `seen` items `live` lacks to `live`.
/// Returns whether `seen` grew. Linear: a level has a few hundred links at most.
fn merge<T: Clone + PartialEq>(seen: &mut Vec<T>, live: &mut Vec<T>) -> bool {
    let before = seen.len();
    for l in live.iter() {
        if !seen.contains(l) {
            seen.push(l.clone());
        }
    }
    let missing: Vec<T> = seen.iter().filter(|s| !live.contains(s)).cloned().collect();
    live.extend(missing);
    seen.len() > before
}

#[derive(Debug, Default)]
pub struct Session {
    /// The current game's state; meaningful once `started`.
    state: GameState,
    /// A snapshot has been applied (`state.player_id` is the current game's).
    started: bool,
    /// State loaded from disk ([`Session::restore`]), adopted by the first snapshot if its
    /// player id matches, else dropped.
    restored: Option<GameState>,
    /// `state` changed since [`Session::take_changed`] last returned it.
    changed: bool,
    /// Superunique preset each superunique monster (by unit id) was first seen alive near in
    /// this game. Its death kills that preset however far it was chased. Not saved: a restart
    /// mid-fight falls back to the corpse's nearest preset.
    bound: HashMap<u32, LevelPos>,
    /// Level id of the previous snapshot (kept across games: only its change matters).
    last_level: Option<u32>,
    /// Level whose arrival point may still move, and how many more snapshots it may.
    settling: Option<(u32, u8)>,
    /// The shared stash captured the last time live tab units were seen (the user had opened
    /// the stash at least once this game). Kept until the player unit id changes: the tab
    /// units vanish once the stash UI closes or on area transitions, but the contents stay
    /// valid for the game (user decision, 2026-11-17). Not saved to disk.
    stash: Option<Vec<StashTab>>,
}

/// Snapshots (~1 s at 5 Hz) after an arrival during which the arrival point follows the player
/// while no tile lies near it: the first snapshot after a level change can still carry the old
/// level's position, or 0,0 mid-load.
const ARRIVAL_SETTLE: u8 = 5;

/// Name of the superunique whose monster preset has txt id `txt`, if it is one.
#[must_use]
pub fn superunique(txt: u32) -> Option<&'static str> {
    SUPERUNIQUES
        .binary_search_by_key(&txt, |&(id, _)| id)
        .ok()
        .and_then(|i| SUPERUNIQUES.get(i))
        .map(|&(_, name)| name)
}

impl Session {
    /// Offers a saved game state; the first snapshot adopts it if its player id matches.
    pub fn restore(&mut self, state: GameState) {
        self.restored = Some(state);
    }

    /// Unit id of the player of the current game (or of the restored state before the first
    /// snapshot): pass it to [`crate::game::snapshot`] so the choice stays on that player.
    #[must_use]
    pub fn player_id(&self) -> Option<u32> {
        if self.started {
            Some(self.state.player_id)
        } else {
            self.restored.as_ref().map(|s| s.player_id)
        }
    }

    /// Cows counted dead in the Moo Moo Farm in this game.
    #[must_use]
    pub fn cow_kills(&self) -> u64 {
        u64::try_from(self.state.cows.len()).unwrap_or(u64::MAX)
    }

    /// The game state if it changed since the last call (which then clears the change).
    pub fn take_changed(&mut self) -> Option<&GameState> {
        (self.started && std::mem::take(&mut self.changed)).then_some(&self.state)
    }

    /// Switches to the game of player unit id `player_id`; returns whether that is a new game
    /// (the first snapshot is not, unless it discarded a restored state of another game).
    fn enter_game(&mut self, player_id: u32) -> bool {
        if self.started && self.state.player_id == player_id {
            return false;
        }
        let first = !std::mem::replace(&mut self.started, true);
        self.bound.clear();
        match self.restored.take() {
            Some(state) if first && state.player_id == player_id => {
                self.state = state;
                false
            }
            restored => {
                self.state = GameState::new(player_id);
                self.changed = true;
                !first || restored.is_some()
            }
        }
    }

    /// Folds `snap` into the session and rewrites it for drawing: remembered presets, shrines
    /// and wells of the level are added, then monster presets are kept only
    /// if they are a superunique's (see [`superunique`]) or a quest monster's (see
    /// [`quest_monster`]) not killed in this game, and consumable
    /// quest object presets only if not seen operated in this game. A level once seen
    /// terrorized stays [`Snapshot::terrorized`] until the player leaves it (re-detected on the next visit).
    pub fn apply(&mut self, snap: &mut Snapshot) -> Update {
        let mut update = Update {
            new_game: self.enter_game(snap.player.id),
            ..Update::default()
        };
        self.stash_tick(snap, &mut update);
        self.changed |= self.state.remember(snap);
        let level = snap.level.id;
        // Terror Zones rotate hourly: the latch holds for one visit, and a new visit re-detects.
        self.latch_terrorized(snap, level);
        let previous = self.last_level.replace(level);
        let level_changed = previous.is_some_and(|old| old != level);
        let here = (snap.player.x, snap.player.y);
        if (level_changed || update.new_game) && !self.state.arrivals.contains_key(&level) {
            self.changed |= self.state.arrivals.insert(level, here) != Some(here);
            if let Some(old) = previous.filter(|_| level_changed) {
                self.changed |= self.state.came_from.insert(level, old) != Some(old);
            }
            self.settling = Some((level, ARRIVAL_SETTLE));
        }
        // A tile preset or a border to the level come from near the arrival point is the entry.
        // `|`, not `||`: both kinds get marked.
        let from = self.state.came_from.get(&level).copied();
        let mark = |snap: &mut Snapshot, at| {
            mark_entry(snap, at) | from.is_some_and(|from| mark_border_entry(snap, from, at))
        };
        if let Some(&arrival) = self.state.arrivals.get(&level) {
            let found = mark(snap, arrival);
            let was_settling = self.settling.is_some_and(|(l, _)| l == level);
            self.settling = match self.settling {
                Some((l, left)) if l == level && !found && left > 0 => {
                    self.changed |= self.state.arrivals.insert(level, here) != Some(here);
                    if mark(snap, here) {
                        None
                    } else {
                        Some((l, left - 1))
                    }
                }
                Some((l, _)) if l == level => None,
                other => other,
            };
            if was_settling && self.settling.is_none() {
                update.arrival = self.state.arrivals.get(&level).map(|&(x, y)| (level, x, y));
            }
        }

        // A living superunique near its spawn binds its unit id to the nearest superunique
        // preset; its death kills that preset wherever it happens. A dead superunique never seen
        // alive there (killed while loading, say) falls back to the nearest preset within
        // KILL_RADIUS of the corpse.
        let nearest_preset = |m: &Monster, radius: f64| {
            let (mx, my) = (f64::from(m.x), f64::from(m.y));
            snap.presets
                .iter()
                .filter(|p| p.kind == PresetKind::Monster && superunique(p.txt).is_some())
                .map(|p| ((f64::from(p.x) - mx).hypot(f64::from(p.y) - my), p))
                .filter(|&(d, _)| d <= radius)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, p)| (level, p.x, p.y))
        };
        for m in snap
            .monsters
            .iter()
            .filter(|m| m.flags & monster_data::SUPERUNIQUE != 0)
        {
            if m.is_dead() {
                if let Some(pos) = self
                    .bound
                    .get(&m.id)
                    .copied()
                    .or_else(|| nearest_preset(m, KILL_RADIUS))
                {
                    self.changed |= self.state.killed.insert(pos);
                }
            } else if !self.bound.contains_key(&m.id)
                && let Some(pos) = nearest_preset(m, BIND_RADIUS)
            {
                self.bound.insert(m.id, pos);
            }
        }

        self.kill_quest_monsters(snap);
        if level == cows::COW_LEVEL {
            self.count_cows(snap, &mut update);
        }
        self.follow_trail(snap);

        for o in &snap.operated_objects {
            self.changed |= self.state.operated.insert((level, o.x, o.y));
        }
        let operated_near = |x: u32, y: u32| {
            self.state.operated.iter().any(|&(l, ox, oy)| {
                l == level
                    && (f64::from(x) - f64::from(ox)).hypot(f64::from(y) - f64::from(oy)) <= OPERATED_RADIUS
            })
        };

        snap.presets.retain(|p| match p.kind {
            PresetKind::Monster => {
                (superunique(p.txt).is_some() || quest_monster(p.txt).is_some())
                    && !self.state.killed.contains(&(level, p.x, p.y))
            }
            PresetKind::Object => !(is_consumable_quest_object(p.txt) && operated_near(p.x, p.y)),
            _ => true,
        });
        update
    }
}

/// A live stash read merged over the cached capture, per tab (review 2026-11-17): a live tab
/// that reads empty keeps the cached tab of the same kind (its unit not loaded yet, or a torn
/// read), and a live read without a materials tab (the memory-order fallback when the UI-order
/// tab list is unreadable) never replaces a cache that came from the list.
fn merge_stash(cached: Option<&[stash::StashTab]>, live: Vec<stash::StashTab>) -> Vec<stash::StashTab> {
    let Some(cached) = cached else { return live };
    let has_materials = |tabs: &[stash::StashTab]| tabs.iter().any(|t| t.kind == stash::TabKind::Materials);
    if has_materials(cached) && !has_materials(&live) {
        return cached.to_vec();
    }
    live.into_iter()
        .map(|tab| {
            if tab.items.is_empty()
                && let Some(old) = cached.iter().find(|c| c.kind == tab.kind && !c.items.is_empty())
            {
                return old.clone();
            }
            tab
        })
        .collect()
}

impl Session {
    /// Latches the level's terrorized state for this visit: a level once seen terrorized stays
    /// so until the player leaves it (re-detected on the next visit).
    fn latch_terrorized(&mut self, snap: &mut Snapshot, level: u32) {
        if self.last_level != Some(level) {
            self.changed |= self.state.terrorized.remove(&level);
        }
        if snap.terrorized {
            self.changed |= self.state.terrorized.insert(level);
        } else {
            snap.terrorized = self.state.terrorized.contains(&level);
        }
    }

    /// Captures, caches and serves the shared stash (see [`crate::stash`]): the live tabs are
    /// captured the first time they appear (the user opened the stash), kept while the tab
    /// units are gone, and dropped on a new game. Reports a first capture or an invalidation
    /// in `update`.
    ///
    /// A degraded live read — tab units present but every tab still empty (a torn mid-load
    /// capture, research note §3.4) — never downgrades a cache that has items: the session
    /// would otherwise serve an empty or partial stash until the user reopens the stash UI.
    /// The trade-off: a stash emptied entirely keeps showing its last capture until the next
    /// live read that has items (or a new game). The first-capture event fires only for a
    /// read with at least one item, so the journal never records a torn under-count.
    fn stash_tick(&mut self, snap: &mut Snapshot, update: &mut Update) {
        if update.new_game && self.stash.take().is_some() {
            update.stash_invalidated = true;
        }
        let live = std::mem::take(&mut snap.stash);
        let live_items: usize = live.iter().map(|t| t.items.len()).sum();
        let cached_items: usize = self.stash.iter().flatten().map(|t| t.items.len()).sum();
        if !live.is_empty() && (live_items > 0 || cached_items == 0) {
            if cached_items == 0 && live_items > 0 {
                update.stash_captured = Some((
                    u32::try_from(live.len()).unwrap_or(u32::MAX),
                    u32::try_from(live_items).unwrap_or(u32::MAX),
                ));
            }
            self.stash = Some(merge_stash(self.stash.as_deref(), live));
        }
        if let Some(cached) = self.stash.clone() {
            snap.stash = cached;
            snap.stash_names = stash::stash_names(&snap.stash);
        }
    }

    /// In the Moo Moo Farm: marks the game as a cow run (once), counts each dead cow once by
    /// unit id and sets [`Snapshot::cows`] for the info line.
    fn count_cows(&mut self, snap: &mut Snapshot, update: &mut Update) {
        if !std::mem::replace(&mut self.state.cow_run, true) {
            update.cow_run = true;
            self.changed = true;
        }
        let mut alive = 0u32;
        for m in snap.monsters.iter().filter(|m| cows::is_cow(m.txt)) {
            if !m.is_dead() {
                alive = alive.saturating_add(1);
            } else if self.state.cows.insert(m.id) {
                update.new_cows = update.new_cows.saturating_add(1);
                self.changed = true;
            }
        }
        snap.cows = Some(cows::Tally {
            kills: self.cow_kills(),
            alive,
        });
    }

    /// Adds the player's position to the trail if it is at least [`TRAIL_STEP`] from the last
    /// point (0,0 is a loading frame and skipped), dropping the oldest past [`TRAIL_CAP`], and
    /// hands the trail to the snapshot.
    fn follow_trail(&mut self, snap: &mut Snapshot) {
        let here = (snap.player.x, snap.player.y);
        let trail = self.state.trails.entry(snap.level.id).or_default();
        let far_enough = trail.last().is_none_or(|&(x, y)| {
            (f64::from(here.0) - f64::from(x)).hypot(f64::from(here.1) - f64::from(y)) >= TRAIL_STEP
        });
        if here != (0, 0) && far_enough {
            if trail.len() >= TRAIL_CAP {
                // O(n) shift, but at most once per new point of a 5000-point Vec.
                trail.remove(0);
            }
            trail.push(here);
            self.changed = true;
        }
        snap.trail.clone_from(trail);
    }

    /// A dead quest monster (prison door broken) kills the nearest quest monster preset of its
    /// txt id within [`QUEST_KILL_RADIUS`]. Normal monsters get no unit-id binding: a door
    /// doesn't move.
    fn kill_quest_monsters(&mut self, snap: &Snapshot) {
        let level = snap.level.id;
        for m in snap
            .monsters
            .iter()
            .filter(|m| m.is_dead() && quest_monster(m.txt).is_some())
        {
            let (mx, my) = (f64::from(m.x), f64::from(m.y));
            if let Some((_, p)) = snap
                .presets
                .iter()
                .filter(|p| p.kind == PresetKind::Monster && p.txt == m.txt)
                .map(|p| ((f64::from(p.x) - mx).hypot(f64::from(p.y) - my), p))
                .filter(|&(d, _)| d <= QUEST_KILL_RADIUS)
                .min_by(|a, b| a.0.total_cmp(&b.0))
            {
                self.changed |= self.state.killed.insert((level, p.x, p.y));
            }
        }
    }
}

/// Marks the tile preset nearest to `arrival` (within [`ENTRY_RADIUS`]) and the tiles clustered
/// around it as the entry. Returns whether one was found.
fn mark_entry(snap: &mut Snapshot, (ax, ay): (u16, u16)) -> bool {
    let dist = |x: u32, y: u32, (ox, oy): (f64, f64)| (f64::from(x) - ox).hypot(f64::from(y) - oy);
    let arrival = (f64::from(ax), f64::from(ay));
    let nearest = snap
        .presets
        .iter()
        .filter(|p| p.kind == PresetKind::Tile)
        .map(|p| (dist(p.x, p.y, arrival), p))
        .filter(|&(d, _)| d <= ENTRY_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| (f64::from(p.x), f64::from(p.y)));
    let Some(centre) = nearest else { return false };
    for p in &mut snap.presets {
        if p.kind == PresetKind::Tile && dist(p.x, p.y, centre) <= ENTRY_CLUSTER {
            p.entry = true;
        }
    }
    true
}

/// Marks the border links to `from` (the level the player came from) as the entry, if the
/// arrival point lies within [`ENTRY_RADIUS`] of one of their rooms in this level: walking over
/// the border puts the player there, while a waypoint or portal from that level does not.
/// Returns whether it was.
fn mark_border_entry(snap: &mut Snapshot, from: u32, (ax, ay): (u16, u16)) -> bool {
    let near = |r: &RoomRect| {
        let gap = |a: u16, lo: u32, len: u32| {
            let (a, lo, hi) = (f64::from(a), f64::from(lo), f64::from(lo.saturating_add(len)));
            (lo - a).max(a - hi).max(0.0)
        };
        gap(ax, r.x, r.w).hypot(gap(ay, r.y, r.h)) <= ENTRY_RADIUS
    };
    let walked_in = snap
        .border_links
        .iter()
        .any(|l| l.to_level_id == from && near(&l.from_room));
    if walked_in {
        for l in snap.border_links.iter_mut().filter(|l| l.to_level_id == from) {
            l.entry = true;
        }
    }
    walked_in
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Addr, BorderLink, Level, OperatedObject, Player};

    fn monster(flags: u8, mode: u32, x: u16, y: u16) -> Monster {
        Monster {
            addr: Addr(0),
            id: 9,
            txt: 1,
            mode,
            x,
            y,
            flags,
            name_seed: 0,
            owner_id: 0,
            owner_type: 0,
            desecrated: false,
        }
    }

    fn preset(kind: PresetKind, txt: u32, x: u32, y: u32) -> Preset {
        Preset {
            kind,
            txt,
            x,
            y,
            entry: false,
        }
    }

    fn snap(player_id: u32, act: u64, level: u32) -> Snapshot {
        Snapshot {
            player: Player {
                addr: Addr(0x1000),
                id: player_id,
                name: "P".into(),
                class: 0,
                x: 30,
                y: 30,
                level_id: level,
                act: Some(Addr(act)),
                char_level: None,
                speed: None,
                wsm: None,
                strength: None,
                dexterity: None,
            },
            equipped: vec![],
            merc_equipped: vec![],
            equipped_items: vec![],
            owned_names: vec![],
            runeword_copies: vec![],
            merc_worn: vec![],
            stash: vec![],
            stash_names: vec![],
            player_candidates: vec![],
            player_chosen_by: crate::game::ChosenBy::FirstCandidate,
            level: Level {
                id: level,
                bounds: None,
                rooms: vec![],
            },
            presets: vec![],
            room_links: vec![],
            border_links: vec![],
            monsters: vec![],
            ground_items: 0,
            drops: vec![],
            shrines: vec![],
            wells: vec![],
            operated_objects: vec![],
            inventory_items: 0,
            owned_items: vec![],
            held_materials: vec![],
            white_bases: vec![],
            difficulty: None,
            staff_tomb: None,
            terrorized: false,
            cows: None,
            trail: vec![],
            skills: crate::game::Skills::default(),
            shop: None,
            warnings: vec![],
        }
    }

    /// A stash snapshot with one live tab holding `names` (identified uniques).
    fn stash_snap(player_id: u32, act: u64, level: u32, names: &[&str]) -> Snapshot {
        let mut s = snap(player_id, act, level);
        s.stash = vec![crate::stash::StashTab {
            kind: crate::stash::TabKind::Page(1),
            items: names
                .iter()
                .map(|n| crate::stash::StashItem {
                    name: (*n).to_owned(),
                    code: None,
                    quality: 7,
                    identified: true,
                    ethereal: false,
                    runeword: false,
                    quantity: None,
                })
                .collect(),
        }];
        s
    }

    #[test]
    fn the_stash_is_cached_until_the_player_id_changes() {
        let mut session = Session::default();
        let mut first = stash_snap(7, 0xa000, 1, &["Harlequin Crest"]);
        let update = session.apply(&mut first);
        assert_eq!(update.stash_captured, Some((1, 1)), "first capture logs");
        assert!(!update.stash_invalidated);

        // The tab units vanish (stash UI closed): the capture is served and its names stay.
        let mut gone = snap(7, 0xa000, 40);
        session.apply(&mut gone);
        assert_eq!(gone.stash.len(), 1, "cached stash survives empty live reads");
        assert_eq!(gone.stash_names, vec!["Harlequin Crest"]);

        // Live tabs again: recaptured silently (no second capture event).
        let mut again = stash_snap(7, 0xa000, 1, &["Harlequin Crest", "Sigon's Visor"]);
        let update = session.apply(&mut again);
        assert_eq!(update.stash_captured, None, "already captured this game");
        assert_eq!(again.stash_names.len(), 2);

        // A new game (player unit id changed) drops the cache.
        let mut next_game = snap(8, 0xa000, 1);
        let update = session.apply(&mut next_game);
        assert!(update.stash_invalidated);
        assert!(next_game.stash.is_empty() && next_game.stash_names.is_empty());
    }

    #[test]
    fn a_torn_all_tabs_empty_read_does_not_downgrade_the_cache() {
        let mut session = Session::default();
        let mut first = stash_snap(7, 0xa000, 1, &["Harlequin Crest"]);
        session.apply(&mut first);

        // Tab units seen again but still empty (torn mid-load capture, §3.4): the cache with
        // its item survives and keeps being served.
        let mut torn = stash_snap(7, 0xa000, 1, &[]);
        let update = session.apply(&mut torn);
        assert_eq!(update.stash_captured, None);
        assert_eq!(torn.stash.len(), 1, "the cached capture is served");
        assert_eq!(torn.stash_names, vec!["Harlequin Crest"]);
    }

    #[test]
    fn the_first_capture_event_waits_for_a_read_with_items() {
        let mut session = Session::default();
        // A torn first read (tab units exist, no items yet) is stored but silent.
        let mut torn = stash_snap(7, 0xa000, 1, &[]);
        let update = session.apply(&mut torn);
        assert_eq!(update.stash_captured, None, "no under-count in the journal");
        assert_eq!(torn.stash.len(), 1, "the empty capture is cached");

        // The first itemful read is the capture the journal records, exactly once.
        let mut filled = stash_snap(7, 0xa000, 1, &["Harlequin Crest"]);
        let update = session.apply(&mut filled);
        assert_eq!(update.stash_captured, Some((1, 1)));
        let mut again = stash_snap(7, 0xa000, 1, &["Harlequin Crest"]);
        let update = session.apply(&mut again);
        assert_eq!(update.stash_captured, None, "already captured this game");
    }

    #[test]
    fn game_change_is_a_new_player_id_not_act_travel() {
        let mut s = Session::default();
        assert!(
            !s.apply(&mut snap(7, 0xa000, 29)).new_game,
            "first game is not a change"
        );
        assert!(
            !s.apply(&mut snap(7, 0xa000, 35)).new_game,
            "level change in the same game"
        );
        assert!(!s.apply(&mut snap(7, 0xb000, 40)).new_game, "act travel");
        assert!(s.apply(&mut snap(8, 0xb000, 29)).new_game, "new player id");
        assert!(!s.apply(&mut snap(8, 0xb000, 29)).new_game);
    }

    #[test]
    fn terrorized_level_stays_terrorized_for_the_visit() {
        let mut s = Session::default();
        let mut a = snap(7, 0xa000, 39);
        s.apply(&mut a);
        assert!(!a.terrorized);
        a.terrorized = true;
        s.apply(&mut a);
        assert!(a.terrorized);
        // The desecrated monsters are dead or unloaded: still terrorized.
        let mut later = snap(7, 0xa000, 39);
        s.apply(&mut later);
        assert!(later.terrorized, "kept for the level");
        let mut other = snap(7, 0xa000, 40);
        s.apply(&mut other);
        assert!(!other.terrorized, "per level");
        // Back in 39: zones rotate hourly, so the old latch is dropped until re-detected.
        let mut revisit = snap(7, 0xa000, 39);
        s.apply(&mut revisit);
        assert!(!revisit.terrorized, "re-detected per visit");
        let mut next_game = snap(8, 0xa000, 39);
        s.apply(&mut next_game);
        assert!(!next_game.terrorized, "per game");
    }

    #[test]
    fn superunique_table_is_751_plus_hc_idx() {
        assert_eq!(SUPERUNIQUES.len(), 66);
        assert!(SUPERUNIQUES.is_sorted_by(|a, b| a.0 < b.0), "sorted, unique ids");
        assert_eq!(SUPERUNIQUES.first().map(|r| r.0), Some(751));
        assert_eq!(SUPERUNIQUES.last().map(|r| r.0), Some(816));
        assert_eq!(superunique(751), Some("Bishibosh"));
        assert_eq!(superunique(757), Some("The Countess"));
        assert_eq!(superunique(771), Some("The Smith"));
        assert_eq!(superunique(775), Some("Sarina the Battlemaid"));
        assert_eq!(superunique(776), Some("Icehawk Riftwing"));
        for generic in [750, 817, 328, 840] {
            assert_eq!(superunique(generic), None, "{generic}");
        }
    }

    #[test]
    fn only_superunique_monster_presets_are_drawn() {
        let mut s = Session::default();
        let mut a = snap(1, 0xa000, 25);
        a.presets = vec![
            preset(PresetKind::Monster, 757, 100, 100),
            preset(PresetKind::Monster, 328, 108, 100),
            preset(PresetKind::Monster, 840, 300, 300),
            preset(PresetKind::Tile, 7, 100, 100),
        ];
        // A live superunique next to a generic preset changes nothing (no learning).
        a.monsters = vec![monster(0x02, 1, 300, 301)];
        s.apply(&mut a);
        let kept: Vec<u32> = a.presets.iter().map(|p| p.txt).collect();
        assert_eq!(kept, vec![757, 7], "generic monster presets dropped, tiles kept");
    }

    fn with_txt(txt: u32, mode: u32, x: u16, y: u16) -> Monster {
        Monster {
            txt,
            ..monster(0, mode, x, y)
        }
    }

    #[test]
    fn candidate_rooms_seen_loaded_are_remembered_for_the_game() {
        use crate::game::Room;
        use crate::render::{LANDMARKS, Marker, landmarks};
        // Flayer Jungle: clearing 602 (two rooms), clearing 600 (one room), a plain room.
        let jungle = |player_id, loaded: [bool; 4]| {
            let mut s = snap(player_id, 0xa000, 78);
            s.level.rooms = [(0, 602), (40, 602), (200, 600), (300, 534)]
                .into_iter()
                .zip(loaded)
                .map(|((x, preset), loaded)| Room {
                    x,
                    y: 0,
                    w: 40,
                    h: 40,
                    loaded,
                    seen_loaded: false,
                    preset: Some(preset),
                })
                .collect();
            s
        };
        let stars = |s: &Snapshot| {
            landmarks(s, LANDMARKS)
                .into_iter()
                .map(|(m, x, _)| (m, x))
                .collect::<Vec<_>>()
        };
        let gidbinn = Marker::Landmark("gidbinn");
        let mut s = Session::default();

        // The player stands in 602: both its rooms loaded, no altar.
        let mut here = jungle(1, [true, true, false, true]);
        s.apply(&mut here);
        assert_eq!(stars(&here), vec![(gidbinn, 220.0)]);
        assert!(s.take_changed().is_some());
        // Walked away: 602 unloaded but stays ruled out; only candidate rooms are remembered.
        let mut away = jungle(1, [false, false, false, false]);
        s.apply(&mut away);
        assert!(s.take_changed().is_none(), "nothing new to remember");
        let seen: Vec<bool> = away.level.rooms.iter().map(|r| r.seen_loaded).collect();
        assert_eq!(seen, vec![true, true, false, false]);
        assert_eq!(stars(&away), vec![(gidbinn, 220.0)]);
        // A new game (other player unit id) forgets it: both clearings are candidates again.
        let mut next_game = jungle(2, [false, false, false, false]);
        s.apply(&mut next_game);
        assert_eq!(stars(&next_game), vec![(gidbinn, 40.0), (gidbinn, 220.0)]);
    }

    #[test]
    fn prison_door_preset_is_kept_until_a_dead_door_is_seen_near_it() {
        let presets = || {
            vec![
                preset(PresetKind::Monster, 434, 100, 100), // prison door
                preset(PresetKind::Monster, 434, 140, 100), // another cage's door
                preset(PresetKind::Monster, 433, 102, 100), // barricade door: dropped
                preset(PresetKind::Monster, 435, 110, 100), // barricade tower: dropped
            ]
        };
        let kept = |s: &Snapshot| s.presets.iter().map(|p| (p.txt, p.x)).collect::<Vec<_>>();
        let mut s = Session::default();
        let mut alive = snap(1, 0xa000, 111);
        alive.presets = presets();
        alive.monsters = vec![
            with_txt(434, 1, 100, 101),  // standing door
            with_txt(433, 0, 101, 100),  // dead other monster near the door
            with_txt(434, 12, 120, 100), // dead door 20 tiles from either preset
        ];
        s.apply(&mut alive);
        assert_eq!(kept(&alive), vec![(434, 100), (434, 140)]);

        let mut broken = snap(1, 0xa000, 111);
        broken.presets = presets();
        broken.monsters = vec![with_txt(434, 0, 103, 104)];
        s.apply(&mut broken);
        assert_eq!(kept(&broken), vec![(434, 140)], "broken door's preset hidden");
        // Stays hidden after the corpse unloads; another level's preset at that spot is kept.
        let mut later = snap(1, 0xa000, 111);
        later.presets = presets();
        s.apply(&mut later);
        assert_eq!(kept(&later), vec![(434, 140)]);
        let mut other_level = snap(1, 0xa000, 112);
        other_level.presets = presets();
        s.apply(&mut other_level);
        assert_eq!(other_level.presets.len(), 2);
        // A new game brings it back.
        let mut next_game = snap(2, 0xc000, 111);
        next_game.presets = presets();
        s.apply(&mut next_game);
        assert_eq!(kept(&next_game), vec![(434, 100), (434, 140)]);
    }

    #[test]
    fn killed_superunique_marker_is_removed_for_the_game() {
        let countess = || preset(PresetKind::Monster, 757, 100, 100);
        let mut session = Session::default();
        let mut alive = snap(1, 0xa000, 25);
        alive.presets = vec![countess()];
        alive.monsters = vec![monster(0x02, 1, 101, 100)];
        session.apply(&mut alive);
        assert_eq!(alive.presets.len(), 1, "living superunique keeps its marker");

        for dead_mode in [0, 12] {
            let mut session = Session::default();
            let mut killed = snap(1, 0xa000, 25);
            killed.presets = vec![countess()];
            killed.monsters = vec![monster(0x02, dead_mode, 120, 115)];
            session.apply(&mut killed);
            assert_eq!(
                killed.presets,
                vec![],
                "mode {dead_mode}: dead superunique's marker removed"
            );
            // The corpse is gone later: the marker stays removed.
            let mut later = snap(1, 0xa000, 25);
            later.presets = vec![countess()];
            session.apply(&mut later);
            assert_eq!(later.presets, vec![]);
            // The same spot in another level is another preset.
            let mut other_level = snap(1, 0xa000, 26);
            other_level.presets = vec![countess()];
            session.apply(&mut other_level);
            assert_eq!(other_level.presets.len(), 1);
            // A new game brings it back.
            let mut next_game = snap(2, 0xc000, 25);
            next_game.presets = vec![countess()];
            session.apply(&mut next_game);
            assert_eq!(next_game.presets.len(), 1);
        }
    }

    #[test]
    fn a_kill_is_credited_to_the_nearest_superunique_preset() {
        let presets = || {
            vec![
                preset(PresetKind::Monster, 771, 100, 100), // superunique, 19 tiles from the corpse
                preset(PresetKind::Monster, 775, 140, 100), // superunique, 21 tiles from the corpse
                preset(PresetKind::Monster, 840, 119, 101), // generic, nearest: never credited
                preset(PresetKind::Monster, 776, 500, 500), // beyond the radius
            ]
        };
        let mut s = Session::default();
        let mut a = snap(1, 0xa000, 28);
        a.presets = presets();
        a.monsters = vec![
            monster(0x02, 0, 119, 100), // dead superunique between 771 and 775
            monster(0x08, 0, 140, 100), // dead unique on 775: not a superunique
            monster(0x02, 1, 500, 500), // living superunique on 776
        ];
        s.apply(&mut a);
        let kept: Vec<u32> = a.presets.iter().map(|p| p.txt).collect();
        assert_eq!(kept, vec![775, 776]);

        // A dead superunique with no superunique preset within the radius removes nothing.
        let mut s = Session::default();
        let mut far = snap(1, 0xa000, 28);
        far.presets = presets();
        far.monsters = vec![monster(0x02, 0, 300, 300)];
        s.apply(&mut far);
        assert_eq!(far.presets.len(), 3);
    }

    fn superunique_with_id(id: u32, mode: u32, x: u16, y: u16) -> Monster {
        Monster {
            id,
            ..monster(0x02, mode, x, y)
        }
    }

    #[test]
    fn a_superunique_seen_on_its_preset_is_killed_however_far_it_is_chased() {
        // Dac Farren (799) spawns on his preset, chases the player and dies 34+ tiles away.
        let dac = || preset(PresetKind::Monster, 799, 100, 100);
        let mut s = Session::default();
        let mut alive = snap(1, 0xa000, 2);
        alive.presets = vec![dac()];
        alive.monsters = vec![superunique_with_id(42, 1, 100, 100)];
        s.apply(&mut alive);
        assert_eq!(alive.presets.len(), 1);

        // Another superunique (another unit id) dies far away: the bound preset stays.
        let mut other = snap(1, 0xa000, 2);
        other.presets = vec![dac()];
        other.monsters = vec![superunique_with_id(43, 0, 140, 130)];
        s.apply(&mut other);
        assert_eq!(
            other.presets.len(),
            1,
            "a different unit id doesn't kill the bound preset"
        );

        let mut dead = snap(1, 0xa000, 2);
        dead.presets = vec![dac()];
        dead.monsters = vec![superunique_with_id(42, 12, 140, 130)]; // 50 tiles away
        s.apply(&mut dead);
        assert_eq!(
            dead.presets,
            vec![],
            "bound superunique's kill credited beyond KILL_RADIUS"
        );
    }

    #[test]
    fn binding_picks_the_nearest_superunique_preset() {
        let presets = || {
            vec![
                preset(PresetKind::Monster, 771, 100, 100), // 10 tiles from the spawn
                preset(PresetKind::Monster, 775, 116, 100), // 6 tiles from the spawn
                preset(PresetKind::Monster, 840, 110, 100), // generic, on the spawn: never bound
            ]
        };
        let mut s = Session::default();
        let mut alive = snap(1, 0xa000, 28);
        alive.presets = presets();
        alive.monsters = vec![superunique_with_id(42, 1, 110, 100)];
        s.apply(&mut alive);

        let mut dead = snap(1, 0xa000, 28);
        dead.presets = presets();
        dead.monsters = vec![superunique_with_id(42, 0, 300, 300)];
        s.apply(&mut dead);
        let kept: Vec<u32> = dead.presets.iter().map(|p| p.txt).collect();
        assert_eq!(kept, vec![771]);
    }

    #[test]
    fn a_new_game_forgets_bindings() {
        let dac = || preset(PresetKind::Monster, 799, 100, 100);
        let mut s = Session::default();
        let mut alive = snap(1, 0xa000, 2);
        alive.presets = vec![dac()];
        alive.monsters = vec![superunique_with_id(42, 1, 100, 100)];
        s.apply(&mut alive);

        // Same unit id in a new game, dead far from the preset: not a kill of this game's preset.
        let mut next_game = snap(2, 0xc000, 2);
        next_game.presets = vec![dac()];
        next_game.monsters = vec![superunique_with_id(42, 0, 160, 100)];
        s.apply(&mut next_game);
        assert_eq!(next_game.presets.len(), 1);
    }

    #[test]
    fn operated_seal_marker_is_removed_for_the_game() {
        let seals = || {
            vec![
                preset(PresetKind::Object, 392, 7800, 5300),
                preset(PresetKind::Object, 393, 7810, 5300), // another seal, 10 tiles away
                preset(PresetKind::Object, 119, 7801, 5300), // a waypoint: not consumable
            ]
        };
        let txts = |s: &Snapshot| s.presets.iter().map(|p| p.txt).collect::<Vec<_>>();
        let mut s = Session::default();
        let mut a = snap(1, 0xa000, 108);
        a.presets = seals();
        s.apply(&mut a);
        assert_eq!(txts(&a), vec![392, 393, 119], "nothing operated yet");

        // Seal 392 operated, live position two tiles off its preset.
        let mut b = snap(1, 0xa000, 108);
        b.presets = seals();
        b.operated_objects = vec![OperatedObject {
            txt: 392,
            x: 7802,
            y: 5301,
        }];
        s.apply(&mut b);
        assert_eq!(txts(&b), vec![393, 119]);
        // The live object unloads: still hidden.
        let mut c = snap(1, 0xa000, 108);
        c.presets = seals();
        s.apply(&mut c);
        assert_eq!(txts(&c), vec![393, 119]);
        // Same spot in another level is another object.
        let mut other = snap(1, 0xa000, 107);
        other.presets = seals();
        s.apply(&mut other);
        assert_eq!(txts(&other), vec![392, 393, 119]);
        // A new game brings it back.
        let mut next = snap(2, 0xb000, 108);
        next.presets = seals();
        assert!(s.apply(&mut next).new_game);
        assert_eq!(txts(&next), vec![392, 393, 119]);
    }

    fn at(mut s: Snapshot, x: u16, y: u16, presets: &[Preset]) -> Snapshot {
        s.player.x = x;
        s.player.y = y;
        s.presets = presets.to_vec();
        s
    }

    /// (txt, entry) of the tile presets after `apply`.
    fn tiles(s: &Snapshot) -> Vec<(u32, bool)> {
        s.presets
            .iter()
            .filter(|p| p.kind == PresetKind::Tile)
            .map(|p| (p.txt, p.entry))
            .collect()
    }

    #[test]
    fn entry_is_the_tile_nearest_to_the_arrival_point() {
        let level2 = [
            preset(PresetKind::Tile, 1, 120, 100), // up stairs, 20 tiles from the arrival
            preset(PresetKind::Tile, 2, 400, 400), // down stairs
            preset(PresetKind::Object, 119, 101, 100), // waypoint: never an entry
        ];
        let mut s = Session::default();
        // Overlay started mid-level: no arrival known, every tile is an exit.
        let mut first = at(snap(1, 0xa000, 1), 30, 30, &[preset(PresetKind::Tile, 9, 30, 31)]);
        s.apply(&mut first);
        assert_eq!(tiles(&first), vec![(9, false)]);

        let mut arrive = at(snap(1, 0xa000, 2), 100, 100, &level2);
        s.apply(&mut arrive);
        assert_eq!(tiles(&arrive), vec![(1, true), (2, false)]);
        // Walking on: the arrival point stays where the level was entered.
        let mut walk = at(snap(1, 0xa000, 2), 390, 400, &level2);
        s.apply(&mut walk);
        assert_eq!(tiles(&walk), vec![(1, true), (2, false)]);
        assert!(
            walk.presets
                .iter()
                .all(|p| p.kind == PresetKind::Tile || !p.entry),
            "objects untouched"
        );

        // Down to level 3 and back up: level 2 keeps its first arrival.
        let mut down = at(snap(1, 0xa000, 3), 50, 50, &[preset(PresetKind::Tile, 3, 52, 50)]);
        s.apply(&mut down);
        assert_eq!(tiles(&down), vec![(3, true)]);
        let mut back = at(snap(1, 0xa000, 2), 401, 401, &level2);
        s.apply(&mut back);
        assert_eq!(tiles(&back), vec![(1, true), (2, false)]);
    }

    #[test]
    fn entry_cluster_and_radius() {
        let mut s = Session::default();
        s.apply(&mut snap(1, 0xa000, 1));
        let presets = [
            preset(PresetKind::Tile, 1, 110, 100),
            preset(PresetKind::Tile, 2, 113, 103), // 4.2 from tile 1: same entry
            preset(PresetKind::Tile, 3, 110, 100), // same spot
            preset(PresetKind::Tile, 4, 118, 100), // 8 from tile 1: another exit
        ];
        let mut arrive = at(snap(1, 0xa000, 2), 100, 100, &presets);
        s.apply(&mut arrive);
        assert_eq!(tiles(&arrive), vec![(1, true), (2, true), (3, true), (4, false)]);

        // Nearest tile just beyond 25 tiles: no entry.
        let mut s = Session::default();
        s.apply(&mut snap(1, 0xa000, 1));
        let mut far = at(
            snap(1, 0xa000, 2),
            100,
            100,
            &[preset(PresetKind::Tile, 1, 126, 100)],
        );
        s.apply(&mut far);
        assert_eq!(tiles(&far), vec![(1, false)]);
    }

    /// Level 27 with a border room to 26 at the bottom (y 140..180) and to 28 at the top (y 60..100).
    fn cloister(x: u16, y: u16) -> Snapshot {
        let rect = |x, y| RoomRect { x, y, w: 40, h: 40 };
        let link = |from_y, to_level, to_y| BorderLink {
            from_room: rect(100, from_y),
            to_level_id: to_level,
            to_room: rect(100, to_y),
            entry: false,
        };
        let mut s = at(snap(1, 0xa000, 27), x, y, &[]);
        s.border_links = vec![link(140, 26, 180), link(60, 28, 20)];
        s
    }

    fn entry_levels(s: &Snapshot) -> Vec<u32> {
        s.border_links
            .iter()
            .filter(|l| l.entry)
            .map(|l| l.to_level_id)
            .collect()
    }

    #[test]
    fn walking_over_a_border_marks_the_level_come_from() {
        let mut s = Session::default();
        s.apply(&mut at(snap(1, 0xa000, 26), 120, 185, &[]));
        // Arrival inside the room bordering 26: the 26 border is the entry, 28 stays an exit.
        let mut arrive = cloister(120, 178);
        s.apply(&mut arrive);
        assert_eq!(entry_levels(&arrive), vec![26]);
        // It stays the entry while walking on toward 28.
        let mut later = cloister(120, 70);
        s.apply(&mut later);
        assert_eq!(entry_levels(&later), vec![26]);
    }

    #[test]
    fn border_and_room_links_are_remembered_for_the_game() {
        let mut s = Session::default();
        s.apply(&mut at(snap(1, 0xa000, 26), 120, 185, &[]));
        let mut arrive = cloister(120, 178);
        arrive.room_links = vec![RoomLink {
            from_room: crate::game::RoomPos { x: 100, y: 60 },
            to_level_id: 30,
            to_room: crate::game::RoomPos { x: 0, y: 0 },
        }];
        let seen = arrive.border_links.clone();
        s.apply(&mut arrive);
        // Walked on: the game unloaded the border rooms, the live read has no links.
        for _ in 0..3 {
            let mut away = at(snap(1, 0xa000, 27), 120, 70, &[]);
            s.apply(&mut away);
            let pairs: Vec<(u32, RoomRect)> = away
                .border_links
                .iter()
                .map(|l| (l.to_level_id, l.from_room))
                .collect();
            let want: Vec<(u32, RoomRect)> = seen.iter().map(|l| (l.to_level_id, l.from_room)).collect();
            assert_eq!(pairs, want, "remembered once, not duplicated");
            assert_eq!(
                entry_levels(&away),
                vec![26],
                "entry still from the current logic"
            );
            assert_eq!(away.room_links.len(), 1);
        }
        // Seen live again alongside the remembered ones: still no duplicates.
        let mut again = cloister(120, 70);
        s.apply(&mut again);
        assert_eq!(again.border_links.len(), 2);

        let mut next_game = at(snap(2, 0xa000, 27), 120, 70, &[]);
        assert!(s.apply(&mut next_game).new_game);
        assert_eq!(next_game.border_links, vec![], "a new game forgets the links");
        assert_eq!(next_game.room_links, vec![]);
    }

    #[test]
    fn the_chosen_player_id_is_kept_and_a_vanished_one_is_a_new_game() {
        let mut s = Session::default();
        assert_eq!(s.player_id(), None);
        s.apply(&mut snap(1, 0xa000, 2));
        assert_eq!(s.player_id(), Some(1), "fed back into the next choice");
        // The previous player is gone, so the choice moved on: that's a new game.
        assert!(s.apply(&mut snap(2, 0xa000, 2)).new_game);
        assert_eq!(s.player_id(), Some(2));
        // An overlay restart: the saved player id seeds the choice before the first snapshot.
        let mut s = Session::default();
        s.restore(saved_state());
        assert_eq!(s.player_id(), Some(1));
    }

    #[test]
    fn waypoint_from_an_adjacent_level_is_no_border_entry() {
        let mut s = Session::default();
        s.apply(&mut at(snap(1, 0xa000, 26), 120, 185, &[]));
        // Arrived far from the 26 border rooms (a waypoint at the top): nothing is the entry,
        // also not after the settle window has passed.
        for _ in 0..=ARRIVAL_SETTLE {
            let mut wp = cloister(120, 20);
            s.apply(&mut wp);
            assert_eq!(entry_levels(&wp), Vec::<u32>::new());
        }
    }

    #[test]
    fn waypoint_arrival_has_no_entry() {
        let mut s = Session::default();
        s.apply(&mut snap(1, 0xa000, 1));
        let presets = [
            preset(PresetKind::Object, 157, 200, 200),
            preset(PresetKind::Tile, 1, 260, 200),
            preset(PresetKind::Tile, 2, 500, 500),
        ];
        let mut wp = at(snap(1, 0xa000, 28), 201, 201, &presets);
        s.apply(&mut wp);
        assert_eq!(tiles(&wp), vec![(1, false), (2, false)]);
        // Standing at the waypoint through the settle window locks "no entry"...
        for _ in 0..ARRIVAL_SETTLE {
            s.apply(&mut at(snap(1, 0xa000, 28), 201, 201, &presets));
        }
        // ...so walking up to a tile later does not make it the entry.
        let mut walk = at(snap(1, 0xa000, 28), 259, 200, &presets);
        s.apply(&mut walk);
        assert_eq!(tiles(&walk), vec![(1, false), (2, false)]);
    }

    #[test]
    fn stale_first_frame_after_level_change_settles() {
        let mut s = Session::default();
        s.apply(&mut snap(1, 0xa000, 1));
        let presets = [
            preset(PresetKind::Tile, 1, 1000, 1000),
            preset(PresetKind::Tile, 2, 1300, 1300),
        ];
        // First frame in the new level still carries the old level's position (no tile near)...
        let mut stale = at(snap(1, 0xa000, 29), 5, 5, &presets);
        s.apply(&mut stale);
        assert_eq!(tiles(&stale), vec![(1, false), (2, false)]);
        // ...the next one is at the stairs: that becomes the arrival and stays.
        let mut real = at(snap(1, 0xa000, 29), 1003, 1002, &presets);
        s.apply(&mut real);
        assert_eq!(tiles(&real), vec![(1, true), (2, false)]);
        let mut later = at(snap(1, 0xa000, 29), 1290, 1290, &presets);
        s.apply(&mut later);
        assert_eq!(tiles(&later), vec![(1, true), (2, false)]);
    }

    #[test]
    fn settled_arrival_is_reported_once() {
        let presets = [preset(PresetKind::Tile, 1, 1000, 1000)];
        let mut s = Session::default();
        assert_eq!(
            s.apply(&mut snap(1, 0xa000, 1)).arrival,
            None,
            "no arrival at start"
        );
        // Stale first frame: still settling, nothing reported yet.
        assert_eq!(
            s.apply(&mut at(snap(1, 0xa000, 29), 5, 5, &presets)).arrival,
            None
        );
        let settled = s.apply(&mut at(snap(1, 0xa000, 29), 1003, 1002, &presets));
        assert_eq!(settled.arrival, Some((29, 1003, 1002)));
        assert_eq!(
            s.apply(&mut at(snap(1, 0xa000, 29), 1100, 1100, &presets))
                .arrival,
            None
        );

        // No tile near (waypoint): reported once the settle window runs out.
        s.apply(&mut snap(1, 0xa000, 1));
        let reports: Vec<_> = (0..=ARRIVAL_SETTLE + 2)
            .filter_map(|_| s.apply(&mut at(snap(1, 0xa000, 28), 200, 200, &[])).arrival)
            .collect();
        assert_eq!(reports, vec![(28, 200, 200)]);
    }

    #[test]
    fn game_change_resets_arrivals() {
        let presets = [
            preset(PresetKind::Tile, 1, 100, 100),
            preset(PresetKind::Tile, 2, 300, 300),
        ];
        let mut s = Session::default();
        s.apply(&mut snap(1, 0xa000, 1));
        let mut a = at(snap(1, 0xa000, 2), 101, 100, &presets);
        s.apply(&mut a);
        assert_eq!(tiles(&a), vec![(1, true), (2, false)]);

        // New game, same level id: the new arrival (next to tile 2) decides.
        let mut b = at(snap(2, 0xb000, 1), 30, 30, &[]);
        assert!(s.apply(&mut b).new_game);
        let mut c = at(snap(2, 0xb000, 2), 299, 300, &presets);
        s.apply(&mut c);
        assert_eq!(tiles(&c), vec![(1, false), (2, true)]);
    }
    #[test]
    fn a_revisited_level_shows_what_was_seen_there() {
        let mut s = Session::default();
        let mut a = snap(1, 0xa000, 2);
        a.presets = vec![
            preset(PresetKind::Object, 119, 100, 100), // waypoint
            preset(PresetKind::Tile, 5, 300, 300),
            preset(PresetKind::Monster, 840, 200, 200), // generic: remembered, still filtered
        ];
        s.apply(&mut a);
        let mut b = snap(1, 0xa000, 3);
        b.presets = vec![preset(PresetKind::Tile, 6, 50, 50)];
        s.apply(&mut b);
        assert_eq!(b.presets.len(), 1, "level 3 doesn't get level 2's presets");

        let mut back = snap(1, 0xa000, 2);
        s.apply(&mut back);
        let kept: Vec<(u32, u32)> = back.presets.iter().map(|p| (p.txt, p.x)).collect();
        assert_eq!(kept, vec![(119, 100), (5, 300)]);
    }

    #[test]
    fn remembered_and_live_presets_are_not_duplicated() {
        let presets = || {
            vec![
                preset(PresetKind::Tile, 1, 100, 100),
                preset(PresetKind::Tile, 2, 400, 400),
                preset(PresetKind::Object, 119, 100, 100), // same spot, other kind: its own
            ]
        };
        let mut s = Session::default();
        s.apply(&mut snap(1, 0xa000, 1));
        // Arriving next to tile 1 marks it the entry; the remembered copy stays raw.
        let mut a = at(snap(1, 0xa000, 2), 101, 100, &presets());
        s.apply(&mut a);
        let mut b = at(
            snap(1, 0xa000, 2),
            101,
            100,
            &[preset(PresetKind::Tile, 1, 100, 100)],
        );
        s.apply(&mut b);
        assert_eq!(b.presets.len(), 3, "live tile 1 + remembered tile 2 and waypoint");
        assert_eq!(tiles(&b), vec![(1, true), (2, false)]);
        let mut c = at(snap(1, 0xa000, 2), 101, 100, &presets());
        s.apply(&mut c);
        assert_eq!(c.presets, a.presets, "all live again: nothing added twice");
    }

    #[test]
    fn a_kill_survives_act_travel() {
        let countess = || preset(PresetKind::Monster, 757, 100, 100);
        let mut s = Session::default();
        let mut killed = snap(1, 0xa000, 25);
        killed.presets = vec![countess()];
        killed.monsters = vec![monster(0x02, 0, 101, 100)];
        s.apply(&mut killed);
        assert_eq!(killed.presets, vec![]);

        s.apply(&mut snap(1, 0xb000, 40)); // act 2
        let mut back = snap(1, 0xc000, 25); // act 1 again (a new act address)
        back.presets = vec![countess()];
        assert!(!s.apply(&mut back).new_game);
        assert_eq!(back.presets, vec![], "still killed, live preset");
        let mut unloaded = snap(1, 0xc000, 25);
        s.apply(&mut unloaded);
        assert_eq!(unloaded.presets, vec![], "still killed, remembered preset");
    }

    fn shrine(used: bool, x: u32) -> Shrine {
        Shrine {
            txt: 2,
            code: 12,
            used,
            x,
            y: 100,
        }
    }

    #[test]
    fn shrines_and_wells_are_remembered_with_their_latest_state() {
        let well = Well {
            txt: 130,
            x: 50,
            y: 50,
        };
        let mut s = Session::default();
        let mut a = snap(1, 0xa000, 4);
        a.shrines = vec![shrine(false, 100), shrine(false, 200)];
        a.wells = vec![well.clone()];
        s.apply(&mut a);
        let mut used = snap(1, 0xa000, 4);
        used.shrines = vec![shrine(true, 100)];
        s.apply(&mut used);
        assert_eq!(used.shrines, vec![shrine(true, 100), shrine(false, 200)]);
        assert_eq!(used.wells, vec![well.clone()]);

        s.apply(&mut snap(1, 0xa000, 5));
        let mut back = snap(1, 0xa000, 4);
        s.apply(&mut back);
        assert_eq!(back.shrines, vec![shrine(true, 100), shrine(false, 200)]);
        assert_eq!(back.wells, vec![well]);
        // A new game forgets them.
        let mut next = snap(2, 0xa000, 4);
        s.apply(&mut next);
        assert_eq!((next.shrines.len(), next.wells.len()), (0, 0));
    }

    #[test]
    fn take_changed_reports_each_change_once() {
        let mut s = Session::default();
        assert!(s.take_changed().is_none(), "nothing before the first snapshot");
        let mut a = snap(1, 0xa000, 2);
        a.presets = vec![preset(PresetKind::Tile, 1, 10, 10)];
        s.apply(&mut a);
        assert_eq!(s.take_changed().map(|g| g.player_id), Some(1));
        assert!(s.take_changed().is_none());
        let mut same = snap(1, 0xa000, 2);
        same.presets = vec![preset(PresetKind::Tile, 1, 10, 10)];
        s.apply(&mut same);
        assert!(s.take_changed().is_none(), "nothing new");
        let mut more = snap(1, 0xa000, 2);
        more.presets = vec![preset(PresetKind::Tile, 2, 20, 20)];
        s.apply(&mut more);
        assert!(s.take_changed().is_some(), "a new preset");
    }

    /// A session that saw a killed Countess and a tile in level 25 for player 1.
    fn saved_state() -> GameState {
        let mut s = Session::default();
        let mut a = snap(1, 0xa000, 25);
        a.presets = vec![
            preset(PresetKind::Monster, 757, 100, 100),
            preset(PresetKind::Tile, 3, 300, 300),
        ];
        a.monsters = vec![monster(0x02, 0, 101, 100)];
        s.apply(&mut a);
        s.take_changed().cloned().unwrap_or_default()
    }

    #[test]
    fn a_saved_state_needs_its_player_id() {
        // A truncated or foreign file must not be restored as player id 0.
        assert!(serde_json::from_str::<GameState>("{}").is_err());
        let state: GameState = serde_json::from_str(r#"{"player_id": 7}"#).unwrap_or_default();
        assert_eq!(state.player_id, 7, "collections default when missing");
    }

    #[test]
    fn a_restored_state_is_used_only_for_the_same_player_id() {
        let mut s = Session::default();
        s.restore(saved_state());
        let mut same = snap(1, 0xb000, 25);
        same.presets = vec![preset(PresetKind::Monster, 757, 100, 100)];
        assert!(!s.apply(&mut same).new_game);
        let kept: Vec<u32> = same.presets.iter().map(|p| p.txt).collect();
        assert_eq!(kept, vec![3], "kill and remembered tile restored");
        assert!(
            s.take_changed().is_none(),
            "adopting the saved state is no change"
        );

        let mut s = Session::default();
        s.restore(saved_state());
        let mut other = snap(2, 0xb000, 25);
        other.presets = vec![preset(PresetKind::Monster, 757, 100, 100)];
        assert!(s.apply(&mut other).new_game, "the saved game was another");
        let kept: Vec<u32> = other.presets.iter().map(|p| p.txt).collect();
        assert_eq!(kept, vec![757]);
        assert_eq!(s.take_changed().map(|g| g.player_id), Some(2));
    }

    fn cow(id: u32, mode: u32) -> Monster {
        Monster {
            id,
            ..with_txt(391, mode, 10, 10)
        }
    }

    fn farm(player_id: u32, monsters: Vec<Monster>) -> Snapshot {
        let mut s = snap(player_id, 0xa000, crate::cows::COW_LEVEL);
        s.monsters = monsters;
        s
    }

    #[test]
    fn a_cow_counts_once_per_unit_id_and_only_dead() {
        let mut s = Session::default();
        let mut first = farm(1, vec![cow(1, 1), cow(2, 0), cow(3, 12)]);
        let update = s.apply(&mut first);
        assert_eq!(update.new_cows, 2, "modes 0 and 12 are dead, 1 is not");
        assert_eq!(first.cows, Some(crate::cows::Tally { kills: 2, alive: 1 }));
        // The same corpses again, and cow 1 dies: only cow 1 is new.
        let mut b = farm(1, vec![cow(1, 0), cow(2, 0), cow(3, 12)]);
        assert_eq!(s.apply(&mut b).new_cows, 1);
        assert_eq!(b.cows, Some(crate::cows::Tally { kills: 3, alive: 0 }));
        // A dead non-cow doesn't count.
        let mut c = farm(1, vec![with_txt(410, 0, 1, 1)]);
        assert_eq!(s.apply(&mut c).new_cows, 0);
        assert_eq!(s.cow_kills(), 3);
    }

    #[test]
    fn cows_count_in_level_39_only() {
        let mut s = Session::default();
        let mut elsewhere = snap(1, 0xa000, 4);
        elsewhere.monsters = vec![cow(1, 0)];
        let u = s.apply(&mut elsewhere);
        assert_eq!((u.new_cows, u.cow_run), (0, false));
        assert_eq!(elsewhere.cows, None, "no counter outside the farm");
        assert_eq!(s.cow_kills(), 0);
    }

    #[test]
    fn a_new_game_resets_the_run_and_starts_a_new_one() {
        let mut s = Session::default();
        assert!(s.apply(&mut farm(1, vec![cow(1, 0)])).cow_run, "first arrival");
        assert!(!s.apply(&mut farm(1, vec![])).cow_run, "once per game");
        s.apply(&mut snap(1, 0xa000, 1));
        assert!(
            !s.apply(&mut farm(1, vec![])).cow_run,
            "back in the farm, same game"
        );
        assert_eq!(s.cow_kills(), 1);

        let u = s.apply(&mut farm(2, vec![cow(5, 1)]));
        assert!(u.new_game && u.cow_run, "a new game is a new run");
        assert_eq!(s.cow_kills(), 0, "the count starts over");
    }

    #[test]
    fn counted_cows_survive_a_restore() -> Result<(), serde_json::Error> {
        let mut s = Session::default();
        s.apply(&mut farm(1, vec![cow(1, 0)]));
        let saved: GameState = serde_json::from_str(&serde_json::to_string(&s.state)?)?;
        let mut again = Session::default();
        again.restore(saved);
        let u = again.apply(&mut farm(1, vec![cow(1, 0)]));
        assert_eq!(
            (u.cow_run, u.new_cows),
            (false, 0),
            "neither the run nor the cow again"
        );
        assert_eq!(again.cow_kills(), 1);
        Ok(())
    }

    fn farm_at(player_id: u32, level: u32, x: u16, y: u16) -> Snapshot {
        let mut s = snap(player_id, 0xa000, level);
        s.player.x = x;
        s.player.y = y;
        s
    }

    #[test]
    fn the_trail_keeps_points_at_least_4_tiles_apart() {
        let mut s = Session::default();
        let mut last = farm_at(1, crate::cows::COW_LEVEL, 100, 50);
        for x in [100, 101, 103, 104, 107, 112] {
            last = farm_at(1, crate::cows::COW_LEVEL, x, 50);
            s.apply(&mut last);
        }
        assert_eq!(
            last.trail,
            vec![(100, 50), (104, 50), (112, 50)],
            "107 is 3 from 104"
        );
        // A loading frame at 0,0 is no point.
        let mut zero = farm_at(1, crate::cows::COW_LEVEL, 0, 0);
        s.apply(&mut zero);
        assert_eq!(zero.trail.len(), 3);
    }

    #[test]
    fn the_trail_is_capped_dropping_the_oldest() {
        let mut s = Session::default();
        let mut last = farm_at(1, crate::cows::COW_LEVEL, 0, 1);
        for i in 0..=TRAIL_CAP {
            let x = u16::try_from(i % 1000 * 5).unwrap_or(0);
            let y = u16::try_from(i / 1000 * 5 + 1).unwrap_or(0);
            last = farm_at(1, crate::cows::COW_LEVEL, x, y);
            s.apply(&mut last);
        }
        assert_eq!(last.trail.len(), TRAIL_CAP);
        assert_eq!(last.trail.first(), Some(&(5, 1)), "the first point went");
    }

    #[test]
    fn the_trail_is_kept_per_level_and_per_game() {
        let mut s = Session::default();
        let mut elsewhere = farm_at(1, 4, 100, 100);
        s.apply(&mut elsewhere);
        assert_eq!(elsewhere.trail, vec![(100, 100)], "every level has a trail");
        s.apply(&mut farm_at(1, crate::cows::COW_LEVEL, 100, 100));
        s.apply(&mut farm_at(1, 1, 200, 200));
        let mut back = farm_at(1, crate::cows::COW_LEVEL, 100, 100);
        s.apply(&mut back);
        assert_eq!(
            back.trail,
            vec![(100, 100)],
            "a town visit adds nothing to the farm's trail"
        );
        let mut stony = farm_at(1, 4, 140, 100);
        s.apply(&mut stony);
        assert_eq!(
            stony.trail,
            vec![(100, 100), (140, 100)],
            "level 4 kept its own trail"
        );
        let mut next = farm_at(2, crate::cows::COW_LEVEL, 300, 300);
        s.apply(&mut next);
        assert_eq!(next.trail, vec![(300, 300)], "a new game starts a new trail");
    }

    #[test]
    fn stash_merge_keeps_cached_tabs_over_empty_or_fallback_reads() {
        use crate::stash::{StashItem, StashTab, TabKind};
        let item = |name: &str| StashItem {
            name: name.to_owned(),
            quality: 5,
            identified: true,
            ethereal: false,
            runeword: false,
            code: None,
            quantity: None,
        };
        let tab = |kind, items: Vec<StashItem>| StashTab { kind, items };
        let cached = vec![
            tab(TabKind::Page(1), vec![item("A")]),
            tab(TabKind::Page(5), vec![item("Sigon")]),
            tab(TabKind::Materials, vec![item("El")]),
        ];
        // Page 1's unit not loaded (empty), page 5 changed: page 1 keeps its cached items.
        let live = vec![
            tab(TabKind::Page(1), vec![]),
            tab(TabKind::Page(5), vec![item("New")]),
            tab(TabKind::Materials, vec![item("El")]),
        ];
        let merged = super::merge_stash(Some(&cached), live);
        let names: Vec<_> = merged
            .iter()
            .map(|t| t.items.first().map_or("", |i| i.name.as_str()))
            .collect();
        assert_eq!(names, ["A", "New", "El"]);
        // A fallback read (no materials tab) never replaces a list-ordered cache.
        let fallback = vec![tab(TabKind::Page(1), vec![item("Sigon")])];
        assert_eq!(super::merge_stash(Some(&cached), fallback), cached);
        // Without a cache the live read is taken as is.
        let live = vec![tab(TabKind::Page(1), vec![])];
        assert_eq!(super::merge_stash(None, live.clone()), live);
    }
}
