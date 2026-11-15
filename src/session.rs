//! State accumulated over snapshots: superuniques killed in this game and where the player
//! arrived in each level.
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

use std::collections::{HashMap, HashSet};

use crate::game::{Addr, Monster, PresetKind, Snapshot};
use crate::levels_data::SUPERUNIQUES;
use crate::offsets::monster_data;
use crate::render::{is_consumable_quest_object, quest_monster};

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
    /// The arrival point settled in this snapshot: (level id, x, y). Once per level per game.
    pub arrival: Option<(u32, u16, u16)>,
}

#[derive(Debug, Default)]
pub struct Session {
    game: Option<GameKey>,
    /// Superunique and quest monster presets whose monster was seen dead in this game, by
    /// (level id, preset x, y).
    killed: HashSet<LevelPos>,
    /// Superunique preset each superunique monster (by unit id) was first seen alive near in
    /// this game. Its death kills that preset however far it was chased.
    bound: HashMap<u32, LevelPos>,
    /// Consumable quest objects (Diablo seals) seen operated in this game, by (level id, live
    /// x, y). Their presets are hidden for the game, also once the live object unloads.
    operated: HashSet<LevelPos>,
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
    /// Folds `snap` into the session and rewrites it for drawing: monster presets are kept only
    /// if they are a superunique's (see [`superunique`]) or a quest monster's (see
    /// [`quest_monster`]) not killed in this game, and consumable
    /// quest object presets only if not seen operated in this game.
    pub fn apply(&mut self, snap: &mut Snapshot) -> Update {
        let mut update = Update::default();
        let key = GameKey {
            player_id: snap.player.id,
            act: snap.player.act,
        };
        if self.game.replace(key).is_some_and(|old| old != key) {
            self.killed.clear();
            self.bound.clear();
            self.operated.clear();
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
            let was_settling = self.settling.is_some_and(|(l, _)| l == level);
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
            if was_settling && self.settling.is_none() {
                update.arrival = self.arrivals.get(&level).map(|&(x, y)| (level, x, y));
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
                    self.killed.insert(pos);
                }
            } else if !self.bound.contains_key(&m.id)
                && let Some(pos) = nearest_preset(m, BIND_RADIUS)
            {
                self.bound.insert(m.id, pos);
            }
        }

        self.kill_quest_monsters(snap);

        for o in &snap.operated_objects {
            self.operated.insert((level, o.x, o.y));
        }
        let operated_near = |x: u32, y: u32| {
            self.operated.iter().any(|&(l, ox, oy)| {
                l == level
                    && (f64::from(x) - f64::from(ox)).hypot(f64::from(y) - f64::from(oy)) <= OPERATED_RADIUS
            })
        };

        snap.presets.retain(|p| match p.kind {
            PresetKind::Monster => {
                (superunique(p.txt).is_some() || quest_monster(p.txt).is_some())
                    && !self.killed.contains(&(level, p.x, p.y))
            }
            PresetKind::Object => !(is_consumable_quest_object(p.txt) && operated_near(p.x, p.y)),
            _ => true,
        });
        update
    }
}

impl Session {
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
                self.killed.insert((level, p.x, p.y));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Level, OperatedObject, Player, Preset};

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
            shrines: vec![],
            wells: vec![],
            operated_objects: vec![],
            inventory_items: 0,
            inventory_keep: vec![],
            difficulty: None,
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
}
