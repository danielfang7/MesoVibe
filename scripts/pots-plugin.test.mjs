// The Session Tracker's potion accounting and views, with a stand-in count source wired in
// (scripts/fixtures/synthetic-count-source.mjs): uses per map, their cost, pot cost/hr, the
// Potions section, the net after potions in Loot, the Loot tab's pots lines, saves and Undo. Also
// checks that the plugin as shipped counts and shows no potions on a host that doesn't read the
// quick slots (scripts/quick-slot-source.test.mjs checks it on one that does).
import { test } from "node:test";
import assert from "node:assert/strict";

import { bundleTracker, run } from "./session-tracker-harness.mjs";
import { load as load070 } from "./fixtures/session-tracker-0.7.0-load.mjs";

const shipped = await bundleTracker();
const synthetic = await bundleTracker({ synthetic: true });

const MEAT = 2010003;
const T0 = 1_800_000_000_000;
const NEEDED = 356_916;
/** A host reading the HP and MP numbers and the meso total. */
const READING = { hpMp: "reading", hpMpMissing: null, mesos: "reading" };

const event = (e, atMs) => ({ type: "event", atMs, event: e });
const exp = (value, atMs) =>
  event({ type: "ExpChanged", level: 42, exp: value, pct_hundredths: Math.round((value * 10_000) / NEEDED), needed: NEEDED }, atMs);
/** A trusted meso total read in the inventory. */
const total = (amount, atMs) => event({ type: "MesoChanged", total: amount, source: "inventory" }, atMs);
const map = (key, atMs) => event({ type: "MapChanged", map_key: key }, atMs);
const hpMp = (hp, mp, atMs) => event({ type: "HpMpChanged", hp, max_hp: 1386, mp, max_mp: 1140, continuous: true }, atMs);
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
 * A plugin reading on map "aaaa" for `seconds`: EXP every 10 s, and an HP and MP reading every
 * second: HP 660 at the seconds in `meat` and 500 otherwise, MP 650 at the seconds in `lemon` and
 * 500 otherwise, so each listed second is one potion. The first gain comes at once, or with `lead`
 * at 10 s, after a lead that is no potion time. `also` sends more at a given second.
 */
function play({ seconds = 600, meat = [], lemon = [], readers = READING, bundle = synthetic, also = {}, lead = false, saved = null } = {}) {
  const plugin = run(bundle);
  plugin.send({ type: "start", saved, atMs: T0, capture: "capturing", readers });
  plugin.send(map("aaaa", T0));
  plugin.send(exp(lead ? 230_000 : 229_990, T0));
  if (!lead) plugin.send(exp(230_000, T0));
  for (let s = 1; s <= seconds; s++) {
    const at = T0 + s * 1000;
    plugin.send(tick(at));
    if (s % 10 === 0) plugin.send(exp(230_000 + (s / 10) * 63, at));
    for (const message of also[s]?.(at) ?? []) plugin.send(message);
    plugin.send(hpMp(meat.includes(s) ? 660 : 500, lemon.includes(s) ? 650 : 500, at));
  }
  return { ...plugin, now: T0 + seconds * 1000 };
}

/** Ten minutes with a Meat every minute and a Lemon every two: 9.3K pot cost/hr. */
const grind = (more = {}) => play({ meat: every(30, 60, 600), lemon: every(60, 120, 600), ...more });

/**
 * A past session as saved: ten minutes on "aaaa", the mesos its pickup notices counted (an older
 * build's), and ten Meat if `pots`.
 */
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

