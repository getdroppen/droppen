#!/usr/bin/env bash
# End-to-end check of the app's quiet updater on this Mac, with two end-to-end
# test builds of different versions (their server built from the same tree, so
# the web app has the update entry):
#
#   desktop/build.sh --ref HEAD --e2e --version 0.0.1-test --arch aarch64 --out /tmp/droppen-e2e-old [--sidecar …]
#   desktop/build.sh --ref HEAD --e2e --version 0.0.2-test --arch aarch64 --out /tmp/droppen-e2e-new --sidecar "/tmp/droppen-e2e-old/Droppen E2E.app/Contents/MacOS/remote-hand-server"
#   desktop/e2e-updater-macos.sh "/tmp/droppen-e2e-old/Droppen E2E.app" "/tmp/droppen-e2e-new/Droppen E2E.app" [<screenshot dir>]
#
# A throwaway updater key is made for the run (never the release key), the new
# build is packed and signed with it by updater-artifacts.sh --test-build, and
# the manifest is served from a local http server; a copy of the old build in
# a scratch "Applications" folder (a real path: the updater refuses one through
# a symbolic link, and /tmp is one) is the app that updates itself.
#
# It runs beside the real Droppen, which it never stops, moves or looks at: an
# end-to-end build is somebody else to macOS (bundle id app.droppen.desktop.e2e,
# "Droppen E2E", its own server instance and ports; e2e-identity.sh), refuses
# the real HOME, and the run gives it a throwaway one with the keychain off.
# Every process it stops it started, by pid, while that pid still runs from
# the run's folders; it never touches the real HOME, keychain, launch agents,
# /Applications or the real app's data. To stay out of the way of whoever uses
# the Mac, the app runs with its window closed (`--autostart`) except for the
# few seconds the web app's entry is photographed, and each app is quit as
# soon as its step is done.
#
# The menus and the web app speak $E2E_LANGUAGE (zh-Hans; any AppleLanguages
# value). What it proves:
#
#   1. updater-artifacts.sh refuses an app of another version than the
#      release's, a test version as a release, and a release served over http;
#   2. shortly after launch the app checks by itself and downloads in the
#      background; an archive signed by another key is thrown away, nothing
#      else changes, and the menu says so — no dialog, no window; the
#      first-run guide the first page opens by itself in a fresh home is gone
#      through there, the window still closed (the web app's update entry is
#      in the rail, which the guide's page has not);
#   3. an asked-for check downloads the genuine one: the menu shows the
#      progress, then "Restart to Update to …", the menu-bar icon its dot, and
#      the web app its entry at the bottom of the rail;
#   4. a plain quit installs it — the server stopped gracefully, a terminal left
#      open survives — and the next launch is the new version, up to date;
#      with the update server gone, an asked-for check says the network;
#   5. a signed archive that is not an app fails at the quit, leaving the app
#      as it was;
#   6. the web app's entry restarts into the update (a request without the
#      launch's nonce is ignored), and the new version starts by itself;
#
# and that every other Droppen app running on the Mac when it started (the
# real one, say) still runs, as the same process, when it ends.
set -euo pipefail

OLD=${1:?usage: e2e-updater-macos.sh <old Droppen E2E.app> <new Droppen E2E.app> [<screenshot dir>]}
NEW=${2:?usage: e2e-updater-macos.sh <old Droppen E2E.app> <new Droppen E2E.app> [<screenshot dir>]}
SHOTS=${3:-/tmp/desktop-updater-review}
E2E_LANGUAGE=${E2E_LANGUAGE:-zh-Hans}
DESKTOP=$(cd "$(dirname "$0")" && pwd -P)
# shellcheck source=e2e-identity.sh
. "$DESKTOP/e2e-identity.sh"
# shellcheck source=e2e-lib.sh
. "$DESKTOP/e2e-lib.sh"
for app in "$OLD" "$NEW"; do
  e2e_check_app "$app"
  LC_ALL=C grep -aqF DROPPEN_DESKTOP_UPDATE_ENDPOINT "$app/Contents/MacOS/Droppen" || { echo "$app is not an --e2e build with the updater" >&2; exit 2; }
