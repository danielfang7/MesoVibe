/**
 * Session Tracker: EXP/hr for the session and the last 5 minutes, time to level, the map, the
 * mesos picked up and what shops paid for the loot (from the meso totals read in the inventory and
 * shop windows), and a history of past sessions kept on this computer. It feeds the session strip
 * and draws the Session view and the Loot tab. Pause stops the session counting until Resume. End
 * saves the session to Past sessions, Reset throws it away and a past session's delete button
 * removes it, each one click with Undo; deleting all history asks first. For 30 minutes after End,
 * what's sold at a shop still counts for the session just ended. Unless the player turns it off in
 * Settings, a session pauses on its own after a minute with no EXP on its map, from the last kill,
 * and the next kill resumes it; a minute off that map with no EXP ends it, from when they left.
 */

import { definePlugin, formatCompact, formatDigits, formatDuration } from "@mesovibe/sdk";
import type { CaptureState, Cell, Readers, RowAction, SessionSummary, ViewNode } from "@mesovibe/sdk";
import { wiredSource } from "./count-source.js";
import type { PotUse } from "./count-source.js";
import {
  addCount,
  addExp,
  addPotUse,
  addReading,
  addTime,
  closeOut,
  countHeld,
  hasLoot,
  HOLD_MS,
  itemsValue,
  ledgerProfit,
  ledgerStale,
  longest,
  leftStorage,
  mapRates,
  MESOS_RATE_MS,
  mergeLoot,
  mesosRate,
  noticeMesosRate,
  openWith,
  profitRate,
  restartSegment,
  sessionItems,
  start as startLoot,
  stopCloseOut,
  takeTime,
  totalMesos,
  uncountHeld,
  uncountSale,
  validCatalog,
  validItems,
  validLedger,
  validMaps,
  validPotSources,
  waitForCloseOut,
} from "./loot.js";
import type { Catalog, ItemEntry, Ledger, LootFields, MapEntry, MesoSource, PotTime, SinceReading } from "./loot.js";
import { COUNT_HELD, heldLines, lootSection } from "./loot-view.js";
import { potRate, potTotals } from "./pot-cost.js";
import { allTimePotRows, potionsSection, potsLine } from "./potions-view.js";
import {
  advance,
  compare,
  finish,
  MAX_TICK_MS,
  MIN_RATE_MS,
  minutesToLevel,
  newSession,
  observe,
  recentRate,
  sessionRate,
} from "./session.js";
import type { PastSession, Reading, SessionState } from "./session.js";

/** How long End, Reset, deleting a past session and counting a held rise can be undone. */
const UNDO_MS = 10_000;
/** What a press of Undo after its ten seconds says. */
const TOO_LATE = {
  "undo-end": "Too late to undo: it's saved in Past sessions.",
  "undo-reset": "Too late to undo the reset.",
  "undo-delete": "Too late to undo: the session is deleted.",
  "undo-count": "Too late to undo: it's counted in Profit.",
  "undo-closeout": "Too late to undo: the sale is counted in that session.",
};
/** How often the running session is saved while nothing changes, so closing MesoVibe loses little. */
const SAVE_EVERY_MS = 30_000;
/** The most often an EXP change saves the session: at most this much play is missing after a close. */
const EXP_SAVE_EVERY_MS = 5_000;
/**
 * The most waiting a session keeps from before its first gain: about the time spent earning that
 * gain, so its EXP isn't measured against no time at all, while a long wait after End still isn't
 * play.
 */
const LEAD_MS = 60_000;
/**
 * How long with no EXP pauses a session on its map, and how long off that map with no EXP ends it:
 * a kill takes seconds, so a minute without one is the player away or done.
 */
const AUTO_MS = 60_000;
/** Past sessions kept. */
const MAX_HISTORY = 500;
/** Past sessions shown in the table, and with "Show more". */
const SHOWN_HISTORY = 20;
const SHOWN_MORE = 50;

/**
 * A session as kept, running or past: its EXP and, since loot tracking, its loot by map. `paused`:
 * no time, EXP or loot counts until Resume. `idle`: the session paused on its own (no EXP for a
 * minute on its map), and its next gain resumes it.
 */
type Session = SessionState & LootFields & { paused?: true; idle?: true };
type Past = PastSession & LootFields;

/**
 * What sessions dropped past MAX_HISTORY add to All time, so it never forgets them: what their
 * meso ledgers counted (mesos picked up, loot sold and the play time between totals) and, for
 * older sessions, what their pickup notices counted.
 */
interface AllTime {
  mesos: number;
  playedMs: number;
  items?: ItemEntry[];
  picked?: number;
  sold?: number;
  measuredMs?: number;
}

interface Saved {
  version: 1;
  history: Past[];
  current: Session | null;
  allTime?: AllTime;
  /** Every item key's name, tab and price, kept once rather than in each session. */
  catalog?: Catalog;
  /** What the count source learned (which potion each quick-slot icon is), as it saved it. */
  slots?: unknown;
  /** False: the player turned off pausing and ending sessions on their own. */
  auto?: false;
}

/**
 * The stretch since the session's last EXP gain, to pause or end it on its own: the gain's map, the
 * capture time since with nothing over the EXP bar (paused or not: a gain can't be seen through a
 * cursor over it), the play time counted since and where it went, and when the player left that map.
 */
interface Quiet {
  map: string | null | undefined;
  /** When the gain was. */
  atMs: number;
  ms: number;
  played: number;
  steps: { map: string | null | undefined; ms: number; pots: PotTime | undefined }[];
  /** The play time a pause keeps after the gain: about one kill's worth. */
  keepMs: number;
  /** When the player left the gain's map: the capture and play time since the gain, and the clock. */
  left: { ms: number; played: number; atMs: number } | null;
}

/**
 * What Undo puts back: the session and history before End, Reset or deleting a past session; for
 * a held rise counted into Profit, the rise to hold again; for a sale the close-out counted, the
 * amount to take back out (`past`: in the past session that started then, not the running one).
 */
interface Undo {
  kind: "end" | "reset" | "delete" | "count" | "closeout";
  session: Session;
  history: Past[];
  allTime: AllTime;
  until: number;
  held?: [number, number];
  sold?: number;
  past?: number;
}

/** The loot fields of a saved session, with anything malformed dropped. */
function validLoot<T extends LootFields>(session: T): T {
  const maps = validMaps(session.maps);
  if (maps === undefined) delete session.maps;
  else session.maps = maps;
  if (session.missed !== true) delete session.missed;
  if (session.shapesWrong !== true) delete session.shapesWrong;
  if (typeof session.unplaced !== "number" || !Number.isFinite(session.unplaced)) delete session.unplaced;
  const held = session.held as unknown;
  if (typeof held !== "object" || held === null || typeof (held as { mesos?: unknown }).mesos !== "number") delete session.held;
  else if (session.held !== undefined) {
    const items = validItems(session.held.items);
    if (items.length > 0) session.held.items = items;
    else delete session.held.items;
  }
  // A malformed potion field is dropped and the session kept.
  const sources = validPotSources(session.ps);
  if (sources === undefined) delete session.ps;
  else session.ps = sources;
  const mesos = (n: unknown) => typeof n === "number" && Number.isFinite(n) && n >= 0;
  if (!mesos(session.pt)) delete session.pt;
  // A malformed meso ledger is dropped and the session kept.
  const ledger = validLedger(session.lg);
  if (ledger === undefined) delete session.lg;
  else session.lg = ledger;
  return session;
}

