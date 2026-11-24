// The Session Tracker's loot as the host runs it: mesos and time by map, a map's mesos/hr, the
// lines about reading pickups, and Past sessions rows named for a map. Bundled from source with the SDK and
// driven only through the SDK's messages, like session-tracker-plugin.test.mjs. Past sessions
// doesn't show mesos/hr yet, so its tests read the saved session through the loot arithmetic.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { bundleTracker, run as runBundle } from "./session-tracker-harness.mjs";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const esbuild = createRequire(join(REPO, "plugins", "package.json"))("esbuild");

const bundle = await bundleTracker();

/** The loot arithmetic (loot.ts), as the plugin bundles it. */
const loot = await import(
  `data:text/javascript;base64,${Buffer.from(
    (
      await esbuild.build({
        entryPoints: [join(REPO, "plugins/src/session-tracker/loot.ts")],
        bundle: true,
        write: false,
        format: "esm",
        platform: "neutral",
        target: "es2022",
        logLevel: "silent",
      })
    ).outputFiles[0].text,
  ).toString("base64")}`
);

const run = () => runBundle(bundle);

const last = (sent, type) => sent.filter((m) => m.type === type).at(-1);
const NEEDED = 356_916;
const T0 = 1_800_000_000_000;
const READING = { notices: "reading", waitingMesos: 0 };
const event = (e, atMs) => ({ type: "event", atMs, event: e });
const exp = (value, atMs) =>
  event({ type: "ExpChanged", level: 42, exp: value, pct_hundredths: Math.round((value * 10_000) / NEEDED), needed: NEEDED }, atMs);
const mesos = (amount, atMs, more = {}) => event({ type: "MesosGained", amount, late: false, shaped: false, ...more }, atMs);
const map = (key, atMs) => event({ type: "MapChanged", map_key: key }, atMs);
const missed = (reason, atMs) => event({ type: "NoticesMissed", reason }, atMs);

/** A section of the last view drawn: the Session view, or with `page` that page ("loot"). */
const section = (sent, title, page) =>
  sent.filter((m) => m.type === "render" && m.page === page).at(-1).view.find((node) => node.title === title);
const thisSession = (sent) => section(sent, "This session")?.children ?? [];
const items = (sent) => Object.fromEntries(thisSession(sent).find((n) => n.kind === "keyValue")?.items ?? []);
/** The Loot section's figures, by label: Profit, Spent, Unsold items. */
const lootFigures = (sent) => Object.fromEntries(section(sent, "Loot")?.children.find((n) => n.kind === "keyValue")?.items ?? []);
const pastSessions = (sent) => section(sent, "Past sessions").children;
const summary = (sent) => last(sent, "summary").summary;
const lines = (sent) => thisSession(sent).filter((n) => n.kind !== "keyValue").map((n) => n.text);

/**
 * A plugin started reading on map "aaaa": EXP every 10 s and `mesosEvery` 50 mesos, for
 * `seconds`. Returns the plugin and the time it reached.
 */
function play({ seconds = 120, readers = READING, key = "aaaa" } = {}) {
  const plugin = run();
  plugin.send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers });
  plugin.send(map(key, T0));
  plugin.send(exp(230_000, T0));
  for (let s = 1; s <= seconds; s++) {
    const at = T0 + s * 1000;
    plugin.send({ type: "tick", atMs: at });
    if (s % 10 === 0) {
      plugin.send(exp(230_000 + (s / 10) * 63, at));
      plugin.send(mesos(50, at));
    }
  }
  return { ...plugin, now: T0 + seconds * 1000 };
}

/** A saved session's mesos/hr on the map its row is named for, or null when there's too little reading. */
const savedMesosRate = (past) => loot.mapRates(loot.longest(past.maps), 60_000).mesos;

/** Ends the session and returns its mesos/hr as saved, rounded to the mesos. */
function endedMesosRate(plugin) {
  plugin.send({ type: "action", action: "end", atMs: plugin.now });
  const rate = savedMesosRate(last(plugin.sent, "save").data.history[0]);
  return rate === null ? null : Math.round(rate);
}

// A map's mesos/hr is the mesos counted over the time the notices were read. Fails if mesos go
// unseen, a rate shows before a minute of reading, or the map's share doesn't add up.
test("mesos count toward the map, and its mesos/hr", () => {
  const played = play({ seconds: 120 });
  const { sent } = played;
  assert.equal(lootFigures(sent).Profit, "≈600", "an estimate from the notices until meso totals are read");
  assert.deepEqual(items(sent).Map, { gameText: "aaaa", label: items(sent).Map.label });
  assert.match(items(sent).Map.label, /^Map first played /);
  const running = last(sent, "save")?.data.current;
  if (running?.maps) {
    const total = running.maps.reduce((sum, m) => sum + m.p, 0);
    assert.equal(total, running.activeMs, "the maps add up to the session");
    for (const m of running.maps) assert.ok(m.l <= m.p, "read time never passes play time");
  }
  // 12 pickups of 50 over two minutes of reading: 300 mesos a minute.
  assert.equal(endedMesosRate(played), 18_000);
  assert.equal(endedMesosRate(play({ seconds: 50 })), null, "under a minute of reading: no rate");
});

// Covered notices are out of the rate both ways: their time and their mesos. Fails if mesos read
// while covered raise the rate, or covered time lowers it.
test("time with the notices covered changes neither side of a map's mesos/hr", () => {
  const plugin = run();
  plugin.send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: READING });
  plugin.send(map("aaaa", T0));
  plugin.send(exp(230_000, T0));
  for (let s = 1; s <= 240; s++) {
    const at = T0 + s * 1000;
    // Covered for the middle two minutes, with mesos still read now and then.
    if (s === 61) plugin.send({ type: "readers", readers: { notices: "covered", waitingMesos: 0 }, atMs: at });
    if (s === 181) plugin.send({ type: "readers", readers: READING, atMs: at });
    plugin.send({ type: "tick", atMs: at });
    if (s % 10 === 0) {
      plugin.send(exp(230_000 + (s / 10) * 63, at));
      plugin.send(mesos(s > 60 && s <= 180 ? 500 : 50, at));
    }
  }
  // The total has every amount read.
  assert.equal(lootFigures(plugin.sent).Profit, `≈${(12 * 50 + 12 * 500).toLocaleString("en-US")}`);
  // Read for two minutes at 300 a minute: the covered 500s don't count, nor do their minutes.
  assert.equal(endedMesosRate({ ...plugin, now: T0 + 240_000 }), 18_000);
});

// Time the notices can't be trusted isn't read time. Fails if a map gets a mesos rate from time
// spent learning the letters or with the notices covered.
test("no mesos/hr from time learning or blocked", () => {
  for (const notices of ["learning", "blocked"]) {
    assert.equal(endedMesosRate(play({ seconds: 120, readers: { notices, waitingMesos: 0 } })), null, notices);
  }
});

