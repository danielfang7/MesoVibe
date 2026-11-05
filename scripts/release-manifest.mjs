#!/usr/bin/env node
// Puts a release together for in-app updates (docs/RELEASING.md): latest.json, which every
// installed MesoVibe reads to find a new version, and the files it points at, under the stable
// names the website links. Nothing is uploaded until every signature checks out against the public
// key in app/tauri.conf.json, so a release can't ship a manifest installed copies would reject.
//
//   node scripts/release-manifest.mjs --version 0.2.0 --notes notes.md \
//     --mac target/release/bundle/macos/MesoVibe.app.tar.gz \
//     --dmg target/release/bundle/dmg/MesoVibe_0.2.0_aarch64.dmg \
//     --windows MesoVibe_0.2.0_x64-setup.exe [--out <folder>] [--publish]
//
// The Mac update package comes with its .sig from the release build. The Windows installer, built
// on the PC, is signed here with the update key (TAURI_SIGNING_PRIVATE_KEY_PATH and
// TAURI_SIGNING_PRIVATE_KEY_PASSWORD), so the key never leaves this Mac. --publish drafts the
// GitHub release with `gh`; it becomes visible to installed copies only once it's published and
// marked Latest, after the draft's files are tested on both OSes.

import { spawnSync } from "node:child_process";
import { createHash, createPublicKey, verify } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO = fileURLToPath(new URL("../", import.meta.url));

/** The names a release uploads, which the website's download links and latest.json point at. */
export const ASSETS = {
  mac: "MesoVibe.app.tar.gz",
  dmg: "MesoVibe.dmg",
  windows: "MesoVibe-Setup.exe",
  manifest: "latest.json",
  checksums: "SHA256SUMS",
};

/** The updater targets (tauri-plugin-updater: `<os>-<arch>`). */
const TARGETS = { mac: "darwin-aarch64", windows: "windows-x86_64" };

/** Ed25519 public keys as DER: this fixed prefix, then the 32-byte key. */
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

/** The lines of a minisign file that Tauri stores base64-encoded (public keys, .sig files). */
function minisignLines(base64Text, what) {
  const text = Buffer.from(base64Text.trim(), "base64").toString("utf8");
  const lines = text.split("\n").map((line) => line.trim());
  if (!lines[0]?.startsWith("untrusted comment:")) throw new Error(`${what} isn't a minisign file.`);
  return lines;
}

/** The algorithm, key id and key of a Tauri updater public key (the `pubkey` in tauri.conf.json). */
export function parsePublicKey(pubkey) {
  if (typeof pubkey !== "string" || pubkey.trim() === "") {
    throw new Error("app/tauri.conf.json has no updater public key (plugins.updater.pubkey). See docs/RELEASING.md.");
  }
  const raw = Buffer.from(minisignLines(pubkey, "The updater public key")[1] ?? "", "base64");
  if (raw.length !== 42 || raw.toString("latin1", 0, 2) !== "Ed") throw new Error("The updater public key isn't an Ed25519 minisign key.");
  return {
    keyId: raw.subarray(2, 10),
    key: createPublicKey({ key: Buffer.concat([ED25519_SPKI_PREFIX, raw.subarray(10)]), format: "der", type: "spki" }),
  };
}

/**
 * Throws unless `signature` (a .sig file's contents) signs `bytes` with `pubkey`, the way the
 * updater checks it (minisign: the file signature, then the global one over the trusted comment).
 */
export function verifySignature(bytes, signature, pubkey, what) {
  const { keyId, key } = parsePublicKey(pubkey);
  const lines = minisignLines(signature, `${what}'s signature`);
  const raw = Buffer.from(lines[1] ?? "", "base64");
  const trusted = lines[2] ?? "";
  const global = Buffer.from(lines[3] ?? "", "base64");
  if (raw.length !== 74 || !trusted.startsWith("trusted comment: ") || global.length !== 64) {
    throw new Error(`${what}'s signature is malformed.`);
  }
  if (!raw.subarray(2, 10).equals(keyId)) {
    throw new Error(`${what} was signed with another key than the one in app/tauri.conf.json.`);
  }
  const algorithm = raw.toString("latin1", 0, 2);
  // "ED": the file's BLAKE2b-512 hash is signed (what Tauri's signer makes); "Ed": the file itself.
  const message = algorithm === "ED" ? createHash("blake2b512").update(bytes).digest() : bytes;
  const sig = raw.subarray(10);
  if (!verify(null, message, key, sig)) throw new Error(`${what}'s signature doesn't match the file.`);
  const comment = Buffer.from(trusted.slice("trusted comment: ".length), "utf8");
  if (!verify(null, Buffer.concat([sig, comment]), key, global)) {
    throw new Error(`${what}'s signature has a tampered trusted comment.`);
  }
}

