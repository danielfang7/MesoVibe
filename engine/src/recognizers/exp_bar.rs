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

use super::exp_table;
use crate::events::Event;
use crate::frame::Frame;
use crate::glyph::{Atlas, Glyph, LearnError, Rect, cut, segment};
use crate::unstretch;

/// The HUD bar's width at 1x.
const HUD_WIDTH: u32 = 800;
/// How far, in game pixels each way, the HUD may sit from where the game area's size puts it.
/// Wine on a Mac places the game up to a point off.
const MAX_SHIFT: u32 = 1;
/// The tallest game area drawn at 1x. Above it the game stretches a 768-line image.
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
/// The EXP total and percent, for example `126,987[39.16%]`. It stops before the CASH SHOP
/// button's art at dx 566, which leaves room for an 8-digit total.
const EXP_TEXT: Area = Area {
    dx: 465,
    up: 28,
    width: 100,
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
/// the frame itself up to 768 lines, or else the HUD's rows of the picture the game stretched.
/// Fails when the HUD can't be placed: narrower than the bar, a picture that can't be recovered,
/// or a frame missing rows the reader needs.
fn at_1x<T>(frame: &Frame, read: impl FnOnce(&Frame, Layout) -> T) -> Result<T, Unreadable> {
    let scale = frame.scale();
    if scale > 1 {
        // Only the rows a backend reads back: the game's picture is doubled there, but not
        // necessarily everywhere (a Mac title bar's shadow falls on the top rows).
        let rows = unstretch::scaled_rows(rows_needed(frame.height() / scale), scale);
        let picture = unstretch::unscale(&frame.bottom_rows(rows))
            .ok_or(Unreadable::UnsupportedScale { scale })?;
        return at_1x(&picture.frame(), read);
    }
    if frame.height() <= MAX_NATIVE_HEIGHT {
        let layout = Layout::of(frame).ok_or_else(|| unsupported(frame))?;
        // Without all of its rows, the HUD can't be looked for everywhere it may be.
        if frame.first_row() + rows_needed(frame.height()) > frame.height() {
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
/// pixels tall: all a capture backend needs to read back, in game rows. The HUD's rows
/// at 1x, or the rows the stretch recovery reads above 768 lines. A backend capturing at a
/// backing scale reads [`unstretch::scaled_rows`] of these.
pub fn rows_needed(height: u32) -> u32 {
    unstretch::rows_for(height, HUD_ROWS)
}

/// What the HUD showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpReading {
    /// From the level tiles once their digits are known, otherwise the table's level whose EXP
    /// needed fits the bar. `None` when neither can tell.
    pub level: Option<u16>,
    /// EXP into the current level.
    pub exp: u64,
    /// The percentage on the bar in hundredths: 3916 is 39.16%.
    pub pct_hundredths: u16,
    /// The EXP the whole level needs: the table's value for the level, when it fits the bar.
    pub needed: Option<u64>,
    /// False while the digits' names rest on the EXP table; true once the HUD alone confirmed
    /// them, or the player typed a reading.
    pub confirmed: bool,
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
    /// The level tiles' glyphs in the last frame `observe` read. They change at a level-up and
    /// not at a death, though the EXP falls at both.
    last_tiles: Option<Vec<Glyph>>,
    /// Where the HUD was last found, relative to where the game area's size puts it.
    offset: Offset,
    /// Started from saved knowledge ([`ExpBarReader::from_knowledge`]) that hasn't read the HUD
    /// yet this session: how many frames in a row its shapes didn't fit.
    restored: Option<u32>,
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

    /// Learns from a frame and reads it. Call it on every frame: while EXP moves, the reader
    /// names the digit shapes itself, and later it follows level-ups and learns digits it hadn't
    /// seen.
    pub fn observe(&mut self, frame: &Frame) -> Result<ExpReading, Unreadable> {
        at_1x(frame, |frame, nominal| {
            dark_if_hidden(self.observe_at(frame, nominal), frame, nominal)
        })
        .and_then(|r| r)
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

    fn observe_at(&mut self, frame: &Frame, nominal: Layout) -> Result<ExpReading, Unreadable> {
        let mut found = self.find_label(frame, nominal);
        if self.restored.is_some() && found.is_none() && find_by_structure(frame, nominal).is_some()
        {
            // The HUD is plainly there, but not with the label that was saved: the game's UI
            // changed since. Learn it again.
            *self = ExpBarReader::new();
            found = None;
        }
        let result = self.observe_found(frame, nominal, found);
        let Some(misses) = self.restored else {
            return result;
        };
        let missed = match result {
            // The saved shapes read the HUD and the bar fits the EXP table: they're right.
            Ok(reading) if reading.needed.is_some() => {
                self.restored = None;
                return Ok(reading);
            }
            // Read, but not provably right: shown unconfirmed, and no events.
            Ok(reading) => Ok(ExpReading {
                confirmed: false,
                ..reading
            }),
            Err(Unreadable::UnknownGlyph(_) | Unreadable::Garbled(_)) => result,
            // Covered, dark or elsewhere: no evidence either way.
            Err(_) => return result,
        };
        if misses + 1 >= RESTORE_PATIENCE {
            *self = ExpBarReader::new();
        } else {
            self.restored = Some(misses + 1);
        }
        missed
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
            && self.learn_unseen_digit(frame, layout)
        {
            result = self.read_at(frame, layout);
        }
        let mut reading = result?;
        self.follow_level(frame, layout, &mut reading);
        self.last = Some(reading);
        Ok(reading)
    }

    /// Reads a frame with what's been learned so far, learning nothing.
    pub fn read(&self, frame: &Frame) -> Result<ExpReading, Unreadable> {
        at_1x(frame, |frame, nominal| {
            let offset = self.find_label(frame, nominal).unwrap_or(self.offset);
            let result = match nominal.shifted(offset) {
                Some(layout) => self.read_at(frame, layout),
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
                if self.learner.observations.len() >= 2 {
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
        if !self.learner.observations.is_empty() && self.learner.tiles != tiles {
            // The level changed, and EXP needed with it: start the evidence over.
            self.keep_newest(0);
            self.learner.provisional = None;
            self.learner.candidate = None;
        }
        self.learner.tiles = tiles;

        let garbled = Unreadable::Garbled(Field::Exp);
        let (marks, names): (Vec<Glyph>, String) = parts.punctuation.into_iter().unzip();
        let mut digits = self.digits.clone();
        digits.learn(&marks, &names).map_err(|_| garbled)?;
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
        let level = read_level(frame, layout, &self.level_digits)
            .or_else(|| table_level(exp, pct_hundredths));
        Ok(ExpReading {
            level,
            exp,
            pct_hundredths,
            needed: level.and_then(|l| table_needed(l, exp, pct_hundredths)),
            confirmed: self.learner.confirmed,
        })
    }

    /// Learns a digit shape that turned up after calibration, when exactly one unnamed digit
    /// makes the bar fit the EXP needed of the last reading without the EXP falling.
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
        let mut fitting = Vec::new();
        for digit in self.digits.missing(DIGITS) {
            let mut atlas = self.digits.clone();
            if atlas
                .learn(std::slice::from_ref(glyph), &digit.to_string())
                .is_err()
            {
                continue;
            }
            let text: Option<String> = found.iter().map(|(_, g)| atlas.name(g)).collect();
            if let Some((exp, pct)) = text.as_deref().and_then(parse_exp_text)
                && exp >= last.exp
                && fits(exp, pct, needed)
            {
                fitting.push(atlas);
            }
        }
        match <[Atlas; 1]>::try_from(fitting) {
            Ok([atlas]) => {
                self.digits = atlas;
                true
            }
            Err(_) => false,
        }
    }

    /// Learns the level tiles' digits and follows level-ups. Unconfirmed readings teach nothing.
    fn follow_level(&mut self, frame: &Frame, layout: Layout, reading: &mut ExpReading) {
        let tiles = glyphs(frame, layout.rect(LEVEL_TEXT), level_ink);
        let tiles_changed = self.last_tiles.is_some() && self.last_tiles != tiles;
        if reading.confirmed && read_level(frame, layout, &self.level_digits).is_none() {
            let lesson = match self.last {
                // A level-up: the EXP fell and the tiles changed (at a death they don't).
                Some(ExpReading {
                    level: Some(level),
                    exp,
                    ..
                }) if reading.exp < exp && tiles_changed => level.checked_add(1),
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
            && fits(reading.exp, reading.pct_hundredths, needed)
        {
            reading.level = Some(level);
            reading.needed = Some(needed);
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

/// The table's EXP needed for `level`, when it fits the bar.
fn table_needed(level: u16, exp: u64, pct_hundredths: u16) -> Option<u64> {
    exp_table::needed(level).filter(|&n| fits(exp, pct_hundredths, n))
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
/// marks in the EXP, one every three digits from the right.
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
    // At least one EXP digit, then `[d.dd%]`.
    if open == 0 || close != found.len() - 1 || close < open + 6 {
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
    if exp.is_empty() || pattern != group_digits(&"0".repeat(exp.len())) {
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
        "{}[{}.{:02}%]",
        group_digits(&exp.to_string()),
        pct_hundredths / 100,
        pct_hundredths % 100
    )
}

/// The inverse of [`exp_text`], strictly: anything `exp_text` wouldn't produce is refused.
fn parse_exp_text(text: &str) -> Option<(u64, u16)> {
    let (number, rest) = text.split_once('[')?;
    let (whole, frac) = rest.strip_suffix("%]")?.split_once('.')?;
    let exp: u64 = number.replace(',', "").parse().ok()?;
    let pct = whole
        .parse::<u16>()
        .ok()?
        .checked_mul(100)?
        .checked_add(frac.parse().ok()?)?;
    (pct <= 10_000 && exp_text(exp, pct) == text).then_some((exp, pct))
}

fn parse_level(text: &str) -> Option<u16> {
    let level: u16 = text.parse().ok()?;
    (level > 0 && level.to_string() == text).then_some(level)
}

/// The events a new reading produces after the previous one: nothing if the HUD didn't change or
/// the reading is unconfirmed (so a correction never looks like a gain), `ExpChanged` if it did,
/// and `LevelUp` as well when the level rose.
pub fn events_between(previous: Option<&ExpReading>, current: &ExpReading) -> Vec<Event> {
    if !current.confirmed || previous == Some(current) {
        return Vec::new();
    }
    let mut events = vec![Event::ExpChanged {
        level: current.level,
        exp: current.exp,
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
        /// Every column holds ink in the top row, so segmentation never splits a glyph and no digit
        /// looks like punctuation. Rows 1-6 differ for every pair of characters in every column
        /// (37 and 63 share no factor), and `seed` gives a different face.
        pub(crate) fn new(chars: &str, seed: u32) -> Font {
            let glyphs = chars
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    let columns = match c {
                        ',' => vec![0b110_0000],
                        '.' => vec![0b100_0000],
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
        /// Varies the invented "EXP." label's pattern: a game update that redrew it.
        pub(crate) label: u32,
    }

    impl Hud {
        pub(crate) fn new() -> Hud {
            Hud {
                exp_font: Font::new(EXP_CHARS, 0),
                level_font: Font::new(LEVEL_CHARS, 7),
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

        pub(crate) fn draw_values(&self, level: u16, exp: u64, pct_hundredths: u16) -> Canvas {
            self.draw(
                1366,
                768,
                &exp_text(exp, pct_hundredths),
                &level.to_string(),
            )
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

    fn values(r: &ExpReading) -> (Option<u16>, u64, u16) {
        (r.level, r.exp, r.pct_hundredths)
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
            assert_eq!((r.exp, r.pct_hundredths), (exp, pct_of(exp, NEEDED_41)));
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
                assert_eq!((r.exp, r.pct_hundredths), (exp, pct_of(exp, needed)));
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
        assert_eq!((r.exp, r.pct_hundredths), (exp, pct_of(exp, NEEDED_41)));
        assert!(!reader.missing().exp.contains(&'7'));
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
                    if (r.exp, r.pct_hundredths) != (exp, pct) {
                        unconfirmed_wrong += 1;
                    }
                }
                if let Ok(r) = result
                    && r.confirmed
                {
                    confirmed_at.get_or_insert(frame);
                    assert_eq!((r.exp, r.pct_hundredths), (exp, pct), "level {level}");
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
        assert_eq!(parse_exp_text("126,987[39.16%]"), Some((126_987, 3916)));
        assert_eq!(parse_exp_text("7[0.05%]"), Some((7, 5)));
        for bad in [
            "12,34[1.00%]",
            "1234[1.00%]",
            "01[1.00%]",
            "1[1.5%]",
            "1[100.01%]",
            "1[1.00%",
            "1,[1.00%]",
            "[1.00%]",
            "1[.00%]",
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

    /// `canvas`, a 768-line picture, stretched to `width` x `height` the way the game does. Only
    /// the rows the HUD's recovery needs are drawn, to keep the tests quick.
    fn stretched(canvas: &Canvas, width: u32, height: u32) -> Canvas {
        assert_eq!(canvas.height, MAX_NATIVE_HEIGHT);
        let rgb: Vec<[u8; 3]> = canvas
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[2], p[1], p[0]])
            .collect();
        let from_row = height - (HUD_ROWS + 4) * height / MAX_NATIVE_HEIGHT;
        let bgra = unstretch::stretch(&rgb, canvas.width, width, height, from_row);
        Canvas {
            width,
            height,
            bgra,
        }
    }

    const STRETCHED: [(u32, u32); 3] = [(1920, 1200), (2560, 1440), (1920, 1080)];

    #[test]
    fn calibrates_itself_at_a_stretched_size() {
        let hud = Hud::new();
        for (w, h) in STRETCHED {
            let picture = unstretch::picture_width(w, h);
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

    #[test]
    fn what_is_learned_at_one_size_reads_every_size() {
        let hud = Hud::new();
        let expected = (Some(41), FIRST.exp, FIRST.pct_hundredths);
        // Learned at 1x, read stretched: the player changed the window size.
        let at_1x = typed(&hud);
        for (w, h) in STRETCHED {
            let picture = unstretch::picture_width(w, h);
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
        let picture = unstretch::picture_width(1920, 1080);
        let large = stretched(&hud.draw_typed(picture, 768, FIRST), 1920, 1080);
        for canvas in [at_1x, large] {
            let (w, h) = (canvas.width, canvas.height);
            let first = h - rows_needed(h);
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
        let first = h - unstretch::scaled_rows(rows_needed(canvas.height), 2);
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
        let picture = unstretch::picture_width(1920, 1080);
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
            exp: 126_987,
            pct_hundredths: 3916,
            needed: Some(NEEDED_41),
            confirmed: true,
        };
        assert_eq!(events_between(Some(&a), &a), vec![]);
        let gained = ExpReading {
            exp: a.exp + 21,
            ..a
        };
        assert_eq!(
            events_between(Some(&a), &gained),
            vec![Event::ExpChanged {
                level: Some(41),
                exp: 127_008,
                pct_hundredths: 3916,
                needed: Some(NEEDED_41)
            }]
        );
        let up = ExpReading {
            level: Some(42),
            exp: 3,
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
