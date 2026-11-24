//! EXP bar reader: level, EXP and percent from the HUD at the bottom of the game.
//!
//! Reads the game's own 1x pixels at the default UI Size. Windowed 800x600 to 1366x768 are drawn
//! at 1x. Taller sizes and Full Screen are a 768-line picture the game stretches, and
//! [`unstretch`] recovers the HUD's rows of that picture first, so every size reads the same
//! way with the same glyph shapes. A Retina Mac doubles every pixel, which [`unstretch::unscale`]
//! undoes exactly before anything else. The HUD is an 800 px bar centered at the bottom, so its
//! text sits at fixed offsets from ((width - 800) / 2, height) of the 1x picture.
//!
//! On a Mac, Wine can place the game up to a point off where the window's bounds put it, so the
//! reader finds the HUD within a pixel of that place: while learning, by the row at which the EXP
//! field makes sense (one row off garbles it), and after that by the learned "EXP." label,
//! matched exactly. A HUD further off is unreadable.
//!
//! The HUD numbers are a one-bit pixel font, and the level is white digits on orange tiles.
//! Matching is exact, so a frame with anything unexpected in the text comes out unreadable, never
//! as a guess. Nothing learned leaves the device, and no game pixels ship.
//!
//! **It calibrates itself**, so the player types nothing. The bar's percent is always
//! round(EXP x 10000 / EXP needed for the level) in hundredths, so of the 3.6 million ways to name
//! the ten digit shapes, only one fits a handful of different EXP values. Punctuation is named by
//! color and position. As EXP moves,
//! [`ExpBarReader::observe`] goes through three states ([`Calibration`]):
//!
//! - **Learning:** too few different EXP values to tell. Nothing is read.
//! - **Provisional:** one naming fits if EXP needed is a value from meowdb's EXP table
//!   ([`super::exp_table`]). Readings come out marked unconfirmed and produce no events.
//! - **Confirmed:** one naming fits on the HUD's evidence alone, without the table.
//!
//! On recorded Windows captures, readings appeared after four different EXP values and were
//! confirmed after six, about four seconds of grinding. The level is the table's level whose EXP
//! needed fits the bar. Once confirmed, the level tiles' digits are learned from it, and a
//! level-up teaches each new tile digit as the previous level plus one. Typing a reading
//! ([`ExpBarReader::calibrate`]) stays as a fallback.
//!
//! **Late in a level the HUD shows only the percent**, `[92.61%]`. On a recorded Windows run at
//! level 58 it did from 92.61% on, past the point where the EXP reached a million (91.95% of
//! 1,087,536). That field is read with the digits learned from the full one, this session or an
//! earlier one ([`ExpBarReader::from_knowledge`]), and the reading's EXP is worked out from the
//! percent and the level's EXP needed ([`ExpReading::exp_shown`] is false). A reader that has
//! only ever seen the percent names the digits by the order they climb in ([`Climb`]), in one or
//! two whole percents of grinding. It reads the percent from then on, but the level and the EXP
//! wait for the full field, at the next level.

use super::exp_table;
use super::hp_mp::{self, HpMp, HpMpUnreadable, SlashLearner};
use crate::events::Event;
use crate::frame::Frame;
use crate::glyph::{Atlas, Glyph, LearnError, Rect, cut, segment};
use crate::unstretch;

/// The HUD bar's width at 1x.
const HUD_WIDTH: u32 = 800;
/// How far, in game pixels each way, the HUD may sit from where the game area's size puts it.
/// Wine on a Mac places the game up to a point off.
const MAX_SHIFT: u32 = 1;
/// The tallest game area drawn at 1x. Above it the game stretches a 768-line image, or in Full
/// Screen the smaller size the player chose ([`Frame::source`]).
pub const MAX_NATIVE_HEIGHT: u32 = unstretch::NATIVE_HEIGHT;

/// A region given by its offset from the HUD's left edge and how far above the frame's bottom
/// its top row is. Measured on recorded captures.
#[derive(Debug, Clone, Copy)]
struct Area {
    dx: u32,
    up: u32,
    width: u32,
    height: u32,
}

/// The "EXP." label, used to check the HUD is where it should be.
const LABEL: Area = Area {
    dx: 438,
    up: 31,
    width: 28,
    height: 10,
};
/// The EXP total and percent, for example `126,987[39.16%]`, or late in a level the percent alone,
/// `[92.61%]`, whose bracket starts at dx 468. It stops before the CASH SHOP button's art at dx
/// 566, which leaves room for an 8-digit total.
const EXP_TEXT: Area = Area {
    dx: 465,
    up: 28,
    width: 100,
    height: 7,
};
/// The HP numbers, for example `[712/1386]`, on the EXP field's rows: from after the "HP" label,
/// whose ink ends at dx 234, to before the "MP" label at dx 329. Measured on recorded captures
/// (windowed 800x600, 1024x768 and 1366x768, on Windows and a Mac): the numbers start at dx 237
/// whatever their length, and `[1386/1386]` ends at dx 297, so four-digit values leave 30 px. Even
/// `[30000/30000]` would end near dx 310.
const HP_TEXT: Area = Area {
    dx: 236,
    up: 28,
    width: 92,
    height: 7,
};
/// The MP numbers: from after the "MP" label, whose ink ends at dx 345, to before the "EXP." label.
/// They start at dx 348; `[1140/1140]` ends at dx 408.
const MP_TEXT: Area = Area {
    dx: 347,
    up: 28,
    width: 91,
    height: 7,
};
/// The level digits on their orange tiles.
const LEVEL_TEXT: Area = Area {
    dx: 44,
    up: 21,
    width: 38,
    height: 7,
};

/// Every character the EXP field can show.
pub const EXP_CHARS: &str = "0123456789,.[]%";
/// Every character the level can show.
pub const LEVEL_CHARS: &str = "0123456789";
const DIGITS: &str = "0123456789";

/// Rows of the EXP field (top row in bit 0) that hold a separator's ink: the bottom two.
const COMMA_ROWS: u32 = 0b110_0000;
/// Rows that hold a decimal point's ink: the bottom one.
const DOT_ROWS: u32 = 0b100_0000;
/// Distinct EXP fields kept while learning.
const MAX_OBSERVATIONS: usize = 64;

/// White digits and the yellow-green brackets on the dark HUD.
fn text_ink(p: [u8; 3]) -> bool {
    p.into_iter().max().unwrap_or(0) >= 150
}

/// White level digits; the orange tiles behind them aren't ink.
fn level_ink(p: [u8; 3]) -> bool {
    p.into_iter().min().unwrap_or(0) >= 170
}

/// Ink with little blue: the yellow-green brackets, not the white digits.
fn green_ink(p: [u8; 3]) -> bool {
    p[2] < 120
}

#[derive(Debug, Clone, Copy)]
struct Layout {
    hud_left: u32,
    bottom: u32,
}

impl Layout {
    fn of(frame: &Frame) -> Option<Layout> {
        let (w, h) = (frame.width(), frame.height());
        // `then`, not `then_some`: the width is only subtracted once it's known to be wide enough.
        (w >= HUD_WIDTH && (LABEL.up..=MAX_NATIVE_HEIGHT).contains(&h)).then(|| Layout {
            hud_left: (w - HUD_WIDTH) / 2,
            bottom: h,
        })
    }

    fn rect(self, area: Area) -> Rect {
        Rect {
            x: self.hud_left + area.dx,
            y: self.bottom - area.up,
            width: area.width,
            height: area.height,
        }
    }

    /// The layout with the HUD moved by `offset` (right and down), if it still fits above the
    /// bottom edge.
    fn shifted(self, (dx, dy): Offset) -> Option<Layout> {
        Some(Layout {
            hud_left: self.hud_left.checked_add_signed(dx)?,
            bottom: self
                .bottom
                .checked_add_signed(dy)
                .filter(|&b| b >= LABEL.up)?,
        })
    }
}

/// Where the HUD sits relative to where the game area's size puts it, in game pixels: (right,
/// down).
type Offset = (i32, i32);

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

/// Rows of the 1x picture the reader looks at, counted up from its bottom: from the label's top
/// row down, with room for the HUD to sit a row higher.
const HUD_ROWS: u32 = LABEL.up + MAX_SHIFT;

/// Calls `read` with the frame at the game's own pixel size and the HUD's layout in it, where the
/// game area's size puts it. A frame that declares a backing scale is undone first; then it's
/// the frame itself up to [`Frame::source`] lines, or else the HUD's rows of the picture the game
/// stretched.
/// Fails when the HUD can't be placed: narrower than the bar, a picture that can't be recovered,
/// or a frame missing rows the reader needs.
fn at_1x<T>(frame: &Frame, read: impl FnOnce(&Frame, Layout) -> T) -> Result<T, Unreadable> {
    let scale = frame.scale();
    if scale > 1 {
        // Only the rows a backend reads back: the game's picture is doubled there, but not
        // necessarily everywhere (a Mac title bar's shadow falls on the top rows).
        let rows =
            unstretch::scaled_rows(rows_needed(frame.height() / scale, frame.source()), scale);
        let picture = unstretch::unscale(&frame.bottom_rows(rows))
            .ok_or(Unreadable::UnsupportedScale { scale })?;
        return at_1x(&picture.frame(), read);
    }
    if frame.height() <= frame.source() {
        let layout = Layout::of(frame).ok_or_else(|| unsupported(frame))?;
        // Without all of its rows, the HUD can't be looked for everywhere it may be.
        if frame.first_row() + rows_needed(frame.height(), frame.source()) > frame.height() {
            return Err(Unreadable::HudNotFound);
        }
        return Ok(read(frame, layout));
    }
    let picture = unstretch::bottom_rows(frame, HUD_ROWS).ok_or_else(|| unsupported(frame))?;
    let recovered = picture.frame();
    Layout::of(&recovered)
        .map(|layout| read(&recovered, layout))
        .ok_or_else(|| unsupported(frame))
}

/// How many of a game area's bottom rows the reader looks at when the area is `height` game
/// pixels tall and scaled up from a picture `source` lines tall ([`Frame::source`]): all a
/// capture backend needs to read back, in game rows. The HUD's rows at 1x, or the rows the
/// stretch recovery reads above `source` lines. A backend capturing at a backing scale reads
/// [`unstretch::scaled_rows`] of these.
pub fn rows_needed(height: u32, source: u32) -> u32 {
    unstretch::rows_for(height, source, HUD_ROWS)
}

/// After a frame whose EXP bar couldn't be read (`reading`): the height of another picture the
/// game area may be scaled up from, if the HUD shows only at that one. Full Screen scales the
/// size the player chose to fit the display, so on a 14-inch MacBook Pro 1366x768 and 1280x720
/// fill the same 1512x850 picture, and only the HUD's size inside it tells them apart. The HUD
/// shows when its "EXP." label and EXP field are where the game area's size puts them, which needs
/// nothing learned. `None` after a reading, while learning (the HUD was seen), on a dark screen,
/// or when the HUD shows at the frame's own [`Frame::source`], at no other size, or at more than
/// one.
pub fn other_source(frame: &Frame, reading: &Result<ExpReading, Unreadable>) -> Option<u32> {
    match reading {
        Err(
            Unreadable::HudNotFound
            | Unreadable::NoText(_)
            | Unreadable::UnknownGlyph(_)
            | Unreadable::Garbled(_)
            | Unreadable::UnsupportedSize { .. },
        ) => {}
        _ => return None,
    }
    hud_source(frame)
}

/// The height of another picture the game area may be scaled up from, if the HUD shows only at
/// that one: [`other_source`] without looking at a reading first, for a caller with no EXP bar
/// reader. `None` when the HUD shows at the frame's own [`Frame::source`], at no other size, or at
/// more than one.
pub fn hud_source(frame: &Frame) -> Option<u32> {
    let shows = |lines: u32| {
        at_1x(&frame.with_source(lines), |frame, nominal| {
            find_by_structure(frame, nominal).is_some()
        })
        .unwrap_or(false)
    };
    if shows(frame.source()) {
        return None;
    }
    let scale = frame.scale();
    let mut found = unstretch::sources(frame.width() / scale, frame.height() / scale)
        .into_iter()
        .filter(|&lines| lines != frame.source() && shows(lines));
    match (found.next(), found.next()) {
        (Some(lines), None) => Some(lines),
        _ => None,
    }
}

/// What the HUD showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpReading {
    /// From the level tiles once their digits are known, otherwise the table's level whose EXP
    /// needed fits the bar. `None` when neither can tell.
    pub level: Option<u16>,
    /// EXP into the current level. When the HUD shows only the percent, the EXP that percent
    /// allows at the level's EXP needed, so off by less than one hundredth of a percent's worth:
    /// the last EXP at the same level while the percent still allows it, else the middle of the
    /// range.
    /// `None` when the HUD shows only the percent and EXP needed isn't known.
    pub exp: Option<u64>,
    /// Whether the HUD showed the EXP. False when it showed only the percent, as it does late in
    /// a level, and `exp` was worked out from it.
    pub exp_shown: bool,
    /// The percentage on the bar in hundredths: 3916 is 39.16%.
    pub pct_hundredths: u16,
    /// The EXP the whole level needs: the table's value for the level, when it fits the bar.
    pub needed: Option<u64>,
    /// False while the digits' names rest on the EXP table; true once the HUD alone confirmed
    /// them, or the player typed a reading.
    pub confirmed: bool,
}

/// What one pass over a frame's HUD read: the EXP bar, and the HP and MP numbers beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudReading {
    pub exp: Result<ExpReading, Unreadable>,
    pub hp_mp: Result<HpMp, HpMpUnreadable>,
}

/// What the player read off the HUD, for typed calibration (the fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedReading {
    pub level: u16,
    pub exp: u64,
    pub pct_hundredths: u16,
}

/// How far self-calibration has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Calibration {
    /// Too few different EXP values to name the digits.
    Learning,
    /// The digits are named with the help of the EXP table; readings are unconfirmed.
    Provisional,
    /// The digits are named on the HUD's evidence alone, or from a typed reading.
    Confirmed,
}

/// Which part of the HUD a problem is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Exp,
    Level,
}

/// Why a frame couldn't be read. None of these produce a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreadable {
    /// The game area can't hold the HUD at 1x: narrower than the HUD bar, or stretched from a
    /// picture that is.
    UnsupportedSize { width: u32, height: u32 },
    /// The frame declares a backing scale, but it isn't the game's picture with each pixel
    /// repeated: something scaled the game another way.
    UnsupportedScale { scale: u32 },
    /// Nothing has been learned yet.
    NotCalibrated,
    /// Self-calibration needs a few more different EXP values.
    Learning,
    /// The "EXP." label isn't where the HUD puts it: covered, hidden, or the UI changed.
    HudNotFound,
    /// Nothing in the HUD's rows is white: the game dimmed the screen, as it does fading to black
    /// for a map change, so it isn't drawing a HUD to read. Not a failure to read one.
    ScreenDark,
    /// The field holds no text.
    NoText(Field),
    /// A shape the atlas doesn't know: not learned yet, or something drawn over the text.
    UnknownGlyph(Field),
    /// The text doesn't make sense as that field.
    Garbled(Field),
}

/// Why a typed calibration was refused. It's all or nothing, so the reader is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalibrationError {
    UnsupportedSize {
        width: u32,
        height: u32,
    },
    /// The level is 0 or the percent is above 100.
    ImplausibleReading,
    /// There's no "EXP." label where the HUD puts it.
    HudNotFound,
    /// The label differs from the one learned before: the HUD moved or changed.
    HudChanged,
    /// The field holds no text, or something other than text.
    NoText(Field),
    /// The typed values don't line up with the glyphs on screen, or contradict earlier ones.
    Mismatch(Field, LearnError),
}

/// Glyphs still unseen, by field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub exp: Vec<char>,
    pub level: Vec<char>,
}

impl Missing {
    pub fn is_empty(&self) -> bool {
        self.exp.is_empty() && self.level.is_empty()
    }
}

/// Reads the EXP bar, with glyph shapes learned from the player's own screen.
#[derive(Debug, Clone, Default)]
pub struct ExpBarReader {
    label: Option<Glyph>,
    /// Punctuation, and the digits once confirmed.
    digits: Atlas,
    level_digits: Atlas,
    learner: Learner,
    /// The last reading `observe` returned, for level-ups and new digits.
    last: Option<ExpReading>,
    /// The last reading with an EXP, which a reading of the percent alone carries on from. Kept
    /// apart from `last`: a frame with the level tiles hidden (the cursor over them) has no level
    /// and so no EXP, and carrying on from it would restart the EXP at the middle of the range.
    last_exp: Option<ExpReading>,
    /// The level tiles' glyphs in the last frame `observe` read. They change at a level-up and
    /// not at a death, though the EXP falls at both.
    last_tiles: Option<Vec<Glyph>>,
    /// Where the HUD was last found, relative to where the game area's size puts it.
    offset: Offset,
    /// Started from saved knowledge ([`ExpBarReader::from_knowledge`]) that hasn't read the HUD
    /// yet this session: how many frames in a row its shapes didn't fit.
    restored: Option<u32>,
    /// The HP and MP numbers' slash, learned on screen each session and never saved.
    slash: SlashLearner,
    /// What confirmed readings of the full field said of the EXP table this session, by level.
    /// The percent alone can't check the table, so it goes by these, and by the table at a level
    /// not seen in full.
    checked: Vec<RowCheck>,
}

/// What confirmed readings of the full field said of the EXP table at one level this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowCheck {
    level: u16,
    /// The table's row until the bar is shown not to fit it; `None` after that.
    needed: Option<u64>,
    /// A reading (EXP, percent) that didn't fit the row. One such reading is left out as a
    /// misread (a field partly covered can read as a smaller EXP), even the level's first; a
    /// second, different one disproves the row.
    miss: Option<(u64, u16)>,
}

impl RowCheck {
    /// Takes one more confirmed reading of the full field at this level: the row it fitted, if
    /// any, and its EXP and percent.
    fn see(&mut self, fitted: Option<u64>, at: (u64, u16)) {
        match (self.needed, fitted) {
            (_, Some(needed)) => {
                self.needed = Some(needed);
                self.miss = None;
            }
            (Some(_), None) => match self.miss {
                Some(before) if before != at => self.needed = None,
                _ => self.miss = Some(at),
            },
            (None, None) => {}
        }
    }
}

/// The first line of [`ExpBarReader::knowledge`]'s text. A reader refuses any other.
const KNOWLEDGE_HEADER: &str = "mesovibe exp-bar 1";

/// Frames in a row that saved knowledge may fail to read the HUD before the reader drops it and
/// learns again: about five seconds at 4 Hz. Only a game update that changed the HUD's shapes
/// should get there.
const RESTORE_PATIENCE: u32 = 20;

