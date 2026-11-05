//! One captured frame: the game's drawing area only, with the title bar and borders cropped.

/// A view of one captured frame as 8-bit BGRA rows, the layout Windows.Graphics.Capture and
/// ScreenCaptureKit deliver. Coordinates are the game's own pixels: (0, 0) is the top-left
/// corner of its drawing area.
///
/// A frame can hold only the bottom rows of the game area ([`Frame::bottom`]): capture backends
/// copy only the rows the readers need off the GPU, and the readers still see the game
/// area's full size.
///
/// A frame can also declare a backing scale ([`Frame::with_scale`]): a Retina Mac shows each game
/// pixel as a 2x2 block of screen pixels, and a capture holds those pixels as they are. Sizes and
/// coordinates are then in captured pixels, and the readers undo the scale first
/// ([`crate::unstretch::unscale`]).
#[derive(Debug, Clone, Copy)]
pub struct Frame<'a> {
    width: u32,
    height: u32,
    /// The first row `bgra` holds; rows above it weren't read back.
    first_row: u32,
    stride: usize,
    bgra: &'a [u8],
    /// Captured pixels per game pixel, each way: 1, or a Mac display's backing scale.
    scale: u32,
}

impl<'a> Frame<'a> {
    /// `stride` is the byte length of one row, at least `width * 4`. Returns `None` when the
    /// stride or the buffer is too short for the size.
    pub fn new(width: u32, height: u32, stride: usize, bgra: &'a [u8]) -> Option<Self> {
        Self::bottom(width, height, 0, stride, bgra)
    }

    /// A `width` x `height` game area of which `bgra` holds only the rows from `first_row` down
    /// to the bottom. Returns `None` when `first_row` is past the bottom (an empty frame excepted),
    /// or the stride or the buffer is too short for those rows.
    pub fn bottom(
        width: u32,
        height: u32,
        first_row: u32,
        stride: usize,
        bgra: &'a [u8],
    ) -> Option<Self> {
        let row = width as usize * 4;
        if stride < row || first_row > height {
            return None;
        }
        let needed = match (height - first_row) as usize {
            0 => 0,
            rows => stride * (rows - 1) + row,
        };
        (bgra.len() >= needed).then_some(Frame {
            width,
            height,
            first_row,
            stride,
            bgra,
            scale: 1,
        })
    }

    /// The same pixels, declared as `scale` captured pixels per game pixel each way. `None` when
    /// `scale` is 0 or the size isn't a whole number of game pixels.
    pub fn with_scale(self, scale: u32) -> Option<Self> {
        (scale > 0 && self.width.is_multiple_of(scale) && self.height.is_multiple_of(scale))
            .then_some(Frame { scale, ..self })
    }

    /// Captured pixels per game pixel, each way: 1 unless the frame declared a backing scale.
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// The same frame holding only its bottom `rows` rows (or all it holds, if fewer).
    pub fn bottom_rows(&self, rows: u32) -> Frame<'a> {
        let first_row = self.height.saturating_sub(rows).max(self.first_row);
        let skip = (first_row - self.first_row) as usize * self.stride;
        Frame {
            first_row,
            bgra: &self.bgra[skip.min(self.bgra.len())..],
            ..*self
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The first row the frame holds: 0 unless it was read back as a bottom strip.
    pub fn first_row(&self) -> u32 {
        self.first_row
    }

    /// The pixel at (x, y) as RGB. Panics outside the rows the frame holds; callers check their
    /// regions first.
    pub fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        let [b, g, r, _] = self.bgra(x, y);
        [r, g, b]
    }

    /// The pixel at (x, y) as stored, BGRA. Alpha is below 255 only where the window itself is
    /// see-through, like a Mac window's rounded corners. Panics outside the rows the frame holds.
    pub fn bgra(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height && y >= self.first_row,
            "({x}, {y}) is outside the frame's rows {}..{}",
            self.first_row,
            self.height
        );
        let i = (y - self.first_row) as usize * self.stride + x as usize * 4;
        let p = &self.bgra[i..i + 4];
        [p[0], p[1], p[2], p[3]]
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;

