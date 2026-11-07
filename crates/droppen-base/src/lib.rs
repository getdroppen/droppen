//! The Droppen server's base helpers: the pieces every subsystem needs and
//! none owns. Nothing here names a subsystem, so the server may use any of it
//! from anywhere without tying its subsystems together.
//!
//! * [`time`] — the clock, and the ISO-8601 spelling the server writes and reads;
//! * [`text`] — hex, the SHA-256 digest spelling, terminal-escape stripping and
//!   the image extensions a browser draws;
//! * [`fsutil`] — the one way a file only this user may read is written;
//! * [`procs`] — what a remembered pid still means, and signalling process groups;
//! * [`outbound`] — the guards every outbound webhook shares;
//! * [`env`](mod@env) — the environment variables the server reads, under their Droppen
//!   names and the ones they had before;
//! * [`process_env`] — the environment a spawned CLI runs with, and the
//!   redaction its output passes through;
//! * [`program`] — the two names the binary answers to;
//! * [`env_block`] — the `KEY=VALUE` block a person types for a CLI's environment;
//! * [`watch`] — a debounced "this file or directory changed" signal.

pub mod env;
pub mod env_block;
pub mod fsutil;
pub mod outbound;
pub mod process_env;
pub mod procs;
pub mod program;
pub mod text;
pub mod time;
pub mod watch;
