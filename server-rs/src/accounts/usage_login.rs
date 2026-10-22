//! `claude auth login`, run in a terminal against one account's probe directory.
//!
//! Port of the `startUsageLogin` half of the former TypeScript server's
//! `usage-login.ts` and the route that drives it in its `app.ts`. The other
//! half — `verifyUsageLogin`, which is a plain `claude auth status --json` with
//! no pty involved — is [`super::verify_usage_login`], and was ported with the
//! accounts routes.
//!
//! A `setup-token` token can run sessions but cannot read usage: its scope is
//! `user:inference` only. Reading the quota needs a real login, so each account
//! gets an isolated `CLAUDE_CONFIG_DIR` that `claude auth login` fills in and the
//! quota probe later reuses. Refreshing that login is the CLI's own business —
//! every `claude -p /usage` spawn renews the token it finds there.

use std::sync::Arc;

use gotcha::{api, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::api::created::Created;
use crate::api::error::ApiError;
use crate::api::extract::ApiPath;
use crate::api::AppState;
use crate::drive::Drive;
use crate::process_env::{claude_env, clean_env_bare, resolve_on_path};
use crate::terminals::{CreateSpec, Registry};

use super::store::Account;

/// `POST /api/accounts/{id}/usage-login` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct UsageLoginStarted {
    /// Attach this terminal to drive the login.
    pub terminal_id: String,
}

/// Why the login wizard could not be started.
#[derive(Debug, thiserror::Error)]
pub enum UsageLoginError {
    /// A pasted key or bearer token authenticates per request, and a codex
    /// account has no Claude login at all; neither has a login to perform.
    #[error("{0}")]
    NotSupported(String),
    /// `claude` is not installed, or not on this server's `PATH`.
    #[error("the `claude` CLI is not on PATH, so the login wizard cannot start")]
    NoCli,
    /// The account's probe directories could not be made.
    #[error("cannot prepare the account's probe directory: {0}")]
    Probe(String),
    /// The pty could not be opened, or `claude` could not be spawned.
    #[error("cannot start the login wizard")]
    Terminal,
}

/// Put `claude auth login` on screen against this account's probe directory.
///
/// The api-key rejection comes first, deliberately: nothing is spawned and no
/// directory is created for an account that can never have a login.
pub async fn start(drive: &Arc<Drive>, terminals: &Arc<Registry>, account: &Account) -> Result<UsageLoginStarted, UsageLoginError> {
    if let Some(refusal) = super::claude_oauth_refusal(account, "perform") {
        return Err(UsageLoginError::NotSupported(refusal));
    }
    if resolve_on_path("claude", &clean_env_bare()).is_none() {
        return Err(UsageLoginError::NoCli);
    }

    let probe = |dir: Result<std::path::PathBuf, super::store::AccountError>| dir.map_err(|error| UsageLoginError::Probe(error.to_string()));
    let config_dir = probe(drive.accounts.usage_config_dir(&account.id))?;
    // An empty working directory, so the CLI adopts no project of the user's.
    let cwd = probe(drive.accounts.usage_cwd(&account.id))?;
    for dir in [&config_dir, &cwd] {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|error| UsageLoginError::Probe(error.to_string()))?;
    }

    let mut spec = CreateSpec::new("claude", vec!["auth".to_string(), "login".to_string()]);
    spec.env = Some(claude_env([
        ("CLAUDE_CONFIG_DIR".to_string(), config_dir.to_string_lossy().into_owned()),
        ("TERM".to_string(), "xterm-256color".to_string()),
    ]));
    spec.cwd = Some(cwd.to_string_lossy().into_owned());
    spec.title = Some(format!("claude auth login — {}", account.label));

    let info = terminals.create_terminal_with(spec).await.map_err(|error| {
        tracing::error!(%error, "the usage-login wizard could not open a terminal");
        UsageLoginError::Terminal
    })?;
    Ok(UsageLoginStarted { terminal_id: info.id })
}

/// Start `claude auth login` for one account, in a terminal.
#[api(id = "start_usage_login", group = "accounts", summary = "Start an account's usage login")]
pub async fn start_usage_login(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Created<UsageLoginStarted>, ApiError> {
    super::require_uuid(&id)?;
    let Some(account) = state.drive.accounts.get(&id).await else {
        return Err(ApiError::not_found("account not found"));
    };
    match start(&state.drive, &state.terminals, &account).await {
        Ok(started) => Ok(Created(started)),
        Err(UsageLoginError::NotSupported(message)) => Err(ApiError::conflict(message)),
        Err(error) => Err(ApiError::internal(error.to_string())),
    }
}
