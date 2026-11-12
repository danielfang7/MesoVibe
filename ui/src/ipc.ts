// The panel's, the mini-HUD's and the chat overlay's only ways to talk to the host: the
// allow-listed commands and events each window may use (app/capabilities/). In a development browser
// session with `?mock=<scenario>` in the URL, a scripted stand-in plays the host instead
// (./mock.ts), so every state can be looked at without the game. Release builds leave it out.
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Event as GameEvent, NoticeState, SessionSummary } from "@mesovibe/sdk";

/** A corner of the game area, for the mini-HUD (app/src/dock.rs). */
export type Corner = "top-left" | "top-right" | "bottom-left" | "bottom-right";

/** The chat types the Chat Hub splits out (engine/src/recognizers/chat.rs). */
export type ChatChannel = "whisper" | "party" | "friend";

export interface ChatChannels {
  whisper: boolean;
  party: boolean;
  friend: boolean;
}

/** A text color for each chat type over the game, as `#rrggbb` (app/src/store.rs). */
export interface ChatColors {
  whisper: string;
  party: string;
  friend: string;
}

/** Where the chat overlay sits: on the game's chat box, or in a box at the game's right or left edge (app/src/dock.rs). */
export type ChatPosition = "above-chat" | "right" | "left";

/** What the chat overlay shows: the chat types, the color of each, and where it sits. */
export interface ChatOverlayChoice {
  channels: ChatChannels;
  colors: ChatColors;
  position: ChatPosition;
}

/** One whisper, party or friend line, as a picture of its text (app/src/chat.rs). */
export interface ChatLine {
  id: number;
  channel: ChatChannel;
  atMs: number;
  /** The indented rest of the line before it. */
  continues: boolean;
  /** How often the reader had lost track of the chat box by then; a change means lines may be missing. */
  gaps: number;
  width: number;
  height: number;
  /** A PNG data URL whose alpha is the text. */
  png: string;
  /** Each word's columns, [from, to). */
  words: [number, number][];
}

export interface Settings {
  safeMode: boolean;
  /** Keep MesoVibe with the game: docked beside it, or the mini-HUD. */
  dock: boolean;
  /** The mini-HUD may sit over the game when the panel has no room. */
  miniHud: boolean;
  miniHudCorner: Corner;
  /** The chat overlay: the latest whisper, party and friend lines over the game. */
  chatOverlay: boolean;
  chatOverlayChannels: ChatChannels;
  chatOverlayColors: ChatColors;
  chatOverlayPosition: ChatPosition;
}

/** "Open with MapleStory Classic": the OS login item (app/src/presence.rs). */
export interface Startup {
  /** MesoVibe opens at login and waits for the game. */
  on: boolean;
  /** Why it can't be turned on from where MesoVibe is now, in words the player can act on. */
  problem: string | null;
}

/** Where updating stands (app/src/updater.rs). */
export type UpdateState = "idle" | "checking" | "upToDate" | "downloading" | "ready" | "installing" | "failed" | "off";

export interface UpdateStatus {
  state: UpdateState;
  /** This copy's version. */
  current: string;
  /** The new version while it downloads, is ready or installs. */
  version: string | null;
  /** Why the last look failed, in words the player can act on. */
  error: string | null;
  /** When GitHub last answered a look, in ms since the epoch. */
  checkedAtMs: number | null;
}

export interface Display {
  dock: boolean;
  miniHud: boolean;
  corner: Corner;
}

/** A picture of a game name, white with the text as alpha, drawn as a mask in the text color. */
export interface GamePicture {
  width: number;
  height: number;
  /** `data:image/png;base64,...` */
  png: string;
}

/** What the mini-HUD shows of the session: part of the panel's SessionSummary. */
export interface HudSummary {
  expPerHour: number | null;
  level: number | null;
  pctHundredths: number | null;
  minutesToLevel: number | null;
  mesosPerHour: number | null;
}

export interface AppInfo {
  version: string;
  os: string;
  eventSchema: number;
}

export type State = "off" | "no_game" | "capturing" | "paused" | "blocked";

