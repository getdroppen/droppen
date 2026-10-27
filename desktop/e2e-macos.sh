#!/usr/bin/env bash
# End-to-end check of an end-to-end *test build* of Droppen.app on this Mac
# (`desktop/build.sh --e2e`: a release build has no test hooks and always uses
# the real keychain, so it is never launched here). It runs in a throwaway HOME
# with the keychain off (the password goes to the file fallback) and drives the
# app through its test hook (DROPPEN_DESKTOP_TEST_CONTROL, src-tauri/src/testctl.rs).
#
#   desktop/e2e-macos.sh "<path/to/Droppen E2E.app>" [<screenshot dir>]
#
# Needs a logged-in GUI session: it opens the app's window for real. It runs
# beside the real Droppen, which it never stops, moves or looks at: an
# end-to-end build is somebody else to macOS (bundle id app.droppen.desktop.e2e,
# "Droppen E2E", server instance droppen-app-e2e, its own ports;
# e2e-identity.sh) and refuses the real HOME. It never touches the real HOME,
# keychain, launch agents or any other Droppen instance; every process it stops
# is stopped by pid, while that pid still runs from the build under test. It
# checks at the end that every other Droppen app that ran before it (the real
# one, say) still runs as the same process. What it proves:
#
#   0. an instance directory the app did not create is refused, untouched;
#   1. first launch: the account, `droppen-app`, is made before the server
#      starts, is app-internal and is the only one (the machine-wide account
#      is not inherited), the server is the bundled sidecar on a loopback
#      port, the web app loads logged in (no login page) and opens the
#      first-run guide by itself, on its welcome page, as the server says a
#      fresh install does; going through it as a new user does (Get started,
#      step 01 on, "Only on this Mac", step 03 skipped, Finish) ends on the
#      session list, the server keeps it finished, and a new load of the home
#      opens neither the guide nor its bar again; the list shows the sessions
#      and puts the unread count in the menu bar; a request from this Mac
#      without a session meets the setup state and the API refuses it;
#      the instance keeps no setup code and its control socket opens nothing;
#      *Open in Browser* asks for a one-time sign-in with the app's own login,
#      and it never reaches a log; the server says it runs under the app and
#      refuses self-upgrade;
#   2. a second launch focuses the first and exits;
#   3. closing the window keeps the app, the menu bar item and the server, and
#      start at login writes (and removes) a login item — the launch agent in
#      the throwaway home: an end-to-end build never uses SMAppService, which
#      would register with the real Login Items;
#   4. Settings → Security has no password change;
#      Settings → Droppen signs in through the system browser, at the app host
#      the hub names in its discovery document (a test hub on loopback whose
#      app is my.localhost) while the server stays with the hub: the sign-in
#      address goes to the browser whole (logged by host only), the window
#      stays and shows "continue in your browser" with the device's
#      fingerprint, and cancelling ends the attempt;
#   5. a logout, or a dead token at load, is answered with one fresh login;
#   6. a locked-out account is recovered by a server restart (no new password,
#      which would revoke every login); the second time one new password is
#      tried, then the app's own page explains, until *Try again* recovers;
#   7. an iframe of the web stays in the page; a link off the app opens in the
#      browser and the window stays; a script leaving the app is handed to the
#      browser and the window is taken back; a download from the app's server
#      lands in ~/Downloads marked as downloaded, one from elsewhere does not;
#   8. a crashed server is restarted; one whose port was taken meanwhile moves
#      and the window follows;
#   9. quit stops the server and leaves no process behind;
#  10. a relaunch reuses the stored login and the port, does not open the
#      finished guide again (the session list has no setup bar, the server
#      still keeps it finished), and re-points a login item that still names
#      where the app used to be;
#  11. a server orphaned by a killed app is stopped by the next launch.
set -euo pipefail

APP=${1:?usage: e2e-macos.sh <Droppen E2E.app (an --e2e build)> [<screenshot dir>]}
SHOTS=${2:-/tmp/desktop-review}
# shellcheck source=e2e-identity.sh
. "$(cd "$(dirname "$0")" && pwd -P)/e2e-identity.sh"
e2e_check_app "$APP"
APP=$(cd "$APP" && pwd -P)
BIN="$APP/Contents/MacOS/Droppen"
SERVER="$APP/Contents/MacOS/remote-hand-server"
[ -x "$BIN" ] && [ -x "$SERVER" ] || { echo "not a Droppen app: $APP" >&2; exit 2; }
e2e_check_scratch "$SHOTS" "the screenshot folder"

# The Droppen apps already running (the real one, say): left alone, and
# checked at the end to be running still, as the same processes.
OTHERS_BEFORE=$(ps -axo pid=,comm= | awk '/\/Contents\/MacOS\/Droppen$/ { print $1 }' | sort -n | tr '\n' ' ')

e2e_lock
ROOT=$(cd "$(mktemp -d /tmp/droppen-e2e.XXXXXX)" && pwd -P)
e2e_check_scratch "$ROOT" "the run's folder"
HOME_DIR="$ROOT/home"
CTL="$ROOT/ctl"
SUPPORT="$HOME_DIR/Library/Application Support/$E2E_BUNDLE_ID"
e2e_check_socket_room "$SUPPORT"
LOG="$HOME_DIR/Library/Logs/$E2E_NAME/desktop.log"
SERVER_LOG="$HOME_DIR/Library/Logs/$E2E_NAME/server.log"
INSTANCE_DIR="$HOME_DIR/.local/share/remote-hand/instances/$E2E_INSTANCE"
mkdir -p "$HOME_DIR/Downloads" "$SHOTS"
mkdir -m 700 "$CTL"
APP_PID=""
LISTENER_PID=""
HUB_PID=""
FAILED=0
# WebKit keeps a web view's data under the *account's* home, whatever $HOME
# says — per bundle id, so the end-to-end build's own: removed before the run
# (left by an earlier one) and after it.
e2e_clean_bundle_dirs

