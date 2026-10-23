#!/usr/bin/env bash
# THIRD_PARTY_NOTICES of the desktop app: 1. other material
# (scripts/notices/attributions.txt), 2. every Rust crate compiled into the
# app and into the server it carries (cargo-about with about.toml's allow-list,
# for the macOS targets), 3. every npm package bundled into the server's web
# app (scripts/notices/web-notices.mjs). Fails on a licence that is unknown or
# not on the allow-list. desktop/build.sh runs it; .github/workflows/release.yml
# runs it in a job of its own, beside the builds, and hands the file over.
#
#   desktop/third-party-notices.sh <src-dir> <out-file> <app version> <server version> <rev>
#
# <src-dir> is a source tree of the commit with
# `pnpm install --frozen-lockfile --filter "@remote-hand/web..."` done. Needs
# cargo-about ABOUT_VERSION (scripts/release-public.sh's) on PATH, and
# DROPPEN_DISTRIBUTION unset.
set -euo pipefail

SRC=${1:?usage: third-party-notices.sh <src-dir> <out-file> <app version> <server version> <rev>}
OUT=${2:?out-file}
VERSION=${3:?app version}
SERVER_VERSION=${4:?server version}
REV=${5:?rev}
REPO=$(cd "$(dirname "$0")/.." && pwd -P)
die() { printf 'desktop/third-party-notices: %s\n' "$*" >&2; exit 1; }

ABOUT_VERSION=$(sed -n 's/^ABOUT_VERSION=//p' "$REPO/scripts/release-public.sh")
[ "$(cargo about --version 2>/dev/null)" = "cargo-about $ABOUT_VERSION" ] \
  || die "needs cargo-about $ABOUT_VERSION: cargo install --locked cargo-about@$ABOUT_VERSION --features cli"
[ -z "${DROPPEN_DISTRIBUTION:-}" ] || die "DROPPEN_DISTRIBUTION is set: these notices are the public build's"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/desktop-notices.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
# about.toml's allow-list, for the macOS targets this app is built for: its
# `targets` replaced, on one line or over several, and the rest kept.
awk '/^[[:space:]]*targets[[:space:]]*=/ {
       print "targets = [\"aarch64-apple-darwin\", \"x86_64-apple-darwin\"]"
       if ($0 !~ /\]/) skip = 1
       next
     }
     skip { if ($0 ~ /\]/) skip = 0; next }
     { print }' "$REPO/about.toml" > "$WORK/about-macos.toml"
[ "$(grep -c '^[[:space:]]*targets[[:space:]]*=' "$WORK/about-macos.toml")" = 1 ] \
  && grep -qx 'targets = \["aarch64-apple-darwin", "x86_64-apple-darwin"\]' "$WORK/about-macos.toml" \
  || die "cannot set the macOS targets in a copy of about.toml"
for kept in '^accepted[[:space:]]*=' '^ignore-build-dependencies[[:space:]]*=[[:space:]]*true' '^ignore-dev-dependencies[[:space:]]*=[[:space:]]*true' '^private[[:space:]]*=[[:space:]]*\{[[:space:]]*ignore[[:space:]]*=[[:space:]]*true[[:space:]]*\}'; do
  grep -qE "$kept" "$WORK/about-macos.toml" || die "the copy of about.toml lost a setting it must keep ($kept)"
done
for manifest in server-rs desktop; do
  cargo fetch --locked --manifest-path "$SRC/$manifest/Cargo.toml" >/dev/null
  cargo about generate --frozen --fail -c "$WORK/about-macos.toml" \
    -m "$SRC/$manifest/Cargo.toml" --format json -o "$WORK/about-$manifest.json" \
    || die "cargo-about refused a Rust licence of $manifest (see above; the allow-list is about.toml's accepted)"
done
node "$REPO/scripts/notices/rust-notices.mjs" "$WORK/about-desktop.json,$WORK/about-server-rs.json" "$WORK/notices-rust.txt" \
  "the app and its bundled server"
node "$REPO/scripts/notices/web-notices.mjs" "$SRC/web" "$WORK/notices-web.txt" \
  || die "an npm package bundled into the web app fails the licence check (see above)"
rule=$(printf '=%.0s' $(seq 1 80))
{
  printf 'THIRD_PARTY_NOTICES for Droppen %s for macOS (with remote-hand-server %s+g%s)\n\n' "$VERSION" "$SERVER_VERSION" "$REV"
  printf 'Droppen is licensed under the GNU Affero General Public License, version 3\n'
  printf 'only (AGPL-3.0-only); see LICENSE.\nCopyright (C) 2026 Kilerd\n\n'
  printf 'The app, and the server it carries, also contain the third-party software\n'
  printf 'below, used under the terms given with each: 1. other material, 2. Rust\n'
  printf 'crates compiled in (cargo-about %s, macOS), 3. npm packages bundled into the\n' "$ABOUT_VERSION"
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
