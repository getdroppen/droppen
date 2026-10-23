#!/usr/bin/env bash
# THIRD_PARTY_NOTICES of the server's release files: 1. other material
# (scripts/notices/attributions.txt), 2. every Rust crate compiled into the
# server (cargo-about with about.toml: the macOS and Linux targets), 3. every
# npm package bundled into its embedded web app (scripts/notices/web-notices.mjs).
# Fails on a licence that is unknown or not on the allow-list.
# scripts/release-public.sh runs it; .github/workflows/release.yml runs it in
# a job of its own, beside the server builds, and hands the file to stage.
#
#   scripts/third-party-notices.sh <src-dir> <out-file> <version> <server version> <rev>
#
# <src-dir> is a source tree of the release's commit with
# `pnpm install --frozen-lockfile --filter "@remote-hand/web..."` done
# (web-notices.mjs builds the web app once more, to see what it bundles).
# Needs cargo-about ABOUT_VERSION (scripts/release-public.sh's) on PATH;
# DROPPEN_DISTRIBUTION unset (the notices are the public build's).
set -euo pipefail

SRC=${1:?usage: third-party-notices.sh <src-dir> <out-file> <version> <server version> <rev>}
OUT=${2:?out-file}
VERSION=${3:?version}
CRATE_VERSION=${4:?server version}
REV=${5:?rev}
REPO=$(cd "$(dirname "$0")/.." && pwd -P)
die() { printf 'third-party-notices: %s\n' "$*" >&2; exit 1; }

ABOUT_VERSION=$(sed -n 's/^ABOUT_VERSION=//p' "$REPO/scripts/release-public.sh")
[ "$(cargo about --version 2>/dev/null)" = "cargo-about $ABOUT_VERSION" ] \
  || die "needs cargo-about $ABOUT_VERSION: cargo install --locked cargo-about@$ABOUT_VERSION --features cli"
[ -z "${DROPPEN_DISTRIBUTION:-}" ] || die "DROPPEN_DISTRIBUTION is set: these notices are the public build's"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/third-party-notices.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
# The targets come from about.toml: the macOS and Linux ones, so one file
# covers every binary of the release. Every target's crates are fetched
# first: cargo-about reads their licences from the sources (--frozen).
cargo fetch --locked --manifest-path "$SRC/server-rs/Cargo.toml" >/dev/null \
  || die "cannot fetch the server's crates for the licences"
cargo about generate --frozen --fail -c "$REPO/about.toml" \
  -m "$SRC/server-rs/Cargo.toml" --format json -o "$WORK/about.json" \
  || die "cargo-about refused a Rust licence (see above; the allow-list is about.toml's accepted)"
node "$REPO/scripts/notices/rust-notices.mjs" "$WORK/about.json" "$WORK/notices-rust.txt"
node "$REPO/scripts/notices/web-notices.mjs" "$SRC/web" "$WORK/notices-web.txt" \
  || die "an npm package bundled into the web app fails the licence check (see above)"
rule=$(printf '=%.0s' $(seq 1 80))
{
  printf 'THIRD_PARTY_NOTICES for Droppen %s (remote-hand-server %s+g%s)\n\n' "$VERSION" "$CRATE_VERSION" "$REV"
  printf 'Droppen is licensed under the GNU Affero General Public License, version 3\n'
  printf 'only (AGPL-3.0-only); see LICENSE.\nCopyright (C) 2026 Kilerd\n\n'
  printf 'The server binary also contains the third-party software below, used under\n'
  printf 'the terms given with each: 1. other material, 2. Rust crates compiled in\n'
  printf '(cargo-about %s, macOS and Linux), 3. npm packages bundled into the embedded web app.\n\n' "$ABOUT_VERSION"
  printf '%s\n1. ' "$rule"
  cat "$REPO/scripts/notices/attributions.txt"
  printf '\n%s\n2. ' "$rule"
  cat "$WORK/notices-rust.txt"
  printf '\n%s\n3. ' "$rule"
  cat "$WORK/notices-web.txt"
} > "$OUT"
if LC_ALL=C grep -qF "$HOME/" "$OUT"; then die "$OUT names paths under $HOME"; fi
echo "ok: $OUT, $(wc -l < "$OUT" | tr -d ' ') lines"
