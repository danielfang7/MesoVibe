//! Pictures of game text MesoVibe can't read as words, kept by the key the engine gives each
//! name's exact shape (a map's `map_key`), so Past sessions can show a map's name long after it
//! was on screen. Kept in the app's data folder like everything else; nothing leaves the device.
//!
//! The file follows the store's rules: written whole, and never saved over when it was there but
//! couldn't be opened. It's bounded: past [`MAX_BYTES`] the pictures seen longest ago go first, and
//! their rows show "Name not available" until the name is seen again.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use mesovibe_engine::recognizers::NamePicture;
use serde::{Deserialize, Serialize};

use crate::chat::coverage_png;
use crate::store::{try_read_json, write_bytes};

/// The most the pictures may take on disk, as JSON.
const MAX_BYTES: usize = 4 * 1024 * 1024;

/// One name's picture, as the panel draws it: a PNG data URL, white with the text as alpha.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub png: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    #[serde(flatten)]
    picture: Picture,
    /// When the name was last on screen, in milliseconds since the epoch.
    seen_ms: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    names: BTreeMap<String, Kept>,
}

pub struct GameText {
    path: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    names: BTreeMap<String, Kept>,
    /// The file was there but couldn't be opened: nothing is saved over it this run.
    unreadable: bool,
}

impl GameText {
    /// Opens the pictures kept in `path`.
    pub fn open(path: PathBuf) -> GameText {
        let (names, unreadable) = match try_read_json::<Saved>(&path) {
            Ok(saved) => (saved.map(|s| s.names).unwrap_or_default(), false),
            Err(_) => (BTreeMap::new(), true),
        };
        GameText {
            path,
            inner: Mutex::new(Inner { names, unreadable }),
        }
    }

    /// Keeps the picture of a name on screen now. A name already kept is only marked seen, and
    /// saved with the next new one.
    pub fn remember(&self, key: &str, picture: &NamePicture, now_ms: u64) -> Result<(), String> {
        let mut inner = self.lock();
        if let Some(kept) = inner.names.get_mut(key) {
            kept.seen_ms = now_ms;
            return Ok(());
        }
        let png = coverage_png(picture.width, picture.height, &picture.coverage)
            .ok_or("the picture couldn't be encoded")?;
        let kept = Kept {
            picture: Picture {
                width: picture.width,
                height: picture.height,
                png,
            },
            seen_ms: now_ms,
        };
        inner.names.insert(key.to_string(), kept);
        if inner.unreadable {
            return Ok(());
        }
        let bytes = loop {
            let saved = Saved {
                version: 1,
                names: std::mem::take(&mut inner.names),
            };
            let bytes = serde_json::to_vec(&saved).map_err(|e| e.to_string())?;
            inner.names = saved.names;
            if bytes.len() <= MAX_BYTES || inner.names.len() <= 1 {
                break bytes;
            }
            let oldest = inner
                .names
                .iter()
                .min_by_key(|(_, kept)| kept.seen_ms)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                inner.names.remove(&oldest);
            }
        };
        write_bytes(&self.path, &bytes)
    }

    /// The pictures kept for `keys`; a key with none is left out.
    pub fn pictures(&self, keys: &[String]) -> BTreeMap<String, Picture> {
        let inner = self.lock();
        keys.iter()
            .filter_map(|key| Some((key.clone(), inner.names.get(key)?.picture.clone())))
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mesovibe-game-text-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A picture `width` wide of noise, which PNG can't shrink much.
    fn picture(width: u32) -> NamePicture {
        let mut x: u32 = 0x9e37_79b9;
        NamePicture {
            width,
            height: 9,
            coverage: (0..width * 9)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    (x >> 24) as u8
                })
                .collect(),
        }
    }

    // A map seen in one run still has its picture in the next. Fails if pictures aren't saved,
    // or a key reads back another name's picture.
    #[test]
    fn pictures_survive_a_restart() {
        let dir = temp("restart");
        let path = dir.join("game-text.json");
        let names = GameText::open(path.clone());
        names.remember("aaaa", &picture(40), 1).unwrap();
        names.remember("bbbb", &picture(60), 2).unwrap();
        let again = GameText::open(path);
        let found = again.pictures(&["bbbb".to_string(), "missing".to_string()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found["bbbb"].width, 60);
        assert!(found["bbbb"].png.starts_with("data:image/png;base64,"));
        let _ = std::fs::remove_dir_all(dir);
    }

    // The store's rule for files it can't open: keep them as they are. Fails if a new picture is
    // saved over a file that couldn't be read.
    #[test]
    fn a_file_that_cant_be_opened_is_never_saved_over() {
        let dir = temp("locked");
        let path = dir.join("game-text.json");
        std::fs::create_dir_all(&path).unwrap();
        let names = GameText::open(path.clone());
        names.remember("aaaa", &picture(40), 1).unwrap();
        assert!(path.is_dir(), "nothing written in its place");
        assert_eq!(
            names.pictures(&["aaaa".to_string()]).len(),
            1,
            "kept for this run"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // The file stays bounded. Fails if it grows past the bound, or if the names seen most
    // recently are the ones dropped.
    #[test]
    fn past_the_bound_the_names_seen_longest_ago_go_first() {
        let dir = temp("bound");
        let path = dir.join("game-text.json");
        let names = GameText::open(path.clone());
        let big = picture(12_000);
        let mut kept = 0;
        for i in 0..60u64 {
            names.remember(&format!("{i:016x}"), &big, i).unwrap();
            kept += 1;
            if std::fs::metadata(&path).unwrap().len() as usize > MAX_BYTES {
                panic!("over the bound after {kept}");
            }
        }
        let all: Vec<String> = (0..60u64).map(|i| format!("{i:016x}")).collect();
        let found = names.pictures(&all);
        assert!(found.len() < 60, "some were dropped");
        assert!(
            found.contains_key(&format!("{:016x}", 59)),
            "the newest stays"
        );
        assert!(
            !found.contains_key(&format!("{:016x}", 0)),
            "the oldest goes"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
