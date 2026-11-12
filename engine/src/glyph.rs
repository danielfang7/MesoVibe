//! One-bit glyph shapes cut from a frame, and the atlas that names them.
//!
//! An atlas is learned on the player's device from their own screen and never ships, so
//! MesoVibe carries no game pixels.

use std::collections::HashMap;

use crate::frame::Frame;

/// A rectangle in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// Whether the rectangle lies inside the rows the frame holds.
    fn fits(&self, frame: &Frame) -> bool {
        self.x
            .checked_add(self.width)
            .is_some_and(|r| r <= frame.width())
            && self.y >= frame.first_row()
            && self
                .y
                .checked_add(self.height)
                .is_some_and(|b| b <= frame.height())
    }
}

/// Widest glyph a [`Glyph`] can hold.
pub const MAX_GLYPH_WIDTH: u32 = 32;

/// Which pixels of a glyph's cell are ink. Each row is a bit mask with the left pixel in bit 0,
/// so glyphs are at most [`MAX_GLYPH_WIDTH`] pixels wide. The cell spans the full height of the
/// region it was cut from, so a comma and an apostrophe stay different shapes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Glyph {
    width: u8,
    rows: Vec<u32>,
}

impl Glyph {
    /// A glyph from its rows' bit masks, left pixel in bit 0, for readers that tell ink from
    /// more than one pixel's color. `None` when it's wider than [`MAX_GLYPH_WIDTH`], or a row has
    /// ink past `width`.
    pub fn from_rows(width: u32, rows: Vec<u32>) -> Option<Glyph> {
        let fits = |row: &u32| width == MAX_GLYPH_WIDTH || *row >> width == 0;
        (width > 0 && width <= MAX_GLYPH_WIDTH && rows.iter().all(fits)).then_some(Glyph {
            width: width as u8,
            rows,
        })
    }

    pub fn width(&self) -> u32 {
        u32::from(self.width)
    }

    pub fn height(&self) -> u32 {
        self.rows.len() as u32
    }

    /// Whether the cell holds no ink at all.
    pub fn is_blank(&self) -> bool {
        self.rows.iter().all(|&row| row == 0)
    }

    /// Whether the pixel `dx` across and `dy` down from the cell's top-left corner is ink.
    pub fn is_ink(&self, dx: u32, dy: u32) -> bool {
        dx < self.width()
            && self
                .rows
                .get(dy as usize)
                .is_some_and(|row| row >> dx & 1 == 1)
    }

    /// The rows that hold ink, as a bit mask with the top row in bit 0.
    pub fn ink_rows(&self) -> u32 {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| **row != 0)
            .fold(0, |mask, (y, _)| mask | 1 << y)
    }

    /// The number of pixels that differ from `other`, or `None` when the cells differ in size.
    pub fn distance(&self, other: &Glyph) -> Option<u32> {
        (self.width == other.width && self.rows.len() == other.rows.len()).then(|| {
            self.rows
                .iter()
                .zip(&other.rows)
                .map(|(a, b)| (a ^ b).count_ones())
                .sum()
        })
    }

    /// The glyph as text, `<width>:<row>,<row>,...` with each row's bit mask in hex, for keeping
    /// what was learned on the player's device.
    pub fn encode(&self) -> String {
        let rows: Vec<String> = self.rows.iter().map(|row| format!("{row:x}")).collect();
        format!("{}:{}", self.width, rows.join(","))
    }

    /// A glyph from [`Glyph::encode`]'s text. `None` for anything it couldn't have written.
    pub fn decode(text: &str) -> Option<Glyph> {
        let (width, rows) = text.split_once(':')?;
        let width: u8 = width.parse().ok()?;
        if width == 0 || u32::from(width) > MAX_GLYPH_WIDTH {
            return None;
        }
        let rows: Vec<u32> = rows
            .split(',')
            .map(|row| u32::from_str_radix(row, 16).ok())
            .collect::<Option<_>>()?;
        let fits = |row: &u32| u32::from(width) == 32 || *row >> width == 0;
        (!rows.is_empty() && rows.len() <= MAX_GLYPH_HEIGHT && rows.iter().all(fits))
            .then_some(Glyph { width, rows })
    }
}

/// Tallest glyph [`Glyph::decode`] accepts: taller than any HUD region.
const MAX_GLYPH_HEIGHT: usize = 64;

/// Why a region couldn't be cut into glyphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutError {
    /// The region reaches outside the frame.
    OutsideFrame,
    /// A run of ink columns is wider than any glyph: something other than text is there.
    TooWide,
}

/// The whole region as one glyph.
pub fn cut(
    frame: &Frame,
    region: Rect,
    is_ink: impl Fn([u8; 3]) -> bool,
) -> Result<Glyph, CutError> {
    if !region.fits(frame) {
        return Err(CutError::OutsideFrame);
    }
    if region.width > MAX_GLYPH_WIDTH {
        return Err(CutError::TooWide);
    }
    Ok(glyph_at(frame, region, region.x, region.width, &is_ink))
}

