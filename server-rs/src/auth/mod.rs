//! Local accounts: who may talk to this server at all.
//!
//! Until this module existed the answer was "anybody who can reach the port",
//! and the README said so in a warning box. That is defensible for a loopback
//! binding and indefensible for anything else, because the API can read and
//! write every file under `$HOME`, open interactive shells, and drive the local
//! CLIs. This is the missing layer, and it is deliberately small:
//!
//! * [`users`] — argon2id password hashes in
//!   `<data>/instances/<name>/users.json`, one account set per instance, so an
//!   instance handed to somebody else is a login handed to somebody else and
//!   nothing more;
//! * [`sessions`] — bearer tokens, stored as `sha256(token)` in the *instance*
//!   directory, absolute seven-day expiry;
//! * [`tickets`] — short-lived, scoped credentials for websockets and image
//!   URLs, the two places a browser cannot send a header;
//! * [`lockout`] — five wrong passwords and the account rests for five minutes;
//! * [`setup`] — the setup state of an instance with no account, and the
//!   one-time code that creates the first one from its page;
//! * [`local_open`] — the local capability: a control socket only this OS user
//!   can reach, through which `remote-hand-server open` gets a one-time token
//!   that opens the page signed in as this device's owner;
//! * [`routes`] — the nine endpoints under `/api/auth/`;
//! * [`cli`] — `remote-hand-server user add|list|passwd|rm`, the local-admin
//!   path to an account that needs no code: whoever has a shell here is trusted.
//!
//! # Where the guard is mounted, and why it matters
//!
//! [`layer`] goes on in [`crate::app::RemoteHandApp::prepare`], **after** the
//! last `fallback_service`. `axum::Router::layer` wraps the routes and the
//! fallback *as they stand when it is called*, so a guard registered inside
//! [`crate::api::routes`] — where [`crate::origin`]'s is — would cover the
//! documented routes and miss `/openapi.json`, `/redoc`, `/scalar`, the SPA
//! fallback and the `/embed/{id}/*` proxy. Those are exactly the paths an
//! unauthenticated caller would try first.
//!
//! # Who has to log in
//!
//! Every request proves who it is, wherever it comes from. Nothing is trusted
//! because of its peer address or its `Host`: a TCP forwarder (`ssh -R`, frp,
//! a "share with LAN" proxy) makes a stranger's connection arrive from
//! loopback, with whatever `Host` the stranger likes.
//!
//! | instance | how a browser gets in |
//! |---|---|
//! | a user account exists | login |
//! | no user account (app-internal ones don't count) | local-open (`remote-hand-server open`, or the desktop app), or the one-time setup code |
//! | `auth.enabled = false` | nothing asked — development and tests only |
//!
//! With no user account the instance is in its **setup state** ([`setup`]):
//! nothing answers without a session but the status route, the setup route,
//! the local-open route and the static shell. Two ways out of it:
//!
//! * **local-open** ([`local_open`]): the OS user who runs this server — the one
//!   the AI CLIs run as, who can read everything anyway — asks the running
//!   server for a one-time token over a socket nobody else can reach, and the
//!   page exchanges it for a session of [`LOCAL_OWNER`]. That session is this
//!   device's owner while there is no user account: it can link the device to
//!   Droppen and see what is copied of other devices, and it can make the first
//!   account without the setup code. The moment a user account exists every
//!   local-owner session is revoked, and logging in is the only way in again.
//! * **the setup code** — for a headless server set up from another device's
//!   browser, which proves nothing about being this device's owner.
//!
//! `remote-hand-server user add` on the device needs neither: a shell here is
//! already trusted.
//!
//! `auth.enabled = true` is the default spelled out. `auth.enabled = false`
//! (`APP_AUTH__ENABLED=false`) switches authentication off: every route answers
//! anyone who can reach the port. An explicit escape hatch for development and
//! tests, unsafe anywhere else, and the server says so loudly every time it
//! starts.
//!
//! Accounts are per-instance and are not inherited: an instance created on a
//! machine that has accounts starts in its setup state. [`users`] has the
//! reasoning, and why the machine-wide set an older build kept is left on disk
//! rather than moved or deleted.
//!
//! # What is deliberately *not* behind the guard
//!
//! Five things, and nothing else:
//!
//! * `POST /api/auth/login`, `POST /api/auth/setup`, `POST /api/auth/local-open`
//!   and `GET /api/auth/status` — the three ways in, and the way to ask which
//!   of them there is;
//! * `POST /api/cloud/link/return` — droppen.app's sealed answer to a sign-in
//!   attempt, handed back through the system browser (the desktop app's
//!   browser has no login here). It is its own credential: it opens only with
//!   the attempt's one-off key, held in this process, and its state, which
//!   only the page that opened the attempt was given
//!   ([`crate::api::link_handoff`]); the handler refuses everything else;
//! * `OPTIONS` — answered by [`crate::origin`]'s CORS preflight, which a browser
//!   never sends credentials on;
//! * `GET`/`HEAD` outside `/api/`, minus the three documentation paths — the SPA
//!   shell and its assets, because an unauthenticated browser has to be able to
//!   *draw the login page*;
//! * `/embed/{id}/*` — a capability URL. The id is a v4 uuid (122 bits) that only
//!   `GET /api/apps` hands out, and the proxied VS Code server speaks its own
//!   protocol on paths this server does not model. The residual risk is real and
//!   written down in the README: anyone who learns an id reaches that app.
//!
//! `/openapi.json`, `/redoc`, `/scalar`, `/api/settings`, `/api/push/vapid` and
//! `POST /api/server/self-upgrade` are **not** exempt. The first three describe
//! the attack surface; the last replaces the binary.
//!
//! # The cloud agent's internal credential
//!
//! An enrolled server bridges a device's end-to-end channel to its own
//! loopback listener ([`crate::agent::bridge`]) so that no handler changes.
//! Those requests carry `x-rh-agent: <credential>`, a random value minted per
//! boot that exists only in this process's memory ([`Auth::internal_credential`]),
//! plus `x-rh-agent-device: <device id>`. [`guard`] accepts the pair **only
//! from a loopback peer** (the listener is served with connect info for this)
//! and then treats the request as `cloud:<device>` — the device the agent
//! authorised against its device chain. Both headers are removed before any
//! handler runs, and a wrong or non-loopback credential is simply no identity.
//!
//! # A third kind of credential, on one route
//!
//! `POST /api/notify` is called by a process *this server spawned* — the
//! `rh-notify` an agent runs — which has no account, no login and no browser to
//! hold a session token. It is nevertheless **not** exempt, and that is the
//! whole point: it reaches out of the machine and onto someone's lock screen, so
//! an open route there would be a spam relay for anything that could reach the
//! port.
//!
//! Instead [`guard`] recognises one more credential on that one path: the keyed,
//! stateless capability [`crate::notify::token`] mints per session and injects
//! into the spawn's environment. Stateless because a resident CLI outlives the
//! server that started it and cannot be handed a new in-memory table; scoped to
//! one route because it is not an identity and must never satisfy anything that
//! asks who you are. It yields a [`crate::notify::NotifyCaller`] in the request
//! extensions rather than an [`AuthIdentity`], so no handler can mistake one for
//! the other, and the notify handler requires it — which is what keeps the route
//! closed on a server where authentication is off entirely.

