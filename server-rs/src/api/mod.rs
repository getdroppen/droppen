//! The HTTP + websocket surface.
//!
//! Paths are written flat rather than assembled with [`gotcha::GotchaRouter::nest`],
//! because `nest` joins with an extra slash and documents `/api//sessions`
//! (`gotcha-0.4.1/src/router.rs:161`).

pub mod cloud;
pub mod created;
pub mod error;
pub mod extract;
pub mod link_handoff;

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use gotcha::axum::extract::ws::{Message, WebSocket};
use gotcha::axum::response::Response;
use gotcha::{api, state, GotchaRouter, Json, Schematic, State, WebSocketUpgrade};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::drive::Drive;
use crate::feed;
use crate::openapi_ext::{self, ExtraResponses};
use crate::sessions::{self, claude, codex, Paths, SessionDetail, SessionList, Settings, Source};
use error::ApiError;
use extract::{ApiPath, ApiQuery};

/// The router every subsystem's `routes` function extends.
///
/// Spelled out once here so a subsystem's registration reads as one line rather
/// than three of generics.
pub type ApiRouter = GotchaRouter<gotcha::GotchaContext<AppState, crate::config::AppConfig>>;

// Defined here rather than in whichever subsystem needed it first, because
// gotcha's component registry is flat and keyed by the Rust type name: two
// `SendResult` types anywhere in the crate silently claim the one schema, and
// the winner is whichever the spec walk reaches last — which made the exported
// document flap between runs. The doc comment below becomes the component's
// description in `openapi/openapi.json`, so it stays contract-facing and this
// reasoning stays out of it.
/// The answer to every route that only has to say "done".
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Schematic)]
pub struct SendResult {
    /// Always `true`; a refusal is an error status, not a `false`.
    pub accepted: bool,
}

/// The answer to a send, which now has one more thing to say than "done".
///
/// A separate type rather than a field on [`SendResult`] because the other
/// routes that answer with one — rename, archive, interrupt — have no queue and
/// never will, and a field that is always `false` for most of its users is a
/// worse contract than a second shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Schematic)]
pub struct SendMessageResult {
    /// Always `true`; a refusal is an error status, not a `false`.
    pub accepted: bool,
    /// The message went in behind a turn that is still running, and will be
    /// delivered when that turn ends. Only a resident claude answers `true`.
    pub queued: bool,
    /// The queue id the message travels under (`resident.queuedMessages[].id`),
    /// when the request carried a `clientMessageId`.
    #[serde(default, skip_serializing_if = "crate::openapi_ext::Optional::is_none")]
    pub qid: crate::openapi_ext::Optional<String>,
    /// `true` when this answers a retry: the `clientMessageId` was already
    /// accepted, nothing was enqueued again, and this is what the first
    /// attempt answered.
    #[serde(default, skip_serializing_if = "crate::openapi_ext::Optional::is_none")]
    pub duplicate: crate::openapi_ext::Optional<bool>,
    /// `true` when the agent was blocked on a question and the message went in
    /// as its answer (the question's free-text `response`) rather than into the
    /// queue behind the turn it would otherwise have waited for; `false` when a
    /// question was open but could not take it — answered from another screen a
    /// moment before, stopped, or the session still reconnecting — so it was
    /// delivered as an ordinary message (`queued` says whether it waits).
    /// Absent when no question was open.
    #[serde(default, skip_serializing_if = "crate::openapi_ext::Optional::is_none")]
    pub answered: crate::openapi_ext::Optional<bool>,
}

/// The title and version `openapi/openapi.json` carries.
const API_TITLE: &str = "remote-hand API";
const API_VERSION: &str = "0.1.0";
const API_DESCRIPTION: &str = "Browse local Claude Code and Codex sessions plus the local filesystem.";

