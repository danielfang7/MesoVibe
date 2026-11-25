// What plugins hear about how the engine reads the screen (the SDK's Readers), taken from the
// engine's status, and when it changed. Apart from the panel so Node can test it.
import type { Readers } from "@mesovibe/sdk";

import type { Status } from "./ipc.js";

/**
 * Why the EXP bar couldn't be read when the HUD is there but its text isn't: something over it,
 * such as a cursor. A HUD that isn't found (character select, the Cash Shop) or a map fading isn't
 * covered: there's no play to miss.
 */
const COVERED = new Set(["no_text", "unknown_glyph", "garbled"]);

/**
 * The readers' states in a status, as plugins receive them. `mesos` and `slots` are there only
 * while the engine reads them: with reading off, nothing asks the player to open the inventory or
 * counts potions from the slots.
 */
export function readersOf(status: Status): Readers {
  return {
    hpMp: status.hpMpState,
    hpMpMissing: status.hpMpMissing,
    ...(status.mesos === null ? {} : { mesos: status.mesos }),
    ...(status.slots === null ? {} : { slots: status.slots }),
    expBar: COVERED.has(status.unreadable ?? "") ? "covered" : "clear",
  };
}

/** Whether any field differs, so a field added to Readers reaches plugins with no change here. */
export function readersChanged(before: Readers, after: Readers): boolean {
  const keys = new Set([...Object.keys(before), ...Object.keys(after)]) as Set<keyof Readers>;
  return [...keys].some((key) => before[key] !== after[key]);
}
