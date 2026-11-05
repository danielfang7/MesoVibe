/**
 * The Session Tracker's arithmetic, kept free of the host so it can be tested on its own.
 *
 * A session takes its baseline from the first confirmed EXP reading and counts time only while the
 * engine is capturing; the plugin restarts its clock at the first gain, keeping at most a minute of
 * the wait before it. EXP/hr counts gains only, so these edge cases never make it negative or
 * garbage:
 * - a level-up counts what was left of the old level plus the new level's EXP;
 * - a death (EXP falls with no level change) is recorded as EXP lost, never as a negative gain;
 * - any other jump (another character, a restarted game, a skipped reading across levels) starts
 *   a new baseline instead of counting as a gain.
 */

/** A confirmed EXP reading, as the engine reports it. */
export interface Reading {
  level: number | null;
  exp: number;
  pctHundredths: number;
  needed: number | null;
}

/** The most a tick may add to time played: longer gaps are the computer sleeping, not play. */
export const MAX_TICK_MS = 5_000;
/** The window for the recent rate, which time to level uses. */
export const RECENT_MS = 5 * 60_000;
/** The shortest session time a rate is shown for, so the first gain isn't a silly number. */
export const MIN_RATE_MS = 10_000;

export interface SessionState {
  startedAtMs: number;
  /** Time played while capturing. */
  activeMs: number;
  expGained: number;
  expLost: number;
  levelUps: number;
  startLevel: number | null;
  /** The last confirmed reading. */
  last: Reading | null;
  /** Gains in the recent window: [time, amount]. */
  recent: [number, number][];
  /** The last time the session's clock moved. */
  lastAtMs: number;
}

/** A finished session, as kept in history. */
export interface PastSession {
  startedAtMs: number;
  endedAtMs: number;
  activeMs: number;
  expGained: number;
  expLost: number;
  startLevel: number | null;
  endLevel: number | null;
}

export function newSession(atMs: number, baseline: Reading | null): SessionState {
  return {
    startedAtMs: atMs,
    activeMs: 0,
    expGained: 0,
    expLost: 0,
    levelUps: 0,
    startLevel: baseline?.level ?? null,
    last: baseline,
    recent: [],
    lastAtMs: atMs,
  };
}

/** What a new reading means after the last one. */
export type Change =
  | { kind: "baseline" }
  | { kind: "same" }
  | { kind: "gain"; amount: number; levelUp: boolean }
  | { kind: "loss"; amount: number };

export function compare(last: Reading | null, next: Reading): Change {
  if (last === null) return { kind: "baseline" };
  if (next.level !== null && last.level !== null && next.level !== last.level) {
    // Exactly one level up, with the old level's size known: the rollover is exact.
    if (next.level === last.level + 1 && last.needed !== null && last.needed >= last.exp) {
      return { kind: "gain", amount: last.needed - last.exp + next.exp, levelUp: true };
    }
    return { kind: "baseline" };
  }
  if (next.exp > last.exp) return { kind: "gain", amount: next.exp - last.exp, levelUp: false };
  if (next.exp < last.exp) {
    // A fall within the same named level is a death; anything else starts a new baseline.
    if (next.level !== null && next.level === last.level) {
      return { kind: "loss", amount: last.exp - next.exp };
    }
    return { kind: "baseline" };
  }
  return { kind: "same" };
}

/** Moves the session's clock to `atMs`; play time grows only while capturing. */
export function advance(session: SessionState, atMs: number, capturing: boolean): void {
  const step = Math.max(0, atMs - session.lastAtMs);
  if (capturing && session.last !== null) session.activeMs += Math.min(step, MAX_TICK_MS);
  session.lastAtMs = Math.max(session.lastAtMs, atMs);
  session.recent = session.recent.filter(([at]) => at > atMs - RECENT_MS);
}

/** Applies a confirmed reading. Returns what it meant. */
export function observe(session: SessionState, reading: Reading, atMs: number): Change {
  const change = compare(session.last, reading);
  switch (change.kind) {
    case "gain":
      session.expGained += change.amount;
      session.recent.push([atMs, change.amount]);
      if (change.levelUp) session.levelUps += 1;
      break;
    case "loss":
      session.expLost += change.amount;
      break;
    case "baseline":
      if (session.last === null) session.startLevel = reading.level;
      break;
    case "same":
      break;
  }
  session.last = reading;
  return change;
}

/** EXP per hour over the whole session, or null before a rate means anything. */
export function sessionRate(session: SessionState): number | null {
  if (session.expGained <= 0 || session.activeMs < MIN_RATE_MS) return null;
  return (session.expGained * 3_600_000) / session.activeMs;
}

/** EXP per hour over the last five minutes of play. */
export function recentRate(session: SessionState): number | null {
  const window = Math.min(RECENT_MS, session.activeMs);
  if (window < MIN_RATE_MS) return null;
  const gained = session.recent.reduce((sum, [, amount]) => sum + amount, 0);
  if (gained <= 0) return null;
  return (gained * 3_600_000) / window;
}

/**
 * Minutes to the next level at the session rate: the same EXP/hr the strip shows above it, so the
 * two always agree. The last-5-minute rate is shown on its own.
 */
export function minutesToLevel(session: SessionState): number | null {
  const last = session.last;
  if (last === null || last.needed === null || last.needed < last.exp) return null;
  const rate = sessionRate(session);
  if (rate === null || rate <= 0) return null;
  return ((last.needed - last.exp) / rate) * 60;
}

/** The session as history keeps it, when it's worth keeping. */
export function finish(session: SessionState): PastSession | null {
  if (session.expGained <= 0) return null;
  const endLevel = session.last?.level ?? null;
  // The level range holds only when the counted level-ups explain it. Another character or a skipped
  // reading starts a new baseline mid-session, so keep just the level it ended on.
  const counted = session.startLevel !== null && endLevel !== null && endLevel - session.startLevel === session.levelUps;
  return {
    startedAtMs: session.startedAtMs,
    endedAtMs: session.lastAtMs,
    activeMs: session.activeMs,
    expGained: session.expGained,
    expLost: session.expLost,
    // With the end level unknown, the start level is still the best line there is.
    startLevel: counted ? session.startLevel : (endLevel ?? session.startLevel),
    endLevel,
  };
}
