//! HP and MP numbers: the `[712/1386]` after "HP" and the `[675/1140]` after "MP", left of the
//! EXP field.
//!
//! The HUD prints them in the EXP field's font, so the EXP bar reader's confirmed digits and
//! brackets read them, in the same pass that reads the EXP bar
//! ([`super::exp_bar::ExpBarReader::observe_hud`]): a frame's HP and MP come with that frame's EXP
//! reading, never from an earlier frame. The one glyph the EXP field never shows is the slash. It's
//! learned by its place, the one unknown glyph between two runs of known digits inside the
//! brackets, once two frames agree on its shape. It isn't saved: every HP and MP field shows one,
//! so it's learned again within two frames of each launch, and what the EXP bar reader saves stays
//! as older builds read it.
//!
//! Matching is exact and parsing strict, like the EXP field: anything unexpected is unreadable,
//! never a guess.

use crate::frame::Frame;
use crate::glyph::{Atlas, Glyph, Rect, segment};

/// The HP and MP numbers the HUD showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HpMp {
    pub hp: u32,
    pub max_hp: u32,
    pub mp: u32,
    pub max_mp: u32,
}

/// Why a frame's HP and MP weren't read. None of these produce a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpMpUnreadable {
    /// The EXP bar's pass didn't place the numbers: its digits aren't confirmed yet, or its label
    /// isn't where the HUD puts it (covered, a dark screen, another size). The EXP reading says
    /// which.
    NoHud,
    /// The EXP bar's digits come from an earlier session or from before the picture changed, and
    /// no reading has checked them yet.
    Unchecked,
    /// The slash isn't learned yet. It is from two frames that agree.
    LearningSlash,
    /// A shape the EXP field hasn't shown yet, while some digit is still unseen there: most
    /// likely that digit.
    MissingDigit,
    /// A field holds no text.
    NoText,
    /// A field isn't `[cur/max]` with known shapes: something is drawn over it, or the UI changed.
    Garbled,
}

/// Learns the slash: the shape between the two numbers.
#[derive(Debug, Clone, Default)]
pub(super) struct SlashLearner {
    slash: Option<Glyph>,
    /// A shape one frame put in the slash's place, waiting for a second frame to agree.
    candidate: Option<Glyph>,
    /// A shape both fields put in the slash's place instead of the learned one, waiting for the
    /// next frame to agree.
    replacement: Option<Glyph>,
}

impl SlashLearner {
    pub(super) fn is_learned(&self) -> bool {
        self.slash.is_some()
    }
}

/// Reads the HP and MP fields at `hp` and `mp` with the EXP field's confirmed `atlas`, learning the
/// slash first if it isn't known. `ink` is the EXP field's ink rule.
pub(super) fn read(
    frame: &Frame,
    (hp, mp): (Rect, Rect),
    ink: fn([u8; 3]) -> bool,
    atlas: &Atlas,
    learner: &mut SlashLearner,
) -> Result<HpMp, HpMpUnreadable> {
    let hp = field(frame, hp, ink)?;
    let mp = field(frame, mp, ink)?;
    match learner.slash {
        None => learn_slash(&hp, &mp, atlas, learner),
        Some(_) => recheck_slash(&hp, &mp, atlas, learner),
    }
    let Some(slash) = &learner.slash else {
        return Err(match learner.candidate {
            Some(_) => HpMpUnreadable::LearningSlash,
            None => unknown(atlas),
        });
    };
    let (hp, max_hp) = parse(&name(&hp, atlas, slash)?).ok_or(HpMpUnreadable::Garbled)?;
    let (mp, max_mp) = parse(&name(&mp, atlas, slash)?).ok_or(HpMpUnreadable::Garbled)?;
    Ok(HpMp {
        hp,
        max_hp,
        mp,
        max_mp,
    })
}

/// A field's glyphs, left to right.
fn field(
    frame: &Frame,
    rect: Rect,
    ink: fn([u8; 3]) -> bool,
) -> Result<Vec<Glyph>, HpMpUnreadable> {
    let found = segment(frame, rect, ink).map_err(|_| HpMpUnreadable::Garbled)?;
    if found.is_empty() {
        return Err(HpMpUnreadable::NoText);
    }
    Ok(found.into_iter().map(|(_, glyph)| glyph).collect())
}

/// What an unknown shape most likely is: a digit the EXP field hasn't shown yet, while one is
/// missing, or else something that isn't the numbers.
fn unknown(atlas: &Atlas) -> HpMpUnreadable {
    match atlas.missing(DIGITS).is_empty() {
        true => HpMpUnreadable::Garbled,
        false => HpMpUnreadable::MissingDigit,
    }
}

