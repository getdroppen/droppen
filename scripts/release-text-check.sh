# The personal-name rule of scripts/release-public.sh's text checks, apart from
# that script so scripts/release-text-check.test.sh can exercise it. Sourced,
# not run; bash 3.2 (macOS's) is enough.
#
# release_text_hits <file> [notices] prints the lines of <file> (numbered, as
# `grep -n` prints them) that name a person they should not, and nothing when
# the file is clean. It always succeeds: the caller decides what a hit means.
# A file it cannot read is a hit.
#
#  - "Kilerd", in any case, only on a line that is the copyright notice and
#    nothing else (COPYRIGHT_KILERD: `Copyright (C) 2026 Kilerd`, indented or
#    not, any year). In THIRD_PARTY_NOTICES (`notices`), which quotes other
#    people's licences, also in the repository URLs of the published crates
#    listed in NOTICE_KILERD (github.com/Kilerd/<crate>: gotcha, mofa, oas).
#    Never in any other form: the private repository, a sentence, a path.
#  - Each name of DROPPEN_RELEASE_FORBIDDEN (release_text_names), a string
#    matched in any case, nowhere; except, in THIRD_PARTY_NOTICES, on a line
#    that one of the extended regular expressions in
#    DROPPEN_RELEASE_NOTICE_ALLOW (one per line) matches as a whole: a
#    third-party copyright line that happens to contain one.
#
# The release operator sets both variables: personal names other than the
# copyright holder's are not written into this repository. A release refuses
# to start unless DROPPEN_RELEASE_FORBIDDEN is the expected list
# (release_text_names_ok), or the names are deferred
# (release_text_names_deferred).
#
# In GitHub Actions (.github/workflows/release.yml) the names come from the
# `release` environment's secret RELEASE_FORBIDDEN_ENV, and only the release
# job has them. Two variables are for that workflow:
#   DROPPEN_RELEASE_NAMES_DEFERRED=1  the build jobs, which run dependency
#       code and so are not given the names: the scripts check "Kilerd" only
#       and leave the names to the release job, which checks every staged text
#       file with them (.github/scripts/release-text-check-staged.sh) before
#       anything is signed or published. Ignored once the names are set.
#   DROPPEN_RELEASE_TEXT_QUIET=1  a hit is reported by its line number only,
#       never with the line, which names the person: the logs are public.

COPYRIGHT_KILERD='^[[:space:]]*Copyright \([Cc]\) [0-9]{4}(-[0-9]{4})? Kilerd$'
NOTICE_KILERD='https?://github\.com/Kilerd/(gotcha|mofa|oas)(\.git)?/?([[:space:]]|$)'

# The sha256 of the names DROPPEN_RELEASE_FORBIDDEN must hold, normalised as
# release_text_names does it. It pins the list without writing the names here,
# so an empty, short, partial or placeholder list does not pass. With the list
# in DROPPEN_RELEASE_FORBIDDEN, the operator computes it on the Mac with
# (`sha256sum` for `shasum -a 256` on Linux)
#
#   printf '%s\n' "$DROPPEN_RELEASE_FORBIDDEN" | LC_ALL=C tr -d '\r' | LC_ALL=C tr '[:upper:]' '[:lower:]' | LC_ALL=C sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e '/^$/d' | LC_ALL=C sort -u | shasum -a 256
#
# and replaces the value below when the list changes. A hash of a few names
# keeps them out of plain sight; it does not make them secret.
RELEASE_TEXT_NAMES_SHA256=6b3a4334ab6f808d90afdcbce25a20306249ba790a544c0057d7fb17436db1cb

# The names in DROPPEN_RELEASE_FORBIDDEN, one per line: without carriage
# returns, lower-case (the match ignores case), trimmed, without blank lines,
# sorted, each once. The pipeline of the command above, minus the hash.
release_text_names() {
  printf '%s\n' "${DROPPEN_RELEASE_FORBIDDEN:-}" | LC_ALL=C tr -d '\r' | LC_ALL=C tr '[:upper:]' '[:lower:]' \
    | LC_ALL=C sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e '/^$/d' | LC_ALL=C sort -u
}

# The sha256 of standard input, in hex.
release_text_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum; else shasum -a 256; fi | cut -d' ' -f1
}

# Whether DROPPEN_RELEASE_FORBIDDEN is the expected list.
release_text_names_ok() {
  [ "$(release_text_names | release_text_sha256)" = "$RELEASE_TEXT_NAMES_SHA256" ]
}

