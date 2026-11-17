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
//!
//! From Nightmare on, the monster-level part (or the cow counter) is coloured by how the
//! character's build suits the area ([`crate::build::rate`]): red when bad, green when good.

use std::collections::HashSet;
use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont, point};
use tiny_skia::{Mask, Paint, Pixmap, Rect, Transform};

use crate::build::{self, Damage, Rating};
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
/// The level-suggestion line(s) between the info line and the keep lines: light blue.
const LEVEL_TEXT: [u8; 4] = [120, 210, 255, 255];
/// Most level-suggestion lines.
const MAX_LEVEL_LINES: usize = 2;
/// From this character level on no areas are suggested (everywhere is fine).
pub const SUGGEST_UNTIL: u8 = 75;
/// The suggested areas' monster level is at most this many above the character level (the top of
/// the band; such areas rank last — a same-level area's champions are already +2 and its uniques
/// +3, which is the efficient XP zone for melee).
const SUGGEST_BONUS: u8 = 2;
/// The suggested areas' monster level is at least this many below the character level (monsters
/// more than this many levels below give heavily penalized experience).
const SUGGEST_FLOOR: u8 = 5;
/// Most suggested areas.
const MAX_SUGGESTIONS: usize = 5;
/// Town level ids: never suggested.
pub const TOWNS: [u32; 5] = [1, 40, 75, 103, 109];
/// Level ids from here on are special or uber levels (Pandemonium, Uber Tristram, …): never
/// suggested.
const FIRST_SPECIAL_LEVEL: u32 = 133;
/// Separator of the sell and give-away entries.
const COMMA: &str = ", ";
/// The last part of a terrorized level's line, drawn in [`TERROR_TEXT`].
const TERRORIZED: &str = "terrorized";
const TERROR_TEXT: [u8; 4] = [190, 120, 255, 255];
const OUTLINE: [u8; 4] = [0, 0, 0, 220];
/// The monster-level part in an area bad for the build ([`Rating::Bad`]).
const BAD_TEXT: [u8; 4] = [235, 70, 60, 255];
/// The monster-level part in an area good for the build ([`Rating::Good`]).
const GOOD_TEXT: [u8; 4] = [80, 220, 100, 255];

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
    /// How the area suits the build: colours the stats before a [`TERRORIZED`] part.
    pub rating: Rating,
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
/// Moo Farm, `cows` replaces them (a terrorized farm keeps [`TERRORIZED`] last). `build` (primary
/// damage first) rates the area ([`build::rate`]).
pub fn info_line(
    level_id: u32,
    difficulty: Option<u8>,
    terrorized: bool,
    cows: Option<Tally>,
    build: &[Damage],
) -> InfoLine {
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
        rating: build::rate(level_id, difficulty, build),
    }
}

