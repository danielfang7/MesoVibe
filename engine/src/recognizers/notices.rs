//! Pickup notice reader: the EXP, mesos and items the game reports at the bottom right.
//!
//! When the player gains EXP or picks something up, the game writes a line at the bottom right of
//! the screen: "You received EXP (+60)", "You have gained mesos (+72)", "Fire Boar's Tooth x1
//! earned. (Etc)", and in a party "Bonus EXP for PARTY (+4)" in yellow. A new line enters at the
//! bottom and pushes the others up; lines are 14 px apart at 1x, and up to six show. Each line
//! is drawn at full strength when it arrives and fades from then on: on Windows recordings at 1x a
//! line had lost about a fifth of its strength a second later, and was too faint to compare after
//! about three seconds.
//!
//! The text is a one-bit pixel font, a white core with a black shadow one pixel down and to the
//! right, drawn over the scene at the line's strength. So the end of a stroke is a pixel much
//! brighter than the one below and to the right of it, whatever the scene behind: over white
//! clouds the core looks like the cloud, but its shadow doesn't. How much brighter says how far
//! the line has faded. The rest of a letter is the white pixels that lead down and to the right to
//! a stroke's end, as bright as it.
//!
//! New lines are counted by how far the lines already on screen moved up ([`crate::lines`]), so
//! the same pickup twice counts twice. A line only ever fades, so a line brighter than the one in
//! its place a frame earlier is a new one. It fades slowly, so a line that was bright a frame ago
//! is still on screen; and when every line is where it was, a frame fainter, nothing arrived, even
//! if the scene sliding behind them as the camera moves changed their shapes. A faded line shows
//! too much of the scene to compare its shape, so it takes part by its strength alone. When the
//! game dims the screen to change maps, the lines still on screen over the new map were counted
//! already ([`NoticeReader::screen_dark`]).
//!
//! **It learns the letters itself**, on the player's device; nothing learned leaves it, and no game
//! pixels ship. An EXP line's amount must be the EXP the HUD gained, so the first EXP lines that
//! arrive one at a time name the digits and the letters of "You received EXP". The other phrases'
//! letters follow from the ones known. Readings are confirmed once three EXP amounts in a row
//! match the HUD; until then they're marked unconfirmed, and nothing should be shown from them.
//! What was confirmed can be kept on the device ([`NoticeReader::knowledge`]), so the next session
//! reads from its first pickup and the digits add up across sessions.
//!
//! **Mesos have nothing on screen to check them against**, so EXP amounts name the digits, and on
//! a map whose monsters all give the same EXP that may be only a few of them. The others are named
//! by their shapes, the way a person reads digits in a new font (loops and strokes), but only
//! after those rules name every digit the EXP lines taught correctly. A digit named that way is
//! dropped, with every other one named by shape, if an EXP line using it ever disagrees with the
//! HUD. A mesos amount with a digit still unknown is reported missed, kept, and counted late as
//! soon as its digits are known.
//!
//! A line's letters are cut from the first frame it shows in, while it's still at full strength:
//! at four frames a second that's within a quarter second of its arrival. Letters are compared
//! exactly, but phrases are matched from the line's end allowing a few letters to differ, so two
//! touching letters or a speck of the scene costs a letter of a name, not the line.
//!
//! **Monsters walk behind the stack** on many maps, and their outlines spoil the letters they pass
//! behind. A line that can't be read when it arrives is tried again in the next frames while it's
//! still strong enough, as the monster moves on; only when frames come often enough for that (the
//! reader measures how fast lines fade between frames). A line seen again is the same line if
//! most of its columns still agree and it hasn't brightened, and where the stack sits is decided
//! by a vote along its lines, so a monster spoils a stretch, not the count.
//!
//! **Missed lines are reported, never guessed.** A stack that lines up with nothing, a new line
//! that can't be read (covered, or a digit not learned yet), or EXP lines that add up to less or
//! more than the HUD gained all mark the reading [`NoticeReading::missed`]. In a party, the party
//! members' HP list sits over the stack where the game puts it, and its lines can't be read there.
//!
//! An item's identity is the exact shape of its name ([`ItemName::key`]): its letters needn't be
//! known. Its text has the letters that are, and `?` for the rest.

use std::collections::HashMap;

use crate::frame::Frame;
use crate::glyph::{Atlas, Glyph, MAX_GLYPH_WIDTH};
use crate::lines::{self, StackLine};
pub use crate::recognizers::NamePicture;
use crate::recognizers::digit_shapes;
use crate::recognizers::exp_bar::ExpReading;
use crate::unstretch::{self, No1x};

/// Lines the stack shows at most.
const SLOTS: usize = 6;
/// Rows from one line's top to the next one's, at 1x.
const LINE_PITCH: u32 = 14;
/// The bottom line's top row, counted up from the bottom of the 1x picture.
const BOTTOM_LINE_UP: u32 = 162;
/// A line's rows, from the top of its brackets to the bottom of its descenders. The game clips
/// each line to these rows, so its bottom row has no shadow.
const LINE_ROWS: u32 = 11;
/// Columns read, from the picture's right edge. Lines end 8 columns in; "Fire Boar's Tooth x1
/// earned. (Etc)" is 186 wide.
pub const NOTICE_COLUMNS: u32 = 400;
/// The columns a line can be compared in: all but the rightmost, which has no neighbor to its
/// right.
const TEXT_COLUMNS: u32 = NOTICE_COLUMNS - 1;
/// A line's last column, counted in from the picture's right edge.
const RIGHT_MARGIN: u32 = 8;
/// Columns of a line that vote together on where the stack sits ([`find_shift`]): a few letters.
const VOTE_COLUMNS: u32 = 24;
/// Votes a place needs, and twice the current place's, for the stack to move there.
const MIN_VOTES: usize = 4;
/// How far, in game pixels up or down, the stack may sit from where the picture's height puts it.
/// Wine on a Mac places the game up to a point off.
const MAX_SHIFT: u32 = 1;
/// 1x rows the reader looks at, up from the bottom: the top line's, with room for the stack to sit
/// a row higher.
pub const NOTICE_ROWS: u32 = BOTTOM_LINE_UP + LINE_PITCH * (SLOTS as u32 - 1) + MAX_SHIFT + 1;

/// A stroke's end at least this much brighter than its shadow counts toward its line's strength.
/// Textured scenery (rock, bark) makes some pixels this much brighter than their neighbor too, so
/// a slot needs [`MIN_PIXELS`] of them.
const FAINT: i16 = 96;
/// Pixels a slot needs at [`FAINT`] to hold a line.
const MIN_PIXELS: usize = 16;
/// A line at least this strong is compared by its shape. Fainter, its shape is lost in the scene.
const SOLID: u8 = 160;
/// A solid line's shape: its pixels at least this much brighter than their shadow.
const INK: i16 = 120;
/// A solid line has at least this many such pixels: the shortest notice, "You received EXP
/// (+1)", has about twice as many at 1x.
const MIN_INK: u32 = 100;
/// Digits learned from EXP lines the shape rules must name right before naming others.
const DIGITS_CHECKED: usize = 3;
/// Mesos amounts with a digit not learned yet kept to read later: a few hours of play.
const UNREAD_KEPT: usize = 500;
/// Frames whose fading [`NoticeReader::fade`] measures.
const FADES_KEPT: usize = 9;
/// A line seen again may differ from how it was in at most one column in this many, for something
/// moving behind it ([`mostly_same_shape`]).
const DAMAGED_PARTS: u32 = 3;
/// How much stronger a line spoiled by something moving behind it may measure than in the frame
/// before and still be the same line: measuring noise, well under a frame's fading.
const STEADY: u8 = 4;
/// A line is read only while this strong. Fainter, too much of the scene shows through to tell
/// every pixel of its letters. Captured four times a second, a line is first seen at about 230.
const READ: u8 = 215;
/// How much stronger a line may measure than in the frame before and still be the same line. Its
/// strength is measured over a different part of the scene once it has moved up.
const BRIGHTER: u8 = 16;
/// Frames' worth of fading a line may lose between two frames and still be the same line: frames
/// don't come evenly, and one after a whole-picture grab comes late. In recordings at four frames
/// a second, no line still on screen lost more than this and [`BRIGHTER`] between two frames.
const LATE_FRAMES: u8 = 3;
/// A gap of at least this many columns between letters is a space. Letters of a word are at most
/// three apart (after a 1), and words at least four.
const SPACE: u32 = 4;
/// A gap wider than this ends the line: whatever is further left is the scene.
const MAX_SPACE: u32 = 10;
/// The strongest a stroke's end can be: white over a black shadow.
const FULL: i16 = 255;
/// Frames a group of EXP lines may wait for the HUD's EXP to match it, and the HUD's gain for the
/// lines: the two needn't change in the same frame.
const SETTLE: u32 = 3;
/// EXP amounts read in a row that must match the HUD before readings are confirmed.
const CONFIRM_AFTER: usize = 3;
/// EXP amounts in a row that may disagree with the HUD while learning before what was learned is
/// dropped and learned again.
const FORGET_AFTER: usize = 3;
/// The first line of [`NoticeReader::knowledge`]'s text. A reader refuses any other.
const KNOWLEDGE_HEADER: &str = "mesovibe notices 1";
/// EXP lines in a row that letters kept from an earlier session can't read before the reader
/// drops them and learns again. Only a game update that redrew the font should get there.
const RESTORE_PATIENCE: usize = 3;

/// The inventory tab a picked-up item went to, as the notice names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Equip,
    Use,
    Setup,
    Etc,
    Cash,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Equip, Tab::Use, Tab::Setup, Tab::Etc, Tab::Cash];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Equip => "Equip",
            Tab::Use => "Use",
            Tab::Setup => "Setup",
            Tab::Etc => "Etc",
            Tab::Cash => "Cash",
        }
    }
}

/// One new notice line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// "You received EXP (+N)".
    Exp(u64),
    /// "Bonus EXP for PARTY (+N)", in a party.
    PartyBonus(u64),
    /// "You have gained mesos (+N)".
    Mesos(u64),
    /// Mesos picked up earlier, while a digit of the amount wasn't learned yet (reported missed
    /// then), read now that its digits are known.
    LateMesos(u64),
    /// `<name> xN earned. (<tab>)`. The tab is `None` while its letters can't tell it yet.
    Item {
        name: ItemName,
        count: u32,
        tab: Option<Tab>,
    },
    /// A line that isn't a pickup, such as "This item is unavailable for pick-up."
    Other,
}

/// An item's name as the notice drew it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemName {
    /// The name's exact shape as text: the same name gives the same key every time, at every size.
    pub key: String,
    /// The name as far as its letters are known, `?` for each letter that isn't.
    pub text: String,
    pub picture: NamePicture,
}

/// Why lines may have been missed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Missed {
    /// The stack couldn't be lined up with the last frame's.
    Unaligned,
    /// A new line couldn't be read: covered, or its letters not learned yet.
    Unreadable,
    /// A mesos line whose amount has a digit not learned yet.
    NotLearned,
    /// The HUD gained more EXP than the EXP lines read add up to.
    ExpShort,
    /// The EXP lines read add up to more than the HUD gained.
    ExpOver,
    /// The stack was out of sight for a while, so lines may have come and gone unseen.
    Lost,
}

impl Missed {
    /// A stable name for the reason, for events.
    pub fn name(self) -> &'static str {
        match self {
            Missed::Unaligned => "unaligned",
            Missed::Unreadable => "unreadable",
            Missed::NotLearned => "not_learned",
            Missed::ExpShort => "exp_short",
            Missed::ExpOver => "exp_over",
            Missed::Lost => "lost",
        }
    }
}

/// What one frame of the notice stack brought.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoticeReading {
    /// New lines, oldest first.
    pub notices: Vec<Notice>,
    /// Lines may have been missed since the last reading.
    pub missed: Option<Missed>,
    /// False while the letters' names haven't been checked against the HUD enough times: nothing
    /// should be shown from the reading.
    pub confirmed: bool,
}

/// Why a frame couldn't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeUnreadable {
    /// The game area is too small to hold the stack.
    UnsupportedSize { width: u32, height: u32 },
    /// The frame has no 1x picture to read.
    Unsupported(No1x),
}

/// Where the stack is in a 1x picture.
#[derive(Debug, Clone, Copy)]
struct Layout {
    /// The first column read, and one past the last one compared.
    left: u32,
    right: u32,
    bottom: u32,
}

impl Layout {
    fn of(frame: &Frame) -> Option<Layout> {
        let (w, h) = (frame.width(), frame.height());
        (w > NOTICE_COLUMNS && h > NOTICE_ROWS && frame.first_row() + NOTICE_ROWS <= h).then(|| {
            Layout {
                left: w - NOTICE_COLUMNS,
                right: w - 1,
                bottom: h,
            }
        })
    }

    /// The top row of slot `k` (0 is the bottom line) with the stack `shift` rows down.
    fn top(self, k: usize, shift: i32) -> u32 {
        (self.bottom - BOTTOM_LINE_UP - LINE_PITCH * k as u32).wrapping_add_signed(shift)
    }
}

