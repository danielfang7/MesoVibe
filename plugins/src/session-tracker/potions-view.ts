/**
 * The Session Tracker's potion views: the Session view's Potions section, and the Loot tab's
 * Net/hr column, pots line and All time rows. They show only once a count source is wired in.
 *
 * Net loot/hr is loot/hr (over the time the pickup notices were read) minus pot cost/hr (over the
 * time potions were counted). The Potions section's headline is always one `stat` in the same
 * place, so the panel doesn't jump as it changes from "shows after a minute" to pot cost/hr to net.
 */

import { formatCompact, formatDigits, formatDuration } from "@mesovibe/sdk";
import type { Cell, Readers, ViewNode } from "@mesovibe/sdk";
import type { CountSource } from "./count-source.js";
import { lootRate } from "./loot.js";
import type { Catalog, LootFields } from "./loot.js";
import { costAtLeast, netAtMost, pastPotTotals, potRate, potTotals } from "./pot-cost.js";
import type { PotTotals } from "./pot-cost.js";

/** A rate or amount, with U+2212 for a negative one: net can fall below zero. */
function signed(n: number): string {
  return n < 0 ? `−${formatCompact(-n)}` : formatCompact(n);
}

/** "a 7", "an 8", or for several "a digit it hasn't seen yet: 0, 4 or 5", as What MesoVibe sees says it. */
function digitsWanted(digits: string): string {
  const list = [...digits];
  if (list.length === 0) return "the digits it needs";
  if (list.length === 1) return `${list[0] === "8" ? "an" : "a"} ${list[0]}`;
  return `a digit it hasn't seen yet: ${list.slice(0, -1).join(", ")} or ${list.at(-1)}`;
}

/** The line about reading the HP and MP numbers, when they aren't being read. */
function readingLine(readers: Readers): ViewNode | null {
  switch (readers.hpMp) {
    case "learning":
      return { kind: "empty", text: "Learning your HP and MP digits from your EXP gains." };
    case "missing_digits":
      return {
        kind: "empty",
        text: `To read your HP and MP, MesoVibe waits for an EXP gain that shows ${digitsWanted(readers.hpMpMissing ?? "")}.`,
      };
    case "covered":
      return { kind: "partial", text: "Potions aren't counted while your HP and MP numbers are covered." };
    case "not_found":
      return {
        kind: "partial",
        text: "Can't see your HP and MP numbers. Keep the bar at the bottom of the game in view. See What MesoVibe sees.",
      };
    default:
      return null;
  }
}

/** The headline: net loot/hr, pot cost/hr while loot/hr isn't known, or "?" before a minute. */
function headline(totals: PotTotals, loot: number | null, healing: boolean): ViewNode {
  const pots = potRate(totals);
  if (pots === null) return { kind: "stat", label: "Net loot/hr", value: null, detail: "Net loot/hr shows after a minute." };
  const plus = costAtLeast(totals) ? "+" : "";
  if (loot === null) return { kind: "stat", label: "Pot cost/hr", value: `${formatCompact(pots)}${plus}`, detail: "Net shows with loot/hr" };
  const net = loot - pots;
  const atMost = netAtMost(totals, healing);
  const none = totals.lines.length === 0;
  const said = none
    ? atMost
      ? "no potions counted yet"
      : "No potions used yet"
    : net < 0
      ? "more spent on potions than picked up"
      : `${formatCompact(loot)} loot/hr − ${formatCompact(pots)}${plus} on potions`;
  return { kind: "stat", label: "Net loot/hr", value: signed(net), detail: atMost ? `At most: ${said}` : said };
}

/** "+160 HP each", or "+160 and +240 HP each" when a recovery skill raises it. */
function amountsText(amounts: number[], resource: "hp" | "mp"): string {
  const unit = resource === "hp" ? "HP" : "MP";
  const listed = amounts.map((n) => `+${formatDigits(n)}`);
  const text = listed.length <= 1 ? (listed[0] ?? "") : `${listed.slice(0, -1).join(", ")} and ${listed.at(-1)}`;
  return `${text} ${unit} each`;
}

