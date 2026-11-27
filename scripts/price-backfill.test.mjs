// The one-off history backfill in web/api: it fills each item's last 30 days from Cozy's per-item
// answer, a few items a run, in its own file, and the price job uses those days where it kept none.
// Run against a fake Cozy and a fake store, so no key or network is needed.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

import { BACKFILL_SCHEDULE, BUDGET_MS, LEASE_MS, MAX_TRIES, PER_RUN, backfill, historyOf } from "../web/api/_lib/backfill.js";
import { BACKFILL_PATH, FILE_PATH, STATE_PATH, refresh, shiftDay } from "../web/api/_lib/prices.js";

const NOW = new Date("2026-11-26T18:00:30Z");
const TODAY = "2026-11-26";
const ENV = { CRON_SECRET: "test-secret", COZY_MARKET_API_KEY: "cozy_test" };
const silent = { error() {}, warn() {} };
const signed = new Request("https://mesovibe.com/api/backfill", { headers: { authorization: "Bearer test-secret" } });

function memoryStore(files = {}) {
  const data = new Map(Object.entries(files));
  const writes = [];
  return {
    data,
    writes,
    async read(path) {
      return data.has(path) ? data.get(path) : null;
    },
    async write(path, text) {
      writes.push(path);
      data.set(path, text);
    },
  };
}

/** A price file listing `count` items in Windia and 2 in Zenith, Cozy ids 5000+. */
function priceFile(count) {
  const items = (n, offset = 0) =>
    Array.from({ length: n }, (_, i) => ({ game_id: 1_000_000 + offset + i, item_id: 5000 + offset + i, name: `Item ${i}`, usual: 1000 }));
  return JSON.stringify({ version: 1, worlds: { windia: { items: items(count) }, zenith: { items: items(2, 900) } } });
}

/** A per-item answer: 30 days of prices as `{ day, usual }`, the last one today. */
const history = (price) => ({ history: Array.from({ length: 30 }, (_, i) => ({ day: shiftDay(TODAY, i - 29), usual: price + i })) });

/** A fake Cozy answering `/items/{id}` with `answer(id, world)`. */
function fakeCozy(answer) {
  const asked = [];
  const fetchImpl = async (url) => {
    const [, id, world] = url.match(/\/items\/(\d+)(?:\?world=(\w+))?$/) ?? [];
    asked.push(Number(id));
    const body = answer(Number(id), world ?? "windia");
    if (body instanceof Response) return body;
    return Response.json(body);
  };
  return { fetchImpl, asked };
}

const run = (store, cozy, extra = {}) =>
  backfill({ request: signed, env: ENV, fetchImpl: cozy.fetchImpl, store, now: NOW, log: silent, ...extra });

// A run asks for at most PER_RUN items, records their days, and the next run carries on where it
// stopped; once everything is filled, runs ask for nothing. Fails if a run goes past Cozy's limit,
// repeats work, or never ends.
test("the backfill fills a few items a run until every item is done", async () => {
  const store = memoryStore({ [FILE_PATH]: priceFile(PER_RUN + 10) });
  const cozy = fakeCozy(() => history(1000));
  const first = await (await run(store, cozy)).json();
  assert.deepEqual(first, { filled: PER_RUN, failed: 0, left: 12 });
  assert.equal(cozy.asked.length, PER_RUN);
  const second = await (await run(store, cozy)).json();
  assert.deepEqual(second, { filled: 12, failed: 0, left: 0 });
  assert.equal(new Set(cozy.asked).size, cozy.asked.length, "no item asked for twice");
  const third = await (await run(store, cozy)).json();
  assert.deepEqual(third, { done: true });
  assert.equal(cozy.asked.length, PER_RUN + 12);
  const kept = JSON.parse(store.data.get(BACKFILL_PATH));
  assert.equal(kept.lease, null);
  assert.equal(Object.keys(kept.worlds.windia[1_000_000].days).length, 30);
});

// A failed or unrecognized answer is tried again on later runs, then left to the day snapshots.
// Fails if one bad item is asked for forever, or a shape the backfill doesn't know fills anything.
test("an item whose answer fails or isn't a history it knows is left after a few tries", async () => {
  const store = memoryStore({ [FILE_PATH]: priceFile(2) });
  const cozy = fakeCozy((id) => (id === 5000 ? new Response("no", { status: 500 }) : id === 5001 ? { prices: [1, 2, 3] } : history(10)));
  for (let i = 0; i < MAX_TRIES; i += 1) await run(store, cozy);
  const kept = JSON.parse(store.data.get(BACKFILL_PATH)).worlds.windia;
  assert.deepEqual(kept[1_000_000], { tries: MAX_TRIES });
  assert.deepEqual(kept[1_000_001], { tries: MAX_TRIES });
  assert.deepEqual(await (await run(store, cozy)).json(), { done: true });
  assert.equal(cozy.asked.filter((id) => id === 5000).length, MAX_TRIES);
});

