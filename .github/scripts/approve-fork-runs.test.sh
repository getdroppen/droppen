#!/usr/bin/env bash
# Tests for approve-fork-runs.sh against mocked API responses (a fake `gh`):
# `bash .github/scripts/approve-fork-runs.test.sh`.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
APPROVE=$HERE/approve-fork-runs.sh
TMP=$(mktemp -d "${TMPDIR:-/tmp}/approve-fork-runs-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
PASSED=0
FAILED=0
ok() { PASSED=$((PASSED + 1)); }
fail() { FAILED=$((FAILED + 1)); echo "FAIL: $*" >&2; }

SHA=94a4ffd32ac834bf708aff461394209e46e73438
LATER=1111111111111111111111111111111111111111
FORK_ID=500100
OTHER_ID=500200

# The fake gh: `api --paginate <url>` prints $FAKE_LIST (or fails when it is
# "fail"), `api -X POST <url>` succeeds unless the run id is in $FAKE_FAIL.
# Every call is logged to $FAKE_LOG.
cat > "$TMP/gh" <<'SH'
#!/usr/bin/env bash
echo "$*" >> "$FAKE_LOG"
if [ "$1 $2" = "api --paginate" ]; then
  if [ "$FAKE_LIST" = fail ]; then echo "HTTP 403: Resource not accessible by integration" >&2; exit 1; fi
  cat "$FAKE_LIST"
  exit 0
fi
if [ "$1 $2 $3" = "api -X POST" ]; then
  id=$(printf '%s' "$4" | sed -n 's|.*/actions/runs/\([^/]*\)/approve$|\1|p')
  case " ${FAKE_FAIL:-} " in *" $id "*) echo "HTTP 403: This workflow run is not waiting for approval" >&2; exit 1 ;; esac
  echo '{}'
  exit 0
fi
echo "unexpected call: $*" >&2
exit 2
SH
chmod +x "$TMP/gh"

run() { # expected exit (0|1), description, [env assignments...]; sets $LOG
  local want=$1 what=$2 got=0
  shift 2
  LOG=$TMP/log-$PASSED-$FAILED
  : > "$LOG"
  env GH="$TMP/gh" FAKE_LOG="$LOG" GH_REPO=getdroppen/droppen HEAD_SHA=$SHA HEAD_REPO_ID=$FORK_ID \
    FAKE_LIST="$TMP/list.json" FAKE_FAIL= GITHUB_STEP_SUMMARY= "$@" \
    bash "$APPROVE" > "$TMP/out" 2>&1 || got=$?
  [ "$got" = "$want" ] && ok || { fail "$what: exit $got, want $want"; cat "$TMP/out" >&2; }
}
approved() { { grep '^api -X POST' "$LOG" || true; } | sed 's|.*/actions/runs/\([0-9a-z]*\)/approve$|\1|' | sort | tr '\n' ' '; }
expect_approved() { # description, the run ids that must have been approved (sorted, space-separated)
  local got
  got=$(approved)
  [ "$got" = "$2" ] && ok || fail "$1: approved '$got', want '$2'"
}

# Waiting runs as the API lists them (the shape of a real response; two pages).
run_json() { # id, head_sha, head repository id, conclusion, name
  printf '{"id":%s,"name":"%s","event":"pull_request","status":"completed","conclusion":"%s","head_sha":"%s","head_branch":"fix","head_repository":{"id":%s,"full_name":"someone/droppen"},"pull_requests":[]}' \
    "$1" "$5" "$4" "$2" "$3"
}
{
  printf '{"total_count":5,"workflow_runs":[%s,%s,%s]}' \
    "$(run_json 101 $SHA $FORK_ID action_required ci)" \
    "$(run_json 102 $SHA $FORK_ID action_required pr-guard)" \
    "$(run_json 103 $LATER $FORK_ID action_required ci)"
  printf '{"total_count":5,"workflow_runs":[%s,%s]}' \
    "$(run_json 104 $SHA $OTHER_ID action_required ci)" \
    "$(run_json 105 $SHA $FORK_ID success ci)"
} > "$TMP/list.json"

run 0 "the reviewed commit's waiting runs"
expect_approved "only the reviewed commit's runs, from the pull request's repository" "101 102 "
grep -q "^api --paginate repos/getdroppen/droppen/actions/runs?head_sha=$SHA&status=action_required&per_page=100$" "$LOG" \
  && ok || fail "lists the waiting runs of exactly the reviewed commit"
grep -q 'skip run 103' "$TMP/out" && grep -q 'skip run 104' "$TMP/out" && ok || fail "says which runs it skipped"
grep -q 'run 105' "$TMP/out" && fail "a run that does not wait is not mentioned" || ok

run 0 "a later push's commit" HEAD_SHA=$LATER
expect_approved "a later push: only its own run" "103 "

echo '{"total_count":0,"workflow_runs":[]}' > "$TMP/empty.json"
run 0 "nothing waiting (a pull request from this repository)" FAKE_LIST="$TMP/empty.json"
expect_approved "nothing waiting" ""

run 1 "an approval that fails" FAKE_FAIL=101
expect_approved "the others are still approved" "101 102 "

run 1 "listing fails" FAKE_LIST=fail
expect_approved "listing fails: nothing approved" ""

for bad in 94a4ffd 94A4FFD32AC834BF708AFF461394209E46E73438 "$SHA;id" "$SHA "; do
  run 1 "a bad HEAD_SHA ($bad)" HEAD_SHA="$bad"
  [ -s "$LOG" ] && fail "a bad HEAD_SHA ($bad): calls the API" || ok
done
for bad in "" abc "1 2"; do
  run 1 "a bad HEAD_REPO_ID ($bad)" HEAD_REPO_ID="$bad"
  [ -s "$LOG" ] && fail "a bad HEAD_REPO_ID ($bad): calls the API" || ok
done

printf '{"workflow_runs":[{"id":"7/../../x","event":"pull_request","conclusion":"action_required","head_sha":"%s","head_repository":{"id":%s}}]}' $SHA $FORK_ID > "$TMP/odd.json"
run 1 "a run id that is not a number" FAKE_LIST="$TMP/odd.json"
expect_approved "a run id that is not a number: nothing approved" ""

echo "approve-fork-runs: $PASSED passed, $FAILED failed"
[ "$FAILED" = 0 ]