function load(saved: unknown): Saved {
  const empty: Saved = { version: 1, history: [], current: null };
  if (typeof saved !== "object" || saved === null) return empty;
  const value = saved as Partial<Saved>;
  if (value.version !== 1) return empty;
  const allTime = value.allTime;
  const items = validItems(allTime?.items);
  const count = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n) && n > 0;
  return {
    version: 1,
    history: Array.isArray(value.history)
      ? value.history.filter((s): s is Past => typeof s === "object" && s !== null).map(validLoot)
      : [],
    current: typeof value.current === "object" && value.current !== null ? validLoot(value.current) : null,
    allTime:
      typeof allTime?.mesos === "number" && typeof allTime.playedMs === "number"
        ? {
            mesos: allTime.mesos,
            playedMs: allTime.playedMs,
            ...(items.length > 0 ? { items } : {}),
            ...(count(allTime.picked) ? { picked: allTime.picked } : {}),
            ...(count(allTime.sold) ? { sold: allTime.sold } : {}),
            ...(count(allTime.measuredMs) ? { measuredMs: allTime.measuredMs } : {}),
          }
        : { mesos: 0, playedMs: 0 },
    catalog: validCatalog(value.catalog),
    ...(value.slots === undefined ? {} : { slots: value.slots }),
    ...(value.auto === false ? { auto: false } : {}),
  };
}

/** An `ExpChanged` as the session's arithmetic takes it. */
function readingOf(event: { level: number | null; exp: number; pct_hundredths: number; needed: number | null; exp_shown: boolean }): Reading {
  return { level: event.level, exp: event.exp, pctHundredths: event.pct_hundredths, needed: event.needed, expShown: event.exp_shown };
}

/**
 * About one kill's worth of play: the shorter middle gap between the gains of the last five
 * minutes, at most AUTO_MS. A pause keeps it after the last gain, so a player who kills a slow mob
 * a minute apart isn't left with EXP and no time.
 */
function killTime(recent: [number, number][]): number {
  const times = recent.map(([at]) => at);
  const gaps = times
    .slice(1)
    .map((at, i) => at - (times[i] ?? at))
    .sort((a, b) => a - b);
  return Math.min(gaps[Math.floor((gaps.length - 1) / 2)] ?? 0, AUTO_MS);
}

/** The session is worth saving at End: it gained EXP, or it held loot for five minutes. */
function canEnd(session: Session, atMs: number): boolean {
  if (session.expGained > 0) return true;
  const held = session.held;
  return held !== undefined && hasLoot(held) && atMs - held.atMs >= HOLD_MS;
}

/**
 * What the strip and the mini-HUD need beside the session: until when the session just ended
 * counts sales (null when its close-out isn't open), whether to ask for a meso reading, and pot
 * cost/hr or the ask for the pet's potions in a quick slot.
 */
interface StripExtras {
  closeOutBy: number | null;
  askForMesos: boolean;
  /** Pausing and ending on their own is on. */
  autoPause: boolean;
  potCostPerHour: number | null;
  potsUncounted: boolean;
}

function summary(session: Session, atMs: number, extras: StripExtras): SessionSummary {
  const last = session.last;
  return {
    expPerHour: sessionRate(session),
    recentExpPerHour: recentRate(session),
    expGained: session.expGained,
    level: last?.level ?? null,
    pctHundredths: last?.pctHundredths ?? null,
    minutesToLevel: minutesToLevel(session),
    expToGo: toGo(session),
    expToGoApprox: last?.expShown === false,
    activeMs: session.activeMs,
    lootPerHour: session.lg === undefined ? null : mesosRate(session.lg),
    canEnd: canEnd(session, atMs),
    canReset: session.expGained > 0 || hasLoot(session.held),
    lootOnly: session.expGained <= 0 && hasLoot(session.held),
    paused: session.paused === true,
    autoPaused: session.idle === true,
    autoPause: extras.autoPause,
    canPause: session.expGained > 0 || session.paused === true,
    closeOutBy: extras.closeOutBy,
    askForMesos: extras.askForMesos,
    potCostPerHour: extras.potCostPerHour,
    potsUncounted: extras.potsUncounted,
  };
}

/** What a sale after End says: "Added +212K sold. Session profit 380K (1.1M/hr)." */
function saleNotice(amount: number, ledger: Ledger): string {
  const rate = profitRate(ledger);
  const pace = rate === null ? "" : ` (${formatCompact(rate)}/hr)`;
  return `Added +${formatCompact(amount)} sold. Session profit ${formatCompact(ledgerProfit(ledger))}${pace}.`;
}

/**
 * Keeps the session's potions with its past self: where each count came from, and the cost at
 * today's prices, frozen so a later price change never rewrites it.
 */
function keepPots(past: Past, session: Session, catalog: Catalog): Past {
  if (session.ps !== undefined) past.ps = { ...session.ps };
  const totals = potTotals(past, catalog);
  if (totals.readMs > 0 || totals.lines.length > 0) past.pt = totals.cost;
  return past;
}

/** The session as history keeps it, loot included, when it's worth keeping. */
function finishWithLoot(session: Session, atMs: number, catalog: Catalog): Past | null {
  const past: Past | null = finish(session);
  if (past !== null) {
    if (session.maps !== undefined) past.maps = structuredClone(session.maps);
    if (session.lg !== undefined) past.lg = structuredClone(session.lg);
    if (session.missed) past.missed = true;
    if (session.shapesWrong) past.shapesWrong = true;
    return keepPots(past, session, catalog);
  }
  // Loot for five minutes and no EXP: kept, as a session of its own.
  const held = session.held;
  if (held === undefined || !canEnd(session, atMs)) return null;
  const played = atMs - held.atMs;
  return {
    startedAtMs: held.atMs,
    endedAtMs: atMs,
    activeMs: played,
    expGained: 0,
    expLost: 0,
    startLevel: session.last?.level ?? null,
    endLevel: session.last?.level ?? null,
    maps: [
      {
        k: held.map ?? null,
        p: played,
        x: 0,
        m: held.mesos,
        r: 0,
        l: 0,
        s: session.last?.level ?? null,
        e: session.last?.level ?? null,
        ...(held.shaped > 0 ? { sm: held.shaped } : {}),
        ...(held.missed ? { mi: true as const } : {}),
        ...(held.items !== undefined && held.items.length > 0 ? { i: held.items } : {}),
      },
    ],
    ...(held.missed ? { missed: true as const } : {}),
  };
}