/// Everything a handler needs, injected as `State<AppState>`.
#[state]
#[derive(Clone)]
pub struct AppState {
    /// The plugins this build bundles, their per-instance switches and
    /// document stores.
    pub plugins: Arc<crate::plugins::Plugins>,
    /// Where sessions are discovered.
    pub paths: Arc<Paths>,
    /// The instance directory this server owns, and the name it reports.
    pub instance: Arc<crate::instance::InstanceDirs>,
    /// When this process started, ISO-8601.
    pub started_at: Arc<str>,
    /// Every live session feed, so shutdown can tear them down.
    pub feeds: Arc<feed::FeedRegistry>,
    /// The shared session-list feed behind `GET /api/events`.
    pub global: Arc<feed::global::GlobalFeed>,
    /// Cancelled by the shutdown supervisor; live sockets close on it.
    pub cancel: CancellationToken,
    /// Accounts, injections, quota and composer preferences — the one field the
    /// process-driving subsystems add.
    pub drive: Arc<Drive>,
    /// Every live web terminal, so shutdown can kill them.
    pub terminals: Arc<crate::terminals::Registry>,
    /// Instant chats: the provider configuration and the API-backed store.
    pub chats: Arc<crate::chats::Chats>,
    /// Every running embedded app, so shutdown can kill them and the
    /// `/embed/{id}/*` proxy can find their ports.
    pub apps: Arc<crate::apps::Registry>,
    /// This server's claim on its instance directory, filled in by
    /// [`crate::app::RemoteHandApp::prepare`] once the flock is held. Empty in
    /// lock-less servers (`export-openapi`, tests), whose self-upgrade handler
    /// therefore refuses: replacing the shared binary is an owner's move.
    ///
    /// Weak, because this state is cloned into every handler and background
    /// task, and some of those outlive the serve task. A strong reference here
    /// kept the flock until the last of them ended, so an instance stayed
    /// claimed after its server had stopped serving, for as long as that took.
    /// The serve task holds a strong reference
    /// ([`crate::app::Prepared::serve_task`]), and a production server keeps
    /// one more until its process exits (`Prepared::claim_for_process`).
    pub lock: Arc<std::sync::OnceLock<std::sync::Weak<crate::instance::InstanceLock>>>,
    /// The hub link: started at boot for an enrolled instance, and at any
    /// time after by an enrollment from the local settings
    /// ([`crate::agent::control`]). Inert in a server that does not own its
    /// instance.
    pub cloud: Arc<cloud::Cloud>,
    /// Whether the Droppen desktop app runs this server (`DROPPEN_DESKTOP_APP`,
    /// see [`crate::desktop_app`]): no self-upgrade, and the app owns the login.
    pub desktop_app: bool,
    /// The first-run guide: tool detection and the children's `PATH`.
    pub setup: Arc<crate::setup::Setup>,
}

/// Query for `GET /api/sessions`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct SessionListQuery {
    /// Restrict to one CLI.
    pub source: Option<Source>,
    /// Case-insensitive substring match over title and cwd.
    pub q: Option<String>,
    /// How many rows to return, from 1 to 1000; defaults to 200.
    pub limit: Option<u32>,
    /// Whose sessions to list: `owned` (default), `external`, or `all`.
    ///
    /// The default is the change this parameter came with. A server's list is
    /// now the sessions *it* made; the transcripts in the user's own `~/.claude`
    /// are still there, read-only, one `scope=external` away. A client that wants
    /// the old behaviour asks for `all` — and gets a `scope` on every row telling
    /// it which it is looking at.
    pub scope: Option<sessions::ScopeFilter>,
}

/// Query for `GET /api/sessions/{source}/{id}`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct SessionDetailQuery {
    /// Absolute item count the client already holds.
    pub after: Option<u32>,
    /// Signature from the previous response; matching means no transcript reads.
    pub sig: Option<String>,
}

/// The default `limit`, and the range the TypeScript route enforced with
/// `z.coerce.number().int().min(1).max(1000).default(200)`.
const DEFAULT_LIMIT: u32 = 200;
const MIN_LIMIT: u32 = 1;
const MAX_LIMIT: u32 = 1000;

