//! Meso total reader: the player's mesos, from the box the inventory, an NPC shop or the storage
//! window shows them in.
//!
//! **The box.** The three windows show the total the same way: a box 16 rows tall with a
//! 1-pixel navy border, its corners left out, white inside, and the digits in one dark color.
//! Which window it is shows in the box's width, its ink, what sits just left of it, and the
//! window's red tab underline at a fixed offset from it. Measured on recorded captures at
//! 1280x720 and 1366x768, on Windows and a Mac:
//!
//! - inventory: 145 wide, black ink, an unbordered white square (the coin) 2 px to its left, the
//!   underline 225 rows above, 203 px long, starting 21 px left of the box;
//! - NPC shop: 92 wide, dark grey ink, a navy-bordered 16x16 square 1 px to its left, the
//!   underline 52 rows below, 227 px long, starting 127 px left;
//! - storage: two 131-wide boxes 208 px apart, each with a navy-bordered 34x16 box to its left
//!   and a 205-px underline 225 rows above, starting 41 px left. The right one shows the mesos on
//!   hand, which is read; the left one shows the mesos stored, which never is.
//!
//! An underline is measured on its bottom row: when the window isn't focused, its active tab is
//! drawn pale over the underline's top row.
//!
//! **Finding it** takes the whole game area, so the runner reads a whole frame for it now and
//! then ([`MesoReader::wants_whole_frame`]). Every 14th row is scanned for the border's navy
//! followed by white, which crosses every box, and each candidate is checked as a whole
//! rectangle: navy edges 16 rows apart, and nothing inside but white and one ink color. A cursor
//! or a tooltip over the box breaks that, and the box reads as covered, never as a wrong total.
//! When no box reads, the red underlines say why: one in full means the window is open with its
//! box covered, one cut by the frame's edge a window partly off screen, one cut short elsewhere a
//! window partly covered.
//!
//! **The digits** are cut at blank columns over the box's 14 inside rows, and named on the
//! player's device: nothing is typed and no game pixels ship. A digit shape is named by its loops
//! and strokes ([`digit_shapes::guess`]) only once its exact bitmap has been seen in two
//! different frames, so a shape spoiled by something passing over it, seen once, never names
//! anything; the comma is named by where it sits, narrow and hanging below the digits. From then
//! on shapes are read only by an exact match. When a sale in an NPC shop is read and the shop's
//! buy-back row for it can be read too (in the shop's price font, its digits named by their shape
//! each time), the rise in the total must be the row's amount: if it isn't, the naming is
//! dropped and learned again. A buy-back row that can't be read checks nothing.
//!
//! **Trust.** A total counts once two frames in a row read it the same ([`MesoReading::send`]).
//!
//! **Sizes.** Only game areas of 1280x720 and 1366x768 drawn at the game's own pixel size are
//! read, the sizes checked on recorded captures ([`SIZES`]). Any other size, Full Screen's scaled
//! pictures included, reports [`MesoState::Unsupported`]. A Retina Mac's frame is read one pixel
//! from each block, where the HUD reader found the blocks start, rather than undone whole.

use serde::Serialize;

use super::digit_shapes;
use super::pixels::Pixels;
use crate::events::MesoSource;
use crate::frame::Frame;
use crate::glyph::{Atlas, Glyph, Rect, segment};

/// The game areas the box has been checked at, in game pixels: windowed, drawn at the game's own
/// pixel size. Others report [`MesoState::Unsupported`] until recorded captures of them pass.
pub const SIZES: [(u32, u32); 2] = [(1280, 720), (1366, 768)];

/// Frames between whole frames while no meso box is in view: about once a second at the
/// engine's four frames a second.
pub const LOOK_EVERY: u32 = 4;

/// The box's border.
const NAVY: [u8; 3] = [0x31, 0x55, 0x73];
/// Rows of a box, its borders included.
const BOX_ROWS: u32 = 16;
/// Rows inside a box's border, over which the digits are cut.
const INSIDE_ROWS: u32 = BOX_ROWS - 2;
/// Rows scanned for a box: every 14th, fewer than the rows inside one, so every box is crossed.
const SCAN_STEP: u32 = INSIDE_ROWS;
/// The narrowest box worth checking: narrower than any of the windows'.
const MIN_BOX_WIDTH: u32 = 12;
/// The widest a comma is; digits are wider.
const MAX_COMMA_WIDTH: u32 = 2;
/// The widest a digit is.
const MAX_DIGIT_WIDTH: u32 = 7;
/// Blank columns between two glyphs of a total: one, two after a comma on Windows, one more
/// around a narrow 1.
const MAX_GAP: u32 = 3;
/// The HUD's rows at the bottom of the game area, its buttons included, which hold red of their
/// own (the HP gauge, the CASH SHOP button): no window's underline is looked for there, and none
/// can be, 225 rows above its box or 52 below the shop's.
const HUD_ROWS: u32 = 40;
/// The shortest red run that may be part of an underline, and the shortest underline.
const MIN_RUN: u32 = 20;
const MIN_LINE: u32 = 60;
/// Full underlines of other windows, which are neither a meso window's nor cut short: the skills
/// window's.
const OTHER_LINES: [u32; 1] = [191];
/// Shapes kept while waiting for a second sighting.
const MAX_HELD: usize = 32;
/// Whole frames in a row that a box may show a shape the naming can't take (the shape namer
/// calls it a digit already named to another shape) before the naming is dropped and learned
/// again: about two seconds with a box in view. The game's font changed, or a shape named
/// earlier was wrong.
const CONFLICT_PATIENCE: u32 = 8;

/// What sits just left of a box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Left {
    /// An unbordered white square, the inventory's coin.
    Coin,
    /// A navy-bordered box this wide.
    Framed(u32),
    Other,
}

/// Where a window's red tab underline sits, from its meso box's top-left corner (border
/// included): its left end and top row, and its length. It's 3 rows tall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Line {
    dx: i64,
    dy: i64,
    len: u32,
}

/// One window's meso box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Kind {
    source: MesoSource,
    /// The box's width, border included.
    width: u32,
    ink: [u8; 3],
    left: Left,
    line: Line,
}

const INVENTORY: Kind = Kind {
    source: MesoSource::Inventory,
    width: 145,
    ink: [0, 0, 0],
    left: Left::Coin,
    line: Line {
        dx: -21,
        dy: -225,
        len: 203,
    },
};
const SHOP: Kind = Kind {
    source: MesoSource::Shop,
    width: 92,
    ink: [0x33, 0x33, 0x33],
    left: Left::Framed(16),
    line: Line {
        dx: -127,
        dy: 52,
        len: 227,
    },
};
/// Either of the storage window's two boxes.
const STORAGE: Kind = Kind {
    source: MesoSource::Storage,
    width: 131,
    ink: [0, 0, 0],
    left: Left::Framed(34),
    line: Line {
        dx: -41,
        dy: -225,
        len: 205,
    },
};
const KINDS: [Kind; 3] = [INVENTORY, SHOP, STORAGE];
/// How far apart the storage window's two boxes are, and their underlines: the mesos stored on
/// the left, those on hand on the right.
const STORAGE_PANES: i64 = 208;

/// An NPC shop's buy-back rows, from the shop's meso box: the row of the left pane's Buy Back
/// tab checked for its active color, and the columns it spans.
const BUY_BACK_TAB: Line = Line {
    dx: -295,
    dy: 35,
    len: 56,
};
/// Where the first buy-back row's amount is looked for, from the shop's meso box: right of the
/// coin icon, over the price's rows with the comma's tail.
const BUY_BACK_PRICE: (i64, i64) = (-298, 83);
const PRICE_COLUMNS: u32 = 64;
const PRICE_ROWS: u32 = 14;
/// Rows between two buy-back rows, and how many show at once.
const BUY_BACK_STEP: i64 = 40;
const BUY_BACK_ROWS: i64 = 6;
/// Blank columns between two glyphs of a price: its font is proportional, with more room after a
/// narrow 1 than the box's. Five or more end it: the space before the word "Mesos".
const PRICE_MAX_GAP: u32 = 4;
const PRICE_END_GAP: u32 = 5;

fn near(p: [u8; 3], q: [u8; 3], by: u8) -> bool {
    p.iter().zip(q).all(|(a, b)| a.abs_diff(b) <= by)
}

fn is_navy(p: [u8; 3]) -> bool {
    near(p, NAVY, 20)
}

fn is_white(p: [u8; 3]) -> bool {
    p.iter().all(|&c| c >= 0xf0)
}

/// Dark and nearly grey: the digits of any of the boxes.
fn is_ink(p: [u8; 3]) -> bool {
    let (max, min) = (p.iter().max().copied(), p.iter().min().copied());
    let (max, min) = (max.unwrap_or(0), min.unwrap_or(0));
    max <= 0x70 && max - min <= 24
}

/// A window's tab underline.
fn is_red(p: [u8; 3]) -> bool {
    p[0] >= 0xa0 && p[1] < 0x30 && p[2] < 0x30
}

/// An active tab, as it's drawn focused or pale.
fn is_pink(p: [u8; 3]) -> bool {
    p[0] >= 0xc0 && p[0] >= p[1].saturating_add(0x20) && p[0] >= p[2].saturating_add(0x20)
}

/// A navy-bordered rectangle 16 rows tall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Found {
    /// The border's columns and rows.
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
    /// The first dark color inside, if any.
    ink: Option<[u8; 3]>,
    /// Nothing inside but white and that one color.
    clean: bool,
}

impl Found {
    fn width(&self) -> u32 {
        self.right - self.left + 1
    }
}

/// Every box in the frame: rows scanned every [`SCAN_STEP`] for navy then white, each candidate
/// checked whole.
fn find_boxes(px: &Pixels) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    let mut y = 0;
    while y < px.height {
        let mut x = 0;
        while x + 2 < px.width {
            if !(is_navy(px.rgb(x, y)) && is_white(px.rgb(x + 1, y))) {
                x += 1;
                continue;
            }
            let mut end = x + 1;
            while end < px.width && {
                let p = px.rgb(end, y);
                is_white(p) || is_ink(p)
            } {
                end += 1;
            }
            if end < px.width
                && is_navy(px.rgb(end, y))
                && end - x >= MIN_BOX_WIDTH
                && let Some(b) = rectangle(px, x, end, y)
                && !found.iter().any(|o| (o.left, o.top) == (b.left, b.top))
            {
                found.push(b);
            }
            x = end.max(x + 1);
        }
        y += SCAN_STEP;
    }
    found
}

