#!/usr/bin/env bash
# The bridge for apps installed before releases moved to getdroppen/droppen.
# Droppen 0.2.125 and 0.2.126 read latest.json from the latest release of
# getdroppen/releases and download an update only from that repository's
# releases. So the first release published on getdroppen/droppen (v0.2.127)
# is also published there once, by hand, with the same signed files and a
# latest.json whose addresses point there; the apps update to it, and from
# then on read getdroppen/droppen. Then getdroppen/releases is archived.
#
#   scripts/bridge-release.sh <tag> [--dry-run]
#
# What it does, from an empty scratch directory:
#   1. downloads every file of getdroppen/droppen's release <tag> (published,
#      not a draft) and its notes;
#   2. checks them: every file against the release's SHA256SUMS, and every
#      file's build provenance attestation (`gh attestation verify`: built by
#      getdroppen/droppen's .github/workflows/release.yml for refs/tags/<tag>);
#   3. rewrites latest.json's `url`s and `notes`, and nothing else, to the
#      same files on getdroppen/releases (checked: the version and the
#      signatures are the attested latest.json's);
#   4. publishes them there with scripts/publish-release.sh
#      (DROPPEN_RELEASE_REPO=getdroppen/releases): a draft, latest.json last,
#      then the latest release. The archive, its signature, the DMG and the
#      tarballs are the attested bytes; that release's SHA256SUMS lists its
#      own latest.json. latest.json is not checked by droppen-update-manifest
#      there (it now accepts getdroppen/droppen's addresses only): its
#      signature is the attested one, over the same archive.
#
# Needs a Mac (the DMG's notarisation ticket), gh logged in with write access
# to getdroppen/releases (the operator's own login: CI has no token for it),
# node, shasum, curl. Nothing in CI runs this.
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }
die() { printf 'bridge-release: %s\n' "$*" >&2; exit 1; }
say() { printf '\n== %s\n' "$*"; }

TAG=""
DRY_RUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage ;;
    -*) echo "unknown option: $1" >&2; usage ;;
    *) [ -z "$TAG" ] || usage; TAG=$1; shift ;;
  esac
done
[ -n "$TAG" ] || usage
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "a release tag is vX.Y.Z, not $TAG"
PLAIN=${TAG#v}
SOURCE=getdroppen/droppen
BRIDGE=getdroppen/releases
ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
for tool in gh node shasum; do command -v "$tool" >/dev/null || die "missing $tool"; done

WORK=$(mktemp -d "${TMPDIR:-/tmp}/droppen-bridge.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
DL=$WORK/download

say "$TAG from $SOURCE"
[ "$(gh release view "$TAG" --repo "$SOURCE" --json isDraft --jq .isDraft)" = false ] \
  || die "$SOURCE has no published release $TAG"
gh release download "$TAG" --repo "$SOURCE" --dir "$DL"
gh release view "$TAG" --repo "$SOURCE" --json body --jq .body >"$WORK/NOTES.md"
ls -l "$DL"
for file in SHA256SUMS latest.json Droppen.app.tar.gz Droppen.app.tar.gz.sig "Droppen-$PLAIN.dmg" LICENSE THIRD_PARTY_NOTICES web-manifest.json; do
  [ -s "$DL/$file" ] || die "$SOURCE's release $TAG has no $file: a bridge carries the desktop app's update"
done

say "checks"
(cd "$DL" && shasum -a 256 -c SHA256SUMS) || die "a file does not match $TAG's SHA256SUMS"
for file in "$DL"/*; do
  case "$file" in *.sigstore.json) continue ;; esac
  gh attestation verify "$file" --repo "$SOURCE" \
    --signer-workflow "$SOURCE/.github/workflows/release.yml" --source-ref "refs/tags/$TAG" >/dev/null \
    || die "$(basename "$file") has no attestation from $SOURCE's release workflow for $TAG"
done
echo "ok: every file matches SHA256SUMS and is attested by $SOURCE/.github/workflows/release.yml at refs/tags/$TAG"

say "the files for $BRIDGE"
SERVER=$WORK/server
DESKTOP=$WORK/desktop
mkdir -p "$SERVER" "$DESKTOP/updater"
cp "$DL"/droppen-"$TAG"-*.tar.gz "$DL"/remote-hand-"$TAG"-*.tar.gz "$DL/LICENSE" "$DL/THIRD_PARTY_NOTICES" "$DL/web-manifest.json" "$SERVER/"
cp "$WORK/NOTES.md" "$SERVER/NOTES.md"
# The server's SHA256SUMS: its tarballs' lines (publish-release.sh adds the
# desktop app's files and latest.json).
grep -E "  \./(droppen|remote-hand)-$TAG-[a-z]+-[a-z0-9_]+\.tar\.gz$" "$DL/SHA256SUMS" >"$SERVER/SHA256SUMS" \
  || die "$TAG's SHA256SUMS lists no tarballs"
cp "$DL/Droppen-$PLAIN.dmg" "$DESKTOP/"
cp "$DL/Droppen.app.tar.gz" "$DL/Droppen.app.tar.gz.sig" "$DESKTOP/updater/"
node -e '
  const fs = require("fs");
  const [from, to, source, bridge, tag] = process.argv.slice(1);
  const m = JSON.parse(fs.readFileSync(from, "utf8"));
  const was = (repo) => `https://github.com/${repo}/releases/download/${tag}/Droppen.app.tar.gz`;
  const out = JSON.parse(JSON.stringify(m));
  for (const [name, p] of Object.entries(out.platforms || {})) {
    if (p.url !== was(source)) throw new Error(`${name}: url ${p.url} is not ${was(source)}`);
    p.url = was(bridge);
  }
  out.notes = `https://github.com/${bridge}/releases/tag/${tag}`;
  // Only the addresses differ.
  const strip = (x) => JSON.stringify({ ...x, notes: null, platforms: Object.fromEntries(Object.entries(x.platforms).map(([k, p]) => [k, { ...p, url: null }])) });
  if (strip(out) !== strip(m) || out.version !== tag.slice(1)) throw new Error("the rewrite changed more than the addresses");
  fs.writeFileSync(to, JSON.stringify(out, null, 2) + "\n");
' "$DL/latest.json" "$DESKTOP/updater/latest.json" "$SOURCE" "$BRIDGE" "$TAG" || die "cannot rewrite latest.json"
cat "$DESKTOP/updater/latest.json"

say "publish on $BRIDGE"
args=("$TAG" --server "$SERVER" --desktop "$DESKTOP" --skip-manifest-check)
[ "$DRY_RUN" = 0 ] || args+=(--dry-run)
DROPPEN_RELEASE_REPO=$BRIDGE "$ROOT/scripts/publish-release.sh" "${args[@]}"
if [ "$DRY_RUN" = 0 ]; then
  cat <<EOF

Bridged: https://github.com/$BRIDGE/releases/tag/$TAG is the latest release
there, and Droppen 0.2.125 and 0.2.126 update to $TAG from it. Leave it the
latest release of $BRIDGE, then archive $BRIDGE (Settings → Archive this
repository); do not delete it.
EOF
fi
