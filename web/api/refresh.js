// GET /api/refresh: the price job Vercel's scheduler runs every 6 hours (vercel.json `crons`).
// See _lib/prices.js.
import { blobStore } from "./_lib/blob-store.js";
import { refresh } from "./_lib/prices.js";

export function GET(request) {
  return refresh({ request, env: process.env, fetchImpl: fetch, store: blobStore, now: new Date() });
}
