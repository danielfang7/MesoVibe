//! MesoVibe's presence: the tray (menu bar) icon it waits in, and when its panel is built, shown,
//! hidden and destroyed.
//!
//! With "Open with MapleStory Classic" on (the OS login item), MesoVibe starts at login with no
//! window and waits in the tray. When the game window appears, the panel is built beside it,
//! showing but not focused, so the game keeps the keyboard. Closing the panel hides it. Once the
//! game has been closed for [`GRACE`], the hidden panel is destroyed, and with it the session: the
//! Session Tracker saves a session that was still running when it starts again. With the
//! switch off, closing the panel quits, as it always has.
//!
//! ```text
//!   login ──► tray only ──game window appears──► panel built beside it, not focused
//!               ▲                                  │ (no room beside it: built hidden, so the
//!               │                                  │  mini-HUD still gets its numbers)
//!               │                                  ▼ close button: hidden
//!               └── destroyed ◄── game closed for 5 min and the panel hidden (hidden first if
//!                                 it's still up and not in use)
//!   tray "Open MesoVibe", opening MesoVibe again, the Dock icon: the panel shows, focused
//!   restart into an update the player chose (updater.rs): the panel opens, whatever the launch
//! ```
//!
//! While reading is off (Safe mode) the engine looks at nothing, not even the window list, so
//! nothing here acts on its own. On a Mac the Dock icon shows only while the panel does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mesovibe_engine::capture::Placement;
use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, CloseRequestApi, ExitRequestApi, Manager, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_autostart::ManagerExt;

use crate::chat::ChatLog;
use crate::dock::{self, Choice, Layout, Rect};
use crate::store::{PanelFrame, Settings, Store};

/// How long the game may stay closed before the panel goes and the session ends:
/// long enough for a crash or a relog through the launcher, and well past the Session Tracker's
/// 30 s save interval, so ending it loses nothing.
pub const GRACE: Duration = Duration::from_secs(5 * 60);

/// What the login item starts MesoVibe with: no window until the game appears.
pub const BACKGROUND_ARG: &str = "--background";

/// The login item's name: the Run value on Windows, the LaunchAgent on a Mac. The Windows
/// uninstaller removes the same name (windows/hooks.nsh).
pub const LOGIN_ITEM: &str = "MesoVibe";

// MARK: when the panel comes and goes

/// What the engine knows about the game this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Game {
    /// Reading is off (Safe mode): the engine looks at nothing, not even the window list.
    Unknown,
    Absent,
    /// The game window, and whether it has settled on screen (a placement to dock to).
    Present {
        placed: bool,
    },
}

/// The panel window as it is now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Panel {
    pub exists: bool,
    pub visible: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// The game just settled on screen and there's no panel: build one for it.
    Create,
    /// The game has been closed for [`GRACE`] and the panel is up but not in use: hide it.
    Hide,
    /// The panel is hidden and no game needs it: destroy it, which ends the session.
    Destroy,
}

/// Decides each tick when the panel is built, hidden and destroyed. It keeps only what the
/// windows can't tell it: when the game came and went.
#[derive(Debug, Default)]
pub struct Presence {
    /// The game window was there last tick.
    here: bool,
    /// The game's arrival was handled: it settled on screen while here.
    arrived: bool,
    /// When the game window went, if one was here since MesoVibe started.
    left_at: Option<Instant>,
    /// The panel was hidden once already since the game went.
    hid: bool,
}

impl Presence {
    /// One tick. `resident`: MesoVibe stays running with no window (the login item is on).
    pub fn step(&mut self, game: Game, panel: Panel, resident: bool, now: Instant) -> Action {
        match game {
            Game::Unknown => Action::Nothing,
            Game::Present { placed } => {
                self.here = true;
                self.left_at = None;
                self.hid = false;
                if self.arrived || !placed {
                    return Action::Nothing;
                }
                self.arrived = true;
                if panel.exists {
                    Action::Nothing
                } else {
                    Action::Create
                }
            }
            Game::Absent => {
                if self.here {
                    self.here = false;
                    self.arrived = false;
                    self.left_at = Some(now);
                    self.hid = false;
                }
                // Not resident, MesoVibe runs only while its panel is open: it stays as it is.
                if !resident || !panel.exists {
                    return Action::Nothing;
                }
                // With no game since MesoVibe started, there's no session to wait for.
                let waited = self
                    .left_at
                    .is_none_or(|left| now.duration_since(left) >= GRACE);
                if !waited {
                    Action::Nothing
                } else if !panel.visible {
                    Action::Destroy
                } else if self.left_at.is_some() && !self.hid && !panel.focused {
                    self.hid = true;
                    Action::Hide
                } else {
                    // Up and in use, or opened again after the hide: it goes once it's closed.
                    Action::Nothing
                }
            }
        }
    }

