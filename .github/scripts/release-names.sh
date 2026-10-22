#!/usr/bin/env bash
# The release text check's names, for release.yml's jobs that have them
# (sign-apple, publish): the `release` environment's secret
# RELEASE_FORBIDDEN_ENV, the release Mac's ~/.config/droppen/release-forbidden.env,
# written where the scripts read it (as data, never run), checked against the
# pinned hash (scripts/release-text-check.sh), and masked in the log in every
# case a text would spell them. Without the secret a dry run goes on with
# "Kilerd" only (DROPPEN_RELEASE_NAMES_DEFERRED, which it says), and a release
# (PUBLISH=true) stops.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
if [ -z "${RELEASE_FORBIDDEN_ENV:-}" ]; then
  if [ "${PUBLISH:-}" = true ]; then
    echo "::error::no RELEASE_FORBIDDEN_ENV secret: a release checks its text for the names (docs/releasing.md)"
    exit 1
  fi
  echo "::warning::no RELEASE_FORBIDDEN_ENV secret: this dry run checks the release's text for \"Kilerd\" only, not for the names; a release would stop here"
  echo "DROPPEN_RELEASE_NAMES_DEFERRED=1" >> "$GITHUB_ENV"
  exit 0
fi
FILE=$HOME/.config/droppen/release-forbidden.env
mkdir -p "$(dirname "$FILE")"
(umask 077 && printf '%s\n' "$RELEASE_FORBIDDEN_ENV" > "$FILE")
. "$ROOT/scripts/release-text-check.sh"
release_text_load_names "$FILE"
# The checks show line numbers only (DROPPEN_RELEASE_TEXT_QUIET); this masks
# the names themselves, as they are and in upper and title case.
while IFS= read -r name; do
  [ -n "$name" ] || continue
  echo "::add-mask::$name"
  echo "::add-mask::$(printf '%s' "$name" | tr '[:lower:]' '[:upper:]')"
  echo "::add-mask::$(printf '%s' "$name" | awk '{ for (i = 1; i <= NF; i++) $i = toupper(substr($i, 1, 1)) substr($i, 2) } 1')"
done <<<"$(release_text_names)"
release_text_names_ok || { echo "::error::RELEASE_FORBIDDEN_ENV is not the expected list of names (RELEASE_TEXT_NAMES_SHA256 in scripts/release-text-check.sh): set it again from ~/.config/droppen/release-forbidden.env"; exit 1; }
release_text_allow_ok || { echo "::error::RELEASE_FORBIDDEN_ENV's DROPPEN_RELEASE_NOTICE_ALLOW is refused (see above)"; exit 1; }
echo "ok: the names are the expected list"
