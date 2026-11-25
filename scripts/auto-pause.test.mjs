// The Session Tracker pausing and ending sessions on its own (on unless turned off in Settings): a
// minute with no EXP on the map pauses the session from the last kill and the next kill resumes it;
// a minute off that map with no EXP ends it from when the player left. Bundled from source and
// driven only through the SDK's messages, like session-tracker-plugin.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";

import { bundleTracker, run } from "./session-tracker-harness.mjs";

const bundle = await bundleTracker();
const synthetic = await bundleTracker({ synthetic: true });

const T0 = 1_800_000_000_000;
const NEEDED = 356_916;
const READING = { hpMp: "off", hpMpMissing: null, mesos: "reading" };
const OFF = { version: 1, history: [], current: null, auto: false };

const event = (e, atMs) => ({ type: "event", atMs, event: e });
const action = (name, atMs) => ({ type: "action", action: name, atMs });
const last = (sent, type) => sent.filter((m) => m.type === type).at(-1);
const summary = (sent) => last(sent, "summary").summary;
const history = (sent) => last(sent, "save").data.history;
const lootFigures = (sent) =>
  Object.fromEntries(
    sent
      .filter((m) => m.type === "render" && m.page === undefined)
      .at(-1)
      .view.find((node) => node.title === "Loot")
      ?.children.find((n) => n.kind === "keyValue")?.items ?? [],
  );

/**
 * A plugin reading on map "aaaa" from `saved`, a second at a time: `kill` gains 63 EXP, `idle`
 * passes seconds with none, `goTo` changes map, `total` reads the meso total, `capture` changes
 * the capture state. `noMap`: no map name read yet. `pots`: the stand-in potion counter, with the
 * HP and MP numbers read.
 */
function session(saved = null, { noMap = false, pots = false } = {}) {
  const plugin = run(pots ? synthetic : bundle);
  const state = { at: T0, exp: 230_000 };
  const exp = () =>
    event({ type: "ExpChanged", level: 42, exp: state.exp, pct_hundredths: Math.round((state.exp * 10_000) / NEEDED), needed: NEEDED }, state.at);
  const readers = pots ? { ...READING, hpMp: "reading" } : READING;
  plugin.send({ type: "start", saved, atMs: T0, capture: "capturing", readers });
  if (!noMap) plugin.send(event({ type: "MapChanged", map_key: "aaaa" }, T0));
  plugin.send(exp());
  const second = () => {
    state.at += 1000;
    plugin.send({ type: "tick", atMs: state.at });
  };
  const idle = (seconds) => {
    for (let s = 0; s < seconds; s++) second();
  };
  const kill = () => {
    second();
    state.exp += 63;
    plugin.send(exp());
  };
  /** `seconds` of play with a kill every `every` seconds, ending on a kill. */
  const grind = (seconds, every = 5) => {
    for (let s = 1; s <= seconds; s++) {
      if (s % every === 0) kill();
      else second();
    }
  };
  const goTo = (key) => plugin.send(event({ type: "MapChanged", map_key: key }, state.at));
  const total = (amount) => plugin.send(event({ type: "MesoChanged", total: amount, source: "inventory" }, state.at));
  const capture = (to) => plugin.send({ type: "capture", state: to, atMs: state.at });
  return { ...plugin, state, idle, kill, grind, goTo, total, capture };
}

// AFK on the grind map: after a minute with no EXP the session pauses, keeping one kill's worth
// of time after the last gain, and the next kill resumes it and counts. Fails if the idle minute
// stays in the session's time, the pause waits for a press, or the kill after it is lost.
test("a minute with no EXP on the map pauses the session from the last kill, and a kill resumes it", () => {
  const game = session();
  game.grind(120);
  game.idle(59);
  assert.equal(summary(game.sent).paused, false, "not before a minute");
  game.idle(31);
  assert.deepEqual([summary(game.sent).paused, summary(game.sent).autoPaused], [true, true]);
  assert.equal(summary(game.sent).activeMs, 125_000, "two minutes of grinding and one kill's worth after it");
  const gained = summary(game.sent).expGained;
  game.idle(120);
  game.kill();
  assert.deepEqual([summary(game.sent).paused, summary(game.sent).autoPaused], [false, false]);
  assert.equal(summary(game.sent).expGained, gained + 63, "the kill that resumed it counts");
  assert.equal(summary(game.sent).activeMs, 125_000, "the time away doesn't");
  game.grind(10);
  assert.equal(summary(game.sent).activeMs, 135_000);
});

