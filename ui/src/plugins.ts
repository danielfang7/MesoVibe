// The plugin host: each first-party plugin runs in its own Web Worker with no DOM, no IPC
// and no network, and talks to the panel only through the SDK's messages. The host draws the
// plugin's components as text, and saves the plugin's data through the host's store.
import type {
  CaptureState,
  Cell,
  Event as GameEvent,
  PluginManifest,
  Readers,
  SessionSummary,
  ToHost,
  ToPlugin,
  ViewNode,
} from "@mesovibe/sdk";
import type { Confirm } from "@mesovibe/sdk";

import { h } from "./dom.js";
import type { GamePicture, Host } from "./ipc.js";

/** CSS pixels to a game pixel for name pictures: the game's letters are small (design: 2x). */
const NAME_SCALE = 2;
/** How long a name with no picture kept waits before it's asked for again. */
const ASK_AGAIN_MS = 5_000;

/**
 * Name pictures by key, asked of the host as views need them. `undefined` while asking, `null`
 * when the host kept none.
 */
export class NamePictures {
  private readonly kept = new Map<string, { picture: GamePicture | null; askedAt: number }>();
  private readonly asking = new Set<string>();

  constructor(
    private readonly host: Pick<Host, "gameText">,
    private readonly arrived: () => void,
  ) {}

  get(key: string): GamePicture | null | undefined {
    return this.kept.get(key)?.picture;
  }

  /** Asks for the pictures of `keys` not kept yet, and again for those the host had none for. */
  want(keys: Iterable<string>): void {
    const now = Date.now();
    const missing = [...new Set(keys)].filter((key) => {
      if (this.asking.has(key)) return false;
      const kept = this.kept.get(key);
      return kept === undefined || (kept.picture === null && now - kept.askedAt > ASK_AGAIN_MS);
    });
    if (missing.length === 0) return;
    for (const key of missing) this.asking.add(key);
    void this.host
      .gameText(missing)
      .catch(() => ({}) as Record<string, GamePicture>)
      .then((found) => {
        for (const key of missing) {
          this.asking.delete(key);
          this.kept.set(key, { picture: found[key] ?? null, askedAt: Date.now() });
        }
        this.arrived();
      });
  }
}

/** The name-picture keys a view shows. */
export function gameTextKeys(nodes: ViewNode[]): string[] {
  const keys: string[] = [];
  const cell = (c: Cell) => {
    if (c !== null && typeof c === "object") keys.push(c.gameText);
  };
  for (const node of nodes) {
    if (node.kind === "section" || node.kind === "row") keys.push(...gameTextKeys(node.children));
    else if (node.kind === "table") node.rows.forEach((row) => row.forEach(cell));
    else if (node.kind === "keyValue") node.items.forEach(([, v]) => cell(v));
  }
  return keys;
}

export interface PluginListener {
  render(view: ViewNode[]): void;
  summary(summary: SessionSummary | null): void;
  notice(text: string, undo?: string): void;
  crashed(message: string): void;
}

export class PluginHost {
  private worker: Worker | null = null;
  /**
   * Messages sent while the plugin starts, before its worker is there. The game's EXP bar is
   * reported only when it changes, so a reading dropped here wouldn't come again until the next
   * gain.
   */
  private waiting: ToPlugin[] | null = null;
  private ticker: number | undefined;
  manifest: PluginManifest | null = null;
  view: ViewNode[] = [];
  private readers: Readers = { notices: "off", waitingMesos: 0 };
  /** The pictures the plugin's views name, kept for drawing them. */
  readonly pictures: NamePictures;

  constructor(
    readonly id: string,
    /** The plugin's name for messages shown before it has started and said its own. */
    private readonly name: string,
    private readonly host: Host,
    private readonly listener: PluginListener,
    private capture: CaptureState,
  ) {
    this.pictures = new NamePictures(host, () => this.listener.render(this.view));
  }

  async start(): Promise<void> {
    this.stop();
    const waiting: ToPlugin[] = [];
    this.waiting = waiting;
    let saved: unknown;
    try {
      saved = await this.host.storageGet(this.id);
    } catch (error) {
      // Stopped, or started again, while the saved data loaded.
      if (this.waiting !== waiting) return;
      this.waiting = null;
      // Started with nothing, the plugin's first save would replace data that may be fine, so it
      // stays off until a read works. Restart reads again.
      const name = this.manifest?.name ?? this.name;
      const cause = error instanceof Error ? error.message : String(error);
      this.listener.crashed(
        `Couldn't read ${name}'s saved data (${cause}). It's kept as it is, and ${name} stays off so nothing is saved over it. Restart to try again.`,
      );
      return;
    }
    // Stopped, or started again, while the saved data loaded.
    if (this.waiting !== waiting) return;
    this.waiting = null;
    const worker = new Worker(`/plugins/${this.id}.js`, { type: "module", name: this.id });
    this.worker = worker;
    worker.onmessage = (message: MessageEvent<ToHost>) => this.receive(message.data);
    worker.onerror = (error) => {
      error.preventDefault();
      this.stop();
      this.listener.crashed(`${this.manifest?.name ?? this.name} stopped responding and was turned off.`);
    };
    this.post({ type: "start", saved, atMs: Date.now(), capture: this.capture, readers: this.readers });
    for (const message of waiting) this.post(message);
    this.ticker = window.setInterval(() => this.post({ type: "tick", atMs: Date.now() }), 1000);
  }

