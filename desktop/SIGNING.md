# Signing and notarising Droppen.app

`desktop/build.sh` produces an **unsigned** app: the bundle is sealed with an
ad hoc signature (`codesign -s -`), which keeps it internally consistent but
identifies nobody. Gatekeeper therefore treats it as from an unidentified
developer. To ship it, `desktop/notarise.sh` signs it with a **Developer ID
Application** certificate and has Apple notarise it. Then
`desktop/updater-artifacts.sh` makes the updater's files from the final app
([The updater](#the-updater)), and `scripts/publish-release.sh` publishes the
release, so installed apps update themselves to it.

Releases are made by GitHub Actions: `.github/workflows/release.yml` runs
these same scripts on GitHub's runners, with the secrets below in the
protected `release` environment ([docs/releasing.md](../docs/releasing.md)).
Run by hand on a Mac, as described here, they are the fallback.

## What the release Mac needs

For a release made by hand (GitHub Actions has these as the `release`
environment's secrets: [The release secrets in GitHub
Actions](#the-release-secrets-in-github-actions)). Three things, all on the
Mac that makes releases. `desktop/notarise.sh --check`
checks the first two and says exactly what is missing; `desktop/build.sh`
(with `APPLE_SIGNING_IDENTITY`), `desktop/notarise.sh` and
`desktop/updater-artifacts.sh` refuse to go on without them and point here.

1. **The Developer ID Application certificate, with its private key**, in the
   login keychain. It needs membership of the Apple Developer Program.
   - **From a backup:** the `.p12` file exported from Keychain Access on the
     Mac that made the certificate (*My Certificates* → the certificate, with
     its private key under it → *Export*). Import it with its password:

     ```sh
     security import DeveloperID.p12 -k ~/Library/Keychains/login.keychain-db -T /usr/bin/codesign
     ```

   - **Without one** (the private key is lost): make a new certificate in Xcode
     → Settings → Accounts → *Manage Certificates* → + → *Developer ID
     Application* (the account holder's role is needed), then export it as a
     `.p12` and keep that offline. Apps already signed with the old
     certificate stay valid.
   - **Check:** `security find-identity -v -p codesigning` lists
     `"Developer ID Application: <name> (<team id>)"`. If Keychain Access shows
     the certificate but this command does not, add Apple's *Developer ID -
     G2* intermediate certificate
     (`https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer`). With
     more than one such identity, choose one with `DROPPEN_SIGNING_IDENTITY`.
2. **The `notarytool` keychain profile `droppen-notary`**: the name
   `desktop/notarise.sh` uses (`DROPPEN_NOTARY_PROFILE` overrides it). Make an
   app-specific password at appleid.apple.com (*Sign-In and Security* →
   *App-Specific Passwords*; not the Apple ID password), then save the profile
   once, **into the login keychain**. Without `--keychain` it may land where
   other sessions of the same user (a script started from elsewhere, an agent)
   cannot read it. Without `--password`, notarytool asks for the password, so
   it never goes on a command line or into the shell's history:

   ```sh
   xcrun notarytool store-credentials droppen-notary --apple-id <apple id> --team-id <team id> \
     --keychain ~/Library/Keychains/login.keychain-db
   ```

   **Check:** `desktop/notarise.sh --check`. It runs
   `xcrun notarytool history --keychain-profile droppen-notary` (it needs the
   network), with `--keychain ~/Library/Keychains/login.keychain-db` first,
   then without, and the notarisation uses whichever found the profile;
   `DROPPEN_NOTARY_KEYCHAIN=<file>` names another keychain file. A profile
   belongs to the macOS user who saved it: run the scripts as that user.
3. **The updater key** in `~/.config/droppen/`, with an offline backup:
   [The key](#the-key).
4. **The release text check's lists** in
   `~/.config/droppen/release-forbidden.env`. `desktop/build.sh`,
   `notarise.sh`, `updater-artifacts.sh` and `scripts/release-public.sh` read
   it when `DROPPEN_RELEASE_FORBIDDEN` is not set, as data (the two
   assignments, `export NAME='…'`, nothing else), never by running it. If a
   name is assigned twice, the first assignment counts (sourcing the file
   would take the last). It holds:
   - `DROPPEN_RELEASE_FORBIDDEN`, the personal names no shipped text may carry
     (pinned by its hash in `scripts/release-text-check.sh`);
   - `DROPPEN_RELEASE_NOTICE_ALLOW`, one line per third-party copyright line of
     `THIRD_PARTY_NOTICES` that happens to contain one of them: that whole line
     written out, `^Copyright …$`, with its regular expression metacharacters
     escaped (`\(c\)`). Anything looser (`.*`, a class, an alternation) is
     refused. Without these lines, the notices of the app and of the server
     stop the build there.

### The release secrets in GitHub Actions

The same material is kept as secrets of the **`release` environment** of
`getdroppen/droppen`, under the names Tauri's GitHub action reads. Only the
`release` job of `.github/workflows/release.yml` reads them, each step only
its own, after a person approves it, once per run
([docs/releasing.md](../docs/releasing.md), which also says how to set
them):

| secret | what it holds |
|---|---|
| `APPLE_CERTIFICATE` | the Developer ID Application certificate with its private key: the `.p12`, base64 |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12`'s password |
| `APPLE_SIGNING_IDENTITY` | the identity's name, `Developer ID Application: <name> (<team id>)` |
| `APPLE_TEAM_ID` | the developer team's id |
| `KEYCHAIN_PASSWORD` | a password for the throwaway keychain the workflow imports the certificate into |
| `APPLE_ID` | the Apple ID notarisation submits as |
| `APPLE_PASSWORD` | an app-specific password of that Apple ID (not its own password) |
| `TAURI_SIGNING_PRIVATE_KEY` | the updater key, the contents of `tauri-updater.key` ([The key](#the-key)) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its password |
| `RELEASE_FORBIDDEN_ENV` | the release text check's lists: the contents of `~/.config/droppen/release-forbidden.env` |

An Actions secret can be replaced but never read back: none of them is a
backup of the certificate or the updater key.

- They live in the protected **`release` environment** only (a required
  reviewer, deployments only from release tags, `v*`, and main for dry
  runs), so the one job that uses it reaches them only once a person
  approves it. Keep no copy among the repository's own
  secrets: the repository is public, and these sign what every Mac runs.
- Never use them from `pull_request_target`, or from any job that runs code a
  fork can change (a pull request's checkout, its scripts, its dependencies,
  a cache it can write): the jobs that hold them check out the tag, run
  nothing from another job's artifacts, and use no cache.

## Making a release

This is the release by hand, on the Mac; `.github/workflows/release.yml` does
the same steps ([docs/releasing.md](../docs/releasing.md)).

```sh
VERSION=0.2.125
# 0. Before the tag: the release notes, docs/releases/v$VERSION.md (what changed
#    for people, English and Chinese, and the desktop app's paragraph), committed
#    with the version bump. release-public.sh puts them at the end of NOTES.md.
# 1. The server, for macOS and Linux, staged in /tmp/droppen-public-release.
scripts/release-public.sh v$VERSION
# 2. The app around that same server, unsigned, in /tmp/droppen-desktop-release.
mkdir -p /tmp/droppen-sidecar
tar -xzf /tmp/droppen-public-release/droppen-v$VERSION-macos-universal.tar.gz -C /tmp/droppen-sidecar droppen
desktop/build.sh --ref v$VERSION --sidecar /tmp/droppen-sidecar/droppen
# 3. Signed with the Developer ID, notarised and stapled, in its DMG.
desktop/notarise.sh /tmp/droppen-desktop-release $VERSION
# 4. The updater's files, from the stapled app.
desktop/updater-artifacts.sh /tmp/droppen-desktop-release/Droppen.app $VERSION --out /tmp/droppen-desktop-release/updater
# 5. The server's checksums signed with the release key, which install.sh pins
#    (docs/releasing.md, "The release key"); publish-release.sh refuses without it.
openssl dgst -sha256 -sign ~/.config/droppen/install-signing.pem \
  -out /tmp/droppen-public-release/SHA256SUMS-server.sig /tmp/droppen-public-release/SHA256SUMS
# 6. Published: a draft, latest.json uploaded last, then the latest release.
scripts/publish-release.sh v$VERSION --server /tmp/droppen-public-release --desktop /tmp/droppen-desktop-release --dry-run
scripts/publish-release.sh v$VERSION --server /tmp/droppen-public-release --desktop /tmp/droppen-desktop-release
```

A release without a new desktop app (a server-only fix) is steps 1, 5 and 6,
without `--desktop`: `scripts/publish-release.sh` then carries the latest
release's `latest.json` forward ([Rules for a release](#rules-for-a-release)).

### What `desktop/notarise.sh` does

On `desktop/build.sh`'s output, after checking that it is a universal release
build of `<version>`:

1. signs inside-out, the server sidecar first, then the app, with the
   hardened runtime (`--options runtime`, which notarisation requires) and a
   secure timestamp, by the identity's SHA-1 (`DROPPEN_SIGNING_IDENTITY`
   takes a name or a SHA-1 when there is more than one);
2. submits the app to Apple (`notarytool submit --wait`, as a zip) and
   staples its ticket into it: the updater ships the app without a DMG, and
   the DMG's copy carries the ticket too;
3. makes a fresh `Droppen-<version>.dmg` around the stapled app (with the
   usual Applications link, and the app's `LICENSE` and `THIRD_PARTY_NOTICES`
   beside it, which it refuses to do without), signs it, submits it and
   staples it;
4. on any submission that is not accepted, prints Apple's log and stops;
5. checks what a user's Mac checks: both tickets, and Gatekeeper accepting
   the DMG and the app inside it and beside it.

**No entitlements yet.** The app is not sandboxed, and WKWebView's JIT runs in
WebKit's own processes. But under the hardened runtime macOS holds the app
responsible for what the sessions it runs do, so check on the first notarised
build that a session can still script another app (`osascript -e 'tell
application "Finder" to name'` from a Droppen terminal asks for permission
rather than failing). If it fails, sign with an entitlements file that has
`com.apple.security.automation.apple-events` (and give the app an
`NSAppleEventsUsageDescription`); the microphone and camera would need
`com.apple.security.device.audio-input` / `.camera` likewise.

`desktop/build.sh` can also sign during the build: with
`APPLE_SIGNING_IDENTITY` (and `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`
to notarise the app), Tauri signs inside-out and notarises the app. A release
does not need it: `desktop/notarise.sh` signs the app again either way, and
only it makes and notarises the DMG.

## What signing changes for users

- **No Gatekeeper warning.** An unsigned build has to be opened the long way
  the first time: right-click → Open, or on macOS 15 and later System
  Settings → Privacy & Security → *Open Anyway*.
- **The keychain stops asking.** The app keeps its own local-login password in
  one keychain item it creates (service `app.droppen.desktop`). The item's
  access list names the app by its code signature. An ad hoc build has a
  different signature every build, so after an update macOS asks once whether
  the new build may read the item. A Developer ID build is named by team and
  bundle id, which stay the same across updates, so it is not asked again.
  If anyone answers *Deny*, the app makes a new password, resets the local
  account with it and keeps working.

## The updater

Installed apps update themselves (`desktop/README.md`, "Updates"): they read
`latest.json` from the *latest* release of `getdroppen/droppen`
(`https://github.com/getdroppen/droppen/releases/latest/download/latest.json`,
`plugins.updater.endpoints` in `src-tauri/tauri.conf.json`), download
`Droppen.app.tar.gz` from the release it names, and install it only if its
minisign signature verifies with the public key in `tauri.conf.json`
(`plugins.updater.pubkey`) and was made for the version `latest.json`
announces (`requireSignedVersion`). This key pair is the updater's own,
separate from the Apple certificate.

Up to 0.2.126 the app read `getdroppen/releases` instead, and downloaded only
from that repository's releases. v0.2.127, the first release of
`getdroppen/droppen`, is also published there once by hand, the same signed
files with a `latest.json` pointing there (`scripts/bridge-release.sh`,
[docs/releasing.md](../docs/releasing.md)), so those apps update to it; then
`getdroppen/releases` is archived.

### The key

The updater key is minisign key **`1CB79070988604BF`**, made on 2026-10-20
with `cargo tauri signer generate -w ~/.config/droppen/tauri-updater.key`
(tauri-cli 2.12.3). It replaced `A0A24C0E2DC6185B`, which was lost when the
release Mac was reimaged, before any app that trusted it had shipped.
`core::update::RELEASE_KEY_ID` and a unit test hold `plugins.updater.pubkey`
to it, and `desktop/build.sh` checks that the built app carries it.

- **Where it is.** The same key, in two places:
  - **The release Mac**, in `~/.config/droppen/`:
    - `tauri-updater.key` (0600): the private key, encrypted with its password;
    - `tauri-updater.key.password` (0600): the password, random;
    - `tauri-updater.key.pub`: the public key, the same text as
      `plugins.updater.pubkey`.

    `desktop/updater-artifacts.sh` reads these by default.
  - **GitHub Actions secrets** of `getdroppen/droppen`'s `release`
    environment (with the Apple ones: [The release secrets in GitHub
    Actions](#the-release-secrets-in-github-actions)):
    - `TAURI_SIGNING_PRIVATE_KEY`: the contents of `tauri-updater.key`;
    - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: the contents of the password file.

    These are the variables `tauri signer sign` reads, and
    `droppen-update-signer` (`desktop/update-signer`) the same.
    `desktop/updater-artifacts.sh` signs with them when
    `TAURI_SIGNING_PRIVATE_KEY` is set and no `--key` is given, and puts
    neither on a command line: that is how `release.yml` signs updates.
- **Keep an offline backup. This is the owner's job.** Neither place above is
  a backup: the Mac can be reimaged (that is how the previous key was lost),
  and an Actions secret can be replaced but never read back. Copy both files
  to storage you hold offline, such as an encrypted disk or a password
  manager's export, with the key and its password apart if you can. Check the
  copy once: sign a scratch file named `Droppen.app.tar.gz` with it
  (`TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat <copy>.password)" cargo tauri signer sign -f <copy> --app-version 0.0.1 Droppen.app.tar.gz`)
  and verify that with the app's key:
  `cargo run --manifest-path desktop/Cargo.toml -p droppen-desktop-core --bin droppen-update-manifest -- write --version 0.0.1 --archive Droppen.app.tar.gz --signature Droppen.app.tar.gz.sig --pubkey desktop/src-tauri/tauri.conf.json --out /tmp/check.json`
  must say "signature verified with key 1CB79070988604BF".
- **Never print, commit or upload** the key or its password. The password
  only means the key file alone is no use to whoever finds it.
- **Losing the key** (the Mac's copy and the backup; the Actions secret cannot
  be read back) **means installed apps can no longer update themselves**: a
  release signed with a new key is refused by every app that trusts the old
  one, and each of those users has to download the DMG once by hand.
- **A leaked key** lets whoever has it sign an update every installed app
  accepts, if they can also publish on the release repository. Rotate it
  ([Rotating the key](#rotating-the-key)).
- **Signing happens after notarisation and stapling**, on the final app,
  wherever the key is used.

### Rotating the key

`.github/workflows/release.yml` does not rotate keys: it checks against
`tauri.conf.json`'s key only. The transition release below, and every
server-only release until the first app after it, are made by hand
([Making a release](#making-a-release)).

An app trusts exactly one key, the one built into it, and the updater plugin
takes no second one. So a new key reaches installed apps only through a
**transition release**: an app that carries the new key, in an update signed
with the old key, which the apps installed today accept. The scripts check
against one key, `tauri.conf.json`'s, unless `--verify-pubkey` names the old
one for the release that needs it:

1. Make the new pair beside the old one, with its password in
   `~/.config/droppen/tauri-updater-new.key.password` (0600), and back all
   three files up offline as for the old ([The key](#the-key)):
   `cargo tauri signer generate -w ~/.config/droppen/tauri-updater-new.key`.
2. The transition release, version N, carries the new key: its `.pub` text in
   `plugins.updater.pubkey`, its key id in `core::update::RELEASE_KEY_ID`.
   Build and notarise it as usual, then sign it with the **old** key and check
   it with the old `.pub`:

   ```sh
   desktop/updater-artifacts.sh "$APP" N --out "$OUT/updater" \
     --key ~/.config/droppen/tauri-updater.key --verify-pubkey ~/.config/droppen/tauri-updater.key.pub
   openssl dgst -sha256 -sign ~/.config/droppen/install-signing.pem -out …/SHA256SUMS-server.sig …/SHA256SUMS
   scripts/publish-release.sh vN --server … --desktop "$OUT" --verify-pubkey ~/.config/droppen/tauri-updater.key.pub
   ```

3. From then on the new key signs. Once N is published, move the old key's
   three files aside first, then the new key's three into their place, so
   that the key, its password and its `.pub` always belong together:

   ```sh
   cd ~/.config/droppen
   mv tauri-updater.key              tauri-updater-old.key
   mv tauri-updater.key.password     tauri-updater-old.key.password
   mv tauri-updater.key.pub          tauri-updater-old.key.pub
   mv tauri-updater-new.key          tauri-updater.key
   mv tauri-updater-new.key.password tauri-updater.key.password
   mv tauri-updater-new.key.pub      tauri-updater.key.pub
   ```

   Then replace the two `TAURI_SIGNING_*` secrets of `getdroppen/droppen`'s
   `release` environment. Until the first release after N that has a desktop
   app, a server-only release carries N's `latest.json`, which the old key
   signed: publish those with
   `--verify-pubkey ~/.config/droppen/tauri-updater-old.key.pub`.
4. **Apps that skip the transition release are stranded.** An app that was not
   running while N was the latest release, and next checks when a release
   signed with the new key is, refuses that update and never updates itself
   again: its user has to download a DMG by hand. So keep N the latest release
   as long as you can, and keep the old key (offline) in case N needs a fix.
   After a leak, waiting is a risk of its own: every app still trusts the
   leaked key until it installs N.

### Why after the build, not by the build

Tauri can make updater archives during `cargo tauri build`
(`bundle.createUpdaterArtifacts`), but those would hold the app as it is before
the Developer ID signature, notarisation and stapling, and their signature
would not match what users get from the DMG. So the build does not make them
(and does not need the key); `desktop/updater-artifacts.sh` makes them from the
final, stapled app (step 4 of [Making a release](#making-a-release)):

```sh
desktop/updater-artifacts.sh "$APP" "$VERSION" --out "$OUT/updater" [--notes-url <url>]
```

It refuses an app that is not Developer ID signed, notarised, stapled and
accepted by Gatekeeper, that is not universal (one archive serves Apple
silicon and Intel), whose version is not `$VERSION`, or that is an `--e2e`
test build. It packs the app as `Droppen.app.tar.gz` (entries under
`Droppen.app/` only, owned by root — no builder's name in a published file —
and no extended attributes; the unpacked copy is checked to verify and to keep
its ticket), signs it with `droppen-update-signer sign Droppen.app.tar.gz
--version $VERSION` (`desktop/update-signer`: what `tauri signer sign
--app-version $VERSION` does, with minisign and few dependencies, so no big
third-party tool runs where the key is; its tests check its signatures the
way the Tauri updater does, and `core/src/update.rs`'s tests check one; the
password goes to the signer through its environment only), and writes
`latest.json` through `droppen-update-manifest`
(`core/src/bin/droppen-update-manifest.rs`), which verifies the signature with
`tauri.conf.json`'s key before writing:

```json
{
  "version": "0.2.125",
  "notes": "https://github.com/getdroppen/droppen/releases/tag/v0.2.125",
  "pub_date": "2026-10-19T08:00:00Z",
  "platforms": {
    "darwin-aarch64": { "signature": "<Droppen.app.tar.gz.sig>", "url": "https://github.com/getdroppen/droppen/releases/download/v0.2.125/Droppen.app.tar.gz" },
    "darwin-x86_64":  { "signature": "<the same>",                "url": "<the same: one universal app>" }
  }
}
```

`notes` is the address the app links *Release Notes* to (only pages under
`github.com/getdroppen/droppen/releases/` or droppen.app are opened; anything
else falls back to the release page). The manifest itself is not signed, so the app trusts nothing in
it but what the signature binds: the archive and its version.

### Rules for a release

- The app's version must be the release's: `desktop/build.sh --ref vX.Y.Z`
  builds the version in `server-rs/Cargo.toml` at that tag and refuses a tag
  that disagrees; the updater compares it with `latest.json`'s `version`.
- **Every release carries a `latest.json`**: the app always reads the
  *latest* release's. Without one, apps' own checks fail quietly and *Check
  for Updates…* says the update service is unavailable.
  `scripts/publish-release.sh` makes sure of it:
  - **With the desktop app** (`--desktop`) it uploads the `latest.json`
    `desktop/updater-artifacts.sh` wrote for this version.
  - **Without it** (a server-only fix) it carries the latest release's
    `latest.json` forward, byte for byte: its URL names the archive on the
    release of the version it announces, which stays where it is. It takes it
    only from a published release that is not a prerelease (GitHub's
    "latest" skips drafts and prereleases, so nothing else is what apps
    read), only if it announces an older version than this release, and only
    after verifying it against that archive with `tauri.conf.json`'s key. If
    the latest release has none, it stops and names the newest release that
    has one (`--manifest-from <tag>`, refused while the latest release has
    its own). `--no-manifest` publishes without one, and is refused once any
    published release has one: it is for the releases before the first app
    with the updater.
  - It also carries that release's `Droppen-<version>.dmg` forward, which must
    be the version the carried `latest.json` announces (and stapled): the
    download page's button goes to the latest release for the DMG.
  - **A prerelease** (`v1.2.3-rc.1`) never becomes the latest release: it is
    published as a GitHub prerelease, with no `latest.json`.
- **"Latest" only moves forward:** `scripts/publish-release.sh` refuses a
  version that is not newer than the latest release. It goes on without a
  latest release only when GitHub says there is none (404); any other failure
  to ask stops it.
- **The desktop app is published from a Mac:** `--desktop` checks the DMG's
  notarisation ticket with `xcrun stapler`, and refuses to run anywhere else.
- **`latest.json` is uploaded last, and the release becomes the latest only
  after it.** `scripts/publish-release.sh` creates the release as a draft
  (invisible, and never GitHub's "latest"), uploads the server's files, the
  desktop app's (or the carried DMG), then `latest.json` on its own; checks
  that the draft holds exactly those files; then publishes it and marks it
  latest in one step, and checks that
  `.../releases/latest/download/latest.json` is the file it uploaded. Apps
  start downloading as soon as it is.
- **Check first:** `--dry-run` reads and checks everything (it downloads and
  verifies a carried `latest.json` and its archive) and prints the writes.
  A `latest.json` can also be checked by hand:
  `cargo run --manifest-path desktop/Cargo.toml -p droppen-desktop-core --bin droppen-update-manifest -- check latest.json --pubkey desktop/src-tauri/tauri.conf.json --archive Droppen.app.tar.gz`.
- A version that was installed by hand before the updater existed (0.2.124
  and older) never checks: it needs one download of a DMG that has the
  updater; from then on it updates itself.
- Server updates come with the app: the bundled server never self-upgrades
  (see `desktop/README.md`).

### Rollback

**There is none: a bad release is fixed forward.** Once an installed app has
swapped in an update, it deletes the previous bundle, and nothing switches back
if the new version will not start. And a published release of
getdroppen/droppen is immutable: none of its files, `latest.json` included,
can be replaced or deleted. What a bad release calls for:

1. **Publish a higher version at once**, the usual way
   ([docs/releasing.md](../docs/releasing.md)): the fix, or, while the fix
   takes longer, the last good source again (revert the bad change on main,
   bump the version, say so in the notes). Installed apps update to it at
   their next check, those on the bad version too while it still starts; an
   app that has downloaded the bad update but not installed it finds the
   newer one at its next check and takes that instead (one that quits before
   then installs the bad one first).
2. Users whose app no longer starts download the fixed DMG by hand.
3. Leave the bad release as it is: until the fixed one is published it is the
   latest release, and the latest release must have its `latest.json`
   ([Rules for a release](#rules-for-a-release)).

Keeping the previous bundle and switching back automatically is not built.
