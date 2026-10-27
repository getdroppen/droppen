#!/usr/bin/env bash
# Which jobs of .github/workflows/ci.yml a change set needs.
#
# Every output is one area, `area=true|false`. The workflow runs a job when its
# area is true; a job that is skipped reports "skipped", which GitHub counts as
# success for a required check (so a docs-only pull request still merges).
#
#   ci-changes.sh                 # the pull request (or push) that started this run
#   ci-changes.sh --paths FILE    # one path per line, to check the map locally
#
# It writes the lines to $GITHUB_OUTPUT when that is set, and always to stdout.
#
# Fail open: a job runs unless this script can prove it is unnecessary. A push,
# a manual run, a missing or unreadable base/head, a git error, an empty diff, a
# path under .github/ and a changed path no rule below knows all mean "every
# area is true". A bug here can therefore waste a run, never hide a test.
#
# The map follows what each job reads, not its directory:
#  - the web job's check:hub-api scans server-rs/src/{agent,aggregator} and
#    rh-{relay,client,devkit}/src, and check:boundary runs its own test;
#  - the web tests read rh-proto's committed vectors and src/sig.rs;
#  - the server's tests read openapi/openapi.json, droppen-plugin-sdk does
#    include_str! of packages/plugin-sdk/test/engine-vectors.json, two tests
#    read scripts/install.sh, and two steps run repository scripts;
#  - the release checks build the web and test scripts/ whose tests read
#    desktop/src-tauri/tauri.conf.json, install.sh, release-public.sh and
#    install-key.sh;
#  - the desktop core tests include desktop/build.sh and scripts/install.sh.
set -u

# One output per job that can be skipped. secrets-scan is not here: it runs on
# every pull request, whatever changed.
AREAS="web web_tests ui_audit web_reproducible server open_boundary release_tooling desktop_core"

paths_file=""
case "${1:-}" in
  --paths)
    paths_file=${2:-}
    [ -n "$paths_file" ] && [ -r "$paths_file" ] || { echo "ci-changes: --paths needs a readable file" >&2; exit 2; }
    ;;
  "") ;;
  *) echo "ci-changes: unknown argument: $1" >&2; exit 2 ;;
esac

emit() { # area=value
  printf '%s\n' "$1"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then printf '%s\n' "$1" >>"$GITHUB_OUTPUT"; fi
}

# Every area true, with the reason. The only output on any uncertain path.
everything() {
  echo "ci-changes: running every job ($1)" >&2
  for area in $AREAS; do
    emit "$area=true"
  done
}

# The changed paths, one per line, with no rename folding (a rename shows its
# old and its new path, so neither side is missed). Any failure is fatal to
# detection and everything runs.
changed_paths() {
  if [ -n "$paths_file" ]; then
    sed -e 's/\r$//' "$paths_file"
    return
  fi
  local event=${DROPPEN_CI_EVENT_NAME:-}
  if [ "$event" != "pull_request" ]; then
    return 1
  fi
  local base=${DROPPEN_CI_BASE_SHA:-} head=${DROPPEN_CI_HEAD_SHA:-}
  [ -n "$base" ] && [ -n "$head" ] || return 1
  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 1
  git cat-file -e "$base^{commit}" 2>/dev/null || return 1
  git cat-file -e "$head^{commit}" 2>/dev/null || return 1
  local merge_base
  merge_base=$(git merge-base "$base" "$head" 2>/dev/null) || return 1
  [ -n "$merge_base" ] || return 1
  git diff --name-only --no-renames "$merge_base" "$head" 2>/dev/null || return 1
}

changes=$(changed_paths) || { everything "the pull request's diff cannot be read"; exit 0; }
[ -n "$changes" ] || { everything "the diff is empty"; exit 0; }

web=0 web_tests=0 ui_audit=0 web_reproducible=0 server=0 open_boundary=0 release_tooling=0 desktop_core=0
unknown=""

