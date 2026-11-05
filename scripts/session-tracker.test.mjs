// The Session Tracker's arithmetic (plugins/src/session-tracker/session.ts), including the edge
// cases: level-up rollover, deaths, character switches and sleeping computers never give a
// negative or garbage EXP/hr. Node runs the TypeScript directly (type stripping).
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  MAX_TICK_MS,
  MIN_RATE_MS,
  RECENT_MS,
  advance,
  compare,
  finish,
  minutesToLevel,
  newSession,
  observe,
  recentRate,
  sessionRate,
} from "../plugins/src/session-tracker/session.ts";

const NEEDED_42 = 356_916;
const at = (level, exp, needed = NEEDED_42) => ({
  level,
  exp,
  pctHundredths: needed === null ? 0 : Math.round((exp * 10_000) / needed),
  needed,
});

/** Plays `steps` of [seconds later, reading or null], ticking once a second while capturing. */
function play(steps, { capturing = true } = {}) {
  let now = 1_000_000;
  const session = newSession(now, null);
  for (const [seconds, reading] of steps) {
    for (let s = 0; s < seconds; s++) {
      now += 1000;
      advance(session, now, capturing);
    }
    if (reading !== null) observe(session, reading, now);
  }
  return { session, now };
}

test("gains on one level add up, and the rate is EXP over time played", () => {
  const { session } = play([
    [0, at(42, 230_000)],
    [30, at(42, 230_063)],
    [30, at(42, 230_126)],
  ]);
  assert.equal(session.expGained, 126);
  assert.equal(session.activeMs, 60_000);
  assert.equal(sessionRate(session), 126 * 60);
});

test("a level-up counts what was left of the old level plus the new level's EXP", () => {
  const change = compare(at(42, 356_900), at(43, 40, 390_000));
  assert.deepEqual(change, { kind: "gain", amount: 16 + 40, levelUp: true });
  const { session } = play([
    [0, at(42, 356_900)],
    [20, at(43, 40, 390_000)],
  ]);
  assert.equal(session.expGained, 56);
  assert.equal(session.levelUps, 1);
});

test("a death is EXP lost, never a negative gain", () => {
  const { session } = play([
    [0, at(42, 230_000)],
    [20, at(42, 230_500)],
    [20, at(42, 215_000)],
    [20, at(42, 215_100)],
  ]);
  assert.equal(session.expGained, 600);
  assert.equal(session.expLost, 15_500);
  assert.ok(sessionRate(session) > 0);
});

test("another character, a skipped level or an unnamed fall starts a new baseline", () => {
  assert.deepEqual(compare(at(42, 230_000), at(17, 5_000, 9_000)), { kind: "baseline" });
  assert.deepEqual(compare(at(42, 230_000), at(44, 5_000, 400_000)), { kind: "baseline" });
  assert.deepEqual(compare({ ...at(42, 230_000), level: null }, { ...at(42, 1_000), level: null }), {
    kind: "baseline",
  });
  // A level-up whose old size is unknown can't be counted exactly, so it isn't counted.
  assert.deepEqual(compare(at(42, 356_900, null), at(43, 40, 390_000)), { kind: "baseline" });
  const { session } = play([
    [0, at(42, 230_000)],
    [20, at(17, 5_000, 9_000)],
    [20, at(17, 5_090, 9_000)],
  ]);
  assert.equal(session.expGained, 90, "the switch itself gains nothing");
});

test("time counts only while capturing, and a sleeping computer adds at most one tick", () => {
  const paused = play([[0, at(42, 230_000)], [60, at(42, 230_063)]], { capturing: false });
  assert.equal(paused.session.activeMs, 0);
  assert.equal(sessionRate(paused.session), null);

  const session = newSession(0, null);
  observe(session, at(42, 230_000), 0);
  advance(session, 60 * 60_000, true);
  assert.equal(session.activeMs, MAX_TICK_MS, "an hour's gap is a sleep, not play");
});

test("there's no rate before the first gain or in the first seconds", () => {
  const still = play([[0, at(42, 230_000)], [120, null]]);
  assert.equal(sessionRate(still.session), null);
  assert.equal(minutesToLevel(still.session), null);
  const early = play([[0, at(42, 230_000)], [(MIN_RATE_MS / 1000) - 2, at(42, 230_063)]]);
  assert.equal(sessionRate(early.session), null);
});

// Time to level must agree with the strip's session EXP/hr. Fails if the recent rate drives the
// estimate again.
test("the recent rate covers the last five minutes, and time to level uses the session rate", () => {
  // Ten minutes at 63 EXP every 10 s, then five minutes at 126 every 10 s.
  const steps = [[0, at(42, 200_000)]];
  let exp = 200_000;
  for (let i = 0; i < 60; i++) steps.push([10, at(42, (exp += 63))]);
  for (let i = 0; i < 30; i++) steps.push([10, at(42, (exp += 126))]);
  const { session } = play(steps);
  assert.equal(session.activeMs, 15 * 60_000);
  const recent = recentRate(session);
  assert.ok(Math.abs(recent - 126 * 6 * 60) < 126 * 6, `recent ${recent}`);
  const rate = sessionRate(session);
  assert.ok(rate < recent, "the two rates differ here");
  const minutes = minutesToLevel(session);
  assert.ok(Math.abs(minutes - ((NEEDED_42 - exp) / rate) * 60) < 1e-6, "the session rate, as the strip shows");
  assert.ok(session.recent.every(([t]) => t > session.lastAtMs - RECENT_MS));
});

test("a finished session keeps its totals, and an empty one isn't kept", () => {
  assert.equal(finish(play([[0, at(42, 230_000)], [30, null]]).session), null);
  const { session } = play([[0, at(42, 230_000)], [30, at(42, 230_063)]]);
  const past = finish(session);
  assert.deepEqual(
    { gained: past.expGained, start: past.startLevel, end: past.endLevel, active: past.activeMs },
    { gained: 63, start: 42, end: 42, active: 30_000 },
  );
});

// A session's level line claims only levels the session earned. Fails if another character or a
// skipped reading yields "Lv 120 → 35".
test("a session that changed character keeps only the level it ended on", () => {
  const other = finish(play([[0, at(120, 1_000)], [10, at(120, 2_000)], [10, at(35, 500)], [10, at(35, 600)]]).session);
  assert.deepEqual([other.startLevel, other.endLevel], [35, 35]);
  const levelled = finish(play([[0, at(42, NEEDED_42 - 100)], [10, at(43, 50)]]).session);
  assert.deepEqual([levelled.startLevel, levelled.endLevel], [42, 43]);
});
