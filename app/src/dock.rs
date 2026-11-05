//! Keeps MesoVibe with the game window, so the player sees their numbers without switching
//! windows.
//!
//! When the game's display has room beside it, the panel sits flush against the game window (on
//! its right, else its left), matches its height and follows it, and stays above other apps'
//! windows while the game or MesoVibe is in front, as if it were part of the game. When there's
//! no room, the
//! mini-HUD, a small click-through card with the EXP/hr, time to level and level progress, sits
//! in the corner of the game area the player chose, above the game, while the game or MesoVibe
//! is in front. With the mini-HUD switched off there's no window over the game at all.
//!
//! The chat overlay, when the player turns it on in the Chat view, is the same kind of
//! click-through window inside the game's right edge, with the latest whisper, party and friend
//! lines (Chat Hub). It's off until then.
//!
//! MesoVibe only moves its own windows. Where the game window is comes from the window list
//! the capture already reads; the game is never moved, resized or sent anything.
//!
//! This module only places windows. Whether the panel exists, shows or hides is presence.rs's.

use std::sync::Mutex;

use mesovibe_engine::capture::{GameWindow, Placement, ScreenRect};
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Position,
    Size, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::store::Store;

/// The panel window's label (tauri.conf.json).
pub const PANEL: &str = "panel";
/// The mini-HUD window's label (capabilities/hud.json).
pub const HUD: &str = "hud";
/// The mini-HUD window, in logical pixels: the 200px card with room for its shadow.
pub const HUD_WIDTH: f64 = 216.0;
pub const HUD_HEIGHT: f64 = 100.0;
/// The chat overlay window's label (capabilities/chat.json).
pub const CHAT: &str = "chat";
/// The chat overlay window, in logical pixels: a few wrapped lines with room for the shadow.
pub const CHAT_WIDTH: f64 = 360.0;
pub const CHAT_HEIGHT: f64 = 220.0;
/// Where the chat overlay's top sits, as a share of the game area's height down from its top:
/// below the mini-HUD's top corners and above the notices and party list at the bottom right.
const CHAT_FROM_TOP: f64 = 0.3;
/// How far the mini-HUD window sits inside the game area, in game pixels. The top keeps clear of
/// the buttons along the game's top edge, the bottom of its status bar and quick slots.
const INSET_SIDE: f64 = 4.0;
const INSET_TOP: f64 = 24.0;
const INSET_BOTTOM: f64 = 80.0;

/// What the mini-HUD shows of the session: the panel's Session Tracker summary, passed on by the
/// host, since the mini-HUD runs no plugins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub exp_per_hour: Option<f64>,
    pub level: Option<u32>,
    pub pct_hundredths: Option<u32>,
    pub minutes_to_level: Option<f64>,
}

/// The latest [`Summary`], for a mini-HUD that has just opened.
#[derive(Default)]
pub struct Latest(Mutex<Option<Summary>>);

impl Latest {
    pub fn set(&self, summary: Option<Summary>) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = summary;
    }

    pub fn get(&self) -> Option<Summary> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// A corner of the game area, for the mini-HUD.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

/// A rectangle in desktop units (see [`ScreenRect`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn bottom(&self) -> f64 {
        self.y + self.height
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Far enough from `other` to count as moved: half a unit on any edge.
    fn differs(&self, other: &Rect) -> bool {
        (self.x - other.x).abs() > 0.5
            || (self.y - other.y).abs() > 0.5
            || (self.width - other.width).abs() > 0.5
            || (self.height - other.height).abs() > 0.5
    }
}

impl From<ScreenRect> for Rect {
    fn from(r: ScreenRect) -> Rect {
        Rect {
            x: f64::from(r.x),
            y: f64::from(r.y),
            width: f64::from(r.width),
            height: f64::from(r.height),
        }
    }
}

/// A display, as far as docking cares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Display {
    /// The part windows may use (no menu bar, Dock or taskbar), in desktop units.
    pub work_area: Rect,
    /// Desktop units per logical pixel: 1 on a Mac (both are points), the display's scale on
    /// Windows (desktop units there are physical pixels).
    pub scale: f64,
}

