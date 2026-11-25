// The Session Tracker's loot as the host runs it: time and EXP by map, Past sessions rows named for
// a map, the meso ledger (meso totals read in the inventory and shop windows), the Loot section and
// Loot tab built on it, sales after End, and sessions saved by builds that counted loot from the
// pickup notices. Bundled from source with the SDK and driven only through the SDK's messages,
// like session-tracker-plugin.test.mjs; the ledger's arithmetic and the Loot section are also
// driven directly through loot.ts and loot-view.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { bundleTracker, run as runBundle } from "./session-tracker-harness.mjs";
import { load as load070 } from "./fixtures/session-tracker-0.7.0-load.mjs";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const esbuild = createRequire(join(REPO, "plugins", "package.json"))("esbuild");

const bundle = await bundleTracker();

/** One of the plugin's modules, as the plugin bundles it. */
async function bundled(file) {
  const result = await esbuild.build({
    entryPoints: [join(REPO, "plugins/src/session-tracker", file)],
    bundle: true,
    write: false,
    format: "esm",
    platform: "neutral",
    target: "es2022",
    alias: { "@mesovibe/sdk": join(REPO, "sdk/src/index.ts") },
    logLevel: "silent",
  });
  return import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}`);
}
/** The loot arithmetic (loot.ts) and the Loot section (loot-view.ts). */
const loot = await bundled("loot.ts");
const lootView = await bundled("loot-view.ts");

const run = () => runBundle(bundle);

const last = (sent, type) => sent.filter((m) => m.type === type).at(-1);
const NEEDED = 356_916;
const T0 = 1_800_000_000_000;
const MIN = 60_000;
/** A host that doesn't read the meso total, and one that does. */
const PLAIN = { hpMp: "off", hpMpMissing: null };
const MESOS = { ...PLAIN, mesos: "reading" };
const event = (e, atMs) => ({ type: "event", atMs, event: e });
const exp = (value, atMs) =>
  event({ type: "ExpChanged", level: 42, exp: value, pct_hundredths: Math.round((value * 10_000) / NEEDED), needed: NEEDED }, atMs);
const map = (key, atMs) => event({ type: "MapChanged", map_key: key }, atMs);
/** A trusted meso total, as the host sends it. */
const total = (amount, atMs, source = "inventory") => event({ type: "MesoChanged", total: amount, source }, atMs);
const action = (name, atMs) => ({ type: "action", action: name, atMs });

/** A section of the last view drawn: the Session view, or with `page` that page ("loot"). */
const section = (sent, title, page) =>
  sent.filter((m) => m.type === "render" && m.page === page).at(-1).view.find((node) => node.title === title);
const thisSession = (sent) => section(sent, "This session")?.children ?? [];
const items = (sent) => Object.fromEntries(thisSession(sent).find((n) => n.kind === "keyValue")?.items ?? []);
const figuresOf = (node) => Object.fromEntries(node?.children.find((n) => n.kind === "keyValue")?.items ?? []);
/** The Loot section's figures, by label. */
const lootFigures = (sent) => figuresOf(section(sent, "Loot"));
/** The Loot section's muted and partial lines. */
const lootLines = (sent) => section(sent, "Loot").children.filter((n) => n.kind === "text" || n.kind === "partial").map((n) => n.text);
const pastSessions = (sent) => section(sent, "Past sessions").children;
const lootTable = (sent) => section(sent, "Past sessions", "loot").children.find((n) => n.kind === "table");
const allTimeFigures = (sent) => figuresOf(section(sent, "All time", "loot"));
const summary = (sent) => last(sent, "summary").summary;
const notices = (sent) => sent.filter((m) => m.type === "notice");

/**
 * A plugin started reading on map `key`, from `saved`: EXP every 10 s for `seconds`. Returns it and
 * the time it reached.
 */
function play({ seconds = 120, readers = MESOS, key = "aaaa", saved = null } = {}) {
  const plugin = run();
  plugin.send({ type: "start", saved, atMs: T0, capture: "capturing", readers });
  plugin.send(map(key, T0));
  plugin.send(exp(230_000, T0));
  for (let s = 1; s <= seconds; s++) {
    const at = T0 + s * 1000;
    plugin.send({ type: "tick", atMs: at });
    if (s % 10 === 0) plugin.send(exp(230_000 + (s / 10) * 63, at));
  }
  return { ...plugin, now: T0 + seconds * 1000 };
}

/** Plays on from `from` for `seconds`, a tick a second and an EXP gain every 10, from EXP `startExp`. */
function grind(plugin, from, seconds, startExp = 231_000) {
  for (let s = 1; s <= seconds; s++) {
    plugin.send({ type: "tick", atMs: from + s * 1000 });
    if (s % 10 === 0) plugin.send(exp(startExp + s * 10, from + s * 1000));
  }
  return from + seconds * 1000;
}

/** Two minutes of play, then 1,000,000 and 1,050,000 read ten minutes of play apart. Returns the time reached. */
function twoReadings(plugin) {
  plugin.send(total(1_000_000, plugin.now));
  const t = grind(plugin, plugin.now, 600);
  plugin.send(total(1_050_000, t));
  return t;
}

// The session's clock starts at its first EXP gain: a meso total after End must not start it
// early. Fails if EXP/hr after End, a total, three idle minutes and a kill differs from the same
// without the total.
test("a meso total before the first kill doesn't start the EXP/hr clock", () => {
  const rateAfter = (withTotal) => {
    const { send, sent, now } = play({ seconds: 60 });
    send(action("end", now));
    if (withTotal) send(total(1_000_000, now + 1_000));
    for (let s = 1; s <= 180; s++) send({ type: "tick", atMs: now + s * 1000 });
    for (let k = 1; k <= 6; k++) {
      send({ type: "tick", atMs: now + 180_000 + k * 10_000 });
      send(exp(230_378 + k * 63, now + 180_000 + k * 10_000));
    }
    return summary(sent).expPerHour;
  };
  assert.equal(rateAfter(true), rateAfter(false));
});

// The pickup notices no longer feed loot: their events, should any arrive, count nothing and
// show nowhere. Fails if a pickup event adds mesos or items to the session or brings back a
// notice-based figure.
test("pickup notice events count nothing", () => {
  const { send, sent, now } = play({ seconds: 120 });
  send(event({ type: "MesosGained", amount: 500, late: false, shaped: false }, now));
  send(event({ type: "ItemGained", item_key: "0000000000000000", count: 2, tab: "Etc", item_id: 4000028, name: "Tablecloth", sell: 34, sell_min: 34, sell_max: 34 }, now));
  send(event({ type: "NoticesMissed", reason: "shapes_wrong" }, now));
  assert.deepEqual(lootFigures(sent), { "Loot sold": "—" });
  assert.equal(section(sent, "Seen picking up"), undefined);
  assert.equal(summary(sent).lootPerHour, null);
  send({ type: "tick", atMs: now + 30_000 });
  const running = last(sent, "save").data.current;
  assert.equal(running.maps[0].m, 0);
  assert.equal(running.maps[0].i, undefined);
  assert.equal(running.shapesWrong, undefined);
  const plain = play({ seconds: 120, readers: PLAIN });
  plain.send(event({ type: "MesosGained", amount: 500, late: false, shaped: false }, plain.now));
  assert.equal(section(plain.sent, "Loot"), undefined, "no Loot from a host that reads no meso total");
});

// Past sessions rows are named for the map played longest, with the session's EXP gained and that
// map's own EXP/hr. Fails if the row shows the wrong map, misses "+N maps" or mixes another map's
// rate in.
test("a past session's row is its longest map, with that map's rates", () => {
  const { send, sent, now } = play({ seconds: 120 });
  send(map("bbbb", now));
  for (let s = 1; s <= 30; s++) {
    send({ type: "tick", atMs: now + s * 1000 });
    if (s % 10 === 0) send(exp(230_756 + s * 100, now + s * 1000));
  }
  send(action("end", now + 30_000));
  assert.match(last(sent, "notice").text, /^Saved: [\d,]+ EXP in \d+m \(.+\/hr\)\.$/);
  const table = pastSessions(sent)[0];
  assert.deepEqual(table.columns, ["Map", "EXP gained", "EXP/hr"]);
  const [cell, gained, expRate] = table.rows[0];
  assert.equal(cell.gameText, "aaaa");
  assert.equal(cell.suffix, "+1 map");
  // The whole session: 12 gains of 63 on aaaa, then 3,000 on bbbb.
  assert.equal(gained, "3.76K");
  // Map aaaa's own EXP/hr: 12 gains of 63 in two minutes.
  assert.equal(expRate, "22.7K");
});

// Every past session uses one layout: one from before maps were tracked reads as an unknown map
// with its own EXP gained and EXP/hr. Fails if it gets a separate table or loses its date line.
test("sessions from before map tracking use the same layout", () => {
  const old = { startedAtMs: T0 - 86_400_000, endedAtMs: T0 - 86_000_000, activeMs: 400_000, expGained: 9_000, expLost: 0, startLevel: 41, endLevel: 41 };
  const { send, sent } = run();
  send({ type: "start", saved: { version: 1, history: [old], current: null }, atMs: T0, capture: "capturing", readers: MESOS });
  send(map("aaaa", T0));
  send(exp(230_000, T0));
  for (let s = 1; s <= 60; s++) {
    send({ type: "tick", atMs: T0 + s * 1000 });
    if (s % 10 === 0) send(exp(230_000 + s * 6, T0 + s * 1000));
  }
  send(action("end", T0 + 60_000));
  const [table, ...rest] = pastSessions(sent);
  assert.equal(table.rows.length, 2, "one table for both");
  assert.ok(!rest.some((node) => node.kind === "table" || node.kind === "text"));
  assert.deepEqual(table.rows[1], ["Unknown map", "9K", "81K"]);
  assert.match(table.details[1][0], /^\w{3} \d+ · 6m$/);
  assert.equal(table.details[1][1], "Lv 41");
});

// A bad value in a save must never stop the tracker. Fails if a malformed map entry or meso
// ledger throws, or the good parts of the session are lost with it.
test("a malformed map entry in a save is dropped, the session kept", () => {
  const good = { k: "aaaa", p: 60_000, x: 300, m: 50, r: 50, l: 60_000, s: 42, e: 42 };
  const past = {
    startedAtMs: T0 - 600_000,
    endedAtMs: T0 - 300_000,
    activeMs: 60_000,
    expGained: 300,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    maps: [good, { k: 7, p: "x" }, null, { ...good, k: "bbbb", m: -5 }],
    lg: { b: 1_000_000, ba: 0, bp: 0, ob: 1_000_000, n: 2, pu: 50_000, so: 0, sp: 0, ct: 0, ho: "x", pt: 600_000 },
  };
  const { send, sent } = run();
  send({ type: "start", saved: { version: 1, history: [past], current: null }, atMs: T0, capture: "capturing", readers: MESOS });
  const row = pastSessions(sent)[0].rows[0];
  assert.equal(row[0].gameText, "aaaa");
  assert.equal(row[0].suffix, undefined, "the bad entries are gone");
  // The damaged ledger is dropped, so the Loot tab shows what the session's notices counted.
  assert.deepEqual(lootTable(sent).rows[0].slice(1), ["≈3K", "≈50"]);
});

/** A session saved by MesoVibe 0.7.0: half an hour on "aaaa", its loot counted from the pickup notices. */
const SAVED_BY_070 = {
  version: 1,
  history: [
    {
      startedAtMs: T0 - 86_400_000,
      endedAtMs: T0 - 84_600_000,
      activeMs: 1_800_000,
      expGained: 41_667,
      expLost: 0,
      startLevel: 41,
      endLevel: 42,
      maps: [{ k: "aaaa", p: 1_800_000, x: 41_667, m: 57_800, r: 54_000, l: 1_620_000, s: 41, e: 42, i: [{ k: "#4000028", c: 30 }], rv: 900 }],
    },
  ],
  current: null,
  allTime: { mesos: 7, playedMs: 600_000, items: [{ k: "#4000028", c: 3 }] },
  catalog: { "#4000028": { n: "Tablecloth", t: "Etc", v: 34 } },
};

// Sessions saved by 0.7.0 still load and show what their pickup notices counted, marked "≈" and
// said so, and All time keeps that apart from anything measured from meso totals. Fails if an old
// session is dropped, its history changes, or its estimate joins a measured rate.
test("sessions saved by 0.7.0 load, with their notice figures as saved", () => {
  const { sent } = play({ seconds: 0, saved: structuredClone(SAVED_BY_070) });
  const table = lootTable(sent);
  assert.deepEqual(table.columns, ["Map", "Mesos/hr", "Profit"]);
  // 54,000 mesos over 27 minutes of notices in view; 57,800 mesos and 30 Tablecloths at 34.
  assert.deepEqual(table.rows[0].slice(1), ["≈120K", "≈58.8K"]);
  assert.ok(table.details[0].includes("Estimated from pickup notices"));
  assert.equal(pastSessions(sent)[0].rows[0][1], "41.7K", "the Session view lists it too");
  // 7 mesos and 3 Tablecloths from sessions past the 500 kept, and this session's 58,820.
  assert.deepEqual(allTimeFigures(sent), { Played: "40m", "From pickup notices": "≈58.9K" });

  // A session measured from meso totals adds up apart from it.
  const played = play({ seconds: 120, saved: structuredClone(SAVED_BY_070) });
  const t = twoReadings(played);
  const figures = allTimeFigures(played.sent);
  assert.equal(figures["From pickup notices"], "≈58.9K");
  assert.equal(figures["Mesos/hr"], "300K", "only meso totals make the rate");

  // Saved again by this build, 0.7.0 still reads the old session as it was.
  played.send(action("end", t + 1_000));
  const back = load070(JSON.parse(JSON.stringify(last(played.sent, "save").data)));
  assert.equal(back.history.length, 2);
  assert.equal(back.history[1].maps[0].m, 57_800);
  assert.deepEqual(back.history[1].maps[0].i, [{ k: "#4000028", c: 30 }]);
  assert.equal(back.history[0].maps[0].m, 0, "a session since keeps 0 for the notices' mesos");
});

// Sessions past the 500 kept go into All time, never lost: what their meso ledgers measured and
// what older ones' notices counted. Fails if a dropped session's mesos vanish from the save or
// from All time after a restart.
test("a session past the 500 kept adds its mesos to All time", () => {
  const session = (i) => ({
    startedAtMs: T0 - (i + 1) * 3_600_000,
    endedAtMs: T0 - (i + 1) * 3_600_000 + 600_000,
    activeMs: 600_000,
    expGained: 100,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    maps: [{ k: "aaaa", p: 600_000, x: 100, m: 0, r: 0, l: 0, s: 42, e: 42 }],
  });
  const history = Array.from({ length: 500 }, (_, i) => session(i));
  // The oldest, the one End pushes out, has a meso ledger and items its notices counted.
  history[499].maps[0] = { ...history[499].maps[0], m: 7, r: 7, l: 600_000, i: [{ k: "#4000028", c: 3 }] };
  history[499].lg = { b: 1_050_000, ba: 0, bp: 600_000, ob: 1_000_000, n: 3, pu: 40_000, so: 10_000, sp: 0, ct: 0, ho: [], pt: 600_000 };
  const catalog = { "#4000028": { n: "Tablecloth", t: "Etc", v: 34 } };
  const plugin = run();
  plugin.send({ type: "start", saved: { version: 1, history, current: null, catalog }, atMs: T0, capture: "capturing", readers: MESOS });
  plugin.send(map("aaaa", T0));
  plugin.send(exp(230_000, T0));
  const now = grind(plugin, T0, 20, 230_000);
  plugin.send(action("end", now));
  const saved = last(plugin.sent, "save").data;
  assert.equal(saved.history.length, 500);
  assert.deepEqual(saved.allTime, { mesos: 7, playedMs: 600_000, items: [{ k: "#4000028", c: 3 }], picked: 40_000, sold: 10_000, measuredMs: 600_000 });
  const expected = { "Mesos picked up": "40K", "Mesos/hr": "240K", "Loot sold": "10K", "Measured for": "10m", "From pickup notices": "≈109" };
  const shown = (sent) => Object.fromEntries(Object.entries(allTimeFigures(sent)).filter(([label]) => label !== "Played"));
  assert.deepEqual(shown(plugin.sent), expected);
  const again = run();
  again.send({ type: "start", saved, atMs: now + 10_000, capture: "capturing", readers: MESOS });
  assert.deepEqual(shown(again.sent), expected);
});

// A save edited by hand, or damaged, can hold item keys no pickup makes ("constructor",
// "__proto__") or a price range that runs high to low: they're dropped, so nothing reads a value
// off the object's prototype or a bad range. Fails if such a key or range prices anything.
test("malformed items and catalog entries in a save are dropped", () => {
  const past = {
    startedAtMs: T0 - 600_000,
    endedAtMs: T0 - 300_000,
    activeMs: 60_000,
    expGained: 300,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    maps: [
      {
        k: "aaaa",
        p: 60_000,
        x: 300,
        m: 50,
        r: 50,
        l: 60_000,
        s: 42,
        e: 42,
        i: [{ k: "constructor", c: 2 }, { k: "#1", c: 2 }, { k: 5, c: 1 }, { k: "#2", c: -1 }, { k: "=Blue Jangoon…", c: 1 }, { k: "=Hat Accuracy Scroll:…", c: 1 }],
      },
    ],
  };
  const catalog = JSON.parse(
    '{"#1": {"n": "Snail Shell", "t": "Etc", "v": 1}, "#2": {"n": 7}, "__proto__": {"n": "x", "t": "Etc", "v": 9},' +
      ' "=Blue Jangoon…": {"n": "Blue Jangoon…", "t": "Equip", "v": null, "r": [4200, 7000]},' +
      ' "=Hat Accuracy Scroll:…": {"n": "Hat Accuracy Scroll:…", "t": "Use", "v": null, "r": [7000, 4200]}}',
  );
  const { send, sent } = run();
  send({ type: "start", saved: { version: 1, history: [past], current: null, catalog }, atMs: T0, capture: "capturing", readers: MESOS });
  // 50 mesos, two Snail Shells at 1 and a Blue Jangoon… at the lower of its prices.
  assert.equal(allTimeFigures(sent)["From pickup notices"], "≈4.25K");
});

// The Loot tab's Past sessions says where sessions come from until one has loot, and has its own
// "Show more" past 20, apart from the Session view's. Fails if the empty line is missing, the
// button doesn't show more or turn into "Show fewer", or one toggle moves the other list.
test("the Loot tab's Past sessions is empty before loot tracking, and has its own Show more", () => {
  const before = { startedAtMs: T0 - 900_000, endedAtMs: T0 - 600_000, activeMs: 300_000, expGained: 900, expLost: 0, startLevel: 41, endLevel: 41 };
  const fresh = run();
  fresh.send({ type: "start", saved: { version: 1, history: [before], current: null }, atMs: T0, capture: "capturing", readers: MESOS });
  assert.equal(pastSessions(fresh.sent)[0].kind, "table", "the Session view lists it");
  assert.deepEqual(section(fresh.sent, "Past sessions", "loot").children, [
    { kind: "empty", text: "Sessions show up here when you end them or close MesoVibe." },
  ]);

  const tracked = (i) => ({
    startedAtMs: T0 - (i + 1) * 3_600_000,
    endedAtMs: T0 - (i + 1) * 3_600_000 + 600_000,
    activeMs: 600_000,
    expGained: 100,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    maps: [{ k: "aaaa", p: 600_000, x: 100, m: 0, r: 0, l: 0, s: 42, e: 42 }],
  });
  const { send, sent } = run();
  send({ type: "start", saved: { version: 1, history: Array.from({ length: 25 }, (_, i) => tracked(i)), current: null }, atMs: T0, capture: "capturing", readers: MESOS });
  const lootPast = () => section(sent, "Past sessions", "loot").children;
  const lootRows = () => lootPast().find((n) => n.kind === "table").rows.length;
  const lootButton = () => lootPast().find((n) => n.kind === "button");
  const sessionRows = () => pastSessions(sent).find((n) => n.kind === "table").rows.length;
  const sessionButton = () => pastSessions(sent).find((n) => n.action === "toggle-history");
  assert.equal(lootRows(), 20);
  assert.deepEqual(lootPast().find((n) => n.kind === "table").rows[0].slice(1), ["—", "—"], "nothing read, nothing sold");
  assert.deepEqual([lootButton().label, lootButton().action], ["Show more", "toggle-loot-history"]);
  send(action("toggle-loot-history", T0 + 1_000));
  assert.equal(lootRows(), 25);
  assert.equal(lootButton().label, "Show fewer");
  assert.equal(sessionRows(), 20, "the Session view's list stays as it was");
  assert.equal(sessionButton().label, "Show more");
  send(action("toggle-history", T0 + 2_000));
  assert.equal(sessionRows(), 25);
  assert.equal(lootRows(), 25, "the Loot tab's list stays as it was");
  send(action("toggle-loot-history", T0 + 3_000));
  assert.equal(lootRows(), 20);
  assert.equal(lootButton().label, "Show more");
  assert.equal(sessionRows(), 25);
});

// The meso ledger's arithmetic, driven directly with trusted totals.
const grinding = (playMs, key = "aaaa") => ({ playMs, map: key, away: false });
const ledgerOf = (fields) => fields.lg;

// A later rise on one grinding map is mesos picked up, for that map's rate. Fails if the opening
// total counts, the map's mesos/hr is wrong, or a rate shows under a minute after the last reading.
test("a rise between two readings on one grinding map is mesos picked up, and that map's rate", () => {
  const fields = {};
  assert.deepEqual(loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0)), { kind: "opening", total: 1_000_000 });
  assert.equal(loot.mesosRate(ledgerOf(fields)), null, "one reading has no rate");
  assert.deepEqual(loot.addReading(fields, 1_050_000, "inventory", T0 + 30 * MIN, grinding(30 * MIN)), { kind: "picked", amount: 50_000 });
  assert.equal(loot.ledgerProfit(ledgerOf(fields)), 50_000);
  assert.equal(loot.mesosRate(ledgerOf(fields)), 100_000);
  const entry = fields.maps.find((m) => m.k === "aaaa");
  assert.deepEqual([entry.fm, entry.ft], [50_000, 30 * MIN]);
  assert.equal(loot.mapRates(entry, 10_000).mesos, 100_000);
  // Mesos come in lumps: no rate until a minute of play lies between readings.
  const quick = {};
  loot.addReading(quick, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(quick, 1_001_000, "inventory", T0 + MIN - 1, grinding(MIN - 1));
  assert.deepEqual([loot.mesosRate(ledgerOf(quick)), loot.profitRate(ledgerOf(quick))], [null, null], "under a minute");
  loot.addReading(quick, 1_001_000, "inventory", T0 + MIN, grinding(MIN));
  assert.deepEqual([loot.mesosRate(ledgerOf(quick)), loot.profitRate(ledgerOf(quick))], [60_000, 60_000]);
});

// A map's mesos/hr comes only from a stretch between two meso totals read on it; a map without one
// has none, whatever its pickup notices counted. Fails if a map without a ledger stretch gets a
// rate, from the notices or anything else.
test("a map without a stretch between two meso totals has no mesos/hr", () => {
  const noticed = { k: "aaaa", p: 30 * MIN, x: 9_000, m: 54_000, r: 54_000, l: 30 * MIN, s: 42, e: 42 };
  assert.equal(loot.mapRates(noticed, 10_000).mesos, null);
  assert.equal(loot.mapRates({ ...noticed, fm: 20_000, ft: 30 * MIN }, 10_000).mesos, 40_000);
  assert.equal(loot.mapRates({ ...noticed, fm: 1_000, ft: 59_999 }, 10_000).mesos, null, "under a minute of it");
  // Played between readings on two maps, neither gets the rise.
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(fields, 1_030_000, "inventory", T0 + 20 * MIN, { playMs: 20 * MIN, map: undefined, away: false });
  assert.equal(fields.maps, undefined);
  assert.equal(ledgerOf(fields).pu, 30_000, "still mesos picked up for the session");
});

// A sale read in a shop is what shops paid; a fall is spent and never taken from profit, so
// restocking potions doesn't make a map look worse. Fails if a purchase lowers profit, or a sale is
// counted as mesos picked up.
test("shop sales count as loot sold, and spending shows apart", () => {
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  assert.deepEqual(loot.addReading(fields, 1_188_450, "shop", T0 + 20 * MIN, { playMs: 20 * MIN, map: undefined, away: true }), { kind: "sold", amount: 188_450 });
  assert.deepEqual(loot.addReading(fields, 1_092_450, "shop", T0 + 21 * MIN, { playMs: 21 * MIN, map: undefined, away: true }), { kind: "spent", amount: 96_000 });
  const ledger = ledgerOf(fields);
  assert.deepEqual([ledger.pu, ledger.so, ledger.sp], [0, 188_450, 96_000]);
  assert.equal(loot.ledgerProfit(ledger), 188_450);
  assert.equal(fields.maps, undefined, "a stretch away from the grind is no map's rate");
});

// Mesos taken out of storage while its window is open are a transfer, not loot. Fails if a
// withdrawal read in storage counts as mesos picked up, sold or held.
test("a withdrawal read in the storage window counts for nothing", () => {
  const plugin = play();
  const t = twoReadings(plugin);
  const before = lootFigures(plugin.sent);
  plugin.send(total(1_050_000, t + 1000, "storage"));
  plugin.send(total(1_250_000, t + 2000, "storage"));
  plugin.send({ type: "tick", atMs: t + 3000 });
  assert.deepEqual(lootFigures(plugin.sent), before);
  // The next inventory total is measured from the balance after the withdrawal.
  const later = grind(plugin, t + 3000, 120);
  plugin.send(total(1_260_000, later));
  assert.equal(lootFigures(plugin.sent)["Mesos picked up"], "60,000");
});

// A storage visit's first total counts by the usual rules (nothing has moved yet); the ones after
// it while the window stays open move the balance. Fails if a transfer counts or a visit's first change is lost.
test("only the totals after a storage visit's first move the balance without counting", () => {
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  const town = (playMs) => ({ playMs, map: undefined, away: true });
  assert.deepEqual(loot.addReading(fields, 1_000_500, "storage", T0 + 10 * MIN, town(10 * MIN)), { kind: "sold", amount: 500 });
  assert.deepEqual(loot.addReading(fields, 1_250_500, "storage", T0 + 11 * MIN, town(11 * MIN)), { kind: "moved", amount: 250_000 });
  assert.deepEqual(loot.addReading(fields, 1_200_500, "storage", T0 + 12 * MIN, town(12 * MIN)), { kind: "moved", amount: -50_000 });
  assert.deepEqual(loot.addReading(fields, 1_230_500, "inventory", T0 + 30 * MIN, grinding(30 * MIN)), { kind: "picked", amount: 30_000 });
  const ledger = ledgerOf(fields);
  assert.deepEqual([ledger.pu, ledger.so, ledger.sp, ledger.ho.length], [30_000, 500, 0, 0]);
  // A storage total after another window's starts a new visit.
  assert.deepEqual(loot.addReading(fields, 1_230_600, "storage", T0 + 40 * MIN, town(40 * MIN)), { kind: "sold", amount: 100 });
  assert.equal(loot.validLedger(JSON.parse(JSON.stringify(ledger))).ls, "storage", "where the last total came from is saved");
});

// A storage visit after grinding counts what was picked up since the last one. Fails if the mesos
// picked up between two storage visits are taken for a transfer.
test("a second storage visit after grinding counts what was picked up between", () => {
  const plugin = play();
  const t = twoReadings(plugin);
  plugin.send(map("cccc", t));
  plugin.send(total(1_050_000, t + 1000, "storage"));
  plugin.send(total(1_000_000, t + 2000, "storage"));
  plugin.send(map("aaaa", t + 3000));
  const later = grind(plugin, t + 3000, 600, 240_000);
  plugin.send(map("cccc", later));
  plugin.send(total(1_040_000, later + 1000, "storage"));
  assert.equal(lootFigures(plugin.sent)["Mesos picked up"], "90,000");
});

// Storage moves count for nothing on a map whose name can't be read, though EXP was gained on such
// maps. Fails if a deposit with the window open counts as spent, or a withdrawal as picked up.
test("storage moves count for nothing where the map name can't be read", () => {
  const plugin = play({ key: null });
  const t = twoReadings(plugin);
  plugin.send(total(1_050_000, t + 1000, "storage"));
  plugin.send(total(990_000, t + 2000, "storage"));
  plugin.send(total(1_010_000, t + 3000, "storage"));
  plugin.send({ type: "tick", atMs: t + 9000 });
  const ledger = last(plugin.sent, "save").data.current.lg;
  assert.deepEqual([ledger.pu, ledger.sp, ledger.ho.length], [50_000, 0, 0]);
});

// After grinding, storage opened again in the close-out counts what was picked up since. Fails if
// it's taken for a transfer because the session's last total was read in storage.
test("a storage total after End counts what was picked up since the last storage visit", () => {
  const plugin = play();
  plugin.send(total(1_000_000, plugin.now));
  let t = grind(plugin, plugin.now, 120);
  plugin.send(total(1_010_000, t, "storage"));
  t = grind(plugin, t, 120, 240_000);
  plugin.send(action("end", t));
  plugin.send({ type: "tick", atMs: t + 11_000 });
  plugin.send(total(1_040_000, t + 20_000, "storage"));
  plugin.send({ type: "tick", atMs: t + 30_000 });
  assert.equal(last(plugin.sent, "save").data.history[0].lg.so, 30_000);
});

// After End, storage moves during the close-out count for nothing, and a sale after them counts.
// Fails if a withdrawal in the close-out counts as sold.
test("a storage withdrawal during the close-out counts for nothing", () => {
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(fields, 1_030_000, "inventory", T0 + 20 * MIN, grinding(20 * MIN));
  loot.waitForCloseOut(fields, T0 + 21 * MIN, true);
  assert.deepEqual(loot.closeOut(fields, 1_030_000, "storage", T0 + 22 * MIN), { kind: "same" });
  assert.deepEqual(loot.closeOut(fields, 1_330_000, "storage", T0 + 23 * MIN), { kind: "moved", amount: 300_000 });
  assert.deepEqual(loot.closeOut(fields, 1_340_000, "shop", T0 + 25 * MIN), { kind: "sold", amount: 10_000 });
  assert.deepEqual([ledgerOf(fields).so, ledgerOf(fields).ho.length], [10_000, 0]);
});

// Without a shop reading, a rise across a pause is what a shop paid, not mesos picked up, so it
// never raises a map's mesos/hr. Fails if it counts as picked up or goes to a map.
test("a rise across a pause counts as sold", () => {
  const fields = {};
  loot.addReading(fields, 500_000, "inventory", T0, grinding(0));
  assert.deepEqual(loot.addReading(fields, 540_000, "inventory", T0 + 40 * MIN, { playMs: 25 * MIN, map: "aaaa", away: true }), { kind: "sold", amount: 40_000 });
  assert.equal(ledgerOf(fields).pu, 0);
  assert.equal(fields.maps, undefined);
  assert.equal(ledgerOf(fields).pt, 25 * MIN, "the rate's clock leaves the pause out");
});

// A potion run: potions bought in a town's shop, a walk back through a map with no kills, then
// more grinding. Fails if time in a town turns the mesos picked up after it into loot sold.
test("mesos picked up after a potion run through town still count as picked up", () => {
  const played = play({ seconds: 120 });
  let t = twoReadings(played);
  played.send(map("cccc", t));
  for (let s = 1; s <= 40; s++) {
    played.send({ type: "tick", atMs: t + s * 1000 });
    if (s === 20) played.send(total(1_050_000, t + s * 1000, "shop"));
    if (s === 25) played.send(total(1_020_000, t + s * 1000, "shop"));
  }
  t += 40_000;
  played.send(map("dddd", t));
  for (let s = 1; s <= 25; s++) played.send({ type: "tick", atMs: t + s * 1000 });
  t += 25_000;
  played.send(map("aaaa", t));
  t = grind(played, t, 600, 240_000);
  played.send(total(1_070_000, t));
  assert.deepEqual(lootFigures(played.sent), { "Mesos picked up": "100,000", "Loot sold": "—", Spent: "30,000" });
});

// A rise outside a shop above both 50,000 mesos and 3,000,000 an hour since the last reading is held
// out of profit; counting it adds it, and Undo holds it again. Fails if a bound moves, a held rise
// counts before the player says so, or Undo loses it.
test("a rise too big to be farming is held until counted", () => {
  const bounds = [
    [50_000, 1_000, "picked"],
    [50_001, 1_000, "held"],
    [3_000_000, 60 * MIN, "picked"],
    [3_000_001, 60 * MIN, "held"],
  ];
  for (const [rise, after, kind] of bounds) {
    const fields = {};
    loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
    const change = loot.addReading(fields, 1_000_000 + rise, "inventory", T0 + after, grinding(after));
    assert.equal(change.kind, kind, `${rise} after ${after} ms`);
  }
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(fields, 6_000_000, "inventory", T0 + 10 * MIN, grinding(10 * MIN));
  const ledger = ledgerOf(fields);
  assert.equal(loot.ledgerProfit(ledger), 0);
  assert.deepEqual(ledger.ho, [[5_000_000, T0 + 10 * MIN]]);
  const counted = loot.countHeld(fields, 0);
  assert.equal(loot.ledgerProfit(ledger), 5_000_000);
  loot.uncountHeld(fields, counted);
  assert.equal(loot.ledgerProfit(ledger), 0);
  assert.deepEqual(ledger.ho, [[5_000_000, T0 + 10 * MIN]]);
  assert.equal(
    loot.addReading(fields, 9_000_000, "shop", T0 + 11 * MIN, { playMs: 11 * MIN, map: undefined, away: true }).kind,
    "sold",
    "a rise read in a shop is never held",
  );
});

// When the EXP side starts a new baseline (another character), the next total opens a new segment:
// the jump between characters counts as nothing, and what was earned stays. Fails if the jump
// counts or the earlier mesos are lost.
test("a new segment doesn't count the jump between characters", () => {
  const fields = {};
  loot.addReading(fields, 2_000_000, "inventory", T0, grinding(0));
  loot.addReading(fields, 2_030_000, "inventory", T0 + 10 * MIN, grinding(10 * MIN));
  loot.restartSegment(fields);
  assert.deepEqual(loot.addReading(fields, 40_000_000, "inventory", T0 + 12 * MIN, grinding(12 * MIN)), { kind: "opening", total: 40_000_000 });
  loot.addReading(fields, 40_010_000, "inventory", T0 + 22 * MIN, grinding(22 * MIN));
  const ledger = ledgerOf(fields);
  assert.equal(ledger.pu, 40_000);
  assert.equal(ledger.pt, 20 * MIN, "the gap between segments isn't on the clock");
});

// A total read in the five minutes before the first EXP gain opens the session, so the player isn't
// asked again; an older one doesn't. Fails if the window is off by a millisecond either way.
test("a total from the five minutes before the first kill opens the session", () => {
  const fresh = {};
  assert.equal(loot.openWith(fresh, 1_204_330, T0 - 5 * MIN, T0), true);
  assert.equal(ledgerOf(fresh).ob, 1_204_330);
  assert.equal(loot.addReading(fresh, 1_224_330, "inventory", T0 + 10 * MIN, grinding(10 * MIN)).kind, "picked");
  const stale = {};
  assert.equal(loot.openWith(stale, 1_204_330, T0 - 5 * MIN - 1, T0), false);
  assert.equal(stale.lg, undefined);
});

// After End, selling takes several readings, so every total in the next 30 minutes counts: in a
// shop a rise is sold and a fall spent, elsewhere a rise is sold unless too big for farming. The
// rate's clock doesn't move. Fails if the close-out ends at its first total, a total after it lands
// in the old session, or a sale moves the clock.
test("the close-out counts every total until its time runs out", () => {
  const ended = () => {
    const fields = {};
    loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
    loot.addReading(fields, 1_100_000, "inventory", T0 + 30 * MIN, grinding(30 * MIN));
    loot.waitForCloseOut(fields, T0 + 31 * MIN, true);
    return fields;
  };
  const shop = ended();
  assert.deepEqual(loot.closeOut(shop, 1_150_000, "shop", T0 + 40 * MIN), { kind: "sold", amount: 50_000 });
  assert.deepEqual(loot.closeOut(shop, 1_149_250, "shop", T0 + 41 * MIN), { kind: "spent", amount: 750 });
  assert.deepEqual(loot.closeOut(shop, 6_149_250, "shop", T0 + 42 * MIN), { kind: "sold", amount: 5_000_000 }, "a sale is never held");
  const ledger = ledgerOf(shop);
  assert.deepEqual([ledger.so, ledger.sp, ledger.pt], [5_050_000, 750, 30 * MIN]);
  assert.equal(ledger.co, T0 + 61 * MIN, "still open");
  assert.equal(loot.closeOut(shop, 7_000_000, "shop", T0 + 61 * MIN + 1), null);
  assert.equal(ledger.co, undefined, "its time ran out");
  assert.equal(ledger.so, 5_050_000);

  const inventory = ended();
  assert.deepEqual(loot.closeOut(inventory, 1_140_000, "inventory", T0 + 40 * MIN), { kind: "sold", amount: 40_000 });
  assert.equal(loot.closeOut(inventory, 9_000_000, "inventory", T0 + 41 * MIN).kind, "held");

  const stopped = ended();
  loot.stopCloseOut(stopped);
  assert.equal(loot.closeOut(stopped, 1_228_400, "inventory", T0 + 40 * MIN), null);

  // A session that read no total gets a close-out where totals can be read: its first total sets
  // the balance, and a close-out that read none leaves no ledger behind.
  const unread = {};
  loot.waitForCloseOut(unread, T0, true);
  assert.deepEqual(loot.closeOut(unread, 500_000, "shop", T0 + MIN), { kind: "opening", total: 500_000 });
  assert.deepEqual(loot.closeOut(unread, 520_000, "shop", T0 + 2 * MIN), { kind: "sold", amount: 20_000 });
  const nowhere = {};
  loot.waitForCloseOut(nowhere, T0, false);
  assert.equal(nowhere.lg, undefined, "no close-out where nothing reads the total");
  const empty = {};
  loot.waitForCloseOut(empty, T0, true);
  loot.stopCloseOut(empty);
  assert.equal(empty.lg, undefined);
});

// Undo of End adds the mesos counted since and keeps the clock, and a balance read on the other
// session's clock starts a new segment. Fails if the mesos are lost, time is added, or the next
// total is measured against the wrong balance.
test("Undo merges the ledger without mixing clocks", () => {
  const into = {};
  loot.addReading(into, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(into, 1_100_000, "inventory", T0 + 30 * MIN, grinding(30 * MIN));
  loot.waitForCloseOut(into, T0 + 31 * MIN, true);
  const since = {};
  loot.addReading(since, 1_120_000, "inventory", T0 + 31 * MIN, grinding(0));
  loot.addReading(since, 1_125_000, "inventory", T0 + 32 * MIN, grinding(MIN));
  loot.mergeLoot(into, since);
  const ledger = ledgerOf(into);
  assert.equal(loot.ledgerProfit(ledger), 105_000);
  assert.equal(ledger.pt, 30 * MIN);
  assert.equal(ledger.co, undefined, "the session runs again");
  assert.equal(loot.addReading(into, 1_200_000, "inventory", T0 + 40 * MIN, grinding(39 * MIN)).kind, "opening");
});

// A malformed saved ledger is dropped with the session kept, and a good one survives a save and
// load. Fails if a bad value throws or a good ledger loses a field.
test("a saved ledger is kept when valid and dropped when not", () => {
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(fields, 6_000_000, "inventory", T0 + 10 * MIN, grinding(10 * MIN));
  loot.waitForCloseOut(fields, T0 + 11 * MIN, true);
  const saved = JSON.parse(JSON.stringify(fields.lg));
  assert.deepEqual(loot.validLedger(saved), fields.lg);
  for (const bad of [null, 5, [], { ...saved, pu: -1 }, { ...saved, b: "a lot" }, { ...saved, ho: 3 }]) {
    assert.equal(loot.validLedger(bad), undefined, JSON.stringify(bad));
  }
  assert.deepEqual(loot.validLedger({ ...saved, ho: [[5, T0], [-1, T0], "x"] }).ho, [[5, T0]]);
  const maps = loot.validMaps([{ k: "aaaa", p: 1, x: 0, m: 0, r: 0, l: 0, s: null, e: null, fm: 50_000, ft: 30 * MIN }]);
  assert.deepEqual([maps[0].fm, maps[0].ft], [50_000, 30 * MIN]);
});

// The Loot section (loot-view.ts), drawn from a ledger directly.
const VIEW = { readers: MESOS, atMs: T0 + 34 * MIN, started: true, paused: false, potsPerHour: null };
/** A session with 1,000,000 then 1,050,000 read 30 minutes of play apart on one map. */
function measured() {
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(fields, 1_050_000, "inventory", T0 + 30 * MIN, grinding(30 * MIN));
  return fields;
}
const lines = (node) => node.children.filter((n) => n.kind === "text" || n.kind === "partial");
const SELL = "Sell your loot at a shop to see what it's worth. Sales up to 30 minutes after End count for this session.";

// Before any total, Loot's headline has no figure and the line asks for the inventory, from a host
// that reads the meso total only. Fails if a rate shows without two totals, the ask is missing, or
// a player is asked for a reading nothing makes.
test("Loot asks for the inventory before any total, only from a host that reads it", () => {
  const node = lootView.lootSection({ maps: [] }, VIEW);
  assert.deepEqual(node.children[0], { kind: "stat", label: "Mesos/hr", value: null });
  assert.deepEqual(lines(node), [
    { kind: "text", text: "Open your inventory to start counting mesos.", variant: "muted" },
    { kind: "text", text: SELL, variant: "muted" },
  ]);
  assert.deepEqual(figuresOf(node), { "Loot sold": "—" });
  assert.equal(lootView.lootSection({ maps: [] }, { ...VIEW, readers: PLAIN }), null);
  assert.equal(lootView.lootSection(measured(), { ...VIEW, started: false }), null, "nothing before the first kill");
});

// One total gives the balance and asks for another later; two make the headline Mesos/hr, with how
// long ago mesos were read. Fails if one reading shows a rate, the balance is missing, or the age
// line is wrong.
test("Loot gives the balance after one total, and Mesos/hr after two", () => {
  const one = {};
  loot.addReading(one, 5_170_316, "inventory", T0, grinding(0));
  const first = lootView.lootSection(one, { ...VIEW, atMs: T0 + MIN });
  assert.equal(first.children[0].value, null);
  assert.equal(first.children[1].text, "Mesos read: 5,170,316. Open your inventory again later to see mesos/hr.");
  assert.deepEqual(figuresOf(first), { "Loot sold": "—" });
  const two = lootView.lootSection(measured(), VIEW);
  assert.deepEqual(two.children.slice(0, 2), [
    { kind: "stat", label: "Mesos/hr", value: "100K", detail: "100,000 mesos picked up per hour" },
    { kind: "text", text: "Mesos read 4 min ago", variant: "muted" },
  ]);
  assert.deepEqual(figuresOf(two), { "Mesos picked up": "50,000", "Loot sold": "—" });
});

// A balance read more than 15 minutes ago mutes the headline and asks for a fresh reading; a paused
// session mutes it and says so; learning the digits and an unreadable size each have their line.
// Fails if a stale or paused figure looks current, or a state passes silently.
test("Loot's line follows the state: stale, paused, learning, can't read", () => {
  const stale = lootView.lootSection(measured(), { ...VIEW, atMs: T0 + 52 * MIN });
  assert.equal(stale.children[0].variant, "muted");
  assert.equal(stale.children[1].text, "Mesos read 22 min ago. Open your inventory to update.");
  const paused = lootView.lootSection(measured(), { ...VIEW, paused: true });
  assert.equal(paused.children[0].variant, "muted");
  assert.equal(paused.children[1].text, "Session paused");
  const learning = lootView.lootSection({ maps: [] }, { ...VIEW, readers: { ...PLAIN, mesos: "learning" } });
  assert.equal(learning.children[1].text, "Learning to read your mesos. Keep the inventory open a few seconds.");
  const unsupported = lootView.lootSection({ maps: [] }, { ...VIEW, readers: { ...PLAIN, mesos: "unsupported" } });
  assert.deepEqual(lines(unsupported), [{ kind: "partial", text: "MesoVibe can't read mesos at this game size." }], "and no ask to sell");
});

// Loot sold stays "—" until a shop pays, and Profit (mesos picked up plus sales) shows only after
// that; Spent shows apart and never lowers Profit. Fails if Profit shows before a sale, a sale is
// missing from it, or spending is taken from it.
test("Loot sold is — until a shop pays, and Profit shows only after a sale", () => {
  const fields = measured();
  assert.equal(figuresOf(lootView.lootSection(fields, VIEW)).Profit, undefined);
  assert.ok(lines(lootView.lootSection(fields, VIEW)).some((n) => n.text === SELL));
  loot.addReading(fields, 1_150_000, "shop", T0 + 33 * MIN, { playMs: 33 * MIN, map: undefined, away: true });
  loot.addReading(fields, 1_120_000, "shop", T0 + 34 * MIN, { playMs: 34 * MIN, map: undefined, away: true });
  const sold = lootView.lootSection(fields, VIEW);
  assert.deepEqual(figuresOf(sold), { "Mesos picked up": "50,000", "Loot sold": "100,000", Profit: "150,000", Spent: "30,000" });
  assert.ok(!lines(sold).some((n) => n.text === SELL), "no ask to sell once sold");
});

// Once potions are counted, a line gives the net after them: from Mesos/hr before a sale, from
// Profit/hr after. Fails if the net uses the wrong rate or subtracts the wrong cost.
test("the net after potions follows Mesos/hr, then Profit/hr once loot is sold", () => {
  const fields = measured();
  const net = (node) => lines(node).find((n) => n.text.includes("on potions"))?.text;
  assert.equal(net(lootView.lootSection(fields, { ...VIEW, potsPerHour: 23_500 })), "76.5K/hr after 23.5K/hr on potions");
  assert.equal(net(lootView.lootSection(fields, VIEW)), undefined, "none without potions counted");
  loot.addReading(fields, 1_100_000, "shop", T0 + 31 * MIN, { playMs: 30 * MIN, map: undefined, away: true });
  // 50,000 picked up and 50,000 sold over 30 minutes: 200K/hr.
  assert.equal(net(lootView.lootSection(fields, { ...VIEW, potsPerHour: 23_500 })), "177K/hr after 23.5K/hr on potions");
});

// A held rise shows as a calm line with a "Count it" button that names the rise for screen readers,
// newest first and at most three. Fails if a held rise passes silently, the button can't say what
// it counts, or the list grows without end.
test("held rises list with Count it, newest first, at most three", () => {
  const fields = measured();
  for (let i = 1; i <= 4; i++) {
    loot.addReading(fields, 1_050_000 + i * 5_000_000, "inventory", T0 + (30 + i) * MIN, grinding((30 + i) * MIN));
  }
  const node = lootView.lootSection(fields, VIEW);
  const held = node.children.filter((n) => n.kind === "partial");
  const buttons = node.children.filter((n) => n.kind === "button");
  assert.equal(held.length, 3);
  assert.equal(held[0].text, "+5M not counted: too big a jump for farming (a trade or a misread)");
  assert.deepEqual(buttons[0], {
    kind: "button",
    label: "Count it",
    action: `count-held:${T0 + 34 * MIN}`,
    style: "secondary",
    spoken: "Count the 5M rise in Profit",
  });
});

// Two meso totals give Loot its Mesos/hr over the play time between them, the first says it was
// read once, and the mini-HUD gets the same figure. Fails if totals don't reach the ledger, the
// notice repeats, or the strip and Loot disagree.
test("meso totals give Loot its Mesos/hr, and the first one says it was read", () => {
  const played = play({ seconds: 120 });
  played.send(total(1_000_000, played.now));
  assert.equal(notices(played.sent).at(-1).text, "Mesos read: 1,000,000");
  assert.equal(summary(played.sent).lootPerHour, null);
  const count = notices(played.sent).length;
  const later = grind(played, played.now, 600);
  played.send(total(1_050_000, later));
  assert.equal(notices(played.sent).length, count, "said once a session");
  assert.equal(summary(played.sent).lootPerHour, 300_000);
  assert.equal(section(played.sent, "Loot").children[0].value, "300K");
  assert.equal(lootLines(played.sent)[0], "Mesos read just now");
  assert.deepEqual(lootFigures(played.sent), { "Mesos picked up": "50,000", "Loot sold": "—" });
  // A shop's pay during the session is loot sold, and Profit shows from then on.
  played.send(total(1_250_000, later + 1_000, "shop"));
  assert.deepEqual(lootFigures(played.sent), { "Mesos picked up": "50,000", "Loot sold": "200,000", Profit: "250,000" });
});

// A total read in the five minutes before the first kill opens the session; an older one doesn't.
// Fails if the player has to open the inventory again right after a check in town, or a stale
// total becomes the start.
test("a total read just before the first kill opens the session", () => {
  const opened = (gapMs) => {
    const plugin = run();
    plugin.send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: MESOS });
    plugin.send(map("aaaa", T0));
    plugin.send(exp(230_000, T0));
    plugin.send(total(1_204_330, T0 + 1_000));
    assert.equal(notices(plugin.sent).at(-1).text, "Mesos read: 1,204,330");
    plugin.send({ type: "tick", atMs: T0 + 1_000 + gapMs });
    plugin.send(exp(230_063, T0 + 1_000 + gapMs));
    // Saves come at most every 5 s; a tick past that saves the kill.
    plugin.send({ type: "tick", atMs: T0 + 7_000 + gapMs });
    return last(plugin.sent, "save").data.current.lg;
  };
  assert.equal(opened(5 * MIN).ob, 1_204_330);
  assert.equal(opened(5 * MIN + 1), undefined);
});

// After End the strip asks the player to sell by a fixed time once End's Undo has passed, and
// every sale in the next 30 minutes adds to the session with its own notice and Undo; an undone
// sale isn't counted again by the next total, a purchase is spent, and the close-out ends at its
// time. Fails if the close-out stops at its first total, a sale lands nowhere or twice, or a total
// after the 30 minutes lands in the old session.
test("sales after End add to the session until the close-out's time runs out", () => {
  const played = play({ seconds: 120 });
  const t = twoReadings(played);
  played.send(action("end", t));
  assert.match(notices(played.sent).at(-1).text, /^Saved: [\d,]+ EXP and 50K mesos so far in 12m \(/);
  assert.equal(summary(played.sent).closeOutBy, null, "not over End's Undo");
  played.send({ type: "tick", atMs: t + 11_000 });
  assert.equal(summary(played.sent).closeOutBy, t + 30 * MIN);
  played.send(total(1_262_000, t + 60_000, "shop"));
  // 50,000 picked up and 212,000 sold over 10 minutes of play.
  assert.deepEqual(
    [notices(played.sent).at(-1).text, notices(played.sent).at(-1).undo],
    ["Added +212K sold. Session profit 262K (1.57M/hr).", "undo-closeout"],
  );
  played.send(total(1_380_000, t + 61_000, "shop"));
  assert.equal(notices(played.sent).at(-1).text, "Added +118K sold. Session profit 380K (2.28M/hr).");
  assert.equal(summary(played.sent).closeOutBy, t + 30 * MIN, "still open after a sale");
  assert.deepEqual(lootTable(played.sent).rows[0].slice(1), ["300K", "380K"]);
  assert.ok(lootTable(played.sent).details[0].some((line) => line.startsWith("Sales count until ")));
  // The last rise wasn't a sale: Undo takes it out, and the next total is measured from it.
  played.send(action("undo-closeout", t + 62_000));
  assert.equal(notices(played.sent).at(-1).text, "Sale undone: +118K isn't counted.");
  assert.equal(lootTable(played.sent).rows[0][2], "262K");
  played.send(total(1_400_000, t + 70_000, "shop"));
  assert.equal(notices(played.sent).at(-1).text, "Added +20K sold. Session profit 282K (1.69M/hr).");
  const said = notices(played.sent).length;
  played.send(total(1_399_250, t + 80_000, "shop"));
  assert.equal(notices(played.sent).length, said, "a purchase says nothing");
  played.send({ type: "tick", atMs: t + 30 * MIN + 1 });
  assert.equal(summary(played.sent).closeOutBy, null);
  played.send(total(1_500_000, t + 31 * MIN, "shop"));
  const ledger = last(played.sent, "save").data.history[0].lg;
  assert.deepEqual([ledger.so, ledger.sp, ledger.co], [232_000, 750, undefined]);
  assert.ok(!notices(played.sent).slice(said).some((n) => n.text.startsWith("Added")));

  // A total read during End's Undo window counts once that window passes.
  const early = play({ seconds: 120 });
  const e = twoReadings(early);
  early.send(action("end", e));
  early.send(total(1_100_000, e + 2_000, "shop"));
  assert.equal(notices(early.sent).at(-1).undo, "undo-end", "End's Undo stays");
  early.send({ type: "tick", atMs: e + 11_000 });
  assert.match(notices(early.sent).at(-1).text, /^Added \+50K sold\./);
});

// A session with no total read still counts what it sells after End, where the meso total can be
// read: the shop's total before selling sets the balance and each sale adds. Fails if such a
// session can't close out, or the first total counts as a sale.
test("a session with no total read counts its sales after End", () => {
  const played = play({ seconds: 120 });
  played.send(action("end", played.now));
  played.send({ type: "tick", atMs: played.now + 11_000 });
  assert.equal(summary(played.sent).closeOutBy, played.now + 30 * MIN);
  played.send(total(800_000, played.now + 60_000, "shop"));
  played.send(total(812_000, played.now + 70_000, "shop"));
  assert.equal(notices(played.sent).at(-1).text, "Added +12K sold. Session profit 12K.");
  assert.deepEqual(lootTable(played.sent).rows[0].slice(1), ["—", "12K"]);
  // From a host that reads no meso total, End asks for no sale.
  const plain = play({ seconds: 120, readers: PLAIN });
  plain.send(action("end", plain.now));
  plain.send({ type: "tick", atMs: plain.now + 11_000 });
  assert.equal(summary(plain.sent).closeOutBy, null);
});

// The next session's first kill ends the close-out, and so does quitting MesoVibe, so a later
// total never lands in the old session. Fails if the old session keeps counting sales into the
// new one or across a restart.
test("the next session's first kill, or quitting, ends the close-out", () => {
  const played = play({ seconds: 120 });
  const t = twoReadings(played);
  played.send(action("end", t));
  played.send({ type: "tick", atMs: t + 11_000 });
  const saved = structuredClone(last(played.sent, "save").data);
  played.send(exp(240_000, t + 20_000));
  played.send(exp(240_100, t + 30_000));
  assert.equal(summary(played.sent).closeOutBy, null);
  played.send(total(1_500_000, t + 40_000, "shop"));
  assert.ok(!notices(played.sent).some((n) => n.text.startsWith("Added")));
  assert.equal(last(played.sent, "save").data.history[0].lg.so, 0);

  assert.equal(saved.history[0].lg.co, t + 30 * MIN);
  const again = run();
  again.send({ type: "start", saved, atMs: t + 60_000, capture: "capturing", readers: MESOS });
  assert.equal(summary(again.sent).closeOutBy, null);
  again.send(total(1_500_000, t + 61_000, "shop"));
  assert.ok(!notices(again.sent).some((n) => n.text.startsWith("Added")));
});

// Undo of End puts the session back, and a total read after it is measured from the session's
// last balance, so nothing picked up in between is lost. Fails if the restored session's ledger is
// lost or a total read in End's Undo window counts twice.
test("Undo after End keeps the session's mesos", () => {
  const played = play({ seconds: 120 });
  const t = twoReadings(played);
  played.send(action("end", t));
  played.send(total(1_080_000, t + 2_000));
  played.send(action("undo-end", t + 3_000));
  const later = grind(played, t + 3_000, 120, 238_000);
  played.send(total(1_090_000, later));
  assert.equal(lootFigures(played.sent)["Mesos picked up"], "90,000");
});

// A jump in the EXP bar (another character) starts a new ledger segment, so the jump between two
// characters' mesos counts as nothing. Fails if the other character's balance shows as mesos
// picked up or as spending.
test("an EXP jump starts a new ledger segment", () => {
  const played = play({ seconds: 120 });
  const t = twoReadings(played);
  played.send(event({ type: "ExpChanged", level: 20, exp: 100, pct_hundredths: 10, needed: 50_000 }, t + 1_000));
  played.send(total(40_000_000, t + 2_000));
  played.send({ type: "tick", atMs: t + 8_000 });
  const ledger = last(played.sent, "save").data.current.lg;
  assert.deepEqual([ledger.pu, ledger.sp, ledger.ho.length], [50_000, 0, 0]);
  assert.equal(lootFigures(played.sent)["Mesos picked up"], "50,000");
});

// A rise across a pause is what a shop paid, never mesos picked up; a rise too big for farming is
// held with Count it, and counting it can be undone. Fails if a pause's rise feeds a map's rate,
// a held rise counts unasked, or Count it or its Undo don't reach Profit.
test("a rise across a pause is sold, and Count it adds a held rise with Undo", () => {
  const played = play({ seconds: 120 });
  played.send(total(1_000_000, played.now));
  let t = grind(played, played.now, 300);
  played.send(action("pause", t));
  played.send(total(1_040_000, t + 60_000));
  played.send(action("resume", t + 61_000));
  const ledger = last(played.sent, "save").data.current.lg;
  assert.deepEqual([ledger.pu, ledger.so], [0, 40_000]);
  assert.equal(last(played.sent, "save").data.current.maps[0].fm, undefined);
  t = grind(played, t + 61_000, 10);
  played.send(total(6_040_000, t));
  const button = section(played.sent, "Loot").children.find((n) => n.kind === "button");
  assert.equal(button.label, "Count it");
  assert.equal(lootFigures(played.sent).Profit, "40,000");
  played.send(action(button.action, t + 1_000));
  assert.deepEqual([notices(played.sent).at(-1).text, notices(played.sent).at(-1).undo], ["Counted +5M in Profit.", "undo-count"]);
  assert.equal(lootFigures(played.sent).Profit, "5,040,000");
  played.send(action("undo-count", t + 2_000));
  assert.equal(notices(played.sent).at(-1).text, "+5M is held out of Profit again.");
  assert.equal(lootFigures(played.sent).Profit, "40,000");
});

// A held rise of the session just ended is listed in the Loot tab during its close-out. Fails if
// it isn't listed, or Count it or its Undo act on any session but the ended one.
test("a held rise in the session just ended can be counted from the Loot tab, with Undo", () => {
  const played = play({ seconds: 120 });
  played.send(total(1_000_000, played.now));
  const t = grind(played, played.now, 600);
  played.send(total(6_000_000, t));
  played.send(action("end", t));
  played.send({ type: "tick", atMs: t + 11_000 });
  played.send(total(6_000_000, t + 20_000, "shop"));
  played.send(total(6_100_000, t + 21_000, "shop"));
  played.send({ type: "tick", atMs: t + 32_000 });
  const lootPast = () => section(played.sent, "Past sessions", "loot").children;
  const countIt = () => lootPast().find((n) => n.kind === "button" && n.label === "Count it");
  assert.equal(lootTable(played.sent).rows[0][2], "100K", "the sale, without the held rise");
  assert.ok(lootTable(played.sent).details[0].includes("Not counted: +5M"));
  assert.ok(lootPast().some((n) => n.kind === "text" && n.text.startsWith("From the session you just ended")));
  played.send(action(countIt().action, t + 40_000));
  assert.deepEqual([notices(played.sent).at(-1).text, notices(played.sent).at(-1).undo], ["Counted +5M in Profit.", "undo-count"]);
  assert.equal(lootTable(played.sent).rows[0][2], "5.1M");
  assert.equal(countIt(), undefined, "nothing left to count");
  played.send(action("undo-count", t + 41_000));
  assert.equal(notices(played.sent).at(-1).text, "+5M is held out of Profit again.");
  assert.equal(lootTable(played.sent).rows[0][2], "100K");
  assert.ok(countIt() !== undefined, "held again in the ended session");
});

// A held rise counted while nothing is sold yet shows in Profit. Fails if Count it changes nothing
// the Loot section shows.
test("Count it with nothing sold shows the rise in Profit", () => {
  const played = play({ seconds: 120 });
  played.send(total(1_000_000, played.now));
  const t = grind(played, played.now, 600);
  played.send(total(6_000_000, t));
  assert.equal(lootFigures(played.sent).Profit, undefined, "no Profit before a sale or a count");
  const button = section(played.sent, "Loot").children.find((n) => n.kind === "button");
  played.send(action(button.action, t + 1_000));
  assert.equal(lootFigures(played.sent).Profit, "5,000,000");
});

// After its 30-minute close-out, the ended session's held rise stays uncounted. Fails if Count it
// still shows, or a late press of it counts the rise.
test("a held rise of the session just ended can't be counted after its close-out", () => {
  const played = play({ seconds: 120 });
  played.send(total(1_000_000, played.now));
  const t = grind(played, played.now, 600);
  played.send(total(6_000_000, t));
  played.send(action("end", t));
  played.send({ type: "tick", atMs: t + 11_000 });
  const countIt = () => section(played.sent, "Past sessions", "loot").children.find((n) => n.kind === "button" && n.label === "Count it");
  const press = countIt().action;
  played.send({ type: "tick", atMs: t + 31 * MIN });
  assert.equal(countIt(), undefined);
  played.send(action(press, t + 31 * MIN + 1_000));
  played.send({ type: "tick", atMs: t + 32 * MIN });
  const past = last(played.sent, "save").data.history[0].lg;
  assert.deepEqual([past.ct, past.ho.length], [0, 1]);
});

// A sale after End never takes the place of another Undo still open: it says what it added once
// that Undo has passed. Fails if a sale read just after Count it hides the count's Undo.
test("a sale after End keeps the Undo of a count just made", () => {
  const played = play({ seconds: 120 });
  played.send(total(1_000_000, played.now));
  const t = grind(played, played.now, 600);
  played.send(total(6_000_000, t));
  played.send(action("end", t));
  played.send({ type: "tick", atMs: t + 11_000 });
  played.send(total(6_000_000, t + 20_000, "shop"));
  const countIt = section(played.sent, "Past sessions", "loot").children.find((n) => n.kind === "button" && n.label === "Count it");
  played.send(action(countIt.action, t + 30_000));
  played.send(total(6_100_000, t + 32_000, "shop"));
  assert.equal(notices(played.sent).at(-1).undo, "undo-count", "the count's Undo still shows");
  played.send(action("undo-count", t + 34_000));
  assert.equal(notices(played.sent).at(-1).text, "+5M is held out of Profit again.");
  played.send({ type: "tick", atMs: t + 46_000 });
  assert.match(notices(played.sent).at(-1).text, /^Added \+100K sold\. Session profit 100K/);
});

// Counting the ended session's held rise during its close-out shows in its Past sessions row, sold
// or not. Fails if the row's Profit stays "—" after Count it.
test("Count it on the session just ended shows in its row with nothing sold", () => {
  const played = play({ seconds: 120 });
  played.send(total(1_000_000, played.now));
  const t = grind(played, played.now, 600);
  played.send(total(6_000_000, t));
  played.send(action("end", t));
  played.send({ type: "tick", atMs: t + 11_000 });
  assert.equal(lootTable(played.sent).rows[0][2], "—");
  const countIt = section(played.sent, "Past sessions", "loot").children.find((n) => n.kind === "button" && n.label === "Count it");
  played.send(action(countIt.action, t + 20_000));
  assert.equal(lootTable(played.sent).rows[0][2], "5M");
});

// Another character after End adds nothing to the ended session. Fails if the second character's
// total counts as a sale, a spend or a held rise in the first one's session.
test("another character after End adds nothing to the ended session", () => {
  const played = play({ seconds: 120 });
  const t = twoReadings(played);
  played.send(action("end", t));
  played.send({ type: "tick", atMs: t + 11_000 });
  played.send(event({ type: "ExpChanged", level: 20, exp: 100, pct_hundredths: 10, needed: 50_000 }, t + 20_000));
  played.send(total(30_000, t + 25_000));
  played.send({ type: "tick", atMs: t + 40_000 });
  const past = last(played.sent, "save").data.history[0].lg;
  assert.deepEqual([past.so, past.sp, past.ho.length], [0, 0, 0]);
  // One at the same level with less EXP reads as a loss, and adds nothing either.
  const same = play({ seconds: 120 });
  const u = twoReadings(same);
  same.send(action("end", u));
  same.send({ type: "tick", atMs: u + 11_000 });
  same.send(exp(100_000, u + 20_000));
  same.send(total(1_070_000, u + 25_000));
  same.send({ type: "tick", atMs: u + 40_000 });
  const ended = last(same.sent, "save").data.history[0].lg;
  assert.deepEqual([ended.so, ended.sp, ended.ho.length], [0, 0, 0]);
});

// The mini-HUD asks for the inventory while the session has no total read or only one older than
// 15 minutes, and only from a host that reads the meso total. Fails if the ask is missing, outlives
// a fresh total, or asks for a reading nothing makes.
test("the mini-HUD asks for the inventory with no total read, or a stale one", () => {
  const reading = play({ seconds: 120 });
  assert.equal(summary(reading.sent).askForMesos, true);
  assert.deepEqual(lootLines(reading.sent).slice(0, 1), ["Open your inventory to start counting mesos."]);
  reading.send(total(1_000_000, reading.now));
  assert.equal(summary(reading.sent).askForMesos, false);
  const later = grind(reading, reading.now, 15 * 60 + 1);
  assert.equal(summary(reading.sent).askForMesos, true, "asks again once it's over 15 minutes old");
  reading.send(total(1_030_000, later));
  assert.equal(summary(reading.sent).askForMesos, false);
  reading.send(action("pause", later + 16 * MIN));
  reading.send({ type: "tick", atMs: later + 16 * MIN + 1_000 });
  assert.equal(summary(reading.sent).askForMesos, false, "not while paused");
  assert.equal(summary(play({ seconds: 120, readers: PLAIN }).sent).askForMesos, false);
  assert.equal(summary(play({ seconds: 120, readers: { ...PLAIN, mesos: "learning" } }).sent).askForMesos, false);
});

// The Session view's This session keeps its map row with the picture key and an accessible name.
// Fails if the map row loses its picture or its name.
test("This session names its map", () => {
  const { sent } = play({ seconds: 20 });
  assert.deepEqual(items(sent).Map, { gameText: "aaaa", label: items(sent).Map.label });
  assert.match(items(sent).Map.label, /^Map first played /);
});
