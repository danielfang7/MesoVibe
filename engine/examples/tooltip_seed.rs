//! Draws the tooltip reader's starting letter shapes (engine/data/tooltip-seed.txt) from Liberation
//! Sans Bold, an openly licensed font close to the one the game draws item names in. The reader
//! takes them as guesses: they let it name an item before the game has shown each letter, and
//! nothing in them comes from the game. `node scripts/tooltip-seed.mjs` downloads and checks the
//! font, then runs this:
//!
//! ```text
//! cargo run -p mesovibe-engine --release --example tooltip_seed -- <LiberationSans-Bold.ttf> \
//!     <out> [--size <pixels per em>] [--baseline <row>] [--threshold <share>]
//! ```
//!
//! Each letter is drawn white the way the game draws a name, at 8 places between two pixels
//! (the game places letters at fractions of a pixel, so one letter comes out a pixel wider or
//! narrower from one name to the next), and cut the way the reader cuts a name: a pixel is ink
//! where the letter covers at least `--threshold` of it, and the letter is its inked columns in
//! the 15-row band the reader cuts. The defaults are the settings whose letters came closest to
//! the letters the reader learned from recorded tooltips on a Mac and on Windows.

use std::collections::BTreeSet;
use std::process::ExitCode;

use ab_glyph::{Font, FontRef, PxScale, point};
use mesovibe_engine::glyph::Glyph;

/// The reader's name band: 15 rows, from 6 above the blue dot's centre row.
const ROWS: usize = 15;
/// Wide enough for any letter at this size, with room on the left for the fractional offsets.
const COLUMNS: usize = 40;
const LEFT: f32 = 4.0;
/// The places between two pixels each letter is drawn at.
const PHASES: u32 = 8;

/// The size, the baseline's place in the band and the share of a pixel that makes it ink. With
/// these the letters land where the game's do (capitals on rows 2 to 11, small letters from row
/// 4, tails down to row 14), and they came closest to the game's letters with the fewest of them
/// nearer another letter's shapes.
const SIZE: f32 = 13.5;
const BASELINE: f32 = 11.625;
const THRESHOLD: f32 = 0.5;

/// The font this is meant for, as the header says where the shapes come from.
const FONT_NAME: &str = "Liberation Sans Bold 2.1.5";
const COPYRIGHT: [&str; 2] = [
    "Digitized data copyright (c) 2010 Google Corporation, with Reserved Font Arimo, Tinos and Cousine.",
    "Copyright (c) 2012 Red Hat, Inc., with Reserved Font Name Liberation.",
];

struct Args {
    font: String,
    out: String,
    size: f32,
    baseline: f32,
    threshold: f32,
}

fn parse_args() -> Result<Args, String> {
    let usage = "usage: tooltip_seed <font.ttf> <out> [--size <px>] [--baseline <row>] [--threshold <share>]";
    let mut args = std::env::args().skip(1);
    let mut paths = Vec::new();
    let (mut size, mut baseline, mut threshold) = (SIZE, BASELINE, THRESHOLD);
    while let Some(a) = args.next() {
        let mut number = |what: &str| -> Result<f32, String> {
            let text = args.next().ok_or(format!("{what} needs a number"))?;
            text.parse()
                .map_err(|_| format!("{what}: not a number: {text}"))
        };
        match a.as_str() {
            "--size" => size = number("--size")?,
            "--baseline" => baseline = number("--baseline")?,
            "--threshold" => threshold = number("--threshold")?,
            _ => paths.push(a),
        }
    }
    let [font, out] = <[String; 2]>::try_from(paths).map_err(|_| usage.to_string())?;
    Ok(Args {
        font,
        out,
        size,
        baseline,
        threshold,
    })
}

