// The price label: the Free Market price of the item the player points at, flush under the
// game's own tooltip and as wide as it; or, while the name's letters are still being learned, a
// card saying so. The host reads the tooltip, looks the price up and places the window; this page
// only draws what it's told (app/src/label.rs). It's click-through and never takes the keyboard,
// and is never announced: the Prices tab is the accessible way in. Every word is a text node,
// never parsed HTML.
import { formatDigits } from "@mesovibe/sdk";

import { h, replace } from "./dom.js";
import { connectLabel } from "./ipc.js";
import type { LabelShown } from "./ipc.js";
import { moveOf, priceText } from "./prices.js";

const root = document.getElementById("label") as HTMLElement;

/** "Nov 23", for old prices' credit row. */
function day(ms: number): string {
  return new Date(ms).toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

function card(view: LabelShown): HTMLElement {
  const old = view.oldSinceMs !== null && view.oldSinceMs !== undefined;
  const rows: HTMLElement[] = [];
  if (view.learning) {
    // While the name's letters are being learned: the same card, saying so.
    rows.push(
      h("div", { class: "label-top label-learning", attrs: { "data-key": "label-learning" } }, view.learning.pick ? "Learning this name" : "Learning item names"),
      h("div", { class: "label-credit", attrs: { "data-key": "label-credit" } }, view.learning.pick ? "Pick it in the Prices tab" : "Point at a few more items"),
    );
  } else if (view.usual !== null && view.usual !== undefined) {
    const move = moveOf(view.change ?? null);
    rows.push(
      h(
        "div",
        { class: "label-top" },
        h("span", { class: "label-price number", attrs: { "data-key": "label-price" } }, priceText(view.usual)),
        h("span", { class: "label-unit" }, "usual"),
        // This copy can't be sold: what tradable copies sell for, muted, and the word.
        view.untradable
          ? h("span", { class: "label-move number", attrs: { "data-key": "label-untradable" } }, "Untradable")
          : // Without a week of prices there's no move to show, and a "—" would read as missing data.
            move.tone !== "none" &&
              h("span", { class: `label-move number move ${move.tone}`, attrs: { "data-key": "label-move" } }, `${move.shown} week`),
      ),
      h("div", { class: "label-credit", attrs: { "data-key": "label-credit" } }, `${view.world ?? ""} · Cozy Markets${old ? ` · from ${day(view.oldSinceMs as number)}` : ""}`),
    );
  } else {
    rows.push(
      h("div", { class: "label-top" }, h("span", { class: "label-price number", attrs: { "data-key": "label-price" } }, `NPC pays ${formatDigits(view.npc ?? 0)}`)),
      h("div", { class: "label-credit", attrs: { "data-key": "label-credit" } }, "No Free Market price"),
    );
  }
  const muted = old || view.untradable === true;
  const frame = h("div", { class: `label-frame${muted ? " stale" : ""}` }, h("div", { class: "label-card" }, ...rows));
  // Through the style properties: the app's security policy drops a style attribute, which left the
  // card as narrow as its text and off the tooltip's left edge.
  frame.style.left = `${view.cardLeft}px`;
  frame.style.width = `${view.cardWidth}px`;
  return frame;
}

void (async () => {
  const host = await connectLabel();
  await host.onView((view) => replace(root, card(view)));
})();
