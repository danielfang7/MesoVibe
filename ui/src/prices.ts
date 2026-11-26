// The Prices tab: Free Market prices from Cozy Markets, found by item name. The app downloads the
// prices and answers every search (app/src/prices.rs); this file only draws the answers.
// Every word on screen is a text node, never parsed HTML.
import { formatCompact, formatDigits } from "@mesovibe/sdk";

import { clockTime } from "./chatlines.js";
import { h, replace } from "./dom.js";
import type { Host, PriceDetail, PriceMovers, PriceRow, PriceStatus, World } from "./ipc.js";

export const WORLDS: [World, string][] = [
  ["windia", "Windia"],
  ["zenith", "Zenith"],
];
const WORLD_NAME: Record<World, string> = { windia: "Windia", zenith: "Zenith" };

const HOUR_MS = 60 * 60 * 1000;
/** Prices older than this (two missed Cozy updates) turn muted and say how old they are. */
export const STALE_MS = 12 * HOUR_MS;
/** How long "Recently checked cleared." keeps its Undo. */
const UNDO_MS = 10_000;
/** Items Recently checked keeps, like the host. */
const RECENT_MAX = 8;
/** How often the tab looks again at how old its prices are, so they turn muted while it's open. */
const AGE_CHECK_MS = 60_000;

/** A price as the panel writes it: "21M", "1.5M", "950", and "under 1" for a fraction of a meso. */
export function priceText(n: number): string {
  return n > 0 && n < 1 ? "under 1" : formatCompact(n);
}

/** A weekly change, shown with an arrow and read aloud in words, so color is never the only sign. */
export function moveOf(change: number | null): { shown: string; spoken: string; tone: "up" | "down" | "flat" | "none" } {
  if (change === null) return { shown: "—", spoken: "no weekly change yet", tone: "none" };
  const pct = Math.round(change * 100);
  if (pct > 0) return { shown: `▲ ${pct}%`, spoken: `up ${pct}% this week`, tone: "up" };
  if (pct < 0) return { shown: `▼ ${-pct}%`, spoken: `down ${-pct}% this week`, tone: "down" };
  return { shown: "0%", spoken: "no change this week", tone: "flat" };
}

function move(change: number | null): HTMLElement {
  const m = moveOf(change);
  return h(
    "span",
    { class: `move ${m.tone}` },
    h("span", { attrs: { "aria-hidden": "true" } }, m.shown),
    h("span", { class: "visually-hidden" }, m.spoken),
  );
}

/** The usual price cell: the price, or "—" when Cozy has none. */
function usual(row: PriceRow): HTMLElement {
  if (row.usual === null) {
    return h("span", {}, h("span", { attrs: { "aria-hidden": "true" } }, "—"), h("span", { class: "visually-hidden" }, "no Free Market price"));
  }
  return h("span", { class: "price-figure" }, priceText(row.usual));
}

/** "Nov 12" for a day written YYYY-MM-DD (a UTC day, like Cozy's). */
export function dayName(day: string): string {
  return new Date(`${day}T00:00:00Z`).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}

/** How fresh the prices are for `world`, and the line that says so when the player should know. */
export function freshness(status: PriceStatus | null, world: World, now: number): { stale: boolean; line: string | null } {
  const updated = status?.updated?.[world];
  if (status === null || updated === undefined) {
    const offline = status !== null && status.failed && !status.downloading;
    return { stale: false, line: offline ? "Prices download when MesoVibe is online." : "Getting prices…" };
  }
  const at = Date.parse(updated);
  if (Number.isNaN(at) || now - at < STALE_MS) return { stale: false, line: null };
  const when = `${new Date(at).toLocaleDateString("en-US", { month: "short", day: "numeric" })}, ${clockTime(at)}`;
  const why = status.failed ? "MesoVibe can't reach its price service." : "No newer prices have come in since.";
  return { stale: true, line: `Prices from ${when}. ${why}` };
}

/**
 * The 30-day chart: one plain line, broken on days without a price, a dot on today, the high and
 * low at the left and the ends of the month under it. Pointing at it shows that day's price; screen
 * readers hear it as one sentence.
 */
