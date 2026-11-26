//! Free Market prices from Cozy Markets, through MesoVibe's price service on mesovibe.com
//! (web/api). The service holds the Cozy key and rebuilds one price file after each Cozy update;
//! the app downloads that file, keeps the last copy for when it's offline, and answers the Prices
//! tab's searches from it. The request carries nothing about the player or the game.
//!
//! ```text
//!   start ──5 s──► download ─┬─ new file (200) ───► kept as prices.json, the panel told
//!     ▲                      ├─ same file (304) ──► noted
//!     │                      └─ offline, or the service down ──► the old copy stays
//!     └── 30 min after each Cozy update (00:30, 06:30, 12:30, 18:30 UTC) plus up to 10 min, never
//!         within an hour of the last try; an hour after a failure while there's no copy at all
//! ```
//!
//! Each world has its own Free Market, so the file has both, and every answer here is for the
//! world the player chose (Windia, Cozy's default, or Zenith). Names come from the game's own
//! item list (engine/data), so a search finds items Cozy doesn't price, with their NPC price.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mesovibe_engine::game_data::{self, Item};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::dock;
use crate::store::{try_read_json, write_bytes};

/// The price file. The `www` host, because the bare domain answers with a redirect to it.
const PRICES_URL: &str = "https://www.mesovibe.com/api/prices";
/// The file format this build reads. A file in another format is left alone: the copy already
/// kept stays in use until an update of MesoVibe can read the new one.
const FORMAT_VERSION: u32 = 1;
/// A file this big isn't the price file (it's under 1 MB), so it isn't parsed.
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;

/// The first download, once the rest of MesoVibe has started.
const FIRST_DOWNLOAD: Duration = Duration::from_secs(5);
/// Cozy updates at these UTC hours; the service rebuilds the file 15 minutes later.
const SLOT_HOURS: [u64; 4] = [0, 6, 12, 18];
/// Downloads come this long after each hour, once the new file has been built.
const SLOT_MINUTE: u64 = 30;
/// Each player's download comes up to this much later, so players don't all ask at once.
const SPREAD: Duration = Duration::from_secs(10 * 60);
/// Never more than one download an hour, whatever the schedule says.
const MIN_GAP: Duration = Duration::from_secs(60 * 60);
/// The longest sleep before the schedule is read again. It's wall-clock time, which keeps going
/// while the computer sleeps, so a long sleep would oversleep a download that came due.
const WAKE_EVERY: Duration = Duration::from_secs(10 * 60);
/// How long one download may take.
const TIMEOUT: Duration = Duration::from_secs(30);

const FILE: &str = "prices.json";
const ETAG_FILE: &str = "prices.etag";
const RECENT_FILE: &str = "prices-recent.json";

/// Rows a search shows.
const SEARCH_ROWS: usize = 8;
/// Biggest moves this week: this many up, then this many down.
const MOVERS_EACH: usize = 5;
/// Items Recently checked keeps.
const RECENT_MAX: usize = 8;

/// A world's Free Market. Each has its own prices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum World {
    /// Cozy's default.
    #[default]
    Windia,
    Zenith,
}

// MARK: the file, as the price service builds it (web/api/_lib/prices.js)

#[derive(Deserialize)]
struct FileHead {
    version: u32,
}

#[derive(Deserialize)]
struct PriceFile {
    worlds: FileWorlds,
}

#[derive(Deserialize)]
struct FileWorlds {
    windia: FileWorld,
    zenith: FileWorld,
}

#[derive(Deserialize)]
struct FileWorld {
    /// When Cozy last updated this world's prices, as an ISO time.
    updated_at: String,
    /// The chart's days, oldest first, one for each `history` entry.
    history_days: Vec<String>,
    items: Vec<FileItem>,
    movers: FileMovers,
}

#[derive(Debug, Clone, Deserialize)]
struct FileItem {
    game_id: u32,
    name: String,
    #[serde(default)]
    gender: Option<String>,
    usual: f64,
    #[serde(default)]
    listings: Option<f64>,
    #[serde(default)]
    sales: Option<f64>,
    #[serde(default)]
    url: Option<String>,
    /// The change against the price kept 7 days ago, as a fraction; none until a week is kept.
    #[serde(default)]
    change_7d: Option<f64>,
    #[serde(default)]
    low_7d: Option<f64>,
    #[serde(default)]
    high_7d: Option<f64>,
    /// The usual price on each of `history_days`; none on a day it had none.
    #[serde(default)]
    history: Vec<Option<f64>>,
}

#[derive(Deserialize)]
struct FileMovers {
    up: Vec<FileMover>,
    down: Vec<FileMover>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileMover {
    game_id: u32,
    name: String,
    #[serde(default)]
    gender: Option<String>,
    #[serde(default)]
    price: Option<f64>,
    /// Cozy's own 7-day change, as a fraction.
    #[serde(default)]
    change: Option<f64>,
}

/// A downloaded price file, ready to look things up in.
pub struct Prices {
    windia: WorldPrices,
    zenith: WorldPrices,
}

struct WorldPrices {
    updated_at: String,
    history_days: Vec<String>,
    items: HashMap<u32, FileItem>,
    /// Priced items the game's item list doesn't have (mostly cash-shop items), by their folded
    /// Cozy name, so a search still finds them.
    unlisted: Vec<(String, u32)>,
    /// Names more than one priced item has, such as a men's and a women's version.
    shared: HashSet<String>,
    up: Vec<FileMover>,
    down: Vec<FileMover>,
}

impl From<FileWorld> for WorldPrices {
    fn from(world: FileWorld) -> WorldPrices {
        let mut seen = HashSet::new();
        let shared = world
            .items
            .iter()
            .filter(|item| !seen.insert(item.name.as_str()))
            .map(|item| item.name.clone())
            .collect();
        let unlisted = world
            .items
            .iter()
            .filter(|item| game_item(item.game_id).is_none())
            .map(|item| (fold(&item.name), item.game_id))
            .collect();
        WorldPrices {
            updated_at: world.updated_at,
            history_days: world.history_days,
            items: world
                .items
                .into_iter()
                .map(|item| (item.game_id, item))
                .collect(),
            unlisted,
            shared,
            up: world.movers.up,
            down: world.movers.down,
        }
    }
}

impl Prices {
    fn world(&self, world: World) -> &WorldPrices {
        match world {
            World::Windia => &self.windia,
            World::Zenith => &self.zenith,
        }
    }

