# Releasing MesoVibe

How a version reaches players: the installers on the website, and the in-app update every
installed copy picks up (`app/src/updater.rs`). Read the whole page before a first release.

## How updates work

Every installed MesoVibe looks at
`https://github.com/danielfang7/MesoVibe/releases/latest/download/latest.json` 30 seconds after it
starts and then once a day (an hour after a failed look). That URL always serves the `latest.json`
of the release marked **Latest**; drafts and prereleases are invisible to it. When `latest.json`
names a higher version than the copy's own, the copy downloads that platform's update package in
the background, checks its signature against the public key built into it, and shows
"MesoVibe <version> is ready." with **Restart to update**. Nothing installs until the player clicks.

The check sends nothing about the player or the game: a GET of a static file with the user agent
`tauri-plugin-updater/<version>`. GitHub sees the IP address, as for any web request. This is what
the first-run screen and Settings > About say.

```
 Mac:  scripts/release-macos.sh ──► MesoVibe.app.tar.gz + .sig, MesoVibe_<v>_aarch64.dmg
 PC:   pnpm tauri build --bundles nsis ──► MesoVibe_<v>_x64-setup.exe ──► copied to the Mac
 Mac:  scripts/release-manifest.mjs ──► signs the Windows installer, checks every signature,
         latest.json + SHA256SUMS + stable names ──► gh release create v<v> --draft
 You:  test the draft's files on both machines ──► publish it, marked Latest
```

## The version

Cargo.toml's `[workspace.package] version` is the app's only version: `app/tauri.conf.json` has
none, so the bundles, Settings > About and the updater all read the same number. An installed copy
offers an update only for a **higher** version. Bump it in its own commit before building.

## The update key

Update packages are signed with the update key, a minisign key made by `tauri signer`. It's
separate from the Apple Developer ID and any Windows code-signing certificate.

- **Where it lives:** `~/.mesovibe/updater.key` on the maintainer's Mac, locked with a password.
  Only that Mac signs. Windows installers are built elsewhere and signed there at release time, so
  the key never sits on the Windows machine or in CI.
- **Backup:** the key file and its password, in a password manager. A lost Mac must not strand
  every install.
- **Public half:** `plugins.updater.pubkey` in `app/tauri.conf.json`. A unit test
  (`updater.rs`) fails if it's missing or malformed.
- **Making it** (once): from the repo root, `pnpm tauri signer generate -w ~/.mesovibe/updater.key`
  and choose a password. Paste the contents of `~/.mesovibe/updater.key.pub` into `pubkey`.

### Rotating the key

Installed copies trust only the public key they were built with, so a new key has to arrive
through an update signed with the old one:

1. Make the new key (`pnpm tauri signer generate -w ~/.mesovibe/updater-new.key`) and back it up.
2. Put the new public key in `app/tauri.conf.json`, bump the version, and release that version
   signed with the **old** key (`MESOVIBE_UPDATER_KEY=~/.mesovibe/updater.key`).
3. Once that release has been Latest for a while (players update within a day or two of starting
   MesoVibe), sign every later release with the new key and retire the old one.

### If the key is lost

Installed copies can't update any more. Make a new key, ship a release with its public key, and
tell players (website, Discord, release notes) to install it by hand once. Updates work again from
there.

### If the key leaks

Anyone holding it and its password could publish an update that installed copies accept, but only
through this repository's GitHub releases (the endpoint is fixed in every copy). Rotate at once
(above), and check the repository's releases and access.

## Making a release

Prerequisites: the Apple credentials file (`~/.mesovibe/apple.env` or `MESOVIBE_APPLE_ENV`; see
`scripts/release-macos.sh`), the update key and its password, `gh` signed in, and a Windows machine
with Rust, Node 24, pnpm and the Visual Studio C++ build tools.

1. **Bump the version** in Cargo.toml (`[workspace.package]`) and merge it to `main`.
2. **Mac:** `scripts/release-macos.sh`. It asks for the update key's
   password, builds, signs and notarizes the app, the DMG and the update package, and checks the
   app inside the update package too.
3. **Windows:** on the Windows machine, at the same commit:
   `pnpm install --frozen-lockfile`, `pnpm build`, then `pnpm tauri build --bundles nsis`. Copy
   `target\release\bundle\nsis\MesoVibe_<version>_x64-setup.exe` to the Mac.
4. **Notes:** write the release notes, in players' words, to a file.
5. **Stage and draft:**
   ```
   node scripts/release-manifest.mjs --version <version> --notes <notes file> --mac target/release/bundle/macos/MesoVibe.app.tar.gz --dmg target/release/bundle/dmg/MesoVibe_<version>_aarch64.dmg --windows <path to the setup.exe> --publish
   ```
   It signs the Windows installer with the update key (asking for the password), checks every
   signature against the public key in `app/tauri.conf.json`, checks that the website's download
   links name files it uploads, and drafts release `v<version>` with `MesoVibe.dmg`,
   `MesoVibe-Setup.exe`, `MesoVibe.app.tar.gz`, `latest.json` and `SHA256SUMS`. The draft names
   this checkout's commit, which must be on `origin/main`, and GitHub makes the tag there when
   it's published. Leave out `--publish` to stage only.
6. **Test the draft's installers** (required: a release that breaks at start can't update itself
   away). On each machine, install the draft's `MesoVibe.dmg` or `MesoVibe-Setup.exe` by hand: it
   starts, reads the game, and Settings > About shows the new version. Then reinstall the previous
   release on one machine for step 7.
7. **Publish** the draft, marked **Latest**, and test the update itself at once: on the machine with
   the previous release, Settings > About > **Check now** shows "ready", and **Restart to update**
   comes back as the new version with the panel open (on Windows, "Open with MapleStory Classic"
   still on). If anything fails, mark the previous release Latest again (see Rolling back) and fix.
   Within a day, installed copies show "Restart to update"; watch bug reports for the first hours.

### When the previous release has no updater

0.1.0 has none, so for the first release (0.2.0) step 7 starts from a **test copy** instead: the
release commit built with a lower version. It's the release's own code, so this tests the updater
players get. Build it after step 5, since it reuses the release's build folder:

- **Mac:** `pnpm tauri build --bundles app --config '{"version":"0.1.1"}'`, then copy
  `target/release/bundle/macos/MesoVibe.app` to Applications. It isn't notarized, but a copy built
  on this Mac runs. After the update, macOS asks for Screen Recording again, because the signer
  changed; a real update keeps it.
- **Windows** (PowerShell): `Set-Content $env:TEMP\test-copy.json '{"version":"0.1.1"}'`, then
  `pnpm tauri build --bundles nsis --config $env:TEMP\test-copy.json`, and run
  `target\release\bundle\nsis\MesoVibe_0.1.1_x64-setup.exe`.

Settings > About in the test copy says 0.1.1. After step 7 publishes the release, **Check now**
there finds it. Every later release tests from the release before it.

## Rolling back

An installed copy only takes a higher version. First, mark the previous release **Latest** again:
`latest.json` then names the older version, so copies that haven't updated stop being offered the
bad one. Then publish a new, higher version built from the last good commit; copies that already
took the bad release update to it.

## Building releases in CI

Releases are built on the maintainer's machines for now, and every release publishes `SHA256SUMS`.
Moving the builds into GitHub Actions changes where the update key lives, so it needs its own plan
for keeping the key safe.
