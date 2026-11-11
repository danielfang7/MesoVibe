//! Map name reader: which map the player is on, from the name at the top of the minimap.
//!
//! The minimap sits at the game area's top-left corner. Under its "MINI MAP" title bar is a band
//! of one flat color with the map's icon and, right of it, the region and the map's name in white:
//! "Victoria Road" over "East Rocky Mountain V", cut off where the minimap ends. The name isn't
//! read as text: its font isn't the pickup notices', and Wine draws it thinner on a Mac. A map is
//! known by the exact shape of its name, the way an item is, so the same map gives the same key
//! every time on the same computer, at every window size.
//!
//! The band is found by its look, not at fixed pixels: the game puts the minimap at the very edge at
//! 1280x720 and 20 pixels in at 1366x768, and Wine places the game a pixel or two off.
//! When it isn't there (the minimap closed, collapsed to its title bar, or another window over it)
//! or something dark is drawn over the name, the name is unreadable: never a guess.
//!
//! The engine grabs the top of the game area only now and then: [`MapFollower`] asks for a whole
//! frame at the first capture, after each map change (the screen fades to black and back), and
//! every 30 seconds in case a map changed without a fade. It reports a map once two reads in a row
//! agree, so a minimap still fading in isn't taken for another map.

use crate::frame::Frame;
use crate::recognizers::NamePicture;
use crate::unstretch::{self, No1x};

/// The corner of the 1x picture the reader looks in. The widest minimap's band fits.
const CORNER_COLUMNS: u32 = 512;
const CORNER_ROWS: u32 = 96;
/// A row inside the name band, between its two lines of text.
const PROBE_ROW: u32 = 40;
/// Rows that must be the band's color in the strip left of the icon for the band to be there: the
/// height of its two lines of text.
const BAND_ROWS: std::ops::RangeInclusive<u32> = 26..=56;
/// The band's strip left of the icon starts left of this column, wherever the game puts the
/// minimap.
const MAX_BAND_LEFT: u32 = 80;
/// The text starts within this many columns of that strip, past the icon.
const MAX_ICON: u32 = 64;
/// The icon's frame is this dark in every channel.
const ICON_FRAME: u8 = 64;
/// The "MINI MAP" title bar above the band is this light in every channel.
const TITLE_BAR: u8 = 220;
/// Rows between the title bar and the band: a Mac draws a line of another shade between them.
const TITLE_GAP: u32 = 2;
/// How far, in any channel, a pixel may be from the band's color and still be the band.
const SAME: u8 = 8;
/// How far, in some channel, a pixel must be from the band's color to be part of the name.
const INK: u8 = 32;
/// A pixel this dark in every channel is neither the name nor its shadow: something is drawn over
/// the band.
const DARK: u8 = 48;

/// Frames between reads of the map's name while nothing says it changed: 30 s at four frames a
/// second.
pub const CHECK_EVERY: u32 = 120;
/// Reads after a map change that may disagree before the map is reported unknown: 2 s at four
/// frames a second, for the minimap to fade in.
const SETTLE_READS: u32 = 8;

/// A map, by its name as the minimap draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapName {
    /// The name's exact shape as text (the region's and the map's lines): the same map gives the
    /// same key every time on the same computer.
    pub key: String,
    /// The map's own name, its last line, as the game drew it.
    pub picture: NamePicture,
}

/// Why the map's name couldn't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapUnreadable {
    /// No minimap name band where it can be: the minimap is closed, collapsed, or covered by
    /// another window.
    NotFound,
    /// Something dark is drawn over the name.
    Covered,
    /// The band is there with no name in it.
    NoName,
    /// The frame has no 1x picture to read, or doesn't hold the game area's top rows.
    Unsupported(No1x),
}

/// The map's name in a frame that holds the whole game area.
pub fn read(frame: &Frame) -> Result<MapName, MapUnreadable> {
    unstretch::with_top_left_1x(frame, CORNER_ROWS, CORNER_COLUMNS, read_1x)
        .map_err(MapUnreadable::Unsupported)
        .and_then(|r| r)
}

