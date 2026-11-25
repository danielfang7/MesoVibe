//! A frame's game pixels for the readers that work in game pixels: the frame itself at 1x, or at a
//! backing scale one pixel of each block, so a Retina frame is read where its blocks are rather
//! than undone whole.

use crate::frame::Frame;
use crate::glyph::Rect;
use crate::unstretch;

/// A frame's game pixels, whole or its bottom rows (a strip), in game coordinates.
#[derive(Clone, Copy)]
pub(crate) struct Pixels<'a> {
    frame: Frame<'a>,
    scale: u32,
    x0: u32,
    y0: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The first game row the frame holds: 0 for a whole frame.
    pub(crate) top: u32,
}

impl<'a> Pixels<'a> {
    /// `blocks`: where the blocks start, when a reader of the same frame found it. Otherwise it's
    /// found in the bottom `hud_rows` game rows, which a Mac draws as whole blocks, unlike the top
    /// rows under a Mac window's title bar.
    pub(crate) fn of(
        frame: Frame<'a>,
        blocks: Option<(u32, u32)>,
        hud_rows: u32,
    ) -> Option<Pixels<'a>> {
        let scale = frame.scale();
        let (x0, y0) = match scale {
            1 => (0, 0),
            _ => blocks.or_else(|| {
                let rows = unstretch::scaled_rows(hud_rows, scale);
                unstretch::block_offset(&frame.bottom_rows(rows))
            })?,
        };
        (x0 < scale && y0 < scale).then(|| Pixels {
            frame,
            scale,
            x0,
            y0,
            width: frame.width() / scale,
            height: frame.height() / scale,
            top: frame.first_row().saturating_sub(y0).div_ceil(scale),
        })
    }

    /// The pixel at a place the frame holds.
    pub(crate) fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        self.frame
            .rgb(self.x0 + self.scale * x, self.y0 + self.scale * y)
    }

    /// The pixel at (`x`, `y`), or `None` outside what the frame holds.
    pub(crate) fn get(&self, x: i64, y: i64) -> Option<[u8; 3]> {
        let inside = (0..i64::from(self.width)).contains(&x)
            && (i64::from(self.top)..i64::from(self.height)).contains(&y);
        inside.then(|| self.rgb(x as u32, y as u32))
    }

    /// A region's game pixels as a 1x frame of its own, for the glyph helpers.
    pub(crate) fn copy(&self, region: Rect) -> Option<Vec<u8>> {
        let fits = region.x.checked_add(region.width)? <= self.width
            && region.y >= self.top
            && region.y.checked_add(region.height)? <= self.height;
        fits.then(|| {
            let mut bgra = Vec::with_capacity((region.width * region.height * 4) as usize);
            for y in region.y..region.y + region.height {
                for x in region.x..region.x + region.width {
                    let [r, g, b] = self.rgb(x, y);
                    bgra.extend_from_slice(&[b, g, r, 255]);
                }
            }
            bgra
        })
    }
}
