//! In-app updates. MesoVibe looks for a newer version on GitHub Releases, downloads it in the
//! background, checks its signature, and installs it only when the player clicks "Restart to
//! update". Installing exits MesoVibe at once (and on Windows shows the installer's progress
//! window), so it never happens on its own.
//!
//! ```text
//!   start ──30 s──► look ─┬─ newer ──► download, signature check ──► Ready ──click──► install,
//!     ▲                   │                                                   restart, panel back
//!     │                   ├─ nothing newer, or no release published yet ──► UpToDate
//!     │                   └─ GitHub unreachable ──► Failed ──1 h──► look
//!     └── 24 h after a look GitHub answered; "Check now" at any time
//! ```
//!
//! A look fetches the static `latest.json` of the newest release and sends nothing about the
//! player or the game, only the plugin's user agent. Settings > About says so. Debug builds
//! never look: an update would replace the development build.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Error as UpdateError, Update, UpdaterExt};

use crate::dock;
use crate::presence;

/// The first look after MesoVibe starts.
const FIRST_CHECK: Duration = Duration::from_secs(30);
/// Between looks GitHub answered.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// After a look that failed: offline, or GitHub down.
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// The longest wait before the schedule is read again. The schedule is wall-clock time, which
/// keeps going while the computer sleeps, so a long wait would oversleep a look that came due.
const WAKE_EVERY: Duration = Duration::from_secs(10 * 60);
/// How long asking GitHub may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the download may take, on a slow connection.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Where updating stands, for the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// No look yet.
    Idle,
    Checking,
    UpToDate,
    Downloading,
    /// Downloaded and checked: "Restart to update".
    Ready,
    Installing,
    Failed,
    /// A development build, which never looks.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub state: State,
    /// This copy's version.
    pub current: String,
    /// The new version while it downloads, is ready or installs.
    pub version: Option<String>,
    /// Why the last look failed, in words the player can act on.
    pub error: Option<String>,
    /// When GitHub last answered a look, in ms since the epoch.
    pub checked_at_ms: Option<u64>,
}

struct Inner {
    status: UpdateStatus,
    /// The downloaded, checked update, waiting for "Restart to update".
    ready: Option<(Update, Vec<u8>)>,
}

/// The update state, owned by its own thread; the panel reads it and asks for looks and the
/// install through commands (main.rs).
pub struct Updates {
    /// Wakes the thread for "Check now". `None` when it never started (a debug build).
    commands: Option<Sender<()>>,
    inner: Arc<Mutex<Inner>>,
}

impl Updates {
    pub fn start(app: &AppHandle) -> Updates {
        let off = cfg!(debug_assertions);
        let inner = Arc::new(Mutex::new(Inner {
            status: UpdateStatus {
                state: if off { State::Off } else { State::Idle },
                current: app.package_info().version.to_string(),
                version: None,
                error: None,
                checked_at_ms: None,
            },
            ready: None,
        }));
        if off {
            return Updates {
                commands: None,
                inner,
            };
        }
        let (commands, inbox) = mpsc::channel();
        let (app, shared) = (app.clone(), Arc::clone(&inner));
        let started = std::thread::Builder::new()
            .name("mesovibe-updates".to_string())
            .spawn(move || run(&app, &shared, &inbox))
            .is_ok();
        Updates {
            commands: started.then_some(commands),
            inner,
        }
    }

    pub fn status(&self) -> UpdateStatus {
        lock(&self.inner).status.clone()
    }

