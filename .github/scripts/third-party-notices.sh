#!/usr/bin/env bash
# THIRD_PARTY_NOTICES for the server tarball release.yml builds, written the way
# scripts/release-public.sh writes it for getdroppen/releases (keep the two in
# step): 1. other material (scripts/notices/attributions.txt), 2. every Rust
# crate compiled into the server (cargo-about with about.toml: the macOS and
# Linux targets), 3. every npm package bundled into its embedded web app
# (scripts/notices/web-notices.mjs). Fails on a licence that is unknown or not
# on the allow-list.
#
#   .github/scripts/third-party-notices.sh <out-file> <title line>
#
# Run from the repository root after `pnpm install`, with cargo-about
# $ABOUT_VERSION (0.9.2) on PATH. DROPPEN_DISTRIBUTION must be unset: the
# notices are the public build's.
set -euo pipefail

out=${1:?usage: third-party-notices.sh <out-file> <title line>}
title=${2:?usage: third-party-notices.sh <out-file> <title line>}
ABOUT_VERSION=${ABOUT_VERSION:-0.9.2}

[ -z "${DROPPEN_DISTRIBUTION:-}" ] || { echo "DROPPEN_DISTRIBUTION is set: these notices are for the public build" >&2; exit 1; }
have=$(cargo about --version 2>/dev/null || true)
case "$have" in
  *" $ABOUT_VERSION") ;;
  *) echo "needs cargo-about $ABOUT_VERSION (cargo install --locked cargo-about@$ABOUT_VERSION --features cli), found: ${have:-none}" >&2; exit 1 ;;
esac
grep -q "GNU AFFERO GENERAL PUBLIC LICENSE" LICENSE || { echo "LICENSE is not the AGPL-3.0 text" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# Every target's crates, not only this runner's: about.toml lists Linux too.
cargo fetch --locked --manifest-path server-rs/Cargo.toml
cargo about generate --frozen --fail -c about.toml -m server-rs/Cargo.toml --format json -o "$work/about.json"
node scripts/notices/rust-notices.mjs "$work/about.json" "$work/notices-rust.txt"
node scripts/notices/web-notices.mjs web "$work/notices-web.txt"

rule=$(printf '=%.0s' $(seq 1 80))
{
  printf '%s\n\n' "$title"
  printf 'Droppen is licensed under the GNU Affero General Public License, version 3\n'
  printf 'only (AGPL-3.0-only); see LICENSE.\nCopyright (C) 2026 Kilerd\n\n'
  printf 'The server binary also contains the third-party software below, used under\n'
  printf 'the terms given with each: 1. other material, 2. Rust crates compiled in\n'
  printf '(cargo-about %s, macOS and Linux), 3. npm packages bundled into the embedded web app.\n\n' "$ABOUT_VERSION"
  printf '%s\n1. ' "$rule"
  cat scripts/notices/attributions.txt
  printf '\n%s\n2. ' "$rule"
  cat "$work/notices-rust.txt"
  printf '\n%s\n3. ' "$rule"
  cat "$work/notices-web.txt"
} > "$out"
if LC_ALL=C grep -qF "$PWD/" "$out"; then echo "$out names paths under $PWD" >&2; exit 1; fi
echo "ok: $out, $(wc -l < "$out" | tr -d ' ') lines"