    /// Applies [`Presence::step`] to the real windows, from the engine thread, each tick.
    /// `placement`: where the game window is, when it's there and settled.
    pub fn follow(&mut self, app: &AppHandle, game: Game, placement: Option<&Placement>) {
        let window = app.get_webview_window(dock::PANEL);
        let panel = Panel {
            exists: window.is_some(),
            visible: window
                .as_ref()
                .is_some_and(|w| w.is_visible().unwrap_or(false)),
            focused: window
                .as_ref()
                .is_some_and(|w| w.is_focused().unwrap_or(false)),
        };
        match self.step(game, panel, resident(app), Instant::now()) {
            Action::Nothing => {}
            // One attempt per arrival: a panel that can't be built isn't retried every tick.
            Action::Create => {
                if let Some(placement) = placement {
                    let _ = open_for_game(app, placement);
                }
            }
            Action::Hide => hide(app),
            Action::Destroy => end(app),
        }
    }
}

// MARK: the panel window

/// The panel, built for the game that just appeared. Beside the game it's built showing but not
/// focused: a window shown later would take the keyboard from the game on Windows.
/// With no room beside it, it's built hidden, never over the game, so the plugins still
/// run for the mini-HUD, and the tray opens it.
fn open_for_game(app: &AppHandle, game: &Placement) -> tauri::Result<()> {
    let settings = settings(app);
    let displays = dock::displays(app);
    let choice = Choice {
        dock: settings.dock,
        mini_hud: settings.mini_hud,
        corner: settings.mini_hud_corner,
    };
    let game_frame = Rect::from(game.frame);
    let width = settings.panel.map_or_else(
        || {
            let scale = dock::display_of(&game_frame, &displays).map_or(1.0, |d| d.scale);
            panel_config(app).map_or(360.0, |c| c.width) * scale
        },
        |saved| f64::from(saved.width),
    );
    match dock::layout(game, &displays, width, &choice) {
        Layout::Beside(target) => {
            build(app, Some(target), true, false)?;
            set_dock_icon(app, true);
        }
        Layout::MiniHud(_) | Layout::Free => {
            build(app, saved_frame(&settings, &displays), false, false)?;
        }
    }
    Ok(())
}

/// Shows the panel and gives it focus, building it if needed: the player asked for it (the tray,
/// opening MesoVibe again, the Dock icon, a normal launch).
pub fn open(app: &AppHandle) {
    let panel = match app.get_webview_window(dock::PANEL) {
        Some(panel) => panel,
        None => {
            let frame = saved_frame(&settings(app), &dock::displays(app));
            match build(app, frame, true, true) {
                Ok(panel) => panel,
                Err(_) => return,
            }
        }
    };
    set_dock_icon(app, true);
    let _ = panel.unminimize();
    let _ = panel.show();
    let _ = panel.set_focus();
}

/// [`open`], asked for by an event: the tray, opening MesoVibe again, the Dock icon. Those
/// handlers run on the main thread, where building a window deadlocks on Windows (WebView2; see
/// tauri's `WebviewWindowBuilder::new`), and the panel is built here whenever a play session
/// has ended. So it opens from a thread of its own.
pub fn open_from_event(app: &AppHandle) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("mesovibe-open".to_string())
        .spawn(move || open(&app));
}

/// Hides the panel. Its plugins keep running, so the session and the mini-HUD go on.
pub fn hide(app: &AppHandle) {
    if let Some(panel) = app.get_webview_window(dock::PANEL) {
        remember(app, &panel);
        let _ = panel.hide();
    }
    set_dock_icon(app, false);
}

