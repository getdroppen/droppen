//! The global feed: one websocket carrying every session list change.
//!
//! Port of the former TypeScript server's `global-feed.ts` plus its socket in
//! `index.ts`. A client connects to `/api/events` once and receives
//! `settings`, then a `sessions:snapshot`, then a `sessions:delta` whenever the
//! list changes — which is what lets the sidebar stay live without polling
//! `/api/sessions` (`web/src/app/global-feed.tsx`). The same socket carries
//! `screens:changed` / `screens:list` after every screen write
//! ([`GlobalFeed::publish`]), so another device's open layout follows along.
//!
//! The watcher only runs while somebody is listening: it is started by the first
//! client and torn down by the last, so an idle server holds no recursive
//! filesystem watches. The 5 s poll alongside it is not redundant — a live CLI
//! process exiting changes a session's status without touching any file the
//! watcher can see.
//!
//! Unlike the TypeScript, the state is owned rather than module-level (the same
//! reasoning as `feed::FeedRegistry`): two servers in one test process must not
//! share, or tear down, each other's watcher.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gotcha::axum::extract::ws::{Message, WebSocket};
use gotcha::axum::response::Response;
use gotcha::{State, WebSocketUpgrade};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::mux::{encode, hello_frame, parse_client_message, MuxOptions, Subscriptions};
use super::watch;
use crate::api::extract::ApiQuery;
use crate::api::{ApiRouter, AppState};
use crate::drive::Drive;
use crate::openapi_ext::ExtraResponses;
use crate::sessions::{self, Paths, SessionSummary, Source};

/// Status changes (a live process exiting) touch no file; poll cheaply for them.
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// How long filesystem events are coalesced before re-listing.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
/// A mutation asks for a much prompter refresh than a filesystem event does.
const REQUEST_DEBOUNCE: Duration = Duration::from_millis(50);
/// A newly spawned session takes a moment to write its first transcript line, so
/// a caller that named the session it expects gets these extra look-ups.
const DISCOVERY_RETRY: [Duration; 4] = [
    Duration::from_millis(250),
    Duration::from_millis(750),
    Duration::from_millis(1500),
    Duration::from_millis(3000),
];
/// How many frames a slow client may fall behind before it is resynchronised.
const CHANNEL_CAPACITY: usize = 64;

/// The feed's shared state, alive only while a client is attached.
struct Live {
    tx: broadcast::Sender<Arc<str>>,
    requests: mpsc::Sender<Option<String>>,
    cancel: CancellationToken,
    clients: usize,
    /// `source/id` -> serialized summary, for diffing.
    seen: HashMap<String, String>,
    /// The newest list, so a second client attaches without re-scanning.
    list: Vec<SessionSummary>,
}

/// Every client of the global feed, and the watcher they share.
#[derive(Default)]
pub struct GlobalFeed {
    live: Mutex<Option<Live>>,
}

impl GlobalFeed {
    /// An idle feed, ready to be shared by every handler.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Whether the watcher is running (i.e. at least one client is attached).
    pub fn is_running(&self) -> bool {
        self.live.lock().expect("global feed poisoned").is_some()
    }

    /// Ask for a prompt re-list, optionally naming a session the caller has just
    /// created and expects to appear.
    ///
    /// This is the hook every mutating route uses (`requestGlobalRefresh` in the
    /// TypeScript). With nobody listening there is nothing to refresh, so it is
    /// a no-op rather than an error.
    pub fn request_refresh(&self, expected: Option<(Source, &str)>) {
        let live = self.live.lock().expect("global feed poisoned");
        let Some(live) = live.as_ref() else { return };
        let key = expected.map(|(source, id)| format!("{source}/{id}"));
        // A full queue already means "a refresh is pending", so a drop here
        // loses nothing but the discovery retries, which only speed things up.
        let _ = live.requests.try_send(key);
    }

    /// Push a frame that is not about the session list to every attached client.
    ///
    /// For the stores that change by request rather than on disk — today the
    /// screens (`screens:changed`, `screens:list`). Nothing re-lists or diffs: the
    /// writer already knows exactly what moved, and the frame says so. With
    /// nobody listening there is nobody to tell, so it is a no-op; a client that
    /// attaches later reads the store itself. A client that falls behind is sent
    /// a `sessions:snapshot`, which the web also takes as its cue to re-read the
    /// screens it may have missed news of.
    pub fn publish(&self, frame: String) {
        let live = self.live.lock().expect("global feed poisoned");
        if let Some(live) = live.as_ref() {
            let _ = live.tx.send(Arc::from(frame.as_str()));
        }
    }

