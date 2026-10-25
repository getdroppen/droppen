# HTTP routes

Every route the hub serves, and the four a relay serves. Conventions, the
auth model, the error shape and the limits are in [README.md](README.md); the
exact bodies are in [`schema/http.schema.json`](schema/http.schema.json)
under `$defs["<METHOD> <path>"]`. "Errors" lists the codes a route itself
returns; any route may also answer `429 rate`, `503 unavailable`, and the
framework's non-JSON 400/413/415/422; a route that needs no sign-in also
`408 slow_request` when its body takes longer than 10 s to arrive.

Audience: **client** routes are what devices and agents use; the **operator**
routes (`/v1/admin/*`) are documented with the Hub, not here; **relay** and
**tooling** routes are marked.

## Public (no session)

### `GET /healthz`
Liveness: `200 ok` (text). Also served on the Turnstile host. Limits: the
static routes' 600 per 10 s and 3,000 a minute per address (README).

### `GET /install.sh` — tooling
`200`, `text/plain; charset=utf-8`: the server installer for macOS and Linux,
the `scripts/install.sh` of the release of `getdroppen/droppen` the hub was
built from, compiled into the hub — byte for byte that release's `install.sh`
file (attested, and never changed once published: docs/releasing.md). This is
what `curl -fsSL https://droppen.app/install.sh | sh` runs. The hub serves it
rather than redirect to the latest release's file, so that whoever could
replace a release's files could not replace the installer, nor the release
key it pins (docs/releasing.md, "The release key"); `/.well-known/droppen-build`
says its sha256 (`install_sh`). `Cache-Control: no-cache` with
`ETag: "sha256-<hex>"` (`304` to a matching `If-None-Match`) and
`X-Content-Type-Options: nosniff`; `HEAD` the same without the body; other
methods `405`. The installer downloads a release from that repository and
checks it against the release's signed checksums before installing anything.
The hub serves no binary. A hub built without an installer on purpose answers
`302 Found` to
`https://github.com/getdroppen/droppen/releases/latest/download/install.sh`
instead, with `Cache-Control: no-store` and no body. Limited like `/healthz`
(the static routes' share per address); served on every host of the hub but
the Turnstile host (a 404 there), never redirected.

### `GET /turnstile?action=signup|login|resend[&theme=dark]` — Turnstile host only
An HTML page that renders Cloudflare's widget and hands its result to the app
with `postMessage`, aimed at the app's origin only. The app frames it (its
CSP gains `frame-src` for this origin and never loads Cloudflare's script
itself). Messages (schema: ws.schema.json `/turnstile`):

| direction | message |
|---|---|
| page → app | `{t: "turnstile", action, token}` on success; `{t: "turnstile", action, token: null, expired: true}` or `{…, error: true}` |
| app → page | `{t: "turnstile-reset"}`: render a fresh challenge |

The page, and the 404 for anything else on that host, count against the
static routes' share per address, as does a redirected host's `308`.

### `GET /.well-known/droppen-build` — only when the hub serves the web app
→ `{commit: null | "<hex>", repo?: "getdroppen/droppen", web: {digest: "sha256:<hex>", files}, install_sh?: {digest: "sha256:<hex>"}}`, with
`Cache-Control: no-store` and `Access-Control-Allow-Origin: *`. Which build of
the web app this hub serves: the commit of this repository its web app was
built from (`null` when the hub does not know it; `repo` names the repository
whenever `commit` is set, and a reader trusts a commit only with it) and the
digest of the files as built — one
`<sha256>  <path>\n` line per file, sorted by path bytewise, hashed; the shell
before the hub adds its `rh-hub` marker. A release's `web-manifest.json` has
the same `digest`; `scripts/verify-web.mjs` compares it with the latest
release's manifest (naming the earlier release that has it, if one does) and
then checks the served bytes themselves ([web-integrity](../web-integrity.md)).
`install_sh`, when there, is the sha256 of the installer `GET /install.sh`
serves (a release's `install.sh`, which the release's `SHA256SUMS` does not
list but its attestation covers). A claim like everything else here, not a
proof. It, the web app's own files
and any path nothing else serves share the static routes' limit per address.
Served the same on every host of the hub that serves the web app (the origin,
the app host, the site host), as is `/.well-known/security.txt`.