/// Every channel of `a` within `by` of `b`'s.
fn near(a: [u8; 3], b: [u8; 3], by: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= by)
}

/// Where the name is: the band's color, and the band's rectangle right of the icon.
struct Band {
    color: [u8; 3],
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
}

/// The name band whose strip left of the icon is at column `x`, when the minimap is there: a flat
/// strip light enough for white letters, the icon in its dark frame, and the band again with the
/// white title bar right above it.
fn band_at(picture: &Frame, x: u32, width: u32, height: u32) -> Option<Band> {
    let color = picture.rgb(x, PROBE_ROW);
    let is_band = |x: u32, y: u32| near(picture.rgb(x, y), color, SAME);
    let clear = |x: u32| BAND_ROWS.clone().all(|y| is_band(x, y));
    let shade = color.iter().max().copied().unwrap_or(0);
    let headroom = color.iter().min().copied().unwrap_or(0);
    if shade < DARK || headroom > 200 || !(x - 1..=x + 1).all(clear) {
        return None;
    }
    let limit = (x + MAX_ICON).min(width);
    let icon = (x..limit).find(|&c| !clear(c))?;
    let left = (icon..limit).find(|&c| clear(c))?;
    let framed = (icon..left).any(|c| {
        BAND_ROWS
            .clone()
            .any(|y| picture.rgb(c, y).iter().all(|&v| v < ICON_FRAME))
    });
    // The band's rows, where the text starts: the title bar is above, the map's picture below.
    let top = (0..PROBE_ROW)
        .rev()
        .take_while(|&y| is_band(left, y))
        .last()
        .unwrap_or(PROBE_ROW);
    let bottom = (PROBE_ROW..height)
        .take_while(|&y| is_band(left, y))
        .last()
        .map_or(PROBE_ROW, |y| y + 1);
    let titled = (top.saturating_sub(TITLE_GAP + 1)..top)
        .any(|y| picture.rgb(left, y).iter().all(|&v| v >= TITLE_BAR));
    if !framed || !titled {
        return None;
    }
    // The band's right edge, along its first rows, above the text.
    let right = (left..width)
        .take_while(|&x| is_band(x, top + 1))
        .last()
        .map_or(left, |x| x + 1);
    Some(Band {
        color,
        left,
        right,
        top,
        bottom,
    })
}

fn read_1x(picture: &Frame) -> Result<MapName, MapUnreadable> {
    let width = picture.width().min(CORNER_COLUMNS);
    let height = picture.height().min(CORNER_ROWS);
    if width <= MAX_BAND_LEFT + MAX_ICON || height <= *BAND_ROWS.end() {
        return Err(MapUnreadable::NotFound);
    }
    let Band {
        color: band,
        left,
        right,
        top,
        bottom,
    } = (1..MAX_BAND_LEFT)
        .find_map(|x| band_at(picture, x, width, height))
        .ok_or(MapUnreadable::NotFound)?;

    let mut ink = Vec::new();
    for y in top..bottom {
        for x in left..right {
            let p = picture.rgb(x, y);
            if p.iter().all(|&c| c < DARK) {
                return Err(MapUnreadable::Covered);
            }
            if !near(p, band, INK) {
                ink.push((x, y));
            }
        }
    }
    let (Some(x0), Some(x1)) = (ink.iter().map(|p| p.0).min(), ink.iter().map(|p| p.0).max())
    else {
        return Err(MapUnreadable::NoName);
    };
    let (y0, y1) = (ink[0].1, ink[ink.len() - 1].1);

    // FNV-1a over the name's ink, row by row within its bounds: the same pixels, the same key.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |b: u8| {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    w.to_le_bytes()
        .into_iter()
        .chain(h.to_le_bytes())
        .for_each(&mut feed);
    let mut bits = vec![0u8; (w * h).div_ceil(8) as usize];
    for &(x, y) in &ink {
        let i = ((y - y0) * w + (x - x0)) as usize;
        bits[i / 8] |= 1 << (i % 8);
    }
    bits.into_iter().for_each(&mut feed);

    // The last line: the rows of ink after the last blank row.
    let line_top = (y0..=y1)
        .rev()
        .take_while(|&y| ink.iter().any(|p| p.1 == y))
        .last()
        .unwrap_or(y1);
    let line: Vec<&(u32, u32)> = ink.iter().filter(|p| p.1 >= line_top).collect();
    let lx0 = line.iter().map(|p| p.0).min().unwrap_or(x0);
    let lx1 = line.iter().map(|p| p.0).max().unwrap_or(x1);
    Ok(MapName {
        key: format!("{hash:016x}"),
        picture: picture_of(picture, band, (lx0, line_top), (lx1, y1), (top, bottom)),
    })
}