export function chart(history: (number | null)[], days: string[]): HTMLElement {
  const points = history.flatMap((value, i) => (value === null ? [] : [{ i, value }]));
  if (points.length < 2) {
    return h("p", { class: "text muted", attrs: { "data-key": "price-chart-empty" } }, "The 30-day chart fills in as daily prices are kept.");
  }
  const values = points.map((p) => p.value);
  const high = Math.max(...values);
  const low = Math.min(...values);
  const last = history.length - 1;
  const width = 300;
  const height = 96;
  const pad = 6;
  const x = (i: number) => (last === 0 ? 0 : (i / last) * width);
  const y = (value: number) => (high === low ? height / 2 : pad + ((high - value) / (high - low)) * (height - 2 * pad));
  // Unbroken runs of days with a price: a day without one breaks the line.
  const runs: string[] = [];
  let run = "";
  history.forEach((value, i) => {
    if (value === null) {
      if (run) runs.push(run);
      run = "";
      return;
    }
    run += `${run ? "L" : "M"}${x(i).toFixed(1)} ${y(value).toFixed(1)}`;
  });
  if (run) runs.push(run);

  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", `0 0 ${width} ${height}`);
  svg.setAttribute("preserveAspectRatio", "none");
  svg.setAttribute("class", "chart-lines");
  svg.setAttribute("aria-hidden", "true");
  const shape = (tag: string, attrs: Record<string, string>) => {
    const element = document.createElementNS(ns, tag);
    for (const [k, v] of Object.entries(attrs)) element.setAttribute(k, v);
    svg.appendChild(element);
  };
  shape("line", { x1: "0", y1: String(height - 0.5), x2: String(width), y2: String(height - 0.5), class: "chart-base" });
  for (const d of runs) shape("path", { d, class: "chart-line" });
  const today = history[last];
  // A zero-length stroke with round ends: a dot that stays round when the chart stretches.
  if (today !== null && today !== undefined) {
    const ty = y(today).toFixed(1);
    shape("line", { x1: String(width), y1: ty, x2: String(width), y2: ty, class: "chart-dot" });
  }

  const at = (value: number) => days[points.find((p) => p.value === value)?.i ?? 0] ?? "";
  const first = values[0] ?? low;
  const end = values[values.length - 1] ?? high;
  const highDay = days.lastIndexOf(at(high)) === last ? "today" : `on ${dayName(at(high))}`;
  const lowDay = days.lastIndexOf(at(low)) === last ? "today" : `on ${dayName(at(low))}`;
  const sentence = `Usual price over 30 days: from ${priceText(first)} to ${priceText(end)}, high ${priceText(high)} ${highDay}, low ${priceText(low)} ${lowDay}.`;

  const pointed = h("p", { class: "chart-pointed number", attrs: { "aria-hidden": "true" } });
  const frame = h(
    "div",
    {
      class: "chart-frame",
      on: {
        pointermove: (event) => {
          const box = frame.getBoundingClientRect();
          const i = Math.round((((event as PointerEvent).clientX - box.left) / box.width) * last);
          const day = Math.max(0, Math.min(last, i));
          const value = history[day];
          pointed.textContent = `${dayName(days[day] ?? "")}: ${value === null || value === undefined ? "no price" : priceText(value)}`;
        },
        pointerleave: () => {
          pointed.textContent = "";
        },
      },
    },
    svg,
    h("span", { class: "chart-high number", attrs: { "aria-hidden": "true" } }, priceText(high)),
    h("span", { class: "chart-low number", attrs: { "aria-hidden": "true" } }, priceText(low)),
  );
  return h(
    "figure",
    { class: "chart", attrs: { role: "img", "aria-label": sentence, "data-key": "price-chart" } },
    frame,
    h("div", { class: "chart-ends", attrs: { "aria-hidden": "true" } }, h("span", {}, "30 days ago"), h("span", {}, "Today")),
    pointed,
  );
}

/**
 * The tab. It's built once and kept while the panel runs, so the search field keeps what the player
 * typed and where the caret is while results come and go around it.
 */
export class PricesTab {
  readonly root: HTMLElement;
  private world: World;
  private status: PriceStatus | null = null;
  private open = false;

