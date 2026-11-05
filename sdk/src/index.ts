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

import type { Event } from "./generated/events.js";

export type { Event } from "./generated/events.js";
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

/** One number with its label, like "EXP/hr 1.84M". `value` null shows "?" (never 0). */
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
      rows: (string | null)[][];
      numeric?: boolean[];
      /** A smaller, muted second line under each row's first cell, by row; null for none. */
      details?: (string | null)[];
    }
  | { kind: "keyValue"; items: [string, string | null][] }
  | { kind: "text"; text: string; variant?: Variant }
  | {
      kind: "button";
      label: string;
      /** Sent back to the plugin as an action when pressed. */
      action: string;
      style?: "primary" | "secondary" | "danger-link";
      confirm?: Confirm;
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
  /** Time played while capturing, in milliseconds. */
  activeMs: number;
}

// MARK: the protocol between host and plugin

/** Host to plugin. */
export type ToPlugin =
  | { type: "start"; saved: unknown; atMs: number; capture: CaptureState }
  | { type: "event"; event: Event; atMs: number }
  | { type: "capture"; state: CaptureState; atMs: number }
  | { type: "action"; action: string; atMs: number }
  | { type: "tick"; atMs: number };

/** Plugin to host. */
export type ToHost =
  | { type: "ready"; manifest: PluginManifest }
  | { type: "render"; view: ViewNode[] }
  | { type: "summary"; summary: SessionSummary | null }
  | { type: "save"; data: unknown }
  | { type: "notice"; text: string; undo?: string };

/** What a running plugin can do. */
export interface PluginContext {
  /** What this plugin saved last time, or null. */
  readonly saved: unknown;
  /** Replaces the plugin's view. */
  render(view: ViewNode[]): void;
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
  onAction?(action: string, atMs: number): void;
  /** Once a second, for clocks and rates. */
  onTick?(atMs: number): void;
}

export interface Plugin {
  manifest: PluginManifest;
  start(context: PluginContext, atMs: number, capture: CaptureState): Handlers;
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
        render: (view) => send({ type: "render", view }),
        save: (saved) => send({ type: "save", data: saved }),
        setSummary: (summary) => send({ type: "summary", summary }),
        notice: (text, undo) => send(undo === undefined ? { type: "notice", text } : { type: "notice", text, undo }),
      };
      handlers = plugin.start(context, data.atMs, data.capture);
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
