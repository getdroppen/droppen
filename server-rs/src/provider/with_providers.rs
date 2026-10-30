//! The provider base's tests that need what only the server has: the
//! providers the entry registers, the app, and the server's own sources. They
//! stay in the server until the providers leave it.

mod adoption;
mod gate;
mod journal;
mod registry;

/// Where this binary's session shim is: a provider's tests that start a CLI
/// behind a shim run it as this module's `session_shim`
/// ([`droppen_provider::testing::shims_run_as`]).
pub(crate) const SHIMS: &str = module_path!();

/// Not a test: this binary's session shim
/// ([`droppen_provider::testing::session_shim`]).
#[test]
#[ignore = "the session shim a provider test's respawn runs, not a test"]
fn session_shim() {
    droppen_provider::testing::session_shim();
}
