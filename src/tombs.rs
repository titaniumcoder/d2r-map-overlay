//! The seven Tal Rasha's Tombs (levels 66..=72) and their Horadric symbols, drawn on the
//! Canyon of the Magi's exits instead of seven identical "Tal Rasha's Tomb" names (user
//! request, 2026-11-18).
//!
//! The symbol belongs to the tomb level, not to the game: each tomb has its own entrance warp
//! (lvlwarp.txt "Act 2 Desert to Tomb Tal 1".."7", rows 38..=44, which Canyon of the Magi's
//! levels.txt row lists as `Warp1..7` next to `Vis1..7` = levels 66..=72), so its doorway
//! always carries the same symbol. Only which tomb is the real one changes per game.
//!
//! The pictures: classic `data\global\ui\MENU\invps.DC6` (d2data.mpq), 7 frames of 102 × 93,
//! one per tomb level in order; the 1.13c quest log draws frame `staff tomb level − 66` as
//! "The Symbol of the True Tomb of Tal Rasha" (D2Client.dll disassembly, troytt/diablo2-web
//! `kb/ui/d2client-quest-log-true-tomb-symbol-113c.md`). The frames were extracted from the
//! local d2data.mpq and decoded on 2026-11-18; [`Symbol`] describes each one.

use std::ops::RangeInclusive;

use tiny_skia::{Path, PathBuilder};

/// Level ids of Tal Rasha's Tomb 1..7.
pub const TOMB_LEVELS: RangeInclusive<u32> = 66..=72;

/// A tomb's symbol, as the invps.DC6 frame shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    /// Tomb 1 (66): a pinwheel, four curved blades around a small square.
    Pinwheel,
    /// Tomb 2 (67): a square frame between two upright posts.
    Gate,
    /// Tomb 3 (68): a crescent, horns up.
    Crescent,
    /// Tomb 4 (69): a ring.
    Ring,
    /// Tomb 5 (70): two nested chevrons pointing down under a small diamond.
    Chevrons,
    /// Tomb 6 (71): a triangle inside a triangle, pointing up.
    Triangle,
    /// Tomb 7 (72): a ring above a broad crescent.
    Eclipse,
}

/// Tomb level 66..=72 → symbol, in invps.DC6 frame order.
const SYMBOLS: [Symbol; 7] = [
    Symbol::Pinwheel,
    Symbol::Gate,
    Symbol::Crescent,
    Symbol::Ring,
    Symbol::Chevrons,
    Symbol::Triangle,
    Symbol::Eclipse,
];

/// The symbol of tomb level `level`; `None` for any other level.
pub fn symbol(level: u32) -> Option<Symbol> {
    let index = level.checked_sub(*TOMB_LEVELS.start())?;
    SYMBOLS.get(usize::try_from(index).ok()?).copied()
}

/// A symbol's shapes in canvas px: `lines` are stroked, `solid` (where the symbol has filled
/// parts) is filled. Both get the same dark outline.
#[derive(Debug)]
pub struct Glyph {
    pub lines: Option<Path>,
    pub solid: Option<Path>,
}

