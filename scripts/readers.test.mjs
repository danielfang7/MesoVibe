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

// Statuses arrive four times a second; plugins hear only changes. Fails if an unchanged status is
// sent on, or a change is missed, including a field one side has and the other doesn't.
test("only changes are sent on", () => {
  assert.equal(readersChanged(readersOf(status()), readersOf(status())), false);
  assert.equal(readersChanged(readersOf(status()), readersOf(status({ hpMpState: "covered" }))), true);
  assert.equal(readersChanged(readersOf(status()), { ...readersOf(status()), mesos: "reading" }), true);
});
