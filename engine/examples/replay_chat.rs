//! Replays private capture runs through the chat reader.
//!
//! ```text
//! cargo run -p mesovibe-engine --example replay_chat -- [--dump <folder>] [--lines <folder>] <run folder>...
//! ```
//!
//! It prints what each frame brought. `--lines` saves each new line's picture as a PNG (text
//! white on black), named by frame and order; `--dump` writes the bottom of each frame's 1x
//! picture, for measuring.

use std::path::PathBuf;
use std::process::ExitCode;

use mesovibe_engine::recognizers::chat::{self, ChatReader, ChatUnreadable};
use mesovibe_engine::unstretch;

#[path = "common/capture_run.rs"]
mod capture_run;
use capture_run::{Scene, frames_of, load};

fn write_png(path: &std::path::Path, width: u32, height: u32, rgb: &[u8]) {
    let file = std::fs::File::create(path).expect("create the PNG");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("PNG header");
    writer.write_image_data(rgb).expect("PNG data");
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut dump: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut runs = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dump" => dump = args.next().map(PathBuf::from),
            "--lines" => save = args.next().map(PathBuf::from),
            _ => runs.push(PathBuf::from(a)),
        }
    }
    if runs.is_empty() {
        eprintln!("usage: replay_chat [--dump <folder>] [--lines <folder>] <run>...");
        return ExitCode::FAILURE;
    }
    let mut counts = std::collections::BTreeMap::new();
    let (mut gaps, mut unreadable, mut took) = (0, 0, std::time::Duration::ZERO);
    for run in &runs {
        let mut reader = ChatReader::new();
        let mut scene = Scene::default();
        for path in frames_of(run) {
            let image = match load(run, &path) {
                Ok(image) => image,
                Err(e) => {
                    eprintln!("{e}");
                    continue;
                }
            };
            // No EXP bar reader here: the picture's size is found from the HUD alone.
            let frame = scene.picture(image.frame());
            let frame = scene.look(frame);
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let started = std::time::Instant::now();
            let reading = reader.observe(&frame);
            took += started.elapsed();
            match &reading {
                Ok(r) => {
                    if r.gap {
                        gaps += 1;
                    }
                    let what: Vec<String> = r
                        .lines
                        .iter()
                        .map(|l| format!("{:?}{}", l.channel, if l.continues { "+" } else { "" }))
                        .collect();
                    // Where the box's top is, in rows up from the bottom: where the chat overlay sits.
                    let top = match r.place {
                        Some(place) => format!(" [box top {} up]", place.height - place.top),
                        None => " [box top not found]".to_string(),
                    };
                    println!(
                        "{name} {}{}{top}",
                        what.join(" "),
                        if r.gap { " (gap)" } else { "" }
                    );
                    for (k, line) in r.lines.iter().enumerate() {
                        *counts.entry(format!("{:?}", line.channel)).or_insert(0) += 1;
                        if let Some(dir) = &save {
                            std::fs::create_dir_all(dir).expect("lines folder");
                            let rgb: Vec<u8> =
                                line.coverage.iter().flat_map(|&c| [c, c, c]).collect();
                            let file = format!(
                                "{}-{k}-{:?}.png",
                                name.trim_end_matches(".png"),
                                line.channel
                            );
                            write_png(&dir.join(file), line.width, line.height, &rgb);
                        }
                    }
                }
                Err(e) => {
                    unreadable += 1;
                    let why = match e {
                        ChatUnreadable::NotFound => "no chat box".to_string(),
                        ChatUnreadable::ScrolledBack => "scrolled back".to_string(),
                        ChatUnreadable::Unsupported(n) => format!("{n:?}"),
                    };
                    println!("{name} unreadable: {why}");
                }
            }
            if let Some(dir) = &dump {
                // The rows the reader reads first, which a recording cropped to the bottom 300
                // rows of a 1080-line game still holds.
                let rows = chat::CHAT_ROWS;
                let started = std::time::Instant::now();
                // The 1x picture the reader sees: one pixel per block on a Retina Mac, the
                // stretch undone above 768 lines, the frame itself otherwise.
                let picture = unstretch::with_bottom_1x(&frame, rows, |p| {
                    let from = p.first_row().max(p.height().saturating_sub(rows));
                    let mut rgb = Vec::new();
                    for y in from..p.height() {
                        for x in 0..p.width() {
                            rgb.extend_from_slice(&p.rgb(x, y));
                        }
                    }
                    (p.width(), p.height() - from, rgb)
                });
                let took = started.elapsed();
                let Ok((width, height, rgb)) = picture else {
                    eprintln!("{}: no 1x picture", path.display());
                    continue;
                };
                std::fs::create_dir_all(dir).expect("dump folder");
                let name = path.file_name().unwrap();
                write_png(&dir.join(name), width, height, &rgb);
                println!("{} {:?}", name.to_string_lossy(), took);
            }
        }
    }
    println!(
        "lines: {counts:?}; gaps: {gaps}; unreadable frames: {unreadable}; reading took {took:?}"
    );
    ExitCode::SUCCESS
}
