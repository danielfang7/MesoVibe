// The price label: the Free Market price of the item the player points at, flush under the
// game's own tooltip and as wide as it. The host reads the tooltip, looks the price up and places
// the window; this page only draws what it's told (app/src/label.rs). It's click-through and never
// takes the keyboard, and is never announced: the Prices tab is the accessible way in. Every word
// is a text node, never parsed HTML.
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
  const old = view.oldSinceMs !== null;
  const rows: HTMLElement[] = [];
  if (view.usual !== null) {
    const move = moveOf(view.change);
    rows.push(
      h(
        "div",
        { class: "label-top" },
        h("span", { class: "label-price number", attrs: { "data-key": "label-price" } }, priceText(view.usual)),
        h("span", { class: "label-unit" }, "usual"),
        h("span", { class: `label-move number move ${move.tone}`, attrs: { "data-key": "label-move" } }, move.tone === "none" ? "—" : `${move.shown} week`),
      ),
      h("div", { class: "label-credit", attrs: { "data-key": "label-credit" } }, `${view.world} · Cozy Markets${old ? ` · from ${day(view.oldSinceMs as number)}` : ""}`),
    );
  } else {
    rows.push(
      h("div", { class: "label-top" }, h("span", { class: "label-price number", attrs: { "data-key": "label-price" } }, `NPC pays ${formatDigits(view.npc ?? 0)}`)),
      h("div", { class: "label-credit", attrs: { "data-key": "label-credit" } }, "No Free Market price"),
    );
  }
  return h(
    "div",
    { class: `label-frame${old ? " stale" : ""}`, attrs: { style: `left: ${view.cardLeft}px; width: ${view.cardWidth}px` } },
    h("div", { class: "label-card" }, ...rows),
  );
}

void (async () => {
  const host = await connectLabel();
  await host.onView((view) => replace(root, card(view)));
})();
