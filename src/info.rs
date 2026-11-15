//! The info line at the bottom of the overlay: difficulty, area name, monster levels and the
//! area's boss, e.g. `Normal · Flayer Dungeon Level 1 · mlvl 22 · champ 24 · unique 25`.
//!
//! Monster levels are the table values from `levels_data` (expansion). Terror Zones raise the
//! actual monster levels, so in a terrorized level (`Snapshot::terrorized`) the line shows
//! `terrorized` in violet instead of the table values and boss level:
//! `Normal · Moo Moo Farm · terrorized`.
//!
//! In the Moo Moo Farm with a cow counter (`Snapshot::cows`, see [`crate::cows`]) the monster and
//! boss levels give way to it: `Normal · Moo Moo Farm · kill count: 57 / victims alive: 23`.

use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont, point};
use tiny_skia::{Mask, Paint, Pixmap, Rect, Transform};

use crate::cows::{COW_LEVEL, Tally};
use crate::levels_data::{BOSSES, LEVELS};
use crate::valuables::{Keep, Spare, Tier};

/// Champions are this many levels above the area's monster level (D2 rule).
const CHAMPION_BONUS: u8 = 2;
/// Uniques and superuniques are this many levels above the area's monster level (D2 rule).
const UNIQUE_BONUS: u8 = 3;

const SEPARATOR: &str = " · ";
const ELLIPSIS: char = '…';

/// Font files tried in order; the first that loads is used. None is shipped with the program.
const FONT_PATHS: [&str; 3] = [
    "/usr/share/fonts/noto/NotoSans-Medium.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];
/// Text size in px.
const TEXT_PX: f32 = 16.0;
/// Px between the text (with its outline) and the canvas's left and bottom edges, and between
/// the text and the map above it.
const PAD: u32 = 3;
const TEXT: [u8; 4] = [255, 255, 255, 255];
/// The inventory keep lines: green, same outline as the info line.
const KEEP_TEXT: [u8; 4] = [90, 230, 90, 255];
/// Most keep lines; the last one ends in "+N more" when the list is longer.
pub const MAX_KEEP_LINES: usize = 3;
const KEEP_PREFIX: &str = "Keep: ";
/// The sell lines below the keep lines: grey.
pub const SELL_TEXT: [u8; 4] = [175, 175, 175, 255];
pub const SELL_PREFIX: &str = "Sell: ";
/// The give-away lines below the sell lines: yellow.
pub const GIVE_TEXT: [u8; 4] = [255, 225, 60, 255];
pub const GIVE_PREFIX: &str = "Give away: ";
/// Separator of the sell and give-away entries.
const COMMA: &str = ", ";
/// The last part of a terrorized level's line, drawn in [`TERROR_TEXT`].
const TERRORIZED: &str = "terrorized";
const TERROR_TEXT: [u8; 4] = [190, 120, 255, 255];
const OUTLINE: [u8; 4] = [0, 0, 0, 220];

pub const fn difficulty_name(d: u8) -> Option<&'static str> {
    match d {
        0 => Some("Normal"),
        1 => Some("Nightmare"),
        2 => Some("Hell"),
        _ => None,
    }
}

/// The parts of the info line. Only the area name is shortened when the line is too wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoLine {
    /// Difficulty name; `None` if unknown.
    pub difficulty: Option<&'static str>,
    /// Area name, or `Level <id>` for an id missing from the table.
    pub area: String,
    /// The monster-level and boss parts after the area name (empty without a known difficulty),
    /// or just [`TERRORIZED`] in a terrorized level.
    pub stats: Vec<String>,
    /// The level is terrorized: the last part is drawn in violet.
    pub terrorized: bool,
}

impl InfoLine {
    /// The line with `area` in place of the area name (for shortening it).
    pub fn with_area(&self, area: &str) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(self.stats.len() + 2);
        parts.extend(self.difficulty);
        parts.push(area);
        parts.extend(self.stats.iter().map(String::as_str));
        parts.join(SEPARATOR)
    }

    pub fn text(&self) -> String {
        self.with_area(&self.area)
    }
}

