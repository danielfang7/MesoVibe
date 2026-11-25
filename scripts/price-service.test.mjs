// The price service in web/api: the job that builds the price file from Cozy Markets, and the
// endpoint the app downloads it from. Run against a fake Cozy and a fake store, so no key or
// network is needed.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

import {
  FILE_PATH,
  REFRESH_SCHEDULE,
  STATE_PATH,
  refresh,
  secondsUntilNextBuild,
  serve,
  shiftDay,
} from "../web/api/_lib/prices.js";

const NOW = new Date("2026-11-25T06:15:30Z");
const UPDATED = "2026-11-25T06:08:59.160Z";
const ENV = { CRON_SECRET: "test-secret", COZY_MARKET_API_KEY: "cozy_test" };
const silent = { error() {} };

function memoryStore(files = {}) {
  const data = new Map(Object.entries(files));
  return {
    data,
    async read(path) {
      return data.has(path) ? data.get(path) : null;
    },
    async write(path, text) {
      data.set(path, text);
    },
  };
}

const item = (game_id, usual, extra = {}) => ({
  item_id: game_id ?? 9_000_001,
  game_id,
  name: `Item ${game_id}`,
  gender: null,
  usual,
  basis: "sold",
  window: "day",
  listings: 10,
  sales: 5,
  url: `https://cozy.markets/items/${game_id}`,
  ...extra,
});

function usualPrices(items, world = "windia", updated_at = UPDATED) {
  return {
    source: "Cozy Markets",
    world,
    updated_at,
    next_update_at: "2026-11-25T12:00:00.000Z",
    count: items.length,
    items,
  };
}

function trends(world = "windia") {
  const mover = (game_id, change) => ({ item_id: game_id ?? 1, game_id, name: "M", gender: null, price: 1500000, change, url: "u" });
  return {
    source: "Cozy Markets",
    world,
    basis: "sold",
    movers: { "7d": { up: [mover(1082000, 0.25), mover(null, 0.5)], down: [mover(1002071, -0.1)] } },
  };
}

/** A fake Cozy that answers each path with `answers[path]` (a body, or a Response for errors). */
function fakeCozy(answers) {
  const calls = [];
  const fetchImpl = async (url, init) => {
    calls.push({ url, auth: init.headers.Authorization });
    const path = url.replace("https://cozy.markets/trade-api/dev/v1", "");
    const answer = answers[path];
    if (answer instanceof Error) throw answer;
    if (answer instanceof Response) return answer;
    return Response.json(answer);
  };
  return { fetchImpl, calls };
}

const goodAnswers = (windiaItems = [item(1002071, 1200), item(1082000, 1500000)]) => ({
  "/usual-prices": usualPrices(windiaItems),
  "/trends": trends(),
  "/usual-prices?world=zenith": usualPrices([item(1002071, 1300), item(1082000, 1400000)], "zenith"),
  "/trends?world=zenith": trends("zenith"),
});

const signed = new Request("https://mesovibe.com/api/refresh", { headers: { authorization: "Bearer test-secret" } });

async function run(answers, store = memoryStore()) {
  const cozy = fakeCozy(answers);
  const response = await refresh({ request: signed, env: ENV, fetchImpl: cozy.fetchImpl, store, now: NOW, log: silent });
  return { response, store, cozy };
}

// The file the app reads: change, week range and chart come from the kept days. Fails if the
// weekly change compares against the wrong day, or the chart's days drift out of line.
test("the job builds each world's prices, weekly change, week range and 30-day chart", async () => {
  const days = {
    [shiftDay("2026-11-25", -7)]: { 1002071: 1000 },
    [shiftDay("2026-11-25", -3)]: { 1002071: 900, 1082000: 1600000 },
  };
  const store = memoryStore({ [STATE_PATH]: JSON.stringify({ version: 1, worlds: { windia: { count: 2, days } } }) });
  const { response } = await run(goodAnswers(), store);
  assert.equal(response.status, 200);

  const file = JSON.parse(store.data.get(FILE_PATH));
  assert.equal(file.version, 1);
  assert.equal(file.attribution, "Prices from Cozy Markets (https://cozy.markets)");
  const windia = file.worlds.windia;
  assert.equal(windia.updated_at, UPDATED);
  assert.equal(windia.history_days.length, 30);
  assert.equal(windia.history_days.at(-1), "2026-11-25");

  const cap = windia.items.find((i) => i.game_id === 1002071);
  assert.equal(cap.usual, 1200);
  assert.equal(cap.change_7d, 0.2, "1200 against 1000 kept 7 days ago");
  assert.equal(cap.low_7d, 900);
  assert.equal(cap.high_7d, 1200, "today's price counts in the week's range");
  assert.equal(cap.history.at(-1), 1200);
  assert.equal(cap.history.at(-4), 900);
  assert.equal(cap.history.at(-2), null, "a day with no kept price stays empty, never guessed");

  const gloves = windia.items.find((i) => i.game_id === 1082000);
  assert.equal(gloves.change_7d, null, "nothing kept from 7 days ago");
  assert.equal(file.worlds.zenith.items.find((i) => i.game_id === 1002071).usual, 1300);

  assert.deepEqual(windia.movers.up.map((m) => m.game_id), [1082000], "movers the game doesn't have are left out");
  assert.deepEqual(windia.movers.down.map((m) => m.game_id), [1002071]);
});

