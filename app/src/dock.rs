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
//! click-through window, just above the game's chat box, with the latest whisper, party and
//! friend lines (Chat Hub) at the chat's own size. It's off until then.
//!
//! MesoVibe only moves its own windows. Where the game window is comes from the window list
//! the capture already reads; the game is never moved, resized or sent anything.
//!
//! This module only places windows. Whether the panel exists, shows or hides is presence.rs's.

use std::sync::Mutex;

use mesovibe_engine::capture::{GameWindow, Placement, ScreenRect};
use mesovibe_engine::recognizers::chat::{self, ChatPlace};
use mesovibe_engine::unstretch;
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
/// Chat lines the overlay has room for above the game's chat box.
const CHAT_LINES: u32 = 6;
/// The chat overlay in a box at the game's side: as wide as this, in game pixels (long lines
/// wrap), with room for `SIDE_LINES` lines and `SIDE_PADDING` around them. The box's top sits
/// `SIDE_FROM_TOP` of the game area's height down from its top: below the minimap and the quest
/// helper in the top corners, above the quick slots and notices at the bottom.
pub const SIDE_WIDTH: u32 = 300;
const SIDE_LINES: u32 = 8;
const SIDE_PADDING: u32 = 4;
const SIDE_FROM_TOP: f64 = 0.3;
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

/// Where the chat overlay sits in the game area.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatPosition {
    /// On the top edge of the game's chat box, as split chat.
    AboveChat,
    /// In a box inside the game area's right edge.
    Right,
    /// In a box inside its left edge.
    #[default]
    Left,
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

/// Where the chat overlay goes for the game window at `game` with its chat box at `place`: on the
/// box's top edge, as wide as the box up to its scrollbar, with room for [`CHAT_LINES`] lines at
/// the game's own size. `None` when there's no room above the box for a line.
pub fn chat_overlay(game: &Placement, place: &ChatPlace) -> Option<Rect> {
    if place.width == 0 || place.height == 0 {
        return None;
    }
    // Desktop units per game pixel, each way: the game area on screen is its 1x picture, scaled.
    let area = Rect::from(game.area);
    let (across, down) = (
        area.width / f64::from(place.width),
        area.height / f64::from(place.height),
    );
    let bottom = area.y + f64::from(place.top) * down;
    let top = (bottom - f64::from(CHAT_LINES * chat::LINE_HEIGHT) * down).max(area.y);
    (bottom - top >= f64::from(chat::LINE_HEIGHT) * down).then_some(Rect {
        x: area.x + f64::from(place.left) * across,
        y: top,
        width: f64::from(chat::BOX_WIDTH) * across,
        height: bottom - top,
    })
}

/// Desktop units per game pixel, each way, for a game area `width` by `height` game pixels shown
/// at `area`. Above `source` lines the game stretches a picture that tall to fill the area (768
/// lines, or less for some Full Screen sizes), so a game pixel is one of that picture's. `None` for
/// an empty area.
fn game_pixel(width: u32, height: u32, source: u32, area: &Rect) -> Option<(f64, f64)> {
    let (width, height) = if height > source {
        (unstretch::picture_width(width, height, source), source)
    } else {
        (width, height)
    };
    (width > 0 && height > 0).then(|| {
        (
            area.width / f64::from(width),
            area.height / f64::from(height),
        )
    })
}

