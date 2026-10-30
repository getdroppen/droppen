//! Each CLI's part of the accounts, until it moves into its provider: Claude
//! Code's and Codex's [`droppen_accounts::login::Login`] and quota probes, and
//! the two wizards that sign Claude Code in (`setup_token`, `usage_login`).

pub mod login;
pub mod quota;
pub mod setup_token;
pub mod usage_login;

#[cfg(test)]
mod cli_tests;