// Items the game's own files don't have can't be matched by the app. Fails if one reaches the file.
test("items with no game id are left out of the file", async () => {
  const { store } = await run(goodAnswers([item(1002071, 1200), item(null, 500)]));
  const ids = JSON.parse(store.data.get(FILE_PATH)).worlds.windia.items.map((i) => i.game_id);
  assert.deepEqual(ids, [1002071]);
});

// Cozy lists a few items at 0 (arrows sell for under a meso), which the app shows at their NPC
// price. Fails if one blocks the build, or reaches the file or the kept days as a price.
test("items priced 0 are left out, and a few of them don't block the build", async () => {
  const items = [item(2060000, 0), ...Array.from({ length: 24 }, (_, i) => item(1002000 + i, 100 + i))];
  const answers = { ...goodAnswers(), "/usual-prices": usualPrices(items) };
  const { response, store } = await run(answers);
  assert.equal(response.status, 200, "1 of 25 at 0 is under the 5% limit");
  const ids = JSON.parse(store.data.get(FILE_PATH)).worlds.windia.items.map((i) => i.game_id);
  assert.ok(!ids.includes(2060000));
  assert.equal(ids.length, 24);
  assert.ok(!("2060000" in JSON.parse(store.data.get(STATE_PATH)).worlds.windia.days["2026-11-25"]));
});

// The kept days feed the chart; older ones are dropped. Fails if the state grows without end.
test("the job keeps 31 days of prices, today included", async () => {
  const days = Object.fromEntries(
    Array.from({ length: 40 }, (_, i) => [shiftDay("2026-11-25", -(i + 1)), { 1002071: 1000 + i }]),
  );
  const store = memoryStore({ [STATE_PATH]: JSON.stringify({ version: 1, worlds: { windia: { count: 2, days } } }) });
  await run(goodAnswers(), store);
  const kept = Object.keys(JSON.parse(store.data.get(STATE_PATH)).worlds.windia.days).sort();
  assert.equal(kept.length, 31);
  assert.equal(kept[0], shiftDay("2026-11-25", -30));
  assert.equal(kept.at(-1), "2026-11-25");
});

// Each world is asked for by name (Windia is Cozy's default), with the key only in the header.
// Fails if a world's prices come from the wrong address or the key leaks into a URL.
test("the job asks Cozy for both worlds with the key in the Authorization header", async () => {
  const { cozy } = await run(goodAnswers());
  assert.deepEqual(
    cozy.calls.map((c) => c.url.replace("https://cozy.markets/trade-api/dev/v1", "")),
    ["/usual-prices", "/trends", "/usual-prices?world=zenith", "/trends?world=zenith"],
  );
  assert.ok(cozy.calls.every((c) => c.auth === "Bearer cozy_test" && !c.url.includes("cozy_test")));
});

