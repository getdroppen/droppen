#!/usr/bin/env bash
# One prebuilt release tool, as .github/release-tools.lock pins it: upstream's
# release file of the pinned version, downloaded and checked against the
# lock's sha256, then its program put in <dest>/bin. Fails on any difference,
# and never builds or falls back to anything else.
#
#   .github/scripts/fetch-release-tool.sh <tool> <dest> [<platform>]
#
# <tool>      cargo-about or tauri-cli, in the version the source pins
#             (cargo-about: ABOUT_VERSION in scripts/release-public.sh;
#             tauri-cli: TAURI_CLI_VERSION in desktop/build.sh), which the
#             lock must name; or one of the AppImage tools tauri-cli's bundler
#             runs to make the Linux app's AppImage, which the lock alone
#             pins, in one version for both platforms and never a rolling
#             tag (`continuous`):
#               linuxdeploy       the build of it tauri-apps publishes and the
#                                 bundler names (its LINUXDEPLOY_COMMIT_HASH),
#                                 with its own appimage plugin and appimagetool
#               apprun            the bundler's AppRun
#               appimage-runtime  the AppImage type 2 runtime, the program at
#                                 the head of every AppImage, which appimagetool
#                                 would otherwise download from `continuous`
# <platform>  the Rust target the program is for (aarch64-apple-darwin, the
#             macOS runners', by default); the AppImage tools are for
#             x86_64-unknown-linux-gnu and aarch64-unknown-linux-gnu only.
#
# The program goes in <dest>/bin under the name its user looks for:
# cargo-about, cargo-tauri, and for the AppImage tools the names in the
# bundler's tools directory (linuxdeploy-<version>-<arch>.AppImage,
# AppRun-<arch>) and runtime-<arch>.
#
# Where each tool comes from is fixed here, not in the lock: the URL is made
# from the tool, the version and the platform (upstream_url below), and the
# lock's url must be exactly it. A lock naming any other file (another
# repository, release or name) is refused, whatever its hash.
set -euo pipefail

USAGE='usage: fetch-release-tool.sh <cargo-about|tauri-cli|linuxdeploy|apprun|appimage-runtime> <dest> [<platform>]'
TOOL=${1:?$USAGE}
DEST=${2:?$USAGE}
PLATFORM=${3:-aarch64-apple-darwin}
ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
LOCK=$ROOT/.github/release-tools.lock
die() { printf 'fetch-release-tool: %s: %s\n' "$TOOL" "$*" >&2; exit 1; }

# Upstream's release file of <tool> <version> for <platform>: the one place
# that says where a tool comes from (scripts/update-release-tool-pins.sh
# makes the same URLs). Nothing for a platform upstream has no file for.
upstream_url() { # tool, version, platform
  local arch=${3%%-*}
  case "$1:$3" in
    cargo-about:*) printf 'https://github.com/EmbarkStudios/cargo-about/releases/download/%s/cargo-about-%s-%s.tar.gz\n' "$2" "$2" "$3" ;;
    tauri-cli:*-apple-darwin) printf 'https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v%s/cargo-tauri-%s.zip\n' "$2" "$3" ;;
    tauri-cli:*-unknown-linux-gnu) printf 'https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v%s/cargo-tauri-%s.tgz\n' "$2" "$3" ;;
    linuxdeploy:*-unknown-linux-gnu) printf 'https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy-%s/linuxdeploy-%s.AppImage\n' "$2" "$arch" ;;
    apprun:*-unknown-linux-gnu) printf 'https://github.com/tauri-apps/binary-releases/releases/download/apprun-%s/AppRun-%s\n' "$2" "$arch" ;;
    appimage-runtime:*-unknown-linux-gnu) printf 'https://github.com/AppImage/type2-runtime/releases/download/%s/runtime-%s\n' "$2" "$arch" ;;
    *) return 1 ;;
  esac
}

case "$TOOL" in
  cargo-about) PINNED=$(sed -n 's/^ABOUT_VERSION=//p' "$ROOT/scripts/release-public.sh") ;;
  tauri-cli) PINNED=$(sed -n 's/^TAURI_CLI_VERSION=//p' "$ROOT/desktop/build.sh") ;;
  # Pinned by the lock alone.
  linuxdeploy | apprun | appimage-runtime) PINNED="" ;;
  *) die "not a tool of .github/release-tools.lock" ;;
