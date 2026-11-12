//! Replays private capture runs through the map name reader and prints, for each run, every map
//! it found (its key and how many frames showed it, with its picture drawn in characters), why
//! the other frames couldn't be read, and how long a read took.
//!
//! The captures come from the capture tools (tools/capture, Windows and macOS) or `live_exp`, and
//! stay outside the repo. Only frames that hold the whole game area can show the minimap.
//!
//! ```text
//! cargo run -p mesovibe-engine --release --example replay_map -- <run folder>...
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use mesovibe_engine::recognizers::NamePicture;
use mesovibe_engine::recognizers::map_name::read;

#[path = "common/capture_run.rs"]
mod capture_run;
use capture_run::{Scene, frames_of, load};

/// The picture with a character per pixel: `#` for letter, `+` for part letter.
fn draw(picture: &NamePicture) -> String {
    picture
        .coverage
        .chunks(picture.width as usize)
        .map(|row| {
            let line: String = row
                .iter()
                .map(|&c| match c {
                    192.. => '#',
                    64.. => '+',
                    _ => ' ',
                })
                .collect();
            format!("    |{line}|\n")
        })
        .collect()
}

fn main() -> ExitCode {
    let runs: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if runs.is_empty() {
        eprintln!("usage: replay_map <run>...");
        return ExitCode::FAILURE;
    }
    let mut timings: Vec<u128> = Vec::new();
    for run in &runs {
        // Key: (frames, first frame, picture).
        let mut maps: BTreeMap<String, (usize, String, NamePicture)> = BTreeMap::new();
        let mut unreadable: BTreeMap<String, usize> = BTreeMap::new();
        let frames = frames_of(run);
        // The Full Screen bar and picture size, carried from frame to frame as the engine does.
        let mut scene = Scene::default();
        for path in &frames {
            let label = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let image = match load(run, path) {
                Ok(image) => image,
                Err(e) => {
                    *unreadable.entry(format!("load: {e}")).or_default() += 1;
                    continue;
                }
            };
            let started = Instant::now();
            // No EXP bar reader here: the picture's size is found from the HUD alone.
            let frame = scene.picture(image.frame());
            let frame = scene.look(frame);
            let result = read(&frame);
            timings.push(started.elapsed().as_micros());
            match result {
                Ok(name) => maps.entry(name.key).or_insert((0, label, name.picture)).0 += 1,
                Err(why) => *unreadable.entry(format!("{why:?}")).or_default() += 1,
            }
        }
        println!("== {}: {} frames", run.display(), frames.len());
        for (key, (count, first, picture)) in &maps {
            println!("  map [{}]: {count} frames, first {first}", &key[..6]);
            print!("{}", draw(picture));
        }
        for (why, count) in &unreadable {
            println!("  unreadable ({why}): {count} frames");
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
