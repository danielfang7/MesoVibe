/**
 * The Session Tracker's loot arithmetic: mesos, items and time per map, kept free of the host so
 * it can be tested on its own.
 *
 * A session's play is split by map, in first-visit order. Mesos/hr is measured only over the time
 * the pickup notices were being read: the mesos counted then over that time, so a stretch with
 * the notices covered (a party's HP list, a monster walking behind them) neither raises nor
 * lowers it. Items count the same way, valued at what an NPC shop pays for them, so loot/hr is
 * mesos plus that value over the read time. Loot picked up before the session's first EXP gain is
 * held: it joins the session if
 * EXP follows within five minutes on the same map, so a walk to the grinding map doesn't count as
 * play and an errand in town doesn't become a session.
 *
 * Potions drunk count on the map from the session's first EXP gain (a potion on the walk to a map
 * isn't the grind's cost), measured over their own read time, the time the HP and MP numbers were
 * read, so pot cost/hr leaves out a stretch where a potion couldn't have been seen.
 */

import type { Event, NoticeState } from "@mesovibe/sdk";

/**
 * One map's share of a session. Saved with short keys, since 500 sessions are kept.
 */
export interface MapEntry {
  /** The map's key, or null for a map whose name couldn't be read ("Unknown map"). */
  k: string | null;
  /** Time played on it. */
  p: number;
  /** EXP gained on it. */
  x: number;
  /** Mesos counted on it. */
  m: number;
  /** Mesos counted while the pickup notices were being read: mesos/hr's numerator. */
  r: number;
  /** Time on it while the pickup notices were being read: mesos/hr's denominator. */
  l: number;
  /** The level when the session reached it, and when it last played it. */
  s: number | null;
  e: number | null;
  /** Mesos counted with a digit named by its shape, which may turn out wrong. */
  sm?: number;
  /** A pickup was missed while on it. */
  mi?: true;
  /** A party bonus line was read while on it. */
  pa?: true;
  /** The items picked up on it, in the order first picked up. */
  i?: ItemEntry[];
  /** The NPC value of the items picked up on it while the notices were read: loot/hr's share. */
  rv?: number;
  /** Potions drunk on it, by potion. */
  u?: PotCount[];
  /** Time on it while potions were being counted: pot cost/hr's denominator. */
  h?: number;
  /** Part of `h` while heals that aren't potions may have hidden some. */
  hm?: number;
}

/** One potion's uses, by its item ID in potions.ts. */
export interface PotCount {
  k: number;
  /** Uses counted. */
  c: number;
  /** Rises that may have been this potion: shown apart, costing nothing. */
  y?: number;
}

/** One kind of item picked up, by its key in the [`Catalog`]. */
export interface ItemEntry {
  /** The item's key: see [`itemKey`]. */
  k: string;
  /** How many were picked up. */
  c: number;
}

/** What the game data said about an item's key, kept once for every session. */
export interface ItemInfo {
  /** Its name, or null when no item fit the name read ("Unread items"). */
  n: string | null;
  /** The inventory tab it went to, when the notice's letters told it. */
  t: string | null;
  /** Mesos an NPC shop pays for one, when known. */
  v: number | null;
  /**
   * For a name several items fit that sell for different prices, the lowest and highest price of
   * one: it's worth at least the first. Absent for anything else.
   */
  r?: [number, number];
}

/** What one of an item is worth at least: its price, or the lowest of the prices it may have. */
export function leastPrice(info: ItemInfo | undefined): number | null {
  return info?.v ?? info?.r?.[0] ?? null;
}

/** An item event's lowest and highest price, when they differ. */
function priceRange(event: Extract<Event, { type: "ItemGained" }>): [number, number] | undefined {
  const { sell_min: min, sell_max: max } = event;
  return min != null && max != null && min < max ? [min, max] : undefined;
}

/** Item keys and what's known about them. */
export type Catalog = Record<string, ItemInfo>;

/** Loot picked up before the session's first EXP gain. */
export interface Held {
  /** The map it was picked up on (undefined: not known yet). */
  map: string | null | undefined;
  /** When the first of it was picked up. */
  atMs: number;
  mesos: number;
  shaped: number;
  missed: boolean;
  items?: ItemEntry[];
}

