// The Session Tracker as shipped, counting potions from the quick slots
// (plugins/src/session-tracker/count-source.ts): a slot's count changing in the same moment HP or
// MP rises is one use, and the rise's size names the slot's potion.
import { test } from "node:test";
import assert from "node:assert/strict";

import { bundleTracker, run } from "./session-tracker-harness.mjs";

const shipped = await bundleTracker();

const MEAT = 2010003;
const LEMON = 2010005;
const T0 = 1_800_000_000_000;
const NEEDED = 356_916;
/** A host reading the HP and MP numbers, the meso total and the quick slots. */
const READING = { hpMp: "reading", hpMpMissing: null, mesos: "reading", slots: "reading" };

const event = (e, atMs) => ({ type: "event", atMs, event: e });
const exp = (value, atMs) =>
  event({ type: "ExpChanged", level: 42, exp: value, pct_hundredths: Math.round((value * 10_000) / NEEDED), needed: NEEDED }, atMs);
const hpMp = (hp, mp, atMs, continuous = true) => event({ type: "HpMpChanged", hp, max_hp: 1800, mp, max_mp: 1500, continuous }, atMs);
const slot = (index, count, icon, atMs) => event({ type: "QuickSlotChanged", slot: index, count_key: count, icon_key: icon }, atMs);
const tick = (atMs) => ({ type: "tick", atMs });
const last = (sent, type) => sent.filter((m) => m.type === type).at(-1);
const section = (sent, title) =>
  sent
    .filter((m) => m.type === "render" && m.page === undefined)
    .at(-1)
    .view.find((node) => node.title === title);
const potions = (sent) => section(sent, "Potions");
const used = (sent) =>
  Object.fromEntries((potions(sent)?.children.find((n) => n.kind === "table")?.rows ?? []).map(([name, count]) => [name, count]));
const lines = (sent) => potions(sent).children.filter((n) => n.kind !== "table" && n.kind !== "stat").map((n) => n.text);

/**
 * A session two minutes in on one map, gaining EXP every 10 s, with HP 1000 and MP 300 read every
 * second and the quick slots first read: Lemon (count "c274") in slot 0, Meat ("c377") in slot 6.
 * Returns the plugin, the time reached, and `use` to send what one second brings.
 */
function grinding({ readers = READING, saved = null } = {}) {
  const plugin = run(shipped);
  plugin.send({ type: "start", saved, atMs: T0, capture: "capturing", readers });
  plugin.send(event({ type: "MapChanged", map_key: "aaaa" }, T0));
  plugin.send(exp(230_000, T0));
  const state = { hp: 1000, mp: 300, at: T0 };
  const second = (messages = []) => {
    state.at += 1000;
    const s = (state.at - T0) / 1000;
    plugin.send(tick(state.at));
    if (s % 10 === 0) plugin.send(exp(230_000 + s * 6, state.at));
    for (const m of messages) plugin.send(m);
  };
  for (let s = 0; s < 120; s++) {
    second([hpMp(state.hp, state.mp, state.at + 1000)]);
    if (s === 3) {
      plugin.send(slot(0, "c274", "lemon-icon", state.at));
      plugin.send(slot(6, "c377", "meat-icon", state.at));
    }
  }
  /** One second: HP and MP rise by `hp` and `mp`, and the given slots read their new counts. */
  const use = ({ hp = 0, mp = 0, slots = [] } = {}) => {
    state.hp += hp;
    state.mp += mp;
    second([hpMp(state.hp, state.mp, state.at + 1000)]);
    // A slot's change is sent a frame after the rise shows.
    second([hpMp(state.hp, state.mp, state.at + 1000), ...slots.map(([i, count, icon]) => slot(i, count, icon, state.at + 1000))]);
  };
  return { ...plugin, state, second, use };
}

// A slot's count changing as HP rises by Meat's 160 is a Meat; as MP rises by Lemon's 150, a
// Lemon. Fails if a paired use is lost or goes to the wrong potion.
test("a slot's count changing as HP or MP rises by a potion's amount is one use of it", () => {
  const game = grinding();
  game.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  game.use({ mp: 150, slots: [[0, "c273", "lemon-icon"]] });
  game.use({ hp: 160, slots: [[6, "c375", "meat-icon"]] });
  assert.deepEqual(used(game.sent), { Meat: "×2", Lemon: "×1" });
  assert.deepEqual(potions(game.sent).children.at(-1), {
    kind: "text",
    text: "Counted from your quick slots, as their counts drop. Only Meat and Lemon are counted.",
    variant: "muted",
  });
});