/// Self-calibration's evidence and progress.
#[derive(Debug, Clone, Default)]
struct Learner {
    /// Digit shapes seen while learning, numbered in order of first appearance.
    shapes: Vec<Glyph>,
    /// Distinct EXP fields seen, with digits as indexes into `shapes`.
    observations: Vec<Observation>,
    /// Punctuation and digits as named so far, before confirmation: the naming the HUD alone
    /// singles out, or else the one the EXP table singles out.
    provisional: Option<Atlas>,
    /// The naming the HUD alone singles out, and for how many new fields in a row it has.
    candidate: Option<(Vec<u8>, usize)>,
    /// The level tiles' glyphs in the frames the evidence came from. When they change, the level
    /// did, and EXP needed with it, so the evidence starts over.
    tiles: Vec<Glyph>,
    /// Frames of the percent alone, whose climb names the digits when the HUD hides the EXP.
    climb: Climb,
    confirmed: bool,
}

/// How many new fields in a row the HUD alone must single out the same naming before it's
/// confirmed. A margin against a wrong naming that happens to fit the evidence so far.
const CONFIRM_AFTER: usize = 3;

impl ExpBarReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn calibration(&self) -> Calibration {
        match (self.learner.confirmed, &self.learner.provisional) {
            (true, _) => Calibration::Confirmed,
            (false, Some(_)) => Calibration::Provisional,
            (false, None) => Calibration::Learning,
        }
    }

    /// Learns from a frame and reads its EXP bar: [`ExpBarReader::observe_hud`] without the HP and
    /// MP numbers.
    pub fn observe(&mut self, frame: &Frame) -> Result<ExpReading, Unreadable> {
        self.observe_hud(frame).exp
    }

    /// Learns from a frame and reads it. Call it on every frame: while EXP moves, the reader
    /// names the digit shapes itself, and later it follows level-ups and learns digits it hadn't
    /// seen.
    ///
    /// The HP and MP numbers are read in the same pass, with the EXP field's digits, once those are
    /// confirmed and checked this session, wherever the learned "EXP." label is found. They don't
    /// need the EXP field itself to read: a cursor over it leaves them readable.
    pub fn observe_hud(&mut self, frame: &Frame) -> HudReading {
        at_1x(frame, |frame, nominal| {
            let (exp, placed) = self.observe_at(frame, nominal);
            let hp_mp = match placed {
                None => Err(HpMpUnreadable::NoHud),
                Some(_) if !self.learner.confirmed => Err(HpMpUnreadable::NoHud),
                Some(_) if self.restored.is_some() => Err(HpMpUnreadable::Unchecked),
                Some(layout) => hp_mp::read(
                    frame,
                    (layout.rect(HP_TEXT), layout.rect(MP_TEXT)),
                    text_ink,
                    &self.digits,
                    &mut self.slash,
                ),
            };
            HudReading {
                exp: dark_if_hidden(exp, frame, nominal),
                hp_mp,
            }
        })
        .unwrap_or_else(|why| HudReading {
            exp: Err(why),
            hp_mp: Err(HpMpUnreadable::NoHud),
        })
    }

    /// Whether the HP and MP numbers' slash has been learned this session.
    pub fn knows_slash(&self) -> bool {
        self.slash.is_learned()
    }

    /// What the reader has confirmed, as text to keep on the player's device so the next session
    /// reads from its first frame ([`ExpBarReader::from_knowledge`]). The learned shapes never
    /// leave the device. `None` until the digits are confirmed.
    pub fn knowledge(&self) -> Option<String> {
        if !self.learner.confirmed {
            return None;
        }
        let label = self.label.as_ref()?;
        let mut text = format!("{KNOWLEDGE_HEADER}\nlabel {}\n", label.encode());
        for (name, glyph) in self.digits.entries() {
            text.push_str(&format!("exp {name} {}\n", glyph.encode()));
        }
        for (name, glyph) in self.level_digits.entries() {
            text.push_str(&format!("level {name} {}\n", glyph.encode()));
        }
        Some(text)
    }

    /// A reader that starts from [`ExpBarReader::knowledge`] saved in an earlier session, so it
    /// reads the HUD at once. Until a reading fits the EXP table (or the shapes stop fitting the
    /// HUD, after a game update), it checks itself: a reading that doesn't fit is marked
    /// unconfirmed, and if the saved label is missing from a HUD that's plainly there, or the
    /// shapes keep failing, it drops what was saved and learns again. `None` for text it couldn't
    /// have written.
    pub fn from_knowledge(text: &str) -> Option<ExpBarReader> {
        let mut lines = text.lines();
        if lines.next()? != KNOWLEDGE_HEADER {
            return None;
        }
        let mut reader = ExpBarReader::new();
        for line in lines.filter(|l| !l.is_empty()) {
            let parts: Vec<&str> = line.split(' ').collect();
            match parts[..] {
                ["label", glyph] => reader.label = Some(Glyph::decode(glyph)?),
                ["exp", name, glyph] if name.chars().count() == 1 && EXP_CHARS.contains(name) => {
                    reader.digits.learn(&[Glyph::decode(glyph)?], name).ok()?;
                }
                ["level", name, glyph] if name.len() == 1 && LEVEL_CHARS.contains(name) => {
                    reader
                        .level_digits
                        .learn(&[Glyph::decode(glyph)?], name)
                        .ok()?;
                }
                _ => return None,
            }
        }
        if reader.label.is_none() || reader.digits.is_empty() {
            return None;
        }
        reader.learner.confirmed = true;
        reader.restored = Some(0);
        Some(reader)
    }

    /// Learns from and reads the EXP bar, and says where the learned "EXP." label was matched
    /// exactly, which places the HP and MP numbers.
    fn observe_at(
        &mut self,
        frame: &Frame,
        nominal: Layout,
    ) -> (Result<ExpReading, Unreadable>, Option<Layout>) {
        let mut found = self.find_label(frame, nominal);
        if self.restored.is_some() && found.is_none() && find_by_structure(frame, nominal).is_some()
        {
            // The HUD is plainly there, but not with the label that was saved: the game's UI
            // changed since. Learn it again.
            *self = ExpBarReader::new();
            found = None;
        }
        let result = self.observe_found(frame, nominal, found);
        let placed = found.and_then(|offset| nominal.shifted(offset));
        let Some(misses) = self.restored else {
            return (result, placed);
        };
        let missed = match result {
            // The saved shapes read the HUD and the bar fits the EXP table: they're right. The
            // percent alone can't be checked against the table, but every one of its shapes
            // matched a saved one exactly. Late in a level it's all the HUD shows, for hours at
            // high levels, and the HP and MP numbers wait for this check.
            Ok(reading) if reading.needed.is_some() || !reading.exp_shown => {
                self.restored = None;
                return (Ok(reading), placed);
            }
            // Read, but not provably right: shown unconfirmed, and no events.
            Ok(reading) => Ok(ExpReading {
                confirmed: false,
                ..reading
            }),
            Err(Unreadable::UnknownGlyph(_) | Unreadable::Garbled(_)) => result,
            // Covered, dark or elsewhere: no evidence either way.
            Err(_) => return (result, placed),
        };
        if misses + 1 >= RESTORE_PATIENCE {
            *self = ExpBarReader::new();
        } else {
            self.restored = Some(misses + 1);
        }
        (missed, placed)
    }

    fn observe_found(
        &mut self,
        frame: &Frame,
        nominal: Layout,
        found: Option<Offset>,
    ) -> Result<ExpReading, Unreadable> {
        let offset = match found {
            Some(offset) => offset,
            None if !self.learner.confirmed => {
                find_by_structure(frame, nominal).unwrap_or(self.offset)
            }
            None => self.offset,
        };
        let layout = nominal.shifted(offset).ok_or(Unreadable::HudNotFound)?;
        if !self.learner.confirmed {
            self.learn(frame, layout)?;
        }
        self.offset = offset;
        let mut result = self.read_at(frame, layout);
        if result == Err(Unreadable::UnknownGlyph(Field::Exp))
            && self.learn_unseen_punctuation(frame, layout)
        {
            result = self.read_at(frame, layout);
        }
        if result == Err(Unreadable::UnknownGlyph(Field::Exp))
            && self.learn_unseen_digit(frame, layout)
        {
            result = self.read_at(frame, layout);
        }
        let mut reading = result?;
        self.follow_level(frame, layout, &mut reading);
        estimate(&mut reading, self.last_exp.as_ref());
        if reading.exp.is_some() {
            self.last_exp = Some(reading);
        }
        self.last = Some(reading);
        Ok(reading)
    }

    /// For when the game's picture changed size or scale: the player switched between windowed
    /// and Full Screen, or picked another size, and the HUD's shapes may come out differently.
    /// Shapes confirmed before are checked again the way saved ones are at the start of a session:
    /// readings with them are unconfirmed until one fits the EXP table, and they're dropped and
    /// learned again when the HUD is plainly there without the learned label, or when they keep
    /// failing. Shapes still being learned start over, and the HP and MP numbers' slash is learned
    /// again.
    pub fn recheck(&mut self) {
        match self.learner.confirmed {
            true => {
                self.restored = self.restored.or(Some(0));
                self.slash = SlashLearner::default();
            }
            false => *self = ExpBarReader::new(),
        }
    }

    /// Reads a frame with what's been learned so far, learning nothing.
    pub fn read(&self, frame: &Frame) -> Result<ExpReading, Unreadable> {
        at_1x(frame, |frame, nominal| {
            let offset = self.find_label(frame, nominal).unwrap_or(self.offset);
            let result = match nominal.shifted(offset) {
                Some(layout) => self.read_at(frame, layout).map(|mut reading| {
                    estimate(&mut reading, self.last_exp.as_ref());
                    reading
                }),
                None => Err(Unreadable::HudNotFound),
            };
            dark_if_hidden(result, frame, nominal)
        })
        .and_then(|r| r)
    }

    /// Where the learned "EXP." label is, matched exactly, within a pixel of where the game
    /// area's size puts it. The last place it was is tried first. `None` when no label is learned,
    /// when it isn't within a pixel (covered, or the UI changed), or when it fits more than one
    /// place.
    fn find_label(&self, frame: &Frame, nominal: Layout) -> Option<Offset> {
        let label = self.label.as_ref()?;
        let at = |offset: Offset| {
            nominal
                .shifted(offset)
                .is_some_and(|l| cut(frame, l.rect(LABEL), text_ink).as_ref() == Ok(label))
        };
        if at(self.offset) {
            return Some(self.offset);
        }
        let mut found = OFFSETS.into_iter().filter(|&o| at(o));
        match (found.next(), found.next()) {
            (Some(offset), None) => Some(offset),
            _ => None,
        }
    }

    /// The fallback: learns glyph shapes from a frame whose HUD showed `reading`, as the player
    /// typed it. Confirms the reader.
    pub fn calibrate(
        &mut self,
        frame: &Frame,
        reading: TypedReading,
    ) -> Result<(), CalibrationError> {
        if reading.level == 0 || reading.pct_hundredths > 10_000 {
            return Err(CalibrationError::ImplausibleReading);
        }
        at_1x(frame, |frame, nominal| {
            self.calibrate_at(frame, nominal, reading)
        })
        .unwrap_or_else(|why| {
            Err(match why {
                Unreadable::HudNotFound => CalibrationError::HudNotFound,
                _ => CalibrationError::UnsupportedSize {
                    width: frame.width() / frame.scale(),
                    height: frame.height() / frame.scale(),
                },
            })
        })
    }

    fn calibrate_at(
        &mut self,
        frame: &Frame,
        nominal: Layout,
        reading: TypedReading,
    ) -> Result<(), CalibrationError> {
        let offset = self
            .find_label(frame, nominal)
            .or_else(|| find_by_structure(frame, nominal))
            .unwrap_or(self.offset);
        let layout = nominal
            .shifted(offset)
            .ok_or(CalibrationError::HudNotFound)?;
        let label =
            cut(frame, layout.rect(LABEL), text_ink).map_err(|_| CalibrationError::HudNotFound)?;
        if label.is_blank() {
            return Err(CalibrationError::HudNotFound);
        }
        if self.learner.confirmed && self.label.as_ref().is_some_and(|known| *known != label) {
            return Err(CalibrationError::HudChanged);
        }
        let exp_glyphs = glyphs(frame, layout.rect(EXP_TEXT), text_ink)
            .ok_or(CalibrationError::NoText(Field::Exp))?;
        let level_glyphs = glyphs(frame, layout.rect(LEVEL_TEXT), level_ink)
            .ok_or(CalibrationError::NoText(Field::Level))?;

        // Start from what's confirmed; anything self-calibration had only guessed is dropped.
        let base = if self.learner.confirmed {
            self.digits.clone()
        } else {
            Atlas::default()
        };
        let mut digits = base;
        digits
            .learn(&exp_glyphs, &exp_text(reading.exp, reading.pct_hundredths))
            .map_err(|e| CalibrationError::Mismatch(Field::Exp, e))?;
        let mut level_digits = self.level_digits.clone();
        level_digits
            .learn(&level_glyphs, &reading.level.to_string())
            .map_err(|e| CalibrationError::Mismatch(Field::Level, e))?;

        self.label = Some(label);
        self.digits = digits;
        self.level_digits = level_digits;
        self.offset = offset;
        self.learner = Learner {
            confirmed: true,
            ..Learner::default()
        };
        self.restored = None;
        Ok(())
    }

    /// The glyphs not seen yet. Frames that only use seen glyphs read fine before this is empty.
    /// Level digits arrive with level-ups, so only the EXP field is expected to fill up early.
    pub fn missing(&self) -> Missing {
        Missing {
            exp: self.digits.missing(EXP_CHARS),
            level: self.level_digits.missing(LEVEL_CHARS),
        }
    }

    /// Adds a frame's EXP field to the evidence and re-solves the digits' names.
    fn learn(&mut self, frame: &Frame, layout: Layout) -> Result<(), Unreadable> {
        let label =
            cut(frame, layout.rect(LABEL), text_ink).map_err(|_| Unreadable::HudNotFound)?;
        if label.is_blank() {
            return Err(Unreadable::HudNotFound);
        }
        let parts = structure(frame, layout.rect(EXP_TEXT))?;
        match &self.label {
            Some(known) if *known != label => {
                // A different HUD before learning got anywhere: start over with it. Once there's
                // evidence, a different label is something drawn over the HUD.
                if self.learner.observations.len() >= 2 || self.learner.climb.stepped() {
                    return Err(Unreadable::HudNotFound);
                }
                self.learner = Learner::default();
                self.digits = Atlas::default();
                self.label = Some(label);
            }
            Some(_) => {}
            None => self.label = Some(label),
        }
        let tiles = glyphs(frame, layout.rect(LEVEL_TEXT), level_ink).unwrap_or_default();
        let evidence = !self.learner.observations.is_empty() || self.learner.climb.started();
        if evidence && self.learner.tiles != tiles {
            // The level changed, and EXP needed with it: start the evidence over. The percent
            // fell to the start of the new level, too.
            self.keep_newest(0);
            self.learner.provisional = None;
            self.learner.candidate = None;
            self.learner.climb = Climb::default();
        }
        self.learner.tiles = tiles;

        let garbled = Unreadable::Garbled(Field::Exp);
        let (marks, names): (Vec<Glyph>, String) = parts.punctuation.into_iter().unzip();
        let mut digits = self.digits.clone();
        digits.learn(&marks, &names).map_err(|_| garbled)?;
        if parts.exp.is_empty() {
            // The percent alone: without the EXP, nothing ties its digits to EXP needed, but the
            // order they climb in names them ([`Climb`]). The full field's evidence stays.
            if parts
                .whole
                .iter()
                .chain(&parts.frac)
                .any(|g| digits.name(g).is_some())
            {
                return Err(garbled);
            }
            self.digits = digits;
            if let Some(named) = self.learner.climb.add(&parts.whole, &parts.frac)? {
                let mut digits = self.digits.clone();
                if digits.learn(&self.learner.climb.shapes, &named).is_ok() {
                    self.digits = digits;
                    self.learner = Learner {
                        confirmed: true,
                        ..Learner::default()
                    };
                }
            }
            return Ok(());
        }
        let mut shapes = self.learner.shapes.clone();
        let mut number = |glyph: &Glyph| -> Result<usize, Unreadable> {
            if digits.name(glyph).is_some() {
                return Err(garbled);
            }
            if let Some(i) = shapes.iter().position(|s| s == glyph) {
                return Ok(i);
            }
            if shapes.len() == DIGITS.len() {
                return Err(garbled);
            }
            shapes.push(glyph.clone());
            Ok(shapes.len() - 1)
        };
        let observation = Observation {
            exp: parts
                .exp
                .iter()
                .map(&mut number)
                .collect::<Result<_, _>>()?,
            whole: parts
                .whole
                .iter()
                .map(&mut number)
                .collect::<Result<_, _>>()?,
            frac: parts
                .frac
                .iter()
                .map(&mut number)
                .collect::<Result<_, _>>()?,
        };
        self.digits = digits;
        if self.learner.observations.contains(&observation) {
            return Ok(());
        }
        self.learner.shapes = shapes;
        self.learner.observations.push(observation);
        if self.learner.observations.len() > MAX_OBSERVATIONS {
            self.keep_newest(MAX_OBSERVATIONS / 2);
        }
        self.solve();
        Ok(())
    }

    fn solve(&mut self) {
        let (observations, shapes) = (&self.learner.observations, self.learner.shapes.len());
        let alone = solve(observations, shapes, false, 2);
        match &alone[..] {
            // No naming fits every field: some field doesn't belong (EXP needed changed, or a
            // frame was misleading). Keep the newest field and carry on.
            [] => {
                self.keep_newest(1);
                self.learner.provisional = None;
                self.learner.candidate = None;
            }
            [only] => {
                // A new shape lengthens the naming; the shapes named before keep their digits.
                let streak = match &self.learner.candidate {
                    Some((before, n)) if only.starts_with(before) => n + 1,
                    _ => 1,
                };
                let mut digits = self.digits.clone();
                if digits.learn(&self.learner.shapes, &names(only)).is_err() {
                    return;
                }
                if streak >= CONFIRM_AFTER {
                    self.digits = digits;
                    self.learner = Learner {
                        confirmed: true,
                        ..Learner::default()
                    };
                } else {
                    self.learner.candidate = Some((only.clone(), streak));
                    self.learner.provisional = Some(digits);
                }
            }
            _ => {
                self.learner.candidate = None;
                let with_table = solve(observations, shapes, true, 2);
                self.learner.provisional = match &with_table[..] {
                    [only] => {
                        let mut digits = self.digits.clone();
                        digits
                            .learn(&self.learner.shapes, &names(only))
                            .ok()
                            .map(|_| digits)
                    }
                    _ => None,
                };
            }
        }
    }

    /// Drops all but the newest `keep` observations and renumbers the shapes they use.
    fn keep_newest(&mut self, keep: usize) {
        let learner = &mut self.learner;
        let start = learner.observations.len().saturating_sub(keep);
        let kept: Vec<Observation> = learner.observations.drain(start..).collect();
        let old = std::mem::take(&mut learner.shapes);
        let mut renumber = |i: &usize| -> usize {
            let glyph = &old[*i];
            match learner.shapes.iter().position(|s| s == glyph) {
                Some(n) => n,
                None => {
                    learner.shapes.push(glyph.clone());
                    learner.shapes.len() - 1
                }
            }
        };
        learner.observations = kept
            .iter()
            .map(|o| Observation {
                exp: o.exp.iter().map(&mut renumber).collect(),
                whole: o.whole.iter().map(&mut renumber).collect(),
                frac: o.frac.iter().map(&mut renumber).collect(),
            })
            .collect();
    }

    fn read_at(&self, frame: &Frame, layout: Layout) -> Result<ExpReading, Unreadable> {
        let atlas = match (self.learner.confirmed, &self.learner.provisional) {
            (true, _) => &self.digits,
            (false, Some(provisional)) => provisional,
            (false, None) if self.label.is_some() => return Err(Unreadable::Learning),
            (false, None) => return Err(Unreadable::NotCalibrated),
        };
        let label = self.label.as_ref().ok_or(Unreadable::NotCalibrated)?;
        if cut(frame, layout.rect(LABEL), text_ink).as_ref() != Ok(label) {
            return Err(Unreadable::HudNotFound);
        }
        let text = read_field(frame, layout.rect(EXP_TEXT), text_ink, atlas, Field::Exp)?;
        let (exp, pct_hundredths) = parse_exp_text(&text).ok_or(Unreadable::Garbled(Field::Exp))?;
        // Without the EXP, only the tiles can tell the level.
        let level = read_level(frame, layout, &self.level_digits)
            .or_else(|| exp.and_then(|exp| table_level(exp, pct_hundredths)));
        Ok(ExpReading {
            level,
            exp,
            exp_shown: exp.is_some(),
            pct_hundredths,
            needed: level.and_then(|l| table_needed(l, exp, pct_hundredths)),
            confirmed: self.learner.confirmed,
        })
    }

    /// Learns punctuation that turned up after calibration, named by the field's structure as
    /// while learning: a reader that learned its digits from the percent alone ([`Climb`]) has
    /// never seen a separator.
    fn learn_unseen_punctuation(&mut self, frame: &Frame, layout: Layout) -> bool {
        if !self.learner.confirmed {
            return false;
        }
        let Ok(parts) = structure(frame, layout.rect(EXP_TEXT)) else {
            return false;
        };
        let (marks, names): (Vec<Glyph>, String) = parts
            .punctuation
            .into_iter()
            .filter(|(glyph, _)| self.digits.name(glyph).is_none())
            .unzip();
        let mut atlas = self.digits.clone();
        if marks.is_empty() || atlas.learn(&marks, &names).is_err() {
            return false;
        }
        self.digits = atlas;
        true
    }

    /// Learns a digit shape that turned up after calibration, when exactly one digit makes the
    /// bar fit the EXP needed of the last reading without the EXP falling, and that digit is
    /// unnamed. Every digit is tried, named or not: in a low place the bar fits several (it allows
    /// about a hundredth of a percent of EXP needed), and there a named digit with something drawn
    /// over it (the game's cursor, a stray pixel) could pass for the missing one. The percent
    /// alone never teaches one: nothing in it can tell a digit from something drawn over one, so a
    /// shape first seen there waits for the full field.
    fn learn_unseen_digit(&mut self, frame: &Frame, layout: Layout) -> bool {
        let Some(last) = self.last.filter(|_| self.learner.confirmed) else {
            return false;
        };
        let Some(needed) = last.needed else {
            return false;
        };
        let Ok(found) = segment(frame, layout.rect(EXP_TEXT), text_ink) else {
            return false;
        };
        let mut unknown: Vec<&Glyph> = Vec::new();
        for (_, g) in &found {
            if self.digits.name(g).is_none() && !unknown.contains(&g) {
                unknown.push(g);
            }
        }
        let [glyph] = unknown[..] else {
            return false;
        };
        let fitting: Vec<char> = DIGITS
            .chars()
            .filter(|&digit| {
                let text: Option<String> = found
                    .iter()
                    .map(|(_, g)| match g == glyph {
                        true => Some(digit),
                        false => self.digits.name(g),
                    })
                    .collect();
                matches!(
                    text.as_deref().and_then(parse_exp_text),
                    Some((Some(exp), pct))
                        if last.exp.is_some_and(|before| exp >= before) && fits(exp, pct, needed)
                )
            })
            .collect();
        let [digit] = fitting[..] else {
            return false;
        };
        let mut atlas = self.digits.clone();
        if atlas
            .learn(std::slice::from_ref(glyph), &digit.to_string())
            .is_err()
        {
            // The digit is named already: this is its shape with something drawn over it.
            return false;
        }
        self.digits = atlas;
        true
    }

    /// Learns the level tiles' digits and follows level-ups. Unconfirmed readings teach nothing.
    fn follow_level(&mut self, frame: &Frame, layout: Layout, reading: &mut ExpReading) {
        let tiles = glyphs(frame, layout.rect(LEVEL_TEXT), level_ink);
        let tiles_changed = self.last_tiles.is_some() && self.last_tiles != tiles;
        if reading.confirmed && read_level(frame, layout, &self.level_digits).is_none() {
            let lesson = match self.last {
                // A level-up: the bar fell and the tiles changed (at a death they don't).
                Some(
                    last @ ExpReading {
                        level: Some(level), ..
                    },
                ) if fell(&last, reading) && tiles_changed => level.checked_add(1),
                // Otherwise the table's level, when its EXP needed fits the bar.
                _ => reading.level.filter(|_| reading.needed.is_some()),
            };
            if let (Some(tiles), Some(level)) = (&tiles, lesson) {
                let mut atlas = self.level_digits.clone();
                if atlas.learn(tiles, &level.to_string()).is_ok() {
                    self.level_digits = atlas;
                    reading.level = Some(level);
                    reading.needed = table_needed(level, reading.exp, reading.pct_hundredths);
                }
            }
        }
        // Still no level: keep the last one while the tiles are unchanged and the bar fits its
        // EXP needed.
        if reading.level.is_none()
            && !tiles_changed
            && let Some(last) = self.last
            && let (Some(level), Some(needed)) = (last.level, last.needed)
            && bar_fits(reading.exp, reading.pct_hundredths, needed)
        {
            reading.level = Some(level);
            reading.needed = Some(needed);
        }
        // The full field checks the table's EXP needed for its level; the percent alone goes by
        // that check, so a row the bar didn't fit stays unused whatever the tiles did between.
        if let Some(level) = reading.level {
            let seen = self.checked.iter().position(|c| c.level == level);
            let at = (reading.exp.unwrap_or_default(), reading.pct_hundredths);
            match (reading.exp_shown && reading.confirmed, seen) {
                (true, Some(i)) => self.checked[i].see(reading.needed, at),
                // A level seen for the first time starts from the table's row, so a first reading
                // that doesn't fit it is one misread, not yet proof.
                (true, None) => {
                    let mut check = RowCheck {
                        level,
                        needed: exp_table::needed(level),
                        miss: None,
                    };
                    check.see(reading.needed, at);
                    self.checked.push(check);
                }
                (false, Some(i)) if !reading.exp_shown => {
                    reading.needed = self.checked[i].needed;
                }
                (false, _) => {}
            }
        }
        self.last_tiles = tiles;
    }
}