/// White and yellow are bright in red and green.
fn lum(p: [u8; 3]) -> i16 {
    i16::from(p[0].min(p[1]))
}

/// How much brighter the pixel at (x, y) is than the one below and to its right, in the lesser of
/// red and green.
fn diff(frame: &Frame, x: u32, y: u32) -> i16 {
    let (a, b) = (frame.rgb(x, y), frame.rgb(x + 1, y + 1));
    (i16::from(a[0]) - i16::from(b[0])).min(i16::from(a[1]) - i16::from(b[1]))
}

/// One slot of the stack as seen in a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Slot {
    /// How bright its strokes are: the middle difference of its pixels at [`FAINT`], 0 when it
    /// holds no line.
    strength: u8,
    /// A solid line's shape, bit `row * TEXT_COLUMNS + column`; empty when it isn't solid.
    ink: Vec<u64>,
    count: u32,
}

impl Slot {
    fn solid(&self) -> bool {
        self.strength >= SOLID
    }
}

impl StackLine for Slot {
    /// `self` is the line in the frame before. A line that's faded since lines up with anything:
    /// lines fade. A solid one is the same line only if it's no brighter than it was and has the
    /// same shape, up to a pixel away each way. In the same place it measures no brighter at all,
    /// over the same scene: a newer line with the same phrase and another amount, which looks
    /// nearly the same, is brighter by a frame's fading. Moved up over another part of the scene,
    /// it may measure a little brighter. Something moving behind it, like a monster walking past,
    /// changes part of its shape; it's still the same line if most of its columns agree and it
    /// hasn't brightened at all.
    fn matches(&self, now: &Slot, moved: bool) -> bool {
        let brighter = match moved {
            true => BRIGHTER,
            false => STEADY,
        };
        match (self.solid(), now.solid()) {
            (_, false) => true,
            (false, true) => false,
            (true, true) => {
                (now.strength <= self.strength.saturating_add(brighter) && same_shape(self, now))
                    || (now.strength <= self.strength.saturating_add(STEADY)
                        && mostly_same_shape(self, now))
            }
        }
    }

    fn is_blank(&self) -> bool {
        !self.solid()
    }
}

/// Whether `now` can be what's left of `before` a frame later, having lost at most `most` of its
/// strength. Lines take seconds to fade away, so a line that was solid a frame ago is still on
/// screen, and not much fainter. A slot that shows nothing may hold a line just fainter than
/// [`FAINT`].
fn still_there(before: &Slot, now: &Slot, most: u8) -> bool {
    let at_most = now.strength.max(FAINT as u8);
    !before.solid() || at_most.saturating_add(most) >= before.strength
}

/// Every line where it was a frame ago, and nothing new: each solid line still there and no
/// brighter, and no solid line where there was none. The scene moving behind lines that stayed
/// put (the camera following the player past striped or edged scenery) changes their shapes, so
/// they may not line up by shape; their strength still shows they're the same lines, a frame
/// fainter. A new line would be brighter than the line it pushed up, and that one would show in
/// the slot above.
fn steady(before: &[Slot], now: &[Slot], most: u8) -> bool {
    before.iter().zip(now).all(|(b, n)| match b.solid() {
        true => still_there(b, n, most) && n.strength <= b.strength.saturating_add(STEADY),
        false => !n.solid(),
    })
}

fn same_shape(a: &Slot, b: &Slot) -> bool {
    let offsets = || (-1..=1).flat_map(|down| (-1..=1).map(move |right| (right, down)));
    let allowed = (a.count.max(b.count) / 10).max(6);
    let same_at = |(right, down): (i32, i32)| {
        let by = down * TEXT_COLUMNS as i32 + right;
        lines::differ(&a.ink, &b.ink, TEXT_COLUMNS * LINE_ROWS, by, None) <= allowed
    };
    a.count.abs_diff(b.count) <= allowed && (same_at((0, 0)) || offsets().any(same_at))
}

/// The same shape but for a stretch spoiled by something moving behind it: at most one column in
/// [`DAMAGED_PARTS`] differs. Lines with the same phrase and another amount pass too, so it's for
/// a line that hasn't brightened ([`Slot::matches`]).
fn mostly_same_shape(a: &Slot, b: &Slot) -> bool {
    let offsets = (-1..=1).flat_map(|down| (-1..=1).map(move |right| (right, down)));
    offsets.into_iter().any(|by| {
        let (differ, inked) = columns_differ(a, b, by);
        inked > 0 && differ * DAMAGED_PARTS <= inked
    })
}

/// In how many columns `a` and `b` moved `(right, down)` differ, and in how many either has ink.
fn columns_differ(a: &Slot, b: &Slot, (right, down): (i32, i32)) -> (u32, u32) {
    let ink = |slot: &Slot, row: i32, col: i32| -> bool {
        if !(0..LINE_ROWS as i32).contains(&row) || !(0..TEXT_COLUMNS as i32).contains(&col) {
            return false;
        }
        let bit = (row as u32 * TEXT_COLUMNS + col as u32) as usize;
        slot.ink
            .get(bit / 64)
            .is_some_and(|word| word >> (bit % 64) & 1 == 1)
    };
    let (mut differ, mut inked) = (0, 0);
    for col in 0..TEXT_COLUMNS as i32 {
        let (mut any, mut other) = (false, false);
        for row in 0..LINE_ROWS as i32 {
            let (x, y) = (ink(a, row, col), ink(b, row - down, col - right));
            any |= x || y;
            other |= x != y;
        }
        inked += u32::from(any);
        differ += u32::from(other);
    }
    (differ, inked)
}

/// The slot whose top row is `top`. A line ends at the right, [`RIGHT_MARGIN`] columns in, and
/// runs left from there to a gap wider than a space; what's beyond that, a name tag or the scene,
/// isn't part of it.
fn slot(frame: &Frame, layout: Layout, top: u32) -> Slot {
    let blank = Slot {
        strength: 0,
        ink: Vec::new(),
        count: 0,
    };
    let faint: Vec<bool> = (layout.left..layout.right)
        .map(|x| (top..top + LINE_ROWS).any(|y| diff(frame, x, y) >= FAINT))
        .collect();
    let end = (layout.right + 1 - RIGHT_MARGIN - layout.left) as usize;
    let ends_right = (end - 2..(end + 2).min(faint.len())).any(|col| faint[col]);
    if !ends_right {
        return blank;
    }
    let mut from = end;
    let mut gap = 0;
    for col in (0..end).rev() {
        if faint[col] {
            from = col;
            gap = 0;
        } else {
            gap += 1;
            if gap > MAX_SPACE {
                break;
            }
        }
    }
    let from = layout.left + from as u32;
    let mut seen = Vec::new();
    for y in top..top + LINE_ROWS {
        for x in from..layout.right {
            let d = diff(frame, x, y);
            if d >= FAINT {
                seen.push(d);
            }
        }
    }
    if seen.len() < MIN_PIXELS {
        return blank;
    }
    let middle = seen.len() / 2;
    let strength = (*seen.select_nth_unstable(middle).1).clamp(0, FULL) as u8;
    if strength < SOLID {
        return Slot {
            strength,
            ink: Vec::new(),
            count: 0,
        };
    }
    let mut ink = vec![0u64; (TEXT_COLUMNS * LINE_ROWS).div_ceil(64) as usize];
    let mut count = 0;
    for row in 0..LINE_ROWS {
        for col in from - layout.left..TEXT_COLUMNS {
            if diff(frame, layout.left + col, top + row) >= INK {
                let bit = (row * TEXT_COLUMNS + col) as usize;
                ink[bit / 64] |= 1 << (bit % 64);
                count += 1;
            }
        }
    }
    // Too few strokes for a line of text: something else at the stack's right edge, like a
    // monster walking past. It takes part as a faded line does, by lining up with anything.
    if count < MIN_INK {
        return Slot {
            strength: strength.min(SOLID - 1),
            ink: Vec::new(),
            count: 0,
        };
    }
    Slot {
        strength,
        ink,
        count,
    }
}

/// Every place the stack may be, rows down from where the picture's height puts it, `prefer` first.
fn shifts(prefer: i32) -> impl Iterator<Item = i32> {
    let max = MAX_SHIFT as i32;
    std::iter::once(prefer).chain((-max..=max).filter(move |&s| s != prefer))
}

/// Where the stack sits. In the right place, a solid line has no stroke ends in the row above it,
/// nor in its bottom row, where the game clips the shadow off; a row off either way, one of those
/// is a row of its strokes (the tops of its capitals, or its baseline). Each line is scored in its
/// own columns at its own strength, a stretch of [`VOTE_COLUMNS`] at a time, and each stretch votes
/// for the place where it has the fewest stroke ends there: a monster walking behind the stack, or
/// something drawn just above a line (the party's HP list ends there), spoils a few stretches, not
/// most of them. Wine's offset doesn't change while the game runs, so it moves only on a clear
/// majority. Without solid lines on screen, `prefer`.
fn find_shift(frame: &Frame, layout: Layout, prefer: i32) -> i32 {
    let mut votes = [0usize; 2 * MAX_SHIFT as usize + 1];
    for k in 0..SLOTS {
        let top = layout.top(k, prefer);
        let line = slot(frame, layout, top);
        if !line.solid() {
            continue;
        }
        let strong = i16::from(line.strength) * 3 / 4;
        let columns: Vec<u32> = (layout.left..layout.right)
            .filter(|&x| (top..top + LINE_ROWS).any(|y| diff(frame, x, y) >= strong))
            .collect();
        let (Some(&from), Some(&last)) = (columns.first(), columns.last()) else {
            continue;
        };
        let stray = |shift: i32, from: u32, to: u32| -> usize {
            let top = layout.top(k, shift);
            [top - 1, top + LINE_ROWS - 1]
                .into_iter()
                .map(|y| (from..to).filter(|&x| diff(frame, x, y) >= strong).count())
                .sum()
        };
        for start in (from..=last).step_by(VOTE_COLUMNS as usize) {
            let to = (start + VOTE_COLUMNS).min(last + 1);
            let scores: Vec<(usize, i32)> = shifts(prefer)
                .map(|shift| (stray(shift, start, to), shift))
                .collect();
            let best = scores.iter().map(|s| s.0).min().unwrap_or(0);
            // A stretch votes only when one place is plainly cleanest.
            if let [(_, shift)] = scores.iter().filter(|s| s.0 == best).collect::<Vec<_>>()[..]
                && scores.iter().all(|s| s.1 == *shift || s.0 >= best + 2)
            {
                votes[(shift + MAX_SHIFT as i32) as usize] += 1;
            }
        }
    }
    let vote = |shift: i32| votes[(shift + MAX_SHIFT as i32) as usize];
    shifts(prefer)
        .max_by_key(|&shift| (vote(shift), shift == prefer))
        .filter(|&shift| vote(shift) >= MIN_VOTES && vote(shift) >= 2 * vote(prefer))
        .unwrap_or(prefer)
}

/// One letter of a line: where its cell starts, and its shape.
#[derive(Debug, Clone)]
struct Piece {
    x: u32,
    glyph: Glyph,
}

/// A solid line cut into letters and words.
#[derive(Debug, Clone)]
struct Cut {
    words: Vec<Vec<Piece>>,
    yellow: bool,
}

