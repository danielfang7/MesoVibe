//! Replays private capture runs through the meso reader, the way the engine runs it on whole
//! frames, and reports what it read: totals and when they change, what each frame showed, the
//! read rate over frames with a box fully in view, changes that look wrong, and how long each
//! frame took.
//!
//! The reader names the digits from the frames itself (nobody types anything), so a run read
//! from scratch shows how soon it reads too. The captures come from the capture tools
//! (tools/capture, Windows and macOS) and stay outside the repo. Mac captures are replayed at
//! their backing scale and read one pixel from each block, where the EXP bar reader found the
//! blocks start, as the engine does live; each such frame is also undone whole (with the
//! allowance in `common/capture_run.rs` for the helper's title-bar rows) and read again, and any
//! frame where the two differ is reported.
//!
//! ```text
//! cargo run -p mesovibe-engine --release --example replay_mesos -- [--quiet] \
//!     [--knowledge <meso-total.txt>] [--keep <file>] <run folder>...
//! ```
//!
//! - `--knowledge` starts from a `meso-total.txt` the app kept, as the app starts a session.
//! - `--keep` writes what the reader named by the end, as the app keeps it, for another replay's
//!   `--knowledge`.
//! - `--quiet` leaves out the line per frame.
//!
//! Runs are replayed in the order given, as one session, so name them in the order they were
//! recorded. Every frame is a whole frame here; the engine reads one every frame while a box is
//! in view, and about once a second otherwise.
//!
//! The read rate counts the frames where a meso box was found whole and confirmed as a window's
//! (read, or showing shapes not named yet); frames with the cursor or a tooltip over the box are
//! covered and listed apart. A change looks wrong when a total read in a window changes by more
//! digits than mesos picked up or a sale plausibly makes, or comes back to the one before it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use mesovibe_engine::events::MesoSource;
use mesovibe_engine::recognizers::exp_bar::ExpBarReader;
use mesovibe_engine::recognizers::meso_total::{MesoReader, MesoView};
use mesovibe_engine::unstretch;

#[path = "common/capture_run.rs"]
mod capture_run;
use capture_run::{Scene, frames_of, load, whole_1x};

struct Args {
    knowledge: Option<(String, MesoReader)>,
    keep: Option<String>,
    quiet: bool,
    runs: Vec<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut out = Args {
        knowledge: None,
        keep: None,
        quiet: false,
        runs: Vec::new(),
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--knowledge" => {
                let path = args.next().ok_or("--knowledge needs a file")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                let reader = MesoReader::from_knowledge(&text)
                    .ok_or(format!("{path}: not text a meso reader kept"))?;
                out.knowledge = Some((path, reader));
            }
            "--keep" => out.keep = Some(args.next().ok_or("--keep needs a file")?),
            "--quiet" => out.quiet = true,
            _ => out.runs.push(PathBuf::from(a)),
        }
    }
    if out.runs.is_empty() {
        return Err(
            "usage: replay_mesos [--quiet] [--knowledge <meso-total.txt>] [--keep <file>] <run>..."
                .into(),
        );
    }
    Ok(out)
}

fn source_name(source: MesoSource) -> &'static str {
    match source {
        MesoSource::Inventory => "inventory",
        MesoSource::Shop => "shop",
        MesoSource::Storage => "storage",
    }
}

fn describe(view: &MesoView) -> String {
    match view {
        MesoView::Read { source, total } => format!("{} {total}", source_name(*source)),
        MesoView::Learning(source) => format!("{} learning", source_name(*source)),
        other => format!("{other:?}"),
    }
}

/// A short name for the kind of view, for the counts.
fn kind(view: &MesoView) -> &'static str {
    match view {
        MesoView::NotOpen => "not open",
        MesoView::Covered => "covered",
        MesoView::OffScreen => "partly off screen",
        MesoView::PartlyCovered => "partly covered",
        MesoView::Learning(_) => "learning",
        MesoView::Read { .. } => "read",
        MesoView::Unsupported => "unsupported size",
    }
}

/// Whether a change between two totals read looks wrong: more than 10% either way, or a
/// different number of digits for less than a tenth of the total.
fn looks_wrong(before: u64, now: u64) -> bool {
    let diff = before.abs_diff(now);
    diff * 10 > before.max(now)
        || (before.to_string().len() != now.to_string().len() && diff * 10 < now)
}

fn ms(micros: u128) -> f64 {
    micros as f64 / 1000.0
}