// A bad Cozy answer must never replace good prices for every player. Each case fails if the job
// writes anything or reports success.
const lastGood = { [FILE_PATH]: "LAST GOOD FILE", [STATE_PATH]: JSON.stringify({ version: 1, worlds: { windia: { count: 2, days: {} } } }) };
const broken = {
  "Cozy is down": { "/usual-prices": new Response("{}", { status: 503 }) },
  "the key is refused": { "/usual-prices": new Response("{}", { status: 401 }) },
  "the app is no longer approved": { "/trends?world=zenith": new Response("{}", { status: 403 }) },
  "too many requests": { "/trends": new Response("{}", { status: 429 }) },
  "Cozy can't be reached": { "/usual-prices": new TypeError("fetch failed") },
  "the answer isn't Cozy's": { "/usual-prices": { ...usualPrices([item(1002071, 1200), item(1082000, 1)]), source: "Someone" } },
  "the list shrank by half": { "/usual-prices": usualPrices([item(1002071, 1200)]) },
  "half the items are priced 0": { "/usual-prices": usualPrices([item(1002071, 0), item(1082000, 1500000)]) },
  "a price isn't a number": { "/usual-prices": usualPrices([item(1002071, "1200"), item(1082000, 1500000)]) },
  "a price is below 0": { "/usual-prices": usualPrices([item(1002071, -5), item(1082000, 1500000)]) },
  "an item has no id": { "/usual-prices": usualPrices([item(1002071, 1200), { ...item(1082000, 5), item_id: "x" }]) },
  "the world is wrong": { "/usual-prices?world=zenith": usualPrices([item(1002071, 1), item(1082000, 2)], "windia") },
  "the trends lost their movers": { "/trends": { ...trends(), movers: {} } },
  "the answer isn't JSON": { "/trends": new Response("<html>", { status: 200 }) },
  "a mover has no price": { "/trends?world=zenith": { ...trends("zenith"), movers: { "7d": { up: [{ item_id: 1, game_id: 1, price: null, change: 0.1 }], down: [] } } } },
  "the prices have no update time": { "/usual-prices": { ...usualPrices([item(1002071, 1200), item(1082000, 1500000)]), updated_at: undefined } },
  "the list is empty": { "/usual-prices?world=zenith": usualPrices([], "zenith") },
};
for (const [name, override] of Object.entries(broken)) {
  test(`a bad answer keeps the last good file: ${name}`, async () => {
    const store = memoryStore({ ...lastGood });
    const { response } = await run({ ...goodAnswers(), ...override }, store);
    assert.equal(response.status, 502);
    assert.equal(store.data.get(FILE_PATH), "LAST GOOD FILE");
    const { count, days } = JSON.parse(store.data.get(STATE_PATH)).worlds.windia;
    assert.deepEqual({ count, days }, { count: 2, days: {} }, "the kept days and count are untouched");
  });
}

// Windia lists of `n` items from Cozy's update at `hour` UTC that day; Zenith stays good.
const sized = (n) => Array.from({ length: n }, (_, i) => item(1_000_000 + i, 100 + i));
const update = (hour) => `2026-11-25T${String(hour).padStart(2, "0")}:08:59.000Z`;
const listing = (n, hour, extra = {}) => ({ ...goodAnswers(), "/usual-prices": usualPrices(sized(n), "windia", update(hour)), ...extra });
const held = (store) => JSON.parse(store.data.get(STATE_PATH)).worlds.windia.held;

// A real change in the size of Cozy's list must not stop the job for good: the first update with
// the new size is held, and the next update with the same size is accepted. Fails if a lasting
// change is rejected forever, or a one-off jump is accepted at once.
test("a lasting change in the size of Cozy's list is accepted once the next update agrees", async () => {
  const store = memoryStore();
  assert.equal((await run(listing(100, 0), store)).response.status, 200);

  assert.equal((await run(listing(130, 6), store)).response.status, 502, "a jump of 30% is held, not used");
  assert.deepEqual(held(store), { count: 130, updated_at: update(6) });
  const before = store.data.get(FILE_PATH);

  assert.equal((await run(listing(128, 12), store)).response.status, 200, "the next update within 5% of the held size is accepted");
  assert.notEqual(store.data.get(FILE_PATH), before);
  assert.equal(JSON.parse(store.data.get(STATE_PATH)).worlds.windia.count, 128);
  assert.equal(held(store), undefined, "an accepted size clears the hold");
});

// A repeated run (Vercel can deliver a scheduled run twice; the owner can retry by hand) sees the
// same Cozy update again. Fails if that repeat confirms a held size and publishes a half-empty file.
test("a repeat of the same Cozy update never confirms a held size", async () => {
  const store = memoryStore();
  await run(listing(100, 0), store);
  const good = store.data.get(FILE_PATH);
  assert.equal((await run(listing(50, 6), store)).response.status, 502);
  assert.equal((await run(listing(50, 6), store)).response.status, 502, "the same update again is still held");
  assert.equal(store.data.get(FILE_PATH), good, "the half-empty list was never published");
});