/// The box whose left and right borders are at `left` and `right` on row `y`, when it's whole:
/// navy rows 16 apart above and below with its corners left out, navy sides, and inside nothing
/// but white and dark ink.
fn rectangle(px: &Pixels, left: u32, right: u32, y: u32) -> Option<Found> {
    let navy_row = |row: u32| (left + 1..right).all(|x| is_navy(px.rgb(x, row)));
    let top = (y.saturating_sub(BOX_ROWS)..y)
        .rev()
        .find(|&t| navy_row(t))?;
    let bottom = (y + 1..(y + BOX_ROWS).min(px.height)).find(|&b| navy_row(b))?;
    if bottom - top + 1 != BOX_ROWS {
        return None;
    }
    let sides = (top + 1..bottom).all(|r| is_navy(px.rgb(left, r)) && is_navy(px.rgb(right, r)));
    let corners = [(left, top), (right, top), (left, bottom), (right, bottom)]
        .into_iter()
        .all(|(x, y)| !is_navy(px.rgb(x, y)));
    if !sides || !corners {
        return None;
    }
    let mut ink = None;
    let mut clean = true;
    for r in top + 1..bottom {
        for x in left + 1..right {
            let p = px.rgb(x, r);
            if is_white(p) {
                continue;
            }
            if !is_ink(p) {
                return None;
            }
            match ink {
                None => ink = Some(p),
                Some(q) if q != p => clean = false,
                Some(_) => {}
            }
        }
    }
    Some(Found {
        left,
        right,
        top,
        bottom,
        ink,
        clean,
    })
}

/// What sits just left of a box: the inventory's unbordered coin square, its right edge 3 px left
/// of the box, or a navy-bordered box ending 1 to 3 px left of it.
fn left_of(px: &Pixels, b: &Found) -> Left {
    let at = |x: i64, y: u32| px.get(x, i64::from(y));
    let white = |x: i64, y: u32| at(x, y).is_some_and(is_white);
    let navy = |x: i64, y: u32| at(x, y).is_some_and(is_navy);
    let (x0, top, bottom) = (i64::from(b.left), b.top, b.bottom);
    let coin = (x0 - 16..x0 - 2).all(|x| white(x, top + 1) && white(x, bottom - 1))
        && (top + 1..bottom).all(|y| white(x0 - 16, y) && white(x0 - 3, y))
        && !at(x0 - 17, top + 5).is_some_and(|p| is_white(p) || is_navy(p));
    if coin {
        return Left::Coin;
    }
    for gap in 1..=3 {
        let r = x0 - gap - 1;
        if !((top + 1..bottom).all(|y| navy(r, y)) && white(r - 1, top + 1)) {
            continue;
        }
        let mut l = r - 1;
        while l > 0 && white(l, top + 1) {
            l -= 1;
        }
        let framed = (top + 1..bottom).all(|y| navy(l, y))
            && (l + 1..r).all(|x| navy(x, top) && navy(x, bottom));
        if framed {
            return Left::Framed((r - l + 1) as u32);
        }
    }
    Left::Other
}

/// Which window's meso box `b` is, by its width, ink and left neighbour.
fn kind_of(px: &Pixels, b: &Found) -> Option<Kind> {
    let ink = b.ink?;
    let left = left_of(px, b);
    KINDS
        .into_iter()
        .find(|k| k.width == b.width() && near(ink, k.ink, 8) && k.left == left)
}

/// How an underline looks where a window puts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineCheck {
    /// There in full, the pixels past its ends not red.
    Full,
    /// Some of it, or the pixels that tell its length, are outside the frame.
    OffFrame,
    /// Not there in full: something is over it.
    Cut,
}

/// The underline whose top row starts at (`x`, `top`), `len` long, measured on its bottom row:
/// there when both its ends are exactly where the window puts them and at least three quarters
/// of it shows. The cursor clicking a tab rests on it often, and only the ends tell its length.
fn line_at(px: &Pixels, x: i64, top: i64, len: u32) -> LineCheck {
    let (bottom, right) = (top + 2, x + i64::from(len) - 1);
    if x < 1 || top < 0 || right + 1 >= i64::from(px.width) || bottom + 1 >= i64::from(px.height) {
        return LineCheck::OffFrame;
    }
    let red = |x: i64, y: i64| is_red(px.rgb(x as u32, y as u32));
    let shown = (x..=right).filter(|&xx| red(xx, bottom)).count();
    let ends =
        red(x, bottom) && red(right, bottom) && !red(x - 1, bottom) && !red(right + 1, bottom);
    let thick = [x, right]
        .into_iter()
        .all(|xx| red(xx, bottom - 1) && !red(xx, bottom + 1));
    match ends && thick && shown * 4 >= len as usize * 3 {
        true => LineCheck::Full,
        false => LineCheck::Cut,
    }
}

/// A red line 3 rows tall, found anywhere: possibly a window's underline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RedLine {
    x: u32,
    top: u32,
    len: u32,
    /// It runs into the frame's left or right edge.
    at_edge: bool,
}

/// Every red line 3 rows tall and at least [`MIN_LINE`] long above the HUD, measured on its
/// bottom row. Rows are scanned every third, which crosses every such line.
fn red_lines(px: &Pixels) -> Vec<RedLine> {
    let red = |x: u32, y: u32| is_red(px.rgb(x, y));
    let mut lines: Vec<RedLine> = Vec::new();
    let mut y = 0;
    while y < px.height.saturating_sub(HUD_ROWS) {
        let mut x = 0;
        while x < px.width {
            if !red(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < px.width && red(x, y) {
                x += 1;
            }
            if x - start < MIN_RUN {
                continue;
            }
            let probe = start + (x - start) / 2;
            // The bottom row: an active tab can be drawn over the top row, never the bottom.
            let mut bottom = y;
            while bottom + 1 < px.height && red(probe, bottom + 1) {
                bottom += 1;
            }
            let three = bottom - y <= 2
                && bottom >= 2
                && red(probe, bottom - 1)
                && !(bottom >= 3 && red(probe, bottom - 3));
            if !three {
                continue;
            }
            let mut l = probe;
            while l > 0 && red(l - 1, bottom) {
                l -= 1;
            }
            let mut r = probe;
            while r + 1 < px.width && red(r + 1, bottom) {
                r += 1;
            }
            let line = RedLine {
                x: l,
                top: bottom - 2,
                len: r - l + 1,
                at_edge: l == 0 || r + 1 == px.width,
            };
            if line.len >= MIN_LINE && !lines.iter().any(|o| (o.x, o.top) == (line.x, line.top)) {
                lines.push(line);
            }
        }
        y += 3;
    }
    lines
}

/// What a glyph of a total is, by where its ink sits: a digit spans the digits' rows exactly; a
/// comma is narrow and hangs below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Digit,
    Comma,
}

/// The top and bottom rows that hold ink.
fn ink_span(glyph: &Glyph) -> Option<(u32, u32)> {
    let rows = glyph.ink_rows();
    (rows != 0).then(|| (rows.trailing_zeros(), 31 - rows.leading_zeros()))
}

/// Each glyph's part, when the glyphs make a total as the game draws one: digits sharing their
/// rows, commas between groups of three, a first group of one to three, and at most
/// `max_gap` blank columns between neighbours. `None` for anything else, such as a mark drawn
/// over the digits.
fn parts(glyphs: &[(u32, Glyph)], max_gap: u32) -> Option<Vec<Part>> {
    let mut wide = glyphs
        .iter()
        .filter(|(_, g)| g.width() > MAX_COMMA_WIDTH)
        .map(|(_, g)| ink_span(g));
    let (top, bottom) = wide.next()??;
    if wide.any(|span| span != Some((top, bottom))) {
        return None;
    }
    let parts: Vec<Part> = glyphs
        .iter()
        .map(|(_, g)| {
            let (t, b) = ink_span(g)?;
            if g.width() > MAX_COMMA_WIDTH {
                (g.width() <= MAX_DIGIT_WIDTH).then_some(Part::Digit)
            } else {
                (t > top + (bottom - top) / 2 && b > bottom).then_some(Part::Comma)
            }
        })
        .collect::<Option<_>>()?;
    let spaced = glyphs
        .windows(2)
        .all(|w| (1..=max_gap).contains(&(w[1].0 - (w[0].0 + w[0].1.width()))));
    let mut groups = vec![0usize];
    for part in &parts {
        match part {
            Part::Digit => *groups.last_mut()? += 1,
            Part::Comma => groups.push(0),
        }
    }
    let grouped = (1..=3).contains(&groups[0]) && groups[1..].iter().all(|&g| g == 3);
    (spaced && grouped).then_some(parts)
}

/// The amount a run of glyphs spells, named by their shapes and places alone (the shop's price
/// font, which isn't learned). `None` unless every digit is named and the run is a total.
fn spelled(glyphs: &[(u32, Glyph)]) -> Option<u64> {
    let parts = parts(glyphs, PRICE_MAX_GAP)?;
    let text: String = glyphs
        .iter()
        .zip(&parts)
        .map(|((_, g), part)| match part {
            Part::Comma => Some(','),
            Part::Digit => digit_shapes::guess(g),
        })
        .collect::<Option<_>>()?;
    amount(&text)
}

/// A total's text as a number: digits in comma groups, no leading zero but a lone "0".
fn amount(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(|&c| c != ',').collect();
    if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return None;
    }
    digits.parse().ok()
}

/// One buy-back row's amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Empty,
    Amount(u64),
}

