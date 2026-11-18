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
use crate::items::{CharmSize, DropKind, GemColour};
use crate::offsets::monster_data;
use crate::valuables::Tier;
use crate::{info, session, xp};

/// Exit labels in an area that gives experience ([`draw_labels`]).
const LABEL_TEXT: [u8; 4] = [255, 255, 255, 255];

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
    /// How a confirmed rule is drawn.
    pub style: LandmarkStyle,
}

/// How a confirmed [`LandmarkRule`] is drawn (an unconfirmed one is always a "?").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandmarkStyle {
    /// A gold landmark star ([`Marker::Landmark`]).
    Star,
    /// Stairs down to a deeper level ([`Marker::StairsDown`]).
    StairsDown,
    /// A gold worm ([`Marker::Worm`]): Coldworm's chamber, the most hated spot in act 2 (user's
    /// choice, 2026-11-15).
    Worm,
    /// A mini orifice ([`Marker::Orifice`]): the real Tal Rasha tomb's staff hole, apart from the
    /// fake tombs' treasure stars (user's choice, 2026-11-16).
    Orifice,
}

/// Landmark rules.
/// - Arcane Sanctuary (74): the arm end holding the Summoner's platform has its own preset per
///   direction: 525 (+x), 526 (−x), 527 (−y), 528 (+y), all four confirmed in game (2026-11-15).
///   Ordinary arm ends use other ids (510, 511, 513, 517 seen).
/// - Tal Rasha's tombs (66–72): the real tomb's orifice chamber is lvlprest "Tomb Talrasha W/E/S/N"
///   (460–463), only ever placed in the real tomb. Confirmed: 462 in tomb 72 (orifice object 152
///   inside it) and 460 in tomb 69 (user stood on the orifice, Nightmare 2026-11-16); 461 and 463
///   confirmed by the family name (2026-11-17). A fake tomb's chest chamber is "Tomb Chest" (472–475;
///   473 seen in tombs 71 and 68, 472 in tomb 67). All four drawn as a gold star at the user's
///   request (2026-11-16): nothing else in the tombs is a landmark. The 462 chamber spans 4 Room2 cells. (An earlier level+390 guess was
///   wrong: 456–459 are the Halls of the Dead cube pieces.)
/// - Halls of the Dead Level 3 (60): lvlprest "Tomb Cube W/E/S/N" (456–459) holds the Horadric Cube
///   chest (object 354) and Bloodwitch's spawn. Confirmed: 459 (chest inside it, 2026-11-15).
/// - Flayer Jungle (78): the Gidbinn altar (object 252) lies in piece 602, live-verified 2026-11-15
///   (player in a 602 room 5 tiles from the altar). 602 spans 16 Room2 cells; 601 is the waypoint
///   area (waypoint object 237).
/// - Kurast Sewers Level 1 (92): the sewer lever (object 367) and the stairs down (object 366) both
///   sat in room preset 739 (live 2026-11-15, one game; the user then went down to level 2).
/// - Act 1 caves, level 1 (Cave 9, Underground Passage 10, Hole 11, Pit 12): lvlprest names the
///   pieces "Cave Next W/E/S/N" (87–90, exit to the next outdoor area) and "Cave Down W/E/S/N"
///   (91–94, stairs to level 2); Underground Passage 1 had one 90 and one 91 piece (2026-11-15).
///   Confirmed: the 90 piece holds the exit to Dark Wood (room link to level 5, user stood on it),
///   drawn as a gold star because it is where the user wants to go (2026-11-15); the 91 piece holds
///   the stairs to Underground Passage 2 (room link to level 14), drawn as a staircase.
/// - Stony Field (4): lvlprest 160 "Act 1 - Cairn Stones", the stone circle; the five stones and the
///   portal stone (objects 17–22) lay inside it (live 2026-11-15).
/// - Barracks (28): lvlprest "Barracks Forge W/E/S/N" (202–205) holds the Smith and the Horadric
///   Malus; confirmed for 204 and 203 (Smith preset 771 and malus object 108 inside it, two games
///   2026-11-15).
/// - Lut Gholein sewers (47–49): lvlprest "Sewer Radament's Lair W/E/S/N" (341–344) in level 3 (49;
///   341, 343 and 344 confirmed in game 2026-11-15, Radament (txt 229) standing in 344); stairs pieces 338 (level 1, link to 48) and 337 (level 2, link to 49) confirmed and "Sewer Next W/E/S/N" (337–340, stairs down) in levels 1–2.
/// - Maggot Lair (62–64): lvlprest "Lair Next W/E/S/N" (501–504) is the stairs down in levels 1–2
///   (drawn as stairs at once: the "Next" = stairs-down naming is confirmed for act 1 caves and the
///   sewers); "Lair Tight Spot S" (509, MagQueen.ds1) in level 3 is Coldworm's chamber with the staff
///   chest (confirmed in game 2026-11-15).
/// - Dark Wood (5): lvlprest 161 "Act 1 - Inifus" (funktree.ds1), the Tree of Inifuss (confirmed in
///   game 2026-11-15).
/// - Black Marsh (6): lvlprest 163 "Act 1 - Tower 1" (Tower1.ds1), the Forgotten Tower entrance
///   (confirmed in game 2026-11-15).
pub const LANDMARKS: &[LandmarkRule] = &[
    LandmarkRule {
        level_ids: 62..=63,
        preset_ids: 501..=504,
        label: "maggot lair stairs down",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
    },
    LandmarkRule {
        level_ids: 64..=64,
        preset_ids: 509..=509,
        label: "coldworm",
        confirmed: true,
        style: LandmarkStyle::Worm,
    },
    LandmarkRule {
        level_ids: 49..=49,
        preset_ids: 341..=341,
        label: "radament's lair",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 49..=49,
        preset_ids: 344..=344,
        label: "radament's lair (N)",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 49..=49,
        preset_ids: 343..=343,
        label: "radament's lair (S)",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 49..=49,
        preset_ids: 342..=342,
        label: "radament's lair (E)",
        // W/E/S/N name family, 341/343/344 live-confirmed: the direction letter only says where
        // in the piece the feature sits (2026-11-17).
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 47..=48,
        preset_ids: 338..=338,
        label: "sewer stairs down (east)",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
    },
    LandmarkRule {
        level_ids: 47..=48,
        preset_ids: 337..=337,
        label: "sewer stairs down (west)",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
    },
    LandmarkRule {
        level_ids: 47..=48,
        preset_ids: 339..=340,
        label: "sewer stairs down (S/N)",
        // "Sewer Next W/E/S/N" family, 337/338 live-confirmed (2026-11-17).
        confirmed: true,
        style: LandmarkStyle::StairsDown,
    },
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 203..=204,
        label: "smith (forge)",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 202..=202,
        label: "smith (forge W)",
        // "Barracks Forge W/E/S/N" family, 203/204 live-confirmed (2026-11-17).
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 28..=28,
        preset_ids: 205..=205,
        label: "smith (forge N)",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 4..=4,
        preset_ids: 160..=160,
        label: "cairn stones",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 90..=90,
        label: "cave exit to the next area (north)",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 87..=89,
        label: "cave exit to the next area",
        // "Cave Next W/E/S/N" family, 90 live-confirmed (2026-11-17).
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 91..=91,
        label: "cave stairs down (west)",
        confirmed: true,
        style: LandmarkStyle::StairsDown,
    },
    LandmarkRule {
        level_ids: 9..=12,
        preset_ids: 92..=94,
        label: "cave stairs down",
        // "Cave Down W/E/S/N" family, 91 live-confirmed. Seen again live 2026-11-17: the
        // Underground Passage 1 stairs to level 2 were a 94 piece carrying the room link to
        // level 14.
        confirmed: true,
        style: LandmarkStyle::StairsDown,
    },
    LandmarkRule {
        level_ids: 5..=5,
        preset_ids: 161..=161,
        label: "tree of inifuss",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 6..=6,
        preset_ids: 163..=163,
        label: "forgotten tower",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 74..=74,
        preset_ids: 525..=528,
        label: "summoner",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 462..=462,
        label: "orifice",
        confirmed: true,
        style: LandmarkStyle::Orifice,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 472..=475,
        label: "tomb treasure",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 60..=60,
        preset_ids: 459..=459,
        label: "horadric cube",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 60..=60,
        preset_ids: 456..=458,
        label: "horadric cube (W/E/S)",
        // "Tomb Cube W/E/S/N" family, 459 live-confirmed with the chest inside it (2026-11-17).
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 460..=460,
        label: "orifice (W)",
        confirmed: true,
        style: LandmarkStyle::Orifice,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 461..=461,
        label: "orifice (E)",
        // "Tomb Talrasha W/E/S/N" family, 460/462 live-confirmed (2026-11-17).
        confirmed: true,
        style: LandmarkStyle::Orifice,
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 463..=463,
        label: "orifice (N)",
        confirmed: true,
        style: LandmarkStyle::Orifice,
    },
    LandmarkRule {
        level_ids: 78..=78,
        preset_ids: 602..=602,
        label: "gidbinn",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
    LandmarkRule {
        level_ids: 92..=92,
        preset_ids: 739..=739,
        label: "sewer stairs",
        confirmed: true,
        style: LandmarkStyle::Star,
    },
];

