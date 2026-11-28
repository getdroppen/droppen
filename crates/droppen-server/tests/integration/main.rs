//! Consolidated integration test binary for `droppen-server`.
//!
//! Grouping tests that share the standard fake-CLI environment into one
//! integration binary avoids linking the full server dependency graph dozens
//! of times per build. Tests that mutate process-global environment variables
//! (`push`, `watchdog`, `wizards`, `wizard_without_cli`) stay in separate
//! binaries so `cargo test` remains race-free even without `nextest`.

#[path = "../support/mod.rs"]
pub mod support;
pub use support::common;

mod accounts;
mod acp;
mod apps;
mod attention;
mod auth;
mod chats;
mod chats_cli;
mod cli_argv;
mod codex;
mod codex_archive;
mod drive;
mod env_names;
mod files;
mod global_feed;
mod inject;
mod inject_restart;
mod origin;
mod ownership;
mod probe;
mod pty_shim;
mod quota;
mod self_update;
mod session_shim;
mod spawn_env;
mod stats;
mod storage;
mod subagents;
mod terminals;
mod worktrees;
