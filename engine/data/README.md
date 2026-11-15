# Game data

Classic World's item and map names, inventory tabs and NPC sell prices, bundled with the engine so
MesoVibe can name what you pick up and value it without any network request.

Data from [OSMS Data Explorer](https://osmsdataexplorer.com), licensed
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). MesoVibe keeps only the columns below
and leaves the sell price blank for items shops won't buy. MapleStory names and other game content
are © Nexon and not covered by that license. MesoVibe is not affiliated with Nexon or OSMS Data
Explorer.

Game build `ff3827e3105012aca0a590cda46d17f356001fad`, data last changed 2026-10-29T15:56:20Z.

## Files

- `items.tsv`: `id`, `tab` (Equip, Use, Setup or Etc), `sell` (mesos a shop pays for one; blank
  when shops won't buy it), `flags` (`quest`, `untradeable`), `name`
- `maps.tsv`: `id`, `region` (the minimap's first line, such as "Victoria Road"), `name`

## Refreshing

After a game patch, run `node scripts/game-data.mjs` and commit what changed. The script checks
each file against the dataset's published sha256 before writing.
