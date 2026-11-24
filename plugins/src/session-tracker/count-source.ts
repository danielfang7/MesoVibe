/**
 * How the Session Tracker learns that a potion was drunk. A count source watches the engine's
 * events and reports each use it finds; the tracker adds the uses to the session and its map,
 * prices them and shows them, the same way whichever source found them.
 *
 * No source is wired in yet, so nothing is counted and the Potions section and the Loot tab's
 * potion lines stay hidden. The tests wire in a stand-in source to check the accounting and the
 * views.
 */

import type { Event } from "@mesovibe/sdk";

/** Uses of one potion that a source found in one event. */
export interface PotUse {
  /** The potion's item ID, as in potions.ts. */
  item: number;
  count: number;
  /**
   * A rise that may have been this potion: shown apart and costing nothing, so the cost shown is a
   * lower bound.
   */
  maybe: boolean;
  /** Counted from the HP or MP numbers going up, or from a quick slot's count going down. */
  source: "rises" | "slots";
}

export interface CountSource {
  /**
   * Takes every engine event, in order, paused or not, and returns the potion uses it found in it.
   * It reports uses only from times the HP and MP numbers were read, since pot cost/hr divides by
   * that time.
   */
  onEvent(event: Event, atMs: number): PotUse[];
  /**
   * Whether heals that aren't potions (a Heal, Drain, MP Eater) may be hiding uses of this
   * resource's potion now, so its count is a lower bound.
   */
  healing(resource: "hp" | "mp", atMs: number): boolean;
  /** The amounts the potion is counted at, the base first: [160], or [160, 240] with a boost. */
  amounts(item: number): number[];
}

/** The source the tracker counts potions with, or null while none is wired in. */
export function wiredSource(): CountSource | null {
  return null;
}
