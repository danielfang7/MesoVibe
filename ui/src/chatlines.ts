// Chat Hub lines: each whisper, party or friend line is a picture of
// the game's text, not text, so nothing is ever misread. In the panel a picture is cut into its
// words, and the words wrap like text in a window narrower than the line; the chat overlay shows
// whole lines at the game's own size. The text takes the window's own color (a CSS mask), never
// the game's colors.
import { h } from "./dom.js";
import type { ChatChannel, ChatLine } from "./ipc.js";

/** The line pictures come at the game's own size; this brings the text near 16px body text. */
export const CHAT_SCALE = 1.5;
/** The room added between two words, in the picture's pixels: with each word's one-pixel margins, about the game's own space. */
const WORD_GAP = 2;

export const CHANNEL_NAME: Record<ChatChannel, string> = {
  whisper: "Whisper",
  party: "Party",
  friend: "Friend",
};

/** A line and the indented rest of it that arrived with it. */
export interface ChatMessage {
  channel: ChatChannel;
  atMs: number;
  /** Lines may be missing between this message and the one before it in the same list. */
  gapBefore: boolean;
  lines: ChatLine[];
}

/** `lines` (oldest first) as messages: each indented rest joins the line before it. */
export function messages(lines: ChatLine[]): ChatMessage[] {
  const out: ChatMessage[] = [];
  let lastGaps: number | null = null;
  for (const line of lines) {
    const last = out[out.length - 1];
    if (line.continues && last !== undefined && last.channel === line.channel) {
      last.lines.push(line);
    } else {
      out.push({ channel: line.channel, atMs: line.atMs, gapBefore: lastGaps !== null && line.gaps !== lastGaps, lines: [line] });
    }
    lastGaps = line.gaps;
  }
  return out;
}

export function clockTime(atMs: number): string {
  return new Date(atMs).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

/**
 * Shows columns `from` to `to` of `line`'s picture in `element`, `scale` CSS pixels to a picture
 * pixel. CSSOM properties, not a style attribute, so the strict CSP holds.
 */
function showPicture(element: HTMLElement, line: ChatLine, scale: number, from: number, to: number): void {
  const set = (name: string, value: string) => {
    element.style.setProperty(name, value);
    element.style.setProperty(`-webkit-${name}`, value);
  };
  element.style.width = `${(to - from) * scale}px`;
  element.style.height = `${line.height * scale}px`;
  set("mask-image", `url("${line.png}")`);
  set("mask-size", `${line.width * scale}px ${line.height * scale}px`);
  set("mask-position", `${-from * scale}px 0`);
  set("mask-repeat", "no-repeat");
}

/** A line's whole picture, `scale` CSS pixels to a game pixel, colored by the surrounding text color. */
export function linePicture(line: ChatLine, scale: number): HTMLElement {
  const picture = h("span", { class: "chat-picture" });
  showPicture(picture, line, scale, 0, line.width);
  return picture;
}

/** The message's words as pieces of its pictures, colored by the surrounding text color. */
export function messageText(message: ChatMessage): HTMLElement {
  const box = h("span", {
    class: "chat-text",
    attrs: {
      role: "img",
      "aria-label": `${CHANNEL_NAME[message.channel]} message at ${clockTime(message.atMs)}, a picture of the game's text`,
    },
  });
  box.style.setProperty("column-gap", `${WORD_GAP * CHAT_SCALE}px`);
  for (const line of message.lines) {
    for (const [from, to] of line.words) {
      const word = h("span", { class: "chat-word" });
      showPicture(word, line, CHAT_SCALE, from, to);
      box.appendChild(word);
    }
  }
  return box;
}