/// Cuts the solid line whose top row is `top` into letters. `None` when something wider than a
/// letter is there, or the line runs past the columns read.
fn cut(frame: &Frame, layout: Layout, top: u32, strength: u8) -> Option<Cut> {
    let (w, h) = ((layout.right - layout.left) as usize, LINE_ROWS as usize);
    // Pixels at least this bright, against their shadow or as themselves, belong to the line.
    let bright = i16::from(strength) * 3 / 4;
    let at = |row: usize, col: usize| (layout.left + col as u32, top + row as u32);
    // How bright the line's stroke ends are: what its other pixels are measured against.
    let mut ends: Vec<i16> = (0..h - 1)
        .flat_map(|row| (0..w).map(move |col| (row, col)))
        .filter_map(|(row, col)| {
            let (x, y) = at(row, col);
            (diff(frame, x, y) >= bright).then(|| lum(frame.rgb(x, y)))
        })
        .collect();
    if ends.is_empty() {
        return None;
    }
    let middle = ends.len() / 2;
    let white = *ends.select_nth_unstable(middle).1;
    let mut core = vec![false; w * h];
    // The bottom row has no shadow: the game clips each line to its rows. Its pixels are the
    // ends of brackets and descenders, as bright as the line, under a bright pixel.
    for col in 0..w {
        let (x, y) = at(h - 1, col);
        let p = lum(frame.rgb(x, y));
        let above = (col.saturating_sub(1)..(col + 2).min(w))
            .any(|c| lum(frame.rgb(at(h - 2, c).0, y - 1)) >= bright);
        core[(h - 1) * w + col] = p >= bright && p + 12 >= white && above;
    }
    let (mut yellow, mut whites) = (0u32, 0u32);
    for row in (0..h - 1).rev() {
        for col in (0..w).rev() {
            let (x, y) = at(row, col);
            let p = frame.rgb(x, y);
            let end = diff(frame, x, y) >= bright;
            // A letter's pixel whose shadow is hidden under the letter's next pixel down and to
            // the right: as bright as that one, which belongs to the letter.
            let inside = !end
                && col + 1 < w
                && core[(row + 1) * w + col + 1]
                && lum(p) >= bright
                && lum(p) + 8 >= lum(frame.rgb(x + 1, y + 1));
            core[row * w + col] = end || inside;
            if end {
                if p[2] < p[0] / 2 {
                    yellow += 1;
                } else {
                    whites += 1;
                }
            }
        }
    }
    // A bottom-row pixel belongs to a letter only under one of its pixels.
    for col in 0..w {
        let under = (col.saturating_sub(1)..(col + 2).min(w)).any(|c| core[(h - 2) * w + c]);
        core[(h - 1) * w + col] &= under;
    }
    let inked = |col: usize| (0..h).any(|row| core[row * w + col]);
    if inked(0) {
        return None;
    }
    let mut words: Vec<Vec<Piece>> = Vec::new();
    let mut start = None;
    let mut last_end = None;
    for col in 0..=w {
        let ink = col < w && inked(col);
        match (start, ink) {
            (None, true) => start = Some(col),
            (Some(from), false) => {
                let width = (col - from) as u32;
                if width > MAX_GLYPH_WIDTH {
                    return None;
                }
                let rows = (0..h)
                    .map(|row| {
                        (from..col)
                            .filter(|&c| core[row * w + c])
                            .fold(0u32, |mask, c| mask | 1 << (c - from))
                    })
                    .collect();
                let glyph = Glyph::from_rows(width, rows)?;
                let piece = Piece {
                    x: from as u32,
                    glyph,
                };
                match (last_end, words.last_mut()) {
                    (Some(end), Some(word)) if (from - end) < SPACE as usize => word.push(piece),
                    _ => words.push(vec![piece]),
                }
                last_end = Some(col);
                start = None;
            }
            _ => {}
        }
    }
    // Lines end at the right: a word far left of the rest is something in the scene.
    let keep = (1..words.len())
        .rev()
        .find(|&i| {
            let end = words[i - 1].last().map_or(0, |p| p.x + p.glyph.width());
            words[i][0].x - end > MAX_SPACE
        })
        .unwrap_or(0);
    words.drain(..keep);
    (!words.is_empty()).then_some(Cut {
        words,
        yellow: yellow > whites,
    })
}

/// The game's phrases, word by word, for learning their letters from a line that splits into
/// exactly those words.
const EXP_PHRASE: &[&str] = &["You", "received", "EXP"];
const MESOS_PHRASE: &[&str] = &["You", "have", "gained", "mesos"];
const PARTY_PHRASE: &[&str] = &["Bonus", "EXP", "for", "PARTY"];
/// "PARTY" as the party bonus line draws it: T and Y touch, so they're one piece, named with
/// this stand-in for "TY".
const PARTY_PHRASE_JOINED: &[&str] = &["Bonus", "EXP", "for", "PAR\u{E000}"];
/// The same phrases as their letters run, for reading.
const EXP_TEXT: &str = "YoureceivedEXP";
const MESOS_TEXT: &str = "Youhavegainedmesos";
const PARTY_TEXT: &str = "BonusEXPforPARTY";
const PARTY_TEXT_JOINED: &str = "BonusEXPforPAR\u{E000}";
/// Letters a phrase may have unknown, wrong, missing or extra and still be read as that phrase:
/// two touching letters cost two, and a monster walking behind a word can cost three. The
/// phrases are far apart, and the amount after them must still read exactly.
const PHRASE_ERRORS: usize = 3;
/// Letters "earned." may have unknown, wrong, missing or extra in an item line, whose count
/// before it and "x" before that must still read exactly.
const EARNED_ERRORS: usize = 2;
/// Most digits and commas an amount has: "1,234,567".
const MAX_AMOUNT: usize = 9;

/// Whether the reader could have learned `c`: a letter of one of the game's phrases or tabs, a
/// digit, or an amount's punctuation.
fn learnable(c: char) -> bool {
    c.is_ascii_digit()
        || ",(+)x".contains(c)
        || [
            EXP_TEXT,
            MESOS_TEXT,
            PARTY_TEXT,
            PARTY_TEXT_JOINED,
            "earned.",
        ]
        .iter()
        .any(|text| text.contains(c))
        || Tab::ALL.iter().any(|tab| tab.name().contains(c))
}

/// How many letters differ between what was read and `text`, counting each unknown, wrong,
/// missing or extra letter once (edit distance).
fn distance(read: &[Option<char>], text: &str) -> usize {
    let text: Vec<char> = text.chars().collect();
    let mut row: Vec<usize> = (0..=text.len()).collect();
    for (i, r) in read.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, &c) in text.iter().enumerate() {
            let substitute = diagonal + usize::from(*r != Some(c));
            diagonal = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[text.len()]
}

/// How a line's words fit one of the game's phrases: the phrase's own letters and the glyphs that
/// show them, and the glyphs of its number.
struct Fit<'a> {
    fixed: Vec<(&'a Glyph, char)>,
    number: Vec<&'a Piece>,
}

/// Pairs each glyph of `words` with the letter of `texts` in its place, when the word lengths fit.
fn spell<'a>(words: &'a [Vec<Piece>], texts: &[&str], fixed: &mut Vec<(&'a Glyph, char)>) -> bool {
    let fits = words.len() == texts.len()
        && words
            .iter()
            .zip(texts)
            .all(|(word, text)| word.len() == text.chars().count());
    if fits {
        for (word, text) in words.iter().zip(texts) {
            fixed.extend(word.iter().map(|p| &p.glyph).zip(text.chars()));
        }
    }
    fits
}

/// The amount at the end of a line, "(+N)": its digits, with the brackets and plus added to
/// `fixed`.
fn amount<'a>(pieces: &[&'a Piece], fixed: &mut Vec<(&'a Glyph, char)>) -> Option<Vec<&'a Piece>> {
    let last = pieces
        .len()
        .checked_sub(1)
        .filter(|last| (3..=MAX_AMOUNT + 2).contains(last))?;
    fixed.push((&pieces[0].glyph, '('));
    fixed.push((&pieces[1].glyph, '+'));
    fixed.push((&pieces[last].glyph, ')'));
    Some(pieces[2..last].to_vec())
}

/// The words of an amount line, `<phrase> (+N)`, fitted to `phrase`. Everything after the
/// phrase is the amount: a 1 leaves as wide a gap after it as a space, so "(+122)" comes cut
/// into "(+1" and "22)".
fn fit_amount<'a>(cut: &'a Cut, phrase: &[&str]) -> Option<Fit<'a>> {
    if cut.words.len() <= phrase.len() {
        return None;
    }
    let (words, rest) = cut.words.split_at(phrase.len());
    let mut fixed = Vec::new();
    if !spell(words, phrase, &mut fixed) {
        return None;
    }
    let pieces: Vec<&Piece> = rest.iter().flatten().collect();
    let number = amount(&pieces, &mut fixed)?;
    Some(Fit { fixed, number })
}

/// An item line, `<name> xN earned. (<tab>)`, with the tab `tab`.
fn fit_item(cut: &Cut, tab: Option<Tab>) -> Option<Fit<'_>> {
    let n = cut.words.len();
    if n < 4 {
        return None;
    }
    let rest = &cut.words[n - 3..];
    let (count, earned, bracket) = (&rest[0], &rest[1], &rest[2]);
    let mut fixed = Vec::new();
    if count.len() < 2 || !spell(std::slice::from_ref(earned), &["earned."], &mut fixed) {
        return None;
    }
    fixed.push((&count[0].glyph, 'x'));
    let (open, close) = (bracket.first()?, bracket.last()?);
    let letters = bracket.len().checked_sub(2).filter(|&n| n >= 3)?;
    fixed.push((&open.glyph, '('));
    fixed.push((&close.glyph, ')'));
    if let Some(tab) = tab {
        if tab.name().len() != letters {
            return None;
        }
        fixed.extend(
            bracket[1..=letters]
                .iter()
                .map(|p| &p.glyph)
                .zip(tab.name().chars()),
        );
    }
    Some(Fit {
        fixed,
        number: count[1..].iter().collect(),
    })
}

/// How many of the fit's letters the atlas already names as the fit does, or `None` when the fit
/// contradicts the atlas or itself (one shape as two letters, or one letter as two shapes).
fn agreement(atlas: &Atlas, fixed: &[(&Glyph, char)]) -> Option<usize> {
    let mut trial = atlas.clone();
    let glyphs: Vec<Glyph> = fixed.iter().map(|(g, _)| (*g).clone()).collect();
    let text: String = fixed.iter().map(|(_, c)| *c).collect();
    trial.learn(&glyphs, &text).ok()?;
    Some(
        fixed
            .iter()
            .filter(|(g, c)| atlas.name(g) == Some(*c))
            .count(),
    )
}

/// Learns the fit's letters, if they agree with the atlas.
fn learn_fixed(atlas: &mut Atlas, fixed: &[(&Glyph, char)]) -> bool {
    let glyphs: Vec<Glyph> = fixed.iter().map(|(g, _)| (*g).clone()).collect();
    let text: String = fixed.iter().map(|(_, c)| *c).collect();
    atlas.learn(&glyphs, &text).is_ok()
}

/// Digits with a comma every three from the right: `1,234`.
fn group_digits(digits: &str) -> String {
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            text.push(',');
        }
        text.push(c);
    }
    text
}

/// A number as a notice may write it: digits, with commas every three from the right or none.
fn parse_number(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(|&c| c != ',').collect();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if text.contains(',') && group_digits(&digits) != text {
        return None;
    }
    digits.parse().ok()
}

/// The number the glyphs show, when every one is a known digit or comma.
fn read_number<'a>(atlas: &Atlas, glyphs: impl IntoIterator<Item = &'a Glyph>) -> Option<u64> {
    let text: Option<String> = glyphs.into_iter().map(|g| atlas.name(g)).collect();
    parse_number(&text?)
}

/// An EXP line waiting for the HUD's EXP to say what it's worth: its phrase's letters and its
/// amount's glyphs, and the amount if they're all known.
#[derive(Debug, Clone)]
struct ExpLine {
    fixed: Vec<(Glyph, char)>,
    digits: Vec<Glyph>,
    value: Option<u64>,
}

impl ExpLine {
    fn of(fit: &Fit, atlas: &Atlas) -> ExpLine {
        ExpLine {
            fixed: fit.fixed.iter().map(|(g, c)| ((*g).clone(), *c)).collect(),
            digits: fit.number.iter().map(|p| p.glyph.clone()).collect(),
            value: read_number(atlas, fit.number.iter().map(|p| &p.glyph)),
        }
    }

    /// Learns its letters, and its digits as `amount`, if they agree with the atlas.
    fn teach(&self, atlas: &mut Atlas, amount: u64) -> bool {
        let mut trial = atlas.clone();
        let (glyphs, text): (Vec<Glyph>, String) = self.fixed.iter().cloned().unzip();
        if trial.learn(&glyphs, &text).is_err() {
            return false;
        }
        let plain = amount.to_string();
        for text in [group_digits(&plain), plain] {
            let mut digits = trial.clone();
            if text.chars().count() == self.digits.len()
                && digits.learn(&self.digits, &text).is_ok()
            {
                *atlas = digits;
                return true;
            }
        }
        false
    }
}

/// EXP lines read and EXP the HUD gained since they last matched.
#[derive(Debug, Clone, Default)]
struct Window {
    lines: Vec<ExpLine>,
    hud: u64,
    /// Frames since the window opened.
    age: u32,
    /// A line in the window that may have carried EXP couldn't be read, so the window can't
    /// teach or check anything.
    blocked: bool,
}

/// What the HUD said the EXP did since the last frame it was read in.
enum Gain {
    /// Not read in this frame.
    Unknown,
    /// Read, but not comparable with the last reading: the first one, a death, a skipped level.
    Restart,
    Gained(u64),
}

/// The EXP gained from `before` to `now`, across a level-up when the level's EXP needed is known.
fn gained(before: &ExpReading, now: &ExpReading) -> Option<u64> {
    match (before.level, now.level) {
        (a, b) if a == b => now.exp.checked_sub(before.exp),
        (Some(a), Some(b)) if b == a + 1 => Some(before.needed?.checked_sub(before.exp)? + now.exp),
        _ => None,
    }
}

/// A mesos amount read with a digit not learned yet, kept to count once its digits are known.
#[derive(Debug, Clone)]
struct Unread {
    /// Its digits' glyphs, commas included.
    digits: Vec<Glyph>,
}

/// A line read from the stack.
enum Read {
    Exp(ExpLine),
    PartyBonus(ExpLine),
    /// Read, or with a digit not learned yet.
    Mesos(Result<u64, Unread>),
    Item(Option<(ItemName, u32, Option<Tab>)>),
    Other,
    /// It may be a pickup, but it can't be read.
    Unknown,
}

