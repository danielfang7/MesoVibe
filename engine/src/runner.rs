//! The engine's loop: find the game window, follow the capture lifecycle, read the screen and
//! emit typed events (capture, then recognizers, then events).
//!
//! The caller drives it with [`Runner::tick`] at up to 4 Hz. A tick reads back only the
//! rows the readers need, and only while capturing, so a paused or blocked engine reads nothing
//! and emits nothing. Now and then a tick reads the whole game area, for the map's name at the
//! top of the minimap ([`MapFollower`]): at the first capture, after a map change, and every 30 s.
//!
//! In Full Screen on a display of another shape, the game's picture has black bars above and
//! below it. The engine measures them on each frame and reads the picture between them
//! ([`letterbox`]), and reports the game area as that picture.
//!
//! Full Screen also scales the size the player chose up to the display, so a game area above 768
//! lines may be a 720- or 600-line picture scaled up rather than the usual 768-line one. The
//! engine assumes 768 until the EXP bar can't be read, then looks for the HUD at the game's other
//! sizes of that shape and reads at the one where it shows ([`exp_bar::other_source`]).
//!
//! A grab that brought no new frame is stale: the window stopped drawing (a Mac reports a
//! minimized window's last picture). It isn't read. After
//! [`STALE_AFTER`] of them in a row the engine pauses, and it keeps checking for a new frame each
//! tick, which resumes capture. The HUD's pixels alone can't tell, because they often don't change
//! for minutes.

use std::time::Instant;

use crate::capture::{Backend, CaptureError, CaptureSignal, CaptureState, GameWindow, Grab};
use crate::events::Event;
use crate::letterbox;
use crate::notice_health::{NoticeHealth, NoticeState};
use crate::recognizers::chat::{self, ChatReader, ChatReading, ChatUnreadable};
use crate::recognizers::exp_bar::{self, ExpBarReader, ExpReading, Unreadable};
use crate::recognizers::map_name::{MapFollower, MapName};
use crate::recognizers::notices::{self, Notice, NoticeReader, NoticeReading, NoticeUnreadable};
use crate::unstretch::NATIVE_HEIGHT;

/// The name the EXP bar reader goes by in [`Event::Unreadable`].
pub const EXP_BAR: &str = "exp_bar";

/// Grabs in a row with no new frame before the engine pauses: 2 s at 4 Hz.
pub const STALE_AFTER: u32 = 8;

/// How many of a game area's bottom rows the engine reads at first when the area is `height` game
/// pixels tall and scaled up from a picture `source` lines tall: what its readers need, in game
/// rows.
pub fn rows_needed(height: u32, source: u32) -> u32 {
    exp_bar::rows_needed(height, source)
        .max(chat::rows_needed(height, source))
        .max(notices::rows_needed(height, source))
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
    /// New pickup notices this tick: `None` unless capturing.
    pub notices: Option<Result<NoticeReading, NoticeUnreadable>>,
    /// A map the player settled on this tick: its name, or `None` for a map whose name can't be
    /// read.
    pub map: Option<Option<MapName>>,
    /// Typed events. Only ever produced while capturing.
    pub events: Vec<Event>,
    /// Why capture failed, on the tick it did.
    pub error: Option<CaptureError>,
    /// The grab brought no new frame, so nothing was read.
    pub stale: bool,
    /// How many lines tall the picture is that the game scales up to fill the game area, when
    /// it's taller than that: 768, or a smaller size the player chose in Full Screen.
    pub source: u32,
}