/// The letters from `from` to `to` with a pixel of margin inside the band's rows, as how much
/// lighter than the band each pixel is: the white letters show, their dark shadow doesn't.
fn picture_of(
    picture: &Frame,
    band: [u8; 3],
    from: (u32, u32),
    to: (u32, u32),
    (top, bottom): (u32, u32),
) -> NamePicture {
    // The channel the band is darkest in leaves the letters the most room.
    let k = (0..3).min_by_key(|&k| band[k]).unwrap_or(0);
    let room = 255 - u32::from(band[k]);
    let (x0, x1) = (from.0.saturating_sub(1), to.0 + 1);
    let (y0, y1) = (
        from.1.saturating_sub(1).max(top),
        (to.1 + 1).min(bottom - 1),
    );
    let (width, height) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut coverage = Vec::with_capacity((width * height) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let lighter = u32::from(picture.rgb(x, y)[k].saturating_sub(band[k]));
            coverage.push((lighter * 255 / room).min(255) as u8);
        }
    }
    NamePicture {
        width,
        height,
        coverage,
    }
}

/// Why the follower wants a whole frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// The map may have changed: at the first capture, and after the screen went dark.
    Change,
    /// A check, in case a map changed without a fade.
    Check,
}

/// Follows which map the player is on, from frame to frame: when to grab the whole game area to
/// read the map's name, and when a new name has settled.
#[derive(Debug, Clone, Default)]
pub struct MapFollower {
    /// The map last reported: `None` before the first report, `Some(None)` for a map whose name
    /// couldn't be read.
    map: Option<Option<String>>,
    want: Option<Want>,
    /// The last read while waiting for two in a row to agree: its key, or `None` for unreadable.
    last: Option<Option<String>>,
    reads: u32,
    /// Frames since the last whole frame.
    since: u32,
    /// The screen was dark in the last frame: the map is changing.
    dark: bool,
}

impl MapFollower {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the next frame should hold the whole game area.
    pub fn wants_whole_frame(&self) -> bool {
        self.want.is_some()
    }

    /// Follows one frame the engine read: `dark` says the screen was dark in it (a map change
    /// fades to black and back). Returns a map that just settled: its name, or `None` when the
    /// new map's name can't be read.
    pub fn observe(&mut self, frame: &Frame, dark: bool) -> Option<Option<MapName>> {
        if dark {
            self.dark = true;
            self.finish();
            return None;
        }
        if std::mem::take(&mut self.dark) {
            self.start(Want::Change);
            return None;
        }
        if frame.first_row() != 0 {
            self.since = self.since.saturating_add(1);
            if self.want.is_none() {
                if self.map.is_none() {
                    self.start(Want::Change);
                } else if self.since >= CHECK_EVERY {
                    self.start(Want::Check);
                }
            }
            return None;
        }
        self.since = 0;
        let want = self.want.unwrap_or(match self.map {
            None => Want::Change,
            Some(_) => Want::Check,
        });
        let read = read(frame).ok();
        let key = read.as_ref().map(|name| name.key.clone());
        // Nothing new, or a check that can't see the name: no news.
        if self.map.as_ref() == Some(&key) || (want == Want::Check && key.is_none()) {
            self.finish();
            return None;
        }
        if self.last.as_ref() == Some(&key) {
            self.finish();
            self.map = Some(key);
            return Some(read);
        }
        self.last = Some(key);
        self.reads += 1;
        if self.reads >= SETTLE_READS {
            self.finish();
            // After a map change, a name that never settled is an unknown map.
            if want == Want::Change && self.map != Some(None) {
                self.map = Some(None);
                return Some(None);
            }
        }
        None
    }