impl Read {
    /// Read for sure: nothing a better frame of it could add.
    fn sure(&self) -> bool {
        match self {
            Read::Exp(line) | Read::PartyBonus(line) => line.value.is_some(),
            Read::Mesos(amount) => amount.is_ok(),
            Read::Item(_) | Read::Other => true,
            Read::Unknown => false,
        }
    }
}

/// Follows the notice stack from frame to frame and reads each new line once.
#[derive(Debug, Clone)]
pub struct NoticeReader {
    atlas: Atlas,
    /// The stack last frame, bottom first; `None` after a frame that couldn't be read.
    lines: Option<Vec<Slot>>,
    /// The stack was being followed and then wasn't.
    lost: bool,
    /// Where the stack was last found, rows down from where the picture's height puts it.
    shift: i32,
    /// Slots of last frame's lines that arrived but couldn't be read for sure yet.
    pending: Vec<usize>,
    /// Mesos amounts read this session with a digit not learned yet, oldest first.
    unread: std::collections::VecDeque<Unread>,
    /// Digits named by their shapes that no EXP line checked against the HUD has shown yet.
    shaped: Vec<char>,
    /// A digit named by its shape turned out wrong: no more are named that way.
    shapes_failed: bool,
    /// Kept mesos amounts read since the last frame, now that their digits are known.
    late: Vec<u64>,
    /// How much a line faded from one frame to the next in the last frames measured, newest last:
    /// about 9 at four frames a second, 45 at one.
    fades: Vec<u8>,
    /// The last confirmed HUD reading.
    hud: Option<ExpReading>,
    window: Window,
    /// EXP amounts read that matched the HUD, in a row, while learning.
    checks: usize,
    /// EXP amounts that didn't, in a row, while learning.
    misses: usize,
    confirmed: bool,
    /// Started from letters kept in an earlier session ([`NoticeReader::from_knowledge`]) that
    /// no EXP line has checked against the HUD yet this session: how many EXP lines in a row they
    /// couldn't read.
    restored: Option<usize>,
}

impl Default for NoticeReader {
    fn default() -> Self {
        Self::new()
    }
}

impl NoticeReader {
    pub fn new() -> Self {
        NoticeReader {
            atlas: Atlas::default(),
            lines: None,
            lost: false,
            shift: 0,
            pending: Vec::new(),
            unread: std::collections::VecDeque::new(),
            shaped: Vec::new(),
            shapes_failed: false,
            late: Vec::new(),
            fades: Vec::new(),
            hud: None,
            window: Window::default(),
            checks: 0,
            misses: 0,
            confirmed: false,
            restored: None,
        }
    }

    pub fn confirmed(&self) -> bool {
        self.confirmed
    }

    /// What the reader has confirmed, as text to keep on the player's device so the next session
    /// reads from its first pickup ([`NoticeReader::from_knowledge`]): each letter's shape. The
    /// shapes never leave the device. `None` until the letters are confirmed.
    pub fn knowledge(&self) -> Option<String> {
        if !self.confirmed {
            return None;
        }
        let mut text = format!("{KNOWLEDGE_HEADER}\n");
        for (name, glyph) in self.atlas.entries() {
            // A digit named by its shape says so, so the next session still drops it if an EXP
            // line disagrees.
            let shaped = match self.shaped.contains(&name) {
                true => " shape",
                false => "",
            };
            text.push_str(&format!(
                "{:x} {}{shaped}\n",
                u32::from(name),
                glyph.encode()
            ));
        }
        if self.shapes_failed {
            text.push_str("no-shapes\n");
        }
        Some(text)
    }

    /// A reader that starts from [`NoticeReader::knowledge`] kept in an earlier session, so it
    /// reads the first pickup. If the kept letters can't read EXP lines that are plainly there,
    /// as after a game update that redrew the font, it drops them and learns again. `None` for
    /// text it couldn't have written.
    pub fn from_knowledge(text: &str) -> Option<NoticeReader> {
        let mut lines = text.lines();
        if lines.next()? != KNOWLEDGE_HEADER {
            return None;
        }
        let mut reader = NoticeReader::new();
        for line in lines.filter(|l| !l.is_empty()) {
            let parts: Vec<&str> = line.split(' ').collect();
            let (name, glyph, shaped) = match parts[..] {
                ["no-shapes"] => {
                    reader.shapes_failed = true;
                    continue;
                }
                [name, glyph] => (name, glyph, false),
                [name, glyph, "shape"] => (name, glyph, true),
                _ => return None,
            };
            let name = u32::from_str_radix(name, 16)
                .ok()
                .and_then(char::from_u32)
                .filter(|&c| learnable(c))?;
            reader
                .atlas
                .learn(&[Glyph::decode(glyph)?], &name.to_string())
                .ok()?;
            if shaped {
                reader.shaped.push(name);
            }
        }
        if reader.atlas.is_empty() {
            return None;
        }
        reader.confirmed = true;
        reader.restored = Some(0);
        Some(reader)
    }

    /// The new lines in this frame, which holds at least [`rows_needed`] rows. `hud` is the EXP
    /// bar's reading of the same frame, when it has one: the EXP lines are checked against it, and
    /// learned from it.
    pub fn observe(
        &mut self,
        frame: &Frame,
        hud: Option<&ExpReading>,
    ) -> Result<NoticeReading, NoticeUnreadable> {
        let result = unstretch::with_bottom_right_1x(frame, NOTICE_ROWS, NOTICE_COLUMNS, |p| {
            self.observe_1x(p, hud)
        })
        .map_err(NoticeUnreadable::Unsupported)
        .and_then(|r| r);
        if result.is_err() {
            self.reset();
        }
        result
    }

    /// Forgets the stack on screen, for when capture stopped or the game window changed. Lines
    /// on screen when it's next seen aren't counted, and the reading says lines may be missing.
    pub fn reset(&mut self) {
        self.lost |= self.lines.is_some();
        self.lines = None;
        self.pending.clear();
    }

    /// For a frame the game dimmed to change maps: the stack can't be followed, and nothing can be
    /// picked up until the new map shows. Lines on screen when it's back were counted already, so
    /// they aren't counted again, and nothing is reported missed but lines still waiting to be
    /// read again.
    pub fn screen_dark(&mut self) -> NoticeReading {
        let mut notices = Vec::new();
        let mut missed = None;
        for _ in std::mem::take(&mut self.pending) {
            self.accept(Read::Unknown, &mut notices, &mut missed);
        }
        self.lines = None;
        NoticeReading {
            notices,
            missed,
            confirmed: self.confirmed,
        }
    }

    /// Starts over for a game that was closed and started again, maybe with another character:
    /// nothing on screen carries over, and nothing could be picked up meanwhile. What was learned
    /// stays.
    pub fn restart(&mut self) {
        self.lines = None;
        self.pending.clear();
        self.unread.clear();
        self.late.clear();
        self.lost = false;
        self.hud = None;
        self.window = Window::default();
    }

    fn observe_1x(
        &mut self,
        picture: &Frame,
        hud: Option<&ExpReading>,
    ) -> Result<NoticeReading, NoticeUnreadable> {
        let layout = Layout::of(picture).ok_or(NoticeUnreadable::UnsupportedSize {
            width: picture.width(),
            height: picture.height(),
        })?;
        let shift = find_shift(picture, layout, self.shift);
        self.shift = shift;
        let now: Vec<Slot> = (0..SLOTS)
            .map(|k| slot(picture, layout, layout.top(k, shift)))
            .collect();
        let mut missed = None;
        let most = self.most_fading();
        let fresh = match self.lines.as_deref() {
            Some(before) => lines::arrived_by(before, &now, |b, n, moved| {
                b.matches(n, moved) && still_there(b, n, most)
            })
            .or_else(|| steady(before, &now, most).then_some(0))
            .unwrap_or_else(|| {
                missed = Some(Missed::Unaligned);
                newer_than(before, &now)
            }),
            None => {
                if self.lost {
                    missed = Some(Missed::Lost);
                }
                0
            }
        };
        if missed.is_none() {
            self.measure_fade(&now, fresh);
        }
        // Lines that couldn't be read for sure when they arrived, where the stack moved them, to
        // be read again: something moving behind a line (a monster walking past) spoils some
        // frames of it and not others. Missed when the stack can't be followed, or a line
        // scrolled off.
        let mut notices = Vec::new();
        let waiting = std::mem::take(&mut self.pending);
        let mut lines: Vec<(usize, bool)> = Vec::new();
        for k in waiting.into_iter().map(|k| k + fresh) {
            match missed.is_none() && k < SLOTS {
                true => lines.push((k, false)),
                false => self.accept(Read::Unknown, &mut notices, &mut missed),
            }
        }
        lines.extend((0..fresh).map(|k| (k, true)));
        // Oldest first: the highest line.
        lines.sort_by_key(|&(k, _)| std::cmp::Reverse(k));
        for (k, arrived) in lines {
            let line = &now[k];
            // Too faint to tell every pixel of its letters: missed.
            if line.strength < READ {
                self.accept(Read::Unknown, &mut notices, &mut missed);
                continue;
            }
            let (read, damaged) = cut(picture, layout, layout.top(k, shift), line.strength)
                .map_or((Read::Unknown, true), |cut| self.read(&cut, arrived));
            // Read again in the next frame if it will still be strong enough then (only when
            // frames come often), when something passing behind it may have spoiled it: it
            // couldn't be read, or its amount has a glyph that isn't a known digit with letters of
            // its phrase spoiled too. A digit not learned yet comes in a phrase read whole. Only
            // the read that stands teaches an amount from the HUD or says a digit isn't learned.
            if !read.sure() && damaged && line.strength.saturating_sub(self.fade()) >= READ {
                self.pending.push(k);
                continue;
            }
            self.accept(read, &mut notices, &mut missed);
        }
        self.lines = Some(now);
        self.lost = false;
        // Lines may have come and gone unseen: the EXP window can't teach or check anything.
        if matches!(missed, Some(Missed::Unaligned | Missed::Lost)) {
            self.window.blocked = true;
        }
        let gain = self.follow_hud(hud);
        if let Some(short) = self.settle(gain, !self.pending.is_empty()) {
            missed.get_or_insert(short);
        }
        self.resolve();
        notices.extend(self.late.drain(..).map(Notice::LateMesos));
        Ok(NoticeReading {
            notices,
            missed,
            confirmed: self.confirmed,
        })
    }

    /// Takes what was read of a new line: its notice, its EXP for the HUD to check, or why it
    /// may have been missed.
    fn accept(&mut self, read: Read, notices: &mut Vec<Notice>, missed: &mut Option<Missed>) {
        match read {
            Read::Exp(line) => {
                notices.extend(line.value.map(Notice::Exp));
                self.window.lines.push(line);
            }
            Read::PartyBonus(line) => {
                notices.extend(line.value.map(Notice::PartyBonus));
                self.window.lines.push(line);
            }
            Read::Mesos(Ok(amount)) => notices.push(Notice::Mesos(amount)),
            Read::Item(Some((name, count, tab))) => {
                notices.push(Notice::Item { name, count, tab });
            }
            Read::Other => notices.push(Notice::Other),
            // A mesos line whose amount has a digit not learned yet: missed for now, and kept to
            // count once its digits are known. It carried no EXP, so the EXP lines can still
            // teach and check.
            Read::Mesos(Err(unread)) => {
                missed.get_or_insert(Missed::NotLearned);
                self.unread.push_back(unread);
                if self.unread.len() > UNREAD_KEPT {
                    self.unread.pop_front();
                }
            }
            // An item count too large to be one.
            Read::Item(None) => {
                missed.get_or_insert(Missed::Unreadable);
            }
            Read::Unknown => {
                missed.get_or_insert(Missed::Unreadable);
                self.window.blocked = true;
            }
        }
    }

    /// What a solid line says, and whether its phrase was read with letters wrong or unknown, as
    /// when something behind it spoiled it. `arrived` is false when the line is being read
    /// again: kept letters are checked against each line only once.
    fn read(&mut self, cut: &Cut, arrived: bool) -> (Read, bool) {
        if arrived {
            self.check_restored(cut);
        }
        if self.atlas.is_empty() {
            // Before any letter is known, a line shaped like an EXP line waits for the HUD to
            // name its letters and digits.
            let read = match fit_amount(cut, EXP_PHRASE) {
                Some(fit) if !cut.yellow && agreement(&self.atlas, &fit.fixed).is_some() => {
                    Read::Exp(ExpLine::of(&fit, &self.atlas))
                }
                _ if cut.yellow => Read::Unknown,
                _ => Read::Other,
            };
            return (read, false);
        }
        self.learn_from(cut);
        self.parse(cut)
    }

    /// How many mesos amounts with a digit not learned yet are waiting to be read.
    pub fn waiting_mesos(&self) -> usize {
        self.unread.len()
    }

