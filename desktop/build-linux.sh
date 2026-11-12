#!/usr/bin/env bash
# Build the Droppen desktop app for Linux, for one architecture: its AppImage
# (the build that updates itself) and its .deb, with the Linux server inside
# as the sidecar, and LIBRARIES-<arch>.txt, what the AppImage carries of
# Ubuntu's packages. Unsigned: the release signs the AppImage's update
# (.github/workflows/release.yml). Nothing is published.
#
#   desktop/build-linux.sh --arch x86_64|aarch64 --sidecar <droppen-server>
#                          [--ref <git-ref> | --worktree] [--notices <file>]
#                          [--tools <dir>] [--out <dir>] [--keep-work] [--keep-image]
#
#   --arch      the architecture, which Docker runs the build on (the machine's
#               own; the other one emulated, and slowly)
#   --sidecar   the Linux server of the same ref (scripts/build-linux.sh's, or
#               the droppen of release-public.sh's tarball): its --version must
#               be <version>+g<rev> of the ref
#   --ref       what to build (default HEAD), exported with `git archive`
#   --worktree  this checkout as it is, uncommitted files included (never for
#               a release)
#   --notices   THIRD_PARTY_NOTICES as `desktop/third-party-notices.sh
#               --platform linux` wrote it for this version elsewhere, instead
#               of making it here (which needs Node, pnpm and cargo-about)
#   --tools     .github/scripts/fetch-release-tool.sh's <dest> with this
#               architecture's tauri-cli, linuxdeploy, apprun and
#               appimage-runtime, instead of fetching them here
#   --out       where the files go (default /tmp/droppen-desktop-linux-release);
#               must be empty or absent
#   --keep-work keep the build directory
#   --keep-image  keep the build image (droppen-desktop-linux-build:<arch>-<rev>)
#
# How, in three runs of desktop/linux/Dockerfile's image (Ubuntu 22.04 by
# digest, its WebKitGTK stack from the live jammy archive, and Rust), built
# afresh every time, with no cache:
#   1. with the network: the locked crates, and the sidecar's and tauri-cli's
#      versions (desktop/linux/build-in-container.sh prepare);
#   2. without it (--network none): `cargo tauri build --bundles
#      deb,appimage`, with the AppImage tools of .github/release-tools.lock
#      where the bundler looks for them, so nothing it runs or ships is
#      downloaded; then every check, and LIBRARIES-<arch>.txt
#      (build-in-container.sh build);
# all as the calling user, in a scratch directory under /tmp.
#
# What the bundles carry, and what is checked (build-in-container.sh):
#   both     Droppen (no test hooks, no end-to-end identity, the updater key of
#            tauri.conf.json, no builder's paths), the sidecar, and in
#            /usr/lib/Droppen the licence (AGPL-3.0-only) and
#            THIRD_PARTY_NOTICES, which pass the release text check
#            (scripts/release-text-check.sh) first
#   .deb     droppen_<version>_<amd64|arm64>.deb: the sidecar byte for byte,
#            /usr/share/doc/droppen/copyright (the licence), and the
#            dependencies on Ubuntu's WebKitGTK, GTK and appindicator
#   AppImage Droppen-<version>-<arch>.AppImage: the pinned runtime and AppRun,
#            the glibc of Ubuntu 22.04 at most, every library's copyright file
#            and the licence texts they name, README.AppImage
#            (desktop/linux/README.AppImage.in: the runtime's and AppRun's
#            notices, and where the libraries' source is); every file of it is
#            the app's, a tool's or one installed package's, which
#            LIBRARIES-<arch>.txt lists with its version and source package
#
# Needs: git, Docker, curl, and without --notices Node (.nvmrc) and cargo-about
# (scripts/release-public.sh's ABOUT_VERSION).
#
# Environment:
#   DROPPEN_DESKTOP_LINUX_WORK  the scratch directory (/tmp/droppen-desktop-linux-build/<arch>)
#   DROPPEN_MIN_FREE_GB         refuse to start below this much free disk (25)
#   RUST_TOOLCHAIN              must be the Dockerfile's when set (release.yml's)
#   DROPPEN_RELEASE_FORBIDDEN, DROPPEN_RELEASE_NOTICE_ALLOW,
#   DROPPEN_RELEASE_NAMES_DEFERRED  the release text check's, as desktop/build.sh
set -euo pipefail

RELEASE_DIE_NAME='desktop/build-linux.sh'
# shellcheck source=../scripts/release-lib.sh
. "$(cd "$(dirname "$0")" && pwd -P)/../scripts/release-lib.sh"

usage() { release_usage '/^set -euo/'; }

