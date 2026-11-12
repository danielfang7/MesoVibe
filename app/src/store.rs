//! What MesoVibe keeps on this computer: settings, each plugin's own data (JSON files), and the
//! shapes the readers confirmed (the EXP bar's digits in `exp-bar.txt`, the pickup notices'
//! letters in `notices.txt`, so later sessions read from the first frame), all in the app's data
//! folder. Nothing leaves the device.
//!
//! Writes go to a temporary file that is then renamed over the old one, so a crash never leaves a
//! half-written file. A file whose contents can't be understood (a hand edit, a damaged disk) is
//! set aside as `<name>.corrupt` and MesoVibe starts from defaults, rather than refusing to open.
//! A plugin's file that can't be opened at all (locked by another program, a permission) is a
//! different case: it may be fine, so nothing is saved over it until it can be read.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dock::{ChatPosition, Corner};

/// The largest blob a plugin may keep: enough for months of session history.
const MAX_PLUGIN_BYTES: usize = 4 * 1024 * 1024;
/// A reader whose confirmed shapes are kept between sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Learned {
    /// The EXP bar's digits ([`mesovibe_engine::recognizers::exp_bar::ExpBarReader::knowledge`]).
    ExpBar,
    /// The pickup notices' letters and digits
    /// ([`mesovibe_engine::recognizers::notices::NoticeReader::knowledge`]).
    Notices,
}

impl Learned {
    fn file(self) -> &'static str {
        match self {
            Learned::ExpBar => "exp-bar.txt",
            Learned::Notices => "notices.txt",
        }
    }
}
/// There while MesoVibe restarts into an update the player chose: the next start opens the
/// panel, whatever it was started with (presence.rs). A one-time marker, not a choice, so it
/// isn't in [`Settings`].
const OPEN_AFTER_UPDATE_FILE: &str = "open-after-update";

/// The player's choices. A field missing from the file (one saved by an older MesoVibe) takes
/// its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// MesoVibe's first start on this computer is done: it turned "Open with MapleStory Classic"
    /// on (presence.rs). MesoVibe 0.3.0 and earlier showed a first-run screen and saved this as
    /// `disclosureAccepted`, so players who already set it up aren't set up again.
    #[serde(alias = "disclosureAccepted")]
    pub first_run_done: bool,
    /// Capture off: MesoVibe reads nothing.
    pub safe_mode: bool,
    /// Keep MesoVibe with the game: the panel docks beside it, or the mini-HUD stands in.
    pub dock: bool,
    /// The mini-HUD may sit over the game when there's no room for the panel.
    pub mini_hud: bool,
    /// The corner of the game area the mini-HUD sits in.
    pub mini_hud_corner: Corner,
    /// The chat overlay: the latest whisper, party and friend lines over the game (Chat Hub). On
    /// until the player turns it off.
    pub chat_overlay: bool,
    /// Where the chat overlay sits: on the game's chat box, or in a box at either side.
    pub chat_overlay_position: ChatPosition,
    /// Which chat types the overlay shows.
    pub chat_overlay_channels: ChatChannels,
    /// The color of each chat type's text in the overlay.
    pub chat_overlay_colors: ChatColors,
    /// Where the panel was when it last closed, so it comes back there and keeps the width the
    /// player gave it (presence.rs rebuilds the panel for each play session).
    pub panel: Option<PanelFrame>,
}

/// The panel's visible frame, in desktop units (points on a Mac, pixels on Windows; dock.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelFrame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A choice of chat types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatChannels {
    pub whisper: bool,
    pub party: bool,
    pub friend: bool,
}

impl Default for ChatChannels {
    fn default() -> ChatChannels {
        ChatChannels {
            whisper: true,
            party: true,
            friend: true,
        }
    }
}

/// A text color for each chat type, as `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatColors {
    pub whisper: String,
    pub party: String,
    pub friend: String,
}

impl Default for ChatColors {
    /// The game's own colors for each chat type: green whispers, pink party chat and orange
    /// friend chat, so a line over the game reads as the kind it is. Each is one of the panel's
    /// named swatches (ui/src/main.ts).
    fn default() -> ChatColors {
        ChatColors {
            whisper: "#66ff66".into(),
            party: "#ff88cc".into(),
            friend: "#ffaa44".into(),
        }
    }
}

