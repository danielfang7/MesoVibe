# Contributing to MesoVibe

Thanks for helping. Bug fixes and small improvements are welcome as pull requests. For a new feature or a larger change, open an issue first so the approach can be agreed before you write the code.

## Fair play

MesoVibe reads the game only through screen capture of the game window. A change is not accepted if it:

- sends keyboard or mouse input to the game, or automates play in any way;
- reads or writes the game's memory or files, or injects code into it;
- reads, changes or proxies the game's network traffic.

This holds for plugins too.

## Building

You need Rust stable ([rustup](https://rustup.rs)), Node 24 and pnpm 12. On Windows, also the Visual Studio Build Tools with the C++ workload.

```sh
pnpm install
pnpm build        # the SDK, the panel and one bundle per plugin
pnpm tauri dev    # run the app
```

## Checks

Pull requests run these on Windows and macOS. Run them before you push:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm test
pnpm build
```

- `pnpm test` includes a smoke test of the panel in headless Chrome. It is skipped when Chrome isn't installed; set `MESOVIBE_CHROME` to use a particular browser.
- The SDK's event and reader-state types in `sdk/src/generated` are generated from `engine/src/events.rs`, `engine/src/notice_health.rs` and `engine/src/hp_mp_health.rs`. After changing any of them, run `MESOVIBE_UPDATE_SDK=1 cargo test -p mesovibe-engine events`.
- Plugins may import only from `@mesovibe/sdk` and their own folder. `pnpm build` fails on any other import.
- The potions the Session Tracker counts are kept by hand in `plugins/src/session-tracker/potions.ts`. Each shop price says where and when it was checked in the game; `pnpm test` checks the IDs, names and sell prices against `engine/data/items.tsv`.
- The price service in `web/api` is tested by `scripts/price-service.test.mjs` against a fake Cozy Markets and a fake store, so it needs no key. Its own dependencies (`web/package.json`, the Vercel Blob client) are installed by Vercel at deploy time with `npm ci`; `pnpm install` doesn't touch them. Running the service for real needs `COZY_MARKET_API_KEY`, `CRON_SECRET` and a private Blob store, all set for the Vercel project's Production environment only, so a preview deployment of a pull request can't read the key or write the price file.
- `scripts/session-tracker-harness.mjs` runs the Session Tracker the way the app does. Its `synthetic` option wires in the stand-in potion counter from `scripts/fixtures`, for testing potion costs and views without the game. The counter the app ships, which pairs quick-slot count changes with HP and MP rises (`plugins/src/session-tracker/count-source.ts`), is tested in `scripts/quick-slot-source.test.mjs`.

## Pull requests

- Keep each pull request to one change, and describe what it changes for players and how you tested it.
- Add or update tests for behavior you change.
- Include a screenshot for a visible change to the panel.
- Never commit screenshots or recordings of real play: chat shows other players' names. Keep captures outside the repository (`captures/` is ignored).

By contributing, you agree that your contributions are licensed under the [BSD 2-clause license](../LICENSE).
