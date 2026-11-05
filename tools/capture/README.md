# Capture helpers

These tools save screenshots of the MapleStory Classic window, so you can develop and test recognizers against real frames. The engine has its own capture backends in `engine/`; these helpers are small standalone tools for collecting test captures.

**What they do:** they read pixels and nothing else. They never send input or window messages to the game, never read or write game memory or files, and never touch network traffic (see Fair play in the [README](../../README.md)).

**Captures are private.** They go to a `MesoVibe-captures` folder in your home folder, outside the repo. Never commit them: they can include other players' names and chat. `--out <folder>` or the `MESOVIBE_CAPTURES` environment variable picks another folder.

Each run makes a folder like `MesoVibe-captures/2026-11-02/181530-<label>/` with numbered PNGs, `meta.json` (the window and how it was captured) and `frames.jsonl` (timing per frame).

## macOS

`tools/capture/macos/capture` takes screenshots with macOS's own `screencapture` tool, the same thing Cmd-Shift-4 does. It needs Xcode or the Command Line Tools (`xcode-select --install`).

| Command | Does |
|---------|------|
| `tools/capture/macos/capture check` | Checks Screen Recording permission |
| `tools/capture/macos/capture list` | Shows the game's windows; the arrow marks the one it will capture |
| `tools/capture/macos/capture shot <label>` | One screenshot |
| `tools/capture/macos/capture burst <label> [count] [secs]` | `count` shots, one every `secs` (default 30 and 2) |

Options:
- `--window <number>` captures that window when auto-detect picks the wrong one. Find the number with `list --all`.
- `--match <text>` changes the text auto-detect looks for in the app name or bundle ID (default `maple`).
- `--out <folder>` changes where captures go.

`list` shows only the game's own windows, because other windows' titles (chats, mail) are private. Use `list --all` to see everything.

Paste commands one per line, without trailing `# comments`. zsh doesn't treat `#` as a comment at the prompt, so the rest of the line runs as commands.

**Permission:** the first `check` asks macOS for Screen Recording permission for your terminal. Run it from Terminal.app or iTerm2, not an editor's built-in terminal: the permission then goes to that terminal only. Turn the terminal on in System Settings > Privacy & Security > Screen & System Audio Recording, then quit and reopen it.

## Windows

`mesovibe-capture.exe` (source in `tools/capture/windows/`) captures through Windows.Graphics.Capture, the same API the engine uses on Windows. It works even when other windows cover the game. Windows asks no permission for this. Window and process names come from the window manager and the system process list, the same ones Task Manager shows; the tool never opens the game's process.

**Get it:** CI builds it for every pull request and push to `main` that touches `tools/capture/windows/`.
1. Open the run (for a pull request, its **Checks** tab) and pick **Windows capture helper**.
2. Download the `mesovibe-capture-windows` artifact.
3. Unzip it.
4. In PowerShell, run `Unblock-File .\mesovibe-capture.exe` once inside that folder.

To build it yourself instead, install Rust and run `cargo build --release -p mesovibe-capture-windows`.

| Command | Does |
|---------|------|
| `.\mesovibe-capture.exe check` | Windows version, capture support, whether the yellow border can be hidden, display scale |
| `.\mesovibe-capture.exe list` | Shows the game's windows; the arrow marks the one it will capture |
| `.\mesovibe-capture.exe shot <label>` | One screenshot |
| `.\mesovibe-capture.exe burst <label> [count] [secs]` | `count` shots, one every `secs` (default 30 and 2) |

Options:
- `--window`, `--match`, `--out` and `--all` work as on macOS. `--match` looks at the process name.
- `--method screen` copies the screen instead, the way Print Screen does. The game must then be in front and uncovered; the default delay is 3 seconds so you can click into the game.
- `--no-border` asks Windows to hide the yellow capture border (Windows 11 only).
- `--delay <secs>` waits before capturing.
- `--crop-bottom <rows>` saves only the bottom rows of the game area. 40 rows hold the HUD, so an hour at one frame a second is about 200 MB instead of several GB. Use it for long EXP bar runs. Above 768 lines the game stretches its picture, so the tool keeps proportionally more rows (63 at 1920x1200) and the strip still holds the whole HUD.
- `--expect-size <WxH>` refuses to start unless the game area is that size (like `1366x768`), and flags any frame taken after the size changed. Use it for long runs, so a run at the wrong size stops before you play rather than after.

