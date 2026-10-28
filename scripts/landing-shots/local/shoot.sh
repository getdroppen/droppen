#!/usr/bin/env bash
# Inside a throwaway Debian container (bookworm, with Rust), as root: build the
# server and the web app from this checkout, then for each language seed a demo
# home, serve it, stage the live sessions and capture the tour screenshots
# (LANGS, default `en zh-CN`: the homepage shows the Chinese ones on its Chinese
# pages, the English ones on the others).
#
#   shoot.sh [all | setup | lang <en|zh-CN>]   (default: all)
#
#   SRC  the checkout (default: the repository this script is in)
#   OUT  where the PNGs go (default: /work/out)
#
# Nothing here is meant for a machine anyone uses: it creates the user `dev`,
# installs packages and fonts, and replaces /Users/dev. Run it in a container
# whose hostname is `macbook` (run.sh does), since the app shows the host name;
# the home is where a Mac keeps it, since the paths on screen show it.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
SRC=${SRC:-$(cd "$HERE/../../.." && pwd)}
OUT=${OUT:-/work/out}
NODE_VERSION=$(cat "$SRC/.nvmrc")
NODE=/opt/node
DEMO=/opt/demo
PORT=8787
DEV_HOME=/Users/dev
export PATH="$NODE/bin:$PATH"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/work/target}
export PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers
SERVER="$CARGO_TARGET_DIR/release/remote-hand-server"
pnpm() { npx -y pnpm@10.6.3 "$@"; }

