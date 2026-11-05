// Copies the first-party plugin bundles (plugins/dist, built before this package) into the
// panel's static files, where the plugin host starts each one in its own Web Worker.
import { copyFileSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const from = join(here, "..", "plugins", "dist");
const to = join(here, "public", "plugins");
rmSync(to, { recursive: true, force: true });
mkdirSync(to, { recursive: true });
const bundles = readdirSync(from).filter((name) => name.endsWith(".js"));
if (bundles.length === 0) {
  console.error(`No plugin bundles in ${from}. Build the plugins first (pnpm build).`);
  process.exit(1);
}
for (const name of bundles) copyFileSync(join(from, name), join(to, name));
console.log(`copied ${bundles.length} plugin bundle(s) into the panel`);
