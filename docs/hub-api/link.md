# `/link`: signing a computer in from its local settings

`https://<hub>/link?v=1` is a page of the web app the hub serves (the same
open `web/` build as everywhere; `web/src/features/link/`), on the app's
origin, `hubWeb`: the hub's app host where it has one
(`https://my.droppen.app/link?v=1`; [below](#which-host-the-popup-and-the-hand-off-open)),
else the hub's own. A computer's
local settings open it in a popup to add that computer to the account
(`docs/local-aggregator.md` §3.3–3.4). The contract has two sides: the
**opener** — the local page, on the computer's own origin (`http://127.0.0.1:8700`,
`http://192.168.1.5:8700`, …) — and the **popup** on the hub's web origin.
The hub itself takes no part beyond serving the page and the existing routes
the popup calls (`POST /v1/enrollments`, `GET /v1/enrollments/{token}`,
`POST /v1/chain`, the keyring and the mailbox — see `http.md`).

## What crosses, and what never does

- Passwords, Turnstile, recovery words, pairing codes: typed into the popup
  only, on the hub's origin. The opener and the local server never see them,
  nor any login or device session.
- The opener gets back one **RH1 enrollment code** (one machine, 10 minutes,
  useless without the machine's proof) by `postMessage`, and hands it to its
  own server (`POST /api/cloud/link/finish`). It never goes in a URL, and the
  hub only ever sees its token (the HMAC), as with `agent enroll`.
- The popup URL carries no data (`?v=1` only).

## Messages

All are plain objects with `v: 1` and a type `t`. Hex is lower-case.

| `t` | from → to | fields | `targetOrigin` |
|---|---|---|---|
| `rh-link/ready` | popup → opener | — | `"*"` (it carries nothing); repeated each second until a `request` arrives |
| `rh-link/request` | opener → popup | `nonce` (16 bytes), `machine {hostname, signKey, dhKey, enrolled: {accountFp, id} \| null}`, `aggregator {signKey, dhKey} \| null`, `aggregate` | the hub's web origin, fixed |
| `rh-link/code` | popup → opener | `nonce`, `code` (`RH1-…`; the opener reads `DR1-…` too), `email?` | `cb` |
| `rh-link/done` | popup → opener | `nonce`, `account`, `email`, `machine`, `aggregator: null` (until aggregators are admitted) | `cb` |
| `rh-link/cancel` | either way | `nonce` | the other side's origin |
| `rh-link/error` | either way | `nonce`, `code`, `message` | the other side's origin |

## Rules

- The opener answers only messages whose `event.source` is the window it
  opened **and** whose `event.origin` is the app's origin, and posts only
  to that origin. An opener from before discovery (v0.2.103–v0.2.128) knows
  only the hub's own origin: once the app has moved off it, the popup there
  gets the move notice (`web/src/features/hosts/legacy.tsx`), which offers to
  move a device that browser keeps there first, then says to close the window
  and pick "continue in a browser tab instead" (the hand-off, v0.2.125 on),
  or else to update Droppen or add the device with a one-time code (the table
  below).
- The popup takes the `request` only from `window.opener`, and remembers the
  origin the browser reports for it as **`cb`**; every later message goes to
  `cb` only, and only messages from the opener at `cb` with the request's
  `nonce` are taken. Unknown types and malformed messages are ignored.
- `cb` comes from `event.origin` only — never from the URL. The popup shows it
  on the consent page, with the machine's host name (for reference) and its
  **fingerprint**: the first 16 bytes of
  `SHA-256("rh/v1 machine fingerprint\0" ‖ signKey ‖ dhKey)` as 8 groups of 4
  hex digits, which the local settings show too.
- `cb` tiers: only this computer itself passes without a warning —
  `localhost`, `*.localhost`, 127.0.0.0/8, `[::1]`. Every other origin, the
  local network and Tailscale included (a private address, `fd00::/8`,
  `*.local` and 100.64/10 are whoever answers on the browser's network;
  `*.ts.net` is public through Funnel), gets a red warning and an extra
  "this is my own computer" checkbox, next to the fingerprint to compare. The
  hub's own host on any scheme or port, or anything that is not an http(s)
  origin, is refused — and so, where the hub names its hosts
  (`/.well-known/droppen-hub`), are its origin, app and site hosts and any name
  under its origin's (`*.droppen.app`): none of them is a computer's own page. Known and deferred: another program on this computer's
  loopback is "local" too, and the opener can send a second `request` while
  the consent page is open.
- `machine.hostname` is for reference only: the popup drops control and
  bidirectional-override characters and `<>"'`, and shows at most 64
  characters.
- Before adding the machine the popup checks the enrollment proof **and** that
  its keys are the request's `signKey`/`dhKey`: a code that leaked to another
  machine adds nothing.
- `machine.enrolled` set: the computer is already a machine; the popup checks
  that its account key fingerprint (`accountFp`, 16 bytes) is this account's
  and that the chain lists `id` with exactly these keys, and refuses otherwise
  ("already in another account").
- `aggregator`: a second, limited device key pair from before chain v3
  (`docs/local-aggregator.md` §4). Gone: since chain v3 the computer controls
  the account's other devices with its one identity (S1-4), so the opener
  sends `aggregator: null` and the popup sends `aggregator: null` in `done`;
  an older opener's keys are read and ignored.
- The consent page shows 「可管理账号」 as a switch, off by default; for a
  computer that runs AI — every local install — it stays off, with the reason
  beside it (review D1), and the popup adds it as `[AddDevice3{host, admin:
  false}, KeyEpochs]` (v2: `[AddMachine, KeyEpochs]`). After handing the
  computer its own content key, the popup waits up to 20 seconds for its
  device role to sign in at the hub (`POST /v1/session`) and seals it the
  content keys of the hosts whose accept policy admits it (mailbox purpose 5);
  whatever is still missing the computer asks for itself (purpose 4), and an
  online admin answers with host keys only. No other chain entry is written.
- Neither page sets `Cross-Origin-Opener-Policy: same-origin` (it would cut
  `window.opener`); the hub's tests hold it to that.

## Which host the popup and the hand-off open

The opener and the hand-off use the hub's **app origin**, `hubWeb`, which the
local server works out (`server-rs/src/api/hub_web.rs`) and gives in
`GET /api/cloud` and `POST /api/cloud/link`:

- It asks the hub for `GET /.well-known/droppen-hub` (`http.md`) and takes
  its `app` (`https://my.droppen.app` for droppen.app) when it is a plain
  `https` origin (`http` only for a loopback test hub) whose host is the hub's
  own or a subdomain of it, and the document's `origin`, when it has one, is
  the hub the computer dialled.
- Anything else — no document (a hub that serves one host), a redirect, an
  answer it does not take, the hub out of reach — leaves the hub's own origin
  as `https://` (`https://droppen.app`), as before.
- The computer itself keeps signing in to `wss://<hub>` (`/v1/agent`): the
  document moves only where a browser goes.
- An answer is kept for ten minutes (a failure for 30 seconds). The status
  waits at most a second for a hub's first answer; a sign-in asks again when
  its answer is stale, at most 3 seconds.

`/link?v=1` opens at `<hubWeb>`, and the opener takes messages from that
origin only; the hand-off address is `<hubWeb>/link#droppen-link=…`. The
hand-off's answer goes back to the page's own origin
(`<cb>/link/return#droppen-return=…`) and is sealed to the attempt, not to the
host the consent page was on (`rh_proto::link_return`): it works from any of
the hub's hosts.

### Installed versions and an app on another host

What each installed version of the local server does once the app is on
`my.droppen.app` and `droppen.app` no longer serves it to page loads:

| version | sign-in from the local settings | after the move |
|---|---|---|
| up to v0.2.102 | none (a one-time code, `DROPPEN_CODE`) | unaffected: it only uses `/v1/*` and the sockets on `droppen.app`, which never redirect |
| v0.2.103–v0.2.124 | popup at `https://droppen.app/link?v=1`; the opener takes messages from `https://droppen.app` only; no hand-off | the popup cannot finish on `my.droppen.app`: the opener drops its messages. `droppen.app/link?v=1` says so (update Droppen, or add the device with a one-time code from my.droppen.app) |
| v0.2.125–v0.2.128 | the same popup, plus the browser hand-off `https://droppen.app/link#droppen-link=…` ("continue in the browser"; the desktop app's only way) | the popup as above; the hand-off works: `droppen.app` sends it on to `my.droppen.app` with the fragment, and the answer comes back to the local page, bound to nothing about the host |
| from the version with discovery | popup and hand-off at the hub's `app` | both on `my.droppen.app` directly |

The popup sign-in arrived in v0.2.103 and the hand-off in v0.2.125; the
desktop app's window has no `window.opener`, so its sign-in works from
v0.2.125 on, through the hand-off only. Every version keeps its connection to
the hub: agents, relays and the desktop app's server dial `wss://droppen.app`,
unredirected.