while IFS= read -r path; do
  [ -n "$path" ] || continue

  # A workflow or script of this repository: rerun all of it.
  case "$path" in
    .github/*) everything "a .github path changed: $path"; exit 0 ;;
  esac

  # Prose at the top of a tree whose jobs read code there, not these files.
  case "$path" in
    desktop/*.md|server-rs/*.md) continue ;;
  esac

  matched=0

  case "$path" in
    web/*|packages/*|package.json|pnpm-lock.yaml|pnpm-workspace.yaml|.nvmrc|e2e/package.json|distribution.json|openapi/*|docs/hub-api/*|scripts/check-plugin-boundary.mjs|scripts/check-machine-keys.mjs|scripts/check-open-boundary.mjs|scripts/check-open-boundary.test.mjs|server-rs/src/agent/*|server-rs/src/aggregator/*|server-rs/crates/rh-relay/src/*|server-rs/crates/rh-client/src/*|server-rs/crates/rh-devkit/src/*)
      web=1; matched=1 ;;
  esac

  # The open-source boundary check scans these trees for anything of the hub's.
  case "$path" in
    web/src/*|web/test/*|packages/plugin-sdk/src/*|scripts/*|server-rs/*)
      open_boundary=1; matched=1 ;;
  esac

  # The web tests also read rh-proto's vectors and sources (the interop and the
  # security page's code references), the crates and repository paths that page
  # links to (it stats them), SECURITY.md, and the distribution manifest.
  case "$path" in
    web/*|packages/*|package.json|pnpm-lock.yaml|pnpm-workspace.yaml|.nvmrc|e2e/package.json|distribution.json|SECURITY.md|docs/web-integrity.md|scripts/install.sh|scripts/verify-web.mjs|server-rs/crates/rh-proto/*|server-rs/crates/rh-relay/*|server-rs/crates/rh-client/*|server-rs/crates/droppen-relay/*|server-rs/src/inject/drafts.rs)
      web_tests=1; matched=1 ;;
  esac

  case "$path" in
    web/*|packages/*|package.json|pnpm-lock.yaml|pnpm-workspace.yaml|.nvmrc|e2e/package.json|distribution.json|scripts/ui-audit/*|docs/ui-spec.md)
      ui_audit=1; matched=1 ;;
  esac

  case "$path" in
    web/*|packages/*|package.json|pnpm-lock.yaml|pnpm-workspace.yaml|.nvmrc|e2e/package.json|distribution.json|scripts/web-manifest.mjs|scripts/verify-web.mjs|scripts/verify-web.test.mjs|scripts/install.sh|scripts/install.test.mjs|scripts/publish-release.sh|scripts/publish-release.test.mjs|scripts/release-notes.sh|scripts/release-notes.test.mjs|scripts/release-public.sh|scripts/install-key.sh|desktop/src-tauri/tauri.conf.json)
      web_reproducible=1; matched=1 ;;
  esac

  case "$path" in
    server-rs/*|openapi/openapi.json|packages/plugin-sdk/test/engine-vectors.json|scripts/check-open-boundary.mjs|scripts/check-sdk-crate-deps.sh|scripts/install.sh)
      server=1; matched=1 ;;
  esac

  case "$path" in
    scripts/publish-release.sh|scripts/publish-release.test.mjs|scripts/verify-web.mjs|scripts/verify-web.test.mjs|scripts/web-manifest.mjs|scripts/release-notes.sh|scripts/release-notes.test.mjs|scripts/release-public.sh|scripts/install.sh|scripts/install.test.mjs|scripts/install-key.sh|scripts/release-text-check.sh|scripts/release-text-check.test.sh|docs/releases/*|desktop/src-tauri/tauri.conf.json)
      release_tooling=1; matched=1 ;;
  esac

  case "$path" in
    desktop/*|scripts/install.sh)
      desktop_core=1; matched=1 ;;
  esac

  if [ "$matched" = 0 ]; then
    case "$path" in
      # Nothing any job reads: prose and repository metadata.
      docs/*|README.md|README.zh-CN.md|CONTRIBUTING.md|SECURITY.md|LICENSE|about.toml|.gitleaks.toml|.gitignore|.dockerignore|.claude/*|e2e/*) ;;
      *) unknown=$path ;;
    esac
  fi
done <<< "$changes"

if [ -n "$unknown" ]; then
  everything "no rule knows $unknown"
  exit 0
fi

bool() { if [ "$1" = 1 ]; then echo true; else echo false; fi; }
emit "web=$(bool "$web")"
emit "web_tests=$(bool "$web_tests")"
emit "ui_audit=$(bool "$ui_audit")"
emit "web_reproducible=$(bool "$web_reproducible")"
emit "server=$(bool "$server")"
emit "open_boundary=$(bool "$open_boundary")"
emit "release_tooling=$(bool "$release_tooling")"
emit "desktop_core=$(bool "$desktop_core")"