  stop(): void {
    window.clearInterval(this.ticker);
    this.worker?.terminate();
    this.worker = null;
    this.waiting = null;
  }

  event(event: GameEvent, atMs: number): void {
    this.post({ type: "event", event, atMs });
  }

  setCapture(state: CaptureState): void {
    if (state === this.capture) return;
    this.capture = state;
    this.post({ type: "capture", state, atMs: Date.now() });
  }

  /** How the pickup notices are being read; sent on to the plugin when it changes. */
  setReaders(readers: Readers): void {
    if (readers.notices === this.readers.notices && readers.waitingMesos === this.readers.waitingMesos) return;
    this.readers = readers;
    this.post({ type: "readers", readers, atMs: Date.now() });
  }

  action(action: string): void {
    this.post({ type: "action", action, atMs: Date.now() });
  }

  private post(message: ToPlugin): void {
    if (this.worker !== null) this.worker.postMessage(message);
    else this.waiting?.push(message);
  }

  private receive(message: ToHost): void {
    switch (message.type) {
      case "ready":
        this.manifest = message.manifest;
        break;
      case "render":
        this.view = message.view;
        this.pictures.want(gameTextKeys(message.view));
        this.listener.render(message.view);
        break;
      case "summary":
        this.listener.summary(message.summary);
        break;
      case "save":
        this.host.storageSet(this.id, message.data).catch((error: unknown) => {
          this.listener.notice(`Couldn't save ${this.manifest?.name ?? this.name}'s data: ${String(error)}`);
        });
        break;
      case "notice":
        this.listener.notice(message.text, message.undo);
        break;
    }
  }
}

/** Asks before an irreversible action. Cancel has the default focus. */
export function confirmDialog(confirm: Confirm, onConfirm: () => void): void {
  const dialog = h("dialog", { class: "dialog", attrs: { "aria-labelledby": "dialog-title" } });
  const cancel = h("button", { class: "button secondary", attrs: { type: "button" }, on: { click: () => dialog.close() } }, "Cancel");
  const go = h(
    "button",
    {
      class: "button danger",
      attrs: { type: "button" },
      on: {
        click: () => {
          dialog.close();
          onConfirm();
        },
      },
    },
    confirm.confirmLabel,
  );
  dialog.append(
    h("h2", { attrs: { id: "dialog-title" } }, confirm.title),
    h("p", { class: "label" }, "Deletes"),
    h("ul", {}, ...confirm.deletes.map((line) => h("li", {}, line))),
    h("p", { class: "label" }, "Keeps"),
    h("ul", {}, ...confirm.keeps.map((line) => h("li", {}, line))),
    h("p", {}, "This can't be undone."),
    h("div", { class: "dialog-actions" }, cancel, go),
  );
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  cancel.focus();
}

/** Draws a plugin's components. `act` sends a button's action back to the plugin. */
export function renderView(
  nodes: ViewNode[],
  act: (action: string) => void,
  pictures: Pick<NamePictures, "get"> | null = null,
  path = "v",
): Node[] {
  return nodes.map((node, i) => renderNode(node, act, `${path}.${i}`, pictures));
}

/**
 * A name as a picture of the game's lettering, in the text's color (a CSS mask, set through
 * CSSOM: the CSP allows no inline styles). While its picture loads, an empty box; when none is
 * kept, "Name not available".
 */
export function namePicture(cell: Exclude<Cell, string | null>, pictures: Pick<NamePictures, "get"> | null): Node {
  const picture = pictures?.get(cell.gameText);
  const suffix = cell.suffix === undefined ? null : h("span", { class: "game-text-suffix" }, cell.suffix);
  if (picture === null) {
    return h(
      "span",
      { class: "game-text-missing", attrs: { title: "This name's picture is no longer on this computer. It comes back the next time you see it." } },
      "Name not available",
      suffix,
    );
  }
  const box = h("span", {
    class: "game-text",
    attrs: { role: "img", "aria-label": cell.label ?? "A name in the game's lettering", "data-game-text": cell.gameText },
  });
  if (picture !== undefined) {
    const set = (name: string, value: string) => {
      box.style.setProperty(name, value);
      box.style.setProperty(`-webkit-${name}`, value);
    };
    box.style.width = `${picture.width * NAME_SCALE}px`;
    box.style.height = `${picture.height * NAME_SCALE}px`;
    set("mask-image", `url("${picture.png}")`);
    set("mask-size", `${picture.width * NAME_SCALE}px ${picture.height * NAME_SCALE}px`);
    set("mask-repeat", "no-repeat");
  }
  return h("span", { class: "game-text-cell" }, box, suffix);
}