const DIGITS: &str = "0123456789";

/// Takes this frame's evidence for the slash. Each field whose glyphs are all known but one,
/// sitting between two runs of digits inside the brackets, puts that one forward. The fields must
/// agree, and so must two frames.
fn learn_slash(hp: &[Glyph], mp: &[Glyph], atlas: &Atlas, learner: &mut SlashLearner) {
    let seen = match (slash_candidate(hp, atlas), slash_candidate(mp, atlas)) {
        (Some(a), Some(b)) if a != b => return,
        (Some(a), _) | (None, Some(a)) => a,
        (None, None) => return,
    };
    match &learner.candidate {
        Some(before) if *before == seen => learner.slash = Some(seen),
        _ => learner.candidate = Some(seen),
    }
}

/// Replaces a learned slash that both fields disagree with. The real slash is in both fields; a
/// shape learned from one field alone, such as the cursor over part of the slash, is in neither.
/// Two frames must agree, with no frame between them showing a different slash.
fn recheck_slash(hp: &[Glyph], mp: &[Glyph], atlas: &Atlas, learner: &mut SlashLearner) {
    let seen = match (slash_candidate(hp, atlas), slash_candidate(mp, atlas)) {
        (Some(a), Some(b)) if a == b && learner.slash.as_ref() != Some(&a) => a,
        _ => {
            learner.replacement = None;
            return;
        }
    };
    match &learner.replacement {
        Some(before) if *before == seen => {
            learner.slash = Some(seen);
            learner.replacement = None;
        }
        _ => learner.replacement = Some(seen),
    }
}

/// The glyph in the slash's place in one field: `[`, digits, one unknown shape, digits, `]`, with
/// every other glyph known.
fn slash_candidate(glyphs: &[Glyph], atlas: &Atlas) -> Option<Glyph> {
    let [open, inner @ .., close] = glyphs else {
        return None;
    };
    if atlas.name(open) != Some('[') || atlas.name(close) != Some(']') {
        return None;
    }
    let mut unknown = inner
        .iter()
        .enumerate()
        .filter(|(_, g)| atlas.name(g).is_none());
    let (at, slash) = unknown.next()?;
    // A digit touching the slash makes one shape wider than any digit, and taking it for the
    // slash would drop that digit from the number: never learn one.
    let widest = DIGITS
        .chars()
        .filter_map(|c| atlas.shape(c))
        .map(Glyph::width)
        .max()?;
    if slash.width() > widest {
        return None;
    }
    let digits = |g: &[Glyph]| {
        !g.is_empty()
            && g.iter()
                .all(|g| atlas.name(g).is_some_and(|c| c.is_ascii_digit()))
    };
    (unknown.next().is_none() && digits(&inner[..at]) && digits(&inner[at + 1..]))
        .then(|| slash.clone())
}

/// A field's text, with the slash named `/`.
fn name(glyphs: &[Glyph], atlas: &Atlas, slash: &Glyph) -> Result<String, HpMpUnreadable> {
    glyphs
        .iter()
        .map(|g| match g == slash {
            true => Ok('/'),
            false => atlas.name(g).ok_or_else(|| unknown(atlas)),
        })
        .collect()
}

/// The highest max HP or MP read. Higher than any character has: four digits today, and room for
/// five if the game ever allows more.
const MAX_VALUE: u32 = 99_999;

/// A field as the HUD draws it: `[712/1386]`.
pub fn field_text(current: u32, max: u32) -> String {
    format!("[{current}/{max}]")
}

