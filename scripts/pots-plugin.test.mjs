// The Session Tracker's potion accounting and views, with a stand-in count source wired in
// (scripts/fixtures/synthetic-count-source.mjs): uses per map, their cost, pot cost/hr and net
// loot/hr, the Potions section, the Loot tab's Net/hr, saves and Undo. Also checks that the
// plugin as shipped, with no source wired in, counts and shows no potions.
import { test } from "node:test";
import assert from "node:assert/strict";

import { bundleTracker, run } from "./session-tracker-harness.mjs";
import { load as load070 } from "./fixtures/session-tracker-0.7.0-load.mjs";

const shipped = await bundleTracker();
const synthetic = await bundleTracker({ synthetic: true });

const MEAT = 2010003;
const T0 = 1_800_000_000_000;
const NEEDED = 356_916;
const READING = { notices: "reading", waitingMesos: 0, hpMp: "reading", hpMpMissing: null };

const event = (e, atMs) => ({ type: "event", atMs, event: e });
const exp = (value, atMs) =>
  event({ type: "ExpChanged", level: 42, exp: value, pct_hundredths: Math.round((value * 10_000) / NEEDED), needed: NEEDED }, atMs);
const mesos = (amount, atMs) => event({ type: "MesosGained", amount, late: false, shaped: false }, atMs);
const map = (key, atMs) => event({ type: "MapChanged", map_key: key }, atMs);
const hpMp = (hp, mp, atMs) => event({ type: "HpMpChanged", hp, max_hp: 1386, mp, max_mp: 1140, continuous: true }, atMs);
const lemonPickup = (count, atMs) =>
  event({ type: "ItemGained", item_key: "0000000000000000", count, tab: "Use", item_id: null, name: "Lemon", sell: 15 }, atMs);
const action = (name, atMs) => ({ type: "action", action: name, atMs });
const tick = (atMs) => ({ type: "tick", atMs });

const last = (sent, type) => sent.filter((m) => m.type === type).at(-1);
/** A section of the last view drawn: the Session view, or with `page` that page ("loot"). */
const section = (sent, title, page) =>
  sent
    .filter((m) => m.type === "render" && m.page === page)
    .at(-1)
    .view.find((node) => node.title === title);
const potions = (sent) => section(sent, "Potions");
const headline = (sent) => potions(sent).children[0];
const potTable = (sent) => potions(sent).children.find((n) => n.kind === "table");
const potLines = (sent) => potions(sent).children.filter((n) => n.kind === "partial" || n.kind === "empty").map((n) => n.text);
const lootTable = (sent) => section(sent, "Past sessions", "loot").children.find((n) => n.kind === "table");
const allTime = (sent) => Object.fromEntries(section(sent, "All time", "loot").children.find((n) => n.kind === "keyValue").items);
const renders = (sent) => sent.filter((m) => m.type === "render").length;
/** Seconds from `start` to `end`, `step` apart. */
const every = (start, step, end) => Array.from({ length: Math.floor((end - start) / step) + 1 }, (_, i) => start + i * step);

/**
 * A plugin reading on map "aaaa" for `seconds`: EXP and 50 mesos every 10 s, the first gain at
 * 10 s, and an HP and MP reading every second: HP 660 at the seconds in `meat` and 500 otherwise,
 * MP 650 at the seconds in `lemon` and 500 otherwise, so each listed second is one potion. `also`
 * sends more at a given second.
 */
function play({ seconds = 600, meat = [], lemon = [], readers = READING, bundle = synthetic, also = {} } = {}) {
  const plugin = run(bundle);
  plugin.send({ type: "start", saved: null, atMs: T0, capture: "capturing", readers });
  plugin.send(map("aaaa", T0));
  plugin.send(exp(230_000, T0));
  for (let s = 1; s <= seconds; s++) {
    const at = T0 + s * 1000;
    plugin.send(tick(at));
    if (s % 10 === 0) {
      plugin.send(exp(230_000 + (s / 10) * 63, at));
      plugin.send(mesos(50, at));
    }
    for (const message of also[s]?.(at) ?? []) plugin.send(message);
    plugin.send(hpMp(meat.includes(s) ? 660 : 500, lemon.includes(s) ? 650 : 500, at));
  }
  return { ...plugin, now: T0 + seconds * 1000 };
}

