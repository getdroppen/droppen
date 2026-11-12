#!/usr/bin/env bash
# Start the Linux app's bundles as their users would, and check that each comes
# up and goes down again: desktop/build-linux.sh's .deb and AppImage, in Docker
# on the distributions the app is for, and the AppImage once more on this
# machine, through FUSE.
#
#   .github/scripts/smoke-desktop-linux.sh <dir> [--host]
#
# <dir>    desktop/build-linux.sh's --out: Droppen-<version>-<arch>.AppImage and
#          droppen_<version>_<amd64|arm64>.deb, of this machine's architecture
# --host   also the AppImage here, as a user starts it: mounted through FUSE,
#          with a scratch home (needs fusermount3, Xvfb's xvfb-run,
#          dbus-run-session, setsid, pgrep and curl; the runners of
#          .github/workflows/desktop-linux.yml)
#
# In Ubuntu 22.04 (the oldest the app is for), Debian 12 and Ubuntu 24.04, each
# a fresh container of its image by digest (IMAGES below):
#   the .deb       installed by apt, with what it depends on; its desktop entry
#                  passes desktop-file-validate
#   the AppImage   unpacked and run by its own runtime (APPIMAGE_EXTRACT_AND_RUN:
#                  a container has no FUSE), with the system libraries
#                  linuxdeploy leaves to the system installed, as a desktop has
#                  them
# Every start is a user's, under Xvfb and a D-Bus session of its own: the app
# must start its server and log in to it, the server must answer
# /api/auth/status as the desktop app's, and on SIGTERM the app must stop the
# server gracefully and exit 0, with no server left running.
set -euo pipefail

IMAGES="ubuntu:22.04@sha256:74b12b11ffa550fba1929ad4d8d16533c34f6612728469671afba1b03b66881c
debian:12@sha256:268fb24c39bca92d326dd83ea3abebe4a3f7751c804ffef23bb6d857dd343ff8
ubuntu:24.04@sha256:2198e4c65076e9cb0b3d695219df667c0e3b0072da64ca3c2648aca17dcecaf5"
# How long the app may take to log in to its server, and to quit.
UP_SECONDS=${DROPPEN_SMOKE_UP_SECONDS:-120}
DOWN_SECONDS=30

die() { printf 'smoke-desktop-linux: %s\n' "$*" >&2; exit 1; }
say() { printf '\n== %s\n' "$*"; }

# One start of the app (the command), as the user this runs as: up, logged in,
# answering, then down on SIGTERM. Its output is in $LOG. It runs in a session
# of its own ($APP_SESSION, for the cleanup), so the processes it looks for are
# its own, whoever else runs Droppen here.
APP_SESSION=""
start_app() { # command...
  local log=${LOG:?} pid port app status i
  setsid xvfb-run -a dbus-run-session -- "$@" >"$log" 2>&1 &
  pid=$!
  APP_SESSION=$pid
  port=""
  for ((i = 0; i < UP_SECONDS; i++)); do
    port=$(sed -n 's/.* logged in as droppen-app on port \([0-9][0-9]*\);.*/\1/p' "$log" | head -n 1)
    [ -z "$port" ] || break
    kill -0 "$pid" 2>/dev/null || { cat "$log" >&2; die "the app exited before it logged in to its server"; }
    sleep 1
  done
  [ -n "$port" ] || { cat "$log" >&2; die "the app did not log in to its server within ${UP_SECONDS}s"; }
  curl -fsS --max-time 10 "http://127.0.0.1:$port/api/auth/status" | grep '"desktopApp":true' >/dev/null \
    || { cat "$log" >&2; die "the server on port $port does not answer /api/auth/status as the desktop app's"; }
  echo "ok: up in ${i}s, its server answering on port $port"
  if [ "${MOUNTED:-0}" = 1 ]; then
    grep -q '/\.mount_Droppe' /proc/mounts || die "the AppImage is not mounted (no /tmp/.mount_Droppe… in /proc/mounts)"
    echo "ok: the AppImage runs mounted through FUSE"
  fi
  app=$(pgrep -s "$pid" -x Droppen) || die "no Droppen process"
  [ "$(printf '%s\n' "$app" | wc -l | tr -d " ")" = 1 ] || die "more than one Droppen process: $(echo $app)"
  pgrep -s "$pid" -x droppen-server >/dev/null || die "no droppen-server process"
  kill -TERM "$app"
  for ((i = 0; i < DOWN_SECONDS; i++)); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
  done
  kill -0 "$pid" 2>/dev/null && { cat "$log" >&2; die "the app did not quit within ${DOWN_SECONDS}s of SIGTERM"; }
  status=0
  wait "$pid" || status=$?
  [ "$status" = 0 ] || { cat "$log" >&2; die "the app exited with $status on SIGTERM"; }
  grep -q 'server stopped at quit: Gracefully' "$log" || { cat "$log" >&2; die "the app did not stop its server gracefully"; }
  if pgrep -s "$pid" -x droppen-server >/dev/null; then die "a droppen-server is still running after the app quit"; fi
  APP_SESSION=""
  echo "ok: down on SIGTERM, its server stopped gracefully"
}

