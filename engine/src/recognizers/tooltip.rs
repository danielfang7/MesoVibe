//! Item tooltips: the item the player points at in the inventory, an NPC shop, storage or the
//! equipment window, and where its tooltip box sits, for the price label drawn under it.
//!
//! **The box.** An item tooltip is a navy rectangle the scene shows through: every pixel under it
//! is drawn as `(9, 11, 58) + 0.29 × the pixel behind` (white behind gives (83, 85, 132), black
//! (9, 11, 58)). One pixel in from its outer edge runs a ring the tint leaves out, so each edge
//! is a tinted line, an untinted line, then the tinted inside, and its corners are cut. Measured
//! on recorded captures at 1280x720 and 1366x768, on Windows and a Mac:
//!
//! - the name starts with a light blue dot, (85, 204, 255) in a 13-pixel diamond, 11 pixels right
//!   of the box's left edge and 16 rows below its top (17 for an equip);
//! - the box's right edge is 235 pixels right of its left edge for an equip and 287 for other
//!   items, further only for a long name;
//! - hovering an equip that's worn in the same slot shows a second box 3 pixels to the right of
//!   the first, for the worn item. The hovered item is always the left one, even when the pair is
//!   pushed in from the game's right edge, and the worn item's box is never reported;
//! - at the game's edges the box is pushed back inside, never cut;
//! - a quick slot's tooltip opens above the cursor, its bottom-left corner on the quick-slot panel.
//!   Quick slots get no label, so those are left out.
//!
//! The box is found from the dot: its left and top edges from where the dot sits, its right edge
//! from the column where an edge's pattern holds down the box's first rows, and its bottom from
//! where the left and right edges end, the bottom edge's own pattern, and the untinted row above
//! it. An edge's pattern only holds where the scene behind is smooth for three pixels across it,
//! so each edge is a vote along its length.
//!
//! **The name** is bold white, or orange for an item with enhancements, with " (+N)" after it.
//! Men's and women's clothes add " (M)" or " (F)"; the item list leaves that out, so it picks the
//! men's or women's item. Its pixels are letter where the red channel is more than halfway from
//! the tinted background to full, and letters are cut at blank columns; a few pairs of letters
//! touch ("rt") and are read as one piece.
//!
//! **Naming** happens on the player's device: nothing is typed and no game pixels ship. A name's
//! letters are learned from the item list ([`crate::game_data::items`]) itself. While few letters
//! are known, a name whose repeated shapes (two pieces exactly alike are the same letter) fit
//! exactly one item's name teaches each piece its letter. A piece's letter counts once two
//! different names taught it the same, or one name with at least three repeats (which a name the
//! item list doesn't have rarely fits by chance); a piece taught two different letters is dropped.
//! From then on a name is read against every listed name, scoring how close each piece is to the
//! shapes learned for that name's letter, and the best name is taken only when every piece is
//! close, every letter has learned shapes (at most one piece may be letters never seen touching,
//! as wide as those letters), and it beats the next name clearly. Capital I and small l are drawn
//! alike, so names are compared with the two folded together.
//!
//! Names the item list doesn't have (cash items, skills) aren't named. Given the names the game
//! shows that aren't items ([`TooltipReader::with_other_names`]: the price service's cash items),
//! they compete like any name and are never reported, so a cash item one letter from a listed
//! item ("Black Kitty" and the listed "Black Nitty") isn't read as it; and then a name whose other
//! letters are all known, which only one name in the game fits, teaches its last letter too.
//!
//! **Trust.** An item counts once two whole frames in a row show a box of the same size with the
//! same name ([`Tooltip::confirmed`]); it's gone on the first frame where the box isn't. The box may
//! have moved between the two: a tooltip follows the cursor, and a hand resting on an item drifts
//! a few pixels between most frames. A sweep across the grid shows a new item in each frame, so it
//! still shows nothing.
//!
//! **Sizes.** Only game areas of 1280x720 and 1366x768 drawn at the game's own pixel size are
//! read, the sizes the meso total reads whole frames at. A Retina Mac's frame is read one pixel
//! from each block, where the HUD reader found the blocks start.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use serde::Serialize;

use super::pixels::Pixels;
use crate::frame::Frame;
use crate::game_data;
use crate::glyph::{Glyph, Rect};

/// The game sizes tooltips are read at.
const SIZES: [(u32, u32); 2] = [(1280, 720), (1366, 768)];
/// Game rows at the bottom of the frame where its pixel blocks are found, as the meso reader does.
const HUD_ROWS: u32 = 40;

/// The dot before an item's name, and how far its colour may be off (a Mac's colour conversion).
const DOT: [u8; 3] = [85, 204, 255];
const DOT_TOLERANCE: u8 = 4;
/// The dot's pixels around its middle, and the pixels around it that aren't the dot.
const DOT_CORE: [(i32, i32); 13] = [
    (0, -2),
    (-1, -1),
    (0, -1),
    (1, -1),
    (-2, 0),
    (-1, 0),
    (0, 0),
    (1, 0),
    (2, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
    (0, 2),
];
const DOT_AROUND: [(i32, i32); 8] = [
    (-3, 0),
    (3, 0),
    (0, -3),
    (0, 3),
    (-2, -2),
    (2, -2),
    (-2, 2),
    (2, 2),
];
/// From the dot's middle to the box's left edge, and up to its top edge (equips sit a row lower).
const DOT_TO_LEFT: u32 = 11;
const DOT_TO_TOP: [u32; 2] = [16, 17];
/// The box's tint: `TINT_BASE + background × TINT_SPAN / 255`.
const TINT_BASE: [f32; 3] = [9.0, 11.0, 58.0];
const TINT_SPAN: f32 = 74.0;
/// How far, in background levels, the background seen through an edge's tinted lines may differ
/// from the untinted line between them: the scene is rarely flat across three pixels.
const EDGE_TOLERANCE: f32 = 36.0;
/// The untinted line must differ from its own tinted look by this much in some channel, or a
/// navy scene (which the tint barely changes) would look like an edge everywhere.
const EDGE_CONTRAST: f32 = 16.0;
/// How far right of the left edge an item tooltip's right edge is: equips, and other items.
const WIDTHS: [u32; 2] = [235, 287];
/// From the hovered item's right edge to the left edge of the worn item's box beside it.
const PAIR_GAP: u32 = 3;
/// The name's rows around the dot's middle, and its columns from the dot.
const NAME_ROWS: (i32, i32) = (-6, 9);
const NAME_FROM_DOT: u32 = 4;
/// A gap between letters this wide is a space; one wider ends the name.
const SPACE: u32 = 4;
const NAME_END: u32 = 15;
/// The quick-slot panel's top-left corner, as quick_slots.rs finds it: right of the 800-pixel
/// HUD's left edge, up from the game area's bottom; and its size.
const PANEL_DX: u32 = 652;
const PANEL_UP: u32 = 144;
const PANEL: (u32, u32) = (140, 68);

/// Reading a name: the cost of a letter no shape was learned for yet, of a piece read as 2 or
/// 3 touching letters never seen together, and the most a cost can be.
const UNSEEN: u32 = 6;
const TOUCH: u32 = 7;
const CAP: u32 = 12;
/// A name is taken when each piece is at most `CLOSE` from its letter's shapes, every letter has
/// learned shapes, at most `MAX_NEW_TOUCHING` piece is letters never seen touching (as wide as
/// those letters), and it beats the next name by `MARGIN`. A letter no shape was learned for
/// could be any letter: "Black Kitty", a cash item, would read as the listed "Black Nitty".
const CLOSE: u32 = 4;
const MAX_NEW_TOUCHING: u32 = 1;
const MARGIN: u32 = 6;
/// A decoded name's pieces this close to their letter's shapes are learned as more of its shapes.
const TEACH: u32 = 2;
/// Different names that must teach a piece the same letter before it counts; one is enough when
/// its pattern has at least `STRONG` repeats (pieces exactly like an earlier one), which the item
/// list's names rarely fit by chance.
const NAMES_TO_CONFIRM: usize = 2;
const STRONG: usize = 3;
/// What a trusted glyph kept from before counts as having been taught by: names no list has.
const KEPT: [usize; NAMES_TO_CONFIRM] = [usize::MAX, usize::MAX - 1];
/// A piece may stand for up to this many touching letters.
const MAX_TOUCHING: usize = 3;

/// The hovered item's tooltip in one whole frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tooltip {
    /// The box's outer edges, in game pixels.
    pub rect: Rect,
    /// The items with the tooltip's name (more than one when the game has several items of that
    /// name), or `None` when the name wasn't read for sure.
    pub items: Option<Vec<u32>>,
    /// The name as the item list writes it, when read.
    pub name: Option<String>,
    /// The last whole frame showed a box of the same size with the same items, wherever the cursor
    /// moved it: only then should anything be shown for it.
    pub confirmed: bool,
    /// Why the name wasn't read, when it wasn't.
    pub unsure: Option<Unsure>,
}

