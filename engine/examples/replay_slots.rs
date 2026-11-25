//! Replays private capture runs through the quick-slot reader and prints what each slot showed,
//! frame by frame, with the HP and MP numbers the HUD reader read in the same frame. A potion use
//! shows as a slot's count changing as HP or MP rises: each change sent is listed with the rises
//! in its frame and the one before.
//!
//! ```text
//! cargo run -p mesovibe-engine --release --example replay_slots -- [--quiet] \
//!     [--knowledge <exp-bar.txt>] <run folder>...
//! ```
//!
//! - `--quiet` leaves out the line per frame.
//! - `--knowledge` starts the HUD reader from an `exp-bar.txt` the app kept, so the HP and MP
//!   numbers read from the first frame.
//!
//! Each run starts as a game restart: every slot's first agreed reading is sent again. A frame's
//! slots print as `count/icon` keys, cut to four characters, `-` for no count and `?` while the
//! panel isn't found. The captures come from the capture tools or `live_exp` and stay outside the
//! repo: they show other players' names.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use mesovibe_engine::recognizers::exp_bar::ExpBarReader;
use mesovibe_engine::recognizers::hp_mp::HpMp;
use mesovibe_engine::recognizers::quick_slots::{QuickSlotReader, QuickSlotState, Slot};

#[path = "common/capture_run.rs"]
mod capture_run;
use capture_run::{Scene, frames_of, load};

const USAGE: &str = "usage: replay_slots [--quiet] [--knowledge <exp-bar.txt>] <run folder>...";

fn short(slot: &Option<Slot>) -> String {
    match slot {
        None => "?".to_string(),
        Some(s) => format!(
            "{}/{}",
            s.count.as_deref().map_or("-", |c| &c[..4]),
            &s.icon[..4]
        ),
    }
}

/// The rise from one reading to the next, as "HP +160 MP +150", or "" when neither rose.
fn rises(before: Option<HpMp>, now: Option<HpMp>) -> String {
    let (Some(b), Some(n)) = (before, now) else {
        return String::new();
    };
    let mut out = Vec::new();
    if n.hp > b.hp {
        out.push(format!("HP +{}", n.hp - b.hp));
    }
    if n.mp > b.mp {
        out.push(format!("MP +{}", n.mp - b.mp));
    }
    out.join(" ")
}

fn main() -> ExitCode {
    let mut quiet = false;
    let mut exp = ExpBarReader::new();
    let mut runs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--quiet" => quiet = true,
            "--knowledge" => {
                let Some(path) = args.next() else {
                    eprintln!("{USAGE}");
                    return ExitCode::FAILURE;
                };
                let kept = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| ExpBarReader::from_knowledge(&text));
                match kept {
                    Some(reader) => exp = reader,
                    None => {
                        eprintln!("{path}: not a file the app keeps for the EXP bar");
                        return ExitCode::FAILURE;
                    }
                }
            }
            _ if arg.starts_with('-') => {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            _ => runs.push(PathBuf::from(arg)),
        }
    }
    if runs.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }

    let mut reader = QuickSlotReader::new();
    let mut scene = Scene::default();
    let (mut frames, mut found, mut unsupported) = (0usize, 0usize, 0usize);
    let mut sent = 0usize;
    let mut micros: Vec<u128> = Vec::new();
    for run in &runs {
        let name = run.file_name().unwrap_or_default().to_string_lossy();
        reader.new_look();
        let (mut run_frames, mut run_found) = (0usize, 0usize);
        // The HP and MP numbers of the last two frames: a change is sent a frame after it shows.
        let (mut older, mut last): (Option<HpMp>, Option<HpMp>) = (None, None);
        for path in frames_of(run) {
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            let label = format!("{name}/{stem}");
            let image = match load(run, &path) {
                Ok(image) => image,
                Err(e) => {
                    eprintln!("{label}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let frame = scene.picture(image.frame());
            if scene.changed() {
                exp.recheck();
            }
            let hud = exp.observe_hud(&frame);
            scene.follow(&frame, &hud.exp);
            let hp_mp = hud.hp_mp.ok();
            let started = Instant::now();
            let reading = reader.observe(&frame, exp.block_offset());
            micros.push(started.elapsed().as_micros());
            frames += 1;
            run_frames += 1;
            found += usize::from(reading.found);
            run_found += usize::from(reading.found);
            unsupported += usize::from(reader.state() == QuickSlotState::Unsupported);
            if !quiet {
                let slots: Vec<String> = reader.shown().iter().map(short).collect();
                let numbers = hp_mp.map_or("HP/MP unread".to_string(), |n| {
                    format!("HP {}/{} MP {}/{}", n.hp, n.max_hp, n.mp, n.max_mp)
                });
                println!("{label}  {}  {numbers}", slots.join(" "));
            }
            for (slot, read) in &reading.changed {
                sent += 1;
                let around = [rises(older, last), rises(last, hp_mp)]
                    .into_iter()
                    .filter(|r| !r.is_empty())
                    .collect::<Vec<_>>()
                    .join(", then ");
                println!(
                    "{label}  sent slot {slot}: {}  {}",
                    short(&Some(read.clone())),
                    if around.is_empty() {
                        "no rise".to_string()
                    } else {
                        around
                    }
                );
            }
            older = last;
            last = hp_mp;
        }
        println!("== {name}: panel found in {run_found} of {run_frames} frames");
    }
    micros.sort_unstable();
    let median = micros.get(micros.len() / 2).copied().unwrap_or(0);
    let slowest = micros.last().copied().unwrap_or(0);
    println!();
    println!("{frames} frames: panel found in {found}, unsupported size in {unsupported}");
    println!("slot readings sent: {sent}");
    println!(
        "quick-slot reader: median {:.3} ms, slowest {:.3} ms per frame",
        median as f64 / 1000.0,
        slowest as f64 / 1000.0
    );
    ExitCode::SUCCESS
}
