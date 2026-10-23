#!/usr/bin/env bash
# Tests for the personal-name rule of scripts/release-public.sh's text checks
# (scripts/release-text-check.sh): `bash scripts/release-text-check.test.sh`.
# Made-up names only; nothing here builds or publishes anything. Strings that
# the public tree's leak scan forbids (the private repository, a home path) are
# put together from parts at run time, so this file never contains them.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/.." && pwd)
. "$HERE/release-text-check.sh"

TMP=$(mktemp -d "${TMPDIR:-/tmp}/release-text-check.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
unset DROPPEN_RELEASE_FORBIDDEN DROPPEN_RELEASE_NOTICE_ALLOW

FAILED=0
PASSED=0
# expect <description> <the line numbers that must be hits, space-separated>
# <file> [notices]
expect() {
  local what=$1 want=$2 got
  shift 2
  got=$(release_text_hits "$@" | sed -n 's/^\([0-9][0-9]*\):.*/\1/p' | sort -n | tr '\n' ' ' | sed 's/ $//')
  if [ "$got" = "$want" ]; then
    PASSED=$((PASSED + 1))
  else
    FAILED=$((FAILED + 1))
    printf 'FAIL: %s\n  want hits on lines: [%s]\n  got:                [%s]\n%s\n' "$what" "$want" "$got" "$(release_text_hits "$@" | sed 's/^/    /')" >&2
  fi
}
mkfile() { # name, then the file's lines
  local name=$TMP/$1
  shift
  printf '%s\n' "$@" > "$name"
  printf '%s' "$name"
}

# "Kilerd": the copyright line, and nothing else.
f=$(mkfile plain.txt \
  "Droppen is licensed under AGPL-3.0-only (see LICENSE)." \
  "Copyright (C) 2026 Kilerd" \
  "Copyright (c) 2026 Kilerd" \
  "  Copyright (C) 2026-2027 Kilerd" \
  "Droppen, copyright (C) 2026 Kilerd." \
  "Copyright (C) 2026 Kilerd." \
  "Copyright (C) 2026 Kilerd <someone@example.com>" \
  "Copyright (C) 2026 kilerd" \
  "Copyright 2026 Kilerd" \
  "source: https://github.com/Kilerd/""remote-hand" \
  "see https://github.com/Kilerd/gotcha" \
  "/Users""/KILERD/src")
expect "plain: only the bare copyright lines pass" "5 6 7 8 9 10 11 12" "$f"
expect "notices: the published crates' URLs pass too" "5 6 7 8 9 10 12" "$f" notices

f=$(mkfile notices.txt \
  "Copyright (C) 2026 Kilerd" \
  "repository: https://github.com/Kilerd/gotcha" \
  "repository: https://github.com/Kilerd/mofa.git" \
  "repository: https://github.com/Kilerd/oas/ (see above)" \
  "repository: https://github.com/Kilerd/gotcha-but-not" \
  "repository: https://github.com/Kilerd/""remote-hand.git")
expect "notices: published crates only, by exact name" "5 6" "$f" notices

f=$(mkfile clean.txt "Droppen" "MIT License" "Copyright (c) 2020 Someone Else")
expect "a clean file has no hits" "" "$f"
expect "a clean file has no hits in notices mode" "" "$f" notices
[ -n "$(release_text_hits "$TMP/missing.txt")" ] && PASSED=$((PASSED + 1)) \
  || { FAILED=$((FAILED + 1)); echo "FAIL: a missing file printed nothing" >&2; }

# The operator's names: everywhere, in any case.
export DROPPEN_RELEASE_FORBIDDEN=$'alice example\n\n-bob'
f=$(mkfile names.txt \
  "Copyright (C) 2026 Kilerd" \
  "written by ALICE EXAMPLE" \
  "nothing here" \
  "flag -bob" \
  "Copyright (c) 2020 Alice Example")
expect "forbidden names: every line, any case; an option-like name is a string" "2 4 5" "$f"
expect "forbidden names: notices without an allow-list are no different" "2 4 5" "$f" notices
export DROPPEN_RELEASE_FORBIDDEN=$'  Alice Example \r\n-BOB\r\n'
expect "forbidden names: a carriage return or stray spaces do not make one miss" "2 4 5" "$f"
export DROPPEN_RELEASE_FORBIDDEN=$'alice example\n\n-bob'

# The allow-list: whole third-party copyright lines, in the notices only.
export DROPPEN_RELEASE_NOTICE_ALLOW=$'Copyright \\(c\\) [0-9]{4} Alice Example\n\n'
expect "allow-list: not in plain files" "2 4 5" "$f"
expect "allow-list: the whole matching line passes in the notices" "2 4" "$f" notices
f=$(mkfile partial.txt \
  "Copyright (c) 2020 Alice Example and Carol" \
  "  Copyright (c) 2020 Alice Example" \
  "copyright (c) 2020 Alice Example" \
  "Copyright (c) 2020 Alice Example")
expect "allow-list: a pattern matches whole lines, case and all" "1 2 3" "$f" notices
export DROPPEN_RELEASE_NOTICE_ALLOW=$'\n   \n'
expect "allow-list: blank patterns allow nothing" "1 2 3 4" "$f" notices
export DROPPEN_RELEASE_NOTICE_ALLOW='Copyright \(c\) [0-9]{4} Alice Example and ('
expect "allow-list: a broken pattern allows nothing" "1 2 3 4" "$f" notices 2>/dev/null

# Whether the names are the expected list: release-public.sh refuses to start
# unless they are. The committed hash is of the real list; it is a sha256 and
# not that of an empty list. The cases below pin a made-up list instead.
EMPTY_SHA256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
[ "$(printf '' | release_text_sha256)" = "$EMPTY_SHA256" ] && PASSED=$((PASSED + 1)) \
  || { FAILED=$((FAILED + 1)); echo "FAIL: release_text_sha256 is not sha256" >&2; }
[[ "$RELEASE_TEXT_NAMES_SHA256" =~ ^[0-9a-f]{64}$ ]] && [ "$RELEASE_TEXT_NAMES_SHA256" != "$EMPTY_SHA256" ] \
  && PASSED=$((PASSED + 1)) || { FAILED=$((FAILED + 1)); echo "FAIL: RELEASE_TEXT_NAMES_SHA256 is not a list's sha256" >&2; }
RELEASE_TEXT_NAMES_SHA256=$(printf 'alice example\nbob\n' | release_text_sha256)
check_names() { # want (ok or refused), description, value or "unset"
  local got=ok
  if [ "$3" = unset ]; then unset DROPPEN_RELEASE_FORBIDDEN; else export DROPPEN_RELEASE_FORBIDDEN=$3; fi
  release_text_names_ok || got=refused
  if [ "$got" = "$1" ]; then PASSED=$((PASSED + 1)); else FAILED=$((FAILED + 1)); echo "FAIL: names, $2: want $1, got $got" >&2; fi
}
check_names refused "unset" unset
check_names refused "empty" ""
check_names refused "blank lines only" $'  \n\t\r\n'
check_names refused "a placeholder" "changeme"
check_names refused "a placeholder" "x"
check_names refused "a placeholder" "TODO"
check_names refused "one name of two" "alice example"
check_names refused "a misspelt name" $'alice exampel\nbob'
check_names refused "one name too many" $'alice example\nbob\ncarol'
check_names ok "the list" $'alice example\nbob'
check_names ok "the list with carriage returns" $'alice example\r\nbob\r\n'
check_names ok "the list in another order, case and spacing, with blank lines and a repeat" $'\n  BOB \n\nAlice Example\t\nbob\n'

# DROPPEN_RELEASE_NOTICE_ALLOW: whole copyright lines written out, nothing else.
check_allow() { # want (ok or refused), description, value
  local got=ok
  export DROPPEN_RELEASE_NOTICE_ALLOW=$3
  release_text_allow_ok 2>/dev/null || got=refused
  if [ "$got" = "$1" ]; then PASSED=$((PASSED + 1)); else FAILED=$((FAILED + 1)); echo "FAIL: allow, $2: want $1, got $got" >&2; fi
}
check_allow ok "unset (empty)" ""
check_allow ok "blank lines only" $'  \n\r\n'
check_allow ok "one line" '^Copyright \(c\) 2019 Alice Example$'
check_allow ok "two lines, with escapes and a carriage return" $'^Copyright \\(c\\) 2019 Alice Example \\(alice at example dot com\\)$\r\n^Copyright 2020 Bob \\[the elder\\]\\.$'
check_allow refused "not anchored at the start" 'Copyright \(c\) 2019 Alice Example$'
check_allow refused "not anchored at the end" '^Copyright \(c\) 2019 Alice Example'
check_allow refused "not a copyright line" '^Alice Example$'
check_allow refused "a wildcard" '^Copyright .*$'
check_allow refused "everything" '.*'
check_allow refused "a wildcard inside" '^Copyright \(c\) 2019 Alice.*Example$'
check_allow refused "an alternation" '^Copyright 2019 Alice|Bob$'
check_allow refused "a class" '^Copyright 2019 [A-Z]lice Example$'
check_allow refused "an unescaped dot" '^Copyright 2019 Alice Example Inc.$'
check_allow refused "a repeat" '^Copyright 2019 Alice Exampl?e$'
check_allow refused "a group" '^Copyright (c) 2019 Alice Example$'
check_allow refused "a dangling backslash" '^Copyright 2019 Alice Example\$'
check_allow refused "one good line, one bad" $'^Copyright 2019 Alice Example$\n^Copyright .*$'
check_allow ok "an escaped backslash" '^Copyright 2019 Alice\\Example$'
check_allow ok "every metacharacter escaped" '^Copyright \[\]\(\)\{\}\*\+\?\|\^\$\.\\ Alice Example$'
check_allow refused "\\S classes" '^Copyright \S\S\S\S\S\S \S\S\S\S$'
for escape in '\w' '\W' '\s' '\S' '\b' '\B' '\<' '\>' '\d' '\1' '\A'; do
  check_allow refused "the escape $escape" "^Copyright 2019 Alice${escape}Example\$"
done
check_allow refused "an escaped backslash, then a wildcard" '^Copyright 2019 Alice\\.*$'
unset DROPPEN_RELEASE_NOTICE_ALLOW

# The env file is read as data, never run.
cat >"$TMP/names.env" <<'EOF'
# the operator's lists
export DROPPEN_RELEASE_FORBIDDEN='alice example
bob'
DROPPEN_RELEASE_NOTICE_ALLOW='^Copyright \(c\) 2019 Alice Example$'
release_text_hits() { echo "replaced"; }
RELEASE_TEXT_NAMES_SHA256=0
touch "$TMP/ran"
EOF
check_value() { # description, want, got
  if [ "$3" = "$2" ]; then PASSED=$((PASSED + 1)); else FAILED=$((FAILED + 1)); printf 'FAIL: env value, %s: want [%s], got [%s]\n' "$1" "$2" "$3" >&2; fi
}
check_value "a multi-line single-quoted export" $'alice example\nbob' "$(release_text_env_value "$TMP/names.env" DROPPEN_RELEASE_FORBIDDEN)"
check_value "a one-line assignment" '^Copyright \(c\) 2019 Alice Example$' "$(release_text_env_value "$TMP/names.env" DROPPEN_RELEASE_NOTICE_ALLOW)"
release_text_env_value "$TMP/names.env" DROPPEN_NOT_THERE >/dev/null && { FAILED=$((FAILED + 1)); echo "FAIL: env value of a name the file does not set" >&2; } || PASSED=$((PASSED + 1))
SAVED_SHA256=$RELEASE_TEXT_NAMES_SHA256
unset DROPPEN_RELEASE_FORBIDDEN DROPPEN_RELEASE_NOTICE_ALLOW
release_text_load_names "$TMP/names.env"
check_value "loaded names" $'alice example\nbob' "$DROPPEN_RELEASE_FORBIDDEN"
check_value "loaded allowed lines" '^Copyright \(c\) 2019 Alice Example$' "${DROPPEN_RELEASE_NOTICE_ALLOW:-}"
check_value "the file's code did not run" "absent" "$([ -e "$TMP/ran" ] && echo ran || echo absent)"
check_value "the check is not redefined" "$SAVED_SHA256" "$RELEASE_TEXT_NAMES_SHA256"
f=$(mkfile still.txt "bob")
expect "release_text_hits is not redefined" "1" "$f"
export DROPPEN_RELEASE_FORBIDDEN="already set"
release_text_load_names "$TMP/names.env"
check_value "names already set are kept" "already set" "$DROPPEN_RELEASE_FORBIDDEN"

# An app's licence and notices, as notarise.sh and updater-artifacts.sh check
# them again.
export DROPPEN_RELEASE_FORBIDDEN="alice example"
unset DROPPEN_RELEASE_NOTICE_ALLOW
APP=$TMP/Droppen.app
mkdir -p "$APP/Contents/Resources"
check_legal() { # description, the problems' first words wanted (blank: none)
  local got
  got=$(release_text_app_legal_problems "$APP" | { grep -v '^[0-9]' || true; } | tr '\n' '|' | sed 's/|$//')
  if [ "$got" = "$2" ]; then PASSED=$((PASSED + 1)); else FAILED=$((FAILED + 1)); printf 'FAIL: app legal, %s: want [%s], got [%s]\n' "$1" "$2" "$got" >&2; fi
}
check_legal "neither file" "LICENSE: missing or empty|THIRD_PARTY_NOTICES: missing or empty"
cp "$ROOT/LICENSE" "$APP/Contents/Resources/LICENSE"
: >"$APP/Contents/Resources/THIRD_PARTY_NOTICES"
check_legal "empty notices" "THIRD_PARTY_NOTICES: missing or empty"
printf 'Copyright (C) 2026 Kilerd\nSome crate 1.0  MIT\n' >"$APP/Contents/Resources/THIRD_PARTY_NOTICES"
check_legal "both fine" ""
printf 'Some crate 1.0  MIT\nCopyright (c) 2019 Alice Example\n' >"$APP/Contents/Resources/THIRD_PARTY_NOTICES"
check_legal "a name in the notices" "THIRD_PARTY_NOTICES names a person it should not:"
export DROPPEN_RELEASE_NOTICE_ALLOW='^Copyright \(c\) 2019 Alice Example$'
check_legal "the same, allowed as that whole copyright line" ""
unset DROPPEN_RELEASE_NOTICE_ALLOW
printf 'MIT License\n' >"$APP/Contents/Resources/LICENSE"
printf 'Some crate 1.0  MIT\n' >"$APP/Contents/Resources/THIRD_PARTY_NOTICES"
check_legal "a LICENSE that is not the AGPL" "LICENSE: not the AGPL-3.0 text"

# This checkout's own text: the INSTALL templates as release-public.sh fills
# them, the LICENSE and the attributions pass with names set.
export DROPPEN_RELEASE_FORBIDDEN="alice example"
unset DROPPEN_RELEASE_NOTICE_ALLOW
for template in INSTALL.txt.in INSTALL-linux.txt.in; do
  sed -e "s/@ARCH@/x86_64/" -e "s/@CONTENTS@/contents/" "$ROOT/.github/scripts/$template" > "$TMP/$template.txt"
  expect "$template passes" "" "$TMP/$template.txt"
  grep -qx 'Copyright (C) 2026 Kilerd' "$TMP/$template.txt" && PASSED=$((PASSED + 1)) \
    || { FAILED=$((FAILED + 1)); echo "FAIL: $template has no copyright line of its own" >&2; }
done
expect "LICENSE passes" "" "$ROOT/LICENSE"
expect "attributions.txt passes as notices" "" "$ROOT/scripts/notices/attributions.txt" notices

# CI: quiet hits (the line numbers, never the lines), and names deferred by
# the build jobs, which do not have them.
export DROPPEN_RELEASE_FORBIDDEN="alice example"
f=$(mkfile quiet.txt "nothing" "written by Alice Example" "Kilerd wrote this")
export DROPPEN_RELEASE_TEXT_QUIET=1
expect "quiet: the same hits" "2 3" "$f"
check_value "quiet: no line is shown" "" "$(release_text_hits "$f" | grep -iE 'alice|kilerd' || true)"
unset DROPPEN_RELEASE_TEXT_QUIET
check_value "not quiet: the lines are shown" "1" "$(release_text_hits "$f" | grep -ic 'alice')"
check_deferred() { # want (yes or no), description
  local got=no
  release_text_names_deferred && got=yes
  if [ "$got" = "$1" ]; then PASSED=$((PASSED + 1)); else FAILED=$((FAILED + 1)); echo "FAIL: deferred, $2: want $1, got $got" >&2; fi
}
unset DROPPEN_RELEASE_FORBIDDEN DROPPEN_RELEASE_NAMES_DEFERRED
check_deferred no "neither names nor the variable"
export DROPPEN_RELEASE_NAMES_DEFERRED=yes
check_deferred no "a value other than 1"
export DROPPEN_RELEASE_NAMES_DEFERRED=1
check_deferred yes "deferred, without names"
expect "deferred: \"Kilerd\" is still checked" "3" "$f"
export DROPPEN_RELEASE_FORBIDDEN="alice example"
check_deferred no "names that are set are checked"
unset DROPPEN_RELEASE_NAMES_DEFERRED

# Owner placeholders in a built web app: each file that still holds one,
# named once per placeholder; none in a filled-in build; comments, blank
# lines and a missing final newline in the list are fine; an unreadable list
# is a hit.
mkdir -p "$TMP/dist/assets" "$TMP/clean/assets"
printf 'const a="x";const b="[BACKUP RETENTION]";\n' > "$TMP/dist/assets/privacy-abc.js"
printf '<p>[OPERATOR] and [BACKUP RETENTION]</p>\n' > "$TMP/dist/terms.html"
printf 'const b="30 days";const re=/\\[[A-Z][A-Z ]*\\]/;e[A]=1;\n' > "$TMP/clean/assets/privacy-abc.js"
printf '# a comment: [NOT A MARKER]\n\n[BACKUP RETENTION]\r\n[OPERATOR]' > "$TMP/placeholders.txt"
check_value "owner placeholders: each file that holds one" $'assets/privacy-abc.js: [BACKUP RETENTION]\nterms.html: [BACKUP RETENTION]\nterms.html: [OPERATOR]' \
  "$(release_text_owner_placeholders "$TMP/dist" "$TMP/placeholders.txt")"
check_value "owner placeholders: none in a filled-in build (a regex or an index in minified code is no placeholder)" "" \
  "$(release_text_owner_placeholders "$TMP/clean" "$TMP/placeholders.txt")"
check_value "owner placeholders: an unreadable list is a hit" "$TMP/none.txt: cannot be read" \
  "$(release_text_owner_placeholders "$TMP/clean" "$TMP/none.txt")"
check_value "owner placeholders: the repository's list reads" "" \
  "$(release_text_owner_placeholders "$TMP/clean" "$ROOT/web/owner-placeholders.txt")"

echo "release-text-check: $PASSED passed, $FAILED failed"
[ "$FAILED" = 0 ]