pub mod cli;
pub mod local_open;
pub mod lockout;
pub mod routes;
pub mod sessions;
pub mod setup;
pub mod tickets;
pub mod users;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gotcha::axum::extract::{FromRequestParts, Request, State};
use gotcha::axum::http::request::Parts;
use gotcha::axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use gotcha::axum::middleware::{FromFnLayer, Next};
use gotcha::axum::response::{IntoResponse, Response};

use crate::api::error::ApiError;
use crate::config::AuthSettings;
use crate::instance::InstanceDirs;
use crate::sessions::Paths;
use lockout::{LockoutPolicy, Lockouts};
use tickets::{TicketScope, Tickets};
use users::UsersStore;

pub use routes::{describe, routes};

/// The paths gotcha mounts its own documentation on
/// (`gotcha-0.4.1/src/router.rs:311`). They sit outside `/api/`, which is what
/// would otherwise sweep them into the static-asset exemption — and they are the
/// last thing that should be readable by someone who cannot call the API they
/// describe.
const DOC_PATHS: [&str; 3] = ["/openapi.json", "/redoc", "/scalar"];

/// How often the server that owns its instance brings the setup code in line
/// with the account store ([`Auth::in_setup`]): an account made with
/// `user add`, or the last one removed with `user rm`, takes effect within
/// this — without the status route having to change anything to notice.
pub const SETUP_TICK: Duration = Duration::from_secs(3);

/// A day, for turning `session_ttl_days` into a duration.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Whether authentication is on or off; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The default, and `auth.enabled = true`: every request proves who it is.
    Accounts,
    /// `auth.enabled = false`, or a router built by `export-openapi`: nothing
    /// is asked of anyone. Unsafe anywhere a stranger can reach the port.
    Off,
}

/// The identity a local-open session speaks for: this device's owner, the OS
/// user who runs the server, while the instance has no user account.
///
/// Not an account name — a colon is not allowed in one ([`users::valid_name`]),
/// so no account can ever be mistaken for it, nor it for an account. Valid only
/// while the instance has no user account ([`Auth::still_valid`]); every session
/// of it is revoked when the first one is made. [`crate::api::cloud`] treats it
/// as the manager of the Droppen link, which is the one place outside this
/// module that has to know it exists ([`is_local_owner`]).
pub const LOCAL_OWNER: &str = "local:owner";

/// Whether `username` is the local owner ([`LOCAL_OWNER`]).
pub fn is_local_owner(username: &str) -> bool {
    username == LOCAL_OWNER
}