esac
[ -z "$PINNED" ] || [[ "$PINNED" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "cannot read the version the source pins (got '$PINNED')"
[[ "$PLATFORM" =~ ^(aarch64|x86_64)-(apple-darwin|unknown-linux-gnu)$ ]] || die "$PLATFORM is not a platform the release builds on"
ARCH=${PLATFORM%%-*}
LINE=$(awk -v tool="$TOOL" -v platform="$PLATFORM" '!/^#/ && $1 == tool && $3 == platform' "$LOCK")
[ -n "$LINE" ] || die "no line for $PLATFORM in $LOCK"
[ "$(printf '%s\n' "$LINE" | wc -l | tr -d ' ')" = 1 ] || die "more than one line for $PLATFORM in $LOCK"
read -r _ VERSION _ SHA256 URL <<<"$LINE"
if [ -n "$PINNED" ]; then
  [ "$VERSION" = "$PINNED" ] || die "$LOCK pins $VERSION, but the source wants $PINNED: run scripts/update-release-tool-pins.sh and commit its lines"
else
  # A release's own tag: a rolling one (`continuous`) is a different file
  # every time upstream builds, which no hash pins for long.
  [[ "$VERSION" =~ ^[0-9A-Za-z][0-9A-Za-z._-]*$ ]] && [[ "$VERSION" != *..* ]] || die "$LOCK pins '$VERSION', which is not a release tag"
  case "$VERSION" in continuous | latest | nightly) die "$LOCK pins $VERSION, a rolling tag: pin one of upstream's tagged releases" ;; esac
  VERSIONS=$(awk -v tool="$TOOL" '!/^#/ && $1 == tool { print $2 }' "$LOCK" | sort -u)
  [ "$VERSIONS" = "$VERSION" ] || die "$LOCK pins $(printf '%s' "$VERSIONS" | tr '\n' ' ')for its platforms: one version for both"
fi
[[ "$SHA256" =~ ^[0-9a-f]{64}$ ]] || die "$LOCK has no sha256 for it"
WANT_URL=$(upstream_url "$TOOL" "$VERSION" "$PLATFORM") || die "upstream publishes no $TOOL for $PLATFORM"
[ "$URL" = "$WANT_URL" ] || die "$LOCK names $URL, not upstream's release file $WANT_URL"

case "$TOOL" in
  cargo-about) PROGRAM=cargo-about ;;
  tauri-cli) PROGRAM=cargo-tauri ;;
  linuxdeploy) PROGRAM=linuxdeploy-$VERSION-$ARCH.AppImage ;;
  apprun) PROGRAM=AppRun-$ARCH ;;
  appimage-runtime) PROGRAM=runtime-$ARCH ;;
esac

WORK=$(mktemp -d "${TMPDIR:-/tmp}/fetch-release-tool.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
FILE=$WORK/$(basename "$WANT_URL")
curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$FILE" "$WANT_URL" || die "cannot download $WANT_URL"
if command -v sha256sum >/dev/null 2>&1; then GOT=$(sha256sum "$FILE"); else GOT=$(shasum -a 256 "$FILE"); fi
GOT=${GOT%% *}
[ "$GOT" = "$SHA256" ] || die "$(basename "$URL") is $GOT, not the pinned $SHA256"

# The one program, by its name in the archive; an AppImage tool is the file
# itself.
mkdir -p "$WORK/x" "$DEST/bin"
case "$TOOL:$FILE" in
  cargo-about:*.tar.gz) tar -xzf "$FILE" -C "$WORK/x" "$TOOL-$VERSION-$PLATFORM/$PROGRAM"; FOUND=$WORK/x/$TOOL-$VERSION-$PLATFORM/$PROGRAM ;;
  tauri-cli:*.tgz) tar -xzf "$FILE" -C "$WORK/x" "$PROGRAM"; FOUND=$WORK/x/$PROGRAM ;;
  tauri-cli:*.zip) unzip -q "$FILE" "$PROGRAM" -d "$WORK/x"; FOUND=$WORK/x/$PROGRAM ;;
  linuxdeploy:* | apprun:* | appimage-runtime:*) FOUND=$FILE ;;
  *) die "$(basename "$URL") is not a file this script unpacks" ;;
esac
[ -f "$FOUND" ] && [ ! -L "$FOUND" ] || die "$(basename "$URL") has no file $PROGRAM"
cp "$FOUND" "$DEST/bin/$PROGRAM"
chmod 755 "$DEST/bin/$PROGRAM"
echo "ok: $TOOL $VERSION ($PLATFORM): $(basename "$URL") is the pinned $SHA256; $DEST/bin/$PROGRAM"