    /// A receiver on this feed with no watcher behind it, for tests that only
    /// want to see what [`Self::publish`] sends.
    #[cfg(test)]
    pub(crate) fn listen(&self) -> broadcast::Receiver<Arc<str>> {
        let mut guard = self.live.lock().expect("global feed poisoned");
        if let Some(live) = guard.as_mut() {
            live.clients += 1;
            return live.tx.subscribe();
        }
        let (tx, rx) = broadcast::channel(CHANNEL_CAPACITY);
        let (requests, _) = mpsc::channel(1);
        *guard = Some(Live {
            tx,
            requests,
            cancel: CancellationToken::new(),
            clients: 1,
            seen: HashMap::new(),
            list: Vec::new(),
        });
        rx
    }

    /// Stop the watcher and forget the list. Called when the last client leaves,
    /// and by the shutdown supervisor.
    pub fn shutdown(&self) {
        let mut live = self.live.lock().expect("global feed poisoned");
        if let Some(live) = live.take() {
            live.cancel.cancel();
        }
    }

    fn has_seen(&self, key: &str) -> bool {
        self.live
            .lock()
            .expect("global feed poisoned")
            .as_ref()
            .is_some_and(|live| live.seen.contains_key(key))
    }

    fn detach(&self) {
        let mut guard = self.live.lock().expect("global feed poisoned");
        let Some(live) = guard.as_mut() else { return };
        live.clients = live.clients.saturating_sub(1);
        if live.clients == 0 {
            if let Some(live) = guard.take() {
                live.cancel.cancel();
            }
        }
    }
}

/// A client's handle on the feed; dropping it may stop the watcher.
pub struct GlobalAttachment {
    feed: Arc<GlobalFeed>,
    rx: broadcast::Receiver<Arc<str>>,
}

impl Drop for GlobalAttachment {
    fn drop(&mut self) {
        self.feed.detach();
    }
}

impl GlobalAttachment {
    /// Wait for the next frame, or `None` once the feed has stopped.
    ///
    /// A client that fell so far behind that the buffer lapped it is given a
    /// fresh snapshot rather than being dropped: the deltas it missed are
    /// unrecoverable, but the current list is not.
    pub async fn next_frame(&mut self) -> Option<String> {
        match self.rx.recv().await {
            Ok(frame) => Some(frame.to_string()),
            Err(broadcast::error::RecvError::Closed) => None,
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let list = {
                    let live = self.feed.live.lock().expect("global feed poisoned");
                    live.as_ref().map(|live| live.list.clone())?
                };
                Some(snapshot_frame(&list))
            }
        }
    }
}

/// The opening frames plus the handle that yields every later one.
pub struct Attached {
    /// `settings` then `sessions:snapshot`, in that order.
    pub opening: Vec<String>,
    /// The live handle.
    pub attachment: GlobalAttachment,
}

/// Attach a client, starting the watcher if this is the first one.
///
/// [`Drive`] rather than [`Paths`] because a session row carries its account
/// binding, and the binding lives in the account store. `instance` is passed in
/// for the same reason `started_at` is: it describes the *server*, which a feed
/// knows nothing about.
///
/// `plugins` is the instance's plugin list as of the attach; a later switch
/// arrives as its own `plugins` frame.
pub async fn attach(
    feed: &Arc<GlobalFeed>,
    drive: &Arc<Drive>,
    instance: &str,
    started_at: &str,
    plugins: &[droppen_plugin_sdk::PluginStatus],
) -> Attached {
    let paths = drive.paths.as_ref();
    let rx = {
        let mut guard = feed.live.lock().expect("global feed poisoned");
        match guard.as_mut() {
            Some(live) => {
                live.clients += 1;
                live.tx.subscribe()
            }
            None => {
                let (tx, rx) = broadcast::channel(CHANNEL_CAPACITY);
                let (requests, request_rx) = mpsc::channel(CHANNEL_CAPACITY);
                let cancel = CancellationToken::new();
                *guard = Some(Live {
                    tx: tx.clone(),
                    requests,
                    cancel: cancel.clone(),
                    clients: 1,
                    seen: HashMap::new(),
                    list: Vec::new(),
                });
                tokio::spawn(run(feed.clone(), drive.clone(), tx, cancel, request_rx));
                rx
            }
        }
    };

    // First subscriber since the feed started: build the initial list. A second
    // client attaching meanwhile reuses it rather than scanning again.
    let needs_initial = feed
        .live
        .lock()
        .expect("global feed poisoned")
        .as_ref()
        .is_some_and(|live| live.list.is_empty());
    if needs_initial {
        let sessions = collect_sessions(drive).await;
        let mut guard = feed.live.lock().expect("global feed poisoned");
        if let Some(live) = guard.as_mut() {
            if live.list.is_empty() {
                live.seen = index(&sessions);
                live.list = sessions;
            }
        }
    }

    let snapshot = {
        let guard = feed.live.lock().expect("global feed poisoned");
        snapshot_frame(guard.as_ref().map(|live| live.list.as_slice()).unwrap_or_default())
    };

    Attached {
        opening: vec![settings_frame(paths, instance, started_at, plugins), snapshot],
        attachment: GlobalAttachment { feed: feed.clone(), rx },
    }
}

