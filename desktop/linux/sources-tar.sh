#!/usr/bin/env bash
# Droppen-<version>-linux-sources.tar: the source code of what the Linux
# AppImages carry besides Droppen itself, which a release publishes beside
# them (desktop/linux/README.AppImage.in points to it). It merges the two
# architectures' sources, which desktop/build-linux.sh fetched in each build's
# own image right after the build (sources-<arch>/), and adds the AppImage
# runtime's, fetched here and checked against the pins below.
#
#   desktop/linux/sources-tar.sh <version> <out-dir> <sources-x86_64> <sources-aarch64>
#
# In the tar (uncompressed: what it holds is compressed already), under
# Droppen-<version>-linux-sources/:
#   README                 what it holds, and where each part comes from
#   LIBRARIES-<arch>.txt   both architectures': every file each AppImage takes
#                          from an Ubuntu package, with the package's version
#                          and source package
#   ubuntu/                every source package those name, in the version
#                          they name: its .dsc and the files it names; and
#                          SOURCES, each source package with its version
#   copyright/<package>    each of those packages' copyright file, as the
#                          AppImages carry it
#   common-licenses/       Ubuntu's /usr/share/common-licenses, which the
#                          copyright files name
#   appimage-runtime/      type2-runtime-<release>.tar.gz (`git archive` of the
#                          release's commit), and the libfuse and squashfuse
#                          releases its build compiles in
#
# Checks: a file both architectures have is the same file; each source package
# is there with every file its .dsc names, by sha256; the runtime is the
# release .github/release-tools.lock pins, its tag is the commit pinned here,
# and its own build script takes the very libfuse and squashfuse files pinned
# here. Needs git, curl, gzip, sha256sum and GNU tar ($TAR, `gtar` on a Mac).
#
# Environment: SOURCE_DATE_EPOCH, every member's time (default: the commit's).
set -euo pipefail

# The source of the AppImage runtime (appimage-runtime in
# .github/release-tools.lock): its release tag's commit, and the two files its
# scripts/common/install-dependencies.sh downloads and checks by these sha256s.
# A runtime bump needs new ones.
RUNTIME_RELEASE=20251108
RUNTIME_COMMIT=dd6cebedcbddde9c82f89b011e8e1d40b6e43868
RUNTIME_REPO=https://github.com/AppImage/type2-runtime
FUSE_FILE=fuse-3.15.0.tar.xz
FUSE_URL=https://github.com/libfuse/libfuse/releases/download/fuse-3.15.0/fuse-3.15.0.tar.xz
FUSE_SHA256=70589cfd5e1cff7ccd6ac91c86c01be340b227285c5e200baa284e401eea2ca0
SQUASHFUSE_FILE=0.5.2.tar.gz
SQUASHFUSE_URL=https://github.com/vasi/squashfuse/archive/0.5.2.tar.gz
SQUASHFUSE_SHA256=db0238c5981dabbd80ee09ae15387f390091668ca060a7bc38047912491443d3

USAGE='usage: desktop/linux/sources-tar.sh <version> <out-dir> <sources-x86_64> <sources-aarch64>'
VERSION=${1:?$USAGE}
OUTDIR=${2:?$USAGE}
FROM_X86_64=${3:?$USAGE}
FROM_AARCH64=${4:?$USAGE}
die() { printf 'sources-tar: %s\n' "$*" >&2; exit 1; }
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "$VERSION is not a version"
REPO=$(cd "$(dirname "$0")/../.." && pwd -P)
TAR=${TAR:-tar}
"$TAR" --version 2>/dev/null | grep '^tar (GNU tar)' >/dev/null || die "needs GNU tar (set TAR, e.g. TAR=gtar)"
for tool in git curl gzip; do command -v "$tool" >/dev/null || die "missing $tool"; done
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$REPO" log -1 --format=%ct)}
LOCKED=$(awk '!/^#/ && $1 == "appimage-runtime" { print $2 }' "$REPO/.github/release-tools.lock" | sort -u)
[ "$LOCKED" = "$RUNTIME_RELEASE" ] \
  || die ".github/release-tools.lock pins appimage-runtime $LOCKED, but this script has the source of $RUNTIME_RELEASE: pin that release's commit and files here"
TARBALL=$OUTDIR/Droppen-$VERSION-linux-sources.tar
[ ! -e "$TARBALL" ] || die "$TARBALL is there already"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/droppen-sources.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
STAGE=$WORK/Droppen-$VERSION-linux-sources
mkdir -p "$STAGE/ubuntu" "$STAGE/copyright" "$STAGE/appimage-runtime"