done
OLD=$(cd "$OLD" && pwd -P)
NEW=$(cd "$NEW" && pwd -P)
OLD_V=$(plutil -extract CFBundleShortVersionString raw "$OLD/Contents/Info.plist")
NEW_V=$(plutil -extract CFBundleShortVersionString raw "$NEW/Contents/Info.plist")
[ "$OLD_V" != "$NEW_V" ] || { echo "both builds are $OLD_V" >&2; exit 2; }
e2e_check_scratch "$SHOTS" "the screenshot folder"
mkdir -p "$SHOTS"
TAURI=${DROPPEN_TOOLS_DIR:-$HOME/.cache/droppen-desktop/tools}/bin/cargo-tauri
[ -x "$TAURI" ] || { echo "no $TAURI (desktop/build.sh installs it)" >&2; exit 2; }
cargo build --quiet --release --locked --manifest-path "$DESKTOP/Cargo.toml" -p droppen-desktop-core --bin droppen-update-manifest
# Where cargo put it: CARGO_TARGET_DIR when set (relative to here, as cargo
# reads it), else the workspace's own target/.
MANIFEST_TOOL="${CARGO_TARGET_DIR:-$DESKTOP/target}/release/droppen-update-manifest"

# The Droppen apps already running (the real one, say): left alone, and
# checked at the end to be running still, as the same processes.
OTHERS_BEFORE=$(ps -axo pid=,comm= | awk '/\/Contents\/MacOS\/Droppen$/ { print $1 }' | sort -n | tr '\n' ' ')

e2e_lock
ROOT=$(cd "$(mktemp -d /tmp/droppen-upd.XXXXXX)" && pwd -P)
e2e_check_scratch "$ROOT" "the run's folder"
HOME_DIR="$ROOT/home"
CTL="$ROOT/ctl"
APPS="$ROOT/Applications"
SERVE="$ROOT/serve"
KEYS="$ROOT/keys"
SUPPORT="$HOME_DIR/Library/Application Support/$E2E_BUNDLE_ID"
e2e_check_socket_room "$SUPPORT"
LOG="$HOME_DIR/Library/Logs/$E2E_NAME/desktop.log"
INSTALLED="$APPS/$(basename "$OLD")"
BIN="$INSTALLED/Contents/MacOS/Droppen"
mkdir -p "$HOME_DIR" "$APPS" "$SERVE" "$KEYS" "$ROOT/bin"
mkdir -m 700 "$CTL"
APP_PID=""
HTTP_PID=""
FAILED=0
e2e_clean_bundle_dirs

ours() { e2e_pids_in "$ROOT"; }