/// The engine: one capture backend and the readers.
pub struct Runner<B> {
    backend: B,
    state: CaptureState,
    window: Option<GameWindow>,
    exp: ExpBarReader,
    chat: ChatReader,
    notices: NoticeReader,
    map: MapFollower,
    /// The last confirmed reading, which events are measured from.
    confirmed: Option<ExpReading>,
    /// Whether the EXP bar read last tick, so a covered bar is reported once, not every tick.
    readable: bool,
    /// What the engine last saw, for the "What MesoVibe sees" panel and the recorder.
    last_grab: Option<Grab>,
    /// Whether that was a whole frame, read for the map's name rather than the usual strip.
    last_whole: bool,
    /// Grabs in a row that brought no new frame.
    stale: u32,
    /// How the pickup notices are being read, decided each tick.
    health: NoticeHealth,
    notice_state: NoticeState,
    /// When the runner started: the clock `health` measures time on.
    started: Instant,
    /// Capturing on the last tick, so a resumed capture checks the map again.
    was_capturing: bool,
    /// The black bar above and below the game's picture in Full Screen, in game pixels, as last
    /// measured: 0 when the picture fills the window.
    bar: u32,
    /// How many lines tall the picture is that the game scales up to fill the game area: 768
    /// until the HUD shows another of the game's sizes.
    source: u32,
}

impl<B: Backend> Runner<B> {
    pub fn new(backend: B) -> Self {
        Self::with_readers(backend, ExpBarReader::new(), NoticeReader::new())
    }

    /// Starts with what an EXP bar reader and a notice reader learned before.
    pub fn with_readers(backend: B, exp: ExpBarReader, notices: NoticeReader) -> Self {
        Runner {
            backend,
            state: CaptureState::NoGame,
            window: None,
            exp,
            chat: ChatReader::new(),
            notices,
            map: MapFollower::new(),
            confirmed: None,
            readable: false,
            last_grab: None,
            last_whole: false,
            stale: 0,
            health: NoticeHealth::new(),
            notice_state: NoticeState::Off,
            started: Instant::now(),
            was_capturing: false,
            bar: 0,
            source: NATIVE_HEIGHT,
        }
    }

    /// The rows the engine read on the last tick that captured, exactly as it saw them: the
    /// bottom strip, or now and then the whole game area for the map's name.
    pub fn last_grab(&self) -> Option<&Grab> {
        self.last_grab.as_ref()
    }

    /// The last grab when it was the bottom strip. The "What MesoVibe sees" picture shows only
    /// this: the whole game area, read for the map's name, holds chat and other players' names.
    pub fn last_strip(&self) -> Option<&Grab> {
        self.last_grab.as_ref().filter(|_| !self.last_whole)
    }

    pub fn state(&self) -> CaptureState {
        self.state
    }

    pub fn exp_reader(&self) -> &ExpBarReader {
        &self.exp
    }

    pub fn notice_reader(&self) -> &NoticeReader {
        &self.notices
    }

    /// How the pickup notices are being read, as of the last tick.
    pub fn notice_state(&self) -> NoticeState {
        self.notice_state
    }