/// Every session both CLIs know about, newest first — the whole list, unfiltered,
/// because a feed client filters client-side.
///
/// One stream, both scopes. Splitting the feed in two would mean two watchers,
/// two diffs and a client that has to merge them to notice that adopting a
/// session moved it from one to the other; instead every row says which scope it
/// is in, and an adoption arrives as an ordinary upsert.
async fn collect_sessions(drive: &Arc<Drive>) -> Vec<SessionSummary> {
    let bindings = drive.accounts.session_account_map().await;
    // The sidebar is the one surface that shows every session at once, so it is
    // the one that most needs to be right about which of them are working.
    let drivers = drive.injections.driver_states();
    sessions::list_and_filter(&drive.paths, &bindings, &drivers, None, None, sessions::ScopeFilter::All).await
}

/// [`crate::sessions::SessionRef::local_key`] -> serialized summary, the form
/// the diff compares.
fn index(sessions: &[SessionSummary]) -> HashMap<String, String> {
    sessions
        .iter()
        .map(|s| (key_of(s), serde_json::to_string(s).unwrap_or_default()))
        .collect()
}

/// The key a row is filed under, and the one the `removed:` array of a
/// `sessions:delta` frame carries — so it is a contract with the web client,
/// not an implementation detail. Server-free for that reason.
fn key_of(summary: &SessionSummary) -> String {
    crate::sessions::reference::local_key(summary.source, &summary.id)
}

/// The one task per feed that watches, polls, and pushes deltas.
async fn run(
    feed: Arc<GlobalFeed>,
    drive: Arc<Drive>,
    tx: broadcast::Sender<Arc<str>>,
    cancel: CancellationToken,
    mut requests: mpsc::Receiver<Option<String>>,
) {
    let mut events = watch::watch_dirs(&watch_targets(drive.paths.as_ref()).await, WATCH_DEBOUNCE);
    let mut poll = tokio::time::interval(REFRESH_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    poll.tick().await; // the first tick is immediate

    // A refresh a mutation asked for, and the look-ups that chase a session
    // which has not shown up on disk yet.
    let mut due: Option<Instant> = None;
    let mut retries: Vec<(Instant, String)> = Vec::new();

    loop {
        let idle = Instant::now() + Duration::from_secs(3600);
        let next_due = due.unwrap_or(idle);
        let next_retry = retries.iter().map(|(at, _)| *at).min().unwrap_or(idle);

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = poll.tick() => refresh(&feed, &drive, &tx).await,
            _ = tokio::time::sleep_until(next_due) => {
                due = None;
                refresh(&feed, &drive, &tx).await;
            }
            _ = tokio::time::sleep_until(next_retry) => {
                let Some(index) = retries.iter().position(|(at, _)| *at == next_retry) else { continue };
                let (_, key) = retries.remove(index);
                if !feed.has_seen(&key) {
                    refresh(&feed, &drive, &tx).await;
                }
            }
            request = requests.recv() => match request {
                // Every sender is gone: the feed has been torn down.
                None => break,
                Some(expected) => {
                    due = Some(Instant::now() + REQUEST_DEBOUNCE);
                    if let Some(key) = expected {
                        retries.extend(DISCOVERY_RETRY.iter().map(|delay| (Instant::now() + *delay, key.clone())));
                    }
                }
            },
            event = events.recv() => {
                if event.is_none() {
                    // The watcher died; the poll backstop still refreshes.
                    events = watch::never();
                    continue;
                }
                // `watch_dirs` has already coalesced the burst.
                refresh(&feed, &drive, &tx).await;
            }
        }
    }

    tracing::debug!("global feed watcher stopped");
}