/// The info line for `level_id`. Without a known difficulty there is no monster-level column to
/// pick, so only the area name is shown. An area with monster level 0 (towns, the Forgotten
/// Tower's ground floor) has no monster levels. A `terrorized` level shows [`TERRORIZED`] in
/// place of monster and boss levels (the table values don't apply in a Terror Zone). In the Moo
/// Moo Farm, `cows` replaces them (a terrorized farm keeps [`TERRORIZED`] last).
pub fn info_line(level_id: u32, difficulty: Option<u8>, terrorized: bool, cows: Option<Tally>) -> InfoLine {
    let difficulty = difficulty.filter(|&d| difficulty_name(d).is_some());
    let level = LEVELS.iter().find(|&&(id, _, _)| id == level_id);
    let area = level.map_or_else(|| format!("Level {level_id}"), |&(_, name, _)| name.to_owned());
    let mut stats = Vec::new();
    if let Some(c) = cows.filter(|_| level_id == COW_LEVEL) {
        stats.push(format!("kill count: {} / victims alive: {}", c.kills, c.alive));
        if terrorized {
            stats.push(TERRORIZED.to_owned());
        }
    } else if terrorized {
        stats.push(TERRORIZED.to_owned());
    } else if let Some(d) = difficulty.map(usize::from) {
        if let Some(&mlvl) = level.and_then(|(_, _, lvls)| lvls.get(d))
            && mlvl > 0
        {
            stats.push(format!("mlvl {mlvl}"));
            stats.push(format!("champ {}", mlvl.saturating_add(CHAMPION_BONUS)));
            stats.push(format!("unique {}", mlvl.saturating_add(UNIQUE_BONUS)));
        }
        for (boss, _, lvls) in BOSSES.iter().filter(|&&(_, id, _)| id == level_id) {
            if let Some(blvl) = lvls.get(d) {
                stats.push(format!("{boss} {blvl}"));
            }
        }
    }
    InfoLine {
        difficulty: difficulty.and_then(difficulty_name),
        area,
        stats,
        terrorized,
    }
}

/// The info-line font, loaded on first use from [`FONT_PATHS`]; `None` (logged once) if none loads.
pub fn font() -> Option<&'static FontVec> {
    // `OnceLock` runs the loader once, even if the reader thread and `--render` race for it.
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let font = FONT_PATHS.iter().find_map(|p| {
            let bytes = std::fs::read(p).ok()?;
            FontVec::try_from_vec(bytes).ok()
        });
        if font.is_none() {
            eprintln!("info line off: no font could be loaded from {FONT_PATHS:?}");
        }
        font
    })
    .as_ref()
}

/// Height in px of one text line with its outline.
pub fn line_height(font: &FontVec) -> u32 {
    let scaled = font.as_scaled(PxScale::from(TEXT_PX));
    // +2: the 1 px outline above and below.
    ceil_px(scaled.height()) + 2
}

/// Extra canvas px at the bottom for `lines` text rows below the info line, gap rows included
/// (0 for none; see [`stack`]).
pub fn keep_band_height(font: &FontVec, lines: usize) -> u32 {
    u32::try_from(lines).map_or(0, |n| n.saturating_mul(line_height(font)))
}

/// One sell or give-away entry as text: `Gravenspine (inv)`, `Magefist ×2 (stash)`.
pub fn spare_entry(s: &Spare) -> String {
    let place = s.place.label();
    if s.count > 1 {
        format!("{} ×{} ({place})", s.name, s.count)
    } else {
        format!("{} ({place})", s.name)
    }
}

/// A sell or give-away list as lines after `prefix`, comma-separated, of at most `max_width` px
/// in `font`, capped like the keep lines.
pub fn spare_lines(font: &FontVec, prefix: &str, spares: &[Spare], max_width: f32) -> Vec<String> {
    let entries: Vec<String> = spares.iter().map(spare_entry).collect();
    wrap(prefix, COMMA, &entries, |t| text_width(font, t) <= max_width)
}

/// The text rows below the info line, top to bottom, each with its colour: the keep lines
/// (green), the sell lines (grey), the give-away lines (yellow), with an empty gap row before
/// each non-empty block (so also between the info line and the first block).
pub fn stack(keep: Vec<String>, sell: Vec<String>, give: Vec<String>) -> Vec<(String, [u8; 4])> {
    let mut rows = Vec::new();
    for (lines, colour) in [(keep, KEEP_TEXT), (sell, SELL_TEXT), (give, GIVE_TEXT)] {
        if lines.is_empty() {
            continue;
        }
        // A gap row before every block, also the first: it keeps the lists apart from the info
        // line above them (user request, 2026-11-15).
        rows.push((String::new(), colour));
        rows.extend(lines.into_iter().map(|l| (l, colour)));
    }
    rows
}

