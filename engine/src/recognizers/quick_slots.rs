//! The quick slots: the eight slots above the HUD's menu buttons (Shift, Ins, Hm and Pup over Ctrl,
//! Del, End and Pdn, as the game binds them at first), read from the bottom strip each frame.
//!
//! Each slot gives a key for its icon's picture, the same every time the same item sits in it, and
//! a key for its count's picture. The count's digits aren't named: their font doesn't read by shape
//! alone, and nothing else on screen shows the same numbers. A potion use shows as a slot's count
//! changing in the same moment the HP or MP numbers rise, which the Session Tracker pairs; which
//! potion an icon is, it learns from the size of that rise.
//!
//! The panel is found where the HUD puts it (an 800-pixel layout centred in the game area), checked
//! by its white border, a row or column either way. A count's digit is a piece of the count's light
//! fill (its strokes joined side by side or corner to corner) with black above, below and beside
//! every pixel and exactly as tall as the count's row, so a key label (grey outlined), a skill's
//! text, an icon's light pixels or a translucent tooltip never count as a digit.
//!
//! Only the game sizes the panel's place was checked at are read. Nothing is learned or saved.

use serde::Serialize;

use super::pixels::Pixels;
use crate::frame::Frame;
use crate::glyph::Glyph;

/// The quick slots, left to right along the top row, then the bottom row.
pub const SLOTS: usize = 8;
/// The HUD's width, centred in the game area.
const HUD_WIDTH: u32 = 800;
/// The panel's top-left corner (the corner of its white border): right of the HUD's left edge, and
/// up from the game area's bottom.
const PANEL_DX: u32 = 652;
const PANEL_UP: u32 = 144;
/// The panel's left border, checked white over these rows below its corner.
const LEFT_BORDER: (u32, u32) = (3, 68);
/// The panel's top border, checked white over these columns right of its corner.
const TOP_BORDER: (u32, u32) = (3, 100);
/// Of the border pixels checked, the share that must be white.
const BORDER_WHITE: f32 = 0.9;
/// From one slot to the next: right along a row, down to the next row.
const STEP: (u32, u32) = (35, 34);
/// A slot's count: the columns its digits may take (from the panel's corner) and the rows its
/// digits fill.
const COUNT_X: (u32, u32) = (5, 31);
const COUNT_Y: (u32, u32) = (26, 9);
/// The widest a count digit's fill is.
const MAX_DIGIT_WIDTH: u32 = 8;
/// A slot's icon: the columns and rows between its key label and its count's outline, from the
/// panel's corner. The label is left out, a descender's tail too (the p of Pup): it differs from
/// slot to slot, and the same item should give the same key in any slot.
const ICON_X: (u32, u32) = (7, 28);
const ICON_Y: (u32, u32) = (20, 5);
/// The game sizes the panel's place was checked at.
const SIZES: [(u32, u32); 2] = [(1280, 720), (1366, 768)];
/// Game rows the reader needs, counted up from the game area's bottom: the panel's top border,
/// with room for the HUD to sit a row higher.
pub const ROWS: u32 = PANEL_UP + 2;

/// Where the panel may be, nearest first: (right, down) from where the game area's size puts it.
const OFFSETS: [(i64, i64); 9] = [
    (0, 0),
    (0, -1),
    (0, 1),
    (-1, 0),
    (1, 0),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (1, 1),
];

/// What one slot showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    /// A key for the count's picture, or `None` when the slot shows no count (an empty slot, a
    /// skill, an item that doesn't stack, or a count something is drawn over whole). Something over
    /// part of it drops the digits it covers, so the key changes.
    pub count: Option<String>,
    /// A key for the icon's picture, the same every time the same item sits in the slot on this
    /// computer; an empty slot has one too.
    pub icon: String,
}

/// How the quick slots are read, as plugins hear it (the SDK's `QuickSlotState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickSlotState {
    /// Found and read.
    Reading,
    /// The panel isn't where the HUD puts it: hidden, covered, or another screen.
    NotFound,
    /// A game size the panel's place wasn't checked at.
    Unsupported,
}

