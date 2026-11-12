#!/usr/bin/env bash
# The release text check (scripts/release-text-check.sh) over the Linux app's
# text, with the names, in release.yml's release job, before anything is
# signed: the build jobs run dependency code, so they check for "Kilerd" only
# (DROPPEN_RELEASE_NAMES_DEFERRED). As release-text-check-staged.sh does with
# the server's tarballs, it reads each text out of an archive by its name,
# writes nothing by an archive's paths and runs nothing from them.
#
#   .github/scripts/release-text-check-linux.sh <dir> <x.y.z>
#
# <dir>: both architectures' Droppen-<x.y.z>-<arch>.AppImage,
# droppen_<x.y.z>_<amd64|arm64>.deb and LIBRARIES-<arch>.txt. Checked:
#   - LIBRARIES-<arch>.txt;
#   - each .deb: the names of its members, its control file, and its
#     usr/lib/Droppen/LICENSE, usr/lib/Droppen/THIRD_PARTY_NOTICES (as
#     notices) and usr/share/doc/droppen/copyright. The licence and the
#     copyright file must be the source's LICENSE, and both .debs' notices one
#     file, the Linux app's of this version;
#   - README.AppImage as the AppImages carry it: desktop/linux/README.AppImage.in
#     for this version, as notices (desktop/build-linux.sh checked that each
#     AppImage's is this, byte for byte; a squashfs is not read here). The
#     AppImages' LICENSE and THIRD_PARTY_NOTICES are the .debs' (the same
#     check).
# The names: as release-text-check-staged.sh takes them.
set -euo pipefail

DIR=${1:?usage: release-text-check-linux.sh <dir> <x.y.z>}
PLAIN=${2:?usage: release-text-check-linux.sh <dir> <x.y.z>}
ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
die() { printf 'release-text-check-linux: %s\n' "$*" >&2; exit 1; }

. "$ROOT/scripts/release-text-check.sh"
release_text_load_names "$HOME/.config/droppen/release-forbidden.env"
if release_text_names_deferred; then
  echo "note: DROPPEN_RELEASE_NAMES_DEFERRED and no names: \"Kilerd\" only"
else
  release_text_names_ok \
    || die "DROPPEN_RELEASE_FORBIDDEN is not the expected list of names (its sha256 is RELEASE_TEXT_NAMES_SHA256 in scripts/release-text-check.sh)"
  release_text_allow_ok \
    || die "DROPPEN_RELEASE_NOTICE_ALLOW may only spell out whole third-party copyright lines, each ^Copyright …\$ with its metacharacters escaped"
fi

[[ "$PLAIN" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$PLAIN is not a version"
[ -d "$DIR" ] || die "$DIR is not a directory"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/release-text-check-linux.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

FOUND=0
CHECKED=0
check() { # file, label, [notices]
  local hits
  hits=$(release_text_hits "$1" "${3:-}")
  CHECKED=$((CHECKED + 1))
  if [ -n "$hits" ]; then
    printf '%s names a person it should not:\n%s\n' "$2" "$hits" >&2
    FOUND=$((FOUND + 1))
  fi
}

for arch in x86_64 aarch64; do
  [ -f "$DIR/Droppen-$PLAIN-$arch.AppImage" ] || die "$DIR has no Droppen-$PLAIN-$arch.AppImage"
  [ -f "$DIR/LIBRARIES-$arch.txt" ] || die "$DIR has no LIBRARIES-$arch.txt"
  check "$DIR/LIBRARIES-$arch.txt" "LIBRARIES-$arch.txt"
done

for debarch in amd64 arm64; do
  deb=$DIR/droppen_${PLAIN}_$debarch.deb
  name=$(basename "$deb")
  [ -f "$deb" ] || die "$DIR has no $name"
  # Exactly the members a .deb has (Tauri writes them in this order).
  [ "$(tar -tf "$deb")" = $'debian-binary\ncontrol.tar.gz\ndata.tar.gz' ] || die "$name does not hold exactly debian-binary, control.tar.gz and data.tar.gz"
  tar -xOf "$deb" control.tar.gz | tar -xzOf - control >"$WORK/$debarch.control"
  check "$WORK/$debarch.control" "$name: control"
  tar -xOf "$deb" data.tar.gz | tar -tzf - >"$WORK/$debarch.listing"
  check "$WORK/$debarch.listing" "$name (its listing)"
  for member in usr/lib/Droppen/LICENSE usr/lib/Droppen/THIRD_PARTY_NOTICES usr/share/doc/droppen/copyright; do
    grep -qxF "$member" "$WORK/$debarch.listing" || die "$name has no $member"
    out=$WORK/$debarch.$(basename "$member")
    tar -xOf "$deb" data.tar.gz | tar -xzOf - "$member" >"$out"
    if [ "$member" = usr/lib/Droppen/THIRD_PARTY_NOTICES ]; then
      check "$out" "$name: $member" notices
    else
      check "$out" "$name: $member"
      cmp -s "$out" "$ROOT/LICENSE" || die "$name: $member is not the source's LICENSE"
    fi
  done
done
cmp -s "$WORK/amd64.THIRD_PARTY_NOTICES" "$WORK/arm64.THIRD_PARTY_NOTICES" || die "the two .debs carry different THIRD_PARTY_NOTICES"
case "$(head -n 1 "$WORK/amd64.THIRD_PARTY_NOTICES")" in
  "THIRD_PARTY_NOTICES for Droppen $PLAIN for Linux (with droppen-server $PLAIN+g"*")") ;;
  *) die "the .debs' THIRD_PARTY_NOTICES is not the Linux app's of Droppen $PLAIN (desktop/third-party-notices.sh --platform linux)" ;;
esac

sed -e "s/@VERSION@/$PLAIN/g" "$ROOT/desktop/linux/README.AppImage.in" >"$WORK/README.AppImage"
check "$WORK/README.AppImage" "README.AppImage" notices

[ "$FOUND" = 0 ] || die "$FOUND of $CHECKED texts name a person (above)"
if release_text_names_deferred; then
  echo "ok: the Linux app's $CHECKED texts: \"Kilerd\" only on the copyright lines (the names were NOT checked)"
else
  echo "ok: the Linux app's $CHECKED texts: no personal names"
fi
