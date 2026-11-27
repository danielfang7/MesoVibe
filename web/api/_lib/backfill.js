// The one-off history backfill: fills each item's last 30 days once from Cozy's per-item answer,
// so week moves, the 30-day chart and Biggest moves work before the price job's own day snapshots
// cover a week. It runs on its own one-minute schedule (vercel.json), a few items a run within
// Cozy's 60 requests a minute for that endpoint, and keeps its own file: the price job reads it for
// days it hasn't kept itself, so the two jobs never write the same file. Once every item is filled
// (or given up after a few tries), a run does nothing, and the schedule can be removed.
//
// Cozy's per-item answer isn't documented publicly. Only a history this file recognizes fills an
// item; any other answer counts as a failed try, and that item fills from the day snapshots as
// before.
import { BACKFILL_PATH, FILE_PATH, WORLDS, dayOf, getCozy, json, readBackfill, shiftDay, signedWith } from "./prices.js";

/** Every minute, until it's removed once everything is filled. vercel.json repeats it. */
export const BACKFILL_SCHEDULE = "* * * * *";
/** Items asked for in one run: under Cozy's 60 a minute for the endpoint, with room to spare. */
export const PER_RUN = 50;
/** A run stops asking after this long, so it ends before the next one starts. */
export const BUDGET_MS = 45_000;
/** A run that started less than this long ago is still going: another run leaves it be. */
export const LEASE_MS = 55_000;
/** Failed answers before an item is left to the day snapshots. */
export const MAX_TRIES = 3;
/** Days of history kept per item, like the price file's chart. */
const DAYS = 30;

/**
 * GET /api/backfill, run by Vercel's scheduler, which signs it with CRON_SECRET. Asks Cozy for the
 * history of up to PER_RUN items not filled yet, in the order the price file lists them, and
 * records what it gets in its own file.
 */
export async function backfill({ request, env, fetchImpl, store, now, clock = () => Date.now(), log = console }) {
  const secret = env.CRON_SECRET;
  const key = env.COZY_MARKET_API_KEY;
  if (!secret || !key) {
    log.error("backfill: CRON_SECRET or COZY_MARKET_API_KEY is missing");
    return json(500, { error: "The backfill isn't set up." });
  }
  if (!signedWith(request, secret)) {
    return json(401, { error: "Not allowed." });
  }

  const state = readBackfill(await store.read(BACKFILL_PATH, { fresh: true }), log);
  if (state.lease !== null && now.getTime() - Date.parse(state.lease) < LEASE_MS) {
    return json(200, { busy: true });
  }
  const file = parseFile(await store.read(FILE_PATH, { fresh: true }));
  if (file === null) {
    return json(200, { waiting: "no price file yet" });
  }
  const pending = [];
  for (const world of WORLDS) {
    for (const item of file.worlds?.[world]?.items ?? []) {
      if (!Number.isInteger(item?.game_id) || !Number.isInteger(item?.item_id)) continue;
      const kept = state.worlds[world]?.[item.game_id];
      if (kept?.days || (kept?.tries ?? 0) >= MAX_TRIES) continue;
      pending.push({ world, item });
    }
  }
  if (pending.length === 0) {
    return json(200, { done: true });
  }

  // The lease: an overlapping run sees it and leaves the file alone.
  state.lease = now.toISOString();
  await store.write(BACKFILL_PATH, JSON.stringify(state));
  const started = clock();
  let filled = 0;
  let failed = 0;
  for (const { world, item } of pending.slice(0, PER_RUN)) {
    if (clock() - started > BUDGET_MS) break;
    const path = `/items/${item.item_id}`;
    const answer = await getCozy(fetchImpl, key, path, world);
    const days = answer.problem ? null : historyOf(answer.body, dayOf(now.toISOString()));
    const worldState = (state.worlds[world] ??= {});
    if (days === null) {
      const tries = (worldState[item.game_id]?.tries ?? 0) + 1;
      worldState[item.game_id] = { tries };
      failed += 1;
      if (tries === MAX_TRIES) log.warn?.(`backfill: ${world} ${path}: ${answer.problem ?? "no history it recognizes"}; left to the day snapshots`);
    } else {
      worldState[item.game_id] = { days };
      filled += 1;
    }
  }
  state.lease = null;
  await store.write(BACKFILL_PATH, JSON.stringify(state));
  return json(200, { filled, failed, left: pending.length - filled - failed });
}

/**
 * The days → prices an item's answer gives, at most the last 30 days up to `today`; `null` when it
 * isn't a history this recognizes. Two shapes are read: a list of `{ day | date, usual | price }`,
 * or a list of prices with the matching `history_days`, oldest first (the price file's own shape).
 */
export function historyOf(body, today) {
  const oldest = shiftDay(today, -(DAYS - 1));
  const source = body?.item ?? body;
  const days = {};
  const keep = (day, price) => {
    if (typeof day === "string" && /^\d{4}-\d{2}-\d{2}/.test(day) && isPrice(price)) {
      const d = day.slice(0, 10);
      if (d >= oldest && d <= today) days[d] = price;
    }
  };
  const history = source?.history;
  if (!Array.isArray(history)) return null;
  if (history.every((h) => h !== null && typeof h === "object")) {
    for (const h of history) keep(h.day ?? h.date, h.usual ?? h.price);
  } else if (Array.isArray(source.history_days) && source.history_days.length === history.length) {
    history.forEach((price, i) => keep(source.history_days[i], price));
  } else {
    return null;
  }
  return Object.keys(days).length > 0 ? days : null;
}

const isPrice = (value) => typeof value === "number" && Number.isFinite(value) && value > 0;

function parseFile(text) {
  if (text === null) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}
