#!/usr/bin/env bash
# Cut an RC release (vX.Y.Z-rc.N) from the branch you are on, without merging
# it: the operator half of "An RC from a pull request" (docs/releasing.md).
# The release workflow admits an RC tag whose commit is on an open pull request
# to main from this repository; this script does the steps in order and refuses
# before it writes anything when one of them cannot hold.
#
#   scripts/cut-rc.sh vX.Y.Z-rc.N
#   scripts/cut-rc.sh X.Y.Z-rc.N
#
# It refuses unless:
#  - it runs inside a git worktree, on a branch that is not main;
#  - that branch has an open pull request to main from this repository whose
#    head is exactly the current commit (the tag must point at the reviewed
#    commit, not one the branch has moved on from);
#  - the version is a vX.Y.Z-rc.N version newer than every existing
#    vX.Y.Z-rc.* tag, locally and on origin (equal is allowed, for a re-run),
#    and its tag exists in neither place;
#  - docs/releases/v<version>.md exists, is not empty, carries the bilingual
#    banner ("test release" and "测试版本") and names no app file (an RC is
#    server-only: scripts/publish-release.sh refuses an RC with the app);
#  - the working tree is clean but for that notes file (untracked or
#    modified).
#
# Then it bumps the server's version, commits it signed-off, pushes the branch,
# tags the commit `Droppen v<version>` and pushes the tag. A push that fails
# keeps the local commit and tag, so nothing is lost; it says so and exits 1.
set -euo pipefail

die() { printf 'cut-rc: %s\n' "$*" >&2; exit 1; }

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2
  exit 2
}