    /// The names of priced items the game's item list doesn't have (cash-shop items, mostly), in
    /// either world: the tooltip reader tells items apart from them and never names them.
    pub fn other_names(&self) -> Vec<String> {
        let mut names: Vec<String> = [&self.windia, &self.zenith]
            .iter()
            .flat_map(|world| world.items.values())
            .filter(|item| game_item(item.game_id).is_none())
            .map(|item| item.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// Reads a price file. An error, in plain words, for one that isn't a price file this build reads.
fn parse(bytes: &[u8]) -> Result<Prices, String> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err("The price file is too big to be the price file.".into());
    }
    let head: FileHead = serde_json::from_slice(bytes)
        .map_err(|e| format!("The price file isn't one MesoVibe can read: {e}"))?;
    if head.version != FORMAT_VERSION {
        return Err(format!(
            "The price file is in format {}; this MesoVibe reads format {FORMAT_VERSION}. An update of MesoVibe reads it.",
            head.version
        ));
    }
    let file: PriceFile = serde_json::from_slice(bytes)
        .map_err(|e| format!("The price file isn't one MesoVibe can read: {e}"))?;
    Ok(Prices {
        windia: file.worlds.windia.into(),
        zenith: file.worlds.zenith.into(),
    })
}

// MARK: answers for the panel

/// One item in a list: search results, Recently checked and Biggest moves.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: u32,
    /// The game's own name for it.
    pub name: String,
    /// "Men's" or "Women's", for an item the game names the same for both.
    pub gender: Option<&'static str>,
    /// The usual Free Market price; none when Cozy has none.
    pub usual: Option<f64>,
    /// The change over the week, as a fraction.
    pub change: Option<f64>,
    /// What an NPC shop pays for one.
    pub npc: Option<u32>,
}

/// A search's rows, best first, and how many items matched in all.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Search {
    pub rows: Vec<Row>,
    pub total: usize,
}

/// Biggest moves this week.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Movers {
    pub up: Vec<Row>,
    pub down: Vec<Row>,
}

/// Everything the item detail shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub id: u32,
    pub name: String,
    pub gender: Option<&'static str>,
    pub usual: Option<f64>,
    pub change: Option<f64>,
    pub low_7d: Option<f64>,
    pub high_7d: Option<f64>,
    /// Sales and listings Cozy counted this week.
    pub sales: Option<f64>,
    pub listings: Option<f64>,
    pub npc: Option<u32>,
    /// It can't be traded, so it never has a Free Market price.
    pub untradeable: bool,
    /// The usual price on each of `history_days`, oldest first.
    pub history: Vec<Option<f64>>,
    pub history_days: Vec<String>,
    /// Cozy has a page for it ("Open on Cozy Markets").
    pub page: bool,
}

/// Cozy's word for an item's version, as the panel shows it. Anything else isn't shown.
fn gender_label(gender: Option<&str>) -> Option<&'static str> {
    match gender?.trim().to_ascii_lowercase().as_str() {
        "m" | "male" | "men" | "mens" | "men's" => Some("Men's"),
        "f" | "w" | "female" | "women" | "womens" | "women's" => Some("Women's"),
        _ => None,
    }
}

/// The game's item, by id.
fn game_item(id: u32) -> Option<&'static Item> {
    let items = game_data::items();
    items
        .binary_search_by_key(&id, |item| item.id)
        .ok()
        .map(|at| &items[at])
}

/// Lowercase, with runs of spaces made one, for matching what the player typed.
fn fold(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Every game item's folded name, in `game_data::items()` order.
fn folded_names() -> &'static [String] {
    static FOLDED: OnceLock<Vec<String>> = OnceLock::new();
    FOLDED.get_or_init(|| {
        game_data::items()
            .iter()
            .map(|item| fold(item.name))
            .collect()
    })
}

/// How well `name` fits `query`, both folded: the whole name, its start, a word's start, or
/// anywhere in it. `None` when it doesn't.
fn fit(name: &str, query: &str) -> Option<u8> {
    if name == query {
        return Some(0);
    }
    if name.starts_with(query) {
        return Some(1);
    }
    let word_start = name
        .match_indices(query)
        .any(|(at, _)| at > 0 && !name.as_bytes()[at - 1].is_ascii_alphanumeric());
    if word_start {
        Some(2)
    } else if name.contains(query) {
        Some(3)
    } else {
        None
    }
}

/// Names the game gives more than one item.
fn game_shared_names() -> &'static HashSet<&'static str> {
    static SHARED: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SHARED.get_or_init(|| {
        let mut seen = HashSet::new();
        game_data::items()
            .iter()
            .filter(|item| !seen.insert(item.name))
            .map(|item| item.name)
            .collect()
    })
}

/// The weekly change shown for an item: the service's own, or Cozy's when the item is one of the
/// week's movers and the service hasn't kept a week of prices yet.
fn change_of(world: &WorldPrices, id: u32) -> Option<f64> {
    world
        .items
        .get(&id)
        .and_then(|item| item.change_7d)
        .or_else(|| {
            world
                .up
                .iter()
                .chain(&world.down)
                .find(|mover| mover.game_id == id)
                .and_then(|mover| mover.change)
        })
}