cleanup() {
  local pid server
  # By pid only, and only while it runs from this run's folders: the app
  # running now (its lock names it), the http server, the app's server, and
  # anything else still running from them.
  pid=$(cat "$SUPPORT/app.lock" 2>/dev/null || true)
  for pid in "$APP_PID" "$pid"; do
    if alive "$pid"; then e2e_signal TERM "$pid" "$ROOT"; sleep 3; fi
    if alive "$pid"; then e2e_signal KILL "$pid" "$ROOT"; fi
  done
  if alive "$HTTP_PID"; then kill -TERM "$HTTP_PID" 2>/dev/null || true; fi
  server=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$SUPPORT/server.pid" 2>/dev/null || true)
  if alive "$server"; then e2e_signal TERM "$server" "$ROOT"; fi
  for pid in $(ours); do e2e_signal TERM "$pid" "$ROOT"; done
  e2e_unregister "$INSTALLED"
  e2e_clean_runtime_dirs "$HOME_DIR/.local/share/remote-hand/instances/$E2E_INSTANCE"
  e2e_clean_bundle_dirs
  e2e_unlock
  cp "$LOG" "$SHOTS/desktop.log" 2>/dev/null || true
  if [ "${KEEP:-0}" != 1 ]; then rm -rf "$ROOT"; else echo "kept $ROOT"; fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM

PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
ENDPOINT="http://127.0.0.1:$PORT/latest.json"

launch() { # the app at $BIN, window closed, with the update endpoint and the run's key
  env -i HOME="$HOME_DIR" USER="$USER" LOGNAME="$USER" SHELL=/bin/zsh LANG=en_US.UTF-8 \
    PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    DROPPEN_DESKTOP_KEYCHAIN=0 DROPPEN_DESKTOP_TEST_CONTROL="$CTL" \
    DROPPEN_DESKTOP_UPDATE_ENDPOINT="$ENDPOINT" DROPPEN_DESKTOP_UPDATE_PUBKEY="$(cat "$KEYS/test.key.pub")" \
    "$BIN" --autostart -AppleLanguages "($E2E_LANGUAGE)" >>"$ROOT/stdout.log" 2>&1 &
  APP_PID=$!
}

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
guide_at() { local page; page=$(read_page) && [ "$(jget "$page" guide)" = "$1" ]; }
# Through the first-run guide as e2e-macos.sh goes (which checks and
# photographs each step): Get started, step 01 on, "Only on this Mac", step
# 03 skipped, Finish — onto the session list.
finish_guide() {
  press "[data-testid=welcome-start]"
  wait_for "step 01" 'guide_at tools' 20 || return 1
  press "[data-testid=step-action]"
  wait_for "step 02" 'guide_at where' 20 || return 1
  press "[data-choice=local]"
  sleep 1
  press "[data-testid=step-action]"
  wait_for "step 03" 'guide_at first' 20 || return 1
  press "[data-testid=skip-step]"
  wait_for "step 04" 'guide_at wrap' 20 || return 1
  press "[data-testid=step-action]"
  wait_for "the session list" 'P=$(read_page) && [ "$(jget "$P" path)" = /sessions ]' 20
}
api_login() { # port -> a token
  curl -fsS -H 'Content-Type: application/json' -d "{\"username\":\"$(username)\",\"password\":\"$(password)\"}" \
    "http://127.0.0.1:$1/api/auth/login" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])'
}
api_logout() { curl -s -o /dev/null -X POST -H "Authorization: Bearer $2" "http://127.0.0.1:$1/api/auth/logout" || true; }
terminals() { # port token -> the ids of the live terminals
  curl -fsS -H "Authorization: Bearer $2" "http://127.0.0.1:$1/api/terminals" | python3 -c 'import json,sys; print(" ".join(t["id"] for t in json.load(sys.stdin)["terminals"]))'
}
bundle_version() { plutil -extract CFBundleShortVersionString raw "$INSTALLED/Contents/Info.plist"; }
serve() { # what to serve: good, other-key, broken
  rm -f "$SERVE/latest.json"
  case "$1" in
    good) cp "$ROOT/good/latest.json" "$SERVE/latest.json" ;;
    other-key) cp "$ROOT/other-key.json" "$SERVE/latest.json" ;;
    broken) cp "$ROOT/broken/latest.json" "$SERVE/latest.json" ;;
  esac
}
start_http() {
  python3 -I "$ROOT/bin/serve.py" "$PORT" "$SERVE" &
  HTTP_PID=$!
  wait_for "the update server" 'curl -fs -o /dev/null "http://127.0.0.1:$PORT/"' 10
}
started() { # wait for the app to run and the web app to load (past $1 probes)
  # A local: the condition is evaluated in wait_for, where $1 is its own.
  local loaded=$1
  wait_for "the app" '[ "$(field phase)" = running ] && [ "$(probes)" -gt "$loaded" ]' 90 || true
}
quit_app() { # quit and wait for the app to go; prints how long it took
  local began=$SECONDS pid=$APP_PID
  command quit >/dev/null
  wait_for "the app to quit" '! alive "$pid"' 60 || true
  echo $((SECONDS - began))
}

say "setup: a throwaway updater key; the new build packed and signed with it"
ditto "$OLD" "$INSTALLED"
for key in test other; do
  CI=true "$TAURI" signer generate --ci -w "$KEYS/$key.key" >/dev/null 2>&1
  chmod 600 "$KEYS/$key.key"
