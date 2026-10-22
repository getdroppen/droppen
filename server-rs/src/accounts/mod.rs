//! Several Claude logins, one managed config root.
//!
//! Port of the account routes in the former TypeScript server's `app.ts`. Three
//! of them are wizards the user drives in a terminal and live in their own
//! modules: [`setup_token`] mints a token off `claude setup-token`'s own
//! output, and [`usage_login`] puts `claude auth login` on screen against one
//! account's probe directory. `POST /api/accounts/{id}/usage-verify` stays
//! here, because it is a plain `claude auth status --json` with no PTY
//! involved.

pub mod limits;
pub mod local_login;
pub mod rest;
pub mod rotation;
pub mod setup_token;
pub mod store;
pub mod usage_login;
pub mod verify;

use std::time::Duration;

use futures_util::StreamExt;

use gotcha::axum::http::StatusCode;
use gotcha::{api, GotchaContext, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::api::created::Created;
use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath, ApiQuery};
use crate::api::{AppState, SendResult};
use crate::claude_roots::ClaudeRoot;
use crate::config::AppConfig;
use crate::openapi_ext::{required_nullable, ExtraResponses, Nullable, Optional};
use crate::process_env::{apply_env, claude_env};
use crate::sessions::{claude, Source};
use rest::AccountLimitEventList;
use store::{Account, AccountError, AccountKind, AccountList, AccountPatch, AccountProvider, Credential};

pub use store::AccountRef;

/// `POST /api/accounts` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountCreate {
    /// Display name for the account.
    pub label: String,
    /// How it authenticates.
    pub kind: AccountKind,
    /// The OAuth token, for `kind: oauth_token`. Never returned.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub secret: Optional<String>,
    /// The `KEY=VALUE` block, for `kind: env`. Never returned.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub env: Optional<String>,
    /// Which CLI it is for; defaults to claude.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub provider: Optional<AccountProvider>,
    /// Whether it joins the rotation pool; defaults by provider and kind.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub rotation: Optional<bool>,
}

/// `PATCH /api/accounts/{id}` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountUpdate {
    /// New display name.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub label: Optional<String>,
    /// A replacement credential — re-authorization by paste.
    ///
    /// Checked against Anthropic before it is stored, and never returned. The
    /// account keeps its id, so every session bound to it carries on under it.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub secret: Optional<String>,
    /// A replacement `KEY=VALUE` block, for an env account.
    ///
    /// Replaces the whole block: what is not in the text is not set. Checked
    /// before it is stored, exactly as a replacement token is.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub env: Optional<String>,
    /// Put the account in or take it out of the rotation pool.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub rotation: Optional<bool>,
}

/// `PUT /api/accounts/default` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountDefault {
    /// The account new sessions follow; `null` chooses the local login, which
    /// then stays where they start even while it is signed out.
    #[serde(deserialize_with = "required_nullable")]
    pub account_id: Nullable<String>,
    /// Which CLI's default to set; defaults to claude.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub provider: Optional<AccountProvider>,
    /// `true` (with `accountId: null`) clears the default rather than choosing
    /// the local login: new sessions then start on the local login, or — while
    /// it is signed out — on an account in the auto-rotation pool standing in
    /// for it (`GET /api/accounts/start`). What a fresh install has.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub automatic: Optional<bool>,
}

/// `PUT /api/sessions/{source}/{id}/account` body and response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionAccount {
    /// The account this session runs under; `null` unbinds it.
    #[serde(deserialize_with = "required_nullable")]
    pub account_id: Nullable<String>,
}

/// `POST /api/sessions/{source}/{id}/continue` body.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionContinue {
    /// The stored claude account the new session runs under.
    pub account_id: String,
}

/// The session `POST /api/sessions/{source}/{id}/continue` made.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ContinuedSession {
    /// Which CLI it belongs to; always `claude`.
    pub source: Source,
    /// The new session's id. The session it was copied from keeps its own.
    pub id: String,
}

/// The `409` body when a token is already stored.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountConflict {
    /// Always `duplicate_account`.
    pub error: String,
    /// The label the token is already filed under.
    pub existing_label: String,
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

/// `GET /api/accounts/limit-events` and `GET /api/accounts/{id}/limit-events` query.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LimitEventsQuery {
    /// How many events, newest first; 100 for the whole log and 50 for one
    /// account when absent, never more than 500.
    pub limit: Option<u32>,
}

/// `GET /api/accounts/start` query.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountStartQuery {
    /// Which CLI; claude when absent.
    pub provider: Option<AccountProvider>,
    /// The model the start will ask for. A claude rest scoped to one model
    /// family stands in the way of that family's sessions only, so the answer
    /// can depend on it; absent is a start that names no model at all (the
    /// CLI's own default, as a scheduled task with none set).
    pub model: Option<String>,
    /// The start will ask for whichever model this answer names (a composer
    /// with nothing stored): choose the login and its model together, so that
    /// sending that model lands on that login. Ignored with `model`.
    pub choose_model: Option<bool>,
}

/// `DELETE /api/accounts/{id}/rest` query.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct RestClearQuery {
    /// End only this scope's rest (`all`, or a model family); absent ends every one.
    pub scope: Option<String>,
}

/// An account id that is not a uuid can name no account.
fn account_not_found() -> ApiError {
    ApiError::not_found("account not found")
}