/** The table of potions: Item | Used | Cost, with each one's amount, rate and maybe uses under it. */
function potTable(totals: PotTotals, source: CountSource): ViewNode {
  const hours = totals.readMs / 3_600_000;
  const rows = totals.lines.map((line) => {
    const amounts = source.amounts(line.potion.id);
    const parts = [amountsText(amounts.length > 0 ? amounts : [line.potion.amount], line.potion.resource)];
    if (potRate(totals) !== null && line.used > 0) parts.push(`${formatDigits(line.used / hours)}/hr`);
    if (line.maybe > 0) parts.push(`+${formatDigits(line.maybe)} maybe`);
    const details = [parts.join(" · ")];
    if (line.picked > 0) details.push(`${formatDigits(line.picked)} picked up, at NPC sell price`);
    return { cells: [line.potion.name, `×${formatDigits(line.used)}`, formatDigits(line.cost)] as Cell[], details };
  });
  return {
    kind: "table",
    columns: ["Item", "Used", "Cost"],
    numeric: [false, true, true],
    rows: rows.map((row) => row.cells),
    details: rows.map((row) => row.details),
  };
}

/**
 * The Session view's Potions section, or null before the session's first EXP gain, when no potion
 * counts yet. `loot` is the loot/hr Items shows, or null when it isn't known.
 */
export function potionsSection(
  session: LootFields & { expGained: number },
  catalog: Catalog,
  source: CountSource,
  readers: Readers,
  loot: number | null,
  atMs: number,
): ViewNode | null {
  if (session.expGained <= 0) return null;
  const totals = potTotals(session, catalog);
  const healing = source.healing("hp", atMs) || source.healing("mp", atMs);
  const children: ViewNode[] = [headline(totals, loot, healing)];
  if (totals.lines.length > 0) children.push(potTable(totals, source));
  // At most two lines about why counts may be off, the reading first.
  const reading = readingLine(readers);
  if (reading !== null) children.push(reading);
  if (healing) {
    children.push({ kind: "partial", text: "Some healing isn't from potions (a Heal, Drain or MP Eater), so potions may be undercounted." });
  }
  const fromSlots = Object.values(session.ps ?? {}).some((s) => s !== "rises");
  children.push({
    kind: "text",
    text: fromSlots ? "Only Meat and Lemon are counted." : "Counted from your HP and MP numbers going up. Only Meat and Lemon are counted.",
    variant: "muted",
  });
  return { kind: "section", title: "Potions", children };
}

/** A past session's Net/hr cell: "—" when potions weren't counted in it, "?" when its loot/hr isn't known. */
export function netCell(past: LootFields, catalog: Catalog): Cell {
  const totals = pastPotTotals(past, catalog);
  if (totals.readMs <= 0) return "—";
  const pots = potRate(totals);
  const loot = lootRate(past);
  return pots === null || loot === null ? null : signed(loot - pots);
}

/** A past session's pots line, "Pots 3.1K: Meat ×30, Lemon ×12", or null when it used none. */
export function potsLine(past: LootFields, catalog: Catalog): string | null {
  const totals = pastPotTotals(past, catalog);
  if (totals.lines.length === 0) return null;
  const used = totals.lines.filter((line) => line.used > 0).map((line) => `${line.potion.name} ×${formatDigits(line.used)}`);
  const cost = `${formatCompact(totals.cost)}${costAtLeast(totals) ? "+" : ""}`;
  return used.length === 0 ? `Pots ${cost}` : `Pots ${cost}: ${used.join(", ")}`;
}

/**
 * All time's potion rows, over the sessions that counted potions only (sessions from before potions
 * were counted have none, and would make net look better than it was): how long that was, what
 * their potions cost, and their net loot/hr.
 */
export function allTimePotRows(sessions: LootFields[], catalog: Catalog, running: LootFields | null): [string, Cell][] {
  const counted = sessions
    .map((s) => ({ s, totals: s === running ? potTotals(s, catalog) : pastPotTotals(s, catalog) }))
    .filter(({ totals }) => totals.readMs > 0);
  if (counted.length === 0) return [];
  const readMs = counted.reduce((sum, { totals }) => sum + totals.readMs, 0);
  const cost = counted.reduce((sum, { totals }) => sum + totals.cost, 0);
  const plus = counted.some(({ totals }) => costAtLeast(totals)) ? "+" : "";
  const rows: [string, Cell][] = [
    ["Pots counted for", formatDuration(readMs)],
    ["Pot cost", `${formatCompact(cost)}${plus}`],
  ];
  const pots = potRate({ cost, readMs });
  const loot = lootRate({ maps: counted.flatMap(({ s }) => s.maps ?? []) });
  if (pots !== null && loot !== null) rows.push(["Net loot/hr", signed(loot - pots)]);
  return rows;
}
