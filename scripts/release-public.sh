#!/usr/bin/env bash
# Build the public Droppen distribution for macOS and Linux and stage a release
# for getdroppen/droppen's releases. Nothing is published: the
# last thing this prints is the scripts/publish-release.sh command for a human to
# review and run (after the desktop app, when the release has one).
#
# Releases are made by .github/workflows/release.yml (docs/releasing.md), which
# runs this with --prebuilt; run by hand, it is the release Mac's fallback.
#
#   scripts/release-public.sh <version> [--ref <git-ref>] [--out <dir>] [--snapshot] [--no-linux] [--keep-work]
#                             [--prebuilt <dir>] [--no-linux-smoke]
#                             [--web-dist <dir>] [--notices <file>]
#
#   <version>   the release tag, e.g. v0.2.119. It must match the server
#               crate's version at the ref.
#   --ref       what to build (default: the tag <version>). Exported with
#               `git archive`, so uncommitted changes never reach a release.
#   --out       where the release files go (default /tmp/droppen-public-release);
#               must be empty or absent.
#   --snapshot  a dry run of an unreleased ref: <version> may differ from the
#               crate's, and no upload command is printed
#   --no-linux  with --snapshot only: skip the Linux builds (which need a
#               Kubernetes pod); a release always has them
#   --keep-work keep the build directory (sources, node_modules, cargo output)
#   --prebuilt  take the server binaries from <dir> instead of building them:
#               <dir>/<target>/remote-hand-server for each macOS target and
#               <dir>/linux/<arch>/remote-hand-server for each Linux one, built
#               from the same ref as below (with web/dist of the same build
#               embedded). .github/workflows/release.yml builds them in jobs of
#               their own; everything else here is the same.
#   --no-linux-smoke  leave the Linux smoke tests (Docker) to the caller:
#               release.yml runs .github/scripts/smoke-linux.sh on Linux
#               runners, on the binaries unpacked from both tarballs
#   --web-dist  the web app as built elsewhere, from the same ref (the one
#               the --prebuilt binaries embed): its manifest is written from
#               it, and it is not built again
#   --notices   THIRD_PARTY_NOTICES as scripts/third-party-notices.sh wrote it
#               for this version elsewhere (release.yml: a job of its own)
#
# Needs: Node as in .nvmrc, rustup, cargo-about 0.9.2
# (`cargo install --locked cargo-about@0.9.2 --features cli`), and for Linux
# kubectl access to a cluster (scripts/build-linux.sh: DROPPEN_K8S_CONTEXT,
# DROPPEN_K8S_NAMESPACE) and Docker to smoke-test the Linux binaries
# (.github/scripts/smoke-linux.sh).
#
# What it does (with --prebuilt, steps 3's builds are release.yml's jobs):
#
#   1. exports the ref to a scratch directory under /tmp;
#   2. builds the web app as the PUBLIC distribution — DROPPEN_DISTRIBUTION is
#      unset, so `distribution.json` (no plugins) is what gets embedded — and
#      writes its web-manifest.json;
#   3. builds aarch64-apple-darwin and x86_64-apple-darwin (adding the rustup
#      targets if missing), with the builder's paths remapped out of the binary,
#      and meanwhile x86_64 and aarch64 Linux (glibc 2.28) in a throwaway pod
#      (scripts/build-linux.sh);
#   4. checks the binaries (on the programs unpacked from the tarballs, step
#      8, which are byte for byte these): the version names the commit, the
#      only distribution marker is "public", no plugin is bundled, no path
#      under $HOME is left, and nothing names the private source repository;
#   5. writes THIRD_PARTY_NOTICES: every Rust crate compiled in (cargo-about,
#      `about.toml`) and every npm package bundled into the web app
#      (scripts/notices/web-notices.mjs), each licence in full, and fails on a
#      licence that is unknown or not on the allow-list;
#   6. smoke-tests every slice this Mac can run (.github/scripts/smoke-macos.sh)
#      and both Linux binaries on Debian 10 in Docker (smoke-linux.sh);
#   7. fuses a universal binary with lipo and packages universal, aarch64 and
#      x86_64 macOS tarballs, and x86_64 and aarch64 Linux ones, each twice
#      (docs/local-aggregator.md §7.2):
#        droppen-<version>-<os>-<arch>.tar.gz      `droppen`, `remote-hand-server`
#                                                  (a hard link: one file, stored once)
#        remote-hand-<version>-<os>-<arch>.tar.gz  `remote-hand-server`, as before,
#                                                  for installers that know only it
#      plus `INSTALL.txt`, `LICENSE` (AGPL-3.0) and `THIRD_PARTY_NOTICES`, listed
#      by name, with COPYFILE_DISABLE=1 — and `SHA256SUMS` over every tarball,
#      and LICENSE, THIRD_PARTY_NOTICES, the ref's scripts/install.sh (the
#      installer https://droppen.app/install.sh serves, once Droppen Hub is
#      built from this release) and
#      INSTALL-macos.txt / INSTALL-linux.txt as release files of their own;
#   8. unpacks every tarball and checks it again: exactly its members, each
#      program the very binary checked and smoke-tested above, the step-4
#      checks on each program, and a smoke test of the unpacked `droppen` (the
#      program run by its new name) wherever this Mac or Docker can run it;
#   9. writes NOTES.md, the release's text: what the files are, then what
#      changed for people, docs/releases/<version>.md at the ref (written for
#      each release, in English and Chinese, with the desktop app's paragraph
#      when the release has the app; a release without one is refused, a
#      snapshot goes without), all of it through the text checks;
#  10. deletes the scratch directory (cargo target and build dirs included) and
#      prints the publish command: scripts/publish-release.sh, which creates
#      the release as a draft, uploads latest.json (the desktop app's, or the
#      latest release's carried forward) last, and only then publishes it as
#      the latest release.
#
# Environment:
#   DROPPEN_MIN_FREE_GB      refuse to start, and stop a build, when the volume
#                            holding the scratch directory has less free (25)
#   DROPPEN_RELEASE_WORK     the scratch directory (/tmp/droppen-public-build)
#   DROPPEN_RELEASE_REPO     the repository releases are published in (getdroppen/droppen)
#   DROPPEN_ALLOW_NODE_MISMATCH=1  build with a Node other than .nvmrc's (the
#                            web manifest may then not match droppen.app's)
#   DROPPEN_RELEASE_FORBIDDEN  required: the personal names no staged text file
#                            may contain, one per line, matched in any case.
#                            The release operator keeps them out of this
#                            repository and sets them; the list must hash to
#                            RELEASE_TEXT_NAMES_SHA256 in
#                            scripts/release-text-check.sh, which also has the
#                            rule for "Kilerd" (the copyright holder). Unset,
#                            it is read (as data, never run) from
#                            ~/.config/droppen/release-forbidden.env
#   DROPPEN_RELEASE_NOTICE_ALLOW  the lines of THIRD_PARTY_NOTICES that may
#                            contain one of them anyway, one per line, each
#                            one whole third-party copyright line written out:
#                            ^Copyright …$ with its metacharacters escaped,
#                            nothing else; set by the operator with the names
#   DROPPEN_RELEASE_NAMES_DEFERRED=1  release.yml's build jobs only: without
#                            the names, check "Kilerd" only and leave the names
#                            to the release job (scripts/release-text-check.sh)
#
# The install script itself (scripts/install.sh, published with every release
# as its file install.sh: https://github.com/getdroppen/droppen/releases/latest/download/install.sh,
# which https://droppen.app/install.sh serves once Droppen Hub is built from
# the release) downloads `SHA256SUMS` and then
# `droppen-<version>-<macos|linux>-<arch>.tar.gz`, or, when that is missing,
# `remote-hand-<version>-<macos|linux>-<arch>.tar.gz` — which every older copy
# of it (and README's by-hand steps) asks for: keep publishing both names until
# at least OLD_ASSETS_UNTIL below.
set -euo pipefail

