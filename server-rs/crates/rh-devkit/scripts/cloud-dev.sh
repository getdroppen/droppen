#!/usr/bin/env bash
# A local cloud stack for trying the browser by hand (or by a driven
# browser): Postgres and Redis in docker, sealed pages in a directory
# (DROPPEN_HUB_BUCKET_DIR), a hub serving web/dist, and a throwaway agent
# instance (APP_INSTANCE=cloudtest, its own HOME/config/data under the state
# directory, with one fixture transcript). Nothing touches 8700/8701 or the
# keychain.
#
#   cloud-dev.sh up              start Postgres, Redis and the hub; print the URL and an invite
#                                (DROPPEN_HUB_CHAIN_V3: `write` by default — the web app makes every
#                                new account on chain v3, so it signs up only there; `read` or `off`
#                                to try a rollback)
#                                (RH_CLOUD_DEV_OPEN=1: open sign-up, verification links in the
#                                hub log, and Turnstile with Cloudflare's always-pass testing keys
#                                on http://verify.localhost:<port>;
#                                RH_CLOUD_DEV_TURNSTILE=off: open sign-up without Turnstile, by
#                                choice, as droppen.app may run;
#                                RH_CLOUD_DEV_EMAIL_LOG=1: verification links in the hub log
#                                with sign-up by invite too;
#                                RH_CLOUD_DEV_HOST=localhost: the hub's origin on localhost, not
#                                127.0.0.1 — passkeys need a domain: no browser takes an IP
#                                address as a relying party)
#   cloud-dev.sh remote <url>    no local hub: only the throwaway agent, for a deployed hub
#   cloud-dev.sh invite          print another invite
#   cloud-dev.sh enroll <code>   run `agent enroll` for the throwaway instance
#   cloud-dev.sh agent           start the instance's server (`agent run`)
#   cloud-dev.sh stop-agent      stop it (the machine goes offline; its encrypted copy stays readable)
#   cloud-dev.sh restart-hub     rebuild a source hub, then SIGTERM the hub (it closes sockets with 1012) and start it again
#   cloud-dev.sh canary <text>   append an assistant message to the fixture session
#   cloud-dev.sh image           draw a picture on the host and link it from the fixture session (blob RPC)
#   cloud-dev.sh relays          start three relays: loc-a (near), loc-b (+30 ms) and loc-c (+60 ms),
#                                the last two behind a toxiproxy container
#   cloud-dev.sh relay-stop <id> kill a relay (a crash); relay-start <id> starts it again
#   cloud-dev.sh check <text>... dump the database and the hub log; count canary hits
#   cloud-dev.sh down            stop everything and delete the state
#
# The fixtures: an owned session (the canary goes into it), a long owned
# session (copied to the hub as snapshot pages), and a session in ~/.claude
# that the server lists but never copies to the hub.
#
# The hub: where this tree has its source (server-rs/crates/rh-hub), `up`
# builds it and runs target/debug/rh-hub. Elsewhere — the open repository has
# no hub — it runs the image DROPPEN_HUB_IMAGE names, which must be pinned by
# its digest (<registry>/<name>@sha256:<64 hex digits>): a tag is refused, so
# a stale copy of one can never stand in for the hub you meant. Where the
# source is here, the image runs only when asked for with
# DROPPEN_CLOUD_DEV_HUB=image (and DROPPEN_HUB_IMAGE). `up` records which hub
# it chose (HUB, HUB_IMAGE in the state's env) for the commands after it, and
# `up` and `restart-hub` say which on stderr, every time. A private image
# must be pulled beforehand (`docker pull`, logged in): this script runs
# docker with an empty config, never with your credentials. With no hub to
# run, `up` says so and starts nothing (`remote` needs no local hub).
# docs/hub-api/running-the-hub-image.md is what this relies on of the image.
#
# The image runs as you, with host networking, the state directory mounted
# at its own path and web/dist too (the image's own web app when web/dist is
# not built), so it takes the very environment a hub built from source does:
# Postgres, Redis and the relays at the same 127.0.0.1 addresses. Host
# networking needs Linux, OrbStack (the docker this script uses by default)
# or Docker Desktop with host networking turned on. On a Mac the hub then
# runs in docker's Linux VM, which forwards its port to the Mac; but it also
# dials each relay's origin (127.0.0.1) to check it, and that reaches a relay
# on the Mac only where the runtime forwards loopback both ways. Where it
# does not, `relays` registers none: try relays with the source build.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)" # server-rs
state="${RH_CLOUD_DEV_DIR:-/tmp/rh-cloud-dev}"
export DOCKER_HOST="${DOCKER_HOST:-unix://$HOME/.orbstack/run/docker.sock}"
mkdir -p "$state"
# Absolute: the hub image mounts it at the same path.
state="$(cd "$state" && pwd)"
export DOCKER_CONFIG="$state/docker"
mkdir -p "$DOCKER_CONFIG"
[ -f "$DOCKER_CONFIG/config.json" ] || echo '{}' >"$DOCKER_CONFIG/config.json"
# Containers by name (RH_CLOUD_DEV_NAME, with RH_CLOUD_DEV_DIR for a second
# stack beside another's: `up` and `down` remove containers of these names).
name="${RH_CLOUD_DEV_NAME:-rh-cloud-dev}"
container=$name-pg
redis=$name-redis
toxi=$name-toxi
# The hub's container, when it runs from DROPPEN_HUB_IMAGE.
hubc=$name-hub
# The web app the hub serves: this tree's build.
web_dist="$(cd "$root/.." && pwd)/web/dist"
session_id=c10dc0de-0000-4000-8000-00000000e2e0
big_id=c10dc0de-0000-4000-8000-0000000b16b1
external_id=c10dc0de-0000-4000-8000-0000000e7e2a
# The instance's own (managed) Claude root, so the session is listed as its own.
project_dir() { echo "$state/data/instances/cloudtest/claude/projects/-tmp-cloud-e2e"; }

