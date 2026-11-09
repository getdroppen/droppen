#!/usr/bin/env bash
# THIRD_PARTY_NOTICES of the desktop app: 1. other material
# (scripts/notices/attributions.txt), 2. every Rust crate compiled into the
# app and into the server it carries (cargo-about with about.toml's allow-list,
# for the platform's targets), 3. every npm package bundled into the server's
# web app (scripts/notices/web-notices.mjs). Fails on a licence that is
# unknown or not on the allow-list. desktop/build.sh runs it;
# .github/workflows/release.yml runs it in a job of its own, beside the
# builds, and hands the file over.
#
#   desktop/third-party-notices.sh [--platform macos|linux] <src-dir> <out-file> <app version> <server version> <rev>
#
# --platform  whose app: macOS (the default; aarch64-apple-darwin and
#             x86_64-apple-darwin) or Linux (x86_64-unknown-linux-gnu and
#             aarch64-unknown-linux-gnu, whose app links GTK, WebKitGTK and
#             D-Bus through crates the Mac's does not have, and carries the
#             Linux server). The platform is named on the first line.
#
# <src-dir> is a source tree of the commit with
# `pnpm install --frozen-lockfile --filter "@droppen/web..."` done. Needs
# cargo-about ABOUT_VERSION (scripts/release-public.sh's) on PATH.
set -euo pipefail

PLATFORM=macos
if [ "${1:-}" = --platform ]; then
  PLATFORM=${2:?--platform needs macos or linux}
  shift 2
fi
SRC=${1:?usage: third-party-notices.sh [--platform macos|linux] <src-dir> <out-file> <app version> <server version> <rev>}
OUT=${2:?out-file}
VERSION=${3:?app version}
SERVER_VERSION=${4:?server version}
REV=${5:?rev}
REPO=$(cd "$(dirname "$0")/.." && pwd -P)
die() { printf 'desktop/third-party-notices: %s\n' "$*" >&2; exit 1; }
case "$PLATFORM" in
  macos) NAME=macOS; TARGETS='"aarch64-apple-darwin", "x86_64-apple-darwin"' ;;
  linux) NAME=Linux; TARGETS='"x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"' ;;
  *) die "--platform is macos or linux, not $PLATFORM" ;;
esac

ABOUT_VERSION=$(sed -n 's/^ABOUT_VERSION=//p' "$REPO/scripts/release-public.sh")
[ "$(cargo about --version 2>/dev/null)" = "cargo-about $ABOUT_VERSION" ] \
  || die "needs cargo-about $ABOUT_VERSION: cargo install --locked cargo-about@$ABOUT_VERSION --features cli"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/desktop-notices.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
# about.toml's allow-list, for the targets this app is built for on the
# platform: its `targets` replaced, on one line or over several, and the rest
# kept.
CONFIG=$WORK/about-$PLATFORM.toml
awk -v targets="targets = [$TARGETS]" '/^[[:space:]]*targets[[:space:]]*=/ {
       print targets
       if ($0 !~ /\]/) skip = 1
       next
     }
     skip { if ($0 ~ /\]/) skip = 0; next }
     { print }' "$REPO/about.toml" > "$CONFIG"
[ "$(grep -c '^[[:space:]]*targets[[:space:]]*=' "$CONFIG")" = 1 ] \
  && grep -qxF "targets = [$TARGETS]" "$CONFIG" \
  || die "cannot set the $NAME targets in a copy of about.toml"
for kept in '^accepted[[:space:]]*=' '^ignore-build-dependencies[[:space:]]*=[[:space:]]*true' '^ignore-dev-dependencies[[:space:]]*=[[:space:]]*true' '^private[[:space:]]*=[[:space:]]*\{[[:space:]]*ignore[[:space:]]*=[[:space:]]*true[[:space:]]*\}'; do
  grep -qE "$kept" "$CONFIG" || die "the copy of about.toml lost a setting it must keep ($kept)"
done
for manifest in . desktop; do
  case "$manifest" in
    .) label=server ;;
    *) label=$manifest ;;
  esac
  cargo fetch --locked --manifest-path "$SRC/$manifest/Cargo.toml" >/dev/null
  cargo about generate --frozen --fail -c "$CONFIG" \
    -m "$SRC/$manifest/Cargo.toml" --format json -o "$WORK/about-$label.json" \
    || die "cargo-about refused a Rust licence of $label (see above; the allow-list is about.toml's accepted)"
done
node "$REPO/scripts/notices/rust-notices.mjs" "$WORK/about-desktop.json,$WORK/about-server.json" "$WORK/notices-rust.txt" \
  "the app and its bundled server"
node "$REPO/scripts/notices/web-notices.mjs" "$SRC/web" "$WORK/notices-web.txt" \
  || die "an npm package bundled into the web app fails the licence check (see above)"
rule=$(printf '=%.0s' $(seq 1 80))
{
  printf 'THIRD_PARTY_NOTICES for Droppen %s for %s (with droppen-server %s+g%s)\n\n' "$VERSION" "$NAME" "$SERVER_VERSION" "$REV"
  printf 'Droppen is licensed under the GNU Affero General Public License, version 3\n'
  printf 'only (AGPL-3.0-only); see LICENSE.\nCopyright (C) 2026 Kilerd\n\n'
  printf 'The app, and the server it carries, also contain the third-party software\n'
  printf 'below, used under the terms given with each: 1. other material, 2. Rust\n'
  printf 'crates compiled in (cargo-about %s, %s), 3. npm packages bundled into the\n' "$ABOUT_VERSION" "$NAME"
  printf "server's web app.\n\n"
  printf '%s\n1. ' "$rule"
  cat "$REPO/scripts/notices/attributions.txt"
  printf '\n%s\n2. ' "$rule"
  cat "$WORK/notices-rust.txt"
  printf '\n%s\n3. ' "$rule"
  cat "$WORK/notices-web.txt"
} > "$OUT"
if LC_ALL=C grep -qF "$HOME/" "$OUT"; then die "$OUT names paths under $HOME"; fi
echo "ok: $OUT, $(wc -l < "$OUT" | tr -d ' ') lines"
