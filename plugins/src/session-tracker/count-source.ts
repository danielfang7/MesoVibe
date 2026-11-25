/**
 * How the Session Tracker learns that a potion was drunk. A count source watches the engine's
 * events and reports each use it finds; the tracker adds the uses to the session and its map,
 * prices them and shows them, the same way whichever source found them.
 *
 * The source wired in counts from the quick slots: a potion use shows as a slot's count changing
 * in the same moment the HP or MP numbers rise. The engine doesn't read the count's digits, only
 * that its picture changed, so the rise is what makes it a use (a potion picked up or bought
 * changes the count with no rise) and says which potion the slot holds: +160 HP is Meat, +150 MP
 * Lemon. An icon's potion is learned from two uses whose rises are that potion's amount, short of
 * the max (a rise capped by the max says nothing of the amount), each near no other item's count
 * change; then it's kept. Uses before that wait, and count once it's named. After that a use of it
 * may rise by less (HP topped up to full, or a hit in the same frame) or a little more (natural
 * recovery in the same frame), and one count change takes every rise before it that fits: two
 * potions a moment apart can show as one change.
 */

import type { Event } from "@mesovibe/sdk";
import { POTIONS } from "./potions.js";
import type { Potion } from "./potions.js";

/** Uses of one potion that a source found in one event. */
export interface PotUse {
  /** The potion's item ID, as in potions.ts. */
  item: number;
  count: number;
  /**
   * A rise that may have been this potion: shown apart and costing nothing, so the cost shown is a
   * lower bound.
   */
  maybe: boolean;
  /** Counted from the HP or MP numbers going up, or from a quick slot's count going down. */
  source: "rises" | "slots";
}

export interface CountSource {
  /**
   * Takes every engine event, in order, paused or not, and returns the potion uses it found in it.
   * It reports uses only from times the HP and MP numbers were read, since pot cost/hr divides by
   * that time.
   */
  onEvent(event: Event, atMs: number): PotUse[];
  /**
   * Whether heals that aren't potions (a Heal, Drain, MP Eater) may be hiding uses of this
   * resource's potion now, so its count is a lower bound.
   */
  healing(resource: "hp" | "mp", atMs: number): boolean;
  /** The amounts the potion is counted at, the base first: [160], or [160, 240] with a boost. */
  amounts(item: number): number[];
  /** What the source learned, to keep with the tracker's data and hand back next time. */
  saved?(): unknown;
  /**
   * It counts from the quick slots: potions count only while the host reads them, and not at all
   * from a host that doesn't.
   */
  readsSlots?: boolean;
}

/**
 * How long after an HP or MP rise a slot's count change may come and still be one use with it: a
 * slot's change is sent once two frames agree, a frame after the rise, and frames come up to a
 * second apart. So a change's rise always comes before it, never with it or after: a rise in the
 * frame a change is sent belongs to another slot's change, sent the next frame, and one after a
 * change (potions picked up, then a heal) is no use.
 */
export const PAIR_MS = 1_500;
/** How far past a potion's amount the rise that names an icon may go: natural recovery in the same frame. */
const NAME_SLACK = 10;
/** Rises of a potion's amount it takes to name an icon: one could be another potion plus recovery. */
const NAME_VOTES = 2;
/** Icons whose potion is kept, the one used longest ago first. */
const MAX_ICONS = 32;
/** How long a use of an icon not named yet waits for its name, and how many wait. */
const WAIT_MS = 10 * 60_000;
const MAX_WAITING = 20;

/** A rise of the HP or MP numbers between two readings in a row. */
interface Rise {
  resource: "hp" | "mp";
  amount: number;
  /** It reached the max: a potion near full restores less than its amount. */
  full: boolean;
  atMs: number;
}

/** A slot's count changing with the same item in it. */
interface Change {
  icon: string;
  atMs: number;
}

/** A count change of an icon not named yet, with the rises before it, waiting for the icon's name. */
interface Waiting {
  icon: string;
  rises: Rise[];
  atMs: number;
}

/** A rise of a potion's amount near an unnamed icon's count change: a vote to name it that potion. */
interface Ballot {
  icon: string;
  item: number;
  rise: Rise;
}

/** What a slot showed last. */
interface SlotSeen {
  count: string | null;
  icon: string;
}

type QuickSlotChanged = Extract<Event, { type: "QuickSlotChanged" }>;
type HpMpChanged = Extract<Event, { type: "HpMpChanged" }>;

/** The icons kept from an earlier session, as [icon key, potion ID] pairs; anything else is dropped. */
function keptIcons(saved: unknown): Map<string, number> {
  const icons = new Map<string, number>();
  const list = (saved as { icons?: unknown } | null)?.icons;
  if (!Array.isArray(list)) return icons;
  for (const entry of list.slice(-MAX_ICONS)) {
    if (!Array.isArray(entry) || entry.length !== 2) continue;
    const [icon, item] = entry as unknown[];
    if (typeof icon === "string" && POTIONS.some((p) => p.id === item)) icons.set(icon, item as number);
  }
  return icons;
}

/**
 * Whether `rise` can be a use of `potion`. Once the icon is known, loosely: half its amount (a hit
 * in the same frame), or a fifth of it reaching the max (topped up to full; less is natural
 * recovery). To name the icon, by its amount, never from a rise the max capped.
 */
function fits(potion: Potion, rise: Rise, known: boolean): boolean {
  if (potion.resource !== rise.resource) return false;
  if (known) return rise.amount * 2 >= potion.amount || (rise.full && rise.amount * 5 >= potion.amount);
  return !rise.full && rise.amount >= potion.amount && rise.amount <= potion.amount + NAME_SLACK;
}