[ $# -eq 1 ] || usage
version=$1
[[ $version =~ ^v?[0-9]+\.[0-9]+\.[0-9]+-rc\.[0-9]+$ ]] \
  || die "$version is not an RC version (vX.Y.Z-rc.N)"
version=${version#v}
tag=v$version
base=${version%-rc.*}
notes=docs/releases/$tag.md

git rev-parse --is-inside-work-tree >/dev/null 2>&1 \
  || die "not inside a git worktree: run this from the checkout"
root=$(git rev-parse --show-toplevel)
cd "$root"

repo=${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)} \
  || die "cannot tell the repository (gh auth status?)"
[[ $repo =~ ^[^/[:space:]]+/[^/[:space:]]+$ ]] || die "not a repository name: $repo"
owner=${repo%%/*}

branch=$(git rev-parse --abbrev-ref HEAD)
[ "$branch" != HEAD ] || die "detached HEAD: check out the pull request's branch"
[ "$branch" != main ] || die "on main: an RC is cut from a pull request's branch, not main"
sha=$(git rev-parse HEAD)

# The pull request must exist, be open, be to main, come from this repository,
# and have this very commit as its head: an RC from a commit the branch has
# already moved past would not be the reviewed one.
prs=$(REPO=$repo SHA=$sha gh api --paginate "repos/$repo/pulls?state=open&head=$owner:$branch&per_page=100" \
  --jq '.[] | select(.state == "open" and .base.ref == "main" and .head.repo.full_name == env.REPO and .headRefOid == env.SHA) | [.number, .html_url] | @tsv') \
  || die "cannot read the pull requests of $repo (gh auth status?)"
first=$(printf '%s\n' "$prs" | head -n 1)
[ -n "$first" ] \
  || die "no open pull request to main for $branch with head $sha: open one, push the branch, and let it be reviewed first (a fork's pull request does not count)"
number=${first%%$'\t'*}
pr_url=${first#*$'\t'}
[[ $number =~ ^[0-9]+$ ]] || die "the API gave no pull request number: $first"

# The tag must not exist yet, here or on origin.
git rev-parse -q --verify "refs/tags/$tag" >/dev/null 2>&1 && die "$tag already exists locally"
remote_tags=$(git ls-remote --tags origin "refs/tags/$tag") || die "cannot read the tags of origin"
[ -z "$remote_tags" ] || die "$tag already exists on origin"

# Never move back: an RC older than one already made for this version would
# hide it from installs. Equal is allowed (re-running the same version).
all_tags=$(
  git tag -l "v$base-rc.*"
  git ls-remote --tags origin "refs/tags/v$base-rc.*" | sed -n 's|^[0-9a-f]*[[:space:]]*refs/tags/||p' | sed 's|\^{}$||'
) || die "cannot read the tags of origin"
all_tags=$(printf '%s\n' "$all_tags" | sed '/^$/d')
newest=$(printf '%s\n' "$all_tags" | sort -V | tail -n 1)
if [ -n "$newest" ] && [ "$(printf '%s\n%s\n' "$newest" "$tag" | sort -V | tail -n 1)" != "$tag" ]; then
  die "$tag is older than $newest: an RC of $base continues from it"
fi

# The notes: server-only, and billed as a test in both languages. The banner
# also keeps the prerelease out of what `scripts/publish-release.sh` treats as
# a releasable app's notes.
[ -s "$notes" ] || die "$notes is missing or empty: write the RC's notes first"
grep -qF "test release" "$notes" || die "$notes does not say \"test release\" (the English half of the banner)"
grep -qF "测试版本" "$notes" || die "$notes does not say \"测试版本\" (the Chinese half of the banner)"
plain=$base
for name in "Droppen-$plain.dmg" "Droppen-$plain-x86_64.AppImage" "Droppen-$plain-aarch64.AppImage" \
  "droppen_${plain}_amd64.deb" "droppen_${plain}_arm64.deb"; do
  if grep -qF "$name" "$notes"; then
    die "$notes names $name: a prerelease is server-only, so its notes must not name the app's files"
  fi
done

# Only the notes may be untracked or modified; the commit takes exactly the
# notes and the two version files, nothing stray along.
while IFS= read -r line; do
  [ -n "$line" ] || continue
  path=${line:3}
  [ "$path" = "$notes" ] || die "the working tree has other changes ($line): commit or put them aside first"
done <<< "$(git status --porcelain -uall)"

cargo=crates/droppen-server/Cargo.toml
lock=Cargo.lock
[ -f "$cargo" ] && [ -f "$lock" ] || die "run this from the repository root's checkout ($cargo or $lock is missing)"
work=$(mktemp -d "${TMPDIR:-/tmp}/cut-rc.XXXXXX")
trap 'rm -rf "$work"' EXIT

# The package's version, the first column-0 `version = ` line (the package's;
# dependencies are indented), as `sed -n 's/^version = //p'` in the workflow
# reads it.
awk -v v="$version" '!done && /^version = / { print "version = \"" v "\""; done = 1; next } { print }' "$cargo" > "$work/Cargo.toml" \
  || die "cannot rewrite $cargo"
grep -qx "version = \"$version\"" "$work/Cargo.toml" || die "$cargo has no version = line"

# Cargo.lock, only the droppen-server package's block: name and version sit
# together, and the block ends at the next one.
awk -v v="$version" '
  /^name = "droppen-server"$/ { package = 1; print; next }
  package && /^version = / { print "version = \"" v "\""; package = 0; done = 1; next }
  { print }
  END { if (!done) exit 3 }
' "$lock" > "$work/Cargo.lock" || die "$lock has no droppen-server package block"

mv "$work/Cargo.toml" "$cargo"
mv "$work/Cargo.lock" "$lock"

git add "$cargo" "$lock" "$notes"
git commit -s -m "chore: release $tag" || die "cannot commit the version bump"
git tag -a "$tag" -m "Droppen $tag" || die "cannot create the tag $tag"

failed=0
if ! git push origin "refs/heads/$branch"; then
  failed=1
  echo "cut-rc: the branch push failed; the local commit and tag $tag are kept. Push the branch, then: git push origin $tag" >&2
fi
if ! git push origin "refs/tags/$tag"; then
  failed=1
  echo "cut-rc: the tag push failed; the local commit and tag $tag are kept. Retry: git push origin $tag" >&2
fi

echo "pull request: $pr_url"
echo "actions: https://github.com/$repo/actions/workflows/release.yml"
[ "$failed" = 0 ] || exit 1
