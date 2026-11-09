//! Chat reader (Chat Hub v1): whisper, party and friend lines from the chat box, as pictures.
//!
//! The Chat Hub shows these lines as pictures first, and reading their text comes later. At the
//! game's own pixel size each chat type is drawn in one exact color (a Windows
//! capture at 1920x1080, recovered to 1x): whispers (0, 255, 0), party chat (255, 153, 204)
//! and friend chat (255, 153, 0). Normal chat is white, system notices are
//! yellow, and a megaphone fills its whole line with a colored band.
//!
//! The chat box sits on the HUD, which is centered at the bottom: its lines are 16 px apart with
//! the newest at the bottom, just above the input bar, and a scrollbar runs down its right edge.
//! New lines always enter at the bottom, so the reader looks only at the default box's eight
//! lines, even when the player made the box taller.
//!
//! Party and friend chat comes as "name : words". The game also writes some notices of its own in
//! those colors (a party member left, a pet out of potions); they have no name and colon, so they
//! stay out of the hub, along with the indented rest of them.
//!
//! New lines are counted by how far the box's lines moved up since the last frame
//! ([`crate::lines`], shared with the pickup notices), so a friend saying "ok" twice shows twice. When the lines can't be
//! lined up (the box was scrolled back, covered, or just came into view), every colored line on
//! screen counts as new unless it was handed back recently, and the reading says lines may be
//! missing. While the player has scrolled back through the
//! chat, the box shows old lines and nothing is read.
//!
//! The game draws its own mouse pointer into the picture. Where it is over the lines, found by
//! its highlight, is left out when lines are compared, so a line looks the same with the pointer
//! over it or not.
//!
//! The game leaves the bottom of the letters (the tails of g, p and y) off its lowest line, and
//! draws them once the line moves up. Lines are compared without those rows. A line is handed back
//! as soon as it shows; if it showed cut off, or with the pointer over it, it's handed back again,
//! whole, under the same id, from the first frame where it has moved up and the pointer isn't over
//! it.
//!
//! Chat pictures never become [`crate::events::Event`]s: plugins don't see them, and nothing
//! leaves the device.

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::frame::Frame;
use crate::lines::{self, StackLine};
use crate::unstretch::{self, No1x};

/// The HUD bar's width at 1x.
const HUD_WIDTH: u32 = 800;
/// How far, in game pixels each way, the HUD may sit from where the game area's size puts it.
/// Wine on a Mac places the game up to a point off.
const MAX_SHIFT: u32 = 1;
/// Where the HUD sits relative to where the game area's size puts it, in game pixels: (right,
/// down).
type Offset = (i32, i32);
/// How far apart two frames may read the same line, in pixels each way: each frame may find the
/// HUD up to [`MAX_SHIFT`] from where the game area's size puts it, so not always at the same place.
/// Reading a line two places as two lines would hand every line on screen back again.
const DRIFT: i32 = 2 * MAX_SHIFT as i32;
/// Every place the HUD may be, nearest first.
const OFFSETS: [Offset; 9] = [
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
/// The columns chat text can use, from the HUD's left edge (the scrollbar starts at 551).
const TEXT_LEFT: u32 = 4;
const TEXT_RIGHT: u32 = 550;
/// How wide chat text can run, at 1x.
const TEXT_WIDTH: u32 = TEXT_RIGHT - TEXT_LEFT;
/// The scrollbar's left edge, a light gray column from its bottom arrow to its top arrow.
const SCROLLBAR_EDGE: u32 = 551;
/// How wide the box's dark backing is, at 1x: from the HUD's left edge up to the scrollbar.
pub const BOX_WIDTH: u32 = SCROLLBAR_EDGE;
/// A column through the scrollbar's thumb, and a row just above the bottom arrow: the thumb is
/// there while the box shows the newest lines.
const THUMB: (u32, u32) = (556, 88);
/// The input bar's top edge: a white row over a near-black one, from the HUD's left edge.
const INPUT_TOP_UP: u32 = 67;
/// The lowest line's top row, counted up from the bottom, and the distance between lines. A line
/// runs from just above its capitals to the bottom of its descenders.
const FIRST_LINE_UP: u32 = 85;
pub const LINE_HEIGHT: u32 = 16;
/// A line's rows the game always draws. On its lowest line it leaves off the rest, the bottom of
/// letters such as g and p, and draws them once the line moves up: the same line, captured a
/// second apart, has nothing in its last three rows at the bottom of the box and its tails one
/// line up.
const DRAWN_ROWS: u32 = 13;
/// The lines read: the default box's.
const MAX_LINES: u32 = 8;
/// In the default box, the top of the scrollbar's top arrow is this far above the top line's top
/// row. A resized box can leave less: 7 rows in a 5-line box (Mac captures, 2026-11-02).
const ARROW_ABOVE_LINES: u32 = 10;
/// Rows read above the scrollbar's top arrow, and with room for the HUD to sit a row higher.
const ABOVE_BOX: u32 = 4 + MAX_SHIFT;
/// 1x rows the default box reaches, up from where the game area's size puts the HUD's bottom: the
/// lines, the scrollbar's top arrow, and a few rows above it.
const BOX_ROWS: u32 = FIRST_LINE_UP + LINE_HEIGHT * (MAX_LINES - 1) + ARROW_ABOVE_LINES + 4;
/// 1x rows the reader looks at first: the default box's, with room for the HUD to sit a row
/// higher. The player can make the box taller; the reader then follows its top up (ChatReader).
pub const CHAT_ROWS: u32 = BOX_ROWS + MAX_SHIFT;
/// How many more rows the reader asks for when a box runs past the top of the rows it read.
const GROW: u32 = LINE_HEIGHT * MAX_LINES;
/// The scrollbar's top arrow: a pale face around a blue arrow, `ARROW_ROWS` tall from the top of
/// the scrollbar's edge down, under a blue-gray border row. Column `ARROW_FACE.0` crosses both.
const ARROW_ROWS: u32 = 12;
const ARROW_FACE: (u32, [u8; 3]) = (556, [238, 238, 255]);
const ARROW_BORDER: [u8; 3] = [136, 154, 170];
/// The columns of the arrow itself.
const ARROW_COLUMNS: std::ops::RangeInclusive<u32> = 556..=561;
/// A line whose text starts this far in is the indented rest of a long line above it.
const INDENT: u32 = 16;
/// How far into a line a speaker's " : " can be, from [`TEXT_LEFT`]: past the game's longest names.
const NAME_COLUMNS: usize = 120;
/// Lines handed back recently, for telling old lines from new ones after the box was scrolled.
const RECENT: usize = 64;
/// The game draws its own mouse pointer into the picture: a white glove pointing up and to the
/// left, about 24 columns by 28 rows at 1x, edged in black and shaded blue-gray. Over a line it
/// adds and hides up to 140 of the line's text pixels (a Windows recording, 2026-11-06), more
/// than a line may change and still be the same line. Its pale cyan highlight is near no chat
/// color, so it shows where the pointer is.
const POINTER_HIGHLIGHT: [u8; 3] = [221, 255, 255];
/// How far the glove reaches past its highlight: its black edge, and its shaded palm to the right.
const POINTER_REACH: u32 = 8;
/// No highlight wider or taller than this is the pointer's.
const POINTER_SPAN: u32 = 32;

/// The next line id, shared by every reader, so no id is used twice while MesoVibe runs.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The chat types the Chat Hub splits out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Whisper,
    Party,
    Friend,
}

impl Channel {
    const ALL: [Channel; 3] = [Channel::Whisper, Channel::Party, Channel::Friend];

    /// The text color at 1x.
    pub fn color(self) -> [u8; 3] {
        match self {
            Channel::Whisper => [0, 255, 0],
            Channel::Party => [255, 153, 204],
            Channel::Friend => [255, 153, 0],
        }
    }
}

/// One new chat line, as a picture of its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    /// The line's own id, set when it's handed back; the same when it's handed back again, whole.
    pub id: u64,
    pub channel: Channel,
    /// The indented rest of a long line, which arrived with the line before it.
    pub continues: bool,
    pub width: u32,
    pub height: u32,
    /// How much of each pixel is the channel's text color, row by row: 255 is solid text, 0 is
    /// none. The scene behind the chat box is left out.
    pub coverage: Vec<u8>,
}

