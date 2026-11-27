// A scripted stand-in for the host, for looking at the panel in a development browser without
// the game: `?mock=grind` (default play), `first` (a new install, before any chat), `blocked`,
// `safe`, `levelup`, and `moved` (run from a disk image, where "Open with MapleStory Classic"
// can't be turned on),
// `loginfail` (the OS refuses the login item), `update` (MesoVibe 0.2.0 downloaded and ready:
// Restart to update), `updatefail` (GitHub couldn't be reached; Check now then finds nothing)
// `unreadable` (the Session Tracker's saved data can't be opened the first time it's read), and
// `loot` (play with pickups: a map read from the minimap, mesos every few kills, a second map
// after a while), and `site` (a steady session for the website's pictures, on the game picture's
// map: level 41, mesos every few kills, a Meat or a Lemon from the quick slots when HP or MP runs
// low, loot sold at a shop once, no deaths; it reaches 39%, the game picture's EXP, about 47
// minutes in).
// The mini-HUD page (hud.html) takes `grind`, `learning` and `blocked`; the chat overlay page
// (chat.html) takes `grind`; the price label page (label.html) takes `label`, `labelold`,
// `labelnpc`, `labelwide`, `labelnew`, `labeluntradable`, `labellearn` and `labelpick`. During play, whisper, party and friend lines arrive every few
// seconds, drawn here as pictures the way the host hands them over.
// Release builds leave this file out (ipc.ts loads it only in development).
import { SCHEMA_VERSION, type Event as GameEvent } from "@mesovibe/sdk";

import type {
  ChatChannel,
  ChatHost,
  ChatLine,
  ChatPosition,
  GamePicture,
  HudHost,
  HudSummary,
  Host,
  LabelHost,
  LabelShown,
  Pointed,
  Settings,
  Startup,
  Status,
  UpdateStatus,
} from "./ipc.js";
import { mockPrices } from "./mock-prices.js";

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

/**
 * What the `loot` scenario starts with saved: one session from a build that counted loot from the
 * pickup notices, which the Loot tab still shows, as an estimate.
 */
function noticeEraSave(now: number): unknown {
  const startedAtMs = now - 86_400_000;
  return {
    version: 1,
    history: [
      {
        startedAtMs,
        endedAtMs: startedAtMs + 1_800_000,
        activeMs: 1_800_000,
        expGained: 41_667,
        expLost: 0,
        startLevel: 41,
        endLevel: 42,
        maps: [{ k: "0a1b2c3d4e5f6071", p: 1_800_000, x: 41_667, m: 57_800, r: 57_800, l: 1_800_000, s: 41, e: 42, i: [{ k: "#4000028", c: 30 }] }],
      },
    ],
    current: null,
    allTime: { mesos: 0, playedMs: 0 },
    catalog: { "#4000028": { n: "Tablecloth", t: "Etc", v: 34 } },
  };
}