    /// Names digits no EXP amount has shown yet by their shapes ([`digit_shapes`]), then reads the
    /// kept mesos amounts whose digits are all known now.
    fn resolve(&mut self) {
        self.name_by_shape();
        let atlas = &self.atlas;
        let late = &mut self.late;
        self.unread
            .retain(|unread| match read_number(atlas, &unread.digits) {
                Some(amount) => {
                    late.push(amount);
                    false
                }
                None => true,
            });
    }

    /// Names the digits of waiting mesos amounts by their shapes, when the shape rules name every
    /// digit learned so far the way it was learned, at least [`DIGITS_CHECKED`] of them. Only a
    /// glyph as tall as those digits and in two or more waiting amounts is named, and only when no
    /// other glyph looks like the same digit: a speck left by a monster passing isn't a digit.
    fn name_by_shape(&mut self) {
        if self.shapes_failed || self.unread.is_empty() {
            return;
        }
        let known: Vec<(char, Glyph)> = self
            .atlas
            .entries()
            .into_iter()
            .filter(|(name, _)| name.is_ascii_digit())
            .map(|(name, glyph)| (name, glyph.clone()))
            .collect();
        let rows = known.first().map(|(_, glyph)| glyph.ink_rows());
        let fits = |glyph: &Glyph| Some(glyph.ink_rows()) == rows;
        if known.len() < DIGITS_CHECKED
            || known
                .iter()
                .any(|(name, glyph)| digit_shapes::guess(glyph) != Some(*name) || !fits(glyph))
        {
            return;
        }
        let mut found: HashMap<Glyph, usize> = HashMap::new();
        for unread in &self.unread {
            let mut glyphs: Vec<&Glyph> = Vec::new();
            for glyph in &unread.digits {
                if self.atlas.name(glyph).is_none() && fits(glyph) && !glyphs.contains(&glyph) {
                    glyphs.push(glyph);
                }
            }
            for glyph in glyphs {
                *found.entry(glyph.clone()).or_default() += 1;
            }
        }
        let mut by_digit: HashMap<char, Vec<Glyph>> = HashMap::new();
        for (glyph, amounts) in found {
            let digit = digit_shapes::guess(&glyph).filter(|&d| self.atlas.shape(d).is_none());
            if let Some(digit) = digit.filter(|_| amounts >= 2) {
                by_digit.entry(digit).or_default().push(glyph);
            }
        }
        for (digit, glyphs) in by_digit {
            if let [glyph] = &glyphs[..]
                && self
                    .atlas
                    .learn(std::slice::from_ref(glyph), &digit.to_string())
                    .is_ok()
            {
                self.shaped.push(digit);
            }
        }
    }

    /// Drops every digit named by its shape, one of them having turned out wrong, and names no
    /// more that way.
    fn drop_shaped(&mut self) {
        for digit in self.shaped.drain(..) {
            self.atlas.forget(digit);
        }
        self.shapes_failed = true;
    }

    /// Measures how much the solid lines still on screen faded since the last frame, `fresh` new
    /// lines having pushed them up: the middle of their drops.
    fn measure_fade(&mut self, now: &[Slot], fresh: usize) {
        let Some(before) = self.lines.as_deref() else {
            return;
        };
        let mut drops: Vec<u8> = before
            .iter()
            .zip(now.iter().skip(fresh))
            .filter(|(b, n)| b.solid() && n.solid() && n.strength <= b.strength)
            .map(|(b, n)| b.strength - n.strength)
            .collect();
        if !drops.is_empty() {
            let middle = drops.len() / 2;
            self.fades.push(*drops.select_nth_unstable(middle).1);
            if self.fades.len() > FADES_KEPT {
                self.fades.remove(0);
            }
        }
    }

    /// How much a line fades from one frame to the next: the middle of the last frames measured,
    /// as the time between frames varies. Unmeasured, as much as a line has, so nothing waits to
    /// be read again.
    fn fade(&self) -> u8 {
        let mut fades = self.fades.clone();
        match fades.len() {
            0 => u8::MAX,
            n => *fades.select_nth_unstable(n / 2).1,
        }
    }

    /// The most a line's strength may drop from one frame to the next: [`LATE_FRAMES`] frames'
    /// fading, and [`BRIGHTER`] for measuring it over another part of the scene. Unmeasured, any.
    fn most_fading(&self) -> u8 {
        self.fade()
            .saturating_mul(LATE_FRAMES)
            .saturating_add(BRIGHTER)
    }

    /// Checks letters kept from an earlier session against a line shaped like an EXP line. Kept
    /// letters that can't read most of its phrase count against them, and after
    /// [`RESTORE_PATIENCE`] such lines in a row they're dropped and learned again. Lines drawn
    /// over by something else rarely split into the phrase's words, so they don't count.
    fn check_restored(&mut self, cut: &Cut) {
        let Some(strikes) = self.restored else {
            return;
        };
        let Some(fit) = fit_amount(cut, EXP_PHRASE).filter(|_| !cut.yellow) else {
            return;
        };
        let reads =
            agreement(&self.atlas, &fit.fixed).is_some_and(|known| known * 2 >= fit.fixed.len());
        self.restored = match reads {
            true => Some(0),
            false if strikes + 1 < RESTORE_PATIENCE => Some(strikes + 1),
            false => {
                self.atlas = Atlas::default();
                self.confirmed = false;
                self.checks = 0;
                self.misses = 0;
                self.window = Window::default();
                None
            }
        };
    }

    /// Learns the letters of a line that splits exactly into one of the game's phrases, when the
    /// atlas already knows at least half of them as that phrase has them.
    fn learn_from(&mut self, cut: &Cut) {
        let amounts = match cut.yellow {
            true => [
                fit_amount(cut, PARTY_PHRASE),
                fit_amount(cut, PARTY_PHRASE_JOINED),
            ],
            false => [fit_amount(cut, EXP_PHRASE), fit_amount(cut, MESOS_PHRASE)],
        };
        let mut fits: Vec<Fit> = amounts.into_iter().flatten().collect();
        // The tab whose letters the atlas knows best, if just one does.
        let tabs: Vec<(usize, Fit)> = Tab::ALL
            .iter()
            .filter_map(|&tab| fit_item(cut, Some(tab)))
            .filter_map(|fit| agreement(&self.atlas, &fit.fixed).map(|n| (n, fit)))
            .collect();
        let best = tabs.iter().map(|(n, _)| *n).max();
        match best {
            Some(n) if tabs.iter().filter(|(m, _)| *m == n).count() == 1 => {
                fits.extend(tabs.into_iter().find(|(m, _)| *m == n).map(|(_, fit)| fit));
            }
            _ => fits.extend(fit_item(cut, None)),
        }
        let known = |fit: &Fit| {
            agreement(&self.atlas, &fit.fixed).is_some_and(|known| known * 2 >= fit.fixed.len())
        };
        if let Some(fit) = fits.into_iter().find(known) {
            learn_fixed(&mut self.atlas, &fit.fixed);
        }
    }

    /// Reads a line from its end with the letters known: every pickup ends with a bracket. A
    /// letter drawn over, or two touching, costs a letter here and there, not the line: also
    /// whether it cost any in an amount's phrase.
    fn parse(&self, cut: &Cut) -> (Read, bool) {
        let pieces: Vec<&Piece> = cut.words.iter().flatten().collect();
        let names: Vec<Option<char>> = pieces.iter().map(|p| self.atlas.name(&p.glyph)).collect();
        let n = names.len();
        match names.last() {
            Some(Some(')')) => {}
            // A white line ending in a letter it knows, like "This item is unavailable for
            // pick-up.": not a pickup. One ending in a letter it doesn't know may be one.
            Some(Some(_)) if !cut.yellow => return (Read::Other, false),
            _ => return (Read::Unknown, true),
        }
        // An amount: a phrase, then "(+", digits and ")".
        let open = (n.saturating_sub(MAX_AMOUNT + 3)..n.saturating_sub(2))
            .rev()
            .find(|&i| names[i] == Some('(') && names[i + 1] == Some('+'));
        if let Some(open) = open {
            let phrase = &names[..open];
            let line = ExpLine {
                fixed: Vec::new(),
                digits: pieces[open + 2..n - 1]
                    .iter()
                    .map(|p| p.glyph.clone())
                    .collect(),
                value: read_number(
                    &self.atlas,
                    pieces[open + 2..n - 1].iter().map(|p| &p.glyph),
                ),
            };
            let off = |text: &str| distance(phrase, text);
            let (read, errors) = match cut.yellow {
                true => (
                    Read::PartyBonus(line),
                    off(PARTY_TEXT).min(off(PARTY_TEXT_JOINED)),
                ),
                false => match (off(EXP_TEXT), off(MESOS_TEXT)) {
                    (exp, mesos) if exp <= mesos => (Read::Exp(line), exp),
                    (_, mesos) => {
                        let amount = line.value.ok_or_else(|| Unread {
                            digits: line.digits.clone(),
                        });
                        (Read::Mesos(amount), mesos)
                    }
                },
            };
            return match errors {
                0 => (read, false),
                errors if errors <= PHRASE_ERRORS => (read, true),
                _ => (Read::Unknown, true),
            };
        }
        match self.parse_item(&pieces, &names) {
            Some(read) => (read, false),
            None => (Read::Unknown, true),
        }
    }

    /// An item line: its name, "x", the count, "earned.", and the tab in brackets.
    fn parse_item(&self, pieces: &[&Piece], names: &[Option<char>]) -> Option<Read> {
        let n = names.len();
        let open = (n.checked_sub(7)?..=n.checked_sub(5)?)
            .rev()
            .find(|&i| names[i] == Some('('))?;
        let tab_names = &names[open + 1..n - 1];
        let tab = Tab::ALL.into_iter().find(|t| {
            t.name().len() == tab_names.len()
                && tab_names
                    .iter()
                    .zip(t.name().chars())
                    .filter(|(name, c)| **name != Some(*c))
                    .count()
                    <= 1
        });
        // "earned." as a run of letters before the bracket, as long as it reads, or shorter or
        // longer for letters that touch or a speck: the first that the count and "x" before it
        // fit.
        let (x, count_start, count_end) = [7, 6, 8, 5, 9]
            .into_iter()
            .filter(|&len| {
                open >= len && distance(&names[open - len..open], "earned.") <= EARNED_ERRORS
            })
            .find_map(|len| {
                let count_end = open - len;
                let count_start = (0..count_end)
                    .rev()
                    .take_while(|&i| names[i].is_some_and(|c| c.is_ascii_digit()))
                    .last()?;
                let x = count_start
                    .checked_sub(1)
                    .filter(|&x| x > 0 && names[x] == Some('x'))?;
                Some((x, count_start, count_end))
            })?;
        let count = read_number(
            &self.atlas,
            pieces[count_start..count_end].iter().map(|p| &p.glyph),
        )?;
        Some(Read::Item(
            u32::try_from(count)
                .ok()
                .map(|count| (self.name(&pieces[..x]), count, tab)),
        ))
    }

