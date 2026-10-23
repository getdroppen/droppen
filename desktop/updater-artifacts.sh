#!/usr/bin/env bash
# Make the updater's files for a release from the FINAL Droppen.app — signed
# with the Developer ID, notarised and stapled (desktop/SIGNING.md) — so that
# what installed apps download is exactly what a DMG download gets:
#
#   Droppen.app.tar.gz       the app, as the updater unpacks it
#   Droppen.app.tar.gz.sig   its minisign signature, by the updater key, bound
#                            to <version> (the app refuses any other pairing)
#   latest.json              the manifest the app fetches from the newest release
#
#   desktop/updater-artifacts.sh <Droppen.app> <version> --out <dir>
#                                [--notes-url <url>] [--key <file>] [--verify-pubkey <file>]
#                                [--no-manifest]
#                                [--test-build --key <file> --pubkey <file> --base-url <url>]
#
#   <version>     the release's version (0.2.125 or v0.2.125); the app's own
#                 version (Info.plist) must be the same.
#   --out         where the three files go; must be empty or absent.
#   --notes-url   the release notes the app links to (default: the release page
#                 https://github.com/getdroppen/droppen/releases/tag/v<version>).
#   --key         the updater's private key file (default: TAURI_SIGNING_PRIVATE_KEY
#                 when that is set, as in GitHub Actions; else
#                 ~/.config/droppen/tauri-updater.key). A file's password is read
#                 from <key>.password if that exists, else from
#                 TAURI_SIGNING_PRIVATE_KEY_PASSWORD (empty if unset). Neither the
#                 key nor its password is ever printed or put on a command line.
#   --verify-pubkey  a key rotation's transition release only (desktop/SIGNING.md,
#                 "Rotating the key"): the .pub file of the key installed apps
#                 trust, which signs this archive, while the app itself carries
#                 tauri.conf.json's new key. Refused when it is that same key.
#   --no-manifest the archive and its signature only: no latest.json, and none
#                 of droppen-update-manifest's checks (the key's id): the one
#                 program run where the key is, is the signer. release.yml
#                 writes and checks latest.json in a job without secrets
#                 (`droppen-update-manifest write`, which verifies the
#                 signature with tauri.conf.json's key). Not with
#                 --verify-pubkey or --test-build.
#   --test-build  for the updater's end-to-end test only (e2e-updater-macos.sh):
#                 accept an ad hoc signed, unstapled app, and an http://127.0.0.1
#                 --base-url. It needs a throwaway --key and its --pubkey: never
#                 a key of ~/.config/droppen (the current one, or an -old or
#                 -new one around a rotation) or CI's, by any name or copy, nor
#                 a .pub of the same key id. Never uploaded.
#   --base-url    with --test-build: where the archive is served instead of
#                 https://github.com/getdroppen/droppen/releases/download/v<version>.
#   --pubkey      with --test-build: the throwaway key's .pub file, verified with
#                 instead of the key in desktop/src-tauri/tauri.conf.json.
#
# Nothing is uploaded: the last thing this prints is the upload command.
#
# Environment:
#   TAURI_SIGNING_PRIVATE_KEY           the key itself (the contents of
#                                       tauri-updater.key; an Actions secret)
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD  its password (an Actions secret)
#   DROPPEN_UPDATE_SIGNER  a built droppen-update-signer (desktop/update-signer,
#                          which signs as `tauri signer sign --app-version` does)
#                          to use instead of `cargo run` there
#   DROPPEN_UPDATE_MANIFEST  a built droppen-update-manifest to use instead of
#                          `cargo run` in desktop/
#   DROPPEN_RELEASE_REPO   the repository releases are published in (getdroppen/droppen)
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }
die() { printf 'desktop/updater-artifacts.sh: %s\n' "$*" >&2; exit 1; }
say() { printf '\n== %s\n' "$*"; }

