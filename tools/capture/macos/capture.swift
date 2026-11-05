// MesoVibe capture helper, macOS: screenshots of the game window for developing and testing
// recognizers.
//
// Saves screenshots of the MapleStory window using macOS's own `screencapture` tool, the same
// thing Cmd-Shift-4 does. It only reads pixels from the screen. It never sends input to the game,
// never reads or writes game memory or files, and never touches network traffic (see Fair
// play in README.md).
//
// Captures are private: they can include other players' names and chat. They are written outside
// the repo, to ~/MesoVibe-captures by default, and must never be committed.
//
// Usage: tools/capture/macos/capture <command> [options]   (see `help`)

import AppKit
import CoreGraphics
import Foundation
import ImageIO

let toolVersion = 1

// MARK: - Window discovery

struct Rect: Codable {
    let x: Double, y: Double, width: Double, height: Double
}

struct GameWindow: Codable {
    let windowID: Int
    let owner: String
    let pid: Int
    let title: String?
    let bounds: Rect  // in points (macOS logical pixels)
    let onScreen: Bool
    let layer: Int
    let bundleID: String?
    let appPath: String?
    let executable: String?
}

func windows(includeOffscreen: Bool) -> [GameWindow] {
    let options: CGWindowListOption =
        includeOffscreen ? [.optionAll, .excludeDesktopElements] : [.optionOnScreenOnly, .excludeDesktopElements]
    let raw = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
    return raw.compactMap { info in
        guard let id = info[kCGWindowNumber as String] as? Int,
            let pid = info[kCGWindowOwnerPID as String] as? Int,
            let boundsDict = info[kCGWindowBounds as String] as? NSDictionary,
            let cgBounds = CGRect(dictionaryRepresentation: boundsDict)
        else { return nil }
        let app = NSRunningApplication(processIdentifier: pid_t(pid))
        return GameWindow(
            windowID: id,
            owner: info[kCGWindowOwnerName as String] as? String ?? "?",
            pid: pid,
            title: info[kCGWindowName as String] as? String,
            bounds: Rect(
                x: cgBounds.origin.x, y: cgBounds.origin.y, width: cgBounds.width, height: cgBounds.height),
            onScreen: info[kCGWindowIsOnscreen as String] as? Bool ?? false,
            layer: info[kCGWindowLayer as String] as? Int ?? 0,
            bundleID: app?.bundleIdentifier,
            appPath: app?.bundleURL?.path,
            executable: app?.executableURL?.path
        )
    }
}

/// App windows big enough to be a game, not menu-bar items or tooltips. Any window layer: Wine can
/// raise the game above normal windows (it was at layer 26 on 2026-11-03).
func isAppWindow(_ w: GameWindow) -> Bool {
    w.bounds.width >= 200 && w.bounds.height >= 150
}

/// Matches the app's name and bundle ID only. Window titles and paths would mislead: an editor
/// window titled "… — MesoVibe" contains "maple", and on macOS the game itself is installed inside
/// "MapleStory Launcher.app".
func matches(_ w: GameWindow, _ pattern: String) -> Bool {
    let p = pattern.lowercased()
    return [w.owner, w.bundleID].compactMap { $0?.lowercased() }.contains { $0.contains(p) }
}

/// Nexon's launcher ("MapleStory Launcher", window "Nexon Global Launcher"), not the game.
func isLauncher(_ w: GameWindow) -> Bool {
    [w.owner, w.title].compactMap { $0?.lowercased() }.contains { $0.contains("launcher") }
}

/// The likely game window: a match for the pattern that isn't the launcher, largest first.
func findGameWindow(pattern: String) -> GameWindow? {
    let candidates = windows(includeOffscreen: false).filter { isAppWindow($0) && matches($0, pattern) }
    let notLauncher = candidates.filter { !isLauncher($0) }
    let pool = notLauncher.isEmpty ? candidates : notLauncher
    return pool.max { $0.bounds.width * $0.bounds.height < $1.bounds.width * $1.bounds.height }
}

func window(id: Int) -> GameWindow? {
    windows(includeOffscreen: true).first { $0.windowID == id }
}

