// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Only the Tauri CLI embeds the UI. A plain `cargo build --release` would make an app that
// loads the dev server and shows nothing. tauri-build sets `dev` unless the CLI built us.
#[cfg(all(dev, not(debug_assertions)))]
compile_error!("Release builds must go through `pnpm tauri build`, which embeds the UI.");

mod chat;
mod dock;
mod engine;
mod game_text;
mod presence;
mod store;
mod updater;

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

use engine::{Command, Engine};
use store::{ChatChannels, ChatColors, Settings, Store};

fn main() {
    // Tauri creates the windows inside its event loop and panics if that fails, so a
    // missing WebView2 runtime arrives as a panic, not as the Err below.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        report_fatal_error("MesoVibe stopped because of an error.", &info.to_string());
        default_hook(info);
    }));
    // Started by the login item: no window until the game appears (presence.rs).
    let background = std::env::args().any(|arg| arg == presence::BACKGROUND_ARG);
    let app = tauri::Builder::default()
        // First, so a second MesoVibe hands over to this one before it starts anything: one
        // engine, one Session Tracker writing the session.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            presence::open_from_event(app);
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name(presence::LOGIN_ITEM)
                .arg(presence::BACKGROUND_ARG)
                .build(),
        )
        // Signed updates from GitHub Releases; updater.rs decides when to look and install.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let store = Store::open(app.path().app_data_dir()?);
            let settings = store.settings();
            // MesoVibe reads the game from its first start; Safe mode in Settings turns that off.
            let reading = !settings.safe_mode;
            // Before the engine starts: it reads the display settings every tick.
            app.manage(game_text::GameText::open(
                app.path().app_data_dir()?.join("game-text.json"),
            ));
            app.manage(store);
            app.manage(dock::Latest::default());
            app.manage(chat::ChatLog::default());
            app.manage(Engine::start(app.handle().clone(), reading));
            presence::start(app.handle(), background);
            app.manage(updater::Updates::start(app.handle()));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == dock::PANEL
            {
                presence::close_requested(window.app_handle(), api);
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            get_settings,
            set_safe_mode,
            retry_capture,
            watch_capture,
            open_privacy_settings,
            plugin_storage_get,
            plugin_storage_set,
            game_text,
            set_display,
            share_summary,
            hud_summary,
            chat_history,
            set_chat_overlay,
            get_startup,
            set_startup,
            update_status,
            check_for_updates,
            install_update,
        ])
        .build(tauri::generate_context!());
    match app {
        Ok(app) => app.run(|app, event| {
            if let RunEvent::ExitRequested { code, api, .. } = &event {
                presence::exit_requested(app, *code, api);
            }
            // The Dock icon, or opening MesoVibe again from Finder while it waits in the tray.
            #[cfg(target_os = "macos")]
            {
                if let RunEvent::Reopen { .. } = event {
                    presence::open_from_event(app);
                }
            }
        }),
        Err(error) => {
            report_fatal_error("MesoVibe could not start.", &error.to_string());
            std::process::exit(1);
        }
    }
}

// MARK: commands the panel may call (allow-listed in capabilities/default.json)

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    os: &'static str,
    event_schema: u32,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        event_schema: mesovibe_engine::events::SCHEMA_VERSION,
    }
}

#[tauri::command]
fn get_settings(store: State<'_, Store>) -> Settings {
    store.settings()
}

/// "Open with MapleStory Classic" (Settings > Reading): the OS login item.
#[tauri::command]
fn get_startup(app: AppHandle) -> presence::Startup {
    presence::startup(&app)
}

#[tauri::command]
fn set_startup(on: bool, app: AppHandle) -> Result<presence::Startup, String> {
    presence::set_startup(&app, on)
}

/// Where updating stands, for the panel's update line and Settings > About.
#[tauri::command]
fn update_status(updates: State<'_, updater::Updates>) -> updater::UpdateStatus {
    updates.status()
}

/// "Check now" in Settings > About.
#[tauri::command]
fn check_for_updates(updates: State<'_, updater::Updates>) {
    updates.check_now();
}

/// "Restart to update": the player's click is the only thing that installs an update. Async, so
/// replacing the app and restarting run off the main thread.
#[tauri::command]
async fn install_update(
    app: AppHandle,
    updates: State<'_, updater::Updates>,
) -> Result<(), String> {
    updates.install(&app)
}

#[tauri::command]
fn set_safe_mode(
    on: bool,
    store: State<'_, Store>,
    engine: State<'_, Engine>,
) -> Result<Settings, String> {
    let settings = store.update(|s| s.safe_mode = on)?;
    engine.send(Command::SetReading(!on));
    Ok(settings)
}

#[tauri::command]
fn retry_capture(engine: State<'_, Engine>) {
    engine.send(Command::Retry);
}

/// The "What MesoVibe sees" view opened or closed: thumbnails only flow while it's open.
#[tauri::command]
fn watch_capture(on: bool, engine: State<'_, Engine>) {
    engine.send(Command::Watch(on));
}

/// Opens the system's Screen Recording settings (macOS), where the player turns MesoVibe on.
#[tauri::command]
fn open_privacy_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("There's no capture permission to set on this system.".to_string())
    }
}

/// Pictures of game names by key, for the plugin views' name cells. A key with no picture kept is
/// left out.
#[tauri::command]
fn game_text(
    keys: Vec<String>,
    names: State<'_, game_text::GameText>,
) -> std::collections::BTreeMap<String, game_text::Picture> {
    names.pictures(&keys)
}