export function mockHost(scenario: string): Host {
  const problem = scenario === "moved" ? "Move MesoVibe to your Applications folder and open it from there first." : null;
  const refused = scenario === "loginfail" ? "MesoVibe couldn't add itself to your login items: Access is denied." : null;
  // A returning player kept the first-run default; a first run starts with nothing registered.
  // Like the host's first start: on, unless MesoVibe can't keep a login item from where it is.
  let startup: Startup = { on: problem === null && refused === null, problem };
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
    safeMode: scenario === "safe",
    dock: true,
    miniHud: true,
    miniHudCorner: "top-right",
    chatOverlay: true,
    chatOverlayChannels: { whisper: true, party: true, friend: true },
    chatOverlayColors: { whisper: "#66ff66", party: "#ff88cc", friend: "#ffaa44" },
    chatOverlayPosition: "left",
    priceWorld: "windia",
    priceLabel: true,
  };
  const statusHandlers: ((status: Status) => void)[] = [];
  const eventHandlers: ((event: GameEvent, atMs: number) => void)[] = [];
  const stored = new Map<string, unknown>(scenario === "loot" ? [["session-tracker", noticeEraSave(Date.now())]] : []);
  /** Plugin reads and saves, for the panel tests (`window.__mockStorage`). */
  const storage = { reads: 0, saves: 0 };
  (window as unknown as { __mockStorage: typeof storage }).__mockStorage = storage;
  const started = Date.now();
  let exp = scenario === "levelup" ? NEEDED - 400 : scenario === "site" ? 24_600 : 230_000;
  let level = scenario === "site" ? 41 : 42;
  let lastGain = 0;
  /** The `loot` scenario's meso total, as the inventory shows it, and the last one sent. */
  let mesos = 1_204_330;
  let sentMesos = 0;
  /**
   * The `site` scenario's HP and MP, its Meat and Lemon counts in quick slots 0 and 1, and the
   * slots whose new count is sent on the next tick: the engine sends a slot's change a frame after
   * the HP or MP rise that goes with it.
   */
  const pots = { hp: 1_180, mp: 980, counts: [214, 188] as [number, number], slotsSent: false, due: [] as number[] };
  /** The `site` scenario sells its loot at a shop this long in, once. */
  const SALE_MS = 30 * 60_000;
  let sold = false;

  const status = (): Status => {
    const now = Date.now();
    const t = now - started;
    const base = {
      reading: !settings.safeMode,
      atMs: now,
      stale: false,
      chat: null,
      hpMp: null,
      hpMpState: "off" as const,
      hpMpMissing: null,
      map: null,
      mesos: settings.safeMode ? null : ("reading" as const),
      slots: settings.safeMode ? null : ("reading" as const),
      // `&tooltips=learning` shows the price label still learning the game's letters.
      tooltips:
        settings.safeMode || !settings.priceLabel
          ? null
          : new URLSearchParams(location.search).get("tooltips") === "learning"
            ? ("learning" as const)
            : ("reading" as const),
      fullScreen: false,
    };
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
    if (scenario === "dark") return darkStatus(started, game);
    if (scenario === "fullscreen") return fullScreenStatus(started);
    // A first reading named by the digits' shapes, which no EXP gain has confirmed yet.
    if (scenario === "checking") {
      const unconfirmed = { level: 42, exp: 230_640, expShown: true, pctHundredths: 6462, needed: NEEDED, confirmed: false };
      return { ...base, state: "capturing", game, exp: unconfirmed, unreadable: null, error: null, calibration: "provisional" };
    }
    if (t < 4000) {
      return { ...base, state: "capturing", game, exp: null, unreadable: "learning", error: null, chat: "reading", calibration: "learning", hpMpState: "learning" };
    }
    const pctHundredths = Math.round((exp * 10_000) / NEEDED);
    return {
      ...base,
      state: "capturing",
      chat: "reading",
      // A potion every few seconds, spent again in between.
      hpMp:
        scenario === "site"
          ? { hp: pots.hp, maxHp: 1386, mp: pots.mp, maxMp: 1140 }
          : { hp: 1386 - (Math.floor(t / 250) % 12) * 40, maxHp: 1386, mp: 1140 - (Math.floor(t / 250) % 9) * 16, maxMp: 1140 },
      hpMpState: "reading",
      map: scenario === "loot" ? { key: lootMap(t) } : scenario === "site" ? { key: SITE_MAP } : null,
      game,
      exp: { level, exp, expShown: true, pctHundredths, needed: NEEDED, confirmed: true },
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
    if (scenario === "site") {
      const slot = (index: number) => {
        const icon = index === 0 ? "meat" : "lemon";
        const changed: GameEvent = { type: "QuickSlotChanged", slot: index, count_key: `${icon}-${pots.counts[index]}`, icon_key: icon };
        for (const handler of eventHandlers) handler(changed, s.atMs);
      };
      if (!pots.slotsSent) {
        pots.slotsSent = true;
        slot(0);
        slot(1);
      }
      for (const index of pots.due.splice(0)) slot(index);
    }
    // A kill every two seconds; a death once in a while.
    if (s.atMs - lastGain >= 2000) {
      lastGain = s.atMs;
      const before = level;
      exp += 63 + Math.round(Math.random() * 40);
      if (scenario !== "site" && Math.random() < 0.02) exp = Math.max(0, exp - 5_000);
      if (exp >= NEEDED) {
        exp -= NEEDED;
        level += 1;
      }
      const event: GameEvent = {
        type: "ExpChanged",
        level,
        exp,
        exp_shown: true,
        pct_hundredths: Math.round((exp * 10_000) / NEEDED),
        needed: NEEDED,
      };
      for (const handler of eventHandlers) handler(event, s.atMs);
      if (level > before) for (const handler of eventHandlers) handler({ type: "LevelUp", level }, s.atMs);
      if (scenario === "loot" || scenario === "site") {
        const map = scenario === "loot" ? lootMap(s.atMs - started) : SITE_MAP;
        if (map !== lastMap) {
          lastMap = map;
          for (const handler of eventHandlers) handler({ type: "MapChanged", map_key: map }, s.atMs);
        }
        if (scenario === "site") {
          const reading = (): GameEvent => ({ type: "HpMpChanged", hp: pots.hp, max_hp: 1386, mp: pots.mp, max_mp: 1140, continuous: true });
          // A hit now and then, and a skill's MP every kill; a potion when one runs low.
          if (Math.random() < 0.25) pots.hp -= 30 + Math.round(Math.random() * 30);
          pots.mp -= 4;
          for (const handler of eventHandlers) handler(reading(), s.atMs);
          for (const [index, low, amount] of [
            [0, 1_000, 160],
            [1, 820, 150],
          ] as const) {
            if ((index === 0 ? pots.hp : pots.mp) >= low) continue;
            if (index === 0) pots.hp += amount;
            else pots.mp += amount;
            pots.counts[index] -= 1;
            pots.due.push(index);
            for (const handler of eventHandlers) handler(reading(), s.atMs);
          }
        }
        // Mesos picked up about every other kill, with the inventory open: its total is sent, like
        // the engine's, whenever it changed.
        if (Math.random() < 0.5) mesos += 40 + Math.round(Math.random() * 80);
        if (mesos !== sentMesos) {
          sentMesos = mesos;
          const total: GameEvent = { type: "MesoChanged", total: mesos, source: "inventory" };
          for (const handler of eventHandlers) handler(total, s.atMs);
        }
        // The shop's total as its window opens, then after the loot is sold.
        if (scenario === "site" && !sold && s.atMs - started >= SALE_MS) {
          sold = true;
          for (const amount of [mesos, mesos + 8_460]) {
            const total: GameEvent = { type: "MesoChanged", total: amount, source: "shop" };
            for (const handler of eventHandlers) handler(total, s.atMs);
          }
          mesos += 8_460;
          sentMesos = mesos;
        }
      }
    }
  }, 250);
  let lastMap: string | null = null;

  return {
    appInfo: async () => ({ version: "0.1.0-mock", os: "macos", eventSchema: SCHEMA_VERSION }),
    settings: async () => settings,
    startup: async () => startup,
    setStartup: async (on) => {
      if (on && (problem ?? refused) !== null) throw new Error(problem ?? refused ?? "");
      return (startup = { ...startup, on });
    },
    setSafeMode: async (on) => (settings = { ...settings, safeMode: on }),
    retry: async () => {},
    watch: async () => {},
    openPrivacySettings: async () => {},
    gameText: async (keys) => Object.fromEntries(keys.flatMap((key) => (MAP_NAMES[key] ? [[key, namePicture(MAP_NAMES[key])]] : []))),
    storageGet: async (plugin) => {
      // Like a file another program holds open: the first read fails, a later one works.
      if (scenario === "unreadable" && storage.reads++ === 0) throw new Error("Access is denied. (os error 5)");
      return stored.get(plugin) ?? null;
    },
    storageSet: async (plugin, value) => {
      storage.saves++;
      stored.set(plugin, value);
    },
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
    ...mockPrices(scenario),
    setPriceWorld: async (world) => (settings = { ...settings, priceWorld: world }),
    setPriceLabel: async (on) => (settings = { ...settings, priceLabel: on }),
    onPricePointed: async (handler) => {
      // The panel tests point at items with `window.__mockPoint([id])`, or at a name being learned
      // with `window.__mockPoint({ ids: [], stuck: { token, candidates } })`.
      (window as unknown as { __mockPoint: (pointed: number[] | Pointed) => void }).__mockPoint = (pointed) =>
        handler(Array.isArray(pointed) ? { ids: pointed, stuck: null } : pointed);
    },
    teachTooltipName: async (token, name) => {
      // The panel tests read the answers sent from `window.__mockTaught`.
      const w = window as unknown as { __mockTaught?: [number, string | null][] };
      (w.__mockTaught ??= []).push([token, name]);
    },
  };
}