say() { printf '\n== %s\n' "$*"; }
pass() { printf 'PASS %s\n' "$*"; }
fail() { printf 'FAIL %s\n' "$*"; FAILED=1; }
check() { if eval "$2"; then pass "$1"; else fail "$1"; fi; }

# Whether no FILE has PATTERN — and every FILE is there to be read. A bare
# `! grep` passes when a file is missing too (grep exits 2), which would take a
# log that was never written for a clean one. `-F`: PATTERN is a fixed string.
nowhere() { # [-F] pattern file…
  local flags=-q status file
  if [ "$1" = -F ]; then flags=-qF; shift; fi
  local pattern=$1
  shift
  for file in "$@"; do
    [ -f "$file" ] || { echo "nowhere: there is no $file to look in" >&2; return 1; }
  done
  grep "$flags" -- "$pattern" "$@" && status=0 || status=$?
  [ "$status" = 1 ]
}

alive() { [ -n "${1:-}" ] && kill -0 "$1" 2>/dev/null; }

# A pid of the build under test (the app, its server), stopped only while it
# still runs from that build.
stop() { e2e_signal "$1" "$2" "$APP"; }

cleanup() {
  # By pid only: the app this script started, the listener it started, then
  # whatever server the app recorded.
  if alive "$APP_PID"; then stop TERM "$APP_PID"; sleep 3; fi
  if alive "$APP_PID"; then stop KILL "$APP_PID"; fi
  if alive "$LISTENER_PID"; then kill -TERM "$LISTENER_PID" 2>/dev/null || true; fi
  if alive "$HUB_PID"; then kill -TERM "$HUB_PID" 2>/dev/null || true; fi
  local server
  server=$(sed -n 's/.*"pid":\([0-9]*\).*/\1/p' "$SUPPORT/server.pid" 2>/dev/null || true)
  if alive "$server"; then stop TERM "$server"; fi
  e2e_unregister "$APP"
  e2e_clean_runtime_dirs "$INSTANCE_DIR"
  if [ "${KEEP:-0}" != 1 ]; then rm -rf "$ROOT"; else echo "kept $ROOT"; fi
  e2e_clean_bundle_dirs
  e2e_unlock
}
trap cleanup EXIT

# The hub the server signs in to (DROPPEN_DESKTOP_TEST_HUB): a test hub on
# loopback that only answers its discovery document, `/.well-known/droppen-hub`
# (docs/hub-api/http.md), naming its app on another host, as api.droppen.app
# names my.droppen.app. Signing in to Droppen opens that host; the server stays with
# the hub (server-rs/src/api/hub_web.rs). Nothing has to listen on the app's
# host: an end-to-end build logs the address instead of opening it.
HUB_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
HUB="http://localhost:$HUB_PORT"
HUB_APP="http://my.localhost:$HUB_PORT"
SIGN_IN_AT="http://my.localhost"
mkdir -p "$ROOT/hub/.well-known"
printf '{"origin":"ws://localhost:%s","app":"%s","site":"http://www.localhost:%s"}\n' "$HUB_PORT" "$HUB_APP" "$HUB_PORT" >"$ROOT/hub/.well-known/droppen-hub"
python3 -m http.server "$HUB_PORT" --bind 127.0.0.1 --directory "$ROOT/hub" >"$ROOT/hub.log" 2>&1 &
HUB_PID=$!

launch() {
  env -i HOME="$HOME_DIR" USER="$USER" LOGNAME="$USER" SHELL=/bin/zsh LANG=en_US.UTF-8 \
    PATH=/usr/bin:/bin:/usr/sbin:/sbin \
    DROPPEN_DESKTOP_KEYCHAIN=0 DROPPEN_DESKTOP_TEST_CONTROL="$CTL" DROPPEN_DESKTOP_TEST_HUB="$HUB" \
    "$BIN" >>"$ROOT/stdout.log" 2>&1 &
  APP_PID=$!
}

# The server binary's own CLI, against the throwaway home.
server_cli() { env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin "$SERVER" "$@"; }

# Send a command to the app and print its result line. Each command carries an
# id of its own (testctl's `@<id> `) and its result is the line with that id: a
# snapshot or a probe can answer after its wait gave up, and that late line is
# never taken for a later command's. The command file is written whole and
# renamed into place: the app, which polls for it, could otherwise read it
# between its creation and its text, take it for empty and drop it.
command() {
  local id line deadline
  id=$(uuidgen)
  printf '@%s %s\n' "$id" "$1" >"$CTL/command.new"
  mv -f "$CTL/command.new" "$CTL/command"
  deadline=$((SECONDS + 20))
  while [ $SECONDS -lt $deadline ]; do
    line=$(grep -F "\"id\":\"$id\"" "$CTL/results.jsonl" 2>/dev/null | tail -1 || true)
    if [ -n "$line" ]; then printf '%s\n' "$line"; return 0; fi
    sleep 0.2
  done
  echo '{"timeout":true}'
}

