# WebSockets

| path | served by | who connects | carries |
|---|---|---|---|
| `/v1/client` | the hub | a device (the web app, `rh-client`: the test device and the local install's device role) | control text, rh-proto frames |
| `/v1/agent` | the hub | a machine's agent | control text, rh-proto frames |
| `/v1/relay` | the hub | a relay (`droppen-relay`) | control text only |
| `/v1/connect` | a relay | a device or an agent, with a ticket | control text, a subset of the frames |

Every **text** message is one JSON object tagged by `t`
([`schema/ws.schema.json`](schema/ws.schema.json): `$defs["<path> c2s"]`,
`$defs["<path> s2c"]`, and one `$defs["<short>.<dir>.<t>"]` per message,
`short` being `client`, `agent`, `relay` or `connect`). Every **binary**
message is exactly one `rh_proto::frame` frame. `c2s` is client to server
(device, agent or relay to the hub; device or agent to a relay). Ignore
unknown `t` values and unknown fields; the server answers an unknown `t` it
was sent with `error` and a close (see the close codes below), so a client
sends a new message type only after negotiating it.

## Sign-in

Every socket opens with a `challenge` from the server, and the client answers
with a signature over `rh_proto::sig::HubLogin {role, account, id, hub,
nonce, time_ms}` (label `rh/v1 hub-login`):

| role | socket | `account` | `id` | `hub` | signed with |
|---|---|---|---|---|---|
| `Device` | `/v1/client` | the account | the device | the hub origin dialled | the device key |
| `Agent` | `/v1/agent` | the account | the machine | the hub origin dialled | the machine key |
| `Relay` | `/v1/relay` | `""` | the relay id | the hub origin dialled | the relay's key |
| `RelayDevice`, `RelayAgent` | `/v1/connect` | the ticket's account | the ticket's id | **the relay's** origin | the key the ticket names |
| `DeviceSession` | `POST /v1/session` (HTTP) | the account | the device | the hub origin dialled | the device key |

`hub` is always the origin the client dialled (`wss://host[:port]`), never the
one a `challenge` claims: a signature for one server is worthless at
another. `time_ms` must be within 5 minutes of the server's clock. Signing in
takes at most 30 s on the hub (10 s on a relay).

## `/v1/client` (device → hub)

```
hub    → challenge {nonce, hub, version: 1}
device → login {role: "device", account, id, time_ms, sig, session}
hub    → welcome {account, id, chain_seq}
hub    → presence {machines: [...]}            the snapshot
hub    → plan {…, detail}                       if the plan can be read
hub    → main_device {machine, set_ms}          the account's main device
         … then frames and control both ways
```

A refused sign-in is `error {code, message}` and a close: 4401 (`denied`:
bad signature, clock, session not this device's own device session) or 4403
(`removed`; `host_devices_off`: a host signing in as a device while the hub
runs `DROPPEN_HUB_CHAIN_V3=off`; `account_suspended`). The web app renews its device session (HTTP) after a 4401 and
reconnects.

A sign-in also records when the device was last seen and, if the upgrade's
`User-Agent` names a known device or browser, the device's short name
(`GET /v1/me` `devices[].ua_label` in http.md); the `User-Agent` itself is not
kept.

**Device → hub**

| `t` | fields | |
|---|---|---|
| `login` | `role: "device"`, `account`, `id`, `time_ms`, `sig` (64-byte hex), `session` | `session`: this device's own **device** session token (`POST /v1/session`); a login session or another device's is refused |
| `chain_sub` | `from` | send the chain from index `from` (`CHAIN` frames on `ch` 0), then every entry as it is appended |
| `stream_unsub` | `ch` | end the stream subscription on `ch` |

**Hub → device**

| `t` | fields | |
|---|---|---|
| `challenge` | `nonce` (32-byte hex), `hub`, `version` | first message on every hub socket |
| `welcome` | `account`, `id`, `chain_seq` (`null`: no chain yet), `features` | signed in. `features`: what the hub speaks beyond the base protocol — `rotate/1` (identity key renewals; every hub with them offers it, whatever `DROPPEN_HUB_HOST_ROTATION` says). A peer sends a feature's messages only on a connection whose `welcome` offered it: a hub without it answers an unknown `t` with 4400. Absent from hubs before renewals |
| `presence` | `machines: [presence]` | a snapshot after `welcome`, then only changes |
| `plan` | `plan`, `retention_days`, `storage_bytes` (`null`: none), `usage_bytes`, `grace_until_ms?`, `detail` | after `welcome` and on every change; `detail` is `GET /v1/me`'s `plan` |
| `mail` | `id`, `sender`, `purpose`, `epoch`, `sealed` | mail for this device was posted (it stays in `GET /v1/mailbox` until acknowledged over HTTP) |
| `device_registered` | `device`, `created_ms` | a device was registered with the account's password (`POST /v1/devices`); worth a look if it was not you |
| `sign_in_changed` | `id`, `kind`, `device` (`null`: none), `label` (`null`: the password's), `at_ms` | how the account signs in changed (http.md, "The account's ways to sign in"): `kind` is `passkey_added`, `passkey_renamed`, `passkey_removed`, `password_set` (it had none), `password_changed` or `password_removed`; `device` the device session that did it; `label` the passkey's (the new one, for a rename). Sent to every device of the account once the change is made, exactly as `GET /v1/me`'s `sign_in_changes` lists it (`id` grows); worth a look if it was not you |
| `main_device` | `machine` (`null`: none), `set_ms` (`null`: never set) | the account's main device (http.md `GET /v1/main-device`): after `welcome`, and whenever it is set or cleared (it left the account). The snapshot is read after the connection is registered, so a change made meanwhile may arrive before it: keep the one with the newer `set_ms` |
| `chain_result` | `ok`, `seq` (`null` on failure), `error` (`null` on success; the reason in words, starting with the code where there is one: `chain_v2_genesis:`, `email_unverified:`) | the answer to a `CHAIN` frame this device sent |
| `stream_synced` | `ch`, `stream`, `next_seq`, `exists` | a subscription caught up (`exists: false`: no such stream yet; it stays and follows) |
| `stream_refused` | `ch`, `stream`, `code`, `retry_ms?` | a subscription the hub will not serve: `forbidden`, `invalid` (a `ch` in use), `limit` (256 per connection) |
| `rotation` | `host`, `state`, `reason?` | a host's identity key renewal changed state (http.md [identity key renewals](http.md#identity-key-renewals)): `pending`, `attempt`, `staged`, `committed`, or `none` (withdrawn); `reason` when an attempt or a staging failed (`not_acked`, `conflict`, `expired`, `refused`, `interrupted`, `replaced`: the last two no evidence — ask again with a fresh nonce), or with `state: "staged"` and `reason: "replaced"` when a staged host's agent connection was replaced (the new sign-in owes the confirmation). `GET /v1/rotations` has the rest |
| `host_dispute` | `host`, `kind`, `seen_ms` | evidence that someone else holds a host's key: `disputed` (a retired key's signed statement disputing the renewal that retired it — the only kind that isolates the host; http.md `POST /v1/rotations/{host}/disputes`), `retired_login` (a sign-in with a key a renewal retired), `not_acked`, `conflict` (a voided staging). At most 4 an hour per host; `GET /v1/rotations` `disputes` has the evidence itself |
| `error` | `code`, `message`, `fp?` | see the codes below |

`presence` items: `{id, state: "online"|"offline"|"unknown", since_ms,
via?: [{relay, rtt_ms?}], relays?: bool}` — `unknown` for a while after the
hub restarted; `via`: the relays the machine says it is signed in on
(information, not authority); `relays`: whether its agent attaches to relays
(absent until it said).

**Frames.** Device → hub: `PING` (every 20 s; answered `PONG`), `CHAIN {ch:
0, entries}` (append; answered `chain_result`), `ATTEST {ch: 0}` (this
device's attestation of the chain head, relayed to the account's agents;
`error bad_attestation` if refused), `SUB {ch, stream, from_seq}` (a stream
subscription), and `OPEN/DATA/CREDIT/CLOSE` for end-to-end channels where
the hub still carries them. Hub → device: `PONG`, `CHAIN {ch: 0}`, and on a
subscription's `ch`: `GONE`, `CHECKPOINT`, `RECS`, `HEAD`, `EPHEMERAL`; the
catch-up ends with `stream_synced`. A hub that carries no channels (the relay
cut-over) answers `OPEN` with `CLOSE` 4426 and drops `DATA/CREDIT/CLOSE`:
channels go through a relay (`/v1/connect`).

## `/v1/agent` (agent → hub)

```
hub   → challenge {nonce, hub, version: 1}
agent → login {role: "agent", account, id, time_ms, sig}
hub   → welcome {account, id, chain_seq, features: ["rotate/1"]}
hub   → stream_watched {streams}, plan {…, machine_usage_bytes}
hub   → ATTEST frames (the stored attestations), relay_set {…}
hub   → mail × ≤ 64                              the first page of its mailbox
agent → agent_info {version, features}, chain_sub {from}
hub   → rotation_pending {…}                     only while a renewal of this host is staged,
                                                 once agent_info declared rotate/1
agent → rotation_ack {sign_key, sig}             within 20 s of welcome
```

**Renewing the host's identity key** (`rotate/1`, offered in `welcome`;
http.md [identity key renewals](http.md#identity-key-renewals)). Once an
admin registered an attempt (`PUT /v1/rotations/{host}` with `nonce_hash`)
and asked the host over its channel, the agent stages its proof on its own
connection, signed in with its current key:

```
agent → rotation_stage {nonce, staged_seq, staged_head, sign_key, dh_key, assurance, old_sig, new_sig}
hub   → rotation_staged {nonce_hash, ready_ms}   or rotation_refused {nonce_hash, reason}
```

The hub checks that `SHA-256(nonce)` is an open attempt's (with the same
`assurance`), that `staged_seq`/`staged_head` is an entry of its chain, that
the host's current key and the new key both signed the proof (`rh/v1
host-rotate`, `rh_proto::chain::HostRotation`) and that neither new key was
ever named by the chain. `reason`: `no_attempt`, `not_on_chain`, `bad_proof`,
`key_reused` (both end the attempt: `failure: refused`), `conflict` (another
staging for a staged host: it is void, `not_acked`'s twin), `disabled`. Never
fatal; the same staging again answers the same. Only the host's **current**
agent connection stages: a connection replaced by a newer sign-in (4001) is
dead from that moment, and whatever it still sends is dropped (a socket
delivers data for a while after the hub's close). Any other agent connection
of the host still open when it stages owes the confirmation below.

**Sign-in errors.** A sign-in the hub cannot check now (its database) is
`error {code: "unavailable"}` and 1011 — never `denied`, so an agent never
takes it for a word about its key; it backs off and tries the same key again.
A sign-in whose key changed between its check and `welcome` (a renewal
committed meanwhile) is `error {code: "rotated"}` and 4401.

**The window.** While the host is staged, every sign-in with its current
(old) key owes the new key's confirmation within 20 s of `welcome`: once its
`agent_info` declares `rotate/1` it gets `rotation_pending {sign_key, dh_key,
staged_seq, staged_head, nonce_hash, assurance, body_hash}` (`body_hash`:
SHA-256 of the proof body) and answers `rotation_ack {sign_key, sig}` — the
new key's `rh/v1 rotate-ack` signature over this connection's challenge
`nonce` (`rh_proto::chain::sign_rotate_ack`) — or `rotation_unknown
{sign_key}` if it does not hold that key. Any sign-in in the window that
does not confirm voids the staging, graded:

- `not_acked` (clone evidence: a dispute, and 30 days of assurance 1 only):
  `rotation_unknown`, a wrong signature, or a connection still open and
  silent at its deadline (one that never declares `rotate/1` included);
- `interrupted` (no evidence; the page asks again with a fresh nonce): the
  connection went away — closed, timed out, replaced — before it answered,
  or the hub itself failed to note what it owes (or lost the connection to
  a restart);
- a second `interrupted` (or `replaced`, below) for the host within a day is
  `not_acked` — but the hub's own failures are never counted.

**Replaced.** The host's agent connection replaced by a newer sign-in (4001)
while an attempt is open voids the attempt, `replaced` (graded like
`interrupted`); while staged, the new sign-in owes the confirmation, and the
devices hear `rotation {host, state: "staged", reason: "replaced"}`.

**On the commit** the host's connections get the `CHAIN` frame with the
renewal first, then `error {code: "rotated", message, fp}` (`fp`: the new
key's fingerprint) and a close with **4005**: sign in again with the new key.
**A sign-in with a retired key** (one a renewal replaced; on `/v1/agent`
and `/v1/client` alike) is refused `error {code: "rotated", message,
retired_seq}` — `retired_seq` the `RotateHostKey` that retired it — and kept
(it isolates nothing: an agent that finds the renewal foreign disputes it
with a signed statement over HTTP instead). The refusal is no evidence to the
signer by itself: the socket stays open up to 10 s for exactly one
`chain_sub {from}`, answered with `CHAIN` frames from `from` up to and
including that `RotateHostKey` and the `KeyEpochs` right after it — never
beyond — so the agent can check the renewal against its own chain; then it
closes with 4401. No `chain_sub` within 10 s, or anything else (any other
message or frame), closes it 4401 at once.

```
agent → login {…}                                signed with a retired key
hub   → error {code: "rotated", message, retired_seq}
agent → chain_sub {from: its seq + 1}            optional, within 10 s
hub   → CHAIN frames from..=retired_seq + 1
hub   ✕ close 4401
```

An agent from before renewals takes it for an ordinary refusal and backs off.

**Enrollment** replaces `login` once, with the 16-byte token of the code a
full device announced (`POST /v1/enrollments`):

```
agent → enroll {token}
hub   → enroll_assigned {account, machine}       (or error unknown_enrollment; 3 tries per connection)
agent → enroll_proof {sign_key, dh_key, time_ms, sig, proof}
hub   → welcome …                                as above: this is also its sign-in
```

`sig` is the `HubLogin {role: Agent}` signature under the new `sign_key` with
the assigned ids (proof the agent holds it); `proof` is the enrollment proof
the device checks (`GET /v1/enrollments/{token}`) before it adds the host
(`[AddDevice3{host}, KeyEpochs]`; v2: `AddMachine`). Until the machine is in
the chain its stream writes are dropped.

**A removed machine** (its account's chain has a `RemoveMachine` naming it)
is cut off and stays out: its live connection gets the `CHAIN` frame with
the removal first, then `error {code: "removed"}` and a close with 4403, and
every later `login` as it is refused the same way (after the signature is
checked). An agent of a suspended account is refused with
`account_suspended` (4403) until the account is unsuspended; one of a
deleted account is cut off (`account_deleted`, 4403), and the hub knows it no
more. The open agent does **not** take the hub's word for it: a refused
sign-in is a failure it shows and retries with backoff; only a
`RemoveMachine` in its own verified chain stops it for good. Re-joining means
a new enrollment (a new machine id). A machine that was never in the chain,
or is in it, signs in as before.

**Agent → hub**

| `t` | fields | |
|---|---|---|
| `login` | `role: "agent"`, `account`, `id`, `time_ms`, `sig` | sign in as an enrolled machine |
| `enroll` | `token` (16-byte hex) | claim an enrollment code |
| `enroll_proof` | `sign_key`, `dh_key`, `time_ms`, `sig`, `proof` (32-byte hex each but `sig`) | finish it |
| `agent_info` | `version` (≤ 64 characters kept), `features` | what it speaks: `relays/1` (it attaches to the account's relays; devices hear it as `presence.relays`); `chain/3` (it reads chain v3: until it says so, the hub sends it no v3 chain entry); `policy/1` (it refuses the admin-only routes to devices that are not admins). An account goes to v3 only once every current host declared both, and a host joins a v3 account only once it declared them (`GET /v1/me` `chain_v3`). Send it before `chain_sub` |
| `chain_sub` | `from` | as on `/v1/client` |
| `mail_ack` | `id` | done with a mail: it is deleted (only its own machine keys, purpose 2: the rest of its mailbox is its device role's) |
| `mail_release` | `id` | cannot judge a mail yet and will not hold it: it stays for the next sign-in |
| `relay_tickets` | `relays: [id]` | tickets for these relays of the account's set (8 at most are served) |
| `relay_state` | `rtts: {relay: ms}`, `attached: [relay]` | its round trip to every listed relay and the ones it is signed in on (≤ 64 and 16 kept) |
| `stream_list` | | list the streams the hub holds for this machine |
| `stream_delete` | `stream` (32-byte hex) | delete one of its streams (ignored unless the machine is current in the chain) |
| `rotation_stage` | `nonce` (16-byte hex), `staged_seq`, `staged_head`, `sign_key`, `dh_key` (32-byte hex), `assurance` (0 or 1), `old_sig`, `new_sig` (64-byte hex) | `rotate/1` only: stage the host's proof for an admin's renewal attempt (above) |
| `rotation_ack` | `sign_key`, `sig` (64-byte hex) | `rotate/1` only: confirm the staged key, answering `rotation_pending` |
| `rotation_unknown` | `sign_key` | `rotate/1` only: this agent does not hold that key |

`agent_info.features` also says `rotate/1`: it renews its identity key
(`GET /v1/me` `machines[].rotates`). A batch with a renewal is taken only once
every current host declared it.

Mail is pushed a page (64) at a time; the next page comes once every mail of
the last is answered with `mail_ack` or `mail_release`. Only purpose 2 (the
host's own machine keys) is pushed here: anything else for the host is its
device role's (`GET /v1/mailbox`).

**Hub → agent**

| `t` | fields | |
|---|---|---|
| `challenge`, `welcome`, `error` | as on `/v1/client` | |
| `enroll_assigned` | `account`, `machine` | the ids this enrollment gets |
| `mail` | `id`, `sender`, `purpose`, `epoch`, `sealed` | a sealed key for this machine (pushed; answer each) |
| `plan` | `plan`, `retention_days`, `storage_bytes`, `usage_bytes`, `machine_usage_bytes`, `grace_until_ms?` | keep the offline copy within `retention_days` and its share of `storage_bytes` |
| `relay_set` | `relays: [{id, origin}]`, `emergency: [{id, origin}]`, `limit`, `directory: [{id, origin, state}]` | the relays to attach to (the set's members that are listed, and any emergency grants), `relay.max`, and every listed relay to measure; again whenever it changes. Once the agent's `agent_info` names `relays/1` the hub keeps the set itself — one working public relay, two with the account's standby preference (`relay.max` allowing), nearest first by the agents' `relay_state` round trips — and replaces a member that drains, goes down or loses its control link at once: no device has to ask. A member it drops is left once no channel is open on it |
| `relay_tickets` | `tickets: [ticket]`, `errors: [refusal]` | the answer to `relay_tickets` (shapes as `POST /v1/relays/tickets`) |
| `stream_watched` | `streams: [hex]` | which of its streams have live readers now (send drafts only then) |
| `stream_list` | `streams: [{stream, next_seq}]` | the answer to `stream_list` |
| `stream_refused` | `ch`, `stream`, `code`, `retry_ms?` | a write it will not take (not a CAS answer): `quota`, `rate` (retry after `retry_ms`), `compact` (checkpoint first), `forbidden`, `missing_pages`, `invalid`, `unavailable` (retry after `retry_ms`) |
| `rotation_staged` | `nonce_hash`, `ready_ms` | the staging is kept; it may be committed from `ready_ms` |
| `rotation_refused` | `nonce_hash`, `reason` | the staging is not kept (above) |
| `rotation_pending` | `sign_key`, `dh_key`, `staged_seq`, `staged_head`, `nonce_hash`, `assurance`, `body_hash` | confirm the staged key (above) |

**Frames.** Agent → hub: `PING`, `APPEND {ch, stream, expect_seq, records}`
(answered `ACK {ch, head}` or `CONFLICT {ch, head}`), `HEAD {head}`, `PAGE
{ch, stream, hash, data}`, `CHECKPOINT {ch, stream, upto_seq, manifest}`
(answered `ACK`/`CONFLICT`), `EPHEMERAL {stream, payload}` (to live readers,
never stored), and `OPEN/DATA/CREDIT/CLOSE` answers on channels the hub
carries. Hub → agent: `PONG`, `ACK`, `CONFLICT`, `ATTEST` and `CHAIN` on
`ch` 0, and channel frames. Heads and manifests must verify under the
machine's enrolled key.

## `/v1/relay` (relay → hub)

Text only (a binary message closes it, 4400); messages up to 4 KiB until the
relay has signed in (`login` or `register`; a longer one is answered
`error {code: "too_large"}` and closes the link, 1009), then up to 1 MiB.

```
hub   → challenge {nonce, hub, version: 1}
relay → login {id, time_ms, sig, version, features?}
hub   → welcome {id, ticket_keys: [{kid, key}]}
        — or error {code: "unknown_relay"}, and the relay may, once:
relay → register {token, sign_key, origin, version, time_ms, sig, features?}
hub   → registered {id, ticket_keys}            (its key is pinned from now on)
hub   → revoke × n, replayed {revocations}, throttle × n, throttles {accounts}
```

`features` names what the relay takes beyond this link's first version:
`revoke-role/1`, a `revoke` scoped to one ticket role (below). A hub sends a
scoped revocation only to a relay that names it; to one that does not, it
sends none (an unscoped one would take the other role's tickets too), so
relays are released with `revoke-role/1` before a hub that scopes
revocations.

At registration the hub fetches `GET <origin>/v1/relay-id?nonce=…` to see
that the relay answers at the origin it claims. A sign-in under a key that is
not the pinned one is refused for good (`key_mismatch`, 4401): a relay never
quietly becomes another. A relay signing in again closes its previous link
(4001).

**Relay → hub**

| `t` | fields | |
|---|---|---|
| `login` | `id`, `time_ms`, `sig`, `version?` | sign in under the pinned key |
| `register` | `token` (`RHR1-…`; `DRR1-…` with the same body is the same token), `sign_key`, `origin`, `version?`, `time_ms`, `sig` | a first sign-in with a registration token (`sig` under `sign_key`) |
| `health` | `conns`, `channels`, `bytes_in_per_s`, `bytes_out_per_s`, `load` (0–1), `version` | every 20 s |
| `usage` | `boot`, `seq`, `from_ms`, `to_ms`, `accounts: [{account, bytes}]` | relayed `DATA` bytes per account in a window, every minute until acknowledged (`(boot, seq)` makes it idempotent; ≤ 500 accounts a report) |
| `meta` | `city?`, `version?`, `origin?` | update the directory (a new origin is proved like a registration) |
| `draining` | `until_ms` | it is shutting down: no new tickets for it |
| `deregister` | `reason?` | retire it (answered `deregistered`, then closed 1000) |
| `rotate` | `new_key`, `time_ms`, `sig_new` | pin a new key (`sig_new`: the same challenge signed with the new key) |

**Hub → relay**

| `t` | fields | |
|---|---|---|
| `challenge` | `nonce`, `hub`, `version` | |
| `welcome`, `registered` | `id`, `ticket_keys: [{kid, key}]` | the keys tickets are signed with |
| `error` | `code`, `message` | `unknown_relay` (register next), `key_mismatch`, `denied`, `bad_token`, `bad_key`, `bad_origin`, `origin_proof` (the relay retries later), `origin_taken`, `refused`, `retired`, `too_many_attempts`, `bad_message`, `sign_in_first`, `unexpected`, `unavailable` |
| `usage_ok` | `boot`, `seq` | a usage report is recorded |
| `revoke` | `account`, `id` (`null`: the whole account, e.g. a deleted one), `at_ms`, `role?` (`"device"` or `"agent"`: only the tickets of that role — a host's device-role and agent tickets carry the same id; absent: both) | close those connections and refuse tickets issued at or before `at_ms`; replayed at every sign-in while in force. `role` only to a relay that named `revoke-role/1`; a relay ignores a role it does not know |
| `replayed` | `revocations` | the replay is complete (a relay that just started waits for this before taking sign-ins) |
| `throttle` | `account`, `bytes_per_sec` (`null`: lifted) | one account's relayed rate |
| `throttles` | `accounts: [{account, bytes_per_sec}]` | the whole throttle table: anything not in it is not throttled |
| `deregistered`, `rotated` | `id` | answers |
| `keys` | `ticket_keys` | **reserved**: the relay replaces its ticket keys; the hub does not send it yet |
| `drain` | | **reserved**: the relay says `goaway` to everyone; the hub does not send it yet |

## `/v1/connect` (device or agent → relay)

```
relay  → challenge {nonce, relay, origin, version: 1}
client → login {ticket, time_ms, sig}
relay  → welcome {relay, id, expires_ms}
relay  → presence {machines: [{id, state}]}      devices only: the ticket's machines on this relay
         … frames both ways; renew before expires_ms:
client → ticket {ticket}
relay  → ticket_ok {expires_ms}
```

The ticket comes from `POST /v1/relays/tickets` (devices) or `relay_tickets`
on `/v1/agent` (agents); `sig` is `HubLogin {role: RelayDevice|RelayAgent}`
naming **the relay's origin**, under the key the ticket names. A relay that
just started waits up to 5 s for the hub's replay of revocations before
answering. A ticket that runs out and is not renewed closes the connection
(4419). A relay takes a ticket (at sign-in or as a renewal) only if it runs
no longer than the hub issues them for, `expires_ms − issued_ms` at most 15
minutes (`rh_proto::ticket::MAX_TICKET_LIFETIME_MS`), and its `issued_ms` is
at most 5 minutes ahead of the relay's clock (`TICKET_CLOCK_SKEW_MS`).
Anything else is refused as any bad ticket is: at sign-in, `error denied`
and a close (4401); as a renewal, `error bad_ticket` with the connection left
open on the ticket it has — which, not renewed, closes it 60 s after it runs
out (4419).

| `t` | dir | fields | |
|---|---|---|---|
| `login` | c2s | `ticket`, `time_ms`, `sig` | |
| `ticket` | c2s | `ticket` | a renewal (same holder) |
| `challenge` | s2c | `nonce`, `relay`, `origin`, `version` | |
| `welcome` | s2c | `relay`, `id`, `expires_ms` | |
| `presence` | s2c | `machines: [{id, state: "online"|"offline", since_ms?}]` | devices: a snapshot (no `since_ms`), then changes |
| `ticket_ok` | s2c | `expires_ms` | |
| `goaway` | s2c | `retry_ms`, `reason` | the relay is draining: move to another |
| `error` | s2c | `code`, `message`, `retry_ms?` | `denied`, `limit`, `expired`, `bad_ticket` (a renewal refused; the connection stays), `busy`, `bad_message`, `sign_in_first`, `unexpected` |

**Frames**: `OPEN` (devices only; an agent's is answered `CLOSE` 4400),
`DATA`, `CREDIT`, `CLOSE`, `PING`, `PONG`; any other critical frame closes
the connection (4400). The relay copies `DATA` without parsing it.

## Binary frames

`[type u8][ch u32][len u16][payload]`, at most 64 KiB with the header; the
codec and payloads are `rh_proto::frame` (and `web/src/lib/e2e/frame.ts`).
Types `≥ 0x80` are optional and skipped when unknown; an unknown type below
that closes the connection (4400).

| type | name | where |
|---|---|---|
| `0x01` | `OPEN` | device → hub/relay: a channel to a machine (the server rewrites the peer) |
| `0x02` | `DATA` | channels: an opaque end-to-end payload |
| `0x03` | `CLOSE` | channels: the end, with a code (below) |
| `0x04` | `CREDIT` | channels: flow control |
| `0x05` / `0x06` | `PING` / `PONG` | liveness on `ch` 0 |
| `0x10` | `APPEND` | agent → hub |
| `0x11` / `0x12` | `ACK` / `CONFLICT` | hub → agent |
| `0x13` | `SUB` | device → hub |
| `0x14` | `RECS` | hub → device |
| `0x15` | `GONE` | hub → device |
| `0x16` | `CHECKPOINT` | both (agent writes, device reads) |
| `0x17` | `EPHEMERAL` | both |
| `0x18` | `HEAD` | both |
| `0x19` | `PAGE` | agent → hub |
| `0x20` | `CHAIN` | both, `ch` 0 |
| `0x21` | `PUSH_SEND` | reserved |
| `0x22` | `ATTEST` | device → hub → agents, `ch` 0 |

## Limits on the sockets

| | hub (`/v1/client`, `/v1/agent`) | hub (`/v1/relay`) | relay (`/v1/connect`) |
|---|---|---|---|
| sign in within | 30 s | 30 s, 2 attempts | 10 s |
| control message | ≤ 16 KiB (else 1009) | ≤ 4 KiB before sign-in (else 1009), then ≤ 1 MiB | ≤ 16 KiB (else 1009) |
| frame | ≤ 64 KiB | — | ≤ 64 KiB |
| silence | 90 s closes (4002); send `PING` every 20 s | 90 s | 90 s |
| inbound rate | bytes: the plan's `conn.bytes_per_sec` (4 MiB/s by default), 1000 frames/s, bursts 2×; a text message costs 10 frames; over it the hub stops reading (backpressure) | — | the ticket's rate (or a throttle) and frames/s, the same way |
| outbound queue | 32 MiB, then 1013 | 32 MiB | 32 MiB, then 1013 |
| channels | 64 per device connection, 1024 per agent connection | — | the ticket's |
| other | 3 enrollment attempts per connection; 256 stream subscriptions per connection | | 48 connections per account per relay |

## Close codes

**WebSocket close codes**

| code | meaning | from |
|---|---|---|
| 1000 | normal (a relay deregistered; the server is done) | all |
| 1009 | a control message too large | hub, relay |
| 1011 | the server cannot serve now (database, chain); reconnect later | hub |
| 1012 | the server is restarting; reconnect | hub, relay |
| 1013 | not reading fast enough (outbound budget), or the relay is full or shutting down | hub, relay |
| 4001 | the same device, machine or relay signed in again elsewhere (on a relay: the same account's machine id; another account's machine of the same id is another machine) | hub, relay |
| 4002 | no heartbeat (silent for 90 s) | hub, relay |
| 4005 | this host's identity key was renewed (`error rotated` with the new key's fingerprint before it): sign in with the new key. Only to a renewed host's agent and device connections | hub |
| 4400 | a message or frame the server does not understand, or one not allowed here | hub, relay |
| 4401 | sign-in refused, or not signed in in time (renew the session, then reconnect); `rotated`: signed with a key a renewal retired (after one `chain_sub`, or 10 s) | hub, relay |
| 4403 | removed from the account (device or machine), revoked (relay), or a retired relay; a host's device role while its identity key is being renewed, or an isolated host's agent connection (`rotation_pending`); the account suspended or deleted (`account_suspended`, `account_deleted`; on a relay, the account-wide `revoke` of a deleted account) | hub, relay |
| 4409 | the enrollment was already completed | hub (`/v1/agent`) |
| 4419 | the ticket ran out and was not renewed | relay |
| 4429 | too many attempts (enrollment, relay registration), or over the account's connections on a relay | hub, relay |

**Channel `CLOSE` codes** (inside `CLOSE` frames, per channel; the
connection stays):

| code | meaning | from |
|---|---|---|
| 1000 | closed normally (codes outside this table are the two ends' own) | either end |
| 4400 | protocol (channel numbers must increase; agents do not open channels) | hub, relay |
| 4403 | not allowed (the device or machine is not current in the chain; the host does not accept control from the device; or the device is not an admin and the host's agent has not declared `policy/1`) | hub, relay |
| 4404 | the machine is offline | hub, relay |
| 4408 | flow control broken (sent past credit), or the agent saw no first message in time | relay, hub, agent |
| 4409 | too many channels | hub, relay |
| 4410 | the other end went away | hub, relay |
| 4412 | stale chain view: attest and retry | agent |
| 4426 | this hub carries no channels: use a relay | hub |

Codes 4408 and 4412 from the agent are end to end: the hub and relays pass
`CLOSE` frames between the two ends unchanged.

## Error codes on the sockets

`error {code, message}` before a close, or alone for something recoverable:

| code | close | |
|---|---|---|
| `bad_message` | 4400 | not a control message this server understands |
| `unexpected` | 4400 | a message this peer may not send now (or ever) |
| `bad_frame` | 4400 | not one whole valid frame |
| `sign_in_first` | 4401 | anything but sign-in before it |
| `denied` | 4401 | the sign-in does not verify |
| `removed` | 4403 | the device or machine was removed from the account (the chain names it removed); do not reconnect as it |
| `forbidden` | 4403 | `SUB` from a device not current in the chain |
| `host_devices_off` | 4403 | a host signing in as a device while the hub runs `DROPPEN_HUB_CHAIN_V3=off` |
| `account_suspended` | 4403 | the hub's operator suspended the account (README "Suspended accounts"): every connection of it is closed, and every sign-in refused, until that is lifted |
| `account_deleted` | 4403 | the account was deleted (http.md `DELETE /v1/account`): every connection of it is closed; do not reconnect — its devices and machines are gone, and a device session renewal signed with its key answers `410 account_deleted` (http.md `POST /v1/session`). The web app wipes its keys for the account on either |
| `rotated` | 4005 or 4401 | 4005: this host's identity key was just renewed (`fp`: the new key's fingerprint), sign in with the new key; 4401: the sign-in was signed with a key a renewal retired (`retired_seq`: that renewal; kept as evidence, the host may be isolated) — the close comes after one `chain_sub` answered up to that renewal, or 10 s (see `/v1/agent`) |
| `rotation_pending` | 4403 | a host signing in as a device (or signed in as one) while its identity key is being renewed: retry once the renewal is committed or withdrawn; or, on `/v1/agent`, the host was just isolated (http.md, identity key renewals): its channels through the hub end with the connection; sign in again (the hub routes nothing new to it until the isolation clears) |
| `too_large` | 1009 | a control message over 16 KiB (on `/v1/relay`: over 4 KiB before sign-in) |
| `bad_token`, `bad_keys`, `weak_keys` | 4400 | a malformed enrollment |
| `unknown_enrollment` | — | no such code, or expired or used: try another |
| `enrollment_gone` | 4409 | |
| `too_many_attempts` | 4429 | |
| `chain_unavailable`, `unavailable` | 1011 | |
| `bad_attestation` | — | an `ATTEST` the hub refused |