/**
 * The price label's stand-in (label.html): `?mock=label` a fresh price, `labelold` two-day-old
 * prices, `labelnpc` an item with no Free Market price, `labelwide` a card wider than the window
 * would give, `labelnew` a price with no week yet, `labeluntradable` an untradable copy,
 * `labellearn` a name being learned, `labelpick` one the Prices tab can teach.
 */
export function mockLabelHost(scenario: string): LabelHost {
  const base: LabelShown = { usual: 21_000_000, change: 0.08, npc: 20_000, world: "Windia", oldSinceMs: null, untradable: false, learning: null, cardWidth: 236, cardLeft: 8 };
  const learning = (pick: boolean): LabelShown => ({ learning: { pick }, cardWidth: 236, cardLeft: 8 });
  const view: LabelShown =
    scenario === "labelold"
      ? { ...base, change: -0.03, oldSinceMs: Date.now() - 48 * 60 * 60 * 1000 }
      : scenario === "labelnpc"
        ? { ...base, usual: null, change: null, npc: 30 }
        : scenario === "labelwide"
          ? { ...base, cardWidth: 180 }
          : scenario === "labelnew"
            ? { ...base, usual: 1_050, change: null, cardWidth: 288 }
            : scenario === "labeluntradable"
              ? { ...base, usual: 1_333_333, untradable: true }
              : scenario === "labellearn"
                ? learning(false)
                : scenario === "labelpick"
                  ? learning(true)
                  : base;
  return {
    onView: async (handler) => handler(view),
  };
}