/// The buy-back row whose price text is looked for from (`x`, `y`): empty, its amount, or `None`
/// when it can't be read: highlighted, under the cursor or a tooltip, or anything but a price.
fn price_row(px: &Pixels, x: i64, y: i64) -> Option<Row> {
    let region = Rect {
        x: u32::try_from(x).ok()?,
        y: u32::try_from(y).ok()?,
        width: PRICE_COLUMNS,
        height: PRICE_ROWS,
    };
    let bgra = px.copy(region)?;
    let frame = Frame::new(
        region.width,
        region.height,
        (region.width * 4) as usize,
        &bgra,
    )?;
    let ink = |p: [u8; 3]| near(p, SHOP.ink, 8);
    // The row's own light background and the price's ink, nothing else: a highlighted row, the
    // cursor's outline or a tooltip is something else.
    let light = |p: [u8; 3]| p.iter().all(|&c| c >= 0xa0);
    if !bgra
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| [p[2], p[1], p[0]])
        .all(|p| ink(p) || light(p))
    {
        return None;
    }
    let all = Rect {
        x: 0,
        y: 0,
        width: region.width,
        height: region.height,
    };
    let glyphs = segment(&frame, all, ink).ok()?;
    if glyphs.is_empty() {
        return Some(Row::Empty);
    }
    // The price, up to the space before "Mesos".
    let mut number = vec![glyphs[0].clone()];
    for pair in glyphs.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        if next.0 - (prev.0 + prev.1.width()) >= PRICE_END_GAP {
            break;
        }
        number.push(next.clone());
    }
    spelled(&number).map(Row::Amount)
}

/// The amounts in an NPC shop's buy-back rows, top to bottom, from the shop's meso box `shop`:
/// what each sale still listed paid. `None` unless the left pane shows its Buy Back tab and every
/// row reads (one empty row ends the list), so a row that can't be read checks nothing.
fn buy_back(px: &Pixels, shop: &Found) -> Option<Vec<u64>> {
    let (x0, top) = (i64::from(shop.left), i64::from(shop.top));
    let tab = BUY_BACK_TAB;
    let pink = (0..i64::from(tab.len))
        .filter(|dx| px.get(x0 + tab.dx + dx, top + tab.dy).is_some_and(is_pink))
        .count();
    // Under the All tab the pane lists the shop's goods, whose prices aren't sales.
    if pink * 2 < tab.len as usize {
        return None;
    }
    let mut amounts = Vec::new();
    let mut ended = false;
    for k in 0..BUY_BACK_ROWS {
        let (dx, dy) = BUY_BACK_PRICE;
        match price_row(px, x0 + dx, top + dy + k * BUY_BACK_STEP)? {
            Row::Amount(_) if ended => return None,
            Row::Amount(amount) => amounts.push(amount),
            Row::Empty => ended = true,
        }
    }
    Some(amounts)
}

/// Whether a rise in the shop's total agrees with the sales the buy-back rows list: `before` and
/// `now` are the rows read with the last trusted total and this one. Only rows added to the end
/// of the same list are this rise's sales; any other change (rows expired, scrolled, the pane
/// switched) checks nothing (`None`). Nor does a full list: more sales may be past its last row.
fn sale_agrees(rise: u64, before: &[u64], now: &[u64]) -> Option<bool> {
    let full = now.len() >= BUY_BACK_ROWS as usize;
    (now.len() > before.len() && !full && now.starts_with(before))
        .then(|| now[before.len()..].iter().sum::<u64>() == rise)
}

/// What a whole frame showed of the meso total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MesoView {
    /// No inventory, shop or storage window on screen.
    NotOpen,
    /// A window's meso box is in view but can't be read: the cursor, a tooltip or another window
    /// is over it.
    Covered,
    /// A window is partly off the edge of the game area, its box or tab row with it.
    OffScreen,
    /// Another window is over part of a window: its tab row is cut short.
    PartlyCovered,
    /// The box shows a shape not named yet: it's named once seen the same in two frames.
    Learning(MesoSource),
    /// The box was read.
    Read { source: MesoSource, total: u64 },
    /// The game area isn't a size the box has been checked at.
    Unsupported,
}

/// How the meso total is read, as plugins hear it (the SDK's `MesoState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MesoState {
    /// It's read whenever a window showing it is open long enough.
    Reading,
    /// Its digits are still being learned: a box in view for a moment longer names them.
    Learning,
    /// It can't be read at this game size.
    Unsupported,
}

/// What one frame brought.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MesoReading {
    pub view: MesoView,
    /// A trusted total to send as `MesoChanged`: read the same in two frames in a row, and new
    /// (another total or window than the last one sent, or the first since a window came into
    /// view).
    pub send: Option<(u64, MesoSource)>,
    /// A rise in a shop's total checked against the buy-back rows it added: whether they agreed.
    /// `None` when there was nothing to check, or the rows couldn't be read.
    pub sale: Option<bool>,
}

/// A trusted shop total and the buy-back rows read with it.
type ShopReading = (u64, Option<Vec<u64>>);

/// What a box's glyphs came to.
enum Read {
    Total(u64),
    /// A shape not named yet.
    Learning,
    /// A shape the naming can't take, or a total no game would show: the naming may be wrong.
    Conflict,
}

/// The first line of [`MesoReader::knowledge`]'s text. A reader refuses any other.
const KNOWLEDGE_HEADER: &str = "mesovibe meso-total 1";
/// The names a box's shapes can have.
const NAMES: &str = "0123456789,";

/// Reads the meso total from whole frames, with digit shapes named on the player's device.
#[derive(Debug, Clone, Default)]
pub struct MesoReader {
    names: Atlas,
    /// Shapes seen and not named yet, each with the whole frame it was first seen in.
    held: Vec<(Glyph, u64)>,
    /// Whole frames looked at.
    frames: u64,
    /// Frames since the last whole frame. A new reader starts at 0: its first frames are the
    /// bottom rows, which tell the game's size before it asks for a whole frame.
    since: u32,
    /// The last whole frame showed a meso box, read or covered.
    in_view: bool,
    /// The last frame's game area isn't a checked size.
    unsupported: bool,
    last_view: Option<MesoView>,
    /// What the last whole frame read, with the buy-back rows read with a shop total.
    last: Option<(MesoSource, u64, Option<Vec<u64>>)>,
    /// The last total sent, and whether one was sent since a window came into view.
    sent: Option<(MesoSource, u64)>,
    sent_this_look: bool,
    /// The last trusted shop total while the shop stays in view.
    shop: Option<ShopReading>,
    /// Whole frames in a row whose box showed a conflict.
    conflicts: u32,
}

