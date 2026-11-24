/**
 * MesoVibe plugin SDK.
 *
 * Plugins run in a Web Worker with no DOM, no Tauri IPC and no network. They get
 * typed events from the engine, describe their panels with host-rendered components, and pick
 * semantic variants, never colors. Event types are generated from the engine's Rust schema,
 * in ./generated.
 *
 * A plugin is one module that calls {@link definePlugin}. The host and the plugin talk only
 * through the messages in {@link ToPlugin} and {@link ToHost}, so the same plugin runs unchanged
 * if plugins later run isolated in a Web Worker.
 */

import type { Event, HpMpState, NoticeState } from "./generated/events.js";

export type { Event, HpMpState, NoticeState } from "./generated/events.js";
export { SCHEMA_VERSION } from "./generated/events.js";
export { formatCompact, formatDigits, formatDuration, formatMinutes, formatPercent } from "./format.js";

/** Semantic variants; the host maps them to its color tokens and owns contrast. */
export type Variant = "default" | "accent" | "warn" | "danger" | "muted";

/** What a plugin declares about itself. */
export interface PluginManifest {
  /** Stable id, kebab-case, matching the plugin's folder name. */
  id: string;
  /** Name shown in the rail tooltip and settings. */
  name: string;
}

/** Where capture is, as the host reports it. Events arrive only while `capturing`. */
export type CaptureState = "off" | "no_game" | "capturing" | "paused" | "blocked";

// MARK: components. The host renders them as text, never HTML.

/**
 * A name shown as a picture of the game's own lettering: an item's or map's `item_key` or
 * `map_key` from an event. The host draws the picture it keeps for that key, in the text's color.
 * `label` is what screen readers hear; `suffix` is muted text after the picture, like "+2 maps".
 *
 * @experimental May change before the Loot Tracker's first release.
 * @example
 * { kind: "keyValue", items: [["Map", { gameText: event.map_key, label: "Map first played Nov 10, 9:42 PM" }]] }
 */
export interface GameTextCell {
  gameText: string;
  label?: string;
  suffix?: string;
}

/** What a table cell or key-value value holds: text, nothing yet ("?"), or a name picture. */
export type Cell = string | null | GameTextCell;

/**
 * One number with its label, like "EXP/hr 1.84M". `value` null shows "?" (never 0). On its own it's
 * a section's headline figure, drawn large with its label after it; in a `statRow`, a smaller one
 * under its label.
 */
export interface StatNode {
  kind: "stat";
  label: string;
  value: string | null;
  /** A smaller line under the value, like a full-digit count. */
  detail?: string;
  variant?: Variant;
}

/** Asks the player before an irreversible action: what goes, what stays. */
export interface Confirm {
  title: string;
  deletes: string[];
  keeps: string[];
  /** The confirm button's label, naming the action ("Delete session history"), never "OK". */
  confirmLabel: string;
}

/**
 * An icon button at the right end of a table row, like Past sessions' delete. Its label names the
 * row ("Delete the session from Nov 10, 5:15 PM"): screen readers read it and the pointer shows it.
 */
export interface RowAction {
  icon: "delete";
  label: string;
  /** Sent back to the plugin as an action when pressed. */
  action: string;
}

export type ViewNode =
  | { kind: "section"; title: string; children: ViewNode[] }
  | { kind: "row"; children: ViewNode[] }
  | { kind: "divider" }
  | StatNode
  | { kind: "statRow"; stats: StatNode[] }
  | { kind: "progress"; label: string; fraction: number | null; text: string }
  | {
      kind: "table";
      columns: string[];
      rows: Cell[][];
      numeric?: boolean[];
      /**
       * Smaller, muted lines under each row's first cell, by row: one line, several, or null for
       * none.
       */
      details?: (string | string[] | null)[];
      /** An icon button at the end of each row, by row, or null for none. */
      rowActions?: (RowAction | null)[];
    }
  | { kind: "keyValue"; items: [string, Cell][] }
  | { kind: "text"; text: string; variant?: Variant }
  | {
      kind: "button";
      label: string;
      /** Sent back to the plugin as an action when pressed. */
      action: string;
      style?: "primary" | "secondary" | "danger-link";
      confirm?: Confirm;
      /**
       * What screen readers hear when the label alone doesn't say what the button acts on: a
       * "Count it" under a line about a held rise is "Count the 5M rise in Profit".
       */
      spoken?: string;
    }
  | { kind: "loading"; text: string }
  | { kind: "empty"; text: string }
  | { kind: "error"; text: string }
  | { kind: "partial"; text: string };