/// What a frame gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickSlotsReading {
    /// The panel was found in this frame.
    pub found: bool,
    /// Slots whose reading two frames in a row agreed on and that differ from what was sent last:
    /// the slot's index ([`SLOTS`]) and its reading.
    pub changed: Vec<(u8, Slot)>,
}

/// Reads the quick slots frame by frame, sending a slot's reading once two frames agree on it.
#[derive(Debug, Clone, Default)]
pub struct QuickSlotReader {
    /// Each slot's reading in the last frame, waiting for the next to agree.
    candidate: [Option<Slot>; SLOTS],
    /// Each slot's reading sent last.
    sent: [Option<Slot>; SLOTS],
    unsupported: bool,
    found: bool,
}

impl QuickSlotReader {
    pub fn new() -> QuickSlotReader {
        QuickSlotReader::default()
    }

    /// Reads a frame (a strip or a whole frame). `blocks`: where a Retina frame's pixel blocks
    /// start, as the HUD reader found them.
    pub fn observe(&mut self, frame: &Frame, blocks: Option<(u32, u32)>) -> QuickSlotsReading {
        let scale = frame.scale();
        let area = (frame.width() / scale, frame.height() / scale);
        self.unsupported = !SIZES.contains(&area) || area.1 > frame.source();
        let slots = Pixels::of(*frame, blocks, ROWS)
            .filter(|_| !self.unsupported)
            .and_then(|px| panel(&px).map(|corner| read_slots(&px, corner)));
        self.found = slots.is_some();
        let Some(slots) = slots else {
            self.candidate = Default::default();
            return QuickSlotsReading {
                found: false,
                changed: Vec::new(),
            };
        };
        let mut changed = Vec::new();
        for (i, slot) in slots.into_iter().enumerate() {
            let agreed = self.candidate[i].as_ref() == Some(&slot);
            if agreed && self.sent[i].as_ref() != Some(&slot) {
                self.sent[i] = Some(slot.clone());
                changed.push((i as u8, slot.clone()));
            }
            self.candidate[i] = Some(slot);
        }
        QuickSlotsReading {
            found: true,
            changed,
        }
    }

    /// Each slot as the last frame showed it, before a second frame agrees; all `None` while the
    /// panel isn't found.
    pub fn shown(&self) -> &[Option<Slot>; SLOTS] {
        &self.candidate
    }

    /// How the slots are read, as of the last frame.
    pub fn state(&self) -> QuickSlotState {
        if self.unsupported {
            QuickSlotState::Unsupported
        } else if self.found {
            QuickSlotState::Reading
        } else {
            QuickSlotState::NotFound
        }
    }

    /// Forgets what was sent, so each slot's next agreed reading is sent again: the game was
    /// restarted, or capture paused and resumed.
    pub fn new_look(&mut self) {
        self.candidate = Default::default();
        self.sent = Default::default();
    }
}

/// White, as the panel's border is drawn.
fn is_white(p: [u8; 3]) -> bool {
    p.iter().all(|&c| c >= 0xf0)
}

/// The count's light fill, white at the top to light blue at the bottom, always at full blue. The
/// slot's own near-white background (#f8f9fa) isn't: a digit joined corner to corner would leak
/// into it.
fn is_fill(p: [u8; 3]) -> bool {
    p[0] >= 0x90 && p[1] >= 0xc0 && p[2] >= 0xfc
}

/// The count's black outline.
fn is_black(p: [u8; 3]) -> bool {
    p.iter().all(|&c| c <= 0x28)
}