usage() { sed -n '2,/^# Needs:/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }

VERSION=""
REF=""
OUT=/tmp/droppen-public-release
KEEP_WORK=0
SNAPSHOT=0
LINUX=1
PREBUILT=""
LINUX_SMOKE=1
WEB_DIST=""
NOTICES_FROM=""
while [ $# -gt 0 ]; do
  case "$1" in
    --ref) REF=${2:?--ref needs a value}; shift 2 ;;
    --out) OUT=${2:?--out needs a value}; shift 2 ;;
    --keep-work) KEEP_WORK=1; shift ;;
    --snapshot) SNAPSHOT=1; shift ;;
    --no-linux) LINUX=0; shift ;;
    --prebuilt) PREBUILT=${2:?--prebuilt needs a directory}; shift 2 ;;
    --no-linux-smoke) LINUX_SMOKE=0; shift ;;
    --web-dist) WEB_DIST=${2:?--web-dist needs a directory}; shift 2 ;;
    --notices) NOTICES_FROM=${2:?--notices needs a file}; shift 2 ;;
    -h|--help) usage ;;
    -*) echo "unknown option: $1" >&2; usage ;;
    *) [ -z "$VERSION" ] || usage; VERSION=$1; shift ;;
  esac
done
[ -n "$VERSION" ] || usage
[ "$LINUX" = 1 ] || [ "$SNAPSHOT" = 1 ] || { echo "--no-linux is for --snapshot dry runs only: a release carries the Linux builds" >&2; exit 2; }
[[ "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || { echo "version must look like v1.2.3, not $VERSION" >&2; exit 2; }

PUBLIC_REPO=${DROPPEN_RELEASE_REPO:-getdroppen/droppen}
MIN_FREE_GB=${DROPPEN_MIN_FREE_GB:-25}
WORK=${DROPPEN_RELEASE_WORK:-/tmp/droppen-public-build}
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
LINUX_ARCHES=(x86_64 aarch64)
# The oldest glibc the Linux binaries may need (build-linux.sh links against it).
GLIBC_MAX=2.28
BIN_NAME=remote-hand-server
# The same program's new name: the command people type.
NEW_NAME=droppen
# The old tarball names stay on every release until at least this date (three
# months after the first release that also had the new ones).
# shellcheck disable=SC2034 # scripts/release-notes.sh reads it from here
OLD_ASSETS_UNTIL=2027-01-31
ABOUT_VERSION=0.9.2
# Strings no public binary may contain: the private repositories' names (the
# source as it is today, and the closed Hub's repository after the split).
FORBIDDEN=("Kilerd/remote-hand" "getdroppen/hub")

say() { printf '\n== %s\n' "$*"; }
die() { printf 'release-public: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "this builds macOS binaries and needs a Mac"
for tool in git cargo rustup lipo shasum tar node npx file; do
  command -v "$tool" >/dev/null || die "missing $tool"
done
if [ -n "$PREBUILT" ]; then
  PREBUILT=$(cd "$PREBUILT" 2>/dev/null && pwd -P) || die "--prebuilt: no such directory"
fi
if [ "$LINUX" = 1 ]; then
  [ -n "$PREBUILT" ] || command -v kubectl >/dev/null || die "the Linux builds need kubectl (or pass --no-linux)"
  [ "$LINUX_SMOKE" = 0 ] || docker info >/dev/null 2>&1 || die "the Linux smoke tests need Docker (or pass --no-linux)"
fi
if [ -z "$NOTICES_FROM" ]; then
  [ "$(cargo about --version 2>/dev/null)" = "cargo-about $ABOUT_VERSION" ] \
    || die "needs cargo-about $ABOUT_VERSION: cargo install --locked cargo-about@$ABOUT_VERSION --features cli"
else
  [ -s "$NOTICES_FROM" ] || die "--notices: no such file"
fi
if [ -n "$WEB_DIST" ]; then
  [ -f "$WEB_DIST/index.html" ] || die "--web-dist: $WEB_DIST holds no built web app"
  WEB_DIST=$(cd "$WEB_DIST" && pwd -P)
fi

REPO=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
REF=${REF:-$VERSION}
SHA=$(git -C "$REPO" rev-parse --verify --quiet "$REF^{commit}") \
  || die "cannot resolve $REF (fetch the tag, or pass --ref <commit>)"
REV=$(git -C "$REPO" rev-parse --short "$SHA")
COMMIT_TIME=$(git -C "$REPO" log -1 --format=%ct "$SHA")
CRATE_VERSION=$(git -C "$REPO" show "$SHA:server-rs/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)
if [ "v$CRATE_VERSION" != "$VERSION" ] && [ "$SNAPSHOT" != 1 ]; then
  die "$REF is server version $CRATE_VERSION, not $VERSION (pass --ref for the right commit)"
fi
# What changed, in people's words: NOTES.md ends with it (step 9).
RELEASE_NOTES=docs/releases/$VERSION.md
if ! git -C "$REPO" cat-file -e "$SHA:$RELEASE_NOTES" 2>/dev/null; then
  [ "$SNAPSHOT" = 1 ] || die "$REF has no $RELEASE_NOTES: write what changed for people, in English and Chinese (docs/releases/ has the earlier ones), commit it, and tag that"
  RELEASE_NOTES=""
fi

case "$WORK" in /tmp/*|/private/tmp/*|"${TMPDIR:-/nonexistent}"*) ;; *) die "DROPPEN_RELEASE_WORK must be under /tmp" ;; esac
case "$WORK$OUT" in *" "*) die "no spaces in the work or output path, please" ;; esac
if [ -e "$OUT" ] && [ -n "$(ls -A "$OUT")" ]; then
  die "$OUT is not empty; move it aside or pass --out <new dir>"
fi
# The rule of the text checks at the end (personal names), and the names it
# looks for besides "Kilerd": without all of them the checks would pass what
# they exist to stop, so nothing is built.
. "$REPO/scripts/release-text-check.sh"
# Unset, they are read from the operator's file, as data (never run).
release_text_load_names "$HOME/.config/droppen/release-forbidden.env"
if release_text_names_deferred; then
  echo "note: DROPPEN_RELEASE_NAMES_DEFERRED: the text checks look for \"Kilerd\" only; the names are checked later, by whoever has them"
else
  release_text_names_ok \
    || die "DROPPEN_RELEASE_FORBIDDEN is not the expected list of names (its sha256 is RELEASE_TEXT_NAMES_SHA256 in scripts/release-text-check.sh; see Environment in this script's header)"
fi
release_text_allow_ok \
  || die "DROPPEN_RELEASE_NOTICE_ALLOW may only spell out whole third-party copyright lines, each ^Copyright …\$ with its metacharacters escaped"

free_gb() { df -Pk "$1" | awk 'NR == 2 { print int($4 / 1048576) }'; }
mkdir -p "$(dirname "$WORK")"
FREE=$(free_gb "$(dirname "$WORK")")
# The build needs roughly 4 GB (two release targets' intermediates and
# node_modules); refuse unless that still leaves the floor intact.
[ "$FREE" -ge $((MIN_FREE_GB + 5)) ] \
  || die "only ${FREE} GB free; need at least $((MIN_FREE_GB + 5)) GB to stay above ${MIN_FREE_GB} GB"

NVMRC=$(git -C "$REPO" show "$SHA:.nvmrc" | tr -d '[:space:]')
NODE_VERSION=$(node --version | sed 's/^v//')
if [ "$NODE_VERSION" != "$NVMRC" ] && [ "${DROPPEN_ALLOW_NODE_MISMATCH:-}" != 1 ]; then
  die "node is $NODE_VERSION but .nvmrc says $NVMRC: put Node $NVMRC first on PATH (the public web build is reproducible only with it), or set DROPPEN_ALLOW_NODE_MISMATCH=1"
fi

cleanup() {
  local status=$?
  # Each long step runs in a process group of its own (set -m): stop the
  # group, by its id, so nothing it started lingers. The cargo step first, on
  # its own; then the Linux build, whose trap deletes its pod as it ends.
  if [ -n "${WATCHED:-}" ] && kill -0 "$WATCHED" 2>/dev/null; then kill -TERM -- "-$WATCHED" 2>/dev/null || kill -TERM "$WATCHED" 2>/dev/null || true; fi
  if [ -n "${LINUX_PID:-}" ] && kill -0 "$LINUX_PID" 2>/dev/null; then
    kill -TERM -- "-$LINUX_PID" 2>/dev/null || kill -TERM "$LINUX_PID" 2>/dev/null || true
    wait "$LINUX_PID" 2>/dev/null || true
  fi
  if [ "$KEEP_WORK" = 1 ]; then
    echo "kept the build directory: $WORK" >&2
  else
    rm -rf "$WORK"
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# Run a long step in its own process group, and stop that group (by its id)
# if the volume falls under the floor: other builds share this disk.
set -m
run_watched() {
  "$@" &
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

say "$VERSION from $SHA (rev $REV), ${FREE} GB free, work in $WORK"
rm -rf "$WORK"
# Everything the release consists of is made here and moves to $OUT only once
# the text checks below have passed: $OUT never holds a release that failed them.
STAGE=$WORK/release
mkdir -p "$WORK/src" "$STAGE"
git -C "$REPO" archive --format=tar "$SHA" | tar -x -C "$WORK/src"
SRC=$WORK/src

if [ -z "$PREBUILT" ]; then
  say "rust targets"
  installed=$(rustup target list --installed)
  for target in "${TARGETS[@]}"; do
    grep -qx "$target" <<<"$installed" || rustup target add "$target"
  done
fi

say "web app: the public distribution"
PNPM_VERSION=$(sed -n 's/.*"packageManager": *"pnpm@\([^"]*\)".*/\1/p' "$SRC/package.json")
mkdir -p "$WORK/shims"
if ! command -v pnpm >/dev/null || [ "$(pnpm --version 2>/dev/null)" != "$PNPM_VERSION" ]; then
  # Package scripts call `pnpm`; give them the pinned one.
  printf '#!/bin/sh\nexec npx -y pnpm@%s "$@"\n' "$PNPM_VERSION" > "$WORK/shims/pnpm"
  chmod +x "$WORK/shims/pnpm"
fi
# No DROPPEN_DISTRIBUTION: the build then reads `distribution.json`, the public one.
web_env=(env -u DROPPEN_DISTRIBUTION PATH="$WORK/shims:$PATH" SOURCE_DATE_EPOCH="$COMMIT_TIME")
if [ -n "$WEB_DIST" ]; then
  # Built once, elsewhere: the very bytes the binaries embed. (CI's
  # web-reproducible jobs keep checking that a build is reproducible.)
  rm -rf "$SRC/web/dist"
  cp -R "$WEB_DIST" "$SRC/web/dist"
  echo "web app: $WEB_DIST (--web-dist), not built again"
  # The notices build it once more, to see what it bundles.
  [ -n "$NOTICES_FROM" ] || run_watched "${web_env[@]}" pnpm -C "$SRC" install --frozen-lockfile --filter "@remote-hand/web..."
else
  run_watched "${web_env[@]}" pnpm -C "$SRC" install --frozen-lockfile --filter "@remote-hand/web..."
  run_watched "${web_env[@]}" pnpm -C "$SRC" --filter @remote-hand/web build
fi
# The public distribution, and nothing else.
node -e '
  const m = require(process.argv[1]);
  if (m.distribution !== undefined && m.distribution !== "public") { console.error("distribution is", m.distribution); process.exit(1); }
  if ((m.plugins || []).length) { console.error("the public build bundles plugins:", m.plugins.map((p) => p.id)); process.exit(1); }
' "$SRC/web/dist/plugins/manifest.json" || die "web/dist is not the public distribution"
# No fact only the owner can supply left unfilled on droppen.app's pages: the
# placeholders show marked there and main's tests pass with them, but a
# release ships none (web/owner-placeholders.txt, release_text_owner_placeholders).
placeholders=$(release_text_owner_placeholders "$SRC/web/dist" "$SRC/web/owner-placeholders.txt")
[ -z "$placeholders" ] || die "web/dist still holds owner placeholders: fill them in first (web/owner-placeholders.txt lists them)
$placeholders"
echo "ok: no owner placeholder left in web/dist"
node "$SRC/scripts/web-manifest.mjs" "$SRC/web/dist" --commit "$SHA" --ref "refs/tags/$VERSION" --out "$STAGE/web-manifest.json"

if [ "$LINUX" = 1 ] && [ -z "$PREBUILT" ]; then
  say "server: Linux ${LINUX_ARCHES[*]}, in a pod, alongside the macOS builds"
  "$REPO/scripts/build-linux.sh" "$SRC" "$WORK/linux" "$REV" > "$WORK/linux-build.log" 2>&1 &
  LINUX_PID=$!
fi

CARGO_HOME_DIR=${CARGO_HOME:-$HOME/.cargo}
if [ -n "$PREBUILT" ]; then
  say "server: prebuilt, from $PREBUILT"
  # Where the builds below would have left them, so that everything after
  # treats them alike.
  for target in "${TARGETS[@]}"; do
    [ -x "$PREBUILT/$target/$BIN_NAME" ] || die "--prebuilt: no executable $PREBUILT/$target/$BIN_NAME"
    mkdir -p "$WORK/target/$target/release"
    cp "$PREBUILT/$target/$BIN_NAME" "$WORK/target/$target/release/"
  done
  if [ "$LINUX" = 1 ]; then
    for arch in "${LINUX_ARCHES[@]}"; do
      [ -x "$PREBUILT/linux/$arch/$BIN_NAME" ] || die "--prebuilt: no executable $PREBUILT/linux/$arch/$BIN_NAME"
      mkdir -p "$WORK/linux/$arch"
      cp "$PREBUILT/linux/$arch/$BIN_NAME" "$WORK/linux/$arch/"
    done
  fi
  TARGETS_TO_BUILD=()
else
  say "server: ${TARGETS[*]}"
  TARGETS_TO_BUILD=("${TARGETS[@]}")
fi
for target in ${TARGETS_TO_BUILD[@]+"${TARGETS_TO_BUILD[@]}"}; do
  # A dedicated target and build dir under the scratch directory, so the
  # cleanup takes every byte of it; the remaps keep the builder's paths
  # (and with them the user name) out of panic locations. The commit goes in
  # under both names: REF may be from before the rename to Droppen, whose
  # build reads only RH_BUILD_REV.
  run_watched env \
    CARGO_TARGET_DIR="$WORK/target" \
    CARGO_BUILD_BUILD_DIR="$WORK/build" \
    DROPPEN_BUILD_REV="$REV" RH_BUILD_REV="$REV" \
    RUSTFLAGS="--remap-path-prefix=$SRC=/droppen --remap-path-prefix=$CARGO_HOME_DIR=/cargo" \
    cargo build --release --locked --target "$target" --manifest-path "$SRC/server-rs/Cargo.toml"
done

if [ "$LINUX" = 1 ] && [ -z "$PREBUILT" ]; then
  say "server: waiting for the Linux builds"
  if ! wait "$LINUX_PID"; then
    LINUX_PID=
    tail -n 40 "$WORK/linux-build.log" >&2
    die "the Linux builds failed (the log's last lines are above)"
  fi
  LINUX_PID=
  tail -n 3 "$WORK/linux-build.log"
fi

# The checks of every binary (check_binary) are made on the programs of
# every tarball, below ("tarball checks"): byte for byte these.
bin_of() { echo "$WORK/target/$1/release/$BIN_NAME"; }
check_binary() { # path
  local bin=$1 markers kind
  markers=$(LC_ALL=C grep -ao '"distribution": "[^"]*"' "$bin" | sort -u)
  [ "$markers" = '"distribution": "public"' ] || die "$bin: distribution markers are [$markers], not exactly \"public\""
  for needle in '"id": "tasks"' 'plugin-tasks' '.droppen-plugins/'; do
    if LC_ALL=C grep -aqF "$needle" "$bin"; then die "$bin: contains $needle (a bundled plugin)"; fi
  done
  if LC_ALL=C grep -aqF "$HOME/" "$bin"; then
    die "$bin: still names paths under $HOME ($(LC_ALL=C grep -aoF "$HOME/" "$bin" | wc -l | tr -d ' ') times)"
  fi
  for needle in "${FORBIDDEN[@]}"; do
    if LC_ALL=C grep -aqF "$needle" "$bin"; then die "$bin: names $needle"; fi
  done
  # The pod's build paths (remapped like $HOME's on the Mac).
  if LC_ALL=C grep -aqF "/work/src/" "$bin"; then die "$bin: still names the build pod's /work/src"; fi
  kind=$(file -b "$bin")
  case "$kind" in
    ELF*)
      local glibc
      glibc=$(LC_ALL=C grep -aoE 'GLIBC_2\.[0-9]+' "$bin" | sort -t. -k2,2n -u | tail -n 1)
      [ "$(printf '%s\n%s\n' "GLIBC_$GLIBC_MAX" "$glibc" | sort -t. -k2,2n | tail -n 1)" = "GLIBC_$GLIBC_MAX" ] \
        || die "$bin: needs $glibc, newer than glibc $GLIBC_MAX"
      kind="$(printf '%s' "$kind" | cut -d, -f2 | sed 's/^ //'), needs ${glibc:-no versioned glibc}" ;;
    *) kind=$(lipo -archs "$bin") ;;
  esac
  echo "ok: $kind $bin: distribution public, no plugins, no \$HOME paths, no ${FORBIDDEN[*]}"
}
check_linux_arch() { # path, arch
  case "$(file -b "$1")" in
    *"$([ "$2" = x86_64 ] && echo x86-64 || echo aarch64)"*) ;;
    *) die "$1 is not an $2 binary" ;;
  esac
}