// The session's clock starts at its first EXP gain, as before loot: a pickup after End must not
// start it early. Fails if EXP/hr after End, a pickup, three idle minutes and a kill differs from
// the same without the pickup.
test("a pickup before the first kill doesn't start the EXP/hr clock", () => {
  const rateAfter = (withPickup) => {
    const { send, sent, now } = play({ seconds: 60 });
    send({ type: "action", action: "end", atMs: now });
    if (withPickup) send(mesos(80, now + 1_000));
    for (let s = 1; s <= 180; s++) send({ type: "tick", atMs: now + s * 1000 });
    for (let k = 1; k <= 6; k++) {
      send({ type: "tick", atMs: now + 180_000 + k * 10_000 });
      send(exp(230_378 + k * 63, now + 180_000 + k * 10_000));
    }
    return summary(sent).expPerHour;
  };
  assert.equal(rateAfter(true), rateAfter(false));
});

// Loot before the first kill joins the session when EXP follows soon on the same map, and the
// strip says so meanwhile. Fails if it's lost, or counted after a long wait.
test("loot before the first kill is held, then joins or is dropped", () => {
  const plugin = run();
  plugin.send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: READING });
  plugin.send(map("aaaa", T0));
  plugin.send(exp(230_000, T0));
  plugin.send(mesos(80, T0 + 1_000));
  assert.equal(summary(plugin.sent).lootOnly, true);
  assert.equal(summary(plugin.sent).canReset, true);
  assert.equal(summary(plugin.sent).canEnd, false, "not worth saving yet");
  plugin.send({ type: "tick", atMs: T0 + 60_000 });
  plugin.send(exp(230_063, T0 + 60_000));
  assert.equal(lootFigures(plugin.sent).Profit, "≈80", "joined");

  const late = run();
  late.send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: READING });
  late.send(map("aaaa", T0));
  late.send(exp(230_000, T0));
  late.send(mesos(80, T0 + 1_000));
  late.send({ type: "tick", atMs: T0 + 400_000 });
  late.send(exp(230_063, T0 + 400_000));
  assert.equal(lootFigures(late.sent).Profit, undefined, "dropped after five minutes");
});

// What the notices are doing shows under This session; amounts still being read don't, since
// they're added once their digits are known, and nor do missed pickups, since every figure from
// the notices is marked "≈" and Seen picking up says they miss some. Fails if a state passes
// silently, or a waiting or missed line, or a "+", comes back.
test("learning and wrong-digit lines, and none for amounts still being read or missed", () => {
  const learning = play({ seconds: 20, readers: { notices: "learning", waitingMesos: 0 } });
  assert.deepEqual(lines(learning.sent), ["Learning the pickup text. Mesos show after a few kills."]);
  learning.send({ type: "tick", atMs: learning.now + 5 * 60_000 });
  assert.match(lines(learning.sent)[0], /^Still learning the pickup text: it learns from EXP lines/);

  const { send, sent, now } = play({ seconds: 20 });
  send({ type: "readers", readers: { notices: "reading", waitingMesos: 2 }, atMs: now });
  assert.deepEqual(lines(sent), []);
  assert.equal(lootFigures(sent).Profit, "≈100");
  send({ type: "readers", readers: READING, atMs: now + 1_000 });
  send(missed("not_learned", now + 2_000));
  assert.deepEqual(lines(sent), [], "an amount waiting for a digit isn't missed");
  send(missed("unaligned", now + 3_000));
  assert.deepEqual(lines(sent), []);
  assert.equal(lootFigures(sent).Profit, "≈100");
  send(missed("shapes_wrong", now + 4_000));
  assert.deepEqual(lines(sent), ["Some mesos amounts may be wrong: a digit was misread and is no longer used."]);

  const party = play({ seconds: 20, readers: { notices: "blocked", waitingMesos: 0 } });
  assert.deepEqual(lines(party.sent), ["Pickups aren't being read: something keeps covering the notices, like a party list."]);
});

// Past sessions rows are named for the map played longest, with the session's EXP gained and that
// map's own rates. Fails if the row shows the wrong map, misses "+N maps", mixes another map's
// rates in, or shows mesos/hr before it's ready.
test("a past session's row is its longest map, with that map's rates", () => {
  const { send, sent, now } = play({ seconds: 120 });
  send(map("bbbb", now));
  for (let s = 1; s <= 30; s++) {
    send({ type: "tick", atMs: now + s * 1000 });
    if (s % 10 === 0) send(exp(230_756 + s * 100, now + s * 1000));
  }
  send({ type: "action", action: "end", atMs: now + 30_000 });
  assert.match(last(sent, "notice").text, /^Saved: [\d,]+ EXP and ≈600 mesos in /);
  const table = pastSessions(sent)[0];
  assert.deepEqual(table.columns, ["Map", "EXP gained", "EXP/hr"]);
  const [cell, gained, expRate] = table.rows[0];
  assert.equal(cell.gameText, "aaaa");
  assert.equal(cell.suffix, "+1 map");
  // The whole session: 12 gains of 63 on aaaa, then 3,000 on bbbb.
  assert.equal(gained, "3.76K");
  // Map aaaa's own EXP/hr: 12 gains of 63 in two minutes.
  assert.equal(expRate, "22.7K");
  // Mesos/hr is still kept with the session, for aaaa alone.
  assert.equal(Math.round(savedMesosRate(last(sent, "save").data.history[0])), 18_000);
});

// Every past session uses one layout: one from before maps were tracked reads as an unknown map
// with its own EXP gained and EXP/hr. Fails if it gets a separate table or loses its date line.
test("sessions from before map tracking use the same layout", () => {
  const old = { startedAtMs: T0 - 86_400_000, endedAtMs: T0 - 86_000_000, activeMs: 400_000, expGained: 9_000, expLost: 0, startLevel: 41, endLevel: 41 };
  const { send, sent } = run();
  const saved = { version: 1, history: [old], current: null };
  send({ type: "start", saved, atMs: T0, capture: "capturing", readers: READING });
  send(map("aaaa", T0));
  send(exp(230_000, T0));
  for (let s = 1; s <= 60; s++) {
    send({ type: "tick", atMs: T0 + s * 1000 });
    if (s % 10 === 0) send(exp(230_000 + s * 6, T0 + s * 1000));
  }
  send({ type: "action", action: "end", atMs: T0 + 60_000 });
  const [table, ...rest] = pastSessions(sent);
  assert.equal(table.rows.length, 2, "one table for both");
  assert.ok(!rest.some((node) => node.kind === "table" || node.kind === "text"));
  assert.deepEqual(table.rows[1], ["Unknown map", "9K", "81K"]);
  assert.match(table.details[1][0], /^\w{3}\u00a0\d+ · 6m$/);
  assert.equal(table.details[1][1], "Lv\u00a041");
});

// Pickups made while the session is paused aren't the session's. Fails if they count, or Resume
// stops pickups counting again.
test("pickups don't count while paused", () => {
  const { send, sent, now } = play({ seconds: 20 });
  send({ type: "action", action: "pause", atMs: now });
  send(mesos(500, now + 1_000));
  assert.equal(lootFigures(sent).Profit, "≈100");
  send({ type: "action", action: "resume", atMs: now + 2_000 });
  send(mesos(70, now + 3_000));
  assert.equal(lootFigures(sent).Profit, "≈170");
});