/// One keep-list entry as text: `Harlequin Crest (S)`, `Lem Rune ×2 (A)`.
pub fn keep_entry(k: &Keep) -> String {
    let tier = match k.tier {
        Tier::S => "S",
        Tier::A => "A",
        Tier::B => "B",
    };
    if k.count > 1 {
        format!("{} ×{} ({tier})", k.name, k.count)
    } else {
        format!("{} ({tier})", k.name)
    }
}

/// The keep list as lines of at most `max_width` px in `font` (see [`wrap_keep`]).
pub fn keep_lines(font: &FontVec, keep: &[Keep], max_width: f32) -> Vec<String> {
    let entries: Vec<String> = keep.iter().map(keep_entry).collect();
    wrap_keep(&entries, |t| text_width(font, t) <= max_width)
}

/// Packs `entries` after "Keep: " into lines joined by " · " for which `fits` holds (an entry too
/// wide for a line of its own gets one anyway), at most [`MAX_KEEP_LINES`]. If they don't all
/// fit, the last line ends in "+N more" for the rest. Empty for no entries.
pub fn wrap_keep(entries: &[String], fits: impl Fn(&str) -> bool) -> Vec<String> {
    wrap(KEEP_PREFIX, SEPARATOR, entries, fits)
}

/// [`wrap_keep`] with any `prefix` before the first line and `sep` between entries.
fn wrap(prefix: &str, sep: &str, entries: &[String], fits: impl Fn(&str) -> bool) -> Vec<String> {
    let render = |i: usize, line: &[String]| {
        let body = line.join(sep);
        if i == 0 { format!("{prefix}{body}") } else { body }
    };
    let mut lines: Vec<Vec<String>> = Vec::new();
    for e in entries {
        let i = lines.len().saturating_sub(1);
        match lines.last_mut() {
            Some(line) => {
                line.push(e.clone());
                if line.len() > 1 && !fits(&render(i, line)) {
                    line.pop();
                    lines.push(vec![e.clone()]);
                }
            }
            None => lines.push(vec![e.clone()]),
        }
    }
    if lines.len() > MAX_KEEP_LINES {
        lines.truncate(MAX_KEEP_LINES);
        let placed: usize = lines.iter().map(Vec::len).sum();
        let mut rest = entries.len().saturating_sub(placed);
        if let Some(last) = lines.last_mut() {
            loop {
                last.push(format!("+{rest} more"));
                if last.len() == 1 || fits(&render(MAX_KEEP_LINES - 1, last)) {
                    break;
                }
                // Drop "+N more" and move the line's last entry into the rest.
                last.pop();
                last.pop();
                rest += 1;
            }
        }
    }
    lines.iter().enumerate().map(|(i, l)| render(i, l)).collect()
}

/// Draws the text `lines` ([`stack`]) bottom-left into `pm` in their colours, the last one at
/// the bottom. Draws nothing if the canvas is shorter than the lines.
pub fn draw_keep(pm: &mut Pixmap, font: &FontVec, lines: &[(String, [u8; 4])]) {
    let h = pm.height();
    if h < keep_band_height(font, lines.len()) + 2 * PAD {
        return;
    }
    let scaled = font.as_scaled(PxScale::from(TEXT_PX));
    let lh = line_height(font);
    let x0 = f32_px(PAD + 1);
    for (from_bottom, (line, colour)) in (0u32..).zip(lines.iter().rev()) {
        if line.is_empty() {
            continue; // a gap row
        }
        let baseline = f32_px(h - PAD - 1 - from_bottom * lh) + scaled.descent();
        draw_outlined(pm, font, TEXT_PX, *colour, line, (x0, baseline), false);
    }
}

/// Canvas px at the bottom kept free of the map for the info line.
pub fn band_height(font: &FontVec) -> u32 {
    let scaled = font.as_scaled(PxScale::from(TEXT_PX));
    // +2: the 1 px outline above and below.
    ceil_px(scaled.height()) + 2 + 2 * PAD
}

