#!/usr/bin/env bash
# Tests for unpack-artifact.sh: `bash .github/scripts/unpack-artifact.test.sh`.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
UNPACK=$HERE/unpack-artifact.sh
TMP=$(mktemp -d "${TMPDIR:-/tmp}/unpack-artifact-test.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
PASSED=0
FAILED=0
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
expect() { # want (ok|refused), description, tar, [sha], [allowed]
  local want=$1 what=$2 tarball=$3 sum=${4:-} allowed=${5:-'^bin(/(cargo-about|cargo-tauri))?$'} got=ok dest
  [ -n "$sum" ] || sum=$(sha "$tarball")
  dest=$TMP/out-$PASSED-$FAILED
  "$UNPACK" "$tarball" "$sum" "$dest" "$allowed" >/dev/null 2>&1 || got=refused
  if [ "$got" = "$want" ]; then PASSED=$((PASSED + 1)); else FAILED=$((FAILED + 1)); echo "FAIL: $what: want $want, got $got" >&2; fi
}
mk() { mkdir -p "$TMP/src/$1" && rm -rf "$TMP/src/$1"/* && cd "$TMP/src/$1"; }

mk good; mkdir bin; echo a > bin/cargo-about; echo t > bin/cargo-tauri
COPYFILE_DISABLE=1 tar -cf "$TMP/good.tar" bin
expect ok "the expected members" "$TMP/good.tar"
expect refused "another sha256" "$TMP/good.tar" 0000000000000000000000000000000000000000000000000000000000000000
expect refused "no sha256" "$TMP/good.tar" "-"
expect refused "a member not allowed" "$TMP/good.tar" "" '^bin(/cargo-about)?$'
grep -q '^t$' "$TMP/out-0-0/bin/cargo-tauri" && PASSED=$((PASSED + 1)) || { FAILED=$((FAILED + 1)); echo "FAIL: not unpacked" >&2; }
mkdir -p "$TMP/busy" && touch "$TMP/busy/x"
"$UNPACK" "$TMP/good.tar" "$(sha "$TMP/good.tar")" "$TMP/busy" '^bin(/(cargo-about|cargo-tauri))?$' >/dev/null 2>&1 \
  && { FAILED=$((FAILED + 1)); echo "FAIL: a destination that is not empty" >&2; } || PASSED=$((PASSED + 1))

mk link; mkdir bin; ln -s /etc/hosts bin/cargo-about
COPYFILE_DISABLE=1 tar -cf "$TMP/link.tar" bin
expect refused "a symbolic link" "$TMP/link.tar"
mk hard; mkdir bin; echo a > bin/cargo-about; ln bin/cargo-about bin/cargo-tauri
COPYFILE_DISABLE=1 tar -cf "$TMP/hard.tar" bin
expect refused "a hard link" "$TMP/hard.tar"
mk dots; mkdir -p bin/x; echo a > bin/cargo-about
COPYFILE_DISABLE=1 tar -cf "$TMP/dots.tar" bin/x/../cargo-about 2>/dev/null || COPYFILE_DISABLE=1 tar -cf "$TMP/dots.tar" -P bin/x/../cargo-about
expect refused "a .. component" "$TMP/dots.tar" "" '^bin(/[^/]+)*$'
mk abs; echo a > "$TMP/src/abs/file"
COPYFILE_DISABLE=1 tar -cPf "$TMP/abs.tar" "$TMP/src/abs/file"
expect refused "an absolute path" "$TMP/abs.tar" "" '.*'

echo "unpack-artifact: $PASSED passed, $FAILED failed"
[ "$FAILED" = 0 ]
