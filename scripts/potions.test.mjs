// The potions the Session Tracker counts (plugins/src/session-tracker/potions.ts) agree with the
// game data in engine/data/items.tsv. Fails, naming the entry, when an ID, name or NPC sell price
// is wrong, or when a potion's ID isn't the lowest of the items sharing its name.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { POTIONS } from "../plugins/src/session-tracker/potions.ts";

const items = readFileSync(new URL("../engine/data/items.tsv", import.meta.url), "utf8")
  .trim()
  .split("\n")
  .slice(1)
  .map((line) => {
    const [id, tab, sell, , name] = line.split("\t");
    return { id: Number(id), tab, sell: sell === "" ? null : Number(sell), name };
  });

test("each potion's ID, name and sell price match the game data", () => {
  for (const potion of POTIONS) {
    const item = items.find((i) => i.id === potion.id);
    assert.ok(item, `${potion.name}: no item ${potion.id} in engine/data/items.tsv`);
    assert.equal(item.name, potion.name, `${potion.name}: item ${potion.id} is "${item.name}" in engine/data/items.tsv`);
    assert.equal(item.tab, "Use", `${potion.name}: item ${potion.id} isn't in the Use tab`);
    assert.equal(item.sell, potion.sell, `${potion.name}: engine/data/items.tsv sells it for ${item.sell}, potions.ts says ${potion.sell}`);
    const lowest = Math.min(...items.filter((i) => i.name === potion.name).map((i) => i.id));
    assert.equal(potion.id, lowest, `${potion.name}: use ${lowest}, the lowest ID with its name, so pickups match it`);
  }
});

test("each potion is listed once, with a shop price above its sell price", () => {
  assert.equal(new Set(POTIONS.map((p) => p.id)).size, POTIONS.length, "a potion is listed twice");
  for (const potion of POTIONS) {
    assert.ok(potion.amount > 0, `${potion.name}: amount must be positive`);
    assert.ok(potion.buy > potion.sell, `${potion.name}: a shop charges more than it pays`);
    assert.ok(potion.checked.length > 0, `${potion.name}: say where the shop price was checked`);
  }
});
