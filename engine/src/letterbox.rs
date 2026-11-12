//! Full Screen on a display that isn't the game's shape: black bars above and below the picture.
//!
//! In Full Screen the game keeps its picture's shape, scales it to the display's width and fills
//! the rest with black. On a 16:10 Mac screen of 1512x982 points, 1366x768 in Full Screen is a
//! 1512x850 picture with a 66-point bar above it and another below it. The readers measure up
//! from the game area's bottom row and work out the stretch from its height, so the bars must go
//! before they read: [`inside`] cuts them off, and what's left is an ordinary stretched picture.
//!
//! The bottom bar is measured on every frame ([`bottom_bar`]). The engine reads only the bottom
//! of the game window, so the top bar isn't seen; the game centers its picture, so it's taken to
//! be as tall as the bottom one. A windowed game has no bars: its bottom row is the HUD's frame,
//! which is never black all the way across. Nor is the row of black Wine can leave under a game
//! it places a point off ([`MIN_BAR`]), or black that leaves a picture of another shape, like a
//! loading screen's band ([`PICTURE`]).

use crate::capture::{GameWindow, Placement, ScreenRect};
use crate::frame::Frame;

/// How bright the picture's bottom edge must get for the black under it to count as a bar. The
/// HUD's light bottom line is about 129 there in Full Screen, after the game's scaling blends it,
/// and 217 windowed. A screen fading to black for a map change dims it below this, and then the
/// bar isn't measured: dark frames keep the last measurement.
const LIT: u8 = 64;

/// Game rows at the picture's bottom edge that are looked at for [`LIT`]: the HUD's line may
/// fall on the row above a last row the scaling only partly covers.
const EDGE_ROWS: u32 = 2;

/// The shape of the picture the game letterboxes in Full Screen: 1366x768 (1280x720 is the same
/// shape). Bars above and below it leave that shape at the display's full width.
const PICTURE: (u32, u32) = (1366, 768);

/// How far, in game pixels, the picture between the bars may be from [`PICTURE`]'s shape: the game
/// rounds the picture's height, and the bar above it may be a point taller than the one below.
const SHAPE_SLACK: f64 = 2.0;

/// The shortest bar, in game pixels. Wine can place a windowed game up to a point off where its
/// window puts it, which can leave a row of black under it; a Full Screen bar is tens of points
/// (66 on a 1512x982 screen).
const MIN_BAR: u32 = 4;

/// How tall the black bar under the game's picture is, in game pixels: 0 when the picture reaches
/// the frame's bottom. Bar rows are pure black across the frame's whole width, at least
/// [`MIN_BAR`] of them, and leave a picture of [`PICTURE`]'s shape between them and a bar as tall
/// above. `None` when the frame can't tell: black as far up as it holds (a map change's fade, or a
/// bar taller than the rows read), a picture edge too dark to be sure of, a bar that isn't a whole
/// number of game pixels, or black that isn't a bar (Wine's row under a windowed game, a loading
/// screen's band).
pub fn bottom_bar(frame: &Frame) -> Option<u32> {
    let (width, height, first, scale) = (
        frame.width(),
        frame.height(),
        frame.first_row(),
        frame.scale(),
    );
    let black = |y: u32| (0..width).all(|x| frame.rgb(x, y) == [0, 0, 0]);
    let edge = (first..height).rev().find(|&y| !black(y))?;
    let lit = (edge.saturating_sub(EDGE_ROWS * scale - 1).max(first)..=edge)
        .any(|y| (0..width).any(|x| frame.rgb(x, y).into_iter().max().unwrap_or(0) >= LIT));
    let rows = height - 1 - edge;
    if !lit || !rows.is_multiple_of(scale) {
        return None;
    }
    let bar = rows / scale;
    let picture = f64::from((height / scale).saturating_sub(bar.saturating_mul(2)));
    let shaped = f64::from(width / scale) * f64::from(PICTURE.1) / f64::from(PICTURE.0);
    let letterboxed = bar >= MIN_BAR && (picture - shaped).abs() <= SHAPE_SLACK;
    (bar == 0 || letterboxed).then_some(bar)
}

