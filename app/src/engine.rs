//! Runs the engine (capture, recognizers, typed events) on its own thread and reports to the panel.
//!
//! The thread owns the platform's capture backend and ticks the engine at 4 Hz while reading is
//! on and a game window is known. With no game window it looks for one every 2 s, the way
//! MesoVibe waits in the tray for hours (presence.rs). Each tick sends the panel a small status
//! (`engine://status`) and each event (`engine://event`), and new whisper, party and friend lines
//! go to the Chat Hub's log and to MesoVibe's own windows only (`engine://chat`).
//! While the "What MesoVibe sees" view is open, it also sends the exact rows the engine read, as
//! a PNG, at most once a second (`engine://snapshot`). Each tick also keeps MesoVibe's windows
//! with the game window (`dock.rs`) and decides whether the panel should be built, hidden or
//! destroyed (`presence.rs`). Nothing is captured while reading is off (Safe mode).
//!
//! The shapes the readers confirm (the EXP bar's digits, the pickup notices' letters) are kept on
//! this computer (`Store`), and the next time reading starts the readers begin from them, so they
//! read level, EXP and pickups from the first frame instead of learning again.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mesovibe_engine::capture::{Backend, CaptureState, Grab};
use mesovibe_engine::events::Event;
use mesovibe_engine::notice_health::NoticeState;
use mesovibe_engine::recognizers::chat::ChatUnreadable;
use mesovibe_engine::recognizers::exp_bar::{Calibration, ExpBarReader, Unreadable};
use mesovibe_engine::recognizers::notices::NoticeReader;
use mesovibe_engine::runner::{Runner, Tick};
use mesovibe_engine::unstretch;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::chat::ChatLog;
use crate::dock::{self, Docker};
use crate::presence::{self, Presence};
use crate::store::{Learned, Store};

/// One engine tick: 4 Hz, the capture ceiling.
const TICK: Duration = Duration::from_millis(250);
/// The wait between looks for the game while no game window is known: the game takes far longer
/// than this to start, and MesoVibe may wait like this for hours.
const IDLE_TICK: Duration = Duration::from_secs(2);
/// How often the engine checks for commands while reading is off.
const OFF_WAIT: Duration = Duration::from_millis(500);
/// How often the "What MesoVibe sees" thumbnail updates while it's open.
const SNAPSHOT_EVERY: Duration = Duration::from_secs(1);

#[cfg(target_os = "macos")]
type Platform = mesovibe_engine::capture::macos::MacBackend;
#[cfg(windows)]
type Platform = mesovibe_engine::capture::windows::WindowsBackend;
#[cfg(not(any(windows, target_os = "macos")))]
type Platform = Unsupported;

/// What the panel asks of the engine.
pub enum Command {
    /// Reading on (capture) or off (Safe mode).
    SetReading(bool),
    /// The player fixed what blocked capture: look again.
    Retry,
    /// The "What MesoVibe sees" view opened or closed.
    Watch(bool),
}

/// The handle the app keeps to its engine thread.
pub struct Engine {
    commands: Mutex<Sender<Command>>,
}

impl Engine {
    pub fn start(app: AppHandle, reading: bool) -> Engine {
        let (commands, inbox) = mpsc::channel();
        std::thread::Builder::new()
            .name("mesovibe-engine".to_string())
            .spawn(move || run(&app, &inbox, reading))
            .expect("start the engine thread");
        Engine {
            commands: Mutex::new(commands),
        }
    }

    pub fn send(&self, command: Command) {
        let commands = self
            .commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = commands.send(command);
    }
}

/// What the panel shows about capture and the EXP bar, every tick.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Reading is switched on.
    pub reading: bool,
    /// `off`, `no_game`, `capturing`, `paused` or `blocked`.
    pub state: &'static str,
    pub game: Option<Game>,
    /// The game window covers its whole display: Full Screen, where the game scales its picture
    /// to the display and only some of its sizes can be read.
    pub full_screen: bool,
    /// The EXP bar this tick, when it was read.
    pub exp: Option<Reading>,
    /// Why the EXP bar couldn't be read this tick (`learning`, `screen_dark`, `hud_not_found`,
    /// ...), when it wasn't.
    pub unreadable: Option<&'static str>,
    /// Why capture is blocked, in words the player can act on.
    pub error: Option<String>,
    /// The chat box this tick: `reading`, `not_found`, `scrolled_back` or `unsupported`, when
    /// capturing.
    pub chat: Option<&'static str>,
    /// `learning`, `provisional` or `confirmed`.
    pub calibration: &'static str,
    /// This tick brought no new frame.
    pub stale: bool,
    /// The pickup notices: `off`, `learning`, `reading`, `covered`, `blocked` or `not_found`.
    pub notices: NoticeState,
    /// Mesos amounts waiting for a digit to be learned before they're counted.
    pub waiting_mesos: usize,
    /// The map the player is on: `None` before one settled; its key, or no key when its name
    /// can't be read.
    pub map: Option<MapStatus>,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapStatus {
    pub key: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub width: u32,
    pub height: u32,
    pub scale: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub level: Option<u16>,
    pub exp: u64,
    pub pct_hundredths: u16,
    pub needed: Option<u64>,
    pub confirmed: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Stamped<'a> {
    at_ms: u64,
    event: &'a Event,
}