say "licences"
# The licence and notices tooling from this checkout, like INSTALL.txt below;
# the dependency graph and node_modules from the ref.
LICENSE_FILE=$SRC/LICENSE
[ -f "$LICENSE_FILE" ] || LICENSE_FILE=$REPO/LICENSE
grep -q "GNU AFFERO GENERAL PUBLIC LICENSE" "$LICENSE_FILE" || die "$LICENSE_FILE is not the AGPL-3.0 text"
NOTICES=$WORK/THIRD_PARTY_NOTICES
if [ -n "$NOTICES_FROM" ]; then
  # Written elsewhere by scripts/third-party-notices.sh, for this very version.
  cp "$NOTICES_FROM" "$NOTICES"
  [ "$(head -n 1 "$NOTICES")" = "THIRD_PARTY_NOTICES for Droppen $VERSION (remote-hand-server $CRATE_VERSION+g$REV)" ] \
    || die "--notices: $NOTICES_FROM is not the THIRD_PARTY_NOTICES of $VERSION (remote-hand-server $CRATE_VERSION+g$REV)"
  if LC_ALL=C grep -qF "$HOME/" "$NOTICES"; then die "THIRD_PARTY_NOTICES names paths under $HOME"; fi
else
  # The licence and notices tooling from this checkout, like INSTALL.txt
  # below; the dependency graph and node_modules from the ref.
  run_watched "${web_env[@]}" "$REPO/scripts/third-party-notices.sh" "$SRC" "$NOTICES" "$VERSION" "$CRATE_VERSION" "$REV" \
    || die "THIRD_PARTY_NOTICES failed (see above)"