// Undo puts the ended session back with the pickups made since, mesos and items. Fails if they're
// lost, or the items from before End and after it don't add up.
test("Undo after End keeps pickups made in its window", () => {
  const { send, sent, now } = play({ seconds: 60 });
  send(item("Tablecloth", now, { item_id: 4000028, sell: 34 }));
  send({ type: "action", action: "end", atMs: now });
  send(exp(230_400, now + 2_000));
  send(mesos(70, now + 3_000));
  send(item("Tablecloth", now + 3_500, { item_id: 4000028, sell: 34 }));
  send(item("Leather", now + 4_000, { item_id: 4000049, sell: 10 }));
  send({ type: "action", action: "undo-end", atMs: now + 5_000 });
  assert.equal(lootFigures(sent).Profit, "≈370");
  assert.deepEqual(itemTable(sent), [
    [
      ["Tablecloth", "×2", "68"],
      ["Leather", "×1", "10"],
    ],
  ]);
});

// A bad value in a save must never stop the tracker. Fails if a malformed map entry throws, or
// the good parts of the session are lost with it.
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
  };
  const { send, sent } = run();
  send({ type: "start", saved: { version: 1, history: [past], current: null }, atMs: T0, capture: "capturing", readers: READING });
  const row = pastSessions(sent)[0].rows[0];
  assert.equal(row[0].gameText, "aaaa");
  assert.equal(row[0].suffix, undefined, "the bad entries are gone");
});

// Sessions past the 500 kept go into All time, never lost. Fails if a dropped session's mesos or
// items vanish from the save, or All time stops listing those items after a restart.
test("a session past the 500 kept adds its mesos and items to All time", () => {
  const session = (i) => ({
    startedAtMs: T0 - (i + 1) * 3_600_000,
    endedAtMs: T0 - (i + 1) * 3_600_000 + 600_000,
    activeMs: 600_000,
    expGained: 100,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    // Only the oldest, the one End pushes out, picked up items.
    maps: [{ k: "aaaa", p: 600_000, x: 100, m: 7, r: 7, l: 600_000, s: 42, e: 42, ...(i === 499 ? { i: [{ k: "#4000028", c: 3 }] } : {}) }],
  });
  const history = Array.from({ length: 500 }, (_, i) => session(i));
  const catalog = { "#4000028": { n: "Tablecloth", t: "Etc", v: 34 } };
  const { send, sent, now } = (() => {
    const plugin = run();
    plugin.send({ type: "start", saved: { version: 1, history, current: null, catalog }, atMs: T0, capture: "capturing", readers: READING });
    plugin.send(map("aaaa", T0));
    plugin.send(exp(230_000, T0));
    for (let s = 1; s <= 20; s++) {
      plugin.send({ type: "tick", atMs: T0 + s * 1000 });
      if (s % 10 === 0) plugin.send(exp(230_000 + s * 6, T0 + s * 1000));
    }
    return { ...plugin, now: T0 + 20_000 };
  })();
  send({ type: "action", action: "end", atMs: now });
  const saved = last(sent, "save").data;
  assert.equal(saved.history.length, 500);
  assert.deepEqual(saved.allTime, { mesos: 7, playedMs: 600_000, items: [{ k: "#4000028", c: 3 }] });
  assert.deepEqual(itemTable(sent, "All time", "loot"), [[["Tablecloth", "×3", "102"]]]);
  const again = run();
  again.send({ type: "start", saved, atMs: now + 10_000, capture: "capturing", readers: READING });
  assert.deepEqual(itemTable(again.sent, "All time", "loot"), [[["Tablecloth", "×3", "102"]]]);
});

const item = (name, atMs, more = {}) =>
  event(
    {
      type: "ItemGained",
      item_key: "0000000000000000",
      count: 1,
      tab: "Etc",
      item_id: name === null ? null : 4_000_000 + name.length,
      name,
      sell: name === null ? null : 10,
      ...more,
    },
    atMs,
  );
const itemTable = (sent, title = "Seen picking up", page) =>
  section(sent, title, page)?.children.filter((n) => n.kind === "table").map((t) => t.rows.map((r) => r.slice(0, 3)));
/** The Loot tab's All time figures, by label. */
const allTimeFigures = (sent) => Object.fromEntries(section(sent, "All time", "loot").children.find((n) => n.kind === "keyValue")?.items ?? []);

// Items picked up count on the map with their NPC value, shown as unsold items beside Profit, never
// in it, so selling them later never makes the headline drop; All time counts them in its profit as
// if sold. Fails if items go unseen, their value is wrong, they creep into Profit/hr, or All time
// leaves them out.
test("items count with their NPC value as unsold items, and in All time's profit", () => {
  const { send, sent, now } = play({ seconds: 120 });
  send(item("Tablecloth", now, { item_id: 4000028, sell: 34, count: 3 }));
  send(item("Jr. Necki Skin", now + 1, { item_id: 4000018, sell: 21 }));
  send(item("Tablecloth", now + 2, { item_id: 4000028, sell: 34 }));
  assert.equal(lootFigures(sent)["Unsold items"], "≈157");
  assert.deepEqual(itemTable(sent), [
    [
      ["Tablecloth", "×4", "136"],
      ["Jr. Necki Skin", "×1", "21"],
    ],
  ]);
  // 600 mesos over two minutes of reading; the items wait to be sold.
  assert.equal(Math.round(summary(sent).lootPerHour), 18_000);
  assert.equal(summary(sent).lootApprox, true);
  assert.deepEqual(section(sent, "Loot").children[0], {
    kind: "stat",
    label: "Profit/hr",
    value: "≈18K",
    detail: "About 18,000 mesos per hour, from pickup notices",
  });
  assert.equal(items(sent).Mesos, undefined, "Loot has the money; This session doesn't repeat it");
  assert.equal(items(sent)["Item value"], undefined);
  assert.equal(section(sent, "All time"), undefined, "All time is in the Loot tab");
  assert.equal(allTimeFigures(sent).Profit, "≈757");
  assert.equal(allTimeFigures(sent)["Profit/hr"], "≈22.7K");
});

// The game cut Blue Jangoon Armor (7,000) and Blue Jangoon Shoes (4,200) to the same "Blue
// Jangoon…": the row shows both prices, and Unsold items counts the lower one. "—" stays for one
// item shops won't buy, and a name with no price at all shows "?". Fails if such a pickup reads as
// worthless, counts a price it may not have, or moves Profit/hr before it's sold.
test("a name several items fit shows their price range and counts the lowest", () => {
  const { send, sent, now } = play({ seconds: 120 });
  send(item("Blue Jangoon…", now, { item_id: null, sell: null, sell_min: 4200, sell_max: 7000, tab: "Equip", count: 2 }));
  send(item("White Undershirt", now, { item_id: 1040001, sell: null, sell_min: null, sell_max: null, tab: "Equip" }));
  send(item("Old Wooden Box…", now, { item_id: null, sell: null, sell_min: null, sell_max: null }));
  assert.deepEqual(itemTable(sent), [
    [
      ["Blue Jangoon…", "×2", "8.4K–14K"],
      ["White Undershirt", "×1", "—"],
      ["Old Wooden Box…", "×1", null],
    ],
  ]);
  assert.equal(lootFigures(sent)["Unsold items"], "≈8,400");
  assert.equal(Math.round(summary(sent).lootPerHour), 18_000, "600 mesos over two minutes; items wait to be sold");
});

// No loot rate while the pickup text is still learned. Fails if a rate shows before pickups count.
test("no loot/hr while learning the pickup text", () => {
  const { sent } = play({ seconds: 120, readers: { notices: "learning", waitingMesos: 0 } });
  assert.equal(summary(sent).lootPerHour, null);
});