/// Cuts the region into glyphs: runs of columns that hold ink, split by columns that hold none.
/// Each glyph is returned with the x where its cell starts, left to right.
pub fn segment(
    frame: &Frame,
    region: Rect,
    is_ink: impl Fn([u8; 3]) -> bool,
) -> Result<Vec<(u32, Glyph)>, CutError> {
    if !region.fits(frame) {
        return Err(CutError::OutsideFrame);
    }
    let inked = |x: u32| (region.y..region.y + region.height).any(|y| is_ink(frame.rgb(x, y)));
    let mut glyphs = Vec::new();
    let mut start = None;
    for x in region.x..=region.x + region.width {
        let ink = x < region.x + region.width && inked(x);
        match (start, ink) {
            (None, true) => start = Some(x),
            (Some(s), false) => {
                if x - s > MAX_GLYPH_WIDTH {
                    return Err(CutError::TooWide);
                }
                glyphs.push((s, glyph_at(frame, region, s, x - s, &is_ink)));
                start = None;
            }
            _ => {}
        }
    }
    Ok(glyphs)
}

fn glyph_at(
    frame: &Frame,
    region: Rect,
    x0: u32,
    width: u32,
    is_ink: &impl Fn([u8; 3]) -> bool,
) -> Glyph {
    let rows = (region.y..region.y + region.height)
        .map(|y| {
            (0..width)
                .filter(|&dx| is_ink(frame.rgb(x0 + dx, y)))
                .fold(0u32, |mask, dx| mask | 1 << dx)
        })
        .collect();
    Glyph {
        width: width as u8,
        rows,
    }
}

/// Why an atlas refused to learn. Learning is all or nothing, so the atlas is unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearnError {
    /// The region held a different number of glyphs than the text has characters.
    CountMismatch { glyphs: usize, chars: usize },
    /// One shape was given two names, in this text or against what was learned before.
    ShapeNamedTwice { first: char, second: char },
    /// Two different shapes were given the same name.
    NameUsedTwice { name: char },
}

/// Names glyph shapes. Each shape has one name and each name one shape, which catches most
/// typing mistakes during calibration: a mistyped digit names two shapes the same way.
#[derive(Debug, Clone, Default)]
pub struct Atlas {
    names: HashMap<Glyph, char>,
    shapes: HashMap<char, Glyph>,
}

impl Atlas {
    /// Names each glyph with the character of `text` in the same position.
    pub fn learn(&mut self, glyphs: &[Glyph], text: &str) -> Result<usize, LearnError> {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() != glyphs.len() {
            return Err(LearnError::CountMismatch {
                glyphs: glyphs.len(),
                chars: chars.len(),
            });
        }
        let mut next = self.clone();
        let mut added = 0;
        for (glyph, &name) in glyphs.iter().zip(&chars) {
            match (next.names.get(glyph), next.shapes.get(&name)) {
                (Some(&first), _) if first != name => {
                    return Err(LearnError::ShapeNamedTwice {
                        first,
                        second: name,
                    });
                }
                (Some(_), _) => {}
                (None, Some(_)) => return Err(LearnError::NameUsedTwice { name }),
                (None, None) => {
                    next.names.insert(glyph.clone(), name);
                    next.shapes.insert(name, glyph.clone());
                    added += 1;
                }
            }
        }
        *self = next;
        Ok(added)
    }

    /// The name of an exactly matching shape. Near misses aren't accepted: a one-bit pixel font
    /// draws every glyph the same way each time, so a difference means something else is there.
    pub fn name(&self, glyph: &Glyph) -> Option<char> {
        self.names.get(glyph).copied()
    }

    /// The shape learned for `name`, if any.
    pub fn shape(&self, name: char) -> Option<&Glyph> {
        self.shapes.get(&name)
    }

    /// Drops what was learned for `name`, so its shape is unknown again.
    pub fn forget(&mut self, name: char) {
        if let Some(shape) = self.shapes.remove(&name) {
            self.names.remove(&shape);
        }
    }

    /// The characters of `wanted` the atlas hasn't learned yet.
    pub fn missing(&self, wanted: &str) -> Vec<char> {
        wanted
            .chars()
            .filter(|c| !self.shapes.contains_key(c))
            .collect()
    }

