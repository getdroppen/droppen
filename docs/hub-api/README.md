# Droppen hub API, v1

This is the contract between the **open** clients — the local server and its
agent (`server-rs`), the web app (`web/`, which droppen.app also serves, at `my.droppen.app`),
`droppen-relay` (`server-rs/crates/rh-relay`), `rh-proto`, `rh-client` (the
device side, in Rust) and `rh-devkit` —
and the **closed** Droppen Hub (a separate, private repository). Open code may rely
on what is written here and on nothing else of the hub: not its source, its
tables, its environment or behaviour this folder does not describe
(`docs/local-aggregator.md` §2.1).

| file | |
|---|---|
| [`http.md`](http.md) | every HTTP route: auth, bodies, errors, limits |
| [`websocket.md`](websocket.md) | `/v1/client`, `/v1/agent`, `/v1/relay` on the hub and `/v1/connect` on a relay: sign-in, every control message, binary frames, close codes |
| [`schema/http.schema.json`](schema/http.schema.json) | JSON Schema (2020-12) of every HTTP body: `$defs["<METHOD> <path>"].$defs.{request,response,error}` |
| [`schema/ws.schema.json`](schema/ws.schema.json) | JSON Schema of every control message: `$defs["<channel> c2s"]` / `["<channel> s2c"]`, one `$defs["<short>.<dir>.<t>"]` per message |
| [`link.md`](link.md) | `/link`, the web app's sign-in popup a computer's local settings open: the `postMessage` handshake and its rules |
| [`running-the-hub-image.md`](running-the-hub-image.md) | the hub image's run interface open tooling relies on (`cloud-dev.sh`): commands, settings, ports, volumes |
| [`index.json`](index.json) | the machine-readable list of routes and message types |
| [`check.mjs`](check.mjs) | the contract check (below) |

## The hub is not trusted

The architecture assumes a malicious hub (`docs/cross-machine-architecture.md`
§7), and nothing in this API changes that. The hub sees ids, public keys,
signatures, sealed mail, opaque keyring blobs, ciphertext records and pages,
and who talks to whom; it never sees a key or a plaintext. Whatever it says is
checked by the open side before anything depends on it: admission against the
device chain the agent verifies itself, keys against the chain's commitments,
stream heads and manifests against the machine's signature, relay tickets by
the relays offline. A client must treat every field here as a claim. The
hub's checks (who may append, write the keyring, post mail) are hygiene and
abuse control, not security.

## Conventions

- JSON in and out (`Content-Type: application/json`); binary values are
  **lower-case hex**; times are **milliseconds since the Unix epoch**; ids
  (account `a-…`, device `d-…`, machine `m-…`, pairing `p-…`) are 1–64
  printable ASCII characters and otherwise opaque; relay ids are
  `[a-z][a-z0-9-]{0,31}`.
- Objects may carry fields this documentation does not list: **ignore them**.
  The hub ignores unknown request fields too.
- `null` and an absent field are distinct only where a schema says so.
- Where an entitlement value appears, `null` means "no limit" — except
  `GET /v1/relays` `limit` and the agent's `relay_set` `limit`, which carry
  `9223372036854775807` (i64::MAX) for no limit.

## Hosts