/// How much of each pixel of the band `c` covers, drawn `phase` of a pixel right of a pixel's edge.
fn coverage(
    font: &FontRef,
    scale: PxScale,
    c: char,
    phase: f32,
    baseline: f32,
) -> Vec<[f32; COLUMNS]> {
    let mut band = vec![[0.0; COLUMNS]; ROWS];
    let glyph = font
        .glyph_id(c)
        .with_scale_and_position(scale, point(LEFT + phase, baseline));
    if let Some(outline) = font.outline_glyph(glyph) {
        let bounds = outline.px_bounds();
        outline.draw(|x, y, c| {
            let (x, y) = (
                bounds.min.x as i32 + x as i32,
                bounds.min.y as i32 + y as i32,
            );
            if (0..COLUMNS as i32).contains(&x) && (0..ROWS as i32).contains(&y) {
                band[y as usize][x as usize] = c;
            }
        });
    }
    band
}

/// The piece the reader would cut from the band: `None` when there's no ink, or when blank
/// columns split it (the reader would read two pieces, as for a double quote).
fn cut(band: &[[f32; COLUMNS]], threshold: f32) -> Option<Glyph> {
    let inked: Vec<usize> = (0..COLUMNS)
        .filter(|&x| band.iter().any(|row| row[x] >= threshold))
        .collect();
    let (&first, &last) = (inked.first()?, inked.last()?);
    if last - first + 1 != inked.len() {
        return None;
    }
    let rows = band
        .iter()
        .map(|row| {
            (first..=last).fold(0u32, |m, x| {
                if row[x] >= threshold {
                    m | 1 << (x - first)
                } else {
                    m
                }
            })
        })
        .collect();
    Glyph::from_rows((last - first + 1) as u32, rows)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let bytes = match std::fs::read(&args.font) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}: {e}", args.font);
            return ExitCode::FAILURE;
        }
    };
    let Ok(font) = FontRef::try_from_slice(&bytes) else {
        eprintln!("{}: not a font", args.font);
        return ExitCode::FAILURE;
    };
    // ab_glyph scales a font by its height (ascent to descent); the size given is pixels per em.
    let Some(per_em) = font.units_per_em() else {
        eprintln!("{}: no units per em", args.font);
        return ExitCode::FAILURE;
    };
    let scale = PxScale::from(args.size * font.height_unscaled() / per_em);
    let mut lines = BTreeSet::new();
    let mut skipped = Vec::new();
    // Every printable ASCII character but capital I, which the game draws like small l and the
    // reader reads as one, and the curly apostrophe a few item names use.
    for c in ('!'..='~').filter(|&c| c != 'I').chain(['’']) {
        let shapes: BTreeSet<String> = (0..PHASES)
            .filter_map(|p| {
                let band = coverage(&font, scale, c, p as f32 / PHASES as f32, args.baseline);
                cut(&band, args.threshold)
            })
            .map(|g| g.encode())
            .collect();
        if shapes.is_empty() {
            skipped.push(c);
        }
        lines.extend(shapes.into_iter().map(|g| format!("{c}\t{g}")));
    }
    let header = [
        "# The tooltip reader's starting letter shapes: guesses at the game's name font, drawn from".to_string(),
        format!("# {FONT_NAME} by examples/tooltip_seed.rs ({} px per em, baseline at row {}, ink from {} cover).", args.size, args.baseline, args.threshold),
        format!("# {}", COPYRIGHT[0]),
        format!("# {}", COPYRIGHT[1]),
        "# Licensed under the SIL Open Font License 1.1: see LiberationSans-OFL.txt beside this file.".to_string(),
        "# Each line: a character, a tab, then its shape (width:rows, each row's ink as a hex bit mask).".to_string(),
    ];
    let text = header
        .into_iter()
        .chain(lines.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    if let Err(e) = std::fs::write(&args.out, text) {
        eprintln!("{}: {e}", args.out);
        return ExitCode::FAILURE;
    }
    println!(
        "{}: {} shapes; none for {:?}",
        args.out,
        lines.len(),
        skipped.iter().collect::<String>()
    );
    ExitCode::SUCCESS
}
