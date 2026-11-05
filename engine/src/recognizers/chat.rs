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
//! New lines are counted by how far the box's lines moved up since the last frame, the way the
//! notice stack is counted, so a friend saying "ok" twice shows twice. When the lines can't be
//! lined up (the box was scrolled back, covered, or just came into view), every colored line on
//! screen counts as new unless it was handed back recently, and the reading says lines may be
//! missing. While the player has scrolled back through the
//! chat, the box shows old lines and nothing is read.
//!
//! Chat pictures never become [`crate::events::Event`]s: plugins don't see them, and nothing
//! leaves the device.

use std::collections::VecDeque;

use serde::Serialize;

use crate::frame::Frame;
use crate::unstretch::{self, No1x};

/// The HUD bar's width at 1x.
const HUD_WIDTH: u32 = 800;
/// The columns chat text can use, from the HUD's left edge (the scrollbar starts at 551).
const TEXT_LEFT: u32 = 4;
const TEXT_RIGHT: u32 = 550;
const TEXT_WIDTH: u32 = TEXT_RIGHT - TEXT_LEFT;
/// The scrollbar's left edge, a light gray column from its bottom arrow to its top arrow.
const SCROLLBAR_EDGE: u32 = 551;
/// A column through the scrollbar's thumb, and a row just above the bottom arrow: the thumb is
/// there while the box shows the newest lines.
const THUMB: (u32, u32) = (556, 88);
/// The input bar's top edge: a white row over a near-black one, from the HUD's left edge.
const INPUT_TOP_UP: u32 = 67;
/// The lowest line's top row, counted up from the bottom, and the distance between lines. A line
/// runs from just above its capitals to the bottom of its descenders.
const FIRST_LINE_UP: u32 = 85;
const LINE_HEIGHT: u32 = 16;
/// The lines read: the default box's.
const MAX_LINES: u32 = 8;
/// In the default box, the top of the scrollbar's top arrow is this far above the top line's top
/// row. A resized box can leave less: 7 rows in a 5-line box (Mac captures, 2026-11-02).
const ARROW_ABOVE_LINES: u32 = 10;
/// 1x rows the reader looks at: the lines, the scrollbar's top arrow, and a few rows above it.
pub const CHAT_ROWS: u32 = FIRST_LINE_UP + LINE_HEIGHT * (MAX_LINES - 1) + ARROW_ABOVE_LINES + 4;
/// A line whose text starts this far in is the indented rest of a long line above it.
const INDENT: u32 = 16;
/// Lines handed back recently, for telling old lines from new ones after the box was scrolled.
const RECENT: usize = 64;

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
    /// The box couldn't be lined up with the last frame, so lines may be missing.
    pub gap: bool,
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
    /// the same as the line moves up the box while the scene behind it changes.
    ink: Vec<u64>,
    count: u32,
    /// The first column with ink, from [`TEXT_LEFT`].
    left: u32,
    /// The picture handed back if the line is new: only for channel lines.
    picture: Option<ChatLine>,
}

impl Line {
    /// The same line: a band of the same color, or text with nearly the same pixels (a mouse
    /// pointer over the line changes a few).
    fn matches(&self, other: &Line) -> bool {
        match (self.kind, other.kind) {
            (Kind::Band(a), Kind::Band(b)) => {
                return a.into_iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24);
            }
            (a, b) if a != b => return false,
            _ => {}
        }
        let differ: u32 = self
            .ink
            .iter()
            .zip(&other.ink)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum();
        differ <= (self.count.max(other.count) / 10).max(6)
    }

    fn channel(&self) -> Option<Channel> {
        match self.kind {
            Kind::Text(channel) => channel,
            _ => None,
        }
    }
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