impl MesoReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the next frame should hold the whole game area, which is `area` game pixels:
    /// every frame while a meso box is in view, so a short look at the inventory counts, and
    /// about once a second ([`LOOK_EVERY`]) otherwise. Never at a size it can't read.
    pub fn wants_whole_frame(&self, area: (u32, u32)) -> bool {
        SIZES.contains(&area) && !self.unsupported && (self.in_view || self.since + 1 >= LOOK_EVERY)
    }

    /// How the total is read: unsupported at a size it can't read, learning while it knows no
    /// shape or the last box showed one not named yet, else reading.
    pub fn state(&self) -> MesoState {
        if self.unsupported {
            MesoState::Unsupported
        } else if self.names.is_empty() || matches!(self.last_view, Some(MesoView::Learning(_))) {
            MesoState::Learning
        } else {
            MesoState::Reading
        }
    }

    /// What the last whole frame showed.
    pub fn last_view(&self) -> Option<MesoView> {
        self.last_view
    }

    /// Follows one frame the engine read. A frame holding only the bottom rows is counted toward
    /// the next whole frame, and `None` comes back; a whole frame is looked at. `blocks`, at a
    /// backing scale: where the game's pixel blocks start in this frame, as the HUD reader found
    /// ([`super::exp_bar::ExpBarReader::block_offset`]).
    pub fn observe(&mut self, frame: &Frame, blocks: Option<(u32, u32)>) -> Option<MesoReading> {
        let scale = frame.scale();
        let area = (frame.width() / scale, frame.height() / scale);
        self.unsupported = !SIZES.contains(&area) || area.1 > frame.source();
        if frame.first_row() != 0 {
            self.since = self.since.saturating_add(1);
            return None;
        }
        self.since = 0;
        self.frames += 1;
        // Only a whole frame: the boxes can be anywhere in it.
        let pixels =
            Pixels::of(*frame, blocks, HUD_ROWS).filter(|px| px.top == 0 && !self.unsupported);
        let reading = match pixels {
            Some(px) => self.look(&px),
            None => {
                self.end_look();
                // At a checked size, a frame whose pixel blocks weren't found (a loading screen,
                // say) shows nothing readable; it doesn't make the size unsupported, which would
                // flip the state players see until the next strip.
                let view = if self.unsupported {
                    MesoView::Unsupported
                } else {
                    MesoView::NotOpen
                };
                MesoReading {
                    view,
                    send: None,
                    sale: None,
                }
            }
        };
        self.in_view = matches!(
            reading.view,
            MesoView::Read { .. } | MesoView::Learning(_) | MesoView::Covered
        );
        self.last_view = Some(reading.view);
        Some(reading)
    }

    /// Starts afresh what follows from frame to frame, keeping the shapes named: the game was
    /// restarted (maybe another character), or capture paused and resumed, so the next total is
    /// sent even if it's the last one sent. Looks at the next frame.
    pub fn new_look(&mut self) {
        self.end_look();
        self.sent = None;
        self.in_view = false;
        self.since = LOOK_EVERY;
    }

    /// For when the game's picture changed size or scale: shapes seen once are forgotten, and the
    /// shapes named are checked again by the frames to come. If the box keeps showing shapes the
    /// naming can't take, it's dropped and learned again.
    pub fn recheck(&mut self) {
        self.held.clear();
        self.conflicts = 0;
        self.new_look();
    }

    /// The shapes named, as text to keep on the player's device so the next session reads from
    /// its first frame ([`MesoReader::from_knowledge`]). `None` while none is named. They never
    /// leave the device.
    pub fn knowledge(&self) -> Option<String> {
        if self.names.is_empty() {
            return None;
        }
        let mut text = format!("{KNOWLEDGE_HEADER}\n");
        for (name, glyph) in self.names.entries() {
            text.push_str(&format!("shape {name} {}\n", glyph.encode()));
        }
        Some(text)
    }

    /// A reader that starts from [`MesoReader::knowledge`] kept in an earlier session. `None` for
    /// text it couldn't have written.
    pub fn from_knowledge(text: &str) -> Option<MesoReader> {
        let mut lines = text.lines();
        if lines.next()? != KNOWLEDGE_HEADER {
            return None;
        }
        let mut reader = MesoReader::new();
        for line in lines.filter(|l| !l.is_empty()) {
            let parts: Vec<&str> = line.split(' ').collect();
            let ["shape", name, glyph] = parts[..] else {
                return None;
            };
            let glyph = Glyph::decode(glyph).filter(|g| g.height() == INSIDE_ROWS)?;
            if name.chars().count() != 1 || !NAMES.contains(name) {
                return None;
            }
            reader.names.learn(&[glyph], name).ok()?;
        }
        (!reader.names.is_empty()).then_some(reader)
    }

    fn end_look(&mut self) {
        self.last = None;
        self.sent_this_look = false;
        self.shop = None;
    }

    fn drop_naming(&mut self) {
        self.names = Atlas::default();
        self.held.clear();
        self.conflicts = 0;
        self.last = None;
        self.shop = None;
    }

    /// Looks at a whole frame: finds the box, reads it, and follows trust from frame to frame.
    fn look(&mut self, px: &Pixels) -> MesoReading {
        let (view, rows) = self.see(px);
        let read = match view {
            MesoView::Read { source, total } => Some((source, total)),
            _ => None,
        };
        let before = self.last.take();
        if view == MesoView::NotOpen {
            self.end_look();
        }
        let Some((source, total)) = read else {
            return MesoReading {
                view,
                send: None,
                sale: None,
            };
        };
        let trusted = before
            .as_ref()
            .is_some_and(|(s, t, _)| (*s, *t) == (source, total));
        // The buy-back rows read with this total, in this frame or the one before: a dialog or a
        // tooltip over them in one of the two leaves the other's.
        let rows_read = match (&rows, &before) {
            (Some(rows), _) => Some(rows.clone()),
            (None, Some((_, _, rows))) if trusted => rows.clone(),
            (None, _) => None,
        };
        self.last = Some((source, total, rows));
        if !trusted {
            return MesoReading {
                view,
                send: None,
                sale: None,
            };
        }
        let mut sale = None;
        if source == MesoSource::Shop {
            let earlier = self.shop.take();
            // Rows that didn't read this time leave the ones read with the same total.
            let kept = match &earlier {
                Some((was, rows)) if *was == total && rows_read.is_none() => rows.clone(),
                _ => rows_read.clone(),
            };
            self.shop = Some((total, kept));
            sale = match (earlier, &rows_read) {
                (Some((was, Some(before))), Some(now)) if total > was => {
                    sale_agrees(total - was, &before, now)
                }
                _ => None,
            };
            if sale == Some(false) {
                // The rise isn't what the buy-back rows say the sale paid: the digits' naming is
                // wrong somewhere, so the total isn't trusted and the digits are learned again.
                self.drop_naming();
                return MesoReading {
                    view: MesoView::Learning(source),
                    send: None,
                    sale,
                };
            }
        } else {
            self.shop = None;
        }
        let new = !self.sent_this_look || self.sent != Some((source, total));
        self.sent = Some((source, total));
        self.sent_this_look = true;
        MesoReading {
            view,
            send: new.then_some((total, source)),
            sale,
        }
    }

    /// What the frame shows, with the buy-back rows read beside a shop total.
    fn see(&mut self, px: &Pixels) -> (MesoView, Option<Vec<u64>>) {
        let mut off_screen = false;
        let mut cut = false;
        // A shop or storage box set aside for its underline (cut short or off the frame).
        let mut npc_set_aside = false;
        let mut candidates: Vec<(Kind, Found)> = Vec::new();
        for b in find_boxes(px) {
            let Some(kind) = kind_of(px, &b) else {
                continue;
            };
            let (lx, ly) = (
                i64::from(b.left) + kind.line.dx,
                i64::from(b.top) + kind.line.dy,
            );
            let npc = kind.source != MesoSource::Inventory;
            match line_at(px, lx, ly, kind.line.len) {
                LineCheck::Full => {}
                LineCheck::OffFrame => {
                    off_screen = true;
                    npc_set_aside |= npc;
                    continue;
                }
                LineCheck::Cut => {
                    cut = true;
                    npc_set_aside |= npc;
                    continue;
                }
            }
            if kind.source == MesoSource::Storage {
                // Only the box for the mesos on hand, whose pane has the stored mesos' pane left
                // of it, is read; the stored mesos' box never is.
                let len = kind.line.len;
                let stored = line_at(px, lx - STORAGE_PANES, ly, len);
                let on_hand = line_at(px, lx + STORAGE_PANES, ly, len);
                match (stored, on_hand) {
                    (LineCheck::Full, LineCheck::Full) => continue,
                    (LineCheck::Full, _) => {}
                    (_, LineCheck::Full) => continue,
                    (LineCheck::OffFrame, _) => {
                        off_screen = true;
                        npc_set_aside = true;
                        continue;
                    }
                    _ => {
                        cut = true;
                        npc_set_aside = true;
                        continue;
                    }
                }
            }
            candidates.push((kind, b));
        }
        // The shop first, where a rise is a sale; then storage, where a change is a transfer.
        candidates.sort_by_key(|(kind, _)| match kind.source {
            MesoSource::Shop => 0,
            MesoSource::Storage => 1,
            MesoSource::Inventory => 2,
        });
        // Only the first box is read. A shop or storage window open with its box not read (not
        // found, set aside for its underline, or under the cursor or a tooltip) keeps the frame
        // from being read: the inventory's total beside it would send a sale or a transfer as
        // mesos picked up.
        let Some((kind, b)) = candidates.into_iter().next() else {
            return (why(px, off_screen, cut), None);
        };
        if kind.source == MesoSource::Inventory {
            if npc_set_aside {
                return (why(px, off_screen, cut), None);
            }
            if npc_window_open(px) {
                return (MesoView::Covered, None);
            }
        }
        let glyphs = self.glyphs(px, &b);
        let Some((glyphs, parts)) = glyphs.and_then(|g| parts(&g, MAX_GAP).map(|p| (g, p))) else {
            return (MesoView::Covered, None);
        };
        let source = kind.source;
        match self.name(&glyphs, &parts) {
            Read::Total(total) => {
                self.conflicts = 0;
                let rows = (source == MesoSource::Shop)
                    .then(|| buy_back(px, &b))
                    .flatten();
                (MesoView::Read { source, total }, rows)
            }
            Read::Learning => (MesoView::Learning(source), None),
            Read::Conflict => {
                self.conflicts += 1;
                if self.conflicts >= CONFLICT_PATIENCE {
                    self.drop_naming();
                }
                (MesoView::Learning(source), None)
            }
        }
    }

    /// The glyphs in a clean box's inside rows.
    fn glyphs(&self, px: &Pixels, b: &Found) -> Option<Vec<(u32, Glyph)>> {
        if !b.clean {
            return None;
        }
        let region = Rect {
            x: b.left + 1,
            y: b.top + 1,
            width: b.width() - 2,
            height: INSIDE_ROWS,
        };
        let bgra = px.copy(region)?;
        let frame = Frame::new(
            region.width,
            region.height,
            (region.width * 4) as usize,
            &bgra,
        )?;
        let all = Rect {
            x: 0,
            y: 0,
            ..region
        };
        let glyphs = segment(&frame, all, |p| !is_white(p)).ok()?;
        (!glyphs.is_empty()).then_some(glyphs)
    }

    /// Names what it can of a box's shapes and reads the box when every one is named. A shape is
    /// named the second time it's seen, in another frame than the first: a comma by its place, a
    /// digit by its shape.
    fn name(&mut self, glyphs: &[(u32, Glyph)], parts: &[Part]) -> Read {
        let (mut unnamed, mut conflict) = (false, false);
        for ((_, glyph), part) in glyphs.iter().zip(parts) {
            if self.names.name(glyph).is_some() {
                continue;
            }
            let seen = self.held.iter().position(|(g, _)| g == glyph);
            match seen {
                Some(i) if self.held[i].1 != self.frames => {
                    let name = match part {
                        Part::Comma => Some(','),
                        Part::Digit => digit_shapes::guess(glyph),
                    };
                    match name {
                        Some(name)
                            if self
                                .names
                                .learn(std::slice::from_ref(glyph), &name.to_string())
                                .is_ok() =>
                        {
                            self.held.remove(i);
                        }
                        Some(_) => conflict = true,
                        None => unnamed = true,
                    }
                }
                Some(_) => unnamed = true,
                None => {
                    if self.held.len() >= MAX_HELD {
                        self.held.remove(0);
                    }
                    self.held.push((glyph.clone(), self.frames));
                    unnamed = true;
                }
            }
        }
        if conflict {
            return Read::Conflict;
        }
        if unnamed {
            return Read::Learning;
        }
        let text: Option<String> = glyphs
            .iter()
            .zip(parts)
            .map(|((_, g), part)| {
                let name = self.names.name(g)?;
                // A shape named as a comma in a digit's place, or the other way round.
                ((name == ',') == (*part == Part::Comma)).then_some(name)
            })
            .collect();
        match text.as_deref().and_then(amount) {
            Some(total) => Read::Total(total),
            None => Read::Conflict,
        }
    }
}

/// Whether a shop's or storage's tab underline shows whole: that window is open.
fn npc_window_open(px: &Pixels) -> bool {
    red_lines(px).iter().any(|line| {
        !line.at_edge
            && KINDS
                .iter()
                .any(|k| k.source != MesoSource::Inventory && k.line.len == line.len)
    })
}

