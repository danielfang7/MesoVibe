//! Replays private capture runs through the EXP bar reader, the way the engine will run it, and
//! checks its invariants: EXP never falls except at a level-up or a death, and the percent matches
//! EXP / EXP needed.
//!
//! The reader calibrates itself from the frames (nobody types anything), so the result counts
//! toward the read-rate bar below: no templates cut from screenshots. The captures come from the
//! capture tools (tools/capture, Windows and macOS) or `live_exp`, and stay outside the repo. Mac
//! captures are replayed at their backing scale, as the engine sees them live.
//!
//! ```text
//! cargo run -p mesovibe-engine --example replay_exp -- [--quiet] \
//!     [--needed <level>=<EXP for that whole level>]... \
//!     [--calibrate <run folder>/<frame number> <level> <exp> <percent, like 39.16>]... \
//!     <run folder>...
//! ```
//!
//! - `--needed` checks the percent against an EXP needed you read off the game's tooltip, for
//!   that level, independently of the reader's own EXP table.
//! - `--calibrate` uses the typed fallback instead of self-calibration.
//!
//! The summary ends with the read-rate count: frames from the first confirmed reading on, of
//! which at least 99.5% must read. Frames the reader reports as `ScreenDark` (the
//! game fading to black for a map change, with no HUD drawn) are left out by that rule; every other
//! unreadable frame, a frame that fails to load included, counts against it.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use mesovibe_engine::recognizers::exp_bar::{ExpBarReader, ExpReading, TypedReading, Unreadable};

#[path = "common/capture_run.rs"]
mod capture_run;
use capture_run::{frames_of, load};

fn parse_pct(text: &str) -> Option<u16> {
    let (whole, frac) = text.split_once('.')?;
    (frac.len() == 2).then_some(())?;
    whole
        .parse::<u16>()
        .ok()?
        .checked_mul(100)?
        .checked_add(frac.parse().ok()?)
}

struct Args {
    calibrations: Vec<(PathBuf, String, TypedReading)>,
    needed: HashMap<u16, u64>,
    quiet: bool,
    runs: Vec<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut out = Args {
        calibrations: Vec::new(),
        needed: HashMap::new(),
        quiet: false,
        runs: Vec::new(),
    };
    while let Some(a) = args.next() {
        let mut next = || args.next().ok_or(format!("{a} needs more values"));
        match a.as_str() {
            "--calibrate" => {
                let spec = next()?;
                let (run, frame) = spec
                    .rsplit_once(['/', '\\'])
                    .ok_or("--calibrate takes <run>/<frame>")?;
                let level = next()?.parse().map_err(|_| "level must be a number")?;
                let exp = next()?
                    .replace(',', "")
                    .parse()
                    .map_err(|_| "exp must be a number")?;
                let pct = parse_pct(&next()?).ok_or("percent looks like 39.16")?;
                let frame = format!(
                    "{:04}.png",
                    frame.parse::<u32>().map_err(|_| "frame is a number")?
                );
                let reading = TypedReading {
                    level,
                    exp,
                    pct_hundredths: pct,
                };
                out.calibrations.push((PathBuf::from(run), frame, reading));
            }
            "--needed" => {
                let spec = next()?;
                let (level, exp) = spec
                    .split_once('=')
                    .ok_or("--needed takes <level>=<exp>, like 41=324240")?;
                let level = level
                    .parse()
                    .map_err(|_| "--needed: level must be a number")?;
                let exp = exp
                    .replace(',', "")
                    .parse()
                    .map_err(|_| "--needed: exp must be a number")?;
                out.needed.insert(level, exp);
            }
            "--quiet" => out.quiet = true,
            _ => out.runs.push(PathBuf::from(a)),
        }
    }
    if out.runs.is_empty() {
        return Err("usage: replay_exp [--quiet] [--needed <level>=<exp>]... [--calibrate <run>/<frame> <level> <exp> <pct>]... <run>...".into());
    }
    Ok(out)
}

