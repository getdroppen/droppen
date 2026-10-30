//! remote-hand's server, on the gotcha framework.
//!
//! The port is in progress. It began as a four-route feasibility probe, chosen
//! because between them those routes exercise every framework seam the port
//! depends on: OpenAPI parity with multi-status responses, a programmatic spec
//! export, a filesystem-driven websocket feed, an embedded SPA, and graceful
//! shutdown. Everything that has landed since is a faithful port of the
//! matching TypeScript module, named after it.
//!
//! # Layout
//!
//! Each subsystem is a module directory exposing `routes` and `describe`, and
//! is registered with one line each in [`api`]. Its state is one struct, and it
//! adds one field to [`api::AppState`].
//!
//! * [`sessions`] — discovery, the transcript normalizer and the wire shapes;
//! * [`feed`] — the per-session websocket feed, and [`feed::global`] the
//!   session-list one behind `GET /api/events`;
//! * [`provider::claude_code::claude_roots`] / [`droppen_base::process_env`] — what a spawn runs
//!   against and with;
//! * [`droppen_base::env`] — the environment variables this server reads,
//!   under their Droppen names and the ones they had before;
//! * [`accounts`] — several Claude logins, stored server-side, and the two pty
//!   wizards that mint them;
//! * [`provider`] — driving the local CLIs, one run per session;
//! * [`drive`] — the session-driving routes, and the state all of the above
//!   hangs off ([`drive::Drive`]);
//! * [`chats`] — instant chats: CLI sessions in an anchor directory, plus the
//!   BYO-API backend and its streamed replies;
//! * [`quota`] — the subscription panel;
//! * [`droppen_files`] — browsing, reading and uploading under the home directory, and
//!   the one place a client-supplied path is resolved and bounded;
//! * [`droppen_files::media`] — images in private GitHub repositories, fetched through the
//!   host's own `gh` login so a transcript can show what an agent pushed;
//! * [`droppen_workbench::terminals`] — PTYs the browser drives over a websocket, each one owned
//!   by a detached [`droppen_pty`] shim so it survives a server restart;
//! * [`droppen_instance::instance`] — which server owns the adoptable resources on this machine;
//! * [`droppen_workbench::apps`] — a VS Code server per app, and the [`droppen_workbench::apps::embed`] reverse proxy
//!   the iframe reaches it through;
//! * [`droppen_auth`] — local accounts: who may talk to this server at all, and the
//!   guard every request passes through before [`droppen_http::origin`]'s;
//! * [`attention`] — which sessions are waiting on you, and [`push`] — telling
//!   a phone about it while the tab is closed;
//! * [`notify`] — the other direction of the same idea: the notification an
//!   *agent* decides to send, through a channel whose credential it never sees;
//! * [`schedule`] — cron tasks: a prompt, a directory and an expression, read in
//!   the server's own time zone, that start a session while nobody is watching;
//! * [`setup`] — the first-run guide's server half: which AI tools this
//!   device has, and the `PATH` that finds them;
//! * [`droppen_workbench::prefs`] / [`droppen_workbench::screens`] — the sidecar stores;
//! * [`stats`] / [`droppen_provider::models`] — token accounting and the model list;
//! * [`droppen_http::origin`] — the guard every `/api/*` request passes through.
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

pub mod accounts;
pub mod api;
pub mod app;
pub mod chats;
pub mod cli;
pub mod desktop_app;
pub mod drive;
pub mod export;
pub mod feed;
pub mod provider;
pub mod schedule;
pub mod sessions;
pub mod shutdown;
#[cfg(test)]
mod setup_tests;
pub mod static_files;
pub mod stream_source;