  private query = "";
  private results: PriceRow[] = [];
  private total = 0;
  /** The result the arrow keys are on; -1 for none. */
  private active = -1;
  /** Searches answered out of order are dropped: only the latest counts. */
  private asked = 0;
  /** The item the detail shows. */
  private shown: PriceDetail | null = null;
  private recentIds: number[] = [];
  private recentRows: PriceRow[] = [];
  private movers: PriceMovers = { up: [], down: [] };
  /** Recently checked just cleared: the list Undo brings back, until when. */
  private cleared: { ids: number[]; until: number } | null = null;
  /** The price label is on, so the tab can follow what the player points at. */
  private pointing = false;
  /** The items last pointed at in game, shown when the tab opens. */
  private lastPointed: number[] | null = null;
  /** Several items the game names alike, to pick from ("Which item was it?"). */
  private which: PriceRow[] | null = null;

  private readonly input: HTMLInputElement;
  private readonly live: HTMLElement;
  private readonly worlds: HTMLElement;
  private readonly note: HTMLElement;
  private readonly main: HTMLElement;
  private readonly recentBlock: HTMLElement;
  private readonly moversBlock: HTMLElement;

  constructor(
    private readonly host: Host,
    world: World,
    /** The player picked a world here; the panel saves it and calls `setWorld`. */
    private readonly pickWorld: (world: World) => void,
  ) {
    this.world = world;
    this.input = h("input", {
      class: "field",
      attrs: {
        id: "price-search",
        type: "text",
        role: "combobox",
        autocomplete: "off",
        spellcheck: "false",
        "aria-autocomplete": "list",
        "aria-controls": "price-results",
        "aria-expanded": "false",
        "data-key": "price-search",
      },
      on: {
        input: () => this.typed(),
        keydown: (event) => this.fieldKey(event as KeyboardEvent),
      },
    });
    this.live = h("p", { class: "visually-hidden", attrs: { role: "status", "aria-live": "polite", "data-key": "price-count" } });
    this.worlds = h("div", { class: "segmented world-toggle", attrs: { role: "radiogroup", "aria-label": "Prices for world" } });
    this.note = h("p", { class: "text muted", attrs: { role: "status", "data-key": "price-note" } });
    this.main = h("div", { class: "price-main" });
    this.recentBlock = h("section", { class: "block", attrs: { "aria-label": "Recently checked", "data-key": "price-recent" } });
    this.moversBlock = h("section", { class: "block", attrs: { "aria-label": "Biggest moves this week", "data-key": "price-movers" } });
    this.root = h(
      "div",
      { class: "prices-tab" },
      h(
        "section",
        { class: "block" },
        h("div", { class: "search-head" }, h("label", { class: "label", attrs: { for: "price-search" } }, "Find an item"), this.worlds),
        this.input,
        this.live,
        this.note,
        this.main,
      ),
      this.recentBlock,
      this.moversBlock,
      h(
        "p",
        { class: "text muted price-credit" },
        "Free Market prices from ",
        h(
          "button",
          { class: "link", attrs: { type: "button", "data-key": "price-credit" }, on: { click: () => void this.host.openPricePage(null, this.world).catch(() => {}) } },
          "Cozy Markets ↗",
        ),
        ", which isn't part of MesoVibe. Prices are estimates.",
      ),
    );
    window.setInterval(() => {
      if (this.open) this.paintAge();
    }, AGE_CHECK_MS);
    void this.host.priceRecent().then((ids) => {
      this.recentIds = ids.slice(0, RECENT_MAX);
      if (this.open) void this.refresh();
    });
  }

  /** The tab opened: everything is fetched again for the world chosen, and the item last pointed
   * at in game is shown. */
  opened(): void {
    this.open = true;
    void this.refresh().then(() => {
      if (this.lastPointed !== null) this.follow(this.lastPointed);
    });
  }

  /** Whether the price label is on (and reading), which changes the how-to line. */
  setPointing(on: boolean): void {
    if (on === this.pointing) return;
    this.pointing = on;
    if (this.open) this.paintMain();
  }

  /**
   * The item the player points at in game. With the tab open and nothing typed in the search, the
   * detail follows it; opened later, the tab shows the last one. The tab never switches by
   * itself and never clears what the player typed.
   */
  pointed(ids: number[]): void {
    this.lastPointed = ids;
    if (this.open) this.follow(ids);
  }

  private follow(ids: number[]): void {
    const typing = this.query.trim() !== "" || document.activeElement === this.input;
    if (typing || ids.length === 0) return;
    if (ids.length === 1) {
      const [id] = ids as [number];
      if (this.shown?.id !== id || this.which !== null) void this.choose(id);
      return;
    }
    void this.host.priceRows(ids, this.world).then((rows) => {
      if (this.query.trim() !== "") return;
      this.which = rows;
      this.paintMain();
    });
  }