// A run can confirm a lasting change and still fail on something else, such as Zenith's movers.
// Fails if that run's own update replaces the hold, so retrying the same update is refused.
test("a retry after a confirming run that failed elsewhere still accepts the change", async () => {
  const store = memoryStore();
  await run(listing(100, 0), store);
  await run(listing(130, 6), store);
  const zenithDown = { "/trends?world=zenith": new Response("{}", { status: 503 }) };
  assert.equal((await run(listing(128, 12, zenithDown), store)).response.status, 502);
  assert.deepEqual(held(store), { count: 130, updated_at: update(6) }, "the earlier update's hold is kept");
  assert.equal((await run(listing(128, 12), store)).response.status, 200, "the retry accepts the change");
});

// A hold lasts only until the next run. Fails if a run that saw a normal list but failed for another
// reason leaves the hold in place, letting two broken lists hours apart agree.
test("a run that sees a normal-size list clears the hold even when it fails", async () => {
  const store = memoryStore();
  await run(listing(100, 0), store);
  await run(listing(50, 6), store);
  const badTrends = { "/trends": { ...trends(), movers: {} } };
  assert.equal((await run(listing(100, 12, badTrends), store)).response.status, 502);
  assert.equal(held(store), undefined, "a normal list clears the hold");
  assert.equal((await run(listing(50, 18), store)).response.status, 502, "a later short list is held again, not accepted");
});

// Only Vercel's scheduler, signed with CRON_SECRET, may make the job call Cozy. Fails if an
// unsigned request reaches Cozy or a misconfigured job runs.
test("the job refuses unsigned requests and runs only when set up", async () => {
  for (const headers of [{}, { authorization: "Bearer wrong" }]) {
    const cozy = fakeCozy(goodAnswers());
    const request = new Request("https://mesovibe.com/api/refresh", { headers });
    const response = await refresh({ request, env: ENV, fetchImpl: cozy.fetchImpl, store: memoryStore(), now: NOW, log: silent });
    assert.equal(response.status, 401);
    assert.equal(cozy.calls.length, 0);
  }
  for (const env of [{ COZY_MARKET_API_KEY: "k" }, { CRON_SECRET: "test-secret" }]) {
    const cozy = fakeCozy(goodAnswers());
    const response = await refresh({ request: signed, env, fetchImpl: cozy.fetchImpl, store: memoryStore(), now: NOW, log: silent });
    assert.equal(response.status, 500);
    assert.equal(cozy.calls.length, 0);
  }
});

// A damaged state only loses the kept days. Fails if it stops the job from building a file.
test("a damaged state starts the kept days again instead of failing", async () => {
  const store = memoryStore({ [STATE_PATH]: "not json" });
  const { response } = await run(goodAnswers(), store);
  assert.equal(response.status, 200);
  assert.deepEqual(Object.keys(JSON.parse(store.data.get(STATE_PATH)).worlds.windia.days), ["2026-11-25"]);
});

const get = (url, headers = {}) => new Request(url, { headers });

// One plain address, so a query string or an Authorization header (never cached by the CDN) can't
// make it miss and bill a run. Fails if either is served or reads the store.
test("the endpoint refuses a query string or an Authorization header without reading the store", async () => {
  let reads = 0;
  const store = { async read() { reads += 1; return "{}"; } };
  const requests = [
    get("https://mesovibe.com/api/prices?x=1"),
    get("https://mesovibe.com/api/prices?world=zenith"),
    get("https://mesovibe.com/api/prices", { Authorization: "Bearer anything" }),
  ];
  for (const request of requests) {
    const response = await serve({ request, store, now: NOW });
    assert.equal(response.status, 400);
  }
  assert.equal(reads, 0);
});

// Storage failing (no Blob store connected yet, an outage) must answer like "not ready", not crash.
// Fails if serve() throws or the error answer is cached for long.
test("the endpoint answers 503 for a short time when the price file can't be read", async () => {
  const store = { async read() { throw new Error("Vercel Blob: No blob credentials found."); } };
  const response = await serve({ request: get("https://mesovibe.com/api/prices"), store, now: NOW, log: silent });
  assert.equal(response.status, 503);
  assert.equal(response.headers.get("cache-control"), "public, max-age=0, s-maxage=60");
  assert.equal(response.headers.get("retry-after"), "300");
});

