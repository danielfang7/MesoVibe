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
- `tooltip-seed.txt`: the item-name letter shapes the tooltip reader starts from (below)
- `LiberationSans-OFL.txt`: the copyright and license of the font they're drawn from

## Letter shapes

`tooltip-seed.txt` holds letter shapes drawn from
[Liberation Sans](https://github.com/liberationfonts/liberation-fonts) Bold 2.1.5, digitized data
copyright (c) 2010 Google Corporation, copyright (c) 2012 Red Hat, Inc., licensed under the
[SIL Open Font License 1.1](LiberationSans-OFL.txt). The tooltip reader takes them as guesses at
the game's item-name letters until the game shows it each one. Nothing in them comes from the
game. `node scripts/tooltip-seed.mjs` draws them again from the font's official release, checked
against its sha256 first, and isn't part of refreshing the game data.

## Refreshing

After a game patch, run `node scripts/game-data.mjs` and commit what changed. The script checks
each file against the dataset's published sha256 before writing.