// Names no item fits, in any tab, are one "Unread items" row with no value, below every named item
// however many more of them there are. Fails if
// unread pickups vanish, split by tab, rise above named items or get a made-up value.
test("names no item fits are one unread row at the bottom, with no value", () => {
  const { send, sent, now } = play({ seconds: 20 });
  send(item(null, now, { tab: "Use", count: 2 }));
  send(item(null, now + 1, { tab: "Etc", count: 3 }));
  send(item("Tablecloth", now + 2, { item_id: 4000028, sell: 34 }));
  assert.deepEqual(itemTable(sent), [
    [
      ["Tablecloth", "×1", "34"],
      ["Unread items", "×5", null],
    ],
  ]);
  const table = section(sent, "Seen picking up").children.find((n) => n.kind === "table");
  assert.deepEqual(table.details, [null, null]);
  assert.equal(lootFigures(sent)["Unsold items"], "≈34");
  // A named item's tab shows under its name unless it's Etc; the unread row never gets one, even
  // with Use pickups in it. Fails if a Use item loses its tab, or Etc or unread rows gain one.
  send(item("Red Potion", now + 3, { item_id: 2000000, sell: 25, tab: "Use" }));
  const withUse = section(sent, "Seen picking up").children.find((n) => n.kind === "table");
  assert.deepEqual(withUse.rows.map((r) => r[0]), ["Tablecloth", "Red Potion", "Unread items"]);
  assert.deepEqual(withUse.details, [null, "Use", null]);
});

// The unread row stays when a long list shows only its first 8. Fails if "Show all" hides it.
test("the unread row shows under a list cut to 8", () => {
  const { send, sent, now } = play({ seconds: 20 });
  for (let i = 0; i < 10; i++) send(item(`Item ${i}`, now + i, { item_id: 4_100_000 + i, count: i + 2 }));
  send(item(null, now + 20));
  const [rows] = itemTable(sent);
  assert.equal(rows.length, 9);
  assert.deepEqual(rows.at(-1), ["Unread items", "×1", null]);
  assert.equal(section(sent, "Seen picking up").children.find((n) => n.kind === "button").label, "Show all 10 items");
});

// Before a minute of pickups is read, Loot says its Profit/hr is coming rather than leaving a gap.
// Fails if the line is missing, or shows while the pickup text is still being learned.
test("Loot says Profit/hr comes after a minute of pickups", () => {
  const { send, sent, now } = play({ seconds: 20 });
  send(item("Tablecloth", now, { item_id: 4000028, sell: 34 }));
  assert.deepEqual(section(sent, "Loot").children[0], { kind: "empty", text: "Profit/hr shows after a minute of pickups." });
  send({ type: "readers", readers: { notices: "learning", waitingMesos: 0 }, atMs: now + 1 });
  assert.equal(section(sent, "Loot").children[0].kind, "keyValue");
});

// Seen picking up says once that its counts come from the notices, which miss some; the old line
// about missed pickups is gone, here and in This session. Fails if the note is missing or the old
// line comes back.
test("Seen picking up says the notices miss some, with no missed-pickups line", () => {
  const { send, sent, now } = play({ seconds: 20 });
  send(item("Tablecloth", now, { item_id: 4000028, sell: 34 }));
  send(missed("unaligned", now + 1));
  assert.deepEqual(section(sent, "Seen picking up").children.at(-1), {
    kind: "text",
    text: "Counted from pickup notices, which miss some on busy maps.",
    variant: "muted",
  });
  const said = sent
    .filter((m) => m.type === "render" && m.page === undefined)
    .at(-1)
    .view.flatMap((node) => node.children ?? [])
    .map((n) => n.text);
  assert.ok(!said.some((text) => text?.startsWith("Some pickups may be missing")));
});

// With items from two maps, each map's items list under its name, in the order played. Fails if
// they're mixed into one list, or one map's items land under the other.
test("items group by map when they came from two maps", () => {
  const { send, sent, now } = play({ seconds: 20 });
  send(item("Tablecloth", now, { item_id: 4000028, sell: 34 }));
  send(map("bbbb", now + 1));
  send(item("Leather", now + 2, { item_id: 4000049, sell: 10 }));
  const children = section(sent, "Seen picking up").children;
  assert.deepEqual(children.filter((n) => n.kind === "keyValue").map((n) => n.items[0][1].gameText), ["aaaa", "bbbb"]);
  assert.deepEqual(itemTable(sent), [[["Tablecloth", "×1", "34"]], [["Leather", "×1", "10"]]]);
});

// A list shows the 8 picked up most, then "Show all N items" until pressed. Fails if the list grows
// past 8 unasked, or the button doesn't show the rest.
test("an item list shows 8 until Show all", () => {
  const { send, sent, now } = play({ seconds: 20 });
  for (let i = 0; i < 10; i++) send(item(`Item ${i}`, now + i, { item_id: 4_100_000 + i, count: i + 1 }));
  assert.equal(itemTable(sent)[0].length, 8);
  assert.equal(itemTable(sent)[0][0][0], "Item 9", "the most picked up first");
  const button = section(sent, "Seen picking up").children.find((n) => n.kind === "button");
  assert.equal(button.label, "Show all 10 items");
  send({ type: "action", action: button.action, atMs: now + 20 });
  assert.equal(itemTable(sent)[0].length, 10);
  // Pressed again, "Show fewer" cuts the list back to 8. Fails if the list can't be shortened.
  assert.equal(section(sent, "Seen picking up").children.find((n) => n.kind === "button").label, "Show fewer");
  send({ type: "action", action: button.action, atMs: now + 21 });
  assert.equal(itemTable(sent)[0].length, 8);
  assert.equal(section(sent, "Seen picking up").children.find((n) => n.kind === "button").label, "Show all 10 items");
  // End and Reset start every list collapsed, here All time's in the Loot tab, which outlives both.
  // Fails if a list opened in one session stays open in the next.
  const allTimeButton = () => section(sent, "All time", "loot").children.find((n) => n.kind === "button");
  const allTimeRows = () => itemTable(sent, "All time", "loot")[0].length;
  send({ type: "action", action: allTimeButton().action, atMs: now + 22 });
  assert.equal(allTimeRows(), 10);
  send({ type: "action", action: "end", atMs: now + 23 });
  assert.equal(last(sent, "save").data.history.length, 1, "ended");
  assert.equal(allTimeRows(), 8);
  assert.equal(allTimeButton().label, "Show all 10 items");
  send(exp(231_000, now + 30_000));
  send(exp(231_100, now + 40_000));
  send({ type: "action", action: allTimeButton().action, atMs: now + 41_000 });
  assert.equal(allTimeRows(), 10);
  send({ type: "action", action: "reset", atMs: now + 42_000 });
  assert.equal(last(sent, "notice").text, "Session reset.");
  assert.equal(allTimeRows(), 8);
});

// Items picked up before the first kill wait with the held mesos and join on the same map. Fails if
// they're lost or counted on another map.
test("items before the first kill are held and join the session", () => {
  const plugin = run();
  plugin.send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: READING });
  plugin.send(map("aaaa", T0));
  plugin.send(exp(230_000, T0));
  plugin.send(item("Tablecloth", T0 + 1_000, { item_id: 4000028, sell: 34 }));
  plugin.send(exp(230_100, T0 + 10_000));
  assert.deepEqual(itemTable(plugin.sent), [[["Tablecloth", "×1", "34"]]]);
});

