// Builds one bundle per first-party plugin folder: src/<id>/index.ts → dist/<id>.js
// A plugin may bundle only its own folder and @mesovibe/sdk.
//
// That rule is enforced twice. scripts/check-plugin-imports.mjs scans the source first, and also sees
// type-only imports. This script then checks what esbuild actually bundled, after it followed
// symlinks and package.json fields:
// - every bundled file is a real file inside the plugin's own folder or the SDK's build output;
// - the bundle is one JavaScript file with no import left in it (remote URLs, dynamic import());
// - plugin folders hold no symlinks and no files that change import resolution.
// A plugin that fails gets no bundle, and dist/ is cleared first so no stale bundle survives.
// The text check for import()/importScripts() is a heuristic; the Web Worker sandbox is the
// runtime boundary.
import { build } from "esbuild";
import { lstatSync, mkdirSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const real = (path) => {
  try {
    return realpathSync(path);
  } catch {
    return resolve(path);
  }
};

// Real paths throughout: esbuild reports the real path of every file it bundles.
const root = real(fileURLToPath(new URL(".", import.meta.url)));
const src = join(root, "src");
const dist = join(root, "dist");
const sdkDist = real(fileURLToPath(new URL("../sdk/dist/", import.meta.url)));
// Compared lowercase: macOS and Windows file systems ignore case, and so does esbuild there.
const RESOLVER_CONFIG = new Set(["package.json", "tsconfig.json", "jsconfig.json"]);
const changesResolution = (name) => RESOLVER_CONFIG.has(name.toLowerCase());

const within = (path, dir) => path === dir || path.startsWith(dir + sep);

function folderProblems(dir) {
  const problems = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    const stat = lstatSync(path);
    if (stat.isSymbolicLink()) problems.push(`${relative(root, path)} is a symlink`);
    else if (stat.isDirectory()) problems.push(...folderProblems(path));
    else if (changesResolution(name)) problems.push(`${relative(root, path)} changes how imports resolve`);
  }
  return problems;
}

async function buildPlugin(id) {
  const pluginRoot = join(src, id);
  const problems = folderProblems(pluginRoot);
  if (problems.length > 0) return problems;

  const outfile = join(dist, `${id}.js`);
  const result = await build({
    absWorkingDir: root,
    entryPoints: [join(pluginRoot, "index.ts")],
    outfile,
    // Only this package's own tsconfig, never one found in a plugin folder.
    tsconfig: join(root, "tsconfig.json"),
    write: false,
    metafile: true,
    bundle: true,
    format: "esm",
    platform: "browser",
    target: "es2022",
    logLevel: "warning",
  });
  const allowed = [pluginRoot, sdkDist];
  for (const input of Object.keys(result.metafile.inputs)) {
    let path;
    try {
      path = realpathSync(resolve(root, input));
    } catch {
      problems.push(`bundles ${input}, which is not a file`);
      continue;
    }
    if (!allowed.some((dir) => within(path, dir))) problems.push(`bundles ${relative(root, path)}`);
  }
  // A bundle must be self-contained. esbuild leaves some imports (https:// and // URLs, say)
  // external without being asked, and those never appear among the inputs.
  for (const output of Object.values(result.metafile.outputs)) {
    for (const kept of output.imports) problems.push(`keeps an import of ${kept.path}`);
  }
  if (result.outputFiles.length !== 1) {
    problems.push("produces more than one output file (CSS or assets); plugins render through the host");
  }
  const bundle = result.outputFiles[0];
  if (/\bimport\s*\(|\bimportScripts\s*\(/.test(bundle.text)) {
    problems.push("leaves a dynamic import() in its bundle; import plugin files statically");
  }
  if (problems.length === 0) {
    mkdirSync(dirname(outfile), { recursive: true });
    writeFileSync(outfile, bundle.contents);
  }
  return problems;
}

rmSync(dist, { recursive: true, force: true });
const failures = [];
const ids = [];
for (const name of readdirSync(src)) {
  const path = join(src, name);
  if (statSync(path).isDirectory()) ids.push(name);
  else if (changesResolution(name)) failures.push(`  src: ${relative(root, path)} changes how imports resolve`);
}
for (const id of ids) {
  for (const problem of await buildPlugin(id)) failures.push(`  ${id}: ${problem}`);
}
if (failures.length > 0) {
  console.error("A plugin may bundle only its own folder and @mesovibe/sdk:");
  console.error(failures.join("\n"));
  process.exit(1);
}
console.log(`built ${ids.length} plugin bundle(s): ${ids.join(", ")}`);
