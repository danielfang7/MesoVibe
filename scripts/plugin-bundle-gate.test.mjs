// Protects the bundle-level half of the plugin boundary (plugins/build.mjs): a plugin bundle may
// contain only its own folder and the SDK, whatever the source scan missed.
import { test } from "node:test";
import assert from "node:assert/strict";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const PLUGINS = join(REPO, "plugins");
// A junction needs no extra rights on Windows; elsewhere a plain directory symlink.
const DIR_LINK = process.platform === "win32" ? "junction" : "dir";
// esbuild's real folder, the only package build.mjs imports. The temp checkout links straight to it.
// Linking to plugins/node_modules instead, itself a folder of pnpm links, broke on the Windows CI
// runner, where the checkout is on D: and the temp folder on C:.
const ESBUILD = dirname(realpathSync(createRequire(join(PLUGINS, "package.json")).resolve("esbuild/package.json")));

/** Copies plugins/build.mjs and its tsconfig into a temp checkout whose path has a space, with the given src/ files. */
function withPluginsDir(files, fn) {
  const tmp = mkdtempSync(join(tmpdir(), "mesovibe-bundle-"));
  const checkout = join(tmp, "with space");
  const plugins = join(checkout, "plugins");
  try {
    mkdirSync(join(plugins, "src"), { recursive: true });
    copyFileSync(join(PLUGINS, "build.mjs"), join(plugins, "build.mjs"));
    copyFileSync(join(PLUGINS, "tsconfig.json"), join(plugins, "tsconfig.json"));
    copyFileSync(join(REPO, "tsconfig.base.json"), join(checkout, "tsconfig.base.json"));
    mkdirSync(join(plugins, "node_modules"));
    symlinkSync(ESBUILD, join(plugins, "node_modules", "esbuild"), DIR_LINK);
    for (const [path, source] of Object.entries(files)) {
      mkdirSync(dirname(join(plugins, "src", path)), { recursive: true });
      writeFileSync(join(plugins, "src", path), source);
    }
    return fn(plugins);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

const runBuild = (plugins) => spawnSync(process.execPath, [join(plugins, "build.mjs")], { encoding: "utf8" });

test("builds a clean plugin from a checkout path that contains a space", () => {
  // `pnpm build` must work in any checkout path. Fails if build.mjs turns file URLs into paths with
  // URL.pathname, which keeps %20 and gives /C:/ on Windows.
  withPluginsDir(
    { "good/index.ts": 'import { x } from "./own";\nexport const y = x;', "good/own.ts": "export const x = 1;" },
    (plugins) => {
      const result = runBuild(plugins);
      assert.equal(result.status, 0, result.stderr);
      assert.ok(existsSync(join(plugins, "dist", "good.js")));
    },
  );
});

test("fails the build, and writes no bundle, for a plugin that reaches outside its folder", (t) => {
  // A bundle is one JS file of the plugin's own real files: no other plugin, remote import, symlink, resolver config, data: module, CSS or import().
  // Fails if a metafile input/output check, the folder or src-level config check (any case), the one-output check, the import() check or the dist/ reset is dropped.
  withPluginsDir(
    {
      "good/index.ts": 'import { x } from "./own";\nexport const y = x;',
      "good/own.ts": "export const x = 1;",
      "reach/index.ts": 'import {\n  x, // don\'t remove\n} from "../good/own";\nexport const z = x;',
      "remap/index.ts": "export const b = 1;",
      "remap/TSConfig.json": '{ "compilerOptions": { "paths": { "@mesovibe/sdk": ["../good/own.ts"] } } }',
      "dynamic/index.ts": 'const name = "./elsewhere.js";\nexport const load = () => import(name);',
      "linked/index.ts": "export const a = 1;",
      "remote/index.ts": 'import x from "https://example.invalid/a.js";\nexport * from "//cdn.example.invalid/b.js";\nexport const y = x;',
      "datauri/index.ts": 'import v from "data:text/javascript,export default 1";\nexport const w = v;',
      "styled/index.ts": 'import "./look.css";\nexport const s = 1;',
      "styled/look.css": "p { color: red; }",
      "package.json": '{ "sideEffects": false }',
    },
    (plugins) => {
      try {
        symlinkSync(join(plugins, "src", "good"), join(plugins, "src", "linked", "shared"), "dir");
      } catch (error) {
        if (error.code === "EPERM") return t.skip("symlinks need extra rights on this OS");
        throw error;
      }
      // Bundles from an earlier build must not survive a failing one.
      mkdirSync(join(plugins, "dist"), { recursive: true });
      for (const stale of ["reach", "orphan"]) writeFileSync(join(plugins, "dist", `${stale}.js`), "// old");

      const result = runBuild(plugins);
      assert.equal(result.status, 1, result.stdout);
      assert.match(result.stderr, /reach: bundles src[\\/]good[\\/]own\.ts/);
      assert.match(result.stderr, /remap: src[\\/]remap[\\/]TSConfig\.json changes how imports resolve/);
      assert.match(result.stderr, /src: src[\\/]package\.json changes how imports resolve/);
      assert.match(result.stderr, /dynamic: leaves a dynamic import\(\)/);
      assert.match(result.stderr, /linked: src[\\/]linked[\\/]shared is a symlink/);
      assert.match(result.stderr, /remote: keeps an import of https:\/\/example\.invalid\/a\.js/);
      assert.match(result.stderr, /remote: keeps an import of \/\/cdn\.example\.invalid\/b\.js/);
      assert.match(result.stderr, /datauri: bundles .*data:text\/javascript.*, which is not a file/);
      assert.match(result.stderr, /styled: produces more than one output file/);
      assert.ok(existsSync(join(plugins, "dist", "good.js")), "a clean plugin still builds");
      for (const id of ["reach", "remap", "dynamic", "linked", "remote", "datauri", "styled", "orphan"]) {
        assert.ok(!existsSync(join(plugins, "dist", `${id}.js`)), `${id} must have no bundle`);
      }
    },
  );
});
