// Protects the plugin boundary: plugins import only @mesovibe/sdk or their own files.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { checkPlugins } from "./check-plugin-imports.mjs";

function withPlugins(files, fn) {
  const root = mkdtempSync(join(tmpdir(), "mesovibe-imports-"));
  try {
    for (const [path, source] of Object.entries(files)) {
      const full = join(root, path);
      mkdirSync(join(full, ".."), { recursive: true });
      writeFileSync(full, source);
    }
    return fn(root);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

const specifiersOf = (violations) => violations.map((v) => v.specifier).sort();

test("allows the SDK, its subpaths and files inside the same plugin", () => {
  withPlugins(
    {
      "alpha/index.ts": 'import type { Variant } from "@mesovibe/sdk";\nimport { x } from "./util";\nexport * from "./parts/more";',
      "alpha/util.ts": 'import { y } from "@mesovibe/sdk/components";\nexport const x = 1;',
      "alpha/parts/more.ts": 'import { x } from "../util";\nexport const z = x;',
    },
    (root) => assert.deepEqual(checkPlugins(root), []),
  );
});

test("rejects reaching into another plugin, the UI or host internals", () => {
  // Only @mesovibe/sdk and its "/" subpaths pass, not a look-alike scoped package like @mesovibe/sdk-internal.
  // Fails if the SDK check is loosened to startsWith("@mesovibe/sdk").
  withPlugins(
    {
      "alpha/index.ts": [
        'import { a } from "../beta/index";',
        'import { b } from "../../ui/src/main";',
        'import { c } from "@mesovibe/ui";',
        'import { d } from "@mesovibe/sdk-internal";',
      ].join("\n"),
      "beta/index.ts": "export const a = 1;",
    },
    (root) =>
      assert.deepEqual(specifiersOf(checkPlugins(root)), [
        "../../ui/src/main",
        "../beta/index",
        "@mesovibe/sdk-internal",
        "@mesovibe/ui",
      ]),
  );
});

test("rejects third-party packages and Node built-ins", () => {
  withPlugins(
    { "alpha/index.ts": 'import fs from "node:fs";\nimport React from "react";' },
    (root) => assert.deepEqual(specifiersOf(checkPlugins(root)), ["node:fs", "react"]),
  );
});

test("catches dynamic imports, require and side-effect imports", () => {
  withPlugins(
    {
      "alpha/index.ts": 'const m = await import("../beta/x");\nconst r = require("child_process");\nimport "../../app/secret";',
    },
    (root) =>
      assert.deepEqual(specifiersOf(checkPlugins(root)), ["../../app/secret", "../beta/x", "child_process"]),
  );
});

test("a sibling folder whose name starts with the plugin's name is still another plugin", () => {
  withPlugins(
    { "alpha/index.ts": 'import { a } from "../alpha-evil/index";', "alpha-evil/index.ts": "export const a = 1;" },
    (root) => assert.deepEqual(specifiersOf(checkPlugins(root)), ["../alpha-evil/index"]),
  );
});

test("checks re-exports, multi-line imports, nested folders and every JS/TS source extension", () => {
  // Every import form in every plugin source file is checked, not just one-line imports in top-level .ts.
  // Fails if export-from or multi-line matching is dropped, the folder walk stops recursing, or SOURCE_EXT narrows to .ts.
  withPlugins(
    {
      "alpha/index.ts": 'export { a } from "../beta/index";\nimport {\n  b,\n  c,\n} from "../beta/multi";',
      "alpha/deep/er/view.tsx": 'import React from "react";',
      "alpha/legacy.js": 'const cp = require("child_process");',
      "alpha/worker.mts": 'import "node:net";',
      "beta/index.ts": "export const a = 1;",
    },
    (root) =>
      assert.deepEqual(specifiersOf(checkPlugins(root)), [
        "../beta/index",
        "../beta/multi",
        "child_process",
        "node:net",
        "react",
      ]),
  );
});

test("catches compact imports written without spaces and template-literal dynamic imports", () => {
  // Valid imports with no spaces (import{a}from"x", import*as, export{x}from) or as import(`x`) are still checked.
  // Fails if the scanner needs whitespace after import/export or quotes in import(), so these pass silently.
  withPlugins(
    {
      "alpha/a.ts": 'import{a}from"../beta/index";',
      "alpha/b.ts": 'import*as r from "react";',
      "alpha/c.ts": 'export{x}from"../beta/reexport";',
      "alpha/d.ts": "const m = await import(`../beta/lazy`);",
      "beta/index.ts": "export const a = 1;",
    },
    (root) =>
      assert.deepEqual(specifiersOf(checkPlugins(root)), [
        "../beta/index",
        "../beta/lazy",
        "../beta/reexport",
        "react",
      ]),
  );
});

test('bare "." and ".." are relative: allowed while inside the plugin, rejected once they leave it', () => {
  // Fails if only "./" and "../" prefixes count as relative, or "." and ".." are allowed without resolving where they point.
  withPlugins(
    {
      "alpha/index.ts": 'import { b } from ".";\nimport { c } from "..";',
      "alpha/deep/index.ts": 'import { b } from "..";',
    },
    (root) =>
      assert.deepEqual(
        checkPlugins(root).map((v) => `${relative(root, resolve(v.file)).split(sep).join("/")}: ${v.specifier}`),
        ["alpha/index.ts: .."],
      ),
  );
});

const CHECKER = fileURLToPath(new URL("./check-plugin-imports.mjs", import.meta.url));
const runChecker = (...args) => spawnSync(process.execPath, [CHECKER, ...args], { encoding: "utf8" });

test("the CLI exits 1 naming each violation, 0 on a clean tree and 2 without a directory", () => {
  // `pnpm build` relies on these exit codes to stop a disallowed plugin import.
  // Fails if the CLI prints but exits 0, its main-module guard stops matching, or the usage error exits 0.
  withPlugins({ "alpha/index.ts": 'import fs from "node:fs";' }, (root) => {
    const result = runChecker(root);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /index\.ts.*node:fs/);
  });
  withPlugins({ "alpha/index.ts": 'import type { Variant } from "@mesovibe/sdk";' }, (root) => {
    const result = runChecker(root);
    assert.equal(result.status, 0, result.stderr);
  });
  assert.equal(runChecker().status, 2);
});

test("the CLI still enforces the rule when run through a symlinked script path", (t) => {
  // `pnpm build` must still fail on a violation when the checker's path has a symlink in it (a checkout under macOS /tmp, say).
  // Fails if the main-module guard compares the unresolved argv[1] with the real module path, so the CLI exits 0 silently.
  withPlugins({ "src/alpha/index.ts": 'import fs from "node:fs";' }, (root) => {
    const link = join(root, "check-plugin-imports.mjs");
    try {
      symlinkSync(CHECKER, link);
    } catch (error) {
      if (process.platform === "win32" && error.code === "EPERM") return t.skip("creating symlinks needs Developer Mode on Windows");
      throw error;
    }
    const result = spawnSync(process.execPath, [link, join(root, "src")], { encoding: "utf8" });
    assert.equal(result.status, 1, `exit ${result.status} via ${link}; stdout: ${result.stdout}; stderr: ${result.stderr}`);
    assert.match(result.stderr, /index\.ts.*node:fs/);
  });
});

test("fails closed: comments, regex literals, string names, escapes and directives can't hide an import", () => {
  // Fails if the raw or the comment-stripped scan is dropped, the from-clause can't cross a quoted name, escapes resolve,
  // or directives pass. Imports built at runtime are out of reach here; plugin-bundle-gate.test.mjs checks what ships.
  withPlugins(
    {
      "alpha/a.ts": 'import {\n  x, // don\'t remove\n} from "../beta/index";',
      "alpha/b.ts": 'import { "a-b" as ab } from "../beta/named";\nexport { "c-d" } from "../beta/reexport";',
      "alpha/c.ts": 'import /* polyfill */ "node:fs";\nawait import(/* lazy */ "../beta/lazy");',
      "alpha/d.ts": 'await import("../beta/data.json", { with: { type: "json" } });\nawait import("../beta/trailing",);',
      "alpha/e.ts": 'import { y } from "./\\u002e\\u002e/beta/index";',
      "alpha/f.ts": '// import fake from "lodash";\n/* import "../beta/old"; */\nconst url = "http://example.com";\nexport { url };',
      "alpha/g.ts": '/// <reference lib="dom" />\nexport const z = 1;',
      "alpha/h.ts": 'const trim = (s) => s.replace(/\\/*$/, "");\nimport type { T } from "../beta/types";\nexport { trim };',
      "beta/index.ts": "export const x = 1;",
    },
    (root) =>
      assert.deepEqual(specifiersOf(checkPlugins(root)), [
        "../beta/data.json",
        "../beta/index",
        "../beta/lazy",
        "../beta/named",
        "../beta/old",
        "../beta/reexport",
        "../beta/trailing",
        "../beta/types",
        "./\\u002e\\u002e/beta/index",
        '/// <reference lib="dom" />',
        "lodash",
        "node:fs",
      ]),
  );
});
