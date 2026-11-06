//! Undoes the game's stretch and a Mac's pixel doubling, so readers see the game's own 1x pixels
//! at every size and on every display.
//!
//! **Pixel doubling ([`unscale`]).** A Retina Mac shows each game pixel as a 2x2 block of identical
//! screen pixels: no smoothing, so taking one pixel from each block gives the game's picture back
//! exactly (checked pixel by pixel on Mac captures). Wine places the game up to a point off
//! where the window's bounds put it, and half a point is one pixel, so where the blocks start is
//! read from the pixels. The stretch below then applies to the recovered picture as on Windows.
//!
//! **The stretch ([`bottom_rows`]).**
//! Up to 768 lines tall the game draws at its own pixel size. Above that it draws a 768-line
//! picture and stretches it to fill the game area with bilinear filtering: a 1228x768 picture at
//! 1920x1200, and 1366x768 at 2560x1440 and in Full Screen there. A bilinear stretch is linear
//! and its geometry is known, so least squares undoes it. On recorded captures the recovered HUD
//! is within 1.2 (out of 255) of the same HUD at 1x, and every HUD glyph comes out exactly as it
//! is at 1x. So the same glyph shapes, learned once, read every size.
//!
//! Each direction is solved on its own (the stretch is separable), and each solve is a
//! tridiagonal system, because every stretched pixel mixes two neighbouring 1x pixels.

use crate::frame::Frame;

/// The tallest game area the game draws at 1x. Above it the game stretches a picture this tall.
pub const NATIVE_HEIGHT: u32 = 768;

/// Extra 1x rows solved above the ones asked for and then dropped: the rows nearest the top of
/// the solved band are the least constrained.
const GUARD_ROWS: usize = 2;

/// The width of the picture the game stretches to fill a game area taller than 768 lines: the
/// nearest even width with the same shape, which keeps the centered HUD on whole pixels.
/// Measured: 1228 at 1920x1200 (1228.8 by shape), 1366 at 2560x1440 (1365.3).
pub fn picture_width(width: u32, height: u32) -> u32 {
    if height == 0 {
        return 0;
    }
    let half_width = (u64::from(width) * u64::from(NATIVE_HEIGHT / 2) + u64::from(height / 2))
        / u64::from(height);
    (2 * half_width) as u32
}

/// Pixels owned in the layout [`Frame`] reads: rows `first_row..height` of a picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    width: u32,
    height: u32,
    first_row: u32,
    bgra: Vec<u8>,
}

impl Picture {
    pub fn frame(&self) -> Frame<'_> {
        Frame::bottom(
            self.width,
            self.height,
            self.first_row,
            self.width as usize * 4,
            &self.bgra,
        )
        .expect("a picture's buffer fits its size")
    }
}

/// How many captured pixel rows a frame at `scale` must hold for [`unscale`] to recover `rows`
/// whole game rows, wherever the blocks start: what a capture backend reads back.
pub fn scaled_rows(rows: u32, scale: u32) -> u32 {
    rows.saturating_mul(scale)
        .saturating_add(scale.saturating_sub(1))
}

/// The game's own picture behind a frame that declares a backing scale above 1: one pixel from
/// each block, keeping the frame's bottom-strip shape in game rows. `None` when the frame is at
/// scale 1, when it holds no whole block, or when no placement of the blocks makes every block
/// one color in both directions (something scaled the game another way, which isn't supported).
pub fn unscale(frame: &Frame) -> Option<Picture> {
    let scale = frame.scale();
    if scale < 2 {
        return None;
    }
    let (px, py) = (phase(frame, false)?, phase(frame, true)?);
    let (width, height) = (frame.width() / scale, frame.height() / scale);
    // The first game row whose block starts inside the rows the frame holds.
    let first_row = frame.first_row().saturating_sub(py).div_ceil(scale);
    let mut bgra = Vec::with_capacity(((height - first_row) * width * 4) as usize);
    for k in first_row..height {
        for c in 0..width {
            bgra.extend_from_slice(&frame.bgra(px + scale * c, py + scale * k));
        }
    }
    Some(Picture {
        width,
        height,
        first_row,
        bgra,
    })
}