fn row(prices: Option<&Prices>, world: World, id: u32) -> Option<Row> {
    let game = game_item(id);
    let world = prices.map(|prices| prices.world(world));
    let priced = world.and_then(|world| world.items.get(&id));
    let name = game
        .map(|item| item.name.to_string())
        .or_else(|| priced.map(|item| item.name.clone()))?;
    // "Men's" or "Women's" only tells two items apart, so it shows only where a name is shared.
    let shared = game_shared_names().contains(name.as_str())
        || world.is_some_and(|world| world.shared.contains(&name));
    Some(Row {
        id,
        gender: if shared {
            gender_label(priced.and_then(|item| item.gender.as_deref()))
        } else {
            None
        },
        name,
        usual: priced.map(|item| item.usual),
        change: world.and_then(|world| change_of(world, id)),
        npc: game.and_then(|item| item.sell),
    })
}

/// The items whose names fit `query`, best first: the closest fit, then those with a Free Market
/// price, then the shortest name. Quest items without a price are left out: they can't be sold.
/// Priced items the game's list doesn't have are found by Cozy's name for them.
pub fn search(prices: Option<&Prices>, world: World, query: &str) -> Search {
    let query = fold(query);
    if query.is_empty() {
        return Search {
            rows: Vec::new(),
            total: 0,
        };
    }
    let priced = |id: u32| prices.is_some_and(|prices| prices.world(world).items.contains_key(&id));
    let mut found: Vec<(u8, bool, usize, u32)> = game_data::items()
        .iter()
        .zip(folded_names())
        .filter_map(|(item, name)| {
            let fit = fit(name, &query)?;
            let has_price = priced(item.id);
            (has_price || !item.quest).then_some((fit, !has_price, name.len(), item.id))
        })
        .collect();
    if let Some(prices) = prices {
        found.extend(
            prices
                .world(world)
                .unlisted
                .iter()
                .filter_map(|(name, id)| Some((fit(name, &query)?, false, name.len(), *id))),
        );
    }
    found.sort_unstable();
    Search {
        total: found.len(),
        rows: found
            .iter()
            .take(SEARCH_ROWS)
            .filter_map(|&(.., id)| row(prices, world, id))
            .collect(),
    }
}

/// Rows for these items, in this order, leaving out any neither the game nor the prices have.
pub fn rows(prices: Option<&Prices>, world: World, ids: &[u32]) -> Vec<Row> {
    ids.iter()
        .filter_map(|&id| row(prices, world, id))
        .collect()
}

/// Cozy's biggest moves this week, the game's items only.
pub fn movers(prices: Option<&Prices>, world: World) -> Movers {
    let Some(file) = prices else {
        return Movers {
            up: Vec::new(),
            down: Vec::new(),
        };
    };
    let list = |movers: &[FileMover]| -> Vec<Row> {
        movers
            .iter()
            .filter_map(|mover| {
                let mut row = row(prices, world, mover.game_id).or_else(|| {
                    Some(Row {
                        id: mover.game_id,
                        name: mover.name.clone(),
                        gender: None,
                        usual: None,
                        change: None,
                        npc: None,
                    })
                })?;
                row.usual = row.usual.or(mover.price);
                row.change = mover.change.or(row.change);
                row.gender = row.gender.or(gender_label(mover.gender.as_deref()));
                Some(row)
            })
            .take(MOVERS_EACH)
            .collect()
    };
    let world = file.world(world);
    Movers {
        up: list(&world.up),
        down: list(&world.down),
    }
}

/// One item's detail, or `None` when neither the game nor Cozy has it.
pub fn detail(prices: Option<&Prices>, world: World, id: u32) -> Option<Detail> {
    let base = row(prices, world, id)?;
    let world = prices.map(|prices| prices.world(world));
    let priced = world.and_then(|world| world.items.get(&id));
    Some(Detail {
        id,
        name: base.name,
        gender: base.gender,
        usual: base.usual,
        change: base.change,
        low_7d: priced.and_then(|item| item.low_7d),
        high_7d: priced.and_then(|item| item.high_7d),
        sales: priced.and_then(|item| item.sales),
        listings: priced.and_then(|item| item.listings),
        npc: base.npc,
        untradeable: game_item(id).is_some_and(|item| item.untradeable),
        history: priced.map(|item| item.history.clone()).unwrap_or_default(),
        history_days: world
            .map(|world| world.history_days.clone())
            .unwrap_or_default(),
        page: priced
            .and_then(|item| item.url.as_deref())
            .is_some_and(cozy_page),
    })
}

/// An address on Cozy Markets, made only of characters a link needs. The panel can open only
/// these, so a damaged or altered price file can't send the browser anywhere else.
fn cozy_page(url: &str) -> bool {
    url.starts_with("https://cozy.markets/")
        && url.len() <= 512
        && url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~/%?=&+:#".contains(&b))
}

/// The page to open for "Open on Cozy Markets": the item's own, or Cozy's front page for the
/// credit line (`id` none).
pub fn page(prices: Option<&Prices>, world: World, id: Option<u32>) -> Option<String> {
    let Some(id) = id else {
        return Some("https://cozy.markets/".to_string());
    };
    let url = prices?.world(world).items.get(&id)?.url.as_deref()?;
    cozy_page(url).then(|| url.to_string())
}

// MARK: the label over the game

