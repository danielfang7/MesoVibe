/**
 * Session Tracker: EXP/hr for the session and the last 5 minutes, time to
 * level, the map, the mesos and items picked up and what they're worth, and a history of past
 * sessions kept on this computer. It
 * feeds the session strip and draws the Session view and the Loot tab. Pause stops the session counting until
 * Resume. End saves the session to Past sessions, Reset throws it away and a past session's delete
 * button removes it, each one click with Undo; deleting all history asks first.
 */

import { definePlugin, formatCompact, formatDigits, formatDuration } from "@mesovibe/sdk";
import type { CaptureState, Cell, Readers, RowAction, SessionSummary, ViewNode } from "@mesovibe/sdk";
import {
  addCount,
  addExp,
  addItem,
  addPickup,
  addTime,
  hasLoot,
  HOLD_MS,
  itemsValue,
  longest,
  lootRate,
  mapRates,
  MESOS_RATE_MS,
  mergeLoot,
  reads,
  sessionItems,
  start as startLoot,
  sumItems,
  totalMesos,
  validCatalog,
  validItems,
  validMaps,
} from "./loot.js";
import type { Catalog, ItemEntry, ItemInfo, LootFields, MapEntry } from "./loot.js";
import {
  advance,
  compare,
  finish,
  MIN_RATE_MS,
  minutesToLevel,
  newSession,
  observe,
  recentRate,
  sessionRate,
} from "./session.js";
import type { PastSession, SessionState } from "./session.js";

/** How long End, Reset and deleting a past session can be undone. */
const UNDO_MS = 10_000;
/** What a press of Undo after its ten seconds says. */
const TOO_LATE = {
  "undo-end": "Too late to undo: it's saved in Past sessions.",
  "undo-reset": "Too late to undo the reset.",
  "undo-delete": "Too late to undo: the session is deleted.",
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
/** Past sessions kept. */
const MAX_HISTORY = 500;
/** Past sessions shown in the table, and with "Show more". */
const SHOWN_HISTORY = 20;
const SHOWN_MORE = 50;
/** After this long still learning the pickup text, the line says what helps. */
const LEARNING_LONG_MS = 5 * 60_000;
/** Items listed before "Show all". */
const SHOWN_ITEMS = 8;

/**
 * A session as kept, running or past: its EXP and, since loot tracking, its loot by map. `paused`:
 * the player paused it, so no time, EXP or loot counts until Resume.
 */
type Session = SessionState & LootFields & { paused?: true };
type Past = PastSession & LootFields;

/** What sessions dropped past MAX_HISTORY add to All time, so it never forgets them. */
interface AllTime {
  mesos: number;
  playedMs: number;
  items?: ItemEntry[];
}

interface Saved {
  version: 1;
  history: Past[];
  current: Session | null;
  allTime?: AllTime;
  /** Every item key's name, tab and price, kept once rather than in each session. */
  catalog?: Catalog;
}

/** What Undo puts back: the session and history before End, Reset or deleting a past session. */
interface Undo {
  kind: "end" | "reset" | "delete";
  session: Session;
  history: Past[];
  allTime: AllTime;
  until: number;
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
  return session;
}

function load(saved: unknown): Saved {
  const empty: Saved = { version: 1, history: [], current: null };
  if (typeof saved !== "object" || saved === null) return empty;
  const value = saved as Partial<Saved>;
  if (value.version !== 1) return empty;
  const allTime = value.allTime;
  const items = validItems(allTime?.items);
  return {
    version: 1,
    history: Array.isArray(value.history)
      ? value.history.filter((s): s is Past => typeof s === "object" && s !== null).map(validLoot)
      : [],
    current: typeof value.current === "object" && value.current !== null ? validLoot(value.current) : null,
    allTime:
      typeof allTime?.mesos === "number" && typeof allTime.playedMs === "number"
        ? { mesos: allTime.mesos, playedMs: allTime.playedMs, ...(items.length > 0 ? { items } : {}) }
        : { mesos: 0, playedMs: 0 },
    catalog: validCatalog(value.catalog),
  };
}

/** The session is worth saving at End: it gained EXP, or it held loot for five minutes. */
function canEnd(session: Session, atMs: number): boolean {
  if (session.expGained > 0) return true;
  const held = session.held;
  return held !== undefined && hasLoot(held) && atMs - held.atMs >= HOLD_MS;
}

/** Whether a loot rate means anything: not while the pickup text is learned or the notices are covered. */
function lootShown(readers: Readers): boolean {
  return readers.notices !== "learning" && readers.notices !== "blocked";
}

function summary(session: Session, atMs: number, readers: Readers): SessionSummary {
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
    lootPerHour: lootShown(readers) ? lootRate(session) : null,
    canEnd: canEnd(session, atMs),
    canReset: session.expGained > 0 || hasLoot(session.held),
    lootOnly: session.expGained <= 0 && hasLoot(session.held),
    paused: session.paused === true,
    canPause: session.expGained > 0 || session.paused === true,
  };
}

