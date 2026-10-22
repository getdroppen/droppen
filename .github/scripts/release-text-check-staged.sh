#!/usr/bin/env bash
# The release text check (scripts/release-text-check.sh) over everything
# scripts/release-public.sh staged, with the names: release.yml's build jobs
# run dependency code, so they are not given the names and check "Kilerd" only
# (DROPPEN_RELEASE_NAMES_DEFERRED); the release job, which has them, runs this
# before anything is signed or published. desktop/notarise.sh and
# desktop/updater-artifacts.sh check the app's files the same way there.
#
#   .github/scripts/release-text-check-staged.sh <release-public.sh's --out>
#
# Every text file of the release: NOTES.md, SHA256SUMS, LICENSE,
# THIRD_PARTY_NOTICES (as notices), web-manifest.json, install.sh and
# INSTALL-*.txt, and every member of
# every tarball but the programs (read out by name: the job that runs this
# holds secrets, and nothing from an archive's paths is written). The names come from DROPPEN_RELEASE_FORBIDDEN
# or ~/.config/droppen/release-forbidden.env (read as data), and must be the
# pinned list; DROPPEN_RELEASE_NAMES_DEFERRED=1 without them is release.yml's
# dry run without the secret, which checks "Kilerd" only and says so. Set
# DROPPEN_RELEASE_TEXT_QUIET=1 where the output is public: a hit is then shown
# by its line number only.
set -euo pipefail

DIR=${1:?usage: release-text-check-staged.sh <staged release dir>}
ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
die() { printf 'release-text-check-staged: %s\n' "$*" >&2; exit 1; }

. "$ROOT/scripts/release-text-check.sh"
release_text_load_names "$HOME/.config/droppen/release-forbidden.env"
if release_text_names_deferred; then
  echo "note: DROPPEN_RELEASE_NAMES_DEFERRED and no names: \"Kilerd\" only"
else
  release_text_names_ok \
    || die "DROPPEN_RELEASE_FORBIDDEN is not the expected list of names (its sha256 is RELEASE_TEXT_NAMES_SHA256 in scripts/release-text-check.sh)"
  release_text_allow_ok \
    || die "DROPPEN_RELEASE_NOTICE_ALLOW may only spell out whole third-party copyright lines, each ^Copyright …\$ with its metacharacters escaped"
fi

[ -d "$DIR" ] || die "$DIR is not a directory"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/release-text-check-staged.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

FOUND=0
CHECKED=0
check() { # file, label, [notices]
  local hits
  hits=$(release_text_hits "$1" "${3:-}")
  CHECKED=$((CHECKED + 1))
  if [ -n "$hits" ]; then
    printf '%s names a person it should not:\n%s\n' "$2" "$hits" >&2
    FOUND=$((FOUND + 1))
  fi
}

for name in NOTES.md SHA256SUMS LICENSE web-manifest.json install.sh; do
  [ -f "$DIR/$name" ] || die "$DIR has no $name"
  check "$DIR/$name" "$name"
done
for file in "$DIR"/INSTALL-*.txt; do
  [ -f "$file" ] || continue
  check "$file" "$(basename "$file")"
done
[ -f "$DIR/THIRD_PARTY_NOTICES" ] || die "$DIR has no THIRD_PARTY_NOTICES"
check "$DIR/THIRD_PARTY_NOTICES" THIRD_PARTY_NOTICES notices

TARBALLS=0
for tarball in "$DIR"/*.tar.gz; do
  [ -f "$tarball" ] || continue
  TARBALLS=$((TARBALLS + 1))
  name=$(basename "$tarball" .tar.gz)
  # Exactly the members release-public.sh packs, and nothing is written from
  # the archive's paths: each text member is read out by name (`tar -O`).
  case "$name" in
    droppen-*) want=$'droppen\nremote-hand-server\nINSTALL.txt\nLICENSE\nTHIRD_PARTY_NOTICES' ;;
    remote-hand-*) want=$'remote-hand-server\nINSTALL.txt\nLICENSE\nTHIRD_PARTY_NOTICES' ;;
    *) die "an unexpected tarball: $name.tar.gz" ;;
  esac
  tar -tzf "$tarball" >"$WORK/$name.listing"
  [ "$(cat "$WORK/$name.listing")" = "$want" ] || die "$name.tar.gz does not hold exactly the members release-public.sh packs"
  # The listing too: a member's name is text as well.
  check "$WORK/$name.listing" "$name.tar.gz (its listing)"
  for member in INSTALL.txt LICENSE THIRD_PARTY_NOTICES; do
    tar -xOzf "$tarball" "$member" >"$WORK/$name.$member"
    if [ "$member" = THIRD_PARTY_NOTICES ]; then
      check "$WORK/$name.$member" "$name.tar.gz: $member" notices
    else
      check "$WORK/$name.$member" "$name.tar.gz: $member"
    fi
  done
done
[ "$TARBALLS" -gt 0 ] || die "$DIR has no tarballs"

[ "$FOUND" = 0 ] || die "$FOUND of $CHECKED files name a person (above)"
if release_text_names_deferred; then
  echo "ok: $CHECKED files of $TARBALLS tarballs and the release: \"Kilerd\" only on the copyright lines (the names were NOT checked)"
else
  echo "ok: $CHECKED files of $TARBALLS tarballs and the release: no personal names"
fi
