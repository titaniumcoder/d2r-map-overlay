//! Draws a [`Snapshot`] as an isometric corner minimap into a tiny-skia [`Pixmap`].
//!
//! Pure: no I/O. The `--render` CLI writes the pixmap as PNG; the overlay copies it into a
//! Wayland shm buffer.

use ab_glyph::FontVec;
use anyhow::{Context, Result};
use tiny_skia::{FillRule, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, StrokeDash, Transform};

use crate::game::{Bounds, Monster, Preset, PresetKind, Room, RoomRect, Snapshot};
use crate::items::{CharmSize, DropKind, GemColour};
use crate::valuables::Tier;
use crate::{info, session};

/// Object txt ids of waypoints: every waypoint row of classic objects.txt. Live-verified: 119
/// (Act 1 town), 156 (Act 2 outdoor, Dry Hills), 157 (Act 1 indoor, Jail 1).
pub const WAYPOINT_IDS: &[u32] = &[
    119, 145, 156, 157, 237, 238, 288, 323, 324, 398, 402, 429, 494, 496, 511, 539,
];
/// A room that marks a landmark: the rooms of a level in `level_ids` whose level-preset id (lvlprest
/// row) is in `preset_ids` get one marker at the centre of their combined bounding box (one piece
/// often spans several Room2 cells): [`Marker::Landmark`] if the rule is confirmed in game,
/// [`Marker::Suspected`] (a "?") if it is only assumed.
#[derive(Debug)]
pub struct LandmarkRule {
    pub level_ids: std::ops::RangeInclusive<u32>,
    pub preset_ids: std::ops::RangeInclusive<u32>,
    /// What the landmark is (not drawn; for tests and debugging).
    pub label: &'static str,
    /// Seen in game; `false` for a rule extrapolated from a pattern.
    pub confirmed: bool,
    /// Once confirmed, draw it as a level exit ([`Marker::Exit`]) instead of a landmark star.
    pub exit: bool,
}

/// Landmark rules.
/// - Arcane Sanctuary (74): the arm end holding the Summoner's platform has its own preset per
///   direction: 525 (+x), 526 (−x), 527 (−y), 528 (+y), all four confirmed in game (2026-11-15).
///   Ordinary arm ends use other ids (510, 511, 513, 517 seen).
/// - Tal Rasha's tombs (66–72): the real tomb's orifice chamber. Confirmed: 462 in tomb 72 (orifice
///   object 152 inside it). A fake tomb's chest chamber was 473 in tomb 71. Pattern assumed: orifice
///   chamber = level + 390 (456–462), chest chamber = level + 402 (468–474); only 462 is confirmed,
///   so 456–461 are an unconfirmed rule. The 462 chamber spans 4 Room2 cells.
/// - Flayer Jungle (78): the Gidbinn altar (object 252) lies in piece 602, live-verified 2026-11-15
///   (player in a 602 room 5 tiles from the altar). 602 spans 16 Room2 cells; 601 is the waypoint
///   area (waypoint object 237).
/// - Kurast Sewers Level 1 (92): the sewer lever (object 367) and the stairs down (object 366) both
///   sat in room preset 739 (live 2026-11-15, one game; the user then went down to level 2).
/// - Act 1 caves, level 1 (Cave 9, Underground Passage 10, Hole 11, Pit 12): lvlprest names the
///   pieces "Cave Next W/E/S/N" (87–90, exit to the next outdoor area) and "Cave Down W/E/S/N"
///   (91–94, stairs to level 2); Underground Passage 1 had one 90 and one 91 piece (2026-11-15).
///   Confirmed: the 90 piece holds the exit to Dark Wood (room link to level 5, user stood on it).
/// - Stony Field (4): lvlprest 160 "Act 1 - Cairn Stones", the stone circle; the five stones and the
///   portal stone (objects 17–22) lay inside it (live 2026-11-15).
/// - Barracks (28): lvlprest "Barracks Forge W/E/S/N" (202–205) holds the Smith and the Horadric
///   Malus; confirmed for 204 (Smith preset 771 and malus object 108 both inside it, 2026-11-15).
/// - Dark Wood (5): lvlprest 161 "Act 1 - Inifus" (funktree.ds1), the Tree of Inifuss (confirmed in
///   game 2026-11-15).
/// - Black Marsh (6): lvlprest 163 "Act 1 - Tower 1" (Tower1.ds1), the Forgotten Tower entrance
///   (confirmed in game 2026-11-15).
pub const LANDMARKS: &[LandmarkRule] = &[
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 204..=204,
        label: "smith (forge)",
        confirmed: true,
        exit: false,
    },
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 202..=203,
        label: "smith (forge W/E)",
        confirmed: false,
        exit: false,
    },
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 205..=205,
        label: "smith (forge N)",
        confirmed: false,
        exit: false,
    },
    LandmarkRule {
        level_ids: 4..=4,
        preset_ids: 160..=160,
        label: "cairn stones",
        confirmed: true,
        exit: false,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 90..=90,
        label: "cave exit to the next area (north)",
        confirmed: true,
        exit: true,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 87..=89,
        label: "cave exit to the next area",
        confirmed: false,
        exit: true,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 91..=94,
        label: "cave stairs down",
        confirmed: false,
        exit: false,
    },
    LandmarkRule {
        level_ids: 5..=5,
        preset_ids: 161..=161,
        label: "tree of inifuss",
        confirmed: true,
        exit: false,
    },
    LandmarkRule {
        level_ids: 6..=6,
        preset_ids: 163..=163,
        label: "forgotten tower",
        confirmed: true,
        exit: false,
    },
    LandmarkRule {
        level_ids: 74..=74,
        preset_ids: 525..=528,
        label: "summoner",
        confirmed: true,
        exit: false,
    },
    LandmarkRule {
        level_ids: 72..=72,
        preset_ids: 462..=462,
        label: "orifice",
        confirmed: true,
        exit: false,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 456..=461,
        label: "orifice (assumed level+390 pattern)",
        confirmed: false,
        exit: false,
    },
    LandmarkRule {
        level_ids: 78..=78,
        preset_ids: 602..=602,
        label: "gidbinn",
        confirmed: true,
        exit: false,
    },
    LandmarkRule {
        level_ids: 92..=92,
        preset_ids: 739..=739,
        label: "sewer stairs",
        confirmed: true,
        exit: false,
    },
];

/// Quest objects (object txt ids from objects.txt, which matched live) drawn where their preset
/// is, once the room holding it is loaded. The id alone identifies them, so they are confirmed.
pub const QUEST_OBJECTS: &[(u32, &str)] = &[
    // Barracks (28), live 2026-11-15: the Horadric Malus stand, inside the Smith's forge piece 204.
    (108, "horadric malus"),
    (152, "orifice"),
    (252, "gidbinn altar"),
    (366, "sewer stairs"),
    (367, "sewer lever"),
    // River of Flame (107), live 2026-11-15: at the forge, 25 tiles from Hephasto's superunique
    // preset 792 (superuniques.txt calls him "The Feature Creep").
    (376, "hellforge"),
    // Chaos Sanctuary (108): the five Diablo seals (objects.txt "Seal", OperateFn 52-56); the seal
    // bosses are superunique presets 787-789 (Infector of Souls, Lord De Seis, Grand Vizier).
    (392, "diablo seal"),
    (393, "diablo seal"),
    (394, "diablo seal"),
    (395, "diablo seal"),
    (396, "diablo seal"),
    // Frozen River (114): Anya's spot is object 460, a dummy "Drehya Start Outside Town" (Drehya =
    // Anya's internal name), live 2026-11-15 next to Frozenstein's preset 810. Object 558 "frozen
    // anya" (the ice figure) is not in the level layout. The dummy can't be operated, so the star
    // stays after she is freed.
    (460, "anya"),
];

/// Quest monster presets (monster preset txt id, label) drawn like a quest object until the
/// monster is seen dead near its preset (see [`crate::session`]).
pub const QUEST_MONSTERS: &[(u32, &str)] = &[
    // Frigid Highlands (111, data name "Rigid Highlands"), quest "Rescue on Mount Arreat": the
    // breakable door of each barbarian prisoner's cage is the monster `prisondoor` (monstats
    // hcIdx 434). Verified live: every cage has monster preset 434, so a normal monster's preset
    // txt id is its hcIdx. The cage itself is object preset 473 ("cagedwussie1"), ~10 tiles
    // from the door; deliberately not in QUEST_OBJECTS so a cage gets one marker, not two.
    // Monster presets 432/433 (Barricade Door) and 435 (Barricade Tower) are siege barricades,
    // not prisoners: dropped. Still unverified: the live door's txt (hcIdx 434) and its dead
    // mode once broken, which hide the marker.
    (434, "prison door"),
    // Halls of Vaught (124): Nihlathak, monstats `nihlathakboss` hcIdx 526 (preset txt = hcIdx, as
    // verified for the prison door). Not yet seen live. The level is one piece type (864), so no
    // landmark rule can find his platform; this marker appears once his preset is loaded.
    (526, "nihlathak"),
];

/// The quest-monster label of monster preset txt id `txt`, if it is one.
pub fn quest_monster(txt: u32) -> Option<&'static str> {
    QUEST_MONSTERS
        .iter()
        .find(|&&(id, _)| id == txt)
        .map(|&(_, label)| label)
}

/// Quest objects of [`QUEST_OBJECTS`] that are used up once operated: their marker goes away for
/// the game when the live object (loaded near the player) is seen in a non-neutral mode (see
/// [`crate::session`]).
pub const CONSUMABLE_QUEST_OBJECTS: &[u32] = &[392, 393, 394, 395, 396];

/// True if object txt id `txt` is in [`CONSUMABLE_QUEST_OBJECTS`].
pub fn is_consumable_quest_object(txt: u32) -> bool {
    CONSUMABLE_QUEST_OBJECTS.contains(&txt)
}

/// The quest-object label of object txt id `txt`, if it is one.
pub fn quest_object(txt: u32) -> Option<&'static str> {
    QUEST_OBJECTS
        .iter()
        .find(|&&(id, _)| id == txt)
        .map(|&(_, label)| label)
}

/// Monster flag bits for superunique (0x02), champion (0x04) and unique/boss (0x08); minion is 0x10.
const ELITE_FLAGS: u8 = 0x0E;

/// Canvas px kept free around the level so markers on its edge stay inside the canvas.
const MARGIN: f64 = 14.0;
/// Half-size in tiles of the box drawn around the player when the level has no rooms.
const FALLBACK_SPAN: u32 = 50;

type Rgba = [u8; 4];
const ROOM: Rgba = [70, 74, 84, 120];
const ROOM_LOADED: Rgba = [125, 130, 142, 160];
const MARKER_OUTLINE: Rgba = [0, 0, 0, 220];
/// Landmark star: gold fill, a thick dark outline and a gold halo ring around it.
const LANDMARK_GOLD: Rgba = [255, 200, 30, 255];
/// Halo ring radius in px, beyond the star's tips.
const LANDMARK_RING: f32 = 13.0;
/// "?" marker for an unconfirmed landmark: a light gold glyph on a dark translucent disc.
const SUSPECT_TEXT: Rgba = [255, 235, 150, 255];
const SUSPECT_DISC: Rgba = [0, 0, 0, 150];
const SUSPECT_DISC_RADIUS: f32 = 13.0;
/// Font size; the "?" ink is about 16 px tall at this size.
const SUSPECT_GLYPH_PX: f32 = 22.0;
/// Unique items, and unique charms.
const UNIQUE_GOLD: Rgba = [235, 190, 60, 255];
/// Charms: a light blue, apart from the rune orange and the entry teal.
const CHARM_BLUE: Rgba = [120, 170, 255, 255];
/// A base shown only for a valuables rule (white 4os Crystal Sword, ...).
const BASE_WHITE: Rgba = [240, 240, 240, 255];
/// Valuables ring: radius = drop marker size + this, stroke width, and the dash pattern
/// (on, off px) of an uncertain match.
const RING_GAP: f32 = 4.0;
const RING_WIDTH: f32 = 2.0;
const RING_DASH: [f32; 2] = [3.0, 2.5];
/// Shrine dots have a white outline, apart from every other marker's dark one.
const SHRINE_OUTLINE: Rgba = [255, 255, 255, 230];
/// Well: a hollow cyan ring (the entry is a teal square) on a dark backing ring.
const WELL_CYAN: Rgba = [0, 230, 255, 255];
const WELL_RING_WIDTH: f32 = 2.0;
const WELL_BACKING_WIDTH: f32 = 3.5;