json() { python3 -c 'import json,sys; v=json.loads(sys.argv[1]); print(v.get(sys.argv[2]))' "$1" "$2"; }
state() { command state; }
field() { json "$(state)" "$1"; }

wait_for() { # description, condition, seconds
  local deadline=$((SECONDS + ${3:-60}))
  while [ $SECONDS -lt $deadline ]; do
    if eval "$2"; then return 0; fi
    sleep 0.5
  done
  echo "timed out waiting for $1" >&2
  return 1
}

count() { local n; n=$(grep -c -- "$1" "$LOG" 2>/dev/null) || true; echo "${n:-0}"; }
probes() { count ' probe: '; }
last_probe() { grep ' probe: ' "$LOG" | tail -1 | sed 's/.* probe: //'; }
probe_field() { python3 -c 'import json,sys; v=json.loads(json.loads(sys.argv[1])); print(v.get(sys.argv[2]))' "$(last_probe)" "$1"; }
reload_and_probe() { # js, then wait for the probe after the next page load
  local before
  before=$(probes)
  command "js $1" >/dev/null
  wait_for "the page to load" '[ "$(probes)" -gt "$before" ]' 30 || true
}
# One reading of the page, as the probe's JSON (fields: `json "$P" path`): now,
# without a page load (testctl's `probe`; the first-run guide's steps are one
# page), or as the last page load left it (the log's). A probe that failed
# fails here: it is no reading, never a page without what is looked for. A
# check takes one reading and reads every field from it.
read_page() {
  python3 -c 'import json,sys
r = json.loads(sys.argv[1])
if r.get("ok") is not True or not isinstance(r.get("probe"), dict): sys.exit(1)
print(json.dumps(r["probe"]))' "$(command probe)" 2>/dev/null
}
loaded_page() {
  python3 -c 'import json,sys
v = json.loads(json.loads(sys.argv[1]))
if not isinstance(v, dict): sys.exit(1)
print(json.dumps(v))' "$(last_probe)" 2>/dev/null
}
guide_at() { local page; page=$(read_page) && [ "$(json "$page" guide)" = "$1" ]; }
on_sessions() { json "$1" path | grep -q '^/sessions'; } # the session list's layout, where the setup bar would be
press() { command "js document.querySelector(\"$1\").click()" >/dev/null; } # selector: a click on it, as a person's
# The guide's progress as the server keeps it (GET /api/setup/progress),
# through a login of the app's own, revoked after.
setup_progress() {
  local port token
  port=$(field port)
  token=$(curl -fsS -H 'Content-Type: application/json' -d "{\"username\":\"$(username)\",\"password\":\"$(password)\"}" "http://127.0.0.1:$port/api/auth/login" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])' || true)
  curl -sS -H "Authorization: Bearer $token" "http://127.0.0.1:$port/api/setup/progress" || true
  curl -s -o /dev/null -X POST -H "Authorization: Bearer $token" "http://127.0.0.1:$port/api/auth/logout" || true
}
# Never started, and the app's first window is to open it by itself.
progress_new() { python3 -c 'import json,sys; p=json.loads(sys.argv[1]); sys.exit(0 if p["autoOpen"] is True and p["startedAt"] is None and p["finishedAt"] is None else 1)' "$1" 2>/dev/null; }
# Finished the way this run goes through it, and never to open by itself again.
progress_finished() {
  python3 -c 'import json,sys
p = json.loads(sys.argv[1])
steps = {"tools": "done", "where": "done", "first": "skipped", "wrap": "done"}
sys.exit(0 if p["startedAt"] and p["finishedAt"] and p["steps"] == steps and p["where"]["choice"] == "local" and p["autoOpen"] is False else 1)' "$1" 2>/dev/null
}
login_code() { # user password -> the HTTP status of a login with them
  curl -s -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' \
    -d "{\"username\":\"$1\",\"password\":\"$2\"}" "http://127.0.0.1:$(field port)/api/auth/login"
}
password() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["password"])' "$SUPPORT/credentials.json"; }
username() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["username"])' "$SUPPORT/credentials.json"; }
hostile_logins() { # five wrong passwords for the app's account: the server locks it
  local user port
  user=$(username)
  port=$(field port)
  for _ in 1 2 3 4 5 6; do
    curl -s -o /dev/null -H 'Content-Type: application/json' \
      -d "{\"username\":\"$user\",\"password\":\"wrong-password-0000\"}" "http://127.0.0.1:$port/api/auth/login" || true
  done
}

say "0. an instance directory the app did not create is refused, untouched"
mkdir -p "$INSTANCE_DIR"
echo "someone else's" >"$INSTANCE_DIR/notes.txt"
launch
wait_for "the refusal" '[ "$(field phase)" = failed ]' 60 || true
STATE=$(state)
echo "$STATE"
check "refused with a reason, on the app's own page" '[ "$(json "$STATE" phase)" = failed ] && json "$STATE" message | grep -q "did not create" && [ "$(json "$STATE" windowScheme)" = tauri ]'
check "nothing written into it" '[ "$(ls "$INSTANCE_DIR")" = notes.txt ]'
command quit >/dev/null
wait_for "the app to exit" '! alive "$APP_PID"' 30 || true
rm -rf "$INSTANCE_DIR"

