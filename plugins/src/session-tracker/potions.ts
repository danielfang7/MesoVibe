/**
 * The potions the Session Tracker counts, kept by hand: Meat and Lemon, the common setup, and the
 * Orange, White and Blue Potions players buy next. Each restores a fixed amount of HP or MP unless
 * something else changes it (a recovery skill, a Heal), and near full it fills to the max and no
 * further. A quick slot's potion is told apart only by that amount (count-source.ts), so two of
 * the same kind must restore more than 10 apart. Meat's and Lemon's amounts and every NPC sell
 * price come from the game data (see Game data in README.md); the other amounts and every shop
 * price were checked in the game. A `pnpm test` check compares the IDs, names and sell prices with
 * engine/data/items.tsv, and the amounts with each other.
 */

export interface Potion {
  /** Its item ID: the lowest one when several items share its name, as two Lemons do. */
  id: number;
  name: string;
  /** What it restores. */
  resource: "hp" | "mp";
  /** How much it restores before a recovery skill raises it. */
  amount: number;
  /** What a potion shop charges for one. */
  buy: number;
  /** What an NPC shop pays for one: what a potion picked up during the session is worth. */
  sell: number;
  /** Where and when `buy` was checked. */
  checked: string;
}

export const POTIONS: readonly Potion[] = [
  { id: 2010003, name: "Meat", resource: "hp", amount: 160, buy: 80, sell: 8, checked: "in the game, 2026-11-15" },
  { id: 2010005, name: "Lemon", resource: "mp", amount: 150, buy: 150, sell: 15, checked: "in the game, 2026-11-15" },
  { id: 2000001, name: "Orange Potion", resource: "hp", amount: 250, buy: 150, sell: 15, checked: "in the game, 2026-11-27" },
  { id: 2000002, name: "White Potion", resource: "hp", amount: 500, buy: 350, sell: 35, checked: "in the game, 2026-11-27" },
  { id: 2000003, name: "Blue Potion", resource: "mp", amount: 200, buy: 220, sell: 22, checked: "in the game, 2026-11-27" },
];
