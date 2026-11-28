// What the mini-HUD card says in each state (ui/src/hudview.ts): which message wins the status
// band, what takes the level row's place, and what each of the four boxes shows before its number
// exists. Node runs the TypeScript directly (type stripping).
import { test } from "node:test";
import assert from "node:assert/strict";

import { boxMinutes, hudView } from "../ui/src/hudview.ts";

const capturing = (over = {}) => ({
  reading: true,
  state: "capturing",
  game: { width: 1366, height: 768, scale: 2 },
  fullScreen: false,
  exp: { level: 42, exp: 230_640, expShown: true, pctHundredths: 6462, needed: 356_916, confirmed: true },
  unreadable: null,
  error: null,
  chat: null,
  calibration: "confirmed",
  stale: false,
  atMs: 0,
  ...over,
});
const grind = (over = {}) => ({
  expPerHour: 48_210,
  recentExpPerHour: 43_412,
  expGained: 96_420,
  level: 42,
  pctHundredths: 6462,
  minutesToLevel: 174.5,
  lootPerHour: 21_480,
  ...over,
});
const view = (status, summary, over = {}) =>
  hudView({ status, summary, longDark: false, fullScreenUnread: false, stale: false, time: () => "4:30 PM", ...over });

// Fails if a message other than the most urgent one takes the band, or the name shows while
// something is wrong.
test("the band says what's wrong first, then a pause, then learning or waiting, then the name", () => {
  assert.deepEqual(view(capturing(), grind()).band, { text: null, tone: "live" });
  assert.deepEqual(view(capturing(), grind({ paused: true, autoPaused: true })).band, { text: "Paused: no EXP", tone: "idle" });
  assert.deepEqual(view(capturing(), grind({ paused: true })).band, { text: "Paused", tone: "idle" });
  // A covered EXP bar outranks the pause it may have caused.
  assert.deepEqual(view(capturing({ unreadable: "no_text" }), grind({ paused: true })).band, { text: "Can't see the EXP bar", tone: "warn" });
  assert.deepEqual(view(capturing({ state: "blocked" }), grind()).band, { text: "Can't capture the game", tone: "danger" });
  // Still learning is live, so a pause outranks it.
  assert.deepEqual(view(capturing({ unreadable: "learning" }), grind({ paused: true })).band.text, "Paused");
  assert.deepEqual(view(capturing({ unreadable: "learning" }), null).band, { text: "Learning your EXP bar…", tone: "live" });
  assert.deepEqual(view(capturing(), null).band, { text: "Earn more EXP to start", tone: "live" });
  assert.deepEqual(view(null, null).band.text, "Earn more EXP to start");
});

// Fails if the Full Screen fix, the sell-by time or the inventory ask goes missing, or one hides a
// more urgent one.
test("a tip takes the level row: the problem's fix, then selling, then the inventory", () => {
  assert.equal(view(capturing(), grind()).tip, null);
  assert.equal(view(capturing(), grind({ askForMesos: true })).tip, "Open inventory for mesos/hr");
  assert.equal(view(capturing(), grind({ askForMesos: true, closeOutBy: 1 })).tip, "Sell at a shop by 4:30 PM");
  const full = view(capturing({ fullScreen: true }), grind({ closeOutBy: 1 }), { fullScreenUnread: true });
  assert.equal(full.band.text, "Can't read Full Screen");
  assert.equal(full.tip, "Try windowed or 1366×768");
});

// Fails if a box shows a number it doesn't have, or a dash where the 5-minute rate is truly 0.
test("each box shows its figure, a 0 rate after five minutes without EXP, or nothing yet", () => {
  const live = view(capturing(), grind({ potCostPerHour: 3_120 }));
  assert.deepEqual(
    [live.recent, live.session, live.gained, live.minutesToLevel, live.loot, live.pots, live.potsHint],
    [43_412, 48_210, 96_420, 174.5, 21_480, 3_120, false],
  );
  // No potion counted where they could be: the pot box asks instead of showing a figure.
  const noPots = view(capturing(), grind({ potCostPerHour: null, potsUncounted: true }));
  assert.deepEqual([noPots.pots, noPots.potsHint], [null, true]);
  const town = view(capturing(), grind({ recentExpPerHour: null, minutesToLevel: null }));
  assert.equal(town.recent, 0);
  assert.equal(town.minutesToLevel, null);
  const fresh = view(capturing(), { expPerHour: null, recentExpPerHour: null, expGained: 0, level: 42, pctHundredths: 6462, minutesToLevel: null, lootPerHour: null });
  assert.deepEqual([fresh.recent, fresh.session, fresh.loot, fresh.gained, fresh.minutesToLevel, fresh.pots], [null, null, null, null, null, null]);
  // Before the panel shares a summary, the level comes from the engine's confirmed reading.
  const early = view(capturing(), null);
  assert.deepEqual([early.level, early.pctHundredths], [42, 6462]);
  assert.deepEqual(view(capturing({ exp: null }), null).level, null);
});

// Fails if a paused or stale card keeps its figures bright.
test("the figures dim when stale or paused", () => {
  assert.equal(view(capturing(), grind()).dim, false);
  assert.equal(view(capturing(), grind(), { stale: true }).dim, true);
  assert.equal(view(capturing(), grind({ paused: true })).dim, true);
});

// Fails if a time to level is wider than its box allows ("99h 59m") or rounds into "1h 60m".
test("time to level fits its box", () => {
  assert.equal(boxMinutes(0.4), "<1 min");
  assert.equal(boxMinutes(23.4), "23 min");
  assert.equal(boxMinutes(59.6), "1h");
  assert.equal(boxMinutes(119.7), "2h");
  assert.equal(boxMinutes(913), "15h 13m");
  assert.equal(boxMinutes(5999), "99h 59m");
  assert.equal(boxMinutes(5999.6), "100h");
  assert.equal(boxMinutes(7425), "123h");
});