// Items alone, with no mesos and no kill, are loot like mesos: Reset is offered at once, End after
// five minutes, and End keeps them as a session of their own. Fails if items-only loot can't be
// ended or reset, or End saves the session without its items.
test("items picked up without a kill can be reset, and ended after five minutes", () => {
  const { send, sent } = run();
  send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: READING });
  send(map("aaaa", T0));
  send(exp(230_000, T0));
  send(item("Tablecloth", T0 + 1_000, { item_id: 4000028, sell: 34, count: 2 }));
  assert.equal(summary(sent).canReset, true);
  assert.equal(summary(sent).canEnd, false, "not worth saving yet");
  send({ type: "tick", atMs: T0 + 301_000 });
  assert.equal(summary(sent).canEnd, true);
  send({ type: "action", action: "end", atMs: T0 + 301_000 });
  assert.equal(last(sent, "notice").text, "Saved: 2 items.");
  const [past] = last(sent, "save").data.history;
  assert.deepEqual(past.maps[0].i, [{ k: "#4000028", c: 2 }]);
  const table = section(sent, "Past sessions", "loot").children.find((n) => n.kind === "table");
  assert.deepEqual(table.rows[0].slice(1), ["≈68", null]);
});

// Before the first kill nothing is read toward Profit/hr, so Loot doesn't promise it in a minute.
// Fails if the line shows for held items and waits there for good.
test("held items before the first kill don't promise Profit/hr", () => {
  const { send, sent } = run();
  send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: READING });
  send(map("aaaa", T0));
  send(exp(230_000, T0));
  send(item("Tablecloth", T0 + 1_000, { item_id: 4000028, sell: 34 }));
  assert.equal(section(sent, "Loot").children[0].kind, "keyValue");
});

// Mesos picked up while the notices are covered aren't read time, so they stay out of Profit/hr's
// estimate, and items stay out of it until sold; Undo after End keeps both. Fails if covered
// pickups or unsold items raise the rate, or Undo loses the session's mesos or items.
test("Profit/hr leaves out covered pickups and unsold items, and Undo keeps them", () => {
  const { send, sent, now } = play({ seconds: 120 });
  // The clock moves a few milliseconds between readings, so rates compare within 0.1%.
  const near = (a, b) => assert.ok(Math.abs(a - b) <= b / 1000, `${a} vs ${b}`);
  const before = summary(sent).lootPerHour;
  send({ type: "readers", readers: { notices: "blocked", waitingMesos: 0 }, atMs: now });
  send(mesos(34_000, now + 1));
  send({ type: "readers", readers: READING, atMs: now + 2 });
  near(summary(sent).lootPerHour, before);
  send(item("Tablecloth", now + 3, { item_id: 4000028, sell: 34_000 }));
  near(summary(sent).lootPerHour, before);
  assert.equal(lootFigures(sent)["Unsold items"], "≈34,000");
  send({ type: "action", action: "end", atMs: now + 4 });
  send({ type: "action", action: "undo-end", atMs: now + 5 });
  near(summary(sent).lootPerHour, before);
  assert.deepEqual([lootFigures(sent).Profit, lootFigures(sent)["Unsold items"]], ["≈34,600", "≈34,000"]);
});

// A count misread as 0 is a pickup lost to a misread: it must not make loot to keep or end, or
// show as an item. Fails if a ×0 item offers End or Reset, or shows in Loot or Seen picking up.
test("a pickup read as ×0 counts as missed, not as loot", () => {
  const { send, sent } = run();
  send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: READING });
  send(map("aaaa", T0));
  send(exp(230_000, T0));
  send(item("Tablecloth", T0 + 1_000, { item_id: 4000028, sell: 34, count: 0 }));
  send({ type: "tick", atMs: T0 + 301_000 });
  assert.equal(summary(sent).canReset, false);
  assert.equal(summary(sent).canEnd, false);
  assert.equal(section(sent, "Loot"), undefined);
  assert.equal(section(sent, "Seen picking up"), undefined);
  const playing = play({ seconds: 20 });
  playing.send(item("Tablecloth", playing.now, { item_id: 4000028, sell: 34, count: 0 }));
  assert.equal(section(playing.sent, "Seen picking up"), undefined);
});

// With mesos alone, Loot heads with the session's Profit/hr, where it comes from, and Profit: it
// shows nowhere else in the Session view (the Loot tab's is All time). Fails if a mesos-only
// session's rate or its source line disappears.
test("mesos alone head Loot with the session's Profit/hr", () => {
  const { sent } = play({ seconds: 120 });
  assert.ok(summary(sent).lootPerHour > 0);
  assert.deepEqual(section(sent, "Loot").children.map((n) => n.kind), ["stat", "text", "keyValue"]);
  assert.equal(section(sent, "Loot").children[1].text, "From pickup notices, which miss some.");
  assert.equal(section(sent, "Seen picking up"), undefined);
});

// A price once known stays when a later pickup of the same name comes without one (its tab wasn't
// read, so it could be items in other tabs that sell for less or more). Fails if a past session's
// item value drops to nothing after such a pickup.
test("a later pickup without a price keeps the known price", () => {
  const { send, sent, now } = play({ seconds: 20 });
  send(item("Blue Potion", now, { item_id: null, sell: 22, count: 2 }));
  send({ type: "action", action: "end", atMs: now + 1 });
  const lootGained = () => section(sent, "Past sessions", "loot").children.find((n) => n.kind === "table").rows[0][1];
  const before = lootGained();
  send(exp(230_100, now + 2_000));
  send(exp(230_200, now + 3_000));
  send(item("Blue Potion", now + 4_000, { item_id: null, sell: null }));
  assert.equal(allTimeFigures(sent).Profit, "≈166", "100 mesos and three Blue Potions at 22");
  assert.equal(lootGained(), before);
});

// A save edited by hand, or damaged, can hold item keys no pickup makes ("constructor",
// "__proto__"): they're dropped, so nothing reads a value off the object's prototype. Fails if
// such a key shows as a row or prices anything.
test("item keys a pickup can't make are dropped from a save", () => {
  const past = {
    startedAtMs: T0 - 600_000,
    endedAtMs: T0 - 300_000,
    activeMs: 60_000,
    expGained: 300,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    maps: [{ k: "aaaa", p: 60_000, x: 300, m: 50, r: 50, l: 60_000, s: 42, e: 42, i: [{ k: "constructor", c: 2 }, { k: "#1", c: 1 }] }],
  };
  const catalog = JSON.parse('{"#1": {"n": "Snail Shell", "t": "Etc", "v": 1}, "__proto__": {"n": "x", "t": "Etc", "v": 9}}');
  const { send, sent } = run();
  send({ type: "start", saved: { version: 1, history: [past], current: null, catalog }, atMs: T0, capture: "capturing", readers: READING });
  assert.deepEqual(itemTable(sent, "All time", "loot"), [[["Snail Shell", "×1", "1"]]]);
  assert.equal(allTimeFigures(sent).Profit, "≈51", "50 mesos and one Snail Shell");
});

