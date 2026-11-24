# Running the A2A proxy

`droppen a2a-proxy` lets an A2A client that cannot reach your computers — a
service in the cloud, such as dont-do on Railway — start and follow coding
sessions on them. It is a device of your Droppen account of its own kind (an
*A2A proxy*): it takes standard A2A v1.0 requests over HTTP and carries each
over an end-to-end encrypted channel, through a relay that only sees
ciphertext, to the A2A server of the computer you name
([a2a.md](a2a.md) §7, in Chinese):

```
A2A client ──A2A over HTTP (private network)──▶ droppen a2a-proxy ──Noise channel, via a relay──▶ your computer: /a2a/*
```

The computer decides what it serves: its A2A entries, and which A2A proxies
each lets in. The proxy can do nothing else: it cannot control a computer,
read your sessions or hold any of your keys, and the Hub never sees a
request.

## Before you start

- Every computer that runs AI in the account, and the desktop app, must run a
  version that knows A2A proxies; "Add an A2A proxy" on my.droppen.app lists
  the ones that do not, and waits for them.
- The proxy needs a **persistent volume**: its identity (its keys) and its
  verified copy of the account's device chain live there. Losing it means a
  new proxy: remove the old one on my.droppen.app and add it again.

## The image

Each release has an image, built from that release's attested Linux binary,
for `amd64` and `arm64`:

```sh
docker pull ghcr.io/getdroppen/droppen-a2a-proxy:vX.Y.Z
gh attestation verify oci://ghcr.io/getdroppen/droppen-a2a-proxy:vX.Y.Z --repo getdroppen/droppen
```

Or build it from the repository root: `docker build -f
crates/droppen-a2a-proxy/Dockerfile .`. The image runs `droppen a2a-proxy` as
an unprivileged user (uid 10001), keeps its data in `/data` (a volume), listens
on `PORT` (8080), and checks its own health (`droppen a2a-proxy --health`).

## Configuration

All of it is the environment:

