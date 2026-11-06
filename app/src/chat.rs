//! The Chat Hub's log: the whisper, party and friend lines the chat reader found, as pictures,
//! for the panel's Chat view and the optional chat overlay.
//!
//! Lines are kept in memory only, for as long as MesoVibe runs, and only ever go to MesoVibe's
//! own windows: never to plugins, never off this computer.
//!
//! Each line is a PNG whose alpha is the text (the panel colors it), plus the column ranges of its
//! words, so a line wider than the panel wraps between words like text.

use std::collections::VecDeque;
use std::sync::Mutex;

use mesovibe_engine::recognizers::chat::{Channel, ChatLine, ChatReading};
use serde::Serialize;

/// Lines kept, all channels together; the oldest go first.
const KEEP: usize = 1000;
/// Columns with at least this much text belong to a word.
const INK: u8 = 64;
/// Empty columns that end a word: wider than the gap between two letters.
const SPACE: u32 = 3;

/// One chat line as the panel gets it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEntry {
    pub id: u64,
    pub channel: Channel,
    pub at_ms: u64,
    /// The indented rest of the line before it.
    pub continues: bool,
    /// How many times so far the reader lost track of the chat box. When it differs between two
    /// lines of a channel, lines may be missing between them.
    pub gaps: u64,
    pub width: u32,
    pub height: u32,
    /// `data:image/png;base64,...`, gray with the text in its alpha.
    pub png: String,
    /// Each word's columns, `[from, to)`.
    pub words: Vec<[u32; 2]>,
}

#[derive(Default)]
struct Log {
    lines: VecDeque<ChatEntry>,
    next_id: u64,
    gaps: u64,
}

/// The lines found so far this run.
#[derive(Default)]
pub struct ChatLog(Mutex<Log>);

impl ChatLog {
    /// Keeps a frame's new lines and returns them as the panel gets them.
    pub fn record(&self, reading: &ChatReading, at_ms: u64) -> Vec<ChatEntry> {
        let mut log = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if reading.gap {
            log.gaps += 1;
        }
        let mut entries = Vec::with_capacity(reading.lines.len());
        for line in &reading.lines {
            let Some(png) = encode(line) else { continue };
            log.next_id += 1;
            let entry = ChatEntry {
                id: log.next_id,
                channel: line.channel,
                at_ms,
                continues: line.continues,
                gaps: log.gaps,
                width: line.width,
                height: line.height,
                png,
                words: words(line),
            };
            if log.lines.len() == KEEP {
                log.lines.pop_front();
            }
            log.lines.push_back(entry.clone());
            entries.push(entry);
        }
        entries
    }

    /// Every line kept, oldest first.
    pub fn history(&self) -> Vec<ChatEntry> {
        let log = self.0.lock().unwrap_or_else(|p| p.into_inner());
        log.lines.iter().cloned().collect()
    }

    /// Forgets the lines kept so far: the play session they came from has ended (presence.rs).
    /// Line ids keep counting, so a window that saw earlier lines never mistakes new ones for them.
    pub fn clear(&self) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .lines
            .clear();
    }
}

/// The column ranges of a line's words: runs of columns with text, split where at least
/// [`SPACE`] columns in a row have none.
pub fn words(line: &ChatLine) -> Vec<[u32; 2]> {
    let (w, h) = (line.width as usize, line.height as usize);
    let inked = |col: usize| (0..h).any(|row| line.coverage[row * w + col] >= INK);
    let mut words = Vec::new();
    let mut start: Option<u32> = None;
    let mut blank = 0;
    for col in 0..line.width {
        if inked(col as usize) {
            if start.is_none() {
                start = Some(col);
            }
            blank = 0;
        } else if let Some(from) = start {
            blank += 1;
            if blank == SPACE {
                words.push([from, col + 1 - SPACE]);
                start = None;
            }
        }
    }
    if let Some(from) = start {
        words.push([from, line.width - blank]);
    }
    // One column of margin each side, for the letters' smoothed edges.
    words
        .into_iter()
        .map(|[from, to]| [from.saturating_sub(1), (to + 1).min(line.width)])
        .collect()
}

/// The line as a PNG data URL: white, with the text's coverage as alpha.
fn encode(line: &ChatLine) -> Option<String> {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, line.width, line.height);
        encoder.set_color(png::ColorType::GrayscaleAlpha);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        let pixels: Vec<u8> = line.coverage.iter().flat_map(|&a| [255, a]).collect();
        writer.write_image_data(&pixels).ok()?;
    }
    Some(format!(
        "data:image/png;base64,{}",
        crate::engine::base64(&png)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-row line whose columns hold text where `pattern` has '#'.
    fn line(pattern: &str, channel: Channel) -> ChatLine {
        ChatLine {
            channel,
            continues: false,
            width: pattern.len() as u32,
            height: 1,
            coverage: pattern
                .chars()
                .map(|c| if c == '#' { 255 } else { 0 })
                .collect(),
        }
    }

    #[test]
    fn words_split_at_spaces_not_between_letters() {
        // "ab" with one column between letters, a three-column space, then "c".
        let l = line("#.#...#", Channel::Whisper);
        assert_eq!(words(&l), [[0, 4], [5, 7]]);
        assert_eq!(words(&line("....", Channel::Party)), Vec::<[u32; 2]>::new());
        assert_eq!(words(&line("##..", Channel::Party)), [[0, 3]]);
    }

    #[test]
    fn the_log_keeps_lines_in_order_and_counts_gaps() {
        let log = ChatLog::default();
        let reading = |gap: bool, channels: &[Channel]| ChatReading {
            lines: channels.iter().map(|&c| line("#.#", c)).collect(),
            gap,
            place: None,
        };
        let first = log.record(&reading(false, &[Channel::Whisper, Channel::Party]), 10);
        assert_eq!(first.iter().map(|e| e.id).collect::<Vec<_>>(), [1, 2]);
        assert!(first[0].png.starts_with("data:image/png;base64,iVBOR"));
        assert_eq!(first[0].gaps, 0);
        let second = log.record(&reading(true, &[Channel::Friend]), 20);
        assert_eq!((second[0].id, second[0].gaps, second[0].at_ms), (3, 1, 20));
        // A gap with no lines still counts, so the next line shows lines may be missing.
        log.record(&reading(true, &[]), 30);
        let third = log.record(&reading(false, &[Channel::Friend]), 40);
        assert_eq!(third[0].gaps, 2);
        assert_eq!(
            log.history().iter().map(|e| e.channel).collect::<Vec<_>>(),
            [
                Channel::Whisper,
                Channel::Party,
                Channel::Friend,
                Channel::Friend
            ]
        );
    }

    #[test]
    fn the_log_drops_its_oldest_lines_past_its_size() {
        let log = ChatLog::default();
        let one = ChatReading {
            lines: vec![line("#", Channel::Party)],
            gap: false,
            place: None,
        };
        for _ in 0..KEEP + 5 {
            log.record(&one, 0);
        }
        let history = log.history();
        assert_eq!(history.len(), KEEP);
        assert_eq!(history[0].id, 6);
    }
}