    /// An item's name: its key, its text so far, and its picture.
    fn name(&self, pieces: &[&Piece]) -> ItemName {
        // FNV-1a over the letters' shapes, which don't depend on how wide the gaps between them
        // came out: stable across sessions and machines.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut text = String::new();
        let mut last_end = None;
        for piece in pieces {
            for &b in piece.glyph.encode().as_bytes().iter().chain(b";") {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
            if last_end.is_some_and(|end| piece.x - end >= SPACE) {
                text.push(' ');
            }
            text.push(self.atlas.name(&piece.glyph).unwrap_or('?'));
            last_end = Some(piece.x + piece.glyph.width());
        }
        ItemName {
            key: format!("{hash:016x}"),
            text,
            picture: picture_of(pieces),
        }
    }

    fn follow_hud(&mut self, hud: Option<&ExpReading>) -> Gain {
        let Some(now) = hud.filter(|r| r.confirmed) else {
            return Gain::Unknown;
        };
        let gain = self.hud.as_ref().and_then(|before| gained(before, now));
        self.hud = Some(*now);
        gain.map_or(Gain::Restart, Gain::Gained)
    }

    /// Checks the EXP lines read against the HUD's gain, learns digits from a single EXP line,
    /// and confirms the reader. The reason lines were missed, if the two don't add up. While a
    /// line is `waiting` to be read again, which may be an EXP line, nothing is concluded.
    fn settle(&mut self, gain: Gain, waiting: bool) -> Option<Missed> {
        match gain {
            Gain::Gained(g) => self.window.hud += g,
            Gain::Restart => {
                self.window = Window::default();
                return None;
            }
            // No HUD to check against: a window can't be judged until there's one again.
            Gain::Unknown if self.hud.is_none() => {
                self.window = Window::default();
                return None;
            }
            Gain::Unknown => return None,
        }
        let window = &mut self.window;
        if window.lines.is_empty() && window.hud == 0 {
            *window = Window::default();
            return None;
        }
        window.age += 1;
        if waiting {
            return None;
        }
        let read: Option<u64> = window.lines.iter().map(|l| l.value).sum();
        if read == Some(window.hud) && !window.lines.is_empty() {
            if !window.blocked && !self.confirmed {
                self.checks += 1;
                self.misses = 0;
                self.confirmed = self.checks >= CONFIRM_AFTER;
            }
            // Letters kept from an earlier session read an EXP amount the HUD agrees with, and so
            // did any digits named by their shapes.
            if !window.blocked {
                self.restored = None;
                let atlas = &self.atlas;
                let shown: Vec<char> = window
                    .lines
                    .iter()
                    .flat_map(|line| &line.digits)
                    .filter_map(|glyph| atlas.name(glyph))
                    .collect();
                self.shaped.retain(|digit| !shown.contains(digit));
            }
            self.window = Window::default();
            return None;
        }
        // One line whose digits aren't all known: the HUD says what it's worth. A digit named by
        // its shape that's in the way is wrong.
        let unknown: Vec<ExpLine> = window
            .lines
            .iter()
            .filter(|l| l.value.is_none())
            .cloned()
            .collect();
        if let [line] = &unknown[..] {
            let rest: u64 = window.lines.iter().filter_map(|l| l.value).sum();
            let (blocked, gained) = (window.blocked, window.hud);
            if !blocked && gained > rest {
                let mut taught = line.teach(&mut self.atlas, gained - rest);
                if !taught && !self.shaped.is_empty() {
                    self.drop_shaped();
                    taught = line.teach(&mut self.atlas, gained - rest);
                }
                if taught {
                    self.window = Window::default();
                    return None;
                }
            }
        }
        let window = &mut self.window;
        if window.age < SETTLE {
            return None;
        }
        let short = read.is_none_or(|read| read < window.hud);
        let evidence = !window.blocked && read.is_some();
        // EXP lines read with a digit named by its shape that don't add up: that digit is wrong.
        let atlas = &self.atlas;
        let used_shaped = window
            .lines
            .iter()
            .flat_map(|line| &line.digits)
            .filter_map(|glyph| atlas.name(glyph))
            .any(|digit| self.shaped.contains(&digit));
        self.window = Window::default();
        if evidence && used_shaped {
            self.drop_shaped();
        }
        if self.confirmed {
            return Some(if short {
                Missed::ExpShort
            } else {
                Missed::ExpOver
            });
        }
        // EXP amounts all read that don't add up to the HUD's gain are evidence against the
        // digits' names; a window with a line that couldn't be read is not.
        if evidence {
            self.checks = 0;
            self.misses += 1;
            if self.misses >= FORGET_AFTER {
                self.atlas = Atlas::default();
                self.misses = 0;
            }
        }
        None
    }
}

/// The letters of `pieces` as the game drew them, with a pixel of margin each side.
fn picture_of(pieces: &[&Piece]) -> NamePicture {
    let from = pieces.first().map_or(0, |p| p.x);
    let to = pieces.last().map_or(0, |p| p.x + p.glyph.width());
    let width = to - from + 2;
    let mut coverage = vec![0u8; (width * LINE_ROWS) as usize];
    for piece in pieces {
        for row in 0..LINE_ROWS {
            for dx in 0..piece.glyph.width() {
                if piece.glyph.is_ink(dx, row) {
                    let col = piece.x - from + 1 + dx;
                    coverage[(row * width + col) as usize] = 255;
                }
            }
        }
    }
    NamePicture {
        width,
        height: LINE_ROWS,
        coverage,
    }
}

/// After a frame that lines up with nothing: the new lines at the bottom that are surely new,
/// brighter than every line in the frame before.
fn newer_than(before: &[Slot], now: &[Slot]) -> usize {
    let brightest = before.iter().map(|s| s.strength).max().unwrap_or(0);
    now.iter()
        .take_while(|s| s.solid() && s.strength > brightest.saturating_add(BRIGHTER))
        .count()
}

/// How many of a game area's bottom rows the reader looks at when the area is `height` game
/// pixels tall, in game rows.
pub fn rows_needed(height: u32) -> u32 {
    unstretch::rows_for(height, NOTICE_ROWS)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::recognizers::exp_bar::test_support::{Canvas, Font, NEEDED_41, pct_of};

    /// Every character the invented notice font has: the phrases' letters, the test items', and
    /// digits and punctuation.
    const CHARS: &str = "abcdefghijklmnopqrstuvwxyzABEPRSTUXY0123456789(+).,' ";
    pub(crate) const WHITE: [u8; 3] = [255, 255, 255];
    pub(crate) const YELLOW: [u8; 3] = [255, 255, 0];
    /// How much strength a line loses each frame: about what four frames a second see.
    const FADE: u8 = 12;
    /// A monster's body and outline, as a white ghost walking behind the stack.
    const BODY: [u8; 3] = [252, 252, 255];
    const OUTLINE: [u8; 3] = [16, 20, 36];

    /// How wide a [`Scene::Girder`] is: most of a line.
    pub(crate) const GIRDER: u32 = 120;

    /// What shows behind the stack.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Scene {
        /// Brown and textured, like rock.
        Rock,
        /// Near white, like the clouds a white letter vanishes into.
        Cloud,
        /// Rock with a girder across it, its top row at `top` and its left end at `left`: yellow
        /// paint over black, an edge as sharp as a letter's against its shadow. As the camera
        /// follows the player, it slides behind the stack.
        Girder { top: u32, left: u32 },
    }

    impl Scene {
        fn at(self, x: u32, y: u32) -> [u8; 3] {
            let v = ((x * 7 + y * 13) % 29) as u8;
            match self {
                Scene::Rock => [85 + v, 51 + v / 2, 34],
                Scene::Cloud => [226 + v / 2, 232 + v / 3, 240],
                Scene::Girder { top, left }
                    if (top..top + 4).contains(&y) && (left..left + GIRDER).contains(&x) =>
                {
                    match y < top + 2 {
                        true => [220, 170, 40],
                        false => [40, 35, 30],
                    }
                }
                Scene::Girder { .. } => Scene::Rock.at(x, y),
            }
        }
    }

    fn blend(top: [u8; 3], under: [u8; 3], strength: u8) -> [u8; 3] {
        let a = f32::from(strength) / 255.0;
        std::array::from_fn(|i| {
            (f32::from(top[i]) * a + f32::from(under[i]) * (1.0 - a)).round() as u8
        })
    }

    /// A line on screen: its text, its color, and how strong it still is (255 as it arrives).
    struct Shown {
        text: String,
        color: [u8; 3],
        strength: u8,
    }

    /// The notice stack over a scene, drawn the way the game draws it, frame after frame, with
    /// the HUD's EXP beside it.
    pub(crate) struct Game {
        pub(crate) width: u32,
        pub(crate) height: u32,
        pub(crate) scene: Scene,
        font: Font,
        /// Bottom line first.
        lines: Vec<Shown>,
        pub(crate) exp: u64,
        /// An opaque box over the bottom line, as something drawn over it.
        pub(crate) covered: bool,
        /// A monster behind the stack: the top-left corner of a white ghost with a dark outline
        /// and eyes, in the scene, so the lines are drawn over it.
        pub(crate) monster: Option<(u32, u32)>,
        /// Strength a line loses each frame: [`FADE`] at four frames a second.
        pub(crate) fade: u8,
        /// The edge of a monster just inside the stack's right edge, in slot `k`: a little white
        /// and dark at the end of an empty line.
        pub(crate) edge_in_slot: Option<usize>,
    }

    impl Game {
        pub(crate) fn new(scene: Scene) -> Game {
            let mut font = Font::new(CHARS, 11);
            // As in the game, a 1 is narrow and leaves as wide a gap after it as a space: "(+12)"
            // comes cut into "(+1" and "2)".
            if let Some((_, one)) = font.glyphs.iter_mut().find(|(c, _)| *c == '1') {
                *one = vec![0b000_0011, 0b111_1111, 0, 0, 0];
            }
            Game {
                width: 1366,
                height: 768,
                scene,
                font,
                lines: Vec::new(),
                exp: 100_000,
                covered: false,
                monster: None,
                fade: FADE,
                edge_in_slot: None,
            }
        }

        /// The same game with digits drawn the plain way the shape rules read.
        pub(crate) fn plain_digits(mut self) -> Game {
            for (digit, rows) in crate::recognizers::digit_shapes::test_support::PLAIN {
                let columns: Vec<u8> = (0..rows[0].len())
                    .map(|x| {
                        rows.iter()
                            .enumerate()
                            .filter(|(_, row)| row.as_bytes()[x] == b'#')
                            .fold(0, |bits, (y, _)| bits | 1 << y)
                    })
                    .collect();
                if let Some((_, glyph)) = self.font.glyphs.iter_mut().find(|(c, _)| *c == digit) {
                    *glyph = columns;
                }
            }
            self
        }

        /// The same game with digits `a` and `b` drawn each as the other.
        pub(crate) fn swap_digit_shapes(mut self, a: char, b: char) -> Game {
            let shape = |game: &Game, c: char| game.font.columns(c).to_vec();
            let (shape_a, shape_b) = (shape(&self, a), shape(&self, b));
            for (c, glyph) in self.font.glyphs.iter_mut() {
                if *c == a {
                    *glyph = shape_b.clone();
                } else if *c == b {
                    *glyph = shape_a.clone();
                }
            }
            self
        }

        /// The same game after an update that redrew the font's letters, digits and brackets:
        /// each one's rows below the top flipped, so none keeps any old letter's shape.
        pub(crate) fn redrawn(mut self) -> Game {
            for (c, columns) in &mut self.font.glyphs {
                if c.is_alphanumeric() || "(+)".contains(*c) {
                    for column in columns {
                        *column ^= 0b111_1110;
                    }
                }
            }
            self
        }

        /// The next frame: the lines on screen fade, `arrive` (oldest first) enter at the
        /// bottom, and the HUD gains `gained`.
        pub(crate) fn next(&mut self, arrive: &[(&str, [u8; 3])], gained: u64) -> Canvas {
            for line in &mut self.lines {
                line.strength = line.strength.saturating_sub(self.fade);
            }
            self.lines.retain(|l| l.strength > 60);
            for (text, color) in arrive {
                self.lines.insert(
                    0,
                    Shown {
                        text: text.to_string(),
                        color: *color,
                        strength: 255,
                    },
                );
            }
            self.lines.truncate(SLOTS);
            self.exp += gained;
            self.draw()
        }

        pub(crate) fn hud(&self) -> ExpReading {
            ExpReading {
                level: Some(41),
                exp: self.exp,
                pct_hundredths: pct_of(self.exp, NEEDED_41),
                needed: Some(NEEDED_41),
                confirmed: true,
            }
        }

        pub(crate) fn draw(&self) -> Canvas {
            let (w, h) = (self.width, self.height);
            let mut canvas = Canvas::new(w, h);
            for y in h - NOTICE_ROWS - 4..h {
                for x in 0..w {
                    canvas.set(x, y, self.scene.at(x, y));
                }
            }
            if let Some((mx, my)) = self.monster {
                canvas.fill(mx, my, 56, 30, OUTLINE);
                canvas.fill(mx + 1, my + 1, 54, 28, BODY);
                canvas.fill(mx + 14, my + 8, 5, 8, OUTLINE);
                canvas.fill(mx + 34, my + 8, 5, 8, OUTLINE);
                // Its ragged cloak: dark specks all over.
                for dy in (3..27).step_by(4) {
                    for dx in (3..52).step_by(6) {
                        canvas.fill(mx + dx + dy % 3, my + dy, 2, 1, OUTLINE);
                    }
                }
            }
            if let Some(k) = self.edge_in_slot {
                let top = h - BOTTOM_LINE_UP - LINE_PITCH * k as u32;
                canvas.fill(w - RIGHT_MARGIN - 12, top, 13, LINE_ROWS, OUTLINE);
                canvas.fill(w - RIGHT_MARGIN - 11, top + 1, 11, LINE_ROWS - 3, BODY);
                canvas.fill(w - RIGHT_MARGIN - 8, top + 3, 2, 2, OUTLINE);
                canvas.fill(w - RIGHT_MARGIN - 4, top + 3, 2, 2, OUTLINE);
            }
            for (k, line) in self.lines.iter().enumerate() {
                let top = h - BOTTOM_LINE_UP - LINE_PITCH * k as u32;
                let columns: Vec<&[u8]> = line.text.chars().map(|c| self.font.columns(c)).collect();
                let width: u32 = columns.iter().map(|c| c.len() as u32 + 1).sum::<u32>() - 1;
                let mut x = w - RIGHT_MARGIN + 1 - width;
                let mut core = std::collections::HashSet::new();
                for glyph in columns {
                    for &bits in glyph {
                        for row in 0..7 {
                            if bits >> row & 1 == 1 {
                                core.insert((x, top + 1 + row));
                            }
                        }
                        x += 1;
                    }
                    x += 1;
                }
                // The shadow, one pixel down and to the right, under the letters.
                for &(x, y) in &core {
                    if !core.contains(&(x + 1, y + 1)) {
                        let under = canvas.get(x + 1, y + 1);
                        canvas.set(x + 1, y + 1, blend([0, 0, 0], under, line.strength));
                    }
                }
                for &(x, y) in &core {
                    let under = canvas.get(x, y);
                    canvas.set(x, y, blend(line.color, under, line.strength));
                }
            }
            if self.covered {
                let top = h - BOTTOM_LINE_UP - 2;
                canvas.fill(w - 300, top, 300, LINE_ROWS + 4, [60, 60, 70]);
            }
            canvas
        }
    }

