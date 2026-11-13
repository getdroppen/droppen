#!/usr/bin/env bash
# The Linux app's artifacts of a release.yml run, each checked by the sha256
# its job put out (.github/scripts/unpack-artifact.sh), side by side in
# <dest>, as scripts/publish-release.sh --desktop-linux takes them:
#
#   .github/scripts/unpack-desktop-linux.sh builds|all <downloads> <dest> <x.y.z>
#
# builds  the release job's: the two builds, to read and sign
#         <downloads>/desktop-linux-<arch>/desktop-linux-<arch>.tar, for x86_64
#         and aarch64 (LINUX_X86_64_SHA, LINUX_AARCH64_SHA): the AppImage, the
#         .deb and LIBRARIES-<arch>.txt (.github/workflows/desktop-linux.yml's
#         build)
# all     verify's and publish's: the release's nine files, each one required
#         the builds, and
#         <downloads>/desktop-linux-sources/Droppen-<x.y.z>-linux-sources.tar
#         (LINUX_SOURCES_SHA): the file itself (desktop-linux.yml's sources
#         job), whose sha256 that is, and a plain tar (tar-guard.sh: what
#         reads it later decompresses nothing)
#         <downloads>/signed-linux/signed-linux.tar (LINUX_SIGNED_SHA): the
#         AppImages' updater signatures (the release job's)
#
# The allowlists are release.yml's ALLOW_DESKTOP_LINUX and ALLOW_SIGNED_LINUX.
# <dest> must be empty or absent, and ends up with exactly these files.
set -euo pipefail

USAGE='usage: unpack-desktop-linux.sh builds|all <downloads> <dest> <x.y.z>'
MODE=${1:?$USAGE}
DL=${2:?$USAGE}
DEST=${3:?$USAGE}
PLAIN=${4:?$USAGE}
die() { printf 'unpack-desktop-linux: %s\n' "$*" >&2; exit 1; }
case "$MODE" in
  builds) ;;
  all) [ -n "${LINUX_SOURCES_SHA:-}" ] && [ -n "${LINUX_SIGNED_SHA:-}" ] \
      || die "all: the sources tar's and the signatures' sha256s are required (LINUX_SOURCES_SHA, LINUX_SIGNED_SHA): their jobs put out none" ;;
  *) die "$USAGE" ;;
esac
: "${LINUX_X86_64_SHA:?}" "${LINUX_AARCH64_SHA:?}" "${ALLOW_DESKTOP_LINUX:?}"
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

[[ "$PLAIN" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$PLAIN is not a version"
if [ -e "$DEST" ] && [ -n "$(ls -A "$DEST")" ]; then die "$DEST is not empty"; fi
ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
UNPACK=$ROOT/.github/scripts/unpack-artifact.sh
# shellcheck source=tar-guard.sh
. "$ROOT/.github/scripts/tar-guard.sh"
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
if [ "$MODE" = all ]; then
  name=Droppen-$PLAIN-linux-sources.tar
  [ -d "$DL/desktop-linux-sources" ] || die "no desktop-linux-sources artifact"
  [ "$(ls -A "$DL/desktop-linux-sources")" = "$name" ] || die "desktop-linux-sources holds other than $name"
  file=$DL/desktop-linux-sources/$name
  [ -f "$file" ] && [ ! -L "$file" ] || die "$name is not a file"
  [ "$(sha256 "$file")" = "$LINUX_SOURCES_SHA" ] || die "$name is not the file desktop-linux.yml's sources job made"
  tar_guard_plain "$file" || die "$name is not a plain tar"
  mv "$file" "$DEST/"
  want+=("$name")
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
echo "ok: the Linux app's ${#want[@]} files ($MODE) in $DEST: ${want[*]}"
