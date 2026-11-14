//! State accumulated over snapshots: superuniques killed in this game, where the player arrived
//! in each level, and the (persistent) set of superunique preset ids.
//!
//! Pure: no I/O. [`Session::apply`] folds each snapshot in and rewrites it for drawing, so the
//! renderer stays a function of the snapshot alone.
//!
//! **Game change**: the per-game part is cleared when the player's (unit id, act pointer) pair
//! changes. The server hands out a new player unit id per game (live ids are large and
//! random-looking), and a new game allocates a new act; either alone could repeat by chance
//! (offline ids, or the allocator reusing the act's address), both together practically never.
//! The act also changes on act travel, which clears the killed set then too: a superunique
//! killed in an act you return to shows again. That errs towards showing a marker, never
//! towards hiding a living superunique in a new game.
//!
//! **Entry**: the player's position when the level id changes is the level's arrival point (first
//! visit per game wins; a new game counts as a level change). For [`ARRIVAL_SETTLE`] snapshots it
//! follows the player while no tile is near it, so a stale first frame can't lock a wrong point.
//! The tile preset nearest to it, within [`ENTRY_RADIUS`], is the way in and gets
//! [`Preset::entry`](crate::game::Preset::entry); the other tiles stay exits. Arriving by waypoint or portal has no tile nearby, so no entry. The
//! very first snapshot of a session has no previous level (the overlay may start mid-level),
//! so it records no arrival.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::game::{Addr, PresetKind, Snapshot};
use crate::offsets::monster_data;

/// Most distance in tiles between a superunique and the monster preset it spawned from.
const LEARN_RADIUS: f64 = 6.0;
/// Most distance in tiles between the arrival point and the entry tile preset.
const ENTRY_RADIUS: f64 = 25.0;
/// Tile presets this close to the entry tile are part of the same entry (multi-tile stairs).
const ENTRY_CLUSTER: f64 = 5.0;

/// (level id, x, y) in world tiles of a preset.
type LevelPos = (u32, u32, u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GameKey {
    player_id: u32,
    act: Option<Addr>,
}

/// What [`Session::apply`] found besides rewriting the snapshot.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Update {
    /// The game changed since the previous snapshot; the per-game state was cleared.
    pub new_game: bool,
    /// Preset txt ids learned as superuniques by this snapshot (to persist).
    pub learned: Vec<u32>,
}

#[derive(Debug, Default)]
pub struct Session {
    /// Monster preset txt ids known to be superuniques (kept across games).
    pub superuniques: BTreeSet<u32>,
    game: Option<GameKey>,
    /// Superunique presets whose monster was seen dead in this game, by (level id, preset x, y).
    killed: HashSet<LevelPos>,
    /// Level id of the previous snapshot (kept across games: only its change matters).
    last_level: Option<u32>,
    /// Player position on arriving in each level of this game, by level id.
    arrivals: HashMap<u32, (u16, u16)>,
    /// Level whose arrival point may still move, and how many more snapshots it may.
    settling: Option<(u32, u8)>,
}

/// Snapshots (~1 s at 5 Hz) after an arrival during which the arrival point follows the player
/// while no tile lies near it: the first snapshot after a level change can still carry the old
/// level's position, or 0,0 mid-load.
const ARRIVAL_SETTLE: u8 = 5;

impl Session {
    pub fn new(superuniques: BTreeSet<u32>) -> Self {
        Self {
            superuniques,
            ..Self::default()
        }
    }