say "seed a machine-wide account the new instance must not inherit"
printf '%s\n' "intruder-password-0001" | server_cli user add intruder --instance scratch >/dev/null
mkdir -p "$HOME_DIR/.config/remote-hand"
cp "$HOME_DIR/.local/share/remote-hand/instances/scratch/users.json" "$HOME_DIR/.config/remote-hand/users.json"
rm -rf "$HOME_DIR/.local/share/remote-hand/instances/scratch"

say "1. first launch"
launch
wait_for "the web app to load" '[ "$(probes)" -ge 1 ]' 90
STATE=$(state)
echo "$STATE"
PORT=$(json "$STATE" port)
SERVER_PID=$(json "$STATE" serverPid)
USER_NAME=$(username)
check "phase is running" '[ "$(json "$STATE" phase)" = running ]'
check "the port is not 8700/8701 and is loopback-only" '[ "$PORT" != 8700 ] && [ "$PORT" != 8701 ] && lsof -nP -a -p "$SERVER_PID" -iTCP:"$PORT" -sTCP:LISTEN | grep -q "127.0.0.1:$PORT"'
check "the server is the bundled sidecar" '[ "$(ps -o comm= -p "$SERVER_PID")" = "$SERVER" ]'
check "the instance is app-made (marker) and held by that server" '[ -f "$INSTANCE_DIR/droppen-app.json" ] && [ "$(cat "$INSTANCE_DIR/lock")" = "$SERVER_PID" ] && [ "$(cat "$INSTANCE_DIR/server.port")" = "$PORT" ]'
check "the account was made before the server started" '[ "$(grep -n "password kept in the" "$LOG" | head -1 | cut -d: -f1)" -lt "$(grep -n "server started" "$LOG" | head -1 | cut -d: -f1)" ]'
check "the app's login is droppen-app" '[ "$USER_NAME" = droppen-app ]'
check "the machine-wide account was not inherited; only the app's app-internal account is on the instance" '! grep -q "inherited account(s)" "$LOG" && [ "$(server_cli user list --app --instance "$E2E_INSTANCE" | sed 1d | cut -f1)" = droppen-app ] && server_cli user list --instance "$E2E_INSTANCE" | head -1 | grep -q "^no accounts"'
check "the machine-wide login is refused" '[ "$(login_code intruder intruder-password-0001)" = 401 ]'
check "the marker records the pruning done" 'grep -q "\"pruned\": true" "$INSTANCE_DIR/droppen-app.json"'
check "the password is in the 0600 file (keychain off)" '[ "$(stat -f %Lp "$SUPPORT/credentials.json")" = 600 ] && [ "$(stat -f %Lp "$SUPPORT")" = 700 ]'
check "no secret in the app log" 'nowhere -F "$(password)" "$LOG"'
STATUS=$(curl -fsS "http://127.0.0.1:$PORT/api/auth/status")
check "without a session, this Mac too meets the setup state; the server says it runs under the app" 'echo "$STATUS" | grep -q "\"authRequired\":true" && echo "$STATUS" | grep -q "\"setupRequired\":true" && echo "$STATUS" | grep -q "\"desktopApp\":true"'
check "nothing another process of this user can reach opens the app's instance: no setup code, a control socket that refuses" '[ ! -e "$INSTANCE_DIR/setup-code" ] && ! server_cli open --print --instance "$E2E_INSTANCE" >"$ROOT/open.out" 2>&1 && grep -q "Open in Browser" "$ROOT/open.out" && nowhere "local-open=" "$ROOT/open.out"'
check "the API refuses a request from this Mac without a session (401)" '[ "$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:$PORT/api/settings")" = 401 ]'
command open-in-browser >/dev/null
wait_for "Open in Browser" 'grep -q "would open the page in the browser" "$LOG"' 20 || true
check "Open in Browser: a one-time sign-in asked for with the app's login, in no log" 'grep -q "would open the page in the browser with a one-time sign-in" "$LOG" && nowhere "local-open=[A-Za-z0-9_-]" "$LOG" "$SERVER_LOG"'
TOKEN=$(curl -fsS -H 'Content-Type: application/json' -d "{\"username\":\"$USER_NAME\",\"password\":\"$(password)\"}" "http://127.0.0.1:$PORT/api/auth/login" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')
printf 'not a server\n' >"$ROOT/fake-binary"
UPGRADE=$(curl -s -w ' %{http_code}' -H "Authorization: Bearer $TOKEN" -F "binary=@$ROOT/fake-binary" "http://127.0.0.1:$PORT/api/server/self-upgrade")
curl -s -o /dev/null -X POST -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:$PORT/api/auth/logout"
unset TOKEN
check "self-upgrade is refused (403) and the shared bin/ untouched" 'echo "$UPGRADE" | grep -q "self_upgrade_disabled" && [ "${UPGRADE##* }" = 403 ] && [ ! -e "$HOME_DIR/.local/share/remote-hand/bin" ]'
echo "probe: $(last_probe)"
check "first launch in a fresh home: the first-run guide opens by itself, on its welcome page, logged in (token stored, no login form)" \
  'P=$(loaded_page) && [ "$(json "$P" path)" = /welcome ] && [ "$(json "$P" guide)" = welcome ] && [ "$(json "$P" hasToken)" = True ] && [ "$(json "$P" loginForm)" = False ]'
PROGRESS=$(setup_progress)
check "the server says the guide never started and is to open by itself" 'progress_new "$PROGRESS"'

say "   the first-run guide, gone through as a new user does"
sleep 2 # the welcome page says what it found of Claude Code and Codex
command "snapshot $SHOTS/00a-guide-welcome.png" >/dev/null
press "[data-testid=welcome-start]"
wait_for "step 01" 'guide_at tools' 20 || true
check "Get started opens step 01 (AI tools)" 'guide_at tools'
sleep 1
command "snapshot $SHOTS/00b-guide-tools.png" >/dev/null
# "Continue", or "Continue without AI tools" where none is ready.
press "[data-testid=step-action]"
wait_for "step 02" 'guide_at where' 20 || true
check "step 01 goes on to step 02 (where to use it)" 'guide_at where'
press "[data-choice=local]"
sleep 1
command "snapshot $SHOTS/00c-guide-where.png" >/dev/null
press "[data-testid=step-action]"
wait_for "step 03" 'guide_at first' 20 || true
check "Only on this Mac goes on to step 03 (first session)" 'guide_at first'
sleep 1
command "snapshot $SHOTS/00d-guide-first.png" >/dev/null
press "[data-testid=skip-step]"
wait_for "step 04" 'guide_at wrap' 20 || true
check "skipping step 03 goes on to step 04 (finishing up)" 'guide_at wrap'
sleep 1
command "snapshot $SHOTS/00e-guide-wrap.png" >/dev/null
press "[data-testid=step-action]"
wait_for "the session list" 'P=$(read_page) && [ "$(json "$P" path)" = /sessions ] && [ "$(json "$P" guide)" = None ]' 20 || true
check "Finish leaves the guide for the session list, with no setup bar" \
  'P=$(read_page) && [ "$(json "$P" path)" = /sessions ] && [ "$(json "$P" guide)" = None ] && [ "$(json "$P" setupBar)" = None ]'
PROGRESS=$(setup_progress)
echo "progress: $PROGRESS"
check "the server keeps it finished (steps done, done, skipped, done; only this Mac), never to open by itself again" 'progress_finished "$PROGRESS"'
reload_and_probe 'location.assign("/")'
echo "probe: $(last_probe)"
check "a new load of the home, still without a session: the session list, neither the guide nor its bar back" \
  'P=$(loaded_page) && on_sessions "$P" && [ "$(json "$P" guide)" = None ] && [ "$(json "$P" setupBar)" = None ]'
check "the web app is logged in (token stored, no login form, not on /login)" '[ "$(probe_field hasToken)" = True ] && [ "$(probe_field loginForm)" = False ] && [ "$(probe_field path)" != /login ] && [ "$(probe_field firstRunCard)" = True ]'

say "   sessions arrive in the list"
for id in 11111111-1111-4111-8111-111111111111 22222222-2222-4222-8222-222222222222; do
  mkdir -p "$INSTANCE_DIR/claude/projects/-workspace"
  printf '%s\n%s\n' \
    '{"type":"user","cwd":"/workspace","timestamp":"2026-10-18T10:00:00.000Z","message":{"content":[{"type":"text","text":"tidy the release notes ('"${id:0:4}"')"}]}}' \
    '{"type":"assistant","timestamp":"2026-10-18T10:00:09.000Z","message":{"content":[{"type":"text","text":"Done, have a look."}]}}' \
    >"$INSTANCE_DIR/claude/projects/-workspace/$id.jsonl"
done
sleep 2
reload_and_probe "location.reload()"
if [ "$(probe_field sessionLinks)" -lt 2 ]; then sleep 3; reload_and_probe "location.reload()"; fi
check "the session list shows the sessions" '[ "$(probe_field sessionLinks)" -ge 2 ]'
command "snapshot $SHOTS/01-first-launch.png"
wait_for "the unread count" '[ "$(field trayTitle)" = 2 ]' 15 || true
check "the unread count reaches the menu bar" '[ "$(field trayTitle)" = 2 ]'

say "2. a second launch focuses the first"
SECOND_START=$SECONDS
env -i HOME="$HOME_DIR" USER="$USER" SHELL=/bin/zsh PATH=/usr/bin:/bin DROPPEN_DESKTOP_KEYCHAIN=0 "$BIN" >>"$ROOT/stdout.log" 2>&1 && SECOND_EXIT=0 || SECOND_EXIT=$?
check "the second launch exited (0) within 10s" '[ "$SECOND_EXIT" = 0 ] && [ $((SECONDS - SECOND_START)) -lt 10 ]'
check "the first one was told to show its window" 'grep -q "asked it to show its window: true" "$LOG" && grep -q "another launch: showing the window" "$LOG"'
check "still one server" '[ "$(field serverPid)" = "$SERVER_PID" ]'

say "3. closing the window keeps the app, the menu bar item and the server"
command close >/dev/null
sleep 1
STATE=$(state)
echo "$STATE"
check "window hidden, app running, tray present" '[ "$(json "$STATE" windowVisible)" = False ] && alive "$APP_PID" && [ "$(json "$STATE" trayPresent)" = True ]'
check "the server is still up" 'alive "$SERVER_PID" && curl -fsS "http://127.0.0.1:$PORT/api/auth/status" >/dev/null'
command show >/dev/null
sleep 1
check "the window comes back" '[ "$(field windowVisible)" = True ]'
AGENT="$HOME_DIR/Library/LaunchAgents/$E2E_BUNDLE_ID.plist"
# The real LaunchAgents: the release's agent as it was, and none of this build's.
real_agents() { ls -l "$E2E_ACCOUNT_HOME/Library/LaunchAgents/" 2>/dev/null | grep -F "$RELEASE_BUNDLE_ID" || true; }
REAL_AGENTS_BEFORE=$(real_agents)
RESULT=$(command autostart)
check "start at login: the agent opens the app hidden" '[ "$(json "$RESULT" on)" = True ] && grep -q "<string>$BIN</string><string>--autostart</string>" "$AGENT"'
check "the real LaunchAgents is untouched" '[ "$(real_agents)" = "$REAL_AGENTS_BEFORE" ] && [ ! -e "$E2E_ACCOUNT_HOME/Library/LaunchAgents/$E2E_BUNDLE_ID.plist" ]'
RESULT=$(command autostart)
check "start at login off: the agent is gone" '[ "$(json "$RESULT" on)" = False ] && [ ! -e "$AGENT" ]'

say "4. Settings under the app"
reload_and_probe 'location.assign("/settings?section=security")'
sleep 1
command "snapshot $SHOTS/03-settings-security.png"
echo "probe: $(last_probe)"
check "Access: no password change, no sign-out; a Droppen section" '[ "$(probe_field securityDesktopApp)" = True ] && [ "$(probe_field loginForm)" = False ] && [ "$(probe_field droppenSection)" = True ]'
# (`?e2e`: this look is not counted as the server's below.)
check "the test hub answers its discovery document" 'curl -fsS "$HUB/.well-known/droppen-hub?e2e" | grep -qF "\"app\":\"$HUB_APP\""'
reload_and_probe 'location.assign("/settings?section=Droppen")'
SIGN_INS=$(count "would open the Droppen sign-in at $SIGN_IN_AT in the browser")
command 'js document.querySelector("[data-testid=droppen-sign-in]").click()' >/dev/null
wait_for "the sign-in to open" '[ "$(count "would open the Droppen sign-in at $SIGN_IN_AT in the browser")" -gt "$SIGN_INS" ]' 15 || true
sleep 1
command "snapshot $SHOTS/04-droppen-sign-in.png"
check "Sign in to Droppen opens the hub's app host (my.localhost) in the browser, by host only in the log" '[ "$(count "would open the Droppen sign-in at $SIGN_IN_AT in the browser")" -gt "$SIGN_INS" ] && [ "$(count "would open the Droppen sign-in at http://localhost in the browser")" = 0 ] && nowhere -F "droppen-link=" "$LOG"'
check "the window stays on the app" '[ "$(field windowVisible)" = True ] && [ "$(field windowScheme)" = http ] && [ "$(field windowPort)" = "$(field port)" ]'
SIGN_IN_TOKEN=$(curl -fsS -H 'Content-Type: application/json' -d "{\"username\":\"$USER_NAME\",\"password\":\"$(password)\"}" "http://127.0.0.1:$PORT/api/auth/login" | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])' || true)
ATTEMPT_JSON=$(curl -sS -H "Authorization: Bearer $SIGN_IN_TOKEN" "http://127.0.0.1:$PORT/api/cloud" || true)
curl -s -o /dev/null -X POST -H "Authorization: Bearer $SIGN_IN_TOKEN" "http://127.0.0.1:$PORT/api/auth/logout" || true
unset SIGN_IN_TOKEN
check "an attempt waits, with its address on the hub's app host in a fragment only" 'python3 -c "import json,sys; a=json.loads(sys.argv[1])[\"attempt\"]; u=a[\"handoffUrl\"]; sys.exit(0 if a[\"mine\"] and u.startswith(sys.argv[2] + \"/link#droppen-link=\") and \"?\" not in u else 1)" "$ATTEMPT_JSON" "$HUB_APP"'
check "the server stays with the hub, and says where a browser signs in to it" 'python3 -c "import json,sys; s=json.loads(sys.argv[1]); sys.exit(0 if s[\"hub\"] == sys.argv[2] and s[\"hubWeb\"] == sys.argv[3] else 1)" "$ATTEMPT_JSON" "ws://localhost:$HUB_PORT" "$HUB_APP" && grep -qF "\"GET /.well-known/droppen-hub HTTP/" "$ROOT/hub.log"'
command 'js document.querySelector("[data-testid=droppen-cancel]").click()' >/dev/null
sleep 1
reload_and_probe 'location.assign("/")'