/// The `400` for an account that belongs to the other CLI.
///
/// A machine-readable token beside the sentence, like
/// `local_session_cannot_use_account`: the client picks the account list it
/// offers from the session's own CLI, so reaching this is a bug in the client
/// rather than something a person typed, and it should be able to say so.
fn provider_mismatch() -> ApiError {
    ApiError::with_body(
        StatusCode::BAD_REQUEST,
        serde_json::json!({
            "error": "provider_mismatch",
            "hint": "this account authenticates the other CLI; a codex session needs a codex account and a claude session a claude one",
        }),
    )
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

/// An id that is not a uuid names no account, so it gets the same `404` a uuid
/// that names nothing does.
///
/// The TypeScript answered `400` here — `z.string().uuid()` on the path
/// parameter failed before the handler ran, and `@hono/zod-validator` renders
/// every failure as `c.json(result, 400)`. But that status was never part of the
/// contract: `DELETE /api/accounts/{id}`, `/usage-login` and `/usage-verify`
/// document only `404` for a missing account, and `PATCH`'s `400` is documented
/// as "unusable label". A client written against the contract handles `404`; it
/// has no reason to expect a `400` that is not written down.
fn require_uuid(id: &str) -> Result<(), ApiError> {
    if store::is_uuid(id) {
        Ok(())
    } else {
        Err(account_not_found())
    }
}

/// `z.string().min(1).max(100)` on a label.
fn require_label(label: &str) -> Result<(), ApiError> {
    match label.chars().count() {
        0 => Err(ApiError::bad_request("the label is empty")),
        n if n > 100 => Err(ApiError::bad_request("the label is too long")),
        _ => Ok(()),
    }
}

/// Store errors become the statuses the TypeScript routes answer with.
///
/// Everything the TypeScript recognises as an `AccountError` is reported
/// verbatim; anything else is a plain `fs` failure it lets escape as a 500, so
/// [`AccountError::Io`] is the only variant that does not become a `4xx`.
fn store_error(error: AccountError, fallback: &str) -> ApiError {
    match error {
        AccountError::Duplicate { existing_label, .. } => ApiError::with_body(
            StatusCode::CONFLICT,
            serde_json::json!({ "error": "duplicate_account", "existingLabel": existing_label }),
        ),
        AccountError::Invalid(message) => ApiError::bad_request(message),
        AccountError::ProviderMismatch { .. } => provider_mismatch(),
        AccountError::NotFound(_) => ApiError::bad_request(fallback.to_string()),
        AccountError::Io(_) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, fallback.to_string()),
    }
}

/// Stored Claude accounts (secrets never leave the server).
#[api(id = "list_accounts", group = "accounts", summary = "List accounts")]
pub async fn list_accounts(State(state): State<AppState>) -> Json<AccountList> {
    Json(state.drive.accounts.account_list().await)
}

/// `z.string().min(1).max(4096)` on a secret; the store checks the rest.
fn require_secret(secret: &str) -> Result<(), ApiError> {
    if secret.is_empty() || secret.chars().count() > 4096 {
        return Err(ApiError::bad_request("the token is unusable"));
    }
    Ok(())
}

/// Spend one real request proving a credential works before it is filed.
///
/// The shape of a credential says nothing about whether it *is* one: a token cut
/// short by a wrapped terminal is the right length, the right alphabet and the
/// right prefix, and 401s on every session started under it. Only a refusal from
/// Anthropic keeps a secret out — see [`verify`] for why a probe that could not
/// run is not a refusal.
///
/// `Ok(Some(why))` is that third case: stored, but nobody has proved it works.
/// It is returned rather than swallowed because the UI promises a check, and a
/// promise kept silently only when convenient is not one.
async fn require_working(state: &AppState, provider: AccountProvider, credential: &Credential) -> Result<Option<String>, ApiError> {
    match verify::verify_secret(&state.drive.paths, provider, credential).await {
        verify::Verdict::Valid => Ok(None),
        verify::Verdict::Invalid(reason) => Err(ApiError::bad_request(reason)),
        verify::Verdict::Unverifiable(why) => {
            tracing::warn!(%why, "storing a credential without checking it");
            Ok(Some(why))
        }
    }
}

/// The credential a request carries, or a `400` saying why it carries none.
///
/// The two shapes are exclusive in both directions. An OAuth account is a token
/// and an env account is a block, and accepting both — or neither — would store
/// a row whose `kind` says one thing and whose contents say another.
fn credential_of(kind: AccountKind, secret: Option<&str>, env: Option<&str>) -> Result<Credential, ApiError> {
    match (kind, secret, env) {
        (AccountKind::OauthToken, Some(secret), None) => {
            require_secret(secret)?;
            Ok(Credential::Oauth(secret.trim().to_string()))
        }
        (AccountKind::Env, None, Some(env)) => store::parse_env_block(env)
            .map(Credential::Env)
            .map_err(|error| store_error(error, "cannot read the environment block")),
        (AccountKind::OauthToken, _, Some(_)) | (AccountKind::Env, Some(_), _) => Err(ApiError::bad_request(
            "an OAuth account takes a token and an env account takes an environment block; send one, not both",
        )),
        (AccountKind::OauthToken, None, None) => Err(ApiError::bad_request("an OAuth account needs a token")),
        (AccountKind::Env, None, None) => Err(ApiError::bad_request("an env account needs an environment block")),
    }
}

/// Whether a patch changes what the credential *is*, rather than what it is
/// called. A rename must not spend a request on a probe.
fn touches_credential(body: &AccountUpdate) -> bool {
    body.secret.as_deref().is_some() || body.env.as_deref().is_some()
}

/// Store a new Claude account.
#[api(id = "create_account", group = "accounts", summary = "Add an account")]
pub async fn create_account(State(state): State<AppState>, ApiJson(body): ApiJson<AccountCreate>) -> Result<Created<Account>, ApiError> {
    require_label(&body.label)?;
    let provider = body.provider.0.unwrap_or_default();
    // The shape is checked before a request is spent on the credential: a
    // combination that could never spawn is not worth a round trip to a vendor.
    store::validate_pairing(provider, body.kind).map_err(|error| store_error(error, "cannot store account"))?;
    let credential = credential_of(body.kind, body.secret.as_deref(), body.env.as_deref())?;
    let unchecked = require_working(&state, provider, &credential).await?;
    state
        .drive
        .accounts
        .create_with(&body.label, provider, credential, body.rotation.0)
        .await
        .inspect(|_| {
            // A new account may be where a held session can go.
            state.drive.injections.recheck_holds();
        })
        .map(|account| {
            Created(Account {
                unverified: Optional(unchecked),
                ..account
            })
        })
        .map_err(|error| store_error(error, "cannot store account"))
}

