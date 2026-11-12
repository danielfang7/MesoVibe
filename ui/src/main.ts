// The side panel: the session strip and icon rail, and the Session, Chat (the Chat Hub),
// "What MesoVibe sees" and Settings views.
// Every word on screen is a text node, never parsed HTML.
import { formatCompact, formatDigits, formatMinutes, formatPercent } from "@mesovibe/sdk";
import type { CaptureState, SessionSummary } from "@mesovibe/sdk";

import { CHANNEL_NAME, clockTime, messageText, messages } from "./chatlines.js";
import { darkWatch, fullScreenWatch } from "./dark.js";
import { append, h, icon, replace } from "./dom.js";
import { connect } from "./ipc.js";
import type { AppInfo, ChatChannel, ChatChannels, ChatColors, ChatLine, ChatPosition, Corner, Display, Host, Settings, Startup, Status, UpdateStatus } from "./ipc.js";
import { PluginHost, confirmDialog, fill, namePicture, renderView } from "./plugins.js";

type ViewName = "session" | "chat" | "sees" | "settings";

const CHANNELS: ChatChannel[] = ["whisper", "party", "friend"];
const TAB_NAME: Record<ChatChannel, string> = { whisper: "Whispers", party: "Party", friend: "Friends" };
const NO_LINES: Record<ChatChannel, string> = {
  whisper: "No whispers yet.",
  party: "No party chat yet.",
  friend: "No friend chat yet.",
};
/** Lines the panel keeps, like the host's log (app/src/chat.rs). */
const KEEP_LINES = 1000;
/** The colors chat over the game can take, named, so a color is never the only sign of a choice. */
const CHAT_COLORS: [string, string][] = [
  ["#00ffff", "Cyan"],
  ["#ffffff", "White"],
  ["#ffff00", "Yellow"],
  ["#66ff66", "Green"],
  ["#ff88cc", "Pink"],
  ["#ffaa44", "Orange"],
];

/** Where chat over the game can sit, named for the picker, in their order across the game. */
const CHAT_POSITIONS: [ChatPosition, string][] = [
  ["left", "Left side"],
  ["above-chat", "Above chat"],
  ["right", "Right side"],
];

const CORNERS: [Corner, string][] = [
  ["top-left", "Top left"],
  ["top-right", "Top right"],
  ["bottom-left", "Bottom left"],
  ["bottom-right", "Bottom right"],
];

/** How long a notice like "Session reset." stays, with its Undo. */
const NOTICE_MS = 10_000;
/** How long after End or Reset its notice may still take keyboard focus for Undo. */
const FOCUS_UNDO_MS = 2_000;
/** After this long without a reading, values dim and say how old they are. */
const STALE_MS = 15_000;

const root = document.getElementById("app") as HTMLElement;
/** The host, real or (in development) the stand-in. */
let host: Host;

function button(label: string, onClick: () => void, style: "primary" | "secondary" = "secondary", key?: string) {
  return h("button", { class: `button ${style}`, attrs: { type: "button", "data-key": key }, on: { click: onClick } }, label);
}

function toggle(label: string, on: boolean, onChange: (on: boolean) => void, key: string, disabled = false) {
  return h(
    "button",
    {
      class: "switch",
      attrs: { type: "button", role: "switch", "aria-checked": on ? "true" : "false", "data-key": key, disabled },
      on: { click: () => onChange(!on) },
    },
    h("span", { class: "switch-label" }, label),
    h("span", { class: "switch-track", attrs: { "aria-hidden": "true" } }, h("span", { class: "switch-knob" })),
  );
}

let hints = 0;
/**
 * A control with its explanation in a hint that shows while the pointer is on it or it has
 * keyboard focus, so Settings reads as a short list of names. Screen readers hear the hint as the
 * control's description. Anything the player must act on, like why a switch is off, stays on the
 * page instead.
 */
function hinted(control: HTMLElement, hint: string, key?: string): HTMLElement {
  hints += 1;
  const id = `hint-${hints}`;
  control.classList.add("hint-anchor");
  for (const button of control.matches("button") ? [control] : control.querySelectorAll("button")) {
    button.setAttribute("aria-describedby", id);
  }
  return h("div", { class: "hinted" }, control, h("p", { class: "hint", attrs: { id, role: "tooltip", "data-key": key } }, hint));
}

/** Where MesoVibe waits while its panel is closed, in the OS's own words. */
function trayName(os: string | undefined): string {
  return os === "macos" ? "menu bar" : "system tray";
}

/** What "Open with MapleStory Classic" does (app/src/presence.rs). */
function startupNote(os: string | undefined): string {
  const tray = trayName(os);
  return `MesoVibe starts when you log in and waits in the ${tray}. When MapleStory Classic starts, MesoVibe opens with it. Closing the panel keeps MesoVibe waiting; quit from its ${tray} icon.`;
}

/** What the update check sends, in Settings > About. */
const UPDATE_CHECK_NOTE =
  "Once a day MesoVibe asks GitHub whether a newer version is out. The request carries nothing about you or the game; GitHub sees your IP address, as any website does. An update installs only when you choose Restart to update.";

// MARK: the panel

interface Panel {
  settings: Settings;
  status: Status | null;
  summary: SessionSummary | null;
  view: ViewName;
  notice: { text: string; undo?: string; until: number } | null;
  /** When the EXP bar last read, for "last read 40s ago". */
  lastReadAt: number | null;
  /** The screen has been dark for long: not a map change (dark.ts). */
  longDark: boolean;
  /** Full Screen with no HUD seen for long: maybe a size MesoVibe can't read (dark.ts). */
  fullScreenUnread: boolean;
  snapshot: string | null;
  helpOpen: boolean;
  crashed: string | null;
  info: AppInfo | null;
  /** The first EXP/hr of this run has arrived (its one animation). */
  rateShown: boolean;
  /** "Open with MapleStory Classic", once the host has said. */
  startup: Startup | null;
  /** Why the last change to it didn't happen. */
  startupError: string | null;
  /** Where updating stands, once the host has said (app/src/updater.rs). */
  update: UpdateStatus | null;
  /** Why the last "Restart to update" didn't install. */
  updateError: string | null;
  /** The Chat Hub's lines this run, oldest first. */
  chat: ChatLine[];
  chatTab: ChatChannel;
  /** Messages that arrived while their tab wasn't open. */
  unread: Record<ChatChannel, number>;
}

