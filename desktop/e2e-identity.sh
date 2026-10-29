# shellcheck shell=bash
# The end-to-end test build's identity, and the guards that keep a test run
# off the real Droppen app on the same Mac. Sourced by e2e-macos.sh and
# e2e-updater-macos.sh; not run on its own.
#
# An end-to-end build (desktop/build.sh --e2e) is somebody else to macOS than
# the release throughout — bundle id, name, server instance, port range
# (desktop/core/src/identity.rs, whose tests keep the names below in step) —
# and refuses to run under the account's real HOME. So a run needs nothing of
# the real app's: it never stops it, never moves its data aside, never looks at
# its windows. What a run does outside its own folder is only this:
#
#   * the web view's data, the user defaults and the saved window state that
#     macOS keeps per bundle id under the account's real ~/Library whatever
#     $HOME says — the e2e bundle id's only ([e2e_bundle_dirs]), removed before
#     and after each run;
#   * one run at a time, as those are shared by every e2e build (a lock in /tmp).
#
# Every process a run stops it started itself, and is stopped by pid, only
# while its executable is still in the run's own folders ([e2e_signal]).

RELEASE_BUNDLE_ID=app.droppen.desktop
RELEASE_NAME=Droppen
RELEASE_INSTANCE=droppen-app
E2E_BUNDLE_ID=app.droppen.desktop.e2e
E2E_NAME="Droppen E2E"
E2E_INSTANCE=droppen-app-e2e

e2e_die() { printf '%s: %s\n' "$(basename "$0")" "$*" >&2; exit 2; }

# The account's home as the user database has it, whatever $HOME says.
E2E_ACCOUNT_HOME=$(dscl . -read "/Users/$(id -un)" NFSHomeDirectory 2>/dev/null | awk '{ print $2 }')
[ -n "$E2E_ACCOUNT_HOME" ] && [ -d "$E2E_ACCOUNT_HOME" ] || e2e_die "cannot read this account's home from the user database"
E2E_ACCOUNT_HOME=$(cd "$E2E_ACCOUNT_HOME" && pwd -P)

# A path with every link, `.` and `..` resolved (it need not exist yet).
e2e_resolve() {
  local dir base
  if [ -d "$1" ]; then (cd "$1" && pwd -P); return; fi
  dir=$(dirname "$1")
  base=$(basename "$1")
  case "$base" in . | ..) e2e_die "cannot resolve $1" ;; esac
  dir=$(e2e_resolve "$dir")
  printf '%s/%s\n' "${dir%/}" "$base"
}

# Whether $1 is $2 or inside it.
e2e_within() { case "$1/" in "$2"/*) return 0 ;; *) return 1 ;; esac; }

# What the real app (and the owner's other Droppen installs) keep: no run may
# write to, move or remove any of it.
e2e_real_places() {
  local home=$E2E_ACCOUNT_HOME
  printf '%s\n' \
    "/Applications" \
    "$home/Applications" \
    "$home/Library/Application Support/$RELEASE_BUNDLE_ID" \
    "$home/Library/Logs/$RELEASE_NAME" \
    "$home/Library/WebKit/$RELEASE_BUNDLE_ID" \
    "$home/Library/Caches/$RELEASE_BUNDLE_ID" \
    "$home/Library/HTTPStorages/$RELEASE_BUNDLE_ID" \
    "$home/Library/Preferences/$RELEASE_BUNDLE_ID.plist" \
    "$home/Library/Saved Application State/$RELEASE_BUNDLE_ID.savedState" \
    "$home/Library/LaunchAgents" \
    "$home/.local/share/remote-hand" \
    "$home/.config/remote-hand" \
    "$home/.local/share/cm-remote-hand" \
    "$home/.config/cm-remote-hand"
}

# Refuse a folder a run would write to unless it is its own: not the account's
# home or a folder above it, and nothing of the real app's.
e2e_check_scratch() { # path, what it is
  local path home=$E2E_ACCOUNT_HOME place
  path=$(e2e_resolve "$1")
  case "$path" in /*) ;; *) e2e_die "$2 is '$1', not an absolute path" ;; esac
  case "/$path/" in */./* | */../*) e2e_die "$2 is '$1', which cannot be resolved" ;; esac
  [ "$path" != / ] || e2e_die "$2 is '$1'"
  if e2e_within "$home" "$path"; then e2e_die "$2 ($path) is the account's home or a folder above it"; fi
  while IFS= read -r place; do
    if e2e_within "$path" "$place" || e2e_within "$place" "$path"; then
      e2e_die "$2 ($path) is or holds $place, the real app's"
    fi
  done < <(e2e_real_places)
}

# Refuse anything but an end-to-end test build of this identity, and one that
# is not where the real app is.
e2e_check_app() { # path to the .app
  local app=$1 id
  [ -d "$app" ] && [ -f "$app/Contents/Info.plist" ] && [ -x "$app/Contents/MacOS/Droppen" ] || e2e_die "not a Droppen app: $app"
  app=$(cd "$app" && pwd -P)
  id=$(plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist" 2>/dev/null || true)
  [ "$id" != "$RELEASE_BUNDLE_ID" ] || e2e_die "$app is the released app ($id); a run only ever starts an end-to-end test build"
  [ "$id" = "$E2E_BUNDLE_ID" ] || e2e_die "$app has the bundle id '$id', not $E2E_BUNDLE_ID: build it with desktop/build.sh --e2e from this checkout"
  LC_ALL=C grep -aqF DROPPEN_DESKTOP_TEST_CONTROL "$app/Contents/MacOS/Droppen" || e2e_die "$app is not an --e2e build"
  LC_ALL=C grep -aqF "$E2E_BUNDLE_ID" "$app/Contents/MacOS/Droppen" || e2e_die "$app was built without the end-to-end identity"
  e2e_check_scratch "$app" "the app under test"
}

# What macOS keeps per bundle id under the account's real ~/Library, for the
# e2e bundle id only: removed before a run (left from an earlier one) and after.
e2e_bundle_dirs() {
  local home=$E2E_ACCOUNT_HOME
  printf '%s\n' \
    "$home/Library/WebKit/$E2E_BUNDLE_ID" \
    "$home/Library/Caches/$E2E_BUNDLE_ID" \
    "$home/Library/HTTPStorages/$E2E_BUNDLE_ID" \
    "$home/Library/Saved Application State/$E2E_BUNDLE_ID.savedState"
}

e2e_clean_bundle_dirs() {
  local dir
  [ "$E2E_BUNDLE_ID" != "$RELEASE_BUNDLE_ID" ] || e2e_die "the end-to-end bundle id is the release's"
  while IFS= read -r dir; do
    case "$dir" in
      */"$E2E_BUNDLE_ID" | */"$E2E_BUNDLE_ID.savedState") rm -rf "$dir" ;;
      *) e2e_die "not the end-to-end build's: $dir" ;;
    esac
  done < <(e2e_bundle_dirs)
  # Its user defaults, through cfprefsd (which caches them): this domain only.
  defaults delete "$E2E_BUNDLE_ID" >/dev/null 2>&1 || true
}