load() { [ -f "$state/env" ] && . "$state/env"; }

# The hub's host as clients dial it: 127.0.0.1, or localhost (RH_CLOUD_DEV_HOST).
hub_host() { echo "${HUB_HOST:-127.0.0.1}"; }

# Whether this stack's hub is the image (`up` chose, and recorded HUB=image).
image_mode() { [ "${HUB:-source}" = image ]; }

# An image reference pinned by its digest: <name>@sha256:<64 hex digits>.
pinned() { [[ "$1" =~ @sha256:[0-9a-f]{64}$ ]]; }

# Which hub runs, on stderr: stdout's `hub:` line is the URL that callers read.
say_hub() {
  if ! image_mode; then
    echo "hub: source (server-rs/crates/rh-hub)" >&2
  elif [ -d "$root/crates/rh-hub" ]; then
    echo "hub: image $HUB_IMAGE — this tree's rh-hub is not used" >&2
  else
    echo "hub: image $HUB_IMAGE" >&2
  fi
}

# The hub's environment, as K=V words in the array `hub_vars`: the same for
# the binary built from source and for the image, which sees the same paths.
set_hub_vars() {
  hub_vars=(DATABASE_URL="$DATABASE_URL" DROPPEN_HUB_REDIS_URL="$REDIS_URL" DROPPEN_HUB_BUCKET_DIR="$state/bucket" PORT="$HUB_PORT"
    DROPPEN_HUB_BIND=127.0.0.1 DROPPEN_HUB_ORIGIN="ws://$(hub_host):$HUB_PORT" DROPPEN_HUB_SESSION_KEY="$SESSION_KEY"
    DROPPEN_HUB_CHAIN_V3="${CHAIN_V3:-write}")
  if ! image_mode || [ -d "$web_dist" ]; then
    hub_vars+=(DROPPEN_HUB_WEB_DIR="$web_dist")
  fi
}

