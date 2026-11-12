//! Lines that stack up from the bottom and move up as new ones arrive: the chat box, and the
//! pickup notices at the bottom right. New lines are counted by how far the lines already on
//! screen moved, never by their text, so the same line said twice counts twice.

/// A line of a stack, as a reader cut it from one frame.
pub trait StackLine {
    /// The same line, seen in another frame: it may sit a pixel or two away, or have something
    /// drawn over part of it. `moved` when lines arrived in between, so it's another line's
    /// place it's in now.
    fn matches(&self, other: &Self, moved: bool) -> bool;
    /// Nothing shows in this line's place.
    fn is_blank(&self) -> bool;
}

/// How many lines arrived at the bottom: the smallest shift that lines up the last frame's lines
/// with this one's. Both lists run from the bottom line up. One line may differ, for something
/// drawn over it, when four or more pairs of lines with something showing on both sides line up:
/// blank lines say nothing about where the stack moved. `None` when no shift lines up.
pub fn arrived<L: StackLine>(before: &[L], now: &[L]) -> Option<usize> {
    arrived_by(before, now, L::matches)
}

/// [`arrived`], with `same` telling whether a line of the last frame is the line in its place now,
/// as [`StackLine::matches`] does, for a reader that knows more than the two lines show.
pub fn arrived_by<L: StackLine>(
    before: &[L],
    now: &[L],
    same: impl Fn(&L, &L, bool) -> bool,
) -> Option<usize> {
    (0..=now.len()).find(|&shift| {
        let pairs: Vec<(bool, bool)> = before
            .iter()
            .zip(now.iter().skip(shift))
            .map(|(b, n)| (same(b, n, shift > 0), !b.is_blank() && !n.is_blank()))
            .collect();
        let misses = pairs.iter().filter(|(m, _)| !m).count();
        let shown = pairs.iter().filter(|(_, shown)| *shown).count();
        match shift == now.len() {
            // Everything on screen is new: that only lines up if the last frame had no lines.
            true => before.iter().all(StackLine::is_blank),
            false => misses == 0 || (shown >= 4 && misses <= 1),
        }
    })
}

/// How many pixels differ between `a` and `b` moved `by` pixels along, leaving out the `hidden`
/// ones. All three are one-bit pictures of a line `len` pixels in all, row after row (bit
/// `row * width + column`), so moving a row down is `by = width`. A row's end runs on into the
/// next row's start, but text never reaches the line's edges.
pub fn differ(a: &[u64], b: &[u64], len: u32, by: i32, hidden: Option<&[u64]>) -> u32 {
    let len = i64::from(len);
    // The 64 pixels of `b` moved along that land at `start`: none from outside the line.
    let moved = |start: i64| -> u64 {
        let from = start - i64::from(by);
        let word = |i: i64| {
            usize::try_from(i)
                .ok()
                .and_then(|i| b.get(i))
                .copied()
                .unwrap_or(0)
        };
        let (q, r) = (from.div_euclid(64), from.rem_euclid(64) as u32);
        let low = word(q) >> r;
        let high = if r == 0 { 0 } else { word(q + 1) << (64 - r) };
        low | high
    };
    a.iter()
        .enumerate()
        .map(|(i, &word)| {
            let start = i as i64 * 64;
            // The last word's pixels past the line's end, where `b`'s bottom row may move to.
            let inside = match len - start {
                n if n >= 64 => u64::MAX,
                n => (1u64 << n) - 1,
            };
            let shown = !hidden.map_or(0, |hidden| hidden[i]);
            ((word ^ moved(start)) & inside & shown).count_ones()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test line: its text, or blank.
    struct Line(&'static str);

    impl StackLine for Line {
        fn matches(&self, other: &Self, _moved: bool) -> bool {
            self.0 == other.0
        }
        fn is_blank(&self) -> bool {
            self.0.is_empty()
        }
    }

    fn lines(texts: &[&'static str]) -> Vec<Line> {
        texts.iter().map(|&t| Line(t)).collect()
    }

    /// A one-bit line `width` wide and `height` tall with ink at each (column, row).
    fn ink(width: u32, height: u32, at: &[(u32, u32)]) -> Vec<u64> {
        let mut bits = vec![0u64; (width * height).div_ceil(64) as usize];
        for &(col, row) in at {
            let bit = (row * width + col) as usize;
            bits[bit / 64] |= 1 << (bit % 64);
        }
        bits
    }

    // Guards counting by movement: two identical new lines count as two, and a frame that
    // lines up with nothing says so instead of guessing.
    #[test]
    fn counts_new_lines_by_how_far_the_old_ones_moved() {
        let before = lines(&["+6 EXP", "+9 mesos", ""]);
        assert_eq!(
            arrived(&before, &lines(&["+6 EXP", "+9 mesos", ""])),
            Some(0)
        );
        assert_eq!(
            arrived(&before, &lines(&["+6 EXP", "+6 EXP", "+9 mesos"])),
            Some(1)
        );
        assert_eq!(
            arrived(&before, &lines(&["+6 EXP", "+6 EXP", "+6 EXP"])),
            Some(2)
        );
        // Two lines entering an empty stack are two arrivals.
        assert_eq!(
            arrived(&lines(&["", "", ""]), &lines(&["a", "b", ""])),
            Some(2)
        );
        assert_eq!(
            arrived(&lines(&["x", "y", "z"]), &lines(&["a", "b", "c"])),
            None
        );
        // One line may differ among four that line up, for something drawn over it. Blank lines
        // don't count toward the four: they line up with any shift.
        let full = lines(&["a", "b", "c", "d", "e"]);
        assert_eq!(arrived(&full, &lines(&["?", "b", "c", "d", "e"])), Some(0));
        let sparse = lines(&["a", "", "", "", ""]);
        assert_eq!(arrived(&sparse, &lines(&["?", "", "", "", ""])), None);
    }

    // Guards the width parameter: the same shape a pixel to the right, or a row down, differs
    // by nothing once moved back, at a width other than the chat box's.
    #[test]
    fn compares_lines_moved_along_at_any_width() {
        let (w, h) = (360, 14);
        let shape = [(10, 3), (11, 3), (12, 4), (200, 9), (359, 13)];
        let a = ink(w, h, &shape);
        let four = ink(w, h, &shape[..4]);
        let right: Vec<(u32, u32)> = shape[..4].iter().map(|&(c, r)| (c + 1, r)).collect();
        let down: Vec<(u32, u32)> = shape[..4].iter().map(|&(c, r)| (c, r + 1)).collect();
        assert_eq!(differ(&a, &a, w * h, 0, None), 0);
        assert_eq!(differ(&four, &ink(w, h, &right), w * h, -1, None), 0);
        assert_eq!(
            differ(&four, &ink(w, h, &down), w * h, -(w as i32), None),
            0
        );
        assert_eq!(
            differ(&a, &ink(w, h, &[]), w * h, 0, None),
            shape.len() as u32
        );
        // The last pixel of the line is the only difference; hiding it leaves none.
        assert_eq!(differ(&a, &four, w * h, 0, None), 1);
        let hidden = ink(w, h, &[(359, 13)]);
        assert_eq!(differ(&a, &four, w * h, 0, Some(&hidden)), 0);
    }
}