/// List sessions from claude and codex, newest first.
#[api(id = "list_sessions", group = "sessions", summary = "List sessions")]
pub async fn list_sessions(State(state): State<AppState>, ApiQuery(query): ApiQuery<SessionListQuery>) -> Result<Json<SessionList>, ApiError> {
    // Clamping would answer a `limit=0` with 200 rows, which is neither what was
    // asked for nor what the zod validator did: out of range was a `400` there,
    // and silently serving something else hides the client's bug.
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(MIN_LIMIT..=MAX_LIMIT).contains(&limit) {
        return Err(ApiError::bad_request(format!("limit must be between {MIN_LIMIT} and {MAX_LIMIT}")));
    }

    let bindings = state.drive.accounts.session_account_map().await;
    let drivers = state.drive.injections.driver_states();
    let mut sessions = sessions::list_and_filter(
        &state.paths,
        &bindings,
        &drivers,
        query.source,
        query.q.as_deref(),
        query.scope.unwrap_or_default(),
    )
    .await;

    // `total` counts everything that matched, not just the page returned.
    let total = sessions.len();
    sessions.truncate(limit as usize);
    Ok(Json(SessionList { sessions, total }))
}

/// Session state plus transcript delta (full on first call or reset).
#[api(id = "session_detail", group = "sessions", summary = "Get a session")]
pub async fn session_detail(
    State(state): State<AppState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<SessionDetailQuery>,
) -> Result<Json<SessionDetail>, ApiError> {
    let mut detail = feed::load_session_detail(&state.drive, source, &id)
        .await
        .ok_or_else(|| ApiError::not_found("session not found"))?;

    // Cheap path: nothing appended since the client's last poll.
    if let (Some(after), Some(sig)) = (query.after, query.sig.as_deref()) {
        if sig == detail.sig {
            let after = after as usize;
            detail.items_changed = false;
            detail.reset = false;
            detail.items = vec![];
            detail.start_index = after;
            detail.end_index = after;
            detail.total_items = after;
            detail.has_more_before = after > 0;
            detail.omitted_items = 0;
        }
    }
    Ok(Json(detail))
}

/// Server and data-source information.
#[api(id = "settings", group = "meta", summary = "Server settings")]
pub async fn settings(State(state): State<AppState>) -> Json<Settings> {
    let paths = state.paths.as_ref();
    let bindings = state.drive.accounts.session_account_map().await;
    let (claude_sessions, codex_sessions) = tokio::join!(claude::list_claude_sessions(paths, &bindings), codex::list_codex_sessions(paths));
    Json(Settings {
        instance: state.instance.name().to_string(),
        server_id: crate::sessions::ServerId::local(paths).to_string(),
        hostname: hostname(),
        platform: node_platform().to_string(),
        // The field name is the contract's; the value is this server's runtime.
        node_version: RUNTIME_VERSION.to_string(),
        server_version: SERVER_VERSION.to_string(),
        home_dir: paths.home.to_string_lossy().into_owned(),
        claude_dir: paths.claude_projects().to_string_lossy().into_owned(),
        claude_managed_dir: paths.claude_managed_projects().to_string_lossy().into_owned(),
        codex_dir: paths.codex_sessions().to_string_lossy().into_owned(),
        claude_session_count: claude_sessions.len(),
        codex_session_count: codex_sessions.len(),
        server_started_at: state.started_at.to_string(),
        plugins: state.plugins.status_async().await.into_iter().map(Into::into).collect(),
    })
}

/// The live transcript feed. Deliberately **not** annotated with `#[api]`:
/// `WebSocketUpgrade` is not a `ParameterProvider`, so the macro's generated
/// `<WebSocketUpgrade as ParameterProvider>::generate(..)` fails to compile
/// (`gotcha_macro-0.4.1/src/route.rs:88`). An undocumented route coexists with
/// documented ones fine; the TypeScript server leaves it out of the spec too.
///
/// Query: `?since=<endIndex>&fp=<prefixFp>` when the client already holds a
/// window, so it can be resumed with a `delta` instead of re-sent a snapshot;
/// see the module docs of [`crate::feed`] for the frames.
///
/// A socket opened by a login closes with 4401 once that login no longer
/// stands ([`crate::auth::login_ended`]).
pub async fn session_feed_ws(
    State(state): State<AppState>,
    who: crate::auth::MaybeIdentity,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<SessionFeedQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let resume = feed::ResumeFrom::parse(query.since.as_deref(), query.fp.as_deref());
    let login = crate::auth::login_of(&who);
    upgrade.on_upgrade(move |socket| run_session_socket(socket, state, source, id, resume, login))
}

