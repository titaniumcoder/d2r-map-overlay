//! Research journal: what the overlay saw, as JSON Lines, for finding patterns offline (room
//! preset ids of special rooms, which objects sit in which piece, elite and drop spots).
//!
//! One file per UTC day (`YYYY-MM-DD.jsonl`, UTC because the local offset needs a time-zone
//! database), appended to. Every line is `{"t": unix secs, "game": key, "ev": kind, ...}`.
//! [`Recorder`] is the pure part: it turns snapshots into events and dedupes them per game, which
//! keeps the volume small. [`Journal`] adds the file: buffered, flushed once per call, never
//! synced. A write error is printed once and turns the journal off for the run; it never stops
//! the overlay.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::config::SHRINE_NAMES;
use crate::game::{Bounds, PresetKind, Room, Snapshot};
use crate::guidance::{Category, Tier};
use crate::guidance::{MarkKind, Marks};
use crate::items::DropKind;
use crate::session;

/// Monster flag bits of an elite: superunique 0x02, champion 0x04, unique 0x08.
const ELITE: u8 = 0x0E;

/// `$XDG_DATA_HOME/d2r-map-overlay` (else `~/.local/share/...`); `None` if neither is set.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    let absolute = |var| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = absolute("XDG_DATA_HOME").or_else(|| absolute("HOME").map(|h| h.join(".local/share")))?;
    Some(base.join("d2r-map-overlay"))
}

/// `journal` in the [`data_dir`]; `None` if there is none.
#[must_use]
pub fn default_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("journal"))
}

/// One journal event; the variant name is the line's `ev`.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "ev", rename_all = "lowercase")]
pub enum Event {
    /// A level's room layout: on first sight and whenever its room count is one not seen before.
    Level {
        level: u32,
        bounds: Option<Bounds>,
        /// Rooms grouped by preset id (`null`: unknown), ascending.
        pieces: Vec<Piece>,
        rooms: Vec<RoomRec>,
    },
    /// Where the player entered a level, once the arrival point settled.
    Arrival {
        level: u32,
        x: u16,
        y: u16,
        piece: Option<u32>,
    },
    /// A preset unit (tile, object or monster), before the superunique filter.
    Marker {
        level: u32,
        kind: PresetKind,
        txt: u32,
        x: u32,
        y: u32,
        piece: Option<u32>,
        /// The superunique's name, for a monster preset whose txt id is one (see
        /// `session::superunique`).
        #[serde(skip_serializing_if = "Option::is_none")]
        superunique: Option<&'static str>,
    },
    /// A room-tile link from the room at `x`,`y` to another level.
    Link {
        level: u32,
        x: u32,
        y: u32,
        from_piece: Option<u32>,
        to_level: u32,
    },
    /// A live elite monster on first sight.
    Monster {
        level: u32,
        id: u32,
        txt: u32,
        flags: u8,
        x: u16,
        y: u16,
        piece: Option<u32>,
        mode: u32,
    },
    /// A recorded elite first seen dead.
    Killed {
        level: u32,
        id: u32,
        txt: u32,
        x: u16,
        y: u16,
        piece: Option<u32>,
    },
    /// A shown ground item (unique, set, gem, rune, charm, marked base) on first sight, with
    /// its mark (`mark` keep / farm / trade, `null`: none; `tier` and `category` only for a
    /// trade mark; `rule` the valuables rule, the piece or "<runeword> base") and raw fields
    /// for checking unverified offsets.
    Drop {
        level: u32,
        id: u32,
        code: Option<String>,
        kind: DropKind,
        quality: u32,
        x: u32,
        y: u32,
        /// Journals written before marks v2 have no `mark` (their marks were all trade); a
        /// reader of old lines takes a missing one as `null` (`Event` is only written here).
        mark: Option<MarkKind>,
        tier: Option<Tier>,
        category: Option<Category>,
        rule: Option<String>,
        uncertain: bool,
        sockets: u8,
        ethereal: bool,
        flags: u32,
        unique_index: Option<u32>,
        /// Hex of the first `item_data::HEAD_LEN` bytes of `ItemData`.
        item_data_head: String,
    },
    /// A shrine (any type, used or not) on first sight at its spot.
    Shrine {
        level: u32,
        txt: u32,
        x: u32,
        y: u32,
        shrine_type: u8,
        /// `config::SHRINE_NAMES` of `shrine_type`.
        name: &'static str,
        used: bool,
    },
    /// A well on first sight at its spot.
    Well { level: u32, txt: u32, x: u32, y: u32 },
    /// The level was first seen terrorized (Terror Zone) in this game.
    Terror { level: u32, terrorized: bool },
    /// A monster whose `MonsterData` owner id matches the local player (merc/pet hypothesis,
    /// NOT yet live-verified — the raw owner fields decide whether the offset is right).
    Merc {
        level: u32,
        id: u32,
        txt: u32,
        owner_type: u32,
        owner_id: u32,
        x: u16,
        y: u16,
        mode: u32,
    },
    /// The shared-stash tab units were first seen live in a game (`captured`, with the tab
    /// and item counts) or the session's stash cache was dropped (a new game; counts 0).
    /// Answers whether the stash must be opened before the tab units exist (research note
    /// `stash-format.md` §3.4).
    Stash { captured: bool, tabs: u32, items: u32 },
}

