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

use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont, point};
use tiny_skia::{Mask, Paint, Pixmap, Rect, Transform};

use crate::build::Rating;
use crate::cows::{COW_LEVEL, Tally};
use crate::guidance::{Action, Advice};
use crate::levels_data::{BOSSES, LEVELS};
use crate::names::same_name;

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
/// The keep-for-own-use lines: green.
pub const KEEP_TEXT: [u8; 4] = [90, 230, 90, 255];
/// The sell lines: grey (dashboard Sell panel).
pub const SELL_TEXT: [u8; 4] = [175, 175, 175, 255];
/// The give-away lines: yellow (dashboard, riding with the Sell panel).
pub const GIVE_TEXT: [u8; 4] = [255, 225, 60, 255];
/// The move lines (a keeper on the wrong stash pages): orange.
pub const MOVE_TEXT: [u8; 4] = [255, 160, 60, 255];
/// The keep-for-trade lines: light blue (the own keep lines are [`KEEP_TEXT`]).
pub const TRADE_TEXT: [u8; 4] = [110, 190, 255, 255];
/// The breakpoint/gear/runeword guide lines (user request, 2026-11-17): light violet.
pub const GUIDE_TEXT: [u8; 4] = [200, 130, 255, 255];
/// The `BiS` chase list ([`bis_lines`]): light gold.
pub const BIS_TEXT: [u8; 4] = [255, 210, 120, 255];
/// Most `BiS` chase lines per section (per-slot lines; hard truncation beyond it).
const MAX_BIS_LINES: usize = 12;
/// The highest monster level a character farms is at most this many above its own level
/// ([`reachable_level`]). Unique monsters are area level +3 and the experience penalty starts
/// above clvl +5, so +2 keeps an area's uniques at or under clvl +5: a level-43 character farms
/// areas up to 45, whose uniques are 48 (user request, 2026-11-20: "if I am 43 then the unique
/// monsters in a 47 level area would be 50, too high").
const REACHABLE_BONUS: u8 = 2;
/// Town level ids: never farmed.
pub const TOWNS: [u32; 5] = [1, 40, 75, 103, 109];
/// Level ids from here on are special or uber levels (Pandemonium, Uber Tristram, …): not part
/// of a difficulty's normal area levels.
const FIRST_SPECIAL_LEVEL: u32 = 133;
/// The last part of a terrorized level's line, drawn in [`TERROR_TEXT`].
const TERRORIZED: &str = "terrorized";
const TERROR_TEXT: [u8; 4] = [190, 120, 255, 255];
const OUTLINE: [u8; 4] = [0, 0, 0, 220];
/// The monster-level part in an area bad for the build ([`Rating::Bad`]).
/// The red of a bad rating — also the colour of a resistance below 75 (user request).
pub const BAD_TEXT: [u8; 4] = [235, 70, 60, 255];
/// The monster-level part in an area good for the build ([`Rating::Good`]).
const GOOD_TEXT: [u8; 4] = [80, 220, 100, 255];
/// An area name (info line, exit labels) where the character gets almost no experience
/// ([`crate::xp::is_low`]); white otherwise (user request, 2026-11-18).
pub const LOW_XP_TEXT: [u8; 4] = [140, 140, 140, 255];

#[must_use]
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
    /// The area gives the character almost no experience ([`crate::xp::is_low`]): the area name
    /// is drawn in [`LOW_XP_TEXT`]. Set by the caller, which knows the character level.
    pub low_xp: bool,
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

    #[must_use]
    pub fn text(&self) -> String {
        self.with_area(&self.area)
    }
}

/// The info line for `level_id`. Without a known difficulty there is no monster-level column to
/// pick, so only the area name is shown. An area with monster level 0 (towns, the Forgotten
/// Tower's ground floor) has no monster levels. A `terrorized` level shows [`TERRORIZED`] in
/// place of monster and boss levels (the table values don't apply in a Terror Zone). In the Moo
/// Moo Farm, `cows` replaces them (a terrorized farm keeps [`TERRORIZED`] last). `rating` is how
/// the area suits the build (the playstyle's [`crate::build::rate`]) and colours the stats.
pub fn info_line(
    level_id: u32,
    difficulty: Option<u8>,
    terrorized: bool,
    cows: Option<Tally>,
    rating: Rating,
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
        rating,
        low_xp: false,
    }
}

/// The highest monster level a character of `char_level` in `difficulty` (0 Normal … 2 Hell)
/// realistically farms, the item-level cap for findable upgrades and natural-socket bases:
/// the character level + [`REACHABLE_BONUS`], kept within the difficulty's area levels (a
/// level-20 character in Nightmare still meets level-36 monsters; nothing in Normal is above
/// 43). `None` for an unknown level or difficulty.
#[must_use]
pub fn reachable_level(char_level: Option<u8>, difficulty: Option<u8>) -> Option<u8> {
    let (lvl, d) = (char_level?, usize::from(difficulty?));
    let mlvls = || {
        LEVELS
            .iter()
            .filter(|&&(id, _, _)| !TOWNS.contains(&id) && id != COW_LEVEL && id < FIRST_SPECIAL_LEVEL)
            .filter_map(move |&(_, _, lvls)| lvls.get(d).copied())
            .filter(|&mlvl| mlvl > 0)
    };
    let (lowest, highest) = (mlvls().min()?, mlvls().max()?);
    Some(lvl.saturating_add(REACHABLE_BONUS).clamp(lowest, highest))
}

