#!/usr/bin/env bash
# Sign Droppen.app with the Developer ID and have Apple notarise it, then make
# its DMG around the stapled app and notarise that too, so that the app and
# the DMG in <dir> are what users download and what
# desktop/updater-artifacts.sh then packs for the updater (desktop/SIGNING.md,
# "Making a release"):
#
#   desktop/notarise.sh <dir> <version>
#   desktop/notarise.sh --check
#
#   <dir>      desktop/build.sh's --out (a universal release build): its
#              Droppen.app is signed, notarised and stapled in place, and its
#              unsigned DMG replaced by Droppen-<version>.dmg, signed,
#              notarised and stapled around it
#   <version>  the release's version (0.2.125 or v0.2.125): the app's own
#   --check    only check that this Mac can sign and notarise, and say exactly
#              what is missing; changes nothing
#
# It needs, in the login keychain (desktop/SIGNING.md, "What the release Mac
# needs"):
#   - a Developer ID Application identity: the certificate AND its private
#     key, imported from the .p12; `security find-identity -v -p codesigning`
#     lists it as "Developer ID Application: <name> (<team id>)";
#   - the notarytool keychain profile droppen-notary, saved into the login
#     keychain (`xcrun notarytool store-credentials droppen-notary ...
#     --keychain ~/Library/Keychains/login.keychain-db`).
#
# Environment:
#   DROPPEN_SIGNING_IDENTITY  the identity to sign with (its name or SHA-1),
#                             when the keychain has more than one Developer ID
#                             Application identity
#   DROPPEN_NOTARY_PROFILE    the notarytool keychain profile (droppen-notary)
#   DROPPEN_NOTARY_KEYCHAIN   the keychain file the profile is in (default: the
#                             login keychain, then wherever notarytool keeps a
#                             profile saved without --keychain)
set -euo pipefail

RELEASE_DIE_NAME='desktop/notarise.sh'
# shellcheck source=../scripts/release-lib.sh
. "$(cd "$(dirname "$0")" && pwd -P)/../scripts/release-lib.sh"

usage() { release_usage '/^set -euo/'; }