#[tauri::command]
fn plugin_storage_get(plugin: String, store: State<'_, Store>) -> Result<Value, String> {
    store.plugin_get(&plugin)
}

#[tauri::command]
fn plugin_storage_set(plugin: String, value: Value, store: State<'_, Store>) -> Result<(), String> {
    store.plugin_set(&plugin, &value)
}

/// The player's display choices (Settings > Display). Docking follows them from the engine's
/// next tick.
#[tauri::command]
fn set_display(
    dock: bool,
    mini_hud: bool,
    corner: dock::Corner,
    store: State<'_, Store>,
) -> Result<Settings, String> {
    store.update(|s| {
        s.dock = dock;
        s.mini_hud = mini_hud;
        s.mini_hud_corner = corner;
    })
}

/// The panel's latest session summary, passed on to the mini-HUD.
#[tauri::command]
fn share_summary(summary: Option<dock::Summary>, app: AppHandle, latest: State<'_, dock::Latest>) {
    latest.set(summary.clone());
    let _ = app.emit_to(dock::HUD, "hud://summary", summary);
}

/// The latest session summary, for a mini-HUD that has just opened (capabilities/hud.json).
#[tauri::command]
fn hud_summary(latest: State<'_, dock::Latest>) -> Option<dock::Summary> {
    latest.get()
}

/// The Chat Hub's lines so far this run, for a panel or chat overlay that has just opened. They
/// stay in MesoVibe's own windows.
#[tauri::command]
fn chat_history(log: State<'_, chat::ChatLog>) -> Vec<chat::ChatEntry> {
    log.history()
}

/// What the chat overlay shows: the chat types, the color of each, and where it sits.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatOverlayChoice {
    channels: ChatChannels,
    colors: ChatColors,
    position: dock::ChatPosition,
}

/// The chat overlay turned on or off, the chat types it shows, their colors and where it sits.
/// The overlay appears, goes or moves on the engine's next tick (dock.rs).
#[tauri::command]
fn set_chat_overlay(
    on: bool,
    channels: ChatChannels,
    colors: ChatColors,
    position: dock::ChatPosition,
    app: AppHandle,
    store: State<'_, Store>,
) -> Result<Settings, String> {
    if !colors.valid() {
        return Err("A chat color isn't a #rrggbb color.".into());
    }
    let settings = store.update(|s| {
        s.chat_overlay = on;
        s.chat_overlay_channels = channels;
        s.chat_overlay_colors = colors;
        s.chat_overlay_position = position;
    })?;
    let _ = app.emit_to(
        dock::CHAT,
        "chat://overlay",
        ChatOverlayChoice {
            channels: settings.chat_overlay_channels,
            colors: settings.chat_overlay_colors.clone(),
            position: settings.chat_overlay_position,
        },
    );
    Ok(settings)
}

static REPORTED: AtomicBool = AtomicBool::new(false);

/// A release build has no console, so a fatal error (a missing WebView2 runtime, say) would
/// otherwise vanish. Print it, keep a copy in the app's log folder, and show it on Windows.
/// Reports once: an error while reporting must not report again.
fn report_fatal_error(headline: &str, detail: &str) {
    if REPORTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let message = format!(
        "{headline}\n\n{detail}\n\n\
         If this mentions WebView2, install the Microsoft Edge WebView2 Runtime and try again."
    );
    // Not eprintln!: it panics if stderr is closed, which would panic inside the hook.
    let _ = writeln!(std::io::stderr(), "{message}");
    if let Some(dir) = log_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("last-error.log"), &message);
    }
    show_error_box(&message);
}

fn log_dir() -> Option<std::path::PathBuf> {
    use std::{env, path::PathBuf};
    if cfg!(windows) {
        env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("MesoVibe").join("logs"))
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME").map(|dir| PathBuf::from(dir).join("Library/Logs/MesoVibe"))
    } else {
        env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|dir| PathBuf::from(dir).join(".local/state")))
            .map(|dir| dir.join("mesovibe"))
    }
}

#[cfg(windows)]
fn show_error_box(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |text: &str| {
        text.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (text, title) = (wide(message), wide("MesoVibe"));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; a null owner window is allowed.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

// macOS and Linux keep stderr and the log file; a WebView failure there is rare.
#[cfg(not(windows))]
fn show_error_box(_message: &str) {}

#[cfg(test)]
mod tests {
    /// The names inside `generate_handler![...]`.
    fn handlers(main: &str) -> Vec<String> {
        let start =
            main.find("generate_handler![").expect("the handler list") + "generate_handler![".len();
        let end = start + main[start..].find(']').expect("its end");
        main[start..end]
            .split(',')
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect()
    }

    // A command the windows aren't allowed to call is refused at run time, which no browser test
    // sees: the panel's tests run against a stand-in host. Fails if a command registered here is
    // missing from build.rs's list or from every window's capabilities.
    #[test]
    fn every_command_is_allowed_for_some_window() {
        let main = include_str!("main.rs");
        let build = include_str!("../build.rs");
        let capabilities = [
            include_str!("../capabilities/default.json"),
            include_str!("../capabilities/hud.json"),
            include_str!("../capabilities/chat.json"),
        ]
        .concat();
        let commands = handlers(main);
        assert!(commands.len() > 5, "{commands:?}");
        for command in commands {
            assert!(
                build.contains(&format!("\"{command}\"")),
                "{command} isn't in build.rs"
            );
            let allow = format!("\"allow-{}\"", command.replace('_', "-"));
            assert!(
                capabilities.contains(&allow),
                "no window may call {command}"
            );
        }
    }
}