/// Who a request is, once the guard has decided.
///
/// Put in the request extensions by [`guard`] and read back out by the
/// [`Identity`] extractor, so a handler never parses a header itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthIdentity {
    /// The account name.
    pub username: String,
    /// The digest of the session token this request carried, when it carried
    /// one — or, for a request authorized by a ticket, of the session that
    /// minted the ticket (`None` for a ticket no session minted).
    pub token_sha256: Option<String>,
}

/// Everything the guard and the `/api/auth/*` routes need.
///
/// Hangs off [`crate::drive::Drive`], like every other registry in this server,
/// so two servers in one process cannot see each other's sessions, tickets or
/// failure counts.
pub struct Auth {
    /// This instance's accounts. Not the machine's: see [`users`].
    pub users: UsersStore,
    /// This instance's live logins.
    pub sessions: sessions::SessionsStore,
    /// Live websocket and download tickets.
    pub tickets: Tickets,
    /// Failed-login counters, per account.
    pub lockouts: Lockouts,
    /// The setup state's code and its rate limit.
    pub setup: setup::Setup,
    /// The local-open tokens minted over the control socket.
    pub local_open: local_open::LocalOpen,
    /// One login attempt at a time.
    ///
    /// The lockout is a check-then-act with an argon2id verification in the
    /// middle of it, so it is only a limit at all while the check, the hash and
    /// the record of what the hash said cannot be interleaved with another
    /// attempt's. Held by [`routes::login`] across all three. See the comment
    /// there for why this is a gate rather than a counter claimed up front.
    pub login_gate: tokio::sync::Mutex<()>,
    policy: LockoutPolicy,
    mode: Mode,
    /// Where [`crate::notify`]'s token secret is stored, and which instance a
    /// session token must have been minted for. Held here rather than reached
    /// through the application state because [`guard`] runs as a layer over the
    /// router and only ever sees this struct.
    paths: Paths,
    instance: String,
    /// The cloud agent's per-boot loopback credential (hex), see the module docs.
    internal: String,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Auth")
            .field("mode", &self.mode)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl Auth {
    /// Build the subsystem for one server.
    ///
    /// `disabled` is `export-openapi`'s: that command serves the real router in
    /// process and reads `/openapi.json` back off it, which on a developer's own
    /// machine — where accounts exist — would otherwise be a `401` and a failed
    /// `pnpm gen:api`. It is the same escape hatch
    /// [`crate::app::RemoteHandApp::skip_lock`] is, for the same command.
    pub fn new(paths: &Paths, instance: &InstanceDirs, settings: &AuthSettings, disabled: bool) -> Arc<Self> {
        let mode = match (disabled, settings.enabled()) {
            (true, _) | (_, Some(false)) => Mode::Off,
            (_, Some(true) | None) => Mode::Accounts,
        };
        Arc::new(Self {
            users: UsersStore::new(paths),
            sessions: sessions::SessionsStore::new(paths.instance_db(), DAY * settings.session_ttl_days()),
            tickets: Tickets::new(),
            lockouts: Lockouts::new(),
            setup: setup::Setup::default(),
            local_open: local_open::LocalOpen::default(),
            login_gate: tokio::sync::Mutex::new(()),
            policy: LockoutPolicy {
                attempts: settings.lockout_attempts(),
                window: Duration::from_secs(60) * settings.lockout_minutes(),
            },
            mode,
            paths: paths.clone(),
            instance: instance.name().to_string(),
            internal: {
                use rand::RngCore;
                let mut bytes = [0u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut bytes);
                crate::text::hex(&bytes)
            },
        })
    }

    /// The per-boot credential the cloud agent's bridge presents on loopback.
    pub fn internal_credential(&self) -> &str {
        &self.internal
    }

    /// The bridged device a request speaks for, if it carries this boot's
    /// internal credential and came from a loopback peer. `None` otherwise.
    fn bridged(&self, headers: &HeaderMap, peer: Option<std::net::SocketAddr>) -> Option<AuthIdentity> {
        bridged_identity(&self.internal, headers, peer)
    }

    /// The lockout rules this server was configured with.
    pub fn policy(&self) -> LockoutPolicy {
        self.policy
    }

    /// Whether authentication is switched off (`auth.enabled = false`).
    pub fn is_off(&self) -> bool {
        self.mode == Mode::Off
    }

    /// Whether this instance is in its setup state: authentication not
    /// switched off, and no user account on file. A read, and only a read —
    /// `GET /api/auth/status` asks it, and a `GET` changes nothing.
    pub async fn setup_pending(&self) -> bool {
        !self.is_off() && !self.users.has_users().await
    }

    /// [`Auth::setup_pending`], with the setup code brought in line on the
    /// way: one minted if the state has begun on a server that owns its
    /// instance, the live one dropped if an account has appeared (`user add`).
    /// Asked at boot, every few seconds by the owner's [`SETUP_TICK`] loop,
    /// and by `POST /api/auth/setup`.
    pub async fn in_setup(&self) -> bool {
        let setup = self.setup_pending().await;
        self.setup.refresh(setup);
        setup
    }

    /// Who this request's `Authorization: Bearer` says it is, if anyone.
    ///
    /// The account is re-confirmed on every request rather than trusted from the
    /// session record, which is what makes `user rm` take effect immediately and
    /// everywhere — including in instances this process knows nothing about.
    async fn identify(&self, headers: &HeaderMap) -> Option<AuthIdentity> {
        let token = bearer(headers)?;
        let username = self.sessions.resolve(&token).await?;
        if !self.still_valid(&username).await {
            return None;
        }
        Some(AuthIdentity {
            username,
            token_sha256: Some(sessions::digest_of(&token)),
        })
    }

    /// Whether a session or a ticket of `username` still speaks for anyone: an
    /// account that is still on file, or the local owner while the instance
    /// has no user account (and authentication is on).
    pub async fn still_valid(&self, username: &str) -> bool {
        if is_local_owner(username) {
            return !self.is_off() && !self.users.has_users().await;
        }
        self.users.exists(username).await
    }

    /// Whether the login `digest` still stands for `user`: the session live
    /// (not signed out, revoked, swept or expired) and its identity still
    /// valid ([`Auth::still_valid`]) — what a long-lived socket opened by that
    /// login asks again every [`LOGIN_RECHECK`] ([`login_ended`]).
    pub async fn login_stands(&self, user: &str, digest: &str) -> bool {
        self.sessions.alive(digest).as_deref() == Some(user) && self.still_valid(user).await
    }

    /// The session a `POST /api/notify` proves it is, if any.
    ///
    /// Delegated to [`crate::notify::caller`] rather than implemented here: the
    /// secret, the MAC and the token format are that module's, and this one has
    /// no business knowing any of them. What belongs here is *where* the
    /// question is asked, which is the guard and nowhere else.
    async fn notify_caller(&self, headers: &HeaderMap) -> Option<crate::notify::NotifyCaller> {
        crate::notify::caller(&self.paths, &self.instance, headers).await
    }
}

/// How often a long-lived socket opened by a login looks again at whether that
/// login still stands.
pub const LOGIN_RECHECK: Duration = Duration::from_secs(5);

/// The login behind a request, for a socket to watch: `(account, digest of its
/// session)`. `None` for a request no login opened — authentication off, or a
/// device bridged through Droppen, whose channel the agent ends itself.
pub fn login_of(who: &MaybeIdentity) -> Option<(String, String)> {
    who.0
        .as_ref()
        .and_then(|identity| identity.token_sha256.clone().map(|digest| (identity.username.clone(), digest)))
}

/// Resolves once the login a socket was opened by stops standing — signed
/// out, revoked, its account removed, or (for the local owner) the device's
/// first account made — looked at every [`LOGIN_RECHECK`]; never, for a socket
/// no login opened. The socket's loop selects on it and closes with 4401, so
/// ending a login ends what it has open too, not only its next request.
pub async fn login_ended(auth: Arc<Auth>, login: Option<(String, String)>) {
    let Some((user, digest)) = login else {
        return std::future::pending().await;
    };
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + LOGIN_RECHECK, LOGIN_RECHECK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if !auth.login_stands(&user, &digest).await {
            return;
        }
    }
}