/** The session strip's first line: whether MesoVibe is reading the game, and if not, why. */
function statusLine(panel: Panel): { text: string; tone: "live" | "idle" | "warn" | "danger" } {
  const status = panel.status;
  if (panel.settings.safeMode) return { text: "Safe mode: MesoVibe isn't reading the game.", tone: "idle" };
  if (status === null || !status.reading) return { text: "Starting…", tone: "idle" };
  switch (status.state) {
    case "off":
      return { text: "Not reading the game.", tone: "idle" };
    case "no_game":
      return { text: "Looking for MapleStory Classic…", tone: "idle" };
    case "paused":
      return { text: "Paused while the game is minimized or hidden.", tone: "idle" };
    case "blocked":
      return { text: "Can't capture the game window.", tone: "danger" };
    case "capturing":
      break;
  }
  // Dark for a moment is a map change's fade. Dark for long, MesoVibe isn't seeing the HUD. The
  // watch keeps a dark spell through ticks that brought no new frame, which say nothing either way.
  if (panel.fullScreenUnread) {
    return { text: "Can't read the game in Full Screen at this size. In the game's Options > Graphics, try windowed or 1366×768.", tone: "warn" };
  }
  if (panel.longDark) return { text: "Can't see the EXP bar. If you play in Full Screen, try windowed.", tone: "warn" };
  switch (status.unreadable) {
    case "learning":
    case "not_calibrated":
      return {
        text: "Learning your EXP bar from your first few EXP gains. Only the first time: after that it reads at once.",
        tone: "live",
      };
    case "hud_not_found":
    case "no_text":
    case "unknown_glyph":
    case "garbled":
      return { text: "Can't see the EXP bar. Is something covering it?", tone: "warn" };
    case "unsupported_size":
    case "unsupported_scale":
      return { text: "MesoVibe can't read the game at this size yet. Try windowed, up to 1366×768.", tone: "warn" };
    default:
      return { text: "Reading game", tone: "live" };
  }
}

function captureState(status: Status | null): CaptureState {
  if (status === null || !status.reading) return "off";
  return status.state;
}

