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
 * Adds `step` of play on `map`; `reading` when the pickup notices were being read. Before the map
 * is known the time waits, and goes to the first map known, so the maps add up to the session.
 */
export function addTime(loot: LootFields, map: string | null | undefined, step: number, reading: boolean, level: number | null): void {
  if (step <= 0) return;
  if (map === undefined) {
    loot.unplaced = (loot.unplaced ?? 0) + step;
    return;
  }
  const entry = entryFor(loot, map, level);
  entry.p += step + (loot.unplaced ?? 0);
  delete loot.unplaced;
  if (reading) entry.l += step;
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
  catalog[key] = { n: event.name, t: event.tab, v: price };
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
  if (reading && price !== null) entry.rv = (entry.rv ?? 0) + price * event.count;
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
export function start(loot: LootFields, map: string | null | undefined, lead: number, reading: boolean, level: number | null, atMs: number): void {
  // A list, even empty, marks a session as one from after map and mesos tracking began.
  loot.maps = [];
  delete loot.unplaced;
  const held = loot.held;
  delete loot.held;
  addTime(loot, map, lead, reading, level);
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

/** What NPC shops pay for `items`, counting the items whose price is known. */
export function itemsValue(items: ItemEntry[], catalog: Catalog): number {
  return items.reduce((sum, item) => sum + item.c * (catalog[item.k]?.v ?? 0), 0);
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

/** Adds `from`'s loot into `into`: Undo puts a session back, keeping pickups made since. */
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
  }
  if (from.held !== undefined) {
    const map = from.held.map;
    if (map !== undefined) {
      const target = entryFor(into, map, null);
      target.m += from.held.mesos;
      for (const item of from.held.items ?? []) addCount((target.i ??= []), item.k, item.c);
    }
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

/** What an item key looks like: "#<item ID>", "=<name>" or "?<tab>" (itemKey). */
const ITEM_KEY = /^(#\d+|=.+|\?.*)$/s;

/** A saved catalog, with anything malformed dropped. It has no prototype, so no key reads one. */
export function validCatalog(value: unknown): Catalog {
  const catalog: Catalog = Object.create(null) as Catalog;
  if (typeof value !== "object" || value === null || Array.isArray(value)) return catalog;
  const text = (x: unknown): x is string | null => x === null || typeof x === "string";
  for (const [key, raw] of Object.entries(value)) {
    if (typeof raw !== "object" || raw === null) continue;
    const { n, t, v } = raw as Record<string, unknown>;
    const price = v === null || (typeof v === "number" && Number.isFinite(v) && v >= 0);
    if (ITEM_KEY.test(key) && text(n) && text(t) && price) catalog[key] = { n, t, v: v as number | null };
  }
  return catalog;
}