/** What the Session Tracker shows in the always-visible session strip. */
export interface SessionSummary {
  /** Session EXP per hour; null until the first gain. */
  expPerHour: number | null;
  /** The last 5 minutes' rate; null until the first gain. */
  recentExpPerHour: number | null;
  expGained: number;
  level: number | null;
  /** Percent through the level, in hundredths (6420 = 64.20%). */
  pctHundredths: number | null;
  /** Minutes to the next level at the session rate, the same rate as `expPerHour`; null when unknown. */
  minutesToLevel: number | null;
  /** EXP left to the next level; null when the level's size isn't known. */
  expToGo: number | null;
  /**
   * `expToGo` was worked out from the percent, which the HUD shows alone late in a level, so it
   * can be off by up to a hundredth of a percent's worth: show it as approximate.
   */
  expToGoApprox: boolean;
  /** Time played while capturing, in milliseconds. */
  activeMs: number;
  /**
   * The Session view's Loot headline: profit per hour, mesos picked up plus what shops paid. Exact
   * once two meso totals were read in the inventory or a shop, an estimate from the pickup notices
   * before that (`lootApprox`); null until loot is counted, and while the notices are still being
   * learned or are blocked.
   *
   * @experimental May change before the Loot Tracker's item list is released.
   */
  lootPerHour: number | null;
  /** `lootPerHour` is an estimate from the pickup notices, which miss some: show it with "≈". */
  lootApprox: boolean;
  /** The session has EXP, or loot without EXP, worth keeping: End is offered. */
  canEnd: boolean;
  /** The session has something to throw away: Reset is offered. */
  canReset: boolean;
  /** The session counted loot but no EXP yet: the strip says "Pickups only so far". */
  lootOnly: boolean;
  /** The player paused the session: no time, EXP or loot counts until Resume. */
  paused: boolean;
  /** Pause is offered (Resume, while paused): the session has started. */
  canPause: boolean;
  /**
   * The session just ended waits until then (epoch milliseconds) for a meso total that adds what
   * was sold after End: the strip asks for the inventory by that time. Null when nothing waits.
   *
   * @experimental May change before the meso total is read.
   */
  closeOutBy: number | null;
  /**
   * No meso total has ever been read though MesoVibe can read one: the mini-HUD asks for the
   * inventory once.
   *
   * @experimental May change before the meso total is read.
   */
  askForMesos: boolean;
}

/**
 * The HP and MP numbers next to the EXP bar, as the engine read them: sent when any changed, and
 * with `continuous` false when something may have happened unseen since the last reading (see
 * {@link Event}). A plugin that starts mid-session didn't see the reading before its first one, so
 * it takes that first one as its starting point whatever `continuous` says. MesoVibe only reads
 * these numbers from the screen: a plugin can't drink a potion or send any input to the game (see
 * Fair play in README.md).
 *
 * @experimental May change when potions are counted from the quick slots.
 * @example
 * onEvent(event) {
 *   if (event.type === "HpMpChanged") {
 *     context.render([{ kind: "stat", label: "HP", value: `${event.hp}/${event.max_hp}` }]);
 *   }
 * }
 */
export type HpMpChanged = Extract<Event, { type: "HpMpChanged" }>;

/**
 * How the engine is reading the pickup notices and the HP and MP numbers. `waitingMesos` is how many
 * mesos amounts wait for a digit to be learned before they're counted. While `hpMp` is
 * `missing_digits`, `hpMpMissing` lists the digits the EXP field hasn't shown yet, such as "7" or
 * "045": the one the HP and MP numbers wait for is among them. It is null otherwise. `mesos` says
 * how the meso total in the inventory and shop windows is read; it's absent from a host that
 * doesn't read it.
 *
 * @experimental May change before the Loot Tracker's first release.
 */
export interface Readers {
  notices: NoticeState;
  waitingMesos: number;
  hpMp: HpMpState;
  hpMpMissing: string | null;
  mesos?: MesoState;
}

