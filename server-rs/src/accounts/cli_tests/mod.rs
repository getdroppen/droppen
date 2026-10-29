//! The accounts' rules against each CLI's own login.
//!
//! The accounts' own tests run on stand-in logins
//! ([`super::login::Logins::stand_ins`]), which say nothing about what Claude
//! Code's or Codex's login answers. The tests here are the ones that do: each
//! runs on the logins the entry builds ([`crate::app::logins`]), so a login
//! filed under the wrong CLI, or a question asked of the wrong CLI's login,
//! fails them.

mod local_login;
mod login;
mod rotation;
mod routes;
mod store;