REF=""
WORKTREE=0
ARCH=""
SIDECAR=""
NOTICES_FROM=""
TOOLS_FROM=""
OUT=/tmp/droppen-desktop-linux-release
KEEP_WORK=0
KEEP_IMAGE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --ref) REF=${2:?--ref needs a value}; shift 2 ;;
    --worktree) WORKTREE=1; shift ;;
    --arch) ARCH=${2:?--arch needs a value}; shift 2 ;;
    --sidecar) SIDECAR=${2:?--sidecar needs a path}; shift 2 ;;
    --notices) NOTICES_FROM=${2:?--notices needs a file}; shift 2 ;;
    --tools) TOOLS_FROM=${2:?--tools needs a directory}; shift 2 ;;
    --out) OUT=${2:?--out needs a value}; shift 2 ;;
    --keep-work) KEEP_WORK=1; shift ;;
    --keep-image) KEEP_IMAGE=1; shift ;;
    -h|--help) usage ;;
    *) echo "unknown argument: $1" >&2; usage ;;
  esac
done
[ "$WORKTREE" = 0 ] || [ -z "$REF" ] || { echo "--ref and --worktree are exclusive" >&2; exit 2; }
case "$ARCH" in
  x86_64) PLATFORM=linux/amd64 ;;
  aarch64) PLATFORM=linux/arm64 ;;
  *) echo "--arch is x86_64 or aarch64" >&2; exit 2 ;;
esac
TARGET=$ARCH-unknown-linux-gnu
[ -n "$SIDECAR" ] || { echo "--sidecar is required: the Linux server of the same ref" >&2; exit 2; }

ABOUT_VERSION_PIN=""
MIN_FREE_GB=${DROPPEN_MIN_FREE_GB:-25}
WORK=${DROPPEN_DESKTOP_LINUX_WORK:-/tmp/droppen-desktop-linux-build/$ARCH}
for tool in git docker curl; do command -v "$tool" >/dev/null || die "missing $tool"; done
[ -f "$SIDECAR" ] || die "--sidecar: no such file"
SIDECAR=$(cd "$(dirname "$SIDECAR")" && pwd -P)/$(basename "$SIDECAR")
if [ -n "$NOTICES_FROM" ]; then
  [ -s "$NOTICES_FROM" ] || die "--notices: no such file"
  NOTICES_FROM=$(cd "$(dirname "$NOTICES_FROM")" && pwd -P)/$(basename "$NOTICES_FROM")
fi
if [ -n "$TOOLS_FROM" ]; then
  [ -d "$TOOLS_FROM/bin" ] || die "--tools: no bin/ in $TOOLS_FROM (.github/scripts/fetch-release-tool.sh's <dest>)"
  TOOLS_FROM=$(cd "$TOOLS_FROM" && pwd -P)
fi

REPO=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
# The release text check over the licence, the notices and README.AppImage,
# as desktop/build.sh does it.
. "$REPO/scripts/release-text-check.sh"
NAMES_FILE=$HOME/.config/droppen/release-forbidden.env
release_text_load_names "$NAMES_FILE"
if release_text_names_deferred; then
  echo "note: DROPPEN_RELEASE_NAMES_DEFERRED: the legal files are checked for \"Kilerd\" only here; the names are checked later, by whoever has them"
else
  release_text_names_ok \
    || die "DROPPEN_RELEASE_FORBIDDEN is not the expected list of names (set it, or keep it in $NAMES_FILE; its sha256 is RELEASE_TEXT_NAMES_SHA256 in scripts/release-text-check.sh)"
fi
release_text_allow_ok \
  || die "DROPPEN_RELEASE_NOTICE_ALLOW may only spell out whole third-party copyright lines, each ^Copyright …\$ with its metacharacters escaped"

if [ "$WORKTREE" = 1 ]; then
  SHA=$(git -C "$REPO" rev-parse HEAD)
else
  REF=${REF:-HEAD}
  SHA=$(git -C "$REPO" rev-parse --verify --quiet "$REF^{commit}") || die "cannot resolve $REF"
fi
REV=$(git -C "$REPO" rev-parse --short "$SHA")
COMMIT_TIME=$(git -C "$REPO" log -1 --format=%ct "$SHA")
if [ "$WORKTREE" = 1 ]; then
  VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$REPO/crates/droppen-server/Cargo.toml" | head -1)
