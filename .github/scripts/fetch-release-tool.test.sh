#!/usr/bin/env bash
# Tests for fetch-release-tool.sh: `bash .github/scripts/fetch-release-tool.test.sh`.
# Offline: the script runs from a copy of the layout it reads (the lock, the
# two files that pin the versions), with a curl on PATH that serves local
# files and logs what it was asked for.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/fetch-release-tool-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
PASSED=0
FAILED=0
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
pass() { PASSED=$((PASSED + 1)); }
fail() { FAILED=$((FAILED + 1)); echo "FAIL: $*" >&2; }

ROOT=$TMP/root
mkdir -p "$ROOT/.github/scripts" "$ROOT/scripts" "$ROOT/desktop" "$TMP/files" "$TMP/fakebin"
cp "$HERE/fetch-release-tool.sh" "$ROOT/.github/scripts/"
echo 'ABOUT_VERSION=0.9.2' > "$ROOT/scripts/release-public.sh"
echo 'TAURI_CLI_VERSION=2.12.3' > "$ROOT/desktop/build.sh"
FETCH=$ROOT/.github/scripts/fetch-release-tool.sh

# curl -fsSL ... -o <file> <url>: the file of that name from $TMP/files.
cat > "$TMP/fakebin/curl" <<'EOF'
#!/usr/bin/env bash
out="" url=""
while [ $# -gt 0 ]; do
  case "$1" in -o) out=$2; shift 2 ;; --proto|--retry) shift 2 ;; -*) shift ;; *) url=$1; shift ;; esac
done
echo "$url" >> "$CURL_LOG"
[ -f "$FILES/$(basename "$url")" ] || exit 22
cp "$FILES/$(basename "$url")" "$out"
EOF
chmod +x "$TMP/fakebin/curl"
export FILES=$TMP/files CURL_LOG=$TMP/curl.log

ABOUT_URL=https://github.com/EmbarkStudios/cargo-about/releases/download/0.9.2/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz
TAURI_URL=https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v2.12.3/cargo-tauri-aarch64-apple-darwin.zip
mkdir -p "$TMP/mk/cargo-about-0.9.2-aarch64-apple-darwin"
echo about > "$TMP/mk/cargo-about-0.9.2-aarch64-apple-darwin/cargo-about"
tar -czf "$FILES/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz" -C "$TMP/mk" cargo-about-0.9.2-aarch64-apple-darwin
ABOUT_SHA=$(sha "$FILES/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz")
echo tauri > "$TMP/mk/cargo-tauri"
ZIP=1
if command -v zip >/dev/null 2>&1; then
  (cd "$TMP/mk" && zip -q "$FILES/cargo-tauri-aarch64-apple-darwin.zip" cargo-tauri)
elif command -v python3 >/dev/null 2>&1; then
  python3 -I -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1], "w").write(sys.argv[2], "cargo-tauri")' \
    "$FILES/cargo-tauri-aarch64-apple-darwin.zip" "$TMP/mk/cargo-tauri"
else
  ZIP=0
fi
TAURI_SHA=$( [ "$ZIP" = 0 ] || sha "$FILES/cargo-tauri-aarch64-apple-darwin.zip")
TAURI_SHA=${TAURI_SHA:-0000000000000000000000000000000000000000000000000000000000000000}

N=0
expect() { # want (ok|refused), description, tool, lock line, [platform]
  local want=$1 what=$2 tool=$3 line=$4 platform=${5:-aarch64-apple-darwin} got=ok
  N=$((N + 1))
  DEST=$TMP/out-$N
  printf '# a test lock\n%s\n' "$line" > "$ROOT/.github/release-tools.lock"
  rm -f "$CURL_LOG"
  PATH="$TMP/fakebin:$PATH" "$FETCH" "$tool" "$DEST" "$platform" >"$TMP/log-$N" 2>&1 || got=refused
  if [ "$got" = "$want" ]; then pass; else fail "$what: want $want, got $got ($(tail -n 1 "$TMP/log-$N"))"; fi
}
downloaded() { [ -f "$CURL_LOG" ] && cat "$CURL_LOG"; }
lock() { printf '%-13s %-8s %-21s %s  %s' "$@"; }

