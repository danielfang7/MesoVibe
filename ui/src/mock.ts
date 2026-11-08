// A scripted stand-in for the host, for looking at the panel in a development browser without
// the game: `?mock=grind` (default play), `first` (first run), `blocked`, `safe`, `levelup`, and
// `moved` (first run from a disk image, where "Open with MapleStory Classic" can't be turned on),
// `loginfail` (the OS refuses the login item), `update` (MesoVibe 0.2.0 downloaded and ready:
// Restart to update) and `updatefail` (GitHub couldn't be reached; Check now then finds nothing).
// The mini-HUD page (hud.html) takes `grind`, `learning` and `blocked`; the chat overlay page
// (chat.html) takes `grind`. During play, whisper, party and friend lines arrive every few
// seconds, drawn here as pictures the way the host hands them over.
// Release builds leave this file out (ipc.ts loads it only in development).
import type { Event as GameEvent } from "@mesovibe/sdk";

import type { ChatChannel, ChatHost, ChatLine, ChatPosition, HudHost, HudSummary, Host, Settings, Startup, Status, UpdateStatus } from "./ipc.js";

const NEEDED = 356_916;

/** What the stand-in's friends say, in turn. */
const SAID: [ChatChannel, string][] = [
  ["whisper", "Lieon[Windia-25]>> where pot"],
  ["party", "Fangkip : party test"],
  ["friend", "Lieon() : kpq +5"],
  ["whisper", "Lieon[Windia-25]>> are you coming to the party quest in ellinia or should i go alone"],
  ["friend", "0004() : yo guys come to ellinia"],
];

let nextLine = 0;

/** A chat line as the host hands it over (app/src/chat.rs): white text in a PNG's alpha, and its words. */
function mockLine(channel: ChatChannel, text: string, atMs: number): ChatLine {
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d") as CanvasRenderingContext2D;
  context.font = "12px sans-serif";
  const width = Math.ceil(context.measureText(text).width) + 4;
  canvas.width = width;
  canvas.height = 16;
  context.font = "12px sans-serif";
  context.fillStyle = "#fff";
  context.fillText(text, 2, 12);
  const alpha = context.getImageData(0, 0, width, 16).data;
  const inked = (x: number) => Array.from({ length: 16 }, (_, y) => alpha[(y * width + x) * 4 + 3] ?? 0).some((a) => a >= 64);
  const words: [number, number][] = [];
  let start: number | null = null;
  let blank = 0;
  for (let x = 0; x < width; x += 1) {
    if (inked(x)) {
      start ??= x;
      blank = 0;
    } else if (start !== null && ++blank === 3) {
      words.push([Math.max(0, start - 1), x - 1]);
      start = null;
    }
  }
  if (start !== null) words.push([Math.max(0, start - 1), width]);
  nextLine += 1;
  return { id: nextLine, channel, atMs, continues: false, gaps: 0, width, height: 16, png: canvas.toDataURL("image/png"), words };
}

/** Calls `handler` with a new chat line every three seconds, starting after one. */
function chatter(handler: (lines: ChatLine[]) => void): void {
  let turn = 0;
  const say = () => {
    const [channel, text] = SAID[turn % SAID.length] as [ChatChannel, string];
    turn += 1;
    handler([mockLine(channel, text, Date.now())]);
  };
  window.setTimeout(() => {
    say();
    window.setInterval(say, 3000);
  }, 1000);
}