# The signing secrets, read once and taken out of the environment: nothing
# this runs inherits them (cargo, codesign, the manifest tool) but the signer,
# which gets them for its one command.
ENV_KEY=${TAURI_SIGNING_PRIVATE_KEY:-}
ENV_PASSWORD=${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD TAURI_SIGNING_PRIVATE_KEY_PATH

[ $# -ge 2 ] || usage
APP=$1
VERSION=${2#v}
shift 2
OUT=""
NOTES=""
KEY=""
TEST=0
NO_MANIFEST=0
BASE_URL=""
PUBKEY_FILE=""
VERIFY_PUBKEY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT=${2:?--out needs a value}; shift 2 ;;
    --notes-url) NOTES=${2:?--notes-url needs a value}; shift 2 ;;
    --key) KEY=${2:?--key needs a path}; shift 2 ;;
    --test-build) TEST=1; shift ;;
    --no-manifest) NO_MANIFEST=1; shift ;;
    --base-url) BASE_URL=${2:?--base-url needs a value}; shift 2 ;;
    --pubkey) PUBKEY_FILE=${2:?--pubkey needs a path}; shift 2 ;;
    --verify-pubkey) VERIFY_PUBKEY=${2:?--verify-pubkey needs a path}; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown argument: $1" >&2; usage ;;
  esac
done
[ -n "$OUT" ] || die "--out is required"
if [ "$NO_MANIFEST" = 1 ] && { [ "$TEST" = 1 ] || [ -n "$VERIFY_PUBKEY" ]; }; then
  die "--no-manifest is for a release's signature only: not with --test-build or --verify-pubkey, whose checks need the manifest tool"
fi
if [ "$TEST" = 0 ] && { [ -n "$BASE_URL" ] || [ -n "$PUBKEY_FILE" ]; }; then
  die "--base-url and --pubkey are for --test-build only; a release is verified with tauri.conf.json's key and served from the release"