/// Why a tooltip's name wasn't read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unsure {
    /// A quick slot's tooltip: no label there.
    QuickSlot,
    /// Nothing was cut from the name's row, or a piece was too wide to be letters.
    NoLetters,
    /// No listed name fits (a cash item, a skill, or a letter misread).
    NoFit,
    /// Too many of the name's letters haven't been learned yet.
    Learning,
    /// A piece isn't close to its letter's shapes.
    Unclear,
    /// Two listed names fit about as well.
    TwoNames,
    /// A men's or women's item whose (M) or (F) wasn't read.
    Gender,
}

/// What one whole frame showed: the hovered item's tooltip, if any.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TooltipReading {
    pub tooltip: Option<Tooltip>,
}

/// A name the reader tells apart, as the decoder reads it: an item's, or one the game shows that
/// isn't an item (no ids), which is never reported.
struct Name {
    text: String,
    /// Its letters without spaces, capital I as small l.
    letters: Vec<char>,
    /// Where each word after the first starts, in `letters`.
    starts: HashSet<usize>,
    words: usize,
    ids: Vec<u32>,
}

/// A piece cut from a name: its glyph's id, whether a space comes before it, and how many
/// touching letters it could hold (a letter is at least 4 pixels wide, with its gap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Piece {
    glyph: u32,
    space: bool,
    letters: usize,
}

/// A reading's pieces and the letters known for them then.
type PatternKey = (Vec<Piece>, Vec<Option<String>>);

/// The decoder's verdict on one listed name.
#[derive(Debug, Clone, Copy)]
struct Fit {
    cost: u32,
    /// Letters with no shapes learned yet.
    unseen: u32,
    /// Pieces read as letters never seen touching.
    touching: u32,
    worst: u32,
    name: usize,
}

/// Reads tooltips from whole frames. Keeps what it learned of the name font for the session, and
/// across sessions through [`TooltipReader::knowledge`].
#[derive(Clone)]
pub struct TooltipReader {
    lexicon: Arc<Lexicon>,
    /// Whether names that aren't items were given to tell apart ([`TooltipReader::with_other_names`]).
    others: bool,
    /// Every glyph seen, by id.
    glyphs: Vec<Glyph>,
    glyph_ids: HashMap<Glyph, u32>,
    /// For each glyph, the letters each listed name its pattern fit taught it.
    taught: HashMap<u32, HashMap<String, HashSet<usize>>>,
    /// Letters (one, or a few drawn touching) and the glyphs learned for them.
    shapes: HashMap<String, Vec<u32>>,
    /// Each glyph's distance to the nearest learned shape of each letter.
    distances: HashMap<u32, HashMap<String, u32>>,
    /// Decoded names, by the pieces read and how many shapes were known then.
    decoded: HashMap<(Vec<Piece>, usize), Vec<Fit>>,
    /// Names whose pattern was strong enough to confirm the letters they taught on their own.
    strong: HashSet<usize>,
    /// The one listed name each reading's pattern fit, by the pieces and the letters known then.
    patterns: HashMap<PatternKey, Option<(usize, Vec<String>, bool)>>,
    learned: usize,
    /// The last whole frame's hovered box and items, for two frames in a row.
    last: Option<(Rect, Option<Vec<u32>>)>,
}

impl Default for TooltipReader {
    fn default() -> Self {
        Self::new()
    }
}

impl TooltipReader {
    pub fn new() -> Self {
        TooltipReader {
            lexicon: Arc::clone(item_lexicon()),
            others: false,
            glyphs: Vec::new(),
            glyph_ids: HashMap::new(),
            taught: HashMap::new(),
            shapes: HashMap::new(),
            distances: HashMap::new(),
            decoded: HashMap::new(),
            patterns: HashMap::new(),
            strong: HashSet::new(),
            learned: 0,
            last: None,
        }
    }

    /// A reader that also tells items apart from `names` the game shows that aren't items (cash
    /// items the price service lists): see [`TooltipReader::set_other_names`].
    pub fn with_other_names<'a>(
        mut self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> TooltipReader {
        self.set_other_names(names);
        self
    }