setup() {
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  # Fonts: Inter and JetBrains Mono stand in for the Mac's system fonts, Noto
  # CJK for Chinese; procps for the server's process checks; tini reaps what
  # the server leaves behind when it is stopped.
  apt-get install -y -qq --no-install-recommends \
    git procps psmisc tini xz-utils ca-certificates fontconfig \
    fonts-inter fonts-jetbrains-mono fonts-noto-cjk fonts-noto-color-emoji >/dev/null
  cat >/etc/fonts/local.conf <<'XML'
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <alias><family>sans-serif</family><prefer><family>Inter</family><family>Noto Sans CJK SC</family></prefer></alias>
  <alias><family>system-ui</family><prefer><family>Inter</family><family>Noto Sans CJK SC</family></prefer></alias>
  <alias><family>monospace</family><prefer><family>JetBrains Mono</family><family>Noto Sans Mono CJK SC</family></prefer></alias>
</fontconfig>
XML
  fc-cache -f >/dev/null

  if [ ! -x "$NODE/bin/node" ] || [ "$("$NODE/bin/node" --version)" != "v$NODE_VERSION" ]; then
    local f="node-v${NODE_VERSION}-linux-x64.tar.xz"
    (cd /tmp && curl -fsSLO "https://nodejs.org/dist/v${NODE_VERSION}/${f}" &&
      curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/SHASUMS256.txt" | grep " ${f}\$" | sha256sum -c - >/dev/null)
    rm -rf "$NODE" && mkdir -p "$NODE" && tar -xJf "/tmp/$f" -C "$NODE" --strip-components=1
  fi

  # Where a login shell finds them, for `npm` in the tour's terminals.
  ln -sf "$NODE/bin/node" "$NODE/bin/npm" "$NODE/bin/npx" /usr/local/bin/
  mkdir -p "$(dirname "$DEV_HOME")"
  if id dev >/dev/null 2>&1; then usermod -d "$DEV_HOME" dev; else useradd -m -d "$DEV_HOME" -s /bin/bash dev; fi

  # The app, from this checkout: the web build is embedded in the server.
  (cd "$SRC" && pnpm install --frozen-lockfile >/dev/null && pnpm --filter @remote-hand/web build >/dev/null)
  (cd "$SRC/server-rs" && DROPPEN_BUILD_REV="" cargo build --release --quiet -p remote-hand-server)
  (cd "$SRC" && pnpm --filter @remote-hand/e2e exec playwright install --with-deps chromium >/dev/null)

  # The scripted CLIs, on the server's PATH as `claude` and `codex`.
  mkdir -p "$DEMO/bin" "$DEMO/state"
  for cli in claude codex; do
    printf '#!/bin/sh\nDEMO_STATE=%s exec %s %s "$@"\n' "$DEMO/state" "$NODE/bin/node" "$HERE/cli/$cli.mjs" >"$DEMO/bin/$cli"
    chmod 755 "$DEMO/bin/$cli"
  done
  chmod -R a+rX "$SRC/scripts/landing-shots" "$NODE"
}

# The time zone in which it is mid-afternoon now, so the clocks in the shots
# read like a working day whenever they are taken.
afternoon_zone() {
  node -e '
    const zones = ["Pacific/Honolulu","America/Los_Angeles","America/Denver","America/Chicago","America/New_York","America/Sao_Paulo","Atlantic/Azores","Europe/London","Europe/Berlin","Europe/Athens","Europe/Moscow","Asia/Dubai","Asia/Karachi","Asia/Dhaka","Asia/Bangkok","Asia/Shanghai","Asia/Tokyo","Australia/Brisbane","Pacific/Noumea","Pacific/Auckland"];
    const hour = (tz) => Number(new Intl.DateTimeFormat("en-GB", { hour: "numeric", hourCycle: "h23", timeZone: tz }).format(new Date()));
    console.log(zones.reduce((best, tz) => (Math.abs(hour(tz) - 15) < Math.abs(hour(best) - 15) ? tz : best)));'
}

stop_server() {
  # The server, its shims, the residents they hold and the terminals' shells:
  # everything the demo user runs.
  pkill -KILL -u dev 2>/dev/null || true
  sleep 1
}

as_dev() { runuser -u dev -- env -i HOME="$DEV_HOME" USER=dev LOGNAME=dev SHELL=/bin/bash LANG=C.UTF-8 TZ="$TZ_NAME" PATH="$DEMO/bin:$NODE/bin:/usr/local/bin:/usr/bin:/bin" "$@"; }

shoot_lang() {
  local lang=$1
  TZ_NAME=$(afternoon_zone)
  stop_server
  rm -rf "$DEV_HOME" "$DEMO/state" && mkdir -p "$DEV_HOME" "$DEMO/state"
  cp /etc/skel/.bashrc "$DEV_HOME/.bashrc"
  # A tidy prompt: user@host, the directory, the git branch.
  cat >"$DEV_HOME/.bash_profile" <<'SH'
. ~/.bashrc
__branch() { local b; b=$(git symbolic-ref --short HEAD 2>/dev/null) && printf ' (%s)' "$b"; }
PS1='\[\e[1;32m\]\u@\h\[\e[0m\] \[\e[1;34m\]\w\[\e[0;35m\]$(__branch)\[\e[0m\] $ '
export CLICOLOR=1 GIT_PAGER=cat
alias ls='ls --color=auto'
clear
SH
  printf '[color]\n\tui = always\n[init]\n\tdefaultBranch = main\n' >"$DEV_HOME/.gitconfig"
  chown -R dev:dev "$DEV_HOME" "$DEMO/state"

  as_dev node "$HERE/seed.mjs" --lang "$lang" --home "$DEV_HOME" --state "$DEMO/state"
  # A fake `vite` for the web project's `npm run dev` (see vite.mjs).
  mkdir -p "$DEV_HOME/code/web/node_modules/.bin"
  printf '#!/bin/sh\nexec node %s "$@"\n' "$HERE/vite.mjs" >"$DEV_HOME/code/web/node_modules/.bin/vite"
  chmod 755 "$DEV_HOME/code/web/node_modules/.bin/vite" && chown -R dev:dev "$DEV_HOME/code/web/node_modules"

  as_dev bash -c "cd '$DEV_HOME' && APP_HOME_DIR='$DEV_HOME' APP_AUTH__ENABLED=false APP_SERVER__PORT=$PORT \
    DROPPEN_STALL_MS=86400000 DROPPEN_SESSION_LOG=0 exec '$SERVER' serve" >"$DEMO/server-$lang.log" 2>&1 &
  as_dev node "$HERE/own.mjs" --home "$DEV_HOME" --state "$DEMO/state"
  node "$HERE/stage.mjs" --lang "$lang" --url "http://127.0.0.1:$PORT" --state "$DEMO/state"
  node "$HERE/capture.mjs" --lang "$lang" --url "http://127.0.0.1:$PORT" --state "$DEMO/state" --out "$OUT" --tz "$TZ_NAME"
  stop_server
}

# Stopped servers leave their shims and shells to be adopted; in a container
# whose first process is not an init nothing would reap them, so the shooting
# runs under tini as a subreaper.
under_tini() {
  if [ -z "${SHOOT_UNDER_TINI:-}" ] && command -v tini >/dev/null; then
    SHOOT_UNDER_TINI=1 exec tini -s -- bash "$0" "$@"
  fi
}

case "${1:-all}" in
  setup) setup ;;
  lang)
    under_tini "$@"
    shoot_lang "${2:?lang}"
    ;;
  all)
    [ -n "${SHOOT_UNDER_TINI:-}" ] || setup
    under_tini "$@"
    mkdir -p "$OUT"
    for lang in ${LANGS:-en zh-CN}; do shoot_lang "$lang"; done
    ls -1 "$OUT"
    ;;
  *) echo "usage: shoot.sh [all | setup | lang <en|zh-CN>]" >&2; exit 2 ;;
esac