else
  VERSION=$(git -C "$REPO" show "$SHA:crates/droppen-server/Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)
fi
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "cannot read the server version (got '$VERSION')"
if [[ "${REF:-}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] && [ "$REF" != "v$VERSION" ]; then
  die "$REF has the server version $VERSION in crates/droppen-server/Cargo.toml; the app would call itself $VERSION"
fi

case "$WORK" in /tmp/*|/private/tmp/*|"${TMPDIR:-/nonexistent}"*) ;; *) die "DROPPEN_DESKTOP_LINUX_WORK must be under /tmp" ;; esac
case "$WORK$OUT" in *" "*) die "no spaces in the work or output path, please" ;; esac
if [ -e "$OUT" ] && [ -n "$(ls -A "$OUT")" ]; then die "$OUT is not empty; move it aside or pass --out <new dir>"; fi
mkdir -p "$(dirname "$WORK")"
FREE=$(free_gb "$(dirname "$WORK")")
[ "$FREE" -ge $((MIN_FREE_GB + 8)) ] || die "only ${FREE} GB free; need at least $((MIN_FREE_GB + 8)) GB to stay above ${MIN_FREE_GB} GB"

IMAGE=droppen-desktop-linux-build:$ARCH-$REV
IMAGE_BUILT=0
finish() {
  local status=$?
  if [ "$IMAGE_BUILT" = 1 ] && [ "$KEEP_IMAGE" = 0 ]; then docker image rm "$IMAGE" >/dev/null 2>&1 || true; fi
  if [ "$KEEP_WORK" = 1 ]; then echo "kept the build directory: $WORK" >&2; else rm -rf "$WORK"; fi
  exit "$status"
}
trap finish EXIT
trap 'exit 130' INT TERM

say "Droppen $VERSION for Linux ($ARCH) from $SHA (rev $REV), ${FREE} GB free, work in $WORK"
rm -rf "$WORK"
mkdir -p "$WORK/src" "$WORK/cargo" "$WORK/home" "$WORK/tools" "$OUT"
if [ "$WORKTREE" = 1 ]; then
  (
    cd "$REPO"
    git ls-files -z --cached --others --exclude-standard | while IFS= read -r -d '' file; do
      [ -e "$file" ] && printf '%s\0' "$file"
    done | tar --null -T - -cf -
  ) | tar -x -C "$WORK/src"
else
  git -C "$REPO" archive --format=tar "$SHA" | tar -x -C "$WORK/src"
fi
SRC=$WORK/src
TAURI_DIR=$SRC/desktop/src-tauri
[ -f "$SRC/desktop/linux/Dockerfile" ] || die "$SHA has no desktop/linux/Dockerfile"

# The Rust of release.yml's builds is the image's.
DOCKER_RUST=$(sed -n 's/^ARG RUST_TOOLCHAIN=//p' "$SRC/desktop/linux/Dockerfile")
[ -z "${RUST_TOOLCHAIN:-}" ] || [ "$RUST_TOOLCHAIN" = "$DOCKER_RUST" ] \
  || die "RUST_TOOLCHAIN is $RUST_TOOLCHAIN, but desktop/linux/Dockerfile installs $DOCKER_RUST"

say "licence and third-party notices"
LEGAL=$TAURI_DIR/legal
mkdir -p "$LEGAL"
grep -q "GNU AFFERO GENERAL PUBLIC LICENSE" "$SRC/LICENSE" || die "LICENSE is not the AGPL-3.0 text"
cp "$SRC/LICENSE" "$LEGAL/LICENSE"
if [ -n "$NOTICES_FROM" ]; then
  cp "$NOTICES_FROM" "$LEGAL/THIRD_PARTY_NOTICES"
else
  ABOUT_VERSION_PIN=$(sed -n 's/^ABOUT_VERSION=//p' "$REPO/scripts/release-public.sh")
  [ "$(cargo about --version 2>/dev/null)" = "cargo-about $ABOUT_VERSION_PIN" ] \
    || die "needs cargo-about $ABOUT_VERSION_PIN for THIRD_PARTY_NOTICES (or --notices): cargo install --locked cargo-about@$ABOUT_VERSION_PIN --features cli"
  for tool in node npx; do command -v "$tool" >/dev/null || die "missing $tool, for THIRD_PARTY_NOTICES (or --notices)"; done
  pnpm_web_env "$SRC"
  "${web_env[@]}" pnpm -C "$SRC" install --frozen-lockfile --filter "@droppen/web..."
  "${web_env[@]}" "$REPO/desktop/third-party-notices.sh" --platform linux "$SRC" "$LEGAL/THIRD_PARTY_NOTICES" "$VERSION" "$VERSION" "$REV" \
    || die "THIRD_PARTY_NOTICES failed (see above)"
  rm -rf "$SRC/node_modules" "$SRC/web/node_modules"
fi
[ "$(head -n 1 "$LEGAL/THIRD_PARTY_NOTICES")" = "THIRD_PARTY_NOTICES for Droppen $VERSION for Linux (with droppen-server $VERSION+g$REV)" ] \
  || die "THIRD_PARTY_NOTICES is not the Linux app's of Droppen $VERSION (droppen-server $VERSION+g$REV): desktop/third-party-notices.sh --platform linux"
sed -e "s/@VERSION@/$VERSION/g" "$SRC/desktop/linux/README.AppImage.in" > "$LEGAL/README.AppImage"
for legal in LICENSE THIRD_PARTY_NOTICES README.AppImage; do
  hits=$(release_text_hits "$LEGAL/$legal" "$([ "$legal" = LICENSE ] || echo notices)")
  [ -z "$hits" ] || die "$legal names a person it should not:
$hits"
done
echo "ok: LICENSE (AGPL-3.0), THIRD_PARTY_NOTICES $(wc -l < "$LEGAL/THIRD_PARTY_NOTICES" | tr -d ' ') lines, README.AppImage; all pass the release text check"

say "tools (.github/release-tools.lock)"
if [ -n "$TOOLS_FROM" ]; then
  cp "$TOOLS_FROM"/bin/* "$WORK/tools/"
else
  for tool in tauri-cli linuxdeploy apprun appimage-runtime; do
    "$REPO/.github/scripts/fetch-release-tool.sh" "$tool" "$WORK/fetched" "$TARGET"
  done
  cp "$WORK/fetched"/bin/* "$WORK/tools/"
fi
for file in cargo-tauri "AppRun-$ARCH" "runtime-$ARCH"; do [ -f "$WORK/tools/$file" ] || die "no $file among the tools"; done
ls "$WORK/tools"/linuxdeploy-*-"$ARCH".AppImage >/dev/null 2>&1 || die "no linuxdeploy-<version>-$ARCH.AppImage among the tools"

say "sidecar: $SIDECAR"
cp "$SIDECAR" "$TAURI_DIR/binaries/droppen-server-$TARGET"
chmod 755 "$TAURI_DIR/binaries/droppen-server-$TARGET"
if LC_ALL=C grep -aqF "$HOME/" "$TAURI_DIR/binaries/droppen-server-$TARGET"; then die "$SIDECAR names paths under $HOME"; fi

say "image ($PLATFORM): desktop/linux/Dockerfile"
docker build --pull --no-cache --platform "$PLATFORM" -t "$IMAGE" -f "$SRC/desktop/linux/Dockerfile" "$SRC/desktop/linux"
IMAGE_BUILT=1
in_image() { # docker run options..., then build-in-container.sh's mode
  docker run --rm --platform "$PLATFORM" --user "$(id -u):$(id -g)" -e HOME=/work/home \
    -e VERSION="$VERSION" -e SOURCE_DATE_EPOCH="$COMMIT_TIME" -v "$WORK:/work" "$@"
}

say "1/2 with the network: the crates, and the versions"
PREPARED=$(in_image "$IMAGE" bash /work/src/desktop/linux/build-in-container.sh prepare)
printf '%s\n' "$PREPARED"
SIDE_VERSION=$(sed -n 's/^sidecar=//p' <<<"$PREPARED")
SIDE_REV=${SIDE_VERSION##*+g}
# The server of this very ref (desktop/build.sh's rule): another one would
# ship under this app's version, and the notices would not be its own.
if [ "$SIDE_REV" = "$SIDE_VERSION" ] || [ "${SIDE_VERSION%+g*}" != "$VERSION" ] || ! [[ "$SIDE_REV" =~ ^[0-9a-f]{7,40}$ ]] \
  || [ "$(git -C "$REPO" rev-parse --verify --quiet "$SIDE_REV^{commit}" 2>/dev/null)" != "$SHA" ]; then
  die "$SIDECAR says it is \"$SIDE_VERSION\", not $VERSION+g<a revision of $SHA>: it is not the server of this ref"
fi
TAURI_CLI_VERSION=$(sed -n 's/^TAURI_CLI_VERSION=//p' "$REPO/desktop/build.sh")
[ "$(sed -n 's/^tauri_cli=//p' <<<"$PREPARED")" = "tauri-cli $TAURI_CLI_VERSION" ] || die "the tools' cargo-tauri is not tauri-cli $TAURI_CLI_VERSION"
echo "ok: the sidecar is $SIDE_VERSION, this ref's server; tauri-cli $TAURI_CLI_VERSION"

say "2/2 without the network: the app, its bundles, their checks"
in_image --network none "$IMAGE" bash /work/src/desktop/linux/build-in-container.sh build

cp "$WORK/out"/* "$OUT/"
say "built in $OUT"
ls -l "$OUT"
cat <<EOF

Nothing has been published. The AppImage's update is signed by the release
(.github/workflows/release.yml); the .deb is covered by the release's
SHA256SUMS and attestation.
EOF