/// Width in px of `text` set in `font` (advances plus kerning).
fn text_width(font: &FontVec, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(TEXT_PX));
    let mut width = 0.0;
    let mut prev: Option<GlyphId> = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            width += scaled.kern(p, id);
        }
        width += scaled.h_advance(id);
        prev = Some(id);
    }
    width
}

/// The line's text, with the area name cut and ended with `…` until it fits `max_width` px. If even
/// a bare `…` doesn't fit, that is returned (and clipped when drawn).
pub fn fit_text(font: &FontVec, line: &InfoLine, max_width: f32) -> String {
    let full = line.text();
    if text_width(font, &full) <= max_width {
        return full;
    }
    let chars: Vec<char> = line.area.chars().collect();
    let mut keep = chars.len();
    loop {
        let area: String = chars
            .iter()
            .take(keep)
            .collect::<String>()
            .trim_end()
            .chars()
            .chain([ELLIPSIS])
            .collect();
        let text = line.with_area(&area);
        if keep == 0 || text_width(font, &text) <= max_width {
            return text;
        }
        keep -= 1;
    }
}

/// Draws `line` bottom-left into `pm`, `inset` px above the bottom (room for the keep lines):
/// white text with a dark 1 px outline, a terrorized line's last part in violet. Draws nothing
/// if the canvas is smaller than the band.
pub fn draw(pm: &mut Pixmap, font: &FontVec, line: &InfoLine, inset: u32) {
    let w = pm.width();
    let Some(h) = pm.height().checked_sub(inset) else {
        return;
    };
    if h < band_height(font) {
        return;
    }
    let scaled = font.as_scaled(PxScale::from(TEXT_PX));
    // Left edge and baseline; +1 leaves room for the outline.
    let x0 = f32_px(PAD + 1);
    let max_width = f32_px(w.saturating_sub(2 * (PAD + 1)));
    let baseline = f32_px(h - PAD - 1) + scaled.descent();
    let text = fit_text(font, line, max_width);
    // Shortening only cuts the area name, so the fitted text still ends in the violet part.
    let (head, tail) = match text.strip_suffix(TERRORIZED) {
        Some(head) if line.terrorized => (head, TERRORIZED),
        _ => (text.as_str(), ""),
    };
    draw_outlined(pm, font, TEXT_PX, TEXT, head, (x0, baseline), false);
    if !tail.is_empty() {
        let x = x0 + text_width(font, head);
        draw_outlined(pm, font, TEXT_PX, TERROR_TEXT, tail, (x, baseline), false);
    }
}

/// Draws `c` at `size` px in `colour` with the info line's dark 1 px outline, its ink box centred
/// on (`cx`, `cy`), emboldened by 1 px. Used for the map's "?" markers.
pub fn draw_centred(pm: &mut Pixmap, font: &FontVec, size: f32, colour: [u8; 4], c: char, cx: f32, cy: f32) {
    let scaled = font.as_scaled(PxScale::from(size));
    let glyph = scaled
        .glyph_id(c)
        .with_scale_and_position(PxScale::from(size), point(0.0, 0.0));
    let Some(outlined) = scaled.outline_glyph(glyph) else {
        return;
    };
    let b = outlined.px_bounds();
    let pen = cx - f32::midpoint(b.min.x, b.max.x);
    let baseline = cy - f32::midpoint(b.min.y, b.max.y);
    draw_outlined(pm, font, size, colour, &c.to_string(), (pen, baseline), true);
}

