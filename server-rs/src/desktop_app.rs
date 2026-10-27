//! A server run by the Droppen desktop app (`desktop/` in this repository).
//!
//! The app starts this binary as a sidecar with `DROPPEN_DESKTOP_APP=1` (and,
//! for a pre-rename server, `RH_DESKTOP_APP=1`, which is still
//! read: [`crate::env`]), and two things are different about such a server:
//!
//! * **It never replaces its binary.** The app runs the server it ships
//!   inside its own bundle and is updated as a whole. `POST
//!   /api/server/self-upgrade` would instead write `<data>/bin`, the store
//!   every `up`-supervised instance on the device runs from, and leave the app
//!   itself unchanged — so it is refused ([`crate::selfupdate`]).
//! * **Its login is the app's.** The app made the password and signs in with
//!   it on every launch; nobody is meant to know it. `GET /api/auth/status`
//!   says `desktopApp: true`, and the web app hides what would be a dead end
//!   there — changing a password nobody has, and signing out of a login the
//!   app restores at once — and signs in to Droppen through the system browser
//!   instead of a popup its window cannot have (the browser hand-off,
//!   [`crate::api::link_handoff`]).
//!
//! Read once, at boot, into [`crate::api::AppState::desktop_app`], so a test
//! sets the field instead of the process environment every test shares.

/// The environment variable the app sets (under its old name too).
pub const ENV: &str = "DROPPEN_DESKTOP_APP";

/// Whether this process was started by the desktop app.
///
/// The spellings are [`crate::env::flag`]'s, like every other switch: `1`,
/// `true`, `yes`, `on` (any case) turn it on, and nothing else does.
pub fn from_env() -> bool {
    crate::env::flag(ENV)
}
