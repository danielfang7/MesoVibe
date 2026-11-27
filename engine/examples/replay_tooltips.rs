//! Replays private capture runs through the tooltip reader, the way the engine runs it on whole
//! frames, and reports the hovered item's tooltip in each frame: its box, the name read and
//! whether two frames in a row agree, or why the name wasn't read. Ends with how long the reader
//! took per frame.
//!
//! The reader learns the name font from the frames and the item list itself (nobody types
//! anything), so runs read from scratch show how soon it names items. The captures come from the
//! capture tools (tools/capture, Windows and macOS) and stay outside the repo. Mac captures are
//! read at their backing scale, one pixel from each block, where the EXP bar reader found the
//! blocks start, as the engine does live.
//!
//! ```text
//! cargo run -p mesovibe-engine --release --example replay_tooltips -- [--quiet] \
//!     [--knowledge <tooltip-font.txt>] [--keep <file>] [--other-names <file>] <run folder>...
//! ```
//!
//! - `--knowledge` starts from what an earlier replay kept with `--keep`, as the app starts a
//!   session from what it kept.
//! - `--other-names` gives names the game shows that the item list lacks (one per line, each
//!   optionally followed by a tab and its ids, comma-separated: the price service's cash items),
//!   told apart from items, and reported when they have ids, as the app does.
//! - `--quiet` leaves out the line per frame.
//!
//! Runs are replayed in the order given, as one session, so name them in the order they were
//! recorded. Each frame is read three times from the same starting point and the fastest time
//! kept, so a one-off pause by the system doesn't count.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use mesovibe_engine::recognizers::exp_bar::ExpBarReader;
use mesovibe_engine::recognizers::tooltip::{Tooltip, TooltipReader};

#[path = "common/capture_run.rs"]
mod capture_run;
use capture_run::{Scene, frames_of, load};

struct Args {
    knowledge: Option<(String, TooltipReader)>,
    keep: Option<String>,
    other_names: Option<String>,
    quiet: bool,
    runs: Vec<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut out = Args {
        knowledge: None,
        keep: None,
        other_names: None,
        quiet: false,
        runs: Vec::new(),
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--knowledge" => {
                let path = args.next().ok_or("--knowledge needs a file")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                let reader = TooltipReader::from_knowledge(&text)
                    .ok_or(format!("{path}: not text a tooltip reader kept"))?;
                out.knowledge = Some((path, reader));
            }
            "--keep" => out.keep = Some(args.next().ok_or("--keep needs a file")?),
            "--other-names" => {
                let path = args.next().ok_or("--other-names needs a file")?;
                out.other_names =
                    Some(std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?);
            }
            "--quiet" => out.quiet = true,
            _ => out.runs.push(PathBuf::from(a)),
        }
    }
    if out.runs.is_empty() {
        return Err(
            "usage: replay_tooltips [--quiet] [--knowledge <file>] [--keep <file>] [--other-names <file>] <run>..."
                .into(),
        );
    }
    Ok(out)
}

fn describe(t: &Tooltip) -> String {
    let r = t.rect;
    let place = format!("box {},{} {}x{}", r.x, r.y, r.width, r.height);
    match (&t.name, &t.items) {
        (Some(name), Some(ids)) => format!(
            "{place} {name:?} {ids:?}{}",
            if t.confirmed { " confirmed" } else { "" }
        ),
        _ => format!("{place} unsure {:?}", t.unsure),
    }
}

fn ms(micros: u128) -> f64 {
    micros as f64 / 1000.0
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut reader = match args.knowledge {
        Some((path, kept)) => {
            println!("starting from the letters kept in {path}");
            kept
        }
        None => {
            println!("learning the name font from the frames and the item list: nothing kept");
            TooltipReader::new()
        }
    };
    if let Some(text) = &args.other_names {
        // One name per line, or a name, a tab and its ids ("Angel Halo\t1007077"), as the app
        // gives the price file's cash items.
        let names: Vec<(&str, Vec<u32>)> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| match line.split_once('\t') {
                Some((name, ids)) => (
                    name,
                    ids.split(',')
                        .filter_map(|id| id.trim().parse().ok())
                        .collect(),
                ),
                None => (line, Vec::new()),
            })
            .collect();
        reader = reader.with_other_names(names.iter().map(|(name, ids)| (*name, ids.as_slice())));
        println!("telling items apart from {} other names", names.len());
    }
    let mut exp = ExpBarReader::new();
    let mut scene = Scene::default();
    let mut timings: BTreeMap<(u32, u32, u32), Vec<u128>> = BTreeMap::new();
    let (mut frames, mut tooltips, mut named, mut confirmed) = (0usize, 0usize, 0usize, 0usize);

    for run in &args.runs {
        let name = run
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for path in frames_of(run) {
            let frame_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let label = format!("{name}/{frame_name}");
            let image = match load(run, &path) {
                Ok(i) => i,
                Err(e) => {
                    println!("{label}  load error: {e}");
                    continue;
                }
            };
            let frame = scene.picture(image.frame());
            if frame.first_row() != 0 {
                continue;
            }
            frames += 1;
            let hud = exp.observe_hud(&frame);
            scene.follow(&frame, &hud.exp);
            let blocks = exp.block_offset();
            // The fastest of three reads from the same starting point.
            let mut fastest = u128::MAX;
            for _ in 0..2 {
                let mut trial = reader.clone();
                let started = Instant::now();
                let _ = trial.observe(&frame, blocks);
                fastest = fastest.min(started.elapsed().as_micros());
            }
            let started = Instant::now();
            let reading = reader.observe(&frame, blocks);
            fastest = fastest.min(started.elapsed().as_micros());
            let scale = frame.scale();
            timings
                .entry((frame.width() / scale, frame.height() / scale, scale))
                .or_default()
                .push(fastest);
            match &reading.tooltip {
                Some(t) => {
                    tooltips += 1;
                    named += usize::from(t.items.is_some());
                    confirmed += usize::from(t.confirmed);
                    if !args.quiet {
                        println!("{label}  {}", describe(t));
                    }
                }
                None if !args.quiet => println!("{label}  none"),
                None => {}
            }
        }
    }

    println!(
        "\n{frames} whole frames, {tooltips} with an item tooltip, {named} named, {confirmed} confirmed"
    );
    for ((w, h, s), mut t) in timings {
        t.sort_unstable();
        let median = t.get(t.len() / 2).copied().unwrap_or(0);
        let slowest = t.last().copied().unwrap_or(0);
        println!(
            "{w}x{h} at {s}x: {} frames, median {:.2} ms, slowest {:.2} ms",
            t.len(),
            ms(median),
            ms(slowest)
        );
    }
    if let Some(path) = args.keep {
        match reader.knowledge() {
            Some(text) => match std::fs::write(&path, text) {
                Ok(()) => println!("kept the letters learned in {path}"),
                Err(e) => println!("{path}: {e}"),
            },
            None => println!("nothing learned to keep"),
        }
    }
    ExitCode::SUCCESS
}
