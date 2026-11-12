//! How the pickup notices are being read, as the panel and plugins show it: still learning their
//! letters, reading them, covered for a moment, covered most of the time (as by a party's HP
//! list), or not found where the game draws them. One place decides it, from the notice reader's
//! readings, so the panel and plugins only show it.

use std::collections::VecDeque;

use serde::Serialize;
use ts_rs::TS;

use crate::recognizers::notices::{Missed, Notice, NoticeReading, NoticeUnreadable};

/// How long after a missed line the notices still count as covered.
const COVERED_FOR_MS: u64 = 10_000;
/// How far back "blocked" and "not found" look.
const WINDOW_MS: u64 = 120_000;
/// Covered for longer than this within the window: blocked.
const BLOCKED_MS: u64 = 60_000;
/// EXP gains within the window with no notice line seen: the notices aren't where they're looked
/// for.
const GAINS_UNSEEN: usize = 3;

/// The pickup notices' state. In status messages it's the state's snake_case name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum NoticeState {
    /// Not capturing.
    Off,
    /// The reader hasn't confirmed the digits and letters yet: nothing is counted.
    Learning,
    Reading,
    /// A line was missed in the last few seconds: something passed over the notices.
    Covered,
    /// Covered most of the last two minutes, or a party bonus line showed while covered: the
    /// party's HP list sits over the notices.
    Blocked,
    /// EXP went up several times and no notice line was seen, or the game area can't hold them.
    NotFound,
}

/// Follows the notice reader's readings over time.
#[derive(Debug, Default)]
pub struct NoticeHealth {
    /// Each capturing tick within the window: when, and whether the notices counted as covered.
    ticks: VecDeque<(u64, bool)>,
    last_miss: Option<u64>,
    last_party: Option<u64>,
    /// When the EXP bar gained, within the window.
    gains: VecDeque<u64>,
    last_line: Option<u64>,
}

impl NoticeHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one tick: `now` in milliseconds on any steady clock, whether the engine is capturing,
    /// the notice reader's reading (`None` when nothing was read), whether the reader has
    /// confirmed its letters, and whether the EXP bar gained this tick.
    pub fn observe(
        &mut self,
        now: u64,
        capturing: bool,
        reading: Option<&Result<NoticeReading, NoticeUnreadable>>,
        confirmed: bool,
        exp_gained: bool,
    ) -> NoticeState {
        if !capturing {
            // A gap in capture isn't evidence of anything: start the window afresh.
            *self = NoticeHealth::default();
            return NoticeState::Off;
        }
        let since = now.saturating_sub(WINDOW_MS);
        match reading {
            Some(Err(_)) => return NoticeState::NotFound,
            Some(Ok(reading)) => {
                if !reading.notices.is_empty() || reading.missed.is_some() {
                    self.last_line = Some(now);
                }
                if reading.missed.is_some_and(covers) {
                    self.last_miss = Some(now);
                }
                if reading
                    .notices
                    .iter()
                    .any(|n| matches!(n, Notice::PartyBonus(_)))
                {
                    self.last_party = Some(now);
                }
            }
            None => {}
        }
        if exp_gained {
            self.gains.push_back(now);
        }
        while self.gains.front().is_some_and(|&t| t < since) {
            self.gains.pop_front();
        }
        let covered = self
            .last_miss
            .is_some_and(|t| now.saturating_sub(t) < COVERED_FOR_MS);
        self.ticks.push_back((now, covered));
        while self.ticks.front().is_some_and(|&(t, _)| t < since) {
            self.ticks.pop_front();
        }

        let unseen = self.last_line.is_none_or(|t| t < since);
        if self.gains.len() >= GAINS_UNSEEN && unseen {
            return NoticeState::NotFound;
        }
        if !confirmed {
            return NoticeState::Learning;
        }
        let party = self.last_party.is_some_and(|t| t >= since);
        if self.covered_ms() > BLOCKED_MS || (covered && party) {
            return NoticeState::Blocked;
        }
        match covered {
            true => NoticeState::Covered,
            false => NoticeState::Reading,
        }
    }

    /// How long the notices were covered within the window: each covered tick until the next.
    fn covered_ms(&self) -> u64 {
        self.ticks
            .iter()
            .zip(self.ticks.iter().skip(1))
            .filter(|((_, covered), _)| *covered)
            .map(|((t, _), (next, _))| next - t)
            .sum()
    }
}