// A run stops asking once its time is up, so it ends before the next one starts; a run that finds
// another one's fresh lease leaves the file alone. Fails if two runs could write the file at once.
test("a run keeps to its time, and an overlapping run writes nothing", async () => {
  const store = memoryStore({ [FILE_PATH]: priceFile(20) });
  let t = 0;
  const slow = fakeCozy(() => {
    t += 10_000;
    return history(1000);
  });
  const body = await (await run(store, slow, { clock: () => t })).json();
  assert.equal(body.filled, Math.floor(BUDGET_MS / 10_000) + 1);
  const busy = memoryStore({
    [FILE_PATH]: priceFile(2),
    [BACKFILL_PATH]: JSON.stringify({ version: 1, lease: new Date(NOW.getTime() - LEASE_MS + 1000).toISOString(), worlds: {} }),
  });
  assert.deepEqual(await (await run(busy, fakeCozy(() => history(1)))).json(), { busy: true });
  assert.deepEqual(busy.writes, []);
});

// Only the scheduler, signed with CRON_SECRET, may run the backfill. Fails if anyone could spend
// the key's requests.
test("the backfill answers only a signed request", async () => {
  const store = memoryStore({ [FILE_PATH]: priceFile(2) });
  const response = await backfill({
    request: new Request("https://mesovibe.com/api/backfill"),
    env: ENV,
    fetchImpl: fakeCozy(() => history(1)).fetchImpl,
    store,
    now: NOW,
    log: silent,
  });
  assert.equal(response.status, 401);
  assert.deepEqual(store.writes, []);
});

// Both shapes the backfill reads give the same days, only the last 30 up to today. Fails if a day
// outside the window, or a missing price, gets in.
test("a history is read as days and prices within the last 30 days", () => {
  const listed = historyOf({ history: [{ date: shiftDay(TODAY, -40), price: 5 }, { date: shiftDay(TODAY, -1), price: 7 }, { day: TODAY, usual: null }] }, TODAY);
  assert.deepEqual(listed, { [shiftDay(TODAY, -1)]: 7 });
  const days = [shiftDay(TODAY, -2), shiftDay(TODAY, -1), TODAY];
  assert.deepEqual(historyOf({ history: [3, null, 5], history_days: days }, TODAY), { [days[0]]: 3, [days[2]]: 5 });
  assert.equal(historyOf({ history: [3, 4] }, TODAY), null);
  assert.equal(historyOf({}, TODAY), null);
});

// The price job uses backfilled days only where it kept none, so the week's change and the chart
// work before it has a week of its own: a day it kept always wins. Fails if the backfill could
// overwrite a kept price, or the week's change ignores it.
test("the price job fills missing days from the backfill, never over a kept one", async () => {
  const usual = (items, world) => ({ source: "Cozy Markets", world, updated_at: "2026-11-26T18:08:00.000Z", count: items.length, items });
  const trends = (world) => ({ source: "Cozy Markets", world, basis: "sold", movers: { "7d": { up: [], down: [] } } });
  const item = (game_id, price) => ({ item_id: 7, game_id, name: "Cap", gender: null, usual: price, basis: "sold", window: "day", listings: 1, sales: 1, url: "u" });
  const answers = {
    "/usual-prices": usual([item(1002071, 1200)], "windia"),
    "/trends": trends("windia"),
    "/usual-prices?world=zenith": usual([item(1002071, 1300)], "zenith"),
    "/trends?world=zenith": trends("zenith"),
  };
  const fetchImpl = async (url) => Response.json(answers[url.replace("https://cozy.markets/trade-api/dev/v1", "")]);
  const kept = { [shiftDay(TODAY, -2)]: { 1002071: 1100 } };
  const filled = { [shiftDay(TODAY, -7)]: 1000, [shiftDay(TODAY, -2)]: 1, [shiftDay(TODAY, -20)]: 900 };
  const store = memoryStore({
    [STATE_PATH]: JSON.stringify({ version: 1, worlds: { windia: { count: 1, days: kept } } }),
    [BACKFILL_PATH]: JSON.stringify({ version: 1, lease: null, worlds: { windia: { 1002071: { days: filled } } } }),
  });
  const response = await refresh({
    request: new Request("https://mesovibe.com/api/refresh", { headers: { authorization: "Bearer test-secret" } }),
    env: ENV,
    fetchImpl,
    store,
    now: NOW,
    log: silent,
  });
  assert.equal(response.status, 200);
  const cap = JSON.parse(store.data.get(FILE_PATH)).worlds.windia.items[0];
  assert.equal(cap.change_7d, 0.2, "1200 against the backfilled 1000 seven days ago");
  assert.equal(cap.history.at(-3), 1100, "the kept day, not the backfill's 1");
  assert.equal(cap.history.at(-21), 900);
  const state = JSON.parse(store.data.get(STATE_PATH));
  assert.equal(state.worlds.windia.days[shiftDay(TODAY, -7)], undefined, "backfilled days stay in the backfill's file");
});

// vercel.json and the code both name the backfill's schedule. Fails if they drift apart.
test("vercel.json runs the backfill on the schedule the code expects", async () => {
  const config = JSON.parse(await readFile(new URL("../web/vercel.json", import.meta.url), "utf8"));
  assert.deepEqual(
    config.crons.filter((cron) => cron.path === "/api/backfill"),
    [{ path: "/api/backfill", schedule: BACKFILL_SCHEDULE }],
  );
  assert.equal(config.functions["api/backfill.js"].maxDuration, 60);
});
