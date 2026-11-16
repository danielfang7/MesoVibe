//! How the HP and MP numbers are being read, as the panel and plugins show it: still learning, read,
//! covered after being read, never found, or waiting for a digit the EXP bar hasn't shown. One
//! place decides it, from each frame's readings, so the panel and plugins only show it.

use serde::Serialize;
use ts_rs::TS;

use crate::recognizers::exp_bar::{ExpReading, Unreadable};
use crate::recognizers::hp_mp::{HpMp, HpMpUnreadable};

/// EXP bar reading time with the numbers never read before they count as not found.
const NOT_FOUND_AFTER_MS: u64 = 3 * 60_000;
/// EXP bar reading time with a digit still unseen before the panel names the digit.
const MISSING_DIGITS_AFTER_MS: u64 = 5 * 60_000;
/// The most one step adds to the EXP bar's reading time, so a stall doesn't count as reading.
const MAX_STEP_MS: u64 = 1_000;

/// The HP and MP numbers' state. In status messages it's the state's snake_case name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum HpMpState {
    /// Not capturing.
    Off,
    /// The EXP bar's digits, the slash between the numbers, or a digit in them isn't known yet.
    Learning,
    Reading,
    /// Read before, and not now: something is over the numbers.
    Covered,
    /// Never read in three minutes of reading the EXP bar: the numbers aren't where they're looked
    /// for.
    NotFound,
    /// Still learning after five minutes of reading the EXP bar, because a digit the numbers use
    /// hasn't shown in the EXP field yet.
    MissingDigits,
}

/// One frame's EXP bar reading and HP and MP reading.
pub type FrameReading<'a> = (
    &'a Result<ExpReading, Unreadable>,
    &'a Result<HpMp, HpMpUnreadable>,
);

/// Follows the HP and MP readings over time.
#[derive(Debug)]
pub struct HpMpHealth {
    state: HpMpState,
    ever_read: bool,
    /// How long the EXP bar has read with confirmed digits since capture started.
    exp_read_ms: u64,
    last_now: Option<u64>,
}

impl Default for HpMpHealth {
    fn default() -> Self {
        HpMpHealth {
            state: HpMpState::Off,
            ever_read: false,
            exp_read_ms: 0,
            last_now: None,
        }
    }
}

