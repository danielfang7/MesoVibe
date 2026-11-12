// When the panel and the mini-HUD say the game has been black too long (ui/src/dark.ts): a map
// change's second or two of black says nothing, a screen that stays black does. Node runs the
// TypeScript directly (type stripping).
import { test } from "node:test";
import assert from "node:assert/strict";

import { DARK_WARN_MS, darkWatch, fullScreenWatch } from "../ui/src/dark.ts";

/** A status `ms` after the start, capturing unless `over` says otherwise. */
const at = (ms, over = {}) => ({
  reading: true,
  state: "capturing",
  game: { width: 1366, height: 768, scale: 2 },
  fullScreen: false,
  exp: null,
  unreadable: null,
  error: null,
  chat: null,
  calibration: "confirmed",
  stale: false,
  atMs: ms,
  ...over,
});
const dark = (ms) => at(ms, { unreadable: "screen_dark" });

// A map change fades to black for a moment; only a long dark spell warns. Fails if the panel
// warns as soon as the screen goes dark, or never does.
test("the warning comes once the screen has been dark for the whole wait", () => {
  const watch = darkWatch();
  assert.equal(watch(dark(0)), false);
  assert.equal(watch(dark(2_000)), false, "a map change's fade");
  assert.equal(watch(dark(DARK_WARN_MS - 1)), false);
  assert.equal(watch(dark(DARK_WARN_MS)), true);
  assert.equal(watch(dark(DARK_WARN_MS + 60_000)), true, "still dark, still warned");
});

// Only one unbroken dark spell counts. Fails if map changes add up into a warning, a warning
// outlives the next reading, or a tick with no new frame (which reads nothing) cuts a long dark
// spell short.
test("anything read ends a dark spell, and a tick with no new frame doesn't", () => {
  const ends = [
    ["an EXP reading", at(9_000, { exp: { level: 41, exp: 1, pctHundredths: 0, needed: 2, confirmed: true } })],
    ["the bar covered", at(9_000, { unreadable: "hud_not_found" })],
    ["the game minimized", at(9_000, { state: "paused" })],
    ["paused as frames stopped coming", at(9_000, { state: "paused", stale: true })],
    ["the game closed", at(9_000, { state: "no_game", game: null })],
    ["reading turned off", at(9_000, { reading: false, state: "off" })],
  ];
  for (const [what, status] of ends) {
    const watch = darkWatch();
    watch(dark(0));
    watch(dark(8_000));
    watch(status);
    assert.equal(watch(dark(10_000)), false, `${what} ends the first spell`);
    assert.equal(watch(dark(10_000 + DARK_WARN_MS)), true, `${what}: a new spell warns in its own time`);
  }

  const warned = darkWatch();
  warned(dark(0));
  assert.equal(warned(dark(DARK_WARN_MS)), true);
  const reading = at(DARK_WARN_MS + 250, { exp: { level: 41, exp: 1, pctHundredths: 0, needed: 2, confirmed: true } });
  assert.equal(warned(reading), false, "a reading clears the warning at once");
  assert.equal(warned(dark(DARK_WARN_MS + 500)), false, "the next map change starts a new spell");

  const watch = darkWatch();
  watch(dark(0));
  watch(at(4_000, { stale: true }));
  watch(at(8_000, { stale: true }));
  assert.equal(watch(dark(DARK_WARN_MS)), true, "dark the whole time, with no new frame for a while");
});

/** Full Screen with no HUD seen, `ms` after the start. */
const unseen = (ms, unreadable = "hud_not_found") => at(ms, { fullScreen: true, unreadable });

// At a Full Screen size MesoVibe can't read, every frame has no HUD or looks dark. Fails if that
// doesn't warn after the wait, or if a windowed game, or one still learning its digits, does.
test("Full Screen with no HUD seen for the whole wait says so", () => {
  const watch = fullScreenWatch();
  assert.equal(watch(unseen(0)), false);
  assert.equal(watch(unseen(3_000, "screen_dark")), false, "dark and no HUD are one spell");
  assert.equal(watch(unseen(DARK_WARN_MS - 1, "garbled")), false);
  assert.equal(watch(unseen(DARK_WARN_MS)), true);

  const windowed = fullScreenWatch();
  windowed(at(0, { unreadable: "hud_not_found" }));
  assert.equal(windowed(at(60_000, { unreadable: "hud_not_found" })), false, "windowed: something covers the bar");

  const learning = fullScreenWatch();
  learning(unseen(0, "learning"));
  assert.equal(learning(unseen(60_000, "learning")), false, "learning reads the HUD");
});

// Fails if the warning outlives the game leaving Full Screen or the next reading, or if a tick
// with no new frame cuts the spell short.
test("leaving Full Screen or reading ends the spell, and a tick with no new frame doesn't", () => {
  const ends = [
    ["windowed again", at(9_000)],
    ["an EXP reading", at(9_000, { fullScreen: true, exp: { level: 41, exp: 1, pctHundredths: 0, needed: 2, confirmed: true } })],
    ["learning", unseen(9_000, "learning")],
    ["the game closed", at(9_000, { state: "no_game", game: null })],
  ];
  for (const [what, status] of ends) {
    const watch = fullScreenWatch();
    watch(unseen(0));
    watch(status);
    assert.equal(watch(unseen(10_000)), false, `${what} ends the spell`);
    assert.equal(watch(unseen(10_000 + DARK_WARN_MS)), true, `${what}: a new spell warns in its own time`);
  }

  const watch = fullScreenWatch();
  watch(unseen(0));
  watch(at(5_000, { fullScreen: true, stale: true }));
  assert.equal(watch(unseen(DARK_WARN_MS)), true, "no new frame for a while in between");
});