/// Where the HUD is when no label is learned yet: the one row, within a pixel of where the game
/// area's size puts it, at which the EXP field makes sense. A row off cuts the text's top or
/// bottom row, which garbles the field. The column isn't searched: the field reads the same a
/// column either way. `None` unless exactly one row fits.
fn find_by_structure(frame: &Frame, nominal: Layout) -> Option<Offset> {
    let max = MAX_SHIFT as i32;
    let fits = |dy: i32| {
        nominal.shifted((0, dy)).is_some_and(|l| {
            cut(frame, l.rect(LABEL), text_ink).is_ok_and(|g| !g.is_blank())
                && structure(frame, l.rect(EXP_TEXT)).is_ok()
        })
    };
    let mut found = (-max..=max).filter(|&dy| fits(dy));
    match (found.next(), found.next()) {
        (Some(dy), None) => Some((0, dy)),
        _ => None,
    }
}

/// The HUD's text is pure white whenever the game draws it, so a HUD that can't be found with
/// nothing near white in its rows is a dimmed screen. 250, not 255: the stretch recovery at large
/// sizes can land a point or two below white.
const WHITE_MIN: u8 = 250;

/// `ScreenDark` in place of `HudNotFound` when nothing in the HUD's rows, across the whole bar,
/// is white. A HUD covered by a window or a tooltip still has white around it, and stays
/// `HudNotFound`.
fn dark_if_hidden(
    result: Result<ExpReading, Unreadable>,
    frame: &Frame,
    layout: Layout,
) -> Result<ExpReading, Unreadable> {
    if result != Err(Unreadable::HudNotFound) {
        return result;
    }
    let top = layout
        .bottom
        .saturating_sub(HUD_ROWS)
        .max(frame.first_row());
    let bottom = layout.bottom.min(frame.height());
    let right = (layout.hud_left + HUD_WIDTH).min(frame.width());
    let lit = (top..bottom).any(|y| {
        (layout.hud_left..right)
            .any(|x| frame.rgb(x, y).into_iter().max().unwrap_or(0) >= WHITE_MIN)
    });
    match lit {
        true => result,
        false => Err(Unreadable::ScreenDark),
    }
}

fn unsupported(frame: &Frame) -> Unreadable {
    Unreadable::UnsupportedSize {
        width: frame.width(),
        height: frame.height(),
    }
}

/// The glyphs in a region, or `None` when it holds no text or something other than text.
fn glyphs(frame: &Frame, rect: Rect, ink: fn([u8; 3]) -> bool) -> Option<Vec<Glyph>> {
    let found = segment(frame, rect, ink).ok()?;
    (!found.is_empty()).then(|| found.into_iter().map(|(_, g)| g).collect())
}

fn read_field(
    frame: &Frame,
    rect: Rect,
    ink: fn([u8; 3]) -> bool,
    atlas: &Atlas,
    field: Field,
) -> Result<String, Unreadable> {
    let found = segment(frame, rect, ink).map_err(|_| Unreadable::Garbled(field))?;
    if found.is_empty() {
        return Err(Unreadable::NoText(field));
    }
    found
        .iter()
        .map(|(_, glyph)| atlas.name(glyph).ok_or(Unreadable::UnknownGlyph(field)))
        .collect()
}

/// The level from the tiles, when every tile digit is known.
fn read_level(frame: &Frame, layout: Layout, atlas: &Atlas) -> Option<u16> {
    read_field(
        frame,
        layout.rect(LEVEL_TEXT),
        level_ink,
        atlas,
        Field::Level,
    )
    .ok()
    .and_then(|text| parse_level(&text))
}

/// The one table level whose EXP needed fits the bar, if exactly one does.
fn table_level(exp: u64, pct_hundredths: u16) -> Option<u16> {
    let (lo, hi) = needed_range(exp, pct_hundredths)?;
    let mut levels = exp_table::levels_within(lo, hi);
    match (levels.next(), levels.next()) {
        (Some(level), None) => Some(level),
        _ => None,
    }
}

/// The table's EXP needed for `level`, when it fits the bar: the EXP and percent shown, or the
/// percent alone when the HUD hides the EXP.
fn table_needed(level: u16, exp: Option<u64>, pct_hundredths: u16) -> Option<u64> {
    exp_table::needed(level).filter(|&n| bar_fits(exp, pct_hundredths, n))
}

/// Whether a bar showing `exp` (`None` when the HUD hides it) at `pct_hundredths` fits a level
/// needing `needed`. The percent alone fits whenever some EXP below `needed` shows it.
fn bar_fits(exp: Option<u64>, pct_hundredths: u16, needed: u64) -> bool {
    match exp {
        Some(exp) => fits(exp, pct_hundredths, needed),
        None => exp_range(pct_hundredths, needed).is_some(),
    }
}

/// The EXP values a bar showing `pct_hundredths` allows at a level needing `needed`, lowest and
/// highest: [`needed_range`]'s rule turned around, (2 x pct - 1) x needed <= 20000 x exp <
/// (2 x pct + 1) x needed, with EXP below what the level needs. `None` when no value fits.
pub fn exp_range(pct_hundredths: u16, needed: u64) -> Option<(u64, u64)> {
    let p = u64::from(pct_hundredths);
    let lo = match p {
        0 => 0,
        _ => (2 * p - 1).checked_mul(needed)?.div_ceil(20_000),
    };
    let hi = (2 * p + 1)
        .checked_mul(needed)?
        .div_ceil(20_000)
        .checked_sub(1)?
        .min(needed.checked_sub(1)?);
    (lo <= hi).then_some((lo, hi))
}

/// Whether the bar went down from `before` to `now`: the EXP when both have one, else the
/// percent.
fn fell(before: &ExpReading, now: &ExpReading) -> bool {
    match (before.exp, now.exp) {
        (Some(before), Some(now)) => now < before,
        _ => now.pct_hundredths < before.pct_hundredths,
    }
}

/// Works out the EXP of a reading whose HUD showed only the percent, when EXP needed is known:
/// of the values the percent allows, the one nearest the last EXP at the same level and EXP
/// needed, else the middle one. So the EXP holds while the percent does, carries on from the
/// last EXP shown, and moves only as far as a new percent makes it.
fn estimate(reading: &mut ExpReading, last: Option<&ExpReading>) {
    if reading.exp_shown {
        return;
    }
    let Some((lo, hi)) = reading
        .needed
        .and_then(|needed| exp_range(reading.pct_hundredths, needed))
    else {
        return;
    };
    let before = last
        .filter(|last| last.level == reading.level && last.needed == reading.needed)
        .and_then(|last| last.exp);
    reading.exp = Some(match before {
        Some(exp) => exp.clamp(lo, hi),
        None => lo + (hi - lo) / 2,
    });
}

/// The EXP needed values a bar showing `exp` at `pct_hundredths` allows. The bar shows
/// round(exp x 10000 / needed), so (2 x pct - 1) x needed <= 20000 x exp < (2 x pct + 1) x needed,
/// and EXP into a level is always below what the level needs. `None` when no value fits.
pub fn needed_range(exp: u64, pct_hundredths: u16) -> Option<(u64, u64)> {
    let scaled = exp.checked_mul(20_000)?;
    let p = u64::from(pct_hundredths);
    let lo = (scaled / (2 * p + 1) + 1).max(exp + 1);
    let hi = match p {
        0 => u64::MAX,
        _ => scaled / (2 * p - 1),
    };
    (lo <= hi).then_some((lo, hi))
}

/// Whether a bar showing `exp` at `pct_hundredths` fits a level needing `needed`.
fn fits(exp: u64, pct_hundredths: u16, needed: u64) -> bool {
    needed_range(exp, pct_hundredths).is_some_and(|(lo, hi)| (lo..=hi).contains(&needed))
}

/// An EXP field cut into its parts by color and position, before any digit is named.
struct Structure {
    /// The brackets, percent sign, decimal point and separators, with their names.
    punctuation: Vec<(Glyph, char)>,
    exp: Vec<Glyph>,
    whole: Vec<Glyph>,
    frac: Vec<Glyph>,
}

/// Names the EXP field's punctuation without knowing any digit: the brackets are the only
/// yellow-green glyphs and close the field, the percent sign sits before the closing bracket,
/// the decimal point is the percent's only bottom-row dot, and separators are the bottom-row
/// marks in the EXP, one every three digits from the right. A field that opens with the bracket
/// is the percent alone, with no EXP.
fn structure(frame: &Frame, rect: Rect) -> Result<Structure, Unreadable> {
    let garbled = Unreadable::Garbled(Field::Exp);
    let found = segment(frame, rect, text_ink).map_err(|_| garbled)?;
    if found.is_empty() {
        return Err(Unreadable::NoText(Field::Exp));
    }
    let greens: Vec<usize> = (0..found.len())
        .filter(|&i| is_green(frame, rect, &found[i]))
        .collect();
    let [open, close] = greens[..] else {
        return Err(garbled);
    };
    // The EXP, unless the HUD hides it, then `[d.dd%]`.
    if close != found.len() - 1 || close < open + 6 {
        return Err(garbled);
    }
    let glyph = |i: usize| found[i].1.clone();
    let mut punctuation = vec![
        (glyph(open), '['),
        (glyph(close), ']'),
        (glyph(close - 1), '%'),
    ];

    let mut exp = Vec::new();
    let mut pattern = String::new();
    for i in 0..open {
        let g = glyph(i);
        if g.ink_rows() & !COMMA_ROWS == 0 {
            pattern.push(',');
            punctuation.push((g, ','));
        } else {
            pattern.push('0');
            exp.push(g);
        }
    }
    // Separators alone, with no digit, aren't an EXP.
    if pattern != group_digits(&"0".repeat(exp.len())) {
        return Err(garbled);
    }

    let pct: Vec<Glyph> = (open + 1..close - 1).map(glyph).collect();
    let dots: Vec<usize> = (0..pct.len())
        .filter(|&i| pct[i].ink_rows() & !DOT_ROWS == 0)
        .collect();
    let [dot] = dots[..] else {
        return Err(garbled);
    };
    if !(1..=3).contains(&dot) || pct.len() != dot + 3 {
        return Err(garbled);
    }
    punctuation.push((pct[dot].clone(), '.'));
    Ok(Structure {
        punctuation,
        exp,
        whole: pct[..dot].to_vec(),
        frac: pct[dot + 1..].to_vec(),
    })
}

/// Whether most of a glyph's ink is yellow-green.
fn is_green(frame: &Frame, rect: Rect, (x, glyph): &(u32, Glyph)) -> bool {
    let (mut green, mut all) = (0, 0);
    for dy in 0..glyph.height() {
        for dx in 0..glyph.width() {
            if glyph.is_ink(dx, dy) {
                all += 1;
                if green_ink(frame.rgb(x + dx, rect.y + dy)) {
                    green += 1;
                }
            }
        }
    }
    green * 2 > all
}

/// One EXP field's digits, as indexes into the learner's shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observation {
    exp: Vec<usize>,
    whole: Vec<usize>,
    frac: Vec<usize>,
}