/**
 * How the meso total is read: `reading` (it's read whenever the inventory or a shop is open long
 * enough), `learning` (its digits are still being learned; keep the inventory open a few seconds),
 * or `unsupported` (it can't be read at this game size).
 *
 * @experimental May change before the meso total is read.
 */
export type MesoState = "reading" | "learning" | "unsupported";

/**
 * A page a plugin draws besides its main view, in a rail tab of its own. First-party Session
 * Tracker only, for now: `loot` is the Loot tab.
 *
 * @experimental May change before the Loot Tracker's first release.
 */
export type PluginPage = "loot";

// MARK: the protocol between host and plugin

/** Host to plugin. */
export type ToPlugin =
  | { type: "start"; saved: unknown; atMs: number; capture: CaptureState; readers: Readers }
  | { type: "event"; event: Event; atMs: number }
  | { type: "capture"; state: CaptureState; atMs: number }
  | { type: "readers"; readers: Readers; atMs: number }
  | { type: "action"; action: string; atMs: number }
  | { type: "tick"; atMs: number };

/** Plugin to host. */
export type ToHost =
  | { type: "ready"; manifest: PluginManifest }
  | { type: "render"; view: ViewNode[]; page?: PluginPage }
  | { type: "summary"; summary: SessionSummary | null }
  | { type: "save"; data: unknown }
  | { type: "notice"; text: string; undo?: string };

/** What a running plugin can do. */
export interface PluginContext {
  /** What this plugin saved last time, or null. */
  readonly saved: unknown;
  /** Replaces the plugin's view, or with `page`, that page's ({@link PluginPage}). */
  render(view: ViewNode[], page?: PluginPage): void;
  /** Keeps data on this computer, replacing what was kept before. */
  save(data: unknown): void;
  /** Feeds the session strip. First-party Session Tracker only, for now. */
  setSummary(summary: SessionSummary | null): void;
  /** A short message in the strip, with an optional Undo that sends `undo` back as an action. */
  notice(text: string, undo?: string): void;
}

/** What a plugin reacts to. Times are the host's clock, in milliseconds since the epoch. */
export interface Handlers {
  onEvent?(event: Event, atMs: number): void;
  onCapture?(state: CaptureState, atMs: number): void;
  /** How the pickup notices or the HP and MP numbers are read changed. @experimental */
  onReaders?(readers: Readers, atMs: number): void;
  onAction?(action: string, atMs: number): void;
  /** Once a second, for clocks and rates. */
  onTick?(atMs: number): void;
}

export interface Plugin {
  manifest: PluginManifest;
  start(context: PluginContext, atMs: number, capture: CaptureState, readers: Readers): Handlers;
}

/** The parts of a Worker's global scope the SDK uses. */
interface WorkerScope {
  postMessage(message: ToHost): void;
  onmessage: ((message: { data: ToPlugin }) => void) | null;
}

/**
 * Runs `plugin` in this Worker: answers the host's messages and sends the plugin's.
 * Call it once, at the top level of the plugin's module.
 */
export function definePlugin(plugin: Plugin): void {
  const scope = globalThis as unknown as WorkerScope;
  const send = (message: ToHost) => scope.postMessage(message);
  let handlers: Handlers | null = null;
  scope.onmessage = ({ data }) => {
    if (data.type === "start") {
      const context: PluginContext = {
        saved: data.saved ?? null,
        render: (view, page) => send(page === undefined ? { type: "render", view } : { type: "render", view, page }),
        save: (saved) => send({ type: "save", data: saved }),
        setSummary: (summary) => send({ type: "summary", summary }),
        notice: (text, undo) => send(undo === undefined ? { type: "notice", text } : { type: "notice", text, undo }),
      };
      handlers = plugin.start(context, data.atMs, data.capture, data.readers);
      return;
    }
    if (handlers === null) return;
    switch (data.type) {
      case "event":
        handlers.onEvent?.(data.event, data.atMs);
        break;
      case "capture":
        handlers.onCapture?.(data.state, data.atMs);
        break;
      case "readers":
        handlers.onReaders?.(data.readers, data.atMs);
        break;
      case "action":
        handlers.onAction?.(data.action, data.atMs);
        break;
      case "tick":
        handlers.onTick?.(data.atMs);
        break;
    }
  };
  send({ type: "ready", manifest: plugin.manifest });
}
