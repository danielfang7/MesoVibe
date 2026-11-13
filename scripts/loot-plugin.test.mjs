// The Session Tracker's loot as the host runs it: mesos and time by map, a map's mesos/hr, the
// lines about reading pickups, and Past sessions rows named for a map. Bundled from source with the SDK and
// driven only through the SDK's messages, like session-tracker-plugin.test.mjs. Past sessions
// doesn't show mesos/hr yet, so its tests read the saved session through the loot arithmetic.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const esbuild = createRequire(join(REPO, "plugins", "package.json"))("esbuild");

const bundle = (
  await esbuild.build({
    entryPoints: [join(REPO, "plugins/src/session-tracker/index.ts")],
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    alias: { "@mesovibe/sdk": join(REPO, "sdk/src/index.ts") },
    logLevel: "silent",
  })
).outputFiles[0].text;

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

function run() {
  const sent = [];
  const scope = { postMessage: (message) => sent.push(structuredClone(message)), onmessage: null };
  scope.globalThis = scope;
  for (const name of ["structuredClone", "Date", "Intl", "Math", "JSON", "Number", "Array", "Object", "String", "Map", "Set"]) {
    scope[name] = globalThis[name];
  }
  vm.createContext(scope);
  vm.runInContext(bundle, scope);
  return { send: (data) => scope.onmessage({ data }), sent };
}

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

const section = (sent, title) => last(sent, "render").view.find((node) => node.title === title);
const thisSession = (sent) => section(sent, "This session")?.children ?? [];
const items = (sent) => Object.fromEntries(thisSession(sent).find((n) => n.kind === "keyValue")?.items ?? []);
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
  assert.equal(items(sent).Mesos, "600");
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
  assert.equal(items(plugin.sent).Mesos, (12 * 50 + 12 * 500).toLocaleString("en-US"));
  // Read for two minutes at 300 a minute: the covered 500s don't count, nor do their minutes.
  assert.equal(endedMesosRate({ ...plugin, now: T0 + 240_000 }), 18_000);
});

// Time the notices can't be trusted isn't read time. Fails if a map gets a mesos rate from time
// spent learning the letters or under the party list.
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
  assert.equal(items(plugin.sent).Mesos, "80", "joined");

  const late = run();
  late.send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers: READING });
  late.send(map("aaaa", T0));
  late.send(exp(230_000, T0));
  late.send(mesos(80, T0 + 1_000));
  late.send({ type: "tick", atMs: T0 + 400_000 });
  late.send(exp(230_063, T0 + 400_000));
  assert.equal(items(late.sent).Mesos, undefined, "dropped after five minutes");
});

// What the notices are doing shows under This session; amounts still being read don't, since
// they're added once their digits are known. Fails if a state passes silently, or the waiting
// line or its "+" comes back.
test("learning, missed and wrong-digit lines, and none for amounts still being read", () => {
  const learning = play({ seconds: 20, readers: { notices: "learning", waitingMesos: 0 } });
  assert.deepEqual(lines(learning.sent), ["Learning the pickup text. Mesos show after a few kills."]);
  learning.send({ type: "tick", atMs: learning.now + 5 * 60_000 });
  assert.match(lines(learning.sent)[0], /^Still learning the pickup text: it learns from EXP lines/);

  const { send, sent, now } = play({ seconds: 20 });
  send({ type: "readers", readers: { notices: "reading", waitingMesos: 2 }, atMs: now });
  assert.deepEqual(lines(sent), []);
  assert.equal(items(sent).Mesos, "100");
  send({ type: "readers", readers: READING, atMs: now + 1_000 });
  send(missed("not_learned", now + 2_000));
  assert.deepEqual(lines(sent), [], "an amount waiting for a digit isn't missed");
  send(missed("unaligned", now + 3_000));
  assert.deepEqual(lines(sent), ["Some pickups may be missing: MesoVibe couldn't read every notice."]);
  assert.equal(items(sent).Mesos, "100+");
  send(missed("shapes_wrong", now + 4_000));
  assert.deepEqual(lines(sent), ["Some mesos amounts may be wrong: a digit was misread and is no longer used."]);

  const party = play({ seconds: 20, readers: { notices: "blocked", waitingMesos: 0 } });
  assert.deepEqual(lines(party.sent), ["Pickups aren't read in a party yet: the party list covers the notices."]);
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
  assert.match(last(sent, "notice").text, /^Saved: [\d,]+ EXP and 600 mesos in /);
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
  assert.equal(items(sent).Mesos, "100");
  send({ type: "action", action: "resume", atMs: now + 2_000 });
  send(mesos(70, now + 3_000));
  assert.equal(items(sent).Mesos, "170");
});

// Undo puts the ended session back with the pickups made since. Fails if they're lost.
test("Undo after End keeps pickups made in its window", () => {
  const { send, sent, now } = play({ seconds: 60 });
  send({ type: "action", action: "end", atMs: now });
  send(exp(230_400, now + 2_000));
  send(mesos(70, now + 3_000));
  send({ type: "action", action: "undo-end", atMs: now + 5_000 });
  assert.equal(items(sent).Mesos, "370");
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

// Sessions past the 500 kept go into All time, never lost. Fails if a dropped session's mesos
// vanish from the save.
test("a session past the 500 kept adds its mesos to All time", () => {
  const session = (i) => ({
    startedAtMs: T0 - (i + 1) * 3_600_000,
    endedAtMs: T0 - (i + 1) * 3_600_000 + 600_000,
    activeMs: 600_000,
    expGained: 100,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    maps: [{ k: "aaaa", p: 600_000, x: 100, m: 7, r: 7, l: 600_000, s: 42, e: 42 }],
  });
  const history = Array.from({ length: 500 }, (_, i) => session(i));
  const { send, sent, now } = (() => {
    const plugin = run();
    plugin.send({ type: "start", saved: { version: 1, history, current: null }, atMs: T0, capture: "capturing", readers: READING });
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
  assert.deepEqual(saved.allTime, { mesos: 7, playedMs: 600_000 });
});
