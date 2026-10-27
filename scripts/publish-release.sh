#!/usr/bin/env bash
# Publish a release on getdroppen/droppen (the source repository's own
# releases; DROPPEN_RELEASE_REPO names another, as scripts/bridge-release.sh
# does): the server scripts/release-public.sh staged and, with --desktop, the desktop
# app desktop/SIGNING.md made (notarised DMG, updater archive, signature,
# latest.json). Installed desktop apps read `latest.json` from the repository's
# *latest* release, so every release carries one, uploaded last, and a release
# becomes the latest only once it has it:
#
#   1. checks everything first: the staged files against their SHA256SUMS; the
#      desktop files, and their latest.json against the archive and
#      tauri.conf.json's updater key; that no release (draft or not) is
#      called <version> yet, and that <version> is newer than the latest
#      release;
#   2. picks the release's latest.json:
#        --desktop: the one desktop/updater-artifacts.sh wrote for <version>;
#        otherwise the one installed apps read today, carried forward
#        unchanged: the latest release's (or --manifest-from's, only when the
#        latest release has none). It must come from a published release that
#        is not a prerelease, announce a version older than <version>, and
#        verify against the archive it names, downloaded from that version's
#        release (also published, not a prerelease);
#      and without --desktop, the app's DMG for the download page: the one on
#      the release the manifest comes from (or the latest release), carried
#      forward too;
#      and, when the release's install.sh pins a release key (SUMS_KEY,
#      scripts/install-key.sh), that SHA256SUMS-server.sig in the server's
#      directory is a good signature over its SHA256SUMS with that key;
#   3. creates <version> as a DRAFT with the server's files (a draft is
#      invisible, and GitHub's "latest" never points at one), uploads the
#      desktop files (or the carried DMG), then latest.json LAST;
#   4. checks that the draft holds exactly those files, sizes and all;
#   5. publishes it and marks it latest, in one step;
#   6. checks that .../releases/latest/download/latest.json is now this file.
#
#   scripts/publish-release.sh <version> --server <dir> [--desktop <dir>]
#                              [--manifest-from <tag> | --no-manifest]
#                              [--verify-pubkey <file>] [--dry-run [--local-only]]
#                              [--sums-out <file>] [--sums-expected <file>]
#                              [--skip-manifest-check] [--sums-only <file>]
#                              [--verify-tag]
#
#   <version>        the tag, e.g. v0.2.125: the one release-public.sh was given
#   --server         release-public.sh's --out directory
#   --desktop        desktop/build.sh's --out directory once desktop/SIGNING.md
#                    is done with it: the notarised Droppen-<x.y.z>.dmg, and
#                    updater/ (desktop/updater-artifacts.sh --out <dir>/updater)
#   --manifest-from  without --desktop, when the latest release has no
#                    latest.json: carry this release's
#   --no-manifest    without --desktop: publish no latest.json; refused once
#                    any published release has one (before the first app with
#                    the updater is released, desktop/SIGNING.md, "Rules for a
#                    release")
#   --verify-pubkey  during a key rotation only (desktop/SIGNING.md, "Rotating
#                    the key"): the .pub file of the key the latest.json being
#                    published is signed with — the old key, which installed
#                    apps trust — instead of tauri.conf.json's new one
#   --dry-run        read and check everything (the carried manifest and its
#                    archive are downloaded and verified) and print the writes
#                    instead of making them
#   --local-only     with --dry-run only: check the files of step 1 and stop,
#                    asking the repository nothing (a dry run of a version
#                    that is published already: release.yml's dry runs of
#                    itself). --sums-out then lists no carried files.
#   --sums-out       also write the release's SHA256SUMS (the server's
#                    tarballs, then the desktop app's or the carried files,
#                    and latest.json) to <file>, with --dry-run too
#   --sums-expected  stop before the first write unless the release's
#                    SHA256SUMS is <file>, byte for byte: release.yml attests
#                    every file it publishes before publishing, SHA256SUMS
#                    made by a --dry-run --sums-out among them
#   --skip-manifest-check  do not run droppen-update-manifest (latest.json
#                    against its archive and the key): for a caller that ran
#                    these checks in a job of its own and holds this run to
#                    their result with --sums-expected (release.yml: the job
#                    that publishes runs nothing built from dependencies)
#   --verify-tag     the tag must exist in the repository already, and the
#                    release is made for it (in the source repository, whose
#                    tag release.yml runs on; without it, publishing a release
#                    creates its tag at the default branch's head)
#   --sums-only      write the release's SHA256SUMS to <file> and stop: the
#                    checks of a --dry-run but those of --skip-manifest-check,
#                    and nothing written (release.yml's attest job)
#
# A prerelease version (v1.2.3-rc.1) is published as a GitHub prerelease,
# never marked latest, with no latest.json and no desktop app: installed apps
# never read it.
#
# If a step after the draft was created fails, the draft stays (invisible):
# look at it, delete it (gh release delete <version> --repo <repo>) and run
# this again.
#
# Needs: gh (logged in, with write access to the repository), node, shasum,
# curl, and cargo for the desktop workspace (droppen-update-manifest).
#
# Environment:
#   DROPPEN_RELEASE_REPO     the repository to publish in (getdroppen/droppen)
#   DROPPEN_UPDATE_MANIFEST  a built droppen-update-manifest to use instead of
#                            `cargo run` in desktop/
#   DROPPEN_PUBLISH_RETRY_SECONDS  the pause between the last step's six tries (5)
#
# Tested by scripts/publish-release.test.mjs, against a stand-in gh.
set -euo pipefail

