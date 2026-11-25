// GET /api/prices: the price file the MesoVibe app downloads. See _lib/prices.js.
import { blobStore } from "./_lib/blob-store.js";
import { serve } from "./_lib/prices.js";

export function GET(request) {
  return serve({ request, store: blobStore, now: new Date() });
}

export function HEAD(request) {
  return serve({ request, store: blobStore, now: new Date() });
}