/// Why a `BiS` line carries its ✓: the character wears the piece ([`Tick::Worn`]), owns it
/// without wearing it ([`Tick::Owned`]: carried, cubed or personally stashed) or it sits in
/// the shared stash ([`Tick::Stash`]). The dashboard ticks worn pieces green and the others
/// yellow ("you have it, go equip it"; user request 2026-11-18).
///
/// The overlay ticks all three the same (plain ✓, embedded in [`BisLine::text`]); the dashboard
/// distinguishes them via the line's structured `tick` field ([`crate::web::live_json`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    Worn,
    Owned,
    Stash,
}

impl Tick {
    /// The string the dashboard JSON emits for a [`Tick`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Worn => "worn",
            Self::Owned => "owned",
            Self::Stash => "stash",
        }
    }
}

/// One `BiS` chase line: the overlay's text (any ✓ already embedded) and, structurally, why
/// it is ticked (`None` when the piece is owned neither by the character nor the stash).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BisLine {
    pub text: String,
    pub tick: Option<Tick>,
    /// The runeword behind the piece, when the piece names one: the dashboard renders its
    /// runes and bases as a dim hint line under the piece (the overlay doesn't).
    pub runeword: Option<crate::runewords_data::Runeword>,
}

/// The `BiS` per-slot chase lines (user request, 2026-11-17): one line per configured slot —
/// `helm: Mang Song's Lesson (82)` — in the user's slot order, only pieces relevant for the
/// character's level (required level ≤ character level + 5), and skipping slots where the
/// equipped piece is already set/unique quality (presumed the current `BiS`). The mercenary's
/// slots follow prefixed `merc`. At most [`MAX_BIS_LINES`] per section.
// One line: the bis query's inputs are flat snapshot fields; a struct would only shuffle them.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn bis_lines(
    bis: &crate::guidance::Bis,
    char_level: Option<u8>,
    equipped: &[(u8, u32)],
    merc_equipped: &[(u8, u32)],
    worn_names: &[String],
    owned_names: &[String],
    stash_names: &[String],
    font: &FontVec,
    max_width: f32,
) -> Vec<BisLine> {
    let mut out = Vec::new();
    let lvl = u32::from(char_level.unwrap_or(0));
    // A piece the character already owns (equipped, carried or stashed) gets a ✓ (user pick,
    // 2026-11-17): equip it instead of chasing a second one. A piece sitting in the shared
    // stash is ticked too ([`Tick::Stash`]); the overlay shows the same plain ✓, the dashboard
    // marks ✓(stash) from the line's tick.
    for slot in crate::config::SLOT_NAMES {
        let Some(pieces) = bis.slots.get(slot) else {
            continue;
        };
        // Already better: a set/unique equipped in that slot is presumed the current BiS.
        if slot_equipped_better(slot, equipped) {
            continue;
        }
        // Only pieces relevant for the current level (req ≤ level + 5); the highest-level one
        // is the current chase target, so a slot graduates from Stealth to Enigma.
        let Some((piece, req)) = pieces
            .iter()
            .filter_map(|p| piece_level(p).map(|l| (p, l)))
            .filter(|&(_, l)| l <= lvl + 5)
            .max_by_key(|&(_, l)| l)
        else {
            continue;
        };
        let t = piece_tick(piece, worn_names, owned_names, stash_names);
        let mark = if t.is_some() { "✓ " } else { "" };
        if out.len() < MAX_BIS_LINES {
            out.push(BisLine {
                text: format!(
                    "{slot}: {mark}{} ({req})",
                    piece.split('@').next().unwrap_or(piece)
                ),
                tick: t,
                runeword: runeword_of(piece),
            });
        }
    }
    for slot in crate::config::SLOT_NAMES {
        let Some(pieces) = bis.merc_slots.get(slot) else {
            continue;
        };
        if slot_equipped_better(slot, merc_equipped) {
            continue;
        }
        let Some((piece, req)) = pieces
            .iter()
            .filter_map(|p| piece_level(p).map(|l| (p, l)))
            .filter(|&(_, l)| l <= lvl + 5)
            .max_by_key(|&(_, l)| l)
        else {
            continue;
        };
        // The character's worn names are not the mercenary's.
        let t = piece_tick(piece, &[], owned_names, stash_names);
        let mark = if t.is_some() { "✓ " } else { "" };
        if out.len() < MAX_BIS_LINES {
            out.push(BisLine {
                text: format!(
                    "merc {slot}: {mark}{} ({req})",
                    piece.split('@').next().unwrap_or(piece)
                ),
                tick: t,
                runeword: runeword_of(piece),
            });
        }
    }
    // Wrapped per line so text and tick stay aligned even when fit_plain shortens a line.
    out.into_iter()
        .map(|l| BisLine {
            tick: l.tick,
            runeword: l.runeword,
            text: fit_plain(font, &l.text, max_width),
        })
        .collect()
}