/// Where the chat overlay goes in a box at the game's side, for the game window `window` at
/// `game`, scaled up from a picture `source` lines tall: inside the game area's left or right
/// edge, below its top, at the game's own size. `None` when the game area is too small to hold
/// it.
pub fn chat_side(window: &GameWindow, game: &Placement, source: u32, left: bool) -> Option<Rect> {
    let area = Rect::from(game.area);
    let (across, down) = game_pixel(window.width, window.height, source, &area)?;
    let (width, height) = (
        f64::from(SIDE_WIDTH) * across,
        f64::from(SIDE_LINES * chat::LINE_HEIGHT + 2 * SIDE_PADDING) * down,
    );
    let inset = INSET_SIDE * across;
    let y = area.y + (area.height * SIDE_FROM_TOP).round();
    (area.width >= width + 2.0 * inset && y + height <= area.bottom()).then_some(Rect {
        x: if left {
            area.x + inset
        } else {
            area.right() - inset - width
        },
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
    /// Where the chat overlay was last put, so the window moves only when that changes.
    chat_placed: Option<Rect>,
    /// Where the game's chat box was last seen. It stays put while the box can't be read for a
    /// moment (a map change, the player scrolling back).
    chat_box: Option<ChatPlace>,
}

impl Docker {
    /// Places MesoVibe's windows for the game window this tick (`None`: no game found), scaled up
    /// from a picture `source` lines tall, with the game's chat box where the chat reader saw it
    /// this tick, if it did.
    pub fn follow(
        &mut self,
        app: &AppHandle,
        window: Option<&GameWindow>,
        source: u32,
        chat_box: Option<ChatPlace>,
    ) {
        if chat_box.is_some() {
            self.chat_box = chat_box;
        } else if window.is_none() {
            // The game is gone: the next one may be at another size or place.
            self.chat_box = None;
        }
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
        let overlay = match settings.chat_overlay_position {
            ChatPosition::AboveChat => self
                .chat_box
                .and_then(|place| chat_overlay(&placement, &place)),
            side => {
                window.and_then(|w| chat_side(w, &placement, source, side == ChatPosition::Left))
            }
        }
        .filter(|_| settings.chat_overlay && in_front);
        match overlay {
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
        let chat = match app.get_webview_window(CHAT) {
            Some(chat) => chat,
            None => match create_overlay(app, CHAT) {
                // A new window (the last one may have closed with its play session) is hidden and
                // not yet placed.
                Ok(chat) => {
                    self.chat_shown = false;
                    self.chat_placed = None;
                    chat
                }
                Err(_) => return,
            },
        };
        if self.chat_placed.is_none_or(|placed| placed.differs(target)) {
            let _ = chat.set_position(position(target.x, target.y));
            let _ = chat.set_size(size(target.width, target.height));
            self.chat_placed = Some(*target);
        }
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
        self.chat_placed = None;
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
        // Sized to the chat box once placed.
        CHAT => (
            "chat.html",
            "MesoVibe chat",
            f64::from(chat::BOX_WIDTH),
            f64::from(CHAT_LINES * chat::LINE_HEIGHT),
        ),
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
    above_the_game(&hud);
    Ok(hud)
}

/// Lifts an overlay window above the game's own window. While the game is the app in front, Wine
/// raises its window above the floating level that "always on top" gives, to just over the menu
/// bar's (layer 26 in the window list), so the overlays showed only while MesoVibe had focus.
/// The pop-up menu level is above that.
#[cfg(target_os = "macos")]
fn above_the_game(window: &WebviewWindow) {
    let target = window.clone();
    let _ = window.run_on_main_thread(move || {
        let Ok(pointer) = target.ns_window() else {
            return;
        };
        // SAFETY: the NSWindow behind this Tauri window, which lives as long as `target`, used on
        // the main thread as AppKit requires.
        let ns_window = unsafe { &*pointer.cast::<objc2_app_kit::NSWindow>() };
        ns_window.setLevel(objc2_app_kit::NSPopUpMenuWindowLevel);
    });
}

#[cfg(not(target_os = "macos"))]
fn above_the_game(_window: &WebviewWindow) {}

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

    /// The height of the picture the game scales up above it, unless Full Screen says otherwise.
    const NATIVE: u32 = unstretch::NATIVE_HEIGHT;

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

    /// `a` and `b` within a millionth of a unit on every edge.
    fn close(a: Rect, b: Rect) -> bool {
        [
            (a.x, b.x),
            (a.y, b.y),
            (a.width, b.width),
            (a.height, b.height),
        ]
        .iter()
        .all(|(p, q)| (p - q).abs() < 1e-6)
    }

    // The chat overlay sits on the game's chat box, as wide as the box, with the game's own
    // line size. Fails if it covers the box, drifts from it, or ignores how the game is scaled.
    #[test]
    fn the_chat_overlay_sits_on_top_of_the_games_chat_box() {
        // A 1366×768 game at 1x on a Mac, with its chat box's top 207 rows above the bottom.
        let place = ChatPlace {
            width: 1366,
            height: 768,
            left: 283,
            top: 561,
        };
        let mac = game(76, 40, 1366, 768);
        let rect = chat_overlay(&mac, &place).expect("room above the box");
        assert!(close(
            rect,
            Rect {
                x: 76.0 + 283.0,
                y: 68.0 + 561.0 - 96.0,
                width: 551.0,
                height: 96.0
            }
        ));
        // Full screen on a Windows display: 1920×1080 pixels showing the game's 1366×768 picture.
        let screen = ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let full = Placement {
            frame: screen,
            area: screen,
            front: true,
        };
        let (across, down) = (1920.0 / 1366.0, 1080.0 / 768.0);
        assert!(close(
            chat_overlay(&full, &place).expect("room above the box"),
            Rect {
                x: 283.0 * across,
                y: (561.0 - 96.0) * down,
                width: 551.0 * across,
                height: 96.0 * down
            }
        ));
        // Near the game's top edge it takes the room there is, and with no room for a line, none.
        let high = ChatPlace { top: 40, ..place };
        let squeezed = chat_overlay(&mac, &high).expect("room for two lines");
        assert_eq!((squeezed.y, squeezed.height), (68.0, 40.0));
        assert_eq!(chat_overlay(&mac, &ChatPlace { top: 10, ..place }), None);
        // A reading with no game size can't be scaled to the screen.
        assert_eq!(chat_overlay(&mac, &ChatPlace { width: 0, ..place }), None);
        assert_eq!(chat_overlay(&mac, &ChatPlace { height: 0, ..place }), None);
    }

    // The side boxes sit inside the game's left or right edge, below its top, at the game's own
    // size. Fails if a box leaves the game, ignores a stretched game's scale, or crowds a small one.
    #[test]
    fn the_chat_overlay_box_sits_inside_either_side_of_the_game() {
        let window = |width, height| GameWindow {
            id: 7,
            width,
            height,
            scale: 1,
            minimized: false,
            placement: None,
        };
        let height = f64::from(SIDE_LINES * chat::LINE_HEIGHT + 2 * SIDE_PADDING);
        // A 1366×768 game at 1x on a Mac: a game pixel is a point.
        let mac = game(76, 40, 1366, 768);
        let right = chat_side(&window(1366, 768), &mac, NATIVE, false).expect("room on the right");
        assert!(close(
            right,
            Rect {
                x: 76.0 + 1366.0 - 4.0 - 300.0,
                y: 68.0 + 230.0,
                width: 300.0,
                height
            }
        ));
        let left = chat_side(&window(1366, 768), &mac, NATIVE, true).expect("room on the left");
        assert!(close(
            left,
            Rect {
                x: 76.0 + 4.0,
                ..right
            }
        ));
        // Full screen at 1920×1080: the game stretched its 1366×768 picture, so its text is that
        // much larger, and so is the box.
        let screen = ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let full = Placement {
            frame: screen,
            area: screen,
            front: true,
        };
        let (across, down) = (1920.0 / 1366.0, 1080.0 / 768.0);
        assert!(close(
            chat_side(&window(1920, 1080), &full, NATIVE, false).expect("room on the right"),
            Rect {
                x: 1920.0 - (4.0 + 300.0) * across,
                y: 324.0,
                width: 300.0 * across,
                height: height * down
            }
        ));
        // Full Screen at 1280×720 on the same display: a 720-line picture scaled up, so the text
        // and the box are larger again.
        let (across, down) = (1920.0 / 1280.0, 1080.0 / 720.0);
        assert!(close(
            chat_side(&window(1920, 1080), &full, 720, false).expect("room on the right"),
            Rect {
                x: 1920.0 - (4.0 + 300.0) * across,
                y: 324.0,
                width: 300.0 * across,
                height: height * down
            }
        ));
        // A game area narrower than the box has no room for it.
        assert_eq!(
            chat_side(&window(300, 600), &game(76, 40, 300, 600), NATIVE, false),
            None
        );
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