/// The player's display choices (Settings > Display).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    /// Keep MesoVibe with the game: the panel docks beside it, or the mini-HUD stands in.
    pub dock: bool,
    /// The mini-HUD may sit over the game when the panel has no room.
    pub mini_hud: bool,
    pub corner: Corner,
}

/// Where MesoVibe's windows go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Layout {
    /// The panel's frame, flush beside the game window.
    Beside(Rect),
    /// No room beside the game: the mini-HUD window's frame, in a corner of the game area.
    MiniHud(Rect),
    /// Neither: the panel stays where the player put it, and there's no mini-HUD.
    Free,
}

/// Where MesoVibe's windows go for the game window at `game`. `panel_width` is the panel's
/// frame width in desktop units: docking keeps the width the player gave it.
pub fn layout(game: &Placement, displays: &[Display], panel_width: f64, choice: &Choice) -> Layout {
    if !choice.dock {
        return Layout::Free;
    }
    let frame = Rect::from(game.frame);
    let (cx, cy) = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
    let Some(display) = displays.iter().find(|d| d.work_area.contains(cx, cy)) else {
        return Layout::Free;
    };
    let work = display.work_area;
    let top = frame.y.max(work.y);
    let height = frame.bottom().min(work.bottom()) - top;
    if height > 0.0 && work.right() - frame.right() >= panel_width {
        return Layout::Beside(Rect {
            x: frame.right(),
            y: top,
            width: panel_width,
            height,
        });
    }
    if height > 0.0 && frame.x - work.x >= panel_width {
        return Layout::Beside(Rect {
            x: frame.x - panel_width,
            y: top,
            width: panel_width,
            height,
        });
    }
    if !choice.mini_hud {
        return Layout::Free;
    }
    let area = Rect::from(game.area);
    let (width, height) = (HUD_WIDTH * display.scale, HUD_HEIGHT * display.scale);
    let left = area.x + INSET_SIDE;
    let right = area.right() - INSET_SIDE - width;
    let upper = area.y + INSET_TOP;
    let lower = area.bottom() - INSET_BOTTOM - height;
    let (x, y) = match choice.corner {
        Corner::TopLeft => (left, upper),
        Corner::TopRight => (right, upper),
        Corner::BottomLeft => (left, lower),
        Corner::BottomRight => (right, lower),
    };
    Layout::MiniHud(Rect {
        x,
        y,
        width,
        height,
    })
}

/// Where the chat overlay goes for the game window at `game`: inside the game area's right edge,
/// below its top. `None` when the game isn't on a known display or is too small to hold it.
pub fn chat_overlay(game: &Placement, displays: &[Display]) -> Option<Rect> {
    let area = Rect::from(game.area);
    let (cx, cy) = (area.x + area.width / 2.0, area.y + area.height / 2.0);
    let display = displays.iter().find(|d| d.work_area.contains(cx, cy))?;
    let (width, height) = (CHAT_WIDTH * display.scale, CHAT_HEIGHT * display.scale);
    let y = area.y + (area.height * CHAT_FROM_TOP).round();
    (area.width >= width + 2.0 * INSET_SIDE && y + height <= area.bottom() - INSET_BOTTOM)
        .then_some(Rect {
            x: area.right() - INSET_SIDE - width,
            y,
            width,
            height,
        })
}

/// Applies [`layout`] to the real windows, from the engine thread, each tick.
#[derive(Default)]
pub struct Docker {
    /// Where the panel was last docked. It's moved again only when that changes, so a panel the
    /// player drags away stays put until the game window moves.
    docked: Option<Rect>,
    /// The panel is kept above other windows (docked, with the game or MesoVibe in front).
    panel_on_top: bool,
    hud_shown: bool,
    chat_shown: bool,
}

