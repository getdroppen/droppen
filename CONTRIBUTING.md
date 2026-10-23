# Contributing to Droppen

Bug reports, fixes and features are welcome. For anything large, open an issue
first so we can agree on the approach before you write the code.

Security issues are the exception: report them privately, as described in
[SECURITY.md](SECURITY.md), never in a public issue or pull request.

## Sign off every commit (DCO)

Droppen uses the [Developer Certificate of Origin](https://developercertificate.org/)
(DCO) instead of a contributor licence agreement. Every commit must end with a
sign-off line in your name and with an email address that reaches you:

```
Signed-off-by: Ada Lovelace <ada@example.com>
```

`git commit -s` adds it for you. To sign off commits you already made, use
`git commit --amend -s` for the last one, or `git rebase --signoff main` for a
whole branch.

By signing off you certify the DCO 1.1 for that commit. In short: you wrote the
change, or otherwise have the right to submit it under the licence of the part
of the project it changes; and you understand that the contribution, with your
sign-off, is public and kept for good. Read the
[full text](https://developercertificate.org/) before your first commit.

Sign-off is enforced. A required check on every pull request (the DCO GitHub
App, or an equivalent required status check) fails when any of its commits
lacks a `Signed-off-by:` line, and a pull request cannot be merged until it
passes. The maintainers, members of the getdroppen organisation, are exempt
(`.github/dco.yml`). Pull requests are squash-merged, and the squash commit's
message keeps the `Signed-off-by:` lines of the commits it combines, so every
sign-off stays in the history.

## Licence of contributions

Inbound is outbound: you contribute each change under the licence of the part
of the repository it changes.

| Part | Licence |
|---|---|
| The local program (the root crate in `server-rs/`), the desktop app (`desktop/`), the web app (`web/`) and the relay program (`droppen-relay`) | AGPL-3.0-only ([LICENSE](LICENSE)) |
| `rh-proto`, `rh-relay` (the library), `rh-client`, `rh-devkit`, and the plugin SDKs (`packages/plugin-sdk`, `server-rs/crates/droppen-plugin-sdk`) | MIT OR Apache-2.0 (`LICENSE-MIT` and `LICENSE-APACHE` in each) |

A change that spans both is contributed under each part's licence for the
files in that part. Do not move code from an AGPL-3.0-only part into an
`MIT OR Apache-2.0` crate unless you wrote all of it. Do not add code under a
licence that is incompatible with the part it goes into; if you bring in
third-party code, say where it comes from in the pull request and keep its
copyright and licence notice.

## Commits and pull requests

- Write commit messages as [Conventional Commits](https://www.conventionalcommits.org/):
  `type(scope): summary`, for example `fix(web): …`, `feat(server): …`,
  `docs: …`. Types in use: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`,
  `build`, `ci`, `chore`. Mark a breaking change with `!`, as in
  `refactor!: …`.
- Pull requests are squash-merged: the pull request's title becomes the
  commit's subject line, so write the title the same way. The commit's message
  keeps your commits' `Signed-off-by:` lines (above).
- Keep a pull request to one change. Say what it changes and why, and add or
  update tests for the behaviour you changed.
- On a pull request from a fork, CI waits until the automated security review
  has passed for its latest commit. The review then starts CI itself. If the
  review blocks, it says why in a comment and a maintainer decides. A
  maintainer also decides when the pull request changes files under
  `.github/` ([docs/ci-security.md](docs/ci-security.md)).
- Keep the documentation in step: [README.zh-CN.md](README.zh-CN.md) is the full
  manual, and the server's API contract (`openapi/openapi.json`,
  `web/src/api/schema.d.ts`) is regenerated with `pnpm gen:api` after a change
  to the server's routes or types, and committed with it.

## Run the tests before you open a pull request

Setup is in the [README](README.md#build-from-source): Rust (stable), Node 22
and pnpm 10.6.3.

The web app:

```sh
pnpm install --frozen-lockfile
pnpm --filter @remote-hand/web typecheck
pnpm --filter @remote-hand/web test
```

The local program and the crates, in `server-rs/`, as CI runs them, with
[cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`):

```sh
cd server-rs
DROPPEN_ALLOW_MISSING_WEB_DIST=1 cargo nextest run                # the local program
DROPPEN_ALLOW_MISSING_WEB_DIST=1 cargo nextest run -p rh-proto    # and each crate you changed
```

`DROPPEN_ALLOW_MISSING_WEB_DIST=1` lets the server build without a built web app.
nextest runs every test in its own process. The local program's tests change
the process environment, which is not safe to do while other threads read it
(on Linux, running them on several threads of one process can hang), and one
process per test is what makes them safe to run in parallel. Without nextest,
`cargo test -- --test-threads=1` is the same check on one thread, and slower.

`pnpm test` at the repository root is a shortcut for a quick local run: it runs
the web app's tests, then `cargo test` for the local program on several
threads. It does not replace the run above. `pnpm typecheck`
type-checks every package. The plugin SDK has its own tests
(`pnpm test:sdk`), and the desktop app's tests run with
`cargo test --workspace` in `desktop/`.

## Code that talks to the Hub

The Hub at droppen.app is not in this repository. The open code may rely only
on what [`docs/hub-api/`](docs/hub-api/README.md) documents: not on the Hub's
source, its storage, its configuration or any behaviour the documentation does
not describe.

If your change uses a Hub route, field or message, or depends on how the Hub
behaves, the documentation must already say so or your pull request must add
it (`http.md`, `websocket.md`, the JSON schemas and `index.json` in
`docs/hub-api/`). If you need something the Hub does not offer, open an issue
that describes the API you need.

Names follow the same line. A crate or directory named `rh-hub`,
`droppen-hub`, or `rh-hub-*` / `droppen-hub-*` (`rh_hub…` in Rust) belongs to
the Hub's side; nothing here may depend on one, include a file from one, or
build a path to one. Don't give an open crate such a name.
`pnpm check:boundary` checks all of this: dependencies of any kind, paths
outside the repository, and hub crates, paths and environment variables in
the open sources.