/** The session as history keeps it, loot included, when it's worth keeping. */
function finishWithLoot(session: Session, atMs: number): Past | null {
  const past: Past | null = finish(session);
  if (past !== null) {
    if (session.maps !== undefined) past.maps = structuredClone(session.maps);
    if (session.missed) past.missed = true;
    if (session.shapesWrong) past.shapesWrong = true;
    return past;
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

/** Compact mesos, "+" when some pickups were missed (the total is at least that). */
function mesosText(amount: number, partial: boolean, compact: boolean): string {
  return `${compact ? formatCompact(amount) : formatDigits(amount)}${partial ? "+" : ""}`;
}

function when(ms: number): string {
  const date = new Date(ms);
  const day = date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  const time = date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  // No-break spaces inside each part, so a narrow panel wraps "Nov 3," above "5:15 PM" and nowhere else.
  const keep = (part: string) => part.replace(/ /g, " ");
  return `${keep(day)}, ${keep(time)}`;
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
 * when it counted any: "Saved: 41,667 EXP and 57.8K mesos in 30m (83.3K EXP/hr)."
 */
function recap(session: Session): string {
  const rate = sessionRate(session);
  const mesos = totalMesos(session);
  if (mesos <= 0 && session.expGained <= 0) {
    // Items held five minutes without a kill.
    const count = sessionItems(session).reduce((sum, item) => sum + item.c, 0);
    return `Saved: ${formatDigits(count)} item${count === 1 ? "" : "s"}.`;
  }
  if (mesos <= 0) {
    const pace = rate === null ? "" : ` (${formatCompact(rate)}/hr)`;
    return `Saved: ${formatDigits(session.expGained)} EXP in ${formatDuration(session.activeMs)}${pace}.`;
  }
  const pace = rate === null ? "" : ` (${formatCompact(rate)} EXP/hr)`;
  const loot = `${mesosText(mesos, session.missed === true, true)} mesos`;
  if (session.expGained <= 0) return `Saved: ${loot}.`;
  return `Saved: ${formatDigits(session.expGained)} EXP and ${loot} in ${formatDuration(session.activeMs)}${pace}.`;
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

/** What the plugin knows about reading the pickup notices, for the Session view's lines. */
interface Notices {
  readers: Readers;
  /** Since when the notices have been learned, while they are. */
  learningSince: number | null;
  atMs: number;
}

/** The muted lines under This session about reading pickups, if there are any to show. */
function noticeLines(session: Session, notices: Notices): ViewNode[] {
  const lines: ViewNode[] = [];
  const { readers } = notices;
  if (readers.notices === "learning") {
    const long = notices.learningSince !== null && notices.atMs - notices.learningSince >= LEARNING_LONG_MS;
    lines.push({
      kind: "empty",
      text: long
        ? "Still learning the pickup text: it learns from EXP lines while the notices are in view. See What MesoVibe sees."
        : "Learning the pickup text. Mesos show after a few kills.",
    });
  } else if (readers.notices === "blocked") {
    lines.push({ kind: "partial", text: "Pickups aren't being read: something keeps covering the notices, like a party list." });
  }
  if (session.shapesWrong) {
    lines.push({ kind: "partial", text: "Some mesos amounts may be wrong: a digit was misread and is no longer used." });
  }
  return lines;
}

/**
 * The line saying some pickups weren't read, so the items and their value may be short. It goes
 * at the bottom of Items, or of This session before any item. Not with a misread digit, whose line
 * says more, nor while the notices are covered, whose line says why.
 */
function missingLine(session: Session, readers: Readers): ViewNode[] {
  if (!session.missed || session.shapesWrong || readers.notices === "blocked") return [];
  return [{ kind: "partial", text: "Some pickups may be missing: MesoVibe couldn't read every notice." }];
}

/** The action that deletes one past session, by when it started. */
const DELETE_SESSION = "delete-session:";
/** The action that shows all of one item list, or fewer again, by its group. */
const TOGGLE_ITEMS = "toggle-items:";

/**
 * Whether `items` are worth at least their counted value, not exactly: some had names no item fit,
 * or fit several items, counted at the lowest price they may have or, with no price, not at all.
 */
function valueAtLeast(items: ItemEntry[], catalog: Catalog): boolean {
  return items.some((item) => {
    const info = catalog[item.k];
    return info === undefined || info.n === null || (info.v === null && !item.k.startsWith("#"));
  });
}

/**
 * A named item's row: its name, the count and what an NPC shop pays for them all, with its tab
 * under the name when that isn't Etc, where most drops go. A name several items fit that sell for
 * different prices (as a name the game cut short can) shows the lowest and highest, "4.2K–7K".
 * With no price, "—" when it's one item shops won't buy, and no value ("?") when which item it is
 * isn't known.
 */
function itemRow(item: ItemEntry, name: string, info: ItemInfo): { cells: Cell[]; detail: string | null } {
  const value =
    info.v !== null
      ? formatCompact(item.c * info.v)
      : info.r !== undefined
        ? `${formatCompact(item.c * info.r[0])}–${formatCompact(item.c * info.r[1])}`
        : item.k.startsWith("#")
          ? "—"
          : null;
  return { cells: [name, `×${formatDigits(item.c)}`, value], detail: info.t === null || info.t === "Etc" ? null : info.t };
}

/**
 * A list of items, the most picked up first (ties in the order first picked up), the first 8 unless
 * its group is expanded, with "Show all N items" or "Show fewer" below when there are more.
 * Pickups whose names fit no item are one "Unread items" row with no value, always last and always
 * shown.
 */
function itemList(items: ItemEntry[], catalog: Catalog, group: string, expanded: Set<string>): ViewNode[] {
  const named: { item: ItemEntry; name: string; info: ItemInfo; i: number }[] = [];
  let unreadCount = 0;
  items.forEach((item, i) => {
    const info = catalog[item.k];
    if (info === undefined || info.n === null) unreadCount += item.c;
    else named.push({ item, name: info.n, info, i });
  });
  const sorted = named.sort((a, b) => b.item.c - a.item.c || a.i - b.i);
  const all = expanded.has(group);
  const rows = (all ? sorted : sorted.slice(0, SHOWN_ITEMS)).map(({ item, name, info }) => itemRow(item, name, info));
  if (unreadCount > 0) rows.push({ cells: ["Unread items", `×${formatDigits(unreadCount)}`, null], detail: null });
  const nodes: ViewNode[] = [
    {
      kind: "table",
      columns: ["Item", "Count", "Value"],
      numeric: [false, true, true],
      rows: rows.map((row) => row.cells),
      details: rows.map((row) => row.detail),
    },
  ];
  if (sorted.length > SHOWN_ITEMS) {
    nodes.push({ kind: "button", label: all ? "Show fewer" : `Show all ${sorted.length} items`, action: `${TOGGLE_ITEMS}${group}`, style: "secondary" });
  }
  return nodes;
}

/** This session's items: one list, or one per map under its name when items came from two or more. */
function itemsView(session: Session, catalog: Catalog, first: Map<string, number>, expanded: Set<string>): ViewNode[] {
  const maps = (session.maps ?? []).filter((entry) => (entry.i?.length ?? 0) > 0);
  if (maps.length < 2) {
    const items = sessionItems(session);
    return items.length === 0 ? [] : itemList(items, catalog, "session", expanded);
  }
  return maps.flatMap((entry): ViewNode[] => [
    { kind: "keyValue", items: [["Map", mapCell(entry.k, first)]] },
    ...itemList(entry.i ?? [], catalog, `map:${entry.k ?? ""}`, expanded),
  ]);
}

/** A past session's value of what was picked up: its mesos and its items at NPC prices. */
function lootValue(loot: LootFields, catalog: Catalog): number {
  return totalMesos(loot) + itemsValue(sessionItems(loot), catalog);
}

/** Whether a session's loot is at least what's shown: some pickups missed, or items' values not known exactly. */
function lootPartial(loot: LootFields, catalog: Catalog): boolean {
  return loot.missed === true || valueAtLeast(sessionItems(loot), catalog);
}

/**
 * All time: time played, mesos, item value and loot/hr over every session kept and the running
 * one, and the items picked up most. Empty until anything was picked up.
 */
function allTimeView(session: Session, history: Past[], allTime: AllTime, catalog: Catalog, expanded: Set<string>): ViewNode[] {
  const running = session.expGained > 0 || hasLoot(session.held) ? [session] : [];
  // Oldest first, so ties in the item list go to the item picked up first.
  const sessions: (Past | Session)[] = [...[...history].reverse(), ...running];
  // Counted with a map, not a list search: this runs on every redraw, over up to 500 sessions.
  const items = sumItems([allTime.items ?? [], ...sessions.map(sessionItems)]);
  const mesos = sessions.reduce((sum, s) => sum + totalMesos(s), allTime.mesos);
  if (mesos <= 0 && items.length === 0) {
    return [{ kind: "empty", text: "Mesos and items you pick up add up here, over every session." }];
  }
  const partial = sessions.some((s) => s.missed === true);
  const rate = lootRate({ maps: sessions.flatMap((s) => s.maps ?? []) });
  const rows: [string, Cell][] = [
    ["Played", formatDuration(sessions.reduce((sum, s) => sum + s.activeMs, allTime.playedMs))],
    ["Mesos", mesosText(mesos, partial, true)],
  ];
  if (items.length > 0) rows.push(["Item value", mesosText(itemsValue(items, catalog), partial || valueAtLeast(items, catalog), true)]);
  if (rate !== null) rows.push(["Loot/hr", formatCompact(rate)]);
  const children: ViewNode[] = [{ kind: "keyValue", items: rows }];
  if (items.length > 0) children.push(...itemList(items, catalog, "all-time", expanded));
  return children;
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
 * The Loot tab: All time on top, then each past session's loot, laid out like Past sessions: what
 * it picked up (mesos and items at NPC prices) and its loot/hr over the time pickups were read.
 * Sessions from before loot tracking aren't listed.
 */
function lootView(
  session: Session,
  history: Past[],
  allTime: AllTime,
  catalog: Catalog,
  expanded: Set<string>,
  showMore: boolean,
): ViewNode[] {
  const first = firstPlayed(history, session);
  const tracked = history.filter((past) => past.maps !== undefined);
  const rows = tracked.slice(0, showMore ? SHOWN_MORE : SHOWN_HISTORY).map((past): [Cell[], string[]] => {
    const rate = lootRate(past);
    return [
      [pastMap(past, first).cell, mesosText(lootValue(past, catalog), lootPartial(past, catalog), true), rate === null ? null : formatCompact(rate)],
      pastLines(past),
    ];
  });
  const past: ViewNode[] =
    tracked.length === 0
      ? [{ kind: "empty", text: "Sessions show up here when you end them or close MesoVibe." }]
      : [
          {
            kind: "table",
            columns: ["Map", "Loot gained", "Loot/hr"],
            numeric: [false, true, true],
            rows: rows.map(([cells]) => cells),
            details: rows.map(([, lines]) => lines),
          },
          ...(tracked.length > SHOWN_HISTORY
            ? [{ kind: "button", label: showMore ? "Show fewer" : "Show more", action: "toggle-loot-history", style: "secondary" } as const]
            : []),
        ];
  return [
    { kind: "section", title: "All time", children: allTimeView(session, history, allTime, catalog, expanded) },
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
  notices: Notices,
  showMore: boolean,
  catalog: Catalog,
  expanded: Set<string>,
): ViewNode[] {
  const current: ViewNode[] = [];
  const left = toGo(session);
  const reading = capture === "capturing";
  const first = firstPlayed(history, session);
  const mesos = totalMesos(session);
  const partial = session.missed === true;
  const items = sessionItems(session);
  const mapRow: [string, Cell][] = map === undefined ? [] : [["Map", mapCell(map, first)]];
  const lootRows: [string, Cell][] = [
    ...(mesos > 0 ? [["Mesos", mesosText(mesos, partial, false)] as [string, Cell]] : []),
    ...(items.length > 0
      ? [["Item value", mesosText(itemsValue(items, catalog), partial || valueAtLeast(items, catalog), false)] as [string, Cell]]
      : []),
  ];
  if (session.last === null) {
    if (!reading) current.push({ kind: "loading", text: "Waiting for the game. Your session starts with your first EXP gain." });
  } else if (session.expGained <= 0) {
    // At startup and after End or Reset: no list of zeros, and the EXP to go is in the strip.
    if (!reading) current.push({ kind: "empty", text: "Your session numbers appear after your first gain." });
    const rows: [string, Cell][] = [...(lootRows.length > 0 ? mapRow : []), ...lootRows];
    if (rows.length > 0) current.push({ kind: "keyValue", items: rows });
  } else {
    const recent = recentRate(session);
    const rows: [string, Cell][] = [
      ...mapRow,
      ["Last 5 min EXP/hr", recent === null ? null : formatCompact(recent)],
      ["Time played", formatDuration(session.activeMs)],
      ["EXP gained", formatDigits(session.expGained)],
      ...lootRows,
    ];
    // Worked out from the percent alone, it's within a hundredth of a percent, not exact.
    if (left !== null) rows.push(["EXP to go", `${session.last.expShown === false ? "≈" : ""}${formatDigits(left)}`]);
    if (session.levelUps > 0) rows.push(["Level-ups", String(session.levelUps)]);
    if (session.expLost > 0) rows.push(["EXP lost", formatDigits(session.expLost)]);
    current.push({ kind: "keyValue", items: rows });
  }
  if (reading) current.push(...noticeLines(session, notices));

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

  // Loot/hr heads the Items section: mesos and items' NPC value per hour of pickups read. Until
  // there's a minute of them, a line says it's coming. Not before the first kill, when nothing is
  // read toward it, nor after a minute with nothing shops pay for; nor while the notices aren't
  // read at all, which This session's own line explains.
  const shown = lootShown(notices.readers);
  const rate = shown ? lootRate(session) : null;
  const readMs = (session.maps ?? []).reduce((sum, entry) => sum + entry.l, 0);
  const itemNodes = itemsView(session, catalog, first, expanded);
  const head: ViewNode[] =
    rate !== null
      ? [{ kind: "stat", label: "Loot/hr", value: formatCompact(rate), detail: `${formatDigits(rate)} mesos and NPC item value per hour` }]
      : itemNodes.length > 0 && shown && reading && session.expGained > 0 && readMs < MESOS_RATE_MS
        ? [{ kind: "empty", text: "Loot/hr shows after a minute of pickups." }]
        : [];
  const missing = reading ? missingLine(session, notices.readers) : [];
  // With mesos alone, the section is just the rate: the session's loot/hr shows nowhere else here.
  const itemSection = [...head, ...itemNodes];
  if (itemSection.length > 0) itemSection.push(...missing);
  else current.push(...missing);
  return [
    ...(current.length === 0 ? [] : [{ kind: "section", title: "This session", children: current } as const]),
    ...(itemSection.length === 0 ? [] : [{ kind: "section", title: "Items", children: itemSection } as const]),
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
        allTime = {
          mesos: allTime.mesos + totalMesos(dropped),
          playedMs: allTime.playedMs + dropped.activeMs,
          ...(items.length > 0 ? { items } : {}),
        };
      }
    };
    // A session still running when MesoVibe closed ends where it last moved: closing counts as
    // End.
    const leftOver = saved.current === null ? null : finishWithLoot(saved.current, saved.current.lastAtMs);
    if (leftOver !== null) pushHistory(leftOver);

    let session: Session = newSession(startedAtMs, null);
    let capture: CaptureState = startCapture;
    let readers: Readers = startReaders ?? { notices: "off", waitingMesos: 0, hpMp: "off", hpMpMissing: null };
    let learningSince: number | null = readers.notices === "learning" ? startedAtMs : null;
    /** The map the player is on: undefined until the engine says. */
    let map: string | null | undefined;
    let showMore = false;
    /** The Loot tab's Past sessions shows more. */
    let lootMore = false;
    /** The item lists showing all their items, until the session ends. */
    let expanded = new Set<string>();
    let undo: Undo | null = null;
    let savedAt = Number.NEGATIVE_INFINITY;
    // An EXP change the 5 s cap held back; the next tick past the cap saves it.
    let unsaved = false;

    /** Saves when `gapMs` has passed since the last save (0: now). */
    const persist = (atMs: number, gapMs = SAVE_EVERY_MS) => {
      if (atMs - savedAt < gapMs) return;
      const running = session.last === null && session.held === undefined ? null : session;
      context.save({ version: 1, history, current: running, allTime, catalog } satisfies Saved);
      savedAt = atMs;
      unsaved = false;
    };
    const show = (atMs: number) => {
      context.setSummary(summary(session, atMs, readers));
      context.render(view(session, history, capture, map, { readers, learningSince, atMs }, showMore, catalog, expanded));
      context.render(lootView(session, history, allTime, catalog, expanded, lootMore), "loot");
    };
    const remember = (kind: Undo["kind"], atMs: number) => {
      undo = { kind, session: structuredClone(session), history: history.slice(), allTime, until: atMs + UNDO_MS };
    };
    /** Moves the session's clock, giving the time played to the map the player is on. */
    const tick = (atMs: number) => {
      const before = session.activeMs;
      advance(session, atMs, capture === "capturing" && session.paused !== true);
      addTime(session, map, session.activeMs - before, reads(readers.notices), session.last?.level ?? null);
    };
    persist(startedAtMs, leftOver !== null ? 0 : SAVE_EVERY_MS);
    show(startedAtMs);

    return {
      onEvent(event, atMs) {
        tick(atMs);
        if (event.type === "ExpChanged" && session.paused) {
          // Paused, the bar is still followed: the strip's level stays true, and Resume counts only
          // what's gained after it. A level-up still counts, so the session's levels add up.
          const reading = {
            level: event.level,
            exp: event.exp,
            pctHundredths: event.pct_hundredths,
            needed: event.needed,
            expShown: event.exp_shown,
          };
          const change = compare(session.last, reading);
          if (change.kind === "gain" && change.levelUp) session.levelUps += 1;
          session.last = reading;
          show(atMs);
        } else if (event.type === "ExpChanged") {
          const firstGain = session.expGained <= 0;
          const before = session.last;
          const gainedBefore = session.expGained;
          const reading = {
            level: event.level,
            exp: event.exp,
            pctHundredths: event.pct_hundredths,
            needed: event.needed,
            expShown: event.exp_shown,
          };
          observe(session, reading, atMs);
          // A session starts at its first gain: a wait before it, such as after End or Reset,
          // keeps at most LEAD_MS, and its level and losses start from the reading before the gain.
          if (firstGain && session.expGained > 0) {
            const lead = Math.min(session.activeMs, LEAD_MS);
            session.startedAtMs = atMs - lead;
            session.activeMs = lead;
            session.startLevel = before?.level ?? event.level;
            session.expLost = 0;
            startLoot(session, map, lead, reads(readers.notices), event.level, atMs);
          }
          addExp(session, map, session.expGained - gainedBefore, event.level);
          // Saved on EXP changes too, at most every 5 s, so a close loses at most that.
          unsaved = true;
          persist(atMs, EXP_SAVE_EVERY_MS);
          show(atMs);
        } else if (event.type === "MapChanged") {
          map = event.map_key;
          show(atMs);
        } else if (
          // Pickups made while paused aren't the session's.
          !session.paused &&
          (event.type === "MesosGained" || event.type === "PartyBonus" || event.type === "NoticesMissed")
        ) {
          addPickup(session, event, map, session.expGained > 0, reads(readers.notices), atMs, session.last?.level ?? null);
          unsaved = true;
          persist(atMs, EXP_SAVE_EVERY_MS);
          show(atMs);
        } else if (!session.paused && event.type === "ItemGained") {
          addItem(session, catalog, event, map, session.expGained > 0, reads(readers.notices), atMs, session.last?.level ?? null);
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
        if (next.notices === "learning" && readers.notices !== "learning") learningSince = atMs;
        if (next.notices !== "learning") learningSince = null;
        readers = next;
        show(atMs);
      },
      onTick(atMs) {
        tick(atMs);
        if (undo !== null && atMs > undo.until) undo = null;
        persist(atMs, unsaved ? EXP_SAVE_EVERY_MS : SAVE_EVERY_MS);
        show(atMs);
      },
      onAction(action, atMs) {
        tick(atMs);
        if (action === "end") {
          // Saves the session to Past sessions and starts the next one at the next gain.
          const past = finishWithLoot(session, atMs);
          if (past !== null) {
            const said = recap(session);
            remember("end", atMs);
            pushHistory(past);
            session = newSession(atMs, session.last);
            expanded = new Set();
            persist(atMs, 0);
            context.notice(said, "undo-end");
          }
        } else if (action === "reset") {
          // Throws the session's numbers away and starts counting afresh; history is untouched.
          if (session.expGained > 0 || session.held !== undefined) {
            remember("reset", atMs);
            session = newSession(atMs, session.last);
            expanded = new Set();
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
          if (session.paused) {
            delete session.paused;
            persist(atMs, 0);
          }
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
              // Pickups made since End or Reset belong to the session put back.
              const since = session;
              ({ session, history, allTime } = undo);
              mergeLoot(session, since);
              session.lastAtMs = atMs;
            }
            undo = null;
            persist(atMs, 0);
            context.notice("Session restored.");
          } else if (undo === null || atMs > undo.until) {
            // The panel's Undo can outlive the window by a tick; say so rather than do nothing.
            undo = null;
            context.notice(TOO_LATE[action]);
          }
        } else if (action === "toggle-history") {
          showMore = !showMore;
        } else if (action === "toggle-loot-history") {
          lootMore = !lootMore;
        } else if (action.startsWith(TOGGLE_ITEMS)) {
          const group = action.slice(TOGGLE_ITEMS.length);
          if (!expanded.delete(group)) expanded.add(group);
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