say "5. a logout, or a dead token at load, is answered with one fresh login"
RELOGINS=$(count "logged in again")
reload_and_probe 'fetch("/api/auth/logout",{method:"POST",headers:{Authorization:"Bearer "+localStorage.getItem("rh.auth.token")}}).then(function(){localStorage.removeItem("rh.auth.token");location.replace("/login")})'
check "a logout: logged in again once, no login page" '[ "$(count "logged in again")" = $((RELOGINS + 1)) ] && [ "$(probe_field hasToken)" = True ] && [ "$(probe_field loginForm)" = False ]'
RELOGINS=$(count "logged in again")
reload_and_probe 'localStorage.setItem("rh.auth.token","not-a-token"); location.reload()'
check "a dead token: logged in again once, no login page" '[ "$(count "logged in again")" = $((RELOGINS + 1)) ] && [ "$(probe_field loginForm)" = False ] && [ "$(probe_field path)" != /login ]'
check "the replaced login was revoked" 'grep -q "revoked the login this one replaces" "$LOG"'

say "6. a locked-out account: a restart first, a new password second, then the app's own page"
hostile_logins
reload_and_probe 'localStorage.removeItem("rh.auth.token"); location.replace("/login")'
wait_for "the recovery" 'grep -q "restarting the server to clear a lockout" "$LOG" && [ "$(field phase)" = running ]' 90 || true
check "a restart cleared the lockout; no new password (which would revoke every login)" 'grep -q "restarting the server to clear a lockout" "$LOG" && [ "$(count "setting a new password")" = 0 ] && [ "$(field phase)" = running ] && [ "$(probe_field loginForm)" = False ]'
SERVER_PID=$(field serverPid)
hostile_logins
command 'js localStorage.removeItem("rh.auth.token"); location.replace("/login")' >/dev/null
wait_for "the app's own page" '[ "$(field phase)" = failed ]' 60 || true
STATE=$(state)
echo "$STATE"
check "the second time: the app's own page with a reason, never the login page" '[ "$(json "$STATE" phase)" = failed ] && [ "$(json "$STATE" windowScheme)" = tauri ] && json "$STATE" message | grep -q "refused the app'"'"'s login"'
command "snapshot $SHOTS/04-login-refused.png"
check "one restart and one new password per run, no more" '[ "$(count "restarting the server to clear a lockout")" = 1 ] && [ "$(count "setting a new password")" = 1 ]'
BEFORE=$(probes)
command retry >/dev/null
wait_for "try again" '[ "$(field phase)" = running ] && [ "$(probes)" -gt "$BEFORE" ]' 90 || true
check "try again recovers (a fresh server clears the lockout)" '[ "$(field phase)" = running ] && [ "$(probe_field loginForm)" = False ]'
SERVER_PID=$(field serverPid)
PORT=$(field port)