# In a container, as root: what the bundle needs, a user, and one start as it.
in_container() { # deb|appimage
  local kind=$1 os
  os=$(. /etc/os-release && echo "$ID $VERSION_ID")
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq >/dev/null
  # The session: Xvfb, D-Bus, and what this script runs.
  local base=(xvfb xauth dbus procps curl ca-certificates)
  if [ "$kind" = deb ]; then
    cp /in/droppen_*.deb /tmp/
    apt-get install -y -qq --no-install-recommends "${base[@]}" desktop-file-utils /tmp/droppen_*.deb >/tmp/apt.log 2>&1 \
      || { tail -n 20 /tmp/apt.log >&2; die "$os: apt cannot install the .deb"; }
    desktop-file-validate /usr/share/applications/Droppen.desktop || die "$os: the .deb's desktop entry does not validate"
    echo "ok: $os: the .deb installs ($(dpkg-query -W -f='${Version}' droppen)), and its desktop entry validates"
    set -- /usr/bin/Droppen
  else
    # What linuxdeploy's excludelist leaves to the system: the graphics
    # drivers' libraries, fontconfig and what text rendering needs, the C++
    # runtime, and a font.
    apt-get install -y -qq --no-install-recommends "${base[@]}" \
      libgl1 libegl1 libgles2 libgbm1 libfontconfig1 libfreetype6 libharfbuzz0b libfribidi0 \
      libwayland-client0 libx11-xcb1 libxcb1 libexpat1 libgmp10 libgpg-error0 libstdc++6 fonts-dejavu-core \
      >/tmp/apt.log 2>&1 || { tail -n 20 /tmp/apt.log >&2; die "$os: apt cannot install the AppImage's system libraries"; }
    set -- env APPIMAGE_EXTRACT_AND_RUN=1 /in/Droppen-*.AppImage
  fi
  useradd -m smoke
  runuser -u smoke -- env HOME=/home/smoke LOG=/home/smoke/app.log DROPPEN_SMOKE_UP_SECONDS="$UP_SECONDS" \
    bash /smoke/smoke-desktop-linux.sh --start "$@"
  echo "ok: $os: the $kind"
}

case "${1:-}" in
  --start) shift; start_app "$@"; exit 0 ;;
  --in-container) in_container "$2"; exit 0 ;;
esac

DIR=${1:?usage: .github/scripts/smoke-desktop-linux.sh <dir> [--host]}
HOST=0
case "${2:-}" in --host) HOST=1 ;; "") ;; *) die "unknown argument: $2" ;; esac
case "$(uname -m)" in
  x86_64) ARCH=x86_64 DEBARCH=amd64 PLATFORM=linux/amd64 ;;
  aarch64 | arm64) ARCH=aarch64 DEBARCH=arm64 PLATFORM=linux/arm64 ;;
  *) die "no Linux app for $(uname -m)" ;;
esac
DIR=$(cd "$DIR" && pwd -P)
APPIMAGE=$(cd "$DIR" && ls Droppen-*-"$ARCH".AppImage 2>/dev/null) || die "no Droppen-<version>-$ARCH.AppImage in $DIR"
DEB=$(cd "$DIR" && ls droppen_*_"$DEBARCH".deb 2>/dev/null) || die "no droppen_<version>_$DEBARCH.deb in $DIR"
[ "$(printf '%s\n' "$APPIMAGE" | wc -l | tr -d " ")" = 1 ] && [ "$(printf '%s\n' "$DEB" | wc -l | tr -d " ")" = 1 ] || die "more than one AppImage or .deb in $DIR"
HERE=$(cd "$(dirname "$0")" && pwd -P)

# Each bundle alone in its container's /in, read-only.
IN=$(mktemp -d "${TMPDIR:-/tmp}/droppen-smoke.XXXXXX")
CONTAINER=""
cleanup() {
  [ -z "$CONTAINER" ] || docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  [ -z "$APP_SESSION" ] || pkill -KILL -s "$APP_SESSION" 2>/dev/null || true
  rm -rf "$IN"
}
trap cleanup EXIT
mkdir -p "$IN/deb" "$IN/appimage"
cp "$DIR/$DEB" "$IN/deb/"
cp "$DIR/$APPIMAGE" "$IN/appimage/"
chmod 755 "$IN/appimage/$APPIMAGE"
for image in $IMAGES; do
  for kind in deb appimage; do
    say "$kind on ${image%@*}"
    CONTAINER=droppen-smoke-$kind-$(printf '%s' "${image%@*}" | tr ':.' '--')-$$
    docker run --rm --name "$CONTAINER" --platform "$PLATFORM" -v "$IN/$kind:/in:ro" -v "$HERE:/smoke:ro" "$image" \
      bash /smoke/smoke-desktop-linux.sh --in-container "$kind"
    CONTAINER=""
  done
done

if [ "$HOST" = 1 ]; then
  say "the AppImage here, through FUSE"
  for tool in fusermount3 xvfb-run dbus-run-session setsid pgrep curl; do command -v "$tool" >/dev/null || die "--host needs $tool"; done
  mkdir -p "$IN/home"
  LOG=$IN/host.log MOUNTED=1 start_app env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME \
    HOME="$IN/home" "$IN/appimage/$APPIMAGE"
fi
say "smoke: the .deb and the AppImage of $ARCH start, serve and stop"