/// What one frame of chat brought.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatReading {
    /// New whisper, party and friend lines, oldest first.
    pub lines: Vec<ChatLine>,
    /// Lines handed back earlier with part of their text missing, now whole: they were on the
    /// box's lowest line, where the game leaves off the bottom of the letters, or under the
    /// pointer. Each keeps its id.
    pub whole: Vec<ChatLine>,
    /// The box couldn't be lined up with the last frame, so lines may be missing.
    pub gap: bool,
    /// Where the box is, for showing chat just above it.
    pub place: Option<ChatPlace>,
}

/// Where the chat box is in the game area's 1x picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatPlace {
    /// The 1x picture's size, which maps it onto the game area on screen.
    pub width: u32,
    pub height: u32,
    /// The box's left edge.
    pub left: u32,
    /// The box's top row, where its dark backing starts: the border just above the scrollbar's
    /// top arrow.
    pub top: u32,
}

/// Why the chat box couldn't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatUnreadable {
    /// No chat box where the HUD puts it: hidden, covered, or not the default UI.
    NotFound,
    /// The player scrolled back through the chat, so the box shows old lines.
    ScrolledBack,
    /// The frame has no 1x picture to read.
    Unsupported(No1x),
}

/// What a line on screen is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Blank,
    /// A megaphone's colored band, and its color. The bands are see-through, so their pixels
    /// change with the scene behind them; their color doesn't.
    Band([u8; 3]),
    /// Text, in a channel's color or not (normal chat, notices).
    Text(Option<Channel>),
}

/// One line of the box as seen in a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    kind: Kind,
    /// Which pixels are solid text, bit `row * TEXT_WIDTH + column`: the line's shape, which stays
    /// the same as the line moves up the box while the scene behind it changes. Only the rows
    /// above [`DRAWN_ROWS`], which the game draws on every line.
    ink: Vec<u64>,
    count: u32,
    /// The first column with ink, from [`TEXT_LEFT`].
    left: u32,
    /// The columns the game's pointer may cover, from [`TEXT_LEFT`]: left out of `ink` here, and
    /// out of the other line when the two are compared.
    hidden: Option<Range<u32>>,
    /// The picture handed back if the line is new: only for channel lines.
    picture: Option<ChatLine>,
    /// Someone said it: a party or friend line that starts "name : ", or any whisper. False for the
    /// indented rest of a line, which goes with the line above it.
    said: bool,
}

impl StackLine for Line {
    /// The same line: a band of the same color, or text with nearly the same pixels, up to
    /// [`DRIFT`] pixels away each way. Columns the pointer covers in either line are left out.
    fn matches(&self, other: &Line) -> bool {
        match (self.kind, other.kind) {
            (Kind::Band(a), Kind::Band(b)) => {
                return a.into_iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24);
            }
            (a, b) if a != b => return false,
            _ => {}
        }
        let hidden = hidden_bits(self.hidden.as_ref(), other.hidden.as_ref());
        let seen = |line: &Line| match &hidden {
            Some(bits) => line
                .ink
                .iter()
                .zip(bits)
                .map(|(ink, hidden)| (ink & !hidden).count_ones())
                .sum(),
            None => line.count,
        };
        let (a, b) = (seen(self), seen(other));
        let allowed = (a.max(b) / 10).max(6);
        // Moved anywhere, they differ by at least their difference in pixels: no need to look.
        if a.abs_diff(b) > allowed {
            return false;
        }
        let same_at = |(right, down): Offset| {
            let by = down * TEXT_WIDTH as i32 + right;
            lines::differ(
                &self.ink,
                &other.ink,
                TEXT_WIDTH * LINE_HEIGHT,
                by,
                hidden.as_deref(),
            ) <= allowed
        };
        // Almost always where it was; only then a pixel or two away.
        same_at((0, 0))
            || (-DRIFT..=DRIFT)
                .flat_map(|down| (-DRIFT..=DRIFT).map(move |right| (right, down)))
                .any(same_at)
    }

    fn is_blank(&self) -> bool {
        self.kind == Kind::Blank
    }
}

impl Line {
    fn channel(&self) -> Option<Channel> {
        match self.kind {
            Kind::Text(channel) => channel,
            _ => None,
        }
    }
}

/// The columns hidden in either line, as [`Line::ink`] bits in every row, and as far again as two
/// lines are compared apart. `None` when neither line has any.
fn hidden_bits(a: Option<&Range<u32>>, b: Option<&Range<u32>>) -> Option<Vec<u64>> {
    if a.is_none() && b.is_none() {
        return None;
    }
    let mut bits = vec![0u64; (TEXT_WIDTH * LINE_HEIGHT).div_ceil(64) as usize];
    for columns in [a, b].into_iter().flatten() {
        let from = columns.start.saturating_sub(DRIFT as u32);
        let to = (columns.end + DRIFT as u32).min(TEXT_WIDTH);
        for row in 0..LINE_HEIGHT {
            for col in from..to {
                let bit = (row * TEXT_WIDTH + col) as usize;
                bits[bit / 64] |= 1 << (bit % 64);
            }
        }
    }
    Some(bits)
}

/// Bright enough to be a megaphone's band, or text.
fn bright(p: [u8; 3]) -> bool {
    p.into_iter().max().unwrap_or(0) >= 200
}

/// The solid middle of a letter: every text color has a channel at full strength. The box's dark
/// see-through background keeps the scene behind it, name tags included, well below this.
fn solid(p: [u8; 3]) -> bool {
    p.into_iter().max().unwrap_or(0) >= 245
}

/// Solid text in `channel`'s color.
fn solid_in(p: [u8; 3], channel: Channel) -> bool {
    solid(p) && distance(p, channel.color()) <= 60
}

fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.into_iter()
        .zip(b)
        .map(|(x, y)| (i32::from(x) - i32::from(y)).pow(2) as u32)
        .sum::<u32>()
        .isqrt()
}

/// How much of the pixel is text of `color`: solid within 24 of it, none from 184 away, which
/// keeps the smoothed edges of the letters. Only pixels next to solid text are asked, so the
/// scene behind the box isn't.
fn coverage(p: [u8; 3], color: [u8; 3]) -> u8 {
    const SOLID: u32 = 24;
    const NONE: u32 = 184;
    let d = distance(p, color);
    if d <= SOLID {
        255
    } else if d >= NONE {
        0
    } else {
        (255 * (NONE - d) / (NONE - SOLID)) as u8
    }
}

fn light_gray(p: [u8; 3]) -> bool {
    let (lo, hi) = (
        p.into_iter().min().unwrap_or(0),
        p.into_iter().max().unwrap_or(0),
    );
    lo >= 200 && hi - lo <= 16
}

/// The HUD in a 1x picture whose bottom row is the game area's, at one of the places it may be.
struct Hud<'f, 'a> {
    frame: &'f Frame<'a>,
    /// The HUD's left column and the row just below its bottom.
    left: u32,
    bottom: u32,
}

impl<'f, 'a> Hud<'f, 'a> {
    /// The HUD moved by `offset` from where the game area's size puts it, if the rows and columns
    /// the reader looks at are still in the picture.
    fn at_offset(frame: &'f Frame<'a>, (right, down): Offset) -> Option<Self> {
        let left = ((frame.width() - HUD_WIDTH) / 2).checked_add_signed(right)?;
        let bottom = frame.height().checked_add_signed(down)?;
        (left + THUMB.0 < frame.width() && bottom >= BOX_ROWS + frame.first_row()).then_some(Hud {
            frame,
            left,
            bottom,
        })
    }

    /// The pixel `dx` columns from the HUD's left edge and `up` rows above its bottom.
    fn at(&self, dx: u32, up: u32) -> [u8; 3] {
        self.frame.rgb(self.left + dx, self.bottom - up)
    }

