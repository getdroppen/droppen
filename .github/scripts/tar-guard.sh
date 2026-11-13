# shellcheck shell=bash
# What release.yml's jobs check before tar reads an archive another job made:
# that it is a plain tar, or a gzip whose content is one. Sourced by
# unpack-artifact.sh, unpack-desktop-linux.sh and the release job's text
# checks.
#
# Why: when it reads, tar picks the decompressor by the archive's first bytes,
# whatever the command line says. GNU tar runs gzip, xz, zstd and the others as
# programs found on PATH; macOS's bsdtar does so for the ones it does not
# carry (zstd, lz4, lzop, lrzip, grzip: Homebrew's, on a runner). So an
# artifact a build job made could have such a program parse its bytes in the
# job that reads it, the one that holds the secrets included. Hence a plain tar
# only (or a gzip this file decompresses itself, with the system's gzip), and
# tar and gzip run with the system's PATH (TAR_GUARD_PATH), where macOS has
# no zstd, lz4, lzop, lrzip or grzip at all.

TAR_GUARD_PATH=/usr/bin:/bin

# The first bytes of every compressed stream either tar would decompress
# (GNU tar's magic table and libarchive's bidders), in hex.
TAR_GUARD_MAGICS="1f8b 1f9d 425a68 fd377a585a00 5d0000 4c5a4950 28b52ffd 04224d18 02214c18 894c5a4f 4c525a49 47525a6970 626567696e20 edabeedb"

# $3 bytes of file $1 from offset $2, in hex.
tar_guard_hex() { od -An -v -tx1 -j "$2" -N "$3" "$1" | tr -d ' \n'; }

# Whether file $1 is a plain tar: none of the compressed magics at its start,
# at least a header long, and "ustar" at offset 257 (POSIX, pax and GNU headers
# alike). Says why not on stderr.
tar_guard_plain() {
  local size head magic
  [ -f "$1" ] && [ ! -L "$1" ] || { echo "$1 is not a file" >&2; return 1; }
  head=$(tar_guard_hex "$1" 0 8)
  for magic in $TAR_GUARD_MAGICS; do
    case "$head" in "$magic"*) echo "$1 is compressed (its first bytes are $magic), not a plain tar" >&2; return 1 ;; esac
  done
  size=$(wc -c <"$1" | tr -d ' ')
  [ "$size" -ge 512 ] || { echo "$1 is shorter than a tar header" >&2; return 1; }
  [ "$(tar_guard_hex "$1" 257 5)" = 7573746172 ] || { echo "$1 is not a tar (no ustar header)" >&2; return 1; }
}

# The plain tar inside gzip file $1, as file $2: gzip's own magic first, the
# system's gzip to decompress, then the same check of what came out (gzip
# around another compressed stream is refused too).
tar_guard_gunzip() {
  [ -f "$1" ] && [ ! -L "$1" ] || { echo "$1 is not a file" >&2; return 1; }
  [ "$(tar_guard_hex "$1" 0 2)" = 1f8b ] || { echo "$1 is not gzip" >&2; return 1; }
  /usr/bin/env PATH="$TAR_GUARD_PATH" gzip -dc <"$1" >"$2" || { echo "$1 does not decompress" >&2; return 1; }
  tar_guard_plain "$2"
}

# tar, the system's, with the system's PATH: only ever on a file
# tar_guard_plain accepted.
tar_guard_tar() { /usr/bin/env PATH="$TAR_GUARD_PATH" tar "$@"; }