/** What a session keeps of its loot, beside its EXP. All optional: older saves have none. */
export interface LootFields {
  maps?: MapEntry[];
  held?: Held;
  /** Time played before the map was known, given to the first map known. */
  unplaced?: number;
  /** A pickup may have been missed: totals are at least what's shown. */
  missed?: true;
  /** A digit named by its shape turned out wrong: amounts counted with it may be wrong. */
  shapesWrong?: true;
  /** Where each potion's count came from, by item ID: rises, its quick slot, or both in turn. */
  ps?: Record<string, PotSource>;
  /** A past session's pot cost at End, so a later price change never rewrites history. */
  pt?: number;
}

/** Where a potion's count came from. */
export type PotSource = "rises" | "slots" | "both";

/** Whether potions were being counted during a step, and whether heals from others may hide some. */
export interface PotTime {
  counting: boolean;
  healing: boolean;
}

/** How long held loot waits for EXP on its map. */
export const HOLD_MS = 5 * 60_000;
/** The least read time a mesos rate is shown for: mesos come in lumps. */
export const MESOS_RATE_MS = 60_000;

/** The entry for `key`, made with the level now if it's new. */
export function entryFor(loot: LootFields, key: string | null, level: number | null): MapEntry {
  const maps = (loot.maps ??= []);
  let entry = maps.find((m) => m.k === key);
  if (entry === undefined) {
    entry = { k: key, p: 0, x: 0, m: 0, r: 0, l: 0, s: level, e: level };
    maps.push(entry);
  }
  return entry;
}

/**
 * Adds `step` of play on `map`; `reading` when the pickup notices were being read, `pots` when
 * potions were being counted. Before the map is known the time waits, and goes to the first map
 * known, so the maps add up to the session.
 */
export function addTime(
  loot: LootFields,
  map: string | null | undefined,
  step: number,
  reading: boolean,
  level: number | null,
  pots?: PotTime,
): void {
  if (step <= 0) return;
  if (map === undefined) {
    loot.unplaced = (loot.unplaced ?? 0) + step;
    return;
  }
  const entry = entryFor(loot, map, level);
  entry.p += step + (loot.unplaced ?? 0);
  delete loot.unplaced;
  if (reading) entry.l += step;
  if (pots?.counting) {
    entry.h = (entry.h ?? 0) + step;
    if (pots.healing) entry.hm = (entry.hm ?? 0) + step;
  }
  if (level !== null) entry.e = level;
}

/** Adds an EXP gain on `map`. */
export function addExp(loot: LootFields, map: string | null | undefined, amount: number, level: number | null): void {
  if (map === undefined) return;
  const entry = entryFor(loot, map, level);
  entry.x += amount;
  if (level !== null) entry.e = level;
}

/** Adds `count` of `key` to `list`, a new key at its end. */
export function addCount(list: ItemEntry[], key: string, count: number): void {
  const entry = list.find((e) => e.k === key);
  if (entry === undefined) list.push({ k: key, c: count });
  else entry.c += count;
}

/**
 * The key an item pickup is counted under: "#<item ID>" when the game data names one item, the name
 * when several items share it or start it ("=<name>"), or the tab when no item fits ("?<tab>").
 */
export function itemKey(event: Extract<Event, { type: "ItemGained" }>): string {
  if (event.item_id !== null) return `#${event.item_id}`;
  if (event.name !== null) return `=${event.name}`;
  return `?${event.tab ?? ""}`;
}

/** Adds `count` uses of potion `item`, or that many maybe uses, to `list`. */
export function addPot(list: PotCount[], item: number, count: number, maybe: boolean): void {
  let entry = list.find((e) => e.k === item);
  if (entry === undefined) {
    entry = { k: item, c: 0 };
    list.push(entry);
  }
  if (maybe) entry.y = (entry.y ?? 0) + count;
  else entry.c += count;
}

/** Adds every count in `from` to `list`. */
function addPots(list: PotCount[], from: readonly PotCount[]): void {
  for (const pot of from) {
    addPot(list, pot.k, pot.c, false);
    if (pot.y) addPot(list, pot.k, pot.y, true);
  }
}

/**
 * Counts a potion use on the map, and where its count came from. `started`: the session has its
 * first EXP gain; before it nothing counts, so a potion drunk on the walk to a map, or in town,
 * isn't the session's.
 */
export function addPotUse(
  loot: LootFields,
  use: { item: number; count: number; maybe: boolean; source: "rises" | "slots" },
  map: string | null | undefined,
  started: boolean,
  level: number | null,
): void {
  if (!started || map === undefined || !(use.count > 0)) return;
  addPot((entryFor(loot, map, level).u ??= []), use.item, use.count, use.maybe);
  notePotSource(loot, use.item, use.source);
}

