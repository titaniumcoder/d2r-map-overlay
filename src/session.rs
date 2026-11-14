//! State accumulated over snapshots: superuniques killed in this game, and the (persistent) set
//! of superunique preset ids.
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

use std::collections::{BTreeSet, HashSet};

use crate::game::{Addr, PresetKind, Snapshot};
use crate::offsets::monster_data;

/// Most distance in tiles between a superunique and the monster preset it spawned from.
const LEARN_RADIUS: f64 = 6.0;

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
}

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
            update.new_game = true;
        }
        let level = snap.level.id;

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
        Preset { kind, txt, x, y }
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
            items: vec![],
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
}
