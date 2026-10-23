#!/bin/sh
# Droppen server installer for macOS and Linux: https://github.com/getdroppen/droppen
#
# This installs the Droppen server, to run in the background or on a device
# with no screen. For everyday use on a Mac the Droppen desktop App, on the
# same releases page, is the recommended way.
#
#   curl -fsSL https://droppen.app/install.sh | sh                          install or upgrade
#   curl -fsSL https://droppen.app/install.sh | DROPPEN_CODE=<code> sh      ... and join your account
#   curl -fsSL https://droppen.app/install.sh | sh -s -- --code -           (the same, typing the code)
#   curl -fsSL https://droppen.app/install.sh | sh -s -- --system           Linux: a system service
#   curl -fsSL https://droppen.app/install.sh | sh -s -- --uninstall        uninstall (keeps your data)
#
# What it does, for the user who runs it:
#   1. downloads the latest release (or $DROPPEN_VERSION) for this OS and CPU from
#      github.com/getdroppen/droppen/releases and checks it against the release's SHA256SUMS;
#   2. creates the first account for the server's local page (you choose the
#      password; without a terminal one is generated and shown once, or saved
#      to a file only you can read when nobody is watching the output);
#   3. with a one-time code from my.droppen.app (my devices → add a device that
#      runs AI → show a one-time code), joins this device to your Droppen
#      account (`droppen agent enroll`), so it can be used from
#      my.droppen.app with no browser here at all;
#   4. installs the server to ~/.local/share/remote-hand/bin/remote-hand-server,
#      with the command `droppen` beside it (a link to the same file),
#      and runs it as a service on 127.0.0.1:8700 or the next free port,
#      restarted if it stops: a launchd user agent (app.droppen.server) on macOS,
#      a systemd user service (droppen.service) on Linux, or — with --system
#      and sudo — a system service that starts at boot and runs as you;
#   5. waits until its own server answers and refuses visitors without a login,
#      then opens the local page when there is a screen to open it on (macOS,
#      or a Linux desktop) and nothing to enrol.
#
# Steps 2 and 3 run the downloaded server before it replaces the installed
# one, so a step that fails leaves the device as it was. Running it again
# upgrades in place and keeps your data. It never touches a Droppen
# (remote-hand) server it did not install itself: it lists what it found and
# leaves it alone. It does not run as root: the server runs AI agents and
# shells as the user it runs as.
#
# The code is a one-time secret. Give it in DROPPEN_CODE, or type it after
# `--code -`; it is never accepted on the command line, which other users of a
# shared device can read. It reaches the server on standard input, never on a
# command line either.
#
# Settings, all optional:
#   DROPPEN_CODE=<code>        join the account the code is from
#   DROPPEN_HUB=wss://...      the Droppen hub to join (default wss://droppen.app)
#   DROPPEN_VERSION=v0.2.120   install this release instead of the latest
#   DROPPEN_PORT=8700          the first port to try on a new install
#   DROPPEN_USER=name          the first local account's name (default: your user name)
#   DROPPEN_NO_OPEN=1          do not open the browser
#   DROPPEN_SERVICE_PATH=...   the PATH the server runs with (default: your
#                              current PATH, then common tool directories)
#   DROPPEN_RELEASE_BASE=...   where releases come from
#                              (default https://github.com/getdroppen/droppen/releases)
#   DROPPEN_LABEL=...          the launchd label or systemd unit name
#                              (default app.droppen.server / droppen)
#
# The command is `droppen` (<data>/bin/droppen, also on the release as
# droppen-<version>-<os>-<arch>.tar.gz). The binary the service runs, its data
# and its log directories keep the remote-hand name, so an existing install,
# its launchd job or systemd unit and its self-updates carry on unchanged.

set -eu

