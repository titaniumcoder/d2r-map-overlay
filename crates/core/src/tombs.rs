//! The seven Tal Rasha's Tombs (levels 66..=72) and their Horadric symbols, drawn on the
//! Canyon of the Magi's exits. The symbol belongs to the tomb level, not to the game.

use std::ops::RangeInclusive;

use tiny_skia::{Path, PathBuilder};

/// Level ids of Tal Rasha's Tomb 1..7.
pub const TOMB_LEVELS: RangeInclusive<u32> = 66..=72;

/// A tomb's symbol, as frame `level − 66` of `invps.DC6` shows it.
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
#[must_use]
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
    #[must_use]
    pub fn glyph(self, cx: f32, cy: f32, half: f32) -> Glyph {
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
                for quarter_turns in 0..4u8 {
                    add_pinwheel_blade(&mut solid, &p, quarter_turns);
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
            Self::Crescent => add_crescent(&mut solid, &p, (1.0, -0.7), 1.7, 0.6),
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
                let (x, y) = p(0.0, -0.4);
                lines.push_circle(x, y, 0.5 * half);
                add_crescent(&mut solid, &p, (0.95, 0.0), 1.9, 0.9);
            }
        }
        Glyph {
            lines: lines.finish(),
            solid: solid.finish(),
        }
    }
}

/// One curved pinwheel blade: the upward one (bowing left, tip turned right) rotated clockwise
/// by `quarter_turns`. `p` maps unit coordinates (-1..=1, y down) to canvas px.
fn add_pinwheel_blade(pb: &mut PathBuilder, p: &impl Fn(f32, f32) -> (f32, f32), quarter_turns: u8) {
    let turn = |u: f32, w: f32| {
        let (mut x, mut y) = (u, -w);
        for _ in 0..quarter_turns {
            (x, y) = (-y, x);
        }
        p(x, y)
    };
    let [(ax, ay), (bx, by), (tx, ty), (cx, cy), (dx, dy)] = [
        (-0.14, 0.12),
        (-0.4, 0.6),
        (0.06, 1.0),
        (-0.08, 0.55),
        (0.14, 0.12),
    ]
    .map(|(u, w)| turn(u, w));
    pb.move_to(ax, ay);
    pb.quad_to(bx, by, tx, ty);
    pb.quad_to(cx, cy, dx, dy);
    pb.close();
}

/// A horns-up crescent between the horns (±`horn.0`, `horn.1`), bounded by two quads whose
/// control points sit at `outer_v` and `inner_v` below the centre (each quad's bottom lies
/// halfway between the horns' height and its control point).
fn add_crescent(
    pb: &mut PathBuilder,
    p: &impl Fn(f32, f32) -> (f32, f32),
    horn: (f32, f32),
    outer_v: f32,
    inner_v: f32,
) {
    let ((lx, ly), (rx, ry)) = (p(-horn.0, horn.1), p(horn.0, horn.1));
    let ((ox, oy), (ix, iy)) = (p(0.0, outer_v), p(0.0, inner_v));
    pb.move_to(lx, ly);
    pb.quad_to(ox, oy, rx, ry);
    pb.quad_to(ix, iy, lx, ly);
    pb.close();
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
                assert!(b.left() >= 41.9 && b.right() <= 58.1, "{s:?} x {b:?}");
                assert!(b.top() >= 31.9 && b.bottom() <= 48.1, "{s:?} y {b:?}");
                assert!(b.width().max(b.height()) >= 3.0, "{s:?} tiny {b:?}");
            }
        }
    }
}
