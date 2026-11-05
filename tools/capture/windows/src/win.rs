//! Commands, window discovery and output for the Windows capture helper.

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, GetDpiForWindow,
    MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_HWNDPREV, GetClientRect, GetForegroundWindow, GetWindow, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
};
use windows::core::BOOL;

use crate::{screen, wgc};

/// 2 added `--crop-bottom` and `cropBottom` in meta.json.
const TOOL_VERSION: u32 = 3;

// MARK: window discovery

#[derive(Clone)]
struct GameWindow {
    hwnd: HWND,
    pid: u32,
    process: String,
    title: String,
    /// Outer bounds as drawn, in physical pixels (DWM extended frame bounds).
    frame: RECT,
    /// The game's drawing area, in physical pixels.
    client: RECT,
    minimized: bool,
}

impl GameWindow {
    fn id(&self) -> usize {
        self.hwnd.0 as usize
    }
}

fn width(r: &RECT) -> i32 {
    r.right - r.left
}

fn height(r: &RECT) -> i32 {
    r.bottom - r.top
}

/// Executable names by process ID, from the system process list (no handle to any process).
fn process_names() -> HashMap<u32, String> {
    let mut names = HashMap::new();
    // SAFETY: a process snapshot read with a correctly sized entry, then closed.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return names;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                names.insert(
                    entry.th32ProcessID,
                    String::from_utf16_lossy(&entry.szExeFile[..len]),
                );
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    names
}

unsafe extern "system" fn collect_window(hwnd: HWND, list: LPARAM) -> BOOL {
    // SAFETY: `list` is the &mut Vec<HWND> passed to EnumWindows below, alive for the call.
    unsafe { (*(list.0 as *mut Vec<HWND>)).push(hwnd) };
    BOOL(1)
}

fn top_level_windows() -> Vec<HWND> {
    let mut list: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `list`, which outlives the call.
    let _ = unsafe {
        EnumWindows(
            Some(collect_window),
            LPARAM(&mut list as *mut Vec<HWND> as isize),
        )
    };
    list
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: a u32 out-value, as DWMWA_CLOAKED expects.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            4,
        )
    }
    .is_ok()
        && cloaked != 0
}

fn frame_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: RECT out-values of the sizes these calls expect.
    unsafe {
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
        .is_err()
        {
            let _ = GetWindowRect(hwnd, &mut rect);
        }
    }
    rect
}

fn window_info(hwnd: HWND, names: &HashMap<u32, String>) -> GameWindow {
    let mut pid = 0u32;
    let mut text = [0u16; 512];
    let mut client = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: window-manager queries with out-values owned here. GetWindowTextW reads another
    // process's title from the window manager without sending that window a message.
    let (len, minimized) = unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let len = GetWindowTextW(hwnd, &mut text);
        let _ = GetClientRect(hwnd, &mut client);
        let _ = ClientToScreen(hwnd, &mut origin);
        (len, IsIconic(hwnd).as_bool())
    };
    GameWindow {
        hwnd,
        pid,
        process: names.get(&pid).cloned().unwrap_or_else(|| "?".to_string()),
        title: String::from_utf16_lossy(&text[..len.max(0) as usize]),
        frame: frame_rect(hwnd),
        client: RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + client.right,
            bottom: origin.y + client.bottom,
        },
        minimized,
    }
}

/// Normal app windows big enough to be a game; minimized ones count too.
fn app_windows() -> Vec<GameWindow> {
    let names = process_names();
    top_level_windows()
        .into_iter()
        // SAFETY: a read-only visibility query.
        .filter(|&hwnd| unsafe { IsWindowVisible(hwnd) }.as_bool() && !is_cloaked(hwnd))
        .map(|hwnd| window_info(hwnd, &names))
        .filter(|w| w.minimized || (width(&w.frame) >= 200 && height(&w.frame) >= 150))
        .collect()
}

/// The process name, or the exact window title "MapleStory". Partial titles would mislead: an
/// editor window titled "… MesoVibe" contains "maple".
fn matches(w: &GameWindow, pattern: &str) -> bool {
    w.process.to_lowercase().contains(&pattern.to_lowercase())
        || w.title.eq_ignore_ascii_case("MapleStory")
}

