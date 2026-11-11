//! Recognizers: one module each, holding its region, parser, confidence and tests together.

pub mod chat;
mod digit_shapes;
pub mod exp_bar;
pub mod exp_table;
pub mod map_name;
pub mod notices;

/// A name as the game drew it (an item's, a map's), for showing it before or without its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamePicture {
    pub width: u32,
    pub height: u32,
    /// How much of each pixel is letter, row by row: 255 is all letter, 0 is none.
    pub coverage: Vec<u8>,
}
