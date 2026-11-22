//! In-game UI marks: a click-through layer over the whole screen that points at things in the
//! game's own windows. While a vendor trade window is open it lists the worthwhile store items
//! in a box at the top centre and frames the inventory's items to sell; the stash window's
//! frames ([`Window::Stash`]) and the store-grid frames take state the reader doesn't have yet
//! (whether the stash is open, its shown tab, the shown store tab).

use ab_glyph::FontVec;
use tiny_skia::{Paint, PathBuilder, Pixmap, Rect as SkRect, Stroke, Transform};

use crate::game::CellRect;
use crate::guidance::{Action, Advice, MarkKind, Spot};
use crate::offsets::stat_list;
use crate::shop::{Shop, ShopItem};
use crate::web::ShopNote;
use crate::{info, render};

pub type Rgba = [u8; 4];

/// px from the screen top to the box, which the layer centres horizontally: the vendor window
/// is on the left half, the inventory on the right. To tune after the calibration screenshots.
pub const BOX_TOP: u32 = 12;
/// Lines in the box, the "+N more" line included.
pub const MAX_LINES: usize = 8;
/// A build piece, an upgrade over the worn item, or a runeword base the builds want.
pub const BUILD: Rgba = [120, 235, 130, 255];
/// Sells for a lot (a valuables match that gets the coin on the map).
pub const MONEY: Rgba = [255, 205, 40, 255];
/// Sell.
pub const SELL: Rgba = [240, 60, 50, 255];
/// Move to other stash pages, or stow on a gems / materials / runes tab.
pub const MOVE: Rgba = [255, 140, 30, 255];
const MORE: Rgba = [200, 200, 200, 255];
const BACKDROP: Rgba = [0, 0, 0, 170];
const TEXT_PX: f32 = 15.0;
/// px between the box edge and its text.
const PAD: f32 = 6.0;
/// px between two lines.
const LINE_GAP: f32 = 3.0;
const FRAME_WIDTH: f32 = 2.0;

/// The screen size the grids below were measured at (D2R fullscreen, 2026-11-22 screenshots);
/// frames are drawn only at this size.
pub const CALIBRATED: (u32, u32) = (1920, 1080);
/// px from one grid cell to the next; a cell's own box is one px less.
pub const CELL_PITCH: u32 = 49;
/// The trade window's store grid (left half).
pub const VENDOR_GRID: Grid = Grid {
    left: 163,
    top: 221,
    cols: 10,
    rows: 10,
};
/// The stash window's Personal tab and shared pages (left half).
pub const STASH_GRID: Grid = Grid {
    left: 163,
    top: 213,
    cols: 10,
    rows: 10,
};
/// The inventory below the worn gear (right half).
pub const INVENTORY_GRID: Grid = Grid {
    left: 1269,
    top: 552,
    cols: 10,
    rows: 4,
};

/// An item grid of the game's windows at [`CALIBRATED`]: its first cell's top-left screen px
/// and its size in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub left: u32,
    pub top: u32,
    pub cols: u8,
    pub rows: u8,
}

impl Grid {
    /// The screen rectangle over `cells` (cell boxes and the gaps between them); `None` when
    /// they leave the grid.
    #[must_use]
    pub fn rect(self, cells: CellRect) -> Option<Rect> {
        let fits =
            |at: u8, len: u8, count: u8| len > 0 && at.checked_add(len).is_some_and(|end| end <= count);
        if !fits(cells.x, cells.w, self.cols) || !fits(cells.y, cells.h, self.rows) {
            return None;
        }
        let span = |len: u8| CELL_PITCH * u32::from(len) - 1;
        Some(Rect {
            x: self.left + CELL_PITCH * u32::from(cells.x),
            y: self.top + CELL_PITCH * u32::from(cells.y),
            w: span(cells.w),
            h: span(cells.h),
        })
    }
}

/// A rectangle in screen px (the layer covers the whole output, so surface px are screen px).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub colour: Rgba,
}

/// What the layer shows: frames around screen rectangles and the text box's lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiMarks {
    pub frames: Vec<(Rect, Rgba)>,
    pub lines: Vec<Line>,
}

impl UiMarks {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.frames.is_empty() && self.lines.is_empty()
    }
}

/// Why a store item is listed, in the box's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Pick {
    Piece,
    Better,
    Base,
    Money,
}

