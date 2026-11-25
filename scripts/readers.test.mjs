// What plugins hear about how the engine reads the screen (ui/src/readers.ts). Node runs the
// TypeScript directly (type stripping).
import { test } from "node:test";
import assert from "node:assert/strict";

import { readersChanged, readersOf } from "../ui/src/readers.ts";

/** An engine status with the readers' fields that matter here. */
const status = (over = {}) => ({
  hpMpState: "reading",
  hpMpMissing: null,
  mesos: null,
  slots: null,
  ...over,
});

// A change in how the HP and MP numbers are read reaches plugins even when nothing else changed.
// Fails if the check the plugin host sends Readers on by misses the HP and MP fields, so plugins
// never hear that the numbers went from reading to covered.
test("a change to the HP and MP state alone reaches plugins", () => {
  const reading = readersOf(status());
  const covered = readersOf(status({ hpMpState: "covered" }));
  assert.equal(covered.hpMp, "covered");
  assert.equal(readersChanged(reading, covered), true);
  const waiting = readersOf(status({ hpMpState: "missing_digits", hpMpMissing: "7" }));
  assert.deepEqual([waiting.hpMp, waiting.hpMpMissing], ["missing_digits", "7"]);
  assert.equal(readersChanged(readersOf(status({ hpMpState: "missing_digits", hpMpMissing: "07" })), waiting), true);
});

// Plugins hear how the meso total is read only from a host that reads it, so the Session Tracker
// asks for the inventory only then. Fails if a state is lost on the way, or reading off still
// reports one.
test("the meso total's state reaches plugins while the engine reads it", () => {
  for (const state of ["learning", "reading", "unsupported"]) {
    assert.equal(readersOf(status({ mesos: state })).mesos, state);
  }
  assert.equal("mesos" in readersOf(status({ mesos: null })), false, "reading off");
  assert.equal(readersChanged(readersOf(status({ mesos: "learning" })), readersOf(status({ mesos: "reading" }))), true);
  assert.equal(readersChanged(readersOf(status({ mesos: "reading" })), readersOf(status({ mesos: null }))), true);
});

// Plugins hear how the quick slots are read only from a host that reads them, so the Session
// Tracker counts potions from them only then. Fails if a state is lost or reading off reports one.
test("the quick slots' state reaches plugins while the engine reads them", () => {
  for (const state of ["reading", "not_found", "unsupported"]) {
    assert.equal(readersOf(status({ slots: state })).slots, state);
  }
  assert.equal("slots" in readersOf(status({ slots: null })), false, "reading off");
  assert.equal(readersChanged(readersOf(status({ slots: "reading" })), readersOf(status({ slots: "not_found" }))), true);
});

// Statuses arrive four times a second; plugins hear only changes. Fails if an unchanged status is
// sent on, or a change is missed, including a field one side has and the other doesn't.
// Whether the EXP bar was read reaches plugins: the Session Tracker counts the minute with no EXP
// only while it is. Fails if an unread bar looks read, or the change isn't sent on.
test("whether the EXP bar is read reaches plugins", () => {
  assert.equal(readersOf(status({ exp: { confirmed: true } })).expBar, "reading");
  assert.equal(readersOf(status({ exp: { confirmed: false } })).expBar, "unread", "not confirmed yet");
  assert.equal(readersOf(status({ exp: null })).expBar, "unread");
  assert.equal(readersChanged(readersOf(status({ exp: { confirmed: true } })), readersOf(status({ exp: null }))), true);
});

test("only changes are sent on", () => {
  assert.equal(readersChanged(readersOf(status()), readersOf(status())), false);
  assert.equal(readersChanged(readersOf(status()), readersOf(status({ hpMpState: "covered" }))), true);
  assert.equal(readersChanged(readersOf(status()), { ...readersOf(status()), mesos: "reading" }), true);
});