/// Where the blocks start along one direction, in captured pixels: the first offset at which
/// every whole block the frame holds is one color. If two offsets both fit, the frame is one
/// color along that direction and either gives the same picture.
///
/// Pixels at the window's own see-through edges aren't the game's and aren't compared: a Mac
/// window's rounded corners are partly transparent (alpha below 255), and the opaque pixels next
/// to them can be off by one where macOS blends the corner in (seen in a recorded run).
fn phase(frame: &Frame, vertical: bool) -> Option<u32> {
    let (scale, width, first_row) = (frame.scale(), frame.width(), frame.first_row());
    let rows = frame.height() - first_row;
    let index = |x: u32, y: u32| ((y - first_row) * width + x) as usize;
    // Pixels within one pixel of a see-through one. Those are rare, so mark around each.
    let mut edge = vec![false; (rows * width) as usize];
    for y in first_row..frame.height() {
        for x in 0..width {
            if frame.bgra(x, y)[3] < 255 {
                for yy in y.saturating_sub(1).max(first_row)..(y + 2).min(frame.height()) {
                    for xx in x.saturating_sub(1)..(x + 2).min(width) {
                        edge[index(xx, yy)] = true;
                    }
                }
            }
        }
    }
    let (along, across) = match vertical {
        true => (first_row..frame.height(), 0..width),
        false => (0..width, first_row..frame.height()),
    };
    // (x, y) of the pixel `a` along and `b` across.
    let xy = |a: u32, b: u32| if vertical { (b, a) } else { (a, b) };
    (0..scale).find(|&p| {
        let mut compared = false;
        let mut start = along.start + (p + scale - along.start % scale) % scale;
        while start + scale <= along.end {
            for b in across.clone() {
                let pixel = |k: u32| xy(start + k, b);
                if (0..scale).any(|k| {
                    let (x, y) = pixel(k);
                    edge[index(x, y)]
                }) {
                    continue;
                }
                let (x0, y0) = pixel(0);
                let first = frame.bgra(x0, y0);
                if (1..scale).any(|k| {
                    let (x, y) = pixel(k);
                    frame.bgra(x, y) != first
                }) {
                    return false;
                }
                compared = true;
            }
            start += scale;
        }
        compared
    })
}

/// The vertical inverse that recovers the bottom `rows` 1x rows of a frame `height` tall.
fn vertical(height: u32, rows: u32) -> Option<Inverse> {
    if height <= NATIVE_HEIGHT || rows == 0 || rows > NATIVE_HEIGHT {
        return None;
    }
    let first_row = (NATIVE_HEIGHT - rows) as usize;
    Inverse::new(
        height as usize,
        NATIVE_HEIGHT as usize,
        first_row,
        GUARD_ROWS,
    )
}

/// How many of a stretched frame's bottom rows [`bottom_rows`] reads to recover `rows` 1x rows:
/// what a capture backend must read back. `None` when a frame `height` tall isn't stretched, or
/// `rows` is 0 or more than 768.
pub fn rows_needed(height: u32, rows: u32) -> Option<u32> {
    let first = vertical(height, rows)?.taps.first()?.0;
    Some(height - first as u32)
}