expect ok "cargo-about from its upstream release" cargo-about "$(lock cargo-about 0.9.2 aarch64-apple-darwin "$ABOUT_SHA" "$ABOUT_URL")"
[ "$(cat "$TMP/out-$N/bin/cargo-about" 2>/dev/null)" = about ] && pass || fail "cargo-about not put in bin/"
[ "$(downloaded)" = "$ABOUT_URL" ] && pass || fail "downloaded $(downloaded), not $ABOUT_URL"

# Another file with the same hash: refused before anything is downloaded.
for url in \
  https://github.com/someone-else/cargo-about/releases/download/0.9.2/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz \
  https://github.com/EmbarkStudios/cargo-about-fork/releases/download/0.9.2/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz \
  https://github.com/EmbarkStudios/cargo-about/releases/download/0.9.1/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz \
  https://github.com/EmbarkStudios/cargo-about/releases/download/0.9.2/cargo-about-0.9.2-x86_64-apple-darwin.tar.gz \
  https://github.com/EmbarkStudios/cargo-about/releases/download/0.9.2/../../../../someone-else/x/releases/download/0.9.2/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz \
  http://github.com/EmbarkStudios/cargo-about/releases/download/0.9.2/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz \
  https://github.com.example.org/EmbarkStudios/cargo-about/releases/download/0.9.2/cargo-about-0.9.2-aarch64-apple-darwin.tar.gz \
  "$TAURI_URL"; do
  expect refused "cargo-about from $url" cargo-about "$(lock cargo-about 0.9.2 aarch64-apple-darwin "$ABOUT_SHA" "$url")"
  [ -z "$(downloaded)" ] && pass || fail "downloaded $(downloaded) for a refused url"
done
expect refused "the url and something after it" cargo-about "$(lock cargo-about 0.9.2 aarch64-apple-darwin "$ABOUT_SHA" "$ABOUT_URL") x"
expect refused "another sha256" cargo-about "$(lock cargo-about 0.9.2 aarch64-apple-darwin 0000000000000000000000000000000000000000000000000000000000000000 "$ABOUT_URL")"
expect refused "no sha256" cargo-about "$(lock cargo-about 0.9.2 aarch64-apple-darwin - "$ABOUT_URL")"
expect refused "a version the source does not pin" cargo-about "$(lock cargo-about 0.9.1 aarch64-apple-darwin "$ABOUT_SHA" "${ABOUT_URL//0.9.2/0.9.1}")"
expect refused "a tool not in the lock's list" cargo-deny "$(lock cargo-deny 0.9.2 aarch64-apple-darwin "$ABOUT_SHA" "$ABOUT_URL")"
expect refused "a platform outside the list" cargo-about "$(lock cargo-about 0.9.2 ../x "$ABOUT_SHA" "$ABOUT_URL")" ../x

for url in \
  https://github.com/someone-else/tauri/releases/download/tauri-cli-v2.12.3/cargo-tauri-aarch64-apple-darwin.zip \
  https://github.com/tauri-apps/tauri/releases/download/tauri-v2.12.3/cargo-tauri-aarch64-apple-darwin.zip \
  https://github.com/tauri-apps/tauri-cli/releases/download/tauri-cli-v2.12.3/cargo-tauri-aarch64-apple-darwin.zip \
  "$ABOUT_URL"; do
  expect refused "tauri-cli from $url" tauri-cli "$(lock tauri-cli 2.12.3 aarch64-apple-darwin "$TAURI_SHA" "$url")"
  [ -z "$(downloaded)" ] && pass || fail "downloaded $(downloaded) for a refused url"
done
if [ "$ZIP" = 1 ]; then
  expect ok "tauri-cli from its upstream release" tauri-cli "$(lock tauri-cli 2.12.3 aarch64-apple-darwin "$TAURI_SHA" "$TAURI_URL")"
  [ "$(cat "$TMP/out-$N/bin/cargo-tauri" 2>/dev/null)" = tauri ] && pass || fail "cargo-tauri not put in bin/"
else
  echo "note: neither zip nor python3 here: the tauri-cli download itself is not tested" >&2
fi

echo "fetch-release-tool: $PASSED passed, $FAILED failed"
[ "$FAILED" = 0 ]