export interface Reading {
  level: number | null;
  exp: number;
  pctHundredths: number;
  needed: number | null;
  confirmed: boolean;
}

/** What the engine saw and did on its last tick (app/src/engine.rs). */
export interface Status {
  reading: boolean;
  state: State;
  game: { width: number; height: number; scale: number } | null;
  exp: Reading | null;
  unreadable: string | null;
  error: string | null;
  /** The chat box this tick, while capturing. */
  chat: "reading" | "not_found" | "scrolled_back" | "unsupported" | null;
  calibration: "learning" | "provisional" | "confirmed";
  stale: boolean;
  /** How the pickup notices are being read. */
  notices: NoticeState;
  /** Mesos amounts waiting for a digit to be learned before they're counted. */
  waitingMesos: number;
  /** The map the player is on: null before one settled; no key when its name can't be read. */
  map: { key: string | null } | null;
  atMs: number;
}

export interface Host {
  appInfo(): Promise<AppInfo>;
  settings(): Promise<Settings>;
  startup(): Promise<Startup>;
  /** Turns the login item on or off; rejects with the reason when it can't. */
  setStartup(on: boolean): Promise<Startup>;
  setSafeMode(on: boolean): Promise<Settings>;
  retry(): Promise<void>;
  watch(on: boolean): Promise<void>;
  openPrivacySettings(): Promise<void>;
  storageGet(plugin: string): Promise<unknown>;
  storageSet(plugin: string, value: unknown): Promise<void>;
  /** Pictures of game names by key; a key with none kept is left out. */
  gameText(keys: string[]): Promise<Record<string, GamePicture>>;
  setDisplay(display: Display): Promise<Settings>;
  /** Passes the session strip's summary on to the mini-HUD. */
  shareSummary(summary: SessionSummary | null): Promise<void>;
  onStatus(handler: (status: Status) => void): Promise<unknown>;
  onGameEvent(handler: (event: GameEvent, atMs: number) => void): Promise<unknown>;
  onSnapshot(handler: (png: string) => void): Promise<unknown>;
  /** The Chat Hub's lines so far this run. */
  chatHistory(): Promise<ChatLine[]>;
  onChat(handler: (lines: ChatLine[]) => void): Promise<unknown>;
  setChatOverlay(on: boolean, channels: ChatChannels, colors: ChatColors, position: ChatPosition): Promise<Settings>;
  updateStatus(): Promise<UpdateStatus>;
  onUpdateStatus(handler: (status: UpdateStatus) => void): Promise<unknown>;
  /** "Check now": a look at GitHub at once. */
  checkForUpdates(): Promise<void>;
  /** "Restart to update": installs the ready update and restarts; rejects with the reason when it can't. */
  installUpdate(): Promise<void>;
}

/**
 * Listens for `event` when it's sent to this window or to every window. Tauri's plain `listen`
 * also hears events sent only to another window, so chat lines sent to the panel and to the chat
 * overlay arrived twice in each.
 */
function hear<T>(event: string, handler: (payload: T) => void): Promise<unknown> {
  return getCurrentWebviewWindow().listen<T>(event, (message) => handler(message.payload));
}

