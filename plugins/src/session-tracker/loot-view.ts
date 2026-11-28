/**
 * The Session view's Loot section: the session's mesos, measured from the meso totals read in the
 * inventory and shop windows (the meso ledger in loot.ts).
 *
 * Its headline is Mesos/hr: mesos picked up over the play time between two totals, never an
 * estimate. What the loot picked up is worth isn't known until it's sold, so Loot sold shows "—"
 * until a shop pays, and Profit (mesos picked up plus what shops paid) waits for that too. Sales
 * up to 10 minutes after End still count for the session (its close-out, in index.ts).
 */

import { formatCompact, formatDigits, formatDuration } from "@mesovibe/sdk";
import type { Cell, Readers, ViewNode } from "@mesovibe/sdk";
import { STALE_MS, ledgerProfit, mesosRate, mesosRateReady, profitRate } from "./loot.js";
import type { Ledger, LootFields } from "./loot.js";

/** The action that counts a held rise into Profit, by when it was read. */
export const COUNT_HELD = "count-held:";
/** Held rises listed at most, newest first. */
const SHOWN_HELD = 3;

/** What the Loot section needs to know beside the session. */
export interface LootContext {
  readers: Readers;
  atMs: number;
  /** The session has gained EXP. */
  started: boolean;
  paused: boolean;
  /** The session's play time, the clock its meso totals are timed on. */
  playMs: number;
  /** Pot cost/hr while potions are counted, for the net line; null otherwise. */
  potsPerHour: number | null;
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

function muted(text: string): ViewNode {
  return { kind: "text", text, variant: "muted" };
}

/**
 * Whether the headline's figure is old: its last total was read over 15 minutes ago, or a new
 * segment (another character) is waiting for its first total.
 */
function stale(ledger: Ledger, atMs: number): boolean {
  return ledger.b === null || atMs - ledger.ba > STALE_MS;
}

/**
 * The line under the headline, by state: paused, the meso digits being learned or unreadable at
 * this size, how long ago mesos were read, or what to do to start. It asks for the inventory only
 * when the host reads the meso total (`readers.mesos`), so no player is asked for a reading
 * nothing makes.
 */
function stateLine(ledger: Ledger | undefined, rate: number | null, context: LootContext): ViewNode | null {
  const { readers, atMs } = context;
  if (context.paused) return muted("Session paused");
  if (readers.mesos === "learning") return muted("Learning to read your mesos. Keep the inventory open a few seconds.");
  if (readers.mesos === "unsupported") return { kind: "partial", text: "MesoVibe can't read mesos at this game size." };
  const asks = readers.mesos === "reading";
  if (ledger !== undefined && rate !== null) {
    const read = `Mesos read ${ago(atMs - ledger.ba)}`;
    return muted(stale(ledger, atMs) && asks ? `${read}. Open your inventory to update.` : read);
  }
  if (ledger !== undefined && ledger.b !== null) {
    // After a minute of play the next total makes mesos/hr, and the mini-HUD asks for it too.
    const when = mesosRateReady(ledger, context.playMs) ? "now" : "in a minute";
    return muted(`Mesos read: ${formatDigits(ledger.b)}.${asks ? ` Open your inventory again ${when} to see mesos/hr.` : ""}`);
  }
  return asks ? muted("Open your inventory to start counting mesos.") : null;
}

/**
 * The Session view's Loot section, or null before the session's first EXP gain, and from a host
 * that doesn't read the meso total while none was read. Its headline is Mesos/hr ("?" before two
 * totals), with a line under it saying how fresh it is or how to start; then Mesos picked up, Loot
 * sold, Profit once a shop has paid or a held rise was counted, and Spent (shown apart, never
 * taken from Profit); a line about selling while nothing is sold; the net after potions once
 * they're counted; and any rise held out of Profit with a button to count it.
 */
export function lootSection(session: LootFields, context: LootContext): ViewNode | null {
  const ledger = session.lg;
  if (!context.started || (ledger === undefined && context.readers.mesos === undefined)) return null;
  const rate = ledger === undefined ? null : mesosRate(ledger);
  const children: ViewNode[] = [
    {
      kind: "stat",
      label: "Mesos/hr",
      value: rate === null ? null : formatCompact(rate),
      ...(rate === null ? {} : { detail: `${formatDigits(rate)} mesos picked up per hour` }),
      ...(ledger !== undefined && rate !== null && (context.paused || stale(ledger, context.atMs)) ? { variant: "muted" as const } : {}),
    },
  ];
  const line = stateLine(ledger, rate, context);
  if (line !== null) children.push(line);

  const sold = ledger !== undefined && ledger.so > 0;
  // Profit once a shop has paid, or a held rise was counted into it (so Count it shows).
  const profit = sold || (ledger !== undefined && ledger.ct > 0);
  const rows: [string, Cell][] = [];
  if (ledger !== undefined && ledger.n >= 2) rows.push(["Mesos picked up", formatDigits(ledger.pu)]);
  rows.push(["Loot sold", sold ? formatDigits(ledger.so) : "—"]);
  if (profit) rows.push(["Profit", formatDigits(ledgerProfit(ledger))]);
  if (ledger !== undefined && ledger.sp > 0) rows.push(["Spent", formatDigits(ledger.sp)]);
  children.push({ kind: "keyValue", items: rows });

  if (!sold && context.readers.mesos !== undefined && context.readers.mesos !== "unsupported") {
    children.push(muted("Sell your loot at a shop to see what it's worth. Sales up to 10 minutes after End count for this session."));
  }
  // Net after potions: from Profit/hr once Profit shows, from Mesos/hr until then.
  const base = profit ? profitRate(ledger) : rate;
  if (context.potsPerHour !== null && base !== null) {
    children.push(muted(`${signed(base - context.potsPerHour)}/hr after ${formatCompact(context.potsPerHour)}/hr on potions`));
  }

  if (ledger !== undefined) children.push(...heldLines(ledger));
  return { kind: "section", title: "Loot", children };
}

/**
 * Each rise held out of Profit, newest first and at most three: a calm line saying why, and a
 * button to count it, stacked so it fits the narrowest panel. Its spoken name says which rise.
 */
export function heldLines(ledger: Ledger): ViewNode[] {
  return [...ledger.ho]
    .reverse()
    .slice(0, SHOWN_HELD)
    .flatMap(([amount, readAt]): ViewNode[] => {
      const rise = formatCompact(amount);
      return [
        { kind: "partial", text: `+${rise} not counted: too big a jump for farming (a trade or a misread)` },
        { kind: "button", label: "Count it", action: `${COUNT_HELD}${readAt}`, style: "secondary", spoken: `Count the ${rise} rise in Profit` },
      ];
    });
}