impl Docker {
    /// Places MesoVibe's windows for the game window this tick (`None`: no game found).
    pub fn follow(&mut self, app: &AppHandle, window: Option<&GameWindow>) {
        let Some(store) = app.try_state::<Store>() else {
            return;
        };
        let settings = store.settings();
        let choice = Choice {
            dock: settings.dock,
            mini_hud: settings.mini_hud,
            corner: settings.mini_hud_corner,
        };
        if !choice.mini_hud {
            // Switched off: no window over the game at all.
            self.close_hud(app);
        }
        if !settings.chat_overlay {
            self.close_chat(app);
        }
        let Some(panel) = app.get_webview_window(PANEL) else {
            return;
        };
        let Some(placement) = window.and_then(|w| w.placement) else {
            self.free(app, &panel);
            return;
        };
        let Some(panel_width) = frame_width(&panel) else {
            return;
        };
        let in_front = placement.front || panel.is_focused().unwrap_or(false);
        let displays = displays(app);
        match chat_overlay(&placement, &displays).filter(|_| settings.chat_overlay && in_front) {
            Some(target) => self.show_chat(app, &target),
            None => self.hide_chat(app),
        }
        match layout(&placement, &displays, panel_width, &choice) {
            Layout::Beside(target) => {
                self.hide_hud(app);
                let minimized = panel.is_minimized().unwrap_or(false);
                if !minimized && self.docked.is_none_or(|d| d.differs(&target)) {
                    place_panel(&panel, &target);
                    self.docked = Some(target);
                }
                // Docked, it shows whenever the game does, even under another app's window.
                self.keep_on_top(&panel, in_front && !minimized);
            }
            Layout::MiniHud(target) => {
                self.docked = None;
                self.keep_on_top(&panel, false);
                if in_front {
                    self.show_hud(app, &target);
                } else {
                    self.hide_hud(app);
                }
            }
            Layout::Free => self.free(app, &panel),
        }
    }

    /// Reading is off (Safe mode): nothing over the game, and the panel is a plain window.
    pub fn rest(&mut self, app: &AppHandle) {
        if let Some(panel) = app.get_webview_window(PANEL) {
            self.free(app, &panel);
        }
        self.close_hud(app);
        self.close_chat(app);
    }

    /// No game to stay with: the panel is a plain window, and the mini-HUD and chat overlay are
    /// hidden.
    fn free(&mut self, app: &AppHandle, panel: &WebviewWindow) {
        self.docked = None;
        self.keep_on_top(panel, false);
        self.hide_hud(app);
        self.hide_chat(app);
    }

    /// Changing the window level never activates MesoVibe, so the game keeps the keyboard.
    fn keep_on_top(&mut self, panel: &WebviewWindow, on: bool) {
        if self.panel_on_top != on {
            let _ = panel.set_always_on_top(on);
            self.panel_on_top = on;
        }
    }

    fn show_hud(&mut self, app: &AppHandle, target: &Rect) {
        let Some(hud) = app.get_webview_window(HUD).or_else(|| create_hud(app).ok()) else {
            return;
        };
        let _ = hud.set_position(position(target.x, target.y));
        if !self.hud_shown {
            // Not focusable, so showing it never takes the keyboard from the game.
            let _ = hud.show();
            self.hud_shown = true;
        }
    }

    fn hide_hud(&mut self, app: &AppHandle) {
        if self.hud_shown {
            if let Some(hud) = app.get_webview_window(HUD) {
                let _ = hud.hide();
            }
            self.hud_shown = false;
        }
    }

    fn close_hud(&mut self, app: &AppHandle) {
        if let Some(hud) = app.get_webview_window(HUD) {
            let _ = hud.destroy();
        }
        self.hud_shown = false;
    }

    fn show_chat(&mut self, app: &AppHandle, target: &Rect) {
        let Some(chat) = app
            .get_webview_window(CHAT)
            .or_else(|| create_overlay(app, CHAT).ok())
        else {
            return;
        };
        let _ = chat.set_position(position(target.x, target.y));
        if !self.chat_shown {
            let _ = chat.show();
            self.chat_shown = true;
        }
    }