/// Whether a missed line means something was over the notices. Amounts waiting for a digit, given
/// up, or read with a wrong shape aren't covers.
fn covers(missed: Missed) -> bool {
    match missed {
        Missed::Unaligned
        | Missed::Unreadable
        | Missed::ExpShort
        | Missed::ExpOver
        | Missed::Lost => true,
        Missed::NotLearned | Missed::Dropped | Missed::ShapesWrong => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(
        notices: Vec<Notice>,
        missed: Option<Missed>,
    ) -> Result<NoticeReading, NoticeUnreadable> {
        Ok(NoticeReading {
            notices,
            missed,
            confirmed: true,
        })
    }

    fn mesos() -> Vec<Notice> {
        vec![Notice::Mesos {
            amount: 10,
            shaped: false,
        }]
    }

    // Each state comes from what the reader saw, on a clock the test controls. Fails if a state
    // shows too early, too late, or never.
    #[test]
    fn states_follow_the_readings() {
        let mut health = NoticeHealth::new();
        let quiet = reading(Vec::new(), None);
        assert_eq!(
            health.observe(0, true, Some(&quiet), false, false),
            NoticeState::Learning
        );
        assert_eq!(
            health.observe(250, true, Some(&reading(mesos(), None)), true, true),
            NoticeState::Reading
        );
        let missed = reading(Vec::new(), Some(Missed::Unaligned));
        assert_eq!(
            health.observe(500, true, Some(&missed), true, false),
            NoticeState::Covered
        );
        assert_eq!(
            health.observe(9_000, true, Some(&quiet), true, false),
            NoticeState::Covered
        );
        assert_eq!(
            health.observe(10_600, true, Some(&quiet), true, false),
            NoticeState::Reading
        );
        // A digit not learned yet isn't a cover.
        let waiting = reading(Vec::new(), Some(Missed::NotLearned));
        assert_eq!(
            health.observe(10_850, true, Some(&waiting), true, false),
            NoticeState::Reading
        );
        assert_eq!(
            health.observe(11_000, false, None, true, false),
            NoticeState::Off
        );
    }

    // A party's HP list covers the notices most of the time. Fails if a long cover, or a party
    // bonus line read while covered, doesn't say blocked.
    #[test]
    fn covered_most_of_the_time_or_in_a_party_is_blocked() {
        let mut health = NoticeHealth::new();
        let missed = reading(Vec::new(), Some(Missed::Unreadable));
        let mut state = NoticeState::Off;
        for tick in 0..=(61_000 / 250) {
            state = health.observe(tick * 250, true, Some(&missed), true, false);
        }
        assert_eq!(state, NoticeState::Blocked);

        let mut health = NoticeHealth::new();
        let party = reading(vec![Notice::PartyBonus(4)], Some(Missed::Unaligned));
        assert_eq!(
            health.observe(0, true, Some(&party), true, false),
            NoticeState::Blocked
        );
    }

    // EXP gains with no notice line at all mean the notices aren't where they're looked for.
    // Fails if that stays "learning" or "reading", or a single gain is enough.
    #[test]
    fn gains_with_no_line_seen_are_not_found() {
        let mut health = NoticeHealth::new();
        let quiet = reading(Vec::new(), None);
        let mut states = Vec::new();
        for (i, gained) in [true, false, true, false, true].into_iter().enumerate() {
            states.push(health.observe(i as u64 * 1_000, true, Some(&quiet), false, gained));
        }
        assert_eq!(states[2], NoticeState::Learning, "two gains aren't enough");
        assert_eq!(states[4], NoticeState::NotFound);
        let size = Err(NoticeUnreadable::UnsupportedSize {
            width: 640,
            height: 480,
        });
        assert_eq!(
            health.observe(5_000, true, Some(&size), false, false),
            NoticeState::NotFound
        );
    }
}