/// Rooms sharing a preset id: how many and their bounding box (world tiles, max exclusive).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Piece {
    pub preset: Option<u32>,
    pub count: u32,
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RoomRec {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub preset: Option<u32>,
}

/// A journal line: the event with its time and game.
#[derive(Debug, Serialize)]
struct Line<'a> {
    t: u64,
    game: &'a str,
    #[serde(flatten)]
    ev: &'a Event,
}

/// Preset id of the room containing world tile `x`,`y`; `None` if no room does or it has none.
#[must_use]
pub fn piece_at(rooms: &[Room], x: u32, y: u32) -> Option<u32> {
    rooms
        .iter()
        .find(|r| (r.x..r.x.saturating_add(r.w)).contains(&x) && (r.y..r.y.saturating_add(r.h)).contains(&y))
        .and_then(|r| r.preset)
}

/// The game key Session uses: player unit id and act address, in hex.
fn game_key(snap: &Snapshot) -> String {
    let id = snap.player.id;
    snap.player
        .act
        .map_or_else(|| format!("{id:08x}-none"), |act| format!("{id:08x}-{:x}", act.0))
}

/// Turns snapshots into events, each emitted once per game.
#[derive(Debug, Default)]
pub struct Recorder {
    game: String,
    /// (level id, room count) already written.
    levels: HashSet<(u32, usize)>,
    /// (level id, kind, txt, x, y).
    markers: HashSet<(u32, PresetKind, u32, u32, u32)>,
    /// (level id, from room x, y, to level id).
    links: HashSet<(u32, u32, u32, u32)>,
    /// Elite unit id → seen dead.
    monsters: HashMap<u32, bool>,
    drops: HashSet<u32>,
    /// Unit ids seen as the local player's merc/pet.
    mercs: HashSet<u32>,
    /// (level id, x, y) of shrines and of wells.
    shrines: HashSet<(u32, u32, u32)>,
    wells: HashSet<(u32, u32, u32)>,
    /// Levels seen terrorized.
    terror: HashSet<u32>,
}

impl Recorder {
    /// The key of the current game (empty before the first snapshot).
    #[must_use]
    pub fn game(&self) -> &str {
        &self.game
    }

