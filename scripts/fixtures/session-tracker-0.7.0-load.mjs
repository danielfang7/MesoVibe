// How the Session Tracker in MesoVibe 0.7.0 loaded its saved data: load() and the checks it calls,
// copied from plugins/src/session-tracker/index.ts and loot.ts at the v0.7.0 tag with the types
// taken out. Tests run a save from the current tracker through it, so a player who goes back to
// an older build keeps their sessions. Don't change it to match newer code: it stands for the
// released build.

function addCount(list, key, count) {
  const entry = list.find((e) => e.k === key);
  if (entry === undefined) list.push({ k: key, c: count });
  else entry.c += count;
}

function validMaps(value) {
  if (!Array.isArray(value)) return undefined;
  const count = (n) => typeof n === "number" && Number.isFinite(n) && n >= 0;
  const level = (n) => n === null || (typeof n === "number" && Number.isInteger(n) && n > 0);
  const maps = [];
  for (const raw of value) {
    if (typeof raw !== "object" || raw === null) continue;
    const e = raw;
    if (!(typeof e.k === "string" || e.k === null)) continue;
    if (![e.p, e.x, e.m, e.r, e.l].every(count) || !level(e.s) || !level(e.e)) continue;
    const entry = { k: e.k, p: e.p, x: e.x, m: e.m, r: e.r, l: e.l, s: e.s, e: e.e };
    if (count(e.sm) && e.sm > 0) entry.sm = e.sm;
    if (e.mi === true) entry.mi = true;
    if (e.pa === true) entry.pa = true;
    const items = validItems(e.i);
    if (items.length > 0) entry.i = items;
    if (count(e.rv) && e.rv > 0) entry.rv = e.rv;
    maps.push(entry);
  }
  return maps;
}

function validItems(value) {
  if (!Array.isArray(value)) return [];
  const items = [];
  for (const raw of value) {
    if (typeof raw !== "object" || raw === null) continue;
    const { k, c } = raw;
    if (typeof k === "string" && ITEM_KEY.test(k) && typeof c === "number" && Number.isInteger(c) && c > 0) addCount(items, k, c);
  }
  return items;
}

const ITEM_KEY = /^(#\d+|=.+|\?.*)$/s;

function validCatalog(value) {
  const catalog = Object.create(null);
  if (typeof value !== "object" || value === null || Array.isArray(value)) return catalog;
  const text = (x) => x === null || typeof x === "string";
  const mesos = (x) => typeof x === "number" && Number.isFinite(x) && x >= 0;
  const range = (x) => Array.isArray(x) && x.length === 2 && mesos(x[0]) && mesos(x[1]) && x[0] <= x[1];
  for (const [key, raw] of Object.entries(value)) {
    if (typeof raw !== "object" || raw === null) continue;
    const { n, t, v, r } = raw;
    if (!ITEM_KEY.test(key) || !text(n) || !text(t) || !(v === null || mesos(v))) continue;
    catalog[key] = range(r) ? { n, t, v, r: [r[0], r[1]] } : { n, t, v };
  }
  return catalog;
}

function validLoot(session) {
  const maps = validMaps(session.maps);
  if (maps === undefined) delete session.maps;
  else session.maps = maps;
  if (session.missed !== true) delete session.missed;
  if (session.shapesWrong !== true) delete session.shapesWrong;
  if (typeof session.unplaced !== "number" || !Number.isFinite(session.unplaced)) delete session.unplaced;
  const held = session.held;
  if (typeof held !== "object" || held === null || typeof held.mesos !== "number") delete session.held;
  else if (session.held !== undefined) {
    const items = validItems(session.held.items);
    if (items.length > 0) session.held.items = items;
    else delete session.held.items;
  }
  return session;
}

export function load(saved) {
  const empty = { version: 1, history: [], current: null };
  if (typeof saved !== "object" || saved === null) return empty;
  const value = saved;
  if (value.version !== 1) return empty;
  const allTime = value.allTime;
  const items = validItems(allTime?.items);
  return {
    version: 1,
    history: Array.isArray(value.history) ? value.history.filter((s) => typeof s === "object" && s !== null).map(validLoot) : [],
    current: typeof value.current === "object" && value.current !== null ? validLoot(value.current) : null,
    allTime:
      typeof allTime?.mesos === "number" && typeof allTime.playedMs === "number"
        ? { mesos: allTime.mesos, playedMs: allTime.playedMs, ...(items.length > 0 ? { items } : {}) }
        : { mesos: 0, playedMs: 0 },
    catalog: validCatalog(value.catalog),
  };
}