/// Quest objects (object txt ids from objects.txt, which matched live) drawn where their preset
/// is, once the room holding it is loaded. The id alone identifies them, so they are confirmed.
pub const QUEST_OBJECTS: &[(u32, &str)] = &[
    // Barracks (28), live 2026-11-15: the Horadric Malus stand, inside the Smith's forge piece 204.
    (108, "horadric malus"),
    // Halls of the Dead Level 3 (60), live 2026-11-15: the Horadric Cube chest, next to
    // Bloodwitch's spawn preset 762.
    (354, "horadric cube chest"),
    // Maggot Lair Level 3 (64), live 2026-11-15: the Staff of Kings chest (OperateFn 41) in
    // Coldworm's chamber 509, next to his spawn preset 766.
    (356, "staff of kings chest"),
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

/// Short map labels of the non-monster quest objects ([`QUEST_OBJECTS`]) and landmarks
/// ([`LANDMARKS`]), keyed by their label: at most 5 letters. A quest object and the landmark star
/// on the same spot share a tag, so [`draw_labels`] shows it once. Monster spots (superuniques,
/// quest monsters, the summoner, Radament, the Smith, Coldworm) and stairs or cave exits (the exit
/// label names them) have none. Shrines: [`crate::config::SHRINE_TAGS`].
pub const MARKER_TAGS: &[(&str, &str)] = &[
    ("horadric malus", "malus"),
    ("horadric cube chest", "cube"),
    ("horadric cube", "cube"),
    ("horadric cube (W/E/S)", "cube"),
    ("staff of kings chest", "staff"),
    ("orifice", "orif"),
    ("gidbinn altar", "altar"),
    ("gidbinn", "altar"),
    ("sewer stairs", "stair"),
    ("sewer lever", "lever"),
    ("hellforge", "forge"),
    ("diablo seal", "seal"),
    ("anya", "anya"),
    ("cairn stones", "cairn"),
    ("tree of inifuss", "tree"),
    ("forgotten tower", "tower"),
    ("tomb treasure", "chest"),
];
/// Waypoint and well labels.
const WAYPOINT_TAG: &str = "wp";
const WELL_TAG: &str = "well";

/// The map label of a quest object or landmark label, if it has one ([`MARKER_TAGS`]).
fn marker_tag_of(label: &str) -> Option<&'static str> {
    MARKER_TAGS
        .iter()
        .find(|&&(l, _)| l == label)
        .map(|&(_, tag)| tag)
}

