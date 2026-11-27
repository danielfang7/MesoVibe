//! The price label over the game: each tick, from the engine's item tooltip reading and the
//! prices downloaded (prices.rs), what the label says and where it goes (dock.rs), or that it
//! hides; and which item the player points at, for the Prices tab.
//!
//! The label shows only while the latest tick confirmed the tooltip (two whole frames in a row
//! agreeing on its box's size and its name), and moves with it as the cursor drifts. It hides on
//! the first tick that doesn't confirm it, after 600 ms with no confirming tick, whenever capture
//! isn't running, and while the game isn't in front. A name read unsure shows nothing, except a
//! name held back only by letters not learned yet: then the same card says MesoVibe is learning
//! the game's letters (and, when the Prices tab can teach it, to pick it there).
//!
//! The tooltip reader is on only while the player has "Prices next to item tooltips" on; off, no
//! tooltip is read and no whole frame is grabbed for one. What it learned of the name font is
//! kept on this computer (`tooltip-font.txt`), the names the player said aren't items in their
//! own file (`tooltip-dismissed.txt`), and the price file's cash items (names the game's item list
//! doesn't have) are given to it with their ids, so they're priced like any item.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mesovibe_engine::capture::{Backend, CaptureState};
use mesovibe_engine::recognizers::tooltip::{Candidate, Stuck, TooltipReader};
use mesovibe_engine::runner::{Runner, Tick};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::dock::{self, Docker};
use crate::prices::{self, LabelView, PriceBook, Prices};
use crate::store::{Learned, Store};

/// How long a label stays without a tick confirming it, as when frames stop coming.
const HOLD: Duration = Duration::from_millis(600);

/// What the label window is told: what it says, and how wide its card is.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelShown {
    /// The price, when the label shows one.
    #[serde(flatten)]
    pub view: Option<LabelView>,
    /// When the name's letters are still being learned instead.
    pub learning: Option<LearningCard>,
    /// The card's width and the room left of it in the window, in logical pixels.
    pub card_width: f64,
    pub card_left: f64,
}

/// The card shown while a name's letters are being learned.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningCard {
    /// The Prices tab can teach this name: it lists what the name could be.
    pub pick: bool,
}

/// What the player points at, for the Prices tab (`prices://pointed`): the items named, or a name
/// being learned that the tab can teach.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pointed {
    pub ids: Vec<u32>,
    pub stuck: Option<PointedStuck>,
}

/// A name being learned, for the Prices tab's "Which item is this?": its token, which the tab's
/// answer carries back, and the items it could be.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointedStuck {
    pub token: u64,
    pub candidates: Vec<Candidate>,
}

/// Follows the engine's tooltip readings from tick to tick.
#[derive(Default)]
pub struct LabelFollower {
    /// When a tick last confirmed a tooltip.
    confirmed_at: Option<Instant>,
    /// What was last told to the Prices tab, so each pointing is told once.
    pointed: Option<Pointed>,
    /// The price file the reader was last given the names that aren't items from.
    names_from: Option<Arc<Prices>>,
    /// The reader's letters and dismissed names as last kept on this computer.
    kept: Option<String>,
    dismissed_kept: Option<String>,
    /// The name being learned that the Prices tab was last told about, by its token. Kept after
    /// its tooltip is gone: the player answers in the panel, away from the game.
    stuck: Option<(u64, Stuck)>,
    tokens: u64,
    /// The label is on.
    on: bool,
    /// Letters were kept from an earlier session, or a price label showed in this one: past
    /// learning the game's letters from scratch.
    learned: bool,
}

