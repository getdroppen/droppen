#!/usr/bin/env bash
# The lines of .github/release-tools.lock for the versions the source pins
# (ABOUT_VERSION in scripts/release-public.sh, TAURI_CLI_VERSION in
# desktop/build.sh) and the AppImage tools' versions the lock pins now, or the
# ones given: downloads each upstream release file, takes its sha256, checks it
# against what upstream publishes beside it, and prints the lines. Nothing is
# written: a person reads the checks, puts the lines in
# .github/release-tools.lock and commits them.
#
#   scripts/update-release-tool-pins.sh [--about <version>] [--tauri <version>]
#                                       [--linuxdeploy <tag>] [--apprun <tag>]
#                                       [--appimage-runtime <tag>]
#
#   --linuxdeploy  the build tauri-cli's bundler names: LINUXDEPLOY_COMMIT_HASH
#                  in tauri-bundler's src/bundle/linux/appimage/linuxdeploy.rs,
#                  of the tauri-bundler version tauri-cli's Cargo.toml requires
#   --apprun       the bundler's AppRun release (apprun-<tag>, same file)
#   --appimage-runtime  a tagged release of AppImage/type2-runtime
#
# What it checks, besides taking the hash itself:
#   - cargo-about: the `.sha256` file of the same release;
#   - the AppImage runtime: its `.sig`, with the same release's
#     signing-pubkey.asc (when gpg is here; it says when it is not);
#   - all: the sha256 GitHub records for the release file (`gh`, logged in);
#   - all: a build provenance attestation, when upstream publishes one
#     (`gh attestation verify --repo <upstream>`); it says when there is none.
# Any disagreement stops it, and so does a rolling tag (`continuous`).
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }
die() { printf 'update-release-tool-pins: %s\n' "$*" >&2; exit 1; }

ROOT=$(cd "$(dirname "$0")/.." && pwd -P)
ABOUT=$(sed -n 's/^ABOUT_VERSION=//p' "$ROOT/scripts/release-public.sh")
TAURI=$(sed -n 's/^TAURI_CLI_VERSION=//p' "$ROOT/desktop/build.sh")
# What the lock pins now, for the tools it alone pins.
locked() { awk -v tool="$1" '!/^#/ && $1 == tool { print $2; exit }' "$ROOT/.github/release-tools.lock"; }
LINUXDEPLOY=$(locked linuxdeploy)
APPRUN=$(locked apprun)
RUNTIME=$(locked appimage-runtime)
while [ $# -gt 0 ]; do
  case "$1" in
    --about) ABOUT=${2:?--about needs a version}; shift 2 ;;
    --tauri) TAURI=${2:?--tauri needs a version}; shift 2 ;;
    --linuxdeploy) LINUXDEPLOY=${2:?--linuxdeploy needs a tag}; shift 2 ;;
    --apprun) APPRUN=${2:?--apprun needs a tag}; shift 2 ;;
    --appimage-runtime) RUNTIME=${2:?--appimage-runtime needs a tag}; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done
for tool in curl gh awk; do command -v "$tool" >/dev/null || die "missing $tool"; done
for tag in "$LINUXDEPLOY" "$APPRUN" "$RUNTIME"; do
  case "$tag" in "" | continuous | latest | nightly) die "'$tag' is not a tagged release (--linuxdeploy, --apprun, --appimage-runtime)" ;; esac
done
# The platforms the release's jobs run the tools on: macOS runners for the
# notices and the Mac app, Linux ones for the Linux app.
MAC_PLATFORMS=(aarch64-apple-darwin)
LINUX_PLATFORMS=(x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu)

