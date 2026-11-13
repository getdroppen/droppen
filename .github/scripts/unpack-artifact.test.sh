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

# Only a plain tar is read, and no decompressor ever runs on it: tar would
# pick one by the first bytes, whatever its options, and run it from PATH.
# Logging stand-ins for every one of them go first on PATH for these cases.
mkdir -p "$TMP/shims"
for prog in zstd unzstd lz4 lzop lrzip grzip xz unxz lzma bzip2 gzip lzip compress; do
  printf '#!/bin/sh\necho "%s $*" >>"%s/decompressor.log"\ncat\n' "$prog" "$TMP" >"$TMP/shims/$prog"
  chmod +x "$TMP/shims/$prog"
done
refused_unread() { # description, tar
  local sum
  sum=$(sha "$2")
  rm -f "$TMP/decompressor.log"
  if PATH="$TMP/shims:$PATH" "$UNPACK" "$2" "$sum" "$TMP/out-$PASSED-$FAILED" '.*' >"$TMP/refusal.log" 2>&1; then
    FAILED=$((FAILED + 1)); echo "FAIL: $1: want refused, got ok" >&2
  elif [ -e "$TMP/decompressor.log" ]; then
    FAILED=$((FAILED + 1)); echo "FAIL: $1: a decompressor ran: $(cat "$TMP/decompressor.log")" >&2
  else
    PASSED=$((PASSED + 1))
  fi
}
mk packed; mkdir bin; echo a > bin/cargo-about
COPYFILE_DISABLE=1 tar -czf "$TMP/gzip.tar" bin
refused_unread "a gzip-compressed tar" "$TMP/gzip.tar"
# A real tar, \"ustar\" at 257 and all, whose first bytes are zstd's magic: the
# bytes bsdtar and GNU tar both take as zstd, and hand to the zstd on PATH.
cp "$TMP/good.tar" "$TMP/zstd-magic.tar"
printf '\050\265\057\375' | dd of="$TMP/zstd-magic.tar" bs=1 seek=0 count=4 conv=notrunc 2>/dev/null
refused_unread "a tar that starts with zstd's magic" "$TMP/zstd-magic.tar"
grep -q "compressed (its first bytes are 28b52ffd)" "$TMP/refusal.log" && PASSED=$((PASSED + 1)) \
  || { FAILED=$((FAILED + 1)); echo "FAIL: the zstd tar was not refused as compressed: $(cat "$TMP/refusal.log")" >&2; }
if command -v zstd >/dev/null 2>&1; then
  zstd -q -c "$TMP/good.tar" >"$TMP/zstd.tar"
  refused_unread "a zstd-compressed tar" "$TMP/zstd.tar"
fi
printf '\004\042\115\030' | cat - "$TMP/good.tar" >"$TMP/lz4-magic.tar"
refused_unread "a tar after lz4's magic" "$TMP/lz4-magic.tar"
head -c 2048 /dev/zero | tr '\0' 'x' >"$TMP/text.tar"
refused_unread "no tar at all" "$TMP/text.tar"
printf 'short' >"$TMP/short.tar"
refused_unread "shorter than a header" "$TMP/short.tar"

echo "unpack-artifact: $PASSED passed, $FAILED failed"
[ "$FAILED" = 0 ]
