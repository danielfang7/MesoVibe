import { fileURLToPath } from "node:url";

import { defineConfig } from "vite";

// The panel takes the SDK from source, so it builds and runs (scripts/panel.test.mjs) without the
// SDK's dist. Plugins still bundle against the SDK's build output (plugins/build.mjs).
export default defineConfig({
  resolve: {
    alias: {
      "@mesovibe/sdk": fileURLToPath(new URL("../sdk/src/index.ts", import.meta.url)),
    },
  },
  server: { strictPort: true },
  // Three pages: the panel, and the mini-HUD and chat overlay the host opens over the game
  // (app/src/dock.rs).
  build: {
    rollupOptions: {
      input: {
        panel: fileURLToPath(new URL("index.html", import.meta.url)),
        hud: fileURLToPath(new URL("hud.html", import.meta.url)),
        chat: fileURLToPath(new URL("chat.html", import.meta.url)),
      },
    },
  },
});