    fn start(&mut self, want: Want) {
        self.want = Some(want);
        self.last = None;
        self.reads = 0;
    }

    fn finish(&mut self) {
        self.want = None;
        self.last = None;
        self.reads = 0;
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::recognizers::exp_bar::test_support::{Canvas, Font};

    const CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ. ";
    pub(crate) const BAND: [u8; 3] = [0x99, 0xbb, 0xcc];
    const SHADOW: [u8; 3] = [0x52, 0x75, 0x9c];

    /// The minimap at the game's top-left corner, drawn the way the game draws it.
    pub(crate) struct Minimap {
        pub(crate) region: String,
        pub(crate) name: String,
        /// How wide the minimap is: a long name is cut off at its right edge.
        pub(crate) width: u32,
        /// Its top-left corner: 20 pixels in at 1366x768, at the edge at 1280x720, and a pixel or
        /// two off where Wine placed the game.
        pub(crate) at: (u32, u32),
        pub(crate) font: Font,
    }

    impl Minimap {
        pub(crate) fn new(region: &str, name: &str) -> Minimap {
            Minimap {
                region: region.to_string(),
                name: name.to_string(),
                width: 220,
                at: (20, 1),
                font: Font::new(CHARS, 5),
            }
        }

        /// Draws it over `canvas`.
        pub(crate) fn draw(&self, canvas: &mut Canvas) {
            let ((left, top), w) = (self.at, self.width);
            canvas.fill(left, top, w, 72, [0x55, 0x77, 0x99]);
            // The title bar, white with dark letters.
            canvas.fill(left + 4, top + 4, w - 8, 14, [255, 255, 255]);
            canvas.fill(left + 8, top + 9, 40, 5, [20, 20, 20]);
            // The band, with the map's icon in a dark box.
            canvas.fill(left + 1, top + 18, w - 2, 52, BAND);
            canvas.fill(left + 7, top + 22, 36, 36, [16, 16, 32]);
            canvas.fill(left + 12, top + 27, 26, 26, [230, 190, 40]);
            // The map's picture under the band.
            canvas.fill(left + 1, top + 70, w - 2, 2, [255, 255, 255]);
            for (text, line) in [(&self.region, 29), (&self.name, 43)] {
                self.text(canvas, text, left + 51, top + line, left + w - 1);
            }
        }

        /// White letters with a shadow down and to the right, cut off at `right`.
        fn text(&self, canvas: &mut Canvas, text: &str, x: u32, top: u32, right: u32) {
            let mut x = x;
            let mut core = Vec::new();
            for c in text.chars() {
                for &bits in self.font.columns(c) {
                    for row in 0..7 {
                        if bits >> row & 1 == 1 {
                            core.push((x, top + row));
                        }
                    }
                    x += 1;
                }
                x += 1;
            }
            for &(x, y) in &core {
                if x + 1 < right && !core.contains(&(x + 1, y + 1)) {
                    canvas.set(x + 1, y + 1, SHADOW);
                }
            }
            for &(x, y) in &core {
                if x < right {
                    canvas.set(x, y, [255, 255, 255]);
                }
            }
        }
    }

    /// A 1366x768 game with the minimap drawn, or none.
    pub(crate) fn game(minimap: Option<&Minimap>) -> Canvas {
        let mut canvas = Canvas::new(1366, 768);
        canvas.fill(0, 0, 1366, 120, [120, 160, 220]);
        if let Some(minimap) = minimap {
            minimap.draw(&mut canvas);
        }
        canvas
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::recognizers::exp_bar::test_support::Canvas;

    fn key(canvas: &Canvas) -> Result<String, MapUnreadable> {
        read(&canvas.frame()).map(|name| name.key)
    }

    // Guards a map's identity: the same name gives the same key wherever the game puts the
    // minimap, another name another key. Fails if the key depends on more than the name's shape,
    // or the minimap is only looked for in one place.
    #[test]
    fn a_map_is_known_by_its_name_s_shape() {
        let forest = Minimap::new("Victoria Road", "Forest I");
        let first = key(&game(Some(&forest))).unwrap();
        // Wine's pixel or two, and the very edge of the game area, as at 1280x720.
        for at in [(21, 3), (1, 0)] {
            let moved = Minimap {
                at,
                ..Minimap::new("Victoria Road", "Forest I")
            };
            assert_eq!(key(&game(Some(&moved))), Ok(first.clone()), "at {at:?}");
        }
        let other = Minimap::new("Victoria Road", "Forest II");
        assert_ne!(key(&game(Some(&other))).unwrap(), first);
        let elsewhere = Minimap::new("Ossyria", "Forest I");
        assert_ne!(key(&game(Some(&elsewhere))).unwrap(), first);
        // The picture is the map's own line, white letters on nothing.
        let name = read(&game(Some(&forest)).frame()).unwrap();
        assert!(name.picture.height >= 7 && name.picture.height <= 10);
        assert!(name.picture.coverage.contains(&255));
        assert_eq!(
            name.picture.coverage.len() as u32,
            name.picture.width * name.picture.height
        );
    }

    // Guards the honesty rule: a minimap that isn't there or is drawn over gives no name, never a
    // wrong one.
    #[test]
    fn a_closed_or_covered_minimap_is_unreadable() {
        assert_eq!(key(&game(None)), Err(MapUnreadable::NotFound));
        let empty = Minimap::new("", "");
        assert_eq!(key(&game(Some(&empty))), Err(MapUnreadable::NoName));
        let mut covered = game(Some(&Minimap::new("Victoria Road", "Forest I")));
        covered.fill(100, 44, 12, 8, [0, 0, 0]);
        assert_eq!(key(&covered), Err(MapUnreadable::Covered));
        // A screen faded to black has no band.
        assert_eq!(key(&Canvas::new(1366, 768)), Err(MapUnreadable::NotFound));
    }

    // Guards a name cut off by a narrow minimap: still one key, from what shows.
    #[test]
    fn a_name_cut_off_by_the_minimap_s_edge_still_has_one_key() {
        let narrow = |width| Minimap {
            width,
            ..Minimap::new("Victoria Road", "East Rocky Mountain V")
        };
        let first = key(&game(Some(&narrow(150)))).unwrap();
        assert_eq!(key(&game(Some(&narrow(150)))), Ok(first.clone()));
        assert_ne!(key(&game(Some(&narrow(240)))).unwrap(), first);
    }

    // Guards every window size: the name comes back from a stretched or pixel-doubled frame
    // with the shape it has at 1x, so a map keeps its key when the window changes size.
    #[test]
    fn a_map_has_the_same_key_stretched_and_doubled() {
        let forest = Minimap::new("Victoria Road", "Forest I");
        let canvas = game(Some(&forest));
        let at_1x = key(&canvas).unwrap();
        let rgb: Vec<[u8; 3]> = canvas
            .bgra
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[2], p[1], p[0]])
            .collect();
        let (width, height) = (1920, 1080);
        let bgra = unstretch::stretch(&rgb, canvas.width, width, height, 0);
        let stretched = Frame::new(width, height, width as usize * 4, &bgra).unwrap();
        assert_eq!(read(&stretched).map(|n| n.key), Ok(at_1x.clone()));
        // A Retina Mac: every game pixel a 2x2 block.
        let (w, h) = (canvas.width, canvas.height);
        let mut doubled = vec![0u8; (4 * w * h * 4) as usize];
        for y in 0..2 * h {
            for x in 0..2 * w {
                let from = (((y / 2) * w + x / 2) * 4) as usize;
                let to = ((y * 2 * w + x) * 4) as usize;
                doubled[to..to + 4].copy_from_slice(&canvas.bgra[from..from + 4]);
            }
        }
        let frame = Frame::new(2 * w, 2 * h, (2 * w * 4) as usize, &doubled)
            .and_then(|f| f.with_scale(2))
            .unwrap();
        assert_eq!(read(&frame).map(|n| n.key), Ok(at_1x));
    }

