//! The HTTP + websocket surface.
//!
//! Paths are written flat rather than assembled with [`gotcha::GotchaRouter::nest`],
//! because `nest` joins with an extra slash and documents `/api//sessions`
//! (`gotcha-0.4.1/src/router.rs:161`).


use std::sync::Arc;

use futures_util::StreamExt;
use gotcha::axum::extract::ws::{Message, WebSocket};
use gotcha::axum::response::Response;
use gotcha::{api, state, GotchaRouter, Json, Schematic, State, WebSocketUpgrade};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use droppen_drive::Drive;
use droppen_drive::feed;
use droppen_plugins::CloudIdentityExt;
use droppen_http::error::ApiError;
use droppen_http::extract::{ApiPath, ApiQuery};
use droppen_http::{close_within, hostname, node_platform, send_within, Heartbeat, CLOSE_LIMIT, RUNTIME_VERSION, SERVER_VERSION};
use droppen_http::openapi_ext::{self, ExtraResponses};
use droppen_instance::paths::Paths;
use droppen_provider_claude_code::sessions::claude;
use droppen_provider_codex::sessions::codex;
use droppen_sessions::{SessionDetail, SessionList, Settings, Source};

mod state;

/// The ACP agents' routes (`droppen_provider_acp::agents`), through the
/// router as the server builds it.
#[cfg(test)]
mod acp_agents;
/// The login guard and the `/api/auth/*` routes (`droppen_auth`), through the
/// router as the server builds it.
#[cfg(test)]
mod auth_routes;
/// The bridge's route allow-list (`droppen_agent::bridge::ROUTES`) against
/// every route the server registers.
#[cfg(test)]
mod bridge_routes;
/// The chat kinds (`droppen_drive::chats`) against the providers the server registers.
#[cfg(test)]
mod chat_kinds;
/// Cloud's routes and the remote-view gate (`droppen_cloud`) on the state the
/// server builds, its link control (`droppen_cloud::control`) with the link
/// `app` attaches, and the aggregator gateway (`droppen_cloud::gateway`) as
/// the server serves it.
#[cfg(test)]
mod cloud_control;
#[cfg(test)]
mod cloud_gateway;
#[cfg(test)]
mod cloud_routes;
/// The drive's routes, a session's feed, the session feeds' socket, the
/// session-list socket and the scheduled-task routes (`droppen_drive::routes`,
/// `droppen_drive::feed`, `droppen_drive::feed::mux`, `droppen_drive::feed::global`,
/// `droppen_drive::schedule`) on the drive and the router the server builds.
#[cfg(test)]
mod drive_routes;
#[cfg(test)]
mod feed_global;
#[cfg(test)]
mod feed_mux;
#[cfg(test)]
mod feed_session;
#[cfg(test)]
mod schedule_routes;

/// The plugin routes (`droppen_plugins::routes`) and the plugin assets
/// (`droppen_plugins::serve`), through the router as the server builds it.
#[cfg(test)]
mod plugins_routes;
#[cfg(test)]
mod plugins_serve;

/// The router every subsystem's `routes` function extends.
///
/// Spelled out once here so a subsystem's registration reads as one line rather
/// than three of generics.
pub type ApiRouter = GotchaRouter<gotcha::GotchaContext<AppState, droppen_http::config::AppConfig>>;

/// The title and version `openapi/openapi.json` carries.
const API_TITLE: &str = "remote-hand API";
const API_VERSION: &str = "0.1.0";
const API_DESCRIPTION: &str = "Browse local Claude Code and Codex sessions plus the local filesystem.";