# How the image runs (the array `hub_docker`): as you, on the host's network,
# with the state directory and web/dist where the hub's environment says.
set_hub_docker() {
  hub_docker=(--network host --user "$(id -u):$(id -g)" -v "$state:$state")
  if [ -d "$web_dist" ]; then
    hub_docker+=(-v "$web_dist:$web_dist:ro")
  fi
}

# `rh-hub <args>` on this stack's database (an invite, a relay's token): the
# binary built from source, or the image's.
hub_cli() {
  set_hub_vars
  hub_vars+=(RUST_LOG=warn)
  if image_mode; then
    set_hub_docker
    docker run --rm "${hub_docker[@]}" "${hub_vars[@]/#/--env=}" "$HUB_IMAGE" rh-hub "$@" </dev/null
  else
    env "${hub_vars[@]}" "$root/target/debug/rh-hub" "$@"
  fi
}

agent_env() {
  env -u PORT -u HOST HOME="$state/home" APP_HOME_DIR="$state/home" APP_INSTANCE=cloudtest \
    APP_SERVER__PORT="$AGENT_PORT" APP_SERVER__HOST=127.0.0.1 \
    DROPPEN_CONFIG_DIR="$state/config" DROPPEN_DATA_DIR="$state/data" SHELL=/bin/sh RUST_LOG=info \
    DROPPEN_AGENT_RELAY_INSECURE=1 "$@"
}

# Open sign-up for a browser run (RH_CLOUD_DEV_OPEN=1 at `up`): verification
# links go to the hub's log, and Turnstile uses Cloudflare's testing keys on
# its own origin (verify.localhost resolves to the loopback in browsers). The
# hub refuses open sign-up without real Turnstile and mail, but for a loopback
# origin that says DROPPEN_HUB_DEV_OPEN_SIGNUP=1.
open_env() {
  if [ "${OPEN:-0}" = 1 ]; then
    echo DROPPEN_HUB_SIGNUP=open DROPPEN_HUB_DEV_OPEN_SIGNUP=1 DROPPEN_HUB_EMAIL_LOG=1 DROPPEN_HUB_PUBLIC_URL=http://$(hub_host):$HUB_PORT
    if [ "${TURNSTILE:-on}" = off ]; then
      echo DROPPEN_HUB_TURNSTILE=off
    else
      # Turnstile's page on a host of its own, never on the passkeys'
      # relying party (the hub refuses that): verify.localhost when the hub
      # is on 127.0.0.1 (no passkeys there), 127.0.0.1 when it is on localhost.
      local turnstile_host=verify.localhost
      [ "$(hub_host)" = localhost ] && turnstile_host=127.0.0.1
      echo DROPPEN_HUB_TURNSTILE_SITE_KEY=1x00000000000000000000AA \
        DROPPEN_HUB_TURNSTILE_SECRET=1x0000000000000000000000000000000AA DROPPEN_HUB_TURNSTILE_ORIGIN=http://$turnstile_host:$HUB_PORT
    fi
  elif [ "${EMAIL_LOG:-0}" = 1 ]; then
    echo DROPPEN_HUB_EMAIL_LOG=1 DROPPEN_HUB_PUBLIC_URL=http://$(hub_host):$HUB_PORT
  fi
}