/** Ten minutes with a Meat every minute and a Lemon every two: 18K loot/hr, 9.3K pot cost/hr. */
const grind = (more = {}) => play({ meat: every(30, 60, 600), lemon: every(60, 120, 600), ...more });

/** A past session as saved: ten minutes on "aaaa", 18K loot/hr, and ten Meat if `pots`. */
const pastSession = (startedAtMs, { pots = true, mesos = 3_000 } = {}) => ({
  startedAtMs,
  endedAtMs: startedAtMs + 600_000,
  activeMs: 600_000,
  expGained: 3_780,
  expLost: 0,
  startLevel: 42,
  endLevel: 42,
  maps: [
    {
      k: "aaaa",
      p: 600_000,
      x: 3_780,
      m: mesos,
      r: mesos,
      l: 600_000,
      s: 42,
      e: 42,
      ...(pots ? { u: [{ k: MEAT, c: 10 }], h: 600_000 } : {}),
    },
  ],
  ...(pots ? { ps: { [MEAT]: "rises" }, pt: 800 } : {}),
});

const startWith = (saved) => {
  const plugin = run(synthetic);
  plugin.send({ type: "start", saved, atMs: T0 + 3_600_000, capture: "capturing", readers: READING });
  return plugin;
};

// With no count source wired in, as shipped, nothing about potions shows or is saved. Fails if
// the Potions section or the Net/hr column appears before anything counts potions, or a session
// saves read time that would later read as "no potions used".
test("as shipped, no potions are counted or shown", () => {
  const { send, sent, now } = play({ bundle: shipped, meat: every(30, 60, 600) });
  assert.equal(potions(sent), undefined);
  send(action("end", now));
  assert.deepEqual(lootTable(sent).columns, ["Map", "Loot gained", "Loot/hr"]);
  assert.deepEqual(Object.keys(allTime(sent)), ["Played", "Mesos", "Loot/hr"]);
  const past = last(sent, "save").data.history[0];
  assert.equal(past.maps[0].h, undefined);
  assert.equal(past.maps[0].u, undefined);
  assert.equal(past.pt, undefined);
});

// Potions count at their shop price, and pot cost/hr is over the time the HP and MP numbers were
// read, so net loot/hr subtracts like for like. Fails if a use is lost, priced wrong, the rate
// uses the wrong time, or All time leaves out the running session's potions.
test("potions cost their shop price, and net loot/hr is loot/hr minus pot cost/hr", () => {
  const { sent } = grind();
  assert.deepEqual(headline(sent), { kind: "stat", label: "Net loot/hr", value: "8.7K", detail: "18K loot/hr − 9.3K on potions" });
  const table = potTable(sent);
  assert.deepEqual(table.columns, ["Item", "Used", "Cost"]);
  assert.deepEqual(table.rows, [
    ["Meat", "×10", "800"],
    ["Lemon", "×5", "750"],
  ]);
  assert.deepEqual(table.details, [["+160 HP each · 60/hr"], ["+150 MP each · 30/hr"]]);
  assert.deepEqual(potions(sent).children.at(-1), {
    kind: "text",
    text: "Counted from your HP and MP numbers going up. Only Meat and Lemon are counted.",
    variant: "muted",
  });
  const order = sent.filter((m) => m.type === "render" && m.page === undefined).at(-1).view.map((node) => node.title);
  assert.deepEqual(order.slice(-3), ["Items", "Potions", "Past sessions"], "Potions sits after Items");
  assert.equal(allTime(sent)["Pot cost"], "1.55K", "All time counts the running session's potions");
});