    /// Every learned shape with its name, in name order.
    pub fn entries(&self) -> Vec<(char, &Glyph)> {
        let mut entries: Vec<(char, &Glyph)> = self.shapes.iter().map(|(c, g)| (*c, g)).collect();
        entries.sort_by_key(|(c, _)| *c);
        entries
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white-on-black strip from ASCII rows: '#' is ink.
    fn strip(rows: &[&str]) -> (u32, u32, Vec<u8>) {
        let (w, h) = (rows[0].len() as u32, rows.len() as u32);
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                if c == '#' {
                    let i = (y * w as usize + x) * 4;
                    bgra[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        (w, h, bgra)
    }

    fn white(p: [u8; 3]) -> bool {
        p.iter().all(|&c| c > 128)
    }

    #[test]
    fn segments_on_empty_columns_and_keeps_full_height() {
        let (w, h, bgra) = strip(&["#..##..", "#..#...", "#.....#"]);
        let frame = Frame::new(w, h, (w * 4) as usize, &bgra).unwrap();
        let all = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let glyphs = segment(&frame, all, white).unwrap();
        let xs: Vec<u32> = glyphs.iter().map(|(x, _)| *x).collect();
        assert_eq!(xs, vec![0, 3, 6]);
        assert_eq!(glyphs[1].1.width(), 2);
        assert_eq!(glyphs[1].1.height(), 3);
        // The third glyph's ink is only in its bottom row, and its cell still has all three rows.
        assert_eq!(
            glyphs[2].1,
            Glyph {
                width: 1,
                rows: vec![0, 0, 1]
            }
        );
    }

    #[test]
    fn refuses_regions_outside_the_frame_and_runs_too_wide_for_text() {
        let (w, h, bgra) = strip(&[&"#".repeat(40)]);
        let frame = Frame::new(w, h, (w * 4) as usize, &bgra).unwrap();
        let wide = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        assert_eq!(segment(&frame, wide, white), Err(CutError::TooWide));
        let outside = Rect {
            x: 30,
            y: 0,
            width: 20,
            height: 1,
        };
        assert_eq!(segment(&frame, outside, white), Err(CutError::OutsideFrame));
        assert_eq!(cut(&frame, outside, white), Err(CutError::OutsideFrame));
    }

    #[test]
    fn distance_counts_differing_pixels_of_same_sized_cells() {
        let a = Glyph {
            width: 3,
            rows: vec![0b101, 0b010],
        };
        let b = Glyph {
            width: 3,
            rows: vec![0b100, 0b011],
        };
        assert_eq!(a.distance(&b), Some(2));
        assert_eq!(
            a.distance(&Glyph {
                width: 2,
                rows: vec![0, 0]
            }),
            None
        );
    }

    fn g(bits: u32) -> Glyph {
        Glyph {
            width: 4,
            rows: vec![bits],
        }
    }

    #[test]
    fn learns_names_and_reports_what_is_missing() {
        let mut atlas = Atlas::default();
        // A repeated shape with the same name is fine; it's learned once.
        assert_eq!(atlas.learn(&[g(1), g(2), g(1)], "aba"), Ok(2));
        assert_eq!(atlas.name(&g(2)), Some('b'));
        assert_eq!(atlas.name(&g(3)), None);
        assert_eq!(atlas.missing("abc"), vec!['c']);
        assert_eq!(atlas.learn(&[g(3)], "c"), Ok(1));
        assert!(atlas.missing("abc").is_empty());
        assert_eq!(atlas.len(), 3);
    }

    #[test]
    fn learning_is_all_or_nothing_and_catches_typos() {
        let mut atlas = Atlas::default();
        atlas.learn(&[g(1), g(2)], "12").unwrap();
        let before = atlas.len();
        // Wrong count.
        assert_eq!(
            atlas.learn(&[g(3), g(4)], "345"),
            Err(LearnError::CountMismatch {
                glyphs: 2,
                chars: 3
            })
        );
        // A typo that names two shapes '7' (one of them really an '8').
        assert_eq!(
            atlas.learn(&[g(7), g(8)], "77"),
            Err(LearnError::NameUsedTwice { name: '7' })
        );
        // A shape learned as '1' now called '4'.
        assert_eq!(
            atlas.learn(&[g(9), g(1)], "34"),
            Err(LearnError::ShapeNamedTwice {
                first: '1',
                second: '4'
            })
        );
        // A new shape for a name already taken.
        assert_eq!(
            atlas.learn(&[g(5)], "2"),
            Err(LearnError::NameUsedTwice { name: '2' })
        );
        assert_eq!(atlas.len(), before, "a refused lesson changes nothing");
        assert_eq!(atlas.name(&g(9)), None);
    }

    #[test]
    fn glyphs_and_atlases_keep_as_text_and_refuse_anything_else() {
        let glyph = Glyph {
            width: 5,
            rows: vec![0b11111, 0b10001, 0, 0b100],
        };
        assert_eq!(glyph.encode(), "5:1f,11,0,4");
        assert_eq!(Glyph::decode(&glyph.encode()), Some(glyph.clone()));
        let wide = Glyph {
            width: 32,
            rows: vec![u32::MAX],
        };
        assert_eq!(Glyph::decode(&wide.encode()), Some(wide));
        for bad in [
            "", "5", "5:", "0:1", "33:1", "5:20", "5:1,,2", "x:1", "5:-1",
        ] {
            assert_eq!(Glyph::decode(bad), None, "{bad:?}");
        }
        let mut atlas = Atlas::default();
        atlas.learn(std::slice::from_ref(&glyph), "7").unwrap();
        assert_eq!(atlas.entries(), [('7', &glyph)]);
    }
}