    /// Tells items apart from `names` the game shows that aren't items (cash items the price
    /// service lists): they're matched like any name and never reported, so a cash item one
    /// letter from a listed item ("Black Kitty", "Black Nitty") isn't read as it. Given them, a
    /// name whose other letters are all known teaches its last letter on its own. Letters already
    /// trusted are kept; what was half learned starts again.
    pub fn set_other_names<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        let names: Vec<&str> = names.into_iter().collect();
        // Names are numbered afresh, so what a glyph was taught by which name no longer holds:
        // a trusted glyph keeps its letters as kept knowledge does, the rest are taught again.
        let trusted: Vec<(u32, String)> = self
            .taught
            .keys()
            .filter_map(|&g| self.confirmed(g).map(|l| (g, l.to_string())))
            .collect();
        self.taught.clear();
        self.strong.clear();
        for (glyph, letters) in trusted {
            self.taught
                .entry(glyph)
                .or_default()
                .entry(letters)
                .or_default()
                .extend(KEPT);
        }
        self.lexicon = Arc::new(Lexicon::new(&names));
        self.others = !names.is_empty();
        self.decoded.clear();
        self.patterns.clear();
    }

    /// Reads one frame. Only a whole frame at a supported size is looked at; anything else forgets
    /// the last box. `blocks`, at a backing scale: where the game's pixel blocks start, as the HUD
    /// reader found ([`super::exp_bar::ExpBarReader::block_offset`]).
    pub fn observe(&mut self, frame: &Frame, blocks: Option<(u32, u32)>) -> TooltipReading {
        let scale = frame.scale();
        let area = (frame.width() / scale, frame.height() / scale);
        let pixels = Pixels::of(*frame, blocks, HUD_ROWS)
            .filter(|px| px.top == 0 && SIZES.contains(&area) && area.1 <= frame.source());
        let Some(px) = pixels else {
            self.last = None;
            return TooltipReading::default();
        };
        let tooltip = self.look(&px);
        self.last = tooltip.as_ref().map(|t| (t.rect, t.items.clone()));
        TooltipReading { tooltip }
    }

    fn look(&mut self, px: &Pixels) -> Option<Tooltip> {
        let dots = find_dots(px);
        // A dot with another on its row to its left is the worn item's box: its name teaches
        // letters, and it's never reported.
        let worn = |&(x, y): &(u32, u32)| dots.iter().any(|&(ox, oy)| oy == y && ox < x);
        for &(x, y) in dots.iter().filter(|d| worn(d)) {
            let right = (x + WIDTHS[0]).saturating_sub(DOT_TO_LEFT).min(px.width);
            let _ = self.read_name(px, (x, y), right);
        }
        let &(dx, dy) = dots.iter().find(|d| !worn(d))?;
        let beside = dots
            .iter()
            .filter(|&&(ox, oy)| oy == dy && ox > dx)
            .map(|&(ox, _)| ox)
            .min();
        let left = dx.checked_sub(DOT_TO_LEFT)?;
        let right = beside.and_then(|ox| ox.checked_sub(DOT_TO_LEFT + PAIR_GAP));
        let rect = DOT_TO_TOP
            .iter()
            .filter_map(|&up| {
                let top = dy.checked_sub(up)?;
                find_box(px, left, top, right)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(rect, _)| rect)?;
        let (items, name, unsure) = if on_quick_slots(px, rect) {
            (None, None, Some(Unsure::QuickSlot))
        } else {
            match self.read_name(px, (dx, dy), rect.x + rect.width - 1) {
                Ok((name, ids)) => (Some(ids), Some(name), None),
                Err(why) => (None, None, Some(why)),
            }
        };
        let confirmed = items.is_some()
            && self.last.as_ref().is_some_and(|(r, i)| {
                (r.width, r.height) == (rect.width, rect.height) && *i == items
            });
        Some(Tooltip {
            rect,
            items,
            name,
            confirmed,
            unsure,
        })
    }

    /// Reads the name after the dot at `dot`, up to the box's right edge at `right`.
    fn read_name(
        &mut self,
        px: &Pixels,
        dot: (u32, u32),
        right: u32,
    ) -> Result<(String, Vec<u32>), Unsure> {
        let cut = cut_name(px, dot, right).ok_or(Unsure::NoLetters)?;
        let (base, inner) = split_suffix(&cut);
        if base.is_empty() {
            return Err(Unsure::NoLetters);
        }
        let pieces: Vec<Piece> = base
            .iter()
            .map(|p| Piece {
                glyph: self.intern(&p.glyph),
                space: p.space,
                letters: ((p.glyph.width() / 4) as usize).clamp(1, MAX_TOUCHING),
            })
            .collect();
        self.learn_from_pattern(&pieces);
        let fits = self.decode(&pieces);
        let best = *fits.first().ok_or(Unsure::NoFit)?;
        let second = fits.get(1).map_or(u32::MAX, |f| f.cost);
        if best.unseen > 0 || best.touching > MAX_NEW_TOUCHING {
            return Err(Unsure::Learning);
        }
        if best.worst > CLOSE {
            return Err(Unsure::Unclear);
        }
        if second.saturating_sub(best.cost) < MARGIN {
            return Err(Unsure::TwoNames);
        }
        let lexicon = Arc::clone(&self.lexicon);
        let name = &lexicon.names[best.name];
        if name.ids.is_empty() {
            // Read for sure, but not an item: it teaches, and nothing's reported.
            self.teach(&pieces, best.name);
            return Err(Unsure::NoFit);
        }
        self.teach(&pieces, best.name);
        let (text, all) = (name.text.clone(), name.ids.clone());
        let gender = match inner.as_slice() {
            [one] => {
                let id = self.intern(&one.glyph);
                let (m, f) = (self.cost(id, "M"), self.cost(id, "F"));
                if m <= TEACH && f >= m + 4 {
                    Some(Some(true))
                } else if f <= TEACH && m >= f + 4 {
                    Some(Some(false))
                } else {
                    Some(None)
                }
            }
            _ => None,
        };
        let ids = match gender {
            None => all,
            Some(Some(men)) => of_gender(&all, men),
            Some(None) if of_gender(&all, true) == all => all,
            Some(None) => return Err(Unsure::Gender),
        };
        Ok((text, ids))
    }

    fn intern(&mut self, glyph: &Glyph) -> u32 {
        if let Some(&id) = self.glyph_ids.get(glyph) {
            return id;
        }
        let id = self.glyphs.len() as u32;
        self.glyphs.push(glyph.clone());
        self.glyph_ids.insert(glyph.clone(), id);
        id
    }

    /// The letters a glyph was taught by two different names alike (or one strong one), and never
    /// anything else.
    fn confirmed(&self, glyph: u32) -> Option<&str> {
        let taught = self.taught.get(&glyph)?;
        match taught.iter().collect::<Vec<_>>().as_slice() {
            [(letters, names)]
                if names.len() >= NAMES_TO_CONFIRM
                    || names.iter().any(|n| self.strong.contains(n)) =>
            {
                Some(letters.as_str())
            }
            _ => None,
        }
    }

    /// Teaches the pieces their letters when their repeated shapes fit exactly one listed name.
    fn learn_from_pattern(&mut self, pieces: &[Piece]) {
        let known: Vec<Option<String>> = pieces
            .iter()
            .map(|p| self.confirmed(p.glyph).map(str::to_string))
            .collect();
        if known.iter().all(Option::is_some) {
            return;
        }
        let key = (pieces.to_vec(), known.clone());
        let fit = match self.patterns.get(&key) {
            Some(fit) => fit.clone(),
            None => {
                let fit = self.pattern_fit_one(pieces, &known);
                self.patterns.insert(key, fit.clone());
                fit
            }
        };
        let Some((name, assigned, alone)) = fit else {
            return;
        };
        let distinct: HashSet<u32> = pieces.iter().map(|p| p.glyph).collect();
        let unknown = known.iter().filter(|k| k.is_none()).count();
        // Strong: many repeats, or (when names that aren't items are told apart too) every other
        // letter already known, so only one name in the game fits.
        if pieces.len() - distinct.len() >= STRONG || (self.others && alone && unknown <= 1) {
            self.strong.insert(name);
        }
        for (piece, letters) in pieces.iter().zip(assigned) {
            let taught = self.taught.entry(piece.glyph).or_default();
            taught.entry(letters.clone()).or_default().insert(name);
            if taught.len() > 1 {
                // Taught two different letters: whatever it was learned as can't be trusted.
                self.forget_shape(piece.glyph);
            } else if self.confirmed(piece.glyph) == Some(letters.as_str()) {
                self.add_shape(piece.glyph, &letters);
            }
        }
    }

    /// The one item name the pieces' pattern fits, with the letters each piece stands for, and
    /// whether no other name (one that isn't an item) fits it too; `None` when no item name or
    /// several do.
    fn pattern_fit_one(
        &self,
        pieces: &[Piece],
        known: &[Option<String>],
    ) -> Option<(usize, Vec<String>, bool)> {
        let k = pieces.len();
        let words = 1 + pieces.iter().skip(1).filter(|p| p.space).count();
        let lexicon = Arc::clone(&self.lexicon);
        let index_of = &lexicon.by_size;
        let mut fit = None;
        let mut alone = true;
        let most: usize = pieces.iter().map(|p| p.letters).sum();
        for n in k..=most {
            for &i in index_of.get(&(words, n)).into_iter().flatten() {
                let name = &lexicon.names[i];
                if name.ids.is_empty() && !alone {
                    continue;
                }
                if let Some(assigned) = pattern_fit(pieces, known, name) {
                    if name.ids.is_empty() {
                        alone = false;
                    } else if fit.is_some() {
                        return None;
                    } else {
                        fit = Some((i, assigned));
                    }
                }
            }
        }
        fit.map(|(i, assigned)| (i, assigned, alone))
    }

    /// Learns a decoded name's pieces: close single letters as more shapes of theirs, and letters
    /// read touching as that group's shape.
    fn teach(&mut self, pieces: &[Piece], name: usize) {
        let Some(letters) = self.align(pieces, name) else {
            return;
        };
        for (piece, letters) in pieces.iter().zip(letters) {
            if letters.chars().count() > 1 {
                if !self.shapes.contains_key(&letters) {
                    self.add_shape(piece.glyph, &letters);
                }
            } else if self.cost(piece.glyph, &letters) <= TEACH {
                self.add_shape(piece.glyph, &letters);
            }
        }
    }

    /// The letters each piece stands for in the listed name `name`, the cheapest way.
    fn align(&mut self, pieces: &[Piece], name: usize) -> Option<Vec<String>> {
        let widths = self.letter_widths();
        let lexicon = Arc::clone(&self.lexicon);
        let named = &lexicon.names[name];
        let letters = &named.letters;
        let (k, n) = (pieces.len(), letters.len());
        // best[i][j]: the cheapest cost of pieces[..i] as letters[..j], and the last piece's
        // letter count.
        let mut best = vec![vec![(u32::MAX, 0usize); n + 1]; k + 1];
        best[0][0] = (0, 0);
        for i in 0..k {
            for j in 0..n {
                let cost = best[i][j].0;
                if cost == u32::MAX || (i > 0 && pieces[i].space != named.starts.contains(&j)) {
                    continue;
                }
                for t in 1..=pieces[i].letters.min(n - j) {
                    if (j + 1..j + t).any(|q| named.starts.contains(&q)) {
                        break;
                    }
                    let text: String = letters[j..j + t].iter().collect();
                    let c = if t == 1 {
                        self.cost(pieces[i].glyph, &text)
                    } else {
                        match self.cost_touching(pieces[i].glyph, &text, &widths) {
                            (CAP, _) => continue,
                            (c, _) => c,
                        }
                    };
                    if cost + c < best[i + 1][j + t].0 {
                        best[i + 1][j + t] = (cost + c, t);
                    }
                }
            }
        }
        if best[k][n].0 == u32::MAX {
            return None;
        }
        let mut out = vec![String::new(); k];
        let (mut i, mut j) = (k, n);
        while i > 0 {
            let t = best[i][j].1;
            out[i - 1] = letters[j - t..j].iter().collect();
            i -= 1;
            j -= t;
        }
        Some(out)
    }

    /// Each learned letter's usual width: the middle of its shapes' widths.
    fn letter_widths(&self) -> HashMap<char, u32> {
        self.shapes
            .iter()
            .filter(|(letters, _)| letters.chars().count() == 1)
            .filter_map(|(letters, shapes)| {
                let mut w: Vec<u32> = shapes
                    .iter()
                    .map(|&s| self.glyphs[s as usize].width())
                    .collect();
                w.sort_unstable();
                Some((letters.chars().next()?, *w.get(w.len() / 2)?))
            })
            .collect()
    }

    /// Drops a glyph from the shapes learned, for every letter.
    fn forget_shape(&mut self, glyph: u32) {
        let mut changed = false;
        for shapes in self.shapes.values_mut() {
            let before = shapes.len();
            shapes.retain(|&s| s != glyph);
            changed |= shapes.len() != before;
        }
        if changed {
            self.shapes.retain(|_, shapes| !shapes.is_empty());
            self.distances.clear();
            self.learned += 1;
        }
    }

    fn add_shape(&mut self, glyph: u32, letters: &str) {
        if self.taught.get(&glyph).is_some_and(|t| t.len() > 1) {
            return;
        }
        let shapes = self.shapes.entry(letters.to_string()).or_default();
        if shapes.contains(&glyph) {
            return;
        }
        shapes.push(glyph);
        self.learned += 1;
        let new = self.glyphs[glyph as usize].clone();
        for (&other, row) in self.distances.iter_mut() {
            let d = if other == glyph {
                0
            } else {
                distance(&self.glyphs[other as usize], &new)
            };
            let entry = row.entry(letters.to_string()).or_insert(CAP);
            *entry = (*entry).min(d);
        }
    }

    fn distances_of(&mut self, glyph: u32) -> &HashMap<String, u32> {
        if !self.distances.contains_key(&glyph) {
            let g = &self.glyphs[glyph as usize];
            let row = self
                .shapes
                .iter()
                .map(|(letters, shapes)| {
                    let d = shapes
                        .iter()
                        .map(|&s| {
                            if s == glyph {
                                0
                            } else {
                                distance(g, &self.glyphs[s as usize])
                            }
                        })
                        .min()
                        .unwrap_or(CAP);
                    (letters.clone(), d)
                })
                .collect();
            self.distances.insert(glyph, row);
        }
        &self.distances[&glyph]
    }

    /// How far a glyph is from a letter's learned shapes: `UNSEEN` for a letter with none yet.
    /// A narrow glyph (i, l, a quote) is drawn the same every time, so only an exact match counts.
    fn cost(&mut self, glyph: u32, letter: &str) -> u32 {
        if !self.shapes.contains_key(letter) {
            return UNSEEN;
        }
        let narrow = self.glyphs[glyph as usize].width() <= 3;
        let d = self.distances_of(glyph).get(letter).copied().unwrap_or(CAP);
        if narrow && d > 0 { CAP } else { d }
    }

    /// The cost of a piece read as `letters` (two or three touching ones), and whether those
    /// letters were never seen touching.
    fn cost_touching(
        &mut self,
        glyph: u32,
        letters: &str,
        widths: &HashMap<char, u32>,
    ) -> (u32, bool) {
        if self.shapes.contains_key(letters) {
            return (self.cost(glyph, letters), false);
        }
        let width = self.glyphs[glyph as usize].width();
        let count = letters.chars().count() as u32;
        if width < 4 * count {
            return (CAP, true);
        }
        let sum: Option<u32> = letters.chars().map(|c| widths.get(&c).copied()).sum();
        if sum.is_some_and(|sum| width.abs_diff(sum) > 2) {
            return (CAP, true);
        }
        (TOUCH * (count - 1) + 2, true)
    }

    /// Every listed name the pieces could spell, best first.
    fn decode(&mut self, pieces: &[Piece]) -> Vec<Fit> {
        let key = (pieces.to_vec(), self.learned);
        if let Some(fits) = self.decoded.get(&key) {
            return fits.clone();
        }
        let widths = self.letter_widths();
        let k = pieces.len();
        let words = 1 + pieces.iter().skip(1).filter(|p| p.space).count();
        let extra: usize = pieces.iter().map(|p| p.letters - 1).sum();
        let mut fits = Vec::new();
        // Each piece's cost as each letter it might be, and whether that letter has no shapes
        // yet, worked out once per decode.
        let mut single: HashMap<(usize, char), (u32, bool)> = HashMap::new();
        let lexicon = Arc::clone(&self.lexicon);
        let index_of = &lexicon.by_size;
        let candidates: Vec<usize> = (k..=k + extra)
            .filter_map(|n| index_of.get(&(words, n)))
            .flatten()
            .copied()
            .collect();
        for index in candidates {
            let n = lexicon.names[index].letters.len();
            const NONE: (u32, u32, u32, u32) = (u32::MAX, 0, 0, 0);
            // best[i][j]: (cost, unseen letters, new touching pieces, worst) reading pieces[..i]
            // as letters[..j].
            let mut best = vec![vec![NONE; n + 1]; k + 1];
            best[0][0] = (0, 0, 0, 0);
            for i in 0..k {
                let piece = pieces[i];
                for j in 0..n {
                    let (cost, unseen, touching, worst) = best[i][j];
                    if cost == u32::MAX {
                        continue;
                    }
                    if i > 0 && piece.space != lexicon.names[index].starts.contains(&j) {
                        continue;
                    }
                    for t in 1..=piece.letters {
                        if j + t > n {
                            break;
                        }
                        if (j + 1..j + t).any(|q| lexicon.names[index].starts.contains(&q)) {
                            break;
                        }
                        let (c, new_letter) = if t == 1 {
                            let letter = lexicon.names[index].letters[j];
                            match single.get(&(i, letter)) {
                                Some(&known) => known,
                                None => {
                                    let text = letter.to_string();
                                    let known = (
                                        self.cost(piece.glyph, &text),
                                        !self.shapes.contains_key(&text),
                                    );
                                    single.insert((i, letter), known);
                                    known
                                }
                            }
                        } else {
                            let letters: String =
                                lexicon.names[index].letters[j..j + t].iter().collect();
                            match self.cost_touching(piece.glyph, &letters, &widths) {
                                // Too narrow or wide for those letters: not a way to read it.
                                (CAP, _) => continue,
                                cost => cost,
                            }
                        };
                        let total = cost.saturating_add(c);
                        if total < best[i + 1][j + t].0 {
                            let single = t == 1;
                            best[i + 1][j + t] = (
                                total,
                                unseen + u32::from(new_letter && single),
                                touching + u32::from(new_letter && !single),
                                if new_letter { worst } else { worst.max(c) },
                            );
                        }
                    }
                }
            }
            let (cost, unseen, touching, worst) = best[k][n];
            if cost < 200 {
                fits.push(Fit {
                    cost,
                    unseen,
                    touching,
                    worst,
                    name: index,
                });
            }
        }
        fits.sort_by_key(|f| (f.cost, f.unseen, f.touching, f.worst, f.name));
        self.decoded.insert(key, fits.clone());
        fits
    }

    /// What was learned of the name font, to keep on the player's device: one line per shape,
    /// its letters, a tab, then the glyph.
    pub fn knowledge(&self) -> Option<String> {
        if self.shapes.is_empty() {
            return None;
        }
        let mut lines: Vec<String> = self
            .shapes
            .iter()
            .flat_map(|(letters, shapes)| {
                shapes
                    .iter()
                    .map(move |&s| format!("{letters}\t{}", self.glyphs[s as usize].encode()))
            })
            .collect();
        lines.sort();
        Some(lines.join("\n"))
    }

    /// A reader that knows what [`TooltipReader::knowledge`] kept. `None` when the text isn't
    /// knowledge.
    pub fn from_knowledge(text: &str) -> Option<TooltipReader> {
        let mut reader = TooltipReader::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let (letters, glyph) = line.split_once('\t')?;
            let glyph = Glyph::decode(glyph)?;
            if letters.is_empty() || letters.chars().count() > MAX_TOUCHING {
                return None;
            }
            let id = reader.intern(&glyph);
            let taught = reader
                .taught
                .entry(id)
                .or_default()
                .entry(letters.to_string())
                .or_default();
            // Kept knowledge was confirmed when it was learned.
            taught.extend(KEPT);
            reader.add_shape(id, letters);
        }
        Some(reader)
    }
}