/// Draws `text` from pen position `x0` on `baseline`: `colour` glyphs over a dark 1 px outline.
/// `bold` widens the glyphs by 1 px first (a faux bold; the outline grows with them).
fn draw_outlined(
    pm: &mut Pixmap,
    font: &FontVec,
    size: f32,
    colour: [u8; 4],
    text: &str,
    (x0, baseline): (f32, f32),
    bold: bool,
) {
    let (w, h) = (pm.width(), pm.height());
    let scaled = font.as_scaled(PxScale::from(size));
    // Glyph coverage goes into a mask; the outline is the mask grown by 1 px. tiny-skia then
    // blends a solid colour through each mask, which keeps premultiplied alpha right.
    let (Some(mut glyphs), Some(mut outline)) = (Mask::new(w, h), Mask::new(w, h)) else {
        return;
    };
    let mut pen = x0;
    let mut prev: Option<GlyphId> = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            pen += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(PxScale::from(size), point(pen, baseline));
        pen += scaled.h_advance(id);
        prev = Some(id);
        let Some(outlined) = scaled.outline_glyph(glyph) else {
            continue; // a space
        };
        let min = outlined.px_bounds().min;
        let (gx, gy) = (round_px(min.x), round_px(min.y));
        let data = glyphs.data_mut();
        outlined.draw(|x, y, coverage| {
            if let Some(i) = mask_index(w, h, gx + i64::from(x), gy + i64::from(y))
                && let Some(px) = data.get_mut(i)
            {
                // Glyphs may overlap by a pixel: keep the stronger coverage.
                *px = (*px).max(coverage_u8(coverage));
            }
        });
    }
    if bold {
        widen(glyphs.data_mut(), w);
    }
    grow(glyphs.data(), outline.data_mut(), w, h);

    let Some(rect) = Rect::from_xywh(0.0, 0.0, f32_px(w), f32_px(h)) else {
        return;
    };
    for (colour, mask) in [(OUTLINE, &outline), (colour, &glyphs)] {
        let mut paint = Paint::default();
        let [red, green, blue, alpha] = colour;
        paint.set_color_rgba8(red, green, blue, alpha);
        pm.fill_rect(rect, &paint, Transform::identity(), Some(mask));
    }
}

/// Faux bold: each pixel of the `w`-wide mask `data` takes the max of itself and its left
/// neighbour. Horizontal only, so gaps between a glyph's parts above each other (the dot of "?")
/// stay open.
fn widen(data: &mut [u8], w: u32) {
    let Ok(w) = usize::try_from(w) else { return };
    if w == 0 {
        return;
    }
    for row in data.chunks_mut(w) {
        // Right to left, so each pixel reads its left neighbour's original value.
        for x in (1..row.len()).rev() {
            let left = row.get(x - 1).copied().unwrap_or(0);
            if let Some(cur) = row.get_mut(x) {
                *cur = (*cur).max(left);
            }
        }
    }
}

/// `dst` = `src` dilated by 1 px (each pixel the max of its 3×3 neighbourhood).
fn grow(src: &[u8], dst: &mut [u8], w: u32, h: u32) {
    for y in 0..i64::from(h) {
        for x in 0..i64::from(w) {
            let mut v = 0u8;
            for (dx, dy) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (-1, 0),
                (0, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                if let Some(s) = mask_index(w, h, x + dx, y + dy).and_then(|i| src.get(i)) {
                    v = v.max(*s);
                }
            }
            if let Some(d) = mask_index(w, h, x, y).and_then(|i| dst.get_mut(i)) {
                *d = v;
            }
        }
    }
}

/// Index of pixel (`x`, `y`) in a `w`×`h` mask; `None` outside it.
fn mask_index(w: u32, h: u32, x: i64, y: i64) -> Option<usize> {
    let (x, y) = (u32::try_from(x).ok()?, u32::try_from(y).ok()?);
    if x >= w || y >= h {
        return None;
    }
    usize::try_from(u64::from(y) * u64::from(w) + u64::from(x)).ok()
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "coverage is clamped to 0..=1 first, so the product fits a u8"
)]
fn coverage_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "glyph pixel bounds are small canvas coordinates"
)]
const fn round_px(v: f32) -> i64 {
    v.round() as i64
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "font line heights are a few dozen px"
)]
const fn ceil_px(v: f32) -> u32 {
    v.max(0.0).ceil() as u32
}