/// The open trade window's worthwhile items (`notes` in window order, as the dashboard's
/// Vendor panel has them): build pieces, upgrades, wanted runeword bases, then money items;
/// identical lines once. Framed in the store grid are those on the store tab `tab` (`None`:
/// the shown tab is unknown, no frames).
#[must_use]
pub fn vendor(shop: &Shop, notes: &[ShopNote], tab: Option<u8>) -> UiMarks {
    debug_assert_eq!(shop.items.len(), notes.len(), "one note per store item");
    let mut picks: Vec<(Pick, String)> = Vec::new();
    let mut frames = Vec::new();
    for (item, note) in shop.items.iter().zip(notes) {
        let Some(pick) = pick(note) else {
            continue;
        };
        if tab == Some(item.page)
            && let Some(rect) = item.cell.and_then(|c| VENDOR_GRID.rect(c))
        {
            frames.push((rect, pick_colour(pick)));
        }
        let line = (pick, short_name(item));
        if !picks.contains(&line) {
            picks.push(line);
        }
    }
    picks.sort_by_key(|(pick, _)| *pick);
    let lines = picks
        .into_iter()
        .map(|(pick, text)| Line {
            text,
            colour: pick_colour(pick),
        })
        .collect();
    UiMarks {
        frames,
        lines: capped(lines),
    }
}

/// One stash item to sell: where it is and its top-left cell (`None`: not read).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StashSell<'a> {
    spot: Spot,
    name: &'a str,
    cell: Option<(u8, u8)>,
}

/// The stash's items to take out and sell, by the stash plan's name (with "eth" and sockets)
/// and place, personal stash first, then by page: "Bone Visor 3os (page 3)", "Spear
/// (personal)". Items of the same name on the same page each get their top-left cell, 1-based:
/// "Ring (page 3, row 2, col 5)". Not the inventory's, the cube's or the gems / materials /
/// runes tab's.
#[must_use]
pub fn stash_sells(advice: &[Advice]) -> Vec<Line> {
    let mut found = Vec::new();
    for line in advice.iter().filter(|line| line.action == Action::Sell) {
        if !matches!(line.spot, Spot::Stash | Spot::Page(_)) {
            continue;
        }
        let count = usize::try_from(line.count).unwrap_or(usize::MAX).max(1);
        let cells = line.cells.iter().map(|c| Some((c.y, c.x)));
        let unread = std::iter::repeat_n(None, count.saturating_sub(line.cells.len()));
        found.extend(cells.chain(unread).map(|cell| StashSell {
            spot: line.spot,
            name: &line.name,
            cell,
        }));
    }
    found.sort();
    let twins = |item: &StashSell| {
        found
            .iter()
            .filter(|other| other.spot == item.spot && other.name == item.name)
            .count()
            > 1
    };
    let lines = found
        .iter()
        .map(|item| {
            let page = match item.spot {
                Spot::Page(n) => format!("page {n}"),
                _ => "personal".to_owned(),
            };
            let place = match item.cell {
                Some((row, col)) if twins(item) => {
                    format!("{page}, row {}, col {}", row + 1, col + 1)
                }
                _ => page,
            };
            Line {
                text: format!("{} ({place})", item.name),
                colour: SELL,
            }
        })
        .collect();
    capped(lines)
}

/// At most [`MAX_LINES`], the last one "+N more" when there are more.
fn capped(mut lines: Vec<Line>) -> Vec<Line> {
    if lines.len() > MAX_LINES {
        let hidden = lines.len() - (MAX_LINES - 1);
        lines.truncate(MAX_LINES - 1);
        lines.push(Line {
            text: format!("+{hidden} more"),
            colour: MORE,
        });
    }
    lines
}

const fn pick_colour(pick: Pick) -> Rgba {
    match pick {
        Pick::Money => MONEY,
        Pick::Piece | Pick::Better | Pick::Base => BUILD,
    }
}

/// The game window the player's item frames are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// A trade window: the inventory's items to sell.
    Vendor,
    /// The stash window showing `Some(spot)` ([`Spot::Stash`] or a shared [`Spot::Page`];
    /// `None`: tab unknown): that tab's items to sell or to move, and the inventory's items
    /// to put into the stash.
    Stash(Option<Spot>),
}