| host | serves |
|---|---|
| the canonical machine origin, e.g. `https://api.droppen.app` (sockets on `wss://api.droppen.app`) | `/v1/*` (never redirected: sockets and agents do not follow redirects), `/healthz`, `/install.sh` (the installer of the release the hub was built from), `/dl/*` (the download buttons' redirects), `/.well-known/*`; the pages call it cross-origin rather than being served here |
| `https://droppen.app` (a **temporary alias** of the machine origin during the compatibility window) | the same machine routes as the canonical origin, and the web app's public pages and legacy app redirects while browsers move off it; sockets on `wss://droppen.app` are accepted as an alias of the origin the client dialled |
| the app host (optional, e.g. `https://my.droppen.app`) | the web app (`/` is the app), the `/v1/*` it calls cross-origin on the machine origin, `/dl/*`, `/.well-known/*`, `/healthz`; the public pages are redirected to the site host |
| the site host (optional, e.g. `https://www.droppen.app`) | the web app's public pages (`/`, `/pricing`, `/download`, `/security`, `/privacy`, `/terms`), the `/v1/*` they call cross-origin on the machine origin, the `/dl/*` they link, `/.well-known/*`, `/healthz`; every other page is redirected to the app host |
| the Turnstile host (optional, e.g. `https://verify.droppen.app`) | `GET /turnstile` only, and `/healthz`; everything else is 404 |
| other host names the hub is configured with | `308` to the same path on the origin |
| a relay's origin (open, `droppen-relay`) | `/healthz`, `/v1/ping`, `/v1/relay-id`, `/v1/connect` |

The machine origin and the page hosts may be different names of the same hub
(`api.`, `my.`, `www.` under one domain): the web app is served on the app and
site hosts, and its shell's `rh-hub` marker names the machine origin whose
cross-origin HTTP API the page calls (`wss://api.droppen.app` →
`https://api.droppen.app`). A hub that serves everything on one host answers
with that host as its origin, as it always has.

**Cross-origin (CORS).** A page on the app or site host calls the API on
another origin: `https://my.droppen.app` → `https://api.droppen.app/v1/*`, and
the sockets on `wss://api.droppen.app/v1/{client,agent,relay}`. Sessions are
`Authorization: Bearer` tokens kept in `localStorage`; **there are no
cookies**, and no request sends credentials. Every `/v1/*` response — a
success, an error, and a refused WebSocket upgrade — carries
`Access-Control-Allow-Origin: <the page's origin>` for the hub's configured
page origins (its app host, site host and apex) and never `*`; a preflight
`OPTIONS` is answered with the methods the API uses (`GET`, `POST`, `PUT`,
`DELETE`), the headers `authorization` and `content-type`, and no
`Access-Control-Allow-Credentials`. A request from any other origin gets no
CORS headers and the browser refuses the answer.

A hub with an app host says which of its names does what at
`GET /.well-known/droppen-hub` (http.md), the same answer on every host:
`{origin, app, site?, apex_mode?}`. The web app reads it before anything else
and runs as the app on the app host, as the public pages alone (no key store,
no service worker) on the site host, and, on the hub's own name (the apex),
as before (`apex_mode: app`), as the move notice (`move`: a browser that keeps
a device there is offered to move it to the app host, remove it, or go on
without it; any other is sent on to the app host with its path, query and
fragment), or not at all (`redirect`: the hub sends the origin's pages to the
app host).
A hub that does not answer it serves everything on one host, as before, and
the web app behaves as it always has. Devices that run AI, relays, the
installer and every relay ticket keep the origin: only pages move.

The origin a client signs (`HubLogin.hub`) is the one **it dialled**
(`wss://host[:port]`), never the one the hub announces in its `challenge`.
A page signs in to the hub its shell's marker names, and dials that: the
canonical machine origin (`wss://api.droppen.app`) where the hub has one, or
the host it was served on. The hub takes a browser device's sign-in (`device`
on `/v1/client`, `deviceSession` on `/v1/session`) bound to any of its origin,
app host and site host; every other role is bound to the origin (or its
temporary apex alias).

**WebAuthn only on the app's host** (`docs/passkeys.md` §3.8). Every
response for the host the app is on (the app host, or the hub's own host
when there is none) carries `Permissions-Policy:
publickey-credentials-get=(self), publickey-credentials-create=(self)`;
every response for any other host it answers (the hub's origin once there
is an app host, the site host, the Turnstile host, a redirected name, any
subdomain) and every relay response carries
`publickey-credentials-get=(), publickey-credentials-create=()`. A page
there cannot ask for the app's passkeys, though they are bound to the
parent domain; the hub takes no ceremony made on an origin it does not
list (`https://my.droppen.app`, and `https://droppen.app` while browsers are
still moving off it). Any other host under the relying party id that the hub
does not serve (another `*.droppen.app` site) must send the `=()` policy
itself. The hub refuses to start with its Turnstile page on the relying party
id or a subdomain of it.