/// Why no box read. A meso box found whole with its own underline off the frame or cut short
/// (`off_screen`, `cut`) says so first. Otherwise the red underlines tell: one of a meso window's
/// in full means its box is covered (or off the frame, when that's where the box would be); one
/// running into the frame's edge, a window partly off screen; one cut short elsewhere, a window
/// partly covered.
fn why(px: &Pixels, off_screen: bool, cut: bool) -> MesoView {
    if off_screen {
        return MesoView::OffScreen;
    }
    if cut {
        return MesoView::PartlyCovered;
    }
    let longest = KINDS.iter().map(|k| k.line.len).max().unwrap_or(0);
    let (mut covered, mut off_screen, mut cut) = (false, false, false);
    for line in red_lines(px) {
        let full = KINDS.iter().find(|k| k.line.len == line.len);
        match full {
            Some(kind) if !line.at_edge => {
                let x = i64::from(line.x) - kind.line.dx;
                let y = i64::from(line.top) - kind.line.dy;
                let inside = x >= 0
                    && y >= 0
                    && x + i64::from(kind.width) <= i64::from(px.width)
                    && y + i64::from(BOX_ROWS) <= i64::from(px.height);
                match inside {
                    true => covered = true,
                    false => off_screen = true,
                }
            }
            _ if line.at_edge && line.len < longest => off_screen = true,
            _ if line.len < longest && !OTHER_LINES.contains(&line.len) => cut = true,
            _ => {}
        }
    }
    if covered {
        MesoView::Covered
    } else if off_screen {
        MesoView::OffScreen
    } else if cut {
        MesoView::PartlyCovered
    } else {
        MesoView::NotOpen
    }
}

