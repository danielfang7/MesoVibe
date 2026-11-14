// Protects game-data.mjs, which turns OSMS Data Explorer's dataset into the engine's bundled
// items.tsv and maps.tsv. A wrong tab or sell price here would mislabel or misvalue every pickup.
import { test } from "node:test";
import assert from "node:assert/strict";

import { itemsTsv, mapsTsv, readme } from "./game-data.mjs";

const lines = (tsv) => tsv.trimEnd().split("\n").map((line) => line.split("\t"));

// Each category goes to its game tab, rows come out sorted by ID, and the flags are kept.
test("items keep their tab, sell price and flags, sorted by id", () => {
  const out = lines(
    itemsTsv([
      { id: 4000010, name: "Pig's Ribbon", category: "Etc", price: 10 },
      { id: 2000001, name: "Orange Potion", category: "Consumable", price: 80 },
      { id: 4031013, name: "Dark Marble", category: "Etc", price: 1, stats: { quest: 1, tradeBlock: "1" } },
    ]),
  );
  assert.deepEqual(out, [
    ["id", "tab", "sell", "flags", "name"],
    ["2000001", "Use", "80", "", "Orange Potion"],
    ["4000010", "Etc", "10", "", "Pig's Ribbon"],
    ["4031013", "Etc", "1", "quest,untradeable", "Dark Marble"],
  ]);
});

// Shops won't buy a notSale item, so it must have no sell value, whatever price the game file has;
// a zero, fractional or non-numeric price is no price either.
test("an item shops won't buy, or with no price, has a blank sell price", () => {
  const out = lines(
    itemsTsv([
      { id: 4000001, name: "Snail Shell", category: "Etc", price: 1, stats: { notSale: 1 } },
      { id: 4000002, name: "Blue Snail Shell", category: "Etc" },
      { id: 1002107, name: "Pink Jester", category: "Equipment", price: "1800" },
      { id: 4000003, name: "Tree Branch", category: "Etc", price: 0 },
      { id: 4000004, name: "Squishy Liquid", category: "Etc", price: 2.5 },
      { id: 4000005, name: "Leaf", category: "Etc", price: "free" },
    ]),
  );
  assert.deepEqual(
    out.slice(1).map((row) => [row[4], row[2]]),
    [
      ["Pink Jester", "1800"],
      ["Snail Shell", ""],
      ["Blue Snail Shell", ""],
      ["Tree Branch", ""],
      ["Squishy Liquid", ""],
      ["Leaf", ""],
    ],
  );
});

// A reshaped dataset must stop the script instead of writing a broken file.
test("unexpected data stops the script", () => {
  const bad = [
    [{ id: 5000000, name: "Pet", category: "Cash" }],
    [{ id: 2000001, name: "Orange Potion", category: "Etc" }],
    [{ id: 4000010, name: "Pig's\tRibbon", category: "Etc" }],
    [{ id: 4000010, name: "", category: "Etc" }],
    [
      { id: 4000010, name: "A", category: "Etc" },
      { id: 4000010, name: "B", category: "Etc" },
    ],
  ];
  for (const items of bad) assert.throws(() => itemsTsv(items), JSON.stringify(items));
});

// Maps keep the minimap's two lines; a map with no name can't be matched, so it's left out.
test("maps keep region and name, and skip maps with no name", () => {
  const out = lines(
    mapsTsv([
      { id: 10, name: "Mushroom Town", street_name: "Maple Road" },
      { id: 10006121, name: "", street_name: "" },
      { id: 1, name: "Mushroom Town - West Entrance" },
    ]),
  );
  assert.deepEqual(out, [
    ["id", "region", "name"],
    ["1", "", "Mushroom Town - West Entrance"],
    ["10", "Maple Road", "Mushroom Town"],
  ]);
});

// CC BY 4.0 requires the credit and the license link wherever the data goes.
test("the data folder's README credits the source and names the game build", () => {
  const text = readme({ client_build: "abc123", data_updated_at: "2026-10-29T15:56:20Z" });
  assert.match(text, /OSMS Data Explorer\]\(https:\/\/osmsdataexplorer\.com\)/);
  assert.match(text, /creativecommons\.org\/licenses\/by\/4\.0/);
  assert.match(text, /abc123/);
});