/// Median and slowest, in milliseconds.
fn spread(micros: &mut [u128]) -> Option<(f64, f64)> {
    micros.sort_unstable();
    Some((ms(*micros.get(micros.len() / 2)?), ms(*micros.last()?)))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut reader = match &args.knowledge {
        Some((path, kept)) => {
            println!("starting from the shapes the app kept in {path}");
            kept.clone()
        }
        None => {
            println!("naming the digits from the frames: nothing kept, nothing typed");
            MesoReader::new()
        }
    };
    // The same frames undone whole, read by a reader of their own, as a check on reading a
    // Retina frame at its blocks.
    let mut at_1x = reader.clone();
    let mut exp = ExpBarReader::new();
    let mut scene = Scene::default();

    let mut total_frames = 0usize;
    let mut views: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut in_view, mut read) = (0usize, 0usize);
    let mut sent = Vec::new();
    let mut wrong = Vec::new();
    let mut mismatches = Vec::new();
    let mut dropped = 0usize;
    // Rises in a shop checked against the buy-back rows they added: agreed, disagreed.
    let (mut sales_agreed, mut sales_disagreed) = (0usize, 0usize);
    let mut first_read: Option<String> = None;
    // Microseconds per frame: the meso reader, undoing the whole frame, and copying it.
    let mut timings: BTreeMap<(u32, u32, u32), Vec<u128>> = BTreeMap::new();
    let (mut unscale_us, mut copy_us) = (Vec::new(), Vec::new());
    let mut load_errors = 0usize;

    for run in &args.runs {
        let name = run
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut run_views: BTreeMap<&str, usize> = BTreeMap::new();
        let mut run_totals: Vec<(String, MesoSource, u64)> = Vec::new();
        let mut run_sent: Vec<String> = Vec::new();
        for path in frames_of(run) {
            total_frames += 1;
            let frame_name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let label = format!("{name}/{frame_name}");
            let image = match load(run, &path) {
                Ok(i) => i,
                Err(e) => {
                    load_errors += 1;
                    println!("{label}  load error: {e}");
                    continue;
                }
            };
            let frame = scene.picture(image.frame());
            if scene.changed() {
                exp.recheck();
                reader.recheck();
                at_1x.recheck();
            }
            let hud = exp.observe_hud(&frame);
            scene.follow(&frame, &hud.exp);

            let started = Instant::now();
            let reading = reader.observe(&frame, exp.block_offset());
            let took = started.elapsed().as_micros();
            let scale = frame.scale();
            timings
                .entry((frame.width() / scale, frame.height() / scale, scale))
                .or_default()
                .push(took);
            let started = Instant::now();
            let copied = image.bgra.clone();
            copy_us.push(started.elapsed().as_micros());
            drop(copied);

            if scale > 1 {
                // What undoing the whole frame would cost the engine, below the window's edge
                // rows (which aren't whole blocks): reading at the blocks avoids it.
                let started = Instant::now();
                let undone = frame
                    .without_rows(2 * scale, 0)
                    .and_then(|f| unstretch::unscale(&f));
                let took = started.elapsed().as_micros();
                if undone.is_some() {
                    unscale_us.push(took);
                }
                if let Some(whole) = whole_1x(&image) {
                    let other = at_1x.observe(&whole.frame().with_source(frame.source()), None);
                    if other.map(|r| r.view) != reading.map(|r| r.view) {
                        mismatches.push(format!(
                            "{label}: at its blocks {:?}, undone whole {:?}",
                            reading.map(|r| r.view),
                            other.map(|r| r.view)
                        ));
                    }
                }
            }

            let had_names = reader.knowledge().is_some();
            let Some(reading) = reading else {
                continue;
            };
            if had_names && reader.knowledge().is_none() {
                dropped += 1;
                println!("{label}  the naming was dropped and is learned again");
            }
            *views.entry(kind(&reading.view)).or_default() += 1;
            *run_views.entry(kind(&reading.view)).or_default() += 1;
            match reading.view {
                MesoView::Read { source, total } => {
                    in_view += 1;
                    read += 1;
                    first_read.get_or_insert(label.clone());
                    let changed = run_totals
                        .last()
                        .is_none_or(|(_, s, t)| (*s, *t) != (source, total));
                    if changed {
                        if let Some((at, s, before)) = run_totals.last()
                            && *s == source
                            && looks_wrong(*before, total)
                        {
                            wrong.push(format!("{label}: {before} ({at}) to {total}"));
                        }
                        if let Some(i) = run_totals.len().checked_sub(2)
                            && run_totals[i].1 == source
                            && run_totals[i].2 == total
                        {
                            wrong.push(format!("{label}: back to {total} ({})", run_totals[i].0));
                        }
                        run_totals.push((frame_name.clone(), source, total));
                    }
                }
                MesoView::Learning(_) => in_view += 1,
                _ => {}
            }
            match reading.sale {
                Some(true) => sales_agreed += 1,
                Some(false) => sales_disagreed += 1,
                None => {}
            }
            if let Some((total, source)) = reading.send {
                run_sent.push(format!("{frame_name} {} {total}", source_name(source)));
                sent.push((label.clone(), source, total));
            }
            if !args.quiet {
                let send = match reading.send {
                    Some((total, source)) => {
                        format!("  -> MesoChanged {} {total}", source_name(source))
                    }
                    None => String::new(),
                };
                let sale = match reading.sale {
                    Some(true) => "  (the rise is the buy-back rows' amount)",
                    Some(false) => "  (the rise disagrees with the buy-back rows)",
                    None => "",
                };
                println!("{label}  {}{send}{sale}", describe(&reading.view));
            }
        }
        let counts: Vec<String> = run_views.iter().map(|(k, n)| format!("{n} {k}")).collect();
        println!("== {name}: {}", counts.join(", "));
        if !run_totals.is_empty() {
            let totals: Vec<String> = run_totals
                .iter()
                .map(|(f, s, t)| format!("{t} ({} from {f})", source_name(*s)))
                .collect();
            println!("   totals read: {}", totals.join(", "));
        }
        if !run_sent.is_empty() {
            println!("   sent: {}", run_sent.join(", "));
        }
    }

    println!("\n{total_frames} frames ({load_errors} failed to load)");
    let counts: Vec<String> = views.iter().map(|(k, n)| format!("{n} {k}")).collect();
    println!("  views: {}", counts.join(", "));
    if let Some(label) = &first_read {
        println!("  first total read at {label}");
    }
    println!(
        "  read rate: {read} of {in_view} frames with a box fully in view ({:.1}%); {} covered (the cursor, a tooltip or a window over the box)",
        if in_view > 0 {
            read as f64 * 100.0 / in_view as f64
        } else {
            0.0
        },
        views.get("covered").copied().unwrap_or(0)
    );
    println!("  MesoChanged sent: {}", sent.len());
    println!(
        "  shop rises checked against their buy-back rows: {sales_agreed} agreed, {sales_disagreed} disagreed"
    );
    println!("  naming dropped by a shop sale or conflicting shapes: {dropped}");
    println!(
        "  shapes named: {}",
        reader
            .knowledge()
            .map_or(0, |k| k.lines().count().saturating_sub(1))
    );
    for ((width, height, scale), micros) in &mut timings {
        if let Some((median, slowest)) = spread(micros) {
            println!(
                "  meso reader at {width}x{height} (scale {scale}): median {median:.3} ms, slowest {slowest:.3} ms per whole frame ({} frames, PNG decode left out)",
                micros.len()
            );
        }
    }
    if let Some((median, slowest)) = spread(&mut unscale_us) {
        println!(
            "  for comparison, the engine undoing a Retina frame whole: median {median:.2} ms, slowest {slowest:.2} ms"
        );
    }
    if let Some((median, slowest)) = spread(&mut copy_us) {
        println!(
            "  copying a whole frame's pixels in memory: median {median:.2} ms, slowest {slowest:.2} ms (the CPU side of a readback; the GPU's part isn't measured here)"
        );
    }
    if let (Some(path), Some(text)) = (&args.keep, reader.knowledge())
        && let Err(e) = std::fs::write(path, text)
    {
        eprintln!("{path}: {e}");
    }
    let mut failed = false;
    if mismatches.is_empty() {
        println!("Retina check: reading at the blocks matched the frames undone whole");
    } else {
        failed = true;
        println!("Retina check: {} frame(s) differ", mismatches.len());
        for m in &mismatches {
            println!("  {m}");
        }
    }
    if wrong.is_empty() {
        println!("wrong-looking changes: none");
    } else {
        failed = true;
        println!("wrong-looking changes: {}", wrong.len());
        for w in &wrong {
            println!("  {w}");
        }
    }
    match failed {
        true => ExitCode::FAILURE,
        false => ExitCode::SUCCESS,
    }
}