# Start the hub in the background, recording *its* pid (not a subshell's).
# From the image that is `docker run`'s, which hands the hub the signals it
# gets (attached, it proxies them) and exits with it, and writes its log: so
# `restart-hub` and `down` stop either hub the same way. Every start says
# which hub it is.
start_hub() {
  say_hub
  set_hub_vars
  # shellcheck disable=SC2207
  hub_vars+=($(open_env) DROPPEN_HUB_RELAY_INSECURE=1 DROPPEN_HUB_CHANNEL_RELAY="${DROPPEN_HUB_CHANNEL_RELAY:-${RH_HUB_CHANNEL_RELAY:-on}}"
    RUST_LOG="trace,sqlx=info,hyper=info,hyper_util=info,h2=info,tokio_tungstenite=info,tungstenite=info")
  if image_mode; then
    set_hub_docker
    docker rm -f "$hubc" >/dev/null 2>&1 || true
    (exec docker run --rm --name "$hubc" "${hub_docker[@]}" "${hub_vars[@]/#/--env=}" "$HUB_IMAGE" rh-hub serve >>"$state/hub.log" 2>&1 </dev/null) &
  else
    (exec env "${hub_vars[@]}" "$root/target/debug/rh-hub" serve >>"$state/hub.log" 2>&1 </dev/null) &
  fi
  echo $! >"$state/hub.pid"
  for _ in $(seq 1 60); do curl -sf "http://127.0.0.1:$HUB_PORT/healthz" >/dev/null && break; sleep 0.25; done
}

fixtures() {
  mkdir -p "$(project_dir)" "$state/home/.claude/projects/-tmp-cloud-e2e-external"
  cat >"$(project_dir)/$session_id.jsonl" <<'JSONL'
{"type":"user","cwd":"/tmp/cloud-e2e","timestamp":"2026-10-15T10:00:00.000Z","message":{"content":[{"type":"text","text":"cloud e2e fixture session"}]}}
{"type":"assistant","timestamp":"2026-10-15T10:00:01.000Z","message":{"model":"claude-x","content":[{"type":"text","text":"ready"}]}}
JSONL
  {
    printf '%s\n' '{"type":"user","cwd":"/tmp/cloud-e2e","timestamp":"2026-10-15T08:00:00.000Z","message":{"content":[{"type":"text","text":"a long session, copied as snapshot pages"}]}}'
    for i in $(seq 1 300); do
      printf '{"type":"assistant","timestamp":"2026-10-15T08:00:01.000Z","message":{"model":"claude-x","content":[{"type":"text","text":"line %s of a long session: %s"}]}}\n' "$i" "$(printf 'lorem ipsum %.0s' $(seq 1 60))"
    done
    printf '%s\n' '{"type":"assistant","timestamp":"2026-10-15T08:00:02.000Z","message":{"model":"claude-x","content":[{"type":"text","text":"the end of the long session"}]}}'
  } >"$(project_dir)/$big_id.jsonl"
  printf '%s\n' '{"type":"user","cwd":"/tmp/cloud-e2e-external","timestamp":"2026-10-15T09:00:00.000Z","message":{"content":[{"type":"text","text":"an external session (not copied to the hub)"}]}}' \
    >"$state/home/.claude/projects/-tmp-cloud-e2e-external/$external_id.jsonl"
}

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }

# One relay: `start_relay <id> <listen port> <origin>`; its key and token are kept in the state.
start_relay() {
  local id=$1 port=$2 origin=$3
  [ -f "$state/relay-$id.key" ] || openssl rand -hex 32 >"$state/relay-$id.key"
  if [ ! -f "$state/relay-$id.token" ]; then
    hub_cli relay token create "$id" --region loc --city "$id" | head -1 >"$state/relay-$id.token"
  fi
  (exec env -i PATH="$PATH" DROPPEN_RELAY_HUB="ws://127.0.0.1:$HUB_PORT" DROPPEN_RELAY_ORIGIN="$origin" DROPPEN_RELAY_ID="$id" \
    DROPPEN_RELAY_KEY="$(cat "$state/relay-$id.key")" DROPPEN_RELAY_REGISTRATION_TOKEN="$(cat "$state/relay-$id.token")" \
    DROPPEN_RELAY_BIND=127.0.0.1 PORT="$port" DROPPEN_RELAY_DRAIN_SECS=3 RUST_LOG=trace \
    "$root/target/debug/droppen-relay" serve >>"$state/relay-$id.log" 2>&1 </dev/null) &
  echo $! >"$state/relay-$id.pid"
  echo "$id $port $origin" >"$state/relay-$id.conf"
}