# The two architectures', merged: a name both have is one file.
merge() { # from directory, into directory
  local file
  for file in "$1"/*; do
    if [ -e "$2/${file##*/}" ]; then
      cmp -s "$file" "$2/${file##*/}" || die "${file##*/} differs between the architectures"
    else
      cp "$file" "$2/"
    fi
  done
}
for arch in x86_64 aarch64; do
  if [ "$arch" = x86_64 ]; then from=$FROM_X86_64; else from=$FROM_AARCH64; fi
  for part in SOURCES "LIBRARIES-$arch.txt" ubuntu copyright common-licenses; do
    [ -e "$from/$part" ] || die "$from has no $part (desktop/build-linux.sh's sources-$arch/)"
  done
  head -n 1 "$from/LIBRARIES-$arch.txt" | grep -F "LIBRARIES of Droppen-$VERSION-$arch.AppImage:" >/dev/null \
    || die "$from/LIBRARIES-$arch.txt is not Droppen-$VERSION-$arch.AppImage's"
  cp "$from/LIBRARIES-$arch.txt" "$STAGE/"
  merge "$from/ubuntu" "$STAGE/ubuntu"
  merge "$from/copyright" "$STAGE/copyright"
  if [ -e "$STAGE/common-licenses" ]; then
    diff -r "$from/common-licenses" "$STAGE/common-licenses" >/dev/null || die "common-licenses differ between the architectures"
  else
    cp -R "$from/common-licenses" "$STAGE/common-licenses"
  fi
done
LC_ALL=C sort -u "$FROM_X86_64/SOURCES" "$FROM_AARCH64/SOURCES" >"$STAGE/ubuntu/SOURCES"
# What the two architectures' source packages have in common, and not.
ONLY_X86_64=$(LC_ALL=C comm -23 <(LC_ALL=C sort -u "$FROM_X86_64/SOURCES") <(LC_ALL=C sort -u "$FROM_AARCH64/SOURCES") | tr '\t' ' ' | paste -sd, -)
ONLY_AARCH64=$(LC_ALL=C comm -13 <(LC_ALL=C sort -u "$FROM_X86_64/SOURCES") <(LC_ALL=C sort -u "$FROM_AARCH64/SOURCES") | tr '\t' ' ' | paste -sd, -)
for arch in x86_64 aarch64; do
  if [ "$arch" = x86_64 ]; then from=$FROM_X86_64; else from=$FROM_AARCH64; fi
  echo "ok: $arch: $(wc -l <"$from/SOURCES" | tr -d ' ') source packages, $(du -sk "$from/ubuntu" | cut -f1) KiB"
done
echo "ok: only x86_64's: ${ONLY_X86_64:-none}; only aarch64's: ${ONLY_AARCH64:-none}"