/// The panel's corner, where its white border is found nearest where the HUD puts it.
fn panel(px: &Pixels) -> Option<(u32, u32)> {
    let hud_left = px.width.checked_sub(HUD_WIDTH)? / 2;
    let nominal = (
        i64::from(hud_left + PANEL_DX),
        i64::from(px.height.checked_sub(PANEL_UP)?),
    );
    OFFSETS.iter().find_map(|&(dx, dy)| {
        let (x, y) = (nominal.0 + dx, nominal.1 + dy);
        let (from, rows) = LEFT_BORDER;
        let left = (i64::from(from)..i64::from(from + rows)).map(|dy| (x, y + dy));
        let (from, cols) = TOP_BORDER;
        let top = (i64::from(from)..i64::from(from + cols)).map(|dx| (x + dx, y));
        (mostly_white(px, left) && mostly_white(px, top)).then_some((x as u32, y as u32))
    })
}

/// Whether enough of these pixels are white for a border: the cursor over it still leaves it one.
fn mostly_white(px: &Pixels, points: impl Iterator<Item = (i64, i64)>) -> bool {
    let (mut all, mut lit) = (0u32, 0u32);
    for (x, y) in points {
        all += 1;
        lit += u32::from(px.get(x, y).is_some_and(is_white));
    }
    lit as f32 >= all as f32 * BORDER_WHITE
}

/// Every slot's reading, from the panel's corner.
fn read_slots(px: &Pixels, (x0, y0): (u32, u32)) -> [Slot; SLOTS] {
    std::array::from_fn(|i| {
        let (col, row) = ((i % 4) as u32, (i / 4) as u32);
        let (x, y) = (x0 + col * STEP.0, y0 + row * STEP.1);
        Slot {
            count: count(px, x + COUNT_X.0, y + COUNT_Y.0),
            icon: icon(px, x + ICON_X.0, y + ICON_Y.0),
        }
    })
}

/// The eight pixels around one, the four beside it first.
const NEIGHBOURS: [(i64, i64); 8] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (1, 1),
];

/// FNV-1a: the same bytes, the same key.
fn fnv(bytes: impl IntoIterator<Item = u8>) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The key for the icon's picture at (`x`, `y`).
fn icon(px: &Pixels, x: u32, y: u32) -> String {
    let (cols, rows) = (ICON_X.1, ICON_Y.1);
    fnv((y..y + rows).flat_map(|yy| (x..x + cols).flat_map(move |xx| px.rgb(xx, yy))))
}

/// The key for the count whose digits may start at column `x` and fill rows from `top`, or `None`
/// when no digit is there.
fn count(px: &Pixels, x: u32, top: u32) -> Option<String> {
    let digits = digits(px, x, top);
    (!digits.is_empty()).then(|| {
        let text: Vec<String> = digits.iter().map(Glyph::encode).collect();
        fnv(text.join("|").into_bytes())
    })
}

/// The count's digits left to right: each piece of light fill in its columns, its pixels joined
/// side by side or corner to corner as the font's strokes are, with black beside, above and below
/// every pixel, that fills exactly the count's rows.
fn digits(px: &Pixels, x: u32, top: u32) -> Vec<Glyph> {
    let (cols, rows) = (COUNT_X.1, COUNT_Y.1);
    // One row above and below too, so a piece running past the count's rows is seen whole.
    let (x0, x1) = (i64::from(x), i64::from(x + cols));
    let (y0, y1) = (i64::from(top) - 1, i64::from(top + rows) + 1);
    let inside = |x: i64, y: i64| (x0..x1).contains(&x) && (y0..y1).contains(&y);
    let mut seen = vec![false; ((x1 - x0) * (y1 - y0)) as usize];
    let at = |x: i64, y: i64| ((y - y0) * (x1 - x0) + (x - x0)) as usize;
    let mut found: Vec<(i64, Glyph)> = Vec::new();
    for sy in y0..y1 {
        for sx in x0..x1 {
            if seen[at(sx, sy)] || !px.get(sx, sy).is_some_and(is_fill) {
                continue;
            }
            seen[at(sx, sy)] = true;
            let mut piece = vec![(sx, sy)];
            let mut queue = vec![(sx, sy)];
            let mut enclosed = true;
            while let Some((cx, cy)) = queue.pop() {
                for (dx, dy) in NEIGHBOURS {
                    let (nx, ny) = (cx + dx, cy + dy);
                    let side = dx == 0 || dy == 0;
                    match px.get(nx, ny) {
                        Some(p) if is_fill(p) => {
                            if !inside(nx, ny) {
                                enclosed = false;
                            } else if !seen[at(nx, ny)] {
                                seen[at(nx, ny)] = true;
                                piece.push((nx, ny));
                                queue.push((nx, ny));
                            }
                        }
                        // Only the sides need black: a stroke's outline doesn't fill the corners.
                        Some(p) if !side || is_black(p) => {}
                        _ => enclosed = false,
                    }
                }
            }
            if let Some(glyph) = enclosed.then(|| glyph(&piece, top, rows)).flatten() {
                let left = piece.iter().map(|&(x, _)| x).min().unwrap_or(x0);
                found.push((left, glyph));
            }
        }
    }
    found.sort_by_key(|&(left, _)| left);
    found.into_iter().map(|(_, glyph)| glyph).collect()
}