/// Synthetic meso windows for tests, here and in the engine loop's: the box's border and inks are
/// the colors the reader looks for, and everything else (the digits, comma, coin, panels, cursor)
/// is invented. No game pixels are used.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::recognizers::digit_shapes::test_support::PLAIN;
    use crate::recognizers::exp_bar::test_support::Canvas;

    /// A window's panel: an invented grey-blue, neither white, navy nor ink.
    pub(crate) const PANEL: [u8; 3] = [0xa8, 0xb8, 0xc8];
    /// Another window drawn over one: an invented darker grey-blue.
    pub(crate) const OTHER_PANEL: [u8; 3] = [0x80, 0x90, 0xa4];
    const WHITE: [u8; 3] = [255, 255, 255];
    const RED: [u8; 3] = [0xd8, 0x10, 0x18];
    /// An active tab drawn pale over an underline's top row.
    const PALE: [u8; 3] = [0xe8, 0xb8, 0xb8];
    const PINK: [u8; 3] = [0xe8, 0x60, 0x80];
    const GREY: [u8; 3] = [0xbd, 0xbd, 0xbd];
    const COIN: [u8; 3] = [0xf0, 0xc0, 0x20];
    /// A buy-back row's light background, and one highlighted.
    const ROW: [u8; 3] = [0xc8, 0xd4, 0xe0];
    pub(crate) const HIGHLIGHT: [u8; 3] = [0xe8, 0x98, 0x30];
    /// An invented comma, two columns wide, hanging below the digits.
    const COMMA: [&str; 3] = [".#", ".#", "#."];
    /// An invented cursor: a black outline around a white arrow.
    pub(crate) const CURSOR: [&str; 12] = [
        "#.......", "##......", "#w#.....", "#ww#....", "#www#...", "#wwww#..", "#wwwww#.",
        "#www####", "#w#w#...", "##.#w#..", "#...#w#.", ".....##.",
    ];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Window {
        Inventory,
        Shop,
        Storage,
    }

    /// One meso window, drawn the way the game lays it out.
    #[derive(Debug, Clone)]
    pub(crate) struct MesoWindow {
        pub(crate) window: Window,
        pub(crate) total: u64,
        /// Storage: the mesos stored, in the left box.
        pub(crate) stored: u64,
        /// The meso box's top-left corner, border included (storage: the box for the mesos on
        /// hand). Parts may fall off the canvas.
        pub(crate) at: (i64, i64),
        /// Blank columns after a comma: 1 on a Mac, 2 on Windows.
        pub(crate) comma_gap: u32,
        /// The active tab drawn pale over the underline's top row, as when the window isn't
        /// focused.
        pub(crate) pale_tab: bool,
        /// Shop: the left pane's Buy Back tab, listing what these sales paid; `None` shows the
        /// All tab and the shop's goods.
        pub(crate) buy_back: Option<Vec<u64>>,
        /// Shop: buy-back rows drawn highlighted, which can't be read.
        pub(crate) highlighted: Vec<usize>,
    }

    impl MesoWindow {
        pub(crate) fn new(window: Window, total: u64) -> MesoWindow {
            let at = match window {
                Window::Inventory => (600, 500),
                Window::Shop => (800, 250),
                Window::Storage => (700, 520),
            };
            MesoWindow {
                window,
                total,
                stored: 1_200,
                at,
                comma_gap: 2,
                pale_tab: false,
                buy_back: None,
                highlighted: Vec::new(),
            }
        }

        pub(crate) fn at(mut self, x: i64, y: i64) -> MesoWindow {
            self.at = (x, y);
            self
        }

        fn kind(&self) -> Kind {
            match self.window {
                Window::Inventory => INVENTORY,
                Window::Shop => SHOP,
                Window::Storage => STORAGE,
            }
        }

        pub(crate) fn draw(&self, c: &mut Canvas) {
            let kind = self.kind();
            let (x0, top) = self.at;
            match self.window {
                Window::Inventory => fill(c, x0 - 30, top - 240, 220, 275, PANEL),
                Window::Shop => fill(c, x0 - 330, top - 80, 450, 420, PANEL),
                Window::Storage => {
                    fill(c, x0 - 41 - STORAGE_PANES - 20, top - 240, 440, 275, PANEL)
                }
            }
            let mut boxes = vec![(x0, self.total)];
            if self.window == Window::Storage {
                boxes.push((x0 - STORAGE_PANES, self.stored));
            }
            for (x, total) in boxes {
                draw_box(c, x, top, kind, total, self.comma_gap);
                let line = kind.line;
                fill(c, x + line.dx, top + line.dy, line.len, 3, RED);
                if self.pale_tab {
                    fill(c, x + line.dx + 60, top + line.dy, 40, 1, PALE);
                }
            }
            if self.window == Window::Shop {
                self.draw_left_pane(c);
            }
        }

        /// The shop's left pane: its Buy Back tab and six rows of prices.
        fn draw_left_pane(&self, c: &mut Canvas) {
            let (x0, top) = self.at;
            let tab = BUY_BACK_TAB;
            let color = if self.buy_back.is_some() { PINK } else { GREY };
            fill(c, x0 + tab.dx - 2, top + tab.dy - 6, tab.len + 4, 14, color);
            let goods = vec![80, 50, 150, 35, 1_000, 1_000];
            let amounts = self.buy_back.clone().unwrap_or(goods);
            for k in 0..BUY_BACK_ROWS {
                let (dx, dy) = BUY_BACK_PRICE;
                let (x, y) = (x0 + dx, top + dy + k * BUY_BACK_STEP);
                let background = match self.highlighted.contains(&(k as usize)) {
                    true => HIGHLIGHT,
                    false => ROW,
                };
                fill(c, x - 7, y - 3, 110, 20, background);
                if let Some(&amount) = amounts.get(k as usize) {
                    let end = text(c, x + 2, y + 2, &digits(amount), 2, SHOP.ink);
                    // The word "Mesos", as invented marks after a space.
                    for i in 0..4 {
                        fill(c, end + 6 + i * 5, y + 2, 3, 7, SHOP.ink);
                    }
                }
            }
        }
    }

    /// `n` with its digits grouped by commas, as the game writes a total.
    pub(crate) fn digits(n: u64) -> String {
        let plain = n.to_string();
        let mut out = String::new();
        for (i, ch) in plain.chars().enumerate() {
            if i > 0 && (plain.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(ch);
        }
        out
    }

    pub(crate) fn put(c: &mut Canvas, x: i64, y: i64, color: [u8; 3]) {
        if (0..i64::from(c.width)).contains(&x) && (0..i64::from(c.height)).contains(&y) {
            c.set(x as u32, y as u32, color);
        }
    }

    pub(crate) fn fill(c: &mut Canvas, x: i64, y: i64, w: u32, h: u32, color: [u8; 3]) {
        for yy in y..y + i64::from(h) {
            for xx in x..x + i64::from(w) {
                put(c, xx, yy, color);
            }
        }
    }

    /// Draws `text` (digits and commas) with its digits' top row at `y`, from `x`: one blank
    /// column between glyphs and `comma_gap` after a comma. Returns the column after the last.
    fn text(c: &mut Canvas, mut x: i64, y: i64, text: &str, comma_gap: u32, ink: [u8; 3]) -> i64 {
        for ch in text.chars() {
            let (rows, dy, gap): (Vec<&str>, i64, u32) = match ch {
                ',' => (COMMA.to_vec(), 5, comma_gap),
                d => {
                    let (_, rows) = PLAIN.iter().find(|(p, _)| *p == d).expect("a digit");
                    (rows.to_vec(), 0, 1)
                }
            };
            for (r, row) in rows.iter().enumerate() {
                for (col, cell) in row.chars().enumerate() {
                    if cell == '#' {
                        put(c, x + col as i64, y + dy + r as i64, ink);
                    }
                }
            }
            x += rows[0].len() as i64 + i64::from(gap);
        }
        x
    }

    /// The width `text` takes, as [`text`] draws it.
    fn text_width(text: &str, comma_gap: u32) -> i64 {
        let gaps: i64 = text
            .chars()
            .map(|ch| match ch {
                ',' => 2 + i64::from(comma_gap),
                _ => 6,
            })
            .sum();
        gaps - if text.ends_with(',') {
            i64::from(comma_gap)
        } else {
            1
        }
    }

    /// A meso box of `kind` at (`x0`, `top`) with `total` right-aligned in it, and what sits left
    /// of it.
    pub(super) fn draw_box(
        c: &mut Canvas,
        x0: i64,
        top: i64,
        kind: Kind,
        total: u64,
        comma_gap: u32,
    ) {
        let w = kind.width;
        let right = x0 + i64::from(w) - 1;
        fill(c, x0 + 1, top, w - 2, 1, NAVY);
        fill(c, x0 + 1, top + 15, w - 2, 1, NAVY);
        fill(c, x0, top + 1, 1, 14, NAVY);
        fill(c, right, top + 1, 1, 14, NAVY);
        fill(c, x0 + 1, top + 1, w - 2, 14, WHITE);
        let shown = digits(total);
        let start = right - 3 - text_width(&shown, comma_gap);
        text(c, start, top + 4, &shown, comma_gap, kind.ink);
        match kind.left {
            Left::Coin => {
                fill(c, x0 - 16, top + 1, 14, 14, WHITE);
                fill(c, x0 - 13, top + 4, 8, 8, COIN);
            }
            Left::Framed(width) => {
                let (r, l) = (x0 - 2, x0 - 1 - i64::from(width));
                fill(c, l + 1, top, width - 2, 1, NAVY);
                fill(c, l + 1, top + 15, width - 2, 1, NAVY);
                fill(c, l, top + 1, 1, 14, NAVY);
                fill(c, r, top + 1, 1, 14, NAVY);
                fill(c, l + 1, top + 1, width - 2, 14, WHITE);
                fill(c, l + 4, top + 4, width - 8, 8, COIN);
            }
            Left::Other => {}
        }
    }

    /// The invented cursor with its tip at (`x`, `y`).
    pub(crate) fn draw_cursor(c: &mut Canvas, x: i64, y: i64) {
        for (r, row) in CURSOR.iter().enumerate() {
            for (col, cell) in row.chars().enumerate() {
                let color = match cell {
                    '#' => [0, 0, 0],
                    'w' => WHITE,
                    _ => continue,
                };
                put(c, x + col as i64, y + r as i64, color);
            }
        }
    }

    /// An invented tooltip: a dark blue box with a light border and light marks, its top-left at
    /// (`x`, `y`).
    pub(crate) fn draw_tooltip(c: &mut Canvas, x: i64, y: i64) {
        fill(c, x, y, 120, 60, [0xc0, 0xc8, 0xe0]);
        fill(c, x + 1, y + 1, 118, 58, [0x30, 0x38, 0x70]);
        fill(c, x + 8, y + 8, 60, 6, [0xf0, 0xf0, 0xf8]);
    }

    /// A game area with `windows` drawn over the invented scenery.
    pub(crate) fn game(width: u32, height: u32, windows: &[&MesoWindow]) -> Canvas {
        let mut c = Canvas::new(width, height);
        fill(&mut c, 0, 0, width, height - 40, [0x60, 0x90, 0x60]);
        for window in windows {
            window.draw(&mut c);
        }
        c
    }

    /// `canvas` as a Retina Mac shows it: every pixel a 2x2 block, the blocks starting `phase`
    /// pixels in.
    pub(crate) fn doubled(canvas: &Canvas, (px, py): (u32, u32)) -> Vec<u8> {
        let (w, h) = (canvas.width * 2, canvas.height * 2);
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let (gx, gy) = (x.saturating_sub(px) / 2, y.saturating_sub(py) / 2);
                let from = ((gy.min(canvas.height - 1) * canvas.width + gx.min(canvas.width - 1))
                    * 4) as usize;
                let to = ((y * w + x) * 4) as usize;
                bgra[to..to + 4].copy_from_slice(&canvas.bgra[from..from + 4]);
            }
        }
        bgra
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::recognizers::exp_bar::test_support::Canvas;

    /// A total with every digit, so a reader shown it twice knows them all.
    const EVERY_DIGIT: u64 = 1_234_567_890;

    fn look(reader: &mut MesoReader, canvas: &Canvas) -> MesoReading {
        reader
            .observe(&canvas.frame(), None)
            .expect("a whole frame is looked at")
    }

    fn inventory(total: u64) -> MesoWindow {
        MesoWindow::new(Window::Inventory, total)
    }

    fn one(window: &MesoWindow) -> Canvas {
        game(1366, 768, &[window])
    }

    /// A reader that has named every digit and the comma, from two frames of the inventory.
    fn taught() -> MesoReader {
        let mut reader = MesoReader::new();
        let canvas = one(&inventory(EVERY_DIGIT));
        look(&mut reader, &canvas);
        look(&mut reader, &canvas);
        assert_eq!(reader.state(), MesoState::Reading);
        reader.new_look();
        reader
    }

    fn read(source: MesoSource, total: u64) -> MesoView {
        MesoView::Read { source, total }
    }

    // A box is found and read wherever the window is dragged, edges included. Fails if the
    // 14-row scan can step over a box, or a box near an edge is missed or read wrong.
    #[test]
    fn a_box_is_read_wherever_the_window_is() {
        let mut reader = taught();
        let (w, h) = (1366i64, 768i64);
        // The underline, 21 px left of the box and 225 rows above it, bounds where the whole
        // window is on screen; the box's bottom border, the bottom edge.
        let xs = [22, 23, 300, 701, w - 183];
        let tops: Vec<i64> = (225..225 + 14).chain([400, 555, h - 16]).collect();
        for &x in &xs {
            for &top in &tops {
                let window = inventory(5_170_316).at(x, top);
                let seen = look(&mut reader, &one(&window)).view;
                assert_eq!(
                    seen,
                    read(MesoSource::Inventory, 5_170_316),
                    "at ({x}, {top})"
                );
            }
        }
        // The shop and storage too, at the edges their windows allow.
        // The shop's underline runs 52 rows below its box and 99 px right of its left edge.
        for (x, top) in [(128, 0), (w - 101, h - 56), (700, 300)] {
            let shop = MesoWindow::new(Window::Shop, 98_765).at(x, top);
            let seen = look(&mut reader, &one(&shop)).view;
            assert_eq!(seen, read(MesoSource::Shop, 98_765), "shop at ({x}, {top})");
        }
        // Storage's underline runs 164 px right of the box's left edge.
        for (x, top) in [(42 + STORAGE_PANES, 225), (w - 166, h - 16)] {
            let storage = MesoWindow::new(Window::Storage, 5_206_000).at(x, top);
            let seen = look(&mut reader, &one(&storage)).view;
            assert_eq!(
                seen,
                read(MesoSource::Storage, 5_206_000),
                "storage at ({x}, {top})"
            );
        }
    }

    // The cursor or a tooltip over the box makes it covered, never a wrong total. Fails if any
    // placement reads a total other than the one shown, or sends one.
    #[test]
    fn a_cursor_or_tooltip_over_the_box_is_covered_never_a_wrong_total() {
        let mut reader = taught();
        let window = inventory(5_170_316);
        let (x0, top) = window.at;
        let base = one(&window);
        let mut canvas = one(&window);
        let mut covered = 0;
        // Every placement over the digits, and every few elsewhere over the box.
        let placements = (85..146)
            .flat_map(|dx| (-2..13).step_by(2).map(move |dy| (dx, dy)))
            .chain(
                (-9..85)
                    .step_by(6)
                    .flat_map(|dx| (-12..18).step_by(3).map(move |dy| (dx, dy))),
            );
        for (dx, dy) in placements {
            draw_cursor(&mut canvas, x0 + dx, top + dy);
            let reading = look(&mut reader, &canvas);
            canvas.bgra.copy_from_slice(&base.bgra);
            match reading.view {
                MesoView::Read { total, .. } => assert_eq!(total, 5_170_316, "({dx}, {dy})"),
                MesoView::Covered => covered += 1,
                other => panic!("({dx}, {dy}): {other:?}"),
            }
            if let Some((total, _)) = reading.send {
                assert_eq!(total, 5_170_316);
            }
        }
        assert!(covered > 400, "most placements cover the box: {covered}");
        for (dx, dy) in [(-100, -40), (20, -30), (100, 8), (-60, 10)] {
            let mut canvas = one(&window);
            draw_tooltip(&mut canvas, x0 + dx, top + dy);
            assert_eq!(
                look(&mut reader, &canvas).view,
                MesoView::Covered,
                "({dx}, {dy})"
            );
        }
        // The shop's box, whose grey ink the cursor's black never matches.
        let shop = MesoWindow::new(Window::Shop, 5_171_242);
        let mut canvas = one(&shop);
        draw_cursor(&mut canvas, shop.at.0 + 60, shop.at.1 + 2);
        assert_eq!(look(&mut reader, &canvas).view, MesoView::Covered);
    }

    // With the inventory open beside a shop, a cursor over the shop's box makes the frame covered.
    // Fails if the inventory's total is read instead, which would send a sale as mesos picked up.
    #[test]
    fn a_covered_shop_box_is_never_read_as_the_inventory() {
        let mut reader = taught();
        let shop = MesoWindow::new(Window::Shop, 5_171_242).at(900, 300);
        let inv = inventory(5_171_242).at(150, 450);
        let canvas = game(1366, 768, &[&shop, &inv]);
        assert_eq!(
            look(&mut reader, &canvas).view,
            read(MesoSource::Shop, 5_171_242)
        );
        let mut covered = canvas.clone();
        draw_cursor(&mut covered, shop.at.0 + 60, shop.at.1 + 2);
        assert_eq!(look(&mut reader, &covered).view, MesoView::Covered);
        // A tooltip over the end of the shop's underline, its box still clean.
        let mut cut = canvas.clone();
        draw_tooltip(&mut cut, 760, 330);
        let seen = look(&mut reader, &cut).view;
        assert!(!matches!(seen, MesoView::Read { .. }), "{seen:?}");
    }

    // A window dragged half off the game area's edge is reported partly off screen, and its box
    // isn't read. Fails if a cut box is read or the window is called closed.
    #[test]
    fn a_window_half_off_screen_is_reported_and_not_read() {
        let mut reader = taught();
        for at in [(1300, 500), (-60, 500), (1200, 500)] {
            let window = inventory(5_170_316).at(at.0, at.1);
            assert_eq!(
                look(&mut reader, &one(&window)).view,
                MesoView::OffScreen,
                "{at:?}"
            );
        }
        // Dragged so far up that its tab row is above the frame: the box shows whole, but the
        // window it's in can't be told.
        let high = inventory(5_170_316).at(600, 100);
        assert_eq!(look(&mut reader, &one(&high)).view, MesoView::OffScreen);
    }

    // Another window over part of the inventory cuts its underline short: partly covered. Fails
    // if the cut box is read or the inventory is called closed or off screen.
    #[test]
    fn a_window_over_part_of_the_inventory_is_partly_covered() {
        let mut reader = taught();
        let window = inventory(5_170_316);
        let (x0, top) = window.at;
        let mut canvas = one(&window);
        fill(&mut canvas, x0 + 90, top - 260, 300, 300, OTHER_PANEL);
        assert_eq!(look(&mut reader, &canvas).view, MesoView::PartlyCovered);
        // A window over the box alone, the underline whole: open, its box covered.
        let mut canvas = one(&window);
        fill(&mut canvas, x0 - 40, top - 20, 260, 60, OTHER_PANEL);
        assert_eq!(look(&mut reader, &canvas).view, MesoView::Covered);
        // Closed: nothing.
        assert_eq!(
            look(&mut reader, &game(1366, 768, &[])).view,
            MesoView::NotOpen
        );
    }

    // When the window isn't focused its active tab is drawn pale over the underline's top row.
    // Fails if the underline is measured on its top row, which loses every such frame.
    #[test]
    fn a_pale_tab_over_the_underline_still_reads() {
        let mut reader = taught();
        for window in [Window::Inventory, Window::Shop, Window::Storage] {
            let mut pale = MesoWindow::new(window, 5_200_326);
            pale.pale_tab = true;
            let seen = look(&mut reader, &one(&pale)).view;
            assert!(
                matches!(
                    seen,
                    MesoView::Read {
                        total: 5_200_326,
                        ..
                    }
                ),
                "{window:?}: {seen:?}"
            );
        }
    }

    // Storage shows two boxes: the mesos on hand are read as source "storage"; the mesos stored
    // never are, even with the other box covered. Fails if the stored total is ever read.
    #[test]
    fn storage_reads_the_mesos_on_hand_and_never_those_stored() {
        let mut reader = taught();
        let storage = MesoWindow::new(Window::Storage, 5_206_000);
        let canvas = one(&storage);
        assert_eq!(
            look(&mut reader, &canvas).view,
            read(MesoSource::Storage, 5_206_000)
        );
        assert_eq!(
            look(&mut reader, &canvas).send,
            Some((5_206_000, MesoSource::Storage))
        );
        let (x0, top) = storage.at;
        let mut covered = one(&storage);
        draw_cursor(&mut covered, x0 + 40, top + 3);
        let seen = look(&mut reader, &covered).view;
        assert_eq!(
            seen,
            MesoView::Covered,
            "the stored box is never read instead"
        );
        // Storage with its left pane off the frame: the box can't be told from the stored one.
        let left = MesoWindow::new(Window::Storage, 5_206_000).at(150, 520);
        let seen = look(&mut reader, &one(&left)).view;
        assert!(!matches!(seen, MesoView::Read { .. }), "{seen:?}");
    }

    // Boxes in other windows (skills, stats, an inventory-wide box without its coin) never
    // yield a total. Fails if any is read.
    #[test]
    fn other_windows_boxes_are_never_read() {
        let mut reader = taught();
        let (x0, top) = (600i64, 500i64);
        let mut taller = game(1366, 768, &[]);
        draw_box(&mut taller, x0, top, INVENTORY, 3, 2);
        fill(&mut taller, x0 + 1, top + 15, 143, 1, [255, 255, 255]);
        fill(&mut taller, x0 + 1, top + 16, 143, 1, NAVY);
        fill(&mut taller, x0, top + 15, 1, 1, NAVY);
        fill(&mut taller, x0 + 144, top + 15, 1, 1, NAVY);
        fill(&mut taller, x0 - 21, top - 225, 191, 3, [0xd8, 0x10, 0x18]);
        let mut black = game(1366, 768, &[]);
        draw_box(&mut black, x0, top, INVENTORY, 3, 2);
        fill(&mut black, x0 + 1, top, 143, 1, [0, 0, 0]);
        fill(&mut black, x0 + 1, top + 15, 143, 1, [0, 0, 0]);
        let mut no_coin = one(&inventory(5_170_316));
        fill(&mut no_coin, x0 - 20, top, 20, 16, PANEL);
        for (name, canvas) in [("taller", taller), ("black", black), ("no coin", no_coin)] {
            for _ in 0..2 {
                let reading = look(&mut reader, &canvas);
                assert!(
                    !matches!(reading.view, MesoView::Read { .. }),
                    "{name}: {:?}",
                    reading.view
                );
                assert_eq!(reading.send, None, "{name}");
            }
        }
        // The skills window's full underline alone is no meso window.
        let mut skills = game(1366, 768, &[]);
        fill(&mut skills, 700, 270, 191, 3, [0xd8, 0x10, 0x18]);
        assert_eq!(look(&mut reader, &skills).view, MesoView::NotOpen);
    }

    // A digit shape is named only after its exact bitmap is seen in two different frames, and a
    // total is read only once every shape in it is named. Fails if one sighting names a shape.
    #[test]
    fn a_shape_seen_once_is_held_and_named_the_second_time() {
        let mut reader = MesoReader::new();
        assert_eq!(reader.state(), MesoState::Learning);
        let at = |n: u64| one(&inventory(n));
        assert_eq!(
            look(&mut reader, &at(55)).view,
            MesoView::Learning(MesoSource::Inventory)
        );
        assert_eq!(reader.knowledge(), None, "nothing named from one frame");
        // 5 seen again in another frame is named; 6 is seen once.
        assert_eq!(
            look(&mut reader, &at(56)).view,
            MesoView::Learning(MesoSource::Inventory)
        );
        assert!(reader.knowledge().is_some_and(|k| k.lines().count() == 2));
        assert_eq!(
            look(&mut reader, &at(56)).view,
            read(MesoSource::Inventory, 56)
        );
        assert_eq!(reader.state(), MesoState::Reading);
        // The same shape twice in one frame is one sighting.
        let mut fresh = MesoReader::new();
        look(&mut fresh, &at(77));
        assert_eq!(fresh.knowledge(), None);
    }

    // A total is sent only once two frames in a row read it the same, and once per look at an
    // unchanged total. Fails if a single read is sent, or the same total is sent every frame.
    #[test]
    fn a_total_is_sent_only_after_two_reads_agree() {
        let mut reader = taught();
        let a = one(&inventory(5_170_316));
        let b = one(&inventory(5_170_369));
        let closed = game(1366, 768, &[]);
        assert_eq!(look(&mut reader, &a).send, None, "one read");
        assert_eq!(look(&mut reader, &b).send, None, "a different read");
        assert_eq!(look(&mut reader, &closed).send, None);
        assert_eq!(look(&mut reader, &b).send, None, "after a frame without it");
        assert_eq!(
            look(&mut reader, &b).send,
            Some((5_170_369, MesoSource::Inventory))
        );
        assert_eq!(look(&mut reader, &b).send, None, "unchanged while open");
        // Covered for a moment: the same look, nothing new.
        let mut covered = b.clone();
        draw_tooltip(&mut covered, 590, 490);
        assert_eq!(look(&mut reader, &covered).send, None);
        assert_eq!(look(&mut reader, &b).send, None);
        assert_eq!(look(&mut reader, &b).send, None);
        // Closed and opened again: a fresh reading, though unchanged.
        look(&mut reader, &closed);
        look(&mut reader, &b);
        assert_eq!(
            look(&mut reader, &b).send,
            Some((5_170_369, MesoSource::Inventory))
        );
        // The same total in the shop is a reading from another window.
        let shop = one(&MesoWindow::new(Window::Shop, 5_170_369));
        look(&mut reader, &shop);
        assert_eq!(
            look(&mut reader, &shop).send,
            Some((5_170_369, MesoSource::Shop))
        );
    }

    fn shop_with(total: u64, rows: &[u64]) -> MesoWindow {
        let mut shop = MesoWindow::new(Window::Shop, total);
        shop.buy_back = Some(rows.to_vec());
        shop
    }

    // A sale whose buy-back row disagrees with the shop's rise drops the naming; one that agrees
    // is sent. Fails if a disagreeing sale is trusted or an agreeing one drops the naming.
    #[test]
    fn a_sale_that_disagrees_with_its_buy_back_row_drops_the_naming() {
        let mut reader = taught();
        let show = |reader: &mut MesoReader, shop: &MesoWindow| {
            let canvas = one(shop);
            look(reader, &canvas);
            look(reader, &canvas)
        };
        assert_eq!(
            show(&mut reader, &shop_with(5_171_223, &[])).send,
            Some((5_171_223, MesoSource::Shop))
        );
        let agrees = show(&mut reader, &shop_with(5_171_242, &[19]));
        assert_eq!(agrees.send, Some((5_171_242, MesoSource::Shop)));
        let agrees = show(&mut reader, &shop_with(5_171_328, &[19, 2, 84]));
        assert_eq!(
            agrees.send,
            Some((5_171_328, MesoSource::Shop)),
            "two sales in one rise"
        );
        assert!(reader.knowledge().is_some());
        let disagrees = show(&mut reader, &shop_with(5_171_496, &[19, 2, 84, 186]));
        assert_eq!(disagrees.send, None);
        assert_eq!(disagrees.view, MesoView::Learning(MesoSource::Shop));
        assert_eq!(reader.knowledge(), None, "learned again from scratch");
        assert_eq!(reader.state(), MesoState::Learning);
    }

    // A buy-back row that can't be read (highlighted, the All tab, expired) checks nothing.
    // Fails if an unreadable or unrelated row drops the naming.
    #[test]
    fn a_buy_back_row_that_cant_be_read_checks_nothing() {
        let mut reader = taught();
        let show = |reader: &mut MesoReader, shop: &MesoWindow| {
            let canvas = one(shop);
            look(reader, &canvas);
            look(reader, &canvas).send
        };
        show(&mut reader, &shop_with(1_000, &[50]));
        let mut highlighted = shop_with(1_700, &[50, 600]);
        highlighted.highlighted = vec![1];
        assert_eq!(
            show(&mut reader, &highlighted),
            Some((1_700, MesoSource::Shop))
        );
        // The All tab lists the shop's goods.
        let goods = MesoWindow::new(Window::Shop, 2_000);
        assert_eq!(show(&mut reader, &goods), Some((2_000, MesoSource::Shop)));
        assert_eq!(
            show(&mut reader, &shop_with(2_500, &[50, 600])),
            Some((2_500, MesoSource::Shop))
        );
        // Rows that expired in the meantime: a shorter list.
        assert_eq!(
            show(&mut reader, &shop_with(3_100, &[600])),
            Some((3_100, MesoSource::Shop))
        );
        assert!(reader.knowledge().is_some_and(|k| k.lines().count() == 12));
        // A fall is a purchase, with no row to check.
        assert_eq!(
            show(&mut reader, &shop_with(2_350, &[600])),
            Some((2_350, MesoSource::Shop))
        );
    }

    // The buy-back rows read from where the shop puts them. Fails if a row is misread, an empty
    // row isn't the end of the list, or a highlighted row reads.
    #[test]
    fn reads_the_buy_back_rows() {
        let shop = shop_with(5_208_350, &[5_712, 814, 1_786, 116, 200]);
        let canvas = one(&shop);
        let frame = canvas.frame();
        let px = Pixels::of(frame, None, HUD_ROWS).unwrap();
        let mut found = find_boxes(&px);
        found.retain(|b| kind_of(&px, b) == Some(SHOP));
        assert_eq!(
            buy_back(&px, &found[0]),
            Some(vec![5_712, 814, 1_786, 116, 200])
        );
        let mut highlighted = shop.clone();
        highlighted.highlighted = vec![4];
        let canvas = one(&highlighted);
        let px = Pixels::of(canvas.frame(), None, HUD_ROWS).unwrap();
        assert_eq!(buy_back(&px, &found[0]), None);
        let goods = MesoWindow::new(Window::Shop, 5_208_350);
        let canvas = one(&goods);
        let px = Pixels::of(canvas.frame(), None, HUD_ROWS).unwrap();
        assert_eq!(buy_back(&px, &found[0]), None, "the All tab");
        assert_eq!(sale_agrees(500, &[19], &[19, 300]), Some(false));
        assert_eq!(sale_agrees(300, &[19], &[19, 300]), Some(true));
        assert_eq!(sale_agrees(500, &[19], &[300]), None, "not the same list");
        assert_eq!(sale_agrees(500, &[19, 300], &[19, 300]), None, "no new row");
        assert_eq!(
            sale_agrees(9, &[1, 2, 3, 4, 5], &[1, 2, 3, 4, 5, 6]),
            None,
            "a full list may hide sales past its last row"
        );
    }

    // After a size change, named shapes that still match read at once and ones that don't are
    // learned again. Fails if a size change loses a good naming or keeps a bad one.
    #[test]
    fn a_new_size_rechecks_what_was_named() {
        let mut reader = taught();
        reader.recheck();
        let small = game(1280, 720, &[&inventory(5_170_316).at(500, 400)]);
        assert_eq!(
            look(&mut reader, &small).view,
            read(MesoSource::Inventory, 5_170_316)
        );
        // A naming with a wrong 5 (one pixel off), kept from before: the real 5, named by its
        // shape, can't take the name, so after a moment the naming is learned again.
        let good = taught().knowledge().unwrap();
        let wrong: String = good
            .lines()
            .map(|line| match line.strip_prefix("shape 5 ") {
                Some(glyph) => {
                    let g = Glyph::decode(glyph).unwrap();
                    let mut rows: Vec<u32> = (0..g.height())
                        .map(|y| {
                            (0..g.width())
                                .filter(|&x| g.is_ink(x, y))
                                .fold(0, |m, x| m | 1 << x)
                        })
                        .collect();
                    rows[0] ^= 1;
                    format!(
                        "shape 5 {}\n",
                        Glyph::from_rows(g.width(), rows).unwrap().encode()
                    )
                }
                None => format!("{line}\n"),
            })
            .collect();
        let mut reader = MesoReader::from_knowledge(&wrong).unwrap();
        reader.recheck();
        let canvas = one(&inventory(5_555));
        let mut views = Vec::new();
        for _ in 0..CONFLICT_PATIENCE + 3 {
            views.push(look(&mut reader, &canvas).view);
        }
        assert!(
            views[..CONFLICT_PATIENCE as usize]
                .iter()
                .all(|v| !matches!(v, MesoView::Read { .. }))
        );
        assert_eq!(
            views.last(),
            Some(&read(MesoSource::Inventory, 5_555)),
            "{views:?}"
        );
    }

    // Only the checked sizes are read; any other game area, Full Screen's scaled pictures
    // included, is unsupported and asks for no whole frames. Fails if another size is read.
    #[test]
    fn other_sizes_are_unsupported() {
        let mut reader = taught();
        assert!(reader.wants_whole_frame((1366, 768)));
        assert!(reader.wants_whole_frame((1280, 720)));
        for (w, h) in [(1024, 768), (800, 600), (1512, 850), (1920, 1080)] {
            assert!(!reader.wants_whole_frame((w, h)), "{w}x{h}");
            let canvas = game(w, h, &[&inventory(5_170_316).at(300, 300)]);
            assert_eq!(
                look(&mut reader, &canvas).view,
                MesoView::Unsupported,
                "{w}x{h}"
            );
            assert_eq!(reader.state(), MesoState::Unsupported);
        }
        // 1280x720 scaled up from a smaller picture is Full Screen's, not the window's.
        let canvas = game(1280, 720, &[&inventory(5_170_316).at(300, 300)]);
        let frame = canvas.frame().with_source(600);
        assert_eq!(
            reader.observe(&frame, None).map(|r| r.view),
            Some(MesoView::Unsupported)
        );
        assert_eq!(
            look(&mut reader, &canvas).view,
            read(MesoSource::Inventory, 5_170_316)
        );
        assert_eq!(reader.state(), MesoState::Reading);
    }

    // The shapes named come back from the text kept on the device, and read from the first
    // frame. Fails if a round trip loses a shape, or text it couldn't have written is accepted.
    #[test]
    fn what_was_named_is_kept_and_read_at_once_next_session() {
        let text = taught().knowledge().unwrap();
        assert!(text.starts_with("mesovibe meso-total 1\n"));
        assert_eq!(text.lines().count(), 12, "ten digits and the comma");
        let mut reader = MesoReader::from_knowledge(&text).unwrap();
        assert_eq!(reader.knowledge().as_deref(), Some(text.as_str()));
        assert_eq!(reader.state(), MesoState::Reading);
        let canvas = one(&inventory(8_642));
        assert_eq!(
            look(&mut reader, &canvas).view,
            read(MesoSource::Inventory, 8_642)
        );
        assert_eq!(
            look(&mut reader, &canvas).send,
            Some((8_642, MesoSource::Inventory))
        );
        for bad in [
            "",
            "mesovibe meso-total 1\n",
            "mesovibe exp-bar 1\nshape 5 5:0\n",
            "mesovibe meso-total 1\nshape x 5:0,0,0,0,0,0,0,0,0,0,0,0,0,0\n",
            "mesovibe meso-total 1\nshape 5 5:1\n",
            "mesovibe meso-total 1\nletter 5 5:0,0,0,0,0,0,0,0,0,0,0,0,0,0\n",
        ] {
            assert!(MesoReader::from_knowledge(bad).is_none(), "{bad:?}");
        }
    }

    // A Retina Mac's whole frame is read one pixel from each block, wherever the blocks start,
    // with the same result as the game's own pixels. Fails if a phase is misread.
    #[test]
    fn a_retina_frame_reads_at_its_blocks() {
        let canvas = one(&inventory(5_170_316));
        for phase in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let bgra = doubled(&canvas, phase);
            let frame = Frame::new(2732, 1536, 2732 * 4, &bgra)
                .and_then(|f| f.with_scale(2))
                .unwrap();
            for blocks in [Some(phase), None] {
                let mut reader = taught();
                let seen = reader.observe(&frame, blocks).map(|r| r.view);
                assert_eq!(
                    seen,
                    Some(read(MesoSource::Inventory, 5_170_316)),
                    "{phase:?} {blocks:?}"
                );
            }
        }
    }

    // A Retina frame whose pixel blocks can't be found (a loading screen) reads nothing and leaves
    // the size supported. Fails if the state players see flips to unsupported.
    #[test]
    fn a_retina_frame_without_blocks_leaves_the_size_supported() {
        let bgra = vec![0u8; 2732 * 1536 * 4];
        let frame = Frame::new(2732, 1536, 2732 * 4, &bgra)
            .and_then(|f| f.with_scale(2))
            .unwrap();
        let mut reader = taught();
        let seen = reader.observe(&frame, None).map(|r| r.view);
        assert_eq!(seen, Some(MesoView::NotOpen));
        assert_ne!(reader.state(), MesoState::Unsupported);
    }

    // Frames that hold only the bottom rows aren't looked at; the reader asks for a whole frame
    // about once a second, and every frame while a box is in view. Fails if the rate is off.
    #[test]
    fn asks_for_whole_frames_once_a_second_and_every_frame_while_a_box_is_in_view() {
        let mut reader = taught();
        let area = (1366, 768);
        let closed = game(1366, 768, &[]);
        let open = one(&inventory(5_170_316));
        let strip = |canvas: &Canvas| {
            let first = canvas.height - 200;
            let stride = canvas.width as usize * 4;
            let bgra = canvas.bgra[first as usize * stride..].to_vec();
            (bgra, first)
        };
        let mut asked = Vec::new();
        for i in 0..12 {
            let canvas = if i < 6 { &closed } else { &open };
            let wants = reader.wants_whole_frame(area);
            asked.push(wants);
            if wants {
                assert!(reader.observe(&canvas.frame(), None).is_some());
            } else {
                let (bgra, first) = strip(canvas);
                let frame = Frame::bottom(1366, 768, first, 1366 * 4, &bgra).unwrap();
                assert!(reader.observe(&frame, None).is_none());
            }
        }
        // Looks at once, then every fourth frame until the box shows, then every frame.
        let want = [
            true, false, false, false, true, false, false, false, true, true, true, true,
        ];
        assert_eq!(asked, want);
    }

    // The engine tells plugins how the total is read by the SDK's names for it. Fails if a state
    // is renamed on one side only.
    #[test]
    fn states_go_by_the_sdks_names() {
        let sdk =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../sdk/src/index.ts"))
                .unwrap_or_default();
        let union = sdk
            .lines()
            .find(|l| l.starts_with("export type MesoState ="))
            .expect("the SDK's MesoState");
        for state in [
            MesoState::Reading,
            MesoState::Learning,
            MesoState::Unsupported,
        ] {
            let name = serde_json::to_string(&state).unwrap();
            assert!(union.contains(&name), "{name} in {union}");
        }
    }
}