    fn hide_chat(&mut self, app: &AppHandle) {
        if self.chat_shown {
            if let Some(chat) = app.get_webview_window(CHAT) {
                let _ = chat.hide();
            }
            self.chat_shown = false;
        }
    }

    fn close_chat(&mut self, app: &AppHandle) {
        if let Some(chat) = app.get_webview_window(CHAT) {
            let _ = chat.destroy();
        }
        self.chat_shown = false;
    }
}

fn create_hud(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    create_overlay(app, HUD)
}

/// The mini-HUD or the chat overlay window: borderless, see-through outside its card, above
/// other windows, out of the taskbar and the Dock's window list, never focused, and
/// click-through, so every click lands in the game below it.
fn create_overlay(app: &AppHandle, label: &str) -> tauri::Result<WebviewWindow> {
    let (page, title, width, height) = match label {
        CHAT => ("chat.html", "MesoVibe chat", CHAT_WIDTH, CHAT_HEIGHT),
        _ => ("hud.html", "MesoVibe mini-HUD", HUD_WIDTH, HUD_HEIGHT),
    };
    let hud = WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .title(title)
        .inner_size(width, height)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    hud.set_ignore_cursor_events(true)?;
    Ok(hud)
}

/// Moves and sizes the panel so its visible frame is `target`, keeping its width.
pub fn place_panel(panel: &WebviewWindow, target: &Rect) {
    let (Ok(outer), Ok(inner), Ok(scale)) =
        (panel.outer_size(), panel.inner_size(), panel.scale_factor())
    else {
        return;
    };
    let margins = invisible_margins(panel);
    // What the frame adds around the web view (the title bar, and on Windows its borders).
    let frame_height = desktop(f64::from(outer.height - inner.height), scale);
    let inner_height = target.height + margins.top + margins.bottom - frame_height;
    let inner_width = desktop(f64::from(inner.width), scale);
    let _ = panel.set_position(position(target.x - margins.left, target.y - margins.top));
    let _ = panel.set_size(size(inner_width, inner_height.max(1.0)));
}

/// The panel's visible frame width, in desktop units.
fn frame_width(panel: &WebviewWindow) -> Option<f64> {
    let outer = panel.outer_size().ok()?;
    let scale = panel.scale_factor().ok()?;
    let margins = invisible_margins(panel);
    Some(desktop(f64::from(outer.width), scale) - margins.left - margins.right)
}

/// The panel's visible frame, in desktop units.
pub fn panel_frame(panel: &WebviewWindow) -> Option<Rect> {
    let outer = panel.outer_position().ok()?;
    let size = panel.outer_size().ok()?;
    let scale = panel.scale_factor().ok()?;
    let margins = invisible_margins(panel);
    Some(Rect {
        x: desktop(f64::from(outer.x), scale) + margins.left,
        y: desktop(f64::from(outer.y), scale) + margins.top,
        width: desktop(f64::from(size.width), scale) - margins.left - margins.right,
        height: desktop(f64::from(size.height), scale) - margins.top - margins.bottom,
    })
}

/// The display a frame's center is on, if any.
pub fn display_of<'a>(frame: &Rect, displays: &'a [Display]) -> Option<&'a Display> {
    let (cx, cy) = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
    displays.iter().find(|d| d.work_area.contains(cx, cy))
}

pub fn displays(app: &AppHandle) -> Vec<Display> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let scale = m.scale_factor();
            let area = m.work_area();
            Display {
                work_area: Rect {
                    x: desktop(f64::from(area.position.x), scale),
                    y: desktop(f64::from(area.position.y), scale),
                    width: desktop(f64::from(area.size.width), scale),
                    height: desktop(f64::from(area.size.height), scale),
                },
                scale: if cfg!(target_os = "macos") {
                    1.0
                } else {
                    scale
                },
            }
        })
        .collect()
}

/// A length Tauri gives in physical pixels, in desktop units: points on a Mac, the same physical
/// pixels on Windows.
fn desktop(physical: f64, scale: f64) -> f64 {
    if cfg!(target_os = "macos") {
        physical / scale
    } else {
        physical
    }
}