/// Query for the session websocket.
///
/// Both strings, and read leniently by [`feed::ResumeFrom::parse`]: a socket a
/// browser cannot open looks to it like a network failure, so a malformed
/// marker must cost a snapshot, never the connection.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SessionFeedQuery {
    /// The `endIndex` of the window the client holds.
    pub since: Option<String>,
    /// The `prefixFp` it was last sent.
    pub fp: Option<String>,
}

const CLOSED_FRAME: &str = r#"{"type":"closed"}"#;

/// How often a live socket is pinged.
const PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a socket may go without a frame from the client before it is dropped.
///
/// Three missed pings. A browser answers a `Ping` with a `Pong` on its own, so a
/// tab that is still there always beats this; what does not is a connection that
/// died without a `FIN` — a laptop that slept, a NAT that forgot, a phone that
/// changed network. Those sockets would otherwise hold a feed watcher (and, for
/// a terminal, a pty) open until the process restarts.
pub(crate) const IDLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(90);

/// The keepalive half of a websocket loop.
///
/// Every socket in this server is server-to-client except for the terminal's
/// control frames, so without a ping there is nothing to notice a dead peer
/// with: a `send` to a black hole succeeds into the kernel buffer for a long
/// time before it ever fails.
pub struct Heartbeat {
    ticker: tokio::time::Interval,
    idle_limit: std::time::Duration,
    last_seen: tokio::time::Instant,
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new(PING_INTERVAL, IDLE_LIMIT)
    }
}

