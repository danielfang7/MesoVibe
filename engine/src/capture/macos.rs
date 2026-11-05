//! The macOS capture backend: ScreenCaptureKit on the game window only.
//!
//! On a Mac, Classic is the Windows client running under Wine inside Nexon's launcher, in a
//! normal macOS window.
//!
//! - **Finding the game** reads only the system window list (the same facts the Dock and Mission
//!   Control use) and the owning app's bundle ID. That needs no permission, and never touches the
//!   game's memory or files (see Fair play in README.md).
//! - **Capturing** is a ScreenCaptureKit stream of that one window, for only its bottom rows
//!   (`sourceRect`), at most 5 frames a second (`minimumFrameInterval`), in BGRA, without the
//!   cursor. Each frame's strip, about 0.7 MB on a Retina display, is copied out and kept
//!   until the engine asks. Windows covering the game don't matter: the stream sees the window
//!   itself.
//! - **Pixels as the display shows them:** at backing scale 2 every game pixel is a 2x2 block. The
//!   grab says so ([`Grab::scale`]), and the readers undo it exactly.
//! - **Colors as the game draws them:** the stream asks for sRGB. By default it hands over the
//!   display's color space, Display P3 on a built-in Retina screen, which shifts every color the
//!   readers match (white and black stay put). Frames recorded before 2026-11-04 are in P3.
//! - **Minimized or on another Space:** the window list says it's off screen, so the window
//!   reports `minimized`. While it animates to and from the Dock the list still says it's on
//!   screen, with passing sizes, so a window whose size just changed, or that's smaller than any
//!   game size, also reports `minimized` until its size holds. If frames stop arriving anyway,
//!   grabs say they aren't fresh.
//! - **Screen Recording permission** is asked for once, and its absence is a [`CaptureError`]
//!   that says how to turn it on.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, available, define_class, msg_send};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGGetDisplaysWithPoint,
    CGPreflightScreenCaptureAccess, CGRectMakeWithDictionaryRepresentation,
    CGRequestScreenCaptureAccess, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGColorSpaceSRGB, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowName, kCGWindowNumber,
    kCGWindowOwnerPID,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSError, NSObjectProtocol, NSString};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCFrameStatus, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamDelegate, SCStreamErrorCode, SCStreamFrameInfoStatus,
    SCStreamOutput, SCStreamOutputType, SCWindow,
};

use super::{Backend, CaptureError, GameWindow, Grab, Placement, ScreenRect};
use crate::unstretch::scaled_rows;

/// The game's app: MapleStory Classic World inside Nexon's launcher.
const GAME_BUNDLE_ID: &str = "com.nexon.maplestoryna_classic_exe";
/// The game window's title, for an install under another bundle ID (Steam is unverified).
/// Titles are only visible with Screen Recording permission.
const GAME_TITLE: &str = "MapleStory";
/// The macOS title bar above the game, in points (measured on macOS 26). A window that fills its
/// display (Full Screen) has none.
const TITLE_BAR: f64 = 28.0;
/// The fastest the stream delivers frames: a little faster than the engine reads (up to 4 Hz),
/// so a frame is usually waiting when it asks.
const MIN_FRAME_INTERVAL: (i64, i32) = (1, 5);
/// How long a grab with no new frame waits for one before handing over the last picture.
const FRESH_WAIT: Duration = Duration::from_millis(100);
/// How long a new stream waits for its first frame before calling capture broken.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(2);
/// The smallest game area the game offers (800x600). Anything smaller is the window animating.
const MIN_GAME: (u32, u32) = (800, 600);
/// How long ScreenCaptureKit gets to list windows or start a stream.
const CALL_WAIT: Duration = Duration::from_secs(5);

/// What the player does when Screen Recording is off.
const PERMISSION_FIX: &str = "MesoVibe needs Screen Recording permission to see the game \
    window. Open System Settings > Privacy & Security > Screen & System Audio Recording, turn on \
    MesoVibe (or the terminal you started it from), then quit and reopen it.";