/// The play session is over: the panel and the overlays go, which frees their web views, and the
/// chat lines and the mini-HUD's numbers from that session go with them. The Session Tracker saves
/// the session the next time the panel opens.
fn end(app: &AppHandle) {
    if let Some(panel) = app.get_webview_window(dock::PANEL) {
        remember(app, &panel);
        let _ = panel.destroy();
    }
    for label in [dock::HUD, dock::CHAT] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.destroy();
        }
    }
    if let Some(log) = app.try_state::<ChatLog>() {
        log.clear();
    }
    if let Some(latest) = app.try_state::<dock::Latest>() {
        latest.set(None);
    }
    set_dock_icon(app, false);
}

/// Quits MesoVibe, keeping where the panel was.
pub fn quit(app: &AppHandle) {
    if let Some(panel) = app.get_webview_window(dock::PANEL) {
        remember(app, &panel);
    }
    app.exit(0);
}

/// The panel's close button. Resident, it hides the panel; otherwise it quits, even with a
/// hidden mini-HUD or chat overlay still open, which alone would keep MesoVibe running.
pub fn close_requested(app: &AppHandle, api: &CloseRequestApi) {
    api.prevent_close();
    if resident(app) {
        hide(app);
    } else {
        quit(app);
    }
}

/// Tauri asks to exit when the last window is gone (`code` `None`). A resident MesoVibe keeps
/// running in the tray; quitting from the tray passes a code.
pub fn exit_requested(app: &AppHandle, code: Option<i32>, api: &ExitRequestApi) {
    if code.is_none() && resident(app) {
        api.prevent_exit();
    }
}

/// Builds the panel from its entry in tauri.conf.json, at `frame` (desktop units) when given.
fn build(
    app: &AppHandle,
    frame: Option<Rect>,
    visible: bool,
    focused: bool,
) -> tauri::Result<WebviewWindow> {
    let config = panel_config(app).ok_or(tauri::Error::WindowNotFound)?;
    let mut builder = WebviewWindowBuilder::from_config(app, &config)?
        .visible(visible)
        .focused(focused);
    match frame {
        Some(frame) => {
            // The builder takes logical pixels; place_panel sets the exact frame right after.
            let scale = dock::display_of(&frame, &dock::displays(app)).map_or(1.0, |d| d.scale);
            builder = builder
                .position(frame.x / scale, frame.y / scale)
                .inner_size(frame.width / scale, frame.height / scale);
        }
        None => builder = builder.center(),
    }
    let panel = builder.build()?;
    if let Some(frame) = frame {
        dock::place_panel(&panel, &frame);
    }
    Ok(panel)
}

fn panel_config(app: &AppHandle) -> Option<tauri::utils::config::WindowConfig> {
    app.config()
        .app
        .windows
        .iter()
        .find(|w| w.label == dock::PANEL)
        .cloned()
}

/// Where the panel was last, if that's still on a display.
fn saved_frame(settings: &Settings, displays: &[dock::Display]) -> Option<Rect> {
    let saved = settings.panel?;
    let frame = Rect {
        x: f64::from(saved.x),
        y: f64::from(saved.y),
        width: f64::from(saved.width),
        height: f64::from(saved.height),
    };
    dock::display_of(&frame, displays).map(|_| frame)
}

/// Keeps where the panel is and how wide, for the next time it's built.
fn remember(app: &AppHandle, panel: &WebviewWindow) {
    if panel.is_minimized().unwrap_or(false) {
        return;
    }
    let (Some(frame), Some(store)) = (dock::panel_frame(panel), app.try_state::<Store>()) else {
        return;
    };
    let saved = PanelFrame {
        x: frame.x.round() as i32,
        y: frame.y.round() as i32,
        width: frame.width.round().max(1.0) as u32,
        height: frame.height.round().max(1.0) as u32,
    };
    if store.settings().panel != Some(saved) {
        let _ = store.update(|s| s.panel = Some(saved));
    }
}

fn settings(app: &AppHandle) -> Settings {
    app.try_state::<Store>()
        .map(|store| store.settings())
        .unwrap_or_default()
}

/// On a Mac, the Dock icon (and Cmd-Tab) shows only while the panel does.
fn set_dock_icon(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if visible {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        };
        let _ = app.set_activation_policy(policy);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
}

