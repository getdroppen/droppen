#!/usr/bin/env bash
# Tests for release-rc-commit.sh against mocked commit→pull-request responses
# (a fake `gh`, which applies the script's --jq filter with the real jq):
# `bash .github/scripts/release-rc-commit.test.sh`.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$HERE/release-rc-commit.sh
TMP=$(mktemp -d "${TMPDIR:-/tmp}/release-rc-commit-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
PASSED=0
FAILED=0
N=0
ok() { PASSED=$((PASSED + 1)); }
fail() { FAILED=$((FAILED + 1)); echo "FAIL: $*" >&2; }

REPO=getdroppen/droppen
SHA=94a4ffd32ac834bf708aff461394209e46e73438
UPPER=94A4FFD32AC834BF708AFF461394209E46E73438

# The fake gh: `api --paginate <url> --jq <filter>` runs jq with the filter
# against $FAKE_JSON; every call is logged to $FAKE_LOG.
cat > "$TMP/gh" <<'SH'
#!/usr/bin/env bash
echo "$*" >> "$FAKE_LOG"
if [ "$1" = api ] && [ "$2" = --paginate ]; then
  GITHUB_REPOSITORY=$FAKE_REPO jq -r "$5" "$FAKE_JSON"
  exit 0
fi
echo "unexpected call: $*" >&2
exit 2
SH
chmod +x "$TMP/gh"

# pr_json: number, state, base ref, head repository full name
pr_json() {
  printf '{"number":%s,"state":"%s","base":{"ref":"%s"},"head":{"repo":{"full_name":"%s"}},"html_url":"https://github.com/getdroppen/droppen/pull/%s"}' \
    "$1" "$2" "$3" "$4" "$1"
}
fixture() { # pr_json args...
  local out="[" sep=""
  for pr in "$@"; do out="$out$sep$pr"; sep=","; done
  printf '%s]' "$out" > "$TMP/pulls.json"
}

run() { # expected exit (0|1), description, sha, [fixture pr args...]; sets $OUT and $LOG
  local want=$1 what=$2 sha=$3 got=0
  shift 3
  fixture "$@"
  LOG=$TMP/log-$PASSED-$FAILED
  OUT=$TMP/out-$PASSED-$FAILED
  : > "$LOG"
  : > "$TMP/output"
  env PATH="$TMP:$PATH" FAKE_LOG="$LOG" FAKE_JSON="$TMP/pulls.json" FAKE_REPO=$REPO \
    GITHUB_REPOSITORY=$REPO GH_TOKEN=fake GITHUB_OUTPUT=$TMP/output \
    bash "$SCRIPT" "$sha" > "$OUT" 2>&1 || got=$?
  [ "$got" = "$want" ] && ok || { fail "$what: exit $got, want $want"; cat "$OUT" >&2; }
}

# An open pull request to main from this repository passes and is reported.
run 0 "an open same-repository pull request to main" "$SHA" "$(pr_json 41 open main "$REPO")"
grep -q "^ok: $SHA is on the open pull request #41 (https://github.com/getdroppen/droppen/pull/41)$" "$OUT" \
  && ok || fail "prints the commit and the pull request"
grep -qx 'pr=41' "$TMP/output" && grep -qx 'pr_url=https://github.com/getdroppen/droppen/pull/41' "$TMP/output" \
  && ok || fail "writes pr and pr_url to GITHUB_OUTPUT"
grep -q "commits/$SHA/pulls" "$LOG" && ok || fail "queries the commit's pull requests"
grep -q 'pull request #41' "$OUT" && ok || fail "names the pull request"

# The first qualifying pull request wins when several qualify.
run 0 "the first of several open pull requests" "$SHA" \
  "$(pr_json 42 open main "$REPO")" "$(pr_json 43 open main "$REPO")"
grep -q '#42' "$OUT" && ok || fail "takes the first line"

# An upper-case sha is accepted and reported lower-case.
run 0 "an upper-case sha" "$UPPER" "$(pr_json 44 open main "$REPO")"
grep -q "^ok: $SHA is on" "$OUT" && ok || fail "lower-cases the sha"

# No pull request at all fails.
run 1 "no pull request" "$SHA"
grep -q '::error::' "$OUT" && ok || fail "reports the failure as an error"
grep -q 'no pull request' "$OUT" && ok || fail "says there is no pull request"

# A closed pull request fails.
run 1 "a closed pull request" "$SHA" "$(pr_json 45 closed main "$REPO")"
grep -q '::error::.*closed' "$OUT" && ok || fail "says the pull request is closed"

# A fork's pull request fails, and says so.
run 1 "a fork's pull request" "$SHA" "$(pr_json 46 open main someone/droppen)"
grep -q '::error::.*not from' "$OUT" && ok || fail "says the pull request is from a fork"

# A pull request to another branch fails.
run 1 "a pull request to another branch" "$SHA" "$(pr_json 47 open release "$REPO")"
grep -q '::error::.*not to main' "$OUT" && ok || fail "says the base is not main"

# An open qualifying pull request beside others is enough.
run 0 "one qualifying pull request among others" "$SHA" \
  "$(pr_json 48 closed main "$REPO")" "$(pr_json 49 open main someone/droppen)" "$(pr_json 50 open main "$REPO")"
grep -q '#50' "$OUT" && ok || fail "picks the qualifying pull request"

# --- merged pull requests ---------------------------------------------------
# A squash merge leaves the tagged branch commit on no main: the script takes
# a merged pull request only if this commit is behind its head and its merge
# commit is on origin/main, from real git.

mbegin() { # a bare origin and a clone, a main branch and feat as the PR head
  N=$((N + 1))
  MORIGIN=$TMP/morigin-$N.git
  MREPO=$TMP/mclone-$N
  PRNO=$((N + 50))
  git init -q --bare "$MORIGIN"
  git clone -q "$MORIGIN" "$MREPO" 2>/dev/null
  cd "$MREPO"
  git config user.email test@example.com
  git config user.name Test
  echo base > f; git add f; git commit -q -m base; git push -q origin HEAD:main
  git checkout -q -b feat; echo b > b; git add b; git commit -q -m b; git push -q origin feat
  MSHA=$(git rev-parse HEAD)
  git push -q origin "HEAD:refs/pull/$PRNO/head"
  git checkout -q main; echo m > m; git add m; git commit -q -m merge; git push -q origin main
  MMERGE=$(git rev-parse HEAD)
  git checkout -q feat
  git fetch -q origin
}

merged_pr_json() { # number, merge commit sha, head repository
  printf '{"number":%s,"state":"closed","merged_at":"2026-01-01T00:00:00Z","merge_commit_sha":"%s","base":{"ref":"main"},"head":{"repo":{"full_name":"%s"}},"html_url":"https://github.com/getdroppen/droppen/pull/%s"}' \
    "$1" "$2" "$3" "$1"
}

mbegin
run 0 "a merged pull request" "$MSHA" "$(merged_pr_json "$PRNO" "$MMERGE" "$REPO")"
grep -q 'was merged in pull request' "$OUT" && ok || fail "accepts a merged pull request"

mbegin
git checkout -q -b later; echo l > l; git add l; git commit -q -m later
git push -q origin "HEAD:refs/pull/$PRNO/head"
run 0 "a commit behind a merged pull request's head" "$MSHA" "$(merged_pr_json "$PRNO" "$MMERGE" "$REPO")"
grep -q 'was merged in pull request' "$OUT" && ok || fail "accepts a commit the merged head carries"

mbegin
run 1 "a merged pull request whose merge is not on main" "$MSHA" "$(merged_pr_json "$PRNO" 1111111111111111111111111111111111111111 "$REPO")"
grep -q 'did not take it into main' "$OUT" && ok || fail "refuses a merge that is not on main"

mbegin
git checkout -q -b other; echo o > o; git add o; git commit -q -m other
run 1 "a commit ahead of the merged pull request's head" "$(git rev-parse HEAD)" "$(merged_pr_json "$PRNO" "$MMERGE" "$REPO")"
grep -q 'did not take it into main' "$OUT" && ok || fail "refuses a commit the merged head does not carry"

mbegin
run 1 "a merged pull request from a fork" "$MSHA" "$(merged_pr_json "$PRNO" "$MMERGE" someone/droppen)"
grep -q 'not from' "$OUT" && ok || fail "refuses a merged fork"

mbegin
run 1 "a closed, unmerged pull request" "$MSHA" "$(pr_json "$PRNO" closed main "$REPO")"
grep -q 'did not take it into main' "$OUT" && ok || fail "refuses a closed, unmerged pull request"

# A sha that is not 40 hexadecimal digits fails before any API call.
run 1 "a short sha" 94a4ffd
grep -q '::error::\|release-rc-commit:.*not a full commit sha' "$OUT" && ok || fail "refuses a short sha"
[ -s "$LOG" ] && fail "the API is not called for a bad sha" || ok
run 1 "a non-hex sha" 94a4ffd32ac834bf708aff461394209e46e7343z
grep -q 'not a full commit sha' "$OUT" && ok || fail "refuses a non-hex sha"

echo "release-rc-commit: $PASSED passed, $FAILED failed"
[ "$FAILED" = 0 ]
