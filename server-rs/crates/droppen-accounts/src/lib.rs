//! Several Claude logins, one managed config root.
//!
//! Port of the account routes in the former TypeScript server's `app.ts`. The
//! routes that drive `claude` against one account live with Claude Code's
//! code in the server and register themselves, because only that CLI has them:
//! `droppen_provider_claude_code::setup_token` mints a token off `claude setup-token`'s own output,
//! and `droppen_provider_claude_code::usage_login` puts `claude auth login` on screen against an
//! account's probe directory and checks it with `claude auth status --json`.
//!
//! This crate names no CLI: what differs between them is asked of each one's
//! [`login::Login`], which the server implements, and the entry hands them in
//! as one [`login::Logins`]. A name in backticks such as `provider` in these docs
//! is a module of the server, which this crate cannot link to.

pub mod limits;
pub mod local_login;
pub mod login;
pub mod quota;
pub mod rest;
pub mod rotation;
pub mod store;
pub mod verify;

use std::sync::Arc;

use futures_util::StreamExt;

use gotcha::axum::extract::FromRef;
use gotcha::axum::http::StatusCode;
use gotcha::{api, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use droppen_http::created::Created;
use droppen_http::error::{ApiError, Hinted};
use droppen_http::extract::{ApiJson, ApiPath, ApiQuery};
use droppen_http::SendResult;
use droppen_http::openapi_ext::{required_nullable, ExtraResponses, Nullable, Optional};
use rest::AccountLimitEventList;
use store::{Account, AccountError, AccountKind, AccountList, AccountPatch, AccountProvider, Credential};

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
    /// The `KEY=VALUE` block, for `kind: env`. Returned only by
    /// `GET /api/accounts/{id}/env`.
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

/// `GET /api/accounts/{id}/env` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountEnvBlock {
    /// The account's whole block, one `KEY=VALUE` per line, as the editor
    /// shows it. Values included: this route exists to read them back.
    pub env: String,
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

/// The `409` body when a token is already stored.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountConflict {
    /// Always `duplicate_account`.
    pub error: String,
    /// The label the token is already filed under.
    pub existing_label: String,
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
pub fn account_not_found() -> ApiError {
    ApiError::not_found("account not found")
}

/// The `400` for an account that belongs to the other CLI.
///
/// A machine-readable token beside the sentence, like
/// `local_session_cannot_use_account`: the client picks the account list it
/// offers from the session's own CLI, so reaching this is a bug in the client
/// rather than something a person typed, and it should be able to say so.
pub fn provider_mismatch() -> ApiError {
    ApiError::refusal(
        StatusCode::BAD_REQUEST,
        Hinted {
            error: "provider_mismatch",
            hint: "this account authenticates the other CLI; a codex session needs a codex account and a claude session a claude one",
        },
    )
}

/// The checks an account route makes of what a request names before it
/// touches the store: an account id from the path, a label from the body.
pub trait AccountInputExt {
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
    fn require_uuid(&self) -> Result<(), ApiError>;

    /// `z.string().min(1).max(100)` on a label.
    fn require_label(&self) -> Result<(), ApiError>;
}

impl AccountInputExt for str {
    fn require_uuid(&self) -> Result<(), ApiError> {
        let id = self;
        if store::is_uuid(id) {
            Ok(())
        } else {
            Err(account_not_found())
        }
    }

    fn require_label(&self) -> Result<(), ApiError> {
        let label = self;
        match label.chars().count() {
            0 => Err(ApiError::bad_request("the label is empty")),
            n if n > 100 => Err(ApiError::bad_request("the label is too long")),
            _ => Ok(()),
        }
    }
}

/// Store errors become the statuses the TypeScript routes answer with.
///
/// Everything the TypeScript recognises as an `AccountError` is reported
/// verbatim; anything else is a plain `fs` failure it lets escape as a 500, so
/// [`AccountError::Io`] is the only variant that does not become a `4xx`.
fn store_error(error: AccountError, fallback: &str) -> ApiError {
    match error {
        AccountError::Duplicate { existing_label, .. } => ApiError::refusal(
            StatusCode::CONFLICT,
            DuplicateAccount {
                error: "duplicate_account",
                existing_label: &existing_label,
            },
        ),
        AccountError::Invalid(message) => ApiError::bad_request(message),
        AccountError::ProviderMismatch { .. } => provider_mismatch(),
        AccountError::NotFound(_) => ApiError::bad_request(fallback.to_string()),
        AccountError::Io(_) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, fallback.to_string()),
    }
}

/// `duplicate_account`'s body: the label of the account that already holds
/// the token, which the client shows.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateAccount<'a> {
    error: &'static str,
    existing_label: &'a str,
}