    /// "You received EXP (+N)".
    pub(crate) fn exp(amount: &str) -> String {
        format!("You received EXP (+{amount})")
    }

    pub(crate) fn mesos(amount: &str) -> String {
        format!("You have gained mesos (+{amount})")
    }

    pub(crate) fn party(amount: &str) -> String {
        format!("Bonus EXP for PARTY (+{amount})")
    }

    pub(crate) fn item(name: &str, count: u32, tab: &str) -> String {
        format!("{name} x{count} earned. ({tab})")
    }

    /// A frame: the lines that arrive in it, oldest first, and the EXP the HUD gains.
    pub(crate) type Step = (Vec<(String, [u8; 3])>, u64);

    /// One line arriving, with the HUD's gain.
    pub(crate) fn one(text: &str, gained: u64) -> Step {
        (vec![(text.to_string(), WHITE)], gained)
    }

    /// Plays frames into the reader.
    pub(crate) fn run(
        game: &mut Game,
        reader: &mut NoticeReader,
        steps: Vec<Step>,
    ) -> Vec<NoticeReading> {
        steps
            .into_iter()
            .map(|(arrive, gained)| {
                let arrive: Vec<(&str, [u8; 3])> =
                    arrive.iter().map(|(t, c)| (t.as_str(), *c)).collect();
                let canvas = game.next(&arrive, gained);
                reader.observe(&canvas.frame(), Some(&game.hud())).unwrap()
            })
            .collect()
    }

    /// EXP lines one at a time, each worth what the HUD gains, a few quiet frames between: they
    /// name every digit, then three in a row check out.
    pub(crate) fn learning(amounts: &[u64]) -> Vec<Step> {
        let mut frames = vec![(Vec::new(), 0)];
        for &n in amounts {
            frames.push(one(&exp(&group_digits(&n.to_string())), n));
            frames.push((Vec::new(), 0));
        }
        frames
    }

    pub(crate) const EVERY_DIGIT: [u64; 8] = [21, 43, 60, 57, 89, 21, 43, 60];

    /// A reader that learned every digit from `game`'s EXP lines and confirmed them.
    pub(crate) fn confirmed_reader(game: &mut Game) -> NoticeReader {
        let mut reader = NoticeReader::new();
        run(game, &mut reader, learning(&EVERY_DIGIT));
        assert!(
            reader.confirmed(),
            "three EXP amounts in a row matched the HUD"
        );
        reader
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    fn notices(readings: &[NoticeReading]) -> Vec<Notice> {
        readings.iter().flat_map(|r| r.notices.clone()).collect()
    }

    // Guards self-calibration: EXP lines name the digits only as the HUD's gain says, nothing
    // is confirmed until three amounts in a row match, and then mesos, items and party bonus
    // lines read. Fails if a reading shows before it's checked, or a kind of line misreads.
    #[test]
    fn learns_from_exp_lines_and_the_hud_then_reads_every_kind_of_line() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = NoticeReader::new();
        let readings = run(&mut game, &mut reader, learning(&EVERY_DIGIT));
        let first_confirmed = readings.iter().position(|r| r.confirmed).unwrap();
        assert!(
            readings[..first_confirmed].iter().all(|r| !r.confirmed),
            "nothing confirmed while learning"
        );
        assert_eq!(notices(&readings[first_confirmed..]), [Notice::Exp(60)]);

        let readings = run(
            &mut game,
            &mut reader,
            vec![
                one(&mesos("75"), 0),
                (Vec::new(), 0),
                one(&item("Red Potion", 1, "Use"), 0),
                (Vec::new(), 0),
                (vec![(exp("17"), WHITE), (party("5"), YELLOW)], 22),
                (Vec::new(), 0),
                (Vec::new(), 0),
            ],
        );
        assert!(readings.iter().all(|r| r.confirmed && r.missed.is_none()));
        let read = notices(&readings);
        assert_eq!(read[0], Notice::Mesos(75));
        let Notice::Item { name, count, tab } = &read[1] else {
            panic!("an item: {read:?}");
        };
        // R and t are in no phrase seen yet; the Use tab is told by its s and e.
        assert_eq!(
            (name.text.as_str(), *count, *tab),
            ("?ed Po?ion", 1, Some(Tab::Use))
        );
        assert_eq!(read[2..], [Notice::Exp(17), Notice::PartyBonus(5)]);
    }

