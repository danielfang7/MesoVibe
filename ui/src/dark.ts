// Spells of not reading the game that last. The engine calls a frame with nothing white where the
// HUD goes dark, which is normal for the second or two a map change fades to black, so the panel
// and the mini-HUD say nothing about it. When it lasts, MesoVibe isn't seeing the game's picture:
// nothing would ever be tracked, and a green light would hide that.
//
// In Full Screen the game scales its picture to the display, and at some of its sizes MesoVibe
// can't read the result yet: every frame then looks dark or has no HUD where one should be. The
// player can fix that in the game's options, so after the same wait MesoVibe says how.
import type { Status } from "./ipc.js";

/** How long the screen may stay dark, or Full Screen unread, before MesoVibe says so. */
export const DARK_WARN_MS = 10_000;

/** Reasons the EXP bar wasn't read that mean no HUD was seen. Learning isn't one: it reads the HUD. */
const UNSEEN = new Set(["screen_dark", "hud_not_found", "no_text", "unknown_glyph", "garbled", "unsupported_size", "unsupported_scale"]);

/**
 * Follows the statuses as they come and says whether `inSpell` has held for {@link DARK_WARN_MS}
 * while capturing. A tick with no new frame reads nothing, so it neither starts nor ends a spell;
 * anything else ends it.
 */
function spellWatch(inSpell: (status: Status) => boolean): (status: Status) => boolean {
  let since: number | null = null;
  return (status) => {
    const capturing = status.reading && status.state === "capturing";
    if (capturing && inSpell(status)) since ??= status.atMs;
    else if (!(capturing && status.stale)) since = null;
    return since !== null && status.atMs - since >= DARK_WARN_MS;
  };
}

/** Whether the screen has been dark for {@link DARK_WARN_MS}. */
export function darkWatch(): (status: Status) => boolean {
  return spellWatch((status) => status.unreadable === "screen_dark");
}

/**
 * Whether the game has been in Full Screen with no HUD seen for {@link DARK_WARN_MS}: the size it's
 * scaled from may be one MesoVibe can't read yet. Leaving Full Screen or reading anything ends it.
 */
export function fullScreenWatch(): (status: Status) => boolean {
  return spellWatch((status) => status.fullScreen && status.unreadable !== null && UNSEEN.has(status.unreadable));
}