fi
echo "ok: THIRD_PARTY_NOTICES $(wc -l < "$NOTICES" | tr -d ' ') lines; LICENSE from ${LICENSE_FILE#"$WORK/"}"

mkdir -p "$WORK/dist/universal" "$WORK/dist/aarch64" "$WORK/dist/x86_64"
lipo -create -output "$WORK/dist/universal/$BIN_NAME" "$(bin_of aarch64-apple-darwin)" "$(bin_of x86_64-apple-darwin)"
cp "$(bin_of aarch64-apple-darwin)" "$WORK/dist/aarch64/"
cp "$(bin_of x86_64-apple-darwin)" "$WORK/dist/x86_64/"
lipo -info "$WORK/dist/universal/$BIN_NAME"

say "smoke tests"
can_run() { # arch
  case "$1" in
    aarch64) [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ] ;;
    # Natively on Intel; on Apple silicon only with Rosetta installed.
    x86_64) [ "$(uname -m)" = x86_64 ] || /usr/bin/arch -x86_64 /usr/bin/true 2>/dev/null ;;
    universal) true ;;
  esac
}
for arch in universal aarch64 x86_64; do
  bin="$WORK/dist/$arch/$BIN_NAME"
  if ! can_run "$arch"; then echo "skip: this Mac cannot run the $arch slice"; continue; fi
  version=$("$bin" --version)
  case "$version" in *"+g$REV"*) ;; *) die "$arch --version says '$version', not the commit $REV" ;; esac
  # The server reads APP_* from the environment; a session's own must not leak in.
  env -u APP_INSTANCE -u APP_SERVER__PORT -u APP_SERVER__HOST -u APP_HOME_DIR \
    -u DROPPEN_DATA_DIR -u DROPPEN_CONFIG_DIR -u REMOTE_HAND_DATA_DIR -u REMOTE_HAND_CONFIG_DIR \
    bash "$SRC/.github/scripts/smoke-macos.sh" "$bin"
