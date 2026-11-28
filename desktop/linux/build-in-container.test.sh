#!/usr/bin/env bash
# Tests for desktop/linux/build-in-container.sh's fetch_dsc_from_pool, offline:
# stand-in archives on a local HTTP server holding stand-in source packages.
# `bash desktop/linux/build-in-container.test.sh`.
#
# What it checks: the pool path of a source package (Debian's
# pool/main/<letter>/<source>/, the fourth letter of lib*), every file of the
# .dsc found beside it, the hash and size check, the version match, the first
# archive that has it winning, and a version or a file no archive has failing.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd -P)
SCRIPT=$HERE/build-in-container.sh
TMP=$(mktemp -d "${TMPDIR:-/tmp}/build-in-container-test.XXXXXX")
trap '[ -n "${PID:-}" ] && kill "$PID" 2>/dev/null; rm -rf "$TMP"' EXIT
PASSED=0
FAILED=0
pass() { PASSED=$((PASSED + 1)); }
fail() { FAILED=$((FAILED + 1)); echo "FAIL: $*" >&2; }

# fetch_dsc_from_pool and the helpers it uses, cut out of the script: no
# docker, no apt, no build runs here.
{
  echo 'set -euo pipefail'
  echo 'die() { printf "build-in-container: %s\n" "$*" >&2; exit 1; }'
  sed -n '/^dsc_files()/,/^}/p' "$SCRIPT"
  sed -n '/^fetch_dsc_from_pool()/,/^}/p' "$SCRIPT"
  echo '"$@"'
} >"$TMP/fn.sh"

mkpkg() { # archive-dir, source, version, file...
  local dir=$1 src=$2 ver=$3 area d sha size
  shift 3
  case "$src" in lib*) area=${src:3:1} ;; *) area=${src:0:1} ;; esac
  d=$dir/pool/main/$area/$src
  mkdir -p "$d"
  {
    printf 'Format: 3.0 (quilt)\nSource: %s\nVersion: %s\nChecksums-Sha256:\n' "$src" "$ver"
    for f in "$@"; do
      printf 'payload of %s\n' "$f" >"$d/$f"
      sha=$(sha256sum "$d/$f" | cut -d' ' -f1)
      size=$(wc -c <"$d/$f" | tr -d ' ')
      printf ' %s %s %s\n' "$sha" "$size" "$f"
    done
  } >"$d/${src}_${ver#*:}.dsc"
}

SRV=$TMP/srv
mkdir -p "$SRV/one" "$SRV/two"
# one: libssl (area l, the fourth letter of lib*); two: libssl too, and krb5
# (area k) which one does not have.
mkpkg "$SRV/one" libssl3 3.0.2-0ubuntu1.20 libssl3_3.0.2-0.orig.tar.gz libssl3_3.0.2-0.debian.tar.xz
mkpkg "$SRV/two" libssl3 3.0.2-0ubuntu1.20 libssl3_3.0.2-0.orig.tar.gz libssl3_3.0.2-0.debian.tar.xz
mkpkg "$SRV/two" krb5 1.19.2-2ubuntu0.12 krb5_1.19.2.orig.tar.gz krb5_1.19.2-2ubuntu0.12.debian.tar.xz

python3 -u -c 'import sys, http.server, functools
done = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[1]))
print(done.server_address[1], flush=True)
done.serve_forever()' "$SRV" >"$TMP/server.log" 2>&1 &
PID=$!
for _ in $(seq 100); do
  PORT=$(head -1 "$TMP/server.log" 2>/dev/null | tr -cd '0-9')
  [ -n "$PORT" ] && break
  sleep 0.1
done
[ -n "${PORT:-}" ] || { echo "no local server" >&2; exit 1; }
BASE=http://127.0.0.1:$PORT

# The function's three bases, rewritten to the stand-in's two archives.
sed -e "s#https://security.ubuntu.com/ubuntu#$BASE/one#g" \
    -e "s#https://archive.ubuntu.com/ubuntu#$BASE/two#g" \
    -e "s#https://ports.ubuntu.com/ubuntu-ports#$BASE/none#g" \
    -e "s#--proto '=https' --proto-redir '=https' --tlsv1.2#--proto '=http' --proto-redir '=http'#" \
    "$TMP/fn.sh" >"$TMP/fn-http.sh"

dst() { mkdir -p "$TMP/$1"; echo "$TMP/$1"; }
# libssl: the first archive has it.
if D=$(dst a) && bash "$TMP/fn-http.sh" fetch_dsc_from_pool libssl3 3.0.2-0ubuntu1.20 "$D" >/dev/null 2>&1; then pass; else fail "libssl3 from the first archive"; fi
for f in libssl3_3.0.2-0ubuntu1.20.dsc libssl3_3.0.2-0.orig.tar.gz libssl3_3.0.2-0.debian.tar.xz; do
  [ -f "$TMP/a/$f" ] && pass || fail "libssl3: $f"
done
# krb5: only the second has it (area k, not lib*).
if D=$(dst b) && bash "$TMP/fn-http.sh" fetch_dsc_from_pool krb5 1.19.2-2ubuntu0.12 "$D" >/dev/null 2>&1; then pass; else fail "krb5 from the second archive"; fi
[ -f "$TMP/b/krb5_1.19.2-2ubuntu0.12.dsc" ] && pass || fail "krb5: its .dsc"
# The version is matched, not assumed: one no archive has is not taken.
if D=$(dst c) && bash "$TMP/fn-http.sh" fetch_dsc_from_pool krb5 1.19.2-2ubuntu0.13 "$D" >/dev/null 2>&1; then fail "a version no archive has"; else pass; fi
# A file the .dsc names and the pool lacks: the hash check fails it.
rm "$SRV/two/pool/main/k/krb5/krb5_1.19.2-2ubuntu0.12.debian.tar.xz"
if D=$(dst d) && bash "$TMP/fn-http.sh" fetch_dsc_from_pool krb5 1.19.2-2ubuntu0.12 "$D" >/dev/null 2>&1; then fail "a .dsc whose file is missing"; else pass; fi

echo "build-in-container test: $PASSED passed, $FAILED failed"
[ "$FAILED" = 0 ]