# Every source package, whole: its .dsc and each file it names, by sha256,
# and nothing else.
: >"$WORK/names"
while IFS=$'\t' read -r src ver; do
  dsc=${src}_${ver#*:}.dsc
  [ -f "$STAGE/ubuntu/$dsc" ] || die "no $dsc for $src $ver"
  files=$(awk '/^Checksums-Sha256:/ { f = 1; next } /^[^ ]/ { f = 0 } f && NF == 3 { print $1 "  " $3 }' "$STAGE/ubuntu/$dsc")
  [ -n "$files" ] || die "$dsc names no files"
  while read -r sum name; do
    [ -f "$STAGE/ubuntu/$name" ] && [ "$(sha256 "$STAGE/ubuntu/$name")" = "$sum" ] || die "$name is not the file $dsc names"
    echo "$name" >>"$WORK/names"
  done <<<"$files"
  echo "$dsc" >>"$WORK/names"
done <"$STAGE/ubuntu/SOURCES"
echo SOURCES >>"$WORK/names"
STRAY=$(cd "$STAGE/ubuntu" && find . -mindepth 1 | sed 's|^\./||' | LC_ALL=C sort | comm -23 - <(LC_ALL=C sort -u "$WORK/names"))
[ -z "$STRAY" ] || die "in ubuntu/, files of no source package: $STRAY"
echo "ok: $(wc -l <"$STAGE/ubuntu/SOURCES" | tr -d ' ') source packages of both architectures, whole"

# The runtime's source: the commit of its release tag, then the two files its
# build script fetches, which must be the ones pinned here.
TAGGED=$(git ls-remote "$RUNTIME_REPO" "refs/tags/$RUNTIME_RELEASE" "refs/tags/$RUNTIME_RELEASE^{}" | awk 'END { print $1 }')
[ "$TAGGED" = "$RUNTIME_COMMIT" ] || die "type2-runtime's tag $RUNTIME_RELEASE is $TAGGED, not the pinned $RUNTIME_COMMIT"
git -C "$WORK" init -q runtime
git -C "$WORK/runtime" fetch -q --depth 1 "$RUNTIME_REPO" "$RUNTIME_COMMIT"
[ "$(git -C "$WORK/runtime" rev-parse FETCH_HEAD)" = "$RUNTIME_COMMIT" ] || die "fetched no $RUNTIME_COMMIT"
DEPS=$(git -C "$WORK/runtime" show FETCH_HEAD:scripts/common/install-dependencies.sh)
grep -F "$FUSE_SHA256  $FUSE_FILE" <<<"$DEPS" >/dev/null && grep -F "$FUSE_URL" <<<"$DEPS" >/dev/null \
  || die "type2-runtime $RUNTIME_RELEASE's build does not take $FUSE_FILE $FUSE_SHA256 from $FUSE_URL"
grep -F "$SQUASHFUSE_SHA256  $SQUASHFUSE_FILE" <<<"$DEPS" >/dev/null && grep -F "$SQUASHFUSE_URL" <<<"$DEPS" >/dev/null \
  || die "type2-runtime $RUNTIME_RELEASE's build does not take $SQUASHFUSE_FILE $SQUASHFUSE_SHA256 from $SQUASHFUSE_URL"
git -C "$WORK/runtime" archive --format=tar --prefix="type2-runtime-$RUNTIME_RELEASE/" FETCH_HEAD \
  | gzip -n -9 >"$STAGE/appimage-runtime/type2-runtime-$RUNTIME_RELEASE.tar.gz"
fetch() { # url, sha256, name here
  curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$STAGE/appimage-runtime/$3" "$1" || die "cannot download $1"
  [ "$(sha256 "$STAGE/appimage-runtime/$3")" = "$2" ] || die "$1 is not the pinned $2"
}
fetch "$FUSE_URL" "$FUSE_SHA256" "$FUSE_FILE"
fetch "$SQUASHFUSE_URL" "$SQUASHFUSE_SHA256" squashfuse-0.5.2.tar.gz
echo "ok: type2-runtime $RUNTIME_RELEASE ($RUNTIME_COMMIT), $FUSE_FILE and squashfuse 0.5.2, as its build takes them"

cat >"$STAGE/README" <<EOF
The source code of what the Linux AppImages of Droppen $VERSION carry besides
Droppen itself (Droppen's own is at https://github.com/getdroppen/droppen, tag
v$VERSION). Each AppImage's usr/share/doc/droppen/README.AppImage lists what
it carries and under which terms.

LIBRARIES-x86_64.txt, LIBRARIES-aarch64.txt
  Every file each AppImage takes from an Ubuntu 22.04 package: the package,
  its version, its source package and that one's version.

ubuntu/
  The Ubuntu source package of every package those name, in the version they
  name, as Ubuntu's archive has it: the .dsc and the files it names, which
  dpkg-source -x <name>.dsc unpacks. SOURCES lists them, with their versions.

copyright/<package>, common-licenses/
  Each of those packages' copyright file, and the licence texts they name
  (Ubuntu's /usr/share/common-licenses), as the AppImages carry them.

appimage-runtime/
  The AppImage runtime, the program at the head of each AppImage:
  type2-runtime-$RUNTIME_RELEASE.tar.gz, the source of its release $RUNTIME_RELEASE
  (commit $RUNTIME_COMMIT of $RUNTIME_REPO), with
  $FUSE_FILE and squashfuse-0.5.2.tar.gz (the squashfuse 0.5.2 release), the
  libfuse and squashfuse releases its build compiles in; its build is
  scripts/common/install-dependencies.sh and scripts/build-runtime.sh there.
  musl libc, libzstd, zlib and mimalloc come from Alpine Linux 3.21's
  packages, under the permissive terms README.AppImage gives.
EOF

mkdir -p "$OUTDIR"
"$TAR" --sort=name --format=gnu --owner=0 --group=0 --numeric-owner --mtime="@$SOURCE_DATE_EPOCH" \
  --mode='a+rX,u+w,go-w' -cf "$TARBALL" -C "$WORK" "Droppen-$VERSION-linux-sources"
echo "ok: $TARBALL, $(du -h "$TARBALL" | cut -f1), $("$TAR" -tf "$TARBALL" | grep -vc '/$') files; sha256 $(sha256 "$TARBALL")"