impl Observation {
    fn shapes(&self) -> impl Iterator<Item = usize> + '_ {
        self.exp
            .iter()
            .chain(&self.whole)
            .chain(&self.frac)
            .copied()
    }

    /// The EXP needed values this field allows when shape `i` is digit `digits[i]`, or `None`
    /// when that naming makes the field impossible.
    fn range(&self, digits: &[u8]) -> Option<(u64, u64)> {
        let number = |idx: &[usize]| {
            idx.iter().try_fold(0u64, |v, &i| {
                v.checked_mul(10)?.checked_add(u64::from(digits[i]))
            })
        };
        let exp = number(&self.exp)?;
        let pct = number(&self.whole)?
            .checked_mul(100)?
            .checked_add(number(&self.frac)?)?;
        needed_range(exp, u16::try_from(pct).ok().filter(|&p| p <= 10_000)?)
    }
}

/// Up to `limit` ways to name `shapes` digit shapes with different digits, under which every
/// observation's EXP and percent agree on one EXP needed (and, with `use_table`, that EXP needed
/// is one of the table's values). Shapes are numbered in order of first appearance, so each
/// observation is checked as soon as its last shape is named, which prunes early.
fn solve(
    observations: &[Observation],
    shapes: usize,
    use_table: bool,
    limit: usize,
) -> Vec<Vec<u8>> {
    let mut complete_at = vec![Vec::new(); shapes];
    let mut no_zero = vec![false; shapes];
    for (i, o) in observations.iter().enumerate() {
        if let Some(last) = o.shapes().max() {
            complete_at[last].push(i);
        }
        // No leading zeros.
        if o.exp.len() > 1 {
            no_zero[o.exp[0]] = true;
        }
        if o.whole.len() > 1 {
            no_zero[o.whole[0]] = true;
        }
    }
    let mut search = Search {
        observations,
        complete_at,
        no_zero,
        use_table,
        limit,
        digits: vec![0; shapes],
        used: [false; 10],
        found: Vec::new(),
    };
    search.step(0, (1, u64::MAX), false);
    search.found
}

struct Search<'a> {
    observations: &'a [Observation],
    /// The observations whose last shape is shape `i`.
    complete_at: Vec<Vec<usize>>,
    no_zero: Vec<bool>,
    use_table: bool,
    limit: usize,
    digits: Vec<u8>,
    used: [bool; 10],
    found: Vec<Vec<u8>>,
}

impl Search<'_> {
    fn step(&mut self, depth: usize, range: (u64, u64), checked: bool) {
        if depth == self.digits.len() {
            self.found.push(self.digits.clone());
            return;
        }
        for digit in 0..10u8 {
            if self.used[usize::from(digit)] || (digit == 0 && self.no_zero[depth]) {
                continue;
            }
            self.digits[depth] = digit;
            let mut next = Some(range);
            for &o in &self.complete_at[depth] {
                next = next.and_then(|(lo, hi)| {
                    let (a, b) = self.observations[o].range(&self.digits)?;
                    let (lo, hi) = (lo.max(a), hi.min(b));
                    (lo <= hi).then_some((lo, hi))
                });
            }
            let checked = checked || !self.complete_at[depth].is_empty();
            if let Some((lo, hi)) = next
                && !(self.use_table && checked && !exp_table::any_within(lo, hi))
            {
                self.used[usize::from(digit)] = true;
                self.step(depth + 1, (lo, hi), checked);
                self.used[usize::from(digit)] = false;
            }
            if self.found.len() >= self.limit {
                return;
            }
        }
    }
}

/// Frames of the percent alone, while learning: how a reader names the digits when the HUD
/// hides the EXP. Nothing ties a digit to a value there, but the percent only climbs while the
/// player grinds. Within one whole percent, a step up shows which of two shapes is the lower, in
/// the first place they differ: from `[92.61%]` to `[92.66%]`, the 1's shape is below the 6's.
/// Once those steps put all ten shapes in a single row, the row is 0 to 9.
///
/// A death makes the percent fall, and a fall reads as a rise, wrongly putting one shape below
/// another. Across whole percents a fall can't be told from a rise at all, so those steps count
/// for nothing. Within one, a fall would have to cost under 1%, and even then it shows only once:
/// every step between neighbours in the row must be seen twice before the row names anything.
/// A wrong step with the right row would make a loop, which starts the climb over from the
/// newest field. So no single fall can name a digit wrong, and the row must then hold for
/// [`CONFIRM_AFTER`] new fields.
///
/// Every neighbouring step seen twice takes one or two whole percents: the tenths step through
/// each digit once per whole percent, and gains of a hundredth or so add steps in the last
/// digit. A recorded level 58 run climbed 1% in about five minutes.
#[derive(Debug, Clone, Default)]
struct Climb {
    /// Digit shapes seen, numbered in order of first appearance.
    shapes: Vec<Glyph>,
    /// The last field, as indexes into `shapes`.
    last: Option<PctField>,
    /// `below[a][b]`: how many steps showed shape `a` below shape `b`.
    below: [[u8; 10]; 10],
    /// The naming the steps single out, and for how many new fields in a row they have.
    candidate: Option<(Vec<u8>, usize)>,
}

/// The percent alone's digits, as indexes into [`Climb::shapes`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct PctField {
    whole: Vec<usize>,
    frac: Vec<usize>,
}

/// What a climb's steps say about the order of its shapes.
enum Order {
    /// They make a loop: one of them was a fall.
    Loop,
    /// Not yet one row of all ten shapes, each step between neighbours seen twice.
    Open,
    /// Shape `i` is digit `digits[i]`.
    Named(Vec<u8>),
}

impl Climb {
    /// Whether a field has been added since the climb started.
    fn started(&self) -> bool {
        self.last.is_some()
    }

    /// Whether any step has been seen.
    fn stepped(&self) -> bool {
        self.below.iter().flatten().any(|&n| n > 0)
    }

    /// Adds a field of the percent alone: the glyphs of its whole percent and of its two
    /// decimals. Once the climb names the digits and a few new fields agree, the names of
    /// [`Climb::shapes`] in order. A field with an eleventh shape means one of the ten held was
    /// something drawn over a digit (the game's cursor): the climb starts over from that field.
    fn add(&mut self, whole: &[Glyph], frac: &[Glyph]) -> Result<Option<String>, Unreadable> {
        let mut shapes = self.shapes.clone();
        let field = match number_field(&mut shapes, whole, frac) {
            Some(field) => field,
            None => {
                *self = Climb::default();
                shapes.clear();
                number_field(&mut shapes, whole, frac).ok_or(Unreadable::Garbled(Field::Exp))?
            }
        };
        if self.last.as_ref() == Some(&field) {
            return Ok(None);
        }
        self.shapes = shapes;
        if let Some((lo, hi)) = self
            .last
            .as_ref()
            .and_then(|before| step_up(before, &field))
        {
            self.below[lo][hi] = self.below[lo][hi].saturating_add(1);
        }
        self.last = Some(field);
        let digits = match self.order() {
            Order::Named(digits) if self.spells_a_percent(&digits) => digits,
            Order::Open => {
                self.candidate = None;
                return Ok(None);
            }
            Order::Named(_) | Order::Loop => {
                self.restart();
                return Ok(None);
            }
        };
        let streak = match &self.candidate {
            Some((before, n)) if *before == digits => n + 1,
            _ => 1,
        };
        if streak >= CONFIRM_AFTER {
            return Ok(Some(names(&digits)));
        }
        self.candidate = Some((digits, streak));
        Ok(None)
    }

    /// Puts the shapes in a row, lowest first, by the steps seen: each next shape is the one
    /// nothing unplaced is below.
    fn order(&self) -> Order {
        let n = self.shapes.len();
        // How many unplaced shapes a step showed below each shape.
        let mut under = [0u8; 10];
        for above in &self.below[..n] {
            for (count, &steps) in under.iter_mut().zip(&above[..n]) {
                *count += u8::from(steps > 0);
            }
        }
        let mut placed = [false; 10];
        let mut row = Vec::with_capacity(n);
        let mut single = n == DIGITS.len();
        while row.len() < n {
            let lowest: Vec<usize> = (0..n).filter(|&s| !placed[s] && under[s] == 0).collect();
            let Some(&shape) = lowest.first() else {
                return Order::Loop;
            };
            single &= lowest.len() == 1;
            placed[shape] = true;
            row.push(shape);
            for (count, &steps) in under.iter_mut().zip(&self.below[shape][..n]) {
                *count -= u8::from(steps > 0);
            }
        }
        if !single || row.windows(2).any(|w| self.below[w[0]][w[1]] < 2) {
            return Order::Open;
        }
        let mut digits = vec![0; n];
        for (digit, &shape) in (0u8..).zip(&row) {
            digits[shape] = digit;
        }
        Order::Named(digits)
    }

    /// Whether the last field, named by `digits`, is a percent the HUD could show: no leading
    /// zero, and no more than 100.00%.
    fn spells_a_percent(&self, digits: &[u8]) -> bool {
        let Some(field) = &self.last else {
            return false;
        };
        let spell = |idx: &[usize]| names(&idx.iter().map(|&i| digits[i]).collect::<Vec<u8>>());
        let text = format!("[{}.{}%]", spell(&field.whole), spell(&field.frac));
        parse_exp_text(&text).is_some()
    }

    /// Starts over from the newest field, keeping only the shapes it shows.
    fn restart(&mut self) {
        let Some(last) = self.last.take() else {
            *self = Climb::default();
            return;
        };
        let old = std::mem::take(&mut self.shapes);
        let mut shapes: Vec<Glyph> = Vec::new();
        let mut renumber = |i: &usize| match shapes.iter().position(|s| *s == old[*i]) {
            Some(n) => n,
            None => {
                shapes.push(old[*i].clone());
                shapes.len() - 1
            }
        };
        let field = PctField {
            whole: last.whole.iter().map(&mut renumber).collect(),
            frac: last.frac.iter().map(&mut renumber).collect(),
        };
        *self = Climb {
            shapes,
            last: Some(field),
            ..Climb::default()
        };
    }
}

/// Numbers a field's glyphs as indexes into `shapes`, adding the new ones: `None` when that
/// would make more than ten.
fn number_field(shapes: &mut Vec<Glyph>, whole: &[Glyph], frac: &[Glyph]) -> Option<PctField> {
    let mut number = |glyph: &Glyph| match shapes.iter().position(|s| s == glyph) {
        Some(i) => Some(i),
        None if shapes.len() < DIGITS.len() => {
            shapes.push(glyph.clone());
            Some(shapes.len() - 1)
        }
        None => None,
    };
    Some(PctField {
        whole: whole.iter().map(&mut number).collect::<Option<_>>()?,
        frac: frac.iter().map(&mut number).collect::<Option<_>>()?,
    })
}

/// What a step from `before` to `now` within one whole percent shows: the shape in the first
/// place they differ, before, below the one there now. `None` across whole percents, where a
/// fall can't be told from a rise.
fn step_up(before: &PctField, now: &PctField) -> Option<(usize, usize)> {
    if before.whole != now.whole {
        return None;
    }
    before
        .frac
        .iter()
        .zip(&now.frac)
        .find(|(a, b)| a != b)
        .map(|(&a, &b)| (a, b))
}

/// A naming as text: shape `i` gets the character of digit `digits[i]`.
fn names(digits: &[u8]) -> String {
    digits.iter().map(|&d| char::from(b'0' + d)).collect()
}

/// Digits with a separator every three from the right: `126987` becomes `126,987`.
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

/// The EXP field as the HUD draws it: `126,987[39.16%]`.
pub fn exp_text(exp: u64, pct_hundredths: u16) -> String {
    format!(
        "{}{}",
        group_digits(&exp.to_string()),
        pct_text(pct_hundredths)
    )
}

/// The EXP field as the HUD draws it late in a level, the percent alone: `[92.61%]`.
pub fn pct_text(pct_hundredths: u16) -> String {
    format!("[{}.{:02}%]", pct_hundredths / 100, pct_hundredths % 100)
}

/// The inverse of [`exp_text`] and [`pct_text`], strictly: anything they wouldn't produce is
/// refused. The EXP is `None` for the percent alone.
fn parse_exp_text(text: &str) -> Option<(Option<u64>, u16)> {
    let (number, rest) = text.split_once('[')?;
    let (whole, frac) = rest.strip_suffix("%]")?.split_once('.')?;
    let pct = whole
        .parse::<u16>()
        .ok()?
        .checked_mul(100)?
        .checked_add(frac.parse().ok()?)?;
    if pct > 10_000 {
        return None;
    }
    let (exp, drawn) = match number {
        "" => (None, pct_text(pct)),
        _ => {
            let exp: u64 = number.replace(',', "").parse().ok()?;
            (Some(exp), exp_text(exp, pct))
        }
    };
    (drawn == text).then_some((exp, pct))
}

fn parse_level(text: &str) -> Option<u16> {
    let level: u16 = text.parse().ok()?;
    (level > 0 && level.to_string() == text).then_some(level)
}

/// The events a new reading produces after the previous one: nothing if the HUD didn't change or
/// the reading is unconfirmed (so a correction never looks like a gain), `ExpChanged` if it did,
/// and `LevelUp` as well when the level rose. Nothing either for the percent alone at a level
/// whose EXP needed isn't known, which leaves no EXP to report.
pub fn events_between(previous: Option<&ExpReading>, current: &ExpReading) -> Vec<Event> {
    let Some(exp) = current.exp.filter(|_| current.confirmed) else {
        return Vec::new();
    };
    if previous == Some(current) {
        return Vec::new();
    }
    let mut events = vec![Event::ExpChanged {
        level: current.level,
        exp,
        exp_shown: current.exp_shown,
        pct_hundredths: current.pct_hundredths,
        needed: current.needed,
    }];
    if let (Some(before), Some(level)) = (previous.and_then(|p| p.level), current.level)
        && level > before
    {
        events.push(Event::LevelUp { level });
    }
    events
}