/// The directories a session file can appear in.
///
/// On a fresh install the managed root does not exist yet, and a watch on a
/// missing directory simply fails — leaving account sessions with no watcher at
/// all until a restart. Creating an empty projects directory is cheap and
/// harmless; an unwritable home just falls back to the poll interval.
async fn watch_targets(paths: &Paths) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = paths.claude_projects_dirs().into_iter().map(|(_, dir)| dir).collect();
    for dir in &dirs {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    dirs.push(paths.codex_sessions());
    dirs
}

/// Re-list, diff against what clients hold, and push a delta if anything moved.
async fn refresh(feed: &Arc<GlobalFeed>, drive: &Arc<Drive>, tx: &broadcast::Sender<Arc<str>>) {
    let sessions = collect_sessions(drive).await;
    let next = index(&sessions);

    let mut guard = feed.live.lock().expect("global feed poisoned");
    let Some(live) = guard.as_mut() else { return };

    if let Some(frame) = delta_frame(&live.seen, &sessions, &next) {
        let _ = tx.send(Arc::from(frame.as_str()));
    }
    live.seen = next;
    live.list = sessions;
}

/// The `sessions:delta` a client holding `seen` needs to arrive at `sessions`,
/// or `None` when nothing it can see has moved.
///
/// A row is an upsert whenever its serialisation changed at all, so every field
/// of a summary — the unread count as much as the title — travels the moment it
/// moves, with no list of "fields worth pushing" to keep in step.
fn delta_frame(seen: &HashMap<String, String>, sessions: &[SessionSummary], next: &HashMap<String, String>) -> Option<String> {
    let upserts: Vec<&SessionSummary> = sessions
        .iter()
        .filter(|s| {
            let key = key_of(s);
            seen.get(&key) != next.get(&key)
        })
        .collect();
    let mut removed: Vec<&String> = seen.keys().filter(|key| !next.contains_key(*key)).collect();
    removed.sort();
    if upserts.is_empty() && removed.is_empty() {
        return None;
    }
    Some(serde_json::json!({ "type": "sessions:delta", "upserts": upserts, "removed": removed }).to_string())
}

/// `{"type":"settings", …}` — the server description the sidebar shows.
///
/// Every field of the `/api/settings` body except the two session counts, which
/// would mean scanning both CLIs' corpora on every reconnect. That relationship
/// is the contract — a client reads whichever of the two it has — and
/// `the_settings_frame_carries_every_field_the_settings_route_does` pins it.
fn settings_frame(paths: &Paths, instance: &str, started_at: &str, plugins: &[droppen_plugin_sdk::PluginStatus]) -> String {
    serde_json::json!({
        "type": "settings",
        // Which of possibly several servers on this machine this socket reached.
        // Everything that survives a restart is scoped to the instance, so a
        // client watching the feed has to be able to say which one it is
        // watching.
        "instance": instance,
        // The same name, as a session reference scopes it. See
        // `crate::sessions::Settings::server_id`.
        "serverId": crate::sessions::ServerId::local(paths).to_string(),
        "hostname": crate::api::hostname(),
        "platform": crate::api::node_platform(),
        "nodeVersion": crate::api::RUNTIME_VERSION,
        // The one field a client re-reads *for* its value rather than to
        // display it: a reconnect that brings back a different build is how the
        // web knows a self-upgrade happened underneath it.
        "serverVersion": crate::api::SERVER_VERSION,
        "homeDir": paths.home.to_string_lossy(),
        "claudeDir": paths.claude_projects().to_string_lossy(),
        "claudeManagedDir": paths.claude_managed_projects().to_string_lossy(),
        "codexDir": paths.codex_sessions().to_string_lossy(),
        "serverStartedAt": started_at,
        // Which plugins are on decides which parts of the app exist, so it
        // has to be here before anything renders; `plugins` frames follow it.
        "plugins": plugins,
    })
    .to_string()
}

fn snapshot_frame(sessions: &[SessionSummary]) -> String {
    serde_json::json!({ "type": "sessions:snapshot", "sessions": sessions }).to_string()
}