/** The count source that pairs quick-slot count changes with HP and MP rises. */
export function slotSource(saved?: unknown): CountSource {
  const icons = keptIcons(saved);
  const slots = new Map<number, SlotSeen>();
  let rises: Rise[] = [];
  let changes: Change[] = [];
  let waiting: Waiting[] = [];
  let ballots: Ballot[] = [];
  /** Rises near two items' count changes: they name neither. */
  const contested = new WeakSet<Rise>();
  /** Rises counted as a use: one rise is never two uses. */
  const spent = new WeakSet<Rise>();
  let last: HpMpChanged | null = null;

  const use = (potion: Potion, rise: Rise): PotUse => {
    spent.add(rise);
    rises = rises.filter((r) => r !== rise);
    return { item: potion.id, count: 1, maybe: false, source: "slots" };
  };

  /** Names an icon, or marks it used, keeping the icons used most recently. */
  const keep = (icon: string, potion: Potion) => {
    icons.delete(icon);
    icons.set(icon, potion.id);
    while (icons.size > MAX_ICONS) icons.delete(icons.keys().next().value as string);
  };

  /** A rise of `item`'s amount near an unnamed icon's change; a rise near two icons' changes names neither. */
  const vote = (icon: string, item: number, rise: Rise) => {
    if (contested.has(rise)) return;
    const other = ballots.find((b) => b.rise === rise);
    if (other === undefined) ballots.push({ icon, item, rise });
    else if (other.icon !== icon) contested.add(rise);
  };

  /**
   * Names each waiting icon that has enough votes from rises no other change can claim any more
   * (older than PAIR_MS), and counts its waiting uses.
   */
  const name = (atMs: number): PotUse[] => {
    const uses: PotUse[] = [];
    for (const icon of new Set(waiting.map((w) => w.icon))) {
      const settled = ballots.filter((b) => b.icon === icon && !contested.has(b.rise) && atMs - b.rise.atMs > PAIR_MS);
      const potion = POTIONS.find((p) => settled.filter((b) => b.item === p.id).length >= NAME_VOTES);
      if (potion === undefined) continue;
      keep(icon, potion);
      for (const w of waiting.filter((w) => w.icon === icon)) {
        for (const r of w.rises) if (!spent.has(r) && fits(potion, r, true)) uses.push(use(potion, r));
      }
      waiting = waiting.filter((w) => w.icon !== icon);
      ballots = ballots.filter((b) => b.icon !== icon);
    }
    return uses;
  };

  /**
   * Pairs each change with the rises before it, oldest first. A known icon's change counts every
   * rise that fits; an unknown icon's waits with its rises, voting to name the icon.
   */
  const pair = (atMs: number): PotUse[] => {
    rises = rises.filter((r) => atMs - r.atMs <= PAIR_MS);
    changes = changes.filter((c) => atMs - c.atMs <= PAIR_MS);
    waiting = waiting.filter((w) => atMs - w.atMs <= WAIT_MS).slice(-MAX_WAITING);
    ballots = ballots.filter((b) => atMs - b.rise.atMs <= WAIT_MS);
    const uses: PotUse[] = [];
    for (const change of [...changes]) {
      const known = POTIONS.find((p) => p.id === icons.get(change.icon));
      const near = rises.filter((r) => r.atMs < change.atMs && change.atMs - r.atMs <= PAIR_MS);
      if (known !== undefined) {
        const fitting = near.filter((r) => fits(known, r, true));
        if (fitting.length === 0) continue;
        changes = changes.filter((c) => c !== change);
        for (const r of fitting) uses.push(use(known, r));
        keep(change.icon, known);
        continue;
      }
      changes = changes.filter((c) => c !== change);
      if (near.length === 0) continue;
      for (const r of near) {
        const [only, ...more] = POTIONS.filter((p) => fits(p, r, false));
        if (only !== undefined && more.length === 0) vote(change.icon, only.id, r);
      }
      waiting.push({ icon: change.icon, rises: near, atMs: change.atMs });
    }
    return [...uses, ...name(atMs)];
  };

  const onSlot = (event: QuickSlotChanged, atMs: number) => {
    const before = slots.get(event.slot);
    slots.set(event.slot, { count: event.count_key, icon: event.icon_key });
    // The same item, its count now another: a use if a rise goes with it. A count that went
    // unreadable (something drawn over it) or came back isn't a change.
    const counted = before !== undefined && before.icon === event.icon_key;
    if (counted && before.count !== null && event.count_key !== null && before.count !== event.count_key) {
      changes.push({ icon: event.icon_key, atMs });
    }
  };

  const onHpMp = (event: HpMpChanged, atMs: number) => {
    const before = last;
    last = event;
    if (before === null || !event.continuous) return;
    if (event.hp > before.hp) rises.push({ resource: "hp", amount: event.hp - before.hp, full: event.hp >= event.max_hp, atMs });
    if (event.mp > before.mp) rises.push({ resource: "mp", amount: event.mp - before.mp, full: event.mp >= event.max_mp, atMs });
  };

  return {
    onEvent(event, atMs) {
      if (event.type === "QuickSlotChanged") onSlot(event, atMs);
      else if (event.type === "HpMpChanged") onHpMp(event, atMs);
      else return [];
      return pair(atMs);
    },
    // A heal from a skill raises HP or MP with no slot changing, so it never hides a use.
    healing: () => false,
    amounts: (item) => {
      const potion = POTIONS.find((p) => p.id === item);
      return potion === undefined ? [] : [potion.amount];
    },
    saved: () => ({ icons: [...icons] }),
    readsSlots: true,
  };
}

/** The source the tracker counts potions with, from what it saved last time. */
export function wiredSource(saved?: unknown): CountSource | null {
  return slotSource(saved);
}