/// The wait after a tick in `state`.
fn tick_wait(state: CaptureState) -> Duration {
    match state {
        CaptureState::NoGame => IDLE_TICK,
        CaptureState::Capturing | CaptureState::Paused | CaptureState::CaptureBlocked => TICK,
    }
}

fn run(app: &AppHandle, inbox: &Receiver<Command>, mut reading: bool) {
    let mut runner: Option<Runner<Platform>> = None;
    let mut docker = Docker::default();
    let mut presence = Presence::default();
    // A command that ended a long wait early, handled first on the next tick.
    let mut woke: Option<Command> = None;
    let mut watching = false;
    let mut last_snapshot: Option<Instant> = None;
    // Why capture is blocked, kept while it stays blocked: the error comes on one tick only.
    let mut blocked: Option<String> = None;
    let mut kept = Kept::default();
    loop {
        let started = Instant::now();
        for command in woke.take().into_iter().chain(inbox.try_iter()) {
            match command {
                Command::SetReading(on) => reading = on,
                Command::Retry => {
                    if let Some(runner) = runner.as_mut() {
                        runner.retry();
                    }
                    blocked = None;
                }
                Command::Watch(on) => {
                    watching = on;
                    last_snapshot = None;
                }
            }
        }
        let wait = if reading {
            let (state, grabbed) = tick(
                app,
                &mut runner,
                &mut docker,
                &mut presence,
                &mut blocked,
                &mut kept,
            );
            let due = last_snapshot.is_none_or(|t| t.elapsed() >= SNAPSHOT_EVERY);
            if watching && grabbed && due {
                // Only the bottom strip, never a whole frame read for the map's name; a tick
                // that read one leaves the picture to the next.
                let strip = runner.as_ref().and_then(|runner| runner.last_strip());
                if let Some(strip) = strip {
                    if let Some(png) = snapshot(strip) {
                        let _ = app.emit("engine://snapshot", png);
                    }
                    last_snapshot = Some(Instant::now());
                }
            }
            tick_wait(state)
        } else {
            // Dropping the runner stops capture and frees what it holds.
            runner = None;
            blocked = None;
            docker.rest(app);
            presence.follow(app, presence::Game::Unknown, None);
            let _ = app.emit("engine://status", off_status());
            OFF_WAIT
        };
        let left = wait.saturating_sub(started.elapsed());
        if wait > TICK {
            // A long wait ends early for a command, so turning reading on or retrying is instant.
            match inbox.recv_timeout(left) {
                Ok(command) => woke = Some(command),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            // A tick's own wait runs out, so reading never goes past 4 Hz.
            std::thread::sleep(left);
        }
    }
}

/// One tick of reading: capture, report, and keep MesoVibe's windows with the game. Returns the
/// capture state, and whether the engine grabbed rows (for the "What MesoVibe sees" thumbnail).
fn tick(
    app: &AppHandle,
    runner: &mut Option<Runner<Platform>>,
    docker: &mut Docker,
    presence: &mut Presence,
    blocked: &mut Option<String>,
    kept: &mut Kept,
) -> (CaptureState, bool) {
    let runner = runner.get_or_insert_with(|| {
        let (runner, now) = kept_runner(app);
        *kept = now;
        runner
    });
    let tick = runner.tick();
    let exp_bar = runner.exp_reader().knowledge();
    keep(app, Learned::ExpBar, exp_bar, &mut kept.exp_bar);
    let notices = runner.notice_reader().knowledge();
    keep(app, Learned::Notices, notices, &mut kept.notices);
    if let Some(error) = &tick.error {
        *blocked = Some(error.0.clone());
    }
    if tick.state != CaptureState::CaptureBlocked {
        *blocked = None;
    }
    let full_screen = tick
        .window
        .as_ref()
        .and_then(|w| w.placement.as_ref())
        .is_some_and(|p| dock::fills_display(p, &dock::displays(app)));
    let _ = app.emit(
        "engine://status",
        status(&tick, runner, blocked.clone(), full_screen),
    );
    let chat_box = match &tick.chat {
        Some(Ok(reading)) => reading.place,
        _ => None,
    };
    docker.follow(app, tick.window.as_ref(), chat_box);
    let game = match &tick.window {
        Some(window) => presence::Game::Present {
            placed: window.placement.is_some(),
        },
        None => presence::Game::Absent,
    };
    let placement = tick.window.as_ref().and_then(|w| w.placement.as_ref());
    presence.follow(app, game, placement);
    let at_ms = now_ms();
    // A settled map's name picture, kept so Past sessions can show it later.
    if let Some(Some(name)) = &tick.map
        && let Err(e) =
            app.state::<crate::game_text::GameText>()
                .remember(&name.key, &name.picture, at_ms)
    {
        eprintln!("Couldn't keep the map's name picture: {e}");
    }
    for event in &tick.events {
        let _ = app.emit("engine://event", Stamped { at_ms, event });
    }
    if let (Some(Ok(reading)), Some(log)) = (&tick.chat, app.try_state::<ChatLog>()) {
        let lines = log.record(reading, at_ms);
        if !lines.is_empty() {
            // Only to MesoVibe's own panel and chat overlay, never to every window.
            for window in [dock::PANEL, dock::CHAT] {
                let _ = app.emit_to(window, "engine://chat", &lines);
            }
        }
    }
    (tick.state, tick.exp.is_some() || tick.stale)
}

/// The readers' confirmed shapes as last kept on this computer.
#[derive(Default)]
struct Kept {
    exp_bar: Option<String>,
    notices: Option<String>,
}

/// A runner whose readers start from the shapes kept on this computer, where they still read,
/// and the text each would keep.
fn kept_runner(app: &AppHandle) -> (Runner<Platform>, Kept) {
    let store = app.try_state::<Store>();
    let text = |reader| store.as_ref().and_then(|store| store.knowledge(reader));
    let exp = text(Learned::ExpBar)
        .and_then(|text| ExpBarReader::from_knowledge(&text))
        .unwrap_or_default();
    let notices = text(Learned::Notices)
        .and_then(|text| NoticeReader::from_knowledge(&text))
        .unwrap_or_default();
    let kept = Kept {
        exp_bar: exp.knowledge(),
        notices: notices.knowledge(),
    };
    (Runner::with_readers(platform(), exp, notices), kept)
}

/// Keeps a reader's confirmed shapes when they change (one more learned), and forgets them when
/// the reader dropped them because they no longer fit the game: a confirmed reader has knowledge
/// until it drops it.
fn keep(app: &AppHandle, reader: Learned, now: Option<String>, kept: &mut Option<String>) {
    if now == *kept {
        return;
    }
    let Some(store) = app.try_state::<Store>() else {
        return;
    };
    if store.keep_knowledge(reader, now.as_deref()).is_ok() {
        *kept = now;
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn platform() -> Platform {
    Platform::new()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform() -> Platform {
    Unsupported
}

/// No capture on this OS: the game is never found.
#[cfg(not(any(windows, target_os = "macos")))]
pub struct Unsupported;

#[cfg(not(any(windows, target_os = "macos")))]
impl Backend for Unsupported {
    fn find(&mut self) -> Option<mesovibe_engine::capture::GameWindow> {
        None
    }
    fn grab(
        &mut self,
        _window: &mesovibe_engine::capture::GameWindow,
        _rows: u32,
    ) -> Result<Grab, mesovibe_engine::capture::CaptureError> {
        Err(mesovibe_engine::capture::CaptureError(
            "MesoVibe captures on Windows and macOS only.".to_string(),
        ))
    }
    fn stop(&mut self) {}
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn off_status() -> Status {
    Status {
        reading: false,
        state: "off",
        game: None,
        full_screen: false,
        exp: None,
        unreadable: None,
        error: None,
        chat: None,
        calibration: "learning",
        stale: false,
        notices: NoticeState::Off,
        waiting_mesos: 0,
        map: None,
        at_ms: now_ms(),
    }
}

fn status<B: Backend>(
    tick: &Tick,
    runner: &Runner<B>,
    blocked: Option<String>,
    full_screen: bool,
) -> Status {
    let (exp, unreadable) = match &tick.exp {
        Some(Ok(r)) => (
            Some(Reading {
                level: r.level,
                exp: r.exp,
                pct_hundredths: r.pct_hundredths,
                needed: r.needed,
                confirmed: r.confirmed,
            }),
            None,
        ),
        Some(Err(why)) => (None, Some(reason(why))),
        None => (None, None),
    };
    Status {
        reading: true,
        state: match tick.state {
            CaptureState::NoGame => "no_game",
            CaptureState::Capturing => "capturing",
            CaptureState::Paused => "paused",
            CaptureState::CaptureBlocked => "blocked",
        },
        game: tick.window.map(|w| Game {
            width: w.width,
            height: w.height,
            scale: w.scale,
        }),
        full_screen,
        exp,
        unreadable,
        error: blocked,
        chat: tick.chat.as_ref().map(|chat| match chat {
            Ok(_) => "reading",
            Err(ChatUnreadable::NotFound) => "not_found",
            Err(ChatUnreadable::ScrolledBack) => "scrolled_back",
            Err(ChatUnreadable::Unsupported(_)) => "unsupported",
        }),
        calibration: match runner.exp_reader().calibration() {
            Calibration::Learning => "learning",
            Calibration::Provisional => "provisional",
            Calibration::Confirmed => "confirmed",
        },
        stale: tick.stale,
        notices: runner.notice_state(),
        waiting_mesos: runner.notice_reader().waiting_mesos(),
        map: runner.current_map().map(|key| MapStatus {
            key: key.map(str::to_string),
        }),
        at_ms: now_ms(),
    }
}

/// A stable name for why the EXP bar wasn't read, for the panel.
pub fn reason(why: &Unreadable) -> &'static str {
    match why {
        Unreadable::UnsupportedSize { .. } => "unsupported_size",
        Unreadable::UnsupportedScale { .. } => "unsupported_scale",
        Unreadable::NotCalibrated => "not_calibrated",
        Unreadable::Learning => "learning",
        Unreadable::HudNotFound => "hud_not_found",
        Unreadable::ScreenDark => "screen_dark",
        Unreadable::NoText(_) => "no_text",
        Unreadable::UnknownGlyph(_) => "unknown_glyph",
        Unreadable::Garbled(_) => "garbled",
    }
}

/// The rows the engine read, at the game's own pixel size, as a PNG data URL: exactly what
/// MesoVibe sees, never more. It stays on this computer.
pub fn snapshot(grab: &Grab) -> Option<String> {
    let frame = grab.frame()?;
    let picture;
    let frame = if grab.scale > 1 {
        picture = unstretch::unscale(&frame)?;
        picture.frame()
    } else {
        frame
    };
    let (width, top) = (frame.width(), frame.first_row());
    let rows = frame.height() - top;
    let mut rgb = Vec::with_capacity((width * rows * 3) as usize);
    for y in top..frame.height() {
        for x in 0..width {
            rgb.extend_from_slice(&frame.rgb(x, y));
        }
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, rows);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&rgb).ok()?;
    }
    Some(format!("data:image/png;base64,{}", base64(&png)))
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fails if the idle wait applies while capturing or paused, or the idle look runs at 4 Hz.
    #[test]
    fn the_engine_slows_down_only_with_no_game_window() {
        assert_eq!(tick_wait(CaptureState::NoGame), Duration::from_secs(2));
        for state in [
            CaptureState::Capturing,
            CaptureState::Paused,
            CaptureState::CaptureBlocked,
        ] {
            assert_eq!(tick_wait(state), Duration::from_millis(250), "{state:?}");
        }
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_snapshot_is_the_grab_at_the_games_own_size() {
        // A 4x2 game area at backing scale 2, every pixel doubled: an 8x4 grab.
        let mut bgra = Vec::new();
        for y in 0..4u8 {
            for x in 0..8u8 {
                bgra.extend_from_slice(&[x / 2, y / 2, 9, 255]);
            }
        }
        let grab = Grab {
            width: 8,
            height: 4,
            first_row: 0,
            bgra,
            scale: 2,
            fresh: true,
        };
        let png = snapshot(&grab).expect("a PNG");
        assert!(png.starts_with("data:image/png;base64,iVBOR"), "{png}");
    }
}