say "7. frames stay in the page; a page off the app goes to the browser"
OPENED=$(count "would open https://example.com in the browser")
command 'js var f=document.createElement("iframe"); f.src="https://example.com/"; document.body.appendChild(f)' >/dev/null
sleep 4
check "an iframe of the web is loaded in place, not sent to the browser" '[ "$(count "would open https://example.com in the browser")" = "$OPENED" ]'
command 'js var a=document.createElement("a"); a.href="https://example.com/"; a.textContent="x"; document.body.appendChild(a); a.click()' >/dev/null
wait_for "the link to go to the browser" '[ "$(count "would open https://example.com in the browser")" -gt "$OPENED" ]' 15 || true
STATE=$(state)
check "a plain link off the app goes to the browser, the window stays" '[ "$(count "would open https://example.com in the browser")" -gt "$OPENED" ] && [ "$(json "$STATE" windowPort)" = "$PORT" ]'
OPENED=$(count "would open https://example.com in the browser")
command 'js location.href = "https://example.com/"' >/dev/null
# The page is caught when it commits, which waits for the network.
wait_for "the hand-off to the browser" '[ "$(count "would open https://example.com in the browser")" -gt "$OPENED" ]' 45 || true
wait_for "the window to step back" '[ "$(field windowPort)" = "$PORT" ]' 20 || true
STATE=$(state)
check "a script leaving the app: the page goes to the browser, the window is taken back" '[ "$(count "would open https://example.com in the browser")" -gt "$OPENED" ] && [ "$(json "$STATE" windowPort)" = "$PORT" ]'

