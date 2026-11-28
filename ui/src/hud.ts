// The mini-HUD: a small card over a corner of the game, for when the panel has no room beside the
// game window. A status band on top, the level and its progress under it, then six figures from
// the panel's Session Tracker, two to a row: EXP/hr over the session and over the last 5 minutes,
// the session's EXP and the time to level at the 5-minute rate (in the 5-minute rate's column),
// mesos/hr and pot cost/hr. What each part says is worked out in hudview.ts. The host
// places it, keeps it click-through and never lets it take the keyboard (app/src/dock.rs). Every
// word is a text node, never parsed HTML.
import { formatCompact, formatPercent } from "@mesovibe/sdk";

import { darkWatch, fullScreenWatch } from "./dark.js";
import { h, replace } from "./dom.js";
import { boxMinutes, hudView } from "./hudview.js";
import type { HudView } from "./hudview.js";
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

/** One of the four boxes: a muted label over its figure, or a muted dash while it isn't known. */
function box(label: string, figure: string | null): HTMLElement {
  return h(
    "div",
    { class: "hud-box" },
    h("span", { class: "hud-label" }, label),
    h("span", { class: figure === null ? "number hud-figure none" : "number hud-figure" }, figure ?? "—"),
  );
}

/** The pot cost box while no potion is counted: what to do instead of a figure, on two lines. */
function potsHintBox(): HTMLElement {
  return h(
    "div",
    { class: "hud-box" },
    h("span", { class: "hud-label" }, "Pot cost/hr"),
    h("span", { class: "hud-hint" }, h("span", {}, "Add pet pots"), " ", h("span", {}, "to quick slots")),
  );
}

function levelRow(view: HudView): HTMLElement {
  const pct = view.pctHundredths;
  const progress: Record<string, string> = { role: "progressbar", "aria-label": "Level progress", "aria-valuemin": "0", "aria-valuemax": "100" };
  if (pct !== null) progress["aria-valuenow"] = String(Math.round(pct / 100));
  return h(
    "div",
    { class: "hud-level" },
    h("span", { class: view.level === null ? "number hud-lv none" : "number hud-lv" }, view.level === null ? "Lv —" : `Lv ${view.level}`),
    h("div", { class: "progress", attrs: progress }, fill((pct ?? 0) / 10_000)),
    h("span", { class: pct === null ? "number hud-pct none" : "number hud-pct" }, pct === null ? "—" : formatPercent(pct)),
  );
}

function paint(hud: Hud): void {
  const { status } = hud;
  const stale = hud.lastReadAt !== null && (status?.atMs ?? Date.now()) - hud.lastReadAt > STALE_MS;
  const view = hudView({ status, summary: hud.summary, longDark: hud.longDark, fullScreenUnread: hud.fullScreenUnread, stale });
  const band = h(
    "div",
    { class: "hud-band" },
    h("span", { class: `dot ${view.band.tone}`, attrs: { "aria-hidden": "true" } }),
    view.band.text === null ? h("span", { class: "hud-brand" }, "MesoVibe") : h("span", { class: `hud-note ${view.band.tone}` }, view.band.text),
  );
  const compact = (n: number | null) => (n === null ? null : formatCompact(n));
  const grid = h(
    "div",
    { class: "hud-grid" },
    box("EXP/hr", compact(view.session)),
    box("EXP/hr · 5m", compact(view.recent)),
    box("This session", view.gained === null ? null : `+${formatCompact(view.gained)}`),
    box(view.level === null ? "To level up" : `To Lv ${view.level + 1}`, view.minutesToLevel === null ? null : boxMinutes(view.minutesToLevel)),
    box("Mesos/hr", compact(view.loot)),
    view.potsHint ? potsHintBox() : box("Pot cost/hr", compact(view.pots)),
  );
  const middle = view.tip === null ? levelRow(view) : h("p", { class: "hud-tip" }, view.tip);
  const card = h("div", { class: view.dim ? "hud dim" : "hud", attrs: { role: "status", "aria-label": "MesoVibe" } }, band, middle, grid);
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
