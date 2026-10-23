#!/usr/bin/env bash
# One prebuilt release tool, as .github/release-tools.lock pins it: upstream's
# release file for the version the source pins (cargo-about: ABOUT_VERSION in
# scripts/release-public.sh; tauri-cli: TAURI_CLI_VERSION in desktop/build.sh),
# downloaded and checked against the lock's sha256, then its program put in
# <dest>/bin. Fails on any difference, and never builds or falls back to
# anything else.
#
#   .github/scripts/fetch-release-tool.sh <cargo-about|tauri-cli> <dest> [<platform>]
#
# <platform> is the Rust target the program is for (aarch64-apple-darwin, the
# macOS runners').
set -euo pipefail

TOOL=${1:?usage: fetch-release-tool.sh <cargo-about|tauri-cli> <dest> [<platform>]}
DEST=${2:?usage: fetch-release-tool.sh <cargo-about|tauri-cli> <dest> [<platform>]}
PLATFORM=${3:-aarch64-apple-darwin}
ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
LOCK=$ROOT/.github/release-tools.lock
die() { printf 'fetch-release-tool: %s: %s\n' "$TOOL" "$*" >&2; exit 1; }

case "$TOOL" in
  cargo-about) PINNED=$(sed -n 's/^ABOUT_VERSION=//p' "$ROOT/scripts/release-public.sh"); PROGRAM=cargo-about ;;
  tauri-cli) PINNED=$(sed -n 's/^TAURI_CLI_VERSION=//p' "$ROOT/desktop/build.sh"); PROGRAM=cargo-tauri ;;
  *) die "not a tool of .github/release-tools.lock" ;;
esac
[ -n "$PINNED" ] || die "cannot read the version the source pins"
LINE=$(awk -v tool="$TOOL" -v platform="$PLATFORM" '!/^#/ && $1 == tool && $3 == platform' "$LOCK")
[ -n "$LINE" ] || die "no line for $PLATFORM in $LOCK"
[ "$(printf '%s\n' "$LINE" | wc -l | tr -d ' ')" = 1 ] || die "more than one line for $PLATFORM in $LOCK"
read -r _ VERSION _ SHA256 URL <<<"$LINE"
[ "$VERSION" = "$PINNED" ] || die "$LOCK pins $VERSION, but the source wants $PINNED: run scripts/update-release-tool-pins.sh and commit its lines"
[[ "$SHA256" =~ ^[0-9a-f]{64}$ ]] || die "$LOCK has no sha256 for it"
case "$URL" in https://github.com/*) ;; *) die "$LOCK names $URL, not a GitHub release file" ;; esac

WORK=$(mktemp -d "${TMPDIR:-/tmp}/fetch-release-tool.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
FILE=$WORK/$(basename "$URL")
curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$FILE" "$URL" || die "cannot download $URL"
if command -v sha256sum >/dev/null 2>&1; then GOT=$(sha256sum "$FILE"); else GOT=$(shasum -a 256 "$FILE"); fi
GOT=${GOT%% *}
[ "$GOT" = "$SHA256" ] || die "$(basename "$URL") is $GOT, not the pinned $SHA256"

# The one program, by its name in the archive.
mkdir -p "$WORK/x" "$DEST/bin"
case "$FILE" in
  *.tar.gz) tar -xzf "$FILE" -C "$WORK/x" "$TOOL-$VERSION-$PLATFORM/$PROGRAM"; FOUND=$WORK/x/$TOOL-$VERSION-$PLATFORM/$PROGRAM ;;
  *.zip) unzip -q "$FILE" "$PROGRAM" -d "$WORK/x"; FOUND=$WORK/x/$PROGRAM ;;
  *) die "$(basename "$URL") is neither a .tar.gz nor a .zip" ;;
esac
[ -f "$FOUND" ] && [ ! -L "$FOUND" ] || die "$(basename "$URL") has no file $PROGRAM"
cp "$FOUND" "$DEST/bin/$PROGRAM"
chmod 755 "$DEST/bin/$PROGRAM"
echo "ok: $TOOL $VERSION ($PLATFORM): $(basename "$URL") is the pinned $SHA256; $DEST/bin/$PROGRAM"