    /// The input bar's top edge, and the scrollbar's edge just above it.
    fn has_chat_box(&self) -> bool {
        let input = [10, 200, 400].into_iter().all(|dx| {
            self.at(dx, INPUT_TOP_UP).into_iter().all(|c| c >= 240)
                && self.at(dx, INPUT_TOP_UP + 1).into_iter().all(|c| c <= 40)
        });
        input && light_gray(self.at(SCROLLBAR_EDGE, FIRST_LINE_UP))
    }

    /// The scrollbar's top arrow at the top of its edge, `top` rows up: the top row of its pale
    /// face, and the blue arrow in it. A light scene above the box can't pass for it.
    fn has_top_arrow(&self, top: u32) -> bool {
        let blue = |p: [u8; 3]| i32::from(p[2]) - i32::from(p[0]) >= 80;
        let like = |up: u32, color: [u8; 3]| distance(self.at(ARROW_FACE.0, up), color) <= 24;
        like(top, ARROW_FACE.1)
            && like(top + 1, ARROW_BORDER)
            && (2..ARROW_ROWS - 2).any(|k| {
                ARROW_COLUMNS
                    .into_iter()
                    .any(|dx| blue(self.at(dx, top - k)))
            })
    }
}

/// A frame's chat box: its lines, bottom line first, and where it is.
struct Scan {
    lines: Vec<Line>,
    offset: Offset,
    /// Where the box is, when its top arrow was found.
    place: Option<ChatPlace>,
    /// The box's top, in rows up from the picture's bottom, when its top arrow was found.
    top: Option<u32>,
    /// The box runs past the top of the rows read.
    clipped: bool,
}

/// The chat box in a 1x picture whose bottom row is the game area's. `prefer` is where the HUD
/// was last time, looked at first.
fn scan(frame: &Frame, prefer: Offset) -> Result<Scan, ChatUnreadable> {
    if frame.width() < HUD_WIDTH || frame.height() < CHAT_ROWS {
        return Err(ChatUnreadable::NotFound);
    }
    let (offset, hud) = std::iter::once(prefer)
        .chain(OFFSETS)
        .find_map(|offset| {
            Hud::at_offset(frame, offset)
                .filter(Hud::has_chat_box)
                .map(|hud| (offset, hud))
        })
        .ok_or(ChatUnreadable::NotFound)?;
    let thumb = hud.at(THUMB.0, THUMB.1);
    if light_gray(thumb) || i32::from(thumb[2]) - i32::from(thumb[0]) < 30 {
        return Err(ChatUnreadable::ScrolledBack);
    }
    // The scrollbar's top: the box holds the lines whose top row is under it. Its edge is followed
    // up to the top arrow, as far as the picture goes, for a box the player made taller. Stopping
    // at the arrow keeps a light scene above the box from passing for more scrollbar.
    let highest = hud.bottom - frame.first_row();
    let mut top = FIRST_LINE_UP;
    let mut found = false;
    while top < highest && light_gray(hud.at(SCROLLBAR_EDGE, top + 1)) {
        top += 1;
        if top < highest && top >= FIRST_LINE_UP + ARROW_ROWS && hud.has_top_arrow(top) {
            found = true;
            break;
        }
    }
    let clipped = !found && top == highest;
    let count = ((top - FIRST_LINE_UP) / LINE_HEIGHT + 1).min(MAX_LINES);
    let x = hud.left + TEXT_LEFT;
    // Each line's top row, bottom line first.
    let rows: Vec<u32> = (0..count)
        .map(|k| hud.bottom - FIRST_LINE_UP - LINE_HEIGHT * k)
        .collect();
    let pointer = pointer(frame, x, rows[rows.len() - 1]..rows[0] + LINE_HEIGHT);
    let lines = rows
        .iter()
        .map(|&y| {
            let hidden = pointer
                .as_ref()
                .filter(|p| p.rows.start < y + LINE_HEIGHT && y < p.rows.end)
                .map(|p| p.columns.clone());
            line(frame, x, y, hidden)
        })
        .collect();
    Ok(Scan {
        lines,
        offset,
        place: found.then(|| ChatPlace {
            width: frame.width(),
            height: frame.height(),
            left: hud.left,
            top: hud.bottom - (top + 1),
        }),
        top: found.then(|| frame.height() - (hud.bottom - top)),
        clipped,
    })
}

/// Where the game's pointer is over the lines: what it may cover of them.
struct Pointer {
    /// The text columns, from [`TEXT_LEFT`].
    columns: Range<u32>,
    /// The picture's rows.
    rows: Range<u32>,
}

/// The game's pointer over the text columns from `x` in these rows, found by its highlight. None
/// when no highlight shows, or one bigger than the pointer's.
fn pointer(frame: &Frame, x: u32, rows: Range<u32>) -> Option<Pointer> {
    // The highlight's leftmost and rightmost columns, top and bottom rows.
    let mut seen: Option<[u32; 4]> = None;
    for y in rows {
        for col in 0..TEXT_WIDTH {
            let p = frame.rgb(x + col, y);
            if p[1] < 240 || distance(p, POINTER_HIGHLIGHT) > 12 {
                continue;
            }
            seen = Some(match seen {
                None => [col, col, y, y],
                Some([left, right, top, bottom]) => {
                    [left.min(col), right.max(col), top.min(y), bottom.max(y)]
                }
            });
        }
    }
    let [left, right, top, bottom] = seen?;
    (right - left < POINTER_SPAN && bottom - top < POINTER_SPAN).then(|| Pointer {
        columns: left.saturating_sub(POINTER_REACH)..(right + 1 + POINTER_REACH).min(TEXT_WIDTH),
        rows: top.saturating_sub(POINTER_REACH)..bottom + 1 + POINTER_REACH,
    })
}

/// The line whose top-left text pixel is (x, y). Its `hidden` columns, under the pointer, are
/// left out of its ink.
fn line(frame: &Frame, x: u32, y: u32, hidden: Option<Range<u32>>) -> Line {
    let (w, h) = (TEXT_WIDTH as usize, LINE_HEIGHT as usize);
    let pixels: Vec<[u8; 3]> = (0..LINE_HEIGHT)
        .flat_map(|row| (0..TEXT_WIDTH).map(move |col| (col, row)))
        .map(|(col, row)| frame.rgb(x + col, y + row))
        .collect();
    let shown = TEXT_WIDTH - hidden.as_ref().map_or(0, |columns| columns.len() as u32);
    let mut ink = vec![0u64; (w * h).div_ceil(64)];
    let mut count = 0;
    let mut near = [0u32; 3];
    let mut first = u32::MAX;
    let mut lit: [Vec<u8>; 3] = Default::default();
    for (bit, &p) in pixels.iter().enumerate() {
        if hidden
            .as_ref()
            .is_some_and(|columns| columns.contains(&((bit % w) as u32)))
        {
            continue;
        }
        if bright(p) {
            for (c, v) in lit.iter_mut().zip(p) {
                c.push(v);
            }
        }
        // The rows the game may leave off are in the picture, not the shape.
        if !solid(p) || bit / w >= DRAWN_ROWS as usize {
            continue;
        }
        ink[bit / 64] |= 1 << (bit % 64);
        count += 1;
        first = first.min((bit % w) as u32);
        for (n, channel) in near.iter_mut().zip(Channel::ALL) {
            *n += u32::from(solid_in(p, channel));
        }
    }
    let kind = if lit[0].len() as u32 * 2 > shown * LINE_HEIGHT {
        // The band's color: the middle value of its bright pixels, each way.
        Kind::Band(lit.map(|mut values| {
            let middle = values.len() / 2;
            *values.select_nth_unstable(middle).1
        }))
    } else if count < 4 {
        Kind::Blank
    } else {
        let (best, n) = near
            .into_iter()
            .zip(Channel::ALL)
            .map(|(n, c)| (c, n))
            .max_by_key(|&(_, n)| n)
            .expect("three channels");
        Kind::Text((n >= 20 && n * 10 >= count * 6).then_some(best))
    };
    let picture = match kind {
        Kind::Text(Some(channel)) => Some(picture(&pixels, channel)),
        _ => None,
    };
    let said = match kind {
        Kind::Text(Some(Channel::Whisper)) => true,
        Kind::Text(Some(channel)) => has_speaker(&pixels, channel),
        _ => false,
    };
    Line {
        kind,
        ink,
        count,
        left: if count == 0 { 0 } else { first },
        hidden,
        picture,
        said,
    }
}