RELEASES_URL=https://github.com/getdroppen/droppen
INSTALL_CMD="curl -fsSL https://droppen.app/install.sh | sh"
BASE=${DROPPEN_RELEASE_BASE:-$RELEASES_URL/releases}
HUB=${DROPPEN_HUB:-wss://droppen.app}
# The instance this installer owns: `default`, the name a server takes when
# nothing says otherwise. Two servers on one device need different instances.
INSTANCE=default
# The binary the service runs and the supervisor, self-update and rollback
# manage: never renamed.
BIN_NAME=remote-hand-server
# The command people type: a link to it, beside it.
CMD_NAME=droppen
# The oldest glibc the Linux builds run on.
GLIBC_MIN=2.28
# Where my.droppen.app shows a one-time code.
WHERE_CODES="my devices → add a device that runs AI → show a one-time code"

say() { printf '%s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf '\nerror: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Droppen server installer for macOS and Linux, for background and headless use.
On a Mac, the Droppen desktop App is the recommended way for everyday use:
https://github.com/getdroppen/droppen/releases/latest

  curl -fsSL https://droppen.app/install.sh | sh                        install or upgrade
  curl -fsSL https://droppen.app/install.sh | DROPPEN_CODE=<code> sh    ... and join your account
  curl -fsSL https://droppen.app/install.sh | sh -s -- --code -         (the same, typing the code)
  curl -fsSL https://droppen.app/install.sh | sh -s -- --system         Linux: a system service (sudo)
  curl -fsSL https://droppen.app/install.sh | sh -s -- --uninstall      uninstall (keeps your data)

Options:
  --code -      ask for the one-time code on the terminal, without echo
  --system      Linux: a system service that starts at boot and runs as you
  --uninstall   stop and remove the service and the server; keep the data
  -h, --help    this text

Settings (environment):
  DROPPEN_CODE=<code>       join the account the code is from
                            (my.droppen.app: my devices → add a device that runs AI
                            → show a one-time code)
  DROPPEN_HUB=wss://...     the Droppen hub to join (default wss://droppen.app)
  DROPPEN_VERSION=v...      install this release instead of the latest
  DROPPEN_PORT=8700         the first port to try on a new install
  DROPPEN_USER=name         the first local account's name
  DROPPEN_NO_OPEN=1         do not open the browser
  DROPPEN_SERVICE_PATH=...  the PATH the server runs with
  DROPPEN_RELEASE_BASE=...  where releases come from
  DROPPEN_LABEL=...         the launchd label or systemd unit name

More: https://github.com/getdroppen/droppen#readme
EOF
}

# ---------------------------------------------------------------- places

set_paths() {
  case "${HOME:-}" in
    /*) ;;
    *) die "HOME is not set to an absolute path" ;;
  esac
  DATA_DIR=$HOME/.local/share/remote-hand
  CONFIG_DIR=$HOME/.config/remote-hand
  BIN_DIR=$DATA_DIR/bin
  BIN=$BIN_DIR/$BIN_NAME
  CMD=$BIN_DIR/$CMD_NAME
  # The binary the steps run: the downloaded one until it is installed.
  RUN_BIN=$BIN
  # What this installer writes to recognise its own install on the next run.
  MARKER=$CONFIG_DIR/droppen-install.env
  LOCK=$DATA_DIR/instances/$INSTANCE/lock
  ME=$(id -un)
  case "$(uname -s)" in
    Darwin)
      OS=macos
      LABEL=${DROPPEN_LABEL:-app.droppen.server}
      SERVICE=launchd
      PLIST=$HOME/Library/LaunchAgents/$LABEL.plist
      LOG_DIR=$HOME/Library/Logs/remote-hand
      DOMAIN=gui/$(id -u)
      ;;
    Linux)
      OS=linux
      LABEL=${DROPPEN_LABEL:-droppen}
      USER_UNIT_DIR=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
      LOG_DIR=${XDG_STATE_HOME:-$HOME/.local/state}/remote-hand
      ;;
    *) OS=other ;;
  esac
  # A launchd label and a systemd unit name: no spaces, slashes or quotes.
  printf '%s\n' "${LABEL:-x}" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9:_.@-]*$' || die "DROPPEN_LABEL may hold only letters, digits and : _ . @ -"
}

marker_get() { # key
  [ -f "$MARKER" ] || return 0
  sed -n "s/^$1=//p" "$MARKER" | head -n 1
}

# This installer made this data directory (and still owns it after an
# uninstall, which keeps the data): the marker says so.
ours() { [ -f "$MARKER" ]; }

# The server binary, run the way the service runs it: from $HOME, with nothing
# of this shell's environment (an APP_INSTANCE or APP_HOME_DIR here must not
# redirect it).
run_bin() {
  (cd "$HOME" && env -i HOME="$HOME" PATH=/usr/bin:/bin:/usr/sbin:/sbin LANG="${LANG:-C.UTF-8}" "$RUN_BIN" "$@")
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'; else shasum -a 256 "$1" | awk '{ print $1 }'; fi
}

has_display() {
  [ "$OS" = macos ] && return 0
  [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]
}

# curl for the release files: https only, TLS 1.2 or newer, unless
# DROPPEN_RELEASE_BASE points at a plain-http test server on purpose.
fetch() {
  case "$BASE" in
    https://*) curl -fsSL --proto '=https' --tlsv1.2 --retry 3 "$@" ;;
    *) curl -fsSL --retry 3 "$@" ;;
  esac
}

# ---------------------------------------------------------------- checks

check_platform() {
  case "$OS" in
    macos)
      case "$(uname -m)" in
        arm64) ARCH=aarch64 ;;
        x86_64)
          # A shell under Rosetta reports x86_64 on Apple silicon; install the native one.
          if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then ARCH=aarch64; else ARCH=x86_64; fi ;;
        *) die "unsupported Mac architecture: $(uname -m)" ;;
      esac
      for tool in curl shasum tar cmp launchctl plutil sed awk; do
        command -v "$tool" >/dev/null 2>&1 || die "missing $tool"
      done ;;
    linux)
      case "$(uname -m)" in
        x86_64 | amd64) ARCH=x86_64 ;;
        aarch64 | arm64) ARCH=aarch64 ;;
        *) die "unsupported Linux architecture: $(uname -m) (x86_64 and aarch64 are supported)" ;;
      esac
      for tool in curl tar cmp sed awk ps; do
        command -v "$tool" >/dev/null 2>&1 || die "missing $tool"
      done
      command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 || die "missing sha256sum"
      check_glibc ;;
    *) die "this installer supports macOS and Linux (found $(uname -s))" ;;
  esac
  [ "$(id -u)" -ne 0 ] || die "run this as the user Droppen should run as, not as root: it runs AI agents and shells as that user (for a boot-time service, use --system, which asks sudo for the service file only)"
}

check_glibc() {
  have=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{ print $2 }' || true)
  if [ -z "$have" ]; then
    die "this Linux has no glibc (musl, as on Alpine?); the Linux builds need glibc $GLIBC_MIN or newer"
  fi
  newest=$(printf '%s\n%s\n' "$GLIBC_MIN" "$have" | sort -t. -k1,1n -k2,2n | tail -n 1)
  [ "$newest" = "$have" ] || die "glibc $have is too old; Droppen needs glibc $GLIBC_MIN or newer (Debian 10, Ubuntu 20.04, RHEL 8 or later)"
}

user_bus() { command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; }

no_user_bus() { # what was being attempted
  die "$1: this needs your systemd user manager, which this session cannot reach (no XDG_RUNTIME_DIR or user bus). Run it from a login session: an SSH login, or 'machinectl shell $ME@' from root, not 'sudo -u' or 'su'. Nothing was changed."
}

choose_service() {
  if [ "$OS" != linux ]; then
    [ "$SYSTEM" = 0 ] || die "--system is for Linux; on a Mac the server runs as a launchd user agent"
    return 0
  fi
  PREVIOUS=$(marker_get service)
  case "$PREVIOUS" in
    systemd-user)
      # Ours, but only reachable through the user manager: never mistake that
      # for somebody else's install, and never start a second one beside it.
      [ "$SYSTEM" = 0 ] || die "Droppen is installed here as a systemd user service; uninstall it first (from a login session) to switch to --system"
      user_bus || no_user_bus "Droppen is installed here as the systemd user service $(marker_get label).service"
      SERVICE=systemd-user
      LABEL=$(marker_get label)
      UNIT_FILE=$USER_UNIT_DIR/$LABEL.service ;;
    systemd-system)
      SERVICE=systemd-system
      LABEL=$(marker_get label)
      UNIT_FILE=/etc/systemd/system/$LABEL.service
      command -v sudo >/dev/null 2>&1 || die "Droppen is installed here as a system service; updating it needs sudo" ;;
    *)
      if [ "$SYSTEM" = 1 ]; then
        command -v systemctl >/dev/null 2>&1 || die "--system needs systemd"
        command -v sudo >/dev/null 2>&1 || die "--system needs sudo, to write the service file"
        SERVICE=systemd-system
        LABEL=${DROPPEN_LABEL:-droppen-$ME}
        UNIT_FILE=/etc/systemd/system/$LABEL.service
      elif user_bus; then
        SERVICE=systemd-user
        UNIT_FILE=$USER_UNIT_DIR/$LABEL.service
      else
        SERVICE=none
      fi ;;
  esac
  printf '%s\n' "$LABEL" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9:_.@-]*$' || die "the unit name $LABEL is not one systemd takes"
}

# Every other Droppen (remote-hand) server this user has: launchd jobs and
# agent plists on macOS, systemd units on Linux. Sets OTHERS (a printable list)
# and CONFLICT (why this install would touch one of them; empty when not).
find_others() {
  OTHERS=""
  CONFLICT=""
  if [ "$OS" = macos ]; then
    for job in $(launchctl list 2>/dev/null | awk 'NR > 1 { print $3 }' | grep -iE 'remote-hand|droppen' || true); do
      [ "$job" = "$LABEL" ] && continue
      info=$(launchctl print "$DOMAIN/$job" 2>/dev/null || true)
      program=$(printf '%s\n' "$info" | sed -n 's/^[[:space:]]*program = //p' | head -n 1)
      port=$(printf '%s\n' "$info" | sed -n 's/^[[:space:]]*APP_SERVER__PORT => //p' | head -n 1)
      OTHERS="$OTHERS
    $job (launchd, port ${port:-8700}, ${program:-unknown program})"
      if [ "$program" = "$BIN" ]; then
        CONFLICT="the launchd job $job runs $BIN, which this installer would replace"
      fi
    done
    unit_files=$(ls "$HOME"/Library/LaunchAgents/*.plist 2>/dev/null || true)
    ours_file=$PLIST
  else
    unit_files=$(ls "$USER_UNIT_DIR"/*.service /etc/systemd/system/*.service 2>/dev/null || true)
    ours_file=${UNIT_FILE:-}
  fi
  for file in $unit_files; do
    [ -f "$file" ] || continue
    [ "$file" = "$ours_file" ] && continue
    grep -qE 'remote-hand|droppen' "$file" 2>/dev/null || continue
    job=$(basename "$file")
    job=${job%.plist}
    case "$OTHERS" in *"    $job "*) ;; *) OTHERS="$OTHERS
    $job ($file)" ;; esac
    if grep -qF "$BIN" "$file"; then
      CONFLICT="$file runs $BIN, which this installer would replace"
    fi
  done
  if [ -z "$CONFLICT" ] && ! ours; then
    if [ -e "$BIN" ]; then
      CONFLICT="$BIN exists, but this installer did not put it there"
    elif [ -d "$DATA_DIR/instances" ]; then
      CONFLICT="$DATA_DIR already holds the data of a Droppen server this installer did not create"
    fi
  fi
}

report_others() {
  [ -n "$OTHERS" ] || return 0
  say ""
  say "Other Droppen (remote-hand) servers here, left alone:$OTHERS"
}

refuse_conflict() {
  report_others
  say ""
  say "Found an existing install: $CONFLICT."
  say "发现已有的安装，本脚本不会改动它。"
  say "Nothing was changed. This installer only manages a server it installed itself"
  say "($LABEL). Keep using the existing one, or remove it first."
  exit 1
}

# ---------------------------------------------------------------- download

resolve_version() {
  if [ -n "${DROPPEN_VERSION:-}" ]; then
    VERSION=$DROPPEN_VERSION
    case "$VERSION" in v*) ;; *) VERSION=v$VERSION ;; esac
  else
    # GitHub answers /releases/latest with a redirect to /releases/tag/<tag>.
    url=$(fetch -I -o /dev/null -w '%{url_effective}' "$BASE/latest") \
      || die "could not reach $BASE/latest to find the latest release"
    VERSION=${url##*/tag/}
    [ "$VERSION" != "$url" ] || die "could not find the latest release at $BASE/latest"
  fi
  printf '%s\n' "$VERSION" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' \
    || die "not a release version: $VERSION"
}

