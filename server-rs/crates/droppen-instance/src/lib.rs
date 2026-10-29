//! The Droppen server's instance: where it keeps its files, the directory and
//! lock that make it the one owner of what it can adopt, who it is, and its
//! databases. Nearly every subsystem of the server stands on these, so they
//! name none of them.
//!
//! * [`paths`] — the home directory, the config and data roots, the
//!   instance's directory and databases, and the two Claude config roots;
//! * [`instance`] — the directory one server exclusively owns, and the lock
//!   that says so;
//! * [`identity`] — who this server is, in a form a rename cannot change;
//! * [`db`] — the instance's own state, in SQLite, and its append-only schema.
//!
//! A name in backticks such as `supervisor` or `record` in these docs is a
//! module of the server, which this crate cannot link to.

pub mod db;
pub mod identity;
pub mod instance;
pub mod paths;