## Authentication

| auth (index.json) | how | what it is good for |
|---|---|---|
| `none` | — | sign-up, login (password or passkey, and the passkey routes' options), the device-session exchange, e-mail verification, `GET /v1/releases/latest`, the waitlist, the WebSocket upgrades (which sign in on the socket) |
| `session` | `Authorization: Bearer <token>`, a **login** or a **device** session | `POST /v1/devices`, `GET /v1/me`, `GET /v1/chain`, `POST /v1/email/resend`, `POST /v1/logout` |
| `device` | `Authorization: Bearer <token>` of a **device** session | everything else under `/v1/` — managing the account's passkeys and password, and deleting the account, included (a current full device: a login session never changes how the account signs in) |
| `admin` | `Authorization: Bearer <the operator's admin token>` | `/v1/admin/*`, the operator routes (documented with the Hub) |

**Login session.** `POST /v1/signup` or `POST /v1/login` (email and
password), or `POST /v1/signup/passkey` or `POST /v1/login/passkey` (a
passkey, http.md), gives `{account, token, expires_ms, email_verified,
verification, known_client}` (`known_client`: keep it for this account's
next login, http.md):
a session with no device, good for 15 minutes (at most the hub's session
lifetime) and only for the `session` routes. Its first use is to register
this device's keys: `POST /v1/devices {sign_key, dh_key}` → `{device}`.

**Device session.** Minted by the device's signature, never by another
token:

1. `POST /v1/session/challenge {account, device}` → `{nonce}` (32 bytes).
   Stateless and issued for any well-formed ids, so it says nothing about
   which ids exist; a nonce is good for 2 minutes and answers once.
2. Sign `rh_proto::sig::HubLogin {role: DeviceSession, account, id: device,
   hub: <the origin dialled>, nonce, time_ms}` with the device's Ed25519 key.
3. `POST /v1/session {account, device, time_ms, nonce, signature}` →
   `{account, device, token, expires_ms}`. `time_ms` must be within 5
   minutes of the hub's clock.

A device session lasts the hub's session lifetime (30 days by default). A
device holds its 16 newest sessions, and an account its 16 newest login
sessions: minting one more ends the oldest (`401` on it — renew). Expired
sessions are deleted within the hour.
Removing the device from the chain revokes all of its sessions at once
(`403 device_removed` at step 3; `401` on every route; the sockets close with
4403). Clients renew by repeating the exchange (the web app does so before
expiry and once on a `401`).

**Suspended accounts.** The hub's operator may suspend an account (abuse
control; the operator routes). While it is suspended, `POST /v1/login`,
`POST /v1/login/passkey` and `POST /v1/session` answer `403
account_suspended` — only once the password, the passkey or the device's
signature checked out, so the answer says nothing to anyone else — every
session of the account is `401 unauthorized` on every route, its sockets
close with 4403 after `error {code: "account_suspended"}` (websocket.md),
and none of its devices or agents signs in until the operator lifts it.
Nothing of the account is deleted. A
**deleted** account (`DELETE /v1/account`, http.md) is gone: its sessions
end, its sockets close the same way (`account_deleted`), and `POST
/v1/session` answers `410 account_deleted` to a device of it whose signature
checks out (http.md) — never to an account the hub merely does not know; a
passkey sign-in in the moments its deletion takes is `403 account_deleted`
(its address is gone at once: a password one is `401 bad_login`).