// MARK: staying resident: the login item and the tray

/// Whether MesoVibe stays running with no window: the login item is on, and the tray icon
/// exists to bring it back. The OS owns the login item; this only caches what it said.
#[derive(Default)]
pub struct Residency {
    login: AtomicBool,
    tray: AtomicBool,
}

pub fn resident(app: &AppHandle) -> bool {
    app.try_state::<Residency>()
        .is_some_and(|r| r.login.load(Ordering::SeqCst) && r.tray.load(Ordering::SeqCst))
}

/// "Open with MapleStory Classic", for the panel.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Startup {
    /// MesoVibe opens at login and waits for the game.
    pub on: bool,
    /// Why it can't be turned on from where MesoVibe is now, in words the player can act on.
    pub problem: Option<String>,
}

/// Sets up the tray and the login item, then opens the panel, unless MesoVibe started at login
/// and there's nothing to show until the game appears.
pub fn start(app: &AppHandle, background: bool) {
    let login = app.autolaunch().is_enabled().unwrap_or(false);
    if login {
        point_login_item_here(app);
    }
    let tray = build_tray(app).is_ok();
    app.manage(Residency {
        login: AtomicBool::new(login),
        tray: AtomicBool::new(tray),
    });
    let after_update = app
        .try_state::<Store>()
        .is_some_and(|store| store.take_open_after_update());
    if opens_at_start(
        background,
        settings(app).disclosure_accepted,
        resident(app),
        after_update,
    ) {
        open(app);
    } else {
        set_dock_icon(app, false);
    }
}

/// Whether the panel opens as MesoVibe starts. Started by the login item (`background`), it
/// waits for the game in the tray, unless the first-run screen hasn't been accepted yet, or the
/// player just clicked "Restart to update" in the panel and expects it back: a restart keeps the
/// login item's `--background` (updater.rs).
fn opens_at_start(background: bool, accepted: bool, resident: bool, after_update: bool) -> bool {
    !(background && accepted && resident) || after_update
}

/// MesoVibe is about to restart into an update the player chose: keep where the panel is, and
/// have the next start open it.
pub fn before_update_restart(app: &AppHandle) {
    if let Some(panel) = app.get_webview_window(dock::PANEL) {
        remember(app, &panel);
    }
    if let Some(store) = app.try_state::<Store>() {
        let _ = store.mark_open_after_update();
    }
}

/// The update didn't install after all, so the next start is an ordinary one.
pub fn cancel_update_restart(app: &AppHandle) {
    if let Some(store) = app.try_state::<Store>() {
        store.take_open_after_update();
    }
}

pub fn startup(app: &AppHandle) -> Startup {
    Startup {
        on: app
            .try_state::<Residency>()
            .is_some_and(|r| r.login.load(Ordering::SeqCst)),
        problem: install_problem(),
    }
}

/// Turns the login item on or off.
pub fn set_startup(app: &AppHandle, on: bool) -> Result<Startup, String> {
    let login = app.autolaunch();
    if on {
        if let Some(problem) = install_problem() {
            return Err(problem);
        }
        login
            .enable()
            .map_err(|e| format!("MesoVibe couldn't add itself to your login items: {e}"))?;
    } else if login.is_enabled().unwrap_or(true) {
        login
            .disable()
            .map_err(|e| format!("MesoVibe couldn't remove itself from your login items: {e}"))?;
    }
    let now = login.is_enabled().unwrap_or(on);
    if let Some(residency) = app.try_state::<Residency>() {
        residency.login.store(now, Ordering::SeqCst);
    }
    Ok(startup(app))
}

/// A copy run from the disk image, or from where macOS moves a downloaded app it hasn't checked
/// yet, isn't there after a restart, so a login item pointing at it would quietly stop working,
/// and an update can't replace it (updater.rs).
pub(crate) fn install_problem() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().ok()?;
        let path = exe.to_string_lossy();
        if path.starts_with("/Volumes/") || path.contains("/AppTranslocation/") {
            return Some(
                "Move MesoVibe to your Applications folder and open it from there first."
                    .to_string(),
            );
        }
    }
    None
}