    /// "Check now": a look at once.
    pub fn check_now(&self) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(());
        }
    }

    /// "Restart to update": installs the ready update and restarts into it. Returns only when it
    /// couldn't, with the reason in the player's words; the update stays ready.
    pub fn install(&self, app: &AppHandle) -> Result<(), String> {
        // A Mac copy run from the disk image can't replace itself.
        if let Some(problem) = presence::install_problem() {
            return Err(problem);
        }
        let (update, bytes) = lock(&self.inner)
            .ready
            .take()
            .ok_or("There's no update ready to install.")?;
        step(app, &self.inner, Step::Installing);
        presence::before_update_restart(app);
        // Windows: installing ends this process with `std::process::exit`, which skips the exit
        // event where the single-instance plugin releases its lock (its `destroy` docs).
        #[cfg(windows)]
        tauri_plugin_single_instance::destroy(app);
        match update.install(&bytes) {
            // On a Mac the new app is in place. On Windows this isn't reached: the installer runs
            // and relaunches MesoVibe with `/UPDATE`, so hooks.nsh keeps the login item.
            Ok(()) => app.restart(),
            Err(error) => {
                presence::cancel_update_restart(app);
                let version = update.version.clone();
                lock(&self.inner).ready = Some((update, bytes));
                step(app, &self.inner, Step::Downloaded { version });
                Err(format!("MesoVibe couldn't install the update: {error}"))
            }
        }
    }
}

/// The update thread: looks when due or asked, downloads what it finds.
fn run(app: &AppHandle, inner: &Mutex<Inner>, inbox: &Receiver<()>) {
    let started = now_ms();
    let mut last: Option<(u64, Outcome)> = None;
    loop {
        let due = next_check(started, last);
        let now = now_ms();
        let wait = Duration::from_millis(due.saturating_sub(now)).min(WAKE_EVERY);
        let asked = match inbox.recv_timeout(wait) {
            Ok(()) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        // One look answers every click made meanwhile.
        while inbox.try_recv().is_ok() {}
        if !asked && now_ms() < due {
            continue;
        }
        last = Some((now_ms(), look(app, inner)));
    }
}

/// One look, and the download of whatever newer version it finds.
fn look(app: &AppHandle, inner: &Mutex<Inner>) -> Outcome {
    step(app, inner, Step::Checking);
    let checked = tauri::async_runtime::block_on(async {
        app.updater_builder()
            .timeout(CHECK_TIMEOUT)
            .build()?
            .check()
            .await
    });
    let at_ms = now_ms();
    let mut update = match after_check(checked) {
        Err(words) => {
            step(app, inner, Step::Failed { words });
            return Outcome::Failed;
        }
        Ok(None) => {
            step(
                app,
                inner,
                Step::Answered {
                    at_ms,
                    downloading: None,
                },
            );
            return Outcome::Answered;
        }
        Ok(Some(update)) => update,
    };
    let already = lock(inner)
        .ready
        .as_ref()
        .is_some_and(|(ready, _)| ready.version == update.version);
    step(
        app,
        inner,
        Step::Answered {
            at_ms,
            downloading: (!already).then(|| update.version.clone()),
        },
    );
    if already {
        return Outcome::Answered;
    }
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    // `download` checks the signature against the public key in tauri.conf.json.
    match tauri::async_runtime::block_on(update.download(|_, _| {}, || {})) {
        Ok(bytes) => {
            let version = update.version.clone();
            lock(inner).ready = Some((update, bytes));
            step(app, inner, Step::Downloaded { version });
            Outcome::Answered
        }
        Err(error) => {
            step(
                app,
                inner,
                Step::Failed {
                    words: words(&error),
                },
            );
            Outcome::Failed
        }
    }
}

/// How the last look went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Answered,
    Failed,
}

/// When the next look is due (ms since the epoch), from when MesoVibe started and how the last
/// look went.
fn next_check(started_ms: u64, last: Option<(u64, Outcome)>) -> u64 {
    let after = |at: u64, wait: Duration| at.saturating_add(wait.as_millis() as u64);
    match last {
        None => after(started_ms, FIRST_CHECK),
        Some((at, Outcome::Answered)) => after(at, CHECK_EVERY),
        Some((at, Outcome::Failed)) => after(at, RETRY_AFTER),
    }
}

/// What a look means: `Ok(None)` nothing newer, `Ok(Some(_))` a newer version, `Err` the words to
/// show.
fn after_check<T>(checked: Result<Option<T>, UpdateError>) -> Result<Option<T>, String> {
    match checked {
        Ok(found) => Ok(found),
        // No release has published latest.json yet (GitHub answers 404): nothing newer exists.
        Err(UpdateError::ReleaseNotFound) => Ok(None),
        Err(error) => Err(words(&error)),
    }
}