    /// The map the player is on: `None` before one settled, `Some(None)` for a map whose name
    /// can't be read, else its key.
    pub fn current_map(&self) -> Option<Option<&str>> {
        self.map.current()
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
            self.notices.restart();
            self.map = MapFollower::new();
            self.readable = false;
            self.stale = 0;
        }
        // Another size (in or out of Full Screen, say) has bars of its own, if any, and its own
        // picture.
        if self.window.is_none_or(|before| {
            found.is_none_or(|now| (now.width, now.height) != (before.width, before.height))
        }) {
            self.bar = 0;
            self.source = NATIVE_HEIGHT;
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
            notices: None,
            map: None,
            events: Vec::new(),
            error: None,
            stale: false,
            source: self.source,
        };
        // Paused because frames stopped, not because the window says it's hidden: look again.
        let probing = self.state == CaptureState::Paused
            && frames_stopped
            && found.is_some_and(|w| !w.minimized);
        let capturing = self.state == CaptureState::Capturing;
        if let Some(window) = found.filter(|_| capturing || probing) {
            // The chat reader may want more rows than at first, for a chat box the player made
            // taller, and the map's name is at the top. Under a bar, the readers' rows start above
            // it.
            let whole = self.map.wants_whole_frame();
            let area = window.height.saturating_sub(self.bar.saturating_mul(2));
            let rows = match whole {
                true => window.height,
                false => (self.bar
                    + rows_needed(area, self.source).max(self.chat.rows_needed(area, self.source)))
                .min(window.height),
            };
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
                            let sized_for = self.bar;
                            if let Some(bar) = letterbox::bottom_bar(&frame) {
                                self.bar = bar;
                            }
                            // A taller bar than the grab was sized for leaves too few rows above it
                            // for the chat box: this frame only measures, and the next one reads.
                            if self.bar <= sized_for {
                                // A bar taller than the rows read can't be cut: read the frame as it is.
                                let frame = letterbox::inside(&frame, self.bar)
                                    .unwrap_or(frame)
                                    .with_source(self.source);
                                let reading = self.exp.observe(&frame);
                                // Another of the game's sizes scaled up: read at it from the next
                                // frame, which is grabbed with the rows it needs.
                                if let Some(lines) = exp_bar::other_source(&frame, &reading) {
                                    self.source = lines;
                                }
                                // The game fades to black when the map changes.
                                let dark = matches!(reading, Err(Unreadable::ScreenDark));
                                let notices = match dark {
                                    true => Ok(self.notices.screen_dark()),
                                    false => self.notices.observe(&frame, reading.as_ref().ok()),
                                };
                                tick.events = self.events_for(&reading);
                                tick.events.extend(notice_events(&notices));
                                if let Some(map) = self.map.observe(&frame, dark) {
                                    tick.events.push(Event::MapChanged {
                                        map_key: map.as_ref().map(|name| name.key.clone()),
                                    });
                                    tick.map = Some(map);
                                }
                                tick.exp = Some(reading);
                                tick.chat = Some(self.chat.observe(&frame));
                                tick.notices = Some(notices);
                            }
                        }
                        None => tick.error = Some(CaptureError("a malformed frame".to_string())),
                    }
                    self.last_grab = Some(grab);
                    self.last_whole = whole;
                }
                Err(error) => tick.error = Some(error),
            }
            if tick.error.is_some() {
                self.state = self.state.next(CaptureSignal::CaptureFailed);
                self.backend.stop();
            }
        }
        // Not reading: pickups may come and go unseen until it reads again.
        if self.state != CaptureState::Capturing {
            self.notices.reset();
        }
        debug_assert!(tick.events.is_empty() || self.state.emits_events());
        tick.state = self.state;
        tick.source = self.source;
        tick.window = found.map(|window| letterbox::in_window(window, self.bar));
        let capturing = self.state == CaptureState::Capturing;
        // Back from a pause: the player may have taken a portal meanwhile.
        if capturing && !self.was_capturing {
            self.map.recheck();
        }
        self.was_capturing = capturing;
        let gained = tick
            .events
            .iter()
            .any(|e| matches!(e, Event::ExpChanged { .. }));
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.notice_state = self.health.observe(
            now,
            capturing,
            tick.notices.as_ref(),
            self.notices.confirmed(),
            gained,
        );
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