    /// The new events in `snap`, each drop with its mark from `marks`. Feed it the snapshot as
    /// read, before `Session::apply` filters the monster presets. A new game clears what was
    /// seen.
    pub fn observe(&mut self, snap: &Snapshot, marks: &Marks) -> Vec<Event> {
        let key = game_key(snap);
        if key != self.game {
            *self = Self {
                game: key,
                ..Self::default()
            };
        }
        let level = snap.level.id;
        let rooms = &snap.level.rooms;
        let piece = |x: u32, y: u32| piece_at(rooms, x, y);
        let mut out = Vec::new();

        self.level(snap, &mut out);
        for p in &snap.presets {
            if !matches!(
                p.kind,
                PresetKind::Tile | PresetKind::Object | PresetKind::Monster
            ) {
                continue;
            }
            if self.markers.insert((level, p.kind, p.txt, p.x, p.y)) {
                out.push(Event::Marker {
                    level,
                    kind: p.kind,
                    txt: p.txt,
                    x: p.x,
                    y: p.y,
                    piece: piece(p.x, p.y),
                    superunique: match p.kind {
                        PresetKind::Monster => session::superunique(p.txt),
                        _ => None,
                    },
                });
            }
        }
        for l in &snap.room_links {
            let (x, y) = (l.from_room.x, l.from_room.y);
            if self.links.insert((level, x, y, l.to_level_id)) {
                out.push(Event::Link {
                    level,
                    x,
                    y,
                    from_piece: rooms.iter().find(|r| r.x == x && r.y == y).and_then(|r| r.preset),
                    to_level: l.to_level_id,
                });
            }
        }
        for m in snap.monsters.iter().filter(|m| m.flags & ELITE != 0) {
            let at = piece(u32::from(m.x), u32::from(m.y));
            let dead = self.monsters.entry(m.id).or_insert_with(|| {
                out.push(Event::Monster {
                    level,
                    id: m.id,
                    txt: m.txt,
                    flags: m.flags,
                    x: m.x,
                    y: m.y,
                    piece: at,
                    mode: m.mode,
                });
                false
            });
            if m.is_dead() && !*dead {
                *dead = true;
                out.push(Event::Killed {
                    level,
                    id: m.id,
                    txt: m.txt,
                    x: m.x,
                    y: m.y,
                    piece: at,
                });
            }
        }
        self.mercs_seen(snap, level, &mut out);
        for d in &snap.drops {
            if self.drops.insert(d.id) {
                let h = marks.get(&d.id);
                out.push(Event::Drop {
                    level,
                    id: d.id,
                    code: d.code.clone(),
                    kind: d.kind,
                    quality: d.quality,
                    x: d.x,
                    y: d.y,
                    mark: h.map(|h| h.kind),
                    tier: h.and_then(|h| h.trade).map(|(tier, _)| tier),
                    category: h.and_then(|h| h.trade).map(|(_, category)| category),
                    rule: h.map(|h| h.rule.clone()),
                    uncertain: h.is_some_and(|h| h.uncertain),
                    sockets: d.sockets,
                    ethereal: d.ethereal,
                    flags: d.flags,
                    unique_index: d.unique_index,
                    item_data_head: d.item_data_head.clone(),
                });
            }
        }
        self.objects(snap, &mut out);
        out
    }

    /// The level event of `snap` (once per room count) and its terror event (once per level).
    fn level(&mut self, snap: &Snapshot, out: &mut Vec<Event>) {
        let (level, rooms) = (snap.level.id, &snap.level.rooms);
        if !rooms.is_empty() && self.levels.insert((level, rooms.len())) {
            out.push(level_event(level, snap.level.bounds, rooms));
        }
        if snap.terrorized && self.terror.insert(level) {
            out.push(Event::Terror {
                level,
                terrorized: true,
            });
        }
    }

    /// The shrine and well events of `snap`, once per spot.
    fn objects(&mut self, snap: &Snapshot, out: &mut Vec<Event>) {
        let level = snap.level.id;
        for sh in &snap.shrines {
            if self.shrines.insert((level, sh.x, sh.y)) {
                out.push(Event::Shrine {
                    level,
                    txt: sh.txt,
                    x: sh.x,
                    y: sh.y,
                    shrine_type: sh.code,
                    name: SHRINE_NAMES.get(usize::from(sh.code)).copied().unwrap_or("?"),
                    used: sh.used,
                });
            }
        }
        for w in &snap.wells {
            if self.wells.insert((level, w.x, w.y)) {
                out.push(Event::Well {
                    level,
                    txt: w.txt,
                    x: w.x,
                    y: w.y,
                });
            }
        }
    }