# The SHA-256 SHA256SUMS lists for a file, or nothing. Lines are
# `<sha256>  ./<file>` (or without the ./, or with a * for binary mode).
listed_sha256() { # file
  awk -v f="$1" '{ n = $2; sub(/^\*/, "", n); sub(/^\.\//, "", n); if (n == f) { print $1; exit } }' "$TMP/SHA256SUMS"
}

download_and_verify() {
  case "$BASE" in
    https://*) ;;
    *) warn "downloading over plain HTTP from $BASE (DROPPEN_RELEASE_BASE)" ;;
  esac
  step "Downloading Droppen $VERSION for $OS $ARCH"
  fetch -o "$TMP/SHA256SUMS" "$BASE/download/$VERSION/SHA256SUMS" \
    || die "could not download $BASE/download/$VERSION/SHA256SUMS"

  # The release's tarball under its new name, holding `droppen` (and the same
  # file as `remote-hand-server`); else, for a release from before it had one
  # or a download that fails, the one under the old name, holding
  # `remote-hand-server`. Only a file SHA256SUMS lists is downloaded.
  ASSET=""
  tried=""
  for candidate in "$CMD_NAME:$CMD_NAME-$VERSION-$OS-$ARCH.tar.gz" "$BIN_NAME:remote-hand-$VERSION-$OS-$ARCH.tar.gz"; do
    member=${candidate%%:*}
    file=${candidate#*:}
    tried="$tried${tried:+ or }$file"
    expected=$(listed_sha256 "$file")
    printf '%s\n' "$expected" | grep -Eq '^[0-9a-f]{64}$' || continue
    if fetch -o "$TMP/$file" "$BASE/download/$VERSION/$file"; then
      ASSET=$file
      MEMBER=$member
      break
    fi
    warn "could not download $BASE/download/$VERSION/$file"
  done
  [ -n "$ASSET" ] || die "SHA256SUMS of $VERSION lists no $tried that could be downloaded; nothing was installed"
  actual=$(sha256_of "$TMP/$ASSET")
  [ "$actual" = "$expected" ] || die "checksum mismatch for $ASSET (expected $expected, got $actual); nothing was installed"
  say "SHA-256 verified: $actual ($ASSET)"

  mkdir "$TMP/x"
  tar -xzf "$TMP/$ASSET" -C "$TMP/x" "$MEMBER" || die "$ASSET does not contain $MEMBER"
  # Run from beside the installed binary, not from $TMP: a /tmp mounted
  # noexec runs nothing. Not a .staging-* name, which a starting server sweeps.
  mkdir -p "$BIN_DIR"
  NEW=$BIN_DIR/.installer-new
  STAGED=$NEW
  cp "$TMP/x/$MEMBER" "$NEW"
  chmod 755 "$NEW"
  [ "$OS" != macos ] || xattr -d com.apple.quarantine "$NEW" 2>/dev/null || true
  NEW_VERSION=$(env -i PATH=/usr/bin:/bin "$NEW" --version 2>/dev/null) || die "the downloaded server does not run on this device"
}

# ---------------------------------------------------------------- install

# Something is listening on 127.0.0.1:<port> unless curl cannot connect at all (7).
port_busy() { # port
  rc=0
  curl -s -o /dev/null --max-time 2 "http://127.0.0.1:$1/" || rc=$?
  [ "$rc" -ne 7 ]
}

choose_port() {
  PORT=$(marker_get port)
  if [ -n "$PORT" ] && { [ -z "${DROPPEN_PORT:-}" ] || [ "$DROPPEN_PORT" = "$PORT" ]; }; then
    return 0 # keep the address the browser already knows
  fi
  first=${DROPPEN_PORT:-8700}
  case "$first" in '' | *[!0-9]*) die "DROPPEN_PORT must be a number" ;; esac
  PORT=$first
  while port_busy "$PORT"; do
    PORT=$((PORT + 1))
    [ "$PORT" -lt $((first + 50)) ] || die "no free port between $first and $PORT"
  done
}

# Make the downloaded binary the installed one. A changed binary leaves
# `restart=1` in the marker until the service has restarted into it, so a run
# that stops in between still restarts on the next one.
install_binary() {
  mkdir -p "$BIN_DIR"
  if [ -f "$BIN" ] && cmp -s "$NEW" "$BIN"; then
    say "Already installed: $NEW_VERSION"
    return 0
  fi
  OLD_VERSION=""
  if [ -f "$BIN" ]; then
    OLD_VERSION=$(env -i PATH=/usr/bin:/bin "$BIN" --version 2>/dev/null || echo unknown)
    # Keep the one being replaced as .prev, the supervisor's rollback.
    cp -p "$BIN" "$BIN_DIR/.installer-prev"
    mv -f "$BIN_DIR/.installer-prev" "$BIN.prev"
  fi
  RESTART_PENDING=1
  write_marker
  # Staged beside it, then renamed: the installed path is never half written.
  mv -f "$NEW" "$BIN"
  if [ -n "$OLD_VERSION" ]; then
    say "Upgraded $OLD_VERSION -> $NEW_VERSION"
  else
    say "Installed $NEW_VERSION to $BIN"
  fi
}

