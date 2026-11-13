//! What a test needs to start real shims, and a server must never be handed.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The binary every shim this process starts is spawned from, once a test has
/// said which ([`shims_run_from`]).
static SHIM_PROGRAM: OnceLock<PathBuf> = OnceLock::new();

/// Spawn every shim this process starts from `program`, the server binary
/// cargo built for the test: `env!("CARGO_BIN_EXE_droppen-server")` in the
/// server's integration tests.
///
/// A test runs a server inside a test harness, where [`super::shim_program`]'s
/// own executable is the test binary, which has no `pty-shim`: libtest refuses
/// `--socket`, exits 101, and the socket the server dials never appears. So a
/// test that starts real shims says where the server is, rather than leaving
/// it to be worked out from where cargo put the test binary.
///
/// For the whole process, and the first word stands: every test in one binary
/// names the same file. Not an environment variable, because a test would have
/// to write it while a server in the same process reads the environment, which
/// on glibc is undefined behaviour.
pub fn shims_run_from(program: impl Into<PathBuf>) {
    let _ = SHIM_PROGRAM.set(program.into());
}

/// The program [`shims_run_from`] named, if a test has.
pub(crate) fn shim_program() -> Option<PathBuf> {
    SHIM_PROGRAM.get().cloned()
}