/// The line starts the way party and friend chat does, "name : words": a colon on its own between
/// spaces, after the name and before the words.
fn has_speaker(pixels: &[[u8; 3]], channel: Channel) -> bool {
    let (w, h) = (TEXT_WIDTH as usize, LINE_HEIGHT as usize);
    // Each column's rows of text, bit `row`.
    let columns: Vec<u16> = (0..w)
        .map(|col| {
            (0..h)
                .filter(|&row| solid_in(pixels[row * w + col], channel))
                .fold(0, |rows, row| rows | 1 << row)
        })
        .collect();
    // The glyphs: runs of columns with text, `[from, to)`.
    let mut glyphs = Vec::new();
    let mut start = None;
    for (col, &rows) in columns.iter().enumerate() {
        match (rows != 0, start) {
            (true, None) => start = Some(col),
            (false, Some(from)) => {
                glyphs.push((from, col));
                start = None;
            }
            _ => {}
        }
    }
    glyphs.windows(3).any(|three| {
        let [(_, name_end), (from, to), (words, _)] = [three[0], three[1], three[2]];
        from < NAME_COLUMNS
            && to - from <= 2
            && from - name_end >= 2
            && words - to >= 2
            && is_colon(columns[from..to].iter().fold(0, |rows, &c| rows | c))
    })
}

/// Two dots one above the other: two short runs of rows with a gap between. Not a period (one
/// dot), an `i` or a `!` (a long stroke), or a quote mark (one).
fn is_colon(rows: u16) -> bool {
    let mut runs = Vec::new();
    let mut run = 0;
    for row in 0..=16 {
        if row < 16 && rows >> row & 1 == 1 {
            run += 1;
        } else if run > 0 {
            runs.push(run);
            run = 0;
        }
    }
    runs.len() == 2 && runs.iter().all(|&run| run <= 3)
}

/// A channel line's picture: the columns its solid text spans, one pixel of margin each side.
fn picture(pixels: &[[u8; 3]], channel: Channel) -> ChatLine {
    let (w, h) = (TEXT_WIDTH as usize, LINE_HEIGHT as usize);
    let text: Vec<bool> = pixels.iter().map(|&p| solid_in(p, channel)).collect();
    let columns = (0..w).filter(|&col| (0..h).any(|row| text[row * w + col]));
    let (first, last) = columns.fold((w, 0), |(lo, hi), col| (lo.min(col), hi.max(col)));
    let (from, to) = (first.saturating_sub(1), (last + 2).min(w));
    let near_text = |row: usize, col: usize| {
        (row.saturating_sub(1)..(row + 2).min(h))
            .any(|r| (col.saturating_sub(1)..(col + 2).min(w)).any(|c| text[r * w + c]))
    };
    let mut coverage_of = Vec::with_capacity((to - from) * h);
    for row in 0..h {
        for col in from..to {
            coverage_of.push(match near_text(row, col) {
                true => coverage(pixels[row * w + col], channel.color()),
                false => 0,
            });
        }
    }
    ChatLine {
        // Given when the line is handed back.
        id: 0,
        channel,
        continues: false,
        width: (to - from) as u32,
        height: LINE_HEIGHT,
        coverage: coverage_of,
    }
}

/// Follows the chat box from frame to frame and hands back each new whisper, party and friend
/// line once.
#[derive(Debug, Default)]
pub struct ChatReader {
    /// The lines on screen last frame, bottom first; `None` after a frame that couldn't be read.
    lines: Option<Vec<Line>>,
    /// Lines handed back recently, newest last.
    recent: VecDeque<Line>,
    /// Lines handed back with part of their text missing (from the lowest line, or under the
    /// pointer), until they're seen whole: each one's id, and its place last frame (0 is the
    /// lowest).
    partial: Vec<(u64, usize)>,
    /// The box was being followed and then wasn't, so lines may have come and gone unseen.
    lost: bool,
    /// Where the HUD was last found, relative to where the game area's size puts it.
    offset: Offset,
    /// 1x rows to read for a box taller than the default; [`CHAT_ROWS`] when below that.
    rows: u32,
}

impl ChatReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// The 1x rows to read: the default box's, or more to reach the top of a taller one.
    fn rows(&self) -> u32 {
        self.rows.max(CHAT_ROWS)
    }

    /// How many of a game area's bottom rows the reader looks at when the area is `height` game
    /// pixels tall, in game rows.
    pub fn rows_needed(&self, height: u32) -> u32 {
        unstretch::rows_for(height, self.rows())
    }

    /// The new chat lines in this frame, which holds at least [`ChatReader::rows_needed`] rows.
    pub fn observe(&mut self, frame: &Frame) -> Result<ChatReading, ChatUnreadable> {
        let prefer = self.offset;
        let rows = self.rows();
        let scanned = unstretch::with_bottom_1x(frame, rows, |picture| {
            scan(picture, prefer).map(|scan| (scan, picture.height()))
        })
        .map_err(ChatUnreadable::Unsupported)
        .and_then(|scanned| scanned);
        let (
            Scan {
                lines: now,
                offset,
                place,
                top,
                clipped,
            },
            height,
        ) = match scanned {
            Ok(scanned) => scanned,
            Err(e) => {
                self.reset();
                return Err(e);
            }
        };
        self.offset = offset;
        // A box taller than the rows read: more next frame, until its top shows. A box that got
        // shorter: fewer again. Always the default box's rows and whole steps more, so dragging
        // the box's edge changes the rows read only when it crosses a step.
        if clipped {
            self.rows = (rows + GROW).min(height);
        } else if let Some(top) = top {
            let steps = (top + ABOVE_BOX).saturating_sub(CHAT_ROWS).div_ceil(GROW);
            self.rows = self.rows.min(CHAT_ROWS + steps * GROW);
        }
        let lined_up = self
            .lines
            .as_deref()
            .and_then(|before| lines::arrived(before, &now));
        let (fresh, gap) = match lined_up {
            Some(n) => (n, false),
            // Can't line up: everything on screen may be new.
            None => (now.len(), self.lines.is_some() || self.lost),
        };
        // Lines that can't be lined up may still be old ones: the ones on screen last frame, or
        // handed back recently, for after frames the box couldn't be read in (scrolled back,
        // covered) or that lined up with nothing.
        let seen_before = |line: &Line| {
            lined_up.is_none()
                && self
                    .lines
                    .iter()
                    .flatten()
                    .chain(&self.recent)
                    .any(|old| old.matches(line))
        };
        // The indented rest of a long line, and whether someone said each line: the rest of a line
        // goes with the line above it, so a notice's rest stays out with it. Top line first.
        let continues: Vec<bool> = (0..now.len())
            .map(|i| {
                now[i].left >= INDENT
                    && now
                        .get(i + 1)
                        .is_some_and(|above| above.channel() == now[i].channel())
            })
            .collect();
        // Lines handed back with part of their text missing, now whole: moved up, and out from
        // under the pointer. Lines that go off the box, or can't be followed, stay as they were
        // handed back.
        let mut whole = Vec::new();
        match lined_up {
            Some(moved) => self.partial.retain_mut(|(id, place)| {
                *place += moved;
                let Some(line) = now.get(*place) else {
                    return false;
                };
                if *place == 0 || line.hidden.is_some() {
                    return true;
                }
                if let Some(picture) = &line.picture {
                    whole.push(ChatLine {
                        id: *id,
                        continues: continues[*place],
                        ..picture.clone()
                    });
                }
                false
            }),
            None => self.partial.clear(),
        }
        let mut reading = ChatReading {
            lines: Vec::new(),
            whole,
            gap,
            place,
        };
        let mut said = vec![false; now.len()];
        for i in (0..now.len()).rev() {
            said[i] = match continues[i] {
                true => said[i + 1],
                false => now[i].said,
            };
        }
        let mut handed_back = Vec::new();
        // Oldest first: the highest new line.
        for i in (0..fresh).rev() {
            let line = &now[i];
            let Some(picture) = &line.picture else {
                continue;
            };
            if !said[i] || seen_before(line) {
                continue;
            }
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            if i == 0 || line.hidden.is_some() {
                self.partial.push((id, i));
            }
            reading.lines.push(ChatLine {
                id,
                continues: continues[i],
                ..picture.clone()
            });
            handed_back.push(Line {
                picture: None,
                ..line.clone()
            });
        }
        for line in handed_back {
            if self.recent.len() == RECENT {
                self.recent.pop_front();
            }
            self.recent.push_back(line);
        }
        self.lines = Some(now);
        self.lost = false;
        Ok(reading)
    }

    /// Forgets the lines on screen, for when capture stopped or the game window changed: the next
    /// frame is lined up against the lines handed back recently instead.
    pub fn reset(&mut self) {
        self.lost |= self.lines.is_some();
        self.lines = None;
        self.partial.clear();
    }
}

