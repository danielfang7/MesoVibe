// What plugins hear about how the engine reads the screen (the SDK's Readers), taken from the
// engine's status, and when it changed. Apart from the panel so Node can test it.
import type { Readers } from "@mesovibe/sdk";

import type { Status } from "./ipc.js";

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
    expBar: status.exp?.confirmed ? "reading" : "unread",
  };
}

/** Whether any field differs, so a field added to Readers reaches plugins with no change here. */
export function readersChanged(before: Readers, after: Readers): boolean {
  const keys = new Set([...Object.keys(before), ...Object.keys(after)]) as Set<keyof Readers>;
  return [...keys].some((key) => before[key] !== after[key]);
}
