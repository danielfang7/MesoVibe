// Protects release-manifest.mjs: latest.json is the one file every installed MesoVibe reads to find
// an update, so a wrong URL or signature would break updates everywhere at once. Keys and
// signatures here come from the real `tauri signer`, the format the updater plugin checks.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { ASSETS, cargoVersion, draftArgs, linkedAssets, stageRelease } from "./release-manifest.mjs";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const CONFIG = JSON.parse(readFileSync(join(REPO, "app/tauri.conf.json"), "utf8"));
const CARGO = readFileSync(join(REPO, "Cargo.toml"), "utf8");
const WEB = readFileSync(join(REPO, "web/index.html"), "utf8");
const VERSION = cargoVersion(CARGO);

function tauri(...args) {
  const run = spawnSync("pnpm", ["exec", "tauri", ...args], { cwd: REPO, encoding: "utf8", shell: process.platform === "win32" });
  assert.equal(run.status, 0, `tauri ${args[0]} ${args[1]}: ${run.stderr}`);
}

/** A throwaway update key, files signed with it, and a config that trusts it. */
function setUp() {
  const dir = mkdtempSync(join(tmpdir(), "mesovibe-release-"));
  const key = join(dir, "updater.key");
  tauri("signer", "generate", "--ci", "-w", key, "-p", "test");
  const files = {};
  for (const [which, content] of Object.entries({ mac: "a Mac update", dmg: "a disk image", windows: "a Windows installer" })) {
    files[which] = join(dir, `${which}.bin`);
    writeFileSync(files[which], content);
  }
  for (const which of ["mac", "windows"]) tauri("signer", "sign", "-f", key, "-p", "test", files[which]);
  const pubkey = readFileSync(`${key}.pub`, "utf8").trim();
  const config = { ...CONFIG, plugins: { updater: { ...CONFIG.plugins.updater, pubkey } } };
  const signatures = { mac: readFileSync(`${files.mac}.sig`, "utf8").trim(), windows: readFileSync(`${files.windows}.sig`, "utf8").trim() };
  const release = (change = {}) =>
    stageRelease({ version: VERSION, notes: "Fixes.", files, signatures, config, cargoToml: CARGO, webHtml: WEB, out: join(dir, "out"), now: new Date("2026-11-05T12:00:00.123Z"), ...change });
  return { dir, key, files, signatures, config, release };
}

// latest.json points every platform at a file this release uploads, under the names the website
// links, with signatures the updater accepts. Fails if a target key, URL, name or signature drifts.
test("a release is staged with latest.json at stable names, signed with the update key", (t) => {
  const { dir, signatures, release } = setUp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const staged = release();
  assert.deepEqual(staged.map((path) => path.slice(join(dir, "out").length + 1)).sort(), Object.values(ASSETS).sort());
  const manifest = JSON.parse(readFileSync(join(dir, "out", ASSETS.manifest), "utf8"));
  const base = `https://github.com/danielfang7/MesoVibe/releases/download/v${VERSION}/`;
  assert.deepEqual(manifest, {
    version: VERSION,
    notes: "Fixes.",
    pub_date: "2026-11-05T12:00:00Z",
    platforms: {
      "darwin-aarch64": { signature: signatures.mac, url: `${base}MesoVibe.app.tar.gz` },
      "windows-x86_64": { signature: signatures.windows, url: `${base}MesoVibe-Setup.exe` },
    },
  });
  const sums = readFileSync(join(dir, "out", ASSETS.checksums), "utf8");
  for (const name of [ASSETS.mac, ASSETS.dmg, ASSETS.windows, ASSETS.manifest]) assert.match(sums, new RegExp(`^[0-9a-f]{64}  ${name.replaceAll(".", "\\.")}$`, "m"));
});

// No release ships a signature installed copies would reject. Fails if the check is skipped, or
// accepts another key or a changed file.
test("a signature from another key, or for a changed file, stops the release", (t) => {
  const { dir, files, release } = setUp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const other = join(dir, "other.key");
  tauri("signer", "generate", "--ci", "-w", other, "-p", "test");
  const foreign = join(dir, "foreign.bin");
  writeFileSync(foreign, "a Windows installer");
  tauri("signer", "sign", "-f", other, "-p", "test", foreign);
  assert.throws(() => release({ signatures: { mac: readFileSync(`${files.mac}.sig`, "utf8"), windows: readFileSync(`${foreign}.sig`, "utf8") } }), /signed with another key/);
  writeFileSync(files.mac, "a Mac update, changed after signing");
  assert.throws(() => release(), /doesn't match the file/);
});

// Fails if any of these checks is dropped.
test("a release refuses a wrong version, a missing signature, a dead web link or no public key", (t) => {
  const { dir, signatures, config, release } = setUp();
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  assert.throws(() => release({ version: "9.9.9" }), /isn't the app's version in Cargo\.toml/);
  assert.throws(() => release({ signatures: { mac: signatures.mac } }), /No signature for the windows update/);
  assert.throws(() => release({ webHtml: `${WEB}<a href="https://github.com/x/y/releases/latest/download/MesoVibe-x64.dmg">` }), /MesoVibe-x64\.dmg/);
  const unkeyed = { ...config, plugins: { updater: { ...config.plugins.updater, pubkey: "" } } };
  assert.throws(() => release({ config: unkeyed }), /no updater public key/);
});

// The tag GitHub makes on publishing must name the commit that was built. Fails if the draft drops
// --target and the tag lands on whatever main is at publish time.
test("a draft release tags the commit it was built from", () => {
  const args = draftArgs({ version: "0.2.0", notesFile: "notes.md", commit: "2641533abc", files: ["a.dmg", "latest.json"] });
  assert.deepEqual(args.slice(0, 6), ["release", "create", "v0.2.0", "--draft", "--target", "2641533abc"]);
  assert.deepEqual(args.slice(-4), ["--notes-file", "notes.md", "a.dmg", "latest.json"]);
});

// Fails if the page or ASSETS renames a download.
test("the website links only files a release uploads", () => {
  const linked = linkedAssets(WEB);
  assert.ok(linked.length >= 2, "the page links the installers");
  for (const name of linked) assert.ok(Object.values(ASSETS).includes(name), name);
});