#[allow(
    clippy::cast_precision_loss,
    reason = "canvas sizes are at most 8192, exact in f32"
)]
const fn f32_px(v: u32) -> f32 {
    v as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_table_has_flayer_dungeon() {
        assert!(
            LEVELS.contains(&(88, "Flayer Dungeon Level 1", [22, 51, 81])),
            "generated table changed"
        );
    }

    #[test]
    fn difficulty_names() {
        assert_eq!(difficulty_name(0), Some("Normal"));
        assert_eq!(difficulty_name(1), Some("Nightmare"));
        assert_eq!(difficulty_name(2), Some("Hell"));
        assert_eq!(difficulty_name(3), None);
    }

    #[test]
    fn line_with_elite_offsets() {
        assert_eq!(
            info_line(88, Some(0), false, None).text(),
            "Normal · Flayer Dungeon Level 1 · mlvl 22 · champ 24 · unique 25"
        );
        assert_eq!(
            info_line(88, Some(1), false, None).text(),
            "Nightmare · Flayer Dungeon Level 1 · mlvl 51 · champ 53 · unique 54"
        );
        assert_eq!(
            info_line(88, Some(2), false, None).text(),
            "Hell · Flayer Dungeon Level 1 · mlvl 81 · champ 83 · unique 84"
        );
    }

    #[test]
    fn line_with_boss() {
        assert_eq!(
            info_line(102, Some(0), false, None).text(),
            "Normal · Durance of Hate Level 3 · mlvl 25 · champ 27 · unique 28 · Mephisto 26"
        );
        assert_eq!(
            info_line(102, Some(2), false, None)
                .stats
                .last()
                .map(String::as_str),
            Some("Mephisto 87")
        );
    }

    #[test]
    fn unknown_level_has_no_monster_levels() {
        let line = info_line(9999, Some(1), false, None);
        assert_eq!(line.text(), "Nightmare · Level 9999");
    }

    #[test]
    fn unknown_difficulty_is_left_out() {
        for d in [None, Some(3), Some(200)] {
            assert_eq!(
                info_line(88, d, false, None).text(),
                "Flayer Dungeon Level 1",
                "{d:?}"
            );
        }
        // Mephisto's level depends on the difficulty too.
        assert_eq!(info_line(102, None, false, None).stats, Vec::<String>::new());
    }

    #[test]
    fn long_area_names_are_shortened() {
        let Some(font) = font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        let line = info_line(88, Some(0), false, None);
        let full = line.text();
        assert_eq!(fit_text(font, &line, 10_000.0), full);
        let narrow = text_width(font, &full) - 30.0;
        let short = fit_text(font, &line, narrow);
        assert!(text_width(font, &short) <= narrow, "{short}");
        assert!(short.starts_with("Normal · Flayer"), "{short}");
        assert!(short.contains("… · mlvl 22 · champ 24 · unique 25"), "{short}");
        // Too narrow for anything: the area is cut to a bare ellipsis.
        assert_eq!(
            fit_text(font, &line, 5.0),
            "Normal · … · mlvl 22 · champ 24 · unique 25"
        );
    }

    fn keep(name: &str, tier: Tier, count: u32) -> Keep {
        Keep {
            name: name.into(),
            tier,
            category: crate::valuables::Category::Trade,
            count,
        }
    }

    #[test]
    fn keep_entries() {
        assert_eq!(
            keep_entry(&keep("Harlequin Crest", Tier::S, 1)),
            "Harlequin Crest (S)"
        );
        assert_eq!(keep_entry(&keep("Lem Rune", Tier::A, 2)), "Lem Rune ×2 (A)");
    }

    #[test]
    fn keep_lines_wrap_and_cap() {
        let entries: Vec<String> = [
            "aaaa", "bbbb", "cccc", "dddd", "eeee", "ffff", "gggg", "hhhh", "iiii", "jjjj",
        ]
        .map(str::to_owned)
        .to_vec();
        // Width by characters: 20 per line.
        let fits = |t: &str| t.chars().count() <= 20;
        assert_eq!(wrap_keep(&[], fits), Vec::<String>::new());
        assert_eq!(
            wrap_keep(entries.get(..2).unwrap_or_default(), fits),
            vec!["Keep: aaaa · bbbb"]
        );
        assert_eq!(
            wrap_keep(entries.get(..5).unwrap_or_default(), fits),
            vec!["Keep: aaaa · bbbb", "cccc · dddd · eeee"]
        );
        // Ten entries need four lines: three, the last ending in "+N more" (two entries make
        // room for it).
        let capped = wrap_keep(&entries, fits);
        assert_eq!(
            capped,
            vec!["Keep: aaaa · bbbb", "cccc · dddd · eeee", "ffff · +4 more"]
        );
        assert!(capped.iter().all(|l| fits(l)), "{capped:?}");
        // An entry wider than a line still gets one.
        let long = vec!["x".repeat(30), "y".into()];
        assert_eq!(wrap_keep(&long, fits).len(), 2);
    }

    #[test]
    fn spare_entries_and_lines() {
        use crate::valuables::Place;
        let spare = |name: &str, place, count| Spare {
            name: name.into(),
            place,
            count,
        };
        assert_eq!(
            spare_entry(&spare("Gravenspine", Place::Inventory, 1)),
            "Gravenspine (inv)"
        );
        assert_eq!(
            spare_entry(&spare("Magefist", Place::Stash, 2)),
            "Magefist ×2 (stash)"
        );
        assert_eq!(spare_entry(&spare("a", Place::Cube, 1)), "a (cube)");
        assert_eq!(spare_entry(&spare("a", Place::Shared, 1)), "a (shared)");
        let entries: Vec<String> = (0..12).map(|i| format!("i{i:02}")).collect();
        let fits = |t: &str| t.chars().count() <= 20;
        let lines = wrap(SELL_PREFIX, COMMA, &entries, fits);
        assert_eq!(
            lines,
            vec!["Sell: i00, i01, i02", "i03, i04, i05, i06", "i07, i08, +3 more"]
        );
    }

    #[test]
    fn stack_orders_colours_and_gaps() {
        let l = |s: &str| vec![s.to_owned()];
        let rows = stack(
            l("Keep: k"),
            l("Sell: s"),
            vec!["Give away: g".into(), "h".into()],
        );
        let want: Vec<(String, [u8; 4])> = vec![
            (String::new(), KEEP_TEXT),
            ("Keep: k".into(), KEEP_TEXT),
            (String::new(), SELL_TEXT),
            ("Sell: s".into(), SELL_TEXT),
            (String::new(), GIVE_TEXT),
            ("Give away: g".into(), GIVE_TEXT),
            ("h".into(), GIVE_TEXT),
        ];
        assert_eq!(rows, want);
        assert_eq!(
            stack(vec![], vec![], l("Give away: g")),
            vec![(String::new(), GIVE_TEXT), ("Give away: g".to_owned(), GIVE_TEXT)],
            "a gap below the info line even for the only block"
        );
        assert_eq!(stack(vec![], vec![], vec![]), vec![]);
    }

    #[test]
    fn keep_band_heights() {
        let Some(font) = font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        let lh = line_height(font);
        assert!(lh > 10, "{lh}");
        assert_eq!(keep_band_height(font, 0), 0, "no lines, no band");
        assert_eq!(keep_band_height(font, 1), lh);
        assert_eq!(keep_band_height(font, 3), 3 * lh);
        assert_eq!(
            keep_band_height(font, 7),
            7 * lh,
            "keep, sell, give away and gaps"
        );
    }

    #[test]
    fn terrorized_line_drops_table_levels() {
        // Moo Moo Farm (39).
        let line = info_line(39, Some(0), true, None);
        assert_eq!(line.text(), "Normal · Moo Moo Farm · terrorized");
        assert!(line.terrorized);
        assert!(
            info_line(39, Some(0), false, None).text().contains("mlvl"),
            "not terrorized: table values"
        );
        // No boss level either (Mephisto's level is a table value too).
        assert_eq!(
            info_line(102, Some(2), true, None).text(),
            "Hell · Durance of Hate Level 3 · terrorized"
        );
        assert_eq!(
            info_line(39, None, true, None).text(),
            "Moo Moo Farm · terrorized"
        );
    }

    #[test]
    fn zero_mlvl_area_has_no_monster_levels() {
        // Rogue Encampment: monster level 0 in the table.
        assert_eq!(
            info_line(1, Some(0), false, None).text(),
            "Normal · Rogue Encampment"
        );
    }

    #[test]
    fn the_cow_counter_replaces_the_monster_levels_in_level_39_only() {
        let cows = Some(Tally { kills: 57, alive: 23 });
        assert_eq!(
            info_line(39, Some(0), false, cows).text(),
            "Normal · Moo Moo Farm · kill count: 57 / victims alive: 23"
        );
        let terror = info_line(39, Some(2), true, cows);
        assert_eq!(
            terror.text(),
            "Hell · Moo Moo Farm · kill count: 57 / victims alive: 23 · terrorized"
        );
        assert!(terror.terrorized, "still ends in the violet part");
        // Elsewhere, or without a counter, the usual line.
        assert_eq!(
            info_line(88, Some(0), false, cows),
            info_line(88, Some(0), false, None)
        );
        assert!(info_line(39, Some(0), false, None).text().contains("mlvl"));
    }
}