function when(ms: number): string {
  const date = new Date(ms);
  const day = date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const time = date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  // No-break spaces inside each part, so a narrow panel wraps "Nov 3," above "5:15 PM" and nowhere else.
  const keep = (part: string) => part.replace(/ /g, " ");
  return `${keep(day)}, ${keep(time)}`;
}

/** A time of day, "5:45 PM", kept whole on one line. */
function clockTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }).replace(/ /g, " ");
}

/**
 * A past session's levels: "Lv 41 → 43", "Lv 43" when it didn't change, null when unknown.
 * No-break spaces keep "Lv 41" and "→ 43" whole, so a narrow panel breaks only between them.
 */
function levels(past: PastSession): string | null {
  const { startLevel: start, endLevel: end } = past;
  if (start === null && end === null) return null;
  // Levels never go down; a falling range is another character, saved before finish() checked it.
  if (start !== null && end !== null && end < start) return levels({ ...past, startLevel: end });
  if (start === null || end === null || start === end) return `Lv ${end ?? start}`;
  return `Lv ${start} → ${end}`;
}

/**
 * What End says about the session it saved: "Saved: 41,667 EXP in 30m (81.3K/hr).", with its mesos
 * once two meso totals were read: "Saved: 41,667 EXP and 57.8K mesos so far in 30m (83.3K EXP/hr).",
 * "so far" since what's sold after End can still be added.
 */
function recap(session: Session): string {
  const rate = sessionRate(session);
  const ledger = session.lg;
  const mesos = ledger !== undefined && ledger.n >= 2 ? ledgerProfit(ledger) : 0;
  const gained = `${formatDigits(session.expGained)} EXP`;
  if (mesos <= 0) {
    const pace = rate === null ? "" : ` (${formatCompact(rate)}/hr)`;
    return `Saved: ${gained} in ${formatDuration(session.activeMs)}${pace}.`;
  }
  const pace = rate === null ? "" : ` (${formatCompact(rate)} EXP/hr)`;
  return `Saved: ${gained} and ${formatCompact(mesos)} mesos so far in ${formatDuration(session.activeMs)}${pace}.`;
}

/** EXP left to the next level, when the level's size is known. */
function toGo(session: SessionState): number | null {
  const last = session.last;
  if (last === null || last.needed === null || last.needed < last.exp) return null;
  return last.needed - last.exp;
}

/** The day and play time under a past session's map: "Nov 10 · 1h 12m". */
function dayAndTime(past: Past): string {
  const day = new Date(past.startedAtMs).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  return `${day.replace(/ /g, " ")} · ${formatDuration(past.activeMs)}`;
}

/** When each map was first played, oldest first: maps' accessible names tell them apart by it. */
function firstPlayed(history: Past[], session: Session): Map<string, number> {
  const first = new Map<string, number>();
  for (const s of [session, ...history]) {
    for (const entry of s.maps ?? []) {
      if (entry.k === null) continue;
      const was = first.get(entry.k);
      if (was === undefined || s.startedAtMs < was) first.set(entry.k, s.startedAtMs);
    }
  }
  return first;
}

/** A map's name: its picture, or "Unknown map" when its name couldn't be read. */
function mapCell(key: string | null, first: Map<string, number>, suffix?: string): Cell {
  if (key === null) return suffix === undefined ? "Unknown map" : `Unknown map ${suffix}`;
  const played = first.get(key);
  const label = played === undefined ? "A map" : `Map first played ${when(played)}`;
  return suffix === undefined ? { gameText: key, label } : { gameText: key, label, suffix };
}

/** The action that deletes one past session, by when it started. */
const DELETE_SESSION = "delete-session:";

/**
 * What an older session's pickup notices counted: its mesos and its items at NPC prices, its
 * profit as if it sold everything. Sessions since mesos came from meso totals have none.
 */
function lootValue(loot: LootFields, catalog: Catalog): number {
  return totalMesos(loot) + itemsValue(sessionItems(loot), catalog);
}

/**
 * All time: time played and, over every session kept and the running one, the mesos picked up,
 * mesos/hr, loot sold and the play time those were measured over, all from meso totals read. What
 * older sessions' pickup notices counted shows on its own row, marked "≈": it was an estimate, so
 * it never joins a measured figure. With potions counted (`pots`), their rows follow. Empty until
 * anything was counted.
 */
function allTimeView(session: Session, history: Past[], allTime: AllTime, catalog: Catalog, pots: boolean): ViewNode[] {
  const running = session.expGained > 0 ? [session] : [];
  const sessions: (Past | Session)[] = [...history, ...running];
  let picked = allTime.picked ?? 0;
  let sold = allTime.sold ?? 0;
  let measured = allTime.measuredMs ?? 0;
  // Sessions dropped past the 500 kept bring what their notices counted too.
  let noticed = allTime.mesos + itemsValue(allTime.items ?? [], catalog);
  for (const s of sessions) {
    const ledger = s.lg;
    if (ledger !== undefined) {
      picked += ledger.pu;
      sold += ledger.so;
      measured += ledger.pt;
    } else noticed += lootValue(s, catalog);
  }
  const potRows = pots ? allTimePotRows(sessions, catalog, running[0] ?? null) : [];
  if (picked <= 0 && sold <= 0 && noticed <= 0 && potRows.length === 0) {
    return [{ kind: "empty", text: "Mesos you pick up and sell add up here, over every session." }];
  }
  const rows: [string, Cell][] = [["Played", formatDuration(sessions.reduce((sum, s) => sum + s.activeMs, allTime.playedMs))]];
  if (picked > 0) rows.push(["Mesos picked up", formatCompact(picked)]);
  if (measured >= MESOS_RATE_MS) rows.push(["Mesos/hr", formatCompact((picked * 3_600_000) / measured)]);
  if (sold > 0) rows.push(["Loot sold", formatCompact(sold)]);
  if (measured > 0) rows.push(["Measured for", formatDuration(measured)]);
  if (noticed > 0) rows.push(["From pickup notices", `≈${formatCompact(noticed)}`]);
  rows.push(...potRows);
  return [{ kind: "keyValue", items: rows }];
}

/** A past session's map: its longest map's name, "+N maps" after it when it played more. */
function pastMap(past: Past, first: Map<string, number>): { cell: Cell; main: MapEntry | null } {
  const main = longest(past.maps ?? []);
  if (main === null) return { cell: mapCell(null, first), main };
  const others = (past.maps?.length ?? 1) - 1;
  const suffix = others === 0 ? undefined : `+${others} map${others === 1 ? "" : "s"}`;
  return { cell: mapCell(main.k, first, suffix), main };
}

/** The day, play time and levels under a past session's map. */
function pastLines(past: Past): string[] {
  return [dayAndTime(past), levels(past)].filter((line): line is string => line !== null);
}

