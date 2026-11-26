//! The engine's loop: find the game window, follow the capture lifecycle, read the screen and
//! emit typed events (capture, then recognizers, then events).
//!
//! The caller drives it with [`Runner::tick`] at up to 4 Hz. A tick reads back only the
//! rows the readers need, and only while capturing, so a paused or blocked engine reads nothing
//! and emits nothing. Now and then a tick reads the whole game area instead: for the map's name at
//! the top of the minimap ([`MapFollower`]), at the first capture, after a map change and every
//! 30 s; and for the meso total in the inventory, shop or storage window ([`MesoReader`]), about
//! once a second, and every tick while one of those windows' meso box is in view; and, with the
//! price label switched on, every tick while an item's tooltip is in view ([`TooltipReader`]),
//! so a tooltip first seen on a once-a-second look (in the equipment window, say) is followed
//! from then on. One whole frame serves them all when several ask on the same tick. The map's name is read only on whole frames it
//! asked for, so the meso total's frames never change when it's read; the EXP bar, HP and MP and
//! chat read every tick's frame, whole or not. Whole frames are never offered for the "What MesoVibe
//! sees" picture ([`Runner::last_strip`]).
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
//! When the picture changes (its size, bars or scale), the HUD may be drawn differently, so the
//! readers check what they learned before against it, and learn again if it no longer fits.
//!
//! The HP and MP numbers are read in the EXP bar's pass, so each frame's numbers come with that
//! frame. `HpMpChanged` is `continuous` only when the frame just before also read them: any frame
//! without a reading (covered, dark, a pause, a new size, a restart) breaks the chain, while a stale
//! grab, which reads nothing new, leaves it as it was.
//!
//! A grab that brought no new frame is stale: the window stopped drawing (a Mac reports a
//! minimized window's last picture). It isn't read. After
//! [`STALE_AFTER`] of them in a row the engine pauses, and it keeps checking for a new frame each
//! tick, which resumes capture. The HUD's pixels alone can't tell, because they often don't change
//! for minutes.

use std::time::Instant;

use crate::capture::{Backend, CaptureError, CaptureSignal, CaptureState, GameWindow, Grab};
use crate::events::Event;
use crate::hp_mp_health::{HpMpHealth, HpMpState};
use crate::letterbox;
use crate::recognizers::chat::{self, ChatReader, ChatReading, ChatUnreadable};
use crate::recognizers::exp_bar::{self, ExpBarReader, ExpReading, Unreadable};
use crate::recognizers::hp_mp::{HpMp, HpMpUnreadable};
use crate::recognizers::map_name::{MapFollower, MapName};
use crate::recognizers::meso_total::{MesoReader, MesoState, MesoView};
use crate::recognizers::quick_slots::{self, QuickSlotReader, QuickSlotState};
use crate::recognizers::tooltip::{TooltipReader, TooltipReading};
use crate::unstretch::{self, NATIVE_HEIGHT};

/// The name the EXP bar reader goes by in [`Event::Unreadable`].
pub const EXP_BAR: &str = "exp_bar";
/// The name the HP and MP reader goes by in [`Event::Unreadable`].
pub const HP_MP: &str = "hp_mp";

/// Grabs in a row with no new frame before the engine pauses: 2 s at 4 Hz.
pub const STALE_AFTER: u32 = 8;

/// How many of a game area's bottom rows the engine reads at first when the area is `height` game
/// pixels tall and scaled up from a picture `source` lines tall: what its readers need, in game
/// rows.
pub fn rows_needed(height: u32, source: u32) -> u32 {
    exp_bar::rows_needed(height, source)
        .max(chat::rows_needed(height, source))
        .max(unstretch::rows_for(height, source, quick_slots::ROWS))
}

/// Who asked for a tick's whole frame. Several may, and then one frame serves them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WholeFrameFor {
    /// The map's name ([`MapFollower`]).
    pub map: bool,
    /// The meso total ([`MesoReader`]).
    pub mesos: bool,
    /// An item's tooltip was in view on the last whole frame ([`TooltipReader`]).
    pub tooltip: bool,
}