/// The close frame a socket ends with when its login has.
pub fn signed_out_frame() -> gotcha::axum::extract::ws::CloseFrame {
    gotcha::axum::extract::ws::CloseFrame {
        code: 4401,
        reason: "signed out".into(),
    }
}

/// The identity a request bridged by the cloud agent speaks for: only with
/// `expected` (this boot's credential) and only from a loopback peer.
fn bridged_identity(expected: &str, headers: &HeaderMap, peer: Option<std::net::SocketAddr>) -> Option<AuthIdentity> {
    let offered = headers.get(crate::agent::bridge::CREDENTIAL_HEADER)?.as_bytes();
    if !peer.is_some_and(|p| p.ip().is_loopback()) {
        return None;
    }
    if !bool::from(subtle::ConstantTimeEq::ct_eq(offered, expected.as_bytes())) {
        return None;
    }
    let device = headers.get(crate::agent::bridge::DEVICE_HEADER)?.to_str().ok()?;
    let valid = !device.is_empty() && device.len() <= 64 && device.bytes().all(|b| b.is_ascii_graphic());
    valid.then(|| AuthIdentity {
        username: format!("cloud:{device}"),
        token_sha256: None,
    })
}

/// The already-boxed future [`layer`] hands to axum.
type GuardFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;
/// A plain function pointer, so the layer's type can be written down.
type GuardFn = fn(State<Arc<Auth>>, Request, Next) -> GuardFuture;