/// The live session-list feed. Deliberately **not** annotated with `#[api]`:
/// `WebSocketUpgrade` is not a `ParameterProvider`, so the macro's generated
/// call fails to compile (`gotcha_macro-0.4.1/src/route.rs:88`). The TypeScript
/// server leaves this route out of its spec too — it is registered in
/// the former TypeScript server's `index.ts`, outside the OpenAPI app.
///
/// `?v=2` turns on the session subscriptions of [`super::mux`], and `&z=1`
/// compression of large frames; without them the socket is exactly what it
/// always was.
///
/// A socket opened by a login (a bearer token, or a ticket that names the
/// login that minted it) looks again every [`LOGIN_RECHECK`] at whether that
/// login is still signed in, and closes with 4401 once it is not: signing out
/// or revoking a login ends its live feed too, not only its next request.
pub async fn events_ws(
    State(state): State<AppState>,
    who: crate::auth::MaybeIdentity,
    ApiQuery(query): ApiQuery<EventsQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let options = MuxOptions::from_query(query.v.as_deref(), query.z.as_deref());
    let login = who.0.and_then(|identity| identity.token_sha256.map(|digest| (identity.username, digest)));
    upgrade.on_upgrade(move |socket| run_socket(socket, state, options, login))
}

/// How often a feed socket opened by a login looks again at whether that
/// login is still signed in.
pub const LOGIN_RECHECK: Duration = Duration::from_secs(5);

/// Query for the global socket; see [`super::mux`]. Strings, read leniently,
/// for the reason the session socket's are: a socket a browser cannot open
/// looks like a network failure, so a bad value must cost a feature, never the
/// connection.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct EventsQuery {
    /// `2` for the subscription protocol.
    pub v: Option<String>,
    /// `1` to have large frames compressed.
    pub z: Option<String>,
}

