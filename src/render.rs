//! Draws a [`Snapshot`] as an isometric corner minimap into a tiny-skia [`Pixmap`].
//!
//! Pure: no I/O. The `--render` CLI writes the pixmap as PNG; the overlay copies it into a
//! Wayland shm buffer.

use anyhow::{Context, Result};
use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::game::{Bounds, Monster, Preset, PresetKind, Snapshot};

/// Object txt ids of waypoints (seen live: 119 town, 157 Jail 1). Full list: objects.txt waypoint rows.
pub const WAYPOINT_IDS: &[u32] = &[119, 157];
/// Monster flag bits for superunique (0x02), champion (0x04) and unique/boss (0x08); minion is 0x10.
const ELITE_FLAGS: u8 = 0x0E;
/// Monster modes of a dead or dying unit.
const DEAD_MODES: &[u32] = &[0, 12];

/// Canvas px kept free around the level so markers on its edge stay inside the canvas.
const MARGIN: f64 = 14.0;
/// Corner radius of the background panel in px.
const CORNER: f32 = 10.0;
/// Half-size in tiles of the box drawn around the player when the level has no rooms.
const FALLBACK_SPAN: u32 = 50;

type Rgba = [u8; 4];
const BACKGROUND: Rgba = [12, 14, 18, 110];
const ROOM: Rgba = [70, 74, 84, 120];
const ROOM_LOADED: Rgba = [125, 130, 142, 160];
const ROOM_OUTLINE: Rgba = [190, 195, 205, 170];
const MARKER_OUTLINE: Rgba = [0, 0, 0, 220];

/// Something drawn as a dot on the map. Variant order is draw order (player last, on top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Marker {
    /// Tile preset: a level exit.
    Exit,
    Waypoint,
    /// Monster preset: superunique or other special spawn.
    Special,
    /// Live superunique, champion or unique monster.
    Elite,
    Player,
}

impl Marker {
    /// Fill colour and radius in px.
    const fn style(self) -> (Rgba, f32) {
        match self {
            Self::Exit => ([170, 90, 255, 255], 5.0),
            Self::Waypoint => ([60, 140, 255, 255], 5.0),
            Self::Special => ([255, 110, 40, 255], 4.5),
            Self::Elite => ([230, 30, 30, 255], 3.5),
            Self::Player => ([40, 255, 60, 255], 4.5),
        }
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
    m.flags & ELITE_FLAGS != 0 && !DEAD_MODES.contains(&m.mode)
}

/// The marker a preset is drawn as, if any.
pub fn preset_marker(p: &Preset) -> Option<Marker> {
    match p.kind {
        PresetKind::Object if WAYPOINT_IDS.contains(&p.txt) => Some(Marker::Waypoint),
        PresetKind::Tile => Some(Marker::Exit),
        PresetKind::Monster => Some(Marker::Special),
        PresetKind::Object | PresetKind::Other(_) => None,
    }
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
        .collect();
    out.push((Marker::Player, f64::from(snap.player.x), f64::from(snap.player.y)));
    // Stable sort: keeps snapshot order within one kind.
    out.sort_by_key(|&(m, _, _)| m);
    out
}

/// Renders the snapshot's level into a new `width`×`height` pixmap (transparent outside the panel).
///
/// # Errors
/// Fails if the canvas size is zero or too large for tiny-skia.
pub fn render(snap: &Snapshot, width: u32, height: u32) -> Result<Pixmap> {
    let mut pm =
        Pixmap::new(width, height).with_context(|| format!("invalid canvas size {width}x{height}"))?;
    if let Some(panel) = rounded_rect(px(f64::from(width)), px(f64::from(height)), CORNER) {
        pm.fill_path(
            &panel,
            &paint(BACKGROUND),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    let view = View::fit(level_bounds(snap), width, height);
    let outline = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    for room in &snap.level.rooms {
        let (x0, y0) = (f64::from(room.x), f64::from(room.y));
        let (x1, y1) = (f64::from(room.x + room.w), f64::from(room.y + room.h));
        let Some(path) = polygon([
            view.to_screen(x0, y0),
            view.to_screen(x1, y0),
            view.to_screen(x1, y1),
            view.to_screen(x0, y1),
        ]) else {
            continue;
        };
        let fill = if room.loaded { ROOM_LOADED } else { ROOM };
        pm.fill_path(
            &path,
            &paint(fill),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        pm.stroke_path(&path, &paint(ROOM_OUTLINE), &outline, Transform::identity(), None);
    }

    let ring = Stroke {
        width: 1.5,
        ..Stroke::default()
    };
    for (marker, x, y) in markers(snap) {
        let (colour, radius) = marker.style();
        let (cx, cy) = view.to_screen(x, y);
        if let Some(dot) = PathBuilder::from_circle(cx, cy, radius) {
            pm.fill_path(
                &dot,
                &paint(colour),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            pm.stroke_path(&dot, &paint(MARKER_OUTLINE), &ring, Transform::identity(), None);
        }
    }
    Ok(pm)
}

fn paint([r, g, b, a]: Rgba) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

fn polygon(points: [(f32, f32); 4]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let [(x, y), rest @ ..] = points;
    pb.move_to(x, y);
    for (x, y) in rest {
        pb.line_to(x, y);
    }
    pb.close();
    pb.finish()
}

fn rounded_rect(w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(r, 0.0);
    pb.line_to(w - r, 0.0);
    pb.quad_to(w, 0.0, w, r);
    pb.line_to(w, h - r);
    pb.quad_to(w, h, w - r, h);
    pb.line_to(r, h);
    pb.quad_to(0.0, h, 0.0, h - r);
    pb.line_to(0.0, r);
    pb.quad_to(0.0, 0.0, r, 0.0);
    pb.close();
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Addr, Level, Player, Room};

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
            items: vec![],
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
            },
            Room {
                x: 0,
                y: 30,
                w: 5,
                h: 10,
                loaded: true,
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
        assert_eq!(
            preset_marker(&preset(PresetKind::Object, 119)),
            Some(Marker::Waypoint)
        );
        assert_eq!(preset_marker(&preset(PresetKind::Object, 16)), None);
        assert_eq!(preset_marker(&preset(PresetKind::Tile, 13)), Some(Marker::Exit));
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

        // Rounded corner stays transparent; the panel edge has the background alpha.
        assert_eq!(
            pm.pixel(0, 0).map(tiny_skia::PremultipliedColorU8::alpha),
            Some(0)
        );
        assert_eq!(pm.pixel(3, h / 2).context("edge pixel")?.alpha(), BACKGROUND[3]);

        // Pixels in exactly the (opaque) player colour exist and all sit on the projected player.
        let v = View::fit(level_bounds(&snap), w, h);
        let (cx, cy) = v.to_screen(f64::from(snap.player.x), f64::from(snap.player.y));
        let (player, radius) = Marker::Player.style();
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

    #[test]
    fn render_rejects_zero_size() {
        assert!(render(&empty_snapshot(1, 1), 0, 10).is_err());
    }
}