// Left the grind map for a minute with no EXP: the session ends and is saved with its time and end
// as of leaving, and the next kill starts a new one. Fails if the time off the map counts, the
// session keeps running, or the next kill joins it.
test("a minute off the map with no EXP ends the session from when the player left", () => {
  const game = session();
  game.grind(120);
  game.idle(10);
  game.goTo("bbbb");
  game.idle(59);
  assert.equal(history(game.sent).length, 0, "not before a minute");
  game.idle(1);
  const [past] = history(game.sent);
  assert.equal(past.activeMs, 130_000, "the grinding and the time on the map after it");
  assert.equal(past.endedAtMs, T0 + 130_000);
  assert.deepEqual(
    past.maps.map((m) => m.k),
    ["aaaa"],
    "the map left for isn't one it played",
  );
  const notice = last(game.sent, "notice");
  assert.match(notice.text, /^Ended when you left the map\. Saved: /);
  assert.equal(notice.undo, "undo-end");
  game.grind(30);
  assert.equal(history(game.sent).length, 1);
  assert.equal(summary(game.sent).expGained, 6 * 63, "a new session");
});

// Paused on its own and then left: the session ends with the time it had at the pause. Fails if
// leaving after a pause keeps it paused forever, or gives back the time the pause took away.
test("a session paused on its own and then left ends as it was at the pause", () => {
  const game = session();
  game.grind(120);
  game.idle(70);
  game.goTo("bbbb");
  game.idle(60);
  const [past] = history(game.sent);
  assert.equal(past.activeMs, 125_000);
  assert.equal(past.endedAtMs, T0 + 125_000);
});

// Stepping through a portal and straight back is staying on the map: going AFK after it pauses the
// session rather than ending it. Fails if a return to the map still counts as having left it.
test("a player back on the map they left is on it again", () => {
  const game = session();
  game.grind(120);
  game.goTo("bbbb");
  game.idle(20);
  game.goTo("aaaa");
  game.idle(50);
  assert.equal(history(game.sent).length, 0);
  assert.deepEqual([summary(game.sent).paused, summary(game.sent).autoPaused], [true, true]);
});

// Walking to another grind map: a kill there within a minute of leaving keeps the session, walk
// included. Fails if moving between maps splits a session.
test("a kill on another map within a minute of leaving keeps the session", () => {
  const game = session();
  game.grind(120);
  game.goTo("bbbb");
  game.idle(40);
  game.grind(120);
  game.goTo("cccc");
  game.idle(70);
  assert.equal(history(game.sent).length, 1, "ended once, after the last map");
  assert.equal(history(game.sent)[0].activeMs, 280_000, "both maps and the walk between them");
});

// A pause the player pressed is theirs: leaving the map or a kill while it's on changes nothing,
// and after Resume the minute with no EXP starts from the press. Fails if MesoVibe resumes or ends
// a session the player paused, or pauses it again the moment they resume.
test("a session the player paused is never resumed or ended on its own", () => {
  const game = session();
  game.grind(120);
  game.send(action("pause", game.state.at));
  game.goTo("bbbb");
  game.idle(180);
  game.kill();
  assert.deepEqual([summary(game.sent).paused, summary(game.sent).autoPaused], [true, false]);
  assert.equal(history(game.sent).length, 0);
  const played = summary(game.sent).activeMs;
  game.send(action("resume", game.state.at));
  game.grind(10);
  assert.equal(summary(game.sent).paused, false);
  assert.equal(summary(game.sent).activeMs, played + 10_000);
});

// Turned off in Settings, nothing pauses or ends on its own, and the choice is kept with the
// tracker's data. Fails if the setting is ignored, lost on a restart, or kept once turned back on.
test("turned off, sessions never pause or end on their own, and the setting is kept", () => {
  const game = session();
  game.send(action("auto-off", game.state.at));
  assert.equal(summary(game.sent).autoPause, false);
  assert.equal(last(game.sent, "save").data.auto, false);
  game.grind(120);
  game.idle(120);
  assert.equal(summary(game.sent).paused, false);
  game.goTo("bbbb");
  game.idle(120);
  assert.equal(history(game.sent).length, 0);

  const again = session(last(game.sent, "save").data);
  assert.equal(summary(again.sent).autoPause, false, "after a restart");
  again.send(action("auto-on", again.state.at));
  assert.equal(summary(again.sent).autoPause, true);
  assert.equal("auto" in last(again.sent, "save").data, false);
  assert.equal(summary(session(OFF).sent).autoPause, false);
});