fn is_launcher(w: &GameWindow) -> bool {
    w.process.to_lowercase().contains("launcher") || w.title.to_lowercase().contains("launcher")
}

/// The likely game window: a match that isn't the launcher, on screen, largest first.
fn find_game_window(pattern: &str) -> Option<GameWindow> {
    app_windows()
        .into_iter()
        .filter(|w| matches(w, pattern))
        .min_by_key(|w| {
            (
                is_launcher(w),
                w.minimized,
                -(width(&w.client) as i64 * height(&w.client) as i64),
            )
        })
}

fn describe(w: &GameWindow) -> String {
    let mut parts = vec![
        format!("#{}", w.id()),
        w.process.clone(),
        format!("\"{}\"", w.title),
        format!("{}x{} px", width(&w.client), height(&w.client)),
    ];
    if w.minimized {
        parts.push("(minimized)".to_string());
    }
    parts.join("  ")
}

/// Process names of visible windows above the game that overlap it. A screen copy would
/// include them.
fn covered_by(target: &GameWindow) -> Vec<String> {
    let names = process_names();
    let mut covering = Vec::new();
    let mut hwnd = target.hwnd;
    for _ in 0..4096 {
        // SAFETY: z-order and visibility queries only.
        let Ok(above) = (unsafe { GetWindow(hwnd, GW_HWNDPREV) }) else {
            break;
        };
        hwnd = above;
        // SAFETY: as above.
        let shown = unsafe { IsWindowVisible(above).as_bool() && !IsIconic(above).as_bool() };
        if !shown || is_cloaked(above) {
            continue;
        }
        let r = frame_rect(above);
        let f = &target.frame;
        if r.left < f.right && f.left < r.right && r.top < f.bottom && f.top < r.bottom {
            let mut pid = 0u32;
            // SAFETY: as above.
            unsafe { GetWindowThreadProcessId(above, Some(&mut pid)) };
            let name = names.get(&pid).cloned().unwrap_or_else(|| "?".to_string());
            if !covering.contains(&name) {
                covering.push(name);
            }
        }
    }
    covering
}

fn dpi_of(w: &GameWindow) -> (u32, u32) {
    let (mut x, mut y) = (0u32, 0u32);
    // SAFETY: DPI queries with out-values owned here.
    unsafe {
        let window = GetDpiForWindow(w.hwnd);
        let monitor = MonitorFromWindow(w.hwnd, MONITOR_DEFAULTTONEAREST);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y);
        (window, x)
    }
}

fn rect_json(r: &RECT) -> Value {
    json!({ "x": r.left, "y": r.top, "width": width(r), "height": height(r) })
}

fn window_json(w: &GameWindow) -> Value {
    let (window_dpi, monitor_dpi) = dpi_of(w);
    json!({
        "id": w.id(),
        "process": w.process,
        "pid": w.pid,
        "title": w.title,
        "frame": rect_json(&w.frame),
        "client": rect_json(&w.client),
        "minimized": w.minimized,
        "windowDpi": window_dpi,
        "monitorDpi": monitor_dpi,
        // A DPI-unaware game on a scaled display is stretched by Windows, which smooths it.
        "stretchedByWindows": window_dpi != monitor_dpi,
    })
}

// MARK: options

#[derive(Clone, Copy, PartialEq)]
enum Method {
    Wgc,
    Screen,
}

struct Options {
    positional: Vec<String>,
    window_id: Option<usize>,
    pattern: String,
    out_root: PathBuf,
    all: bool,
    method: Method,
    hide_border: bool,
    delay: Option<f64>,
    crop_bottom: Option<u32>,
    expect_size: Option<(i32, i32)>,
}

