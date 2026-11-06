//! The engine's loop: find the game window, follow the capture lifecycle, read the screen and
//! emit typed events (capture, then recognizers, then events).
//!
//! The caller drives it with [`Runner::tick`] at up to 4 Hz. A tick reads back only the
//! rows the readers need, and only while capturing, so a paused or blocked engine reads nothing
//! and emits nothing.
//!
//! A grab that brought no new frame is stale: the window stopped drawing (a Mac reports a
//! minimized window's last picture). It isn't read. After
//! [`STALE_AFTER`] of them in a row the engine pauses, and it keeps checking for a new frame each
//! tick, which resumes capture. The HUD's pixels alone can't tell, because they often don't change
//! for minutes.

use crate::capture::{Backend, CaptureError, CaptureSignal, CaptureState, GameWindow, Grab};
use crate::events::Event;
use crate::recognizers::chat::{self, ChatReader, ChatReading, ChatUnreadable};
use crate::recognizers::exp_bar::{self, ExpBarReader, ExpReading, Unreadable};

/// The name the EXP bar reader goes by in [`Event::Unreadable`].
pub const EXP_BAR: &str = "exp_bar";

/// Grabs in a row with no new frame before the engine pauses: 2 s at 4 Hz.
pub const STALE_AFTER: u32 = 8;

/// How many of a game area's bottom rows the engine reads at first when the area is `height` game
/// pixels tall: what its readers need, in game rows.
pub fn rows_needed(height: u32) -> u32 {
    exp_bar::rows_needed(height).max(chat::rows_needed(height))
}

/// What one tick saw and did.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub state: CaptureState,
    pub window: Option<GameWindow>,
    /// The EXP bar this tick: `None` unless capturing.
    pub exp: Option<Result<ExpReading, Unreadable>>,
    /// New chat lines this tick: `None` unless capturing. Chat stays out of `events`, so plugins
    /// never see it.
    pub chat: Option<Result<ChatReading, ChatUnreadable>>,
    /// Typed events. Only ever produced while capturing.
    pub events: Vec<Event>,
    /// Why capture failed, on the tick it did.
    pub error: Option<CaptureError>,
    /// The grab brought no new frame, so nothing was read.
    pub stale: bool,
}

/// The engine: one capture backend and the readers.
pub struct Runner<B> {
    backend: B,
    state: CaptureState,
    window: Option<GameWindow>,
    exp: ExpBarReader,
    chat: ChatReader,
    /// The last confirmed reading, which events are measured from.
    confirmed: Option<ExpReading>,
    /// Whether the EXP bar read last tick, so a covered bar is reported once, not every tick.
    readable: bool,
    /// What the engine last saw, for the "What MesoVibe sees" panel.
    last_grab: Option<Grab>,
    /// Grabs in a row that brought no new frame.
    stale: u32,
}

impl<B: Backend> Runner<B> {
    pub fn new(backend: B) -> Self {
        Self::with_reader(backend, ExpBarReader::new())
    }

    /// Starts with what an EXP bar reader learned before.
    pub fn with_reader(backend: B, exp: ExpBarReader) -> Self {
        Runner {
            backend,
            state: CaptureState::NoGame,
            window: None,
            exp,
            chat: ChatReader::new(),
            confirmed: None,
            readable: false,
            last_grab: None,
            stale: 0,
        }
    }

    /// The rows the engine read on the last tick that captured, exactly as it saw them.
    pub fn last_grab(&self) -> Option<&Grab> {
        self.last_grab.as_ref()
    }

    pub fn state(&self) -> CaptureState {
        self.state
    }