**Where the device stands** matters beyond the session: `requires: full`
routes need a *current full device* of the chain (`403 not_a_full_device`),
`requires: current` ones any current device (`403 not_in_chain` /
`forbidden`). A registered device outside the chain may still read its own
mail, open a pairing for itself and read the keyring.

**Sockets** sign in on the socket itself (websocket.md): a device with a
`HubLogin {role: Device}` signature plus its device session token, an agent
with `{role: Agent}` (or enrolls with a one-time token), a relay with
`{role: Relay}` under its pinned key; on a relay, a device or agent with
`{role: RelayDevice | RelayAgent}` and a hub-signed ticket.

## Errors

Every hub error is JSON with a stable code and a message for people:

```json
{"error": "limit", "message": "this account has 3 machines …", "key": "machines.max", "limit": 3, "used": 3}
```

| field | when |
|---|---|
| `error` | always: the code a client switches on |
| `message` | always (hub): human text, may change |
| `retry_after_ms` | on `429`; the `Retry-After` header carries the same in seconds |
| `key`, `limit`, `used` | on `409 limit`: the entitlement (`devices.max`, `machines.max`, `storage.bytes`, `relay.max`, …), its value (`null`: none) and the current use |
| `grant` | on `409 unlock_on` from `DELETE /v1/passkeys/{id}` after a password or passkey proof: send it back as `{grant}` once the passkey slot is off (http.md) |
| `until_ms` | on `409 recently_deleted` from the sign-up routes: when the address may sign up again (http.md, `POST /v1/signup`) |

The codes, by status (http.md lists them per route):

