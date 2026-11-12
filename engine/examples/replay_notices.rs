//! Replays private capture runs through the pickup notice reader, with the EXP bar reader beside it
//! the way the engine runs them, and prints what it read: each frame's new lines, the totals per
//! run, and where lines may have been missed.
//!
//! The readers learn everything from the frames themselves. The captures come from the capture
//! tools (tools/capture, Windows and macOS) or `live_exp`, and stay outside the repo, along with
//! any tally of what the runs really hold.
//!
//! ```text
//! cargo run -p mesovibe-engine --release --example replay_notices -- [--quiet] [--warm] [--carry | --join] [--knowledge <notices.txt>] <run folder>...
//! ```
//!
//! Each run starts with readers that know nothing, unless `--carry` passes what the notice reader
//! confirmed from one run to the next through the text the app keeps between launches, or `--join`
//! reads the runs as one session with gaps between them, keeping what it was still learning too.
//! `--knowledge` starts the first run's notice reader from a `notices.txt` the app kept, as the app
//! starts a session.
//! The EXP bar reader learns from scratch in each run too, unless `--warm` first runs it over all
//! the runs to learn its digits and starts each run from them, as the app does with the digits it
//! keeps between launches.
//!
//! Mesos amounts with a digit not learned yet are kept and counted once their digits are known;
//! the totals say how many were read late and how many are still waiting.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use mesovibe_engine::recognizers::exp_bar::{ExpBarReader, ExpReading, Unreadable};
use mesovibe_engine::recognizers::notices::{Missed, Notice, NoticeReader};

#[path = "common/capture_run.rs"]
mod capture_run;
use capture_run::{frames_of, load, picture};

struct Args {
    quiet: bool,
    warm: bool,
    carry: bool,
    join: bool,
    /// A notice reader to start the first run from, as the app starts one from `notices.txt`.
    knowledge: Option<NoticeReader>,
    runs: Vec<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut out = Args {
        quiet: false,
        warm: false,
        carry: false,
        join: false,
        knowledge: None,
        runs: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--quiet" => out.quiet = true,
            "--warm" => out.warm = true,
            "--carry" => out.carry = true,
            "--join" => out.join = true,
            "--knowledge" => {
                let path = args.next().ok_or("--knowledge needs a notices.txt")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                let reader = NoticeReader::from_knowledge(&text)
                    .ok_or(format!("{path}: not text a notice reader kept"))?;
                out.knowledge = Some(reader);
            }
            _ => out.runs.push(PathBuf::from(a)),
        }
    }
    if out.runs.is_empty() {
        return Err(
            "usage: replay_notices [--quiet] [--warm] [--carry | --join] \
                    [--knowledge <notices.txt>] <run>..."
                .into(),
        );
    }
    Ok(out)
}

/// Marks an amount read with a digit named by its shape.
fn shape_mark(shaped: bool) -> &'static str {
    if shaped { " (shape)" } else { "" }
}

fn describe(notice: &Notice) -> String {
    match notice {
        Notice::Exp(n) => format!("EXP +{n}"),
        Notice::PartyBonus(n) => format!("party bonus +{n}"),
        Notice::Mesos { amount, shaped } => format!("mesos +{amount}{}", shape_mark(*shaped)),
        Notice::LateMesos { amount, shaped } => {
            format!("mesos +{amount} (late){}", shape_mark(*shaped))
        }
        Notice::Item { name, count, tab } => {
            let tab = tab.map_or("?".into(), |t| t.name().to_string());
            format!("\"{}\" x{count} ({tab}) [{}]", name.text, &name.key[..6])
        }
        Notice::Other => "other".into(),
    }
}