fi
if [ "$TEST" = 1 ]; then
  [ -z "$VERIFY_PUBKEY" ] || die "--verify-pubkey is for a key rotation's release, not a --test-build"
  [ -n "$KEY" ] || die "--test-build needs its throwaway --key: test artifacts are never signed with the release key"
  [ -n "$PUBKEY_FILE" ] || die "--test-build needs its throwaway --pubkey: test artifacts are never checked against the release key"
  # A release key by any name, before anything is signed with it: every key
  # file in ~/.config/droppen (tauri-updater.key, and the -old and -new ones
  # around a rotation: apps not yet past the transition still trust the old
  # one) by its path, a link to it or a copy of its bytes, and a copy of CI's
  # TAURI_SIGNING_PRIVATE_KEY. Copies are compared without trailing newlines,
  # which a secret or an editor may add or drop.
  [ -f "$KEY" ] || die "no key at $KEY"
  real_path() { echo "$(cd "$(dirname "$1")" 2>/dev/null && pwd -P)/$(basename "$1")"; }
  KEY_TEXT=$(cat "$KEY")
  for release_key in "$HOME"/.config/droppen/*.key; do
    [ -f "$release_key" ] || continue
    if [ "$(real_path "$KEY")" = "$(real_path "$release_key")" ] || [ "$KEY_TEXT" = "$(cat "$release_key")" ]; then
      die "--test-build with $(basename "$release_key") ($KEY): test artifacts are signed with a throwaway key only"
    fi
  done
  if [ -n "$ENV_KEY" ] && [ "$KEY_TEXT" = "$(printf '%s' "$ENV_KEY")" ]; then
    die "--test-build with CI's TAURI_SIGNING_PRIVATE_KEY ($KEY): test artifacts are signed with a throwaway key only"
  fi
  unset KEY_TEXT
fi

DESKTOP=$(cd "$(dirname "$0")" && pwd -P)
# The personal-name rule of the release's text checks (scripts/release-public.sh
# uses the same): "Kilerd" only as the copyright notice, and none of the names
# in DROPPEN_RELEASE_FORBIDDEN, which a release must hold as the pinned list.
# A test build, never uploaded, checks the names only when they are set.
. "$DESKTOP/../scripts/release-text-check.sh"
# The operator's file, read as data (never run), when the names are not set.
release_text_load_names "$HOME/.config/droppen/release-forbidden.env"
if [ "$TEST" = 0 ] && release_text_names_deferred; then
  # Only release.yml's dry run without the names (docs/releasing.md) gets here.
  echo "note: DROPPEN_RELEASE_NAMES_DEFERRED: the text checks look for \"Kilerd\" only, not for the names"
elif [ "$TEST" = 0 ]; then
  release_text_names_ok \
    || die "DROPPEN_RELEASE_FORBIDDEN is not the expected list of names (set it, or keep it in ~/.config/droppen/release-forbidden.env; its sha256 is RELEASE_TEXT_NAMES_SHA256 in scripts/release-text-check.sh)"
  release_text_allow_ok \
    || die "DROPPEN_RELEASE_NOTICE_ALLOW may only spell out whole third-party copyright lines, each ^Copyright …\$ with its metacharacters escaped"
elif ! release_text_names_ok; then
  echo "note: DROPPEN_RELEASE_FORBIDDEN is not the release list; this test build checks only \"Kilerd\" and $(id -un)"
fi
REPO_PUBLIC=${DROPPEN_RELEASE_REPO:-getdroppen/droppen}
ARCHIVE=Droppen.app.tar.gz
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "$VERSION is not a semantic version"
if [ "$TEST" = 0 ] && ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then die "a release version is x.y.z, not $VERSION"; fi
BASE_URL=${BASE_URL:-https://github.com/$REPO_PUBLIC/releases/download/v$VERSION}
NOTES=${NOTES:-https://github.com/$REPO_PUBLIC/releases/tag/v$VERSION}
RELEASE_PUBKEY=$DESKTOP/src-tauri/tauri.conf.json
PUBKEY_FILE=${PUBKEY_FILE:-${VERIFY_PUBKEY:-$RELEASE_PUBKEY}}

[ "$(uname -s)" = Darwin ] || die "this needs a Mac (codesign, stapler)"
[ -d "$APP" ] && [ -f "$APP/Contents/Info.plist" ] || die "$APP is not an app bundle"
APP=$(cd "$APP" && pwd -P)
if [ -e "$OUT" ] && [ -n "$(ls -A "$OUT")" ]; then die "$OUT is not empty"; fi
case "$OUT" in *" "*) die "no spaces in the output path, please" ;; esac
# The key: --key's file, else the environment's (GitHub Actions secrets), else
# the release Mac's file.
if [ -z "$KEY" ] && [ -n "$ENV_KEY" ]; then
  KEY_FROM="TAURI_SIGNING_PRIVATE_KEY"
  KEY_PASSWORD=$ENV_PASSWORD
else
  KEY=${KEY:-$HOME/.config/droppen/tauri-updater.key}
  KEY_FROM=$(basename "$KEY")
  [ -f "$KEY" ] || die "no updater key at $KEY, and TAURI_SIGNING_PRIVATE_KEY is not set (desktop/SIGNING.md, \"The key\")"
  [ "$(stat -f %Lp "$KEY")" = 600 ] || die "$KEY must be 0600"
  if [ -f "$KEY.password" ]; then
    [ "$(stat -f %Lp "$KEY.password")" = 600 ] || die "$KEY.password must be 0600"
    KEY_PASSWORD=$(cat "$KEY.password")
  else
    KEY_PASSWORD=$ENV_PASSWORD
  fi
  ENV_KEY=""
fi
unset ENV_PASSWORD

say "the app: $APP"
ID=$(plutil -extract CFBundleIdentifier raw "$APP/Contents/Info.plist")
APP_VERSION=$(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist")
# A release is app.droppen.desktop; a --test-build is an end-to-end build,
# which has an identity of its own (desktop/e2e-identity.sh).
if [ "$TEST" = 1 ]; then WANT_ID=app.droppen.desktop.e2e; else WANT_ID=app.droppen.desktop; fi
[ "$ID" = "$WANT_ID" ] || die "bundle id is $ID, not $WANT_ID"
[ "$APP_VERSION" = "$VERSION" ] || die "the app is version $APP_VERSION, not $VERSION: the updater would offer it as $VERSION"
if LC_ALL=C grep -aqF DROPPEN_DESKTOP_TEST_CONTROL "$APP/Contents/MacOS/Droppen"; then
  [ "$TEST" = 1 ] || die "$APP is an end-to-end test build (desktop/build.sh --e2e): never shipped"
fi
codesign --verify --deep --strict "$APP" || die "$APP: the code signature does not verify"
AUTHORITY=$(codesign -dv --verbose=2 "$APP" 2>&1 | sed -n 's/^Authority=//p' | head -1)
if [ "$TEST" = 0 ]; then
  case "$AUTHORITY" in
    "Developer ID Application:"*) ;;
    *) die "$APP is not signed with a Developer ID (${AUTHORITY:-ad hoc}): sign and notarise it first with desktop/notarise.sh $(dirname "$APP") $VERSION, which needs the Developer ID certificate and the notarytool profile droppen-notary in this Mac's keychain (desktop/notarise.sh --check says what is missing; desktop/SIGNING.md, \"What the release Mac needs\")" ;;
  esac
  xcrun stapler validate "$APP" >/dev/null || die "$APP has no stapled notarisation ticket: desktop/notarise.sh notarises and staples it (desktop/SIGNING.md)"
  spctl --assess --type execute "$APP" || die "Gatekeeper does not accept $APP"
  # One archive serves both of the updater's macOS targets.
  for bin in "$APP/Contents/MacOS/Droppen" "$APP/Contents/MacOS/remote-hand-server"; do
    lipo "$bin" -verify_arch arm64 x86_64 || die "$bin is not universal: the one archive is offered to both Apple silicon and Intel Macs"
  done
  # What installs from the update ships its licence and notices too, and
  # they pass the release text check again (the names were checked above).
  LEGAL_PROBLEMS=$(release_text_app_legal_problems "$APP")
  [ -z "$LEGAL_PROBLEMS" ] || die "$APP's licence and notices are not fit to ship (desktop/build.sh puts them in Contents/Resources):
$LEGAL_PROBLEMS"
  echo "ok: Developer ID, notarised and stapled, accepted by Gatekeeper, universal, with LICENSE and THIRD_PARTY_NOTICES"
else
  echo "test build: ${AUTHORITY:-ad hoc} signature, not checked for notarisation"
fi

# The signer of this checkout (desktop/update-signer: minisign, as the Tauri
# updater checks it, and few dependencies), or a build of it
# (DROPPEN_UPDATE_SIGNER: release.yml builds it in a job of its own).
signer_tool() {
  if [ -n "${DROPPEN_UPDATE_SIGNER:-}" ]; then
    "$DROPPEN_UPDATE_SIGNER" "$@"
    return
  fi
  cargo run --quiet --release --locked --manifest-path "$DESKTOP/update-signer/Cargo.toml" -- "$@"
}

# The manifest tool of this checkout (desktop/core), or a build of it
# (DROPPEN_UPDATE_MANIFEST: release.yml builds it in a job of its own, so that
# none of its build scripts runs in the job that holds the key).
manifest_tool() {
  if [ -n "${DROPPEN_UPDATE_MANIFEST:-}" ]; then
    "$DROPPEN_UPDATE_MANIFEST" "$@"
    return
  fi
  cargo run --quiet --release --locked --manifest-path "$DESKTOP/Cargo.toml" -p droppen-desktop-core --bin droppen-update-manifest -- "$@"
}

if [ "$NO_MANIFEST" = 1 ]; then
  echo "--no-manifest: the key is checked when latest.json is written (droppen-update-manifest write)"
else
say "the key it is checked with"
RELEASE_KEY_ID=$(manifest_tool key-id --pubkey "$RELEASE_PUBKEY") || die "cannot read the updater key in $RELEASE_PUBKEY"
CHECK_KEY_ID=$(manifest_tool key-id --pubkey "$PUBKEY_FILE") || die "$PUBKEY_FILE holds no minisign public key"
if [ "$TEST" = 1 ]; then
  [ "$CHECK_KEY_ID" != "$RELEASE_KEY_ID" ] \
    || die "--pubkey is the release key ($RELEASE_KEY_ID): a test build is signed and checked with a throwaway key only"
  # Nor any other key of ~/.config/droppen (an old or a new one around a rotation).
  for pub in "$HOME"/.config/droppen/*.pub; do
    [ -f "$pub" ] || continue
    pub_id=$(manifest_tool key-id --pubkey "$pub" 2>/dev/null) || continue
    [ "$CHECK_KEY_ID" != "$pub_id" ] \
      || die "--pubkey is the key of $(basename "$pub") ($pub_id): a test build is signed and checked with a throwaway key only"
  done
  echo "test key $CHECK_KEY_ID (the release key is $RELEASE_KEY_ID)"
elif [ -n "$VERIFY_PUBKEY" ]; then
  [ "$CHECK_KEY_ID" != "$RELEASE_KEY_ID" ] \
    || die "--verify-pubkey is tauri.conf.json's own key ($RELEASE_KEY_ID): without a rotation, leave it out"
  echo "KEY ROTATION: signed and checked with $CHECK_KEY_ID, the key installed apps trust; the app carries $RELEASE_KEY_ID (desktop/SIGNING.md, \"Rotating the key\")"
else
  echo "the release key, $RELEASE_KEY_ID"
fi

fi # (the key's checks, without --no-manifest)

WORK=$(mktemp -d /tmp/droppen-updater-artifacts.XXXXXX)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT"

say "$ARCHIVE"
# A copy named Droppen.app, without extended attributes (quarantine and
# provenance marks are this Mac's, not the release's; the signature and the
# stapled ticket live in the bundle's files). The archive's entries all start
# with Droppen.app/ — the updater drops that first component — and name no
# owner but root: the archive is published.
ditto --noextattr --noqtn --noacl "$APP" "$WORK/stage/Droppen.app"
codesign --verify --deep --strict "$WORK/stage/Droppen.app" || die "the copy without extended attributes does not verify"
COPYFILE_DISABLE=1 tar -c --no-mac-metadata --no-xattrs --no-acls --no-fflags \
  --uid 0 --gid 0 --uname root --gname wheel \
  -z -f "$OUT/$ARCHIVE" -C "$WORK/stage" Droppen.app
STRAY=$(tar -tzf "$OUT/$ARCHIVE" | grep -v -e '^Droppen\.app$' -e '^Droppen\.app/' || true)
[ -z "$STRAY" ] || die "the archive has entries outside Droppen.app/: $(echo "$STRAY" | head -3)"
OWNERS=$(tar -tvzf "$OUT/$ARCHIVE" | awk '{ print $3 " " $4 }' | sort -u)
[ "$OWNERS" = "root wheel" ] || die "the archive names owners other than root/wheel: $OWNERS"
tar -tvzf "$OUT/$ARCHIVE" >"$WORK/listing"
HITS=$(release_text_hits "$WORK/listing"; LC_ALL=C grep -niF -e "$(id -un)" "$WORK/listing" || true)
[ -z "$HITS" ] || die "the archive names a person: $(printf '%s\n' "$HITS" | head -3)"
mkdir "$WORK/unpacked"
tar -xzf "$OUT/$ARCHIVE" -C "$WORK/unpacked"
codesign --verify --deep --strict "$WORK/unpacked/Droppen.app" || die "the app unpacked from the archive does not verify"
if [ "$TEST" = 0 ]; then
  xcrun stapler validate "$WORK/unpacked/Droppen.app" >/dev/null || die "the unpacked app lost its stapled ticket"
fi
echo "ok: $(du -h "$OUT/$ARCHIVE" | awk '{ print $1 }'), entries under Droppen.app/ only, owned by root, the unpacked app verifies"

say "signature ($KEY_FROM)"
# The key (when it comes from the environment) and its password reach the
# signer in its environment, for this one command (an assignment before the
# command, never `env VAR=…`, whose arguments any process may read).
if [ "$KEY_FROM" = TAURI_SIGNING_PRIVATE_KEY ]; then
  TAURI_SIGNING_PRIVATE_KEY=$ENV_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$KEY_PASSWORD \
    signer_tool sign "$OUT/$ARCHIVE" --version "$VERSION" >/dev/null \
    || die "signing failed (wrong TAURI_SIGNING_PRIVATE_KEY_PASSWORD?)"
else
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$KEY_PASSWORD \
    signer_tool sign "$OUT/$ARCHIVE" --version "$VERSION" --key-file "$KEY" >/dev/null \
    || die "signing failed (wrong key password?)"
fi
unset KEY_PASSWORD ENV_KEY
[ -s "$OUT/$ARCHIVE.sig" ] || die "no $ARCHIVE.sig"
echo "ok: $ARCHIVE.sig ($(base64 -d <"$OUT/$ARCHIVE.sig" | sed -n 's/^trusted comment: //p' | tr '\t' ' '))"

if [ "$NO_MANIFEST" = 1 ]; then
  say "made in $OUT (--no-manifest)"
  ls -l "$OUT"
  (cd "$OUT" && shasum -a 256 "$ARCHIVE" "$ARCHIVE.sig")
  exit 0
fi

say "latest.json"
MANIFEST_ARGS=(write --version "$VERSION" --archive "$OUT/$ARCHIVE" --signature "$OUT/$ARCHIVE.sig"
  --pubkey "$PUBKEY_FILE" --url "$BASE_URL/$ARCHIVE" --notes "$NOTES" --out "$OUT/latest.json")
[ "$TEST" = 0 ] || MANIFEST_ARGS+=(--loopback-http)
if ! manifest_tool "${MANIFEST_ARGS[@]}"; then
  # Signed by another key than the one checked with (or not a manifest the
  # app takes): the signature is no use to anyone, and must not lie about.
  rm -f "$OUT/$ARCHIVE.sig"
  die "the manifest was refused (the signature above is removed)"
fi
HITS=$(release_text_hits "$OUT/latest.json"; LC_ALL=C grep -niF -e "$(id -un)" "$OUT/latest.json" || true)
[ -z "$HITS" ] || die "latest.json names a person: $(printf '%s\n' "$HITS" | head -3)"
cat "$OUT/latest.json"

say "made in $OUT"
ls -l "$OUT"
(cd "$OUT" && shasum -a 256 "$ARCHIVE" "$ARCHIVE.sig" latest.json)
if [ "$TEST" = 1 ]; then
  echo
  echo "TEST ARTIFACTS ($KEY_FROM): for the end-to-end test only. Never upload them."
  exit 0
fi
cat <<EOF

Nothing has been published. scripts/publish-release.sh publishes them with
the signed, notarised DMG and the server's files: the release stays a draft
until latest.json is uploaded, last, and only then becomes the latest
release, whose latest.json offers installed apps this version. With --out
<dir>/updater beside the app and the DMG:

  scripts/publish-release.sh v${VERSION} --server <release-public.sh's --out> --desktop <dir> --dry-run
EOF
