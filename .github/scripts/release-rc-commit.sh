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
# The commit must be reachable from the head of an *open* pull request whose
# base is main and whose head repository is this repository: a fork's pull
# request is refused (its code is not this repository's, and a prerelease
# would run it with the release secrets without the main-side review a merge
# gives). The commit need not be the head: a maintainer re-runs a release after
# the branch moved on, and the tag still points at the commit that was there.
# The workflow calls this only when the commit is not on main; a commit on main
# is accepted there, so a merged rc still re-runs.
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

# One line per qualifying pull request: its number and URL, tab separated. The
# head repository is compared with this repository, which is what refuses a
# fork: .head.repo is null once a fork was deleted, hence the // "".
prs=$(gh api --paginate "repos/$GITHUB_REPOSITORY/commits/$sha/pulls" \
  --jq '.[] | select(.state == "open" and .base.ref == "main" and .head.repo.full_name == env.GITHUB_REPOSITORY) | [.number, .html_url] | @tsv') \
  || die "cannot read the pull requests of $sha in $GITHUB_REPOSITORY (gh auth status?)"

first=$(printf '%s\n' "$prs" | head -n 1)
if [ -z "$first" ]; then
  # Nothing qualified. Say why, from the same association, so a fork or a
  # pull request to another branch is not reported as "no pull request".
  all=$(gh api --paginate "repos/$GITHUB_REPOSITORY/commits/$sha/pulls" \
    --jq '.[] | [.number, .state, .base.ref, (.head.repo.full_name // "")] | @tsv') \
    || die "cannot read the pull requests of $sha in $GITHUB_REPOSITORY (gh auth status?)"
  fork="" open_to_main="" closed=""
  while IFS=$'\t' read -r number state base head; do
    [ -n "$number" ] || continue
    if [ "$state" != open ]; then
      closed="$number"
    elif [ "$base" != main ]; then
      open_to_main="$number (to $base)"
    elif [ "$head" != "$GITHUB_REPOSITORY" ]; then
      fork="$number (from ${head:-a deleted fork})"
    fi
  done <<< "$all"
  if [ -n "$fork" ]; then
    die "$sha is on the open pull request $fork, not from $GITHUB_REPOSITORY: a prerelease is released from this repository's own branch, not a fork"
  fi
  if [ -n "$open_to_main" ]; then
    die "$sha is on the open pull request $open_to_main, not to main: only a pull request to main is released"
  fi
  if [ -n "$closed" ]; then
    die "$sha is on pull request $closed only, which is closed: only an open pull request is released"
  fi
  # A fork's head commit is not associated with its pull request in this
  # repository's API (GitHub returns no association for it), so the fork case
  # above is a bonus and this message carries the advice.
  die "$sha is on no pull request: open one to main from a branch of this repository (push a fork's branch here first)"
fi

number=${first%%$'\t'*}
url=${first#*$'\t'}
[[ $number =~ ^[0-9]+$ ]] || die "the API gave no pull request number for $sha: $first"
[ -n "$url" ] || die "the API gave no pull request URL for $sha: $first"

echo "ok: $sha is on the open pull request #$number ($url)"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "pr=$number"
    echo "pr_url=$url"
  } >> "$GITHUB_OUTPUT"
fi