/// Rename an account, or re-authorize it with a pasted credential.
#[api(id = "update_account", group = "accounts", summary = "Update an account")]
pub async fn update_account(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<AccountUpdate>,
) -> Result<Json<Account>, ApiError> {
    require_uuid(&id)?;
    if let Some(label) = body.label.as_deref() {
        require_label(label)?;
    }
    let mut unchecked = None;
    let mut credential = None;
    if touches_credential(&body) {
        let Some(account) = state.drive.accounts.get(&id).await else {
            return Err(account_not_found());
        };
        // Read under the *account's* own kind, so a token posted at an env
        // account (or the other way round) is a `400` here rather than a row
        // whose `kind` and contents disagree.
        let replacement = credential_of(account.kind, body.secret.as_deref(), body.env.as_deref())?;
        // A block is a credential in as many pieces as it has variables, and
        // any of them can be the one that stops it working — so a re-typed
        // block is proved exactly as a re-pasted token is.
        unchecked = require_working(&state, account.provider, &replacement).await?;
        credential = Some(replacement);
    }
    let patch = AccountPatch {
        label: body.label.0,
        rotation: body.rotation.0,
        credential,
        ..Default::default()
    };
    let updated = state.drive.accounts.update(&id, patch).await;
    if matches!(updated, Ok(Some(_))) {
        // A re-authorized account, or one just put in the pool, may be where a
        // held session can go — or the very account it is held against.
        state.drive.injections.recheck_holds();
    }
    match updated {
        Ok(Some(account)) => Ok(Json(Account {
            unverified: Optional(unchecked),
            ..account
        })),
        Ok(None) => Err(account_not_found()),
        Err(error) => Err(store_error(error, "cannot update account")),
    }
}

/// Delete an account, its session bindings and its probe login.
#[api(id = "delete_account", group = "accounts", summary = "Delete an account")]
pub async fn delete_account(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    require_uuid(&id)?;
    if !state
        .drive
        .accounts
        .delete(&id)
        .await
        .map_err(|e| store_error(e, "cannot delete account"))?
    {
        return Err(account_not_found());
    }
    Ok(Json(SendResult { accepted: true }))
}

/// Point new sessions at an account, or back at the local login.
#[api(id = "set_default_account", group = "accounts", summary = "Set the default account")]
pub async fn set_default_account(State(state): State<AppState>, ApiJson(body): ApiJson<AccountDefault>) -> Result<Json<AccountList>, ApiError> {
    let account_id = body.account_id.0;
    let provider = body.provider.0.unwrap_or_default();
    if body.automatic.0 == Some(true) {
        if account_id.is_some() {
            return Err(ApiError::bad_request("automatic clears the default: it takes no accountId"));
        }
        state
            .drive
            .accounts
            .clear_default(provider)
            .await
            .map_err(|e| store_error(e, "cannot clear the default account"))?;
        return Ok(Json(state.drive.accounts.account_list().await));
    }
    if let Some(id) = account_id.as_deref() {
        require_uuid(id)?;
    }
    if !state
        .drive
        .accounts
        .set_default(account_id.as_deref(), provider)
        .await
        .map_err(|e| store_error(e, "cannot set the default account"))?
    {
        return Err(account_not_found());
    }
    Ok(Json(state.drive.accounts.account_list().await))
}

/// The login a start that names no account would run on now, and why.
///
/// The rule a chat, a scheduled task and `POST /api/sessions/new` without
/// `accountId` follow ([`rotation::start_for`]), asked beforehand so a picker
/// shows what the start will actually use — including the moves only the
/// server can see coming: a default resting on the model family asked for, or
/// a window about to run out.
///
/// With `chooseModel` the answer also names the model, because then the model
/// is the composer's default and that default is the head of the login's own
/// list — so the login decides the model while the model's family can decide
/// the login. Asked one at a time the two never settle; asked together they
/// do: the logins in start order ([`rotation::StartInputs::start_order`]), each
/// with its own models in order, and the first model the rule places on its
/// own login wins. A start sent with that model and no account lands there by
/// the same rule the spawn applies.
#[api(id = "account_start", group = "accounts", summary = "Where a start that names no account runs")]
pub async fn account_start(State(state): State<AppState>, ApiQuery(query): ApiQuery<AccountStartQuery>) -> Json<rotation::AccountStart> {
    let provider = query.provider.unwrap_or_default();
    let model = query.model.filter(|model| !model.trim().is_empty());
    let inputs = rotation::StartInputs::gather(&state.drive.accounts, &state.drive.quota, provider).await;
    if model.is_some() || query.choose_model != Some(true) || !inputs.model_matters() {
        return Json(inputs.place(model.as_deref()));
    }
    // The candidates' lists a few at a time, in start order: a list the CLI
    // has to be asked for can take seconds (and asking it starts a `claude`),
    // and a start left to the server waits for this answer — but the first
    // login with a model of its own it lands on ends the walk, and no further
    // list is asked for.
    let source = provider.source();
    let state = &state;
    let mut lists = futures_util::stream::iter(inputs.start_order())
        .map(|login| async move {
            let models = crate::models::model_ids(state, source, login.as_deref()).await;
            (login, models)
        })
        .buffered(START_PROBES);
    let mut asked = Vec::new();
    while let Some((login, models)) = lists.next().await {
        if inputs.own_model(login.as_deref(), &models).is_some() {
            return Json(inputs.choose(&[(login, models)]));
        }
        asked.push((login, models));
    }
    Json(inputs.choose(&asked))
}

/// How many logins' model lists `GET /api/accounts/start` asks for at once.
const START_PROBES: usize = 3;