  closed(): void {
    this.open = false;
  }

  setStatus(status: PriceStatus): void {
    const before = JSON.stringify(this.status?.updated ?? null);
    this.status = status;
    // A new file: every figure may have changed.
    if (this.open && JSON.stringify(status.updated) !== before) void this.refresh();
    else this.paintAge();
  }

  setWorld(world: World): void {
    if (world === this.world) return;
    this.world = world;
    if (this.open) void this.refresh();
  }

  /** Ctrl/Cmd+F or "/" anywhere in the panel finds an item, while this tab is open. */
  key(event: KeyboardEvent): void {
    if (!this.open) return;
    const typing = event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement;
    const find = (event.key === "f" || event.key === "F") && (event.ctrlKey || event.metaKey) && !event.altKey;
    if (find || (event.key === "/" && !typing && !event.ctrlKey && !event.metaKey && !event.altKey)) {
      event.preventDefault();
      this.input.focus();
      this.input.select();
    }
  }

  // MARK: fetching

  private async refresh(): Promise<void> {
    const world = this.world;
    const [movers, recent, detail] = await Promise.all([
      this.host.priceMovers(world),
      this.host.priceRows(this.recentIds, world),
      this.shown === null ? Promise.resolve(null) : this.host.priceItem(this.shown.id, world),
    ]);
    if (world !== this.world) return;
    this.movers = movers;
    this.recentRows = recent;
    if (detail !== null) this.shown = detail;
    if (this.query !== "") await this.search();
    this.paint();
  }

  private typed(): void {
    this.query = this.input.value;
    this.active = -1;
    if (this.query.trim() === "") {
      this.asked += 1;
      this.results = [];
      this.total = 0;
      this.live.textContent = "";
      this.paintMain();
      return;
    }
    void this.search().then(() => this.paintMain());
  }

  private async search(): Promise<void> {
    const asked = ++this.asked;
    const found = await this.host.priceSearch(this.query, this.world);
    if (asked !== this.asked) return;
    this.results = found.rows;
    this.total = found.total;
    this.active = Math.min(this.active, found.rows.length - 1);
    this.live.textContent =
      found.total === 0 ? "No items" : found.total > found.rows.length ? `${found.total} items, the first ${found.rows.length} shown` : `${found.total} ${found.total === 1 ? "item" : "items"}`;
  }

  /** Shows an item's detail, and puts it at the top of Recently checked. */
  private async choose(id: number): Promise<void> {
    const detail = await this.host.priceItem(id, this.world);
    if (detail === null) return;
    this.shown = detail;
    this.input.value = "";
    this.query = "";
    this.results = [];
    this.active = -1;
    this.asked += 1;
    this.live.textContent = "";
    this.cleared = null;
    this.which = null;
    this.recentIds = [id, ...this.recentIds.filter((other) => other !== id)].slice(0, RECENT_MAX);
    void this.host.setPriceRecent(this.recentIds).catch(() => {});
    this.recentRows = await this.host.priceRows(this.recentIds, this.world);
    this.paint();
  }

  private clearRecent(): void {
    this.cleared = { ids: this.recentIds, until: Date.now() + UNDO_MS };
    this.recentIds = [];
    this.recentRows = [];
    void this.host.setPriceRecent([]).catch(() => {});
    window.setTimeout(() => {
      if (this.cleared !== null && Date.now() >= this.cleared.until) {
        this.cleared = null;
        this.paintRecent();
      }
    }, UNDO_MS + 50);
    this.paintRecent();
    this.paintMain();
  }

  private async undoClear(): Promise<void> {
    if (this.cleared === null) return;
    this.recentIds = this.cleared.ids;
    this.cleared = null;
    void this.host.setPriceRecent(this.recentIds).catch(() => {});
    this.recentRows = await this.host.priceRows(this.recentIds, this.world);
    this.paintRecent();
    this.paintMain();
  }

  private fieldKey(event: KeyboardEvent): void {
    const n = this.results.length;
    if (event.key === "ArrowDown" && n > 0) {
      event.preventDefault();
      this.active = (this.active + 1) % n;
      this.paintMain();
    } else if (event.key === "ArrowUp" && n > 0) {
      event.preventDefault();
      this.active = this.active <= 0 ? n - 1 : this.active - 1;
      this.paintMain();
    } else if (event.key === "Enter" && n > 0) {
      event.preventDefault();
      const row = this.results[Math.max(0, this.active)];
      if (row !== undefined) void this.choose(row.id);
    } else if (event.key === "Escape" && this.input.value !== "") {
      // Back to the last item shown.
      event.preventDefault();
      this.input.value = "";
      this.typed();
    }
  }