/// ScreenCaptureKit capture of the MapleStory Classic window.
#[derive(Default)]
pub struct MacBackend {
    session: Option<Session>,
    /// Bundle IDs by process ID, so the window list is cheap to read every tick.
    bundles: HashMap<i32, Option<String>>,
    /// The game window found last and its bounds then. It's looked up alone first (a full window
    /// list takes about 4 ms).
    last_found: Option<(u32, CGRect)>,
    /// The last picture, kept across streams: a stream restarted with nothing to show yet hands it
    /// over as not fresh.
    last_grab: Option<Grab>,
    /// Whether the system permission prompt was shown already this run.
    asked_permission: bool,
}

impl MacBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many frames of each kind the current stream has delivered: (complete, idle, other).
    /// For diagnostics: a stream that only delivers idle frames is alive but showing nothing new.
    pub fn frame_counts(&self) -> Option<(u64, u64, u64)> {
        self.session.as_ref().map(|s| {
            let state = s.shared.lock();
            (state.complete, state.idle, state.other)
        })
    }
}

impl Backend for MacBackend {
    fn find(&mut self) -> Option<GameWindow> {
        autoreleasepool(|_| {
            let again = self
                .last_found
                .and_then(|(id, _)| find_game_window(&mut self.bundles, Some(id)));
            let found = again.or_else(|| find_game_window(&mut self.bundles, None));
            let before = self.last_found.take();
            let found = found?;
            self.last_found = Some((found.id, found.bounds));
            let front = frontmost_pid() == Some(found.pid);
            Some(settled(before, &found, found.game_window(front)))
        })
    }

    fn grab(&mut self, window: &GameWindow, rows: u32) -> Result<Grab, CaptureError> {
        autoreleasepool(|_| {
            let key = Key::of(window, rows);
            if self.session.as_ref().is_none_or(|s| s.key != key) {
                self.session = None;
                // Checked only when starting (it takes about 9 ms). A permission turned off
                // later stops the stream, which the next grab reports.
                if !CGPreflightScreenCaptureAccess() {
                    if !self.asked_permission {
                        self.asked_permission = true;
                        // Adds MesoVibe to the Screen Recording list and shows the prompt.
                        CGRequestScreenCaptureAccess();
                    }
                    return Err(CaptureError(PERMISSION_FIX.to_string()));
                }
                self.session = Some(Session::start(key)?);
            }
            match self.session.as_mut().expect("started above").grab()? {
                Some(grab) => {
                    self.last_grab = Some(grab.clone());
                    Ok(grab)
                }
                None => match &self.last_grab {
                    Some(last) => Ok(Grab {
                        fresh: false,
                        ..last.clone()
                    }),
                    None => Err(CaptureError(
                        "the game window sent no picture. Is it minimized, or still starting?"
                            .to_string(),
                    )),
                },
            }
        })
    }

    fn stop(&mut self) {
        self.session = None;
        self.last_grab = None;
    }
}

/// `window` as the engine should see it: minimized while it animates to or from the Dock or is
/// being resized (its bounds changed since `before`, the same window's last bounds), or while it's
/// smaller than any game size. Then there's no picture worth reading yet, and its bounds are only
/// passing through, so there's no place to follow either.
fn settled(before: Option<(u32, CGRect)>, found: &Found, mut window: GameWindow) -> GameWindow {
    let moving =
        before.is_some_and(|(id, bounds)| id == found.id && !same_rect(&bounds, &found.bounds));
    if moving || window.width < MIN_GAME.0 || window.height < MIN_GAME.1 {
        window.minimized = true;
        window.placement = None;
    }
    window
}

// MARK: finding the game window

/// The game window as the window list shows it.
struct Found {
    id: u32,
    /// The process that owns the window.
    pid: i32,
    /// The whole window, title bar included, in points.
    bounds: CGRect,
    on_screen: bool,
}

