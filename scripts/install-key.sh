#!/bin/sh
# The release key an install.sh checks the release's SHA256SUMS-server with
# (its SUMS_KEY, docs/releasing.md "The release key"): the PEM on standard
# output, or nothing while it has none (`SUMS_KEY=''`, or an installer from
# before there was one). A SUMS_KEY written any other way is refused (exit 1)
# rather than taken for "no key".
#
#   scripts/install-key.sh <install.sh>
#
# Used by scripts/publish-release.sh and .github/workflows/release.yml.
set -eu

[ $# -eq 1 ] && [ -f "$1" ] || { echo "usage: $0 <install.sh>" >&2; exit 2; }
n=$(grep -c '^SUMS_KEY=' "$1" || true)
[ "$n" != 0 ] || exit 0
[ "$n" = 1 ] || { echo "$1: expected one SUMS_KEY= line, found $n" >&2; exit 1; }
awk -v q="'" '
  !on && $0 == "SUMS_KEY=" q q { found = 1; exit }
  !on && $0 == "SUMS_KEY=" q "-----BEGIN PUBLIC KEY-----" { on = 1; found = 1; print "-----BEGIN PUBLIC KEY-----"; next }
  on && $0 == "-----END PUBLIC KEY-----" q { print "-----END PUBLIC KEY-----"; on = 0; exit }
  on && /^[A-Za-z0-9+\/=]+$/ { print; next }
  on { bad = 1; exit }
  END { if (!found || on || bad) exit 1 }
' "$1" || { echo "$1: SUMS_KEY is neither '' nor a PEM public key block" >&2; exit 1; }