/// The model ids one login of a CLI lists: what a start that names no model
/// is chosen against. Only the CLI's provider can list them, and the providers
/// sit above accounts, so the entry hands an implementation in.
pub trait ModelIds: Send + Sync {
    /// The ids `GET /api/models` lists for one login of `source`, in its order;
    /// `account_id` absent is the user's own login.
    fn model_ids<'a>(&'a self, source: droppen_sessions::Source, account_id: Option<&'a str>) -> futures_util::future::BoxFuture<'a, Vec<String>>;
}

/// What the account routes use of the server's state.
#[derive(Clone)]
pub struct AccountsState {
    /// The stored accounts.
    pub accounts: Arc<store::AccountsStore>,
    /// The cached quota readings, which say where a start would run.
    pub quota: Arc<quota::QuotaCache>,
    /// The instance's paths, which a secret is verified under.
    pub paths: Arc<droppen_instance::paths::Paths>,
    /// The model ids of one login, for `GET /api/accounts/start`.
    pub model_ids: Arc<dyn ModelIds>,
    /// Re-checks the runs held for a usable account, after a write that may
    /// have made one usable. The runs sit above accounts, so the entry hands
    /// the call in.
    pub recheck_holds: Arc<dyn Fn() + Send + Sync>,
}
droppen_http::substate!(AccountsState);