fn position(x: f64, y: f64) -> Position {
    if cfg!(target_os = "macos") {
        LogicalPosition::new(x, y).into()
    } else {
        PhysicalPosition::new(x.round() as i32, y.round() as i32).into()
    }
}

fn size(width: f64, height: f64) -> Size {
    if cfg!(target_os = "macos") {
        LogicalSize::new(width, height).into()
    } else {
        PhysicalSize::new(width.round() as u32, height.round() as u32).into()
    }
}

/// The parts of a window's frame that take no space on screen, in desktop units.
#[derive(Debug, Default, Clone, Copy)]
struct Margins {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// Windows 10 and 11 give a window invisible resize borders outside what's drawn, so a panel
/// placed by its outer frame would leave a gap. macOS draws the whole frame.
#[cfg(windows)]
fn invisible_margins(window: &WebviewWindow) -> Margins {
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let Ok(handle) = window.hwnd() else {
        return Margins::default();
    };
    let hwnd: HWND = handle.0;
    let zero = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let (mut drawn, mut outer) = (zero, zero);
    // SAFETY: RECT out-values of the sizes these read-only queries of our own window expect.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut drawn as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        ) == 0
            && GetWindowRect(hwnd, &mut outer) != 0
    };
    if !ok {
        return Margins::default();
    }
    Margins {
        left: f64::from(drawn.left - outer.left),
        top: f64::from(drawn.top - outer.top),
        right: f64::from(outer.right - drawn.right),
        bottom: f64::from(outer.bottom - drawn.bottom),
    }
}