/// A failed look or download in words the player can act on.
fn words(error: &UpdateError) -> String {
    match error {
        UpdateError::Reqwest(_) | UpdateError::Network(_) => {
            "MesoVibe couldn't reach GitHub to check for updates. It tries again in an hour."
                .to_string()
        }
        UpdateError::Minisign(_) | UpdateError::Base64(_) | UpdateError::SignatureUtf8(_) => {
            "The update didn't pass its signature check, so it wasn't installed.".to_string()
        }
        other => format!("MesoVibe couldn't check for updates: {other}"),
    }
}

/// What happened, for [`apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Checking,
    /// GitHub answered: nothing newer (`downloading: None`), or a newer version to download.
    Answered {
        at_ms: u64,
        downloading: Option<String>,
    },
    /// The update downloaded and passed its signature check.
    Downloaded {
        version: String,
    },
    Failed {
        words: String,
    },
    Installing,
}

/// The panel's status after `step`. Once an update is ready the panel keeps saying so: a later
/// look that fails, finds nothing or downloads a newer one doesn't hide it.
fn apply(status: &mut UpdateStatus, step: Step) {
    let ready = matches!(status.state, State::Ready | State::Installing);
    match step {
        Step::Checking if !ready => {
            status.state = State::Checking;
        }
        Step::Answered { at_ms, downloading } => {
            status.checked_at_ms = Some(at_ms);
            if !ready {
                status.error = None;
                status.state = match downloading {
                    Some(_) => State::Downloading,
                    None => State::UpToDate,
                };
                status.version = downloading;
            }
        }
        Step::Downloaded { version } => {
            status.state = State::Ready;
            status.version = Some(version);
            status.error = None;
        }
        Step::Failed { words } if !ready => {
            status.state = State::Failed;
            status.version = None;
            status.error = Some(words);
        }
        Step::Installing => status.state = State::Installing,
        Step::Checking | Step::Failed { .. } => {}
    }
}