done
artifacts() { env -u TAURI_SIGNING_PRIVATE_KEY -u TAURI_SIGNING_PRIVATE_KEY_PASSWORD "$DESKTOP/updater-artifacts.sh" "$@"; }
artifacts "$NEW" "$NEW_V" --test-build --key "$KEYS/test.key" --pubkey "$KEYS/test.key.pub" \
  --base-url "http://127.0.0.1:$PORT/v$NEW_V" --out "$ROOT/good" >"$ROOT/artifacts.log" 2>&1 \
  || { cat "$ROOT/artifacts.log"; echo "updater-artifacts.sh failed" >&2; exit 1; }
mkdir -p "$SERVE/v$NEW_V" "$SERVE/broken" "$ROOT/other" "$ROOT/broken"
cp "$ROOT/good/Droppen.app.tar.gz" "$SERVE/v$NEW_V/"
# Another key's signature over the same archive.
cp "$ROOT/good/Droppen.app.tar.gz" "$ROOT/other/"
env TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" "$TAURI" signer sign -f "$KEYS/other.key" --app-version "$NEW_V" "$ROOT/other/Droppen.app.tar.gz" >/dev/null 2>&1
python3 -c 'import json,sys
m=json.load(open(sys.argv[1])); sig=open(sys.argv[2]).read().strip()
for p in m["platforms"].values(): p["signature"]=sig
json.dump(m,open(sys.argv[3],"w"),indent=2)' "$ROOT/good/latest.json" "$ROOT/other/Droppen.app.tar.gz.sig" "$ROOT/other-key.json"
# A genuinely signed archive that is not an app.
head -c 65536 /dev/urandom | gzip >"$SERVE/broken/Droppen.app.tar.gz"
env TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" "$TAURI" signer sign -f "$KEYS/test.key" --app-version "$NEW_V" "$SERVE/broken/Droppen.app.tar.gz" >/dev/null 2>&1
"$MANIFEST_TOOL" write --version "$NEW_V" --archive "$SERVE/broken/Droppen.app.tar.gz" \
  --signature "$SERVE/broken/Droppen.app.tar.gz.sig" --pubkey "$KEYS/test.key.pub" \
  --url "http://127.0.0.1:$PORT/broken/Droppen.app.tar.gz" --notes "https://github.com/getdroppen/droppen/releases/tag/v$NEW_V" \
  --out "$ROOT/broken/latest.json" --loopback-http >/dev/null
# The update server: plain http on loopback, the archive sent slowly enough
# (a few seconds) for the menu's progress to be seen.
cat >"$ROOT/bin/serve.py" <<'PY'
import functools, http.server, sys, time
class Slow(http.server.SimpleHTTPRequestHandler):
    def copyfile(self, source, out):
        while True:
            chunk = source.read(256 * 1024)
            if not chunk:
                break
            out.write(chunk)
            if self.path.endswith(".tar.gz"):
                time.sleep(0.03)
    def log_message(self, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), functools.partial(Slow, directory=sys.argv[2])).serve_forever()
PY
start_http
tail -2 "$ROOT/artifacts.log"

say "1. updater-artifacts.sh refuses what is not a release"
out=$(artifacts "$OLD" "$NEW_V" --test-build --key "$KEYS/test.key" --pubkey "$KEYS/test.key.pub" --base-url "http://127.0.0.1:$PORT" --out "$ROOT/x1" 2>&1) && code=0 || code=$?
check "an app of another version than the release's" '[ "$code" != 0 ] && echo "$out" | grep -q "the app is version $OLD_V, not $NEW_V"'
out=$(artifacts "$NEW" "$NEW_V" --key "$KEYS/test.key" --out "$ROOT/x2" 2>&1) && code=0 || code=$?
check "a test version as a release" '[ "$code" != 0 ] && echo "$out" | grep -q "a release version is x.y.z"'
out=$(artifacts "$NEW" "$NEW_V" --key "$KEYS/test.key" --base-url "http://127.0.0.1:$PORT" --out "$ROOT/x3" 2>&1) && code=0 || code=$?
check "a release served from anywhere but the release" '[ "$code" != 0 ] && echo "$out" | grep -q "for --test-build only"'
check "the manifest names one universal archive for both Mac targets" \
  'python3 -c "import json,sys; m=json.load(open(sys.argv[1])); assert sorted(m[\"platforms\"])==[\"darwin-aarch64\",\"darwin-x86_64\"] and len({p[\"url\"] for p in m[\"platforms\"].values()})==1 and m[\"version\"]==sys.argv[2]" "$ROOT/good/latest.json" "$NEW_V"'