impl Symbol {
    /// The symbol centred on (`cx`, `cy`), fitting a square of half-side `half` px (strokes
    /// reach a little past it).
    pub fn glyph(self, cx: f32, cy: f32, half: f32) -> Glyph {
        // Unit coordinates (-1..=1, y down) to canvas px.
        let p = |u: f32, v: f32| (u.mul_add(half, cx), v.mul_add(half, cy));
        let mut lines = PathBuilder::new();
        let mut solid = PathBuilder::new();
        let poly = |pb: &mut PathBuilder, pts: &[(f32, f32)], close: bool| {
            for (i, &(u, v)) in pts.iter().enumerate() {
                let (x, y) = p(u, v);
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            if close {
                pb.close();
            }
        };
        match self {
            Self::Pinwheel => {
                // Four curved blades pointing up, right, down and left from a small square: the
                // up blade bows left with its tip turned right, the others are it turned
                // clockwise by quarter turns.
                for k in 0..4u8 {
                    let turn = |u: f32, w: f32| {
                        let (mut x, mut y) = (u, -w);
                        for _ in 0..k {
                            (x, y) = (-y, x);
                        }
                        p(x, y)
                    };
                    let [(ax, ay), (bx, by), (tx, ty), (cx2, cy2), (dx, dy)] = [
                        (-0.14, 0.12),
                        (-0.4, 0.6),
                        (0.06, 1.0),
                        (-0.08, 0.55),
                        (0.14, 0.12),
                    ]
                    .map(|(u, w)| turn(u, w));
                    solid.move_to(ax, ay);
                    solid.quad_to(bx, by, tx, ty);
                    solid.quad_to(cx2, cy2, dx, dy);
                    solid.close();
                }
                poly(
                    &mut solid,
                    &[(-0.22, -0.22), (0.22, -0.22), (0.22, 0.22), (-0.22, 0.22)],
                    true,
                );
            }
            Self::Gate => {
                poly(&mut lines, &[(-0.8, -1.0), (-0.8, 1.0)], false);
                poly(&mut lines, &[(0.8, -1.0), (0.8, 1.0)], false);
                poly(&mut lines, &[(-0.8, -0.7), (0.8, -0.7)], false);
                poly(&mut lines, &[(-0.8, 0.7), (0.8, 0.7)], false);
            }
            Self::Crescent => {
                let ((lx, ly), (rx, ry)) = (p(-1.0, -0.7), p(1.0, -0.7));
                // Quad bottoms halfway to the control points: outer 0.5, inner -0.05.
                let ((ox, oy), (ix, iy)) = (p(0.0, 1.7), p(0.0, 0.6));
                solid.move_to(lx, ly);
                solid.quad_to(ox, oy, rx, ry);
                solid.quad_to(ix, iy, lx, ly);
                solid.close();
            }
            Self::Ring => {
                let (x, y) = p(0.0, 0.0);
                lines.push_circle(x, y, 0.75 * half);
            }
            Self::Chevrons => {
                poly(&mut lines, &[(-0.95, -0.85), (0.0, 0.95), (0.95, -0.85)], false);
                poly(&mut lines, &[(-0.5, -0.85), (0.0, 0.15), (0.5, -0.85)], false);
                poly(
                    &mut solid,
                    &[(0.0, -0.75), (0.17, -0.5), (0.0, -0.25), (-0.17, -0.5)],
                    true,
                );
            }
            Self::Triangle => {
                poly(&mut lines, &[(0.0, -1.0), (1.0, 0.8), (-1.0, 0.8)], true);
                poly(&mut lines, &[(0.0, 0.0), (0.33, 0.5), (-0.33, 0.5)], true);
            }
            Self::Eclipse => {
                // A ring above a broad crescent that cups it.
                let (x, y) = p(0.0, -0.4);
                lines.push_circle(x, y, 0.5 * half);
                let ((lx, ly), (rx, ry)) = (p(-0.95, 0.0), p(0.95, 0.0));
                // Quad bottoms halfway to the control points: outer 0.95, inner 0.45.
                let ((ox, oy), (ix, iy)) = (p(0.0, 1.9), p(0.0, 0.9));
                solid.move_to(lx, ly);
                solid.quad_to(ox, oy, rx, ry);
                solid.quad_to(ix, iy, lx, ly);
                solid.close();
            }
        }
        Glyph {
            lines: lines.finish(),
            solid: solid.finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tomb_level_has_its_own_symbol() {
        let all: Vec<Symbol> = TOMB_LEVELS.filter_map(symbol).collect();
        assert_eq!(all.len(), 7);
        for (i, a) in all.iter().enumerate() {
            assert!(!all.iter().skip(i + 1).any(|b| b == a), "{a:?} twice");
        }
        assert_eq!(symbol(66), Some(Symbol::Pinwheel));
        assert_eq!(symbol(69), Some(Symbol::Ring));
        assert_eq!(symbol(72), Some(Symbol::Eclipse));
        for level in [0, 46, 65, 73, u32::MAX] {
            assert_eq!(symbol(level), None, "{level}");
        }
    }

    #[test]
    fn glyphs_stay_near_their_box() {
        for s in SYMBOLS {
            let g = s.glyph(50.0, 40.0, 8.0);
            let paths: Vec<&Path> = g.lines.iter().chain(g.solid.iter()).collect();
            assert!(!paths.is_empty(), "{s:?} draws nothing");
            for b in paths.iter().filter_map(|p| p.compute_tight_bounds()) {
                // Half-side 8 around (50, 40), with float slack.
                assert!(b.left() >= 41.9 && b.right() <= 58.1, "{s:?} x {b:?}");
                assert!(b.top() >= 31.9 && b.bottom() <= 48.1, "{s:?} y {b:?}");
                assert!(b.width().max(b.height()) >= 3.0, "{s:?} tiny {b:?}");
            }
        }
    }
}
