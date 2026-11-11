//! Names a digit by its shape, the way a person reads a digit in a font they haven't seen: by its
//! closed loops and strokes, not by matching it to a picture. An 8 has two loops; a 0 one tall
//! loop; a 6 a loop at the bottom and a 9 at the top; a 4 a loop over a bar, with a thin stem
//! below and a thin point above. Without loops, a 1 is narrow, a 7 is a bar over a thin stroke, a
//! 2 sits on a bar, and a 5 has strokes on the left where a 3 has none.
//!
//! This holds for ordinary digits, not for every font, so the notice reader only trusts it after
//! it names every digit the reader already learned from EXP lines the way they were learned.

use crate::glyph::Glyph;

/// The digit a glyph's shape shows, or `None` when it doesn't look like one.
pub(crate) fn guess(glyph: &Glyph) -> Option<char> {
    let ink = Ink::of(glyph)?;
    let (w, h) = (ink.width, ink.height);
    if h < 5 || w < 2 {
        return None;
    }
    let holes = ink.holes();
    let row = |y: usize| ink.row_ink(y);
    match holes[..] {
        [_, _] => Some('8'),
        [(top, bottom)] => {
            if (bottom - top + 1) * 2 >= h {
                Some('0')
            } else if row(0) <= 2 && row(h - 1) <= 2 {
                Some('4')
            } else if (top + bottom) as f32 / 2.0 > (h - 1) as f32 / 2.0 {
                Some('6')
            } else {
                Some('9')
            }
        }
        [] => {
            if w * 2 <= h {
                Some('1')
            } else if row(0) + 1 >= w && row(h - 1) <= 2 {
                Some('7')
            } else if row(h - 1) + 1 >= w {
                Some('2')
            } else if (2..=h / 2).any(|y| (0..w / 2).any(|x| ink.at(x, y))) {
                Some('5')
            } else {
                Some('3')
            }
        }
        _ => None,
    }
}

/// A glyph's ink, cropped to the rows and columns that hold any.
struct Ink {
    width: usize,
    height: usize,
    /// Row-major, `width` per row.
    cells: Vec<bool>,
}

impl Ink {
    fn of(glyph: &Glyph) -> Option<Ink> {
        let (gw, gh) = (glyph.width(), glyph.height());
        let rows: Vec<u32> = (0..gh)
            .filter(|&y| (0..gw).any(|x| glyph.is_ink(x, y)))
            .collect();
        let cols: Vec<u32> = (0..gw)
            .filter(|&x| (0..gh).any(|y| glyph.is_ink(x, y)))
            .collect();
        let (top, bottom) = (*rows.first()?, *rows.last()?);
        let (left, right) = (*cols.first()?, *cols.last()?);
        let (width, height) = ((right - left + 1) as usize, (bottom - top + 1) as usize);
        let mut cells = Vec::with_capacity(width * height);
        for y in top..=bottom {
            for x in left..=right {
                cells.push(glyph.is_ink(x, y));
            }
        }
        Some(Ink {
            width,
            height,
            cells,
        })
    }

    fn at(&self, x: usize, y: usize) -> bool {
        self.cells[y * self.width + x]
    }

    fn row_ink(&self, y: usize) -> usize {
        (0..self.width).filter(|&x| self.at(x, y)).count()
    }

    /// The closed loops: blank areas no path of blank pixels (side by side, not corner to corner)
    /// leads out of. Each as its top and bottom row.
    fn holes(&self) -> Vec<(usize, usize)> {
        let (w, h) = (self.width + 2, self.height + 2);
        // A blank border around the ink, so the outside is one area.
        let blank = |x: usize, y: usize| {
            x == 0 || y == 0 || x == w - 1 || y == h - 1 || !self.at(x - 1, y - 1)
        };
        let mut seen = vec![false; w * h];
        let fill = |start: (usize, usize), seen: &mut Vec<bool>| -> Option<(usize, usize)> {
            let mut stack = vec![start];
            let mut rows: Option<(usize, usize)> = None;
            while let Some((x, y)) = stack.pop() {
                if seen[y * w + x] || !blank(x, y) {
                    continue;
                }
                seen[y * w + x] = true;
                rows = Some(rows.map_or((y, y), |(t, b)| (t.min(y), b.max(y))));
                if x > 0 {
                    stack.push((x - 1, y));
                }
                if y > 0 {
                    stack.push((x, y - 1));
                }
                if x + 1 < w {
                    stack.push((x + 1, y));
                }
                if y + 1 < h {
                    stack.push((x, y + 1));
                }
            }
            rows
        };
        fill((0, 0), &mut seen);
        let mut holes = Vec::new();
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if !seen[y * w + x]
                    && blank(x, y)
                    && let Some((top, bottom)) = fill((x, y), &mut seen)
                {
                    holes.push((top - 1, bottom - 1));
                }
            }
        }
        holes
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::glyph::Glyph;

    /// Digits drawn by hand in a plain style, unlike any game's: what the rules must read.
    pub(crate) const PLAIN: [(char, [&str; 7]); 10] = [
        (
            '0',
            [
                ".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
            ],
        ),
        (
            '1',
            [
                "..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###.",
            ],
        ),
        (
            '2',
            [
                ".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####",
            ],
        ),
        (
            '3',
            [
                ".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###.",
            ],
        ),
        (
            '4',
            [
                "...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#.",
            ],
        ),
        (
            '5',
            [
                "#####", "#....", "####.", "....#", "....#", "#...#", ".###.",
            ],
        ),
        (
            '6',
            [
                "..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###.",
            ],
        ),
        (
            '7',
            [
                "#####", "....#", "...#.", "..#..", ".#...", ".#...", ".#...",
            ],
        ),
        (
            '8',
            [
                ".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###.",
            ],
        ),
        (
            '9',
            [
                ".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##..",
            ],
        ),
    ];

    /// A glyph from rows of `#` and `.`, in a cell two rows taller, as a line's cells are.
    pub(crate) fn drawn(rows: &[&str]) -> Glyph {
        let width = rows[0].len() as u32;
        let mut masks: Vec<u32> = rows
            .iter()
            .map(|row| {
                row.chars()
                    .enumerate()
                    .filter(|(_, c)| *c == '#')
                    .fold(0, |mask, (x, _)| mask | 1 << x)
            })
            .collect();
        masks.extend([0, 0]);
        Glyph::from_rows(width, masks).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{PLAIN, drawn};
    use super::*;

    // Fails if a plain digit is named wrong.
    #[test]
    fn names_plain_digits_by_their_loops_and_strokes() {
        for (digit, rows) in PLAIN {
            assert_eq!(guess(&drawn(&rows)), Some(digit), "{digit}");
        }
    }

    // Shapes that aren't digits get no name: too small, or more loops than any digit.
    #[test]
    fn names_nothing_that_isnt_shaped_like_a_digit() {
        assert_eq!(guess(&drawn(&["##", "##"])), None, "a dot");
        let three_loops = [
            "#####", "#.#.#", "#####", "#...#", "#####", "#.#.#", "#####",
        ];
        assert_eq!(guess(&drawn(&three_loops)), None);
    }
}