Captures go to `%USERPROFILE%\MesoVibe-captures\`. `meta.json` also records the capture method, what happened to the border request, and whether Windows stretches the game for display scaling.

## Recording with the engine's own capture

The engine can record what it reads, so a long run can use the capture path the app ships (ScreenCaptureKit on macOS, Windows.Graphics.Capture on Windows) instead of these helpers. Record an hour at one frame a second, then replay it through the EXP bar reader:

```
cargo run -p mesovibe-engine --release --example live_exp -- --record ~/MesoVibe-captures/<date>/<time>-exp 3600 1
cargo run -p mesovibe-engine --release --example replay_exp -- ~/MesoVibe-captures/<date>/<time>-exp
```

The folder gets the same format as `--crop-bottom` runs (meta.json, numbered strip PNGs, frames.jsonl), plus each frame's live reading in frames.jsonl. Pick a new folder for each run. On a Retina Mac an hour at 1 Hz is about 0.3 GB. On macOS, run it from Terminal or iTerm2 with Screen Recording permission.

`replay_exp` and `replay_chat` also take folders from the capture helpers.

## Shot list (both systems)

A set of captures that covers what the recognizers read. Play windowed at 1366×768 with the default UI Size, unless a row says otherwise. Before each command, set up the scene described in the table. Write each command after the tool for your system: `tools/capture/macos/capture` on macOS, `.\mesovibe-capture.exe` on Windows. The label is the first word after `shot` or `burst`.

| # | Command | On screen | Shows |
|---|---------|-----------|-------|
| 1 | `shot hud` | Normal play in a field, no windows open, EXP bar visible | The HUD digits and the EXP bar layout |
| 2 | `shot chat` | Chat box showing a few of your own lines plus system lines (EXP gained, item or meso pickups) | The chat font and colors |
| 3 | `shot minimap-town` | A busy town (Henesys, say) with other players nearby, minimap open | Whether the minimap shows other players |
| 4 | `shot minimap-alone` | The minimap on a quiet map | Same, for comparison |
| 5 | `shot inventory` | Inventory open on a tab that shows your mesos | The meso count |
| 6 | `burst grind 60 1` | Killing mobs for a minute | Gains over time (the notice stack), chat scroll speed |
| 7 | `burst hidden 10 1` | Start it, then minimize the game a few seconds in and restore it | What capture does while the window is hidden |
| 8 | `shot party` | Optional: in a party, with the party window or bonus EXP text visible | Party-bonus text |
| 9 | `shot res-800x600`, `shot res-1024x768`, `shot res-1280x720` | Shot 1's scene, windowed, at each resolution in Options > Graphics | Whether the HUD keeps its pixel size across resolutions |
| 10 | `shot fullscreen-1366x768` with a delay (macOS: start it with `sleep 5 &&`; Windows: add `--delay 5`) | Run it, then switch to the game within 5 seconds (Full Screen, 1366×768) | Whether full screen can be captured, and how it is scaled |

**Lie Detector:** don't go looking for it. If one appears, answer it first. A running burst will already have caught its layout.

## Windows-only checks

| # | Command | On screen | Shows |
|---|---------|-----------|-------|
| W1 | `burst border 10 1` | Watch the game window while it runs | Whether Windows draws a yellow border around the game |
| W2 | `burst noborder 10 1 --no-border` | Watch again | Whether the border can be hidden |
| W3 | `shot hud-screen --method screen` | Shot 1's scene; click into the game during the 3-second delay | Whether the screen copy and Windows.Graphics.Capture give the same pixels |
| W4 | `shot meso-before`, then `burst grind2 60 1`, then `shot meso-after` | Inventory open for the two shots and closed for the burst | Whether the meso pickups add up to the inventory change |
