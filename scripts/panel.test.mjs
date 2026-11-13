// A smoke test of the panel (ui/src) in headless Chrome, against the development stand-in for the
// host (ui/src/mock.ts): a new install reading straight away, the live session strip, End and
// Reset with Undo, Safe mode's way back, the blocked state, the display and startup settings and
// the mini-HUD. It drives Chrome over the DevTools protocol with
// Node's own WebSocket, so it needs no browser package. It's skipped when no Chrome is installed;
// set MESOVIBE_CHROME to point at one.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const CHROME = [
  process.env.MESOVIBE_CHROME,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
  "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
].find((path) => path && existsSync(path));

/** Resolves with the first match of `pattern` in a stream's text. */
function waitFor(stream, pattern, what) {
  return new Promise((resolve, reject) => {
    let seen = "";
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${what}: ${seen.slice(-500)}`)), 30_000);
    stream.on("data", (chunk) => {
      // Colors off, whatever the environment forces.
      seen += String(chunk).replace(/\x1b\[[0-9;]*m/g, "");
      const match = seen.match(pattern);
      if (match) {
        clearTimeout(timer);
        resolve(match);
      }
    });
  });
}

/** A minimal DevTools-protocol client for one page. */
async function openPage(port, url) {
  // Encoded, so a page URL with more than one query parameter reaches Chrome whole.
  const target = await (await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, { method: "PUT" })).json();
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  let id = 0;
  const pending = new Map();
  const errors = [];
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.text);
    const waiting = pending.get(message.id);
    if (waiting) {
      pending.delete(message.id);
      if (message.error) waiting.reject(new Error(message.error.message));
      else waiting.resolve(message.result);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      pending.set(++id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  await send("Runtime.enable");
  await send("Emulation.setDeviceMetricsOverride", { width: 360, height: 720, deviceScaleFactor: 1, mobile: false });
  const evaluate = async (expression) => {
    const { result, exceptionDetails } = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    // Chrome's own text is just "Uncaught"; the description carries the actual error.
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
    return result.value;
  };
  /**
   * Polls `expression` until it's truthy, and returns it. A page that is still loading can throw
   * (no document.body yet, which a cold Vite start makes likely for the first page), so an error
   * means "not yet" and only fails the wait at its deadline, where the last error is reported.
   */
  const until = async (expression, what, ms = 30_000) => {
    const deadline = Date.now() + ms;
    let lastError = null;
    while (Date.now() < deadline) {
      try {
        const value = await evaluate(expression);
        if (value) return value;
        lastError = null;
      } catch (error) {
        lastError = error;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
    const seen = await evaluate(`document.body?.innerText ?? "(no page body)"`).catch((error) => String(error));
    throw new Error(`timed out waiting for ${what}${lastError ? ` (last error: ${lastError.message})` : ""}: ${seen}`);
  };
  const click = (text) =>
    evaluate(
      `(() => { const b = [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(text)}); if (!b) return false; b.click(); return true; })()`,
    );
  /** Moves the mouse onto the middle of the first element matching `selector`, as a player would. */
  const hover = async (selector) => {
    const { x, y } = await evaluate(
      `(() => { const r = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`,
    );
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
  };
  /** Sizes the page's window, as the host sizes the mini-HUD's. */
  const resize = (width, height) => send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  return { evaluate, until, click, hover, resize, errors, close: () => socket.close() };
}

test("the panel", { skip: CHROME ? false : "no Chrome found (set MESOVIBE_CHROME)", timeout: 180_000 }, async (t) => {
  // The panel loads the Session Tracker's bundle from /plugins; build it from source.
  const esbuild = createRequire(join(REPO, "plugins", "package.json"))("esbuild");
  const plugins = join(REPO, "ui", "public", "plugins");
  mkdirSync(plugins, { recursive: true });
  await esbuild.build({
    entryPoints: [join(REPO, "plugins/src/session-tracker/index.ts")],
    outfile: join(plugins, "session-tracker.js"),
    bundle: true,
    format: "esm",
    platform: "browser",
    target: "es2022",
    alias: { "@mesovibe/sdk": join(REPO, "sdk/src/index.ts") },
    logLevel: "silent",
  });

  const viteDir = dirname(createRequire(join(REPO, "ui", "package.json")).resolve("vite/package.json"));
  const viteBin = join(viteDir, JSON.parse(readFileSync(join(viteDir, "package.json"), "utf8")).bin.vite);
  const port = 5300 + Math.floor(Math.random() * 400);
  const vite = spawn(process.execPath, [viteBin, "--port", String(port), "--host", "127.0.0.1"], {
    cwd: join(REPO, "ui"),
    env: { ...process.env, NO_COLOR: "1", FORCE_COLOR: "0" },
  });
  const profile = mkdtempSync(join(tmpdir(), "mesovibe-panel-"));
  const chrome = spawn(CHROME, [
    "--headless=new",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-gpu",
    "about:blank",
  ]);
  t.after(async () => {
    vite.kill();
    // Chrome keeps writing its profile while it shuts down: wait for it, then clean up.
    const exited = new Promise((resolve) => chrome.once("exit", resolve));
    chrome.kill();
    await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 5_000))]);
    rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  });
  await waitFor(vite.stdout, /Local:/, "vite");
  const [, devtools] = await waitFor(chrome.stderr, /DevTools listening on ws:\/\/127\.0\.0\.1:(\d+)\//, "Chrome");
  const at = (scenario, page = "") => `http://127.0.0.1:${port}/${page}?mock=${scenario}`;

  const checked = (key) => `document.querySelector('[data-key="${key}"]')?.getAttribute("aria-checked")`;
  const disabled = (key) => `document.querySelector('[data-key="${key}"]')?.disabled`;
  const textOf = (key) => `document.querySelector('[data-key="${key}"]')?.textContent`;
  const openSettings = async (page) => {
    await page.until(`document.querySelector(".strip")`, "the panel");
    await page.evaluate(`document.querySelector('[aria-label="Settings"]').click()`);
  };

  // A new install goes straight to the panel and reads the game: no first-run screen, and no
  // Safe mode offer while it looks for the game. "Open with MapleStory Classic" starts on. Fails
  // if a first-run choice comes back, Safe mode is offered outside Settings, or the switch starts
  // off.
  await t.test("a new install reads the game straight away, with Open with MapleStory Classic on", async () => {
    const page = await openPage(devtools, at("first"));
    const status = `document.querySelector(".status-text")?.textContent`;
    await page.until(`${status} && ${status} !== "Reading game"`, "looking for the game");
    const buttons = `[...document.querySelectorAll("button")].map((b) => b.textContent.trim())`;
    assert.ok(!(await page.evaluate(buttons)).includes("Use Safe mode"), "no Safe mode offer while looking");
    await page.until(`${status} === "Reading game"`, "reading");
    assert.doesNotMatch(await page.evaluate("document.body.innerText"), /Before you start/);
    await openSettings(page);
    assert.equal(await page.until(checked("settings-startup"), "the startup switch"), "true", "on from the first start");
    assert.match(
      await page.evaluate(textOf("settings-startup-note")),
      /waits in the menu bar.+quit from its menu bar icon\. Off, closing the panel quits MesoVibe\.$/,
    );
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // About also says how the last check went. Fails if the update line is missing, the click isn't
  // sent, a failure is hidden, or Check now does nothing.
  await t.test("a ready update is offered in the strip and installs only when clicked", async () => {
    const page = await openPage(devtools, at("update"));
    const line = `document.querySelector('[data-key="strip-update"]')?.textContent`;
    assert.equal(await page.until(line, "the update line"), "MesoVibe 0.2.0 is ready.Restart to update");
    assert.equal(await page.evaluate("window.__mockInstalls.count"), 0, "nothing installs on its own");
    await page.evaluate(`document.querySelector('[data-key="strip-update-restart"]').click()`);
    await page.until(`${line} === "Restarting into MesoVibe 0.2.0…"`, "restarting");
    assert.equal(await page.evaluate("window.__mockInstalls.count"), 1);
    assert.deepEqual(page.errors, []);
    page.close();

    const failed = await openPage(devtools, at("updatefail"));
    await openSettings(failed);
    const status = textOf("settings-update-status");
    assert.match(await failed.until(status, "About's update line"), /^Update check failed\. MesoVibe couldn't reach GitHub/);
    assert.equal(await failed.evaluate(line), "", "a failed check stays out of the strip");
    await failed.evaluate(`document.querySelector('[data-key="settings-update-check"]').click()`);
    await failed.until(`${status}?.startsWith("MesoVibe is up to date. Last checked at ")`, "up to date after Check now");
    assert.deepEqual(failed.errors, []);
    failed.close();
  });

  // The map shows as a picture of its name, with a name screen readers can say, and mesos as a
  // total. Fails if the picture never arrives from the host, is drawn without its mask, or the
  // Mesos row is missing.
  await t.test("This session shows the map's name picture and the mesos counted", async () => {
    const page = await openPage(devtools, at("loot"));
    const picture = `document.querySelector('.view .game-text[aria-label^="Map first played"]')`;
    await page.until(`${picture}?.style.maskImage?.startsWith('url("data:image/png')`, "the map's picture");
    assert.ok(await page.evaluate(`${picture}.getBoundingClientRect().width > 0`), "drawn with a size");
    const mesos = `[...document.querySelectorAll(".view dt")].find((d) => d.textContent === "Mesos")?.nextElementSibling?.textContent`;
    assert.match(await page.until(mesos, "the Mesos row"), /^[\d,]+\+?$/);

    // What MesoVibe sees says how the notices are read and shows the map's name as read. Fails if
    // either row is missing or the map row has no picture.
    await page.evaluate(`document.querySelector('[aria-label="What MesoVibe sees"]').click()`);
    const notices = `document.querySelector('[data-key="sees-notices"]')?.textContent`;
    assert.equal(await page.until(notices, "the notices row"), "Pickup noticesReading");
    await page.until(`document.querySelector('[data-key="sees-map"] .game-text')?.style.maskImage`, "the map row's picture");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // Saved sessions that can't be opened must not read as empty: the tracker stays off with the
  // reason and Restart, and saves nothing over them. Fails if it starts with nothing (its first
  // save would replace the history) or if Restart doesn't read again.
  await t.test("saved sessions that can't be read keep the tracker off until Restart reads them", async () => {
    const page = await openPage(devtools, at("unreadable"));
    const alert = `document.querySelector(".view .state.error")?.textContent`;
    assert.equal(
      await page.until(alert, "the read error"),
      "Couldn't read Session Tracker's saved data (Access is denied. (os error 5)). It's kept as it is, and Session Tracker stays off so nothing is saved over it. Restart to try again.",
    );
    assert.equal(await page.evaluate("window.__mockStorage.saves"), 0, "nothing saved over it");
    await page.evaluate(`document.querySelector('[data-key="restart"]').click()`);
    await page.until(`!document.querySelector(".view .state.error") && window.__mockStorage.reads === 2`, "the tracker after Restart");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // A login item registered from the disk image would stop working, so the switch stays off and
  // says what to do instead. Fails if the switch is on or usable, or the reason is hidden.
  await t.test("run from the disk image, Open with MapleStory Classic says to move MesoVibe first", async () => {
    const page = await openPage(devtools, at("moved"));
    await openSettings(page);
    assert.equal(await page.until(checked("settings-startup"), "the startup switch"), "false");
    await page.until(`${disabled("settings-startup")} === true`, "the startup switch, disabled");
    // Why it can't be turned on is something to act on, so it stays in view rather than in a hint.
    const problem = `document.querySelector('[data-key="settings-startup-problem"]')`;
    assert.match(await page.evaluate(`${problem}.textContent`), /Move MesoVibe to your Applications folder/);
    assert.equal(await page.evaluate(`${problem}.checkVisibility()`), true, "in view");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // Fails if the OS's error is swallowed or the switch flips anyway.
  await t.test("Settings switches the opt-in, and says why when the OS refuses", async () => {
    const page = await openPage(devtools, at("grind"));
    await openSettings(page);
    assert.equal(await page.until(checked("settings-startup"), "the startup switch"), "true", "on from the first start");
    await page.evaluate(`document.querySelector('[data-key="settings-startup"]').click()`);
    await page.until(`${checked("settings-startup")} === "false"`, "switched off");
    page.close();

    const refused = await openPage(devtools, at("loginfail"));
    await openSettings(refused);
    await refused.until(`${checked("settings-startup")} === "false"`, "the startup switch");
    await refused.evaluate(`document.querySelector('[data-key="settings-startup"]').click()`);
    const error = await refused.until(textOf("settings-startup-error"), "the error");
    assert.match(error, /^Open with MapleStory Classic didn't change\. .+Access is denied\.$/);
    assert.equal(await refused.evaluate(checked("settings-startup")), "false");
    assert.deepEqual(refused.errors, []);
    refused.close();
  });

  // The strip and Session view as the player sees them. Fails if host and plugin action names drift,
  // focus is lost, or a repeat returns.
  await t.test("the strip shows the live rate, End sits under Reset, and both can be undone", async () => {
    const page = await openPage(devtools, at("grind"));
    const actions = `document.querySelector(".strip-actions")`;
    await page.until(`${actions}`, "the strip");
    assert.equal(await page.evaluate(`${actions}.hidden`), true, "nothing to end before the first gain");
    await page.until(`document.querySelector(".status-text")?.textContent === "Reading game"`, "reading");
    const rate = await page.until(`document.querySelector(".number-hero")?.textContent`, "the EXP/hr");
    assert.match(rate, /^\d+(\.\d+)?K$/);
    assert.match(await page.evaluate(`document.querySelector(".to-level").textContent`), /^Lv 42 → 43 in /);
    await page.until(`!${actions}.hidden`, "Reset and End after the first gain");
    assert.deepEqual(
      await page.evaluate(`[...${actions}.querySelectorAll("button")].map((b) => b.dataset.key)`),
      ["strip-pause", "strip-reset", "strip-end"],
      "Pause, then Reset, then End",
    );
    // The Session view repeats nothing the strip shows.
    const view = await page.evaluate(`document.querySelector(".view").innerText`);
    assert.match(view, /EXP gained/);
    assert.doesNotMatch(view, /Time to level|Reset session/);
    assert.equal(await page.evaluate(`document.querySelectorAll(".view .progress").length`), 0);
    assert.equal(await page.evaluate(`document.querySelectorAll("[role=switch]").length`), 0, "no Safe mode switch in the strip");

    const focusedKey = `document.activeElement?.dataset?.key`;
    await page.evaluate(`document.querySelector('[data-key="strip-reset"]').click()`);
    await page.until(`document.querySelector(".notice")?.textContent.includes("Session reset.")`, "the reset notice");
    assert.equal(await page.until(focusedKey, "focus on Undo"), "notice-undo");
    assert.ok(await page.click("Undo"));
    await page.until(`document.querySelector(".notice")?.textContent.includes("Session restored.")`, "the undo");
    assert.ok(await page.evaluate(`document.activeElement === document.querySelector(".strip")`), "focus back on the strip");

    await page.until(`!${actions}.hidden`, "Reset and End again");
    await page.evaluate(`document.querySelector('[data-key="strip-end"]').click()`);
    const saved = await page.until(`document.querySelector(".notice")?.textContent.match(/Saved: [\\d,]+ EXP in .+\\./)?.[0]`, "the recap");
    assert.match(saved, /^Saved: [\d,]+ EXP in /);
    assert.equal(await page.until(focusedKey, "focus on Undo"), "notice-undo");
    // Under the session's map: the day and time played, then the level line.
    const detailLines = `[...(document.querySelector(".view .table td")?.querySelectorAll(".cell-detail") ?? [])].map((d) => d.textContent)`;
    const lines = await page.until(`${detailLines}.length === 2 && ${detailLines}`, "the detail lines");
    assert.match(lines[0], /· \d+s$/);
    assert.equal(lines[1], "Lv\u00a042");
    // Statuses repaint the strip four times a second; the focused Undo must stay the same element
    // through them, and an Enter held down on End must not repeat onto it.
    await page.evaluate(`(() => {
      window.__undo = document.querySelector('[data-key="notice-undo"]');
      window.__paints = 0;
      new MutationObserver(() => window.__paints++).observe(document.querySelector(".strip-progress"), { childList: true });
    })()`);
    await page.until(`window.__paints >= 2`, "two status repaints");
    assert.ok(await page.evaluate(`document.querySelector('[data-key="notice-undo"]') === window.__undo`), "Undo kept across repaints");
    const press = (repeat) => `window.__undo.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", repeat: ${repeat}, cancelable: true }))`;
    assert.equal(await page.evaluate(press(true)), false, "a held Enter is ignored");
    assert.equal(await page.evaluate(press(false)), true, "a fresh Enter is left alone");
    // The next session's first gain arrives within seconds; until a rate means anything the strip
    // says it's coming.
    await page.until(`document.querySelector(".hero-empty")?.textContent === "Your rate appears after a few seconds of play."`, "the rate on its way");
    assert.ok(await page.click("Undo"));
    await page.until(`document.querySelector(".notice")?.textContent.includes("Session restored.")`, "the undo of End");
    await page.until(`document.querySelector(".view .cell-detail") === null`, "the history row taken back");
    assert.ok(await page.evaluate(`document.querySelector(".number-hero").textContent.length > 0`));
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // Before the first gain the strip shows the level, its bar and the EXP to go under it, with no
  // rate and no empty row pushing them down from the status. The game sends no EXP change here, so
  // this fails if the tracker only learns the bar from a change it may have missed.
  await t.test("before the first gain the strip shows the level, its bar and the EXP to go, without a rate", async () => {
    const page = await openPage(devtools, at("idle"));
    await page.until(`document.querySelector(".status-text")?.textContent === "Reading game"`, "reading");
    assert.equal(await page.until(`document.querySelector(".to-level")?.textContent || null`, "the level"), "Lv 42");
    await page.until(`document.querySelector('.strip-progress [role="progressbar"]')`, "the level's bar");
    assert.equal(await page.evaluate(`document.querySelector(".hero").hidden`), true, "no rate yet");
    assert.equal(await page.evaluate(`document.querySelector(".hero-empty").textContent`), "");
    assert.equal(await page.evaluate(`document.querySelector(".strip-hero").hidden`), true, "no empty rate row");
    const gap = await page.evaluate(
      `document.querySelector(".to-level").getBoundingClientRect().top - document.querySelector(".strip-status").getBoundingClientRect().bottom`,
    );
    assert.ok(gap <= 16, `the level sits ${gap}px under the status`);
    const toGo = await page.until(`document.querySelector(".to-go:not([hidden])")?.textContent`, "the EXP to go");
    assert.equal(toGo, `${(356_916 - 230_000).toLocaleString("en-US")}EXP to go`);
    const order = await page.evaluate(`[...document.querySelector(".strip").children].map((c) => c.className).filter((c) => /to-level|strip-progress|to-go/.test(c))`);
    assert.deepEqual(order, ["to-level", "strip-progress", "to-go"], "under the bar");
    assert.doesNotMatch(await page.evaluate(`document.querySelector(".view").innerText`), /EXP to go/, "said once");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // Pause and Resume are one button that keeps the keyboard focus. Fails if the host and plugin
  // action names drift, the strip doesn't say it's paused, or gains keep counting while paused.
  await t.test("Pause turns into Resume, and the session stops counting until it's pressed", async () => {
    const page = await openPage(devtools, at("grind"));
    const pause = `document.querySelector('[data-key="strip-pause"]')`;
    await page.until(`!${pause}.hidden && ${pause}.textContent === "Pause"`, "Pause after the first gain");
    await page.evaluate(`${pause}.focus(); ${pause}.click()`);
    await page.until(`${pause}.textContent === "Resume"`, "Resume");
    assert.equal(await page.evaluate(`document.activeElement === ${pause}`), true, "focus stays on the button");
    assert.equal(await page.until(`document.querySelector(".hero-empty")?.textContent || null`, "the paused line"), "Session paused");
    // The stand-in gains EXP every two seconds; none of it counts while paused.
    const gained = `[...document.querySelectorAll(".view dt")].find((dt) => dt.textContent === "EXP gained")?.nextElementSibling.textContent`;
    const before = await page.until(gained, "EXP gained");
    await new Promise((resolve) => setTimeout(resolve, 4_500));
    assert.equal(await page.evaluate(gained), before, "no gains while paused");
    await page.evaluate(`${pause}.click()`);
    await page.until(`${pause}.textContent === "Pause"`, "Pause again");
    await page.until(`${gained} !== ${JSON.stringify(before)}`, "gains count again");
    assert.notEqual(await page.evaluate(`document.querySelector(".hero-empty").textContent`), "Session paused");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // A past session goes with one click on its bin, with Undo like End. Fails if the button has no
  // name, focus is lost with its row instead of moving to Undo, or Undo doesn't bring the row back.
  await t.test("a past session's delete button removes its row, with Undo", async () => {
    const page = await openPage(devtools, at("grind"));
    await page.until(`!document.querySelector(".strip-actions").hidden`, "End after the first gain");
    await page.evaluate(`document.querySelector('[data-key="strip-end"]').click()`);
    const bin = `document.querySelector(".view .row-action-button")`;
    const label = await page.until(`${bin}?.getAttribute("aria-label")`, "the delete button");
    assert.match(label, /^Delete the session from /);
    assert.deepEqual(
      await page.evaluate(`[...document.querySelector(".view .table thead tr").cells].map((c) => c.textContent)`),
      ["Map", "EXP gained", "EXP/hr", "Actions"],
    );
    // The delete's notice takes the place of End's, and its Undo takes the focus.
    await page.evaluate(`${bin}.focus(); ${bin}.click()`);
    await page.until(`document.querySelector(".notice")?.textContent.includes("Session deleted.")`, "the delete notice");
    assert.equal(await page.until(`document.activeElement?.dataset?.key`, "focus on Undo"), "notice-undo");
    await page.until(`document.querySelector(".view .table") === null`, "the row gone");
    assert.ok(await page.click("Undo"));
    await page.until(`document.querySelector(".notice")?.textContent.includes("Session restored.")`, "the undo");
    await page.until(`${bin}?.getAttribute("aria-label") === ${JSON.stringify(label)}`, "the row back");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // A view taller than the panel scrolls inside itself, under a strip and beside a rail that stay
  // put. Fails if the page grows with the view instead, which pushes the rail's last icons below
  // the window.
  await t.test("a long view scrolls inside the panel, and the rail stays in the window", async () => {
    const page = await openPage(devtools, at("grind"));
    await openSettings(page);
    await page.until(checked("settings-startup"), "Settings");
    const layout = await page.evaluate(`(() => {
      const view = document.querySelector(".view");
      const lastIcon = [...document.querySelectorAll(".rail-item")].at(-1).getBoundingClientRect();
      return { window: innerHeight, page: document.documentElement.scrollHeight, scrolls: view.scrollHeight > view.clientHeight, lastIcon: lastIcon.bottom };
    })()`);
    assert.ok(layout.scrolls, "Settings is taller than the panel at this size");
    assert.equal(layout.page, layout.window, "the page is no taller than the window");
    assert.ok(layout.lastIcon <= layout.window, `the rail's last icon ends at ${layout.lastIcon}px, past the window's ${layout.window}px`);
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // Safe mode's switch lives in Settings. Fails if the strip's switch returns or Start reading does
  // nothing.
  await t.test("Safe mode has no switch in the strip, and Start reading turns reading back on", async () => {
    const page = await openPage(devtools, at("safe"));
    await page.until(`document.querySelector(".status-text")?.textContent.startsWith("Safe mode")`, "Safe mode");
    assert.equal(await page.evaluate(`document.querySelectorAll(".strip [role=switch]").length`), 0);
    // Pressed from the keyboard: the button goes away with Safe mode, and focus stays in the strip.
    await page.evaluate(`(() => { const b = document.querySelector('[data-key="start-reading"]'); b.focus(); b.click(); })()`);
    await page.until(`!document.querySelector(".status-text")?.textContent.startsWith("Safe mode")`, "reading again");
    assert.ok(await page.until(`document.activeElement === document.querySelector(".strip")`, "focus kept in the strip"));
    assert.equal(await page.evaluate(`[...document.querySelectorAll("button")].some((b) => b.textContent === "Start reading")`), false);
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // A black screen for a moment is a map change; for long, MesoVibe isn't seeing the game's picture.
  // Fails if a screen that stays black still reads as "Reading game".
  await t.test("a screen that stays black says the EXP bar can't be seen", async () => {
    const page = await openPage(devtools, at("dark"));
    await page.until(
      `document.querySelector(".status-text")?.textContent === "Can't see the EXP bar. If you play in Full Screen, try windowed."`,
      "a long dark spell",
    );
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // At a Full Screen size MesoVibe can't read yet, nothing covers the bar: the player changes a
  // setting in the game. Fails if the panel doesn't say which.
  await t.test("Full Screen that can't be read says what to change in the game", async () => {
    const page = await openPage(devtools, at("fullscreen"));
    await page.until(
      `document.querySelector(".status-text")?.textContent === "Can't read the game in Full Screen at this size. In the game's Options > Graphics, try windowed or 1366×768."`,
      "the Full Screen hint",
    );
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // Safe mode is turned on only in Settings. Fails if the fix goes missing or the strip offers
  // Safe mode again.
  await t.test("a blocked capture says so and how to fix it", async () => {
    const page = await openPage(devtools, at("blocked"));
    await page.until(`document.querySelector(".status-text")?.textContent === "Can't capture the game window."`, "blocked");
    assert.ok(await page.click("How to fix"));
    await page.until(`document.querySelector(".help")?.textContent.includes("Screen Recording")`, "the fix");
    assert.ok(await page.evaluate(`[...document.querySelectorAll("button")].some((b) => b.textContent === "Try again")`));
    assert.equal(await page.evaluate(`[...document.querySelectorAll("button")].some((b) => b.textContent === "Use Safe mode")`), false);
    assert.deepEqual(page.errors, []);
    page.close();
  });

  await t.test("Settings keeps MesoVibe with the game, and the mini-HUD can be switched off", async () => {
    const page = await openPage(devtools, at("grind"));
    await page.until(`document.querySelector(".strip")`, "the panel");
    await page.evaluate(`document.querySelector('[aria-label="Settings"]').click()`);
    assert.equal(await page.until(checked("settings-dock"), "the dock switch"), "true");
    assert.equal(await page.evaluate(checked("settings-hud")), "true");
    assert.equal(await page.evaluate(checked("corner-top-right")), "true", "top right by default");
    await page.evaluate(`document.querySelector('[data-key="corner-bottom-left"]').click()`);
    await page.until(`${checked("corner-bottom-left")} === "true"`, "the chosen corner");
    await page.evaluate(`document.querySelector('[data-key="settings-hud"]').click()`);
    await page.until(`${checked("settings-hud")} === "false"`, "the mini-HUD off");
    assert.equal(await page.evaluate(`document.querySelector('[role="radiogroup"]')`), null, "no corner without a mini-HUD");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // Settings lists each switch by name; its explanation shows only while the pointer is on it or
  // it has keyboard focus, and screen readers hear it as the switch's description. Fails if the
  // explanations show all the time again, a hint doesn't appear or blocks the next switch, or a
  // name that wraps is centered.
  await t.test("Settings explains each switch in a hint that shows on hover or keyboard focus", async () => {
    const page = await openPage(devtools, at("grind"));
    await openSettings(page);
    await page.until(checked("settings-startup"), "the startup switch");
    const shown = (key) => `document.querySelector('[data-key="${key}"]').checkVisibility()`;
    const hints = ["settings-safe-hint", "settings-startup-note", "settings-dock-hint", "settings-hud-hint", "settings-update-note"];
    for (const key of hints) assert.equal(await page.evaluate(shown(key)), false, `${key} hidden at first`);
    await page.hover('[data-key="settings-dock"]');
    await page.until(shown("settings-dock-hint"), "the hint on hover");
    assert.equal(await page.evaluate(shown("settings-hud-hint")), false, "only the hovered switch's");
    // The next switch sits under that hint; the pointer reaches it, and its hint takes over.
    await page.hover('[data-key="settings-hud"]');
    await page.until(shown("settings-hud-hint"), "the next switch's hint");
    assert.equal(await page.evaluate(shown("settings-dock-hint")), false);
    const describedBy = `document.getElementById(document.querySelector('[data-key="settings-hud"]').getAttribute("aria-describedby"))?.dataset.key`;
    assert.equal(await page.evaluate(describedBy), "settings-hud-hint");
    await page.evaluate(`document.querySelector('[data-key="settings-safe"]').focus()`);
    await page.until(shown("settings-safe-hint"), "the hint on keyboard focus");
    // About's note on what an update check sends shows from the keyboard too, on Check now.
    await page.evaluate(`document.querySelector('[data-key="settings-update-check"]').focus()`);
    assert.match(await page.until(`${shown("settings-update-note")} && ${textOf("settings-update-note")}`, "the update note"), /^Once a day MesoVibe asks GitHub/);
    const align = `getComputedStyle(document.querySelector('[data-key="settings-startup"]')).textAlign`;
    assert.equal(await page.evaluate(align), "left");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  await t.test("the mini-HUD shows the rate, time to level and progress, or what's wrong", async () => {
    const page = await openPage(devtools, at("grind", "hud.html"));
    assert.equal(await page.until(`document.querySelector(".hud-rate")?.textContent`, "the rate"), "48.2K");
    await page.until(`document.querySelector(".hud-line")?.textContent === "Lv 42 → 43 in 51 min"`, "time to level");
    assert.equal(await page.evaluate(`document.querySelector(".hud-progress .number").textContent`), "64.62%");
    assert.deepEqual(page.errors, []);
    page.close();

    // The bar is read but nothing is gained yet: the card asks for the gain that starts the rate.
    const idle = await openPage(devtools, at("idle", "hud.html"));
    await idle.until(`document.querySelector(".hud-note")?.textContent === "Earn more EXP to start"`, "waiting for a gain");
    assert.deepEqual(idle.errors, []);
    idle.close();
    const learning = await openPage(devtools, at("learning", "hud.html"));
    await learning.until(`document.querySelector(".hud-note")?.textContent === "Learning your EXP bar…"`, "learning");
    learning.close();
    const blocked = await openPage(devtools, at("blocked", "hud.html"));
    await blocked.until(`document.querySelector(".hud-note")?.textContent === "Can't capture the game"`, "blocked");
    assert.ok(await blocked.evaluate(`document.querySelector(".dot.danger") !== null`));
    blocked.close();
    // A screen that stays black isn't a map change: fails if the mini-HUD stays green and waits
    // for EXP that will never be read.
    const dark = await openPage(devtools, at("dark", "hud.html"));
    await dark.until(`document.querySelector(".hud-note")?.textContent === "Can't see the EXP bar"`, "a long dark spell");
    assert.ok(await dark.evaluate(`document.querySelector(".dot.warn") !== null`));
    assert.deepEqual(dark.errors, []);
    dark.close();
    // Full Screen at a size MesoVibe can't read: fails if the mini-HUD doesn't say what to change,
    // or the hint is cut off or spills out of the card under a session's rate.
    const full = await openPage(devtools, at("fullscreen", "hud.html"));
    await full.until(`document.querySelector(".hud-note")?.textContent === "Can't read Full Screen"`, "the Full Screen note");
    assert.equal(await full.evaluate(`document.querySelector(".hud-line")?.textContent`), "Try windowed or 1366×768");
    assert.deepEqual(full.errors, []);
    full.close();
    const mid = await openPage(devtools, at("fullscreen-grind", "hud.html"));
    await mid.resize(216, 100);
    await mid.until(`document.querySelector(".hud-issue")?.textContent === "Can't read Full Screen"`, "the note under the rate");
    const fits = `(() => {
      const card = document.querySelector(".hud").getBoundingClientRect();
      const rows = [...document.querySelectorAll(".hud > *")].map((row) => row.getBoundingClientRect());
      const hint = [...document.querySelectorAll(".hud-line")].at(-1);
      return rows.every((r) => r.top >= card.top && r.bottom <= card.bottom) && hint.scrollWidth <= hint.clientWidth && hint.textContent;
    })()`;
    assert.equal(await mid.until(fits, "the hint inside the card"), "Try windowed or 1366×768");
    assert.equal(await mid.evaluate(`document.querySelector(".hud-progress")`), null, "the hint takes the progress row");
    assert.deepEqual(mid.errors, []);
    mid.close();
  });

  // A chat line as the host hands it over (app/src/chat.rs), with a one-pixel picture: these tests
  // look at what the panel does with lines, not at the pictures.
  const PIXEL = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
  const chatLine = (id, channel, { continues = false, gaps = 0, words = [[0, 4], [6, 10]] } = {}) =>
    ({ id, channel, atMs: 1_762_234_857_000 + id * 1000, continues, gaps, width: 10, height: 16, png: PIXEL, words });
  const say = (lines) => `window.mockChat(${JSON.stringify(lines)}), true`;

  // Fails if lines land in the wrong tab, unread counts drift, the rest of a long line splits off, a
  // line that arrives twice shows twice, or the missing-lines marker or the overlay controls break.
  await t.test("the Chat view splits whisper, party and friend lines and counts what's unread", async () => {
    const page = await openPage(devtools, at("levelup"));
    await page.until(`document.querySelector(".strip") && window.mockChat`, "the panel");
    const lines = [
      chatLine(1, "whisper"),
      chatLine(2, "whisper", { continues: true, words: [[0, 10]] }),
      chatLine(3, "party", { gaps: 1 }),
      chatLine(4, "friend", { gaps: 1 }),
    ];
    await page.evaluate(say(lines));
    await page.evaluate(say(lines));
    const badge = `document.querySelector(".rail-badge").textContent`;
    assert.equal(await page.until(badge, "the unread count"), "3", "the rest of a long line isn't another message");
    assert.equal(await page.evaluate(`document.querySelector('[aria-label^="Chat"]').getAttribute("aria-label")`), "Chat, 3 unread");

    await page.evaluate(`document.querySelector('[aria-label^="Chat"]').click()`);
    await page.until(`document.querySelector('[data-key="chat-tab-whisper"]')?.getAttribute("aria-selected") === "true"`, "whispers first");
    assert.equal(await page.until(`${badge} === "2" && ${badge}`, "the whisper read"), "2");
    assert.equal(await page.evaluate(`document.querySelectorAll(".chat-message").length`), 1);
    assert.equal(await page.evaluate(`document.querySelectorAll(".chat-message .chat-word").length`), 3, "both lines' words in one message");
    assert.match(await page.evaluate(`document.querySelector(".chat-text").getAttribute("aria-label")`), /^Whisper message at /);
    // The game draws a line whole once it moves up, and it comes again under its id: the whole
    // picture takes the cut one's place, and isn't another unread line.
    await page.evaluate(say([chatLine(1, "whisper", { words: [[0, 10]] })]));
    await page.until(`document.querySelectorAll(".chat-message .chat-word").length === 2`, "the whole picture in place");
    assert.equal(await page.evaluate(`document.querySelectorAll(".chat-message").length`), 1);
    assert.equal(await page.evaluate(badge), "2", "not another unread line");

    const partyTab = `document.querySelector('[data-key="chat-tab-party"]')`;
    assert.equal(await page.evaluate(`${partyTab}.getAttribute("aria-label")`), "Party, 1 unread");
    await page.evaluate(`${partyTab}.click()`);
    await page.until(`${badge} === "1"`, "the party line read");
    // A party line while the Party tab is open is read at once; the reader lost track in between.
    await page.evaluate(say([chatLine(5, "party", { gaps: 2 })]));
    await page.until(`document.querySelectorAll(".chat-message").length === 2`, "the new party line");
    assert.equal(await page.evaluate(badge), "1");
    assert.equal(await page.evaluate(`document.querySelector(".chat-gap")?.textContent`), "Some lines may be missing here.");

    // Chat over the game starts on, with its choices showing.
    assert.equal(await page.evaluate(`document.querySelector('[data-key="chat-overlay"]').getAttribute("aria-checked")`), "true");
    const pick = (channel) => `document.querySelector('[data-key="chat-overlay-${channel}"]')`;
    assert.equal(await page.until(`${pick("whisper")}?.textContent`, "the overlay's chat types"), "✓ Whispers");
    await page.evaluate(`${pick("whisper")}.click()`);
    await page.until(`${pick("whisper")}.getAttribute("aria-pressed") === "false"`, "whispers off the overlay");
    assert.equal(await page.evaluate(`${pick("whisper")}.textContent`), "Whispers");
    assert.equal(await page.evaluate(`${pick("party")}.getAttribute("aria-pressed")`), "true");
    // Each chat type's color: green whispers, pink party and orange friends to start, named, and a
    // pick sticks.
    const swatch = (channel, name) => `document.querySelector('[data-key="chat-color-${channel}-${name}"]')`;
    for (const [channel, name] of [["whisper", "green"], ["party", "pink"], ["friend", "orange"]]) {
      assert.equal(await page.evaluate(`${swatch(channel, name)}.getAttribute("aria-checked")`), "true", `${channel} in ${name}`);
    }
    await page.evaluate(`${swatch("party", "cyan")}.click()`);
    await page.until(`${swatch("party", "cyan")}.getAttribute("aria-checked") === "true"`, "party in cyan");
    assert.equal(await page.evaluate(`${swatch("party", "pink")}.getAttribute("aria-checked")`), "false");
    assert.equal(await page.evaluate(`${swatch("whisper", "green")}.getAttribute("aria-checked")`), "true", "only party changed");
    assert.equal(await page.evaluate(`${swatch("party", "cyan")}.getAttribute("aria-label")`), "Cyan");
    // Where it sits: on the left side to start, and a pick sticks.
    const place = (position) => `document.querySelector('[data-key="chat-position-${position}"]')`;
    assert.equal(await page.evaluate(`${place("left")}.getAttribute("aria-checked")`), "true");
    await page.evaluate(`${place("right")}.click()`);
    await page.until(`${place("right")}.getAttribute("aria-checked") === "true"`, "chat on the right");
    assert.equal(await page.evaluate(`${place("left")}.getAttribute("aria-checked")`), "false");
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // Fails if Settings and What MesoVibe sees swap back, or a view goes missing from the rail.
  await t.test("the rail lists Session and Chat, then Settings, then What MesoVibe sees", async () => {
    const page = await openPage(devtools, at("grind"));
    await page.until(`document.querySelector(".rail")`, "the rail");
    assert.deepEqual(await page.evaluate(`[...document.querySelectorAll(".rail-item")].map((b) => b.getAttribute("title"))`), [
      "Session",
      "Chat",
      "Settings",
      "What MesoVibe sees",
    ]);
    page.close();
  });

  // Split chat: the lines in the player's colors, the newest last, on the game's own chat-box wash.
  // Fails if the wash changes shade, covers more than the lines or leaves the box's edges, a line
  // that arrives twice shows twice, or a color change doesn't reach them.
  await t.test("the chat overlay shows the newest lines in their colors, each once, and nothing before one", async () => {
    const page = await openPage(devtools, `${at("levelup", "chat.html")}&position=above-chat`);
    await page.until(`window.mockChat && window.mockChoice`, "the overlay");
    assert.equal(await page.evaluate(`document.querySelector(".split-chat")`), null, "nothing over the game yet");
    const lines = [chatLine(1, "whisper"), chatLine(2, "whisper", { continues: true }), chatLine(3, "party")];
    await page.evaluate(say(lines));
    await page.evaluate(say(lines));
    await page.until(`document.querySelectorAll(".split-message").length === 2`, "two messages");
    assert.equal(await page.evaluate(`document.querySelectorAll(".split-line").length`), 3, "each line once");
    assert.equal(await page.evaluate(`document.querySelectorAll(".split-line.rest").length`), 1, "the rest of the long line");
    assert.match(await page.evaluate(`document.querySelector(".split-message").getAttribute("aria-label")`), /^Whisper message at /);
    // The game draws a line whole once it moves up; it comes again under its id, in its place.
    const partyWidth = `document.querySelectorAll(".split-message")[1].querySelector(".chat-picture").getBoundingClientRect().width`;
    const cutWidth = await page.evaluate(partyWidth);
    await page.evaluate(say([{ ...chatLine(3, "party"), width: 20 }]));
    await page.until(`${partyWidth} > ${cutWidth * 1.5}`, "the whole picture in its place");
    assert.equal(await page.evaluate(`document.querySelectorAll(".split-line").length`), 3, "still each line once");
    const colors = `[...document.querySelectorAll(".split-message")].map((m) => getComputedStyle(m).color)`;
    assert.deepEqual(await page.evaluate(colors), ["rgb(102, 255, 102)", "rgb(255, 136, 204)"], "the settings' colors to start");
    await page.evaluate(
      `window.mockChoice({ channels: { whisper: true, party: true, friend: true }, colors: { whisper: "#66ff66", party: "#ffff00", friend: "#ffaa44" }, position: "above-chat" }), true`,
    );
    await page.until(`${colors}[1] === "rgb(255, 255, 0)"`, "party in yellow");
    await page.evaluate(
      `window.mockChoice({ channels: { whisper: false, party: true, friend: true }, colors: { whisper: "#66ff66", party: "#ffff00", friend: "#ffaa44" }, position: "above-chat" }), true`,
    );
    await page.until(`document.querySelectorAll(".split-message").length === 1`, "whispers off");
    const wash = await page.evaluate(`(() => {
      const list = document.querySelector(".split-chat");
      const { left, width, bottom, height } = list.getBoundingClientRect();
      return { color: getComputedStyle(list).backgroundColor, left, width, bottom, height };
    })()`);
    const [innerWidth, innerHeight] = await page.evaluate(`[innerWidth, innerHeight]`);
    assert.equal(wash.color, "rgba(0, 0, 0, 0.5)", "the game's chat-box shade");
    assert.deepEqual([wash.left, wash.width, wash.bottom], [0, innerWidth, innerHeight], "the box's width, on its top edge");
    assert.ok(wash.height < innerHeight / 2, `only as tall as the lines, not ${wash.height}px of ${innerHeight}`);
    assert.deepEqual(page.errors, []);
    page.close();
  });

  // A box at the game's side: the same wash and text size, against the chosen edge, with a long
  // line wrapping at the box's width. Fails if the box leaves its edge, the text isn't the game's
  // size, or a long line runs past the box instead of wrapping.
  await t.test("the chat overlay in a box at the game's side keeps the game's text size and wraps long lines", async () => {
    const page = await openPage(devtools, at("levelup", "chat.html"));
    await page.until(`window.mockChat && window.mockChoice`, "the overlay");
    const choice = (position) =>
      `window.mockChoice({ channels: { whisper: true, party: true, friend: true }, colors: { whisper: "#00ffff", party: "#00ffff", friend: "#00ffff" }, position: "${position}" }), true`;
    await page.evaluate(choice("right"));
    // Wider than the box: three words, of which two fit on a row.
    const long = { ...chatLine(1, "friend"), width: 420, words: [[0, 140], [142, 280], [282, 420]] };
    await page.evaluate(say([long, chatLine(2, "whisper")]));
    await page.until(`document.querySelectorAll(".side-message").length === 2`, "two messages in the box");
    const box = await page.evaluate(`(() => {
      const list = document.querySelector(".side-chat.right");
      const { right, bottom } = list.getBoundingClientRect();
      const words = [...list.querySelector(".side-message").querySelectorAll(".chat-word")].map((w) => w.getBoundingClientRect());
      return {
        color: getComputedStyle(list).backgroundColor,
        right,
        bottom,
        rows: new Set(words.map((w) => Math.round(w.top))).size,
        height: words[0].height,
        reach: Math.max(...words.map((w) => w.right)),
      };
    })()`);
    const [innerWidth, innerHeight] = await page.evaluate(`[innerWidth, innerHeight]`);
    const gamePixel = innerWidth / 300;
    assert.equal(box.color, "rgba(0, 0, 0, 0.5)", "the game's chat-box shade");
    assert.deepEqual([box.right, box.bottom], [innerWidth, innerHeight], "against the right edge, newest at the bottom");
    assert.ok(Math.abs(box.height - 16 * gamePixel) < 0.5, `a row is 16 game pixels tall, not ${box.height}px`);
    assert.equal(box.rows, 2, "the long line wraps");
    assert.ok(box.reach <= box.right, "no word runs past the box");
    await page.evaluate(choice("left"));
    await page.until(`document.querySelector(".side-chat.left")?.getBoundingClientRect().left === 0`, "against the left edge");
    await page.evaluate(choice("above-chat"));
    await page.until(`document.querySelectorAll(".split-message").length === 2`, "back above the chat box");
    assert.deepEqual(page.errors, []);
    page.close();
  });
});