// A pause on its own isn't a trip to a shop: the mesos picked up across it still count as picked
// up. Fails if an idle minute turns the pickups since the last meso total into loot sold.
test("mesos picked up across a pause on its own still count as picked up", () => {
  const game = session();
  game.kill();
  game.total(1_000_000);
  game.grind(120);
  game.idle(90);
  game.kill();
  game.grind(60);
  game.total(1_050_000);
  const figures = lootFigures(game.sent);
  assert.deepEqual([figures["Mesos picked up"], figures["Loot sold"]], ["50,000", "—"]);
});

// A player who kills a slow mob about once every 70 seconds is paused between kills, but each
// pause keeps their usual time between kills, up to a minute. Fails if each wait is taken back to
// the kill, which would leave the session with its EXP and almost no time.
test("a slow kill's wait isn't taken back to nothing", () => {
  const game = session();
  game.grind(700, 70);
  assert.ok(summary(game.sent).activeMs >= 60_000 + 8 * 60_000, `${summary(game.sent).activeMs} ms`);
});

// With the minimap closed no map name is read: that's neither leaving the map nor a new one, and
// the first name read after a kill is that kill's map. Fails if a session ends while the player
// stays put with the minimap closed, or reopening it on the same map counts as leaving.
test("a map name that can't be read is neither leaving the map nor a new one", () => {
  const game = session();
  game.grind(120);
  game.goTo(null);
  game.idle(70);
  assert.deepEqual([summary(game.sent).autoPaused, history(game.sent).length], [true, 0], "paused, not ended, with the minimap closed");
  game.kill();
  game.goTo("aaaa");
  game.idle(70);
  assert.deepEqual([summary(game.sent).autoPaused, history(game.sent).length], [true, 0], "the name read after the kill is its map");
  game.goTo("bbbb");
  game.idle(60);
  assert.equal(history(game.sent).length, 1, "leaving that map still ends the session");
});

// Before any map name is read, play time waits for the first map read. A pause takes the idle
// minute back from that waiting time, and the map read after it is the kill's. Fails if that map
// is given the idle minute anyway, or reading its name ends the session as a map left.
test("a map read only after the last kill is the kill's map, and gets none of the idle minute", () => {
  const game = session(null, { noMap: true });
  game.grind(120);
  game.idle(70);
  game.goTo("aaaa");
  game.idle(60);
  assert.deepEqual([summary(game.sent).autoPaused, history(game.sent).length], [true, 0]);
  game.kill();
  game.grind(10);
  game.send(action("end", game.state.at));
  const [past] = history(game.sent);
  assert.equal(past.activeMs, 135_000);
  assert.deepEqual(
    past.maps.map((m) => [m.k, m.p]),
    [["aaaa", 135_000]],
    "the map's time adds up to the session's",
  );
});

// Only time the game is seen counts towards the minute: none while it can't be found, and at most
// five seconds across a gap like the computer sleeping. Fails if the session pauses the moment the
// game is seen again.
test("time the game isn't seen doesn't count towards the minute with no EXP", () => {
  const game = session();
  game.grind(120);
  game.capture("no_game");
  game.idle(120);
  game.capture("capturing");
  game.idle(30);
  assert.equal(summary(game.sent).paused, false, "after two minutes with the game not found");
  game.state.at += 600_000;
  game.send({ type: "tick", atMs: game.state.at });
  assert.equal(summary(game.sent).paused, false, "after ten minutes asleep");
  game.idle(25);
  assert.equal(summary(game.sent).paused, true, "after a minute of it seen");
});

// A pause takes the idle minute back from the potions' read time too, which pot cost/hr is
// measured over, including the part with heals from others on. Fails if pot cost/hr keeps it.
test("a pause on its own takes the idle minute back from the potions' read time", () => {
  const game = session(null, { pots: true });
  game.grind(120);
  game.idle(20);
  game.send(event({ type: "TestHealing", resource: "hp", on: true }, game.state.at));
  game.idle(40);
  assert.equal(summary(game.sent).autoPaused, true);
  const [entry] = last(game.sent, "save").data.current.maps;
  // Read time leaves out the five seconds before the first kill, as it always does.
  assert.deepEqual([entry.p, entry.h, entry.hm ?? 0], [125_000, 120_000, 0]);
});

// Walking on through more maps doesn't restart the minute: the session ends a minute after the
// player left the map of the last kill. Fails if each new map gives the walk another minute.
test("a minute off the map ends the session however many maps the player walks through", () => {
  const game = session();
  game.grind(120);
  game.goTo("bbbb");
  game.idle(40);
  game.goTo("cccc");
  game.idle(20);
  const [past] = history(game.sent);
  assert.equal(past?.activeMs, 120_000);
  assert.equal(past.endedAtMs, T0 + 120_000);
  assert.deepEqual(past.maps.map((m) => m.k), ["aaaa"], "the maps walked through aren't ones it played");
});