/// The map label of `marker` and its rank (lower wins a crowded spot; exit labels are rank 0):
/// waypoint 1, quest object or landmark 2, shrine 3, well 4. `None` for monsters, drops, exits
/// and the player.
fn marker_tag(marker: Marker) -> Option<(u8, &'static str)> {
    match marker {
        Marker::Waypoint => Some((1, WAYPOINT_TAG)),
        Marker::QuestObject(label) | Marker::Landmark(label) => marker_tag_of(label).map(|t| (2, t)),
        // The Orifice landmark shares the "orifice" quest object's tag.
        Marker::Orifice => marker_tag_of("orifice").map(|t| (2, t)),
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
/// The Moo Moo Farm trail ([`Snapshot::trail`]): a faint pink, apart from the grey room fill.
const TRAIL: Rgba = [255, 200, 225, 90];
const TRAIL_WIDTH: f32 = 3.0;
/// Consecutive trail points farther apart than this (tiles) were a teleport, waypoint or
/// portal: the line breaks there instead of drawing a long jump.
const TRAIL_BREAK: f64 = 30.0;
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
/// Live elites: the dot and the superunique and boss skulls.
const ELITE_RED: Rgba = [230, 30, 30, 255];
/// Skull icons: the dark outline is stroked under the fill, so about 1 px of it shows around the
/// skull and it darkens the small eye holes.
const SKULL_OUTLINE_WIDTH: f32 = 2.0;
/// Most distance in tiles between a live superunique or boss and a superunique or quest monster
/// preset whose marker it replaces for the frame: the live icon is drawn, the preset dot not.
/// A bit above `session`'s bind radius (15), as the monster moves around its spawn while alive;
/// once it is dead the session hides the preset for the game.
const LIVE_REPLACES_PRESET_RADIUS: f64 = 20.0;
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
    /// Death skull: cranium, jaw, two eye holes ([`skull_path`]); the size is a quarter of its
    /// height and width, so 3.5 gives the 8 px footprint of an elite dot.
    Skull,
    /// [`Shape::Skull`] with two horns, the demon skull of a boss.
    HornedSkull,
    /// A wavy worm body (an open centre line, stroked; see [`worm_path`]) with a round head on the
    /// right; the size is half its length.
    Worm,
    /// Three steps descending to the right ([`stairs_path`]); the size is half the side.
    Stairs,
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
    /// A confirmed [`LandmarkStyle::Worm`] landmark: a wavy gold worm with a head, landmark size.
    Worm,
    /// A confirmed [`LandmarkStyle::Orifice`] landmark: a dark hole in a gold ring with the staff
    /// standing in it.
    Orifice,
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
    /// Live champion or unique monster.
    Elite,
    /// Live superunique (`MonsterData` flag 0x02): a red death skull.
    Superunique,
    /// Live quest boss ([`BOSS_MONSTERS`](crate::levels_data::BOSS_MONSTERS) by txt id): a red
    /// horned skull.
    Boss,
    /// The local player's mercenary (`MonsterData` owner id, unverified hypothesis): a pale
    /// green dot, drawn below the player.
    Merc,
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
            // Size 7: about 11 px long with the outline; at 4 it was barely visible (user, 2026-11-15).
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
            Self::Ring { kind, tier, .. } => {
                let radius = Self::Drop(kind).style().1 + RING_GAP;
                (tier_colour(tier), radius, Shape::Circle)
            }
            Self::Merc => ([150, 255, 190, 255], 2.5, Shape::Circle),
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
/// `NON_HOSTILE_MONSTERS`), not just elites (user requests, 2026-11-15): Den of Evil (8), whose
/// quest needs every monster killed, and the Moo Moo Farm (39), to see where the cow packs are.
pub const ALL_MONSTER_LEVELS: &[u32] = &[8, 39];

/// Live superunique/champion/unique, not dead. Minion-only (0x10) and normal (0) monsters are skipped.
///
/// Allies never count: assassin traps (e.g. Wake of Fire, txt 410) carry the unique flag 0x08
/// (live 2026-11-15), so every `NON_HOSTILE_MONSTERS` id is skipped.
pub fn is_elite(m: &Monster) -> bool {
    m.flags & ELITE_FLAGS != 0 && !m.is_dead() && !crate::levels_data::NON_HOSTILE_MONSTERS.contains(&m.txt)
}

/// The icon marker of a live monster: [`Marker::Boss`] for a quest boss by txt id (whatever its
/// flags: the boss check comes first), else [`Marker::Superunique`] or [`Marker::Elite`] for an
/// elite (see [`is_elite`]). `None` for a dead or ordinary monster.
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

/// True if a preset drawn as `marker` at (`x`, `y`) is a superunique or quest monster spawn that
/// one of the live superunique or boss positions `live` stands within
/// [`LIVE_REPLACES_PRESET_RADIUS`] of: the live icon shows instead.
fn replaced_by_live(p: &Preset, marker: Marker, live: &[(f64, f64)]) -> bool {
    p.kind == PresetKind::Monster
        && matches!(marker, Marker::Special(_) | Marker::QuestObject(_))
        && live
            .iter()
            .any(|&(mx, my)| (f64::from(p.x) - mx).hypot(f64::from(p.y) - my) <= LIVE_REPLACES_PRESET_RADIUS)
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
            let marker = match (rule.confirmed, rule.style) {
                (false, _) => Marker::Suspected(rule.label),
                (true, LandmarkStyle::Star) => Marker::Landmark(rule.label),
                (true, LandmarkStyle::StairsDown) => Marker::StairsDown,
                (true, LandmarkStyle::Worm) => Marker::Worm,
                (true, LandmarkStyle::Orifice) => Marker::Orifice,
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
            Some((id, Marker::BorderExit { dir, entry }, x, y))
        })
        .collect()
}

/// One label per level this level links to: the target level id and where its name goes (world
/// tiles). A room-tile link's label sits on the tile exit preset in its room (the room's centre
/// if the preset isn't loaded); a walk-through border's on its [`border_exits`] triangle. Links
/// from rooms the level doesn't list are left out.
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
        let inside = |p: &&Preset| {
            p.kind == PresetKind::Tile
                && (room.x..room.x.saturating_add(room.w)).contains(&p.x)
                && (room.y..room.y.saturating_add(room.h)).contains(&p.y)
        };
        let (x, y) = snap.presets.iter().find(inside).map_or_else(
            || {
                (
                    f64::from(room.x) + f64::from(room.w) / 2.0,
                    f64::from(room.y) + f64::from(room.h) / 2.0,
                )
            },
            |p| (f64::from(p.x), f64::from(p.y)),
        );
        out.push((l.to_level_id, x, y));
    }
    for (id, _, x, y) in border_exits_to(snap) {
        if !out.iter().any(|&(seen, _, _)| seen == id) {
            out.push((id, x, y));
        }
    }
    out
}