# Whether `droppen` beside the installed binary is the link this installer makes.
our_link() { [ -L "$CMD" ] && [ "$(readlink "$CMD")" = "$BIN_NAME" ]; }

# `droppen` beside the installed binary: a relative link, so it follows every
# upgrade (the installer's, or a self-update's rename) and the directory if it
# moves. Never over anything else already there, a link included.
link_command() {
  our_link && return 0
  if [ -e "$CMD" ] || [ -L "$CMD" ]; then
    warn "$CMD is not the link this installer makes, so it was left alone; run $BIN instead"
    CMD=$BIN
    return 0
  fi
  ln -sf "$BIN_NAME" "$BIN_DIR/.installer-link"
  mv -f "$BIN_DIR/.installer-link" "$CMD"
}

default_user() {
  name=${DROPPEN_USER:-}
  [ -n "$name" ] || name=$(printf '%s' "$ME" | LC_ALL=C tr '[:upper:]' '[:lower:]')
  if printf '%s\n' "$name" | grep -Eq '^[a-z0-9._-]{1,32}$'; then
    printf '%s\n' "$name"
  elif [ -n "${DROPPEN_USER:-}" ]; then
    die "DROPPEN_USER must be 1-32 characters of a-z, 0-9, '.', '_' or '-'"
  else
    printf 'droppen\n'
  fi
}

# What a failed step leaves: the server as it was, running or not.
nothing_changed() {
  if service_running; then
    printf 'Nothing was changed: the Droppen server already here keeps running as before.'
  else
    printf 'Droppen was not started.'
  fi
}

# The local page has its own login (accounts of this server, kept in its
# instance). Enrolling refuses an instance without one, and the page would
# otherwise wait in its setup state, so the first one is made before the
# server is ever started.
ensure_account() {
  step "Local login"
  listing=$(run_bin user list --instance "$INSTANCE") || die "could not read this server's accounts. $(nothing_changed)"
  case "$listing" in
    "no accounts"*) ;;
    *)
      USERS=$(printf '%s\n' "$listing" | sed 1d | cut -f1 | tr '\n' ' ' | sed 's/ $//')
      say "Sign in with your existing account: $USERS"
      ACCOUNT_NOTE="your existing account ($USERS)"
      return 0 ;;
  esac
  USER_NAME=$(default_user)
  if (: </dev/tty) 2>/dev/null; then
    say "Choose a password for this server's local page (user: $USER_NAME, at least 12 characters)."
    say "为这台设备上的 Droppen 本地页面设置密码（用户名 ${USER_NAME}，至少 12 个字符）。"
    tries=0
    until run_bin user add "$USER_NAME" --instance "$INSTANCE" </dev/tty; do
      tries=$((tries + 1))
      [ "$tries" -lt 3 ] || die "no account was created (the page would wait in its setup state, and adding this device to Droppen needs one). $(nothing_changed) Run the installer again."
      say "Try again. / 请重试。"
    done
    ACCOUNT_NOTE="$USER_NAME, with the password you just chose"
    return 0
  fi
  # No terminal (an unattended run): generate one.
  password=$(LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c 24)
  [ "${#password}" -eq 24 ] || die "could not generate a password"
  printf '%s\n' "$password" | run_bin user add "$USER_NAME" --instance "$INSTANCE" >/dev/null \
    || die "could not create the first account. $(nothing_changed)"
  if [ -t 1 ]; then
    say "No terminal to ask for a password on, so one was generated. It is shown only this once:"
    say ""
    say "    user:     $USER_NAME"
    say "    password: $password"
    say ""
    ACCOUNT_NOTE="$USER_NAME, with the generated password above"
  else
    # Output that goes to a log is not the place for a password.
    mkdir -p "$CONFIG_DIR"
    (umask 077 && printf 'user: %s\npassword: %s\n' "$USER_NAME" "$password" > "$CONFIG_DIR/initial-password")
    say "No terminal to ask for a password on, so one was generated and saved, readable only by you, in"
    say "    $CONFIG_DIR/initial-password"
    say "Read it, keep it somewhere safe, and delete that file."
    ACCOUNT_NOTE="$USER_NAME, with the password in $CONFIG_DIR/initial-password"
  fi
  say "Change it later with: $CMD user passwd $USER_NAME --instance $INSTANCE"
}

# `instance default: enrolled`, `...: not enrolled (...)`, `...: enrolling (...)`.
agent_standing() {
  run_bin agent status --instance "$INSTANCE" 2>/dev/null | head -n 1 | sed -n 's/^instance [^:]*: //p'
}

# Join the account the one-time code is from: the existing CLI enrolment, with
# the code on stdin. It waits until the my.droppen.app page that showed the code
# adds this device, which it does by itself; up to the code's ten minutes.
enroll() {
  ENROLLED=""
  standing=$(agent_standing)
  case "$standing" in
    enrolled*)
      ENROLLED=1
      [ -z "$CODE" ] || say "This device is already in a Droppen account; the code was not used."
      return 0 ;;
  esac
  if [ -z "$CODE" ]; then
    case "$standing" in
      enrolling*) warn "this device is still waiting to join an account with an earlier code; to try again, get a new code ($WHERE_CODES) and run the installer with DROPPEN_CODE" ;;
    esac
    return 0
  fi
  step "Joining your Droppen account"
  HALTED=""
  case "$standing" in
    enrolling*)
      # A running server follows the earlier attempt and would write it back:
      # stop it, then this code replaces that attempt.
      if service_running; then
        service_halt
        HALTED=1
        say "Stopped the server, which was waiting on an earlier code."
      fi ;;
  esac
  say "Keep the my.droppen.app page that showed the code open: it adds this device by itself. Waiting up to 10 minutes..."
  say "请保持显示该代码的 my.droppen.app 页面打开，它会自动添加这台设备（最多等待 10 分钟）。"
  # The command's exit status says how it ended (`agent enroll --help`): 0 the
  # device is added, 76 the hub took the code and the page has not added the
  # device yet, anything else the code did not take. The state on disk cannot
  # tell the last two apart: a refused code leaves an earlier attempt as it was.
  rc=0
  printf '%s\n' "$CODE" | run_bin agent enroll - --hub "$HUB" --instance "$INSTANCE" || rc=$?
  case "$rc" in
    0)
      ENROLLED=1
      ENROLLED_NOW=1
      say "Joined. This device is now in your Droppen account." ;;
    76)
      warn "the page has not added this device yet: if it is still open, the server finishes joining by itself once it does"
      ENROLLED_NOW=1 ;;
    *)
      resume_halted
      why="the message above says why"
      [ "$rc" -le 128 ] || why="it was stopped by signal $((rc - 128))"
      die "this device did not join the account with that code (exit $rc; $why). $(nothing_changed) Get a new code ($WHERE_CODES) and run the installer again." ;;
  esac
}

