//! Draws a [`Snapshot`] as an isometric corner minimap into a tiny-skia [`Pixmap`].
//!
//! Pure: no I/O. The `--render` CLI writes the pixmap as PNG; the overlay copies it into a
//! Wayland shm buffer.

use ab_glyph::FontVec;
use anyhow::{Context, Result};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, StrokeDash, Transform,
};

use crate::game::{Bounds, Monster, Preset, PresetKind, Room, RoomRect, Snapshot};
use crate::guidance::Tier;
use crate::guidance::{Action, Advice, Side};
use crate::guidance::{Category, Guidance, Mark, MarkKind, Marks};
use crate::items::{CharmSize, DropKind, GemColour};
use crate::objects::{SUPER_CHEST_LEVELS, SUPER_CHEST_PRESET};
use crate::offsets::monster_data;
use crate::{info, session, tombs, xp};

/// Exit labels in an area that gives experience.
const LABEL_TEXT: [u8; 4] = [255, 255, 255, 255];

/// Object txt ids of waypoints: every waypoint row of classic objects.txt.
pub const WAYPOINT_IDS: &[u32] = &[
    119, 145, 156, 157, 237, 238, 288, 323, 324, 398, 402, 429, 494, 496, 511, 539,
];
/// A landmark found by room preset: in a level of `level_ids`, each piece ([`pieces`]) of rooms
/// whose lvlprest id is in `preset_ids` gets one marker at its centre.
#[derive(Debug)]
pub struct LandmarkRule {
    pub level_ids: std::ops::RangeInclusive<u32>,
    pub preset_ids: std::ops::RangeInclusive<u32>,
    /// What the landmark is (not drawn).
    pub label: &'static str,
    /// Seen in game; an unconfirmed rule draws a "?" ([`Marker::Suspected`]).
    pub confirmed: bool,
    /// How a confirmed rule is drawn.
    pub style: LandmarkStyle,
    /// `Some(txt)`: the pieces are only candidates for where quest object `txt` lies; see
    /// [`ruled_out`]. Once the object's preset is loaded its own marker replaces them all.
    pub candidates_for: Option<u32>,
}

/// How a confirmed [`LandmarkRule`] is drawn (an unconfirmed one is always a "?").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandmarkStyle {
    /// A gold landmark star ([`Marker::Landmark`]).
    Star,
    /// Stairs down to a deeper level ([`Marker::StairsDown`]).
    StairsDown,
    /// A gold worm ([`Marker::Worm`]).
    Worm,
    /// A mini orifice ([`Marker::Orifice`]).
    Orifice,
}

/// The landmark rules. The W/E/S/N variants of a lvlprest family ("Cave Next W/E/S/N") count as
/// confirmed once one of them is: the letter only says where in the piece the feature sits.
pub const LANDMARKS: &[LandmarkRule] = &[
    LandmarkRule {
        level_ids: 62..=63,
        preset_ids: 501..=504,
        label: "maggot lair stairs down",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 64..=64,
        preset_ids: 509..=509,
        label: "coldworm",
        confirmed: true,
        style: LandmarkStyle::Worm,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 49..=49,
        preset_ids: 341..=341,
        label: "radament's lair",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 49..=49,
        preset_ids: 344..=344,
        label: "radament's lair (N)",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 49..=49,
        preset_ids: 343..=343,
        label: "radament's lair (S)",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 49..=49,
        preset_ids: 342..=342,
        label: "radament's lair (E)",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 47..=48,
        preset_ids: 338..=338,
        label: "sewer stairs down (east)",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 47..=48,
        preset_ids: 337..=337,
        label: "sewer stairs down (west)",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 47..=48,
        preset_ids: 339..=340,
        label: "sewer stairs down (S/N)",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 203..=204,
        label: "smith (forge)",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 202..=202,
        label: "smith (forge W)",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 205..=205,
        label: "smith (forge N)",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 4..=4,
        preset_ids: 160..=160,
        label: "cairn stones",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 90..=90,
        label: "cave exit to the next area (north)",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 87..=89,
        label: "cave exit to the next area",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 91..=91,
        label: "cave stairs down (west)",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 92..=94,
        label: "cave stairs down",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 5..=5,
        preset_ids: 161..=161,
        label: "tree of inifuss",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 6..=6,
        preset_ids: 163..=163,
        label: "forgotten tower",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 74..=74,
        preset_ids: 525..=528,
        label: "summoner",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 462..=462,
        label: "orifice",
        confirmed: true,
        style: LandmarkStyle::Orifice,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 472..=475,
        label: "tomb treasure",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 60..=60,
        preset_ids: 459..=459,
        label: "horadric cube",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 60..=60,
        preset_ids: 456..=458,
        label: "horadric cube (W/E/S)",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 460..=460,
        label: "orifice (W)",
        confirmed: true,
        style: LandmarkStyle::Orifice,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 461..=461,
        label: "orifice (E)",
        confirmed: true,
        style: LandmarkStyle::Orifice,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 463..=463,
        label: "orifice (N)",
        confirmed: true,
        style: LandmarkStyle::Orifice,
        candidates_for: None,
    },
    LandmarkRule {
        level_ids: 78..=78,
        preset_ids: 595..=604,
        label: "gidbinn",
        // The altar lies in one of the pygmy clearings, a different one each game.
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: Some(252),
    },
    LandmarkRule {
        level_ids: 92..=92,
        preset_ids: 739..=739,
        label: "sewer stairs",
        confirmed: true,
        style: LandmarkStyle::Star,
        candidates_for: None,
    },
];

/// Quest objects (objects.txt ids) drawn as a small star where their preset is.
pub const QUEST_OBJECTS: &[(u32, &str)] = &[
    (108, "horadric malus"),
    (354, "horadric cube chest"),
    (356, "staff of kings chest"),
    (152, "orifice"),
    (252, "gidbinn altar"),
    (366, "sewer stairs"),
    (367, "sewer lever"),
    (376, "hellforge"),
    (392, "diablo seal"),
    (393, "diablo seal"),
    (394, "diablo seal"),
    (395, "diablo seal"),
    (396, "diablo seal"),
    // A dummy object that can't be operated, so the star stays after Anya is freed.
    (460, "anya"),
];

/// Quest monster presets (monster preset txt id = monstats hcIdx) drawn like a quest object
/// until the monster is seen dead near its preset (see [`crate::session`]).
pub const QUEST_MONSTERS: &[(u32, &str)] = &[
    // The breakable door of a barbarian prisoner's cage; the cage object (473) is left out so a
    // cage gets one marker.
    (434, "prison door"),
    (526, "nihlathak"),
];

/// The quest-monster label of monster preset txt id `txt`, if it is one.
#[must_use]
pub fn quest_monster(txt: u32) -> Option<&'static str> {
    QUEST_MONSTERS
        .iter()
        .find(|&&(id, _)| id == txt)
        .map(|&(_, label)| label)
}

/// The quest-object label of object txt id `txt`, if it is one.
#[must_use]
pub fn quest_object(txt: u32) -> Option<&'static str> {
    QUEST_OBJECTS
        .iter()
        .find(|&&(id, _)| id == txt)
        .map(|&(_, label)| label)
}

/// Marker tags ([`marker_tag`]); shrines take theirs from [`crate::config::SHRINE_TAGS`].
const WAYPOINT_TAG: &str = "wp";
const WELL_TAG: &str = "well";
const ORIFICE_TAG: &str = "orif";

/// The map tag of `marker` and its rank (lower wins a crowded spot; exit labels are rank 0).
fn marker_tag(marker: Marker) -> Option<(u8, &'static str)> {
    match marker {
        Marker::Waypoint => Some((1, WAYPOINT_TAG)),
        Marker::Orifice => Some((2, ORIFICE_TAG)),
        Marker::Shrine(code) => crate::config::SHRINE_TAGS.get(usize::from(code)).map(|&t| (3, t)),
        Marker::Well => Some((4, WELL_TAG)),
        _ => None,
    }
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
/// The Moo Moo Farm trail ([`Snapshot::trail`]).
const TRAIL: Rgba = [255, 200, 225, 90];
const TRAIL_WIDTH: f32 = 3.0;
/// Tiles between consecutive trail points beyond which the player teleported: the line breaks.
const TRAIL_BREAK: f64 = 30.0;
const MARKER_OUTLINE: Rgba = [0, 0, 0, 220];
const LANDMARK_GOLD: Rgba = [255, 200, 30, 255];
/// Radius in px of the landmark star's halo ring.
const LANDMARK_RING: f32 = 13.0;
/// The "?" of an unconfirmed landmark: glyph colour, disc colour and disc radius.
const SUSPECT_TEXT: Rgba = [255, 235, 150, 255];
const SUSPECT_DISC: Rgba = [0, 0, 0, 150];
const SUSPECT_DISC_RADIUS: f32 = 13.0;
const SUSPECT_GLYPH_PX: f32 = 22.0;
const UNIQUE_GOLD: Rgba = [235, 190, 60, 255];
const CHARM_BLUE: Rgba = [120, 170, 255, 255];
const BASE_WHITE: Rgba = [240, 240, 240, 255];
/// Drop mark ring: px between the drop marker and the ring, stroke width, and the dash
/// pattern (on, off px) of an uncertain match.
const RING_GAP: f32 = 4.0;
const RING_WIDTH: f32 = 2.0;
const RING_DASH: [f32; 2] = [3.0, 2.5];
/// Keep and farm rings.
const KEEP_GREEN: Rgba = [60, 240, 150, 255];
/// Tip radius in px of the farm ring's star.
const FARM_STAR: f32 = 3.5;
/// A marked drop's tag ([`Mark::label`]): text size in px, and px between the ring and the tag.
const DROP_LABEL_PX: f32 = 11.0;
const DROP_LABEL_CLEAR: f32 = 3.0;
/// The lowest trade tier whose drops get the gold coin ([`high_value`]).
const COIN_MIN_TIER: Tier = Tier::A;
/// The gold coin on a high-value drop's ring: radius in px, colours and rim width.
const COIN_RADIUS: f32 = 3.0;
const COIN_GOLD: Rgba = [255, 205, 40, 255];
const COIN_RIM: Rgba = [140, 90, 0, 255];
const COIN_RIM_WIDTH: f32 = 1.5;
const ELITE_RED: Rgba = [230, 30, 30, 255];
/// Stroked under the fill, so about 1 px of it shows around the skull.
const SKULL_OUTLINE_WIDTH: f32 = 2.0;
/// Tiles within which a live superunique or boss hides its spawn preset's marker; a bit above
/// `session`'s bind radius (15), as the monster wanders around its spawn.
const LIVE_REPLACES_PRESET_RADIUS: f64 = 20.0;
const SHRINE_OUTLINE: Rgba = [255, 255, 255, 230];
const WELL_CYAN: Rgba = [0, 230, 255, 255];
const WELL_RING_WIDTH: f32 = 2.0;
const WELL_BACKING_WIDTH: f32 = 3.5;
/// Super chest: wooden fill, gold rim and lid seam.
const CHEST_WOOD: Rgba = [150, 90, 35, 255];
const CHEST_RIM_WIDTH: f32 = 1.5;
const CHEST_BACKING_WIDTH: f32 = 3.5;

/// Outline shape of a marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Circle,
    /// Square on its tip; the size is the centre-to-tip distance.
    Diamond,
    /// Axis-aligned; the size is half the side.
    Square,
    /// Five-pointed star, one tip up; the size is the centre-to-tip distance.
    Star,
    /// Isosceles triangle ([`triangle_path`]); the size is the centre-to-tip distance.
    Triangle,
    /// Death skull ([`skull_path`]); the size is a quarter of its height and width.
    Skull,
    /// [`Shape::Skull`] with two horns, the demon skull of a boss.
    HornedSkull,
    /// A wavy worm ([`worm_path`]); the size is half its length.
    Worm,
    /// Three steps descending to the right ([`stairs_path`]); the size is half the side.
    Stairs,
    /// A chest: a box wider than tall ([`chest_path`]); the size is half the width.
    Chest,
}

