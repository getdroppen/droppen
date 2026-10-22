#!/usr/bin/env bash
# Unpack a tar another job of .github/workflows/release.yml made (an
# artifact), only if it is the very file that job reported and holds only
# what it should. Every artifact goes through here, into an empty directory
# of its own:
#
#   .github/scripts/unpack-artifact.sh <tar> <sha256> <dest> <allowed>
#
#   <sha256>   what the producing job put out for it (a job output, which no
#              other job can change), so an artifact replaced while the run
#              waits for an approval is refused
#   <allowed>  an extended regular expression every member's path (without a
#              trailing slash) must match as a whole; members are regular
#              files and directories only (no links, devices or FIFOs), and
#              no path is absolute or has a `.` or `..` component
set -euo pipefail

TARBALL=${1:?usage: unpack-artifact.sh <tar> <sha256> <dest> <allowed ERE>}
WANT=${2:-}
DEST=${3:?usage: unpack-artifact.sh <tar> <sha256> <dest> <allowed ERE>}
ALLOWED=${4:?usage: unpack-artifact.sh <tar> <sha256> <dest> <allowed ERE>}
NAME=$(basename "$TARBALL")
die() { printf 'unpack-artifact: %s: %s\n' "$NAME" "$*" >&2; exit 1; }

[[ "$WANT" =~ ^[0-9a-f]{64}$ ]] || die "no sha256 to check it against (the job that made it put out none)"
[ -f "$TARBALL" ] || die "no such file"
if command -v sha256sum >/dev/null 2>&1; then GOT=$(sha256sum "$TARBALL"); else GOT=$(shasum -a 256 "$TARBALL"); fi
GOT=${GOT%% *}
[ "$GOT" = "$WANT" ] || die "its sha256 is $GOT, not $WANT: not the file its job made"
if [ -e "$DEST" ] && [ -n "$(ls -A "$DEST")" ]; then die "$DEST is not empty"; fi

# Types first: the first character of each `tar -tv` line (GNU tar and bsdtar
# alike) is `-` for a file and `d` for a directory.
LC_ALL=C tar -tvf "$TARBALL" | while IFS= read -r line; do
  case "${line:0:1}" in
    - | d) ;;
    *) die "a member that is neither a file nor a directory: $line" ;;
  esac
done
COUNT=0
while IFS= read -r member; do
  path=${member%/}
  [ -n "$path" ] || die "a member with an empty path"
  case "/$path/" in
    //* | */./* | */../*) die "a member outside its directory: $member" ;;
  esac
  [[ "$path" =~ $ALLOWED ]] || die "a member it should not have: $member"
  COUNT=$((COUNT + 1))
done < <(LC_ALL=C tar -tf "$TARBALL")
[ "$COUNT" -gt 0 ] || die "empty"

mkdir -p "$DEST"
tar -xf "$TARBALL" -C "$DEST"
echo "ok: $NAME ($GOT, $COUNT members) in $DEST"