CHECK=0
case "${1:-}" in
  --check) CHECK=1; [ $# -eq 1 ] || usage ;;
  -h|--help|"") usage ;;
  *) [ $# -eq 2 ] || usage ;;
esac
PROFILE=${DROPPEN_NOTARY_PROFILE:-droppen-notary}
ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
# The release text check (scripts/release-text-check.sh), with the
# operator's names: the licence and notices it ships are checked again.
. "$ROOT/scripts/release-text-check.sh"
release_text_load_names "$HOME/.config/droppen/release-forbidden.env"
LOGIN_KEYCHAIN=$HOME/Library/Keychains/login.keychain-db
# How every notarytool call here names its credentials: set below, once the
# profile is found.
NOTARY=(--keychain-profile "$PROFILE")
[ "$(uname -s)" = Darwin ] || die "this needs a Mac (codesign, notarytool, stapler)"

# Everything missing at once, each with what to do about it.
PROBLEMS=()
for tool in codesign security hdiutil spctl ditto plutil lipo xcrun; do
  command -v "$tool" >/dev/null || PROBLEMS+=("missing $tool")
done
if ! xcrun --find notarytool >/dev/null 2>&1 || ! xcrun --find stapler >/dev/null 2>&1; then
  PROBLEMS+=("xcrun cannot find notarytool and stapler: install Xcode, or the Command Line Tools (xcode-select --install)")
fi

# "<SHA-1> <name>" per valid Developer ID Application identity, once each (the
# same certificate in two keychains is listed twice).
IDENTITIES=$(security find-identity -v -p codesigning 2>/dev/null \
  | sed -n 's/^ *[0-9][0-9]*) \([0-9A-F]\{40\}\) "\(Developer ID Application: .*\)"$/\1 \2/p' | sort -u)
if [ -n "${DROPPEN_SIGNING_IDENTITY:-}" ]; then
  CHOSEN=$(awk -v want="$DROPPEN_SIGNING_IDENTITY" '$1 == want || substr($0, 42) == want' <<<"$IDENTITIES")
else
  CHOSEN=$IDENTITIES
fi
IDENTITY=""
IDENTITY_NAME=""
if [ -z "$IDENTITIES" ]; then
  PROBLEMS+=("no Developer ID Application signing identity in the keychain (security find-identity -v -p codesigning finds none).
    Import the certificate together with its private key, exported as a .p12 from the Mac that made it:
      security import DeveloperID.p12 -k ~/Library/Keychains/login.keychain-db -T /usr/bin/codesign
    If it is listed but not valid, add Apple's \"Developer ID - G2\" intermediate certificate
    (https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer). If the private key is lost,
    make a new certificate in Xcode (Settings > Accounts > Manage Certificates > + > Developer ID Application).
    See desktop/SIGNING.md, \"What the release Mac needs\".")
elif [ -z "$CHOSEN" ]; then
  PROBLEMS+=("DROPPEN_SIGNING_IDENTITY is \"$DROPPEN_SIGNING_IDENTITY\", which names no valid Developer ID Application identity in the keychain:
$(sed 's/^/      /' <<<"$IDENTITIES")")
elif [ "$(wc -l <<<"$CHOSEN" | tr -d ' ')" -gt 1 ]; then
  PROBLEMS+=("several Developer ID Application identities; choose one by its SHA-1 with DROPPEN_SIGNING_IDENTITY:
$(sed 's/^/      /' <<<"$CHOSEN")")
else
  # Signed by its SHA-1: a name two certificates share would be ambiguous.
  IDENTITY=${CHOSEN%% *}
  IDENTITY_NAME=${CHOSEN#* }
fi

# notarytool reads the profile from a keychain and asks Apple with it: the
# very call notarising makes, so what it says here is what notarising would.
# Where to look: DROPPEN_NOTARY_KEYCHAIN; else the login keychain, where
# SIGNING.md says to save it (every session of the user can read it), then
# wherever notarytool keeps a profile saved without --keychain.
if [ -n "${DROPPEN_NOTARY_KEYCHAIN:-}" ]; then
  KEYCHAINS=("$DROPPEN_NOTARY_KEYCHAIN")
else
  KEYCHAINS=("$LOGIN_KEYCHAIN" "")
fi
FOUND=0
NOT_FOUND=()
for keychain in "${KEYCHAINS[@]}"; do
  try=(--keychain-profile "$PROFILE")
  [ -z "$keychain" ] || try+=(--keychain "$keychain")
  if NOTARY_ERR=$(xcrun notarytool history "${try[@]}" 2>&1 >/dev/null); then
    NOTARY=("${try[@]}")
    FOUND=1
    break
  elif grep -q 'No Keychain password item found' <<<"$NOTARY_ERR"; then
    NOT_FOUND+=("\`xcrun notarytool history ${try[*]}\`: $(grep -m 1 . <<<"$NOTARY_ERR")")
  elif grep -qiE 'User interaction is not allowed|errSecInteractionNotAllowed|-25308' <<<"$NOTARY_ERR"; then
    # Not Apple's answer: macOS would not open the keychain without asking,
    # and nobody can be asked (over SSH, or with no one logged in).
    PROBLEMS+=("the keychain${keychain:+ $keychain} is locked, so notarytool cannot read the profile \"$PROFILE\" ($(grep -m 1 . <<<"$NOTARY_ERR")).
    Unlock it first (security unlock-keychain ~/Library/Keychains/login.keychain-db, which asks for the
    login password), or run this from a Terminal in the logged-in session.")
    FOUND=1
    break
  else
    PROBLEMS+=("the notarytool profile \"$PROFILE\"${keychain:+ in $keychain} is there, but Apple did not take it: $(grep -m 1 . <<<"$NOTARY_ERR")
    It needs the network; a revoked app-specific password needs saving again (store-credentials).")
    FOUND=1
    break
  fi
done
if [ "$FOUND" = 0 ]; then
  PROBLEMS+=("notarytool finds no profile \"$PROFILE\" for the macOS user $(id -un):
$(printf '      %s\n' "${NOT_FOUND[@]}")
    A profile belongs to the user who saved it and to the keychain it went into. Save it into the login
    keychain, which every session of that user can read (notarytool then asks for an app-specific password
    made at appleid.apple.com, so it is never on a command line):
      xcrun notarytool store-credentials $PROFILE --apple-id <apple id> --team-id <team id> --keychain ~/Library/Keychains/login.keychain-db
    or, if it is in another keychain file, set DROPPEN_NOTARY_KEYCHAIN=<file>. See desktop/SIGNING.md,
    \"What the release Mac needs\".")
fi

if release_text_names_deferred; then
  # Only release.yml's dry run without the names (docs/releasing.md) gets here.
  echo "note: DROPPEN_RELEASE_NAMES_DEFERRED: the app's licence and notices are checked for \"Kilerd\" only, not for the names"
else
  release_text_names_ok \
    || PROBLEMS+=("the release text check's names are not the expected list: set DROPPEN_RELEASE_FORBIDDEN, or keep it in ~/.config/droppen/release-forbidden.env (desktop/SIGNING.md, \"What the release Mac needs\")")
fi
release_text_allow_ok 2>/dev/null \
  || PROBLEMS+=("DROPPEN_RELEASE_NOTICE_ALLOW may only spell out whole third-party copyright lines, each ^Copyright …\$ with its metacharacters escaped")

if [ "${#PROBLEMS[@]}" -gt 0 ]; then
  printf 'desktop/notarise.sh: this Mac cannot sign and notarise Droppen yet:\n' >&2
  for problem in "${PROBLEMS[@]}"; do printf '  - %s\n' "$problem" >&2; done
  exit 1
fi
echo "ok: signing as $IDENTITY_NAME ($IDENTITY); notarising with \`notarytool ${NOTARY[*]}\`"
[ "$CHECK" = 0 ] || exit 0

DIR=$(cd "$1" && pwd -P)
VERSION=${2#v}
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "a release version is x.y.z, not $VERSION"
APP=$DIR/Droppen.app
DMG=$DIR/Droppen-$VERSION.dmg
SERVER_BIN=$APP/Contents/MacOS/remote-hand-server
case "$DIR" in *" "*) die "no spaces in the path, please" ;; esac
[ -d "$APP" ] && [ -f "$APP/Contents/Info.plist" ] || die "no Droppen.app in $DIR (desktop/build.sh --out $DIR)"
[ ! -e "$DIR/E2E-TEST-BUILD.txt" ] || die "$DIR is an end-to-end test build: never signed for release"
if LC_ALL=C grep -aqF DROPPEN_DESKTOP_TEST_CONTROL "$APP/Contents/MacOS/Droppen"; then die "$APP is an end-to-end test build"; fi
[ "$(plutil -extract CFBundleIdentifier raw "$APP/Contents/Info.plist")" = app.droppen.desktop ] || die "$APP is not app.droppen.desktop"
APP_VERSION=$(plutil -extract CFBundleShortVersionString raw "$APP/Contents/Info.plist")
[ "$APP_VERSION" = "$VERSION" ] || die "$APP is version $APP_VERSION, not $VERSION"
for bin in "$APP/Contents/MacOS/Droppen" "$SERVER_BIN"; do
  lipo "$bin" -verify_arch arm64 x86_64 || die "$bin is not universal (arm64 and x86_64): a release is the universal build"
done
LEGAL_PROBLEMS=$(release_text_app_legal_problems "$APP")
[ -z "$LEGAL_PROBLEMS" ] || die "$APP's licence and notices are not fit to ship (desktop/build.sh puts them in Contents/Resources):
$LEGAL_PROBLEMS"

WORK=$(mktemp -d /tmp/droppen-notarise.XXXXXX)
MOUNT=""
cleanup() {
  [ -z "$MOUNT" ] || hdiutil detach -quiet "$MOUNT" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# Submit $1 to Apple and wait; on anything but Accepted, print Apple's log and
# stop. notarytool's exit status alone does not say whether it was accepted.
notarise() { # file, what
  local file=$1 what=$2 status=0 state submission
  xcrun notarytool submit "$file" "${NOTARY[@]}" --wait --output-format json >"$WORK/submit.json" || status=$?
  state=$(plutil -extract status raw "$WORK/submit.json" 2>/dev/null || echo unknown)
  submission=$(plutil -extract id raw "$WORK/submit.json" 2>/dev/null || echo "")
  if [ "$state" != Accepted ]; then
    if [ -n "$submission" ]; then
      xcrun notarytool log "$submission" "${NOTARY[@]}" >&2 || true
      echo "(Apple's log for submission $submission, above)" >&2
    fi
    die "notarisation of $what: $state (exit $status, submission ${submission:-none}): $(cat "$WORK/submit.json")"
  fi
  echo "ok: $what accepted (submission $submission)"
}

say "sign: $APP"
# Inside-out: the server sidecar, then the app. --options runtime is the
# hardened runtime notarisation requires, --timestamp a secure timestamp. No
# entitlements: the app is not sandboxed, and WKWebView's JIT runs in
# WebKit's own processes.
codesign --force --options runtime --timestamp --sign "$IDENTITY" "$SERVER_BIN"
codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

say "notarise the app (this waits for Apple, usually a few minutes)"
# The app first, on its own, so that its ticket is stapled into it: the
# updater ships it without a DMG, and the copy in the DMG carries it too.
ditto -c -k --keepParent "$APP" "$WORK/Droppen.zip"
notarise "$WORK/Droppen.zip" "the app"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"

say "DMG: $DMG"
# A fresh DMG around the stapled app, with the usual Applications link and
# the licence and notices beside it (as build.sh makes it); it replaces
# build.sh's unsigned one.
mkdir "$WORK/stage"
ditto "$APP" "$WORK/stage/Droppen.app"
ln -s /Applications "$WORK/stage/Applications"
cp "$APP/Contents/Resources/LICENSE" "$APP/Contents/Resources/THIRD_PARTY_NOTICES" "$WORK/stage/"
hdiutil create -quiet -volname Droppen -srcfolder "$WORK/stage" -ov -format UDZO "$DMG"
codesign --force --timestamp --sign "$IDENTITY" "$DMG"
notarise "$DMG" "the DMG"
xcrun stapler staple "$DMG"

say "check what a user's Mac checks"
xcrun stapler validate "$DMG"
spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"
MOUNT=$WORK/mount
mkdir "$MOUNT"
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$MOUNT" "$DMG"
xcrun stapler validate "$MOUNT/Droppen.app"
spctl --assess --type execute --verbose=2 "$MOUNT/Droppen.app"
hdiutil detach -quiet "$MOUNT"
MOUNT=""
spctl --assess --type execute --verbose=2 "$APP"
(cd "$DIR" && shasum -a 256 "Droppen-$VERSION.dmg" > SHA256SUMS && cat SHA256SUMS)

say "signed and notarised in $DIR"
cat <<EOF

Next, the updater's files from the stapled app, then the release
(desktop/SIGNING.md, "Making a release"):

  desktop/updater-artifacts.sh $APP $VERSION --out $DIR/updater
  scripts/publish-release.sh v$VERSION --server <release-public.sh's --out> --desktop $DIR --dry-run
EOF
