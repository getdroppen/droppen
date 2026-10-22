#!/usr/bin/env bash
# For release.yml's verify, attest and publish jobs: the staged release and,
# with the desktop app (DESKTOP=true), the signed DMG and the updater's
# files, each checked and unpacked by unpack-artifact.sh (the shas are the
# producing jobs' outputs: STAGED_SHA, SIGNED_SHA, UPDATER_SHA) and laid out
# as scripts/publish-release.sh takes them:
#
#   $RUNNER_TEMP/a/staged/release   its --server
#   $RUNNER_TEMP/a/desktop          its --desktop: Droppen-<x.y.z>.dmg, updater/
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
DL=$RUNNER_TEMP/dl
A=$RUNNER_TEMP/a
"$HERE/unpack-artifact.sh" "$DL/staged/staged.tar" "${STAGED_SHA:-}" "$A/staged" "$ALLOW_STAGED"
if [ "${DESKTOP:-}" = true ]; then
  "$HERE/unpack-artifact.sh" "$DL/signed-app/signed-app.tar" "${SIGNED_SHA:-}" "$A/signed" "$ALLOW_SIGNED"
  "$HERE/unpack-artifact.sh" "$DL/updater/updater.tar" "${UPDATER_SHA:-}" "$A/desktop" "$ALLOW_UPDATER"
  mv "$A/signed/Droppen-${PLAIN:?}.dmg" "$A/desktop/"
fi
