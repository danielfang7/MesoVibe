/**
 * What a session's potions cost, and pot cost/hr. A potion bought costs its shop price; one picked
 * up during the session costs what an NPC shop would have paid for it, since that's what drinking
 * it gave up. Potions stack, so which ones were drunk isn't known: the first uses, up to the number
 * picked up, are taken as the picked-up ones, which makes the cost a lower bound.
 */

import { MESOS_RATE_MS, sessionItems } from "./loot.js";
import type { Catalog, LootFields } from "./loot.js";
import { POTIONS } from "./potions.js";
import type { Potion } from "./potions.js";

/** The least time potions were counted before pot cost/hr is shown: the same minute as loot/hr. */
export const POT_RATE_MS = MESOS_RATE_MS;

/** One potion's line in a session's totals. */
export interface PotLine {
  potion: Potion;
  /** Uses counted. */
  used: number;
  /** Maybe uses: not in `used` and costing nothing. */
  maybe: number;
  /** Uses taken as potions picked up this session, priced at what a shop pays for them. */
  picked: number;
  /** What `used` cost. */
  cost: number;
}

export interface PotTotals {
  /** The potions used or maybe used, in the order potions.ts lists them. */
  lines: PotLine[];
  /** What every use cost. */
  cost: number;
  /** Time potions were counted. */
  readMs: number;
  /** Part of `readMs` while heals that aren't potions may have hidden some. */
  healMs: number;
  maybe: number;
}

/** How many of `potion` were picked up in the session: pickups match by name, as Lemon has two IDs. */
function pickedUp(loot: LootFields, catalog: Catalog, potion: Potion): number {
  return sessionItems(loot).reduce((sum, item) => sum + (catalog[item.k]?.n === potion.name ? item.c : 0), 0);
}

/** A session's potions, what each cost, and how long they were counted. Potions it doesn't know are left out. */
export function potTotals(loot: LootFields, catalog: Catalog): PotTotals {
  const maps = loot.maps ?? [];
  const sums = new Map<number, { c: number; y: number }>();
  for (const pot of maps.flatMap((m) => m.u ?? [])) {
    const sum = sums.get(pot.k) ?? { c: 0, y: 0 };
    sum.c += pot.c;
    sum.y += pot.y ?? 0;
    sums.set(pot.k, sum);
  }
  const totals: PotTotals = {
    lines: [],
    cost: 0,
    readMs: maps.reduce((sum, m) => sum + (m.h ?? 0), 0),
    healMs: maps.reduce((sum, m) => sum + (m.hm ?? 0), 0),
    maybe: 0,
  };
  for (const potion of POTIONS) {
    const sum = sums.get(potion.id);
    if (sum === undefined || (sum.c <= 0 && sum.y <= 0)) continue;
    const picked = Math.min(sum.c, pickedUp(loot, catalog, potion));
    const cost = picked * potion.sell + (sum.c - picked) * potion.buy;
    totals.lines.push({ potion, used: sum.c, maybe: sum.y, picked, cost });
    totals.cost += cost;
    totals.maybe += sum.y;
  }
  return totals;
}

/**
 * A past session's potions, with the cost frozen at its End when it has one, so a later price
 * change never rewrites it.
 */
export function pastPotTotals(loot: LootFields, catalog: Catalog): PotTotals {
  const totals = potTotals(loot, catalog);
  if (loot.pt !== undefined) totals.cost = loot.pt;
  return totals;
}

/** Pot cost per hour of potions counted, or null before a minute of it. */
export function potRate(totals: Pick<PotTotals, "cost" | "readMs">): number | null {
  if (totals.readMs < POT_RATE_MS) return null;
  return (totals.cost * 3_600_000) / totals.readMs;
}

/** Whether the pot cost shown is a lower bound: some rises may have been potions. */
export function costAtLeast(totals: PotTotals): boolean {
  return totals.maybe > 0;
}

/**
 * Whether net loot/hr is an upper bound: potions may be missing from the cost (maybe uses, or heals
 * from others hiding some), or some were priced as picked up when they may have been bought.
 */
export function netAtMost(totals: PotTotals, healing: boolean): boolean {
  return totals.maybe > 0 || totals.healMs > 0 || healing || totals.lines.some((line) => line.picked > 0);
}