    #[test]
    fn rejects_buffers_too_short_for_the_size() {
        assert!(Frame::new(2, 2, 8, &[0; 16]).is_some());
        assert!(Frame::new(2, 2, 8, &[0; 15]).is_none());
        assert!(
            Frame::new(2, 2, 7, &[0; 64]).is_none(),
            "stride below width * 4"
        );
        // Padding after the last row isn't required.
        assert!(Frame::new(2, 2, 12, &[0; 20]).is_some());
        assert!(Frame::new(0, 0, 0, &[]).is_some());
    }

    #[test]
    fn reads_bgra_as_rgb_with_a_padded_stride() {
        let mut bgra = vec![0u8; 12 * 2];
        // Pixel (1, 1): blue 3, green 2, red 1.
        bgra[12 + 4..12 + 8].copy_from_slice(&[3, 2, 1, 255]);
        let frame = Frame::new(2, 2, 12, &bgra).unwrap();
        assert_eq!(frame.rgb(1, 1), [1, 2, 3]);
        assert_eq!(frame.rgb(0, 0), [0, 0, 0]);
    }

    #[test]
    fn a_bottom_strip_keeps_the_game_areas_coordinates() {
        // A 2x100 game area of which only rows 98 and 99 were read back.
        let mut bgra = vec![0u8; 8 * 2];
        bgra[8 + 4..8 + 8].copy_from_slice(&[3, 2, 1, 255]);
        let frame = Frame::bottom(2, 100, 98, 8, &bgra).unwrap();
        assert_eq!(
            (frame.width(), frame.height(), frame.first_row()),
            (2, 100, 98)
        );
        assert_eq!(frame.rgb(1, 99), [1, 2, 3]);
        assert_eq!(frame.rgb(0, 98), [0, 0, 0]);
        assert!(
            Frame::bottom(2, 100, 97, 8, &bgra).is_none(),
            "3 rows need 24 bytes"
        );
        assert!(Frame::bottom(2, 100, 101, 8, &bgra).is_none());
    }

    #[test]
    fn a_frame_declares_its_backing_scale_in_whole_game_pixels() {
        let bgra = vec![0u8; 4 * 4 * 2];
        let frame = Frame::bottom(4, 6, 4, 16, &bgra).unwrap();
        assert_eq!(frame.scale(), 1);
        assert_eq!(frame.with_scale(2).map(|f| f.scale()), Some(2));
        assert!(frame.with_scale(0).is_none());
        assert!(
            frame.with_scale(4).is_none(),
            "6 rows aren't whole 4x4 blocks"
        );
    }

    #[test]
    fn a_frame_narrows_to_its_bottom_rows() {
        let mut bgra = vec![0u8; 8 * 4];
        bgra[8 * 3..8 * 3 + 4].copy_from_slice(&[3, 2, 1, 255]);
        let frame = Frame::new(2, 4, 8, &bgra).unwrap().with_scale(2).unwrap();
        let bottom = frame.bottom_rows(2);
        assert_eq!(
            (bottom.first_row(), bottom.height(), bottom.scale()),
            (2, 4, 2)
        );
        assert_eq!(bottom.rgb(0, 3), [1, 2, 3]);
        assert_eq!(frame.bottom_rows(9).first_row(), 0, "all it holds");
        let strip = Frame::bottom(2, 4, 3, 8, &bgra[24..]).unwrap();
        assert_eq!(strip.bottom_rows(2).first_row(), 3);
    }

    #[test]
    #[should_panic(expected = "outside the frame's rows 98..100")]
    fn rows_above_a_bottom_strip_are_not_there() {
        let bgra = vec![0u8; 8 * 2];
        Frame::bottom(2, 100, 98, 8, &bgra).unwrap().rgb(0, 97);
    }
}