/// The area name of `level_id` from the levels table, or `None` for an unknown id (callers
/// fall back to `Level <id>`, like [`info_line`]).
#[must_use]
pub fn level_name(level_id: u32) -> Option<&'static str> {
    LEVELS
        .iter()
        .find(|&&(id, _, _)| id == level_id)
        .map(|&(_, name, _)| name)
}

/// The slot name of a body equip-location id (the reverse of [`slot_body`]; the swap slot
/// shares the weapon's id). `None` for ids with no `BiS` slot (the mercenary's or the head).
#[must_use]
pub const fn body_slot_name(body: u8) -> Option<&'static str> {
    match body {
        1 => Some("helm"),
        2 => Some("amulet"),
        3 => Some("body"),
        4 => Some("weapon"),
        5 => Some("offhand"),
        6 => Some("ring1"),
        7 => Some("ring2"),
        8 => Some("belt"),
        9 => Some("boots"),
        10 => Some("gloves"),
        _ => None,
    }
}

/// The required character level of a `BiS` piece: the `@level` config suffix, else the known
/// runeword levels; `None` when unknown (the piece then always shows). Shared by the overlay's
/// chase list and the config page's resolved view ([`crate::web`]).
#[must_use]
pub fn piece_level(piece: &str) -> Option<u32> {
    if let Some(at) = piece.rfind('@')
        && let Ok(n) = piece[at + 1..].parse::<u32>()
    {
        return Some(n);
    }
    let name = piece.split('@').next().unwrap_or(piece);
    RUNEWORD_LEVELS
        .iter()
        .find(|(w, _)| same_name(name, w))
        .map(|&(_, l)| l)
}

/// The bare piece name of a `BiS` piece string (`"Insight@27"` → `"Insight"`); shared by the
/// overlay's chase list and the config page's resolved view ([`crate::web`]).
#[must_use]
pub fn piece_name(piece: &str) -> &str {
    piece.split_once('@').map_or(piece, |(name, _)| name).trim()
}

/// Why a `BiS` piece is ticked: the character wears it (`worn_names`), owns it otherwise
/// (`owned_names`: carried, cubed or personally stashed; may include the worn ones) or it
/// sits in the shared stash ([`Tick`]); `None` for none of these. Shared by the overlay's
/// chase list and the dashboard and config pages ([`crate::web`]).
#[must_use]
pub fn piece_tick(
    piece: &str,
    worn_names: &[String],
    owned_names: &[String],
    stash_names: &[String],
) -> Option<Tick> {
    let name = piece_name(piece);
    if worn_names.iter().any(|n| same_name(n, name)) {
        Some(Tick::Worn)
    } else if owned_names.iter().any(|n| same_name(n, name)) {
        Some(Tick::Owned)
    } else if stash_names.iter().any(|n| same_name(n, name)) {
        Some(Tick::Stash)
    } else {
        None
    }
}

/// Whether the equipped piece in `slot`'s body location is set or unique quality: presumed the
/// current `BiS`, so the slot's chase pieces are already beaten (the overlay skips the slot
/// entirely, the config page strikes its pieces through instead).
#[must_use]
pub fn slot_equipped_better(slot: &str, equipped: &[(u8, u32)]) -> bool {
    slot_body(slot).is_some_and(|body| equipped.iter().any(|&(b, q)| b == body && (q == 5 || q == 7)))
}

/// The runeword behind a `BiS` piece string, when the piece names one (case-insensitive).
fn runeword_of(piece: &str) -> Option<crate::runewords_data::Runeword> {
    crate::runewords_data::lookup(piece_name(piece)).copied()
}

/// Body slot ids for the `BiS` slot names ([`crate::game`] equip locations).
fn slot_body(slot: &str) -> Option<u8> {
    match slot {
        "helm" => Some(1),
        "amulet" => Some(2),
        "body" => Some(3),
        "weapon" | "swap" => Some(4),
        "offhand" => Some(5),
        "ring1" => Some(6),
        "ring2" => Some(7),
        "belt" => Some(8),
        "boots" => Some(9),
        "gloves" => Some(10),
        _ => None,
    }
}

/// The known runeword required character levels (classic tables); named uniques carry their
/// level in the config's `@level` suffix instead.
const RUNEWORD_LEVELS: [(&str, u32); 20] = [
    ("Steel", 13),
    ("Stealth", 17),
    ("Leaf", 19),
    ("Ancient's Pledge", 21),
    ("Zephyr", 21),
    ("Spirit", 25),
    ("Strength", 25),
    ("Insight", 27),
    ("Lore", 27),
    ("Honor", 27),
    ("Smoke", 37),
    ("Bulwark", 35),
    ("Treachery", 43),
    ("Harlequin Crest", 42),
    ("Andariel's Visage", 66),
    ("Heart of the Oak", 55),
    ("Infinity", 63),
    ("Fortitude", 59),
    ("Enigma", 65),
    ("Mang Song's Lesson", 82),
];

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

