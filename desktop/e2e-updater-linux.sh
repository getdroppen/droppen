#!/usr/bin/env bash
# End-to-end check of the AppImage's quiet updater on Linux, with two
# end-to-end AppImages (feature e2e, src-tauri/tauri.e2e.conf.json) of
# different versions, named as a release names them:
#
#   desktop/e2e-updater-linux.sh <dir>/Droppen-0.0.1-test-<arch>.AppImage <dir>/Droppen-0.0.2-test-<arch>.AppImage [<log dir>]
#
# <log dir> receives the app's logs and what it printed (default
# /tmp/desktop-updater-linux-review).
#
# desktop/build-linux.sh makes release AppImages only. An end-to-end one is
# the same build in its image (desktop/linux/Dockerfile, the bundler's tools
# as desktop/linux/build-in-container.sh puts them) with
#   cargo tauri build --bundles appimage --features e2e --config tauri.e2e.conf.json --config '{"version": "<v>"}'
#
# It runs on Linux as a user, not root (root may write anywhere, and step 1
# needs a folder it cannot), on a desktop or in a container that can mount
# FUSE (docker run --device /dev/fuse --cap-add SYS_ADMIN), with a throwaway
# HOME: an end-to-end build refuses the account's own, and the run never
# touches it, its autostart entry or any other Droppen. The run has an X
# display and a D-Bus session of its own (Xvfb, dbus-daemon), which outlive
# each launch as a desktop's session does: a restart starts the new version
# in them. Each launch is a session of its own (setsid), and every process the
# run stops is in one of those, or is its own display, bus or http server. A
# throwaway updater key is made for the run, never the release key, and the
# update is served from a local http server.
#
# Needs: python3, curl, fusermount3, Xvfb, dbus-daemon, setsid, pkill,
# sha256sum; cargo-tauri for the keys (DROPPEN_TOOLS_DIR/bin, as
# .github/scripts/fetch-release-tool.sh puts it); droppen-update-signer and
# droppen-update-manifest built for this machine (DROPPEN_UPDATE_SIGNER and
# DROPPEN_UPDATE_MANIFEST, or cargo builds them here).
#
# What it proves, the old AppImage installed in a scratch Apps folder:
#
#   1. where it cannot update, it says so and never checks: in a folder this
#      user cannot write, the menu says to move it; run without its mount
#      (APPIMAGE_EXTRACT_AND_RUN), it does not update;
#   2. shortly after launch it checks by itself and downloads its own
#      architecture's AppImage (linux-<arch>-appimage): a tampered one is
#      thrown away, the menu says the signature failed, nothing else changes;
#      asked, one signed for another version is thrown away too;
#   3. a genuinely signed file that is not an AppImage downloads and
#      verifies, and is refused at the quit: the AppImage is as it was;
#   4. the genuine one is ready (Restart to Update to <new>, the tray dot,
#      the download kept); a plain quit writes it over the old AppImage under
#      its name, and the next launch is the new version, up to date;
#   5. the web app's entry restarts into the update (a request without the
#      launch's token is ignored), and the new version starts by itself with
#      no path into the old one's mount in its environment.
set -euo pipefail

OLD=${1:?usage: e2e-updater-linux.sh <Droppen-<old>-<arch>.AppImage> <Droppen-<new>-<arch>.AppImage>}
NEW=${2:?usage: e2e-updater-linux.sh <Droppen-<old>-<arch>.AppImage> <Droppen-<new>-<arch>.AppImage>}
LOGS=${3:-/tmp/desktop-updater-linux-review}
DESKTOP=$(cd "$(dirname "$0")" && pwd -P)
die() { printf 'e2e-updater-linux: %s\n' "$*" >&2; exit 2; }
[ "$(uname -s)" = Linux ] || die "this runs on Linux"
[ "$(id -u)" != 0 ] || die "run it as a user, not root: root writes into any folder, so step 1 would prove nothing"
for tool in python3 curl fusermount3 Xvfb dbus-daemon setsid pkill sha256sum; do
  command -v "$tool" >/dev/null || die "missing $tool"
