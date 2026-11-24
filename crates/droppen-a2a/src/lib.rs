//! Droppen's A2A v1.0 server (`docs/a2a.md`): other agents hand this device a
//! task, follow it, answer its questions and take its result.
//!
//! * [`wire`] — the protocol's objects, as their JSON is written;
//! * [`error`] — its errors, and how each binding writes them;
//! * [`entry`] — an entry: one agent, configured here;
//! * [`token`] — A2A tokens, `dpa_<id>_<secret>`;
//! * [`store`] — the A2A tables of the instance database;
//! * [`backend`] — [`SessionBackend`], the port the sessions are reached
//!   through, and its events;
//! * [`engine`] — [`A2a`]: what each operation and each event does to a task;
//! * [`protocol`] — `/a2a/*`: the Agent Card and the JSON-RPC and HTTP+JSON
//!   bindings;
//! * [`manage`] — `/api/a2a/*`: entries, tokens and the task list;
//! * `tck` — the A2A TCK's stand-in entry.
//!
//! The server builds [`A2a`] with a backend and an [`backend::EventInbox`],
//! nests [`protocol::router`] at `/a2a`, registers [`manage::A2aState`]'s
//! routes, and calls [`A2a::resume`] once it owns its instance.

pub mod backend;
pub mod engine;
pub mod entry;
pub mod error;
pub mod manage;
pub mod protocol;
pub mod store;
mod tck;
/// A fake [`backend::SessionBackend`] for tests. Only built for tests and with
/// the `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod token;
pub mod wire;

#[cfg(test)]
mod tests;

pub use backend::SessionBackend;
pub use engine::{A2a, Caller, EventKind, Settings, TaskEvent, TaskEventSink};
pub use manage::A2aState;