/// One Sell / Keep panel entry as text: `Gravenspine (inv)`, `Magefist ×2 (stash)`, a move
/// with its target pages `Hunter's Bow 4os (page 4 → 1–2)`, a scored sell with its reason
/// `Rare Ring (page 4, no build stats)`.
#[must_use]
pub fn advice_entry(a: &Advice) -> String {
    let mut place = a.spot.label();
    if let Action::Move(side) = a.action {
        place = format!("{place} → {}", side.pages());
    }
    if let Some(reason) = a.reason {
        place = format!("{place}, {reason}");
    }
    if a.count > 1 {
        format!("{} ×{} ({place})", a.name, a.count)
    } else {
        format!("{} ({place})", a.name)
    }
}

/// One dashboard text row (the breakpoint guide): its text, colour, and optional coloured segments
/// (`spans`: byte offset into `text` where a differently coloured part starts — used for the
/// guide's low-resistance values, user request, 2026-11-17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    pub text: String,
    pub colour: [u8; 4],
    pub spans: Vec<(usize, [u8; 4])>,
}

impl Band {
    /// A plain row without spans (tests: the core's and the playstyle crates').
    #[cfg(any(test, feature = "testutil"))]
    pub fn plain(text: impl Into<String>, colour: [u8; 4]) -> Self {
        Self {
            text: text.into(),
            colour,
            spans: Vec::new(),
        }
    }
}

/// Canvas px at the bottom kept free of the map for the info line.
#[must_use]
pub fn band_height(font: &FontVec) -> u32 {
    let scaled = font.as_scaled(PxScale::from(TEXT_PX));
    // +2: the 1 px outline above and below.
    ceil_px(scaled.height()) + 2 + 2 * PAD
}

/// Width in px of `text` set in `font` (advances plus kerning).
fn text_width(font: &FontVec, text: &str) -> f32 {
    text_width_at(font, TEXT_PX, text)
}

/// [`text_width`] at `size` px.
fn text_width_at(font: &FontVec, size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
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
#[must_use]
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

/// Draws `line` bottom-left into `pm`: white text with a dark 1 px outline, a terrorized
/// line's last part in violet. Draws nothing if the canvas is smaller than the band.
pub fn draw(pm: &mut Pixmap, font: &FontVec, line: &InfoLine) {
    let w = pm.width();
    let h = pm.height();
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

/// The fitted `text` of `line` ([`fit_text`]) as coloured parts, left to right: white, the area
/// name grey when it gives almost no experience, then the stats in the rating's colour (red or
/// green; white when neutral), then a terrorized line's [`TERRORIZED`] in violet. Empty parts
/// are left out.
fn spans<'a>(line: &InfoLine, text: &'a str) -> Vec<(&'a str, [u8; 4])> {
    let mut parts = rated_spans(line, text);
    if !line.low_xp {
        return parts;
    }
    // The text is `[difficulty · ]area[ · stats]` and fitting only shortens the area, so the
    // area sits between the known prefix and suffix lengths, inside the first (white) part.
    let start = line.difficulty.map_or(0, |d| d.len() + SEPARATOR.len());
    let tail: usize = line.stats.iter().map(|s| SEPARATOR.len() + s.len()).sum();
    let end = text.len().saturating_sub(tail);
    let Some(&(head, TEXT)) = parts.first() else {
        return parts;
    };
    let (Some(before), Some(area), Some(after)) = (head.get(..start), head.get(start..end), head.get(end..))
    else {
        return parts;
    };
    parts.splice(
        ..1,
        [(before, TEXT), (area, LOW_XP_TEXT), (after, TEXT)]
            .into_iter()
            .filter(|(part, _)| !part.is_empty()),
    );
    parts
}

/// [`spans`] without the area colouring.
fn rated_spans<'a>(line: &InfoLine, text: &'a str) -> Vec<(&'a str, [u8; 4])> {
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

/// Truncates a [`Band`] to `max_width` px in `font`, appending `…` when it doesn't fit; spans
/// beyond the cut are dropped (their text is gone anyway). For plain one-line texts use
/// [`fit_plain`]; the info line has its own span-aware fitting.
#[must_use]
pub fn fit_band(mut band: Band, font: &FontVec, max_width: f32) -> Band {
    if text_width(font, &band.text) <= max_width {
        return band;
    }
    let chars: Vec<char> = band.text.chars().collect();
    for keep in (1..chars.len()).rev() {
        let cut: String = chars.get(..keep).unwrap_or_default().iter().collect();
        let cut = format!("{}…", cut.trim_end());
        if text_width(font, &cut) <= max_width {
            let limit = cut.len() - '…'.len_utf8();
            band.spans.retain(|&(off, _)| off < limit);
            band.text = cut;
            return band;
        }
    }
    band
}

/// Truncates `text` to `max_width` px in `font`, appending `…` when it doesn't fit. For plain
/// one-line texts (the guide lines); the info line has its own span-aware fitting.
#[must_use]
pub fn fit_plain(font: &FontVec, text: &str, max_width: f32) -> String {
    if text_width(font, text) <= max_width {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (1..chars.len()).rev() {
        let cut: String = chars.get(..keep).unwrap_or_default().iter().collect();
        let cut = cut.trim_end();
        let cut = format!("{cut}…");
        if text_width(font, &cut) <= max_width {
            return cut;
        }
    }
    text.to_owned()
}

/// Map label text size in px (exit target names, marker tags).
const LABEL_PX: f32 = 12.0;
/// Px between a label's baseline and the centre of the marker it names (label above it).
const LABEL_RAISE: f32 = 7.0;
/// Px between a marker's centre and the near edge of a label beside it.
const LABEL_SIDE: f32 = 8.0;

/// The box a map label's text takes: `width` px wide, `ascent` px above its baseline and
/// `-descent` px below it (`descent` is negative, as ab-glyph gives it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelSize {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
}

/// The [`LabelSize`] of `text` set as a map label in `font`.
#[must_use]
pub fn label_size(font: &FontVec, text: &str) -> LabelSize {
    let scaled = font.as_scaled(PxScale::from(LABEL_PX));
    LabelSize {
        width: text_width_at(font, LABEL_PX, text),
        ascent: scaled.ascent(),
        descent: scaled.descent(),
    }
}

/// One map label for [`place_labels`]: its size, the canvas centre of the marker it names, and
/// whether it is drawn (above its marker) even when every spot overlaps a label placed earlier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelSpec {
    pub size: LabelSize,
    pub at: (f32, f32),
    pub keep: bool,
}