// The file, its ETag and a CDN lifetime that ends just after the next build. Fails if the app
// can't revalidate or the CDN holds a file past the next build.
test("the endpoint serves the file with an ETag, answers 304 when unchanged, and 503 before the first build", async () => {
  const store = memoryStore({ [FILE_PATH]: '{"version":1}' });
  const first = await serve({ request: get("https://mesovibe.com/api/prices"), store, now: new Date("2026-11-25T05:00:00Z") });
  assert.equal(first.status, 200);
  assert.equal(await first.text(), '{"version":1}');
  assert.equal(first.headers.get("cache-control"), "public, max-age=0, s-maxage=4800, stale-if-error=86400");
  const etag = first.headers.get("etag");
  assert.match(etag, /^"[\w-]+"$/);

  const again = await serve({ request: get("https://mesovibe.com/api/prices", { "If-None-Match": etag }), store, now: NOW });
  assert.equal(again.status, 304);

  const empty = await serve({ request: get("https://mesovibe.com/api/prices"), store: memoryStore(), now: NOW });
  assert.equal(empty.status, 503);
});

// The CDN keeps a copy until 5 minutes after the next run. A copy fetched while a run may still be
// writing lasts only to the end of its margin. Fails if a run time, the margin or the day's
// rollover is wrong, which could hold an old file for 6 hours.
test("the cached copy lasts until just after the next scheduled run", () => {
  const at = (iso) => secondsUntilNextBuild(new Date(iso));
  assert.equal(at("2026-11-25T06:14:00Z"), 6 * 60);
  assert.equal(at("2026-11-25T06:16:00Z"), 4 * 60, "inside the 06:15 run's margin");
  assert.equal(at("2026-11-25T06:19:30Z"), 60, "never under a minute");
  assert.equal(at("2026-11-25T06:20:00Z"), 6 * 3600, "once the margin ends, the next run counts");
  assert.equal(at("2026-11-25T23:00:00Z"), 3600 + 20 * 60);
});

// vercel.json and the code both name the schedule. Fails if they drift apart.
test("vercel.json runs the job on the schedule the code expects", async () => {
  const config = JSON.parse(await readFile(new URL("../web/vercel.json", import.meta.url), "utf8"));
  assert.deepEqual(config.crons, [{ path: "/api/refresh", schedule: REFRESH_SCHEDULE }]);
});

// The two limits are "more than": a list that moves by exactly 20%, or exactly 5% priced 0, still
// builds. Fails if either check turns strict and a normal day blocks the file.
test("the count and zero-price limits allow exactly 20% and 5%", async () => {
  const many = (n, zero = 0) => Array.from({ length: n }, (_, i) => item(1_000_000 + i, i < zero ? 0 : 100));
  const after100 = () => memoryStore({ [STATE_PATH]: JSON.stringify({ version: 1, worlds: { windia: { count: 100, days: {} } } }) });
  const status = async (items, store = after100()) =>
    (await run({ ...goodAnswers(), "/usual-prices": usualPrices(items) }, store)).response.status;
  assert.equal(await status(many(120)), 200);
  assert.equal(await status(many(80)), 200);
  assert.equal(await status(many(121)), 502);
  assert.equal(await status(many(100, 5), memoryStore()), 200);
  assert.equal(await status(many(100, 6), memoryStore()), 502);
});

// HEAD, other methods and the app's revalidation forms. Fails if HEAD sends the body, a POST is
// served, or an If-None-Match list or "*" misses and sends the whole file again.
test("the endpoint answers HEAD without a body, refuses POST, and honors If-None-Match lists", async () => {
  const store = memoryStore({ [FILE_PATH]: '{"version":1}' });
  const url = "https://mesovibe.com/api/prices";
  const head = await serve({ request: new Request(url, { method: "HEAD" }), store, now: NOW });
  assert.equal(head.status, 200);
  assert.equal(await head.text(), "");
  const etag = head.headers.get("etag");
  const post = await serve({ request: new Request(url, { method: "POST" }), store, now: NOW });
  assert.equal(post.status, 405);
  assert.equal(post.headers.get("allow"), "GET, HEAD");
  for (const tags of [`"old", ${etag}`, "*"]) {
    const response = await serve({ request: get(url, { "If-None-Match": tags }), store, now: NOW });
    assert.equal(response.status, 304, tags);
  }
  const stale = await serve({ request: get(url, { "If-None-Match": '"old"' }), store, now: NOW });
  assert.equal(stale.status, 200);
});
