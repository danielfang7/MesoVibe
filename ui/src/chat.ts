// The chat overlay: the latest whisper, party and friend lines just above the game's chat box,
// like split chat: the text at the chat's own size, in the colors the player picked, on the same
// faint dark wash as the box itself. The player turns it on and picks its chat types and colors in
// the panel's Chat view. The host sizes the window to the chat box's width and puts it on the box's
// top edge, keeps it click-through and never lets it take the keyboard (app/src/dock.rs). It shows
// nothing until a line arrives.
import { h, replace } from "./dom.js";
import { CHANNEL_NAME, clockTime, linePicture, messages } from "./chatlines.js";
import { connectChat } from "./ipc.js";
import type { ChatLine, ChatOverlayChoice } from "./ipc.js";

/**
 * How wide the game's chat box is up to its scrollbar, in game pixels (BOX_WIDTH in
 * engine/src/recognizers/chat.rs). The window is that wide on screen, which gives the game's scale.
 */
const BOX_WIDTH = 551;
/** The most messages shown; the oldest leave first, and earlier when they don't all fit. */
const SHOWN = 5;
/** Lines kept for repainting. */
const KEPT = 200;

const root = document.getElementById("chat") as HTMLElement;

let lines: ChatLine[] = [];
let choice: ChatOverlayChoice | null = null;

/** New lines, without any already kept: each line has its own id. */
function keep(fresh: ChatLine[]): void {
  const known = new Set(lines.map((line) => line.id));
  lines = [...lines, ...fresh.filter((line) => !known.has(line.id))].sort((a, b) => a.id - b.id).slice(-KEPT);
}

function paint(): void {
  if (choice === null) return;
  const { channels, colors } = choice;
  // CSS pixels per game pixel.
  const scale = window.innerWidth / BOX_WIDTH;
  document.documentElement.style.setProperty("--game-px", `${scale}px`);
  const shown = messages(lines.filter((line) => channels[line.channel])).slice(-SHOWN);
  if (shown.length === 0) {
    replace(root);
    return;
  }
  const list = h(
    "ol",
    { class: "split-chat", attrs: { "aria-label": "Latest chat" } },
    ...shown.map((message) => {
      const item = h(
        "li",
        {
          class: "split-message",
          attrs: {
            role: "img",
            "aria-label": `${CHANNEL_NAME[message.channel]} message at ${clockTime(message.atMs)}, a picture of the game's text`,
          },
        },
        // One row per line, as the game draws them: the rest of a long line is indented.
        ...message.lines.map((line) => h("span", { class: line.continues ? "split-line rest" : "split-line" }, linePicture(line, scale))),
      );
      item.style.color = colors[message.channel];
      return item;
    }),
  );
  replace(root, list);
  // Whole messages only: the oldest leave until the rest fit, rather than showing half of one. The
  // list keeps its newest lines at the bottom, so what doesn't fit spills past the top of its
  // content, into the room above the top line and beyond. Layout rounds to fractions of a pixel,
  // so a message counts as spilling only when it's half a pixel over.
  const top = list.getBoundingClientRect().top + parseFloat(getComputedStyle(list).paddingTop) - 0.5;
  while (list.childElementCount > 1 && (list.firstElementChild as HTMLElement).getBoundingClientRect().top < top) {
    list.firstElementChild?.remove();
  }
}

async function boot(): Promise<void> {
  const host = await connectChat();
  // Listening first and keeping lines by id, so a line that arrives while the history loads is
  // neither lost nor shown twice.
  void host.onChat((fresh) => {
    keep(fresh);
    paint();
  });
  void host.onChoice((next) => {
    choice = next;
    paint();
  });
  window.addEventListener("resize", paint);
  const [settings, history] = await Promise.all([host.settings(), host.chatHistory()]);
  choice ??= { channels: settings.chatOverlayChannels, colors: settings.chatOverlayColors };
  keep(history);
  paint();
}

void boot();
