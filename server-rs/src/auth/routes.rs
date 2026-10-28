//! The ten operations under `/api/auth/`.
//!
//! Four of them answer without a token — [`login`], [`setup`], [`status`] and
//! [`local_open`] — and are the only four [`super::guard`] lets past on these
//! paths. The rest need one, and say so in the spec through
//! `#[api(security = "bearerAuth")]`.
//!
//! Every refusal on the login path is the same refusal: `401` with
//! `{"error":"invalid credentials"}`, whether the account does not exist, the
//! password is wrong, or the account is locked. Distinguishing them would hand
//! an attacker a list of valid user names for free, and the lockout state on top
//! of it.

use std::time::SystemTime;

use gotcha::axum::extract::FromRequestParts;
use gotcha::axum::http::request::Parts;
use gotcha::axum::http::{header, StatusCode};
use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{ApiRouter, AppState, SendResult};
use crate::openapi_ext::{ExtraResponses, Nullable, Optional};
use crate::sessions::iso8601;

use super::setup::Refused;
use super::tickets::TicketScope;
use super::users::{self, UserError, Verdict};
use super::{is_local_owner, Identity, MaybeIdentity, LOCAL_OWNER};

/// The one route that mints a token, named here so [`super::exempt`] and the
/// registration below cannot drift apart.
pub const LOGIN_PATH: &str = "/api/auth/login";
/// The one route that answers whether there is anything to log in to.
pub const STATUS_PATH: &str = "/api/auth/status";
/// The route that makes the first account with the setup code
/// ([`super::setup`]).
pub const SETUP_PATH: &str = "/api/auth/setup";
/// The route that turns a local-open token into a session
/// ([`super::local_open`]).
pub const LOCAL_OPEN_PATH: &str = "/api/auth/local-open";

/// Where the desktop app asks for a local-open token with its own login.
pub const LOCAL_OPEN_GRANT_PATH: &str = "/api/auth/local-open/grant";

/// The security scheme name the guarded operations reference, and the key it is
/// registered under in `components/securitySchemes`.
pub const BEARER_SCHEME: &str = "bearerAuth";

/// Longest client label kept from a `User-Agent`.
const MAX_LABEL: usize = 120;

/// What every refusal on the login path says.
fn invalid_credentials() -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "invalid credentials")
}

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

/// Body of `POST /api/auth/login`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    /// The account name.
    pub username: String,
    /// The password, in the clear — this connection is expected to be loopback,
    /// a tunnel, or TLS terminated in front of it.
    pub password: String,
}

/// What a successful login hands back.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LoginResult {
    /// The bearer token. **The only copy**: the server keeps a sha256 of it and
    /// cannot reproduce this string, so a client that loses it logs in again.
    pub token: String,
    /// When it stops working, ISO-8601. Absolute; using it does not extend it.
    pub expires_at: String,
    /// Who it belongs to.
    pub username: String,
}

/// What `GET /api/auth/status` answers, logged in or not.
///
/// Answered to anyone, so it says nothing but these few things — all of which
/// are either about the caller or visible from the way every other route
/// answers them anyway.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    /// Whether a caller has to sign in: `true` unless authentication is
    /// switched off. Nobody is let in for where they connect from.
    pub auth_required: bool,
    /// Whether this device has no user account yet (the desktop app's own
    /// account does not count). A caller with no session then gets the setup
    /// page: open the page from the device (`remote-hand-server open`, or the
    /// app), or make the first account with the one-time setup code.
    pub setup_required: bool,
    /// Whether authentication is switched off (`auth.enabled = false`), so
    /// that every route answers anyone who can reach the port.
    pub auth_off: bool,
    /// Who the request's token says it is, or `null`: an account name,
    /// `local:owner` for a session opened with a local-open token, or
    /// `cloud:<device>` for a device bridged in through Droppen.
    pub user: Nullable<String>,
    /// Whether the Droppen desktop app runs this server and owns its login
    /// (`crate::desktop_app`): the web app then shows no password change, no
    /// sign-out and no sign-in to Droppen through a browser popup.
    pub desktop_app: bool,
    /// How a person on this device runs this server's binary, for the commands
    /// the web app asks them to type (`<droppenPath> user add <name>`): the
    /// `droppen` link beside the running binary if there is one, else the
    /// binary itself, with the home directory as `~` (`crate::program`). A
    /// path, not yet quoted for a shell. Servers older than this field send
    /// none; the web app then says `remote-hand-server`.
    pub droppen_path: String,
    /// What those commands need in their environment to act on this server's
    /// instance, to put before `droppenPath` (`APP_INSTANCE=bob …/droppen
    /// setup-code`): the instance when it is not `default`, and where this
    /// server keeps its files when that is not the usual place
    /// (`crate::program::droppen_env`). Empty for the usual install; values
    /// not yet quoted for a shell. Servers older than this field send none.
    pub droppen_env: Vec<CommandEnv>,
}

/// One environment variable of a command the web app shows.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CommandEnv {
    /// Its name, e.g. `APP_INSTANCE`.
    pub name: String,
    /// Its value.
    pub value: String,
}

/// What `POST /api/auth/local-open/grant` answers the desktop app.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LocalOpenGrant {
    /// A one-time local-open token for this server's page
    /// (`http://127.0.0.1:<port>/#local-open=<token>`), good once within a
    /// minute; `null` when the instance has a user account, whose login page
    /// is the one to open.
    pub token: Nullable<String>,
}

/// Body of `POST /api/auth/local-open`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LocalOpenRequest {
    /// The one-time token `remote-hand-server open` put in the page's address
    /// (after `#local-open=`). In the body, never in a URL.
    pub token: String,
}

/// One live login, as the account panel lists it.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
    /// Its id, which is `sha256(token)` — the same value the server stores.
    /// Knowing it is not knowing the token, and cannot be turned into one.
    pub id: String,
    /// Which account it belongs to.
    pub username: String,
    /// When it was created, ISO-8601.
    pub created_at: String,
    /// When it stops working, ISO-8601.
    pub expires_at: String,
    /// Last time it was used, ISO-8601, to within five minutes.
    pub last_seen_at: String,
    /// What the client called itself when it logged in, or `null`.
    pub label: Nullable<String>,
    /// Whether this is the session making the request.
    pub current: bool,
}

/// What `GET /api/auth/sessions` answers.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AuthSessionList {
    /// The caller's own live sessions, oldest first.
    pub sessions: Vec<AuthSession>,
}

/// Body of `POST /api/auth/ticket`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct TicketRequest {
    /// What the ticket is for. `ws` is single-use and lives a minute; `download`
    /// is reusable and lives a minute too.
    pub scope: TicketScope,
}

/// A minted ticket.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct TicketResult {
    /// The ticket, to be sent as `?ticket=…` on a path its scope covers.
    pub ticket: String,
    /// When it stops working, ISO-8601.
    pub expires_at: String,
}

/// Body of `POST /api/auth/setup`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequest {
    /// The one-time setup code the server printed at startup. Case, spaces
    /// and dashes do not matter. Not needed from a local-owner session, which
    /// has already proved it is this device's owner.
    pub code: Optional<String>,
    /// The first account's name: 1-32 characters of a-z, 0-9, '.', '_' or '-'.
    pub username: String,
    /// Its password, under the rules `user add` applies.
    pub password: String,
}

