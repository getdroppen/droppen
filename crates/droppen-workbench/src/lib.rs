//! The Droppen server's panes and the layout around sessions:
//!
//! * [`terminals`] — PTYs the browser drives over a websocket, each one owned
//!   by a detached [`droppen_pty`] shim so it survives a server restart;
//! * [`apps`] — a VS Code server per app, and the [`apps::embed`] reverse proxy
//!   the iframe reaches it through;
//! * [`screens`] — saved pane layouts, followed live by every open page;
//! * [`prefs`] — the UI preferences.
//!
//! A name in backticks such as `feed` in these docs is a module of the server,
//! which this crate cannot link to.

pub mod apps;
pub mod prefs;
pub mod screens;
pub mod terminals;
