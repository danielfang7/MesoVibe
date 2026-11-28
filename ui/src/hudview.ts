// What the mini-HUD card says, worked out from the engine's status and the panel's session summary,
// apart from drawing it (hud.ts) so its rules can be tested on their own. The card has three parts
// that never move: a status band, the level row (or a tip in its place) and four figures. Where a
// part has several things to say, the rules below are in the order they win.
import type { HudSummary, Status } from "./ipc.js";

export type Tone = "live" | "idle" | "warn" | "danger";

/** A word for the card, and what the player can do about it. */
export interface Issue {
  text: string;
  tone: Tone;
  hint?: string;
}

/** A word for when the numbers can't be trusted right now, in the strip's terms. */
export function problem(status: Status | null, longDark: boolean, fullScreenUnread: boolean): Issue | null {
  // No status yet: the engine's first tick is on its way, and nothing is wrong.
  if (status === null) return null;
  if (!status.reading) return { text: "Not reading", tone: "idle" };
  switch (status.state) {
    case "blocked":
      return { text: "Can't capture the game", tone: "danger" };
    case "paused":
    case "off":
    case "no_game":
      return { text: "Paused", tone: "idle" };
    case "capturing":
      break;
  }
  // Kept through ticks that brought no new frame, which say nothing either way (dark.ts).
  if (fullScreenUnread) return { text: "Can't read Full Screen", tone: "warn", hint: "Try windowed or 1366×768" };
  if (longDark) return { text: "Can't see the EXP bar", tone: "warn" };
  switch (status.unreadable) {
    case "learning":
    case "not_calibrated":
      return { text: "Learning your EXP bar…", tone: "live" };
    case "hud_not_found":
    case "no_text":
    case "unknown_glyph":
    case "garbled":
      return { text: "Can't see the EXP bar", tone: "warn" };
    case "unsupported_size":
    case "unsupported_scale":
      return { text: "Can't read this game size", tone: "warn" };
    default:
      return null;
  }
}

export interface HudInput {
  status: Status | null;
  summary: HudSummary | null;
  /** The screen has been dark for long: not a map change (dark.ts). */
  longDark: boolean;
  /** Full Screen with no HUD seen for long: maybe a size MesoVibe can't read (dark.ts). */
  fullScreenUnread: boolean;
  /** No confirmed reading for a while: the numbers dim. */
  stale: boolean;
  /** A clock time as the player reads it, for the sell-by tip. */
  time?: (ms: number) => string;
}

export interface HudView {
  /** The band: `text` null shows MesoVibe's name, beside a dot in `tone`. */
  band: { text: string | null; tone: Tone };
  /** Said in the level row's place. */
  tip: string | null;
  level: number | null;
  pctHundredths: number | null;
  /** EXP per hour over the last 5 minutes; 0 once the session has EXP but none in them. */
  recent: number | null;
  /** EXP per hour over the whole session. */
  session: number | null;
  loot: number | null;
  /** Mesos spent on potions per hour, once some were counted. */
  pots: number | null;
  /** None counted where they could be: ask for the pet's potions in a quick slot. */
  potsHint: boolean;
  /** EXP gained this session, once there's any. */
  gained: number | null;
  /** Minutes to the next level at the 5-minute rate. */
  minutesToLevel: number | null;
  /** Stale or paused: every figure turns muted. */
  dim: boolean;
}

const clock = (ms: number) => new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });

export function hudView({ status, summary, longDark, fullScreenUnread, stale, time = clock }: HudInput): HudView {
  const issue = problem(status, longDark, fullScreenUnread);
  const started = (summary?.expGained ?? 0) > 0 || summary?.expPerHour != null;
  // The band: what's wrong, then a pause, then still learning or waiting for a gain, then the name.
  const band =
    issue !== null && issue.tone !== "live"
      ? { text: issue.text, tone: issue.tone }
      : summary?.paused
        ? { text: summary.autoPaused ? "Paused: no EXP" : "Paused", tone: "idle" as const }
        : issue !== null
          ? { text: issue.text, tone: issue.tone }
          : { text: started ? null : "Earn more EXP to start", tone: "live" as const };
  // The level row gives way to what the player can do: the problem's fix, then selling after End,
  // then opening the inventory for a meso total, since players who never see the panel need these.
  const closeOutBy = summary?.closeOutBy ?? null;
  const tip =
    issue?.hint ??
    (closeOutBy !== null ? `Sell at a shop by ${time(closeOutBy)}` : summary?.askForMesos ? "Open inventory for mesos/hr" : null);
  // The panel may not have shared a summary yet; the engine's own reading has the level meanwhile.
  const reading = status?.exp?.confirmed ? status.exp : null;
  const recent = summary?.recentExpPerHour ?? null;
  return {
    band,
    tip,
    level: summary?.level ?? reading?.level ?? null,
    pctHundredths: summary?.pctHundredths ?? reading?.pctHundredths ?? null,
    // No gain in the last 5 minutes is a rate of 0, not an unknown, once the session has a rate.
    recent: recent ?? (summary?.expPerHour != null ? 0 : null),
    session: summary?.expPerHour ?? null,
    loot: summary?.lootPerHour ?? null,
    pots: summary?.potCostPerHour ?? null,
    potsHint: summary?.potsUncounted === true,
    gained: (summary?.expGained ?? 0) > 0 ? (summary?.expGained ?? null) : null,
    minutesToLevel: summary?.minutesToLevel ?? null,
    dim: stale || summary?.paused === true,
  };
}

/**
 * Time to level in a figure box about 82px wide: the panel's words ("51 min", "15h 13m", "2h"),
 * but "<1 min", and whole hours from 100 hours, so the longest is "99h 59m".
 */
export function boxMinutes(minutes: number): string {
  if (minutes < 1) return "<1 min";
  const whole = Math.round(minutes);
  if (whole < 60) return `${whole} min`;
  const hours = Math.floor(whole / 60);
  const rest = whole % 60;
  return hours >= 100 || rest === 0 ? `${hours}h` : `${hours}h ${rest}m`;
}