case "${1:-}" in
up)
  # The hub first, and nothing starts when there is none to run: this tree's
  # source unless the image is asked for; the image where there is no source.
  hub_image="${DROPPEN_HUB_IMAGE:-}"
  case "${DROPPEN_CLOUD_DEV_HUB:-}" in
    "") if [ -d "$root/crates/rh-hub" ]; then hub=source; else hub=image; fi ;;
    source | image) hub=$DROPPEN_CLOUD_DEV_HUB ;;
    *) echo "cloud-dev.sh: DROPPEN_CLOUD_DEV_HUB is \`source\` or \`image\`, not \`$DROPPEN_CLOUD_DEV_HUB\`" >&2; exit 2 ;;
  esac
  if [ "$hub" = source ] && [ ! -d "$root/crates/rh-hub" ]; then
    echo "cloud-dev.sh: DROPPEN_CLOUD_DEV_HUB=source, but this tree has no server-rs/crates/rh-hub" >&2
    exit 2
  fi
  if [ "$hub" = image ] && [ -z "$hub_image" ]; then
    if [ -d "$root/crates/rh-hub" ]; then
      echo "cloud-dev.sh: DROPPEN_CLOUD_DEV_HUB=image needs DROPPEN_HUB_IMAGE, the image to run" >&2
    else
      echo "cloud-dev.sh: no hub to run: this tree has no server-rs/crates/rh-hub to build it from." >&2
    fi
    echo "  Set DROPPEN_HUB_IMAGE to a hub image pinned by digest (<registry>/<name>@sha256:<64 hex digits>)," >&2
    echo "  or use \`cloud-dev.sh remote <url>\` with a deployed hub." >&2
    exit 2
  fi
  if [ "$hub" = image ] && ! pinned "$hub_image"; then
    echo "cloud-dev.sh: DROPPEN_HUB_IMAGE=$hub_image is not pinned by digest: give <registry>/<name>@sha256:<64 hex digits>," >&2
    echo "  so a cached image under a tag can never stand in for the hub you meant" >&2
    exit 2
  fi
  if [ "$hub" = source ]; then
    if [ -n "$hub_image" ]; then
      echo "cloud-dev.sh: DROPPEN_HUB_IMAGE is set but not used: this tree's hub source wins unless DROPPEN_CLOUD_DEV_HUB=image" >&2
    fi
    hub_image=""
  elif ! docker image inspect "$hub_image" >/dev/null 2>&1; then
    docker pull -q "$hub_image" >/dev/null ||
      { echo "cloud-dev.sh: cannot pull $hub_image (from a private registry: \`docker pull\` it yourself first, logged in)" >&2; exit 1; }
  fi
  docker rm -f "$container" >/dev/null 2>&1 || true
  docker run -d --rm --name "$container" -e POSTGRES_PASSWORD=rhtest -e POSTGRES_DB=rh -p 127.0.0.1::5432 postgres:16-alpine >/dev/null
  for _ in $(seq 1 60); do docker exec "$container" pg_isready -U postgres -h 127.0.0.1 >/dev/null 2>&1 && break; sleep 0.5; done
  pg_port="$(docker port "$container" 5432 | head -1 | sed 's/.*://')"
  docker rm -f "$redis" >/dev/null 2>&1 || true
  redis_port="$(free_port)"
  docker run -d --name "$redis" -p "127.0.0.1:$redis_port:6379" redis:7-alpine redis-server --appendonly yes --appendfsync everysec >/dev/null
  for _ in $(seq 1 60); do docker exec "$redis" redis-cli ping 2>/dev/null | grep -q PONG && break; sleep 0.5; done
  cat >"$state/env" <<ENV