/// Prices this old turn the label's figures muted, with their date; older than `TOO_OLD_MS`
/// there's no label at all.
const STALE_MS: u64 = 12 * 60 * 60 * 1000;
const TOO_OLD_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// What the price label says for the item the player points at.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelView {
    /// The usual Free Market price, or none: then `npc` stands in, with "No Free Market price".
    pub usual: Option<f64>,
    pub change: Option<f64>,
    pub npc: Option<u32>,
    /// The world's name, for the credit row ("Windia · Cozy Markets").
    pub world: &'static str,
    /// When the prices were updated, in epoch milliseconds, once they're old: the figures turn
    /// muted and the credit row says "from Nov 23".
    pub old_since_ms: Option<u64>,
}

/// The label for the items a tooltip names (one, or a few the game names alike), from prices
/// downloaded at least `now_ms - TOO_OLD_MS`: `None` when there's nothing honest to show (no
/// price and no NPC price, prices too old or never downloaded, or several of the items priced,
/// which would be a guess).
pub fn label(prices: Option<&Prices>, world: World, ids: &[u32], now_ms: u64) -> Option<LabelView> {
    let file = prices?;
    let updated = utc_ms(&file.world(world).updated_at)?;
    let age = now_ms.saturating_sub(updated);
    if age > TOO_OLD_MS {
        return None;
    }
    let rows: Vec<Row> = ids
        .iter()
        .filter_map(|&id| row(prices, world, id))
        .collect();
    let priced: Vec<&Row> = rows.iter().filter(|r| r.usual.is_some()).collect();
    let (usual, change, npc) = match priced.as_slice() {
        [one] => (one.usual, one.change, one.npc),
        [] => {
            // No Free Market price: what an NPC shop pays, when every item named so pays the same.
            let npc = rows.first()?.npc?;
            if rows.iter().any(|r| r.npc != Some(npc)) {
                return None;
            }
            (None, None, Some(npc))
        }
        _ => return None,
    };
    Some(LabelView {
        usual,
        change,
        npc,
        world: match world {
            World::Windia => "Windia",
            World::Zenith => "Zenith",
        },
        old_since_ms: (age >= STALE_MS).then_some(updated),
    })
}

/// Epoch milliseconds for a UTC time written `2026-11-25T18:08:59.000Z` (the price file's form).
fn utc_ms(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let millis = match b[19] {
        b'.' => {
            let end = text[20..]
                .find(|c: char| !c.is_ascii_digit())
                .map_or(text.len(), |i| 20 + i);
            let frac = &text[20..end];
            let ms: i64 = format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()?;
            if &text[end..] != "Z" {
                return None;
            }
            ms
        }
        b'Z' if b.len() == 20 => 0,
        _ => return None,
    };
    // Days since 1970-01-01, by the civil-from-days algorithm run backwards.
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(((days * 24 + hh) * 60 + mm) * 60_000 + ss * 1000 + millis).ok()
}

// MARK: downloading

/// Where the prices stand, for the panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceStatus {
    /// A download is under way.
    pub downloading: bool,
    /// The last download failed: offline, or the price service down.
    pub failed: bool,
    /// When Cozy last updated each world's prices in the copy kept, as ISO times; none before
    /// the first download.
    pub updated: Option<Updated>,
    /// When the price service last answered, in ms since the epoch.
    pub checked_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Updated {
    pub windia: String,
    pub zenith: String,
}

fn updated(prices: &Prices) -> Updated {
    Updated {
        windia: prices.windia.updated_at.clone(),
        zenith: prices.zenith.updated_at.clone(),
    }
}

struct Inner {
    prices: Option<Arc<Prices>>,
    /// The ETag of the copy kept, sent so an unchanged file comes back as a short "not modified".
    etag: Option<String>,
    status: PriceStatus,
    /// Recently checked, newest first.
    recent: Vec<u32>,
}

/// The prices kept on this computer, and the thread that keeps them current.
pub struct PriceBook {
    dir: PathBuf,
    inner: Arc<Mutex<Inner>>,
}

impl PriceBook {
    /// Loads the copy kept in `dir`, if any, and starts the downloads.
    pub fn start(app: &AppHandle, dir: PathBuf) -> PriceBook {
        let book = PriceBook::open(dir);
        let (app, dir, inner) = (app.clone(), book.dir.clone(), Arc::clone(&book.inner));
        let _ = std::thread::Builder::new()
            .name("mesovibe-prices".to_string())
            .spawn(move || run(&app, &dir, &inner));
        book
    }

    /// The copy kept in `dir`, without downloading.
    fn open(dir: PathBuf) -> PriceBook {
        let (prices, etag) = load(&dir);
        let recent = try_read_json::<Vec<u32>>(&dir.join(RECENT_FILE))
            .ok()
            .flatten()
            .unwrap_or_default();
        let status = PriceStatus {
            downloading: false,
            failed: false,
            updated: prices.as_ref().map(updated),
            checked_at_ms: None,
        };
        PriceBook {
            dir,
            inner: Arc::new(Mutex::new(Inner {
                prices: prices.map(Arc::new),
                etag,
                status,
                recent: recent.into_iter().take(RECENT_MAX).collect(),
            })),
        }
    }

    pub fn status(&self) -> PriceStatus {
        lock(&self.inner).status.clone()
    }

    /// The prices kept, for one lookup. The lock is let go before the lookup runs, so a download
    /// finishing meanwhile never waits on a search.
    pub fn prices(&self) -> Option<Arc<Prices>> {
        lock(&self.inner).prices.clone()
    }

    pub fn recent(&self) -> Vec<u32> {
        lock(&self.inner).recent.clone()
    }

    /// Replaces Recently checked (the panel adds to it, clears it and undoes a clear).
    pub fn set_recent(&self, ids: Vec<u32>) -> Result<(), String> {
        let mut ids = ids;
        ids.truncate(RECENT_MAX);
        let text = serde_json::to_vec(&ids).map_err(|e| e.to_string())?;
        let mut inner = lock(&self.inner);
        write_bytes(&self.dir.join(RECENT_FILE), &text)?;
        inner.recent = ids;
        Ok(())
    }
}

