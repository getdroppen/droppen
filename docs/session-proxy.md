# Running the session proxy

`droppen session-proxy` lets an external backend that cannot reach your
computers — a service in the cloud, such as dont-do on Railway — start and
follow coding sessions on them. It is a device of your Droppen account of its
own, and unlike the [A2A proxy](a2a-proxy.md) it is a **normal device with
full control**: it takes a small session HTTP surface from the backend and
carries each request over an end-to-end encrypted channel, through a relay
that only sees ciphertext, to the computer the request names:

```
backend ──HTTP (private network)──▶ droppen session-proxy ──Noise channel, via a relay──▶ your computer: /api/sessions/*
```

The computer decides what it accepts: the account's device chain and its
accept policy. The proxy can reach `/api/*` — the session routes, the feed
socket, everything a browser on the same network could — and it holds none of
your keys to do it: its own device key is all it has, and it never sees your
account key or a computer's machine key. The Hub never sees a request.

## Before you start

- Every computer the proxy should reach must run a version that knows chain
  v3, and must accept control from the proxy (its device settings).
- The proxy needs a **persistent volume**: its identity (its keys) and its
  verified copy of the account's device chain live there. Losing it means a
  new proxy: remove the old one on my.droppen.app and add it again.

## The image

Each release has an image, built from that release's attested Linux binary,
for `amd64` and `arm64`:

```sh
docker pull ghcr.io/getdroppen/droppen-session-proxy:vX.Y.Z
gh attestation verify oci://ghcr.io/getdroppen/droppen-session-proxy:vX.Y.Z --repo getdroppen/droppen
```

Or build it from the repository root: `docker build -f
crates/droppen-session-proxy/Dockerfile .`. The image runs `droppen
session-proxy` as an unprivileged user (uid 10001), keeps its data in `/data`
(a volume), listens on `PORT` (8080), and checks its own health (`droppen
session-proxy --health`).

## Configuration

All of it is the environment:

