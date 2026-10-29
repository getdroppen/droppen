//! The Droppen server's supervisor and self-update: the loop that keeps a
//! server running (`remote-hand-server up`), and the route that replaces the
//! running binary from inside the running server. They stand on the instance
//! and the HTTP kit, and name none of the server's subsystems.
//!
//! * [`supervisor`] — start one child, wait for it, start it again, step down
//!   to `.prev` and the baked binary when a new one will not stay up, and reap
//!   whatever else wanders in;
//! * [`selfupdate`] — `POST /api/server/self-upgrade`, and the `<data>/bin`
//!   store of binaries it installs into.
//!
//! A name in backticks such as `auth` or `desktop_app` in these docs is a
//! module of the server, which this crate cannot link to.

pub mod selfupdate;
pub mod supervisor;
