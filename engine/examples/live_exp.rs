//! Runs the engine on the live game window and prints what it reads: the capture state, each new
//! EXP bar reading, and the events. Windows and macOS.
//!
//! Read-only: Windows.Graphics.Capture or ScreenCaptureKit of the game window, nothing else
//! (see Fair play in README.md). The reader learns the digits from scratch each run.
//!
//! ```text
//! cargo run -p mesovibe-engine --release --example live_exp -- [--save <folder>] \
//!     [--record <folder>] [seconds, default 60] [Hz, default 4]
//! ```
//!
//! - `--save` writes the last rows the engine read to a private folder in the capture tool's
//!   format (meta.json and 0001.png), so `replay_exp` can read exactly what the engine saw.
//! - `--record` writes every frame the engine read, for a test run on the engine's own capture:
//!   meta.json (with `cropBottom` and `scale`), numbered strip PNGs and frames.jsonl, with one line
//!   per grab and what the engine read from it. `replay_exp <folder>` then checks the run. Grabs
//!   with no new frame are logged without a PNG, because the engine didn't read them. The
//!   engine grabs the whole game area now and then, to read the map's name; those frames are
//!   saved whole, so `replay_map` can read them. On a Retina Mac a strip is about 90 KB, so an
//!   hour is about 0.3 GB at 1 Hz and 1.3 GB at 4 Hz. A run stopped with Ctrl-C keeps every frame
//!   and log line up to the stop, but prints no summary.
//!
//! Like every capture, recordings stay off the repo. On macOS, run it from a terminal that
//! has Screen Recording permission (System Settings > Privacy & Security > Screen & System Audio
//! Recording).

