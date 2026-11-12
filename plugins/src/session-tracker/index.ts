/**
 * Session Tracker: EXP/hr for the session and the last 5 minutes, time to
 * level, mesos/hr and the map, and a history of past sessions kept on this computer. It feeds the
 * session strip and draws the Session view. End saves the session to Past sessions and Reset
 * throws it away, each one click with Undo; deleting history asks first.
 */

import { definePlugin, formatCompact, formatDigits, formatDuration } from "@mesovibe/sdk";
import type { CaptureState, Cell, Readers, SessionSummary, ViewNode } from "@mesovibe/sdk";
import {
  addExp,
  addPickup,
  addTime,
  HOLD_MS,
  longest,
  mapRates,
  mergeLoot,
  mesosRate,
  reads,
  start as startLoot,
  totalMesos,
  validMaps,
} from "./loot.js";
import type { LootFields, MapEntry } from "./loot.js";
import {
  advance,
  finish,
  MIN_RATE_MS,
  minutesToLevel,
  newSession,
  observe,
  recentRate,
  sessionRate,
} from "./session.js";
import type { PastSession, SessionState } from "./session.js";

/** How long End and Reset can be undone. */
const UNDO_MS = 10_000;
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

/** A session as kept, running or past: its EXP and, since loot tracking, its loot by map. */
type Session = SessionState & LootFields;
type Past = PastSession & LootFields;

/** What sessions dropped past MAX_HISTORY add to All time, so it never forgets them. */
interface AllTime {
  mesos: number;
  playedMs: number;
}

interface Saved {
  version: 1;
  history: Past[];
  current: Session | null;
  allTime?: AllTime;
}

/** What Undo puts back: the session and history before End or Reset. */
interface Undo {
  kind: "end" | "reset";
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
  return session;
}

function load(saved: unknown): Saved {
  const empty: Saved = { version: 1, history: [], current: null };
  if (typeof saved !== "object" || saved === null) return empty;
  const value = saved as Partial<Saved>;
  if (value.version !== 1) return empty;
  const allTime = value.allTime;
  return {
    version: 1,
    history: Array.isArray(value.history)
      ? value.history.filter((s): s is Past => typeof s === "object" && s !== null).map(validLoot)
      : [],
    current: typeof value.current === "object" && value.current !== null ? validLoot(value.current) : null,
    allTime:
      typeof allTime?.mesos === "number" && typeof allTime.playedMs === "number" ? allTime : { mesos: 0, playedMs: 0 },
  };
}

/** The session is worth saving at End: it gained EXP, or it held loot for five minutes. */
function canEnd(session: Session, atMs: number): boolean {
  if (session.expGained > 0) return true;
  const held = session.held;
  return held !== undefined && held.mesos > 0 && atMs - held.atMs >= HOLD_MS;
}

function summary(session: Session, readers: Readers, atMs: number): SessionSummary {
  const last = session.last;
  // While the letters are learned, or the party list covers the notices, no rate is shown.
  const mesosShown = readers.notices !== "learning" && readers.notices !== "blocked";
  return {
    expPerHour: sessionRate(session),
    recentExpPerHour: recentRate(session),
    expGained: session.expGained,
    level: last?.level ?? null,
    pctHundredths: last?.pctHundredths ?? null,
    minutesToLevel: minutesToLevel(session),
    activeMs: session.activeMs,
    mesosPerHour: mesosShown ? mesosRate(session) : null,
    canEnd: canEnd(session, atMs),
    canReset: session.expGained > 0 || (session.held?.mesos ?? 0) > 0,
    lootOnly: session.expGained <= 0 && (session.held?.mesos ?? 0) > 0,
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
  return `${day.replace(/ /g, " ")} · ${formatDuration(past.activeMs)}`;
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

/** The muted line under This session about reading pickups, if there's one to show. */
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
    lines.push({ kind: "partial", text: "Pickups aren't read in a party yet: the party list covers the notices." });
  }
  if (readers.waitingMesos > 0) {
    lines.push({
      kind: "text",
      variant: "muted",
      text:
        readers.waitingMesos === 1
          ? "1 amount is still being read; it's added once its digits are known."
          : `${readers.waitingMesos} amounts still being read; they're added once their digits are known.`,
    });
  }
  if (session.shapesWrong) {
    lines.push({ kind: "partial", text: "Some mesos amounts may be wrong: a digit was misread and is no longer used." });
  } else if (session.missed && readers.notices !== "blocked") {
    lines.push({ kind: "partial", text: "Some pickups may be missing: MesoVibe couldn't read every notice." });
  }
  return lines;
}

