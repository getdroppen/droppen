#!/usr/bin/env bash
# The release text check (scripts/release-text-check.sh) over the Linux app's
# text, with the names, in release.yml's release job, before anything is
# signed: the build jobs run dependency code, so they check for "Kilerd" only
# (DROPPEN_RELEASE_NAMES_DEFERRED). And for a release, the .debs' maintainer.
#
#   .github/scripts/release-text-check-linux.sh [--publish] <dir> <x.y.z>
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
# --publish: a release, not a dry run. Each .deb's control must name a
# maintainer one can write to, `Maintainer: Name <local@domain>` (Debian's
# policy, 5.6.2), not the placeholder `droppen`; without it a placeholder is
# only reported. The built package is what counts: whatever the app crate's
# authors and tauri.linux.conf.json's publisher say, Tauri's bundler wrote this.
#
# The job that runs this holds the secrets, and a build job made the .debs, so
# nothing from them is run or written by their paths: their ar members are
# read here by their headers (exactly debian-binary, control.tar.gz and
# data.tar.gz), each a gzip of a plain tar, decompressed by the system's gzip,
# and each text read out of it by name with the system's tar, with the
# system's PATH: no decompressor anyone else's runs on their bytes
# (tar-guard.sh). The names: as release-text-check-staged.sh takes them.
set -euo pipefail

USAGE='usage: release-text-check-linux.sh [--publish] <dir> <x.y.z>'
PUBLISH=0
if [ "${1:-}" = --publish ]; then PUBLISH=1; shift; fi
DIR=${1:?$USAGE}
PLAIN=${2:?$USAGE}
ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
die() { printf 'release-text-check-linux: %s\n' "$*" >&2; exit 1; }
# shellcheck source=tar-guard.sh
. "$ROOT/.github/scripts/tar-guard.sh"
# Only the system's programs from here on.
export PATH=$TAR_GUARD_PATH

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

# The members of .deb $1, each as $2/<name>: exactly debian-binary ("2.0"),
# control.tar.gz and data.tar.gz, in that order (as Tauri's bundler writes
# them), by their ar headers: "!<arch>\n", then for each a 60-byte header
# (the name in 16 bytes, the size in bytes 49 to 58, "`\n") and its bytes,
# padded to an even length.
deb_members() {
  local deb=$1 out=$2 name total off=8 want hdr size
  name=$(basename "$deb")
  total=$(wc -c <"$deb" | tr -d ' ')
  [ "$(tar_guard_hex "$deb" 0 8)" = 213c617263683e0a ] || die "$name is not an ar archive, as a .deb is"
  for want in debian-binary control.tar.gz data.tar.gz; do
    [ $((off + 60)) -le "$total" ] || die "$name ends before its $want"
    # The header, anything unprintable (its closing newline too) as "~".
    hdr=$(head -c $((off + 60)) "$deb" | tail -c 60 | LC_ALL=C tr -c '[:print:]' '~')
    [ "${hdr:0:16}" = "$(printf '%-16s' "$want")" ] && [[ "${hdr:48:10}" =~ ^[0-9]+\ *$ ]] && [ "${hdr:58:2}" = '`~' ] \
      || die "$name does not hold exactly debian-binary, control.tar.gz and data.tar.gz (where $want would be: \"${hdr:0:16}\")"
    size=${hdr:48:10}
    size=$((10#${size%% *}))
    off=$((off + 60))
    [ $((off + size)) -le "$total" ] || die "$name ends inside its $want"
    head -c $((off + size)) "$deb" | tail -c "$size" >"$out/$want"
    off=$((off + size + size % 2))
  done
  [ "$off" = "$total" ] || die "$name holds more than debian-binary, control.tar.gz and data.tar.gz"
  [ "$(cat "$out/debian-binary")" = 2.0 ] || die "$name is not a .deb of format 2.0"
}

for arch in x86_64 aarch64; do
  [ -f "$DIR/Droppen-$PLAIN-$arch.AppImage" ] || die "$DIR has no Droppen-$PLAIN-$arch.AppImage"
  [ -f "$DIR/LIBRARIES-$arch.txt" ] || die "$DIR has no LIBRARIES-$arch.txt"
  check "$DIR/LIBRARIES-$arch.txt" "LIBRARIES-$arch.txt"
done

# A maintainer one can write to: "Name <local@domain>".
CONTACT='^[^<>@[:space:]][^<>@]* <[^<>@[:space:]]+@[^<>@[:space:]]+>$'
MAINTAINER=""
for debarch in amd64 arm64; do
  deb=$DIR/droppen_${PLAIN}_$debarch.deb
  name=$(basename "$deb")
  [ -f "$deb" ] && [ ! -L "$deb" ] || die "$DIR has no $name"
  m=$WORK/$debarch
  mkdir -p "$m"
  deb_members "$deb" "$m"
  for part in control data; do
    tar_guard_gunzip "$m/$part.tar.gz" "$m/$part.tar" || die "$name: $part.tar.gz is not a gzip of a plain tar"
    rm -f "$m/$part.tar.gz"
  done
  tar_guard_tar -xOf "$m/control.tar" control >"$WORK/$debarch.control"
  check "$WORK/$debarch.control" "$name: control"
  [ "$(grep -ci '^maintainer:' "$WORK/$debarch.control" || true)" = 1 ] || die "$name: its control does not name exactly one Maintainer"
  maintainer=$(sed -n 's/^Maintainer: //p' "$WORK/$debarch.control")
  [ -z "$MAINTAINER" ] || [ "$maintainer" = "$MAINTAINER" ] || die "the two .debs name different maintainers"
  MAINTAINER=$maintainer
  tar_guard_tar -tf "$m/data.tar" >"$WORK/$debarch.listing"
  check "$WORK/$debarch.listing" "$name (its listing)"
  for member in usr/lib/Droppen/LICENSE usr/lib/Droppen/THIRD_PARTY_NOTICES usr/share/doc/droppen/copyright; do
    grep -qxF "$member" "$WORK/$debarch.listing" || die "$name has no $member"
    out=$WORK/$debarch.$(basename "$member")
    tar_guard_tar -xOf "$m/data.tar" "$member" >"$out"
    if [ "$member" = usr/lib/Droppen/THIRD_PARTY_NOTICES ]; then
      check "$out" "$name: $member" notices
    else
      check "$out" "$name: $member"
      cmp -s "$out" "$ROOT/LICENSE" || die "$name: $member is not the source's LICENSE"
    fi
  done
  rm -rf "$m"
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

# The maintainer, from the built .debs (after the text check, which read it).
if [[ "$MAINTAINER" =~ $CONTACT ]] && [ "$MAINTAINER" != droppen ]; then
  echo "ok: the .debs' Maintainer is $MAINTAINER"
elif [ "$PUBLISH" = 1 ]; then
  die "the .debs' Maintainer is \"$MAINTAINER\", not a maintainer one can write to (\"Name <local@domain>\"): set bundle.publisher in desktop/src-tauri/tauri.linux.conf.json (or the app crate's authors) before a Linux release"
else
  echo "note: the .debs' Maintainer is \"$MAINTAINER\", not \"Name <local@domain>\": a release (--publish) stops here"
fi