/// Frames around the player's items in the grids `window` shows, by the stash plan's action:
/// sell red, move / stow orange. Each window marks only what is done there: at a vendor the
/// inventory's sell items; at the stash its shown tab's sell and move items and the
/// inventory's move items. Kept and give-away items get no frame.
#[must_use]
pub fn item_frames(advice: &[Advice], window: Window) -> Vec<(Rect, Rgba)> {
    let mut frames = Vec::new();
    for line in advice {
        let colour = match line.action {
            Action::Sell => SELL,
            Action::Move(_) | Action::Stow(_) => MOVE,
            Action::Keep(_) | Action::GiveAway => continue,
        };
        let grid = match (window, line.spot) {
            (Window::Vendor, Spot::Inventory) if colour == SELL => INVENTORY_GRID,
            (Window::Stash(_), Spot::Inventory) if colour == MOVE => INVENTORY_GRID,
            (Window::Stash(Some(shown)), Spot::Stash | Spot::Page(_)) if shown == line.spot => STASH_GRID,
            _ => continue,
        };
        frames.extend(
            line.cells
                .iter()
                .filter_map(|&c| grid.rect(c))
                .map(|r| (r, colour)),
        );
    }
    frames
}

fn pick(note: &ShopNote) -> Option<Pick> {
    if note.slot.is_some() {
        return Some(Pick::Piece);
    }
    if note.why.is_some() {
        return Some(Pick::Better);
    }
    let mark = note.mark.as_ref()?;
    if mark.kind == MarkKind::Keep {
        return Some(Pick::Base);
    }
    render::high_value(mark).then_some(Pick::Money)
}

/// The game's name plus the socket count: "Crystal Sword 4os".
fn short_name(item: &ShopItem) -> String {
    let sockets = item
        .stats
        .iter()
        .find(|s| s.id == stat_list::NUM_SOCKETS)
        .map_or(0, |s| s.value);
    if sockets > 0 {
        format!("{} {sockets}os", item.name)
    } else {
        item.name.clone()
    }
}

/// The layer's picture at the screen size `(width, height)`; `None` when there is nothing to
/// show (or no size yet). Frames only at [`CALIBRATED`].
#[must_use]
pub fn render(marks: &UiMarks, (width, height): (u32, u32), font: &FontVec) -> Option<Pixmap> {
    if marks.is_empty() {
        return None;
    }
    let mut pm = Pixmap::new(width, height)?;
    if (width, height) == CALIBRATED {
        for (rect, colour) in &marks.frames {
            draw_frame(&mut pm, *rect, *colour);
        }
    }
    draw_box(&mut pm, &marks.lines, font);
    Some(pm)
}

fn draw_frame(pm: &mut Pixmap, rect: Rect, colour: Rgba) {
    let inset = FRAME_WIDTH / 2.0;
    let Some(r) = SkRect::from_xywh(
        px(rect.x) + inset,
        px(rect.y) + inset,
        px(rect.w) - FRAME_WIDTH,
        px(rect.h) - FRAME_WIDTH,
    ) else {
        return;
    };
    let stroke = Stroke {
        width: FRAME_WIDTH,
        ..Stroke::default()
    };
    pm.stroke_path(
        &PathBuilder::from_rect(r),
        &paint(colour),
        &stroke,
        Transform::identity(),
        None,
    );
}

/// The lines left-aligned on a dark backdrop, the box centred [`BOX_TOP`] px below the top.
fn draw_box(pm: &mut Pixmap, lines: &[Line], font: &FontVec) {
    if lines.is_empty() {
        return;
    }
    let size = info::label_size_at(font, TEXT_PX, "");
    let row = size.ascent - size.descent + LINE_GAP;
    let widest = lines
        .iter()
        .map(|l| info::label_size_at(font, TEXT_PX, &l.text).width)
        .fold(0.0, f32::max);
    let (w, h) = (
        2.0f32.mul_add(PAD, widest),
        2.0f32.mul_add(PAD, row.mul_add(count(lines.len()), -LINE_GAP)),
    );
    let x = ((px(pm.width()) - w) / 2.0).max(0.0);
    let y = px(BOX_TOP);
    if let Some(r) = SkRect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(BACKDROP), Transform::identity(), None);
    }
    let mut baseline = y + PAD + size.ascent;
    for line in lines {
        info::draw_label_at(pm, font, TEXT_PX, line.colour, &line.text, (x + PAD, baseline));
        baseline += row;
    }
}

