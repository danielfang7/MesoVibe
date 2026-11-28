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
  // The engine sends the HP and MP numbers when they change, so once here, then on each rise.
  for (let s = 0; s < 120; s++) {
    second(s === 0 ? [hpMp(state.hp, state.mp, state.at + 1000)] : []);
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
    second(slots.map(([i, count, icon]) => slot(i, count, icon, state.at + 1000)));
  };
  /**
   * Two quiet seconds, then a hit of 1 HP: a name settles once its last vote is 1.5 s old, at the
   * next event the engine sends.
   */
  const settle = () => {
    second();
    second();
    second([hpMp((state.hp -= 1), state.mp, state.at + 1000)]);
  };
  return { ...plugin, state, second, use, settle };
}

/** A save in which the Lemon and Meat icons are already named. */
const NAMED = { version: 1, history: [], current: null, slots: { icons: [["lemon-icon", LEMON], ["meat-icon", MEAT]] } };

// A named slot's count changing as HP rises is a Meat, as MP rises a Lemon. Fails if a paired use
// is lost or goes to the wrong potion.
test("a slot's count changing as HP or MP rises is one use of its potion", () => {
  const game = grinding({ saved: NAMED });
  game.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  game.use({ mp: 150, slots: [[0, "c273", "lemon-icon"]] });
  game.use({ hp: 160, slots: [[6, "c375", "meat-icon"]] });
  assert.deepEqual(used(game.sent), { Meat: "×2", Lemon: "×1" });
  assert.deepEqual(potions(game.sent).children.at(-1), {
    kind: "text",
    text: "Counted from your quick slots, as their counts drop. Only Meat, Lemon, Orange Potion, White Potion and Blue Potion are counted.",
    variant: "muted",
  });
});

