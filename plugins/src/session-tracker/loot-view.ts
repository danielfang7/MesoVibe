/**
 * The Session view's Loot section: the session's money in one place, headed by one Profit/hr.
 *
 * Profit/hr means the same thing whatever it's measured from: mesos picked up plus what shops
 * paid. Measured from two meso totals read in the inventory or a shop it's exact; until then it's
 * an estimate from the pickup notices, marked "≈", which miss pickups on busy maps. Items not sold
 * yet show apart as "Unsold items", never in Profit, so reading the inventory never makes the
 * headline drop.
 */

import { formatCompact, formatDigits, formatDuration } from "@mesovibe/sdk";
import type { Cell, Readers, ViewNode } from "@mesovibe/sdk";
import { MESOS_RATE_MS, itemsValue, ledgerProfit, ledgerRate, ledgerStale, sessionItems, totalMesos } from "./loot.js";
import type { Catalog, LootFields } from "./loot.js";

/** The action that counts a held rise into Profit, by when it was read. */
export const COUNT_HELD = "count-held:";
/** Held rises listed at most, newest first. */
const SHOWN_HELD = 3;

/** What the Loot section needs to know beside the session. */
export interface LootContext {
  readers: Readers;
  atMs: number;
  /** Capture is running, so pickups are being read. */
  reading: boolean;
  /** The session has gained EXP. */
  started: boolean;
  paused: boolean;
  /** Pot cost/hr while potions are counted, for the net line; null otherwise. */
  potsPerHour: number | null;
}

/** Whether a rate from the pickup notices means anything: not while they're learned or covered. */
function noticesRead(readers: Readers): boolean {
  return readers.notices !== "learning" && readers.notices !== "blocked";
}

/** Mesos per hour from the pickup notices, over the time they were read; null under a minute. */
export function noticeMesosRate(loot: LootFields): number | null {
  const maps = loot.maps ?? [];
  const read = maps.reduce((sum, m) => sum + m.l, 0);
  const mesos = maps.reduce((sum, m) => sum + m.r, 0);
  if (read < MESOS_RATE_MS || mesos <= 0) return null;
  return (mesos * 3_600_000) / read;
}

/** The Loot section's headline rate and whether it's an estimate, for the strip and mini-HUD too. */
export function lootHeadline(loot: LootFields, readers: Readers): { rate: number | null; approx: boolean } {
  const exact = loot.lg === undefined ? null : ledgerRate(loot.lg);
  if (exact !== null) return { rate: exact, approx: false };
  return { rate: noticesRead(readers) ? noticeMesosRate(loot) : null, approx: true };
}

/** "just now", "4 min ago", or "1h 5m ago". */
function ago(ms: number): string {
  if (ms < 60_000) return "just now";
  if (ms < 3_600_000) return `${Math.floor(ms / 60_000)} min ago`;
  return `${formatDuration(ms)} ago`;
}

/** A rate or amount, with U+2212 for a negative one. */
function signed(n: number): string {
  return n < 0 ? `−${formatCompact(-n)}` : formatCompact(n);
}

/**
 * The Session view's Loot section, or null with nothing to show yet. Its headline is Profit/hr,
 * with a muted line under it saying where the figure comes from and how fresh it is; then the
 * net after potions once both are known, Profit, Spent and Unsold items, and any rise held out of
 * Profit with a button to count it.
 */
export function lootSection(session: LootFields, catalog: Catalog, context: LootContext): ViewNode | null {
  const ledger = session.lg;
  const { rate, approx } = lootHeadline(session, context.readers);
  const mesos = totalMesos(session);
  const unsold = itemsValue(sessionItems(session), catalog);
  const exactPair = ledger !== undefined && ledger.n >= 2;
  const children: ViewNode[] = [];

  if (rate !== null) {
    const stale = !approx && ledger !== undefined && ledgerStale(ledger, context.atMs);
    children.push({
      kind: "stat",
      label: "Profit/hr",
      value: `${approx ? "≈" : ""}${formatCompact(rate)}`,
      detail: approx ? `About ${formatDigits(rate)} mesos per hour, from pickup notices` : `${formatDigits(rate)} mesos of profit per hour`,
      ...(context.paused || stale ? { variant: "muted" as const } : {}),
    });
    let line: string;
    if (context.paused) line = "Session paused";
    else if (!approx && ledger !== undefined) {
      line = stale
        ? `Mesos read ${ago(context.atMs - ledger.ba)}. Open your inventory to update.`
        : `Mesos read ${ago(context.atMs - ledger.ba)}`;
    } else if (ledger?.ob != null) {
      line = `Mesos at start ${formatDigits(ledger.ob)}. Open your inventory again later for exact profit.`;
    } else line = "From pickup notices, which miss some.";
    children.push({ kind: "text", text: line, variant: "muted" });
    if (!approx && context.potsPerHour !== null) {
      children.push({
        kind: "text",
        text: `${signed(rate - context.potsPerHour)}/hr after ${formatCompact(context.potsPerHour)}/hr on potions`,
        variant: "muted",
      });
    }
  } else if (
    (mesos > 0 || unsold > 0) &&
    noticesRead(context.readers) &&
    context.reading &&
    context.started &&
    (session.maps ?? []).reduce((sum, m) => sum + m.l, 0) < MESOS_RATE_MS
  ) {
    children.push({ kind: "empty", text: "Profit/hr shows after a minute of pickups." });
  }

  const rows: [string, Cell][] = [];
  if (exactPair) rows.push(["Profit", formatDigits(ledgerProfit(ledger))]);
  else if (mesos > 0) rows.push(["Profit", `≈${formatDigits(mesos)}`]);
  if (exactPair && ledger.sp > 0) rows.push(["Spent", formatDigits(ledger.sp)]);
  // Once a shop has paid for items, what they sold for is in Profit, and an estimate of the same
  // items beside it would read as counting them twice.
  if (unsold > 0 && !(ledger !== undefined && ledger.so > 0)) rows.push(["Unsold items", `≈${formatDigits(unsold)}`]);
  if (rows.length > 0) children.push({ kind: "keyValue", items: rows });

  for (const [amount, readAt] of [...(ledger?.ho ?? [])].reverse().slice(0, SHOWN_HELD)) {
    const rise = formatCompact(amount);
    children.push(
      { kind: "partial", text: `+${rise} not counted: too big a jump for farming (storage, a trade or a misread)` },
      { kind: "button", label: "Count it", action: `${COUNT_HELD}${readAt}`, style: "secondary", spoken: `Count the ${rise} rise in Profit` },
    );
  }
  return children.length === 0 ? null : { kind: "section", title: "Loot", children };
}