export function mockHost(scenario: string): Host {
  const firstRun = scenario === "first" || scenario === "moved";
  const problem = scenario === "moved" ? "Move MesoVibe to your Applications folder and open it from there first." : null;
  const refused = scenario === "loginfail" ? "MesoVibe couldn't add itself to your login items: Access is denied." : null;
  // A returning player kept the first-run default; a first run starts with nothing registered.
  let startup: Startup = { on: !firstRun && refused === null, problem };
  const checkedAtMs = Date.now() - 20 * 60_000;
  let update: UpdateStatus =
    scenario === "update"
      ? { state: "ready", current: "0.1.0-mock", version: "0.2.0", error: null, checkedAtMs }
      : scenario === "updatefail"
        ? { state: "failed", current: "0.1.0-mock", version: null, error: "MesoVibe couldn't reach GitHub to check for updates. It tries again in an hour.", checkedAtMs: null }
        : { state: "upToDate", current: "0.1.0-mock", version: null, error: null, checkedAtMs };
  const updateHandlers: ((status: UpdateStatus) => void)[] = [];
  const setUpdate = (next: UpdateStatus) => {
    update = next;
    for (const handler of updateHandlers) handler(update);
  };
  /** How many times the panel asked to install, for the panel tests (`window.__mockInstalls`). */
  const installs = { count: 0 };
  (window as unknown as { __mockInstalls: typeof installs }).__mockInstalls = installs;
  let settings: Settings = {
    disclosureAccepted: !firstRun,
    safeMode: scenario === "safe",
    dock: true,
    miniHud: true,
    miniHudCorner: "top-right",
    chatOverlay: true,
    chatOverlayChannels: { whisper: true, party: true, friend: true },
    chatOverlayColors: { whisper: "#66ff66", party: "#ff88cc", friend: "#ffaa44" },
    chatOverlayPosition: "left",
  };
  const statusHandlers: ((status: Status) => void)[] = [];
  const eventHandlers: ((event: GameEvent, atMs: number) => void)[] = [];
  const stored = new Map<string, unknown>();
  const started = Date.now();
  let exp = scenario === "levelup" ? NEEDED - 400 : 230_000;
  let level = 42;
  let lastGain = 0;

  const status = (): Status => {
    const now = Date.now();
    const t = now - started;
    const base = { reading: settings.disclosureAccepted && !settings.safeMode, atMs: now, stale: false, chat: null };
    if (!base.reading) return { ...base, state: "off", game: null, exp: null, unreadable: null, error: null, calibration: "learning" };
    const game = { width: 1366, height: 768, scale: 2 };
    if (scenario === "blocked") {
      return {
        ...base,
        state: "blocked",
        game,
        exp: null,
        unreadable: null,
        error:
          "MesoVibe needs Screen Recording permission to see the game window. Open System Settings > Privacy & Security > Screen & System Audio Recording, turn on MesoVibe (or the terminal you started it from), then quit and reopen it.",
        calibration: "learning",
      };
    }
    if (t < 1500) return { ...base, state: "no_game", game: null, exp: null, unreadable: null, error: null, calibration: "learning" };
    if (t < 4000) {
      return { ...base, state: "capturing", game, exp: null, unreadable: "learning", error: null, chat: "reading", calibration: "learning" };
    }
    const pctHundredths = Math.round((exp * 10_000) / NEEDED);
    return {
      ...base,
      state: "capturing",
      chat: "reading",
      game,
      exp: { level, exp, pctHundredths, needed: NEEDED, confirmed: true },
      unreadable: null,
      error: null,
      calibration: "confirmed",
    };
  };

  window.setInterval(() => {
    const s = status();
    for (const handler of statusHandlers) handler(s);
    // `idle`: the bar is read, but nothing is killed yet.
    if (s.exp === null || scenario === "idle") return;
    // A kill every two seconds; a death once in a while.
    if (s.atMs - lastGain >= 2000) {
      lastGain = s.atMs;
      const before = level;
      exp += 63 + Math.round(Math.random() * 40);
      if (Math.random() < 0.02) exp = Math.max(0, exp - 5_000);
      if (exp >= NEEDED) {
        exp -= NEEDED;
        level += 1;
      }
      const event: GameEvent = {
        type: "ExpChanged",
        level,
        exp,
        pct_hundredths: Math.round((exp * 10_000) / NEEDED),
        needed: NEEDED,
      };
      for (const handler of eventHandlers) handler(event, s.atMs);
      if (level > before) for (const handler of eventHandlers) handler({ type: "LevelUp", level }, s.atMs);
    }
  }, 250);

  return {
    appInfo: async () => ({ version: "0.1.0-mock", os: "macos", eventSchema: 2 }),
    settings: async () => settings,
    acceptDisclosure: async (safeMode, openWithGame) => {
      // Like the host: accepted whatever the login item does.
      startup = { ...startup, on: openWithGame && problem === null };
      return (settings = { ...settings, disclosureAccepted: true, safeMode });
    },
    startup: async () => startup,
    setStartup: async (on) => {
      if (on && (problem ?? refused) !== null) throw new Error(problem ?? refused ?? "");
      return (startup = { ...startup, on });
    },
    setSafeMode: async (on) => (settings = { ...settings, safeMode: on }),
    retry: async () => {},
    watch: async () => {},
    openPrivacySettings: async () => {},
    storageGet: async (plugin) => stored.get(plugin) ?? null,
    storageSet: async (plugin, value) => void stored.set(plugin, value),
    setDisplay: async ({ dock, miniHud, corner }) => (settings = { ...settings, dock, miniHud, miniHudCorner: corner }),
    shareSummary: async () => {},
    onStatus: async (handler) => statusHandlers.push(handler),
    onGameEvent: async (handler) => eventHandlers.push(handler),
    onSnapshot: async () => {},
    chatHistory: async () => (scenario === "grind" ? [mockLine("party", "DeepNight : idk", started)] : []),
    onChat: async (handler) => {
      if (scenario === "grind") chatter(handler);
      // Previews and tests can hand the panel lines of their own.
      (window as unknown as { mockChat?: (lines: ChatLine[]) => void }).mockChat = handler;
    },
    setChatOverlay: async (on, channels, colors, position) =>
      (settings = { ...settings, chatOverlay: on, chatOverlayChannels: channels, chatOverlayColors: colors, chatOverlayPosition: position }),
    updateStatus: async () => update,
    onUpdateStatus: async (handler) => updateHandlers.push(handler),
    checkForUpdates: async () => {
      // Like the host: the look answers a moment later, and finds nothing newer.
      setUpdate({ ...update, state: "checking" });
      window.setTimeout(() => setUpdate({ ...update, state: "upToDate", error: null, version: null, checkedAtMs: Date.now() }), 300);
    },
    installUpdate: async () => {
      installs.count += 1;
      // The real host restarts here; the stand-in says so and stays.
      setUpdate({ ...update, state: "installing" });
    },
  };
}