/// Stored Claude accounts (secrets never leave the server).
#[api(id = "list_accounts", group = "accounts", summary = "List accounts")]
pub async fn list_accounts(State(state): State<AccountsState>) -> Json<AccountList> {
    Json(state.accounts.account_list().await)
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
///
/// A credential with nothing to check is the store's refusal, and gets the
/// store's body: a launch for the other CLI is the same `provider_mismatch`
/// here as at a spawn.
async fn require_working(state: &AccountsState, provider: AccountProvider, credential: &Credential) -> Result<Option<String>, ApiError> {
    let verdict = state.accounts.logins().verify_secret(&state.paths, provider, credential)
        .await
        .map_err(|error| store_error(error, "cannot check the credential"))?;
    match verdict {
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
        (AccountKind::Env, None, Some(env)) => droppen_base::env_block::parse_env_block(env)
            .map(Credential::Env)
            .map_err(|error| store_error(error.into(), "cannot read the environment block")),
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
pub async fn create_account(State(state): State<AccountsState>, ApiJson(body): ApiJson<AccountCreate>) -> Result<Created<Account>, ApiError> {
    body.label.require_label()?;
    let provider = body.provider.0.unwrap_or_default();
    // The shape is checked before a request is spent on the credential: a
    // combination that could never spawn is not worth a round trip to a vendor.
    store::validate_pairing(provider, body.kind).map_err(|error| store_error(error, "cannot store account"))?;
    let credential = credential_of(body.kind, body.secret.as_deref(), body.env.as_deref())?;
    let unchecked = require_working(&state, provider, &credential).await?;
    state
        .accounts
        .create_with(&body.label, provider, credential, body.rotation.0)
        .await
        .inspect(|_| {
            // A new account may be where a held session can go.
            (state.recheck_holds)();
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
    State(state): State<AccountsState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<AccountUpdate>,
) -> Result<Json<Account>, ApiError> {
    id.require_uuid()?;
    if let Some(label) = body.label.as_deref() {
        label.require_label()?;
    }
    let mut unchecked = None;
    let mut credential = None;
    if touches_credential(&body) {
        let Some(account) = state.accounts.get(&id).await else {
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
    let updated = state.accounts.update(&id, patch).await;
    if matches!(updated, Ok(Some(_))) {
        // A re-authorized account, or one just put in the pool, may be where a
        // held session can go — or the very account it is held against.
        (state.recheck_holds)();
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

/// The whole block of an env account, for editing it in place.
///
/// The account list carries only the variable names, so the settings page
/// cannot prefill its editor without this read. Kept apart from
/// `GET /api/accounts` on purpose: a credential should leave the server when
/// somebody asks for that account, not with every list a picker fetches.
#[api(id = "get_account_env", group = "accounts", summary = "Get an account's environment block")]
pub async fn get_account_env(State(state): State<AccountsState>, ApiPath(id): ApiPath<String>) -> Result<Json<AccountEnvBlock>, ApiError> {
    id.require_uuid()?;
    match state.accounts.env_block(&id).await {
        Ok(env) => Ok(Json(AccountEnvBlock { env })),
        Err(AccountError::NotFound(_)) => Err(account_not_found()),
        Err(error) => Err(store_error(error, "cannot read the environment block")),
    }
}

/// Delete an account, its session bindings and its probe login.
#[api(id = "delete_account", group = "accounts", summary = "Delete an account")]
pub async fn delete_account(State(state): State<AccountsState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    id.require_uuid()?;
    if !state
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
pub async fn set_default_account(State(state): State<AccountsState>, ApiJson(body): ApiJson<AccountDefault>) -> Result<Json<AccountList>, ApiError> {
    let account_id = body.account_id.0;
    let provider = body.provider.0.unwrap_or_default();
    if body.automatic.0 == Some(true) {
        if account_id.is_some() {
            return Err(ApiError::bad_request("automatic clears the default: it takes no accountId"));
        }
        state
            .accounts
            .clear_default(provider)
            .await
            .map_err(|e| store_error(e, "cannot clear the default account"))?;
        return Ok(Json(state.accounts.account_list().await));
    }
    if let Some(id) = account_id.as_deref() {
        id.require_uuid()?;
    }
    if !state
        .accounts
        .set_default(account_id.as_deref(), provider)
        .await
        .map_err(|e| store_error(e, "cannot set the default account"))?
    {
        return Err(account_not_found());
    }
    Ok(Json(state.accounts.account_list().await))
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
pub async fn account_start(State(state): State<AccountsState>, ApiQuery(query): ApiQuery<AccountStartQuery>) -> Json<rotation::AccountStart> {
    let provider = query.provider.unwrap_or_default();
    let model = query.model.filter(|model| !model.trim().is_empty());
    let inputs = rotation::StartInputs::gather(&state.accounts, &state.quota, provider).await;
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
            let models = state.model_ids.model_ids(source, login.as_deref()).await;
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

/// Resolve the `{id}` of a limit route: a stored account, or the machine's
/// own login under its `local:claude` / `local:codex` name.
async fn limit_subject(state: &AccountsState, id: &str) -> Result<(), ApiError> {
    if rest::is_local(id) {
        return Ok(());
    }
    id.require_uuid()?;
    match state.accounts.get(id).await {
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
pub async fn list_limit_events(State(state): State<AccountsState>, ApiQuery(query): ApiQuery<LimitEventsQuery>) -> Json<AccountLimitEventList> {
    let limit = query.limit.unwrap_or(100) as usize;
    Json(AccountLimitEventList {
        events: state.accounts.limit_log().events(None, limit),
    })
}

/// One account's limit events, newest first.
#[api(id = "list_account_limit_events_for", group = "accounts", summary = "List an account's limit events")]
pub async fn list_account_limit_events(
    State(state): State<AccountsState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<LimitEventsQuery>,
) -> Result<Json<AccountLimitEventList>, ApiError> {
    limit_subject(&state, &id).await?;
    let limit = query.limit.unwrap_or(50) as usize;
    Ok(Json(AccountLimitEventList {
        events: state.accounts.limit_log().events(Some(&id), limit),
    }))
}

/// End an account's rest — one scope, or all of them.
#[api(id = "clear_account_rest", group = "accounts", summary = "Clear an account's rest")]
pub async fn clear_account_rest(
    State(state): State<AccountsState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<RestClearQuery>,
) -> Result<Json<AccountList>, ApiError> {
    limit_subject(&state, &id).await?;
    let scope = query.scope.as_deref().map(str::trim).filter(|scope| !scope.is_empty());
    match state.accounts.limit_log().clear(&id, scope) {
        Ok(cleared) => tracing::info!(account = %id, scope = ?scope, cleared, "account rest cleared by hand"),
        Err(error) => {
            tracing::warn!(account = %id, %error, "cannot clear an account's rest");
            return Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "cannot clear the rest".to_string()));
        }
    }
    // Somebody says this account is usable again: a session held against it
    // lifts, and one held with nowhere to go may now have somewhere.
    (state.recheck_holds)();
    Ok(Json(state.accounts.account_list().await))
}

impl AccountsState {
    /// Register the account routes.
    pub fn routes<S>(router: GotchaRouter<S>) -> GotchaRouter<S>
    where
        S: Clone + Send + Sync + 'static,
        AccountsState: FromRef<S>,
    {
        router
            .get("/api/accounts", list_accounts)
            .get("/api/accounts/limit-events", list_limit_events)
            .get("/api/accounts/start", account_start)
            .get("/api/accounts/{id}/env", get_account_env)
            .get("/api/accounts/{id}/limit-events", list_account_limit_events)
            .delete("/api/accounts/{id}/rest", clear_account_rest)
            .post("/api/accounts", create_account)
            .patch("/api/accounts/{id}", update_account)
            .delete("/api/accounts/{id}", delete_account)
            .put("/api/accounts/default", set_default_account)
    }
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
            "/api/accounts/{id}/env",
            200,
            "The account's whole environment block, values included, for editing it",
        )
        .error("GET", "/api/accounts/{id}/env", 400, "The account has no environment block")
        .error("GET", "/api/accounts/{id}/env", 404, "Account not found")
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
        .error("PUT", "/api/accounts/default", 404, "Account not found");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model lists of no login: the id routes never ask for one.
    struct NoModels;

    impl ModelIds for NoModels {
        fn model_ids<'a>(&'a self, _source: droppen_sessions::Source, _account_id: Option<&'a str>) -> futures_util::future::BoxFuture<'a, Vec<String>> {
            Box::pin(async { Vec::new() })
        }
    }

    /// A route refuses an id that is not a uuid before it asks the store
    /// anything. The store would answer for such an id: here a stored
    /// account's row is renamed behind its back, so reading or deleting under
    /// the new name would find it.
    #[tokio::test]
    async fn a_route_asks_the_store_nothing_about_an_id_that_is_not_a_uuid() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(droppen_instance::paths::Paths::new(dir.path().to_path_buf()));
        let accounts = Arc::new(store::AccountsStore::new(paths.clone(), login::Logins::stand_ins()));
        let block = droppen_base::env_block::parse_env_block("ANTHROPIC_AUTH_TOKEN=t\nANTHROPIC_BASE_URL=https://relay.example").expect("block");
        let account = accounts.create("Relay", AccountProvider::Claude, Credential::Env(block)).await.expect("stored");
        let renamed = "not-a-uuid";
        let db = rusqlite::Connection::open(paths.config_db()).expect("the store's database");
        assert_eq!(db.execute("UPDATE accounts SET id = ?1 WHERE id = ?2", (renamed, &account.id)).expect("rename"), 1);
        let state = AccountsState {
            accounts,
            quota: Arc::new(quota::QuotaCache::new(paths.clone(), login::Logins::stand_ins())),
            paths,
            model_ids: Arc::new(NoModels),
            recheck_holds: Arc::new(|| {}),
        };

        let read = get_account_env(State(state.clone()), ApiPath(renamed.to_string())).await;
        assert_eq!(read.err().expect("refused").status(), StatusCode::NOT_FOUND, "the block is not read");
        let deleted = delete_account(State(state), ApiPath(renamed.to_string())).await;
        assert_eq!(deleted.err().expect("refused").status(), StatusCode::NOT_FOUND);
        let left: i64 = db.query_row("SELECT COUNT(*) FROM accounts WHERE id = ?1", (renamed,), |row| row.get(0)).expect("count");
        assert_eq!(left, 1, "the row is not deleted");
    }

    /// What an account route checks before it touches the store: an id that
    /// is not a uuid is the same `404` as one that names no account, and a
    /// label is 1 to 100 characters, counted as characters rather than bytes.
    #[test]
    fn a_route_takes_only_a_uuid_for_an_id_and_a_label_of_one_to_a_hundred_characters() {
        assert!("0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f".require_uuid().is_ok());
        for id in ["", "not-a-uuid", "../0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f", "0b6f7a7e-2f7a-4d8e-9d0c-1a2b3c4d5e6f/x"] {
            let error = id.require_uuid().expect_err(id);
            assert_eq!(error.status(), StatusCode::NOT_FOUND, "{id:?}");
            assert_eq!(error.body().to_string(), account_not_found().body().to_string(), "{id:?}");
        }

        assert!("W".require_label().is_ok());
        assert!("é".repeat(100).require_label().is_ok(), "100 characters, 200 bytes");
        for label in [String::new(), "x".repeat(101)] {
            let error = label.require_label().expect_err("refused");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{} characters", label.chars().count());
        }
    }

    /// The two refusals with a code of their own, byte for byte: the client
    /// acts on `error`, shows `hint`, and names `existingLabel`.
    #[test]
    fn the_account_refusals_are_written_as_pinned() {
        let mismatch = provider_mismatch();
        assert_eq!(mismatch.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            mismatch.body().to_string(),
            r#"{"error":"provider_mismatch","hint":"this account authenticates the other CLI; a codex session needs a codex account and a claude session a claude one"}"#
        );
        let duplicate = store_error(
            AccountError::Duplicate {
                existing_id: "a-1".into(),
                existing_label: "Work \"main\"".into(),
            },
            "unused",
        );
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        assert_eq!(duplicate.body().to_string(), r#"{"error":"duplicate_account","existingLabel":"Work \"main\""}"#);
    }

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
}
