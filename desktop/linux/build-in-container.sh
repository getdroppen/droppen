#!/usr/bin/env bash
# The Linux app's build itself, in desktop/linux/Dockerfile's image, as
# desktop/build-linux.sh runs it (never on its own):
#
#   build-in-container.sh prepare   with the network: the locked crates, into
#                                   /work/cargo, and the versions of the
#                                   sidecar and of tauri-cli
#   build-in-container.sh build     without it (`docker run --network none`):
#                                   the .deb and the AppImage, their checks,
#                                   and LIBRARIES-<arch>.txt
#
# /work/src    the ref's source, with the sidecar in
#              desktop/src-tauri/binaries/droppen-server-<target> and the
#              legal files in desktop/src-tauri/legal/ (LICENSE,
#              THIRD_PARTY_NOTICES, README.AppImage)
# /work/tools  fetch-release-tool.sh's bin/ for this architecture:
#              cargo-tauri, linuxdeploy-<version>-<arch>.AppImage,
#              AppRun-<arch>, runtime-<arch>
# /work/out    receives Droppen-<version>-<arch>.AppImage,
#              droppen_<version>_<debarch>.deb and LIBRARIES-<arch>.txt
#
# Environment: VERSION (the app's), SOURCE_DATE_EPOCH (the commit's time).
set -euo pipefail

MODE=${1:?usage: build-in-container.sh prepare|build}
: "${VERSION:?}" "${SOURCE_DATE_EPOCH:?}"
die() { printf 'build-in-container: %s\n' "$*" >&2; exit 1; }
say() { printf '\n== %s\n' "$*"; }

ARCH=$(uname -m)
case "$ARCH" in
  x86_64) DEBARCH=amd64; TAURI_AI_ARCH=amd64 ;;
  aarch64) DEBARCH=arm64; TAURI_AI_ARCH=aarch64 ;;
  *) die "no Linux app for $ARCH" ;;
esac
TARGET=$ARCH-unknown-linux-gnu
SRC=/work/src
TAURI_DIR=$SRC/desktop/src-tauri
SIDECAR=$TAURI_DIR/binaries/droppen-server-$TARGET
TOOLS=/work/tools
export CARGO_HOME=/work/cargo

if [ "$MODE" = prepare ]; then
  cargo fetch --locked --manifest-path "$SRC/desktop/Cargo.toml" >&2
  printf 'sidecar=%s\n' "$("$SIDECAR" --version)"
  printf 'tauri_cli=%s\n' "$("$TOOLS/cargo-tauri" --version)"
  printf 'rustc=%s\n' "$(rustc --version)"
  exit 0
fi
[ "$MODE" = build ] || die "unknown mode $MODE"

# The bundler's tools, where it looks for them before it would download them
# ($XDG_CACHE_HOME/tauri), and the AppImage runtime appimagetool takes
# instead of downloading `continuous` (linuxdeploy's appimage plugin passes
# LDAI_RUNTIME_FILE on as --runtime-file). Fresh copies: the bundler blanks
# linuxdeploy's AppImage magic bytes in place.
export XDG_CACHE_HOME=/work/cache
rm -rf "$XDG_CACHE_HOME" && mkdir -p "$XDG_CACHE_HOME/tauri"
LINUXDEPLOY=$(cd "$TOOLS" && ls linuxdeploy-*-"$ARCH".AppImage)
cp "$TOOLS/$LINUXDEPLOY" "$TOOLS/AppRun-$ARCH" "$XDG_CACHE_HOME/tauri/"
export LDAI_RUNTIME_FILE=$TOOLS/runtime-$ARCH
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/work/target CARGO_BUILD_BUILD_DIR=/work/build

CHECK=/work/check
BUNDLE=$CARGO_TARGET_DIR/$TARGET/release/bundle
DEB=$BUNDLE/deb/Droppen_${VERSION}_$DEBARCH.deb
APPIMAGE=$BUNDLE/appimage/Droppen_${VERSION}_$TAURI_AI_ARCH.AppImage
LEGAL=$TAURI_DIR/legal
rm -rf "$CHECK" && mkdir -p "$CHECK"