// As shipped, potions are counted from the quick slots, so from a host that doesn't read them
// nothing about potions shows or is saved. Fails if the Potions section appears with nothing to
// count from, or a session saves read time that would later read as "no potions used".
test("as shipped, no potions are counted or shown without the quick slots", () => {
  const { send, sent, now } = play({ bundle: shipped, meat: every(30, 60, 600) });
  assert.equal(potions(sent), undefined);
  send(action("end", now));
  assert.deepEqual(lootTable(sent).columns, ["Map", "Mesos/hr", "Profit"]);
  assert.deepEqual(section(sent, "All time", "loot").children, [
    { kind: "empty", text: "Mesos you pick up and sell add up here, over every session." },
  ]);
  const past = last(sent, "save").data.history[0];
  assert.equal(past.maps[0].h, undefined);
  assert.equal(past.maps[0].u, undefined);
  assert.equal(past.pt, undefined);
});

// Potions count at their shop price, and pot cost/hr is over the time the HP and MP numbers were
// read. Fails if a use is lost, priced wrong, the rate uses the wrong time, or All time leaves out
// the running session's potions.
test("potions cost their shop price, and pot cost/hr is over the time they were counted", () => {
  const { sent } = grind();
  assert.deepEqual(headline(sent), { kind: "stat", label: "Pot cost/hr", value: "9.3K", detail: "9,300 mesos on potions per hour" });
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
  assert.deepEqual(order.slice(-3), ["Loot", "Potions", "Past sessions"], "Potions sits after Loot");
  assert.equal(allTime(sent)["Pot cost"], "1.55K", "All time counts the running session's potions");
});

// With potions counted and two meso totals read, Loot gives the net after potions: Mesos/hr less
// pot cost/hr. Fails if the net is missing once both rates are known, or takes the wrong cost.
test("Loot gives mesos/hr after potions once both are known", () => {
  const net = (sent) => section(sent, "Loot").children.find((n) => n.kind === "text" && n.text.includes("on potions"))?.text;
  const { sent } = grind({ also: { 60: (at) => [total(1_000_000, at)], 600: (at) => [total(1_018_000, at)] } });
  // 18,000 mesos over nine minutes of play is 120K/hr, less 9.3K/hr on potions.
  assert.equal(net(sent), "111K/hr after 9.3K/hr on potions");
  const unread = grind();
  assert.equal(net(unread.sent), undefined, "no net without a mesos rate");
});

// Pot cost/hr is one stat in every state, with a visible line saying why there's no rate yet.
// Fails if a state changes the node, shows a rate early, or the line is missing, hidden or untrue.
test("the headline is pot cost/hr in every state, with or without loot/hr", () => {
  const early = play({ seconds: 40, meat: [20] });
  assert.deepEqual(headline(early.sent), { kind: "stat", label: "Pot cost/hr", value: null });
  assert.deepEqual(potions(early.sent).children[1], { kind: "empty", text: "Pot cost/hr shows after a minute." });
  // While HP and MP aren't read the minute never comes, and the reading line says why.
  const unread = play({ seconds: 40, meat: [20], readers: { ...READING, hpMp: "learning" } });
  assert.deepEqual(potLines(unread.sent), ["Learning your HP and MP digits from your EXP gains."]);
  assert.deepEqual(potTable(early.sent).details, [["+160 HP each"]], "no uses/hr before a minute either");
  const learning = grind({ readers: { ...READING, mesos: "learning" } });
  assert.deepEqual(headline(learning.sent), { kind: "stat", label: "Pot cost/hr", value: "9.3K", detail: "9,300 mesos on potions per hour" });
  assert.equal(potions(learning.sent).children[1].kind, "table", "no waiting line once there's a rate");
  const none = play({ seconds: 120 });
  assert.deepEqual(headline(none.sent), { kind: "stat", label: "Pot cost/hr", value: "0" });
  assert.deepEqual(potions(none.sent).children[1], { kind: "empty", text: "No potions used yet." });
  assert.equal(potTable(none.sent), undefined);
});