/// The copy kept and its ETag. A copy that can't be read is set aside, and its ETag dropped, so
/// the next download fetches the whole file again.
fn load(dir: &Path) -> (Option<Prices>, Option<String>) {
    let path = dir.join(FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return (None, None),
    };
    match parse(&bytes) {
        Ok(prices) => {
            let etag = std::fs::read_to_string(dir.join(ETAG_FILE))
                .ok()
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty());
            (Some(prices), etag)
        }
        Err(_) => {
            let _ = std::fs::rename(&path, dir.join(format!("{FILE}.corrupt")));
            let _ = std::fs::remove_file(dir.join(ETAG_FILE));
            (None, None)
        }
    }
}

/// The download thread: downloads when due, forever.
fn run(app: &AppHandle, dir: &Path, inner: &Mutex<Inner>) {
    let client = match client(&app.package_info().version.to_string()) {
        Ok(client) => client,
        // Nothing can download: say so, rather than "Getting prices…" for good.
        Err(_) => {
            let mut inner = lock(inner);
            inner.status.failed = true;
            tell(app, &inner.status);
            return;
        }
    };
    let mut due = now_ms().saturating_add(FIRST_DOWNLOAD.as_millis() as u64);
    loop {
        let now = now_ms();
        if now < due {
            std::thread::sleep(Duration::from_millis(due - now).min(WAKE_EVERY));
            continue;
        }
        let tried = now_ms();
        let ok = download(app, dir, inner, &client);
        let has_copy = lock(inner).prices.is_some();
        due = next_download(tried, ok, has_copy, spread_ms());
    }
}

/// The HTTP client: the same TLS as MesoVibe's update check, and a user agent naming only
/// MesoVibe and its version.
fn client(version: &str) -> Result<reqwest::Client, String> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    reqwest::Client::builder()
        .user_agent(format!("MesoVibe/{version}"))
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| e.to_string())
}

/// One download. Whether the price service answered.
fn download(app: &AppHandle, dir: &Path, inner: &Mutex<Inner>, client: &reqwest::Client) -> bool {
    let etag = {
        let mut inner = lock(inner);
        inner.status.downloading = true;
        tell(app, &inner.status);
        inner.etag.clone()
    };
    let fetched = tauri::async_runtime::block_on(fetch(client, etag.as_deref()));
    let answered = fetched.and_then(|(status, etag, body)| answer(status, etag, body));
    let mut inner = lock(inner);
    inner.status.downloading = false;
    let ok = match answered {
        Ok(Answer::New {
            prices,
            bytes,
            etag,
        }) => {
            // Kept on disk for the next start; if that fails, this run still uses it.
            if write_bytes(&dir.join(FILE), &bytes).is_ok() {
                let _ = match &etag {
                    Some(tag) => write_bytes(&dir.join(ETAG_FILE), tag.as_bytes()),
                    None => std::fs::remove_file(dir.join(ETAG_FILE)).map_err(|e| e.to_string()),
                };
            }
            inner.status.updated = Some(updated(&prices));
            inner.prices = Some(Arc::from(prices));
            inner.etag = etag;
            true
        }
        Ok(Answer::Same) => true,
        Err(_) => false,
    };
    inner.status.failed = !ok;
    if ok {
        inner.status.checked_at_ms = Some(now_ms());
    }
    tell(app, &inner.status);
    ok
}

async fn fetch(
    client: &reqwest::Client,
    etag: Option<&str>,
) -> Result<(u16, Option<String>, Vec<u8>), String> {
    let mut request = client.get(PRICES_URL);
    if let Some(tag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, tag);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|tag| tag.to_str().ok())
        .map(str::to_string);
    if response
        .content_length()
        .is_some_and(|n| n > MAX_FILE_BYTES as u64)
    {
        return Err("The price file is too big to be the price file.".into());
    }
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    Ok((status, etag, body.to_vec()))
}

/// What the price service's answer means.
enum Answer {
    /// A new file, read and ready.
    New {
        prices: Box<Prices>,
        bytes: Vec<u8>,
        etag: Option<String>,
    },
    /// The file hasn't changed since the copy kept.
    Same,
}

fn answer(status: u16, etag: Option<String>, body: Vec<u8>) -> Result<Answer, String> {
    match status {
        200 => {
            let prices = Box::new(parse(&body)?);
            Ok(Answer::New {
                prices,
                bytes: body,
                etag,
            })
        }
        304 => Ok(Answer::Same),
        503 => Err("The price service has no prices right now.".into()),
        other => Err(format!("The price service answered {other}.")),
    }
}

/// When the next download is due (ms since the epoch), after one tried at `tried_ms`: the next
/// slot after it, but never within an hour of it. While there's no copy at all, a failure is
/// tried again an hour later, so a first start while offline doesn't wait hours for prices once
/// the computer is back online.
fn next_download(tried_ms: u64, ok: bool, has_copy: bool, spread_ms: u64) -> u64 {
    let gap = tried_ms.saturating_add(MIN_GAP.as_millis() as u64);
    if !ok && !has_copy {
        return gap;
    }
    const HOUR: u64 = 60 * 60 * 1000;
    const DAY: u64 = 24 * HOUR;
    let day = tried_ms / DAY * DAY;
    let slot = (0..=1)
        .flat_map(|d| SLOT_HOURS.map(|h| day + d * DAY + h * HOUR + SLOT_MINUTE * 60 * 1000))
        .map(|at| at + spread_ms)
        .find(|&at| at > tried_ms)
        .unwrap_or(gap);
    slot.max(gap)
}