# Whether the names are left to a later check (DROPPEN_RELEASE_NAMES_DEFERRED=1,
# CI's build jobs; see the top of this file) because this one does not have
# them. Names that are set are always checked.
release_text_names_deferred() {
  [ "${DROPPEN_RELEASE_NAMES_DEFERRED:-}" = 1 ] && [ -z "${DROPPEN_RELEASE_FORBIDDEN:-}" ]
}

# Whether every pattern of DROPPEN_RELEASE_NOTICE_ALLOW is one whole copyright
# line written out: `^Copyright `, the rest of the line with every regular
# expression metacharacter escaped (`\(`, `\.`), and `$` at the end. A
# wildcard, a class or an alternation could let through lines nobody looked
# at (`^Copyright .*$`, `.*`), so none is accepted. Unset or blank: no
# exceptions, which is fine. On a refusal, says which line (not what it
# holds: the patterns name people).
release_text_allow_ok() {
  local pattern n=0 middle
  while IFS= read -r pattern; do
    n=$((n + 1))
    pattern=${pattern%$'\r'}
    [[ "$pattern" == *[![:space:]]* ]] || continue
    case "$pattern" in '^Copyright '*'$') ;; *)
      printf 'DROPPEN_RELEASE_NOTICE_ALLOW line %d is not ^Copyright …$, one whole copyright line\n' "$n" >&2
      return 1 ;;
    esac
    # What is left of the line between the anchors, with the escapes of real
    # metacharacters (`\(`, `\.`, `\\`) taken out, holds no metacharacter and
    # no backslash: any other escape (`\w`, `\S`, `\b`, `\<`) is a class or an
    # anchor to grep -E, not a character.
    middle=${pattern#^}
    middle=${middle%\$}
    middle=$(printf '%s' "$middle" | LC_ALL=C sed -E 's/\\[]()[{}*+?|^$.\\]//g')
    # (Not `[.` first in a bracket expression: that opens a collating element.
    # grep failing is a refusal too.)
    if printf '%s' "$middle" | LC_ALL=C grep -q '[]()[{}*+?|^$.\\]'; [ "${PIPESTATUS[1]}" != 1 ]; then
      printf 'DROPPEN_RELEASE_NOTICE_ALLOW line %d has an unescaped regular expression metacharacter: it must spell out one line\n' "$n" >&2
      return 1
    fi
  done <<<"${DROPPEN_RELEASE_NOTICE_ALLOW:-}"
  return 0
}

# The value of NAME in an env file such as ~/.config/droppen/release-forbidden.env,
# read as data, never run: `NAME=value` or `export NAME=value`, the value bare
# (to the end of the line) or in single quotes, over as many lines as it
# takes. Prints it; fails when the file does not set NAME.
release_text_env_value() { # file, name
  LC_ALL=C awk -v name="$2" '
    !inside && match($0, "^[[:space:]]*(export[[:space:]]+)?" name "=") {
      rest = substr($0, RLENGTH + 1)
      if (substr(rest, 1, 1) != "\047") { print rest; found = 1; exit }
      rest = substr(rest, 2)
      if ((i = index(rest, "\047")) > 0) { print substr(rest, 1, i - 1); found = 1; exit }
      value = rest; inside = 1; next
    }
    inside {
      if ((i = index($0, "\047")) > 0) { print value "\n" substr($0, 1, i - 1); found = 1; exit }
      value = value "\n" $0
    }
    END { exit !found }' "$1"
}

# DROPPEN_RELEASE_FORBIDDEN and DROPPEN_RELEASE_NOTICE_ALLOW, when the first is
# not set already, from <file> (read as data: release_text_env_value).
release_text_load_names() { # file
  [ -z "${DROPPEN_RELEASE_FORBIDDEN:-}" ] && [ -f "$1" ] || return 0
  DROPPEN_RELEASE_FORBIDDEN=$(release_text_env_value "$1" DROPPEN_RELEASE_FORBIDDEN) || DROPPEN_RELEASE_FORBIDDEN=""
  if [ -z "${DROPPEN_RELEASE_NOTICE_ALLOW:-}" ]; then
    DROPPEN_RELEASE_NOTICE_ALLOW=$(release_text_env_value "$1" DROPPEN_RELEASE_NOTICE_ALLOW) || DROPPEN_RELEASE_NOTICE_ALLOW=""
  fi
}