/// A piece of fill as a digit: `None` unless it fills exactly the count's `rows` from `top` and is
/// no wider than a digit.
fn glyph(piece: &[(i64, i64)], top: u32, rows: u32) -> Option<Glyph> {
    let left = piece.iter().map(|&(x, _)| x).min()?;
    let right = piece.iter().map(|&(x, _)| x).max()?;
    let high = piece.iter().map(|&(_, y)| y).min()?;
    let low = piece.iter().map(|&(_, y)| y).max()?;
    let width = (right - left + 1) as u32;
    if high != i64::from(top) || low != i64::from(top + rows - 1) || width > MAX_DIGIT_WIDTH {
        return None;
    }
    let mut bits = vec![0u32; rows as usize];
    for &(x, y) in piece {
        bits[(y - high) as usize] |= 1 << (x - left);
    }
    Glyph::from_rows(width, bits)
}

/// A synthetic quick-slot panel for tests, here and in the engine loop's: the border, the slots'
/// backgrounds, invented icons and an invented digit font drawn in the count's fill and outline
/// colours. No game pixels are used.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::recognizers::exp_bar::test_support::Canvas;

    const PANEL: [u8; 3] = [64, 72, 84];
    const SLOT: [u8; 3] = [248, 249, 250];
    const LABEL_EDGE: [u8; 3] = [200, 204, 210];
    /// The count's fill, row by row from the top.
    const FILL: [[u8; 3]; 9] = [
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [255, 255, 255],
        [238, 255, 255],
        [221, 238, 255],
        [187, 221, 255],
        [170, 204, 255],
        [153, 204, 255],
    ];

    /// What to draw in one slot: an invented icon, and its count if it shows one.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Drawn {
        pub(crate) icon: u8,
        pub(crate) count: Option<u32>,
    }

    pub(crate) const fn item(icon: u8, count: u32) -> Option<Drawn> {
        Some(Drawn {
            icon,
            count: Some(count),
        })
    }

    /// The panel's corner where the HUD puts it in a game area this size.
    pub(crate) fn corner(width: u32, height: u32) -> (u32, u32) {
        ((width - HUD_WIDTH) / 2 + PANEL_DX, height - PANEL_UP)
    }

    /// An invented digit, up to 6 wide and 9 tall: a stem and a bar, each digit's in its own
    /// place. From 5 up the stem steps right halfway down, its two halves joined only corner to
    /// corner, as the game's strokes often are.
    fn digit(d: u32) -> Vec<(u32, u32)> {
        let stem = |y: u32| d % 5 + u32::from(d >= 5 && y >= 5);
        let bar = (d / 5) * 8;
        let mut fill: Vec<(u32, u32)> = (0..9).map(|y| (stem(y), y)).collect();
        fill.extend((0..5).filter(|&x| x != stem(bar)).map(|x| (x, bar)));
        fill
    }

    /// Draws `n` in the count's colours with its digits from column `x` and its rows from `top`.
    pub(crate) fn draw_count(canvas: &mut Canvas, n: u32, x: u32, top: u32) {
        let mut fill = Vec::new();
        for (i, c) in n.to_string().chars().enumerate() {
            let d = c.to_digit(10).unwrap();
            let left = x + 1 + i as u32 * 8;
            fill.extend(digit(d).into_iter().map(|(dx, dy)| (left + dx, top + dy)));
        }
        for &(fx, fy) in &fill {
            for oy in fy - 1..=fy + 1 {
                for ox in fx - 1..=fx + 1 {
                    canvas.set(ox, oy, [0, 0, 0]);
                }
            }
        }
        for &(fx, fy) in &fill {
            canvas.set(fx, fy, FILL[(fy - top) as usize]);
        }
    }

    /// Draws the panel with its corner at `at`.
    pub(crate) fn draw_panel(canvas: &mut Canvas, (x0, y0): (u32, u32), slots: &[Option<Drawn>]) {
        canvas.fill(x0, y0, 150, 76, PANEL);
        canvas.fill(x0, y0, 1, 76, [255, 255, 255]);
        canvas.fill(x0, y0, 150, 1, [255, 255, 255]);
        for i in 0..SLOTS {
            let (col, row) = ((i % 4) as u32, (i / 4) as u32);
            let (x, y) = (x0 + col * STEP.0, y0 + row * STEP.1);
            canvas.fill(x + 7, y + 7, 28, 30, SLOT);
            // A key label: white letters with a grey edge, never a count digit.
            canvas.fill(x + 9, y + 8, 12, 7, LABEL_EDGE);
            canvas.fill(x + 10, y + 9, 10, 5, [255, 255, 255]);
            let Some(drawn) = slots.get(i).copied().flatten() else {
                continue;
            };
            // An invented icon over the slot's middle, its colours from its number.
            for dy in 0..18 {
                for dx in 0..22 {
                    let k = u32::from(drawn.icon);
                    let shade = ((dx * 7 + dy * 13 + k * 31) % 200) as u8;
                    canvas.set(
                        x + 11 + dx,
                        y + 15 + dy,
                        [shade, 80 + (k * 40 % 120) as u8, 40],
                    );
                }
            }
            if let Some(n) = drawn.count {
                draw_count(canvas, n, x + COUNT_X.0, y + COUNT_Y.0);
            }
        }
    }

    /// A game area `width` x `height` with the panel where the HUD puts it.
    pub(crate) fn game(width: u32, height: u32, slots: &[Option<Drawn>]) -> Canvas {
        let mut canvas = Canvas::new(width, height);
        draw_panel(&mut canvas, corner(width, height), slots);
        canvas
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::recognizers::exp_bar::test_support::Canvas;

    const LEMON: u8 = 1;
    const MEAT: u8 = 2;

    fn slots(shift: Option<Drawn>, end: Option<Drawn>) -> [Option<Drawn>; SLOTS] {
        let mut all = [None; SLOTS];
        all[0] = shift;
        all[6] = end;
        all
    }

    fn look(reader: &mut QuickSlotReader, canvas: &Canvas) -> QuickSlotsReading {
        reader.observe(&canvas.frame(), None)
    }

    /// The reading sent for each slot after two agreeing frames.
    fn settled(canvas: &Canvas) -> Vec<(u8, Slot)> {
        let mut reader = QuickSlotReader::new();
        assert!(
            look(&mut reader, canvas).changed.is_empty(),
            "nothing on one frame"
        );
        look(&mut reader, canvas).changed
    }

    // Each slot's icon and count give keys: the same picture the same key, another count or
    // another item another key, and a slot showing no count none. Fails if a key depends on the
    // slot it's in or a count is read where none is drawn.
    #[test]
    fn each_slot_gives_its_icons_key_and_its_counts() {
        let mut all = slots(item(LEMON, 274), item(MEAT, 377));
        all[3] = item(LEMON, 274);
        all[4] = Some(Drawn {
            icon: 9,
            count: None,
        });
        let sent = settled(&game(1366, 768, &all));
        assert_eq!(sent.len(), SLOTS, "every slot once");
        let slot = |i: usize| &sent[i].1;
        assert_eq!(
            slot(0),
            slot(3),
            "the same item and count, wherever it sits"
        );
        assert_ne!(slot(0).icon, slot(6).icon);
        assert_ne!(slot(0).count, slot(6).count);
        assert!(slot(0).count.is_some());
        assert_eq!(slot(4).count, None, "a slot showing no count");
        assert_eq!(slot(1).count, None, "an empty slot");
        assert_eq!(slot(1).icon, slot(2).icon, "empty slots look alike");
    }

    // Every digit reads, the ones whose strokes join only at a corner too, and no two alike. Fails
    // if a digit splits into pieces and drops out of its count.
    #[test]
    fn every_digit_reads_whole() {
        let mut keys = std::collections::BTreeSet::new();
        for first in [0u32, 8] {
            let mut all = [None; SLOTS];
            for (i, slot) in all.iter_mut().enumerate() {
                let d = first + i as u32;
                *slot = (d < 10).then_some(Drawn {
                    icon: 1,
                    count: Some(d),
                });
            }
            for (i, slot) in settled(&game(1366, 768, &all)).into_iter().enumerate() {
                if first + (i as u32) < 10 {
                    keys.insert(slot.1.count.expect("a single digit reads"));
                }
            }
        }
        assert_eq!(keys.len(), 10);
    }

    // A count's change is sent once two frames agree on it; a change one frame shows is never
    // sent. Fails if one frame's picture reaches plugins or an agreed change is lost.
    #[test]
    fn a_change_is_sent_once_two_frames_agree() {
        let mut reader = QuickSlotReader::new();
        let before = game(1366, 768, &slots(item(LEMON, 274), item(MEAT, 377)));
        let after = game(1366, 768, &slots(item(LEMON, 273), item(MEAT, 377)));
        look(&mut reader, &before);
        let first = look(&mut reader, &before).changed;
        assert_eq!(first.len(), SLOTS);
        assert!(look(&mut reader, &after).changed.is_empty(), "one frame");
        assert!(
            look(&mut reader, &before).changed.is_empty(),
            "back as it was"
        );
        assert!(look(&mut reader, &after).changed.is_empty());
        let changed = look(&mut reader, &after).changed;
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, 0);
        assert_ne!(changed[0].1.count, first[0].1.count);
        assert_eq!(changed[0].1.icon, first[0].1.icon, "the same item");
        assert!(look(&mut reader, &after).changed.is_empty(), "sent once");
        assert_eq!(reader.state(), QuickSlotState::Reading);
    }

    // The panel is found a row or a column from where the HUD puts it, as on a Mac. Fails if a
    // shifted panel is lost or read at the wrong place.
    #[test]
    fn the_panel_is_found_a_row_or_column_off() {
        let expected = settled(&game(1366, 768, &slots(item(LEMON, 274), None)));
        for (dx, dy) in [(0i64, -1i64), (1, 0), (-1, 1)] {
            let mut canvas = Canvas::new(1366, 768);
            let (x, y) = corner(1366, 768);
            let at = ((i64::from(x) + dx) as u32, (i64::from(y) + dy) as u32);
            draw_panel(&mut canvas, at, &slots(item(LEMON, 274), None));
            assert_eq!(settled(&canvas), expected, "({dx}, {dy})");
        }
    }

    // With no panel where the HUD puts it (hidden, or another screen) nothing is read or sent.
    // Fails if anything is read off the game's picture.
    #[test]
    fn no_panel_reads_nothing() {
        let mut reader = QuickSlotReader::new();
        let empty = Canvas::new(1366, 768);
        for _ in 0..3 {
            assert_eq!(
                look(&mut reader, &empty),
                QuickSlotsReading {
                    found: false,
                    changed: Vec::new()
                }
            );
        }
        assert_eq!(reader.state(), QuickSlotState::NotFound);
        // Two misplaced pictures of the panel aren't it either.
        let mut far = Canvas::new(1366, 768);
        draw_panel(&mut far, (600, 400), &slots(item(LEMON, 274), None));
        assert!(!look(&mut reader, &far).found);
    }

    // White text in a slot's count row that isn't enclosed by black (a skill's name, an icon's
    // light pixels) is no count. Fails if it's read as digits.
    #[test]
    fn light_pixels_without_a_black_outline_are_no_count() {
        let mut canvas = game(1366, 768, &slots(None, None));
        let (x, y) = corner(1366, 768);
        // "ATTACK"-like text in the Shift slot's count row, edged grey and dark red.
        canvas.fill(x + 7, y + 27, 24, 6, [120, 40, 60]);
        canvas.fill(x + 8, y + 28, 22, 4, [255, 255, 255]);
        let sent = settled(&canvas);
        assert_eq!(sent[0].1.count, None);
        // A count with something drawn over one digit's outline loses that digit.
        let mut covered = game(1366, 768, &slots(item(LEMON, 274), None));
        covered.set(x + COUNT_X.0 + 1, y + COUNT_Y.0 - 1, [128, 128, 128]);
        let mut clean = game(1366, 768, &slots(item(LEMON, 274), None));
        clean.set(x + 100, y + 70, [128, 128, 128]);
        assert_ne!(settled(&covered)[0].1.count, settled(&clean)[0].1.count);
    }

    // A translucent tooltip over the slots leaves no count readable under it, never another count.
    // Fails if a darkened count reads as digits.
    #[test]
    fn a_tooltip_over_a_count_leaves_no_count() {
        let mut canvas = game(1366, 768, &slots(item(LEMON, 274), None));
        let (x, y) = corner(1366, 768);
        for yy in y + 20..y + 40 {
            for xx in x + 3..x + 40 {
                let [r, g, b] = canvas.get(xx, yy);
                canvas.set(xx, yy, [r / 2 + 20, g / 2 + 20, b / 2 + 40]);
            }
        }
        assert_eq!(settled(&canvas)[0].1.count, None);
    }

    // On a Retina Mac the strip is read at its pixel blocks, the same as at 1x. Fails if the
    // keys differ or the strip's rows aren't enough.
    #[test]
    fn a_retina_strip_reads_at_its_blocks() {
        let canvas = game(1280, 720, &slots(item(LEMON, 274), item(MEAT, 53)));
        let expected = settled(&canvas);
        let bgra = crate::recognizers::meso_total::test_support::doubled(&canvas, (1, 0));
        let rows = ROWS * 2;
        let first = 1440 - rows;
        let stride = 2560 * 4;
        let frame = Frame::bottom(2560, 1440, first, stride, &bgra[first as usize * stride..])
            .and_then(|f| f.with_scale(2))
            .unwrap();
        let mut reader = QuickSlotReader::new();
        reader.observe(&frame, Some((1, 0)));
        assert_eq!(reader.observe(&frame, Some((1, 0))).changed, expected);
    }

    // Only the sizes the panel's place was checked at are read. Fails if another size is read or
    // isn't reported as unsupported.
    #[test]
    fn other_sizes_are_unsupported() {
        let mut reader = QuickSlotReader::new();
        let canvas = game(1024, 768, &slots(item(LEMON, 274), None));
        look(&mut reader, &canvas);
        assert!(look(&mut reader, &canvas).changed.is_empty());
        assert_eq!(reader.state(), QuickSlotState::Unsupported);
    }

    // After the game restarts (or capture resumes), each slot's reading is sent again. Fails if a
    // plugin is left without the slots' state.
    #[test]
    fn a_new_look_sends_every_slot_again() {
        let mut reader = QuickSlotReader::new();
        let canvas = game(1366, 768, &slots(item(LEMON, 274), None));
        look(&mut reader, &canvas);
        assert_eq!(look(&mut reader, &canvas).changed.len(), SLOTS);
        reader.new_look();
        look(&mut reader, &canvas);
        assert_eq!(look(&mut reader, &canvas).changed.len(), SLOTS);
    }
}
