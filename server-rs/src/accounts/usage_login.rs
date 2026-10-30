//! `claude auth login`, run in a terminal against one account's probe directory.
//!
//! Port of the `startUsageLogin` half of the former TypeScript server's
//! `usage-login.ts` and the route that drives it in its `app.ts`. The other
//! half — `verifyUsageLogin`, which is a plain `claude auth status --json` with
//! no pty involved — is [`verify_usage_login`], beside the login it checks.
//!
//! A `setup-token` token can run sessions but cannot read usage: its scope is
//! `user:inference` only. Reading the quota needs a real login, so each account
//! gets an isolated `CLAUDE_CONFIG_DIR` that `claude auth login` fills in and the
//! quota probe later reuses. Refreshing that login is the CLI's own business —
//! every `claude -p /usage` spawn renews the token it finds there.

use std::sync::Arc;
use std::time::Duration;

use gotcha::axum::extract::FromRef;
use gotcha::{api, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use droppen_http::created::Created;
use droppen_http::error::ApiError;
use droppen_http::extract::ApiPath;
use droppen_http::openapi_ext::{ExtraResponses, Optional};
use droppen_base::process_env::{apply_env, claude_env, clean_env_bare, resolve_on_path};
use droppen_workbench::terminals::{CreateSpec, Registry};

use droppen_accounts::store::{Account, AccountKind, AccountPatch, AccountProvider, AccountsStore};
use droppen_accounts::{account_not_found, parse_auth_status, AccountInputExt, AccountsState};

/// `POST /api/accounts/{id}/usage-login` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct UsageLoginStarted {
    /// Attach this terminal to drive the login.
    pub terminal_id: String,
}

/// `POST /api/accounts/{id}/usage-verify` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct UsageVerifyResult {
    /// Whether the account's probe directory holds a live login.
    pub logged_in: bool,
    /// The address that login belongs to, when the CLI reported one.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub email: Optional<String>,
}

/// Why this account cannot take part in a Claude OAuth flow, if it cannot.
///
/// Every wizard and login in this module drives `claude` against an account's
/// own probe directory, and only an *OAuth* account has one to drive: a pasted
/// key or bearer token authenticates per request, and a codex account holds a
/// key for a different vendor's CLI entirely. The codex answer names codex
/// rather than reusing `usage_not_supported`, because "this is the wrong CLI"
/// and "this kind of credential has no login" are different things to fix.
fn claude_oauth_refusal(account: &Account, verb: &str) -> Option<String> {
    if account.provider == AccountProvider::Codex {
        return Some(format!("codex accounts hold a local API key, so there is no login to {verb}"));
    }
    (account.kind != AccountKind::OauthToken).then(|| "usage_not_supported".to_string())
}