/// The names a reader tells apart, and their indexes by word count and letter count.
struct Lexicon {
    names: Vec<Name>,
    by_size: HashMap<(usize, usize), Vec<usize>>,
}

impl Lexicon {
    /// The item list's names, then `others` that aren't one.
    fn new(others: &[&str]) -> Lexicon {
        let mut by_name: HashMap<String, Vec<u32>> = HashMap::new();
        for item in game_data::items() {
            by_name
                .entry(item.name.to_string())
                .or_default()
                .push(item.id);
        }
        for other in others {
            by_name.entry(other.trim().to_string()).or_default();
        }
        by_name.retain(|text, _| !text.is_empty());
        let mut names: Vec<Name> = by_name
            .into_iter()
            .map(|(text, mut ids)| {
                ids.sort_unstable();
                let mut letters = Vec::new();
                let mut starts = HashSet::new();
                let mut words = 0;
                for word in text.split(' ').filter(|w| !w.is_empty()) {
                    if words > 0 {
                        starts.insert(letters.len());
                    }
                    words += 1;
                    letters.extend(word.chars().map(fold));
                }
                Name {
                    text,
                    letters,
                    starts,
                    words,
                    ids,
                }
            })
            .collect();
        names.sort_by(|a, b| a.text.cmp(&b.text));
        let mut by_size: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (i, name) in names.iter().enumerate() {
            by_size
                .entry((name.words, name.letters.len()))
                .or_default()
                .push(i);
        }
        Lexicon { names, by_size }
    }
}

