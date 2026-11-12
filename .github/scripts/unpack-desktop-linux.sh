#!/usr/bin/env bash
# The Linux app's artifacts of a release.yml run, each checked by the sha256
# its job put out (.github/scripts/unpack-artifact.sh), side by side in
# <dest>, as scripts/publish-release.sh --desktop-linux takes them:
#
#   .github/scripts/unpack-desktop-linux.sh <downloads> <dest> <x.y.z>
#
# <downloads>/desktop-linux-<arch>/desktop-linux-<arch>.tar, for x86_64 and
#     aarch64 (LINUX_X86_64_SHA, LINUX_AARCH64_SHA): the AppImage, the .deb and
#     LIBRARIES-<arch>.txt (.github/workflows/desktop-linux.yml's build)
# <downloads>/desktop-linux-sources/Droppen-<x.y.z>-linux-sources.tar, when
#     LINUX_SOURCES_SHA is set: the file itself (desktop-linux.yml's sources),
#     whose sha256 that is
# <downloads>/signed-linux/signed-linux.tar, when LINUX_SIGNED_SHA is set: the
#     AppImages' updater signatures (the release job's)
#
# The allowlists are release.yml's ALLOW_DESKTOP_LINUX and ALLOW_SIGNED_LINUX.
# <dest> must be empty or absent, and ends up with exactly these files.
set -euo pipefail

DL=${1:?usage: unpack-desktop-linux.sh <downloads> <dest> <x.y.z>}
DEST=${2:?usage: unpack-desktop-linux.sh <downloads> <dest> <x.y.z>}
PLAIN=${3:?usage: unpack-desktop-linux.sh <downloads> <dest> <x.y.z>}
: "${LINUX_X86_64_SHA:?}" "${LINUX_AARCH64_SHA:?}" "${ALLOW_DESKTOP_LINUX:?}"
die() { printf 'unpack-desktop-linux: %s\n' "$*" >&2; exit 1; }
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

[[ "$PLAIN" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$PLAIN is not a version"
if [ -e "$DEST" ] && [ -n "$(ls -A "$DEST")" ]; then die "$DEST is not empty"; fi
ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
UNPACK=$ROOT/.github/scripts/unpack-artifact.sh
WORK=$(mktemp -d "${TMPDIR:-/tmp}/unpack-desktop-linux.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$DEST"

want=()
for arch in x86_64 aarch64; do
  if [ "$arch" = x86_64 ]; then sum=$LINUX_X86_64_SHA debarch=amd64; else sum=$LINUX_AARCH64_SHA debarch=arm64; fi
  "$UNPACK" "$DL/desktop-linux-$arch/desktop-linux-$arch.tar" "$sum" "$WORK/$arch" "$ALLOW_DESKTOP_LINUX"
  for name in "Droppen-$PLAIN-$arch.AppImage" "droppen_${PLAIN}_$debarch.deb" "LIBRARIES-$arch.txt"; do
    [ -f "$WORK/$arch/desktop-linux/$name" ] || die "desktop-linux-$arch.tar has no $name"
    mv "$WORK/$arch/desktop-linux/$name" "$DEST/"
    want+=("$name")
  done
done
if [ -n "${LINUX_SOURCES_SHA:-}" ]; then
  name=Droppen-$PLAIN-linux-sources.tar
  [ "$(ls -A "$DL/desktop-linux-sources")" = "$name" ] || die "desktop-linux-sources holds other than $name"
  file=$DL/desktop-linux-sources/$name
  [ -f "$file" ] && [ ! -L "$file" ] || die "$name is not a file"
  [ "$(sha256 "$file")" = "$LINUX_SOURCES_SHA" ] || die "$name is not the file desktop-linux.yml's sources job made"
  mv "$file" "$DEST/"
  want+=("$name")
fi
if [ -n "${LINUX_SIGNED_SHA:-}" ]; then
  : "${ALLOW_SIGNED_LINUX:?}"
  "$UNPACK" "$DL/signed-linux/signed-linux.tar" "$LINUX_SIGNED_SHA" "$WORK/signed" "$ALLOW_SIGNED_LINUX"
  for arch in x86_64 aarch64; do
    name=Droppen-$PLAIN-$arch.AppImage.sig
    [ -f "$WORK/signed/linux/$name" ] || die "signed-linux.tar has no $name"
    mv "$WORK/signed/linux/$name" "$DEST/"
    want+=("$name")
  done
fi
[ "$(cd "$DEST" && ls | LC_ALL=C sort)" = "$(printf '%s\n' "${want[@]}" | LC_ALL=C sort)" ] || die "$DEST does not hold exactly the Linux app's files"
echo "ok: the Linux app's ${#want[@]} files in $DEST: ${want[*]}"
