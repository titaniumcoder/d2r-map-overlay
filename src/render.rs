//! Draws a [`Snapshot`] as an isometric corner minimap into a tiny-skia [`Pixmap`].
//!
//! Pure: no I/O. The `--render` CLI writes the pixmap as PNG; the overlay copies it into a
//! Wayland shm buffer.

use anyhow::{Context, Result};
use tiny_skia::{FillRule, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::game::{Bounds, Monster, Preset, PresetKind, Room, Snapshot};
use crate::items::{DropKind, GemColour};

/// Object txt ids of waypoints: every waypoint row of classic objects.txt. Live-verified: 119
/// (Act 1 town), 156 (Act 2 outdoor, Dry Hills), 157 (Act 1 indoor, Jail 1).
pub const WAYPOINT_IDS: &[u32] = &[
    119, 145, 156, 157, 237, 238, 288, 323, 324, 398, 402, 429, 494, 496, 511, 539,
];
/// A room that marks a landmark: the rooms of a level in `level_ids` whose level-preset id (lvlprest
/// row) is in `preset_ids` get one [`Marker::Landmark`] at the centre of their combined bounding box
/// (one piece often spans several Room2 cells).
#[derive(Debug)]
pub struct LandmarkRule {
    pub level_ids: std::ops::RangeInclusive<u32>,
    pub preset_ids: std::ops::RangeInclusive<u32>,
    /// What the landmark is (not drawn; for tests and debugging).
    pub label: &'static str,
}

/// Landmark rules.
/// - Arcane Sanctuary (74): the arm end holding the Summoner's platform has its own preset per
///   direction: 525 (+x), 526 (−x), 527 (−y), 528 (+y), all four confirmed in game (2026-11-15).
///   Ordinary arm ends use other ids (510, 511, 513, 517 seen).
/// - Tal Rasha's tombs (66–72): the real tomb's orifice chamber. Confirmed: 462 in tomb 72 (orifice
///   object 152 inside it). A fake tomb's chest chamber was 473 in tomb 71. Pattern assumed: orifice
///   chamber = level + 390 (456–462), chest chamber = level + 402 (468–474); only 462 is confirmed.
///   The 462 chamber spans 4 Room2 cells.
/// - Flayer Jungle (78): the Gidbinn altar (object 252) lies in piece 602, live-verified 2026-11-15
///   (player in a 602 room 5 tiles from the altar). 602 spans 16 Room2 cells; 601 is the waypoint
///   area (waypoint object 237).
pub const LANDMARKS: &[LandmarkRule] = &[
    LandmarkRule {
        level_ids: 74..=74,
        preset_ids: 525..=528,
        label: "summoner",
    },
    LandmarkRule {
        level_ids: 66..=72,
        preset_ids: 456..=462,
        label: "orifice",
    },
    LandmarkRule {
        level_ids: 78..=78,
        preset_ids: 602..=602,
        label: "gidbinn",
    },
];

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
}

/// Something drawn on the map. Variant order is draw order (player last, on top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Marker {
    /// Tile preset: a level exit.
    Exit,
    /// The tile preset the player entered the level by (see `Session::apply`).
    Entry,
    Waypoint,
    /// Monster preset: superunique or other special spawn.
    Special,
    /// A landmark room (see [`LANDMARKS`]), with the rule's label. Drawn below live elites so the
    /// boss standing on it stays visible.
    Landmark(&'static str),
    /// Live superunique, champion or unique monster.
    Elite,
    /// An item on the ground worth picking up.
    Drop(DropKind),
    Player,
}