# `cargo tauri build` of <bundles>. The licence and the notices go in
# /usr/lib/Droppen (resources), as on macOS, and the .deb's copyright file is
# the licence. The AppImage also carries README.AppImage, the licence texts of
# /usr/share/common-licenses, which its libraries' copyright files name, and
# those copyright files: linuxdeploy copies the ones of the libraries it
# deploys itself, and <extra> (more `"<in the AppImage>": "<here>"` members of
# its files) the others.
bundle() { # bundles, [extra]
  local config
  config=$(cat <<JSON
{"version": "$VERSION",
 "bundle": {"resources": {"legal/LICENSE": "LICENSE", "legal/THIRD_PARTY_NOTICES": "THIRD_PARTY_NOTICES"},
            "linux": {"deb": {"files": {"/usr/share/doc/droppen/copyright": "legal/LICENSE"}},
                      "appimage": {"files": {"/usr/share/doc/droppen/README.AppImage": "legal/README.AppImage",
                                             "/usr/share/common-licenses": "/usr/share/common-licenses"${2:-}}}}}}
JSON
)
  (
    cd "$TAURI_DIR"
    RUSTFLAGS="--remap-path-prefix=$SRC=/droppen --remap-path-prefix=$CARGO_HOME=/cargo" \
      "$TOOLS/cargo-tauri" build --ci --target "$TARGET" --bundles "$1" --config "$config" -- --locked
  )
  # Nothing was downloaded: the bundler's tools directory holds what was put
  # there and the plugin scripts it writes. (Without the network, its try at
  # linuxdeploy-plugin-appimage's `continuous` fails, and linuxdeploy uses the
  # appimage plugin it carries.)
  local extra="" file
  for file in "$XDG_CACHE_HOME"/tauri/*; do
    case "${file##*/}" in
      "$LINUXDEPLOY" | "AppRun-$ARCH" | linuxdeploy-plugin-gtk.sh | linuxdeploy-plugin-gstreamer.sh) ;;
      *) extra="$extra ${file##*/}" ;;
    esac
  done
  [ -z "$extra" ] || die "the bundler fetched tools of its own:$extra"
}

# Every file installed here, with its package, under its merged-/usr path.
INDEX=$CHECK/dpkg-index
for list in /var/lib/dpkg/info/*.list; do
  sed -E 's#^/(lib|lib64|bin|sbin)/#/usr/\1/#' "$list" | awk -v pkg="$(basename "$list" .list)" '{ print $0 "\t" pkg }'
done >"$INDEX"
# Where a file of the AppImage comes from, as "<package>\t<its file here>"
# lines: the file at the same path here, or else, by its name, the files under
# /usr/lib (linuxdeploy puts the libraries in usr/lib, whatever their
# directory here); each through its links, to the package that installed
# what it resolves to.
origins_of() { # file in the AppImage
  local file=$1 candidate path
  if [ -e "/$file" ]; then
    echo "/$file"
  else
    awk -F'\t' -v name="${file##*/}" '$1 ~ /^\/usr\/lib\// { n = split($1, part, "/"); if (part[n] == name) print $1 }' "$INDEX"
  fi | sort -u | while IFS= read -r candidate; do
    path=$(readlink -f "$candidate")
    awk -F'\t' -v path="$path" '$1 == path { print $2 "\t" path }' "$INDEX"
  done | sort -u
}