/// The bottom `rows` rows of the 1x picture behind a stretched frame, at the picture's full
/// width, as the bottom strip of that 768-line picture (rows `768 - rows` to 768), so a reader
/// measures up from the game's own height, as it does in an unscaled picture. `None` when the
/// frame isn't stretched (768 lines or fewer), when `rows` is 0 or more than 768, when the
/// picture would be narrower than one pixel, or when the frame doesn't hold the bottom
/// [`rows_needed`] rows the recovery reads (all it reads).
pub fn bottom_rows(frame: &Frame, rows: u32) -> Option<Picture> {
    let (width, height) = (frame.width(), frame.height());
    let picture_width = picture_width(width, height);
    if picture_width == 0 {
        return None;
    }
    let vertical = vertical(height, rows)?;
    if vertical
        .taps
        .first()
        .is_none_or(|&(j, ..)| j < frame.first_row() as usize)
    {
        return None;
    }
    let horizontal = Inverse::new(width as usize, picture_width as usize, 0, 0)?;
    let (width, rows, picture_width) = (width as usize, rows as usize, picture_width as usize);
    let dropped = vertical.unknowns - rows;

    // Vertical pass: for each frame column, the 1x rows. `columns[(row * width + x) * 3 + c]`.
    let mut columns = vec![0f64; rows * width * 3];
    let mut solved = vec![0f64; vertical.unknowns];
    for x in 0..width {
        for c in 0..3 {
            vertical.solve(|j| f64::from(frame.rgb(x as u32, j as u32)[c]), &mut solved);
            for (row, value) in solved[dropped..].iter().enumerate() {
                columns[(row * width + x) * 3 + c] = *value;
            }
        }
    }

    // Horizontal pass: for each 1x row, the 1x columns.
    let mut bgra = vec![255u8; rows * picture_width * 4];
    let mut solved = vec![0f64; horizontal.unknowns];
    for row in 0..rows {
        for c in 0..3 {
            horizontal.solve(|j| columns[(row * width + j) * 3 + c], &mut solved);
            for (x, value) in solved.iter().enumerate() {
                // RGB in, BGRA out.
                bgra[(row * picture_width + x) * 4 + 2 - c] = value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Some(Picture {
        width: picture_width as u32,
        height: NATIVE_HEIGHT,
        first_row: NATIVE_HEIGHT - rows as u32,
        bgra,
    })
}

/// Why a frame has no 1x picture to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum No1x {
    /// A backing scale whose blocks aren't one color (something scaled the game another way).
    Scale(u32),
    /// A stretched frame whose picture can't be recovered.
    Stretch,
    /// The frame doesn't hold the rows asked for.
    Rows,
}

/// How many of a game area's bottom rows hold the bottom `rows` rows of its 1x picture, in game
/// rows: `rows` itself up to 768 lines, or the rows the stretch recovery reads above that. What a
/// capture backend reads back for a reader that needs `rows` 1x rows.
pub fn rows_for(height: u32, rows: u32) -> u32 {
    if height <= NATIVE_HEIGHT {
        rows.min(height)
    } else {
        rows_needed(height, rows).unwrap_or(height)
    }
}

/// Calls `read` with the bottom `rows` rows of the frame's 1x picture: a frame that declares a
/// backing scale is undone first; then it's the frame itself up to 768 lines, or else the rows
/// recovered from the picture the game stretched. The bottom row of the picture `read` gets is
/// the game area's bottom row, so a reader measures up from its height.
pub fn with_bottom_1x<T>(
    frame: &Frame,
    rows: u32,
    read: impl FnOnce(&Frame) -> T,
) -> Result<T, No1x> {
    let scale = frame.scale();
    if scale > 1 {
        let held = scaled_rows(rows_for(frame.height() / scale, rows), scale);
        let picture = unscale(&frame.bottom_rows(held)).ok_or(No1x::Scale(scale))?;
        return with_bottom_1x(&picture.frame(), rows, read);
    }
    if frame.first_row() + rows_for(frame.height(), rows) > frame.height() {
        return Err(No1x::Rows);
    }
    if frame.height() <= NATIVE_HEIGHT {
        return Ok(read(frame));
    }
    let picture = bottom_rows(frame, rows).ok_or(No1x::Stretch)?;
    Ok(read(&picture.frame()))
}

/// The least-squares inverse of a one-dimensional bilinear stretch from `source` pixels to
/// `stretched` pixels, for the source pixels from `first - guard` to the end. Stretched pixel `j`
/// samples source position `(j + 0.5) * source / stretched - 0.5`, clamped at the edges, which
/// is how the game's stretch lines up (fitted on recorded captures to a hundredth of a pixel).
#[derive(Debug)]
struct Inverse {
    /// Source pixels solved for, from `start`.
    unknowns: usize,
    /// Each stretched pixel used: its index, the unknown its left tap falls on, and the weight
    /// of its right tap (the left tap's is 1 minus that). The right tap is the next unknown,
    /// or the same one at the far edge.
    taps: Vec<(usize, usize, f64)>,
    /// Off-diagonal of the normal equations: how much unknowns `i` and `i + 1` share.
    shared: Vec<f64>,
    /// The Thomas algorithm's forward factors, which depend only on the geometry.
    factor: Vec<f64>,
    pivot: Vec<f64>,
}

impl Inverse {
    fn new(stretched: usize, source: usize, first: usize, guard: usize) -> Option<Inverse> {
        if stretched <= source || first >= source {
            return None;
        }
        let start = first.saturating_sub(guard);
        let unknowns = source - start;
        let scale = source as f64 / stretched as f64;
        let mut taps = Vec::new();
        for j in 0..stretched {
            let t = (j as f64 + 0.5) * scale - 0.5;
            let left = t.floor();
            let weight = t - left;
            let clamp = |i: f64| i.clamp(0.0, (source - 1) as f64) as usize;
            let (left_tap, right_tap) = (clamp(left), clamp(left + 1.0));
            if left_tap < start {
                continue;
            }
            let weight = if right_tap == left_tap { 0.0 } else { weight };
            taps.push((j, left_tap - start, weight));
        }
        // Normal equations (the stretch's matrix times its transpose): tridiagonal.
        let mut diagonal = vec![0f64; unknowns];
        let mut shared = vec![0f64; unknowns];
        for &(_, i, w) in &taps {
            diagonal[i] += (1.0 - w) * (1.0 - w);
            if w > 0.0 {
                diagonal[i + 1] += w * w;
                shared[i] += (1.0 - w) * w;
            }
        }
        let mut factor = vec![0f64; unknowns];
        let mut pivot = vec![0f64; unknowns];
        for i in 0..unknowns {
            let p = match i {
                0 => diagonal[0],
                _ => diagonal[i] - shared[i - 1] * factor[i - 1],
            };
            if p <= 1e-9 {
                return None;
            }
            pivot[i] = p;
            factor[i] = shared[i] / p;
        }
        Some(Inverse {
            unknowns,
            taps,
            shared,
            factor,
            pivot,
        })
    }

    /// Fills `out` (length `unknowns`) with the source pixels that best explain the stretched
    /// values `stretched(j)`.
    fn solve(&self, stretched: impl Fn(usize) -> f64, out: &mut [f64]) {
        out.fill(0.0);
        for &(j, i, w) in &self.taps {
            let v = stretched(j);
            out[i] += (1.0 - w) * v;
            if w > 0.0 {
                out[i + 1] += w * v;
            }
        }
        for i in 0..self.unknowns {
            if i > 0 {
                out[i] -= self.shared[i - 1] * out[i - 1];
            }
            out[i] /= self.pivot[i];
        }
        for i in (0..self.unknowns.saturating_sub(1)).rev() {
            out[i] -= self.factor[i] * out[i + 1];
        }
    }
}

/// Stretches a picture the way the game does, for tests: bilinear, with the same alignment and
/// edge clamping as [`Inverse`], rounded to 8 bits. `picture` is RGB rows of `width` pixels and
/// 768 rows; only the stretched rows from `from_row` down are drawn, the rest stay black.
#[cfg(test)]
pub(crate) fn stretch(
    picture: &[[u8; 3]],
    width: u32,
    to_width: u32,
    to_height: u32,
    from_row: u32,
) -> Vec<u8> {
    let (w, h) = (width as usize, NATIVE_HEIGHT as usize);
    assert_eq!(picture.len(), w * h);
    let sample = |n: usize, to: u32, j: u32| {
        let t = (f64::from(j) + 0.5) * n as f64 / f64::from(to) - 0.5;
        let left = t.floor();
        let clamp = |i: f64| i.clamp(0.0, (n - 1) as f64) as usize;
        (clamp(left), clamp(left + 1.0), t - left)
    };
    let mut bgra = vec![0u8; (to_width * to_height * 4) as usize];
    for y in from_row..to_height {
        let (y0, y1, fy) = sample(h, to_height, y);
        for x in 0..to_width {
            let (x0, x1, fx) = sample(w, to_width, x);
            let i = ((y * to_width + x) * 4) as usize;
            for c in 0..3 {
                let p = |yy: usize, xx: usize| f64::from(picture[yy * w + xx][c]);
                let top = p(y0, x0) * (1.0 - fx) + p(y0, x1) * fx;
                let bottom = p(y1, x0) * (1.0 - fx) + p(y1, x1) * fx;
                bgra[i + 2 - c] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
            bgra[i + 3] = 255;
        }
    }
    bgra
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_width_is_the_nearest_even_width_of_the_same_shape() {
        assert_eq!(picture_width(1920, 1200), 1228);
        assert_eq!(picture_width(2560, 1440), 1366);
        assert_eq!(picture_width(1920, 1080), 1366);
        assert_eq!(picture_width(3840, 2160), 1366);
        assert_eq!(picture_width(2560, 1600), 1228);
        assert_eq!(picture_width(10, 0), 0);
    }

    /// A busy 1x picture: hard one-pixel edges in every color, like HUD text.
    fn busy_picture(width: u32) -> Vec<[u8; 3]> {
        (0..NATIVE_HEIGHT)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let v = |k: u32| ((x * 7 + y * 13 + k * 29 + (x ^ y) * 3) % 251) as u8;
                    if (x / 3 + y / 2) % 4 == 0 {
                        [255, 255, 255]
                    } else {
                        [v(0), v(1), v(2)]
                    }
                })
            })
            .collect()
    }

    #[test]
    fn recovers_the_1x_rows_of_a_stretched_picture() {
        for (width, height) in [(1920, 1200), (2560, 1440), (1920, 1080)] {
            let narrow = picture_width(width, height);
            let picture = busy_picture(narrow);
            let rows = 32;
            let needed = rows_needed(height, rows).unwrap();
            // The capture tool's 40-row strips (scaled with the stretch) hold enough rows.
            assert!(needed <= (40 * height).div_ceil(NATIVE_HEIGHT), "{needed}");
            // Read back only the rows the recovery needs, as a capture backend does.
            let from_row = height - needed;
            let bgra = stretch(&picture, narrow, width, height, from_row);
            let stride = width as usize * 4;
            let strip = &bgra[from_row as usize * stride..];
            let frame = Frame::bottom(width, height, from_row, stride, strip).unwrap();
            let short = Frame::bottom(width, height, from_row + 1, stride, &strip[stride..]);
            assert_eq!(bottom_rows(&short.unwrap(), rows), None, "one row short");
            let recovered = bottom_rows(&frame, rows).unwrap();
            let recovered = recovered.frame();
            // The bottom of the game's own 768-line picture, so readers measure up from there.
            assert_eq!(
                (recovered.width(), recovered.height(), recovered.first_row()),
                (narrow, NATIVE_HEIGHT, NATIVE_HEIGHT - rows)
            );
            let mut worst = 0;
            for y in NATIVE_HEIGHT - rows..NATIVE_HEIGHT {
                for x in 0..narrow {
                    let truth = picture[(y * narrow + x) as usize];
                    let got = recovered.rgb(x, y);
                    for c in 0..3 {
                        worst = worst.max(truth[c].abs_diff(got[c]));
                    }
                }
            }
            assert!(worst <= 3, "{width}x{height}: off by up to {worst}");
        }
    }

    /// `picture` (RGB rows of `width`) as a Mac shows it at backing scale 2: every pixel a 2x2
    /// block, with the blocks starting `phase` pixels in, and black where the picture doesn't
    /// reach (the band Wine leaves when it places the game off the window's edge).
    fn doubled(picture: &[[u8; 3]], width: u32, (px, py): (u32, u32)) -> Vec<u8> {
        let height = picture.len() as u32 / width;
        let mut bgra = vec![0u8; (4 * width * height * 4) as usize];
        for y in 0..2 * height {
            for x in 0..2 * width {
                let i = ((y * 2 * width + x) * 4) as usize;
                let rgb = match (x.checked_sub(px), y.checked_sub(py)) {
                    (Some(sx), Some(sy)) => picture[((sy / 2) * width + sx / 2) as usize],
                    _ => [0, 0, 0],
                };
                bgra[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
            }
        }
        bgra
    }

    // Fails if the phase is misread by a row, a row or column is dropped at the strip's top, or
    // see-through corner pixels throw the phase off. The frame holds only the rows a backend reads.
    #[test]
    fn undoes_pixel_doubling_exactly_at_every_phase() {
        let (width, height) = (120, 40);
        let picture: Vec<[u8; 3]> = busy_picture(width)[..(width * height) as usize].to_vec();
        for phase in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let mut bgra = doubled(&picture, width, phase);
            let stride = (2 * width * 4) as usize;
            // The window's rounded bottom corners: see-through pixels of any color.
            for (x, y) in [
                (0, 2 * height - 1),
                (1, 2 * height - 1),
                (2 * width - 1, 2 * height - 2),
            ] {
                let i = y as usize * stride + x as usize * 4;
                bgra[i..i + 4].copy_from_slice(&[9, 99, 199, 40]);
            }
            // An opaque pixel beside them, one off where macOS blended the corner in.
            let i = (2 * height - 2) as usize * stride + 4;
            bgra[i] = bgra[i].wrapping_add(1);
            let rows = 12;
            let from = 2 * height - scaled_rows(rows, 2);
            let strip = &bgra[from as usize * stride..];
            let frame = Frame::bottom(2 * width, 2 * height, from, stride, strip)
                .and_then(|f| f.with_scale(2))
                .unwrap();
            let recovered = unscale(&frame).unwrap();
            let recovered = recovered.frame();
            assert_eq!((recovered.width(), recovered.height()), (width, height));
            assert!(recovered.first_row() <= height - rows, "{phase:?}");
            for y in recovered.first_row()..height {
                for x in 0..width {
                    // The see-through corners are the window's, not the game's.
                    if y == height - 1 && (x <= 1 || x == width - 1) {
                        continue;
                    }
                    let truth = picture[(y * width + x) as usize];
                    assert_eq!(recovered.rgb(x, y), truth, "{phase:?} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn refuses_a_frame_that_is_not_pixel_doubled() {
        // A smoothly scaled 2x frame (bilinear, the game's own stretch) has no flat blocks.
        let picture = busy_picture(60);
        let bgra = stretch(&picture, 60, 120, 1536, 1536 - 40);
        let stride = 120 * 4;
        let strip = &bgra[(1536 - 40) * stride..];
        let frame = Frame::bottom(120, 1536, 1536 - 40, stride, strip).unwrap();
        assert!(unscale(&frame.with_scale(2).unwrap()).is_none());
        // Nothing to undo at scale 1, and a strip without one whole block can't be placed.
        assert!(unscale(&frame).is_none());
        let one_row = Frame::bottom(120, 1536, 1535, stride, &strip[39 * stride..]).unwrap();
        assert!(unscale(&one_row.with_scale(2).unwrap()).is_none());
    }

    #[test]
    fn leaves_frames_at_1x_alone() {
        let bgra = vec![0u8; 1366 * 768 * 4];
        let frame = Frame::new(1366, 768, 1366 * 4, &bgra).unwrap();
        assert_eq!(bottom_rows(&frame, 32), None);
        // 100x800 is a 96x768 picture stretched.
        let bgra = vec![0u8; 100 * 800 * 4];
        let frame = Frame::new(100, 800, 100 * 4, &bgra).unwrap();
        assert!(bottom_rows(&frame, 0).is_none());
        assert!(bottom_rows(&frame, 769).is_none());
        assert!(bottom_rows(&frame, 32).is_some());
        assert_eq!(rows_needed(768, 32), None);
        assert_eq!(rows_needed(1080, 0), None);
    }
}