/** Records where a potion's count came from: a second kind makes it "both". */
function notePotSource(loot: LootFields, item: number, source: "rises" | "slots"): void {
  const sources = (loot.ps ??= {});
  const was = sources[item];
  sources[item] = was === undefined || was === source ? source : "both";
}

/** Whether held loot has anything in it. */
export function hasLoot(held: Held | undefined): boolean {
  return held !== undefined && (held.mesos > 0 || (held.items?.length ?? 0) > 0);
}

/**
 * Counts an item pickup, as [`addPickup`] counts mesos: on the map, or held before the session's
 * first EXP gain. Its name, tab and price go into `catalog`.
 */
export function addItem(
  loot: LootFields,
  catalog: Catalog,
  event: Extract<Event, { type: "ItemGained" }>,
  map: string | null | undefined,
  started: boolean,
  reading: boolean,
  atMs: number,
  level: number | null,
): void {
  // The game never shows x0: a count read as 0 is a misread pickup, so it counts as missed.
  if (!(event.count > 0)) {
    loot.missed = true;
    return;
  }
  const key = itemKey(event);
  // A pickup no single item fits is counted under its tab ("?Etc"), one key for all of them: a
  // price there would price every unread pickup of the tab, in every session, so it keeps none.
  // Otherwise a price once known stays when a later pickup of the same key brings none (one whose
  // tab wasn't read can fit items in several tabs that sell for different prices).
  const price = key.startsWith("?") ? null : (event.sell ?? catalog[key]?.v ?? null);
  // A name several items fit at different prices has no one price, but its range is kept the
  // same way, and it counts at the lowest.
  const range = key.startsWith("?") || price !== null ? undefined : (priceRange(event) ?? catalog[key]?.r);
  catalog[key] = range === undefined ? { n: event.name, t: event.tab, v: price } : { n: event.name, t: event.tab, v: price, r: range };
  const least = price ?? range?.[0] ?? null;
  if (!started) {
    if (loot.held === undefined || loot.held.map !== map) {
      loot.held = { map, atMs, mesos: 0, shaped: 0, missed: false };
    }
    addCount((loot.held.items ??= []), key, event.count);
    return;
  }
  if (map === undefined) return;
  const entry = entryFor(loot, map, level);
  addCount((entry.i ??= []), key, event.count);
  if (reading && least !== null) entry.rv = (entry.rv ?? 0) + least * event.count;
}

/**
 * Counts a pickup event. `started`: the session has its first EXP gain; before it, loot is held.
 * `reading`: the notices are being read now, so mesos count toward the rate.
 */
export function addPickup(
  loot: LootFields,
  event: Event,
  map: string | null | undefined,
  started: boolean,
  reading: boolean,
  atMs: number,
  level: number | null,
): void {
  if (event.type === "MesosGained") {
    if (!started) {
      // Another map than the held loot's: that loot was an errand, and starts over here.
      if (loot.held === undefined || loot.held.map !== map) {
        loot.held = { map, atMs, mesos: 0, shaped: 0, missed: false };
      }
      loot.held.mesos += event.amount;
      if (event.shaped) loot.held.shaped += event.amount;
      return;
    }
    if (map === undefined) return;
    const entry = entryFor(loot, map, level);
    entry.m += event.amount;
    if (reading) entry.r += event.amount;
    if (event.shaped) entry.sm = (entry.sm ?? 0) + event.amount;
  } else if (event.type === "PartyBonus") {
    if (started && map !== undefined) entryFor(loot, map, level).pa = true;
  } else if (event.type === "NoticesMissed") {
    switch (event.reason) {
      // Waiting for a digit: counted late, nothing missed.
      case "not_learned":
        return;
      case "shapes_wrong":
        loot.shapesWrong = true;
        for (const entry of loot.maps ?? []) if ((entry.sm ?? 0) > 0) entry.mi = true;
        return;
      default:
        loot.missed = true;
        if (!started && loot.held !== undefined) loot.held.missed = true;
        else if (started && map !== undefined) entryFor(loot, map, level).mi = true;
    }
  }
}

/**
 * The session's first EXP gain: held loot joins it when it was picked up on this map within
 * [`HOLD_MS`], and is dropped otherwise. The maps start afresh with the session's `lead` of play
 * (the session's clock restarts at its first gain, keeping a short lead).
 */
