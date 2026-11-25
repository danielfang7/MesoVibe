/**
 * The Session Tracker's loot arithmetic, kept free of the host so it can be tested on its own:
 * time and EXP by map, potions drunk, and the meso ledger.
 *
 * A session's play is split by map, in first-visit order. Its mesos come from the meso totals read
 * in the inventory and shop windows ([`Ledger`]), which miss nothing: what changed between two
 * totals is mesos picked up, what a shop paid or what was spent, and a stretch between two totals
 * with only one grinding map played gives that map its mesos/hr.
 *
 * Sessions saved before then counted mesos and items from the pickup notices, which miss pickups
 * on busy maps. What they counted is kept as saved and shown as an estimate, apart from anything
 * measured from meso totals.
 *
 * Potions drunk count on the map from the session's first EXP gain (a potion on the walk to a map
 * isn't the grind's cost), measured over their own read time, the time the HP and MP numbers were
 * read, so pot cost/hr leaves out a stretch where a potion couldn't have been seen.
 */

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
  /**
   * Mesos the pickup notices counted on it, all of them and those read while the notices were in
   * view, and the time they were in view. Only sessions saved before mesos came from meso totals
   * have any; later ones keep 0, so older builds still load them.
   */
  m: number;
  r: number;
  l: number;
  /** The level when the session reached it, and when it last played it. */
  s: number | null;
  e: number | null;
  /**
   * From the pickup notices, in older sessions: mesos counted with a digit named by its shape, a
   * pickup missed, a party bonus line read, the items picked up (in the order first picked up),
   * and the NPC value of those picked up while the notices were in view.
   */
  sm?: number;
  mi?: true;
  pa?: true;
  i?: ItemEntry[];
  rv?: number;
  /** Potions drunk on it, by potion. */
  u?: PotCount[];
  /** Time on it while potions were being counted: pot cost/hr's denominator. */
  h?: number;
  /** Part of `h` while heals that aren't potions may have hidden some. */
  hm?: number;
  /** Mesos picked up on it by the meso ledger: rises between two readings with only this map played. */
  fm?: number;
  /** Play time between those readings: the map's exact mesos/hr's denominator. */
  ft?: number;
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
  /**
   * The item's key: "#<item ID>" when the game data named one item, "=<name>" when several items
   * share the name or start it, "?<tab>" when no item fit.
   */
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

/** Item keys and what's known about them. */
export type Catalog = Record<string, ItemInfo>;

/**
 * Loot the pickup notices counted before the session's first EXP gain, in a session an older build
 * left running.
 */
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
  /** From the pickup notices, in older sessions: a pickup may have been missed, or a digit misread. */
  missed?: true;
  shapesWrong?: true;
  /** Where each potion's count came from, by item ID: rises, its quick slot, or both in turn. */
  ps?: Record<string, PotSource>;
  /** A past session's pot cost at End, so a later price change never rewrites history. */
  pt?: number;
  /** The meso ledger, from the meso totals read in the inventory and shop windows. */
  lg?: Ledger;
}

/** Where a potion's count came from. */
export type PotSource = "rises" | "slots" | "both";

/** Whether potions were being counted during a step, and whether heals from others may hide some. */
export interface PotTime {
  counting: boolean;
  healing: boolean;
}

/**
 * How long before the first EXP gain a meso total may open the session, and how long loot an older
 * build held before it waited.
 */
export const HOLD_MS = 5 * 60_000;
/** The least play time a mesos rate is shown for: mesos come in lumps. */
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
 * Adds `step` of play on `map`; `pots` when potions were being counted. Before the map is known
 * the time waits, and goes to the first map known, so the maps add up to the session.
 */