impl Marker {
    /// Fill colour, size in px (radius, or see [`Shape`]) and shape.
    const fn style(self) -> (Rgba, f32, Shape) {
        match self {
            Self::Exit => ([170, 90, 255, 255], 5.0, Shape::Circle),
            Self::Entry => ([40, 200, 200, 255], 4.5, Shape::Square),
            Self::Waypoint => ([60, 140, 255, 255], 5.0, Shape::Circle),
            Self::Special => ([255, 110, 40, 255], 4.5, Shape::Circle),
            Self::Landmark(_) => (LANDMARK_GOLD, 9.0, Shape::Star),
            Self::Elite => ([230, 30, 30, 255], 3.5, Shape::Circle),
            Self::Drop(DropKind::Unique) => ([235, 190, 60, 255], 5.0, Shape::Diamond),
            Self::Drop(DropKind::Set) => ([30, 220, 30, 255], 5.0, Shape::Diamond),
            Self::Drop(DropKind::Rune(_)) => ([255, 150, 20, 255], 3.5, Shape::Square),
            Self::Drop(DropKind::Gem(colour)) => (gem_colour(colour), 3.5, Shape::Diamond),
            Self::Player => ([40, 255, 60, 255], 4.5, Shape::Circle),
        }
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

/// Live superunique/champion/unique, not dead. Minion-only (0x10) and normal (0) monsters are skipped.
pub fn is_elite(m: &Monster) -> bool {
    m.flags & ELITE_FLAGS != 0 && !m.is_dead()
}

/// The marker a preset is drawn as, if any.
pub fn preset_marker(p: &Preset) -> Option<Marker> {
    match p.kind {
        PresetKind::Object if WAYPOINT_IDS.contains(&p.txt) => Some(Marker::Waypoint),
        PresetKind::Tile if p.entry => Some(Marker::Entry),
        PresetKind::Tile => Some(Marker::Exit),
        PresetKind::Monster => Some(Marker::Special),
        PresetKind::Object | PresetKind::Other(_) => None,
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
            Some((Marker::Landmark(rule.label), x, y))
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
            snap.monsters
                .iter()
                .filter(|m| is_elite(m))
                .map(|m| (Marker::Elite, f64::from(m.x), f64::from(m.y))),
        )
        .chain(
            snap.drops
                .iter()
                .map(|d| (Marker::Drop(d.kind), f64::from(d.x), f64::from(d.y))),
        )
        .chain(landmarks(snap, LANDMARKS))
        .collect();
    out.push((Marker::Player, f64::from(snap.player.x), f64::from(snap.player.y)));
    // Stable sort: keeps snapshot order within one kind.
    out.sort_by_key(|&(m, _, _)| m);
    out
}

/// Renders the snapshot's level into a new `width`×`height` pixmap. There is no background panel:
/// everything outside the level shape and the markers is fully transparent.
///
/// # Errors
/// Fails if the canvas size is zero or too large for tiny-skia.
pub fn render(snap: &Snapshot, width: u32, height: u32) -> Result<Pixmap> {
    let mut pm =
        Pixmap::new(width, height).with_context(|| format!("invalid canvas size {width}x{height}"))?;
    let view = View::fit(level_bounds(snap), width, height);
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
        let Some(dot) = marker_path(shape, cx, cy, size) else {
            continue;
        };
        if shape == Shape::Star {
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
    Ok(pm)
}

/// The outline of a marker centred on (`cx`, `cy`); `None` for a degenerate size.
fn marker_path(shape: Shape, cx: f32, cy: f32, size: f32) -> Option<Path> {
    match shape {
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
    use crate::game::{Addr, ItemDrop, Level, Player};

    const FIXTURE: &str = include_str!("../tests/fixtures/jail1-snapshot.json");

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
            level: Level {
                id: 1,
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
            Some(Marker::Special)
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
    fn markers_draw_player_last() {
        let mut snap = empty_snapshot(5, 5);
        snap.monsters = vec![monster(0x08, 1), monster(0x10, 1)];
        snap.presets = vec![preset(PresetKind::Monster, 1), preset(PresetKind::Tile, 1)];
        let kinds: Vec<Marker> = markers(&snap).into_iter().map(|(m, _, _)| m).collect();
        assert_eq!(
            kinds,
            vec![Marker::Exit, Marker::Special, Marker::Elite, Marker::Player]
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
        assert!(hits >= 9, "only {hits} player pixels");
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
                Marker::Drop(DropKind::Gem(GemColour::Ruby)),
                Marker::Drop(DropKind::Rune(8)),
                Marker::Drop(DropKind::Unique),
                Marker::Player,
            ],
            "unique on top of the other drops, the player on top of all"
        );
        // Drops never look like a dot: every drop style has a non-circle shape.
        for kind in [
            DropKind::Unique,
            DropKind::Set,
            DropKind::Rune(1),
            DropKind::Gem(GemColour::Skull),
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
            Marker::Special,
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
}