// A pickup no single item fits is counted under its tab with every other unread pickup there, so
// it can't carry a price, even when the items it might be all sell for the same. Fails if one such
// pickup prices every unread pickup of the tab.
test("unread pickups never take a price", () => {
  const { send, sent, now } = play({ seconds: 20 });
  for (let i = 0; i < 10; i++) send(item(null, now + i, { tab: "Etc", sell: null }));
  send(item(null, now + 20, { tab: "Etc", sell: 5_000 }));
  assert.deepEqual(itemTable(sent), [[["Unread items", "×11", null]]]);
  assert.equal(lootFigures(sent)["Unsold items"], undefined);
});

// All time adds up every session's items and mesos, and survives a restart from the save. Fails if
// a saved session's items, or the names kept for them, are lost.
test("All time lists items from every session, from the save", () => {
  const { send, sent, now } = play({ seconds: 60 });
  send(item("Tablecloth", now, { item_id: 4000028, sell: 34, count: 2 }));
  send({ type: "action", action: "end", atMs: now + 1 });
  const saved = last(sent, "save").data;
  assert.deepEqual(saved.catalog["#4000028"], { n: "Tablecloth", t: "Etc", v: 34 });
  const again = run();
  again.send({ type: "start", saved, atMs: now + 10_000, capture: "capturing", readers: READING });
  const allTime = allTimeFigures(again.sent);
  assert.equal(allTime.Profit, "≈368", "300 mesos and two Tablecloths");
  assert.deepEqual(itemTable(again.sent, "All time", "loot"), [[["Tablecloth", "×2", "68"]]]);
});

// The Loot tab lists each past session like Past sessions: its map, then without meso totals read,
// "≈" its mesos plus item value and that over the time pickups were read. Fails if a session's
// items are left out, its estimate loses its mark, or a session from before loot tracking gets a
// row of zeros.
test("the Loot tab lists past sessions' estimated profit and profit/hr", () => {
  const { send, sent, now } = play({ seconds: 120 });
  send(item("Tablecloth", now, { item_id: 4000028, sell: 34, count: 2 }));
  send({ type: "action", action: "end", atMs: now + 1 });
  const before = { startedAtMs: T0 - 900_000, endedAtMs: T0 - 600_000, activeMs: 300_000, expGained: 900, expLost: 0, startLevel: 41, endLevel: 41 };
  const saved = last(sent, "save").data;
  const again = run();
  again.send({ type: "start", saved: { ...saved, history: [...saved.history, before] }, atMs: now + 10_000, capture: "capturing", readers: READING });
  const table = section(again.sent, "Past sessions", "loot").children.find((n) => n.kind === "table");
  assert.deepEqual(table.columns, ["Map", "Profit", "Profit/hr"]);
  // 600 mesos and 68 of Tablecloth over two minutes of reading.
  assert.deepEqual(table.rows, [[{ gameText: "aaaa", label: table.rows[0][0].label }, "≈668", "≈20K"]]);
  assert.equal(table.details[0].length, 2, "its day and play time, and its level");
});

// The Loot tab's Past sessions says where sessions come from until one has loot, and has its own
// "Show more" past 20, apart from the Session view's. Fails if the empty line is missing, the
// button doesn't show more or turn into "Show fewer", or one toggle moves the other list.
test("the Loot tab's Past sessions is empty before loot tracking, and has its own Show more", () => {
  const before = { startedAtMs: T0 - 900_000, endedAtMs: T0 - 600_000, activeMs: 300_000, expGained: 900, expLost: 0, startLevel: 41, endLevel: 41 };
  const fresh = run();
  fresh.send({ type: "start", saved: { version: 1, history: [before], current: null }, atMs: T0, capture: "capturing", readers: READING });
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
    maps: [{ k: "aaaa", p: 600_000, x: 100, m: 7, r: 7, l: 600_000, s: 42, e: 42 }],
  });
  const { send, sent } = run();
  send({ type: "start", saved: { version: 1, history: Array.from({ length: 25 }, (_, i) => tracked(i)), current: null }, atMs: T0, capture: "capturing", readers: READING });
  const lootPast = () => section(sent, "Past sessions", "loot").children;
  const lootRows = () => lootPast().find((n) => n.kind === "table").rows.length;
  const lootButton = () => lootPast().find((n) => n.kind === "button");
  const sessionRows = () => pastSessions(sent).find((n) => n.kind === "table").rows.length;
  const sessionButton = () => pastSessions(sent).find((n) => n.action === "toggle-history");
  assert.equal(lootRows(), 20);
  assert.deepEqual([lootButton().label, lootButton().action], ["Show more", "toggle-loot-history"]);
  send({ type: "action", action: "toggle-loot-history", atMs: T0 + 1_000 });
  assert.equal(lootRows(), 25);
  assert.equal(lootButton().label, "Show fewer");
  assert.equal(sessionRows(), 20, "the Session view's list stays as it was");
  assert.equal(sessionButton().label, "Show more");
  send({ type: "action", action: "toggle-history", atMs: T0 + 2_000 });
  assert.equal(sessionRows(), 25);
  assert.equal(lootRows(), 25, "the Loot tab's list stays as it was");
  send({ type: "action", action: "toggle-loot-history", atMs: T0 + 3_000 });
  assert.equal(lootRows(), 20);
  assert.equal(lootButton().label, "Show more");
  assert.equal(sessionRows(), 25);
});

// Items picked up while paused aren't the session's. Fails if they count.
test("items don't count while paused", () => {
  const { send, sent, now } = play({ seconds: 20 });
  send({ type: "action", action: "pause", atMs: now });
  send(item("Tablecloth", now + 1_000, { item_id: 4000028, sell: 34 }));
  assert.equal(section(sent, "Seen picking up"), undefined);
});

// A bad item entry or catalog value in a save must never stop the tracker, and a price range must
// run low to high. Fails if it throws, the good entries go with it, or a bad range is shown.
test("malformed items and catalog entries in a save are dropped", () => {
  const past = {
    startedAtMs: T0 - 600_000,
    endedAtMs: T0 - 300_000,
    activeMs: 60_000,
    expGained: 300,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    maps: [{ k: "aaaa", p: 60_000, x: 300, m: 50, r: 50, l: 60_000, s: 42, e: 42, i: [{ k: "#1", c: 2 }, { k: 5, c: 1 }, { k: "#2", c: -1 }, { k: "=Blue Jangoon…", c: 1 }, { k: "=Hat Accuracy Scroll:…", c: 1 }] }],
  };
  const catalog = {
    "#1": { n: "Snail Shell", t: "Etc", v: 1 },
    "#2": { n: 7 },
    "=Blue Jangoon…": { n: "Blue Jangoon…", t: "Equip", v: null, r: [4200, 7000] },
    "=Hat Accuracy Scroll:…": { n: "Hat Accuracy Scroll:…", t: "Use", v: null, r: [7000, 4200] },
  };
  const { send, sent } = run();
  send({ type: "start", saved: { version: 1, history: [past], current: null, catalog }, atMs: T0, capture: "capturing", readers: READING });
  assert.deepEqual(itemTable(sent, "All time", "loot"), [
    [
      ["Snail Shell", "×2", "2"],
      ["Blue Jangoon…", "×1", "4.2K–7K"],
      ["Hat Accuracy Scroll:…", "×1", null],
    ],
  ]);
});

