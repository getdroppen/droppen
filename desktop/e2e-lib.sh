# shellcheck shell=bash
# What both macOS end-to-end harnesses share: the PASS/FAIL lines and checks,
# the test-control command channel, the app's state and the web view's probes,
# and the run's login. Sourced by e2e-macos.sh and e2e-updater-macos.sh (after
# e2e-identity.sh); not run on its own.
#
# The caller sets CTL (the test-control directory), LOG (the app's desktop
# log), SUPPORT (the app's Application Support directory) and FAILED, and
# keeps its own JSON field reader (`field`: e2e-macos.sh's `json`, and
# e2e-updater-macos.sh's `jget`, which also reads dotted paths).

say() { printf '\n== %s\n' "$*"; }
pass() { printf 'PASS %s\n' "$*"; }
fail() { printf 'FAIL %s\n' "$*"; FAILED=1; }
check() { if eval "$2"; then pass "$1"; else fail "$1"; fi; }

alive() { [ -n "${1:-}" ] && kill -0 "$1" 2>/dev/null; }

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

state() { command state; }

count() { local n; n=$(grep -c -- "$1" "$LOG" 2>/dev/null) || true; echo "${n:-0}"; }
probes() { count ' probe: '; }
last_probe() { grep ' probe: ' "$LOG" | tail -1 | sed 's/.* probe: //'; }
probe_field() { python3 -c 'import json,sys; v=json.loads(json.loads(sys.argv[1])); print(v.get(sys.argv[2]))' "$(last_probe)" "$1"; }
# One reading of the page, as the probe's JSON (fields: the caller's
# `json "$P" path`, `jget` in e2e-updater-macos.sh): now, without a page load
# (testctl's `probe`; the first-run guide's steps are one page), or as the
# last page load left it (the log's). A probe that failed fails here: it is no
# reading, never a page without what is looked for. A check takes one reading
# and reads every field from it.
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

press() { command "js document.querySelector(\"$1\").click()" >/dev/null; } # selector: a click on it, as a person's
password() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["password"])' "$SUPPORT/credentials.json"; }
username() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["username"])' "$SUPPORT/credentials.json"; }