/// Body of `POST /api/auth/password`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct PasswordChange {
    /// The current password. Required even though the caller is authenticated:
    /// a token left behind on an unlocked laptop must not be enough to take the
    /// account over.
    pub current_password: String,
    /// The replacement, which has to satisfy the same rules `user add` applies.
    pub new_password: String,
}

/// The requesting client's `User-Agent`, trimmed to something list-sized.
///
/// A hand-written extractor rather than `HeaderMap`, for the same reason
/// [`Identity`] is one: it carries no OpenAPI contribution, so its parameter is
/// annotated `#[api(skip)]` and gotcha never asks it for one.
pub struct ClientLabel(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for ClientLabel {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let label = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(MAX_LABEL).collect::<String>());
        Ok(ClientLabel(label))
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// The address a request came from, when the listener recorded one.
///
/// The socket's, never a forwarded header: [`setup`] rate-limits by it, and a
/// header is something a guesser could set to a new value on every request.
pub struct Peer(pub Option<std::net::IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<gotcha::axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0.ip());
        Ok(Peer(peer))
    }
}

/// A refusal with a machine-readable `error` and a sentence for the person.
fn refused(status: StatusCode, code: &str, hint: &str) -> ApiError {
    ApiError::with_body(status, serde_json::json!({ "error": code, "hint": hint }))
}

/// Exchange a name and a password for a bearer token.
#[api(id = "auth_login", group = "auth", summary = "Log in")]
pub async fn login(
    State(state): State<AppState>,
    #[api(skip)] ClientLabel(label): ClientLabel,
    ApiJson(body): ApiJson<LoginRequest>,
) -> Result<Json<LoginResult>, ApiError> {
    let auth = &state.drive.auth;
    let name = body.username.as_str();

    // One login attempt at a time, for the whole of it.
    //
    // The lockout is a check-then-act — ask whether the account is locked, check
    // the password, record what the answer turned out to be — and the middle
    // step is an argon2id verification, i.e. ~100 ms. Without this gate every
    // request that arrives inside that window passes a check that none of them
    // has yet had the chance to fail, so twenty concurrent guesses got twenty
    // attempts against an allowance of five: the allowance was the attacker's
    // connection count rather than the policy's number.
    //
    // Serializing costs no throughput, because logins are never concurrent past
    // this point anyway: `UsersStore::verify` holds its own lock across the hash
    // so that N logins cannot allocate N × 64 MiB. What it does cost is that
    // simultaneous logins queue — the right answer, and the reason this is a
    // gate rather than counting the attempt up front: counting would make six
    // *correct* simultaneous logins lock the account.
    let _attempt = auth.login_gate.lock().await;

    // A name that could not be an account and a name that is not one are the
    // same answer, at the same cost: `burn_a_verification` spends one argon2id
    // hash so that "no such user" does not come back faster than "wrong
    // password". `verify` burns its own for a name that is merely absent.
    if !users::valid_name(name) || auth.lockouts.locked(name) {
        auth.users.burn_a_verification().await;
        return Err(invalid_credentials());
    }

    if auth.users.verify(name, &body.password).await == Verdict::No {
        // Only a real account is counted. A map keyed on whatever an attacker
        // types is a map an attacker can grow, and locking out an account that
        // does not exist protects nothing.
        if auth.users.exists(name).await {
            auth.lockouts.record_failure(name, auth.policy());
        }
        return Err(invalid_credentials());
    }

    auth.lockouts.record_success(name);
    let issued = auth
        .sessions
        .issue(name, label)
        .await
        .map_err(|error| ApiError::internal(format!("could not start a session: {error}")))?;
    tracing::info!(user = name, "login");
    Ok(Json(LoginResult {
        token: issued.token,
        expires_at: issued.expires_at,
        username: body.username,
    }))
}

/// Revoke the token this request arrived with.
#[api(id = "auth_logout", group = "auth", summary = "Log out", security = "bearerAuth")]
pub async fn logout(State(state): State<AppState>, #[api(skip)] Identity(identity): Identity) -> Json<SendResult> {
    if let Some(digest) = identity.token_sha256.as_deref() {
        state.drive.auth.sessions.revoke(digest).await;
    }
    Json(SendResult { accepted: true })
}

/// Whether this server requires a login, and who the caller is.
///
/// The one route besides [`login`] that answers without a token, because the web
/// app has to ask it before it knows whether to draw a login page.
#[api(id = "auth_status", group = "auth", summary = "Authentication status")]
pub async fn status(State(state): State<AppState>, #[api(skip)] MaybeIdentity(identity): MaybeIdentity) -> Json<AuthStatus> {
    let auth = &state.drive.auth;
    Json(AuthStatus {
        auth_required: !auth.is_off(),
        setup_required: auth.setup_pending().await,
        auth_off: auth.is_off(),
        user: Nullable(identity.map(|identity| identity.username)),
        desktop_app: state.desktop_app,
        droppen_path: crate::program::droppen_path(),
        droppen_env: crate::program::droppen_env(&state.paths)
            .into_iter()
            .map(|(name, value)| CommandEnv { name, value })
            .collect(),
    })
}

/// Exchange a local-open token for a session of this device's owner.
///
/// The token came over the control socket only the OS user running this server
/// can reach ([`super::local_open`]), so whoever holds it is that user — the
/// one the AI CLIs run as. Works once, for a minute, and only while the device
/// has no user account: once it has one, logging in is the way in.
#[api(id = "auth_local_open", group = "auth", summary = "Open the page as this device's owner")]
pub async fn local_open(
    State(state): State<AppState>,
    #[api(skip)] MaybeIdentity(identity): MaybeIdentity,
    #[api(skip)] ClientLabel(label): ClientLabel,
    ApiJson(body): ApiJson<LocalOpenRequest>,
) -> Result<Json<LoginResult>, ApiError> {
    let auth = &state.drive.auth;
    if identity.as_ref().is_some_and(|who| who.username.starts_with("cloud:")) {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "not_from_cloud",
            "a page is opened as this device's owner on the device itself, not through Droppen",
        ));
    }
    if auth.is_off() {
        return Err(refused(
            StatusCode::CONFLICT,
            "auth_off",
            "local authentication is off on this device (auth.enabled = false): there is nothing to open",
        ));
    }
    let accounts_exist = || refused(StatusCode::CONFLICT, "accounts_exist", "this device has an account: sign in with it");
    // Under the setup gate, as the first account is made under it: an owner
    // session and the first account cannot be made across each other here.
    let _gate = auth.setup.gate.lock().await;
    if auth.users.has_users().await {
        // Spent either way, so it cannot be kept for a later moment with no
        // account.
        auth.local_open.redeem(&body.token);
        return Err(accounts_exist());
    }
    if !auth.local_open.redeem(&body.token) {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "invalid_local_open",
            "that link was already used or has expired: run remote-hand-server open on this device again",
        ));
    }
    let issued = auth
        .sessions
        .issue(LOCAL_OWNER, label)
        .await
        .map_err(|error| ApiError::internal(format!("could not start a session: {error}")))?;
    // An account made meanwhile from the shell (`user add`, which no gate in
    // this process holds back): this session must not outlive the moment.
    if auth.users.has_users().await {
        auth.sessions.revoke(&super::sessions::digest_of(&issued.token)).await;
        return Err(accounts_exist());
    }
    tracing::info!("the page was opened as this device's owner");
    Ok(Json(LoginResult {
        token: issued.token,
        expires_at: issued.expires_at,
        username: LOCAL_OWNER.to_string(),
    }))
}

