//! The Droppen desktop app's shell logic, kept free of Tauri so it can be unit
//! tested on its own (`cargo test -p droppen-desktop-core`) without bundling a
//! sidecar or opening a window.
//!
//! The app is a thin shell around the ordinary Droppen server binary
//! (`remote-hand-server`), bundled as a sidecar. Everything the shell has to
//! decide before it can show the server's own web app lives here:
//!
//! * [`dirs`] — where the app keeps its state, its logs and the server's pid;
//! * [`identity`] — who the app is to macOS, and the end-to-end test build's
//!   own identity, apart from the release's;
//! * [`port`] — a free loopback port, kept across launches when it can be;
//! * [`env_path`] — the `PATH` the server and the AI CLIs it starts get, since a
//!   GUI app on macOS does not inherit the shell's;
//! * [`sidecar`] — the server's environment, its lifecycle (start, restart with
//!   back-off, stop by pid) and the clean-up of one a crashed app left behind;
//! * [`instance`] — the instance directory the app owns, its lock, and the
//!   proof that the server on the port is the app's own;
//! * [`single`] — one app per macOS user;
//! * [`login_item`] — *Start at Login*: `SMAppService` for a signed app, the
//!   old launch agent carried over to it once;
//! * [`credentials`] — the local account's password: the app's own secret, in
//!   the macOS Keychain or, failing that, a 0600 file;
//! * [`accounts`] — creating that account with the server's own `user` CLI;
//! * [`server`] — health, login and logout over the server's existing API, and
//!   *Open in Browser*'s one-time page link, asked for with the app's login;
//! * [`browser`] — handing the browser that link, off every command line;
//! * [`sign_in`] — signing the device in to Droppen through the system
//!   browser: the hand-off address kept whole, and the window brought forward
//!   once the device joined;
//! * [`web`] — the seam to the web app the window shows: the login in the URL
//!   fragment, the unread count out of the title, the login page never shown;
//! * [`state`] and [`logging`] — the little the app remembers, and its log;
//! * [`update`] — the app's own updates: when to check, what to ask, and the
//!   manifest a release publishes.
//!
//! Nothing here changes how the server authenticates: the app is a client that
//! happens to know a password it generated itself.

pub mod accounts;
pub mod browser;
pub mod credentials;
pub mod dirs;
pub mod env_path;
pub mod identity;
pub mod instance;
pub mod logging;
pub mod login_item;
pub mod port;
pub mod server;
pub mod sidecar;
pub mod sign_in;
pub mod single;
pub mod state;
pub mod update;
pub mod web;

/// The server instance the app runs: its own directory, lock, accounts and
/// sessions under `~/.local/share/remote-hand/instances/droppen-app`, apart
/// from any instance a command-line install (`default`) or a hand-made launchd
/// agent runs. Distinctive on purpose: the app refuses to adopt a directory of
/// that name it did not create ([`instance`]). An end-to-end test build's is
/// `droppen-app-e2e` ([`identity`]).
pub const INSTANCE: &str = identity::THIS.instance;

/// The bundle identifier, also the Keychain service name and the name of the
/// app's directory under `~/Library/Application Support` and of its launch
/// agent: `app.droppen.desktop`, or an end-to-end test build's own
/// `app.droppen.desktop.e2e` ([`identity`]).
pub const BUNDLE_ID: &str = identity::THIS.bundle_id;

/// The server binary's name, as Tauri bundles it beside the app's own
/// executable (`Contents/MacOS/remote-hand-server`).
pub const SIDECAR_NAME: &str = "remote-hand-server";
