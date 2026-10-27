# Droppen for macOS

Install it, open it, and Droppen runs on this device: the app starts the
Droppen server it carries, logs in for you and shows the web app in its window.
This device is then both a device that runs AI and the place you use it from.
There is no setup and no login page.

This is the **local mode** of `docs/local-aggregator.md` §7.3, and nothing
more yet, apart from signing in to Droppen through the system browser: no
signing (see [What is not here yet](#what-is-not-here-yet)). It keeps itself
up to date, quietly ([Updates](#updates)).

It needs **macOS 13 (Ventura) or later** (`minimumSystemVersion` in
`src-tauri/tauri.conf.json`): Claude Code's official installer, the one the
app's first-run guide offers, does not run on anything older.

## What it does

- **On launch** it starts `remote-hand-server serve` (bundled as a Tauri
  sidecar, `Contents/MacOS/remote-hand-server`) on `127.0.0.1` and a free port,
  as instance **`droppen-app`**, waits for it, logs in, and loads
  `http://127.0.0.1:<port>/` in the window.
- **Closing the window** leaves Droppen running in the menu bar. The menu-bar
  item shows the unread count and has *Open Droppen*, *Open in Browser*, the
  update item and *Release Notes*, *Start at Login* and *Quit Droppen*. The
  Dock icon brings the window back too.
- **Open in Browser** asks the server, with the window's own login, for a
  one-time token (`POST /api/auth/local-open/grant`; single use, 60 s) and
  opens the default browser at the page with it, signed in as this device's
  owner, while the instance has no user account — at the login page once it
  has one. The token goes to the browser on `osascript`'s standard input
  (`core/src/browser.rs`): never on a command line, never in a log. If that
  fails, the app opens the plain `http://127.0.0.1:<port>/` and logs why.
  The server never lets a request in just for coming from this Mac, and on
  the app's instance — which never leaves its setup state, its own account
  being app-internal — it keeps no setup code and its control socket opens
  nothing (any process of this macOS user could ask either): without a user
  account a browser here needs the app's window or this item.
- **Quit** (menu bar, ⌘Q, or a `SIGTERM`/`SIGINT` to the app) logs the app out
  and stops the server.
- **A second launch** by the same macOS user focuses the running one
  (`core/src/single.rs`: an `flock` and a Unix socket in the user's own 0700
  support directory, so two macOS users each run their own app).
- **Start at Login** (`src-tauri/src/login_item.rs`, decisions in
  `core/src/login_item.rs`). A release build — signed with a Developer ID
  Application certificate — registers the app with `SMAppService.mainApp`:
  System Settings → General → Login Items lists it as Droppen, and it follows
  the app when it is moved or renamed. macOS opens it with no arguments; the
  launch's Apple event ("launched as login item", logged at every start as
  `launch Apple event: …`) is what makes it start in the menu bar without a
  window, and a reopen macOS sends for the login item while the app already
  runs does not show it either. The menu shows what macOS says: checked when
  the item is enabled or registered; a line that opens Login Items when it
  needs the user's approval there (a turn-on that needs it opens Login Items
  at once), or right after a turn-on macOS did not take. `notFound` — also
  what macOS says once the user removed the item with − — is shown as off. The
  status is read again (at most every 2 s) when the pointer reaches the
  menu-bar item.
  **On by default**: a new install's
  first launch turns *Start at Login* on, once, and records that in
  `state.json` (`loginDefault`); no later launch turns it on again, so what
  the user chooses afterwards stands. An install that ran before this (its
  `state.json` has no record) or one that already has a login item is left
  as it is, and a first launch from a temporary App Translocation copy waits
  for the launch from where the app is moved. An end-to-end build never
  applies it.
  Earlier versions wrote a launch agent naming the app's path,
  `~/Library/LaunchAgents/app.droppen.desktop.plist`. If it is there (it is
  only while *Start at Login* is on), the first launch of a release build
  registers the app and then removes the file — unless the user had switched
  the old item off in Login Items (`SMAppService.statusForLegacyURL` says
  `requiresApproval`): then it only removes the file, and *Start at Login*
  stays off. If macOS refuses to register, the file stays (the app still opens
  at login) and the next launch tries again. Turning *Start at Login* off
  removes such a file too. A launch with `--autostart` while the app already
  runs exits without showing the window.
  **Going back** to a version before this one after the move: macOS keeps
  opening the app at login (now the older one, with its window), and the
  older version cannot turn that off — it only knows the launch agent. Remove
  Droppen in System Settings → Login Items, or turn *Start at Login* off
  before going back.
  Every other build — development, ad hoc (`build.sh` without an identity),
  Apple Development–signed, end-to-end — keeps the launch agent with
  `--autostart`, as before: `SMAppService.mainApp` is keyed by the app's
  identity, so a local build would take over the release's Login Item, and an
  end-to-end build has no `SMAppService` calls compiled in at all.
- **Run from a quarantined download** (App Translocation): a notice says to
  move Droppen to Applications, and *Start at Login* stays off until it is. A
  launch agent made before the app was moved is pointed at where it is now
  (unsigned builds); a signed build moves an old one over only once it runs
  from where it was moved.

## Process model

```
Droppen (Tauri, the app)                     ~/Library/Logs/Droppen/desktop.log
 └─ remote-hand-server serve                 ~/Library/Logs/Droppen/server.log
      APP_INSTANCE=droppen-app  APP_SERVER__HOST=127.0.0.1  APP_SERVER__PORT=<port>  DROPPEN_DESKTOP_APP=1  RH_DESKTOP_APP=1
      PATH=<login shell's PATH>:~/.local/bin:~/.npm-global/bin:…:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
      ├─ claude / codex sessions, pty and session shims (the server's own)
```

- **Its own instance, provably.** Data under
  `~/.local/share/remote-hand/instances/droppen-app/` (the binary's normal
  directories). The app creates that directory itself, with a marker file
  (`droppen-app.json`) in it, before anything else writes there; a directory of
  that name *without* the marker belongs to someone else, and the app refuses
  to run any `user` command against it and says so. Before touching it the app
  also checks with the kernel that no other server holds the instance's lock.
- **Ports.** The one the app used last time if it is free, whatever range it
  came from — asked again for up to 5 s if it is taken at start; else a free
  one from 18700–18759, below the kernel's ephemeral range (49152–65535), which
  every outgoing connection on the Mac draws from, searched from a place fixed
  per install (from the uid, and from the support folder when it is not the
  usual one under the user's home). Never 8700–8899 (a command-line install
  and the apps its server runs) or 5180 (the web app's development server). A
  port is taken if 127.0.0.1 cannot be bound or something answers a connect to
  127.0.0.1 or [::1] there (a listener on `0.0.0.0`, `[::]` or `::1`); the
  probes never bind a wildcard address, which macOS refuses whenever another
  user's connection has the port. It is kept across launches because it is
  the web app's origin, and with it the web app's login and local storage —
  for cloud use, the window's device keys — so a new one is written to
  `state.json` only once a window has loaded the web app on it: a start that
  fails in between goes back to the old port. If a restart finds it taken, the
  server moves and the window follows (`core/src/port.rs`).
- **The app supervises `serve` itself** instead of running `up`. `up` prefers
  `<data>/bin/remote-hand-server` — the self-upgrade store every instance on
  the device shares — over its own binary, so under `up` the app would run some
  other install's server, or seed that store with its own. The app restarts a
  server that dies after a back-off (1 s doubling to 30 s, reset after a
  healthy minute), stops it with `SIGTERM` then `SIGKILL` after 5 s, by pid
  only, and writes its pid to `server.pid` so a launch after an app crash can
  stop a server the crashed app left behind — only if that pid still runs a
  `remote-hand-server` that either sits in an app bundle with this bundle id
  (wherever the app was moved) or holds the instance's lock. A server that
  exits with 75 (instance held elsewhere) is reported as such.
- **PATH.** An app opened from the Finder gets launchd's bare PATH. The server
  gets whatever `$SHELL -ilc` says `PATH` is, read once per start with a 5 s
  limit — which is what finds nvm/volta/asdf installs of the CLIs — and then
  the usual tool directories as a fallback (`~/.local/bin`, Homebrew, the
  system's), each directory once; the folders adopted in Settings come last.
  The shell first, so the server runs the `node` and `claude` the user's
  terminal runs. A command-line install's launchd agent gets the same order
  (`scripts/install.sh` `service_path`).
- **The environment** is the app's own with every `APP_*`, `GOTCHA_*`, `RH_*`,
  `REMOTE_HAND_*`, `DROPPEN_*`, `CLAUDE_CODE_*`, `PORT`, `HOST` and
  `CLAUDECODE` removed, so an app started from a shell inside another Droppen
  session does not inherit that session's instance or port. Then
  `DROPPEN_DESKTOP_APP=1` (`server-rs/src/desktop_app.rs`), and `RH_DESKTOP_APP=1`,
  its name before the rename to Droppen, which a server that old reads instead
  (`server-rs/src/env.rs`): the server refuses
  `POST /api/server/self-upgrade` with `403 self_upgrade_disabled` — it would
  replace the shared `<data>/bin`, not the app — and tells the web app it runs
  under the app (`GET /api/auth/status` → `desktopApp: true`), which then hides
  the password change and the sign-out, and signs in to Droppen through the
  system browser instead of a popup.
  The server keeps `DROPPEN_DESKTOP_APP` (and `RH_DESKTOP_APP`) to itself: the
  terminals and CLIs it starts do not get it.
- **Shared with other installs on the same Mac.** The app's server is an
  ordinary instance, so it shares the machine-wide files every instance shares:
  `~/.config/remote-hand/` (`state.db` — session titles, the seen and archived
  lists, UI preferences — plus the AI accounts in `accounts.json` and the
  phone-notification settings in `notify.json`) and `~/.local/share/remote-hand/`
  outside `instances/` (uploads, `bin/`). Its sessions, terminals, logins and
  local accounts are its own.
- **What survives a quit** is what survives any server restart: the server
  starts terminals and resident sessions under shims that outlive it, so that a
  restart does not kill them. A terminal or a running session left open at quit
  keeps running and is picked up again at the next launch.

## Updates

The app updates itself without asking anything and without a window
(`src-tauri/src/updater.rs`, the decisions in `core/src/update.rs`):

- **Checks** 15 s after launch and every six hours of wall-clock time, from
  `latest.json` on the newest release of `getdroppen/droppen`
  (`plugins.updater` in `tauri.conf.json`). A failed check is tried again in
  half an hour, silently.
- **Downloads** a newer release in the background — not on a metered network
  (hotspot, cellular, Low Data Mode) — and verifies its minisign signature with
  the key in `tauri.conf.json` (key id `1CB79070988604BF`; `SIGNING.md`, "The
  key"), bound to the version the manifest announces.
  One that does not verify is thrown away. One that does waits in
  `~/Library/Application Support/app.droppen.desktop/update/` (0700/0600).
- **Says it is ready** passively: the update item becomes *Restart to Update
  to 0.2.125* / 「重启以更新到 0.2.125」, the menu-bar icon gets a dot, and the
  web app's rail has an **Updates** entry (`web/src/app/desktop-update.tsx`).
  The entry is always there in the app and only opens the update page
  (`/updates`, `web/src/features/updates/updates-view.tsx`) — it never
  restarts anything itself. The page says which version is in hand, what
  changed in it, and offers the one *Restart to update*; *Later* puts the rail
  mark away for that version, and the update still installs at the next quit.
- **Reads the release notes** for the page: every successful check also asks
  GitHub's API for the body of the `v<version>` release
  (`update::notes_api_url`, `notes_from_json`) and keeps the two “what's new”
  sections, English and Chinese, in the status the page reads. The read is
  best effort and never delays a download; without it the page links to the
  release on GitHub. In a browser (no app bridge) the page just says updates
  are handled by the app.
- **Installs at the next quit**: the ordinary quit (the server stopped with
  `SIGTERM`, so terminals and running sessions carry on under their shims),
  then the archive is verified again and swapped in for the bundle in one step;
  the next launch is the new version, which starts its own server. Never while
  the app runs. A failed install leaves the bundle as it was. If the swap
  would need an admin password — the bundle or its folder not writable, the
  bundle locked, or in a sticky folder and not this user's — a quit leaves
  the update for *Restart to Update*, which may ask for one. One case the app
  cannot see beforehand, macOS refusing the swap on its own (a privacy rule),
  still ends in the password prompt at the quit: the updater plugin answers
  any refused swap with it.
- ***Restart to Update*** (the menu item, or the web app's entry) is that quit
  followed by starting the app again, its window closed if it was.
- ***Check for Updates…*** answers in the menu item itself: *Checking…*,
  *Downloading Update… 42%*, *Up to Date (0.2.124)*, or one honest line — only
  a network failure says "network"; a bad signature, a location that cannot be
  updated, a failed install each say so. The update page's *Check now* asks for
  the same check through the bridge (`droppen-desktop://update/check`, with
  the launch's token, like the restart).
- **Not updatable where it runs** — App Translocation, a disk image, a path
  through a symbolic link — the item says *Move Droppen to Applications to
  Update*. A development build (not in a bundle) never updates.

The bundled server is updated with the app and only with it: its own
`self-upgrade` refuses under the app. The web app hears about the update
through a small bridge in `init.js` (part 4): the app hands it the status with
this launch's nonce, and the entry's *restart* is a `droppen-desktop://`
navigation the window cancels — a stand-in for the app↔server control channel
(TODO(control channel) in `updater.rs`). Releasing an update:
`SIGNING.md`, "Making a release" and "The updater".

**There is no rollback: a bad release is fixed forward.** Once an update is
swapped in, the previous bundle is gone, and nothing switches back if the new
version will not start. The way out is a fixed, higher version; meanwhile the
last good release's `latest.json` can stop the bad one reaching more apps
(`SIGNING.md`, "Rollback").

## Auth model

The app adds nothing to the server's auth; it is a client that knows a
password it made up.

1. **The app's account is `droppen-app`**, app-internal — never a person's
   login. First launch: a random 43-character password, and the server's own
   `remote-hand-server user add droppen-app --app --instance droppen-app`,
   password on stdin, **before the server first starts**. `--app` makes it
   the desktop app's own: it signs the window in, `user list` leaves it out,
   and it does not end the setup state, so the macOS login name stays free
   for the person's own user account (`user add <name> --instance
   droppen-app`, or the setup page). Check with `remote-hand-server user list
   --app --instance droppen-app`.
   **Earlier installs** named the app's account after the macOS login name
   (app-internal, or a user account in the first review builds). The next
   start makes `droppen-app` with a new password and keeps that login in
   place of the old one; once it has signed in, it removes the old account —
   the one the old stored login names, with `user rm <old> --yes`, and no
   other (with nothing stored — a keychain that would not give it up — and no
   `droppen-app` yet, the account of the login name). The pending removal is
   recorded in `state.json` first, so a start cut short in between still
   finishes it at the next sign-in; and before anything else changes, an old
   login that is a user account is made app-internal (`user mark-app`), so
   that whatever happens after, no browser is left facing a login nobody knows
   the password of. Once `droppen-app` exists, a user account of the old name
   is the person's and is never touched — whether the app found the name by
   the login name or read it from a stale keychain item (one the last save
   could not replace; a save that falls back to the file removes it) — and
   the removal itself refuses a user account.
2. **Only the app's account.** Accounts are per-instance; a new instance does
   not inherit the machine-wide set an older build kept
   (`server-rs/src/auth/users.rs`). One cleanup remains for instances an older
   build first started: if that start was cut short before the inherited
   accounts were removed, the marker still says so and every start retries the
   removal (`user rm --yes`, which the server asks for before removing the last
   user account). `droppen-app`, app-internal already or nothing removed,
   stays; accounts someone adds on purpose are left alone.
3. **The password** is the app's own secret: one generic-password item in the
   login keychain, service `app.droppen.desktop`, account
   `local-login:droppen-app`. If the keychain will not take or give it, a
   `credentials.json` (0600) in
   `~/Library/Application Support/app.droppen.desktop/` (0700) instead. The app
   reads and writes only that item.
4. **Only to its own server.** Before the password is sent, the app checks that
   the server answering on the port is its child: the instance's lock (asked of
   the kernel) is held by the child's pid, and the instance's `server.port` —
   which only the lock holder writes, right after binding, and which the app
   clears before every start — names that port. After the login the app checks
   the instance name too, and logs out of anything else.
5. **Every launch** it logs in through `POST /api/auth/login` and opens the web
   app at
   `http://127.0.0.1:<port>/?droppen-desktop-load=<n>#droppen-desktop-nonce=<nonce>&droppen-desktop-token=<token>`.
   The init script (`src-tauri/src/init.js`, built with this launch's nonce)
   stores the token under `rh.auth.token` — where the web app keeps it — only
   if the nonce matches, and strips both from the address before any page
   script runs. A URL fragment is never sent to a server; the `load` counter
   only makes every such load a new document. The path is only ever one of the
   web app's own pages — never `/api`, `/embed` (the embedded apps' proxy) or
   the API documents — because a redirect keeps the fragment, and the login
   must never ride one off the server. No URL handed to the browser carries a
   fragment, but for the one shape a Droppen sign-in hands over, never on the
   app's own server ([Signing in to Droppen](#signing-in-to-droppen)).
6. **The login page is never shown, nor the setup page.** If the web app goes
   to `/login` (logged out, token expired or revoked), the window cancels the
   navigation and the app logs in again, to the page the web app wanted; the
   login it replaces is revoked. `/setup` is answered the same way: the app's
   own account is app-internal, so its instance is always in its setup state,
   and a lapsed login sends the web app there instead of to `/login` (the init
   script makes that the login page, coming back to the page being left). A
   refused login spends at most **one server restart** first (it
   clears the server's in-memory lockout after five wrong passwords, and
   changes nothing else) and then at most **one new password** (`user passwd`,
   which also revokes the account's logins) per app run or per *Try again*;
   and at most five re-logins in ten minutes. Past that — say, another local process keeps trying passwords — the
   window shows the app's own page with the reason and *Try again*. At quit the
   token is revoked.

The server's web app gets **no IPC** into the app: the only capability
(`src-tauri/capabilities/main.json`) is for the bundled start-up page. The
contract between the two is the URL fragment in and the document title out
(`core/src/web.rs`).

## What the web view supports

Checked on macOS 26 with the end-to-end build's probe (it logs what it finds
once the web app has loaded; `e2e-macos.sh` prints it): `localStorage`,
IndexedDB, websockets, the clipboard and the service worker all work (the
origin counts as a secure context), and the web app's worker registers. There
is no Push API and no Badging API. `Notification` exists (permission
`default`), but the web app asks for it only together with Web Push, which it
checks for first, so it never asks here and nothing breaks; the unread count
reaches the menu bar and the Dock badge through the title instead. (Phone
notifications through `rh-notify` are the server's and work as usual.)

Links: `window.open` to the app's own server opens a second app window (sharing
the login); a plain link off the app opens in the default browser (the init
script turns it into a new-window request). Frames inside a page load in place
— WebKit does not say which frame a navigation is for. A *page* load off the
app that gets past that (a script setting `location`, a form, a redirect) can
only be caught when it commits — wry has no earlier main-frame hook — so the
page goes to the browser and the window is taken back to the app's last page
from Rust, not through the page. Downloads are taken only from the app's own
server (or a `blob:` its pages made) into `~/Downloads`, marked with
`com.apple.quarantine` as a browser marks them; a download from anywhere else
goes to the browser instead.

WebKit keeps the web view's data (local storage, IndexedDB, the service
worker's cache) under the account's real home —
`~/Library/WebKit/app.droppen.desktop` and `~/Library/Caches/app.droppen.desktop`
— whatever `$HOME` says. An end-to-end test build's is its own bundle id's
(`app.droppen.desktop.e2e`, below), which the test scripts remove.

## Build

```sh
desktop/build.sh                       # universal Droppen.app + Droppen-<version>.dmg of HEAD
desktop/build.sh --ref v0.2.119        # a release
desktop/build.sh --worktree --arch aarch64   # this checkout as it is, Apple silicon only
desktop/build.sh --sidecar /path/to/remote-hand-server   # reuse a built server
desktop/build.sh --e2e --arch aarch64 --sidecar …        # an end-to-end test build (below)
```

It builds the public-distribution server exactly as `scripts/release-public.sh`
does, places it as `src-tauri/binaries/remote-hand-server-<target-triple>`,
installs the pinned `tauri-cli` (`cargo install --locked`, under
`~/.cache/droppen-desktop/tools`) and runs `cargo tauri build`. Output in
`/tmp/droppen-desktop-release/`; the scratch directory is deleted at the end.
It checks the release app carries no test hooks and no `$HOME` paths. The app
is its own Cargo workspace, outside `server-rs`'s, so server builds never
compile Tauri.

Every build but `--e2e` ships the app's licence and third-party notices:
`LICENSE` (AGPL-3.0-only) and `THIRD_PARTY_NOTICES` go into
`Droppen.app/Contents/Resources` and beside the app in the DMG (which
`build.sh` makes itself, like `notarise.sh`). The notices cover every Rust
crate of the app and of its bundled server, and every npm package of the
server's web app, made with the generators and allow-lists of
`scripts/release-public.sh` (`scripts/notices/`, cargo-about 0.9.2, which
such a build needs). Both pass the release text check
(`scripts/release-text-check.sh`) first, with the names from
`DROPPEN_RELEASE_FORBIDDEN` or `~/.config/droppen/release-forbidden.env`
(read as data, never run); a build refuses to finish without them, and
`notarise.sh` and `updater-artifacts.sh` run the check on the app's copies
again and refuse an app whose files are missing or fail it. A `--sidecar`
must be the server of the same ref: its `--version` must say `<version>+g<rev>`
of that commit.

Run the unit tests with `cargo test --workspace` here (`core/` holds everything
testable without a window: port, PATH, the sidecar's lifecycle, instance
ownership and the listener proof, the credential stores, the login calls, the
web contract, one app per user). A debug build makes a placeholder
`src-tauri/binaries/remote-hand-server-<target>` if there is none (Tauri
insists the sidecar exists; the placeholder only says to run `build.sh`); a
release build never does.

### End-to-end test build

The cargo feature `e2e` — only from `build.sh --e2e` — adds the test hooks a
release never has: `DROPPEN_DESKTOP_TEST_CONTROL=<dir>` (a private 0700
directory of your own; see `src-tauri/src/testctl.rs`) to drive close, show,
quit, *Start at Login* and *Try again* and to snapshot the web view;
`DROPPEN_DESKTOP_KEYCHAIN=0` to keep the password in the file;
`DROPPEN_DESKTOP_UPDATE_ENDPOINT` / `DROPPEN_DESKTOP_UPDATE_PUBKEY` for the
updater (without them a test build never checks for updates, and its build
allows an `http://` endpoint); `DROPPEN_DESKTOP_TEST_HUB=http://127.0.0.1:<port>`
for the hub the server signs in to (a local hub, `cloud-dev.sh up`; the
server's `DROPPEN_HUB_DEFAULT`, which the app otherwise never passes on); the
web view probe; and links logged instead of opened.

A test build is somebody else to macOS than the release, so it runs on a Mac
whose real Droppen is running without touching it (`core/src/identity.rs`,
`src-tauri/tauri.e2e.conf.json`, `e2e-identity.sh`):

| | release | end-to-end test build |
|---|---|---|
| bundle id — and with it the web view's data, user defaults, saved window state, LaunchServices, privacy permissions, the leftover-server check | `app.droppen.desktop` | `app.droppen.desktop.e2e` |
| bundle, logs folder, menu-bar tooltip | `Droppen.app`, `~/Library/Logs/Droppen` | `Droppen E2E.app`, `~/Library/Logs/Droppen E2E` |
| support folder (single-instance lock and socket, `state.json`, `server.pid`, the update's download), launch agent | `…/app.droppen.desktop` | `…/app.droppen.desktop.e2e` |
| Keychain item | `app.droppen.desktop` / `local-login:droppen-app` | `app.droppen.desktop.e2e` / `local-login:droppen-app-e2e` (and the runs turn the keychain off) |
| server instance | `droppen-app` | `droppen-app-e2e` |
| new ports from | 18700–18759 | 18800–18859, never one of the release's |
| `$HOME` | the account's | refused: only a throwaway one |
| *Start at Login* | `SMAppService` when signed with a Developer ID | never `SMAppService`; a launch agent file in the throwaway home only |

The app refuses to start when the bundle id it was built for and the bundle's
own differ, so a test build without `tauri.e2e.conf.json` never runs — and
the build refuses such an app first: `src-tauri/build.rs` checks the
identifier the build gets (`tauri.conf.json` merged with `--config`) against
`core/src/identity.rs` (an error, but for a debug build with the `e2e`
feature and no `--config`, as `cargo test --features e2e`, which only warns),
and `build.sh` checks the config and the built bundle against it too. A
release whose identifier changed without `identity.rs` would otherwise exit
on every launch, wherever the updater installed it. The test
scripts start nothing but such a build (`e2e_check_app`), write nowhere the
real app keeps anything (`e2e_check_scratch`), stop only processes running
from their own folders and by pid (`e2e_signal`), never quit, kill or look at
another app by name, run one at a time (`/tmp/droppen-e2e-run.lock`), and
check at the end that every Droppen app that ran before them still runs as
the same process. `e2e-macos.sh` runs the whole check against such a build in
a throwaway home with the keychain off:

```sh
desktop/build.sh --ref HEAD --out /tmp/droppen-release           # the release
desktop/build.sh --ref HEAD --e2e --arch aarch64 --out /tmp/droppen-e2e \
  --sidecar /tmp/droppen-release/Droppen.app/Contents/MacOS/remote-hand-server
desktop/e2e-macos.sh "/tmp/droppen-e2e/Droppen E2E.app" /tmp/desktop-review
```

Tests and test runs must never touch a real login keychain, and a release build
always uses it — so a release build is never launched by a test.

The updater has its own end-to-end check, with two test builds of different
versions (`--version`, test builds only) and a throwaway signing key:

```sh
desktop/build.sh --ref HEAD --e2e --version 0.0.1-test --arch aarch64 --out /tmp/droppen-e2e-old
desktop/build.sh --ref HEAD --e2e --version 0.0.2-test --arch aarch64 --out /tmp/droppen-e2e-new \
  --sidecar "/tmp/droppen-e2e-old/Droppen E2E.app/Contents/MacOS/remote-hand-server"
desktop/e2e-updater-macos.sh "/tmp/droppen-e2e-old/Droppen E2E.app" "/tmp/droppen-e2e-new/Droppen E2E.app" /tmp/desktop-updater-review
```

It keeps the app's window closed except for a few seconds, and photographs the
menu states and the web app's entry.

Icons: `icons-src/*.svg` (the mark from `web/public`), rendered with
`rsvg-convert -w 1024 -h 1024 icons-src/app-icon.svg -o /tmp/app-icon.png &&
cargo tauri icon /tmp/app-icon.png -o src-tauri/icons` and
`rsvg-convert -w 44 -h 44 icons-src/tray.svg -o src-tauri/icons/tray.png`
(keep only the macOS sizes the config lists).

## Signing in to Droppen

The window has no popups — a new window goes to the default browser — so
Settings → Droppen and the first-run card sign the device in through the
system browser (`docs/local-aggregator.md` §4.9, the browser hand-off):

1. the web app opens a sign-in attempt at its server and opens the attempt's
   `https://droppen.app/link#droppen-link=…` as a new window; the app hands it
   to the browser **with** its fragment (only an address of exactly that shape
   keeps one, `core/src/sign_in.rs`; the log says the host only);
2. in the browser the person signs in to droppen.app, compares the fingerprint
   with the one the window shows, and adds the device; droppen.app hands the
   enrollment code back, sealed to the attempt's one-off key, through a tab at
   `http://127.0.0.1:<port>/link/return` — the app's own server, which enrolls
   in process and starts its link to Droppen at once (no restart);
3. meanwhile the app asks its server how the device stands (`GET /api/cloud`,
   every 2 s, for at most 26 minutes) and brings its window to the front once
   it is `linked` (`src-tauri/src/sign_in.rs`).

The attempt is the app's own account's (`droppen-app`, app-internal): on the
app's server — and only there — that account may sign in a device whose link
nobody owns yet, and owns the link it signs in; a link a person's account owns
stays theirs (`server-rs/src/api/cloud.rs`, `may_manage`).

droppen.app has to serve this hand-off before an app that uses it ships: an
older droppen.app only says to open the sign-in from the device's settings.

## What is not here yet

- **Signing and notarisation**: `desktop/notarise.sh` does it, once the
  release Mac has the Developer ID certificate and the `droppen-notary`
  profile back (`SIGNING.md`, "What the release Mac needs";
  `desktop/notarise.sh --check` says what is missing). Until then users open
  the app with right-click → Open (macOS 15+: System Settings → Privacy &
  Security → Open Anyway), and no update can be released: the updater ships
  only notarised apps.
- **Signing in from a phone** (TODO(cloud sign-in, phone)): a phone that
  scans the device's code needs the hub rendezvous of
  `docs/local-aggregator.md` §4.9. 「登录 Droppen」 in the window itself works
  ([Signing in to Droppen](#signing-in-to-droppen)).
- **`TauriShell` / `KeyStore`** (TODO(TauriShell)): the web app's shell
  abstraction (§6.2) gets a Tauri implementation with a real IPC bridge, the
  SPA loads from the signed bundle, and keys live in the keychain behind the
  `KeyStore` interface (§7.3) — `core/src/credentials.rs` is where that
  interface starts.
- **A launch agent instead of the app** for running with nobody logged in —
  that is what `scripts/install.sh` is for.
