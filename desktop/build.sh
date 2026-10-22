#!/usr/bin/env bash
# Build the Droppen desktop app for macOS: Droppen.app and Droppen-<version>.dmg,
# with the public-distribution server bundled inside as its sidecar. Unsigned
# (ad-hoc sealed only) — desktop/SIGNING.md has the Developer ID signing and
# notarisation steps. Nothing is published: the last thing this prints is the
# upload command for a human to review and run.
#
#   desktop/build.sh [--ref <git-ref> | --worktree] [--arch universal|aarch64|x86_64]
#                    [--out <dir>] [--sidecar <prebuilt remote-hand-server>]
#                    [--e2e [--version <v>]] [--keep-work]
#
#   --ref       what to build (default HEAD), exported with `git archive` so
#               uncommitted changes never reach a release.
#   --worktree  build this checkout as it is, uncommitted files included (for
#               trying a change; never for a release).
#   --arch      universal (default: the one the download page links), aarch64
#               or x86_64. A single-architecture build is named
#               Droppen-<version>-<arch>.dmg so it cannot be mistaken for it.
#   --out       where the .app and .dmg go (default /tmp/droppen-desktop-release);
#               must be empty or absent.
#   --sidecar   use this server binary instead of building one (it must contain
#               the architectures --arch asks for; e.g. the universal binary
#               scripts/release-public.sh produced for the same version).
#   --e2e       an end-to-end test build (feature `e2e`: the test-control hook,
#               the web view probe, DROPPEN_DESKTOP_KEYCHAIN=0, the updater's
#               test endpoint and key, links logged instead of opened; and the
#               updater may fetch over http) with an identity of its own, so it
#               runs beside an installed Droppen without touching it
#               (src-tauri/tauri.e2e.conf.json: bundle id app.droppen.desktop.e2e,
#               "Droppen E2E.app"; core/src/identity.rs) — only the app, no DMG,
#               never uploaded. A release build is checked to contain none of it.
#   --version   with --e2e only: the app's version instead of the server's,
#               e.g. 0.0.1-test (the updater's end-to-end test needs two). A
#               release is always the server's version, which the updater
#               compares with the newest release's.
#   --keep-work keep the build directory.
#
# The server is built exactly as scripts/release-public.sh builds the public
# release (the web app as the public distribution, the builder's paths
# remapped out, the same binary checks), then placed where Tauri wants a
# sidecar: src-tauri/binaries/remote-hand-server-<target-triple>.
#
# Every build but an --e2e one ships the app's licence and third-party
# notices, in Droppen.app/Contents/Resources and beside the app in the DMG:
#   LICENSE              the AGPL-3.0 text (the repository's LICENSE)
#   THIRD_PARTY_NOTICES  every Rust crate compiled into the app and into its
#                        bundled server (cargo-about, scripts/notices/
#                        rust-notices.mjs, macOS targets), every npm package
#                        bundled into the server's web app (scripts/notices/
#                        web-notices.mjs) and the other material of
#                        scripts/notices/attributions.txt — the generators and
#                        allow-lists scripts/release-public.sh uses
# Both pass the release text check (scripts/release-text-check.sh) before
# they go in, and the build refuses to finish without them.
#
# Needs: Node as in .nvmrc, rustup, and for every build but --e2e,
# cargo-about 0.9.2 (`cargo install --locked cargo-about@0.9.2 --features cli`).
#
# Environment:
#   DROPPEN_MIN_FREE_GB      refuse to start, and stop a step, below this much
#                            free disk (25)
#   DROPPEN_DESKTOP_WORK     the scratch directory (/tmp/droppen-desktop-build/work)
#   DROPPEN_TOOLS_DIR        where the pinned tauri-cli is installed
#                            (~/.cache/droppen-desktop/tools)
#   DROPPEN_BUILD_JOBS       cargo -j (default: half the cores)
#   DROPPEN_ALLOW_NODE_MISMATCH=1  build with a Node other than .nvmrc's
#   DROPPEN_RELEASE_REPO     where the DMG is published (getdroppen/droppen)
#   APPLE_SIGNING_IDENTITY   sign with this identity instead of ad hoc (see
#                            SIGNING.md); APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID
#                            then also notarise, through Tauri
#   DROPPEN_RELEASE_FORBIDDEN, DROPPEN_RELEASE_NOTICE_ALLOW
#                            the release text check's names and allowed notice
#                            lines (scripts/release-public.sh's Environment);
#                            when DROPPEN_RELEASE_FORBIDDEN is unset, read (as
#                            data, never run) from
#                            ~/.config/droppen/release-forbidden.env. Required,
#                            and checked against its hash, for every build but
#                            --e2e; the allowed lines must each be one whole
#                            copyright line written out (^Copyright ...$).
#                            DROPPEN_RELEASE_NAMES_DEFERRED=1 (release.yml's
#                            build job, which is not given the names): check
#                            "Kilerd" only, and leave the names to notarise.sh
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }

REF=""
WORKTREE=0
ARCH=universal
OUT=/tmp/droppen-desktop-release
SIDECAR=""
KEEP_WORK=0
E2E=0
TEST_VERSION=""
while [ $# -gt 0 ]; do
  case "$1" in
    --ref) REF=${2:?--ref needs a value}; shift 2 ;;
    --worktree) WORKTREE=1; shift ;;
    --arch) ARCH=${2:?--arch needs a value}; shift 2 ;;
    --out) OUT=${2:?--out needs a value}; shift 2 ;;
    --sidecar) SIDECAR=${2:?--sidecar needs a path}; shift 2 ;;
    --keep-work) KEEP_WORK=1; shift ;;
    --e2e) E2E=1; shift ;;
    --version) TEST_VERSION=${2:?--version needs a value}; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown argument: $1" >&2; usage ;;
  esac
done
[ "$WORKTREE" = 0 ] || [ -z "$REF" ] || { echo "--ref and --worktree are exclusive" >&2; exit 2; }
[ -z "$TEST_VERSION" ] || [ "$E2E" = 1 ] || { echo "--version is for --e2e test builds only; a release is the server's version" >&2; exit 2; }

TAURI_CLI_VERSION=2.12.3
ABOUT_VERSION=0.9.2
BIN_NAME=remote-hand-server
PUBLIC_REPO=${DROPPEN_RELEASE_REPO:-getdroppen/droppen}
MIN_FREE_GB=${DROPPEN_MIN_FREE_GB:-25}
WORK=${DROPPEN_DESKTOP_WORK:-/tmp/droppen-desktop-build/work}
TOOLS=${DROPPEN_TOOLS_DIR:-$HOME/.cache/droppen-desktop/tools}
JOBS=${DROPPEN_BUILD_JOBS:-$(( $(sysctl -n hw.ncpu) / 2 ))}
[ "$JOBS" -ge 1 ] || JOBS=1

case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin); TAURI_TARGET=universal-apple-darwin; DMG_ARCH=universal ;;
  aarch64) TARGETS=(aarch64-apple-darwin); TAURI_TARGET=aarch64-apple-darwin; DMG_ARCH=aarch64 ;;
  x86_64) TARGETS=(x86_64-apple-darwin); TAURI_TARGET=x86_64-apple-darwin; DMG_ARCH=x64 ;;
  *) echo "--arch is universal, aarch64 or x86_64, not $ARCH" >&2; exit 2 ;;
esac

say() { printf '\n== %s\n' "$*"; }
die() { printf 'desktop/build.sh: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "this builds a macOS app and needs a Mac"
for tool in git cargo rustup lipo shasum hdiutil codesign ditto; do
  command -v "$tool" >/dev/null || die "missing $tool"
done
# The licence and notices (every build but --e2e) need Node and cargo-about,
# even with a prebuilt server: its web app's notices are made from the source.
if [ -z "$SIDECAR" ] || [ "$E2E" = 0 ]; then
  for tool in node npx; do command -v "$tool" >/dev/null || die "missing $tool"; done
fi
if [ "$E2E" = 0 ]; then
  [ "$(cargo about --version 2>/dev/null)" = "cargo-about $ABOUT_VERSION" ] \
    || die "needs cargo-about $ABOUT_VERSION for THIRD_PARTY_NOTICES: cargo install --locked cargo-about@$ABOUT_VERSION --features cli"
fi
# Signing during the build: the identity must be in the keychain, which is
# better found out now than after the build.
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ "$APPLE_SIGNING_IDENTITY" != - ]; then
  security find-identity -v -p codesigning | grep -qF -e "\"$APPLE_SIGNING_IDENTITY\"" -e " $APPLE_SIGNING_IDENTITY " \
    || die "APPLE_SIGNING_IDENTITY is \"$APPLE_SIGNING_IDENTITY\", which is not a valid signing identity in the keychain (security find-identity -v -p codesigning). desktop/notarise.sh --check says what is missing; desktop/SIGNING.md, \"What the release Mac needs\", how to restore it"
fi