# The server stopped for a new code, started again as it was: after a code
# that did not take, or a stop (Ctrl-C) before the new server is started.
resume_halted() {
  [ -n "${HALTED:-}" ] || return 0
  HALTED=""
  service_resume
}

xml() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

service_path() {
  if [ -n "${DROPPEN_SERVICE_PATH:-}" ]; then
    printf '%s\n' "$DROPPEN_SERVICE_PATH"
    return 0
  fi
  # A service does not read your shell profile: without this, `claude`,
  # `codex`, `node` and `git` would be missing from every session it starts.
  # Your PATH first, so the server runs the `node` and `claude` your terminal
  # runs (nvm's, say, rather than Homebrew's); then the usual places, as a
  # fallback, whether or not they exist yet. Each directory once, where it
  # first appears. The desktop app builds its server's PATH the same way
  # (desktop/core/src/env_path.rs).
  path=""
  old_ifs=$IFS
  IFS=:
  set -f
  for dir in ${PATH:-}; do
    case "$dir" in
      /*) ;;
      *) continue ;;
    esac
    case ":$path:" in *":$dir:"*) continue ;; esac
    case "$dir" in */remote-hand/instances/*) continue ;; esac # a session's own bin dir
    [ -d "$dir" ] && path="${path:+$path:}$dir"
  done
  for dir in "$HOME/.local/bin" "$HOME/.npm-global/bin" "$HOME/.cargo/bin" "$HOME/.bun/bin" "$HOME/.volta/bin" \
    /opt/homebrew/bin /usr/local/bin /usr/bin /bin /usr/sbin /sbin; do
    case ":$path:" in *":$dir:"*) continue ;; esac
    path="${path:+$path:}$dir"
  done
  set +f
  IFS=$old_ifs
  printf '%s\n' "$path"
}

# Written before anything is installed, so a run that stops halfway (a
# password refused three times, a closed terminal) is still recognised as ours.
# `service` is empty once uninstalled; `restart=1` until the service has
# restarted into a newly installed binary.
write_marker() {
  mkdir -p "$CONFIG_DIR"
  cat > "$MARKER" <<EOF
# Written by the Droppen installer ($RELEASES_URL), which reads it to
# recognise its own install. Deleting it makes the installer treat the server
# below as somebody else's.
label=$LABEL
service=$MARKER_SERVICE
instance=$INSTANCE
port=$PORT
version=$VERSION
restart=$RESTART_PENDING
EOF
}

# ------------------------------------------------ services: macOS (launchd)

launchd_loaded() { launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; }

# The server's logs, readable by this user alone: they say where the setup
# code is and what the server did, which is nobody else's business on a shared
# device. Made before anything writes them, so they never exist wider.
private_logs() {
  mkdir -p "$LOG_DIR"
  chmod 700 "$LOG_DIR" 2>/dev/null || true
  for log in "$@"; do
    (umask 077 && : >>"$log")
    chmod 600 "$log" 2>/dev/null || true
  done
}

launchd_write() {
  mkdir -p "$HOME/Library/LaunchAgents"
  private_logs "$LOG_DIR/$INSTANCE.log" "$LOG_DIR/$INSTANCE.err.log"
  cat > "$TMP/service" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Droppen server, written by $(xml "$INSTALL_CMD"). Rerun it to upgrade. -->
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$(xml "$LABEL")</string>
    <!-- The supervisor: restarts into self-updates, rolls back a binary that will not stay up. -->
    <key>ProgramArguments</key>
    <array>
        <string>$(xml "$BIN")</string>
        <string>up</string>
        <string>--</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <!-- Terminals and agent runs outlive a server restart; launchd must not kill them with it. -->
    <key>AbandonProcessGroup</key>
    <true/>
    <key>WorkingDirectory</key>
    <string>$(xml "$HOME")</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>$(xml "$HOME")</string>
        <!-- Loopback only. Reach it from elsewhere through Droppen, an SSH tunnel or Tailscale. -->
        <key>APP_SERVER__HOST</key>
        <string>127.0.0.1</string>
        <key>APP_SERVER__PORT</key>
        <string>$PORT</string>
        <key>APP_INSTANCE</key>
        <string>$INSTANCE</string>
        <key>LANG</key>
        <string>$(xml "${LANG:-en_US.UTF-8}")</string>
        <key>PATH</key>
        <string>$(xml "$(service_path)")</string>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>StandardOutPath</key>
    <string>$(xml "$LOG_DIR/$INSTANCE.log")</string>
    <key>StandardErrorPath</key>
    <string>$(xml "$LOG_DIR/$INSTANCE.err.log")</string>
</dict>
</plist>
EOF
  plutil -lint "$TMP/service" >/dev/null || die "the launchd agent file did not come out valid"
  if [ -f "$PLIST" ] && cmp -s "$TMP/service" "$PLIST"; then
    SERVICE_CHANGED=0
  else
    SERVICE_CHANGED=1
    cp "$TMP/service" "$PLIST"
  fi
}

launchd_wait_gone() {
  i=0
  while launchd_loaded; do
    i=$((i + 1))
    [ "$i" -lt 100 ] || die "launchd did not stop $LABEL"
    sleep 0.1
  done
}

launchd_start() {
  launchctl enable "$DOMAIN/$LABEL" 2>/dev/null || true
  if launchd_loaded; then
    if [ "$SERVICE_CHANGED" = 1 ]; then
      launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
      launchd_wait_gone
      launchctl bootstrap "$DOMAIN" "$PLIST" || die "launchctl bootstrap failed for $PLIST"
      launchctl kickstart "$DOMAIN/$LABEL" 2>/dev/null || true
      say "Restarted $LABEL with its new settings"
    elif [ "$RESTART" = 1 ]; then
      launchctl kickstart -k "$DOMAIN/$LABEL" || die "launchctl kickstart failed for $LABEL"
      say "Restarted $LABEL"
    else
      launchctl kickstart "$DOMAIN/$LABEL" 2>/dev/null || true
      say "$LABEL is running"
    fi
  else
    launchctl bootstrap "$DOMAIN" "$PLIST" || die "launchctl bootstrap failed for $PLIST"
    # RunAtLoad may be held back as a "speculative" spawn on a busy Mac: start it now.
    launchctl kickstart "$DOMAIN/$LABEL" 2>/dev/null || true
    say "Started $LABEL (starts at login, restarts if it stops)"
  fi
}

launchd_stop() {
  if launchd_loaded; then
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    launchd_wait_gone
    say "Stopped $LABEL"
  fi
  rm -f "$PLIST"
}

# ------------------------------------------------ services: Linux (systemd)

# Systemd quoting: one double-quoted word, with \ and " escaped and % as %%.
unit_quote() { printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/%/%%/g')"; }
# ExecStart= also expands $VAR, so a $ there is $$; Environment= takes it as it is.
unit_exec_quote() { unit_quote "$(printf '%s' "$1" | sed -e 's/\$/$$/g')"; }
unit_env() { printf 'Environment=%s\n' "$(unit_quote "$1=$2")"; }

systemd_unit() { # wanted-by, extra [Service] lines
  cat <<EOF
# Droppen server, written by $INSTALL_CMD. Rerun it to upgrade.
[Unit]
Description=Droppen server (remote-hand-server, instance $INSTANCE)
Documentation=$RELEASES_URL

[Service]
Type=simple
$2
WorkingDirectory=~
ExecStart=$(unit_exec_quote "$BIN") up -- serve
$(unit_env HOME "$HOME")
$(unit_env APP_SERVER__HOST 127.0.0.1)
$(unit_env APP_SERVER__PORT "$PORT")
$(unit_env APP_INSTANCE "$INSTANCE")
$(unit_env LANG "${LANG:-C.UTF-8}")
$(unit_env PATH "$(service_path)")
# The supervisor ("up") restarts into self-updates and forwards a stop to the
# server only: terminals and agent runs it started outlive a restart, so
# systemd must not kill them with it.
KillMode=process
Restart=always
RestartSec=2

[Install]
WantedBy=$1
EOF
}

sctl() { # systemctl for the service's scope
  if [ "$SERVICE" = systemd-system ]; then sudo systemctl "$@"; else systemctl --user "$@"; fi
}

systemd_write() {
  if [ "$SERVICE" = systemd-system ]; then
    systemd_unit multi-user.target "User=$ME
Group=$(id -gn)" > "$TMP/service"
    if sudo test -f "$UNIT_FILE" && sudo cmp -s "$TMP/service" "$UNIT_FILE"; then
      SERVICE_CHANGED=0
    else
      SERVICE_CHANGED=1
      say "Writing $UNIT_FILE (sudo)"
      sudo install -m 0644 "$TMP/service" "$UNIT_FILE" || die "could not write $UNIT_FILE"
    fi
  else
    mkdir -p "$USER_UNIT_DIR"
    systemd_unit default.target "" > "$TMP/service"
    if [ -f "$UNIT_FILE" ] && cmp -s "$TMP/service" "$UNIT_FILE"; then
      SERVICE_CHANGED=0
    else
      SERVICE_CHANGED=1
      cp "$TMP/service" "$UNIT_FILE"
    fi
  fi
  [ "$SERVICE_CHANGED" = 0 ] || sctl daemon-reload || die "systemctl daemon-reload failed"
}

systemd_start() {
  sctl enable "$LABEL.service" >/dev/null 2>&1 || die "systemctl enable $LABEL.service failed"
  if sctl is-active --quiet "$LABEL.service"; then
    if [ "$SERVICE_CHANGED" = 1 ] || [ "$RESTART" = 1 ]; then
      sctl restart "$LABEL.service" || die "systemctl restart $LABEL.service failed"
      say "Restarted $LABEL.service"
    else
      say "$LABEL.service is running"
    fi
  else
    sctl start "$LABEL.service" || die "systemctl start $LABEL.service failed"
    say "Started $LABEL.service (restarts if it stops)"
  fi
}

systemd_stop() {
  if [ "$SERVICE" = systemd-user ]; then
    user_bus || no_user_bus "Uninstalling $LABEL.service"
  fi
  if [ -f "$UNIT_FILE" ] || sctl cat "$LABEL.service" >/dev/null 2>&1; then
    sctl disable --now "$LABEL.service" >/dev/null 2>&1 || die "could not stop $LABEL.service (systemctl disable --now failed); nothing was removed"
    if sctl is-active --quiet "$LABEL.service"; then die "$LABEL.service is still running; nothing was removed"; fi
  fi
  if [ "$SERVICE" = systemd-system ]; then sudo rm -f "$UNIT_FILE"; else rm -f "$UNIT_FILE"; fi
  sctl daemon-reload || true
  say "Stopped and removed $LABEL.service"
}

linger_note() {
  [ "$SERVICE" = systemd-user ] || return 0
  linger=$(loginctl show-user "$ME" --property=Linger --value 2>/dev/null || true)
  [ "$linger" != yes ] || return 0
  LINGER_NOTE=1
}

# ------------------------------------------------ services: none

# The pid in this instance's lock, if that process is alive: the server that owns it.
lock_owner() {
  pid=$(cat "$LOCK" 2>/dev/null | tr -dc '0-9')
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && printf '%s\n' "$pid"
}

# The supervisor of a server started by hand: the parent of the lock's owner,
# if that is this binary's `up`.
manual_supervisor() {
  pid=$(lock_owner) || return 1
  parent=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -dc '0-9')
  [ -n "$parent" ] || return 1
  case "$(ps -o args= -p "$parent" 2>/dev/null)" in
    "$BIN up"*) printf '%s\n' "$parent" ;;
    *) return 1 ;;
  esac
}

stop_manual() { # pid of the supervisor
  # The supervisor passes the stop on to the server and exits after it.
  kill -TERM "$1" 2>/dev/null || true
  i=0
  while kill -0 "$1" 2>/dev/null && [ "$i" -lt 100 ]; do i=$((i + 1)); sleep 0.1; done
}

manual_command() {
  printf 'cd ~ && APP_SERVER__HOST=127.0.0.1 APP_SERVER__PORT=%s APP_INSTANCE=%s nohup %s up -- serve >>%s 2>&1 &' \
    "$PORT" "$INSTANCE" "$BIN" "$LOG_DIR/$INSTANCE.log"
}

# What manual_command does, for a server this installer stopped and puts back.
manual_start() {
  private_logs "$LOG_DIR/$INSTANCE.log"
  (cd "$HOME" && APP_SERVER__HOST=127.0.0.1 APP_SERVER__PORT="$PORT" APP_INSTANCE="$INSTANCE" \
    nohup "$BIN" up -- serve >>"$LOG_DIR/$INSTANCE.log" 2>&1 </dev/null &)
}

# ------------------------------------------------ services: dispatch

service_running() {
  case "$SERVICE" in
    launchd) launchd_loaded && [ -n "$(lock_owner || true)" ] ;;
    systemd-user) user_bus && systemctl --user is-active --quiet "$LABEL.service" ;;
    systemd-system) systemctl is-active --quiet "$LABEL.service" 2>/dev/null ;;
    none) manual_supervisor >/dev/null ;;
  esac
}

# Stop the server for a moment, keeping its service: for a new enrolment.
service_halt() {
  case "$SERVICE" in
    launchd) launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true; launchd_wait_gone ;;
    systemd-user | systemd-system) sctl stop "$LABEL.service" || die "could not stop $LABEL.service" ;;
    none) if sup=$(manual_supervisor); then stop_manual "$sup"; fi ;;
  esac
}

# Start it again as it was, after service_halt.
service_resume() {
  case "$SERVICE" in
    launchd) [ ! -f "$PLIST" ] || { launchctl bootstrap "$DOMAIN" "$PLIST" 2>/dev/null || true; launchctl kickstart "$DOMAIN/$LABEL" 2>/dev/null || true; } ;;
    systemd-user | systemd-system) sctl start "$LABEL.service" || true ;;
    none) manual_supervisor >/dev/null || manual_start ;;
  esac
  # Until it holds its instance: what is said next asks whether it runs.
  i=0
  while ! service_running && [ "$i" -lt 50 ]; do i=$((i + 1)); sleep 0.2; done
  say "Started the server again, as it was."
}

service_write() {
  case "$SERVICE" in
    launchd) launchd_write ;;
    systemd-user | systemd-system) systemd_write ;;
    none) private_logs "$LOG_DIR/$INSTANCE.log"; SERVICE_CHANGED=0 ;;
  esac
}

service_start() {
  # A server this installer once left for you to start by hand, now that a
  # service takes over: stop it first, or the two would fight over the instance.
  if [ "$SERVICE" != none ] && [ "${PREVIOUS:-}" = none ] && sup=$(manual_supervisor); then
    stop_manual "$sup"
    say "Stopped the server you had started by hand (pid $sup); the service runs it from now on."
  fi
  case "$SERVICE" in
    launchd) launchd_start ;;
    systemd-user | systemd-system) systemd_start ;;
    none)
      if [ -n "${HALTED:-}" ] && ! manual_supervisor >/dev/null; then
        # Stopped for the new code: put back, now on the installed binary.
        manual_start
        say "Started the server you had started by hand again."
        RUNNING_BY_HAND=1
      elif sup=$(manual_supervisor); then
        if [ "$RESTART" = 1 ]; then
          stop_manual "$sup"
          say "Stopped the server you started by hand (pid $sup); start it again with the command below."
        else
          say "The server you started by hand is running (pid $sup)."
          RUNNING_BY_HAND=1
        fi
      fi ;;
  esac
}

service_stop() {
  case "$SERVICE" in
    launchd) launchd_stop ;;
    systemd-user | systemd-system) systemd_stop ;;
    none) if sup=$(manual_supervisor); then stop_manual "$sup"; say "Stopped the server (pid $sup)"; fi ;;
  esac
}

service_describe() {
  case "$SERVICE" in
    launchd) printf 'launchd %s, starts at login' "$LABEL" ;;
    systemd-user) printf 'systemd user service %s.service (systemctl --user status %s)' "$LABEL" "$LABEL" ;;
    systemd-system) printf 'systemd service %s.service, starts at boot, runs as %s' "$LABEL" "$ME" ;;
    none) printf 'none (start it yourself: the command above)' ;;
  esac
}

service_logs() {
  case "$SERVICE" in
    launchd | none) printf '%s' "$LOG_DIR/$INSTANCE.log" ;;
    systemd-user) printf 'journalctl --user -u %s' "$LABEL" ;;
    systemd-system) printf 'journalctl -u %s' "$LABEL" ;;
  esac
}

# The pids listening on the port, where lsof or ss can tell; nothing where
# neither is installed (or they cannot see the owner).
port_listeners() {
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -t -iTCP:"$PORT" -sTCP:LISTEN 2>/dev/null || true
  elif command -v ss >/dev/null 2>&1; then
    ss -ltnp "sport = :$PORT" 2>/dev/null | grep -o 'pid=[0-9]*' | cut -d= -f2 || true
  fi
}

# The server answering on the port is ours: it owns this instance's lock, runs
# the installed binary and, where that can be seen, holds the listening
# socket, rather than being anything else that happens to listen there.
ours_answering() {
  pid=$(lock_owner) || return 1
  case "$(ps -o args= -p "$pid" 2>/dev/null)" in
    "$BIN serve"* | "$BIN"*" serve"*) ;;
    *) return 1 ;;
  esac
  listeners=$(port_listeners)
  [ -z "$listeners" ] || printf '%s\n' "$listeners" | grep -qx "$pid"
}

wait_healthy() {
  step "Waiting for Droppen on 127.0.0.1:$PORT"
  i=0
  status=""
  while [ "$i" -lt 120 ]; do
    if status=$(curl -fsS -m 2 "$URL/api/auth/status" 2>/dev/null) && ours_answering; then break; fi
    status=""
    i=$((i + 1))
    sleep 0.5
  done
  if [ -z "$status" ]; then
    case "$SERVICE" in
      launchd) say "The last lines of $LOG_DIR/$INSTANCE.err.log:"; tail -n 20 "$LOG_DIR/$INSTANCE.err.log" 2>/dev/null || true ;;
      systemd-*) sctl status --no-pager "$LABEL.service" 2>/dev/null | tail -n 20 || true ;;
    esac
    die "this device's Droppen server did not answer on 127.0.0.1:$PORT within a minute"
  fi
  case "$status" in
    *'"authRequired":true'*) say "Healthy, and the local page asks for a login" ;;
    *)
      # Never leave it open: stop it rather than serve anyone who can reach the port.
      service_stop
      die "the server answered without requiring a login, so it was stopped ($status)" ;;
  esac
}

# Leaving, however it ends: a server stopped for a new code is started again
# (after a Ctrl-C, a kill or a failed step before the new one starts), and the
# download is removed.
on_exit() {
  trap '' INT TERM HUP
  resume_halted || true
  rm -rf "$TMP"
  [ -z "$STAGED" ] || rm -f "$STAGED"
}

do_install() {
  check_platform
  choose_service
  find_others
  [ -z "$CONFLICT" ] || refuse_conflict
  resolve_version
  TMP=$(mktemp -d "${TMPDIR:-/tmp}/droppen-install.XXXXXX")
  HALTED=""
  STAGED=""
  trap on_exit EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
  download_and_verify
  choose_port
  URL=http://127.0.0.1:$PORT
  step "Installing"
  MARKER_SERVICE=$SERVICE
  RESTART_PENDING=$(marker_get restart)
  write_marker
  # The local login and the enrolment run the downloaded server, before it
  # replaces anything: a step that fails leaves the device as it was.
  RUN_BIN=$NEW
  ensure_account
  ENROLLED_NOW=""
  enroll
  install_binary
  link_command
  RUN_BIN=$BIN
  step "Starting the server"
  RESTART=0
  if [ "${RESTART_PENDING:-}" = 1 ] || [ -n "$ENROLLED_NOW" ]; then RESTART=1; fi
  RUNNING_BY_HAND=""
  service_write
  service_start
  HALTED=""
  if [ "$SERVICE" != none ] || [ -n "$RUNNING_BY_HAND" ]; then
    wait_healthy
  fi
  # Whatever runs now runs the installed binary. With no service: one started
  # by hand was restarted or stopped above, and one started later starts it.
  RESTART_PENDING=""
  write_marker
  LINGER_NOTE=""
  [ "$OS" != linux ] || linger_note
  # The local page, opened only where there is a screen and nothing to enrol.
  if [ "$SERVICE" != none ] && has_display && [ -z "$CODE" ] && [ -z "$ENROLLED" ] && [ "${DROPPEN_NO_OPEN:-}" != 1 ]; then
    if [ "$OS" = macos ]; then
      open "$URL/" >/dev/null 2>&1 || true
    elif command -v xdg-open >/dev/null 2>&1; then
      xdg-open "$URL/" >/dev/null 2>&1 || true
    fi
  fi
  report_others
  say ""
  if [ "$SERVICE" = none ] && [ -z "$RUNNING_BY_HAND" ]; then
    say "Droppen $VERSION is installed. There is no systemd user session here, so nothing starts it."
    say "Droppen $VERSION 已安装；这里没有 systemd 用户会话，需要你自己启动它。"
    say "Start it now, and from your init system at boot:"
    say ""
    say "    $(manual_command)"
  else
    say "Droppen $VERSION is running on 127.0.0.1:$PORT."
    say "Droppen $VERSION 已在 127.0.0.1:$PORT 运行。"
  fi
  say ""
  if [ -n "$ENROLLED" ]; then
    say "  Use it:     on my.droppen.app, from any device signed in to your account"
    if has_display; then say "  Local page: $URL (signing in as $ACCOUNT_NOTE)"; fi
  elif has_display; then
    say "  Local page: $URL"
    say "  Sign in:    $ACCOUNT_NOTE"
    say "  Next:       choose 「登录 Droppen」 on that page to add this device to your Droppen account"
  else
    say "  Join:       on my.droppen.app, $WHERE_CODES; then run the installer again with DROPPEN_CODE=<code>"
    say "  Local page: on this device only; from elsewhere, an SSH tunnel to port $PORT, signing in as $ACCOUNT_NOTE"
  fi
  say "  Command:    $CMD (the server; $BIN_NAME is the same program)"
  say "  Service:    $(service_describe), port $PORT"
  say "  Data:       $DATA_DIR (instance $INSTANCE), $CONFIG_DIR"
  say "  Logs:       $(service_logs)"
  say "  Upgrade:    $INSTALL_CMD"
  say "  Uninstall:  $INSTALL_CMD -s -- --uninstall"
  if [ -n "$LINGER_NOTE" ]; then
    say ""
    say "A systemd user service stops when you log out. To keep Droppen running, and start it at boot:"
    say "    sudo loginctl enable-linger $ME"
  fi
  if [ "$OS" = macos ]; then
    say ""
    say "This is the server, for background and headless use. For everyday use, the"
    say "Droppen desktop App is recommended: $RELEASES_URL/releases/latest"
    say "这是后台/无界面使用的服务端；日常使用推荐 Droppen 桌面 App（同一发布页）。"
  fi
}

# ---------------------------------------------------------------- uninstall

do_uninstall() {
  check_platform
  if [ "$OS" = linux ]; then
    SERVICE=$(marker_get service)
    [ -n "$SERVICE" ] || SERVICE=systemd-user
    stored=$(marker_get label)
    [ -z "$stored" ] || LABEL=$stored
    if [ "$SERVICE" = systemd-system ]; then UNIT_FILE=/etc/systemd/system/$LABEL.service; else UNIT_FILE=$USER_UNIT_DIR/$LABEL.service; fi
  fi
  find_others
  service_file=${PLIST:-${UNIT_FILE:-}}
  if ! ours && [ ! -f "$service_file" ]; then
    report_others
    say ""
    say "Droppen is not installed by this installer for this user (no $LABEL); nothing to do."
    exit 0
  fi
  if [ ! -f "$service_file" ] && [ ! -e "$BIN" ] && [ -z "$(marker_get service)" ]; then
    say "Droppen is not installed here (its data, kept by an earlier uninstall, is): nothing to do."
    exit 0
  fi
  step "Uninstalling $LABEL"
  service_stop
  if [ -n "$CONFLICT" ] && [ -e "$BIN" ]; then
    say "Kept $BIN: $CONFLICT."
  else
    rm -f "$BIN" "$BIN.prev" "$BIN.bad" "$BIN_DIR"/.staging-* "$BIN_DIR/.installer-link"
    removed=$BIN
    # The link, if it is ours (anything else there is not).
    if our_link; then
      rm -f "$CMD"
      removed="$BIN and $CMD"
    fi
    rmdir "$BIN_DIR" 2>/dev/null || true
    say "Removed $removed"
  fi
  # Kept, so the data stays recognised as ours, but with no service: the next
  # install chooses one afresh (a user service, or --system).
  if ours; then
    PORT=$(marker_get port)
    VERSION=$(marker_get version)
    MARKER_SERVICE=""
    RESTART_PENDING=""
    write_marker
  fi
  report_others
  cat <<EOF

Droppen is uninstalled. Your data was kept, so installing again picks it up:
Droppen 已卸载；数据已保留，重新安装即可继续使用：
    $DATA_DIR
    $CONFIG_DIR
    $LOG_DIR
If this device joined a Droppen account, remove it on my.droppen.app too (my devices).
To delete the data as well (sessions, accounts, settings, this device's key), run:
    rm -rf "$DATA_DIR" "$CONFIG_DIR" "$LOG_DIR"
EOF
}

# `--code -`: the code from the terminal, not echoed.
ask_code() {
  (: </dev/tty) 2>/dev/null || die "--code - needs a terminal to type the code on; set DROPPEN_CODE instead"
  saved=$(stty -g </dev/tty 2>/dev/null || true)
  # The terminal gets its echo back however the prompt ends: a Ctrl-C, a kill
  # or a closed terminal included.
  trap 'code_interrupted 130' INT
  trap 'code_interrupted 143' TERM
  trap 'code_interrupted 129' HUP
  # Echo off before the prompt shows, so nothing typed or pasted is echoed.
  stty -echo </dev/tty 2>/dev/null || true
  printf 'One-time code from my.droppen.app (%s): ' "$WHERE_CODES" >/dev/tty
  IFS= read -r CODE </dev/tty || CODE=""
  echo_back
  # A pasted code may bring a space or a carriage return along.
  CODE=$(printf '%s' "$CODE" | tr -d '[:space:]')
  trap - INT TERM HUP
  printf '\n' >/dev/tty
  [ -n "$CODE" ] || die "no code was typed. Get one on my.droppen.app ($WHERE_CODES) and run the installer again."
}

echo_back() { [ -z "$saved" ] || stty "$saved" </dev/tty 2>/dev/null || true; }

code_interrupted() { # exit status
  echo_back
  printf '\n' >/dev/tty 2>/dev/null || true
  exit "$1"
}

main() {
  action=do_install
  # Taken from the environment and dropped from it: no command this runs
  # (curl, systemctl, the server) inherits the code.
  CODE=${DROPPEN_CODE:-}
  unset DROPPEN_CODE
  SYSTEM=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --uninstall) action=do_uninstall ;;
      --system) SYSTEM=1 ;;
      --code)
        [ "${2:-}" = - ] || die "--code takes no code on the command line (other users of this device can read it): use DROPPEN_CODE=<code>, or --code - to type it"
        ask_code
        shift ;;
      --code=*) die "--code takes no code on the command line (other users of this device can read it): use DROPPEN_CODE=<code>, or --code - to type it" ;;
      -h | --help) usage; exit 0 ;;
      *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
  done
  case "$CODE" in
    *[!A-Za-z0-9-]*) die "that does not look like a one-time code from my.droppen.app" ;;
  esac
  set_paths
  "$action"
}

# Everything above only defines functions, so a download cut short runs nothing.
main "$@"
