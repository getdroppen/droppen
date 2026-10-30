//! Droppen's server: the entry. Every subsystem is a crate of its own under
//! `server-rs/crates`, in the layers `server-rs/layers.json` gives it; this
//! crate builds them into one program and serves them.
//!
//! * [`app`] — the application: its configuration and the state every route
//!   reads ([`api::AppState`]), the CLIs it drives
//!   ([`app::register_providers`], [`app::transcript_sources`],
//!   [`app::logins`]), the ports one crate declares and another crate's types
//!   answer, and a serve loop that can be stopped;
//! * [`api`] — the router: each subsystem's routes on a state of its own,
//!   made from [`api::AppState`], and the session list, detail, socket and
//!   settings, which read several subsystems' state at once;
//! * [`stream_source`] — what the link copies of this machine's sessions, the
//!   agent's [`droppen_agent::streams::source::StreamSource`] over the
//!   sessions and the drive's live state;
//! * [`cli`] — the command line, and [`export`] its `export-openapi`;
//! * [`static_files`] — the web app, embedded;
//! * [`desktop_app`] — a server the desktop app started;
//! * [`shutdown`] — the signals that stop the server and what it cleans up on
//!   the way out.
//!
//! Its tests are the ones that need the whole program: the provider base's
//! on the providers [`app::register_providers`] registers
//! (`provider_tests`), the accounts' and the setup guide's on the logins
//! [`app::logins`] builds (`accounts_tests`, `setup_tests`), and the routes
//! through the router.
//!
//! # Shared contract plumbing
//!
//! Four pieces exist for every subsystem, not just the ones that added them:
//! [`droppen_http::openapi_ext::Nullable`] /
//! [`droppen_http::openapi_ext::Optional`] for zod's two flavours of "may be
//! absent", [`droppen_http::created::Created`] for a `201`, and
//! [`droppen_http::extract`] for extractors whose rejection is a JSON `400`
//! rather than axum's `422`/`415` text.
//!
//! # What is deliberately absent
//!
//! One thing only: **browsers** (`/api/browsers*`). The feature is being
//! retired along with the TypeScript server, so it was dropped rather than
//! ported and then deleted, and the web app's link to it goes with it.
//!
//! Everything else the TypeScript server serves is here.

#![deny(missing_docs)]

pub use gotcha::{GotchaError, GotchaResult};

#[cfg(test)]
mod accounts_tests;
pub mod api;
pub mod app;
pub mod cli;
pub mod desktop_app;
pub mod export;
#[cfg(test)]
mod provider_tests;
#[cfg(test)]
mod setup_tests;
pub mod shutdown;
pub mod static_files;
pub mod stream_source;
