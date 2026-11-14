#!/usr/bin/env node
// Refreshes the item and map data bundled in the engine (engine/data) from OSMS Data Explorer's
// public dataset, which is licensed CC BY 4.0. Run it after a game patch and commit what changed:
//
//   node scripts/game-data.mjs
//
// It keeps only what MesoVibe uses (names, inventory tabs, NPC sell prices and two flags), checks
// each download against the dataset's own sha256 list before writing anything, and writes the
// data folder's README with the credit, the license and the game build the data came from. The
// app never downloads this data itself: it ships inside each release.

import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const OUT = join(REPO, "engine/data");
export const SOURCE = "https://osmsdataexplorer.com/data/api/v1/";
const USER_AGENT = "MesoVibe game-data script (https://github.com/danielfang7/MesoVibe)";

/** The inventory tab each of the dataset's categories goes to, as the game's notices name it. */
const TABS = { Equipment: "Equip", Consumable: "Use", Setup: "Setup", Etc: "Etc" };
/** An item ID's first digit gives its tab too (1xxxxxx is equipment, and so on). */
const TAB_DIGIT = { Equip: 1, Use: 2, Setup: 3, Etc: 4 };

/** A field of a tab-separated line: no tabs or line breaks inside. */
function field(text, what) {
  if (/[\t\r\n]/.test(text)) {
    throw new Error(`${what} has a tab or line break: ${JSON.stringify(text)}`);
  }
  return text;
}

/** Whether a WZ flag is set: the dataset writes 1, and now and then "1". */
const isSet = (value) => Number(value) > 0;

/** The NPC sell price, or "" when shops won't buy the item or the game gives it no price. */
function sellPrice(item) {
  const price = Number(item.price);
  if (!Number.isInteger(price) || price <= 0 || isSet(item.stats?.notSale)) return "";
  return String(price);
}

/** items.json as the engine's items.tsv: id, tab, sell price, flags, name, sorted by id. */
export function itemsTsv(items) {
  const rows = items.map((item) => {
    const tab = TABS[item.category];
    if (!tab) throw new Error(`item ${item.id}: unknown category ${JSON.stringify(item.category)}`);
    if (Math.floor(item.id / 1_000_000) !== TAB_DIGIT[tab]) {
      throw new Error(`item ${item.id}: its ID doesn't belong in the ${tab} tab`);
    }
    if (!item.name) throw new Error(`item ${item.id}: no name`);
    const flags = [];
    if (isSet(item.stats?.quest)) flags.push("quest");
    if (isSet(item.stats?.tradeBlock)) flags.push("untradeable");
    return [item.id, tab, sellPrice(item), flags.join(","), field(item.name, `item ${item.id}'s name`)];
  });
  return tsv(["id", "tab", "sell", "flags", "name"], rows);
}

/** maps.json as the engine's maps.tsv: id, region (the minimap's first line), name. */
export function mapsTsv(maps) {
  const rows = maps
    .filter((map) => map.name)
    .map((map) => [
      map.id,
      field(map.street_name ?? "", `map ${map.id}'s region`),
      field(map.name, `map ${map.id}'s name`),
    ]);
  return tsv(["id", "region", "name"], rows);
}

function tsv(header, rows) {
  rows.sort((a, b) => a[0] - b[0]);
  for (let i = 1; i < rows.length; i++) {
    if (rows[i][0] === rows[i - 1][0]) throw new Error(`ID ${rows[i][0]} appears twice`);
  }
  return [header, ...rows].map((row) => row.join("\t")).join("\n") + "\n";
}

/** The data folder's README: what the files are, where they come from, and the credit. */
export function readme(meta) {
  return `# Game data

Classic World's item and map names, inventory tabs and NPC sell prices, bundled with the engine so
MesoVibe can name what you pick up and value it without any network request.

Data from [OSMS Data Explorer](https://osmsdataexplorer.com), licensed
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). MesoVibe keeps only the columns below
and leaves the sell price blank for items shops won't buy. MapleStory names and other game content
are © Nexon and not covered by that license. MesoVibe is not affiliated with Nexon or OSMS Data
Explorer.

Game build \`${meta.client_build}\`, data last changed ${meta.data_updated_at}.

## Files

- \`items.tsv\`: \`id\`, \`tab\` (Equip, Use, Setup or Etc), \`sell\` (mesos a shop pays for one; blank
  when shops won't buy it), \`flags\` (\`quest\`, \`untradeable\`), \`name\`
- \`maps.tsv\`: \`id\`, \`region\` (the minimap's first line, such as "Victoria Road"), \`name\`

## Refreshing

After a game patch, run \`node scripts/game-data.mjs\` and commit what changed. The script checks
each file against the dataset's published sha256 before writing.
`;
}

async function download(name) {
  const response = await fetch(new URL(name, SOURCE), { headers: { "User-Agent": USER_AGENT } });
  if (!response.ok) throw new Error(`${name}: HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

/** Downloads a data file and checks it against the sha256 meta.json lists for it. */
async function verified(meta, name) {
  const bytes = await download(name);
  const want = meta.files?.[name]?.sha256;
  const got = createHash("sha256").update(bytes).digest("hex");
  if (got !== want) throw new Error(`${name}: sha256 ${got}, but meta.json says ${want}`);
  return JSON.parse(bytes.toString("utf8"));
}

async function main() {
  const meta = JSON.parse((await download("meta.json")).toString("utf8"));
  if (meta.schema_version !== 1) {
    throw new Error(`meta.json: schema version ${meta.schema_version}; this script reads version 1`);
  }
  const items = itemsTsv(await verified(meta, "items.json"));
  const maps = mapsTsv(await verified(meta, "maps.json"));
  writeFileSync(join(OUT, "items.tsv"), items);
  writeFileSync(join(OUT, "maps.tsv"), maps);
  writeFileSync(join(OUT, "README.md"), readme(meta));
  const lines = (text) => text.split("\n").length - 2;
  console.log(`engine/data: ${lines(items)} items, ${lines(maps)} maps, game build ${meta.client_build}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