// A count changing with no rise (potions picked up or bought) and a rise with no count changing
// (a Heal, natural recovery) are no use. Fails if either is counted.
test("a count change without a rise, or a rise without a count change, counts nothing", () => {
  const game = grinding();
  game.use({ slots: [[6, "c380", "meat-icon"]] });
  game.use({ hp: 160 });
  game.use({ mp: 150 });
  assert.deepEqual(used(game.sent), {});
});

// An icon is named only by a rise of its potion's amount, give or take natural recovery; once
// named, a use counts though HP tops up to full or a hit lands in the same frame. Fails if a
// stray rise names an icon, or a named icon's uses need the exact amount.
test("an icon's potion is learned from a rise of its amount, then counted loosely", () => {
  const game = grinding();
  game.use({ mp: 208, slots: [[0, "c273", "lemon-icon"]] });
  assert.deepEqual(used(game.sent), {}, "+208 MP is no Lemon yet");
  game.use({ mp: 150, slots: [[0, "c272", "lemon-icon"]] });
  game.use({ mp: 208, slots: [[0, "c271", "lemon-icon"]] });
  game.use({ mp: 90, slots: [[0, "c270", "lemon-icon"]] });
  game.use({ mp: 40, slots: [[0, "c269", "lemon-icon"]] });
  assert.deepEqual(used(game.sent), { Lemon: "×3" }, "named by +150, then +208 and +90 count, +40 doesn't");
  // MP rising to its max counts whatever the rise.
  game.use({ mp: 1500 - game.state.mp, slots: [[0, "c268", "lemon-icon"]] });
  assert.deepEqual(used(game.sent), { Lemon: "×4" });
});

// The potion each icon is is saved with the tracker's data and known from the first use next
// time. Fails if it's lost on restart or a damaged entry breaks loading.
test("which potion an icon is comes back after a restart", () => {
  const first = grinding();
  first.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  for (let i = 0; i < 6; i++) first.second();
  const saved = last(first.sent, "save").data;
  assert.deepEqual(saved.slots, { icons: [["meat-icon", MEAT]] });
  saved.slots.icons.push(["bad", 7], [1, MEAT], "junk");
  const second = grinding({ saved });
  second.use({ hp: 100, slots: [[6, "c375", "meat-icon"]] });
  assert.deepEqual(used(second.sent), { Meat: "×1" }, "a known icon counts a rise short of 160");
});

// A rise and a count change more than a second and a half apart are two things, not one use;
// the same item in two slots changes both, and one rise counts once. Fails on either.
test("a use is one rise and one change close together", () => {
  const game = grinding();
  game.use({ hp: 160 });
  game.second();
  game.second([slot(6, "c376", "meat-icon", game.state.at + 1000)]);
  assert.deepEqual(used(game.sent), {}, "too far apart");
  game.send(slot(3, "c376", "meat-icon", game.state.at));
  game.use({ hp: 160, slots: [[6, "c375", "meat-icon"], [3, "c375", "meat-icon"]] });
  assert.deepEqual(used(game.sent), { Meat: "×1" });
});

// A stack moved to another slot, or a slot whose count goes unreadable and back, is no use even
// with a rise. Fails if either is counted.
test("a moved stack or a covered count is no use", () => {
  const game = grinding();
  game.use({ hp: 160, slots: [[6, null, "empty-icon"], [5, "c377", "meat-icon"]] });
  game.use({ hp: 160, slots: [[5, null, "meat-icon"]] });
  game.use({ hp: 160, slots: [[5, "c377", "meat-icon"]] });
  assert.deepEqual(used(game.sent), {});
});

// While the quick slots can't be read, potions aren't counted and the Potions section says why.
// Fails if uses or time count while the slots are hidden, or the line is missing.
test("while the quick slots are hidden, potions aren't counted", () => {
  const game = grinding({ readers: { ...READING, slots: "not_found" } });
  game.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  assert.deepEqual(used(game.sent), {});
  assert.ok(lines(game.sent).includes("Potions aren't counted while your quick slots are hidden or covered."));
  const odd = grinding({ readers: { ...READING, slots: "unsupported" } });
  assert.ok(lines(odd.sent).includes("Can't read your quick slots at this game size, so potions aren't counted."));
});