/// The events a notice reading brings: its mesos and items, and whether lines may have been
/// missed. None while the reader is still learning its letters.
fn notice_events(reading: &Result<NoticeReading, NoticeUnreadable>) -> Vec<Event> {
    let Some(reading) = reading.as_ref().ok().filter(|r| r.confirmed) else {
        return Vec::new();
    };
    let gained = reading.notices.iter().filter_map(|notice| match notice {
        Notice::Mesos { amount, shaped } => Some(Event::MesosGained {
            amount: *amount,
            late: false,
            shaped: *shaped,
        }),
        Notice::LateMesos { amount, shaped } => Some(Event::MesosGained {
            amount: *amount,
            late: true,
            shaped: *shaped,
        }),
        Notice::Item { name, count, tab } => Some(Event::ItemGained {
            item_key: name.key.clone(),
            count: *count,
            tab: *tab,
        }),
        Notice::PartyBonus(amount) => Some(Event::PartyBonus { amount: *amount }),
        // EXP comes from the EXP bar.
        Notice::Exp(_) | Notice::Other => None,
    });
    let missed = reading.missed.map(|reason| Event::NoticesMissed { reason });
    gained.chain(missed).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::exp_bar::test_support::{Canvas, DARK, GRIND, Hud, NEEDED_41, pct_of};
    use crate::recognizers::notices::{Missed, Tab};

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
        assert_eq!(
            runner.backend_mut().grabs,
            [(7, rows_needed(768, NATIVE_HEIGHT))]
        );
        let seen = runner
            .last_grab()
            .map(|g| (g.height - g.first_row, g.width));
        assert_eq!(seen, Some((rows_needed(768, NATIVE_HEIGHT), 1366)));
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
        let mut chat = ChatBox::new(&[("lieon : ok", [255, 153, 0])]);
        chat.size = 14;
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(chat.draw()),
            ..Fake::default()
        });
        for _ in 0..4 {
            runner.tick();
        }
        let grabs = &runner.backend_mut().grabs;
        assert_eq!(grabs[0], (7, rows_needed(768, NATIVE_HEIGHT)));
        // Any whole frames read for the map's name aside, the next strip reaches higher.
        let strips: Vec<u32> = grabs.iter().map(|g| g.1).filter(|&r| r < 768).collect();
        assert!(
            strips.len() >= 2 && strips[1] > rows_needed(768, NATIVE_HEIGHT),
            "the next strip asks for more rows: {grabs:?}"
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
        // The map's events are another test's: there's no minimap here.
        let next = |runner: &mut Runner<Fake>| {
            let mut tick = runner.tick();
            tick.events
                .retain(|e| !matches!(e, Event::MapChanged { .. }));
            tick
        };
        let mut changes = Vec::new();
        for &exp in &GRIND {
            runner.backend_mut().canvas = Some(hud_at(&hud, exp));
            let tick = next(&mut runner);
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
        assert!(next(&mut runner).events.is_empty());
        // A map change fades the screen to black: there's no bar to read, and nothing to report.
        runner.backend_mut().canvas = Some(Canvas::new(1366, 768));
        let tick = next(&mut runner);
        assert_eq!(tick.exp, Some(Err(Unreadable::ScreenDark)));
        assert!(tick.events.is_empty());
        // Something covers the bar: reported once, then quiet until it reads again.
        let mut covered = hud_at(&hud, last);
        covered.fill(283 + 430, 768 - 34, 40, 14, DARK);
        runner.backend_mut().canvas = Some(covered);
        let unreadable = Event::Unreadable {
            recognizer: EXP_BAR,
        };
        let tick = next(&mut runner);
        assert_eq!(tick.exp, Some(Err(Unreadable::HudNotFound)));
        assert_eq!(tick.events, [unreadable]);
        assert!(next(&mut runner).events.is_empty());
    }

    // The map's name is read from whole frames only at the first capture and after a fade, and a
    // new map reaches plugins once two reads agree. Fails if a new map is never reported, or the
    // whole frame is grabbed when nothing asked for it.
    #[test]
    fn the_map_is_read_at_the_start_and_after_each_fade() {
        use crate::recognizers::map_name::test_support::Minimap;
        let hud = Hud::new();
        let on_map = |name: &str| {
            let mut canvas = hud_at(&hud, GRIND[0]);
            Minimap::new("Victoria Road", name).draw(&mut canvas);
            canvas
        };
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(on_map("Forest I")),
            ..Fake::default()
        });
        let maps = |runner: &mut Runner<Fake>, ticks: usize| -> Vec<Option<String>> {
            (0..ticks)
                .flat_map(|_| runner.tick().events)
                .filter_map(|e| match e {
                    Event::MapChanged { map_key } => Some(map_key),
                    _ => None,
                })
                .collect()
        };
        let forest = maps(&mut runner, 6);
        assert!(matches!(forest.as_slice(), [Some(_)]), "{forest:?}");
        // A map change: the screen goes dark, then the new map shows.
        runner.backend_mut().canvas = Some(Canvas::new(1366, 768));
        assert!(maps(&mut runner, 2).is_empty());
        runner.backend_mut().canvas = Some(on_map("Henesys"));
        let town = maps(&mut runner, 6);
        assert!(matches!(town.as_slice(), [Some(_)]), "{town:?}");
        assert_ne!(town, forest);
        // Two whole frames for each map, and strips otherwise.
        let whole = runner
            .backend_mut()
            .grabs
            .iter()
            .filter(|g| g.1 == 768)
            .count();
        assert_eq!(whole, 4);
    }

    // A player can take a portal while capture is paused (the game minimized). Fails if the map
    // isn't read again as soon as capture resumes, which would file the new map's play under the
    // old one for up to 30 s.
    #[test]
    fn the_map_is_read_again_when_capture_resumes() {
        use crate::recognizers::map_name::test_support::Minimap;
        let mut canvas = hud_at(&Hud::new(), GRIND[0]);
        Minimap::new("Victoria Road", "Forest I").draw(&mut canvas);
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(canvas),
            ..Fake::default()
        });
        for _ in 0..6 {
            runner.tick();
        }
        assert!(matches!(runner.current_map(), Some(Some(_))));
        runner.backend_mut().window = Some(GameWindow {
            minimized: true,
            ..GAME
        });
        runner.tick();
        assert_ne!(runner.tick().state, CaptureState::Capturing);
        runner.backend_mut().window = Some(GAME);
        let before = runner.backend_mut().grabs.len();
        for _ in 0..3 {
            runner.tick();
        }
        let whole = runner.backend_mut().grabs[before..]
            .iter()
            .filter(|g| g.1 == 768)
            .count();
        assert!(whole > 0, "the map was read again");
    }

    // The "What MesoVibe sees" picture shows only the bottom strip. Fails if a whole frame read
    // for the map's name, which holds chat and other players' names, is offered for it.
    #[test]
    fn only_strips_are_offered_for_the_picture() {
        use crate::recognizers::map_name::test_support::Minimap;
        let mut canvas = hud_at(&Hud::new(), GRIND[0]);
        Minimap::new("Victoria Road", "Forest I").draw(&mut canvas);
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(canvas),
            ..Fake::default()
        });
        let mut whole = 0;
        for _ in 0..6 {
            runner.tick();
            let height = |g: &Grab| g.height - g.first_row;
            let last = runner.last_grab().map(height);
            if last == Some(768) {
                whole += 1;
                assert_eq!(runner.last_strip().map(height), None);
            } else {
                assert_eq!(runner.last_strip().map(height), last);
            }
        }
        assert!(whole > 0, "a whole frame was read");
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

    // Pickups reach plugins as events only once the notice reader's letters are confirmed, and a
    // spell of not reading says pickups may be missing. Fails if a pickup is lost between the
    // reader and the events, one shows while learning, or a pause passes silently.
    #[test]
    fn pickups_become_events_once_the_letters_are_confirmed() {
        use crate::recognizers::notices::test_support::{
            Game, Scene, WHITE, confirmed_reader, item, mesos,
        };
        let mut game = Game::new(Scene::Rock);
        let mut learning = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(lit(game.next(&[], 0))),
            ..Fake::default()
        });
        learning.tick();
        learning.backend_mut().canvas = Some(lit(game.next(&[(&mesos("12"), WHITE)], 0)));
        let tick = learning.tick();
        assert!(matches!(&tick.notices, Some(Ok(r)) if !r.confirmed));
        assert!(pickups(tick.events).is_empty(), "nothing while learning");

        let reader = confirmed_reader(&mut game);
        let mut runner = Runner::with_readers(
            Fake {
                window: Some(GAME),
                canvas: Some(lit(game.next(&[], 0))),
                ..Fake::default()
            },
            ExpBarReader::new(),
            reader,
        );
        assert!(runner.notice_reader().confirmed());
        assert!(
            pickups(runner.tick().events).is_empty(),
            "lines already on screen"
        );
        runner.backend_mut().canvas = Some(lit(game.next(&[(&mesos("75"), WHITE)], 0)));
        assert_eq!(
            pickups(runner.tick().events),
            [Event::MesosGained {
                amount: 75,
                late: false,
                shaped: false
            }]
        );
        runner.backend_mut().canvas =
            Some(lit(game.next(&[(&item("Red Potion", 2, "Use"), WHITE)], 0)));
        let events = pickups(runner.tick().events);
        assert!(
            matches!(
                events.as_slice(),
                [Event::ItemGained { item_key, count: 2, tab: Some(Tab::Use) }] if item_key.len() == 16
            ),
            "{events:?}"
        );
        // Minimized and back: whatever arrived meanwhile went unseen.
        runner.backend_mut().window = Some(GameWindow {
            minimized: true,
            ..GAME
        });
        runner.tick();
        runner.backend_mut().window = Some(GAME);
        runner.backend_mut().canvas = Some(lit(game.next(&[], 0)));
        assert_eq!(
            pickups(runner.tick().events),
            [Event::NoticesMissed {
                reason: Missed::Lost
            }]
        );
        // Closed and started again: nothing could be picked up meanwhile.
        runner.backend_mut().window = None;
        runner.tick();
        runner.backend_mut().window = Some(GameWindow { id: 8, ..GAME });
        assert!(pickups(runner.tick().events).is_empty());
        runner.backend_mut().canvas = Some(lit(game.next(&[(&mesos("12"), WHITE)], 0)));
        assert_eq!(
            pickups(runner.tick().events),
            [Event::MesosGained {
                amount: 12,
                late: false,
                shaped: false
            }]
        );
    }

    /// The notices over the scene, with white where the HUD's text would be: a frame with nothing
    /// white there is a screen the game dimmed to change maps.
    fn lit(mut canvas: Canvas) -> Canvas {
        canvas.fill(300, canvas.height - 10, 2, 2, [255, 255, 255]);
        canvas
    }

    /// The events but the map's: the frames these tests draw have no minimap.
    fn pickups(events: Vec<Event>) -> Vec<Event> {
        events
            .into_iter()
            .filter(|e| !matches!(e, Event::MapChanged { .. }))
            .collect()
    }

    // Pickups on screen when the player takes a portal stay on screen over the new map, still
    // fading. Fails if they're counted again when the new map shows, or the dark screen between
    // is reported as pickups missed.
    #[test]
    fn a_map_change_counts_no_pickup_twice() {
        use crate::recognizers::notices::test_support::{
            Game, Scene, WHITE, confirmed_reader, mesos,
        };
        let mut game = Game::new(Scene::Rock);
        let reader = confirmed_reader(&mut game);
        let mut runner = Runner::with_readers(
            Fake {
                window: Some(GAME),
                canvas: Some(lit(game.next(&[], 0))),
                ..Fake::default()
            },
            ExpBarReader::new(),
            reader,
        );
        runner.tick();
        runner.backend_mut().canvas = Some(lit(game.next(&[(&mesos("75"), WHITE)], 0)));
        assert_eq!(
            pickups(runner.tick().events),
            [Event::MesosGained {
                amount: 75,
                late: false,
                shaped: false
            }]
        );
        let mut events = Vec::new();
        for _ in 0..2 {
            game.next(&[], 0);
            runner.backend_mut().canvas = Some(Canvas::new(1366, 768));
            events.extend(runner.tick().events);
        }
        game.scene = Scene::Cloud;
        for _ in 0..3 {
            runner.backend_mut().canvas = Some(lit(game.next(&[], 0)));
            events.extend(runner.tick().events);
        }
        assert_eq!(pickups(events), []);
        runner.backend_mut().canvas = Some(lit(game.next(&[(&mesos("12"), WHITE)], 0)));
        assert_eq!(
            pickups(runner.tick().events),
            [Event::MesosGained {
                amount: 12,
                late: false,
                shaped: false
            }]
        );
    }

    // Plugins tell a mesos amount read late (reported missed when it arrived) from one read at
    // once, and one read with a digit named by its shape; a party bonus line becomes its own
    // event. Fails if any of them comes out the same as a plain amount, or the bonus is dropped.
    #[test]
    fn mesos_read_late_or_by_shape_say_so() {
        let reading = NoticeReading {
            notices: vec![
                Notice::Mesos {
                    amount: 30,
                    shaped: false,
                },
                Notice::LateMesos {
                    amount: 75,
                    shaped: false,
                },
                Notice::Mesos {
                    amount: 58,
                    shaped: true,
                },
                Notice::PartyBonus(4),
            ],
            missed: Some(Missed::ShapesWrong),
            confirmed: true,
        };
        assert_eq!(
            notice_events(&Ok(reading)),
            [
                Event::MesosGained {
                    amount: 30,
                    late: false,
                    shaped: false
                },
                Event::MesosGained {
                    amount: 75,
                    late: true,
                    shaped: false
                },
                Event::MesosGained {
                    amount: 58,
                    late: false,
                    shaped: true
                },
                Event::PartyBonus { amount: 4 },
                Event::NoticesMissed {
                    reason: Missed::ShapesWrong
                }
            ]
        );
    }

    /// The HUD at `exp` in Full Screen on a 1512x982 screen: the 1366x768 picture scaled to the
    /// screen's width, 1512x850, between black bars 66 pixels tall. The HUD has the light bottom
    /// line the game's has.
    fn full_screen(hud: &Hud, exp: u64) -> Canvas {
        full_screen_from(hud, exp, (1366, 768))
    }

    /// [`full_screen`] with the game at another size of the same shape: 1280x720 fills the same
    /// 1512x850 picture.
    fn full_screen_from(hud: &Hud, exp: u64, (width, lines): (u32, u32)) -> Canvas {
        let field = exp_bar::exp_text(exp, pct_of(exp, NEEDED_41));
        let mut picture = hud.draw(width, lines, &field, "41");
        picture.fill((width - 800) / 2, lines - 1, 800, 1, [200, 200, 200]);
        let rgb: Vec<[u8; 3]> = picture
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[2], p[1], p[0]])
            .collect();
        let scaled = crate::unstretch::stretch(&rgb, width, 1512, 850, 0);
        let mut bgra: Vec<u8> = [0, 0, 0, 255].repeat(1512 * 982);
        bgra[66 * 1512 * 4..(66 + 850) * 1512 * 4].copy_from_slice(&scaled);
        Canvas {
            width: 1512,
            height: 982,
            bgra,
        }
    }

    // Full Screen on a 16:10 Mac screen leaves black bars above and below the picture, so the
    // window's bottom rows are black. Fails if the bars are taken for a dark screen (nothing is
    // ever tracked), the grab stops short of the picture above the bar, the game area isn't
    // reported as the picture, a map change's black frames make the engine forget the bars, or
    // the bars outlive a switch back to windowed.
    #[test]
    fn reads_the_picture_between_full_screen_bars() {
        let hud = Hud::new();
        let full = GameWindow {
            width: 1512,
            height: 982,
            ..GAME
        };
        let mut runner = Runner::new(Fake {
            window: Some(full),
            ..Fake::default()
        });
        // The first frame is grabbed for no bar, too short above the bar for the chat box: it's
        // measured, not read.
        runner.backend_mut().canvas = Some(full_screen(&hud, GRIND[0]));
        let first = runner.tick();
        assert_eq!((first.exp, first.chat), (None, None));
        assert_eq!(first.window.map(|w| (w.width, w.height)), Some((1512, 850)));
        let mut changes = Vec::new();
        for &exp in &GRIND {
            runner.backend_mut().canvas = Some(full_screen(&hud, exp));
            let tick = runner.tick();
            assert_ne!(tick.exp, Some(Err(Unreadable::ScreenDark)));
            assert_eq!(
                tick.chat,
                Some(Err(ChatUnreadable::NotFound)),
                "the chat box looked for with all the rows it needs above the bar"
            );
            assert_eq!(tick.window.map(|w| (w.width, w.height)), Some((1512, 850)));
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
        let rows = runner.backend_mut().grabs.last().map(|g| g.1);
        assert!(
            rows >= Some(66 + rows_needed(850, NATIVE_HEIGHT)),
            "{rows:?}"
        );

        // A map change fades it all to black, where no bar can be measured. The bars stay as they
        // were: MesoVibe's windows don't jump to the black around the picture, and it reads again
        // as soon as the picture is back.
        runner.backend_mut().canvas = Some(Canvas {
            width: 1512,
            height: 982,
            bgra: [0, 0, 0, 255].repeat(1512 * 982),
        });
        let tick = runner.tick();
        assert_eq!(tick.exp, Some(Err(Unreadable::ScreenDark)));
        assert!(tick.events.is_empty());
        assert_eq!(tick.window.map(|w| (w.width, w.height)), Some((1512, 850)));
        runner.backend_mut().canvas = Some(full_screen(&hud, last));
        let tick = runner.tick();
        assert!(
            matches!(tick.exp, Some(Ok(r)) if r.exp == last),
            "{:?}",
            tick.exp
        );

        // The game restarted at the same size draws the same bars: its black loading frames keep
        // them, so MesoVibe's windows stay on the picture.
        runner.backend_mut().window = Some(GameWindow { id: 8, ..full });
        runner.backend_mut().canvas = Some(Canvas {
            width: 1512,
            height: 982,
            bgra: [0, 0, 0, 255].repeat(1512 * 982),
        });
        let tick = runner.tick();
        assert_eq!(tick.window.map(|w| w.height), Some(850));

        // Back to windowed: the same game, no bars.
        runner.backend_mut().window = Some(GAME);
        runner.backend_mut().canvas = Some(hud_at(&hud, last + 21));
        let tick = runner.tick();
        assert_eq!(tick.window.map(|w| w.height), Some(768));
        assert!(
            matches!(tick.exp, Some(Ok(r)) if r.exp == last + 21),
            "{:?}",
            tick.exp
        );
    }

    // Full Screen scales the size the player chose: at 1280x720 the HUD fills the same 1512x850
    // picture as at 1366x768, only larger. Fails if the engine never reads past the usual
    // 768-line size (the EXP bar stays unreadable, as players saw), grabs too few rows for the
    // larger scale, or keeps the smaller size after the window changes.
    #[test]
    fn reads_full_screen_scaled_up_from_a_smaller_size() {
        let hud = Hud::new();
        let full = GameWindow {
            width: 1512,
            height: 982,
            ..GAME
        };
        let mut runner = Runner::new(Fake {
            window: Some(full),
            ..Fake::default()
        });
        let mut sources = Vec::new();
        let mut changes = Vec::new();
        for &exp in [GRIND[0], GRIND[0]].iter().chain(&GRIND) {
            runner.backend_mut().canvas = Some(full_screen_from(&hud, exp, (1280, 720)));
            let tick = runner.tick();
            assert_ne!(tick.exp, Some(Err(Unreadable::ScreenDark)));
            sources.push(tick.source);
            changes.extend(tick.events);
        }
        assert_eq!(sources[0], 768, "768 lines until the HUD shows otherwise");
        assert_eq!(sources.last(), Some(&720));
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
        let rows = runner.backend_mut().grabs.last().map(|g| g.1);
        assert!(rows >= Some(66 + rows_needed(850, 720)), "{rows:?}");

        // Back to windowed: the usual size again, read at once.
        runner.backend_mut().window = Some(GAME);
        runner.backend_mut().canvas = Some(hud_at(&hud, last + 21));
        let tick = runner.tick();
        assert_eq!(tick.source, 768);
        assert!(
            matches!(tick.exp, Some(Ok(r)) if r.exp == last + 21),
            "{:?}",
            tick.exp
        );
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