impl LabelFollower {
    /// One tick: switches the tooltip reader with the setting, and shows, moves or hides the label.
    pub fn follow<B: Backend>(
        &mut self,
        app: &AppHandle,
        runner: &mut Runner<B>,
        tick: &Tick,
        docker: &mut Docker,
    ) {
        let Some(store) = app.try_state::<Store>() else {
            return;
        };
        let settings = store.settings();
        self.on = settings.price_label;
        if !settings.price_label {
            if runner.tooltip_reader_mut().is_some() {
                runner.set_tooltip_reader(None);
            }
            docker.close_label(app);
            self.confirmed_at = None;
            self.names_from = None;
            self.stuck = None;
            return;
        }
        if runner.tooltip_reader_mut().is_none() {
            let kept = store.knowledge(Learned::TooltipFont);
            let mut reader = kept
                .as_deref()
                .and_then(TooltipReader::from_knowledge)
                .unwrap_or_default();
            // A dismissed-names file that can't be read is ignored on its own.
            if let Some(text) = store.knowledge(Learned::TooltipDismissed) {
                let _ = reader.keep_dismissed(&text);
            }
            self.kept = reader.knowledge();
            self.dismissed_kept = reader.dismissed_knowledge();
            self.learned |= self.kept.is_some();
            self.names_from = None;
            self.stuck = None;
            runner.set_tooltip_reader(Some(reader));
        }
        let book = app.try_state::<PriceBook>();
        let prices = book.as_ref().and_then(|book| book.prices());
        if let (Some(reader), Some(file)) = (runner.tooltip_reader_mut(), &prices)
            && self
                .names_from
                .as_ref()
                .is_none_or(|from| !Arc::ptr_eq(from, file))
        {
            let names = file.other_names();
            reader.set_other_names(
                names
                    .iter()
                    .map(|(name, ids)| (name.as_str(), ids.as_slice())),
            );
            self.names_from = Some(Arc::clone(file));
        }
        self.keep(&store, runner);

        if tick.state != CaptureState::Capturing {
            self.confirmed_at = None;
            docker.hide_label(app);
            return;
        }
        let Some(reading) = &tick.tooltip else {
            // No whole frame this tick: a shown label holds a moment, as frames may come again.
            if self.confirmed_at.is_none_or(|at| at.elapsed() > HOLD) {
                self.confirmed_at = None;
                docker.hide_label(app);
            }
            return;
        };
        let confirmed = reading.tooltip.as_ref().filter(|t| t.confirmed);
        let (Some(tooltip), Some(window)) = (confirmed, tick.window.as_ref()) else {
            self.confirmed_at = None;
            docker.hide_label(app);
            return;
        };
        self.confirmed_at = Some(Instant::now());
        let shown = if let Some(ids) = &tooltip.items {
            self.point(
                app,
                Pointed {
                    ids: ids.clone(),
                    stuck: None,
                },
            );
            prices::label(prices.as_deref(), settings.price_world, ids, now_ms())
                .map(|view| LabelView {
                    untradable: tooltip.untradable,
                    ..view
                })
                .map(|view| (Some(view), None))
        } else if let Some(stuck) = &tooltip.stuck {
            let token = self.stuck_token(stuck);
            self.point(
                app,
                Pointed {
                    ids: Vec::new(),
                    stuck: Some(PointedStuck {
                        token,
                        candidates: stuck.candidates.clone(),
                    }),
                },
            );
            let pick = !stuck.candidates.is_empty();
            Some((None, Some(LearningCard { pick })))
        } else {
            None
        };
        let place = window.placement.as_ref().and_then(|placement| {
            let frame = dock::Rect::from(placement.frame);
            let scale = dock::display_of(&frame, &dock::displays(app)).map_or(1.0, |d| d.scale);
            let r = tooltip.rect;
            dock::label_rect(
                window,
                placement,
                tick.source,
                (r.x, r.y, r.width, r.height),
                scale,
            )
        });
        match (shown, place) {
            (Some((view, learning)), Some(place)) => {
                self.learned |= view.is_some();
                docker.show_label(
                    app,
                    &place,
                    &LabelShown {
                        view,
                        learning,
                        card_width: place.card_width,
                        card_left: place.card_left,
                    },
                );
            }
            _ => docker.hide_label(app),
        }
    }

    /// The player's answer in the Prices tab for the name being learned with `token`: one of the
    /// items it could be, or none of them (`None`). An answer for a name that isn't the latest
    /// one changes nothing: the player pointed at something else meanwhile.
    pub fn teach<B: Backend>(
        &mut self,
        app: &AppHandle,
        runner: &mut Runner<B>,
        token: u64,
        choice: Option<&str>,
    ) {
        let Some((current, stuck)) = self.stuck.take() else {
            return;
        };
        if current != token {
            self.stuck = Some((current, stuck));
            return;
        }
        let Some(reader) = runner.tooltip_reader_mut() else {
            return;
        };
        match choice {
            Some(name) => {
                let _ = reader.teach_name(&stuck, name);
            }
            None => reader.dismiss(&stuck),
        }
        // Told again from the next tooltip, whatever it is.
        self.pointed = None;
        if let Some(store) = app.try_state::<Store>() {
            self.keep(&store, runner);
        }
    }

    /// How item tooltips are read, for What MesoVibe sees and the Prices tab: `learning` until
    /// letters were kept from an earlier session or a price label has shown, then `reading`;
    /// `None` while the label is off.
    pub fn state(&self) -> Option<&'static str> {
        self.on
            .then_some(if self.learned { "reading" } else { "learning" })
    }

    /// The token for a name being learned: the same while the tooltip keeps offering the same
    /// items (its pieces change with the scene behind the box), a new one for another name.
    fn stuck_token(&mut self, stuck: &Stuck) -> u64 {
        match &mut self.stuck {
            Some((token, last)) if last.candidates == stuck.candidates => {
                *last = stuck.clone();
                *token
            }
            _ => {
                self.tokens += 1;
                self.stuck = Some((self.tokens, stuck.clone()));
                self.tokens
            }
        }
    }

    /// Tells the Prices tab what the player points at, once per change.
    fn point(&mut self, app: &AppHandle, pointed: Pointed) {
        if self.pointed.as_ref() != Some(&pointed) {
            let _ = app.emit_to(dock::PANEL, "prices://pointed", pointed.clone());
            self.pointed = Some(pointed);
        }
    }

    /// Keeps what the reader learned, and the names dismissed, when they changed.
    fn keep<B: Backend>(&mut self, store: &Store, runner: &mut Runner<B>) {
        let Some(reader) = runner.tooltip_reader_mut() else {
            return;
        };
        let letters = reader.knowledge();
        if letters != self.kept
            && store
                .keep_knowledge(Learned::TooltipFont, letters.as_deref())
                .is_ok()
        {
            self.kept = letters;
        }
        let dismissed = reader.dismissed_knowledge();
        if dismissed != self.dismissed_kept
            && store
                .keep_knowledge(Learned::TooltipDismissed, dismissed.as_deref())
                .is_ok()
        {
            self.dismissed_kept = dismissed;
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