  // MARK: painting

  private paint(): void {
    this.paintWorlds();
    this.paintAge();
    this.paintMain();
    this.paintRecent();
    this.paintMovers();
  }

  private paintWorlds(): void {
    replace(
      this.worlds,
      ...WORLDS.map(([world, label]) =>
        h(
          "button",
          {
            class: "segment",
            attrs: { type: "button", role: "radio", "aria-checked": this.world === world ? "true" : "false", "data-key": `price-world-${world}` },
            on: { click: () => this.pickWorld(world) },
          },
          label,
        ),
      ),
    );
  }

  /** The line under the field about getting prices or their age, and muted figures when old. */
  private paintAge(): void {
    const fresh = freshness(this.status, this.world, Date.now());
    this.root.classList.toggle("stale", fresh.stale);
    if (this.note.textContent !== (fresh.line ?? "")) this.note.textContent = fresh.line ?? "";
    this.note.hidden = fresh.line === null;
  }

  private paintMain(): void {
    const searching = this.query.trim() !== "";
    this.input.setAttribute("aria-expanded", searching && this.results.length > 0 ? "true" : "false");
    if (searching && this.active >= 0) this.input.setAttribute("aria-activedescendant", `price-result-${this.active}`);
    else this.input.removeAttribute("aria-activedescendant");
    if (searching) {
      replace(this.main, this.resultsList());
    } else if (this.which !== null) {
      replace(
        this.main,
        h("h2", { class: "price-title", attrs: { "data-key": "price-which" } }, "Which item was it?"),
        this.rowButtons(this.which, "price-which-list"),
      );
    } else if (this.shown !== null) {
      replace(this.main, ...this.detail(this.shown));
    } else if (this.recentIds.length === 0) {
      const how = this.pointing ? "Point at an item in game, or search for one." : "Search for an item to see its Free Market price.";
      replace(this.main, h("p", { class: "text muted", attrs: { "data-key": "price-how" } }, how));
    } else {
      replace(this.main);
    }
  }

  private resultsList(): HTMLElement {
    if (this.results.length === 0) {
      return h("p", { class: "state", attrs: { "data-key": "price-none" } }, `No item called "${this.query.trim()}". Check the spelling.`);
    }
    return h(
      "div",
      { class: "price-list", attrs: { id: "price-results", role: "listbox", "aria-label": "Items found", "data-key": "price-results" } },
      this.header(),
      ...this.results.map((row, i) =>
        h(
          "div",
          {
            class: "price-row",
            attrs: { id: `price-result-${i}`, role: "option", "aria-selected": i === this.active ? "true" : "false" },
            on: {
              // Keeps focus in the field, so the list stays open under the pointer.
              mousedown: (event) => event.preventDefault(),
              click: () => void this.choose(row.id),
            },
          },
          ...this.cells(row),
        ),
      ),
    );
  }

  private header(): HTMLElement {
    return h(
      "div",
      { class: "price-row price-head", attrs: { "aria-hidden": "true" } },
      h("span", {}, "Item"),
      h("span", { class: "num" }, "Usual"),
      h("span", { class: "num" }, "Week"),
    );
  }

  private cells(row: PriceRow): HTMLElement[] {
    return [
      h("span", { class: "price-name" }, row.name, row.gender ? h("span", { class: "cell-detail" }, row.gender) : null),
      h("span", { class: "num number" }, usual(row)),
      h("span", { class: "num number" }, move(row.change)),
    ];
  }

  /** A list of items as buttons, each named for its item: Recently checked and Biggest moves. */
  private rowButtons(rows: PriceRow[], key: string): HTMLElement {
    return h(
      "div",
      { class: "price-list", attrs: { "data-key": key } },
      this.header(),
      ...rows.map((row) => h("button", { class: "price-row", attrs: { type: "button" }, on: { click: () => void this.choose(row.id) } }, ...this.cells(row))),
    );
  }