impl Found {
    /// The window as the engine sees it; `front` when its app is the one in front.
    fn game_window(&self, front: bool) -> GameWindow {
        let (scale, display) = display_at(&self.bounds);
        // A window that fills its display is Full Screen, with no title bar.
        let title = match display {
            Some(d) if same_size(&d, &self.bounds) => 0.0,
            _ => TITLE_BAR,
        };
        let area = screen_rect(&self.bounds, title);
        GameWindow {
            id: u64::from(self.id),
            width: area.width,
            height: area.height,
            scale,
            minimized: !self.on_screen,
            placement: self.on_screen.then(|| Placement {
                frame: screen_rect(&self.bounds, 0.0),
                area,
                front,
            }),
        }
    }
}

/// `bounds` (points, from the main display's top-left) as a desktop rectangle, without the top
/// `top` points.
fn screen_rect(bounds: &CGRect, top: f64) -> ScreenRect {
    ScreenRect {
        x: bounds.origin.x.round() as i32,
        y: (bounds.origin.y + top).round() as i32,
        width: bounds.size.width.round().max(0.0) as u32,
        height: (bounds.size.height - top).round().max(0.0) as u32,
    }
}

/// The process of the app in front, the one taking the keyboard.
fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|app| app.processIdentifier())
}

fn same_rect(a: &CGRect, b: &CGRect) -> bool {
    same_size(a, b)
        && (a.origin.x - b.origin.x).abs() < 0.5
        && (a.origin.y - b.origin.y).abs() < 0.5
}

fn same_size(a: &CGRect, b: &CGRect) -> bool {
    (a.size.width - b.size.width).abs() < 0.5 && (a.size.height - b.size.height).abs() < 0.5
}

/// The backing scale of the display under the window's center, and that display's bounds in
/// points. 1 and no bounds when no display is there.
fn display_at(bounds: &CGRect) -> (u32, Option<CGRect>) {
    let center = CGPoint {
        x: bounds.origin.x + bounds.size.width / 2.0,
        y: bounds.origin.y + bounds.size.height / 2.0,
    };
    let (mut display, mut count) = (0u32, 0u32);
    // SAFETY: room for one display ID and the count, both owned here.
    let err = unsafe { CGGetDisplaysWithPoint(center, 1, &mut display, &mut count) };
    if err.0 != 0 || count == 0 {
        return (1, None);
    }
    let points = CGDisplayBounds(display);
    let mode = CGDisplayCopyDisplayMode(display);
    let scale = match mode.as_deref() {
        Some(mode) => {
            let (pixels, points) = (
                CGDisplayMode::pixel_width(Some(mode)),
                CGDisplayMode::width(Some(mode)),
            );
            if points == 0 {
                1
            } else {
                ((pixels as f64 / points as f64).round() as u32).max(1)
            }
        }
        None => 1,
    };
    (scale, Some(points))
}

