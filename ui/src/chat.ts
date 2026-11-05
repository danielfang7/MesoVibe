// The chat overlay: the latest whisper, party and friend lines in a
// small box inside the game's right edge, for players who want them in sight while they play.
// The player turns it on and picks its chat types in the panel's Chat view; the host places it,
// keeps it click-through and never lets it take the keyboard (app/src/dock.rs). It shows nothing
// until a line arrives.
import { h, replace } from "./dom.js";
import { CHANNEL_NAME, messageText, messages } from "./chatlines.js";
import { connectChat } from "./ipc.js";
import type { ChatChannels, ChatLine } from "./ipc.js";

/** The most messages the box holds; the oldest leave first when they don't all fit. */
const SHOWN = 6;

const root = document.getElementById("chat") as HTMLElement;

function paint(lines: ChatLine[], channels: ChatChannels): void {
  const shown = messages(lines.filter((line) => channels[line.channel])).slice(-SHOWN);
  if (shown.length === 0) {
    replace(root);
    return;
  }
  const list = h(
    "ol",
    { class: "chat-overlay", attrs: { "aria-label": "Latest chat" } },
    ...shown.map((message) =>
      h("li", { class: "chat-overlay-line" }, h("span", { class: "label chat-tag" }, CHANNEL_NAME[message.channel]), messageText(message)),
    ),
  );
  replace(root, h("div", { class: "chat-frame" }, list));
  // Whole messages only: the oldest leave until the rest fit, rather than showing half of one. The
  // list keeps its newest lines at the bottom, so what doesn't fit spills past its top edge.
  const top = list.getBoundingClientRect().top;
  while (list.childElementCount > 1 && (list.firstElementChild as HTMLElement).getBoundingClientRect().top < top) {
    list.firstElementChild?.remove();
  }
}

async function boot(): Promise<void> {
  const host = await connectChat();
  let channels: ChatChannels = (await host.settings()).chatOverlayChannels;
  let lines: ChatLine[] = [];
  void host.onChat((fresh) => {
    lines = [...lines, ...fresh].slice(-200);
    paint(lines, channels);
  });
  void host.onChannels((next) => {
    channels = next;
    paint(lines, channels);
  });
  lines = [...(await host.chatHistory()), ...lines].slice(-200);
  paint(lines, channels);
}

void boot();
