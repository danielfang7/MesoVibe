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
- The SDK's event types in `sdk/src/generated` are generated from `engine/src/events.rs`. After changing that file, run `MESOVIBE_UPDATE_SDK=1 cargo test -p mesovibe-engine events`.
- Plugins may import only from `@mesovibe/sdk` and their own folder. `pnpm build` fails on any other import.

## Pull requests

- Keep each pull request to one change, and describe what it changes for players and how you tested it.
- Add or update tests for behavior you change.
- Include a screenshot for a visible change to the panel.
- Never commit screenshots or recordings of real play: chat shows other players' names. Keep captures outside the repository (`captures/` is ignored).

By contributing, you agree that your contributions are licensed under the [BSD 2-clause license](../LICENSE).