/** The chat overlay's stand-in: lines so far, and more every few seconds. */
export function mockChatHost(scenario: string): ChatHost {
  const settings: Settings = {
    safeMode: false,
    dock: true,
    miniHud: true,
    miniHudCorner: "top-right",
    chatOverlay: true,
    chatOverlayChannels: { whisper: true, party: true, friend: true },
    chatOverlayColors: { whisper: "#66ff66", party: "#ff88cc", friend: "#ffaa44" },
    // `?position=above-chat` or `right` shows the other places in a browser.
    chatOverlayPosition: (new URLSearchParams(location.search).get("position") as ChatPosition | null) ?? "left",
    priceWorld: "windia",
    priceLabel: true,
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

/**
 * `dark`: the game's screen stays black, as when MesoVibe reads only black bars. Its clock runs 20
 * times fast, so the panel's wait for a long dark spell takes half a second.
 */
function darkStatus(started: number, game: Status["game"]): Status {
  return {
    reading: true,
    atMs: started + (Date.now() - started) * 20,
    stale: false,
    chat: "not_found",
    state: "capturing",
    game,
    fullScreen: false,
    exp: null,
    unreadable: "screen_dark",
    error: null,
    calibration: "learning",
    hpMp: null,
    hpMpState: "reading",
    hpMpMissing: null,
    map: null,
    mesos: "reading",
    slots: "reading",
  };
}

/**
 * `fullscreen`: the game fills a 1512×982 Mac display at a size MesoVibe can't read, so no HUD is
 * ever found. The clock runs 20 times fast, as in `dark`.
 */
function fullScreenStatus(started: number): Status {
  return {
    ...darkStatus(started, { width: 1512, height: 982, scale: 2 }),
    fullScreen: true,
    unreadable: "hud_not_found",
    mesos: "unsupported",
    slots: "unsupported",
  };
}

/** The `site` scenario's map, the one in the website's game picture. */
const SITE_MAP = "e45f0c2a9b7d3e61";
/** The `loot` scenario's maps (the first for two minutes, then the second), and the `site` one. */
const MAP_NAMES: Record<string, string> = {
  "0a1b2c3d4e5f6071": "Ant Tunnel I",
  "8192a3b4c5d6e7f8": "Ant Tunnel II",
  [SITE_MAP]: "East Rocky Mountain V",
};
function lootMap(elapsedMs: number): string {
  return elapsedMs < 120_000 ? "0a1b2c3d4e5f6071" : "8192a3b4c5d6e7f8";
}

/** A name drawn the way the engine hands pictures over: white, with the letters as alpha. */
function namePicture(name: string): GamePicture {
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (context === null) return { width: 1, height: 1, png: canvas.toDataURL() };
  context.font = "10px sans-serif";
  canvas.width = Math.ceil(context.measureText(name).width) + 2;
  canvas.height = 12;
  context.font = "10px sans-serif";
  context.fillStyle = "white";
  context.textBaseline = "top";
  context.fillText(name, 1, 1);
  return { width: canvas.width, height: canvas.height, png: canvas.toDataURL("image/png") };
}

/**
 * The mini-HUD's stand-in: a steady grind, still learning, blocked, a screen that stays dark, or
 * Full Screen at a size MesoVibe can't read (`fullscreen-grind`: after a session's rate came in).
 * Any other scenario, like `idle`, reads the bar before the first gain.
 */
export function mockHudHost(scenario: string): HudHost {
  const summary: HudSummary | null =
    scenario === "grind" || scenario === "fullscreen-grind" ? { expPerHour: 48_210, level: 42, pctHundredths: 6462, minutesToLevel: 51, lootPerHour: 21_480 } : null;
  const started = Date.now();
  const status = (): Status => {
    const base = {
      reading: true,
      atMs: Date.now(),
      stale: false,
      chat: null,
      game: { width: 1366, height: 768, scale: 2 },
      fullScreen: false,
      hpMp: null,
      hpMpState: "reading" as const,
      hpMpMissing: null,
      map: null,
      mesos: "reading" as const,
      slots: "reading" as const,
    };
    if (scenario === "dark") return darkStatus(started, base.game);
    if (scenario.startsWith("fullscreen")) return fullScreenStatus(started);
    if (scenario === "blocked") {
      return { ...base, state: "blocked", exp: null, unreadable: null, error: "No permission.", calibration: "learning" };
    }
    if (scenario === "learning") {
      return { ...base, state: "capturing", exp: null, unreadable: "learning", error: null, calibration: "learning" };
    }
    return {
      ...base,
      state: "capturing",
      exp: { level: 42, exp: 230_640, expShown: true, pctHundredths: 6462, needed: NEEDED, confirmed: true },
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
