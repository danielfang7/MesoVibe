// MesoVibe's price service: Free Market prices from Cozy Markets, rebuilt every 6 hours and served
// to the app as one file. The Cozy key stays on this server (Cozy asks that it never ship in an app
// people download), and players' requests never reach Cozy: they get the file this job built.
//
// Everything here takes its storage and fetch as arguments, so the tests run it without Vercel.
// api/refresh.js and api/prices.js pass in the real ones.
import { createHash, timingSafeEqual } from "node:crypto";

export const COZY_API = "https://cozy.markets/trade-api/dev/v1";
/** Each world has its own Free Market. Cozy answers for Windia when no world is named. */
export const WORLDS = ["windia", "zenith"];
/** The file's own format. The app ignores a file whose version it doesn't know. */
export const FORMAT_VERSION = 1;
/** Every day's prices, kept to work out each item's weekly change and its 30-day chart. */
export const STATE_PATH = "prices/state.json";
/** The file /api/prices serves. */
export const FILE_PATH = "prices/prices.json";
/** When the job runs: 15 minutes after each Cozy update (00, 06, 12 and 18 UTC). vercel.json repeats it. */
export const REFRESH_SCHEDULE = "15 0,6,12,18 * * *";
const REFRESH_HOURS = [0, 6, 12, 18];
const REFRESH_MINUTE = 15;
/** How long a run may take before the CDN asks for the new file. */
const BUILD_MARGIN_MINUTES = 5;
/** Days on each item's chart, today included. */
export const HISTORY_DAYS = 30;
/** Days of prices kept: the chart's 30, and the day before them. */
const KEEP_DAYS = HISTORY_DAYS + 1;
/**
 * A list that grows or shrinks by more than this since the last good run is held, not used: a
 * broken answer often comes back short. If the next run's list, from a later Cozy update, is within
 * HELD_MATCH of the held size, the change lasted and is accepted, so a real change in Cozy's
 * catalog never stops the job.
 */
const MAX_COUNT_CHANGE = 0.2;
const HELD_MATCH = 0.05;
/**
 * Cozy lists a few items at a usual price of 0 (arrows, which sell for under a meso each). Those
 * have no market price here, so the app shows their NPC price instead. More than this share at 0
 * means the answer is broken.
 */
const MAX_ZERO_SHARE = 0.05;
const COZY_TIMEOUT_MS = 20_000;
/** How long the CDN may keep serving the last copy while this function fails. */
const STALE_IF_ERROR_SECONDS = 24 * 60 * 60;

// MARK: answering the app

/**
 * GET /api/prices. Only the plain address is served, so every player shares one cached copy: a query
 * string or an Authorization header (which the CDN never caches) can't make it miss and run this
 * again. The copy is cached until shortly after the next build, and kept for a day if this fails.
 */