done
ARCH=$(uname -m)
case "$ARCH" in x86_64) OTHER_ARCH=aarch64 ;; aarch64) OTHER_ARCH=x86_64 ;; *) die "no AppImage for $ARCH" ;; esac
version_of() { # the version a release name gives an AppImage of this architecture
  local name
  name=$(basename "$1")
  [[ "$name" =~ ^Droppen-([0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?)-$ARCH\.AppImage$ ]] || die "$1 is not named Droppen-<version>-$ARCH.AppImage"
  printf '%s\n' "${BASH_REMATCH[1]}"
}
OLD_V=$(version_of "$OLD")
NEW_V=$(version_of "$NEW")
[ "$OLD_V" != "$NEW_V" ] || die "both AppImages are $OLD_V"
OLD=$(cd "$(dirname "$OLD")" && pwd -P)/$(basename "$OLD")
NEW=$(cd "$(dirname "$NEW")" && pwd -P)/$(basename "$NEW")
TAURI=${DROPPEN_TOOLS_DIR:-$HOME/.cache/droppen-desktop/tools}/bin/cargo-tauri
[ -x "$TAURI" ] || die "no $TAURI (.github/scripts/fetch-release-tool.sh tauri-cli <dir> $ARCH-unknown-linux-gnu)"

ROOT=$(cd "$(mktemp -d /tmp/droppen-upd-linux.XXXXXX)" && pwd -P)
HOME_DIR=$ROOT/home
CTL=$ROOT/ctl
APPS=$ROOT/Apps
SERVE=$ROOT/serve
KEYS=$ROOT/keys
SUPPORT=$HOME_DIR/.local/share/app.droppen.desktop.e2e
LOG=$HOME_DIR/.local/state/app.droppen.desktop.e2e/logs/desktop.log
INSTALLED=$APPS/Droppen.AppImage
mkdir -p "$HOME_DIR" "$APPS" "$SERVE" "$KEYS" "$ROOT/bin" "$ROOT/look"
mkdir -m 700 "$CTL"
FAILED=0
SESSIONS=""
HTTP_PID=""
XVFB_PID=""
BUS_PID=""
APP_PID=""
# e2e-lib.sh's command ids, where util-linux's uuidgen is not installed.
command -v uuidgen >/dev/null || uuidgen() { cat /proc/sys/kernel/random/uuid; }
# shellcheck source=e2e-lib.sh
. "$DESKTOP/e2e-lib.sh"

cleanup() {
  local sid pid
  # Only the sessions of this run's launches (each one's own setsid), and its
  # http server; then whatever extraction step 1 left in /tmp under its name.
  for sid in $SESSIONS; do pkill -TERM -s "$sid" 2>/dev/null || true; done
  sleep 2
  for sid in $SESSIONS; do pkill -KILL -s "$sid" 2>/dev/null || true; done
  for pid in "$HTTP_PID" "$BUS_PID" "$XVFB_PID"; do
    if alive "$pid"; then kill -TERM "$pid" 2>/dev/null || true; fi
  done
  chmod -R u+w "$ROOT" 2>/dev/null || true
  if [ -n "${EXTRACTED:-}" ] && [ -d "$EXTRACTED" ] && [ "$(stat -c %u "$EXTRACTED")" = "$(id -u)" ]; then rm -rf "$EXTRACTED"; fi
  mkdir -p "$LOGS"
  cp "$LOG" "$(dirname "$LOG")/server.log" "$ROOT/stdout.log" "$LOGS/" 2>/dev/null || true
  if [ "${KEEP:-0}" != 1 ]; then rm -rf "$ROOT"; else echo "kept $ROOT"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# A field of a JSON object by dotted path; booleans as true/false, missing as "".
jget() {
  python3 -c 'import json,sys
v=json.loads(sys.argv[1])
for k in sys.argv[2].split("."):
    v=v.get(k) if isinstance(v,dict) else None
print("" if v is None else ("true" if v is True else "false" if v is False else v if isinstance(v,str) else json.dumps(v)))' "$1" "$2"
}
field() { jget "$(state)" "$1"; }
menu_text() { field updateMenu.text; }
wait_for() { # description, condition, seconds
  local deadline=$((SECONDS + ${3:-60}))
  while [ $SECONDS -lt $deadline ]; do
    if eval "$2"; then return 0; fi
    sleep 0.5
  done
  echo "timed out waiting for $1" >&2
  echo "  log: $(tail -3 "$LOG" 2>/dev/null | tr '\n' ' ')" >&2
  return 1
}
sha() { sha256sum "$1" | cut -d' ' -f1; }

PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
ENDPOINT="http://127.0.0.1:$PORT/latest.json"

# The run's display and session bus, as a desktop session has them.
Xvfb -displayfd 3 -nolisten tcp -screen 0 1280x800x24 3>"$ROOT/display" >"$ROOT/xvfb.log" 2>&1 &
XVFB_PID=$!
wait_for "the display" '[ -s "$ROOT/display" ]' 20
DISPLAY_NAME=:$(cat "$ROOT/display")
dbus-daemon --session --fork --nopidfile --print-address=3 --print-pid=4 3>"$ROOT/bus" 4>"$ROOT/bus.pid"
BUS=$(head -1 "$ROOT/bus")
BUS_PID=$(head -1 "$ROOT/bus.pid")

launch() { # the AppImage $1, window closed, in a session of its own; [extra environment...]
  local image=$1 sid
  shift
  rm -f "$SUPPORT/app.lock"
  setsid env -i HOME="$HOME_DIR" USER="$(id -un)" LOGNAME="$(id -un)" SHELL=/bin/sh LANG=C.UTF-8 \
    PATH=/usr/local/bin:/usr/bin:/bin DISPLAY="$DISPLAY_NAME" DBUS_SESSION_BUS_ADDRESS="$BUS" \
    DROPPEN_DESKTOP_KEYCHAIN=0 DROPPEN_DESKTOP_TEST_CONTROL="$CTL" \
    DROPPEN_DESKTOP_UPDATE_ENDPOINT="$ENDPOINT" DROPPEN_DESKTOP_UPDATE_PUBKEY="$(cat "$KEYS/test.key.pub")" "$@" \
    "$image" --autostart >>"$ROOT/stdout.log" 2>&1 &
  sid=$!
  SESSIONS="$SESSIONS $sid"
  wait_for "the app" 'APP_PID=$(cat "$SUPPORT/app.lock" 2>/dev/null) && alive "$APP_PID" && [ "$(jget "$(state)" version)" != "" ]' 90 || true
  APP_PID=$(cat "$SUPPORT/app.lock" 2>/dev/null || true)
}
quit_app() { # quit and wait for the app to go
  local pid=$APP_PID
  command quit >/dev/null
  wait_for "the app to quit" '! alive "$pid"' 60 || true
}
mount_of() { { tr '\0' '\n' <"/proc/$1/environ"; } 2>/dev/null | sed -n 's/^APPDIR=//p'; }
# Put $1 where the app is installed as a person would: beside it, then renamed
# over it. The AppImage that just quit may still be busy (its runtime goes
# a moment after the app), and a busy program cannot be written into.
install_appimage() { cp "$1" "$INSTALLED.new" && mv -f "$INSTALLED.new" "$INSTALLED"; }

say "setup: a throwaway updater key; the new AppImage signed with it, beside its stand-ins"
SIGNER=${DROPPEN_UPDATE_SIGNER:-}
MANIFEST_TOOL=${DROPPEN_UPDATE_MANIFEST:-}
if [ -z "$SIGNER" ]; then
  cargo build --quiet --release --locked --manifest-path "$DESKTOP/update-signer/Cargo.toml"
  SIGNER="${CARGO_TARGET_DIR:-$DESKTOP/update-signer/target}/release/droppen-update-signer"
fi
if [ -z "$MANIFEST_TOOL" ]; then
  cargo build --quiet --release --locked --manifest-path "$DESKTOP/Cargo.toml" -p droppen-desktop-core --bin droppen-update-manifest
  MANIFEST_TOOL="${CARGO_TARGET_DIR:-$DESKTOP/target}/release/droppen-update-manifest"
fi
CI=true "$TAURI" signer generate --ci -w "$KEYS/test.key" >/dev/null 2>&1
chmod 600 "$KEYS/test.key"
sign() { # file version
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" "$SIGNER" sign "$1" --version "$2" --key-file "$KEYS/test.key" >/dev/null
}
# What `write` needs beside this architecture's AppImage: the Mac archive and
# the other architecture's AppImage, stand-ins signed with the same key.
stand_ins() { # dir
  printf 'a stand-in Mac archive\n' >"$1/Droppen.app.tar.gz"
  printf '\177ELF\002\001\001\000AI\002 a stand-in for the %s AppImage\n' "$OTHER_ARCH" >"$1/Droppen-$NEW_V-$OTHER_ARCH.AppImage"
  sign "$1/Droppen.app.tar.gz" "$NEW_V"
  sign "$1/Droppen-$NEW_V-$OTHER_ARCH.AppImage" "$NEW_V"
}
write_manifest() { # dir (served as http://…/<dir>/), out
  local dir=$1 name
  name=$(basename "$dir")
  "$MANIFEST_TOOL" write --version "$NEW_V" --archive "$dir/Droppen.app.tar.gz" --signature "$dir/Droppen.app.tar.gz.sig" \
    --pubkey "$KEYS/test.key.pub" --url "http://127.0.0.1:$PORT/$name/Droppen.app.tar.gz" \
    --notes "https://github.com/getdroppen/droppen/releases/tag/v$NEW_V" --loopback-http \
    --linux "$ARCH=$dir/Droppen-$NEW_V-$ARCH.AppImage,$dir/Droppen-$NEW_V-$ARCH.AppImage.sig" \
    --linux "$OTHER_ARCH=$dir/Droppen-$NEW_V-$OTHER_ARCH.AppImage,$dir/Droppen-$NEW_V-$OTHER_ARCH.AppImage.sig" \
    --out "$2" >/dev/null
}
mkdir -p "$SERVE/good" "$SERVE/tampered" "$SERVE/broken"
cp "$NEW" "$SERVE/good/Droppen-$NEW_V-$ARCH.AppImage"
sign "$SERVE/good/Droppen-$NEW_V-$ARCH.AppImage" "$NEW_V"
stand_ins "$SERVE/good"
write_manifest "$SERVE/good" "$ROOT/good.json"
# The same AppImage, one byte changed after it was signed.
cp "$SERVE/good"/* "$SERVE/tampered/"
python3 -c 'import sys
p=sys.argv[1]; b=bytearray(open(p,"rb").read()); b[-100]^=1; open(p,"wb").write(b)' "$SERVE/tampered/Droppen-$NEW_V-$ARCH.AppImage"
sed "s#/good/#/tampered/#g" "$ROOT/good.json" >"$ROOT/tampered.json"
# The genuine AppImage, its signature made for another version.
mkdir -p "$ROOT/other-version"
cp "$NEW" "$ROOT/other-version/Droppen-$NEW_V-$ARCH.AppImage"
sign "$ROOT/other-version/Droppen-$NEW_V-$ARCH.AppImage" 0.0.9-test
python3 -c 'import json,sys
m=json.load(open(sys.argv[1])); m["platforms"]["linux-"+sys.argv[2]+"-appimage"]["signature"]=open(sys.argv[3]).read().strip()
json.dump(m,open(sys.argv[4],"w"),indent=2)' "$ROOT/good.json" "$ARCH" "$ROOT/other-version/Droppen-$NEW_V-$ARCH.AppImage.sig" "$ROOT/other-version.json"
# A program, genuinely signed under the AppImage's name, that is not an AppImage.
(cd "$ROOT/look" && "$NEW" --appimage-extract 'usr/bin/*' >/dev/null)
PROGRAM=$(find "$ROOT/look/squashfs-root/usr/bin" -type f -name 'droppen-server*' | head -1)
[ -n "$PROGRAM" ] || die "no droppen-server in $NEW"
LC_ALL=C grep -aqF DROPPEN_DESKTOP_UPDATE_ENDPOINT "$ROOT/look/squashfs-root/usr/bin"/* || die "$NEW is not an end-to-end build with the updater"
cp "$PROGRAM" "$SERVE/broken/Droppen-$NEW_V-$ARCH.AppImage"
sign "$SERVE/broken/Droppen-$NEW_V-$ARCH.AppImage" "$NEW_V"
stand_ins "$SERVE/broken"
write_manifest "$SERVE/broken" "$ROOT/broken.json"
serve() { cp "$ROOT/$1.json" "$SERVE/latest.json"; }
# The update server: plain http on loopback, the AppImage sent slowly enough
# (a few seconds) for the menu's progress to be seen.
cat >"$ROOT/bin/serve.py" <<'PY'
import functools, http.server, sys, time
class Slow(http.server.SimpleHTTPRequestHandler):
    def copyfile(self, source, out):
        while True:
            chunk = source.read(1024 * 1024)
            if not chunk:
                break
            out.write(chunk)
            if self.path.endswith(".AppImage"):
                time.sleep(0.05)
    def log_message(self, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), functools.partial(Slow, directory=sys.argv[2])).serve_forever()
PY
python3 -I "$ROOT/bin/serve.py" "$PORT" "$SERVE" &
HTTP_PID=$!
wait_for "the update server" 'curl -fs -o /dev/null "http://127.0.0.1:$PORT/"' 10
check "the manifest offers this architecture's AppImage beside the Mac's archive, never a bare linux-$ARCH" \
  'python3 -c "import json,sys; m=json.load(open(sys.argv[1])); p=m[\"platforms\"]; assert sorted(p)==sorted([\"darwin-aarch64\",\"darwin-x86_64\",\"linux-aarch64-appimage\",\"linux-x86_64-appimage\"]), p; assert p[\"linux-\"+sys.argv[2]+\"-appimage\"][\"url\"].endswith(\"/good/Droppen-\"+sys.argv[3]+\"-\"+sys.argv[2]+\".AppImage\")" "$ROOT/good.json" "$ARCH" "$NEW_V"'
serve tampered

say "1. where it cannot update, it says so, and never checks"
mkdir -p "$ROOT/locked"
cp "$OLD" "$ROOT/locked/Droppen.AppImage"
chmod 555 "$ROOT/locked"
launch "$ROOT/locked/Droppen.AppImage"
STATE=$(state)
echo "menu: $(jget "$STATE" updateMenu.text)"
check "in a folder this user cannot write: the menu says so, disabled, and updates are off" \
  '[ "$(jget "$STATE" update.place)" = notWritable ] && [ "$(jget "$STATE" updateMenu.text)" = "Move Droppen to a Folder You Can Write to Update" ] && [ "$(jget "$STATE" updateMenu.enabled)" = false ] && grep -q "updates: off (NotWritable)" "$LOG"'
quit_app
chmod 755 "$ROOT/locked"
launch "$ROOT/locked/Droppen.AppImage" APPIMAGE_EXTRACT_AND_RUN=1
EXTRACTED=$(dirname "$(dirname "$(dirname "$(readlink "/proc/$APP_PID/exe")")")")
STATE=$(state)
check "run without its mount (extracted): updates are off" \
  '[ "$(jget "$STATE" update.place)" = notABundle ] && [ "$(jget "$STATE" updateMenu.enabled)" = false ] && grep -q "not run from its mount" "$LOG" && grep -q "updates: off (NotABundle)" "$LOG"'
quit_app
check "neither checked for an update" '[ "$(count "updates: checking")" = 0 ]'

say "2. on its own, shortly after launch: a tampered AppImage is thrown away, quietly"
install_appimage "$OLD"
OLD_SHA=$(sha "$INSTALLED")
launch "$INSTALLED"
STATE=$(state)
check "running $OLD_V from its mount, updates on" \
  '[ "$(jget "$STATE" version)" = "$OLD_V" ] && [ "$(jget "$STATE" update.place)" = updatable ] && grep -q "updates: on for Droppen $OLD_V" "$LOG" && grep -q "the AppImage $INSTALLED, mounted, replaceable here" "$LOG"'
wait_for "the check of its own and the download" 'grep -q "was not kept (Signature)" "$LOG"' 90 || true
STATE=$(state)
echo "menu: $(jget "$STATE" updateMenu.text)"
check "checked by itself, downloaded this architecture's AppImage, refused the signature" \
  'grep -q "updates: checking$" "$LOG" && grep -q "updates: downloading Droppen $NEW_V from http://127.0.0.1:$PORT/tampered/Droppen-$NEW_V-$ARCH.AppImage" "$LOG" && grep -q "was not kept (Signature)" "$LOG"'
check "nothing kept, no dot, the menu says the signature failed; the AppImage as it was" \
  '[ "$(jget "$STATE" updateMenu.staged)" = false ] && [ "$(jget "$STATE" updateMenu.dot)" = false ] && [ "$(jget "$STATE" update.problem)" = signature ] && [ "$(jget "$STATE" updateMenu.text)" = "Update Failed Its Signature Check and Was Discarded" ] && [ "$(sha "$INSTALLED")" = "$OLD_SHA" ]'
serve other-version
command update-check >/dev/null
wait_for "the second refusal" '[ "$(count "was not kept (Signature)")" -ge 2 ]' 60 || true
check "asked: one signed for another version is thrown away too" \
  'grep -q "was not kept (Signature).*0.0.9-test" "$LOG" && [ "$(field updateMenu.staged)" = false ] && [ "$(field update.problem)" = signature ]'
quit_app
check "a quit with nothing ready installs nothing" '[ "$(sha "$INSTALLED")" = "$OLD_SHA" ] && [ "$(count "updates: installed")" = 0 ]'

say "3. a signed file that is not an AppImage: refused at the quit"
serve broken
launch "$INSTALLED"
command update-check >/dev/null
wait_for "ready" '[ "$(field update.state)" = ready ]' 60 || true
check "it downloaded and verified" '[ "$(field updateMenu.staged)" = true ]'
quit_app
check "not installed at the quit: the AppImage is $OLD_V as it was" \
  'grep -q "updates: not installed (Install), Droppen is as it was: the update is not an AppImage" "$LOG" && [ "$(sha "$INSTALLED")" = "$OLD_SHA" ] && [ "$(ls "$APPS")" = Droppen.AppImage ]'

say "4. the genuine one: ready, then installed at a plain quit"
serve good
launch "$INSTALLED"
command update-check >/dev/null
MENUS=""
DEADLINE=$((SECONDS + 90))
while [ $SECONDS -lt $DEADLINE ]; do
  text=$(menu_text)
  case "$MENUS" in *"[$text]"*) ;; *) MENUS="$MENUS[$text]" ;; esac
  [ "$(field update.state)" != ready ] || break
  sleep 0.3
done
echo "the menu said: $MENUS"
STATE=$(state)
check "the menu showed the download's progress" 'echo "$MENUS" | grep -qE "[0-9]+%"'
check "ready: Restart to Update to $NEW_V, the dot, the download kept; nothing installed yet" \
  '[ "$(jget "$STATE" updateMenu.text)" = "Restart to Update to $NEW_V" ] && [ "$(jget "$STATE" updateMenu.enabled)" = true ] && [ "$(jget "$STATE" updateMenu.dot)" = true ] && [ "$(jget "$STATE" updateMenu.staged)" = true ] && [ "$(sha "$INSTALLED")" = "$OLD_SHA" ]'
quit_app
check "installed at the quit: the AppImage is now $NEW_V's, under its own name and mode, and nothing restarted" \
  'grep -q "updates: installed Droppen $NEW_V" "$LOG" && [ "$(sha "$INSTALLED")" = "$(sha "$NEW")" ] && [ "$(ls "$APPS")" = Droppen.AppImage ] && [ -x "$INSTALLED" ] && [ "$(count "started Droppen again")" = 0 ] && [ ! -e "$SUPPORT/update" ]'
launch "$INSTALLED"
command update-check >/dev/null
wait_for "the answer" 'menu_text | grep -qF "($NEW_V)"' 30 || true
STATE=$(state)
check "the next launch is $NEW_V, and asked, up to date" \
  '[ "$(jget "$STATE" version)" = "$NEW_V" ] && [ "$(jget "$STATE" updateMenu.text)" = "Up to Date ($NEW_V)" ]'
quit_app

say "5. the web app's entry restarts into the update"
install_appimage "$OLD"
launch "$INSTALLED"
check "the old version runs again" '[ "$(field version)" = "$OLD_V" ]'
command update-check >/dev/null
wait_for "ready" '[ "$(field update.state)" = ready ]' 90 || true
OLD_APP=$APP_PID
OLD_MOUNT=$(mount_of "$OLD_APP")
wait_for "the app's page" '[ "$(field phase)" = running ]' 90 || true
# Without a tray host an --autostart launch shows its window (tray.rs): the
# restart keeps the window as it was, whichever it is.
OLD_WINDOW=$(field windowVisible)
command 'js location.assign("droppen-desktop://update/restart?token=0000")' >/dev/null
sleep 1
check "a restart request without the launch's restart token is ignored" 'alive "$OLD_APP" && grep -q "without this launch.s restart token was ignored" "$LOG"'
command 'js window.__droppenDesktop.restartToUpdate()' >/dev/null
wait_for "the old app to go" '! alive "$OLD_APP"' 60 || true
wait_for "the new version" 'P=$(cat "$SUPPORT/app.lock" 2>/dev/null); [ -n "$P" ] && [ "$P" != "$OLD_APP" ] && alive "$P"' 60 || true
APP_PID=$(cat "$SUPPORT/app.lock" 2>/dev/null || true)
wait_for "it to answer" '[ "$(field version)" != "" ]' 60 || true
STATE=$(state)
NEW_MOUNT=$(mount_of "$APP_PID")
STALE=$({ tr '\0' '\n' <"/proc/$APP_PID/environ"; } 2>/dev/null | grep -F "$OLD_MOUNT" || true)
echo "running $(jget "$STATE" version) (pid $APP_PID, window visible: $(jget "$STATE" windowVisible), before: $OLD_WINDOW), mounted at ${NEW_MOUNT:-?}; the old mount was ${OLD_MOUNT:-?}"
[ -z "$STALE" ] || printf 'its variables naming the old mount:\n%s\n' "$STALE"
check "restarted into $NEW_V by itself, its window as it was" \
  '[ "$(jget "$STATE" version)" = "$NEW_V" ] && grep -q "Droppen $NEW_V, started by its update" "$LOG" && [ "$(jget "$STATE" windowVisible)" = "$OLD_WINDOW" ] && [ "$(sha "$INSTALLED")" = "$(sha "$NEW")" ]'
check "from its own mount, with nothing of the old one's in its environment" \
  '[ -n "$OLD_MOUNT" ] && [ -n "$NEW_MOUNT" ] && [ "$NEW_MOUNT" != "$OLD_MOUNT" ] && [ -z "$STALE" ]'
# The relaunched app is in the old launch's session (the restart started it).
quit_app
check "quit: nothing of the run's still runs" '! alive "$APP_PID" && ! alive "$OLD_APP"'

echo
if [ "$FAILED" = 0 ]; then echo "ALL PASSED"; else echo "SOME CHECKS FAILED"; exit 1; fi