// The meso ledger: what the meso totals read from the inventory and shop windows say a session
// earned. These tests drive its arithmetic directly with trusted totals, since no reader sends
// them yet.
const MIN = 60_000;
const grinding = (playMs, map = "aaaa") => ({ playMs, map, away: false });
const ledgerOf = (fields) => fields.lg;

// The first total only sets the balance; a later rise on the one grinding map is mesos picked up,
// given to that map with the play time between the two readings. Fails if the opening total counts
// as profit, or the map's exact mesos/hr uses the wrong mesos or time.
test("a rise between two readings on one grinding map is mesos picked up, and that map's rate", () => {
  const fields = {};
  assert.deepEqual(loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0)), { kind: "opening", total: 1_000_000 });
  assert.equal(loot.ledgerRate(ledgerOf(fields)), null, "one reading has no rate");
  assert.deepEqual(loot.addReading(fields, 1_050_000, "inventory", T0 + 30 * MIN, grinding(30 * MIN)), { kind: "picked", amount: 50_000 });
  assert.equal(loot.ledgerProfit(ledgerOf(fields)), 50_000);
  assert.equal(loot.ledgerRate(ledgerOf(fields)), 100_000);
  const map = fields.maps.find((m) => m.k === "aaaa");
  assert.deepEqual([map.fm, map.ft], [50_000, 30 * MIN]);
  assert.equal(loot.fieldRate(map), 100_000);
});

// A sale read in a shop is profit as what shops paid; a fall is spent and never taken from profit,
// so restocking potions doesn't make a map look worse. Fails if a purchase lowers profit, or a
// sale is counted as mesos picked up.
test("shop sales count in profit, and spending shows apart", () => {
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  assert.deepEqual(loot.addReading(fields, 1_188_450, "shop", T0 + 20 * MIN, { playMs: 20 * MIN, map: undefined, away: true }), { kind: "sold", amount: 188_450 });
  assert.deepEqual(loot.addReading(fields, 1_092_450, "shop", T0 + 21 * MIN, { playMs: 21 * MIN, map: undefined, away: true }), { kind: "spent", amount: 96_000 });
  const ledger = ledgerOf(fields);
  assert.deepEqual([ledger.pu, ledger.so, ledger.sp], [0, 188_450, 96_000]);
  assert.equal(loot.ledgerProfit(ledger), 188_450);
  assert.equal(fields.maps, undefined, "a stretch away from the grind is no map's rate");
});