/// The item list's names alone, built once.
fn item_lexicon() -> &'static Arc<Lexicon> {
    static ITEMS: OnceLock<Arc<Lexicon>> = OnceLock::new();
    ITEMS.get_or_init(|| Arc::new(Lexicon::new(&[])))
}

/// Capital I and small l are drawn alike, so names are compared with them folded together.
fn fold(c: char) -> char {
    if c == 'I' { 'l' } else { c }
}

/// The men's or the women's items among `ids`, when it holds both: the game's tops, overalls and
/// bottoms are numbered 104xxxx/105xxxx/106xxxx, men's with a 0 after that and women's a 1.
fn of_gender(ids: &[u32], men: bool) -> Vec<u32> {
    let kind = |id: &u32| {
        (104..=106)
            .contains(&(id / 10_000))
            .then(|| (id / 1000) % 10)
    };
    let has = |g| ids.iter().any(|id| kind(id) == Some(g));
    if !(has(0) && has(1)) {
        return ids.to_vec();
    }
    let want = if men { 0 } else { 1 };
    ids.iter()
        .copied()
        .filter(|id| kind(id) == Some(want))
        .collect()
}

/// How far apart two glyphs are: the pixels that differ, lined up at the best of the two ways when
/// their widths differ by one. `CAP` when they differ more, or in height.
fn distance(a: &Glyph, b: &Glyph) -> u32 {
    if a.height() != b.height() || a.width().abs_diff(b.width()) > 1 {
        return CAP;
    }
    let (wide, narrow) = if a.width() >= b.width() {
        (a, b)
    } else {
        (b, a)
    };
    let shifts: &[u32] = if wide.width() == narrow.width() {
        &[0]
    } else {
        &[0, 1]
    };
    shifts
        .iter()
        .map(|&s| {
            (0..wide.height())
                .map(|y| (wide.row(y) ^ (narrow.row(y) << s)).count_ones())
                .sum::<u32>()
        })
        .min()
        .unwrap_or(CAP)
        .min(CAP)
}

/// Whether `pieces` spell `name` with the letters known so far, two pieces exactly alike being the
/// same letters, a piece maybe 2 or 3 touching letters. The letters each piece stands for, the
/// first way found.
fn pattern_fit(pieces: &[Piece], known: &[Option<String>], name: &Name) -> Option<Vec<String>> {
    fn go(
        i: usize,
        j: usize,
        pieces: &[Piece],
        known: &[Option<String>],
        name: &Name,
        classes: &mut HashMap<u32, String>,
        out: &mut Vec<String>,
    ) -> bool {
        let n = name.letters.len();
        if i == pieces.len() {
            return j == n;
        }
        let piece = pieces[i];
        if i > 0 && piece.space != name.starts.contains(&j) {
            return false;
        }
        // The pieces left must be able to hold the letters left.
        let most: usize = pieces[i..].iter().map(|p| p.letters).sum();
        if n - j < pieces.len() - i || n - j > most {
            return false;
        }
        for t in 1..=piece.letters {
            if j + t > n || (j + 1..j + t).any(|q| name.starts.contains(&q)) {
                break;
            }
            let letters: String = name.letters[j..j + t].iter().collect();
            if known[i].as_ref().is_some_and(|k| *k != letters) {
                continue;
            }
            let fresh = match classes.get(&piece.glyph) {
                Some(c) if *c != letters => continue,
                Some(_) => false,
                None => {
                    classes.insert(piece.glyph, letters.clone());
                    true
                }
            };
            out.push(letters);
            if go(i + 1, j + t, pieces, known, name, classes, out) {
                return true;
            }
            out.pop();
            if fresh {
                classes.remove(&piece.glyph);
            }
        }
        false
    }
    let mut out = Vec::new();
    go(0, 0, pieces, known, name, &mut HashMap::new(), &mut out).then_some(out)
}

/// Whether a box is a quick slot's tooltip: its bottom-left corner on the quick-slot panel.
fn on_quick_slots(px: &Pixels, rect: Rect) -> bool {
    let (Some(x), Some(y)) = (
        (px.width.checked_sub(800)).map(|m| m / 2 + PANEL_DX),
        px.height.checked_sub(PANEL_UP),
    ) else {
        return false;
    };
    let bottom = rect.y + rect.height - 1;
    (x..=x + PANEL.0).contains(&rect.x) && (y.saturating_sub(8)..=y + PANEL.1 + 4).contains(&bottom)
}

fn near(p: [u8; 3], c: [u8; 3], tolerance: u8) -> bool {
    p.iter().zip(c).all(|(&a, b)| a.abs_diff(b) <= tolerance)
}

/// The middles of the name dots in a frame.
fn find_dots(px: &Pixels) -> Vec<(u32, u32)> {
    let mut dots = Vec::new();
    let is_dot = |x: i64, y: i64| px.get(x, y).is_some_and(|p| near(p, DOT, DOT_TOLERANCE));
    // The dot's three middle rows are each at least 3 pixels wide, so every 3rd row and column
    // meets one of them, within 2 pixels of the dot's middle.
    let mut y = 3;
    while y + 3 < px.height {
        let mut x = 0;
        while x < px.width {
            if near(px.rgb(x, y), DOT, DOT_TOLERANCE) {
                for cy in y - 2..=y + 2 {
                    for cx in x.saturating_sub(2)..=(x + 2).min(px.width - 1) {
                        let (ix, iy) = (i64::from(cx), i64::from(cy));
                        let core = DOT_CORE
                            .iter()
                            .all(|&(dx, dy)| is_dot(ix + i64::from(dx), iy + i64::from(dy)));
                        let around = DOT_AROUND
                            .iter()
                            .all(|&(dx, dy)| !is_dot(ix + i64::from(dx), iy + i64::from(dy)));
                        if core && around && !dots.contains(&(cx, cy)) {
                            dots.push((cx, cy));
                        }
                    }
                }
            }
            x += 3;
        }
        y += 3;
    }
    dots.sort_by_key(|&(x, y)| (y, x));
    dots
}

/// The background a tinted pixel shows, channel by channel (outside 0-255 when it isn't tinted).
fn untint(p: [u8; 3]) -> [f32; 3] {
    [0, 1, 2].map(|c| (f32::from(p[c]) - TINT_BASE[c]) * 255.0 / TINT_SPAN)
}

fn tint(p: [u8; 3]) -> [f32; 3] {
    [0, 1, 2].map(|c| TINT_BASE[c] + f32::from(p[c]) * TINT_SPAN / 255.0)
}

