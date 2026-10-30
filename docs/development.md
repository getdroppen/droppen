# Developing Droppen

## What is in this repository

| Part | Where | What it is |
|---|---|---|
| Local program | `server-rs/` (root crate `remote-hand-server`; the command is `droppen`) | One binary with the web app built in. It runs the CLIs and serves the local API and web app. Once signed in, it is also the agent that connects this computer to your other devices, and the local gateway through which the local web app sees your other computers without holding keys itself. |
| Web app | `web/` | Vite + React: the local UI, and the cloud mode droppen.app serves (`web/src/features/cloud`, `web/src/lib/hub`, `web/src/lib/e2e`). |
| Desktop app | `desktop/` | Droppen for macOS (Tauri). It carries the local program and runs it for you. Its own Cargo workspace; see [desktop/README.md](../desktop/README.md). |
| Relay | `server-rs/crates/droppen-relay` (the program), `server-rs/crates/rh-relay` (the library) | `droppen-relay` pairs a device's channel with a computer's on tickets the Hub signs and copies the ciphertext between them. It stores nothing and holds no keys. See [Running a relay](relay.md). |
| Protocol | `server-rs/crates/rh-proto` | Hub frames, the Noise end-to-end channel, the device chain, encrypted log records and interoperability vectors. |
| Client libraries | `server-rs/crates/rh-client`, `server-rs/crates/rh-devkit` | The device side of the Hub API in Rust; a headless device for tests. |
| Plugin SDKs | `packages/plugin-sdk` (`@droppen/plugin-sdk`), `server-rs/crates/droppen-plugin-sdk` | For writing plugins. Droppen ships none: a plugin is installed into an instance at runtime and served by that instance's server; see [plugins/runtime.md](plugins/runtime.md). |
| Hub API | [`docs/hub-api/`](hub-api/README.md) | The contract between this code and the Hub. |

Droppen Hub, the service at droppen.app, is not in this repository and is not
open source; [Security model](security-model.md#droppen-hub) says what it is
trusted with.

## What the local program does

It runs on the computer where the CLIs run. It reads their sessions where they
are, starts and resumes them, and shows them in a web app: the transcript, a
composer whose messages queue while the agent is busy, the agent's questions to
answer, terminals, files and split-screen layouts. It also keeps several Claude
and Codex accounts with their quotas, runs scheduled prompts and sends
notifications.

Besides Claude Code and Codex, it starts and drives any agent that speaks the
[Agent Client Protocol](https://agentclientprotocol.com) (ACP) over stdio.
Settings → Agents & accounts → ACP holds one card per agent: add a known one
from the built-in presets
([`server-rs/src/provider/acp/acp-agents.json`](../server-rs/src/provider/acp/acp-agents.json)) or give
any agent's command line, and check that it starts. Every session records the
agent it was started on and is always resumed with that agent.

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
[desktop/README.md](../desktop/README.md).

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
request has to pass; [CONTRIBUTING.md](../CONTRIBUTING.md#run-the-tests-before-you-open-a-pull-request)
has the exact commands. The desktop app's tests run with
`cargo test --workspace` in `desktop/`.

The full manual, in Chinese, is [manual.zh-CN.md](manual.zh-CN.md).