say "2. on its own, shortly after launch: another key's archive is thrown away, quietly"
serve other-key
launch
started 0
STATE=$(state)
SERVER_PID=$(jget "$STATE" serverPid)
check "running $OLD_V, window closed, updates on" \
  '[ "$(jget "$STATE" version)" = "$OLD_V" ] && [ "$(jget "$STATE" windowVisible)" = false ] && grep -q "updates: on for Droppen $OLD_V" "$LOG"'
echo "menu: $(jget "$STATE" updateMenu.text) | $(jget "$STATE" updateMenu.notes)"
command "menu-snapshot $SHOTS/01-menu-check.png" >/dev/null
wait_for "the check of its own and the download" 'grep -q "was not kept (Signature)" "$LOG"' 60 || true
STATE=$(state)
echo "menu: $(jget "$STATE" updateMenu.text)"
command "menu-snapshot $SHOTS/02-menu-signature-refused.png" >/dev/null
check "checked by itself, downloaded, refused the signature" \
  'grep -q "updates: checking$" "$LOG" && grep -q "updates: downloading Droppen $NEW_V" "$LOG" && grep -q "was not kept (Signature)" "$LOG"'
check "nothing kept, no dot, the menu says the signature failed" \
  '[ "$(jget "$STATE" updateMenu.staged)" = false ] && [ "$(jget "$STATE" updateMenu.dot)" = false ] && [ "$(jget "$STATE" update.problem)" = signature ] && [ "$(jget "$STATE" updateMenu.enabled)" = true ]'
check "no window, the app and its server as they were" \
  '[ "$(jget "$STATE" windowVisible)" = false ] && alive "$APP_PID" && [ "$(jget "$STATE" serverPid)" = "$SERVER_PID" ] && [ "$(bundle_version)" = "$OLD_V" ]'
check "the first page is the first-run guide, opened by itself in a fresh home" \
  'P=$(loaded_page) && [ "$(jget "$P" path)" = /welcome ] && [ "$(jget "$P" guide)" = welcome ]'
finish_guide || true
PORT_APP=$(field port)
TOKEN=$(api_login "$PORT_APP")
PROGRESS=$(curl -fsS -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:$PORT_APP/api/setup/progress" || true)
api_logout "$PORT_APP" "$TOKEN"
check "gone through with the window closed: the session list, the guide kept finished" \
  'P=$(read_page) && [ "$(jget "$P" path)" = /sessions ] && [ -z "$(jget "$P" guide)" ] && [ -n "$(jget "$PROGRESS" finishedAt)" ] && [ "$(jget "$PROGRESS" autoOpen)" = false ] && [ "$(field windowVisible)" = false ]'

say "3. asked: the genuine archive downloads; ready, quietly"
serve good
command update-check >/dev/null
MENUS=""
DEADLINE=$((SECONDS + 60))
SNAPPED=0
while [ $SECONDS -lt $DEADLINE ]; do
  text=$(menu_text)
  case "$MENUS" in *"[$text]"*) ;; *) MENUS="$MENUS[$text]" ;; esac
  if [ "$SNAPPED" = 0 ] && echo "$text" | grep -qE '[0-9]+%'; then
    command "menu-snapshot $SHOTS/03-menu-downloading.png" >/dev/null
    SNAPPED=1
  fi
  [ "$(field update.state)" != ready ] || break
  sleep 0.3