/// What one tick saw and did.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub state: CaptureState,
    pub window: Option<GameWindow>,
    /// The EXP bar this tick: `None` unless capturing.
    pub exp: Option<Result<ExpReading, Unreadable>>,
    /// The HP and MP numbers this tick, read in the EXP bar's pass: `None` unless a fresh frame
    /// was read.
    pub hp_mp: Option<Result<HpMp, HpMpUnreadable>>,
    /// New chat lines this tick: `None` unless capturing. Chat stays out of `events`, so plugins
    /// never see it.
    pub chat: Option<Result<ChatReading, ChatUnreadable>>,
    /// A map the player settled on this tick: its name, or `None` for a map whose name can't be
    /// read.
    pub map: Option<Option<MapName>>,
    /// What this tick's whole frame showed of the meso total: `None` on a tick that read only the
    /// bottom rows, or none.
    pub mesos: Option<MesoView>,
    /// Who asked for this tick's whole frame, when it grabbed one.
    pub whole: Option<WholeFrameFor>,
    /// The item tooltip this tick's whole frame showed, with the price label switched on: `None`
    /// on a tick that read only the bottom rows, or with it off.
    pub tooltip: Option<TooltipReading>,
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
///
/// The pickup notices at the bottom right ("You have gained mesos (+N)", items picked up) aren't
/// read: on a busy map they miss many pickups, so mesos come from the meso total the inventory
/// and shop windows show ([`Event::MesoChanged`]) instead. Their reader stays in
/// [`notices`](crate::recognizers::notices), with its tests and the `replay_notices` example.
pub struct Runner<B> {
    backend: B,
    state: CaptureState,
    window: Option<GameWindow>,
    exp: ExpBarReader,
    chat: ChatReader,
    map: MapFollower,
    meso: MesoReader,
    slots: QuickSlotReader,
    /// Item tooltips, for the price label: `None` while it's switched off.
    tooltips: Option<TooltipReader>,
    /// The last whole frame showed an item tooltip.
    tooltip_in_view: bool,
    /// The last confirmed reading, which events are measured from.
    confirmed: Option<ExpReading>,
    /// Whether the EXP bar read last tick, so a covered bar is reported once, not every tick.
    readable: bool,
    /// The HP and MP numbers read on the last fresh frame, or `None` when that frame didn't read
    /// them, or capture stopped since: what makes the next reading continuous.
    hp_mp: Option<HpMp>,
    /// Whether the HP and MP numbers read since they were last reported unreadable.
    hp_mp_readable: bool,
    hp_mp_health: HpMpHealth,
    hp_mp_state: HpMpState,
    /// What the engine last saw, for the "What MesoVibe sees" panel and the recorder.
    last_grab: Option<Grab>,
    /// Whether that was a whole frame, read for the map's name or the meso total rather than the
    /// usual strip.
    last_whole: bool,
    /// Grabs in a row that brought no new frame.
    stale: u32,
    /// When the runner started: the clock `hp_mp_health` measures time on.
    started: Instant,
    /// Capturing on the last tick, so a resumed capture checks the map again.
    was_capturing: bool,
    /// The black bar above and below the game's picture in Full Screen, in game pixels, as last
    /// measured: 0 when the picture fills the window.
    bar: u32,
    /// How many lines tall the picture is that the game scales up to fill the game area: 768
    /// until the HUD shows another of the game's sizes.
    source: u32,
    /// The game's picture as last read: the game area's size, the Full Screen bar, and the height
    /// it's scaled up from. When it changes, the readers check what they learned against it.
    picture: Option<(u32, u32, u32, u32)>,
}

impl<B: Backend> Runner<B> {
    pub fn new(backend: B) -> Self {
        Self::with_exp_reader(backend, ExpBarReader::new())
    }

    /// Starts with what an EXP bar reader learned before.
    pub fn with_exp_reader(backend: B, exp: ExpBarReader) -> Self {
        Self::with_readers(backend, exp, MesoReader::new())
    }

    /// Starts with what the EXP bar and meso total readers learned before.
    pub fn with_readers(backend: B, exp: ExpBarReader, meso: MesoReader) -> Self {
        Runner {
            backend,
            state: CaptureState::NoGame,
            window: None,
            exp,
            chat: ChatReader::new(),
            map: MapFollower::new(),
            meso,
            slots: QuickSlotReader::new(),
            tooltips: None,
            tooltip_in_view: false,
            confirmed: None,
            readable: false,
            hp_mp: None,
            hp_mp_readable: false,
            hp_mp_health: HpMpHealth::new(),
            hp_mp_state: HpMpState::Off,
            last_grab: None,
            last_whole: false,
            stale: 0,
            started: Instant::now(),
            was_capturing: false,
            bar: 0,
            source: NATIVE_HEIGHT,
            picture: None,
        }
    }

    /// The rows the engine read on the last tick that captured, exactly as it saw them: the
    /// bottom strip, or now and then the whole game area for the map's name or the meso total.
    pub fn last_grab(&self) -> Option<&Grab> {
        self.last_grab.as_ref()
    }

    /// The last grab when it was the bottom strip. The "What MesoVibe sees" picture shows only
    /// this: the whole game area, read for the map's name or the meso total, holds other players'
    /// names and more.
    pub fn last_strip(&self) -> Option<&Grab> {
        self.last_grab.as_ref().filter(|_| !self.last_whole)
    }

    pub fn state(&self) -> CaptureState {
        self.state
    }