/// Places map `labels`, highest priority first, on a `canvas_w` px wide canvas above `bottom` (the
/// map area's height, so labels stay off the info line). Each goes to the first spot around its
/// marker (above, below, right, left), pushed inside the canvas edges, whose box overlaps no label
/// placed before it. A label with no free spot is dropped (`None`), unless it is `keep`: then it
/// goes above its marker anyway. Returns each label's pen position and baseline, in input order.
#[must_use]
pub fn place_labels(labels: &[LabelSpec], canvas_w: f32, bottom: f32) -> Vec<Option<(f32, f32)>> {
    let mut taken: Vec<[f32; 4]> = Vec::new();
    labels
        .iter()
        .map(|l| {
            let spots = [0, 1, 2, 3].map(|slot| label_spot(l, slot, canvas_w, bottom));
            let free = spots.into_iter().find(|&s| {
                let b = label_box(l.size, s);
                !taken.iter().any(|t| boxes_overlap(*t, b))
            });
            let spot = free.or_else(|| l.keep.then_some(spots[0]))?;
            taken.push(label_box(l.size, spot));
            Some(spot)
        })
        .collect()
}

/// Pen position and baseline of `l` in spot `slot` (0 above its marker, 1 below, 2 right, 3 left),
/// pushed inside the canvas.
fn label_spot(l: &LabelSpec, slot: u8, canvas_w: f32, bottom: f32) -> (f32, f32) {
    let LabelSize {
        width,
        ascent,
        descent,
    } = l.size;
    let (cx, cy) = l.at;
    // Above: the box's bottom edge sits `LABEL_RAISE + descent` px above the centre; below
    // mirrors it. Beside: the box is centred on the marker's height.
    let beside = cy + f32::midpoint(ascent, descent);
    let (pen, baseline) = match slot {
        0 => (cx - width / 2.0, cy - LABEL_RAISE),
        1 => (cx - width / 2.0, cy + LABEL_RAISE + descent + ascent),
        2 => (cx + LABEL_SIDE, beside),
        _ => (cx - LABEL_SIDE - width, beside),
    };
    // +1/+2: room for the outline. The lowest baseline keeps the descenders and outline above
    // `bottom`.
    (
        pen.min(canvas_w - width - 2.0).max(1.0),
        baseline.min(bottom + descent - 1.0).max(ascent + 1.0),
    )
}

/// The canvas box `[x0, y0, x1, y1]` a label of `size` at (`pen`, `baseline`) covers, outline
/// included.
fn label_box(size: LabelSize, (pen, baseline): (f32, f32)) -> [f32; 4] {
    [
        pen - 1.0,
        baseline - size.ascent,
        pen + size.width + 1.0,
        baseline - size.descent + 1.0,
    ]
}