REPO=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
if [ "$E2E" = 0 ]; then
  # The release text check, over LICENSE and THIRD_PARTY_NOTICES before they
  # go into the app: without the operator's names it would pass what it
  # exists to stop, so nothing is built.
  . "$REPO/scripts/release-text-check.sh"
  # The operator's file is read as data (the two variables, nothing else),
  # never run: run, it could redefine the check or its hash.
  NAMES_FILE=$HOME/.config/droppen/release-forbidden.env
  release_text_load_names "$NAMES_FILE"
  if release_text_names_deferred; then
    echo "note: DROPPEN_RELEASE_NAMES_DEFERRED: LICENSE and THIRD_PARTY_NOTICES are checked for \"Kilerd\" only here; the names are checked later, by whoever has them (desktop/notarise.sh does)"
  else
    release_text_names_ok \
      || die "DROPPEN_RELEASE_FORBIDDEN is not the expected list of names (set it, or keep it in $NAMES_FILE; its sha256 is RELEASE_TEXT_NAMES_SHA256 in scripts/release-text-check.sh)"
  fi
  release_text_allow_ok \
    || die "DROPPEN_RELEASE_NOTICE_ALLOW may only spell out whole third-party copyright lines, each ^Copyright …\$ with its metacharacters escaped"
fi
if [ "$WORKTREE" = 1 ]; then
  SHA=$(git -C "$REPO" rev-parse HEAD)
  DIRTY=$(git -C "$REPO" status --porcelain | wc -l | tr -d ' ')
else
  REF=${REF:-HEAD}
  SHA=$(git -C "$REPO" rev-parse --verify --quiet "$REF^{commit}") || die "cannot resolve $REF"
  DIRTY=0
fi
REV=$(git -C "$REPO" rev-parse --short "$SHA")
COMMIT_TIME=$(git -C "$REPO" log -1 --format=%ct "$SHA")
if [ "$WORKTREE" = 1 ]; then
  VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/server-rs/Cargo.toml" | head -1)