impl HpMpHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one tick: `now` in milliseconds on any steady clock, whether the engine is capturing,
    /// the frame's EXP bar and HP and MP readings (`None` when no frame was read), and whether the
    /// EXP bar's digits are confirmed.
    pub fn observe(
        &mut self,
        now: u64,
        capturing: bool,
        read: Option<FrameReading>,
        exp_confirmed: bool,
    ) -> HpMpState {
        if !capturing {
            // A gap in capture isn't evidence of anything: start afresh.
            *self = HpMpHealth::default();
            return HpMpState::Off;
        }
        let step = self
            .last_now
            .map_or(0, |last| now.saturating_sub(last).min(MAX_STEP_MS));
        self.last_now = Some(now);
        let Some((exp, hp_mp)) = read else {
            // No frame this tick (a stale grab): nothing new.
            if self.state == HpMpState::Off {
                self.state = HpMpState::Learning;
            }
            return self.state;
        };
        // Only confirmed readings count: before then the numbers can't be read at all.
        if exp.as_ref().is_ok_and(|r| r.confirmed) {
            self.exp_read_ms = self.exp_read_ms.saturating_add(step);
        }
        self.state = match hp_mp {
            Ok(_) => {
                self.ever_read = true;
                HpMpState::Reading
            }
            // The game fading to black for a map change: no HUD drawn, so no evidence either way.
            Err(_) if *exp == Err(Unreadable::ScreenDark) && self.state != HpMpState::Off => {
                self.state
            }
            Err(HpMpUnreadable::NoHud) if !exp_confirmed => HpMpState::Learning,
            Err(HpMpUnreadable::Unchecked | HpMpUnreadable::LearningSlash) => HpMpState::Learning,
            Err(HpMpUnreadable::MissingDigit) if self.exp_read_ms >= MISSING_DIGITS_AFTER_MS => {
                HpMpState::MissingDigits
            }
            Err(HpMpUnreadable::MissingDigit) => HpMpState::Learning,
            Err(_) if self.ever_read => HpMpState::Covered,
            Err(_) if self.exp_read_ms >= NOT_FOUND_AFTER_MS => HpMpState::NotFound,
            Err(_) => HpMpState::Learning,
        };
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXP: Result<ExpReading, Unreadable> = Ok(ExpReading {
        level: Some(41),
        exp: 128_027,
        pct_hundredths: 3949,
        needed: Some(324_240),
        confirmed: true,
    });
    const READ: Result<HpMp, HpMpUnreadable> = Ok(HpMp {
        hp: 712,
        max_hp: 1386,
        mp: 675,
        max_mp: 1140,
    });

    /// Feeds `ticks` frames 250 ms apart from `start`, all with the same readings.
    fn feed(
        health: &mut HpMpHealth,
        start: u64,
        ticks: u64,
        exp: &Result<ExpReading, Unreadable>,
        hp_mp: &Result<HpMp, HpMpUnreadable>,
    ) -> HpMpState {
        let mut state = HpMpState::Off;
        for i in 0..ticks {
            state = health.observe(start + i * 250, true, Some((exp, hp_mp)), true);
        }
        state
    }

    // Read, then covered, then read again; off stops it all. Fails if covered numbers look
    // unread from the start, or a pause keeps an old state.
    #[test]
    fn reading_then_covered_then_off() {
        let mut health = HpMpHealth::new();
        assert_eq!(feed(&mut health, 0, 1, &EXP, &READ), HpMpState::Reading);
        let covered = Err(HpMpUnreadable::Garbled);
        assert_eq!(
            feed(&mut health, 250, 4, &EXP, &covered),
            HpMpState::Covered
        );
        assert_eq!(feed(&mut health, 1250, 1, &EXP, &READ), HpMpState::Reading);
        assert_eq!(health.observe(1500, false, None, true), HpMpState::Off);
        let hud = Err(HpMpUnreadable::NoHud);
        assert_eq!(
            feed(&mut health, 1750, 1, &Err(Unreadable::HudNotFound), &hud),
            HpMpState::Learning,
            "after a pause, nothing read yet"
        );
    }

    // Numbers that never read become "not found" after three minutes of EXP reading, not before,
    // and not while the EXP bar itself is still being learned.
    #[test]
    fn never_read_is_not_found_after_three_minutes_of_exp() {
        let mut health = HpMpHealth::new();
        let garbled = Err(HpMpUnreadable::Garbled);
        let minute = 4 * 60;
        assert_eq!(
            feed(&mut health, 0, 2 * minute, &EXP, &garbled),
            HpMpState::Learning
        );
        assert_eq!(
            feed(&mut health, 120_000, minute + 4, &EXP, &garbled),
            HpMpState::NotFound
        );
        let mut learning = HpMpHealth::new();
        let calibrating = Err(Unreadable::Learning);
        let state = (0..4 * minute).fold(HpMpState::Off, |_, i| {
            learning.observe(
                i * 250,
                true,
                Some((&calibrating, &Err(HpMpUnreadable::NoHud))),
                false,
            )
        });
        assert_eq!(state, HpMpState::Learning, "no EXP read, so no clock");
    }

    // A digit the EXP field hasn't shown keeps the numbers learning, and after five minutes of
    // EXP reading the panel says which digit it waits for.
    #[test]
    fn a_missing_digit_is_named_after_five_minutes() {
        let mut health = HpMpHealth::new();
        let missing = Err(HpMpUnreadable::MissingDigit);
        let minute = 4 * 60;
        assert_eq!(
            feed(&mut health, 0, 4 * minute, &EXP, &missing),
            HpMpState::Learning
        );
        assert_eq!(
            feed(&mut health, 240_000, minute + 4, &EXP, &missing),
            HpMpState::MissingDigits
        );
    }

    // A map change's fade to black changes nothing: reading stays reading.
    #[test]
    fn a_dark_screen_keeps_the_state() {
        let mut health = HpMpHealth::new();
        feed(&mut health, 0, 1, &EXP, &READ);
        let dark = Err(Unreadable::ScreenDark);
        assert_eq!(
            feed(&mut health, 250, 8, &dark, &Err(HpMpUnreadable::NoHud)),
            HpMpState::Reading
        );
    }

    // A long gap between two ticks (the computer slept while capturing) counts as at most a
    // second of EXP reading. Fails if one jump in the clock makes unread numbers "not found".
    #[test]
    fn a_stall_does_not_count_as_reading_time() {
        let mut health = HpMpHealth::new();
        let garbled = Err(HpMpUnreadable::Garbled);
        feed(&mut health, 0, 1, &EXP, &garbled);
        assert_eq!(
            feed(&mut health, 10 * 60_000, 2, &EXP, &garbled),
            HpMpState::Learning
        );
    }

    // A tick without a new frame keeps the state it had, and starts at learning after a pause.
    // Fails if a stale grab reads as covered or leaves the state off while capturing.
    #[test]
    fn a_tick_without_a_frame_keeps_the_state() {
        let mut health = HpMpHealth::new();
        assert_eq!(health.observe(0, true, None, true), HpMpState::Learning);
        feed(&mut health, 250, 1, &EXP, &READ);
        assert_eq!(health.observe(500, true, None, true), HpMpState::Reading);
    }

    // Minutes of EXP readings whose digits aren't confirmed yet don't run the "not found" clock,
    // since the numbers can't be read before then. Fails if the first covered frame after the
    // digits are confirmed says "not found".
    #[test]
    fn unconfirmed_exp_readings_do_not_run_the_clock() {
        let mut health = HpMpHealth::new();
        let provisional = Ok(ExpReading {
            confirmed: false,
            ..EXP.unwrap()
        });
        let minute = 4 * 60;
        let state = (0..4 * minute).fold(HpMpState::Off, |_, i| {
            health.observe(
                i * 250,
                true,
                Some((&provisional, &Err(HpMpUnreadable::NoHud))),
                false,
            )
        });
        assert_eq!(state, HpMpState::Learning);
        assert_eq!(
            feed(&mut health, 240_000, 1, &EXP, &Err(HpMpUnreadable::Garbled)),
            HpMpState::Learning
        );
    }
}