export function start(
  loot: LootFields,
  map: string | null | undefined,
  lead: number,
  reading: boolean,
  level: number | null,
  atMs: number,
  pots?: PotTime,
): void {
  // A list, even empty, marks a session as one from after map and mesos tracking began.
  loot.maps = [];
  delete loot.unplaced;
  const held = loot.held;
  delete loot.held;
  addTime(loot, map, lead, reading, level, pots);
  if (held !== undefined && held.map === map && map !== undefined && atMs - held.atMs <= HOLD_MS) {
    const entry = entryFor(loot, map, level);
    // Its mesos count in the totals, not in the rate: they came before the time measured.
    entry.m += held.mesos;
    for (const item of held.items ?? []) addCount((entry.i ??= []), item.k, item.c);
    if (held.shaped > 0) entry.sm = (entry.sm ?? 0) + held.shaped;
    if (held.missed) {
      entry.mi = true;
      loot.missed = true;
    }
  }
}

/** Mesos counted in the session, held loot included. */
export function totalMesos(loot: LootFields): number {
  return (loot.maps ?? []).reduce((sum, m) => sum + m.m, 0) + (loot.held?.mesos ?? 0);
}

/** The counts in `lists` added up by key, in the order each key was first seen. */
export function sumItems(lists: Iterable<readonly ItemEntry[]>): ItemEntry[] {
  const counts = new Map<string, number>();
  for (const list of lists) for (const item of list) counts.set(item.k, (counts.get(item.k) ?? 0) + item.c);
  return [...counts].map(([k, c]) => ({ k, c }));
}

/** Every item the session picked up, held loot included, in the order first picked up. */
export function sessionItems(loot: LootFields): ItemEntry[] {
  return sumItems([...(loot.maps ?? []).map((entry) => entry.i ?? []), loot.held?.items ?? []]);
}

/**
 * What NPC shops pay for `items`, counting the items whose price is known, and the ones that may be
 * several items at the lowest of their prices.
 */
export function itemsValue(items: ItemEntry[], catalog: Catalog): number {
  return items.reduce((sum, item) => sum + item.c * (leastPrice(catalog[item.k]) ?? 0), 0);
}

/**
 * Mesos and the NPC value of items per hour, over the time the notices were read, or null before
 * that means anything.
 */
export function lootRate(loot: LootFields): number | null {
  const maps = loot.maps ?? [];
  const read = maps.reduce((sum, m) => sum + m.l, 0);
  const value = maps.reduce((sum, m) => sum + m.r + (m.rv ?? 0), 0);
  if (read < MESOS_RATE_MS || value <= 0) return null;
  return (value * 3_600_000) / read;
}

/** A map's own EXP/hr and mesos/hr, each null when there's too little time for it. */
export function mapRates(entry: MapEntry, minExpMs: number): { exp: number | null; mesos: number | null } {
  return {
    exp: entry.p >= minExpMs && entry.x > 0 ? (entry.x * 3_600_000) / entry.p : null,
    mesos: entry.l >= MESOS_RATE_MS ? (entry.r * 3_600_000) / entry.l : null,
  };
}

/** The map played longest, which a past session's row is named for. */
export function longest(maps: MapEntry[]): MapEntry | null {
  return maps.reduce<MapEntry | null>((best, m) => (best === null || m.p > best.p ? m : best), null);
}

/** Whether the notices state lets loot count toward a rate. */
export function reads(state: NoticeState): boolean {
  return state === "reading";
}

/**
 * Adds `from`'s loot into `into`: Undo puts a session back, keeping pickups since and potions
 * counted since. A potion counts only once a session has EXP, so one drunk after End and before
 * the next gain isn't brought back. Neither read time is added, so mesos/hr and pot cost/hr keep
 * the same base after an Undo.
 */
export function mergeLoot(into: LootFields, from: LootFields): void {
  for (const entry of from.maps ?? []) {
    const target = entryFor(into, entry.k, entry.s);
    target.m += entry.m;
    target.r += entry.r;
    for (const item of entry.i ?? []) addCount((target.i ??= []), item.k, item.c);
    if (entry.rv) target.rv = (target.rv ?? 0) + entry.rv;
    if (entry.sm) target.sm = (target.sm ?? 0) + entry.sm;
    if (entry.mi) target.mi = true;
    if (entry.pa) target.pa = true;
    if (entry.u !== undefined) addPots((target.u ??= []), entry.u);
  }
  if (from.held !== undefined) {
    const map = from.held.map;
    if (map !== undefined) {
      const target = entryFor(into, map, null);
      target.m += from.held.mesos;
      for (const item of from.held.items ?? []) addCount((target.i ??= []), item.k, item.c);
    }
  }
  for (const [item, source] of Object.entries(from.ps ?? {})) {
    if (source === "both") (into.ps ??= {})[item] = "both";
    else notePotSource(into, Number(item), source);
  }
  if (from.missed) into.missed = true;
  if (from.shapesWrong) into.shapesWrong = true;
}