// MARK: - Capture

struct Frame: Codable {
    let index: Int
    let file: String?
    let capturedAt: String
    let captureMs: Int
    let ok: Bool
    let error: String?
    let pixelWidth: Int?
    let pixelHeight: Int?
    let windowOnScreen: Bool?
    let windowBounds: Rect?
}

func pixelSize(of url: URL) -> (Int, Int)? {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
        let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
        let w = props[kCGImagePropertyPixelWidth] as? Int,
        let h = props[kCGImagePropertyPixelHeight] as? Int
    else { return nil }
    return (w, h)
}

let isoFormatter: ISO8601DateFormatter = {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return f
}()

/// One PNG of one window, without its shadow, without the shutter sound.
func captureFrame(windowID: Int, index: Int, into dir: URL) -> Frame {
    let name = String(format: "%04d.png", index)
    let url = dir.appendingPathComponent(name)
    let started = Date()
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    process.arguments = ["-l", String(windowID), "-o", "-x", "-t", "png", url.path]
    let stderr = Pipe()
    process.standardError = stderr
    var error: String?
    do {
        try process.run()
        process.waitUntilExit()
        if process.terminationStatus != 0 {
            let text = String(data: stderr.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
            error = "screencapture exit \(process.terminationStatus): \(text.trimmingCharacters(in: .whitespacesAndNewlines))"
        }
    } catch {
        return Frame(
            index: index, file: nil, capturedAt: isoFormatter.string(from: started), captureMs: 0, ok: false,
            error: "could not run screencapture: \(error)", pixelWidth: nil, pixelHeight: nil,
            windowOnScreen: nil, windowBounds: nil)
    }
    let ms = Int(Date().timeIntervalSince(started) * 1000)
    let size = FileManager.default.fileExists(atPath: url.path) ? pixelSize(of: url) : nil
    if error == nil && size == nil { error = "no image written" }
    let current = window(id: windowID)
    return Frame(
        index: index, file: size == nil ? nil : name, capturedAt: isoFormatter.string(from: started),
        captureMs: ms, ok: error == nil, error: error, pixelWidth: size?.0, pixelHeight: size?.1,
        windowOnScreen: current?.onScreen, windowBounds: current?.bounds)
}

// MARK: - Session metadata

struct Display: Codable {
    let name: String
    let points: Rect
    let backingScale: Double
}

struct Session: Codable {
    let tool: String
    let toolVersion: Int
    let label: String
    let startedAt: String
    let macOS: String
    let machine: String
    let displays: [Display]
    let window: GameWindow
    let windowDisplayScale: Double?
    let plannedFrames: Int
    let intervalSeconds: Double
}

func machineModel() -> String {
    var size = 0
    sysctlbyname("hw.model", nil, &size, nil, 0)
    var buffer = [CChar](repeating: 0, count: max(size, 1))
    sysctlbyname("hw.model", &buffer, &size, nil, 0)
    return String(cString: buffer)
}

func displays() -> [Display] {
    NSScreen.screens.map {
        Display(
            name: $0.localizedName,
            points: Rect(x: $0.frame.origin.x, y: $0.frame.origin.y, width: $0.frame.width, height: $0.frame.height),
            backingScale: Double($0.backingScaleFactor))
    }
}

/// Backing scale of the display holding the window's center (2.0 on Retina). CGWindow bounds use a
/// top-left origin on the main display; NSScreen frames use bottom-left, so flip y first.
func displayScale(for w: GameWindow) -> Double? {
    guard let mainHeight = NSScreen.screens.first?.frame.height else { return nil }
    let center = NSPoint(x: w.bounds.x + w.bounds.width / 2, y: mainHeight - (w.bounds.y + w.bounds.height / 2))
    return NSScreen.screens.first { $0.frame.contains(center) }.map { Double($0.backingScaleFactor) }
}

func writeJSON<T: Encodable>(_ value: T, to url: URL) throws {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(value).write(to: url)
}

func appendJSONLine<T: Encodable>(_ value: T, to url: URL) throws {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    var data = try encoder.encode(value)
    data.append(0x0A)
    if let handle = try? FileHandle(forWritingTo: url) {
        defer { try? handle.close() }
        try handle.seekToEnd()
        try handle.write(contentsOf: data)
    } else {
        try data.write(to: url)
    }
}

// MARK: - Commands

struct Options {
    var positional: [String] = []
    var windowID: Int?
    var pattern = "maple"
    var outRoot = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("MesoVibe-captures")
    var all = false
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

func parse(_ args: [String]) -> Options {
    var o = Options()
    if let env = ProcessInfo.processInfo.environment["MESOVIBE_CAPTURES"], !env.isEmpty {
        o.outRoot = URL(fileURLWithPath: (env as NSString).expandingTildeInPath)
    }
    var i = 0
    while i < args.count {
        let a = args[i]
        func value() -> String {
            i += 1
            guard i < args.count else { fail("\(a) needs a value") }
            return args[i]
        }
        switch a {
        case "--window": guard let id = Int(value()) else { fail("--window takes a window number from `list`") }; o.windowID = id
        case "--match": o.pattern = value()
        case "--out": o.outRoot = URL(fileURLWithPath: (value() as NSString).expandingTildeInPath)
        case "--all": o.all = true
        default: o.positional.append(a)
        }
        i += 1
    }
    return o
}

func ensurePermission() {
    if CGPreflightScreenCaptureAccess() { return }
    // Adds this terminal app to the Screen Recording list and shows the system prompt once.
    _ = CGRequestScreenCaptureAccess()
    fail(
        """
        Screen Recording permission is off for the app running this command.
          1. Open System Settings > Privacy & Security > Screen & System Audio Recording.
          2. Turn on the terminal you ran this from (Terminal.app is recommended).
          3. Quit and reopen that terminal, then run the command again.
        """)
}

func describe(_ w: GameWindow) -> String {
    let size = "\(Int(w.bounds.width))x\(Int(w.bounds.height))"
    var parts = ["#\(w.windowID)", w.owner, "\"\(w.title ?? "(title hidden)")\"", size + " pt"]
    if !w.onScreen { parts.append("(hidden)") }
    if let bundle = w.bundleID { parts.append(bundle) }
    return parts.joined(separator: "  ")
}

/// Lists only windows of apps matching the pattern unless --all: other windows' titles (chats,
/// mail) are private and easy to paste by accident.
func cmdList(_ o: Options) {
    let visible = windows(includeOffscreen: false).filter(isAppWindow)
    let shown = o.all ? visible : visible.filter { matches($0, o.pattern) }
    let pick = findGameWindow(pattern: o.pattern)
    let scope = o.all ? "Visible app windows" : "Windows of apps matching \"\(o.pattern)\""
    print("\(scope) (\u{2192} = what `shot` and `burst` would capture):")
    for w in shown.sorted(by: { $0.owner < $1.owner }) {
        let marker = w.windowID == pick?.windowID ? "\u{2192}" : " "
        print(" \(marker) \(describe(w))")
    }
    if pick == nil {
        print("\nNo game window found. Start the game, or run `list --all` and pass --window <number>.")
    } else if let pick, isLauncher(pick) {
        print("\nOnly the launcher is open. Start the game from it, then run `list` again.")
    }
    if !CGPreflightScreenCaptureAccess() {
        print("\nNote: Screen Recording permission is off, so window titles are hidden. Run `check` to fix it.")
    }
}

func cmdCheck() {
    if CGPreflightScreenCaptureAccess() {
        print("Screen Recording permission: on. You're ready to capture.")
    } else {
        ensurePermission()
    }
}

func cmdCapture(_ o: Options, burst: Bool) {
    ensurePermission()
    let label = o.positional.first ?? (burst ? "burst" : "shot")
    var count = 1
    var interval = 0.0
    if burst {
        let c = o.positional.count > 1 ? Int(o.positional[1]) : 30
        let s = o.positional.count > 2 ? Double(o.positional[2]) : 2.0
        guard let c, (1...3600).contains(c) else { fail("frame count must be 1 to 3600") }
        guard let s, s >= 0.25 else { fail("interval must be at least 0.25 seconds") }
        count = c
        interval = s
    }

    let target: GameWindow
    if let id = o.windowID {
        guard let w = window(id: id) else { fail("No window #\(id). Run `list` to see window numbers.") }
        target = w
    } else {
        guard let w = findGameWindow(pattern: o.pattern) else {
            fail("No game window found (looked for \"\(o.pattern)\"). Run `list`, then pass --window <number>.")
        }
        target = w
    }
    if isLauncher(target) {
        print("Warning: this looks like the launcher, not the game: \(describe(target))")
    }

    let now = Date()
    let day = DateFormatter()
    day.dateFormat = "yyyy-MM-dd"
    let time = DateFormatter()
    time.dateFormat = "HHmmss"
    let safeLabel = label.lowercased().map { $0.isLetter || $0.isNumber ? $0 : "-" }
    let dir = o.outRoot
        .appendingPathComponent(day.string(from: now))
        .appendingPathComponent("\(time.string(from: now))-\(String(safeLabel))")
    do {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try writeJSON(
            Session(
                tool: "mesovibe-capture-macos", toolVersion: toolVersion, label: label,
                startedAt: isoFormatter.string(from: now),
                macOS: ProcessInfo.processInfo.operatingSystemVersionString, machine: machineModel(),
                displays: displays(), window: target, windowDisplayScale: displayScale(for: target),
                plannedFrames: count, intervalSeconds: interval),
            to: dir.appendingPathComponent("meta.json"))
    } catch {
        fail("Could not write to \(dir.path): \(error)")
    }

    print("Capturing \(describe(target))")
    if burst { print("\(count) frames, one every \(interval)s. Ctrl-C stops early; frames so far are kept.") }
    var okCount = 0
    let framesFile = dir.appendingPathComponent("frames.jsonl")
    for index in 1...count {
        let tick = Date()
        let frame = captureFrame(windowID: target.windowID, index: index, into: dir)
        try? appendJSONLine(frame, to: framesFile)
        if frame.ok { okCount += 1 }
        let status =
            frame.ok
            ? "\(frame.pixelWidth ?? 0)x\(frame.pixelHeight ?? 0) px in \(frame.captureMs) ms"
            : "FAILED: \(frame.error ?? "?")"
        print(String(format: "  %04d  %@", index, status))
        if index < count {
            let wait = interval - Date().timeIntervalSince(tick)
            if wait > 0 { Thread.sleep(forTimeInterval: wait) }
        }
    }
    print("Saved \(okCount)/\(count) frame(s) to \(dir.path)")
    if okCount < count && burst {
        print("Failed frames are expected while the game is minimized or hidden; that's useful data too.")
    }
}

let usage = """
    MesoVibe capture helper (macOS). Screenshots of the game window, nothing else.

      capture check                         check Screen Recording permission
      capture list                          show the game's windows and which one is the game
      capture shot  <label>                 one screenshot of the game window
      capture burst <label> [count] [secs]  count screenshots, one every secs (default 30, 2)

    Options:
      --window <number>   capture this window from `list` instead of auto-detecting
      --match <text>      auto-detect by this text in the app name or bundle ID (default: maple)
      --all               `list` every visible app window, not only the game's
      --out <folder>      where to save (default: ~/MesoVibe-captures, or $MESOVIBE_CAPTURES)

    Each run makes a folder like ~/MesoVibe-captures/2026-11-02/181530-<label>/ with numbered
    PNGs, meta.json (window, app, displays) and frames.jsonl (timing per frame).
    """

let opts = parse(Array(CommandLine.arguments.dropFirst()))
switch opts.positional.first {
case "check": cmdCheck()
case "list": cmdList(opts)
case "shot", "burst":
    var sub = opts
    sub.positional = Array(opts.positional.dropFirst())
    cmdCapture(sub, burst: opts.positional.first == "burst")
case nil, "help", "-h", "--help": print(usage)
default: fail("Unknown command \"\(opts.positional[0])\".\n\n" + usage)
}