/// Outline shape of a marker. Monsters and presets are circles (the level entry a square); drops
/// are diamonds and squares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Circle,
    /// Square on its tip; the size is the centre-to-tip distance.
    Diamond,
    /// Axis-aligned; the size is half the side.
    Square,
    /// Five-pointed star, one tip up; the size is the centre-to-tip distance.
    Star,
    /// Isosceles triangle, tip up unless drawn with a direction ([`triangle_path`]); the size is
    /// the centre-to-tip distance.
    Triangle,
}

/// Something drawn on the map. Variant order is draw order (player last, on top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Marker {
    /// Tile preset: a level exit.
    Exit,
    /// The tile preset the player entered the level by (see `Session::apply`).
    Entry,
    /// A walk-through border to an adjacent level (see [`border_exits`]): a small triangle on
    /// the shared border pointing from this level's rooms toward the other level's. `dir` is that
    /// direction in world tiles (unnormalised); `entry` if the player came from that level (teal
    /// instead of the exit purple).
    BorderExit {
        dir: (i32, i32),
        entry: bool,
    },
    Waypoint,
    /// Monster preset: superunique or other special spawn, with the superunique's name when its
    /// txt id is in the table (see `session::superunique`). Not drawn as text; kept for later use.
    Special(Option<&'static str>),
    /// A room an unconfirmed landmark rule matched (see [`LANDMARKS`]), with the rule's label:
    /// drawn as a "?".
    Suspected(&'static str),
    /// A landmark room (see [`LANDMARKS`]), with the rule's label. Drawn below live elites so the
    /// boss standing on it stays visible.
    Landmark(&'static str),
    /// A quest object preset (see [`QUEST_OBJECTS`]) or quest monster preset (see
    /// [`QUEST_MONSTERS`]), with its label: a small gold star without
    /// halo, drawn above the landmark star so both show when they overlap.
    QuestObject(&'static str),
    /// A well near the player (live object): a hollow cyan ring. Wells refill: drawn used or not.
    Well,
    /// An unused shrine of a type the config shows (live object), with its shrines.txt code.
    Shrine(u8),
    /// Any other living monster, only on levels in [`ALL_MONSTER_LEVELS`]: a small dim red dot
    /// below the elites.
    Monster,
    /// Live superunique, champion or unique monster.
    Elite,
    /// The player. Drawn below drops and rings: on big levels a fresh drop lies within a pixel or
    /// two of the player and would otherwise be hidden (live, Bloody Foothills, a large charm 3.6
    /// tiles away).
    Player,
    /// An item on the ground worth picking up.
    Drop(DropKind),
    /// A valuables highlight around the drop marker of `kind`: a ring coloured by tier, dashed
    /// when the match is uncertain. Drawn after all drop markers, so no drop hides a ring.
    Ring {
        kind: DropKind,
        tier: Tier,
        uncertain: bool,
    },
}

impl Marker {
    /// Fill colour, size in px (radius, or see [`Shape`]) and shape.
    const fn style(self) -> (Rgba, f32, Shape) {
        match self {
            Self::Exit => ([170, 90, 255, 255], 5.0, Shape::Circle),
            Self::Entry => ([40, 200, 200, 255], 4.5, Shape::Square),
            Self::BorderExit { entry: false, .. } => (Self::Exit.style().0, 4.0, Shape::Triangle),
            Self::BorderExit { entry: true, .. } => (Self::Entry.style().0, 4.0, Shape::Triangle),
            Self::Waypoint => ([60, 140, 255, 255], 5.0, Shape::Circle),
            Self::Special(_) => ([255, 110, 40, 255], 4.5, Shape::Circle),
            // The ring is the no-font fallback; with a font the "?" glyph is drawn instead.
            Self::Suspected(_) => (SUSPECT_TEXT, 7.0, Shape::Circle),
            Self::Landmark(_) => (LANDMARK_GOLD, 9.0, Shape::Star),
            Self::QuestObject(_) => (LANDMARK_GOLD, 6.0, Shape::Star),
            Self::Well => (WELL_CYAN, 4.5, Shape::Circle),
            Self::Shrine(code) => (shrine_colour(code), 4.0, Shape::Circle),
            Self::Monster => ([255, 120, 120, 200], 2.0, Shape::Circle),
            Self::Elite => ([230, 30, 30, 255], 3.5, Shape::Circle),
            Self::Drop(DropKind::Unique) => (UNIQUE_GOLD, 5.0, Shape::Diamond),
            Self::Drop(DropKind::Set) => ([30, 220, 30, 255], 5.0, Shape::Diamond),
            Self::Drop(DropKind::Rune(_)) => ([255, 150, 20, 255], 3.5, Shape::Square),
            Self::Drop(DropKind::Charm { size, unique }) => {
                let colour = if unique { UNIQUE_GOLD } else { CHARM_BLUE };
                let half = match size {
                    CharmSize::Small => 2.5,
                    CharmSize::Large => 3.0,
                    CharmSize::Grand => 3.5,
                };
                (colour, half, Shape::Square)
            }
            Self::Drop(DropKind::Gem(colour)) => (gem_colour(colour), 3.5, Shape::Diamond),
            Self::Drop(DropKind::Base) => (BASE_WHITE, 3.5, Shape::Diamond),
            Self::Ring { kind, tier, .. } => {
                let radius = Self::Drop(kind).style().1 + RING_GAP;
                (tier_colour(tier), radius, Shape::Circle)
            }
            Self::Player => ([40, 255, 60, 255], 2.5, Shape::Circle),
        }
    }
}

/// Ring colour of a valuables tier.
const fn tier_colour(t: Tier) -> Rgba {
    match t {
        Tier::S => [230, 40, 40, 255],
        Tier::A => [255, 140, 0, 255],
        Tier::B => [255, 220, 0, 255],
    }
}

/// Fill colour of a shrine by shrines.txt code: the five default ones apart from each other and from
/// the waypoint blue, exit purple and elite red; other types a neutral grey.
const fn shrine_colour(code: u8) -> Rgba {
    match code {
        1 => [120, 210, 255, 255],  // refilling: light blue
        12 => [255, 80, 200, 255],  // skill: magenta
        14 => [180, 235, 30, 255],  // stamina: yellow-green
        18 => [250, 250, 250, 255], // gem: white
        20 => [140, 0, 20, 255],    // monster: dark red
        _ => [150, 150, 150, 255],
    }
}

const fn gem_colour(c: GemColour) -> Rgba {
    match c {
        GemColour::Amethyst => [175, 80, 235, 255],
        GemColour::Topaz => [255, 230, 40, 255],
        GemColour::Sapphire => [60, 120, 255, 255],
        GemColour::Emerald => [40, 200, 90, 255],
        GemColour::Ruby => [235, 30, 50, 255],
        GemColour::Diamond => [245, 245, 245, 255],
        GemColour::Skull => [185, 185, 185, 255],
    }
}

/// Isometric projection of world tiles, as in the game's automap.
pub fn iso(x: f64, y: f64) -> (f64, f64) {
    (x - y, f64::midpoint(x, y))
}

/// World tiles → canvas px: iso projection, then uniform scale and offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    scale: f64,
    dx: f64,
    dy: f64,
}

impl View {
    /// Scales and centres `bounds` into a `width`×`height` canvas, keeping [`MARGIN`] free.
    pub fn fit(bounds: Bounds, width: u32, height: u32) -> Self {
        let (min_x, min_y) = (f64::from(bounds.min_x), f64::from(bounds.min_y));
        let (max_x, max_y) = (f64::from(bounds.max_x), f64::from(bounds.max_y));
        // The extremes of x - y and (x + y) / 2 over the box are at its corners.
        let (left, right) = (min_x - max_y, max_x - min_y);
        let (top, bottom) = (f64::midpoint(min_x, min_y), f64::midpoint(max_x, max_y));
        let (w, h) = (f64::from(width), f64::from(height));
        let avail_w = 2.0f64.mul_add(-MARGIN, w).max(1.0);
        let avail_h = 2.0f64.mul_add(-MARGIN, h).max(1.0);
        let scale = (avail_w / (right - left).max(1.0)).min(avail_h / (bottom - top).max(1.0));
        Self {
            scale,
            dx: scale.mul_add(-f64::midpoint(left, right), w / 2.0),
            dy: scale.mul_add(-f64::midpoint(top, bottom), h / 2.0),
        }
    }

    /// Canvas position of world tile (`x`, `y`).
    pub fn to_screen(self, x: f64, y: f64) -> (f32, f32) {
        let (sx, sy) = iso(x, y);
        (
            px(sx.mul_add(self.scale, self.dx)),
            px(sy.mul_add(self.scale, self.dy)),
        )
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "canvas coordinates are small; f32 is what tiny-skia takes"
)]
const fn px(v: f64) -> f32 {
    v as f32
}

/// Level bounds; falls back to the rooms, then to a box around the player.
pub fn level_bounds(snap: &Snapshot) -> Bounds {
    if let Some(b) = snap.level.bounds {
        return b;
    }
    let rooms = &snap.level.rooms;
    if let (Some(min_x), Some(min_y), Some(max_x), Some(max_y)) = (
        rooms.iter().map(|r| r.x).min(),
        rooms.iter().map(|r| r.y).min(),
        rooms.iter().map(|r| r.x + r.w).max(),
        rooms.iter().map(|r| r.y + r.h).max(),
    ) {
        return Bounds {
            min_x,
            min_y,
            max_x,
            max_y,
        };
    }
    let (x, y) = (u32::from(snap.player.x), u32::from(snap.player.y));
    Bounds {
        min_x: x.saturating_sub(FALLBACK_SPAN),
        min_y: y.saturating_sub(FALLBACK_SPAN),
        max_x: x + FALLBACK_SPAN,
        max_y: y + FALLBACK_SPAN,
    }
}

/// Levels where every living enemy is drawn (not hirelings, summons or critters, see
/// `NON_HOSTILE_MONSTERS`), not just elites: Den of Evil (8), whose quest
/// needs every monster killed (user request, 2026-11-15).
pub const ALL_MONSTER_LEVELS: &[u32] = &[8];

/// Live superunique/champion/unique, not dead. Minion-only (0x10) and normal (0) monsters are skipped.
pub fn is_elite(m: &Monster) -> bool {
    m.flags & ELITE_FLAGS != 0 && !m.is_dead()
}

/// The marker a preset is drawn as, if any.
pub fn preset_marker(p: &Preset) -> Option<Marker> {
    match p.kind {
        PresetKind::Object if WAYPOINT_IDS.contains(&p.txt) => Some(Marker::Waypoint),
        PresetKind::Object => quest_object(p.txt).map(Marker::QuestObject),
        PresetKind::Tile if p.entry => Some(Marker::Entry),
        PresetKind::Tile => Some(Marker::Exit),
        PresetKind::Monster => Some(quest_monster(p.txt).map_or_else(
            || Marker::Special(session::superunique(p.txt)),
            Marker::QuestObject,
        )),
        PresetKind::Other(_) => None,
    }
}

/// One landmark marker per rule in `rules` with matching rooms, at the centre of the bounding box of
/// all of them.
pub fn landmarks(snap: &Snapshot, rules: &[LandmarkRule]) -> Vec<(Marker, f64, f64)> {
    let level = &snap.level;
    rules
        .iter()
        .filter(|rule| rule.level_ids.contains(&level.id))
        .filter_map(|rule| {
            let (x0, y0, x1, y1) = level
                .rooms
                .iter()
                .filter(|r| r.preset.is_some_and(|id| rule.preset_ids.contains(&id)))
                .map(|r| (r.x, r.y, r.x.saturating_add(r.w), r.y.saturating_add(r.h)))
                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))?;
            let x = f64::midpoint(f64::from(x0), f64::from(x1));
            let y = f64::midpoint(f64::from(y0), f64::from(y1));
            let marker = if rule.confirmed && rule.exit {
                Marker::Exit
            } else if rule.confirmed {
                Marker::Landmark(rule.label)
            } else {
                Marker::Suspected(rule.label)
            };
            Some((marker, x, y))
        })
        .collect()
}