/// The game: the window of MapleStory Classic World's app, or one titled exactly "MapleStory".
/// Never MesoVibe itself, nor Nexon's launcher. The largest such window that's on screen wins.
/// With `only`, just that window is looked at, whatever its size: a game window shrinking into
/// the Dock is still the game.
fn find_game_window(
    bundles: &mut HashMap<i32, Option<String>>,
    only: Option<u32>,
) -> Option<Found> {
    let list = match only {
        Some(id) => CGWindowListCopyWindowInfo(CGWindowListOption::OptionIncludingWindow, id),
        None => CGWindowListCopyWindowInfo(
            CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements,
            0,
        ),
    }?;
    // SAFETY: the window list is an array of dictionaries keyed by strings.
    let list: CFRetained<objc2_core_foundation::CFArray<CFDictionary<CFString, CFType>>> =
        unsafe { CFRetained::cast_unchecked(list) };
    let own = std::process::id() as i32;
    let mut best: Option<Found> = None;
    // The processes with windows in this list, so a full look forgets the ones that went.
    let mut seen = HashSet::new();
    for info in list.iter() {
        // SAFETY: the keys are the window list's own constants.
        let (number, pid, name, bounds, on_screen) = unsafe {
            (
                kCGWindowNumber,
                kCGWindowOwnerPID,
                kCGWindowName,
                kCGWindowBounds,
                kCGWindowIsOnscreen,
            )
        };
        let Some(pid) = int(&info, pid).map(|p| p as i32) else {
            continue;
        };
        if pid == own {
            continue;
        }
        seen.insert(pid);
        let bundle = bundles
            .entry(pid)
            .or_insert_with(|| {
                NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                    .and_then(|app| app.bundleIdentifier())
                    .map(|id| id.to_string())
            })
            .clone();
        let title = info
            .get(name)
            .and_then(|v| v.downcast::<CFString>().ok())
            .map(|s| s.to_string());
        let is_game =
            bundle.as_deref() == Some(GAME_BUNDLE_ID) || title.as_deref() == Some(GAME_TITLE);
        let is_launcher = bundle
            .as_deref()
            .is_some_and(|b| b.to_lowercase().contains("launcher"));
        if !is_game || is_launcher {
            continue;
        }
        let Some(bounds) = info.get(bounds).and_then(|v| {
            let dict = v.downcast::<CFDictionary>().ok()?;
            let mut rect = CGRect::default();
            // SAFETY: a rectangle out-value owned here.
            unsafe { CGRectMakeWithDictionaryRepresentation(Some(&dict), &mut rect) }
                .then_some(rect)
        }) else {
            continue;
        };
        // Skip the app's helper windows, which are too small to hold the game.
        if only.is_none() && (bounds.size.width < 200.0 || bounds.size.height < 150.0) {
            continue;
        }
        let Some(id) = int(&info, number).map(|n| n as u32) else {
            continue;
        };
        let on_screen = info
            .get(on_screen)
            .and_then(|v| v.downcast::<CFBoolean>().ok())
            .is_some_and(|b| b.as_bool());
        let found = Found {
            id,
            pid,
            bounds,
            on_screen,
        };
        let rank = |f: &Found| (f.on_screen, f.bounds.size.width * f.bounds.size.height);
        if best.as_ref().is_none_or(|b| rank(&found) > rank(b)) {
            best = Some(found);
        }
    }
    // MesoVibe may run for days in the tray, and macOS reuses process ids: a remembered bundle
    // for a process that's gone could make another app's window count as the game, which the
    // first-run screen promises MesoVibe never records.
    if only.is_none() {
        bundles.retain(|pid, _| seen.contains(pid));
    }
    best
}

fn int(info: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    info.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}

// MARK: capture

/// What a stream was set up for. A different key needs a new stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    window: u64,
    /// The game area in game pixels (points), and the backing scale.
    width: u32,
    height: u32,
    scale: u32,
    /// Pixel rows to read back.
    pixel_rows: u32,
}

impl Key {
    fn of(window: &GameWindow, rows: u32) -> Key {
        let scale = window.scale.max(1);
        Key {
            window: window.id,
            width: window.width,
            height: window.height,
            scale,
            pixel_rows: scaled_rows(rows.min(window.height), scale),
        }
    }

    /// The points of the window's bottom edge the stream captures: whole points that hold the
    /// pixel rows.
    fn points(&self) -> u32 {
        self.pixel_rows.div_ceil(self.scale)
    }
}

/// A frame's strip, copied out of ScreenCaptureKit's buffer.
struct Pixels {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

/// What the stream's callbacks hand over to grabs.
#[derive(Default)]
struct State {
    /// The newest complete frame.
    latest: Option<Pixels>,
    /// Counts every delivered frame that shows the window drawing: complete or idle (nothing
    /// changed).
    alive: u64,
    complete: u64,
    idle: u64,
    other: u64,
    /// Why the stream stopped on its own (the window closed, permission was revoked).
    stopped: Option<String>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// The size frames must have, in pixels.
    expected: (u32, u32),
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn receive(&self, sample: &CMSampleBuffer) {
        let status = frame_status(sample);
        let pixels = match status {
            Some(SCFrameStatus::Complete) => copy_pixels(sample, self.expected),
            _ => None,
        };
        let mut state = self.lock();
        match status {
            Some(SCFrameStatus::Complete) => {
                state.complete += 1;
                if let Some(pixels) = pixels {
                    state.latest = Some(pixels);
                    state.alive += 1;
                }
            }
            Some(SCFrameStatus::Idle) => {
                state.idle += 1;
                state.alive += 1;
            }
            _ => state.other += 1,
        }
        drop(state);
        self.changed.notify_all();
    }