| variable | |
|---|---|
| `DROPPEN_A2A_PROXY_TOKEN` | required: the Bearer token your A2A client presents, at least 32 characters (`openssl rand -hex 32`). The proxy compares it in constant time and never logs it |
| `DROPPEN_DATA_DIR` | the persistent volume (the image sets `/data`) |
| `DROPPEN_ENROLL_CODE` | the **first start only**: the `DA1-…` code from my.droppen.app that joins the proxy to your account. Remove it once the proxy has joined (below) |
| `DROPPEN_HUB` | the Hub (default `https://api.droppen.app`, Droppen's) |
| `PORT` | where to listen (default 8080) |
| `DROPPEN_A2A_PROXY_BASE` | optional: the proxy's address as your client reaches it (`http://droppen-a2a-proxy.railway.internal:8080`), for the Agent Card's interface addresses; unset, the request's `Host` over `http` |
| `DROPPEN_A2A_PROXY_ALIASES` | optional: `name=device,…`, names to reach computers by besides their device ids |
| `RUST_LOG` | the log filter (default `info`) |

## On Railway, beside your client

Run the proxy as a second service in the project of your A2A client, on
Railway's private network, with no public domain:

1. **New service** → **Docker Image** → `ghcr.io/getdroppen/droppen-a2a-proxy:vX.Y.Z`
   (or **GitHub Repo** with the Dockerfile path
   `crates/droppen-a2a-proxy/Dockerfile`). Name it, say,
   `droppen-a2a-proxy`.
2. **Add a volume** to it, mounted at `/data`. Railway mounts volumes as
   root; if the proxy's log says it cannot write `/data`, set
   `RAILWAY_RUN_UID=0` on this service (Railway's switch for images that run
   as another user).
3. **Variables** of this service: `DROPPEN_A2A_PROXY_TOKEN` (a new
   `openssl rand -hex 32`), `PORT=8080`, and, when you add it (next section),
   `DROPPEN_ENROLL_CODE`. Optionally
   `DROPPEN_A2A_PROXY_BASE=http://droppen-a2a-proxy.railway.internal:8080`.
4. **Healthcheck path** `/healthz`. It answers without a token, and says
   nothing but that the proxy is up and whether it has joined:
   `{"status": "up", "enrolled": true}`.
5. In your **client's** service, point it at
   `http://droppen-a2a-proxy.railway.internal:8080` with the token — as a
   reference variable (`${{droppen-a2a-proxy.DROPPEN_A2A_PROXY_TOKEN}}`), so
   the token is set in one place.

Railway's private network is IPv6; the proxy listens on every address, IPv6
and IPv4. Do not give it a public domain unless your client is elsewhere: then
put it behind HTTPS, since the token is a Bearer token.

## Joining your account

1. On my.droppen.app, **My devices** → **Add an A2A proxy**. The page makes a
   `DA1-…` code, good for 10 minutes and one use.
2. Set it as `DROPPEN_ENROLL_CODE` **on the proxy's service only**, and deploy.
   On its first start the proxy makes its identity, claims the code at the
   Hub, keeps its identity in `/data`, and logs its **device fingerprint**:

   ```
   INFO waiting to be added to the account: on my.droppen.app, paste this fingerprint where the page asks for it device=… fingerprint=ABCD-EFGH-…
   ```

3. **Paste that fingerprint into the page.** The page adds the proxy only if
   it is the fingerprint of the device that proved the code; if it does not
   match, or the page says the code was claimed more than once, stop: someone
   else used the code. Make a new one.
4. Choose which computers accept the proxy. The page writes it into the
   account's device chain; the proxy sees itself there, checks that the chain
   names it with its own keys and as an A2A proxy, logs `added to the account
   as an A2A proxy`, and carries requests from then on.
5. **Remove `DROPPEN_ENROLL_CODE`** from the service. A proxy that has joined
   ignores it and says so in its log, but it should not stay in the
   environment.

Why the code goes on this one service, and goes away: whoever holds the code
within its 10 minutes can claim it before your proxy does. Railway's *shared*
variables are visible to every service of the project — your client, and all
the code it runs — so the code is set on the proxy's service alone. The
fingerprint you paste is what stops a claim that was not your proxy's: the
page adds only the device whose fingerprint came from your proxy's own log.
After joining, the code is spent, but a secret has no reason to stay where
others can read it.

On each start the proxy checks its data before anything else: it refuses to
run if the account added its device id with other keys, or without the A2A
proxy flag, and it refuses an `RH1-` or `DR1-` code (those add a computer that
runs AI: `droppen agent enroll`).

## Calling it

On each computer you want to reach, create an A2A entry and allow the proxy
on it (the computer's A2A settings). Then, from your client, with
`Authorization: Bearer <DROPPEN_A2A_PROXY_TOKEN>`:

| | |
|---|---|
| `GET /a2a/<device>/agents/<entry>/.well-known/agent-card.json` | the entry's Agent Card; its interface addresses are the proxy's |
| `POST /a2a/<device>/agents/<entry>/jsonrpc` | JSON-RPC |
| `/a2a/<device>/agents/<entry>/rest/…` | HTTP+JSON |

`<device>` is the computer's device id (my.droppen.app shows it) or a name
from `DROPPEN_A2A_PROXY_ALIASES`. Streams (`message:stream`, `SubscribeToTask`)
come through as they are sent, for as long as they run; one cut short — a
relay moving, a computer restarting — is not resumed by the proxy: subscribe
again (`SubscribeToTask`) or poll the task.

When the proxy cannot get a request through, it answers in the binding's own
error form with a `google.rpc.ErrorInfo` of the domain `droppen.app`
([a2a.md](a2a.md) §3.5, §7.4):

| `metadata.code` | HTTP | when |
|---|---|---|
| `droppen.device_offline` | 503 | the computer is not reachable now |
| `droppen.attestation_stale` | 400 | no device of yours confirmed the account as that computer sees it in 7 days: open Droppen on any of them |
| `droppen.not_accepted` | 403 | the computer does not accept this proxy |
| `droppen.device_removed` | 403 | the proxy was removed from the account |
| `droppen.quota_exceeded` | 400 | a request body over 10 MiB |

Before the proxy has joined its account, every request is answered 503.

## Removing it

Remove the proxy on my.droppen.app (**My devices**). The computers stop taking
its calls at once; the proxy answers `droppen.device_removed` from then on.
Then delete the service and its volume.
