// A game screen that stays dark. The engine calls a frame with nothing white where the HUD goes
// dark, which is normal for the second or two a map change fades to black, so the panel and the
// mini-HUD say nothing about it. When it lasts, MesoVibe isn't seeing the game's picture: nothing
// would ever be tracked, and a green light would hide that.
import type { Status } from "./ipc.js";

/** How long the screen may stay dark before MesoVibe says it can't see the EXP bar. */
export const DARK_WARN_MS = 10_000;

/**
 * Follows the statuses as they come and says whether the screen has been dark for
 * {@link DARK_WARN_MS}. A tick with no new frame reads nothing, so it neither starts nor ends a
 * dark spell; anything else read ends it.
 */
export function darkWatch(): (status: Status) => boolean {
  let since: number | null = null;
  return (status) => {
    const capturing = status.reading && status.state === "capturing";
    if (capturing && status.unreadable === "screen_dark") since ??= status.atMs;
    else if (!(capturing && status.stale)) since = null;
    return since !== null && status.atMs - since >= DARK_WARN_MS;
  };
}
