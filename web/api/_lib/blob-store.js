// The price service's storage: two private files in the Vercel Blob store connected to this
// project (BLOB_READ_WRITE_TOKEN). Private, so only this project's functions can read them; players
// get the price file through /api/prices.
import { get, put } from "@vercel/blob";

/** A read that takes longer than this fails rather than holding the function open. */
const READ_TIMEOUT_MS = 10_000;

export const blobStore = {
  /** The file's text, or null when it doesn't exist yet. `fresh` skips Blob's own cache. */
  async read(pathname, { fresh = false } = {}) {
    const result = await get(pathname, {
      access: "private",
      useCache: !fresh,
      abortSignal: AbortSignal.timeout(READ_TIMEOUT_MS),
    });
    if (result === null) return null;
    return await new Response(result.stream).text();
  },
  async write(pathname, text) {
    await put(pathname, text, {
      access: "private",
      allowOverwrite: true,
      contentType: "application/json",
      // The shortest Blob allows, so /api/prices sees a new file within a minute.
      cacheControlMaxAge: 60,
    });
  },
};