// Without a shop reading, a rise after time in town or across a pause is what a shop paid, not
// mesos picked up, so it never raises a map's mesos/hr. Fails if it counts as picked up or goes
// to a map.
test("a rise after time away from the grind counts as sold", () => {
  const fields = {};
  loot.addReading(fields, 500_000, "inventory", T0, grinding(0));
  assert.deepEqual(loot.addReading(fields, 540_000, "inventory", T0 + 40 * MIN, { playMs: 25 * MIN, map: "aaaa", away: true }), { kind: "sold", amount: 40_000 });
  assert.equal(ledgerOf(fields).pu, 0);
  assert.equal(fields.maps, undefined);
  assert.equal(ledgerOf(fields).pt, 25 * MIN, "Profit/hr's clock leaves the pause out");
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
// counts or the earlier profit is lost.
test("a new segment doesn't count the jump between characters", () => {
  const fields = {};
  loot.addReading(fields, 2_000_000, "inventory", T0, grinding(0));
  loot.addReading(fields, 2_030_000, "inventory", T0 + 10 * MIN, grinding(10 * MIN));
  loot.restartSegment(fields);
  assert.deepEqual(loot.addReading(fields, 40_000_000, "inventory", T0 + 12 * MIN, grinding(12 * MIN)), { kind: "opening", total: 40_000_000 });
  loot.addReading(fields, 40_010_000, "inventory", T0 + 22 * MIN, grinding(22 * MIN));
  const ledger = ledgerOf(fields);
  assert.equal(loot.ledgerProfit(ledger), 40_000);
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

// After End the session waits 30 minutes for a reading that adds what was sold, without moving
// Profit/hr's clock; later, or after the wait is stopped, a reading closes nothing. Fails if a late
// reading lands in the old session, or the sale changes the clock.
test("the close-out adds sales within 30 minutes of End", () => {
  const ended = () => {
    const fields = {};
    loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
    loot.addReading(fields, 1_100_000, "inventory", T0 + 30 * MIN, grinding(30 * MIN));
    loot.waitForCloseOut(fields, T0 + 31 * MIN);
    return fields;
  };
  const inTime = ended();
  assert.deepEqual(loot.closeOut(inTime, 1_228_400, "inventory", T0 + 61 * MIN), { kind: "sold", amount: 128_400 });
  assert.equal(loot.ledgerProfit(ledgerOf(inTime)), 228_400);
  assert.equal(ledgerOf(inTime).pt, 30 * MIN);
  assert.equal(loot.closeOut(inTime, 1_300_000, "inventory", T0 + 62 * MIN), null, "only one close-out");
  const late = ended();
  assert.equal(loot.closeOut(late, 1_228_400, "inventory", T0 + 61 * MIN + 1), null);
  assert.equal(loot.ledgerProfit(ledgerOf(late)), 100_000);
  const stopped = ended();
  loot.stopCloseOut(stopped);
  assert.equal(loot.closeOut(stopped, 1_228_400, "inventory", T0 + 40 * MIN), null);
});

// Undo of End adds the mesos counted since and keeps the clock, and a balance read on the other
// session's clock starts a new segment. Fails if the mesos are lost, time is added, or the next
// total is measured against the wrong balance.
test("Undo merges the ledger without mixing clocks", () => {
  const into = {};
  loot.addReading(into, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(into, 1_100_000, "inventory", T0 + 30 * MIN, grinding(30 * MIN));
  loot.waitForCloseOut(into, T0 + 31 * MIN);
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
  loot.waitForCloseOut(fields, T0 + 11 * MIN);
  const saved = JSON.parse(JSON.stringify(fields.lg));
  assert.deepEqual(loot.validLedger(saved), fields.lg);
  for (const bad of [null, 5, [], { ...saved, pu: -1 }, { ...saved, b: "a lot" }, { ...saved, ho: 3 }]) {
    assert.equal(loot.validLedger(bad), undefined, JSON.stringify(bad));
  }
  assert.deepEqual(loot.validLedger({ ...saved, ho: [[5, T0], [-1, T0], "x"] }).ho, [[5, T0]]);
  const maps = loot.validMaps([{ k: "aaaa", p: 1, x: 0, m: 0, r: 0, l: 0, s: null, e: null, fm: 50_000, ft: 30 * MIN }]);
  assert.deepEqual([maps[0].fm, maps[0].ft], [50_000, 30 * MIN]);
});

/** The Loot section's view (loot-view.ts), as the plugin bundles it. */
const lootView = await import(
  `data:text/javascript;base64,${Buffer.from(
    (
      await esbuild.build({
        entryPoints: [join(REPO, "plugins/src/session-tracker/loot-view.ts")],
        bundle: true,
        write: false,
        format: "esm",
        platform: "neutral",
        target: "es2022",
        alias: { "@mesovibe/sdk": join(REPO, "sdk/src/index.ts") },
        logLevel: "silent",
      })
    ).outputFiles[0].text,
  ).toString("base64")}`
);
const VIEW = { readers: READING, atMs: T0 + 34 * MIN, reading: true, started: true, paused: false, potsPerHour: null };
/** A session with an exact ledger: 1,000,000 then 1,050,000 read 30 minutes apart on one map. */
function exactSession() {
  const fields = {};
  loot.addReading(fields, 1_000_000, "inventory", T0, grinding(0));
  loot.addReading(fields, 1_050_000, "inventory", T0 + 30 * MIN, grinding(30 * MIN));
  return fields;
}
const figuresOf = (node) => Object.fromEntries(node.children.find((n) => n.kind === "keyValue")?.items ?? []);

// With two meso totals read, Loot's headline is exact, unmarked, with how long ago the balance was
// read, and Profit is the ledger's; the mini-HUD gets the same figure without "≈". Fails if an
// exact figure keeps the estimate's mark, the age line is wrong, or the strip and Loot disagree.
test("Loot's headline is exact once two meso totals were read", () => {
  const fields = exactSession();
  const node = lootView.lootSection(fields, {}, VIEW);
  assert.deepEqual(node.children.slice(0, 2), [
    { kind: "stat", label: "Profit/hr", value: "100K", detail: "100,000 mesos of profit per hour" },
    { kind: "text", text: "Mesos read 4 min ago", variant: "muted" },
  ]);
  assert.deepEqual(figuresOf(node), { Profit: "50,000" });
  assert.deepEqual(lootView.lootHeadline(fields, READING), { rate: 100_000, approx: false });
});

// A balance read more than 15 minutes ago mutes the headline and asks for a fresh reading; a
// paused session mutes it and says so. Fails if a stale or paused figure looks current.
test("a stale or paused Loot headline is muted, with words", () => {
  const stale = lootView.lootSection(exactSession(), {}, { ...VIEW, atMs: T0 + 52 * MIN });
  assert.equal(stale.children[0].variant, "muted");
  assert.equal(stale.children[1].text, "Mesos read 22 min ago. Open your inventory to update.");
  const paused = lootView.lootSection(exactSession(), {}, { ...VIEW, paused: true });
  assert.equal(paused.children[0].variant, "muted");
  assert.equal(paused.children[1].text, "Session paused");
});

// With only the opening balance read, the headline stays the notices' estimate and the line gives
// the starting balance. Fails if one reading shows an exact rate or loses the starting balance.
test("with only an opening balance, Loot gives it under the estimate", () => {
  const fields = { maps: [{ k: "aaaa", p: 120_000, x: 600, m: 600, r: 600, l: 120_000, s: 42, e: 42 }] };
  loot.addReading(fields, 1_204_330, "inventory", T0, grinding(0));
  const node = lootView.lootSection(fields, {}, VIEW);
  assert.equal(node.children[0].value, "≈18K");
  assert.equal(node.children[1].text, "Mesos at start 1,204,330. Open your inventory again later for exact profit.");
});

// Once potions are counted, a line under an exact headline gives the net after them; with only an
// estimate there is no net, so a known-low figure is never netted. Fails if net shows on an
// estimate, or subtracts the wrong cost.
test("the net line shows under an exact headline once potions are counted", () => {
  const node = lootView.lootSection(exactSession(), {}, { ...VIEW, potsPerHour: 23_500 });
  assert.equal(node.children[2].text, "76.5K/hr after 23.5K/hr on potions");
  const estimate = { maps: [{ k: "aaaa", p: 120_000, x: 600, m: 600, r: 600, l: 120_000, s: 42, e: 42 }] };
  const plain = lootView.lootSection(estimate, {}, { ...VIEW, potsPerHour: 23_500 });
  assert.ok(!plain.children.some((n) => n.kind === "text" && n.text.includes("on potions")));
});

// Spent shows apart from Profit, and once a shop has paid, the unsold-items estimate goes so the
// same items aren't counted twice. Fails if spending lowers Profit, or Unsold items stays after a
// sale.
test("Spent shows apart, and Unsold items goes once a shop pays", () => {
  const fields = exactSession();
  fields.maps[0].i = [{ k: "#4000028", c: 2 }];
  const catalog = { "#4000028": { n: "Tablecloth", t: "Etc", v: 34 } };
  assert.equal(figuresOf(lootView.lootSection(fields, catalog, VIEW))["Unsold items"], "≈68");
  loot.addReading(fields, 1_150_000, "shop", T0 + 33 * MIN, { playMs: 33 * MIN, map: undefined, away: true });
  loot.addReading(fields, 1_120_000, "shop", T0 + 34 * MIN, { playMs: 34 * MIN, map: undefined, away: true });
  assert.deepEqual(figuresOf(lootView.lootSection(fields, catalog, VIEW)), { Profit: "150,000", Spent: "30,000" });
});

// A held rise shows as a calm line with a "Count it" button that names the rise for screen readers,
// newest first and at most three. Fails if a held rise passes silently, the button can't say what
// it counts, or the list grows without end.
test("held rises list with Count it, newest first, at most three", () => {
  const fields = exactSession();
  for (let i = 1; i <= 4; i++) {
    loot.addReading(fields, 1_050_000 + i * 5_000_000, "inventory", T0 + (30 + i) * MIN, grinding((30 + i) * MIN));
  }
  const node = lootView.lootSection(fields, {}, VIEW);
  const lines = node.children.filter((n) => n.kind === "partial");
  const buttons = node.children.filter((n) => n.kind === "button");
  assert.equal(lines.length, 3);
  assert.equal(lines[0].text, "+5M not counted: too big a jump for farming (storage, a trade or a misread)");
  assert.deepEqual(buttons[0], {
    kind: "button",
    label: "Count it",
    action: `count-held:${T0 + 34 * MIN}`,
    style: "secondary",
    spoken: "Count the 5M rise in Profit",
  });
});

// A past session with an exact ledger shows its exact profit and profit/hr, what it spent and any
// rise not counted, and All time says how long was measured exactly. Fails if an exact session is
// marked as an estimate, its details are missing, or All time hides how much is exact.
test("the Loot tab shows a past session's exact profit, spending and held rises", () => {
  const fields = exactSession();
  loot.addReading(fields, 1_020_000, "inventory", T0 + 40 * MIN, grinding(40 * MIN));
  loot.addReading(fields, 7_020_000, "inventory", T0 + 41 * MIN, grinding(41 * MIN));
  const past = { startedAtMs: T0, endedAtMs: T0 + 41 * MIN, activeMs: 41 * MIN, expGained: 900, expLost: 0, startLevel: 42, endLevel: 42, ...fields };
  const started = run();
  started.send({ type: "start", saved: { version: 1, history: [past], current: null }, atMs: T0 + 50 * MIN, capture: "capturing", readers: READING });
  const table = section(started.sent, "Past sessions", "loot").children.find((n) => n.kind === "table");
  assert.deepEqual(table.rows[0].slice(1), ["50K", "73.2K"]);
  assert.ok(table.details[0].includes("Spent 30K"));
  assert.ok(table.details[0].includes("Not counted: +6M"));
  const figures = allTimeFigures(started.sent);
  assert.equal(figures.Profit, "50K");
  assert.equal(figures["Exact for"], "41m");
});