// The headline stays one stat in one place in every state, so the panel doesn't jump. Fails if a
// state draws another kind of node, or shows a rate before a minute.
test("the headline is a stat before a minute, without loot/hr, and with net", () => {
  const early = play({ seconds: 40, meat: [20] });
  assert.deepEqual(headline(early.sent), { kind: "stat", label: "Net loot/hr", value: null, detail: "Net loot/hr shows after a minute." });
  assert.deepEqual(potTable(early.sent).details, [["+160 HP each"]], "no uses/hr before a minute either");
  const learning = grind({ readers: { ...READING, notices: "learning" } });
  assert.deepEqual(headline(learning.sent), { kind: "stat", label: "Pot cost/hr", value: "9.3K", detail: "Net shows with loot/hr" });
  const none = play({ seconds: 120 });
  assert.deepEqual(headline(none.sent), { kind: "stat", label: "Net loot/hr", value: "18K", detail: "No potions used yet" });
  assert.equal(potTable(none.sent), undefined);
});

// Maybe uses cost nothing, so the cost is a lower bound and net an upper one; heals from others
// say so too. Fails if a maybe use is priced, or net claims to be exact when it can't be.
test("maybe uses and heals from others make net an upper bound", () => {
  const { send, sent, now } = grind();
  send(event({ type: "TestMaybe", item: MEAT, count: 2 }, now));
  assert.deepEqual(headline(sent), {
    kind: "stat",
    label: "Net loot/hr",
    value: "8.7K",
    detail: "At most: 18K loot/hr − 9.3K+ on potions",
  });
  assert.deepEqual(potTable(sent).rows[0], ["Meat", "×10", "800"]);
  assert.deepEqual(potTable(sent).details[0], ["+160 HP each · 60/hr · +2 maybe"]);
  send(event({ type: "TestHealing", resource: "hp", on: true }, now));
  send(tick(now + 1000));
  assert.deepEqual(potLines(sent), ["Some healing isn't from potions (a Heal, Drain or MP Eater), so potions may be undercounted."]);
});

// Time when heals from others may have hidden potions keeps net an upper bound after they stop,
// and with no potion counted yet. Fails if that time isn't kept, or net then claims to be exact.
test("heals from others keep net an upper bound after they stop", () => {
  const heal = (on) => (at) => [event({ type: "TestHealing", resource: "hp", on }, at)];
  const none = play({ seconds: 120, also: { 60: heal(true) } });
  assert.deepEqual(headline(none.sent), { kind: "stat", label: "Net loot/hr", value: "18K", detail: "At most: no potions counted yet" });
  const { send, sent, now } = grind({ also: { 100: heal(true), 200: heal(false) } });
  assert.deepEqual(potLines(sent), [], "the healing line goes when the heals stop");
  assert.deepEqual(headline(sent), { kind: "stat", label: "Net loot/hr", value: "8.7K", detail: "At most: 18K loot/hr − 9.3K on potions" });
  send(action("end", now));
  assert.equal(last(sent, "save").data.history[0].maps[0].hm, 100_000);
});

// Spending more on potions than picking up shows a negative net in plain text, never an error
// color. Fails if it's clamped to zero or loses its sign.
test("net below zero shows with a minus sign", () => {
  const { sent } = play({ meat: every(20, 20, 600), lemon: every(10, 20, 600) });
  assert.deepEqual(headline(sent), { kind: "stat", label: "Net loot/hr", value: "−23.4K", detail: "more spent on potions than picked up" });
});

// A Lemon picked up this session is worth what a shop pays for it, not what a shop charges, and
// it may still be in the bag, so net is at most what's shown. Fails if a pickup with no item ID
// isn't matched by name, every use is priced as picked up, or more are than were drunk.
test("a potion picked up this session costs its NPC sell price", () => {
  const { sent } = play({ lemon: [60, 180], also: { 100: (at) => [lemonPickup(1, at)] } });
  assert.deepEqual(potTable(sent).rows, [["Lemon", "×2", "165"]]);
  assert.deepEqual(potTable(sent).details, [["+150 MP each · 12/hr", "1 picked up, at NPC sell price"]]);
  assert.match(headline(sent).detail, /^At most: /);
  // Picking up more than were drunk prices only the ones drunk.
  const more = play({ lemon: [60], also: { 30: (at) => [lemonPickup(3, at)] } });
  assert.deepEqual(potTable(more.sent).rows, [["Lemon", "×1", "15"]]);
  assert.deepEqual(potTable(more.sent).details, [["+150 MP each · 6/hr", "1 picked up, at NPC sell price"]]);
});

