#!/usr/bin/env bash
# Is this commit on an open pull request to main, from this repository? A
# prerelease (vX.Y.Z-rc.N) is released from a pull request's branch, so that a
# branch can be tried without merging it (docs/releasing.md, "An RC from a
# pull request"); a stable release still has to be on main, which this does not
# check (the workflow's stable path still does).
#
#   GITHUB_REPOSITORY=getdroppen/droppen GH_TOKEN=... \
#     .github/scripts/release-rc-commit.sh <sha>
#
# The commit must be reachable from the head of a pull request whose base is
# main and whose head repository is this repository, and that pull request must
# be open or merged into main: a fork's pull request is refused (its code is not
# this repository's, and a prerelease would run it with the release secrets
# without the main-side review a merge gives). The commit need not be the head:
# a maintainer re-runs a release after the branch moved on, and the tag still
# points at the commit that was there. The workflow calls this only when the
# commit is not on main (it accepts a commit on main itself, which is how a tag
# that points at the squash commit re-runs); a merged pull request is accepted
# because a squash merge leaves the tagged branch commit on no main at all.
#
# origin/main must be fetched already: a merged pull request's merge commit is
# checked against it.
#
# GitHub's commit→pull-request association is queried, every page of it
# (--paginate); the first qualifying pull request is the one reported. On
# success it prints the pull request and, when GITHUB_OUTPUT is set, appends
# `pr` and `pr_url` to it. On failure it writes one ::error:: with the reason
# and exits 1.
set -euo pipefail

die() { printf '::error::release-rc-commit: %s\n' "$*" >&2; exit 1; }

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2
  exit 2
}

[ $# -eq 1 ] || usage
sha=$1
[ -n "${GITHUB_REPOSITORY:-}" ] || die "GITHUB_REPOSITORY is not set, e.g. getdroppen/droppen"
[ -n "${GH_TOKEN:-}" ] || die "GH_TOKEN is not set"
[[ $sha =~ ^[0-9a-fA-F]{40}$ ]] || die "$sha is not a full commit sha (40 hexadecimal digits)"
# The API takes the lower-case form; a person may paste an upper-case one.
sha=$(printf '%s' "$sha" | tr 'A-F' 'a-f')

# One line per qualifying pull request: number, URL, state and merge commit,
# tab separated. A pull request qualifies while open, or once merged: a squash
# merge (this repository's) leaves the tagged branch commit on no main, so the
# merge commit is what says it reached main. The head repository is compared
# with this repository, which is what refuses a fork: .head.repo is null once a
# fork was deleted, hence the // "".
prs=$(gh api --paginate "repos/$GITHUB_REPOSITORY/commits/$sha/pulls" \
  --jq '.[] | select(.base.ref == "main" and .head.repo.full_name == env.GITHUB_REPOSITORY and (.state == "open" or .merged_at != null)) | [.number, .html_url, .state, (.merge_commit_sha // "")] | @tsv') \
  || die "cannot read the pull requests of $sha in $GITHUB_REPOSITORY (gh auth status?)"

# The first one that holds: an open pull request as it is; a merged one only if
# this commit is part of it and its merge commit is on main. The caller has
# fetched origin/main (the on-main check before this). Fetching the pull
# request's head is what tests "part of it": the head may have moved on after
# the tag, and this commit must still be behind it.
number=""
url=""
merged=0
while IFS=$'\t' read -r n u state merge_sha; do
  [ -n "$n" ] || continue
  if [ "$state" = open ]; then
    number=$n; url=$u; break
  fi
  [ -n "$merge_sha" ] || continue
  git fetch --no-tags --filter=blob:none origin "+refs/pull/$n/head:refs/remotes/origin/pr/$n" >/dev/null 2>&1 || continue
  git merge-base --is-ancestor "$sha" "refs/remotes/origin/pr/$n" 2>/dev/null || continue
  git merge-base --is-ancestor "$merge_sha" origin/main 2>/dev/null || continue
  number=$n; url=$u; merged=1; break
done <<< "$prs"

if [ -n "$number" ]; then
  if [ "$merged" = 1 ]; then
    echo "ok: $sha was merged in pull request #$number ($url), whose merge is on main"
  else
    echo "ok: $sha is on the open pull request #$number ($url)"
  fi
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    {
      echo "pr=$number"
      echo "pr_url=$url"
    } >> "$GITHUB_OUTPUT"
  fi
  exit 0
fi

# Nothing qualified. Say why, from the same association, so a fork or a pull
# request to another branch is not reported as "no pull request".
all=$(gh api --paginate "repos/$GITHUB_REPOSITORY/commits/$sha/pulls" \
  --jq '.[] | [.number, .state, .base.ref, (.head.repo.full_name // "")] | @tsv') \
  || die "cannot read the pull requests of $sha in $GITHUB_REPOSITORY (gh auth status?)"
fork="" other_base="" closed=""
while IFS=$'\t' read -r n state base head; do
  [ -n "$n" ] || continue
  if [ "$base" != main ]; then
    other_base="$n (to $base)"
  elif [ "$head" != "$GITHUB_REPOSITORY" ]; then
    fork="$n (from ${head:-a deleted fork})"
  elif [ "$state" != open ]; then
    closed="$n"
  fi
done <<< "$all"
if [ -n "$fork" ]; then
  die "$sha is on pull request $fork, not from $GITHUB_REPOSITORY: a prerelease is released from this repository's own branch, not a fork"
fi
if [ -n "$other_base" ]; then
  die "$sha is on the open pull request $other_base, not to main: only a pull request to main is released"
fi
if [ -n "$closed" ]; then
  die "$sha is on pull request $closed only, which is closed and did not take it into main: only an open pull request, or one merged into main, is released"
fi
# A fork's head commit is not associated with its pull request in this
# repository's API (GitHub returns no association for it), so the fork case
# above is a bonus and this message carries the advice.
die "$sha is on no pull request: open one to main from a branch of this repository (push a fork's branch here first)"