/// The game's picture inside a frame with bars `bar` game pixels tall above and below it: the
/// frame without those rows, so its bottom row is the picture's. The frame itself for no bars, and
/// `None` when the bars would leave no picture or the frame holds none of its rows.
pub fn inside<'a>(frame: &Frame<'a>, bar: u32) -> Option<Frame<'a>> {
    match bar {
        0 => Some(*frame),
        _ => {
            let rows = bar.checked_mul(frame.scale())?;
            frame.without_rows(rows, rows)
        }
    }
}

/// `window` with its game area cut to the picture between bars `bar` game pixels tall, so that
/// MesoVibe's windows line up with what the readers saw. Unchanged for no bars, or bars that
/// would leave nothing.
pub fn in_window(window: GameWindow, bar: u32) -> GameWindow {
    let Some(height) = window
        .height
        .checked_sub(bar.saturating_mul(2))
        .filter(|&h| h > 0)
    else {
        return window;
    };
    GameWindow {
        height,
        placement: window.placement.map(|placement| Placement {
            area: ScreenRect {
                y: placement.area.y.saturating_add_unsigned(bar),
                height: placement.area.height.saturating_sub(bar.saturating_mul(2)),
                ..placement.area
            },
            ..placement
        }),
        ..window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `rows` rows `width` wide, in captured pixels: dark gray with a light HUD edge on the
    /// picture's bottom row, over `bar` black rows.
    fn strip(width: u32, rows: u32, bar: u32) -> Vec<u8> {
        let mut bgra = vec![0u8; (width * rows * 4) as usize];
        for (y, row) in bgra.chunks_mut(width as usize * 4).enumerate() {
            let color = match rows - y as u32 {
                below if below <= bar => 0,
                below if below == bar + 1 => 220,
                _ => 40,
            };
            for px in row.chunks_mut(4) {
                px.copy_from_slice(&[color, color, color, 255]);
            }
        }
        bgra
    }

    fn frame(bgra: &[u8], width: u32, height: u32, rows: u32, scale: u32) -> Frame<'_> {
        Frame::bottom(width, height, height - rows, width as usize * 4, bgra)
            .and_then(|f| f.with_scale(scale))
            .unwrap()
    }

    // The measured case: 1366x768 in Full Screen on a 1512x982 Retina screen, 66 points of black
    // under the picture. Fails if the bar is misread, the cut picture isn't 1512x850, or a dim
    // last row the scaling only partly covers hides the HUD's light line just above it.
    #[test]
    fn measures_the_bar_under_a_full_screen_picture_and_cuts_both_bars() {
        let (width, height, rows) = (3024, 1964, 400);
        let mut bgra = strip(width, rows, 132);
        let frame = frame(&bgra, width, height, rows, 2);
        assert_eq!(bottom_bar(&frame), Some(66));
        let picture = inside(&frame, 66).unwrap();
        assert_eq!(
            (picture.width(), picture.height(), picture.scale()),
            (3024, 1700, 2)
        );
        assert_eq!(picture.first_row(), 1964 - 400 - 132);
        assert_eq!(picture.rgb(0, 1699), [220, 220, 220], "the HUD's edge");

        // Scaled to 850 rows, the picture's last game row can be a dim blend of the HUD's line
        // and the black under it, with the line itself one game row up.
        let line = width as usize * 4;
        let last = (rows - 133) as usize;
        bgra[(last - 1) * line..(last + 1) * line].fill(30);
        bgra[(last - 3) * line..(last - 1) * line].fill(130);
        assert_eq!(
            bottom_bar(&self::frame(&bgra, width, height, rows, 2)),
            Some(66)
        );
    }

    // A windowed game's bottom row is the HUD. Fails if a picture that reaches the bottom is
    // given a bar, or isn't read as it is.
    #[test]
    fn a_picture_that_reaches_the_bottom_has_no_bar() {
        let bgra = strip(1366, 100, 0);
        let frame = frame(&bgra, 1366, 768, 100, 1);
        assert_eq!(bottom_bar(&frame), Some(0));
        assert_eq!(inside(&frame, 0).map(|f| f.height()), Some(768));
    }

    // A map change fades everything to black. Fails if a fade is taken for a bar, which would
    // cut off the HUD when the picture comes back.
    #[test]
    fn a_black_or_fading_screen_measures_nothing() {
        let black = vec![0u8; 1366 * 100 * 4];
        assert_eq!(bottom_bar(&frame(&black, 1366, 768, 100, 1)), None);
        // Fading: the picture's dimmed bottom row is too dark to be sure where it ends.
        let mut dim = strip(1366, 100, 10);
        for px in dim.chunks_mut(4) {
            px[..3].iter_mut().for_each(|c| *c /= 8);
        }
        assert_eq!(bottom_bar(&frame(&dim, 1366, 768, 100, 1)), None);
    }

    // One pixel that isn't black in a bottom row means the picture reaches it. Fails if a bar is
    // measured under a picture whose bottom row is mostly dark.
    #[test]
    fn a_single_lit_pixel_ends_the_bar() {
        let mut bgra = strip(1366, 100, 10);
        let i = ((99 * 1366 + 700) * 4) as usize;
        bgra[i..i + 3].copy_from_slice(&[90, 90, 90]);
        assert_eq!(bottom_bar(&frame(&bgra, 1366, 768, 100, 1)), Some(0));
    }

    // At backing scale 2 a bar is whole points. Fails if an odd number of black pixel rows is
    // turned into a bar that splits the game's pixels.
    #[test]
    fn a_bar_must_be_whole_game_pixels() {
        let bgra = strip(200, 100, 21);
        assert_eq!(bottom_bar(&frame(&bgra, 200, 400, 100, 2)), None);
    }

    // Only black that leaves a letterboxed picture's shape is a bar. Fails if the row of black Wine
    // can leave under a windowed game, or a loading screen's black band, is taken for one: that
    // would cut the picture short, misjudge its stretch and move MesoVibe's windows.
    #[test]
    fn only_black_that_leaves_the_pictures_shape_is_a_bar() {
        // (window width, height, black rows, scale) that aren't bars.
        for (width, height, bar, scale) in [
            (1366, 768, 1, 1),
            (1366, 768, 2, 2),
            (1280, 720, 1, 1),
            (1920, 1080, 1, 2),
            (1366, 768, 40, 1),
            (1512, 982, 40, 1),
            (1512, 982, 80, 2),
        ] {
            let bgra = strip(width * scale, 120 * scale, bar * scale);
            let measured = bottom_bar(&frame(
                &bgra,
                width * scale,
                height * scale,
                120 * scale,
                scale,
            ));
            assert_eq!(
                measured, None,
                "{bar} rows under {width}x{height} at scale {scale}"
            );
        }
        // Letterboxed pictures: 1512x982 (MacBook Pro 14), 1440x900, 1728x1117 (MacBook Pro 16).
        for (width, height, bar) in [(1512, 982, 66), (1440, 900, 45), (1728, 1117, 73)] {
            let bgra = strip(width, 120, bar);
            assert_eq!(
                bottom_bar(&frame(&bgra, width, height, 120, 1)),
                Some(bar),
                "{width}x{height}"
            );
        }
    }

    // MesoVibe's windows line up with the picture, not the black around it. Fails if the area
    // isn't moved down by the top bar and shortened by both.
    #[test]
    fn the_game_area_on_screen_is_the_picture() {
        let area = ScreenRect {
            x: 0,
            y: 0,
            width: 1512,
            height: 982,
        };
        let window = GameWindow {
            id: 1,
            width: 1512,
            height: 982,
            scale: 2,
            minimized: false,
            placement: Some(Placement {
                frame: area,
                area,
                front: true,
            }),
        };
        let cut = in_window(window, 66);
        assert_eq!(cut.height, 850);
        assert_eq!(
            cut.placement.map(|p| (p.area, p.frame)),
            Some((
                ScreenRect {
                    x: 0,
                    y: 66,
                    width: 1512,
                    height: 850
                },
                area
            ))
        );
        assert_eq!(in_window(window, 0), window);
        assert_eq!(in_window(window, 491), window, "nothing left");
    }
}