  private detail(item: PriceDetail): Node[] {
    const name = h("h2", { class: "price-title", attrs: { "data-key": "price-title" } }, item.name, item.gender ? h("span", { class: "cell-detail" }, item.gender) : null);
    const npc = item.npc === null ? "—" : formatDigits(item.npc);
    if (item.usual === null) {
      const why = item.untradeable
        ? "This item can't be traded, so it has no Free Market price."
        : this.status?.updated
          ? "No Free Market price yet. Cozy Markets lists an item once enough shops sell it."
          : "No Free Market prices have downloaded yet.";
      return [
        name,
        h("p", { class: "text", attrs: { "data-key": "price-no-market" } }, why),
        h("dl", { class: "key-value" }, h("dt", {}, "NPC shop pays"), h("dd", { class: "number" }, npc)),
      ];
    }
    const range = item.low7d !== null && item.high7d !== null ? `${priceText(item.low7d)} – ${priceText(item.high7d)}` : "—";
    const count = (n: number | null) => (n === null ? "—" : formatDigits(n));
    // The age line above says why old prices are old; here they only say from when.
    const updated = this.status?.updated?.[this.world];
    const at = updated === undefined ? Number.NaN : Date.parse(updated);
    const source = Number.isNaN(at)
      ? `${WORLD_NAME[this.world]} · Cozy Markets`
      : freshness(this.status, this.world, Date.now()).stale
        ? `${WORLD_NAME[this.world]} · Cozy Markets · from ${new Date(at).toLocaleDateString("en-US", { month: "short", day: "numeric" })}`
        : `${WORLD_NAME[this.world]} · Cozy Markets, updated ${clockTime(at)}`;
    return [
      name,
      h(
        "p",
        { class: "price-hero", attrs: { "data-key": "price-usual" } },
        h("span", { class: "number-hero price-figure" }, priceText(item.usual)),
        h("span", { class: "hero-unit" }, " usual price"),
      ),
      item.change === null
        ? h("p", { class: "text muted", attrs: { "data-key": "price-change" } }, "No weekly change yet.")
        : h("p", { class: "text change-line", attrs: { "data-key": "price-change" } }, move(item.change), " from a week ago"),
      chart(item.history, item.historyDays),
      h(
        "dl",
        { class: "key-value" },
        h("dt", {}, "Week's range"),
        h("dd", { class: "number price-figure" }, range),
        h("dt", {}, "Sales"),
        h("dd", { class: "number" }, count(item.sales)),
        h("dt", {}, "Listings"),
        h("dd", { class: "number" }, count(item.listings)),
        h("dt", {}, "NPC shop pays"),
        h("dd", { class: "number" }, npc),
      ),
      item.page
        ? h(
            "button",
            {
              class: "link",
              attrs: { type: "button", "data-key": "price-open" },
              on: { click: () => void this.host.openPricePage(item.id, this.world).catch(() => {}) },
            },
            "Open on Cozy Markets ↗",
          )
        : null,
      h("p", { class: "text muted", attrs: { "data-key": "price-source" } }, source),
    ].filter((node): node is HTMLElement => node !== null);
  }

  private paintRecent(): void {
    if (this.cleared !== null) {
      const cleared = this.cleared;
      replace(
        this.recentBlock,
        h(
          "div",
          { class: "notice-body", attrs: { role: "status" } },
          h("span", {}, "Recently checked cleared."),
          h("button", { class: "button secondary", attrs: { type: "button", "data-key": "price-recent-undo" }, on: { click: () => void this.undoClear() } }, "Undo"),
        ),
      );
      this.recentBlock.hidden = Date.now() >= cleared.until;
      return;
    }
    this.recentBlock.hidden = this.recentRows.length === 0;
    if (this.recentRows.length === 0) {
      replace(this.recentBlock);
      return;
    }
    replace(
      this.recentBlock,
      h(
        "div",
        { class: "block-head" },
        h("h2", { class: "label" }, "Recently checked"),
        h("button", { class: "text-button", attrs: { type: "button", "data-key": "price-recent-clear" }, on: { click: () => this.clearRecent() } }, "Clear"),
      ),
      this.rowButtons(this.recentRows, "price-recent-rows"),
    );
  }

  private paintMovers(): void {
    const { up, down } = this.movers;
    this.moversBlock.hidden = up.length + down.length === 0;
    replace(this.moversBlock, h("h2", { class: "label" }, "Biggest moves this week"), this.rowButtons([...up, ...down], "price-movers-rows"));
  }
}