/// A local-open token for the desktop app's *Open in Browser*, asked for with
/// the app's own login.
///
/// On the desktop app's instance the control socket opens nothing (any
/// process of the user could ask it, and that instance never leaves its setup
/// state: [`super::local_open`], "Not on the desktop app's instance"). The app
/// asks here instead, signed in as its own app-internal account (the password
/// in its keychain item), and hands the browser the page with the token, which
/// the page trades for a session of this device's owner
/// ([`local_open`]). Only on a server the desktop app runs, and only for an
/// app-internal account; `null` once the instance has a user account.
#[api(
    id = "auth_local_open_grant",
    group = "auth",
    summary = "A one-time page link for the desktop app",
    security = "bearerAuth"
)]
pub async fn local_open_grant(State(state): State<AppState>, #[api(skip)] Identity(identity): Identity) -> Result<Json<LocalOpenGrant>, ApiError> {
    let auth = &state.drive.auth;
    if !state.desktop_app {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "not_desktop_app",
            "only the Droppen app's server hands out page links over HTTP; on this device run remote-hand-server open",
        ));
    }
    if !auth.users.is_app(&identity.username).await {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "not_app_account",
            "only the Droppen app's own login asks for a page link",
        ));
    }
    if auth.is_off() {
        return Err(refused(
            StatusCode::CONFLICT,
            "auth_off",
            "local authentication is off on this device (auth.enabled = false): there is nothing to open",
        ));
    }
    if auth.users.has_users().await {
        return Ok(Json(LocalOpenGrant { token: Nullable(None) }));
    }
    let token = auth.local_open.mint().ok_or_else(|| {
        refused(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_links",
            "too many unused open links: wait a minute and try again",
        )
    })?;
    tracing::info!("a page link for the Droppen app's Open in Browser");
    Ok(Json(LocalOpenGrant { token: Nullable(Some(token)) }))
}

/// Create the first account, and sign in as it.
///
/// Answers only in the setup state. A local-owner session ([`LOCAL_OWNER`])
/// needs nothing more — it has proved it is this device's owner. Anyone else
/// needs the code the server printed: it works once, and every wrong code is
/// counted against the caller's address and against the server as a whole
/// (see [`super::setup`]). What it hands back is exactly what a login does, so
/// the page that made the account is signed in to it; every local-owner session
/// ends with it.
#[api(id = "auth_setup", group = "auth", summary = "Create the first account with the setup code")]
pub async fn setup(
    State(state): State<AppState>,
    #[api(skip)] MaybeIdentity(identity): MaybeIdentity,
    #[api(skip)] Peer(peer): Peer,
    #[api(skip)] ClientLabel(label): ClientLabel,
    ApiJson(body): ApiJson<SetupRequest>,
) -> Result<Json<LoginResult>, ApiError> {
    let auth = &state.drive.auth;
    if identity.as_ref().is_some_and(|who| who.username.starts_with("cloud:")) {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "not_from_cloud",
            "the first account is made on this device's own page, not through Droppen",
        ));
    }
    if auth.is_off() {
        return Err(refused(
            StatusCode::CONFLICT,
            "auth_off",
            "local authentication is off on this device (auth.enabled = false); add accounts with remote-hand-server user add",
        ));
    }
    if !auth.in_setup().await {
        return Err(refused(
            StatusCode::CONFLICT,
            "setup_done",
            "this device already has an account: sign in instead",
        ));
    }
    // The rules first, before the code is looked at: a right code with a
    // password that is too short must not be spent on an account that cannot
    // be made, and the rules are no secret.
    let name = body.username.as_str();
    if !users::valid_name(name) {
        let rule = format!("a user name is 1-{} characters of a-z, 0-9, '.', '_' or '-'", users::MAX_USERNAME);
        return Err(refused(StatusCode::BAD_REQUEST, "invalid_account", &rule));
    }
    users::check_password(name, &body.password).map_err(|error| refused(StatusCode::BAD_REQUEST, "invalid_account", &error.to_string()))?;

    let owner = identity.as_ref().is_some_and(|who| is_local_owner(&who.username));
    let code = body.code.0.clone().unwrap_or_default();
    if !owner && code.trim().is_empty() {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            "code_required",
            "the one-time setup code is needed: the server printed it when it started, and remote-hand-server setup-code shows it again",
        ));
    }

    // One attempt at a time, from the check of the code to the session: the
    // counters cannot be outrun by concurrency, and two right guesses — there
    // is one code — cannot both make an account.
    let _attempt = auth.setup.gate.lock().await;
    if auth.users.has_users().await {
        return Err(refused(
            StatusCode::CONFLICT,
            "setup_done",
            "this device already has an account: sign in instead",
        ));
    }
    let checked = if owner { Ok(()) } else { auth.setup.attempt(peer, &code) };
    match checked {
        Ok(()) => {}
        Err(Refused::TooMany(wait)) => {
            let seconds = wait.as_secs().max(1);
            return Err(ApiError::with_body(
                StatusCode::TOO_MANY_REQUESTS,
                serde_json::json!({
                    "error": "too_many_attempts",
                    "hint": format!("too many wrong setup codes: try again in {seconds} s, or create the account on the device with remote-hand-server user add"),
                    "retryAfterSeconds": seconds,
                }),
            ));
        }
        Err(Refused::NoCode) => {
            return Err(refused(
                StatusCode::CONFLICT,
                "no_setup_code",
                "this server has no setup code; restart the device's server to get one, or create the account there with remote-hand-server user add",
            ));
        }
        Err(Refused::Wrong) => {
            tracing::warn!(peer = ?peer, "a wrong setup code");
            return Err(refused(
                StatusCode::FORBIDDEN,
                "wrong_setup_code",
                "that is not the setup code this server printed when it started",
            ));
        }
    }
    // From here until the account exists, the code is being spent: any way
    // this ends without an account — a refusal below, an error, the request
    // dropped with its connection — puts it back, so the person holding it is
    // not left with nothing to type. Nothing was minted in its place
    // meanwhile (`Setup::attempt`).
    let give_back = GiveBack((!owner).then_some(&auth.setup));
    // The desktop app's own account holds its name; it does not hold the
    // instance, so this is "pick another name", not "already set up" — and it
    // is asked only of somebody who has got this far, so the setup page tells
    // a stranger nothing about which names exist.
    if auth.users.exists(name).await {
        return Err(name_taken());
    }
    if let Err(error) = auth.users.add(name, &body.password).await {
        return Err(match error {
            UserError::Invalid(message) => refused(StatusCode::BAD_REQUEST, "invalid_account", &message),
            UserError::Duplicate(_) => name_taken(),
            other => ApiError::internal(format!("could not create the account: {other}")),
        });
    }
    auth.setup.spent();
    give_back.done();
    // From now on this device has an account, and logging in is the way in:
    // the owner's local-open sessions — this one included — end here, and the
    // code the server printed with them.
    let ended = auth.sessions.revoke_user(LOCAL_OWNER, None).await;
    auth.in_setup().await;
    // A Droppen link the owner signed in is this account's now.
    state.cloud.hand_over_from_local_owner(&auth.users).await;
    if owner {
        tracing::info!(user = name, ended, "the first account was created by this device's owner");
    } else {
        tracing::info!(user = name, peer = ?peer, ended, "the first account was created with the setup code");
    }
    let issued = auth
        .sessions
        .issue(name, label)
        .await
        .map_err(|error| ApiError::internal(format!("the account exists, but could not start a session: {error}")))?;
    Ok(Json(LoginResult {
        token: issued.token,
        expires_at: issued.expires_at,
        username: body.username,
    }))
}