// Orange, White and Blue Potions are learned from their amounts like Meat and Lemon, and priced at
// their shop price. Fails if one is named as another potion, a use is lost, or a cost is wrong.
test("Orange, White and Blue Potions are learned from what they restore", () => {
  const game = grinding();
  game.send(slot(2, "c50", "orange-icon", game.state.at));
  game.send(slot(3, "c50", "white-icon", game.state.at));
  game.send(slot(4, "c50", "blue-icon", game.state.at));
  game.use({ hp: 250, slots: [[2, "c49", "orange-icon"]] });
  game.use({ hp: 250, slots: [[2, "c48", "orange-icon"]] });
  game.use({ hp: -1000 });
  game.use({ hp: 500, slots: [[3, "c49", "white-icon"]] });
  game.use({ hp: 500, slots: [[3, "c48", "white-icon"]] });
  game.use({ mp: 200, slots: [[4, "c49", "blue-icon"]] });
  game.use({ mp: 200, slots: [[4, "c48", "blue-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), { "Orange Potion": "×2", "White Potion": "×2", "Blue Potion": "×2" });
  const table = potions(game.sent).children.find((n) => n.kind === "table");
  assert.deepEqual(
    table.rows.map(([name, , cost]) => [name, cost]),
    [
      ["Orange Potion", "300"],
      ["White Potion", "700"],
      ["Blue Potion", "440"],
    ],
  );
});

// A count changing with no rise (potions picked up or bought) and a rise with no count changing
// (a Heal, natural recovery) are no use. Fails if either is counted.
test("a count change without a rise, or a rise without a count change, counts nothing", () => {
  const game = grinding({ saved: NAMED });
  game.use({ slots: [[6, "c380", "meat-icon"]] });
  game.use({ hp: 160 });
  game.use({ mp: 150 });
  assert.deepEqual(used(game.sent), {});
});

// An icon is named by two rises of its potion's amount; its uses before that wait and count then,
// and after it a use counts on a loose rise. Fails if one rise names it, an earlier use is lost,
// or a named icon's uses need the exact amount.
test("an icon is named by two rises of its potion's amount, then counted loosely", () => {
  const game = grinding();
  game.use({ mp: 230, slots: [[0, "c273", "lemon-icon"]] });
  game.use({ mp: 150, slots: [[0, "c272", "lemon-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), {}, "one rise of 150 names nothing");
  game.use({ mp: 150, slots: [[0, "c271", "lemon-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), { Lemon: "×3" }, "named, with the +230 and +150 before");
  game.use({ mp: 90, slots: [[0, "c270", "lemon-icon"]] });
  game.use({ mp: 40, slots: [[0, "c269", "lemon-icon"]] });
  assert.deepEqual(used(game.sent), { Lemon: "×4" }, "+90 counts, +40 doesn't");
});

// A rise the max capped says nothing of the potion's amount: a bigger potion topping HP up by
// Meat's 160 names nothing. Fails if such rises name an icon.
test("a rise capped by the max names nothing", () => {
  const game = grinding();
  game.send(slot(1, "c100", "white-icon", game.state.at));
  for (const count of ["c99", "c98"]) {
    game.use({ hp: 1800 - 165 - game.state.hp });
    game.use({ hp: 165, slots: [[1, count, "white-icon"]] });
  }
  game.settle();
  for (let i = 0; i < 6; i++) game.second();
  assert.deepEqual(used(game.sent), {});
  assert.equal(last(game.sent, "save").data.slots?.icons.some(([icon]) => icon === "white-icon") ?? false, false);
});

// A rise near two unnamed items' count changes names neither, in whatever order the slots come,
// even for an item one vote from a name. Fails if the first slot takes the other's rise as a name.
test("a rise near two unnamed items' changes names neither", () => {
  const game = grinding();
  game.use({ hp: 160, slots: [[0, "c273", "lemon-icon"]] });
  game.use({ hp: 160, slots: [[0, "c272", "lemon-icon"], [6, "c376", "meat-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), {});
  game.use({ hp: 160, slots: [[6, "c375", "meat-icon"]] });
  game.use({ hp: 160, slots: [[6, "c374", "meat-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), { Meat: "×3" }, "two clean rises name Meat, and its use before counts");
  for (let i = 0; i < 6; i++) game.second();
  assert.deepEqual(last(game.sent, "save").data.slots.icons, [["meat-icon", MEAT]]);
});

// Two potions a moment apart show as one count change; both count. Fails if the second is lost.
test("two potions a moment apart count as two", () => {
  const game = grinding({ saved: NAMED });
  const { hp, mp, at } = game.state;
  // Four frames a second: two rises a quarter second apart, then the slot's one change.
  game.send(hpMp(hp + 160, mp, at + 250));
  game.send(hpMp(hp + 320, mp, at + 500));
  game.send(slot(6, "c375", "meat-icon", at + 750));
  assert.deepEqual(used(game.sent), { Meat: "×2" });
});

// Natural recovery topping HP up to full is no potion, even with a pickup just after it; a potion
// topping it up counts. Fails if either is miscounted.
test("natural recovery topping HP up isn't a use, a potion topping it up is", () => {
  const game = grinding({ saved: NAMED });
  game.use({ hp: 1800 - 5 - game.state.hp });
  game.use({ hp: 5, slots: [[6, "c390", "meat-icon"]] });
  assert.deepEqual(used(game.sent), {});
  game.use({ hp: -60 });
  game.use({ hp: 60, slots: [[6, "c389", "meat-icon"]] });
  assert.deepEqual(used(game.sent), { Meat: "×1" });
});

// The potion each icon is is saved with the tracker's data and known from the first use next
// time. Fails if it's lost on restart or a damaged entry breaks loading.
test("which potion an icon is comes back after a restart", () => {
  const first = grinding();
  first.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  first.use({ hp: 160, slots: [[6, "c375", "meat-icon"]] });
  first.settle();
  for (let i = 0; i < 6; i++) first.second();
  const saved = last(first.sent, "save").data;
  assert.deepEqual(saved.slots, { icons: [["meat-icon", MEAT]] });
  saved.slots.icons.push(["bad", 7], [1, MEAT], "junk");
  const second = grinding({ saved });
  second.use({ hp: 100, slots: [[6, "c374", "meat-icon"]] });
  assert.deepEqual(used(second.sent), { Meat: "×1" }, "a known icon counts a rise short of 160");
});

// A rise and a count change more than a second and a half apart are two things, not one use;
// the same item in two slots changes both, and one rise counts once. Fails on either.
test("a use is one rise and one change close together", () => {
  const game = grinding({ saved: NAMED });
  game.use({ hp: 160 });
  game.second();
  game.second([slot(6, "c376", "meat-icon", game.state.at + 1000)]);
  assert.deepEqual(used(game.sent), {}, "too far apart");
  game.send(slot(3, "c376", "meat-icon", game.state.at));
  game.use({ hp: 160, slots: [[6, "c375", "meat-icon"], [3, "c375", "meat-icon"]] });
  assert.deepEqual(used(game.sent), { Meat: "×1" });
});

// The same item in two slots, used before it's named: both counts wait with each rise, and naming
// it counts each rise once. Fails if one rise becomes two uses or two votes.
test("uses waiting for a name count each rise once", () => {
  const game = grinding();
  game.send(slot(3, "c274", "lemon-icon", game.state.at));
  game.use({ mp: 180, slots: [[0, "c273", "lemon-icon"], [3, "c273", "lemon-icon"]] });
  game.use({ mp: 150, slots: [[0, "c272", "lemon-icon"], [3, "c272", "lemon-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), {}, "one rise of 150, though two slots changed");
  game.use({ mp: 150, slots: [[0, "c271", "lemon-icon"], [3, "c271", "lemon-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), { Lemon: "×3" });
});

// A Heal just before a potion is no second potion, whether the potion rose by its amount or only
// topped HP up to full. Fails if a change counts a Heal's rise beside its own potion's.
test("a Heal just before a potion counts one potion", () => {
  const game = grinding({ saved: NAMED });
  const { hp, mp, at } = game.state;
  game.send(hpMp(hp + 300, mp, at + 250));
  game.send(hpMp(hp + 460, mp, at + 500));
  game.send(slot(6, "c376", "meat-icon", at + 750));
  assert.deepEqual(used(game.sent), { Meat: "×1" });
  game.send(hpMp(1800 - 100, mp, at + 5_000));
  game.send(hpMp(1800 - 60, mp, at + 5_250));
  game.send(hpMp(1800, mp, at + 5_500));
  game.send(slot(6, "c375", "meat-icon", at + 5_750));
  assert.deepEqual(used(game.sent), { Meat: "×2" }, "+40 then +60 to full: one potion");
});

// A rise a named icon's change counted never votes to name another icon that changed with it.
// Fails if a pickup at the moment of a Meat use is named Meat.
test("a rise a named icon counted names no other icon", () => {
  const game = grinding({ saved: { ...NAMED, slots: { icons: [["meat-icon", MEAT]] } } });
  game.send(slot(1, "c50", "red-icon", game.state.at));
  for (const [red, meat] of [["c51", "c376"], ["c52", "c375"]]) {
    game.second([hpMp((game.state.hp += 160), game.state.mp, game.state.at + 1000)]);
    game.second([slot(1, red, "red-icon", game.state.at + 1000), slot(6, meat, "meat-icon", game.state.at + 1000)]);
  }
  game.settle();
  assert.deepEqual(used(game.sent), { Meat: "×2" });
  for (let i = 0; i < 6; i++) game.second();
  assert.deepEqual(last(game.sent, "save").data.slots.icons, [["meat-icon", MEAT]]);
});

// If the count shows a frame before the HP number does, the rise comes in the change's own frame;
// a named icon takes it when no earlier rise fits. Fails if that use is lost.
test("a named icon's change takes a rise in its own frame when none came before", () => {
  const game = grinding({ saved: NAMED });
  const { hp, mp, at } = game.state;
  game.send(hpMp(hp + 160, mp, at + 250));
  game.send(slot(6, "c376", "meat-icon", at + 250));
  assert.deepEqual(used(game.sent), { Meat: "×1" });
  // Meat picked up in the frame of a Heal: the Heal's rise isn't a Meat's amount, so no use.
  game.send(hpMp(hp + 360, mp, at + 5_000));
  game.send(slot(6, "c386", "meat-icon", at + 5_000));
  assert.deepEqual(used(game.sent), { Meat: "×1" });
});

// A player drinking less than once every ten minutes still gets the icon named; only the uses
// that waited too long are dropped. Fails if the name never comes.
test("a potion drunk rarely still gets named", () => {
  const game = grinding();
  game.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  for (let i = 0; i < 660; i++) game.second();
  game.use({ hp: 160, slots: [[6, "c375", "meat-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), { Meat: "×1" }, "the first waited eleven minutes");
  game.use({ hp: 160, slots: [[6, "c374", "meat-icon"]] });
  assert.deepEqual(used(game.sent), { Meat: "×2" });
});

// A name that comes while no event does (a quiet stretch after the second use) still counts the
// uses that waited for it. Fails if they age out before the next event names the icon.
test("a name that comes in a quiet stretch still counts the uses that waited", () => {
  const game = grinding();
  game.use({ mp: 150, slots: [[0, "c273", "lemon-icon"]] });
  game.use({ mp: 150, slots: [[0, "c272", "lemon-icon"]] });
  for (let i = 0; i < 660; i++) game.second();
  game.second([hpMp(game.state.hp, (game.state.mp -= 1), game.state.at + 1000)]);
  assert.deepEqual(used(game.sent), { Lemon: "×2" });
});

// A smaller potion whose rises now and then reach Meat's amount with recovery isn't named Meat:
// rises short of an amount count against it. Fails if two lucky frames name it.
test("rises short of a potion's amount keep an icon from being named it", () => {
  const game = grinding();
  const rises = [150, 160, 150, 150, 165, 150];
  rises.forEach((hp, i) => {
    game.use({ hp: -hp });
    game.use({ hp, slots: [[1, `c${90 - i}`, "orange-icon"]] });
  });
  game.settle();
  for (let i = 0; i < 6; i++) game.second();
  assert.deepEqual(used(game.sent), {});
  assert.equal(last(game.sent, "save").data.slots?.icons.some(([icon]) => icon === "orange-icon") ?? false, false);
});

// A Lemon whose rises are often cut by a skill's MP cost in the same reading still gets named: a
// rise well short of the amount says nothing against it. Fails if those rises keep it unnamed.
test("rises cut well short by a skill's cost don't keep an icon unnamed", () => {
  const game = grinding();
  [130, 150, 130, 150].forEach((mp, i) => game.use({ mp, slots: [[0, `c${273 - i}`, "lemon-icon"]] }));
  game.settle();
  assert.deepEqual(used(game.sent), { Lemon: "×4" });
});

// A player who drinks Lemons while casting a 16 MP skill sees +134, never +150: those rises name
// the Lemon. Fails if a rise short by a skill's cost doesn't vote.
test("a Lemon drunk as a skill is cast is named from rises short by the skill's cost", () => {
  const game = grinding();
  for (const count of ["c273", "c272"]) {
    game.use({ mp: -16 });
    game.use({ mp: -16 });
    game.use({ mp: 134, slots: [[0, count, "lemon-icon"]] });
  }
  game.settle();
  assert.deepEqual(used(game.sent), { Lemon: "×2" });
});

// Only a skill's MP cost, seen repeating, makes up a short rise: not an MP drop seen once, not a
// repeating HP drop, and not for a rise just short of the amount. Fails if any of these names an icon.
test("only a skill's repeating MP cost makes up a rise short of a potion's amount", () => {
  const game = grinding();
  const quiet = () => {
    for (let i = 0; i < 12; i++) game.second();
  };
  const twice = (icon, slotIndex, before, rise) => {
    game.send(slot(slotIndex, "c51", icon, game.state.at));
    for (const count of ["c50", "c49"]) {
      for (const drop of before) game.use(drop);
      game.use({ ...rise, slots: [[slotIndex, count, icon]] });
      quiet();
    }
  };
  twice("once-icon", 1, [{ mp: -16 }], { mp: 134 });
  twice("hp-icon", 2, [{ hp: -20 }, { hp: -20 }], { hp: 140 });
  twice("smaller-icon", 3, [{ mp: -5 }, { mp: -5 }], { mp: 145 });
  game.settle();
  assert.deepEqual(used(game.sent), {});
  assert.deepEqual(last(game.sent, "save").data.slots?.icons ?? [], []);
});

// An icon with enough votes is named and saved though its waiting uses were dropped at Resume, so
// its next use counts at once. Fails if the name waits for another change.
test("an icon is named on its votes after its waiting uses were dropped", () => {
  const game = grinding();
  game.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  game.use({ hp: 160, slots: [[6, "c375", "meat-icon"]] });
  game.send({ type: "action", action: "pause", atMs: game.state.at });
  game.send({ type: "action", action: "resume", atMs: game.state.at + 1 });
  game.settle();
  assert.deepEqual(used(game.sent), {}, "the two before Pause aren't counted");
  for (let i = 0; i < 6; i++) game.second();
  assert.deepEqual(last(game.sent, "save").data.slots.icons, [["meat-icon", MEAT]], "saved before its next use");
  game.use({ hp: 160, slots: [[6, "c374", "meat-icon"]] });
  assert.deepEqual(used(game.sent), { Meat: "×1" });
});

// Uses held back for a name aren't counted after Pause: a potion drunk before Pause, or while
// paused, isn't counted after Resume. Fails if it is.
test("uses waiting for a name are dropped when the session pauses", () => {
  const game = grinding();
  game.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  game.send({ type: "action", action: "pause", atMs: game.state.at });
  game.send({ type: "action", action: "resume", atMs: game.state.at + 1 });
  game.use({ hp: 160, slots: [[6, "c375", "meat-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), { Meat: "×1" }, "named by both, counting only the use after Resume");
});

// A stack moved to another slot, or a slot whose count goes unreadable and back, is no use even
// with a rise. Fails if either is counted.
test("a moved stack or a covered count is no use", () => {
  const game = grinding({ saved: NAMED });
  game.use({ hp: 160, slots: [[6, null, "empty-icon"], [5, "c377", "meat-icon"]] });
  game.use({ hp: 160, slots: [[5, null, "meat-icon"]] });
  game.use({ hp: 160, slots: [[5, "c377", "meat-icon"]] });
  assert.deepEqual(used(game.sent), {});
});

// A use waits for its icon's name ten minutes at most, and the icons kept are the 32 used most
// recently. Fails if an old use counts, or the list grows or drops an icon in use.
test("what the source keeps has limits", () => {
  const game = grinding();
  game.use({ mp: 208, slots: [[0, "c273", "lemon-icon"]] });
  for (let i = 0; i < 601; i++) game.second();
  game.use({ mp: 150, slots: [[0, "c272", "lemon-icon"]] });
  game.use({ mp: 150, slots: [[0, "c271", "lemon-icon"]] });
  game.settle();
  assert.deepEqual(used(game.sent), { Lemon: "×2" }, "the +208 waited too long");
  const icons = [["meat-icon", MEAT], ...Array.from({ length: 31 }, (_, i) => [`old-${i}`, MEAT])];
  const full = grinding({ saved: { ...NAMED, slots: { icons } } });
  full.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  full.use({ mp: 150, slots: [[0, "c273", "lemon-icon"]] });
  full.use({ mp: 150, slots: [[0, "c272", "lemon-icon"]] });
  full.settle();
  for (let i = 0; i < 6; i++) full.second();
  const kept = last(full.sent, "save").data.slots.icons.map(([icon]) => icon);
  assert.equal(kept.length, 32);
  assert.ok(kept.includes("meat-icon") && kept.includes("lemon-icon"));
  assert.ok(!kept.includes("old-0"), "the one used longest ago goes");
});

// While the quick slots are hidden, potions aren't counted and the Potions section says why; with
// reading off, past sessions' potions still show. Fails if uses count while hidden, the line is
// missing, or saved potions vanish while reading is off.
test("while the quick slots are hidden, potions aren't counted", () => {
  const game = grinding({ readers: { ...READING, slots: "not_found" }, saved: NAMED });
  game.use({ hp: 160, slots: [[6, "c376", "meat-icon"]] });
  assert.deepEqual(used(game.sent), {});
  assert.ok(lines(game.sent).includes("Potions aren't counted while your quick slots are hidden or covered."));
  const odd = grinding({ readers: { ...READING, slots: "unsupported" } });
  assert.ok(lines(odd.sent).includes("Can't read your quick slots at this game size, so potions aren't counted."));
  const past = {
    startedAtMs: T0 - 3_600_000,
    endedAtMs: T0 - 3_000_000,
    activeMs: 600_000,
    expGained: 3_780,
    expLost: 0,
    startLevel: 42,
    endLevel: 42,
    maps: [{ k: "aaaa", p: 600_000, x: 3_780, m: 0, r: 0, l: 0, s: 42, e: 42, u: [{ k: MEAT, c: 10 }], h: 600_000 }],
    ps: { [MEAT]: "slots" },
    pt: 800,
  };
  const off = run(shipped);
  off.send({ type: "start", saved: { version: 1, history: [past], current: null }, atMs: T0, capture: "capturing", readers: { hpMp: "off", hpMpMissing: null } });
  const table = off.sent
    .filter((m) => m.type === "render" && m.page === "loot")
    .at(-1)
    .view.find((n) => n.title === "Past sessions")
    .children.find((n) => n.kind === "table");
  assert.equal(table.details[0].at(-1), "Pots 800: Meat ×10");
});
