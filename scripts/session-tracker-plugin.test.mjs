// The Session Tracker plugin as the host runs it: bundled from source with the SDK, run in an
// isolated context with a fake Worker scope, and driven only through the SDK's messages.
// Covers what session.test can't: the plugin's wiring, its views, Reset with Undo, saving,
// and a session left running when the app closed.
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

/** Starts the plugin; returns a function to send it a message and the messages it sent. */
function run() {
  const sent = [];
  const scope = { postMessage: (message) => sent.push(structuredClone(message)), onmessage: null };
  scope.globalThis = scope;
  for (const name of ["structuredClone", "Date", "Intl", "Math", "JSON", "Number", "Array", "Object", "String"]) {
    scope[name] = globalThis[name];
  }
  vm.createContext(scope);
  vm.runInContext(bundle, scope);
  return { send: (data) => scope.onmessage({ data }), sent };
}

const last = (sent, type) => sent.filter((m) => m.type === type).at(-1);
const NEEDED = 356_916;
const exp = (value, atMs) => ({
  type: "event",
  atMs,
  event: { type: "ExpChanged", level: 42, exp: value, pct_hundredths: Math.round((value * 10_000) / NEEDED), needed: NEEDED },
});
const text = (view) => JSON.stringify(view);

/** Starts a plugin and plays a minute of grinding: 63 EXP every 10 s. */
function grind() {
  const plugin = run();
  const t0 = 1_800_000_000_000;
  plugin.send({ type: "start", saved: null, atMs: t0, capture: "capturing" });
  plugin.send(exp(230_000, t0));
  for (let s = 1; s <= 60; s++) {
    plugin.send({ type: "tick", atMs: t0 + s * 1000 });
    if (s % 10 === 0) plugin.send(exp(230_000 + (s / 10) * 63, t0 + s * 1000));
  }
  return { ...plugin, t0, now: t0 + 60_000 };
}

/** A section of the last view drawn, by its title. */
const section = (sent, title) => last(sent, "render").view.find((node) => node.title === title);
/** The "This session" section's children (none when it's left out), and its label and value pairs. */
const thisSession = (sent) => section(sent, "This session")?.children ?? [];
const items = (sent) => thisSession(sent).find((node) => node.kind === "keyValue")?.items ?? [];
const pastSessions = (sent) => section(sent, "Past sessions").children;

// Before the first gain the strip shows the level and its bar. Fails if the Session view adds a
// line of its own while the game is being read, or stops saying it's waiting when it isn't.
test("the plugin introduces itself, and before the first gain adds nothing to the strip", () => {
  const { send, sent } = run();
  assert.deepEqual(sent[0], { type: "ready", manifest: { id: "session-tracker", name: "Session Tracker" } });
  send({ type: "start", saved: null, atMs: 1_000, capture: "capturing" });
  assert.equal(section(sent, "This session"), undefined, "nothing to show yet");
  assert.doesNotMatch(text(last(sent, "render").view), /first gain/);
  assert.equal(last(sent, "summary").summary.expPerHour, null);
  send({ type: "capture", state: "no_game", atMs: 2_000 });
  send({ type: "tick", atMs: 3_000 });
  assert.match(text(thisSession(sent)), /Waiting for the game/);
});
const saves = (sent) => sent.filter((m) => m.type === "save").length;

// The Session view repeats nothing the strip shows and hides zero rows. Fails if EXP/hr, progress,
// time to level or a Reset button come back, or zeros show.
test("a minute of grinding feeds the strip, and the Session view shows only what the strip doesn't", () => {
  const { sent } = grind();
  const summary = last(sent, "summary").summary;
  assert.equal(summary.expGained, 378);
  // The 10 s before the first gain is within the minute a session keeps from before it.
  assert.equal(summary.activeMs, 60_000);
  assert.equal(summary.expPerHour, 378 * 60);
  assert.equal(summary.level, 42);
  assert.ok(summary.minutesToLevel > 0);
  assert.deepEqual(
    items(sent).map(([label]) => label),
    ["Last 5 min EXP/hr", "Time played", "EXP gained", "EXP to go"],
    "Level-ups and EXP lost stay hidden at 0",
  );
  const values = Object.fromEntries(items(sent));
  assert.equal(values["Time played"], "1m");
  assert.equal(values["EXP gained"], "378");
  assert.equal(values["EXP to go"], (NEEDED - 230_378).toLocaleString("en-US"));
  assert.match(values["Last 5 min EXP/hr"], /^[\d.]+[KM]?$/);
  assert.deepEqual(thisSession(sent).map((node) => node.kind), ["keyValue"]);
  const view = text(last(sent, "render").view);
  for (const repeat of ["Time to level", "Reset session", "EXP this level needs"]) {
    assert.doesNotMatch(view, new RegExp(repeat));
  }
  assert.match(text(pastSessions(sent)), /Sessions show up here when you end them or close MesoVibe/);
});

