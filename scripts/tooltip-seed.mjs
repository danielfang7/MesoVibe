#!/usr/bin/env node
// Draws the tooltip reader's starting letter shapes (engine/data/tooltip-seed.txt) from Liberation
// Sans Bold, an openly licensed font close to the one the game draws item names in. The reader
// takes them as guesses, so it can name items before the game has shown it every letter. Run it
// after changing how the shapes are drawn (engine/examples/tooltip_seed.rs) and commit what
// changed:
//
//   node scripts/tooltip-seed.mjs
//
// It downloads the font's official release, checks it against the sha256 below before using it,
// writes the font's license beside the shapes, and runs the example that draws them. The font
// file itself is neither kept in the repo nor shipped: only the shapes drawn from it are.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gunzipSync } from "node:zlib";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const OUT = join(REPO, "engine/data");
const USER_AGENT = "MesoVibe tooltip-seed script (https://github.com/danielfang7/MesoVibe)";

/** Liberation Fonts 2.1.5, the TrueType release its release page links, and that file's sha256. */
export const RELEASE =
  "https://github.com/liberationfonts/liberation-fonts/files/7261482/liberation-fonts-ttf-2.1.5.tar.gz";
export const RELEASE_SHA256 = "7191c669bf38899f73a2094ed00f7b800553364f90e2637010a69c0e268f25d0";
const FOLDER = "liberation-fonts-ttf-2.1.5/";

/** The bytes, once their sha256 is the one expected; an error naming both otherwise. */
export function checked(bytes, sha256, what) {
  const got = createHash("sha256").update(bytes).digest("hex");
  if (got !== sha256) throw new Error(`${what}: sha256 ${got}, expected ${sha256}`);
  return bytes;
}

/** The plain files in an uncompressed tar archive, by path: enough to unpack a font release. */
export function untar(bytes) {
  const files = new Map();
  let at = 0;
  while (at + 512 <= bytes.length) {
    const header = bytes.subarray(at, at + 512);
    if (header.every((b) => b === 0)) break;
    const text = (from, to) => header.subarray(from, to).toString("latin1").replace(/\0[\s\S]*$/, "");
    const size = parseInt(text(124, 136).trim() || "0", 8);
    if (!Number.isInteger(size)) throw new Error(`tar entry at ${at}: no size`);
    // Only the POSIX format keeps a path prefix there; the older GNU one ("ustar" and a space)
    // keeps other fields in those bytes.
    const prefix = text(257, 263) === "ustar" ? text(345, 500) : "";
    const name = (prefix ? `${prefix}/` : "") + text(0, 100);
    const type = text(156, 157);
    at += 512;
    if (type === "0" || type === "") files.set(name, bytes.subarray(at, at + size));
    at += Math.ceil(size / 512) * 512;
  }
  return files;
}

async function main() {
  const response = await fetch(RELEASE, { headers: { "User-Agent": USER_AGENT } });
  if (!response.ok) throw new Error(`${RELEASE}: HTTP ${response.status}`);
  const archive = checked(Buffer.from(await response.arrayBuffer()), RELEASE_SHA256, RELEASE);
  const files = untar(gunzipSync(archive));
  const font = files.get(`${FOLDER}LiberationSans-Bold.ttf`);
  const license = files.get(`${FOLDER}LICENSE`);
  if (!font || !license) throw new Error(`${RELEASE}: no LiberationSans-Bold.ttf or LICENSE in it`);
  const dir = mkdtempSync(join(tmpdir(), "mesovibe-font-"));
  try {
    const path = join(dir, "LiberationSans-Bold.ttf");
    writeFileSync(path, font);
    const run = spawnSync(
      "cargo",
      ["run", "--quiet", "--release", "-p", "mesovibe-engine", "--example", "tooltip_seed", "--", path, join(OUT, "tooltip-seed.txt")],
      { cwd: REPO, stdio: "inherit" },
    );
    if (run.status !== 0) throw new Error(`drawing the letters failed (${run.error?.message ?? `exit ${run.status}`})`);
    writeFileSync(join(OUT, "LiberationSans-OFL.txt"), license);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