/** The chat overlay's stand-in: lines so far, and more every few seconds. */
export function mockChatHost(scenario: string): ChatHost {
  const settings: Settings = {
    disclosureAccepted: true,
    safeMode: false,
    dock: true,
    miniHud: true,
    miniHudCorner: "top-right",
    chatOverlay: true,
    chatOverlayChannels: { whisper: true, party: true, friend: true },
    chatOverlayColors: { whisper: "#66ff66", party: "#ff88cc", friend: "#ffaa44" },
    // `?position=above-chat` or `right` shows the other places in a browser.
    chatOverlayPosition: (new URLSearchParams(location.search).get("position") as ChatPosition | null) ?? "left",
  };
  return {
    settings: async () => settings,
    chatHistory: async () => (scenario === "grind" ? [mockLine("party", "DeepNight : idk", Date.now())] : []),
    onChat: async (handler) => {
      if (scenario === "grind") chatter(handler);
      (window as unknown as { mockChat?: (lines: ChatLine[]) => void }).mockChat = handler;
    },
    onChoice: async (handler) => {
      (window as unknown as { mockChoice?: typeof handler }).mockChoice = handler;
    },
  };
}

/** The mini-HUD's stand-in: a steady grind, still learning, or blocked. */
export function mockHudHost(scenario: string): HudHost {
  const summary: HudSummary | null =
    scenario === "grind" ? { expPerHour: 48_210, level: 42, pctHundredths: 6462, minutesToLevel: 51 } : null;
  const status = (): Status => {
    const base = { reading: true, atMs: Date.now(), stale: false, chat: null, game: { width: 1366, height: 768, scale: 2 } };
    if (scenario === "blocked") {
      return { ...base, state: "blocked", exp: null, unreadable: null, error: "No permission.", calibration: "learning" };
    }
    if (scenario === "learning") {
      return { ...base, state: "capturing", exp: null, unreadable: "learning", error: null, calibration: "learning" };
    }
    return {
      ...base,
      state: "capturing",
      exp: { level: 42, exp: 230_640, pctHundredths: 6462, needed: NEEDED, confirmed: true },
      unreadable: null,
      error: null,
      calibration: "confirmed",
    };
  };
  const statusHandlers: ((status: Status) => void)[] = [];
  window.setInterval(() => {
    const s = status();
    for (const handler of statusHandlers) handler(s);
  }, 250);
  return {
    latestSummary: async () => summary,
    onStatus: async (handler) => statusHandlers.push(handler),
    onSummary: async () => {},
  };
}
