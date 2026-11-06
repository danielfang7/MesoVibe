/**
 * Session Tracker: EXP/hr for the session and the last 5 minutes, time to
 * level, and a history of past sessions kept on this computer. It feeds the session strip
 * and draws the Session view. End saves the session to Past sessions and Reset throws it away,
 * each one click with Undo; deleting history asks first.
 */

import { definePlugin, formatCompact, formatDigits, formatDuration } from "@mesovibe/sdk";
import type { CaptureState, SessionSummary, ViewNode } from "@mesovibe/sdk";
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
/** Past sessions shown in the table. */
const SHOWN_HISTORY = 50;

interface Saved {
  version: 1;
  history: PastSession[];
  current: SessionState | null;
}

/** What Undo puts back: the session and history before End or Reset. */
interface Undo {
  kind: "end" | "reset";
  session: SessionState;
  history: PastSession[];
  until: number;
}

function load(saved: unknown): Saved {
  const empty: Saved = { version: 1, history: [], current: null };
  if (typeof saved !== "object" || saved === null) return empty;
  const value = saved as Partial<Saved>;
  if (value.version !== 1) return empty;
  return {
    version: 1,
    history: Array.isArray(value.history) ? value.history : [],
    current: value.current ?? null,
  };
}

function summary(session: SessionState): SessionSummary {
  const last = session.last;
  return {
    expPerHour: sessionRate(session),
    recentExpPerHour: recentRate(session),
    expGained: session.expGained,
    level: last?.level ?? null,
    pctHundredths: last?.pctHundredths ?? null,
    minutesToLevel: minutesToLevel(session),
    activeMs: session.activeMs,
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

/** What End says about the session it saved: "Saved: 41,667 EXP in 30m (81.3K/hr)." */
function recap(session: SessionState): string {
  const rate = sessionRate(session);
  const pace = rate === null ? "" : ` (${formatCompact(rate)}/hr)`;
  return `Saved: ${formatDigits(session.expGained)} EXP in ${formatDuration(session.activeMs)}${pace}.`;
}

/** EXP left to the next level, when the level's size is known. */
function toGo(session: SessionState): number | null {
  const last = session.last;
  if (last === null || last.needed === null || last.needed < last.exp) return null;
  return last.needed - last.exp;
}

/**
 * The Session view. "This session" holds only what the strip above it doesn't show:
 * no EXP/hr, level progress or time to level, which the strip already has. Before the first gain
 * while the game is being read, the strip says "Earn some EXP to start", so nothing here repeats
 * it, and with nothing else to show the section is left out.
 */
function view(session: SessionState, history: PastSession[], capture: CaptureState): ViewNode[] {
  const current: ViewNode[] = [];
  const left = toGo(session);
  const reading = capture === "capturing";
  if (session.last === null) {
    if (!reading) current.push({ kind: "loading", text: "Waiting for the game. Your session starts with your first EXP gain." });
  } else if (session.expGained <= 0) {
    // At startup and after End or Reset: no list of zeros.
    if (!reading) current.push({ kind: "empty", text: "Your session numbers appear after your first gain." });
    if (left !== null) current.push({ kind: "keyValue", items: [["EXP to go", formatDigits(left)]] });
  } else {
    const recent = recentRate(session);
    const items: [string, string | null][] = [
      ["Last 5 min", recent === null ? null : `${formatCompact(recent)}/hr`],
      ["Time played", formatDuration(session.activeMs)],
      ["EXP gained", formatDigits(session.expGained)],
    ];
    if (left !== null) items.push(["EXP to go", formatDigits(left)]);
    if (session.levelUps > 0) items.push(["Level-ups", String(session.levelUps)]);
    if (session.expLost > 0) items.push(["EXP lost", formatDigits(session.expLost)]);
    current.push({ kind: "keyValue", items });
  }

  const shown = history.slice(0, SHOWN_HISTORY);
  const past: ViewNode[] =
    history.length === 0
      ? [{ kind: "empty", text: "Sessions show up here when you end them or close MesoVibe." }]
      : [
          {
            kind: "table",
            columns: ["Started", "Played", "Gained", "EXP/hr"],
            numeric: [false, true, true, true],
            rows: shown.map((s) => [
              when(s.startedAtMs),
              formatDuration(s.activeMs),
              formatCompact(s.expGained),
              // Like the strip, no rate for a session too short for one, so a big first gain ended
              // seconds later can't save an absurd EXP/hr.
              s.activeMs >= MIN_RATE_MS ? formatCompact((s.expGained * 3_600_000) / s.activeMs) : null,
            ]),
            details: shown.map(levels),
          },
          {
            kind: "button",
            label: "Delete all session history…",
            action: "delete-history",
            style: "danger-link",
            confirm: {
              title: "Delete all session history?",
              deletes: ["Every past session on this computer: EXP gained, time played and rates."],
              keeps: ["The session running now, your settings, and everything outside the Session Tracker."],
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
  start(context, startedAtMs, startCapture) {
    const saved = load(context.saved);
    let history = saved.history;
    // A session still running when MesoVibe closed ends where it last moved: closing counts as
    // End.
    const leftOver = saved.current === null ? null : finish(saved.current);
    if (leftOver !== null) history = [leftOver, ...history].slice(0, MAX_HISTORY);

    let session = newSession(startedAtMs, null);
    let capture: CaptureState = startCapture;
    let undo: Undo | null = null;
    let savedAt = Number.NEGATIVE_INFINITY;
    // An EXP change the 5 s cap held back; the next tick past the cap saves it.
    let unsaved = false;

    /** Saves when `gapMs` has passed since the last save (0: now). */
    const persist = (atMs: number, gapMs = SAVE_EVERY_MS) => {
      if (atMs - savedAt < gapMs) return;
      context.save({ version: 1, history, current: session.last === null ? null : session } satisfies Saved);
      savedAt = atMs;
      unsaved = false;
    };
    const show = () => {
      context.setSummary(summary(session));
      context.render(view(session, history, capture));
    };
    const remember = (kind: Undo["kind"], atMs: number) => {
      undo = { kind, session: structuredClone(session), history: history.slice(), until: atMs + UNDO_MS };
    };
    persist(startedAtMs, leftOver !== null ? 0 : SAVE_EVERY_MS);
    show();

    return {
      onEvent(event, atMs) {
        advance(session, atMs, capture === "capturing");
        if (event.type === "ExpChanged") {
          const firstGain = session.expGained <= 0;
          const before = session.last;
          observe(session, { level: event.level, exp: event.exp, pctHundredths: event.pct_hundredths, needed: event.needed }, atMs);
          // A session starts at its first gain: a wait before it, such as after End or Reset,
          // keeps at most LEAD_MS, and its level and losses start from the reading before the gain.
          if (firstGain && session.expGained > 0) {
            const lead = Math.min(session.activeMs, LEAD_MS);
            session.startedAtMs = atMs - lead;
            session.activeMs = lead;
            session.startLevel = before?.level ?? event.level;
            session.expLost = 0;
          }
          // Saved on EXP changes too, at most every 5 s, so a close loses at most that.
          unsaved = true;
          persist(atMs, EXP_SAVE_EVERY_MS);
          show();
        }
      },
      onCapture(state, atMs) {
        advance(session, atMs, capture === "capturing");
        capture = state;
        show();
      },
      onTick(atMs) {
        advance(session, atMs, capture === "capturing");
        if (undo !== null && atMs > undo.until) undo = null;
        persist(atMs, unsaved ? EXP_SAVE_EVERY_MS : SAVE_EVERY_MS);
        show();
      },
      onAction(action, atMs) {
        advance(session, atMs, capture === "capturing");
        if (action === "end") {
          // Saves the session to Past sessions and starts the next one at the next gain.
          const past = finish(session);
          if (past !== null) {
            const said = recap(session);
            remember("end", atMs);
            history = [past, ...history].slice(0, MAX_HISTORY);
            session = newSession(atMs, session.last);
            persist(atMs, 0);
            context.notice(said, "undo-end");
          }
        } else if (action === "reset") {
          // Throws the session's numbers away and starts counting afresh; history is untouched.
          if (session.expGained > 0) {
            remember("reset", atMs);
            session = newSession(atMs, session.last);
            persist(atMs, 0);
            context.notice("Session reset.", "undo-reset");
          }
        } else if (action === "undo-end" || action === "undo-reset") {
          if (undo !== null && action === `undo-${undo.kind}` && atMs <= undo.until) {
            ({ session, history } = undo);
            undo = null;
            session.lastAtMs = atMs;
            persist(atMs, 0);
            context.notice("Session restored.");
          } else if (undo === null || atMs > undo.until) {
            // The panel's Undo can outlive the window by a tick; say so rather than do nothing.
            undo = null;
            context.notice(action === "undo-end" ? "Too late to undo: it's saved in Past sessions." : "Too late to undo the reset.");
          }
        } else if (action === "delete-history") {
          history = [];
          undo = null;
          persist(atMs, 0);
          context.notice("Session history deleted.");
        }
        show();
      },
    };
  },
});