# The app's single-instance socket, `<support>/app.sock`, must fit a Unix
# socket address (104 bytes with its NUL) or a second launch cannot hand over
# to the first: a run's folder name is kept short for it.
e2e_check_socket_room() { # the support directory
  local socket="$1/app.sock"
  [ "$(printf '%s' "$socket" | wc -c)" -le 103 ] || e2e_die "$socket is too long for a socket address; use a shorter folder for the run"
}

# The folders under /tmp the server falls back to for its sockets when an
# instance's own path is too long for one (`server-rs/crates/droppen-pty/src/sidecar.rs`,
# `runtime_dir`): named after a hash of the socket's folder, so those of the
# run's own instance are the run's alone. The server leaves them, emptied.
e2e_runtime_dirs() { # the run's instance directory
  local dir
  for dir in control pty-shims session-shims; do
    printf '/tmp/remote-hand-%s-%s\n' "$(id -u)" "$(printf '%s' "$1/$dir" | shasum -a 256 | cut -c1-12)"
  done
}

e2e_clean_runtime_dirs() { # the run's instance directory
  local dir
  e2e_within "$1" /tmp || e2e_within "$1" /private/tmp || e2e_die "not a run's instance: $1"
  while IFS= read -r dir; do
    [[ "$dir" =~ ^/tmp/remote-hand-[0-9]+-[0-9a-f]{12}$ ]] || e2e_die "not a runtime folder: $dir"
    rm -rf "$dir"
  done < <(e2e_runtime_dirs "$1")
}

# Take an e2e app the run started out of LaunchServices' records again (which
# keeps one for every app that ran, until it is gone), by its path only.
e2e_unregister() { # the .app
  local id
  [ -d "$1" ] || return 0
  id=$(plutil -extract CFBundleIdentifier raw "$1/Contents/Info.plist" 2>/dev/null || true)
  [ "$id" = "$E2E_BUNDLE_ID" ] || return 0
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$1" >/dev/null 2>&1 || true
}

# One run at a time: every e2e build shares the directories above.
E2E_LOCK=/tmp/droppen-e2e-run.lock
e2e_lock() {
  shlock -f "$E2E_LOCK" -p $$ || e2e_die "another end-to-end run holds $E2E_LOCK (pid $(cat "$E2E_LOCK" 2>/dev/null)); wait for it"
}
e2e_unlock() {
  if [ "$(cat "$E2E_LOCK" 2>/dev/null)" = "$$" ]; then rm -f "$E2E_LOCK"; fi
}

# Signal a pid only while its executable is inside one of the given folders —
# the run's own: a pid read from a file may have been reused since by any
# other process, the real app's included.
e2e_signal() { # signal pid folder…
  local signal=$1 pid=$2 exe folder
  shift 2
  [[ "$pid" =~ ^[0-9]+$ ]] && [ "$pid" -gt 1 ] || return 0
  exe=$(ps -o comm= -p "$pid" 2>/dev/null) || return 0
  for folder in "$@"; do
    [ -n "$folder" ] && [ "$folder" != / ] || continue
    if [[ "$exe" == "$folder"/* ]]; then
      kill "-$signal" "$pid" 2>/dev/null || true
      return 0
    fi
  done
  echo "not stopping pid $pid ($exe): not this run's" >&2
}

# The pids of processes whose executable is inside the folder.
e2e_pids_in() { # folder
  local pid exe
  [ -n "$1" ] && [ "$1" != / ] || return 0
  ps -axo pid=,comm= | while read -r pid exe; do
    if [[ "$exe" == "$1"/* ]]; then echo "$pid"; fi
  done
}