| status | codes |
|---|---|
| 400 | `bad_request` (a malformed or too-long field; `message` names it), `disposable_email`, `bad_token`, `confirm_mismatch` |
| 401 | `unauthorized` (no, unknown or expired session), `bad_login`, `bad_passkey`, `unknown_passkey`, `denied` (device session exchange), `rotated` (device session exchange signed with a key an identity key renewal retired), `device_session_required` |
| 403 | `passkeys_off`, `not_a_full_device`, `not_in_chain`, `forbidden`, `device_removed`, `email_unverified`, `signup_closed`, `bad_invite`, `turnstile_required`, `turnstile_retry`, `turnstile_failed`, `bad_key_name`, `pairing_window_closed`, `not_the_creator`, `host_devices_off`, `rotation_pending` (a host's device role while its identity key is being renewed), `isolated` (relay tickets of an isolated host), `reauth_required`, `reauth_failed`, `account_suspended`, `account_deleted` (a sign-in while the account is being deleted) |
| 404 | `not_found`, `no_such_renewal` |
| 408 | `slow_request` (a route that needs no sign-in got its body too slowly: within 10 s, or not at all; the connection is closed) |
| 409 | `limit`, `chain_conflict`, `chain_v3_not_ready`, `rotation_not_ready` (with `reason`, and `waiting` or `ready_ms`), `rotation_in_progress`, `exists`, `email_taken`, `no_chain`, `verification_off`, `already_verified`, `not_a_host`, `refused` (admin), `too_many_passkeys`, `passkey_exists`, `last_credential`, `unlock_on`, `no_passkey`, `recently_deleted` (with `until_ms`) |
| 410 | `expired` (e-mail link), `host_removed` (a dispute statement for a device that left its account), `account_deleted` (`POST /v1/session`, signed by a device of an account being deleted, or deleted within the hold) |
| 422 | `invalid_entry` (a chain entry the hub's verification refused), `chain_v2_genesis` (new accounts are v3 only: a v2 genesis is refused), `invalid_dispute` |
| 429 | `rate` (with `retry_after_ms`), `mail_rate`, `device_rate`, `too_many_pending_devices`, `too_many_enrollments`, `too_many_pairings`, `pairing_full` |
| 500 | `internal` |
| 503 | `unavailable` (database, page store), `turnstile_unavailable`, `release_unavailable` (`GET /v1/releases/latest` before the hub has read a release) |
| 507 | `mailbox_full`, `keyring_full` |

Treat an unknown code by its status. **Not every error body is JSON**: the
web framework answers some malformed requests itself, before the hub's code
runs — a body that is not JSON or lacks a required field (400/415/422,
`text/plain`), a body over the route's cap (413, `text/plain`), a method the
path does not take (405, empty) — and with the web app configured, an unknown
`/v1/` path is a `text/plain` 404. A client must cope with a non-JSON error
body (the web app reads it as `{error: "error"}` and keeps the status).
Relays answer `429 {error: "rate", retry_after_ms}` without a `message`.

## Without Turnstile

A hub may run without Turnstile (its operator's choice). The web app draws
no widget either way. No route then asks for it — there is no
`403 turnstile_required` a client could not satisfy — and what Turnstile
guarded is held by limits alone: sign-up and resends per address and per
network, resends per account and per inbox, hub-wide caps on sign-ups and
verification mail, and login backoffs answered with `429 rate` and
`retry_after_ms` (below), which a client that signed in before skips with its
`known_client` token (http.md, `POST /v1/login`; a device in use gets a fresh
one with every `GET /v1/me`).

What this does not cover: a client **without** a valid token — a new
browser, cleared storage, a device unused for 30 days — can be kept from
signing in with the password for as long as an attacker keeps restarting the
e-mail's backoff from fresh networks (about 17 requests an hour). The way
in for such a client is a passkey (`POST /v1/login/passkey`), which none of
the password's backoffs ever holds, or a mailed link (planned).

## Rate limits and caps

Limits are token buckets ("n per period, sliding") keyed by client address
(behind the hub's proxy, the address it reports), by device, or by account.
A refused request costs nothing and leaves nothing behind.

| what | limit |
|---|---|
| every unauthenticated route (incl. the WebSocket upgrades and the operator routes), but the static ones below | 300 a minute per address, bursts of 60; a body (if any) read whole within 10 s before anything else, else `408 slow_request` and the connection closed |
| the static and stateless routes: `/healthz`, `/install.sh`, `/.well-known/droppen-build`, `GET /v1/releases/latest`, the web app's files and any path nothing else serves, the Turnstile host's page and its 404s, a redirected host's `308` | 600 per 10 s and 3,000 a minute per address (a browser loads some 40 files) |
| argon2 (`POST /v1/login`, `POST /v1/signup`) | one hash at a time per address, two at a time per network (IPv4 /24, IPv6 /48), four across the hub: one source never holds every slot; the rest wait their turn |
| sign-up attempts: `POST /v1/signup`, `POST /v1/passkeys/options {signup}` | every attempt, account made or not (each can say whether an e-mail is taken): 10 an hour and 30 a day per address, spent first (counted twice while Turnstile is unreachable and the hub fails open); and, only once the attempt passed every cheap check (mode, the e-mail's syntax and domain, the invite, Turnstile) — just before the e-mail is looked up — 20 an hour and 50 a day per network (IPv4 /24, IPv6 /48) and 200 an hour and 1,000 a day across the hub. Junk spends only its own address's share |
| accounts made: `POST /v1/signup`, `POST /v1/signup/passkey` | 5 an hour and 20 a day per address, counted only when an account is made (by either route: a cancelled passkey prompt or a refused sign-up spends none). Without Turnstile also 20 an hour and 50 a day per network and 200 an hour and 1,000 a day across the hub, and none while the hub's verification mail is spent (below) |
| `POST /v1/login` | 30 per 10 minutes per address; 5 failures for one e-mail from one address (IPv4 as a whole, IPv6 by its /64) lock that pair for 5 minutes; 3 failures for an e-mail (or 10 from one IPv4 /24 or IPv6 /48) within an hour ask for Turnstile (`403 turnstile_required`), which never locks anyone. A hub without Turnstile backs off instead (`429 rate` with `retry_after_ms`; hours slide): past 10 failures of an e-mail in the last hour (at most 5 counted from any one IPv4 /24 or IPv6 /48 — one machine handed a /48 is one network), its next login waits 2 s, doubling with each further failure up to 5 minutes; and past 5 failures from one /24 (or /48) at an e-mail, that network's next try waits 2 s, doubling up to 15 minutes. A login with a valid `known_client` token for the account skips both, until that token has failed 3 times in the last hour. A try counts as a failure from the moment it is checked (a right password, or a request cancelled before its answer, takes it back), so tries that arrive together share one opening. An e-mail with no account is counted, locked and answered exactly like one with an account |
| `POST /v1/passkeys/login/options` | 60 a minute per address |
| `POST /v1/login/passkey` | 30 per 10 minutes per address, a bucket of its own; a network's (IPv4 /24, IPv6 /48) **failed** assertions 120 per 10 minutes, past which its failures are answered `429 rate` — a valid assertion is never held by it; 30 valid logins per 10 minutes per passkey (`429 rate`), spent only by that passkey's own valid assertions. No lockout, no backoff, no known-client budget, no Turnstile, and nothing it does counts towards the password login's |
| `POST /v1/passkeys` (adding a passkey) | 10 a day per account (`429 rate`), spent only by a passkey that checked out; at most 10 passkeys at once (`409 too_many_passkeys`) |
| the current password, given as proof it is still the person (`POST /v1/passkeys/add/options`, `DELETE /v1/passkeys/{id}`, `PUT`/`DELETE /v1/password`, `DELETE /v1/account`) | 3 checks per 10 minutes and 10 a day per device, then 6 per 10 minutes and 20 a day per account (`429 rate`), every check, both before the hash: one device never spends the account's whole share; argon2 as for a login |
| `POST /v1/session` | 30 a minute and 300 an hour per device (`429 rate`), spent only by that device's own valid signature — never by a made-up one naming it, nor by another device of the account |
| `POST /v1/email/verify` | 30 a minute per address |
| `POST /v1/waitlist` | every attempt, junk included: 5 an hour and 20 a day per address, spent first; once the attempt passed the cheap checks (plan, the address's syntax and domain), just before it is written: 20 an hour and 100 a day per network (IPv4 /24, IPv6 /48). Across the hub past 1,000 a day the hub only logs it (once an hour) and keeps taking them, so no one source can close the list |
| `POST /v1/email/resend` | 10 an hour per address; 1 a minute and 5 a day per account; 5 an hour and 10 a day per inbox across accounts (an address with its `+tag` folded off, and at Gmail its dots too, `googlemail.com` counting as `gmail.com`; a new account's first mail always goes, and counts towards it). Without Turnstile also 30 an hour per network, and verification mail of every kind 300 an hour and 2,000 a day across the hub |
| `POST /v1/devices` | 10 an hour per address; 10 a day per account (`429 device_rate`; a host's own device row is not one); 3 registered devices outside the chain at once (`429 too_many_pending_devices`), each counted only within its 30-minute pairing window and deleted a day after it registered |
| `POST /v1/enrollments` | 10 an hour per device; 5 waiting per account (`429 too_many_enrollments`); `machines.max` counts waiting ones (`409 limit`); a code lives 10 minutes |
| `POST /v1/relays/tickets` | 120 an hour per device |
| `PUT /v1/rotations/{host}` | 6 renewal attempts an hour and 20 a day per host (`429 rate`; marking a host pending is not one); `host_dispute` pushes, and retired-key evidence kept, 4 an hour per host |
| `POST /v1/rotations/{host}/disputes` (no session) | 60 an hour per address; 10 an hour per host, counting only statements that verify (`429 rate`) |
| `POST /v1/mailbox` | 120 a minute per sending device, sliding (`429 mail_rate`); per recipient **mailbox role** (a host's agent: purpose 2; any device role: the rest — counted apart, so neither fills the other's) 256 mails and 1 MiB, one sender at most 128 of them, mail that delivers no key at most 192 and 512 KiB (`507 mailbox_full`); non-key mail counts against `storage.bytes` (`409 limit`) |
| pairings | 4 open per account (`429 too_many_pairings`), 8 messages each (`429 pairing_full`), 10 minutes; a device outside the chain holds 1, within 30 minutes of registering |
| keyring | one name per writer per committed epoch plus 64, at most 10,000 names per account (`507 keyring_full`) |
| WebSocket upgrades (`/v1/client`, `/v1/agent`, `/v1/relay`, `/v1/probe/rooms/{room}`) | 60 a minute per address |
| every route with a session | 600 per 10 s and 3,000 a minute per address before the session is looked up; 600 a minute per device (or login session), snapshot pages excepted |
| relay `/v1/ping`, `/v1/relay-id` | 120 a minute per address |
| relay `/v1/connect` upgrades | 60 a minute and 20 per 20 s per address |

| body cap | routes |
|---|---|
| 8 KiB | unauthenticated routes and the operator routes |
| 16 KiB | the four passkey routes, and adding, renaming or removing a passkey, setting the password and deleting the account (`/v1/passkeys`, `/v1/passkeys/*`, `/v1/password`, `/v1/account`) |
| 256 KiB | `/v1/chain` (1–64 entries per append) |
| 20 KiB | `/v1/mailbox` (`sealed` ≤ 8 KiB) |
| 16 KiB | `/v1/keys/*` (a blob ≤ 4 KiB) |
| 64 KiB | every other route with a session |

Field caps: tokens ≤ 128 bytes; ids 1–64 printable ASCII; e-mail ≤ 254
bytes; password 10–1024 bytes; invite ≤ 128 bytes (an admin invite's note
≤ 256 bytes, its days ≤ 3650); Turnstile token ≤ 2048;
`lang` ≤ 16; keyring names `[a-z0-9._/-]{1,128}` without `..`; pairing
message ≤ 4 KiB; relay tickets: 1–8 relays a request, at most 2 emergency
tickets. WebSocket limits are in websocket.md.

## Versioning

- **The path is the version.** Everything here is `/v1/`; the frame
  envelope is versioned by the same paths (`rh_proto::frame::CLIENT_PATH`,
  `AGENT_PATH`).
- **Within `/v1/` the API only grows**: new routes, new optional request
  fields, new response fields, new control message types, new error codes,
  new frame types `≥ 0x80` (skippable). Clients ignore what they do not know:
  unknown fields, unknown `t` values, unknown ignorable frames. A field's
  type and meaning, a required request field, a route or a message type are
  never changed or removed within `/v1/`.
- **Features are negotiated, not assumed**: an agent says what it speaks in
  `agent_info.features` (`relays/1`, `chain/3`, `policy/1`, `rotate/1`), and the hub tells devices
  (`presence.relays`, `GET /v1/me` `machines[].rotates`); the hub says what it
  speaks in `welcome.features` (`rotate/1`), and a peer sends a feature's
  messages only where it was offered (a hub without it answers an unknown `t`
  with 4400); what the hub offers an account shows in `GET /v1/me`
  (its `plan`, `email.verification`). A client
  tests for a field's presence rather than a hub version.
- **The hub keeps working with the previous N−2 released versions of the open
  clients** (the current release and the two before it): a change that would
  break one of them waits, or ships behind negotiation.
- **Breaking changes go to `/v2/`**, served next to `/v1/` for at least that
  window.
- **The device chain is the exception that cannot roll back.** Chain entries
  carry their own version byte (v2, v3). The hub serves whatever the chain
  holds, and starts an account on v3 only with `DROPPEN_HUB_CHAIN_V3=write` and
  only when its every current host declared `chain/3` and `policy/1`
  (`GET /v1/me` `chain_v3`). An account already on v3 keeps working in every
  mode; only adding a host to it waits for `write` (`POST /v1/chain`). Once an account holds one v3 entry, a client
  that reads only v2 cannot verify that chain any more — it is append-only and
  hash-linked, nothing deletes the entry — so **no reader (agent, web app)
  may be rolled back below the first release that reads v3**. The web app
  makes every new account on v3 from its genesis (`Genesis3`), so no new
  account can be made on a hub that does not write v3. Going back to
  `read` stops new v3 accounts and new hosts on v3 accounts; it does not undo
  a v3 entry, and an account already on v3 carries on (`off` also stops
  hosts acting as devices)
  (`rh_proto::chain` module docs; the test
  `c16_once_a_v3_entry_lands_readers_cannot_go_back_below_v3`). Likewise a
  host's identity key renewal (`RotateHostKey`, kind 20), the account's
  renewal policy (`SetRenewalPolicy`, kind 21) and the pause of control over
  hosts while they are renewed (`RenewalPause`, kind 22): the hub takes one only with
  `DROPPEN_HUB_HOST_ROTATION=on` and once every current host declared
  `rotate/1`; after the account's first, no reader may be rolled back below
  the first release that reads kinds 20 to 22 (switching the hub back to
  `off` stops new ones, it does not undo one). The end of a pause the chain
  holds open (`completed`, `cancelled`) passes the gate and the switch: it
  starts nothing, and every reader of that chain reads kind 22 already.
- **Every change to the hub API updates this folder in the same round**: the
  closed change implements, the open change documents (here) and adapts the
  clients; `check.mjs` must pass.

## The contract check

```sh
node docs/hub-api/check.mjs            # or: pnpm check:hub-api
node docs/hub-api/check.mjs --list     # also print every route and `t` found, with where
node docs/hub-api/check.mjs --traffic /tmp/traffic.jsonl
```

It scans the open clients (`web/src/lib/hub`, `web/src/lib/e2e`,
`web/src/features/cloud`, `server-rs/src/agent`, `server-rs/src/aggregator`,
`rh-relay`, `rh-client`, `rh-devkit`) for
`/v1/…` paths in string literals (format strings and template literals
included, their placeholders read as path parameters) and for control
message types — `{t: "x"}` and `json!({"t": "x"})` sent, `.t === "x"`,
`switch (m.t) { case "x" }`, `v["t"] == "x"`, `match v["t"].as_str() {
Some("x") }` and `#[serde(tag = "t")]` enums matched — and fails when one is
missing from `index.json` on a channel that file speaks, in the right
direction. It also checks that `index.json` and both schemas list the same
routes and messages, and that the schemas only use the keywords its small
validator implements. A short, commented ignore list in the script holds the
few things that look like hub API and are not.

**Traffic.** With `DROPPEN_HUB_API_TRAFFIC=<file>` set (`RH_HUB_API_TRAFFIC`
before the rename to Droppen, deprecated: it stops working in v0.2.135),
`rh-client` (its HTTP
helpers and `/v1/client` / `/v1/connect` sockets, `"src": "client"`),
`rh-devkit` (the echo agent), the agent (`/v1/agent`, its relay connections,
and its device role through `rh-client`, with device session tokens
redacted) and `droppen-relay` (its link to the hub) append every JSON body
they exchange with the hub or a relay to that file, one line each:

```json
{"src": "client", "kind": "http", "dir": "out", "method": "POST", "path": "/v1/session", "body": {…}}
{"src": "agent", "kind": "ws", "socket": "/v1/agent", "dir": "in", "body": {"t": "welcome", …}}
```

`--traffic` validates every record against the schemas (`out`: a request or
a `c2s` message; `in`: a response, an error or an `s2c` message).
Setting it for a whole end-to-end run (the clients and the relays together)
captures everything they exchange; `--traffic` then checks it all. The file holds what
went over the wire, test credentials and tokens included: delete it after.