/// A fresh spread for the next slot, from 0 up to [`SPREAD`]. Not secret, so the standard
/// library's randomly keyed hasher is random enough.
fn spread_ms() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(now_ms());
    hasher.finish() % (SPREAD.as_millis() as u64 + 1)
}

/// Tells the panel. Only the panel listens: never every window.
fn tell(app: &AppHandle, status: &PriceStatus) {
    let _ = app.emit_to(dock::PANEL, "prices://status", status.clone());
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

/// Opens an address in the player's browser.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide = |text: &str| {
            text.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        let (verb, file) = (wide("open"), wide(url));
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; the owner window,
        // parameters and directory may be null.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // Above 32 means it opened (ShellExecuteW's documented convention).
        if result as isize > 32 {
            Ok(())
        } else {
            Err("Windows couldn't open the page in your browser.".to_string())
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const HOUR: u64 = 60 * 60 * 1000;
    const MINUTE: u64 = 60 * 1000;
    /// 2026-11-25T00:00:00Z.
    const DAY_START: u64 = 1_795_564_800_000;

    /// Real game items: Ilbi Throwing Stars and Work Gloves are priced; Red Potion has only an NPC
    /// price here. The numbers are made up.
    pub(crate) fn sample_file() -> String {
        let item = |id: u32, name: &str, usual: f64, change: Option<f64>| {
            serde_json::json!({
                "game_id": id, "item_id": id, "name": name, "gender": null, "usual": usual,
                "basis": "listed", "window": "week", "listings": 40, "sales": 12,
                "url": format!("https://cozy.markets/items/{id}"),
                "change_7d": change, "low_7d": usual * 0.9, "high_7d": usual,
                "history": [usual * 0.8, null, usual],
            })
        };
        let gendered = |mut item: serde_json::Value, gender: &str| {
            item["gender"] = gender.into();
            item
        };
        let world = |scale: f64| {
            serde_json::json!({
                "updated_at": "2026-11-25T18:08:59.000Z",
                "next_update_at": "2026-11-26T00:00:00.000Z",
                "history_days": ["2026-11-23", "2026-11-24", "2026-11-25"],
                "items": [
                    item(2070006, "Ilbi Throwing Stars", 21_000_000.0 * scale, Some(0.08)),
                    item(1082000, "Work Gloves", 1_500_000.0 * scale, None),
                    gendered(item(1060008, "Red Cloth Pants", 1_000.0 * scale, None), "men"),
                    gendered(item(1061009, "Red Cloth Pants", 1_750.0 * scale, None), "women"),
                    gendered(item(1050000, "Beige Plain Robe", 2_500.0 * scale, None), "men"),
                    item(9_999_999, "Sparkly Cash Hat", 300_000.0 * scale, None),
                ],
                "movers": {
                    "up": [{ "game_id": 1082000, "item_id": 1, "name": "Work Gloves", "gender": null, "price": 1_500_000.0 * scale, "change": 0.25, "url": "u" }],
                    "down": [{ "game_id": 2070006, "item_id": 2, "name": "Ilbi Throwing Stars", "gender": null, "price": 21_000_000.0 * scale, "change": -0.03, "url": "u" }],
                },
            })
        };
        serde_json::json!({
            "version": 1,
            "source": "Cozy Markets",
            "built_at": "2026-11-25T18:15:00.000Z",
            "worlds": { "windia": world(1.0), "zenith": world(0.5) },
        })
        .to_string()
    }

    fn sample() -> Prices {
        parse(sample_file().as_bytes()).expect("the sample parses")
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "mesovibe-prices-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    // Downloads land 30 minutes after each Cozy update plus the spread, never within an hour of
    // the last. Fails if a slot is skipped, two downloads come within an hour, or the day wraps
    // wrong.
    #[test]
    fn downloads_follow_cozys_updates_and_never_come_within_an_hour() {
        let at = |h: u64, m: u64| DAY_START + h * HOUR + m * MINUTE;
        // At start, 05:00: the 06:30 slot.
        assert_eq!(next_download(at(5, 0), true, true, 0), at(6, 30));
        assert_eq!(next_download(at(5, 0), true, true, 7 * MINUTE), at(6, 37));
        // Started at 06:05: 06:30 is too soon, so an hour after, not 12:30.
        assert_eq!(next_download(at(6, 5), true, true, 0), at(7, 5));
        // Just after a slot's download: the next slot.
        assert_eq!(next_download(at(6, 34), true, true, 0), at(12, 30));
        // Late in the day: tomorrow's first slot.
        assert_eq!(
            next_download(at(18, 40), true, true, 0),
            DAY_START + 24 * HOUR + 30 * MINUTE
        );
        // Offline with a copy kept: the next slot, like any other.
        assert_eq!(next_download(at(6, 34), false, true, 0), at(12, 30));
    }

    // A first start while offline keeps trying hourly, so prices arrive soon after the computer is
    // back online. Fails if it waits for the next slot, or tries more than once an hour.
    #[test]
    fn with_no_copy_a_failed_download_is_tried_again_an_hour_later() {
        let tried = DAY_START + 6 * HOUR + 40 * MINUTE;
        assert_eq!(next_download(tried, false, false, 0), tried + HOUR);
    }

    // The service's file reads into both worlds. Fails if a world is mixed up with the other, a
    // field is lost, or the change isn't the service's.
    #[test]
    fn a_price_file_reads_into_both_worlds() {
        let prices = sample();
        let ilbi = detail(Some(&prices), World::Windia, 2070006).expect("Ilbi");
        assert_eq!(ilbi.usual, Some(21_000_000.0));
        assert_eq!(ilbi.change, Some(0.08));
        assert_eq!(
            ilbi.history,
            vec![Some(16_800_000.0), None, Some(21_000_000.0)]
        );
        assert_eq!(ilbi.history_days.len(), 3);
        assert_eq!((ilbi.sales, ilbi.listings), (Some(12.0), Some(40.0)));
        assert!(ilbi.page);
        let zenith = detail(Some(&prices), World::Zenith, 2070006).expect("Ilbi on Zenith");
        assert_eq!(zenith.usual, Some(10_500_000.0));
    }

    // A file in another format, or one that isn't JSON, is refused with words to show, so the
    // copy kept stays in use. Fails if a newer format is read as this one.
    #[test]
    fn a_file_in_another_format_is_refused() {
        let newer = sample_file().replacen("\"version\":1", "\"version\":2", 1);
        let words = parse(newer.as_bytes()).err().expect("refused");
        assert!(words.contains("format 2"), "{words}");
        assert!(parse(b"<html>").is_err());
        assert!(parse(b"{\"version\":1}").is_err(), "no worlds");
    }

    // The service's answers: a new file, "not modified", and failures. Fails if a 304 drops the
    // copy, a 503 reads as success, or a broken 200 is kept.
    #[test]
    fn the_services_answers_mean_new_same_or_failed() {
        let new = answer(200, Some("\"abc\"".into()), sample_file().into_bytes());
        assert!(matches!(new, Ok(Answer::New { etag: Some(ref tag), .. }) if tag == "\"abc\""));
        assert!(matches!(answer(304, None, Vec::new()), Ok(Answer::Same)));
        assert!(answer(503, None, Vec::new()).is_err());
        assert!(answer(404, None, Vec::new()).is_err());
        assert!(answer(200, None, b"{}".to_vec()).is_err());
    }

    // The copy kept comes back after a restart, with its ETag; a damaged one is set aside so the
    // next download fetches the whole file. Fails if a damaged copy blocks prices for good or its
    // ETag is still sent.
    #[test]
    fn the_copy_kept_survives_a_restart_and_a_damaged_one_is_set_aside() {
        let dir = temp_dir("load");
        std::fs::write(dir.join(FILE), sample_file()).unwrap();
        std::fs::write(dir.join(ETAG_FILE), "\"abc\"\n").unwrap();
        let book = PriceBook::open(dir.clone());
        assert!(book.prices().is_some());
        assert_eq!(lock(&book.inner).etag.as_deref(), Some("\"abc\""));
        assert_eq!(
            book.status().updated.map(|u| u.windia),
            Some("2026-11-25T18:08:59.000Z".to_string())
        );

        std::fs::write(dir.join(FILE), "{\"version\":1,\"worl").unwrap();
        let damaged = PriceBook::open(dir.clone());
        assert!(damaged.prices().is_none());
        assert_eq!(lock(&damaged.inner).etag, None, "the ETag goes with it");
        assert!(dir.join(format!("{FILE}.corrupt")).exists());
        assert!(!dir.join(ETAG_FILE).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    // Search finds the game's items by name, closest first, priced before unpriced, with NPC
    // prices for items Cozy doesn't list. Fails if a word in the middle isn't found, the order
    // drifts, or a quest item without a price shows up.
    #[test]
    fn search_finds_game_items_closest_first() {
        let prices = sample();
        let found = search(Some(&prices), World::Windia, "  WORK   gloves ");
        assert_eq!(found.rows[0].name, "Work Gloves");
        assert_eq!(found.rows[0].usual, Some(1_500_000.0));
        let stars = search(Some(&prices), World::Windia, "throwing");
        assert!(stars.total > 1, "every kind of throwing star");
        assert_eq!(
            stars.rows[0].name, "Ilbi Throwing Stars",
            "the priced one first"
        );
        let potion = search(Some(&prices), World::Windia, "red potion");
        assert_eq!(potion.rows[0].name, "Red Potion");
        assert_eq!(potion.rows[0].usual, None, "Cozy doesn't price it here");
        assert!(potion.rows[0].npc.is_some(), "an NPC price instead");
        assert!(search(Some(&prices), World::Windia, "  ").rows.is_empty());
        let no_prices = search(None, World::Windia, "work gloves");
        assert_eq!(
            no_prices.rows[0].usual, None,
            "still found before any download"
        );
        for row in &search(Some(&prices), World::Windia, "e").rows {
            let item = game_item(row.id).unwrap();
            assert!(
                !item.quest || row.usual.is_some(),
                "{} is a quest item",
                row.name
            );
        }
        assert!(search(Some(&prices), World::Windia, "e").rows.len() <= SEARCH_ROWS);
    }

    // Biggest moves use Cozy's weekly change, and an item that's a mover shows that change in
    // its row while the service hasn't kept a week yet. Fails if the movers lose their change or
    // the two lists disagree.
    #[test]
    fn movers_carry_cozys_weekly_change() {
        let prices = sample();
        let moves = movers(Some(&prices), World::Windia);
        assert_eq!(moves.up[0].name, "Work Gloves");
        assert_eq!(moves.up[0].change, Some(0.25));
        assert_eq!(moves.down[0].change, Some(-0.03));
        let gloves = rows(Some(&prices), World::Windia, &[1082000]);
        assert_eq!(
            gloves[0].change,
            Some(0.25),
            "no week kept yet: Cozy's change"
        );
        assert!(movers(None, World::Windia).up.is_empty());
    }

    // Only Cozy pages open from the panel. Fails if a changed price file could open any other
    // address.
    #[test]
    fn only_cozy_pages_open() {
        assert!(cozy_page("https://cozy.markets/items/2070006?world=zenith"));
        assert!(!cozy_page("https://cozy.markets.evil.example/x"));
        assert!(!cozy_page("http://cozy.markets/items/1"));
        assert!(!cozy_page("https://cozy.markets/a b"));
        assert!(!cozy_page("https://cozy.markets/\"onclick"));
        let prices = sample();
        assert_eq!(
            page(Some(&prices), World::Windia, Some(2070006)).as_deref(),
            Some("https://cozy.markets/items/2070006")
        );
        assert_eq!(page(Some(&prices), World::Windia, Some(2000000)), None);
        assert_eq!(
            page(None, World::Windia, None).as_deref(),
            Some("https://cozy.markets/")
        );
    }

    // Recently checked comes back after a restart, holding at most eight. Fails if it's lost or
    // grows past eight.
    #[test]
    fn recently_checked_survives_a_restart() {
        let dir = temp_dir("recent");
        let book = PriceBook::open(dir.clone());
        assert!(book.recent().is_empty());
        book.set_recent((1..=10).collect()).unwrap();
        assert_eq!(
            PriceBook::open(dir.clone()).recent(),
            (1..=8).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // An item the game's list doesn't have (a cash-shop item, say) is still found by Cozy's name.
    // Fails if search covers only the game's list.
    #[test]
    fn search_finds_priced_items_the_game_list_lacks() {
        let prices = sample();
        let found = search(Some(&prices), World::Windia, "sparkly");
        assert_eq!(found.total, 1);
        assert_eq!(found.rows[0].name, "Sparkly Cash Hat");
        assert_eq!(
            (found.rows[0].usual, found.rows[0].npc),
            (Some(300_000.0), None)
        );
        assert!(search(None, World::Windia, "sparkly").rows.is_empty());
    }

    // "Men's" and "Women's" tell apart items that share a name, and stay off items that don't.
    // Fails if every gendered item is labelled, or a pair isn't.
    #[test]
    fn gender_shows_only_where_a_name_is_shared() {
        let prices = sample();
        let pants = search(Some(&prices), World::Windia, "red cloth pants");
        let labels: Vec<_> = pants.rows.iter().map(|row| row.gender).collect();
        assert_eq!(labels, vec![Some("Men's"), Some("Women's")]);
        let robe = search(Some(&prices), World::Windia, "beige plain robe");
        assert_eq!(robe.rows[0].gender, None);
    }

    #[test]
    fn genders_read_in_cozys_words() {
        assert_eq!(gender_label(Some("male")), Some("Men's"));
        assert_eq!(gender_label(Some("F")), Some("Women's"));
        assert_eq!(gender_label(Some("both")), None);
        assert_eq!(gender_label(None), None);
    }

    // The label shows the usual price and the week's move with the world, and dates old prices;
    // with no market price it shows what an NPC pays; too-old prices, no prices, or several
    // priced items named alike show nothing. Fails if the label could show a guess or a price too
    // old to trust.
    #[test]
    fn the_label_says_the_price_or_honestly_nothing() {
        let file = sample();
        let updated = utc_ms("2026-11-25T18:08:59.000Z").unwrap();
        let fresh = label(Some(&file), World::Windia, &[2070006], updated + HOUR).unwrap();
        assert_eq!(
            (fresh.usual, fresh.change, fresh.world, fresh.old_since_ms),
            (Some(21_000_000.0), Some(0.08), "Windia", None)
        );
        let zenith = label(Some(&file), World::Zenith, &[2070006], updated + HOUR).unwrap();
        assert_eq!((zenith.usual, zenith.world), (Some(10_500_000.0), "Zenith"));
        let old = label(Some(&file), World::Windia, &[2070006], updated + 13 * HOUR).unwrap();
        assert_eq!(old.old_since_ms, Some(updated));
        assert_eq!(
            label(
                Some(&file),
                World::Windia,
                &[2070006],
                updated + 8 * 24 * HOUR
            ),
            None,
            "a week old"
        );
        assert_eq!(
            label(None, World::Windia, &[2070006], updated),
            None,
            "never downloaded"
        );
        // Red Potion: no Free Market price, an NPC price.
        let npc = label(Some(&file), World::Windia, &[2000000], updated).unwrap();
        assert_eq!(
            (npc.usual, npc.npc),
            (None, game_item(2000000).and_then(|i| i.sell))
        );
        assert!(npc.npc.is_some());
        // Two items named alike, both priced: which one is a guess.
        assert_eq!(
            label(Some(&file), World::Windia, &[1060008, 1061009], updated),
            None
        );
        // Of two named alike, one priced: that one.
        let one = label(Some(&file), World::Windia, &[1082000, 2000000], updated).unwrap();
        assert_eq!(one.usual, Some(1_500_000.0));
    }

    // The price file's times read as epoch milliseconds, and anything else doesn't. Fails if
    // the label's age is worked out wrong.
    #[test]
    fn price_file_times_read_as_epoch_milliseconds() {
        assert_eq!(utc_ms("2026-11-25T00:00:00.000Z"), Some(DAY_START));
        assert_eq!(utc_ms("2026-11-25T00:00:00Z"), Some(DAY_START));
        assert_eq!(
            utc_ms("2026-11-25T18:08:59.5Z"),
            Some(DAY_START + 18 * HOUR + 8 * MINUTE + 59_500)
        );
        assert_eq!(utc_ms("1970-01-01T00:00:00.000Z"), Some(0));
        for bad in [
            "",
            "2026-11-25",
            "2026-13-25T00:00:00Z",
            "2026-11-25T00:00:00+01:00",
            "yesterday",
        ] {
            assert_eq!(utc_ms(bad), None, "{bad}");
        }
    }

    // The cash-shop items the price file prices but the game's item list doesn't have, for the
    // tooltip reader to tell apart. Fails if a listed item or a duplicate gets in.
    #[test]
    fn other_names_are_the_priced_items_the_item_list_lacks() {
        assert_eq!(sample().other_names(), vec!["Sparkly Cash Hat".to_string()]);
    }
}