    pub fn exp_reader(&self) -> &ExpBarReader {
        &self.exp
    }

    pub fn meso_reader(&self) -> &MesoReader {
        &self.meso
    }

    /// Switches item tooltips on, with what a reader learned before, or off (`None`): off, no
    /// tooltip is read and no whole frame is grabbed for one.
    pub fn set_tooltip_reader(&mut self, reader: Option<TooltipReader>) {
        self.tooltips = reader;
        self.tooltip_in_view = false;
    }

    /// The tooltip reader while tooltips are on, for keeping what it learned and telling it the
    /// names that aren't items.
    pub fn tooltip_reader_mut(&mut self) -> Option<&mut TooltipReader> {
        self.tooltips.as_mut()
    }

    /// How the meso total is read, as of the last tick: for plugins (the SDK's `Readers.mesos`).
    pub fn meso_state(&self) -> MesoState {
        self.meso.state()
    }

    /// How the quick slots are read, as of the last frame.
    pub fn quick_slot_state(&self) -> QuickSlotState {
        self.slots.state()
    }

    /// How the HP and MP numbers are being read, as of the last tick.
    pub fn hp_mp_state(&self) -> HpMpState {
        self.hp_mp_state
    }

    /// While the HP and MP numbers wait for a digit, the digits the EXP field hasn't shown yet: the
    /// one they wait for is among them, but its shape alone can't say which.
    pub fn hp_mp_missing(&self) -> Option<String> {
        (self.hp_mp_state == HpMpState::MissingDigits).then(|| {
            self.exp
                .missing()
                .exp
                .into_iter()
                .filter(char::is_ascii_digit)
                .collect()
        })
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
            self.map = MapFollower::new();
            self.meso.new_look();
            self.slots.new_look();
            self.readable = false;
            self.hp_mp = None;
            self.hp_mp_readable = false;
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
            hp_mp: None,
            chat: None,
            map: None,
            mesos: None,
            whole: None,
            tooltip: None,
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
            // taller, and the map's name and the meso total need the whole game area. Under a
            // bar, the readers' rows start above it.
            let area = window.height.saturating_sub(self.bar.saturating_mul(2));
            let strip =
                rows_needed(area, self.source).max(self.chat.rows_needed(area, self.source));
            let wants = WholeFrameFor {
                map: self.map.wants_whole_frame(),
                mesos: self.meso.wants_whole_frame((window.width, area)),
                tooltip: self.tooltips.is_some() && self.tooltip_in_view,
            };
            let whole = wants.map || wants.mesos || wants.tooltip;
            let rows = match whole {
                true => window.height,
                false => (self.bar + strip).min(window.height),
            };
            let grab = self.backend.grab(&window, rows);
            match grab {
                Ok(grab) => {
                    tick.whole = whole.then_some(wants);
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
                                // Another size or scale (windowed to Full Screen, or another size
                                // in Full Screen) can draw the HUD differently.
                                let picture = (window.width, window.height, self.bar, self.source);
                                if self.picture.is_some_and(|before| before != picture) {
                                    self.exp.recheck();
                                    self.meso.recheck();
                                    self.hp_mp_health.new_picture();
                                }
                                self.picture = Some(picture);
                                let hud = self.exp.observe_hud(&frame);
                                let reading = hud.exp;
                                // Another of the game's sizes scaled up: read at it from the next
                                // frame, which is grabbed with the rows it needs.
                                if let Some(lines) = exp_bar::other_source(&frame, &reading) {
                                    self.source = lines;
                                }
                                // The game fades to black when the map changes.
                                let dark = matches!(reading, Err(Unreadable::ScreenDark));
                                tick.events = self.events_for(&reading);
                                // To the map, a whole frame it didn't ask for is the strip: its
                                // name is read at its own times only.
                                let for_map = match whole && !wants.map {
                                    true => frame
                                        .bottom_rows(unstretch::scaled_rows(strip, frame.scale())),
                                    false => frame,
                                };
                                if let Some(map) = self.map.observe(&for_map, dark) {
                                    tick.events.push(Event::MapChanged {
                                        map_key: map.as_ref().map(|name| name.key.clone()),
                                    });
                                    tick.map = Some(map);
                                }
                                let hp_mp = self.hp_mp_events(&hud.hp_mp, dark);
                                tick.events.extend(hp_mp);
                                let slots = self.slots.observe(&frame, self.exp.block_offset());
                                tick.events.extend(slots.changed.into_iter().map(
                                    |(slot, read)| Event::QuickSlotChanged {
                                        slot,
                                        count_key: read.count,
                                        icon_key: read.icon,
                                    },
                                ));
                                // A Retina frame's meso box is read where the HUD reader found the
                                // pixel blocks start, rather than undoing the whole frame.
                                if let Some(meso) =
                                    self.meso.observe(&frame, self.exp.block_offset())
                                {
                                    if let Some((total, source)) = meso.send {
                                        tick.events.push(Event::MesoChanged { total, source });
                                    }
                                    tick.mesos = Some(meso.view);
                                }
                                // The same whole frame shows any item tooltip, for the label.
                                if whole && let Some(reader) = self.tooltips.as_mut() {
                                    let reading = reader.observe(&frame, self.exp.block_offset());
                                    self.tooltip_in_view = reading.tooltip.is_some();
                                    tick.tooltip = Some(reading);
                                }
                                tick.exp = Some(reading);
                                tick.hp_mp = Some(hud.hp_mp);
                                tick.chat = Some(self.chat.observe(&frame));
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
        // A tick that read no frame, other than a stale grab while capturing, may have missed a
        // change: the next reading isn't continuous.
        if self.state != CaptureState::Capturing || (tick.hp_mp.is_none() && !tick.stale) {
            self.hp_mp = None;
        }
        debug_assert!(tick.events.is_empty() || self.state.emits_events());
        tick.state = self.state;
        tick.source = self.source;
        tick.window = found.map(|window| letterbox::in_window(window, self.bar));
        let capturing = self.state == CaptureState::Capturing;
        // Back from a pause: the player may have taken a portal, or spent mesos, meanwhile.
        if capturing && !self.was_capturing {
            self.map.recheck();
            self.meso.new_look();
            self.slots.new_look();
        }
        self.was_capturing = capturing;
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let read = tick.exp.as_ref().zip(tick.hp_mp.as_ref());
        let confirmed = self.exp.calibration() == exp_bar::Calibration::Confirmed;
        self.hp_mp_state = self.hp_mp_health.observe(now, capturing, read, confirmed);
        tick
    }

    /// The events a frame's HP and MP reading brings: `HpMpChanged` when the numbers changed or the
    /// reading isn't continuous, and `Unreadable` once when they stop reading. As for the EXP bar,
    /// a dark screen or a reader still learning isn't "can't see the numbers".
    fn hp_mp_events(&mut self, reading: &Result<HpMp, HpMpUnreadable>, dark: bool) -> Vec<Event> {
        let before = self.hp_mp.take();
        match reading {
            Ok(r) => {
                self.hp_mp = Some(*r);
                self.hp_mp_readable = true;
                if before == Some(*r) {
                    return Vec::new();
                }
                vec![Event::HpMpChanged {
                    hp: r.hp,
                    max_hp: r.max_hp,
                    mp: r.mp,
                    max_mp: r.max_mp,
                    continuous: before.is_some(),
                }]
            }
            Err(HpMpUnreadable::Unchecked | HpMpUnreadable::LearningSlash) => Vec::new(),
            Err(_) if dark => Vec::new(),
            Err(_) if self.hp_mp_readable => {
                self.hp_mp_readable = false;
                vec![Event::Unreadable { recognizer: HP_MP }]
            }
            Err(_) => Vec::new(),
        }
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
    use crate::events::MesoSource;
    use crate::recognizers::exp_bar::test_support::{Canvas, DARK, GRIND, Hud, NEEDED_41, pct_of};
    use crate::recognizers::map_name::CHECK_EVERY;
    use crate::recognizers::map_name::test_support::Minimap;
    use crate::recognizers::meso_total::test_support::{MesoWindow, Window};

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
                exp_shown: true,
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
        // Whole frames the map asked for, counted apart from the meso total's.
        let mut for_map = 0;
        let mut maps = |runner: &mut Runner<Fake>, ticks: usize| -> Vec<Option<String>> {
            let mut found = Vec::new();
            for _ in 0..ticks {
                let tick = runner.tick();
                for_map += usize::from(tick.whole.is_some_and(|w| w.map));
                found.extend(tick.events.into_iter().filter_map(|e| match e {
                    Event::MapChanged { map_key } => Some(map_key),
                    _ => None,
                }));
            }
            found
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
        // Two whole frames for each map; any others were the meso total's.
        assert_eq!(for_map, 4);
    }

    // A player can take a portal while capture is paused (the game minimized). Fails if the map
    // isn't read again as soon as capture resumes, which would file the new map's play under the
    // old one for up to 30 s.
    #[test]
    fn the_map_is_read_again_when_capture_resumes() {
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
        let whole = (0..3)
            .filter(|_| runner.tick().whole.is_some_and(|w| w.map))
            .count();
        assert!(whole > 0, "the map was read again");
    }

    // The "What MesoVibe sees" picture shows only the bottom strip. Fails if a whole frame read
    // for the map's name, which holds chat and other players' names, is offered for it.
    #[test]
    fn only_strips_are_offered_for_the_picture() {
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
                exp_shown: true,
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
            matches!(tick.exp, Some(Ok(r)) if r.exp == Some(last)),
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
            matches!(tick.exp, Some(Ok(r)) if r.exp == Some(last + 21)),
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
                exp_shown: true,
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
            matches!(tick.exp, Some(Ok(r)) if r.exp == Some(last + 21)),
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

    /// The HUD at `exp` in a `width` x `height` game area, with HP `[hp/1386]` and MP
    /// `[mp/1140]`.
    fn hud_hp_at(hud: &Hud, (width, height): (u32, u32), exp: u64, hp: u32, mp: u32) -> Canvas {
        let field = exp_bar::exp_text(exp, pct_of(exp, NEEDED_41));
        let mut canvas = hud.draw(width, height, &field, "41");
        hud.draw_hp_mp(
            &mut canvas,
            &format!("[{hp}/1386]"),
            &format!("[{mp}/1140]"),
        );
        canvas
    }

    fn hud_hp(hud: &Hud, exp: u64, hp: u32, mp: u32) -> Canvas {
        hud_hp_at(hud, (1366, 768), exp, hp, mp)
    }

    /// The HP and MP events of a tick.
    fn hp_mp_events(tick: &Tick) -> Vec<Event> {
        tick.events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::HpMpChanged { .. } | Event::Unreadable { recognizer: HP_MP }
                )
            })
            .cloned()
            .collect()
    }

    fn changed(hp: u32, mp: u32, continuous: bool) -> Event {
        Event::HpMpChanged {
            hp,
            max_hp: 1386,
            mp,
            max_mp: 1140,
            continuous,
        }
    }

    // The digits the panel names while HP and MP wait for one are digits only, and only in that
    // state. Fails if the EXP field's brackets or punctuation reach the panel, or the list shows
    // while the numbers read or are covered.
    #[test]
    fn the_missing_digits_are_named_only_while_waiting_for_one() {
        let mut runner = Runner::new(Fake::default());
        runner.hp_mp_state = HpMpState::MissingDigits;
        assert_eq!(runner.hp_mp_missing().as_deref(), Some("0123456789"));
        for state in [HpMpState::Reading, HpMpState::Covered, HpMpState::Learning] {
            runner.hp_mp_state = state;
            assert_eq!(runner.hp_mp_missing(), None, "{state:?}");
        }
    }

    // HP and MP reach plugins when they change, and `continuous` is true only when the frame just
    // before read them too. Fails if a reading after a covered HUD, a dark screen, a pause, a
    // window closed or reopened, a game restart or a new size claims to follow on (a plugin would
    // count what happened unseen as one rise), or a stale grab breaks the chain.
    #[test]
    fn hp_and_mp_changes_say_whether_they_follow_on() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            ..Fake::default()
        });
        let mut seen = Vec::new();
        for &exp in &GRIND {
            runner.backend_mut().canvas = Some(hud_hp(&hud, exp, 700, 600));
            seen.extend(hp_mp_events(&runner.tick()));
        }
        assert_eq!(seen, [changed(700, 600, false)], "the first reading");
        let last = *GRIND.last().unwrap();
        let show = |runner: &mut Runner<Fake>, hp: u32, mp: u32| {
            runner.backend_mut().canvas = Some(hud_hp(&hud, last, hp, mp));
            hp_mp_events(&runner.tick())
        };
        assert_eq!(show(&mut runner, 700, 600), [], "unchanged");
        assert_eq!(show(&mut runner, 540, 600), [changed(540, 600, true)]);

        // A stale grab reads nothing and keeps the chain.
        runner.backend_mut().frozen = true;
        let tick = runner.tick();
        assert_eq!((tick.hp_mp, hp_mp_events(&tick)), (None, vec![]));
        runner.backend_mut().frozen = false;
        assert_eq!(show(&mut runner, 380, 600), [changed(380, 600, true)]);

        // Covered: reported once; the next reading starts a new chain, even unchanged.
        let mut covered = hud_hp(&hud, last, 380, 600);
        covered.fill(283 + 430, 768 - 34, 40, 14, DARK);
        runner.backend_mut().canvas = Some(covered);
        let unreadable = Event::Unreadable { recognizer: HP_MP };
        assert_eq!(hp_mp_events(&runner.tick()), [unreadable]);
        assert_eq!(hp_mp_events(&runner.tick()), []);
        assert_eq!(
            show(&mut runner, 380, 600),
            [changed(380, 600, false)],
            "after covered"
        );

        // A map change's fade to black isn't "can't see", but it breaks the chain.
        runner.backend_mut().canvas = Some(Canvas::new(1366, 768));
        assert_eq!(hp_mp_events(&runner.tick()), []);
        assert_eq!(
            show(&mut runner, 380, 600),
            [changed(380, 600, false)],
            "after dark"
        );

        // Minimized, then back.
        runner.backend_mut().window = Some(GameWindow {
            minimized: true,
            ..GAME
        });
        runner.tick();
        runner.backend_mut().window = Some(GAME);
        assert_eq!(
            show(&mut runner, 380, 600),
            [changed(380, 600, false)],
            "after a pause"
        );

        // Frames stopped coming long enough to pause, then came back.
        runner.backend_mut().frozen = true;
        for _ in 0..STALE_AFTER + 1 {
            runner.tick();
        }
        runner.backend_mut().frozen = false;
        let back = show(&mut runner, 380, 600);
        assert_eq!(back, [changed(380, 600, false)], "after frames stopped");

        // The window closed and opened again, and then a restarted game.
        runner.backend_mut().window = None;
        runner.tick();
        runner.backend_mut().window = Some(GAME);
        assert_eq!(
            show(&mut runner, 380, 600),
            [changed(380, 600, false)],
            "reopened"
        );
        runner.backend_mut().window = Some(GameWindow { id: 8, ..GAME });
        assert_eq!(
            show(&mut runner, 380, 600),
            [changed(380, 600, false)],
            "restarted"
        );

        // Another size: the shapes are checked again and the slash learned again first.
        let size = (1024, 768);
        runner.backend_mut().window = Some(GameWindow {
            id: 8,
            width: 1024,
            ..GAME
        });
        let mut after = Vec::new();
        for _ in 0..3 {
            runner.backend_mut().canvas = Some(hud_hp_at(&hud, size, last, 380, 600));
            after.extend(hp_mp_events(&runner.tick()));
        }
        assert_eq!(after, [changed(380, 600, false)], "after a new size");
    }

    // A tick's events come in one order (the EXP bar's, the map's, then HP and MP's), as the SDK
    // promises plugins. Fails if HpMpChanged ever comes before an event of the same tick that it
    // should follow.
    #[test]
    fn a_ticks_events_come_in_a_fixed_order() {
        let rank = |e: &Event| match e {
            Event::ExpChanged { .. } => 0,
            Event::LevelUp { .. } => 1,
            Event::Unreadable { recognizer } if *recognizer == EXP_BAR => 1,
            // Pickup events: in the schema, but the live engine doesn't send them.
            Event::MesosGained { .. }
            | Event::ItemGained { .. }
            | Event::PartyBonus { .. }
            | Event::NoticesMissed { .. } => 2,
            Event::MapChanged { .. } => 3,
            Event::HpMpChanged { .. } | Event::Unreadable { .. } => 4,
            Event::QuickSlotChanged { .. } => 5,
            Event::MesoChanged { .. } => 6,
        };
        let hud = Hud::new();
        let on_map = |name: &str, exp: u64, hp: u32| {
            let mut canvas = hud_hp(&hud, exp, hp, 600);
            Minimap::new("Victoria Road", name).draw(&mut canvas);
            // The inventory open, its total rising every third frame.
            let total = 5_170_316 + u64::from(900 - hp) / 3 * 50;
            MesoWindow::new(Window::Inventory, total).draw(&mut canvas);
            // A potion count in the Shift slot going down every other frame.
            let corner = quick_slots::test_support::corner(1366, 768);
            let count = 300 - (900 - hp) / 2;
            let slots = [quick_slots::test_support::item(1, count)];
            quick_slots::test_support::draw_panel(&mut canvas, corner, &slots);
            canvas
        };
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            ..Fake::default()
        });
        let mut ticks = Vec::new();
        for (i, &exp) in GRIND.iter().enumerate() {
            runner.backend_mut().canvas = Some(on_map("Forest I", exp, 900 - i as u32));
            ticks.push(runner.tick().events);
        }
        // A map change: dark, then the new map, with EXP and HP moving on every frame.
        runner.backend_mut().canvas = Some(Canvas::new(1366, 768));
        ticks.push(runner.tick().events);
        let last = *GRIND.last().unwrap();
        for i in 1..=6 {
            runner.backend_mut().canvas = Some(on_map("Henesys", last + 21 * i, 800 - i as u32));
            ticks.push(runner.tick().events);
        }
        for events in &ticks {
            let ranks: Vec<u8> = events.iter().map(rank).collect();
            assert!(ranks.is_sorted(), "{events:?}");
        }
        let has = |events: &Vec<Event>, f: fn(&Event) -> bool| events.iter().any(f);
        let hp = |e: &Event| matches!(e, Event::HpMpChanged { .. });
        let exp = |e: &Event| matches!(e, Event::ExpChanged { .. });
        let map = |e: &Event| matches!(e, Event::MapChanged { .. });
        assert!(ticks.iter().any(|t| has(t, hp) && has(t, exp)));
        assert!(
            ticks.iter().any(|t| has(t, hp) && has(t, map)),
            "a tick with a map change and HP: {ticks:?}"
        );
        let meso = |e: &Event| matches!(e, Event::MesoChanged { .. });
        assert!(
            ticks.iter().any(|t| has(t, hp) && has(t, meso)),
            "a tick with a meso total and HP: {ticks:?}"
        );
        let slot = |e: &Event| matches!(e, Event::QuickSlotChanged { .. });
        assert!(
            ticks
                .iter()
                .any(|t| has(t, hp) && has(t, slot) && has(t, meso)),
            "a tick with HP, a quick slot and a meso total: {ticks:?}"
        );
    }

    /// The HUD at `exp` with HP `hp`, the minimap, and `windows` open.
    fn scene(hud: &Hud, exp: u64, hp: u32, windows: &[&MesoWindow]) -> Canvas {
        let mut canvas = hud_hp(hud, exp, hp, 600);
        Minimap::new("Victoria Road", "Forest I").draw(&mut canvas);
        for window in windows {
            window.draw(&mut canvas);
        }
        canvas
    }

    // With the price label on, a tooltip seen on a whole frame keeps whole frames coming every
    // tick until it's gone, so one first seen on a once-a-second look is followed; with it off,
    // no tooltip is read and no whole frame is grabbed for one. Fails if tooltips cost whole
    // frames while switched off, or go unread while on.
    #[test]
    fn a_tooltip_in_view_keeps_whole_frames_coming_while_the_label_is_on() {
        let hud = Hud::new();
        let plain = scene(&hud, GRIND[0], 700, &[]);
        let mut with_tooltip = scene(&hud, GRIND[0], 700, &[]);
        let width = with_tooltip.width;
        crate::recognizers::tooltip::test_support::draw(
            &mut with_tooltip.bgra,
            width,
            400,
            150,
            288,
            113,
            "Mithril Ingot",
            false,
        );
        for on in [false, true] {
            let mut runner = Runner::new(Fake {
                window: Some(GAME),
                canvas: Some(with_tooltip.clone()),
                ..Fake::default()
            });
            runner.set_tooltip_reader(on.then(TooltipReader::new));
            let ticks: Vec<Tick> = (0..12).map(|_| runner.tick()).collect();
            let for_tooltip = ticks
                .iter()
                .filter(|t| t.whole.is_some_and(|w| w.tooltip))
                .count();
            let read = ticks
                .iter()
                .filter(|t| t.tooltip.as_ref().is_some_and(|r| r.tooltip.is_some()))
                .count();
            if on {
                assert!(read >= 8, "read on most ticks: {read}");
                assert!(for_tooltip >= 7, "{for_tooltip}");
                // Gone: whole frames go back to the meso total's once a second.
                runner.backend_mut().canvas = Some(plain.clone());
                let after: Vec<Tick> = (0..8).map(|_| runner.tick()).collect();
                assert!(
                    after
                        .iter()
                        .skip(1)
                        .all(|t| !t.whole.is_some_and(|w| w.tooltip))
                );
            } else {
                assert_eq!((read, for_tooltip), (0, 0));
                assert!(ticks.iter().all(|t| t.tooltip.is_none()));
            }
        }
    }

    // Map reads keep their own times among the meso reader's whole frames, counted by who asked.
    // Fails if meso frames are taken for map reads, or a map read comes early, late or never.
    #[test]
    fn map_reads_keep_their_times_among_the_meso_totals_whole_frames() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(scene(&hud, GRIND[0], 700, &[])),
            ..Fake::default()
        });
        let (mut for_map, mut for_mesos, mut whole) = (Vec::new(), 0, Vec::new());
        let ticks = 4 + CHECK_EVERY as usize + 4;
        for n in 1..=ticks {
            if let Some(asked) = runner.tick().whole {
                whole.push(n);
                if asked.map {
                    for_map.push(n);
                }
                for_mesos += usize::from(asked.mesos);
            }
        }
        assert_eq!(for_map, [2, 3, 4 + CHECK_EVERY as usize]);
        // The meso total looks about once a second while no box is in view: a whole frame at
        // least every fourth tick, its own or the map's.
        assert!(whole.windows(2).all(|w| w[1] - w[0] <= 4), "{whole:?}");
        assert!(for_mesos >= ticks / 4 - 2, "{for_mesos}");
    }

    // Whole frames read for the meso total never reach the "What MesoVibe sees" picture. Fails
    // if one, which holds other players' names, is offered for the picture.
    #[test]
    fn meso_whole_frames_never_reach_the_picture() {
        let hud = Hud::new();
        let inventory = MesoWindow::new(Window::Inventory, 5_170_316);
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(scene(&hud, GRIND[0], 700, &[&inventory])),
            ..Fake::default()
        });
        let height = |g: &Grab| g.height - g.first_row;
        let mut meso_frames = 0;
        for _ in 0..12 {
            let tick = runner.tick();
            meso_frames += usize::from(tick.whole.is_some_and(|w| w.mesos));
            assert_ne!(runner.last_strip().map(height), Some(768));
            if tick.whole.is_some() {
                assert_eq!(runner.last_grab().map(height), Some(768));
            }
        }
        assert!(
            meso_frames >= 10,
            "every tick once the box is in view: {meso_frames}"
        );
    }

    // When the map and the meso total both want a whole frame on the same tick, one grab serves
    // both. Fails if the tick grabs twice, or either reader misses the frame.
    #[test]
    fn one_whole_frame_serves_the_map_and_the_meso_total() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            canvas: Some(scene(&hud, GRIND[0], 700, &[])),
            ..Fake::default()
        });
        for _ in 0..6 {
            runner.tick();
        }
        // Minimized and back: both read again at once.
        runner.backend_mut().window = Some(GameWindow {
            minimized: true,
            ..GAME
        });
        runner.tick();
        runner.backend_mut().window = Some(GAME);
        runner.tick();
        let grabs = runner.backend_mut().grabs.len();
        let tick = runner.tick();
        assert_eq!(
            tick.whole,
            Some(WholeFrameFor {
                map: true,
                mesos: true,
                tooltip: false
            })
        );
        assert_eq!(runner.backend_mut().grabs.len(), grabs + 1, "one grab");
        assert_eq!(
            tick.mesos,
            Some(MesoView::NotOpen),
            "the meso total looked at it"
        );
    }

    // The EXP bar, HP and MP and chat read every tick's frame, whole or not, and HP and MP stay
    // continuous across whole frames. Fails if a whole frame skips a reader or breaks the chain.
    #[test]
    fn exp_hp_mp_and_chat_read_every_tick_whole_or_not() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            ..Fake::default()
        });
        let (mut whole, mut strips) = (0, 0);
        let mut changes = Vec::new();
        for (i, &exp) in GRIND.iter().chain(&GRIND).enumerate() {
            let hp = 900 - i as u32;
            runner.backend_mut().canvas = Some(scene(&hud, exp, hp, &[]));
            let tick = runner.tick();
            assert!(
                tick.exp.is_some() && tick.hp_mp.is_some() && tick.chat.is_some(),
                "tick {i}"
            );
            match tick.whole {
                Some(_) => whole += 1,
                None => strips += 1,
            }
            changes.extend(hp_mp_events(&tick));
        }
        assert!(whole >= 4 && strips >= 4, "{whole} whole, {strips} strips");
        // Once read, every change follows on from the one before.
        assert!(changes.len() > 10, "{changes:?}");
        assert!(
            changes[1..].iter().all(|e| matches!(
                e,
                Event::HpMpChanged {
                    continuous: true,
                    ..
                }
            )),
            "{changes:?}"
        );
    }

    // A meso total reaches plugins once named and read the same twice, then when it changes.
    // Fails if a total is sent from one frame, sent again unchanged, or never.
    #[test]
    fn trusted_meso_totals_reach_plugins() {
        let hud = Hud::new();
        let mut runner = Runner::new(Fake {
            window: Some(GAME),
            ..Fake::default()
        });
        assert_eq!(runner.meso_state(), MesoState::Learning);
        let mut sent = Vec::new();
        for (i, &exp) in GRIND.iter().enumerate() {
            let total = if i < 10 { 5_170_316 } else { 5_170_369 };
            let inventory = MesoWindow::new(Window::Inventory, total);
            runner.backend_mut().canvas = Some(scene(&hud, exp, 700, &[&inventory]));
            sent.extend(runner.tick().events.into_iter().filter_map(|e| match e {
                Event::MesoChanged { total, source } => Some((total, source)),
                _ => None,
            }));
        }
        assert_eq!(
            sent,
            [
                (5_170_316, MesoSource::Inventory),
                (5_170_369, MesoSource::Inventory)
            ]
        );
        assert_eq!(runner.meso_state(), MesoState::Reading);
        assert!(runner.meso_reader().knowledge().is_some());
    }

    // At a game size the meso box hasn't been checked at, no whole frame is read for it and
    // plugins hear it can't be read. Fails if the engine reads whole frames it can't use.
    #[test]
    fn no_meso_frames_at_an_unchecked_size() {
        let hud = Hud::new();
        let window = GameWindow {
            width: 1024,
            ..GAME
        };
        let mut runner = Runner::new(Fake {
            window: Some(window),
            ..Fake::default()
        });
        for &exp in &GRIND {
            runner.backend_mut().canvas = Some(hud_hp_at(&hud, (1024, 768), exp, 700, 600));
            let tick = runner.tick();
            assert!(!tick.whole.is_some_and(|w| w.mesos));
        }
        assert_eq!(runner.meso_state(), MesoState::Unsupported);
    }
}