/// How many of a game area's bottom rows a new reader looks at when the area is `height` game
/// pixels tall, in game rows.
pub fn rows_needed(height: u32) -> u32 {
    unstretch::rows_for(height, CHAT_ROWS)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::recognizers::exp_bar::test_support::{Canvas, Font};

    pub(crate) const BRICK: [u8; 3] = [85, 59, 34];
    pub(crate) const WHITE: [u8; 3] = [255, 255, 255];
    pub(crate) const BAND: [u8; 3] = [232, 180, 5];
    const GRAY: [u8; 3] = [221, 221, 221];
    const THUMB_BLUE: [u8; 3] = [51, 102, 136];
    const TRACK: [u8; 3] = [231, 231, 231];

    /// A chat box over a brick wall in a `width` x `height` (at most 768) game area, with up to
    /// eight lines, bottom first.
    pub(crate) struct ChatBox {
        pub(crate) width: u32,
        pub(crate) height: u32,
        pub(crate) lines: Vec<(String, [u8; 3])>,
        pub(crate) scrolled_back: bool,
        /// Lines the box holds, from its scrollbar.
        pub(crate) size: u32,
        /// Rows from the top line's top row up to the top of the scrollbar's top arrow.
        pub(crate) arrow_above: u32,
        /// The scene showing through the megaphone bands: a different pattern for each value.
        pub(crate) scene: u32,
        /// How far the whole HUD sits from where the game area's size puts it: (right, down).
        pub(crate) shift: (i32, i32),
        /// How far the text sits from its place in the box, as when the box is read a pixel off:
        /// (right, down).
        pub(crate) nudge: (i32, i32),
        /// The game's mouse pointer over the box, its top-left corner this far from the HUD's left
        /// edge and up from its bottom: (right, up).
        pub(crate) pointer: Option<(u32, u32)>,
    }

    impl ChatBox {
        pub(crate) fn new(lines: &[(&str, [u8; 3])]) -> ChatBox {
            ChatBox {
                width: 1366,
                height: 768,
                lines: lines.iter().map(|(t, c)| (t.to_string(), *c)).collect(),
                scrolled_back: false,
                size: MAX_LINES,
                arrow_above: ARROW_ABOVE_LINES,
                scene: 0,
                shift: (0, 0),
                nudge: (0, 0),
                pointer: None,
            }
        }

        pub(crate) fn draw(&self) -> Canvas {
            let font = Font::new("abcdefghijklmnopqrstuvwxyz:>.! ", 3);
            let mut canvas = Canvas::new(self.width, self.height);
            canvas.fill(0, 0, self.width, self.height, BRICK);
            let left = ((self.width - HUD_WIDTH) / 2).wrapping_add_signed(self.shift.0);
            let y = |up: u32| (self.height - up).wrapping_add_signed(self.shift.1);
            // The input bar's top edge.
            canvas.fill(left, y(INPUT_TOP_UP + 1), HUD_WIDTH, 1, [8, 8, 8]);
            canvas.fill(left, y(INPUT_TOP_UP), HUD_WIDTH, 1, WHITE);
            // The scrollbar, from the bottom arrow to the top arrow.
            let top = FIRST_LINE_UP + LINE_HEIGHT * (self.size - 1) + self.arrow_above;
            canvas.fill(left + SCROLLBAR_EDGE, y(top), 1, top - 70 + 1, GRAY);
            canvas.fill(left + SCROLLBAR_EDGE + 1, y(top), 10, top - 70 + 1, TRACK);
            // The top arrow: a pale face around a blue arrow, under its border and the box's dark
            // top edge.
            canvas.fill(left + SCROLLBAR_EDGE, y(top + 1), 2, 1, [36, 22, 9]);
            canvas.fill(left + SCROLLBAR_EDGE + 2, y(top + 1), 11, 1, ARROW_BORDER);
            canvas.fill(
                left + SCROLLBAR_EDGE + 2,
                y(top),
                11,
                ARROW_ROWS,
                ARROW_FACE.1,
            );
            canvas.fill(left + 557, y(top - 3), 4, 5, [34, 102, 153]);
            if !self.scrolled_back {
                canvas.fill(left + THUMB.0 - 3, y(107), 8, 107 - 85 + 1, THUMB_BLUE);
            }
            for (k, (text, color)) in self.lines.iter().enumerate() {
                let line_top = y(FIRST_LINE_UP + LINE_HEIGHT * k as u32);
                if *color == BAND {
                    canvas.fill(left + TEXT_LEFT, line_top, TEXT_WIDTH, LINE_HEIGHT, BAND);
                    for col in (self.scene % 7..TEXT_WIDTH).step_by(7) {
                        let row = (col * 5 + self.scene) % LINE_HEIGHT;
                        canvas.set(left + TEXT_LEFT + col, line_top + row, [250, 245, 240]);
                    }
                    canvas.text(&font, text, left + 9, line_top + 4, |_| [60, 20, 0]);
                    continue;
                }
                let indent = if text.starts_with(' ') { 29 } else { 9 };
                let x = (left + indent).wrapping_add_signed(self.nudge.0);
                let row = |r: u32| (line_top + r).wrapping_add_signed(self.nudge.1);
                canvas.text(&font, text.trim_start(), x, row(4), |_| *color);
                // The tails of g, p and y, under the letters to the bottom of the line. The game
                // leaves their bottom rows off its lowest line.
                let bottom = if k == 0 { DRAWN_ROWS } else { LINE_HEIGHT };
                let mut at = x;
                for c in text.trim_start().chars() {
                    if "gpy".contains(c) {
                        for r in 11..bottom {
                            canvas.set(at, row(r), *color);
                        }
                    }
                    at += font.columns(c).len() as u32 + 1;
                }
            }
            if let Some((right, up)) = self.pointer {
                // A glove: a finger 8 columns wide over a palm 20 wide. A black edge, the pale
                // highlight inside it, then white, and the palm's shaded right side.
                let inside = |col: i32, row: i32| {
                    (0..28).contains(&row) && (0..if row < 14 { 8 } else { 20 }).contains(&col)
                };
                for row in 0..28 {
                    for col in 0..20 {
                        if !inside(col, row) {
                            continue;
                        }
                        let depth = (1..3)
                            .find(|&d| {
                                [(-d, 0), (d, 0), (0, -d), (0, d)]
                                    .into_iter()
                                    .any(|(c, r)| !inside(col + c, row + r))
                            })
                            .unwrap_or(3);
                        let color = match depth {
                            1 => [0, 0, 0],
                            2 => POINTER_HIGHLIGHT,
                            _ if col >= 13 => [153, 187, 204],
                            _ => WHITE,
                        };
                        canvas.set(left + right + col as u32, y(up) + row as u32, color);
                    }
                }
            }
            canvas
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::frame::Frame;
    use crate::recognizers::exp_bar::test_support::Canvas;

    const GREEN: [u8; 3] = [0, 255, 0];
    const PINK: [u8; 3] = [255, 153, 204];
    const ORANGE: [u8; 3] = [255, 153, 0];
    const YELLOW: [u8; 3] = [255, 255, 0];

    fn read(reader: &mut ChatReader, chat: &ChatBox) -> Result<ChatReading, ChatUnreadable> {
        reader.observe(&chat.draw().frame())
    }

    fn channels(reading: &ChatReading) -> Vec<Channel> {
        reading.lines.iter().map(|l| l.channel).collect()
    }

    // Fails if a channel's color is misclassified, or white, notice or megaphone lines leak into
    // the hub.
    #[test]
    fn splits_whisper_party_and_friend_lines_from_the_rest() {
        let chat = ChatBox::new(&[
            ("aeco: any nx", WHITE),
            ("fangkip : party test", PINK),
            ("temple: cape accuracy", BAND),
            ("notice: party invitations", YELLOW),
            ("lieon : what it do", ORANGE),
            ("lieon>> where pot", GREEN),
            ("medicaid: pure water", WHITE),
        ]);
        let reading = read(&mut ChatReader::new(), &chat).unwrap();
        // Oldest first: the highest line.
        assert_eq!(
            channels(&reading),
            [Channel::Whisper, Channel::Friend, Channel::Party]
        );
        assert!(!reading.gap, "the first frame isn't a gap");
        let party = &reading.lines[2];
        assert_eq!(party.height, LINE_HEIGHT);
        assert_eq!(party.coverage.len(), (party.width * party.height) as usize);
        assert!(party.coverage.contains(&255), "solid text");
        assert!(
            party.coverage.iter().filter(|&&c| c == 0).count() > party.coverage.len() / 2,
            "the brick wall behind isn't text"
        );
    }

    // Fails if the game's own notices in a chat type's color reach the hub, or chat with them goes
    // missing. Only lines someone said, "name : words", and the rest of them belong there.
    #[test]
    fn notices_in_a_chat_types_color_are_left_out() {
        let chat = ChatBox::new(&[
            ("fangkip : bye bye", PINK),
            ("fangkip has become the leader of the party.", PINK),
            (" that your pet is supposed to use.", PINK),
            ("you are lacking the mp potion", PINK),
            (" and the rest of it", PINK),
            ("naynshi : a long party line", PINK),
            ("lieon is online.", ORANGE),
            ("lieon>> no colon in a whisper", GREEN),
        ]);
        let reading = read(&mut ChatReader::new(), &chat).unwrap();
        assert_eq!(
            reading
                .lines
                .iter()
                .map(|l| (l.channel, l.continues))
                .collect::<Vec<_>>(),
            [
                (Channel::Whisper, false),
                (Channel::Party, false),
                (Channel::Party, true),
                (Channel::Party, false),
            ]
        );
    }

    #[test]
    fn counts_new_lines_by_how_far_the_box_moved() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[("lieon : ok", ORANGE), ("aeco : any nx", WHITE)]);
        assert_eq!(read(&mut reader, &chat).unwrap().lines.len(), 1);
        // Nothing new: the same lines.
        assert!(read(&mut reader, &chat).unwrap().lines.is_empty());
        // The same friend line again, then a whisper and a megaphone.
        let before = chat.lines.clone();
        chat.lines = vec![
            ("medicaid: pure water".into(), WHITE),
            ("lieon>> hello".into(), GREEN),
            ("lieon : ok".into(), ORANGE),
        ];
        chat.lines.extend(before);
        let reading = read(&mut reader, &chat).unwrap();
        assert_eq!(
            channels(&reading),
            [Channel::Friend, Channel::Whisper],
            "a repeated line counts again"
        );
        assert!(!reading.gap);
    }

    // The game leaves the tails of g, p and y off its lowest line, where new lines show first.
    // Fails if such a line isn't handed back whole under its id once it moves up, comes back as a
    // new line instead, or is handed back whole more than once.
    #[test]
    fn a_line_cut_off_at_the_bottom_comes_back_whole_once_it_moves_up() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[("lieon>> happy guppy", GREEN), ("aeco: any nx", WHITE)]);
        let first = read(&mut reader, &chat).unwrap();
        assert!(first.whole.is_empty());
        let [cut] = first.lines.as_slice() else {
            panic!("one new line: {first:?}");
        };
        assert!(
            read(&mut reader, &chat).unwrap().whole.is_empty(),
            "still the lowest line"
        );

        // A line of normal chat pushes it up, where the game draws it whole.
        chat.lines.insert(0, ("medicaid: pure water".into(), WHITE));
        let moved = read(&mut reader, &chat).unwrap();
        assert!(moved.lines.is_empty() && !moved.gap, "{moved:?}");
        let [whole] = moved.whole.as_slice() else {
            panic!("the line again, whole: {moved:?}");
        };
        assert_eq!(
            (whole.id, whole.channel, whole.continues, whole.width),
            (cut.id, Channel::Whisper, false, cut.width)
        );
        let rows = |line: &ChatLine, rows: Range<u32>| {
            line.coverage[(rows.start * line.width) as usize..(rows.end * line.width) as usize]
                .to_vec()
        };
        assert_eq!(rows(whole, 0..DRAWN_ROWS), rows(cut, 0..DRAWN_ROWS));
        assert!(rows(cut, DRAWN_ROWS..LINE_HEIGHT).iter().all(|&c| c == 0));
        assert!(
            rows(whole, DRAWN_ROWS..LINE_HEIGHT).contains(&255),
            "the tails"
        );

        chat.lines.insert(0, ("aeco: ok".into(), WHITE));
        let later = read(&mut reader, &chat).unwrap();
        assert!(
            later.lines.is_empty() && later.whole.is_empty(),
            "{later:?}"
        );
    }

    #[test]
    fn a_see_through_band_lines_up_while_the_scene_behind_it_changes() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[
            ("lieon : ok", ORANGE),
            ("temple: cape accuracy", BAND),
            ("aeco: any nx", WHITE),
            ("pide: nx", BAND),
        ]);
        assert_eq!(read(&mut reader, &chat).unwrap().lines.len(), 1);
        chat.scene = 3;
        let reading = read(&mut reader, &chat).unwrap();
        assert!(reading.lines.is_empty(), "nothing new: {reading:?}");
        assert!(!reading.gap, "the bands still line up");
    }

    #[test]
    fn a_box_that_turned_over_doesnt_repeat_a_line_still_on_screen() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[
            ("lieon : ok", ORANGE),
            ("aeco: any nx", WHITE),
            ("medicaid: pure water", WHITE),
        ]);
        chat.size = 3;
        assert_eq!(read(&mut reader, &chat).unwrap().lines.len(), 1);
        // No shift lines these up: everything else changed.
        chat.lines = vec![
            ("oakville: scrolls".into(), WHITE),
            ("lieon : ok".into(), ORANGE),
            ("nieox: pots".into(), WHITE),
        ];
        let reading = read(&mut reader, &chat).unwrap();
        assert!(reading.gap);
        assert!(reading.lines.is_empty(), "{reading:?}");
    }

    // Fails if a line read a pixel away from where it was last frame counts as a new line: every
    // line on screen would come back again.
    #[test]
    fn lines_read_a_pixel_off_are_the_same_lines() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[
            ("lieon : ok", ORANGE),
            ("aeco: any nx", WHITE),
            ("fangkip : party test", PINK),
            ("lieon>> where pot", GREEN),
        ]);
        assert_eq!(read(&mut reader, &chat).unwrap().lines.len(), 3);
        // A pixel off, then two pixels from the frame before: each frame may find the box a pixel
        // either way of where it is.
        for nudge in [(1, 0), (-1, 1), (1, -1), (0, 0)] {
            chat.nudge = nudge;
            let reading = read(&mut reader, &chat).unwrap();
            assert!(
                reading.lines.is_empty(),
                "{nudge:?}: {:?}",
                channels(&reading)
            );
            assert!(!reading.gap, "{nudge:?}: the lines still line up");
        }
    }

    // The game draws its own pointer into the picture (a Windows recording, 2026-11-06): a whisper
    // came in under it, and a megaphone came in as it moved off. Fails if the whisper, read with
    // and without the pointer over it, isn't the same line: in a 4-line box nothing else can line
    // the frames up then, and the whisper comes back after "lines may be missing".
    #[test]
    fn a_line_the_pointer_was_over_isnt_handed_back_again() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[
            ("lindalena>> test", GREEN),
            ("lindalena is online.", ORANGE),
            ("temple: cape accuracy", BAND),
            ("pide: nx", BAND),
        ]);
        chat.size = 4;
        assert_eq!(
            channels(&read(&mut reader, &chat).unwrap()),
            [Channel::Whisper]
        );
        let arrives = |chat: &mut ChatBox, line: (&str, [u8; 3])| {
            chat.lines.insert(0, (line.0.into(), line.1));
            chat.lines.truncate(4);
        };
        arrives(&mut chat, ("lindalena>> hello", GREEN));
        chat.pointer = Some((100, FIRST_LINE_UP - 2));
        let reading = read(&mut reader, &chat).unwrap();
        assert_eq!(channels(&reading), [Channel::Whisper], "the new whisper");
        assert!(!reading.gap);
        // The pointer moves along the whisper, then off it as a megaphone comes in.
        chat.pointer = Some((106, FIRST_LINE_UP - 4));
        let reading = read(&mut reader, &chat).unwrap();
        assert_eq!(channels(&reading), [], "nothing new");
        assert!(!reading.gap, "the pointer moved");
        chat.pointer = None;
        arrives(&mut chat, ("oglemon: slots", BAND));
        let reading = read(&mut reader, &chat).unwrap();
        assert_eq!(channels(&reading), [], "the whisper again");
        assert!(!reading.gap, "the lines still line up");
    }

    // Moving the pointer over the box while chatting, as on a Mac where whispers came back three
    // times: it comes to rest over two lines at once, and a reply comes in under it. Fails if two
    // lines changed by the pointer keep the frames from lining up, so the whisper under it comes
    // back.
    #[test]
    fn the_pointer_resting_over_two_lines_doesnt_bring_them_back() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[
            ("aeco: any nx", WHITE),
            ("lieon>> holy moly that was a close one", GREEN),
            ("medicaid: pure water", WHITE),
            ("fangkip : party test", PINK),
        ]);
        assert_eq!(
            channels(&read(&mut reader, &chat).unwrap()),
            [Channel::Party, Channel::Whisper]
        );
        chat.pointer = Some((40, FIRST_LINE_UP + LINE_HEIGHT + 4));
        let reading = read(&mut reader, &chat).unwrap();
        assert_eq!(channels(&reading), [], "the pointer came over the box");
        assert!(!reading.gap, "the pointer came over the box");
        chat.lines.insert(0, ("lieon>> ok".into(), GREEN));
        let reading = read(&mut reader, &chat).unwrap();
        assert_eq!(channels(&reading), [Channel::Whisper], "only the reply");
        assert!(!reading.gap, "a reply came in");
    }

    // The pointer hides the text under it, so a line read with the pointer over it is missing
    // those pixels. Fails if such a line isn't handed back again, whole, once the pointer moves
    // off, or if a line that moves up under the pointer is handed back with the pointer's hole.
    #[test]
    fn a_line_the_pointer_covered_comes_back_whole_once_it_moves_off() {
        let ink = |line: &ChatLine| line.coverage.iter().filter(|&&c| c > 0).count();
        let over_second_line = Some((40, FIRST_LINE_UP + LINE_HEIGHT + 4));
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[
            ("aeco: any nx", WHITE),
            ("lieon>> holy moly that was a close one", GREEN),
        ]);
        chat.pointer = over_second_line;
        let first = read(&mut reader, &chat).unwrap();
        let [covered] = first.lines.as_slice() else {
            panic!("one new line: {first:?}");
        };
        let still = read(&mut reader, &chat).unwrap();
        assert!(still.whole.is_empty(), "the pointer is still over it");
        chat.pointer = None;
        let off = read(&mut reader, &chat).unwrap();
        let [whole] = off.whole.as_slice() else {
            panic!("the line again, whole: {off:?}");
        };
        assert_eq!(whole.id, covered.id);
        assert!(ink(whole) > ink(covered), "the text the pointer was over");
        assert!(off.lines.is_empty() && !off.gap, "{off:?}");

        // A new whisper, cut off at the bottom, moves up with the pointer over it.
        chat.lines.insert(0, ("lieon>> happy guppy".into(), GREEN));
        let cut = read(&mut reader, &chat).unwrap().lines.remove(0);
        chat.lines.insert(0, ("medicaid: pure water".into(), WHITE));
        chat.pointer = over_second_line;
        let up = read(&mut reader, &chat).unwrap();
        assert!(up.lines.is_empty() && !up.gap, "{up:?}");
        assert!(up.whole.is_empty(), "the pointer is over it");
        chat.pointer = None;
        let off = read(&mut reader, &chat).unwrap();
        assert_eq!(off.whole.iter().map(|l| l.id).collect::<Vec<_>>(), [cut.id]);
        assert!(
            read(&mut reader, &chat).unwrap().whole.is_empty(),
            "only once"
        );
    }

    #[test]
    fn a_frame_that_lines_up_with_nothing_doesnt_bring_back_older_lines() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[("lieon : ok", ORANGE), ("aeco : any nx", WHITE)]);
        chat.size = 2;
        assert_eq!(read(&mut reader, &chat).unwrap().lines.len(), 1);
        // A frame where nothing lines up, then the first frame's lines again: the friend line was
        // handed back already, though it wasn't on screen in the frame just before.
        let first = chat.lines.clone();
        chat.lines = vec![
            ("oakville: scrolls".into(), WHITE),
            ("nieox: pots".into(), WHITE),
        ];
        assert!(read(&mut reader, &chat).unwrap().lines.is_empty());
        chat.lines = first;
        let reading = read(&mut reader, &chat).unwrap();
        assert!(reading.gap);
        assert!(reading.lines.is_empty(), "{:?}", channels(&reading));
    }

    #[test]
    fn the_rest_of_a_long_line_is_marked() {
        let chat = ChatBox::new(&[
            (" and the rest of it", GREEN),
            ("lieon>> a long whisper", GREEN),
        ]);
        let reading = read(&mut ChatReader::new(), &chat).unwrap();
        assert_eq!(
            reading
                .lines
                .iter()
                .map(|l| l.continues)
                .collect::<Vec<_>>(),
            [false, true]
        );
    }

    #[test]
    fn a_scrolled_back_box_reads_nothing_and_old_lines_dont_repeat() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[("lieon : ok", ORANGE), ("aeco : any nx", WHITE)]);
        assert_eq!(read(&mut reader, &chat).unwrap().lines.len(), 1);
        chat.scrolled_back = true;
        assert_eq!(read(&mut reader, &chat), Err(ChatUnreadable::ScrolledBack));
        // Back at the bottom with one new whisper: the friend line was handed back already.
        chat.scrolled_back = false;
        chat.lines.insert(0, ("lieon>> back".into(), GREEN));
        let reading = read(&mut reader, &chat).unwrap();
        assert_eq!(channels(&reading), [Channel::Whisper]);
        assert!(reading.gap, "lines may have come and gone meanwhile");
    }

    #[test]
    fn a_smaller_box_reads_only_its_lines() {
        let mut chat = ChatBox::new(&[("lieon : ok", ORANGE)]);
        chat.size = 3;
        let mut extra = vec![("x".to_string(), WHITE); 2];
        extra.push(("lieon>> above the box".into(), GREEN));
        chat.lines.extend(extra);
        // The fourth line is drawn above where the box ends, as the scene would be.
        let reading = read(&mut ChatReader::new(), &chat).unwrap();
        assert_eq!(channels(&reading), [Channel::Friend]);
    }

    // A chat box resized to 5 lines, seen on a Mac. Fails if its top line is dropped because less
    // of the arrow shows above it than in the default 8-line box.
    #[test]
    fn a_resized_box_reads_its_top_line() {
        let mut chat = ChatBox::new(&[
            ("aeco: any nx", WHITE),
            ("medicaid: pure water", WHITE),
            ("x", WHITE),
            ("x", WHITE),
            ("lieon : kumbis", ORANGE),
        ]);
        chat.size = 5;
        chat.arrow_above = 7;
        let reading = read(&mut ChatReader::new(), &chat).unwrap();
        assert_eq!(channels(&reading), [Channel::Friend]);
    }

    // Wine on a Mac can put the game a point off, which moves the whole HUD a game pixel (a
    // 1024x768 Mac capture, 2026-11-02). Fails if the reader only looks where the game area's size
    // puts the box, which reports a chat box in plain view as missing.
    #[test]
    fn reads_a_box_a_pixel_off() {
        for shift in [(1, 1), (-1, -1), (1, 0), (0, -1)] {
            let mut chat = ChatBox::new(&[("lieon : ok", ORANGE), ("aeco : any nx", WHITE)]);
            chat.shift = shift;
            let reading =
                read(&mut ChatReader::new(), &chat).unwrap_or_else(|e| panic!("{shift:?}: {e:?}"));
            assert_eq!(channels(&reading), [Channel::Friend], "{shift:?}");
        }
    }

    // The overlay sits on the box's top edge. Fails if the place the reading gives drifts from
    // where the box is drawn, or ignores the HUD's shift.
    #[test]
    fn says_where_the_box_is() {
        let mut chat = ChatBox::new(&[("lieon : ok", ORANGE)]);
        let place = |chat: &ChatBox| read(&mut ChatReader::new(), chat).unwrap().place.unwrap();
        let top_up = FIRST_LINE_UP + LINE_HEIGHT * (MAX_LINES - 1) + ARROW_ABOVE_LINES;
        assert_eq!(
            place(&chat),
            ChatPlace {
                width: 1366,
                height: 768,
                left: 283,
                top: 768 - (top_up + 1),
            }
        );
        chat.shift = (1, 1);
        assert_eq!(
            (place(&chat).left, place(&chat).top),
            (284, 769 - (top_up + 1))
        );
        // A smaller box: its top is lower.
        chat.shift = (0, 0);
        chat.size = 5;
        chat.arrow_above = 7;
        assert_eq!(
            place(&chat).top,
            768 - (FIRST_LINE_UP + LINE_HEIGHT * 4 + 7 + 1)
        );
    }

    // The player can make the box taller than the default. Fails if the reader stops at the rows it
    // reads first, which puts the overlay on the box's upper lines, or keeps reading the extra rows
    // after the box is made short again.
    #[test]
    fn follows_a_taller_box_up_and_back() {
        let mut reader = ChatReader::new();
        let mut look = |chat: &ChatBox| {
            let canvas = chat.draw();
            let frame = canvas.frame();
            let reading = reader.observe(&frame.bottom_rows(reader.rows_needed(768)));
            (reading.unwrap().place, reader.rows_needed(768))
        };
        let mut chat = ChatBox::new(&[("lieon : ok", ORANGE)]);
        chat.size = 14;
        let (place, rows) = look(&chat);
        assert_eq!(place, None, "the box's top is above the rows read");
        assert!(rows > CHAT_ROWS, "more rows next time");
        let (place, _) = look(&chat);
        let top_up = FIRST_LINE_UP + LINE_HEIGHT * 13 + ARROW_ABOVE_LINES;
        assert_eq!(place.map(|p| p.top), Some(768 - (top_up + 1)));
        // Back to the default size: the place follows at once, the rows read go back down.
        chat.size = MAX_LINES;
        let (place, rows) = look(&chat);
        assert_eq!(
            place.map(|p| p.top),
            Some(768 - (top_up + 1 - LINE_HEIGHT * 6))
        );
        assert_eq!(rows, CHAT_ROWS);
    }

    // A light scene right above the box, like a snowy map. Fails if the reader takes the scene for
    // more scrollbar, puts the box's top inside it and reads ever more rows looking for it.
    #[test]
    fn a_light_scene_above_the_box_isnt_its_top() {
        let chat = ChatBox::new(&[("lieon : ok", ORANGE)]);
        let mut canvas = chat.draw();
        let top_up = FIRST_LINE_UP + LINE_HEIGHT * 7 + ARROW_ABOVE_LINES;
        // Snow above the box, and showing through its edge next to the arrow's border.
        let snow = [235, 235, 238];
        canvas.fill(0, 0, 1366, 768 - top_up - 1, snow);
        canvas.fill(283 + SCROLLBAR_EDGE, 768 - top_up - 1, 2, 1, snow);
        let mut reader = ChatReader::new();
        let frame = canvas.frame();
        let reading = reader
            .observe(&frame.bottom_rows(reader.rows_needed(768)))
            .unwrap();
        assert_eq!(
            reading.place.map(|p| p.top),
            Some(768 - (top_up + 1)),
            "the border above the arrow"
        );
        assert_eq!(channels(&reading), [Channel::Friend]);
        assert_eq!(
            reader.rows_needed(768),
            CHAT_ROWS,
            "no more rows for the snow"
        );
    }

    #[test]
    fn no_chat_box_is_not_found() {
        let canvas = Canvas::new(1366, 768);
        assert_eq!(
            ChatReader::new().observe(&canvas.frame()),
            Err(ChatUnreadable::NotFound)
        );
    }

    #[test]
    fn reads_the_picture_the_game_stretched() {
        let chat = ChatBox::new(&[
            ("lieon : ok", ORANGE),
            ("aeco: any nx", WHITE),
            ("lieon>> hello", GREEN),
        ]);
        let canvas = chat.draw();
        let picture: Vec<[u8; 3]> = (0..768)
            .flat_map(|y| (0..1366).map(move |x| (x, y)))
            .map(|(x, y)| canvas.frame().rgb(x, y))
            .collect();
        let bgra = unstretch::stretch(&picture, 1366, 1920, 1080, 0);
        let frame = Frame::new(1920, 1080, 1920 * 4, &bgra).unwrap();
        let reading = ChatReader::new().observe(&frame).unwrap();
        assert_eq!(channels(&reading), [Channel::Whisper, Channel::Friend]);
    }

    // In a game stretched to full screen, where the box is is still given in the game's own
    // 768-line picture, and a taller box is still followed up. Fails if the place is measured in
    // the strip the stretch recovery rebuilds, which hid the overlay at 1080p.
    #[test]
    fn says_where_a_stretched_box_is_and_follows_it_taller() {
        let stretched = |chat: &ChatBox| {
            let canvas = chat.draw();
            let picture: Vec<[u8; 3]> = (0..768)
                .flat_map(|y| (0..1366).map(move |x| (x, y)))
                .map(|(x, y)| canvas.frame().rgb(x, y))
                .collect();
            unstretch::stretch(&picture, 1366, 1920, 1080, 0)
        };
        let mut chat = ChatBox::new(&[("lieon : ok", ORANGE)]);
        let bgra = stretched(&chat);
        let frame = Frame::new(1920, 1080, 1920 * 4, &bgra).unwrap();
        let top_up = FIRST_LINE_UP + LINE_HEIGHT * (MAX_LINES - 1) + ARROW_ABOVE_LINES;
        assert_eq!(
            ChatReader::new().observe(&frame).unwrap().place,
            Some(ChatPlace {
                width: 1366,
                height: 768,
                left: 283,
                top: 768 - (top_up + 1),
            })
        );

        chat.size = 14;
        let bgra = stretched(&chat);
        let frame = Frame::new(1920, 1080, 1920 * 4, &bgra).unwrap();
        let mut reader = ChatReader::new();
        let first = reader.observe(&frame).unwrap();
        assert_eq!(first.place, None, "the box's top is above the rows read");
        assert!(reader.rows_needed(1080) > rows_needed(1080));
        let top_up = FIRST_LINE_UP + LINE_HEIGHT * 13 + ARROW_ABOVE_LINES;
        assert_eq!(
            reader.observe(&frame).unwrap().place.map(|p| p.top),
            Some(768 - (top_up + 1))
        );
    }
}
