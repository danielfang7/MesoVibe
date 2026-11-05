//! Capture state machine:
//!
//! ```text
//! NoGame ─found, visible─► Capturing ─minimized/occluded─► Paused ─restored─► Capturing
//!   └────found, hidden───────────────────────────────────► Paused
//! Capturing ─no new frame for 2 s─► Paused ─a new frame─► Capturing
//! NoGame / Capturing / Paused ─capture error─► CaptureBlocked ─user fixes / retries─► NoGame
//! any state ─window closed─► NoGame
//! ```
//!
//! Invariants:
//! - Events are emitted only while `Capturing`.
//! - `Capturing` is entered only with a visible window. A found window reports whether
//!   it's visible, and a retry looks for the window again instead of assuming it is.
//! - A capture error before the window is found (macOS Screen Recording permission
//!   denied, say) still blocks, so the player sees the fix.
//!
//! Platform backends implement one interface, [`Backend`]: `windows` uses
//! Windows.Graphics.Capture, and `macos` uses ScreenCaptureKit. They read back only the rows the
//! readers ask for, and only when the engine asks, at up to 4 Hz.

use crate::frame::Frame;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

/// The game window as a capture backend sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameWindow {
    /// Identifies the window while it exists (its handle on Windows). A different id means the
    /// game was restarted.
    pub id: u64,
    /// The game area's size in the game's own pixels, without the title bar and borders. On a
    /// Mac these are points: the game draws one pixel per point.
    pub width: u32,
    pub height: u32,
    /// Captured pixels per game pixel, each way: 1 on Windows, the display's backing scale on a
    /// Mac (2 on Retina).
    pub scale: u32,
    /// Minimized or off screen, so there's nothing new to capture.
    pub minimized: bool,
    /// Where the window is on the desktop, for MesoVibe to place its own windows beside or
    /// over it. `None` while it's minimized or off screen.
    pub placement: Option<Placement>,
}

/// Where the game window sits on the desktop, as the window manager reports it. MesoVibe only
/// moves its own windows to match: never the game's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The whole window, title bar and borders included.
    pub frame: ScreenRect,
    /// The game area inside it.
    pub area: ScreenRect,
    /// The game is the app in front, the one taking the keyboard.
    pub front: bool,
}

/// A rectangle on the desktop in the system's own units, y growing downward: points from the
/// main display's top-left corner on a Mac, physical pixels of the virtual screen on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The bottom rows of the game area, read back from the newest captured frame, in captured
/// pixels exactly as the backend got them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grab {
    /// The game area's size in captured pixels: its size in game pixels times `scale`.
    pub width: u32,
    pub height: u32,
    /// The first pixel row `bgra` holds.
    pub first_row: u32,
    /// Rows `first_row..height` as BGRA, `width * 4` bytes each.
    pub bgra: Vec<u8>,
    /// Captured pixels per game pixel, each way (see [`GameWindow::scale`]).
    pub scale: u32,
    /// False when no new frame arrived since the last grab, so this is the previous image. A
    /// new frame can show the same pixels: the game drew, and the rows didn't change.
    pub fresh: bool,
}

impl Grab {
    /// The grab as a frame of the whole game area that holds only its bottom rows, declaring
    /// its scale. `None` when the buffer doesn't match the size.
    pub fn frame(&self) -> Option<Frame<'_>> {
        Frame::bottom(
            self.width,
            self.height,
            self.first_row,
            self.width as usize * 4,
            &self.bgra,
        )?
        .with_scale(self.scale)
    }
}

/// Why a backend couldn't capture, in words the player can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureError(pub String);

/// One platform's capture of the game window: pixels only, never input, memory, files or
/// network traffic (see Fair play in README.md).
pub trait Backend {
    /// The game window, if it's open.
    fn find(&mut self) -> Option<GameWindow>;
    /// The bottom `rows` game rows of `window`'s game area, from its newest frame. At a scale
    /// above 1 that's [`crate::unstretch::scaled_rows`] pixel rows, so every game row is whole
    /// wherever the display's pixel blocks start. Starts capturing on first use, and again when
    /// the window is a different one.
    fn grab(&mut self, window: &GameWindow, rows: u32) -> Result<Grab, CaptureError>;
    /// Stops capturing and frees what capture holds. The next `grab` starts again.
    fn stop(&mut self);
}

/// Where the engine is in the capture lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureState {
    NoGame,
    Capturing,
    Paused,
    CaptureBlocked,
}

/// Things that move the capture state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSignal {
    /// The game window was found. `visible` is false when it's minimized or fully covered.
    WindowFound {
        visible: bool,
    },
    WindowHidden,
    WindowRestored,
    WindowClosed,
    CaptureFailed,
    Retry,
}