#[cfg(any(windows, target_os = "macos"))]
fn main() {
    let (mut save, mut record) = (None, None);
    let mut numbers = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--save" => save = args.next().map(std::path::PathBuf::from),
            "--record" => record = args.next().map(std::path::PathBuf::from),
            _ => numbers.push(arg),
        }
    }
    let seconds: u64 = numbers.first().and_then(|a| a.parse().ok()).unwrap_or(60);
    let hz: f64 = numbers
        .get(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(4.0f64)
        .clamp(0.5, 4.0);
    let recorder = match record.as_deref().map(|f| strip::Recorder::create(f, hz)) {
        Some(Ok(recorder)) => Some(recorder),
        Some(Err(e)) => {
            eprintln!("Can't record: {e}");
            std::process::exit(1);
        }
        None => None,
    };

    #[cfg(windows)]
    let backend = {
        use mesovibe_engine::capture::windows::WindowsBackend;
        if !WindowsBackend::supported() {
            eprintln!(
                "This Windows has no Windows.Graphics.Capture (it needs Windows 10 1903 or later)."
            );
            std::process::exit(1);
        }
        WindowsBackend::new()
    };
    #[cfg(target_os = "macos")]
    let backend = mesovibe_engine::capture::macos::MacBackend::new();

    let runner = live::run(backend, seconds, hz, recorder, |_runner| {
        #[cfg(target_os = "macos")]
        if let Some((complete, idle, other)) = _runner.backend_mut().frame_counts() {
            println!(
                "ScreenCaptureKit frames: {complete} complete, {idle} idle (nothing new), \
                 {other} other."
            );
        }
    });
    if let (Some(folder), Some(grab)) = (save, runner.last_grab()) {
        match strip::save_grab(&folder, grab) {
            Ok(()) => println!("Saved what the engine last saw to {}", folder.display()),
            Err(e) => eprintln!("Couldn't save to {}: {e}", folder.display()),
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod live {
    use std::time::{Duration, Instant};

    use mesovibe_engine::capture::Backend;
    use mesovibe_engine::runner::Runner;

    use super::strip::Recorder;

    /// Runs the engine for `seconds` at `hz`, printing as it goes and recording if asked, then
    /// `finish` before the summary.
    pub fn run<B: Backend>(
        backend: B,
        seconds: u64,
        hz: f64,
        mut recorder: Option<Recorder>,
        finish: impl FnOnce(&mut Runner<B>),
    ) -> Runner<B> {
        println!("Reading the game for {seconds} s at {hz} Hz. Ctrl-C stops.");
        let interval = Duration::from_secs_f64(1.0 / hz);
        let mut runner = Runner::new(backend);
        let started = Instant::now();
        let (mut last_state, mut last_line) = (None, String::new());
        let (mut ticks, mut read, mut confirmed, mut events) = (0, 0, 0, 0);
        let (mut grabs, mut stale) = (0, 0);
        let mut tick_ms: Vec<f64> = Vec::new();
        while started.elapsed() < Duration::from_secs(seconds) {
            let tick_started = Instant::now();
            let tick = runner.tick();
            let took = tick_started.elapsed().as_secs_f64() * 1000.0;
            let at = started.elapsed().as_secs_f64();
            ticks += 1;
            if let Some(recorder) = &mut recorder
                && let Err(e) = recorder.record(&tick, runner.last_grab(), at)
            {
                eprintln!("[{at:6.1}s]   recording stopped: {e}");
                recorder.stop();
            }
            if last_state != Some(tick.state) {
                let window = tick
                    .window
                    .map(|w| format!(" (game area {}x{}, scale {})", w.width, w.height, w.scale))
                    .unwrap_or_default();
                println!("[{at:6.1}s] {:?}{window}", tick.state);
                last_state = Some(tick.state);
            }
            if let Some(error) = &tick.error {
                println!("[{at:6.1}s]   capture failed: {}", error.0);
            }
            if tick.stale {
                grabs += 1;
                stale += 1;
            }
            if let Some(exp) = &tick.exp {
                grabs += 1;
                tick_ms.push(took);
                let line = match exp {
                    Ok(r) => {
                        read += 1;
                        confirmed += usize::from(r.confirmed);
                        let level = r.level.map_or("?".to_string(), |l| l.to_string());
                        let mark = if r.confirmed { "" } else { "  (unconfirmed)" };
                        format!(
                            "level {level}  EXP {}  {}.{:02}%{mark}",
                            r.exp,
                            r.pct_hundredths / 100,
                            r.pct_hundredths % 100
                        )
                    }
                    Err(why) => format!("unreadable: {why:?}"),
                };
                if line != last_line {
                    println!("[{at:6.1}s]   {line}   ({took:.1} ms)");
                    last_line = line;
                }
            }
            for event in &tick.events {
                events += 1;
                println!("[{at:6.1}s]     event: {event:?}");
            }
            std::thread::sleep(interval.saturating_sub(tick_started.elapsed()));
        }

        tick_ms.sort_by(f64::total_cmp);
        let median = tick_ms.get(tick_ms.len() / 2).copied().unwrap_or(0.0);
        let slowest = tick_ms.last().copied().unwrap_or(0.0);
        println!(
            "\n{ticks} ticks: {read} read ({confirmed} confirmed), {events} events. \
             {grabs} grabs, {stale} with no new frame. A capturing tick took {median:.1} ms \
             (median), {slowest:.1} ms at most. Calibration: {:?}.",
            runner.exp_reader().calibration()
        );
        if let Some(recorder) = recorder {
            match recorder.finish() {
                Ok(summary) => println!("{summary}"),
                Err(e) => eprintln!("Recording incomplete: {e}"),
            }
        }
        finish(&mut runner);
        runner
    }
}

/// Grabs written in the capture tool's format, which `replay_exp` reads.
#[cfg(any(windows, target_os = "macos"))]
mod strip {
    use std::fs::File;
    use std::io::{BufWriter, Write};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use std::time::{SystemTime, UNIX_EPOCH};

    use mesovibe_engine::capture::Grab;
    use mesovibe_engine::runner::Tick;

    /// The meta.json for strips shaped like `grab`: the game area in captured pixels, the rows
    /// the first strip holds (each PNG holds its own), the scale, and the colors. The Mac stream hands over sRGB since
    /// 2026-11-04; replays take a Mac recording without `colorSpace` as Display P3.
    fn meta(grab: &Grab, extra: serde_json::Value) -> serde_json::Value {
        let mut meta = serde_json::json!({
            "tool": "live_exp",
            "toolVersion": 1,
            "os": std::env::consts::OS,
            "colorSpace": "sRGB",
            "cropBottom": grab.height - grab.first_row,
            "scale": grab.scale,
            "game": { "width": grab.width / grab.scale, "height": grab.height / grab.scale },
            "window": {
                "frame": { "x": 0, "y": 0 },
                "client": { "x": 0, "y": 0, "width": grab.width, "height": grab.height },
            },
        });
        if let (Some(meta), serde_json::Value::Object(extra)) = (meta.as_object_mut(), extra) {
            meta.extend(extra);
        }
        meta
    }

    /// A grab's rows as an RGBA PNG. Alpha is kept: a Mac window's rounded corners are
    /// see-through. Written under a temporary name and then renamed, so a run stopped with Ctrl-C
    /// never leaves half a PNG.
    fn write_png(path: &Path, grab: &Grab) -> Result<(), String> {
        let partial = path.with_extension("png.part");
        write_png_to(&partial, grab)?;
        std::fs::rename(&partial, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn write_png_to(path: &Path, grab: &Grab) -> Result<(), String> {
        let rows = grab.height - grab.first_row;
        let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), grab.width, rows);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let rgba: Vec<u8> = grab
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], p[3]])
            .collect();
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgba).map_err(|e| e.to_string())
    }

    /// Writes one grab: meta.json and 0001.png.
    pub fn save_grab(folder: &Path, grab: &Grab) -> Result<(), String> {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let meta = meta(grab, serde_json::json!({}));
        std::fs::write(folder.join("meta.json"), meta.to_string()).map_err(|e| e.to_string())?;
        write_png(&folder.join("0001.png"), grab)
    }

    fn unix_ms() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    /// Every frame the engine reads, written as it goes. PNGs are encoded on their own thread so
    /// the engine's ticks keep their pace.
    pub struct Recorder {
        folder: PathBuf,
        hz: f64,
        /// (width, height, scale) of the first grab, which meta.json describes. Grabs may hold
        /// more rows than the first: the whole game area, for the map's name.
        shape: Option<(u32, u32, u32)>,
        grabs: u32,
        saved: u32,
        /// Grabs not saved because the game area's size changed after the first.
        resized: u32,
        log: BufWriter<File>,
        frames: Option<mpsc::Sender<(PathBuf, Grab)>>,
        writer: Option<JoinHandle<Result<(), String>>>,
    }

    impl Recorder {
        /// Starts a recording in `folder`, which must be new or empty: a run is never mixed
        /// with another.
        pub fn create(folder: &Path, hz: f64) -> Result<Recorder, String> {
            if std::fs::read_dir(folder).is_ok_and(|mut entries| entries.next().is_some()) {
                return Err(format!(
                    "{} isn't empty. Pick a new folder for each run.",
                    folder.display()
                ));
            }
            std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
            let log = File::create(folder.join("frames.jsonl"))
                .map_err(|e| format!("frames.jsonl: {e}"))?;
            let (frames, queue) = mpsc::channel::<(PathBuf, Grab)>();
            let writer = std::thread::spawn(move || {
                for (path, grab) in queue {
                    write_png(&path, &grab)?;
                }
                Ok(())
            });
            Ok(Recorder {
                folder: folder.to_path_buf(),
                hz,
                shape: None,
                grabs: 0,
                saved: 0,
                resized: 0,
                log: BufWriter::new(log),
                frames: Some(frames),
                writer: Some(writer),
            })
        }

        /// Records a tick that grabbed: its PNG if the engine read it, and a log line either way.
        /// `at` is seconds since the run started.
        pub fn record(&mut self, tick: &Tick, grab: Option<&Grab>, at: f64) -> Result<(), String> {
            let Some(grab) = grab.filter(|_| tick.exp.is_some() || tick.stale) else {
                return Ok(());
            };
            let frames = self.frames.as_ref().ok_or("stopped")?;
            self.grabs += 1;
            // The rows a grab holds can change within a run (the whole game area for the map's
            // name; Full Screen adds the bar under the picture once it's measured); each PNG keeps
            // its own, so only the game area counts.
            let shape = (grab.width, grab.height, grab.scale);
            if self.shape.is_none() {
                let meta = meta(
                    grab,
                    serde_json::json!({ "startedAt": unix_ms(), "hz": self.hz }),
                );
                std::fs::write(self.folder.join("meta.json"), meta.to_string())
                    .map_err(|e| format!("meta.json: {e}"))?;
                self.shape = Some(shape);
            }
            let resized = self.shape != Some(shape);
            self.resized += u32::from(resized);
            let file = (!tick.stale && !resized).then(|| format!("{:06}.png", self.saved + 1));
            if let Some(name) = &file {
                self.saved += 1;
                frames
                    .send((self.folder.join(name), grab.clone()))
                    .map_err(|_| "the PNG writer stopped".to_string())?;
            }
            let reading = match &tick.exp {
                Some(Ok(r)) => serde_json::json!({
                    "level": r.level,
                    "exp": r.exp,
                    "pctHundredths": r.pct_hundredths,
                    "confirmed": r.confirmed,
                }),
                Some(Err(why)) => serde_json::json!({ "unreadable": format!("{why:?}") }),
                None => serde_json::Value::Null,
            };
            let line = serde_json::json!({
                "index": self.grabs,
                "file": file,
                "atMs": (at * 1000.0).round() as u64,
                "unixMs": unix_ms(),
                "state": format!("{:?}", tick.state),
                "fresh": !tick.stale,
                "sizeChanged": resized,
                "width": grab.width,
                "height": grab.height,
                "rows": grab.height - grab.first_row,
                "scale": grab.scale,
                "reading": reading,
            });
            // Flushed every line: a run stopped with Ctrl-C keeps its whole log.
            writeln!(self.log, "{line}")
                .and_then(|()| self.log.flush())
                .map_err(|e| format!("frames.jsonl: {e}"))
        }

        /// Stops recording after an error; what's written so far stays.
        pub fn stop(&mut self) {
            self.frames = None;
        }

        /// Waits for the last PNGs and says what was written.
        pub fn finish(mut self) -> Result<String, String> {
            self.frames = None;
            self.log.flush().map_err(|e| format!("frames.jsonl: {e}"))?;
            if let Some(writer) = self.writer.take() {
                writer
                    .join()
                    .map_err(|_| "the PNG writer crashed".to_string())??;
            }
            let resized = match self.resized {
                0 => String::new(),
                n => format!(" {n} frames weren't saved because the game area changed size."),
            };
            Ok(format!(
                "Recorded {} of {} grabs to {} (the rest brought no new frame).{resized} Check \
                 it with: cargo run -p mesovibe-engine --release --example replay_exp -- {}",
                self.saved,
                self.grabs,
                self.folder.display(),
                self.folder.display()
            ))
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("live_exp needs the Windows or macOS capture backend.");
    std::process::exit(1);
}