const tauri: Host = {
  appInfo: () => invoke<AppInfo>("app_info"),
  settings: () => invoke<Settings>("get_settings"),
  startup: () => invoke<Startup>("get_startup"),
  setStartup: (on) => invoke<Startup>("set_startup", { on }),
  setSafeMode: (on) => invoke<Settings>("set_safe_mode", { on }),
  retry: () => invoke<void>("retry_capture"),
  watch: (on) => invoke<void>("watch_capture", { on }),
  openPrivacySettings: () => invoke<void>("open_privacy_settings"),
  storageGet: (plugin) => invoke<unknown>("plugin_storage_get", { plugin }),
  gameText: (keys) => invoke<Record<string, GamePicture>>("game_text", { keys }),
  storageSet: (plugin, value) => invoke<void>("plugin_storage_set", { plugin, value }),
  setDisplay: ({ dock, miniHud, corner }) => invoke<Settings>("set_display", { dock, miniHud, corner }),
  shareSummary: (summary) => invoke<void>("share_summary", { summary: summary === null ? null : hudSummary(summary) }),
  onStatus: (handler) => hear<Status>("engine://status", handler),
  onGameEvent: (handler) => hear<{ atMs: number; event: GameEvent }>("engine://event", ({ event, atMs }) => handler(event, atMs)),
  onSnapshot: (handler) => hear<string>("engine://snapshot", handler),
  chatHistory: () => invoke<ChatLine[]>("chat_history"),
  onChat: (handler) => hear<ChatLine[]>("engine://chat", handler),
  setChatOverlay: (on, channels, colors, position) => invoke<Settings>("set_chat_overlay", { on, channels, colors, position }),
  updateStatus: () => invoke<UpdateStatus>("update_status"),
  onUpdateStatus: (handler) => hear<UpdateStatus>("updates://status", handler),
  checkForUpdates: () => invoke<void>("check_for_updates"),
  installUpdate: () => invoke<void>("install_update"),
};

/** Only what the mini-HUD shows, in the shape the host checks. */
export function hudSummary(summary: SessionSummary): HudSummary {
  const whole = (n: number | null) => (n === null ? null : Math.round(n));
  return {
    expPerHour: summary.expPerHour,
    level: whole(summary.level),
    pctHundredths: whole(summary.pctHundredths),
    minutesToLevel: summary.minutesToLevel,
    mesosPerHour: summary.mesosPerHour,
  };
}

/** The mini-HUD's host: it listens and asks for the latest summary, nothing else. */
export interface HudHost {
  latestSummary(): Promise<HudSummary | null>;
  onStatus(handler: (status: Status) => void): Promise<unknown>;
  onSummary(handler: (summary: HudSummary | null) => void): Promise<unknown>;
}

const tauriHud: HudHost = {
  latestSummary: () => invoke<HudSummary | null>("hud_summary"),
  onStatus: tauri.onStatus,
  onSummary: (handler) => hear<HudSummary | null>("hud://summary", handler),
};

/** The chat overlay's host: it reads the settings and the lines so far, and listens. */
export interface ChatHost {
  settings(): Promise<Settings>;
  chatHistory(): Promise<ChatLine[]>;
  onChat(handler: (lines: ChatLine[]) => void): Promise<unknown>;
  /** The player changed which chat types show, or their colors. */
  onChoice(handler: (choice: ChatOverlayChoice) => void): Promise<unknown>;
}

const tauriChat: ChatHost = {
  settings: tauri.settings,
  chatHistory: tauri.chatHistory,
  onChat: tauri.onChat,
  onChoice: (handler) => hear<ChatOverlayChoice>("chat://overlay", handler),
};

/** The development stand-in's scenario, when the URL asks for one outside the app. */
function mockScenario(): string | null {
  if ("__TAURI_INTERNALS__" in window) return null;
  return new URLSearchParams(location.search).get("mock");
}

// Each `import.meta.env.DEV` check stays inline, so release builds drop the stand-in entirely.

/** The host to use: the real one, or in development the stand-in when the URL asks for it. */
export async function connect(): Promise<Host> {
  if (import.meta.env.DEV) {
    const scenario = mockScenario();
    if (scenario !== null) return (await import("./mock.js")).mockHost(scenario);
  }
  return tauri;
}

/** The mini-HUD's host, real or (in development) the stand-in. */
export async function connectHud(): Promise<HudHost> {
  if (import.meta.env.DEV) {
    const scenario = mockScenario();
    if (scenario !== null) return (await import("./mock.js")).mockHudHost(scenario);
  }
  return tauriHud;
}

/** The chat overlay's host, real or (in development) the stand-in. */
export async function connectChat(): Promise<ChatHost> {
  if (import.meta.env.DEV) {
    const scenario = mockScenario();
    if (scenario !== null) return (await import("./mock.js")).mockChatHost(scenario);
  }
  return tauriChat;
}