/** A cell's content: its text, "?" for a value not known yet, or a name picture. */
function cellContent(cell: Cell, pictures: Pick<NamePictures, "get"> | null): Node | string {
  if (cell !== null && typeof cell === "object") return namePicture(cell, pictures);
  return value(cell);
}

function value(text: string | null): string {
  return text ?? "?";
}

function renderNode(
  node: ViewNode,
  act: (action: string) => void,
  key: string,
  pictures: Pick<NamePictures, "get"> | null,
): Node {
  switch (node.kind) {
    case "section":
      return h(
        "section",
        { class: "block" },
        h("h2", { class: "label" }, node.title),
        ...renderView(node.children, act, pictures, key),
      );
    case "row":
      return h("div", { class: "row" }, ...renderView(node.children, act, pictures, key));
    case "divider":
      return h("hr", { class: "divider" });
    case "stat":
      return h(
        "div",
        { class: `stat ${node.variant ?? ""}`.trim() },
        h("span", { class: "stat-label" }, node.label),
        h("span", { class: "stat-value number", attrs: { title: node.detail } }, value(node.value)),
      );
    case "statRow":
      return h("div", { class: "stat-row" }, ...node.stats.map((s, i) => renderNode(s, act, `${key}.${i}`, pictures)));
    case "progress": {
      const fraction = node.fraction === null ? 0 : Math.min(1, Math.max(0, node.fraction));
      return h(
        "div",
        { class: "progress-block" },
        h("div", { class: "progress-head" }, h("span", {}, node.label), h("span", { class: "number" }, node.text)),
        h(
          "div",
          {
            class: "progress",
            attrs: {
              role: "progressbar",
              "aria-label": node.label,
              "aria-valuemin": "0",
              "aria-valuemax": "100",
              "aria-valuenow": node.fraction === null ? null : String(Math.round(fraction * 100)),
              "aria-valuetext": node.text,
            },
          },
          fill(fraction),
        ),
      );
    }
    case "table": {
      const numeric = node.numeric ?? [];
      const details = node.details ?? [];
      return h(
        "table",
        { class: "table" },
        h("thead", {}, h("tr", {}, ...node.columns.map((c, i) => h("th", { class: numeric[i] ? "num" : "" }, c)))),
        h(
          "tbody",
          {},
          ...node.rows.map((row, r) =>
            h(
              "tr",
              {},
              ...row.map((cell, i) => {
                const detail = i === 0 ? details[r] : null;
                const lines = detail === null || detail === undefined ? [] : Array.isArray(detail) ? detail : [detail];
                return h(
                  "td",
                  { class: numeric[i] ? "num number" : "" },
                  cellContent(cell, pictures),
                  ...lines.filter((line) => line !== "").map((line) => h("span", { class: "cell-detail" }, line)),
                );
              }),
            ),
          ),
        ),
      );
    }
    case "keyValue":
      return h(
        "dl",
        { class: "key-value" },
        ...node.items.flatMap(([k, v]) => [
          h("dt", {}, k),
          h("dd", { class: v !== null && typeof v === "object" ? "" : "number" }, cellContent(v, pictures)),
        ]),
      );
    case "text":
      return h("p", { class: `text ${node.variant ?? ""}`.trim() }, node.text);
    case "button": {
      const press = () => (node.confirm ? confirmDialog(node.confirm, () => act(node.action)) : act(node.action));
      const style = node.style ?? "secondary";
      return h(
        "button",
        { class: style === "danger-link" ? "danger-link" : `button ${style}`, attrs: { type: "button", "data-key": key }, on: { click: press } },
        node.label,
      );
    }
    case "loading":
      return h("p", { class: "state loading", attrs: { role: "status" } }, node.text);
    case "empty":
      return h("p", { class: "state empty" }, node.text);
    case "error":
      return h("p", { class: "state error", attrs: { role: "alert" } }, node.text);
    case "partial":
      return h("p", { class: "state partial" }, node.text);
  }
}

/** A progress bar's fill. Its width is set through CSSOM: the CSP allows no inline styles. */
export function fill(fraction: number): HTMLElement {
  const bar = h("div", { class: "progress-fill" });
  bar.style.width = `${(fraction * 100).toFixed(2)}%`;
  return bar;
}