// Each HP and MP reading state says why potions aren't counted, and the reading line comes before
// the healing one. Fails if a state has no line, or the lines' order changes.
test("each HP and MP reading state has its line, reading first", () => {
  const { send, sent, now } = grind();
  const say = (readers, at) => {
    send({ type: "readers", readers: { ...READING, ...readers }, atMs: at });
    return potLines(sent);
  };
  assert.deepEqual(say({ hpMp: "learning" }, now), ["Learning your HP and MP digits from your EXP gains."]);
  assert.deepEqual(say({ hpMp: "missing_digits", hpMpMissing: "7" }, now), ["To read your HP and MP, MesoVibe waits for an EXP gain that shows a 7."]);
  assert.deepEqual(say({ hpMp: "missing_digits", hpMpMissing: "045" }, now), [
    "To read your HP and MP, MesoVibe waits for an EXP gain that shows a digit it hasn't seen yet: 0, 4 or 5.",
  ]);
  assert.deepEqual(say({ hpMp: "not_found" }, now), [
    "Can't see your HP and MP numbers. Keep the bar at the bottom of the game in view. See What MesoVibe sees.",
  ]);
  send(event({ type: "TestHealing", resource: "mp", on: true }, now));
  assert.deepEqual(say({ hpMp: "covered" }, now), [
    "Potions aren't counted while your HP and MP numbers are covered.",
    "Some healing isn't from potions (a Heal, Drain or MP Eater), so potions may be undercounted.",
  ]);
  assert.deepEqual(say({ hpMp: "off" }, now), ["Some healing isn't from potions (a Heal, Drain or MP Eater), so potions may be undercounted."]);
});

// HP and MP readings come four times a second; only a counted potion redraws the panel. Fails if
// every reading rebuilds it.
test("HP and MP readings redraw nothing until a potion is counted", () => {
  const { send, sent, now } = play({ seconds: 120 });
  const before = renders(sent);
  for (let i = 1; i <= 4; i++) send(hpMp(500 - i * 10, 500, now + i * 250));
  assert.equal(renders(sent), before);
  send(hpMp(620, 500, now + 1_250));
  assert.ok(renders(sent) > before);
  assert.deepEqual(potTable(sent).rows, [["Meat", "×1", "80"]]);
});

// Pot cost/hr's time grows only while the HP and MP numbers are read: five minutes at full HP
// count five minutes, and covered numbers or a pause add none. Fails if covered or paused time
// thins out pot cost/hr.
test("potions' read time grows only while HP and MP are read and the session isn't paused", () => {
  const { send, sent, now } = play({ seconds: 300 });
  send({ type: "readers", readers: { ...READING, hpMp: "covered" }, atMs: now });
  for (let s = 1; s <= 60; s++) send(tick(now + s * 1000));
  send({ type: "readers", readers: READING, atMs: now + 60_000 });
  send(action("pause", now + 60_000));
  for (let s = 61; s <= 120; s++) send(tick(now + s * 1000));
  send(action("resume", now + 120_000));
  for (let s = 121; s <= 180; s++) send(tick(now + s * 1000));
  send(action("end", now + 180_000));
  const entry = last(sent, "save").data.history[0].maps[0];
  assert.equal(entry.h, 360_000, "five minutes before, one after resuming");
  assert.equal(entry.p, 420_000, "covered time is still play");
});

// No potion is counted across a pause, nor while paused. Fails if the first reading after Resume
// is compared with the last one before Pause, or a potion drunk while paused counts.
test("a pause counts no potion across it or during it", () => {
  const { send, sent, now } = play({ seconds: 120 });
  send(action("pause", now + 1_000));
  send(hpMp(660, 500, now + 2_000));
  // The source counts this rise and the maybe use; the paused session must not.
  send(hpMp(820, 500, now + 2_500));
  send(event({ type: "TestMaybe", item: MEAT, count: 1 }, now + 2_600));
  send(action("resume", now + 3_000));
  send(hpMp(820, 500, now + 4_000));
  assert.equal(potTable(sent), undefined);
  send(hpMp(980, 500, now + 5_000));
  assert.deepEqual(potTable(sent).rows, [["Meat", "×1", "80"]]);
});