/// Whether `line` and `inside` look like the box's tinted edge and inside with `gap`, the
/// untinted line between them.
fn edge(line: [u8; 3], gap: [u8; 3], inside: [u8; 3]) -> bool {
    let shows = |p: [u8; 3]| {
        let u = untint(p);
        (0..3).all(|c| {
            (-8.0..=263.0).contains(&u[c]) && (u[c] - f32::from(gap[c])).abs() <= EDGE_TOLERANCE
        })
    };
    let t = tint(gap);
    let contrast = (0..3).any(|c| (f32::from(gap[c]) - t[c]).abs() >= EDGE_CONTRAST);
    contrast && shows(line) && shows(inside)
}

/// Whether a pixel could be tinted at all.
fn tintable(p: [u8; 3]) -> bool {
    (0..3).all(|c| {
        let v = f32::from(p[c]);
        v >= TINT_BASE[c] - 2.0 && v <= TINT_BASE[c] + TINT_SPAN + 2.0
    })
}

/// Edge tests: a right edge's line at column `x` (untinted line left of it, inside further left),
/// a left edge's at `x`, a bottom edge's at row `y`, a top edge's at row `y`.
fn right_edge(px: &Pixels, x: u32, y: u32) -> bool {
    x >= 2 && edge(px.rgb(x, y), px.rgb(x - 1, y), px.rgb(x - 2, y))
}
fn left_edge(px: &Pixels, x: u32, y: u32) -> bool {
    x + 2 < px.width && edge(px.rgb(x, y), px.rgb(x + 1, y), px.rgb(x + 2, y))
}
fn bottom_edge(px: &Pixels, x: u32, y: u32) -> bool {
    y >= 2 && edge(px.rgb(x, y), px.rgb(x, y - 1), px.rgb(x, y - 2))
}
fn top_edge(px: &Pixels, x: u32, y: u32) -> bool {
    y + 2 < px.height && edge(px.rgb(x, y), px.rgb(x, y + 1), px.rgb(x, y + 2))
}

/// The box whose left edge is column `left` and top edge row `top`: its right edge `right` when
/// known (the worn item's box beside it says where), and a score for how well its edges hold.
fn find_box(px: &Pixels, left: u32, top: u32, right: Option<u32>) -> Option<(Rect, f32)> {
    let (w, h) = (px.width, px.height);
    if top + 30 >= h || left + 30 >= w {
        return None;
    }
    let right = right.or_else(|| right_edge_of(px, left, top))?;
    if right >= w || right < left + 30 {
        return None;
    }
    let (score, bottom) = bottom_edge_of(px, left, top, right)?;
    if score < 1.0 {
        return None;
    }
    let along = (left + 2..right - 1)
        .filter(|&x| top_edge(px, x, top))
        .count() as f32;
    let rect = Rect {
        x: left,
        y: top,
        width: right - left + 1,
        height: bottom - top + 1,
    };
    Some((rect, along / (right - left - 3) as f32))
}

/// The right edge: one of the usual widths where the edge holds down most of the box's first rows,
/// else the first column where it holds down at least half of them.
fn right_edge_of(px: &Pixels, left: u32, top: u32) -> Option<u32> {
    let rows = top + 2..(top + 40).min(px.height);
    let count = rows.len().max(1) as f32;
    let shares = std::cell::RefCell::new(HashMap::new());
    let share = |x: u32| {
        *shares.borrow_mut().entry(x).or_insert_with(|| {
            rows.clone().filter(|&y| right_edge(px, x, y)).count() as f32 / count
        })
    };
    let peak = |x: u32, s: f32| {
        (x.saturating_sub(3)..=x + 3)
            .filter(|&o| o < px.width)
            .all(|o| share(o) <= s)
    };
    for width in WIDTHS {
        let x = left + width;
        if x < px.width {
            let s = share(x);
            if s >= 0.3 && peak(x, s) {
                return Some(x);
            }
        }
    }
    (left + 24..px.width).find(|&x| {
        let s = share(x);
        s >= 0.5 && peak(x, s)
    })
}

/// The bottom edge: the row where the left and right edges end, the bottom edge's pattern holds,
/// and the untinted row above it dips, with a score for how well.
fn bottom_edge_of(px: &Pixels, left: u32, top: u32, right: u32) -> Option<(f32, u32)> {
    let h = px.height as usize;
    let from = top as usize;
    let col = |f: &dyn Fn(u32) -> bool| -> Vec<bool> {
        (0..h).map(|y| y >= from && f(y as u32)).collect()
    };
    let left_col = col(&|y| left_edge(px, left, y));
    let right_col = col(&|y| right_edge(px, right, y));
    let span = left + 2..right - 1;
    let inside = (right - left - 1) as f32;
    let tinted: Vec<f32> = (0..h)
        .map(|y| {
            if y < from {
                0.0
            } else {
                (left + 1..right)
                    .filter(|&x| tintable(px.rgb(x, y as u32)))
                    .count() as f32
                    / inside
            }
        })
        .collect();
    let ends = |col: &[bool], y: usize| {
        let share = |r: std::ops::Range<usize>| {
            let n = r.len();
            if n == 0 {
                0.0
            } else {
                col[r].iter().filter(|&&b| b).count() as f32 / n as f32
            }
        };
        share(y.saturating_sub(9)..y.saturating_sub(1))
            - share((y + 1).min(col.len())..(y + 13).min(col.len()))
    };
    // Everything but the bottom edge's own vote, which is at most 1: rows that can't win even
    // with it aren't voted on.
    let mut rows: Vec<(f32, usize)> = (from + 24..h)
        .map(|y| {
            let dip = tinted[y].min(tinted[y - 2]) - tinted[y - 1];
            let below = if y + 1 < h {
                let r = y + 1..(y + 4).min(h);
                tinted[r.clone()].iter().sum::<f32>() / r.len() as f32
            } else {
                0.0
            };
            (
                ends(&left_col, y) + ends(&right_col, y) + dip + 0.5 * (tinted[y] - below),
                y,
            )
        })
        .collect();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut best: Option<(f32, u32)> = None;
    for (rest, y) in rows {
        if best.is_some_and(|(s, _)| rest + 1.0 < s) {
            break;
        }
        let along = span
            .clone()
            .filter(|&x| bottom_edge(px, x, y as u32))
            .count() as f32
            / span.len().max(1) as f32;
        let score = rest + along;
        if best.is_none_or(|(s, by)| score > s || (score == s && (y as u32) < by)) {
            best = Some((score, y as u32));
        }
    }
    best
}

/// A piece cut from the name's row: its glyph, whether a space comes before it, and its first and
/// last inked rows.
#[derive(Debug, Clone)]
struct Cut {
    glyph: Glyph,
    space: bool,
    top: u32,
    bottom: u32,
}

/// The name's pieces, from the dot to `right`. A pixel is letter where its red channel is more
/// than halfway from the tinted background around it to full: white and orange names both are
/// full red. `None` when a piece is too wide to be letters.
fn cut_name(px: &Pixels, (dx, dy): (u32, u32), right: u32) -> Option<Vec<Cut>> {
    let top = i64::from(dy) + i64::from(NAME_ROWS.0);
    let rows = (NAME_ROWS.1 - NAME_ROWS.0) as u32;
    let x0 = dx + NAME_FROM_DOT;
    let x1 = right.saturating_sub(3).min(px.width);
    if top < 0 || top as u32 + rows > px.height || x1 <= x0 + 2 {
        return None;
    }
    let top = top as u32;
    let width = x1 - x0;
    let red: Vec<Vec<u8>> = (0..rows)
        .map(|y| (0..width).map(|x| px.rgb(x0 + x, top + y)[0]).collect())
        .collect();
    // The background's red level around each column: the median of the pixels that aren't letter.
    let background: Vec<f32> = (0..width as usize)
        .map(|x| {
            let mut v: Vec<u8> = (x.saturating_sub(8)..(x + 9).min(width as usize))
                .flat_map(|c| red.iter().map(move |row| row[c]))
                .filter(|&r| r < 120)
                .collect();
            if v.is_empty() {
                return 60.0;
            }
            v.sort_unstable();
            let m = v.len() / 2;
            if v.len().is_multiple_of(2) {
                (f32::from(v[m - 1]) + f32::from(v[m])) / 2.0
            } else {
                f32::from(v[m])
            }
        })
        .collect();
    let ink = |x: usize, y: usize| {
        let f = background[x];
        (f32::from(red[y][x]) - f) / (255.0 - f).max(1.0) >= 0.5
    };
    let inked: Vec<bool> = (0..width as usize)
        .map(|x| (0..rows as usize).any(|y| ink(x, y)))
        .collect();
    let mut cuts = Vec::new();
    let mut x = 0;
    let mut last_end: Option<usize> = None;
    while x < width as usize {
        if !inked[x] {
            x += 1;
            continue;
        }
        let start = x;
        while x < width as usize && inked[x] {
            x += 1;
        }
        if last_end.is_some_and(|end| start - end >= NAME_END as usize) {
            break;
        }
        let w = (x - start) as u32;
        let rows_bits: Vec<u32> = (0..rows as usize)
            .map(|y| {
                (0..w as usize).fold(
                    0u32,
                    |m, dx| if ink(start + dx, y) { m | 1 << dx } else { m },
                )
            })
            .collect();
        let glyph = Glyph::from_rows(w, rows_bits)?;
        let inked_rows: Vec<u32> = (0..rows).filter(|&y| glyph.row(y) != 0).collect();
        cuts.push(Cut {
            glyph,
            space: last_end.is_some_and(|end| start - end >= SPACE as usize),
            top: *inked_rows.first().unwrap_or(&0),
            bottom: *inked_rows.last().unwrap_or(&0),
        });
        last_end = Some(x);
    }
    (!cuts.is_empty()).then_some(cuts)
}