/// [`guard`] as a router layer.
///
/// The explicit return type is [`crate::origin::layer`]'s trick and exists for
/// the same reason: `from_fn_with_state` leaves its extractor-tuple parameter to
/// be inferred from the `Service` bound at the call site, which rustc will not do
/// backwards through the impl (`axum-0.8.9/src/middleware/from_fn.rs:259`).
pub fn layer(auth: Arc<Auth>) -> FromFnLayer<GuardFn, Arc<Auth>, (State<Arc<Auth>>, Request)> {
    fn boxed(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> GuardFuture {
        Box::pin(guard(auth, request, next))
    }
    gotcha::axum::middleware::from_fn_with_state(auth, boxed as GuardFn)
}

/// Refuse anything that has not proved who it is, and say so the same way every
/// time.
pub async fn guard(auth: Arc<Auth>, mut request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    // A preflight carries no credentials — the browser strips them by
    // specification — so answering it with a `401` would break every
    // cross-origin request before the real one was ever sent. It goes straight
    // through to `crate::origin`, which answers it without routing.
    if method == Method::OPTIONS {
        return with_headers(next.run(request).await, &path);
    }

    // The cloud agent's bridge: honoured only from loopback with this boot's
    // credential, and stripped either way so no handler ever sees it.
    let peer = request
        .extensions()
        .get::<gotcha::axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0);
    let bridged = auth.bridged(request.headers(), peer);
    request.headers_mut().remove(crate::agent::bridge::CREDENTIAL_HEADER);
    request.headers_mut().remove(crate::agent::bridge::DEVICE_HEADER);

    let is_exempt = exempt(&method, &path);
    // Resolved wherever it could matter, which is two overlapping sets. Every
    // `/api/` path, exempt or not: `GET /api/auth/status` is exempt and still has
    // to be able to say who you are. And everything *not* exempt, `/api/` or
    // otherwise: `/openapi.json`, `/redoc` and `/scalar` sit outside `/api/` and
    // are guarded, so a token has to be able to open them.
    //
    // What is left out is the case that matters for cost: an exempt static
    // asset, which is most requests a browser makes and now touches neither the
    // session store nor the account file.
    let mut identity = if bridged.is_some() {
        bridged
    } else if path.starts_with("/api/") || !is_exempt {
        auth.identify(request.headers()).await
    } else {
        None
    };

    // A ticket is only ever looked at on the two surfaces that cannot carry a
    // header, and only for the scope that surface belongs to. Everywhere else a
    // `?ticket=` in the URL is ignored, which is what keeps it from becoming a
    // general-purpose credential in a query string.
    if identity.is_none() {
        if let Some((ticket, scope)) = ticket_offered(&method, &path, request.uri(), request.headers()) {
            if let Some(redeemed) = auth.tickets.redeem(&ticket, scope) {
                // A ticket is worth no more than the login that minted it: that
                // session still live, and its account still valid. The session
                // goes with the identity, so a long-lived socket opened with it
                // ends with that login (the local gateway, the feeds).
                let live = auth.sessions.alive(&redeemed.session).is_some_and(|owner| owner == redeemed.username);
                if live && auth.still_valid(&redeemed.username).await {
                    identity = Some(AuthIdentity {
                        username: redeemed.username,
                        token_sha256: Some(redeemed.session),
                    });
                }
            }
        }
    }

    // The third credential, and only ever on its own route. Asked after the two
    // above so a logged-in browser calling the same path is simply itself, and
    // gated on the path so no other request pays for the file read.
    let caller = if crate::notify::is_notify_request(&method, &path) {
        auth.notify_caller(request.headers()).await
    } else {
        None
    };

    if let Some(found) = identity.clone() {
        request.extensions_mut().insert(found);
    }
    if let Some(found) = caller.clone() {
        request.extensions_mut().insert(found);
    }

    // Nobody is let through for where they connect from: an identity, the
    // notify capability on its one route, the exempt surface — or nothing at
    // all asked, with authentication switched off. Everyone else is refused
    // without a word about why; the status route is where a client learns it.
    if identity.is_some() || caller.is_some() || is_exempt || auth.is_off() {
        return with_headers(next.run(request).await, &path);
    }
    with_headers(ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized").into_response(), &path)
}

/// `Referrer-Policy: no-referrer`, on everything this server serves.
///
/// Session tokens live in the page's own storage rather than in URLs, so this is
/// belt and braces — but the page also renders arbitrary text from transcripts,
/// and a `Referer` header is how a link a model wrote tells a third party which
/// host and path the person clicked it from.
///
/// And no framing by another site, on everything but an embedded app — whose
/// proxy (`crate::apps::embed`) answers for itself, since the page frames it:
/// a page somebody else serves must not be able to lay this one, signed in,
/// under a transparent frame and have its clicks land here.
fn with_headers(mut response: Response, path: &str) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if !is_embed(path) {
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
        if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
            headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("frame-ancestors 'self'"));
        }
    }
    response
}