WORK=$(mktemp -d "${TMPDIR:-/tmp}/release-tool-pins.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
sha256_of() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

LINES=()
pin() { # tool, version, upstream repo, release tag, file name
  local tool=$1 version=$2 repo=$3 tag=$4 name=$5 url file sum recorded sidecar
  url=https://github.com/$repo/releases/download/$tag/$name
  file=$WORK/$name
  printf '\n== %s %s: %s\n' "$tool" "$version" "$url" >&2
  curl -fsSL --proto '=https' -o "$file" "$url" || die "cannot download $url"
  sum=$(sha256_of "$file")
  recorded=$(gh api "repos/$repo/releases/tags/$tag" --jq ".assets[] | select(.name == \"$name\") | .digest" 2>/dev/null || true)
  [ "$recorded" = "sha256:$sum" ] || die "$name is $sum here, but GitHub records ${recorded:-nothing} for it"
  echo "ok: GitHub records the same sha256 for $name" >&2
  if [ "$tool" = cargo-about ]; then
    curl -fsSL --proto '=https' -o "$WORK/$name.sha256" "$url.sha256" || die "cannot download $url.sha256"
    sidecar=$(awk '{ print $1; exit }' "$WORK/$name.sha256")
    [ "$sidecar" = "$sum" ] || die "$name is $sum here, but its .sha256 says $sidecar"
    echo "ok: $name.sha256 says the same" >&2
  fi
  if [ "$tool" = appimage-runtime ]; then
    if command -v gpg >/dev/null; then
      curl -fsSL --proto '=https' -o "$file.sig" "$url.sig" || die "cannot download $url.sig"
      curl -fsSL --proto '=https' -o "$WORK/signing-pubkey.asc" "https://github.com/$repo/releases/download/$tag/signing-pubkey.asc" \
        || die "cannot download $tag's signing-pubkey.asc"
      mkdir -p "$WORK/gnupg" && chmod 700 "$WORK/gnupg"
      GNUPGHOME=$WORK/gnupg gpg -q --import "$WORK/signing-pubkey.asc" 2>/dev/null || die "cannot read $tag's signing-pubkey.asc"
      GNUPGHOME=$WORK/gnupg gpg -q --verify "$file.sig" "$file" 2>/dev/null || die "$name.sig does not verify with $tag's signing-pubkey.asc"
      echo "ok: $name.sig verifies with $tag's signing-pubkey.asc ($(GNUPGHOME=$WORK/gnupg gpg --with-colons --fingerprint 2>/dev/null | awk -F: '/^fpr/ { print $10; exit }'))" >&2
    else
      echo "note: no gpg here: $name.sig is not checked" >&2
    fi
  fi
  if gh attestation verify "$file" --repo "$repo" >/dev/null 2>&1; then
    echo "ok: attested by $repo (gh attestation verify)" >&2
  else
    echo "note: $repo publishes no attestation for $name: the pin is the hash alone" >&2
  fi
  LINES+=("$(printf '%-17s %-8s %-25s %s  %s' "$tool" "$version" "$PLATFORM" "$sum" "$url")")
}
for PLATFORM in "${MAC_PLATFORMS[@]}"; do
  pin cargo-about "$ABOUT" EmbarkStudios/cargo-about "$ABOUT" "cargo-about-$ABOUT-$PLATFORM.tar.gz"
  pin tauri-cli "$TAURI" tauri-apps/tauri "tauri-cli-v$TAURI" "cargo-tauri-$PLATFORM.zip"
done
for PLATFORM in "${LINUX_PLATFORMS[@]}"; do
  pin tauri-cli "$TAURI" tauri-apps/tauri "tauri-cli-v$TAURI" "cargo-tauri-$PLATFORM.tgz"
done
for PLATFORM in "${LINUX_PLATFORMS[@]}"; do
  pin linuxdeploy "$LINUXDEPLOY" tauri-apps/binary-releases "linuxdeploy-$LINUXDEPLOY" "linuxdeploy-${PLATFORM%%-*}.AppImage"
done
for PLATFORM in "${LINUX_PLATFORMS[@]}"; do
  pin apprun "$APPRUN" tauri-apps/binary-releases "apprun-$APPRUN" "AppRun-${PLATFORM%%-*}"
done
for PLATFORM in "${LINUX_PLATFORMS[@]}"; do
  pin appimage-runtime "$RUNTIME" AppImage/type2-runtime "$RUNTIME" "runtime-${PLATFORM%%-*}"
done

printf '\nThe lines for .github/release-tools.lock (replace the old ones, check, commit):\n\n' >&2
printf '%s\n' "${LINES[@]}"