    /// First sight of the local player's merc/pet (`MonsterData` owner id, unverified
    /// hypothesis): emits the raw owner fields so the offset can be verified live.
    fn mercs_seen(&mut self, snap: &Snapshot, level: u32, out: &mut Vec<Event>) {
        for m in snap.monsters.iter().filter(|m| m.owner_id == snap.player.id) {
            if !m.is_dead() && self.mercs.insert(m.id) {
                out.push(Event::Merc {
                    level,
                    id: m.id,
                    txt: m.txt,
                    owner_type: m.owner_type,
                    owner_id: m.owner_id,
                    x: m.x,
                    y: m.y,
                    mode: m.mode,
                });
            }
        }
    }
}

/// The arrival event for a settled arrival `(level, x, y)` from `Session::apply`.
#[must_use]
pub fn arrival_event(snap: &Snapshot, (level, x, y): (u32, u16, u16)) -> Event {
    Event::Arrival {
        level,
        x,
        y,
        piece: piece_at(&snap.level.rooms, u32::from(x), u32::from(y)),
    }
}

fn level_event(level: u32, bounds: Option<Bounds>, rooms: &[Room]) -> Event {
    let mut groups: BTreeMap<Option<u32>, Piece> = BTreeMap::new();
    for r in rooms {
        let (x1, y1) = (r.x.saturating_add(r.w), r.y.saturating_add(r.h));
        groups
            .entry(r.preset)
            .and_modify(|g| {
                g.count = g.count.saturating_add(1);
                g.min_x = g.min_x.min(r.x);
                g.min_y = g.min_y.min(r.y);
                g.max_x = g.max_x.max(x1);
                g.max_y = g.max_y.max(y1);
            })
            .or_insert(Piece {
                preset: r.preset,
                count: 1,
                min_x: r.x,
                min_y: r.y,
                max_x: x1,
                max_y: y1,
            });
    }
    Event::Level {
        level,
        bounds,
        pieces: groups.into_values().collect(),
        rooms: rooms
            .iter()
            .map(|r| RoomRec {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                preset: r.preset,
            })
            .collect(),
    }
}

/// The open day file.
#[derive(Debug)]
struct DayFile {
    date: String,
    out: BufWriter<File>,
}

/// The recorder plus its file. Disabled ([`Journal::disabled`], or after a write error), it does
/// nothing.
#[derive(Debug)]
pub struct Journal {
    rec: Recorder,
    /// `None`: disabled.
    dir: Option<PathBuf>,
    file: Option<DayFile>,
}

impl Journal {
    /// A journal writing day files into `dir` (created on the first write).
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            rec: Recorder::default(),
            dir: Some(dir),
            file: None,
        }
    }

    #[must_use]
    pub fn disabled() -> Self {
        Self {
            rec: Recorder::default(),
            dir: None,
            file: None,
        }
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// Records the new events of `snap` (as read, before `Session::apply`), its drops with their
    /// `marks`.
    pub fn observe(&mut self, snap: &Snapshot, marks: &Marks) {
        if self.is_enabled() {
            let events = self.rec.observe(snap, marks);
            self.write(&events, unix_now());
        }
    }

    /// Records a settled arrival reported by `Session::apply` for `snap`.
    pub fn arrival(&mut self, snap: &Snapshot, arrival: (u32, u16, u16)) {
        if self.is_enabled() {
            self.write(&[arrival_event(snap, arrival)], unix_now());
        }
    }

    /// Records a shared-stash capture or invalidation reported by `Session::apply`.
    pub fn stash(&mut self, captured: bool, tabs: u32, items: u32) {
        if self.is_enabled() {
            self.write(
                &[Event::Stash {
                    captured,
                    tabs,
                    items,
                }],
                unix_now(),
            );
        }
    }

    /// Appends `events` at time `t` and flushes; the first error disables the journal.
    fn write(&mut self, events: &[Event], t: u64) {
        if events.is_empty() {
            return;
        }
        if let Err(e) = self.try_write(events, t) {
            if let Some(dir) = self.dir.take() {
                eprintln!(
                    "journal: writing to {} failed, journal off for this run: {e}",
                    dir.display()
                );
            }
            self.file = None;
        }
    }

    fn try_write(&mut self, events: &[Event], t: u64) -> io::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let date = utc_date(t);
        let file = match &mut self.file {
            Some(f) if f.date == date => f,
            slot => {
                if let Some(mut old) = slot.take() {
                    old.out.flush()?;
                }
                slot.insert(open_day(dir, date)?)
            }
        };
        for ev in events {
            let line = Line {
                t,
                game: self.rec.game(),
                ev,
            };
            serde_json::to_writer(&mut file.out, &line)?;
            file.out.write_all(b"\n")?;
        }
        file.out.flush()
    }
}