/// Everything a handler needs, injected as `State<AppState>`.
#[state]
#[derive(Clone)]
pub struct AppState {
    /// The plugins installed in this instance, their switches and document
    /// stores.
    pub plugins: Arc<droppen_plugins::Plugins>,
    /// Where sessions are discovered.
    pub paths: Arc<Paths>,
    /// The instance directory this server owns, and the name it reports.
    pub instance: Arc<droppen_instance::instance::InstanceDirs>,
    /// When this process started, ISO-8601.
    pub started_at: Arc<str>,
    /// Every live session feed, so shutdown can tear them down.
    pub feeds: Arc<feed::FeedRegistry>,
    /// The shared session-list feed behind `GET /api/events`.
    pub global: Arc<feed::global::GlobalFeed>,
    /// The same feed's inputs, for the subsystems that sit below it and so
    /// may not name it.
    pub refresh: droppen_http::refresh::Refresh,
    /// Cancelled by the shutdown supervisor; live sockets close on it.
    pub cancel: CancellationToken,
    /// Accounts, injections, quota and composer preferences — the one field the
    /// process-driving subsystems add.
    pub drive: Arc<Drive>,
    /// Every `claude setup-token` run this server has started, which the token
    /// wizard's routes start, poll and finish.
    pub wizards: Arc<droppen_provider_claude_code::setup_token::Captures>,
    /// The ACP agents' routes' state: the ACP provider's runs, found among the
    /// providers once, when this state is built.
    pub(crate) acp_agents: droppen_provider_acp::agents::AcpAgentState,
    /// Every live web terminal, so shutdown can kill them.
    pub terminals: Arc<droppen_workbench::terminals::Registry>,
    /// Instant chats: the provider configuration and the API-backed store.
    pub chats: Arc<droppen_drive::chats::Chats>,
    /// Every running embedded app, so shutdown can kill them and the
    /// `/embed/{id}/*` proxy can find their ports.
    pub apps: Arc<droppen_workbench::apps::Registry>,
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
    pub lock: Arc<std::sync::OnceLock<std::sync::Weak<droppen_instance::instance::InstanceLock>>>,
    /// The hub link: started at boot for an enrolled instance, and at any
    /// time after by an enrollment from the local settings
    /// ([`droppen_cloud::control`]). Inert in a server that does not own its
    /// instance.
    pub cloud: Arc<droppen_cloud::Cloud>,
    /// Whether the Droppen desktop app runs this server (`DROPPEN_DESKTOP_APP`,
    /// see [`crate::desktop_app`]): no self-upgrade, and the app owns the login.
    pub desktop_app: bool,
    /// The first-run guide: tool detection and the children's `PATH`.
    pub setup: Arc<droppen_setup::Setup>,
    /// The port this server listens on, once it does: what a request from the
    /// app's own window names as its `Host` and `Origin`. Filled once, by the
    /// entry when its listener is bound; read by the desktop-window gate
    /// through [`AppState::window`]. Until then, and in a server that never
    /// binds, nothing counts as the app's window.
    pub listen_port: Arc<std::sync::OnceLock<u16>>,
    /// How long one frame on any of this server's websockets may take to
    /// leave before its socket is given up on: [`SEND_LIMIT`](droppen_http::SEND_LIMIT), except in a test
    /// that cannot wait that long.
    pub send_limit: std::time::Duration,
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
    pub scope: Option<droppen_sessions::ScopeFilter>,
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
    let drivers = state.drive.providers.driver_states();
    let mut sessions = state
        .drive
        .transcripts
        .list_and_filter(
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
///
/// A request bridged from droppen.app gets no plugin status — and so no
/// `entry`, which carries the instance's capability key — for the reason
/// `docs/plugins/runtime.md` §5 gives: the cloud sees neither the list, the
/// code nor the documents. The device's own pages still get it.
#[api(id = "settings", group = "meta", summary = "Server settings")]
pub async fn settings(State(state): State<AppState>, #[api(skip)] who: droppen_auth::MaybeIdentity) -> Json<Settings> {
    let paths = state.paths.as_ref();
    let bindings = state.drive.accounts.session_account_map().await;
    let (claude_sessions, codex_sessions) = tokio::join!(claude::list_claude_sessions(paths, &bindings), codex::list_codex_sessions(paths));
    // The agent's own status mirror reads this route as `cloud:agent-sync`, so
    // the hub's offline copy gets the same empty list (`docs/plugins/
    // runtime.md` §5).
    let plugins: Vec<droppen_plugins::wire::PluginStatus> = if who.is_cloud() {
        Vec::new()
    } else {
        state.plugins.status_async().await.into_iter().map(Into::into).collect()
    };
    Json(Settings {
        instance: state.instance.name().to_string(),
        server_id: droppen_instance::identity::ServerId::local(paths).to_string(),
        hostname: hostname(),
        platform: node_platform().to_string(),
        // The field name is the contract's; the value is this server's runtime.
        node_version: RUNTIME_VERSION.to_string(),
        server_version: SERVER_VERSION.to_string(),
        home_dir: paths.home.to_string_lossy().into_owned(),
        claude_dir: droppen_provider_claude_code::claude_roots::ClaudeRoots::of(&paths).local_projects().to_string_lossy().into_owned(),
        claude_managed_dir: droppen_provider_claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().to_string_lossy().into_owned(),
        codex_dir: paths.codex_sessions().to_string_lossy().into_owned(),
        claude_session_count: claude_sessions.len(),
        codex_session_count: codex_sessions.len(),
        server_started_at: state.started_at.to_string(),
        plugins,
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
/// see the module docs of [`droppen_drive::feed`] for the frames.
///
/// A socket opened by a login closes with 4401 once that login no longer
/// stands ([`droppen_auth::Auth::watch_login`]).
pub async fn session_feed_ws(
    State(state): State<AppState>,
    who: droppen_auth::MaybeIdentity,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<SessionFeedQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let resume = feed::ResumeFrom::parse(query.since.as_deref(), query.fp.as_deref());
    let login = who.login();
    upgrade.on_upgrade(move |socket| run_session_socket(socket, state, source, id, resume, login))
}

/// Query for the session websocket.
///
/// Both strings, and read leniently by [`droppen_drive::feed::ResumeFrom::parse`]: a socket a
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

async fn run_session_socket(
    socket: WebSocket,
    state: AppState,
    source: Source,
    id: String,
    resume: Option<feed::ResumeFrom>,
    login: Option<(String, String)>,
) {
    let (mut sender, mut receiver) = socket.split();
    let ended = state.drive.auth.clone().watch_login(login);
    tokio::pin!(ended);
    // Every send below is bounded ([`send_within`]): a client that stops
    // reading would otherwise park this task, and the feed it holds, for good.
    let (limit, shutdown) = (state.send_limit, &state.cancel);

    let Some(attached) = feed::attach_session_feed(&state.feeds, &state.drive, source, &id, resume.as_ref()).await else {
        let _ = send_within(&mut sender, Message::Text(CLOSED_FRAME.into()), limit, shutdown).await;
        close_within(&mut sender, CLOSE_LIMIT, shutdown).await;
        return;
    };
    let mut attachment = attached.attachment;
    for frame in attached.opening {
        if !send_within(&mut sender, Message::Text(frame.into()), limit, shutdown).await {
            return;
        }
    }

    let mut heartbeat = Heartbeat::default();
    loop {
        tokio::select! {
            () = &mut ended => {
                let _ = send_within(&mut sender, Message::Close(Some(droppen_auth::signed_out_frame())), limit, shutdown).await;
                break;
            }
            frame = attachment.next_frame() => match frame {
                Some(frame) => {
                    if !send_within(&mut sender, Message::Text(frame.into()), limit, shutdown).await {
                        break;
                    }
                }
                None => {
                    let _ = send_within(&mut sender, Message::Text(CLOSED_FRAME.into()), limit, shutdown).await;
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
                            if !send_within(&mut sender, Message::Text(frame.into()), limit, shutdown).await {
                                break;
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
            alive = heartbeat.tick() => {
                if !alive || !send_within(&mut sender, Message::Ping(Default::default()), limit, shutdown).await {
                    break;
                }
            }
            _ = state.cancel.cancelled() => {
                let _ = send_within(&mut sender, Message::Text(CLOSED_FRAME.into()), limit, shutdown).await;
                break;
            }
        }
    }
    // Nothing more is sent from the feed, so it is let go now rather than
    // for as long as the close takes.
    drop(attachment);
    close_within(&mut sender, CLOSE_LIMIT, shutdown).await;
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
    droppen_accounts::describe(&mut reg);
    droppen_provider_claude_code::setup_token::describe(&mut reg);
    droppen_provider_claude_code::usage_login::describe(&mut reg);
    droppen_provider_acp::agents::describe(&mut reg);
    droppen_workbench::apps::describe(&mut reg);
    droppen_sessions::attention::routes::describe(&mut reg);
    droppen_auth::describe(&mut reg);
    droppen_drive::chats::describe(&mut reg);
    droppen_cloud::describe(&mut reg);
    droppen_drive::describe(&mut reg);
    droppen_drive::feed::global::describe(&mut reg);
    droppen_files::describe(&mut reg);
    droppen_files::media::describe(&mut reg);
    droppen_provider::models::describe(&mut reg);
    droppen_notify::notify::describe(&mut reg);
    droppen_plugins::routes::describe(&mut reg);
    droppen_workbench::prefs::describe(&mut reg);
    droppen_notify::push::describe(&mut reg);
    droppen_accounts::quota::describe(&mut reg);
    droppen_drive::schedule::describe(&mut reg);
    droppen_workbench::screens::describe(&mut reg);
    droppen_supervisor::selfupdate::describe(&mut reg);
    droppen_setup::describe(&mut reg);
    droppen_sessions::stats::describe(&mut reg);
    droppen_workbench::terminals::describe(&mut reg);
    reg
}

/// Register every route and the spec post-transform.
pub fn routes(router: ApiRouter) -> ApiRouter {
    let router = router
        .get("/api/sessions", list_sessions)
        .get("/api/sessions/{source}/{id}", session_detail)
        .get("/api/sessions/{source}/{id}/ws", session_feed_ws)
        .get("/api/settings", settings);
    let router = droppen_accounts::AccountsState::routes(router);
    let router = droppen_provider_claude_code::setup_token::TokenWizardState::routes(router);
    let router = droppen_provider_claude_code::usage_login::UsageLoginState::routes(router);
    let router = droppen_provider_acp::agents::AcpAgentState::routes(router);
    let router = droppen_workbench::apps::AppsState::routes(router);
    let router = droppen_sessions::attention::routes::AttentionState::routes(router);
    let router = droppen_auth::AuthState::routes(router);
    let router = droppen_drive::chats::ChatsState::routes(router);
    let router = droppen_cloud::CloudState::routes(router);
    let router = droppen_cloud::gateway::routes(router);
    let router = droppen_drive::DriveState::routes(router);
    let router = droppen_drive::feed::global::EventsState::routes(router);
    let router = droppen_files::FilesState::routes(router);
    let router = droppen_files::media::routes(router);
    let router = droppen_provider::models::ModelsState::routes(router);
    let router = droppen_notify::notify::NotifyState::routes(router);
    let router = droppen_plugins::routes::PluginsState::routes(router);
    let router = droppen_workbench::prefs::PrefsState::routes(router);
    let router = droppen_notify::push::PushState::routes(router);
    let router = droppen_accounts::quota::QuotaState::routes(router);
    let router = droppen_drive::schedule::ScheduleState::routes(router);
    let router = droppen_workbench::screens::ScreensState::routes(router);
    let router = droppen_supervisor::selfupdate::SelfUpdateState::routes(router);
    let router = droppen_setup::SetupState::routes(router);
    let router = droppen_sessions::stats::StatsState::routes(router);
    let router = droppen_workbench::terminals::TerminalsState::routes(router);
    router
        .openapi(|mut spec| {
            spec.info.title = API_TITLE.to_string();
            spec.info.version = API_VERSION.to_string();
            spec.info.description = Some(API_DESCRIPTION.to_string());
            let spec = openapi_ext::apply(spec, &response_overrides());
            // Every JSON object sorted: gotcha builds some of the spec's maps (a
            // schema's `properties`) from hash maps, and a `Value` keeps the
            // order it was given (`preserve_order`), so each server would
            // otherwise serve its own order.
            let Ok(mut value) = serde_json::to_value(&spec) else { return spec };
            value.sort_all_objects();
            serde_json::from_value(value).unwrap_or(spec)
        })
        .layer(droppen_http::origin::layer())
        .fallback_service(crate::static_files::service())
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
        (state, router.layer(droppen_auth::layer(auth)))
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
            .layer(droppen_auth::layer(auth));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let _ = state.listen_port.set(addr.port());
        tokio::spawn(async move {
            let _ = gotcha::axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
        });
        (state, addr)
    }

    /// A websocket ticket for the device's owner, whom a server with no
    /// account yet lets in.
    pub(crate) async fn owner_ticket(state: &super::AppState) -> String {
        let auth = &state.drive.auth;
        let token = auth.sessions.issue(droppen_auth::LOCAL_OWNER, None).await.expect("an owner login").token;
        let digest = droppen_auth::sessions::digest_of(&token);
        auth.tickets
            .issue(droppen_auth::LOCAL_OWNER, &digest, droppen_auth::tickets::TicketScope::Ws)
            .expect("a ticket")
            .0
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

    /// A bridged caller is droppen.app — and the agent's own status mirror,
    /// which reads this route as `cloud:agent-sync` and is what the hub's
    /// offline copy is made from. Neither may learn of a plugin
    /// (`docs/plugins/runtime.md` §5); a local page still may.
    #[tokio::test]
    async fn a_bridged_settings_says_there_are_no_plugins() {
        let home = tempfile::tempdir().unwrap();
        let mut state = test_support::state(home.path()).await;
        state.plugins = Arc::new(droppen_plugins::testing::installed_host(
            home.path(),
            &["tasks"],
            droppen_http::config::PluginSettings::default(),
            None,
        ));

        let local = settings(State(state.clone()), droppen_auth::MaybeIdentity(None)).await.0;
        assert_eq!(local.plugins.len(), 1, "a local caller sees the instance's plugins");

        for device in ["agent-sync", "d-1"] {
            let bridged = settings(
                State(state.clone()),
                droppen_auth::MaybeIdentity(Some(droppen_http::identity::AuthIdentity {
                    username: format!("cloud:{device}"),
                    token_sha256: None,
                })),
            )
            .await
            .0;
            assert!(
                bridged.plugins.is_empty(),
                "a bridged caller must not learn of any plugin: {device}"
            );
        }
    }

    const WIDE_ID: &str = "44444444-4444-4444-8444-444444444444";

    /// `turns` claude turns from turn `from` on, each a question and an answer
    /// of `width` characters.
    fn wide_turns(from: usize, turns: usize, width: usize) -> String {
        let mut out = String::new();
        for turn in from..from + turns {
            let stamp = format!("2026-09-29T00:{:02}:{:02}.000Z", (turn / 60) % 60, turn % 60);
            for (kind, letter) in [("user", "q"), ("assistant", "a")] {
                let content = serde_json::json!([{ "type": "text", "text": letter.repeat(width) }]);
                let line = serde_json::json!({
                    "type": kind,
                    "message": { "role": kind, "content": content },
                    "timestamp": stamp,
                    "cwd": "/tmp",
                    "sessionId": WIDE_ID,
                    "uuid": format!("{letter}{turn}"),
                });
                out.push_str(&line.to_string());
                out.push('\n');
            }
        }
        out
    }

    /// The session [`WIDE_ID`] in `state`'s home, holding `turns` turns of
    /// `width`; its transcript's path.
    fn wide_session(state: &AppState, turns: usize, width: usize) -> std::path::PathBuf {
        let project = droppen_provider_claude_code::claude_roots::ClaudeRoots::of(&state.paths).local_projects().join("-w-wide");
        std::fs::create_dir_all(&project).expect("project");
        let file = project.join(format!("{WIDE_ID}.jsonl"));
        std::fs::write(&file, wide_turns(0, turns, width)).expect("transcript");
        file
    }

    /// The session feed socket of [`WIDE_ID`], opened and then never read from.
    async fn never_reading_feed(
        state: &AppState,
        addr: std::net::SocketAddr,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
        let ticket = test_support::owner_ticket(state).await;
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/sessions/claude/{WIDE_ID}/ws?ticket={ticket}"))
            .await
            .expect("let in")
            .0
    }

    /// Wait for `done`, and fail with `what` if it takes longer than `within`.
    async fn until(what: &str, within: Duration, done: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + within;
        while !done() {
            assert!(tokio::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A client that stops reading cannot hold its session feed for good: a
    /// frame it makes no room for gives up at the server's limit, and the
    /// socket task lets the feed go before its short close.
    ///
    /// Real sockets, and a client that never reads after connecting. Its
    /// opening snapshot is small and goes out; then the transcript grows by a
    /// page of wide turns, a frame of megabytes that the kernel's buffers do
    /// not hold, and every byte the server sends after that has to wait.
    #[tokio::test]
    async fn a_session_feed_client_that_never_reads_is_given_up_on() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, addr) = test_support::served_with(home.path(), |state| state.send_limit = Duration::from_secs(1)).await;
        let file = wide_session(&state, 1, 10);
        let _client = never_reading_feed(&state, addr).await;
        until("the socket attaches to the feed", Duration::from_secs(30), || state.feeds.len() == 1).await;

        let page = wide_turns(1, droppen_drive::feed::SESSION_PAGE_SIZE / 2, droppen_sessions::util::TEXT_LIMIT);
        let mut transcript = std::fs::OpenOptions::new().append(true).open(&file).expect("open");
        std::io::Write::write_all(&mut transcript, page.as_bytes()).expect("append");
        until(
            "a frame that cannot leave gives up at the limit, and the feed is let go",
            Duration::from_secs(60),
            || state.feeds.is_empty(),
        )
        .await;
    }

    /// A server shutting down does not wait on a session feed client that has
    /// stopped reading: the send it is stuck on gives way at once, well inside
    /// the real limit.
    #[tokio::test]
    async fn a_shutdown_does_not_wait_for_a_session_feed_client_that_never_reads() {
        let home = tempfile::tempdir().expect("tempdir");
        let (state, addr) = test_support::served(home.path()).await;
        assert_eq!(state.send_limit, droppen_http::SEND_LIMIT);
        // An opening snapshot of megabytes, more than the kernel's buffers hold.
        wide_session(&state, droppen_drive::feed::SESSION_PAGE_SIZE / 2, droppen_sessions::util::TEXT_LIMIT);
        let _client = never_reading_feed(&state, addr).await;
        until("the socket attaches to the feed", Duration::from_secs(30), || state.feeds.len() == 1).await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        state.cancel.cancel();
        until("a shutdown lets the stuck socket go at once", Duration::from_secs(10), || state.feeds.is_empty()).await;
    }
}
