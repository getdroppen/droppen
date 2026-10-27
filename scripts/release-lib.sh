# shellcheck shell=bash
# What every release script shares: the say/die/usage lines, the free-space
# guard and its process-group cleanup, the watched long step, the rustup
# targets, and the pnpm shim and web environment.
# Sourced by desktop/build.sh, scripts/release-public.sh, desktop/notarise.sh,
# desktop/updater-artifacts.sh and scripts/publish-release.sh; not run on its
# own.
#
# A caller sets RELEASE_DIE_NAME before sourcing (the prefix its error lines
# have always carried), and the globals the functions it uses read: WORK,
# MIN_FREE_GB, KEEP_WORK, WATCHED (and LINUX_PID, for the Linux build),
# TARGETS, SRC, COMMIT_TIME. Every function prints, exits and returns exactly
# as the copy it replaces did.

say() { printf '\n== %s\n' "$*"; }

die() { printf '%s: %s\n' "$RELEASE_DIE_NAME" "$*" >&2; exit 1; }

# The usage text: the comment block at the top of the script, from its second
# line to the line matching $1 (which is dropped), without the leading "# ".
release_usage() { sed -n "2,${1:-/^set -euo/}p" "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }

free_gb() { df -Pk "$1" | awk 'NR == 2 { print int($4 / 1048576) }'; }

# At exit: stop the long steps this script started — the watched one, by its
# process group's id WATCHED; release-public's Linux build, LINUX_PID; each
# only while it runs — then remove the scratch directory unless KEEP_WORK says
# to keep it.
cleanup() {
  local status=$?
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

# Run a long step in its own process group (the caller's `set -m`), and stop
# that group (by its id) if the volume falls under the floor: other builds
# share this disk. RELEASE_WATCH_NICE=1 nices the step, as build.sh's builds
# have always done.
run_watched() {
  if [ "${RELEASE_WATCH_NICE:-0}" = 1 ]; then
    nice -n 10 "$@" &
  else
    "$@" &
  fi
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

# The rust targets a macOS build needs, installed if missing, under the step
# title both callers printed.
install_rust_targets() {
  say "rust targets"
  local installed target
  installed=$(rustup target list --installed)
  for target in "$@"; do
    grep -qx "$target" <<<"$installed" || rustup target add "$target"
  done
}

# The pnpm the package manager field pins, when the caller's is not that one,
# and the environment a web build runs in: the shim first on PATH and a
# reproducible timestamp. Sets the globals PNPM_VERSION and web_env.
pnpm_web_env() { # the exported ref (its package.json names the pnpm)
  PNPM_VERSION=$(sed -n 's/.*"packageManager": *"pnpm@\([^"]*\)".*/\1/p' "$1/package.json")
  mkdir -p "$WORK/shims"
  if ! command -v pnpm >/dev/null || [ "$(pnpm --version 2>/dev/null)" != "$PNPM_VERSION" ]; then
    printf '#!/bin/sh\nexec npx -y pnpm@%s "$@"\n' "$PNPM_VERSION" > "$WORK/shims/pnpm"
    chmod +x "$WORK/shims/pnpm"
  fi
  web_env=(env PATH="$WORK/shims:$PATH" SOURCE_DATE_EPOCH="$COMMIT_TIME")
}