#[cfg(not(windows))]
fn invisible_margins(_window: &WebviewWindow) -> Margins {
    Margins::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1512×982-point Mac display with a 33-point menu bar, like a 14-inch MacBook Pro's.
    const MAC: Display = Display {
        work_area: Rect {
            x: 0.0,
            y: 33.0,
            width: 1512.0,
            height: 949.0,
        },
        scale: 1.0,
    };

    fn game(x: i32, y: i32, width: u32, height: u32) -> Placement {
        Placement {
            frame: ScreenRect {
                x,
                y,
                width,
                height: height + 28,
            },
            area: ScreenRect {
                x,
                y: y + 28,
                width,
                height,
            },
            front: true,
        }
    }

    const ON: Choice = Choice {
        dock: true,
        mini_hud: true,
        corner: Corner::TopRight,
    };

    // With room beside the game, the panel docks flush, matches its height, and tries the right
    // first. Fails if the panel overlaps the game, leaves a gap, or docks left when the right fits.
    #[test]
    fn with_room_beside_the_game_the_panel_docks_flush_to_it() {
        // An 1024×768 game on the left of the display leaves 488 points on its right.
        let at_left = game(0, 40, 1024, 768);
        assert_eq!(
            layout(&at_left, &[MAC], 360.0, &ON),
            Layout::Beside(Rect {
                x: 1024.0,
                y: 40.0,
                width: 360.0,
                height: 796.0
            })
        );
        // Pushed to the right edge, it docks on the left instead.
        let at_right = game(488, 40, 1024, 768);
        assert_eq!(
            layout(&at_right, &[MAC], 360.0, &ON),
            Layout::Beside(Rect {
                x: 128.0,
                y: 40.0,
                width: 360.0,
                height: 796.0
            })
        );
    }

    // The chat overlay sits inside the game area's right edge, below the mini-HUD's corners and
    // clear of the notices and status bar at the bottom. Fails if it lands outside the game.
    #[test]
    fn the_chat_overlay_sits_inside_the_games_right_edge() {
        let full = game(76, 40, 1361, 768);
        let rect = chat_overlay(&full, &[MAC]).expect("room for the overlay");
        assert_eq!(
            (rect.x, rect.y, rect.width, rect.height),
            (
                76.0 + 1361.0 - 4.0 - CHAT_WIDTH,
                68.0 + 230.0,
                CHAT_WIDTH,
                CHAT_HEIGHT
            )
        );
        // The smallest game size still holds it; a game area narrower than the overlay doesn't.
        assert!(chat_overlay(&game(76, 40, 800, 600), &[MAC]).is_some());
        assert_eq!(chat_overlay(&game(76, 40, 300, 600), &[MAC]), None);
        // Off every display: nowhere to put it.
        assert_eq!(chat_overlay(&game(-5000, 40, 1361, 768), &[MAC]), None);
    }

    // A 1361-point game on a 1512-point display leaves no room: the mini-HUD sits in the chosen
    // corner, clear of the game's top buttons and status bar. Fails if it lands outside the game.
    #[test]
    fn without_room_the_mini_hud_sits_in_the_chosen_corner_of_the_game() {
        let full = game(76, 40, 1361, 768);
        let hud = |corner| {
            let choice = Choice { corner, ..ON };
            match layout(&full, &[MAC], 360.0, &choice) {
                Layout::MiniHud(rect) => rect,
                other => panic!("expected the mini-HUD, got {other:?}"),
            }
        };
        let top_right = hud(Corner::TopRight);
        assert_eq!(
            (top_right.x, top_right.y),
            (76.0 + 1361.0 - 4.0 - HUD_WIDTH, 68.0 + 24.0)
        );
        assert_eq!((top_right.width, top_right.height), (HUD_WIDTH, HUD_HEIGHT));
        let bottom_left = hud(Corner::BottomLeft);
        assert_eq!(
            (bottom_left.x, bottom_left.y),
            (80.0, 68.0 + 768.0 - 80.0 - HUD_HEIGHT)
        );
    }

    // With docking off nothing moves, and with the mini-HUD off there's no window over the game.
    #[test]
    fn switched_off_nothing_docks_and_nothing_sits_over_the_game() {
        let full = game(76, 40, 1361, 768);
        let no_dock = Choice { dock: false, ..ON };
        assert_eq!(layout(&full, &[MAC], 360.0, &no_dock), Layout::Free);
        let no_hud = Choice {
            mini_hud: false,
            ..ON
        };
        assert_eq!(layout(&full, &[MAC], 360.0, &no_hud), Layout::Free);
        let roomy = game(0, 40, 1024, 768);
        assert!(matches!(
            layout(&roomy, &[MAC], 360.0, &no_hud),
            Layout::Beside(_)
        ));
    }

    // With several displays, the game's own display decides, bounds the docked height and sets
    // the HUD's scale. Fails if another display's room is used or the panel runs under the taskbar.
    #[test]
    fn the_display_under_the_game_decides_and_bounds_the_panel() {
        // A 1920×1080 display at 150% beside a full-HD one, with a 48-pixel taskbar.
        let left = Display {
            work_area: Rect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1032.0,
            },
            scale: 1.0,
        };
        let right = Display {
            work_area: Rect {
                x: 1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1032.0,
            },
            scale: 1.5,
        };
        // Taller than the work area: the panel stops above the taskbar.
        let tall = Placement {
            frame: ScreenRect {
                x: 100,
                y: 0,
                width: 1366,
                height: 1060,
            },
            ..game(100, 0, 1366, 1000)
        };
        assert_eq!(
            layout(&tall, &[right, left], 360.0, &ON),
            Layout::Beside(Rect {
                x: 1466.0,
                y: 0.0,
                width: 360.0,
                height: 1032.0
            })
        );
        // Full screen on the right display: no room, and the HUD is sized at 150%.
        let full = game(1920, 0, 1920, 1032);
        match layout(&full, &[left, right], 540.0, &ON) {
            Layout::MiniHud(rect) => {
                assert_eq!(
                    (rect.width, rect.height),
                    (HUD_WIDTH * 1.5, HUD_HEIGHT * 1.5)
                );
                assert!(rect.x >= 1920.0, "on the game's display");
            }
            other => panic!("expected the mini-HUD, got {other:?}"),
        }
        // A game off every display: nothing to dock to.
        let lost = game(-5000, 0, 1366, 768);
        assert_eq!(layout(&lost, &[left, right], 540.0, &ON), Layout::Free);
    }
}
