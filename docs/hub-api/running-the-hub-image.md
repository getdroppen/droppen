# Running the hub image

What open tooling relies on when it runs a Droppen Hub **image** instead of a
hub it built: today `crates/droppen-devkit/scripts/cloud-dev.sh` with
`DROPPEN_HUB_IMAGE`, the local cloud stack for developing the web app and the
local server against a throwaway hub. This page is that image's run
interface for such a hub — how to start it, what to set, what it listens on
and what it writes — and nothing more. It is not how droppen.app is
deployed, and open code may rely on no other setting, command or file of the
image (README.md).

## The image

- Named by digest, `<registry>/<name>@sha256:<64 hex digits>`, so every run
  gets the same hub. `cloud-dev.sh` refuses a tag.
- `rh-hub` is on `PATH`; the default command is `rh-hub serve`.
- It runs as whatever user `--user` gives it (an unprivileged one by
  default) and needs no files of its own but the two directories below.
- It carries a build of the web app and serves it, unless
  `DROPPEN_HUB_WEB_DIR` names another.

## Commands

Each takes the environment below; the last two do their one thing against
the database and exit.

| command | what for | output |
|---|---|---|
| `rh-hub serve` | the hub | its log on stdout and stderr; `GET /healthz` answers `200 ok` once it serves (http.md) |
| `rh-hub invite create --note <text>` | an invite for sign-up by invite | the invite code, alone on stdout |
| `rh-hub relay token create <id> --region <region> --city <city>` | a local relay's `DROPPEN_RELAY_REGISTRATION_TOKEN` | the token on stdout's first line |

`SIGTERM` stops `serve`: it closes every socket with `1012` (websocket.md)
and exits.

## Settings

Required by `serve`:

| variable | value |
|---|---|
| `DATABASE_URL` | a Postgres URL, a database of the hub's own (it sets up its schema there) |
| `DROPPEN_HUB_REDIS_URL` | a Redis URL, a Redis of the hub's own |
| `DROPPEN_HUB_BUCKET_DIR` | a directory the hub may write: a development hub's storage, in place of a bucket |
| `DROPPEN_HUB_SESSION_KEY` | a secret of at least 32 bytes, in hex |
| `DROPPEN_HUB_ORIGIN` | the hub's origin as clients dial it, e.g. `ws://127.0.0.1:<port>`: every sign-in signature binds it |
| `PORT` | the port to listen on (default 8080) |
| `DROPPEN_HUB_BIND` | the address to bind (default `::`); `cloud-dev.sh` binds `127.0.0.1` |

Optional, as `cloud-dev.sh` sets them:

| variable | value |
|---|---|
| `DROPPEN_HUB_WEB_DIR` | a `pnpm build` of the web app (`web/dist`) to serve instead of the image's own |
| `DROPPEN_HUB_CHAIN_V3` | `write` (the default), `read` or `off`: whether the hub takes new chain v3 entries (`docs/local-aggregator.md` §3.5). The web app makes every new account on chain v3, so sign-up needs `write`; `read` and `off` are kill switches — under them sign-up stops at creating the account's keys, saying the hub does not take chain v3 yet |
| `DROPPEN_HUB_RELAY_INSECURE` | `1`: relays may register `ws://` origins on loopback (local testing only) |
| `DROPPEN_HUB_CHANNEL_RELAY` | `on` (the default): the hub still carries channels itself; `off`: channels go through relays only |
| `RUST_LOG` | the log filter |

For a browser run with open sign-up (`DROPPEN_CLOUD_DEV_OPEN=1`):

| variable | value |
|---|---|
| `DROPPEN_HUB_SIGNUP` | `invite` (the default), `open` or `closed` |
| `DROPPEN_HUB_DEV_OPEN_SIGNUP` | `1`: allow `open` on a loopback origin without real mail and Turnstile |
| `DROPPEN_HUB_EMAIL_LOG` | `1`: write each verification link to the log and send no mail |
| `DROPPEN_HUB_PUBLIC_URL` | the origin the links in mail point at |
| `DROPPEN_HUB_TURNSTILE` | `off`: no Turnstile, by choice ("Without Turnstile", README.md) |
| `DROPPEN_HUB_TURNSTILE_SITE_KEY`, `DROPPEN_HUB_TURNSTILE_SECRET`, `DROPPEN_HUB_TURNSTILE_ORIGIN` | Turnstile, its widget on an origin of its own; all three or none (`cloud-dev.sh` uses Cloudflare's always-pass testing keys) |

## Network and volumes

- It listens on `DROPPEN_HUB_BIND`:`PORT`: the HTTP routes (http.md) and the
  sockets (websocket.md).
- It dials its database and Redis, and each relay's origin when the relay
  registers (`GET <origin>/v1/relay-id`, websocket.md). A relay on loopback
  is reached only if the hub shares its network: `cloud-dev.sh` runs the
  image with `--network host`, so the hub finds Postgres, Redis and the relays
  at the same `127.0.0.1` addresses a hub built from source does.
- It writes nothing but `DROPPEN_HUB_BUCKET_DIR`, and reads
  `DROPPEN_HUB_WEB_DIR`. `cloud-dev.sh` mounts both at the paths they have on
  the host, the web app read-only, and runs the image as the calling user
  (`--user $(id -u):$(id -g)`) so the directory stays writable.