# Unpack the AppImage, by its own runtime (no FUSE needed), into $ROOT, and
# account for each of its files: the app's own, what the bundler and
# linuxdeploy write, the licence texts and copyright files, or one installed
# package's ($TRACED: package, file): a data file byte for byte, a library but
# for the run path linuxdeploy sets in it. $UNTRACED: what is none of these,
# and why. $MISSING: the packages with no copyright file in it.
inspect_appimage() {
  rm -rf "$CHECK/appimage" && mkdir -p "$CHECK/appimage"
  (cd "$CHECK/appimage" && "$APPIMAGE" --appimage-extract >/dev/null)
  ROOT=$CHECK/appimage/squashfs-root
  [ -d "$ROOT" ] || die "the AppImage did not unpack"
  TRACED=$CHECK/traced UNTRACED=$CHECK/untraced
  : >"$TRACED"
  : >"$UNTRACED"
  local file origins pkgs
  while IFS= read -r file; do
    case "$file" in
      usr/bin/Droppen | usr/bin/droppen-server | usr/lib/Droppen/* | usr/share/doc/droppen/* \
        | usr/share/applications/Droppen.desktop | usr/share/icons/hicolor/*/apps/Droppen.png | Droppen.desktop \
        | AppRun | AppRun.wrapped | apprun-hooks/linuxdeploy-plugin-gtk.sh \
        | usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache | usr/lib/gio/modules/giomodule.cache \
        | usr/lib/gtk-3.0/3.0.0/immodules.cache | usr/share/glib-2.0/schemas/gschemas.compiled \
        | usr/share/common-licenses/* | usr/share/doc/*/copyright)
        continue ;;
    esac
    origins=$(origins_of "$file")
    pkgs=$(printf '%s\n' "$origins" | cut -f1 | grep . | sort -u || true)
    case "$(printf '%s' "$pkgs" | grep -c .)" in
      0) printf '%s\tthe app\x27s, no tool\x27s and no installed package\x27s\n' "$file" >>"$UNTRACED" ;;
      1)
        if head -c 4 "$ROOT/$file" | grep -q ELF \
          || printf '%s\n' "$origins" | cut -f2 | while IFS= read -r path; do cmp -s "$ROOT/$file" "$path" && echo same; done | grep same >/dev/null; then
          printf '%s\t%s\n' "$pkgs" "$file" >>"$TRACED"
        else
          printf '%s\tnot %s\x27s file, byte for byte\n' "$file" "$pkgs" >>"$UNTRACED"
        fi
        ;;
      *) printf '%s\tthe file of several packages: %s\n' "$file" "$(echo $pkgs)" >>"$UNTRACED" ;;
    esac
  done < <(cd "$ROOT" && find . -type f | sed 's|^\./||' | LC_ALL=C sort)
  MISSING=$(cut -f1 "$TRACED" | sed 's/:.*//' | sort -u | while IFS= read -r pkg; do
    [ -f "$ROOT/usr/share/doc/$pkg/copyright" ] || echo "$pkg"
  done)
}

say "app: $TARGET, the .deb and the AppImage"
bundle deb,appimage
[ -f "$DEB" ] || die "no $DEB"
[ -f "$APPIMAGE" ] || die "no $APPIMAGE"
inspect_appimage
if [ -n "$MISSING" ]; then
  say "the AppImage again, with the copyright files linuxdeploy left out: $(echo $MISSING)"
  EXTRA=""
  for pkg in $MISSING; do
    [ -f "/usr/share/doc/$pkg/copyright" ] || die "$pkg is in the AppImage, but has no copyright file here"
    EXTRA="$EXTRA, \"/usr/share/doc/$pkg/copyright\": \"/usr/share/doc/$pkg/copyright\""
  done
  bundle appimage "$EXTRA"
  inspect_appimage
fi

fail=0
bad() { printf 'FAIL: %s\n' "$*" >&2; fail=1; }
same() { # file, expected, what
  if cmp -s "$1" "$2"; then echo "ok: $3"; else bad "$3: ${1#"$CHECK"/} is not $(basename "$2")"; fi
}
# The checks desktop/build.sh makes of the macOS app's program.
PUBKEY=$(sed -n 's/.*"pubkey": *"\([^"]*\)".*/\1/p' "$TAURI_DIR/tauri.conf.json")
[ -n "$PUBKEY" ] || die "cannot read the updater public key of tauri.conf.json"
check_program() { # program, where
  local needle
  for needle in DROPPEN_DESKTOP_TEST_CONTROL DROPPEN_DESKTOP_KEYCHAIN 'test control is on' DROPPEN_DESKTOP_UPDATE_ENDPOINT DROPPEN_DESKTOP_TEST_HUB app.droppen.desktop.e2e; do
    if LC_ALL=C grep -aqF -- "$needle" "$1"; then bad "$2: a release build with the end-to-end build's $needle in it"; fi
  done
  LC_ALL=C grep -aqF -- "$PUBKEY" "$1" || bad "$2: does not carry the updater public key of tauri.conf.json"
  for needle in /work/ /root/; do
    if LC_ALL=C grep -aqF -- "$needle" "$1"; then bad "$2: names the builder's paths ($needle)"; fi
  done
}