    // Guards counting by movement: the same pickup twice, in two frames or in one, is two
    // pickups. Fails if identical lines are taken for one line seen again.
    #[test]
    fn the_same_pickup_twice_counts_twice() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        let twelve = mesos("12");
        let readings = run(
            &mut game,
            &mut reader,
            vec![
                one(&twelve, 0),
                one(&twelve, 0),
                (vec![(twelve.clone(), WHITE), (twelve.clone(), WHITE)], 0),
                (Vec::new(), 0),
            ],
        );
        assert_eq!(notices(&readings), vec![Notice::Mesos(12); 4]);
        assert!(readings.iter().all(|r| r.missed.is_none()));
    }

    // Guards reading over a scene as white as the letters: the shadow tells them apart. Fails if
    // the cloud is taken for letters.
    #[test]
    fn reads_over_white_clouds() {
        let mut game = Game::new(Scene::Cloud);
        let mut reader = confirmed_reader(&mut game);
        let readings = run(
            &mut game,
            &mut reader,
            vec![one(&mesos("98"), 0), (Vec::new(), 0)],
        );
        assert_eq!(notices(&readings), [Notice::Mesos(98)]);
    }

    // Guards item identity: the same name gives the same key, another name another key, and the
    // text has the letters the phrases taught with `?` for the rest. Fails if a key depends on
    // more than the name's shape.
    #[test]
    fn an_item_is_known_by_its_name_s_shape() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        let readings = run(
            &mut game,
            &mut reader,
            vec![
                one(&item("Snail Shell", 1, "Etc"), 0),
                (Vec::new(), 0),
                one(&item("Red Potion", 3, "Use"), 0),
                one(&item("Snail Shell", 2, "Etc"), 0),
            ],
        );
        let items: Vec<(String, String, u32)> = notices(&readings)
            .into_iter()
            .map(|n| match n {
                Notice::Item { name, count, .. } => (name.key, name.text, count),
                other => panic!("not an item: {other:?}"),
            })
            .collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].0, items[2].0, "the same name, the same key");
        assert_ne!(items[0].0, items[1].0);
        // S, h and l are in no phrase the reader has seen.
        assert_eq!(items[0].1, "?nai? ??e??");
        assert_eq!((items[0].2, items[1].2, items[2].2), (1, 3, 2));
        let Notice::Item { name, .. } = &notices(&readings)[0] else {
            unreachable!()
        };
        assert_eq!(name.picture.height, LINE_ROWS);
        assert!(name.picture.coverage.contains(&255));
    }

    // Guards the honesty rules: a new line that can't be seen, a digit never learned, and EXP
    // lines short of the HUD all say lines were missed, and none is guessed. Fails if a missed
    // pickup passes silently.
    #[test]
    fn lines_that_cant_be_read_are_reported_missed() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = NoticeReader::new();
        // Learned from amounts without a 5, 7, 8 or 9.
        run(&mut game, &mut reader, learning(&[21, 43, 60, 21, 43, 60]));
        assert!(reader.confirmed());

        let unknown = run(&mut game, &mut reader, vec![one(&mesos("75"), 0)]);
        assert_eq!(unknown[0].missed, Some(Missed::NotLearned));
        assert!(unknown[0].notices.is_empty());

        game.covered = true;
        let covered = run(&mut game, &mut reader, vec![one(&mesos("12"), 0)]);
        game.covered = false;
        assert!(covered[0].notices.is_empty());
        assert!(covered[0].missed.is_some());

        let short = run(
            &mut game,
            &mut reader,
            vec![
                (Vec::new(), 0),
                (Vec::new(), 0),
                one(&exp("21"), 64),
                (Vec::new(), 0),
                (Vec::new(), 0),
                (Vec::new(), 0),
            ],
        );
        assert_eq!(notices(&short), [Notice::Exp(21)]);
        assert!(short.iter().any(|r| r.missed == Some(Missed::ExpShort)));
    }

    // A mesos line with a digit not learned yet carries no EXP, so it mustn't stop EXP lines
    // from teaching. Fails if the 9 is never learned because the mesos line beside it blocked.
    #[test]
    fn a_mesos_line_it_cant_read_doesnt_stop_exp_lines_teaching() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = NoticeReader::new();
        run(&mut game, &mut reader, learning(&[21, 43, 60, 21, 43, 60]));
        assert!(reader.confirmed());
        let readings = run(
            &mut game,
            &mut reader,
            vec![
                (vec![(mesos("57"), WHITE), (exp("29"), WHITE)], 29),
                (Vec::new(), 0),
                one(&mesos("99"), 0),
            ],
        );
        assert_eq!(readings[0].missed, Some(Missed::NotLearned));
        assert_eq!(notices(&readings), [Notice::Mesos(99)], "the 9 was learned");
    }

    // Guards keeping letters between launches: the next session reads its first pickup, and
    // saves back the same text. Fails if kept letters read unconfirmed or don't round-trip.
    #[test]
    fn what_it_confirmed_is_kept_and_read_at_once_next_session() {
        let mut game = Game::new(Scene::Rock);
        assert_eq!(
            NoticeReader::new().knowledge(),
            None,
            "nothing to keep before confirming"
        );
        let kept = confirmed_reader(&mut game).knowledge().unwrap();
        let mut next = NoticeReader::from_knowledge(&kept).unwrap();
        assert!(next.confirmed());
        assert_eq!(next.knowledge(), Some(kept.clone()), "saved back the same");
        // A file that picked up Windows line endings reads the same.
        assert!(NoticeReader::from_knowledge(&kept.replace('\n', "\r\n")).is_some());
        let mut game = Game::new(Scene::Cloud);
        let readings = run(
            &mut game,
            &mut next,
            vec![(Vec::new(), 0), one(&mesos("85"), 0)],
        );
        assert_eq!(notices(&readings), [Notice::Mesos(85)]);
        assert!(readings.iter().all(|r| r.confirmed && r.missed.is_none()));
        // The mesos line taught its phrase's letters, and what was kept is still there.
        let grown = next.knowledge().unwrap();
        assert!(grown.len() > kept.len());
        assert!(kept.lines().all(|line| grown.lines().any(|l| l == line)));
    }

    #[test]
    fn kept_letters_must_be_text_a_reader_wrote() {
        let kept = confirmed_reader(&mut Game::new(Scene::Rock))
            .knowledge()
            .unwrap();
        let first = kept.lines().nth(1).unwrap();
        let (name, glyph) = first.split_once(' ').unwrap();
        for bad in [
            String::new(),
            "hello".to_string(),
            KNOWLEDGE_HEADER.to_string(),
            kept.replace("notices 1", "notices 2"),
            // A letter no phrase has.
            format!("{kept}5a {glyph}\n"),
            // A second shape named the same.
            format!("{kept}{name} 1:1\n"),
            format!("{kept}junk\n"),
        ] {
            assert!(NoticeReader::from_knowledge(&bad).is_none(), "{bad:?}");
        }
    }

    // After a game update that redrew the font, kept letters must never read wrong, and the
    // reader must learn the new ones. Fails if it stays unreadable for good or shows a wrong
    // amount.
    #[test]
    fn kept_letters_that_no_longer_fit_are_dropped_and_learned_again() {
        let kept = confirmed_reader(&mut Game::new(Scene::Rock))
            .knowledge()
            .unwrap();
        let mut reader = NoticeReader::from_knowledge(&kept).unwrap();
        let mut game = Game::new(Scene::Rock).redrawn();
        let amounts: Vec<u64> = EVERY_DIGIT.iter().chain(&EVERY_DIGIT).copied().collect();
        let readings = run(&mut game, &mut reader, learning(&amounts));
        for r in readings.iter().filter(|r| r.confirmed) {
            for n in &r.notices {
                assert!(matches!(n, Notice::Exp(v) if amounts.contains(v)), "{n:?}");
            }
        }
        assert!(reader.confirmed(), "learned the redrawn letters");
        let relearned = reader.knowledge().unwrap();
        assert_ne!(relearned, kept);
        assert_eq!(
            notices(&run(&mut game, &mut reader, vec![one(&mesos("70"), 0)])),
            [Notice::Mesos(70)]
        );
    }

    /// Where a monster stands over the bottom line's amount: the right end of the stack.
    fn over_the_amount(game: &Game) -> (u32, u32) {
        (game.width - 60, game.height - BOTTOM_LINE_UP - 8)
    }

    // A monster walking behind a line as it arrives spoils its letters for a frame or two. Fails
    // if the line is counted wrong, or not read once the monster has moved on while it's still
    // bright, at four frames a second.
    #[test]
    fn a_line_spoiled_by_a_monster_behind_it_is_read_once_it_moves_on() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        game.monster = Some(over_the_amount(&game));
        let spoiled = run(&mut game, &mut reader, vec![one(&mesos("75"), 0)]);
        game.monster = None;
        let after = run(&mut game, &mut reader, vec![(Vec::new(), 0)]);
        assert!(spoiled[0].notices.is_empty(), "spoiled: {:?}", spoiled[0]);
        assert_eq!(spoiled[0].missed, None, "waits for the next frame");
        assert_eq!(after[0].notices, [Notice::Mesos(75)]);
        assert_eq!(after[0].missed, None);
    }

    // A monster behind an EXP line's amount as it arrives spoils its digits. Fails if the kill is
    // lost or counted twice once the monster moves on.
    #[test]
    fn an_amount_spoiled_by_a_monster_is_read_again() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        game.monster = Some((game.width - 74, game.height - BOTTOM_LINE_UP - 8));
        let spoiled = run(&mut game, &mut reader, vec![one(&exp("43"), 43)]);
        game.monster = None;
        let after = run(&mut game, &mut reader, vec![(Vec::new(), 0); 4]);
        let all: Vec<NoticeReading> = spoiled.into_iter().chain(after).collect();
        assert_eq!(notices(&all), [Notice::Exp(43)]);
        assert!(all.iter().all(|r| r.missed.is_none()), "{all:?}");
    }

    // At one frame a second a line has faded too far to read by the next frame: waiting would only
    // hold back the EXP checks. Fails if a line that can't be read isn't reported missed at once.
    #[test]
    fn at_one_frame_a_second_a_spoiled_line_is_missed_at_once() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        // A few lines fading at one frame a second, for the reader to measure the pace.
        game.fade = 45;
        let pace = vec![one(&mesos("12"), 0), (Vec::new(), 0), (Vec::new(), 0)];
        run(
            &mut game,
            &mut reader,
            [pace.clone(), pace.clone(), pace].concat(),
        );
        game.monster = Some(over_the_amount(&game));
        let spoiled = run(&mut game, &mut reader, vec![one(&mesos("75"), 0)]);
        assert!(spoiled[0].notices.is_empty());
        assert_eq!(spoiled[0].missed, Some(Missed::Unreadable));
    }

    // A monster walking behind lines already read changes their shapes frame by frame. Fails if
    // they're taken for new lines and counted again, or the stack is lost.
    #[test]
    fn a_monster_walking_behind_the_stack_counts_nothing_twice() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        let read = run(
            &mut game,
            &mut reader,
            vec![one(&mesos("98"), 0), one(&item("Red Potion", 1, "Use"), 0)],
        );
        assert_eq!(notices(&read).len(), 2);
        let (x, y) = over_the_amount(&game);
        let mut walking = Vec::new();
        for step in 0..6 {
            game.monster = Some((x - 12 * step, y - 6));
            walking.extend(run(&mut game, &mut reader, vec![(Vec::new(), 0)]));
        }
        game.monster = None;
        walking.extend(run(&mut game, &mut reader, vec![one(&mesos("57"), 0)]));
        assert_eq!(notices(&walking), [Notice::Mesos(57)]);
        assert!(walking.iter().all(|r| r.missed.is_none()), "{walking:?}");
    }

    // The camera following the player slides sharp-edged scenery, like a painted girder, behind
    // lines that stay put, changing their shapes from frame to frame. Fails if a line is taken for
    // a new one and counted again, or the stack is lost.
    #[test]
    fn scenery_sliding_behind_the_lines_counts_nothing_twice() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        let read = run(
            &mut game,
            &mut reader,
            vec![one(&mesos("98"), 0), one(&item("Red Potion", 1, "Use"), 0)],
        );
        assert_eq!(notices(&read).len(), 2);
        // Across the bottom line, from below its first row of letters to its last.
        let bottom = game.height - BOTTOM_LINE_UP;
        let mut sliding = Vec::new();
        for step in 0..5 {
            game.scene = Scene::Girder {
                top: bottom + step,
                left: game.width - RIGHT_MARGIN - GIRDER - 10 * step,
            };
            sliding.extend(run(&mut game, &mut reader, vec![(Vec::new(), 0)]));
        }
        game.scene = Scene::Rock;
        sliding.extend(run(
            &mut game,
            &mut reader,
            vec![(Vec::new(), 0), one(&mesos("57"), 0)],
        ));
        assert_eq!(notices(&sliding), [Notice::Mesos(57)], "{sliding:?}");
        assert!(sliding.iter().all(|r| r.missed.is_none()), "{sliding:?}");
    }

    // The edge of a monster stepping into an empty slot above the lines isn't a line: nothing new
    // arrived, so nothing is counted again. Fails if the lines below are taken for new ones and
    // counted twice.
    #[test]
    fn a_monster_s_edge_above_the_lines_counts_nothing_again() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        // The lines learned from fade away first, so the slots above are empty.
        run(&mut game, &mut reader, vec![(Vec::new(), 0); 20]);
        let first = run(&mut game, &mut reader, vec![one(&exp("43"), 43)]);
        game.edge_in_slot = Some(3);
        let later = run(&mut game, &mut reader, vec![(Vec::new(), 0); 4]);
        assert_eq!(notices(&first), [Notice::Exp(43)]);
        assert_eq!(notices(&later), []);
        assert!(later.iter().all(|r| r.missed.is_none()), "{later:?}");
    }

    /// A reader that learned only the digits 0, 1, 2, 3, 4 and 6 from EXP lines, with the lines
    /// it learned from faded away.
    fn missing_five_seven_eight_nine(game: &mut Game) -> NoticeReader {
        let mut reader = NoticeReader::new();
        run(game, &mut reader, learning(&[21, 43, 60, 21, 43, 60]));
        run(game, &mut reader, vec![(Vec::new(), 0); 20]);
        assert!(reader.confirmed());
        reader
    }

    // A mesos amount with a digit not learned yet is kept, and counted as soon as an EXP line
    // teaches the digit. Fails if it's lost, or counted before its digits are known.
    #[test]
    fn a_mesos_amount_is_counted_once_its_digits_are_learned() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = missing_five_seven_eight_nine(&mut game);
        let unread = run(
            &mut game,
            &mut reader,
            vec![one(&mesos("75"), 0), (Vec::new(), 0)],
        );
        assert_eq!(notices(&unread), []);
        assert_eq!(reader.waiting_mesos(), 1);
        let taught = run(
            &mut game,
            &mut reader,
            vec![one(&exp("57"), 57), (Vec::new(), 0), (Vec::new(), 0)],
        );
        // The EXP line taught the 5 and 7; it isn't reported itself, its digits unknown when read.
        assert_eq!(notices(&taught), [Notice::LateMesos(75)]);
        assert_eq!(reader.waiting_mesos(), 0);
    }

    // Digits no EXP line showed are named by their shapes once the shape rules name every digit
    // the EXP lines taught, and the mesos amounts waiting on them are counted. Fails if a digit
    // isn't named, an amount is lost, or the names aren't kept for the next session.
    #[test]
    fn digits_no_exp_line_showed_are_named_by_their_shapes() {
        let mut game = Game::new(Scene::Rock).plain_digits();
        let mut reader = missing_five_seven_eight_nine(&mut game);
        let mut steps = Vec::new();
        for amount in ["75", "57", "85", "58", "78"] {
            steps.push(one(&mesos(amount), 0));
            steps.push((Vec::new(), 0));
        }
        let read = run(&mut game, &mut reader, steps);
        assert_eq!(
            notices(&read),
            [
                Notice::LateMesos(75),
                Notice::LateMesos(57),
                Notice::LateMesos(85),
                Notice::LateMesos(58),
                Notice::Mesos(78)
            ]
        );
        assert_eq!(reader.waiting_mesos(), 0);
        let kept = reader.knowledge().unwrap();
        assert_eq!(kept.lines().filter(|l| l.ends_with(" shape")).count(), 3);
        let next = NoticeReader::from_knowledge(&kept).unwrap();
        assert_eq!(next.knowledge(), Some(kept));
    }

    // In a font whose digits the shape rules misread, checked on the digits the EXP lines taught,
    // no digit is named by its shape: amounts wait for EXP lines instead. Fails if one is.
    #[test]
    fn no_digit_is_named_by_shape_in_a_font_the_rules_misread() {
        // Plain digits, but the 1 drawn as a 2 and the 2 as a 1: the rules misread both, which
        // the EXP lines taught, so they're trusted for no other digit.
        let mut game = Game::new(Scene::Rock)
            .plain_digits()
            .swap_digit_shapes('1', '2');
        let mut reader = missing_five_seven_eight_nine(&mut game);
        let mut steps = Vec::new();
        for amount in ["75", "57", "75"] {
            steps.push(one(&mesos(amount), 0));
            steps.push((Vec::new(), 0));
        }
        let read = run(&mut game, &mut reader, steps);
        assert_eq!(notices(&read), []);
        assert_eq!(reader.waiting_mesos(), 3);
    }

    // A digit named by its shape that's wrong shows when an EXP line using it disagrees with the
    // HUD: every digit named that way is dropped, and no more are. Fails if a wrong name stays.
    #[test]
    fn a_digit_named_wrong_by_its_shape_is_dropped_when_an_exp_line_disagrees() {
        // A font whose 5 looks like a 7 and 7 like a 5: the rules name them the wrong way round.
        let mut game = Game::new(Scene::Rock)
            .plain_digits()
            .swap_digit_shapes('5', '7');
        let mut reader = missing_five_seven_eight_nine(&mut game);
        let mut steps = Vec::new();
        for amount in ["75", "57"] {
            steps.push(one(&mesos(amount), 0));
            steps.push((Vec::new(), 0));
        }
        let wrong = run(&mut game, &mut reader, steps);
        assert_eq!(
            notices(&wrong),
            [Notice::LateMesos(57), Notice::LateMesos(75)],
            "read the wrong way round"
        );
        let checked = run(
            &mut game,
            &mut reader,
            [learning(&[57]), vec![(Vec::new(), 0); 3]].concat(),
        );
        assert!(checked.iter().any(|r| r.missed == Some(Missed::ExpOver)));
        let after = run(&mut game, &mut reader, vec![one(&mesos("75"), 0)]);
        assert_eq!(after[0].missed, Some(Missed::NotLearned));
        assert!(reader.knowledge().unwrap().contains("no-shapes"));
    }

    // Guards amounts over a thousand: the HUD's gain names the comma too.
    #[test]
    fn amounts_with_commas() {
        assert_eq!(parse_number("1,234"), Some(1234));
        assert_eq!(parse_number("1234"), Some(1234));
        assert_eq!(parse_number("12,34"), None);
        assert_eq!(parse_number(",123"), None);
        let mut game = Game::new(Scene::Rock);
        let mut reader = NoticeReader::new();
        let readings = run(
            &mut game,
            &mut reader,
            learning(&[1_234, 5_678, 9_012, 1_234, 5_678, 9_012]),
        );
        assert!(reader.confirmed());
        assert_eq!(notices(&readings).last(), Some(&Notice::Exp(9_012)));
    }

    // Guards the stretched sizes: the stack's letters come back from the picture the game
    // stretched exactly as at 1x, so what was learned at one size reads another.
    #[test]
    fn reads_a_frame_the_game_stretched() {
        let mut game = Game::new(Scene::Rock);
        let mut reader = confirmed_reader(&mut game);
        let canvas = game.next(&[(&mesos("39"), WHITE)], 0);
        let rgb: Vec<[u8; 3]> = canvas
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[2], p[1], p[0]])
            .collect();
        let (width, height) = (1920, 1080);
        let from = height - (NOTICE_ROWS + 8) * height / 768;
        let bgra = unstretch::stretch(&rgb, canvas.width, width, height, from);
        let frame = Frame::new(width, height, width as usize * 4, &bgra).unwrap();
        let reading = reader.observe(&frame, Some(&game.hud())).unwrap();
        assert_eq!(reading.notices, [Notice::Mesos(39)]);
        assert!(reading.missed.is_none());
    }

    // Guards the edit distance that lets a touching pair or a speck cost a letter, not a line.
    #[test]
    fn phrases_are_read_with_a_letter_or_two_unknown() {
        let read = |text: &str| -> Vec<Option<char>> {
            text.chars().map(|c| (c != '?').then_some(c)).collect()
        };
        assert_eq!(distance(&read("YoureceivedEXP"), EXP_TEXT), 0);
        assert_eq!(distance(&read("You?eceivedEXP"), EXP_TEXT), 1);
        assert_eq!(distance(&read("Youhave?inedmesos"), MESOS_TEXT), 2);
        assert!(distance(&read("Youhavegainedmesos"), EXP_TEXT) > PHRASE_ERRORS);
    }
}
