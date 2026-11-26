//! The price label over the game: each tick, from the engine's item tooltip reading and the
//! prices downloaded (prices.rs), what the label says and where it goes (dock.rs), or that it
//! hides; and which item the player points at, for the Prices tab.
//!
//! The label shows only while the latest tick confirmed the tooltip (two whole frames in a row
//! agreeing on its box and name), and hides on the first tick that doesn't, after 600 ms with no
//! confirming tick, whenever capture isn't running, and while the game isn't in front. Nothing
//! shows while a name is being read, and a name read unsure shows nothing.
//!
//! The tooltip reader is on only while the player has "Prices next to item tooltips" on; off, no
//! tooltip is read and no whole frame is grabbed for one. What it learned of the name font is
//! kept on this computer (`tooltip-font.txt`), and the price file's names of items the game's item
//! list doesn't have (cash-shop items) are given to it, so it never reads one as a listed item.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mesovibe_engine::capture::{Backend, CaptureState};
use mesovibe_engine::recognizers::tooltip::TooltipReader;
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
    #[serde(flatten)]
    pub view: LabelView,
    /// The card's width and the room left of it in the window, in logical pixels.
    pub card_width: f64,
    pub card_left: f64,
}

/// The items the player points at, for the Prices tab (`prices://pointed`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pointed {
    pub ids: Vec<u32>,
}

/// Follows the engine's tooltip readings from tick to tick.
#[derive(Default)]
pub struct LabelFollower {
    /// When a tick last confirmed a tooltip.
    confirmed_at: Option<Instant>,
    /// The items last told to the Prices tab, so each pointing is told once.
    pointed: Option<Vec<u32>>,
    /// The price file the reader was last given the names that aren't items from.
    names_from: Option<Arc<Prices>>,
    /// The reader's letters as last kept on this computer.
    kept: Option<String>,
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
        if !settings.price_label {
            if runner.tooltip_reader_mut().is_some() {
                runner.set_tooltip_reader(None);
            }
            docker.close_label(app);
            self.confirmed_at = None;
            self.names_from = None;
            return;
        }
        if runner.tooltip_reader_mut().is_none() {
            let kept = store.knowledge(Learned::TooltipFont);
            let reader = kept
                .as_deref()
                .and_then(TooltipReader::from_knowledge)
                .unwrap_or_default();
            self.kept = reader.knowledge();
            self.names_from = None;
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
            reader.set_other_names(names.iter().map(String::as_str));
            self.names_from = Some(Arc::clone(file));
        }
        if let Some(reader) = runner.tooltip_reader_mut() {
            let now = reader.knowledge();
            if now != self.kept
                && store
                    .keep_knowledge(Learned::TooltipFont, now.as_deref())
                    .is_ok()
            {
                self.kept = now;
            }
        }

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
        let ids = tooltip.items.clone().unwrap_or_default();
        if self.pointed.as_ref() != Some(&ids) && !ids.is_empty() {
            let _ = app.emit_to(
                dock::PANEL,
                "prices://pointed",
                Pointed { ids: ids.clone() },
            );
            self.pointed = Some(ids.clone());
        }
        let view = prices::label(prices.as_deref(), settings.price_world, &ids, now_ms());
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
        match (view, place) {
            (Some(view), Some(place)) => docker.show_label(
                app,
                &place,
                &LabelShown {
                    view,
                    card_width: place.card_width,
                    card_left: place.card_left,
                },
            ),
            _ => docker.hide_label(app),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
