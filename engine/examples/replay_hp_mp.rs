//! Replays private capture runs through the engine loop, as it runs live, and prints every event it
//! emits as one JSON line: `{"atMs":1250,"event":{"type":"HpMpChanged","hp":712,...}}`. That's
//! the stream a plugin receives, so a plugin's logic can be replayed over a recording.
//!
//! ```text
//! cargo run -p mesovibe-engine --example replay_hp_mp -- [--frames] [--quiet] <run folder>...
//! ```
//!
//! - `--frames` also prints each frame's HP and MP reading, or why there was none, to stderr.
//! - `--quiet` leaves out the summary.
//!
//! Each run replays as one game window, one run after another, so a new run is a game restart. A
//! frame's time is its `atMs` in the run's frames.jsonl when the recording has one, or else its
//! index times the run's interval (meta.json `intervalSeconds`, or `hz` for a `live_exp`
//! recording), counted on from the end of the run before. Events go to stdout; the summary of how
//! often HP and MP read, and why not, goes to stderr.
//!
//! A frame that won't load stops the replay: its path and the error go to stderr, exit code 1. The
//! captures come from the capture tools or `live_exp` and stay outside the repo: they show other
//! players' names.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mesovibe_engine::capture::{Backend, CaptureError, GameWindow, Grab};
use mesovibe_engine::recognizers::hp_mp::{HpMp, HpMpUnreadable};
use mesovibe_engine::runner::Runner;
use mesovibe_engine::unstretch::scaled_rows;

// Shared with the other replays; this one hands frames to the engine loop whole.
#[allow(dead_code)]
#[path = "common/capture_run.rs"]
mod capture_run;
use capture_run::{Image, frames_of, load};

const USAGE: &str = "usage: replay_hp_mp [--frames] [--quiet] <run folder>...";

/// A capture backend that hands over a recording's frames, one per tick.
#[derive(Default)]
struct Recording {
    window: Option<GameWindow>,
    image: Option<Image>,
}

impl Backend for Recording {
    fn find(&mut self) -> Option<GameWindow> {
        self.window
    }

    fn grab(&mut self, _window: &GameWindow, rows: u32) -> Result<Grab, CaptureError> {
        let image = self
            .image
            .as_ref()
            .ok_or_else(|| CaptureError("no frame".to_string()))?;
        // As many of the bottom rows as were asked for, or all a cropped recording holds.
        let wanted = scaled_rows(rows, image.scale).min(image.height);
        let first_row = (image.height - wanted).max(image.first_row);
        let skip = ((first_row - image.first_row) * image.width * 4) as usize;
        Ok(Grab {
            width: image.width,
            height: image.height,
            first_row,
            bgra: image.bgra[skip..].to_vec(),
            scale: image.scale,
            fresh: true,
        })
    }

    fn stop(&mut self) {}
}

/// Each frame's time in a run, in milliseconds from the run's start, by file name.
fn frame_times(run: &Path, frames: &[PathBuf]) -> (HashMap<String, u64>, u64) {
    let meta: serde_json::Value = std::fs::read_to_string(run.join("meta.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let interval = match (meta["intervalSeconds"].as_f64(), meta["hz"].as_f64()) {
        (Some(seconds), _) => (seconds * 1000.0).round() as u64,
        (None, Some(hz)) if hz > 0.0 => (1000.0 / hz).round() as u64,
        _ => 1000,
    };
    let mut times = HashMap::new();
    let lines = std::fs::read_to_string(run.join("frames.jsonl")).unwrap_or_default();
    for line in lines.lines() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let (Some(file), Some(at)) = (entry["file"].as_str(), entry["atMs"].as_u64()) {
            times.insert(file.to_string(), at);
        }
    }
    for (i, path) in frames.iter().enumerate() {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        times
            .entry(file.into_owned())
            .or_insert(i as u64 * interval);
    }
    (times, interval)
}

fn describe(reading: &Result<HpMp, HpMpUnreadable>) -> String {
    match reading {
        Ok(r) => format!("HP {}/{}  MP {}/{}", r.hp, r.max_hp, r.mp, r.max_mp),
        Err(why) => format!("{why:?}"),
    }
}

fn main() -> ExitCode {
    let (mut frames_out, mut quiet, mut runs) = (false, false, Vec::new());
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--frames" => frames_out = true,
            "--quiet" => quiet = true,
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => runs.push(PathBuf::from(arg)),
        }
    }
    if runs.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }

    let mut runner = Runner::new(Recording::default());
    let mut base = 0u64;
    let (mut frames, mut read, mut events) = (0usize, 0usize, 0usize);
    let mut why: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_read: Option<String> = None;
    for (index, run) in runs.iter().enumerate() {
        let name = run
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let paths = frames_of(run);
        let (times, interval) = frame_times(run, &paths);
        let mut last = 0;
        for path in &paths {
            let image = match load(run, path) {
                Ok(image) => image,
                Err(e) => {
                    eprintln!("{}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            last = times.get(file.as_ref()).copied().unwrap_or(last);
            let scale = image.scale;
            runner.backend_mut().window = Some(GameWindow {
                id: index as u64 + 1,
                width: image.width / scale,
                height: image.height / scale,
                scale,
                minimized: false,
                placement: None,
            });
            runner.backend_mut().image = Some(image);
            let tick = runner.tick();
            for event in &tick.events {
                events += 1;
                let line = serde_json::json!({ "atMs": base + last, "event": event });
                println!("{line}");
            }
            let label = format!(
                "{name}/{}",
                path.file_stem().unwrap_or_default().to_string_lossy()
            );
            frames += 1;
            match &tick.hp_mp {
                Some(Ok(_)) => {
                    read += 1;
                    first_read.get_or_insert_with(|| label.clone());
                }
                Some(Err(e)) => *why.entry(format!("{e:?}")).or_default() += 1,
                None => *why.entry("no frame read".to_string()).or_default() += 1,
            }
            if frames_out {
                let reading = tick.hp_mp.as_ref().map_or("no frame read".into(), describe);
                eprintln!("{label}  {reading}");
            }
        }
        base += last + interval;
    }
    if !quiet {
        eprintln!("\n{frames} frames, {events} events");
        eprintln!(
            "HP and MP read on {read} frames, first at {}",
            first_read.as_deref().unwrap_or("none")
        );
        for (reason, n) in &why {
            eprintln!("  not read, {reason}: {n}");
        }
    }
    ExitCode::SUCCESS
}