/** A Past sessions row: [cells, detail lines, its delete button]. */
function pastRow(past: Past, first: Map<string, number>): [Cell[], string[], RowAction] {
  const gained = formatCompact(past.expGained);
  const remove: RowAction = {
    icon: "delete",
    label: `Delete the session from ${when(past.startedAtMs)}`,
    action: `${DELETE_SESSION}${past.startedAtMs}`,
  };
  const { cell, main } = pastMap(past, first);
  // No map's name read, or a session from before maps were tracked: its own EXP/hr.
  if (main === null) {
    const rate = past.activeMs >= MIN_RATE_MS ? formatCompact((past.expGained * 3_600_000) / past.activeMs) : null;
    return [[cell, gained, rate], pastLines(past), remove];
  }
  // The row is named for one map, so its rate is that map's own; EXP gained is the whole session's,
  // over the time played under it.
  const rate = mapRates(main, MIN_RATE_MS).exp;
  return [[cell, gained, rate === null ? null : formatCompact(rate)], pastLines(past), remove];
}

/**
 * The Loot tab: All time on top, then each past session's loot, laid out like Past sessions: its
 * mesos/hr and profit from the meso totals read, "—" where there's none (profit waits for a sale).
 * Under each: what it spent, any rise not counted, until when sales still count for it, and with
 * potions counted (`pots`) a line of its potions. A session saved before mesos came from meso
 * totals keeps what its pickup notices counted, marked "≈" and said so. Sessions from before loot
 * tracking aren't listed.
 */
function lootView(
  session: Session,
  history: Past[],
  allTime: AllTime,
  catalog: Catalog,
  showMore: boolean,
  pots: boolean,
  atMs: number,
): ViewNode[] {
  const first = firstPlayed(history, session);
  const tracked = history.filter((past) => past.maps !== undefined);
  const rows = tracked.slice(0, showMore ? SHOWN_MORE : SHOWN_HISTORY).map((past): [Cell[], string[]] => {
    const map = pastMap(past, first).cell;
    const lines = pastLines(past);
    const ledger = past.lg;
    const noticed = ledger === undefined ? lootValue(past, catalog) : 0;
    let cells: Cell[];
    if (noticed > 0) {
      const rate = noticeMesosRate(past);
      cells = [map, rate === null ? "—" : `≈${formatCompact(rate)}`, `≈${formatCompact(noticed)}`];
      lines.push("Estimated from pickup notices");
    } else {
      const rate = ledger === undefined ? null : mesosRate(ledger);
      const counted = ledger !== undefined && (ledger.so > 0 || ledger.ct > 0);
      const profit = counted ? formatCompact(ledgerProfit(ledger)) : "—";
      cells = [map, rate === null ? "—" : formatCompact(rate), profit];
      if (ledger !== undefined && ledger.sp > 0) lines.push(`Spent ${formatCompact(ledger.sp)}`);
    }
    if (ledger !== undefined && ledger.ho.length > 0) {
      lines.push(`Not counted: ${ledger.ho.map(([amount]) => `+${formatCompact(amount)}`).join(", ")}`);
    }
    if (ledger?.co !== undefined && ledger.co > atMs) lines.push(`Sales count until ${clockTime(ledger.co)}`);
    const line = pots ? potsLine(past, catalog) : null;
    if (line !== null) lines.push(line);
    return [cells, lines];
  });
  // Rises the session just ended held out of Profit can still be counted until its close-out ends.
  const closing = history[0]?.lg;
  const countable: ViewNode[] =
    closing?.co !== undefined && closing.co > atMs && closing.ho.length > 0
      ? [{ kind: "text", text: "From the session you just ended, while its sales still count:", variant: "muted" }, ...heldLines(closing)]
      : [];
  const past: ViewNode[] =
    tracked.length === 0
      ? [{ kind: "empty", text: "Sessions show up here when you end them or close MesoVibe." }]
      : [
          {
            kind: "table",
            columns: ["Map", "Mesos/hr", "Profit"],
            numeric: [false, true, true],
            rows: rows.map(([cells]) => cells),
            details: rows.map(([, lines]) => lines),
          },
          ...countable,
          ...(tracked.length > SHOWN_HISTORY
            ? [{ kind: "button", label: showMore ? "Show fewer" : "Show more", action: "toggle-loot-history", style: "secondary" } as const]
            : []),
        ];
  return [
    { kind: "section", title: "All time", children: allTimeView(session, history, allTime, catalog, pots) },
    { kind: "section", title: "Past sessions", children: past },
  ];
}

/**
 * The Session view. "This session" holds only what the strip above it doesn't show:
 * no EXP/hr, level progress or time to level, which the strip already has. Before the
 * first gain the strip shows the level, its bar and the EXP to go, so nothing here waits on the
 * first gain, and with nothing else to show the section is left out.
 */
function view(
  session: Session,
  history: Past[],
  capture: CaptureState,
  map: string | null | undefined,
  readers: Readers,
  atMs: number,
  showMore: boolean,
  potions: ViewNode | null,
  potsPerHour: number | null,
): ViewNode[] {
  const current: ViewNode[] = [];
  const left = toGo(session);
  const reading = capture === "capturing";
  const first = firstPlayed(history, session);
  const mapRow: [string, Cell][] = map === undefined ? [] : [["Map", mapCell(map, first)]];
  if (session.last === null) {
    if (!reading) current.push({ kind: "loading", text: "Waiting for the game. Your session starts with your first EXP gain." });
  } else if (session.expGained <= 0) {
    // At startup and after End or Reset: no list of zeros, and the EXP to go is in the strip.
    if (!reading) current.push({ kind: "empty", text: "Your session numbers appear after your first gain." });
  } else {
    const recent = recentRate(session);
    const rows: [string, Cell][] = [
      ...mapRow,
      ["Last 5 min EXP/hr", recent === null ? null : formatCompact(recent)],
      ["Time played", formatDuration(session.activeMs)],
      ["EXP gained", formatDigits(session.expGained)],
    ];
    // Worked out from the percent alone, it's within a hundredth of a percent, not exact.
    if (left !== null) rows.push(["EXP to go", `${session.last.expShown === false ? "≈" : ""}${formatDigits(left)}`]);
    if (session.levelUps > 0) rows.push(["Level-ups", String(session.levelUps)]);
    if (session.expLost > 0) rows.push(["EXP lost", formatDigits(session.expLost)]);
    current.push({ kind: "keyValue", items: rows });
  }

  const rows = history.slice(0, showMore ? SHOWN_MORE : SHOWN_HISTORY).map((s) => pastRow(s, first));
  const table: ViewNode = {
    kind: "table",
    columns: ["Map", "EXP gained", "EXP/hr"],
    numeric: [false, true, true],
    rows: rows.map(([cells]) => cells),
    details: rows.map(([, lines]) => lines),
    rowActions: rows.map(([, , remove]) => remove),
  };
  const more: ViewNode[] =
    history.length > SHOWN_HISTORY
      ? [{ kind: "button", label: showMore ? "Show fewer" : "Show more", action: "toggle-history", style: "secondary" }]
      : [];
  const past: ViewNode[] =
    history.length === 0
      ? [{ kind: "empty", text: "Sessions show up here when you end them or close MesoVibe." }]
      : [
          table,
          ...more,
          {
            kind: "button",
            label: "Delete all session history…",
            action: "delete-history",
            style: "danger-link",
            confirm: {
              title: "Delete all session history?",
              deletes: ["Every past session on this computer: EXP gained, time played, rates, mesos and items, and the All time totals."],
              keeps: ["The session running now, your settings, item names and NPC prices, map name pictures, and everything outside the Session Tracker."],
              confirmLabel: "Delete session history",
            },
          },
        ];

  const loot = lootSection(session, { readers, atMs, started: session.expGained > 0, paused: session.paused === true, potsPerHour });
  return [
    ...(current.length === 0 ? [] : [{ kind: "section", title: "This session", children: current } as const]),
    ...(loot === null ? [] : [loot]),
    ...(potions === null ? [] : [potions]),
    { kind: "section", title: "Past sessions", children: past },
  ];
}