fn parse(args: Vec<String>) -> Result<Options, String> {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut o = Options {
        positional: Vec::new(),
        window_id: None,
        pattern: "maple".to_string(),
        out_root: std::env::var_os("MESOVIBE_CAPTURES")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("MesoVibe-captures")),
        all: false,
        method: Method::Wgc,
        hide_border: false,
        delay: None,
        crop_bottom: None,
        expect_size: None,
    };
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{a} needs a value"));
        match a.as_str() {
            "--window" => {
                o.window_id = Some(
                    value()?
                        .trim_start_matches('#')
                        .parse()
                        .map_err(|_| "--window takes a number from `list`")?,
                )
            }
            "--match" => o.pattern = value()?,
            "--out" => o.out_root = PathBuf::from(value()?),
            "--all" => o.all = true,
            "--method" => {
                o.method = match value()?.as_str() {
                    "wgc" => Method::Wgc,
                    "screen" => Method::Screen,
                    other => return Err(format!("--method is wgc or screen, not \"{other}\"")),
                }
            }
            "--no-border" => o.hide_border = true,
            "--delay" => {
                let secs: f64 = value()?.parse().map_err(|_| "--delay takes seconds")?;
                o.delay = Some(secs.clamp(0.0, 60.0));
            }
            "--crop-bottom" => {
                let rows: u32 = value()?
                    .parse()
                    .map_err(|_| "--crop-bottom takes a number of rows")?;
                if rows == 0 {
                    return Err("--crop-bottom takes at least 1 row".to_string());
                }
                o.crop_bottom = Some(rows);
            }
            "--expect-size" => {
                let text = value()?;
                let size = text
                    .split_once(['x', 'X'])
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                    .filter(|&(w, h)| w > 0 && h > 0);
                o.expect_size = Some(size.ok_or("--expect-size takes a size like 1366x768")?);
            }
            _ => o.positional.push(a),
        }
    }
    Ok(o)
}

// MARK: commands

const USAGE: &str = "\
MesoVibe capture helper (Windows). Screenshots of the game window, nothing else.

  mesovibe-capture check                         Windows version, capture support, display scale
  mesovibe-capture list                          show the game's windows and which one is the game
  mesovibe-capture shot  <label>                 one screenshot of the game window
  mesovibe-capture burst <label> [count] [secs]  count screenshots, one every secs (default 30, 2)

Options:
  --method wgc|screen  wgc (default): Windows.Graphics.Capture, works even if the game is covered.
                       screen: copy the screen; the game must be in front and uncovered.
  --no-border          ask Windows to hide the yellow capture border (wgc, Windows 11 only)
  --delay <secs>       wait before capturing (default 0 for wgc, 3 for screen)
  --crop-bottom <rows> save only the bottom rows of the game area (40 holds the HUD), for long runs;
                       above 768 lines the game stretches its picture, so more rows are kept to match
  --expect-size <WxH>  refuse to start unless the game area is this size, like 1366x768, and flag
                       frames taken after it changes
  --window <number>    capture this window from `list --all` instead of auto-detecting
  --match <text>       auto-detect by this text in the process name (default: maple)
  --all                `list` every visible app window, not only the game's
  --out <folder>       where to save (default: %USERPROFILE%\\MesoVibe-captures, or %MESOVIBE_CAPTURES%)

Each run makes a folder like MesoVibe-captures\\2026-11-02\\181530-<label>\\ with numbered PNGs,
meta.json (window, method, border, display scale) and frames.jsonl (timing per frame).";

pub fn run(args: Vec<String>) -> i32 {
    // Physical pixels everywhere, so window rectangles match what's captured.
    // SAFETY: process-wide settings made once, before any window is touched.
    let dpi_aware = unsafe {
        let aware =
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok();
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        aware
    };
    let o = match parse(args) {
        Ok(o) => o,
        Err(message) => return fail(&message),
    };
    match o.positional.first().map(String::as_str) {
        Some("check") => cmd_check(&o),
        Some("list") => cmd_list(&o),
        Some(command @ ("shot" | "burst")) => cmd_capture(&o, command == "burst", dpi_aware),
        None | Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            0
        }
        Some(other) => fail(&format!("Unknown command \"{other}\".\n\n{USAGE}")),
    }
}

fn fail(message: &str) -> i32 {
    eprintln!("{message}");
    1
}

