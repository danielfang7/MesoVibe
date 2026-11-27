// GET /api/backfill: the one-off history backfill Vercel's scheduler runs every minute until every
// item is filled (vercel.json `crons`). See _lib/backfill.js.
import { blobStore } from "./_lib/blob-store.js";
import { backfill } from "./_lib/backfill.js";

export function GET(request) {
  return backfill({ request, env: process.env, fetchImpl: fetch, store: blobStore, now: new Date() });
}