export function addTime(loot: LootFields, map: string | null | undefined, step: number, level: number | null, pots?: PotTime): void {
  if (step <= 0) return;
  if (map === undefined) {
    loot.unplaced = (loot.unplaced ?? 0) + step;
    return;
  }
  const entry = entryFor(loot, map, level);
  entry.p += step + (loot.unplaced ?? 0);
  delete loot.unplaced;
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
  if (!started || map === undefined || !Number.isInteger(use.count) || use.count <= 0) return;
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
 * The session's first EXP gain: the maps start afresh with the session's `lead` of play (the
 * session's clock restarts at its first gain, keeping a short lead).
 */
export function start(loot: LootFields, map: string | null | undefined, lead: number, level: number | null, pots?: PotTime): void {
  // A list, even empty, marks a session as one from after map tracking began.
  loot.maps = [];
  delete loot.unplaced;
  delete loot.held;
  addTime(loot, map, lead, level, pots);
}

/** Mesos the pickup notices counted in an older session, held loot included. */
export function totalMesos(loot: LootFields): number {
  return (loot.maps ?? []).reduce((sum, m) => sum + m.m, 0) + (loot.held?.mesos ?? 0);
}

/** The counts in `lists` added up by key, in the order each key was first seen. */
export function sumItems(lists: Iterable<readonly ItemEntry[]>): ItemEntry[] {
  const counts = new Map<string, number>();
  for (const list of lists) for (const item of list) counts.set(item.k, (counts.get(item.k) ?? 0) + item.c);
  return [...counts].map(([k, c]) => ({ k, c }));
}

/** Every item an older session's pickup notices counted, held loot included, in the order first picked up. */
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
 * Mesos per hour an older session's pickup notices counted, over the time they were in view, or
 * null under a minute of it.
 */
export function noticeMesosRate(loot: LootFields): number | null {
  const maps = loot.maps ?? [];
  const read = maps.reduce((sum, m) => sum + m.l, 0);
  const mesos = maps.reduce((sum, m) => sum + m.r, 0);
  if (read < MESOS_RATE_MS || mesos <= 0) return null;
  return (mesos * 3_600_000) / read;
}

/**
 * A map's own EXP/hr and mesos/hr, each null when there's too little time for it. Mesos/hr comes
 * only from the meso ledger ([`fieldRate`]): a map without a stretch between two meso totals read
 * on it has none, whatever its pickup notices counted.
 */
export function mapRates(entry: MapEntry, minExpMs: number): { exp: number | null; mesos: number | null } {
  return {
    exp: entry.p >= minExpMs && entry.x > 0 ? (entry.x * 3_600_000) / entry.p : null,
    mesos: fieldRate(entry),
  };
}

/** The map played longest, which a past session's row is named for. */
export function longest(maps: MapEntry[]): MapEntry | null {
  return maps.reduce<MapEntry | null>((best, m) => (best === null || m.p > best.p ? m : best), null);
}

/**
 * Where a trusted meso total was read: the inventory window, an NPC shop's, or the storage window's
 * mesos on hand.
 */
export type MesoSource = "inventory" | "shop" | "storage";

/**
 * A session's meso ledger: what the meso totals read in the inventory and shop windows say the
 * session earned. Totals reach it already trusted (read the same in several frames), so it only
 * decides what each change between two of them counts as. Saved with short keys; sessions saved
 * before it have none.
 *
 * Profit is mesos picked up plus what shops paid. Mesos spent (potions, gear, a taxi) show on their
 * own and are never taken from profit: potions count as a cost when they're drunk, and buying gear
 * isn't a loss of the grind. A rise too big to be farming, like a storage withdrawal, a trade or a
 * misread, is held out of profit until the player counts it.
 */
export interface Ledger {
  /** The balance the next change is measured from, or null at the start of a segment. */
  b: number | null;
  /** When that balance was read (wall clock), for the held-rise check. */
  ba: number;
  /** The session's play time when it was read, for Profit/hr's clock. */
  bp: number;
  /** The session's first balance, or null before one. */
  ob: number | null;
  /** Trusted totals read this session. */
  n: number;
  /** Mesos picked up: rises between readings outside a shop, with no pause between them. */
  pu: number;
  /** Mesos shops paid: rises read in a shop, or across a pause. */
  so: number;
  /** Mesos spent: every fall. */
  sp: number;
  /** Held rises the player counted. */
  ct: number;
  /** Held rises not counted yet, oldest first: [amount, when read]. */
  ho: [number, number][];
  /** Play time between readings, pauses left out: Profit/hr's clock. */
  pt: number;
  /** Until when an ended session counts what's sold at a shop (wall clock): its close-out. */
  co?: number;
  /**
   * Where the last total was read, so a storage visit's later totals move the balance without
   * counting: mesos put into or taken out of storage are neither loot nor spending.
   */
  ls?: MesoSource;
}

/** What the session did between the last trusted total and this one. */
export interface SinceReading {
  /** The session's play time now, pauses left out. */
  playMs: number;
  /**
   * The one map played since the last reading, a grinding one (null: a map whose name couldn't be
   * read), or undefined when several were or it was a town, so the change goes to no map.
   */
  map: string | null | undefined;
  /**
   * The session was paused since the last reading. A rise then is what a shop paid, never mesos
   * picked up. Time in a town isn't away: a potion run passes through one.
   */
  away: boolean;
}

/** What one reading did, for the panel's notices. */
export type LedgerChange =
  | { kind: "opening"; total: number }
  | { kind: "picked" | "sold" | "spent" | "held"; amount: number }
  /** Mesos moved to or from storage: the balance follows, nothing is counted. */
  | { kind: "moved"; amount: number }
  | { kind: "same" };

/** A rise outside a shop is held when it's more than this... */
export const HOLD_MIN = 50_000;
/** ...and more than this many mesos an hour since the last reading: faster than any farming. */
export const HOLD_PER_HOUR = 3_000_000;
/** How long after End what's sold at a shop still counts for the session. */
export const CLOSE_OUT_MS = 30 * 60_000;
/** A balance read longer ago than this shows as stale. */
export const STALE_MS = 15 * 60_000;

function newLedger(): Ledger {
  return { b: null, ba: 0, bp: 0, ob: null, n: 0, pu: 0, so: 0, sp: 0, ct: 0, ho: [], pt: 0 };
}

/** Whether a rise outside a shop is too big to be farming. */
function tooBig(ledger: Ledger, rise: number, atMs: number): boolean {
  const hours = Math.max(0, atMs - ledger.ba) / 3_600_000;
  return rise > Math.max(HOLD_MIN, HOLD_PER_HOUR * hours);
}

/** Moves the balance to `total`, read at `atMs` with `playMs` of the session played. */
function rebase(ledger: Ledger, total: number, atMs: number, playMs: number, source?: MesoSource): void {
  ledger.b = total;
  ledger.ba = atMs;
  ledger.bp = playMs;
  ledger.n += 1;
  if (ledger.ob === null) ledger.ob = total;
  if (source === undefined) delete ledger.ls;
  else ledger.ls = source;
}

/**
 * Whether a total moves mesos to or from storage: a storage total after another one, read while the
 * window stayed open. The change up to a visit's first storage total counts by the usual rules,
 * since nothing has moved yet when the window opens.
 */
function transfer(ledger: Ledger, source: MesoSource): boolean {
  return source === "storage" && ledger.ls === "storage";
}

/**
 * For an EXP gain: no one gains EXP with the storage window open, so the next storage total is
 * another visit's first, counted by the usual rules (what was picked up meanwhile).
 */
export function leftStorage(loot: LootFields): void {
  if (loot.lg?.ls === "storage") delete loot.lg.ls;
}

/**
 * Counts a trusted meso total read during the session. The first one of a segment only sets the
 * balance. After that a fall is spent; a rise read in a shop is what the shop paid; any other rise
 * is held when it's too big to be farming, else it's what a shop paid when a pause came between,
 * or mesos picked up, which go to the one grinding map played since the last reading.
 */
export function addReading(loot: LootFields, total: number, source: MesoSource, atMs: number, since: SinceReading): LedgerChange {
  const ledger = (loot.lg ??= newLedger());
  const before = ledger.b;
  if (before === null) {
    rebase(ledger, total, atMs, since.playMs, source);
    return { kind: "opening", total };
  }
  const played = Math.max(0, since.playMs - ledger.bp);
  const delta = total - before;
  if (transfer(ledger, source)) {
    ledger.pt += played;
    rebase(ledger, total, atMs, since.playMs, source);
    return { kind: "moved", amount: delta };
  }
  let change: LedgerChange;
  if (delta < 0) {
    ledger.sp += -delta;
    change = { kind: "spent", amount: -delta };
  } else if (delta === 0) {
    change = { kind: "same" };
  } else if (source === "shop") {
    ledger.so += delta;
    change = { kind: "sold", amount: delta };
  } else if (tooBig(ledger, delta, atMs)) {
    ledger.ho.push([delta, atMs]);
    change = { kind: "held", amount: delta };
  } else if (since.away) {
    ledger.so += delta;
    change = { kind: "sold", amount: delta };
  } else {
    ledger.pu += delta;
    change = { kind: "picked", amount: delta };
  }
  // A stretch on one grinding map is that map's exact mesos/hr, whatever the change was.
  if (!since.away && since.map !== undefined && source === "inventory") {
    const entry = entryFor(loot, since.map, null);
    if (change.kind === "picked") entry.fm = (entry.fm ?? 0) + change.amount;
    entry.ft = (entry.ft ?? 0) + played;
  }
  ledger.pt += played;
  rebase(ledger, total, atMs, since.playMs, source);
  return change;
}

/**
 * Takes a trusted total read in the five minutes before the session's first EXP gain as its
 * opening balance, so a player who just checked their inventory in town isn't asked again. False
 * when it's older than that.
 */
export function openWith(loot: LootFields, total: number, readAtMs: number, startAtMs: number, source?: MesoSource): boolean {
  if (startAtMs - readAtMs > HOLD_MS) return false;
  const ledger = (loot.lg = newLedger());
  rebase(ledger, total, readAtMs, 0, source);
  return true;
}

/**
 * Starts a new segment: the EXP side started a new baseline (another character, a restarted game),
 * so the next total sets the balance again rather than counting as a change. What's counted stays.
 */
export function restartSegment(loot: LootFields): void {
  if (loot.lg !== undefined) loot.lg.b = null;
}

/**
 * Ends the session: what's sold at a shop in the next [`CLOSE_OUT_MS`] still counts for it (its
 * close-out). Only where meso totals can be read (`reads`), so no player is asked to sell for a
 * reading nothing makes. A session that read no total gets a ledger here, and the close-out's
 * first total sets its balance.
 */
export function waitForCloseOut(loot: LootFields, atMs: number, reads: boolean): void {
  if (reads) (loot.lg ??= newLedger()).co = atMs + CLOSE_OUT_MS;
}

/**
 * Ends the close-out: its time ran out, the next session gained its first EXP, or MesoVibe quit. A
 * ledger that never read a total goes with it.
 */
export function stopCloseOut(loot: LootFields): void {
  const ledger = loot.lg;
  if (ledger === undefined) return;
  delete ledger.co;
  if (ledger.n === 0 && ledger.ho.length === 0) delete loot.lg;
}

/**
 * Counts a trusted total read during the close-out. Selling takes several readings (the shop's
 * total before the first sale, then a rise per sale), so every total until the close-out ends
 * counts, not only the first. The time since End wasn't play: a rise is what a shop paid (held
 * when it's too big for farming and not read in a shop), a fall is spent, and Profit/hr's clock
 * doesn't move. The first total of a session that read none only sets the balance. Null when no
 * close-out is open, or its time has run out (which ends it).
 */
export function closeOut(loot: LootFields, total: number, source: MesoSource, atMs: number): LedgerChange | null {
  const ledger = loot.lg;
  if (ledger?.co === undefined) return null;
  if (atMs > ledger.co) {
    stopCloseOut(loot);
    return null;
  }
  if (ledger.b === null) {
    rebase(ledger, total, atMs, ledger.bp, source);
    return { kind: "opening", total };
  }
  const delta = total - ledger.b;
  if (transfer(ledger, source)) {
    rebase(ledger, total, atMs, ledger.bp, source);
    return { kind: "moved", amount: delta };
  }
  let change: LedgerChange;
  if (delta < 0) {
    ledger.sp += -delta;
    change = { kind: "spent", amount: -delta };
  } else if (delta === 0) {
    change = { kind: "same" };
  } else if (source !== "shop" && tooBig(ledger, delta, atMs)) {
    ledger.ho.push([delta, atMs]);
    change = { kind: "held", amount: delta };
  } else {
    ledger.so += delta;
    change = { kind: "sold", amount: delta };
  }
  rebase(ledger, total, atMs, ledger.bp, source);
  return change;
}

/**
 * Undoes a sale the close-out counted: the rise wasn't a sale (a trade, a storage withdrawal). The
 * balance stays as read, so the next total is measured from it and the rise isn't counted again.
 */
export function uncountSale(loot: LootFields, amount: number): void {
  if (loot.lg !== undefined) loot.lg.so = Math.max(0, loot.lg.so - amount);
}

/** Counts the held rise at `index` into profit; its amount and when it was read, for Undo. */
export function countHeld(loot: LootFields, index: number): [number, number] | null {
  const held = loot.lg?.ho.splice(index, 1)[0];
  if (held === undefined || loot.lg === undefined) return null;
  loot.lg.ct += held[0];
  return held;
}

/** Undoes [`countHeld`]: the rise is held again, in its place by when it was read. */
export function uncountHeld(loot: LootFields, held: [number, number]): void {
  const ledger = loot.lg;
  if (ledger === undefined) return;
  ledger.ct = Math.max(0, ledger.ct - held[0]);
  const at = ledger.ho.findIndex(([, atMs]) => atMs > held[1]);
  ledger.ho.splice(at === -1 ? ledger.ho.length : at, 0, held);
}

/** Profit: mesos picked up, what shops paid, and the held rises counted. */
export function ledgerProfit(ledger: Ledger): number {
  return ledger.pu + ledger.so + ledger.ct;
}

/**
 * Mesos picked up per hour, over play time between readings: the Loot section's headline. Null
 * without two readings a minute of play apart.
 */
export function mesosRate(ledger: Ledger): number | null {
  if (ledger.n < 2 || ledger.pt < MESOS_RATE_MS) return null;
  return (ledger.pu * 3_600_000) / ledger.pt;
}

/** Profit per hour, over the same play time as [`mesosRate`], or null when it has none. */
export function profitRate(ledger: Ledger): number | null {
  if (ledger.n < 2 || ledger.pt < MESOS_RATE_MS) return null;
  return (ledgerProfit(ledger) * 3_600_000) / ledger.pt;
}

/** A map's exact mesos/hr from the ledger, or null with under a minute of readings on it alone. */
export function fieldRate(entry: MapEntry): number | null {
  const time = entry.ft ?? 0;
  return time >= MESOS_RATE_MS ? ((entry.fm ?? 0) * 3_600_000) / time : null;
}

/** Whether the last balance is older than [`STALE_MS`]. */
export function ledgerStale(ledger: Ledger, atMs: number): boolean {
  return ledger.b !== null && atMs - ledger.ba > STALE_MS;
}

/**
 * Adds `from`'s ledger into `into` on Undo. The mesos counted since add up; play time doesn't, as
 * with the other rates, and the session no longer waits to close out. When totals were read since,
 * their balance was timed on the other session's clock, so the next total starts a new segment
 * rather than being measured against it; otherwise the balance from before stays.
 */
function mergeLedger(into: LootFields, from: Ledger): void {
  const target = (into.lg ??= newLedger());
  target.pu += from.pu;
  target.so += from.so;
  target.sp += from.sp;
  target.ct += from.ct;
  target.n += from.n;
  target.ho.push(...from.ho);
  if (target.ob === null) target.ob = from.ob;
  if (from.b !== null) target.b = null;
  delete target.co;
}

/**
 * Adds `from`'s loot into `into`: Undo puts a session back, keeping the mesos and potions counted
 * since. A potion counts only once a session has EXP, so one drunk after End and before the next
 * gain isn't brought back. Neither clock is added to, so mesos/hr and pot cost/hr keep the same
 * base after an Undo.
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
    if (entry.fm) target.fm = (target.fm ?? 0) + entry.fm;
  }
  if (from.lg !== undefined) mergeLedger(into, from.lg);
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
    if (count(e.ft) && e.ft > 0) {
      entry.ft = e.ft;
      if (count(e.fm) && e.fm > 0) entry.fm = e.fm;
    }
    maps.push(entry);
  }
  return maps;
}

/** A saved meso ledger, or undefined when it's malformed: the session then keeps everything else. */
export function validLedger(value: unknown): Ledger | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
  const l = value as Record<string, unknown>;
  const count = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n) && n >= 0;
  const balance = (n: unknown): n is number | null => n === null || count(n);
  if (![l.ba, l.bp, l.n, l.pu, l.so, l.sp, l.ct, l.pt].every(count) || !balance(l.b) || !balance(l.ob)) return undefined;
  if (!Array.isArray(l.ho)) return undefined;
  const ho: [number, number][] = [];
  for (const raw of l.ho) {
    if (Array.isArray(raw) && raw.length === 2 && count(raw[0]) && raw[0] > 0 && count(raw[1])) ho.push([raw[0], raw[1]]);
  }
  const ledger: Ledger = {
    b: l.b as number | null,
    ba: l.ba as number,
    bp: l.bp as number,
    ob: l.ob as number | null,
    n: l.n as number,
    pu: l.pu as number,
    so: l.so as number,
    sp: l.sp as number,
    ct: l.ct as number,
    ho,
    pt: l.pt as number,
  };
  if (count(l.co)) ledger.co = l.co;
  if (l.ls === "inventory" || l.ls === "shop" || l.ls === "storage") ledger.ls = l.ls;
  return ledger;
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

/** What an item key looks like: "#<item ID>", "=<name>" or "?<tab>" ([`ItemEntry`]). */
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