/// [`claude_oauth_refusal`] as the `409` the routes answer with.
fn claude_oauth_only(account: &Account, verb: &str) -> Option<ApiError> {
    claude_oauth_refusal(account, verb).map(ApiError::conflict)
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
pub async fn start(accounts: &AccountsStore, terminals: &Arc<Registry>, account: &Account) -> Result<UsageLoginStarted, UsageLoginError> {
    if let Some(refusal) = claude_oauth_refusal(account, "perform") {
        return Err(UsageLoginError::NotSupported(refusal));
    }
    if resolve_on_path("claude", &clean_env_bare()).is_none() {
        return Err(UsageLoginError::NoCli);
    }

    let probe = |dir: Result<std::path::PathBuf, droppen_accounts::store::AccountError>| dir.map_err(|error| UsageLoginError::Probe(error.to_string()));
    let config_dir = probe(accounts.usage_config_dir(&account.id))?;
    // An empty working directory, so the CLI adopts no project of the user's.
    let cwd = probe(accounts.usage_cwd(&account.id))?;
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

/// What the usage-login route uses of the server's state.
#[derive(Clone)]
pub struct UsageLoginState {
    /// The stored logins, and each account's probe directory a login runs in.
    pub accounts: Arc<AccountsStore>,
    /// Every live web terminal; the login runs in one.
    pub terminals: Arc<Registry>,
}
droppen_http::substate!(UsageLoginState);

/// Start `claude auth login` for one account, in a terminal.
#[api(id = "start_usage_login", group = "accounts", summary = "Start an account's usage login")]
pub async fn start_usage_login(State(state): State<UsageLoginState>, ApiPath(id): ApiPath<String>) -> Result<Created<UsageLoginStarted>, ApiError> {
    id.require_uuid()?;
    let Some(account) = state.accounts.get(&id).await else {
        return Err(ApiError::not_found("account not found"));
    };
    match start(&state.accounts, &state.terminals, &account).await {
        Ok(started) => Ok(Created(started)),
        Err(UsageLoginError::NotSupported(message)) => Err(ApiError::conflict(message)),
        Err(error) => Err(ApiError::internal(error.to_string())),
    }
}

const STATUS_TIMEOUT: Duration = Duration::from_secs(20);

/// Ask the CLI whether an account's probe directory holds a live login.
///
/// A missing CLI, a non-zero exit and a timeout all mean the same thing to the
/// caller: this config dir is not logged in.
async fn read_auth_status(config_dir: &std::path::Path, cwd: &std::path::Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut command = tokio::process::Command::new("claude");
    command
        .args(["auth", "status", "--json"])
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        // `execFile({timeout})` sends SIGTERM; dropping the future on timeout
        // has to do the same, or the CLI outlives the request that started it.
        .kill_on_drop(true);
    apply_env(
        &mut command,
        &claude_env([("CLAUDE_CONFIG_DIR".to_string(), config_dir.to_string_lossy().into_owned())]),
    );

    let Ok(Ok(output)) = tokio::time::timeout(STATUS_TIMEOUT, command.output()).await else {
        return None;
    };
    if !output.status.success() {
        return None;
    }
    parse_auth_status(&String::from_utf8_lossy(&output.stdout))
}

/// Whether the account's probe directory holds a live login.
///
/// Makes the stored account agree with the answer. Never fails on a CLI
/// failure — an unusable login is a `false`, not a 500.
#[api(id = "verify_usage_login", group = "accounts", summary = "Verify an account's usage login")]
pub async fn verify_usage_login(State(state): State<AccountsState>, ApiPath(id): ApiPath<String>) -> Result<Json<UsageVerifyResult>, ApiError> {
    id.require_uuid()?;
    let accounts = &state.accounts;
    let Some(account) = accounts.get(&id).await else {
        return Err(account_not_found());
    };
    // A pasted key or bearer token authenticates per request, and a codex
    // account has no Claude login at all; neither has a login to verify.
    if let Some(refusal) = claude_oauth_only(&account, "verify") {
        return Err(refusal);
    }

    let (Ok(config_dir), Ok(cwd)) = (accounts.usage_config_dir(&id), accounts.usage_cwd(&id)) else {
        return Ok(Json(UsageVerifyResult {
            logged_in: false,
            email: Optional(None),
        }));
    };
    // An unwritable probe directory is not a live login either.
    if tokio::fs::create_dir_all(&config_dir).await.is_err() || tokio::fs::create_dir_all(&cwd).await.is_err() {
        return Ok(Json(UsageVerifyResult {
            logged_in: false,
            email: Optional(None),
        }));
    }

    let status = read_auth_status(&config_dir, &cwd).await;
    let logged_in = status
        .as_ref()
        .and_then(|s| s.get("loggedIn"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !logged_in {
        let _ = accounts
            .update(
                &id,
                AccountPatch {
                    usage_enabled: Some(false),
                    ..Default::default()
                },
            )
            .await;
        return Ok(Json(UsageVerifyResult {
            logged_in: false,
            email: Optional(None),
        }));
    }

    let email = status
        .and_then(|s| s.get("email").and_then(serde_json::Value::as_str).map(|e| e.trim().to_string()))
        .unwrap_or_default();
    let _ = accounts
        .update(
            &id,
            AccountPatch {
                usage_enabled: Some(true),
                email: Some(email.clone()),
                ..Default::default()
            },
        )
        .await;
    Ok(Json(UsageVerifyResult {
        logged_in: true,
        email: Optional((!email.is_empty()).then_some(email)),
    }))
}

impl UsageLoginState {
    /// Register the usage-login routes: the login, and the check of it.
    pub fn routes<S>(router: GotchaRouter<S>) -> GotchaRouter<S>
    where
        S: Clone + Send + Sync + 'static,
        AccountsState: FromRef<S>,
        UsageLoginState: FromRef<S>,
    {
        router
            .post("/api/accounts/{id}/usage-verify", verify_usage_login)
            .post("/api/accounts/{id}/usage-login", start_usage_login)
    }
}

/// The usage-login routes' statuses gotcha cannot derive.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("POST", "/api/accounts/{id}/usage-verify", 200, "Whether the account's probe directory holds a live login")
        .error("POST", "/api/accounts/{id}/usage-verify", 404, "Account not found")
        .error("POST", "/api/accounts/{id}/usage-verify", 409, "API-key accounts have no login to verify")
        .restatus(
            "POST",
            "/api/accounts/{id}/usage-login",
            200,
            201,
            "Login wizard running in a terminal; verify when it finishes",
        )
        .error("POST", "/api/accounts/{id}/usage-login", 404, "Account not found")
        .error("POST", "/api/accounts/{id}/usage-login", 409, "API-key accounts have no login to perform")
        .error("POST", "/api/accounts/{id}/usage-login", 500, "The wizard could not be started");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an OAuth account has a Claude login, and only a claude account has
    /// a *Claude* one — two different refusals, because they are two different
    /// things to fix.
    #[test]
    fn every_claude_wizard_refuses_a_codex_account_by_name() {
        let account = |provider: AccountProvider, kind: AccountKind| Account {
            id: "11111111-2222-3333-4444-555555555555".into(),
            label: "Account".into(),
            kind,
            email: Optional(None),
            usage_enabled: false,
            created_at: String::new(),
            updated_at: String::new(),
            unverified: Optional(None),
            provider,
            env_keys: Optional((kind == AccountKind::Env).then(|| vec!["ANTHROPIC_API_KEY".to_string()])),
            rest: Vec::new(),
            rotation: false,
        };

        assert_eq!(
            claude_oauth_refusal(&account(AccountProvider::Claude, AccountKind::OauthToken), "verify"),
            None,
            "the one account every wizard is for"
        );
        assert_eq!(
            claude_oauth_refusal(&account(AccountProvider::Claude, AccountKind::Env), "verify").as_deref(),
            Some("usage_not_supported"),
            "the wording env-mode clients already handle: a block authenticates per request"
        );
        let codex = claude_oauth_refusal(&account(AccountProvider::Codex, AccountKind::Env), "perform").expect("refused");
        assert!(codex.contains("codex"), "the refusal says which CLI it is about: {codex}");
    }
}
