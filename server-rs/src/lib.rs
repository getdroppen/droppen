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
//! * [`claude_roots`] / [`process_env`] — what a spawn runs against and with;
//! * [`env`] — the environment variables this server reads, under their
//!   Droppen names and the ones they had before;
//! * [`accounts`] — several Claude logins, stored server-side, and the two pty
//!   wizards that mint them;
//! * [`inject`] — driving the local CLIs, one run per session;
//! * [`drive`] — the session-driving routes, and the state all of the above
//!   hangs off ([`drive::Drive`]);
//! * [`chats`] — instant chats: CLI sessions in an anchor directory, plus the
//!   BYO-API backend and its streamed replies;
//! * [`quota`] — the subscription panel;
//! * [`files`] — browsing, reading and uploading under the home directory, and
//!   the one place a client-supplied path is resolved and bounded;
//! * [`media`] — images in private GitHub repositories, fetched through the
//!   host's own `gh` login so a transcript can show what an agent pushed;
//! * [`terminals`] — PTYs the browser drives over a websocket, each one owned
//!   by a detached [`pty_shim`] so it survives a server restart;
//! * [`instance`] — which server owns the adoptable resources on this machine;
//! * [`apps`] — a VS Code server per app, and the [`apps::embed`] reverse proxy
//!   the iframe reaches it through;
//! * [`auth`] — local accounts: who may talk to this server at all, and the
//!   guard every request passes through before [`origin`]'s;
//! * [`attention`] — which sessions are waiting on you, and [`push`] — telling
//!   a phone about it while the tab is closed;
//! * [`notify`] — the other direction of the same idea: the notification an
//!   *agent* decides to send, through a channel whose credential it never sees;
//! * [`schedule`] — cron tasks: a prompt, a directory and an expression, read in
//!   the server's own time zone, that start a session while nobody is watching;
//! * [`setup`] — the first-run guide's server half: which AI tools this
//!   device has, and the `PATH` that finds them;
//! * [`prefs`] / [`screens`] — the sidecar stores;
//! * [`stats`] / [`models`] — token accounting and the model list;
//! * [`origin`] — the guard every `/api/*` request passes through.
//!
//! # Shared contract plumbing
//!
//! Four pieces exist for every subsystem, not just the ones that added them:
//! [`openapi_ext::Nullable`] / [`openapi_ext::Optional`] for zod's two flavours
//! of "may be absent", [`api::created::Created`] for a `201`, and
//! [`api::extract`] for extractors whose rejection is a JSON `400` rather than
//! axum's `422`/`415` text.
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
pub mod agent;
pub mod aggregator;
pub mod api;
pub mod app;
pub mod apps;
pub mod attention;
pub mod auth;
pub mod chats;
pub mod claude_roots;
pub mod config;
pub mod db;
pub mod desktop_app;
pub mod drive;
pub mod env;
pub mod feed;
pub mod files;
pub mod identity;
pub mod inject;
pub mod instance;
pub mod media;
pub mod models;
pub mod notify;
pub mod openapi_ext;
pub mod origin;
pub mod plugins;
pub mod prefs;
pub mod process_env;
pub mod procs;
pub mod program;
pub mod pty_shim;
pub mod push;
pub mod quota;
pub mod record;
pub mod schedule;
pub mod screens;
pub mod selfupdate;
pub mod sessions;
pub mod setup;
pub mod shutdown;
pub mod static_files;
pub mod stats;
pub mod supervisor;
pub mod terminals;
pub mod text;
pub mod time;
