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
# area is true". A bug here can therefore waste a run, never hide a test. One
# exception: a .github path turns desktop_package (the Linux app's packaging,
# 20-25 minutes of two runners) on only if it is one of the packaging's own
# workflow, scripts and pins.
#
# The map follows what each job reads, not its directory:
#  - the web job's check:hub-api scans crates/droppen-agent/src,
#    crates/droppen-aggregator/src and crates/droppen-{relay,client,devkit}/src, and its
#    plugin-sdk test reads
#    crates/droppen-plugin-sdk/src/lib.rs (the SDK version);
#  - the web tests read droppen-proto's committed vectors and src/sig.rs, the two
#    lint scripts they import, SECURITY.md, and the repository paths the
#    security page cites (droppen-relay-core, droppen-client, droppen-relay, docs);
#  - the server's tests read openapi/openapi.json, droppen-plugin-sdk does
#    include_str! of packages/plugin-sdk/test/engine-vectors.json, two tests
#    read scripts/install.sh, and two steps run repository scripts;
#  - the release checks build the web and test scripts/ whose tests read
#    desktop/src-tauri/tauri.conf.json, install.sh, release-public.sh,
#    install-key.sh, LICENSE, scripts/notices/attributions.txt and
#    web/owner-placeholders.txt;
#  - the desktop core tests read desktop/build.sh, desktop/e2e-identity.sh,
#    scripts/install.sh, the root Cargo.toml and
#    crates/droppen-search-path (the path dependency it compiles,
#    which inherits that workspace's lints);
#  - the Linux app's packaging (desktop-linux.yml) runs desktop/build-linux.sh
#    and desktop/linux/, which read the app's Tauri configuration, build
#    script, icons and lockfiles, desktop/build.sh (the tauri-cli version),
#    the release scripts they source and scripts/release-public.sh (the
#    cargo-about version), and makes the notices (desktop/third-party-notices.sh,
#    about.toml, scripts/notices/) and the server (scripts/build-linux*.sh). It
#    does not follow the app's or the server's code: those have their jobs.
# Everything a rule does not name is unknown, and unknown runs every job.
set -u

# One output per job that can be skipped. secrets-scan is not here: it runs on
# every pull request, whatever changed.
AREAS="web web_tests ui_audit web_reproducible server release_tooling desktop_core desktop_package"

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

web=0 web_tests=0 ui_audit=0 web_reproducible=0 server=0 release_tooling=0 desktop_core=0 desktop_package=0
unknown=""
github=""