    pub fn exp_reader(&self) -> &ExpBarReader {
        &self.exp
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// The player fixed what blocked capture: look for the window again.
    pub fn retry(&mut self) {
        self.state = self.state.next(CaptureSignal::Retry);
    }

    /// One step: look for the window, follow the lifecycle, and read the EXP bar if capturing.
    pub fn tick(&mut self) -> Tick {
        let found = self.backend.find();
        // Gone, or replaced by a new window: the game was closed (and maybe started again).
        if self
            .window
            .is_some_and(|before| found.is_none_or(|now| now.id != before.id))
        {
            self.state = self.state.next(CaptureSignal::WindowClosed);
            self.backend.stop();
            // A game started again may be another character: don't measure from the old one.
            self.confirmed = None;
            self.chat.reset();
            self.readable = false;
            self.stale = 0;
        }
        let frames_stopped = self.stale >= STALE_AFTER;
        if let Some(window) = found {
            let visible = !window.minimized && !frames_stopped;
            for signal in [
                CaptureSignal::WindowFound { visible },
                match visible {
                    true => CaptureSignal::WindowRestored,
                    false => CaptureSignal::WindowHidden,
                },
            ] {
                self.state = self.state.next(signal);
            }
        }
        self.window = found;
        let mut tick = Tick {
            state: self.state,
            window: found,
            exp: None,
            chat: None,
            events: Vec::new(),
            error: None,
            stale: false,
        };
        // Paused because frames stopped, not because the window says it's hidden: look again.
        let probing = self.state == CaptureState::Paused
            && frames_stopped
            && found.is_some_and(|w| !w.minimized);
        let capturing = self.state == CaptureState::Capturing;
        if let Some(window) = found.filter(|_| capturing || probing) {
            // The chat reader may want more rows than at first, for a chat box the player made taller.
            let rows = rows_needed(window.height).max(self.chat.rows_needed(window.height));
            let grab = self.backend.grab(&window, rows);
            match grab {
                Ok(grab) => {
                    if grab.fresh {
                        self.stale = 0;
                        if probing {
                            self.state = self.state.next(CaptureSignal::WindowRestored);
                        }
                    } else {
                        self.stale = self.stale.saturating_add(1);
                        tick.stale = true;
                        if self.stale >= STALE_AFTER {
                            self.state = self.state.next(CaptureSignal::WindowHidden);
                        }
                    }
                    match grab.frame() {
                        Some(_) if !grab.fresh || self.state != CaptureState::Capturing => {}
                        Some(frame) => {
                            let reading = self.exp.observe(&frame);
                            tick.events = self.events_for(&reading);
                            tick.exp = Some(reading);
                            tick.chat = Some(self.chat.observe(&frame));
                        }
                        None => tick.error = Some(CaptureError("a malformed frame".to_string())),
                    }
                    self.last_grab = Some(grab);
                }
                Err(error) => tick.error = Some(error),
            }
            if tick.error.is_some() {
                self.state = self.state.next(CaptureSignal::CaptureFailed);
                self.backend.stop();
            }
        }
        debug_assert!(tick.events.is_empty() || self.state.emits_events());
        tick.state = self.state;
        tick
    }

    fn events_for(&mut self, reading: &Result<ExpReading, Unreadable>) -> Vec<Event> {
        match reading {
            Ok(r) => {
                self.readable = true;
                let events = exp_bar::events_between(self.confirmed.as_ref(), r);
                if r.confirmed {
                    self.confirmed = Some(*r);
                }
                events
            }
            // Still learning isn't "can't see the bar", and neither is a screen the game dimmed
            // (a map change fades to black): there's no bar drawn to read.
            Err(Unreadable::Learning | Unreadable::NotCalibrated | Unreadable::ScreenDark) => {
                Vec::new()
            }
            Err(_) if self.readable => {
                self.readable = false;
                vec![Event::Unreadable {
                    recognizer: EXP_BAR,
                }]
            }
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::exp_bar::test_support::{Canvas, DARK, GRIND, Hud, NEEDED_41, pct_of};

    /// A backend driven by the test: the window it reports and the picture in it.
    #[derive(Default)]
    struct Fake {
        window: Option<GameWindow>,
        canvas: Option<Canvas>,
        fail: Option<CaptureError>,
        /// (window id, rows) of each grab.
        grabs: Vec<(u64, u32)>,
        stops: usize,
        /// No new frame arrives: grabs hand back the last picture.
        frozen: bool,
    }

    impl Backend for Fake {
        fn find(&mut self) -> Option<GameWindow> {
            self.window
        }

        fn grab(&mut self, window: &GameWindow, rows: u32) -> Result<Grab, CaptureError> {
            self.grabs.push((window.id, rows));
            if let Some(error) = &self.fail {
                return Err(error.clone());
            }
            let canvas = self.canvas.as_ref().expect("a picture to grab");
            let first_row = canvas.height - rows;
            Ok(Grab {
                width: canvas.width,
                height: canvas.height,
                first_row,
                bgra: canvas.bgra[(first_row * canvas.width * 4) as usize..].to_vec(),
                scale: 1,
                fresh: !self.frozen,
            })
        }

        fn stop(&mut self) {
            self.stops += 1;
        }
    }

    const GAME: GameWindow = GameWindow {
        id: 7,
        width: 1366,
        height: 768,
        scale: 1,
        minimized: false,
        placement: None,
    };

    fn hud_at(hud: &Hud, exp: u64) -> Canvas {
        hud.draw_values(41, exp, pct_of(exp, NEEDED_41))
    }

    #[test]
    fn waits_for_the_game_and_reads_only_while_capturing() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake::default());
        let tick = runner.tick();
        assert_eq!((tick.state, tick.exp), (CaptureState::NoGame, None));

        runner.backend_mut().window = Some(GAME);
        runner.backend_mut().canvas = Some(hud_at(&hud, GRIND[0]));
        let tick = runner.tick();
        assert_eq!(tick.state, CaptureState::Capturing);
        assert_eq!(tick.exp, Some(Err(Unreadable::Learning)));
        // Only the rows the reader needs are asked for, and the engine keeps what it saw.
        assert_eq!(runner.backend_mut().grabs, [(7, rows_needed(768))]);
        let seen = runner
            .last_grab()
            .map(|g| (g.height - g.first_row, g.width));
        assert_eq!(seen, Some((rows_needed(768), 1366)));
        assert_eq!(
            tick.chat,
            Some(Err(ChatUnreadable::NotFound)),
            "the chat box is read from the same rows"
        );

        runner.backend_mut().window = Some(GameWindow {
            minimized: true,
            ..GAME
        });
        let tick = runner.tick();
        assert_eq!((tick.state, tick.exp), (CaptureState::Paused, None));
        assert_eq!(
            runner.backend_mut().grabs.len(),
            1,
            "nothing read while paused"
        );

        runner.backend_mut().window = Some(GAME);
        assert_eq!(runner.tick().state, CaptureState::Capturing);
        assert_eq!(runner.backend_mut().grabs.len(), 2);

        runner.backend_mut().window = None;
        let tick = runner.tick();
        assert_eq!((tick.state, tick.exp), (CaptureState::NoGame, None));
        assert_eq!(runner.backend_mut().stops, 1, "capture stops with the game");
    }

    // A player who makes the chat box taller needs more rows grabbed to reach its top. Fails if
    // the grab stays at the default size, so the box's upper lines are never read.
    #[test]
    fn a_taller_chat_box_makes_the_next_grab_reach_its_top() {
        use crate::recognizers::chat::test_support::ChatBox;
        let mut chat = ChatBox::new(&[("lieon: ok", [255, 153, 0])]);
        chat.size = 14;
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(chat.draw()),
            ..Fake::default()
        });
        runner.tick();
        runner.tick();
        let grabs = &runner.backend_mut().grabs;
        assert_eq!(grabs[0], (7, rows_needed(768)));
        assert!(
            grabs[1].1 > rows_needed(768),
            "second grab asks for more rows: {grabs:?}"
        );
    }

    #[test]
    fn a_capture_error_blocks_until_the_player_retries() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(hud_at(&hud, GRIND[0])),
            fail: Some(CaptureError("denied".to_string())),
            ..Fake::default()
        });
        let tick = runner.tick();
        assert_eq!(tick.state, CaptureState::CaptureBlocked);
        assert_eq!(tick.error, Some(CaptureError("denied".to_string())));
        assert_eq!(runner.backend_mut().stops, 1);
        // Blocked stays blocked, and reads nothing, while the window is still there.
        runner.backend_mut().fail = None;
        assert_eq!(runner.tick().state, CaptureState::CaptureBlocked);
        assert_eq!(runner.backend_mut().grabs.len(), 1);
        runner.retry();
        assert_eq!(runner.state(), CaptureState::NoGame);
        let tick = runner.tick();
        assert_eq!(tick.state, CaptureState::Capturing);
        assert_eq!(tick.exp, Some(Err(Unreadable::Learning)));
    }

    #[test]
    fn events_follow_confirmed_readings_and_a_covered_bar_is_reported_once() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            ..Fake::default()
        });
        let mut changes = Vec::new();
        for &exp in &GRIND {
            runner.backend_mut().canvas = Some(hud_at(&hud, exp));
            let tick = runner.tick();
            let confirmed = matches!(tick.exp, Some(Ok(r)) if r.confirmed);
            if !confirmed {
                assert!(tick.events.is_empty(), "no events before confirmation");
            }
            changes.extend(tick.events);
        }
        let last = *GRIND.last().unwrap();
        assert_eq!(
            changes.last(),
            Some(&Event::ExpChanged {
                level: Some(41),
                exp: last,
                pct_hundredths: pct_of(last, NEEDED_41),
                needed: Some(NEEDED_41),
            })
        );
        // The same picture again changes nothing.
        assert!(runner.tick().events.is_empty());
        // A map change fades the screen to black: there's no bar to read, and nothing to report.
        runner.backend_mut().canvas = Some(Canvas::new(1366, 768));
        let tick = runner.tick();
        assert_eq!(tick.exp, Some(Err(Unreadable::ScreenDark)));
        assert!(tick.events.is_empty());
        // Something covers the bar: reported once, then quiet until it reads again.
        let mut covered = hud_at(&hud, last);
        covered.fill(283 + 430, 768 - 34, 40, 14, DARK);
        runner.backend_mut().canvas = Some(covered);
        let unreadable = Event::Unreadable {
            recognizer: EXP_BAR,
        };
        let tick = runner.tick();
        assert_eq!(tick.exp, Some(Err(Unreadable::HudNotFound)));
        assert_eq!(tick.events, [unreadable]);
        assert!(runner.tick().events.is_empty());
    }

    // A Mac hands over a minimized window's last picture as if it were new. Fails if stale frames
    // are read, the engine never pauses, stops looking while paused, or probes a minimized window.
    #[test]
    fn frames_that_stop_coming_pause_the_engine_until_they_return() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(hud_at(&hud, GRIND[0])),
            ..Fake::default()
        });
        assert_eq!(runner.tick().exp, Some(Err(Unreadable::Learning)));
        runner.backend_mut().frozen = true;
        for n in 1..STALE_AFTER {
            let tick = runner.tick();
            assert!(
                tick.stale && tick.exp.is_none(),
                "stale grab {n} isn't read"
            );
            assert_eq!(tick.state, CaptureState::Capturing, "not yet: {n}");
        }
        let tick = runner.tick();
        assert_eq!((tick.state, tick.stale), (CaptureState::Paused, true));
        // Paused, it still looks for new frames, and reads nothing.
        let grabs = runner.backend_mut().grabs.len();
        let tick = runner.tick();
        assert_eq!(
            (tick.state, tick.exp.is_none()),
            (CaptureState::Paused, true)
        );
        assert_eq!(runner.backend_mut().grabs.len(), grabs + 1);
        // Minimized, it stops looking.
        runner.backend_mut().window = Some(GameWindow {
            minimized: true,
            ..GAME
        });
        assert_eq!(runner.tick().state, CaptureState::Paused);
        assert_eq!(runner.backend_mut().grabs.len(), grabs + 1);
        // Restored, still frozen: still paused. Then frames come again: capturing and reading.
        runner.backend_mut().window = Some(GAME);
        assert_eq!(runner.tick().state, CaptureState::Paused);
        runner.backend_mut().frozen = false;
        let tick = runner.tick();
        assert_eq!(tick.state, CaptureState::Capturing);
        assert_eq!(tick.exp, Some(Err(Unreadable::Learning)));
        assert!(!tick.stale);
    }

    #[test]
    fn a_restarted_game_is_measured_afresh() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            ..Fake::default()
        });
        for &exp in &GRIND {
            runner.backend_mut().canvas = Some(hud_at(&hud, exp));
            runner.tick();
        }
        assert!(runner.tick().events.is_empty());
        // The game was closed and started again: a new window, same picture.
        runner.backend_mut().window = Some(GameWindow { id: 8, ..GAME });
        let tick = runner.tick();
        assert_eq!(tick.state, CaptureState::Capturing);
        assert_eq!(runner.backend_mut().stops, 1);
        assert!(
            matches!(tick.events.as_slice(), [Event::ExpChanged { .. }]),
            "the first reading of the new game is an event of its own, never a level-up: {:?}",
            tick.events
        );
        // What the reader learned carries over: it reads at once.
        assert!(matches!(tick.exp, Some(Ok(r)) if r.confirmed));
    }
}