done
if [ "$LINUX" = 1 ] && [ "$LINUX_SMOKE" = 1 ]; then
  for arch in "${LINUX_ARCHES[@]}"; do
    EXPECT_VERSION="+g$REV" bash "$REPO/.github/scripts/smoke-linux.sh" "$WORK/linux/$arch/$BIN_NAME" "$arch" \
      || die "the Linux $arch binary failed its smoke test"
  done
elif [ "$LINUX" = 1 ]; then
  echo "skip: the Linux smoke tests (--no-linux-smoke: the caller runs them)"
fi

say "package"
# The INSTALL.txt line that says what the tarball holds.
CONTENTS_NEW="\`droppen\` and \`remote-hand-server\` in this archive are one file."
CONTENTS_OLD="\`remote-hand-server\` in this archive is the same program as \`droppen\`."
# package <binary> <os> <arch> <group> <INSTALL template>: both tarballs of one
# binary. Members by name, not `.`: the archive holds `droppen`, not
# `./droppen`. COPYFILE_DISABLE keeps `._*` AppleDouble files out, --no-xattrs
# the macOS attributes GNU tar warns about on Linux, and the owner is root
# rather than whoever ran this. The supervisor resolves
# `<data>/bin/remote-hand-server`, so that name inside both is load-bearing.
package() {
  local bin=$1 os=$2 arch=$3 group=$4 template=$5 new old
  new=$WORK/pkg/$NEW_NAME-$os-$arch
  old=$WORK/pkg/remote-hand-$os-$arch
  mkdir -p "$new" "$old"
  cp "$bin" "$new/$NEW_NAME"
  chmod 755 "$new/$NEW_NAME"
  # A hard link: the tarball stores the second name as a link to the first.
  ln "$new/$NEW_NAME" "$new/$BIN_NAME"
  cp "$bin" "$old/$BIN_NAME"
  chmod 755 "$old/$BIN_NAME"
  # The templates from this checkout, not from the ref: they point at the
  # public repository, which an older tag's copy predates.
  sed -e "s/@ARCH@/${arch}/" -e "s/@CONTENTS@/${CONTENTS_NEW}/" "$template" > "$new/INSTALL.txt"
  sed -e "s/@ARCH@/${arch}/" -e "s/@CONTENTS@/${CONTENTS_OLD}/" "$template" > "$old/INSTALL.txt"
  for dir in "$new" "$old"; do
    cp "$LICENSE_FILE" "$dir/LICENSE"
    cp "$NOTICES" "$dir/THIRD_PARTY_NOTICES"
  done
  COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata --uid 0 --gid 0 --uname root --gname "$group" \
    -czf "$STAGE/$NEW_NAME-${VERSION}-${os}-${arch}.tar.gz" -C "$new" "$NEW_NAME" "$BIN_NAME" INSTALL.txt LICENSE THIRD_PARTY_NOTICES
  COPYFILE_DISABLE=1 tar --no-xattrs --no-mac-metadata --uid 0 --gid 0 --uname root --gname "$group" \
    -czf "$STAGE/remote-hand-${VERSION}-${os}-${arch}.tar.gz" -C "$old" "$BIN_NAME" INSTALL.txt LICENSE THIRD_PARTY_NOTICES
}
for template in INSTALL.txt.in INSTALL-linux.txt.in; do
  grep -q '@CONTENTS@' "$REPO/.github/scripts/$template" || die "$template has no @CONTENTS@ line"
