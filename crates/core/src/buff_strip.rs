//! The timed-buff strip: tags and time left side by side just below the character.
//!
//! The surface is centred on the screen by the compositor (no anchors), so the strip sits in its
//! lower half and the upper half stays transparent: the screen size is never needed.

use ab_glyph::FontVec;
use tiny_skia::Pixmap;

use crate::buffs::Entry;
use crate::info;

/// px from the screen centre down to the strip's top: D2R draws the character a bit above centre.
pub const BELOW_CENTRE: u32 = 70;
pub const WIDTH: u32 = 640;
const ROW: u32 = 24;
pub const HEIGHT: u32 = 2 * (BELOW_CENTRE + ROW);
const TEXT_PX: f32 = 15.0;
/// px between two entries.
const GAP: f32 = 14.0;
pub const GOOD: [u8; 4] = [120, 235, 130, 255];
/// [`Entry::running_out`].
pub const ENDING: [u8; 4] = [255, 160, 60, 255];

/// The strip for `entries`, or `None` when there is nothing to show.
#[must_use]
pub fn render(entries: &[Entry], font: &FontVec) -> Option<Pixmap> {
    if entries.is_empty() {
        return None;
    }
    let mut pm = Pixmap::new(WIDTH, HEIGHT)?;
    let texts: Vec<(String, [u8; 4])> = entries
        .iter()
        .map(|e| (e.text(), if e.running_out() { ENDING } else { GOOD }))
        .collect();
    let widths: Vec<f32> = texts
        .iter()
        .map(|(text, _)| info::label_size_at(font, TEXT_PX, text).width)
        .collect();
    let total = GAP.mul_add(gaps(entries.len()), widths.iter().sum::<f32>());
    let size = info::label_size_at(font, TEXT_PX, "");
    let baseline = px(HEIGHT / 2 + BELOW_CENTRE) + (px(ROW) + size.ascent + size.descent) / 2.0;
    let mut x = (px(WIDTH) - total).max(0.0) / 2.0;
    for ((text, colour), width) in texts.iter().zip(&widths) {
        info::draw_label_at(&mut pm, font, TEXT_PX, *colour, text, (x, baseline));
        x += width + GAP;
    }
    Some(pm)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "a handful of entries, far below f32's exact integers"
)]
const fn gaps(entries: usize) -> f32 {
    entries.saturating_sub(1) as f32
}

#[allow(
    clippy::cast_precision_loss,
    reason = "surface sizes, far below f32's exact integers"
)]
const fn px(n: u32) -> f32 {
    n as f32
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn entry(tag: &'static str, left: u64) -> Entry {
        Entry {
            tag,
            stacks: None,
            left: Some(Duration::from_secs(left)),
        }
    }

    /// Pixels close to `colour` in rows `rows`, by column span: (count, leftmost x, rightmost x).
    fn tinted(pm: &Pixmap, rows: std::ops::Range<u32>, colour: [u8; 4]) -> (u32, u32, u32) {
        let mut found = (0, u32::MAX, 0);
        for y in rows {
            for x in 0..pm.width() {
                let Some(p) = pm.pixel(x, y).map(|p| p.demultiply()) else {
                    continue;
                };
                let near = |a: u8, b: u8| a.abs_diff(b) <= 30;
                if p.alpha() >= 200
                    && near(p.red(), colour[0])
                    && near(p.green(), colour[1])
                    && near(p.blue(), colour[2])
                {
                    found = (found.0 + 1, found.1.min(x), found.2.max(x));
                }
            }
        }
        found
    }

    #[test]
    fn nothing_to_show_is_no_strip() {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        assert!(render(&[], font).is_none());
    }

    #[test]
    fn entries_sit_centred_below_the_middle_and_ending_ones_turn_orange() -> anyhow::Result<()> {
        use anyhow::Context;
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let pm = render(&[entry("BO", 102), entry("XP", 7)], font).context("a strip for two entries")?;
        assert_eq!((pm.width(), pm.height()), (WIDTH, HEIGHT));
        let strip = HEIGHT / 2 + BELOW_CENTRE..HEIGHT / 2 + BELOW_CENTRE + ROW;
        let (green, g_left, g_right) = tinted(&pm, strip.clone(), GOOD);
        let (orange, o_left, o_right) = tinted(&pm, strip, ENDING);
        assert!(green >= 20, "only {green} green pixels");
        assert!(orange >= 20, "only {orange} orange pixels");
        assert!(g_right < o_left, "BO left of XP: {g_right} vs {o_left}");
        let (left_margin, right_margin) = (g_left, WIDTH - 1 - o_right);
        assert!(
            left_margin.abs_diff(right_margin) <= 4,
            "centred: {left_margin} vs {right_margin}"
        );
        let above = tinted(&pm, 0..HEIGHT / 2 + BELOW_CENTRE, GOOD).0
            + tinted(&pm, 0..HEIGHT / 2 + BELOW_CENTRE, ENDING).0;
        assert_eq!(above, 0, "nothing above the strip's row");
        Ok(())
    }
}