// A potion drunk before the session's first EXP gain, on the walk to a map or in town, isn't the
// grind's cost. Fails if it's counted, now or once EXP follows.
test("potions before the first EXP gain aren't counted", () => {
  const { send, sent, now } = play({ seconds: 120, meat: [5] });
  assert.equal(potTable(sent), undefined);
  assert.deepEqual(headline(sent).detail, "No potions used yet");
  send(action("end", now));
  assert.equal(last(sent, "save").data.history[0].maps[0].u, undefined);
});

// A potion counts on the map it was drunk on, with that map's read time, and the session adds the
// maps up. Fails if a use or its time goes to the wrong map, or only one map is totalled.
test("each potion counts on its map, and the session adds the maps up", () => {
  const { send, sent, now } = play({ meat: every(30, 60, 600), also: { 300: (at) => [map("bbbb", at)] } });
  assert.deepEqual(potTable(sent).rows, [["Meat", "×10", "800"]]);
  send(action("end", now));
  const maps = last(sent, "save").data.history[0].maps.map(({ k, u, h }) => ({ k, u, h }));
  assert.deepEqual(maps, [
    { k: "aaaa", u: [{ k: MEAT, c: 5 }], h: 300_000 },
    { k: "bbbb", u: [{ k: MEAT, c: 5 }], h: 300_000 },
  ]);
  assert.equal(lootTable(sent).details[0].at(-1), "Pots 800: Meat ×10");
  assert.equal(lootTable(sent).rows[0][3], "13.2K");
});

// End keeps the session's potions with their cost frozen, and the Loot tab shows its net loot/hr
// and a pots line. Fails if the Net/hr column, the line or All time's rows are missing or wrong, or
// the next session shows a Potions section before its first gain.
test("an ended session shows its net loot/hr and potions in the Loot tab", () => {
  const { send, sent, now } = grind();
  send(action("end", now));
  const past = last(sent, "save").data.history[0];
  assert.equal(past.pt, 1_550);
  assert.deepEqual(past.ps, { [MEAT]: "rises", 2010005: "rises" });
  assert.equal(potions(sent), undefined, "the next session's Potions section waits for its first gain");
  const table = lootTable(sent);
  assert.deepEqual(table.columns, ["Map", "Loot gained", "Loot/hr", "Net/hr"]);
  assert.deepEqual(table.rows[0].slice(1), ["3K", "18K", "8.7K"]);
  assert.equal(table.details[0].at(-1), "Pots 1.55K: Meat ×10, Lemon ×5");
  const figures = allTime(sent);
  assert.equal(figures["Pots counted for"], "10m");
  assert.equal(figures["Pot cost"], "1.55K");
  assert.equal(figures["Net loot/hr"], "8.7K");
});

// Maybe uses leave a past session's pot cost a lower bound, in its pots line and in All time.
// Fails if the "+" is lost once the session ends, so the cost reads as exact.
test("a past session's cost keeps its + for maybe uses", () => {
  const { send, sent, now } = play({ meat: every(30, 60, 600) });
  send(event({ type: "TestMaybe", item: 2010005, count: 3 }, now));
  send(action("end", now));
  assert.equal(lootTable(sent).details[0].at(-1), "Pots 800+: Meat ×10");
  assert.equal(allTime(sent)["Pot cost"], "800+");
});

// A past session keeps the pot cost it ended with, so a later price change never rewrites it.
// Fails if a saved session is re-priced from today's prices.
test("a past session keeps the cost it was saved with", () => {
  const { sent } = startWith({ version: 1, history: [{ ...pastSession(T0), pt: 1_000 }], current: null });
  assert.equal(lootTable(sent).details[0].at(-1), "Pots 1K: Meat ×10");
  assert.equal(lootTable(sent).rows[0][3], "12K");
});