definePlugin({
  manifest: { id: "session-tracker", name: "Session Tracker" },
  start(context, startedAtMs, startCapture, startReaders) {
    const saved = load(context.saved);
    let history = saved.history;
    let allTime: AllTime = saved.allTime ?? { mesos: 0, playedMs: 0 };
    const catalog: Catalog = saved.catalog ?? {};
    /** Adds a session to history; one past MAX_HISTORY goes into All time, never lost. */
    const pushHistory = (past: Past) => {
      history = [past, ...history];
      while (history.length > MAX_HISTORY) {
        const dropped = history.pop() as Past;
        const items = (allTime.items ?? []).map((item) => ({ ...item }));
        for (const item of sessionItems(dropped)) addCount(items, item.k, item.c);
        const ledger = dropped.lg;
        const picked = (allTime.picked ?? 0) + (ledger?.pu ?? 0);
        const sold = (allTime.sold ?? 0) + (ledger?.so ?? 0);
        const measuredMs = (allTime.measuredMs ?? 0) + (ledger?.pt ?? 0);
        allTime = {
          mesos: allTime.mesos + totalMesos(dropped),
          playedMs: allTime.playedMs + dropped.activeMs,
          ...(items.length > 0 ? { items } : {}),
          ...(picked > 0 ? { picked } : {}),
          ...(sold > 0 ? { sold } : {}),
          ...(measuredMs > 0 ? { measuredMs } : {}),
        };
      }
    };
    // A session still running when MesoVibe closed ends where it last moved: closing counts as
    // End.
    const leftOver = saved.current === null ? null : finishWithLoot(saved.current, saved.current.lastAtMs, catalog);
    if (leftOver !== null) pushHistory(leftOver);
    // Quitting MesoVibe ends any close-out: a reading after a restart may be hours later.
    for (const past of history) stopCloseOut(past);

    let session: Session = newSession(startedAtMs, null);
    let capture: CaptureState = startCapture;
    let readers: Readers = startReaders ?? { hpMp: "off", hpMpMissing: null };
    /** What counts potions, or null while nothing does: then none are counted or shown. */
    const source = wiredSource(saved.slots);
    /** The map the player is on: undefined until the engine says. */
    let map: string | null | undefined;
    let showMore = false;
    /** The Loot tab's Past sessions shows more. */
    let lootMore = false;
    let undo: Undo | null = null;
    /** The last trusted meso total read with no session running: it can open the next one. */
    let pre: { total: number; source: MesoSource; atMs: number } | null = null;
    /** A total read during End's Undo window, counted in the close-out once that window has passed. */
    let closeLater = false;
    /** What sales after End added while another Undo was open, said once it has passed. */
    let saleLater = 0;
    /** What happened since the session's last meso total: the maps played, and any pause. */
    let since = { maps: new Set<string | null>(), paused: false };
    /** "Mesos read" was said for this session, or the wait before it. */
    let readSaid = false;
    let savedAt = Number.NEGATIVE_INFINITY;
    // An EXP change the 5 s cap held back; the next tick past the cap saves it.
    let unsaved = false;
    /** Pausing and ending sessions on their own (Settings): on unless the player turned it off. */
    let auto = saved.auto !== false;
    /** The stretch since the running session's last gain; null before one, or after End or Reset. */
    let quiet: Quiet | null = null;
    /** Starts the stretch afresh from now, on the map the player is on. */
    const quietFrom = (atMs: number): Quiet => ({ map, atMs, ms: 0, played: 0, steps: [], keepMs: killTime(session.recent), left: null });

    /** Saves when `gapMs` has passed since the last save (0: now). */
    const persist = (atMs: number, gapMs = SAVE_EVERY_MS) => {
      if (atMs - savedAt < gapMs) return;
      const running = session.last === null && session.held === undefined ? null : session;
      const slots = source?.saved?.();
      context.save({
        version: 1,
        history,
        current: running,
        allTime,
        catalog,
        ...(slots === undefined ? {} : { slots }),
        ...(auto ? {} : { auto: false }),
      } satisfies Saved);
      savedAt = atMs;
      unsaved = false;
    };
    /** Until when the session just ended counts sales, once End's Undo has passed. */
    const closeOutBy = (atMs: number): number | null => {
      const due = history[0]?.lg?.co;
      if (session.expGained > 0 || due === undefined || due < atMs) return null;
      return undo?.kind === "end" && atMs <= undo.until ? null : due;
    };
    /**
     * Whether to ask for the inventory: the host reads the meso total, and the running session has
     * none read yet, or only one older than 15 minutes.
     */
    const askForMesos = (atMs: number): boolean => {
      if (readers.mesos !== "reading" || session.expGained <= 0 || session.paused) return false;
      const ledger = session.lg;
      return ledger === undefined || ledger.b === null || ledgerStale(ledger, atMs);
    };
    const show = (atMs: number) => {
      const pots = source === null || !potsShown() ? null : potTotals(session, catalog);
      const potsPerHour = pots === null ? null : potRate(pots);
      // None counted where they could be: the pet's potions may not be in a quick slot, the only
      // place their count is read.
      const potsUncounted = pots !== null && session.expGained > 0 && pots.lines.length === 0 && readers.slots !== "unsupported";
      context.setSummary(
        summary(session, atMs, {
          closeOutBy: closeOutBy(atMs),
          askForMesos: askForMesos(atMs),
          autoPause: auto,
          potCostPerHour: pots !== null && pots.lines.length > 0 ? potsPerHour : null,
          potsUncounted,
        }),
      );
      const potions = source === null || pots === null ? null : potionsSection(session, catalog, source, readers, atMs);
      context.render(view(session, history, capture, map, readers, atMs, showMore, potions, potsPerHour));
      // Saved potions show in the Loot tab whenever a source is wired in, reading or not.
      context.render(lootView(session, history, allTime, catalog, lootMore, source !== null, atMs), "loot");
    };
    const remember = (kind: Undo["kind"], atMs: number) => {
      undo = { kind, session: structuredClone(session), history: history.slice(), allTime, until: atMs + UNDO_MS };
    };
    /** Whether potions can show: a source is wired in, and the host reads the quick slots if it counts from them. */
    const potsShown = (): boolean => source !== null && (source.readsSlots !== true || readers.slots !== undefined);
    /** Whether the quick slots are read, or the source doesn't need them. */
    const slotsRead = (): boolean => source?.readsSlots !== true || readers.slots === "reading";
    /**
     * Whether potions are being counted now: a source is wired in, the HP and MP numbers are read,
     * and so are the quick slots if it counts from them.
     */
    const potTime = (atMs: number): PotTime | undefined =>
      source === null || readers.hpMp !== "reading" || !slotsRead()
        ? undefined
        : { counting: true, healing: source.healing("hp", atMs) || source.healing("mp", atMs) };
    /** Tells the count source to drop the uses it holds back; a fault in it never stops the tracker. */
    const forgetHeld = () => {
      try {
        source?.forget?.();
      } catch {
        // Nothing to undo: the source's own state is all it would have touched.
      }
    };
    /**
     * Moves the session's clock, giving the time played to the map the player is on, and noting it
     * in the stretch since the last gain, which a pause or an end on its own can take back.
     */
    const tick = (atMs: number) => {
      const before = session.activeMs;
      // A host that doesn't say whether the EXP bar is covered counts all capture time.
      const seen = capture === "capturing" && readers.expBar !== "covered";
      const captured = seen ? Math.min(Math.max(0, atMs - session.lastAtMs), MAX_TICK_MS) : 0;
      const pots = potTime(atMs);
      advance(session, atMs, capture === "capturing" && session.paused !== true);
      const step = session.activeMs - before;
      addTime(session, map, step, session.last?.level ?? null, pots);
      if (quiet !== null) {
        quiet.ms += captured;
        quiet.played += step;
        const last = quiet.steps.at(-1);
        const same = last !== undefined && last.map === map && last.pots?.counting === pots?.counting && last.pots?.healing === pots?.healing;
        if (step > 0 && same) last.ms += step;
        else if (step > 0) quiet.steps.push({ map, ms: step, pots });
      }
      if (session.expGained > 0) {
        if (map !== undefined && session.activeMs > before) since.maps.add(map);
        // A pause on its own isn't a trip to a shop: the player stayed on the map.
        if (session.paused && !session.idle) since.paused = true;
      }
    };
    /**
     * Takes back the play time since the last gain past its first `keepMs`, from the session and
     * each map it went to, newest first.
     */
    const takeBack = (keepMs: number) => {
      if (quiet === null || quiet.played <= keepMs) return;
      let over = quiet.played - keepMs;
      session.activeMs -= over;
      quiet.played = keepMs;
      for (let step = quiet.steps.at(-1); over > 0 && step !== undefined; step = quiet.steps.at(-1)) {
        const take = Math.min(step.ms, over);
        takeTime(session, step.map, take, step.pots);
        step.ms -= take;
        over -= take;
        if (step.ms <= 0) quiet.steps.pop();
      }
      // A meso total read in that time keeps its play time as read: the next total's stretch is
      // measured from it, so it leaves out the time taken back.
    };
    /** Saves the session to Past sessions and starts the next one at the next gain; `why` opens its notice. */
    const endSession = (atMs: number, why = "") => {
      const past = finishWithLoot(session, atMs, catalog);
      if (past === null) return;
      const said = recap(session);
      remember("end", atMs);
      // Sales after End count for the session where the meso total can be read.
      const reads = readers.mesos === "reading" || readers.mesos === "learning" || (past.lg?.b ?? null) !== null;
      waitForCloseOut(past, atMs, reads);
      pushHistory(past);
      session = newSession(atMs, session.last);
      quiet = null;
      forgetHeld();
      readSaid = false;
      pre = null;
      persist(atMs, 0);
      context.notice(`${why}${said}`, "undo-end");
    };
    /**
     * Pauses the session after a minute with no EXP on its map, from about a kill after the last
     * gain; ends it after a minute off that map with no EXP, from when the player left. Never a
     * session the player paused: that's theirs to resume.
     */
    const quietCheck = (atMs: number) => {
      if (!auto || quiet === null || session.expGained <= 0) return;
      if (session.paused && !session.idle) return;
      const left = quiet.left;
      if (left !== null) {
        // Not over another notice's Undo (a delete's, a count's): ending would take it away. The
        // end waits for it, still dated from when the player left.
        if (quiet.ms - left.ms < AUTO_MS || (undo !== null && atMs <= undo.until)) return;
        takeBack(left.played);
        // The session ends when its time stopped: when the player left, or when it paused.
        session.lastAtMs = Math.min(session.lastAtMs, session.idle ? quiet.atMs + quiet.played : left.atMs);
        endSession(atMs, "Ended when you left the map. ");
      } else if (!session.paused && quiet.ms >= AUTO_MS) {
        takeBack(quiet.keepMs);
        session.paused = true;
        session.idle = true;
        persist(atMs, 0);
      }
    };
    /** Starts afresh what's noted between meso totals, from the map the player is on. */
    const resetSince = () => {
      since = { maps: new Set(map === undefined ? [] : [map]), paused: false };
    };
    /**
     * What the session did since its last meso total. A pause makes a rise what a shop paid, not
     * mesos picked up. Time in a town doesn't: a potion run passes through one, and what a shop pays
     * is read in its window. The stretch is one map's only when that map is a grinding one (the
     * session gained EXP there), so a town never gets a mesos/hr.
     */
    const sinceReading = (): SinceReading => {
      const played = [...since.maps];
      const grinding = (key: string | null | undefined) => ((session.maps ?? []).find((m) => m.k === key)?.x ?? 0) > 0;
      const one = played.length === 1 && grinding(played[0]) ? played[0] : undefined;
      return { playMs: session.activeMs, map: one, away: since.paused };
    };
    /**
     * Counts a total read while the session just ended still counts its sales (its close-out).
     * Each sale says what it added, with an Undo for that sale; a rise too big for farming says
     * it's held. True when it said something.
     */
    const closeOutWith = (total: number, source: MesoSource, atMs: number): boolean => {
      const last = history[0];
      if (last?.lg?.co === undefined) return false;
      const change = closeOut(last, total, source, atMs);
      persist(atMs, 0);
      if (change?.kind === "sold" && last.lg !== undefined) {
        // Never over another Undo still open (a delete's, a count's): the panel shows one notice,
        // so its Undo would be lost. The sale says what it added once that Undo has passed.
        if (undo !== null && atMs <= undo.until && undo.kind !== "closeout") {
          saleLater += change.amount;
          return true;
        }
        undo = { kind: "closeout", session, history, allTime, until: atMs + UNDO_MS, sold: change.amount, past: last.startedAtMs };
        context.notice(saleNotice(change.amount, last.lg), "undo-closeout");
        return true;
      }
      if (change?.kind === "held") {
        context.notice(`+${formatCompact(change.amount)} not counted: too big a jump for farming (a trade or a misread)`);
        return true;
      }
      return false;
    };
    /**
     * A trusted meso total. With no session running it goes to the close-out of the one just ended
     * (after End's Undo window) and is kept to open the next; during a session the ledger counts
     * the change since the last total. The first total of a session says it was read.
     */
    const countMesos = (total: number, source: MesoSource, atMs: number) => {
      let said = false;
      if (session.expGained <= 0) {
        // During End's Undo window the close-out waits, then counts the last total read in it.
        if (undo?.kind === "end" && atMs <= undo.until) closeLater = history[0]?.lg?.co !== undefined;
        else said = closeOutWith(total, source, atMs);
        pre = { total, source, atMs };
      } else {
        addReading(session, total, source, atMs, sinceReading());
        resetSince();
        unsaved = true;
        persist(atMs, EXP_SAVE_EVERY_MS);
      }
      // Never over another notice's Undo (End's, a count's, a sale's): it would take that Undo away.
      const undoOpen = undo !== null && atMs <= undo.until;
      if (!readSaid && !said && !undoOpen) context.notice(`Mesos read: ${formatDigits(total)}`);
      if (said || !undoOpen) readSaid = true;
    };
    persist(startedAtMs, leftOver !== null ? 0 : SAVE_EVERY_MS);
    show(startedAtMs);

    return {
      onEvent(event, atMs) {
        tick(atMs);
        // The source sees every event, paused or not, so it always knows the last reading. A fault
        // in it never stops EXP and loot counting.
        let uses: PotUse[] = [];
        try {
          uses = source?.onEvent(event, atMs) ?? [];
        } catch {
          uses = [];
        }
        // A session paused on its own resumes at its next gain, which counts.
        if (event.type === "ExpChanged" && session.idle && compare(session.last, readingOf(event)).kind === "gain") {
          delete session.paused;
          delete session.idle;
        }
        if (event.type === "ExpChanged" && session.paused) {
          // Paused, the bar is still followed: the strip's level stays true, and Resume counts only
          // what's gained after it. A level-up still counts, so the session's levels add up.
          const reading = readingOf(event);
          const change = compare(session.last, reading);
          if (change.kind === "gain" && change.levelUp) session.levelUps += 1;
          if (change.kind === "gain") leftStorage(session);
          // Another character or a restarted game: the meso ledger starts a new segment too.
          if (change.kind === "baseline" && session.last !== null) restartSegment(session);
          session.last = reading;
          show(atMs);
        } else if (event.type === "ExpChanged") {
          const firstGain = session.expGained <= 0;
          const before = session.last;
          const gainedBefore = session.expGained;
          const change = observe(session, readingOf(event), atMs);
          // Another character or a restarted game: the meso ledger starts a new segment too. After
          // End, the ended session stops counting sales and the total read before is forgotten:
          // another character's mesos are neither.
          if (change.kind === "baseline" && before !== null) restartSegment(session);
          // After End, a reading that can't follow the last one (a jump, or less EXP at the same
          // level: another character, or rarely a death) ends the ended session's sales window and
          // forgets the total read before, so another character's mesos are never counted there.
          if (firstGain && before !== null && (change.kind === "baseline" || change.kind === "loss")) {
            if (history[0] !== undefined) stopCloseOut(history[0]);
            closeLater = false;
            pre = null;
          }
          // A session starts at its first gain: a wait before it, such as after End or Reset,
          // keeps at most LEAD_MS, and its level and losses start from the reading before the gain.
          if (firstGain && session.expGained > 0) {
            const lead = Math.min(session.activeMs, LEAD_MS);
            session.startedAtMs = atMs - lead;
            session.activeMs = lead;
            session.startLevel = before?.level ?? event.level;
            session.expLost = 0;
            // The lead is no potion time: a potion drunk before the first gain isn't the session's,
            // so its time isn't either.
            startLoot(session, map, lead, event.level);
            // Potions held back for a name before the first gain aren't this session's.
            forgetHeld();
            // The session just ended stops counting its sales, and a total read in the five
            // minutes before this kill opens this one.
            if (history[0] !== undefined) stopCloseOut(history[0]);
            closeLater = false;
            if (pre !== null) openWith(session, pre.total, pre.atMs, atMs, pre.source);
            pre = null;
            resetSince();
          }
          if (session.expGained > gainedBefore) leftStorage(session);
          // The stretch since the last gain starts again here, on this map.
          if (change.kind === "gain") quiet = quietFrom(atMs);
          addExp(session, map, session.expGained - gainedBefore, event.level);
          // Saved on EXP changes too, at most every 5 s, so a close loses at most that.
          unsaved = true;
          persist(atMs, EXP_SAVE_EVERY_MS);
          show(atMs);
        } else if (event.type === "MapChanged") {
          // Play time from before the first map name was read moves to that map with the next
          // tick, so the stretch's record of it does too.
          if (map === undefined && quiet !== null) {
            for (const step of quiet.steps) if (step.map === undefined) step.map = event.map_key;
          }
          map = event.map_key;
          // Leaving is from one map to another, both names read: a name that can't be read (the
          // minimap closed) says nothing, and one read only now is where the gain was.
          const to = event.map_key;
          if (quiet !== null && to !== null) {
            if (quiet.map === null || quiet.map === undefined) quiet.map = to;
            else if (to === quiet.map) quiet.left = null;
            else if (quiet.left === null) quiet.left = { ms: quiet.ms, played: quiet.played, atMs };
          }
          show(atMs);
        } else if (event.type === "MesoChanged") {
          // Counted while paused too: a change across a pause is what a shop paid or what was spent.
          countMesos(event.total, event.source, atMs);
          show(atMs);
        }
        // Potions drunk while paused aren't the session's: the source still sees those readings,
        // so the first change after Resume is measured from the last one while paused. HP and MP
        // readings come four times a second, so only a counted use redraws and saves; the 1 Hz
        // tick does the rest.
        // From a source counting from the quick slots, uses count only while they're read, as pot
        // time does.
        if (uses.length > 0 && !session.paused && slotsRead()) {
          for (const use of uses) addPotUse(session, use, map, session.expGained > 0, session.last?.level ?? null);
          unsaved = true;
          persist(atMs, EXP_SAVE_EVERY_MS);
          show(atMs);
        }
      },
      onCapture(state, atMs) {
        tick(atMs);
        capture = state;
        show(atMs);
      },
      onReaders(next, atMs) {
        tick(atMs);
        readers = next;
        show(atMs);
      },
      onTick(atMs) {
        tick(atMs);
        quietCheck(atMs);
        if (undo !== null && atMs > undo.until) undo = null;
        // A sale read while another Undo was open says what it added now that the Undo has passed.
        if (saleLater > 0 && undo === null) {
          const ledger = history[0]?.lg;
          if (ledger !== undefined) context.notice(saleNotice(saleLater, ledger));
          saleLater = 0;
        }
        // A total read during End's Undo window goes to the close-out once Undo has passed.
        if (closeLater && undo === null) {
          closeLater = false;
          if (pre !== null && session.expGained <= 0) closeOutWith(pre.total, pre.source, atMs);
        }
        // The session just ended stops counting sales 30 minutes after End.
        const ended = history[0];
        if (ended?.lg?.co !== undefined && atMs > ended.lg.co) {
          stopCloseOut(ended);
          persist(atMs, 0);
        }
        persist(atMs, unsaved ? EXP_SAVE_EVERY_MS : SAVE_EVERY_MS);
        show(atMs);
      },
      onAction(action, atMs) {
        tick(atMs);
        if (action === "end") {
          endSession(atMs);
        } else if (action === "reset") {
          // Throws the session's numbers away and starts counting afresh; history is untouched.
          if (session.expGained > 0 || session.held !== undefined) {
            remember("reset", atMs);
            session = newSession(atMs, session.last);
            quiet = null;
            forgetHeld();
            readSaid = false;
            pre = null;
            persist(atMs, 0);
            context.notice("Session reset.", "undo-reset");
          }
        } else if (action === "pause") {
          // The clock above ran up to the press; from here nothing counts until Resume.
          if (session.expGained > 0 && session.paused !== true) {
            session.paused = true;
            persist(atMs, 0);
          }
        } else if (action === "resume") {
          // Potions held back for a name before Pause or while paused aren't counted after it; a
          // pause on its own kept the player on the map, so theirs still count. The stretch with no
          // EXP starts again from the press.
          if (session.paused) {
            if (!session.idle) forgetHeld();
            delete session.paused;
            delete session.idle;
            quiet = quietFrom(atMs);
            persist(atMs, 0);
          }
        } else if (action === "auto-on" || action === "auto-off") {
          // Turned back on, the minute with no EXP counts from the press: the time while it was off
          // stays the player's, and a map left then counts as left now.
          if (action === "auto-on" && !auto && quiet !== null) {
            quiet = { ...quiet, ms: 0, played: 0, steps: [], left: quiet.left === null ? null : { ms: 0, played: 0, atMs } };
          }
          auto = action === "auto-on";
          persist(atMs, 0);
        } else if (action.startsWith(DELETE_SESSION)) {
          // One past session, by when it started: one click, with Undo.
          const startedAtMs = Number(action.slice(DELETE_SESSION.length));
          const at = history.findIndex((past) => past.startedAtMs === startedAtMs);
          if (at !== -1) {
            remember("delete", atMs);
            history = history.filter((_, i) => i !== at);
            persist(atMs, 0);
            context.notice("Session deleted.", "undo-delete");
          }
        } else if (action === "undo-end" || action === "undo-reset" || action === "undo-delete") {
          if (undo !== null && action === `undo-${undo.kind}` && atMs <= undo.until) {
            if (undo.kind === "delete") {
              // Only the history: the running session carries on. End, Reset or another delete
              // since would have replaced this Undo, so the history is as it was left.
              history = undo.history;
            } else {
              // Mesos and potions counted since End or Reset belong to the session put back.
              const sinceEnd = session;
              ({ session, history, allTime } = undo);
              mergeLoot(session, sinceEnd);
              session.lastAtMs = atMs;
              // Its stretch since the last gain is gone: the minute with no EXP starts again from the
              // press, as after Resume.
              quiet = quietFrom(atMs);
              // The session runs again: nothing waits to close out, and its meso notes start afresh.
              closeLater = false;
              pre = null;
              readSaid = session.lg !== undefined;
              resetSince();
            }
            undo = null;
            persist(atMs, 0);
            context.notice("Session restored.");
          } else if (undo === null || atMs > undo.until) {
            // The panel's Undo can outlive the window by a tick; say so rather than do nothing.
            undo = null;
            context.notice(TOO_LATE[action]);
          }
        } else if (action.startsWith(COUNT_HELD)) {
          // A rise held out of Profit as too big for farming, counted on the player's word: in the
          // running session, or in the one just ended while its sales still count.
          const readAt = Number(action.slice(COUNT_HELD.length));
          const last = history[0];
          const closing = last?.lg?.co !== undefined && last.lg.co >= atMs ? last : undefined;
          const owner = session.lg?.ho.some(([, when]) => when === readAt) ? session : closing;
          const at = owner?.lg?.ho.findIndex(([, when]) => when === readAt) ?? -1;
          const held = owner === undefined || at === -1 ? null : countHeld(owner, at);
          if (held !== null) {
            undo = {
              kind: "count",
              session,
              history,
              allTime,
              until: atMs + UNDO_MS,
              held,
              ...(owner === session ? {} : { past: last?.startedAtMs }),
            };
            persist(atMs, 0);
            context.notice(`Counted +${formatCompact(held[0])} in Profit.`, "undo-count");
          }
        } else if (action === "undo-count") {
          if (undo?.kind === "count" && undo.held !== undefined && atMs <= undo.until) {
            const startedAt = undo.past;
            const owner = startedAt === undefined ? session : history.find((past) => past.startedAtMs === startedAt);
            if (owner !== undefined) uncountHeld(owner, undo.held);
            const rise = formatCompact(undo.held[0]);
            undo = null;
            persist(atMs, 0);
            context.notice(`+${rise} is held out of Profit again.`);
          } else if (undo === null || atMs > undo.until) {
            undo = null;
            context.notice(TOO_LATE[action]);
          }
        } else if (action === "undo-closeout") {
          // A rise that wasn't a sale (a trade, say): it comes back out of the session, and the
          // balance stays as read, so the next sale is measured from it.
          if (undo?.kind === "closeout" && undo.sold !== undefined && atMs <= undo.until) {
            const startedAt = undo.past;
            const owner = history.find((past) => past.startedAtMs === startedAt);
            if (owner !== undefined) uncountSale(owner, undo.sold);
            const amount = formatCompact(undo.sold);
            undo = null;
            persist(atMs, 0);
            context.notice(`Sale undone: +${amount} isn't counted.`);
          } else if (undo === null || atMs > undo.until) {
            undo = null;
            context.notice(TOO_LATE[action]);
          }
        } else if (action === "toggle-history") {
          showMore = !showMore;
        } else if (action === "toggle-loot-history") {
          lootMore = !lootMore;
        } else if (action === "delete-history") {
          history = [];
          allTime = { mesos: 0, playedMs: 0 };
          undo = null;
          persist(atMs, 0);
          context.notice("Session history deleted.");
        }
        show(atMs);
      },
    };
  },
});