/// Something drawn on the map. Variant order is draw order (player last, on top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Marker {
    /// Tile preset: a level exit.
    Exit,
    /// A confirmed stairs-down landmark ([`LandmarkStyle::StairsDown`]): a purple staircase.
    StairsDown,
    /// The tile preset the player entered the level by (see `Session::apply`).
    Entry,
    /// A walk-through border to an adjacent level ([`border_exits`]): a triangle pointing along
    /// `dir` (world tiles, unnormalised); `entry` if the player came from that level.
    BorderExit {
        dir: (i32, i32),
        entry: bool,
    },
    Waypoint,
    /// Monster preset: a superunique (with its name if known) or other special spawn.
    Special(Option<&'static str>),
    /// A room an unconfirmed landmark rule matched, with the rule's label: drawn as a "?".
    Suspected(&'static str),
    /// A landmark room, with the rule's label. Below the live elites, so a boss on it shows.
    Landmark(&'static str),
    Worm,
    Orifice,
    /// A quest object or quest monster preset, with its label. Above the landmark star, so both
    /// show when they overlap.
    QuestObject(&'static str),
    /// An unopened super chest preset in Kurast ([`SUPER_CHEST_LEVELS`]).
    SuperChest,
    /// A well (wells refill: drawn used or not).
    Well,
    /// An unused shrine, with its shrines.txt code.
    Shrine(u8),
    /// Any other living monster, only on levels in [`ALL_MONSTER_LEVELS`].
    Monster,
    /// Live champion or unique monster.
    Elite,
    Superunique,
    /// Live quest boss ([`BOSS_MONSTERS`](crate::levels_data::BOSS_MONSTERS)).
    Boss,
    /// The local player's mercenary.
    Merc,
    /// The player. Below the drops: on big levels a fresh drop lies within a pixel of the player.
    Player,
    Drop(DropKind),
    /// The mark ring around the drop marker of `kind`, with a gold coin if the drop sells for a
    /// lot ([`high_value`]). After all drops, so no drop hides a ring.
    Ring {
        kind: DropKind,
        look: RingLook,
        uncertain: bool,
        coin: bool,
    },
}

/// How a drop mark looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RingLook {
    /// For the player's own builds.
    Keep,
    /// A still-missing farm-list item: the keep ring with a star on it.
    Farm,
    Trade(Tier),
}

impl RingLook {
    const fn of(mark: &Mark) -> Self {
        match (mark.trade, mark.kind) {
            (Some((tier, _)), _) => Self::Trade(tier),
            (None, MarkKind::Farm) => Self::Farm,
            (None, MarkKind::Keep | MarkKind::Trade) => Self::Keep,
        }
    }

    const fn colour(self) -> Rgba {
        match self {
            Self::Keep | Self::Farm => KEEP_GREEN,
            Self::Trade(Tier::S) => [230, 40, 40, 255],
            Self::Trade(Tier::A) => [255, 140, 0, 255],
            Self::Trade(Tier::B) => [255, 220, 0, 255],
        }
    }
}

impl Marker {
    /// Fill colour, size in px (radius, or see [`Shape`]) and shape.
    const fn style(self) -> (Rgba, f32, Shape) {
        match self {
            Self::Exit => ([170, 90, 255, 255], 5.0, Shape::Circle),
            Self::Entry => ([40, 200, 200, 255], 4.5, Shape::Square),
            Self::StairsDown => (Self::Exit.style().0, 6.0, Shape::Stairs),
            Self::BorderExit { entry: false, .. } => (Self::Exit.style().0, 7.0, Shape::Triangle),
            Self::BorderExit { entry: true, .. } => (Self::Entry.style().0, 7.0, Shape::Triangle),
            Self::Waypoint => ([60, 140, 255, 255], 5.0, Shape::Circle),
            Self::Special(_) => ([255, 110, 40, 255], 4.5, Shape::Circle),
            // The ring is the no-font fallback; with a font the "?" glyph is drawn instead.
            Self::Suspected(_) => (SUSPECT_TEXT, 7.0, Shape::Circle),
            Self::Landmark(_) => (LANDMARK_GOLD, 9.0, Shape::Star),
            Self::Worm => (LANDMARK_GOLD, 9.0, Shape::Worm),
            Self::Orifice => (LANDMARK_GOLD, 7.0, Shape::Circle),
            Self::QuestObject(_) => (LANDMARK_GOLD, 6.0, Shape::Star),
            Self::SuperChest => (CHEST_WOOD, 5.0, Shape::Chest),
            Self::Well => (WELL_CYAN, 4.5, Shape::Circle),
            Self::Shrine(code) => (shrine_colour(code), 4.0, Shape::Circle),
            Self::Monster => ([255, 120, 120, 200], 2.0, Shape::Circle),
            Self::Elite => (ELITE_RED, 3.5, Shape::Circle),
            Self::Superunique => (ELITE_RED, 3.5, Shape::Skull),
            Self::Boss => (ELITE_RED, 3.5, Shape::HornedSkull),
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
            Self::Ring { kind, look, .. } => {
                let radius = Self::Drop(kind).style().1 + RING_GAP;
                (look.colour(), radius, Shape::Circle)
            }
            Self::Merc => ([150, 255, 190, 255], 2.5, Shape::Circle),
            Self::Player => ([40, 255, 60, 255], 3.5, Shape::Circle),
        }
    }
}

/// Fill colour of a shrine by shrines.txt code; a neutral grey for the types without their own.
const fn shrine_colour(code: u8) -> Rgba {
    const REFILLING: u8 = 1;
    const SKILL: u8 = 12;
    const STAMINA: u8 = 14;
    const GEM: u8 = 18;
    const MONSTER: u8 = 20;
    match code {
        REFILLING => [120, 210, 255, 255],
        SKILL => [255, 80, 200, 255],
        STAMINA => [180, 235, 30, 255],
        GEM => [250, 250, 250, 255],
        MONSTER => [140, 0, 20, 255],
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
#[must_use]
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
    #[must_use]
    pub fn fit(bounds: Bounds, width: u32, height: u32) -> Self {
        let (min_x, min_y) = (f64::from(bounds.min_x), f64::from(bounds.min_y));
        let (max_x, max_y) = (f64::from(bounds.max_x), f64::from(bounds.max_y));
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
    #[must_use]
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
#[must_use]
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

/// Levels where every living enemy is drawn, not just elites: Den of Evil (its quest needs every
/// monster killed) and the Moo Moo Farm.
pub const ALL_MONSTER_LEVELS: &[u32] = &[8, 39];

/// A live superunique, champion or unique enemy. Allies are skipped by txt id: assassin traps
/// carry the unique flag.
#[must_use]
pub fn is_elite(m: &Monster) -> bool {
    m.flags & ELITE_FLAGS != 0 && !m.is_dead() && !crate::levels_data::NON_HOSTILE_MONSTERS.contains(&m.txt)
}

/// The icon marker of a live boss (by txt id, whatever its flags) or elite ([`is_elite`]).
#[must_use]
pub fn elite_marker(m: &Monster) -> Option<Marker> {
    if m.is_dead() {
        None
    } else if crate::levels_data::BOSS_MONSTERS.contains(&m.txt) {
        Some(Marker::Boss)
    } else if !is_elite(m) {
        None
    } else if m.flags & monster_data::SUPERUNIQUE != 0 {
        Some(Marker::Superunique)
    } else {
        Some(Marker::Elite)
    }
}

/// Whether the monster spawn preset `p`, drawn as `marker`, has a live superunique or boss of
/// `live` within [`LIVE_REPLACES_PRESET_RADIUS`]: the live icon shows instead.
fn replaced_by_live(p: &Preset, marker: Marker, live: &[(f64, f64)]) -> bool {
    p.kind == PresetKind::Monster
        && matches!(marker, Marker::Special(_) | Marker::QuestObject(_))
        && live
            .iter()
            .any(|&(mx, my)| (f64::from(p.x) - mx).hypot(f64::from(p.y) - my) <= LIVE_REPLACES_PRESET_RADIUS)
}

/// The marker a preset is drawn as, if any; a super chest in any level (`markers_with` keeps it
/// to [`SUPER_CHEST_LEVELS`]).
pub fn preset_marker(p: &Preset) -> Option<Marker> {
    match p.kind {
        PresetKind::Object if WAYPOINT_IDS.contains(&p.txt) => Some(Marker::Waypoint),
        PresetKind::Object if p.txt == SUPER_CHEST_PRESET => Some(Marker::SuperChest),
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

/// One landmark marker per piece matched by a rule in `rules`, at the centre of the piece's
/// bounding box (see [`LandmarkRule`]).
#[must_use]
pub fn landmarks(snap: &Snapshot, rules: &[LandmarkRule]) -> Vec<(Marker, f64, f64)> {
    let level = &snap.level;
    let mut out = Vec::new();
    for rule in rules.iter().filter(|rule| rule.level_ids.contains(&level.id)) {
        if rule.candidates_for.is_some_and(|txt| has_object(snap, txt)) {
            continue;
        }
        let marker = rule_marker(rule);
        for piece in pieces(&level.rooms, &rule.preset_ids) {
            if rule.candidates_for.is_some() && ruled_out(snap, &piece) {
                continue;
            }
            if let Some((x, y)) = piece_centre(&piece) {
                out.push((marker, x, y));
            }
        }
    }
    out
}

/// The marker a piece matched by `rule` gets.
const fn rule_marker(rule: &LandmarkRule) -> Marker {
    match (rule.confirmed, rule.style) {
        (false, _) => Marker::Suspected(rule.label),
        (true, LandmarkStyle::Star) => Marker::Landmark(rule.label),
        (true, LandmarkStyle::StairsDown) => Marker::StairsDown,
        (true, LandmarkStyle::Worm) => Marker::Worm,
        (true, LandmarkStyle::Orifice) => Marker::Orifice,
    }
}

/// Whether an object preset of txt id `txt` is in the snapshot.
fn has_object(snap: &Snapshot, txt: u32) -> bool {
    snap.presets
        .iter()
        .any(|p| p.kind == PresetKind::Object && p.txt == txt)
}

/// Whether a candidate piece can't hold the quest object the snapshot lacks: it holds the
/// waypoint (the two never share a piece), or all its rooms were seen loaded (a loaded room's
/// preset list is complete).
fn ruled_out(snap: &Snapshot, piece: &[(Span, bool)]) -> bool {
    piece.iter().any(|&(s, _)| waypoint_in(snap, s)) || piece.iter().all(|&(_, seen)| seen)
}

/// Whether a waypoint preset lies in `s`.
fn waypoint_in(snap: &Snapshot, s: Span) -> bool {
    snap.presets.iter().any(|p| {
        p.kind == PresetKind::Object
            && WAYPOINT_IDS.contains(&p.txt)
            && (s.x0..s.x1).contains(&p.x)
            && (s.y0..s.y1).contains(&p.y)
    })
}

/// The centre (world tiles) of a piece's bounding box; `None` for an empty piece.
fn piece_centre(piece: &[(Span, bool)]) -> Option<(f64, f64)> {
    let b = piece.iter().map(|&(s, _)| s).reduce(Span::union)?;
    Some((
        f64::midpoint(f64::from(b.x0), f64::from(b.x1)),
        f64::midpoint(f64::from(b.y0), f64::from(b.y1)),
    ))
}

/// Whether a room of level `level` with level-preset id `preset` belongs to a
/// [`LandmarkRule::candidates_for`] rule: the session remembers such rooms seen loaded
/// ([`Room::seen_loaded`]).
#[must_use]
pub fn candidate_room(level: u32, preset: Option<u32>) -> bool {
    preset.is_some_and(|id| {
        LANDMARKS.iter().any(|rule| {
            rule.candidates_for.is_some() && rule.level_ids.contains(&level) && rule.preset_ids.contains(&id)
        })
    })
}

/// The rooms of `rooms` whose preset id is in `ids`, grouped into pieces: rooms of one preset id
/// joined through shared edges ([`Span::shares_edge`]). Pieces come in room order; each room
/// comes with whether it is or was seen loaded.
fn pieces(rooms: &[Room], ids: &std::ops::RangeInclusive<u32>) -> Vec<Vec<(Span, bool)>> {
    let rooms: Vec<(u32, Span, bool)> = rooms
        .iter()
        .filter_map(|r| {
            let id = r.preset.filter(|id| ids.contains(id))?;
            let span = Span::of(RoomRect {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
            });
            Some((id, span, r.loaded || r.seen_loaded))
        })
        .collect();
    let mut rest = rooms;
    rest.reverse();
    let mut out = Vec::new();
    while let Some((id, seed, seen)) = rest.pop() {
        let mut piece = vec![(seed, seen)];
        grow_piece(id, &mut piece, &mut rest);
        out.push(piece);
    }
    out
}

/// Moves the rooms of `rest` with preset `id` that share an edge with `piece` into it, until
/// none is left. A level has at most a few hundred rooms, so the repeated scans are cheap.
fn grow_piece(id: u32, piece: &mut Vec<(Span, bool)>, rest: &mut Vec<(u32, Span, bool)>) {
    loop {
        let (grown, keep): (Vec<_>, Vec<_>) = std::mem::take(rest)
            .into_iter()
            .partition(|&(rid, r, _)| rid == id && piece.iter().any(|&(p, _)| p.shares_edge(r)));
        *rest = keep;
        if grown.is_empty() {
            return;
        }
        piece.extend(grown.into_iter().map(|(_, r, seen)| (r, seen)));
    }
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

    /// Whether the two touch along one axis and overlap by more than a point along the other
    /// (a corner touch does not count).
    const fn shares_edge(self, o: Self) -> bool {
        let x_overlap = self.x0 < o.x1 && o.x0 < self.x1;
        let y_overlap = self.y0 < o.y1 && o.y0 < self.y1;
        let x_touch = self.x1 == o.x0 || o.x1 == self.x0;
        let y_touch = self.y1 == o.y0 || o.y1 == self.y0;
        (x_overlap && (y_overlap || y_touch)) || (y_overlap && x_touch)
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
/// edge ([`border_exit_to`]); levels with a room-tile link keep their tile exit marker instead.
#[must_use]
pub fn border_exits(snap: &Snapshot) -> Vec<(Marker, f64, f64)> {
    border_exits_to(snap)
        .into_iter()
        .map(|(_, marker, x, y)| (marker, x, y))
        .collect()
}

/// [`border_exits`] with each marker's target level id first.
fn border_exits_to(snap: &Snapshot) -> Vec<(u32, Marker, f64, f64)> {
    let mut levels: Vec<u32> = snap.border_links.iter().map(|l| l.to_level_id).collect();
    levels.sort_unstable();
    levels.dedup();
    levels
        .into_iter()
        .filter(|id| !snap.room_links.iter().any(|l| l.to_level_id == *id))
        .filter_map(|id| {
            let (marker, x, y) = border_exit_to(snap, id)?;
            Some((id, marker, x, y))
        })
        .collect()
}

/// The border exit to level `id`, from the border links whose rooms actually touch (a level
/// behind stairs or a cave mouth lies elsewhere in world space). It sits at the centre of the
/// shared border strips and points from this level's touching rooms to the other level's.
fn border_exit_to(snap: &Snapshot, id: u32) -> Option<(Marker, f64, f64)> {
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
}

/// One label per level this level links to: the target level id and where its name goes (world
/// tiles): a room-tile link's on its room's exit ([`room_exit_spot`]), a walk-through border's
/// on its [`border_exits`] triangle. Links from rooms the level doesn't list are left out.
#[must_use]
pub fn exit_labels(snap: &Snapshot) -> Vec<(u32, f64, f64)> {
    let mut out: Vec<(u32, f64, f64)> = Vec::new();
    for l in &snap.room_links {
        if l.to_level_id == snap.level.id || out.iter().any(|&(id, _, _)| id == l.to_level_id) {
            continue;
        }
        let Some(room) = snap
            .level
            .rooms
            .iter()
            .find(|r| r.x == l.from_room.x && r.y == l.from_room.y)
        else {
            continue;
        };
        let (x, y) = room_exit_spot(snap, room);
        out.push((l.to_level_id, x, y));
    }
    for (id, _, x, y) in border_exits_to(snap) {
        if !out.iter().any(|&(seen, _, _)| seen == id) {
            out.push((id, x, y));
        }
    }
    out
}

/// The tile exit preset in `room`, or the room's centre if it isn't loaded (world tiles).
fn room_exit_spot(snap: &Snapshot, room: &Room) -> (f64, f64) {
    let inside = |p: &&Preset| {
        p.kind == PresetKind::Tile
            && (room.x..room.x.saturating_add(room.w)).contains(&p.x)
            && (room.y..room.y.saturating_add(room.h)).contains(&p.y)
    };
    snap.presets.iter().find(inside).map_or_else(
        || {
            (
                f64::from(room.x) + f64::from(room.w) / 2.0,
                f64::from(room.y) + f64::from(room.h) / 2.0,
            )
        },
        |p| (f64::from(p.x), f64::from(p.y)),
    )
}

/// The markers of `snap` with its drops' `marks` as rings, by the landmark `rules`, in draw
/// order (stable: snapshot order within one kind).
fn markers_with(snap: &Snapshot, marks: &Marks, rules: &[LandmarkRule]) -> Vec<(Marker, f64, f64)> {
    let live_bosses: Vec<(f64, f64)> = snap
        .monsters
        .iter()
        .filter(|m| matches!(elite_marker(m), Some(Marker::Superunique | Marker::Boss)))
        .map(|m| (f64::from(m.x), f64::from(m.y)))
        .collect();
    let mut out: Vec<(Marker, f64, f64)> = snap
        .presets
        .iter()
        .filter_map(|p| {
            let marker = preset_marker(p).filter(|&m| {
                !replaced_by_live(p, m, &live_bosses)
                    && (m != Marker::SuperChest || SUPER_CHEST_LEVELS.contains(&snap.level.id))
            })?;
            Some((marker, f64::from(p.x), f64::from(p.y)))
        })
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
            let marker = monster_marker(m, snap.level.id)?;
            Some((marker, f64::from(m.x), f64::from(m.y)))
        }))
        .chain(
            snap.drops
                .iter()
                .map(|d| (Marker::Drop(d.kind), f64::from(d.x), f64::from(d.y))),
        )
        .chain(snap.drops.iter().filter_map(|d| {
            let ring = ring_marker(d.kind, marks.get(&d.id)?);
            Some((ring, f64::from(d.x), f64::from(d.y)))
        }))
        .chain(landmarks(snap, rules))
        .chain(border_exits(snap))
        .collect();
    out.extend(
        snap.monsters
            .iter()
            .filter(|m| !m.is_dead() && m.owner_id == snap.player.id)
            .map(|m| (Marker::Merc, f64::from(m.x), f64::from(m.y))),
    );
    out.push((Marker::Player, f64::from(snap.player.x), f64::from(snap.player.y)));
    out.sort_by_key(|&(m, _, _)| m);
    out
}

/// The marker of living monster `m` in level `level_id`: its elite icon, or a plain dot for
/// any enemy on an [`ALL_MONSTER_LEVELS`] level.
fn monster_marker(m: &Monster, level_id: u32) -> Option<Marker> {
    elite_marker(m).or_else(|| {
        (ALL_MONSTER_LEVELS.contains(&level_id) && !crate::levels_data::NON_HOSTILE_MONSTERS.contains(&m.txt))
            .then_some(Marker::Monster)
    })
}

/// The ring a drop of `kind` with `mark` gets.
fn ring_marker(kind: DropKind, mark: &Mark) -> Marker {
    Marker::Ring {
        kind,
        look: RingLook::of(mark),
        uncertain: mark.uncertain,
        coin: high_value(mark),
    }
}

/// Renders the snapshot's level into a new transparent `width`×`height` pixmap, with the info
/// line at the bottom if a font is available, and `guidance`'s drop rings and rating.
///
/// # Errors
/// Fails if the canvas size is zero or too large for tiny-skia.
pub fn render(snap: &Snapshot, guidance: &Guidance, width: u32, height: u32) -> Result<Pixmap> {
    render_with(snap, guidance, width, height, info::font())
}

/// The canvas height the map is fitted into: all of it, less the info line's band if there is
/// a font to draw it with.
#[must_use]
pub fn map_height(height: u32, font: Option<&FontVec>) -> u32 {
    font.map_or(height, |f| height.saturating_sub(info::band_height(f)).max(1))
}

/// The dashboard's text blocks from one snapshot (the overlay draws none of them): the
/// breakpoint guide, the item plan's groups by action and the `BiS` chase list.
#[derive(Debug, Default)]
pub struct TextBlocks {
    pub guide: Vec<info::Band>,
    pub sell: Vec<Advice>,
    /// Keepers in the wrong place, with their target pages or tab.
    pub moves: Vec<Advice>,
    pub keep_own: Vec<Advice>,
    pub keep_trade: Vec<Advice>,
    pub give: Vec<Advice>,
    pub bis: Vec<info::BisLine>,
}

/// The item plan's entries whose action `wanted` picks.
fn entries(guidance: &Guidance, wanted: impl Fn(Action) -> bool) -> Vec<Advice> {
    guidance
        .items
        .iter()
        .filter(|a| wanted(a.action))
        .cloned()
        .collect()
}

/// The dashboard's groups ([`TextBlocks`]) for a `width` px canvas from one snapshot. None of
/// it without a font.
pub fn text_blocks(snap: &Snapshot, guidance: &Guidance, width: u32, font: Option<&FontVec>) -> TextBlocks {
    let Some(f) = font else {
        return TextBlocks::default();
    };
    let max_width = f32::from(u16::try_from(width).unwrap_or(u16::MAX)) - 8.0;
    TextBlocks {
        guide: crate::breakpoints::guide_band(&snap.player, snap.difficulty)
            .into_iter()
            .map(|band| info::fit_band(band, f, max_width))
            .collect(),
        sell: entries(guidance, |a| a == Action::Sell),
        moves: entries(guidance, |a| matches!(a, Action::Move(_) | Action::Stow(_))),
        keep_own: entries(guidance, |a| a == Action::Keep(Side::Own)),
        keep_trade: entries(guidance, |a| a == Action::Keep(Side::Trade)),
        give: entries(guidance, |a| a == Action::GiveAway),
        bis: guidance.bis.as_ref().map_or_else(Vec::new, |bis| {
            info::bis_lines(
                bis,
                snap.player.char_level,
                &snap.equipped,
                &snap.merc_equipped,
                &crate::game::worn_names(&snap.equipped_items),
                &snap.owned_names,
                &snap.stash_names,
                f,
                max_width,
            )
        }),
    }
}

/// [`render`] with an explicit font; `None` draws no info line and gives the map the full canvas.
///
/// # Errors
/// Fails if the canvas size is zero or too large for tiny-skia.
pub fn render_with(
    snap: &Snapshot,
    guidance: &Guidance,
    width: u32,
    height: u32,
    font: Option<&FontVec>,
) -> Result<Pixmap> {
    render_rules(snap, guidance, width, height, font, LANDMARKS)
}

/// [`render_with`] with custom landmark rules (for tests).
fn render_rules(
    snap: &Snapshot,
    guidance: &Guidance,
    width: u32,
    height: u32,
    font: Option<&FontVec>,
    rules: &[LandmarkRule],
) -> Result<Pixmap> {
    let mut pm =
        Pixmap::new(width, height).with_context(|| format!("invalid canvas size {width}x{height}"))?;
    let view = View::fit(level_bounds(snap), width, map_height(height, font));
    fill_rooms(&mut pm, view, snap);
    draw_trail(&mut pm, view, &snap.trail);
    let mut tagged: Vec<(Marker, f32, f32)> = Vec::new();
    for (marker, x, y) in markers_with(snap, &guidance.marks, rules) {
        let (cx, cy) = view.to_screen(x, y);
        if marker_tag(marker).is_some() {
            tagged.push((marker, cx, cy));
        }
        draw_marker(&mut pm, font, marker, cx, cy);
    }
    if let Some(font) = font {
        let drops = drop_tags(view, snap, &guidance.marks);
        draw_labels(&mut pm, view, font, snap, &tagged, &drops);
        draw_info_line(&mut pm, font, snap, guidance.rating);
    }
    draw_player_on_top(&mut pm, view, snap);
    Ok(pm)
}

/// Draws `marker` centred on canvas point (`cx`, `cy`).
fn draw_marker(pm: &mut Pixmap, font: Option<&FontVec>, marker: Marker, cx: f32, cy: f32) {
    let (colour, size, shape) = marker.style();
    let Some(dot) = marker_outline(marker, shape, cx, cy, size) else {
        return;
    };
    let geom = (cx, cy, size);
    match marker {
        Marker::Suspected(_) => draw_suspected(pm, font, &dot, cx, cy),
        Marker::Orifice => draw_orifice(pm, colour, geom),
        Marker::Ring { .. } => draw_mark_ring(pm, marker, &dot, geom),
        Marker::Well => draw_well(pm, &dot, colour),
        Marker::SuperChest => draw_chest(pm, &dot, colour, geom),
        Marker::Shrine(_) => fill_then_outline(pm, &dot, colour, SHRINE_OUTLINE),
        Marker::QuestObject(_) => outline_then_fill(pm, &dot, colour, &round_join(3.0)),
        _ => match shape {
            Shape::Worm => draw_worm(pm, &dot, colour, geom),
            Shape::Skull | Shape::HornedSkull => {
                draw_skull(pm, &dot, colour, geom, shape == Shape::HornedSkull);
            }
            Shape::Triangle | Shape::Stairs => outline_then_fill(pm, &dot, colour, &marker_stroke()),
            Shape::Star => {
                draw_halo(pm, cx, cy, colour);
                // Round joins: a miter on the sharp tips would spike out into the halo.
                outline_then_fill(pm, &dot, colour, &round_join(4.0));
            }
            Shape::Circle | Shape::Diamond | Shape::Square | Shape::Chest => {
                fill_then_outline(pm, &dot, colour, MARKER_OUTLINE);
            }
        },
    }
}

/// The default marker outline stroke.
fn marker_stroke() -> Stroke {
    Stroke {
        width: 1.5,
        ..Stroke::default()
    }
}

/// A `width` px stroke with round joins.
fn round_join(width: f32) -> Stroke {
    Stroke {
        width,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// Fills `path` in `colour`, then strokes the default outline in `outline` over its edge.
fn fill_then_outline(pm: &mut Pixmap, path: &Path, colour: Rgba, outline: Rgba) {
    solid(pm, path, colour);
    pm.stroke_path(
        path,
        &paint(outline),
        &marker_stroke(),
        Transform::identity(),
        None,
    );
}

/// Strokes the dark outline with `stroke`, then fills `path` in `colour` over it: only the
/// outer half of the outline shows, so small and pointed shapes keep all their colour.
fn outline_then_fill(pm: &mut Pixmap, path: &Path, colour: Rgba, stroke: &Stroke) {
    pm.stroke_path(path, &paint(MARKER_OUTLINE), stroke, Transform::identity(), None);
    solid(pm, path, colour);
}

/// The landmark star's halo: a `colour` ring on a wider dark ring, so it reads on any background.
fn draw_halo(pm: &mut Pixmap, cx: f32, cy: f32, colour: Rgba) {
    let Some(halo) = PathBuilder::from_circle(cx, cy, LANDMARK_RING) else {
        return;
    };
    for (colour, width) in [(MARKER_OUTLINE, 4.5), (colour, 2.0)] {
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(&halo, &paint(colour), &stroke, Transform::identity(), None);
    }
}

/// The player's dot drawn again over everything, labels included, on a dark padding disc.
fn draw_player_on_top(pm: &mut Pixmap, view: View, snap: &Snapshot) {
    let (colour, size, shape) = Marker::Player.style();
    let (px, py) = view.to_screen(f64::from(snap.player.x), f64::from(snap.player.y));
    if let Some(pad) = PathBuilder::from_circle(px, py, size + PLAYER_PADDING) {
        solid(pm, &pad, PLAYER_PADDING_COLOUR);
    }
    if let Some(dot) = marker_outline(Marker::Player, shape, px, py, size) {
        fill_then_outline(pm, &dot, colour, MARKER_OUTLINE);
    }
}

/// Width in px of the dark padding disc around the player's dot.
const PLAYER_PADDING: f32 = 3.0;
const PLAYER_PADDING_COLOUR: Rgba = [10, 10, 10, 220];

/// Half-side in px of a Tal Rasha's Tomb symbol on a Canyon exit, and of the real tomb's.
const TOMB_HALF: f32 = 8.0;
const REAL_TOMB_HALF: f32 = 11.0;
/// Stroke width of a tomb symbol's lines; the real tomb's is wider.
const TOMB_STROKE: f32 = 2.0;
const REAL_TOMB_STROKE: f32 = 2.5;
/// Radius of the gold star beside the real tomb's symbol, and its gap to the symbol.
const REAL_TOMB_STAR: f32 = 5.0;
const REAL_TOMB_STAR_GAP: f32 = 3.0;

/// What a map label shows: text, a tomb's symbol in place of the seven identical
/// "Tal Rasha's Tomb" names (`real`: the tomb with the orifice), or a marked drop's small tag
/// kept `clear` px off the drop's centre.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LabelBody<'a> {
    Text(&'a str),
    Tomb { symbol: tombs::Symbol, real: bool },
    Drop { text: &'a str, clear: f32 },
}

/// A marked drop's tag for [`draw_labels`]: the mark's label in its ring's colour.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DropTag<'a> {
    text: &'a str,
    look: RingLook,
    at: (f32, f32),
    /// The ring's radius plus [`DROP_LABEL_CLEAR`].
    clear: f32,
}

/// The tags of the marked drops with a label, most valuable first (they win a crowded spot):
/// S, A, B trade, then farm, then keep; snapshot order within one look.
fn drop_tags<'a>(view: View, snap: &Snapshot, marks: &'a Marks) -> Vec<DropTag<'a>> {
    let mut tags: Vec<DropTag> = snap
        .drops
        .iter()
        .filter_map(|d| {
            let mark = marks.get(&d.id).filter(|m| !m.label.is_empty())?;
            Some(DropTag {
                text: &mark.label,
                look: RingLook::of(mark),
                at: view.to_screen(f64::from(d.x), f64::from(d.y)),
                clear: Marker::Drop(d.kind).style().1 + RING_GAP + DROP_LABEL_CLEAR,
            })
        })
        .collect();
    tags.sort_by_key(|t| std::cmp::Reverse(t.look));
    tags
}

/// The box a tomb symbol label takes: the symbol's square plus 2 px on each side for its stroke
/// and outline, and the star for the real tomb.
fn tomb_label_size(real: bool) -> info::LabelSize {
    let (half, star) = if real {
        (REAL_TOMB_HALF, REAL_TOMB_STAR.mul_add(2.0, REAL_TOMB_STAR_GAP))
    } else {
        (TOMB_HALF, 0.0)
    };
    info::LabelSize {
        width: half.mul_add(2.0, 4.0) + star,
        ascent: half.mul_add(2.0, 4.0),
        descent: 0.0,
    }
}

/// Draws a tomb's symbol in `colour` with a dark outline at the pen position and baseline
/// [`info::place_labels`] gave it ([`tomb_label_size`]); the real tomb's gets a gold star after it.
fn draw_tomb_label(
    pm: &mut Pixmap,
    symbol: tombs::Symbol,
    real: bool,
    colour: Rgba,
    (pen, baseline): (f32, f32),
) {
    let (half, width) = if real {
        (REAL_TOMB_HALF, REAL_TOMB_STROKE)
    } else {
        (TOMB_HALF, TOMB_STROKE)
    };
    let (cx, cy) = (pen + 2.0 + half, baseline - 2.0 - half);
    let round = |width| Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let glyph = symbol.glyph(cx, cy, half);
    // Every outline before any colour, so no outline covers a neighbouring stroke.
    if let Some(lines) = &glyph.lines {
        pm.stroke_path(
            lines,
            &paint(MARKER_OUTLINE),
            &round(width + 2.0),
            Transform::identity(),
            None,
        );
    }
    if let Some(fill) = &glyph.solid {
        pm.stroke_path(
            fill,
            &paint(MARKER_OUTLINE),
            &round(2.0),
            Transform::identity(),
            None,
        );
    }
    if let Some(lines) = &glyph.lines {
        pm.stroke_path(lines, &paint(colour), &round(width), Transform::identity(), None);
    }
    if let Some(fill) = &glyph.solid {
        solid(pm, fill, colour);
    }
    let star_x = half.mul_add(2.0, pen + 4.0) + REAL_TOMB_STAR_GAP + REAL_TOMB_STAR;
    if real && let Some(star) = marker_path(Shape::Star, star_x, cy, REAL_TOMB_STAR) {
        pm.stroke_path(
            &star,
            &paint(MARKER_OUTLINE),
            &round(3.0),
            Transform::identity(),
            None,
        );
        solid(pm, &star, LANDMARK_GOLD);
    }
}

/// One map label for [`draw_labels`]: what it shows, its colour, the canvas centre of the
/// marker it names, and whether it is drawn even without a free spot.
#[derive(Debug, Clone, Copy)]
struct MapLabel<'a> {
    body: LabelBody<'a>,
    colour: Rgba,
    at: (f32, f32),
    keep: bool,
}

/// The map labels, highest priority first: exit names ([`exit_label_list`]), marker tags
/// ([`marker_tag_list`]) and the marked drops' tags. [`info::place_labels`] moves or drops the
/// lower-ranked one where labels would overlap; exit labels are always drawn.
fn draw_labels(
    pm: &mut Pixmap,
    view: View,
    font: &FontVec,
    snap: &Snapshot,
    tagged: &[(Marker, f32, f32)],
    drops: &[DropTag],
) {
    let names: Vec<(u32, String, f64, f64)> = exit_labels(snap)
        .into_iter()
        .map(|(id, x, y)| {
            let name = info::level_name(id).map_or_else(|| format!("Level {id}"), str::to_owned);
            (id, name, x, y)
        })
        .collect();
    let mut labels = exit_label_list(view, snap, &names);
    labels.extend(marker_tag_list(tagged));
    labels.extend(drops.iter().map(|t| MapLabel {
        body: LabelBody::Drop {
            text: t.text,
            clear: t.clear,
        },
        colour: t.look.colour(),
        at: t.at,
        keep: false,
    }));
    let player = player_box(view, snap);
    let specs: Vec<info::LabelSpec> = labels.iter().map(|l| label_spec(font, l, player)).collect();
    let bottom = map_height(pm.height(), Some(font));
    #[allow(clippy::cast_precision_loss, reason = "canvas widths are far below 2^24")]
    let spots = info::place_labels(&specs, pm.width() as f32, bottom as f32);
    for (l, spot) in labels.iter().zip(spots) {
        let Some(spot) = spot else { continue };
        match l.body {
            LabelBody::Text(text) => info::draw_label(pm, font, l.colour, text, spot),
            LabelBody::Tomb { symbol, real } => draw_tomb_label(pm, symbol, real, l.colour, spot),
            LabelBody::Drop { text, .. } => {
                info::draw_label_at(pm, font, DROP_LABEL_PX, l.colour, text, spot);
            }
        }
    }
}

/// The exit labels for `names` (level id, name, world position): white, grey where the character
/// gets almost no experience ([`xp::is_low`]). A Tal Rasha's Tomb exit shows the tomb's symbol
/// instead, the real tomb's ([`Snapshot::staff_tomb`]) gold.
fn exit_label_list<'a>(
    view: View,
    snap: &Snapshot,
    names: &'a [(u32, String, f64, f64)],
) -> Vec<MapLabel<'a>> {
    names
        .iter()
        .map(|(id, name, x, y)| {
            let real = snap.staff_tomb == Some(*id);
            let colour = if real {
                LANDMARK_GOLD
            } else if xp::is_low(*id, snap.difficulty, snap.player.char_level) {
                info::LOW_XP_TEXT
            } else {
                LABEL_TEXT
            };
            let body =
                tombs::symbol(*id).map_or(LabelBody::Text(name), |symbol| LabelBody::Tomb { symbol, real });
            MapLabel {
                body,
                colour,
                at: view.to_screen(*x, *y),
                keep: true,
            }
        })
        .collect()
}

/// The tags of the `tagged` markers ([`marker_tag`]) by rank, in [`marker_label_colour`].
fn marker_tag_list(tagged: &[(Marker, f32, f32)]) -> Vec<MapLabel<'static>> {
    let mut tags: Vec<(u8, MapLabel<'static>)> = tagged
        .iter()
        .filter_map(|&(m, cx, cy)| {
            let (rank, tag) = marker_tag(m)?;
            let label = MapLabel {
                body: LabelBody::Text(tag),
                colour: marker_label_colour(m.style().0),
                at: (cx, cy),
                keep: false,
            };
            Some((rank, label))
        })
        .collect();
    tags.sort_by_key(|&(rank, _)| rank);
    tags.into_iter().map(|(_, label)| label).collect()
}

/// The canvas box `[x0, y0, x1, y1]` of the player's dot with its padding disc.
fn player_box(view: View, snap: &Snapshot) -> [f32; 4] {
    let (px, py) = view.to_screen(f64::from(snap.player.x), f64::from(snap.player.y));
    let r = Marker::Player.style().1 + PLAYER_PADDING;
    [px - r, py - r, px + r, py + r]
}

/// The placement spec of `l`; a drop tag keeps off its ring and, where it can, the `player` box.
fn label_spec(font: &FontVec, l: &MapLabel, player: [f32; 4]) -> info::LabelSpec {
    let (size, clear, avoid) = match l.body {
        LabelBody::Text(text) => (info::label_size(font, text), None, None),
        LabelBody::Tomb { real, .. } => (tomb_label_size(real), None, None),
        LabelBody::Drop { text, clear } => (
            info::label_size_at(font, DROP_LABEL_PX, text),
            Some(clear),
            Some(player),
        ),
    };
    info::LabelSpec {
        size,
        at: l.at,
        keep: l.keep,
        clear,
        avoid,
    }
}

/// A marker tag's colour: the marker's fill blended 35 % toward white, so dark fills still read
/// on the dark outline.
fn marker_label_colour([r, g, b, _]: Rgba) -> Rgba {
    let lift = |c: u8| {
        let c = u16::from(c);
        u8::try_from((c * 65 + 255 * 35) / 100).unwrap_or(u8::MAX)
    };
    [lift(r), lift(g), lift(b), 255]
}

/// The info line at the bottom of the frame. A terrorized level's monster levels follow the
/// character, so its name is never greyed for low experience.
fn draw_info_line(pm: &mut Pixmap, font: &FontVec, snap: &Snapshot, rating: crate::build::Rating) {
    let mut line = info::info_line(snap.level.id, snap.difficulty, snap.terrorized, snap.cows, rating);
    line.low_xp = !snap.terrorized && xp::is_low(snap.level.id, snap.difficulty, snap.player.char_level);
    info::draw(pm, font, &line);
}

/// Strokes `trail` (world tiles) as one translucent polyline, broken where two points lie more
/// than [`TRAIL_BREAK`] apart. One path, so overlapping segments don't stack their alpha.
fn draw_trail(pm: &mut Pixmap, view: View, trail: &[(u16, u16)]) {
    let mut pb = PathBuilder::new();
    let mut prev: Option<(f64, f64)> = None;
    for &(x, y) in trail {
        let (x, y) = (f64::from(x), f64::from(y));
        let (sx, sy) = view.to_screen(x, y);
        match prev {
            Some((px, py)) if (x - px).hypot(y - py) <= TRAIL_BREAK => pb.line_to(sx, sy),
            _ => pb.move_to(sx, sy),
        }
        prev = Some((x, y));
    }
    let Some(path) = pb.finish() else { return };
    let stroke = Stroke {
        width: TRAIL_WIDTH,
        line_join: LineJoin::Round,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pm.stroke_path(&path, &paint(TRAIL), &stroke, Transform::identity(), None);
}

/// Fills the level's rooms, then the loaded ones on top, each layer as one path: separate fills
/// per room would leave anti-aliasing seams along shared edges.
fn fill_rooms(pm: &mut Pixmap, view: View, snap: &Snapshot) {
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

/// The farm ring's star, on the ring of radius `size` around (`cx`, `cy`) at its upper right.
fn draw_farm_star(pm: &mut Pixmap, (cx, cy, size): (f32, f32, f32), colour: Rgba) {
    let at = size * std::f32::consts::FRAC_1_SQRT_2;
    if let Some(star) = marker_path(Shape::Star, cx + at, cy - at, FARM_STAR) {
        outline_then_fill(pm, &star, colour, &round_join(3.0));
    }
}

/// A drop's mark: the `ring` ([`draw_ring`]) along `circle` of radius `size` around (`cx`,
/// `cy`), with the farm star and the gold coin on it where they apply.
fn draw_mark_ring(pm: &mut Pixmap, ring: Marker, circle: &Path, (cx, cy, size): (f32, f32, f32)) {
    let Marker::Ring {
        look,
        uncertain,
        coin,
        ..
    } = ring
    else {
        return;
    };
    let colour = look.colour();
    draw_ring(pm, circle, colour, uncertain);
    if look == RingLook::Farm {
        draw_farm_star(pm, (cx, cy, size), colour);
    }
    if coin {
        draw_coin(pm, (cx, cy, size));
    }
}

/// Whether a marked drop sells for a lot to other players: a trade-category (not twink)
/// valuables match of at least [`COIN_MIN_TIER`].
pub(crate) fn high_value(mark: &Mark) -> bool {
    matches!(mark.trade, Some((tier, Category::Trade)) if tier >= COIN_MIN_TIER)
}

/// The gold coin on the ring of radius `size` around (`cx`, `cy`), at its upper left: a gold
/// disc with a darker rim.
fn draw_coin(pm: &mut Pixmap, (cx, cy, size): (f32, f32, f32)) {
    let at = size * std::f32::consts::FRAC_1_SQRT_2;
    let Some(coin) = PathBuilder::from_circle(cx - at, cy - at, COIN_RADIUS) else {
        return;
    };
    solid(pm, &coin, COIN_GOLD);
    let rim = Stroke {
        width: COIN_RIM_WIDTH,
        ..Stroke::default()
    };
    pm.stroke_path(&coin, &paint(COIN_RIM), &rim, Transform::identity(), None);
}

/// A well: the hollow `circle` in `colour` on a wider dark ring.
fn draw_well(pm: &mut Pixmap, circle: &Path, colour: Rgba) {
    for (colour, width) in [(MARKER_OUTLINE, WELL_BACKING_WIDTH), (colour, WELL_RING_WIDTH)] {
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(circle, &paint(colour), &stroke, Transform::identity(), None);
    }
}

/// A super chest: `body` ([`chest_path`]) on a dark backing, filled `colour`, with a gold rim
/// and a gold lid seam a third of the way down.
fn draw_chest(pm: &mut Pixmap, body: &Path, colour: Rgba, (cx, cy, size): (f32, f32, f32)) {
    let stroke = |width| Stroke {
        width,
        ..Stroke::default()
    };
    pm.stroke_path(
        body,
        &paint(MARKER_OUTLINE),
        &stroke(CHEST_BACKING_WIDTH),
        Transform::identity(),
        None,
    );
    solid(pm, body, colour);
    pm.stroke_path(
        body,
        &paint(LANDMARK_GOLD),
        &stroke(CHEST_RIM_WIDTH),
        Transform::identity(),
        None,
    );
    let seam_y = cy - size * CHEST_HEIGHT / 3.0;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - size, seam_y);
    pb.line_to(cx + size, seam_y);
    if let Some(seam) = pb.finish() {
        pm.stroke_path(
            &seam,
            &paint(LANDMARK_GOLD),
            &stroke(CHEST_RIM_WIDTH),
            Transform::identity(),
            None,
        );
    }
}

/// A superunique or boss skull: `dot` is its [`skull_path`] with the eye holes, centred on
/// (`cx`, `cy`) with `size`.
fn draw_skull(pm: &mut Pixmap, dot: &Path, colour: Rgba, (cx, cy, size): (f32, f32, f32), horns: bool) {
    // Only the outer contour is outlined: stroked eye holes would merge into one dark band at
    // 8 px, so they are cut out of the fill and filled dark on their own.
    let outline = Stroke {
        width: SKULL_OUTLINE_WIDTH,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    if let Some(outer) = skull_path(cx, cy, size, horns, false) {
        pm.stroke_path(
            &outer,
            &paint(MARKER_OUTLINE),
            &outline,
            Transform::identity(),
            None,
        );
    }
    pm.fill_path(
        dot,
        &paint(colour),
        FillRule::EvenOdd,
        Transform::identity(),
        None,
    );
    if let Some(eyes) = skull_eyes(cx, cy, size) {
        pm.fill_path(
            &eyes,
            &paint(MARKER_OUTLINE),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
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
            let (ux, uy) = iso(f64::from(dx), f64::from(dy));
            triangle_path(cx, cy, size, (px(ux), px(uy)))
        }
        _ => marker_path(shape, cx, cy, size),
    }
}

/// A triangle centred on (`cx`, `cy`) with its tip `size` px from the centre toward screen
/// direction `(ux, uy)` (any length; zero points up). It is 1.5 × `size` long and 1.3 × `size`
/// wide: size 7 gives about 10.5 × 9 px.
fn triangle_path(cx: f32, cy: f32, size: f32, (ux, uy): (f32, f32)) -> Option<Path> {
    let len = ux.hypot(uy);
    let (ux, uy) = if len > f32::EPSILON {
        (ux / len, uy / len)
    } else {
        (0.0, -1.0)
    };
    let (perp_x, perp_y) = (-uy, ux);
    let back = size * 0.5;
    let half_base = size * 0.65;
    let mut pb = PathBuilder::new();
    pb.move_to(ux.mul_add(size, cx), uy.mul_add(size, cy));
    pb.line_to(
        perp_x.mul_add(half_base, ux.mul_add(-back, cx)),
        perp_y.mul_add(half_base, uy.mul_add(-back, cy)),
    );
    pb.line_to(
        perp_x.mul_add(-half_base, ux.mul_add(-back, cx)),
        perp_y.mul_add(-half_base, uy.mul_add(-back, cy)),
    );
    pb.close();
    pb.finish()
}

/// Draws the mini orifice centred on (`cx`, `cy`) with outer radius `size`: a dark hole inside a
/// thick gold ring, and the Horadric Staff as a gold bar standing up out of it.
fn draw_orifice(pm: &mut Pixmap, colour: Rgba, (cx, cy, size): (f32, f32, f32)) {
    let stroke = |width| Stroke {
        width,
        line_cap: tiny_skia::LineCap::Round,
        ..Stroke::default()
    };
    let ring_r = size * 0.7;
    if let Some(hole) = PathBuilder::from_circle(cx, cy, size) {
        solid(pm, &hole, MARKER_OUTLINE);
    }
    if let Some(ring) = PathBuilder::from_circle(cx, cy, ring_r) {
        pm.stroke_path(
            &ring,
            &paint(colour),
            &stroke(size * 0.4),
            Transform::identity(),
            None,
        );
    }
    let mut pb = PathBuilder::new();
    pb.move_to(cx, ring_r.mul_add(0.3, cy));
    pb.line_to(cx, size.mul_add(-1.5, cy));
    if let Some(staff) = pb.finish() {
        pm.stroke_path(
            &staff,
            &paint(MARKER_OUTLINE),
            &stroke(4.0),
            Transform::identity(),
            None,
        );
        pm.stroke_path(&staff, &paint(colour), &stroke(2.0), Transform::identity(), None);
    }
}

/// Fills `path` in `colour`.
fn solid(pm: &mut Pixmap, path: &Path, colour: Rgba) {
    pm.fill_path(
        path,
        &paint(colour),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// The centre line of a worm: one and a half sine waves from `cx - size` to `cx + size`,
/// amplitude a third of `size`.
fn worm_path(cx: f32, cy: f32, size: f32) -> Option<Path> {
    const SEGMENTS: u8 = 16;
    let mut pb = PathBuilder::new();
    for i in 0..=SEGMENTS {
        let t = f32::from(i) / f32::from(SEGMENTS);
        let x = 2.0f32.mul_add(t, -1.0).mul_add(size, cx);
        let y = (t * 3.0 * std::f32::consts::PI).sin().mul_add(size / 3.0, cy);
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.finish()
}

/// Draws the worm `body` (from [`worm_path`]) in `colour` with a dark outline, and a round head
/// at its right end, for a worm centred on (`cx`, `cy`) of half-length `size`.
fn draw_worm(pm: &mut Pixmap, body: &Path, colour: Rgba, (cx, cy, size): (f32, f32, f32)) {
    let round = |width| Stroke {
        width,
        line_cap: tiny_skia::LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let head_x = cx + size;
    let head = PathBuilder::from_circle(head_x, cy, size * 0.3);
    pm.stroke_path(
        body,
        &paint(MARKER_OUTLINE),
        &round(5.0),
        Transform::identity(),
        None,
    );
    if let Some(head) = &head {
        pm.fill_path(
            head,
            &paint(MARKER_OUTLINE),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        pm.stroke_path(
            head,
            &paint(MARKER_OUTLINE),
            &round(2.0),
            Transform::identity(),
            None,
        );
    }
    pm.stroke_path(body, &paint(colour), &round(3.0), Transform::identity(), None);
    if let Some(head) = &head {
        pm.fill_path(
            head,
            &paint(colour),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// A staircase of three steps descending to the right, in a square of half-side `size` centred
/// on (`cx`, `cy`).
fn stairs_path(cx: f32, cy: f32, size: f32) -> Option<Path> {
    let third = size / 3.0;
    let (l, m1, m2, r) = (cx - size, cx - third, cx + third, cx + size);
    let (t, n1, n2, b) = (cy - size, cy - third, cy + third, cy + size);
    let mut pb = PathBuilder::new();
    pb.move_to(l, t);
    pb.line_to(m1, t);
    pb.line_to(m1, n1);
    pb.line_to(m2, n1);
    pb.line_to(m2, n2);
    pb.line_to(r, n2);
    pb.line_to(r, b);
    pb.line_to(l, b);
    pb.close();
    pb.finish()
}

/// The outline of a marker centred on (`cx`, `cy`); `None` for a degenerate size.
fn marker_path(shape: Shape, cx: f32, cy: f32, size: f32) -> Option<Path> {
    match shape {
        Shape::Triangle => triangle_path(cx, cy, size, (0.0, -1.0)),
        Shape::Stairs => stairs_path(cx, cy, size),
        Shape::Worm => worm_path(cx, cy, size),
        Shape::Skull => skull_path(cx, cy, size, false, true),
        Shape::HornedSkull => skull_path(cx, cy, size, true, true),
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
        Shape::Star => star_path(cx, cy, size),
        Shape::Chest => chest_path(cx, cy, size),
    }
}

/// Half-height of a chest as a fraction of its half-width.
const CHEST_HEIGHT: f32 = 0.7;

/// A chest box centred on (`cx`, `cy`), `2 × size` wide and `2 × 0.7 × size` tall.
fn chest_path(cx: f32, cy: f32, size: f32) -> Option<Path> {
    let half_h = size * CHEST_HEIGHT;
    Rect::from_xywh(cx - size, cy - half_h, 2.0 * size, 2.0 * half_h).map(PathBuilder::from_rect)
}

/// A five-pointed star, one tip up: ten points alternating tip (`size` from the centre) and
/// inner corner.
fn star_path(cx: f32, cy: f32, size: f32) -> Option<Path> {
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

/// A skull centred on (`cx`, `cy`), in units of `size` / 4: one outer contour (cranium arc,
/// optional horns, a narrower jaw) and, if `eyes`, the eye circles of [`skull_eyes`] as even-odd
/// holes. The contour is one polygon because overlapping cranium and jaw shapes would cancel out
/// under even-odd.
fn skull_path(cx: f32, cy: f32, size: f32, horns: bool, eyes: bool) -> Option<Path> {
    const CRANIUM_Y: f32 = -0.9;
    const CRANIUM_R: f32 = 3.0;
    const JAW_HALF: f32 = 1.5;
    const JAW_BOTTOM: f32 = 3.8;
    // The right horn's base as cranium angles (degrees, screen y down) and its tip; the left
    // horn mirrors it.
    const HORN_BASE: (f32, f32) = (-30.0, -62.0);
    const HORN_TIP: (f32, f32) = (3.7, -5.0);
    let k = size / 4.0;
    let at = |x: f32, y: f32| (x.mul_add(k, cx), y.mul_add(k, cy));
    let on_cranium = |deg: f32| {
        let (sin, cos) = deg.to_radians().sin_cos();
        at(CRANIUM_R * cos, CRANIUM_R.mul_add(sin, CRANIUM_Y))
    };
    let mut pb = PathBuilder::new();
    let line = |pb: &mut PathBuilder, (x, y): (f32, f32)| pb.line_to(x, y);
    let arc = |pb: &mut PathBuilder, from: f32, to: f32| {
        const STEPS: u8 = 20;
        for i in 0..=STEPS {
            let t = f32::from(i) / f32::from(STEPS);
            line(pb, on_cranium((to - from).mul_add(t, from)));
        }
    };
    let right_jaw_corner = (JAW_HALF / CRANIUM_R).acos().to_degrees();
    let (x, y) = at(JAW_HALF, JAW_BOTTOM);
    pb.move_to(x, y);
    if horns {
        let (tx, ty) = HORN_TIP;
        arc(&mut pb, right_jaw_corner, HORN_BASE.0);
        line(&mut pb, at(tx, ty));
        arc(&mut pb, HORN_BASE.1, -180.0 - HORN_BASE.1);
        line(&mut pb, at(-tx, ty));
        arc(&mut pb, -180.0 - HORN_BASE.0, -180.0 - right_jaw_corner);
    } else {
        arc(&mut pb, right_jaw_corner, -180.0 - right_jaw_corner);
    }
    line(&mut pb, at(-JAW_HALF, JAW_BOTTOM));
    pb.close();
    if eyes {
        push_eyes(&mut pb, cx, cy, size);
    }
    pb.finish()
}

/// The two eye circles of the skull of [`skull_path`], alone, to fill dark.
fn skull_eyes(cx: f32, cy: f32, size: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    push_eyes(&mut pb, cx, cy, size);
    pb.finish()
}

fn push_eyes(pb: &mut PathBuilder, cx: f32, cy: f32, size: f32) {
    const EYE_X: f32 = 1.5;
    const EYE_Y: f32 = -0.4;
    const EYE_R: f32 = 0.95;
    let k = size / 4.0;
    for side in [-1.0, 1.0] {
        pb.push_circle((side * EYE_X).mul_add(k, cx), EYE_Y.mul_add(k, cy), EYE_R * k);
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
    use std::collections::BTreeMap;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/jail1-snapshot.json");

    /// Shadows `super::render`: renders without the info line, so map tests don't depend on the
    /// fonts installed on this machine.
    fn render(snap: &Snapshot, width: u32, height: u32) -> Result<Pixmap> {
        render_with(snap, &Guidance::default(), width, height, None)
    }

    /// The marker kinds of `snap` without drop marks, in draw order.
    fn marker_kinds(snap: &Snapshot) -> Vec<Marker> {
        markers_with(snap, &Marks::default(), LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect()
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
            owner_id: 0,
            owner_type: 0,
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
            player_own: None,
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

    #[test]
    fn iso_matches_automap_axes() {
        assert_eq!(iso(0.0, 0.0), (0.0, 0.0));
        assert_eq!(iso(10.0, 0.0), (10.0, 5.0), "+x goes right-down");
        assert_eq!(iso(0.0, 10.0), (-10.0, 5.0), "+y goes left-down");
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
                seen_loaded: false,
                preset: None,
            },
            Room {
                x: 0,
                y: 30,
                w: 5,
                h: 10,
                loaded: true,
                seen_loaded: false,
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
        let mut trap = monster(0x08, 1);
        trap.txt = 410;
        assert!(!is_elite(&trap), "assassin trap with the unique flag");
        assert_eq!(elite_marker(&trap), None);
    }

    fn monster_txt(txt: u32, flags: u8, mode: u32) -> Monster {
        Monster {
            txt,
            ..monster(flags, mode)
        }
    }

    #[test]
    fn live_monster_icon_classification() {
        let (andariel, baal, baal_clone, nihlathak) = (156, 544, 570, 526);
        for txt in [andariel, baal, baal_clone, nihlathak] {
            assert_eq!(
                elite_marker(&monster_txt(txt, 0x02, 1)),
                Some(Marker::Boss),
                "{txt}"
            );
            assert_eq!(
                elite_marker(&monster_txt(txt, 0x08, 1)),
                Some(Marker::Boss),
                "{txt}"
            );
            assert_eq!(
                elite_marker(&monster_txt(txt, 0x00, 1)),
                Some(Marker::Boss),
                "{txt}: no elite flag needed"
            );
        }
        assert_eq!(elite_marker(&monster(0x02, 1)), Some(Marker::Superunique));
        assert_eq!(
            elite_marker(&monster(0x0A, 1)),
            Some(Marker::Superunique),
            "superunique beats unique"
        );
        assert_eq!(elite_marker(&monster(0x08, 1)), Some(Marker::Elite));
        assert_eq!(elite_marker(&monster(0x04, 1)), Some(Marker::Elite));
        assert_eq!(elite_marker(&monster(0x10, 1)), None, "minion only");
        assert_eq!(elite_marker(&monster(0x00, 1)), None, "normal");
        assert_eq!(elite_marker(&monster(0x02, 0)), None, "dead");
        assert_eq!(elite_marker(&monster_txt(andariel, 0x00, 12)), None, "dead boss");
        assert!(Marker::Shrine(1) < Marker::Elite);
        assert!(Marker::Elite < Marker::Superunique && Marker::Superunique < Marker::Boss);
        assert!(Marker::Boss < Marker::Player);
    }

    #[test]
    fn a_live_superunique_replaces_its_preset_marker() {
        let mut snap = empty_snapshot(5, 5);
        let at = |kind, txt, x, y| Preset {
            x,
            y,
            ..preset(kind, txt)
        };
        snap.presets = vec![
            at(PresetKind::Monster, 757, 100, 100), // the Countess, 19 tiles off
            at(PresetKind::Monster, 771, 200, 100), // the Smith, far away
            at(PresetKind::Monster, 434, 110, 110), // quest monster near her
            at(PresetKind::Object, 460, 101, 100),  // quest object
            at(PresetKind::Tile, 1, 100, 100),      // exit
        ];
        let countess = |mode| Monster {
            x: 115,
            y: 112,
            ..monster(0x02, mode)
        };

        snap.monsters = vec![countess(1)];
        assert_eq!(
            marker_kinds(&snap),
            vec![
                Marker::Exit,
                Marker::Special(Some("The Smith")),
                Marker::QuestObject("anya"),
                Marker::Superunique,
                Marker::Player
            ]
        );
        snap.monsters = vec![Monster {
            x: 100,
            y: 100,
            ..monster(0x08, 1)
        }];
        assert_eq!(
            marker_kinds(&snap)
                .iter()
                .filter(|m| matches!(m, Marker::Special(_)))
                .count(),
            2,
            "a plain unique hides nothing"
        );
        snap.monsters = vec![countess(12)];
        assert_eq!(
            marker_kinds(&snap),
            vec![
                Marker::Exit,
                Marker::Special(Some("The Countess")),
                Marker::Special(Some("The Smith")),
                Marker::QuestObject("anya"),
                Marker::QuestObject("prison door"),
                Marker::Player
            ],
            "dead: the presets are back"
        );
        snap.monsters = vec![Monster {
            x: 105,
            y: 100,
            ..monster_txt(526, 0x00, 1)
        }];
        assert!(!marker_kinds(&snap).contains(&Marker::Special(Some("The Countess"))));
        assert!(marker_kinds(&snap).contains(&Marker::Boss));
    }

    #[test]
    fn skull_icons_draw_red_within_the_elite_dot_footprint() -> Result<()> {
        for (flags, txt, want) in [(0x02, 1, Marker::Superunique), (0x00, 544, Marker::Boss)] {
            let mut snap = empty_snapshot(5, 5);
            snap.monsters = vec![Monster {
                x: 60,
                y: 60,
                ..monster_txt(txt, flags, 1)
            }];
            assert!(marker_kinds(&snap).contains(&want), "{want:?}");
            let pm = render(&snap, 200, 200)?;
            let at = View::fit(level_bounds(&snap), 200, 200).to_screen(60.0, 60.0);
            let red = |p: Rgba| p[0] > 180 && p[1] < 80 && p[2] < 80 && p[3] > 200;
            let mut hits = 0;
            for y in 0..pm.height() {
                for x in 0..pm.width() {
                    if red(rgba(&pm, x, y)?) {
                        let (dx, dy) = (
                            f64::from(x) + 0.5 - f64::from(at.0),
                            f64::from(y) + 0.5 - f64::from(at.1),
                        );
                        assert!(dx.abs() <= 4.5 && dy.abs() <= 4.5, "{want:?}: red at {dx},{dy}");
                        hits += 1;
                    }
                }
            }
            assert!(hits >= 10, "{want:?}: only {hits} red pixels");
            #[allow(
                clippy::cast_sign_loss,
                clippy::cast_possible_truncation,
                reason = "test points are inside the canvas"
            )]
            let pixel =
                |dx: f32, dy: f32| ((at.0 + dx - 0.5).round() as u32, (at.1 + dy - 0.5).round() as u32);
            let (ox, oy) = pixel(0.0, -4.0);
            let top = rgba(&pm, ox, oy)?;
            assert!(top[0] < 120 && top[3] > 100, "{want:?}: outline above: {top:?}");
            let (ex, ey) = pixel(1.2, -0.4);
            assert!(!red(rgba(&pm, ex, ey)?), "{want:?}: eye hole is not red");
        }
        Ok(())
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
    fn jail_fixture_has_a_waypoint_an_exit_and_no_elites() -> Result<()> {
        let snap = fixture()?;
        assert_eq!(
            markers_with(&snap, &Marks::default(), LANDMARKS),
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
        let spirit_wolf = monster_txt(420, 0x00, 1);
        snap.monsters = vec![
            monster(0x00, 1),
            monster(0x10, 1),
            monster(0x08, 1),
            monster(0x00, 12),
            spirit_wolf,
        ];
        assert_eq!(
            marker_kinds(&snap),
            vec![Marker::Elite, Marker::Player],
            "elsewhere: elites only"
        );
        snap.level.id = 8;
        assert_eq!(
            marker_kinds(&snap),
            vec![Marker::Monster, Marker::Monster, Marker::Elite, Marker::Player],
            "Den of Evil: normals and minions too, dead skipped, below the elite"
        );
    }

    #[test]
    fn markers_draw_player_last() {
        let mut snap = empty_snapshot(5, 5);
        snap.monsters = vec![monster(0x08, 1), monster(0x10, 1)];
        snap.presets = vec![preset(PresetKind::Monster, 1), preset(PresetKind::Tile, 1)];
        assert_eq!(
            marker_kinds(&snap),
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
        assert_eq!(
            pm.pixel(0, 0).map(tiny_skia::PremultipliedColorU8::alpha),
            Some(0),
            "no background panel"
        );
        for (x, y) in [(3, h / 2), (w / 2, 0), (w - 1, h - 1), (w / 2, h - 3)] {
            assert_eq!(pm.pixel(x, y).context("edge pixel")?.alpha(), 0, "{x},{y}");
        }
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
            seen_loaded: false,
            preset: None,
        }
    }

    #[test]
    fn underground_passage_exit_is_a_star_and_stairs_down_a_staircase() -> Result<()> {
        let mut snap = empty_snapshot(5, 5);
        snap.level.id = 10;
        let piece = |x, preset| Room {
            preset: Some(preset),
            ..room(x, 0, false)
        };
        snap.level.rooms = vec![piece(0, 90), piece(120, 91), piece(240, 92)];
        let kinds: Vec<Marker> = landmarks(&snap, LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
        assert_eq!(
            kinds,
            vec![
                Marker::Landmark("cave exit to the next area (north)"),
                Marker::StairsDown,
                Marker::StairsDown,
            ]
        );
        let (cx, cy) = (20.0, 20.0);
        let path = marker_path(Shape::Stairs, cx, cy, 6.0).context("stairs path")?;
        let mut pm = Pixmap::new(40, 40).context("pixmap")?;
        pm.fill_path(
            &path,
            &paint(Marker::Exit.style().0),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        assert_eq!(rgba(&pm, 18, 24)?, Marker::Exit.style().0, "bottom step filled");
        assert_eq!(rgba(&pm, 24, 16)?[3], 0, "top-right corner empty");
        Ok(())
    }

    #[test]
    fn coldworm_chamber_draws_a_gold_worm() -> Result<()> {
        let mut snap = empty_snapshot(5, 5);
        snap.level.id = 64;
        snap.level.rooms = vec![Room {
            preset: Some(509),
            ..room(0, 0, false)
        }];
        let kinds: Vec<Marker> = landmarks(&snap, LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
        assert_eq!(kinds, vec![Marker::Worm]);
        let mut pm = Pixmap::new(40, 40).context("pixmap")?;
        let (cx, cy, size) = (20.0, 20.0, 9.0);
        let body = worm_path(cx, cy, size).context("worm path")?;
        draw_worm(&mut pm, &body, LANDMARK_GOLD, (cx, cy, size));
        let gold = (0..40u32)
            .flat_map(|y| (0..40u32).map(move |x| (x, y)))
            .filter(|&(x, y)| rgba(&pm, x, y).is_ok_and(|p| p == LANDMARK_GOLD))
            .count();
        assert!(gold >= 30, "gold pixels {gold}");
        assert_eq!(rgba(&pm, 20, 5)?[3], 0, "nothing far above the body");
        Ok(())
    }

    #[test]
    fn super_chests_draw_as_gold_rimmed_chests_in_kurast_only() -> Result<()> {
        let mut snap = empty_snapshot(5, 5);
        snap.presets = vec![object(584, 50, 50), object(181, 60, 60)];
        let chests = |snap: &Snapshot| {
            markers_with(snap, &Marks::default(), LANDMARKS)
                .into_iter()
                .filter(|&(m, _, _)| m == Marker::SuperChest)
                .count()
        };
        for (level, want) in [(78, 0), (79, 1), (80, 1), (81, 1), (82, 0), (108, 0)] {
            snap.level.id = level;
            assert_eq!(chests(&snap), want, "level {level}");
        }
        assert_eq!(
            preset_marker(&object(181, 0, 0)),
            None,
            "an ordinary jungle chest"
        );
        assert_eq!(
            preset_marker(&Preset {
                kind: PresetKind::Monster,
                ..object(584, 0, 0)
            }),
            Some(Marker::Special(None)),
            "584 on a monster preset is no chest"
        );

        let (colour, _, shape) = Marker::SuperChest.style();
        assert_eq!((colour, shape), (CHEST_WOOD, Shape::Chest));
        let mut pm = Pixmap::new(40, 40).context("pixmap")?;
        draw_marker(&mut pm, None, Marker::SuperChest, 20.0, 20.0);
        // A 10 × 7 px box: wood inside, gold rim and lid seam, a dark edge, nothing beyond.
        assert_eq!(rgba(&pm, 20, 21)?, CHEST_WOOD, "body");
        assert_eq!(rgba(&pm, 20, 18)?, LANDMARK_GOLD, "lid seam");
        assert_eq!(rgba(&pm, 20, 23)?, LANDMARK_GOLD, "bottom rim");
        let edge = rgba(&pm, 20, 24)?;
        assert!(
            edge[0] < 100 && edge[3] > 200,
            "dark edge under the rim: {edge:?}"
        );
        assert_eq!(rgba(&pm, 20, 13)?[3], 0, "nothing above the lid");
        assert_eq!(rgba(&pm, 27, 20)?[3], 0, "nothing right of the box");
        Ok(())
    }

    #[test]
    fn adjacent_rooms_render_without_seams() -> Result<()> {
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
        for i in 1..60 {
            assert_eq!(inside(60.0, f64::from(i))?, fill, "seam at edge tile {i}");
        }
        assert_eq!(inside(90.0, 30.0)?, fill, "both rooms in the same colour");

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
            cell: None,
            id: 1,
            code: None,
            kind,
            quality: 0,
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

    #[test]
    fn drops_are_markers_above_elites() {
        let mut snap = empty_snapshot(5, 5);
        snap.monsters = vec![monster(0x08, 1)];
        snap.drops = vec![
            drop_at(DropKind::Unique, 1, 1),
            drop_at(DropKind::Rune(8), 2, 2),
            drop_at(DropKind::Gem(GemColour::Ruby), 3, 3),
        ];
        let kinds: Vec<Marker> = markers_with(&snap, &Marks::default(), LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
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
            assert_ne!(
                Marker::Drop(kind).style().2,
                Shape::Circle,
                "{kind:?} looks like a dot"
            );
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

    /// A drop of `kind` at (`x`, 30) with unit id 1000 + `x`, and its mark of `look`.
    fn highlighted(kind: DropKind, x: u32, look: RingLook, uncertain: bool) -> (ItemDrop, (u32, Mark)) {
        let mut d = drop_at(kind, x, 30);
        d.id = 1000 + x;
        let mark = Mark {
            kind: match look {
                RingLook::Keep => MarkKind::Keep,
                RingLook::Farm => MarkKind::Farm,
                RingLook::Trade(_) => MarkKind::Trade,
            },
            trade: match look {
                RingLook::Trade(tier) => Some((tier, crate::guidance::Category::Trade)),
                RingLook::Keep | RingLook::Farm => None,
            },
            rule: "test".into(),
            label: "Test".into(),
            uncertain,
        };
        let id = d.id;
        (d, (id, mark))
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
        let (ringed, mark) = highlighted(DropKind::Base, 1, RingLook::Trade(Tier::A), false);
        snap.drops = vec![ringed, drop_at(DropKind::Rune(1), 2, 2)];
        let kinds: Vec<Marker> = markers_with(&snap, &Marks::from([mark]), LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
        assert_eq!(
            kinds,
            vec![
                Marker::Player,
                Marker::Drop(DropKind::Base),
                Marker::Drop(DropKind::Rune(1)),
                Marker::Ring {
                    kind: DropKind::Base,
                    look: RingLook::Trade(Tier::A),
                    uncertain: false,
                    coin: true,
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
        let marks: Marks;
        (snap.drops, marks) = [
            highlighted(DropKind::Unique, 20, RingLook::Trade(Tier::S), false),
            highlighted(DropKind::Base, 60, RingLook::Trade(Tier::A), false),
            highlighted(DropKind::Base, 100, RingLook::Trade(Tier::B), false),
            highlighted(DropKind::Set, 140, RingLook::Trade(Tier::S), true),
        ]
        .into_iter()
        .unzip();
        let (w, h) = (400, 400);
        let guidance = Guidance {
            marks,
            ..Guidance::default()
        };
        let pm = render_with(&snap, &guidance, w, h, None)?;
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
            assert!(
                hits >= 58,
                "solid {colour:?} ring (less the coin) at x {x}: {hits}/72"
            );
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
    fn keep_and_farm_rings_are_green_and_the_farm_ring_has_a_star() -> Result<()> {
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, false), room(60, 0, true)];
        let marks: Marks;
        (snap.drops, marks) = [
            highlighted(DropKind::Base, 20, RingLook::Keep, false),
            highlighted(DropKind::Unique, 100, RingLook::Farm, true),
        ]
        .into_iter()
        .unzip();
        let (w, h) = (400, 400);
        let guidance = Guidance {
            marks,
            ..Guidance::default()
        };
        let pm = render_with(&snap, &guidance, w, h, None)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let at = v.to_screen(20.0, 30.0);
        let radius = Marker::Drop(DropKind::Base).style().1 + RING_GAP;
        let hits = ring_hits(&pm, at, radius, KEEP_GREEN, 72)?;
        assert!(hits >= 68, "solid green keep ring: {hits}/72");
        let at = v.to_screen(100.0, 30.0);
        let radius = Marker::Drop(DropKind::Unique).style().1 + RING_GAP;
        let hits = ring_hits(&pm, at, radius, KEEP_GREEN, 72)?;
        assert!((20..=60).contains(&hits), "dashed farm ring: {hits}/72");
        let star = radius * std::f32::consts::FRAC_1_SQRT_2;
        let star_hits = ring_hits(&pm, (at.0 + star, at.1 - star), 1.5, KEEP_GREEN, 12)?;
        assert!(star_hits >= 10, "the star's body is green: {star_hits}/12");
        assert_ne!(
            KEEP_GREEN,
            Marker::Drop(DropKind::Set).style().0,
            "apart from set diamonds"
        );
        Ok(())
    }

    #[test]
    fn high_value_trade_rings_carry_a_gold_coin() -> Result<()> {
        let mut snap = empty_snapshot(5, 115);
        snap.level.rooms = vec![room(0, 0, false), room(60, 0, true)];
        let mut twink = highlighted(DropKind::Base, 140, RingLook::Trade(Tier::S), false);
        twink.1.1.trade = Some((Tier::S, Category::Twink));
        let marks: Marks;
        (snap.drops, marks) = [
            highlighted(DropKind::Unique, 20, RingLook::Trade(Tier::S), false),
            highlighted(DropKind::Base, 60, RingLook::Trade(Tier::A), true),
            highlighted(DropKind::Base, 100, RingLook::Trade(Tier::B), false),
            twink,
        ]
        .into_iter()
        .unzip();
        let (w, h) = (400, 400);
        let guidance = Guidance {
            marks,
            ..Guidance::default()
        };
        let pm = render_with(&snap, &guidance, w, h, None)?;
        let v = View::fit(level_bounds(&snap), w, h);
        for (x, kind, coin) in [
            (20.0, DropKind::Unique, true),
            (60.0, DropKind::Base, true),
            (100.0, DropKind::Base, false),
            (140.0, DropKind::Base, false),
        ] {
            let at = v.to_screen(x, 30.0);
            let d = (Marker::Drop(kind).style().1 + RING_GAP) * std::f32::consts::FRAC_1_SQRT_2;
            let upper_left = (at.0 - d, at.1 - d);
            // The rim, not the gold, tells the coin apart from the B ring's yellow.
            let rim = ring_hits(&pm, upper_left, COIN_RADIUS, COIN_RIM, 24)?;
            if coin {
                let gold = ring_hits(&pm, upper_left, 1.0, COIN_GOLD, 12)?;
                assert!(gold >= 10, "gold coin at x {x}: {gold}/12");
                assert!(rim >= 18, "darker rim at x {x}: {rim}/24");
            } else {
                assert!(rim <= 2, "no coin at x {x}: {rim}/24 rim");
            }
        }
        Ok(())
    }

    /// Pixels near `colour` (glyph cores and anti-aliasing allowed for) farther than `clear` px
    /// and at most `reach` px from `at`.
    fn tinted_between(pm: &Pixmap, at: (f32, f32), (clear, reach): (f32, f32), colour: Rgba) -> Result<u32> {
        let mut n = 0;
        for y in 0..pm.height() {
            for x in 0..pm.width() {
                let d = (f32::from(u16::try_from(x)?) + 0.5 - at.0)
                    .hypot(f32::from(u16::try_from(y)?) + 0.5 - at.1);
                let p = rgba(pm, x, y)?;
                let close = p.iter().zip(colour).take(3).all(|(&a, b)| a.abs_diff(b) <= 60);
                n += u32::from(d > clear && d <= reach && close && p[3] >= 200);
            }
        }
        Ok(n)
    }

    #[test]
    fn a_marked_drop_gets_its_tag_in_the_ring_colour_outside_the_ring() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let (w, h) = (400, 400);
        let orange = RingLook::Trade(Tier::A).colour();
        let render_tag = |label: &str| -> Result<u32> {
            let (d, (id, mut mark)) = highlighted(DropKind::Base, 60, RingLook::Trade(Tier::A), false);
            mark.label = label.into();
            let mut snap = empty_snapshot(5, 115);
            snap.level.rooms = vec![room(0, 0, false), room(60, 0, true)];
            snap.drops = vec![d];
            let guidance = Guidance {
                marks: Marks::from([(id, mark)]),
                ..Guidance::default()
            };
            let pm = render_with(&snap, &guidance, w, h, Some(font))?;
            let v = View::fit(level_bounds(&snap), w, map_height(h, Some(font)));
            let at = v.to_screen(60.0, 30.0);
            let ring = Marker::Drop(DropKind::Base).style().1 + RING_GAP;
            tinted_between(&pm, at, (ring + 2.5, 40.0), orange)
        };
        let with_tag = render_tag("5os")?;
        assert!(with_tag >= 8, "only {with_tag} orange tag pixels");
        assert_eq!(render_tag("")?, 0, "no tag for an empty label");
        Ok(())
    }

    #[test]
    fn drop_tags_rank_the_most_valuable_first() {
        let mut snap = empty_snapshot(5, 5);
        let marks: Marks;
        (snap.drops, marks) = [
            highlighted(DropKind::Base, 20, RingLook::Keep, false),
            highlighted(DropKind::Base, 40, RingLook::Trade(Tier::B), false),
            highlighted(DropKind::Unique, 60, RingLook::Trade(Tier::S), false),
            highlighted(DropKind::Base, 80, RingLook::Farm, false),
        ]
        .into_iter()
        .unzip();
        let v = View::fit(level_bounds(&snap), 400, 400);
        let looks: Vec<RingLook> = drop_tags(v, &snap, &marks).iter().map(|t| t.look).collect();
        assert_eq!(
            looks,
            vec![
                RingLook::Trade(Tier::S),
                RingLook::Trade(Tier::B),
                RingLook::Farm,
                RingLook::Keep
            ]
        );
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

    /// Light grey to white: glyph cores (thin strokes, so few are pure white).
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
        // Exit labels are white text on the map too.
        snap.room_links.clear();
        snap.border_links.clear();
        let (w, h) = (400, 400);
        let pm = render_with(&snap, &Guidance::default(), w, h, Some(font))?;
        let top = map_height(h, Some(font));
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
        let plain = render_with(&snap, &Guidance::default(), w, h, None)?;
        assert_eq!(map_height(h, None), h);
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
        let violet_core =
            |p: Rgba| p[3] >= 200 && p[2] >= 180 && p[0] >= 120 && p[0].saturating_sub(p[1]) >= 40;
        let (w, h) = (400, 400);
        let top = map_height(h, Some(font));
        let count = |pm: &Pixmap| -> Result<u32> {
            let mut n = 0;
            for y in top..pm.height() {
                for x in 0..pm.width() {
                    n += u32::from(violet_core(rgba(pm, x, y)?));
                }
            }
            Ok(n)
        };
        let mut snap = fixture()?;
        snap.difficulty = Some(0);
        assert_eq!(
            count(&render_with(&snap, &Guidance::default(), w, h, Some(font))?)?,
            0,
            "not terrorized"
        );
        snap.terrorized = true;
        let pm = render_with(&snap, &Guidance::default(), w, h, Some(font))?;
        assert!(count(&pm)? >= 20, "only {} violet pixels", count(&pm)?);
        Ok(())
    }

    /// A helm `BiS` chase list and one item-plan entry per action.
    fn sample_guidance() -> Guidance {
        use crate::guidance::Spot;
        let advice = |name: &str, spot, action| Advice {
            cells: Vec::new(),
            name: name.into(),
            spot,
            action,
            reason: None,
            count: 1,
        };
        Guidance {
            bis: Some(crate::guidance::Bis {
                name: String::new(),
                build: String::new(),
                slots: BTreeMap::from([("helm".to_owned(), vec!["Peasant Crown@28".to_owned()])]),
                merc_slots: BTreeMap::new(),
            }),
            items: vec![
                advice("Gravenspine", Spot::Inventory, Action::Sell),
                advice("Hunter's Bow 4os", Spot::Page(4), Action::Move(Side::Own)),
                advice("Spirit", Spot::Page(1), Action::Keep(Side::Own)),
                advice("Harlequin Crest", Spot::Page(3), Action::Keep(Side::Trade)),
                advice("Magefist", Spot::Stash, Action::GiveAway),
            ],
            ..Guidance::default()
        }
    }

    #[test]
    fn text_blocks_holds_the_dashboard_blocks() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let mut snap = fixture()?;
        snap.difficulty = Some(0);
        snap.player.char_level = Some(34);
        let blocks = text_blocks(&snap, &sample_guidance(), 400, Some(font));
        let text = |list: &[Advice]| list.iter().map(info::advice_entry).collect::<Vec<_>>();
        assert_eq!(text(&blocks.sell), vec!["Gravenspine (inv)"]);
        assert_eq!(text(&blocks.moves), vec!["Hunter's Bow 4os (page 4 → 1–2)"]);
        assert_eq!(text(&blocks.keep_own), vec!["Spirit (page 1)"]);
        assert_eq!(text(&blocks.keep_trade), vec!["Harlequin Crest (page 3)"]);
        assert_eq!(text(&blocks.give), vec!["Magefist (stash)"]);
        let Some(bis) = blocks.bis.first() else {
            anyhow::bail!("no helm chase line: {:?}", blocks.bis);
        };
        assert_eq!(bis.text, "helm: Peasant Crown (28)");
        assert_eq!(bis.tick, None, "nothing owned yet");
        Ok(())
    }

    #[test]
    fn the_overlay_frame_draws_no_guide_or_lists() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let mut snap = fixture()?;
        snap.difficulty = Some(0);
        snap.player.char_level = Some(34);
        snap.player.speed = Some(crate::game::SpeedStats {
            fcr: 22,
            ias: 0,
            fhr: 48,
            res_fire: 40,
            res_cold: 75,
            res_lightning: 10,
            res_poison: 75,
        });
        let (w, h) = (400, 400);
        let pm = render_with(&snap, &sample_guidance(), w, h, Some(font))?;
        let solid = |p: Rgba| p[3] == 255;
        let violet = |p: Rgba| solid(p) && p[2] > 200 && p[0] > 150 && p[1] < 180;
        let cyan = |p: Rgba| solid(p) && p[2] > 200 && p[1] > 170 && p[0] < 160;
        let yellow = |p: Rgba| solid(p) && p[0] > 200 && p[1] > 170 && p[2] < 120;
        let green = |p: Rgba| solid(p) && p[1] > 180 && p[0] < 140 && p[2] < 140;
        let gold = |p: Rgba| solid(p) && p[0] > 200 && p[1] > 170 && (100..=170).contains(&p[2]);
        for (name, pred) in [
            ("guide", &violet as &dyn Fn(Rgba) -> bool),
            ("level", &cyan),
            ("keep", &green),
            ("give away", &yellow),
            ("BiS", &gold),
        ] {
            let mut n = 0;
            for y in h - info::band_height(font)..h {
                for x in 0..w {
                    n += u32::from(pred(rgba(&pm, x, y)?));
                }
            }
            assert_eq!(n, 0, "no {name} text in the overlay frame");
        }
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
        let top = map_height(h, Some(font));
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
        let pm = render_with(&snap, &Guidance::default(), w, h, Some(font))?;
        for y in top..h {
            for x in 0..w {
                let p = rgba(&pm, x, y)?;
                let grey = p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1;
                assert!(p[3] == 0 || grey, "room fill {p:?} in the text band at {x},{y}");
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
                seen_loaded: false,
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
                (
                    Marker::BorderExit {
                        dir: (0, 80),
                        entry: true
                    },
                    160.0,
                    140.5
                ),
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
    fn one_exit_label_per_linked_level() {
        let got = exit_labels(&bordered(28));
        assert_eq!(
            got,
            vec![
                (29, 160.0, 120.0), // the room's centre: no tile preset loaded
                (26, 160.0, 140.5),
                (28, 140.0, 99.5),
            ]
        );
        let mut snap = bordered(28);
        snap.presets = vec![preset(PresetKind::Tile, 1)];
        if let Some(p) = snap.presets.first_mut() {
            (p.x, p.y) = (150, 110);
        }
        assert_eq!(
            exit_labels(&snap).first(),
            Some(&(29, 150.0, 110.0)),
            "on the exit tile"
        );
    }

    /// The canvas box `[x0, y0, x1, y1]` of the pixels `hit` picks in the map area, and their
    /// count; `None` if there are none.
    fn ink_box(pm: &Pixmap, rows: u32, hit: impl Fn([u8; 4]) -> bool) -> Result<Option<([u32; 4], u32)>> {
        let mut found: Option<([u32; 4], u32)> = None;
        for y in 0..rows {
            for x in 0..pm.width() {
                if hit(rgba(pm, x, y)?) {
                    let ([x0, y0, x1, y1], n) = found.unwrap_or(([x, y, x, y], 0));
                    found = Some(([x0.min(x), y0.min(y), x1.max(x), y1.max(y)], n + 1));
                }
            }
        }
        Ok(found)
    }

    fn is_gold(p: [u8; 4]) -> bool {
        p[3] >= 250 && p[0] >= 240 && (180..=215).contains(&p[1]) && p[2] <= 70
    }

    /// One room in the Canyon of the Magi linking to tomb level `tomb`, clvl 15 (full xp there).
    fn canyon_exit(tomb: u32) -> Snapshot {
        let mut snap = empty_snapshot(120, 120);
        snap.level.id = 46;
        snap.level.rooms = vec![room(100, 100, true)];
        snap.room_links = vec![RoomLink {
            from_room: RoomPos { x: 100, y: 100 },
            to_level_id: tomb,
            to_room: RoomPos { x: 0, y: 0 },
        }];
        snap.difficulty = Some(0);
        snap.player.char_level = Some(15);
        snap
    }

    #[test]
    fn tomb_exits_show_a_symbol_not_the_name() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let (w, h) = (400, 400);
        let pm = render_with(&canyon_exit(69), &Guidance::default(), w, h, Some(font))?;
        let (b, n) = ink_box(&pm, map_height(h, Some(font)), is_text_white)?.context("no white symbol")?;
        assert!(
            n >= 20 && b[2] - b[0] <= 20 && b[3] - b[1] <= 20,
            "{n} px in {b:?}"
        );
        assert!(
            ink_box(&pm, map_height(h, Some(font)), is_gold)?.is_none(),
            "not the real tomb"
        );
        Ok(())
    }

    #[test]
    fn the_real_tomb_is_gold_with_a_star() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let (w, h) = (400, 400);
        let rows = map_height(h, Some(font));
        let mut snap = canyon_exit(69);
        snap.staff_tomb = Some(69);
        let pm = render_with(&snap, &Guidance::default(), w, h, Some(font))?;
        let (b, n) = ink_box(&pm, rows, is_gold)?.context("no gold symbol")?;
        assert!(
            n >= 60 && b[2] - b[0] >= 30,
            "bigger, with a star: {n} px in {b:?}"
        );
        assert!(ink_box(&pm, rows, is_text_white)?.is_none(), "drawn white too");
        snap.staff_tomb = Some(70);
        let pm = render_with(&snap, &Guidance::default(), w, h, Some(font))?;
        assert!(
            ink_box(&pm, rows, is_gold)?.is_none(),
            "another tomb's exit stays white"
        );
        Ok(())
    }

    #[test]
    fn tomb_symbols_draw_distinct_glyphs() -> Result<()> {
        let mut masks: Vec<(tombs::Symbol, Vec<bool>)> = Vec::new();
        for id in tombs::TOMB_LEVELS {
            let symbol = tombs::symbol(id).context("tomb without symbol")?;
            let mut pm = Pixmap::new(40, 40).context("pixmap")?;
            draw_tomb_label(&mut pm, symbol, false, LABEL_TEXT, (10.0, 30.0));
            let mask: Vec<bool> = pm
                .pixels()
                .iter()
                .map(|p| p.alpha() >= 128 && p.red() >= 200)
                .collect();
            let ink = mask.iter().filter(|&&on| on).count();
            assert!(ink >= 15, "{symbol:?}: {ink} px");
            for (other, seen) in &masks {
                let differ = mask.iter().zip(seen).filter(|(a, b)| a != b).count();
                assert!(
                    differ >= 10,
                    "{symbol:?} looks like {other:?} ({differ} px differ)"
                );
            }
            masks.push((symbol, mask));
        }
        Ok(())
    }

    #[test]
    fn exit_labels_are_grey_where_xp_is_low() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let bloody_foothills = 110;
        let mut snap = empty_snapshot(120, 120);
        snap.level.rooms = vec![room(100, 100, true)];
        snap.room_links = vec![RoomLink {
            from_room: RoomPos { x: 100, y: 100 },
            to_level_id: bloody_foothills,
            to_room: RoomPos { x: 0, y: 0 },
        }];
        snap.difficulty = Some(0);
        let count = |snap: &Snapshot| -> Result<(u32, u32)> {
            let (w, h) = (400, 400);
            let pm = render_with(snap, &Guidance::default(), w, h, Some(font))?;
            let (mut grey, mut white) = (0, 0);
            for y in 0..map_height(h, Some(font)) {
                for x in 0..w {
                    let p = rgba(&pm, x, y)?;
                    if p[3] >= 250 && p.iter().take(3).all(|&c| (125..=155).contains(&c)) {
                        grey += 1;
                    } else if is_text_white(p) {
                        white += 1;
                    }
                }
            }
            Ok((grey, white))
        };
        snap.player.char_level = Some(10);
        let (grey, white) = count(&snap)?;
        assert!(grey >= 20 && white == 0, "low xp: {grey} grey, {white} white");
        snap.player.char_level = Some(20);
        let (grey, white) = count(&snap)?;
        assert!(
            white >= 20 && grey * 4 < white,
            "full xp: {grey} grey, {white} white"
        );
        Ok(())
    }

    #[test]
    fn border_exit_triangle_points_outward_on_screen() -> Result<()> {
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
            "world -y is screen up-right; tip {tx},{ty}"
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
            assert!(hits >= 20, "only {hits} pixels (entry {entry})");
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
        let orifice = Marker::Orifice;
        let mut snap = sanctuary([Some(462), Some(424), None, None]);
        snap.level.id = 72;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(orifice, 30.0, 30.0)]);
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
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(orifice, 60.0, 60.0)],
            "one marker at the centre of a multi-room chamber"
        );
        let mut snap = sanctuary([Some(473), Some(424), None, None]);
        snap.level.id = 71;
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(Marker::Landmark("tomb treasure"), 30.0, 30.0)],
            "a fake tomb's chest chamber"
        );
        let mut snap = sanctuary([None, Some(460), None, None]);
        snap.level.id = 66;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(Marker::Orifice, 90.0, 30.0)]);
        let mut snap = sanctuary([None, Some(462), None, None]);
        snap.level.id = 66;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(orifice, 90.0, 30.0)]);
        let mut snap = sanctuary([None, Some(456), None, None]);
        snap.level.id = 66;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![], "a cube piece in a tomb");
        let mut snap = sanctuary([Some(462), None, None, None]);
        snap.level.id = 73;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![], "outside the tombs");
    }

    #[test]
    fn summoner_rule_marks_the_room_centre() {
        let summoner = Marker::Landmark("summoner");
        for id in 525..=528 {
            let snap = sanctuary([None, Some(id), None, None]);
            assert_eq!(landmarks(&snap, LANDMARKS), vec![(summoner, 90.0, 30.0)], "{id}");
        }
        let snap = sanctuary([None, Some(526), Some(526), None]);
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(summoner, 120.0, 30.0)],
            "one star for adjacent rooms of one preset"
        );
        let snap = sanctuary([Some(525), None, None, Some(528)]);
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(summoner, 30.0, 30.0), (summoner, 210.0, 30.0)],
            "a star per separate piece"
        );
        let snap = sanctuary([Some(525), Some(528), None, None]);
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(summoner, 30.0, 30.0), (summoner, 90.0, 30.0)],
            "a star per preset for adjacent pieces"
        );
        let snap = sanctuary([Some(510), Some(524), Some(529), None]);
        assert_eq!(landmarks(&snap, LANDMARKS), vec![], "ordinary arm ends");
        let mut snap = sanctuary([Some(525), None, None, None]);
        snap.level.id = 73;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![], "another level");
        snap.level.id = 74;
        snap.monsters = vec![monster(0x02, 1)];
        assert_eq!(
            marker_kinds(&snap),
            vec![summoner, Marker::Superunique, Marker::Player]
        );
    }

    #[test]
    fn gidbinn_rule_marks_every_clearing_but_the_waypoint_one() {
        let gidbinn = Marker::Landmark("gidbinn");
        let mut snap = sanctuary([Some(594), Some(595), Some(604), Some(605)]);
        snap.level.id = 78;
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(gidbinn, 90.0, 30.0), (gidbinn, 150.0, 30.0)],
            "clearings 595-604 only"
        );
        let mut snap = sanctuary([Some(602), Some(602), None, Some(602)]);
        snap.level.id = 78;
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(gidbinn, 60.0, 30.0), (gidbinn, 210.0, 30.0)]
        );
        snap.presets = vec![object(237, 70, 10)];
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(gidbinn, 210.0, 30.0)],
            "the waypoint's clearing is skipped"
        );
        snap.presets.push(object(252, 200, 20));
        assert_eq!(landmarks(&snap, LANDMARKS), vec![], "the altar is loaded");
        let stars: Vec<Marker> = marker_kinds(&snap)
            .into_iter()
            .filter(|m| {
                matches!(
                    m,
                    Marker::Suspected(_) | Marker::Landmark(_) | Marker::QuestObject(_)
                )
            })
            .collect();
        assert_eq!(stars, vec![Marker::QuestObject("gidbinn altar")]);
        let mut snap = sanctuary([None, Some(602), None, None]);
        snap.level.id = 77;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![], "another level");
    }

    #[test]
    fn sewer_stairs_rule_marks_piece_739_in_sewers_1() {
        let stairs = Marker::Landmark("sewer stairs");
        let mut snap = sanctuary([None, Some(739), None, None]);
        snap.level.id = 92;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(stairs, 90.0, 30.0)]);
        snap.level.id = 93;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![], "another level");
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
            assert_ne!(colour, gold, "{other:?} shares the star's colour");
            assert!(other_size < size, "{other:?} is as big as the star");
        }
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "test points are inside the canvas"
        )]
        let nearest_pixel =
            |dx: f32, dy: f32| ((at.0 + dx - 0.5).round() as u32, (at.1 + dy - 0.5).round() as u32);

        let (cx, cy) = nearest_pixel(0.0, 0.0);
        assert_eq!(rgba(&pm, cx, cy)?, gold, "star centre");
        let (tx, ty) = nearest_pixel(0.0, 3.0 - size);
        assert_eq!(rgba(&pm, tx, ty)?, gold, "top tip");
        let (ox, oy) = nearest_pixel(0.0, -size - 1.0);
        let outline = rgba(&pm, ox, oy)?;
        assert!(
            outline[0] < 100 && outline[3] > 150,
            "outline above the tip: {outline:?}"
        );
        let (gx, gy) = nearest_pixel(0.0, size + 3.0);
        assert_ne!(rgba(&pm, gx, gy)?, gold, "gap below the star");
        for (dx, dy) in [
            (LANDMARK_RING, 0.0),
            (-LANDMARK_RING, 0.0),
            (0.0, LANDMARK_RING),
            (0.0, -LANDMARK_RING),
        ] {
            let (hx, hy) = nearest_pixel(dx, dy);
            let p = rgba(&pm, hx, hy)?;
            assert!(p[0] > 200 && p[1] > 140 && p[2] < 120, "halo at {dx},{dy}: {p:?}");
        }
        let (hits, dx, dy) = colour_spread(&pm, gold, at)?;
        assert!(hits >= 60, "only {hits} gold pixels");
        assert!(
            dx.hypot(dy) <= f64::from(LANDMARK_RING) * 1.5,
            "gold spread {dx},{dy}"
        );
        Ok(())
    }

    #[test]
    fn gidbinn_candidates_in_a_live_jungle_skip_the_waypoint_clearing() {
        // A live layout: 595 holds the waypoint, 602 lies north of it, 600 further north-west.
        let clearing = |preset, x0: u32, y0: u32| {
            (0..16u32).map(move |i| Room {
                x: x0 + 40 * (i % 4),
                y: y0 + 40 * (i / 4),
                w: 40,
                h: 40,
                loaded: false,
                seen_loaded: false,
                preset: Some(preset),
            })
        };
        let mut snap = empty_snapshot(5907, 3489);
        snap.level.id = 78;
        snap.level.rooms = clearing(595, 5800, 3880)
            .chain(clearing(602, 5800, 3720))
            .chain(clearing(600, 5640, 3400))
            .collect();
        snap.presets = vec![object(237, 5899, 3974)];
        let gidbinn = Marker::Landmark("gidbinn");
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(gidbinn, 5880.0, 3800.0), (gidbinn, 5720.0, 3480.0)]
        );
    }

    #[test]
    fn gidbinn_candidate_seen_whole_without_the_altar_is_ruled_out() {
        /// The rooms of `snap` at `x`.
        fn at(snap: &mut Snapshot, x: u32) -> impl Iterator<Item = &mut Room> {
            snap.level.rooms.iter_mut().filter(move |r| r.x == x)
        }
        let gidbinn = Marker::Landmark("gidbinn");
        let mut snap = sanctuary([Some(602), Some(602), None, Some(600)]);
        snap.level.id = 78;
        snap.level.rooms.push(Room {
            preset: Some(600),
            ..room(240, 0, false)
        });
        let both = vec![(gidbinn, 60.0, 30.0), (gidbinn, 240.0, 30.0)];
        assert_eq!(landmarks(&snap, LANDMARKS), both);
        at(&mut snap, 0).for_each(|r| r.loaded = true);
        assert_eq!(landmarks(&snap, LANDMARKS), both, "half of 602 loaded");
        at(&mut snap, 60).for_each(|r| r.seen_loaded = true);
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(gidbinn, 240.0, 30.0)],
            "all of 602 seen loaded"
        );
        snap.presets = vec![object(252, 250, 20)];
        assert_eq!(landmarks(&snap, LANDMARKS), vec![], "the altar is loaded");
    }

    #[test]
    fn every_landmark_rule_is_confirmed() {
        assert!(LANDMARKS.iter().all(|r| r.confirmed));
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
        let monster = Preset {
            kind: PresetKind::Monster,
            ..object(152, 0, 0)
        };
        assert_eq!(
            preset_marker(&monster),
            Some(Marker::Special(None)),
            "a quest-object id on a monster preset"
        );
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
        let siege_barricades = [432, 433, 435];
        for barricade in siege_barricades {
            let p = Preset {
                kind: PresetKind::Monster,
                ..object(barricade, 0, 0)
            };
            assert_eq!(preset_marker(&p), Some(Marker::Special(None)), "{barricade}");
        }
    }

    /// Tomb 66 with an orifice room (461) at world 90,30 and the orifice object at 150,30.
    fn tomb_66() -> Snapshot {
        let mut snap = sanctuary([None, Some(461), None, None]);
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
        let unconfirmed = [LandmarkRule {
            level_ids: 66..=72,
            preset_ids: 461..=461,
            label: "orifice (E)",
            confirmed: false,
            style: LandmarkStyle::Orifice,
            candidates_for: None,
        }];
        let plain = render_rules(&snap, &Guidance::default(), w, h, None, &unconfirmed)?;
        let v = View::fit(level_bounds(&snap), w, h);
        let at = v.to_screen(90.0, 30.0);
        assert_eq!(count_near(&plain, at, SUSPECT_DISC_RADIUS, is_suspect_text)?, 0);
        let (_, radius, _) = Marker::Suspected("").style();
        let ring = count_near(&plain, at, radius + 1.5, |p| p == LANDMARK_GOLD)?;
        let centre = count_near(&plain, at, radius - 2.5, |p| p == LANDMARK_GOLD)?;
        assert!(
            ring >= 15 && centre == 0,
            "no font: a hollow ring {ring}, centre {centre}"
        );

        let Some(font) = info::font() else {
            eprintln!("skipped the glyph check: no font on this machine");
            return Ok(());
        };
        let pm = render_rules(&snap, &Guidance::default(), w, h, Some(font), &unconfirmed)?;
        let v = View::fit(level_bounds(&snap), w, map_height(h, Some(font)));
        let at = v.to_screen(90.0, 30.0);
        let near = count_near(&pm, at, SUSPECT_DISC_RADIUS, is_suspect_text)?;
        assert!(near >= 15, "only {near} light glyph pixels near the ?");
        let star_gold = count_near(&pm, at, LANDMARK_RING + 2.0, |p| p == LANDMARK_GOLD)?;
        assert_eq!(star_gold, 0, "gold star pixels at the ?");
        let dark = count_near(&pm, at, SUSPECT_DISC_RADIUS, |p| p[3] >= 150 && p[0] < 50)?;
        assert!(dark >= 30, "only {dark} dark disc/outline pixels");
        Ok(())
    }

    #[test]
    fn quest_object_renders_as_a_small_gold_star() -> Result<()> {
        let mut snap = tomb_66();
        for r in &mut snap.level.rooms {
            r.preset = None;
        }
        let (w, h) = (400, 400);
        let pm = render_with(&snap, &Guidance::default(), w, h, None)?;
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
        let mut snap = sanctuary([None, Some(462), None, None]);
        snap.level.id = 72;
        snap.presets = vec![object(152, 90, 30)];
        assert_eq!(
            marker_kinds(&snap),
            vec![Marker::Orifice, Marker::QuestObject("orifice"), Marker::Player],
            "drawn above the landmark"
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
        assert_eq!(
            marker_kinds(&snap),
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
    fn marker_tags_skip_stars_and_monsters() {
        for &(_, label) in QUEST_OBJECTS.iter().chain(QUEST_MONSTERS) {
            assert_eq!(marker_tag(Marker::QuestObject(label)), None, "{label}");
        }
        for rule in LANDMARKS {
            assert_eq!(marker_tag(Marker::Landmark(rule.label)), None, "{}", rule.label);
        }
        for m in [
            Marker::Exit,
            Marker::Entry,
            Marker::Special(None),
            Marker::Monster,
            Marker::Elite,
            Marker::Superunique,
            Marker::Boss,
            Marker::Merc,
            Marker::Player,
            Marker::Worm,
            Marker::Drop(DropKind::Unique),
        ] {
            assert_eq!(marker_tag(m), None, "{m:?}");
        }
    }

    #[test]
    fn marker_tags_rank_waypoint_then_orifice_then_shrines_then_wells() {
        assert_eq!(marker_tag(Marker::Waypoint), Some((1, "wp")));
        assert_eq!(marker_tag(Marker::Orifice), Some((2, "orif")));
        assert_eq!(marker_tag(Marker::Shrine(15)), Some((3, "EXP")));
        assert_eq!(marker_tag(Marker::Well), Some((4, "well")));
        assert_eq!(marker_tag(Marker::Shrine(99)), None, "unknown code");
    }

    /// Glyph ink of `c`: mostly opaque and, premultiplied, close to `c`.
    fn tinted(p: Rgba, c: Rgba) -> bool {
        p[3] >= 200
            && p.iter()
                .zip(c)
                .take(3)
                .all(|(&got, want)| (u32::from(want) * u32::from(p[3]) / 255).abs_diff(u32::from(got)) <= 30)
    }

    #[test]
    fn shrine_and_well_tags_are_drawn_above_their_markers() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let mut snap = empty_snapshot(120, 120);
        snap.level.rooms = vec![room(100, 100, true)];
        snap.shrines = vec![shrine_at(12, 110, 110)];
        let (w, h) = (400, 400);
        let colour = marker_label_colour(Marker::Shrine(12).style().0);
        let at = |snap: &Snapshot| {
            let view = View::fit(level_bounds(snap), w, map_height(h, Some(font)));
            view.to_screen(110.0, 110.0)
        };
        let (cx, cy) = at(&snap);
        let pm = render_with(&snap, &Guidance::default(), w, h, Some(font))?;
        let ink = count_near(&pm, (cx, cy - 12.0), 14.0, |p| tinted(p, colour))?;
        assert!(ink >= 10, "SKL tag: {ink} px");
        snap.wells = vec![well_at(110, 110)];
        let pm = render_with(&snap, &Guidance::default(), w, h, Some(font))?;
        let again = count_near(&pm, (cx, cy - 12.0), 14.0, |p| tinted(p, colour))?;
        assert_eq!(again, ink, "the shrine outranks a well on the same spot");
        let well = marker_label_colour(WELL_CYAN);
        let well_ink = count_near(&pm, (cx, cy), 40.0, |p| tinted(p, well))?;
        assert!(well_ink >= 5, "well tag moved, not dropped: {well_ink} px");
        Ok(())
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

    #[test]
    fn the_trail_draws_under_the_markers_and_breaks_on_jumps() -> Result<()> {
        let mut snap = empty_snapshot(10, 10);
        snap.level.id = crate::cows::COW_LEVEL;
        snap.level.rooms = vec![room(0, 0, true), room(60, 0, true)];
        let (w, h) = (300, 200);
        let view = View::fit(level_bounds(&snap), w, map_height(h, None));
        let at = |pm: &Pixmap, (x, y): (f64, f64)| -> Result<tiny_skia::ColorU8> {
            let (sx, sy) = view.to_screen(x, y);
            #[allow(
                clippy::cast_sign_loss,
                clippy::cast_possible_truncation,
                reason = "test points are inside the canvas"
            )]
            let px = |v: f32| v.round() as u32;
            Ok(pm.pixel(px(sx), px(sy)).context("pixel")?.demultiply())
        };
        let plain = render_with(&snap, &Guidance::default(), w, h, None)?;
        let teleport_from_40_to_90 = vec![(20, 40), (30, 40), (40, 40), (90, 40), (100, 40)];
        snap.trail = teleport_from_40_to_90;
        let trailed = render_with(&snap, &Guidance::default(), w, h, None)?;
        let (before, after) = (at(&plain, (25.0, 40.0))?, at(&trailed, (25.0, 40.0))?);
        assert_ne!(before, after, "the trail is drawn over the room fill");
        assert!(
            after.red() > before.red(),
            "pinker than the fill: {after:?} vs {before:?}"
        );
        assert_eq!(
            at(&plain, (65.0, 40.0))?,
            at(&trailed, (65.0, 40.0))?,
            "no line across the jump"
        );
        assert_ne!(
            at(&plain, (95.0, 40.0))?,
            at(&trailed, (95.0, 40.0))?,
            "drawn after it"
        );
        Ok(())
    }
}
