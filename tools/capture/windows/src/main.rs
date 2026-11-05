//! MesoVibe capture helper, Windows: screenshots of the game window for developing and testing
//! recognizers.
//!
//! Saves screenshots of the MapleStory window, by default through Windows.Graphics.Capture, the
//! API the engine uses on Windows, or with `--method screen` by copying the screen. It only reads
//! pixels. It never sends input or window messages to the game, never opens the game's process,
//! never reads or writes game memory or files, and never touches network traffic (see Fair
//! play in README.md). Window and process names come from the window manager and the system process list,
//! as Task Manager shows them.
//!
//! Captures are private: they can include other players' names and chat. They are written outside
//! the repo, to %USERPROFILE%\MesoVibe-captures by default, and must never be committed.

#[cfg(windows)]
mod screen;
#[cfg(windows)]
mod wgc;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    std::process::exit(win::run(std::env::args().skip(1).collect()));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("mesovibe-capture runs on Windows only. On macOS, use tools/capture/macos/capture.");
    std::process::exit(1);
}