// The mini-HUD gets pot cost/hr once potions are counted, and with none counted it asks for the
// pet's potions in a quick slot; nothing either way before the first gain or where potions can't
// be counted. Fails if the box shows 0 for potions it can't see, or asks where it can't help.
test("the summary carries pot cost/hr, or asks for the pet's potions in a quick slot", () => {
  const summary = (sent) => last(sent, "summary").summary;
  const counted = grind();
  assert.equal(summary(counted.sent).potCostPerHour, 9_300);
  assert.equal(summary(counted.sent).potsUncounted, false);
  const none = play({ seconds: 120 });
  assert.equal(summary(none.sent).potCostPerHour, null);
  assert.equal(summary(none.sent).potsUncounted, true);
  const early = play({ seconds: 40, meat: [20] });
  assert.deepEqual([summary(early.sent).potCostPerHour, summary(early.sent).potsUncounted], [null, false], "no rate before a minute");
  const shipped_ = play({ bundle: shipped, seconds: 120 });
  assert.deepEqual([summary(shipped_.sent).potCostPerHour, summary(shipped_.sent).potsUncounted], [null, false], "no quick slots read");
});

// Maybe uses cost nothing, so the cost is a lower bound; heals from others say potions may be
// undercounted. Fails if a maybe use is priced, or the cost claims to be exact when it can't be.
test("maybe uses make pot cost a lower bound, and heals from others say so", () => {
  const { send, sent, now } = grind();
  send(event({ type: "TestMaybe", item: MEAT, count: 2 }, now));
  assert.deepEqual(headline(sent), {
    kind: "stat",
    label: "Pot cost/hr",
    value: "9.3K+",
    detail: "At least 9,300 mesos on potions per hour",
  });
  assert.deepEqual(potTable(sent).rows[0], ["Meat", "×10", "800"]);
  assert.deepEqual(potTable(sent).details[0], ["+160 HP each · 60/hr · +2 maybe"]);
  send(event({ type: "TestHealing", resource: "hp", on: true }, now));
  send(tick(now + 1000));
  assert.deepEqual(potLines(sent), ["Some healing isn't from potions (a Heal, Drain or MP Eater), so potions may be undercounted."]);
});