/// Pin a session to an account, or unbind it.
#[api(id = "bind_session_account", group = "accounts", summary = "Bind a session to an account")]
pub async fn bind_session_account(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SessionAccount>,
) -> Result<Json<SessionAccount>, ApiError> {
    let account_id = body.account_id.0;
    let provider = AccountProvider::of(source);
    if let Some(account_id) = account_id.as_deref() {
        require_uuid(account_id)?;
        // Before anything is read off disk: an account for the other CLI is a
        // client bug, and it is the same answer whatever the session turns out
        // to be.
        let Some(account) = state.drive.accounts.get(account_id).await else {
            return Err(ApiError::not_found("account not found"));
        };
        if account.provider != provider {
            return Err(provider_mismatch());
        }
    }
    if source == Source::Codex {
        return bind_codex_session(&state, &id, account_id).await;
    }
    let found = claude::find_claude_session(&state.paths, &id).await;
    // Scoped to claude, because the binding table holds both CLIs' rows: a
    // codex thread id posted here would otherwise skip the not-found check on
    // the strength of its *codex* binding, and then overwrite it.
    let bound = state.drive.accounts.session_account_id_for(&id, provider).await;
    if found.is_none() && bound.is_none() {
        return Err(ApiError::not_found("session not found"));
    }
    // A session in the user's own root keeps the user's own login, always — and
    // that stays true of one *this server* started on the local login. An
    // account spawn runs against this instance's own Claude root, so the
    // transcript would have to be there, and nothing moves it there: copying it
    // would fork the conversation. The fork is a separate, explicit step —
    // [`continue_session_on_account`] — that makes a *new* session and leaves
    // this one as it was.
    if account_id.is_some() && found.map(|f| f.root) == Some(ClaudeRoot::Local) {
        return Err(ApiError::with_body(
            gotcha::axum::http::StatusCode::CONFLICT,
            serde_json::json!({
                "error": "local_session_cannot_use_account",
                "hint": "this session runs on the local login and its transcript is in ~/.claude, \
                         so it cannot move onto an account. Continue it on the account instead, \
                         which copies the conversation into a new session.",
            }),
        ));
    }
    write_binding(&state, Source::Claude, &id, account_id).await
}

/// Carry a local-login claude session onto an account, as a new session.
///
/// The answer to `local_session_cannot_use_account`, for the moment it matters:
/// the local login stopped working and the conversation is stuck on it. The
/// session's transcript is copied into this instance's own root under a new id
/// ([`crate::sessions::continuation`]), the copy is bound to the account, and
/// the first message sent to it resumes the whole conversation there. The
/// original is read and never written; it stays as it was, on the local login.
///
/// Only a session this server started (`external_session` otherwise) and only
/// one on the local login: a session already in this instance's root switches
/// accounts in place, and so does every codex thread.
#[api(
    id = "continue_session_on_account",
    group = "accounts",
    summary = "Continue a local-login session on an account"
)]
pub async fn continue_session_on_account(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiJson(body): ApiJson<SessionContinue>,
) -> Result<Json<ContinuedSession>, ApiError> {
    if source != Source::Claude {
        return Err(ApiError::with_body(
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "error": "continue_not_supported",
                "hint": "a codex session switches accounts in place; bind it to the account instead",
            }),
        ));
    }
    require_uuid(&body.account_id)?;
    let Some(account) = state.drive.accounts.get(&body.account_id).await else {
        return Err(account_not_found());
    };
    if account.provider != AccountProvider::Claude {
        return Err(provider_mismatch());
    }
    // The same row, and the same ownership verdict, the list and the composer
    // were given: somebody else's session is not this server's to copy.
    let summary = crate::drive::routes::writable_session(&state.drive, Source::Claude, &id).await?;
    if summary.claude_root != Some(ClaudeRoot::Local) {
        return Err(ApiError::with_body(
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": "session_not_on_local_login",
                "hint": "this session already runs in this server's own root; switch its account instead",
            }),
        ));
    }

    let new_id = uuid::Uuid::new_v4().to_string();
    let copy = crate::sessions::continuation::copy_local_session(&state.paths, &id, &new_id)
        .await
        .map_err(|error| match error {
            crate::sessions::continuation::ContinueError::NotFound => ApiError::not_found("session not found"),
            crate::sessions::continuation::ContinueError::Taken => ApiError::conflict(error.to_string()),
            crate::sessions::continuation::ContinueError::Io(_) => {
                tracing::warn!(session = %id, %error, "cannot copy a local-login session into this instance's root");
                ApiError::internal("cannot copy the conversation")
            }
        })?;
    if let Err(error) = state
        .drive
        .accounts
        .bind_session_account(&new_id, Some(&account.id), AccountProvider::Claude)
        .await
    {
        // A copy with no account would list as a managed session that refuses
        // every message; better no copy at all.
        tracing::warn!(session = %id, continued = %new_id, %error, "cannot bind the continued session; removing the copy");
        let _ = tokio::fs::remove_file(&copy).await;
        if let Some(project) = copy.parent() {
            let _ = tokio::fs::remove_dir_all(project.join(&new_id)).await;
        }
        return Err(ApiError::not_found("cannot bind account"));
    }
    // A person picked this account: nothing moves it ahead of a limit for a while.
    state
        .drive
        .accounts
        .rotation_log()
        .note_hand_binding(&new_id, crate::accounts::rest::now_ms());
    // A name somebody gave the conversation goes with it.
    let titles = state.drive.titles.read().await;
    if let Some(title) = titles.get(&crate::sessions::reference::title_key(Source::Claude, &id)) {
        if let Err(error) = state.drive.titles.set(Source::Claude, &new_id, title).await {
            tracing::warn!(session = %new_id, %error, "cannot carry the title over to the continued session");
        }
    }
    tracing::info!(session = %id, continued = %new_id, account = %account.id, "continued a local-login session on an account as a new session");
    state.global.request_refresh(Some((Source::Claude, &new_id)));
    Ok(Json(ContinuedSession {
        source: Source::Claude,
        id: new_id,
    }))
}