DATABASE_URL=postgres://postgres:rhtest@127.0.0.1:$pg_port/rh
REDIS_URL=redis://127.0.0.1:$redis_port
HUB_PORT=$(free_port)
AGENT_PORT=$(free_port)
SESSION_KEY=$(openssl rand -hex 32)
OPEN=${RH_CLOUD_DEV_OPEN:-0}
TURNSTILE=${RH_CLOUD_DEV_TURNSTILE:-on}
EMAIL_LOG=${RH_CLOUD_DEV_EMAIL_LOG:-0}
HUB_HOST=${RH_CLOUD_DEV_HOST:-127.0.0.1}
CHAIN_V3=${DROPPEN_HUB_CHAIN_V3:-${RH_HUB_CHAIN_V3:-write}}
HUB=$hub
HUB_IMAGE=$hub_image
ENV
  load
  fixtures
  if image_mode; then
    (cd "$root" && cargo build -q -p droppen-relay -p remote-hand-server)
  else
    (cd "$root" && cargo build -q -p rh-hub -p droppen-relay -p remote-hand-server)
  fi
  start_hub
  echo "hub:    http://$(hub_host):$HUB_PORT"
  echo "invite: $(hub_cli invite create --note dev)"
  ;;
invite)
  load
  hub_cli invite create --note dev
  ;;
remote)
  cat >"$state/env" <<ENV
HUB_URL=$2
AGENT_PORT=$(free_port)
ENV
  load
  fixtures
  (cd "$root" && cargo build -q -p remote-hand-server)
  echo "agent instance ready for $HUB_URL"
  ;;
enroll)
  load
  agent_env "$root/target/debug/remote-hand-server" agent enroll "$2" --hub "${HUB_URL:-http://$(hub_host):$HUB_PORT}"
  ;;
agent)
  load
  (exec env -u PORT -u HOST HOME="$state/home" APP_HOME_DIR="$state/home" APP_INSTANCE=cloudtest \
    APP_SERVER__PORT="$AGENT_PORT" APP_SERVER__HOST=127.0.0.1 \
    DROPPEN_CONFIG_DIR="$state/config" DROPPEN_DATA_DIR="$state/data" SHELL=/bin/sh RUST_LOG=info \
    DROPPEN_AGENT_RELAY_INSECURE=1 \
    "$root/target/debug/remote-hand-server" agent run >>"$state/agent.log" 2>&1 </dev/null) &
  echo $! >"$state/agent.pid"
  echo "agent started (log: $state/agent.log)"
  ;;
stop-agent)
  load
  pid="$(cat "$state/agent.pid")"
  kill -TERM "$pid" 2>/dev/null || true
  for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  kill -0 "$pid" 2>/dev/null && kill -KILL "$pid"
  echo "agent stopped"
  ;;
restart-hub)
  load
  # A source hub is rebuilt first (a changed hub is the usual reason to
  # restart it), before the running one stops: a build that fails leaves it up.
  if ! image_mode; then (cd "$root" && cargo build -q -p rh-hub); fi
  pid="$(cat "$state/hub.pid")"
  kill -TERM "$pid"
  for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
  if kill -0 "$pid" 2>/dev/null; then echo "the hub did not stop" >&2; exit 1; fi
  start_hub
  echo "hub restarted (pid $(cat "$state/hub.pid"))"
  ;;
canary)
  load
  printf '{"type":"assistant","timestamp":"2026-10-15T10:00:02.000Z","message":{"model":"claude-x","content":[{"type":"text","text":"%s"}]}}\n' "$2" \
    >>"$(project_dir)/$session_id.jsonl"
  ;;
image)
  load
  pic="$state/home/cloud-dev-picture.png"
  python3 - "$pic" <<'PY'