### `GET /.well-known/droppen-hub` — which hosts this hub serves
→ `{origin: "wss://<host>", app: "https://<host>", site?: "https://<host>" | null, apex_mode?: "app" | "move" | "redirect"}`.
`origin` is the hub as agents, relays and the agent's own sign-in dial it
(`wss://droppen.app`); it does not move. `app` is where a browser signs in,
pairs and uses the account (`https://my.droppen.app`), `site` where the
landing, download and policy pages are (`https://www.droppen.app`). A hub that
serves everything on one host may have no such document (a 404, or the web
app's shell as for any path); a client then uses the hub's own origin for
everything. `apex_mode` says what the hub's own origin does with the web
app's pages while `app` is another host: it still serves the app there
(`app`, also when absent: the move has not started), shows the move notice in
its place (`move`: a browser that keeps a device there is offered to move it
to `app`, remove it, or go on without it; any other is sent on to `app` with
its path, query and fragment), or redirects them to `app` (`redirect`). The
same answer on every host the hub serves.

The web app reads it before anything else of a hub's page starts
(`web/src/lib/hub/hosts.ts`, README "Hosts"): on `app` it is the app (`/`
included), on `site` only the public pages (no key store, no service worker;
every way into the app a link to `app`), on the hub's origin as `apex_mode`
says. The answer is not part of the attested build, so the page holds it to
the hub's names: `origin` a `wss://` name of two labels or more with no port,
the page on it or under it, and `app` and `site` `https://` names under it at
a label boundary (or `origin`'s own host), with no port — `ws://`, `http://`
and ports only for a hub on the loopback, with a loopback page. Any other
answer — none, a 404, the shell, a body that is not this, a name outside the
apex, no answer within 8 seconds — leaves it on one host, exactly as before.

The local server reads it to know where the sign-in popup and the browser
hand-off open ([link](link.md#which-host-the-popup-and-the-hand-off-open)): it
takes `app` only when it is a plain `https` origin (`http` only for a loopback
test hub) whose host is the hub's own or a subdomain of it, and `origin`,
when given, is the hub it dialled; it does not follow a redirect. It keeps an
answer for ten minutes.

### A service worker fetched from any path but `/sw.js` — only when the hub serves the web app
A `GET` carrying `Service-Worker: script` (the browser fetching or updating a
service worker) for any path other than `/sw.js` — or for `/sw.js` too on a
host that does not run the app (the site host, and the hub's origin once an
app host runs it: README, "Hosts"), before any redirect — gets a small worker that
deletes every cache of the origin and unregisters itself, as
`text/javascript; charset=utf-8` with `Cache-Control: no-store`, instead of
the shell. A worker registered for `/` from another address by a page that
should not have would otherwise stay: the shell fails the browser's update on
its MIME type ([web-integrity](../web-integrity.md)). The local server answers
the same.

### `GET /v1/signup/config`
→ `{signup: "invite"|"open"|"closed", invite_required, turnstile: null | {site_key, origin}, verification: "required"|"off", passkeys: null | {rp_id}, deletion: {email_hold_days}}`.
What the sign-up form must ask for. `passkeys`: the hub offers passkeys
for this relying party id (`null`: it has them off — a hub on an IP
address always does; absent from hubs before passkeys). `deletion`:
`email_hold_days` (≥ 0), how many days a deleted account's address cannot
sign up again (`DELETE /v1/account`; `0`: it can at once), for the page to
say before an account is deleted; absent from hubs before account deletion.

### `GET /v1/releases/latest`
→ `{version, tag, published_at, releases_page, platforms: {macos?, "linux-x86_64"?, "linux-aarch64"?}, fetched_ms}`,
each platform `{name, url, size, download_path?}`. This is the latest public release of
`getdroppen/droppen`, for the download buttons. The hub reads it from
GitHub's API itself, once at start and then every 15 minutes, so the page
asks GitHub for nothing: its CSP's `connect-src` stays the hub and the
relays.

The hub takes only a release that is published, not a prerelease, and
tagged `vX.Y.Z` (`version` is `X.Y.Z`). It takes only these files, under
their exact names:

| platform | file |
|---|---|
| `macos` | `Droppen-X.Y.Z.dmg`, the desktop app (universal) |
| `linux-x86_64` | `droppen-vX.Y.Z-linux-x86_64.tar.gz` |
| `linux-aarch64` | `droppen-vX.Y.Z-linux-aarch64.tar.gz` |

Each `url` is exactly
`https://github.com/getdroppen/droppen/releases/download/vX.Y.Z/<name>`, and
`size` is in bytes. A platform whose file the release lacks is absent.
`releases_page` is the release's page on GitHub (notes, `SHA256SUMS`, every
other file), made from the tag. The hub serves its last good answer
whatever later reads fail with; `fetched_ms` says when it read it. The
answer carries `Cache-Control: public, max-age=300`. The hub never takes an
answer that names an older version than its last good one. The route is
limited like `/healthz` (the static routes' share per address).

`download_path`, on a hub that counts downloads, is `/dl/<platform>` for
the file's own platform: the hub's `GET /dl/{platform}` below. It is a path,
relative to the host the page is on: the hub serves `/dl/*` on every host
it serves pages on, so a page links it on its own host. Hubs before it name
none.

A client links the files: a plain `<a href>`, which downloads in place —
`download_path` when the hub names exactly `/dl/<platform>` for that file
(with `rel="nofollow"`: crawlers should not count), else `url`. It checks the names and addresses as strictly. On any failure
it links `https://github.com/getdroppen/droppen/releases/latest` instead.
Errors: `503 release_unavailable` (with `Cache-Control: no-store` and
`Retry-After`) until the hub has read a release that checks out. A hub
before this route answers `404`.

### `GET /dl/{platform}`
`platform`: `macos`, `linux-x86_64` or `linux-aarch64`. `302 Found` to that
platform's file of the latest release (its `url` above), with
`Cache-Control: no-store` and no body; before the hub has read a release, or
when the release lacks the file, `302` to
`https://github.com/getdroppen/droppen/releases/latest`. The hub counts one
download of the platform for the day: a number per platform and day, and
nothing about who asked — no address, cookie or user agent is kept. The
count is capped per address and per minute (past it, the redirect is the
same and nothing is counted as a download); `HEAD` and a browser's prefetch
are not counted. `GET /install.sh` is counted the same way. Errors: `404
not_found` for any other platform. Limited like `/healthz` (the static
routes' share per address); served on every host of the hub but the
Turnstile host, never redirected. The web app's `/robots.txt` disallows
`/dl/`. A hub before this route serves its web app
there (or `404`): a client links it only when `GET /v1/releases/latest`
names it as `download_path`.

### `POST /v1/signup`
`{email, password, invite?, turnstile?, lang?}` → a login session
`{account, token, expires_ms, email_verified, verification, known_client}`
(`known_client`: see login).
`invite` is required when sign-up is by invite (an invite may also give the
account a plan); `turnstile` is required when the hub uses Turnstile; `lang`
picks the verification mail's language. With verification `required` a
verification mail is queued and the account has 14 days before
`email_unverified` blocks registering devices, creating its chain and
enrolling machines.
With open sign-up and verification `required`, an account that holds the
e-mail but nothing else — never verified; no device, machine or chain entry;
no plan of any source, no override; created without an invite — and that
has been quiet for an hour (no sign-up, no verification mail) is replaced
by the new sign-up in the same transaction: it and its sessions are gone,
and its verification link verifies nothing. Any other account keeps the
e-mail (`409 email_taken`); sign-up by invite never replaces one. A new
account's verification mail always goes out.
An address whose account was deleted within the hub's hold
(`/v1/signup/config` `deletion.email_hold_days`; the address compared in its
canonical form: lower-cased, its `+tag` dropped and, at Gmail, its dots) is
refused with `409 recently_deleted`, whose `until_ms` says when it may sign
up again. `400 disposable_email` covers the hub's built-in list of throwaway
mail domains and any domain on its operator's deny list alike.
Errors: `403 signup_closed | bad_invite | turnstile_required | turnstile_retry | turnstile_failed` (`turnstile_*` only with Turnstile),
`400 bad_request | disposable_email`, `409 email_taken | recently_deleted`, `503 turnstile_unavailable`.

```json
{"error": "recently_deleted", "message": "an account with this address was deleted recently", "until_ms": 1793750400000}
```
Limits: every attempt counts (per address, per network, across the hub),
and accounts made 5/hour and 20/day per address; without Turnstile also
per network and across the hub (`429 rate`, no account made); fields as in
the README.

### `POST /v1/login`
`{email, password, turnstile?, known_client?}` → a login session (as sign-up,
`known_client` included).
Errors: `401 bad_login` (wrong e-mail or password, an unknown account, or a
locked (e-mail, address) pair — indistinguishable on purpose; an IPv4
address as a whole, an IPv6 one by its /64),
`403 turnstile_required | turnstile_retry | turnstile_failed` (only with
Turnstile), `403 account_suspended` (the hub's operator suspended the
account: said only once the password checked out, README "Suspended
accounts"), `429 rate` (also the backoff without Turnstile), `400 bad_request`.
The failures, the lockout and the Turnstile threshold count the e-mail asked
for, whether or not an account has it: an unknown e-mail gets the same
answers in the same order as a wrong password for an existing account.
Without Turnstile on the hub nothing asks for it; instead an e-mail that
failed often, and a network that failed often at it, back off: `429 rate`
with `retry_after_ms` (a few seconds, doubling, at most 5 or 15 minutes),
the same for an unknown e-mail (README).
**Known client.** Sign-up and every successful login answer `known_client`:
the hub's word that this client signed in to the account (`kc1.…`, opaque,
good until the end of the UTC day 30 days out, bound to the account — worth
nothing once the account is replaced; every one minted for an account on one
day is the same token). Keep it per e-mail (across logout too) and send it with the next
login: a valid one for the e-mail's account skips both backoffs — until it
has failed 3 times in the last hour, when it counts as no token for the rest
of the hour (a stolen one gives no more guesses than none). It never skips
the per-address lock or the per-address rate.
Limits: 30 per 10 minutes per address; the lockout, the backoffs and the
Turnstile thresholds in the README.

### Passkeys (`docs/passkeys.md` §3, K1)

Signing up and in with a passkey. WebAuthn's binary values here are
**base64url without padding** (its own JSON encoding: the browser's
`parseCreationOptionsFromJSON`, `parseRequestOptionsFromJSON` and
`toJSON()`), not hex. A **challenge** is 42 bytes, stateless (asking for
one stores nothing), good for 2 minutes and answered once; send it back
exactly as given, with the credential that answers it.

The relying party id is the hub's (`droppen.app`; `/v1/signup/config`
names it) and the only origins taken are the app's own, compared exactly
(`https://my.droppen.app`, and `https://droppen.app` while browsers move off
it: README, "Hosts"): an answer made on any other origin — another subdomain
included — or in a cross-origin frame is refused. Passkeys are discoverable (`residentKey:
required`), need user verification (`userVerification: required`), and
take Ed25519 (-8), ES256 (-7) or RS256 (-257, 2048–4096 bits);
`attestation: none` (a statement is not read).

**A passkey gives a login session and nothing more**: the one a password
gives (`known_client` included), with no device and for 15 minutes. The
device session is still the device key's signature (README). **A passkey
login is never held by the password login's lockout, backoffs or
known-client budget**, never asks for Turnstile, and its failures count
towards none of them: it has per-address and per-network rates of its own.

Every refused credential is `401 bad_passkey` (a type, origin, RP id,
flag, challenge, user handle, signature or counter that does not check
out, a replayed challenge, a credential that is not WebAuthn's shape), or
`401 unknown_passkey` for one this hub does not know: hide it from the
person's passkey provider (`PublicKeyCredential.signalUnknownCredential`,
where the browser has it). A hub without passkeys answers every route
here `403 passkeys_off`.

#### `POST /v1/passkeys/options`
`{purpose: "signup", email, invite?, turnstile?, lang?}` →
`{challenge, options}`, `options` a `PublicKeyCredentialCreationOptionsJSON`
for `navigator.credentials.create()`: `rp {id, name}`, `user {id, name,
displayName}` (a random 32-byte handle; the e-mail as the name the
provider shows), the three algorithms, `timeout: 120000`, `residentKey`
and `userVerification` required, `attestation: "none"`,
`excludeCredentials: []`, and `extensions {credProps: true, prf: {eval:
{first}}}` (a one-off PRF input: Windows Hello makes a PRF-capable
credential only when asked at creation; the result is never sent back).
**Every sign-up check runs here, before the browser makes a passkey**,
exactly as `POST /v1/signup` makes them: the sign-up attempts (per
address at once; per network and across the hub just before the e-mail is
looked up, once the cheap checks passed: each answer says whether an
e-mail is taken), whether the address has accounts left to make (only an account
made spends one), `DROPPEN_HUB_SIGNUP` (closed, invite), Turnstile when the hub
uses it (action `signup`), the e-mail's syntax and throwaway domains, the
invite, without Turnstile the per-network and hub-wide caps — and whether
the e-mail is taken (`409 email_taken`, unless open sign-up would replace
the account that holds it, as `POST /v1/signup` does), or its account was
deleted within the hold (`409 recently_deleted` with `until_ms`). A refusal
here means: do not call `create()`.
Errors: `400 bad_request | disposable_email`, `403 signup_closed |
bad_invite | turnstile_* | passkeys_off`, `409 email_taken |
recently_deleted`, `429 rate`, `503 turnstile_unavailable`. (Adding a passkey to an account takes a
device session: `POST /v1/passkeys/add/options`, below; any other `purpose`
is `400`.)

#### `POST /v1/signup/passkey`
`{challenge, email, invite?, lang?, credential}` → a login session, as
`POST /v1/signup` answers. `credential` is `create()`'s
`RegistrationResponseJSON`; send only `credProps.rk` and `prf.enabled` of
its client extension results. The challenge must be the options' for this
very e-mail. The sign-up's rules are checked again (the invite may be
spent meanwhile, the e-mail taken, the caps full) without charging the
address's limit or Turnstile again; then the account is made with **no
password**, this passkey and its user handle, and the verification mail is
queued, as with `POST /v1/signup`. A password login for such an account is
answered exactly like a wrong password (`401 bad_login`).
Errors: `401 bad_passkey`, `400 bad_request | disposable_email`, `403
signup_closed | bad_invite | passkeys_off`, `409 email_taken` (lost a
race) `| recently_deleted`, `429 rate`. After any refusal the passkey
belongs to no account: signal it unknown.

#### `POST /v1/passkeys/login/options`
`{}` → `{challenge, options}`, `options` a
`PublicKeyCredentialRequestOptionsJSON`: `{challenge, rpId, timeout:
120000, userVerification: "required", allowCredentials: []}` — any of the
account's passkeys; nothing names an account. For the e-mail field's
autofill (`mediation: "conditional"`) ask again before 2 minutes pass.
Limits: 60 a minute per address.

#### `POST /v1/login/passkey`
`{challenge, credential}` → a login session plus `passkey: {id, label,
unlock}` (`id` the hub's `pk-…`; `label` the provider's name by AAGUID, or
`Passkey`; `unlock` `"on"` when a current passkey slot on the account's
chain names this passkey — a new device can join with it,
`docs/passkeys.md` §4.6 — else `"off"`: a hint for the next screen, which
goes by the chain the device verifies itself; a hub from before slots said
`"none"`). `credential` is
`get()`'s `AuthenticationResponseJSON`, `userHandle` included. Checked: the
challenge, type, origin, cross-origin, RP id hash, user presence and
verification, the credential (registered → its account), the user handle
(the account's), the signature over `authenticatorData ‖
SHA-256(clientDataJSON)` under the stored key, the counter (on a
device-bound passkey a counter that does not go up, unless both are 0, is
refused as a possible clone; on a synced one — backup eligible — it is let
in and logged, and the stored counter never goes down) and the
credential's backup eligibility (which may not change). The client data
and the credential must be JSON objects with no key given twice (taken as
sent, never the last of two). An unverified account signs in like with a
password: `email_verified: false`, the same limits.
Errors: `401 bad_passkey | unknown_passkey`, `403 passkeys_off`,
`403 account_suspended` (only once the assertion checked out),
`429 rate`. Limits: 30 per 10 minutes per address, a bucket of its own; a
network's (IPv4 /24, IPv6 /48) failed assertions 120 per 10 minutes, past
which its failures are answered `429` — a valid assertion never is; and 30
logins per 10 minutes per passkey, spent only by its own valid assertions
(past them `429 rate`, which counts as no failure).

### `POST /v1/email/verify`
`{token}` (from the link's fragment) → `{account, verified: true}`. No
session: the token is the proof, and it verifies only the account it was
made for, at that address (`400 bad_token` once that account is gone or
its e-mail changed).
Errors: `400 bad_token`, `410 expired`. Limits: 30/minute per address.

### `POST /v1/waitlist`
`{email, plan: "free" | "pro", locale?}` → `{ok: true}` — the very same
answer, after the same one write, whether the address is new or already on
the list: it never says which. No confirmation mail is sent; submitting is
joining. One entry per inbox (the address with its `+tag` folded off, and at
Gmail its dots too, `googlemail.com` counting as `gmail.com`): asking again
keeps the address first given and takes the latest `plan` and `locale`.
`email` is trimmed and lower-cased, at most 254 bytes (64 before the `@`),
one `@`, a domain of two or more host labels; `plan` is a public plan;
`locale` (≤ 16 bytes) is kept as one of `en`, `zh-CN`, `zh-TW`, `ja`. No IP
address or other request detail is stored; an entry not given again for 365
days is deleted; deleting an account (`DELETE /v1/account`) deletes its
address's entry too. Errors: `400 bad_request` (`message` names the field),
`400 disposable_email` (a throwaway mail domain, or one on the operator's
deny list). Limits: README.md, "Rate limits and caps".

### `POST /v1/session/challenge`
`{account, device}` → `{nonce}` (32-byte hex). Stateless; any well-formed ids
get one. Errors: `400 bad_request` (ids not 1–64 printable characters).

### `POST /v1/session`
`{account, device, time_ms, nonce, signature}` → a device session
`{account, device, token, expires_ms}`. `signature` (64-byte hex) is the
device key's signature of `HubLogin {role: DeviceSession, account, id:
device, hub: <origin dialled>, nonce, time_ms}`; `time_ms` within 5 minutes.
A host (chain v3: a device that runs AI) signs in here with the identity it
enrolled with — `device` is its machine id — once the chain lists it as a
current device; it then controls other hosts as a device. With
`DROPPEN_HUB_CHAIN_V3=off` no host acts as a device at all — a kill switch: this
sign-in (`403 host_devices_off`, even for a host that signed in as a
device before), a host's device session from before on every route (`403
host_devices_off`), its device WebSocket (`4403 host_devices_off`) and its
relay tickets are all refused.
A host whose identity key is being renewed (`pending`, `attempt` or
`staged`, [below](#identity-key-renewals)) — or that is isolated, whatever
its renewal row says — acts as no device meanwhile: this sign-in
(`403 rotation_pending`), its device sessions (revoked when the renewal was
asked for; `403 rotation_pending` on every route for one that slipped
through), its device WebSocket (`4403 rotation_pending`, checked again as it
is registered), its relay tickets and any channel it would open as a device
are refused until the renewal is committed or withdrawn (or, isolated, until
a renewal confirmed on the device). A signature by one of the host's
**retired** keys (one a renewal replaced) is `401 rotated`, its challenge
fresh: the signed exchange is kept as evidence (`GET /v1/rotations`
`disputes`) and may isolate the host. A session is made only if the key it
was signed with is still the device's when it is made: one checked with a
key a renewal retires meanwhile is `401 rotated` too, no session made.
A device (or a host) of a suspended account gets `403 account_suspended`,
once its signature checked out (README "Suspended accounts"). A device of an
account being deleted, or deleted within the hub's hold
(`DELETE /v1/account`; `deletion.email_hold_days`), gets `410
account_deleted` — only once its signature checked out against its own key
(for a deleted account, the public key the hub kept with the tombstone): a
device that kept its keys for a deleted account hears it on its next renewal
(the web app then wipes them, as for a removed device). An account or device
the hub merely does not know — no such evidence — is `401 denied`, as ever:
a hub restored from a backup or pointed at the wrong database never makes a
browser wipe its keys.
Errors: `401 denied` (anything wrong: unknown device, bad signature, stale
or reused nonce, clock — never a check the hub could not make, which is
`503 unavailable`), `401 rotated`, `403 device_removed`,
`403 account_suspended`, `410 account_deleted`,
`403 rotation_pending`, `400 bad_request` (`nonce`
not 32 bytes of hex), `429 rate` (with `retry_after_ms`). Limits: 30 a
minute and 300 an hour per device, spent only by the device's own valid
signature — the hub's table of answered nonces is shared, and no one device
may fill it; a made-up signature naming the device, or another device of the
account, spends none of it. A refused one leaves its nonce unanswered.

### `GET /v1/client`, `GET /v1/agent` — WebSocket
The device and agent sockets ([websocket.md](websocket.md)). 60 upgrades a
minute per address. A `login` whose `account` or `id` is not 1–64
printable characters, or whose `session` is longer than 128, is refused
(`4401 denied`) before anything is looked up.

### `GET /v1/relay` — WebSocket, relay
A relay's control link ([websocket.md](websocket.md#v1relay-relay--hub)).

### `GET /v1/probe/rooms/{room}?role=probe|page` — WebSocket, tooling
Only when the hub runs the direct-path measurement probe; otherwise 404.
`room` is 32 lower-case hex digits (else `400 bad_request`). No sign-in;
60 upgrades a minute per address, like the other sockets. Text
messages ≤ 8 KiB from a page go to the room's probe, the probe's to every
page; on each join or leave every member gets `{"hub": "peers", "probes": n,
"pages": m}`. The messages themselves are the probe's own (`rh-p2p-probe`),
relayed unread; they are not part of this API.

## Signed in: `session` (a login or a device session)

### `POST /v1/devices`
`{sign_key, dh_key, kind?, host?, replaces?}` (32-byte hex each: Ed25519, X25519;
small-order keys are refused) → `{device}`. Registers this device's keys; it
is outside the chain until a full device adds it (or it recovers with the
recovery words). Every connected device of the account gets
`device_registered`.

`kind` (optional) says what the device is, as 1–32 lower-case letters,
digits and dashes; `host` (optional) names the machine it runs on, as 1–64
letters, digits, `.`, `_` and `-`. Both are **metadata only**: kept and shown
in `GET /v1/me`, never checked against the chain and never a reason to allow
anything (what a device may do is the chain's: its `host` and `admin` flags,
chain v3). The open clients send neither since chain v3 (a computer controls
other devices with its one identity; the earlier `"aggregator"` second device
is gone); the hub itself writes `kind: "host"` for a host that signed in as a
device — such a row is no registration: it counts neither towards the 10 a
day nor as waiting.

`replaces` (optional) names the device of this account the new one takes
the place of: the browser lost that one's keys and registers as a new
device. Metadata only, like `kind`: kept and shown in `GET /v1/me`
(`devices[].replaces`) so the device that adds the new one can offer to
remove the old one — the web app asks first and writes the ordinary
removal only if the person confirms; the hub never removes anything for it.
An id of no device of this account is kept as `null`.
Errors: `400 bad_request` (also a malformed `kind`, `host` or `replaces`), `403 email_unverified`, `429 device_rate | too_many_pending_devices`.
Limits: 10/hour per address, 10/day per account, 3 waiting at once (both
counted with the registration itself, so requests at once cannot each slip
under them). A device
outside the chain counts as waiting only within its 30-minute pairing window
(the time it may open a pairing at the hub); it is kept, sessions and all,
for a day after it registered — a new browser showing its QR code may wait
that long to be added, or join with the recovery words — and then deleted
with its row, shown in `GET /v1/me` until then.

### `GET /v1/me`
`?touch=1` (optional): this page's `User-Agent` says "Macintosh" yet it has a
touch screen (`navigator.maxTouchPoints > 1`), as iPadOS Safari asking for
desktop sites — its default — does; no Mac has one. With a device session and
a Mac `User-Agent`, the device is then named an iPad (`devices[].ua_label`,
below); it changes nothing else, nor for any other `User-Agent`, nor with a
login session. Any other value is no hint.

→ `{account, email, plan, chain_seq, chain_v3, relay_prefs, rotations, devices, machines, credentials, sign_in_changes, known_client?}`:
- `known_client` (with a device session only): a fresh known-client token for the account, as a sign-in gives one (`POST /v1/login`). A device in use keeps getting one, so it never ages out and needs no password login to get its first;
- `email`: `{address, verified, verification, due_ms, delivery: "pending"|"sent"|"failed"|null, delivery_error}`
  (`due_ms`: when an unverified account starts being refused; `delivery_error`, for a failed mail only, is generic — `the mail service refused to deliver to this address` or `the mail could not be sent in time` — the mail service's own words stay in the hub's log);
- `plan`: the plan view (`{id, source, status, period_start_ms, period_end_ms, grace_plan, grace_until_ms, entitlements: {<key>: n|null}, usage: {storage_bytes, devices, hosts, machines, relays, transfer: {period_start_ms, period_end_ms, bytes, included, overage_bytes, throttled}}}`), or `null` if it cannot be read. `usage.devices` counts every current device of the chain, hosts included (`devices.max`); `usage.hosts` the ones that run AI (`hosts.max`); `usage.machines` and `entitlements["machines.max"]` are the old names, kept one version;
- `plan.entitlements` keys (`null`: no limit; the values are the plan's, not documented here): `storage.retention_days` (days the cloud's encrypted copy is kept), `storage.bytes` (bytes of it an account may hold), `relay.max` (relays in use at once, the `limit` of `relay_set`), `devices.max` (every current device of the chain, hosts included), `hosts.max` (the devices that run AI; `machines.max` is its old name), `transfer.included_bytes` (relayed transfer a period includes), `transfer.overage` (what happens past it), `transfer.hard_cap_bytes`, `conn.bytes_per_sec` (a connection's inbound byte rate, websocket.md), `streams.records_per_sec`, `relay.beta` (sees hidden relays). A `409 limit` names the key it hit.
- `chain_seq`: the newest chain entry the hub holds, `null` before genesis;
- `relay_prefs`: `{standby}`, the account's relay preference (`PATCH /v1/relays/prefs`); a hub from before it leaves it out;
- `chain_v3`: `{mode: "off"|"read"|"write", writable, waiting: [machine], in_use}` — whether the account may take its first v3 entries, or (once `in_use`) add a host, now (`docs/local-aggregator.md` §3.5): the hub must write them (`DROPPEN_HUB_CHAIN_V3=write`) and, before the first, every current host's agent must have declared `chain/3` and `policy/1` (`waiting` lists those that have not; once `in_use` they no longer hold the account back and are listed so the owner can update them); `in_use`: the chain already holds a v3 entry, so nothing that reads only v2 can follow it, and everything but a new host is taken in every mode. Before genesis (`chain_seq: null`) there is no host to wait for, and `writable` says whether the account can be created at all: the web app makes every new account on v3 (`Genesis3` and user key epoch 0 as v3, one batch) and makes none while it is `false`;
- `rotations`: `{enabled, window_ms, active: [{host, state}]}` — identity key renewals ([below](#identity-key-renewals)): whether this hub takes them (`DROPPEN_HUB_HOST_ROTATION=on`), the window T, and each host with a renewal under way or committed in the last day (`GET /v1/rotations` has the rest); a hub from before renewals leaves it out;
- `devices[]`: `{id, sign_key, dh_key, created_ms, last_seen_ms, in_chain, kind, host, ua_label, replaces, online}` (`kind`, `host`, `replaces`: as registered, `null` when not given; a hub from before local sign-in leaves them out, and one from before `replaces` and `online` leaves those out; `ua_label`: below; `online`: whether the device is signed in on `/v1/client` now — `last_seen_ms` is when it last signed in, which a device connected for hours does not move);
- `devices[].ua_label`: the device's browser by a short name, for a page to tell browsers and phones apart: what the device is and which browser, `"<device> · <browser>"` (a space, `·` U+00B7, a space), or either alone. `<device>` is one of `iPhone`, `iPad`, `Mac`, `Windows`, `Android`, `Linux`, `ChromeOS`, `<browser>` one of `Safari`, `Chrome`, `Firefox`, `Edge`, `Opera`, `Samsung Internet` — nothing else, ever: no version, no model, never the `User-Agent` itself, which the hub does not keep. A webview that names no browser (the desktop app's, a home-screen web app) is the device alone (`"Mac"`, `"iPhone"`); an iPad asking for desktop sites reads as `"Mac"` unless its page says `?touch=1` (above), then `"iPad"`. It comes from the `User-Agent` of the device's own requests only: its sign-in on `/v1/client` and its `GET /v1/me` with its device session — never a request body, a login session or another device's request. Replaced when it changes; a request whose `User-Agent` names nothing known (curl, an HTTP library, the agent) leaves it as it is, and so does one that says `Mac` where the device is known as an `iPad` with the same browser (a request without the hint cannot tell the two apart, and a device never turns from one into the other). `null` until one named something; metadata only, like `kind`; a hub from before it leaves it out;
- `machines[]`: `{id, sign_key, dh_key, created_ms, last_seen_ms, in_chain, presence, copy: {streams, bytes, last_write_ms}, reads_v3, rotates}` (`presence` as in websocket.md; `copy`: the machine's encrypted offline copy on the hub; `in_chain`: a current host; `reads_v3`: its agent declared `chain/3` and `policy/1`; `rotates`: it declared `rotate/1`, it can renew its identity key).
- A host that signed in as a device (`POST /v1/session`) is listed in `devices[]` too, with `kind: "host"`.
- `credentials`: `{password, passkeys}` — whether the account has a password (an account made with a passkey, or one whose password was removed, has none) and how many passkeys; a hub from before passkeys leaves it out.
- `sign_in_changes`: `[{id, kind, device, label, at_ms}]` — how the account's sign-in changed in the last 30 days (below, "The account's ways to sign in"), oldest first, at most the newest 50, each exactly as `sign_in_changed` told the account's devices at the time (websocket.md): `kind` `passkey_added | passkey_renamed | passkey_removed | password_set | password_changed | password_removed`; `device` the device session that made it; `label` the passkey's (the new one, for a rename), `null` for the password. A hub from before it leaves it out.

### `GET /v1/chain?from=N`
→ `{entries: [hex], seq}`: the account's chain entries from index `N`
(default 0), each an encoded `rh_proto::chain::Entry`; `seq` is the head,
`null` before genesis. Verify them yourself.

### `POST /v1/email/resend`
`{turnstile?}` → `{queued}`. Errors: `409 verification_off | already_verified`,
`403 turnstile_*` (only with Turnstile). Limits: 10/hour per address, 1/minute and 5/day per account,
5/hour and 10/day per e-mail address across the accounts that held it
(`429 rate`; a refusal by one of these spends nothing of the others).

### `POST /v1/logout`
Ends this session. `204`.

## Signed in: `device` (a device session)

### `POST /v1/chain`
`{entries: [hex]}` (1–64 encoded entries) → `{seq}`. The hub verifies each
against the chain it holds (the same rules every client applies) and appends
atomically. A device may also append over `/v1/client` (`CHAIN` frame).
Errors: `400 bad_request`, `409 chain_conflict` (the chain moved: fetch and
retry), `422 invalid_entry`, `409 limit` (`devices.max` — every current
device, hosts included, on any added device; `hosts.max` — on an added host
or `SetHost true`; an approval may go one past; recovery joins and
self-activations are never limited), `409 chain_v3_not_ready`
(`{error, mode, hosts, message}`): the
account's **first** v3 entries need the hub's v3 mode to be `write` (`/v1/me`
`chain_v3.mode`) and every host
current *after* the batch to have declared `chain/3` and `policy/1` (`hosts`
names those that have not; a host the batch removes or stops no longer
counts). On a chain **already v3**, only a batch that adds a host or runs AI
again on one is gated: it needs `write`, and that host must have declared
them itself. Everything else — removal, demotion, key rotation, recovery,
pairing a device, a policy — is taken in every mode, whatever other hosts
lag: a rollback to `read` or `off` stops new v3 accounts and new hosts, and
never freezes an account that is already v3. `422 chain_v2_genesis`: a
v2 genesis (`Genesis`). New accounts start on chain v3 (`Genesis3`) in
every mode, so in `read` and `off` no new account is founded at all. The
check comes before the e-mail check, so an app from before chain v3 is
told why. It applies here and to a `CHAIN` frame over `/v1/client`, whose
`chain_result` error starts with the code. An account founded on v2
before is unaffected: it takes v2 entries and upgrades with its first v3
entry. `403 email_unverified` (a v3 genesis, here and as a `CHAIN` frame
over `/v1/client`).
`409 rotation_not_ready` (`{error, reason, message, waiting?, ready_ms?}`)
for a batch that renews a host's identity key (`RotateHostKey`, kind 20),
sets the account's renewal policy (`SetRenewalPolicy`, kind 21) or pauses
control over hosts while they are renewed (`RenewalPause`, kind 22, its
start; [below](#identity-key-renewals)) the hub does not take now — the end
of a pause the chain holds open (`completed`, `cancelled`) is taken whatever
the switch and the gate say — `reason`:
`disabled` (`DROPPEN_HUB_HOST_ROTATION` is off: refused before anything else
is looked at), `v2` (the account's chain is v2), `waiting` (a host current
*after* the batch has not declared `rotate/1`; `waiting` names them — a host
the batch removes or stops does not count), `not_staged` (the host's agent
staged no such renewal, or its staging was voided or ran out), `mismatch`
(the entry is not the staging byte for byte), `window` (T has not passed
since the staging, or a sign-in still owes its confirmation; `ready_ms`
says when), `needs_assurance` (the host takes only a renewal confirmed on
the device: clone evidence within 30 days, or isolated), `conflict` (the
host's keys at the hub are not the ones the entry replaces). Body ≤ 256 KiB.

Besides the chain's own rules the hub checks: a device the batch adds (or
that joins with the recovery words, or founds the chain) is registered to the
account with the same keys and was not revoked (its pairing closed by an
admin device); a host it adds (or makes a
host again) is an *enrolled machine* with the same keys; a batch never ends
with a new or restarted host still owed its machine key epoch (`422`). An
agent that has not declared `chain/3` is never sent a v3 entry (it gets the
v2 prefix only).

A renewal the hub takes is applied in the transaction that stores the
entries: the host's keys are swapped in `machines` and (if it has its row)
`devices` only where they are still the old ones — otherwise the whole
batch is `409 rotation_not_ready` (`conflict`) and nothing is kept — the
old keys are kept as retired, every mail to the host is deleted (it was
sealed to the old key), its device sessions are revoked and the renewal is
`committed`. After it, the entries go out to the chain's subscribers first;
then the host's agent and device connections hear `error {code: "rotated",
fp}` and close with 4005, its relay tickets so far are revoked, and the
account's devices hear `rotation {state: "committed"}`.

### `POST /v1/enrollments` — requires a current full device
`{device, token}` (`device`: the session's own; `token`: the enrollment
code's 16-byte token, hex) → `{expires_ms}` (10 minutes). Announces a code
an agent will claim on `/v1/agent` (`enroll`).
Errors: `400 bad_request`, `403 not_a_full_device | email_unverified`,
`409 limit` (`hosts.max`, waiting enrollments included), `409 exists`,
`429 too_many_enrollments`. Limits: 10/hour per device, 5 waiting per account.

### `GET /v1/enrollments/{token}`
→ `{state: "waiting"|"expired", expires_ms}`, or once the agent proved
`{state: "proved", machine, device, sign_key, dh_key, proof}`. The device
checks `proof` against the code before it adds the host (`[AddDevice3{host},
KeyEpochs]`; v2: `AddMachine`).
Errors: `400 bad_request`, `404 not_found` (another account's, or none).

### `POST /v1/mailbox` — requires a current admin device (but see purpose 4)
`{recipient, sender, purpose, epoch, sealed}` → `{id}`. A sealed key for a
machine or a device of the account; `sender` is the session's device;
`sealed` ≤ 8 KiB. Purposes: `1` a user key epoch — **only to an active
admin device** (not a pending one, not one demoted); `2` a host's own
machine key, for its agent; `3` a pairing's delivery; `4` a request for keys
(no secret: from an admin, for user key epochs; from **any other current
device, to an active admin device only**, for machine keys — answered with
purpose 5, never with a user key); `5` another host's machine key epoch
(`str16 host | sealed`, `rh_proto::wrap::encode_host_key_mail`) for a
current device that is **not an admin and that the named host's accept
policy admits** (machine keys follow the accept policy); the hub stores any
purpose 0–255. Who may send what to whom is judged under the account's
chain lock, against the chain as it stands. Key deliveries (1–3 and 5) never count against
`storage.bytes`; anything else does. The recipient hears it at once
(`mail`) if connected; a host's agent hears only purpose 2.
A host's mailbox holds mail for two roles: its agent reads and acknowledges
only purpose 2 (over `/v1/agent`), its device role everything else
(`GET`/`DELETE /v1/mailbox`) — neither sees nor deletes the other's, and
the caps (README, "Rate limits and caps") count each role on its own, so mail
piling up for a device role that stopped reading never blocks the host's own
machine key.
When the chain demotes an admin (`SetAdmin false`), any user key (purpose 1)
still waiting for it is deleted with the entry; removing a device or a host
deletes the mail from and to it, and nothing else.
Errors: `400 bad_request` (also: no such recipient, or one no role of it
would read: purpose 2 to anything but a machine of the account, any other
purpose to anything but a registered device — a host only once it signed in
as a device; that refusal's message is exactly "this mail goes only to a
device of this account (a computer gets it once it signs in as a device)",
which clients tell apart by "gets it once it
signs in as a device"), `403 not_a_full_device
| not_an_admin | not_in_chain` (purpose 5 to an admin, a pending device or
no device of the account) `| not_accepted` (purpose 5 to a device the named
host does not admit) `| bad_host_key` (purpose 5 that does not name its
host), `409 limit` (`storage.bytes`), `429 mail_rate`, `507 mailbox_full`.
Body ≤ 20 KiB.

### `GET /v1/mailbox?after=N&limit=M`
→ `{mail: [{id, sender, purpose, epoch, sealed}]}`: the session device's own
mail with `id > after`, oldest first, `limit` 1–64 (default 64), never a
host's own machine key (purpose 2: its agent's). Page with the last `id`.

### `DELETE /v1/mailbox/{id}`
Acknowledges (deletes) one of the session device's own mails. `204`, also
for an id that is not its own or is purpose 2 (nothing happens).

### `GET /v1/keys`
→ `{keys: [{name, blob, updated_ms, writer}]}`: the account's keyring,
opaque wrapped-key blobs (AEAD-bound to their account, machine and epochs).
Every copy is listed — current admins' first, then those of a writer no
longer current (removed, or demoted) and names from before writers. The hub
cannot tell a good copy of a wrap from a bad one, so it never hides one
because another writer holds the same wrap: readers try each against the
chain's commitment and keep the one that verifies (a recovery package
likewise, whoever sealed it). Of a writer no longer current the hub keeps one
copy per machine-key epoch (under the newest user key) and one recovery
package per user-key epoch for the current recovery key; packages for any
other recovery key, and what names epochs or machines the chain never
committed, are deleted.

### `PUT /v1/keys/{name}` — requires a current full device
`{blob}` (≤ 4 KiB, hex) → `{name, stored}` (`stored: false`: the same bytes
were already there). `name` (`[a-z0-9._/-]{1,128}`, no `..`; it contains `/`)
is written once. It must end with the writer — `mk/<machine>/<mk epoch>/<uk epoch>/<writer>[.<suffix>]`
(a machine key under a user key) or `rk/<uk epoch>/<writer>/<tag>…` (a user
key for the recovery key whose `RSK` starts with `tag`) — and name epochs the
chain has committed.
Errors: `400 bad_request`, `403 not_a_full_device | bad_key_name`,
`409 exists` (another blob holds the name), `409 no_chain`, `507 keyring_full`.
Body ≤ 16 KiB.

### `POST /v1/pairings`
`{device?}` (if given, the session's own) → `{id}`. Opens a numeric-code
pairing for this device (`docs/cross-machine-architecture.md` §1); a device
outside the chain holds one at a time, within 30 minutes of registering.
Errors: `400 bad_request`, `403 pairing_window_closed`, `429 too_many_pairings`.

### `GET /v1/pairings` — requires a current full device
→ `{pairings: [{id, device, created_ms}]}`: the account's open pairings.

### `POST /v1/pairings/{id}/messages`
`{from: "initiator"|"responder", body}` (hex, ≤ 4 KiB) → `{n}`. Relays one
CPace message. The initiator is a current full device; the responder is the
device that opened the pairing.
Errors: `400 bad_request`, `403 not_a_full_device | not_the_creator`,
`404 not_found`, `429 pairing_full`.

### `GET /v1/pairings/{id}/messages?after=N`
→ `{messages: [{n, from, body}]}` after `n = N`. The opener, or a current full device.
Errors: `404 not_found`, `403 not_a_full_device`.

### `DELETE /v1/pairings/{id}`
Closes (burns) it: its opener, or any current full device — which also
revokes the opener if it is still outside the chain. `204`, also when there
is nothing to close.

### `GET /v1/streams/{stream}/pages/{hash}` — requires a current device
A sealed snapshot page of one of the account's streams
(`application/octet-stream`, `Cache-Control: private, max-age=31536000,
immutable`), streamed from the page store; one `Range: bytes=…` is honoured
(`206`, or `416` with `Content-Range: bytes */<size>`). `stream` and `hash`
are 32-byte hex; verify the page against `hash` yourself.
Errors: `400 bad_request`, `403 not_in_chain`, `404 not_found`. Not counted
against the per-device session limit.

### `GET /v1/relays`
→ `{relays, limit, account_set, emergency, agents, throttled}`:
- `relays[]`: `{id, city, region, origin, state: "up"|"draining", load}`, the relays this account may use now;
- `limit`: `relay.max` (i64::MAX: no limit);
- `account_set[]`: `{id, last_used_ms}`, the relays the account uses at once;
  `last_used_ms` is when a device last got a ticket for it — `0` for one the
  hub put there itself for the account's agents (it keeps one working public
  relay in the set, two with `relay_prefs.standby`, `relay.max` allowing, and
  replaces one that fails or drains at once), which no device has used: like
  a member idle for a week, it may be replaced;
- `emergency[]`: relay ids granted outside the set while none of the set works;
- `agents`: `{<machine>: {<relay>: rtt_ms}}`, each machine's last reported round trips;
- `throttled`: the account's relayed transfer is throttled.

### `PATCH /v1/relays/prefs` — requires a current device
`{standby?: bool}` → `{standby}`: the account's relay preference, for all its
devices. With `standby` the hub keeps two working public relays in the
account's set (`relay.max` allowing) instead of one: each device that runs AI
then holds a second, idle relay connection (not counted as transfer), so a
relay that fails or drains is replaced without its agent having to sign in
anywhere first. Off by default. A change fills or trims the set at once and
the account's agents get `relay_set`. Errors: `403 not_in_chain`.

### `POST /v1/relays/tickets` — requires a current device
`{relays: [id]}` (1–8) → `{tickets: [{relay, origin, ticket, expires_ms, emergency?, throttled?}], errors: [{relay, error, key?, limit?, account_set?}]}`.
A ticket (`rh_proto::ticket::RelayTicket`, signed by the hub, 15 minutes)
signs this device in on that relay (websocket.md `/v1/connect`). Asking for a
relay outside the account's set adds it, replaces a member idle for a week
(or one the hub added that no device used, `last_used_ms` 0), or is refused
(`error: "limit"`, with the set); when every member is
unusable, up to 2 emergency tickets. `error` is `unknown` (not listed for
this account), `unavailable` (not `up`; for an agent's `relay_tickets`,
also when the hub cannot read now whether the host is isolated: try again),
`limit` or `forbidden` (and, for an agent's `relay_tickets`, `isolated`). An
isolated host
([below](#identity-key-renewals)) gets no ticket and is named in none — but
while a renewal confirmed on the device is under way for it, an admin
device's ticket names it and its own agent gets its tickets. The
ticket's `peers` are the hosts that accept control from this device (chain
v3 "who may control me": an admin device reaches every current host; any
other device — a host included — only the hosts an admin's `SetAccept`
lets it reach; never itself) — and, for a device that is not an admin, only
the hosts whose agent last declared `policy/1` (the `policy/1` gate, S1-4:
an agent from before would let such a device reach every route,
self-upgrade included). A host asks here as a
device, with its own device session. The hub routes a device's `OPEN` by the
same rules (`4403` otherwise).
Errors: `400 bad_request`, `403 forbidden` (no chain, or not current),
`403 isolated` (this device is an isolated host), `403 rotation_pending`.
Limits: 120 an hour per device (`429 rate`; tickets last 15 minutes, so a
device needs about one per relay every ten).

### `GET /v1/main-device`
→ `{machine, set_ms}`: the account's **main device**
(`docs/local-aggregator.md` §6.5 「主设备」) — the host whose agent keeps the
account's screens and layouts, and the one the web app's plain addresses
mean; one per account, so every browser and phone of it shows the same
screens. `machine` is a current host of the account's chain (an active
device that runs AI), or `null`: none chosen yet, or the one chosen left
the account (removed, or no longer running AI — a stored value that is not
a current host is never returned). `set_ms`: when it was last set or
cleared (`null`: never). The same value comes over `/v1/client` as
`main_device`, after `welcome` and on every change (websocket.md).

A preference, not authority: a client uses it only if it names a host of the
chain it verified itself, and otherwise keeps its own choice. A hub from
before this route answers `404` (and sends no `main_device`): the client keeps
the main device per browser, as before.

### `PUT /v1/main-device` — requires a current device
`{machine, if_unset?}` → `{machine, set_ms}`, the account's main device
after the request. Any current device of the account's chain may set it
(the web app's 「设为主设备」), only to a current host of that chain; every
device of the account hears the change (`main_device`). Checked and written
under the account's chain lock, so a removal lands either before the write
(refused) or after it (cleared).

`if_unset: true` writes only while the account has none (never set, or the
one it had left), and otherwise changes nothing and answers the one that
stands — so the first page to seed it after the upgrade, or to repoint it
after its main device left, wins, and the others take the answer. Setting
the value the account already has changes nothing (`set_ms` stays, nothing
is pushed).

When the main device leaves the account (removed, or `SetHost false`), the
hub clears it (`main_device {machine: null}` to every device of the account)
and picks no successor: the web app picks one — the device in view, else the
chain's first that runs AI — and claims it with `if_unset`.
Errors: `400 bad_request` (`machine` is not an id), `403 not_in_chain` (the
session's device is not a current device of the account's chain),
`409 not_a_host` (`machine` is not a current host of this account's chain:
unknown, another account's, removed, no longer running AI, or a device that
runs none).

### Identity key renewals

A host's identity key is renewed without enrolling it again
(`docs/host-identity-rotation.md`): an admin device
registers a request here, asks the host over its end-to-end channel
(`POST /rh/rotate`), the host's agent stages its proof on its own `/v1/agent`
connection (`rotation_stage`, websocket.md), and once the window T is over
the admin writes `[RotateHostKey, KeyEpochs]` (`POST /v1/chain`). Only the
host's **current** agent connection stages (one replaced by a newer sign-in
stages nothing). While the renewal is staged, every agent sign-in with the
host's old key must confirm the new one within 20 s of `welcome`, or the
staging is void — graded (websocket.md): `interrupted` when that sign-in
went away before it answered, or the hub itself failed (no evidence; the
page asks again with a fresh nonce), `not_acked` when it answered wrong, said
`rotation_unknown`, or stayed open and silent (evidence). The host's agent
connection replaced while an attempt is open voids it, `replaced` (no
evidence); while staged, the new sign-in owes the confirmation (and the
devices hear `rotation {state: "staged", reason: "replaced"}`). A second
`interrupted` or `replaced` for a host within a day is `not_acked` — the
hub's own failures (a database error, a restart losing the connection that
owed) are shown as `interrupted` but never counted towards it.
Off unless `DROPPEN_HUB_HOST_ROTATION=on` (then `409 rotation_not_ready`,
`disabled`; `GET` answers either way); chain v3 accounts only. T is
`DROPPEN_HUB_ROTATION_WINDOW_SECS` (120 s).

A host's renewal is in one **state**: `pending` (exposed: renew it when it
can — offline, or its last attempt failed, `failure` saying why), `attempt`
(an admin's request, open 10 minutes, one at a time per host), `staged` (the
host's proof, staged by its agent; committable from `ready_ms`; void after
24 hours, `expired`) or `committed` (kept a day, then gone). Every change is
pushed to the account's devices (`rotation {host, state, reason?}`), and
evidence as `host_dispute`. While `pending`, `attempt` or `staged` — and
while isolated — the host's device role is suspended (`403 rotation_pending`,
`4403 rotation_pending`; its device sessions are revoked when the request is
registered; as a device it opens no channel); its agent role is not.
Asking (`PUT`) and withdrawing (`DELETE`) are serialised with each other and
with dispute statements; a withdrawal decides isolation in the transaction
that holds the host's row, so a host isolated meanwhile stays `pending`.
Every request with a host's device session, and the making of one, asks
Postgres whether its device role is suspended. A renewal's commit keeps the revocation of the host's relay
tickets in its own transaction.

**Evidence and isolation.** A sign-in with a retired key (agent, device,
device session) is refused `rotated` and kept — the signed `HubLogin` as it
came, at most 4 an hour per host; on a socket the refusal names the renewal
(`retired_seq`) and the signer may fetch the chain up to it before the close
(websocket.md) — but it isolates nothing: the one signing in may be the
host itself, restored from a backup. It counts as clone evidence (below)
only when the challenge it answered (the socket's, or the device session's
nonce) was issued more than 10 s after the key's retirement; one from
before, or just after, is the host racing its own commit and is listed as
information only (`evidence: false`). A sign-in that was valid when it
started and is refused because a renewal committed meanwhile is no dispute
at all. What isolates is the retired key's
**signed statement** disputing the renewal that retired it
([`POST /v1/rotations/{host}/disputes`](#post-v1rotationshostdisputes--no-session)):
for a renewal **not** confirmed on the device (assurance 0) with no renewal
confirmed on the device after it, the host is
**isolated**: `pending` again, no relay tickets to or for it, no channel
through the hub (its agent connections are closed, 4403 `rotation_pending`,
so a channel already open through the hub ends with them; it signs in
again), until a renewal confirmed on the device (assurance 1) is
committed, or it is removed. While an assurance-1 attempt (opened by an
admin's `PUT`, its 10 minutes) or staging (until 10 minutes past when it
could be committed) is open for it — and only then — the hub routes a
channel to it from a current **admin** device and issues the matching
tickets (the admin device's ticket names it; its own agent gets its
tickets), so the confirmation can reach it; once that attempt ends
(committed, voided, expired, withdrawn) the isolation applies again. A
voided staging (`not_acked`, `conflict`) is evidence too. Clone evidence
within 30 days (or isolation) means only assurance 1 is taken
(`needs_assurance`).

**Always confirm on the device** is the account's, on its chain: an
admin's `SetRenewalPolicy {always_assured}` (chain v3 kind 21). While it is
on, the chain's own rule (`ROTATE_NEEDS_ASSURANCE`, both verifiers) refuses
a `RotateHostKey` not confirmed on the device — the hub answers such a
batch `422 invalid_entry` like any other the chain refuses, and no hub can
switch it off. While it is on, the hub opens no attempt with assurance 0
either (`PUT /v1/rotations/{host}`: `409 rotation_not_ready`,
`needs_assurance`), one the chain would never take. The hub takes a
`SetRenewalPolicy` only under the renewal gate, like a `RotateHostKey`
(`409 rotation_not_ready`: `disabled`, `v2`, `waiting`), so every host that
reads kind 20 reads kind 21 too — and so is `RenewalPause` (kind 22), the
overlay a page opens over hosts' accept policies while a group of them is
renewed (and ends, `completed` or `cancelled`): every reader applies it to
who may control whom, so the hub only gates it — its start. The end of a
pause the chain holds open passes the gate and the switch (the chain takes
an end only for an open pause, so every reader of that chain reads kind 22
already, and the end only gives control back): switching renewals off, or a
host without `rotate/1` joining meanwhile, never strands an open pause.

#### `GET /v1/rotations`
Any device session of the account → `{now_ms, enabled, window_ms,
hosts: [Rotation], disputes: [Dispute]}` — `now_ms` the
hub's clock, which every age here is measured against:
- `Rotation`: `{host, state, assurance, requested_ms, requested_by,
  attempt_expires_ms?, staged_ms?, ready_ms?, sign_key?, dh_key?, staged_seq?,
  nonce_hash?, committed_ms?, committed_seq?, failure?: {reason: "not_acked"|"conflict"|"expired"|"refused"|"interrupted"|"replaced", at_ms},
  needs_assurance, isolated}` — `sign_key`, `dh_key`, `staged_seq` the
  staged (or committed) new keys; `nonce_hash` (an attempt or staging) the
  hash `PUT` opened it with; `requested_by` the admin device, or `hub`
  for a host isolated by evidence;
- `Dispute`: `{host, kind: "disputed"|"retired_login"|"not_acked"|"conflict", seen_ms,
  seq?, entry_hash?, isolates?, retired_seq?, evidence?, role?: "agent"|"device"|"device_session", login?: {account,
  id, hub, nonce, time_ms}, sig?, sign_key?}` (`seq`, `entry_hash`,
  `isolates`: a `disputed` statement; `retired_seq`, `evidence`, `role`,
  `login`: a `retired_login` — `evidence: false` for one racing its own
  commit), the last 90 days, newest
  first, at most 100 — and, besides those, always every dispute a host's
  isolation rests on, however old. Soft voids (`interrupted`, `replaced`)
  are no evidence and not listed. A `retired_login` carries the signed login as it came:
  a device checks `sig` over `HubLogin {role, account, id, hub, nonce,
  time_ms}` with the chain's `keys_at(host, retired_seq - 1)` itself — the
  hub can withhold evidence, never forge it. `login.hub` is the host the
  sign-in dialled, as signed: the hub's origin, or, for `device` and
  `device_session`, its app host (`wss://my.droppen.app`; README "Hosts").

#### `PUT /v1/rotations/{host}` — requires a current full device
`{nonce_hash?, assurance: 0|1}` → the host's `Rotation`. Without
`nonce_hash`: mark the host `pending` (idempotent). With it (SHA-256 of the
request's 16-byte nonce, hex): open an attempt, valid 10 minutes — the same
`nonce_hash` again answers the same; another while an attempt (or a staging)
of another request is open is `409 rotation_in_progress`.
Errors: `400 bad_request`, `409 not_a_host` (not a current host of this
account), `409 rotation_not_ready` with `reason` `disabled`, `v2`,
`agent_too_old` (its agent has not declared `rotate/1`), `policy_unset`
(an attempt before the account's chain has a `SetRenewalPolicy`: an admin
chooses, on or off, before the first — `docs/host-identity-rotation.md` §5.6 「打开的顺序」),
`needs_assurance` (an attempt with assurance 0 for a host that takes only
1, or while the account's chain asks for every renewal to be confirmed on
the device — `SetRenewalPolicy`),
`409 rotation_in_progress`. Limits: 6 attempts an hour and 20 a day per
host (`429 rate`; marking it pending is not one).

#### `DELETE /v1/rotations/{host}` — requires a current full device
`{nonce_hash?, requested_ms?}` (or no body) → `{}`: withdraw the renewal
(its state goes; `rotation {state: "none"}`). A committed one stays its
day; an isolated host stays `pending` (only a renewal confirmed on the
device, or its removal, clears it). With `nonce_hash`, only the attempt (or
staging) opened with it is withdrawn; with `requested_ms`, only the
`pending` mark made then — so a page cleaning up what it read earlier never
withdraws what another page started on the host since. Anything else the
host has stays: `409 rotation_in_progress` while an attempt or staging is
open, `{}` when the one named is gone. Errors: `400 bad_request` (both
given, a malformed or unknown field, or a body that is not an object).

#### `POST /v1/rotations/{host}/disputes` — no session
`{seq, entry_hash, sig}` → `{ok: true}`. A host whose identity key was
renewed elsewhere (its agent found a `RotateHostKey` for its id with keys
that are neither its seed nor its staged one) disputes that renewal with the
key it retired: `sig` is `rh/v1 rotate-dispute` (`rh_proto::chain::sign_rotate_dispute`)
over `str16 account | str16 host | u64 seq | entry_hash 32 | new_sign 32` —
`seq` the disputed entry, `entry_hash` its chain hash, `new_sign` the key it
put in — by `keys_at(host, seq - 1)`'s sign key. No session: the signature is
the credential, checked against the hub's chain
(`ChainState::verify_rotate_dispute`). Kept once per renewal (sending it again
answers the same) and told to the account's devices (`host_dispute {kind:
"disputed"}`); listed in `GET /v1/rotations` (`kind: "disputed"`, `seq`,
`entry_hash`, `sig`, `isolates`) — always, while it isolates. It isolates
the host when the disputed renewal was not confirmed on the device and none
of the host confirmed on the device came after it.
Errors: `404 no_such_renewal` (no such device, or entry `seq` is no renewal
of it), `410 host_removed` (the device left its account: stop sending),
`422 invalid_dispute` (`entry_hash` is not that entry's, or the signature is
not the retired key's over it), `429 rate`. Limits: 60 an hour per address,
for every statement; 10 an hour per host, for statements that verify only
(so junk cannot use up the host's own). What a statement does — kept,
isolating — is decided and applied in one step, serialised with the
account's chain: a renewal or a removal written meanwhile waits for it. A
statement sent again repairs an isolation that was cut short.

### The account's ways to sign in (`docs/passkeys.md` §3.4 B, §3.6, K2)

Passkeys and the password, as the account's settings page shows and changes
them. **Changing them takes a current full device** (`403
not_a_full_device` otherwise; `401 device_session_required` for a login
session): someone with a stolen password, or a stolen login link, must not
give themselves a lasting way in, while a full device already decides who
is in the account. Changing them also needs the account's e-mail verified
once its due date passed (`403 email_unverified`, as registering a device).
**The account always keeps one way to sign in**: a password or a passkey
(`409 last_credential`). Bodies up to 16 KiB (a credential).

**Proving it is still the person.** Adding or removing a passkey, and
setting, changing or removing the password, also take a proof besides the
device session — a browser left signed in must not let anyone give the
account a passkey or a password of theirs (either would then vouch for the
other), nor take the owner's away: `passkey: {challenge, credential}`, one
of the account's own passkeys answering a challenge `POST
/v1/passkeys/reauth/options` gave this very device (`credential` is
`get()`'s `AuthenticationResponseJSON`, checked as at sign-in; a login's
challenge is no answer, nor is this one a login); or, only where the
account has a password, `current_password`. An account made with a passkey
therefore sets its first password with a passkey, and one made with a
password adds its first passkey with the password. Every failed proof is
`403 reauth_failed` (never a 401, which a page reads as its session gone);
none given, `403 reauth_required`. The current password is checked at most
3 times per 10 minutes and 10 a day per device, and 6 times per 10 minutes
and 20 a day per account (`429 rate`; the device's share first, both before
the hash), with argon2 as a login's: one browser left signed in never spends
the account's whole share.

**Every change is recorded and pushed.** A passkey added, renamed or
removed, and the password set, changed or removed, is written with the
change itself — its kind, the device session that made it, the passkey's
label — and once made every device of the account signed in on
`/v1/client` hears `sign_in_changed` (websocket.md); `GET /v1/me` lists the
last 30 days' (`sign_in_changes`). A change nobody on the account's devices
made is then seen, wherever it came from. A refused change records nothing.

#### `GET /v1/passkeys` — requires a current device that runs no AI
→ `{passkeys: [passkey], password, max}`: the account's passkeys, oldest
first; whether it has a password; how many passkeys it may have (10). Each
`passkey` is `{id, label, provider, credential_id, cred_hash,
backup_eligible, backup_state, prf, transports, created_ms, created_by,
last_used_ms, unlock}`:
- `label`: what the person called it (the provider's name until renamed);
  `provider`: its provider's name by AAGUID, else `Passkey`;
- `credential_id` (base64url) and `cred_hash` (hex `SHA-256(credential
  id)`, what a passkey slot on the chain names): nothing secret — every
  assertion carries the id, and the page needs it to ask this very passkey
  for its PRF (`allowCredentials`, `docs/passkeys.md` §4.5);
- `backup_eligible` / `backup_state`: BE (it may sync: a passkey manager's)
  and BS (it does, as of its last use); `prf`: whether the browser said it
  can do PRF when it was made (`yes` / `no` / `unknown`, a hint only);
- `created_by`: the device that added it (`null`: made at sign-up);
  `last_used_ms`: its last sign-in (`null`: never);
- `unlock`: `on` when a current passkey slot on the account's chain names
  it (a new device can join with it), else `off`.
Errors: `403 not_in_chain` (pending, removed, outside the chain, or a
device that runs AI).

#### `POST /v1/passkeys/add/options` — requires a current full device
`{current_password? | passkey?}` → `{challenge, options}`, once the proof
above holds: a challenge bound to the account and this device, and
`PublicKeyCredentialCreationOptionsJSON` as for a sign-up
(`POST /v1/passkeys/options`) but under the **account's user handle** (made
now, 32 random bytes, for an account that had no passkey), with the e-mail
as the name the provider shows, and the account's passkeys as
`excludeCredentials` (a provider that holds one already makes none twice).
The PRF input in `extensions` is a throwaway: a page that wants the PRF
result at creation puts its own there (`docs/passkeys.md` §4.5); it is never
sent back. Nothing is stored but the user handle, the first time.
Errors: `403 passkeys_off | not_a_full_device | email_unverified |
reauth_required | reauth_failed`, `409 too_many_passkeys`, `429 rate`.

#### `POST /v1/passkeys` — requires a current full device
`{challenge, label?, credential}` → the new `passkey` (as listed above).
`credential` is `create()`'s `RegistrationResponseJSON`, checked as at
sign-up against a challenge this very device was given for this account;
`label` is 1–64 characters once trimmed, no control characters (absent: the
provider's name). Then the account's daily share of additions (`429 rate`),
its cap, and the passkey stored; it signs in at once.
Errors: `400 bad_passkey` (a refused credential, a challenge another device
or account was given, a replay — a 400, not the 401 of signing in: this
request has a session, which a 401 would say is gone), `400 bad_request`,
`403 passkeys_off |
not_a_full_device | email_unverified`, `409 too_many_passkeys |
passkey_exists` (the credential is registered already), `429 rate`.
Limits: 10 added a day per account, spent only by a passkey that checked
out; at most 10 at once.

#### `PATCH /v1/passkeys/{id}` — requires a current full device
`{label}` → the `passkey`, renamed (1–64 characters once trimmed, no control
characters). Errors: `400 bad_request`, `403 not_a_full_device |
email_unverified`, `404 not_found`.

#### `DELETE /v1/passkeys/{id}` — requires a current full device
`{current_password? | passkey? | grant?}` → `{id, removed: true}`, once the
proof above holds (or the `grant` below); it signs in no more (a client then
has the provider hide it: `signalUnknownCredential`, where the browser has
it). **The proof is checked first**: a wrong one, or none, is `403
reauth_failed | reauth_required` whatever the passkey's state. Then, refused
for a passkey a current passkey slot on the chain names (`409 unlock_on`):
take the slot off first (`PasskeySlotRemove`), so no slot is left naming a
passkey nobody can sign in with — a chain the hub cannot read now is no
answer (`503 unavailable`); and for the account's last way to sign in (`409
last_credential`: no password and no other passkey). After a password or a
passkey (not a grant), that `409 unlock_on` carries `grant`: the page signs
`PasskeySlotRemove` and sends `{grant}` back instead of a second proof. A
grant is bound to the account, this device and this passkey, good for 2
minutes and taken once, by the removal it finishes (another device's, or one
for another passkey, is `403 reauth_failed`; a grant sent back while the slot
is still on is `409 unlock_on` again, with no new grant). The slot check and
the removal hold the account's chain, so no `PasskeySlotSet` naming the
passkey lands between them. Errors: `403 not_a_full_device |
email_unverified | reauth_required | reauth_failed`, `404 not_found`, `409
last_credential | unlock_on`, `429 rate`.

```json
{"error": "unlock_on", "message": "new devices can join with that passkey: turn that off first", "grant": "AQV…"}
```

#### `POST /v1/passkeys/reauth/options` — requires a current full device
`{}` → `{challenge, options}`: a challenge bound to the account and this
device, for proving it is still the person (here and for
`DELETE /v1/account`, below), and
`PublicKeyCredentialRequestOptionsJSON` naming the account's own passkeys
in `allowCredentials` (with their transports), `userVerification:
"required"`. Nothing is stored. An account whose chain has no entries yet
(no device is full before the genesis) may ask with any of its device
sessions. It takes an unverified e-mail too: the route it serves checks its
own rules (changing a way to sign in still answers `403 email_unverified`
past the due date; deleting the account does not ask). Errors: `403
passkeys_off | not_a_full_device`, `409 no_passkey` (give the current
password instead).

#### `PUT /v1/password` — requires a current full device
`{password, current_password? | passkey?}` → `{password: true}`: the
account's password set (an account made with a passkey has none) or
changed, 10–1024 bytes, after the proof above. Hashed like a sign-up's
(argon2id, the same per-address and per-network slots, which wait rather
than refuse). From then on the e-mail and this password sign in, under the
login's lockout, backoffs and Turnstile rules as any password does.
Errors: `400 bad_request`, `403 not_a_full_device | email_unverified |
reauth_required | reauth_failed`, `429 rate`.

#### `DELETE /v1/password` — requires a current full device
`{current_password? | passkey?}` → `{password: false}`: the account signs
in with its passkeys only (a password login then answers exactly like a
wrong password), after the proof above. Refused without a passkey (`409
last_credential`). Errors: `403 not_a_full_device | email_unverified |
reauth_required | reauth_failed`, `409 last_credential`, `429 rate`.

### `DELETE /v1/account` — requires a current full device
`{confirm, current_password? | passkey?}` → `{deleted: true, purged}`:
the account deleted, for good. `confirm` is the account's e-mail address as
the person typed it (compared as the hub compares inboxes: trimmed, in any
case, a provider's own spellings of it — Gmail's dots, say; anything else is
`400 confirm_mismatch`), and the proof is the one above ("Proving it is
still the person": the current password where the account has one, or a
passkey answering a challenge `POST /v1/passkeys/reauth/options` gave this
device; the same errors, the same limits on the password). A current full
device of the account's chain asks — or, for an account whose chain has no
entries yet, any of its device sessions. An unverified e-mail does not
hold it. Body up to 16 KiB, as the passkey routes.

**What goes.** Every session and socket of the account ends at once (its
sockets close with 4403 after `error {code: "account_deleted"}`,
websocket.md), and the relays drop its connections and refuse its tickets.
The hub deletes the device chain, the devices, the machines (the hosts that
ran AI under the account), the mailbox, the keyring, identity key renewals,
enrollments, plan rows, the ways to sign in (password, passkeys), the
stored encrypted pages and stream tails, the records of verification mail
sent to the address, and the address's waitlist entry. The account keeps no
address from the moment its deletion begins, even while its stored data is
still being erased. **What stays:** a keyed hash (HMAC-SHA256 under a secret
of the hub's) of the inbox the address reaches (lower-cased, a `+tag`
dropped, and each mail provider's own rules — Gmail's dots, say), for the
days `GET /v1/signup/config` names (`deletion.email_hold_days`;
`DROPPEN_HUB_DELETED_EMAIL_DAYS`, 7 by default, `0` keeps none), so the same
inbox cannot sign up again at once (`409 recently_deleted`); for as long,
the ids and public signing keys of its devices and hosts, so a device that
comes back is told the account is gone (`POST /v1/session`, `410`); and an
account-wide revocation of relay tickets (the account's random id and a
time, sent to the relays as `revoke` with `id: null`) until the last ticket
it could stop has expired (a ticket's lifetime and 10 minutes).

Nothing deleted can be brought back, by anyone: the data was encrypted on
the account's devices, and with the account gone every device of it stops
working; hosts must be enrolled again under another account. The hub cannot
reach the account's other browsers: they keep what they hold until they
next ask the hub, which knows them no more. The device that deleted the
account forgets its keys for it there and then (the web app does). `purged:
false`: the stored data is still being erased; the hub finishes on its own,
and the account is gone either way.
Errors: `400 confirm_mismatch | bad_request`, `401 unauthorized |
device_session_required`, `403 not_a_full_device | reauth_required |
reauth_failed`, `429 rate` (with `retry_after_ms`), `503 unavailable`
(nothing was done: try again).

## Operator: `admin`

The `/v1/admin/*` routes are the hub operator's own tools. No open client
uses them, so they are documented with the Hub, not here. `index.json` still
lists their paths and auth (`admin`, a bearer token), so every route the hub
serves is in the index. (The entitlement keys are not operator prose: they
are named under `/v1/me`'s `plan`.)

## Served by a relay (`droppen-relay`)

Open source, listed because devices, agents and the hub rely on them.
Errors are `{error, message?}`; the 429 is `{error: "rate", retry_after_ms}`
with `Retry-After`.

| route | |
|---|---|
| `GET /healthz` | `200 ok` |
| `GET /v1/ping` | `204`, `Access-Control-Allow-Origin: *`, `Timing-Allow-Origin: *`, `Cache-Control: no-store`: what clients time to choose a relay (four requests, the first dropped). 120/minute per address |
| `GET /v1/relay-id?nonce=<64 hex>` | `{relay_id, sign_key, origin, sig}`: `sig` signs the relay's origin and the nonce (`rh_proto::ticket::sign_origin_proof`); the hub asks it at registration and when a relay moves. `400 bad_request` for a bad nonce. 120/minute per address |
| `GET /v1/connect` | the WebSocket devices and agents sign in on ([websocket.md](websocket.md#v1connect-device-or-agent--relay)). 60/minute and 20 per 20 s per address |
