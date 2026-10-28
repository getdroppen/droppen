# Droppen

English | [简体中文](README.zh-CN.md)

Droppen lets you browse and drive your Claude Code and Codex sessions from any
device: a browser, a phone or another computer. It can also start and drive
sessions on any agent you configure that speaks the
[Agent Client Protocol](https://agentclientprotocol.com) (ACP) over stdio.
Settings → Agents & accounts → ACP holds one card per agent: add a known one
from the built-in presets ([`server-rs/src/acp-agents.json`](server-rs/src/acp-agents.json))
or give any agent's command line, and check that it starts. Every session
records the agent it was started on and is always resumed with that agent.

It runs on the computer where the CLIs run. It reads their sessions where they
are, starts and resumes them, and shows them in a web app: the transcript, a
composer whose messages queue while the agent is busy, the agent's questions to
answer, terminals, files and split-screen layouts. It also keeps several Claude
and Codex accounts with their quotas, runs scheduled prompts and sends
notifications.

Droppen is local-first. On one computer it needs no Droppen account and talks to
no Droppen service: the local program serves the web app on `127.0.0.1:8700`.
To use it from your other devices, sign the computer in to Droppen
(droppen.app). Your devices then reach it over channels that are end-to-end
encrypted; the Hub and the relays in between only see ciphertext.

A full manual, in Chinese, is in [README.zh-CN.md](README.zh-CN.md).

## What is in this repository

| Part | Where | What it is |
|---|---|---|
| Local program | `server-rs/` (root crate `remote-hand-server`; the command is `droppen`) | One binary with the web app built in. It runs the CLIs and serves the local API and web app. Once signed in, it is also the agent that connects this computer to your other devices, and the local gateway through which the local web app sees your other computers without holding keys itself. |
| Web app | `web/` | Vite + React: the local UI, and the cloud mode droppen.app serves (`web/src/features/cloud`, `web/src/lib/hub`, `web/src/lib/e2e`). |
| Desktop app | `desktop/` | Droppen for macOS (Tauri). It carries the local program and runs it for you. Its own Cargo workspace; see [desktop/README.md](desktop/README.md). |
| Relay | `server-rs/crates/droppen-relay` (the program), `server-rs/crates/rh-relay` (the library) | `droppen-relay` pairs a device's channel with a computer's on tickets the Hub signs and copies the ciphertext between them. It stores nothing and holds no keys. |
| Protocol | `server-rs/crates/rh-proto` | Hub frames, the Noise end-to-end channel, the device chain, encrypted log records and interoperability vectors. |
| Client libraries | `server-rs/crates/rh-client`, `server-rs/crates/rh-devkit` | The device side of the Hub API in Rust; a headless device for tests. |
| Plugin SDKs | `packages/plugin-sdk` (`@droppen/plugin-sdk`), `server-rs/crates/droppen-plugin-sdk` | For writing plugins. Droppen ships none: a plugin is installed into an instance at runtime and served by that instance's server; see [docs/plugins/runtime.md](docs/plugins/runtime.md). |
| Hub API | [`docs/hub-api/`](docs/hub-api/README.md) | The contract between this code and the Hub. |

### The Hub

The Hub at droppen.app (accounts, sign-in, plans, storage of encrypted data,
relay tickets) is a separate service and is not open source. It never holds a
key that can decrypt your sessions, terminals or files: the design assumes a
malicious Hub, and the guarantees that matter are checked by the open code in
this repository. [`docs/hub-api/`](docs/hub-api/README.md) documents the whole
API this code uses, and the code relies on nothing else of the Hub.
droppen.app also serves the web app, built from `web/`; see
[Security model](#security-model) for how to check it.

## Install

Releases are published as this repository's
[releases](https://github.com/getdroppen/droppen/releases): the desktop app,
the server for macOS and Linux, the installer, `SHA256SUMS`, `LICENSE` and
`THIRD_PARTY_NOTICES`. They are built here by GitHub Actions, and each file
has a build provenance attestation:
`gh attestation verify <file> --repo getdroppen/droppen --signer-workflow getdroppen/droppen/.github/workflows/release.yml --source-ref refs/tags/<version>`
checks that this repository's release workflow built it for that tag
([docs/releasing.md](docs/releasing.md)). Releases up to v0.2.126 are at
[getdroppen/releases](https://github.com/getdroppen/releases).

**Desktop app (macOS 13 or later).** Download `Droppen-<version>.dmg` from the
[latest release](https://github.com/getdroppen/droppen/releases/latest) and
install it. The app starts its own copy of the local program, logs in to it for
you and keeps itself up to date.

**Server (macOS and Linux).** For a computer you use in the background, or one
with no screen:

```sh
curl -fsSL https://droppen.app/install.sh | sh
```

It downloads the latest release, checks it against the release's `SHA256SUMS`,
creates the first local account, installs the program to
`~/.local/share/remote-hand/bin/` (the command is `droppen`) and runs it on
`127.0.0.1:8700` (or the next free port) as a launchd user agent
(`app.droppen.server`) on macOS, or a systemd user service (`droppen.service`)
on Linux (x86_64 and aarch64, glibc 2.28 or later). Run it again to upgrade;
`curl -fsSL https://droppen.app/install.sh | sh -s -- --uninstall` removes it
and keeps your data. `droppen.app/install.sh` serves the `install.sh` of the
release Droppen Hub was built from, byte for byte that release's file
(`https://github.com/getdroppen/droppen/releases/download/<tag>/install.sh`;
its sha256 is `install_sh` in `https://droppen.app/.well-known/droppen-build`),
never a redirect; the source is [`scripts/install.sh`](scripts/install.sh),
which lists every option in its header.

To join the computer to your Droppen account in the same step, pass the
one-time code droppen.app shows under
*my devices → add a device that runs AI → show a one-time code*:

```sh
curl -fsSL https://droppen.app/install.sh | DROPPEN_CODE=<code> sh
```

The local program listens on loopback only. Reach it from other devices
through Droppen, or through an SSH tunnel or Tailscale. Do not expose its port
to the Internet: whoever reaches it can run commands as you. It answers to IP
addresses, `localhost` and local-network names (`mymac`, `*.local`, `*.lan`,
`*.home.arpa`, `*.internal`, a tailnet's `*.ts.net`); a reverse proxy or
tunnel that passes another name on (Caddy, a Cloudflare Tunnel) needs that
name in `DROPPEN_ALLOWED_HOSTS` (comma-separated). Local-network names are
whatever the network's own name resolution says, so a device on the same
network that controls it can still reach the routes that answer without a
session (sign-in, first-run setup, status); everything else needs a session.

**Optional: VS Code in the web app.** The rail's `+` → `vscode` opens VS Code
in the browser through [code-server](https://github.com/coder/code-server),
which Droppen does not install. To use it, install code-server on the computer
the server runs on (see
[code-server's install guide](https://coder.com/docs/code-server/install)),
somewhere on the server's `PATH`. The service the installer sets up (the
launchd agent or the systemd user service) and the desktop app's server have
`~/.local/bin` on their `PATH`, which is where code-server's standalone install
(`--method=standalone`) puts it; `DROPPEN_SERVICE_PATH` sets another `PATH`
when you run the installer. The server looks for `code-server` each time an
editor is opened, so it needs no restart. Without it, opening VS Code says that
code-server is missing and links to the guide.

## Build from source

You need Rust (stable), Node 22 (see `.nvmrc`) and pnpm 10.6.3 (the version in
`package.json`; `corepack enable` picks it up).

```sh
pnpm install
pnpm build:web                                               # first: the web app is embedded in the binary
cargo build --release --manifest-path server-rs/Cargo.toml   # the same as pnpm build:server
```

The program is `server-rs/target/release/remote-hand-server`.
`remote-hand-server serve` runs it in the foreground on
`http://127.0.0.1:8700`; with no account yet, `remote-hand-server open` opens
the page as this computer's owner, and `remote-hand-server user add <name>`
creates an account. Without `web/dist` the build stops on purpose, rather than
produce a binary that shows only a placeholder page; set
`DROPPEN_ALLOW_MISSING_WEB_DIST=1` when you do not need the web app (the `pnpm`
scripts below do).

The desktop app (macOS only) is built with `desktop/build.sh`; see
[desktop/README.md](desktop/README.md).

## Running a relay

`droppen-relay` is one static process: no database and no files (unless
`DROPPEN_RELAY_STATE_DIR` names a volume to keep its key in). Build its image
from the repository root, make it a key, and run it behind a TLS-terminating
proxy:

```sh
docker build -f server-rs/crates/rh-relay/Dockerfile -t droppen-relay .
docker run --rm droppen-relay droppen-relay keygen   # prints DROPPEN_RELAY_KEY=… (a secret)
docker run -p 8080:8080 \
  -e DROPPEN_RELAY_HUB=https://<the Hub's origin> \
  -e DROPPEN_RELAY_ORIGIN=wss://<this relay's public origin> \
  -e DROPPEN_RELAY_KEY=<the key> \
  -e DROPPEN_RELAY_REGISTRATION_TOKEN=<the one-time token the Hub issued for this relay> \
  -e DROPPEN_RELAY_TRUST_PROXY=railway \
  droppen-relay
```

The pre-rename names, `RH_RELAY_*`, are deprecated and stop working in
v0.2.135: set the `DROPPEN_RELAY_*` name with the same value instead.

The Hub accepts the registration token once, on the first start. Keep it set
afterwards all the same, since it also names the relay, or set
`DROPPEN_RELAY_ID` instead. The relay's per-address limits need each client's
own address: behind a proxy, set `DROPPEN_RELAY_TRUST_PROXY=railway` and make
the proxy overwrite `X-Real-IP` with the client's address (never pass on a
value the client sent), or `fly` on Fly.io (`Fly-Client-IP`). Left unset, the
relay takes the socket's address, which behind a proxy is the proxy's for
every client.

Every variable is described in
[`server-rs/crates/rh-relay/src/config.rs`](server-rs/crates/rh-relay/src/config.rs),
and the protocol between a relay and its Hub in
[docs/relay-and-plans.md](docs/relay-and-plans.md) (in Chinese). A relay pairs
channels only on tickets its Hub signs, so it serves the accounts of the Hub
that registered it. droppen.app lists only the relays it runs: it does not take
relays run by others yet.

## Development

```sh
pnpm install
pnpm dev:server   # cargo run -- serve on http://127.0.0.1:8700; needs no web build
pnpm dev:web      # Vite on http://localhost:5180, proxying /api and /embed to the server
```

Tests and checks:

```sh
pnpm test         # the web app's tests, then cargo test for server-rs
pnpm typecheck
pnpm gen:api      # after changing the server's routes or types:
                  # regenerates openapi/openapi.json and web/src/api/schema.d.ts
```

`pnpm test` runs the server's tests on several threads of one process, which is
fine for a quick run on macOS. CI runs them with cargo-nextest, one process per
test (`cargo nextest run` in `server-rs/`), and that is what a pull
request has to pass; [CONTRIBUTING.md](CONTRIBUTING.md#run-the-tests-before-you-open-a-pull-request)
has the exact commands. The desktop app's tests run with
`cargo test --workspace` in `desktop/`.

## Security model

- **Alone, the local program talks to no Droppen service.** It listens on
  `127.0.0.1`, asks for a login (local accounts, per instance), and trusts no
  request for coming from this computer. It runs the CLIs as you, with your
  files, so treat access to its port as access to your account on that
  computer.
- **Every device has its own keys.** A Droppen install, a browser and a phone
  each have their own key pair. A signed device chain, which every device and
  computer checks for itself, decides which devices belong to your account.
- **Content is end-to-end encrypted.** Devices control a computer over a Noise
  channel that the Hub and the relays only forward. Session lists and
  transcripts kept for reading while a computer is offline are encrypted
  streams that the computer signs.
- **What the Hub sees** is metadata. Everything that passes through it is
  specified in [docs/hub-api/](docs/hub-api/README.md); summed up:
  - your account: the email address and whether it is verified, a password
    hash or your passkeys' public keys, the plan (and the invite it came with,
    if any), and the usage it is counted on (bytes stored and transferred);
  - the IP address of every request and connection (its rate limits are keyed
    on it);
  - the User-Agent of every request, of which it keeps only a short name for
    each browser or phone — what the device is and which browser, such as
    "iPhone · Safari" — so the devices page can tell them apart; never the
    User-Agent itself;
  - each device's and computer's random id and public keys, when it was
    registered and last seen, when a computer went online or offline and on
    which relays, and the version and protocol features the local program
    declares;
  - the device chain: which devices belong to the account, which of them run
    AI, which device added which, and which computer is the account's main
    device;
  - for the encrypted data it stores (session streams and their snapshot
    pages, sealed key mail, keyring blobs): how many, how large, when written,
    which device wrote each, and which device each piece of mail is for;
  - which device opens a channel to which computer, through which relay, and
    the sizes and timing of that traffic.

  It never sees keys, transcripts, terminal input or output, or file names and
  contents.
- **The web app at droppen.app is delivered by the Hub.** It is a reproducible
  build of `web/` with a per-file hash manifest on every release, and
  `node scripts/verify-web.mjs` checks what the site serves. That cannot rule
  out a targeted one-off delivery; for high assurance use the local web app or
  the desktop app.

The design documents (in Chinese, apart from the Hub API):

- [docs/local-aggregator.md](docs/local-aggregator.md): the three programs, the
  open-source boundary, signing in from the local settings, the local gateway.
- [docs/cross-machine-architecture.md](docs/cross-machine-architecture.md): the
  key model, the protocol, what the Hub can see, and the threat model (§7).
- [docs/passkeys.md](docs/passkeys.md): signing in to the Hub with a passkey,
  and the optional passkey unlock for new devices.
- [docs/web-integrity.md](docs/web-integrity.md): what the web app check proves
  and what it does not.
- [docs/hub-api/](docs/hub-api/README.md): every Hub route and message this
  code uses.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Every commit needs a `Signed-off-by:`
line (the Developer Certificate of Origin; `git commit -s`), and a required
check on every pull request enforces it.

Please report security issues privately, as described in
[SECURITY.md](SECURITY.md), and not in public issues.

## Licence

Copyright (C) 2026 Kilerd.

- **The applications** are licensed under the GNU Affero General Public License
  v3.0 only ([LICENSE](LICENSE), `AGPL-3.0-only`): the local program (the root
  crate in `server-rs/`), the desktop app (`desktop/`), the web app (`web/`) and
  the relay program (`droppen-relay`).
- **The protocol and library crates**, `rh-proto`, `rh-relay` (the library),
  `rh-client` and `rh-devkit`, and the plugin SDKs (`packages/plugin-sdk`,
  `server-rs/crates/droppen-plugin-sdk`) are licensed under `MIT OR
  Apache-2.0`, at your option; each has `LICENSE-MIT` and `LICENSE-APACHE` in
  its directory. These are the only parts of this repository the closed Hub
  links.

Releases carry `LICENSE` and `THIRD_PARTY_NOTICES`, which lists the licences of
every dependency compiled in. Every build of the web app serves the licences of
the npm packages in it at `/THIRD_PARTY_NOTICES.txt` (on droppen.app too),
linked from the footer of its public pages.