/// The codex half of the route above.
///
/// Ownership rather than a config root, because codex has no second root to
/// own: a thread this server did not start is one the user is driving
/// somewhere else, and re-pointing its login from here would change which
/// account the *other* driver bills. The refusal is the send route's own, so a
/// client that already renders `external_session` renders this too.
async fn bind_codex_session(state: &AppState, id: &str, account_id: Option<String>) -> Result<Json<SessionAccount>, ApiError> {
    let bound = state.drive.accounts.session_account_id_for(id, AccountProvider::Codex).await;
    match crate::drive::routes::writable_session(&state.drive, Source::Codex, id).await {
        Ok(_) => {}
        // A session with no transcript yet but a binding already written is one
        // this server is in the middle of starting; it is ours by construction.
        Err(_) if bound.is_some() => {}
        Err(error) => return Err(error),
    }
    write_binding(state, Source::Codex, id, account_id).await
}

/// Store the binding and tell the feed the row changed.
async fn write_binding(state: &AppState, source: Source, id: &str, account_id: Option<String>) -> Result<Json<SessionAccount>, ApiError> {
    state
        .drive
        .accounts
        .bind_session_account(id, account_id.as_deref(), AccountProvider::of(source))
        .await
        // Both arms are a 404 there: the route reports `err.message` for an
        // `AccountError` and its own fallback for anything else.
        .map_err(|error| match error {
            AccountError::ProviderMismatch { .. } => provider_mismatch(),
            AccountError::NotFound(_) => ApiError::not_found(error.to_string()),
            _ => ApiError::not_found("cannot bind account"),
        })?;
    // Somebody chose this account for the session: nothing moves it ahead of a
    // limit until the choice has stood for a while (see
    // [`crate::accounts::rotation::HAND_BINDING_GRACE_MS`]).
    if source == Source::Claude && account_id.is_some() {
        state.drive.accounts.rotation_log().note_hand_binding(id, crate::accounts::rest::now_ms());
    }
    // The binding shows on the session row, so the list has to be re-read.
    state.global.request_refresh(Some((source, id)));
    Ok(Json(SessionAccount {
        account_id: Nullable(account_id),
    }))
}

const STATUS_TIMEOUT: Duration = Duration::from_secs(20);

/// `{"loggedIn": true, "email": "…"}`, with any banner around it tolerated.
pub fn parse_auth_status(raw: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(&raw[start..=end]) {
        Ok(serde_json::Value::Object(map)) => Some(map),
        _ => None,
    }
}

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
pub async fn verify_usage_login(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<UsageVerifyResult>, ApiError> {
    require_uuid(&id)?;
    let accounts = &state.drive.accounts;
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

/// Resolve the `{id}` of a limit route: a stored account, or the machine's
/// own login under its `local:claude` / `local:codex` name.
async fn limit_subject(state: &AppState, id: &str) -> Result<(), ApiError> {
    if rest::is_local(id) {
        return Ok(());
    }
    require_uuid(id)?;
    match state.drive.accounts.get(id).await {
        Some(_) => Ok(()),
        None => Err(account_not_found()),
    }
}

/// The limits this instance has seen, newest first.
///
/// Recorded, not acted on: a limit here changed nothing about how the session
/// that hit it was driven. Includes the machine's own login, whose events carry
/// `accountId: null`.
#[api(id = "list_account_limit_events", group = "accounts", summary = "List recent limit events")]
pub async fn list_limit_events(State(state): State<AppState>, ApiQuery(query): ApiQuery<LimitEventsQuery>) -> Json<AccountLimitEventList> {
    let limit = query.limit.unwrap_or(100) as usize;
    Json(AccountLimitEventList {
        events: state.drive.accounts.limit_log().events(None, limit),
    })
}

/// One account's limit events, newest first.
#[api(id = "list_account_limit_events_for", group = "accounts", summary = "List an account's limit events")]
pub async fn list_account_limit_events(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<LimitEventsQuery>,
) -> Result<Json<AccountLimitEventList>, ApiError> {
    limit_subject(&state, &id).await?;
    let limit = query.limit.unwrap_or(50) as usize;
    Ok(Json(AccountLimitEventList {
        events: state.drive.accounts.limit_log().events(Some(&id), limit),
    }))
}

/// End an account's rest — one scope, or all of them.
#[api(id = "clear_account_rest", group = "accounts", summary = "Clear an account's rest")]
pub async fn clear_account_rest(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<RestClearQuery>,
) -> Result<Json<AccountList>, ApiError> {
    limit_subject(&state, &id).await?;
    let scope = query.scope.as_deref().map(str::trim).filter(|scope| !scope.is_empty());
    match state.drive.accounts.limit_log().clear(&id, scope) {
        Ok(cleared) => tracing::info!(account = %id, scope = ?scope, cleared, "account rest cleared by hand"),
        Err(error) => {
            tracing::warn!(account = %id, %error, "cannot clear an account's rest");
            return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "cannot clear the rest".to_string()));
        }
    }
    // Somebody says this account is usable again: a session held against it
    // lifts, and one held with nowhere to go may now have somewhere.
    state.drive.injections.recheck_holds();
    Ok(Json(state.drive.accounts.account_list().await))
}

/// Register the account routes.
pub fn routes(router: GotchaRouter<GotchaContext<AppState, AppConfig>>) -> GotchaRouter<GotchaContext<AppState, AppConfig>> {
    router
        .get("/api/accounts", list_accounts)
        .get("/api/accounts/limit-events", list_limit_events)
        .get("/api/accounts/start", account_start)
        .get("/api/accounts/{id}/limit-events", list_account_limit_events)
        .delete("/api/accounts/{id}/rest", clear_account_rest)
        .post("/api/accounts", create_account)
        .patch("/api/accounts/{id}", update_account)
        .delete("/api/accounts/{id}", delete_account)
        .put("/api/accounts/default", set_default_account)
        .post("/api/accounts/{id}/usage-verify", verify_usage_login)
        .post("/api/accounts/setup-token", setup_token::start_setup_token)
        .get("/api/accounts/setup-token/{captureId}", setup_token::setup_token_status)
        .post("/api/accounts/setup-token/{captureId}/code", setup_token::submit_setup_token_code)
        .post("/api/accounts/{id}/setup-token", setup_token::reauth_account)
        .post("/api/accounts/{id}/usage-login", usage_login::start_usage_login)
        .put("/api/sessions/{source}/{id}/account", bind_session_account)
        .post("/api/sessions/{source}/{id}/continue", continue_session_on_account)
}