fn boxes_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Draws `text` as a map label in `colour` with the info line's dark outline, at the pen position
/// and baseline [`place_labels`] gave it.
pub fn draw_label(pm: &mut Pixmap, font: &FontVec, colour: [u8; 4], text: &str, at: (f32, f32)) {
    draw_outlined(pm, font, LABEL_PX, colour, text, at, false);
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
    use crate::build::{self, Damage};

    /// A two-letter label ("wp"): narrow enough for all four spots around one marker to be free.
    const SIZE: LabelSize = LabelSize {
        width: 12.0,
        ascent: 11.0,
        descent: -3.0,
    };

    fn spec(at: (f32, f32), keep: bool) -> LabelSpec {
        LabelSpec { size: SIZE, at, keep }
    }

    fn placed_box(at: Option<(f32, f32)>) -> Option<[f32; 4]> {
        at.map(|a| label_box(SIZE, a))
    }

    #[test]
    fn a_crowded_label_moves_below_then_beside_its_marker() {
        let at = (200.0, 200.0);
        let got = place_labels(
            &[spec(at, true), spec(at, false), spec(at, false), spec(at, false)],
            400.0,
            400.0,
        );
        let boxes: Vec<[f32; 4]> = got.iter().filter_map(|&a| placed_box(a)).collect();
        let &[above, below, right, left] = boxes.as_slice() else {
            unreachable!("above, below, right and left are all free: {got:?}");
        };
        assert!(above[3] <= 200.0 && below[1] >= 200.0, "{above:?} {below:?}");
        assert!(right[0] > 200.0 && left[2] < 200.0, "{right:?} {left:?}");
        for (i, a) in boxes.iter().enumerate() {
            for b in boxes.iter().skip(i + 1) {
                assert!(!boxes_overlap(*a, *b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn a_label_with_no_free_spot_is_dropped_unless_kept() {
        let at = (200.0, 200.0);
        let got = place_labels(&[spec(at, true); 4], 400.0, 400.0);
        assert!(got.iter().all(Option::is_some), "four spots: {got:?}");
        let mut labels = vec![spec(at, true); 4];
        labels.push(spec(at, false));
        labels.push(spec(at, true));
        let got = place_labels(&labels, 400.0, 400.0);
        assert_eq!(got.get(4), Some(&None), "lower-ranked label dropped");
        assert_eq!(
            got.get(5),
            got.first(),
            "kept label falls back to above its marker"
        );
        // A label far away is unaffected.
        let far = spec((50.0, 50.0), false);
        let got = place_labels(&[spec(at, true), far], 400.0, 400.0);
        assert_eq!(got.get(1), Some(&Some(label_spot(&far, 0, 400.0, 400.0))));
    }

    #[test]
    fn labels_stay_inside_the_canvas_and_above_the_bottom() {
        let got = place_labels(
            &[spec((398.0, 2.0), false), spec((1.0, 399.0), false)],
            400.0,
            300.0,
        );
        for b in got.iter().map(|&a| placed_box(a)) {
            assert!(
                b.is_some_and(|[x0, y0, x1, y1]| x0 >= 0.0 && y0 >= 0.0 && x1 <= 400.0 && y1 <= 300.0),
                "{b:?}"
            );
        }
    }

    #[test]
    fn worn_beats_owned_beats_stash() {
        let names = |n: &str| vec![n.to_owned()];
        let stealth = names("Stealth");
        assert_eq!(
            piece_tick("Stealth@17", &stealth, &stealth, &stealth),
            Some(Tick::Worn)
        );
        assert_eq!(
            piece_tick("Stealth@17", &[], &stealth, &stealth),
            Some(Tick::Owned)
        );
        assert_eq!(piece_tick("Stealth@17", &[], &[], &stealth), Some(Tick::Stash));
        assert_eq!(piece_tick("Stealth@17", &[], &[], &[]), None);
    }

    #[test]
    fn bis_pieces_match_the_game_spelling() {
        assert_eq!(piece_level("Ancients' Pledge"), Some(21), "builds' spelling");
        assert_eq!(
            piece_tick("Ancients' Pledge", &[], &["Ancient's Pledge".to_owned()], &[]),
            Some(Tick::Owned)
        );
    }

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
            info_line(88, Some(0), false, None, Rating::Neutral).text(),
            "Normal · Flayer Dungeon Level 1 · mlvl 22 · champ 24 · unique 25"
        );
        assert_eq!(
            info_line(88, Some(1), false, None, Rating::Neutral).text(),
            "Nightmare · Flayer Dungeon Level 1 · mlvl 51 · champ 53 · unique 54"
        );
        assert_eq!(
            info_line(88, Some(2), false, None, Rating::Neutral).text(),
            "Hell · Flayer Dungeon Level 1 · mlvl 81 · champ 83 · unique 84"
        );
    }

    #[test]
    fn line_with_boss() {
        assert_eq!(
            info_line(102, Some(0), false, None, Rating::Neutral).text(),
            "Normal · Durance of Hate Level 3 · mlvl 25 · champ 27 · unique 28 · Mephisto 26"
        );
        assert_eq!(
            info_line(102, Some(2), false, None, Rating::Neutral)
                .stats
                .last()
                .map(String::as_str),
            Some("Mephisto 87")
        );
    }

    #[test]
    fn unknown_level_has_no_monster_levels() {
        let line = info_line(9999, Some(1), false, None, Rating::Neutral);
        assert_eq!(line.text(), "Nightmare · Level 9999");
    }

    #[test]
    fn unknown_difficulty_is_left_out() {
        for d in [None, Some(3), Some(200)] {
            assert_eq!(
                info_line(88, d, false, None, Rating::Neutral).text(),
                "Flayer Dungeon Level 1",
                "{d:?}"
            );
        }
        // Mephisto's level depends on the difficulty too.
        assert_eq!(
            info_line(102, None, false, None, Rating::Neutral).stats,
            Vec::<String>::new()
        );
    }

    #[test]
    fn long_area_names_are_shortened() {
        let Some(font) = font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        let line = info_line(88, Some(0), false, None, Rating::Neutral);
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

    #[test]
    fn advice_entries() {
        use crate::guidance::{Side, Spot};
        let advice = |name: &str, spot, action, count| Advice {
            name: name.into(),
            spot,
            action,
            reason: None,
            count,
        };
        assert_eq!(
            advice_entry(&advice("Gravenspine", Spot::Inventory, Action::Sell, 1)),
            "Gravenspine (inv)"
        );
        assert_eq!(
            advice_entry(&advice("Magefist", Spot::Stash, Action::GiveAway, 2)),
            "Magefist ×2 (stash)"
        );
        assert_eq!(
            advice_entry(&advice(
                "Hunter's Bow 4os",
                Spot::Page(4),
                Action::Move(Side::Own),
                1
            )),
            "Hunter's Bow 4os (page 4 → 1–2)"
        );
        assert_eq!(
            advice_entry(&advice("Shako", Spot::Cube, Action::Move(Side::Trade), 1)),
            "Shako (cube → 3–4)"
        );
        assert_eq!(
            advice_entry(&advice("a", Spot::Page(1), Action::Keep(Side::Own), 1)),
            "a (page 1)"
        );
        let mut ring = advice("Rare Ring", Spot::Page(4), Action::Sell, 1);
        ring.reason = Some("no build stats");
        assert_eq!(advice_entry(&ring), "Rare Ring (page 4, no build stats)");
    }

    #[test]
    fn bis_lines_tick_owned_and_stash_pieces() {
        let Some(font) = font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        let bis = crate::guidance::Bis {
            build: "test".to_owned(),
            slots: [("helm".to_owned(), vec!["Harlequin Crest@82".to_owned()])].into(),
            merc_slots: [("body".to_owned(), vec!["Treachery@23".to_owned()])].into(),
        };
        let owned = ["Harlequin Crest".to_owned()];
        let stash = ["Treachery".to_owned()];
        let lines = bis_lines(&bis, Some(90), &[], &[], &[], &owned, &stash, font, 10_000.0);
        // The character line carries its tick too (not just the merc lines), and the overlay
        // text shows the plain ✓ for both tick kinds. A unique piece is no runeword; a runeword
        // piece carries its runeword for the dashboard's hint line.
        let treachery = match crate::runewords_data::lookup("Treachery") {
            Some(rw) => *rw,
            None => return,
        };
        assert_eq!(
            lines,
            vec![
                BisLine {
                    text: "helm: ✓ Harlequin Crest (82)".to_owned(),
                    tick: Some(Tick::Owned),
                    runeword: None,
                },
                BisLine {
                    text: "merc body: ✓ Treachery (23)".to_owned(),
                    tick: Some(Tick::Stash),
                    runeword: Some(treachery),
                },
            ]
        );
        // The runeword lookup is case-insensitive over the piece string's name part.
        assert_eq!(
            runeword_of("treachery@43").map(|rw| rw.runes),
            Some(&["Shael", "Thul", "Lem"][..])
        );
        // Unticked pieces keep `tick: null` and no mark.
        let none = bis_lines(&bis, Some(90), &[], &[], &[], &[], &[], font, 10_000.0);
        assert_eq!(none.first().map(|l| l.tick), Some(None));
        assert_eq!(
            none.first().map(|l| l.text.clone()),
            Some("helm: Harlequin Crest (82)".to_owned())
        );
    }

    #[test]
    fn reachable_level_is_two_above_the_character_level() {
        // clvl +2 keeps an area's uniques (+3) at or under clvl +5: Nightmare 43 → 45.
        assert_eq!(reachable_level(Some(43), Some(1)), Some(45));
        assert_eq!(reachable_level(Some(70), Some(2)), Some(72));
        // Kept within the difficulty's area levels: Nightmare starts at 36, Normal tops out at 43.
        assert_eq!(reachable_level(Some(20), Some(1)), Some(36));
        assert_eq!(reachable_level(Some(80), Some(0)), Some(43));
        assert_eq!(reachable_level(Some(99), Some(2)), Some(87)); // Arreat Summit
        // Unknown level or difficulty: no level (no findable or base hints).
        assert_eq!(reachable_level(None, Some(0)), None);
        assert_eq!(reachable_level(Some(10), None), None);
        assert_eq!(reachable_level(Some(10), Some(3)), None);
    }

    #[test]
    fn terrorized_line_drops_table_levels() {
        // Moo Moo Farm (39).
        let line = info_line(39, Some(0), true, None, Rating::Neutral);
        assert_eq!(line.text(), "Normal · Moo Moo Farm · terrorized");
        assert!(line.terrorized);
        assert!(
            info_line(39, Some(0), false, None, Rating::Neutral)
                .text()
                .contains("mlvl"),
            "not terrorized: table values"
        );
        // No boss level either (Mephisto's level is a table value too).
        assert_eq!(
            info_line(102, Some(2), true, None, Rating::Neutral).text(),
            "Hell · Durance of Hate Level 3 · terrorized"
        );
        assert_eq!(
            info_line(39, None, true, None, Rating::Neutral).text(),
            "Moo Moo Farm · terrorized"
        );
    }

    #[test]
    fn zero_mlvl_area_has_no_monster_levels() {
        // Rogue Encampment: monster level 0 in the table.
        assert_eq!(
            info_line(1, Some(0), false, None, Rating::Neutral).text(),
            "Normal · Rogue Encampment"
        );
    }

    #[test]
    fn the_cow_counter_replaces_the_monster_levels_in_level_39_only() {
        let cows = Some(Tally { kills: 57, alive: 23 });
        assert_eq!(
            info_line(39, Some(0), false, cows, Rating::Neutral).text(),
            "Normal · Moo Moo Farm · kill count: 57 / victims alive: 23"
        );
        let terror = info_line(39, Some(2), true, cows, Rating::Neutral);
        assert_eq!(
            terror.text(),
            "Hell · Moo Moo Farm · kill count: 57 / victims alive: 23 · terrorized"
        );
        assert!(terror.terrorized, "still ends in the violet part");
        // Elsewhere, or without a counter, the usual line.
        assert_eq!(
            info_line(88, Some(0), false, cows, Rating::Neutral),
            info_line(88, Some(0), false, None, Rating::Neutral)
        );
        assert!(
            info_line(39, Some(0), false, None, Rating::Neutral)
                .text()
                .contains("mlvl")
        );
    }

    const VEX: [Damage; 3] = [Damage::Lightning, Damage::Fire, Damage::Physical];

    #[test]
    fn low_xp_greys_only_the_area_name() {
        // Travincal, Nightmare, good for Vex: the stats stay green.
        let mut line = info_line(83, Some(1), false, None, build::rate(83, Some(1), &VEX));
        line.low_xp = true;
        let text = line.text();
        assert_eq!(
            spans(&line, &text),
            vec![
                ("Nightmare · ", TEXT),
                ("Travincal", LOW_XP_TEXT),
                (" · ", TEXT),
                ("mlvl 54 · champ 56 · unique 57", GOOD_TEXT)
            ]
        );
        // Normal (no rating): the white stats follow the grey name.
        let mut line = info_line(83, Some(0), false, None, build::rate(83, Some(0), &VEX));
        line.low_xp = true;
        let text = line.text();
        assert_eq!(
            spans(&line, &text),
            vec![
                ("Normal · ", TEXT),
                ("Travincal", LOW_XP_TEXT),
                (" · mlvl 24 · champ 26 · unique 27", TEXT)
            ]
        );
        // A shortened name and no difficulty: still just the name.
        let mut line = info_line(83, None, false, None, Rating::Neutral);
        line.low_xp = true;
        assert_eq!(spans(&line, "Travi…"), vec![("Travi…", LOW_XP_TEXT)]);
    }

    #[test]
    fn rating_colours_only_the_stats() {
        // Travincal, Nightmare: no immunes, good for Vex.
        let line = info_line(83, Some(1), false, None, build::rate(83, Some(1), &VEX));
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
        let line = info_line(106, Some(2), false, None, build::rate(106, Some(2), &VEX));
        let text = line.text();
        assert_eq!(
            spans(&line, &text).last(),
            Some(&("mlvl 84 · champ 86 · unique 87", BAD_TEXT))
        );
        // Normal, or no build: one white part.
        for line in [
            info_line(106, Some(0), false, None, build::rate(106, Some(0), &VEX)),
            info_line(106, Some(2), false, None, Rating::Neutral),
        ] {
            let text = line.text();
            assert_eq!(line.rating, Rating::Neutral);
            assert_eq!(spans(&line, &text), vec![(text.as_str(), TEXT)]);
        }
        // Mephisto's level is part of the coloured stats.
        let line = info_line(102, Some(1), false, None, build::rate(102, Some(1), &VEX));
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
        let line = info_line(39, Some(1), true, cows, build::rate(39, Some(1), &build));
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
        let line = info_line(83, Some(1), true, None, build::rate(83, Some(1), &VEX));
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
        let line = info_line(83, Some(1), false, None, build::rate(83, Some(1), &VEX));
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
            draw(&mut pm, font, line);
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
        let good = info_line(83, Some(1), false, None, build::rate(83, Some(1), &VEX));
        // Only fully covered glyph pixels count: a few dozen at 16 px.
        assert!(count(&good, [r, g, b]) > 10, "green glyph pixels");
        assert!(count(&good, [255, 255, 255]) > 10, "white area name");
        let neutral = info_line(83, Some(0), false, None, build::rate(83, Some(0), &VEX));
        assert_eq!(count(&neutral, [r, g, b]), 0, "Normal: no green");
        let [r, g, b, _] = BAD_TEXT;
        assert!(
            count(
                &info_line(106, Some(2), false, None, build::rate(106, Some(2), &VEX)),
                [r, g, b]
            ) > 10,
            "red"
        );
    }
}
