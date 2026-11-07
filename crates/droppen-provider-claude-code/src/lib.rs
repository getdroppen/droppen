//! Claude Code's provider: its resident driver, its model list, its session
//! reader, the config roots its spawns run against, the recording each run
//! leaves, and its part of the accounts: its login, its usage probe and the
//! two wizards that sign an account in.

pub mod claude_roots;
pub mod login;
pub mod models;
pub mod quota;
pub mod record;
pub mod resident;
pub mod sessions;
pub mod setup_token;
pub mod usage_login;

/// This crate's test binary run as the session shim: a resident test's
/// respawn starts it ([`droppen_provider::testing::shims_run_as`]).
#[cfg(test)]
mod shims {
    /// Where this test binary's session shim is.
    pub(crate) const SHIMS: &str = module_path!();

    /// Not a test: this test binary's session shim
    /// ([`droppen_provider::testing::session_shim`]).
    #[test]
    #[ignore = "the session shim a resident test's respawn runs, not a test"]
    fn session_shim() {
        droppen_provider::testing::session_shim();
    }
}