function shell(initial: Settings): void {
  const panel: Panel = {
    settings: initial,
    status: null,
    summary: null,
    view: "session",
    notice: null,
    lastReadAt: null,
    longDark: false,
    fullScreenUnread: false,
    snapshot: null,
    helpOpen: false,
    crashed: null,
    info: null,
    rateShown: false,
    startup: null,
    startupError: null,
    update: null,
    updateError: null,
    chat: [],
    chatTab: "whisper",
    unread: { whisper: 0, party: 0, friend: 0 },
  };

  // The strip's parts, built once and updated in place.
  const dot = h("span", { class: "dot", attrs: { "aria-hidden": "true" } });
  const statusText = h("span", { class: "status-text" });
  const hero = h("span", { class: "number-hero" });
  const heroUnit = h("span", { class: "hero-unit" }, "EXP/hr");
  const heroEmpty = h("p", { class: "hero-empty" });
  /** After End or Reset, keyboard focus goes to the matching notice's Undo. */
  let focusUndo: { undo: string; until: number } | null = null;
  const sessionAction = (action: "end" | "reset") => () => {
    focusUndo = { undo: `undo-${action}`, until: Date.now() + FOCUS_UNDO_MS };
    tracker.action(action);
  };
  /** What the strip last drew in its help row and notice, so a repaint with nothing new keeps them. */
  let paintedHelp = "";
  let paintedNotice: Panel["notice"] | undefined;
  const noticeBody = (notice: NonNullable<Panel["notice"]>) => {
    if (notice.undo === undefined) return h("div", { class: "notice-body" }, h("span", {}, notice.text));
    const undo = notice.undo;
    const undoButton = button(
      "Undo",
      () => {
        tracker.action(undo);
        panel.notice = null;
        paintStrip();
      },
      "secondary",
      "notice-undo",
    );
    // Focus arrives here straight after End or Reset; an Enter held down on End must not repeat onto
    // Undo and take it back.
    undoButton.addEventListener("keydown", (event) => {
      if (event.repeat && (event.key === "Enter" || event.key === " ")) event.preventDefault();
    });
    return h("div", { class: "notice-body" }, h("span", {}, notice.text), undoButton);
  };
  const resetButton = h(
    "button",
    {
      class: "icon-button",
      attrs: { type: "button", title: "Start over without saving this session (you can undo it)", "data-key": "strip-reset" },
      on: { click: sessionAction("reset") },
    },
    icon("reset"),
    "Reset",
  );
  const endButton = h(
    "button",
    {
      class: "icon-button",
      attrs: { type: "button", title: "End the session and save it to Past sessions (you can undo it)", "data-key": "strip-end" },
      on: { click: sessionAction("end") },
    },
    icon("end"),
    "End",
  );
  const sessionActions = h("div", { class: "strip-actions" }, resetButton, endButton);
  const toLevel = h("p", { class: "to-level" });
  const progressSlot = h("div", { class: "strip-progress" });
  const noticeSlot = h("div", { class: "notice", attrs: { role: "status", "aria-live": "polite" } });
  const updateSlot = h("div", { class: "notice", attrs: { role: "status", "aria-live": "polite", "data-key": "strip-update" } });
  const helpSlot = h("div", { class: "help" });
  const strip = h(
    "header",
    { class: "strip", attrs: { "aria-label": "Session", tabindex: "-1" } },
    h("div", { class: "strip-status", attrs: { role: "status", "aria-live": "polite" } }, dot, statusText),
    helpSlot,
    h(
      "div",
      { class: "strip-hero" },
      h("div", { class: "hero-stack" }, h("span", { class: "hero" }, hero, heroUnit), heroEmpty),
      sessionActions,
    ),
    toLevel,
    progressSlot,
    noticeSlot,
    updateSlot,
  );
  const view = h("main", { class: "view", attrs: { tabindex: "-1" } });
  const railButtons = new Map<ViewName, HTMLButtonElement>();
  const railItem = (name: ViewName, label: string, glyph: "session" | "chat" | "sees" | "settings", ...extra: Node[]) => {
    const item = h(
      "button",
      {
        class: "rail-item",
        attrs: { type: "button", "aria-label": label, title: label },
        on: { click: () => show(name) },
      },
      icon(glyph),
      ...extra,
    );
    railButtons.set(name, item);
    return item;
  };
  const chatBadge = h("span", { class: "rail-badge number", attrs: { "aria-hidden": "true" } });
  const rail = h(
    "nav",
    { class: "rail", attrs: { "aria-label": "Views" } },
    railItem("session", "Session", "session"),
    railItem("chat", "Chat", "chat", chatBadge),
    h("span", { class: "rail-spacer" }),
    railItem("settings", "Settings", "settings"),
    railItem("sees", "What MesoVibe sees", "sees"),
  );
  replace(root, h("div", { class: "app" }, h("div", { class: "main" }, strip, view), rail));

  const tracker = new PluginHost(
    "session-tracker",
    "Session Tracker",
    host,
    {
      render: () => {
        if (panel.view === "session") paintView();
      },
      summary: (summary) => {
        panel.summary = summary;
        paintStrip();
        void host.shareSummary(summary);
      },
      notice: (text, undo) => {
        panel.notice = undo === undefined ? { text, until: Date.now() + NOTICE_MS } : { text, undo, until: Date.now() + NOTICE_MS };
        paintStrip();
      },
      crashed: (message) => {
        panel.crashed = message;
        paintStrip();
        paintView();
      },
    },
    "off",
  );
  /**
   * The tracker has had the EXP bar since it started. The engine reports the bar only when it
   * changes, so a tracker that missed that report (the panel opened after it, or the tracker was
   * restarted) gets the bar from the status instead, and shows the level and its bar before the
   * first gain.
   */
  let trackerHasBar = false;
  /** Likewise the map: the engine reports it only when it changes. */
  let trackerHasMap = false;
  const startTracker = () => {
    trackerHasBar = false;
    trackerHasMap = false;
    void tracker.start();
  };
  startTracker();

  const setSafeMode = async (on: boolean) => {
    panel.settings = await host.setSafeMode(on);
    panel.helpOpen = false;
    paintStrip();
    paintView();
  };

  /** "Restart to update": MesoVibe restarts into the new version, so this only returns when it couldn't. */
  const installUpdate = async () => {
    panel.updateError = null;
    try {
      await host.installUpdate();
    } catch (error) {
      panel.updateError = error instanceof Error ? error.message : String(error);
    }
    paintStrip();
    if (panel.view === "settings") paintView();
  };
  const restartButton = (key: string) => button("Restart to update", () => void installUpdate(), "secondary", key);

  /** The strip's update line as last drawn, so a repaint with nothing new keeps a focused button. */
  let paintedUpdate = "";
  function paintUpdate(): void {
    const update = panel.update;
    const key = JSON.stringify([update?.state, update?.version, panel.updateError]);
    if (key === paintedUpdate) return;
    paintedUpdate = key;
    if (panel.updateError !== null) {
      replace(
        updateSlot,
        h(
          "p",
          { class: "text", attrs: { role: "alert", "data-key": "strip-update-error" } },
          h("span", { class: "error-subject" }, "The update didn't install. "),
          panel.updateError,
        ),
      );
    } else if (update?.state === "ready" && update.version !== null) {
      replace(updateSlot, h("div", { class: "notice-body" }, h("span", {}, `MesoVibe ${update.version} is ready.`), restartButton("strip-update-restart")));
    } else if (update?.state === "installing" && update.version !== null) {
      replace(updateSlot, h("div", { class: "notice-body" }, h("span", {}, `Restarting into MesoVibe ${update.version}…`)));
    } else {
      replace(updateSlot);
    }
  }

  const setStartup = async (on: boolean) => {
    try {
      panel.startup = await host.setStartup(on);
      panel.startupError = null;
    } catch (error) {
      panel.startupError = error instanceof Error ? error.message : String(error);
    }
    if (panel.view === "settings") paintView();
  };

  const setDisplay = async (change: Partial<Display>) => {
    const s = panel.settings;
    panel.settings = await host.setDisplay({ dock: s.dock, miniHud: s.miniHud, corner: s.miniHudCorner, ...change });
    if (panel.view === "settings") paintView();
  };

  function show(name: ViewName): void {
    if (panel.view === "sees" && name !== "sees") void host.watch(false);
    if (name === "sees" && panel.view !== "sees") void host.watch(true);
    panel.view = name;
    if (name === "chat") panel.unread[panel.chatTab] = 0;
    for (const [n, item] of railButtons) item.setAttribute("aria-current", n === name ? "page" : "false");
    paintRail();
    paintView();
    if (name === "chat") view.scrollTop = view.scrollHeight;
    view.focus({ preventScroll: true });
  }

  /** The Chat rail item's unread count: a number and words, never color alone. */
  function paintRail(): void {
    const total = CHANNELS.reduce((sum, c) => sum + panel.unread[c], 0);
    chatBadge.textContent = total === 0 ? "" : total > 99 ? "99+" : String(total);
    chatBadge.hidden = total === 0;
    const label = total === 0 ? "Chat" : `Chat, ${total} unread`;
    const chat = railButtons.get("chat");
    chat?.setAttribute("aria-label", label);
    chat?.setAttribute("title", label);
  }

  /**
   * New chat lines: kept, and counted as unread unless their tab is open. A line already kept (each
   * has its own id) comes again once it's read whole: its picture takes the old one's place, and
   * it isn't new.
   */
  function addChat(fresh: ChatLine[]): void {
    const known = new Map(panel.chat.map((line, at) => [line.id, at]));
    const lines = fresh.filter((line) => !known.has(line.id));
    const redrawn = fresh.filter((line) => known.has(line.id));
    if (redrawn.length > 0) {
      panel.chat = panel.chat.slice();
      for (const line of redrawn) panel.chat[known.get(line.id) as number] = line;
    }
    if (lines.length === 0) {
      if (redrawn.length > 0 && panel.view === "chat") paintChat();
      return;
    }
    panel.chat = [...panel.chat, ...lines].slice(-KEEP_LINES);
    for (const line of lines) {
      if (line.continues) continue;
      if (panel.view !== "chat" || panel.chatTab !== line.channel) panel.unread[line.channel] += 1;
    }
    paintRail();
    if (panel.view === "chat") paintChat();
  }

  /** Repaints the Chat view, staying at the newest line if the player was there. */
  function paintChat(): void {
    const atBottom = view.scrollTop + view.clientHeight >= view.scrollHeight - 8;
    const top = view.scrollTop;
    paintView();
    view.scrollTop = atBottom ? view.scrollHeight : top;
  }

  function paintStrip(): void {
    const line = statusLine(panel);
    dot.className = `dot ${line.tone}`;
    statusText.textContent = line.text;

    const status = panel.status;
    const blocked = !panel.settings.safeMode && status?.state === "blocked";
    // Statuses arrive four times a second; rebuild the help row only when it changes, so a focused
    // button stays the same element and screen readers aren't told about it again.
    const helpKey = JSON.stringify([
      panel.settings.safeMode,
      blocked,
      panel.helpOpen,
      blocked ? (status?.error ?? null) : null,
      // The macOS "Open System Settings" button depends on it, and app info can arrive late.
      panel.info?.os ?? null,
    ]);
    if (helpKey !== paintedHelp) {
      paintedHelp = helpKey;
      const helpHadFocus = helpSlot.contains(document.activeElement);
      replace(
        helpSlot,
        // Safe mode is turned on only in Settings, so a new player isn't offered it before
        // MesoVibe has had a chance to read the game; the strip offers the way back.
        panel.settings.safeMode
          ? h("div", { class: "actions" }, button("Start reading", () => void setSafeMode(false), "primary", "start-reading"))
          : blocked
            ? h(
                "div",
                {},
                panel.helpOpen
                  ? h(
                      "div",
                      { class: "help-body" },
                      h("p", {}, status?.error ?? "Capture stopped."),
                      h(
                        "div",
                        { class: "actions" },
                        panel.info?.os === "macos"
                          ? button("Open System Settings", () => void host.openPrivacySettings(), "primary", "help-open")
                          : null,
                        button("Try again", () => void host.retry(), "secondary", "help-retry"),
                      ),
                    )
                  : h(
                      "div",
                      { class: "actions" },
                      button(
                        "How to fix",
                        () => {
                          panel.helpOpen = true;
                          paintStrip();
                        },
                        "primary",
                        "help-how",
                      ),
                    ),
              )
            : null,
      );
      // Start reading removes itself; keep the keyboard place in the strip, as End and Reset do.
      if (helpHadFocus && !helpSlot.contains(document.activeElement)) strip.focus({ preventScroll: true });
    }

    const summary = panel.summary;
    const rate = summary?.expPerHour ?? null;
    const stale = panel.lastReadAt !== null && (status?.atMs ?? Date.now()) - panel.lastReadAt > STALE_MS;
    strip.classList.toggle("dim", stale);
    const heroBox = hero.parentElement as HTMLElement;
    if (rate === null) {
      // Before the first gain the strip shows only the level and its bar; the rate takes its place
      // once there's EXP to measure.
      heroBox.hidden = true;
      const capturing = !panel.settings.safeMode && status?.state === "capturing";
      const gained = (summary?.expGained ?? 0) > 0;
      heroEmpty.textContent = summary?.lootOnly
        ? "Mesos only so far"
        : capturing && gained
          ? "Your rate appears after a few seconds of play."
          : "";
    } else {
      hero.textContent = formatCompact(rate);
      hero.title = `${formatDigits(rate)} EXP per hour`;
      heroBox.hidden = false;
      heroEmpty.textContent = "";
      if (!panel.rateShown) {
        panel.rateShown = true;
        heroBox.classList.add("arrive");
      }
    }
    // End and Reset as the tracker offers them (nothing to end or reset before a gain or a
    // pickup), and none while capture is blocked or the tracker has crashed, when a press could
    // not reach it.
    resetButton.hidden = !summary?.canReset;
    endButton.hidden = !summary?.canEnd;
    sessionActions.hidden =
      panel.settings.safeMode || blocked || panel.crashed !== null || !(summary?.canReset || summary?.canEnd);

    const ago = stale && panel.lastReadAt !== null ? ` · last read ${Math.round(((status?.atMs ?? Date.now()) - panel.lastReadAt) / 1000)}s ago` : "";
    if (summary?.level != null) {
      const minutes = summary.minutesToLevel;
      toLevel.textContent =
        (minutes === null ? `Lv ${summary.level}` : `Lv ${summary.level} → ${summary.level + 1} in ${formatMinutes(minutes)}`) + ago;
    } else {
      toLevel.textContent = ago.replace(/^ · /, "");
    }
    if (summary?.pctHundredths != null) {
      replace(
        progressSlot,
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
              "aria-valuetext": formatPercent(summary.pctHundredths),
            },
          },
          fill(summary.pctHundredths / 10_000),
        ),
        h("span", { class: "number" }, formatPercent(summary.pctHundredths)),
      );
    } else {
      replace(progressSlot);
    }

    const notice = panel.notice;
    const undoHadFocus = noticeSlot.contains(document.activeElement);
    // Rebuilt only when the notice changes, so a focused Undo stays put.
    if (notice !== paintedNotice) {
      paintedNotice = notice;
      replace(noticeSlot, notice === null ? null : noticeBody(notice));
    }
    // Focus follows End and Reset to their own notice's Undo, and back to the strip when that notice
    // goes, so keyboard and screen-reader users never lose their place. Only the matching notice,
    // and only soon after the press, may take focus.
    const undoButton = noticeSlot.querySelector<HTMLElement>('[data-key="notice-undo"]');
    if (focusUndo !== null) {
      const fresh = Date.now() <= focusUndo.until;
      if (fresh && undoButton !== null && notice?.undo === focusUndo.undo) {
        undoButton.focus();
        focusUndo = null;
      } else if (!fresh) {
        focusUndo = null;
      }
    }
    if (undoHadFocus && undoButton === null) strip.focus({ preventScroll: true });
    paintUpdate();
  }

  function paintView(): void {
    switch (panel.view) {
      case "session":
        replace(
          view,
          panel.crashed !== null
            ? h(
                "section",
                { class: "block" },
                h("p", { class: "state error", attrs: { role: "alert" } }, panel.crashed),
                button(
                  "Restart",
                  () => {
                    panel.crashed = null;
                    // The old numbers belong to the stopped tracker; the restarted one sends its own,
                    // and until then there is nothing for End or Reset to act on.
                    panel.summary = null;
                    startTracker();
                    paintStrip();
                  },
                  "primary",
                  "restart",
                ),
              )
            : null,
          ...(panel.crashed === null ? renderView(tracker.view, (action) => tracker.action(action), tracker.pictures) : []),
        );
        break;
      case "chat":
        replace(view, ...chatView());
        break;
      case "sees":
        replace(view, ...seesView());
        break;
      case "settings":
        replace(view, ...settingsView());
        break;
    }
  }

  /** What's wrong with reading chat right now, if anything, in words the player can act on. */
  function chatNote(): string | null {
    const status = panel.status;
    if (panel.settings.safeMode || status?.reading === false) return "MesoVibe isn't reading the game, so no new chat comes in.";
    if (status === null || status.state !== "capturing") return null;
    switch (status.chat) {
      case "not_found":
        return "Can't see the chat box. Is it hidden or covered?";
      case "scrolled_back":
        return "You've scrolled back in the game's chat. New lines come in again once it's back at the bottom.";
      case "unsupported":
        return "MesoVibe can't read the chat at this game size yet.";
      default:
        return null;
    }
  }

  /** Changes the chat overlay's settings: on or off, its chat types, their colors or where it sits. */
  function setChatOverlay(change: { on?: boolean; channels?: ChatChannels; colors?: ChatColors; position?: ChatPosition }): void {
    const s = panel.settings;
    void host
      .setChatOverlay(
        change.on ?? s.chatOverlay,
        change.channels ?? s.chatOverlayChannels,
        change.colors ?? s.chatOverlayColors,
        change.position ?? s.chatOverlayPosition,
      )
      .then((settings) => {
        panel.settings = settings;
        if (panel.view === "chat") paintChat();
      });
  }

  function chatView(): Node[] {
    const tab = panel.chatTab;
    const shown = messages(panel.chat.filter((line) => line.channel === tab));
    const note = chatNote();
    const tabs = h(
      "div",
      { class: "chat-tabs", attrs: { role: "tablist", "aria-label": "Chat types" } },
      ...CHANNELS.map((channel) => {
        const unread = panel.unread[channel];
        return h(
          "button",
          {
            class: "chat-tab",
            attrs: {
              type: "button",
              role: "tab",
              "aria-selected": channel === tab ? "true" : "false",
              "data-key": `chat-tab-${channel}`,
              "aria-label": unread === 0 ? TAB_NAME[channel] : `${TAB_NAME[channel]}, ${unread} unread`,
            },
            on: {
              click: () => {
                panel.chatTab = channel;
                panel.unread[channel] = 0;
                paintRail();
                paintView();
                view.scrollTop = view.scrollHeight;
              },
            },
          },
          TAB_NAME[channel],
          unread === 0 ? null : h("span", { class: "chat-count number", attrs: { "aria-hidden": "true" } }, String(unread)),
        );
      }),
    );
    const items: Node[] = [];
    for (const message of shown) {
      if (message.gapBefore) items.push(h("li", { class: "chat-gap" }, "Some lines may be missing here."));
      items.push(
        h("li", { class: "chat-message" }, h("span", { class: "chat-time number" }, clockTime(message.atMs)), messageText(message)),
      );
    }
    const s = panel.settings;
    const pick = (channel: ChatChannel) =>
      h(
        "button",
        {
          class: "segment",
          attrs: {
            type: "button",
            "aria-pressed": s.chatOverlayChannels[channel] ? "true" : "false",
            "data-key": `chat-overlay-${channel}`,
          },
          on: {
            click: () => setChatOverlay({ channels: { ...s.chatOverlayChannels, [channel]: !s.chatOverlayChannels[channel] } }),
          },
        },
        // A check as well as the pressed look: several can be on at once.
        s.chatOverlayChannels[channel] ? `✓ ${TAB_NAME[channel]}` : TAB_NAME[channel],
      );
    return [
      h(
        "section",
        { class: "block", attrs: { "aria-label": "Chat" } },
        tabs,
        note === null ? null : h("p", { class: "state", attrs: { role: "status" } }, note),
        shown.length === 0
          ? h("p", { class: "state empty" }, `${NO_LINES[tab]} Whispers, party and friend chat land here, away from megaphones.`)
          : h("ol", { class: "chat-log", attrs: { "aria-label": `${CHANNEL_NAME[tab]} messages` } }, ...items),
      ),
      h(
        "section",
        { class: "block" },
        h("h2", { class: "label" }, "Over the game"),
        toggle("Show chat over the game", s.chatOverlay, (on) => setChatOverlay({ on }), "chat-overlay"),
        s.chatOverlay ? positionPicker() : null,
        s.chatOverlay
          ? h("div", { class: "segmented chat-picks", attrs: { role: "group", "aria-label": "Chat over the game" } }, ...CHANNELS.map(pick))
          : null,
        s.chatOverlay ? h("div", { class: "chat-colors" }, ...CHANNELS.map(colorRow)) : null,
      ),
    ];
  }

  /** Where chat sits over the game: on the game's chat box, or in a box at either side. */
  function positionPicker(): Node {
    const s = panel.settings;
    return h(
      "div",
      { class: "segmented chat-positions", attrs: { role: "radiogroup", "aria-label": "Where chat sits over the game" } },
      ...CHAT_POSITIONS.map(([position, label]) => {
        const hook = `chat-position-${position}`;
        return h(
          "button",
          {
            class: "segment",
            attrs: {
              type: "button",
              role: "radio",
              "aria-checked": s.chatOverlayPosition === position ? "true" : "false",
              "data-key": hook,
            },
            on: { click: () => setChatOverlay({ position }) },
          },
          label,
        );
      }),
    );
  }

  /** A chat type's color over the game: one swatch per color, named. */
  function colorRow(channel: ChatChannel): Node {
    const s = panel.settings;
    const chosen = CHAT_COLORS.find(([hex]) => hex === s.chatOverlayColors[channel]);
    const swatch = ([hex, name]: [string, string]) => {
      const on = hex === s.chatOverlayColors[channel];
      const hook = `chat-color-${channel}-${name.toLowerCase()}`;
      const button = h(
        "button",
        {
          class: "swatch",
          attrs: {
            type: "button",
            role: "radio",
            "aria-checked": on ? "true" : "false",
            "aria-label": name,
            title: name,
            "data-key": hook,
          },
          on: { click: () => setChatOverlay({ colors: { ...s.chatOverlayColors, [channel]: hex } }) },
        },
        on ? "✓" : null,
      );
      // CSSOM, not a style attribute, so the strict CSP holds.
      button.style.backgroundColor = hex;
      return button;
    };
    return h(
      "div",
      { class: "chat-color" },
      h("span", { class: "chat-color-name" }, TAB_NAME[channel], chosen === undefined ? null : h("span", { class: "muted" }, ` · ${chosen[1]}`)),
      h("div", { class: "swatches", attrs: { role: "radiogroup", "aria-label": `${TAB_NAME[channel]} color` } }, ...CHAT_COLORS.map(swatch)),
    );
  }

  /** What What MesoVibe sees says about the pickup notices, in words the player can act on. */
  function noticesLine(status: Status): string {
    switch (status.notices) {
      case "off":
        return "Not reading: capture is paused.";
      case "learning":
        return "Learning the pickup text from your EXP lines.";
      case "covered":
        return "Covered for a moment: a few pickups may be missing. Nothing to do.";
      case "blocked":
        return "Covered by the party list.";
      case "not_found":
        return "Notices not found while EXP went up. Keep the bottom of the game window in view.";
      case "reading": {
        const waiting = status.waitingMesos;
        if (waiting === 0) return "Reading";
        return `Reading · ${waiting} ${waiting === 1 ? "amount waits" : "amounts wait"} for a digit to be learned`;
      }
    }
  }

  /** The map's name as MesoVibe last read it, or why it can't. */
  function mapLine(status: Status): Node {
    if (status.map === null) return h("span", {}, "Looking for the minimap.");
    const key = status.map.key;
    if (key === null) return h("span", {}, "Minimap not found. Keep the minimap open and in view.");
    tracker.pictures.want([key]);
    return namePicture({ gameText: key, label: "The map's name, as MesoVibe read it" }, tracker.pictures);
  }

  function seesView(): Node[] {
    const status = panel.status;
    const reading = status?.reading === true && !panel.settings.safeMode;
    let box: string;
    if (!reading) box = "Not reading: Safe mode is on.";
    else if (status?.exp) {
      const r = status.exp;
      const level = r.level === null ? "Level ?" : `Level ${r.level}`;
      box = `✓ ${level} · ${formatDigits(r.exp)} EXP · ${formatPercent(r.pctHundredths)} · ${r.confirmed ? "sure" : "checking"}`;
    } else if (status?.unreadable === "learning" || status?.unreadable === "not_calibrated") box = "Learning the digits from your EXP gains.";
    else if (status?.unreadable) box = `Can't read (${status.unreadable.replaceAll("_", " ")}).`;
    else box = "Waiting for a picture.";
    const game = status?.game;
    return [
      h(
        "section",
        { class: "block" },
        h("h2", { class: "label" }, "What MesoVibe sees"),
        panel.snapshot !== null && reading
          ? h("img", { class: "thumbnail", attrs: { src: panel.snapshot, alt: "The part of the game window MesoVibe read last" } })
          : h("p", { class: "state empty" }, "No picture yet. It appears while MesoVibe is reading the game."),
        h("p", { class: "text muted" }, "The exact rows MesoVibe reads, at the game's own size. This picture stays on this computer."),
        h("div", { class: "reading-box" }, h("span", { class: "label" }, "EXP bar"), h("span", { class: "number" }, box)),
        reading && status ? h("div", { class: "reading-box", attrs: { "data-key": "sees-notices" } }, h("span", { class: "label" }, "Pickup notices"), h("span", {}, noticesLine(status))) : null,
        reading && status ? h("div", { class: "reading-box", attrs: { "data-key": "sees-map" } }, h("span", { class: "label" }, "Map name"), mapLine(status)) : null,
        h(
          "dl",
          { class: "key-value" },
          h("dt", {}, "Game area"),
          h(
            "dd",
            { class: "number" },
            game ? `${game.width}×${game.height}${game.scale > 1 ? `, Retina ${game.scale}×` : ""}` : "?",
          ),
          h("dt", {}, "How often"),
          h(
            "dd",
            {},
            "Up to 4 times a second, the bottom strip. The whole game picture after a map change and every 30 seconds, for the map's name.",
          ),
        ),
      ),
      h(
        "section",
        { class: "block" },
        h("h2", { class: "label" }, "Reads"),
        h("p", {}, "The EXP bar: your level, EXP and percent."),
        h("p", {}, "The pickup notices at the bottom right: the EXP, mesos and items you pick up."),
        h("p", {}, "The minimap's map name, as a picture."),
        h("p", {}, "The chat box: whisper, party and friend lines, as pictures."),
        h("h2", { class: "label" }, "Never reads"),
        h("p", {}, "Your keyboard or mouse · the game's memory · the game's files · network traffic"),
        // Safe mode is turned on only in Settings; here, only the way back.
        panel.settings.safeMode ? button("Start reading", () => void setSafeMode(false), "primary", "sees-start") : null,
      ),
    ];
  }

  function displaySettings(): Node {
    const s = panel.settings;
    return h(
      "section",
      { class: "block" },
      h("h2", { class: "label" }, "Display"),
      hinted(
        toggle("Keep MesoVibe with the game", s.dock, (on) => void setDisplay({ dock: on }), "settings-dock"),
        "With room beside the game window, this panel sits against it and follows it when it moves.",
        "settings-dock-hint",
      ),
      hinted(
        toggle("Mini-HUD when there's no room", s.miniHud, (on) => void setDisplay({ miniHud: on }), "settings-hud"),
        "A small card in a corner of the game with your EXP/hr and time to level. Clicks go through it to the game. Off, MesoVibe never draws over the game.",
        "settings-hud-hint",
      ),
      s.dock && s.miniHud
        ? h(
            "div",
            { class: "segmented", attrs: { role: "radiogroup", "aria-label": "Mini-HUD corner" } },
            ...CORNERS.map(([corner, label]) =>
              h(
                "button",
                {
                  class: "segment",
                  attrs: {
                    type: "button",
                    role: "radio",
                    "aria-checked": s.miniHudCorner === corner ? "true" : "false",
                    "data-key": `corner-${corner}`,
                  },
                  on: { click: () => void setDisplay({ corner }) },
                },
                label,
              ),
            ),
          )
        : null,
    );
  }

  function startupSettings(): Node {
    const startup = panel.startup;
    const os = panel.info?.os;
    return h(
      "div",
      { class: "startup" },
      hinted(
        toggle(
          "Open with MapleStory Classic",
          startup?.on ?? false,
          (on) => void setStartup(on),
          "settings-startup",
          startup === null || (startup.problem !== null && !startup.on),
        ),
        `${startupNote(os)} Off, closing the panel quits MesoVibe.`,
        "settings-startup-note",
      ),
      // Why the switch can't be turned on: something the player can act on, so it stays in view.
      startup?.problem
        ? h("p", { class: "text muted", attrs: { "data-key": "settings-startup-problem" } }, startup.problem)
        : null,
      // Safe mode reads nothing, not even whether the game is open.
      panel.settings.safeMode && startup?.on
        ? h(
            "p",
            { class: "text muted", attrs: { "data-key": "settings-startup-safe" } },
            `In Safe mode MesoVibe can't see the game start, so open it from its ${trayName(os)} icon.`,
          )
        : null,
      panel.startupError !== null
        ? h(
            "p",
            { class: "text", attrs: { role: "alert", "data-key": "settings-startup-error" } },
            h("span", { class: "error-subject" }, "Open with MapleStory Classic didn't change. "),
            panel.startupError,
          )
        : null,
    );
  }

  /** Settings > About's update line, in words for each state (app/src/updater.rs). */
  function updateWords(update: UpdateStatus): Node {
    const checked = update.checkedAtMs === null ? "" : ` Last checked at ${clockTime(update.checkedAtMs)}.`;
    switch (update.state) {
      case "idle":
        return document.createTextNode("MesoVibe looks for updates shortly after it starts.");
      case "checking":
        return document.createTextNode("Checking for updates…");
      case "upToDate":
        return document.createTextNode(`MesoVibe is up to date.${checked}`);
      case "downloading":
        return document.createTextNode(`Downloading MesoVibe ${update.version ?? ""}…`);
      case "ready":
        return document.createTextNode(`MesoVibe ${update.version ?? ""} is ready to install.`);
      case "installing":
        return document.createTextNode(`Restarting into MesoVibe ${update.version ?? ""}…`);
      case "failed":
        return h("span", {}, h("span", { class: "error-subject" }, "Update check failed. "), update.error ?? "", checked);
      case "off":
        return document.createTextNode("Updates are off in development builds.");
    }
  }

  function updateSettings(): Node {
    const update = panel.update;
    const busy = update === null || ["checking", "downloading", "installing", "off"].includes(update.state);
    const check = button(
      "Check now",
      () => {
        if (panel.update !== null) panel.update = { ...panel.update, state: "checking" };
        paintView();
        void host.checkForUpdates();
      },
      "secondary",
      "settings-update-check",
    );
    check.disabled = busy;
    return hinted(
      h(
        "div",
        { class: "update-settings" },
        h("p", { class: "text", attrs: { "data-key": "settings-update-status" } }, update === null ? "…" : updateWords(update)),
        h("div", { class: "actions" }, check, update?.state === "ready" ? restartButton("settings-update-restart") : null),
      ),
      UPDATE_CHECK_NOTE,
      "settings-update-note",
    );
  }

  function settingsView(): Node[] {
    const info = panel.info;
    return [
      h(
        "section",
        { class: "block" },
        h("h2", { class: "label" }, "Reading"),
        hinted(
          toggle("Safe mode", panel.settings.safeMode, (on) => void setSafeMode(on), "settings-safe"),
          "Safe mode turns capture off. MesoVibe then reads nothing at all.",
          "settings-safe-hint",
        ),
        startupSettings(),
      ),
      displaySettings(),
      h(
        "section",
        { class: "block" },
        h("h2", { class: "label" }, "Privacy"),
        h("p", {}, "Everything MesoVibe keeps stays on this computer."),
        h(
          "button",
          {
            class: "danger-link",
            attrs: { type: "button", "data-key": "settings-delete" },
            on: {
              click: () =>
                confirmDialog(
                  {
                    title: "Delete all session history?",
                    deletes: ["Every past session on this computer: EXP gained, time played, rates and mesos, and the All time totals."],
                    keeps: ["The session running now, your settings, item and map name pictures, and everything outside the Session Tracker."],
                    confirmLabel: "Delete session history",
                  },
                  () => tracker.action("delete-history"),
                ),
            },
          },
          "Delete all session history…",
        ),
      ),
      h(
        "section",
        { class: "block" },
        h("h2", { class: "label" }, "About"),
        h(
          "dl",
          { class: "key-value" },
          h("dt", {}, "Version"),
          h("dd", { class: "number" }, info?.version ?? "?"),
          h("dt", {}, "Event schema"),
          h("dd", { class: "number" }, info ? String(info.eventSchema) : "?"),
        ),
        updateSettings(),
        h("p", { class: "text muted" }, "Open source (BSD-2-Clause): github.com/danielfang7/MesoVibe"),
        h(
          "p",
          { class: "text muted" },
          "Fonts: Atkinson Hyperlegible Next and Mono, and Pixelify Sans, under the SIL Open Font License 1.1.",
        ),
        h("p", { class: "text muted" }, "Not affiliated with or endorsed by Nexon. MapleStory is a trademark of Nexon."),
      ),
    ];
  }

  /** The Chat view's note as last drawn. */
  let paintedChatNote: string | null | undefined;
  const dark = darkWatch();
  const fullScreen = fullScreenWatch();
  void host.onStatus((status) => {
    panel.status = status;
    panel.longDark = dark(status);
    panel.fullScreenUnread = fullScreen(status);
    if (status.exp?.confirmed) panel.lastReadAt = status.atMs;
    if (status.state !== "blocked") panel.helpOpen = false;
    tracker.setCapture(captureState(status));
    tracker.setReaders({ notices: status.notices, waitingMesos: status.waitingMesos });
    if (!trackerHasMap && status.map !== null) {
      trackerHasMap = true;
      tracker.event({ type: "MapChanged", map_key: status.map.key }, status.atMs);
    }
    const bar = status.exp;
    if (!trackerHasBar && bar?.confirmed) {
      trackerHasBar = true;
      tracker.event(
        { type: "ExpChanged", level: bar.level, exp: bar.exp, pct_hundredths: bar.pctHundredths, needed: bar.needed },
        status.atMs,
      );
    }
    paintStrip();
    if (panel.view === "chat") {
      // Statuses arrive four times a second; the Chat view repaints only when its note changes.
      const note = chatNote();
      if (note !== paintedChatNote) {
        paintedChatNote = note;
        paintChat();
      }
      // Of the other views, only What MesoVibe sees shows the status. Settings isn't rebuilt
      // under the pointer four times a second, which would drop the hint it's showing.
    } else if (panel.view === "sees") paintView();
  });
  void host.onChat(addChat);
  void host.chatHistory().then((lines) => {
    // Lines that came in meanwhile are already kept; the history goes before them.
    const known = new Set(panel.chat.map((line) => line.id));
    panel.chat = [...lines.filter((line) => !known.has(line.id)), ...panel.chat].slice(-KEEP_LINES);
    if (panel.view === "chat") paintChat();
  });
  void host.onGameEvent((event, atMs) => {
    if (event.type === "ExpChanged") trackerHasBar = true;
    if (event.type === "MapChanged") trackerHasMap = true;
    tracker.event(event, atMs);
  });
  void host.onSnapshot((png) => {
    panel.snapshot = png;
    if (panel.view === "sees") paintView();
  });
  window.setInterval(() => {
    if (panel.notice !== null && Date.now() > panel.notice.until) {
      panel.notice = null;
      paintStrip();
    }
  }, 1000);
  void host.appInfo().then((info) => {
    panel.info = info;
    paintStrip();
    if (panel.view === "settings") paintView();
  });
  void host.startup().then((startup) => {
    panel.startup = startup;
    if (panel.view === "settings") paintView();
  });
  const setUpdate = (update: UpdateStatus) => {
    panel.update = update;
    paintStrip();
    if (panel.view === "settings") paintView();
  };
  void host.onUpdateStatus(setUpdate);
  void host.updateStatus().then(setUpdate);

  show("session");
  paintStrip();
}

async function boot(): Promise<void> {
  try {
    host = await connect();
    shell(await host.settings());
  } catch (error) {
    replace(root, h("p", { class: "state error" }, `MesoVibe couldn't start its panel: ${String(error)}`));
  }
}

append(root);
void boot();
