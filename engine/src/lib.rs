//! MesoVibe engine: capture → recognizers → typed events.
//!
//! The engine only ever reads pixels from the game window. It has no code path
//! that sends input to the game, reads or writes game memory or files, or
//! touches network traffic (see Fair play in README.md).
//!
//! Recognizers live in one module each (region, parser, confidence and
//! fixtures together), not split by pipeline stage.

pub mod capture;
pub mod events;
pub mod frame;
pub mod glyph;
pub mod lines;
pub mod recognizers;
pub mod runner;
pub mod unstretch;