// Level-ups and EXP lost appear once above 0. Fails if they never show, or show at 0.
test("a death shows EXP lost in the Session view", () => {
  const { send, sent, now } = grind();
  send(exp(230_300, now + 1_000));
  assert.deepEqual(items(sent).at(-1), ["EXP lost", "78"]);
});

// Fails if End adds no row, the recap or empty state is wrong, or Undo leaves the row.
test("End saves the session with a recap, and Undo within ten seconds takes it back", () => {
  const { send, sent, now } = grind();
  send({ type: "action", action: "end", atMs: now });
  const notice = last(sent, "notice");
  assert.equal(notice.undo, "undo-end");
  assert.match(notice.text, /^Saved: 378 EXP in 1m \(.+\/hr\)\.$/);
  assert.equal(last(sent, "summary").summary.expGained, 0);
  assert.equal(last(sent, "save").data.history.length, 1);
  // The next session waits for its first gain: the strip has the EXP to go, so This session has
  // nothing to show, not a list of zeros or a waiting line.
  assert.equal(section(sent, "This session"), undefined);
  assert.equal(last(sent, "summary").summary.expToGo, NEEDED - 230_378);
  // Under its map: the day and time played, then its levels.
  const [lines] = pastSessions(sent)[0].details;
  assert.match(lines[0], /· 1m$/);
  assert.equal(lines[1], "Lv\u00a042");

  send({ type: "action", action: "undo-end", atMs: now + 5_000 });
  assert.equal(last(sent, "notice").text, "Session restored.");
  assert.equal(last(sent, "summary").summary.expGained, 378);
  assert.equal(last(sent, "save").data.history.length, 0);

  // Undo only takes back the action it belongs to, and only within ten seconds.
  send({ type: "action", action: "end", atMs: now + 6_000 });
  send({ type: "action", action: "undo-reset", atMs: now + 7_000 });
  assert.equal(last(sent, "save").data.history.length, 1);
  send({ type: "tick", atMs: now + 20_000 });
  send({ type: "action", action: "undo-end", atMs: now + 20_000 });
  assert.equal(last(sent, "save").data.history.length, 1);
  // A late Undo says so instead of silently doing nothing.
  assert.equal(last(sent, "notice").text, "Too late to undo: it's saved in Past sessions.");

  // End with nothing gained does nothing.
  const notices = sent.filter((m) => m.type === "notice").length;
  send({ type: "action", action: "end", atMs: now + 21_000 });
  assert.equal(sent.filter((m) => m.type === "notice").length, notices);
  assert.equal(last(sent, "save").data.history.length, 1);
});