/// Puts back the setup code an attempt let through ([`super::setup::Setup::restore`])
/// when it goes out of scope before [`GiveBack::done`] — however the request
/// ended. `None` for this device's owner, who spent no code.
struct GiveBack<'a>(Option<&'a super::setup::Setup>);

impl GiveBack<'_> {
    /// The account exists: nothing to give back.
    fn done(mut self) {
        self.0 = None;
    }
}

impl Drop for GiveBack<'_> {
    fn drop(&mut self) {
        if let Some(setup) = self.0 {
            setup.restore();
        }
    }
}

/// The refusal for a name the desktop app's own account already has.
fn name_taken() -> ApiError {
    refused(
        StatusCode::CONFLICT,
        "name_taken",
        "that name belongs to the Droppen app's own account on this device: pick another",
    )
}

/// The caller's own live sessions.
#[api(id = "auth_sessions", group = "auth", summary = "List login sessions", security = "bearerAuth")]
pub async fn list_sessions(State(state): State<AppState>, #[api(skip)] Identity(identity): Identity) -> Json<AuthSessionList> {
    let current = identity.token_sha256.as_deref();
    let sessions = state
        .drive
        .auth
        .sessions
        .list()
        .await
        .into_iter()
        .filter(|session| session.username == identity.username)
        .map(|session| AuthSession {
            current: Some(session.token_sha256.as_str()) == current,
            id: session.token_sha256,
            username: session.username,
            created_at: session.created_at,
            expires_at: session.expires_at,
            last_seen_at: session.last_seen_at,
            label: Nullable(session.label),
        })
        .collect();
    Json(AuthSessionList { sessions })
}

/// Revoke one of the caller's sessions — the phone that was left on a train.
#[api(id = "auth_revoke_session", group = "auth", summary = "Revoke a login session", security = "bearerAuth")]
pub async fn revoke_session(
    State(state): State<AppState>,
    #[api(skip)] Identity(identity): Identity,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<SendResult>, ApiError> {
    // Somebody else's session id is a `404`, not a `403`: whether an id exists is
    // not the caller's business either way.
    let mine = state
        .drive
        .auth
        .sessions
        .list()
        .await
        .into_iter()
        .any(|session| session.token_sha256 == id && session.username == identity.username);
    if !mine {
        return Err(ApiError::not_found("session not found"));
    }
    state.drive.auth.sessions.revoke(&id).await;
    Ok(Json(SendResult { accepted: true }))
}

/// Mint a scoped, short-lived credential for a URL that cannot carry a header.
#[api(
    id = "auth_ticket",
    group = "auth",
    summary = "Mint a websocket or download ticket",
    security = "bearerAuth"
)]
pub async fn issue_ticket(
    State(state): State<AppState>,
    #[api(skip)] Identity(identity): Identity,
    ApiJson(body): ApiJson<TicketRequest>,
) -> Result<Json<TicketResult>, ApiError> {
    // A ticket is bound to the login that mints it, so only a login can: a
    // device bridged in through Droppen reaches every path without one.
    let Some(session) = identity.token_sha256.as_deref() else {
        return Err(refused(StatusCode::FORBIDDEN, "no_session", "tickets are minted by a signed-in session"));
    };
    let (ticket, _) = state
        .drive
        .auth
        .tickets
        .issue(&identity.username, session, body.scope)
        .ok_or_else(|| ApiError::new(StatusCode::TOO_MANY_REQUESTS, "too many live tickets"))?;
    Ok(Json(TicketResult {
        ticket,
        expires_at: iso8601(SystemTime::now() + body.scope.ttl()),
    }))
}

/// Change the caller's password, and log every other device out.
#[api(id = "auth_change_password", group = "auth", summary = "Change password", security = "bearerAuth")]
pub async fn change_password(
    State(state): State<AppState>,
    #[api(skip)] Identity(identity): Identity,
    ApiJson(body): ApiJson<PasswordChange>,
) -> Result<Json<SendResult>, ApiError> {
    let auth = &state.drive.auth;
    if auth.users.is_app(&identity.username).await {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "app_account",
            "this is the Droppen app's own account: the app keeps its password",
        ));
    }
    if auth.users.verify(&identity.username, &body.current_password).await == Verdict::No {
        return Err(invalid_credentials());
    }
    auth.users
        .set_password(&identity.username, &body.new_password)
        .await
        .map_err(|error| match error {
            UserError::Invalid(message) => ApiError::bad_request(message),
            UserError::NotFound(name) => ApiError::not_found(format!("no such user: {name}")),
            other => ApiError::internal(other.to_string()),
        })?;
    // Every *other* device, not this one: the point is to evict whoever else has
    // a token, and logging the person out of the browser they just typed their
    // password into would only teach them to avoid doing it.
    let revoked = auth.sessions.revoke_user(&identity.username, identity.token_sha256.as_deref()).await;
    tracing::info!(user = %identity.username, revoked, "password changed");
    Ok(Json(SendResult { accepted: true }))
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .post(LOGIN_PATH, login)
        .post("/api/auth/logout", logout)
        .get(STATUS_PATH, status)
        .post(SETUP_PATH, setup)
        .post(LOCAL_OPEN_PATH, local_open)
        .post(LOCAL_OPEN_GRANT_PATH, local_open_grant)
        .get("/api/auth/sessions", list_sessions)
        .delete("/api/auth/sessions/{id}", revoke_session)
        .post("/api/auth/ticket", issue_ticket)
        .post("/api/auth/password", change_password)
}