fn describe(r: &ExpReading) -> String {
    let level = r.level.map_or("?".to_string(), |l| l.to_string());
    let needed = r.needed.map_or(String::new(), |n| format!("  of {n}"));
    let mark = if r.confirmed { "" } else { "  (unconfirmed)" };
    format!(
        "level {level}  exp {}  {}.{:02}%{needed}{mark}",
        r.exp,
        r.pct_hundredths / 100,
        r.pct_hundredths % 100
    )
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let mut reader = ExpBarReader::new();
    for (run, frame, reading) in &args.calibrations {
        let image = match load(run, &run.join(frame)) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("calibration frame: {e}");
                return ExitCode::FAILURE;
            }
        };
        match reader.calibrate(&image.frame(), *reading) {
            Ok(()) => println!(
                "typed calibration on {}/{frame}: {reading:?}",
                run.display()
            ),
            Err(e) => {
                eprintln!("calibration on {}/{frame} refused: {e:?}", run.display());
                return ExitCode::FAILURE;
            }
        }
    }
    if args.calibrations.is_empty() {
        println!("self-calibrating: no typed readings");
    }

    let (mut total, mut confirmed, mut unconfirmed) = (0, 0, 0);
    // (frame label, frame number) of the first reading and the first confirmed one.
    let mut first_reading: Option<(String, usize)> = None;
    let mut first_confirmed: Option<(String, usize)> = None;
    let mut unreadable: BTreeMap<String, usize> = BTreeMap::new();
    let mut violations = Vec::new();
    // The read-rate bar: frames after calibration, how many of them read, and the screen-dark ones
    // left out.
    let (mut gate_frames, mut gate_read, mut gate_dark) = (0usize, 0usize, 0usize);
    // Microseconds `observe` took, by game area size (in game pixels) and scale.
    let mut timings: BTreeMap<(u32, u32, u32), Vec<u128>> = BTreeMap::new();
    for run in &args.runs {
        let mut previous: Option<ExpReading> = None;
        let name = run
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for path in frames_of(run) {
            total += 1;
            let label = format!(
                "{name}/{}",
                path.file_stem().unwrap_or_default().to_string_lossy()
            );
            let image = match load(run, &path) {
                Ok(i) => i,
                Err(e) => {
                    *unreadable.entry(format!("load error: {e}")).or_default() += 1;
                    gate_frames += usize::from(first_confirmed.is_some());
                    continue;
                }
            };
            let started = Instant::now();
            let result = reader.observe(&image.frame());
            timings
                .entry((
                    image.width / image.scale,
                    image.height / image.scale,
                    image.scale,
                ))
                .or_default()
                .push(started.elapsed().as_micros());
            match result {
                Ok(r) => {
                    if !args.quiet {
                        println!("{label}  {}", describe(&r));
                    }
                    first_reading.get_or_insert((label.clone(), total));
                    if !r.confirmed {
                        unconfirmed += 1;
                        gate_frames += usize::from(first_confirmed.is_some());
                        continue;
                    }
                    confirmed += 1;
                    first_confirmed.get_or_insert((label.clone(), total));
                    gate_frames += 1;
                    gate_read += 1;
                    if let Some(p) = previous
                        && r.exp < p.exp
                        && !matches!((p.level, r.level), (Some(a), Some(b)) if b > a)
                    {
                        violations.push(format!(
                            "{label}: EXP fell from {} to {} without a level-up (a death?)",
                            p.exp, r.exp
                        ));
                    }
                    if let Some(level) = r.level
                        && let Some(&needed) = args.needed.get(&level)
                    {
                        let expected = (r.exp * 20_000 + needed) / (2 * needed);
                        if expected != u64::from(r.pct_hundredths) {
                            violations.push(format!(
                                "{label}: {} / {needed} is {expected} hundredths, the bar says {}",
                                r.exp, r.pct_hundredths
                            ));
                        }
                    }
                    previous = Some(r);
                }
                Err(e) => {
                    if first_confirmed.is_some() {
                        match e {
                            Unreadable::ScreenDark => gate_dark += 1,
                            _ => gate_frames += 1,
                        }
                    }
                    let reason = match e {
                        Unreadable::UnsupportedSize { width, height } => {
                            format!("unsupported size {width}x{height}")
                        }
                        other => format!("{other:?}"),
                    };
                    if !args.quiet {
                        println!("{label}  unreadable: {reason}");
                    }
                    *unreadable.entry(reason).or_default() += 1;
                }
            }
        }
    }

    println!("\n{total} frames: {confirmed} read and confirmed, {unconfirmed} read unconfirmed");
    if let Some((label, n)) = &first_reading {
        println!("  first reading at frame {n} ({label})");
    }
    if let Some((label, n)) = &first_confirmed {
        println!("  confirmed from frame {n} ({label})");
    }
    for (reason, count) in &unreadable {
        println!("  {count} unreadable: {reason}");
    }
    println!(
        "  calibration: {:?}; still unseen: {:?}",
        reader.calibration(),
        reader.missing()
    );
    if gate_frames > 0 {
        let pct = gate_read as f64 * 100.0 / gate_frames as f64;
        let verdict = if gate_read * 1000 >= gate_frames * 995 {
            "meets"
        } else {
            "is below"
        };
        println!(
            "  read rate: {gate_read} of {gate_frames} frames read after calibration \
             ({pct:.2}%), which {verdict} the 99.5% bar; {gate_dark} screen-dark frames left out"
        );
    }
    for ((width, height, scale), micros) in &mut timings {
        micros.sort_unstable();
        let ms = |us: u128| us as f64 / 1000.0;
        println!(
            "  reading time at {width}x{height} (scale {scale}): median {:.2} ms, slowest {:.2} ms ({} frames)",
            ms(micros[micros.len() / 2]),
            ms(micros[micros.len() - 1]),
            micros.len()
        );
    }
    if violations.is_empty() {
        println!("invariants: no violations");
        ExitCode::SUCCESS
    } else {
        println!("invariants: {} violation(s)", violations.len());
        for v in &violations {
            println!("  {v}");
        }
        ExitCode::FAILURE
    }
}