/**
 * A saved per-map list, with anything malformed dropped: a bad value in a save must never stop
 * the tracker, at this launch or any later one.
 */
export function validMaps(value: unknown): MapEntry[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const count = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n) && n >= 0;
  const level = (n: unknown): n is number | null => n === null || (typeof n === "number" && Number.isInteger(n) && n > 0);
  const maps: MapEntry[] = [];
  for (const raw of value) {
    if (typeof raw !== "object" || raw === null) continue;
    const e = raw as Record<string, unknown>;
    if (!(typeof e.k === "string" || e.k === null)) continue;
    if (![e.p, e.x, e.m, e.r, e.l].every(count) || !level(e.s) || !level(e.e)) continue;
    const entry: MapEntry = { k: e.k, p: e.p as number, x: e.x as number, m: e.m as number, r: e.r as number, l: e.l as number, s: e.s, e: e.e };
    if (count(e.sm) && e.sm > 0) entry.sm = e.sm;
    if (e.mi === true) entry.mi = true;
    if (e.pa === true) entry.pa = true;
    const items = validItems(e.i);
    if (items.length > 0) entry.i = items;
    if (count(e.rv) && e.rv > 0) entry.rv = e.rv;
    const pots = validPots(e.u);
    if (pots.length > 0) entry.u = pots;
    if (count(e.h) && e.h > 0) entry.h = e.h;
    if (count(e.hm) && e.hm > 0) entry.hm = Math.min(e.hm, entry.h ?? 0);
    maps.push(entry);
  }
  return maps;
}

/** A saved item list, with anything malformed dropped. */
export function validItems(value: unknown): ItemEntry[] {
  if (!Array.isArray(value)) return [];
  const items: ItemEntry[] = [];
  for (const raw of value) {
    if (typeof raw !== "object" || raw === null) continue;
    const { k, c } = raw as Record<string, unknown>;
    if (typeof k === "string" && ITEM_KEY.test(k) && typeof c === "number" && Number.isInteger(c) && c > 0) addCount(items, k, c);
  }
  return items;
}

/** A saved list of potion counts, with anything malformed dropped. */
export function validPots(value: unknown): PotCount[] {
  if (!Array.isArray(value)) return [];
  const whole = (n: unknown): n is number => typeof n === "number" && Number.isInteger(n) && n >= 0;
  const pots: PotCount[] = [];
  for (const raw of value) {
    if (typeof raw !== "object" || raw === null) continue;
    const { k, c, y } = raw as Record<string, unknown>;
    if (!whole(k) || k === 0 || !whole(c) || !(y === undefined || whole(y))) continue;
    if (c > 0) addPot(pots, k, c, false);
    if (y !== undefined && y > 0) addPot(pots, k, y, true);
  }
  return pots;
}

/** A saved record of where potion counts came from, with anything malformed dropped. */
export function validPotSources(value: unknown): Record<string, PotSource> | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
  const sources: Record<string, PotSource> = {};
  for (const [item, source] of Object.entries(value)) {
    if (/^[1-9]\d*$/.test(item) && (source === "rises" || source === "slots" || source === "both")) sources[item] = source;
  }
  return Object.keys(sources).length > 0 ? sources : undefined;
}

/** What an item key looks like: "#<item ID>", "=<name>" or "?<tab>" (itemKey). */
const ITEM_KEY = /^(#\d+|=.+|\?.*)$/s;

/** A saved catalog, with anything malformed dropped. It has no prototype, so no key reads one. */
export function validCatalog(value: unknown): Catalog {
  const catalog: Catalog = Object.create(null) as Catalog;
  if (typeof value !== "object" || value === null || Array.isArray(value)) return catalog;
  const text = (x: unknown): x is string | null => x === null || typeof x === "string";
  const mesos = (x: unknown): x is number => typeof x === "number" && Number.isFinite(x) && x >= 0;
  const range = (x: unknown): x is [number, number] => Array.isArray(x) && x.length === 2 && mesos(x[0]) && mesos(x[1]) && x[0] <= x[1];
  for (const [key, raw] of Object.entries(value)) {
    if (typeof raw !== "object" || raw === null) continue;
    const { n, t, v, r } = raw as Record<string, unknown>;
    if (!ITEM_KEY.test(key) || !text(n) || !text(t) || !(v === null || mesos(v))) continue;
    catalog[key] = range(r) ? { n, t, v, r: [r[0], r[1]] } : { n, t, v };
  }
  return catalog;
}