/// A login item made before MesoVibe moved (a new copy in Applications) points at the old place:
/// point it here. Only on a Mac, where the item names the app's path; and only when it differs, so
/// macOS doesn't announce it again at every start.
fn point_login_item_here(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
            return;
        };
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let plist = std::path::Path::new(&home)
            .join("Library/LaunchAgents")
            .join(format!("{LOGIN_ITEM}.plist"));
        let here =
            std::fs::read_to_string(plist).is_ok_and(|text| text.contains(&*exe.to_string_lossy()));
        if !here && install_problem().is_none() {
            let _ = app.autolaunch().enable();
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// The tray (menu bar) icon: Open MesoVibe and Quit MesoVibe. On Windows a left click opens
/// the panel; on a Mac a click opens the menu, as menu bar icons do.
fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "open", "Open MesoVibe", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit MesoVibe", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open_item, &separator, &quit_item])?;
    let mut tray = TrayIconBuilder::with_id("mesovibe")
        .tooltip("MesoVibe")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => open_from_event(app),
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if cfg!(target_os = "macos") {
                return;
            }
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                open_from_event(tray.app_handle());
            }
        });
    if let Some(icon) = tray_icon(app) {
        tray = tray.icon(icon).icon_as_template(cfg!(target_os = "macos"));
    }
    tray.build(app)?;
    Ok(())
}