fn paint([r, g, b, a]: Rgba) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, a);
    paint.anti_alias = true;
    paint
}

#[allow(
    clippy::cast_precision_loss,
    reason = "at most MAX_LINES lines, far below f32's exact integers"
)]
const fn count(n: usize) -> f32 {
    n as f32
}

#[allow(
    clippy::cast_precision_loss,
    reason = "screen px, far below f32's exact integers"
)]
const fn px(n: u32) -> f32 {
    n as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guidance::{Category, Mark, Side, Tier};
    use crate::item_stats::ItemStat;
    use crate::shop::shop_item;
    use anyhow::{Context, Result};

    fn note(slot: Option<&str>, why: Option<&str>, mark: Option<Mark>) -> ShopNote {
        ShopNote {
            slot: slot.map(str::to_owned),
            why: why.map(str::to_owned),
            mark,
        }
    }

    fn mark(kind: MarkKind, trade: Option<(Tier, Category)>) -> Mark {
        Mark {
            kind,
            trade,
            rule: String::new(),
            label: String::new(),
            uncertain: false,
        }
    }

    fn shop(items: Vec<ShopItem>) -> Shop {
        Shop {
            npc_txt: 0,
            npc: None,
            items,
        }
    }

    fn socketed(code: &str, sockets: i32) -> ShopItem {
        let mut item = shop_item(Some(code), 2, 1, 0, 0);
        item.stats = vec![ItemStat {
            id: stat_list::NUM_SOCKETS,
            param: 0,
            value: sockets,
            base: false,
        }];
        item
    }

    fn texts(marks: &UiMarks) -> Vec<(&str, Rgba)> {
        marks.lines.iter().map(|l| (l.text.as_str(), l.colour)).collect()
    }

    #[test]
    fn worthwhile_items_build_first_money_last_and_the_rest_left_out() {
        let shop = shop(vec![
            shop_item(Some("wnd"), 4, 1, 0, 0),
            shop_item(Some("hp1"), 2, 3, 0, 0),
            socketed("crs", 4),
            shop_item(Some("vgl"), 2, 0, 0, 0),
            shop_item(Some("lbw"), 2, 1, 0, 0),
            shop_item(Some("bst"), 2, 1, 0, 0),
            shop_item(Some("wnd"), 4, 1, 0, 0),
        ]);
        let notes = [
            note(
                None,
                None,
                Some(mark(MarkKind::Trade, Some((Tier::A, Category::Trade)))),
            ),
            note(None, None, None),
            note(None, None, Some(mark(MarkKind::Keep, None))),
            note(Some("gloves"), None, None),
            note(None, Some("+4 avg dmg"), None),
            note(
                None,
                None,
                Some(mark(MarkKind::Trade, Some((Tier::B, Category::Trade)))),
            ),
            note(
                None,
                None,
                Some(mark(MarkKind::Trade, Some((Tier::S, Category::Trade)))),
            ),
        ];
        assert_eq!(
            texts(&vendor(&shop, &notes, None)),
            [
                ("Heavy Gloves", BUILD),
                ("Long Bow", BUILD),
                ("Crystal Sword 4os", BUILD),
                ("Wand", MONEY),
            ],
            "piece, better, base, money; the potion, a B match and the second same wand left out"
        );
    }

    const fn cells(x: u8, y: u8, w: u8, h: u8) -> CellRect {
        CellRect { x, y, w, h }
    }

    const fn rect(x: u32, y: u32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn grid_cells_map_to_the_measured_screen_px() {
        // Corners measured on the 1920×1080 screenshots: each cell box 48 px, 1 px apart.
        assert_eq!(
            INVENTORY_GRID.rect(cells(0, 0, 1, 1)),
            Some(rect(1269, 552, 48, 48))
        );
        assert_eq!(
            INVENTORY_GRID.rect(cells(9, 3, 1, 1)),
            Some(rect(1710, 699, 48, 48)),
            "the last cell box spans x 1710..=1757, y 699..=746"
        );
        assert_eq!(STASH_GRID.rect(cells(0, 0, 1, 1)), Some(rect(163, 213, 48, 48)));
        assert_eq!(
            STASH_GRID.rect(cells(9, 9, 1, 1)),
            Some(rect(604, 654, 48, 48)),
            "x 604..651; y within a px of the measured 655..702"
        );
        assert_eq!(
            VENDOR_GRID.rect(cells(0, 0, 1, 2)),
            Some(rect(163, 221, 48, 97)),
            "a dagger: two cells and the gap between"
        );
        assert_eq!(VENDOR_GRID.rect(cells(8, 7, 2, 3)), Some(rect(555, 564, 97, 146)));
        assert_eq!(VENDOR_GRID.rect(cells(9, 0, 2, 3)), None, "past the right edge");
        assert_eq!(INVENTORY_GRID.rect(cells(0, 2, 1, 3)), None, "past the bottom");
        assert_eq!(
            INVENTORY_GRID.rect(cells(20, 21, 2, 2)),
            None,
            "not a grid position"
        );
        assert_eq!(INVENTORY_GRID.rect(cells(0, 0, 0, 1)), None, "no size");
    }

    #[test]
    fn only_the_shown_store_tab_is_framed() {
        let at = |code: &str, tab: u8, x: u8, y: u8| {
            let mut item = shop_item(Some(code), 2, tab, 0, 0);
            item.cell = CellRect::at(Some(code), x.into(), y.into());
            item
        };
        let shop = shop(vec![at("crs", 1, 0, 0), at("wnd", 3, 2, 0), at("hp1", 3, 4, 0)]);
        let notes = [
            note(Some("weapon"), None, None),
            note(
                None,
                None,
                Some(mark(MarkKind::Trade, Some((Tier::A, Category::Trade)))),
            ),
            note(None, None, None),
        ];
        assert_eq!(vendor(&shop, &notes, None).frames, [], "tab unknown: no frames");
        assert_eq!(vendor(&shop, &notes, None).lines.len(), 2, "the box lists both");
        assert_eq!(
            vendor(&shop, &notes, Some(3)).frames,
            [(rect(261, 221, 48, 97), MONEY)],
            "the misc tab: the wand (1×2 at column 2), not the potion"
        );
        assert_eq!(
            vendor(&shop, &notes, Some(1)).frames,
            [(rect(163, 221, 97, 146), BUILD)],
            "the weapons tab: the sword (2×3)"
        );
    }

    fn advice(spot: Spot, action: Action, at: &[CellRect]) -> Advice {
        Advice {
            name: String::new(),
            spot,
            action,
            reason: None,
            count: 1,
            cells: at.to_vec(),
        }
    }

    fn every_action_everywhere() -> Vec<Advice> {
        use crate::guidance::Tab;
        vec![
            advice(
                Spot::Inventory,
                Action::Sell,
                &[cells(0, 0, 1, 1), cells(2, 0, 1, 1)],
            ),
            advice(Spot::Inventory, Action::Keep(Side::Own), &[cells(4, 0, 2, 2)]),
            advice(Spot::Inventory, Action::Keep(Side::Trade), &[cells(6, 0, 1, 1)]),
            advice(Spot::Inventory, Action::Move(Side::Trade), &[cells(7, 0, 1, 1)]),
            advice(Spot::Inventory, Action::Stow(Tab::Runes), &[cells(8, 0, 1, 1)]),
            advice(Spot::Inventory, Action::GiveAway, &[cells(9, 0, 1, 1)]),
            advice(Spot::Cube, Action::Sell, &[cells(0, 0, 1, 1)]),
            advice(Spot::Stash, Action::Sell, &[cells(0, 0, 1, 1)]),
            advice(Spot::Stash, Action::Keep(Side::Own), &[cells(5, 5, 1, 1)]),
            advice(Spot::Page(3), Action::Move(Side::Own), &[cells(1, 1, 1, 1)]),
            advice(Spot::Page(3), Action::Stow(Tab::Gems), &[cells(2, 2, 1, 1)]),
            advice(Spot::Page(3), Action::Keep(Side::Trade), &[cells(3, 3, 1, 1)]),
        ]
    }

    fn inv(x: u32) -> Rect {
        rect(1269 + 49 * x, 552, 48, 48)
    }

    #[test]
    fn at_a_vendor_only_the_inventorys_sell_items() {
        assert_eq!(
            item_frames(&every_action_everywhere(), Window::Vendor),
            [(inv(0), SELL), (inv(2), SELL)],
            "no move, stow, keep or give-away frames; no cube or stash grid"
        );
    }

    #[test]
    fn at_the_stash_the_shown_tabs_sell_and_move_items_and_the_inventorys_moves() {
        let plan = every_action_everywhere();
        let inventory = [(inv(7), MOVE), (inv(8), MOVE)];
        assert_eq!(
            item_frames(&plan, Window::Stash(None)),
            inventory,
            "tab unknown: the inventory's items for the stash, no sell marks"
        );
        assert_eq!(
            item_frames(&plan, Window::Stash(Some(Spot::Page(3)))),
            [
                inventory.as_slice(),
                &[(rect(212, 262, 48, 48), MOVE), (rect(261, 311, 48, 48), MOVE)]
            ]
            .concat(),
            "shared page 3 shown: its move and stow items, not its kept one"
        );
        assert_eq!(
            item_frames(&plan, Window::Stash(Some(Spot::Stash))),
            [inventory.as_slice(), &[(rect(163, 213, 48, 48), SELL)]].concat(),
            "the personal tab shown: its sell item, not page 3's"
        );
    }

    fn sell(name: &str, spot: Spot, at: &[CellRect]) -> Advice {
        Advice {
            name: name.to_owned(),
            count: u32::try_from(at.len()).unwrap_or(1).max(1),
            ..advice(spot, Action::Sell, at)
        }
    }

    fn texts_of(lines: &[Line]) -> Vec<(&str, Rgba)> {
        lines.iter().map(|l| (l.text.as_str(), l.colour)).collect()
    }

    #[test]
    fn the_stash_box_says_where_each_sell_item_is() {
        use crate::guidance::Tab;
        let plan = [
            sell("Bone Visor 3os", Spot::Page(3), &[cells(4, 1, 2, 2)]),
            sell("Wand", Spot::Inventory, &[cells(0, 0, 1, 2)]),
            sell("Gem", Spot::Cube, &[]),
            advice(Spot::Page(2), Action::Stow(Tab::Gems), &[]),
            advice(Spot::Stash, Action::Keep(Side::Own), &[]),
            sell("Spear", Spot::Stash, &[cells(7, 0, 1, 3)]),
            sell(
                "Rare Ring",
                Spot::Page(3),
                &[cells(9, 9, 1, 1), cells(0, 0, 1, 1)],
            ),
            sell("Rare Ring", Spot::Page(4), &[cells(2, 2, 1, 1)]),
            sell("Amulet", Spot::Page(1), &[]),
        ];
        assert_eq!(
            texts_of(&stash_sells(&plan)),
            [
                ("Spear (personal)", SELL),
                ("Amulet (page 1)", SELL),
                ("Bone Visor 3os (page 3)", SELL),
                ("Rare Ring (page 3, row 1, col 1)", SELL),
                ("Rare Ring (page 3, row 10, col 10)", SELL),
                ("Rare Ring (page 4)", SELL),
            ],
            "by place; a cell only to tell two same-named items on one page apart"
        );
        assert!(stash_sells(&plan[1..5]).is_empty(), "none in the stash: no lines");
    }

    #[test]
    fn a_full_stash_box_ends_in_more() {
        let many: Vec<Advice> = (1..=10)
            .map(|n| sell("Spear", Spot::Page(n), &[cells(0, 0, 1, 3)]))
            .collect();
        let lines = stash_sells(&many);
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(
            lines.last().map(|l| (l.text.as_str(), l.colour)),
            Some(("+3 more", MORE))
        );
    }

    #[test]
    fn a_twink_match_is_no_money() {
        let shop = shop(vec![socketed("crs", 4)]);
        let notes = [note(
            None,
            None,
            Some(mark(MarkKind::Trade, Some((Tier::S, Category::Twink)))),
        )];
        assert!(vendor(&shop, &notes, None).is_empty());
    }

    #[test]
    fn too_many_end_in_a_more_line() {
        let items: Vec<ShopItem> = (2..=11).map(|n| socketed("crs", n)).collect();
        let notes: Vec<ShopNote> = items.iter().map(|_| note(Some("weapon"), None, None)).collect();
        let marks = vendor(&shop(items), &notes, None);
        assert_eq!(marks.lines.len(), MAX_LINES);
        assert_eq!(
            marks.lines.last().map(|l| (l.text.as_str(), l.colour)),
            Some(("+3 more", MORE))
        );
        let eight: Vec<ShopItem> = (2..=9).map(|n| socketed("crs", n)).collect();
        let notes: Vec<ShopNote> = eight.iter().map(|_| note(Some("weapon"), None, None)).collect();
        let marks = vendor(&shop(eight), &notes, None);
        assert_eq!(marks.lines.len(), MAX_LINES, "exactly eight fit");
        assert!(marks.lines.iter().all(|l| l.colour == BUILD));
    }

    /// Pixels close to `colour` inside `area` (x0, y0, x1, y1 exclusive): (count, x range).
    fn tinted(pm: &Pixmap, area: (u32, u32, u32, u32), colour: Rgba) -> (u32, u32, u32) {
        let mut found = (0, u32::MAX, 0);
        for y in area.1..area.3.min(pm.height()) {
            for x in area.0..area.2.min(pm.width()) {
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
    fn nothing_to_show_is_no_picture() {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return;
        };
        assert!(render(&UiMarks::default(), (800, 600), font).is_none());
        let line = UiMarks {
            frames: Vec::new(),
            lines: vec![Line {
                text: "Wand".into(),
                colour: MONEY,
            }],
        };
        assert!(render(&line, (0, 0), font).is_none(), "no screen size yet");
    }

    #[test]
    fn the_box_sits_top_centre_in_its_colours() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let marks = UiMarks {
            frames: Vec::new(),
            lines: vec![
                Line {
                    text: "Heavy Gloves".into(),
                    colour: BUILD,
                },
                Line {
                    text: "Wand".into(),
                    colour: MONEY,
                },
            ],
        };
        let (w, h) = (1000, 600);
        let pm = render(&marks, (w, h), font).context("a picture for two lines")?;
        assert_eq!((pm.width(), pm.height()), (w, h));
        let top = (0, BOX_TOP, w, BOX_TOP + 60);
        let (green, g_left, g_right) = tinted(&pm, top, BUILD);
        let (gold, o_left, _) = tinted(&pm, top, MONEY);
        assert!(green >= 20, "only {green} green pixels");
        assert!(gold >= 10, "only {gold} gold pixels");
        assert!(
            g_left.abs_diff(o_left) <= 2,
            "left-aligned lines: {g_left} vs {o_left}"
        );
        let (left, right) = (g_left, w - 1 - g_right);
        assert!(
            left.abs_diff(right) <= 2 * 6 + 2,
            "the box (widest line) centred: {left} vs {right}"
        );
        let backdrop = pm.pixel(w / 2, BOX_TOP + 2).context("a pixel")?;
        assert!(backdrop.alpha() >= 150, "a dark backdrop behind the text");
        let below =
            tinted(&pm, (0, BOX_TOP + 60, w, h), BUILD).0 + tinted(&pm, (0, BOX_TOP + 60, w, h), MONEY).0;
        assert_eq!(below, 0, "nothing below the box");
        let corner = pm.pixel(5, h - 5).context("a pixel")?;
        assert_eq!(corner.alpha(), 0, "the rest of the screen stays clear");
        Ok(())
    }

    #[test]
    fn frames_go_around_their_screen_rectangles() -> Result<()> {
        let Some(font) = info::font() else {
            eprintln!("skipped: no info-line font on this machine");
            return Ok(());
        };
        let cell = Rect {
            x: 100,
            y: 200,
            w: 40,
            h: 40,
        };
        let marks = UiMarks {
            frames: vec![(cell, BUILD)],
            lines: Vec::new(),
        };
        let pm = render(&marks, CALIBRATED, font).context("a picture for a frame")?;
        let edge = tinted(&pm, (100, 200, 140, 202), BUILD);
        assert_eq!((edge.1, edge.2), (100, 139), "the top edge spans the cell");
        assert_eq!(
            tinted(&pm, (104, 204, 136, 236), BUILD).0,
            0,
            "the cell's inside stays clear"
        );
        assert_eq!(tinted(&pm, (0, 0, 400, 30), BUILD).0, 0, "no box without lines");
        let other = render(&marks, (2560, 1440), font).context("a picture at another size")?;
        assert_eq!(
            tinted(&other, (0, 0, 400, 400), BUILD).0,
            0,
            "the grids are measured at 1920×1080 only: no frames elsewhere"
        );
        Ok(())
    }
}