/// `#` and six lowercase hex digits.
fn is_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl ChatColors {
    /// Every color is `#` and six lowercase hex digits, so nothing else reaches a page's styles.
    pub fn valid(&self) -> bool {
        [&self.whisper, &self.party, &self.friend]
            .iter()
            .all(|color| is_color(color))
    }

    /// The same colors, with any that isn't `#rrggbb` (from a hand-edited or damaged settings
    /// file) back to that chat type's default.
    fn repaired(self) -> ChatColors {
        let default = ChatColors::default();
        let keep = |color: String, default: String| if is_color(&color) { color } else { default };
        ChatColors {
            whisper: keep(self.whisper, default.whisper),
            party: keep(self.party, default.party),
            friend: keep(self.friend, default.friend),
        }
    }
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            first_run_done: false,
            safe_mode: false,
            dock: true,
            mini_hud: true,
            mini_hud_corner: Corner::TopRight,
            chat_overlay: true,
            chat_overlay_position: ChatPosition::Left,
            chat_overlay_channels: ChatChannels::default(),
            chat_overlay_colors: ChatColors::default(),
            panel: None,
        }
    }
}

pub struct Store {
    dir: PathBuf,
    settings: Mutex<Settings>,
    /// Plugins whose saved data couldn't be opened since the last read that worked. Their saves are
    /// refused, so a plugin that started with nothing never overwrites history it couldn't read.
    unreadable: Mutex<HashSet<String>>,
}

impl Store {
    /// Opens the store in `dir`, creating it if needed.
    pub fn open(dir: PathBuf) -> Store {
        let _ = std::fs::create_dir_all(dir.join("plugins"));
        let mut settings: Settings = read_json(&dir.join("settings.json")).unwrap_or_default();
        // A bad color kept as it is would be refused with every change to the chat overlay's
        // settings (set_chat_overlay), leaving the player unable to change them.
        settings.chat_overlay_colors = settings.chat_overlay_colors.repaired();
        Store {
            dir,
            settings: Mutex::new(settings),
            unreadable: Mutex::new(HashSet::new()),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Changes the settings and saves them.
    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
        let mut settings = self
            .settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = settings.clone();
        change(&mut next);
        write_json(&self.dir.join("settings.json"), &next)?;
        *settings = next.clone();
        Ok(next)
    }

    /// A plugin's data, or `null` when it has kept nothing yet. An error when the file is there but
    /// can't be opened; until a later read works, that plugin's saves are refused.
    pub fn plugin_get(&self, plugin: &str) -> Result<Value, String> {
        let path = self.plugin_path(plugin)?;
        let read = try_read_json(&path);
        let mut unreadable = self
            .unreadable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match read {
            Ok(value) => {
                unreadable.remove(plugin);
                Ok(value.unwrap_or(Value::Null))
            }
            Err(e) => {
                unreadable.insert(plugin.to_string());
                Err(e.to_string())
            }
        }
    }

    /// Replaces a plugin's data.
    pub fn plugin_set(&self, plugin: &str, value: &Value) -> Result<(), String> {
        let path = self.plugin_path(plugin)?;
        let unreadable = self
            .unreadable
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(plugin);
        if unreadable {
            return Err(format!(
                "{plugin}'s saved data couldn't be read, so nothing is saved over it."
            ));
        }
        let size = serde_json::to_vec(value).map_err(|e| e.to_string())?.len();
        if size > MAX_PLUGIN_BYTES {
            return Err(format!(
                "{plugin} tried to keep {size} bytes; the limit is {MAX_PLUGIN_BYTES}."
            ));
        }
        write_json(&path, value)
    }

    /// What a reader confirmed in an earlier session, if any.
    pub fn knowledge(&self, reader: Learned) -> Option<String> {
        std::fs::read_to_string(self.dir.join(reader.file())).ok()
    }

    /// Keeps what a reader confirmed, or forgets it (`None`: it no longer fits the game).
    pub fn keep_knowledge(&self, reader: Learned, text: Option<&str>) -> Result<(), String> {
        let path = self.dir.join(reader.file());
        match text {
            Some(text) => write_bytes(&path, text.as_bytes()),
            None => match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("{}: {e}", path.display()))
                }
                _ => Ok(()),
            },
        }
    }

    /// MesoVibe is about to restart into an update the player chose: the next start opens the
    /// panel.
    pub fn mark_open_after_update(&self) -> Result<(), String> {
        write_bytes(&self.dir.join(OPEN_AFTER_UPDATE_FILE), b"")
    }

    /// Whether this start follows an update the player chose, forgetting it so only this start
    /// opens the panel for it.
    pub fn take_open_after_update(&self) -> bool {
        std::fs::remove_file(self.dir.join(OPEN_AFTER_UPDATE_FILE)).is_ok()
    }

    fn plugin_path(&self, plugin: &str) -> Result<PathBuf, String> {
        if !valid_plugin_id(plugin) {
            return Err(format!("\"{plugin}\" isn't a plugin id."));
        }
        Ok(self.dir.join("plugins").join(format!("{plugin}.json")))
    }
}