done
for arch in universal aarch64 x86_64; do
  package "$WORK/dist/$arch/$BIN_NAME" macos "$arch" wheel "$REPO/.github/scripts/INSTALL.txt.in"
done
if [ "$LINUX" = 1 ]; then
  for arch in "${LINUX_ARCHES[@]}"; do
    package "$WORK/linux/$arch/$BIN_NAME" linux "$arch" root "$REPO/.github/scripts/INSTALL-linux.txt.in"
  done
fi

say "tarball checks"
# Every tarball, unpacked as a person would: exactly its members, each program
# byte for byte the binary the smoke tests above passed, and the binary checks
# (check_binary) on what was unpacked: the one pass of them. All tarballs at
# once, each with a log of its own, printed in order; any failure fails.
UNPACKED=$WORK/unpacked
verify_tarball() { # tarball, the binary it was made from, its members
  local tarball=$1 bin=$2 dir listing member checked=""
  shift 2
  dir=$UNPACKED/$(basename "$tarball" .tar.gz)
  mkdir -p "$dir"
  listing=$(tar -tzf "$tarball")
  [ "$listing" = "$(printf '%s\n' "$@")" ] || die "$(basename "$tarball") holds [$(printf '%s ' "$listing" | tr '\n' ' ')], not [$*]"
  tar -xzf "$tarball" -C "$dir"
  for member in "$NEW_NAME" "$BIN_NAME"; do
    [ -e "$dir/$member" ] || continue
    [ -x "$dir/$member" ] || die "$(basename "$tarball"): $member is not executable"
    cmp -s "$dir/$member" "$bin" || die "$(basename "$tarball"): $member is not the binary that was smoke-tested"
    # A hard link to the member just checked: the same file.
    if [ -n "$checked" ] && [ "$dir/$member" -ef "$checked" ]; then
      echo "ok: $dir/$member is $(basename "$checked"), one file"
      continue
    fi
    check_binary "$dir/$member"
    checked=$dir/$member
  done
}
CHECK_PIDS=()
CHECK_NAMES=()
for tarball in "$STAGE"/*.tar.gz; do
  name=$(basename "$tarball" .tar.gz)
  case "$name" in
    "$NEW_NAME-$VERSION-"*) family=$NEW_NAME; members=("$NEW_NAME" "$BIN_NAME") ;;
    "remote-hand-$VERSION-"*) family=remote-hand; members=("$BIN_NAME") ;;
    *) die "an unexpected tarball: $name" ;;
  esac
  platform=${name#"$family-$VERSION-"}
  os=${platform%%-*}
  arch=${platform#*-}
  case "$os" in
    macos) bin=$WORK/dist/$arch/$BIN_NAME ;;
    linux) bin=$WORK/linux/$arch/$BIN_NAME ;;
    *) die "an unexpected platform in $name" ;;
  esac
  (
    verify_tarball "$tarball" "$bin" "${members[@]}" INSTALL.txt LICENSE THIRD_PARTY_NOTICES
    [ "$os" != linux ] || check_linux_arch "$bin" "$arch"
  ) >"$WORK/check-$name.log" 2>&1 &
  CHECK_PIDS+=("$!")
  CHECK_NAMES+=("$name")
done
CHECK_FAILED=()
for i in "${!CHECK_PIDS[@]}"; do
  wait "${CHECK_PIDS[$i]}" || CHECK_FAILED+=("${CHECK_NAMES[$i]}")
  sed "s|^|${CHECK_NAMES[$i]}: |" "$WORK/check-${CHECK_NAMES[$i]}.log"
done
[ "${#CHECK_FAILED[@]}" = 0 ] || die "the checks of ${CHECK_FAILED[*]} failed (above)"
# Both families of each platform, or neither.
for tarball in "$STAGE/remote-hand-$VERSION-"*.tar.gz; do
  name=$(basename "$tarball")
  [ -f "$STAGE/$NEW_NAME-${name#remote-hand-}" ] || die "$name has no $NEW_NAME-${name#remote-hand-}"
done

say "smoke tests: the unpacked droppen"
# The program run by its new name, from the new tarball; the old tarball's is
# byte for byte the binary smoke-tested above.
for arch in universal aarch64 x86_64; do
  bin="$UNPACKED/$NEW_NAME-${VERSION}-macos-$arch/$NEW_NAME"
  if ! can_run "$arch"; then echo "skip: this Mac cannot run the $arch slice"; continue; fi
  version=$("$bin" --version)
  case "$version" in *"+g$REV"*) ;; *) die "$arch $NEW_NAME --version says '$version', not the commit $REV" ;; esac
  # sed reads to the end, so the binary never writes into a closed pipe.
  case "$("$bin" --help | sed -n 1p)" in "Droppen server: \`$NEW_NAME\`"*) ;; *) die "$arch $NEW_NAME --help does not say $NEW_NAME" ;; esac
  env -u APP_INSTANCE -u APP_SERVER__PORT -u APP_SERVER__HOST -u APP_HOME_DIR \
    -u DROPPEN_DATA_DIR -u DROPPEN_CONFIG_DIR -u REMOTE_HAND_DATA_DIR -u REMOTE_HAND_CONFIG_DIR \
    bash "$SRC/.github/scripts/smoke-macos.sh" "$bin"
done
if [ "$LINUX" = 1 ] && [ "$LINUX_SMOKE" = 1 ]; then
  for arch in "${LINUX_ARCHES[@]}"; do
    EXPECT_VERSION="+g$REV" bash "$REPO/.github/scripts/smoke-linux.sh" "$UNPACKED/$NEW_NAME-${VERSION}-linux-$arch/$NEW_NAME" "$arch" \
      || die "the unpacked Linux $arch $NEW_NAME failed its smoke test"
  done
fi

(cd "$STAGE" && shasum -a 256 ./*.tar.gz > SHA256SUMS && cat SHA256SUMS)
cp "$LICENSE_FILE" "$STAGE/LICENSE"
cp "$NOTICES" "$STAGE/THIRD_PARTY_NOTICES"
# The installer of this very ref, and the tarballs' INSTALL texts for every
# architecture at once (each tarball has its own).
cp "$SRC/scripts/install.sh" "$STAGE/install.sh"
sed -e "s/@ARCH@/Apple silicon and Intel/" -e "s/@CONTENTS@/${CONTENTS_NEW}/" "$REPO/.github/scripts/INSTALL.txt.in" > "$STAGE/INSTALL-macos.txt"
if [ "$LINUX" = 1 ]; then
  sed -e "s/@ARCH@/x86_64 and aarch64/" -e "s/@CONTENTS@/${CONTENTS_NEW}/" "$REPO/.github/scripts/INSTALL-linux.txt.in" > "$STAGE/INSTALL-linux.txt"
fi

# The release's text (scripts/release-notes.sh, which release.yml's verify
# job runs again to compare), with the notes from the exported ref, like
# everything built here.
NOTES_ARGS=("$VERSION" "$SHA" "$REV" "$CRATE_VERSION" "$LINUX")
[ -z "$RELEASE_NOTES" ] || NOTES_ARGS+=("$SRC/$RELEASE_NOTES")
"$REPO/scripts/release-notes.sh" "${NOTES_ARGS[@]}" >"$STAGE/NOTES.md"

say "text checks"
# Fail closed on personal names in what the release ships as text. The
# binaries are checked against FORBIDDEN above; here every staged text file is
# read, by the rule in scripts/release-text-check.sh (sourced above):
#  - "Kilerd" only as the copyright line, `Copyright (C) 2026 Kilerd`, and in
#    THIRD_PARTY_NOTICES, which quotes other people's licences, also in the
#    repository URLs of the published crates listed in NOTICE_KILERD; never in
#    any other form.
#  - None of the names in DROPPEN_RELEASE_FORBIDDEN, which the release
#    operator sets (see Environment above), in any case; in THIRD_PARTY_NOTICES
#    except on the third-party copyright lines DROPPEN_RELEASE_NOTICE_ALLOW
#    matches. Anything new fails and needs a look.
check_text() { # file, [notices]
  local hits
  hits=$(release_text_hits "$@")
  [ -z "$hits" ] || die "$1 names a person it should not:
$hits"
}
for file in "$WORK"/pkg/*/INSTALL.txt "$STAGE"/INSTALL-*.txt "$STAGE/install.sh" "$STAGE/NOTES.md" "$STAGE/LICENSE" "$STAGE/web-manifest.json"; do check_text "$file"; done
check_text "$STAGE/THIRD_PARTY_NOTICES" notices
if release_text_names_deferred; then
  echo "ok: \"Kilerd\" only on the copyright lines of INSTALL.txt, NOTES.md, LICENSE, web-manifest.json and THIRD_PARTY_NOTICES (the names: DROPPEN_RELEASE_NAMES_DEFERRED, checked later)"