/// The chat box's lines in a 1x picture whose bottom row is the game area's, bottom line first.
fn scan(frame: &Frame) -> Result<Vec<Line>, ChatUnreadable> {
    let (width, bottom) = (frame.width(), frame.height());
    if width < HUD_WIDTH || bottom < CHAT_ROWS {
        return Err(ChatUnreadable::NotFound);
    }
    let left = (width - HUD_WIDTH) / 2;
    let at = |dx: u32, up: u32| frame.rgb(left + dx, bottom - up);

    // The input bar's top edge, and the scrollbar's edge just above it.
    let input = [10, 200, 400].into_iter().all(|dx| {
        at(dx, INPUT_TOP_UP).into_iter().all(|c| c >= 240)
            && at(dx, INPUT_TOP_UP + 1).into_iter().all(|c| c <= 40)
    });
    if !input || !light_gray(at(SCROLLBAR_EDGE, FIRST_LINE_UP)) {
        return Err(ChatUnreadable::NotFound);
    }
    let thumb = at(THUMB.0, THUMB.1);
    if light_gray(thumb) || i32::from(thumb[2]) - i32::from(thumb[0]) < 30 {
        return Err(ChatUnreadable::ScrolledBack);
    }
    // The scrollbar's top: the box holds the lines whose top row is under it.
    let mut top = FIRST_LINE_UP;
    while top < CHAT_ROWS - 1 && light_gray(at(SCROLLBAR_EDGE, top + 1)) {
        top += 1;
    }
    let lines = ((top - FIRST_LINE_UP) / LINE_HEIGHT + 1).min(MAX_LINES);
    Ok((0..lines)
        .map(|k| {
            line(
                frame,
                left + TEXT_LEFT,
                bottom - FIRST_LINE_UP - LINE_HEIGHT * k,
            )
        })
        .collect())
}

/// The line whose top-left text pixel is (x, y).
fn line(frame: &Frame, x: u32, y: u32) -> Line {
    let (w, h) = (TEXT_WIDTH as usize, LINE_HEIGHT as usize);
    let pixels: Vec<[u8; 3]> = (0..LINE_HEIGHT)
        .flat_map(|row| (0..TEXT_WIDTH).map(move |col| (col, row)))
        .map(|(col, row)| frame.rgb(x + col, y + row))
        .collect();
    let mut ink = vec![0u64; (w * h).div_ceil(64)];
    let mut count = 0;
    let mut near = [0u32; 3];
    let mut first = u32::MAX;
    let mut lit: [Vec<u8>; 3] = Default::default();
    for (bit, &p) in pixels.iter().enumerate() {
        if bright(p) {
            for (c, v) in lit.iter_mut().zip(p) {
                c.push(v);
            }
        }
        if !solid(p) {
            continue;
        }
        ink[bit / 64] |= 1 << (bit % 64);
        count += 1;
        first = first.min((bit % w) as u32);
        for (n, channel) in near.iter_mut().zip(Channel::ALL) {
            *n += u32::from(solid_in(p, channel));
        }
    }
    let kind = if lit[0].len() as u32 * 2 > TEXT_WIDTH * LINE_HEIGHT {
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
    Line {
        kind,
        ink,
        count,
        left: if count == 0 { 0 } else { first },
        picture,
    }
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
        channel,
        continues: false,
        width: (to - from) as u32,
        height: LINE_HEIGHT,
        coverage: coverage_of,
    }
}

/// How many lines arrived at the bottom: the smallest shift that lines up the last frame's lines
/// with this one's. One line may differ when four or more line up, for a pointer over the box.
/// `None` when no shift does.
fn arrived(before: &[Line], now: &[Line]) -> Option<usize> {
    (0..=now.len()).find(|&shift| {
        let pairs: Vec<bool> = before
            .iter()
            .zip(now.iter().skip(shift))
            .map(|(b, n)| b.matches(n))
            .collect();
        let misses = pairs.iter().filter(|&&m| !m).count();
        match shift == now.len() {
            // Everything on screen is new: that only lines up if the last frame had no lines.
            true => before.iter().all(|l| l.kind == Kind::Blank),
            false => misses == 0 || (pairs.len() >= 4 && misses <= 1),
        }
    })
}

/// Follows the chat box from frame to frame and hands back each new whisper, party and friend
/// line once.
#[derive(Debug, Default)]
pub struct ChatReader {
    /// The lines on screen last frame, bottom first; `None` after a frame that couldn't be read.
    lines: Option<Vec<Line>>,
    /// Lines handed back recently, newest last.
    recent: VecDeque<Line>,
    /// The box was being followed and then wasn't, so lines may have come and gone unseen.
    lost: bool,
}

