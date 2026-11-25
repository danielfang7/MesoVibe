/**
 * How the Session Tracker learns that a potion was drunk. A count source watches the engine's
 * events and reports each use it finds; the tracker adds the uses to the session and its map,
 * prices them and shows them, the same way whichever source found them.
 *
 * The source wired in counts from the quick slots: a potion use shows as a slot's count changing
 * just after the HP or MP numbers rise. The engine doesn't read the count's digits, only that its
 * picture changed, so the rise is what makes it a use (a potion picked up or bought changes the
 * count with no rise) and says which potion the slot holds: +160 HP is Meat, +150 MP Lemon.
 *
 * An icon's potion is learned from its uses: each rise of a potion's amount, short of the max (a
 * rise the max capped says nothing of the amount) and near no other item's count change, is a vote
 * for that potion, and a rise short of its amount (a smaller potion, or a hit) is one against; a
 * rise past it is that potion plus recovery. Two votes, and more votes than against, name it, and
 * the name is kept. Uses before that wait, and count once it's named; the
 * tracker drops them when the session pauses, ends or starts, since they're no longer its.
 *
 * Once named, a use counts on a looser rise: half the amount (a hit in the same frame), or a fifth
 * of it reaching the max (topped up to full; less is natural recovery). Two potions a moment apart
 * can show as one count change, so a change counts every rise before it of the potion's amount,
 * or else its one latest rise that fits loosely: a Heal just before a potion isn't a second potion.
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
   * Drops the uses it holds back (waiting for an icon's name): the session paused, ended or
   * started, and they aren't the running session's.
   */
  forget?(): void;
  /**
   * It counts from the quick slots: potions count only while the host reads them, and not at all
   * from a host that doesn't.
   */
  readsSlots?: boolean;
}

/**
 * How long after an HP or MP rise a slot's count change may come and still be one use with it: a
 * slot's change is sent once two frames agree, a frame after the rise, and frames come up to a
 * second apart. So a change's rise comes before it: a rise in the very frame a change is sent most
 * likely belongs to another slot's change, sent the next frame, and is taken only by a named icon
 * that has no earlier one. A rise after a change (potions picked up, then a heal) is no use.
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
/** How long a vote to name an icon lasts: a player may drink less than once every ten minutes. */
const BALLOT_MS = 60 * 60_000;

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

/**
 * A rise near an unnamed icon's count change: a vote to name it `item`, or with `item` null one
 * that fits no potion, against each potion of its resource whose amount it falls short of.
 */
interface Ballot {
  icon: string;
  item: number | null;
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
  /** Rises counted as a use: one rise is never two uses, nor a vote for another icon. */
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

  /** The rises a change of a named icon counts: every one of the potion's amount, or its latest loose fit. */
  const counted = (potion: Potion, near: Rise[]): Rise[] => {
    const exact = near.filter((r) => fits(potion, r, false));
    if (exact.length > 0) return exact;
    const loose = near.filter((r) => fits(potion, r, true));
    return loose.slice(-1);
  };

  /** A ballot for or against naming `icon`; a rise near two icons' changes counts for neither. */
  const vote = (icon: string, item: number | null, rise: Rise) => {
    if (contested.has(rise)) return;
    const other = ballots.find((b) => b.rise === rise);
    if (other === undefined) ballots.push({ icon, item, rise });
    else if (other.icon !== icon) contested.add(rise);
  };

  /**
   * Names each waiting icon whose settled ballots (rises no other change can claim any more, older
   * than PAIR_MS, and not counted for a named icon) give a potion two votes and more votes than
   * rises short of its amount, then counts its uses that waited no longer than WAIT_MS before the
   * name came.
   */
  const name = (atMs: number): PotUse[] => {
    const uses: PotUse[] = [];
    for (const icon of new Set(waiting.map((w) => w.icon))) {
      const settled = ballots.filter(
        (b) => b.icon === icon && !contested.has(b.rise) && !spent.has(b.rise) && atMs - b.rise.atMs > PAIR_MS,
      );
      const potion = POTIONS.find((p) => {
        const votes = settled.filter((b) => b.item === p.id).length;
        const short = (r: Rise) => r.resource === p.resource && r.amount < p.amount && r.amount * 2 >= p.amount;
        const against = settled.filter((b) => b.item === null && short(b.rise)).length;
        return votes >= NAME_VOTES && votes > against;
      });
      if (potion === undefined) continue;
      const namedAt = Math.max(...settled.filter((b) => b.item === potion.id).map((b) => b.rise.atMs)) + PAIR_MS;
      keep(icon, potion);
      for (const w of waiting.filter((w) => w.icon === icon && namedAt - w.atMs <= WAIT_MS)) {
        for (const r of counted(potion, w.rises.filter((r) => !spent.has(r)))) uses.push(use(potion, r));
      }
      waiting = waiting.filter((w) => w.icon !== icon);
      ballots = ballots.filter((b) => b.icon !== icon);
    }
    return uses;
  };

  /**
   * Pairs each change with the rises before it, oldest first. A named icon's change counts its
   * rises at once; an unnamed icon's waits with them, its rises voting to name it.
   */
  const pair = (atMs: number): PotUse[] => {
    rises = rises.filter((r) => atMs - r.atMs <= PAIR_MS);
    changes = changes.filter((c) => atMs - c.atMs <= PAIR_MS);
    // Before the waiting uses age out: a name that came while no event did still counts them.
    const uses = name(atMs);
    for (const change of [...changes]) {
      const known = POTIONS.find((p) => p.id === icons.get(change.icon));
      const before = rises.filter((r) => r.atMs < change.atMs && change.atMs - r.atMs <= PAIR_MS);
      if (known !== undefined) {
        // A rise in the change's own frame only when none before it fits.
        let taken = counted(known, before);
        if (taken.length === 0) taken = counted(known, rises.filter((r) => r.atMs === change.atMs));
        if (taken.length === 0) continue;
        changes = changes.filter((c) => c !== change);
        for (const r of taken) uses.push(use(known, r));
        keep(change.icon, known);
        continue;
      }
      changes = changes.filter((c) => c !== change);
      if (before.length === 0) continue;
      for (const r of before.filter((r) => !r.full)) {
        const [only, ...more] = POTIONS.filter((p) => fits(p, r, false));
        if (more.length === 0) vote(change.icon, only?.id ?? null, r);
      }
      waiting.push({ icon: change.icon, rises: before, atMs: change.atMs });
    }
    waiting = waiting.filter((w) => atMs - w.atMs <= WAIT_MS).slice(-MAX_WAITING);
    ballots = ballots.filter((b) => atMs - b.rise.atMs <= BALLOT_MS);
    return uses;
  };

  const onSlot = (event: QuickSlotChanged, atMs: number) => {
    const before = slots.get(event.slot);
    slots.set(event.slot, { count: event.count_key, icon: event.icon_key });
    // The same item, its count now another: a use if a rise goes with it. A count that went
    // unreadable (covered whole) or came back isn't a change; one partly covered is, a known limit.
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
    forget: () => {
      waiting = [];
    },
    readsSlots: true,
  };
}

/** The source the tracker counts potions with, from what it saved last time. */
export function wiredSource(saved?: unknown): CountSource | null {
  return slotSource(saved);
}