/// What one run brought.
#[derive(Default)]
struct Totals {
    frames: usize,
    confirmed_from: Option<String>,
    exp_lines: u64,
    hud_gain: u64,
    mesos: u64,
    mesos_lines: usize,
    /// Mesos read once their digits were learned.
    late_mesos: u64,
    late_lines: usize,
    party_bonus: u64,
    /// Item key: (text, pickups, count).
    items: BTreeMap<String, (String, usize, u64)>,
    other: usize,
    /// Confirmed frames whose EXP lines couldn't be compared: the HUD wasn't read in them.
    unchecked_frames: usize,
    missed: BTreeMap<String, Vec<String>>,
    unreadable: BTreeMap<String, usize>,
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    // The EXP bar digits learned over every run, for --warm.
    let warm = args.warm.then(|| {
        let mut hud = ExpBarReader::new();
        for run in &args.runs {
            let mut bar = 0;
            for path in frames_of(run) {
                if let Ok(image) = load(run, &path) {
                    let _ = hud.observe(&picture(image.frame(), &mut bar));
                }
            }
        }
        hud.knowledge()
    });
    if let Some(None) = warm {
        eprintln!("--warm: the EXP bar reader didn't confirm its digits on these runs.");
    }
    let from_file = args.knowledge.is_some();
    let mut notices = args.knowledge.clone().unwrap_or_default();
    let mut timings: Vec<u128> = Vec::new();
    for (i, run) in args.runs.iter().enumerate() {
        let name = run
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut hud = warm
            .as_ref()
            .and_then(|kept| kept.as_deref())
            .and_then(ExpBarReader::from_knowledge)
            .unwrap_or_default();
        // The Full Screen bar last measured, carried from frame to frame as the engine does.
        let mut bar = 0;
        if args.join {
            notices.reset();
        } else if i == 0 && from_file {
            // The reader the kept text made.
        } else if args.carry {
            notices = notices
                .knowledge()
                .and_then(|text| NoticeReader::from_knowledge(&text))
                .unwrap_or_default();
        } else {
            notices = NoticeReader::new();
        }
        let mut totals = Totals::default();
        let mut last_hud: Option<ExpReading> = None;
        for path in frames_of(run) {
            totals.frames += 1;
            let label = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let image = match load(run, &path) {
                Ok(i) => i,
                Err(e) => {
                    *totals.unreadable.entry(format!("load: {e}")).or_default() += 1;
                    notices.reset();
                    continue;
                }
            };
            let frame = picture(image.frame(), &mut bar);
            let reading = hud.observe(&frame);
            // The game fades to black when the map changes, as the engine's runner sees it.
            let dark = matches!(reading, Err(Unreadable::ScreenDark));
            let reading = reading.ok();
            let started = Instant::now();
            let result = match dark {
                true => Ok(notices.screen_dark()),
                false => notices.observe(&frame, reading.as_ref()),
            };
            timings.push(started.elapsed().as_micros());
            // The HUD's gain this frame, when it and the frame before were both read: the EXP
            // lines are compared with it over the same frames.
            let mut hud_gain = None;
            let hud_text = match reading {
                Some(r) if r.confirmed => {
                    let gain = match last_hud {
                        Some(before) if before.level == r.level && r.exp >= before.exp => {
                            Some(r.exp - before.exp)
                        }
                        _ => None,
                    };
                    last_hud = Some(r);
                    hud_gain = gain;
                    if let Some(g) = gain {
                        totals.hud_gain += g;
                    }
                    match gain {
                        Some(g) if g > 0 => format!("hud {} (+{g})", r.exp),
                        _ => format!("hud {}", r.exp),
                    }
                }
                Some(r) => {
                    last_hud = None;
                    format!("hud {} (unconfirmed)", r.exp)
                }
                None => {
                    last_hud = None;
                    "hud -".into()
                }
            };
            match result {
                Ok(r) => {
                    if r.confirmed {
                        totals.confirmed_from.get_or_insert(label.clone());
                    }
                    if r.confirmed && hud_gain.is_none() {
                        totals.unchecked_frames += 1;
                    }
                    for notice in &r.notices {
                        if !r.confirmed {
                            continue;
                        }
                        match notice {
                            Notice::Exp(n) if hud_gain.is_some() => totals.exp_lines += n,
                            Notice::PartyBonus(n) if hud_gain.is_some() => totals.party_bonus += n,
                            Notice::Exp(_) | Notice::PartyBonus(_) => {}
                            Notice::Mesos { amount: n, .. } => {
                                totals.mesos += n;
                                totals.mesos_lines += 1;
                            }
                            Notice::LateMesos { amount: n, .. } => {
                                totals.late_mesos += n;
                                totals.late_lines += 1;
                            }
                            Notice::Item { name, count, .. } => {
                                let entry = totals.items.entry(name.key.clone()).or_insert((
                                    name.text.clone(),
                                    0,
                                    0,
                                ));
                                entry.1 += 1;
                                entry.2 += u64::from(*count);
                            }
                            Notice::Other => totals.other += 1,
                        }
                    }
                    if let Some(m) = r.missed {
                        totals
                            .missed
                            .entry(format!("{m:?}"))
                            .or_default()
                            .push(label.clone());
                    }
                    if !args.quiet && (!r.notices.is_empty() || r.missed.is_some()) {
                        let lines: Vec<String> = r.notices.iter().map(describe).collect();
                        let missed = r
                            .missed
                            .map_or(String::new(), |m: Missed| format!("  missed: {m:?}"));
                        let mark = if r.confirmed { "" } else { "  (learning)" };
                        println!(
                            "{name}/{label}  {hud_text}  [{}]{missed}{mark}",
                            lines.join(", ")
                        );
                    }
                }
                Err(e) => {
                    *totals.unreadable.entry(format!("{e:?}")).or_default() += 1;
                    if !args.quiet {
                        println!("{name}/{label}  unreadable: {e:?}");
                    }
                }
            }
        }
        println!("\n== {name}: {} frames", totals.frames);
        match &totals.confirmed_from {
            Some(f) => println!("  confirmed from frame {f}"),
            None => println!("  never confirmed"),
        }
        println!(
            "  EXP lines {} + party bonus {} against the HUD's gain {}, in frames where the HUD \
             was read in it and the frame before ({} confirmed frames without the HUD left out)",
            totals.exp_lines, totals.party_bonus, totals.hud_gain, totals.unchecked_frames
        );
        println!("  mesos: {} in {} lines", totals.mesos, totals.mesos_lines);
        println!(
            "  mesos read late: {} in {} lines; {} amounts still waiting",
            totals.late_mesos,
            totals.late_lines,
            notices.waiting_mesos()
        );
        // The digits known for sure, from the text the reader keeps.
        let digits: String = notices
            .knowledge()
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once(' '))
            .filter_map(|(code, _)| char::from_u32(u32::from_str_radix(code, 16).ok()?))
            .filter(char::is_ascii_digit)
            .collect();
        println!("  digits known: {digits}");
        for (key, (text, picks, count)) in &totals.items {
            println!(
                "  item \"{text}\" [{}]: {picks} pickups, {count} in all",
                &key[..6]
            );
        }
        println!("  other lines: {}", totals.other);
        for (reason, frames) in &totals.missed {
            println!(
                "  missed ({reason}): {} frames: {}",
                frames.len(),
                frames.join(" ")
            );
        }
        for (reason, n) in &totals.unreadable {
            println!("  unreadable ({reason}): {n} frames");
        }
    }
    timings.sort_unstable();
    if !timings.is_empty() {
        let ms = |us: u128| us as f64 / 1000.0;
        println!(
            "\nreading time: median {:.2} ms, slowest {:.2} ms ({} frames)",
            ms(timings[timings.len() / 2]),
            ms(timings[timings.len() - 1]),
            timings.len()
        );
    }
    ExitCode::SUCCESS
}
