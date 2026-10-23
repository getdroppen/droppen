#!/usr/bin/env bash
# Approve the workflow runs of one commit that wait for a maintainer's approval
# (the repository requires it for every outside contributor's pull request):
# the commit the automated security review has just passed
# (.github/workflows/llm-security-review.yml, docs/ci-security.md).
#
#   GH_REPO=getdroppen/droppen HEAD_SHA=<the reviewed commit> \
#   HEAD_REPO_ID=<id of the pull request's head repository> \
#     .github/scripts/approve-fork-runs.sh
#
# A run is approved only if it waits for approval (action_required), its
# head_sha is HEAD_SHA and its head repository is HEAD_REPO_ID. Runs of any
# other commit, a push after the review among them, wait for their own review.
# Needs GH_TOKEN with Actions: write on GH_REPO. GH (default gh) is the GitHub
# CLI; the test (approve-fork-runs.test.sh) puts a fake in its place.
set -euo pipefail

gh=${GH:-gh}
: "${GH_REPO:?}" "${HEAD_SHA:?}" "${HEAD_REPO_ID:?}"
if ! [[ $HEAD_SHA =~ ^[0-9a-f]{40}$ ]]; then
  echo "::error::HEAD_SHA is not a full commit sha: $HEAD_SHA" >&2
  exit 1
fi
if ! [[ $HEAD_REPO_ID =~ ^[0-9]+$ ]]; then
  echo "::error::HEAD_REPO_ID is not a repository id: $HEAD_REPO_ID" >&2
  exit 1
fi

# Every page, one JSON document each.
pages=$("$gh" api --paginate "repos/$GH_REPO/actions/runs?head_sha=$HEAD_SHA&status=action_required&per_page=100")

# One line per waiting run: id, whether it qualifies, and what it is.
listing=$(printf '%s' "$pages" | jq -r --arg sha "$HEAD_SHA" --argjson repo "$HEAD_REPO_ID" '
  .workflow_runs[]?
  | select(.conclusion == "action_required" or .status == "action_required")
  | [ (.id | tostring),
      (if .head_sha == $sha and .head_repository.id == $repo then "approve" else "skip" end),
      (.head_sha // "-"), ((.head_repository.full_name // "-") + "#" + ((.head_repository.id // "-") | tostring)),
      (.event // "-"), (.name // "-") ]
  | @tsv')

approved=0
failed=0
skipped=0
while IFS=$'\t' read -r id action sha repo event name; do
  [ -n "$id" ] || continue
  if ! [[ $id =~ ^[0-9]+$ ]]; then
    echo "::error::not a run id: $id" >&2
    failed=$((failed + 1))
    continue
  fi
  if [ "$action" != approve ]; then
    echo "skip run $id ($name, $event): commit $sha of $repo is not the reviewed one"
    skipped=$((skipped + 1))
    continue
  fi
  if out=$("$gh" api -X POST "repos/$GH_REPO/actions/runs/$id/approve" 2>&1); then
    echo "approved run $id ($name, $event)"
    approved=$((approved + 1))
  else
    echo "::warning::could not approve run $id ($name, $event): $out"
    failed=$((failed + 1))
  fi
done <<< "$listing"

echo "commit $HEAD_SHA: approved $approved waiting run(s), skipped $skipped, failed $failed"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  echo "Commit \`$HEAD_SHA\`: approved $approved waiting workflow run(s), skipped $skipped, failed $failed." >> "$GITHUB_STEP_SUMMARY"
fi
if [ "$failed" != 0 ]; then
  echo "::error::$failed waiting run(s) were not approved; a maintainer approves them in the pull request (Approve workflows to run)" >&2
  exit 1
fi