say "checks: the .deb"
mkdir -p "$CHECK/deb"
dpkg-deb -x "$DEB" "$CHECK/deb"
field() { dpkg-deb -f "$DEB" "$1"; }
[ "$(field Package)" = droppen ] || bad "the .deb's package is $(field Package), not droppen"
[ "$(field Version)" = "$VERSION" ] || bad "the .deb's version is $(field Version), not $VERSION"
[ "$(field Architecture)" = "$DEBARCH" ] || bad "the .deb's architecture is $(field Architecture), not $DEBARCH"
for dep in libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1; do
  case ", $(field Depends), " in *", $dep, "*) ;; *) bad "the .deb does not depend on $dep" ;; esac
done
STRAY=$(cd "$CHECK/deb" && find . -mindepth 1 -maxdepth 1 ! -name usr)
[ -z "$STRAY" ] || bad "the .deb installs outside /usr: $STRAY"
check_program "$CHECK/deb/usr/bin/Droppen" "the .deb's Droppen"
same "$CHECK/deb/usr/bin/droppen-server" "$SIDECAR" "the .deb's droppen-server is the sidecar, byte for byte"
same "$CHECK/deb/usr/lib/Droppen/LICENSE" "$LEGAL/LICENSE" "the .deb's LICENSE"
same "$CHECK/deb/usr/lib/Droppen/THIRD_PARTY_NOTICES" "$LEGAL/THIRD_PARTY_NOTICES" "the .deb's THIRD_PARTY_NOTICES"
same "$CHECK/deb/usr/share/doc/droppen/copyright" "$LEGAL/LICENSE" "the .deb's copyright file is the licence"
[ -f "$CHECK/deb/usr/share/applications/Droppen.desktop" ] || bad "the .deb has no desktop entry"

say "checks: the AppImage"
# The program at its head is the pinned runtime: of its size and build (its
# bytes differ where appimagetool writes the AppImage's digest and update
# information into it).
RUNTIME_SIZE=$(stat -c %s "$LDAI_RUNTIME_FILE")
[ "$("$APPIMAGE" --appimage-offset)" = "$RUNTIME_SIZE" ] || bad "the AppImage's runtime is not runtime-$ARCH's size"
[ "$("$APPIMAGE" --appimage-version 2>&1)" = "$("$LDAI_RUNTIME_FILE" --appimage-version 2>&1)" ] \
  || bad "the AppImage's runtime is not runtime-$ARCH's build"
same "$ROOT/AppRun.wrapped" "$TOOLS/AppRun-$ARCH" "the AppImage's AppRun is AppRun-$ARCH"
check_program "$ROOT/usr/bin/Droppen" "the AppImage's Droppen"
# linuxdeploy sets the sidecar's run path ($ORIGIN/../lib): the same server,
# not the same bytes.
[ "$("$ROOT/usr/bin/droppen-server" --version)" = "$("$SIDECAR" --version)" ] || bad "the AppImage's droppen-server is not the sidecar"
same "$ROOT/usr/lib/Droppen/LICENSE" "$LEGAL/LICENSE" "the AppImage's LICENSE"
same "$ROOT/usr/lib/Droppen/THIRD_PARTY_NOTICES" "$LEGAL/THIRD_PARTY_NOTICES" "the AppImage's THIRD_PARTY_NOTICES"
same "$ROOT/usr/share/doc/droppen/README.AppImage" "$LEGAL/README.AppImage" "the AppImage's README.AppImage"
diff -r /usr/share/common-licenses "$ROOT/usr/share/common-licenses" >/dev/null && echo "ok: the AppImage's common-licenses are Ubuntu's" \
  || bad "the AppImage's usr/share/common-licenses are not this system's"