/// Everything gotcha cannot derive about those operations.
pub fn describe(reg: &mut ExtraResponses) {
    reg.body::<LoginResult>("POST", LOGIN_PATH, 200, "A bearer token and when it expires")
        .error("POST", LOGIN_PATH, 401, "Invalid credentials — wrong name, wrong password, or locked out")
        .describe("POST", "/api/auth/logout", 200, "The token this request carried is revoked")
        .describe(
            "GET",
            STATUS_PATH,
            200,
            "Whether a login is required, whether the first account is still to be made, and who the caller is",
        )
        .body::<LoginResult>("POST", SETUP_PATH, 200, "The first account is made, and this is a login to it")
        .error(
            "POST",
            SETUP_PATH,
            400,
            "The name or the password does not meet the rules (`invalid_account`), or no code from a caller that needs one (`code_required`)",
        )
        .error(
            "POST",
            SETUP_PATH,
            403,
            "Not the setup code (`wrong_setup_code`), or a device bridged through Droppen (`not_from_cloud`)",
        )
        .error(
            "POST",
            SETUP_PATH,
            409,
            "An account exists already (`setup_done`), the name is the desktop app's own account's (`name_taken`), authentication is off (`auth_off`), or this server has no code (`no_setup_code`)",
        )
        .error(
            "POST",
            SETUP_PATH,
            429,
            "Too many wrong codes; `retryAfterSeconds` says how long to wait (`too_many_attempts`)",
        )
        .body::<LoginResult>("POST", LOCAL_OPEN_PATH, 200, "A session of this device's owner (`local:owner`)")
        .error("POST", LOCAL_OPEN_PATH, 403, "Not a live local-open token (`invalid_local_open`), or a device bridged through Droppen (`not_from_cloud`)")
        .error("POST", LOCAL_OPEN_PATH, 409, "The device has an account: sign in (`accounts_exist`), or authentication is off (`auth_off`)")
        .body::<LocalOpenGrant>("POST", LOCAL_OPEN_GRANT_PATH, 200, "A one-time local-open token, or `null` once the instance has a user account")
        .error(
            "POST",
            LOCAL_OPEN_GRANT_PATH,
            403,
            "Not a server the desktop app runs (`not_desktop_app`), or not the app's own account (`not_app_account`)",
        )
        .error("POST", LOCAL_OPEN_GRANT_PATH, 409, "Authentication is off (`auth_off`)")
        .error("POST", LOCAL_OPEN_GRANT_PATH, 429, "Too many unused links (`too_many_links`)")
        .describe("GET", "/api/auth/sessions", 200, "The caller's own live sessions")
        .body::<SendResult>("DELETE", "/api/auth/sessions/{id}", 200, "The session is revoked")
        .error("DELETE", "/api/auth/sessions/{id}", 404, "No such session")
        .body::<TicketResult>("POST", "/api/auth/ticket", 200, "A scoped, short-lived ticket")
        .error("POST", "/api/auth/ticket", 403, "Not a signed-in session (`no_session`)")
        .error("POST", "/api/auth/ticket", 429, "Too many live tickets")
        .body::<SendResult>(
            "POST",
            "/api/auth/password",
            200,
            "The password is changed and other sessions are revoked",
        )
        .error("POST", "/api/auth/password", 400, "The new password does not meet the rules")
        .error(
            "POST",
            "/api/auth/password",
            403,
            "The Droppen app's own account, whose password the app keeps (`app_account`)",
        )
        .error("POST", "/api/auth/password", 401, "The current password is wrong");
}

#[cfg(test)]
mod tests {
    use super::*;
    use gotcha::axum::body::Body;
    use gotcha::axum::extract::ConnectInfo;
    use gotcha::axum::http::Request;
    use serde_json::{json, Value};
    use tower_service::Service;

    const PASSWORD: &str = "correct horse battery";

    /// Where a request comes from, as a socket and a `Host` show it. None of
    /// them is trusted for it: that is the point of most of these tests.
    #[derive(Clone, Copy, Debug)]
    enum From {
        /// A browser on this device: loopback socket, loopback `Host`.
        ThisDevice,
        /// A TCP relay on this device (`ssh -R`, frp, a LAN-shared proxy)
        /// carrying a stranger, who says `Host: localhost`.
        Relay,
        /// Another device on the network.
        Elsewhere,
    }

    const EVERYWHERE: [From; 3] = [From::ThisDevice, From::Relay, From::Elsewhere];

