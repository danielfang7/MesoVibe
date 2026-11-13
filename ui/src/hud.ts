// The mini-HUD: a small card over a corner of the game, for when the panel has no
// room beside the game window. It shows the session's EXP/hr, time to level and level progress
// from the panel's Session Tracker, and a word when the engine can't read. The host places it,
// keeps it click-through and never lets it take the keyboard (app/src/dock.rs). Every word is a
// text node, never parsed HTML.
import { formatCompact, formatDigits, formatMinutes, formatPercent } from "@mesovibe/sdk";

import { darkWatch, fullScreenWatch } from "./dark.js";
import { h, replace } from "./dom.js";
import { connectHud } from "./ipc.js";
import type { HudSummary, Status } from "./ipc.js";
import { fill } from "./plugins.js";

/** After this long without a reading, the numbers dim, as in the session strip. */
const STALE_MS = 15_000;

const root = document.getElementById("hud") as HTMLElement;

interface Hud {
  status: Status | null;
  summary: HudSummary | null;
  lastReadAt: number | null;
  /** The screen has been dark for long: not a map change (dark.ts). */
  longDark: boolean;
  /** Full Screen with no HUD seen for long: maybe a size MesoVibe can't read (dark.ts). */
  fullScreenUnread: boolean;
}

/** A word for the card, and what the player can do about it. */
interface Issue {
  text: string;
  tone: "live" | "idle" | "warn" | "danger";
  hint?: string;
}

/** A word for when the numbers can't be trusted right now, in the strip's terms. */
function problem(status: Status | null, longDark: boolean, fullScreenUnread: boolean): Issue | null {
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

function paint(hud: Hud): void {
  const { status, summary } = hud;
  const issue = problem(status, hud.longDark, hud.fullScreenUnread);
  const stale = hud.lastReadAt !== null && (status?.atMs ?? Date.now()) - hud.lastReadAt > STALE_MS;
  const rate = summary?.expPerHour ?? null;
  const rows: Node[] = [];
  rows.push(
    h(
      "div",
      { class: "hud-top" },
      h("span", { class: `dot ${issue?.tone ?? "live"}`, attrs: { "aria-hidden": "true" } }),
      rate === null
        ? h("span", { class: "hud-note" }, issue?.text ?? "Earn more EXP to start")
        : h(
            "span",
            { class: "hud-rate-box", attrs: { title: `${formatDigits(rate)} EXP per hour` } },
            h("span", { class: "number hud-rate" }, formatCompact(rate)),
            h("span", { class: "hud-unit" }, "EXP/hr"),
          ),
    ),
  );
  if (rate !== null && issue !== null && issue.tone !== "live") {
    rows.push(h("p", { class: "hud-line hud-issue" }, issue.text));
  } else if (summary?.level != null) {
    const minutes = summary.minutesToLevel;
    rows.push(
      h(
        "p",
        { class: "hud-line" },
        minutes === null ? `Lv ${summary.level}` : `Lv ${summary.level} → ${summary.level + 1} in ${formatMinutes(minutes)}`,
      ),
    );
  }
  // A hint takes the progress row: the card has room for three rows.
  if (issue?.hint !== undefined) {
    rows.push(h("p", { class: "hud-line" }, issue.hint));
  } else if (summary?.pctHundredths != null) {
    rows.push(
      h(
        "div",
        { class: "hud-progress" },
        h(
          "div",
          {
            class: "progress",
            attrs: {
              role: "progressbar",
              "aria-label": "Level progress",
              "aria-valuemin": "0",
              "aria-valuemax": "100",
              "aria-valuenow": String(Math.round(summary.pctHundredths / 100)),
            },
          },
          fill(summary.pctHundredths / 10_000),
        ),
        h("span", { class: "number" }, formatPercent(summary.pctHundredths)),
      ),
    );
  }
  const card = h("div", { class: stale ? "hud dim" : "hud", attrs: { role: "status", "aria-label": "MesoVibe" } }, ...rows);
  replace(root, h("div", { class: "hud-frame" }, card));
}

async function boot(): Promise<void> {
  const host = await connectHud();
  const hud: Hud = { status: null, summary: null, lastReadAt: null, longDark: false, fullScreenUnread: false };
  const dark = darkWatch();
  const fullScreen = fullScreenWatch();
  paint(hud);
  void host.onStatus((status) => {
    hud.status = status;
    hud.longDark = dark(status);
    hud.fullScreenUnread = fullScreen(status);
    if (status.exp?.confirmed) hud.lastReadAt = status.atMs;
    paint(hud);
  });
  void host.onSummary((summary) => {
    hud.summary = summary;
    paint(hud);
  });
  hud.summary = await host.latestSummary();
  paint(hud);
}

void boot();