    /// The bottom strip of `canvas`, as most ticks grab it.
    fn strip(canvas: &Canvas) -> Frame<'_> {
        let rows = 200;
        let first = canvas.height - rows;
        let stride = canvas.width as usize * 4;
        Frame::bottom(
            canvas.width,
            canvas.height,
            first,
            stride,
            &canvas.bgra[first as usize * stride..],
        )
        .unwrap()
    }

    // Guards when the map is read and reported: at the first capture, after a fade, and every
    // 30 s; a name is reported once two reads agree, and a name that never settles after a fade
    // is an unknown map. Fails if a map is reported from one read, or a check that can't see
    // the minimap says the map changed.
    #[test]
    fn follows_the_map_from_fade_to_fade() {
        let forest = game(Some(&Minimap::new("Victoria Road", "Forest I")));
        let town = game(Some(&Minimap::new("Victoria Road", "Henesys")));
        let dark = Canvas::new(1366, 768);
        let mut follower = MapFollower::new();
        assert!(!follower.wants_whole_frame());
        assert_eq!(follower.observe(&strip(&forest), false), None);
        assert!(
            follower.wants_whole_frame(),
            "the first capture reads the map"
        );
        assert_eq!(follower.observe(&forest.frame(), false), None, "one read");
        let first = follower.observe(&forest.frame(), false).unwrap().unwrap();
        assert_eq!(Ok(first.key.clone()), key(&forest));
        assert!(!follower.wants_whole_frame());

        // A map change: dark frames, then the new map, read twice.
        assert_eq!(follower.observe(&strip(&dark), true), None);
        assert!(!follower.wants_whole_frame(), "not while it's dark");
        assert_eq!(follower.observe(&strip(&town), false), None);
        assert!(follower.wants_whole_frame());
        assert_eq!(follower.observe(&town.frame(), false), None);
        let town_name = follower.observe(&town.frame(), false).unwrap().unwrap();
        assert_ne!(town_name.key, first.key);

        // A check every 30 s: the same map says nothing; a closed minimap says nothing.
        let closed = game(None);
        for _ in 0..CHECK_EVERY {
            assert!(!follower.wants_whole_frame());
            assert_eq!(follower.observe(&strip(&town), false), None);
        }
        assert!(follower.wants_whole_frame());
        assert_eq!(follower.observe(&closed.frame(), false), None);
        assert!(!follower.wants_whole_frame());

        // A change into a map whose minimap is closed: unknown once it never settles.
        follower.observe(&strip(&dark), true);
        follower.observe(&strip(&closed), false);
        let mut settled = None;
        for _ in 0..SETTLE_READS {
            if let Some(map) = follower.observe(&closed.frame(), false) {
                settled = Some(map);
                break;
            }
        }
        assert_eq!(settled, Some(None), "an unknown map");
        // Opened later: the next check finds it.
        for _ in 0..CHECK_EVERY {
            follower.observe(&strip(&forest), false);
        }
        assert_eq!(follower.observe(&forest.frame(), false), None);
        assert_eq!(
            follower
                .observe(&forest.frame(), false)
                .map(|m| m.map(|n| n.key)),
            Some(Some(first.key))
        );
    }
}
