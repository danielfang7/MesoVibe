# Game data

Classic World's item and map names, inventory tabs and NPC sell prices, bundled with the engine so
MesoVibe can name what you pick up and value it without any network request.

Data from [OSMS Data Explorer](https://osmsdataexplorer.com), licensed
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/). MesoVibe keeps only the columns below
and leaves the sell price blank for items shops won't buy. MapleStory names and other game content
are © Nexon and not covered by that license. MesoVibe is not affiliated with Nexon or OSMS Data
Explorer.

Game build `697c569e0500f56b6cc53e329dcc57bda728c862`, data last changed 2026-11-24T19:06:14Z.

## Files

- `items.tsv`: `id`, `tab` (Equip, Use, Setup or Etc), `sell` (mesos a shop pays for one; blank
  when shops won't buy it), `flags` (`quest`, `untradeable`), `name`
- `maps.tsv`: `id`, `region` (the minimap's first line, such as "Victoria Road"), `name`

## Refreshing

After a game patch, run `node scripts/game-data.mjs` and commit what changed. The script checks
each file against the dataset's published sha256 before writing.
