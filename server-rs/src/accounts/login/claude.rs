//! Claude Code's part of the accounts: how a credential starts `claude`.

use super::Login;
use crate::accounts::store::{AccountError, AccountKind, AccountLaunch, AccountProvider, EnvVar};

/// Claude Code's [`Login`].
pub struct ClaudeCodeLogin;

impl Login for ClaudeCodeLogin {
    /// The credential as it stands, and nothing on the command line: claude
    /// reads a token or a block out of its environment by itself.
    ///
    /// Never refused. A block is handed over as it stands — a token, a
    /// gateway's four variables, a Bedrock setup with no token at all: the CLI
    /// is the thing that knows which of those work, and second-guessing it here
    /// would refuse configurations that are fine.
    fn spawn_env(&self, kind: AccountKind, secret: &str, env: &[EnvVar]) -> Result<AccountLaunch, AccountError> {
        Ok(AccountLaunch {
            provider: AccountProvider::Claude,
            env: kind.vars(secret, env),
            codex_args: Vec::new(),
        })
    }
}