say "7b. downloads: the app's own only, marked as downloaded"
command 'js var a=document.createElement("a"); a.href="/logo.png"; a.download="droppen-logo.png"; document.body.appendChild(a); a.click()' >/dev/null
wait_for "the download" '[ -f "$HOME_DIR/Downloads/droppen-logo.png" ] && grep -q "marked as downloaded" "$LOG"' 20 || true
check "a download from the app's server lands in ~/Downloads, quarantined" '[ -f "$HOME_DIR/Downloads/droppen-logo.png" ] && xattr -p com.apple.quarantine "$HOME_DIR/Downloads/droppen-logo.png" | grep -q ";Droppen;"'
mkdir -p "$ROOT/elsewhere"
printf 'not from the app\n' >"$ROOT/elsewhere/payload.bin"
OTHER_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
python3 -m http.server "$OTHER_PORT" --bind 127.0.0.1 --directory "$ROOT/elsewhere" >/dev/null 2>&1 &
LISTENER_PID=$!
wait_for "the other server" 'curl -fs -o /dev/null "http://127.0.0.1:$OTHER_PORT/payload.bin"' 10 || true
REFUSED=$(count "goes to the browser")
command "js var f=document.createElement(\"iframe\"); f.src=\"http://127.0.0.1:$OTHER_PORT/payload.bin\"; document.body.appendChild(f)" >/dev/null
wait_for "the refusal" '[ "$(count "goes to the browser")" -gt "$REFUSED" ]' 20 || true
check "a download from anywhere else is not taken (sent to the browser)" '[ "$(count "goes to the browser")" -gt "$REFUSED" ] && [ ! -e "$HOME_DIR/Downloads/payload.bin" ]'
kill -TERM "$LISTENER_PID" 2>/dev/null || true
wait "$LISTENER_PID" 2>/dev/null || true
LISTENER_PID=""