# The glibc it needs is the image's at most: the image's is the floor.
GLIBC=$(find "$ROOT" -type f -exec sh -c 'head -c 4 "$1" | grep -q ELF && objdump -T "$1" 2>/dev/null' _ {} \; \
  | grep -oE 'GLIBC_2\.[0-9]+' | sort -t. -k2,2n | tail -n 1)
HOST_GLIBC=GLIBC_$(ldd --version | head -n 1 | awk '{ print $NF }')
[ "$(printf '%s\n%s\n' "$GLIBC" "$HOST_GLIBC" | sort -t. -k2,2n | tail -n 1)" = "$HOST_GLIBC" ] \
  || bad "the AppImage needs $GLIBC, more than the image's $HOST_GLIBC"
echo "ok: the AppImage needs $GLIBC at most"
while IFS=$'\t' read -r file why; do bad "$file is in the AppImage, but is $why"; done <"$UNTRACED"
for pkg in $MISSING; do bad "the AppImage has no copyright file for $pkg"; done
# Each copyright file is its package's.
while IFS= read -r file; do
  cmp -s "$ROOT/$file" "/$file" || bad "$file is not this system's"
done < <(cd "$ROOT" && find usr/share/doc -mindepth 2 -maxdepth 2 -name copyright ! -path 'usr/share/doc/droppen/*' | LC_ALL=C sort)

say "LIBRARIES-$ARCH.txt"
LIBRARIES=$CHECK/LIBRARIES-$ARCH.txt
# The apt state: the newest Release file of the archive in the image.
LISTS=$(for release in /var/lib/apt/lists/*_InRelease; do date -u -d "$(sed -n 's/^Date: //p' "$release")" +%s; done | sort -n | tail -n 1)
{
  printf '# LIBRARIES of Droppen-%s-%s.AppImage: each of its files that comes from an\n' "$VERSION" "$ARCH"
  printf '# Ubuntu 22.04 package, with the package, its version and its source package,\n'
  printf '# as installed in the image it was built in (the archive as of %s).\n' "$(date -u -d "@$LISTS" '+%Y-%m-%d %H:%M UTC')"
  printf "# Each package's copyright file is in the AppImage, in\n"
  printf '# usr/share/doc/<package>/copyright, with the licence texts those name in\n'
  printf '# usr/share/common-licenses. The source of every one of these packages, in\n'
  printf '# these versions: Droppen-%s-linux-sources.tar, on the same release.\n#\n' "$VERSION"
  printf '# package\tversion\tsource\tsource version\tfile\n'
  cut -f1 "$TRACED" | sort -u | while IFS= read -r pkg; do
    meta=$(dpkg-query -W -f='${binary:Package}\t${Version}\t${source:Package}\t${source:Version}' "$pkg") || die "dpkg knows no $pkg"
    awk -F'\t' -v pkg="$pkg" -v meta="$meta" '$1 == pkg { print meta "\t" $2 }' "$TRACED"
  done | LC_ALL=C sort -t "$(printf '\t')" -k5,5
} >"$LIBRARIES"
echo "ok: $(grep -vc '^#' "$LIBRARIES") files from $(grep -v '^#' "$LIBRARIES" | cut -f1 | sort -u | wc -l) packages, built from $(grep -v '^#' "$LIBRARIES" | cut -f3,4 | sort -u | wc -l) source packages"

[ "$fail" = 0 ] || die "the bundles failed their checks (above)"
mkdir -p /work/out
cp "$APPIMAGE" "/work/out/Droppen-$VERSION-$ARCH.AppImage"
cp "$DEB" "/work/out/droppen_${VERSION}_$DEBARCH.deb"
cp "$LIBRARIES" /work/out/
(cd /work/out && ls -l && sha256sum ./*)