/** A Past sessions row: [cells, detail lines]. */
function pastRow(past: Past, first: Map<string, number>): [Cell[], string[]] {
  const lines = [dayAndTime(past), levels(past)].filter((line): line is string => line !== null);
  const sessionRate = past.activeMs >= MIN_RATE_MS ? formatCompact((past.expGained * 3_600_000) / past.activeMs) : null;
  if (past.maps === undefined) {
    // From before map and mesos tracking: as it read then, with its date in the map's place.
    return [[when(past.startedAtMs), sessionRate, "—"], [formatDuration(past.activeMs), levels(past)].filter((l): l is string => l !== null)];
  }
  const main = longest(past.maps);
  // Tracked, but no map's name was ever read.
  if (main === null) return [[mapCell(null, first), sessionRate, "—"], lines];
  const others = (past.maps?.length ?? 1) - 1;
  const suffix = others === 0 ? undefined : `+${others} map${others === 1 ? "" : "s"}`;
  // The row is named for one map, so its rates are that map's own.
  const rates = mapRates(main, MIN_RATE_MS);
  return [
    [
      mapCell(main.k, first, suffix),
      rates.exp === null ? null : formatCompact(rates.exp),
      rates.mesos === null ? "—" : formatCompact(rates.mesos),
    ],
    lines,
  ];
}

/**
 * The Session view. "This session" holds only what the strip above it doesn't show:
 * no EXP/hr, mesos/hr, level progress or time to level, which the strip already has. Before the
 * first gain while the game is being read, the strip shows the level and its bar, so nothing here
 * waits on the first gain, and with nothing else to show the section is left out.
 */
function view(
  session: Session,
  history: Past[],
  capture: CaptureState,
  map: string | null | undefined,
  notices: Notices,
  showMore: boolean,
): ViewNode[] {
  const current: ViewNode[] = [];
  const left = toGo(session);
  const reading = capture === "capturing";
  const first = firstPlayed(history, session);
  const mesos = totalMesos(session);
  const partial = session.missed === true || notices.readers.waitingMesos > 0;
  const mapRow: [string, Cell][] = map === undefined ? [] : [["Map", mapCell(map, first)]];
  const mesosRow: [string, Cell][] = mesos > 0 ? [["Mesos", mesosText(mesos, partial, false)]] : [];
  if (session.last === null) {
    if (!reading) current.push({ kind: "loading", text: "Waiting for the game. Your session starts with your first EXP gain." });
  } else if (session.expGained <= 0) {
    // At startup and after End or Reset: no list of zeros.
    if (!reading) current.push({ kind: "empty", text: "Your session numbers appear after your first gain." });
    const items: [string, Cell][] = [...mesosRow, ...(mesos > 0 ? mapRow : [])];
    if (left !== null) items.push(["EXP to go", formatDigits(left)]);
    if (items.length > 0) current.push({ kind: "keyValue", items });
  } else {
    const recent = recentRate(session);
    const items: [string, Cell][] = [
      ["Last 5 min", recent === null ? null : `${formatCompact(recent)}/hr`],
      ["Time played", formatDuration(session.activeMs)],
      ["EXP gained", formatDigits(session.expGained)],
      ...mesosRow,
      ...mapRow,
    ];
    if (left !== null) items.push(["EXP to go", formatDigits(left)]);
    if (session.levelUps > 0) items.push(["Level-ups", String(session.levelUps)]);
    if (session.expLost > 0) items.push(["EXP lost", formatDigits(session.expLost)]);
    current.push({ kind: "keyValue", items });
  }
  if (reading) current.push(...noticeLines(session, notices));

  const shown = history.slice(0, showMore ? SHOWN_MORE : SHOWN_HISTORY);
  const tracked = shown.filter((s) => s.maps !== undefined);
  const before = shown.filter((s) => s.maps === undefined);
  const table = (sessions: Past[]): ViewNode => {
    const rows = sessions.map((s) => pastRow(s, first));
    return {
      kind: "table",
      columns: ["Map", "EXP/hr", "Mesos/hr"],
      numeric: [false, true, true],
      rows: rows.map(([cells]) => cells),
      details: rows.map(([, lines]) => lines),
    };
  };
  const more: ViewNode[] =
    history.length > SHOWN_HISTORY
      ? [{ kind: "button", label: showMore ? "Show fewer" : "Show more", action: "toggle-history", style: "secondary" }]
      : [];
  const past: ViewNode[] =
    history.length === 0
      ? [{ kind: "empty", text: "Sessions show up here when you end them or close MesoVibe." }]
      : [
          ...(tracked.length > 0 ? [table(tracked)] : []),
          ...(tracked.length > 0 && before.length > 0
            ? [{ kind: "text", variant: "muted", text: "Before map and mesos tracking" } as const]
            : []),
          ...(before.length > 0 ? [table(before)] : []),
          ...more,
          {
            kind: "button",
            label: "Delete all session history…",
            action: "delete-history",
            style: "danger-link",
            confirm: {
              title: "Delete all session history?",
              deletes: ["Every past session on this computer: EXP gained, time played, rates and mesos, and the All time totals."],
              keeps: ["The session running now, your settings, item and map name pictures, and everything outside the Session Tracker."],
              confirmLabel: "Delete session history",
            },
          },
        ];

  return [
    ...(current.length === 0 ? [] : [{ kind: "section", title: "This session", children: current } as const]),
    { kind: "section", title: "Past sessions", children: past },
  ];
}