import struct, sys, zlib
w, h = 240, 120
rows = b"".join(b"\x00" + bytes(v for x in range(w) for v in (x * 255 // w, y * 255 // h, 160)) for y in range(h))
def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
PY
  printf '{"type":"assistant","timestamp":"2026-10-15T10:00:03.000Z","message":{"model":"claude-x","content":[{"type":"text","text":"a picture on the host: ![host picture](%s)"}]}}\n' "$pic" \
    >>"$(project_dir)/$session_id.jsonl"
  echo "linked $pic"
  ;;
relays)
  load
  docker rm -f "$toxi" >/dev/null 2>&1 || true
  api_port="$(free_port)"; b_port="$(free_port)"; c_port="$(free_port)"
  docker run -d --rm --name "$toxi" -p "127.0.0.1:$api_port:8474" -p "127.0.0.1:$b_port:$b_port" -p "127.0.0.1:$c_port:$c_port" \
    ghcr.io/shopify/toxiproxy:2.9.0 >/dev/null
  for _ in $(seq 1 60); do curl -s -o /dev/null "http://127.0.0.1:$api_port/version" && break; sleep 0.5; done
  a_listen="$(free_port)"; b_listen="$(free_port)"; c_listen="$(free_port)"
  proxy() { # name listen upstream latency
    curl -sf -X POST "http://127.0.0.1:$api_port/proxies" -d "{\"name\":\"$1\",\"listen\":\"0.0.0.0:$2\",\"upstream\":\"host.docker.internal:$3\",\"enabled\":true}" >/dev/null
    for dir in upstream downstream; do
      curl -sf -X POST "http://127.0.0.1:$api_port/proxies/$1/toxics" \
        -d "{\"name\":\"l-$dir\",\"type\":\"latency\",\"stream\":\"$dir\",\"attributes\":{\"latency\":$4}}" >/dev/null
    done
  }
  proxy loc-b "$b_port" "$b_listen" 15
  proxy loc-c "$c_port" "$c_listen" 30
  start_relay loc-a "$a_listen" "ws://127.0.0.1:$a_listen"
  start_relay loc-b "$b_listen" "ws://127.0.0.1:$b_port"
  start_relay loc-c "$c_listen" "ws://127.0.0.1:$c_port"
  echo "relays: loc-a (near), loc-b (+30 ms), loc-c (+60 ms); logs in $state/relay-*.log"
  ;;
relay-stop)
  load
  kill -KILL "$(cat "$state/relay-$2.pid")" 2>/dev/null || true
  echo "$2 killed"
  ;;
relay-start)
  load
  read -r id port origin <"$state/relay-$2.conf"
  start_relay "$id" "$port" "$origin"
  echo "$2 started"
  ;;
check)
  load
  shift
  docker exec "$container" pg_dump -U postgres --inserts rh >"$state/dump.sql"
  hits=0
  for c in "$@"; do
    h="$(printf '%s' "$c" | xxd -p | tr -d '\n')"
    b="$(printf '%s' "$c" | base64 | cut -c1-16)"
    for s in "$c" "$h" "$b"; do
      n=$(( $(grep -c -F -- "$s" "$state/dump.sql" || true) + $(grep -c -F -- "$s" "$state/hub.log" || true) ))
      hits=$((hits + n))
    done
  done
  echo "dump: $(wc -c <"$state/dump.sql") bytes, $(grep -c '^INSERT INTO' "$state/dump.sql") rows; hub log: $(wc -l <"$state/hub.log") lines; canary hits: $hits"
  [ "$hits" -eq 0 ]
  ;;
down)
  load || true
  for p in hub agent relay-loc-a relay-loc-b relay-loc-c; do [ -f "$state/$p.pid" ] && kill "$(cat "$state/$p.pid")" 2>/dev/null || true; done
  [ -z "${HUB_URL:-}" ] && docker rm -f "$container" "$redis" "$toxi" "$hubc" >/dev/null 2>&1 || true
  rm -rf "$state"
  ;;
*)
  sed -n '2,/cloud-dev\.sh down/p' "$0"
  exit 2
  ;;
esac