/// A Mac's menu bar takes a one-color template image that macOS tints to match the bar; the
/// Windows tray shows the app icon.
fn tray_icon(app: &AppHandle) -> Option<Image<'static>> {
    if cfg!(target_os = "macos") {
        Image::from_bytes(include_bytes!("../icons/tray-template.png")).ok()
    } else {
        app.default_window_icon()
            .map(|icon| icon.clone().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Panel = Panel {
        exists: false,
        visible: false,
        focused: false,
    };
    const UP: Panel = Panel {
        exists: true,
        visible: true,
        focused: false,
    };
    const IN_USE: Panel = Panel {
        exists: true,
        visible: true,
        focused: true,
    };
    const HIDDEN: Panel = Panel {
        exists: true,
        visible: false,
        focused: false,
    };
    const PLACED: Game = Game::Present { placed: true };

    fn after(start: Instant, secs: u64) -> Instant {
        start + Duration::from_secs(secs)
    }

    // Never for an unsettled frame and never over an existing panel. Fails if the arrival edge
    // refires every tick or ignores the placement. Real focus and z-order are checked by hand.
    #[test]
    fn the_panel_is_built_once_when_the_game_settles() {
        let t = Instant::now();
        let mut p = Presence::default();
        assert_eq!(p.step(Game::Absent, NONE, true, t), Action::Nothing);
        // The login window animating in: no placement yet.
        let unsettled = Game::Present { placed: false };
        assert_eq!(p.step(unsettled, NONE, true, t), Action::Nothing);
        assert_eq!(p.step(PLACED, NONE, true, t), Action::Create);
        // Built, or failed to build: either way not again for this arrival.
        assert_eq!(p.step(PLACED, NONE, true, t), Action::Nothing);
        assert_eq!(p.step(PLACED, UP, true, t), Action::Nothing);

        // A panel the player already opened is left alone.
        let mut q = Presence::default();
        assert_eq!(q.step(PLACED, IN_USE, true, t), Action::Nothing);
    }

    // One session across a crash or relog, and its end 5 minutes after the game closes. Fails if
    // the grace is skipped, restarts wrong or hides twice.
    #[test]
    fn the_panel_goes_five_minutes_after_the_game_does() {
        let t = Instant::now();
        let mut p = Presence::default();
        assert_eq!(p.step(PLACED, NONE, true, t), Action::Create);
        // The game crashes; the player is back within 5 minutes: nothing happens.
        assert_eq!(p.step(Game::Absent, UP, true, after(t, 1)), Action::Nothing);
        assert_eq!(
            p.step(Game::Absent, UP, true, after(t, 290)),
            Action::Nothing
        );
        assert_eq!(p.step(PLACED, UP, true, after(t, 295)), Action::Nothing);
        // Closed for good: the grace counts from this second leaving, not the first.
        assert_eq!(
            p.step(Game::Absent, UP, true, after(t, 400)),
            Action::Nothing
        );
        assert_eq!(
            p.step(Game::Absent, UP, true, after(t, 699)),
            Action::Nothing
        );
        assert_eq!(p.step(Game::Absent, UP, true, after(t, 700)), Action::Hide);
        assert_eq!(
            p.step(Game::Absent, HIDDEN, true, after(t, 702)),
            Action::Destroy
        );
        // Gone: the next game builds a new panel.
        assert_eq!(p.step(PLACED, NONE, true, after(t, 900)), Action::Create);
    }

    // The panel never vanishes from under a player using it. Fails if hide ignores focus, or
    // hides again a panel the player reopened.
    #[test]
    fn a_panel_in_use_stays_until_it_is_closed() {
        let t = Instant::now();
        let mut p = Presence::default();
        p.step(PLACED, NONE, true, t);
        p.step(Game::Absent, IN_USE, true, after(t, 10));
        assert_eq!(
            p.step(Game::Absent, IN_USE, true, after(t, 400)),
            Action::Nothing
        );
        // The player looks away: it's hidden, once.
        assert_eq!(p.step(Game::Absent, UP, true, after(t, 401)), Action::Hide);
        // Opened again from the tray and left open: it stays until closed.
        assert_eq!(
            p.step(Game::Absent, UP, true, after(t, 500)),
            Action::Nothing
        );
        assert_eq!(
            p.step(Game::Absent, HIDDEN, true, after(t, 600)),
            Action::Destroy
        );
    }

    // A panel opened from the tray before any game is never hidden on its own. Fails if the hide
    // fires without a game having left, or destroy waits on a session that never was.
    #[test]
    fn with_no_game_yet_a_closed_panel_goes_at_once() {
        let t = Instant::now();
        let mut p = Presence::default();
        assert_eq!(
            p.step(Game::Absent, UP, true, after(t, 600)),
            Action::Nothing
        );
        assert_eq!(
            p.step(Game::Absent, HIDDEN, true, after(t, 601)),
            Action::Destroy
        );
        // A game that showed up but never settled counts as a game: its session gets the grace.
        let mut q = Presence::default();
        q.step(Game::Present { placed: false }, HIDDEN, true, t);
        assert_eq!(
            q.step(Game::Absent, HIDDEN, true, after(t, 1)),
            Action::Nothing
        );
        assert_eq!(
            q.step(Game::Absent, HIDDEN, true, after(t, 301)),
            Action::Destroy
        );
    }

    // With "Open with MapleStory Classic" off, or with reading off in Safe mode, the panel is never
    // hidden or destroyed on its own.
    #[test]
    fn not_resident_or_not_reading_nothing_happens_on_its_own() {
        let t = Instant::now();
        let mut p = Presence::default();
        p.step(PLACED, UP, false, t);
        assert_eq!(
            p.step(Game::Absent, UP, false, after(t, 1)),
            Action::Nothing
        );
        assert_eq!(
            p.step(Game::Absent, UP, false, after(t, 9999)),
            Action::Nothing
        );
        assert_eq!(
            p.step(Game::Absent, HIDDEN, false, after(t, 9999)),
            Action::Nothing
        );

        let mut q = Presence::default();
        assert_eq!(q.step(Game::Unknown, NONE, true, t), Action::Nothing);
        assert_eq!(
            q.step(Game::Unknown, HIDDEN, true, after(t, 9999)),
            Action::Nothing
        );
    }

    // The panel comes back after "Restart to update", even for a MesoVibe the login item started.
    // Fails if --background wins over the marker, or a plain login start opens a window.
    #[test]
    fn the_panel_opens_at_start_unless_waiting_for_the_game() {
        // (background, accepted, resident, after_update) → opens
        let cases = [
            ((false, true, true, false), true, "a normal launch"),
            (
                (true, true, true, false),
                false,
                "the login item: waits in the tray",
            ),
            (
                (true, false, true, false),
                true,
                "first run: the disclosure shows",
            ),
            (
                (true, true, false, false),
                true,
                "no tray to come back from",
            ),
            (
                (true, true, true, true),
                true,
                "back from an update the player chose",
            ),
        ];
        for ((background, accepted, resident, after_update), opens, why) in cases {
            assert_eq!(
                opens_at_start(background, accepted, resident, after_update),
                opens,
                "{why}"
            );
        }
    }
}