definePlugin({
  manifest: { id: "session-tracker", name: "Session Tracker" },
  start(context, startedAtMs, startCapture, startReaders) {
    const saved = load(context.saved);
    let history = saved.history;
    let allTime: AllTime = saved.allTime ?? { mesos: 0, playedMs: 0 };
    /** Adds a session to history; one past MAX_HISTORY goes into All time, never lost. */
    const pushHistory = (past: Past) => {
      history = [past, ...history];
      while (history.length > MAX_HISTORY) {
        const dropped = history.pop() as Past;
        allTime = { mesos: allTime.mesos + totalMesos(dropped), playedMs: allTime.playedMs + dropped.activeMs };
      }
    };
    // A session still running when MesoVibe closed ends where it last moved: closing counts as
    // End.
    const leftOver = saved.current === null ? null : finishWithLoot(saved.current, saved.current.lastAtMs);
    if (leftOver !== null) pushHistory(leftOver);

    let session: Session = newSession(startedAtMs, null);
    let capture: CaptureState = startCapture;
    let readers: Readers = startReaders ?? { notices: "off", waitingMesos: 0 };
    let learningSince: number | null = readers.notices === "learning" ? startedAtMs : null;
    /** The map the player is on: undefined until the engine says. */
    let map: string | null | undefined;
    let showMore = false;
    let undo: Undo | null = null;
    let savedAt = Number.NEGATIVE_INFINITY;
    // An EXP change the 5 s cap held back; the next tick past the cap saves it.
    let unsaved = false;

    /** Saves when `gapMs` has passed since the last save (0: now). */
    const persist = (atMs: number, gapMs = SAVE_EVERY_MS) => {
      if (atMs - savedAt < gapMs) return;
      const running = session.last === null && session.held === undefined ? null : session;
      context.save({ version: 1, history, current: running, allTime } satisfies Saved);
      savedAt = atMs;
      unsaved = false;
    };
    const show = (atMs: number) => {
      context.setSummary(summary(session, readers, atMs));
      context.render(view(session, history, capture, map, { readers, learningSince, atMs }, showMore));
    };
    const remember = (kind: Undo["kind"], atMs: number) => {
      undo = { kind, session: structuredClone(session), history: history.slice(), allTime, until: atMs + UNDO_MS };
    };
    /** Moves the session's clock, giving the time played to the map the player is on. */
    const tick = (atMs: number) => {
      const before = session.activeMs;
      advance(session, atMs, capture === "capturing");
      addTime(session, map, session.activeMs - before, reads(readers.notices), session.last?.level ?? null);
    };
    persist(startedAtMs, leftOver !== null ? 0 : SAVE_EVERY_MS);
    show(startedAtMs);

    return {
      onEvent(event, atMs) {
        tick(atMs);
        if (event.type === "ExpChanged") {
          const firstGain = session.expGained <= 0;
          const before = session.last;
          const gainedBefore = session.expGained;
          observe(session, { level: event.level, exp: event.exp, pctHundredths: event.pct_hundredths, needed: event.needed }, atMs);
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
        } else if (event.type === "MesosGained" || event.type === "PartyBonus" || event.type === "NoticesMissed") {
          addPickup(session, event, map, session.expGained > 0, reads(readers.notices), atMs, session.last?.level ?? null);
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
            persist(atMs, 0);
            context.notice(said, "undo-end");
          }
        } else if (action === "reset") {
          // Throws the session's numbers away and starts counting afresh; history is untouched.
          if (session.expGained > 0 || session.held !== undefined) {
            remember("reset", atMs);
            session = newSession(atMs, session.last);
            persist(atMs, 0);
            context.notice("Session reset.", "undo-reset");
          }
        } else if (action === "undo-end" || action === "undo-reset") {
          if (undo !== null && action === `undo-${undo.kind}` && atMs <= undo.until) {
            // Pickups made since End or Reset belong to the session put back.
            const since = session;
            ({ session, history, allTime } = undo);
            mergeLoot(session, since);
            undo = null;
            session.lastAtMs = atMs;
            persist(atMs, 0);
            context.notice("Session restored.");
          } else if (undo === null || atMs > undo.until) {
            // The panel's Undo can outlive the window by a tick; say so rather than do nothing.
            undo = null;
            context.notice(action === "undo-end" ? "Too late to undo: it's saved in Past sessions." : "Too late to undo the reset.");
          }
        } else if (action === "toggle-history") {
          showMore = !showMore;
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