else
  VERSION=$(git -C "$REPO" show "$SHA:server-rs/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)
fi
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "cannot read the server version (got '$VERSION')"
SERVER_VERSION=$VERSION
# The app's version is what the updater compares with the newest release's
# (`latest.json`), so a release tag must name it exactly.
if [[ "${REF:-}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] && [ "$REF" != "v$VERSION" ]; then
  die "$REF has the server version $VERSION in server-rs/Cargo.toml; the app would call itself $VERSION"
fi
if [ -n "$TEST_VERSION" ]; then
  [[ "$TEST_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "--version $TEST_VERSION is not a semantic version"
  VERSION=$TEST_VERSION
fi
if [ "$ARCH" = universal ]; then DMG_NAME="Droppen-${VERSION}.dmg"; else DMG_NAME="Droppen-${VERSION}-${ARCH}.dmg"; fi

case "$WORK" in /tmp/*|/private/tmp/*|"${TMPDIR:-/nonexistent}"*) ;; *) die "DROPPEN_DESKTOP_WORK must be under /tmp" ;; esac
case "$WORK$OUT" in *" "*) die "no spaces in the work or output path, please" ;; esac
if [ -e "$OUT" ] && [ -n "$(ls -A "$OUT")" ]; then
  die "$OUT is not empty; move it aside or pass --out <new dir>"
fi

free_gb() { df -Pk "$1" | awk 'NR == 2 { print int($4 / 1048576) }'; }
mkdir -p "$(dirname "$WORK")"
FREE=$(free_gb "$(dirname "$WORK")")
# Two release targets of the server plus two of the app, and node_modules:
# roughly 8 GB at the peak. Refuse unless that still leaves the floor intact.
[ "$FREE" -ge $((MIN_FREE_GB + 8)) ] \
  || die "only ${FREE} GB free; need at least $((MIN_FREE_GB + 8)) GB to stay above ${MIN_FREE_GB} GB"

if [ -z "$SIDECAR" ]; then
  NVMRC=$( (git -C "$REPO" show "$SHA:.nvmrc" 2>/dev/null || cat "$REPO/.nvmrc") | tr -d '[:space:]')
  NODE_VERSION=$(node --version | sed 's/^v//')
  if [ "$NODE_VERSION" != "$NVMRC" ] && [ "${DROPPEN_ALLOW_NODE_MISMATCH:-}" != 1 ]; then
    die "node is $NODE_VERSION but .nvmrc says $NVMRC: put Node $NVMRC first on PATH, or set DROPPEN_ALLOW_NODE_MISMATCH=1"
  fi
fi

cleanup() {
  local status=$?
  if [ -n "${WATCHED:-}" ] && kill -0 "$WATCHED" 2>/dev/null; then kill -TERM -- "-$WATCHED" 2>/dev/null || kill -TERM "$WATCHED" 2>/dev/null || true; fi
  if [ "$KEEP_WORK" = 1 ]; then
    echo "kept the build directory: $WORK" >&2
  else
    rm -rf "$WORK"
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# Run a long step in its own process group, niced, and stop that group (by
# its id) if the volume falls under the floor: other builds share this disk.
set -m
run_watched() {
  nice -n 10 "$@" &
  WATCHED=$!
  while kill -0 "$WATCHED" 2>/dev/null; do
    if [ "$(free_gb "$WORK")" -lt "$MIN_FREE_GB" ]; then
      echo "free space fell below ${MIN_FREE_GB} GB; stopping this step (process group $WATCHED)" >&2
      kill -TERM -- "-$WATCHED" 2>/dev/null || kill -TERM "$WATCHED" 2>/dev/null || true
      wait "$WATCHED" 2>/dev/null || true
      WATCHED=
      return 1
    fi
    sleep 5
  done
  local status=0
  wait "$WATCHED" || status=$?
  WATCHED=
  return "$status"
}

NOTE=""
[ "$DIRTY" = 0 ] || NOTE=", plus ${DIRTY} uncommitted changes"
say "Droppen ${VERSION} (${ARCH}) from ${SHA} (rev ${REV}${NOTE}), ${FREE} GB free, work in ${WORK}"
rm -rf "$WORK"
mkdir -p "$WORK/src" "$OUT"
if [ "$WORKTREE" = 1 ]; then
  # Tracked and untracked files, never ignored ones (no target/, no node_modules).
  # (A tracked file deleted in the checkout is skipped.)
  (
    cd "$REPO"
    git ls-files -z --cached --others --exclude-standard | while IFS= read -r -d '' file; do
      [ -e "$file" ] && printf '%s\0' "$file"
    done | tar --null -T - -cf -
  ) | tar -x -C "$WORK/src"
else
  git -C "$REPO" archive --format=tar "$SHA" | tar -x -C "$WORK/src"
fi
SRC=$WORK/src
[ -d "$SRC/desktop/src-tauri" ] || die "$REF has no desktop/ app to build"

say "rust targets"
installed=$(rustup target list --installed)
for target in "${TARGETS[@]}"; do
  grep -qx "$target" <<<"$installed" || rustup target add "$target"
done

CARGO_HOME_DIR=${CARGO_HOME:-$HOME/.cargo}
REMAP="--remap-path-prefix=$SRC=/droppen --remap-path-prefix=$CARGO_HOME_DIR=/cargo"
SIDECAR_OUT="$WORK/sidecar/$BIN_NAME"
mkdir -p "$WORK/sidecar"

# The usual cause: the release profile's strip (rust-objcopy) failed, which
# leaves the linker's debug map, with the toolchain's and the registry's paths.
# rust-objcopy finds its LLVM library only when cargo runs through rustup's
# proxy, which sets the library path: a toolchain's own bin/ first on PATH
# skips it.
HOME_HINT=" (if the build said \"stripping debug info with rust-objcopy failed\", put rustup's cargo proxy first on PATH — Homebrew's is in /opt/homebrew/opt/rustup/bin — not a toolchain's bin/)"
check_binary() { # path — the same checks as scripts/release-public.sh
  local bin=$1 markers
  markers=$(LC_ALL=C grep -ao '"distribution": "[^"]*"' "$bin" | sort -u)
  [ "$markers" = '"distribution": "public"' ] || die "$bin: distribution markers are [$markers], not exactly \"public\""
  for needle in '"id": "tasks"' 'plugin-tasks' '.droppen-plugins/'; do
    if LC_ALL=C grep -aqF "$needle" "$bin"; then die "$bin: contains $needle (a bundled plugin)"; fi
  done
  if LC_ALL=C grep -aqF "$HOME/" "$bin"; then die "$bin: still names paths under $HOME$HOME_HINT"; fi
  echo "ok: $(lipo -archs "$bin") $bin: distribution public, no plugins, no \$HOME paths"
}

if [ -n "$SIDECAR" ]; then
  say "server: using $SIDECAR"
  [ -x "$SIDECAR" ] || die "$SIDECAR is not an executable"
  archs=$(lipo -archs "$SIDECAR")
  for target in "${TARGETS[@]}"; do
    case "$target" in
      aarch64-*) want=arm64 ;;
      x86_64-*) want=x86_64 ;;
    esac
    case " $archs " in *" $want "*) ;; *) die "$SIDECAR has [$archs], not $want" ;; esac
  done
  cp "$SIDECAR" "$SIDECAR_OUT"
  check_binary "$SIDECAR_OUT"
  # The server of this very ref: `--version` prints `<version>+g<rev>` bare,
  # as release-public.sh builds it. Another one would ship under this app's
  # version, and the notices would not be its own. The revision is compared
  # as a commit, not as text: git's short form grows with the repository.
  SIDE_VERSION=$("$SIDECAR_OUT" --version 2>/dev/null) || die "cannot run $SIDECAR --version to check it is the server of $SHA"
  SIDE_REV=${SIDE_VERSION##*+g}
  if [ "$SIDE_REV" = "$SIDE_VERSION" ] || [ "${SIDE_VERSION%+g*}" != "$SERVER_VERSION" ] \
    || ! [[ "$SIDE_REV" =~ ^[0-9a-f]{7,40}$ ]] \
    || [ "$(git -C "$REPO" rev-parse --verify --quiet "$SIDE_REV^{commit}" 2>/dev/null)" != "$SHA" ]; then
    die "$SIDECAR says it is \"$SIDE_VERSION\", not $SERVER_VERSION+g<a revision of $SHA>: it is not the server of this ref"
  fi
  echo "ok: the sidecar is $SIDE_VERSION, this ref's server"
fi
if [ -z "$SIDECAR" ] || [ "$E2E" = 0 ]; then
  say "web app: the public distribution"
  PNPM_VERSION=$(sed -n 's/.*"packageManager": *"pnpm@\([^"]*\)".*/\1/p' "$SRC/package.json")
  mkdir -p "$WORK/shims"
  if ! command -v pnpm >/dev/null || [ "$(pnpm --version 2>/dev/null)" != "$PNPM_VERSION" ]; then
    printf '#!/bin/sh\nexec npx -y pnpm@%s "$@"\n' "$PNPM_VERSION" > "$WORK/shims/pnpm"
    chmod +x "$WORK/shims/pnpm"
  fi
  # No DROPPEN_DISTRIBUTION: the build then reads `distribution.json`, the public one.
  web_env=(env -u DROPPEN_DISTRIBUTION PATH="$WORK/shims:$PATH" SOURCE_DATE_EPOCH="$COMMIT_TIME")
  run_watched "${web_env[@]}" pnpm -C "$SRC" install --frozen-lockfile --filter "@remote-hand/web..."
fi
if [ "$E2E" = 0 ]; then
  say "licence and third-party notices"
  # The licence and the notices tooling from this checkout, the dependency
  # graphs and node_modules from the ref — as scripts/release-public.sh does.
  LEGAL=$SRC/desktop/src-tauri/legal
  mkdir -p "$LEGAL"
  LICENSE_FILE=$SRC/LICENSE
  [ -f "$LICENSE_FILE" ] || LICENSE_FILE=$REPO/LICENSE
  grep -q "GNU AFFERO GENERAL PUBLIC LICENSE" "$LICENSE_FILE" || die "$LICENSE_FILE is not the AGPL-3.0 text"
  cp "$LICENSE_FILE" "$LEGAL/LICENSE"
  # about.toml's allow-list, for the macOS targets this app is built for:
  # its `targets` replaced, on one line or over several, and the rest kept.
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
    run_watched cargo fetch --locked --manifest-path "$SRC/$manifest/Cargo.toml" >/dev/null
    cargo about generate --frozen --fail -c "$WORK/about-macos.toml" \
      -m "$SRC/$manifest/Cargo.toml" --format json -o "$WORK/about-$manifest.json" \
      || die "cargo-about refused a Rust licence of $manifest (see above; the allow-list is about.toml's accepted)"
  done
  node "$REPO/scripts/notices/rust-notices.mjs" "$WORK/about-desktop.json,$WORK/about-server-rs.json" "$WORK/notices-rust.txt" \
    "the app and its bundled server"
  run_watched "${web_env[@]}" node "$REPO/scripts/notices/web-notices.mjs" "$SRC/web" "$WORK/notices-web.txt" \
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
  } > "$LEGAL/THIRD_PARTY_NOTICES"
  if LC_ALL=C grep -qF "$HOME/" "$LEGAL/THIRD_PARTY_NOTICES"; then die "THIRD_PARTY_NOTICES names paths under $HOME"; fi
  # The release text check (scripts/release-text-check.sh): no personal names
  # but the copyright holder's on the copyright line.
  for legal in LICENSE THIRD_PARTY_NOTICES; do
    hits=$(release_text_hits "$LEGAL/$legal" "$([ "$legal" = THIRD_PARTY_NOTICES ] && echo notices)")
    [ -z "$hits" ] || die "$legal names a person it should not:
$hits"
  done
  echo "ok: LICENSE (AGPL-3.0), THIRD_PARTY_NOTICES $(wc -l < "$LEGAL/THIRD_PARTY_NOTICES" | tr -d ' ') lines; both pass the release text check"
fi
if [ -z "$SIDECAR" ]; then
  run_watched "${web_env[@]}" pnpm -C "$SRC" --filter @remote-hand/web build
  node -e '
    const m = require(process.argv[1]);
    if (m.distribution !== undefined && m.distribution !== "public") { console.error("distribution is", m.distribution); process.exit(1); }
    if ((m.plugins || []).length) { console.error("the public build bundles plugins:", m.plugins.map((p) => p.id)); process.exit(1); }
  ' "$SRC/web/dist/plugins/manifest.json" || die "web/dist is not the public distribution"

  say "server: ${TARGETS[*]}"
  # The commit goes in under both names: REF may be from before the rename to
  # Droppen, whose build reads only RH_BUILD_REV.
  for target in "${TARGETS[@]}"; do
    run_watched env \
      CARGO_TARGET_DIR="$WORK/target-server" \
      CARGO_BUILD_BUILD_DIR="$WORK/build-server" \
      DROPPEN_BUILD_REV="$REV" RH_BUILD_REV="$REV" \
      RUSTFLAGS="$REMAP" \
      cargo build --release --locked -j "$JOBS" --target "$target" --manifest-path "$SRC/server-rs/Cargo.toml"
    check_binary "$WORK/target-server/$target/release/$BIN_NAME"
  done
  if [ "$ARCH" = universal ]; then
    lipo -create -output "$SIDECAR_OUT" \
      "$WORK/target-server/aarch64-apple-darwin/release/$BIN_NAME" \
      "$WORK/target-server/x86_64-apple-darwin/release/$BIN_NAME"
  else
    cp "$WORK/target-server/${TARGETS[0]}/release/$BIN_NAME" "$SIDECAR_OUT"
  fi
  # The server's intermediates are no longer needed; give the disk back
  # before the app's own build.
  rm -rf "$WORK/target-server" "$WORK/build-server"
fi
rm -rf "$SRC/node_modules" "$SRC/web/node_modules"
chmod +x "$SIDECAR_OUT"
if "$SIDECAR_OUT" --version >/dev/null 2>&1; then
  echo "sidecar: $("$SIDECAR_OUT" --version) [$(lipo -archs "$SIDECAR_OUT")]"
fi
# Tauri's sidecar naming: <name>-<target triple>. A universal build compiles
# the app once per architecture, and each of those builds wants its own
# triple's file, while the bundle takes the universal-apple-darwin one.
BINARIES="$SRC/desktop/src-tauri/binaries"
for target in "${TARGETS[@]}"; do
  case "$target" in aarch64-*) slice=arm64 ;; x86_64-*) slice=x86_64 ;; esac
  if [ "$(lipo -archs "$SIDECAR_OUT")" = "$slice" ]; then
    cp "$SIDECAR_OUT" "$BINARIES/$BIN_NAME-$target"
  else
    lipo -thin "$slice" -output "$BINARIES/$BIN_NAME-$target" "$SIDECAR_OUT"
  fi
done
[ "$ARCH" != universal ] || cp "$SIDECAR_OUT" "$BINARIES/$BIN_NAME-universal-apple-darwin"
chmod +x "$BINARIES/$BIN_NAME"-*

say "tauri-cli $TAURI_CLI_VERSION"
if [ "$("$TOOLS/bin/cargo-tauri" --version 2>/dev/null | awk '{ print $2 }')" != "$TAURI_CLI_VERSION" ]; then
  run_watched env CARGO_TARGET_DIR="$WORK/target-tauri-cli" CARGO_BUILD_BUILD_DIR="$WORK/build-tauri-cli" \
    cargo install tauri-cli --version "$TAURI_CLI_VERSION" --locked --root "$TOOLS" -j "$JOBS"
  rm -rf "$WORK/target-tauri-cli" "$WORK/build-tauri-cli"
fi

CONF="$SRC/desktop/src-tauri/tauri.conf.json"
if [ "$E2E" = 1 ]; then
  # Its own identity (bundle id, name), and the updater's end-to-end test
  # serves its manifest from a local http server: tauri.e2e.conf.json.
  E2E_CONF="$SRC/desktop/src-tauri/tauri.e2e.conf.json"
  BUILD_FLAVOUR=(--bundles app --features e2e --config "$E2E_CONF")
  CONF=$E2E_CONF
  CONFIG="{\"version\":\"$VERSION\"}"
else
  # The DMG is made below, with the licence and notices beside the app.
  BUILD_FLAVOUR=(--bundles app)
  CONFIG="{\"version\":\"$VERSION\",\"bundle\":{\"resources\":{\"legal/LICENSE\":\"LICENSE\",\"legal/THIRD_PARTY_NOTICES\":\"THIRD_PARTY_NOTICES\"}}}"
fi
conf_value() { sed -n "s/.*\"$1\": *\"\([^\"]*\)\".*/\1/p" "$CONF" | head -1; }
BUNDLE_ID=$(conf_value identifier)
PRODUCT=$(conf_value productName)
[ -n "$BUNDLE_ID" ] && [ -n "$PRODUCT" ] || die "cannot read the identifier and productName of $CONF"
# The bundle id the app is compiled with (core/src/identity.rs: RELEASE, or E2E
# with --e2e). The bundle must carry exactly it: the app refuses to start
# otherwise, and the updater would install such a release everywhere. Checked
# against the config now and against the built bundle below; src-tauri/build.rs
# checks the same while compiling.
identity_bundle_id() { sed -n "/^pub const $1: Identity = Identity {/,/^};/s/^ *bundle_id: \"\([^\"]*\)\",\$/\1/p" "$SRC/desktop/core/src/identity.rs"; }
if [ "$E2E" = 1 ]; then WANT_ID=$(identity_bundle_id E2E); else WANT_ID=$(identity_bundle_id RELEASE); fi
[ -n "$WANT_ID" ] || die "cannot read the bundle id from desktop/core/src/identity.rs"
[ "$BUNDLE_ID" = "$WANT_ID" ] || die "$CONF names the bundle id $BUNDLE_ID, but the app is compiled as $WANT_ID (desktop/core/src/identity.rs): it would refuse to start"
say "app: $TAURI_TARGET$([ "$E2E" = 1 ] && echo ' (e2e test build)')"
# CI=true keeps Tauri from scripting the Finder. Ad hoc signing ("-") seals the bundle consistently; it is not a Developer ID
# signature, so Gatekeeper still treats the app as unidentified.
(
  cd "$SRC/desktop/src-tauri"
  run_watched env \
    CI=true \
    APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}" \
    CARGO_TARGET_DIR="$WORK/target-desktop" \
    CARGO_BUILD_BUILD_DIR="$WORK/build-desktop" \
    CARGO_BUILD_JOBS="$JOBS" \
    RUSTFLAGS="$REMAP" \
    "$TOOLS/bin/cargo-tauri" build --ci --target "$TAURI_TARGET" "${BUILD_FLAVOUR[@]}" \
      --config "$CONFIG"
)

BUNDLE="$WORK/target-desktop/$TAURI_TARGET/release/bundle"
APP="$BUNDLE/macos/$PRODUCT.app"
[ -d "$APP" ] || die "no $PRODUCT.app in $BUNDLE/macos"

say "checks"
for bin in "$APP/Contents/MacOS/Droppen" "$APP/Contents/MacOS/$BIN_NAME"; do
  [ -x "$bin" ] || die "missing $bin"
  echo "$(basename "$bin"): $(lipo -archs "$bin")"
done
check_binary "$APP/Contents/MacOS/$BIN_NAME"
# The app's own binary: no builder paths either.
if LC_ALL=C grep -aqF "$HOME/" "$APP/Contents/MacOS/Droppen"; then die "Droppen: still names paths under $HOME$HOME_HINT"; fi
echo "ok: Droppen: no \$HOME paths"
# The test hooks are in an e2e build and in nothing else.
HOOKS=0
for needle in DROPPEN_DESKTOP_TEST_CONTROL DROPPEN_DESKTOP_KEYCHAIN 'test control is on' DROPPEN_DESKTOP_UPDATE_ENDPOINT DROPPEN_DESKTOP_TEST_HUB; do
  if LC_ALL=C grep -aqF "$needle" "$APP/Contents/MacOS/Droppen"; then HOOKS=$((HOOKS + 1)); fi
done
# So is the end-to-end build's own identity (core/src/identity.rs).
if LC_ALL=C grep -aqF app.droppen.desktop.e2e "$APP/Contents/MacOS/Droppen"; then E2E_IDENTITY=1; else E2E_IDENTITY=0; fi
if [ "$E2E" = 1 ]; then
  [ "$HOOKS" = 5 ] || die "an e2e build without its test hooks ($HOOKS/5 found)"
  [ "$E2E_IDENTITY" = 1 ] || die "an e2e build without its own identity (app.droppen.desktop.e2e)"
  echo "ok: e2e test build, test hooks and its own identity present"
else
  [ "$HOOKS" = 0 ] || die "a release build with test hooks in it ($HOOKS found)"
  [ "$E2E_IDENTITY" = 0 ] || die "a release build with the end-to-end build's identity in it"
  echo "ok: no test hooks (test control, keychain switch, updater test endpoint, test hub) and no end-to-end identity in the release build"
fi
# The updater trusts the key in tauri.conf.json; the binary must carry it.
PUBKEY=$(sed -n 's/.*"pubkey": *"\([^"]*\)".*/\1/p' "$SRC/desktop/src-tauri/tauri.conf.json")
{ [ -n "$PUBKEY" ] && LC_ALL=C grep -aqF "$PUBKEY" "$APP/Contents/MacOS/Droppen"; } || die "the app does not carry the updater public key of tauri.conf.json"
echo "ok: the updater public key is the one in tauri.conf.json (key $(printf '%s' "$PUBKEY" | base64 -d | sed -n 's/^untrusted comment: minisign public key: //p'))"
BUILT_ID=$(plutil -extract CFBundleIdentifier raw "$APP/Contents/Info.plist")
[ "$BUILT_ID" = "$WANT_ID" ] || die "the bundle id is $BUILT_ID, but the app is compiled as $WANT_ID (desktop/core/src/identity.rs): it would refuse to start"
echo "ok: bundle id $BUILT_ID, the app's own (core/src/identity.rs)"
plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist" | grep -qx "$VERSION" || die "wrong bundle version"
if [ "$E2E" = 0 ]; then
  # The licence and the notices, in the app as they were checked above.
  for legal in LICENSE THIRD_PARTY_NOTICES; do
    cmp -s "$LEGAL/$legal" "$APP/Contents/Resources/$legal" \
      || die "the app has no $legal in Contents/Resources, or not the one made above: a release ships its licence and notices"
  done
  for crate in tauri tokio; do
    LC_ALL=C grep -qE "^  $crate [0-9]" "$APP/Contents/Resources/THIRD_PARTY_NOTICES" || die "THIRD_PARTY_NOTICES does not list the crate $crate"
  done
  echo "ok: Contents/Resources has LICENSE and THIRD_PARTY_NOTICES"
fi
codesign --verify --deep --strict "$APP" && echo "codesign: $(codesign -dv "$APP" 2>&1 | sed -n 's/^Signature=//p;s/^Authority=//p' | head -1)"
ditto "$APP" "$OUT/$PRODUCT.app"
if [ "$E2E" = 1 ]; then
  echo "An end-to-end test build (feature e2e). Never ship or upload it." > "$OUT/E2E-TEST-BUILD.txt"
else
  # The DMG: the app, the usual Applications link, and the licence and the
  # notices beside them (desktop/notarise.sh makes the release's the same way).
  mkdir "$WORK/dmg"
  ditto "$APP" "$WORK/dmg/$PRODUCT.app"
  ln -s /Applications "$WORK/dmg/Applications"
  cp "$APP/Contents/Resources/LICENSE" "$APP/Contents/Resources/THIRD_PARTY_NOTICES" "$WORK/dmg/"
  hdiutil create -quiet -volname "$PRODUCT" -srcfolder "$WORK/dmg" -ov -format UDZO "$OUT/$DMG_NAME"
  hdiutil verify "$OUT/$DMG_NAME" >/dev/null && echo "dmg: verified ($PRODUCT.app, Applications, LICENSE, THIRD_PARTY_NOTICES)"
  (cd "$OUT" && shasum -a 256 "$DMG_NAME" > SHA256SUMS && cat SHA256SUMS)
fi

say "built in $OUT"
ls -l "$OUT"
cat <<EOF

Nothing has been published. These are unsigned (ad hoc sealed); a release is
signed and notarised with the Developer ID first (desktop/SIGNING.md).
EOF
if [ "$ARCH" = universal ] && [ "$WORKTREE" = 0 ] && [ "$E2E" = 0 ]; then
  cat <<EOF

The download page links ${DMG_NAME} on release v${VERSION} of ${PUBLIC_REPO}.
To release this app (desktop/SIGNING.md, "Making a release"):

  desktop/notarise.sh ${OUT} ${VERSION}
  desktop/updater-artifacts.sh ${OUT}/Droppen.app ${VERSION} --out ${OUT}/updater
  scripts/publish-release.sh v${VERSION} --server <release-public.sh's --out> --desktop ${OUT} --dry-run

EOF
else
  echo "(Not a release build — a universal, non-e2e build of a committed ref is what gets uploaded.)"
fi