while IFS= read -r path; do
  [ -n "$path" ] || continue

  # A workflow or script of this repository: rerun all of it, but for the
  # packaging, which only its own files there rerun.
  case "$path" in
    .github/*)
      github=$path
      case "$path" in
        .github/workflows/desktop-linux.yml|.github/scripts/smoke-desktop-linux.sh|.github/scripts/unpack-artifact.sh|.github/scripts/fetch-release-tool.sh|.github/release-tools.lock)
          desktop_package=1 ;;
      esac
      continue ;;
  esac

  # Prose at the top of a tree whose jobs read code there, not these files.
  # Exact names only: a `crates/droppen-server/*.md` glob would also swallow
  # crates/droppen-server/src/notify/SKILL.md, which inject.rs embeds with
  # include_str!.
  case "$path" in
    desktop/README.md|desktop/SIGNING.md|crates/droppen-server/README.md) continue ;;
  esac

  matched=0

  case "$path" in
    web/*|packages/*|package.json|pnpm-lock.yaml|pnpm-workspace.yaml|.nvmrc|e2e/package.json|distribution.json|openapi/*|docs/hub-api/*|scripts/check-plugin-boundary.mjs|scripts/check-machine-keys.mjs|scripts/check-open-boundary.mjs|scripts/check-open-boundary.test.mjs|crates/droppen-agent/src/*|crates/droppen-aggregator/src/*|crates/droppen-relay-core/src/*|crates/droppen-client/src/*|crates/droppen-devkit/src/*|crates/droppen-plugin-sdk/*)
      web=1; matched=1 ;;
  esac

  # The web tests also read droppen-proto's vectors and sources (the interop and the
  # security page's code references), the crates and repository paths that page
  # links to (it stats them), SECURITY.md, the distribution manifest, and the
  # two lint scripts they import.
  case "$path" in
    web/*|packages/*|package.json|pnpm-lock.yaml|pnpm-workspace.yaml|.nvmrc|e2e/package.json|distribution.json|SECURITY.md|docs/web-integrity.md|scripts/check-machine-keys.mjs|scripts/check-plugin-boundary.mjs|scripts/install.sh|scripts/verify-web.mjs|crates/droppen-proto/*|crates/droppen-relay-core/*|crates/droppen-client/*|crates/droppen-relay/*|crates/droppen-provider/src/drafts.rs)
      web_tests=1; matched=1 ;;
  esac

  # Other docs are prose too, but the security page's formula data cites
  # repository files by path and line, and its test reads them: run the web
  # tests (about a minute). docs/hub-api and docs/releases have their own
  # jobs, above.
  case "$path" in
    docs/hub-api/*|docs/releases/*) ;;
    docs/*) web_tests=1; matched=1 ;;
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
    crates/*|Cargo.toml|Cargo.lock|rustfmt.toml|layers.json|.config/*|.cargo/*|openapi/openapi.json|packages/plugin-sdk/test/engine-vectors.json|scripts/check-open-boundary.mjs|scripts/check-layers.mjs|scripts/check-layers.test.mjs|scripts/check-sdk-crate-deps.sh|scripts/check-serde-json-order.sh|scripts/install.sh)
      server=1; matched=1 ;;
  esac

  case "$path" in
    scripts/publish-release.sh|scripts/publish-release.test.mjs|scripts/verify-web.mjs|scripts/verify-web.test.mjs|scripts/web-manifest.mjs|scripts/release-notes.sh|scripts/release-notes.test.mjs|scripts/release-public.sh|scripts/install.sh|scripts/install.test.mjs|scripts/install-key.sh|scripts/release-text-check.sh|scripts/release-text-check.test.sh|scripts/notices/attributions.txt|LICENSE|web/owner-placeholders.txt|docs/releases/*|desktop/src-tauri/tauri.conf.json)
      release_tooling=1; matched=1 ;;
  esac

  case "$path" in
    desktop/*|crates/droppen-search-path/*|scripts/install.sh|Cargo.toml)
      desktop_core=1; matched=1 ;;
  esac

  case "$path" in
    desktop/linux/*|desktop/build-linux.sh|desktop/build.sh|desktop/third-party-notices.sh|desktop/Cargo.toml|desktop/Cargo.lock|desktop/src-tauri/Cargo.toml|desktop/src-tauri/build.rs|desktop/src-tauri/build_config.rs|desktop/src-tauri/tauri.conf.json|desktop/src-tauri/tauri.linux.conf.json|desktop/src-tauri/icons/*|scripts/build-linux.sh|scripts/build-linux-in-container.sh|scripts/release-lib.sh|scripts/release-text-check.sh|scripts/release-public.sh|scripts/notices/*|about.toml|LICENSE)
      desktop_package=1; matched=1 ;;
  esac

  if [ "$matched" = 0 ]; then
    case "$path" in
      # Nothing any job reads: prose and repository metadata, named exactly.
      # Anything else, a script included, is unknown and runs every job.
      README.md|README.zh-CN.md|CONTRIBUTING.md|about.toml|.gitleaks.toml|.gitignore|.dockerignore|.claude/*|e2e/*) ;;
      *) unknown=$path ;;
    esac
  fi
done <<< "$changes"

if [ -n "$unknown" ]; then
  everything "no rule knows $unknown"
  exit 0
fi

bool() { if [ "$1" = 1 ]; then echo true; else echo false; fi; }

if [ -n "$github" ]; then
  echo "ci-changes: running every job, the packaging's only for its own files (a .github path changed: $github)" >&2
  for area in $AREAS; do
    [ "$area" = desktop_package ] || emit "$area=true"
  done
  emit "desktop_package=$(bool "$desktop_package")"
  exit 0
fi

emit "web=$(bool "$web")"
emit "web_tests=$(bool "$web_tests")"
emit "ui_audit=$(bool "$ui_audit")"
emit "web_reproducible=$(bool "$web_reproducible")"
emit "server=$(bool "$server")"
emit "release_tooling=$(bool "$release_tooling")"
emit "desktop_core=$(bool "$desktop_core")"
emit "desktop_package=$(bool "$desktop_package")"
