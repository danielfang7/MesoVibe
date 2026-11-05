#!/bin/sh
# Builds MesoVibe.app, its DMG and its update package for macOS, signed with a Developer ID and
# notarized by Apple, then checks them the way Gatekeeper and the in-app updater will. Run it on a
# Mac from the repo root (the whole release: docs/RELEASING.md):
#
#   MESOVIBE_APPLE_ENV=/path/to/apple.env scripts/release-macos.sh
#
# The update package (MesoVibe.app.tar.gz and its .sig) is signed with the update key,
# ~/.mesovibe/updater.key or $MESOVIBE_UPDATER_KEY. The script asks for its password and never
# writes it down.
#
# The env file stays outside the repo and is never printed. It holds:
#   APPLE_ID                      the Apple ID that notarizes
#   APPLE_PASSWORD                an app-specific password for it (APPLE_ID_PASSWORD also works)
#   APPLE_TEAM_ID                 the Developer team ID
#   APPLE_SIGNING_IDENTITY        optional: the "Developer ID Application: ..." certificate in the
#                                 login keychain; the one for APPLE_TEAM_ID is found otherwise
# It refuses to build an app that wouldn't be signed and notarized, rather than ship one quietly.
set -eu

# Only committed code is released: the tag names a commit, and the Windows installer is built from
# the same one. The website and docs aren't part of the app.
if ! git diff --quiet HEAD -- . ':(exclude)web' ':(exclude)docs'; then
  echo "Commit or stash these changes first; a release is built from committed code only:" >&2
  git status --short --untracked-files=no -- . ':(exclude)web' ':(exclude)docs' >&2
  exit 1
fi

env_file="${MESOVIBE_APPLE_ENV:-$HOME/.mesovibe/apple.env}"
if [ ! -f "$env_file" ]; then
  echo "No Apple credentials at $env_file. Set MESOVIBE_APPLE_ENV to the env file." >&2
  exit 1
fi
set -a
# shellcheck disable=SC1090
. "$env_file"
set +a
APPLE_PASSWORD="${APPLE_PASSWORD:-${APPLE_ID_PASSWORD:-}}"
export APPLE_PASSWORD
for name in APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  eval "value=\${$name:-}"
  if [ -z "$value" ]; then
    echo "$env_file doesn't set $name." >&2
    exit 1
  fi
done
if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
  APPLE_SIGNING_IDENTITY=$(security find-identity -v -p codesigning |
    sed -n "s/.*\"\(Developer ID Application: .*($APPLE_TEAM_ID)\)\"/\1/p" | head -n 1)
fi
if [ -z "$APPLE_SIGNING_IDENTITY" ]; then
  echo "No \"Developer ID Application\" certificate for team $APPLE_TEAM_ID in the keychain." >&2
  exit 1
fi
export APPLE_SIGNING_IDENTITY
echo "Signing as: $APPLE_SIGNING_IDENTITY"

updater_key="${MESOVIBE_UPDATER_KEY:-$HOME/.mesovibe/updater.key}"
if [ ! -f "$updater_key" ]; then
  echo "No update key at $updater_key. Set MESOVIBE_UPDATER_KEY, or create it (docs/RELEASING.md)." >&2
  exit 1
fi
if [ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]; then
  trap 'stty echo 2>/dev/null' EXIT INT TERM
  printf "Update key password: " >&2
  stty -echo
  read -r TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  stty echo
  printf "\n" >&2
fi
TAURI_SIGNING_PRIVATE_KEY=$(cat "$updater_key")
export TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD

PATH="$HOME/.cargo/bin:$PATH"
pnpm install --frozen-lockfile
pnpm build
# CI=true makes Tauri build the DMG without scripting Finder, which fails when the screen is
# locked or no one is logged in. The DMG then has a plain window.
# Update packages are made only here, so other builds (CI, contributors) don't need the key.
CI=true pnpm tauri build --bundles app,dmg --config '{"bundle":{"createUpdaterArtifacts":true}}'

# The one version: Cargo.toml's (tauri.conf.json has none). Named, so an older DMG isn't picked.
version=$(node --input-type=module -e "import { readFileSync } from 'node:fs'; import { cargoVersion } from './scripts/release-manifest.mjs'; console.log(cargoVersion(readFileSync('Cargo.toml', 'utf8')))")
app="target/release/bundle/macos/MesoVibe.app"
update="$app.tar.gz"
dmg="target/release/bundle/dmg/MesoVibe_${version}_aarch64.dmg"
echo "Checking $app"
codesign --verify --deep --strict --verbose=2 "$app"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=4 "$app"
if [ ! -f "$update" ] || [ ! -f "$update.sig" ] || [ ! -f "$dmg" ]; then
  echo "The build is missing $update, its .sig or $dmg." >&2
  exit 1
fi
# Tauri notarizes the app but not the disk image around it, and Gatekeeper won't open a downloaded
# DMG that isn't notarized itself ("Unnotarized Developer ID").
echo "Notarizing $dmg"
xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
xcrun stapler staple "$dmg"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
# An installed copy replaces itself with what's in the update package: it must be the signed,
# notarized app too.
echo "Checking the app inside $update"
inside=$(mktemp -d)
tar -xzf "$update" -C "$inside"
codesign --verify --deep --strict "$inside/MesoVibe.app"
xcrun stapler validate "$inside/MesoVibe.app"
rm -rf "$inside"
echo "Built and notarized MesoVibe $version:"
echo "  $app"
echo "  $dmg"
echo "  $update (+ .sig)"
shasum -a 256 "$dmg" "$update"
echo "Next: the Windows installer, then scripts/release-manifest.mjs (docs/RELEASING.md)."