export async function serve({ request, store, now, log = console }) {
  if (request.method !== "GET" && request.method !== "HEAD") {
    return text(405, "Use GET.", { Allow: "GET, HEAD" });
  }
  if (new URL(request.url).search !== "" || request.headers.has("authorization")) {
    return text(400, "This address takes no query string and no Authorization header.", { "Cache-Control": SHORT_CACHE });
  }
  let file;
  try {
    file = await store.read(FILE_PATH);
  } catch (error) {
    log.error(`prices: the price file couldn't be read (${error.name})`);
    return json(503, { error: "Prices can't be read right now." }, { "Cache-Control": SHORT_CACHE, "Retry-After": "300" });
  }
  if (file === null) {
    return json(503, { error: "Prices aren't ready yet." }, { "Cache-Control": SHORT_CACHE, "Retry-After": "600" });
  }
  const etag = `"${createHash("sha256").update(file).digest("base64url").slice(0, 27)}"`;
  const headers = {
    "Content-Type": "application/json; charset=utf-8",
    "Cache-Control": `public, max-age=0, s-maxage=${secondsUntilNextBuild(now)}, stale-if-error=${STALE_IF_ERROR_SECONDS}`,
    ETag: etag,
  };
  // Weak comparison, as If-None-Match asks: Vercel's CDN marks the tag weak (W/) when it compresses
  // the file, and the app sends back what it was given.
  const known = (request.headers.get("if-none-match") ?? "").split(",").map((tag) => tag.trim().replace(/^W\//, ""));
  if (known.includes(etag) || known.includes("*")) return new Response(null, { status: 304, headers });
  return new Response(request.method === "HEAD" ? null : file, { status: 200, headers });
}

const SHORT_CACHE = "public, max-age=0, s-maxage=60";

/**
 * Seconds from `now` until a few minutes after the next scheduled run, at least one minute. A run
 * stays "next" until its margin has passed, so a copy fetched while it may still be writing the new
 * file lasts only to the end of that margin, not to the following run.
 */
export function secondsUntilNextBuild(now) {
  const runs = [];
  for (const dayOffset of [0, 1]) {
    for (const hour of REFRESH_HOURS) {
      const run = new Date(now);
      run.setUTCDate(run.getUTCDate() + dayOffset);
      run.setUTCHours(hour, REFRESH_MINUTE, 0, 0);
      runs.push(run.getTime());
    }
  }
  const margin = BUILD_MARGIN_MINUTES * 60_000;
  const next = runs.find((at) => at + margin > now.getTime());
  const until = next + margin - now.getTime();
  return Math.max(60, Math.round(until / 1000));
}

// MARK: the job

/**
 * GET /api/refresh, run by Vercel's scheduler, which signs it with CRON_SECRET. Fetches both
 * worlds' usual prices and trends, checks every response, and only when all four are good records
 * today's prices and writes a new file. Any problem leaves the last good file untouched, so players
 * keep the last prices (the app shows them as old after 12 hours).
 */
export async function refresh({ request, env, fetchImpl, store, now, log = console }) {
  const secret = env.CRON_SECRET;
  const key = env.COZY_MARKET_API_KEY;
  if (!secret || !key) {
    log.error("price job: CRON_SECRET or COZY_MARKET_API_KEY is missing");
    return json(500, { error: "The price job isn't set up." });
  }
  if (!signedWith(request, secret)) {
    return json(401, { error: "Not allowed." });
  }

  const state = readState(await store.read(STATE_PATH, { fresh: true }), log);
  // Every world is checked on every run, so each one's hold reflects this run's answer.
  const fetched = {};
  const holds = {};
  const problems = [];
  for (const world of WORLDS) {
    const kept = state.worlds[world] ?? {};
    const usual = await getCozy(fetchImpl, key, "/usual-prices", world);
    const trends = usual.problem ? null : await getCozy(fetchImpl, key, "/trends", world);
    const shape = usual.problem ?? checkUsualShape(usual.body, world);
    const size = shape ? { hold: null, problem: null } : checkSize(usual.body, kept);
    holds[world] = size.hold;
    if (size.confirmed) {
      log.warn?.(`price job: ${world}: accepting a lasting change to ${usual.body.items.length} items`);
    }
    const problem = shape ?? size.problem ?? trends.problem ?? checkTrends(trends.body, world);
    if (problem) problems.push({ world, problem });
    else fetched[world] = { usual: usual.body, trends: trends.body };
  }
  if (problems.length > 0) {
    for (const { world, problem } of problems) {
      log.error(`price job: ${world}: ${problem}; keeping the last good file`);
    }
    // Only the holds change: a hold lasts exactly until the next run, which confirms or clears it.
    const changed = WORLDS.some((world) => JSON.stringify(holds[world]) !== JSON.stringify(state.worlds[world]?.held ?? null));
    if (changed) {
      for (const world of WORLDS) {
        const { held, ...rest } = state.worlds[world] ?? {};
        state.worlds[world] = holds[world] ? { ...rest, held: holds[world] } : rest;
      }
      await store.write(STATE_PATH, JSON.stringify(state));
    }
    return json(502, { error: problems[0].problem, world: problems[0].world });
  }

  const nextState = { version: 1, worlds: {} };
  const file = {
    version: FORMAT_VERSION,
    source: "Cozy Markets",
    attribution: "Prices from Cozy Markets (https://cozy.markets)",
    built_at: now.toISOString(),
    worlds: {},
  };
  for (const world of WORLDS) {
    const { usual, trends } = fetched[world];
    const today = dayOf(usual.updated_at);
    const days = recordDay(state.worlds[world]?.days ?? {}, today, usual.items);
    nextState.worlds[world] = { count: usual.items.length, days };
    file.worlds[world] = buildWorld(usual, trends, days, today);
  }
  await store.write(STATE_PATH, JSON.stringify(nextState));
  await store.write(FILE_PATH, JSON.stringify(file));
  const items = Object.fromEntries(WORLDS.map((world) => [world, file.worlds[world].items.length]));
  return json(200, { built_at: file.built_at, items });
}

async function getCozy(fetchImpl, key, path, world) {
  const url = `${COZY_API}${path}${world === "windia" ? "" : `?world=${world}`}`;
  let response;
  try {
    response = await fetchImpl(url, {
      headers: { Authorization: `Bearer ${key}`, Accept: "application/json" },
      signal: AbortSignal.timeout(COZY_TIMEOUT_MS),
    });
  } catch (error) {
    return { problem: `${path} couldn't be reached (${error.name})` };
  }
  if (response.status !== 200) return { problem: `${path} answered ${response.status}` };
  try {
    return { body: await response.json() };
  } catch {
    return { problem: `${path} answered with something that isn't JSON` };
  }
}

/** Why a usual-prices answer can't be used because of its shape, or null when it's good. */
function checkUsualShape(body, world) {
  const common = checkCommon(body, world);
  if (common) return common;
  if (Number.isNaN(Date.parse(body.updated_at))) return "usual prices have no update time";
  if (!Array.isArray(body.items) || body.items.length === 0) return "usual prices list no items";
  const bad = body.items.find((item) => !Number.isInteger(item?.item_id) || !isPriceOrZero(item?.usual));
  if (bad) return "an item has no item_id or no usual price";
  const zero = body.items.filter((item) => item.usual === 0).length;
  if (zero / body.items.length > MAX_ZERO_SHARE) return `${zero} of ${body.items.length} items have a usual price of 0`;
  return null;
}

/**
 * The size check against the last good run's count (`kept.count`). A list more than 20% off is
 * held: `hold` is what this run leaves for the next one, and null clears any hold. It's accepted
 * (`confirmed`) when the run right before held a size within 5% of it from an earlier Cozy update,
 * so a repeated run, which sees the same update again, never confirms a hold by itself. A confirmed
 * run keeps the earlier hold: if it then fails for another reason, a retry still confirms.
 */
function checkSize(body, kept) {
  const count = body.items.length;
  const change = (from) => Math.abs(count - from) / from;
  if (!kept.count || change(kept.count) <= MAX_COUNT_CHANGE) return { hold: null, problem: null };
  const hold = { count, updated_at: body.updated_at };
  const held = kept.held;
  const confirmed =
    Number.isInteger(held?.count) && change(held.count) <= HELD_MATCH && Date.parse(body.updated_at) > Date.parse(held.updated_at);
  if (confirmed) return { hold: held, problem: null, confirmed };
  return { hold, problem: `usual prices list ${count} items, against ${kept.count} last time` };
}

/** Why a trends answer can't be used, or null when it's good. */
export function checkTrends(body, world) {
  const common = checkCommon(body, world);
  if (common) return common;
  const week = body.movers?.["7d"];
  if (!Array.isArray(week?.up) || !Array.isArray(week?.down)) return "trends have no 7-day movers";
  const bad = [...week.up, ...week.down].find(
    (item) => !Number.isInteger(item?.item_id) || !isPrice(item?.price) || !Number.isFinite(item?.change),
  );
  if (bad) return "a mover has no item_id, price or change";
  return null;
}

function checkCommon(body, world) {
  if (body === null || typeof body !== "object") return "the answer isn't an object";
  if (body.source !== "Cozy Markets") return "the answer doesn't come from Cozy Markets";
  if (typeof body.world === "string" && body.world.toLowerCase() !== world) {
    return `the answer is for ${body.world}, not ${world}`;
  }
  return null;
}

/** Whether the request carries the job's secret. Compared as digests, in constant time. */
function signedWith(request, secret) {
  const digest = (value) => createHash("sha256").update(value).digest();
  return timingSafeEqual(digest(request.headers.get("authorization") ?? ""), digest(`Bearer ${secret}`));
}

const isPrice = (value) => typeof value === "number" && Number.isFinite(value) && value > 0;
const isPriceOrZero = (value) => value === 0 || isPrice(value);
/** An item the app can price: one the game has (a `game_id`) with a usual price above 0. */
const priced = (item) => Number.isInteger(item.game_id) && isPrice(item.usual);

// MARK: building the file

/** The kept days with `today` set to these items' usual prices, keeping only the last 31 days. */
export function recordDay(days, today, items) {
  const prices = {};
  for (const item of items) {
    if (priced(item)) prices[item.game_id] = item.usual;
  }
  const oldest = shiftDay(today, -(KEEP_DAYS - 1));
  const kept = Object.fromEntries(Object.entries(days).filter(([day]) => day >= oldest && day !== today));
  return { ...kept, [today]: prices };
}

/**
 * One world's part of the file. Items the game doesn't have (no `game_id`) are left out, since the
 * app finds items by the game's own id, and so are items at a usual price of 0, which the app
 * shows at their NPC price. The weekly change is against the price kept 7 days ago, like Cozy's
 * own; it's null until 7 days have been kept.
 */
export function buildWorld(usual, trends, days, today) {
  const historyDays = Array.from({ length: HISTORY_DAYS }, (_, i) => shiftDay(today, i - (HISTORY_DAYS - 1)));
  const week = historyDays.slice(-7);
  const weekAgo = shiftDay(today, -7);
  const items = usual.items
    .filter(priced)
    .map((item) => {
      const kept = (day) => days[day]?.[item.game_id];
      const base = kept(weekAgo);
      const thisWeek = week.map(kept).filter(isPrice);
      return {
        game_id: item.game_id,
        item_id: item.item_id,
        name: item.name,
        gender: item.gender ?? null,
        usual: item.usual,
        basis: item.basis,
        window: item.window,
        listings: item.listings,
        sales: item.sales,
        url: item.url,
        change_7d: isPrice(base) ? Math.round((item.usual / base - 1) * 10_000) / 10_000 : null,
        low_7d: Math.min(...thisWeek),
        high_7d: Math.max(...thisWeek),
        history: historyDays.map((day) => kept(day) ?? null),
      };
    });
  const mover = (item) => ({
    game_id: item.game_id,
    item_id: item.item_id,
    name: item.name,
    gender: item.gender ?? null,
    price: item.price,
    change: item.change,
    url: item.url,
  });
  const known = (item) => Number.isInteger(item.game_id);
  return {
    updated_at: usual.updated_at,
    next_update_at: usual.next_update_at ?? null,
    history_days: historyDays,
    items,
    movers: {
      up: trends.movers["7d"].up.filter(known).map(mover),
      down: trends.movers["7d"].down.filter(known).map(mover),
    },
  };
}

/** The UTC day of an ISO time, as YYYY-MM-DD. */
export const dayOf = (iso) => new Date(iso).toISOString().slice(0, 10);

/** `day` moved by `by` days. */
export function shiftDay(day, by) {
  const date = new Date(`${day}T00:00:00Z`);
  date.setUTCDate(date.getUTCDate() + by);
  return date.toISOString().slice(0, 10);
}

function readState(text, log) {
  if (text === null) return { version: 1, worlds: {} };
  try {
    const state = JSON.parse(text);
    if (state?.version === 1 && typeof state.worlds === "object" && state.worlds !== null) return state;
  } catch {
    // Falls through: a damaged state only costs the kept days, which the next runs fill again.
  }
  log.error("price job: the kept days couldn't be read; starting them again");
  return { version: 1, worlds: {} };
}

// MARK: responses

function json(status, body, headers = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store", ...headers },
  });
}

function text(status, body, headers = {}) {
  return new Response(body, {
    status,
    headers: { "Content-Type": "text/plain; charset=utf-8", "Cache-Control": "no-store", ...headers },
  });
}