# The owner placeholders still in the built web app <dir>: facts only the
# owner of droppen.app can supply (the privacy notice's, the terms'), written
# `[IN CAPITALS]` until then and listed in <list> (web/owner-placeholders.txt:
# one per line, as the source writes it; blank lines and `#` comments
# skipped). Prints `<file>: <placeholder>` for each file that holds one (the
# file relative to <dir>), nothing when none is left; a list that cannot be
# read is a hit. Literal strings, so minified code never matches by chance.
# Always succeeds: the caller decides (scripts/release-public.sh refuses).
release_text_owner_placeholders() { # dir, list
  local dir=$1 list=$2 marker file
  if ! [ -f "$list" ] || ! [ -r "$list" ]; then
    printf '%s: cannot be read\n' "$list"
    return 0
  fi
  while IFS= read -r marker || [ -n "$marker" ]; do
    marker=${marker%$'\r'}
    case "$marker" in '' | '#'*) continue ;; esac
    LC_ALL=C grep -rlF -e "$marker" "$dir" 2>/dev/null | LC_ALL=C sort | while IFS= read -r file; do
      printf '%s: %s\n' "${file#"$dir"/}" "$marker"
    done || true
  done < "$list"
  return 0
}

# What is wrong with the licence and notices an app carries
# (<app>/Contents/Resources/LICENSE and THIRD_PARTY_NOTICES): missing or
# empty, a LICENSE that is not the AGPL-3.0 text, or this check's hits. Prints
# nothing when they are fine. For desktop/notarise.sh and
# desktop/updater-artifacts.sh, which ship what desktop/build.sh checked.
release_text_app_legal_problems() { # app
  local legal file hits
  for legal in LICENSE THIRD_PARTY_NOTICES; do
    file=$1/Contents/Resources/$legal
    if ! [ -s "$file" ]; then
      printf '%s: missing or empty\n' "$legal"
      continue
    fi
    if [ "$legal" = LICENSE ] && ! grep -q 'GNU AFFERO GENERAL PUBLIC LICENSE' "$file"; then
      printf 'LICENSE: not the AGPL-3.0 text\n'
    fi
    hits=$(release_text_hits "$file" "$([ "$legal" = THIRD_PARTY_NOTICES ] && echo notices)")
    [ -z "$hits" ] || printf '%s names a person it should not:\n%s\n' "$legal" "$hits"
  done
}

release_text_hits() { # file, [notices]
  if [ "${DROPPEN_RELEASE_TEXT_QUIET:-}" = 1 ]; then
    # The line numbers only (see the top of this file).
    release_text_hits_lines "$@" | sed -E 's/^([0-9]+):.*/\1: (the line is not shown: DROPPEN_RELEASE_TEXT_QUIET)/'
  else
    release_text_hits_lines "$@"
  fi
}

release_text_hits_lines() { # file, [notices]
  local file=$1 notices=${2:-} extra allowed=" "
  if ! [ -f "$file" ] || ! [ -r "$file" ]; then
    printf '%s: cannot be read\n' "$file"
    return 0
  fi
  # "Kilerd": the notices with the published crates' URLs taken out (line for
  # line, so the numbers are the file's), then every line but the copyright
  # notice.
  if [ "$notices" = notices ]; then
    sed -E "s#$NOTICE_KILERD#\\3#g" "$file"
  else
    cat "$file"
  fi | LC_ALL=C grep -niE 'kilerd' | LC_ALL=C grep -vE "^[0-9]+:${COPYRIGHT_KILERD#^}" || true
  # The operator's names, normalised (so a carriage return or a stray space
  # cannot make one miss). In the notices, first the numbers of the lines that
  # an allowed pattern matches as a whole (carriage returns and blank patterns
  # dropped).
  if [ "$notices" = notices ] && [[ "${DROPPEN_RELEASE_NOTICE_ALLOW:-}" == *[![:space:]]* ]]; then
    allowed=" $(LC_ALL=C grep -nxE -f <(printf '%s\n' "$DROPPEN_RELEASE_NOTICE_ALLOW" | LC_ALL=C tr -d '\r' | LC_ALL=C grep '[^[:space:]]') "$file" \
      | cut -d: -f1 | tr '\n' ' ' || true)"
  fi
  while IFS= read -r extra; do
    [ -n "$extra" ] || continue
    LC_ALL=C grep -niF -e "$extra" "$file" | while IFS= read -r hit; do
      case "$allowed" in *" ${hit%%:*} "*) ;; *) printf '%s\n' "$hit" ;; esac
    done || true
  done <<<"$(release_text_names)"
  return 0
}