impl Heartbeat {
    /// A heartbeat on its own schedule; the defaults are [`Heartbeat::default`].
    pub fn new(interval: std::time::Duration, idle_limit: std::time::Duration) -> Self {
        let start = tokio::time::Instant::now();
        // `interval` fires immediately on its first tick, which would ping a
        // socket that has not finished opening; `interval_at` makes the first
        // one a real delay like every later one.
        let mut ticker = tokio::time::interval_at(start + interval, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Self {
            ticker,
            idle_limit,
            last_seen: start,
        }
    }

    /// Resolves when it is time to ping. `false` means the peer has been quiet
    /// for longer than the idle limit and the socket should be dropped.
    ///
    /// Cancel-safe: [`tokio::time::Interval::tick`] is, and nothing else here
    /// awaits.
    pub async fn tick(&mut self) -> bool {
        self.ticker.tick().await;
        self.last_seen.elapsed() < self.idle_limit
    }

    /// Note that the client is still there.
    pub fn saw_frame(&mut self) {
        self.last_seen = tokio::time::Instant::now();
    }
}

async fn run_session_socket(
    socket: WebSocket,
    state: AppState,
    source: Source,
    id: String,
    resume: Option<feed::ResumeFrom>,
    login: Option<(String, String)>,
) {
    let (mut sender, mut receiver) = socket.split();
    let ended = crate::auth::login_ended(state.drive.auth.clone(), login);
    tokio::pin!(ended);

    let Some(attached) = feed::attach_session_feed(&state.feeds, &state.drive, source, &id, resume.as_ref()).await else {
        let _ = sender.send(Message::Text(CLOSED_FRAME.into())).await;
        let _ = sender.close().await;
        return;
    };
    let mut attachment = attached.attachment;
    for frame in attached.opening {
        if sender.send(Message::Text(frame.into())).await.is_err() {
            return;
        }
    }

    let mut heartbeat = Heartbeat::default();
    loop {
        tokio::select! {
            () = &mut ended => {
                let _ = sender.send(Message::Close(Some(crate::auth::signed_out_frame()))).await;
                break;
            }
            frame = attachment.next_frame() => match frame {
                Some(frame) => {
                    if sender.send(Message::Text(frame.into())).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = sender.send(Message::Text(CLOSED_FRAME.into())).await;
                    break;
                }
            },
            // The client hung up, or sent something. A `Pong` lands here too,
            // which is what keeps the heartbeat happy. The one text frame the
            // feed answers is a `resync` from a client handed a delta it could
            // not place; anything else is ignored.
            incoming = receiver.next() => {
                heartbeat.saw_frame();
                match incoming {
                    None | Some(Ok(Message::Close(_))) | Some(Err(_)) => break,
                    Some(Ok(Message::Text(text))) => {
                        if let Some(frame) = attachment.client_frame(text.as_str()) {
                            if sender.send(Message::Text(frame.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
            alive = heartbeat.tick() => {
                if !alive || sender.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
            _ = state.cancel.cancelled() => {
                let _ = sender.send(Message::Text(CLOSED_FRAME.into())).await;
                break;
            }
        }
    }
    let _ = sender.close().await;
}

/// Everything gotcha cannot express about this module's own routes.
///
/// The `404`s exist because gotcha derives exactly one success response and
/// folds a `Result`'s error side into `default`, with no way to name a status
/// (`gotcha_core-0.4.1/src/responsible.rs:129`).
fn describe(reg: &mut ExtraResponses) {
    reg.describe(
        "GET",
        "/api/sessions",
        200,
        "Sessions from claude and codex, newest first; this server's own unless `scope` says otherwise",
    )
    .error("GET", "/api/sessions", 400, "Invalid limit")
    .describe("GET", "/api/settings", 200, "Server and data-source information")
    .describe(
        "GET",
        "/api/sessions/{source}/{id}",
        200,
        "Session state plus transcript delta (full on first call or reset)",
    )
    .error("GET", "/api/sessions/{source}/{id}", 404, "Session not found");
}

/// Every amendment every subsystem contributes, in one place.
fn response_overrides() -> ExtraResponses {
    let mut reg = ExtraResponses::default();
    describe(&mut reg);
    crate::accounts::describe(&mut reg);
    crate::apps::describe(&mut reg);
    crate::attention::describe(&mut reg);
    crate::auth::describe(&mut reg);
    crate::chats::describe(&mut reg);
    cloud::describe(&mut reg);
    crate::drive::describe(&mut reg);
    crate::feed::global::describe(&mut reg);
    crate::files::describe(&mut reg);
    crate::media::describe(&mut reg);
    crate::models::describe(&mut reg);
    crate::notify::describe(&mut reg);
    crate::plugins::routes::describe(&mut reg);
    crate::prefs::describe(&mut reg);
    crate::push::describe(&mut reg);
    crate::quota::describe(&mut reg);
    crate::schedule::describe(&mut reg);
    crate::screens::describe(&mut reg);
    crate::selfupdate::describe(&mut reg);
    crate::setup::describe(&mut reg);
    crate::stats::describe(&mut reg);
    crate::terminals::describe(&mut reg);
    reg
}

/// Register every route and the spec post-transform.
pub fn routes(router: ApiRouter) -> ApiRouter {
    let router = router
        .get("/api/sessions", list_sessions)
        .get("/api/sessions/{source}/{id}", session_detail)
        .get("/api/sessions/{source}/{id}/ws", session_feed_ws)
        .get("/api/settings", settings);
    let router = crate::accounts::routes(router);
    let router = crate::apps::routes(router);
    let router = crate::attention::routes(router);
    let router = crate::auth::routes(router);
    let router = crate::chats::routes(router);
    let router = cloud::routes(router);
    let router = crate::aggregator::gateway::routes(router);
    let router = crate::drive::routes(router);
    let router = crate::feed::global::routes(router);
    let router = crate::files::routes(router);
    let router = crate::media::routes(router);
    let router = crate::models::routes(router);
    let router = crate::notify::routes(router);
    let router = crate::plugins::routes::routes(router);
    let router = crate::prefs::routes(router);
    let router = crate::push::routes(router);
    let router = crate::quota::routes(router);
    let router = crate::schedule::routes(router);
    let router = crate::screens::routes(router);
    let router = crate::selfupdate::routes(router);
    let router = crate::setup::routes(router);
    let router = crate::stats::routes(router);
    let router = crate::terminals::routes(router);
    router
        .openapi(|mut spec| {
            spec.info.title = API_TITLE.to_string();
            spec.info.version = API_VERSION.to_string();
            spec.info.description = Some(API_DESCRIPTION.to_string());
            openapi_ext::apply(spec, &response_overrides())
        })
        .layer(crate::origin::layer())
        .fallback_service(crate::static_files::service())
}

/// What this server reports where the contract asks for `nodeVersion`.
pub const RUNTIME_VERSION: &str = concat!("rust/", env!("CARGO_PKG_VERSION"));

/// Exactly which build of the server this is.
///
/// The crate version alone cannot answer the question self-update asks —
/// "is the server I am talking to still the one this page loaded against?" —
/// because during development `0.1.0` is replaced by `0.1.0` all day. So the
/// commit is part of the identity, stamped in by `build.rs`; see there for where
/// it comes from and what happens when there is none.
///
/// This is not the OpenAPI document's version ([`API_VERSION`]), which is the
/// *contract*'s and must stay stable across commits or every build would
/// re-generate the exported spec.
pub const SERVER_VERSION: &str = if env!("DROPPEN_BUILD_REV").is_empty() {
    env!("CARGO_PKG_VERSION")
} else {
    concat!(env!("CARGO_PKG_VERSION"), "+g", env!("DROPPEN_BUILD_REV"))
};

/// Node's `process.platform` naming, so the field means the same thing it did.
pub(crate) fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

pub(crate) fn hostname() -> String {
    let mut buffer = vec![0u8; 256];
    // SAFETY: the buffer is owned, correctly sized, and the call only writes a
    // NUL-terminated name into it.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr() as *mut libc::c_char, buffer.len()) };
    if result != 0 {
        return String::new();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// What unit tests need of a whole server without serving one: its state
/// and its router, rooted in a scratch home (never the real one) and owning
/// nothing — no lock, so no adoption, no schedule runner, no quota probe.
#[cfg(test)]
pub(crate) mod test_support {
    use gotcha::{GotchaApp, GotchaContext};

    use crate::app::RemoteHandApp;

    fn app(home: &std::path::Path) -> RemoteHandApp {
        RemoteHandApp {
            home_override: Some(home.to_path_buf()),
            instance_override: Some("unittest".into()),
            port_override: Some(0),
            skip_lock: true,
            auth_disabled: false,
            auth_enabled_override: None,
            desktop_app_override: None,
        }
    }

    /// A server's whole router, guard included — what a request meets, minus
    /// a socket — with authentication as `auth_enabled` overrides it. For the
    /// tests of what the guard lets through, which would otherwise need a
    /// server on a port.
    pub(crate) async fn guarded_router(home: &std::path::Path, auth_enabled: Option<bool>) -> (super::AppState, gotcha::axum::Router) {
        let mut app = app(home);
        app.auth_enabled_override = auth_enabled;
        let config = app.config().await.expect("config");
        let state = app.state(&config).await.expect("state");
        let auth = state.drive.auth.clone();
        let router = app
            .build_router(GotchaContext {
                config,
                state: state.clone(),
            })
            .await
            .expect("router");
        (state, router.layer(crate::auth::layer(auth)))
    }

    /// The handlers' state for a server whose home is `home`.
    pub(crate) async fn state(home: &std::path::Path) -> super::AppState {
        let app = app(home);
        let config = app.config().await.expect("config");
        app.state(&config).await.expect("state")
    }

    /// A server whose home is `home`, its router as the server serves it —
    /// the authentication guard outermost, peers' addresses known — on a
    /// loopback port of its own: its state and where it listens. It stops
    /// with the test's runtime.
    pub(crate) async fn served(home: &std::path::Path) -> (super::AppState, std::net::SocketAddr) {
        served_with(home, |_| {}).await
    }

    /// [`served`], with the state adjusted before the router is built — a
    /// server the desktop app runs (`desktop_app`), say, whose switch is read
    /// once at boot and so cannot be flipped on a router already serving.
    pub(crate) async fn served_with(home: &std::path::Path, adjust: impl FnOnce(&mut super::AppState)) -> (super::AppState, std::net::SocketAddr) {
        let app = app(home);
        let config = app.config().await.expect("config");
        let mut state = app.state(&config).await.expect("state");
        adjust(&mut state);
        let auth = state.drive.auth.clone();
        let router = app
            .build_router(GotchaContext {
                config,
                state: state.clone(),
            })
            .await
            .expect("router")
            .layer(crate::auth::layer(auth));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        state.setup.set_listen_port(addr.port());
        tokio::spawn(async move {
            let _ = gotcha::axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
        });
        (state, addr)
    }

    /// Every route the server registers, as `(METHOD, path template)`, read
    /// off the router the server serves: its OpenAPI document lists every
    /// documented operation.
    pub(crate) async fn documented_routes(home: &std::path::Path) -> std::collections::BTreeSet<(String, String)> {
        use tower_service::Service;
        let app = app(home);
        let config = app.config().await.expect("config");
        let state = app.state(&config).await.expect("state");
        let mut router = app.build_router(GotchaContext { config, state }).await.expect("router");
        let request = gotcha::axum::http::Request::get("/openapi.json")
            .body(gotcha::axum::body::Body::empty())
            .expect("request");
        let response = router.call(request).await.expect("infallible");
        assert!(response.status().is_success(), "/openapi.json: {}", response.status());
        let bytes = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let spec: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let mut routes = std::collections::BTreeSet::new();
        const METHODS: [&str; 8] = ["get", "put", "post", "delete", "options", "head", "patch", "trace"];
        for (path, item) in spec["paths"].as_object().expect("paths") {
            for method in item.as_object().expect("path item").keys() {
                if METHODS.contains(&method.as_str()) {
                    routes.insert((method.to_ascii_uppercase(), path.clone()));
                }
            }
        }
        routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn heartbeat() -> Heartbeat {
        Heartbeat::new(Duration::from_secs(30), Duration::from_secs(90))
    }

    /// The first tick is a real delay: pinging the instant a socket opens would
    /// race the client's own handshake bookkeeping for nothing.
    #[tokio::test(start_paused = true)]
    async fn the_first_ping_waits_a_full_interval() {
        let start = tokio::time::Instant::now();
        assert!(heartbeat().tick().await);
        assert_eq!(start.elapsed(), Duration::from_secs(30));
    }

    /// A connection that died without a `FIN` — a slept laptop, a forgotten NAT
    /// binding — answers no ping. Three unanswered ones and it is dropped.
    #[tokio::test(start_paused = true)]
    async fn a_peer_that_never_answers_is_given_up_on() {
        let mut heartbeat = heartbeat();
        assert!(heartbeat.tick().await, "30s in, still within the idle limit");
        assert!(heartbeat.tick().await, "60s in, still within the idle limit");
        assert!(!heartbeat.tick().await, "90s of silence is the limit");
    }

    /// Any frame counts, which is what makes a browser's automatic `Pong` enough.
    #[tokio::test(start_paused = true)]
    async fn a_frame_from_the_client_resets_the_clock() {
        let mut heartbeat = heartbeat();
        assert!(heartbeat.tick().await);
        // A pong at 30s: the idle window now runs from there, not from connect.
        heartbeat.saw_frame();
        assert!(heartbeat.tick().await, "60s in, 30s since the frame");
        assert!(
            heartbeat.tick().await,
            "90s in, 60s since the frame — a clock from connect would give up here"
        );
        assert!(!heartbeat.tick().await, "120s in, 90s since the frame");
    }
}