/// `login`: the account and its login's digest, when a login opened it.
async fn run_socket(socket: WebSocket, state: AppState, options: MuxOptions, login: Option<(String, String)>) {
    let (mut sender, mut receiver) = socket.split();
    let compress = options.compress;
    if options.enabled && sender.send(Message::Text(hello_frame(options).into())).await.is_err() {
        return;
    }
    let plugins = state.plugins.status_async().await;
    let attached = attach(&state.global, &state.drive, state.instance.name(), state.started_at.as_ref(), &plugins).await;
    let mut attachment = attached.attachment;
    for frame in attached.opening {
        if sender.send(encode(frame, compress)).await.is_err() {
            return;
        }
    }

    // Declared after the attachment so that it is dropped first: every
    // subscription detaches before the list feed does.
    let (mut subscriptions, mut session_frames) = Subscriptions::new(state.feeds.clone(), state.drive.clone());
    let mut heartbeat = crate::api::Heartbeat::default();
    let mut recheck = tokio::time::interval_at(Instant::now() + LOGIN_RECHECK, LOGIN_RECHECK);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut signed_out = false;
    loop {
        tokio::select! {
            _ = recheck.tick(), if login.is_some() => {
                if let Some((user, digest)) = &login {
                    if !state.drive.auth.login_stands(user, digest).await {
                        signed_out = true;
                        break;
                    }
                }
            }
            frame = attachment.next_frame() => match frame {
                Some(frame) => {
                    if sender.send(encode(frame, compress)).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            // The subscriptions' frames, already tagged. The queue never
            // closes while `subscriptions` holds its sender.
            Some(frame) = session_frames.recv() => {
                if sender.send(encode(frame, compress)).await.is_err() {
                    break;
                }
            }
            // The client hung up, or sent something. A `Pong` lands here too,
            // which is what keeps the heartbeat happy. Text is only read on a
            // socket that asked for subscriptions; on any other the feed is
            // read-only, as it always was.
            incoming = receiver.next() => {
                heartbeat.saw_frame();
                match incoming {
                    None | Some(Ok(Message::Close(_))) | Some(Err(_)) => break,
                    Some(Ok(Message::Text(text))) if options.enabled => {
                        let replies = parse_client_message(text.as_str()).map(|message| subscriptions.handle(message)).unwrap_or_default();
                        let mut gone = false;
                        for reply in replies {
                            if sender.send(Message::Text(reply.into())).await.is_err() {
                                gone = true;
                                break;
                            }
                        }
                        if gone {
                            break;
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
            _ = state.cancel.cancelled() => break,
        }
    }
    drop(subscriptions);
    if signed_out {
        tracing::info!("feed socket: its login was signed out; closing it");
        let close = gotcha::axum::extract::ws::CloseFrame {
            code: 4401,
            reason: "signed out".into(),
        };
        let _ = sender.send(Message::Close(Some(close))).await;
    }
    let _ = sender.close().await;
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router.get("/api/events", events_ws)
}

/// Nothing to amend: the websocket carries no OpenAPI operation.
pub fn describe(_reg: &mut ExtraResponses) {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::openapi_ext::Nullable;
    use crate::sessions::{SessionStatus, Settings};

    /// The only fields `GET /api/settings` may have that the frame does not.
    ///
    /// Counting sessions means scanning both CLIs' corpora, and a feed client
    /// reconnects far more often than it asks for the settings route — so the
    /// frame pays for neither.
    const ROUTE_ONLY: [&str; 2] = ["claudeSessionCount", "codexSessionCount"];

    fn summary(id: &str, updated_at: &str) -> SessionSummary {
        SessionSummary {
            pinned: false,
            id: id.to_string(),
            source: Source::Claude,
            scope: crate::sessions::SessionScope::Owned,
            server: "default".into(),
            title: format!("session {id}"),
            cwd: Nullable(None),
            repo_key: Nullable(None),
            git_branch: Nullable(None),
            updated_at: updated_at.to_string(),
            created_at: updated_at.to_string(),
            size_bytes: 0,
            file_path: format!("/tmp/{id}.jsonl"),
            status: SessionStatus::Idle,
            activity: crate::sessions::SessionActivity::Idle,
            resident: Nullable(None),
            claude_root: None,
            account_id: None,
            attention_at: Nullable(None),
            unread: false,
            unread_count: 0,
            asking: false,
        }
    }

    /// The two ways a client can learn what server it is talking to must not be
    /// able to drift apart in silence.
    ///
    /// The relationship is deliberately asymmetric, and this is the direction
    /// each way may differ. The route is allowed to carry *more* — the
    /// [`ROUTE_ONLY`] counts, which cost a corpus scan. The frame is allowed to
    /// carry nothing the route does not: a client reads whichever of the two it
    /// has, so a field that exists only on the socket is a field half the
    /// callers can never see. Anything else added to either one fails here until
    /// it is added to the other or written down above as route-only.
    #[test]
    fn the_settings_frame_carries_every_field_the_settings_route_does() {
        let paths = Paths::new(PathBuf::from("/home/someone"));
        let frame: serde_json::Value =
            serde_json::from_str(&settings_frame(&paths, "default", "2026-01-01T00:00:00.000Z", &[])).expect("a JSON frame");
        // Values are irrelevant here; this is about the shape the two agree on.
        let route = serde_json::to_value(Settings {
            instance: "default".into(),
            server_id: "default".into(),
            hostname: "host".into(),
            platform: "darwin".into(),
            node_version: "1".into(),
            server_version: "1".into(),
            home_dir: "/home/someone".into(),
            claude_dir: "/home/someone/.claude/projects".into(),
            claude_managed_dir: "/home/someone/.claude/managed".into(),
            codex_dir: "/home/someone/.codex/sessions".into(),
            claude_session_count: 0,
            codex_session_count: 0,
            server_started_at: "2026-01-01T00:00:00.000Z".into(),
            plugins: vec![],
        })
        .expect("a JSON body");

        let route_keys: BTreeSet<&str> = route.as_object().expect("an object").keys().map(String::as_str).collect();
        let frame_keys: BTreeSet<&str> = frame
            .as_object()
            .expect("an object")
            // `type` is the socket's own envelope, not part of the description.
            .keys()
            .map(String::as_str)
            .filter(|key| *key != "type")
            .collect();

        let invented: Vec<&&str> = frame_keys.difference(&route_keys).collect();
        assert!(
            invented.is_empty(),
            "the feed frame carries {invented:?}, which `GET /api/settings` does not"
        );
        let missing: Vec<&&str> = route_keys.difference(&frame_keys).filter(|key| !ROUTE_ONLY.contains(key)).collect();
        assert!(
            missing.is_empty(),
            "the feed frame is missing {missing:?}, which `GET /api/settings` reports"
        );
    }

    /// The list itself, not just its key: a client that reads the frame and one
    /// that reads the route see the same plugins in the same JSON.
    #[test]
    fn the_settings_frame_carries_the_plugins_as_the_route_writes_them() {
        let paths = Paths::new(PathBuf::from("/home/someone"));
        let status = droppen_plugin_sdk::PluginStatus {
            id: droppen_plugin_sdk::PluginId::new("tasks").expect("an id"),
            version: droppen_plugin_sdk::sdk_version(),
            name: droppen_plugin_sdk::LocalizedText::Plain("Tasks".into()),
            enabled: true,
            compatible: true,
            forced: false,
            reason: None,
            notice: Some("a notice".into()),
            notices: vec![droppen_plugin_sdk::PluginNotice {
                id: "abc".into(),
                kind: "reconcile".into(),
                text: "a notice".into(),
                at: "2026-01-01T00:00:00.000Z".into(),
                dismissible: true,
            }],
        };
        let frame: serde_json::Value = serde_json::from_str(&settings_frame(
            &paths,
            "default",
            "2026-01-01T00:00:00.000Z",
            std::slice::from_ref(&status),
        ))
        .expect("a JSON frame");
        let route = serde_json::to_value(vec![crate::plugins::wire::PluginStatus::from(status)]).expect("JSON");
        assert_eq!(frame["plugins"], route);
    }

    #[test]
    fn the_index_keys_by_source_and_id() {
        let sessions = vec![summary("a", "2026-01-01T00:00:00.000Z")];
        let indexed = index(&sessions);
        assert!(indexed.contains_key("claude/a"), "the web client keys its map the same way");
    }

    #[test]
    fn a_changed_summary_reserializes_differently() {
        let before = index(&[summary("a", "2026-01-01T00:00:00.000Z")]);
        let after = index(&[summary("a", "2026-01-02T00:00:00.000Z")]);
        assert_ne!(
            before["claude/a"], after["claude/a"],
            "a diff on the serialized row is what makes an upsert"
        );
    }

    /// The unread count is pushed like any other field: a row whose count moved
    /// — a new answer landed, or the user looked and it fell to zero — is an
    /// upsert carrying the new number, and an unmoved list pushes nothing.
    #[test]
    fn a_changed_unread_count_is_pushed_as_an_upsert() {
        let before = vec![summary("a", "2026-01-01T00:00:00.000Z"), summary("b", "2026-01-01T00:00:00.000Z")];
        let seen = index(&before);
        assert_eq!(delta_frame(&seen, &before, &seen), None, "nothing moved, nothing is sent");

        let mut after = before.clone();
        after[0].unread_count = 3;
        after[0].unread = true;
        let next = index(&after);
        let frame: serde_json::Value = serde_json::from_str(&delta_frame(&seen, &after, &next).expect("a delta")).expect("JSON");
        assert_eq!(frame["type"], "sessions:delta");
        let upserts = frame["upserts"].as_array().expect("upserts");
        assert_eq!(upserts.len(), 1, "only the row that changed: {upserts:?}");
        assert_eq!(upserts[0]["id"], "a");
        assert_eq!(upserts[0]["unreadCount"], 3, "the count is on the wire under its contract name");

        // Read: the count falls back to zero, and that is news too.
        let mut read = after.clone();
        read[0].unread_count = 0;
        read[0].unread = false;
        let frame: serde_json::Value = serde_json::from_str(&delta_frame(&next, &read, &index(&read)).expect("a delta")).expect("JSON");
        assert_eq!(frame["upserts"][0]["unreadCount"], 0);
    }

    /// A question opening, and being answered, is an upsert both ways: the
    /// amber state on a row must not outlive the answer by a re-list.
    #[test]
    fn a_question_opening_or_closing_is_pushed_as_an_upsert() {
        let before = vec![summary("a", "2026-01-01T00:00:00.000Z")];
        let seen = index(&before);
        let mut asking = before.clone();
        asking[0].asking = true;
        let next = index(&asking);
        let frame: serde_json::Value = serde_json::from_str(&delta_frame(&seen, &asking, &next).expect("a delta")).expect("JSON");
        assert_eq!(frame["upserts"][0]["asking"], true, "on the wire under its contract name");

        let frame: serde_json::Value = serde_json::from_str(&delta_frame(&next, &before, &seen).expect("a delta")).expect("JSON");
        assert_eq!(frame["upserts"][0]["asking"], false, "and answered is news too");
    }

    #[tokio::test]
    async fn a_request_with_nobody_listening_is_a_no_op() {
        let feed = GlobalFeed::new();
        feed.request_refresh(Some((Source::Claude, "x")));
        assert!(!feed.is_running(), "an idle feed holds no watcher");
    }

    #[tokio::test]
    async fn a_published_frame_reaches_every_listener_and_nobody_is_a_no_op() {
        let feed = GlobalFeed::new();
        feed.publish(r#"{"type":"screens:list","listRevision":1}"#.into());
        assert!(!feed.is_running(), "publishing to nobody starts nothing");

        let mut first = feed.listen();
        let mut second = feed.listen();
        feed.publish(r#"{"type":"screens:list","listRevision":2}"#.into());
        for rx in [&mut first, &mut second] {
            assert_eq!(&*rx.recv().await.expect("frame"), r#"{"type":"screens:list","listRevision":2}"#);
        }
    }

    /// Review of S1-7, round 3: a feed socket opened by a login is closed
    /// (4401) once that login is signed out; one opened by another login stays.
    #[tokio::test]
    async fn signing_out_closes_the_feed_sockets_of_that_login() {
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        let home = tempfile::tempdir().unwrap();
        let (state, addr) = crate::api::test_support::served(home.path()).await;
        let auth = state.drive.auth.clone();
        auth.users.add("ada", "correct horse battery staple").await.unwrap();
        let open = |digest: String| {
            let auth = auth.clone();
            async move {
                let ticket = auth.tickets.issue("ada", &digest, crate::auth::tickets::TicketScope::Ws).unwrap().0;
                tokio_tungstenite::connect_async(format!("ws://{addr}/api/events?ticket={ticket}"))
                    .await
                    .expect("let in")
                    .0
            }
        };
        let digest = |token: &str| crate::auth::sessions::digest_of(token);
        let gone = digest(&auth.sessions.issue("ada", None).await.unwrap().token);
        let kept = digest(&auth.sessions.issue("ada", None).await.unwrap().token);
        let mut signed_out = open(gone.clone()).await;
        let mut stays = open(kept).await;
        assert!(auth.sessions.revoke(&gone).await);
        let closed = tokio::time::timeout(LOGIN_RECHECK + Duration::from_secs(5), async {
            loop {
                match signed_out.next().await {
                    Some(Ok(WsMessage::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                    Some(Ok(_)) => continue,
                    other => panic!("{other:?}"),
                }
            }
        })
        .await
        .expect("closed in time");
        assert_eq!(closed, Some(4401));
        // The other login's socket is still served.
        stays.send(WsMessage::Ping(Default::default())).await.unwrap();
        let alive = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match stays.next().await {
                    Some(Ok(WsMessage::Pong(_))) => return true,
                    Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return false,
                    Some(Ok(_)) => continue,
                }
            }
        })
        .await
        .expect("an answer");
        assert!(alive);
    }

    /// A feed socket opened by the device's owner (`remote-hand-server open`)
    /// closes (4401) once the device has its first account: from then on the
    /// owner's login stands for nobody, whatever its row says.
    #[tokio::test]
    async fn the_first_account_closes_the_owners_feed_socket() {
        use tokio_tungstenite::tungstenite::Message as WsMessage;
        let home = tempfile::tempdir().unwrap();
        let (state, addr) = crate::api::test_support::served(home.path()).await;
        let auth = state.drive.auth.clone();
        let owner = crate::auth::sessions::digest_of(&auth.sessions.issue(crate::auth::LOCAL_OWNER, None).await.unwrap().token);
        let ticket = auth
            .tickets
            .issue(crate::auth::LOCAL_OWNER, &owner, crate::auth::tickets::TicketScope::Ws)
            .unwrap()
            .0;
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/events?ticket={ticket}"))
            .await
            .expect("the owner is let in while there is no account");
        auth.users.add("ada", "correct horse battery staple").await.unwrap();
        let closed = tokio::time::timeout(LOGIN_RECHECK + Duration::from_secs(5), async {
            loop {
                match socket.next().await {
                    Some(Ok(WsMessage::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                    Some(Ok(_)) => continue,
                    other => panic!("{other:?}"),
                }
            }
        })
        .await
        .expect("closed in time");
        assert_eq!(closed, Some(4401));
    }
}