/// Applies `step` and tells the panel. Only the panel listens: never every window.
fn step(app: &AppHandle, inner: &Mutex<Inner>, step: Step) {
    let status = {
        let mut inner = lock(inner);
        apply(&mut inner.status, step);
        inner.status.clone()
    };
    let _ = app.emit_to(dock::PANEL, "updates://status", status);
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 60 * 60 * 1000;

    fn fresh() -> UpdateStatus {
        UpdateStatus {
            state: State::Idle,
            current: "0.1.0".to_string(),
            version: None,
            error: None,
            checked_at_ms: None,
        }
    }

    // Fails if the intervals swap or a failure waits a day. The thread's wake-ups (WAKE_EVERY)
    // aren't covered here.
    #[test]
    fn looks_come_30_s_after_start_then_daily_or_an_hour_after_a_failure() {
        let start = 1_000_000;
        assert_eq!(next_check(start, None), start + 30_000);
        assert_eq!(
            next_check(start, Some((start + HOUR, Outcome::Answered))),
            start + 25 * HOUR
        );
        assert_eq!(
            next_check(start, Some((start + HOUR, Outcome::Failed))),
            start + 2 * HOUR
        );
    }

    // Fails if a 404 reads as a failure, or a signature error as a network one. The errors are
    // built in the test, not taken from GitHub's real responses.
    #[test]
    fn a_missing_release_is_up_to_date_and_failures_get_plain_words() {
        assert_eq!(
            after_check::<()>(Err(UpdateError::ReleaseNotFound)),
            Ok(None)
        );
        assert_eq!(after_check(Ok(Some(7))), Ok(Some(7)));
        let offline = after_check::<()>(Err(UpdateError::Network("timed out".into()))).unwrap_err();
        assert!(offline.contains("couldn't reach GitHub"), "{offline}");
        let tampered = after_check::<()>(Err(UpdateError::SignatureUtf8("x".into()))).unwrap_err();
        assert!(tampered.contains("signature check"), "{tampered}");
        let other = after_check::<()>(Err(UpdateError::TargetNotFound("darwin-aarch64".into())))
            .unwrap_err();
        assert!(other.contains("darwin-aarch64"), "{other}");
    }

    // The panel's line follows a look: checking, up to date, downloading, ready. Fails if a state
    // is skipped or a stale error stays.
    #[test]
    fn a_look_moves_the_panel_from_checking_to_ready() {
        let mut status = fresh();
        apply(&mut status, Step::Checking);
        assert_eq!(status.state, State::Checking);
        apply(
            &mut status,
            Step::Failed {
                words: "offline".into(),
            },
        );
        assert_eq!(status.state, State::Failed);
        apply(
            &mut status,
            Step::Answered {
                at_ms: 5,
                downloading: None,
            },
        );
        assert_eq!(
            (status.state, status.error.as_deref(), status.checked_at_ms),
            (State::UpToDate, None, Some(5))
        );
        apply(
            &mut status,
            Step::Answered {
                at_ms: 9,
                downloading: Some("0.2.0".into()),
            },
        );
        assert_eq!(
            (status.state, status.version.as_deref()),
            (State::Downloading, Some("0.2.0"))
        );
        apply(
            &mut status,
            Step::Downloaded {
                version: "0.2.0".into(),
            },
        );
        assert_eq!(status.state, State::Ready);
    }

    // "Restart to update" stays up once offered. Fails if a later failed or empty look hides a
    // downloaded update.
    #[test]
    fn a_ready_update_stays_offered_through_later_looks() {
        let mut status = fresh();
        apply(
            &mut status,
            Step::Downloaded {
                version: "0.2.0".into(),
            },
        );
        for step in [
            Step::Checking,
            Step::Failed {
                words: "offline".into(),
            },
            Step::Answered {
                at_ms: 42,
                downloading: None,
            },
            Step::Answered {
                at_ms: 43,
                downloading: Some("0.2.1".into()),
            },
        ] {
            apply(&mut status, step);
            assert_eq!(
                (
                    status.state,
                    status.version.as_deref(),
                    status.error.as_deref()
                ),
                (State::Ready, Some("0.2.0"), None)
            );
        }
        assert_eq!(
            status.checked_at_ms,
            Some(43),
            "the last answer is still shown"
        );
        apply(
            &mut status,
            Step::Downloaded {
                version: "0.2.1".into(),
            },
        );
        assert_eq!(
            status.version.as_deref(),
            Some("0.2.1"),
            "a newer one replaces it"
        );
    }

    // Fails if the committed public key is empty or not a minisign key, so updates could never
    // install. That it matches the private key is checked by scripts/release-manifest.mjs.
    #[test]
    fn the_committed_update_key_is_a_minisign_public_key() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let pubkey = config["plugins"]["updater"]["pubkey"]
            .as_str()
            .expect("plugins.updater.pubkey");
        let text = String::from_utf8(decode_base64(pubkey)).expect("the key file is text");
        let mut lines = text.lines();
        assert!(
            lines
                .next()
                .is_some_and(|l| l.starts_with("untrusted comment:")),
            "not a minisign key file: {text:?}"
        );
        let key = decode_base64(lines.next().expect("the key line"));
        assert_eq!(
            key.len(),
            42,
            "algorithm (2) + key id (8) + Ed25519 key (32)"
        );
        assert_eq!(&key[..2], b"Ed");
    }

    #[test]
    fn the_key_tests_decoder_inverts_the_engines_encoder() {
        for bytes in [
            &b""[..],
            b"E",
            b"Ed",
            b"Ed\x00\xff",
            b"untrusted comment: x\n",
        ] {
            assert_eq!(decode_base64(&crate::engine::base64(bytes)), bytes);
        }
    }

    /// Standard base64, for the key test only.
    fn decode_base64(text: &str) -> Vec<u8> {
        let value = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64: {c:?}"),
        };
        let digits: Vec<u8> = text
            .trim()
            .bytes()
            .filter(|&c| c != b'=')
            .map(value)
            .collect();
        digits
            .chunks(4)
            .flat_map(|chunk| {
                let n = chunk
                    .iter()
                    .enumerate()
                    .fold(0u32, |n, (i, &d)| n | u32::from(d) << (18 - 6 * i));
                let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
                bytes.into_iter().take(chunk.len() - 1)
            })
            .collect()
    }
}
