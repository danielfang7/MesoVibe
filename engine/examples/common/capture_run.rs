//! Loads private capture runs (PNG frames plus meta.json from the capture tools or `live_exp`)
//! as the engine's capture backends would hand them over. Shared by the replay examples; the
//! captures stay outside the repo.
//!
//! The engine reads sRGB, the colors the game draws. Mac runs from the capture helper are in the
//! display's color space (macOS's `screencapture` tags them Display P3 on a built-in Retina
//! screen), and so are `live_exp` recordings made on a Mac before 2026-11-04, when the engine's
//! stream began asking for sRGB. Those are converted to sRGB as they load.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mesovibe_engine::frame::Frame;

/// A capture frame's game area as BGRA, cropped using the run's meta.json, in captured pixels.
/// A run saved with --crop-bottom holds only the game area's bottom rows, from `first_row` down,
/// which is what the engine's capture backends read back too.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub first_row: u32,
    pub scale: u32,
    pub bgra: Vec<u8>,
}

impl Image {
    pub fn frame(&self) -> Frame<'_> {
        Frame::bottom(
            self.width,
            self.height,
            self.first_row,
            self.width as usize * 4,
            &self.bgra,
        )
        .and_then(|f| f.with_scale(self.scale))
        .expect("sized buffer")
    }
}

/// The macOS title bar above the game, in points (measured on macOS 26).
const MAC_TITLE_BAR: f64 = 28.0;

/// Where the game area sits in each image of a run, in captured pixels.
struct GameArea {
    x: i64,
    y: i64,
    width: u32,
    height: u32,
    /// Captured pixels per game pixel.
    scale: u32,
}

/// Saved by the macOS capture helper. Runs saved before the helper was renamed carry its earlier
/// name, so any `*-capture-macos` tool is the helper.
fn from_mac_helper(meta: &serde_json::Value) -> bool {
    meta["tool"]
        .as_str()
        .is_some_and(|tool| tool.ends_with("-capture-macos"))
}

fn game_area(meta: &serde_json::Value) -> Result<GameArea, String> {
    let num = |v: &serde_json::Value| v.as_f64().ok_or("meta.json: missing number".to_string());
    let window = &meta["window"];
    if from_mac_helper(meta) {
        // The macOS helper records the window's bounds in points, title bar included, and the
        // backing scale of its display. Full Screen windows fill a display and have no title bar.
        let bounds = &window["bounds"];
        let (w, h) = (num(&bounds["width"])?, num(&bounds["height"])?);
        let scale = meta["windowDisplayScale"].as_f64().unwrap_or(1.0);
        let fills_a_display = meta["displays"].as_array().is_some_and(|displays| {
            displays.iter().any(|d| {
                d["points"]["width"].as_f64() == Some(w)
                    && d["points"]["height"].as_f64() == Some(h)
            })
        });
        let title = if fills_a_display { 0.0 } else { MAC_TITLE_BAR };
        let px = |points: f64| (points * scale).round() as i64;
        return Ok(GameArea {
            x: 0,
            y: px(title),
            width: px(w) as u32,
            height: px(h - title) as u32,
            scale: scale.round() as u32,
        });
    }
    // The Windows tool and live_exp record the window and its game area in captured pixels.
    let (fx, fy) = (num(&window["frame"]["x"])?, num(&window["frame"]["y"])?);
    let client = &window["client"];
    Ok(GameArea {
        x: (num(&client["x"])? - fx) as i64,
        y: (num(&client["y"])? - fy) as i64,
        width: num(&client["width"])? as u32,
        height: num(&client["height"])? as u32,
        scale: meta["scale"].as_u64().unwrap_or(1) as u32,
    })
}

/// The color spaces runs come in.
#[derive(Clone, Copy, PartialEq)]
enum Colors {
    Srgb,
    DisplayP3,
}

/// A frame's color space: its PNG's cICP tag (ITU-T H.273 codes: primaries 1 is sRGB's, 12 is
/// Display P3's, both with sRGB's transfer curve, 13) or sRGB chunk, or else what the run's tool
/// is known to save.
fn colors_of(meta: &serde_json::Value, png: &png::Info) -> Result<Colors, String> {
    if png.srgb.is_some() {
        return Ok(Colors::Srgb);
    }
    if let Some(cicp) = &png.coding_independent_code_points {
        return match (cicp.color_primaries, cicp.transfer_function) {
            (1, 13) => Ok(Colors::Srgb),
            (12, 13) => Ok(Colors::DisplayP3),
            (primaries, transfer) => Err(format!(
                "colors tagged primaries {primaries}, transfer {transfer}: not sRGB or Display P3"
            )),
        };
    }
    match (meta["tool"].as_str(), meta["os"].as_str()) {
        _ if from_mac_helper(meta) => {
            Err("a Mac capture without a color tag: its colors can't be told".into())
        }
        // The engine's Mac stream handed over the display's colors until it asked for sRGB.
        (Some("live_exp"), Some("macos")) if meta["colorSpace"].is_null() => Ok(Colors::DisplayP3),
        _ => Ok(Colors::Srgb),
    }
}