/// Areas to level in for a character of `char_level` in `difficulty`: up to [`MAX_SUGGESTIONS`]
/// areas (name, monster level) whose monster level is in the realistic range — at most
/// [`SUGGEST_BONUS`] above and at most [`SUGGEST_FLOOR`] below the character level — ranked by
/// efficiency: the character level itself first (its champions are +2 and its uniques +3), then
/// downwards to [`SUGGEST_FLOOR`] below, the above-level areas last and mildest first, ties in
/// level-id order; if none is in the range, the nearest ones (a monster level above the target
/// wins a tie). Towns, the cow level, special levels and areas without monster levels are never
/// suggested. Empty for an unknown level or difficulty, and from level [`SUGGEST_UNTIL`] on.
pub fn level_suggestions(char_level: Option<u8>, difficulty: Option<u8>) -> Vec<(&'static str, u8)> {
    let (Some(lvl), Some(d)) = (char_level.filter(|&l| l < SUGGEST_UNTIL), difficulty) else {
        return Vec::new();
    };
    let target = lvl + SUGGEST_BONUS;
    let floor = lvl.saturating_sub(SUGGEST_FLOOR);
    let mut areas: Vec<(&'static str, u8)> = LEVELS
        .iter()
        .filter(|&&(id, _, _)| !TOWNS.contains(&id) && id != COW_LEVEL && id < FIRST_SPECIAL_LEVEL)
        .filter_map(|&(_, name, lvls)| Some((name, *lvls.get(usize::from(d))?)))
        .filter(|&(_, mlvl)| mlvl > 0)
        .collect();
    // The seven Tal Rasha's Tombs share a name and level; suggest that pair once. Same-named
    // areas with different levels (Act 2 and Act 3 Sewers) stay separate.
    let mut seen = HashSet::new();
    areas.retain(|&(name, mlvl)| seen.insert((name, mlvl)));
    let mut in_range: Vec<(&'static str, u8)> = areas
        .iter()
        .copied()
        .filter(|&(_, mlvl)| floor <= mlvl && mlvl <= target)
        .collect();
    if !in_range.is_empty() {
        // Stable sort: equal ranks keep level-id order. Below the character level, the closest
        // first; above it, the mildest first.
        in_range.sort_by_key(|&(_, mlvl)| (mlvl > lvl, mlvl.abs_diff(lvl)));
        in_range.truncate(MAX_SUGGESTIONS);
        return in_range;
    }
    // Stable sort: equally near areas stay in level-id order.
    areas.sort_by_key(|&(_, mlvl)| (mlvl.abs_diff(target), mlvl < target));
    areas.truncate(MAX_SUGGESTIONS);
    areas
}

/// The level suggestions as lines of at most `max_width` px in `font`:
/// `Level 34 → Halls of Death's Calling (34), Halls of Anguish (33)`, at most
/// [`MAX_LEVEL_LINES`] (the last then ends in `+N more`). Empty without suggestions.
pub fn level_lines(
    font: &FontVec,
    char_level: Option<u8>,
    difficulty: Option<u8>,
    max_width: f32,
) -> Vec<String> {
    let (Some(lvl), areas) = (char_level, level_suggestions(char_level, difficulty)) else {
        return Vec::new();
    };
    let entries: Vec<String> = areas
        .iter()
        .map(|(name, mlvl)| format!("{name} ({mlvl})"))
        .collect();
    wrap(
        &format!("Level {lvl} → "),
        COMMA,
        &entries,
        MAX_LEVEL_LINES,
        |t| text_width(font, t) <= max_width,
    )
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
    wrap(prefix, COMMA, &entries, MAX_KEEP_LINES, |t| {
        text_width(font, t) <= max_width
    })
}

/// The text rows below the info line, top to bottom, each with its colour: the level
/// suggestions (light blue), the keep lines (green), the sell lines (grey), the give-away lines
/// (yellow), with an empty gap row before each non-empty block (so also between the info line
/// and the first block).
pub fn stack(
    level: Vec<String>,
    keep: Vec<String>,
    sell: Vec<String>,
    give: Vec<String>,
) -> Vec<(String, [u8; 4])> {
    let mut rows = Vec::new();
    for (lines, colour) in [
        (level, LEVEL_TEXT),
        (keep, KEEP_TEXT),
        (sell, SELL_TEXT),
        (give, GIVE_TEXT),
    ] {
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
    wrap(KEEP_PREFIX, SEPARATOR, entries, MAX_KEEP_LINES, fits)
}

/// [`wrap_keep`] with any `prefix` before the first line and `sep` between entries; `max_lines`
/// caps the lines (the last then ends in `+N more`).
fn wrap(
    prefix: &str,
    sep: &str,
    entries: &[String],
    max_lines: usize,
    fits: impl Fn(&str) -> bool,
) -> Vec<String> {
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
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        let placed: usize = lines.iter().map(Vec::len).sum();
        let mut rest = entries.len().saturating_sub(placed);
        if let Some(last) = lines.last_mut() {
            loop {
                last.push(format!("+{rest} more"));
                if last.len() == 1 || fits(&render(max_lines - 1, last)) {
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
    let mut x = x0;
    for (part, colour) in spans(line, &text) {
        draw_outlined(pm, font, TEXT_PX, colour, part, (x, baseline), false);
        x += text_width(font, part);
    }
}

/// The fitted `text` of `line` ([`fit_text`]) as coloured parts, left to right: white, then the
/// stats in the rating's colour (red or green; white when neutral), then a terrorized line's
/// [`TERRORIZED`] in violet. Empty parts are left out.
fn spans<'a>(line: &InfoLine, text: &'a str) -> Vec<(&'a str, [u8; 4])> {
    // Shortening only cuts the area name, so the fitted text still ends in the stats.
    let (rest, terror) = match text.strip_suffix(TERRORIZED) {
        Some(rest) if line.terrorized => (rest, TERRORIZED),
        _ => (text, ""),
    };
    let rated_parts = line.stats.len().saturating_sub(usize::from(line.terrorized));
    let rated = line.stats.get(..rated_parts).unwrap_or_default().join(SEPARATOR);
    // Before a terrorized part, the stats are followed by the separator, which stays white.
    let (rated_end, sep) = match rest.strip_suffix(SEPARATOR) {
        Some(end) if !terror.is_empty() => (end, SEPARATOR),
        _ => (rest, ""),
    };
    let colour = match line.rating {
        Rating::Bad => BAD_TEXT,
        Rating::Good => GOOD_TEXT,
        Rating::Neutral => TEXT,
    };
    let (head, rated, sep) = match rated_end.strip_suffix(rated.as_str()) {
        Some(head) if !rated.is_empty() && line.rating != Rating::Neutral => {
            (head, rated_end.get(head.len()..).unwrap_or_default(), sep)
        }
        _ => (rest, "", ""),
    };
    [(head, TEXT), (rated, colour), (sep, TEXT), (terror, TERROR_TEXT)]
        .into_iter()
        .filter(|(part, _)| !part.is_empty())
        .collect()
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
            info_line(88, Some(0), false, None, &[]).text(),
            "Normal · Flayer Dungeon Level 1 · mlvl 22 · champ 24 · unique 25"
        );
        assert_eq!(
            info_line(88, Some(1), false, None, &[]).text(),
            "Nightmare · Flayer Dungeon Level 1 · mlvl 51 · champ 53 · unique 54"
        );
        assert_eq!(
            info_line(88, Some(2), false, None, &[]).text(),
            "Hell · Flayer Dungeon Level 1 · mlvl 81 · champ 83 · unique 84"
        );
    }

    #[test]
    fn line_with_boss() {
        assert_eq!(
            info_line(102, Some(0), false, None, &[]).text(),
            "Normal · Durance of Hate Level 3 · mlvl 25 · champ 27 · unique 28 · Mephisto 26"
        );
        assert_eq!(
            info_line(102, Some(2), false, None, &[])
                .stats
                .last()
                .map(String::as_str),
            Some("Mephisto 87")
        );
    }

    #[test]
    fn unknown_level_has_no_monster_levels() {
        let line = info_line(9999, Some(1), false, None, &[]);
        assert_eq!(line.text(), "Nightmare · Level 9999");
    }

    #[test]
    fn unknown_difficulty_is_left_out() {
        for d in [None, Some(3), Some(200)] {
            assert_eq!(
                info_line(88, d, false, None, &[]).text(),
                "Flayer Dungeon Level 1",
                "{d:?}"
            );
        }
        // Mephisto's level depends on the difficulty too.
        assert_eq!(info_line(102, None, false, None, &[]).stats, Vec::<String>::new());
    }

    #[test]
    fn long_area_names_are_shortened() {
        let Some(font) = font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        let line = info_line(88, Some(0), false, None, &[]);
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
        let lines = wrap(SELL_PREFIX, COMMA, &entries, MAX_KEEP_LINES, fits);
        assert_eq!(
            lines,
            vec!["Sell: i00, i01, i02", "i03, i04, i05, i06", "i07, i08, +3 more"]
        );
    }

    #[test]
    fn stack_orders_colours_and_gaps() {
        let l = |s: &str| vec![s.to_owned()];
        let rows = stack(
            l("Level 3 → a (5)"),
            l("Keep: k"),
            l("Sell: s"),
            vec!["Give away: g".into(), "h".into()],
        );
        let want: Vec<(String, [u8; 4])> = vec![
            (String::new(), LEVEL_TEXT),
            ("Level 3 → a (5)".into(), LEVEL_TEXT),
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
            stack(vec![], vec![], vec![], l("Give away: g")),
            vec![(String::new(), GIVE_TEXT), ("Give away: g".to_owned(), GIVE_TEXT)],
            "a gap below the info line even for the only block"
        );
        assert_eq!(stack(vec![], vec![], vec![], vec![]), vec![]);
    }

    #[test]
    fn level_suggestions_prefer_own_level_and_below() {
        // Normal 1: the character level itself first (its champions are +2, its uniques +3 —
        // the efficient XP zone for melee), then downwards; the above-level areas (mlvl 3)
        // would only fill leftover slots, and at most `MAX_SUGGESTIONS` areas are returned.
        assert_eq!(
            level_suggestions(Some(1), Some(0)),
            vec![
                ("Blood Moor", 1),
                ("Den of Evil", 1),
                ("Cold Plains", 2),
                ("Cave Level 1", 2),
                ("Cave Level 2", 2)
            ]
        );
        // Normal 34: mlvl 34 first, then 33, 32, and the mlvl 29 areas in id order; the mlvl 36
        // Halls of Vaught is 2 above and ranks last (cut here by the cap).
        assert_eq!(
            level_suggestions(Some(34), Some(0)),
            vec![
                ("Halls of Death's Calling", 34),
                ("Halls of Anguish", 33),
                ("Nihlathaks Temple", 32),
                ("Crystalized Cavern Level 1", 29),
                ("Cellar of Pity", 29)
            ]
        );
    }

    #[test]
    fn level_suggestions_band_caps_above_and_skips_the_cow_level() {
        // Normal 33: the band is 28–35; nothing is at mlvl 33 except Halls of Anguish, then 32,
        // then the mlvl 29 areas; the mlvl 34 Halls of Death's Calling is above the character
        // level and the mlvl 36 Vaught is out of band, so neither shows.
        assert_eq!(
            level_suggestions(Some(33), Some(0)),
            vec![
                ("Halls of Anguish", 33),
                ("Nihlathaks Temple", 32),
                ("Crystalized Cavern Level 1", 29),
                ("Cellar of Pity", 29),
                ("Crystalized Cavern Level 2", 29)
            ]
        );
        let areas = level_suggestions(Some(33), Some(0));
        assert!(!areas.iter().any(|&(name, _)| name == "Moo Moo Farm"));
        // Normal 18: the band is 13–20; nothing at mlvl 19/20, so 18 first, then the mlvl 17
        // areas in id order (the seven Tal Rasha's Tombs deduplicated to one entry).
        assert_eq!(
            level_suggestions(Some(18), Some(0)),
            vec![
                ("Valley of Snakes", 18),
                ("Lost City", 17),
                ("Maggot Lair Level 1", 17),
                ("Maggot Lair Level 2", 17),
                ("Maggot Lair Level 3", 17)
            ]
        );
        // Same-named areas with different levels are not collapsed: the Sewers Level 1 at
        // Normal clvl 23 is Act 3's (mlvl 23) — under a name-only dedupe the Act 2 entry
        // (mlvl 13, out of band) would have survived and the area would never be suggested.
        assert_eq!(
            level_suggestions(Some(23), Some(0)),
            vec![
                ("Upper Kurast", 23),
                ("Sewers Level 1", 23),
                ("Ruined Temple", 23),
                ("Disused Fane", 23),
                ("Forgotten Reliquary", 23)
            ]
        );
    }

    #[test]
    fn level_suggestions_use_the_difficulty_and_stop_at_75() {
        // Nightmare 46: the mlvl 46 areas first (own level), then 45; the mlvl 48 areas are 2
        // above and rank last (cut here by the cap).
        assert_eq!(
            level_suggestions(Some(46), Some(1)),
            vec![
                ("Lost City", 46),
                ("Valley of Snakes", 46),
                ("Maggot Lair Level 3", 46),
                ("Ancient Tunnels", 46),
                ("Far Oasis", 45)
            ]
        );
        // Hell 74 → 76: the last level with suggestions; the band is 69–76 and the mlvl 74
        // areas come first (the Act 2 and Act 3 Sewers share name and level, so one entry).
        assert_eq!(
            level_suggestions(Some(74), Some(2)),
            vec![
                ("Sewers Level 1", 74),
                ("Sewers Level 2", 74),
                ("Catacombs Level 2", 73),
                ("Catacombs Level 3", 73),
                ("Catacombs Level 4", 73)
            ]
        );
        assert!(
            level_suggestions(Some(75), Some(2)).is_empty(),
            "75: everywhere is fine"
        );
        assert_eq!(level_suggestions(Some(90), Some(0)), Vec::<(&str, u8)>::new());
        assert_eq!(
            level_suggestions(None, Some(0)),
            Vec::<(&str, u8)>::new(),
            "unknown level"
        );
        assert_eq!(
            level_suggestions(Some(10), None),
            Vec::<(&str, u8)>::new(),
            "unknown difficulty"
        );
        assert_eq!(
            level_suggestions(Some(10), Some(3)),
            Vec::<(&str, u8)>::new(),
            "bad difficulty"
        );
    }

    #[test]
    fn level_suggestions_skip_towns_and_special_levels() {
        for d in 0..3 {
            for lvl in 1..SUGGEST_UNTIL {
                for (name, mlvl) in level_suggestions(Some(lvl), Some(d)) {
                    let &(id, _, _) = LEVELS
                        .iter()
                        .find(|&&(_, n, lvls)| n == name && lvls.get(usize::from(d)) == Some(&mlvl))
                        .unwrap_or(&(0, "", [0; 3]));
                    assert!(
                        mlvl > 0 && !TOWNS.contains(&id) && id < FIRST_SPECIAL_LEVEL,
                        "{name}"
                    );
                }
            }
        }
    }

    #[test]
    fn level_lines_text_and_cap() {
        let Some(font) = font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        assert_eq!(
            level_lines(font, Some(1), Some(0), 1000.0),
            vec![
                "Level 1 → Blood Moor (1), Den of Evil (1), Cold Plains (2), Cave Level 1 (2), Cave Level 2 (2)"
            ]
        );
        // At a tiny width every area gets its own line and the cap cuts the rest, marked with
        // a +N more tail.
        assert_eq!(
            level_lines(font, Some(1), Some(0), 10.0),
            vec!["Level 1 → Blood Moor (1)", "+4 more"],
            "wraps and marks the cut with +N more"
        );
        assert_eq!(level_lines(font, Some(80), Some(0), 1000.0), Vec::<String>::new());
        assert_eq!(level_lines(font, None, Some(0), 1000.0), Vec::<String>::new());
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
        let line = info_line(39, Some(0), true, None, &[]);
        assert_eq!(line.text(), "Normal · Moo Moo Farm · terrorized");
        assert!(line.terrorized);
        assert!(
            info_line(39, Some(0), false, None, &[]).text().contains("mlvl"),
            "not terrorized: table values"
        );
        // No boss level either (Mephisto's level is a table value too).
        assert_eq!(
            info_line(102, Some(2), true, None, &[]).text(),
            "Hell · Durance of Hate Level 3 · terrorized"
        );
        assert_eq!(
            info_line(39, None, true, None, &[]).text(),
            "Moo Moo Farm · terrorized"
        );
    }

    #[test]
    fn zero_mlvl_area_has_no_monster_levels() {
        // Rogue Encampment: monster level 0 in the table.
        assert_eq!(
            info_line(1, Some(0), false, None, &[]).text(),
            "Normal · Rogue Encampment"
        );
    }

    #[test]
    fn the_cow_counter_replaces_the_monster_levels_in_level_39_only() {
        let cows = Some(Tally { kills: 57, alive: 23 });
        assert_eq!(
            info_line(39, Some(0), false, cows, &[]).text(),
            "Normal · Moo Moo Farm · kill count: 57 / victims alive: 23"
        );
        let terror = info_line(39, Some(2), true, cows, &[]);
        assert_eq!(
            terror.text(),
            "Hell · Moo Moo Farm · kill count: 57 / victims alive: 23 · terrorized"
        );
        assert!(terror.terrorized, "still ends in the violet part");
        // Elsewhere, or without a counter, the usual line.
        assert_eq!(
            info_line(88, Some(0), false, cows, &[]),
            info_line(88, Some(0), false, None, &[])
        );
        assert!(info_line(39, Some(0), false, None, &[]).text().contains("mlvl"));
    }

    const VEX: [Damage; 3] = [Damage::Lightning, Damage::Fire, Damage::Physical];

    #[test]
    fn rating_colours_only_the_stats() {
        // Travincal, Nightmare: no immunes, good for Vex.
        let line = info_line(83, Some(1), false, None, &VEX);
        assert_eq!(line.rating, Rating::Good);
        let text = line.text();
        assert_eq!(
            spans(&line, &text),
            vec![
                ("Nightmare · Travincal · ", TEXT),
                ("mlvl 54 · champ 56 · unique 57", GOOD_TEXT)
            ]
        );
        // Hell City of the Damned: 4 of 7 types lightning immune.
        let line = info_line(106, Some(2), false, None, &VEX);
        let text = line.text();
        assert_eq!(
            spans(&line, &text).last(),
            Some(&("mlvl 84 · champ 86 · unique 87", BAD_TEXT))
        );
        // Normal, or no build: one white part.
        for line in [
            info_line(106, Some(0), false, None, &VEX),
            info_line(106, Some(2), false, None, &[]),
        ] {
            let text = line.text();
            assert_eq!(line.rating, Rating::Neutral);
            assert_eq!(spans(&line, &text), vec![(text.as_str(), TEXT)]);
        }
        // Mephisto's level is part of the coloured stats.
        let line = info_line(102, Some(1), false, None, &VEX);
        let text = line.text();
        assert!(
            spans(&line, &text)
                .last()
                .is_some_and(|&(part, c)| part.ends_with("Mephisto 59") && c != TEXT),
            "{:?}",
            spans(&line, &text)
        );
    }

    #[test]
    fn rating_keeps_terrorized_violet_and_colours_the_cow_counter() {
        let build = [Damage::Cold];
        let cows = Some(Tally { kills: 5, alive: 9 });
        let line = info_line(39, Some(1), true, cows, &build);
        assert_ne!(line.rating, Rating::Neutral, "Moo Moo Farm in Nightmare is rated");
        let colour = if line.rating == Rating::Bad {
            BAD_TEXT
        } else {
            GOOD_TEXT
        };
        let text = line.text();
        assert_eq!(
            spans(&line, &text),
            vec![
                ("Nightmare · Moo Moo Farm · ", TEXT),
                ("kill count: 5 / victims alive: 9", colour),
                (" · ", TEXT),
                (TERRORIZED, TERROR_TEXT)
            ]
        );
        // Terrorized without the counter: nothing to rate, only the violet part.
        let line = info_line(83, Some(1), true, None, &VEX);
        let text = line.text();
        assert_eq!(
            spans(&line, &text),
            vec![("Nightmare · Travincal · ", TEXT), (TERRORIZED, TERROR_TEXT)]
        );
    }

    #[test]
    fn rating_survives_a_shortened_area() {
        let Some(font) = font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        let line = info_line(83, Some(1), false, None, &VEX);
        let text = fit_text(font, &line, 200.0);
        assert!(text.contains('…'), "{text}");
        assert_eq!(
            spans(&line, &text).last(),
            Some(&("mlvl 54 · champ 56 · unique 57", GOOD_TEXT))
        );
    }

    #[test]
    fn draws_the_rated_part_in_its_colour() {
        let Some(font) = font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        let count = |line: &InfoLine, rgb: [u8; 3]| -> usize {
            let Some(mut pm) = Pixmap::new(600, 40) else {
                return 0;
            };
            draw(&mut pm, font, line, 0);
            pm.pixels()
                .iter()
                .filter(|p| {
                    let c = p.demultiply();
                    // Blending through the mask may be off by one per channel.
                    c.alpha() == 255
                        && [c.red(), c.green(), c.blue()]
                            .iter()
                            .zip(rgb)
                            .all(|(&v, want)| v.abs_diff(want) <= 2)
                })
                .count()
        };
        let [r, g, b, _] = GOOD_TEXT;
        let good = info_line(83, Some(1), false, None, &VEX);
        // Only fully covered glyph pixels count: a few dozen at 16 px.
        assert!(count(&good, [r, g, b]) > 10, "green glyph pixels");
        assert!(count(&good, [255, 255, 255]) > 10, "white area name");
        let neutral = info_line(83, Some(0), false, None, &VEX);
        assert_eq!(count(&neutral, [r, g, b]), 0, "Normal: no green");
        let [r, g, b, _] = BAD_TEXT;
        assert!(
            count(&info_line(106, Some(2), false, None, &VEX), [r, g, b]) > 10,
            "red"
        );
    }
}