// Sessions saved before potions were counted have no pot time: Net/hr says "—", and All time's net
// covers only the sessions that counted potions, saying how long that was. Fails if old sessions
// count as having used no potions, which would inflate net.
test("sessions without potion counts show — and stay out of All time's net", () => {
  const old = pastSession(T0 - 7_200_000, { pots: false, mesos: 60_000 });
  const { sent } = startWith({ version: 1, history: [pastSession(T0), old], current: null });
  assert.deepEqual(
    lootTable(sent).rows.map((row) => row[3]),
    ["13.2K", "—"],
  );
  const figures = allTime(sent);
  assert.equal(figures["Loot/hr"], "189K", "loot/hr covers both");
  assert.equal(figures["Pots counted for"], "10m");
  assert.equal(figures["Pot cost"], "800");
  assert.equal(figures["Net loot/hr"], "13.2K", "net covers only the session that counted potions");
});

// Undo of End puts the session back with the potions counted since, and no extra read time.
// Fails if those potions are lost, or the undo window's time thins pot cost/hr.
test("Undo after End keeps potions counted in its window", () => {
  const { send, sent, now } = grind();
  send(action("end", now));
  send(exp(233_843, now + 1_000));
  send(hpMp(660, 500, now + 1_500));
  send(action("undo-end", now + 2_000));
  assert.deepEqual(potTable(sent).rows[0], ["Meat", "×11", "880"]);
  send(action("end", now + 2_000));
  const entry = last(sent, "save").data.history[0].maps[0];
  assert.equal(entry.h, 600_000);
  assert.deepEqual(entry.u[0], { k: MEAT, c: 11 });
});

// Potion fields survive a save and a restart; a malformed one is dropped and the session kept.
// Fails if a restart loses potions, one bad field loses the session or stops the tracker, or a bad
// saved cost reaches the Loot tab.
test("saves keep potion fields, and drop a malformed one with the session kept", () => {
  const { send, sent, now } = grind();
  send(action("pause", now));
  const saved = last(sent, "save").data;
  assert.deepEqual(saved.current.maps[0].u, [
    { k: MEAT, c: 10 },
    { k: 2010005, c: 5 },
  ]);
  const again = startWith(saved);
  assert.equal(lootTable(again.sent).details[0].at(-1), "Pots 1.55K: Meat ×10, Lemon ×5", "a session left running ends with its potions");

  const bad = pastSession(T0);
  bad.maps[0].u = [{ k: "Meat", c: 1 }, null, { k: MEAT, c: -2 }];
  bad.maps[0].h = -5;
  bad.ps = 5;
  bad.pt = "a lot";
  const broken = startWith({ version: 1, history: [bad], current: null });
  const table = lootTable(broken.sent);
  assert.equal(table.rows.length, 1, "the session is kept");
  assert.equal(table.rows[0][3], "—");
  assert.equal(table.details[0].some((line) => line.startsWith("Pots")), false);

  // A malformed saved cost on a session with good counts is priced again at today's prices.
  for (const pt of ["a lot", -5, null]) {
    const repriced = lootTable(startWith({ version: 1, history: [{ ...pastSession(T0), pt }], current: null }).sent);
    assert.equal(repriced.details[0].at(-1), "Pots 800: Meat ×10", `saved cost ${JSON.stringify(pt)}`);
    assert.equal(repriced.rows[0][3], "13.2K", `saved cost ${JSON.stringify(pt)}`);
  }
});

// A player who goes back to MesoVibe 0.7.0 keeps their sessions: its loader reads a save with
// potion fields without failing, and this build reads what it saves back. Fails if a new field
// makes the old loader drop sessions.
test("MesoVibe 0.7.0 reads a save with potion fields", () => {
  const { send, sent, now } = grind();
  send(action("end", now));
  const saved = JSON.parse(JSON.stringify(last(sent, "save").data));
  const old = load070(saved);
  assert.equal(old.history.length, 1);
  assert.equal(old.history[0].maps[0].m, 3_000);
  assert.equal(old.history[0].maps[0].u, undefined, "0.7.0 drops what it doesn't know");
  const back = startWith(JSON.parse(JSON.stringify(old)));
  assert.equal(lootTable(back.sent).rows.length, 1);
  assert.equal(lootTable(back.sent).rows[0][3], "—");
});