/// Plugin ids are kebab-case names (the SDK's rule), so they can't name a path elsewhere.
pub fn valid_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A JSON file's contents. `None` when it doesn't exist, or when it can't be read, in which case
/// it's set aside as `<name>.corrupt`.
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    try_read_json(path).ok().flatten()
}

/// A JSON file's contents: `None` when it doesn't exist, or when its contents can't be understood,
/// in which case it's set aside. An error when the file can't be opened (another program holds
/// it, a permission): it may be fine, so the caller mustn't treat it as empty.
pub(crate) fn try_read_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> std::io::Result<Option<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        // Not UTF-8: unreadable contents, like a JSON error.
        Err(e) if e.kind() == ErrorKind::InvalidData => {
            set_aside(path);
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    match serde_json::from_str(&text) {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            set_aside(path);
            Ok(None)
        }
    }
}

/// Moves a file whose contents can't be understood to `<name>.json.corrupt`, or, when that's taken
/// by an earlier one, to a name with the time, so an older copy is never replaced.
fn set_aside(path: &Path) {
    let mut aside = path.with_extension("json.corrupt");
    if aside.exists() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        aside = path.with_extension(format!("json.corrupt-{secs}"));
    }
    let _ = std::fs::rename(path, aside);
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    write_bytes(path, &text)
}