/// A synthetic HUD for tests, here and in the engine loop's: invented glyph shapes and colors,
/// laid out the way the real HUD is. No game pixels are used.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) const DARK: [u8; 3] = [40, 44, 52];
    pub(crate) const WHITE: [u8; 3] = [255, 255, 255];
    pub(crate) const GREEN: [u8; 3] = [200, 220, 60];
    pub(crate) const ORANGE: [u8; 3] = [240, 150, 40];

    /// Column bit patterns (bit 0 = top row) for each character of a 7-row face.
    pub(crate) struct Font {
        pub(crate) glyphs: Vec<(char, Vec<u8>)>,
    }

    impl Font {
        /// Every column of a digit or letter holds ink in the top row, so segmentation never splits
        /// a glyph and no digit looks like punctuation. Rows 1-6 differ for every pair of characters in every column
        /// (37 and 63 share no factor), and `seed` gives a different face.
        pub(crate) fn new(chars: &str, seed: u32) -> Font {
            let glyphs = chars
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    let columns = match c {
                        ',' => vec![0b110_0000],
                        '.' => vec![0b100_0000],
                        // As in the game's chat: a blank space, and a colon of two dots.
                        ' ' => vec![0, 0, 0],
                        ':' => vec![0b010_0100],
                        '[' => vec![0b111_1111, 0b100_0001],
                        ']' => vec![0b100_0001, 0b111_1111],
                        _ => {
                            let width = if c == '%' { 6 } else { 5 };
                            (0..width)
                                .map(|col| {
                                    1 | (((i as u32 * 37 + col * 11 + seed * 53 + 5) % 63) as u8)
                                        << 1
                                })
                                .collect()
                        }
                    };
                    (c, columns)
                })
                .collect();
            Font { glyphs }
        }

        pub(crate) fn columns(&self, c: char) -> &[u8] {
            &self
                .glyphs
                .iter()
                .find(|(g, _)| *g == c)
                .expect("character in the font")
                .1
        }
    }

    pub(crate) struct Canvas {
        pub(crate) width: u32,
        pub(crate) height: u32,
        pub(crate) bgra: Vec<u8>,
    }

    impl Canvas {
        pub(crate) fn new(width: u32, height: u32) -> Canvas {
            let mut canvas = Canvas {
                width,
                height,
                bgra: vec![0; (width * height * 4) as usize],
            };
            canvas.fill(0, 0, width, height, DARK);
            canvas
        }

        pub(crate) fn set(&mut self, x: u32, y: u32, [r, g, b]: [u8; 3]) {
            let i = ((y * self.width + x) * 4) as usize;
            self.bgra[i..i + 4].copy_from_slice(&[b, g, r, 255]);
        }

        /// The pixel at (x, y) as RGB.
        pub(crate) fn get(&self, x: u32, y: u32) -> [u8; 3] {
            let i = ((y * self.width + x) * 4) as usize;
            [self.bgra[i + 2], self.bgra[i + 1], self.bgra[i]]
        }

        pub(crate) fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
            for yy in y..y + h {
                for xx in x..x + w {
                    self.set(xx, yy, color);
                }
            }
        }

        /// Draws `text` with its top row at `y`, one column between glyphs.
        pub(crate) fn text(
            &mut self,
            font: &Font,
            text: &str,
            mut x: u32,
            y: u32,
            color: impl Fn(char) -> [u8; 3],
        ) {
            for c in text.chars() {
                for &bits in font.columns(c) {
                    for row in 0..7 {
                        if bits >> row & 1 == 1 {
                            self.set(x, y + row, color(c));
                        }
                    }
                    x += 1;
                }
                x += 1;
            }
        }

        pub(crate) fn frame(&self) -> Frame<'_> {
            Frame::new(
                self.width,
                self.height,
                (self.width * 4) as usize,
                &self.bgra,
            )
            .unwrap()
        }
    }

    pub(crate) struct Hud {
        pub(crate) exp_font: Font,
        pub(crate) level_font: Font,
        /// The HP and MP numbers' slash, the one glyph they have that the EXP field doesn't.
        pub(crate) slash_font: Font,
        /// Varies the invented "EXP." label's pattern: a game update that redrew it.
        pub(crate) label: u32,
    }

    impl Hud {
        pub(crate) fn new() -> Hud {
            Hud {
                exp_font: Font::new(EXP_CHARS, 0),
                level_font: Font::new(LEVEL_CHARS, 7),
                slash_font: Font::new("/", 11),
                label: 3,
            }
        }

        pub(crate) fn draw(&self, width: u32, height: u32, exp_field: &str, level: &str) -> Canvas {
            self.draw_shifted(width, height, exp_field, level, 0)
        }

        /// The HUD moved right by `shift` pixels: a changed UI.
        pub(crate) fn draw_shifted(
            &self,
            width: u32,
            height: u32,
            exp_field: &str,
            level: &str,
            shift: u32,
        ) -> Canvas {
            let mut c = Canvas::new(width, height);
            let left = (width - 800) / 2 + shift;
            // The "EXP." label: an invented mark in the label area.
            for (i, x) in (left + 440..left + 464).enumerate() {
                for y in height - 30..height - 22 {
                    if !(i as u32 + y).is_multiple_of(self.label) {
                        c.set(x, y, WHITE);
                    }
                }
            }
            c.text(&self.exp_font, exp_field, left + 468, height - 28, |ch| {
                if matches!(ch, '[' | ']') {
                    GREEN
                } else {
                    WHITE
                }
            });
            // Level tiles: orange squares with white digits.
            for (i, ch) in level.chars().enumerate() {
                let tile = left + 46 + i as u32 * 12;
                c.fill(tile, height - 24, 11, 13, ORANGE);
                c.text(
                    &self.level_font,
                    &ch.to_string(),
                    tile + 3,
                    height - 21,
                    |_| WHITE,
                );
            }
            c
        }

        /// Adds the "HP" and "MP" labels (invented marks) and the two fields, such as `[712/1386]`,
        /// where the HUD draws them: brackets yellow-green, the rest white.
        pub(crate) fn draw_hp_mp(&self, c: &mut Canvas, hp: &str, mp: &str) {
            let (left, bottom) = ((c.width - 800) / 2, c.height);
            for (from, to) in [(220, 235), (329, 346)] {
                for x in left + from..left + to {
                    for y in bottom - 29..bottom - 21 {
                        if (x + y) % 3 != 0 {
                            c.set(x, y, WHITE);
                        }
                    }
                }
            }
            for (field, dx) in [(hp, 237), (mp, 348)] {
                let mut x = left + dx;
                for ch in field.chars() {
                    let font = if ch == '/' {
                        &self.slash_font
                    } else {
                        &self.exp_font
                    };
                    let color = if matches!(ch, '[' | ']') {
                        GREEN
                    } else {
                        WHITE
                    };
                    c.text(font, &ch.to_string(), x, bottom - 28, |_| color);
                    x += font.columns(ch).len() as u32 + 1;
                }
            }
        }

        pub(crate) fn draw_values(&self, level: u16, exp: u64, pct_hundredths: u16) -> Canvas {
            self.draw(
                1366,
                768,
                &exp_text(exp, pct_hundredths),
                &level.to_string(),
            )
        }

        /// The HUD late in a level, showing only the percent.
        pub(crate) fn draw_pct(&self, level: u16, pct_hundredths: u16) -> Canvas {
            self.draw(1366, 768, &pct_text(pct_hundredths), &level.to_string())
        }

        pub(crate) fn draw_typed(&self, width: u32, height: u32, r: TypedReading) -> Canvas {
            self.draw(
                width,
                height,
                &exp_text(r.exp, r.pct_hundredths),
                &r.level.to_string(),
            )
        }
    }

    /// A 768-line picture as the game stretches it to fill a `width` x `height` game area: only the
    /// HUD's rows and a few above them, which is all the readers need.
    pub(crate) fn stretched(canvas: &Canvas, width: u32, height: u32) -> Canvas {
        let rgb: Vec<[u8; 3]> = canvas
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[2], p[1], p[0]])
            .collect();
        let from_row = height - (HUD_ROWS + 4) * height / canvas.height;
        let bgra = unstretch::stretch(&rgb, canvas.width, width, height, from_row);
        Canvas {
            width,
            height,
            bgra,
        }
    }

    /// The bar's percent for `exp` at a level needing `needed`, rounded half up like the game.
    pub(crate) fn pct_of(exp: u64, needed: u64) -> u16 {
        ((exp * 20_000 + needed) / (2 * needed)) as u16
    }

    /// EXP totals from a recorded Windows run (level 41, EXP needed 324,240).
    pub(crate) const GRIND: [u64; 16] = [
        128_027, 128_048, 128_108, 128_168, 128_249, 128_292, 128_313, 128_334, 128_355, 128_398,
        128_406, 128_427, 128_448, 128_469, 128_512, 128_555,
    ];
    pub(crate) const NEEDED_41: u64 = 324_240;
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_support::*;

    /// Feeds frames of `level` at each EXP to `observe` and returns what came out.
    fn feed(
        hud: &Hud,
        reader: &mut ExpBarReader,
        level: u16,
        needed: u64,
        exps: &[u64],
    ) -> Vec<Result<ExpReading, Unreadable>> {
        exps.iter()
            .map(|&exp| reader.observe(&hud.draw_values(level, exp, pct_of(exp, needed)).frame()))
            .collect()
    }

    const FIRST: TypedReading = TypedReading {
        level: 41,
        exp: 126_987,
        pct_hundredths: 3916,
    };
    /// With FIRST, covers every character of the EXP field. The level digits stay partial.
    const SECOND: TypedReading = TypedReading {
        level: 50,
        exp: 2_345_078,
        pct_hundredths: 6420,
    };

    fn typed(hud: &Hud) -> ExpBarReader {
        let mut reader = ExpBarReader::new();
        reader
            .calibrate(&hud.draw_typed(1366, 768, FIRST).frame(), FIRST)
            .unwrap();
        reader
            .calibrate(&hud.draw_typed(1366, 768, SECOND).frame(), SECOND)
            .unwrap();
        reader
    }

    /// A reading of the full field, which shows the EXP.
    fn values(r: &ExpReading) -> (Option<u16>, u64, u16) {
        assert!(r.exp_shown, "{r:?}");
        (r.level, r.exp.expect("the EXP shown"), r.pct_hundredths)
    }

    // Self-calibration.

    #[test]
    fn learns_the_digits_with_no_typing_as_exp_moves() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let results = feed(&hud, &mut reader, 41, NEEDED_41, &GRIND);
        assert_eq!(results[0], Err(Unreadable::Learning));
        let first_shown = results
            .iter()
            .position(|r| r.is_ok())
            .expect("readings appear");
        let first_confirmed = results
            .iter()
            .position(|r| r.as_ref().is_ok_and(|r| r.confirmed))
            .expect("readings are confirmed");
        assert!(first_shown <= first_confirmed);
        assert!(
            first_confirmed < 10,
            "confirmed after {} frames",
            first_confirmed + 1
        );
        for (r, &exp) in results.iter().zip(&GRIND).skip(first_shown) {
            let r = r.as_ref().unwrap();
            assert_eq!(
                (r.exp, r.pct_hundredths),
                (Some(exp), pct_of(exp, NEEDED_41))
            );
            assert_eq!(r.level, Some(41), "the table's level for 324,240");
            assert_eq!(r.needed, Some(NEEDED_41));
        }
        assert_eq!(reader.calibration(), Calibration::Confirmed);
        // The table's level taught the tiles, so the level now reads from them.
        assert!(!reader.missing().level.contains(&'4') && !reader.missing().level.contains(&'1'));
    }

    // Saved knowledge: a later session shows level and EXP at once, not "learning".

    fn confirmed_knowledge(hud: &Hud) -> String {
        let mut reader = ExpBarReader::new();
        assert_eq!(
            reader.knowledge(),
            None,
            "nothing to keep before confirming"
        );
        feed(hud, &mut reader, 41, NEEDED_41, &GRIND);
        assert_eq!(reader.calibration(), Calibration::Confirmed);
        reader.knowledge().expect("confirmed knowledge")
    }

    // Fails if saved knowledge doesn't read at once, reads unconfirmed, or saves back differently.
    #[test]
    fn what_it_confirmed_is_kept_and_read_at_once_next_session() {
        let hud = Hud::new();
        let kept = confirmed_knowledge(&hud);
        let mut next = ExpBarReader::from_knowledge(&kept).unwrap();
        assert_eq!(next.calibration(), Calibration::Confirmed);
        // The first frame of the new session, before any EXP is gained, reads confirmed.
        let exp = 128_600;
        let frame = hud.draw_values(41, exp, pct_of(exp, NEEDED_41));
        let first = next.observe(&frame.frame()).unwrap();
        assert!(first.confirmed);
        assert_eq!(values(&first), (Some(41), exp, pct_of(exp, NEEDED_41)));
        assert_eq!(next.knowledge(), Some(kept.clone()), "saved back the same");
        // A file that picked up Windows line endings reads the same.
        assert!(ExpBarReader::from_knowledge(&kept.replace('\n', "\r\n")).is_some());
    }

    #[test]
    fn saved_knowledge_must_be_text_a_reader_wrote() {
        let kept = confirmed_knowledge(&Hud::new());
        let no_label: String = kept
            .lines()
            .filter(|l| !l.starts_with("label"))
            .map(|l| format!("{l}\n"))
            .collect();
        for bad in [
            String::new(),
            "hello".to_string(),
            kept.replace("exp-bar 1", "exp-bar 2"),
            // Two shapes named "2".
            kept.replacen("exp 1 ", "exp 2 ", 1),
            no_label,
            format!("{kept}junk\n"),
        ] {
            assert!(ExpBarReader::from_knowledge(&bad).is_none(), "{bad:?}");
        }
    }

    // After a game update, saved shapes that no longer fit must never read wrong. Fails if a
    // stale atlas reads a confirmed wrong value, or stays unreadable for good.
    #[test]
    fn saved_knowledge_that_no_longer_fits_is_dropped_and_learned_again() {
        let kept = confirmed_knowledge(&Hud::new());
        let check = |hud: &Hud, frames: usize| {
            let mut reader = ExpBarReader::from_knowledge(&kept).unwrap();
            let mut first_confirmed = None;
            for (i, &exp) in GRIND.iter().cycle().take(frames).enumerate() {
                let frame = hud.draw_values(41, exp, pct_of(exp, NEEDED_41));
                if let Ok(r) = reader.observe(&frame.frame())
                    && r.confirmed
                {
                    assert_eq!(values(&r), (Some(41), exp, pct_of(exp, NEEDED_41)));
                    first_confirmed.get_or_insert(i);
                }
            }
            assert_eq!(reader.calibration(), Calibration::Confirmed);
            first_confirmed.expect("learned again")
        };
        // Redrawn digits under the same label: dropped after RESTORE_PATIENCE frames.
        let redrawn = Hud {
            exp_font: Font::new(EXP_CHARS, 3),
            ..Hud::new()
        };
        let frames = RESTORE_PATIENCE as usize + GRIND.len();
        assert!(check(&redrawn, frames) >= RESTORE_PATIENCE as usize);
        // A redrawn label on a HUD that's plainly there: dropped on the first frame.
        let relabelled = Hud {
            exp_font: Font::new(EXP_CHARS, 3),
            label: 4,
            ..Hud::new()
        };
        assert!(check(&relabelled, GRIND.len()) < GRIND.len());
    }

    // Switching size can draw the HUD differently: on a Mac, Full Screen at another size blends
    // the label and digits into other shapes. Fails if shapes learned this session keep a reader
    // that can't find its label stuck for good (as players saw until they restarted), or if a
    // recheck at a size where they still fit loses a reading.
    #[test]
    fn shapes_learned_at_one_size_are_learned_again_after_a_switch() {
        let mut reader = ExpBarReader::new();
        let hud = Hud::new();
        for &exp in &GRIND {
            let _ = reader.observe(&hud.draw_values(41, exp, pct_of(exp, NEEDED_41)).frame());
        }
        assert_eq!(reader.calibration(), Calibration::Confirmed);
        let still = reader.clone();
        let switched = Hud {
            exp_font: Font::new(EXP_CHARS, 3),
            label: 4,
            ..Hud::new()
        };
        let frame = |hud: &Hud, exp: u64| hud.draw_values(41, exp, pct_of(exp, NEEDED_41));
        // Without a recheck: never read again.
        let mut stuck = reader.clone();
        assert!(
            GRIND
                .iter()
                .all(|&exp| stuck.observe(&frame(&switched, exp).frame()).is_err())
        );
        // With one: learned again from the next EXP values.
        reader.recheck();
        let read = GRIND
            .iter()
            .filter_map(|&exp| reader.observe(&frame(&switched, exp).frame()).ok())
            .filter(|r| r.confirmed)
            .last();
        let last = *GRIND.last().unwrap();
        assert_eq!(
            read.map(|r| values(&r)),
            Some((Some(41), last, pct_of(last, NEEDED_41)))
        );
        // Where the shapes still fit, the first reading after a recheck confirms them.
        let mut kept = still;
        kept.recheck();
        let r = kept.observe(&frame(&hud, last).frame()).unwrap();
        assert!(r.confirmed);
        assert_eq!(kept.calibration(), Calibration::Confirmed);
    }

    #[test]
    fn the_table_shows_readings_before_the_hud_alone_could() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let mut states = Vec::new();
        for &exp in &GRIND {
            let r = reader.observe(&hud.draw_values(41, exp, pct_of(exp, NEEDED_41)).frame());
            states.push(reader.calibration());
            if reader.calibration() == Calibration::Provisional {
                let r = r.unwrap();
                assert!(
                    !r.confirmed,
                    "a table-assisted reading is marked unconfirmed"
                );
                assert!(
                    events_between(None, &r).is_empty(),
                    "and produces no events"
                );
            }
        }
        let provisional = states.iter().position(|s| *s == Calibration::Provisional);
        let confirmed = states
            .iter()
            .position(|s| *s == Calibration::Confirmed)
            .unwrap();
        assert!(provisional.is_some_and(|p| p < confirmed), "{states:?}");
    }

    #[test]
    fn a_level_the_table_gets_wrong_still_confirms_and_is_never_read_wrong() {
        // EXP needed 300,000 is no level's in the table: the table can't help, and may mislead
        // while learning, but nothing confirmed is wrong.
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let needed = 300_000;
        let exps: Vec<u64> = (0..40).map(|i| 128_027 + i * 37 + (i * i) % 23).collect();
        let results = feed(&hud, &mut reader, 41, needed, &exps);
        assert_eq!(reader.calibration(), Calibration::Confirmed);
        for (r, &exp) in results.iter().zip(&exps) {
            if let Ok(r) = r
                && r.confirmed
            {
                assert_eq!((r.exp, r.pct_hundredths), (Some(exp), pct_of(exp, needed)));
                assert_eq!((r.level, r.needed), (None, None), "no table level fits");
            }
        }
    }

    #[test]
    fn holding_still_teaches_nothing() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let results = feed(&hud, &mut reader, 41, NEEDED_41, &[128_027; 12]);
        assert!(results.iter().all(|r| *r == Err(Unreadable::Learning)));
        assert_eq!(reader.calibration(), Calibration::Learning);
    }

    #[test]
    fn covered_and_foreign_frames_while_learning_change_nothing() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        feed(&hud, &mut reader, 41, NEEDED_41, &GRIND[..2]);
        let mut covered = hud.draw_values(41, GRIND[2], pct_of(GRIND[2], NEEDED_41));
        covered.fill(283 + 470, 768 - 26, 60, 2, WHITE);
        assert_eq!(
            reader.observe(&covered.frame()),
            Err(Unreadable::Garbled(Field::Exp))
        );
        // Brackets missing: not the EXP field's structure.
        let odd = hud.draw(1366, 768, "128,108", "41");
        assert_eq!(
            reader.observe(&odd.frame()),
            Err(Unreadable::Garbled(Field::Exp))
        );
        let results = feed(&hud, &mut reader, 41, NEEDED_41, &GRIND[2..]);
        assert!(results.last().unwrap().as_ref().is_ok_and(|r| r.confirmed));
    }

    #[test]
    fn a_level_up_while_learning_starts_the_evidence_over() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        // Two fields at level 40, then the level-41 grind: no naming fits both levels at once.
        let before = feed(&hud, &mut reader, 40, 294_216, &[290_001, 290_123]);
        assert!(before.iter().all(|r| *r == Err(Unreadable::Learning)));
        let results = feed(&hud, &mut reader, 41, NEEDED_41, &GRIND);
        let last = results.last().unwrap().as_ref().unwrap();
        assert!(last.confirmed);
        assert_eq!(
            values(last),
            (Some(41), 128_555, pct_of(128_555, NEEDED_41))
        );
    }

    #[test]
    fn after_confirming_it_follows_level_ups_with_the_tiles() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let results = feed(&hud, &mut reader, 41, NEEDED_41, &GRIND);
        let before = *results.last().unwrap().as_ref().unwrap();
        let after = reader.observe(&hud.draw_values(42, 12, 0).frame()).unwrap();
        assert_eq!(values(&after), (Some(42), 12, 0));
        assert_eq!(
            after.needed,
            Some(356_916),
            "level 42's EXP needed fits 12 EXP at 0.00%"
        );
        assert!(events_between(Some(&before), &after).contains(&Event::LevelUp { level: 42 }));
        // A death keeps the level: the EXP falls, but the tiles still say 42.
        let died = reader.observe(&hud.draw_values(42, 8, 0).frame()).unwrap();
        assert_eq!(died.level, Some(42));
    }

    #[test]
    fn a_digit_first_seen_after_confirming_is_learned_from_the_bar() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let no_seven = |e: u64| !exp_text(e, pct_of(e, NEEDED_41)).contains('7');
        let exps: Vec<u64> = (0..400)
            .map(|i| 130_000 + i * 113)
            .filter(|&e| no_seven(e))
            .take(30)
            .collect();
        feed(&hud, &mut reader, 41, NEEDED_41, &exps);
        assert_eq!(reader.calibration(), Calibration::Confirmed);
        assert!(
            reader.missing().exp.contains(&'7'),
            "learning never saw a 7"
        );
        let exp = (exps[29]..).find(|&e| !no_seven(e) && e % 10 != 7).unwrap();
        let r = reader
            .observe(&hud.draw_values(41, exp, pct_of(exp, NEEDED_41)).frame())
            .unwrap();
        assert_eq!(
            (r.exp, r.pct_hundredths),
            (Some(exp), pct_of(exp, NEEDED_41))
        );
        assert!(!reader.missing().exp.contains(&'7'));
    }

    // The game's cursor over the full field's last digit, while a 7 was never seen. Fails if the
    // covered 0 is learned as the 7, which would leave every real 7 unreadable.
    #[test]
    fn a_stray_pixel_over_a_low_digit_is_not_learned_as_a_missing_one() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let no_seven = |e: u64| !exp_text(e, pct_of(e, NEEDED_41)).contains('7');
        let exps: Vec<u64> = (0..400)
            .map(|i| 130_000 + i * 113)
            .filter(|&e| no_seven(e))
            .take(30)
            .collect();
        feed(&hud, &mut reader, 41, NEEDED_41, &exps);
        assert_eq!(reader.missing().exp, ['7']);
        // One more white pixel inside the last 0 of 136,100[41.98%].
        let mut covered = hud.draw_values(41, 136_100, pct_of(136_100, NEEDED_41));
        let x = 283
            + 468
            + "136,10"
                .chars()
                .map(|c| hud.exp_font.columns(c).len() as u32 + 1)
                .sum::<u32>();
        let (px, py) = (x..x + 5)
            .flat_map(|px| (740..747).map(move |py| (px, py)))
            .find(|&(px, py)| covered.get(px, py) == DARK)
            .expect("a gap inside the 0");
        covered.set(px, py, WHITE);
        assert_eq!(
            reader.observe(&covered.frame()),
            Err(Unreadable::UnknownGlyph(Field::Exp))
        );
        assert_eq!(reader.missing().exp, ['7']);
        // A real 7 where no other digit fits is still learned.
        let exp = (217_200..).find(|&e| !no_seven(e)).unwrap();
        let full = reader
            .observe(&hud.draw_values(41, exp, pct_of(exp, NEEDED_41)).frame())
            .unwrap();
        assert_eq!(full.exp, Some(exp));
        assert!(reader.missing().exp.is_empty());
    }

    // The percent alone, late in a level.

    // Fails if the percent alone doesn't read with the digits learned from the full field, if
    // the EXP worked out from it isn't one the percent allows, if it moves while the percent
    // holds or doesn't rise with it, if the switch from the full field reads as a gain, or if the
    // level-up back to the full field is missed.
    #[test]
    fn reads_the_percent_alone_with_digits_learned_from_the_full_field() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let shown = *feed(&hud, &mut reader, 41, NEEDED_41, &GRIND)
            .last()
            .unwrap()
            .as_ref()
            .unwrap();
        assert!(shown.confirmed && shown.exp_shown);
        // The HUD hides the EXP at the same percent: the EXP carries on.
        let last = *GRIND.last().unwrap();
        let switched = reader
            .observe(&hud.draw_pct(41, pct_of(last, NEEDED_41)).frame())
            .unwrap();
        assert!(switched.confirmed && !switched.exp_shown);
        assert_eq!(
            (switched.level, switched.exp, switched.needed),
            (Some(41), Some(last), Some(NEEDED_41))
        );
        assert!(matches!(
            events_between(Some(&shown), &switched)[..],
            [Event::ExpChanged { exp, exp_shown: false, .. }] if exp == last
        ));
        let mut before = switched;
        for i in 1..60 {
            let exp = last + i * 37;
            let pct = pct_of(exp, NEEDED_41);
            let r = reader.observe(&hud.draw_pct(41, pct).frame()).unwrap();
            assert!(r.confirmed && !r.exp_shown, "{r:?}");
            assert_eq!(
                (r.level, r.needed, r.pct_hundredths),
                (Some(41), Some(NEEDED_41), pct)
            );
            let worked_out = r.exp.expect("worked out from the percent");
            assert_eq!(
                pct_of(worked_out, NEEDED_41),
                pct,
                "an EXP the percent allows"
            );
            assert!(worked_out.abs_diff(exp) <= NEEDED_41.div_ceil(10_000));
            let held = before.exp.unwrap();
            match pct == before.pct_hundredths {
                true => assert_eq!(worked_out, held, "holds while the percent does"),
                false => assert!(worked_out > held, "rises with the percent"),
            }
            before = r;
        }
        // The level-up brings the full field back.
        let up = reader.observe(&hud.draw_values(42, 12, 0).frame()).unwrap();
        assert_eq!(values(&up), (Some(42), 12, 0));
        assert!(events_between(Some(&before), &up).contains(&Event::LevelUp { level: 42 }));
    }

    // Value: protects=a death while only the percent shows keeps the level and an EXP the fallen
    // percent allows; fails_when=a fall reads as a level-up or the EXP stays above the bar;
    // why_new=no reader test dies in percent-only; seam=none; no_claim=notices or UI
    // Fails if a death while the HUD shows only the percent reads as a level-up, or if the EXP
    // worked out after it isn't one the fallen percent allows.
    #[test]
    fn a_death_while_the_percent_shows_alone_keeps_the_level() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        feed(&hud, &mut reader, 41, NEEDED_41, &GRIND);
        let last = *GRIND.last().unwrap();
        let before = reader
            .observe(&hud.draw_pct(41, pct_of(last, NEEDED_41)).frame())
            .unwrap();
        let dead = last - last / 10;
        let pct = pct_of(dead, NEEDED_41);
        let after = reader.observe(&hud.draw_pct(41, pct).frame()).unwrap();
        assert!(after.confirmed && !after.exp_shown, "{after:?}");
        assert_eq!((after.level, after.needed), (Some(41), Some(NEEDED_41)));
        assert_eq!(after.exp.map(|e| pct_of(e, NEEDED_41)), Some(pct));
        assert!(
            !events_between(Some(&before), &after)
                .iter()
                .any(|e| matches!(e, Event::LevelUp { .. }))
        );
    }

    // A frame with the cursor over the level tiles has no level, so no EXP. Fails if the EXP after
    // it restarts at the middle of the percent's range, which the Session Tracker logs as EXP lost.
    #[test]
    fn a_frame_with_the_level_hidden_leaves_the_worked_out_exp_where_it_was() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        feed(&hud, &mut reader, 41, NEEDED_41, &GRIND);
        let last = *GRIND.last().unwrap();
        let pct = pct_of(last, NEEDED_41);
        let before = reader.observe(&hud.draw_pct(41, pct).frame()).unwrap();
        assert_eq!(before.exp, Some(last));
        let mut cursor = hud.draw_pct(41, pct);
        cursor.fill(283 + 46, 768 - 24, 23, 13, WHITE);
        let hidden = reader.observe(&cursor.frame()).unwrap();
        assert_eq!((hidden.level, hidden.exp), (None, None));
        let after = reader.observe(&hud.draw_pct(41, pct).frame()).unwrap();
        assert_eq!(after, before);
    }

    // Fails if the percent alone trusts the table's EXP needed for a level whose full field
    // disproved it, including after one frame whose level tiles looked different (a cursor).
    #[test]
    fn a_table_row_the_full_field_disproved_is_not_trusted_for_the_percent_alone() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        feed(&hud, &mut reader, 41, NEEDED_41, &GRIND);
        // Level 44 needs 400,000 here, not the table's 428,280. One full field off the row could
        // be a misread; a second, different one disproves it.
        let needed = 400_000;
        let _ = reader.observe(
            &hud.draw_values(44, 299_000, pct_of(299_000, needed))
                .frame(),
        );
        let full = reader
            .observe(
                &hud.draw_values(44, 300_000, pct_of(300_000, needed))
                    .frame(),
            )
            .unwrap();
        assert_eq!(
            (full.level, full.needed),
            (Some(44), None),
            "the bar doesn't fit the row"
        );
        let alone = reader
            .observe(&hud.draw_pct(44, pct_of(396_000, needed)).frame())
            .unwrap();
        assert!(alone.confirmed && !alone.exp_shown);
        assert_eq!(
            (alone.level, alone.needed, alone.exp),
            (Some(44), None, None)
        );
        assert!(events_between(Some(&full), &alone).is_empty());
        // One frame with other tiles, then the level's own again: still the full field's verdict.
        let _ = reader.observe(&hud.draw_pct(45, pct_of(396_050, needed)).frame());
        let back = reader
            .observe(&hud.draw_pct(44, pct_of(396_100, needed)).frame())
            .unwrap();
        assert_eq!((back.level, back.needed, back.exp), (Some(44), None, None));
    }

    // A level's first full field is a misread: partly covered, it reads as a smaller EXP. Fails
    // if the percent alone after it shows no EXP for the rest of the level.
    #[test]
    fn a_misread_first_full_field_at_a_level_doesnt_blank_the_percent_alone() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        feed(&hud, &mut reader, 41, NEEDED_41, &GRIND);
        let needed_44 = exp_table::needed(44).unwrap();
        let misread = reader
            .observe(
                &hud.draw_values(44, 30_000, pct_of(300_000, needed_44))
                    .frame(),
            )
            .unwrap();
        assert_eq!((misread.level, misread.needed), (Some(44), None));
        let pct = pct_of(300_500, needed_44);
        let alone = reader.observe(&hud.draw_pct(44, pct).frame()).unwrap();
        assert!(alone.confirmed && !alone.exp_shown);
        assert_eq!((alone.level, alone.needed), (Some(44), Some(needed_44)));
        assert_eq!(alone.exp.map(|e| pct_of(e, needed_44)), Some(pct));
    }

    // Value: protects=the table's EXP needed works the EXP out at a level with no full field seen
    // this session; fails_when=a restart late in a level shows no EXP; why_new=the disproved-row
    // test covers only a level the full field contradicted; seam=none; no_claim=the table's accuracy
    // A session that starts late in a level sees only the percent. Fails if the table's EXP needed
    // isn't used there, as the player chose, or if a row the full field fitted isn't kept.
    #[test]
    fn a_level_never_seen_in_full_works_the_exp_out_from_the_table() {
        let hud = Hud::new();
        let kept = confirmed_knowledge(&hud);
        let mut reader = ExpBarReader::from_knowledge(&kept).unwrap();
        let needed_44 = exp_table::needed(44).unwrap();
        let alone = reader
            .observe(&hud.draw_pct(44, pct_of(396_000, needed_44)).frame())
            .unwrap();
        assert!(alone.confirmed && !alone.exp_shown);
        assert_eq!((alone.level, alone.needed), (Some(44), Some(needed_44)));
        assert_eq!(
            alone.exp.map(|e| pct_of(e, needed_44)),
            Some(pct_of(396_000, needed_44))
        );
    }

    // Value: protects=with two digits never seen, a small fall in the percent teaches no digit;
    // fails_when=a higher missing digit is learned because it alone keeps the percent from falling;
    // why_new=the unseen-digit test has one digit missing; seam=none; no_claim=none
    // Fails if a fall of a few hundredths, while two digits are still unseen, names the shape of
    // the lower one as the higher one.
    #[test]
    fn a_small_fall_with_two_digits_unseen_teaches_no_digit() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let shown = |e: u64| {
            let text = exp_text(e, pct_of(e, NEEDED_41));
            !text.contains('2') && !text.contains('7')
        };
        let exps: Vec<u64> = (0..1500)
            .map(|i| 130_000 + i * 113)
            .filter(|&e| shown(e))
            .take(30)
            .collect();
        feed(&hud, &mut reader, 41, NEEDED_41, &exps);
        assert_eq!(reader.calibration(), Calibration::Confirmed);
        assert!(reader.missing().exp.contains(&'2') && reader.missing().exp.contains(&'7'));
        let read = reader.observe(&hud.draw_pct(41, 6694).frame());
        assert_eq!(read.map(|r| r.pct_hundredths), Ok(6694));
        // 66.92% after 66.94%: only a 7 there would keep the percent from falling.
        assert_eq!(
            reader.observe(&hud.draw_pct(41, 6692).frame()),
            Err(Unreadable::UnknownGlyph(Field::Exp))
        );
        assert!(reader.missing().exp.contains(&'7'));
    }

    // A few frames of the percent alone can't name the digits. Fails if they read anything, or
    // if frames of it between full ones hold up learning from the full field.
    #[test]
    fn a_few_frames_of_the_percent_alone_read_nothing_and_hold_up_no_learning() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        for &exp in &GRIND {
            let frame = hud.draw_pct(41, pct_of(exp, NEEDED_41));
            assert_eq!(reader.observe(&frame.frame()), Err(Unreadable::Learning));
        }
        assert_eq!(reader.calibration(), Calibration::Learning);
        let mut full_only = ExpBarReader::new();
        let want = *feed(&hud, &mut full_only, 41, NEEDED_41, &GRIND)
            .last()
            .unwrap();
        let mut last = None;
        for &exp in &GRIND {
            let _ = reader.observe(&hud.draw_pct(41, pct_of(exp + 7, NEEDED_41)).frame());
            last = Some(reader.observe(&hud.draw_values(41, exp, pct_of(exp, NEEDED_41)).frame()));
        }
        assert_eq!(last, Some(want));
        assert_eq!(reader.calibration(), Calibration::Confirmed);
    }

    // At high levels the HUD shows only the percent for most of each level. Fails if saved
    // knowledge reads it unconfirmed (no events until the next level), if the HP and MP numbers
    // wait for a check the percent alone can't give, or if a level whose tiles aren't known yet
    // gets an EXP made up for it.
    #[test]
    fn saved_knowledge_reads_the_percent_alone_from_the_first_frame() {
        let hud = Hud::new();
        let kept = confirmed_knowledge(&hud);
        let mut reader = ExpBarReader::from_knowledge(&kept).unwrap();
        let mut canvas = hud.draw_pct(41, 9261);
        hud.draw_hp_mp(&mut canvas, "[712/1386]", "[598/1140]");
        let first = reader.observe_hud(&canvas.frame());
        let r = first.exp.unwrap();
        assert!(r.confirmed && !r.exp_shown);
        assert_eq!(
            (r.level, r.needed, r.pct_hundredths),
            (Some(41), Some(NEEDED_41), 9261)
        );
        assert_eq!(r.exp.map(|e| pct_of(e, NEEDED_41)), Some(9261));
        assert_eq!(events_between(None, &r).len(), 1);
        assert_ne!(first.hp_mp, Err(HpMpUnreadable::Unchecked));
        let hp_mp = (0..4)
            .map(|_| reader.observe_hud(&canvas.frame()).hp_mp)
            .last();
        assert!(matches!(hp_mp, Some(Ok(_))), "{hp_mp:?}");
        assert_eq!(
            reader.knowledge(),
            Some(kept.clone()),
            "kept what was saved"
        );

        let mut unknown_tiles = ExpBarReader::from_knowledge(&kept).unwrap();
        let r = unknown_tiles
            .observe(&hud.draw_pct(58, 9261).frame())
            .unwrap();
        assert!(r.confirmed);
        assert_eq!((r.level, r.exp, r.pct_hundredths), (None, None, 9261));
        assert!(events_between(None, &r).is_empty(), "no EXP to report");
    }

    // Value: protects=a shape first seen in the percent alone is never named from it;
    // fails_when=a stray pixel over the last digit (the game's cursor) is learned as the missing
    // digit; why_new=the percent alone can't tell a digit from something drawn over one;
    // seam=none; no_claim=none
    // A digit the full field never showed turns up in the percent alone, or something is drawn
    // over the percent's last digit. Fails if either is learned as the missing digit from the
    // percent, or if the digit isn't learned once the full field shows it.
    #[test]
    fn a_shape_first_seen_in_the_percent_alone_waits_for_the_full_field() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let no_seven = |e: u64| !exp_text(e, pct_of(e, NEEDED_41)).contains('7');
        let exps: Vec<u64> = (0..400)
            .map(|i| 130_000 + i * 113)
            .filter(|&e| no_seven(e))
            .take(30)
            .collect();
        feed(&hud, &mut reader, 41, NEEDED_41, &exps);
        assert_eq!(reader.missing().exp, ['7']);
        let observe = |reader: &mut ExpBarReader, pct: u16| {
            reader
                .observe(&hud.draw_pct(41, pct).frame())
                .map(|r| r.pct_hundredths)
        };
        let unknown = Err(Unreadable::UnknownGlyph(Field::Exp));
        assert_eq!(observe(&mut reader, 6694), Ok(6694));
        // The game's cursor over the last digit: one more white pixel inside the 4.
        let mut covered = hud.draw_pct(41, 6694);
        let x = 283
            + 468
            + "[66.9"
                .chars()
                .map(|c| hud.exp_font.columns(c).len() as u32 + 1)
                .sum::<u32>();
        let (px, py) = (x..x + 5)
            .flat_map(|px| (740..747).map(move |py| (px, py)))
            .find(|&(px, py)| covered.get(px, py) == DARK)
            .expect("a gap inside the 4");
        covered.set(px, py, WHITE);
        assert_eq!(
            reader.observe(&covered.frame()).map(|r| r.pct_hundredths),
            unknown
        );
        assert_eq!(observe(&mut reader, 6697), unknown, "a real 7 waits too");
        assert_eq!(reader.missing().exp, ['7']);
        // The full field shows a 7: learned from the bar, which checks it.
        let exp = (217_200..).find(|&e| !no_seven(e)).unwrap();
        let full = reader
            .observe(&hud.draw_values(41, exp, pct_of(exp, NEEDED_41)).frame())
            .unwrap();
        assert_eq!(full.exp, Some(exp));
        assert!(reader.missing().exp.is_empty());
        assert_eq!(observe(&mut reader, 6697), Ok(6697));
    }

    // Value: protects=one full field off the table doesn't undo a row that fitted, and two
    // different ones do; fails_when=one odd frame blanks the EXP for the rest of the level, or a
    // wrong row survives; why_new=the disproved-row test starts from no row; seam=none;
    // no_claim=none
    // Fails if one full field the table doesn't fit (a field partly covered, read as a smaller
    // EXP) stops the percent alone using a row earlier fields fitted, or if two different such
    // fields don't.
    #[test]
    fn one_full_field_off_the_table_doesnt_undo_a_row_that_fitted() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        feed(&hud, &mut reader, 41, NEEDED_41, &GRIND);
        let odd = reader
            .observe(&hud.draw_values(41, 166, 9261).frame())
            .unwrap();
        assert_eq!((odd.level, odd.needed), (Some(41), None));
        let alone = reader.observe(&hud.draw_pct(41, 9262).frame()).unwrap();
        assert_eq!(
            alone.needed,
            Some(NEEDED_41),
            "one odd frame isn't evidence"
        );
        let again = reader
            .observe(&hud.draw_values(41, 170, 9263).frame())
            .unwrap();
        assert_eq!(again.needed, None);
        let alone = reader.observe(&hud.draw_pct(41, 9264).frame()).unwrap();
        assert_eq!(
            (alone.needed, alone.exp),
            (None, None),
            "two different ones are"
        );
    }

    // Value: protects=one odd shape early in a climb doesn't stop it naming the digits;
    // fails_when=an 11th shape is refused for good; why_new=the climb tests feed only clean
    // shapes; seam=none; no_claim=none
    // Fails if something drawn over the percent once, before all ten digits showed, keeps the
    // climb from ever naming them.
    #[test]
    fn an_odd_shape_early_in_a_climb_doesnt_stop_it() {
        let shape = |d: u16| Glyph::from_rows(4, vec![u32::from(d) + 1, 0b1001]).unwrap();
        let odd = Glyph::from_rows(4, vec![0b1111, 0b0110]).unwrap();
        let field = |pct: u16| [pct / 1000, pct / 100 % 10, pct / 10 % 10, pct % 10].map(shape);
        let mut climb = Climb::default();
        let first = field(9206);
        let _ = climb.add(&[odd, first[1].clone()], &first[2..]);
        let named = (9207..9999).find_map(|pct| {
            let d = field(pct);
            climb.add(&d[..2], &d[2..]).ok().flatten()
        });
        let names = named.expect("named despite the odd shape");
        for (glyph, name) in climb.shapes.iter().zip(names.chars()) {
            assert_eq!(*glyph, shape(name.to_digit(10).unwrap() as u16));
        }
    }

    // Started late in a level, the HUD only ever shows the percent. Fails if its climb doesn't
    // name the digits within a few whole percents of grinding, if a reading made with them is
    // wrong, if a level or an EXP is made up before the full field shows, or if the full field
    // at the next level doesn't bring them.
    #[test]
    fn learns_the_digits_from_the_percent_climbing_alone() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let needed_58 = 1_087_536;
        let start = 1_007_166;
        let mut exp = start;
        let mut first = None;
        for i in 0..600 {
            exp += [0, 270, 0, 310, 120, 0, 290][i % 7];
            let pct = pct_of(exp, needed_58);
            match reader.observe(&hud.draw_pct(58, pct).frame()) {
                Err(Unreadable::Learning) => assert_eq!(first, None, "frame {i}"),
                Ok(r) => {
                    assert!(r.confirmed, "{r:?}");
                    assert_eq!(
                        (r.level, r.exp, r.exp_shown, r.pct_hundredths, r.needed),
                        (None, None, false, pct, None),
                        "no level or EXP to work out without the full field"
                    );
                    assert!(events_between(None, &r).is_empty());
                    first.get_or_insert(i);
                }
                other => panic!("frame {i}: {other:?}"),
            }
            if first.is_some_and(|f| i > f + 30) {
                break;
            }
        }
        assert!(first.is_some(), "named by the climb");
        let climbed = pct_of(exp, needed_58) - pct_of(start, needed_58);
        assert!(
            climbed < 400,
            "named within a few whole percents, took {climbed}"
        );
        assert_eq!(reader.calibration(), Calibration::Confirmed);
        assert_eq!(
            reader.missing().exp,
            [','],
            "every digit, and no separator yet"
        );
        // The next level shows the full field, separator and all: the table's level, which the
        // tiles learn.
        let needed_59 = 1_147_132;
        let up = reader
            .observe(&hud.draw_values(59, 4_000, pct_of(4_000, needed_59)).frame())
            .unwrap();
        assert_eq!(values(&up), (Some(59), 4_000, pct_of(4_000, needed_59)));
        assert_eq!(up.needed, Some(needed_59));
        assert!(reader.missing().exp.is_empty());
        assert!(!reader.missing().level.contains(&'9'));
    }

    // A death costing under 1% within one whole percent reads as a rise: here 92.91% to 92.01%
    // shows the 9 below the 0. With every other step seen once, that alone would put the 0 above
    // the 9 and name every digit wrong, and the climb after it agrees. Fails if a step seen once
    // names anything.
    #[test]
    fn a_fall_within_one_whole_percent_never_names_a_digit_wrong() {
        let shape = |d: u16| Glyph::from_rows(4, vec![u32::from(d) + 1, 0b1001]).unwrap();
        let mut climb = Climb::default();
        for pct in [
            9211, 9221, 9231, 9241, 9251, 9261, 9271, 9281, 9291, 9201, 9202, 9203, 9205,
        ] {
            let digits = [pct / 1000, pct / 100 % 10, pct / 10 % 10, pct % 10].map(shape);
            assert_eq!(climb.add(&digits[..2], &digits[2..]), Ok(None), "at {pct}");
        }
    }

    /// Random climbs of the percent alone, on invented shapes: gains of every size, and deaths
    /// that cost anything from 0.05% to 10%, a fall under 1% within one whole percent included.
    /// Fails if a climb ever names a digit wrong, or if most clean ones don't name them.
    #[test]
    fn a_climb_never_names_a_digit_wrong() {
        let shape = |d: u32| Glyph::from_rows(4, vec![d + 1, 0b1001, d ^ 0b0110]).unwrap();
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut random = move |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % n
        };
        let (mut clean, mut clean_named, mut named) = (0, 0, 0);
        for session in 0..2_000 {
            // Digit `d` is drawn as shape `face[d]`.
            let mut face: Vec<u32> = (0..10).collect();
            for i in (1..10).rev() {
                face.swap(i, random(i as u64 + 1) as usize);
            }
            let biggest_gain = [1, 3, 8, 20, 60][random(5) as usize];
            // A death one frame in this many; never at 0.
            let deaths = [0, 0, 40, 150][random(4) as usize];
            clean += usize::from(deaths == 0);
            let mut pct = random(10_000) as u16;
            let mut climb = Climb::default();
            for _ in 0..2_000 {
                if deaths > 0 && random(deaths) == 0 {
                    let cost = [5, 30, 70, 99, 100, 101, 500, 1_000][random(8) as usize];
                    pct = pct.saturating_sub(cost);
                } else {
                    pct += random(biggest_gain + 1) as u16;
                }
                if pct >= 10_000 {
                    // A level-up: the full field shows, and the climb starts over.
                    pct -= 10_000;
                    climb = Climb::default();
                }
                let digits: Vec<u32> = pct_text(pct)
                    .chars()
                    .filter_map(|c| c.to_digit(10))
                    .collect();
                let (whole, frac) = digits.split_at(digits.len() - 2);
                let drawn = |ds: &[u32]| {
                    ds.iter()
                        .map(|&d| shape(face[d as usize]))
                        .collect::<Vec<_>>()
                };
                if let Some(names) = climb.add(&drawn(whole), &drawn(frac)).unwrap() {
                    for (glyph, name) in climb.shapes.iter().zip(names.chars()) {
                        let digit = name.to_digit(10).unwrap();
                        assert_eq!(*glyph, shape(face[digit as usize]), "session {session}");
                    }
                    named += 1;
                    clean_named += usize::from(deaths == 0);
                    break;
                }
            }
        }
        assert!(
            clean_named * 10 >= clean * 9,
            "{clean_named} of {clean} clean climbs named"
        );
        assert!(
            named > clean_named,
            "climbs with deaths name the digits too"
        );
    }

    #[test]
    fn the_percent_alone_bounds_the_exp() {
        for needed in [15, NEEDED_41, 1_087_536] {
            for exp in [0, 1, 7, needed / 3, needed - 1] {
                let pct = pct_of(exp, needed);
                let (lo, hi) = exp_range(pct, needed).unwrap();
                assert!((lo..=hi).contains(&exp), "{exp} of {needed}");
                assert!(pct_of(lo, needed) == pct && pct_of(hi, needed) == pct);
                assert!(lo == 0 || pct_of(lo - 1, needed) != pct);
                assert!(hi == needed - 1 || pct_of(hi + 1, needed) != pct);
            }
        }
        assert_eq!(exp_range(1, 15), None, "no EXP of 15 shows 0.01%");
        assert_eq!(
            exp_range(10_000, 15),
            None,
            "nor 100.00%: 14 of 15 is 93.33%"
        );
        assert_eq!(exp_range(10_000, 1_087_536), Some((1_087_482, 1_087_535)));
    }

    #[test]
    fn the_rounding_rule_bounds_exp_needed() {
        let (lo, hi) = needed_range(126_987, 3916).unwrap();
        assert!((lo..=hi).contains(&NEEDED_41));
        assert!(!fits(126_987, 3915, NEEDED_41) && fits(126_987, 3916, NEEDED_41));
        assert_eq!(needed_range(0, 0), Some((1, u64::MAX)));
        assert_eq!(needed_range(1, 100), Some((100, 100)));
        assert_eq!(needed_range(5, 0), Some((100_001, u64::MAX)));
        assert_eq!(needed_range(0, 5), None, "0 EXP can't show 0.05%");
        for exp in [1, 7, 1_000, 128_555, 324_239] {
            assert!(fits(exp, pct_of(exp, NEEDED_41), NEEDED_41), "{exp}");
        }
    }

    #[test]
    fn the_solver_needs_several_different_fields() {
        // Shapes named by their own digits, so the right answer is the identity.
        let observe = |exp: u64| {
            let text = exp_text(exp, pct_of(exp, NEEDED_41));
            let (number, pct) = text.split_once('[').unwrap();
            let (whole, frac) = pct.trim_end_matches("%]").split_once('.').unwrap();
            let digits = |s: &str| {
                s.chars()
                    .filter(char::is_ascii_digit)
                    .map(|c| c as usize - '0' as usize)
                    .collect()
            };
            Observation {
                exp: digits(number),
                whole: digits(whole),
                frac: digits(frac),
            }
        };
        let one = [observe(128_027)];
        assert_eq!(
            solve(&one, 10, false, 2).len(),
            2,
            "one field fits many namings"
        );
        let many: Vec<Observation> = GRIND.iter().map(|&e| observe(e)).collect();
        // The shapes here are numbered by digit, not first appearance; the search still works.
        let found = solve(&many, 10, false, 2);
        assert_eq!(found, vec![(0..10).collect::<Vec<u8>>()]);
    }

    /// Random sessions: random levels, starting EXP and gains, with deaths and level-ups, and in
    /// a fifth of them an EXP needed the table gets wrong. No confirmed reading may ever be wrong.
    /// Slow in a debug build; run it with
    /// `cargo test -p mesovibe-engine --release -- --ignored stress`.
    #[test]
    #[ignore]
    fn stress_never_confirms_a_wrong_reading() {
        let hud = Hud::new();
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rand = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let (mut confirmed_sessions, mut frames_to_confirm, sessions) = (0, 0, 400);
        let (mut unconfirmed, mut unconfirmed_wrong) = (0, 0);
        for _ in 0..sessions {
            let mut level = 10 + rand(39) as u16;
            let wrong_table = rand(5) == 0;
            let needed_at = |level: u16| {
                let n = exp_table::needed(level).unwrap();
                if wrong_table { n + n / 13 } else { n }
            };
            let mut needed = needed_at(level);
            let mut exp = rand(needed * 9 / 10);
            let mut reader = ExpBarReader::new();
            let mut confirmed_at = None;
            for frame in 0..80 {
                match rand(40) {
                    0 => exp -= exp / 10, // a death
                    1..=4 => {}           // no gain this frame
                    _ => exp += 1 + rand(needed / 3000 + 2),
                }
                if exp >= needed {
                    exp -= needed;
                    level += 1;
                    needed = needed_at(level);
                }
                let pct = pct_of(exp, needed);
                let result = reader.observe(&hud.draw_values(level, exp, pct).frame());
                if let Ok(r) = result
                    && !r.confirmed
                {
                    unconfirmed += 1;
                    if (r.exp, r.pct_hundredths) != (Some(exp), pct) {
                        unconfirmed_wrong += 1;
                    }
                }
                if let Ok(r) = result
                    && r.confirmed
                {
                    confirmed_at.get_or_insert(frame);
                    assert_eq!((r.exp, r.pct_hundredths), (Some(exp), pct), "level {level}");
                    if let Some(l) = r.level {
                        assert_eq!(l, level, "exp {exp}");
                    }
                    if let Some(n) = r.needed {
                        assert_eq!(n, needed);
                    }
                }
            }
            if let Some(at) = confirmed_at {
                confirmed_sessions += 1;
                frames_to_confirm += at;
            }
        }
        eprintln!(
            "{confirmed_sessions} of {sessions} sessions confirmed, after {} frames on average; \
             {unconfirmed_wrong} of {unconfirmed} unconfirmed readings were wrong",
            frames_to_confirm / confirmed_sessions.max(1)
        );
        assert!(
            confirmed_sessions * 10 >= sessions * 9,
            "most sessions confirm"
        );
    }

    // Typed calibration, the fallback.

    #[test]
    fn formats_the_exp_field_the_way_the_hud_does() {
        assert_eq!(exp_text(126_987, 3916), "126,987[39.16%]");
        assert_eq!(exp_text(7, 5), "7[0.05%]");
        assert_eq!(exp_text(1_000, 10_000), "1,000[100.00%]");
        assert_eq!(exp_text(999, 0), "999[0.00%]");
    }

    #[test]
    fn parses_only_what_the_hud_could_draw() {
        assert_eq!(
            parse_exp_text("126,987[39.16%]"),
            Some((Some(126_987), 3916))
        );
        assert_eq!(parse_exp_text("7[0.05%]"), Some((Some(7), 5)));
        // Late in a level, the percent alone.
        assert_eq!(parse_exp_text("[92.61%]"), Some((None, 9261)));
        assert_eq!(parse_exp_text("[100.00%]"), Some((None, 10_000)));
        for bad in [
            "12,34[1.00%]",
            "1234[1.00%]",
            "01[1.00%]",
            "1[1.5%]",
            "1[100.01%]",
            "1[1.00%",
            "1,[1.00%]",
            ",[1.00%]",
            "1[.00%]",
            "[092.61%]",
            "[92.6%]",
            "[100.01%]",
        ] {
            assert_eq!(parse_exp_text(bad), None, "{bad}");
        }
        assert_eq!(parse_level("41"), Some(41));
        assert_eq!(parse_level("041"), None);
        assert_eq!(parse_level("0"), None);
    }

    #[test]
    fn typed_calibration_reads_at_every_native_size() {
        let hud = Hud::new();
        let reader = typed(&hud);
        assert!(reader.missing().exp.is_empty(), "{:?}", reader.missing());
        assert_eq!(reader.calibration(), Calibration::Confirmed);
        let r = TypedReading {
            level: 45,
            exp: 98_765_432,
            pct_hundredths: 10_000,
        };
        for (w, h) in [(800, 600), (1024, 768), (1280, 720), (1366, 768)] {
            let got = reader.read(&hud.draw_typed(w, h, r).frame()).unwrap();
            assert_eq!(values(&got), (Some(45), r.exp, r.pct_hundredths), "{w}x{h}");
            assert!(got.confirmed);
        }
    }

    #[test]
    fn a_typed_level_unknown_to_the_tiles_falls_back_to_the_table() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        reader
            .calibrate(&hud.draw_typed(800, 600, FIRST).frame(), FIRST)
            .unwrap();
        // Read at another size: the HUD's pixels don't change with the size.
        let got = reader
            .read(&hud.draw_typed(1366, 768, FIRST).frame())
            .unwrap();
        assert_eq!((got.level, got.needed), (Some(41), Some(NEEDED_41)));
        // Tiles in a face the reader hasn't seen: the table still knows 41.
        let relevelled = Hud {
            level_font: Font::new(LEVEL_CHARS, 9),
            ..Hud::new()
        };
        let got = reader
            .read(&relevelled.draw_typed(1366, 768, FIRST).frame())
            .unwrap();
        assert_eq!(got.level, Some(41));
    }

    #[test]
    fn nothing_reads_before_calibration() {
        let hud = Hud::new();
        let frame = hud.draw_typed(1366, 768, FIRST);
        assert_eq!(
            ExpBarReader::new().read(&frame.frame()),
            Err(Unreadable::NotCalibrated)
        );
    }

    /// Stretched Full Screen sizes the tests cover.
    const STRETCHED: [(u32, u32); 3] = [(1920, 1200), (2560, 1440), (1920, 1080)];

    #[test]
    fn calibrates_itself_at_a_stretched_size() {
        let hud = Hud::new();
        for (w, h) in STRETCHED {
            let picture = unstretch::picture_width(w, h, MAX_NATIVE_HEIGHT);
            let mut reader = ExpBarReader::new();
            let results: Vec<_> = GRIND
                .iter()
                .map(|&exp| {
                    let field = exp_text(exp, pct_of(exp, NEEDED_41));
                    let canvas = stretched(&hud.draw(picture, 768, &field, "41"), w, h);
                    reader.observe(&canvas.frame())
                })
                .collect();
            assert_eq!(reader.calibration(), Calibration::Confirmed, "{w}x{h}");
            for (r, &exp) in results.iter().zip(&GRIND).filter(|(r, _)| r.is_ok()) {
                let r = r.as_ref().unwrap();
                assert_eq!(
                    values(r),
                    (Some(41), exp, pct_of(exp, NEEDED_41)),
                    "{w}x{h}"
                );
            }
        }
    }

    // Full Screen scales the size the player chose: 1280x720 fills the same 1512x850 picture as
    // 1366x768 on a 14-inch MacBook Pro, and 800x600 fills 4:3 between side bars. Fails if the
    // smaller picture isn't found from the HUD, a 768-line one is taken for another size, or the
    // HUD is looked for elsewhere after a reading, while learning or on a dark screen.
    #[test]
    fn finds_the_size_full_screen_scaled_up_from_the_hud() {
        let hud = Hud::new();
        let reader = typed(&hud);
        let expected = (Some(41), FIRST.exp, FIRST.pct_hundredths);
        for ((width, lines), (to_width, to_height)) in
            [((1280, 720), (1512, 850)), ((800, 600), (1308, 982))]
        {
            let canvas = stretched(&hud.draw_typed(width, lines, FIRST), to_width, to_height);
            let frame = canvas.frame();
            assert!(
                reader.read(&frame).is_err(),
                "{width}x{lines} read as 768 lines"
            );
            let missed = Err(Unreadable::HudNotFound);
            assert_eq!(
                other_source(&frame, &missed),
                Some(lines),
                "{width}x{lines}"
            );
            assert_eq!(
                values(&reader.read(&frame.with_source(lines)).unwrap()),
                expected,
                "{width}x{lines}"
            );
            assert_eq!(hud_source(&frame.with_source(lines)), None, "already there");
            for reading in [
                reader.read(&frame.with_source(lines)),
                Err(Unreadable::Learning),
                Err(Unreadable::ScreenDark),
            ] {
                assert_eq!(other_source(&frame, &reading), None, "{reading:?}");
            }
        }
        let usual = stretched(&hud.draw_typed(1366, 768, FIRST), 1512, 850);
        assert_eq!(hud_source(&usual.frame()), None, "1366x768 stays 768 lines");
        // 800x600 between side bars on a 1512x982 display: the bars are centered, so the area
        // reads as a wider 600-line picture with black at its edges.
        let small = stretched(&hud.draw_typed(800, 600, FIRST), 1308, 982);
        let mut barred = Canvas::new(1512, 982);
        barred.fill(0, 0, 1512, 982, [0, 0, 0]);
        for (y, row) in small.bgra.chunks(1308 * 4).enumerate() {
            let at = (y * 1512 + 102) * 4;
            barred.bgra[at..at + row.len()].copy_from_slice(row);
        }
        assert_eq!(hud_source(&barred.frame()), Some(600));
        assert_eq!(
            values(&reader.read(&barred.frame().with_source(600)).unwrap()),
            expected
        );
    }

    #[test]
    fn what_is_learned_at_one_size_reads_every_size() {
        let hud = Hud::new();
        let expected = (Some(41), FIRST.exp, FIRST.pct_hundredths);
        // Learned at 1x, read stretched: the player changed the window size.
        let at_1x = typed(&hud);
        for (w, h) in STRETCHED {
            let picture = unstretch::picture_width(w, h, MAX_NATIVE_HEIGHT);
            let frame = stretched(&hud.draw_typed(picture, 768, FIRST), w, h);
            assert_eq!(
                values(&at_1x.read(&frame.frame()).unwrap()),
                expected,
                "{w}x{h}"
            );
        }
        // Learned stretched (here by the typed fallback), read at 1x.
        let mut at_large = ExpBarReader::new();
        for r in [FIRST, SECOND] {
            let frame = stretched(&hud.draw_typed(1228, 768, r), 1920, 1200);
            at_large.calibrate(&frame.frame(), r).unwrap();
        }
        let frame = hud.draw_typed(1366, 768, FIRST);
        assert_eq!(values(&at_large.read(&frame.frame()).unwrap()), expected);
    }

    #[test]
    fn reads_a_frame_holding_only_the_rows_it_needs() {
        let hud = Hud::new();
        let reader = typed(&hud);
        let expected = (Some(41), FIRST.exp, FIRST.pct_hundredths);
        let at_1x = hud.draw_typed(1366, 768, FIRST);
        let picture = unstretch::picture_width(1920, 1080, MAX_NATIVE_HEIGHT);
        let large = stretched(&hud.draw_typed(picture, 768, FIRST), 1920, 1080);
        for canvas in [at_1x, large] {
            let (w, h) = (canvas.width, canvas.height);
            let first = h - rows_needed(h, MAX_NATIVE_HEIGHT);
            let stride = w as usize * 4;
            let strip = &canvas.bgra[first as usize * stride..];
            let frame = Frame::bottom(w, h, first, stride, strip).unwrap();
            assert_eq!(values(&reader.read(&frame).unwrap()), expected, "{w}x{h}");
            // One row fewer is not enough, and says so rather than misreading.
            let short = Frame::bottom(w, h, first + 1, stride, &strip[stride..]).unwrap();
            assert!(reader.read(&short).is_err(), "{w}x{h}");
        }
    }

    #[test]
    fn a_stretched_frame_without_the_hud_is_unreadable() {
        let reader = typed(&Hud::new());
        let mut scene = Canvas::new(1920, 1200);
        scene.fill(0, 1200 - 80, 1920, 80, WHITE);
        assert_eq!(reader.read(&scene.frame()), Err(Unreadable::HudNotFound));
        let dark = Canvas::new(1920, 1200);
        assert_eq!(reader.read(&dark.frame()), Err(Unreadable::ScreenDark));
    }

    /// `canvas` dimmed to `percent` of its brightness, the way the game fades to black.
    fn dimmed(canvas: &Canvas, percent: u32) -> Canvas {
        let mut out = Canvas::new(canvas.width, canvas.height);
        for (o, i) in out.bgra.chunks_mut(4).zip(canvas.bgra.chunks(4)) {
            for c in 0..3 {
                o[c] = (u32::from(i[c]) * percent / 100) as u8;
            }
            o[3] = i[3];
        }
        out
    }

    // A recorded Mac run's map-change frames were black or dimmed to 10-80%. Fails if a fade comes
    // out HudNotFound, or a covered or moved HUD with light around it comes out ScreenDark.
    #[test]
    fn a_screen_the_game_dimmed_is_dark_but_a_covered_hud_is_not() {
        let hud = Hud::new();
        let mut reader = typed(&hud);
        let drawn = hud.draw_typed(1366, 768, FIRST);
        for percent in [0, 10, 50] {
            let frame = dimmed(&drawn, percent);
            assert_eq!(
                reader.read(&frame.frame()),
                Err(Unreadable::ScreenDark),
                "{percent}%"
            );
            assert_eq!(
                reader.observe(&frame.frame()),
                Err(Unreadable::ScreenDark),
                "{percent}%"
            );
        }
        // Barely dimmed, the HUD still reads, and reads right.
        for percent in [80, 97] {
            let frame = dimmed(&drawn, percent);
            let r = reader.read(&frame.frame()).map(|r| values(&r));
            assert_eq!(
                r,
                Ok((Some(41), FIRST.exp, FIRST.pct_hundredths)),
                "{percent}%"
            );
        }
        // Covered, or moved too far: the white around it says the game is drawing.
        let mut covered = hud.draw_typed(1366, 768, FIRST);
        covered.fill(283 + 430, 768 - 34, 40, 14, DARK);
        assert_eq!(reader.read(&covered.frame()), Err(Unreadable::HudNotFound));
        let moved = hud.draw_shifted(
            1366,
            768,
            &exp_text(FIRST.exp, FIRST.pct_hundredths),
            "41",
            7,
        );
        assert_eq!(reader.read(&moved.frame()), Err(Unreadable::HudNotFound));
        // And it reads again as soon as the fade is over.
        assert_eq!(
            reader.observe(&drawn.frame()).map(|r| values(&r)),
            Ok((Some(41), FIRST.exp, FIRST.pct_hundredths))
        );
    }

    #[test]
    fn sizes_too_narrow_for_the_hud_are_unsupported() {
        let mut reader = typed(&Hud::new());
        // 640x480 at 1x, and 1000x1200, which stretches a 640x768 picture.
        for (w, h) in [(640, 480), (1000, 1200)] {
            let canvas = Canvas::new(w, h);
            let unsupported = Unreadable::UnsupportedSize {
                width: w,
                height: h,
            };
            assert_eq!(reader.read(&canvas.frame()), Err(unsupported));
            assert_eq!(reader.observe(&canvas.frame()), Err(unsupported));
            assert_eq!(
                ExpBarReader::new().calibrate(&canvas.frame(), FIRST),
                Err(CalibrationError::UnsupportedSize {
                    width: w,
                    height: h
                })
            );
        }
    }

    // The fixture suite's changed-UI cases: a shifted or re-fonted HUD must come out
    // unreadable, not wrong. A HUD within a pixel of its place is the same HUD placed by Wine
    // (see `a_hud_a_pixel_off_reads_and_further_off_is_unreadable`).
    #[test]
    fn a_changed_ui_is_unreadable_never_a_wrong_value() {
        let hud = Hud::new();
        let reader = typed(&hud);
        for shift in [2, 7] {
            let shifted = hud.draw_shifted(
                1366,
                768,
                &exp_text(FIRST.exp, FIRST.pct_hundredths),
                "41",
                shift,
            );
            assert_eq!(
                reader.read(&shifted.frame()),
                Err(Unreadable::HudNotFound),
                "shift {shift}"
            );
        }
        let refonted = Hud {
            exp_font: Font::new(EXP_CHARS, 3),
            ..Hud::new()
        };
        assert_eq!(
            reader.read(&refonted.draw_typed(1366, 768, FIRST).frame()),
            Err(Unreadable::UnknownGlyph(Field::Exp))
        );
    }

    #[test]
    fn a_covered_hud_or_garbled_text_is_unreadable() {
        let hud = Hud::new();
        let reader = typed(&hud);
        let mut covered = hud.draw_typed(1366, 768, FIRST);
        covered.fill(283 + 430, 768 - 34, 40, 14, DARK);
        assert_eq!(reader.read(&covered.frame()), Err(Unreadable::HudNotFound));

        let garbled = hud.draw(1366, 768, "12,34[1.00%]", "41");
        assert_eq!(
            reader.read(&garbled.frame()),
            Err(Unreadable::Garbled(Field::Exp))
        );

        // No level tiles and no table level for 1 EXP at 1.00%: the level is unknown, not wrong.
        let no_level = reader
            .read(&hud.draw(1366, 768, "1[1.00%]", "").frame())
            .unwrap();
        assert_eq!(values(&no_level), (None, 1, 100));

        let mut barred = hud.draw_typed(1366, 768, FIRST);
        barred.fill(283 + 470, 768 - 26, 60, 2, WHITE);
        assert_eq!(
            reader.read(&barred.frame()),
            Err(Unreadable::Garbled(Field::Exp))
        );
    }

    #[test]
    fn typed_calibration_refuses_typos_and_changes_nothing() {
        let hud = Hud::new();
        let frame = hud.draw_typed(1366, 768, FIRST);
        let mut reader = ExpBarReader::new();
        let typo = TypedReading {
            exp: 1_269_870,
            ..FIRST
        };
        assert!(matches!(
            reader.calibrate(&frame.frame(), typo),
            Err(CalibrationError::Mismatch(
                Field::Exp,
                LearnError::CountMismatch { .. }
            ))
        ));
        // 126,987 typed as 126,977: two shapes would both be '7'.
        let typo = TypedReading {
            exp: 126_977,
            ..FIRST
        };
        assert!(matches!(
            reader.calibrate(&frame.frame(), typo),
            Err(CalibrationError::Mismatch(
                Field::Exp,
                LearnError::NameUsedTwice { name: '7' }
            ))
        ));
        // The EXP is right but the level is wrong: nothing is learned.
        let typo = TypedReading { level: 4, ..FIRST };
        assert!(matches!(
            reader.calibrate(&frame.frame(), typo),
            Err(CalibrationError::Mismatch(
                Field::Level,
                LearnError::CountMismatch { .. }
            ))
        ));
        assert_eq!(reader.calibration(), Calibration::Learning);
        assert_eq!(reader.missing().exp.len(), EXP_CHARS.len());
        assert_eq!(
            reader.calibrate(&frame.frame(), TypedReading { level: 0, ..FIRST }),
            Err(CalibrationError::ImplausibleReading)
        );
        assert_eq!(
            reader.calibrate(
                &frame.frame(),
                TypedReading {
                    pct_hundredths: 10_001,
                    ..FIRST
                }
            ),
            Err(CalibrationError::ImplausibleReading)
        );
    }

    #[test]
    fn typed_calibration_needs_the_hud_and_notices_when_it_changed() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        assert_eq!(
            reader.calibrate(&Canvas::new(1366, 768).frame(), FIRST),
            Err(CalibrationError::HudNotFound)
        );
        reader
            .calibrate(&hud.draw_typed(1366, 768, FIRST).frame(), FIRST)
            .unwrap();
        let shifted = hud.draw_shifted(
            1366,
            768,
            &exp_text(SECOND.exp, SECOND.pct_hundredths),
            "50",
            2,
        );
        assert_eq!(
            reader.calibrate(&shifted.frame(), SECOND),
            Err(CalibrationError::HudChanged)
        );
    }

    // Where the HUD sits: Wine on a Mac places the game up to a point off.

    /// `canvas` with everything moved right by `dx` and down by `dy`, dark where nothing reaches.
    fn moved(canvas: &Canvas, dx: i32, dy: i32) -> Canvas {
        let mut out = Canvas::new(canvas.width, canvas.height);
        for y in 0..canvas.height {
            for x in 0..canvas.width {
                let (sx, sy) = (x as i32 - dx, y as i32 - dy);
                if sx >= 0 && sy >= 0 && (sx as u32) < canvas.width && (sy as u32) < canvas.height {
                    let i = ((sy as u32 * canvas.width + sx as u32) * 4) as usize;
                    let o = ((y * canvas.width + x) * 4) as usize;
                    out.bgra[o..o + 4].copy_from_slice(&canvas.bgra[i..i + 4]);
                }
            }
        }
        out
    }

    #[test]
    fn a_hud_a_pixel_off_reads_and_further_off_is_unreadable() {
        let hud = Hud::new();
        let reader = typed(&hud);
        let expected = (Some(41), FIRST.exp, FIRST.pct_hundredths);
        let drawn = hud.draw_typed(1366, 768, FIRST);
        for (dx, dy) in OFFSETS {
            let frame = moved(&drawn, dx, dy);
            let r = reader.read(&frame.frame());
            assert_eq!(r.map(|r| values(&r)), Ok(expected), "moved {dx}, {dy}");
        }
        for (dx, dy) in [(2, 0), (-2, 0), (0, 2), (0, -2), (2, 2)] {
            let frame = moved(&drawn, dx, dy);
            assert_eq!(
                reader.read(&frame.frame()),
                Err(Unreadable::HudNotFound),
                "moved {dx}, {dy}"
            );
        }
    }

    #[test]
    fn calibrates_itself_on_a_hud_a_row_off_and_follows_it_when_it_moves() {
        let hud = Hud::new();
        for dy in [-1, 1] {
            let mut reader = ExpBarReader::new();
            for &exp in &GRIND {
                let drawn = hud.draw_values(41, exp, pct_of(exp, NEEDED_41));
                if let Ok(r) = reader.observe(&moved(&drawn, 0, dy).frame()) {
                    assert_eq!(values(&r), (Some(41), exp, pct_of(exp, NEEDED_41)), "{dy}");
                }
            }
            assert_eq!(reader.calibration(), Calibration::Confirmed, "row {dy}");
            // The game was restarted and Wine placed it a pixel differently.
            let exp = *GRIND.last().unwrap();
            let drawn = hud.draw_values(41, exp, pct_of(exp, NEEDED_41));
            for (dx, dy) in [(0, 0), (1, 0), (1, 1)] {
                let r = reader.observe(&moved(&drawn, dx, dy).frame()).unwrap();
                assert_eq!(values(&r), (Some(41), exp, pct_of(exp, NEEDED_41)));
            }
        }
    }

    // A Retina Mac: every game pixel is a 2x2 block of identical screen pixels.

    /// The bottom pixel rows a backend reads back of `canvas` shown at backing scale 2: the
    /// blocks start `phase` pixels in, and Wine placed the game `shift` game pixels right and
    /// down, leaving black where the game doesn't reach. Returns the strip and its first row.
    fn doubled_strip(canvas: &Canvas, phase: (u32, u32), shift: (i32, i32)) -> (Vec<u8>, u32) {
        let (w, h) = (canvas.width * 2, canvas.height * 2);
        let first = h - unstretch::scaled_rows(rows_needed(canvas.height, MAX_NATIVE_HEIGHT), 2);
        let mut bgra = vec![0u8; ((h - first) * w * 4) as usize];
        for y in first..h {
            for x in 0..w {
                let gx = (x as i32 - phase.0 as i32).div_euclid(2) - shift.0;
                let gy = (y as i32 - phase.1 as i32).div_euclid(2) - shift.1;
                let o = (((y - first) * w + x) * 4) as usize;
                bgra[o + 3] = 255;
                if (0..canvas.width as i32).contains(&gx) && (0..canvas.height as i32).contains(&gy)
                {
                    let i = ((gy as u32 * canvas.width + gx as u32) * 4) as usize;
                    bgra[o..o + 4].copy_from_slice(&canvas.bgra[i..i + 4]);
                }
            }
        }
        (bgra, first)
    }

    fn retina<'a>(canvas: &Canvas, strip: &'a (Vec<u8>, u32)) -> Frame<'a> {
        let (w, h) = (canvas.width * 2, canvas.height * 2);
        Frame::bottom(w, h, strip.1, w as usize * 4, &strip.0)
            .and_then(|f| f.with_scale(2))
            .unwrap()
    }

    // Fails if unscale misreads a phase, the row search misses the HUD a point lower, or
    // rows_needed is too short for a scaled strip. Uses only the rows a backend reads back.
    #[test]
    fn calibrates_itself_on_retina_frames_wherever_wine_puts_the_game() {
        let hud = Hud::new();
        // Recorded runs: 1366x768 (half a point lower), 1280x720 (none), 1024x768 (a point right
        // and down), and 1366x768 on 2026-11-03 (half a point right).
        let placements = [
            ((0, 1), (0, 0)),
            ((0, 0), (0, 0)),
            ((0, 0), (1, 1)),
            ((1, 0), (0, 0)),
        ];
        for (phase, shift) in placements {
            let mut reader = ExpBarReader::new();
            let mut confirmed = 0;
            for &exp in &GRIND {
                let canvas = hud.draw_values(41, exp, pct_of(exp, NEEDED_41));
                let strip = doubled_strip(&canvas, phase, shift);
                if let Ok(r) = reader.observe(&retina(&canvas, &strip)) {
                    assert_eq!(
                        values(&r),
                        (Some(41), exp, pct_of(exp, NEEDED_41)),
                        "{phase:?}"
                    );
                    confirmed += usize::from(r.confirmed);
                }
            }
            assert_eq!(
                reader.calibration(),
                Calibration::Confirmed,
                "{phase:?} {shift:?}"
            );
            // As soon as at 1x (`learns_the_digits_with_no_typing_as_exp_moves`).
            assert!(
                GRIND.len() - confirmed < 10,
                "{phase:?} {shift:?}: {confirmed}"
            );
        }
    }

    #[test]
    fn a_retina_frame_of_a_stretched_size_reads_through_both_steps() {
        let hud = Hud::new();
        let reader = typed(&hud);
        // A 1920x1080 game area on a Retina display: the game's own stretch, then the doubling.
        let picture = unstretch::picture_width(1920, 1080, MAX_NATIVE_HEIGHT);
        let canvas = stretched(&hud.draw_typed(picture, 768, FIRST), 1920, 1080);
        let strip = doubled_strip(&canvas, (1, 0), (0, 0));
        assert_eq!(
            reader.read(&retina(&canvas, &strip)).map(|r| values(&r)),
            Ok((Some(41), FIRST.exp, FIRST.pct_hundredths))
        );
    }

    #[test]
    fn a_declared_scale_that_is_not_pixel_doubling_is_unsupported() {
        let reader = typed(&Hud::new());
        // A 1x HUD declared as 2x: its 2x2 blocks aren't one color.
        let canvas = Hud::new().draw_typed(1366, 768, FIRST);
        let frame = canvas.frame().with_scale(2).unwrap();
        assert_eq!(
            reader.read(&frame),
            Err(Unreadable::UnsupportedScale { scale: 2 })
        );
    }

    #[test]
    fn events_follow_confirmed_changes_and_level_ups() {
        let a = ExpReading {
            level: Some(41),
            exp: Some(126_987),
            exp_shown: true,
            pct_hundredths: 3916,
            needed: Some(NEEDED_41),
            confirmed: true,
        };
        assert_eq!(events_between(Some(&a), &a), vec![]);
        let gained = ExpReading {
            exp: Some(127_008),
            ..a
        };
        assert_eq!(
            events_between(Some(&a), &gained),
            vec![Event::ExpChanged {
                level: Some(41),
                exp: 127_008,
                exp_shown: true,
                pct_hundredths: 3916,
                needed: Some(NEEDED_41)
            }]
        );
        let up = ExpReading {
            level: Some(42),
            exp: Some(3),
            pct_hundredths: 0,
            needed: Some(356_916),
            ..a
        };
        assert!(events_between(Some(&gained), &up).contains(&Event::LevelUp { level: 42 }));
        assert_eq!(
            events_between(None, &a).len(),
            1,
            "the first reading is a change, not a level-up"
        );
        let unconfirmed = ExpReading {
            confirmed: false,
            ..gained
        };
        assert!(events_between(Some(&a), &unconfirmed).is_empty());
    }
}