else
  echo "ok: no personal names in INSTALL.txt, NOTES.md, LICENSE, web-manifest.json or THIRD_PARTY_NOTICES"
fi

mkdir -p "$OUT"
mv "$STAGE"/* "$OUT"/
say "staged in $OUT"
ls -l "$OUT"
if [ "$SNAPSHOT" = 1 ]; then
  printf '\nA snapshot of %s (server %s), not a release: nothing to upload.\n' "$SHA" "$CRATE_VERSION"
  exit 0
fi
cat <<EOF

Nothing has been published.

The installer people run is this release's install.sh, staged above from
the ref, once Droppen Hub is built from this release
(https://droppen.app/install.sh serves the install.sh of the release the hub
was built from, not a redirect to
https://github.com/${PUBLIC_REPO}/releases/latest/download/install.sh).

To publish, review the files above. If this release has the desktop app,
build, sign and notarise it first (desktop/SIGNING.md: build.sh, notarise.sh,
updater-artifacts.sh). Then check with a dry run, and publish:

  scripts/publish-release.sh ${VERSION} --server ${OUT} [--desktop /tmp/droppen-desktop-release] --dry-run
  scripts/publish-release.sh ${VERSION} --server ${OUT} [--desktop /tmp/droppen-desktop-release]

Without --desktop it carries the latest release's latest.json and DMG
forward, so installed desktop apps keep finding their update and the download
page its DMG.

EOF