impl ChatReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// The new chat lines in this frame.
    pub fn observe(&mut self, frame: &Frame) -> Result<ChatReading, ChatUnreadable> {
        let now = unstretch::with_bottom_1x(frame, CHAT_ROWS, scan)
            .map_err(ChatUnreadable::Unsupported)
            .and_then(|lines| lines);
        let now = match now {
            Ok(now) => now,
            Err(e) => {
                self.reset();
                return Err(e);
            }
        };
        let lined_up = self
            .lines
            .as_deref()
            .and_then(|before| arrived(before, &now));
        let (fresh, gap) = match lined_up {
            Some(n) => (n, false),
            // Can't line up: everything on screen may be new.
            None => (now.len(), self.lines.is_some() || self.lost),
        };
        // Lines that can't be lined up may still be old ones: after frames the box couldn't be read
        // in (scrolled back, covered), the ones handed back recently; between two readable frames,
        // the ones on screen last frame.
        let seen_before = |line: &Line| match (lined_up, &self.lines) {
            (Some(_), _) => false,
            (None, None) => self.recent.iter().any(|r| r.matches(line)),
            (None, Some(before)) => before.iter().any(|b| b.matches(line)),
        };
        let mut reading = ChatReading {
            lines: Vec::new(),
            gap,
        };
        let mut handed_back = Vec::new();
        // Oldest first: the highest new line.
        for i in (0..fresh).rev() {
            let line = &now[i];
            let Some(picture) = &line.picture else {
                continue;
            };
            if seen_before(line) {
                continue;
            }
            let continues = line.left >= INDENT
                && now
                    .get(i + 1)
                    .is_some_and(|above| above.channel() == line.channel());
            reading.lines.push(ChatLine {
                continues,
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
    }
}

/// How many of a game area's bottom rows the reader looks at when the area is `height` game
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
            }
        }

        pub(crate) fn draw(&self) -> Canvas {
            let font = Font::new("abcdefghijklmnopqrstuvwxyz:>.! ", 3);
            let mut canvas = Canvas::new(self.width, self.height);
            canvas.fill(0, 0, self.width, self.height, BRICK);
            let left = (self.width - HUD_WIDTH) / 2;
            let y = |up: u32| self.height - up;
            // The input bar's top edge.
            canvas.fill(left, y(INPUT_TOP_UP + 1), HUD_WIDTH, 1, [8, 8, 8]);
            canvas.fill(left, y(INPUT_TOP_UP), HUD_WIDTH, 1, WHITE);
            // The scrollbar, from the bottom arrow to the top arrow.
            let top = FIRST_LINE_UP + LINE_HEIGHT * (self.size - 1) + self.arrow_above;
            canvas.fill(left + SCROLLBAR_EDGE, y(top), 1, top - 70 + 1, GRAY);
            canvas.fill(left + SCROLLBAR_EDGE + 1, y(top), 10, top - 70 + 1, TRACK);
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
                canvas.text(
                    &font,
                    text.trim_start(),
                    left + indent,
                    line_top + 4,
                    |_| *color,
                );
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
            ("fangkip: party test", PINK),
            ("temple: cape accuracy", BAND),
            ("notice: party invitations", YELLOW),
            ("lieon: what it do", ORANGE),
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

    #[test]
    fn counts_new_lines_by_how_far_the_box_moved() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[("lieon: ok", ORANGE), ("aeco: any nx", WHITE)]);
        assert_eq!(read(&mut reader, &chat).unwrap().lines.len(), 1);
        // Nothing new: the same lines.
        assert!(read(&mut reader, &chat).unwrap().lines.is_empty());
        // The same friend line again, then a whisper and a megaphone.
        let before = chat.lines.clone();
        chat.lines = vec![
            ("medicaid: pure water".into(), WHITE),
            ("lieon>> hello".into(), GREEN),
            ("lieon: ok".into(), ORANGE),
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

    #[test]
    fn a_see_through_band_lines_up_while_the_scene_behind_it_changes() {
        let mut reader = ChatReader::new();
        let mut chat = ChatBox::new(&[
            ("lieon: ok", ORANGE),
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
            ("lieon: ok", ORANGE),
            ("aeco: any nx", WHITE),
            ("medicaid: pure water", WHITE),
        ]);
        chat.size = 3;
        assert_eq!(read(&mut reader, &chat).unwrap().lines.len(), 1);
        // No shift lines these up: everything else changed.
        chat.lines = vec![
            ("oakville: scrolls".into(), WHITE),
            ("lieon: ok".into(), ORANGE),
            ("nieox: pots".into(), WHITE),
        ];
        let reading = read(&mut reader, &chat).unwrap();
        assert!(reading.gap);
        assert!(reading.lines.is_empty(), "{reading:?}");
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
        let mut chat = ChatBox::new(&[("lieon: ok", ORANGE), ("aeco: any nx", WHITE)]);
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
        let mut chat = ChatBox::new(&[("lieon: ok", ORANGE)]);
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
            ("lieon: kumbis", ORANGE),
        ]);
        chat.size = 5;
        chat.arrow_above = 7;
        let reading = read(&mut ChatReader::new(), &chat).unwrap();
        assert_eq!(channels(&reading), [Channel::Friend]);
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
            ("lieon: ok", ORANGE),
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
}