// Time when heals from others may have hidden potions is saved with the session after the heals
// stop. Fails if that time isn't kept, or the healing line stays after the heals stop.
test("heal time from others is kept after the heals stop", () => {
  const heal = (on) => (at) => [event({ type: "TestHealing", resource: "hp", on }, at)];
  const { send, sent, now } = grind({ also: { 100: heal(true), 200: heal(false) } });
  assert.deepEqual(potLines(sent), [], "the healing line goes when the heals stop");
  send(action("end", now));
  assert.equal(last(sent, "save").data.history[0].maps[0].hm, 100_000);
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
// count five minutes, and no other reading state or a pause adds any. Pausing on its own is off,
// as these minutes have no EXP. Fails if that time thins out pot cost/hr.
test("potions' read time grows only while HP and MP are read and the session isn't paused", () => {
  const { send, sent, now } = play({ seconds: 300, saved: { version: 1, history: [], current: null, auto: false } });
  // Ten seconds in each state that isn't reading, a minute in all.
  const states = ["covered", "learning", "not_found", "missing_digits", "off", "covered"];
  states.forEach((hpMp, i) => {
    send({ type: "readers", readers: { ...READING, hpMp }, atMs: now + i * 10_000 });
    for (let s = 1; s <= 10; s++) send(tick(now + i * 10_000 + s * 1000));
  });
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

// A potion drunk while paused isn't counted, and one drunk right after Resume is, even when it's
// the first change since Resume. Fails if paused uses count, or Resume forgets the last reading so
// the first rise after it becomes the baseline and is lost.
test("a pause counts no potion during it, and the first one after it", () => {
  const { send, sent, now } = play({ seconds: 120 });
  send(action("pause", now + 1_000));
  // The source counts this rise and the maybe use; the paused session must not.
  send(hpMp(660, 500, now + 2_000));
  send(event({ type: "TestMaybe", item: MEAT, count: 1 }, now + 2_600));
  send(action("resume", now + 3_000));
  assert.equal(potTable(sent), undefined);
  send(hpMp(820, 500, now + 5_000));
  assert.deepEqual(potTable(sent).rows, [["Meat", "×1", "80"]]);
});

// A potion drunk before the session's first EXP gain, on the walk to a map or in town, isn't the
// grind's cost, nor is that time. Fails if it's counted, now or once EXP follows, or the lead
// before the gain adds to the time potions were counted over.
test("potions before the first EXP gain aren't counted, nor that time", () => {
  const { send, sent, now } = play({ seconds: 120, meat: [5], lead: true });
  assert.equal(potTable(sent), undefined);
  assert.ok(potLines(sent).includes("No potions used yet."));
  send(action("end", now));
  const entry = last(sent, "save").data.history[0].maps[0];
  assert.equal(entry.u, undefined);
  assert.equal(entry.h, 110_000, "two minutes, less the 10 s before the first gain");
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
});

// End keeps the session's potions with their cost frozen, and the Loot tab shows a pots line with
// no Net column. Fails if the line or All time's rows are missing or wrong, a Net column comes
// back, or the next session shows a Potions section before its first gain.
test("an ended session shows its potions in the Loot tab", () => {
  const { send, sent, now } = grind();
  send(action("end", now));
  const past = last(sent, "save").data.history[0];
  assert.equal(past.pt, 1_550);
  assert.deepEqual(past.ps, { [MEAT]: "rises", 2010005: "rises" });
  assert.equal(potions(sent), undefined, "the next session's Potions section waits for its first gain");
  const table = lootTable(sent);
  assert.deepEqual(table.columns, ["Map", "Mesos/hr", "Profit"]);
  assert.deepEqual(table.rows[0].slice(1), ["—", "—"], "no meso total read, nothing sold");
  assert.equal(table.details[0].at(-1), "Pots 1.55K: Meat ×10, Lemon ×5");
  const figures = allTime(sent);
  assert.equal(figures["Pots counted for"], "10m");
  assert.equal(figures["Pot cost"], "1.55K");
  assert.equal(figures["Net loot/hr"], undefined);
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
});

// Sessions saved before potions were counted have no pot time: no pots line, and All time's pot
// rows cover only the sessions that counted potions, saying how long that was. Fails if old
// sessions count as having used no potions.
test("sessions without potion counts have no pots line and stay out of All time's pot rows", () => {
  const old = pastSession(T0 - 7_200_000, { pots: false, mesos: 60_000 });
  const { sent } = startWith({ version: 1, history: [pastSession(T0), old], current: null });
  const details = lootTable(sent).details;
  assert.equal(details[0].at(-1), "Pots 800: Meat ×10");
  assert.ok(!details[1].some((line) => line.startsWith("Pots")), "the old session has no pots line");
  const figures = allTime(sent);
  assert.equal(figures["From pickup notices"], "≈63K", "both sessions' notice mesos, apart");
  assert.equal(figures["Pots counted for"], "10m");
  assert.equal(figures["Pot cost"], "800");
});

// Undo of End puts the session back with the potions counted since, and no extra read time.
// Fails if those potions are lost, or the undo window's time thins pot cost/hr.
test("Undo after End keeps potions counted in its window", () => {
  const { send, sent, now } = grind();
  send(action("end", now));
  send(exp(233_843, now + 1_000));
  send(hpMp(660, 500, now + 1_500));
  // A maybe use in the window comes back too, so the cost stays a lower bound.
  send(event({ type: "TestMaybe", item: 2010005, count: 1 }, now + 1_600));
  send(action("undo-end", now + 2_000));
  assert.deepEqual(potTable(sent).rows[0], ["Meat", "×11", "880"]);
  assert.deepEqual(potTable(sent).details[1], ["+150 MP each · 30/hr · +1 maybe"]);
  assert.equal(headline(sent).value, "9.78K+");
  send(action("end", now + 2_000));
  const entry = last(sent, "save").data.history[0].maps[0];
  assert.equal(entry.h, 600_000);
  assert.deepEqual(entry.u[0], { k: MEAT, c: 11 });
  assert.deepEqual(entry.u[1], { k: 2010005, c: 5, y: 1 });
});

// Undo of Reset puts the session back with its potions and any counted since, as Undo of End
// does. Fails if Reset's Undo loses the potions or adds the window's time to pot cost/hr.
test("Undo after Reset puts its potions back", () => {
  const { send, sent, now } = grind();
  send(action("reset", now));
  assert.equal(potions(sent), undefined, "a reset session has no potions to show");
  send(exp(233_843, now + 1_000));
  send(hpMp(660, 500, now + 1_500));
  send(action("undo-reset", now + 2_000));
  assert.deepEqual(potTable(sent).rows, [
    ["Meat", "×11", "880"],
    ["Lemon", "×5", "750"],
  ]);
  send(action("end", now + 2_000));
  const entry = last(sent, "save").data.history[0].maps[0];
  assert.equal(entry.h, 600_000);
  assert.deepEqual(entry.u, [
    { k: MEAT, c: 11 },
    { k: 2010005, c: 5 },
  ]);
});

// A potion counted from rises and its quick slot saves as "both", and the footnote drops "going up".
// Fails if one way overwrites the other, Undo loses its window's source, or slots are ignored.
test("a potion counted from its quick slot too is saved as counted both ways", () => {
  const { send, sent, now } = grind();
  send(event({ type: "TestSlot", item: MEAT, count: 1 }, now));
  assert.deepEqual(potTable(sent).rows[0], ["Meat", "×11", "880"]);
  assert.deepEqual(potions(sent).children.at(-1), {
    kind: "text",
    text: "Counted from your quick slots, as their counts drop. Only Meat and Lemon are counted.",
    variant: "muted",
  });
  send(action("end", now));
  assert.deepEqual(last(sent, "save").data.history[0].ps, { [MEAT]: "both", 2010005: "rises" });
  // Lemon counted from its slot in the Undo window comes back counted both ways too.
  send(exp(233_843, now + 1_000));
  send(event({ type: "TestSlot", item: 2010005, count: 1 }, now + 1_500));
  send(action("undo-end", now + 2_000));
  send(action("end", now + 2_000));
  assert.deepEqual(last(sent, "save").data.history[0].ps, { [MEAT]: "both", 2010005: "both" });
});

// Only a whole number of uses counts: a source's fraction or runaway count is dropped. Fails if
// it reaches the session.
test("a use counts only as a whole number", () => {
  const { send, sent, now } = grind();
  for (const count of [1.5, Infinity, Number.NaN, -1]) send(event({ type: "TestSlot", item: MEAT, count }, now));
  assert.deepEqual(potTable(sent).rows[0], ["Meat", "×10", "800"]);
  send(event({ type: "TestSlot", item: MEAT, count: 2 }, now));
  assert.deepEqual(potTable(sent).rows[0], ["Meat", "×12", "960"]);
});

// A count source that throws never stops EXP counting. Fails if the event it threw on is lost.
test("a faulty count source never stops EXP counting", () => {
  const { send, sent, now } = play({ seconds: 60 });
  const gained = () => last(sent, "summary").summary.expGained;
  const before = gained();
  send(event({ type: "TestFault" }, now));
  send(exp(230_000 + 7 * 63, now + 1_000));
  assert.equal(gained(), before + 63);
});

// A counted potion saves the session within five seconds, as an EXP change does, so closing
// MesoVibe loses at most that. Fails if a potion waits for the 30-second save.
test("a counted potion is saved within five seconds", () => {
  const { send, sent, now } = play({ seconds: 120 });
  const saves = () => sent.filter((m) => m.type === "save").length;
  // Ticks with no gain until the next save, so the potion comes a second after one.
  let at = now;
  const before = saves();
  while (saves() === before && at < now + 60_000) send(tick((at += 1000)));
  send(hpMp(660, 500, (at += 1000)));
  for (let i = 0; i < 5; i++) send(tick((at += 1000)));
  assert.deepEqual(last(sent, "save").data.current.maps[0].u, [{ k: MEAT, c: 1 }]);
});

// A potion raised by a recovery skill lists each amount, and one with none uses its own amount.
// Fails if the amounts are joined wrongly or the line goes blank.
test("a potion's line lists each amount it's counted at", () => {
  const amounts = (at) => [
    event({ type: "TestAmounts", item: MEAT, amounts: [160, 240] }, at),
    event({ type: "TestAmounts", item: 2010005, amounts: [] }, at),
  ];
  const { sent } = grind({ also: { 300: amounts } });
  assert.deepEqual(potTable(sent).details, [["+160 and +240 HP each · 60/hr"], ["+150 MP each · 30/hr"]]);
});

// A save from a newer build can count a potion this one doesn't know: it's left out, and the
// tracker still starts. Fails if an unknown potion stops the tracker or shows up priced.
test("a saved potion this build doesn't know is left out", () => {
  const newer = pastSession(T0);
  newer.maps[0].u.push({ k: 2000004, c: 3 });
  // No saved cost, so the cost shown is worked out from the counts.
  delete newer.pt;
  const { sent } = startWith({ version: 1, history: [newer], current: null });
  assert.equal(lootTable(sent).details[0].at(-1), "Pots 800: Meat ×10");
  assert.equal(allTime(sent)["Pot cost"], "800");
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

  // Maybe uses survive too, so the cost keeps its "+" after a restart.
  const maybe = grind();
  maybe.send(event({ type: "TestMaybe", item: MEAT, count: 2 }, maybe.now));
  maybe.send(action("end", maybe.now));
  const reloaded = startWith(JSON.parse(JSON.stringify(last(maybe.sent, "save").data)));
  assert.equal(lootTable(reloaded.sent).details[0].at(-1), "Pots 1.55K+: Meat ×10, Lemon ×5");

  const bad = pastSession(T0);
  bad.maps[0].u = [{ k: "Meat", c: 1 }, null, { k: MEAT, c: -2 }];
  bad.maps[0].h = -5;
  bad.ps = 5;
  bad.pt = "a lot";
  const broken = startWith({ version: 1, history: [bad], current: null });
  const table = lootTable(broken.sent);
  assert.equal(table.rows.length, 1, "the session is kept");
  assert.equal(table.details[0].some((line) => line.startsWith("Pots")), false);

  // A malformed saved cost on a session with good counts is priced again at today's prices.
  for (const pt of ["a lot", -5, null]) {
    const repriced = lootTable(startWith({ version: 1, history: [{ ...pastSession(T0), pt }], current: null }).sent);
    assert.equal(repriced.details[0].at(-1), "Pots 800: Meat ×10", `saved cost ${JSON.stringify(pt)}`);
  }
});

// A player who goes back to MesoVibe 0.7.0 keeps their sessions: its loader reads a save with
// potion fields without failing, and this build reads what it saves back. Fails if a new field
// makes the old loader drop sessions.
test("MesoVibe 0.7.0 reads a save with potion fields", () => {
  const { send, sent, now } = grind({ lead: true });
  send(action("end", now));
  const saved = JSON.parse(JSON.stringify(last(sent, "save").data));
  const old = load070(saved);
  assert.equal(old.history.length, 1);
  assert.equal(old.history[0].maps[0].x, 3_780);
  assert.equal(old.history[0].maps[0].u, undefined, "0.7.0 drops what it doesn't know");
  const back = startWith(JSON.parse(JSON.stringify(old)));
  assert.equal(lootTable(back.sent).rows.length, 1);
  assert.ok(!lootTable(back.sent).details[0].some((line) => line.startsWith("Pots")), "a session 0.7.0 saved back counts no potions");
});