/// sRGB's transfer curve, both ways, as tables: 8-bit values to linear light, and linear light in
/// 65,536 steps back to 8 bits (fine enough to round the darkest steps exactly).
fn curves() -> &'static ([f32; 256], Vec<u8>) {
    static CURVES: OnceLock<([f32; 256], Vec<u8>)> = OnceLock::new();
    CURVES.get_or_init(|| {
        let to_linear = std::array::from_fn(|v| {
            let c = v as f64 / 255.0;
            (if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }) as f32
        });
        let to_8bit = (0..=u16::MAX)
            .map(|step| {
                let l = f64::from(step) / f64::from(u16::MAX);
                let c = if l <= 0.003_130_8 {
                    l * 12.92
                } else {
                    1.055 * l.powf(1.0 / 2.4) - 0.055
                };
                (c * 255.0).round() as u8
            })
            .collect();
        (to_linear, to_8bit)
    })
}

/// A Display P3 color as sRGB: the same light, clamped to sRGB's gamut. Colors the game draws are
/// sRGB to begin with, so they come back as they were.
fn p3_to_srgb(rgb: [u8; 3]) -> [u8; 3] {
    // Linear Display P3 to linear sRGB; both use the D65 white point.
    const M: [[f32; 3]; 3] = [
        [1.224_940_1, -0.224_940_4, 0.0],
        [-0.042_056_9, 1.042_057_1, 0.0],
        [-0.019_637_6, -0.078_636_1, 1.098_273_5],
    ];
    let (to_linear, to_8bit) = curves();
    let l = rgb.map(|v| to_linear[usize::from(v)]);
    std::array::from_fn(|i| {
        let v = M[i][0] * l[0] + M[i][1] * l[1] + M[i][2] * l[2];
        to_8bit[(v.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as usize]
    })
}

pub fn load(run: &Path, png_path: &Path) -> Result<Image, String> {
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(run.join("meta.json"))
            .map_err(|e| format!("{}: {e}", run.display()))?,
    )
    .map_err(|e| format!("meta.json: {e}"))?;
    let GameArea {
        x: cx,
        y: cy,
        width: cw,
        height: ch,
        scale,
    } = game_area(&meta)?;

    let file = File::open(png_path).map_err(|e| format!("{}: {e}", png_path.display()))?;
    let mut reader = png::Decoder::new(BufReader::new(file))
        .read_info()
        .map_err(|e| format!("{}: {e}", png_path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: {e}", png_path.display()))?;
    let colors =
        colors_of(&meta, reader.info()).map_err(|e| format!("{}: {e}", png_path.display()))?;
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => {
            return Err(format!(
                "{}: unsupported PNG color type {other:?}",
                png_path.display()
            ));
        }
    };
    // A run saved with --crop-bottom holds only the bottom rows of the game area (more than asked
    // for above 768 lines, where the tool scales the strip with the game). Each PNG says how many:
    // a recording by `live_exp` saves the frames the engine grabbed whole, for the map's name.
    let (cx, cy, rows) = match meta["cropBottom"].as_u64() {
        Some(_) => (0, 0, info.height.min(ch)),
        None => (cx, cy, ch),
    };
    if cx < 0 || cy < 0 || cx as u32 + cw > info.width || cy as u32 + rows > info.height {
        return Err(format!(
            "{}: game area outside the image",
            png_path.display()
        ));
    }
    let mut bgra = vec![0u8; (cw * rows * 4) as usize];
    for y in 0..rows as usize {
        let row = &buf[(cy as usize + y) * info.line_size..];
        let out = &mut bgra[y * cw as usize * 4..];
        for x in 0..cw as usize {
            let p = &row[(cx as usize + x) * channels..];
            // Alpha matters on a Mac: a window's rounded corners are see-through.
            let alpha = if channels == 4 { p[3] } else { 255 };
            let [r, g, b] = match colors {
                Colors::Srgb => [p[0], p[1], p[2]],
                Colors::DisplayP3 => p3_to_srgb([p[0], p[1], p[2]]),
            };
            out[x * 4..x * 4 + 4].copy_from_slice(&[b, g, r, alpha]);
        }
    }
    Ok(Image {
        width: cw,
        height: ch,
        first_row: ch - rows,
        scale,
        bgra,
    })
}

pub fn frames_of(run: &Path) -> Vec<PathBuf> {
    let mut frames: Vec<PathBuf> = std::fs::read_dir(run)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "png"))
        .collect();
    frames.sort();
    frames
}
