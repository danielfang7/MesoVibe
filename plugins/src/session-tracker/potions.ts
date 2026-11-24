/**
 * The potions the Session Tracker counts, kept by hand: Meat and Lemon, the common setup. Each
 * restores a fixed amount unless something else changes it (a recovery skill, a Heal), and near
 * full it fills to the max and no further. Their amounts and NPC sell prices come from the game
 * data (see Game data in README.md); their shop prices were checked in the game. A `pnpm test`
 * check compares the IDs, names and sell prices with engine/data/items.tsv.
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
];