done
echo "the menu said: $MENUS"
STATE=$(state)
command "menu-snapshot $SHOTS/04-menu-ready.png" >/dev/null
check "the menu showed the download's progress" 'echo "$MENUS" | grep -qE "[0-9]+%"'
check "ready: restart to update to $NEW_V, the dot, the download kept" \
  '[ "$(jget "$STATE" update.state)" = ready ] && jget "$STATE" updateMenu.text | grep -qF "$NEW_V" && [ "$(jget "$STATE" updateMenu.enabled)" = true ] && [ "$(jget "$STATE" updateMenu.dot)" = true ] && [ "$(jget "$STATE" updateMenu.staged)" = true ]'
check "still nothing on screen, nothing installed" '[ "$(jget "$STATE" windowVisible)" = false ] && [ "$(bundle_version)" = "$OLD_V" ]'
BEFORE=$(count "would open the release notes")
command update-notes >/dev/null
check "Release Notes opens the release's page" 'sleep 1; [ "$(count "would open the release notes https://github.com/getdroppen/droppen/releases/tag/v$NEW_V")" -gt "$BEFORE" ]'
# The web app's entry, photographed in both themes: the only time the window shows.
BEFORE=$(probes)
command show >/dev/null
command 'js location.reload()' >/dev/null
wait_for "the page" '[ "$(probes)" -gt "$BEFORE" ]' 30 || true
check "the web app shows its update entry" '[ "$(probe_field desktopUpdate)" = True ]'
command 'js document.documentElement.classList.remove("dark")' >/dev/null
sleep 0.5
command "snapshot $SHOTS/05-rail-entry-light.png" >/dev/null
command 'js document.documentElement.classList.add("dark")' >/dev/null
sleep 0.5
command "snapshot $SHOTS/06-rail-entry-dark.png" >/dev/null
command close >/dev/null
PORT_APP=$(field port)
TOKEN=$(api_login "$PORT_APP")
TERMINAL=$(curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d '{"title":"left open across the update"}' \
  "http://127.0.0.1:$PORT_APP/api/terminals" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])') || TERMINAL=""
api_logout "$PORT_APP" "$TOKEN"
check "(set up) a terminal left open: $TERMINAL" '[ -n "$TERMINAL" ]'

say "4. a plain quit installs it; the next launch is $NEW_V"
OLD_SERVER=$(field serverPid)
TOOK=$(quit_app)
check "quit in ${TOOK}s, the server stopped gracefully" '! alive "$APP_PID" && ! alive "$OLD_SERVER" && grep -q "server stopped at quit: Gracefully (pid Some($OLD_SERVER))" "$LOG"'
check "installed at the quit, the bundle is $NEW_V and verifies, and nothing restarted" \
  'grep -q "updates: installed Droppen $NEW_V" "$LOG" && [ "$(bundle_version)" = "$NEW_V" ] && codesign --verify --deep --strict "$INSTALLED" && [ "$(ls "$APPS")" = "$(basename "$INSTALLED")" ] && [ "$(count "started Droppen again")" = 0 ] && [ ! -e "$SUPPORT/update" ]'
BEFORE=$(probes)
launch
started "$BEFORE"
STATE=$(state)
NEW_SERVER=$(jget "$STATE" serverPid)
check "the next launch is $NEW_V with its own server" \
  '[ "$(jget "$STATE" version)" = "$NEW_V" ] && [ "$(ps -o comm= -p "$NEW_SERVER")" = "$INSTALLED/Contents/MacOS/remote-hand-server" ]'