/// Paths that answer without a token.
fn exempt(method: &Method, path: &str) -> bool {
    if method == Method::POST && [routes::LOGIN_PATH, setup::SETUP_PATH, routes::LOCAL_OPEN_PATH].contains(&path) {
        return true;
    }
    if method == Method::GET && path == routes::STATUS_PATH {
        return true;
    }
    if method == Method::POST && path == crate::api::cloud::RETURN_ROUTE {
        return true;
    }
    if is_embed(path) {
        return true;
    }
    // The SPA and its assets. An unauthenticated browser must be able to load
    // the shell, or there is nowhere to type a password. `index.html` and the
    // hashed bundles say nothing about the machine they are served from; the
    // three documentation paths do, so they are carved back out.
    (method == Method::GET || method == Method::HEAD) && !path.starts_with("/api/") && !DOC_PATHS.contains(&path)
}

/// The `/embed/{id}` prefix, matched exactly as
/// [`crate::apps::embed`] matches it, so the exemption and the proxy cannot
/// disagree about what an embed path is.
fn is_embed(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/embed/") else { return false };
    let id = rest.split('/').next().unwrap_or_default();
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The ticket in this request's query string, and the scope it would have to be
/// in — or `None` when this is not a path that takes tickets.
fn ticket_offered(method: &Method, path: &str, uri: &Uri, headers: &HeaderMap) -> Option<(String, TicketScope)> {
    let scope = if method == Method::GET && path == "/api/fs/download" {
        TicketScope::Download
    } else if method == Method::GET && is_websocket_path(path) && is_upgrade(headers) {
        TicketScope::Ws
    } else {
        return None;
    };
    Some((query_param(uri, "ticket")?, scope))
}

/// The websocket routes, by path: the three of the app, and the local
/// gateway (`/api/agg/ws`, S1-7).
///
/// Written out rather than derived, because "ends in `/ws`" is a rule a future
/// route could satisfy by accident, and this list is a security boundary.
fn is_websocket_path(path: &str) -> bool {
    path == "/api/events"
        || path == "/api/agg/ws"
        || (path.starts_with("/api/sessions/") && path.ends_with("/ws"))
        || (path.starts_with("/api/terminals/") && path.ends_with("/ws"))
}

/// Whether this is really a websocket handshake and not a plain `GET` of the
/// same path. Without the check, a `?ticket=` would be spendable on any request
/// to a `/ws` path.
fn is_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// One query parameter, undecoded.
///
/// Both tickets are base64url, whose alphabet needs no percent-encoding, so a
/// decoder would only be a way for an encoded ticket to match a stored one that
/// is not byte-identical.
fn query_param(uri: &Uri, name: &str) -> Option<String> {
    uri.query()?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

/// The token out of `Authorization: Bearer <token>`.
///
/// A long-lived token is only ever accepted in this header. It is deliberately
/// not read from a query parameter or a cookie: the first ends up in proxy logs
/// and browser history, and the second would make every route forgeable
/// cross-site unless a second anti-CSRF mechanism were added to carry the weight
/// the `Authorization` header carries for free.
fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// The caller, for a route that requires one.
///
/// Reads what [`guard`] put in the extensions. The `401` it rejects with is only
/// reachable on a server where authentication is *off* — with it on, the guard
/// has already refused — which is exactly right: `POST /api/auth/password` has
/// nothing to change the password of when nobody is logged in.
///
/// Annotate the parameter `#[api(skip)]` so gotcha does not ask it for an
/// OpenAPI contribution it has none of (`gotcha_macro-0.4.1/src/route.rs:76`).
#[derive(Debug, Clone)]
pub struct Identity(pub AuthIdentity);

impl<S: Send + Sync> FromRequestParts<S> for Identity {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthIdentity>()
            .cloned()
            .map(Identity)
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

/// The caller, for a route that answers either way.
#[derive(Debug, Clone)]
pub struct MaybeIdentity(pub Option<AuthIdentity>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeIdentity {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(MaybeIdentity(parts.extensions.get::<AuthIdentity>().cloned()))
    }
}

/// Write JSON `0600` through a tmp file.
///
/// The fourth copy of this in the tree, after [`crate::accounts::store`],
/// [`crate::push::store`] and [`crate::chats::provider`], and copied on purpose
/// for the reason `push::store` gives: the rule is what matters, and the rule is
/// written down at each site rather than shared through a module none of them
/// would otherwise import. The rule:
///
/// 1. write to a tmp file and `rename`, so a crash cannot leave half a store;
/// 2. give `0600` to `open`, so the bytes are never world-readable — not even
///    for the instant between `write` and a `chmod`;
/// 3. set `0600` **again** afterwards, because `open`'s mode only applies to a
///    fresh inode and a tmp file left behind by an earlier crash keeps whatever
///    mode it already had.
///
/// Password hashes and session tokens are what this file holds, so all three
/// steps are load-bearing.
///
/// Synchronous, which it was not: [`users::UsersStore`] mirrors from an `async
/// fn` on a blocking thread *and* from the repair in `ensure_imported`, which
/// runs inside a `std::sync::Once` on the first read of the store where there
/// is no `await` to be had. Two implementations of a three-step rule is how one
/// of the three steps eventually goes missing from one of them, so there is one,
/// and it is the one that works in both places.
fn write_json_private(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let body = format!("{}\n", serde_json::to_string_pretty(value)?);
    write_private(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, path)
}

#[cfg(unix)]
fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(body)?;
    file.flush()?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), HeaderValue::from_str(value).expect("header value"));
        }
        map
    }

    #[test]
    fn a_bearer_token_is_read_out_of_the_one_header_that_carries_one() {
        assert_eq!(bearer(&headers(&[(header::AUTHORIZATION, "Bearer abc")])).as_deref(), Some("abc"));
        assert_eq!(bearer(&headers(&[(header::AUTHORIZATION, "bearer abc")])).as_deref(), Some("abc"));
        assert_eq!(bearer(&headers(&[(header::AUTHORIZATION, "Basic abc")])), None);
        assert_eq!(bearer(&headers(&[(header::AUTHORIZATION, "Bearer")])), None);
        assert_eq!(bearer(&headers(&[(header::AUTHORIZATION, "Bearer  ")])), None);
        assert_eq!(bearer(&HeaderMap::new()), None);
    }

    #[test]
    fn the_static_surface_is_exempt_and_the_documentation_is_not() {
        assert!(exempt(&Method::GET, "/"), "the shell, so the login page can be drawn");
        assert!(exempt(&Method::GET, "/assets/index-abc123.js"));
        assert!(exempt(&Method::GET, "/sw.js"));
        assert!(exempt(&Method::GET, "/manifest.webmanifest"));
        assert!(exempt(&Method::GET, "/sessions/claude/x"), "a deep link is still the shell");

        assert!(!exempt(&Method::GET, "/openapi.json"), "the spec describes the attack surface");
        assert!(!exempt(&Method::GET, "/redoc"));
        assert!(!exempt(&Method::GET, "/scalar"));
    }

    #[test]
    fn nothing_under_the_api_is_exempt_but_the_ways_in_and_droppen_app_s_answer() {
        assert!(exempt(&Method::POST, "/api/auth/login"));
        assert!(exempt(&Method::POST, "/api/auth/setup"));
        assert!(exempt(&Method::POST, "/api/auth/local-open"));
        assert!(!exempt(&Method::GET, "/api/auth/local-open"));
        assert!(exempt(&Method::GET, "/api/auth/status"));

        assert!(!exempt(&Method::GET, "/api/auth/setup"), "the setup route is a POST");
        assert!(!exempt(&Method::GET, "/api/cloud"));
        assert!(!exempt(&Method::GET, "/api/events"));
        assert!(!exempt(&Method::POST, "/api/uploads"));
        assert!(!exempt(&Method::GET, "/api/media/github"));

        assert!(!exempt(&Method::GET, "/api/auth/login"), "the login route is a POST");
        assert!(!exempt(&Method::POST, "/api/auth/status"));
        assert!(
            exempt(&Method::POST, "/api/cloud/link/return"),
            "droppen.app's sealed answer, from the system browser"
        );
        assert!(!exempt(&Method::GET, "/api/cloud/link/return"));
        assert!(!exempt(&Method::POST, "/api/cloud/link/finish"));
        assert!(!exempt(&Method::POST, "/api/cloud/link"));
        assert!(!exempt(&Method::GET, "/api/settings"));
        assert!(!exempt(&Method::GET, "/api/push/vapid"));
        assert!(!exempt(&Method::POST, "/api/server/self-upgrade"));
        assert!(!exempt(&Method::GET, "/api/auth/sessions"));
        assert!(!exempt(&Method::GET, "/api/nope"), "the SPA fallback's JSON 404 is still an API path");
    }

    #[test]
    fn the_embed_proxy_is_exempt_on_every_method() {
        for method in [Method::GET, Method::POST, Method::PUT, Method::DELETE] {
            assert!(exempt(&method, "/embed/9f1c/stable/out/main.js"), "{method} on an embed path");
        }
        assert!(is_embed("/embed/9f1c"));
        assert!(!is_embed("/embed/"), "the id is never empty");
        assert!(!is_embed("/embedded/9f1c/x"));
        assert!(!exempt(&Method::POST, "/embed"), "the bare prefix is not an app");
    }

    fn uri(target: &str) -> Uri {
        target.parse().expect("uri")
    }

    #[test]
    fn a_ticket_is_only_looked_at_on_the_two_surfaces_that_need_one() {
        let upgrade = headers(&[(header::UPGRADE, "websocket")]);
        let plain = HeaderMap::new();

        let ws = ticket_offered(&Method::GET, "/api/events", &uri("/api/events?ticket=T"), &upgrade);
        assert_eq!(ws, Some(("T".into(), TicketScope::Ws)));
        let terminal = ticket_offered(&Method::GET, "/api/terminals/abc/ws", &uri("/api/terminals/abc/ws?ticket=T"), &upgrade);
        assert_eq!(terminal, Some(("T".into(), TicketScope::Ws)));
        let download = ticket_offered(&Method::GET, "/api/fs/download", &uri("/api/fs/download?path=%2Fx&ticket=T"), &plain);
        assert_eq!(download, Some(("T".into(), TicketScope::Download)));

        assert_eq!(
            ticket_offered(&Method::GET, "/api/events", &uri("/api/events?ticket=T"), &plain),
            None,
            "a plain GET of a websocket path is not a handshake"
        );
        assert_eq!(
            ticket_offered(&Method::GET, "/api/settings", &uri("/api/settings?ticket=T"), &plain),
            None,
            "everywhere else the parameter is ignored outright"
        );
        assert_eq!(
            ticket_offered(&Method::GET, "/api/fs/read", &uri("/api/fs/read?path=%2Fx&ticket=T"), &plain),
            None,
            "reading a file is not downloading one"
        );
    }

    #[test]
    fn the_websocket_paths_are_the_four_that_exist() {
        assert!(is_websocket_path("/api/events"));
        assert!(is_websocket_path("/api/sessions/claude/abc/ws"));
        assert!(is_websocket_path("/api/terminals/abc/ws"));
        assert!(is_websocket_path("/api/agg/ws"));
        assert!(!is_websocket_path("/api/sessions/claude/abc"));
        assert!(!is_websocket_path("/api/chats/ws"));
        assert!(!is_websocket_path("/api/agg/ws/other"));
    }

    #[test]
    fn the_agents_internal_credential_counts_only_from_loopback() {
        let cred = "ab".repeat(32);
        let with = |value: &str, device: &str| {
            let mut h = HeaderMap::new();
            h.insert(crate::agent::bridge::CREDENTIAL_HEADER, HeaderValue::from_str(value).unwrap());
            h.insert(crate::agent::bridge::DEVICE_HEADER, HeaderValue::from_str(device).unwrap());
            h
        };
        let local: std::net::SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let local6: std::net::SocketAddr = "[::1]:5000".parse().unwrap();
        let remote: std::net::SocketAddr = "192.168.1.9:5000".parse().unwrap();
        assert_eq!(
            bridged_identity(&cred, &with(&cred, "d-abc"), Some(local)).map(|i| i.username),
            Some("cloud:d-abc".to_string())
        );
        assert!(bridged_identity(&cred, &with(&cred, "d-abc"), Some(local6)).is_some());
        assert_eq!(bridged_identity(&cred, &with(&cred, "d-abc"), Some(remote)), None, "not from loopback");
        assert_eq!(bridged_identity(&cred, &with(&cred, "d-abc"), None), None, "no peer address");
        assert_eq!(
            bridged_identity(&cred, &with(&"cd".repeat(32), "d-abc"), Some(local)),
            None,
            "another boot's credential"
        );
        assert_eq!(bridged_identity(&cred, &HeaderMap::new(), Some(local)), None);
        assert_eq!(bridged_identity(&cred, &with(&cred, ""), Some(local)), None, "no device");
    }

    /// What a long-lived socket waits on: nothing for a socket no login
    /// opened; for the device's owner, the moment the first account exists.
    #[tokio::test(start_paused = true)]
    async fn a_login_ends_for_its_sockets_when_it_stops_standing() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let auth = state.drive.auth.clone();
        let token = auth.sessions.issue(LOCAL_OWNER, None).await.expect("session").token;
        let digest = sessions::digest_of(&token);
        let watch = tokio::spawn(login_ended(auth.clone(), Some((LOCAL_OWNER.into(), digest))));
        let nobody = tokio::spawn(login_ended(auth.clone(), None));
        tokio::time::sleep(LOGIN_RECHECK * 3).await;
        assert!(!watch.is_finished(), "the owner stands while there is no account");
        auth.users.add("ada", "correct horse battery staple").await.expect("account");
        tokio::time::sleep(LOGIN_RECHECK * 2).await;
        assert!(watch.is_finished(), "and no longer once there is one");
        assert!(!nobody.is_finished(), "a socket no login opened is never ended for one");
        nobody.abort();
    }

    #[test]
    fn a_query_parameter_is_found_wherever_it_sits() {
        assert_eq!(query_param(&uri("/x?ticket=T"), "ticket").as_deref(), Some("T"));
        assert_eq!(query_param(&uri("/x?a=1&ticket=T&b=2"), "ticket").as_deref(), Some("T"));
        assert_eq!(query_param(&uri("/x?ticketing=T"), "ticket"), None);
        assert_eq!(query_param(&uri("/x"), "ticket"), None);
    }
}