/// Writes a file whole: to `<name>.part`, then renamed over the old one.
pub(crate) fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut partial = path.as_os_str().to_owned();
    partial.push(".part");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, bytes).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mesovibe-store-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    // Each reader's shapes come back after a restart, apart from the other's, until forgotten.
    // Fails if one reader's file overwrites the other's, or forgetting leaves the file behind.
    #[test]
    fn what_the_readers_learned_survives_a_restart_until_forgotten() {
        let dir = temp_dir("learned");
        let store = Store::open(dir.clone());
        assert_eq!(store.knowledge(Learned::ExpBar), None);
        assert_eq!(store.knowledge(Learned::Notices), None);
        store
            .keep_knowledge(Learned::ExpBar, Some("mesovibe exp-bar 1\n"))
            .unwrap();
        store
            .keep_knowledge(Learned::Notices, Some("mesovibe notices 1\n"))
            .unwrap();
        let again = Store::open(dir.clone());
        assert_eq!(
            again.knowledge(Learned::ExpBar).as_deref(),
            Some("mesovibe exp-bar 1\n")
        );
        assert_eq!(
            again.knowledge(Learned::Notices).as_deref(),
            Some("mesovibe notices 1\n")
        );
        again.keep_knowledge(Learned::ExpBar, None).unwrap();
        again.keep_knowledge(Learned::ExpBar, None).unwrap();
        let last = Store::open(dir.clone());
        assert_eq!(last.knowledge(Learned::ExpBar), None);
        assert!(last.knowledge(Learned::Notices).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    // The panel opens once after an update the player chose, on the restarted process. Fails if
    // the marker doesn't survive the restart or is never cleared.
    #[test]
    fn the_open_after_update_marker_is_read_once_across_a_restart() {
        let dir = temp_dir("after-update");
        let store = Store::open(dir.clone());
        assert!(!store.take_open_after_update(), "nothing marked yet");
        store.mark_open_after_update().unwrap();
        let restarted = Store::open(dir.clone());
        assert!(restarted.take_open_after_update());
        assert!(
            !restarted.take_open_after_update(),
            "only the first start opens the panel"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // Chat colors reach the overlay page's styles, so only plain #rrggbb colors are kept. Fails
    // if anything else gets through, or the default stops being valid.
    #[test]
    fn chat_colors_are_plain_hex_colors() {
        assert!(ChatColors::default().valid());
        let with = |color: &str| ChatColors {
            party: color.into(),
            ..ChatColors::default()
        };
        assert!(with("#ff99cc").valid());
        for bad in [
            "#FF99CC",
            "#ff99c",
            "ff99cc0",
            "#ff99cg",
            "red",
            "#ff99cc;x",
            "",
        ] {
            assert!(!with(bad).valid(), "{bad}");
        }
    }

    #[test]
    fn settings_survive_a_restart() {
        let dir = temp_dir("settings");
        let store = Store::open(dir.clone());
        assert_eq!(store.settings(), Settings::default());
        store
            .update(|s| {
                s.first_run_done = true;
                s.safe_mode = true;
            })
            .unwrap();
        let again = Store::open(dir.clone());
        assert!(again.settings().first_run_done && again.settings().safe_mode);
        let _ = std::fs::remove_dir_all(dir);
    }

    // MesoVibe 0.3.0 and earlier saved the first-run flag as "disclosureAccepted". Saved again,
    // the file has the new name only: both names in one file would make it unreadable. Fails if
    // the old name is written back or the flag is lost on the way.
    #[test]
    fn the_first_run_flag_is_saved_under_its_new_name() {
        let dir = temp_dir("first-run-name");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{ "disclosureAccepted": true }"#,
        )
        .unwrap();
        Store::open(dir.clone()).update(|_| {}).unwrap();
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(text.contains("\"firstRunDone\": true") && !text.contains("disclosureAccepted"));
        assert!(Store::open(dir.clone()).settings().first_run_done);
        let _ = std::fs::remove_dir_all(dir);
    }

    // A 0.1.0 settings file has no display fields, and must keep its choices rather than be set
    // aside as unreadable. Its "disclosureAccepted" means the first start is done, so MesoVibe
    // doesn't turn "Open with MapleStory Classic" back on for a player who turned it off. Fails if
    // a new field has no default or the old name is no longer read.
    #[test]
    fn settings_saved_by_an_older_mesovibe_keep_their_choices() {
        let dir = temp_dir("older");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{ "disclosureAccepted": true, "safeMode": false }"#,
        )
        .unwrap();
        let settings = Store::open(dir.clone()).settings();
        assert!(settings.first_run_done);
        assert!(settings.dock && settings.mini_hud);
        assert_eq!(settings.mini_hud_corner, Corner::TopRight);
        assert_eq!(settings.panel, None);
        assert!(settings.chat_overlay);
        assert_eq!(settings.chat_overlay_colors, ChatColors::default());
        assert_eq!(settings.chat_overlay_position, ChatPosition::Left);
        assert!(!dir.join("settings.json.corrupt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    // A hand-edited or damaged chat color must not break the overlay or lose other settings.
    // Fails if the bad color is kept, or the whole file is set aside as corrupt.
    #[test]
    fn a_bad_chat_color_in_the_file_falls_back_to_that_chat_types_default() {
        let dir = temp_dir("bad-color");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r##"{ "disclosureAccepted": true, "chatOverlayColors": { "whisper": "#ffffff", "party": "red", "friend": "#00ffff" } }"##,
        )
        .unwrap();
        let settings = Store::open(dir.clone()).settings();
        assert!(settings.first_run_done);
        let colors = &settings.chat_overlay_colors;
        assert_eq!(colors.whisper, "#ffffff");
        assert_eq!(colors.party, ChatColors::default().party);
        assert_eq!(colors.friend, "#00ffff");
        assert!(!dir.join("settings.json.corrupt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unreadable_file_is_set_aside_and_defaults_are_used() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{ not json").unwrap();
        let store = Store::open(dir.clone());
        assert_eq!(store.settings(), Settings::default());
        assert!(dir.join("settings.json.corrupt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plugins_keep_their_own_data_and_nothing_else() {
        let dir = temp_dir("plugins");
        let store = Store::open(dir.clone());
        assert_eq!(store.plugin_get("session-tracker").unwrap(), Value::Null);
        let value = serde_json::json!({ "sessions": [1, 2, 3] });
        store.plugin_set("session-tracker", &value).unwrap();
        assert_eq!(store.plugin_get("session-tracker").unwrap(), value);
        for id in ["", "../settings", "a/b", "Session", "-x", &"x".repeat(65)] {
            assert!(store.plugin_set(id, &value).is_err(), "{id:?}");
            assert!(store.plugin_get(id).is_err(), "{id:?}");
        }
        let big = Value::String("x".repeat(MAX_PLUGIN_BYTES + 1));
        assert!(store.plugin_set("session-tracker", &big).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    // A plugin's file that can't be opened (here a folder in its place, which fails to open on
    // every OS) must not read as "nothing saved", or the plugin's next save replaces its history.
    // Fails if the read returns null or the save goes through; and if a later good read doesn't
    // let saves through again.
    #[test]
    fn plugin_data_that_cant_be_opened_is_never_saved_over() {
        let dir = temp_dir("plugin-locked");
        let store = Store::open(dir.clone());
        let file = dir.join("plugins").join("session-tracker.json");
        std::fs::create_dir_all(&file).unwrap();
        assert!(store.plugin_get("session-tracker").is_err());
        let value = serde_json::json!({ "version": 1 });
        assert!(store.plugin_set("session-tracker", &value).is_err());
        assert!(file.is_dir(), "nothing was written in its place");
        assert!(
            store.plugin_set("other-plugin", &value).is_ok(),
            "other plugins still save"
        );

        std::fs::remove_dir(&file).unwrap();
        assert_eq!(store.plugin_get("session-tracker").unwrap(), Value::Null);
        store.plugin_set("session-tracker", &value).unwrap();
        assert_eq!(store.plugin_get("session-tracker").unwrap(), value);
        let _ = std::fs::remove_dir_all(dir);
    }

    // The case players hit: a real file another program or a permission keeps closed. Fails if
    // its bytes change after the plugin tries to save.
    #[cfg(unix)]
    #[test]
    fn a_plugin_file_without_read_permission_keeps_its_bytes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("plugin-permission");
        let store = Store::open(dir.clone());
        let file = dir.join("plugins").join("session-tracker.json");
        std::fs::write(&file, br#"{"version":1,"history":[1,2,3]}"#).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&file).is_ok() {
            // Running as root: permissions don't stop the read, so there's nothing to check.
            let _ = std::fs::remove_dir_all(dir);
            return;
        }
        assert!(store.plugin_get("session-tracker").is_err());
        assert!(
            store
                .plugin_set("session-tracker", &serde_json::json!(null))
                .is_err()
        );
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            std::fs::read(&file).unwrap(),
            br#"{"version":1,"history":[1,2,3]}"#
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // Damaged contents are set aside, and a second damaged file doesn't replace the first copy.
    // Fails if the earlier .corrupt file is overwritten.
    #[test]
    fn a_second_damaged_file_keeps_the_first_set_aside_copy() {
        let dir = temp_dir("corrupt-twice");
        let store = Store::open(dir.clone());
        let file = dir.join("plugins").join("session-tracker.json");
        std::fs::write(&file, "{ first").unwrap();
        assert_eq!(store.plugin_get("session-tracker").unwrap(), Value::Null);
        std::fs::write(&file, "{ second").unwrap();
        assert_eq!(store.plugin_get("session-tracker").unwrap(), Value::Null);
        let kept: Vec<String> = std::fs::read_dir(dir.join("plugins"))
            .unwrap()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .collect();
        assert!(kept.contains(&"{ first".to_string()));
        assert!(kept.contains(&"{ second".to_string()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