RELEASE_DIE_NAME='publish-release'
# shellcheck source=release-lib.sh
. "$(cd "$(dirname "$0")" && pwd -P)/release-lib.sh"

usage() { release_usage '/^# Needs:/'; }

VERSION=""
SERVER=""
DESKTOP=""
MANIFEST_FROM=""
NO_MANIFEST=0
DRY_RUN=0
LOCAL_ONLY=0
SKIP_MANIFEST_CHECK=0
SUMS_ONLY=0
VERIFY_TAG=0
SUMS_OUT=""
SUMS_EXPECTED=""
VERIFY_PUBKEY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --server) SERVER=${2:?--server needs a directory}; shift 2 ;;
    --desktop) DESKTOP=${2:?--desktop needs a directory}; shift 2 ;;
    --manifest-from) MANIFEST_FROM=${2:?--manifest-from needs a tag}; shift 2 ;;
    --no-manifest) NO_MANIFEST=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --local-only) LOCAL_ONLY=1; shift ;;
    --sums-out) SUMS_OUT=${2:?--sums-out needs a file}; shift 2 ;;
    --sums-expected) SUMS_EXPECTED=${2:?--sums-expected needs a file}; shift 2 ;;
    --skip-manifest-check) SKIP_MANIFEST_CHECK=1; shift ;;
    --verify-tag) VERIFY_TAG=1; shift ;;
    --sums-only) SUMS_ONLY=1; DRY_RUN=1; SKIP_MANIFEST_CHECK=1; SUMS_OUT=${2:?--sums-only needs a file}; shift 2 ;;
    --verify-pubkey) VERIFY_PUBKEY=${2:?--verify-pubkey needs a path}; shift 2 ;;
    -h|--help) usage ;;
    -*) echo "unknown option: $1" >&2; usage ;;
    *) [ -z "$VERSION" ] || usage; VERSION=$1; shift ;;
  esac
