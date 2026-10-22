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
  has approved it: the jobs that sign and publish run in the protected
  `release` environment, whose required reviewer is the owner and which
  deploys only for `v*` tags. Who may create such a tag is the tag ruleset's
  to say (the owner). The job that publishes uses the run's own token, with
  `contents: write` in that job only; there is no other token that can write
  a release.
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
   *Actions → release → Run workflow*, `dry_run` ticked). It builds and checks
   everything a release does, the Developer ID signature and Apple's
   notarisation included, checks the release against this repository's
   releases (`publish-release.sh --dry-run`), and attests and publishes
   nothing; the files it made stay on the run for two days as the artifact
   `dry-run-vX.Y.Z`. A run anywhere but on a `v*` tag is always a dry run.
   Its signing jobs need the environment's approval like a release's, and the
   environment must allow the branch (below).
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
4. **Approve, three times.** The build jobs run (about twenty minutes).
   Each job that holds a secret then waits for the `release` environment:
   *Review deployments → Approve* on the run's page, for `sign-apple` (the
   Developer ID and notarisation), `sign-updater` (the updater key) and
   `publish` (writing the release), a few minutes apart. Each starts only if
   everything before it passed. A server-only release has two: `approve`
   (before anything is attested) and `publish`.
5. It signs and notarises the app (Apple usually takes a few minutes), makes
   the updater's files, checks the release, attests every file and
   publishes: a draft with every file, `latest.json` last, checked, then
   published as the latest release. Installed apps and
   `curl -fsSL https://droppen.app/install.sh | sh` take it at once. The
   `attest` job's summary lists every file with its sha256.