test("End's recap leaves out the rate when the session is too short for one", () => {
  const { send, sent } = run();
  const t0 = 1_800_000_000_000;
  send({ type: "start", saved: null, atMs: t0, capture: "capturing" });
  send(exp(230_000, t0));
  for (let s = 1; s <= 5; s++) send({ type: "tick", atMs: t0 + s * 1000 });
  send(exp(230_063, t0 + 5_000));
  for (let s = 6; s <= 7; s++) send({ type: "tick", atMs: t0 + s * 1000 });
  send({ type: "action", action: "end", atMs: t0 + 7_000 });
  assert.match(last(sent, "notice").text, /^Saved: 63 EXP in [^(]+\.$/);
  // Past sessions keeps no rate for it either, so a big first gain ended seconds later can't
  // read as "90M/hr"; its EXP gained still shows.
  assert.deepEqual(pastSessions(sent)[0].rows[0].slice(1), ["63", null]);
});

// Fails if Reset writes the session to history again.
test("Reset throws the session away, and Undo within ten seconds brings it back", () => {
  const { send, sent, now } = grind();
  send({ type: "action", action: "reset", atMs: now });
  assert.deepEqual(last(sent, "notice"), { type: "notice", text: "Session reset.", undo: "undo-reset" });
  assert.equal(last(sent, "summary").summary.expGained, 0);
  assert.equal(last(sent, "save").data.history.length, 0, "Reset keeps nothing in history");
  assert.match(text(pastSessions(sent)), /Sessions show up here when you end them or close MesoVibe/);

  send({ type: "action", action: "undo-reset", atMs: now + 5_000 });
  assert.equal(last(sent, "notice").text, "Session restored.");
  assert.equal(last(sent, "summary").summary.expGained, 378);
  assert.equal(last(sent, "save").data.history.length, 0);

  // Too late: the Undo window has closed.
  send({ type: "action", action: "reset", atMs: now + 6_000 });
  send({ type: "tick", atMs: now + 20_000 });
  send({ type: "action", action: "undo-reset", atMs: now + 20_000 });
  assert.equal(last(sent, "summary").summary.expGained, 0);
});

// Fails if deleting stops removing rows. The setup uses End, since Reset no longer makes a row.
test("deleting history removes the saved sessions and keeps the running one", () => {
  const { send, sent, now } = grind();
  send({ type: "action", action: "end", atMs: now });
  assert.equal(last(sent, "save").data.history.length, 1);
  send(exp(231_000, now + 1_000));
  send({ type: "action", action: "delete-history", atMs: now + 2_000 });
  assert.equal(last(sent, "notice").text, "Session history deleted.");
  const saved = last(sent, "save").data;
  assert.equal(saved.history.length, 0);
  assert.notEqual(saved.current, null);
});

// One past session goes with one click, with Undo like End. Fails if the wrong row goes, the
// running session is touched, Undo doesn't bring the row back, or a late Undo is silent.
test("a past session's delete button removes it, and Undo within ten seconds brings it back", () => {
  const { send, sent, now } = grind();
  send({ type: "action", action: "end", atMs: now });
  for (let k = 1; k <= 3; k++) send(exp(230_378 + k * 63, now + 60_000 + k * 1000));
  send({ type: "action", action: "end", atMs: now + 70_000 });
  const before = last(sent, "save").data.history.map((past) => past.startedAtMs);
  assert.equal(before.length, 2);
  const table = pastSessions(sent)[0];
  assert.deepEqual(table.columns, ["Map", "EXP gained", "EXP/hr"]);
  // Each row has its own delete, named for when its session started.
  assert.equal(table.rowActions.length, 2);
  const [newer, older] = table.rowActions;
  assert.equal(newer.icon, "delete");
  assert.match(older.label, /^Delete the session from /);
  assert.notEqual(newer.action, older.action);

  send(exp(231_000, now + 75_000));
  const running = last(sent, "summary").summary.expGained;
  send({ type: "action", action: older.action, atMs: now + 76_000 });
  assert.deepEqual(last(sent, "notice"), { type: "notice", text: "Session deleted.", undo: "undo-delete" });
  assert.deepEqual(last(sent, "save").data.history.map((past) => past.startedAtMs), [before[0]], "only the older one goes");
  assert.equal(pastSessions(sent)[0].rows.length, 1);
  assert.equal(last(sent, "summary").summary.expGained, running, "the running session carries on");

  send({ type: "action", action: "undo-delete", atMs: now + 80_000 });
  assert.equal(last(sent, "notice").text, "Session restored.");
  assert.deepEqual(last(sent, "save").data.history.map((past) => past.startedAtMs), before);
  assert.equal(last(sent, "summary").summary.expGained, running);

  // Too late: the Undo window has closed.
  send({ type: "action", action: newer.action, atMs: now + 81_000 });
  send({ type: "tick", atMs: now + 95_000 });
  send({ type: "action", action: "undo-delete", atMs: now + 95_000 });
  assert.equal(last(sent, "notice").text, "Too late to undo: the session is deleted.");
  assert.deepEqual(last(sent, "save").data.history.map((past) => past.startedAtMs), [before[1]]);
  // A delete for a session that's already gone does nothing.
  const notices = sent.filter((m) => m.type === "notice").length;
  send({ type: "action", action: newer.action, atMs: now + 96_000 });
  assert.equal(sent.filter((m) => m.type === "notice").length, notices);
});

// Pause stops the session counting until Resume, while the strip's bar keeps following the game.
// Fails if time or a gain made while paused counts, or Resume counts the paused gains.
test("Pause stops time and EXP, and Resume counts only what's gained after it", () => {
  const { send, sent, now } = grind();
  const summary = () => last(sent, "summary").summary;
  assert.equal(summary().canPause, true);
  assert.equal(summary().paused, false);
  send({ type: "action", action: "pause", atMs: now });
  assert.equal(summary().paused, true);
  const { expGained, activeMs } = summary();
  for (let s = 1; s <= 30; s++) {
    send({ type: "tick", atMs: now + s * 1000 });
    if (s % 10 === 0) send(exp(230_378 + s * 10, now + s * 1000));
  }
  assert.equal(summary().expGained, expGained, "gains while paused don't count");
  assert.equal(summary().activeMs, activeMs, "nor does the time");
  assert.equal(summary().pctHundredths, Math.round((230_678 * 10_000) / NEEDED), "the bar still follows the game");

  send({ type: "action", action: "resume", atMs: now + 31_000 });
  assert.equal(summary().paused, false);
  send({ type: "tick", atMs: now + 32_000 });
  send(exp(230_741, now + 32_000));
  assert.equal(summary().expGained, expGained + 63, "only the gain after Resume");
  assert.equal(summary().activeMs, activeMs + 1_000);
});

// Fails if Pause is offered before the session starts, End or Reset leave the next session paused,
// or a level-up while paused loses the session's start level.
test("Pause waits for the first gain, a level-up while paused keeps the levels, and End starts unpaused", () => {
  const { send, sent } = run();
  const t0 = 1_800_000_000_000;
  send({ type: "start", saved: null, atMs: t0, capture: "capturing" });
  send(exp(230_000, t0));
  assert.equal(last(sent, "summary").summary.canPause, false, "nothing to pause yet");
  send({ type: "action", action: "pause", atMs: t0 + 1_000 });
  assert.equal(last(sent, "summary").summary.paused, false);

  const played = grind();
  played.send({ type: "action", action: "pause", atMs: played.now });
  played.send({
    type: "event",
    atMs: played.now + 5_000,
    event: { type: "ExpChanged", level: 43, exp: 50, pct_hundredths: 1, needed: NEEDED },
  });
  played.send({ type: "action", action: "end", atMs: played.now + 6_000 });
  assert.equal(last(played.sent, "summary").summary.paused, false, "the next session isn't paused");
  assert.equal(pastSessions(played.sent)[0].details[0][1], "Lv\u00a042 →\u00a043");
});

// Fails if the 30 s throttle returns for EXP changes, or every gain writes.
test("EXP changes save the session at most every five seconds, so a close keeps the gains", () => {
  const plugin = run();
  const t0 = 1_800_000_000_000;
  plugin.send({ type: "start", saved: null, atMs: t0, capture: "capturing" });
  plugin.send(exp(230_000, t0));
  plugin.send(exp(230_063, t0 + 6_000));
  assert.equal(last(plugin.sent, "save").data.current.expGained, 63, "saved 6 s after the last save");
  const count = saves(plugin.sent);
  plugin.send(exp(230_126, t0 + 7_000));
  assert.equal(saves(plugin.sent), count, "a gain 1 s later doesn't write again");
  // The held-back gain isn't left for the 30 s save: the first tick past the cap writes it.
  for (let s = 8; s <= 10; s++) plugin.send({ type: "tick", atMs: t0 + s * 1000 });
  assert.equal(saves(plugin.sent), count, "not before the cap");
  plugin.send({ type: "tick", atMs: t0 + 11_000 });
  assert.equal(last(plugin.sent, "save").data.current.expGained, 126);

  const { send, sent } = run();
  send({ type: "start", saved: last(plugin.sent, "save").data, atMs: t0 + 60_000, capture: "no_game" });
  assert.equal(last(sent, "save").data.history[0].expGained, 126);
});

// A session starts at its first gain. Fails if a long wait after End counts as play, or the first
// gain is measured against no time.
test("a session keeps at most a minute of the wait before its first gain", () => {
  const { send, sent, now } = grind();
  send({ type: "action", action: "end", atMs: now });
  for (let s = 1; s <= 120; s++) send({ type: "tick", atMs: now + s * 1000 });
  send(exp(230_441, now + 121_000));
  assert.equal(last(sent, "summary").summary.activeMs, 60_000, "two minutes of waiting keep one");
  for (let s = 122; s <= 126; s++) send({ type: "tick", atMs: now + s * 1000 });
  assert.equal(last(sent, "save").data.current.startedAtMs, now + 61_000, "Started is a minute before the first gain");
  assert.equal(last(sent, "summary").summary.activeMs, 65_000);
});

// A session's levels and losses start at its first gain. Fails if a character switched after End
// keeps the old start level, or a death before the gain counts as lost.
test("a session that starts on another character after End takes that character's levels", () => {
  const { send, sent, now } = grind();
  send({ type: "action", action: "end", atMs: now });
  const reading = (level, value, atMs) => ({
    type: "event",
    atMs,
    event: { type: "ExpChanged", level, exp: value, pct_hundredths: Math.round((value * 10_000) / NEEDED), needed: NEEDED },
  });
  send(reading(30, 9_000, now + 5_000));
  send(reading(30, 8_000, now + 10_000));
  send(reading(31, 50, now + 15_000));
  assert.ok(last(sent, "summary").summary.expGained > 0);
  assert.ok(!items(sent).some(([label]) => label === "EXP lost"), "a death before the first gain isn't this session's loss");
  send({ type: "action", action: "end", atMs: now + 20_000 });
  assert.deepEqual(pastSessions(sent)[0].details[0][1], "Lv\u00a030 →\u00a031");
});

// At the 300px width each first cell breaks only between its parts. Fails if a wrong or blank level
// line shows, "Lv" parts from its number, or a date splits mid-part.
test("Past sessions shows each session's levels under its date", () => {
  const session = (startLevel, endLevel) => ({
    startedAtMs: 1_800_000_000_000,
    endedAtMs: 1_800_000_600_000,
    activeMs: 600_000,
    expGained: 5_000,
    expLost: 0,
    startLevel,
    endLevel,
  });
  const { send, sent } = run();
  // The last row is a falling range saved before finish() checked level-ups: another character.
  const saved = { version: 1, history: [session(41, 43), session(null, null), session(43, 43), session(120, 35)], current: null };
  send({ type: "start", saved, atMs: 1_800_001_000_000, capture: "no_game" });
  // Under each row's map: the day and time played, then the levels.
  const details = pastSessions(sent)[0].details;
  assert.deepEqual(
    details.map((lines) => lines.slice(1)),
    [["Lv\u00a041 →\u00a043"], [], ["Lv\u00a043"], ["Lv\u00a035"]],
  );
  // The day never splits: "Nov 3" stays whole at the 300px width.
  assert.match(details[0][0], /^\w{3}\u00a0\d+ · 10m$/);
});

test("a session left running when the app closed is kept as history", () => {
  const first = grind();
  first.send({ type: "tick", atMs: first.now + 30_000 });
  const saved = last(first.sent, "save").data;
  const { send, sent } = run();
  send({ type: "start", saved, atMs: first.now + 3_600_000, capture: "no_game" });
  const history = last(sent, "save").data.history;
  assert.equal(history.length, 1);
  assert.equal(history[0].expGained, 378);
  assert.match(text(last(sent, "render").view), /Waiting for the game/);
});

test("time doesn't count while capture is paused", () => {
  const { send, sent, now } = grind();
  send({ type: "capture", state: "paused", atMs: now });
  for (let s = 1; s <= 30; s++) send({ type: "tick", atMs: now + s * 1000 });
  assert.equal(last(sent, "summary").summary.activeMs, 60_000);
});

// Fails if Reset before a gain sends a notice, saves, or leaves an Undo that wipes a later session.
test("Reset with nothing gained does nothing, and leaves no Undo behind", () => {
  const { send, sent, now } = grind();
  send({ type: "action", action: "end", atMs: now });
  const notices = sent.filter((m) => m.type === "notice").length;
  const count = saves(sent);
  send({ type: "action", action: "reset", atMs: now + 1_000 });
  assert.equal(sent.filter((m) => m.type === "notice").length, notices, "no notice");
  assert.equal(saves(sent), count, "no save");
  // The End's Undo is still the one that counts, and a stray undo-reset changes nothing.
  send({ type: "action", action: "undo-reset", atMs: now + 2_000 });
  assert.equal(last(sent, "save").data.history.length, 1);
  send({ type: "action", action: "undo-end", atMs: now + 3_000 });
  assert.equal(last(sent, "save").data.history.length, 0);
  assert.equal(last(sent, "summary").summary.expGained, 378);
});