PORT_APP=$(jget "$STATE" port)
TOKEN=$(api_login "$PORT_APP")
LIVE=$(terminals "$PORT_APP" "$TOKEN" || true)
check "the terminal left open survived the update ($TERMINAL)" '[ -n "$TERMINAL" ] && echo " $LIVE " | grep -qF " $TERMINAL "'
[ -z "$TERMINAL" ] || curl -s -o /dev/null -X DELETE -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:$PORT_APP/api/terminals/$TERMINAL" || true
api_logout "$PORT_APP" "$TOKEN"
command update-check >/dev/null
wait_for "the answer" 'menu_text | grep -qF "$NEW_V"' 30 || true
echo "menu: $(menu_text)"
command "menu-snapshot $SHOTS/07-menu-up-to-date.png" >/dev/null
check "asked: up to date, in the menu" 'menu_text | grep -qF "($NEW_V)" && grep -q "updates: up to date ($NEW_V)" "$LOG"'
kill -TERM "$HTTP_PID" 2>/dev/null || true
wait "$HTTP_PID" 2>/dev/null || true
HTTP_PID=""
command update-check >/dev/null
wait_for "the answer" '[ "$(field update.problem)" = offline ]' 60 || true
STATE=$(state)
echo "menu: $(jget "$STATE" updateMenu.text)"
command "menu-snapshot $SHOTS/08-menu-no-network.png" >/dev/null
check "asked, with the update server gone: the menu says the network" '[ "$(jget "$STATE" update.problem)" = offline ] && [ "$(jget "$STATE" updateMenu.enabled)" = true ]'
quit_app >/dev/null
check "a quit with nothing ready installs nothing" '[ "$(bundle_version)" = "$NEW_V" ] && [ "$(count "updates: installed")" = 1 ]'

say "5. a signed archive that is not an app: the quit leaves the app as it was"
rm -rf "$INSTALLED"
ditto "$OLD" "$INSTALLED"
start_http
serve broken
BEFORE=$(probes)
launch
started "$BEFORE"
command update-check >/dev/null
wait_for "ready" '[ "$(field update.state)" = ready ]' 60 || true
TOOK=$(quit_app)
check "the install failed at the quit (in ${TOOK}s), the bundle is $OLD_V and verifies" \
  'grep -q "updates: not installed (Install)" "$LOG" && [ "$(bundle_version)" = "$OLD_V" ] && codesign --verify --deep --strict "$INSTALLED" && [ "$(ls "$APPS")" = "$(basename "$INSTALLED")" ]'

say "6. the web app's entry restarts into the update"
serve good
BEFORE=$(probes)
launch
started "$BEFORE"
check "the old version runs again" '[ "$(field version)" = "$OLD_V" ]'
command update-check >/dev/null
wait_for "ready" '[ "$(field update.state)" = ready ]' 60 || true
OLD_APP=$APP_PID
command 'js location.assign("droppen-desktop://update/restart?token=0000")' >/dev/null
sleep 1
check "a restart request without the launch's restart token is ignored" 'alive "$OLD_APP" && grep -q "without this launch.s restart token was ignored" "$LOG"'
command 'js window.__droppenDesktop.restartToUpdate()' >/dev/null
wait_for "the old app to go" '! alive "$OLD_APP"' 60 || true
wait_for "the new version" 'P=$(cat "$SUPPORT/app.lock" 2>/dev/null); [ -n "$P" ] && [ "$P" != "$OLD_APP" ] && alive "$P"' 60 || true
APP_PID=$(cat "$SUPPORT/app.lock" 2>/dev/null || true)
wait_for "it to run" '[ "$(field phase)" = running ]' 90 || true
STATE=$(state)
check "restarted into $NEW_V by itself, window still closed" \
  '[ "$(jget "$STATE" version)" = "$NEW_V" ] && grep -q "updates: started Droppen again (pid $APP_PID)" "$LOG" && grep -q "Droppen $NEW_V, started by its update" "$LOG" && [ "$(jget "$STATE" windowVisible)" = false ] && [ "$(bundle_version)" = "$NEW_V" ]'
SERVER_PID=$(jget "$STATE" serverPid)
quit_app >/dev/null
check "quit: nothing runs from the run's folders" '! alive "$APP_PID" && ! alive "$SERVER_PID" && [ -z "$(ours)" ]'

check "every Droppen app that ran before the run still runs, as the same process (${OTHERS_BEFORE:-none})" \
  '( for pid in $OTHERS_BEFORE; do ps -o comm= -p "$pid" | grep -q "/Contents/MacOS/Droppen$" || exit 1; done )'

cp "$HOME_DIR/Library/Logs/$E2E_NAME/server.log" "$SHOTS/server.log" 2>/dev/null || true
echo
if [ "$FAILED" = 0 ]; then echo "ALL PASSED"; else echo "SOME CHECKS FAILED"; exit 1; fi
