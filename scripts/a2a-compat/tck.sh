#!/usr/bin/env bash
# Run the official A2A TCK (a2aproject/a2a-tck) against a local Droppen
# server's `tck` entry (docs/a2a.md §5):
#
#   scripts/a2a-compat/tck.sh [--level must|should|may|all] [extra pytest args...]
#
# The level is `must` when not given: the TCK's mandatory requirements.
#
# What it does, all of it under $A2A_COMPAT_DIR (default /tmp/droppen-a2a),
# never in the repository:
#   1. checks out a2a-tck at $TCK_REV and installs it with uv (the TCK wants
#      Python >= 3.15, which uv downloads);
#   2. builds the server and starts it on 127.0.0.1:$TCK_DROPPEN_PORT with an
#      instance of its own (data and config in the work directory),
#      DROPPEN_A2A_TCK=1, and authentication off for the management API only
#      — `/a2a/*` asks for its A2A token whatever that switch says;
#   3. makes the `tck` entry and a token for it through /api/a2a/*;
#   4. runs `run_tck.py --sut-host http://127.0.0.1:$PORT/a2a/agents/tck`, with
#      a2a_bearer_plugin.py adding the token to every request the TCK makes
#      (the TCK has no option for the SUT's credentials).
#
# Reports: $A2A_COMPAT_DIR/a2a-tck/reports/ (compatibility.json and .html).
# The server ends with the script, and by itself after $LIFETIME_S seconds
# whatever happens to the script.
#
# Needs: git, curl, uv, python3 and cargo. Cargo builds into $CARGO_TARGET_DIR
# when it is set, else the repository's target/.
#
# Not expected to pass in M1a: PUSH-DELIVER-* (nothing delivers pushes before
# M1c, and the TCK's webhook is plain http on localhost, which §6 refuses).
set -euo pipefail

COMPAT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$COMPAT_DIR/../.." && pwd)"
WORK="${A2A_COMPAT_DIR:-/tmp/droppen-a2a}"
RUN="$WORK/run"
TCK="$WORK/a2a-tck"
# The revision these checks were last run with; dont-do's checks pin the same.
TCK_REV="${TCK_REV:-5e6f37c2d38eb3d7684686243c34d365c83df49b}"
PORT="${TCK_DROPPEN_PORT:-4251}"
LIFETIME_S="${LIFETIME_S:-1500}"
TARGET="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"

LEVEL=must
if [ "${1:-}" = "--level" ]; then
  LEVEL=$2
  shift 2
fi

log() { printf '\n== %s\n' "$*" >&2; }

for tool in git curl uv python3 cargo; do
  command -v "$tool" >/dev/null || { echo "tck.sh: $tool is required" >&2; exit 2; }
done

mkdir -p "$WORK"
rm -rf "$RUN"
mkdir -p "$RUN"

log "a2a-tck @ $TCK_REV in $TCK"
if [ ! -d "$TCK/.git" ]; then
  git clone --quiet https://github.com/a2aproject/a2a-tck "$TCK"
fi
git -C "$TCK" checkout --quiet "$TCK_REV" 2>/dev/null || {
  git -C "$TCK" fetch --quiet origin
  git -C "$TCK" checkout --quiet "$TCK_REV"
}
(cd "$TCK" && { [ -x .venv/bin/python ] || uv venv --quiet --python 3.15; } && uv pip install --quiet -e .)

log "building the server"
(cd "$REPO_ROOT" && DROPPEN_ALLOW_MISSING_WEB_DIST=1 CARGO_TARGET_DIR="$TARGET" cargo build --quiet -p droppen-server)
BIN="$TARGET/debug/droppen-server"

log "starting the server on 127.0.0.1:$PORT ($RUN/server.log)"
DROPPEN_DATA_DIR="$RUN/data" DROPPEN_CONFIG_DIR="$RUN/config" APP_INSTANCE=a2a-tck \
  APP_SERVER__HOST=127.0.0.1 APP_SERVER__PORT="$PORT" APP_AUTH__ENABLED=false \
  DROPPEN_A2A_TCK=1 DROPPEN_ALLOW_MISSING_WEB_DIST=1 \
  timeout "$LIFETIME_S" "$BIN" serve >"$RUN/server.log" 2>&1 &
SERVER=$!
trap 'kill "$SERVER" 2>/dev/null || true; wait "$SERVER" 2>/dev/null || true' EXIT

BASE="http://127.0.0.1:$PORT"
for _ in $(seq 1 120); do
  curl -sf -o /dev/null "$BASE/api/auth/status" && break
  kill -0 "$SERVER" 2>/dev/null || { cat "$RUN/server.log" >&2; exit 1; }
  sleep 0.5
done

log "making the tck entry and its token"
curl -sf -X POST "$BASE/api/a2a/entries" -H 'content-type: application/json' \
  -d '{"id": "tck", "name": "Droppen TCK", "description": "The A2A TCK scenarios.", "source": "tck", "enabled": true, "pushOrigins": ["https://example.com"]}' >/dev/null
TOKEN=$(curl -sf -X POST "$BASE/api/a2a/tokens" -H 'content-type: application/json' \
  -d '{"name": "a2a-tck", "entries": ["tck"]}' | python3 -c 'import json, sys; print(json.load(sys.stdin)["secret"])')

level_args=()
[ "$LEVEL" != all ] && level_args=(--level "$LEVEL")
log "running the TCK (level: $LEVEL) against $BASE/a2a/agents/tck"
set +e
(cd "$TCK" && A2A_BEARER_TOKEN="$TOKEN" PYTHONPATH="$COMPAT_DIR" TCK_STREAMING_TIMEOUT="${TCK_STREAMING_TIMEOUT:-2}" \
  .venv/bin/python run_tck.py --sut-host "$BASE/a2a/agents/tck" ${level_args[@]+"${level_args[@]}"} -- -p a2a_bearer_plugin "$@")
status=$?
set -e
log "TCK exit status $status; reports in $TCK/reports/; the server's log is $RUN/server.log"
exit $status
