/**
 * How the Session Tracker learns that a potion was drunk. A count source watches the engine's
 * events and reports each use it finds; the tracker adds the uses to the session and its map,
 * prices them and shows them, the same way whichever source found them.
 *
 * The source wired in counts from the quick slots: a potion use shows as a slot's count changing
 * in the same moment the HP or MP numbers rise. The engine doesn't read the count's digits, only
 * that its picture changed, so the rise is what makes it a use (a potion picked up or bought
 * changes the count with no rise) and says which potion the slot holds: +160 HP is Meat, +150 MP
 * Lemon. Each icon's potion is learned from its first use whose rise is that potion's amount, and
 * kept. After that a use of it may rise by less (HP topped up to full, or a hit in the same frame)
 * or a little more (natural recovery in the same frame).
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
 * second apart. A change never comes before its rise, so one that does (potions picked up, then a
 * heal) is no use.
 */
export const PAIR_MS = 1_500;
/** How far past a potion's amount the rise that names an icon may go: natural recovery in the same frame. */
const NAME_SLACK = 10;
/** Icons whose potion is kept, the newest last. */
const MAX_ICONS = 32;

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

/** Whether `rise` can be a use of `potion`: once the icon is known, loosely; to name it, by its amount. */
function fits(potion: Potion, rise: Rise, known: boolean): boolean {
  if (potion.resource !== rise.resource) return false;
  if (known) return rise.full || rise.amount * 2 >= potion.amount;
  return rise.amount >= potion.amount && rise.amount <= potion.amount + NAME_SLACK;
}

/** The count source that pairs quick-slot count changes with HP and MP rises. */
export function slotSource(saved?: unknown): CountSource {
  const icons = keptIcons(saved);
  const slots = new Map<number, SlotSeen>();
  let rises: Rise[] = [];
  let changes: Change[] = [];
  let last: HpMpChanged | null = null;

  /** Pairs each change with a rise near it, oldest first; a pair is one use. */
  const pair = (atMs: number): PotUse[] => {
    rises = rises.filter((r) => atMs - r.atMs <= PAIR_MS);
    changes = changes.filter((c) => atMs - c.atMs <= PAIR_MS);
    const uses: PotUse[] = [];
    for (const change of [...changes]) {
      const item = icons.get(change.icon);
      const known = POTIONS.find((p) => p.id === item);
      const near = rises.filter((r) => r.atMs <= change.atMs && change.atMs - r.atMs <= PAIR_MS);
      let potion: Potion | undefined;
      let rise: Rise | undefined;
      if (known !== undefined) {
        rise = near.find((r) => fits(known, r, true));
        potion = rise === undefined ? undefined : known;
      } else {
        for (const r of near) {
          const named = POTIONS.filter((p) => fits(p, r, false));
          if (named.length === 1) {
            [potion, rise] = [named[0], r];
            break;
          }
        }
      }
      if (potion === undefined || rise === undefined) continue;
      icons.delete(change.icon);
      icons.set(change.icon, potion.id);
      while (icons.size > MAX_ICONS) icons.delete(icons.keys().next().value as string);
      rises = rises.filter((r) => r !== rise);
      changes = changes.filter((c) => c !== change);
      uses.push({ item: potion.id, count: 1, maybe: false, source: "slots" });
    }
    return uses;
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
