# Security model

[droppen.app/security](https://www.droppen.app/security) describes the
encryption in full: the device chain, every key, the channels and relays, the
offline copies and the threat model. This page is the short version, with
pointers into the code.

## Droppen Hub

Droppen Hub at droppen.app (accounts, sign-in, plans, storage of encrypted
data, relay tickets) is a separate service and is not open source. It never
holds a key that can decrypt your sessions, terminals or files: the design
assumes a malicious Hub, and the guarantees that matter are checked by the open
code in this repository. [`docs/hub-api/`](hub-api/README.md) documents the
whole API this code uses, and the code relies on nothing else of the Hub.
droppen.app also serves the web app, built from `web/`; [What holds](#what-holds)
says how to check it.

## What holds

- **Alone, the local program talks to no Droppen service.** It listens on
  `127.0.0.1`, asks for a login (local accounts, per instance), and trusts no
  request for coming from this computer. It runs the CLIs as you, with your
  files, so treat access to its port as access to your account on that
  computer.
- **Every device has its own keys.** A Droppen install, a browser and a phone
  each have their own key pair. A signed device chain, which every device and
  computer checks for itself, decides which devices belong to your account.
- **Content is end-to-end encrypted.** Devices control a computer over a Noise
  channel, through a Droppen relay that only forwards its ciphertext or over a
  direct path; Droppen Hub carries no channel. Session lists and transcripts
  kept for reading while a computer is offline are encrypted streams that the
  computer signs.
- **What the Hub sees** is metadata. Everything that passes through it is
  specified in [docs/hub-api/](hub-api/README.md); summed up:
  - your account: the email address and whether it is verified, a password
    hash or your passkeys' public keys (with which provider made each, from
    its AAGUID, and the name you gave it), the plan (and the invite it came
    with, if any), and the usage it is counted on (bytes stored and
    transferred);
  - a record of the last 30 days' changes to how you sign in: which change,
    from which device, and the passkey's name;
  - the IP address of every request and connection (its rate limits are keyed
    on it);
  - the User-Agent of every request, of which it keeps only a short name for
    each browser or phone — what the device is and which browser, such as
    "iPhone · Safari" — so the devices page can tell them apart; never the
    User-Agent itself;
  - each device's and computer's random id and public keys, when it was
    registered and last seen, when a computer went online or offline, which
    relays it is on and its round trip to each (a rough location), and the
    version and protocol features the local program declares;
  - the device chain: which devices belong to the account, which of them run
    AI, which device added which, and which computer is the account's main
    device;
  - for the encrypted data it stores (session streams and their snapshot
    pages, sealed key mail, keyring blobs): how many, how large, when written,
    which device wrote each, which device each piece of mail is for, and which
    device reads which stream, and when;
  - from the relays' metering reports, how many bytes each account's channels
    carried in each minute. The sizes and timing of a single channel's
    traffic are seen by the relay that carries it.

  It never sees keys, transcripts, terminal input or output, or file names and
  contents.
- **The web app at droppen.app is delivered by the Hub.** It is a reproducible
  build of `web/` with a per-file hash manifest on every release, and
  `node scripts/verify-web.mjs` checks what the site serves. That cannot rule out a targeted one-off delivery; for high
  assurance use the local web app or the desktop app.
- **Releases are attested.** Every release file has a build provenance
  attestation that ties it to this repository's release workflow and its tag:
  [Verify a download](install.md#verify-a-download).

Droppen is in beta: its encryption design has had internal security reviews,
not an external audit. Report a flaw privately, as
[SECURITY.md](../SECURITY.md) describes.

## A2A callers

An A2A entry turns one directory of this computer into an agent an external
A2A client can reach. The request is served on the computer itself; neither
the Hub nor a relay sees it ([a2a.md](a2a.md)).

- **An A2A token, or an A2A proxy an entry allows, is that user's data access
  on this computer.** Sessions run as you: even a read-only tool can read any
  file (this program's device keys included) and put it in the result. Give it
  only to callers you trust.
- An entry starts disabled and names its source, directory, account, model,
  permission mode and isolation (a worktree by default); the message carries
  only the task. Third-party text (an imported issue) is fenced in the
  template, and an entry that allows an A2A proxy cannot use a bypass
  permission mode.
- Tokens are taken only from this computer's loopback. Only
  `sha256(secret)` is stored, each token names the entries it may use, and it
  can be revoked. An A2A proxy is a device of the account that only calls the
  computers whose entries name it; it holds no key that decrypts anything and
  is held to the chain.
- Push notifications go straight to the client's HTTPS origin, which the entry
  must list; loopback, private and link-local addresses are refused unless the
  entry names them.
- The chain's freshness applies: a computer whose view no device confirmed in
  7 days refuses calls (`droppen.attestation_stale`), and revoking a token,
  removing the proxy from an entry, or disabling the entry cancels its
  unfinished tasks at once.
- Every A2A request is logged locally (time, caller, entry, method, task),
  never its content or the token.

## Design documents

In Chinese, apart from the Hub API:

- [a2a.md](a2a.md): the A2A server, its entries, tokens, mapping and errors,
  and the a2a device with `droppen a2a-proxy`.
- [a2a-proxy.md](a2a-proxy.md): deploying an A2A proxy, its variables and its
  error codes.
- [a2a-compat.md](a2a-compat.md): the official A2A TCK run, and what it
  found.
- [local-aggregator.md](local-aggregator.md): the three programs, the
  open-source boundary, signing in from the local settings, the local gateway.
- [cross-machine-architecture.md](cross-machine-architecture.md): the key
  model, the protocol, what the Hub can see, and the threat model (§7).
- [passkeys.md](passkeys.md): signing in to the Hub with a passkey, and the
  optional passkey unlock for new devices.
- [web-integrity.md](web-integrity.md): what the web app check proves and what
  it does not.
- [hub-api/](hub-api/README.md): every Hub route and message this code uses.