fn windows_version() -> String {
    std::process::Command::new("cmd")
        .args(["/c", "ver"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn cmd_check(o: &Options) -> i32 {
    println!("{}", windows_version());
    println!(
        "Windows.Graphics.Capture supported: {}",
        if wgc::supported() { "yes" } else { "NO" }
    );
    println!(
        "Yellow border can be turned off: {}",
        if wgc::border_setting_present() {
            "yes (Windows 11)"
        } else {
            "no (Windows 10 always draws it)"
        }
    );
    match find_game_window(&o.pattern) {
        Some(w) => {
            let (window_dpi, monitor_dpi) = dpi_of(&w);
            println!("Game window: {}", describe(&w));
            println!(
                "Display scale: {}% (the game draws at {}%{})",
                monitor_dpi * 100 / 96,
                window_dpi * 100 / 96,
                if window_dpi != monitor_dpi {
                    "; Windows stretches it, which smooths the pixels"
                } else {
                    ""
                }
            );
        }
        None => println!("Game window: not found. Start the game, then run `check` again."),
    }
    println!("Nothing else to set up: Windows doesn't ask permission for window capture.");
    0
}

fn cmd_list(o: &Options) -> i32 {
    let windows = app_windows();
    let pick = find_game_window(&o.pattern);
    let shown: Vec<&GameWindow> = windows
        .iter()
        .filter(|w| o.all || matches(w, &o.pattern))
        .collect();
    let scope = if o.all {
        "Visible app windows".to_string()
    } else {
        format!("Windows matching \"{}\"", o.pattern)
    };
    println!("{scope} (→ = what `shot` and `burst` would capture):");
    let mut shown = shown;
    shown.sort_by_key(|w| w.process.to_lowercase());
    for w in shown {
        let marker = if pick.as_ref().is_some_and(|p| p.hwnd == w.hwnd) {
            "→"
        } else {
            " "
        };
        println!(" {marker} {}", describe(w));
    }
    match pick {
        None => println!(
            "\nNo game window found. Start the game, or run `list --all` and pass --window <number>."
        ),
        Some(p) if is_launcher(&p) => {
            println!("\nOnly the launcher is open. Start the game from it, then run `list` again.")
        }
        Some(_) => {}
    }
    0
}

fn local_folder_names() -> (String, String) {
    // SAFETY: returns a SYSTEMTIME by value.
    let t = unsafe { GetLocalTime() };
    (
        format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay),
        format!("{:02}{:02}{:02}", t.wHour, t.wMinute, t.wSecond),
    )
}

fn iso_now() -> String {
    // SAFETY: returns a SYSTEMTIME by value.
    let t = unsafe { GetSystemTime() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

fn save_png(path: &Path, bgra: &[u8], width: u32, height: u32) -> Result<(), String> {
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for px in bgra.as_chunks::<4>().0 {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    let file = File::create(path).map_err(|e| format!("can't create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG header: {e}"))?;
    writer
        .write_image_data(&rgb)
        .map_err(|e| format!("PNG data: {e}"))?;
    writer.finish().map_err(|e| format!("PNG finish: {e}"))
}

/// Rows to keep for `--crop-bottom rows` in a game area `client_height` tall. Up to 768 lines the
/// game draws at its own pixel size; above that it stretches a 768-line picture to fit, so the
/// HUD is taller and the strip grows with it (40 rows at 1366x768 is 63 at 1920x1200).
fn crop_rows(rows: u32, client_height: u32) -> u32 {
    if client_height > 768 {
        (rows * client_height).div_ceil(768)
    } else {
        rows
    }
}

/// The bottom `rows` rows of the game's drawing area, cut from a capture of the whole window.
/// Keeps long runs small: the HUD strip of a 1366x768 game is about 50 KB a frame instead of 2 MB.
fn crop_bottom(
    bgra: &[u8],
    image_width: u32,
    image_height: u32,
    window: &GameWindow,
    rows: u32,
) -> Result<(Vec<u8>, u32, u32), String> {
    let left = window.client.left - window.frame.left;
    let top = window.client.top - window.frame.top;
    let (client_width, client_height) = (width(&window.client), height(&window.client));
    if left < 0
        || top < 0
        || client_width <= 0
        || client_height <= 0
        || (left + client_width) as u32 > image_width
        || (top + client_height) as u32 > image_height
    {
        return Err("the game area isn't inside the captured image".to_string());
    }
    let rows = crop_rows(rows, client_height as u32).min(client_height as u32);
    let first = top as u32 + client_height as u32 - rows;
    let row_bytes = client_width as usize * 4;
    let mut out = Vec::with_capacity(row_bytes * rows as usize);
    for y in first..first + rows {
        let start = (y as usize * image_width as usize + left as usize) * 4;
        out.extend_from_slice(&bgra[start..start + row_bytes]);
    }
    Ok((out, client_width as u32, rows))
}

fn append_line(path: &Path, value: &Value) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{value}");
    }
}

fn cmd_capture(o: &Options, burst: bool, dpi_aware: bool) -> i32 {
    let args = &o.positional[1..];
    let label = args
        .first()
        .cloned()
        .unwrap_or_else(|| if burst { "burst" } else { "shot" }.to_string());
    let (count, interval) = if burst {
        let count = args.get(1).map_or(Some(30), |v| v.parse::<u32>().ok());
        let interval = args.get(2).map_or(Some(2.0), |v| v.parse::<f64>().ok());
        match (count, interval) {
            (Some(c), _) if !(1..=3600).contains(&c) => {
                return fail("frame count must be 1 to 3600");
            }
            (Some(c), Some(s)) if s >= 0.25 => (c, s),
            (None, _) => return fail("frame count must be 1 to 3600"),
            _ => return fail("interval must be at least 0.25 seconds"),
        }
    } else {
        (1, 0.0)
    };

    let target = match o.window_id {
        Some(id) => {
            let names = process_names();
            let hwnd = HWND(id as *mut c_void);
            if !top_level_windows().contains(&hwnd) {
                return fail(&format!(
                    "No window #{id}. Run `list --all` to see window numbers."
                ));
            }
            window_info(hwnd, &names)
        }
        None => match find_game_window(&o.pattern) {
            Some(w) => w,
            None => {
                return fail(&format!(
                    "No game window found (looked for \"{}\"). Run `list --all`, then pass --window <number>.",
                    o.pattern
                ));
            }
        },
    };
    if is_launcher(&target) {
        println!(
            "Warning: this looks like the launcher, not the game: {}",
            describe(&target)
        );
    }
    let game_size = |w: &GameWindow| (width(&w.client), height(&w.client));
    if let Some((want_w, want_h)) = o.expect_size {
        let (w, h) = game_size(&target);
        if (w, h) != (want_w, want_h) {
            return fail(&format!(
                "The game area is {w}x{h}, not the {want_w}x{want_h} this run expects.\n\
                 Change it in the game's settings (Windowed, {want_w}x{want_h}), then run this again."
            ));
        }
    }

    let delay = o
        .delay
        .unwrap_or(if o.method == Method::Screen { 3.0 } else { 0.0 });
    if delay > 0.0 {
        println!("Capturing in {delay}s: bring the game to the front and leave it uncovered.");
        std::thread::sleep(Duration::from_secs_f64(delay));
    }

    let mut session = None;
    let mut border = "not applicable (screen copy)".to_string();
    if o.method == Method::Wgc {
        match wgc::Capture::start(target.hwnd, o.hide_border) {
            Ok(capture) => {
                border = capture.border.clone();
                session = Some(capture);
            }
            Err(e) => {
                return fail(&format!(
                    "Windows.Graphics.Capture couldn't start on this window: {e}\n\
                     Note the error, then try --method screen."
                ));
            }
        }
    }

    let (day, time) = local_folder_names();
    let safe_label: String = label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let dir = o.out_root.join(day).join(format!("{time}-{safe_label}"));
    if let Err(e) = fs::create_dir_all(&dir) {
        return fail(&format!("Could not create {}: {e}", dir.display()));
    }
    let meta = json!({
        "tool": "mesovibe-capture-windows",
        "toolVersion": TOOL_VERSION,
        "label": label,
        "startedAt": iso_now(),
        "windows": windows_version(),
        "method": if o.method == Method::Wgc { "wgc" } else { "screen" },
        "wgcSupported": wgc::supported(),
        "borderSettingPresent": wgc::border_setting_present(),
        "border": border,
        "dpiAware": dpi_aware,
        "window": window_json(&target),
        "plannedFrames": count,
        "intervalSeconds": interval,
        "cropBottom": o.crop_bottom,
    });
    let meta_text = serde_json::to_string_pretty(&meta).unwrap_or_default();
    if let Err(e) = fs::write(dir.join("meta.json"), meta_text) {
        return fail(&format!("Could not write to {}: {e}", dir.display()));
    }

    println!("Capturing {}", describe(&target));
    if o.method == Method::Wgc {
        println!("Yellow border: {border}. Look at the game window now: is there a yellow border?");
    }
    if burst {
        println!(
            "{count} frames, one every {interval}s. Ctrl-C stops early; frames so far are kept."
        );
    }
    let frames_file = dir.join("frames.jsonl");
    let mut ok_count = 0;
    for index in 1..=count {
        let tick = Instant::now();
        let current = window_info(target.hwnd, &process_names());
        // SAFETY: a read-only query.
        let foreground = unsafe { GetForegroundWindow() } == target.hwnd;
        let mut line = json!({
            "index": index,
            "capturedAt": iso_now(),
            "minimized": current.minimized,
            "foreground": foreground,
            "frame": rect_json(&current.frame),
        });
        let grabbed = match session.as_mut() {
            Some(capture) => capture.grab(Duration::from_millis(250)).map(|f| {
                line["fresh"] = json!(f.fresh);
                line["frameAgeMs"] = json!((f.age_ms * 10.0).round() / 10.0);
                (f.bgra, f.width, f.height)
            }),
            None if current.minimized => {
                Err("minimized: a screen copy would show other windows".to_string())
            }
            None => {
                line["coveredBy"] = json!(covered_by(&current));
                screen::grab(current.frame)
            }
        };
        let grabbed = match o.crop_bottom {
            // A minimized window reports no usable rectangles; its stale frame keeps the
            // layout the window had when the run started.
            Some(rows) => grabbed.and_then(|(bgra, w, h)| {
                let layout = if current.minimized { &target } else { &current };
                crop_bottom(&bgra, w, h, layout, rows)
            }),
            None => grabbed,
        };
        let name = format!("{index:04}.png");
        let result = grabbed.and_then(|(bgra, w, h)| {
            let black = bgra
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[0] < 8 && p[1] < 8 && p[2] < 8);
            save_png(&dir.join(&name), &bgra, w, h).map(|()| (w, h, black))
        });
        line["captureMs"] = json!(tick.elapsed().as_millis() as u64);
        let status = match result {
            Ok((w, h, black)) => {
                ok_count += 1;
                line["ok"] = json!(true);
                line["file"] = json!(name);
                line["pixelWidth"] = json!(w);
                line["pixelHeight"] = json!(h);
                line["allBlack"] = json!(black);
                let mut s = format!("{w}x{h} px in {} ms", tick.elapsed().as_millis());
                if black {
                    s.push_str("  ALL BLACK (capture may be blocked)");
                }
                if line["fresh"] == json!(false) {
                    s.push_str("  (no new frame: same image as before)");
                }
                if let Some(want) = o.expect_size
                    && !current.minimized
                    && game_size(&current) != want
                {
                    let (gw, gh) = game_size(&current);
                    line["sizeChanged"] = json!(format!("{gw}x{gh}"));
                    s.push_str(&format!(
                        "  GAME SIZE CHANGED to {gw}x{gh}: set it back to {}x{}",
                        want.0, want.1
                    ));
                }
                if let Some(covering) = line["coveredBy"].as_array().filter(|c| !c.is_empty()) {
                    let names: Vec<&str> = covering.iter().filter_map(Value::as_str).collect();
                    s.push_str(&format!("  covered by: {}", names.join(", ")));
                }
                s
            }
            Err(e) => {
                line["ok"] = json!(false);
                line["error"] = json!(e);
                format!("FAILED: {e}")
            }
        };
        append_line(&frames_file, &line);
        println!("  {index:04}  {status}");
        if index < count {
            let wait = Duration::from_secs_f64(interval).saturating_sub(tick.elapsed());
            std::thread::sleep(wait);
        }
    }
    println!("Saved {ok_count}/{count} frame(s) to {}", dir.display());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_grows_with_a_stretched_game() {
        assert_eq!(crop_rows(40, 600), 40);
        assert_eq!(crop_rows(40, 768), 40);
        assert_eq!(crop_rows(40, 1200), 63);
        assert_eq!(crop_rows(40, 1440), 75);
    }

    #[test]
    fn expect_size_takes_width_by_height() {
        let parsed = |s: &str| {
            parse(vec!["--expect-size".to_string(), s.to_string()]).map(|o| o.expect_size)
        };
        assert_eq!(parsed("1366x768"), Ok(Some((1366, 768))));
        assert_eq!(parsed("800X600"), Ok(Some((800, 600))));
        assert!(parsed("1366").is_err());
        assert!(parsed("0x768").is_err());
    }
}