/** Where a release's files live, from the updater endpoint (one place names the repository). */
export function releaseBase(endpoint, version) {
  const match = /^https:\/\/github\.com\/([^/]+)\/([^/]+)\/releases\/latest\/download\/latest\.json$/.exec(endpoint ?? "");
  if (match === null) throw new Error(`The updater endpoint isn't a GitHub latest-release latest.json: ${endpoint}`);
  return `https://github.com/${match[1]}/${match[2]}/releases/download/v${version}/`;
}

/** The asset names a web page links under releases/latest/download/. */
export function linkedAssets(html) {
  return [...html.matchAll(/releases\/latest\/download\/([^"'?#\s<>]+)/g)].map((match) => match[1]);
}

/** latest.json for `version`, with each platform's URL and signature. */
export function buildManifest({ version, notes, pubDate, base, signatures }) {
  return {
    version,
    notes,
    pub_date: pubDate,
    platforms: {
      [TARGETS.mac]: { signature: signatures.mac, url: `${base}${ASSETS.mac}` },
      [TARGETS.windows]: { signature: signatures.windows, url: `${base}${ASSETS.windows}` },
    },
  };
}

/** The workspace version in Cargo.toml: the app's one version (tauri.conf.json has none). */
export function cargoVersion(cargoToml) {
  const workspace = cargoToml.split(/^\[workspace\.package\]$/m)[1] ?? "";
  const match = /^version\s*=\s*"([^"]+)"/m.exec(workspace);
  if (match === null) throw new Error("Cargo.toml has no [workspace.package] version.");
  return match[1];
}

/**
 * Checks and stages a release in `out`: the update packages and installers under their stable
 * names, latest.json and SHA256SUMS. Returns the staged file paths. Throws, staging nothing
 * useful, on any mismatch.
 */
export function stageRelease({ version, notes, files, signatures, config, cargoToml, webHtml, out, now = new Date() }) {
  const expected = cargoVersion(cargoToml);
  if (version !== expected) throw new Error(`--version ${version} isn't the app's version in Cargo.toml (${expected}).`);
  const updater = config?.plugins?.updater ?? {};
  const base = releaseBase(updater.endpoints?.[0], version);
  const staged = { mac: ASSETS.mac, dmg: ASSETS.dmg, windows: ASSETS.windows };
  const missing = linkedAssets(webHtml).filter((name) => !Object.values(ASSETS).includes(name));
  if (missing.length > 0) throw new Error(`web/index.html links release files this release doesn't upload: ${missing.join(", ")}`);
  for (const which of ["mac", "windows"]) {
    if (signatures[which] === undefined) throw new Error(`No signature for the ${which} update.`);
    verifySignature(readFileSync(files[which]), signatures[which], updater.pubkey, basename(files[which]));
  }
  mkdirSync(out, { recursive: true });
  const paths = [];
  const sums = [];
  for (const [which, name] of Object.entries(staged)) {
    const path = join(out, name);
    copyFileSync(files[which], path);
    paths.push(path);
    sums.push(`${createHash("sha256").update(readFileSync(path)).digest("hex")}  ${name}`);
  }
  const manifest = buildManifest({ version, notes, pubDate: now.toISOString().replace(/\.\d{3}Z$/, "Z"), base, signatures });
  const manifestPath = join(out, ASSETS.manifest);
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  sums.push(`${createHash("sha256").update(readFileSync(manifestPath)).digest("hex")}  ${ASSETS.manifest}`);
  const sumsPath = join(out, ASSETS.checksums);
  writeFileSync(sumsPath, `${sums.join("\n")}\n`);
  return [...paths, manifestPath, sumsPath];
}

/**
 * A .sig next to `file`; when it's missing, it's made with the update key on this Mac
 * (~/.mesovibe/updater.key, or $MESOVIBE_UPDATER_KEY). The signer asks for the key's password
 * unless TAURI_SIGNING_PRIVATE_KEY_PASSWORD is set; it never goes on a command line.
 */
function signatureFor(file, { sign }) {
  const sigPath = `${file}.sig`;
  if (!existsSync(sigPath)) {
    if (!sign) throw new Error(`${sigPath} is missing.`);
    const key = process.env.MESOVIBE_UPDATER_KEY ?? join(homedir(), ".mesovibe", "updater.key");
    if (!existsSync(key)) throw new Error(`${basename(file)} has no .sig, and there's no update key at ${key} to sign it.`);
    // The key goes by its path; a key left in the environment by the build would clash with it.
    const env = { ...process.env };
    delete env.TAURI_SIGNING_PRIVATE_KEY;
    delete env.TAURI_SIGNING_PRIVATE_KEY_PATH;
    const signed = spawnSync("pnpm", ["exec", "tauri", "signer", "sign", "-f", key, file], {
      cwd: REPO,
      env,
      stdio: "inherit",
      shell: process.platform === "win32",
    });
    if (signed.status !== 0) throw new Error(`Signing ${basename(file)} failed.`);
  }
  return readFileSync(sigPath, "utf8").trim();
}

/**
 * The `gh` arguments that draft release v<version>. GitHub makes the tag when the draft is
 * published, so the draft names `commit`, the one the release was built from; with no target the
 * tag would land on wherever main is by then.
 */
export function draftArgs({ version, notesFile, commit, files }) {
  return ["release", "create", `v${version}`, "--draft", "--target", commit, "--title", `MesoVibe ${version}`, "--notes-file", notesFile, ...files];
}

/** The commit being released: this checkout's HEAD, which must already be on GitHub's main. */
function releaseCommit() {
  const git = (...args) => spawnSync("git", args, { cwd: REPO, encoding: "utf8" });
  const head = git("rev-parse", "HEAD");
  if (head.status !== 0) throw new Error("Couldn't read the current commit.");
  const commit = head.stdout.trim();
  if (git("fetch", "--quiet", "origin", "main").status !== 0) throw new Error("Couldn't fetch origin/main to check the release commit.");
  if (git("merge-base", "--is-ancestor", commit, "origin/main").status !== 0) {
    throw new Error(`${commit.slice(0, 7)} isn't on origin/main. Release from a commit that's merged to main.`);
  }
  return commit;
}

function parseArgs(argv) {
  const args = { publish: false };
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    if (flag === "--publish") args.publish = true;
    else if (["--version", "--notes", "--mac", "--dmg", "--windows", "--out"].includes(flag)) args[flag.slice(2)] = argv[(i += 1)];
    else throw new Error(`Unknown argument: ${flag}`);
  }
  for (const required of ["version", "notes", "mac", "dmg", "windows"]) {
    if (!args[required]) throw new Error(`--${required} is required. See the top of scripts/release-manifest.mjs.`);
  }
  return args;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  // Checked first, so a commit that can't be released fails before the key's password is asked.
  const commit = args.publish ? releaseCommit() : null;
  const files = { mac: args.mac, dmg: args.dmg, windows: args.windows };
  const out = args.out ?? mkdtempSync(join(tmpdir(), `mesovibe-${args.version}-`));
  const staged = stageRelease({
    version: args.version,
    notes: readFileSync(args.notes, "utf8").trim(),
    files,
    signatures: { mac: signatureFor(files.mac, { sign: false }), windows: signatureFor(files.windows, { sign: true }) },
    config: JSON.parse(readFileSync(join(REPO, "app/tauri.conf.json"), "utf8")),
    cargoToml: readFileSync(join(REPO, "Cargo.toml"), "utf8"),
    webHtml: readFileSync(join(REPO, "web/index.html"), "utf8"),
    out,
  });
  console.log(`Every signature checks out. Staged in ${out}:`);
  for (const path of staged) console.log(`  ${basename(path)}`);
  if (!args.publish) {
    console.log("Rerun with --publish to draft the GitHub release.");
    return;
  }
  const tag = `v${args.version}`;
  const created = spawnSync("gh", draftArgs({ version: args.version, notesFile: args.notes, commit, files: staged }), {
    cwd: REPO,
    stdio: "inherit",
  });
  if (created.status !== 0) throw new Error("gh release create failed.");
  console.log(`Drafted ${tag} at ${commit.slice(0, 7)}. Test its files on both OSes, then publish it and mark it Latest (docs/RELEASING.md).`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