/// The inverse of [`field_text`], strictly: no leading zero but `0` itself, no comma, `1 <= max <=
/// 99,999` and `current <= max`. Anything else is refused.
pub fn parse(text: &str) -> Option<(u32, u32)> {
    let (current, max) = text.strip_prefix('[')?.strip_suffix(']')?.split_once('/')?;
    let number = |s: &str| -> Option<u32> {
        let n: u32 = s.parse().ok()?;
        (n.to_string() == s).then_some(n)
    };
    let (current, max) = (number(current)?, number(max)?);
    ((1..=MAX_VALUE).contains(&max) && current <= max).then_some((current, max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::exp_bar::test_support::*;
    use crate::recognizers::exp_bar::{ExpBarReader, MAX_NATIVE_HEIGHT, TypedReading, Unreadable};
    use crate::unstretch;

    const FIRST: TypedReading = TypedReading {
        level: 41,
        exp: 126_987,
        pct_hundredths: 3916,
    };
    /// With FIRST, shows every character of the EXP field.
    const SECOND: TypedReading = TypedReading {
        level: 50,
        exp: 2_345_078,
        pct_hundredths: 6420,
    };

    /// A reader that has confirmed the EXP field's shapes from the typed `readings`.
    fn confirmed(hud: &Hud, readings: &[TypedReading]) -> ExpBarReader {
        let mut reader = ExpBarReader::new();
        for &r in readings {
            reader
                .calibrate(&hud.draw_typed(1366, 768, r).frame(), r)
                .unwrap();
        }
        reader
    }

    /// The HUD at FIRST with the numbers `hp` and `mp`, in a `width` x `height` game area: drawn
    /// at 1x up to 768 lines, and above that a 768-line picture stretched as the game does.
    fn numbers(hud: &Hud, (width, height): (u32, u32), hp: &str, mp: &str) -> Canvas {
        if height <= MAX_NATIVE_HEIGHT {
            let mut canvas = hud.draw_typed(width, height, FIRST);
            hud.draw_hp_mp(&mut canvas, hp, mp);
            return canvas;
        }
        let picture = unstretch::picture_width(width, height, MAX_NATIVE_HEIGHT);
        stretched(&numbers(hud, (picture, 768), hp, mp), width, height)
    }

    fn values(hp: u32, max_hp: u32, mp: u32, max_mp: u32) -> Result<HpMp, HpMpUnreadable> {
        Ok(HpMp {
            hp,
            max_hp,
            mp,
            max_mp,
        })
    }

    /// The canvas moved one pixel right, as Wine on a Mac can place the game.
    fn a_pixel_right(canvas: &Canvas) -> Canvas {
        let mut out = Canvas::new(canvas.width, canvas.height);
        for y in 0..canvas.height {
            for x in 1..canvas.width {
                out.set(x, y, canvas.get(x - 1, y));
            }
        }
        out
    }

    // The numbers read with the EXP field's digits at every supported size, four digits and a 0
    // included, once two frames agree on the slash. Fails if a region cuts a digit or reaches a
    // label, or the slash is taken from one frame.
    #[test]
    fn reads_the_numbers_at_every_supported_size() {
        let hud = Hud::new();
        let sizes = [
            (800, 600),
            (1024, 768),
            (1366, 768),
            (1920, 1080),
            (1920, 1200),
            (2560, 1440),
        ];
        for size in sizes {
            let mut reader = confirmed(&hud, &[FIRST, SECOND]);
            let read = |reader: &mut ExpBarReader, hp, mp| {
                reader
                    .observe_hud(&numbers(&hud, size, hp, mp).frame())
                    .hp_mp
            };
            let first = read(&mut reader, "[712/1386]", "[675/1140]");
            assert_eq!(first, Err(HpMpUnreadable::LearningSlash), "{size:?}");
            for (hp, mp, want) in [
                ("[712/1386]", "[675/1140]", values(712, 1386, 675, 1140)),
                ("[9999/9999]", "[9999/9999]", values(9999, 9999, 9999, 9999)),
                ("[0/1386]", "[1140/1140]", values(0, 1386, 1140, 1140)),
                ("[1/1]", "[71/71]", values(1, 1, 71, 71)),
            ] {
                assert_eq!(read(&mut reader, hp, mp), want, "{size:?} {hp} {mp}");
            }
        }
        // A HUD a pixel off, where the reader finds its label.
        let mut reader = confirmed(&hud, &[FIRST, SECOND]);
        let off = a_pixel_right(&numbers(&hud, (1366, 768), "[712/1386]", "[675/1140]"));
        reader.observe_hud(&off.frame());
        assert_eq!(
            reader.observe_hud(&off.frame()).hp_mp,
            values(712, 1386, 675, 1140)
        );
    }

    // Nothing is read before the EXP field's digits are confirmed: they're the numbers' digits.
    #[test]
    fn the_numbers_wait_for_the_exp_digits() {
        let hud = Hud::new();
        let mut reader = ExpBarReader::new();
        let frame = numbers(&hud, (1366, 768), "[712/1386]", "[675/1140]");
        for _ in 0..3 {
            assert_eq!(
                reader.observe_hud(&frame.frame()).hp_mp,
                Err(HpMpUnreadable::NoHud)
            );
        }
    }

    // A field that isn't exactly `[cur/max]` in known shapes is unreadable, never a guess. Fails
    // if a value over its max, a leading zero, a comma, a stray mark or an empty field reads.
    #[test]
    fn numbers_that_could_be_wrong_are_unreadable() {
        let hud = Hud::new();
        let mut reader = confirmed(&hud, &[FIRST, SECOND]);
        let good = numbers(&hud, (1366, 768), "[712/1386]", "[675/1140]");
        reader.observe_hud(&good.frame());
        assert!(reader.observe_hud(&good.frame()).hp_mp.is_ok());
        for (hp, mp) in [
            ("[1387/1386]", "[675/1140]"),
            ("[712/1386]", "[0675/1140]"),
            ("[1,386/1,386]", "[675/1140]"),
            ("[712]", "[675/1140]"),
            ("[712/1386]", "675/1140"),
        ] {
            let frame = numbers(&hud, (1366, 768), hp, mp);
            assert_eq!(
                reader.observe_hud(&frame.frame()).hp_mp,
                Err(HpMpUnreadable::Garbled),
                "{hp} {mp}"
            );
        }
        // A mark the font doesn't have, over the MP digits.
        let mut marked = good;
        marked.fill(283 + 360, 768 - 27, 3, 2, WHITE);
        let read = reader.observe_hud(&marked.frame()).hp_mp;
        assert_eq!(read, Err(HpMpUnreadable::Garbled));
        // No MP numbers at all.
        let mut empty = hud.draw_typed(1366, 768, FIRST);
        hud.draw_hp_mp(&mut empty, "[712/1386]", "");
        let read = reader.observe_hud(&empty.frame()).hp_mp;
        assert_eq!(read, Err(HpMpUnreadable::NoText));
    }

    // A cursor or a tooltip over the EXP field doesn't stop the numbers: they need the HUD's label
    // and the learned digits, not the EXP field itself. A covered label stops both.
    #[test]
    fn a_cursor_over_the_exp_field_leaves_the_numbers_read() {
        let hud = Hud::new();
        let mut reader = confirmed(&hud, &[FIRST, SECOND]);
        let mut frame = numbers(&hud, (1366, 768), "[712/1386]", "[675/1140]");
        reader.observe_hud(&frame.frame());
        frame.fill(283 + 470, 768 - 28, 12, 7, WHITE);
        let read = reader.observe_hud(&frame.frame());
        assert!(read.exp.is_err(), "{:?}", read.exp);
        assert_eq!(read.hp_mp, values(712, 1386, 675, 1140));
        frame.fill(283 + 438, 768 - 31, 28, 10, DARK);
        let read = reader.observe_hud(&frame.frame());
        assert_eq!(read.exp, Err(Unreadable::HudNotFound));
        assert_eq!(read.hp_mp, Err(HpMpUnreadable::NoHud));
    }

    // The slash is learned on screen each session and never saved: what the EXP bar reader saves
    // stays as older builds read it. Fails if knowledge gains a slash line, or a reader started
    // from saved knowledge reads the numbers before its shapes are checked against the HUD.
    #[test]
    fn the_slash_is_learned_each_session_and_never_saved() {
        let hud = Hud::new();
        let mut reader = confirmed(&hud, &[FIRST, SECOND]);
        let saved = reader.knowledge().unwrap();
        let frame = numbers(&hud, (1366, 768), "[712/1386]", "[675/1140]");
        reader.observe_hud(&frame.frame());
        assert!(reader.observe_hud(&frame.frame()).hp_mp.is_ok());
        assert!(reader.knows_slash());
        assert_eq!(reader.knowledge().unwrap(), saved, "the slash isn't saved");

        let mut next = ExpBarReader::from_knowledge(&saved).unwrap();
        assert!(!next.knows_slash());
        // Its saved shapes aren't checked while the EXP field is covered.
        let mut covered = numbers(&hud, (1366, 768), "[712/1386]", "[675/1140]");
        covered.fill(283 + 470, 768 - 28, 12, 7, WHITE);
        let read = next.observe_hud(&covered.frame()).hp_mp;
        assert_eq!(read, Err(HpMpUnreadable::Unchecked));
        // A reading that fits the EXP table checks them; then the slash takes two frames.
        let read = next.observe_hud(&frame.frame()).hp_mp;
        assert_eq!(read, Err(HpMpUnreadable::LearningSlash));
        let read = next.observe_hud(&frame.frame()).hp_mp;
        assert_eq!(read, values(712, 1386, 675, 1140));
        // Another size checks the shapes again and learns the slash again.
        next.recheck();
        assert!(!next.knows_slash());
    }

    // A digit the EXP field hasn't shown yet can't be read in the numbers; the field waits for it
    // rather than guessing. Fails if the slash is learned from a field with an unknown digit.
    #[test]
    fn a_digit_the_exp_field_has_not_shown_waits() {
        let hud = Hud::new();
        // 126,987[39.16%] shows every digit but 0, 4 and 5.
        let mut reader = confirmed(&hud, &[FIRST]);
        let frame = numbers(&hud, (1366, 768), "[712/1386]", "[675/1140]");
        let read = reader.observe_hud(&frame.frame()).hp_mp;
        assert_eq!(read, Err(HpMpUnreadable::LearningSlash));
        let read = reader.observe_hud(&frame.frame()).hp_mp;
        assert_eq!(read, Err(HpMpUnreadable::MissingDigit));
        assert!(
            reader.knows_slash(),
            "learned from HP, whose digits are all known"
        );
        let mut early = confirmed(&hud, &[FIRST]);
        let unseen = numbers(&hud, (1366, 768), "[450/1405]", "[540/1140]");
        for _ in 0..2 {
            let read = early.observe_hud(&unseen.frame()).hp_mp;
            assert_eq!(read, Err(HpMpUnreadable::MissingDigit));
        }
        assert!(!early.knows_slash());
    }

    // A slash learned wrong from one field, here partly covered while MP shows a digit not seen
    // yet, is replaced once both fields agree on another shape for two
    // frames. Fails if a wrong slash keeps the numbers unreadable for the rest of the session.
    #[test]
    fn a_slash_learned_wrong_is_replaced() {
        let hud = Hud::new();
        let mut reader = confirmed(&hud, &[FIRST]);
        let mut touched = numbers(&hud, (1366, 768), "[712/1386]", "[675/1140]");
        // Something dark over the slash's first column.
        let slash = 283 + 237 + text_width(&hud, "[712");
        touched.fill(slash, 768 - 28, 1, 7, DARK);
        for _ in 0..2 {
            assert!(reader.observe_hud(&touched.frame()).hp_mp.is_err());
        }
        assert!(reader.knows_slash(), "learned from HP alone");
        let clean = numbers(&hud, (1366, 768), "[712/1386]", "[671/1196]");
        assert!(reader.observe_hud(&clean.frame()).hp_mp.is_err());
        assert_eq!(
            reader.observe_hud(&clean.frame()).hp_mp,
            values(712, 1386, 671, 1196)
        );
    }

    /// The width of `text` in the EXP field's font, gaps included.
    fn text_width(hud: &Hud, text: &str) -> u32 {
        text.chars()
            .map(|c| hud.exp_font.columns(c).len() as u32 + 1)
            .sum()
    }

    // A digit touching the slash is never taken for it, even when both fields show the same
    // joined shape: naming it `/` would drop that digit and send a wrong number. Fails if
    // [112/1386] with "/1" joined reads as 112/386.
    #[test]
    fn a_digit_touching_the_slash_is_never_the_slash() {
        let hud = Hud::new();
        let mut reader = confirmed(&hud, &[FIRST, SECOND]);
        let mut joined = numbers(&hud, (1366, 768), "[112/1386]", "[675/1140]");
        for (dx, before) in [(237, "[112"), (348, "[675")] {
            let gap =
                283 + dx + text_width(&hud, before) + hud.slash_font.columns('/').len() as u32;
            joined.fill(gap, 768 - 28, 1, 7, WHITE);
        }
        for _ in 0..3 {
            assert!(reader.observe_hud(&joined.frame()).hp_mp.is_err());
        }
        assert!(!reader.knows_slash());
    }

    // A field reads only as the HUD could draw it. Fails if a leading zero, a comma, a value over
    // its max or a max of 0 reads as a number.
    #[test]
    fn parses_only_what_the_hud_could_draw() {
        assert_eq!(parse("[712/1386]"), Some((712, 1386)));
        assert_eq!(parse("[0/1386]"), Some((0, 1386)));
        assert_eq!(parse("[9999/9999]"), Some((9999, 9999)));
        assert_eq!(parse("[30000/30000]"), Some((30_000, 30_000)));
        for bad in [
            "[1387/1386]",
            "[0712/1386]",
            "[712/01386]",
            "[00/1386]",
            "[0/0]",
            "[1,386/1,386]",
            "[100000/100000]",
            "[712/]",
            "[/1386]",
            "712/1386",
            "[712/1386",
            "[712.1386]",
            "[712//1386]",
            "[-1/1386]",
            "[+1/1386]",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        assert_eq!(parse(&field_text(712, 1386)), Some((712, 1386)));
    }
}
