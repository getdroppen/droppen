#!/usr/bin/env bash
# NOTES.md, the text of a release (its page on GitHub, the install command
# included): what the files are, then what changed for people
# (docs/releases/<version>.md). scripts/release-public.sh stages it;
# .github/workflows/release.yml's verify job writes it again from the source
# and refuses a staged NOTES.md that differs by a byte (the stage job runs
# npm install scripts).
#
#   scripts/release-notes.sh <version> <commit> <rev> <server version> <linux: 0|1> [<release notes file>]
#
# Prints it. On GitHub Actions (GITHUB_ACTIONS=true) the files' table names
# the attestation bundles too.
set -euo pipefail

VERSION=${1:?usage: release-notes.sh <version> <commit> <rev> <server version> <linux: 0|1> [<notes>]}
SHA=${2:?commit}
REV=${3:?rev}
CRATE_VERSION=${4:?server version}
LINUX=${5:?linux: 0 or 1}
NOTES_FILE=${6:-}
HERE=$(cd "$(dirname "$0")" && pwd)
NEW_NAME=droppen
# release-public.sh's, so the two say the same.
GLIBC_MAX=$(sed -n 's/^GLIBC_MAX=//p' "$HERE/release-public.sh")
OLD_ASSETS_UNTIL=$(sed -n 's/^OLD_ASSETS_UNTIL=//p' "$HERE/release-public.sh")
[ -n "$GLIBC_MAX" ] && [ -n "$OLD_ASSETS_UNTIL" ] || { echo "release-notes: cannot read GLIBC_MAX and OLD_ASSETS_UNTIL in release-public.sh" >&2; exit 2; }

ROWS="| \`$NEW_NAME-${VERSION}-macos-aarch64.tar.gz\` | Apple silicon |
| \`$NEW_NAME-${VERSION}-macos-x86_64.tar.gz\` | Intel Mac |
| \`$NEW_NAME-${VERSION}-macos-universal.tar.gz\` | either Mac |"
OLD_ROWS="\`remote-hand-${VERSION}-macos-{aarch64,x86_64,universal}.tar.gz\`"
PLATFORMS="macOS (Apple silicon and Intel)"
if [ "$LINUX" = 1 ]; then
  ROWS="$ROWS
| \`$NEW_NAME-${VERSION}-linux-x86_64.tar.gz\` | Linux x86_64, glibc ${GLIBC_MAX}+ |
| \`$NEW_NAME-${VERSION}-linux-aarch64.tar.gz\` | Linux aarch64, glibc ${GLIBC_MAX}+ |"
  OLD_ROWS="$OLD_ROWS and \`remote-hand-${VERSION}-linux-{x86_64,aarch64}.tar.gz\`"
  PLATFORMS="$PLATFORMS and Linux"
fi
ROWS="$ROWS
| $OLD_ROWS | the same server under its old name only, for older copies of the installer (on every release until at least $OLD_ASSETS_UNTIL) |"
# Built by .github/workflows/release.yml, every file gets an attestation.
ATTESTED=""
if [ "${GITHUB_ACTIONS:-}" = true ]; then
  ATTESTED="| \`droppen-${VERSION}.sigstore.json\`, \`web-manifest.sigstore.json\` | the build provenance attestations: every file here was built by this repository's release workflow from the commit below. Check one with \`gh attestation verify <file> --repo getdroppen/droppen --signer-workflow getdroppen/droppen/.github/workflows/release.yml --source-ref refs/tags/${VERSION}\` |
"
fi
cat <<EOF
Droppen ${VERSION}: the server for ${PLATFORMS}.

On a Mac, the Droppen desktop App (\`Droppen-<version>.dmg\`, when it is attached
to a release) is the recommended way for everyday use. The files below are the
server, for running Droppen in the background or on a device with no screen.
Install or upgrade it, and add the device to your account with a one-time code
(on droppen.app: my devices → add a device that runs AI → show a one-time code):

    curl -fsSL https://droppen.app/install.sh | DROPPEN_CODE=<code> sh

Or download the tarball for your device, check it against \`SHA256SUMS\`
(\`shasum -a 256 -c --ignore-missing SHA256SUMS\`, or \`sha256sum -c --ignore-missing SHA256SUMS\`),
and follow \`INSTALL.txt\`. The server is \`droppen\`; \`remote-hand-server\` is the
same program under its old name, which the installed service keeps.

| file | for |
|---|---|
${ROWS}
| \`install.sh\` | the installer above (https://droppen.app/install.sh serves this file once Droppen Hub is built from this release) |
| \`INSTALL-macos.txt\`, \`INSTALL-linux.txt\` | installing by hand (each tarball has its own \`INSTALL.txt\`) |
| \`LICENSE\` | Droppen's licence: AGPL-3.0-only |
| \`THIRD_PARTY_NOTICES\` | the third-party software in the server, with its licences |
| \`web-manifest.json\` | the embedded web app's files and digest |
${ATTESTED}
Droppen is licensed under AGPL-3.0-only. The source of this release is
https://github.com/getdroppen/droppen/tree/${SHA}.

Copyright (C) 2026 Kilerd

Built from commit \`${SHA}\` (server \`${CRATE_VERSION}+g${REV}\`).
EOF
if [ -n "$NOTES_FILE" ]; then
  printf '\n'
  cat "$NOTES_FILE"
fi
