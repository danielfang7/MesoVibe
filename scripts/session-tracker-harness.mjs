// Runs the Session Tracker plugin as the host does: bundled from source with the SDK, run in an
// isolated context with a fake Worker scope, and driven only through the SDK's messages.
// `synthetic: true` wires in the stand-in count source from scripts/fixtures, so potion accounting
// and views can be tested before a real source counts anything.
import { createRequire } from "node:module";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const esbuild = createRequire(join(REPO, "plugins", "package.json"))("esbuild");

/** Points the plugin's count-source module at the stand-in. */
const syntheticSource = {
  name: "synthetic-count-source",
  setup(build) {
    build.onResolve({ filter: /^\.\/count-source\.js$/ }, () => ({ path: join(REPO, "scripts/fixtures/synthetic-count-source.mjs") }));
  },
};

/** The plugin's bundle, as the host loads it. */
export async function bundleTracker({ synthetic = false } = {}) {
  const result = await esbuild.build({
    entryPoints: [join(REPO, "plugins/src/session-tracker/index.ts")],
    bundle: true,
    write: false,
    format: "esm",
    platform: "browser",
    target: "es2022",
    alias: { "@mesovibe/sdk": join(REPO, "sdk/src/index.ts") },
    plugins: synthetic ? [syntheticSource] : [],
    logLevel: "silent",
  });
  return result.outputFiles[0].text;
}

/** Starts `bundle`; returns a function to send it a message and the messages it sent. */
export function run(bundle) {
  const sent = [];
  const scope = { postMessage: (message) => sent.push(structuredClone(message)), onmessage: null };
  scope.globalThis = scope;
  for (const name of ["structuredClone", "Date", "Intl", "Math", "JSON", "Number", "Array", "Object", "String", "Map", "Set"]) {
    scope[name] = globalThis[name];
  }
  vm.createContext(scope);
  vm.runInContext(bundle, scope);
  return { send: (data) => scope.onmessage({ data }), sent };
}