    /// Folds `snap` into the session and rewrites it for drawing: monster presets are kept only
    /// if their txt id is a known superunique not killed in this game.
    pub fn apply(&mut self, snap: &mut Snapshot) -> Update {
        let mut update = Update::default();
        let key = GameKey {
            player_id: snap.player.id,
            act: snap.player.act,
        };
        if self.game.replace(key).is_some_and(|old| old != key) {
            self.killed.clear();
            self.arrivals.clear();
            update.new_game = true;
        }
        let level = snap.level.id;
        let level_changed = self.last_level.replace(level).is_some_and(|old| old != level);
        let here = (snap.player.x, snap.player.y);
        if (level_changed || update.new_game) && !self.arrivals.contains_key(&level) {
            self.arrivals.insert(level, here);
            self.settling = Some((level, ARRIVAL_SETTLE));
        }
        if let Some(&arrival) = self.arrivals.get(&level) {
            let found = mark_entry(snap, arrival);
            self.settling = match self.settling {
                Some((l, left)) if l == level && !found && left > 0 => {
                    self.arrivals.insert(level, here);
                    if mark_entry(snap, here) {
                        None
                    } else {
                        Some((l, left - 1))
                    }
                }
                Some((l, _)) if l == level => None,
                other => other,
            };
        }

        for m in snap
            .monsters
            .iter()
            .filter(|m| m.flags & monster_data::SUPERUNIQUE != 0)
        {
            let (mx, my) = (f64::from(m.x), f64::from(m.y));
            // The nearest monster preset only: other fixed spawns nearby must not be learned.
            let nearest = snap
                .presets
                .iter()
                .filter(|p| p.kind == PresetKind::Monster)
                .map(|p| ((f64::from(p.x) - mx).hypot(f64::from(p.y) - my), p))
                .filter(|&(d, _)| d <= LEARN_RADIUS)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let Some((_, p)) = nearest else { continue };
            if self.superuniques.insert(p.txt) {
                update.learned.push(p.txt);
            }
            if m.is_dead() {
                self.killed.insert((level, p.x, p.y));
            }
        }

        snap.presets.retain(|p| {
            p.kind != PresetKind::Monster
                || (self.superuniques.contains(&p.txt) && !self.killed.contains(&(level, p.x, p.y)))
        });
        update
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Level, Monster, Player, Preset};

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
            },
            player_candidates: vec![],
            level: Level {
                id: level,
                bounds: None,
                rooms: vec![],
            },
            presets: vec![],
            room_links: vec![],
            monsters: vec![],
            ground_items: 0,
            drops: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn game_change_needs_player_id_or_act() {
        let mut s = Session::default();
        assert!(
            !s.apply(&mut snap(7, 0xa000, 29)).new_game,
            "first game is not a change"
        );
        assert!(
            !s.apply(&mut snap(7, 0xa000, 35)).new_game,
            "level change in the same game"
        );
        assert!(s.apply(&mut snap(8, 0xa000, 29)).new_game, "new player id");
        assert!(s.apply(&mut snap(8, 0xb000, 29)).new_game, "new act");
        assert!(!s.apply(&mut snap(8, 0xb000, 29)).new_game);
    }

    #[test]
    fn superunique_presets_are_learned_from_nearby_live_superuniques() {
        let mut s = Session::default();
        let mut a = snap(1, 0xa000, 25);
        a.presets = vec![
            preset(PresetKind::Monster, 853, 100, 100),
            preset(PresetKind::Monster, 40, 108, 100),
            preset(PresetKind::Monster, 41, 300, 300),
            preset(PresetKind::Tile, 7, 100, 100),
        ];
        a.monsters = vec![
            monster(0x02, 1, 101, 101), // superunique next to 853 (40 is farther)
            monster(0x08, 1, 300, 300), // unique: teaches nothing
            monster(0x02, 1, 200, 200), // superunique with no preset within the learn radius
        ];
        let up = s.apply(&mut a);
        assert_eq!(up.learned, vec![853]);
        assert_eq!(s.superuniques, BTreeSet::from([853]));
        let kept: Vec<u32> = a.presets.iter().map(|p| p.txt).collect();
        assert_eq!(kept, vec![853, 7], "unknown monster presets dropped, tiles kept");

        // Seen again: nothing new to persist.
        let mut b = snap(1, 0xa000, 25);
        b.presets = vec![preset(PresetKind::Monster, 853, 100, 100)];
        b.monsters = vec![monster(0x02, 1, 100, 95)];
        assert_eq!(s.apply(&mut b).learned, Vec::<u32>::new());
        assert_eq!(b.presets.len(), 1);
    }

    #[test]
    fn killed_superunique_marker_is_removed_for_the_game() {
        let countess = || preset(PresetKind::Monster, 853, 100, 100);
        let mut session = Session::new(BTreeSet::from([853]));
        let mut alive = snap(1, 0xa000, 25);
        alive.presets = vec![countess()];
        session.apply(&mut alive);
        assert_eq!(alive.presets.len(), 1, "known id drawn without a live monster");

        for dead_mode in [0, 12] {
            let mut session = Session::new(BTreeSet::from([853]));
            let mut killed = snap(1, 0xa000, 25);
            killed.presets = vec![countess()];
            killed.monsters = vec![monster(0x02, dead_mode, 103, 98)];
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
            // A new game brings it back.
            let mut next_game = snap(2, 0xc000, 25);
            next_game.presets = vec![countess()];
            session.apply(&mut next_game);
            assert_eq!(next_game.presets.len(), 1);
        }
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
}