**When it fails.** Before publishing nothing is written: fix the cause and
*Re-run failed jobs* (or, for a fix in the source, a new version). If
publishing failed after the draft was created, the draft stays (invisible):
look at it, delete it (`gh release delete vX.Y.Z --repo getdroppen/droppen`;
the tag stays), and re-run the `publish` job. A published release is never
changed.

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
| `tools` | macOS | nothing | cargo-about and tauri-cli for the build jobs |
| `signer` | macOS | nothing | tauri-cli (the updater's signer) and `droppen-update-manifest` for `sign-updater`, built in a job of their own that builds nothing else |
| `server-macos` | macOS ×2 | nothing | the server for aarch64 and x86_64, as `scripts/release-public.sh` builds it |
| `server-linux` | Linux ×2 | nothing | the server for x86_64 and aarch64 (glibc 2.28): `scripts/build-linux-in-container.sh` in the image `scripts/build-linux.sh` pins |
| `stage` | macOS | nothing | `scripts/release-public.sh --prebuilt`: the binary checks, `THIRD_PARTY_NOTICES`, macOS smoke tests (Apple silicon and Intel, under Rosetta), the universal binary, the tarballs and their checks, `SHA256SUMS`, `install.sh`, the INSTALL texts, `NOTES.md`, `web-manifest.json` (checked against the web build the binaries embed) |
| `smoke-linux` | Linux x86_64, Linux arm64 | nothing | both Linux tarballs, unpacked, on Debian 10 |
| `desktop` | macOS | nothing | `desktop/build.sh --sidecar` with the staged universal server (unsigned) |
| `sign-apple` | macOS | the Developer ID, the notary's password, the names | the text check with the names; `desktop/notarise.sh`: the app and the DMG signed, notarised, stapled; the keychain deleted right after |
| `sign-updater` | macOS | the updater key | `desktop/updater-artifacts.sh`: `Droppen.app.tar.gz`, its signature, `latest.json`, with `signer`'s two programs (checked by sha256); it compiles nothing |
| `verify` | macOS | nothing | `install.sh`, `LICENSE`, the INSTALL texts and `NOTES.md` (the release's page: `scripts/release-notes.sh` writes it again) are the source's, byte for byte; `scripts/publish-release.sh --dry-run`, every check (`latest.json` against its archive and the key included) against this repository's releases, read-only |
| `approve` | Linux | nothing | a server-only release only: waits for the approval before anything is attested (with the app, `sign-apple` and `sign-updater` were approved already) |
| `attest` | macOS | the OIDC token (when publishing) | `SHA256SUMS` (`publish-release.sh --sums-only`, held to verify's) and a build provenance attestation for every file published |
| `publish` | macOS | `contents: write`, the names | the text check with the names; `scripts/publish-release.sh --verify-tag` with the attested `SHA256SUMS`: the release of the tag, made once |

The trust boundaries:

- **The build jobs run dependency code and hold nothing.** `web`, `tools`,
  the server builds, `stage`, `smoke-linux` and `desktop` run npm install
  scripts, Rust build scripts and the programs they build, so they have no
  secret, no write token and no OIDC token. Whatever they make is untrusted
  data from then on.
- **Artifacts are pinned and fenced.** Each artifact is a tar whose sha256
  its job puts out as a job output, which no other job can change. A job
  that takes one checks the download against that output, so an artifact
  replaced while the run waits for an approval is refused, and unpacks it
  into an empty directory of its own, only if every member is a plain file or
  directory on the expected paths (`.github/scripts/unpack-artifact.sh`).
  Two artifacts never share a directory.
- **Nothing from an untrusted artifact runs where a secret is.**
  `sign-apple` only has `codesign`, `notarytool`, `stapler`, `hdiutil` and
  `spctl` read and sign the app; `publish` runs the repository's scripts,
  `gh` and `stapler`; the staged tarballs are read member by member, never
  unpacked by their own paths, and `install.sh`, `LICENSE`, the INSTALL
  texts and `NOTES.md` are compared with the source. The two programs that do run with a
  secret, tauri-cli's signer and `droppen-update-manifest` in
  `sign-updater`, are built by `signer`, a job of their own with no secret
  that builds nothing else, from pinned versions and locked sources
  (crates.io checksums), and checked by sha256; `sign-updater` compiles
  nothing. GitHub gives a job all its secrets when it starts, so whatever
  runs in it could reach them: the code of tauri-cli's dependencies, built
  into the signer, is what meets the updater key. That job holds no Apple,
  write or OIDC credential, and the key alone ships nothing: apps take an
  update only from a release of this repository, which only `publish` makes.
  (Later: a small signer in this repository, with few dependencies, instead
  of tauri-cli.)
- **One secret per job, each approved.** A job gets all the secrets it
  names when it starts, for its whole run, so the secrets are split by job:
  the Developer ID and the notary's password reach only `sign-apple` (its
  keychain is deleted right after the signing), the updater key only
  `sign-updater`, the write token only `publish`. Each of these jobs waits for
  a person's approval.
- **The OIDC token only where nothing built runs, after an approval.**
  `attest` runs the repository's scripts, `shasum`, `gh` and `stapler`, and
  GitHub's attest action. It runs only when publishing (dry runs make no
  attestation), and only after a person approved the release
  (`sign-updater`, or `approve` for a server-only one).
- **What is attested is what was checked, and what is published.** `verify`
  writes the release's `SHA256SUMS`; `attest` makes it again without the
  manifest tool and refuses a different one; `publish` refuses to write
  anything unless its own is the attested one (`--sums-expected`).
- **No caches.** Every dependency is fetched and built afresh, so no earlier
  run can leave anything for a release to pick up.
- **Pinned inputs.** Every action by commit; Rust 1.98.1 (rustup checks its
  downloads); Node as in `.nvmrc`; pnpm as in `package.json`; the Linux image
  by digest, zig by sha256; crates and npm packages by their lockfiles;
  Apple's Developer ID G2 intermediate certificate by sha256.
- **Names stay out of the log.** The release text check's list of personal
  names (`RELEASE_FORBIDDEN_ENV`) reaches only `sign-apple` and `publish`; the
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
   - *Deployment branches and tags*: selected only, the tag pattern `v*`.
     Add the branch `main` too to allow dry runs from main (a run that is not
     on a tag never publishes, and its signing jobs still need the approval).
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