/// A world-tile box, `max` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl Span {
    const fn of(r: RoomRect) -> Self {
        Self {
            x0: r.x,
            y0: r.y,
            x1: r.x.saturating_add(r.w),
            y1: r.y.saturating_add(r.h),
        }
    }

    fn union(self, o: Self) -> Self {
        Self {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    /// Where `self`, grown by one tile on every side, overlaps `o`: the strip of `o` along the
    /// edge the two share. `None` if they don't touch.
    fn touch(self, o: Self) -> Option<Self> {
        let s = Self {
            x0: self.x0.saturating_sub(1).max(o.x0),
            y0: self.y0.saturating_sub(1).max(o.y0),
            x1: self.x1.saturating_add(1).min(o.x1),
            y1: self.y1.saturating_add(1).min(o.y1),
        };
        (s.x0 < s.x1 && s.y0 < s.y1).then_some(s)
    }

    /// Twice the centre (integer, no rounding).
    fn centre2(self) -> (i64, i64) {
        (
            i64::from(self.x0) + i64::from(self.x1),
            i64::from(self.y0) + i64::from(self.y1),
        )
    }
}

/// One [`Marker::BorderExit`] per adjacent level the player can walk into over the level's
/// edge (from [`Snapshot::border_links`]). Only near rooms that actually touch a room of this
/// level count: a level reached by stairs or a cave mouth lies elsewhere in world space, so it
/// never touches, and levels with a room-tile link keep their tile exit markers instead. The
/// marker sits at the centre of the shared border strips; it points from the centre of this
/// level's touching rooms to the centre of the other level's.
pub fn border_exits(snap: &Snapshot) -> Vec<(Marker, f64, f64)> {
    let mut levels: Vec<u32> = snap.border_links.iter().map(|l| l.to_level_id).collect();
    levels.sort_unstable();
    levels.dedup();
    levels
        .into_iter()
        .filter(|id| !snap.room_links.iter().any(|l| l.to_level_id == *id))
        .filter_map(|id| {
            let mut found: Option<(Span, Span, Span)> = None;
            let mut entry = false;
            for l in snap.border_links.iter().filter(|l| l.to_level_id == id) {
                let (from, to) = (Span::of(l.from_room), Span::of(l.to_room));
                let Some(strip) = from.touch(to) else { continue };
                entry |= l.entry;
                found = Some(found.map_or((strip, from, to), |(s, f, t)| {
                    (s.union(strip), f.union(from), t.union(to))
                }));
            }
            let (strip, from, to) = found?;
            let ((fx, fy), (tx, ty)) = (from.centre2(), to.centre2());
            let dir = (i32::try_from(tx - fx).ok()?, i32::try_from(ty - fy).ok()?);
            let (sx, sy) = strip.centre2();
            #[allow(clippy::cast_precision_loss, reason = "world tiles are far below 2^52")]
            let (x, y) = (sx as f64 / 2.0, sy as f64 / 2.0);
            Some((Marker::BorderExit { dir, entry }, x, y))
        })
        .collect()
}

/// Every marker of the snapshot with its world position, in draw order.
pub fn markers(snap: &Snapshot) -> Vec<(Marker, f64, f64)> {
    let mut out: Vec<(Marker, f64, f64)> = snap
        .presets
        .iter()
        .filter_map(|p| preset_marker(p).map(|m| (m, f64::from(p.x), f64::from(p.y))))
        .chain(
            snap.wells
                .iter()
                .map(|w| (Marker::Well, f64::from(w.x), f64::from(w.y))),
        )
        .chain(
            snap.shrines
                .iter()
                .map(|s| (Marker::Shrine(s.code), f64::from(s.x), f64::from(s.y))),
        )
        .chain(snap.monsters.iter().filter(|m| !m.is_dead()).filter_map(|m| {
            let marker = if is_elite(m) {
                Marker::Elite
            } else if ALL_MONSTER_LEVELS.contains(&snap.level.id)
                && !crate::levels_data::NON_HOSTILE_MONSTERS.contains(&m.txt)
            {
                Marker::Monster
            } else {
                return None;
            };
            Some((marker, f64::from(m.x), f64::from(m.y)))
        }))
        .chain(
            snap.drops
                .iter()
                .map(|d| (Marker::Drop(d.kind), f64::from(d.x), f64::from(d.y))),
        )
        .chain(snap.drops.iter().filter_map(|d| {
            let h = d.highlight.as_ref()?;
            let ring = Marker::Ring {
                kind: d.kind,
                tier: h.tier,
                uncertain: h.uncertain,
            };
            Some((ring, f64::from(d.x), f64::from(d.y)))
        }))
        .chain(landmarks(snap, LANDMARKS))
        .chain(border_exits(snap))
        .collect();
    out.push((Marker::Player, f64::from(snap.player.x), f64::from(snap.player.y)));
    // Stable sort: keeps snapshot order within one kind.
    out.sort_by_key(|&(m, _, _)| m);
    out
}

/// Renders the snapshot's level into a new `width`×`height` pixmap, with the info line (see
/// [`info`]) at the bottom if a font is available. There is no background panel: everything
/// outside the level shape, the markers and the text is fully transparent.
///
/// # Errors
/// Fails if the canvas size is zero or too large for tiny-skia.
pub fn render(snap: &Snapshot, width: u32, height: u32) -> Result<Pixmap> {
    render_with(snap, width, height, info::font())
}

/// The canvas height the map is fitted into: all of it, less the info line's band and the
/// band of `keep_lines` keep lines if there is a font to draw them with.
pub fn map_height(height: u32, font: Option<&FontVec>, keep_lines: usize) -> u32 {
    font.map_or(height, |f| {
        let bands = info::band_height(f) + info::keep_band_height(f, keep_lines);
        height.saturating_sub(bands).max(1)
    })
}

/// The inventory keep list as text lines for a `width` px canvas; none without a font.
fn keep_lines(snap: &Snapshot, width: u32, font: Option<&FontVec>) -> Vec<String> {
    match font {
        Some(f) if !snap.inventory_keep.is_empty() => {
            let max_width = f32::from(u16::try_from(width).unwrap_or(u16::MAX)) - 8.0;
            info::keep_lines(f, &snap.inventory_keep, max_width)
        }
        _ => Vec::new(),
    }
}

/// A frame with only the inventory keep lines (for towns, where the map is hidden); `None` if
/// there is nothing to show (empty list, or no font to draw it with).
///
/// # Errors
/// Fails if the canvas size is zero or too large for tiny-skia.
pub fn render_keep_only(
    snap: &Snapshot,
    width: u32,
    height: u32,
    font: Option<&FontVec>,
) -> Result<Option<Pixmap>> {
    let lines = keep_lines(snap, width, font);
    let Some(font) = font.filter(|_| !lines.is_empty()) else {
        return Ok(None);
    };
    let mut pm =
        Pixmap::new(width, height).with_context(|| format!("invalid canvas size {width}x{height}"))?;
    info::draw_keep(&mut pm, font, &lines);
    Ok(Some(pm))
}

/// [`render`] with an explicit font; `None` draws no info line and gives the map the full canvas.
///
/// # Errors
/// Fails if the canvas size is zero or too large for tiny-skia.
pub fn render_with(snap: &Snapshot, width: u32, height: u32, font: Option<&FontVec>) -> Result<Pixmap> {
    let mut pm =
        Pixmap::new(width, height).with_context(|| format!("invalid canvas size {width}x{height}"))?;
    let keep = keep_lines(snap, width, font);
    let view = View::fit(level_bounds(snap), width, map_height(height, font, keep.len()));
    fill_rooms(&mut pm, view, snap);

    let ring = Stroke {
        width: 1.5,
        ..Stroke::default()
    };
    // The landmark star's outline is stroked under its fill: half of it shows, a clean 2 px dark
    // border, and the star's narrow inner corners keep their gold.
    // Round joins: a miter on the sharp tips would spike out into the halo.
    let star_outline = Stroke {
        width: 4.0,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let halo_ring = Stroke {
        width: 2.0,
        ..Stroke::default()
    };
    let halo_backing = Stroke {
        width: 4.5,
        ..Stroke::default()
    };
    // The quest-object star: same scheme as the landmark star, thinner for its smaller size.
    let small_star_outline = Stroke {
        width: 3.0,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let solid = |pm: &mut Pixmap, path: &Path, colour: Rgba| {
        pm.fill_path(
            path,
            &paint(colour),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    };
    for (marker, x, y) in markers(snap) {
        let (colour, size, shape) = marker.style();
        let (cx, cy) = view.to_screen(x, y);
        let Some(dot) = marker_outline(marker, shape, cx, cy, size) else {
            continue;
        };
        if let Marker::Suspected(_) = marker {
            draw_suspected(&mut pm, font, &dot, cx, cy);
        } else if let Marker::Ring { uncertain, .. } = marker {
            draw_ring(&mut pm, &dot, colour, uncertain);
        } else if marker == Marker::Well {
            draw_well(&mut pm, &dot, colour);
        } else if let Marker::Shrine(_) = marker {
            solid(&mut pm, &dot, colour);
            pm.stroke_path(&dot, &paint(SHRINE_OUTLINE), &ring, Transform::identity(), None);
        } else if let Marker::QuestObject(_) = marker {
            pm.stroke_path(
                &dot,
                &paint(MARKER_OUTLINE),
                &small_star_outline,
                Transform::identity(),
                None,
            );
            solid(&mut pm, &dot, colour);
        } else if shape == Shape::Triangle {
            // Outline under the fill: the 6 px triangle keeps all its colour, with half the
            // outline showing around it.
            pm.stroke_path(&dot, &paint(MARKER_OUTLINE), &ring, Transform::identity(), None);
            solid(&mut pm, &dot, colour);
        } else if shape == Shape::Star {
            // Halo: a gold ring on a wider dark ring, so it reads on any background.
            if let Some(halo) = PathBuilder::from_circle(cx, cy, LANDMARK_RING) {
                pm.stroke_path(
                    &halo,
                    &paint(MARKER_OUTLINE),
                    &halo_backing,
                    Transform::identity(),
                    None,
                );
                pm.stroke_path(&halo, &paint(colour), &halo_ring, Transform::identity(), None);
            }
            pm.stroke_path(
                &dot,
                &paint(MARKER_OUTLINE),
                &star_outline,
                Transform::identity(),
                None,
            );
            solid(&mut pm, &dot, colour);
        } else {
            solid(&mut pm, &dot, colour);
            pm.stroke_path(&dot, &paint(MARKER_OUTLINE), &ring, Transform::identity(), None);
        }
    }
    if let Some(font) = font {
        let inset = info::keep_band_height(font, keep.len());
        info::draw(
            &mut pm,
            font,
            &info::info_line(snap.level.id, snap.difficulty, snap.terrorized),
            inset,
        );
        info::draw_keep(&mut pm, font, &keep);
    }
    Ok(pm)
}

/// Fills the level's rooms, then the loaded ones on top.
fn fill_rooms(pm: &mut Pixmap, view: View, snap: &Snapshot) {
    // The level is one continuous shape: all rooms in one path, then the loaded ones in a second
    // path on top. Separate fills per room would leave anti-aliasing seams along shared edges;
    // within one path the edges' coverage adds up, and the loaded layer blends onto the base fill
    // rather than onto the transparent canvas.
    for (rooms, fill) in [
        (rooms_path(view, snap.level.rooms.iter()), ROOM),
        (
            rooms_path(view, snap.level.rooms.iter().filter(|r| r.loaded)),
            ROOM_LOADED,
        ),
    ] {
        if let Some(path) = rooms {
            pm.fill_path(
                &path,
                &paint(fill),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// A valuables ring along `circle`: solid, or dashed for an `uncertain` match.
fn draw_ring(pm: &mut Pixmap, circle: &Path, colour: Rgba, uncertain: bool) {
    let stroke = Stroke {
        width: RING_WIDTH,
        dash: if uncertain {
            StrokeDash::new(RING_DASH.to_vec(), 0.0)
        } else {
            None
        },
        ..Stroke::default()
    };
    pm.stroke_path(circle, &paint(colour), &stroke, Transform::identity(), None);
}

/// A well: the hollow `circle` in `colour` on a wider dark ring, so it reads on any background.
fn draw_well(pm: &mut Pixmap, circle: &Path, colour: Rgba) {
    for (colour, width) in [(MARKER_OUTLINE, WELL_BACKING_WIDTH), (colour, WELL_RING_WIDTH)] {
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(circle, &paint(colour), &stroke, Transform::identity(), None);
    }
}

/// A "?" for an unconfirmed landmark at (`cx`, `cy`): the glyph on a dark translucent disc, or
/// without a font the hollow light-gold `ring` on a dark backing.
fn draw_suspected(pm: &mut Pixmap, font: Option<&FontVec>, ring: &Path, cx: f32, cy: f32) {
    if let Some(font) = font {
        if let Some(disc) = PathBuilder::from_circle(cx, cy, SUSPECT_DISC_RADIUS) {
            pm.fill_path(
                &disc,
                &paint(SUSPECT_DISC),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        info::draw_centred(pm, font, SUSPECT_GLYPH_PX, SUSPECT_TEXT, '?', cx, cy);
    } else {
        for (colour, width) in [(MARKER_OUTLINE, 4.0), (LANDMARK_GOLD, 2.0)] {
            let stroke = Stroke {
                width,
                ..Stroke::default()
            };
            pm.stroke_path(ring, &paint(colour), &stroke, Transform::identity(), None);
        }
    }
}

/// The outline of `marker` (drawn with `shape` and `size`) centred on (`cx`, `cy`): a border
/// exit's triangle points its way, everything else is [`marker_path`].
fn marker_outline(marker: Marker, shape: Shape, cx: f32, cy: f32, size: f32) -> Option<Path> {
    match marker {
        Marker::BorderExit { dir: (dx, dy), .. } => {
            // `iso` is linear, so the screen direction is the projected world direction.
            let (ux, uy) = iso(f64::from(dx), f64::from(dy));
            triangle_path(cx, cy, size, (px(ux), px(uy)))
        }
        _ => marker_path(shape, cx, cy, size),
    }
}

/// A triangle centred on (`cx`, `cy`) with its tip `size` px from the centre toward screen
/// direction `(ux, uy)` (any length; zero points up). Size 4 gives a triangle about 6 px long and wide.
fn triangle_path(cx: f32, cy: f32, size: f32, (ux, uy): (f32, f32)) -> Option<Path> {
    let len = ux.hypot(uy);
    let (ux, uy) = if len > f32::EPSILON {
        (ux / len, uy / len)
    } else {
        (0.0, -1.0)
    };
    // Perpendicular to the direction, for the base corners.
    let (vx, vy) = (-uy, ux);
    let back = size * 0.5;
    let half_base = size * 0.65;
    let mut pb = PathBuilder::new();
    pb.move_to(ux.mul_add(size, cx), uy.mul_add(size, cy));
    pb.line_to(
        vx.mul_add(half_base, ux.mul_add(-back, cx)),
        vy.mul_add(half_base, uy.mul_add(-back, cy)),
    );
    pb.line_to(
        vx.mul_add(-half_base, ux.mul_add(-back, cx)),
        vy.mul_add(-half_base, uy.mul_add(-back, cy)),
    );
    pb.close();
    pb.finish()
}

/// The outline of a marker centred on (`cx`, `cy`); `None` for a degenerate size.
fn marker_path(shape: Shape, cx: f32, cy: f32, size: f32) -> Option<Path> {
    match shape {
        Shape::Triangle => triangle_path(cx, cy, size, (0.0, -1.0)),
        Shape::Circle => PathBuilder::from_circle(cx, cy, size),
        Shape::Square => {
            Rect::from_xywh(cx - size, cy - size, 2.0 * size, 2.0 * size).map(PathBuilder::from_rect)
        }
        Shape::Diamond => {
            let mut pb = PathBuilder::new();
            pb.move_to(cx, cy - size);
            pb.line_to(cx + size, cy);
            pb.line_to(cx, cy + size);
            pb.line_to(cx - size, cy);
            pb.close();
            pb.finish()
        }
        Shape::Star => {
            // Ten points alternating tip and inner corner, starting at the top tip.
            let inner = size * 0.45;
            let mut pb = PathBuilder::new();
            for i in 0..10u8 {
                let r = if i % 2 == 0 { size } else { inner };
                let angle = f32::from(i).mul_add(std::f32::consts::PI / 5.0, -std::f32::consts::FRAC_PI_2);
                let (x, y) = (r.mul_add(angle.cos(), cx), r.mul_add(angle.sin(), cy));
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            pb.close();
            pb.finish()
        }
    }
}

/// One path with a parallelogram per room, all wound the same way (`None` if there are none).
fn rooms_path<'a>(view: View, rooms: impl Iterator<Item = &'a Room>) -> Option<Path> {
    let mut pb = PathBuilder::new();
    for room in rooms {
        let (x0, y0) = (f64::from(room.x), f64::from(room.y));
        let (x1, y1) = (f64::from(room.x + room.w), f64::from(room.y + room.h));
        let (ax, ay) = view.to_screen(x0, y0);
        pb.move_to(ax, ay);
        for (x, y) in [(x1, y0), (x1, y1), (x0, y1)] {
            let (sx, sy) = view.to_screen(x, y);
            pb.line_to(sx, sy);
        }
        pb.close();
    }
    pb.finish()
}

fn paint([r, g, b, a]: Rgba) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Addr, BorderLink, ItemDrop, Level, Player, RoomLink, RoomPos};

    const FIXTURE: &str = include_str!("../tests/fixtures/jail1-snapshot.json");

    /// Shadows `super::render` (a local item wins over the glob import): the map tests render
    /// without the info line, so they don't depend on the fonts installed on this machine.
    fn render(snap: &Snapshot, width: u32, height: u32) -> Result<Pixmap> {
        render_with(snap, width, height, None)
    }

    fn fixture() -> Result<Snapshot> {
        serde_json::from_str(FIXTURE).context("parsing the fixture")
    }

    fn monster(flags: u8, mode: u32) -> Monster {
        Monster {
            addr: Addr(0),
            id: 1,
            txt: 1,
            mode,
            x: 0,
            y: 0,
            flags,
            name_seed: 0,
            desecrated: false,
        }
    }

    fn preset(kind: PresetKind, txt: u32) -> Preset {
        Preset {
            kind,
            txt,
            x: 10,
            y: 20,
            entry: false,
        }
    }

    fn empty_snapshot(px: u16, py: u16) -> Snapshot {
        Snapshot {
            player: Player {
                addr: Addr(0),
                id: 1,
                name: "P".into(),
                class: 0,
                x: px,
                y: py,
                level_id: 1,
                act: None,
            },
            player_candidates: vec![],
            player_chosen_by: crate::game::ChosenBy::FirstCandidate,
            level: Level {
                id: 1,
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
            inventory_keep: vec![],
            difficulty: None,
            terrorized: false,
            warnings: vec![],
        }
    }

    #[test]
    fn iso_matches_automap_axes() {
        assert_eq!(iso(0.0, 0.0), (0.0, 0.0));
        // +x goes right-down, +y goes left-down.
        assert_eq!(iso(10.0, 0.0), (10.0, 5.0));
        assert_eq!(iso(0.0, 10.0), (-10.0, 5.0));
        assert_eq!(iso(10.0, 10.0), (0.0, 10.0));
    }

    #[test]
    fn fit_centres_bounds_and_keeps_corners_inside() {
        let b = Bounds {
            min_x: 17500,
            min_y: 5000,
            max_x: 17860,
            max_y: 5240,
        };
        for (w, h) in [(400, 400), (400, 200), (200, 400), (37, 911)] {
            let v = View::fit(b, w, h);
            let corners = [
                (b.min_x, b.min_y),
                (b.max_x, b.min_y),
                (b.max_x, b.max_y),
                (b.min_x, b.max_y),
            ];
            let pts: Vec<(f32, f32)> = corners
                .iter()
                .map(|&(x, y)| v.to_screen(f64::from(x), f64::from(y)))
                .collect();
            let m = px(MARGIN) - 0.01;
            for &(x, y) in &pts {
                assert!(x >= m && x <= px(f64::from(w)) - m, "{w}x{h}: x {x}");
                assert!(y >= m && y <= px(f64::from(h)) - m, "{w}x{h}: y {y}");
            }
            // Centred: the projected extremes are symmetric around the canvas centre.
            let (lo_x, hi_x) = pts
                .iter()
                .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
            let (lo_y, hi_y) = pts
                .iter()
                .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
            assert!((f32::midpoint(lo_x, hi_x) - px(f64::from(w)) / 2.0).abs() < 0.01);
            assert!((f32::midpoint(lo_y, hi_y) - px(f64::from(h)) / 2.0).abs() < 0.01);
        }
    }

    #[test]
    fn fixture_rooms_fit_the_canvas() -> Result<()> {
        let snap = fixture()?;
        let v = View::fit(level_bounds(&snap), 400, 300);
        for r in &snap.level.rooms {
            for (x, y) in [
                (r.x, r.y),
                (r.x + r.w, r.y),
                (r.x + r.w, r.y + r.h),
                (r.x, r.y + r.h),
            ] {
                let (sx, sy) = v.to_screen(f64::from(x), f64::from(y));
                assert!(
                    (0.0..=400.0).contains(&sx) && (0.0..=300.0).contains(&sy),
                    "{x},{y} → {sx},{sy}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn bounds_fall_back_to_rooms_then_player() {
        let mut snap = empty_snapshot(100, 30);
        assert_eq!(
            level_bounds(&snap),
            Bounds {
                min_x: 50,
                min_y: 0,
                max_x: 150,
                max_y: 80
            }
        );
        snap.level.rooms = vec![
            Room {
                x: 10,
                y: 20,
                w: 5,
                h: 5,
                loaded: false,
                preset: None,
            },
            Room {
                x: 0,
                y: 30,
                w: 5,
                h: 10,
                loaded: true,
                preset: None,
            },
        ];
        assert_eq!(
            level_bounds(&snap),
            Bounds {
                min_x: 0,
                min_y: 20,
                max_x: 15,
                max_y: 40
            }
        );
    }

    #[test]
    fn elite_filter() {
        assert!(is_elite(&monster(0x08, 1)), "unique");
        assert!(is_elite(&monster(0x04, 2)), "champion");
        assert!(is_elite(&monster(0x02, 1)), "superunique");
        assert!(is_elite(&monster(0x0C, 1)), "unique pack leader");
        assert!(is_elite(&monster(0x18, 1)), "unique that is also a minion");
        assert!(!is_elite(&monster(0x10, 1)), "minion only");
        assert!(!is_elite(&monster(0x00, 1)), "normal");
        assert!(!is_elite(&monster(0x08, 0)), "dead (mode 0)");
        assert!(!is_elite(&monster(0x0C, 12)), "dead (mode 12)");
    }

    #[test]
    fn preset_classification() {
        assert_eq!(
            preset_marker(&preset(PresetKind::Object, 157)),
            Some(Marker::Waypoint)
        );
        for wp in [119, 156, 157, 539] {
            assert_eq!(
                preset_marker(&preset(PresetKind::Object, wp)),
                Some(Marker::Waypoint),
                "{wp}"
            );
        }
        assert_eq!(preset_marker(&preset(PresetKind::Object, 16)), None);
        assert_eq!(preset_marker(&preset(PresetKind::Tile, 13)), Some(Marker::Exit));
        let entry = Preset {
            entry: true,
            ..preset(PresetKind::Tile, 13)
        };
        assert_eq!(preset_marker(&entry), Some(Marker::Entry));
        assert_eq!(
            preset_marker(&preset(PresetKind::Monster, 771)),
            Some(Marker::Special(Some("The Smith")))
        );
        assert_eq!(
            preset_marker(&preset(PresetKind::Monster, 840)),
            Some(Marker::Special(None)),
            "unfiltered generic preset: same marker, no name"
        );
        assert_eq!(preset_marker(&preset(PresetKind::Other(9), 1)), None);
    }

    #[test]
    fn fixture_markers() -> Result<()> {
        let snap = fixture()?;
        // Jail 1: one waypoint, one exit tile, no elites (only normals and dead minions), the player.
        assert_eq!(
            markers(&snap),
            vec![
                (Marker::Exit, 17700.0, 5211.0),
                (Marker::Waypoint, 17519.0, 5144.0),
                (Marker::Player, 17594.0, 5157.0),
            ]
        );
        Ok(())
    }

    #[test]
    fn den_of_evil_shows_every_living_monster() {
        let mut snap = empty_snapshot(5, 5);
        let mut wolf = monster(0x00, 1);
        wolf.txt = 420; // druid spirit wolf: a summon, never drawn
        snap.monsters = vec![
            monster(0x00, 1),
            monster(0x10, 1),
            monster(0x08, 1),
            monster(0x00, 12),
            wolf,
        ];
        let kinds = |s: &Snapshot| -> Vec<Marker> { markers(s).into_iter().map(|(m, _, _)| m).collect() };
        assert_eq!(
            kinds(&snap),
            vec![Marker::Elite, Marker::Player],
            "elsewhere: elites only"
        );
        snap.level.id = 8;
        assert_eq!(
            kinds(&snap),
            vec![Marker::Monster, Marker::Monster, Marker::Elite, Marker::Player],
            "Den of Evil: normals and minions too, dead skipped, below the elite"
        );
    }

    #[test]
    fn markers_draw_player_last() {
        let mut snap = empty_snapshot(5, 5);
        snap.monsters = vec![monster(0x08, 1), monster(0x10, 1)];
        snap.presets = vec![preset(PresetKind::Monster, 1), preset(PresetKind::Tile, 1)];
        let kinds: Vec<Marker> = markers(&snap).into_iter().map(|(m, _, _)| m).collect();
        assert_eq!(
            kinds,
            vec![Marker::Exit, Marker::Special(None), Marker::Elite, Marker::Player]
        );
    }

    #[test]
    fn fixture_round_trips() -> Result<()> {
        let original: serde_json::Value = serde_json::from_str(FIXTURE)?;
        let back = serde_json::to_value(fixture()?)?;
        assert_eq!(back, original);
        Ok(())
    }

    #[test]
    fn render_fixture_smoke() -> Result<()> {
        let snap = fixture()?;
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        assert_eq!((pm.width(), pm.height()), (w, h));

        // No background panel: corners and edges are fully transparent.
        assert_eq!(
            pm.pixel(0, 0).map(tiny_skia::PremultipliedColorU8::alpha),
            Some(0)
        );
        for (x, y) in [(3, h / 2), (w / 2, 0), (w - 1, h - 1), (w / 2, h - 3)] {
            assert_eq!(pm.pixel(x, y).context("edge pixel")?.alpha(), 0, "{x},{y}");
        }

        // Pixels in exactly the (opaque) player colour exist and all sit on the projected player.
        let v = View::fit(level_bounds(&snap), w, h);
        let (cx, cy) = v.to_screen(f64::from(snap.player.x), f64::from(snap.player.y));
        let (player, radius, _) = Marker::Player.style();
        let mut hits = 0;
        for y in 0..h {
            for x in 0..w {
                let p = pm.pixel(x, y).context("pixel")?;
                if [p.red(), p.green(), p.blue(), p.alpha()] == player {
                    hits += 1;
                    let dist = (f64::from(x) + 0.5 - f64::from(cx)).hypot(f64::from(y) + 0.5 - f64::from(cy));
                    assert!(dist <= f64::from(radius), "player pixel {x},{y} is {dist} px off");
                }
            }
        }
        assert!(hits >= 3, "only {hits} player pixels");
        Ok(())
    }

    fn rgba(pm: &Pixmap, x: u32, y: u32) -> Result<Rgba> {
        let p = pm.pixel(x, y).with_context(|| format!("pixel {x},{y}"))?;
        Ok([p.red(), p.green(), p.blue(), p.alpha()])
    }

    fn room(x: u32, y: u32, loaded: bool) -> Room {
        Room {
            x,
            y,
            w: 60,
            h: 60,
            loaded,
            preset: None,
        }
    }

    #[test]
    fn adjacent_rooms_render_without_seams() -> Result<()> {
        // Two unloaded rooms side by side in x, sharing the edge x = 60. The player is far off.
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, false), room(60, 0, false)];
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let inside = |x: f64, y: f64| -> Result<Rgba> {
            let (sx, sy) = v.to_screen(x, y);
            #[allow(
                clippy::cast_sign_loss,
                clippy::cast_possible_truncation,
                reason = "test points are inside the canvas"
            )]
            let (px, py) = (sx.floor() as u32, sy.floor() as u32);
            rgba(&pm, px, py)
        };
        let fill = inside(30.0, 30.0)?;
        // Every pixel along the shared edge looks like the inside of a room: no outline, no seam.
        for i in 1..60 {
            assert_eq!(inside(60.0, f64::from(i))?, fill, "seam at edge tile {i}");
        }
        assert_eq!(inside(90.0, 30.0)?, fill, "both rooms in the same colour");

        // A loaded room next to an unloaded one: brighter, and no gap along the shared edge.
        snap.level.rooms = vec![room(0, 0, false), room(60, 0, true)];
        let pm2 = render(&snap, w, h)?;
        let at = |x: f64, y: f64| -> Result<Rgba> {
            let (sx, sy) = v.to_screen(x, y);
            #[allow(
                clippy::cast_sign_loss,
                clippy::cast_possible_truncation,
                reason = "test points are inside the canvas"
            )]
            let (px, py) = (sx.floor() as u32, sy.floor() as u32);
            rgba(&pm2, px, py)
        };
        let (dim, bright) = (at(30.0, 30.0)?, at(90.0, 30.0)?);
        assert!(bright[0] > dim[0] && bright[3] > dim[3], "{bright:?} vs {dim:?}");
        for i in 1..60 {
            let edge = at(60.0, f64::from(i))?;
            assert!(edge[3] >= dim[3], "edge tile {i} darker than the rooms: {edge:?}");
        }
        Ok(())
    }

    fn drop_at(kind: DropKind, x: u32, y: u32) -> ItemDrop {
        ItemDrop {
            id: 1,
            code: None,
            kind,
            quality: 0,
            flags: 0,
            ethereal: false,
            sockets: 0,
            unique_index: None,
            item_data_head: String::new(),
            highlight: None,
            x,
            y,
        }
    }

    #[test]
    fn drops_are_markers_above_elites() {
        let mut snap = empty_snapshot(5, 5);
        snap.monsters = vec![monster(0x08, 1)];
        snap.drops = vec![
            drop_at(DropKind::Unique, 1, 1),
            drop_at(DropKind::Rune(8), 2, 2),
            drop_at(DropKind::Gem(GemColour::Ruby), 3, 3),
        ];
        let kinds: Vec<Marker> = markers(&snap).into_iter().map(|(m, _, _)| m).collect();
        assert_eq!(
            kinds,
            vec![
                Marker::Elite,
                Marker::Player,
                Marker::Drop(DropKind::Gem(GemColour::Ruby)),
                Marker::Drop(DropKind::Rune(8)),
                Marker::Drop(DropKind::Unique),
            ],
            "unique on top of the other drops; drops above the player so a fresh drop isn't hidden"
        );
        // Drops never look like a dot: every drop style has a non-circle shape.
        for kind in [
            DropKind::Unique,
            DropKind::Set,
            DropKind::Rune(1),
            DropKind::Gem(GemColour::Skull),
            DropKind::Charm {
                size: CharmSize::Small,
                unique: false,
            },
        ] {
            assert_ne!(Marker::Drop(kind).style().2, Shape::Circle, "{kind:?}");
        }
    }

    /// Pixels in exactly `colour`: (count, max |dx|, max |dy|) from (`cx`, `cy`).
    fn colour_spread(pm: &Pixmap, colour: Rgba, (cx, cy): (f32, f32)) -> Result<(u32, f64, f64)> {
        let (mut hits, mut dx, mut dy) = (0, 0.0f64, 0.0f64);
        for y in 0..pm.height() {
            for x in 0..pm.width() {
                if rgba(pm, x, y)? == colour {
                    hits += 1;
                    dx = dx.max((f64::from(x) + 0.5 - f64::from(cx)).abs());
                    dy = dy.max((f64::from(y) + 0.5 - f64::from(cy)).abs());
                }
            }
        }
        Ok((hits, dx, dy))
    }

    fn highlighted(kind: DropKind, x: u32, tier: Tier, uncertain: bool) -> ItemDrop {
        let mut d = drop_at(kind, x, 30);
        d.highlight = Some(crate::valuables::Highlight {
            tier,
            category: crate::valuables::Category::Trade,
            rule: "test".into(),
            uncertain,
        });
        d
    }

    /// Of `n` points evenly around the circle of `radius` px around `at`, how many are in the
    /// ring colour `want` (anti-aliasing allowed for).
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "test points are inside the canvas"
    )]
    fn ring_hits(pm: &Pixmap, at: (f32, f32), radius: f32, want: Rgba, n: u16) -> Result<u16> {
        let mut hits = 0;
        for i in 0..n {
            let angle = f32::from(i) * std::f32::consts::TAU / f32::from(n);
            let x = radius.mul_add(angle.cos(), at.0).floor() as u32;
            let y = radius.mul_add(angle.sin(), at.1).floor() as u32;
            let p = rgba(pm, x, y)?;
            let close = p.iter().zip(want).take(3).all(|(&a, b)| a.abs_diff(b) <= 60);
            if close && p[3] >= 150 {
                hits += 1;
            }
        }
        Ok(hits)
    }

    #[test]
    fn rings_come_after_drops_and_the_player() {
        let mut snap = empty_snapshot(5, 5);
        snap.drops = vec![
            highlighted(DropKind::Base, 1, Tier::A, false),
            drop_at(DropKind::Rune(1), 2, 2),
        ];
        let kinds: Vec<Marker> = markers(&snap).into_iter().map(|(m, _, _)| m).collect();
        assert_eq!(
            kinds,
            vec![
                Marker::Player,
                Marker::Drop(DropKind::Base),
                Marker::Drop(DropKind::Rune(1)),
                Marker::Ring {
                    kind: DropKind::Base,
                    tier: Tier::A,
                    uncertain: false
                },
            ]
        );
        let (white, size, shape) = Marker::Drop(DropKind::Base).style();
        assert_eq!(
            (white, shape),
            (BASE_WHITE, Shape::Diamond),
            "bases: a white diamond"
        );
        assert!(
            size < Marker::Drop(DropKind::Unique).style().1,
            "smaller than a unique"
        );
    }

    #[test]
    fn rings_are_coloured_by_tier_and_dashed_when_uncertain() -> Result<()> {
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, false), room(60, 0, true)];
        snap.drops = vec![
            highlighted(DropKind::Unique, 20, Tier::S, false),
            highlighted(DropKind::Base, 60, Tier::A, false),
            highlighted(DropKind::Base, 100, Tier::B, false),
            highlighted(DropKind::Set, 140, Tier::S, true),
        ];
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let expect = [
            (20.0, DropKind::Unique, [230, 40, 40, 255]),
            (60.0, DropKind::Base, [255, 140, 0, 255]),
            (100.0, DropKind::Base, [255, 220, 0, 255]),
        ];
        for (x, kind, colour) in expect {
            let at = v.to_screen(x, 30.0);
            let radius = Marker::Drop(kind).style().1 + RING_GAP;
            let hits = ring_hits(&pm, at, radius, colour, 72)?;
            assert!(hits >= 68, "solid {colour:?} ring at x {x}: {hits}/72");
            // Inside the gap between marker and ring: not ring-coloured.
            let inside = ring_hits(&pm, at, radius - 2.5, colour, 72)?;
            assert!(inside <= 4, "ring colour inside the ring at x {x}: {inside}");
        }
        let at = v.to_screen(140.0, 30.0);
        let radius = Marker::Drop(DropKind::Set).style().1 + RING_GAP;
        let hits = ring_hits(&pm, at, radius, [230, 40, 40, 255], 72)?;
        assert!((20..=60).contains(&hits), "dashed ring: {hits}/72 on the ring");
        Ok(())
    }

    #[test]
    fn rune_and_gem_markers_render_where_the_drop_is() -> Result<()> {
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, false), room(60, 0, true)];
        snap.drops = vec![
            drop_at(DropKind::Rune(30), 30, 30),
            drop_at(DropKind::Gem(GemColour::Sapphire), 90, 30),
        ];
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);

        let (rune, half, shape) = Marker::Drop(DropKind::Rune(30)).style();
        assert_eq!(shape, Shape::Square);
        let at = v.to_screen(30.0, 30.0);
        let (hits, dx, dy) = colour_spread(&pm, rune, at)?;
        assert!(hits >= 9, "only {hits} rune pixels");
        assert!(
            dx <= f64::from(half) && dy <= f64::from(half),
            "rune spread {dx},{dy}"
        );
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "test points are inside the canvas"
        )]
        let centre = |(x, y): (f32, f32)| (x.floor() as u32, y.floor() as u32);
        let (rx, ry) = centre(at);
        assert_eq!(rgba(&pm, rx, ry)?, rune, "rune centre");
        // The dark outline sits on the square's edge.
        let (edge_x, _) = centre((at.0 + half, at.1));
        let edge = rgba(&pm, edge_x, ry)?;
        assert!(edge[0] < 150 && edge[3] > 150, "outline at the edge: {edge:?}");

        let (gem, size, _) = Marker::Drop(DropKind::Gem(GemColour::Sapphire)).style();
        let at = v.to_screen(90.0, 30.0);
        let (hits, dx, dy) = colour_spread(&pm, gem, at)?;
        assert!(hits >= 4, "only {hits} gem pixels");
        assert!(
            dx + dy <= f64::from(size) + 1.0,
            "gem pixels outside the diamond: {dx},{dy}"
        );
        let (gx, gy) = centre(at);
        assert_eq!(rgba(&pm, gx, gy)?, gem, "gem centre");
        Ok(())
    }

    const fn charm(size: CharmSize, unique: bool) -> DropKind {
        DropKind::Charm { size, unique }
    }

    #[test]
    fn charm_styles() {
        let (entry, entry_half, _) = Marker::Entry.style();
        let (rune, rune_half, _) = Marker::Drop(DropKind::Rune(1)).style();
        let mut last = 0.0;
        for size in [CharmSize::Small, CharmSize::Large, CharmSize::Grand] {
            let (colour, half, shape) = Marker::Drop(charm(size, false)).style();
            assert_eq!((colour, shape), (CHARM_BLUE, Shape::Square), "{size:?}");
            assert_ne!(colour, entry);
            assert_ne!(colour, rune);
            assert!(
                half > last && half < entry_half && half <= rune_half,
                "{size:?}: {half}"
            );
            last = half;
            let (gold, unique_half, unique_shape) = Marker::Drop(charm(size, true)).style();
            assert_eq!(
                gold,
                Marker::Drop(DropKind::Unique).style().0,
                "unique charm colour"
            );
            assert_eq!((unique_half, unique_shape), (half, shape), "unique charm shape");
        }
    }

    #[test]
    fn charm_markers_render_where_the_drop_is() -> Result<()> {
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, false), room(60, 0, true)];
        snap.drops = vec![
            drop_at(charm(CharmSize::Grand, false), 30, 30),
            drop_at(charm(CharmSize::Small, true), 90, 30),
        ];
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "test points are inside the canvas"
        )]
        let centre = |(x, y): (f32, f32)| (x.floor() as u32, y.floor() as u32);

        for (kind, (x, y)) in [
            (charm(CharmSize::Grand, false), (30.0, 30.0)),
            (charm(CharmSize::Small, true), (90.0, 30.0)),
        ] {
            let (colour, half, _) = Marker::Drop(kind).style();
            let at = v.to_screen(x, y);
            let (hits, dx, dy) = colour_spread(&pm, colour, at)?;
            assert!(hits >= 4, "only {hits} pixels of {kind:?}");
            assert!(
                dx <= f64::from(half) && dy <= f64::from(half),
                "{kind:?} spread {dx},{dy}"
            );
            let (cx, cy) = centre(at);
            assert_eq!(rgba(&pm, cx, cy)?, colour, "{kind:?} centre");
            let (edge_x, _) = centre((at.0 + half, at.1));
            let edge = rgba(&pm, edge_x, cy)?;
            assert!(edge[0] < 150 && edge[3] > 150, "outline at the edge: {edge:?}");
        }
        Ok(())
    }

    /// Light grey to white: the info line's glyph cores (13 px strokes are thin, so few are pure white).
    fn is_text_white(p: Rgba) -> bool {
        p.iter().all(|&c| c >= 200)
    }

    #[test]
    fn info_line_is_drawn_in_the_bottom_band_only() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let mut snap = fixture()?;
        snap.difficulty = Some(0);
        let (w, h) = (400, 400);
        let pm = render_with(&snap, w, h, Some(font))?;
        let top = map_height(h, Some(font), 0);
        assert!(top < h && h - top < 40, "band {top}..{h}");
        let (mut band, mut above) = (0, 0);
        for y in 0..h {
            for x in 0..w {
                if is_text_white(rgba(&pm, x, y)?) {
                    if y >= top { band += 1 } else { above += 1 }
                }
            }
        }
        assert!(band >= 50, "only {band} white text pixels in the band");
        assert_eq!(above, 0, "white pixels above the band");
        // The dark outline is there too: some opaque, dark pixels in the band.
        let mut dark = 0;
        for y in top..h {
            for x in 0..w {
                let p = rgba(&pm, x, y)?;
                if p[3] >= 200 && p[0] < 40 {
                    dark += 1;
                }
            }
        }
        assert!(dark >= 50, "only {dark} outline pixels");
        // Without a font: no text, and the map gets the whole canvas.
        let plain = render_with(&snap, w, h, None)?;
        assert_eq!(map_height(h, None, 0), h);
        for y in top..h {
            for x in 0..w {
                assert!(!is_text_white(rgba(&plain, x, y)?), "{x},{y}");
            }
        }
        Ok(())
    }

    #[test]
    fn terrorized_info_line_has_violet_text() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        // Glyph cores of the violet part (190, 120, 255); anti-aliased edges blend into the
        // dark outline, so only fairly opaque, blue-heavy pixels with red over green count.
        let violet = |p: Rgba| p[3] >= 200 && p[2] >= 180 && p[0] >= 120 && p[0].saturating_sub(p[1]) >= 40;
        let (w, h) = (400, 400);
        // The info band only: map markers may be violet-ish too.
        let top = map_height(h, Some(font), 0);
        let count = |pm: &Pixmap| -> Result<u32> {
            let mut n = 0;
            for y in top..pm.height() {
                for x in 0..pm.width() {
                    n += u32::from(violet(rgba(pm, x, y)?));
                }
            }
            Ok(n)
        };
        let mut snap = fixture()?;
        snap.difficulty = Some(0);
        assert_eq!(
            count(&render_with(&snap, w, h, Some(font))?)?,
            0,
            "not terrorized"
        );
        snap.terrorized = true;
        let pm = render_with(&snap, w, h, Some(font))?;
        assert!(count(&pm)? >= 20, "only {} violet pixels", count(&pm)?);
        Ok(())
    }

    #[test]
    fn map_fit_leaves_the_text_band_free() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let snap = fixture()?;
        let (w, h) = (400, 400);
        let top = map_height(h, Some(font), 0);
        let v = View::fit(level_bounds(&snap), w, top);
        for r in &snap.level.rooms {
            for (x, y) in [
                (r.x, r.y),
                (r.x + r.w, r.y),
                (r.x + r.w, r.y + r.h),
                (r.x, r.y + r.h),
            ] {
                let (_, sy) = v.to_screen(f64::from(x), f64::from(y));
                assert!(sy <= px(f64::from(top)), "room corner at y {sy}, band from {top}");
            }
        }
        // In the drawn pixmap the band holds no room fill: every non-transparent pixel there is
        // text (white, grey edges) or its outline (black), never the rooms' blue-grey.
        let pm = render_with(&snap, w, h, Some(font))?;
        for y in top..h {
            for x in 0..w {
                let p = rgba(&pm, x, y)?;
                let grey = p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1;
                assert!(p[3] == 0 || grey, "non-text pixel {p:?} at {x},{y}");
            }
        }
        Ok(())
    }

    #[test]
    fn render_rejects_zero_size() {
        assert!(render(&empty_snapshot(1, 1), 0, 10).is_err());
    }

    #[test]
    fn entry_renders_as_a_teal_square_apart_from_exits() -> Result<()> {
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, false), room(60, 0, true)];
        snap.presets = vec![
            Preset {
                kind: PresetKind::Tile,
                txt: 1,
                x: 30,
                y: 30,
                entry: true,
            },
            Preset {
                kind: PresetKind::Tile,
                txt: 2,
                x: 90,
                y: 30,
                entry: false,
            },
        ];
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);

        let (entry, half, shape) = Marker::Entry.style();
        assert_eq!(shape, Shape::Square);
        assert_ne!(entry, Marker::Exit.style().0);
        assert_ne!(
            Marker::Waypoint.style().2,
            Shape::Square,
            "entry unlike the waypoint"
        );
        let at = v.to_screen(30.0, 30.0);
        let (hits, dx, dy) = colour_spread(&pm, entry, at)?;
        assert!(hits >= 25, "only {hits} entry pixels");
        assert!(
            dx <= f64::from(half) && dy <= f64::from(half),
            "entry spread {dx},{dy}"
        );
        // The exit keeps its colour, at its own spot only.
        let (exit, radius, _) = Marker::Exit.style();
        let at = v.to_screen(90.0, 30.0);
        let (hits, dx, dy) = colour_spread(&pm, exit, at)?;
        assert!(hits >= 9, "only {hits} exit pixels");
        assert!(dx.hypot(dy) <= f64::from(radius) * 1.5, "exit spread {dx},{dy}");
        Ok(())
    }

    /// Two 40x40 rooms of this level side by side, each bordering a room of `to` above it (smaller
    /// y), plus links to a far level 30, a tile-linked level 29 and an entry level 26 below.
    fn bordered(to: u32) -> Snapshot {
        let rect = |x, y| RoomRect { x, y, w: 40, h: 40 };
        let link = |from: RoomRect, to_level_id, to_room, entry| BorderLink {
            from_room: from,
            to_level_id,
            to_room,
            entry,
        };
        let (a, b) = (rect(100, 100), rect(140, 100));
        let mut snap = empty_snapshot(120, 120);
        snap.level.rooms = [a, b]
            .iter()
            .map(|r| Room {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                loaded: true,
                preset: None,
            })
            .collect();
        snap.border_links = vec![
            link(a, to, rect(100, 60), false),
            link(a, to, rect(140, 60), false),
            link(b, to, rect(140, 60), false),
            link(a, 30, rect(1000, 1000), false),
            link(b, 29, rect(180, 100), false),
            link(b, 26, rect(140, 140), true),
        ];
        snap.room_links = vec![RoomLink {
            from_room: RoomPos { x: 140, y: 100 },
            to_level_id: 29,
            to_room: RoomPos { x: 180, y: 100 },
        }];
        snap
    }

    #[test]
    fn one_border_exit_per_touching_level() {
        let got = border_exits(&bordered(28));
        assert_eq!(
            got,
            vec![
                // Level 26 below room b: entry, pointing +y.
                (
                    Marker::BorderExit {
                        dir: (0, 80),
                        entry: true
                    },
                    160.0,
                    140.5
                ),
                // Level 28 above both rooms: one marker at the middle of the shared edge, pointing -y.
                (
                    Marker::BorderExit {
                        dir: (0, -80),
                        entry: false
                    },
                    140.0,
                    99.5
                ),
            ],
            "far level 30 and tile-linked level 29 get none"
        );
    }

    #[test]
    fn border_exit_triangle_points_outward_on_screen() -> Result<()> {
        // World -y projects to screen up-right: iso(0, -1) = (1, -0.5).
        let dir = Marker::BorderExit {
            dir: (0, -80),
            entry: false,
        };
        let (_, size, shape) = dir.style();
        assert_eq!(shape, Shape::Triangle);
        let path = marker_outline(dir, shape, 50.0, 50.0, size).context("triangle")?;
        let points = path.points();
        let tip = points.first().context("tip")?;
        let (tx, ty) = (tip.x - 50.0, tip.y - 50.0);
        assert!((tx.hypot(ty) - size).abs() < 1e-4, "tip {tx},{ty}");
        assert!(
            (ty / tx + 0.5).abs() < 1e-4 && tx > 0.0,
            "tip {tx},{ty} not up-right"
        );
        assert!(
            points
                .iter()
                .skip(1)
                .all(|p| (p.x - 50.0).hypot(p.y - 50.0) < size),
            "the tip is the farthest point"
        );
        Ok(())
    }

    #[test]
    fn border_exits_render_purple_and_entry_teal() -> Result<()> {
        let snap = bordered(28);
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);
        for (entry, x, y) in [(false, 140.0, 99.5), (true, 160.0, 140.5)] {
            let (colour, size, _) = Marker::BorderExit { dir: (0, 1), entry }.style();
            assert_eq!(colour, if entry { Marker::Entry } else { Marker::Exit }.style().0);
            let (hits, dx, dy) = colour_spread(&pm, colour, v.to_screen(x, y))?;
            assert!(hits >= 4, "only {hits} pixels (entry {entry})");
            assert!(
                dx.hypot(dy) <= f64::from(size) * 1.5,
                "spread {dx},{dy} (entry {entry})"
            );
        }
        Ok(())
    }

    /// Level 74 with four 60x60 rooms in a row along x, carrying the given preset ids.
    fn sanctuary(presets: [Option<u32>; 4]) -> Snapshot {
        let mut snap = empty_snapshot(5, 115);
        snap.level.id = 74;
        snap.level.rooms = presets
            .iter()
            .zip(0u32..)
            .map(|(&preset, i)| Room {
                preset,
                ..room(i * 60, 0, false)
            })
            .collect();
        snap
    }

    #[test]
    fn orifice_rule_marks_the_real_tomb_chamber_only() {
        let orifice = Marker::Landmark("orifice");
        // Real tomb 72, orifice chamber 462 (confirmed in game).
        let mut snap = sanctuary([Some(462), Some(424), None, None]);
        snap.level.id = 72;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(orifice, 30.0, 30.0)]);
        // The chamber spans several Room2 cells: one star at their combined centre.
        let mut snap = sanctuary([Some(462), Some(462), Some(424), None]);
        snap.level.id = 72;
        snap.level.rooms.extend([
            Room {
                preset: Some(462),
                ..room(0, 60, false)
            },
            Room {
                preset: Some(462),
                ..room(60, 60, false)
            },
        ]);
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(orifice, 60.0, 60.0)]);
        // Fake tomb 71: its chest chamber 473 is no landmark.
        let mut snap = sanctuary([Some(473), Some(424), None, None]);
        snap.level.id = 71;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![]);
        // Tomb 66 with 456: only the assumed level+390 pattern says so, a "?".
        let mut snap = sanctuary([None, Some(456), None, None]);
        snap.level.id = 66;
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(
                Marker::Suspected("orifice (assumed level+390 pattern)"),
                90.0,
                30.0
            )]
        );
        // 462 outside tomb 72 is not confirmed and not in the assumed range either.
        let mut snap = sanctuary([None, Some(462), None, None]);
        snap.level.id = 66;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![]);
        // The orifice id outside the tombs is nothing either.
        let mut snap = sanctuary([Some(462), None, None, None]);
        snap.level.id = 73;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![]);
    }

    #[test]
    fn summoner_rule_marks_the_room_centre() {
        // Every Summoner arm-end preset is matched, at the centre of its room.
        let summoner = Marker::Landmark("summoner");
        for id in 525..=528 {
            let snap = sanctuary([None, Some(id), None, None]);
            assert_eq!(landmarks(&snap, LANDMARKS), vec![(summoner, 90.0, 30.0)], "{id}");
        }
        // Several matching rooms make one star, at the centre of their bounding box.
        let snap = sanctuary([Some(525), None, None, Some(528)]);
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(summoner, 120.0, 30.0)]);
        // Ordinary arm ends, the neighbouring ids and unknown presets: nothing.
        let snap = sanctuary([Some(510), Some(524), Some(529), None]);
        assert_eq!(landmarks(&snap, LANDMARKS), vec![]);
        // The same preset ids in another level: nothing.
        let mut snap = sanctuary([Some(525), None, None, None]);
        snap.level.id = 73;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![]);
        // It is part of the drawn markers, below elites and the player.
        snap.level.id = 74;
        snap.monsters = vec![monster(0x02, 1)];
        let kinds: Vec<Marker> = markers(&snap).into_iter().map(|(m, _, _)| m).collect();
        assert_eq!(kinds, vec![summoner, Marker::Elite, Marker::Player]);
    }

    #[test]
    fn gidbinn_rule_marks_the_altar_piece() {
        let gidbinn = Marker::Landmark("gidbinn");
        let mut snap = sanctuary([Some(601), Some(602), Some(602), None]);
        snap.level.id = 78;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(gidbinn, 120.0, 30.0)]);
        // The waypoint area 601 alone: nothing.
        let mut snap = sanctuary([Some(601), None, None, None]);
        snap.level.id = 78;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![]);
        // 602 in another level: nothing.
        let mut snap = sanctuary([None, Some(602), None, None]);
        snap.level.id = 77;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![]);
    }

    #[test]
    fn sewer_stairs_rule_marks_piece_739_in_sewers_1() {
        let stairs = Marker::Landmark("sewer stairs");
        let mut snap = sanctuary([None, Some(739), None, None]);
        snap.level.id = 92;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(stairs, 90.0, 30.0)]);
        // 739 in another level: nothing.
        snap.level.id = 93;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![]);
    }

    #[test]
    fn summoner_renders_as_a_haloed_gold_star() -> Result<()> {
        let snap = sanctuary([None, Some(527), None, None]);
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let at = v.to_screen(90.0, 30.0);
        let (gold, size, shape) = Marker::Landmark("summoner").style();
        assert_eq!(shape, Shape::Star);
        // Unlike every other marker: its own colour, bigger than all of them.
        for other in [
            Marker::Exit,
            Marker::Entry,
            Marker::Waypoint,
            Marker::Special(None),
            Marker::Elite,
            Marker::Drop(DropKind::Unique),
            Marker::Drop(DropKind::Set),
            Marker::Player,
        ] {
            let (colour, other_size, _) = other.style();
            assert_ne!(colour, gold, "{other:?}");
            assert!(other_size < size, "{other:?}");
        }
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "test points are inside the canvas"
        )]
        // The pixel whose centre is nearest to the point.
        let pixel = |dx: f32, dy: f32| ((at.0 + dx - 0.5).round() as u32, (at.1 + dy - 0.5).round() as u32);

        let (cx, cy) = pixel(0.0, 0.0);
        assert_eq!(rgba(&pm, cx, cy)?, gold, "star centre");
        let (tx, ty) = pixel(0.0, 3.0 - size);
        assert_eq!(rgba(&pm, tx, ty)?, gold, "top tip");
        // Dark outline just above the top tip.
        let (ox, oy) = pixel(0.0, -size - 1.0);
        let outline = rgba(&pm, ox, oy)?;
        assert!(
            outline[0] < 100 && outline[3] > 150,
            "outline above the tip: {outline:?}"
        );
        // Between star and halo: no gold (the room shows through).
        let (gx, gy) = pixel(0.0, size + 3.0);
        assert_ne!(rgba(&pm, gx, gy)?, gold, "gap below the star");
        // The gold halo ring around it, on all four sides.
        for (dx, dy) in [
            (LANDMARK_RING, 0.0),
            (-LANDMARK_RING, 0.0),
            (0.0, LANDMARK_RING),
            (0.0, -LANDMARK_RING),
        ] {
            let (hx, hy) = pixel(dx, dy);
            let p = rgba(&pm, hx, hy)?;
            assert!(p[0] > 200 && p[1] > 140 && p[2] < 120, "halo at {dx},{dy}: {p:?}");
        }
        // All gold pixels belong to this marker.
        let (hits, dx, dy) = colour_spread(&pm, gold, at)?;
        assert!(hits >= 60, "only {hits} gold pixels");
        assert!(
            dx.hypot(dy) <= f64::from(LANDMARK_RING) * 1.5,
            "gold spread {dx},{dy}"
        );
        Ok(())
    }

    #[test]
    fn unconfirmed_rules_are_the_known_list() {
        let unconfirmed: Vec<&str> = LANDMARKS
            .iter()
            .filter(|r| !r.confirmed)
            .map(|r| r.label)
            .collect();
        assert_eq!(
            unconfirmed,
            vec![
                "smith (forge W/E)",
                "smith (forge N)",
                "cave exit to the next area",
                "cave stairs down",
                "orifice (assumed level+390 pattern)"
            ]
        );
    }

    fn object(txt: u32, x: u32, y: u32) -> Preset {
        Preset {
            kind: PresetKind::Object,
            txt,
            x,
            y,
            entry: false,
        }
    }

    #[test]
    fn quest_objects_are_markers_with_labels() {
        for (txt, label) in [
            (152, "orifice"),
            (252, "gidbinn altar"),
            (366, "sewer stairs"),
            (367, "sewer lever"),
            (376, "hellforge"),
            (392, "diablo seal"),
            (396, "diablo seal"),
            (460, "anya"),
        ] {
            assert_eq!(
                preset_marker(&object(txt, 0, 0)),
                Some(Marker::QuestObject(label)),
                "{txt}"
            );
        }
        // Other objects: nothing; waypoints stay waypoints.
        for txt in [0, 151, 153, 365, 368, 375, 377, 391, 397, 473] {
            assert_eq!(preset_marker(&object(txt, 0, 0)), None, "{txt}");
        }
        assert_eq!(preset_marker(&object(237, 0, 0)), Some(Marker::Waypoint));
        // A quest-object id that is a monster or tile preset is not a quest object.
        let monster = Preset {
            kind: PresetKind::Monster,
            ..object(152, 0, 0)
        };
        assert_eq!(preset_marker(&monster), Some(Marker::Special(None)));
        // A quest monster preset (prison door) is drawn as a quest object; as an object it's not.
        // Siege barricades (432, 433, 435) stay generic monster presets.
        let door = Preset {
            kind: PresetKind::Monster,
            ..object(434, 0, 0)
        };
        assert_eq!(preset_marker(&door), Some(Marker::QuestObject("prison door")));
        assert_eq!(preset_marker(&object(434, 0, 0)), None);
        let nihlathak = Preset {
            kind: PresetKind::Monster,
            ..object(526, 0, 0)
        };
        assert_eq!(preset_marker(&nihlathak), Some(Marker::QuestObject("nihlathak")));
        for barricade in [432, 433, 435] {
            let p = Preset {
                kind: PresetKind::Monster,
                ..object(barricade, 0, 0)
            };
            assert_eq!(preset_marker(&p), Some(Marker::Special(None)), "{barricade}");
        }
    }

    /// Tomb 66 with an assumed-orifice room (456) at world 90,30 and the orifice object at 150,30.
    fn tomb_66() -> Snapshot {
        let mut snap = sanctuary([None, Some(456), None, None]);
        snap.level.id = 66;
        snap.presets = vec![object(152, 150, 30)];
        snap
    }

    /// Pixels within `r` px of `at` for which `pred` holds.
    fn count_near(pm: &Pixmap, at: (f32, f32), r: f32, pred: impl Fn(Rgba) -> bool) -> Result<u32> {
        let mut n = 0;
        for y in 0..pm.height() {
            for x in 0..pm.width() {
                let (dx, dy) = (px(f64::from(x)) + 0.5 - at.0, px(f64::from(y)) + 0.5 - at.1);
                if dx.hypot(dy) <= r && pred(rgba(pm, x, y)?) {
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    /// The "?" glyph's light gold, also partly covered over the dark disc: bright red and green,
    /// more blue than the landmark gold, warmer than the room grey.
    fn is_suspect_text(p: Rgba) -> bool {
        p[0] >= 170 && p[1] >= 150 && p[2] >= 80 && p[0] >= p[2] + 40
    }

    #[test]
    fn suspected_landmark_renders_as_a_question_mark() -> Result<()> {
        let snap = tomb_66();
        let (w, h) = (400, 400);
        // Without a font: a hollow gold ring, no glyph.
        let plain = render_with(&snap, w, h, None)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let at = v.to_screen(90.0, 30.0);
        assert_eq!(count_near(&plain, at, SUSPECT_DISC_RADIUS, is_suspect_text)?, 0);
        let (_, radius, _) = Marker::Suspected("").style();
        let ring = count_near(&plain, at, radius + 1.5, |p| p == LANDMARK_GOLD)?;
        let centre = count_near(&plain, at, radius - 2.5, |p| p == LANDMARK_GOLD)?;
        assert!(ring >= 15 && centre == 0, "ring {ring}, centre {centre}");

        let Some(font) = info::font() else {
            eprintln!("skipped the glyph check: no font on this machine");
            return Ok(());
        };
        let pm = render_with(&snap, w, h, Some(font))?;
        let v = View::fit(level_bounds(&snap), w, map_height(h, Some(font), 0));
        let at = v.to_screen(90.0, 30.0);
        let near = count_near(&pm, at, SUSPECT_DISC_RADIUS, is_suspect_text)?;
        assert!(near >= 15, "only {near} light glyph pixels near the ?");
        // No landmark star there: no gold.
        let star_gold = count_near(&pm, at, LANDMARK_RING + 2.0, |p| p == LANDMARK_GOLD)?;
        assert_eq!(star_gold, 0, "gold star pixels at the ?");
        // The disc darkens the room behind the glyph: some dark opaque pixels inside it.
        let dark = count_near(&pm, at, SUSPECT_DISC_RADIUS, |p| p[3] >= 150 && p[0] < 50)?;
        assert!(dark >= 30, "only {dark} dark disc/outline pixels");
        Ok(())
    }

    #[test]
    fn quest_object_renders_as_a_small_gold_star() -> Result<()> {
        // The orifice object alone (no "?" room, whose no-font ring is gold too).
        let mut snap = tomb_66();
        for r in &mut snap.level.rooms {
            r.preset = None;
        }
        let (w, h) = (400, 400);
        let pm = render_with(&snap, w, h, None)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let at = v.to_screen(150.0, 30.0);
        let (gold, size, shape) = Marker::QuestObject("orifice").style();
        assert_eq!((gold, shape), (LANDMARK_GOLD, Shape::Star));
        assert!(
            size < Marker::Landmark("orifice").style().1,
            "smaller than the landmark star"
        );
        let (hits, dx, dy) = colour_spread(&pm, gold, at)?;
        assert!(hits >= 12, "only {hits} gold pixels");
        assert!(dx.hypot(dy) <= f64::from(size), "gold spread {dx},{dy}: no halo");
        // Drawn after (above) the landmark star, so both show when they overlap.
        let mut snap = sanctuary([None, Some(462), None, None]);
        snap.level.id = 72;
        snap.presets = vec![object(152, 90, 30)];
        let kinds: Vec<Marker> = markers(&snap).into_iter().map(|(m, _, _)| m).collect();
        assert_eq!(
            kinds,
            vec![
                Marker::Landmark("orifice"),
                Marker::QuestObject("orifice"),
                Marker::Player
            ]
        );
        Ok(())
    }

    fn shrine_at(code: u8, x: u32, y: u32) -> crate::game::Shrine {
        crate::game::Shrine {
            txt: 236,
            code,
            used: false,
            x,
            y,
        }
    }

    fn well_at(x: u32, y: u32) -> crate::game::Well {
        crate::game::Well { txt: 130, x, y }
    }

    #[test]
    fn shrines_and_wells_sit_above_landmarks_and_below_elites() {
        let mut snap = sanctuary([None, Some(525), None, None]);
        snap.presets = vec![object(152, 90, 30)];
        snap.monsters = vec![monster(0x08, 1)];
        snap.drops = vec![drop_at(DropKind::Unique, 1, 1)];
        snap.shrines = vec![shrine_at(12, 40, 30)];
        snap.wells = vec![well_at(50, 30)];
        let kinds: Vec<Marker> = markers(&snap).into_iter().map(|(m, _, _)| m).collect();
        assert_eq!(
            kinds,
            vec![
                Marker::Landmark("summoner"),
                Marker::QuestObject("orifice"),
                Marker::Well,
                Marker::Shrine(12),
                Marker::Elite,
                Marker::Player,
                Marker::Drop(DropKind::Unique),
            ]
        );
    }

    #[test]
    fn default_shrine_colours_are_distinct() {
        let others = [
            Marker::Exit,
            Marker::Waypoint,
            Marker::Elite,
            Marker::Player,
            Marker::Well,
            Marker::Shrine(2),
        ]
        .map(|m| m.style().0);
        let defaults = [1, 12, 14, 18, 20].map(|c| Marker::Shrine(c).style().0);
        for (i, c) in defaults.iter().enumerate() {
            assert!(!others.contains(c), "{c:?} reused");
            assert!(!defaults.iter().skip(i + 1).any(|d| d == c), "{c:?} twice");
        }
        assert_eq!(
            Marker::Shrine(2).style().0,
            Marker::Shrine(99).style().0,
            "neutral"
        );
    }

    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "test points are inside the canvas"
    )]
    fn centre((x, y): (f32, f32)) -> (u32, u32) {
        (x.floor() as u32, y.floor() as u32)
    }

    #[test]
    fn shrine_renders_as_a_white_outlined_dot() -> Result<()> {
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, true)];
        snap.shrines = vec![shrine_at(12, 30, 30)];
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let at = v.to_screen(30.0, 30.0);
        let (fill, radius, shape) = Marker::Shrine(12).style();
        assert_eq!(shape, Shape::Circle);
        let (cx, cy) = centre(at);
        assert_eq!(rgba(&pm, cx, cy)?, fill, "skill shrine centre");
        let (hits, dx, dy) = colour_spread(&pm, fill, at)?;
        assert!(hits >= 12, "only {hits} shrine pixels");
        assert!(dx.hypot(dy) <= f64::from(radius), "spread {dx},{dy}");
        let white = ring_hits(&pm, at, radius, SHRINE_OUTLINE, 16)?;
        assert!(white >= 12, "white outline: {white}/16");
        Ok(())
    }

    #[test]
    fn well_renders_as_a_hollow_cyan_ring() -> Result<()> {
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, true)];
        snap.wells = vec![well_at(30, 30)];
        let (w, h) = (400, 400);
        let pm = render(&snap, w, h)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let at = v.to_screen(30.0, 30.0);
        let (_, radius, _) = Marker::Well.style();
        let cyan = ring_hits(&pm, at, radius, WELL_CYAN, 16)?;
        assert!(cyan >= 12, "cyan ring: {cyan}/16");
        let (cx, cy) = centre(at);
        let inside = rgba(&pm, cx, cy)?;
        assert!(inside[1] < 200, "hollow: the room shows through, {inside:?}");
        Ok(())
    }
}