impl CaptureState {
    /// The next state for a signal. Signals that don't apply leave the state
    /// unchanged.
    pub fn next(self, signal: CaptureSignal) -> CaptureState {
        use CaptureSignal::*;
        use CaptureState::*;
        match (self, signal) {
            (_, WindowClosed) => NoGame,
            (NoGame, WindowFound { visible: true }) => Capturing,
            (NoGame, WindowFound { visible: false }) => Paused,
            (Capturing, WindowHidden) => Paused,
            (Paused, WindowRestored) => Capturing,
            (NoGame | Capturing | Paused, CaptureFailed) => CaptureBlocked,
            (CaptureBlocked, Retry) => NoGame,
            (state, _) => state,
        }
    }

    /// Whether the event bus may emit in this state. Only `Capturing` emits,
    /// so a paused or blocked engine can never produce game events.
    pub fn emits_events(self) -> bool {
        matches!(self, CaptureState::Capturing)
    }
}

#[cfg(test)]
mod tests {
    use super::CaptureSignal::*;
    use super::CaptureState::*;

    const FOUND: super::CaptureSignal = WindowFound { visible: true };
    const FOUND_HIDDEN: super::CaptureSignal = WindowFound { visible: false };

    #[test]
    fn only_capturing_emits_events() {
        for state in [NoGame, Capturing, Paused, CaptureBlocked] {
            assert_eq!(state.emits_events(), state == Capturing, "{state:?}");
        }
    }

    #[test]
    fn follows_the_documented_lifecycle() {
        let s = NoGame.next(FOUND);
        assert_eq!(s, Capturing);
        let s = s.next(WindowHidden);
        assert_eq!(s, Paused);
        let s = s.next(WindowRestored);
        assert_eq!(s, Capturing);
        let s = s.next(CaptureFailed);
        assert_eq!(s, CaptureBlocked);
        let s = s.next(Retry);
        assert_eq!(s, NoGame, "a retry looks for the window again");
        assert_eq!(s.next(FOUND), Capturing);
    }

    #[test]
    fn closing_the_window_always_returns_to_no_game() {
        for state in [NoGame, Capturing, Paused, CaptureBlocked] {
            assert_eq!(state.next(WindowClosed), NoGame, "{state:?}");
        }
    }

    #[test]
    fn a_paused_engine_cannot_be_unpaused_by_unrelated_signals() {
        for signal in [FOUND, FOUND_HIDDEN, WindowHidden, Retry] {
            assert_eq!(Paused.next(signal), Paused, "{signal:?}");
        }
        assert_eq!(NoGame.next(Retry), NoGame);
    }

    // All 28 state/signal pairs. Fails if a hidden find or a retry enters Capturing, an arm widens
    // (Restored resumes CaptureBlocked) or a failure is ignored.
    #[test]
    fn every_state_and_signal_pair_follows_the_lifecycle() {
        let table = [
            // From NoGame: a found window starts capture only if it's visible.
            (NoGame, FOUND, Capturing),
            (NoGame, FOUND_HIDDEN, Paused),
            (NoGame, WindowHidden, NoGame),
            (NoGame, WindowRestored, NoGame),
            (NoGame, WindowClosed, NoGame),
            (NoGame, CaptureFailed, CaptureBlocked),
            (NoGame, Retry, NoGame),
            // From Capturing.
            (Capturing, FOUND, Capturing),
            (Capturing, FOUND_HIDDEN, Capturing),
            (Capturing, WindowHidden, Paused),
            (Capturing, WindowRestored, Capturing),
            (Capturing, WindowClosed, NoGame),
            (Capturing, CaptureFailed, CaptureBlocked),
            (Capturing, Retry, Capturing),
            // From Paused: a capture error while paused still blocks.
            (Paused, FOUND, Paused),
            (Paused, FOUND_HIDDEN, Paused),
            (Paused, WindowHidden, Paused),
            (Paused, WindowRestored, Capturing),
            (Paused, WindowClosed, NoGame),
            (Paused, CaptureFailed, CaptureBlocked),
            (Paused, Retry, Paused),
            // From CaptureBlocked: only a retry (which looks again) or closing the window leaves it.
            (CaptureBlocked, FOUND, CaptureBlocked),
            (CaptureBlocked, FOUND_HIDDEN, CaptureBlocked),
            (CaptureBlocked, WindowHidden, CaptureBlocked),
            (CaptureBlocked, WindowRestored, CaptureBlocked),
            (CaptureBlocked, WindowClosed, NoGame),
            (CaptureBlocked, CaptureFailed, CaptureBlocked),
            (CaptureBlocked, Retry, NoGame),
        ];
        for (state, signal, expected) in table {
            assert_eq!(state.next(signal), expected, "{state:?} + {signal:?}");
        }
    }
}
