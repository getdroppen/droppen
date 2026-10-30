//! The accounts' rules against each CLI's own login.
//!
//! The accounts' own tests run on stand-in logins
//! (`droppen_accounts::login::Logins::stand_ins`), which say nothing about what Claude
//! Code's or Codex's login answers. The tests here are the ones that do: each
//! runs on the logins the entry builds ([`crate::app::logins`]), so a login
//! filed under the wrong CLI, or a question asked of the wrong CLI's login,
//! fails them. Each CLI's part of the accounts is its provider's:
//! [`droppen_provider_claude_code`] and [`droppen_provider_codex`].

mod local_login;
mod login;
mod rotation;
mod routes;
mod store;