fn open_day(dir: &Path, date: String) -> io::Result<DayFile> {
    fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{date}.jsonl")))?;
    Ok(DayFile {
        date,
        out: BufWriter::new(file),
    })
}

fn unix_now() -> u64 {
    // A clock before 1970 is a broken clock: write 0 rather than fail.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `YYYY-MM-DD` (UTC) of unix time `t` (Howard Hinnant's `civil_from_days`).
fn utc_date(t: u64) -> String {
    let z = t / 86_400 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Addr, ItemDrop, Level, Monster, Player, Preset, RoomLink, RoomPos};
    use crate::guidance::Mark;
    use crate::items::CharmSize;
    use anyhow::{Context, Result};
    use serde_json::json;

    fn room(x: u32, y: u32, preset: Option<u32>) -> Room {
        Room {
            x,
            y,
            w: 40,
            h: 40,
            loaded: false,
            seen_loaded: false,
            preset,
        }
    }

    fn snap(player_id: u32, level: u32) -> Snapshot {
        Snapshot {
            player: Player {
                addr: Addr(0x1000),
                id: player_id,
                name: "P".into(),
                class: 0,
                x: 30,
                y: 30,
                level_id: level,
                act: Some(Addr(0xa000)),
                char_level: None,
                speed: None,
                wsm: None,
                strength: None,
                dexterity: None,
                states: Vec::new(),
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
                rooms: vec![room(0, 0, Some(525)), room(40, 0, Some(510)), room(80, 0, None)],
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
            buff_research: None,
            warnings: vec![],
        }
    }

    fn marker(x: u32) -> Preset {
        Preset {
            kind: PresetKind::Object,
            txt: 357,
            x,
            y: 10,
            entry: false,
        }
    }

    fn elite(mode: u32) -> Monster {
        Monster {
            addr: Addr(0),
            id: 77,
            txt: 250,
            mode,
            x: 45,
            y: 5,
            flags: 0x02,
            name_seed: 0,
            owner_id: 0,
            owner_type: 0,
            desecrated: false,
        }
    }

    fn kinds(events: &[Event]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match e {
                Event::Level { .. } => "level",
                Event::Arrival { .. } => "arrival",
                Event::Marker { .. } => "marker",
                Event::Link { .. } => "link",
                Event::Monster { .. } => "monster",
                Event::Killed { .. } => "killed",
                Event::Drop { .. } => "drop",
                Event::Shrine { .. } => "shrine",
                Event::Well { .. } => "well",
                Event::Terror { .. } => "terror",
                Event::Merc { .. } => "merc",
                Event::Stash { .. } => "stash",
            })
            .collect()
    }

    #[test]
    fn markers_dedupe_per_game() {
        let mut r = Recorder::default();
        let mut a = snap(1, 74);
        a.presets = vec![marker(10), marker(10), marker(50)];
        assert_eq!(
            kinds(&r.observe(&a, &Marks::default())),
            vec!["level", "marker", "marker"]
        );
        assert!(
            r.observe(&a, &Marks::default()).is_empty(),
            "nothing new on the next tick"
        );
        let mut b = snap(2, 74);
        b.presets = vec![marker(10)];
        assert_eq!(
            kinds(&r.observe(&b, &Marks::default())),
            vec!["level", "marker"],
            "a new game emits again"
        );
        assert_eq!(r.game(), "00000002-a000");
    }

    #[test]
    fn piece_lookup() {
        let rooms = snap(1, 74).level.rooms;
        assert_eq!(piece_at(&rooms, 0, 0), Some(525));
        assert_eq!(piece_at(&rooms, 39, 39), Some(525));
        assert_eq!(
            piece_at(&rooms, 40, 39),
            Some(510),
            "max edge belongs to the next room"
        );
        assert_eq!(piece_at(&rooms, 90, 10), None, "room without a preset");
        assert_eq!(piece_at(&rooms, 10, 40), None, "outside all rooms");
    }

    #[test]
    fn level_re_emitted_on_new_room_count() {
        let mut r = Recorder::default();
        let mut s = snap(1, 74);
        assert_eq!(kinds(&r.observe(&s, &Marks::default())), vec!["level"]);
        s.level.rooms.push(room(120, 0, Some(510)));
        assert_eq!(
            kinds(&r.observe(&s, &Marks::default())),
            vec!["level"],
            "more rooms loaded"
        );
        s.level.rooms.pop();
        assert!(
            r.observe(&s, &Marks::default()).is_empty(),
            "a count seen before is not written again"
        );
        s.level.rooms.clear();
        assert_eq!(
            r.observe(&s, &Marks::default()),
            vec![],
            "no rooms (loading): nothing"
        );
    }

    #[test]
    fn terror_once_per_level_and_game() -> Result<()> {
        let mut r = Recorder::default();
        let mut s = snap(1, 39);
        assert_eq!(
            kinds(&r.observe(&s, &Marks::default())),
            vec!["level"],
            "not terrorized: no event"
        );
        s.terrorized = true;
        let events = r.observe(&s, &Marks::default());
        let terror = events.first().context("terror event")?;
        assert_eq!(
            serde_json::to_value(terror)?,
            json!({"ev": "terror", "level": 39, "terrorized": true})
        );
        assert!(r.observe(&s, &Marks::default()).is_empty(), "once per level");
        s.level.id = 40;
        s.level.rooms.clear();
        assert_eq!(
            kinds(&r.observe(&s, &Marks::default())),
            vec!["terror"],
            "another level"
        );
        let mut next = snap(2, 39);
        next.terrorized = true;
        assert_eq!(
            kinds(&r.observe(&next, &Marks::default())),
            vec!["level", "terror"],
            "a new game"
        );
        Ok(())
    }

    #[test]
    fn killed_once() {
        let mut r = Recorder::default();
        let mut s = snap(1, 74);
        r.observe(&s, &Marks::default());
        s.monsters = vec![elite(1)];
        assert_eq!(kinds(&r.observe(&s, &Marks::default())), vec!["monster"]);
        assert_eq!(r.observe(&s, &Marks::default()), vec![]);
        s.monsters = vec![elite(0)];
        assert_eq!(kinds(&r.observe(&s, &Marks::default())), vec!["killed"]);
        s.monsters = vec![elite(12)];
        assert_eq!(r.observe(&s, &Marks::default()), vec![], "corpse seen again");
        let mut normal = elite(1);
        normal.id = 78;
        normal.flags = 0x10;
        s.monsters = vec![normal];
        assert!(
            r.observe(&s, &Marks::default()).is_empty(),
            "minions and normal monsters are not elites"
        );
    }

    #[test]
    fn charm_drop_event_shape() -> Result<()> {
        let mut r = Recorder::default();
        let mut s = snap(0xbeef, 74);
        s.drops = vec![ItemDrop {
            id: 9,
            code: Some("cm3".into()),
            kind: DropKind::Charm {
                size: CharmSize::Grand,
                unique: true,
            },
            quality: 7,
            flags: 0x0080_0000,
            ethereal: false,
            sockets: 0,
            unique_index: Some(370),
            stats: Vec::new(),
            item_data_head: "07000000".into(),
            x: 12,
            y: 13,
        }];
        let marks = Marks::from([(
            9,
            Mark {
                kind: MarkKind::Trade,
                trade: Some((Tier::S, Category::Trade)),
                rule: "Unique grand charms".into(),
                uncertain: true,
            },
        )]);
        let events = r.observe(&s, &marks);
        let drop = events
            .iter()
            .find(|e| matches!(e, Event::Drop { .. }))
            .context("drop event")?;
        assert_eq!(
            serde_json::to_value(drop)?,
            json!({"ev": "drop", "level": 74, "id": 9, "code": "cm3",
                "kind": {"charm": {"size": "grand", "unique": true}}, "quality": 7, "x": 12, "y": 13,
                "mark": "trade", "tier": "S", "category": "trade", "rule": "Unique grand charms",
                "uncertain": true,
                "sockets": 0, "ethereal": false, "flags": 0x0080_0000, "unique_index": 370,
                "item_data_head": "07000000"})
        );
        Ok(())
    }

    #[test]
    fn event_shapes() -> Result<()> {
        let mut r = Recorder::default();
        let mut s = snap(0xbeef, 74);
        s.presets = vec![marker(10)];
        s.room_links = vec![RoomLink {
            from_room: RoomPos { x: 40, y: 0 },
            to_level_id: 75,
            to_room: RoomPos { x: 0, y: 0 },
        }];
        s.monsters = vec![elite(1)];
        s.drops = vec![ItemDrop {
            id: 9,
            code: Some("r01".into()),
            kind: DropKind::Rune(1),
            quality: 2,
            flags: 0,
            ethereal: false,
            sockets: 0,
            unique_index: None,
            stats: Vec::new(),
            item_data_head: String::new(),
            x: 12,
            y: 13,
        }];
        let mut events = r.observe(&s, &Marks::default());
        events.push(arrival_event(&s, (74, 85, 3)));
        let line = |ev: &Event| {
            serde_json::to_value(Line {
                t: 100,
                game: r.game(),
                ev,
            })
        };
        let lines = events.iter().map(line).collect::<Result<Vec<_>, _>>()?;
        let expected = [
            json!({"t": 100, "game": "0000beef-a000", "ev": "level", "level": 74, "bounds": null,
                "pieces": [
                    {"preset": null, "count": 1, "min_x": 80, "min_y": 0, "max_x": 120, "max_y": 40},
                    {"preset": 510, "count": 1, "min_x": 40, "min_y": 0, "max_x": 80, "max_y": 40},
                    {"preset": 525, "count": 1, "min_x": 0, "min_y": 0, "max_x": 40, "max_y": 40}],
                "rooms": [{"x": 0, "y": 0, "w": 40, "h": 40, "preset": 525},
                    {"x": 40, "y": 0, "w": 40, "h": 40, "preset": 510},
                    {"x": 80, "y": 0, "w": 40, "h": 40, "preset": null}]}),
            json!({"t": 100, "game": "0000beef-a000", "ev": "marker", "level": 74, "kind": "object",
                "txt": 357, "x": 10, "y": 10, "piece": 525}),
            json!({"t": 100, "game": "0000beef-a000", "ev": "link", "level": 74, "x": 40, "y": 0,
                "from_piece": 510, "to_level": 75}),
            json!({"t": 100, "game": "0000beef-a000", "ev": "monster", "level": 74, "id": 77,
                "txt": 250, "flags": 2, "x": 45, "y": 5, "piece": 510, "mode": 1}),
            json!({"t": 100, "game": "0000beef-a000", "ev": "drop", "level": 74, "id": 9,
                "code": "r01", "kind": {"rune": 1}, "quality": 2, "x": 12, "y": 13,
                "mark": null, "tier": null, "category": null, "rule": null, "uncertain": false, "sockets": 0,
                "ethereal": false, "flags": 0, "unique_index": null, "item_data_head": ""}),
            json!({"t": 100, "game": "0000beef-a000", "ev": "arrival", "level": 74, "x": 85,
                "y": 3, "piece": null}),
        ];
        assert_eq!(lines, expected);

        let mut s = snap(0xbeef, 74);
        s.monsters = vec![elite(0)];
        let killed = r.observe(&s, &Marks::default());
        let v = serde_json::to_value(killed.first().context("killed event")?)?;
        assert_eq!(
            v,
            json!({"ev": "killed", "level": 74, "id": 77, "txt": 250, "x": 45, "y": 5, "piece": 510})
        );
        Ok(())
    }

    #[test]
    fn shrine_and_well_events_once_per_spot() -> Result<()> {
        let mut r = Recorder::default();
        let mut s = snap(1, 108);
        let shrine = |used| crate::game::Shrine {
            txt: 236,
            code: 2,
            used,
            x: 7,
            y: 8,
        };
        s.shrines = vec![shrine(false)];
        s.wells = vec![crate::game::Well {
            txt: 130,
            x: 9,
            y: 10,
        }];
        let values = r
            .observe(&s, &Marks::default())
            .iter()
            .filter(|e| matches!(e, Event::Shrine { .. } | Event::Well { .. }))
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            values,
            vec![
                json!({"ev": "shrine", "level": 108, "txt": 236, "x": 7, "y": 8, "shrine_type": 2,
                    "name": "health", "used": false}),
                json!({"ev": "well", "level": 108, "txt": 130, "x": 9, "y": 10}),
            ]
        );
        // Used later, or seen again: no new events; a new game sees them again.
        s.shrines = vec![shrine(true)];
        assert_eq!(r.observe(&s, &Marks::default()), vec![]);
        let mut next = snap(2, 108);
        next.shrines = vec![shrine(true)];
        assert_eq!(
            kinds(&r.observe(&next, &Marks::default())),
            vec!["level", "shrine"]
        );
        Ok(())
    }

    #[test]
    fn monster_preset_markers_name_their_superunique() -> Result<()> {
        let mut r = Recorder::default();
        let mut s = snap(1, 74);
        let monster_preset = |txt| Preset {
            kind: PresetKind::Monster,
            txt,
            ..marker(10)
        };
        s.presets = vec![monster_preset(757), monster_preset(840)];
        let values = r
            .observe(&s, &Marks::default())
            .iter()
            .filter(|e| matches!(e, Event::Marker { .. }))
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            values,
            [
                json!({"ev": "marker", "level": 74, "kind": "monster", "txt": 757, "x": 10,
                    "y": 10, "piece": 525, "superunique": "The Countess"}),
                json!({"ev": "marker", "level": 74, "kind": "monster", "txt": 840, "x": 10,
                    "y": 10, "piece": 525}),
            ]
        );
        Ok(())
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(1_794_787_199), "2026-11-15");
        assert_eq!(utc_date(1_794_787_200), "2026-11-16");
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("d2r-journal-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn writes_lines_and_appends() -> Result<()> {
        let dir = scratch("ok");
        let _ = fs::remove_dir_all(&dir);
        let mut j = Journal::new(dir.clone());
        let mut s = snap(1, 74);
        s.presets = vec![marker(10)];
        let events = j.rec.observe(&s, &Marks::default());
        let rest = events.get(1..).context("level and marker events")?;
        j.write(&events, 1_794_787_199);
        j.write(rest, 1_794_787_199);
        j.write(rest, 1_794_787_200);
        assert!(j.is_enabled());
        let day1 = fs::read_to_string(dir.join("2026-11-15.jsonl"))?;
        let day2 = fs::read_to_string(dir.join("2026-11-16.jsonl"))?;
        fs::remove_dir_all(&dir)?;
        assert_eq!(day1.lines().count(), 3);
        assert_eq!(day2.lines().count(), 1, "a new day starts a new file");
        for line in day1.lines().chain(day2.lines()) {
            let v: serde_json::Value = serde_json::from_str(line)?;
            assert_eq!(v.get("game"), Some(&json!("00000001-a000")));
        }
        Ok(())
    }

    #[test]
    fn write_error_disables_without_panic() {
        // A directory below a regular file can't be created.
        let mut j = Journal::new(PathBuf::from("/dev/null/journal"));
        let mut s = snap(1, 74);
        s.presets = vec![marker(10)];
        j.observe(&s, &Marks::default());
        assert!(!j.is_enabled(), "disabled after the error");
        j.observe(&snap(2, 74), &Marks::default());
        j.arrival(&s, (74, 1, 1));
        assert!(!j.is_enabled());
        assert!(!Journal::disabled().is_enabled());
    }
}
