# Releasing Droppen

From v0.2.127 on, every release is built by
[`.github/workflows/release.yml`](../.github/workflows/release.yml) on
GitHub's runners, from this public repository, and published as this
repository's own [GitHub release](https://github.com/getdroppen/droppen/releases)
of the tag, with a build provenance attestation for every file. Nothing is
built or signed on anyone's own computer. Releases up to v0.2.126 were built
on the release Mac, published at
[getdroppen/releases](https://github.com/getdroppen/releases), and have no
attestations ([The move from getdroppen/releases](#the-move-from-getdroppenreleases)).

## Who can publish, what is attested

- **Who can publish.** Only a run of `release.yml` for a `v*` tag of a commit
  on main (the workflow checks the commit is on main), and only once a person
  has approved it: the one job that holds the secrets and signs, `release`,
  runs in the protected `release` environment, whose required reviewer is
  the owner and which deploys only for `v*` tags and main (dry runs); GitHub
  enforces the approval, once per run. Who may create such a tag is the tag
  ruleset's to say (the owner). The job that publishes, `publish`, runs only
  after `release` succeeded, only for a `v*` tag, and uses the run's own
  token with `contents: write`, in that job only; there is no other token
  that can write a release.
- **What is published.** One release per tag, made once: a draft with every
  file, checked, then published and marked latest. Releases are immutable:
  nothing is added, replaced or deleted afterwards, so a bad release is
  fixed forward with a new version (`desktop/SIGNING.md`, "Rollback").
- **What is attested.** Every file the release publishes: the server
  tarballs, `SHA256SUMS`, `install.sh`, `INSTALL-macos.txt`,
  `INSTALL-linux.txt`, `LICENSE`, `THIRD_PARTY_NOTICES`, `web-manifest.json`,
  and with the desktop app `Droppen-X.Y.Z.dmg`, `Droppen.app.tar.gz`, its
  `.sig` and `latest.json`. An attestation is a statement signed through
  Sigstore and recorded by GitHub: this repository's release workflow, run
  for `refs/tags/vX.Y.Z` at commit C, made a file with exactly this sha256.
  The bundles are on the release too (`droppen-vX.Y.Z.sigstore.json` for
  every file, `web-manifest.sigstore.json` for the web manifest alone).
  Dry runs attest nothing.
- **How to check** ([Verifying a download](#verifying-a-download)):
  `gh attestation verify <file> --repo getdroppen/droppen --signer-workflow getdroppen/droppen/.github/workflows/release.yml --source-ref refs/tags/vX.Y.Z`.
  On top of that the desktop app is signed with the Developer ID and
  notarised by Apple, and installed apps take only an update whose minisign
  signature the updater key made for that version.

## Cutting a release

1. **The version bump**, in one pull request: `version` in
   `server-rs/Cargo.toml` (and `Cargo.lock`), and the release notes,
   `docs/releases/vX.Y.Z.md`: what changed for people, in English and Chinese.
   Name `Droppen-X.Y.Z.dmg` in them (the paragraph on the desktop app, as in
   earlier notes) when the release has the desktop app: the workflow builds,
   signs and publishes the app exactly when the notes announce it. Without it,
   the release is server-only and carries the latest `latest.json` and DMG
   forward (`scripts/publish-release.sh`). Merge it.

   **No CI skip marker in its title** (`[skip ci]`, `[ci skip]`, `[no ci]`,
   `[skip actions]`, `[actions skip]`, or a `skip-checks: true` trailer), nor
   in its commits' messages or the squash commit's message when you merge:
   the title becomes the message of the commit the tag points at, and GitHub
   then runs no workflow for the tag's push, so the release never starts.
   (v0.2.126's "chore: release v0.2.126 [skip ci]" had to be started by
   hand.) The `pr-guard` check fails a pull request whose title or commits
   have one. Nothing needs skipping: the release workflow does not run on
   pushes to main.
2. **Optionally, a dry run**: `gh workflow run release.yml --ref main` (or
   *Actions → release → Run workflow*, `dry_run` ticked). It does everything
   a release does but attest and publish: after the same one approval, the
   Developer ID signature, Apple's notarisation, the updater's signature, and
   the checks against this repository's releases (`publish-release.sh
   --dry-run`); the files it made stay on the run for two days as the
   artifact `dry-run-vX.Y.Z`. A run on any other branch builds and checks
   but never reaches the `release` job (the environment allows `v*` tags and
   main only).
3. **Tag** the merged commit and push the tag:

   ```sh
   git tag -a vX.Y.Z <commit> -m "Droppen vX.Y.Z"
   git push origin vX.Y.Z
   ```

   The tag must be the version in `server-rs/Cargo.toml` at that commit,
   and the commit must be on main: the workflow refuses to publish anything
   else. A run of *release* for the tag shows up under *Actions* within a
   minute. If none does (a skip marker in the commit's message, or GitHub
   dropping the event), start it on the tag yourself; on a tag, with
   `dry_run` unticked, it is the release:

   ```sh
   gh workflow run release.yml --ref vX.Y.Z -f dry_run=false
   ```
4. **Approve, once.** The build jobs run (about twenty minutes). Once they
   and the smoke tests have passed, the `release` job waits for the `release`
   environment: *Review deployments → Approve* on the run's page. Nothing
   asks again.
5. It signs and notarises the app (Apple usually takes a few minutes), makes
   the updater's files, checks the release, attests every file and
   publishes: a draft with every file, `latest.json` last, checked, then
   published as the latest release. Installed apps and
   `curl -fsSL https://droppen.app/install.sh | sh` take it at once. The
   `publish` job's summary lists every file with its sha256.

**When it fails.** Before publishing nothing is written: fix the cause and
*Re-run failed jobs* (or, for a fix in the source, a new version). If
publishing failed after the draft was created, the draft stays (invisible):
look at it, delete it (`gh release delete vX.Y.Z --repo getdroppen/droppen`;
the tag stays), and re-run the `release` job (it asks for the approval
again, and signs again). A published release is never changed.

**Not in the workflow**: a key rotation's transition release, and the
server-only releases after it until the next app
(`desktop/SIGNING.md`, "Rotating the key"), are made by hand with the scripts
the workflow runs (`desktop/SIGNING.md`, "Making a release"), which remain
the fallback whenever GitHub Actions cannot be used.

## What the workflow does

| job | runner | holds | what |
|---|---|---|---|
| `meta` | Linux | nothing | the version (the tag, or the source's for a dry run), whether it publishes (a `v*` tag of a commit on main), whether it has the app |
| `web` | Linux | nothing | the public web build, once, for every binary to embed |
| `tools` | Linux | nothing | cargo-about and tauri-cli for the build jobs: upstream's release files, each checked against `.github/release-tools.lock` (nothing built) |
| `signer` | macOS | nothing | `droppen-update-signer` (`desktop/update-signer`, with its format tests) and `droppen-update-manifest`, built from this repository's locked sources, for the `release` job |
| `server-macos` | macOS ×2 | nothing | the server for aarch64 and x86_64, as `scripts/release-public.sh` builds it |
| `server-linux` | Linux ×2 | nothing | the server for x86_64 and aarch64 (glibc 2.28): `scripts/build-linux-in-container.sh` in the image `scripts/build-linux.sh` pins |
| `notices` | macOS | nothing | `THIRD_PARTY_NOTICES` of the server's files (`scripts/third-party-notices.sh`) and of the app (`desktop/third-party-notices.sh`), beside the builds: cargo-about reads the locked dependency graph, not the binaries |
| `stage` | macOS | nothing | `scripts/release-public.sh --prebuilt --web-dist --notices` with `web`'s build (not built again: CI's `web-reproducible` checks that the build is reproducible) and `notices`' file: macOS smoke tests (Apple silicon and Intel, under Rosetta), the universal binary, the tarballs and their checks (the binary checks on the programs in them, the tarballs in parallel), `SHA256SUMS`, `install.sh`, the INSTALL texts, `NOTES.md`, `web-manifest.json` (checked against the web build the binaries embed) |
| `smoke-linux` | Linux x86_64, Linux arm64 | nothing | both Linux tarballs, unpacked, on Debian 10 |
| `desktop-arch` | macOS ×2 | nothing | `desktop/build.sh --arch aarch64` and `--arch x86_64`, each with that architecture's server from `server-macos` and the app's notices from `notices`, beside `stage` (unsigned, no DMG) |
| `desktop` | macOS | nothing | `desktop/build.sh --from-apps`: the two apps' main programs fused with `lipo`, the staged universal server put in, sealed ad hoc and checked as a build (unsigned; nothing compiled) |
| `release` | macOS | every secret (no write token, no OIDC token); approved once per run | the text check with the names; `install.sh`, `LICENSE`, the INSTALL texts and `NOTES.md` compared with the source; with the app, `desktop/notarise.sh` (Developer ID, notarised, stapled, the DMG) and the keychain deleted, then `desktop/updater-artifacts.sh --no-manifest` with `droppen-update-signer` (the archive and its signature) |
| `verify` | macOS | nothing | `latest.json` by `droppen-update-manifest write` (which verifies the signature with `tauri.conf.json`'s key); `scripts/publish-release.sh --dry-run`, every check (`latest.json` against its archive and the key included) against this repository's releases, read-only; its `SHA256SUMS` |
| `publish` | macOS | `contents: write`, the OIDC token; only when publishing a `v*` tag | the attestations of every file and `scripts/publish-release.sh --verify-tag --skip-manifest-check --sums-expected`: the release of the tag, made once |

The trust boundaries:

- **The build jobs run dependency code and hold nothing.** `web`, `tools`,
  `notices`, the server builds, `stage`, `smoke-linux`, `desktop-arch` and
  `desktop` run npm install scripts, Rust build scripts and the programs
  they build, so they have no secret, no write token and no OIDC token.
  Whatever they make is untrusted data from then on.
- **Artifacts are pinned and fenced.** Each artifact is a tar whose sha256
  its job puts out as a job output, which no other job can change. A job
  that takes one checks the download against that output, so an artifact
  replaced while the run waits for an approval is refused, and unpacks it
  into an empty directory of its own, only if every member is a plain file or
  directory on the expected paths (`.github/scripts/unpack-artifact.sh`).
  Two artifacts never share a directory.
- **One job holds the secrets, approved once per run.** `release` is the
  only job in the `release` environment, so GitHub itself makes it wait for
  the owner's approval, every run, whatever the workflow says. A job gets
  all the secrets it names when it starts, for its whole run, so what runs
  in it is kept to what can be read in a few minutes:
  - this repository's scripts (`desktop/notarise.sh`,
    `desktop/updater-artifacts.sh`, the text checks), read from the
    checked-out commit;
  - Apple's tools (`codesign`, `notarytool`, `stapler`, `hdiutil`, `spctl`);
  - one program of ours, `droppen-update-signer` (`desktop/update-signer`:
    minisign, its scrypt and Ed25519 code, base64; about 25 crates), built
    by `signer` (a job without secrets) from this repository's locked
    sources and checked by sha256.

  No third-party build tool (tauri-cli, cargo, npm) runs there, nor the
  manifest tool (its 80 crates run in `verify`, which holds nothing), and
  nothing from an artifact is run: the app, the DMG and the tarballs are only
  read, signed and handed on; the staged tarballs are read member by member,
  never unpacked by their own paths. The job has no write token and no OIDC
  token (`contents: read`), so a dry run never holds one. Each step gets only
  the secrets it uses in its own `env:`: the Developer ID and the notary's
  password the keychain and notarisation steps (the keychain is deleted right
  after, whatever happened), the updater key the updater step, the names the
  text checks.
- **The updater's signer is ours, and checked against the Tauri updater.**
  `droppen-update-signer` writes what `tauri signer sign --app-version`
  (tauri-cli 2.12.3) writes: a minisign signature (minisign 0.9, as
  tauri-cli), the same untrusted comment, and the trusted comment
  `timestamp:…`, `file:Droppen.app.tar.gz`, `version:<version>` (tab
  separated), from the key in Tauri's format (base64 of the encrypted
  minisign secret key, and its password). Its tests make keys the way
  `tauri signer generate` does, read a key tauri-cli made, and verify every
  signature the way tauri-plugin-updater 2.13.2 does (minisign-verify, then
  the signed version against the announced one, `requireSignedVersion`);
  `desktop/core`'s `update.rs` tests check one of its signatures with the
  app's own checks, version and file name included. `signer` runs those
  tests on every run. Before the first release of the app it signs, run the
  updater's end-to-end test once (`desktop/e2e-updater-macos.sh`, on the
  Mac): the real app then takes an update signed by it
  (`updater-artifacts.sh --test-build` signs with it).
- **Write and OIDC tokens only in `publish`.** `publish` has no environment
  and asks for no second approval: it holds no secret, only the tokens
  GitHub makes for this run (`contents: write` on this repository, and the
  OIDC token that signs the attestations), and runs only when `meta` says
  the run publishes (a `v*` tag of a commit on main), after `release` (the
  approved job) and `verify` succeeded. What protects those tokens is that
  only the owner can push a `v*` tag (the tag ruleset), and that every
  attestation names the workflow and the ref it ran for: a release attested
  for anything but `refs/tags/vX.Y.Z` by `release.yml` fails the check
  users run ([Verifying a download](#verifying-a-download)). It runs this
  repository's scripts, `gh`, Apple's `stapler` and GitHub's attest action,
  on files handed over by sha256 (`publish-release.sh --skip-manifest-check`:
  `verify` checked `latest.json`).
- **What is attested is what was checked, and what is published.** `verify`
  writes the release's `SHA256SUMS`; `publish` attests it with every file,
  and refuses to write anything unless its own is the same
  (`--sums-expected`).
- **No caches.** Every dependency is fetched and built afresh, so no earlier
  run can leave anything for a release to pick up.
- **Two prebuilt tools, pinned.** cargo-about (the notices) and tauri-cli
  (the app's build) are not built here: they are upstream's release files
  (EmbarkStudios/cargo-about, tauri-apps/tauri) for the versions the source
  pins, each checked against the sha256 in `.github/release-tools.lock`; any
  difference fails the run, and nothing falls back to building. The trust is
  upstream's release plus our pinned hash (neither publishes an attestation
  for them; cargo-about publishes a `.sha256`, which
  `scripts/update-release-tool-pins.sh` checks, with the sha256 GitHub
  records, when a person makes new pins for a version bump). The workflow
  never fetches a hash, and the tools run in build jobs only.
- **Pinned inputs.** Every action by commit; Rust 1.98.1 (rustup checks its
  downloads); Node as in `.nvmrc`; pnpm as in `package.json`; the Linux image
  by digest, zig by sha256; crates and npm packages by their lockfiles;
  Apple's Developer ID G2 intermediate certificate by sha256.
- **Names stay out of the log.** The release text check's list of personal
  names (`RELEASE_FORBIDDEN_ENV`) reaches only the `release` job's text checks; the
  build jobs check for "Kilerd" only (`DROPPEN_RELEASE_NAMES_DEFERRED`). A hit
  is reported by its line number only (`DROPPEN_RELEASE_TEXT_QUIET`), and the
  names are masked in the log.

## Verifying a download

Every file of a release from v0.2.127 on has an attestation. With the
[GitHub CLI](https://cli.github.com) (logged in), check that this
repository's release workflow built the file for that very tag:

```sh
gh attestation verify droppen-vX.Y.Z-macos-aarch64.tar.gz --repo getdroppen/droppen \
  --signer-workflow getdroppen/droppen/.github/workflows/release.yml \
  --source-ref refs/tags/vX.Y.Z
```

Add `--bundle droppen-vX.Y.Z.sigstore.json` (a file of the same release) to
check against the bundle rather than GitHub's record of it. `SHA256SUMS` is
attested too, so checking it once and then the files against it
(`shasum -a 256 -c --ignore-missing SHA256SUMS`) is the same. The desktop app
is also signed with the Developer ID and notarised by Apple
(`spctl --assess -vv /Applications/Droppen.app`), and installed apps check each
update's minisign signature (`desktop/SIGNING.md`, "The updater"). The web
app's build can be rebuilt byte for byte from the source
([web-integrity.md](web-integrity.md)); the binaries are not reproducible yet,
so for them the attestation is what ties a file to its source.

`https://droppen.app/install.sh` redirects to
`https://github.com/getdroppen/droppen/releases/latest/download/install.sh`:
the latest release's installer, an attested file that never changes once
published, not whatever main holds at the moment.

## Once: the release environment

What the owner sets up once (*Settings* of getdroppen/droppen), before the
first tag:

1. **The environment `release`** (*Settings → Environments*; the first run of
   the workflow creates it bare, with no rules):
   - *Required reviewers*: the owner. *Prevent self-review* off (the owner
     approves the owner's tags), *Allow administrators to bypass* off.
   - *Deployment branches and tags*: selected only, the tag pattern `v*` and
     the branch `main` (dry runs from main: they sign after the same
     approval, and never publish).
2. **Its secrets** (*Add environment secret*, or
   `gh secret set <NAME> --env release --repo getdroppen/droppen`), from the
   release Mac's files, since secrets cannot be read back:

   | secret | from |
   |---|---|
   | `APPLE_CERTIFICATE` | `base64 -i ~/.config/droppen/developer-id.p12` |
   | `APPLE_CERTIFICATE_PASSWORD` | the `.p12`'s password |
   | `APPLE_SIGNING_IDENTITY` | `Developer ID Application: <name> (<team id>)` |
   | `APPLE_TEAM_ID` | the team id |
   | `KEYCHAIN_PASSWORD` | any random string (the run's throwaway keychain) |
   | `APPLE_ID` | the Apple ID that notarises |
   | `APPLE_PASSWORD` | an app-specific password of it (appleid.apple.com) |
   | `TAURI_SIGNING_PRIVATE_KEY` | `< ~/.config/droppen/tauri-updater.key` |
   | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `< ~/.config/droppen/tauri-updater.key.password` |
   | `RELEASE_FORBIDDEN_ENV` | `< ~/.config/droppen/release-forbidden.env` (the whole file; its names must hash to `RELEASE_TEXT_NAMES_SHA256` in `scripts/release-text-check.sh`) |

   Then **delete the repository's own copies** of the Apple and Tauri
   secrets (*Settings → Secrets and variables → Actions → Repository
   secrets*): any workflow on any branch can read a repository secret.
3. **A tag ruleset** that lets only the owner create, move or delete `v*`
   tags, and **immutable releases** (*Settings → General → Releases*): a
   published release's files cannot change. `publish-release.sh` uploads
   everything to the draft first, so it works with them.
4. **The `pr-guard` check** (`.github/workflows/pr-guard.yml`,
   `no-skip-markers`) among main's required checks: it fails a pull request
   whose title or commit messages carry a CI skip marker.

Without `RELEASE_FORBIDDEN_ENV` a dry run checks the text for "Kilerd" only
and says so; a release stops.

## The move from getdroppen/releases

Up to v0.2.126 releases were published at getdroppen/releases, and the
desktop app up to 0.2.126 reads `latest.json` there and downloads updates
only from that repository's releases. So v0.2.127, the first release here,
is also published there once, by hand, by the operator (CI has no token for
that repository):

```sh
scripts/bridge-release.sh v0.2.127 --dry-run
scripts/bridge-release.sh v0.2.127
```

It downloads this repository's release, checks every file against
`SHA256SUMS` and its attestation, rewrites only `latest.json`'s addresses
(the archive's and the notes') to getdroppen/releases, and publishes the same
files there as the latest release (with `scripts/publish-release.sh`).
Installed apps update to v0.2.127 from it, and from then on read this
repository. Then getdroppen/releases is archived, not deleted: its releases
stay where older links point. The hub's `/install.sh` redirect moves to this
repository's `install.sh` once v0.2.127 is published.