| variable | |
|---|---|
| `DROPPEN_SESSION_PROXY_TOKEN` | required: the Bearer token your backend presents, at least 32 characters (`openssl rand -hex 32`). The proxy compares it in constant time and never logs it |
| `DROPPEN_DATA_DIR` | the persistent volume (the image sets `/data`) |
| `DROPPEN_ENROLL_CODE` | the **first start only**: the `RH1-…` code from my.droppen.app that joins the proxy to your account. Remove it once the proxy has joined (below) |
| `DROPPEN_HUB` | the Hub (default `https://api.droppen.app`, Droppen's) |
| `PORT` | where to listen (default 8080) |
| `DROPPEN_SESSION_PROXY_RELAY_INSECURE` | `1`: accept relays on loopback over plain `ws://` too, for a relay you run yourself (the agent has the same switch, `DROPPEN_AGENT_RELAY_INSECURE`). Unset: public `wss://` relays only |
| `RUST_LOG` | the log filter (default `info`) |

## On Railway, beside your backend

Run the proxy as a second service in the project of your backend, on Railway's
private network, with no public domain:

1. **New service** → **Docker Image** →
   `ghcr.io/getdroppen/droppen-session-proxy:vX.Y.Z` (or **GitHub Repo** with
   the Dockerfile path `crates/droppen-session-proxy/Dockerfile`). Name it,
   say, `droppen-session-proxy`.
2. **Add a volume** to it, mounted at `/data`. Railway mounts volumes as
   root; if the proxy's log says it cannot write `/data`, set
   `RAILWAY_RUN_UID=0` on this service (Railway's switch for images that run
   as another user).
3. **Variables** of this service: `DROPPEN_SESSION_PROXY_TOKEN` (a new
   `openssl rand -hex 32`), `PORT=8080`, and, when you add it (next section),
   `DROPPEN_ENROLL_CODE`.
4. **Healthcheck path** `/healthz`. It answers without a token, and says
   nothing but that the proxy is up and whether it has joined:
   `{"status": "up", "enrolled": true}`.
5. In your **backend's** service, point it at
   `http://droppen-session-proxy.railway.internal:8080` with the token — as a
   reference variable (`${{droppen-session-proxy.DROPPEN_SESSION_PROXY_TOKEN}}`),
   so the token is set in one place.

Railway's private network is IPv6; the proxy listens on every address, IPv6
and IPv4. Do not give it a public domain unless your backend is elsewhere:
then put it behind HTTPS, since the token is a Bearer token.

## Joining your account

1. On my.droppen.app, **My devices** → add a device. The page makes an
   `RH1-…` code, good for 10 minutes and one use.
2. Set it as `DROPPEN_ENROLL_CODE` **on the proxy's service only**, and
   deploy. On its first start the proxy makes its identity, claims the code at
   the Hub, keeps its identity in `/data`, and logs its **device fingerprint**:

   ```
   INFO waiting to be added to the account: on my.droppen.app, paste this fingerprint where the page asks for it device=… fingerprint=ABCD-EFGH-…
   ```

3. **Paste that fingerprint into the page.** The page adds the proxy only if
   it is the fingerprint of the device that proved the code; if it does not
   match, or the page says the code was claimed more than once, stop: someone
   else used the code. Make a new one.
4. Choose which computers accept the proxy. The page writes it into the
   account's device chain; the proxy sees itself there, checks that the chain
   names it with its own keys and as a device that runs AI, logs `added to the
   account`, and carries requests from then on.
5. **Remove `DROPPEN_ENROLL_CODE`** from the service. A proxy that has joined
   ignores it and says so in its log, but it should not stay in the
   environment.

Why the code goes on this one service, and goes away: whoever holds the code
within its 10 minutes can claim it before your proxy does. Railway's *shared*
variables are visible to every service of the project — your backend, and all
the code it runs — so the code is set on the proxy's service alone. The
fingerprint you paste is what stops a claim that was not your proxy's: the
page adds only the device whose fingerprint came from your proxy's own log.
After joining, the code is spent, but a secret has no reason to stay where
others can read it.

On each start the proxy checks its data before anything else: it refuses to
run if the account added its device id with other keys, or without the host
flag, and it refuses a `DA1-` code (that one joins an A2A proxy: `droppen
a2a-proxy`).

## Calling it

Every route but `/healthz` needs `Authorization: Bearer
<DROPPEN_SESSION_PROXY_TOKEN>`. `<device>` is the computer's device id
(my.droppen.app shows it).

| | |
|---|---|
| `GET /healthz` | `{"status": "up", "enrolled": bool}`, no token |
| `POST /sessions` | `{device, cwd, text, clientMessageId?, model?, effort?, permission?, accountId?, images?, files?}` → starts a session on `device` → `{id, duplicate?}` |
| `POST /sessions/{id}/message` | `{text, clientMessageId?, images?, files?}` |
| `GET /sessions/{id}/events` | the session's feed as SSE: one `event: <type>` per frame (`hello`, `snapshot`, `delta`, `reset`, `state`, `draft`, …), then `event: closed` when it ends |
| `POST /sessions/{id}/interrupt` | |
| `POST /sessions/{id}/answer` | `{requestId, answers, notes?, response?}`, or `{requestId, dismiss: true}` |
| `POST /rpc` | `{device, method, path, headers?, body?}` — any `/api/…` request straight through; status and `content-type` are returned as they came |

Session ids are the CLI's own, used verbatim; `device` is required on
`/sessions` and `/rpc`. An id the proxy did not start is `404
unknown_session`.

When the proxy cannot get a request through, it answers with a code the
backend can act on — never a 500:

| `error` | HTTP | when |
|---|---|---|
| `host_not_accepting` | 403 | the computer does not accept control from this proxy, or its view lapsed (`why` says) |
| `host_offline` | 503 | the computer is not reachable now |
| `host_stale` | 503 | no device of yours confirmed the account as that computer sees it in 7 days: open Droppen on any of them |
| `device_removed` | 410 | the proxy was removed from the account |
| `channel_failed` | 502 | the channel broke in a way none of the above explains |
| `unknown_session` | 404 | the id is not one this proxy started |
| `not_ready` | 503 | the proxy has not joined its account yet |
| `unauthorized` | 401 | no token, or not the proxy's |
| `not_found` | 404 | no such route |
| `bad_request` | 400 | the body is not what the route takes |
| `answer_too_large` | 413 | a buffered answer over 64 MiB |

An event stream that breaks after it started ends with `event: closed` (with
`error` and `message` in the data); the backend reconnects and starts a new
stream if it wants. It is not resumed for it.

## Removing it

Remove the proxy on my.droppen.app (**My devices**). The computers stop taking
its calls at once; the proxy answers `device_removed` from then on. Then
delete the service and its volume.