    fn stopped(&self, why: String) {
        self.lock().stopped = Some(why);
        self.changed.notify_all();
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Output` doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "MesoVibeCaptureOutput"]
    #[ivars = Arc<Shared>]
    struct Output;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for Output {}

    // SAFETY: the signature matches SCStreamOutput's.
    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Screen {
                self.ivars().receive(sample);
            }
        }
    }

    // SAFETY: the signature matches SCStreamDelegate's.
    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            self.ivars().stopped(describe(error));
        }
    }
);

impl Output {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(shared);
        // SAFETY: NSObject's init on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// A frame's status, from its first attachment dictionary.
fn frame_status(sample: &CMSampleBuffer) -> Option<SCFrameStatus> {
    // SAFETY: reads the sample's attachments without creating any.
    let attachments = unsafe { sample.sample_attachments_array(false) }?;
    // SAFETY: ScreenCaptureKit's attachments are an array of dictionaries keyed by strings.
    let attachments: CFRetained<objc2_core_foundation::CFArray<CFDictionary<CFString, CFType>>> =
        unsafe { CFRetained::cast_unchecked(attachments) };
    let info = attachments.get(0)?;
    // SAFETY: a constant NSString, toll-free bridged to CFString.
    let key: &CFString = unsafe { &*(SCStreamFrameInfoStatus as *const NSString).cast() };
    let status = info.get(key)?.downcast::<CFNumber>().ok()?.as_isize()?;
    Some(SCFrameStatus(status))
}

/// Copies a complete frame's BGRA rows. `None` when the frame isn't BGRA or isn't the size the
/// stream was set up for (a window caught mid-resize).
fn copy_pixels(sample: &CMSampleBuffer, (width, height): (u32, u32)) -> Option<Pixels> {
    // SAFETY: a complete frame carries a pixel buffer.
    let image = unsafe { sample.image_buffer() }?;
    let buffer: &CVPixelBuffer = &image;
    if CVPixelBufferGetPixelFormatType(buffer) != kCVPixelFormatType_32BGRA
        || CVPixelBufferGetWidth(buffer) != width as usize
        || CVPixelBufferGetHeight(buffer) != height as usize
    {
        return None;
    }
    let row = width as usize * 4;
    // SAFETY: locks the buffer for reading, copies whole rows inside it, and unlocks it.
    unsafe {
        if CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) != 0 {
            return None;
        }
        let base = CVPixelBufferGetBaseAddress(buffer) as *const u8;
        let stride = CVPixelBufferGetBytesPerRow(buffer);
        let mut bgra = Vec::with_capacity(row * height as usize);
        if !base.is_null() && stride >= row {
            for y in 0..height as usize {
                bgra.extend_from_slice(std::slice::from_raw_parts(base.add(y * stride), row));
            }
        }
        CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
        (bgra.len() == row * height as usize).then_some(Pixels {
            width,
            height,
            bgra,
        })
    }
}

fn describe(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

/// Something ScreenCaptureKit hands to a completion handler, sent back to the waiting thread.
struct Handed<T>(T);
// SAFETY: ScreenCaptureKit's shareable content is immutable once delivered, and it is only read
// by the thread that waited for it.
unsafe impl<T> Send for Handed<T> {}

/// The windows ScreenCaptureKit can capture, including off-screen ones.
fn shareable_content() -> Result<Retained<SCShareableContent>, CaptureError> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: the handler's arguments are valid for the call, or null.
            let content = unsafe { Retained::retain(content) };
            let error = unsafe { error.as_ref() }.map(|e| (e.code(), describe(e)));
            let _ = tx.send(Handed((content, error)));
        },
    );
    // SAFETY: the handler matches the completion handler's signature.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, false, &handler,
        );
    }
    let Handed((content, error)) = rx.recv_timeout(CALL_WAIT).map_err(|_| {
        CaptureError("macOS didn't answer when asked for the game window.".to_string())
    })?;
    match (content, error) {
        (Some(content), None) => Ok(content),
        (_, Some((code, _))) if code == SCStreamErrorCode::UserDeclined.0 => {
            Err(CaptureError(PERMISSION_FIX.to_string()))
        }
        (_, Some((_, why))) => Err(CaptureError(format!(
            "macOS couldn't list the windows it can capture: {why}"
        ))),
        (None, None) => Err(CaptureError(
            "macOS listed no windows it can capture.".to_string(),
        )),
    }
}

struct Session {
    key: Key,
    shared: Arc<Shared>,
    stream: Retained<SCStream>,
    // Kept alive while the stream uses them.
    _output: Retained<Output>,
    _queue: DispatchRetained<DispatchQueue>,
    /// `State::alive` when the last grab was made, to tell new frames from old.
    seen: u64,
    /// The last grab, for ticks with no new frame.
    last: Option<Grab>,
    /// Whether a grab was made: only the first waits long for a frame.
    grabbed: bool,
}

impl Session {
    fn start(key: Key) -> Result<Session, CaptureError> {
        let content = shareable_content()?;
        // SAFETY: reading the windows of delivered shareable content.
        let window: Retained<SCWindow> = unsafe { content.windows() }
            .iter()
            .find(|w| u64::from(unsafe { w.windowID() }) == key.window)
            .ok_or_else(|| {
                CaptureError(
                    "macOS won't capture the game window. Is it closing, or on another user's \
                     screen?"
                        .to_string(),
                )
            })?;
        // SAFETY: a filter for that one window.
        let filter = unsafe {
            SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
        };
        // SAFETY: the window's frame, in points, title bar included.
        let frame = unsafe { window.frame() };
        let (width, rows) = (key.width * key.scale, key.points() * key.scale);
        let shared = Arc::new(Shared {
            expected: (width, rows),
            ..Shared::default()
        });
        // SAFETY: configuration setters on a fresh configuration.
        let config = unsafe {
            let config = SCStreamConfiguration::new();
            // The bottom rows of the window, in its own points; the output in its pixels, so
            // nothing is resampled.
            let points = f64::from(key.points());
            config.setSourceRect(CGRect {
                origin: CGPoint {
                    x: 0.0,
                    y: frame.size.height - points,
                },
                size: CGSize {
                    width: frame.size.width,
                    height: points,
                },
            });
            config.setWidth(width as usize);
            config.setHeight(rows as usize);
            config.setScalesToFit(false);
            config.setPixelFormat(kCVPixelFormatType_32BGRA);
            // The game's own colors. Left alone, the stream hands over the display's color
            // space, and on a Display P3 screen the friend chat's (255, 153, 0) arrives as
            // (241, 158, 56), which no color reader matches.
            config.setColorSpaceName(kCGColorSpaceSRGB);
            config.setShowsCursor(false);
            config.setQueueDepth(3);
            let (value, timescale) = MIN_FRAME_INTERVAL;
            config.setMinimumFrameInterval(CMTime::new(value, timescale));
            if available!(macos = 14.0) {
                config.setCaptureResolution(SCCaptureResolutionType::Best);
                config.setIgnoreShadowsSingleWindow(true);
            }
            config
        };
        let output = Output::new(shared.clone());
        // SAFETY: a stream of the filter with the configuration, reporting to `output`.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*output)),
            )
        };
        let queue = DispatchQueue::new("app.mesovibe.capture", None);
        // SAFETY: `output` and `queue` outlive the stream (both are kept in the session).
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        }
        .map_err(|e| CaptureError(format!("macOS couldn't start capturing: {}", describe(&e))))?;
        let (tx, rx) = mpsc::channel();
        let started = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: the error is valid for the call, or null.
            let error = unsafe { error.as_ref() }.map(|e| (e.code(), describe(e)));
            let _ = tx.send(error);
        });
        // SAFETY: the handler matches the completion handler's signature.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
        match rx.recv_timeout(CALL_WAIT) {
            Ok(None) => {}
            Ok(Some((code, _))) if code == SCStreamErrorCode::UserDeclined.0 => {
                return Err(CaptureError(PERMISSION_FIX.to_string()));
            }
            Ok(Some((_, why))) => {
                return Err(CaptureError(format!(
                    "macOS couldn't start capturing the game window: {why}"
                )));
            }
            Err(_) => {
                return Err(CaptureError(
                    "macOS didn't start capturing the game window in time.".to_string(),
                ));
            }
        }
        Ok(Session {
            key,
            shared,
            stream,
            _output: output,
            _queue: queue,
            seen: 0,
            last: None,
            grabbed: false,
        })
    }

    /// The bottom rows of the newest frame, waiting a little for one when none is new. `None`
    /// when the stream has shown nothing yet.
    fn grab(&mut self) -> Result<Option<Grab>, CaptureError> {
        let wait = match self.grabbed {
            true => FRESH_WAIT,
            false => FIRST_FRAME_WAIT,
        };
        self.grabbed = true;
        let deadline = Instant::now() + wait;
        let mut state = self.shared.lock();
        while state.alive == self.seen && state.stopped.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        if let Some(why) = &state.stopped {
            return Err(CaptureError(format!(
                "macOS stopped capturing the game window: {why}"
            )));
        }
        let fresh = state.alive != self.seen;
        self.seen = state.alive;
        let grab = match &state.latest {
            Some(pixels) => self.strip(pixels, fresh),
            None => None,
        };
        drop(state);
        match grab {
            Some(grab) => {
                self.last = Some(grab.clone());
                Ok(Some(grab))
            }
            None => Ok(self.last.as_ref().map(|last| Grab {
                fresh: false,
                ..last.clone()
            })),
        }
    }

    /// A frame's bottom `pixel_rows` rows as a grab of the game area.
    fn strip(&self, pixels: &Pixels, fresh: bool) -> Option<Grab> {
        let key = self.key;
        let (width, height) = (key.width * key.scale, key.height * key.scale);
        let rows = key.pixel_rows.min(pixels.height);
        if pixels.width != width || rows == 0 {
            return None;
        }
        let skip = (pixels.height - rows) as usize * width as usize * 4;
        Some(Grab {
            width,
            height,
            first_row: height - rows,
            bgra: pixels.bgra[skip..].to_vec(),
            scale: key.scale,
            fresh,
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: detaches the output before it's released, so no frame still in flight reaches it,
        // then stops the stream, which finishes on its own queue.
        unsafe {
            let _ = self.stream.removeStreamOutput_type_error(
                ProtocolObject::from_ref(&*self._output),
                SCStreamOutputType::Screen,
            );
            self.stream.stopCaptureWithCompletionHandler(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_reads_whole_points_that_hold_the_rows_asked_for() {
        let window = GameWindow {
            id: 1,
            width: 1366,
            height: 768,
            scale: 2,
            minimized: false,
            placement: None,
        };
        let key = Key::of(&window, 32);
        assert_eq!(key.pixel_rows, 65, "32 game rows wherever the blocks start");
        assert_eq!(key.points(), 33);
        let key = Key::of(&GameWindow { scale: 1, ..window }, 32);
        assert_eq!((key.pixel_rows, key.points()), (32, 32));
        // Never more rows than the game area has.
        let key = Key::of(
            &GameWindow {
                height: 20,
                ..window
            },
            32,
        );
        assert_eq!(key.pixel_rows, scaled_rows(20, 2));
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize { width, height },
        }
    }

    // The panel docks to the game window and the mini-HUD sits in its game area. Fails if the
    // title bar is left in the game area, or a window off screen still reports a place.
    #[test]
    fn a_window_on_screen_says_where_it_and_its_game_area_are() {
        let found = Found {
            id: 7,
            pid: 70,
            bounds: rect(49.0, 34.0, 1366.0, 796.0),
            on_screen: true,
        };
        let window = found.game_window(true);
        let placement = window.placement.expect("a window on screen has a place");
        assert_eq!(
            placement.frame,
            ScreenRect {
                x: 49,
                y: 34,
                width: 1366,
                height: 796
            }
        );
        assert_eq!(
            placement.area,
            ScreenRect {
                x: 49,
                y: 62,
                width: 1366,
                height: 768
            },
            "the game area starts below the title bar"
        );
        assert!(placement.front);
        assert_eq!((window.width, window.height), (1366, 768));
        let hidden = Found {
            on_screen: false,
            ..found
        };
        assert_eq!(hidden.game_window(false).placement, None);
    }

    // Minimizing shows passing sizes (1366x797, 1361x797, a genie-sized window) while the window
    // list still says on screen. Read as visible, they restarted streams and blocked capture.
    #[test]
    fn a_window_in_motion_or_too_small_counts_as_minimized() {
        let at_rest = rect(49.0, 34.0, 1366.0, 796.0);
        let found = |bounds| Found {
            id: 7,
            pid: 70,
            bounds,
            on_screen: true,
        };
        let window = |w, h| GameWindow {
            id: 7,
            width: w,
            height: h,
            scale: 2,
            minimized: false,
            placement: None,
        };
        let still = settled(Some((7, at_rest)), &found(at_rest), window(1366, 768));
        assert!(!still.minimized, "a window that holds still is read");
        let first = settled(None, &found(at_rest), window(1366, 768));
        assert!(!first.minimized, "the first sight of a window is read");
        let moving = settled(
            Some((7, at_rest)),
            &found(rect(49.0, 34.0, 1366.0, 797.0)),
            window(1366, 769),
        );
        assert!(moving.minimized, "a window changing size is animating");
        let moved = settled(
            Some((7, at_rest)),
            &found(rect(60.0, 34.0, 1366.0, 796.0)),
            window(1366, 768),
        );
        assert!(
            moved.minimized,
            "a window being dragged is read once it stops"
        );
        let genie = settled(
            Some((7, rect(0.0, 482.0, 84.0, 77.0))),
            &found(rect(0.0, 482.0, 84.0, 77.0)),
            window(84, 49),
        );
        assert!(
            genie.minimized,
            "a window smaller than any game size is the Dock animation"
        );
        let other = settled(
            Some((8, at_rest)),
            &found(rect(0.0, 0.0, 1366.0, 796.0)),
            window(1366, 768),
        );
        assert!(!other.minimized, "another window is a first sight");
    }

    // A hidden game window shown again passed through a 21x12 frame, and the panel docked to it
    // became a 360x12 strip. Fails if a window counted as minimized still reports a place.
    #[test]
    fn a_window_in_motion_or_too_small_reports_no_place() {
        let at_rest = rect(11.0, 35.0, 1366.0, 796.0);
        let passing = rect(745.0, 366.0, 21.0, 12.0);
        let found = |bounds| Found {
            id: 7,
            pid: 70,
            bounds,
            on_screen: true,
        };
        let placed = |found: &Found| settled(Some((7, at_rest)), found, found.game_window(false));
        assert!(
            placed(&found(at_rest)).placement.is_some(),
            "a window at rest is followed"
        );
        let tiny = placed(&found(passing));
        assert!(tiny.minimized);
        assert_eq!(tiny.placement, None, "a passing frame is not followed");
        let first_sight = settled(None, &found(passing), found(passing).game_window(false));
        assert_eq!(
            first_sight.placement, None,
            "a window smaller than any game size is not followed"
        );
        let moved = placed(&found(rect(60.0, 35.0, 1366.0, 796.0)));
        assert!(moved.minimized);
        assert_eq!(
            moved.placement, None,
            "a window being dragged is followed once it stops"
        );
        let resized = placed(&found(rect(11.0, 35.0, 1361.0, 797.0)));
        assert!(resized.minimized);
        assert_eq!(
            resized.placement, None,
            "a window changing size is followed once it holds still"
        );
    }

    #[test]
    fn the_title_bar_is_cropped_unless_the_window_fills_its_display() {
        let display = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 1512.0,
                height: 982.0,
            },
        };
        let window = CGRect {
            origin: CGPoint { x: 83.0, y: 35.0 },
            size: CGSize {
                width: 1366.0,
                height: 796.0,
            },
        };
        assert!(same_size(&display, &display));
        assert!(!same_size(&display, &window));
    }
}