/// Every status gotcha cannot derive, plus the golden contract's wordings.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("GET", "/api/accounts", 200, "Stored Claude accounts (secrets never leave the server)")
        .describe(
            "GET",
            "/api/accounts/limit-events",
            200,
            "Limit events this instance recorded, newest first (recorded, not acted on)",
        )
        .describe("GET", "/api/accounts/{id}/limit-events", 200, "One account's limit events, newest first")
        .error("GET", "/api/accounts/{id}/limit-events", 404, "Account not found")
        .describe(
            "GET",
            "/api/accounts/start",
            200,
            "The login a start that names no account runs on, and why",
        )
        .describe("DELETE", "/api/accounts/{id}/rest", 200, "Rest cleared; the whole account list")
        .error("DELETE", "/api/accounts/{id}/rest", 404, "Account not found")
        .restatus("POST", "/api/accounts", 200, 201, "Account stored")
        .error("POST", "/api/accounts", 400, "Unusable label, or a token Anthropic refused")
        .body::<AccountConflict>("POST", "/api/accounts", 409, "That token is already stored under another label")
        .describe("PATCH", "/api/accounts/{id}", 200, "Account updated")
        .error("PATCH", "/api/accounts/{id}", 400, "Unusable label, or a token Anthropic refused")
        .error("PATCH", "/api/accounts/{id}", 404, "Account not found")
        .body::<AccountConflict>("PATCH", "/api/accounts/{id}", 409, "That token is already stored under another label")
        .describe(
            "DELETE",
            "/api/accounts/{id}",
            200,
            "Account, its session bindings and its probe login are gone",
        )
        .error("DELETE", "/api/accounts/{id}", 404, "Account not found")
        .describe(
            "PUT",
            "/api/accounts/default",
            200,
            "Default account for new sessions (null chooses the local login, signed out or not; automatic clears it)",
        )
        .error("PUT", "/api/accounts/default", 400, "An accountId sent with automatic")
        .error("PUT", "/api/accounts/default", 404, "Account not found")
        .describe(
            "POST",
            "/api/accounts/{id}/usage-verify",
            200,
            "Whether the account's probe directory holds a live login",
        )
        .error("POST", "/api/accounts/{id}/usage-verify", 404, "Account not found")
        .error("POST", "/api/accounts/{id}/usage-verify", 409, "API-key accounts have no login to verify")
        .restatus(
            "POST",
            "/api/accounts/setup-token",
            200,
            201,
            "Wizard running; attach the terminal and poll the capture",
        )
        .error("POST", "/api/accounts/setup-token", 500, "The wizard could not be started")
        .describe(
            "GET",
            "/api/accounts/setup-token/{captureId}",
            200,
            "Where the capture stands (never the token itself)",
        )
        .error("GET", "/api/accounts/setup-token/{captureId}", 404, "Unknown or expired capture")
        .describe(
            "POST",
            "/api/accounts/setup-token/{captureId}/code",
            200,
            "The authorization code was typed into the wizard",
        )
        .error("POST", "/api/accounts/setup-token/{captureId}/code", 400, "Not an authorization code")
        .error("POST", "/api/accounts/setup-token/{captureId}/code", 404, "Unknown or expired capture")
        .error(
            "POST",
            "/api/accounts/setup-token/{captureId}/code",
            409,
            "This capture is no longer waiting for a code",
        )
        .restatus(
            "POST",
            "/api/accounts/{id}/setup-token",
            200,
            201,
            "Wizard running for an account that already exists; its id and bindings are kept",
        )
        .error("POST", "/api/accounts/{id}/setup-token", 404, "Account not found")
        .error("POST", "/api/accounts/{id}/setup-token", 409, "API-key accounts have no OAuth flow")
        .error("POST", "/api/accounts/{id}/setup-token", 500, "The wizard could not be started")
        .restatus(
            "POST",
            "/api/accounts/{id}/usage-login",
            200,
            201,
            "Login wizard running in a terminal; verify when it finishes",
        )
        .error("POST", "/api/accounts/{id}/usage-login", 404, "Account not found")
        .error("POST", "/api/accounts/{id}/usage-login", 409, "API-key accounts have no login to perform")
        .error("POST", "/api/accounts/{id}/usage-login", 500, "The wizard could not be started")
        .describe(
            "PUT",
            "/api/sessions/{source}/{id}/account",
            200,
            "Session pinned to an account (null unbinds it)",
        )
        .error("PUT", "/api/sessions/{source}/{id}/account", 400, "The account belongs to the other CLI")
        .error("PUT", "/api/sessions/{source}/{id}/account", 404, "Session or account not found")
        .error(
            "PUT",
            "/api/sessions/{source}/{id}/account",
            409,
            "The session keeps the local login, or is not this instance's to bind",
        )
        .describe(
            "POST",
            "/api/sessions/{source}/{id}/continue",
            200,
            "A new session on the account, holding a copy of this conversation; the original is untouched",
        )
        .error(
            "POST",
            "/api/sessions/{source}/{id}/continue",
            400,
            "A codex session, or an account for the other CLI",
        )
        .error("POST", "/api/sessions/{source}/{id}/continue", 404, "Session or account not found")
        .error(
            "POST",
            "/api/sessions/{source}/{id}/continue",
            409,
            "Not this instance's session, or not one on the local login",
        )
        .error(
            "POST",
            "/api/sessions/{source}/{id}/continue",
            500,
            "The conversation could not be copied",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_auth_status_survives_a_banner_around_it() {
        let status = parse_auth_status("Welcome!\n{\"loggedIn\": true, \"email\": \"me@example.com\"}\nBye").expect("parsed");
        assert_eq!(status["loggedIn"], serde_json::Value::Bool(true));
        assert_eq!(status["email"], serde_json::Value::String("me@example.com".into()));

        assert!(parse_auth_status("no json here").is_none());
        assert!(
            parse_auth_status("}{").is_none(),
            "a closing brace before the opening one is not an object"
        );
        assert!(parse_auth_status("{broken").is_none());
    }

    /// A credential is the token *and* the endpoint, so moving either one has
    /// to be proved again — and renaming has to stay free.
    #[test]
    fn a_patch_is_checked_when_it_moves_either_half_of_the_credential() {
        let patch = |secret: Option<&str>, env: Option<&str>| AccountUpdate {
            label: Optional(None),
            secret: Optional(secret.map(str::to_owned)),
            env: Optional(env.map(str::to_owned)),
            rotation: Optional(None),
        };

        assert!(!touches_credential(&patch(None, None)), "a rename spends no request");
        assert!(
            !touches_credential(&AccountUpdate {
                label: Optional(Some("Renamed".into())),
                secret: Optional(None),
                env: Optional(None),
                rotation: Optional(Some(false)),
            }),
            "…even with a label in it, or a change of pool"
        );
        assert!(touches_credential(&patch(Some("sk-ant-new"), None)), "a new token");
        assert!(
            touches_credential(&patch(None, Some("ANTHROPIC_API_KEY=k"))),
            "a re-typed block: any one of its variables can be the one that stopped working"
        );
    }

    /// The binding route refuses the other CLI's account with a token the
    /// client can branch on, not a sentence it has to match.
    #[test]
    fn the_other_clis_account_is_a_400_naming_the_mismatch() {
        let rendered = format!("{:?}", provider_mismatch());
        assert!(rendered.contains("provider_mismatch"), "{rendered}");
        assert!(rendered.contains("400"), "a client bug, not a state the server is in: {rendered}");

        // The store's own refusal renders as the same token, so a mismatch that
        // slips past the route's early check reaches the client identically.
        let from_store = store_error(
            AccountError::ProviderMismatch {
                account: AccountProvider::Codex,
                wanted: AccountProvider::Claude,
            },
            "unused",
        );
        assert_eq!(format!("{from_store:?}"), rendered);
    }

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

    #[test]
    fn a_duplicate_becomes_the_widened_409_body() {
        let error = store_error(
            AccountError::Duplicate {
                existing_id: "id".into(),
                existing_label: "Work".into(),
            },
            "unused",
        );
        let rendered = format!("{error:?}");
        assert!(rendered.contains("duplicate_account"), "{rendered}");
        assert!(rendered.contains("Work"), "the existing label is what the UI shows: {rendered}");
    }

    // ------------------------------------------------- the local login

    const STUCK: &str = "11111111-1111-4111-8111-111111111111";

    /// A claude session this server started on the local login, in the user's
    /// own `~/.claude`, with a subagent transcript beside it.
    async fn local_session(paths: &crate::sessions::Paths, id: &str, recorded: bool) -> std::path::PathBuf {
        let project = crate::claude_roots::projects_dir(paths, ClaudeRoot::Local).join("-w");
        std::fs::create_dir_all(project.join(id).join("subagents")).expect("sidecar");
        let path = project.join(format!("{id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                "{{\"type\":\"user\",\"cwd\":\"/w\",\"sessionId\":\"{id}\",\"timestamp\":\"2026-10-19T05:11:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"fix the inbox\"}}}}\n\
                 {{\"type\":\"assistant\",\"sessionId\":\"{id}\",\"isApiErrorMessage\":true,\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"Failed to authenticate\"}}]}}}}\n"
            ),
        )
        .expect("transcript");
        std::fs::write(
            project.join(id).join("subagents").join("agent-1.jsonl"),
            format!("{{\"sessionId\":\"{id}\"}}\n"),
        )
        .expect("subagent");
        if recorded {
            crate::sessions::owned_sessions::record(paths, Source::Claude, id).await.expect("record");
        }
        path
    }

    async fn claude_account(state: &AppState, label: &str) -> Account {
        state
            .drive
            .accounts
            .create(
                label,
                AccountProvider::Claude,
                Credential::Oauth(format!("sk-ant-oat01-{}", "a".repeat(80))),
            )
            .await
            .expect("account")
    }

    async fn continue_on(state: &AppState, source: Source, id: &str, account_id: &str) -> Result<ContinuedSession, ApiError> {
        continue_session_on_account(
            State(state.clone()),
            ApiPath((source, id.to_string())),
            ApiJson(SessionContinue {
                account_id: account_id.to_string(),
            }),
        )
        .await
        .map(|json| json.0)
    }

    /// The trap this route exists for: a session on a signed-out local login
    /// cannot be bound to an account, so it is continued on one instead — a new
    /// session in this instance's root, bound to the account, holding the
    /// conversation, while the original stays exactly where and what it was.
    #[tokio::test]
    async fn a_local_login_session_continues_on_an_account_as_a_new_session() {
        let _guard = crate::db::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let paths = state.drive.paths.clone();
        let original = local_session(&paths, STUCK, true).await;
        let before = std::fs::read(&original).expect("read");
        let work = claude_account(&state, "work").await;

        // Binding is still refused, and now says what to do instead.
        let refused = bind_session_account(
            State(state.clone()),
            ApiPath((Source::Claude, STUCK.to_string())),
            ApiJson(SessionAccount {
                account_id: Nullable(Some(work.id.clone())),
            }),
        )
        .await
        .expect_err("a local session cannot be bound");
        assert_eq!(refused.body()["error"], "local_session_cannot_use_account");
        assert!(refused.body()["hint"].as_str().unwrap_or_default().contains("Continue it on the account"));

        let continued = continue_on(&state, Source::Claude, STUCK, &work.id).await.expect("continued");
        assert_eq!(continued.source, Source::Claude);
        assert_ne!(continued.id, STUCK, "a new session, not the old one moved");
        assert!(crate::sessions::claude::is_session_dir_id(&continued.id));

        assert_eq!(std::fs::read(&original).expect("the original"), before, "the original is untouched");
        let copy = crate::sessions::claude::find_in_root(&paths, ClaudeRoot::Managed, &continued.id)
            .await
            .expect("the copy is in this instance's root");
        let text = std::fs::read_to_string(&copy).expect("copy");
        assert!(
            text.contains(&format!("\"sessionId\":\"{}\"", continued.id)) && !text.contains(STUCK),
            "{text}"
        );
        assert!(copy
            .parent()
            .expect("project")
            .join(&continued.id)
            .join("subagents")
            .join("agent-1.jsonl")
            .is_file());
        assert_eq!(
            state
                .drive
                .accounts
                .session_account_id_for(&continued.id, AccountProvider::Claude)
                .await
                .as_deref(),
            Some(work.id.as_str()),
            "bound to the account, so its first message resumes there"
        );

        // Both list, each as what it is.
        let new_row = claude::load_claude_summary(&paths, &continued.id).await.expect("the new session lists");
        assert_eq!(
            (new_row.scope, new_row.claude_root),
            (crate::sessions::SessionScope::Owned, Some(ClaudeRoot::Managed))
        );
        assert_eq!(new_row.title, "fix the inbox", "the same conversation");
        let old_row = claude::load_claude_summary(&paths, STUCK).await.expect("the original still lists");
        assert_eq!(
            (old_row.scope, old_row.claude_root),
            (crate::sessions::SessionScope::Owned, Some(ClaudeRoot::Local))
        );

        // The copy is in the managed root: it switches accounts in place, and
        // continuing it again is refused.
        let again = continue_on(&state, Source::Claude, &continued.id, &work.id)
            .await
            .expect_err("already managed");
        assert_eq!(
            (again.status().as_u16(), again.body()["error"].as_str()),
            (409, Some("session_not_on_local_login"))
        );
    }

    /// Only this instance's own local-login sessions, only claude, only a real
    /// claude account — and nothing written on any refusal.
    #[tokio::test]
    async fn continuing_refuses_what_is_not_this_servers_to_copy() {
        let _guard = crate::db::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let paths = state.drive.paths.clone();
        let work = claude_account(&state, "work").await;
        let codex = state
            .drive
            .accounts
            .create(
                "codex",
                AccountProvider::Codex,
                Credential::Env(store::parse_env_block("OPENAI_API_KEY=sk-test-abcdefghijklmnop").expect("env block")),
            )
            .await
            .expect("codex account");

        // The user's own session, started in a terminal: external, browse-only.
        let theirs = "33333333-3333-4333-8333-333333333333";
        local_session(&paths, theirs, false).await;
        let error = continue_on(&state, Source::Claude, theirs, &work.id).await.expect_err("external");
        assert_eq!((error.status().as_u16(), error.body()["error"].as_str()), (409, Some("external_session")));

        local_session(&paths, STUCK, true).await;
        let error = continue_on(&state, Source::Codex, STUCK, &work.id).await.expect_err("codex");
        assert_eq!(
            (error.status().as_u16(), error.body()["error"].as_str()),
            (400, Some("continue_not_supported"))
        );
        let error = continue_on(&state, Source::Claude, STUCK, &codex.id)
            .await
            .expect_err("the other CLI's account");
        assert_eq!(
            (error.status().as_u16(), error.body()["error"].as_str()),
            (400, Some("provider_mismatch"))
        );
        for account in ["not-a-uuid", "99999999-9999-4999-8999-999999999999"] {
            let error = continue_on(&state, Source::Claude, STUCK, account).await.expect_err("no such account");
            assert_eq!(error.status().as_u16(), 404, "{account}");
        }
        for id in ["44444444-4444-4444-8444-444444444444", "..", "../../etc/passwd", "-w/../x"] {
            let error = continue_on(&state, Source::Claude, id, &work.id).await.expect_err("no such session");
            assert_eq!(error.status().as_u16(), 404, "{id}");
        }
        assert!(
            !crate::claude_roots::projects_dir(&paths, ClaudeRoot::Managed).join("-w").exists(),
            "not one copy was written"
        );
    }

    /// `GET /api/accounts` says what the local login is worth, so the picker can
    /// stop offering a broken one as the default.
    #[tokio::test]
    async fn the_account_list_reports_the_local_logins_state() {
        use crate::accounts::local_login::LocalLoginState;
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let listed = list_accounts(State(state.clone())).await.0;
        assert_eq!(listed.local.claude.state, LocalLoginState::Unknown, "nothing known yet");

        state
            .drive
            .accounts
            .limit_log()
            .record(
                &rest::Observation {
                    account_id: rest::LOCAL_CLAUDE.to_string(),
                    session_id: Some(STUCK.into()),
                    source: "claude:result",
                    model: None,
                    verdict: limits::Verdict {
                        class: limits::LimitClass::Auth,
                        scope: "all".into(),
                        until_ms: None,
                    },
                    raw: "Failed to authenticate: OAuth session expired and could not be refreshed".into(),
                    reason: String::new(),
                    may_rest: true,
                },
                rest::now_ms(),
            )
            .expect("record");
        let listed = list_accounts(State(state.clone())).await.0;
        assert_eq!(listed.local.claude.state, LocalLoginState::SignedOut);
        assert_eq!(listed.local.claude.rest.len(), 1);
        assert_eq!(listed.local.codex.state, LocalLoginState::Unknown);
        let wire = serde_json::to_value(&listed).expect("json");
        assert_eq!(wire["local"]["claude"]["state"], "signed_out", "spelled as the client reads it");
    }
}