say "8. a crashed server is restarted; a taken port moves it and the window follows"
stop KILL "$SERVER_PID"
wait_for "a new server" '[ "$(field serverPid)" != "$SERVER_PID" ] && [ "$(field serverPid)" != None ]' 30 || true
NEW_SERVER=$(field serverPid)
wait_for "it to answer" 'curl -fsS "http://127.0.0.1:$PORT/api/auth/status" >/dev/null 2>&1' 30 || true
check "restarted on the same port ($NEW_SERVER)" '[ "$NEW_SERVER" != "$SERVER_PID" ] && alive "$NEW_SERVER" && [ "$(field port)" = "$PORT" ]'
SERVER_PID=$NEW_SERVER
stop KILL "$SERVER_PID"
python3 -c 'import socket,sys,time
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(1); time.sleep(120)' "$PORT" &
LISTENER_PID=$!
BEFORE=$(probes)
wait_for "the move" '[ "$(field port)" != "$PORT" ] && [ "$(field windowPort)" = "$(field port)" ] && [ "$(probes)" -gt "$BEFORE" ]' 60 || true
STATE=$(state)
echo "$STATE"
check "moved to another port, and the window with it, logged in" '[ "$(json "$STATE" port)" != "$PORT" ] && [ "$(json "$STATE" windowPort)" = "$(json "$STATE" port)" ] && [ "$(probe_field hasToken)" = True ] && [ "$(probe_field loginForm)" = False ]'
kill -TERM "$LISTENER_PID" 2>/dev/null || true
wait "$LISTENER_PID" 2>/dev/null || true
LISTENER_PID=""
PORT=$(json "$STATE" port)
SERVER_PID=$(json "$STATE" serverPid)

say "   a login item left pointing where the app used to be"
command autostart >/dev/null
sed -i '' "s#<string>$BIN</string>#<string>/Applications/Moved Away.app/Contents/MacOS/Droppen</string>#" "$AGENT"
check "(set up) the login item now names another place" 'nowhere -F "<string>$BIN</string>" "$AGENT" && grep -q "Moved Away" "$AGENT"'

say "9. quit stops the server"
command quit >/dev/null
wait_for "the app to exit" '! alive "$APP_PID"' 30 || true
check "the app exited" '! alive "$APP_PID"'
check "the server is gone (pid $SERVER_PID)" '! alive "$SERVER_PID"'
check "nothing listens on $PORT" '! lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1'
check "no process runs from the bundle" '! ps -axo pid=,comm= | grep -F "$APP/Contents/MacOS/"'
check "the pid record is gone and the token was revoked" '[ ! -e "$SUPPORT/server.pid" ] && grep -q "logged out" "$LOG"'

say "10. a relaunch reuses the stored login and the port"
ACCOUNT_LINES=$(count "password kept in the")
BEFORE=$(probes)
launch
wait_for "the web app to load" '[ "$(probes)" -gt "$BEFORE" ]' 90 || true
STATE=$(state)
echo "probe: $(last_probe)"
check "no new account, the stored login was used" '[ "$(count "password kept in the")" = "$ACCOUNT_LINES" ] && grep -q "using the stored login" "$LOG"'
check "same port" '[ "$(json "$STATE" port)" = "$PORT" ]'
check "logged in, no login page" '[ "$(probe_field hasToken)" = True ] && [ "$(probe_field loginForm)" = False ]'
# Sessions exist by now, so the guide would not open by itself anyway: what
# shows it stayed finished is the session list without the setup bar and the
# server's record.
check "the finished guide does not open again: the session list, no setup bar, the server keeps it finished" \
  'P=$(loaded_page) && on_sessions "$P" && [ "$(json "$P" guide)" = None ] && [ "$(json "$P" setupBar)" = None ] && progress_finished "$(setup_progress)"'
check "the login item starts the app from where it is now" 'grep -qF "<string>$BIN</string>" "$AGENT" && nowhere "Moved Away" "$AGENT" && grep -q "start at login pointed elsewhere" "$LOG"'
RESULT=$(command autostart)
check "(clean up) start at login off again" '[ "$(json "$RESULT" on)" = False ] && [ ! -e "$AGENT" ]'
command "snapshot $SHOTS/05-relaunch.png"
SERVER_PID=$(json "$STATE" serverPid)

say "11. a server orphaned by a killed app is stopped by the next launch"
stop KILL "$APP_PID"
sleep 1
check "the orphan is still running (pid $SERVER_PID)" 'alive "$SERVER_PID"'
BEFORE=$(probes)
launch
wait_for "the web app to load" '[ "$(probes)" -gt "$BEFORE" ]' 90 || true
check "the orphan was stopped" '! alive "$SERVER_PID" && grep -q "a server from an earlier run: Stopped" "$LOG"'
check "a new server is up" '[ "$(field phase)" = running ]'
SERVER_PID=$(field serverPid)
command quit >/dev/null
wait_for "the app to exit" '! alive "$APP_PID"' 30 || true
check "clean exit" '! alive "$APP_PID" && ! alive "$SERVER_PID"'

say "web view support (from the last probe)"
last_probe | python3 -c 'import json,sys; v=json.loads(json.loads(sys.stdin.read())); [print(f"  {k}: {v.get(k)}") for k in ("serviceWorker","serviceWorkers","pushManager","notification","setAppBadge","indexedDB","indexedDBOpen","webSocket","secureContext","clipboard")]'
check "every Droppen app that ran before the run still runs, as the same process (${OTHERS_BEFORE:-none})" \
  '( for pid in $OTHERS_BEFORE; do ps -o comm= -p "$pid" | grep -q "/Contents/MacOS/Droppen$" || exit 1; done )'
cp "$LOG" "$SHOTS/desktop.log"
cp "$SERVER_LOG" "$SHOTS/server.log" 2>/dev/null || true
echo
if [ "$FAILED" = 0 ]; then echo "ALL PASSED"; else echo "SOME CHECKS FAILED"; exit 1; fi
