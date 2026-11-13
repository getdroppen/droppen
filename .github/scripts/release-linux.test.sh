#!/usr/bin/env bash
# Tests for release.yml's Linux app scripts, unpack-desktop-linux.sh and
# release-text-check-linux.sh, on stand-in artifacts:
# `bash .github/scripts/release-linux.test.sh`.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd -P)
ROOT=$(cd "$HERE/../.." && pwd -P)
UNPACK_LINUX=$HERE/unpack-desktop-linux.sh
TEXT_LINUX=$HERE/release-text-check-linux.sh
TMP=$(mktemp -d "${TMPDIR:-/tmp}/release-linux-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
PASSED=0
FAILED=0
pass() { PASSED=$((PASSED + 1)); }
fail() { FAILED=$((FAILED + 1)); echo "FAIL: $*" >&2; }
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
V=0.2.140
export COPYFILE_DISABLE=1

# The text check without the names (no secret here): "Kilerd" only.
export HOME=$TMP/home DROPPEN_RELEASE_NAMES_DEFERRED=1 DROPPEN_RELEASE_TEXT_QUIET=1
unset DROPPEN_RELEASE_FORBIDDEN DROPPEN_RELEASE_NOTICE_ALLOW
mkdir -p "$HOME"

# Logging stand-ins for every decompressor, first on PATH: none may run.
mkdir -p "$TMP/shims"
for prog in zstd unzstd lz4 lzop lrzip grzip xz unxz lzma bzip2 gzip lzip compress; do
  printf '#!/bin/sh\necho "%s $*" >>"%s/decompressor.log"\ncat\n' "$prog" "$TMP" >"$TMP/shims/$prog"
  chmod +x "$TMP/shims/$prog"
done
SHIMMED=$TMP/shims:$PATH

# An ar archive of the files given, as Tauri's bundler writes a .deb.
mkar() { # out, files...
  local out=$1 f size
  shift
  printf '!<arch>\n' >"$out"
  for f in "$@"; do
    size=$(wc -c <"$f" | tr -d ' ')
    printf '%-16s%-12s%-6s%-6s%-8s%-10s`\n' "$(basename "$f")" 0 0 0 100644 "$size" >>"$out"
    cat "$f" >>"$out"
    [ $((size % 2)) = 0 ] || printf '\n' >>"$out"
  done
}
# A stand-in .deb in $1 for arch $2, with Maintainer $3.
mkdeb() { # dir, debarch, maintainer
  local d=$TMP/deb-$2
  rm -rf "$d" && mkdir -p "$d/c" "$d/d/usr/lib/Droppen" "$d/d/usr/share/doc/droppen" "$d/d/usr/bin" "$d/m"
  cp "$ROOT/LICENSE" "$d/d/usr/lib/Droppen/LICENSE"
  cp "$ROOT/LICENSE" "$d/d/usr/share/doc/droppen/copyright"
  printf 'THIRD_PARTY_NOTICES for Droppen %s for Linux (with droppen-server %s+gabcdef12)\n\nnotices\n' "$V" "$V" >"$d/d/usr/lib/Droppen/THIRD_PARTY_NOTICES"
  echo program >"$d/d/usr/bin/Droppen"
  printf 'Package: droppen\nVersion: %s\nArchitecture: %s\nMaintainer: %s\nSection: devel\n' "$V" "$2" "$3" >"$d/c/control"
  echo 2.0 >"$d/m/debian-binary"
  tar -czf "$d/m/control.tar.gz" -C "$d/c" control
  tar -czf "$d/m/data.tar.gz" -C "$d/d" usr
  mkar "$1/droppen_${V}_$2.deb" "$d/m/debian-binary" "$d/m/control.tar.gz" "$d/m/data.tar.gz"
}
# A directory as the release job has it: both builds.
mkbuilds() { # dir, maintainer
  mkdir -p "$1"
  for arch in x86_64 aarch64; do
    echo "appimage $arch" >"$1/Droppen-$V-$arch.AppImage"
    printf '# LIBRARIES\nlibfoo:%s\t1.0\tfoo\t1.0\tusr/lib/libfoo.so.1\n' "$arch" >"$1/LIBRARIES-$arch.txt"
  done
  mkdeb "$1" amd64 "$2"
  mkdeb "$1" arm64 "$2"
}
text() { # want (ok|refused), description, [--publish], dir; the reason must match $WHY when set
  local want=$1 what=$2 got=ok
  shift 2
  rm -f "$TMP/decompressor.log"
  PATH=$SHIMMED bash "$TEXT_LINUX" "$@" "${VER:-$V}" >"$TMP/text.log" 2>&1 || got=refused
  if [ "$got" != "$want" ]; then fail "text check, $what: want $want, got $got: $(tail -n 3 "$TMP/text.log")"
  elif [ -e "$TMP/decompressor.log" ]; then fail "text check, $what: a decompressor ran: $(cat "$TMP/decompressor.log")"
  elif [ -n "${WHY:-}" ] && ! grep -qE "$WHY" "$TMP/text.log"; then fail "text check, $what: not for \"$WHY\": $(tail -n 3 "$TMP/text.log")"
  else pass; fi
}

# The text check, and the maintainer a release needs (D8).
mkbuilds "$TMP/placeholder" droppen
WHY='Maintainer is "droppen".*a release \(--publish\) stops here' text ok "a dry run with the placeholder" "$TMP/placeholder"
WHY='Maintainer is "droppen", not a maintainer one can write to' text refused "a release with the placeholder" --publish "$TMP/placeholder"
mkbuilds "$TMP/contact" "Droppen <hello@droppen.app>"
WHY='ok: the .debs. Maintainer is Droppen <hello@droppen.app>' text ok "a release with a contact" --publish "$TMP/contact"
for bad in "" "Droppen" "Droppen <hello>" "<hello@droppen.app>" "Droppen hello@droppen.app"; do
  rm -rf "$TMP/bad" && mkbuilds "$TMP/bad" "$bad"
  WHY='not a maintainer one can write to' text refused "a release with the Maintainer \"$bad\"" --publish "$TMP/bad"
done
rm -rf "$TMP/two" && mkbuilds "$TMP/two" "Droppen <hello@droppen.app>" && mkdeb "$TMP/two" arm64 "Other <other@droppen.app>"
WHY='different maintainers' text refused "two .debs with different maintainers" "$TMP/two"
rm -rf "$TMP/name" && mkbuilds "$TMP/name" droppen && echo "Copyright Kilerd Chan" >>"$TMP/name/LIBRARIES-x86_64.txt"
WHY='name a person' text refused "a name in LIBRARIES" "$TMP/name"
VER=0.2.141 WHY='has no Droppen-0.2.141' text refused "another version" "$TMP/placeholder"

# A .deb is read by its ar headers, its members only as gzips of plain tars:
# nothing else is handed to tar, and no decompressor runs.
d=$TMP/deb-amd64/m
redeb() { # description, reason, files... (the amd64 .deb's members)
  local what=$1 why=$2
  shift 2
  rm -rf "$TMP/odd" && mkbuilds "$TMP/odd" droppen
  mkar "$TMP/odd/droppen_${V}_amd64.deb" "$@"
  WHY=$why text refused "$what" "$TMP/odd"
}
mkbuilds "$TMP/members" droppen
cp -R "$TMP/deb-amd64/m" "$TMP/m"
printf '\050\265\057\375zstd frame' >"$TMP/m/zstd"
mkdir -p "$TMP/zstd-data" && cp "$TMP/m/zstd" "$TMP/zstd-data/data.tar.gz"
redeb "a zstd data.tar.gz" 'data.tar.gz is not gzip' "$TMP/m/debian-binary" "$TMP/m/control.tar.gz" "$TMP/zstd-data/data.tar.gz"
mkdir -p "$TMP/gz-zstd" && gzip -c <"$TMP/m/zstd" >"$TMP/gz-zstd/data.tar.gz"
redeb "zstd inside gzip" 'compressed \(its first bytes are 28b52ffd\)' "$TMP/m/debian-binary" "$TMP/m/control.tar.gz" "$TMP/gz-zstd/data.tar.gz"
if command -v zstd >/dev/null 2>&1; then
  mkdir -p "$TMP/real-zstd" && tar -cf - -C "$TMP/deb-amd64/d" usr | zstd -q -c >"$TMP/real-zstd/data.tar.gz"
  redeb "a real zstd data.tar.gz" 'data.tar.gz is not gzip' "$TMP/m/debian-binary" "$TMP/m/control.tar.gz" "$TMP/real-zstd/data.tar.gz"
fi
echo extra >"$TMP/m/extra"
redeb "a fourth member" 'more than debian-binary' "$TMP/m/debian-binary" "$TMP/m/control.tar.gz" "$TMP/m/data.tar.gz" "$TMP/m/extra"
redeb "members out of order" 'exactly debian-binary, control.tar.gz and data.tar.gz' "$TMP/m/debian-binary" "$TMP/m/data.tar.gz" "$TMP/m/control.tar.gz"
rm -rf "$TMP/tardeb" && mkbuilds "$TMP/tardeb" droppen && tar -cf "$TMP/tardeb/droppen_${V}_amd64.deb" -C "$TMP/m" debian-binary control.tar.gz data.tar.gz
WHY='not an ar archive' text refused "a tar, not an ar archive" "$TMP/tardeb"
head -c 100 "$TMP/members/droppen_${V}_arm64.deb" >"$TMP/members/cut" && mv "$TMP/members/cut" "$TMP/members/droppen_${V}_arm64.deb"
WHY='ends (before|inside)' text refused "a cut .deb" "$TMP/members"

# unpack-desktop-linux.sh: the builds for the release job; for verify and
# publish all nine files, each required.
export ALLOW_DESKTOP_LINUX='^desktop-linux(/(Droppen-[0-9]+\.[0-9]+\.[0-9]+-(x86_64|aarch64)\.AppImage|droppen_[0-9]+\.[0-9]+\.[0-9]+_(amd64|arm64)\.deb|LIBRARIES-(x86_64|aarch64)\.txt))?$'
export ALLOW_SIGNED_LINUX='^linux(/Droppen-[0-9]+\.[0-9]+\.[0-9]+-(x86_64|aarch64)\.AppImage\.sig)?$'
DL=$TMP/dl
for arch in x86_64 aarch64; do
  if [ "$arch" = x86_64 ]; then debarch=amd64; else debarch=arm64; fi
  mkdir -p "$TMP/art-$arch/desktop-linux" "$DL/desktop-linux-$arch"
  cp "$TMP/contact/Droppen-$V-$arch.AppImage" "$TMP/contact/droppen_${V}_$debarch.deb" "$TMP/contact/LIBRARIES-$arch.txt" "$TMP/art-$arch/desktop-linux/"
  tar -cf "$DL/desktop-linux-$arch/desktop-linux-$arch.tar" -C "$TMP/art-$arch" desktop-linux
done
mkdir -p "$TMP/sig/linux" "$DL/signed-linux" "$DL/desktop-linux-sources" "$TMP/src/Droppen-$V-linux-sources"
for arch in x86_64 aarch64; do echo sig >"$TMP/sig/linux/Droppen-$V-$arch.AppImage.sig"; done
tar -cf "$DL/signed-linux/signed-linux.tar" -C "$TMP/sig" linux
echo README >"$TMP/src/Droppen-$V-linux-sources/README"
tar -cf "$DL/desktop-linux-sources/Droppen-$V-linux-sources.tar" -C "$TMP/src" "Droppen-$V-linux-sources"
export LINUX_X86_64_SHA LINUX_AARCH64_SHA
LINUX_X86_64_SHA=$(sha "$DL/desktop-linux-x86_64/desktop-linux-x86_64.tar")
LINUX_AARCH64_SHA=$(sha "$DL/desktop-linux-aarch64/desktop-linux-aarch64.tar")
SOURCES_SHA=$(sha "$DL/desktop-linux-sources/Droppen-$V-linux-sources.tar")
SIGNED_SHA=$(sha "$DL/signed-linux/signed-linux.tar")
N=0
unpack() { # want (ok|refused), description, files wanted (ok) or the reason (refused), downloads, mode, [env...]
  local want=$1 what=$2 expect=$3 dl=$4 mode=$5 got=ok
  shift 5
  N=$((N + 1))
  rm -f "$TMP/decompressor.log"
  # A copy each time: the sources tar is moved, not copied (427 MB).
  cp -R "$dl" "$TMP/dl$N"
  env PATH="$SHIMMED" "$@" bash "$UNPACK_LINUX" "$mode" "$TMP/dl$N" "$TMP/u$N" "$V" >"$TMP/unpack.log" 2>&1 || got=refused
  if [ "$got" != "$want" ]; then fail "unpack, $what: want $want, got $got: $(tail -n 2 "$TMP/unpack.log")"
  elif [ -e "$TMP/decompressor.log" ]; then fail "unpack, $what: a decompressor ran"
  elif [ "$want" = ok ] && [ "$(ls "$TMP/u$N" | wc -l | tr -d ' ')" != "$expect" ]; then fail "unpack, $what: not $expect files"
  elif [ "$want" = refused ] && ! grep -qE "$expect" "$TMP/unpack.log"; then fail "unpack, $what: not for \"$expect\": $(tail -n 2 "$TMP/unpack.log")"
  else pass; fi
}
unpack ok "builds" 6 "$DL" builds
unpack ok "all" 9 "$DL" all LINUX_SOURCES_SHA="$SOURCES_SHA" LINUX_SIGNED_SHA="$SIGNED_SHA"
unpack refused "all, without the sources' sha256" 'sha256s are required' "$DL" all LINUX_SIGNED_SHA="$SIGNED_SHA"
unpack refused "all, without the signatures' sha256" 'sha256s are required' "$DL" all LINUX_SOURCES_SHA="$SOURCES_SHA"
unpack refused "another mode" 'usage' "$DL" sources
unpack refused "all, another sources sha256" 'not the file' "$DL" all LINUX_SOURCES_SHA="$SIGNED_SHA" LINUX_SIGNED_SHA="$SIGNED_SHA"
cp -R "$DL" "$TMP/dl-nosig" && rm -rf "$TMP/dl-nosig/signed-linux"
unpack refused "all, no signatures artifact" 'no such file' "$TMP/dl-nosig" all LINUX_SOURCES_SHA="$SOURCES_SHA" LINUX_SIGNED_SHA="$SIGNED_SHA"
cp -R "$DL" "$TMP/dl-stray" && echo x >"$TMP/dl-stray/desktop-linux-sources/stray"
unpack refused "all, a stray file beside the sources tar" 'holds other than' "$TMP/dl-stray" all LINUX_SOURCES_SHA="$SOURCES_SHA" LINUX_SIGNED_SHA="$SIGNED_SHA"
cp -R "$DL" "$TMP/dl-gz" && gzip -c <"$DL/desktop-linux-sources/Droppen-$V-linux-sources.tar" >"$TMP/dl-gz/desktop-linux-sources/Droppen-$V-linux-sources.tar"
unpack refused "all, a compressed sources tar" 'compressed' "$TMP/dl-gz" all LINUX_SOURCES_SHA="$(sha "$TMP/dl-gz/desktop-linux-sources/Droppen-$V-linux-sources.tar")" LINUX_SIGNED_SHA="$SIGNED_SHA"

echo "release-linux: $PASSED passed, $FAILED failed"
[ "$FAILED" = 0 ]