/// The name without a last " (...)" group, and the group's pieces between its brackets: a
/// bracket is a narrow piece taller than any letter.
fn split_suffix(cuts: &[Cut]) -> (Vec<Cut>, Vec<Cut>) {
    let bracket = |c: &Cut| c.glyph.width() <= 4 && c.bottom - c.top + 1 >= 12;
    if cuts.len() >= 3 && bracket(&cuts[cuts.len() - 1]) {
        for i in (1..cuts.len() - 1).rev() {
            if bracket(&cuts[i]) && cuts[i].space {
                return (cuts[..i].to_vec(), cuts[i + 1..cuts.len() - 1].to_vec());
            }
            if cuts[i].space {
                break;
            }
        }
    }
    (cuts.to_vec(), Vec::new())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// A made-up name font for the tests (never the game's): each letter its own 6-pixel-wide
    /// shape over 10 rows, inked in every column so it's cut whole; I and l drawn alike, as the
    /// game draws them; brackets narrow and taller than any letter.
    pub(crate) fn letter(c: char) -> (u32, Vec<u32>) {
        match c {
            'I' | 'l' | 'i' => {
                let mut rows = vec![0b11; 10];
                if c == 'i' {
                    rows[1] = 0;
                }
                (2, rows)
            }
            '(' => (
                3,
                (0..13)
                    .map(|r| if (2..=10).contains(&r) { 0b011 } else { 0b100 })
                    .collect(),
            ),
            ')' => (
                3,
                (0..13)
                    .map(|r| if (2..=10).contains(&r) { 0b110 } else { 0b001 })
                    .collect(),
            ),
            _ => {
                let mut h = u32::from(c).wrapping_mul(2_654_435_761);
                let rows = (0..10)
                    .map(|r| {
                        h ^= h << 13;
                        h ^= h >> 17;
                        h ^= h << 5;
                        // Every column inked once (row r inks column r % 6), plus some others.
                        (h & 0b111111) | 1 << (r % 6)
                    })
                    .collect();
                (6, rows)
            }
        }
    }

    /// Draws an item tooltip on a BGRA picture `width` pixels wide: a box from (`x0`, `y0`),
    /// `w` x `h`, tinted with its untinted ring and cut corners, its dot and `name` in the test
    /// font (an equip's name a row lower). Returns the box's rect.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        bgra: &mut [u8],
        width: u32,
        x0: u32,
        y0: u32,
        w: u32,
        h: u32,
        name: &str,
        equip: bool,
    ) -> Rect {
        let get = |bgra: &[u8], x: u32, y: u32| {
            let i = ((y * width + x) * 4) as usize;
            [bgra[i + 2], bgra[i + 1], bgra[i]]
        };
        let set = |bgra: &mut [u8], x: u32, y: u32, [r, g, b]: [u8; 3]| {
            let i = ((y * width + x) * 4) as usize;
            bgra[i..i + 3].copy_from_slice(&[b, g, r]);
        };
        let (x1, y1) = (x0 + w - 1, y0 + h - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let corner = (x == x0 || x == x1) && (y == y0 || y == y1);
                let ring = (x == x0 + 1 || x == x1 - 1) && y > y0 && y < y1
                    || (y == y0 + 1 || y == y1 - 1) && x > x0 && x < x1;
                if !corner && !ring {
                    let t = tint(get(bgra, x, y)).map(|v| v.round() as u8);
                    set(bgra, x, y, t);
                }
            }
        }
        let (dx, dy) = (x0 + DOT_TO_LEFT, y0 + if equip { 17 } else { 16 });
        for (ox, oy) in DOT_CORE {
            set(
                bgra,
                dx.wrapping_add_signed(ox),
                dy.wrapping_add_signed(oy),
                DOT,
            );
        }
        let mut x = dx + 6;
        for c in name.chars() {
            if c == ' ' {
                x += 4;
                continue;
            }
            let (lw, rows) = letter(c);
            let top = if rows.len() > 10 { dy - 5 } else { dy - 4 };
            for (r, bits) in rows.iter().enumerate() {
                for b in 0..lw {
                    if bits >> b & 1 == 1 {
                        set(bgra, x + b, top + r as u32, [255, 255, 255]);
                    }
                }
            }
            x += lw + 1;
        }
        Rect {
            x: x0,
            y: y0,
            width: w,
            height: h,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Canvas {
        width: u32,
        height: u32,
        bgra: Vec<u8>,
    }

    impl Canvas {
        /// A smooth, light scene: no edges of its own for the box's pattern to agree with.
        fn new(width: u32, height: u32) -> Canvas {
            let mut bgra = Vec::with_capacity((width * height * 4) as usize);
            for y in 0..height {
                for x in 0..width {
                    let r = 150 + (x * 60 / width) as u8;
                    let g = 170 + (y * 50 / height) as u8;
                    bgra.extend_from_slice(&[120, g, r, 255]);
                }
            }
            Canvas {
                width,
                height,
                bgra,
            }
        }

        /// A tooltip from (`x0`, `y0`), `width` x `height` ([`test_support::draw`]).
        fn tooltip(
            &mut self,
            x0: u32,
            y0: u32,
            width: u32,
            height: u32,
            name: &str,
            equip: bool,
        ) -> Rect {
            test_support::draw(
                &mut self.bgra,
                self.width,
                x0,
                y0,
                width,
                height,
                name,
                equip,
            )
        }

        fn frame(&self) -> Frame<'_> {
            Frame::new(
                self.width,
                self.height,
                (self.width * 4) as usize,
                &self.bgra,
            )
            .unwrap()
        }
    }

    /// Names with many repeated letters, which teach the reader the font from the item list.
    const TEACHERS: [&str; 8] = [
        "Return Scroll to Kerning City",
        "One-Handed Sword Attack Scroll: Chaos",
        "Faded Maple Leaf Earrings",
        "Iron Hog's Metal Hoof",
        "Wolbi Throwing Stars",
        "Skeledog's Bone",
        "Green Orientican Pants",
        "Dark Silver Stealer Pants",
    ];

    /// Shows one name's tooltip for two frames, as a player resting on it would.
    fn show(reader: &mut TooltipReader, name: &str) -> TooltipReading {
        let mut canvas = Canvas::new(1366, 768);
        canvas.tooltip(300, 200, 288, 113, name, false);
        let _ = reader.observe(&canvas.frame(), None);
        reader.observe(&canvas.frame(), None)
    }

    fn taught() -> TooltipReader {
        let mut reader = TooltipReader::new();
        for name in TEACHERS {
            let _ = show(&mut reader, name);
        }
        reader
    }

    fn ids_of(name: &str) -> Vec<u32> {
        let mut ids: Vec<u32> = game_data::items()
            .iter()
            .filter(|i| i.name == name)
            .map(|i| i.id)
            .collect();
        ids.sort_unstable();
        ids
    }

    // The box is found to the pixel from its dot and its tinted edges, at both sizes. Fails if an
    // edge is off by one (the price label would sit a pixel off the tooltip).
    #[test]
    fn finds_the_box_to_the_pixel() {
        for (w, h) in SIZES {
            // The usual widths (288, and 236 for an equip), and a wider one for a long name.
            for (x0, y0, width, height) in [
                (300, 200, 288, 113),
                (40, 400, 236, 206),
                (w - 320, h - 140, 310, 130),
            ] {
                let mut canvas = Canvas::new(w, h);
                let drawn = canvas.tooltip(x0, y0, width, height, "Mithril Ingot", width == 236);
                let read = TooltipReader::new().observe(&canvas.frame(), None);
                assert_eq!(
                    read.tooltip.map(|t| t.rect),
                    Some(drawn),
                    "{w}x{h} at {x0},{y0}"
                );
            }
        }
    }

    // A few names with many repeated letters teach the font from the item list; then a name the
    // reader never saw is read, and counted only on the second frame. Fails if the learning or the
    // two-frame rule breaks.
    #[test]
    fn learns_the_font_from_the_item_list_then_names_items() {
        let mut reader = taught();
        let mut canvas = Canvas::new(1366, 768);
        canvas.tooltip(500, 300, 288, 113, "Mithril Ingot", false);
        let first = reader.observe(&canvas.frame(), None).tooltip.unwrap();
        assert_eq!(first.items, Some(ids_of("Mithril Ingot")));
        assert!(!first.confirmed);
        let second = reader.observe(&canvas.frame(), None).tooltip.unwrap();
        assert!(second.confirmed);
    }

    // With nothing learned, nothing is named: the first tooltips teach and stay unsure. Fails if a
    // name is guessed from its pattern alone.
    #[test]
    fn names_nothing_before_learning() {
        let mut reader = TooltipReader::new();
        let read = show(&mut reader, "Work Gloves");
        assert_eq!(read.tooltip.unwrap().items, None);
    }

    // A resting hand drifts, and the tooltip follows the cursor: the item stays counted while its box
    // moves, but a box of another size isn't counted on its first frame. Fails if the label would
    // drop out whenever the cursor moves a pixel, or would show for a box only one frame saw.
    #[test]
    fn a_box_following_the_cursor_stays_confirmed() {
        let mut reader = taught();
        for (i, (x, y, height)) in [
            (500, 300, 113),
            (503, 301, 113),
            (497, 306, 113),
            (497, 306, 133),
        ]
        .into_iter()
        .enumerate()
        {
            let mut canvas = Canvas::new(1366, 768);
            canvas.tooltip(x, y, 288, height, "Mithril Ingot", false);
            let t = reader.observe(&canvas.frame(), None).tooltip.unwrap();
            assert_eq!(t.items, Some(ids_of("Mithril Ingot")), "frame {i}");
            assert_eq!(t.confirmed, i == 1 || i == 2, "frame {i}");
        }
    }

    // Hovering an equip worn in the same slot shows the worn item's box beside it: only the
    // hovered (left) one is reported, with its own right edge. Fails if the worn item's name or
    // box is reported instead.
    #[test]
    fn the_worn_items_box_beside_is_never_reported() {
        let mut reader = taught();
        let mut canvas = Canvas::new(1366, 768);
        let hovered = canvas.tooltip(300, 200, 236, 206, "Work Gloves", true);
        canvas.tooltip(300 + 235 + PAIR_GAP, 200, 236, 206, "Steel Slain", true);
        let _ = reader.observe(&canvas.frame(), None);
        let t = reader.observe(&canvas.frame(), None).tooltip.unwrap();
        assert_eq!(t.rect, hovered);
        assert_eq!(t.items, Some(ids_of("Work Gloves")));
    }

    // A quick slot's tooltip (its bottom-left corner on the quick-slot panel) gets no label, even
    // though it names the right item. Fails if quick slots would be labelled.
    #[test]
    fn quick_slot_tooltips_are_left_out() {
        let mut reader = taught();
        let mut canvas = Canvas::new(1366, 768);
        let (px, py) = ((1366 - 800) / 2 + PANEL_DX, 768 - PANEL_UP);
        canvas.tooltip(px + 10, py + 3 - 113, 288, 113, "Mithril Ingot", false);
        let t = reader.observe(&canvas.frame(), None).tooltip.unwrap();
        assert_eq!((t.items, t.unsure), (None, Some(Unsure::QuickSlot)));
    }

    // Names the item list doesn't have (cash items, skills) are never named, even with their
    // letters known. Fails if one is read as a listed item that looks alike.
    #[test]
    fn names_outside_the_item_list_are_never_named() {
        let mut reader = taught();
        for name in [
            "Skull Shirt",
            "Black Kitty",
            "Claw Mastery",
            "Count Dracula Cape",
        ] {
            let t = show(&mut reader, name).tooltip.unwrap();
            assert_eq!(t.items, None, "{name}");
        }
    }

    // Told the names the price service lists that aren't items, the reader learns a name's last
    // letter from the rest, yet a cash item one letter from a listed item ("Black Kitty", the
    // listed "Black Nitty") is still never read as it, before or after its letters are learned.
    #[test]
    fn a_cash_item_a_letter_from_a_listed_one_is_never_named() {
        let mut reader = taught().with_other_names(["Black Kitty", "Kitty Hat"]);
        for _ in 0..3 {
            for name in ["Black Kitty", "Kitty Hat", "Kumbi Throwing Stars"] {
                let t = show(&mut reader, name).tooltip.unwrap();
                if name != "Kumbi Throwing Stars" {
                    assert_eq!(t.items, None, "{name}");
                }
            }
        }
        let t = show(&mut reader, "Kumbi Throwing Stars").tooltip.unwrap();
        assert_eq!(t.items, Some(ids_of("Kumbi Throwing Stars")));
    }

    // A men's or women's item's (M) or (F) picks that item: the item list names both alike.
    // Fails if both are named, or the wrong one.
    #[test]
    fn m_and_f_pick_the_mens_or_womens_item() {
        let mut reader = taught();
        let _ = show(&mut reader, "Mark of the Beta");
        let _ = show(&mut reader, "Fragment of Magic");
        let both = ids_of("Red Cloth Pants");
        assert_eq!(both.len(), 2);
        let t = show(&mut reader, "Red Cloth Pants (M)").tooltip.unwrap();
        assert_eq!(t.items, Some(vec![both[0]]));
        let t = show(&mut reader, "Red Cloth Pants (F)").tooltip.unwrap();
        assert_eq!(t.items, Some(vec![both[1]]));
    }

    // What was learned is kept as text and read back, so a new session names items at once.
    // Fails if the knowledge doesn't round-trip.
    #[test]
    fn learned_letters_are_kept_across_sessions() {
        let text = taught().knowledge().unwrap();
        let mut reader = TooltipReader::from_knowledge(&text).unwrap();
        let t = show(&mut reader, "Mithril Ingot").tooltip.unwrap();
        assert_eq!(t.items, Some(ids_of("Mithril Ingot")));
        assert!(TooltipReader::from_knowledge("not\tknowledge").is_none());
    }

    // Over dark scenes (black, or the dark grey of a cave) the tint barely changes what's behind,
    // yet the box is still found exactly. Fails if the edge contrast needed is set too high.
    #[test]
    fn finds_the_box_over_dark_scenes() {
        for shade in [[0, 0, 0], [40, 44, 52]] {
            let mut canvas = Canvas::new(1366, 768);
            for px in canvas.bgra.chunks_mut(4) {
                px[..3].copy_from_slice(&[shade[2], shade[1], shade[0]]);
            }
            let drawn = canvas.tooltip(400, 150, 288, 113, "Mithril Ingot", false);
            let read = TooltipReader::new().observe(&canvas.frame(), None);
            assert_eq!(read.tooltip.map(|t| t.rect), Some(drawn), "{shade:?}");
        }
    }

    // Only the checked sizes are read. Fails if a tooltip is read at a size the box wasn't
    // measured at.
    #[test]
    fn other_sizes_read_nothing() {
        let mut canvas = Canvas::new(1024, 768);
        canvas.tooltip(300, 200, 287, 113, "Mithril Ingot", false);
        assert_eq!(
            TooltipReader::new().observe(&canvas.frame(), None),
            TooltipReading::default()
        );
    }
}