    /// One request through the router and its guard: `(status, headers, JSON body)`.
    async fn send(
        router: &mut gotcha::axum::Router,
        from: From,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> (u16, gotcha::axum::http::HeaderMap, Value) {
        let (peer, host) = match from {
            From::ThisDevice => ("127.0.0.1:50000", "127.0.0.1:8700"),
            From::Relay => ("127.0.0.1:50001", "localhost"),
            From::Elsewhere => ("192.168.1.9:50000", "192.168.1.5:8700"),
        };
        let mut request = Request::builder().method(method).uri(path).header("host", host);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        let mut request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json"))),
            None => request.body(Body::empty()),
        }
        .expect("request");
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<std::net::SocketAddr>().expect("peer")));
        let response = router.call(request).await.expect("infallible");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("body");
        (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn call(router: &mut gotcha::axum::Router, from: From, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
        let (status, _, body) = send(router, from, method, path, token, body, &[]).await;
        (status, body)
    }

    /// A router as the real server builds it, armed to mint setup codes as the
    /// server that owns its instance is.
    async fn server(auth_enabled: Option<bool>) -> (tempfile::TempDir, AppState, gotcha::axum::Router) {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, router) = crate::api::test_support::guarded_router(home.path(), auth_enabled).await;
        let auth = &state.drive.auth;
        auth.setup.arm_for(&state.paths, "http://127.0.0.1:1/".into());
        // What the owner does at boot.
        auth.in_setup().await;
        (home, state, router)
    }

    async fn status_of(router: &mut gotcha::axum::Router, from: From, token: Option<&str>) -> Value {
        let (status, body) = call(router, from, "GET", STATUS_PATH, token, None).await;
        assert_eq!(status, 200, "{body}");
        body
    }

    /// A local-open token exchanged for a session of the device's owner.
    async fn open_as_owner(router: &mut gotcha::axum::Router, state: &AppState) -> String {
        let token = state.drive.auth.local_open.mint().expect("a token");
        let (status, body) = call(router, From::ThisDevice, "POST", LOCAL_OPEN_PATH, None, Some(json!({"token": token}))).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["username"], LOCAL_OWNER);
        body["token"].as_str().expect("a session token").to_string()
    }

    async fn login_as(router: &mut gotcha::axum::Router, name: &str) -> String {
        let (status, body) = call(
            router,
            From::Elsewhere,
            "POST",
            LOGIN_PATH,
            None,
            Some(json!({"username": name, "password": PASSWORD})),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        body["token"].as_str().expect("token").to_string()
    }

    /// Nobody is let in for where they connect from — not a browser on this
    /// device, and not a stranger whose TCP relay makes them look like one.
    #[tokio::test]
    async fn without_a_session_nobody_gets_in_whatever_the_socket_and_host_say() {
        let (_home, state, mut router) = server(None).await;
        for from in EVERYWHERE {
            for (method, path) in [
                ("GET", "/api/settings"),
                ("GET", "/api/sessions"),
                ("GET", "/api/cloud"),
                ("POST", "/api/cloud/link"),
                ("GET", "/api/terminals"),
                ("GET", "/api/fs/read?path=%2Fetc%2Fhosts"),
                ("GET", "/api/auth/sessions"),
                ("POST", "/api/auth/ticket"),
                ("GET", "/openapi.json"),
            ] {
                assert_eq!(call(&mut router, from, method, path, None, None).await.0, 401, "{from:?} {method} {path}");
            }
            let mut body = status_of(&mut router, from, None).await;
            // Where this binary is, for the commands the page shows: the same
            // for everyone, and no secret.
            assert!(body["droppenPath"].as_str().is_some_and(|path| !path.is_empty()), "{body}");
            assert!(body["droppenEnv"].is_array(), "{body}");
            body.as_object_mut().expect("object").remove("droppenPath");
            body.as_object_mut().expect("object").remove("droppenEnv");
            assert_eq!(
                body,
                json!({"authRequired": true, "setupRequired": true, "authOff": false, "user": null, "desktopApp": false}),
                "{from:?}: the same answer for everyone, and nothing else"
            );
        }
        assert!(state.drive.auth.setup.live().is_some(), "a code for the first account");
    }

    /// The setup code makes the first account once and signs the caller in;
    /// after that the server is an ordinary one, this device included.
    #[tokio::test]
    async fn the_setup_code_makes_the_first_account_once() {
        let (_home, state, mut router) = server(None).await;
        let auth = state.drive.auth.clone();
        let file = super::super::setup::code_file(&state.paths);
        let code = auth.setup.live().expect("minted by the server that owns the instance");
        assert!(file.exists());

        let attempt = |code: Option<&str>, name: &str| match code {
            Some(code) => json!({"code": code, "username": name, "password": PASSWORD}),
            None => json!({"username": name, "password": PASSWORD}),
        };
        let (status, body) = call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(attempt(None, "ada"))).await;
        assert_eq!((status, body["error"].as_str()), (400, Some("code_required")), "{body}");
        let (status, body) = call(&mut router, From::ThisDevice, "POST", SETUP_PATH, None, Some(attempt(None, "ada"))).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (400, Some("code_required")),
            "this device needs it too: {body}"
        );
        let (status, body) = call(
            &mut router,
            From::Elsewhere,
            "POST",
            SETUP_PATH,
            None,
            Some(attempt(Some("ZZZZ-ZZZZ-ZZZZ-ZZZZ"), "ada")),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (403, Some("wrong_setup_code")), "{body}");
        let (status, body) = call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(attempt(Some(&code), "ada"))).await;
        assert_eq!(status, 200, "{body}");
        let token = body["token"].as_str().expect("token").to_string();
        assert!(!file.exists(), "spent");

        assert_eq!(
            call(&mut router, From::Elsewhere, "GET", "/api/settings", Some(&token), None).await.0,
            200
        );
        for from in EVERYWHERE {
            assert_eq!(call(&mut router, from, "GET", "/api/settings", None, None).await.0, 401, "{from:?}");
        }
        let body = status_of(&mut router, From::Elsewhere, Some(&token)).await;
        assert_eq!(body["setupRequired"], false);
        assert_eq!(body["user"], "ada");
        let (status, body) = call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(attempt(Some(&code), "eve"))).await;
        assert_eq!((status, body["error"].as_str()), (409, Some("setup_done")), "{body}");
    }

    /// A local-open token: once, within its minute, for a session of the
    /// device's owner — which is the owner of the Droppen link too.
    #[tokio::test]
    async fn a_local_open_token_signs_the_page_in_as_the_owner_once() {
        let (_home, state, mut router) = server(None).await;
        let (status, body) = call(
            &mut router,
            From::ThisDevice,
            "POST",
            LOCAL_OPEN_PATH,
            None,
            Some(json!({"token": "not-a-token"})),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (403, Some("invalid_local_open")), "{body}");

        let token = state.drive.auth.local_open.mint().expect("a token");
        let (status, body) = call(&mut router, From::Relay, "POST", LOCAL_OPEN_PATH, None, Some(json!({"token": token}))).await;
        assert_eq!(status, 200, "a token is a token wherever it comes from: {body}");
        let session = body["token"].as_str().unwrap().to_string();
        let (status, body) = call(
            &mut router,
            From::ThisDevice,
            "POST",
            LOCAL_OPEN_PATH,
            None,
            Some(json!({"token": token})),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (403, Some("invalid_local_open")), "single use: {body}");

        assert_eq!(
            call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&session), None).await.0,
            200
        );
        let body = status_of(&mut router, From::ThisDevice, Some(&session)).await;
        assert_eq!(body["user"], LOCAL_OWNER);
        assert_eq!(body["setupRequired"], true);
        let (status, cloud) = call(&mut router, From::ThisDevice, "GET", "/api/cloud", Some(&session), None).await;
        assert_eq!(status, 200, "{cloud}");
        assert_eq!(cloud["localAuth"], true, "{cloud}");
        assert_eq!(cloud["canManage"], true, "the owner manages the link: {cloud}");
    }

    /// The owner makes the first account without a code; with it every
    /// local-owner session ends, and logging in is the only way in.
    #[tokio::test]
    async fn the_owner_makes_the_first_account_without_a_code_and_its_sessions_end() {
        let (_home, state, mut router) = server(None).await;
        let owner = open_as_owner(&mut router, &state).await;
        let other = open_as_owner(&mut router, &state).await;
        let (status, body) = call(
            &mut router,
            From::ThisDevice,
            "POST",
            SETUP_PATH,
            Some(&owner),
            Some(json!({"username": "ada", "password": PASSWORD})),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let ada = body["token"].as_str().unwrap().to_string();
        assert_eq!(call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&ada), None).await.0, 200);
        for session in [&owner, &other] {
            assert_eq!(
                call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(session), None).await.0,
                401
            );
        }
        let late = state.drive.auth.local_open.mint().expect("a token");
        let (status, body) = call(&mut router, From::ThisDevice, "POST", LOCAL_OPEN_PATH, None, Some(json!({"token": late}))).await;
        assert_eq!((status, body["error"].as_str()), (409, Some("accounts_exist")), "{body}");
        assert!(!state.drive.auth.local_open.redeem(&late), "spent on the refusal");
    }

    /// An account made from the shell ends the owner's sessions just the same.
    #[tokio::test]
    async fn an_account_from_the_shell_ends_the_owners_sessions_too() {
        let (_home, state, mut router) = server(None).await;
        let owner = open_as_owner(&mut router, &state).await;
        state.drive.auth.users.add("ada", PASSWORD).await.expect("user add");
        assert_eq!(
            call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&owner), None).await.0,
            401
        );
        assert_eq!(status_of(&mut router, From::ThisDevice, Some(&owner)).await["user"], Value::Null);
    }

    /// The desktop app's own account signs its window in and is not a user
    /// account: the app's server is never open without a credential, and a
    /// user may not take the app account's name.
    #[tokio::test]
    async fn the_desktop_apps_own_account_opens_nothing_by_itself() {
        let (_home, state, mut router) = server(None).await;
        state.drive.auth.users.add_app("droppen-app", PASSWORD).await.expect("app account");
        for from in EVERYWHERE {
            assert_eq!(call(&mut router, from, "GET", "/api/settings", None, None).await.0, 401, "{from:?}");
            assert_eq!(status_of(&mut router, from, None).await["setupRequired"], true, "{from:?}");
        }
        let app = login_as(&mut router, "droppen-app").await;
        assert_eq!(call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&app), None).await.0, 200);
        let (status, body) = call(
            &mut router,
            From::ThisDevice,
            "POST",
            "/api/auth/password",
            Some(&app),
            Some(json!({"currentPassword": PASSWORD, "newPassword": "another long password"})),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (403, Some("app_account")), "{body}");

        let code = state.drive.auth.setup.live().expect("a code");
        let (status, body) = call(
            &mut router,
            From::Elsewhere,
            "POST",
            SETUP_PATH,
            None,
            Some(json!({"code": code, "username": "droppen-app", "password": PASSWORD})),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (409, Some("name_taken")), "{body}");
        let (status, body) = call(
            &mut router,
            From::Elsewhere,
            "POST",
            SETUP_PATH,
            None,
            Some(json!({"code": code, "username": "ada", "password": PASSWORD})),
        )
        .await;
        assert_eq!(status, 200, "the code was not spent on the taken name: {body}");
    }

    /// A setup request dropped with its connection after the right code and
    /// before its account exists gives the code back — and while it was being
    /// spent, nothing minted another (the server's 3 s check asked meanwhile).
    #[tokio::test]
    async fn a_setup_dropped_before_its_account_exists_gives_the_code_back() {
        let (_home, state, mut router) = server(None).await;
        let auth = &state.drive.auth;
        let code = auth.setup.live().expect("a code");
        let body = json!({"code": code, "username": "ada", "password": PASSWORD});

        // The store takes no write: the request gets past the code, and waits.
        let writes = auth.users.hold_writes().await;
        {
            let mut request = Box::pin(call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(body.clone())));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while auth.setup.live().is_some() {
                tokio::select! {
                    biased;
                    answer = &mut request => panic!("answered while the store took no write: {answer:?}"),
                    () = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
                assert!(std::time::Instant::now() < deadline, "the request never got past the code");
            }
            // The code is being spent; whatever asks mints nothing over it.
            assert!(auth.in_setup().await);
            assert_eq!(auth.setup.live(), None);
            // The connection goes away, and the request with it.
        }
        drop(writes);
        assert_eq!(auth.setup.live().as_deref(), Some(code.as_str()), "given back, the same code");
        assert!(!auth.users.has_users().await);

        // And it still makes the account.
        let (status, answer) = call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(body)).await;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(auth.setup.live(), None, "used up by the account it made");
    }

    /// The desktop app's *Open in Browser*: its own login asks for a page
    /// link, on its own server only — nobody else gets one, and once the
    /// instance has a user account there is only the login page to open.
    #[tokio::test]
    async fn only_the_desktop_apps_own_login_gets_a_page_link() {
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        state.drive.auth.users.add_app("droppen-app", PASSWORD).await.expect("app account");
        let who = |name: &str| {
            Identity(super::super::AuthIdentity {
                username: name.into(),
                token_sha256: None,
            })
        };
        let code_of = |error: ApiError| (error.status().as_u16(), error.body()["error"].as_str().unwrap_or_default().to_string());

        // Not a server the desktop app runs: `open` is the way there.
        let refused = local_open_grant(State(state.clone()), who("droppen-app")).await.unwrap_err();
        assert_eq!(code_of(refused), (403, "not_desktop_app".into()));

        state.desktop_app = true;
        let Json(grant) = local_open_grant(State(state.clone()), who("droppen-app")).await.expect("a link");
        let token = grant.token.0.expect("a token");
        assert!(state.drive.auth.local_open.redeem(&token), "a live one-time token");
        // Anybody else signed in to this server — the owner it opened, a
        // device through Droppen — gets none.
        for other in [LOCAL_OWNER, "cloud:d-1"] {
            let refused = local_open_grant(State(state.clone()), who(other)).await.unwrap_err();
            assert_eq!(code_of(refused), (403, "not_app_account".into()), "{other}");
        }

        // With a person's account there is only the login page to open.
        state.drive.auth.users.add("ada", PASSWORD).await.expect("account");
        let refused = local_open_grant(State(state.clone()), who("ada")).await.unwrap_err();
        assert_eq!(code_of(refused), (403, "not_app_account".into()));
        let Json(grant) = local_open_grant(State(state.clone()), who("droppen-app")).await.expect("an answer");
        assert_eq!(grant.token.0, None);
    }

    /// An instance with a user account asks for a login everywhere, with
    /// `auth.enabled` unset or `true`: the account ends the setup state, so no
    /// code is offered and only a login gets in.
    #[tokio::test]
    async fn with_a_user_account_everyone_logs_in() {
        for enabled in [None, Some(true)] {
            let (_home, state, mut router) = server(enabled).await;
            state.drive.auth.users.add("ada", PASSWORD).await.expect("account");
            // The owner's next tick.
            state.drive.auth.in_setup().await;
            for from in EVERYWHERE {
                let body = status_of(&mut router, from, None).await;
                assert_eq!(body["authRequired"], true, "{enabled:?} {from:?}");
                assert_eq!(body["setupRequired"], false);
                assert_eq!(
                    call(&mut router, from, "GET", "/api/settings", None, None).await.0,
                    401,
                    "{enabled:?} {from:?}"
                );
            }
            assert_eq!(state.drive.auth.setup.live(), None, "no code for an instance with an account");
            let token = login_as(&mut router, "ada").await;
            assert_eq!(
                call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&token), None).await.0,
                200
            );
        }
    }

    /// `auth.enabled = false`: no setup state, no code, nothing refused.
    #[tokio::test]
    async fn with_authentication_off_nothing_is_asked() {
        let (_home, state, mut router) = server(Some(false)).await;
        for from in EVERYWHERE {
            assert_eq!(call(&mut router, from, "GET", "/api/settings", None, None).await.0, 200, "{from:?}");
            let body = status_of(&mut router, from, None).await;
            assert_eq!(body["authRequired"], false);
            assert_eq!(body["setupRequired"], false);
            assert_eq!(body["authOff"], true);
        }
        assert_eq!(state.drive.auth.setup.live(), None);
        let (status, body) = call(
            &mut router,
            From::Elsewhere,
            "POST",
            SETUP_PATH,
            None,
            Some(json!({"code": "x", "username": "ada", "password": PASSWORD})),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (409, Some("auth_off")));
        let token = state.drive.auth.local_open.mint().unwrap();
        let (status, body) = call(
            &mut router,
            From::ThisDevice,
            "POST",
            LOCAL_OPEN_PATH,
            None,
            Some(json!({"token": token})),
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (409, Some("auth_off")));
    }

    /// Another site's page in a browser that holds a session: `Sec-Fetch-Site`
    /// is the browser's word on who started the request, and anything but this
    /// page itself is refused on `/api/*` — a WebSocket handshake included.
    #[tokio::test]
    async fn another_sites_page_is_refused_even_with_a_session() {
        let (_home, state, mut router) = server(None).await;
        let owner = open_as_owner(&mut router, &state).await;
        for site in ["cross-site", "same-site"] {
            for (method, path) in [
                ("GET", "/api/settings"),
                ("POST", "/api/terminals"),
                ("GET", STATUS_PATH),
                ("POST", SETUP_PATH),
            ] {
                let (status, _, body) = send(
                    &mut router,
                    From::ThisDevice,
                    method,
                    path,
                    Some(&owner),
                    None,
                    &[("sec-fetch-site", site)],
                )
                .await;
                assert_eq!(status, 403, "{site} {method} {path}: {body}");
            }
        }
        for site in ["same-origin", "none"] {
            let (status, _, _) = send(
                &mut router,
                From::ThisDevice,
                "GET",
                "/api/settings",
                Some(&owner),
                None,
                &[("sec-fetch-site", site)],
            )
            .await;
            assert_eq!(status, 200, "{site}");
        }
        let upgrade = [
            ("connection", "upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ];
        let mut foreign = upgrade.to_vec();
        foreign.push(("origin", "https://evil.example"));
        assert_eq!(
            send(&mut router, From::ThisDevice, "GET", "/api/events", Some(&owner), None, &foreign)
                .await
                .0,
            403
        );
        let mut dev = upgrade.to_vec();
        dev.push(("origin", "http://localhost:5180"));
        assert_eq!(
            send(&mut router, From::ThisDevice, "GET", "/api/events", Some(&owner), None, &dev)
                .await
                .0,
            403,
            "a development origin is not trusted for any host"
        );
    }

    async fn mint_ticket(router: &mut gotcha::axum::Router, token: &str) -> String {
        let (status, body) = call(
            router,
            From::ThisDevice,
            "POST",
            "/api/auth/ticket",
            Some(token),
            Some(json!({"scope": "download"})),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        body["ticket"].as_str().unwrap().to_string()
    }

    /// A ticket is worth no more than the login that minted it: logging out or
    /// removing the account kills it, and only a login mints one.
    #[tokio::test]
    async fn a_ticket_dies_with_its_session_and_its_account() {
        let (_home, state, mut router) = server(None).await;
        state.drive.auth.users.add("ada", PASSWORD).await.expect("account");
        let download = |ticket: &str| format!("/api/fs/download?path=%2Fnonexistent&ticket={ticket}");
        let first = login_as(&mut router, "ada").await;
        let ticket = mint_ticket(&mut router, &first).await;
        assert_ne!(
            call(&mut router, From::Elsewhere, "GET", &download(&ticket), None, None).await.0,
            401,
            "live"
        );
        let (status, _) = call(&mut router, From::ThisDevice, "POST", "/api/auth/logout", Some(&first), None).await;
        assert_eq!(status, 200);
        assert_eq!(
            call(&mut router, From::Elsewhere, "GET", &download(&ticket), None, None).await.0,
            401,
            "its session is gone"
        );

        let second = login_as(&mut router, "ada").await;
        let ticket = mint_ticket(&mut router, &second).await;
        assert_ne!(call(&mut router, From::Elsewhere, "GET", &download(&ticket), None, None).await.0, 401);
        state.drive.auth.users.remove("ada").await.expect("user rm");
        assert_eq!(
            call(&mut router, From::Elsewhere, "GET", &download(&ticket), None, None).await.0,
            401,
            "its account is gone"
        );
    }

    /// The page, and every other answer but an embedded app's, may not be put
    /// in another site's frame.
    #[tokio::test]
    async fn no_other_site_may_frame_the_page() {
        let (_home, _state, mut router) = server(None).await;
        for path in ["/", "/sessions", STATUS_PATH] {
            let (_, headers, _) = send(&mut router, From::ThisDevice, "GET", path, None, None, &[]).await;
            assert_eq!(headers.get("x-frame-options").map(|v| v.to_str().unwrap()), Some("SAMEORIGIN"), "{path}");
            assert_eq!(
                headers.get("content-security-policy").map(|v| v.to_str().unwrap()),
                Some("frame-ancestors 'self'"),
                "{path}"
            );
        }
    }

    /// Another device of the account, bridged in by the agent, is itself — and
    /// the bridge keeps working on a device with no user account.
    #[tokio::test]
    async fn a_bridged_device_is_itself() {
        let (_home, state, mut router) = server(None).await;
        let credential = state.drive.auth.internal_credential().to_string();
        let bridged = [
            (crate::agent::bridge::CREDENTIAL_HEADER, credential.as_str()),
            (crate::agent::bridge::DEVICE_HEADER, "d-1"),
        ];
        let (status, _, body) = send(&mut router, From::ThisDevice, "GET", STATUS_PATH, None, None, &bridged).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["user"], "cloud:d-1");
        assert_eq!(
            send(&mut router, From::ThisDevice, "GET", "/api/settings", None, None, &bridged).await.0,
            200
        );
        let (status, _, body) = send(&mut router, From::Elsewhere, "GET", "/api/settings", None, None, &bridged).await;
        assert_eq!(status, 401, "only from loopback: {body}");
        let (status, _, body) = send(
            &mut router,
            From::ThisDevice,
            "POST",
            LOCAL_OPEN_PATH,
            None,
            Some(json!({"token": "x"})),
            &bridged,
        )
        .await;
        assert_eq!((status, body["error"].as_str()), (403, Some("not_from_cloud")));
    }

    #[tokio::test]
    async fn the_status_says_whether_the_desktop_app_owns_the_login() {
        let home = tempfile::tempdir().expect("tempdir");
        let mut state = crate::api::test_support::state(home.path()).await;
        let Json(ordinary) = status(State(state.clone()), MaybeIdentity(None)).await;
        assert!(!ordinary.desktop_app);
        assert!(ordinary.auth_required, "on by default");
        assert!(ordinary.setup_required, "and no account yet");
        state.desktop_app = true;
        let Json(under_the_app) = status(State(state), MaybeIdentity(None)).await;
        assert!(under_the_app.desktop_app);
        let wire = serde_json::to_value(&under_the_app).expect("json");
        assert_eq!(wire["desktopApp"], true, "the web app reads it as desktopApp: {wire}");
    }

    #[tokio::test]
    async fn the_status_says_how_to_run_this_server_on_its_device() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let state_paths = state.paths.clone();
        let Json(answer) = status(State(state), MaybeIdentity(None)).await;
        assert_eq!(answer.droppen_path, crate::program::droppen_path());
        assert!(!answer.droppen_path.is_empty());
        // The test binary has no `droppen` beside it: the binary itself.
        let exe = std::env::current_exe().expect("exe");
        let name = exe.file_name().expect("name").to_string_lossy().into_owned();
        assert!(answer.droppen_path.ends_with(&name), "{} vs {name}", answer.droppen_path);
        let wire = serde_json::to_value(&answer).expect("json");
        assert_eq!(
            wire["droppenPath"],
            answer.droppen_path.as_str(),
            "the web app reads it as droppenPath: {wire}"
        );
        // And what such a command needs to reach this instance: this test's
        // server keeps its files under a home of its own, not the process's.
        let expected: Vec<serde_json::Value> = crate::program::droppen_env(&state_paths)
            .into_iter()
            .map(|(name, value)| json!({"name": name, "value": value}))
            .collect();
        assert!(!expected.is_empty());
        assert_eq!(wire["droppenEnv"], serde_json::Value::Array(expected), "{wire}");
    }
}