done
[ -n "$VERSION" ] && [ -n "$SERVER" ] || usage
[[ "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "version must look like v1.2.3, not $VERSION"
PLAIN=${VERSION#v}
PRERELEASE=0
case "$PLAIN" in *-*) PRERELEASE=1 ;; esac
if [ -n "$DESKTOP" ] && { [ -n "$MANIFEST_FROM" ] || [ "$NO_MANIFEST" = 1 ]; }; then
  die "--desktop brings its own latest.json; --manifest-from and --no-manifest are for a release without the desktop app"
fi
[ -z "$MANIFEST_FROM" ] || [ "$NO_MANIFEST" = 0 ] || die "--manifest-from and --no-manifest are exclusive"
[ "$LOCAL_ONLY" = 0 ] || [ "$DRY_RUN" = 1 ] || die "--local-only is for a --dry-run: a release is published only after every check"
[ -z "$SUMS_EXPECTED" ] || [ -f "$SUMS_EXPECTED" ] || die "--sums-expected $SUMS_EXPECTED: no such file"
[ "$SKIP_MANIFEST_CHECK" = 0 ] || [ -z "$VERIFY_PUBKEY" ] || die "--verify-pubkey is a manifest check; it does not go with --skip-manifest-check or --sums-only"
if [ "$PRERELEASE" = 1 ] && { [ -n "$DESKTOP" ] || [ -n "$MANIFEST_FROM" ]; }; then
  die "$VERSION is a prerelease: it never becomes the latest release, so it carries neither the desktop app's update nor latest.json"
fi
# The desktop files' checks include the DMG's notarisation ticket (stapler),
# which only a Mac can read.
if [ -n "$DESKTOP" ] && [ "$(uname -s)" != Darwin ]; then
  die "--desktop needs a Mac: the DMG's notarisation ticket is checked with xcrun stapler"
fi

REPO_PUBLIC=${DROPPEN_RELEASE_REPO:-getdroppen/droppen}
ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
PUBKEY_CONF=$ROOT/desktop/src-tauri/tauri.conf.json
[ -z "$VERIFY_PUBKEY" ] || [ -f "$VERIFY_PUBKEY" ] || die "--verify-pubkey $VERIFY_PUBKEY: no such file"
for tool in gh node shasum curl; do
  command -v "$tool" >/dev/null || die "missing $tool"
done

TMP_BASE=${TMPDIR:-/tmp}
WORK=$(mktemp -d "${TMP_BASE%/}/droppen-publish.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# The manifest tool of this checkout's desktop workspace, built once first so
# that a build failure is not taken for a refused manifest.
MANIFEST_TOOL_BUILT=0
update_manifest() {
  if [ -n "${DROPPEN_UPDATE_MANIFEST:-}" ]; then
    "$DROPPEN_UPDATE_MANIFEST" "$@"
    return
  fi
  local cargo_args=(--quiet --release --locked --manifest-path "$ROOT/desktop/Cargo.toml" -p droppen-desktop-core --bin droppen-update-manifest)
  if [ "$MANIFEST_TOOL_BUILT" = 0 ]; then
    command -v cargo >/dev/null || die "missing cargo, to build droppen-update-manifest (or set DROPPEN_UPDATE_MANIFEST)"
    cargo build "${cargo_args[@]}" || die "cannot build droppen-update-manifest (desktop/core)"
    MANIFEST_TOOL_BUILT=1
  fi
  cargo run "${cargo_args[@]}" -- "$@"
}

# A check by the manifest tool, unless --skip-manifest-check.
manifest_check() {
  if [ "$SKIP_MANIFEST_CHECK" = 1 ]; then
    echo "skip: droppen-update-manifest $1 ${2:-} (--skip-manifest-check: checked by the caller)"
    return 0
  fi
  update_manifest "$@"
}

# One field of a JSON file (a dotted path), or nothing.
json_field() { # file path
  node -e '
    let v = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    for (const k of process.argv[2].split(".")) v = v == null ? undefined : v[k];
    if (v !== undefined && v !== null) process.stdout.write(typeof v === "string" ? v : JSON.stringify(v));
  ' "$1" "$2"
}

# Whether x.y.z $1 is older than x.y.z $2.
older() {
  local IFS=. i
  local -a a b
  read -r -a a <<<"$1"
  read -r -a b <<<"$2"
  for i in 0 1 2; do
    if ((10#${a[i]} < 10#${b[i]})); then return 0; fi
    if ((10#${a[i]} > 10#${b[i]})); then return 1; fi
  done
  return 1
}

size_of() { wc -c <"$1" | tr -d ' '; }

# Every release, drafts included (gh is logged in with write access), as
# "tag<TAB>draft<TAB>prerelease<TAB>asset,asset,..." lines, newest first.
releases() {
  gh api --paginate --slurp "repos/$REPO_PUBLIC/releases" >"$WORK/releases.json" \
    || die "cannot list the releases of $REPO_PUBLIC (gh auth status?)"
  node -e '
    const pages = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    for (const r of pages.flat()) {
      console.log([r.tag_name, r.draft, r.prerelease, r.assets.map((a) => a.name).join(",")].join("\t"));
    }
  ' "$WORK/releases.json"
}

# The release called $1 as a "draft<TAB>prerelease<TAB>assets" line, or nothing.
release_of() { awk -F'\t' -v tag="$1" '$1 == tag { print $2 "\t" $3 "\t" $4; exit }' "$WORK/releases.tsv"; }

# The release called $1's asset names, comma-separated, or nothing.
assets_of() { awk -F'\t' -v tag="$1" '$1 == tag { print $4; exit }' "$WORK/releases.tsv"; }

# Run a write, or with --dry-run say what it would be.
write() {
  if [ "$DRY_RUN" = 1 ]; then
    printf 'would run:'
    printf ' %q' "$@"
    printf '\n'
  else
    "$@"
  fi
}

say "$VERSION on $REPO_PUBLIC$([ "$DRY_RUN" = 1 ] && echo ' (dry run: nothing is written)')"

if [ -n "$VERIFY_PUBKEY" ]; then
  # A rotation's old key, which installed apps trust: never the app's own.
  RELEASE_KEY_ID=$(update_manifest key-id --pubkey "$PUBKEY_CONF") || die "cannot read the updater key in $PUBKEY_CONF"
  ROTATION_KEY_ID=$(update_manifest key-id --pubkey "$VERIFY_PUBKEY") || die "$VERIFY_PUBKEY holds no minisign public key"
  [ "$ROTATION_KEY_ID" != "$RELEASE_KEY_ID" ] \
    || die "--verify-pubkey is tauri.conf.json's own key ($RELEASE_KEY_ID): without a rotation, leave it out"
  echo "KEY ROTATION: latest.json is checked with $ROTATION_KEY_ID ($VERIFY_PUBKEY), the key installed apps trust, not tauri.conf.json's $RELEASE_KEY_ID (desktop/SIGNING.md, \"Rotating the key\")"
  PUBKEY_CONF=$VERIFY_PUBKEY
fi

say "the server's files: $SERVER"
[ -d "$SERVER" ] || die "$SERVER is not a directory"
for file in NOTES.md SHA256SUMS LICENSE THIRD_PARTY_NOTICES web-manifest.json; do
  [ -s "$SERVER/$file" ] || die "$SERVER has no $file: is it release-public.sh's --out for $VERSION?"
done
# Notes that announce this version's app (docs/releases/<version>.md's
# paragraph on Droppen-<x.y.z>.dmg, that it updates itself) need the app on
# the release: without --desktop the latest release's DMG is carried forward
# instead, and the notes would promise a file and an update nobody gets.
if [ -z "$DESKTOP" ] && grep -qF "Droppen-$PLAIN.dmg" "$SERVER/NOTES.md"; then
  die "NOTES.md announces Droppen-$PLAIN.dmg, but there is no --desktop: publish with the desktop app, or take the app's paragraphs out of docs/releases/$VERSION.md and stage again"
fi
SERVER_ASSETS=()
for tarball in "$SERVER"/droppen-"$VERSION"-*.tar.gz "$SERVER"/remote-hand-"$VERSION"-*.tar.gz; do
  [ -f "$tarball" ] && SERVER_ASSETS+=("$tarball")
done
[ "${#SERVER_ASSETS[@]}" -gt 0 ] || die "$SERVER has no droppen-$VERSION-*.tar.gz: it was staged for another version"
# Published as they are when there: the installer and the INSTALL texts
# (release-public.sh stages them), and release.yml's attestation bundles.
EXTRAS=()
for name in install.sh INSTALL-macos.txt INSTALL-linux.txt web-manifest.sigstore.json "droppen-$VERSION.sigstore.json"; do
  [ ! -e "$SERVER/$name" ] || EXTRAS+=("$SERVER/$name")
done
STRAY=$(cd "$SERVER" && ls | grep -v -x -e NOTES.md -e SHA256SUMS -e LICENSE -e THIRD_PARTY_NOTICES -e web-manifest.json \
  -e install.sh -e INSTALL-macos.txt -e INSTALL-linux.txt -e web-manifest.sigstore.json -e "droppen-$VERSION\.sigstore\.json" \
  -e 'SHA256SUMS-server\.sig' -e "droppen-$VERSION-.*\.tar\.gz" -e "remote-hand-$VERSION-.*\.tar\.gz" || true)
[ -z "$STRAY" ] || die "$SERVER holds files release-public.sh does not make: $(echo "$STRAY" | tr '\n' ' ')"
# SHA256SUMS lists exactly the tarballs, and they match.
LISTED=$(awk '{ n = $2; sub(/^\*/, "", n); sub(/^\.\//, "", n); print n }' "$SERVER/SHA256SUMS" | LC_ALL=C sort)
[ "$LISTED" = "$(for f in "${SERVER_ASSETS[@]}"; do basename "$f"; done | LC_ALL=C sort)" ] \
  || die "$SERVER/SHA256SUMS does not list exactly the tarballs beside it"
(cd "$SERVER" && shasum -a 256 -c SHA256SUMS >/dev/null) || die "$SERVER: a tarball does not match SHA256SUMS"
echo "ok: ${#SERVER_ASSETS[@]} tarballs match SHA256SUMS; NOTES.md, LICENSE, THIRD_PARTY_NOTICES, web-manifest.json"

# The installer's checksums: when the release's install.sh pins a release key
# (SUMS_KEY), it downloads SHA256SUMS-server — the server's SHA256SUMS above —
# and SHA256SUMS-server.sig, the signature release.yml's `release` job made
# over it, and installs nothing unless that checks out with the key. Both go
# up with the release, and the signature must check out here first.
SUMS_SIG=$SERVER/SHA256SUMS-server.sig
PINNED=""
if [ -f "$SERVER/install.sh" ]; then
  PINNED=$("$ROOT/scripts/install-key.sh" "$SERVER/install.sh") || die "cannot read the release key $SERVER/install.sh pins"
fi
if [ -n "$PINNED" ]; then
  [ -s "$SUMS_SIG" ] \
    || die "$SERVER/install.sh checks a signature over SHA256SUMS-server, and $SERVER has no SHA256SUMS-server.sig (release.yml's release job makes it; docs/releasing.md, \"The release key\")"
  command -v openssl >/dev/null || die "missing openssl, to check SHA256SUMS-server.sig"
  printf '%s\n' "$PINNED" >"$WORK/install-key.pem"
  cp "$SERVER/SHA256SUMS" "$WORK/SHA256SUMS-server"
  openssl dgst -sha256 -verify "$WORK/install-key.pem" -signature "$SUMS_SIG" "$WORK/SHA256SUMS-server" >/dev/null 2>&1 \
    || die "$SUMS_SIG is not a signature over $SERVER/SHA256SUMS by the release key install.sh pins"
  EXTRAS+=("$WORK/SHA256SUMS-server" "$SUMS_SIG")
  echo "ok: SHA256SUMS-server.sig checks out with the release key install.sh pins"
elif [ -e "$SUMS_SIG" ]; then
  die "$SERVER has SHA256SUMS-server.sig, but no install.sh beside it that pins a release key to check it with"
fi

DESKTOP_ASSETS=()
if [ -n "$DESKTOP" ]; then
  say "the desktop app: $DESKTOP"
  DMG=$DESKTOP/Droppen-$PLAIN.dmg
  UPDATER=$DESKTOP/updater
  for file in "$DMG" "$UPDATER/Droppen.app.tar.gz" "$UPDATER/Droppen.app.tar.gz.sig" "$UPDATER/latest.json"; do
    [ -s "$file" ] || die "no $file (desktop/SIGNING.md: sign, notarise, then desktop/updater-artifacts.sh --out $UPDATER)"
  done
  [ -z "$(ls "$DESKTOP"/E2E-TEST-BUILD.txt 2>/dev/null)" ] || die "$DESKTOP is an end-to-end test build: never published"
  # What desktop/notarise.sh left: a signed, notarised DMG with its ticket.
  xcrun stapler validate "$DMG" >/dev/null 2>&1 \
    || die "$DMG has no stapled notarisation ticket: run desktop/notarise.sh first (desktop/SIGNING.md)"
  [ "$(json_field "$UPDATER/latest.json" version)" = "$PLAIN" ] \
    || die "$UPDATER/latest.json announces $(json_field "$UPDATER/latest.json" version), not $PLAIN"
  SIGNED=$(json_field "$UPDATER/latest.json" platforms.darwin-aarch64.signature)
  [ "$SIGNED" = "$(tr -d '\r\n' <"$UPDATER/Droppen.app.tar.gz.sig")" ] \
    || die "$UPDATER/latest.json names another signature than Droppen.app.tar.gz.sig"
  manifest_check check "$UPDATER/latest.json" --pubkey "$PUBKEY_CONF" --archive "$UPDATER/Droppen.app.tar.gz" \
    || die "$UPDATER/latest.json was refused"
  DESKTOP_ASSETS=("$DMG" "$UPDATER/Droppen.app.tar.gz" "$UPDATER/Droppen.app.tar.gz.sig")
  MANIFEST=$UPDATER/latest.json
fi

# SHA256SUMS of the release: the server's tarballs, then the desktop app's
# files (or the carried DMG) and latest.json, as $WORK/SHA256SUMS; written to
# --sums-out, and held to --sums-expected.
make_sums() { # the files after the server's tarballs
  local file
  cp "$SERVER/SHA256SUMS" "$WORK/SHA256SUMS"
  for file in "$@"; do
    printf '%s  ./%s\n' "$(shasum -a 256 "$file" | awk '{ print $1 }')" "$(basename "$file")" >>"$WORK/SHA256SUMS"
  done
  cat "$WORK/SHA256SUMS"
  if [ -n "$SUMS_OUT" ]; then
    cp "$WORK/SHA256SUMS" "$SUMS_OUT"
    echo "(written to $SUMS_OUT)"
  fi
  if [ -n "$SUMS_EXPECTED" ]; then
    cmp -s "$WORK/SHA256SUMS" "$SUMS_EXPECTED" \
      || die "the release's SHA256SUMS is not $SUMS_EXPECTED byte for byte (the files changed since it was made): nothing was written"
    echo "ok: the same as $SUMS_EXPECTED"
  fi
}

if [ "$LOCAL_ONLY" = 1 ]; then
  say "SHA256SUMS (--local-only: without carried files)"
  make_sums ${DESKTOP_ASSETS[@]+"${DESKTOP_ASSETS[@]}"} ${MANIFEST:+"$MANIFEST"}
  printf '\nA dry run of the files only (--local-only): %s was not asked anything, and nothing was written.\n' "$REPO_PUBLIC"
  exit 0
fi

say "the releases of $REPO_PUBLIC"
releases >"$WORK/releases.tsv"
echo "ok: $(wc -l <"$WORK/releases.tsv" | tr -d ' ') releases"
[ -z "$(release_of "$VERSION")" ] \
  || die "$REPO_PUBLIC already has a release $VERSION ($(release_of "$VERSION" | awk -F'\t' '{ print ($1 == "true" ? "a draft" : "published") }')): look at it, and delete it if it is a failed attempt"
# What installed apps read today: GitHub's latest release (none before the first).
# Only GitHub's "not found" means there is none yet; any other failure stops
# here, or the checks against the latest release would pass for want of one.
if ! LATEST=$(gh api "repos/$REPO_PUBLIC/releases/latest" --jq .tag_name 2>"$WORK/latest.err"); then
  grep -q 'HTTP 404' "$WORK/latest.err" \
    || die "cannot ask $REPO_PUBLIC for its latest release: $(grep -m 1 . "$WORK/latest.err" || echo "gh failed")"
  LATEST=""
fi
# The newest published, non-prerelease release that has a latest.json.
CARRYING=$(awk -F'\t' '$2 == "false" && $3 == "false" && ("," $4 ",") ~ /,latest\.json,/ { print $1; exit }' "$WORK/releases.tsv")
if [ "$PRERELEASE" = 0 ] && [ -n "$LATEST" ]; then
  [[ "$LATEST" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "the latest release is $LATEST, not vX.Y.Z: look at it first"
  older "${LATEST#v}" "$PLAIN" || die "$VERSION is not newer than the latest release, $LATEST: publishing it would move \"latest\" backwards"
fi
echo "the latest release: ${LATEST:-none}"

CARRY_DMG_FROM=""
if [ "$PRERELEASE" = 1 ]; then
  MANIFEST=""
  echo "a prerelease: no latest.json (installed apps never read one)"
elif [ -n "$DESKTOP" ]; then
  echo "latest.json: the desktop app's, checked above"
elif [ "$NO_MANIFEST" = 1 ]; then
  [ -z "$CARRYING" ] \
    || die "--no-manifest, but $CARRYING has a latest.json: a release without one leaves every installed app's checks failing (and a failed check still installs a download). Carry it forward instead (a bad update is fixed forward: desktop/SIGNING.md, \"Rollback\")"
  MANIFEST=""
  CARRY_DMG_FROM=$LATEST
  echo "no latest.json: no published release has had one yet"
else
  say "latest.json, carried forward"
  if [ -n "$MANIFEST_FROM" ]; then
    case ",$(assets_of "$LATEST")," in
      *,latest.json,*) die "the latest release, $LATEST, has a latest.json: that is the one carried forward (another release's could withdraw the update it offers)" ;;
    esac
    FROM=$MANIFEST_FROM
  else
    FROM=$LATEST
    [ -n "$FROM" ] || die "$REPO_PUBLIC has no latest release to carry latest.json from (pass --no-manifest if no release has had one yet)"
  fi
  FROM_LINE=$(release_of "$FROM")
  [ -n "$FROM_LINE" ] || die "$REPO_PUBLIC has no release $FROM"
  IFS=$'\t' read -r draft prerelease assets <<<"$FROM_LINE"
  [ "$draft" = false ] || die "$FROM is a draft: installed apps cannot see it, so its latest.json is no source"
  [ "$prerelease" = false ] || die "$FROM is a prerelease: installed apps never read it, so its latest.json is no source"
  case ",$assets," in
    *,latest.json,*) ;;
    *)
      die "$FROM carries no latest.json, so installed apps find no update today. $(if [ -n "$CARRYING" ]; then
        echo "The newest release that has one is $CARRYING: carry it with --manifest-from $CARRYING."
      else
        echo "No published release has had one yet: pass --no-manifest."
      fi)"
      ;;
  esac
  mkdir -p "$WORK/carried"
  gh release download "$FROM" --repo "$REPO_PUBLIC" --pattern latest.json --dir "$WORK/carried" \
    || die "cannot download latest.json from $FROM"
  MANIFEST=$WORK/carried/latest.json
  ANNOUNCED=$(json_field "$MANIFEST" version)
  [[ "$ANNOUNCED" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$FROM's latest.json announces '$ANNOUNCED', not a release version"
  older "$ANNOUNCED" "$PLAIN" || die "$FROM's latest.json announces $ANNOUNCED, not older than $PLAIN: this release would need the desktop app (--desktop)"
  # The archive it names stays on the release of the version it announces.
  ARCHIVE_LINE=$(release_of "v$ANNOUNCED")
  [ -n "$ARCHIVE_LINE" ] || die "latest.json names the archive of v$ANNOUNCED, which $REPO_PUBLIC does not have"
  IFS=$'\t' read -r draft prerelease assets <<<"$ARCHIVE_LINE"
  [ "$draft" = false ] && [ "$prerelease" = false ] || die "latest.json names the archive of v$ANNOUNCED, which is a draft or a prerelease"
  gh release download "v$ANNOUNCED" --repo "$REPO_PUBLIC" --pattern Droppen.app.tar.gz --dir "$WORK/carried" \
    || die "cannot download Droppen.app.tar.gz from v$ANNOUNCED"
  manifest_check check "$MANIFEST" --pubkey "$PUBKEY_CONF" --archive "$WORK/carried/Droppen.app.tar.gz" \
    || die "$FROM's latest.json was refused: it does not verify against v$ANNOUNCED's archive with $(basename "$PUBKEY_CONF")'s key (after a key rotation, see desktop/SIGNING.md, \"Rotating the key\")"
  echo "ok: $FROM's latest.json (Droppen $ANNOUNCED) goes on $VERSION unchanged"
  CARRY_DMG_FROM=$FROM
fi

if [ -n "$CARRY_DMG_FROM" ]; then
  # The download page's button goes to the latest release for its DMG: a
  # release without the desktop app carries the one installed apps update to.
  say "the app's DMG, carried forward"
  DMGS=$(assets_of "$CARRY_DMG_FROM" | tr ',' '\n' | grep -E '^Droppen-[0-9]+\.[0-9]+\.[0-9]+\.dmg$' || true)
  case "$(printf '%s' "$DMGS" | grep -c .)" in
    0) echo "WARNING: $CARRY_DMG_FROM has no Droppen-<version>.dmg, so $VERSION will have none for the download page" ;;
    1)
      if [ -n "$MANIFEST" ] && [ "$DMGS" != "Droppen-$ANNOUNCED.dmg" ]; then
        die "$CARRY_DMG_FROM has $DMGS, but its latest.json announces $ANNOUNCED: the DMG and the update would differ"
      fi
      mkdir -p "$WORK/carried"
      gh release download "$CARRY_DMG_FROM" --repo "$REPO_PUBLIC" --pattern "$DMGS" --dir "$WORK/carried" \
        || die "cannot download $DMGS from $CARRY_DMG_FROM"
      if [ -n "$MANIFEST" ] && [ "$(uname -s)" = Darwin ]; then
        xcrun stapler validate "$WORK/carried/$DMGS" >/dev/null 2>&1 \
          || die "$CARRY_DMG_FROM's $DMGS has no stapled notarisation ticket"
      elif [ -n "$MANIFEST" ]; then
        echo "WARNING: not on a Mac, so $DMGS's notarisation ticket is not checked (it was when $CARRY_DMG_FROM was published)"
      fi
      DESKTOP_ASSETS=("$WORK/carried/$DMGS")
      echo "ok: $CARRY_DMG_FROM's $DMGS goes on $VERSION unchanged"
      ;;
    *) die "$CARRY_DMG_FROM has several DMGs ($(echo $DMGS)): which one to carry is for a person to say" ;;
  esac
fi

say "SHA256SUMS"
# The server's tarballs, then the desktop app's files and latest.json.
make_sums ${DESKTOP_ASSETS[@]+"${DESKTOP_ASSETS[@]}"} ${MANIFEST:+"$MANIFEST"}

FIRST=("${SERVER_ASSETS[@]}" "$WORK/SHA256SUMS" "$SERVER/LICENSE" "$SERVER/THIRD_PARTY_NOTICES" "$SERVER/web-manifest.json" ${EXTRAS[@]+"${EXTRAS[@]}"})
EXPECTED=("${FIRST[@]}" ${DESKTOP_ASSETS[@]+"${DESKTOP_ASSETS[@]}"} ${MANIFEST:+"$MANIFEST"})
PRE_FLAG=()
[ "$PRERELEASE" = 0 ] || PRE_FLAG=(--prerelease)
[ "$VERIFY_TAG" = 0 ] || PRE_FLAG+=(--verify-tag)

if [ "$SUMS_ONLY" = 1 ]; then
  printf '\nSHA256SUMS only (--sums-only): written to %s; nothing was written to %s.\n' "$SUMS_OUT" "$REPO_PUBLIC"
  exit 0
fi

say "1/4 a draft with the server's files"
write gh release create "$VERSION" --repo "$REPO_PUBLIC" --draft ${PRE_FLAG[@]+"${PRE_FLAG[@]}"} \
  --title "Droppen $VERSION" --notes-file "$SERVER/NOTES.md" "${FIRST[@]}"

say "2/4 the desktop app's files, then latest.json"
if [ "${#DESKTOP_ASSETS[@]}" -gt 0 ]; then
  write gh release upload "$VERSION" --repo "$REPO_PUBLIC" "${DESKTOP_ASSETS[@]}"
else
  echo "(no desktop app)"
fi
if [ -n "$MANIFEST" ]; then
  # Last, alone.
  write gh release upload "$VERSION" --repo "$REPO_PUBLIC" "$MANIFEST"
else
  echo "(no latest.json)"
fi

say "3/4 the draft holds exactly these files"
if [ "$DRY_RUN" = 1 ]; then
  echo "(dry run: would check ${#EXPECTED[@]} files)"
else
  releases >"$WORK/releases.tsv"
  # Byte order on both sides (LC_ALL=C sort; node's sort of ASCII names):
  # a locale's collation (en_US on the macOS runners mixes the cases) would
  # order the same files differently and leave a right draft unpublished.
  for file in "${EXPECTED[@]}"; do printf '%s\t%s\n' "$(basename "$file")" "$(size_of "$file")"; done | LC_ALL=C sort >"$WORK/expected"
  node -e '
    const [file, tag] = process.argv.slice(1);
    const drafts = JSON.parse(require("fs").readFileSync(file, "utf8")).flat().filter((r) => r.tag_name === tag && r.draft);
    if (drafts.length !== 1) { console.error(`${drafts.length} drafts called ${tag}`); process.exit(1); }
    const lines = drafts[0].assets.map((a) => `${a.name}\t${a.state === "uploaded" ? a.size : a.state}`);
    console.log(lines.sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b))).join("\n"));
  ' "$WORK/releases.json" "$VERSION" >"$WORK/uploaded" || die "cannot find the draft $VERSION"
  if ! diff "$WORK/expected" "$WORK/uploaded" >&2; then
    die "the draft $VERSION does not hold exactly the files above (name, size): it stays a draft; fix it or delete it"
  fi
  echo "ok: $(wc -l <"$WORK/expected" | tr -d ' ') files, sizes match"
fi

say "4/4 publish"
if [ "$PRERELEASE" = 1 ]; then
  write gh release edit "$VERSION" --repo "$REPO_PUBLIC" --draft=false --latest=false
else
  write gh release edit "$VERSION" --repo "$REPO_PUBLIC" --draft=false --latest
fi

if [ "$DRY_RUN" = 1 ]; then
  printf '\nA dry run: nothing was written. Run it again without --dry-run to publish.\n'
  exit 0
fi

if [ "$PRERELEASE" = 0 ]; then
  say "what installed apps now read"
  # Retried a little: GitHub's answers may lag the edit by a moment.
  served() {
    [ -n "$MANIFEST" ] || return 0
    curl -fsSL -o "$WORK/served.json" "https://github.com/$REPO_PUBLIC/releases/latest/download/latest.json" \
      && cmp -s "$WORK/served.json" "$MANIFEST"
  }
  ok=0
  for _ in 1 2 3 4 5 6; do
    LATEST=$(gh api "repos/$REPO_PUBLIC/releases/latest" --jq .tag_name 2>/dev/null || true)
    if [ "$LATEST" = "$VERSION" ] && served; then
      ok=1
      break
    fi
    sleep "${DROPPEN_PUBLISH_RETRY_SECONDS:-5}"
  done
  [ "$ok" = 1 ] || die "$VERSION is published, but the latest release is ${LATEST:-unknown}, or it does not serve the latest.json uploaded: look now"
  if [ -n "$MANIFEST" ]; then
    echo "ok: $VERSION is the latest release and serves this latest.json (Droppen $(json_field "$MANIFEST" version))"
  else
    echo "ok: $VERSION is the latest release (no latest.json)"
  fi
fi
say "published $VERSION: https://github.com/$REPO_PUBLIC/releases/tag/$VERSION"