/// [`markers`] with custom landmark rules (for tests).
fn markers_with(snap: &Snapshot, rules: &[LandmarkRule]) -> Vec<(Marker, f64, f64)> {
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
            let marker = preset_marker(p).filter(|&m| !replaced_by_live(p, m, &live_bosses))?;
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
            let marker = if let Some(elite) = elite_marker(m) {
                elite
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

/// The canvas height the map is fitted into: all of it, less the info line's band if there is
/// a font to draw it with.
pub fn map_height(height: u32, font: Option<&FontVec>) -> u32 {
    font.map_or(height, |f| height.saturating_sub(info::band_height(f)).max(1))
}

/// The text blocks below the info line, kept separate: the breakpoint guide bands, the level
/// suggestions, the inventory keep list, the sell and give-away lists and the `BiS` chase list.
/// The webapp's live dashboard (`live` publishes from the same tick) consumes all of them; the
/// overlay draws none of them (only the info line; the guide moved to the dashboard, user
/// request 2026-11-17).
#[derive(Debug, Default)]
pub struct TextBlocks {
    pub guide: Vec<info::Band>,
    pub keep: Vec<String>,
    pub sell: Vec<String>,
    pub give: Vec<String>,
    /// The `BiS` chase lines with their structural tick ([`info::BisLine::tick`]): the
    /// dashboard marks ✓(stash) from the tick.
    pub bis: Vec<info::BisLine>,
    /// The whole level-suggestion band ([`info::level_lines_full`]) for the dashboard's
    /// "Where to go" panel. Each entry carries its monster level and below/equal/above bucket
    /// so the dashboard can lay the band out in three columns.
    pub level_full: Vec<info::LevelEntry>,
}

/// The dashboard's groups ([`TextBlocks`]) for a `width` px canvas from one snapshot. None of
/// it without a font.
pub fn text_blocks(snap: &Snapshot, width: u32, font: Option<&FontVec>) -> TextBlocks {
    let Some(f) = font else {
        return TextBlocks::default();
    };
    let max_width = f32::from(u16::try_from(width).unwrap_or(u16::MAX)) - 8.0;
    TextBlocks {
        guide:
            // Width-fitted like the other blocks (spans ride along: low resistances are red).
            crate::breakpoints::guide_band(&snap.player, snap.difficulty)
                .into_iter()
                .map(|band| info::fit_band(band, f, max_width))
                .collect(),
        level_full: info::level_lines_full(snap.player.char_level, snap.difficulty),
        keep: info::keep_lines(f, &snap.inventory_keep, max_width),
        sell: info::spare_lines(f, info::SELL_PREFIX, &snap.sell, max_width),
        give: info::spare_lines(f, info::GIVE_PREFIX, &snap.give_away, max_width),
        // The BiS chase list (user request, 2026-11-17): the tick is computed from the owned
        // and shared-stash names separately, so the dashboard can mark ✓(stash) distinctly.
        bis: snap.player.bis.as_ref().map_or_else(Vec::new, |bis| {
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
pub fn render_with(snap: &Snapshot, width: u32, height: u32, font: Option<&FontVec>) -> Result<Pixmap> {
    render_rules(snap, width, height, font, LANDMARKS)
}

/// [`render_with`] with custom landmark rules (for tests).
fn render_rules(
    snap: &Snapshot,
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
    let mut tagged: Vec<(Marker, f32, f32)> = Vec::new();
    for (marker, x, y) in markers_with(snap, rules) {
        let (colour, size, shape) = marker.style();
        let (cx, cy) = view.to_screen(x, y);
        if marker_tag(marker).is_some() {
            tagged.push((marker, cx, cy));
        }
        let Some(dot) = marker_outline(marker, shape, cx, cy, size) else {
            continue;
        };
        if let Marker::Suspected(_) = marker {
            draw_suspected(&mut pm, font, &dot, cx, cy);
        } else if shape == Shape::Worm || marker == Marker::Orifice {
            draw_figure(&mut pm, marker, &dot, colour, (cx, cy, size));
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
        } else if matches!(shape, Shape::Skull | Shape::HornedSkull) {
            draw_skull(&mut pm, &dot, colour, (cx, cy, size), shape == Shape::HornedSkull);
        } else if matches!(shape, Shape::Triangle | Shape::Stairs) {
            // Outline under the fill: the small triangle keeps all its colour, with half the
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
        draw_labels(&mut pm, view, font, snap, &tagged);
        draw_info_line(&mut pm, font, snap);
    }
    Ok(pm)
}

/// Px within which a second marker label with the same text is left out: a quest object and
/// the landmark star of its room, a few tiles apart, get one label.
const SAME_TAG_PX: f32 = 40.0;

/// The map labels: the target area names of the level's exits ([`exit_labels`]), grey where the
/// character gets almost no experience ([`xp::is_low`]), white otherwise; then the short tags of
/// `tagged` markers ([`marker_tag`]) in [`marker_label_colour`]. Where labels would overlap,
/// [`info::place_labels`] moves or drops the lower-ranked one; exit labels are always drawn.
fn draw_labels(pm: &mut Pixmap, view: View, font: &FontVec, snap: &Snapshot, tagged: &[(Marker, f32, f32)]) {
    let mut labels: Vec<(String, Rgba, (f32, f32), bool)> = exit_labels(snap)
        .into_iter()
        .map(|(id, x, y)| {
            let name = info::level_name(id).map_or_else(|| format!("Level {id}"), str::to_owned);
            let colour = if xp::is_low(id, snap.difficulty, snap.player.char_level) {
                info::LOW_XP_TEXT
            } else {
                LABEL_TEXT
            };
            (name, colour, view.to_screen(x, y), true)
        })
        .collect();
    let mut tags: Vec<(u8, &str, Rgba, (f32, f32))> = tagged
        .iter()
        .filter_map(|&(m, cx, cy)| {
            let (rank, tag) = marker_tag(m)?;
            Some((rank, tag, marker_label_colour(m.style().0), (cx, cy)))
        })
        .collect();
    // Stable: snapshot order within one rank.
    tags.sort_by_key(|t| t.0);
    for (_, tag, colour, at) in tags {
        let near = |&(ref text, _, (x, y), _): &(String, Rgba, (f32, f32), bool)| {
            text == tag && (x - at.0).hypot(y - at.1) < SAME_TAG_PX
        };
        if !labels.iter().any(near) {
            labels.push((tag.to_owned(), colour, at, false));
        }
    }
    let specs: Vec<info::LabelSpec> = labels
        .iter()
        .map(|(text, _, at, keep)| info::LabelSpec {
            size: info::label_size(font, text),
            at: *at,
            keep: *keep,
        })
        .collect();
    let bottom = map_height(pm.height(), Some(font));
    #[allow(clippy::cast_precision_loss, reason = "canvas widths are far below 2^24")]
    let spots = info::place_labels(&specs, pm.width() as f32, bottom as f32);
    for ((text, colour, _, _), spot) in labels.iter().zip(spots) {
        if let Some(spot) = spot {
            info::draw_label(pm, font, *colour, text, spot);
        }
    }
}

/// A marker label's colour: the marker's fill blended 35 % toward white, so dark fills (the
/// monster shrine's dark red) still read on the dark outline and every tag stays apart from the
/// white exit names.
fn marker_label_colour([r, g, b, _]: Rgba) -> Rgba {
    let lift = |c: u8| {
        let c = u16::from(c);
        // At most 255: (c * 65 + 255 * 35) / 100 with c <= 255.
        u8::try_from((c * 65 + 255 * 35) / 100).unwrap_or(u8::MAX)
    };
    [lift(r), lift(g), lift(b), 255]
}

/// The info line at the bottom of the frame. A terrorized level's monster levels follow the
/// character, so its name is never greyed.
fn draw_info_line(pm: &mut Pixmap, font: &FontVec, snap: &Snapshot) {
    let mut line = info::info_line(
        snap.level.id,
        snap.difficulty,
        snap.terrorized,
        snap.cows,
        &snap.player.build,
    );
    line.low_xp = !snap.terrorized && xp::is_low(snap.level.id, snap.difficulty, snap.player.char_level);
    info::draw(pm, font, &line);
}

/// Strokes `trail` (world tiles) as one translucent polyline with round joins, broken where two
/// points lie more than [`TRAIL_BREAK`] apart. One path, so overlapping segments don't stack
/// their alpha.
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
    // `None` for an empty trail or a lone point: nothing to draw.
    let Some(path) = pb.finish() else { return };
    let stroke = Stroke {
        width: TRAIL_WIDTH,
        line_join: LineJoin::Round,
        line_cap: LineCap::Round,
        ..Stroke::default()
    };
    pm.stroke_path(&path, &paint(TRAIL), &stroke, Transform::identity(), None);
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

/// A superunique or boss skull: `dot` is its [`skull_path`] with the eye holes, centred on
/// (`cx`, `cy`) with `size`.
fn draw_skull(pm: &mut Pixmap, dot: &Path, colour: Rgba, (cx, cy, size): (f32, f32, f32), horns: bool) {
    // Outline of the outer contour under the fill, as for the stars. Stroking the eye holes too
    // would ring them and merge both eyes into one dark band at 8 px, so even-odd cuts them out
    // of the fill and they are filled dark on their own.
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
            // `iso` is linear, so the screen direction is the projected world direction.
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

/// Draws a figure landmark: the worm (`body` is its path) or the mini orifice.
fn draw_figure(pm: &mut Pixmap, marker: Marker, body: &Path, colour: Rgba, geom: (f32, f32, f32)) {
    if marker == Marker::Orifice {
        draw_orifice(pm, colour, geom);
    } else {
        draw_worm(pm, body, colour, geom);
    }
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
    // sin(3π) = 0: the body ends on the centre line.
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

/// A skull centred on (`cx`, `cy`), in units of `size` / 4 (so the outline spans about ±4 units):
/// one outer contour (cranium arc, with a horn on each upper side if `horns`, down into a narrower
/// jaw) and, if `eyes`, the two eye circles of [`skull_eyes`] inside it. Filled even-odd, the eyes
/// are holes. The outer contour is one polygon on purpose: separate overlapping cranium and jaw
/// shapes would cancel out under even-odd.
fn skull_path(cx: f32, cy: f32, size: f32, horns: bool, eyes: bool) -> Option<Path> {
    // Cranium circle (centre y, radius), jaw half-width and bottom.
    const CRANIUM_Y: f32 = -0.9;
    const CRANIUM_R: f32 = 3.0;
    const JAW_HALF: f32 = 1.5;
    const JAW_BOTTOM: f32 = 3.8;
    // Right horn: its base spans these cranium angles (degrees, screen y down), its tip is here;
    // the left horn mirrors it.
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
    // An arc of the cranium from angle `from` down to `to`: 20 segments, at most ~15 degrees
    // each for the longest (the whole skull top), plenty at 8 px.
    let arc = |pb: &mut PathBuilder, from: f32, to: f32| {
        const STEPS: u8 = 20;
        for i in 0..=STEPS {
            let t = f32::from(i) / f32::from(STEPS);
            line(pb, on_cranium((to - from).mul_add(t, from)));
        }
    };
    // Angle of the right jaw corner on the cranium; the left one is 180 - this.
    let join = (JAW_HALF / CRANIUM_R).acos().to_degrees();
    let (x, y) = at(JAW_HALF, JAW_BOTTOM);
    pb.move_to(x, y);
    if horns {
        let (tx, ty) = HORN_TIP;
        arc(&mut pb, join, HORN_BASE.0);
        line(&mut pb, at(tx, ty));
        arc(&mut pb, HORN_BASE.1, -180.0 - HORN_BASE.1);
        line(&mut pb, at(-tx, ty));
        arc(&mut pb, -180.0 - HORN_BASE.0, -180.0 - join);
    } else {
        arc(&mut pb, join, -180.0 - join);
    }
    line(&mut pb, at(-JAW_HALF, JAW_BOTTOM));
    pb.close();
    if eyes {
        push_eyes(&mut pb, cx, cy, size);
    }
    pb.finish()
}

/// The two eye circles of the skull of [`skull_path`], alone: filled dark so they read on any
/// background (a hole alone would show the room grey).
fn skull_eyes(cx: f32, cy: f32, size: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    push_eyes(&mut pb, cx, cy, size);
    pb.finish()
}

fn push_eyes(pb: &mut PathBuilder, cx: f32, cy: f32, size: f32) {
    // Offset from the centre and radius, in units of `size` / 4 as in `skull_path`.
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
                build: vec![],
                bis: None,
                wsm: None,
                strength: None,
                dexterity: None,
            },
            equipped: vec![],
            merc_equipped: vec![],
            equipped_items: vec![],
            owned_names: vec![],
            stash: vec![],
            stash_names: vec![],
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
            sell: vec![],
            give_away: vec![],
            held_materials: vec![],
            white_bases: vec![],
            difficulty: None,
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
        // Andariel (156), Baal (544), Baal's clone (570), Nihlathak (526): the boss check wins
        // over the superunique flag, and needs no elite flag at all.
        for txt in [156, 544, 570, 526] {
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
                "{txt}"
            );
        }
        // Superunique beats the plain elite, also with the unique and champion bits set.
        assert_eq!(elite_marker(&monster(0x02, 1)), Some(Marker::Superunique));
        assert_eq!(elite_marker(&monster(0x0A, 1)), Some(Marker::Superunique));
        assert_eq!(elite_marker(&monster(0x08, 1)), Some(Marker::Elite));
        assert_eq!(elite_marker(&monster(0x04, 1)), Some(Marker::Elite));
        // Not drawn as an elite: minion only, normal, dead (boss or not).
        assert_eq!(elite_marker(&monster(0x10, 1)), None);
        assert_eq!(elite_marker(&monster(0x00, 1)), None);
        assert_eq!(elite_marker(&monster(0x02, 0)), None);
        assert_eq!(elite_marker(&monster_txt(156, 0x00, 12)), None);
        // Same draw rank as the elite dot: above shrines, below the player.
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
            at(PresetKind::Monster, 757, 100, 100), // Countess preset, 19 tiles off: hidden
            at(PresetKind::Monster, 771, 200, 100), // another superunique preset: kept
            at(PresetKind::Monster, 434, 110, 110), // quest monster preset near him: hidden
            at(PresetKind::Object, 460, 101, 100),  // quest object, not a monster: kept
            at(PresetKind::Tile, 1, 100, 100),      // exit: kept
        ];
        let kinds = |s: &Snapshot| -> Vec<Marker> {
            markers_with(s, LANDMARKS)
                .into_iter()
                .map(|(m, _, _)| m)
                .collect()
        };
        let countess = |mode| Monster {
            x: 115,
            y: 112,
            ..monster(0x02, mode)
        };

        snap.monsters = vec![countess(1)];
        assert_eq!(
            kinds(&snap),
            vec![
                Marker::Exit,
                Marker::Special(Some("The Smith")),
                Marker::QuestObject("anya"),
                Marker::Superunique,
                Marker::Player
            ]
        );
        // A plain unique standing there hides nothing.
        snap.monsters = vec![Monster {
            x: 100,
            y: 100,
            ..monster(0x08, 1)
        }];
        assert_eq!(
            kinds(&snap)
                .iter()
                .filter(|m| matches!(m, Marker::Special(_)))
                .count(),
            2
        );
        // Dead: the preset is back for this frame (the session hides a killed one for the game).
        snap.monsters = vec![countess(12)];
        assert_eq!(
            kinds(&snap),
            vec![
                Marker::Exit,
                Marker::Special(Some("The Countess")),
                Marker::Special(Some("The Smith")),
                Marker::QuestObject("anya"),
                Marker::QuestObject("prison door"),
                Marker::Player
            ]
        );
        // A live boss hides a nearby preset the same way.
        snap.monsters = vec![Monster {
            x: 105,
            y: 100,
            ..monster_txt(526, 0x00, 1)
        }];
        assert!(!kinds(&snap).contains(&Marker::Special(Some("The Countess"))));
        assert!(kinds(&snap).contains(&Marker::Boss));
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
            assert!(
                markers_with(&snap, LANDMARKS).iter().any(|&(m, _, _)| m == want),
                "{want:?}"
            );
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
            // About 8x8 px less the outline, eyes and edge anti-aliasing: a dozen solid pixels.
            assert!(hits >= 10, "{want:?}: only {hits} red pixels");
            // A dark outline just outside the cranium top, and dark eye holes.
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
    fn fixture_markers() -> Result<()> {
        let snap = fixture()?;
        // Jail 1: one waypoint, one exit tile, no elites (only normals and dead minions), the player.
        assert_eq!(
            markers_with(&snap, LANDMARKS),
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
        let kinds = |s: &Snapshot| -> Vec<Marker> {
            markers_with(s, LANDMARKS)
                .into_iter()
                .map(|(m, _, _)| m)
                .collect()
        };
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
        let kinds: Vec<Marker> = markers_with(&snap, LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
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
        // The staircase draws in the exit purple.
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
        let kinds: Vec<Marker> = markers_with(&snap, LANDMARKS)
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
        let kinds: Vec<Marker> = markers_with(&snap, LANDMARKS)
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
        // Exit labels are white text on the map too (`exit_labels_are_grey_where_xp_is_low`).
        snap.room_links.clear();
        snap.border_links.clear();
        let (w, h) = (400, 400);
        let pm = render_with(&snap, w, h, Some(font))?;
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
        // Glyph cores of the violet part (190, 120, 255); anti-aliased edges blend into the
        // dark outline, so only fairly opaque, blue-heavy pixels with red over green count.
        let violet = |p: Rgba| p[3] >= 200 && p[2] >= 180 && p[0] >= 120 && p[0].saturating_sub(p[1]) >= 40;
        let (w, h) = (400, 400);
        // The info band only: map markers may be violet-ish too.
        let top = map_height(h, Some(font));
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
    fn text_blocks_holds_the_dashboard_blocks() -> Result<()> {
        use crate::valuables::{Category, Keep, Place, Spare, Tier};
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let mut snap = fixture()?;
        snap.difficulty = Some(0);
        snap.player.char_level = Some(34);
        snap.player.bis = Some(crate::config::Bis {
            build: String::new(),
            slots: BTreeMap::from([("helm".to_owned(), vec!["Peasant Crown@28".to_owned()])]),
            merc_slots: BTreeMap::new(),
        });
        snap.inventory_keep = vec![Keep {
            name: "Harlequin Crest".into(),
            tier: Tier::S,
            category: Category::Trade,
            count: 1,
        }];
        let spare = |name: &str, place| Spare {
            name: name.into(),
            place,
            count: 1,
        };
        snap.sell = vec![spare("Gravenspine", Place::Inventory)];
        snap.give_away = vec![spare("Magefist", Place::Shared)];
        let blocks = text_blocks(&snap, 400, Some(font));
        // The dashboard's JSON is built from these blocks (crate::web::live_json); the overlay
        // draws only the map and info line, so the lists are asserted here, not in a rendered frame.
        assert_eq!(blocks.keep, vec!["Keep: Harlequin Crest (S)"]);
        assert_eq!(blocks.sell, vec!["Sell: Gravenspine (inv)"]);
        assert_eq!(blocks.give, vec!["Give away: Magefist (shared)"]);
        assert!(
            blocks
                .level_full
                .first()
                .is_some_and(|e| e.text.starts_with("Level 34 → ")),
            "level suggestions for clvl 34: {:?}",
            blocks.level_full
        );
        assert!(!blocks.level_full.is_empty(), "the whole band for the panel");
        let Some(bis) = blocks.bis.first() else {
            anyhow::bail!("no helm chase line: {:?}", blocks.bis);
        };
        assert_eq!(bis.text, "helm: Peasant Crown (28)");
        assert_eq!(bis.tick, None, "nothing owned yet");
        Ok(())
    }

    #[test]
    fn the_overlay_frame_draws_no_guide_or_lists() -> Result<()> {
        use crate::valuables::{Category, Keep, Place, Spare, Tier};
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
        snap.player.bis = Some(crate::config::Bis {
            build: String::new(),
            slots: BTreeMap::from([("helm".to_owned(), vec!["Peasant Crown@28".to_owned()])]),
            merc_slots: BTreeMap::new(),
        });
        snap.inventory_keep = vec![Keep {
            name: "Harlequin Crest".into(),
            tier: Tier::S,
            category: Category::Trade,
            count: 1,
        }];
        let spare = |name: &str, place| Spare {
            name: name.into(),
            place,
            count: 1,
        };
        snap.sell = vec![spare("Gravenspine", Place::Inventory)];
        snap.give_away = vec![spare("Magefist", Place::Shared)];
        let (w, h) = (400, 400);
        let pm = render_with(&snap, w, h, Some(font))?;
        // The bottom band holds only the white info line: no guide (violet) and none of the
        // dashboard's lists in their colours.
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
    fn one_exit_label_per_linked_level() {
        let got = exit_labels(&bordered(28));
        assert_eq!(
            got,
            vec![
                // Tile link from room b without a loaded tile preset: the room's centre.
                (29, 160.0, 120.0),
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

    #[test]
    fn exit_labels_are_grey_where_xp_is_low() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        // One room linking to Bloody Foothills (Normal mlvl 24).
        let mut snap = empty_snapshot(120, 120);
        snap.level.rooms = vec![room(100, 100, true)];
        snap.room_links = vec![RoomLink {
            from_room: RoomPos { x: 100, y: 100 },
            to_level_id: 110,
            to_room: RoomPos { x: 0, y: 0 },
        }];
        snap.difficulty = Some(0);
        let count = |snap: &Snapshot| -> Result<(u32, u32)> {
            let (w, h) = (400, 400);
            let pm = render_with(snap, w, h, Some(font))?;
            let (mut grey, mut white) = (0, 0);
            for y in 0..map_height(h, Some(font)) {
                for x in 0..w {
                    let p = rgba(&pm, x, y)?;
                    // Glyph cores, opaque over the translucent room fill; edges blend.
                    if p[3] >= 250 && p.iter().take(3).all(|&c| (125..=155).contains(&c)) {
                        grey += 1;
                    } else if is_text_white(p) {
                        white += 1;
                    }
                }
            }
            Ok((grey, white))
        };
        // clvl 10: 14 levels below the monsters, 2%.
        snap.player.char_level = Some(10);
        let (grey, white) = count(&snap)?;
        assert!(grey >= 20 && white == 0, "low xp: {grey} grey, {white} white");
        // clvl 20: 4 below, full experience.
        snap.player.char_level = Some(20);
        let (grey, white) = count(&snap)?;
        // White glyph edges blend into a few grey-ish pixels.
        assert!(
            white >= 20 && grey * 4 < white,
            "full xp: {grey} grey, {white} white"
        );
        Ok(())
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
            // Size 7: about 45 px of area; most of it is fully covered.
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
        // Fake tomb 71: its chest chamber 473 is the treasure room, not the orifice.
        let mut snap = sanctuary([Some(473), Some(424), None, None]);
        snap.level.id = 71;
        assert_eq!(
            landmarks(&snap, LANDMARKS),
            vec![(Marker::Landmark("tomb treasure"), 30.0, 30.0)]
        );
        // Tomb 66 with 460 (Talrasha W): confirmed in tomb 69, so a star in any tomb.
        let mut snap = sanctuary([None, Some(460), None, None]);
        snap.level.id = 66;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(Marker::Orifice, 90.0, 30.0)]);
        // 462 is the orifice in whichever tomb is the real one.
        let mut snap = sanctuary([None, Some(462), None, None]);
        snap.level.id = 66;
        assert_eq!(landmarks(&snap, LANDMARKS), vec![(orifice, 90.0, 30.0)]);
        // 456 (a cube piece) in a tomb is nothing.
        let mut snap = sanctuary([None, Some(456), None, None]);
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
        let kinds: Vec<Marker> = markers_with(&snap, LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
        assert_eq!(kinds, vec![summoner, Marker::Superunique, Marker::Player]);
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
    fn all_landmark_rules_are_confirmed() {
        // Every rule is live-confirmed: the W/E/S/N variants of a named piece inherit the
        // confirmation of their family (the direction letter only says where in the piece the
        // feature sits), so no landmark is drawn as a mere "?" any more.
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
        // No live rule is unconfirmed any more, so the ? path is exercised with a synthetic one
        // over the fixture's 461 room.
        let unconfirmed = [LandmarkRule {
            level_ids: 66..=72,
            preset_ids: 461..=461,
            label: "orifice (E)",
            confirmed: false,
            style: LandmarkStyle::Orifice,
        }];
        // Without a font: a hollow gold ring, no glyph.
        let plain = render_rules(&snap, w, h, None, &unconfirmed)?;
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
        let pm = render_rules(&snap, w, h, Some(font), &unconfirmed)?;
        let v = View::fit(level_bounds(&snap), w, map_height(h, Some(font)));
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
        let kinds: Vec<Marker> = markers_with(&snap, LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
        assert_eq!(
            kinds,
            vec![Marker::Orifice, Marker::QuestObject("orifice"), Marker::Player]
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
        let kinds: Vec<Marker> = markers_with(&snap, LANDMARKS)
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
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
    fn marker_tags_are_short_unique_per_label_and_skip_monsters() {
        let mut labels: Vec<&str> = MARKER_TAGS.iter().map(|&(l, _)| l).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), MARKER_TAGS.len(), "a label is listed twice");
        for &(label, tag) in MARKER_TAGS {
            assert!(!tag.is_empty() && tag.len() <= 5, "{label}: {tag:?}");
        }
        for &(_, label) in QUEST_OBJECTS {
            assert!(
                marker_tag_of(label).is_some(),
                "quest object {label:?} has no tag"
            );
        }
        for &(_, label) in QUEST_MONSTERS {
            assert_eq!(marker_tag(Marker::QuestObject(label)), None, "{label}");
        }
        for rule in LANDMARKS {
            if let Some(tag) = marker_tag_of(rule.label) {
                // "orifice" also names the quest object; the Orifice landmark is tagged by kind.
                assert!(
                    rule.confirmed && matches!(rule.style, LandmarkStyle::Star | LandmarkStyle::Orifice),
                    "{tag}"
                );
            }
        }
        for label in ["summoner", "radament's lair", "smith (forge)"] {
            assert_eq!(marker_tag(Marker::Landmark(label)), None, "{label} is a monster");
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
    fn marker_tags_rank_waypoint_then_objects_then_shrines_then_wells() {
        let rank = |m| marker_tag(m).map(|(r, _)| r);
        assert_eq!(marker_tag(Marker::Waypoint), Some((1, "wp")));
        assert_eq!(marker_tag(Marker::QuestObject("hellforge")), Some((2, "forge")));
        assert_eq!(rank(Marker::Orifice), Some(2));
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
        let pm = render_with(&snap, w, h, Some(font))?;
        let ink = count_near(&pm, (cx, cy - 12.0), 14.0, |p| tinted(p, colour))?;
        assert!(ink >= 10, "SKL tag: {ink} px");
        // A well on the same spot: the shrine outranks it, so the well's tag moves or drops and
        // never covers the shrine's.
        snap.wells = vec![well_at(110, 110)];
        let pm = render_with(&snap, w, h, Some(font))?;
        let again = count_near(&pm, (cx, cy - 12.0), 14.0, |p| tinted(p, colour))?;
        assert_eq!(again, ink, "shrine tag unchanged");
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
        let plain = render_with(&snap, w, h, None)?;
        // A walked line, then a 50-tile jump (a teleport) and on.
        snap.trail = vec![(20, 40), (30, 40), (40, 40), (90, 40), (100, 40)];
        let trailed = render_with(&snap, w, h, None)?;
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
