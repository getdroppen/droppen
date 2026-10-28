//! Sub-streams of an end-to-end channel, bridged to this server's own
//! loopback listener (`docs/cross-machine-architecture.md` §5, "MVP 的 agent 侧桥接"), so no handler
//! changes: `rpc` becomes one HTTP request to `/api/*`, `mux` the feed socket
//! (`/api/events`), `term` a terminal's socket (`/api/terminals/{id}/ws`). The
//! wire format inside a stream is `rh_proto::bridge`.
//!
//! Each request carries the per-boot internal credential (`x-rh-agent`) that
//! `auth::guard` accepts only from a loopback peer, and the device id the
//! channel was authorised for (`x-rh-agent-device`), which becomes the
//! request's identity (`cloud:<device>`). Nothing the device sends can set
//! either: a request's headers are rebuilt from an allowlist.

use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rh_proto::bridge::{self, Kind, MessageReader, RequestHead, ResponseHead, CLOSE_BAD_REQUEST, CLOSE_DONE, CLOSE_FORBIDDEN, CLOSE_UPSTREAM};
use rh_proto::driver::{Incoming, Stream, StreamEvent, StreamWriter};
use rh_proto::mux::StreamClass;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

/// The header carrying the internal credential.
pub const CREDENTIAL_HEADER: &str = "x-rh-agent";
/// The header naming the device a bridged request is from.
pub const DEVICE_HEADER: &str = "x-rh-agent-device";
/// The largest rpc request body the bridge buffers.
const MAX_BODY: usize = 64 * 1024 * 1024;

/// Request headers a device may set; everything else is dropped.
const FORWARDED: [&str; 9] = [
    "content-type",
    "accept",
    "accept-language",
    "if-none-match",
    "if-match",
    "if-modified-since",
    "if-unmodified-since",
    "range",
    "cache-control",
];

/// Response headers never passed back (hop-by-hop, or re-framed by the bridge).
const DROPPED: [&str; 8] = [
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "proxy-authenticate",
    "proxy-connection",
    "content-length",
    "set-cookie",
];

/// Where the bridge sends things.
pub struct Loopback {
    /// This server's own listener, as a loopback address.
    pub addr: SocketAddr,
    /// The per-boot credential `auth::guard` accepts from loopback.
    pub credential: String,
    http: reqwest::Client,
}

impl Loopback {
    /// The bridge's target for a server listening on `addr`.
    pub fn new(addr: SocketAddr, credential: String) -> Self {
        // An unspecified bind address is reached on loopback.
        let ip = if addr.ip().is_unspecified() {
            match addr {
                SocketAddr::V4(_) => std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                SocketAddr::V6(_) => std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            }
        } else {
            addr.ip()
        };
        Self {
            addr: SocketAddr::new(ip, addr.port()),
            credential,
            http: reqwest::Client::builder().no_proxy().build().expect("a plain HTTP client"),
        }
    }
}

impl Loopback {
    /// `GET path` on this server as the agent itself (the stream copy reads
    /// the same JSON a page would), or `None` on any failure.
    pub async fn get_json(&self, path: &str) -> Option<serde_json::Value> {
        let res = self
            .http
            .get(format!("http://{}{path}", self.addr))
            .header(CREDENTIAL_HEADER, &self.credential)
            .header(DEVICE_HEADER, "agent-sync")
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .ok()?;
        if !res.status().is_success() {
            return None;
        }
        res.json().await.ok()
    }
}

/// Routes the agent answers itself rather than bridging to loopback: what a
/// device asks of *the agent* (its machine key, its display name), which no
/// loopback handler knows about. Only over `rpc`.
pub const LOCAL_PREFIX: &str = "/rh/";

/// Where a device stands for the admin-only routes, as this agent's own
/// chain says now (chain v3, `docs/local-aggregator.md` §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    /// A current admin device (v2: a full device).
    pub admin: bool,
    /// How this agent's view of the account stands for a management
    /// request, which needs an admin to have vouched for its chain head
    /// within twelve hours ([`rh_proto::chain::MANAGE_FRESHNESS_MS`]) on top
    /// of the channel's seven days.
    pub view: View,
    /// It may still control this machine: the chain lets it, and its control
    /// on this channel has not lapsed (`docs/local-aggregator.md` §3.4). Judged again
    /// when a request is dispatched, which can be long after its stream
    /// opened.
    pub controls: bool,
}

/// This agent's view of the account, for a management request
/// ([`rh_proto::chain::ChainState::freshness`] over twelve hours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// An admin vouched for exactly this head within twelve hours.
    Fresh,
    /// Nobody did (the newest vouching is older, or there is none).
    Stale,
    /// An admin vouched for a head beyond this view: it has to catch up.
    Behind,
    /// An admin vouched for a head this view conflicts with.
    Fork,
}

/// Why a bridged request was refused before it reached loopback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// An admin-only route from a device that is not an admin (403
    /// `limited_device`).
    LimitedDevice,
    /// An admin-only route while this agent's view is not one an admin
    /// vouched for within twelve hours (412 `stale`, with the [`View`] as
    /// its `reason`: `stale`, `behind` or `fork`). Never [`View::Fresh`].
    Stale(View),
    /// Anything from a device that may no longer control this machine, or
    /// whose control lapsed (412 `stale`, `reason` `lapsed`): the channel is
    /// on its way out.
    Lapsed,
    /// A route no device reaches over a channel — the local Droppen link and
    /// gateway, runtime-installed plugins, and what only a process on this
    /// computer can prove it may call (403 `not_from_cloud`).
    NotFromCloud,
    /// A route this computer does not offer other devices: none that
    /// [`ROUTES`] classifies (403 `not_offered`). A route the server gains is
    /// refused to every device until it is classified there.
    Unlisted,
}

impl Refusal {
    /// The status and body a refused request is answered with.
    pub fn answer(self) -> (u16, serde_json::Value) {
        match self {
            Refusal::LimitedDevice => (
                403,
                serde_json::json!({"error": "limited_device", "message": "this device cannot manage the account: ask an admin device, or change it on the device itself"}),
            ),
            Refusal::Stale(View::Behind) => (
                412,
                serde_json::json!({"error": "stale", "reason": "behind", "message": "an admin device has seen more of the account's device chain than the device you reach, which has to catch up first"}),
            ),
            Refusal::Stale(View::Fork) => (
                412,
                serde_json::json!({"error": "stale", "reason": "fork", "message": "the device you reach has a view of the account's device chain that conflicts with what an admin device vouched for"}),
            ),
            Refusal::Stale(View::Stale | View::Fresh) => (
                412,
                serde_json::json!({"error": "stale", "reason": "stale", "message": "no admin device vouched for the account as the device you reach sees it in the last 12 hours: attest and retry"}),
            ),
            Refusal::Lapsed => (
                412,
                serde_json::json!({"error": "stale", "reason": "lapsed", "message": "the device you reach takes no control from this device now: no admin device vouched for the account as it sees it in the last 7 days, or this device may no longer control it"}),
            ),
            Refusal::NotFromCloud => (
                403,
                serde_json::json!({"error": "not_from_cloud", "message": "this is done on the device itself only, not from another device"}),
            ),
            Refusal::Unlisted => (
                403,
                serde_json::json!({"error": "not_offered", "message": "the device you reach does not offer that to other devices"}),
            ),
        }
    }
}

/// What a route is to a device reaching this computer over a channel
/// (`docs/local-aggregator.md` §5.4): the line runs between *using* the computer and
/// changing its security posture — who signs in, which binary runs,
/// credentials, push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RouteClass {
    /// Uses the computer: any device that may control it.
    Use,
    /// Changes its posture: admin devices only, and only with a view of the
    /// account an admin vouched for within twelve hours.
    Posture,
    /// The local Droppen link and gateway, and a session's own `rh-notify`:
    /// no device, ever.
    Never,
}

/// Every route this server registers — and the agent's own ([`LOCAL_PREFIX`])
/// — as `(method, path template, class)`. An allow-list: a request that
/// matches none is refused to every device ([`Refusal::Unlisted`]), and the
/// test that enumerates the server's routes fails until a new route is
/// classified here. `GET` covers `HEAD`.
pub const ROUTES: &[(&str, &str, RouteClass)] = {
    use RouteClass::{Never, Posture, Use};
    &[
        // Sessions and their feeds.
        ("GET", "/api/sessions", Use),
        ("GET", "/api/sessions/{source}/{id}", Use),
        ("GET", "/api/sessions/{source}/{id}/ws", Use),
        ("GET", "/api/settings", Use),
        ("POST", "/api/sessions/{source}/{id}/seen", Use),
        ("PUT", "/api/sessions/{source}/{id}/account", Use),
        ("POST", "/api/sessions/{source}/{id}/continue", Use),
        ("POST", "/api/sessions/new", Use),
        ("POST", "/api/sessions/{source}/{id}/message", Use),
        ("GET", "/api/sessions/{source}/{id}/receipts", Use),
        ("POST", "/api/sessions/{source}/{id}/interrupt", Use),
        ("POST", "/api/sessions/{source}/{id}/tasks/{taskId}/stop", Use),
        ("DELETE", "/api/sessions/{source}/{id}/queue/{qid}", Use),
        ("POST", "/api/sessions/{source}/{id}/queue/{qid}/steer", Use),
        ("GET", "/api/sessions/{source}/{id}/questions", Use),
        ("POST", "/api/sessions/{source}/{id}/questions/{requestId}/answer", Use),
        ("POST", "/api/sessions/{source}/{id}/questions/{requestId}/dismiss", Use),
        ("PUT", "/api/sessions/{source}/{id}/title", Use),
        ("PUT", "/api/sessions/{source}/{id}/pin", Use),
        ("POST", "/api/sessions/{source}/{id}/archive", Use),
        ("GET", "/api/sessions/{source}/{id}/history", Use),
        ("GET", "/api/sessions/{source}/{id}/search", Use),
        ("GET", "/api/sessions/{source}/{id}/first-unread", Use),
        ("GET", "/api/sessions/{source}/{id}/agents", Use),
        ("GET", "/api/sessions/{source}/{id}/agents/{agentId}", Use),
        ("POST", "/api/sessions/{source}/{id}/agents/{agentId}/seen", Use),
        ("GET", "/api/sessions/{source}/{id}/checklist", Use),
        ("GET", "/api/sessions/recent-cwds", Use),
        ("GET", "/api/composer-prefs", Use),
        ("PUT", "/api/composer-prefs/{source}", Use),
        ("PUT", "/api/composer-prefs/{source}/{id}", Use),
        ("GET", "/api/events", Use),
        // Claude accounts: reading them uses the computer; adding, changing,
        // removing them and their credentials is posture — and so is polling
        // a setup-token capture, whose answer carries the captured token.
        ("GET", "/api/accounts", Use),
        ("GET", "/api/accounts/limit-events", Use),
        ("GET", "/api/accounts/start", Use),
        ("GET", "/api/accounts/{id}/limit-events", Use),
        // The whole block, values included: reading it is posture, like
        // changing it.
        ("GET", "/api/accounts/{id}/env", Posture),
        ("GET", "/api/accounts/setup-token/{captureId}", Posture),
        ("DELETE", "/api/accounts/{id}/rest", Posture),
        ("POST", "/api/accounts", Posture),
        ("PATCH", "/api/accounts/{id}", Posture),
        ("DELETE", "/api/accounts/{id}", Posture),
        ("PUT", "/api/accounts/default", Posture),
        ("POST", "/api/accounts/{id}/usage-verify", Posture),
        ("POST", "/api/accounts/setup-token", Posture),
        ("POST", "/api/accounts/setup-token/{captureId}/code", Posture),
        ("POST", "/api/accounts/{id}/setup-token", Posture),
        ("POST", "/api/accounts/{id}/usage-login", Posture),
        // Local sign-in: only whether it is on is anybody's to read.
        ("GET", "/api/auth/status", Use),
        ("POST", "/api/auth/login", Posture),
        ("POST", "/api/auth/logout", Posture),
        ("GET", "/api/auth/sessions", Posture),
        ("DELETE", "/api/auth/sessions/{id}", Posture),
        ("POST", "/api/auth/ticket", Posture),
        ("POST", "/api/auth/password", Posture),
        // The first account is made on this device's own page, with the code
        // it printed; the handler refuses a bridged device too.
        ("POST", "/api/auth/setup", Never),
        // The local capability is this device's OS user's alone; the handler
        // refuses a bridged device too.
        ("POST", "/api/auth/local-open", Never),
        // The desktop app's own login asking for that capability: the app's,
        // on its own device; the handler refuses anybody but its account.
        ("POST", "/api/auth/local-open/grant", Never),
        // Embedded apps, chats, files, tasks, terminals, the rest of using it.
        ("GET", "/api/apps", Use),
        ("POST", "/api/apps", Use),
        ("DELETE", "/api/apps/{id}", Use),
        ("GET", "/api/chats", Use),
        ("POST", "/api/chats/new", Use),
        ("GET", "/api/api-chats/{id}", Use),
        ("DELETE", "/api/api-chats/{id}", Use),
        ("POST", "/api/api-chats/{id}/message", Use),
        ("GET", "/api/chat-provider", Use),
        ("PUT", "/api/chat-provider", Posture),
        ("GET", "/api/chat-provider/models", Use),
        // Which programs run as ACP agents, and with what in their environment.
        ("GET", "/api/acp/agents", Use),
        ("PUT", "/api/acp/agents/{id}", Posture),
        ("DELETE", "/api/acp/agents/{id}", Posture),
        // A check starts the program an agent names; the last one's error is
        // what the program wrote to stderr.
        ("POST", "/api/acp/agents/{id}/check", Posture),
        ("GET", "/api/acp/agents/{id}/check", Posture),
        ("GET", "/api/fs/list", Use),
        ("GET", "/api/fs/read", Use),
        ("GET", "/api/fs/download", Use),
        ("POST", "/api/fs/upload", Use),
        ("POST", "/api/uploads", Use),
        ("GET", "/api/media/github", Use),
        ("GET", "/api/models", Use),
        ("GET", "/api/notify/config", Use),
        ("PUT", "/api/notify/config", Posture),
        ("POST", "/api/notify/test", Use),
        // A session's own `rh-notify`, proved by a capability only a process
        // this server spawned holds: no device can satisfy it over a channel.
        ("POST", "/api/notify", Never),
        ("GET", "/api/prefs", Use),
        ("PUT", "/api/prefs", Use),
        ("GET", "/api/push/vapid", Posture),
        ("POST", "/api/push/subscriptions", Posture),
        ("DELETE", "/api/push/subscriptions", Posture),
        ("GET", "/api/stats/quota", Use),
        ("GET", "/api/stats/usage", Use),
        ("GET", "/api/scheduled-tasks", Use),
        ("POST", "/api/scheduled-tasks", Use),
        ("PATCH", "/api/scheduled-tasks/{id}", Use),
        ("DELETE", "/api/scheduled-tasks/{id}", Use),
        ("GET", "/api/screens", Use),
        ("POST", "/api/screens", Use),
        ("PUT", "/api/screens/order", Use),
        ("PATCH", "/api/screens/{id}", Use),
        ("DELETE", "/api/screens/{id}", Use),
        ("POST", "/api/server/self-upgrade", Posture),
        // Runtime-installed plugins are this computer's own (`docs/plugins/
        // runtime.md` §5): no `/api/plugins…` route is listed here, and
        // [`plugin_route`] refuses every one of them to every device.
        ("GET", "/api/terminals", Use),
        ("POST", "/api/terminals", Use),
        ("PATCH", "/api/terminals/{id}", Use),
        ("DELETE", "/api/terminals/{id}", Use),
        ("GET", "/api/terminals/{id}/ws", Use),
        // The first-run guide: what this computer has — no more than a
        // terminal could tell. Re-reading the login shell into every child's
        // PATH, installing, signing in and adopting a binary change what runs
        // here (and the server refuses them to anything but the desktop app's
        // own window on this computer anyway, `setup::app_caller`).
        ("GET", "/api/setup/tools", Use),
        ("POST", "/api/setup/tools/rescan", Posture),
        ("GET", "/api/setup/commands", Use),
        ("GET", "/api/setup/tools/{tool}/login", Use),
        ("POST", "/api/setup/tools/{tool}/install", Posture),
        ("POST", "/api/setup/tools/{tool}/update", Posture),
        ("POST", "/api/setup/tools/{tool}/login", Posture),
        ("POST", "/api/setup/tools/{tool}/use", Posture),
        ("POST", "/api/setup/git/install", Posture),
        // Where the guide stands, and its first session's folders: no more
        // than the session list, the preferences and a terminal give. The
        // progress is a record nothing acts on: should a choice in it
        // (`where.lan`, `wrap.startAtLogin`) ever be applied, its `PUT`
        // becomes `Posture` first (`setup::progress`).
        ("GET", "/api/setup/progress", Use),
        ("PUT", "/api/setup/progress", Use),
        ("GET", "/api/setup/projects/recent", Use),
        ("POST", "/api/setup/practice-folder", Use),
        // The local Droppen link (and, under the same prefix rule, the
        // gateway `/api/agg/*`): never over a channel.
        ("GET", "/api/cloud", Never),
        ("POST", "/api/cloud/link", Never),
        ("POST", "/api/cloud/link/finish", Never),
        ("POST", "/api/cloud/link/cancel", Never),
        ("POST", "/api/cloud/link/return", Never),
        ("POST", "/api/cloud/aggregator", Never),
        ("POST", "/api/cloud/unlink", Never),
        ("POST", "/api/cloud/re-enroll-fresh", Never),
        ("PUT", "/api/cloud/access", Never),
        ("POST", "/api/cloud/claim", Never),
        ("GET", "/api/agg/ws", Never),
        // The agent's own routes.
        ("GET", "/rh/info", Use),
        ("POST", "/rh/display-name", Posture),
        ("POST", "/rh/machine-key", Posture),
        // Renewing this device's identity key changes who it is
        // (`docs/host-identity-rotation.md` §3.2).
        ("POST", "/rh/rotate", Posture),
    ]
};

/// Whether `route` (no query) is an instance of `template`: the same
/// segments, a `{name}` standing for any one non-empty segment.
fn instance_of(route: &str, template: &str) -> bool {
    let (mut r, mut t) = (route.split('/'), template.split('/'));
    loop {
        match (r.next(), t.next()) {
            (None, None) => return true,
            (Some(seg), Some(pat)) if pat.starts_with('{') && pat.ends_with('}') => {
                if seg.is_empty() {
                    return false;
                }
            }
            (Some(seg), Some(pat)) if seg == pat => {}
            _ => return false,
        }
    }
}

/// How [`ROUTES`] classifies `method route` (`route` without its query),
/// the strictest class of every entry it is an instance of; `None` when it
/// is none's.
pub fn classify(method: &str, route: &str) -> Option<RouteClass> {
    let method = method.to_ascii_uppercase();
    let method = if method == "HEAD" { "GET" } else { method.as_str() };
    ROUTES
        .iter()
        .filter(|(m, template, _)| *m == method && instance_of(route, template))
        .map(|(_, _, class)| *class)
        .max()
}

/// Routes no device reaches over a channel, admin or not, whatever
/// [`ROUTES`] says: the local Droppen link and gateway, by prefix.
pub fn never_from_cloud(route: &str) -> bool {
    let under = |prefix: &str| route == prefix || route.starts_with(&format!("{prefix}/"));
    under("/api/cloud") || under("/api/agg")
}

/// Whether `route` belongs to runtime-installed plugins — listing, switching,
/// installing, and every document route. They exist only on this computer and
/// the pages it serves itself (`docs/plugins/runtime.md` §5): deliberately not
/// in [`ROUTES`] at all, so no device reaches one, and refused before any
/// class is asked for.
fn plugin_route(route: &str) -> bool {
    let under = |prefix: &str| route == prefix || route.starts_with(&format!("{prefix}/"));
    under("/api/plugins")
}

/// What becomes of `method path` from a device standing as `standing`:
/// `None` when it may go on. The route is judged as sent and as the server
/// sees it once percent-decoded, the stricter answer winning; a route
/// [`ROUTES`] does not classify is refused.
pub fn refusal(method: &str, path: &str, standing: Standing) -> Option<Refusal> {
    if !standing.controls {
        return Some(Refusal::Lapsed);
    }
    let route = path.split(['?', '#']).next().unwrap_or_default();
    let decoded = percent_decoded(route);
    if [route, decoded.as_str()].iter().any(|r| never_from_cloud(r) || plugin_route(r)) {
        return Some(Refusal::NotFromCloud);
    }
    let classes = [classify(method, route), classify(method, &decoded_by_segment(route))];
    if classes.contains(&Some(RouteClass::Never)) {
        return Some(Refusal::NotFromCloud);
    }
    if classes.contains(&None) {
        return Some(Refusal::Unlisted);
    }
    if classes.contains(&Some(RouteClass::Posture)) {
        if !standing.admin {
            return Some(Refusal::LimitedDevice);
        }
        if standing.view != View::Fresh {
            return Some(Refusal::Stale(standing.view));
        }
    }
    None
}

/// The agent's own routes ([`LOCAL_PREFIX`]), answered with a status and JSON.
/// `device` is the device the channel was authorised for.
pub trait Local: Send + Sync {
    /// Where `device` stands now for the admin-only routes ([`refusal`]).
    fn standing(&self, device: &str) -> Standing;

    /// Answer one request: a status and a JSON body.
    fn handle<'a>(
        &'a self,
        device: &'a str,
        method: &'a str,
        path: &'a str,
        body: &'a [u8],
    ) -> futures_util::future::BoxFuture<'a, (u16, serde_json::Value)>;

    /// Whether this computer is renewing its push keys with its identity
    /// key, and is not done yet: the [`push_key_route`]s are refused
    /// meanwhile ([`PUSH_RENEWING`]).
    fn push_renewing(&self) -> bool {
        false
    }
}

/// The error code a [`push_key_route`] is refused with (503) while this
/// computer renews its push keys with its identity key.
pub const PUSH_RENEWING: &str = "push_renewing";

/// Whether `method path` hands out this computer's push key or takes a
/// subscription made against it: `GET /api/push/vapid`, `POST
/// /api/push/subscriptions` (judged as sent and percent-decoded). Once this
/// computer took a renewed identity key, its push keys follow it, in the
/// background, and every subscription is forgotten: until that is done, the
/// key such a route would hand out is the one about to be replaced — a page
/// would keep it as current, and miss every notification from then on — and a
/// subscription it took would be forgotten with the others. So they are
/// refused meanwhile (503 [`PUSH_RENEWING`]), and the page asks again.
pub fn push_key_route(method: &str, path: &str) -> bool {
    let method = method.to_ascii_uppercase();
    let method = if method == "HEAD" { "GET" } else { method.as_str() };
    let route = path.split(['?', '#']).next().unwrap_or_default();
    [route.to_string(), decoded_by_segment(route)]
        .iter()
        .any(|r| matches!((method, r.as_str()), ("GET", "/api/push/vapid") | ("POST", "/api/push/subscriptions")))
}

/// The answer to a [`push_key_route`] while the push keys are renewed.
fn push_renewing_answer() -> (u16, serde_json::Value) {
    (
        503,
        serde_json::json!({
            "error": PUSH_RENEWING,
            "message": "this computer is renewing its push keys with its identity key: ask again in a moment",
        }),
    )
}

/// `%XX` escapes decoded (anything malformed left as it is).
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `route` percent-decoded one segment at a time, as the router matches
/// each segment: a slash a segment decodes to stays encoded, inside that
/// segment, so it cannot add a segment, while `self%2Dupgrade` still reads
/// as `self-upgrade`.
fn decoded_by_segment(route: &str) -> String {
    route
        .split('/')
        .map(|seg| percent_decoded(seg).replace('/', "%2F"))
        .collect::<Vec<_>>()
        .join("/")
}

/// The routes a `blob` stream (low priority, for files and pictures) may reach.
const BLOB_ROUTES: [&str; 3] = ["/api/fs/download", "/api/fs/upload", "/api/media/github"];

/// Whether `path` may be reached by a stream of `class`.
pub fn allowed(class: StreamClass, path: &str) -> bool {
    let route = path.split(['?', '#']).next().unwrap_or_default();
    // Judge the route as the server will see it once percent-decoded too:
    // `%2e%2e` would otherwise walk past the dot-segment check.
    let decoded = percent_decoded(route);
    let clean = (route.starts_with("/api/") || (class == StreamClass::Rpc && route.starts_with(LOCAL_PREFIX)))
        && [route, decoded.as_str()]
            .iter()
            .all(|r| !r.split('/').any(|seg| seg == ".." || seg == ".") && !r.contains("//") && !r.contains('\\'))
        && decoded.matches('/').count() == route.matches('/').count()
        && path.bytes().all(|b| b.is_ascii_graphic());
    if !clean {
        return false;
    }
    match class {
        StreamClass::Rpc => true,
        StreamClass::Mux => route == "/api/events",
        StreamClass::Term => {
            let Some(rest) = route.strip_prefix("/api/terminals/") else {
                return false;
            };
            let Some(id) = rest.strip_suffix("/ws") else { return false };
            !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        }
        StreamClass::Blob => BLOB_ROUTES.contains(&route),
    }
}

/// Serve one stream a device opened.
pub async fn serve(incoming: Incoming, device: String, loopback: Arc<Loopback>, local: Arc<dyn Local>) {
    let Incoming { class, meta, stream } = incoming;
    let head = match RequestHead::decode(&meta) {
        Ok(head) => head,
        Err(_) => return stream.close(CLOSE_BAD_REQUEST),
    };
    if !allowed(class, &head.path) {
        tracing::info!(%device, ?class, "a bridged stream asked for a path its class may not reach");
        return stream.close(CLOSE_FORBIDDEN);
    }
    // The admin-only routes, judged on this agent's own chain now (a device
    // demoted since the channel opened is held to what it is now).
    if let Some(refused) = refusal(&head.method, &head.path, local.standing(&device)) {
        let route = head.path.split(['?', '#']).next().unwrap_or_default().to_string();
        tracing::warn!(%device, method = %head.method, %route, ?refused, "refused a request over a channel");
        let mut stream = stream;
        // A request's body first (up to `End`), as any answer does: the
        // device reads the answer once it has sent the whole request.
        if matches!(class, StreamClass::Rpc | StreamClass::Blob) && request_body(&mut stream, MAX_BODY).await.is_none() {
            return;
        }
        return answer(stream, refused.answer()).await;
    }
    match class {
        StreamClass::Rpc if head.path.starts_with(LOCAL_PREFIX) => local_rpc(stream, head, &device, local.as_ref()).await,
        // A blob is an rpc at the lowest priority: the same request, a
        // streamed response, and terminals never wait behind it.
        StreamClass::Rpc | StreamClass::Blob => rpc(stream, head, &device, &loopback, local.as_ref()).await,
        StreamClass::Mux | StreamClass::Term => socket(stream, head, &device, &loopback).await,
    }
}

/// Answer a stream with `status` and a JSON body, without reaching loopback.
async fn answer(stream: Stream, (status, json): (u16, serde_json::Value)) {
    let writer = stream.writer();
    let head = ResponseHead {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
    };
    if !send_head(&writer, &head).await {
        return;
    }
    let _ = writer.send(&bridge::message(Kind::Binary, json.to_string().as_bytes())).await;
    writer.close(CLOSE_DONE);
}

async fn send_head(writer: &StreamWriter, head: &ResponseHead) -> bool {
    match head.encode() {
        Ok(bytes) => writer.send(&bridge::message(Kind::Head, &bytes)).await.is_ok(),
        Err(_) => false,
    }
}

/// The request body of an rpc stream, up to `End` (`None`: the stream is done).
async fn request_body(stream: &mut Stream, max: usize) -> Option<Vec<u8>> {
    let mut reader = MessageReader::new();
    let mut body = Vec::new();
    loop {
        match stream.recv().await {
            Some(StreamEvent::Data(data)) => {
                reader.push(&data);
                loop {
                    match reader.next_message() {
                        Ok(Some((Kind::End, _))) => return Some(body),
                        Ok(Some((Kind::Binary | Kind::Text, chunk))) => {
                            body.extend_from_slice(&chunk);
                            if body.len() > max {
                                stream.close(CLOSE_BAD_REQUEST);
                                return None;
                            }
                        }
                        Ok(Some((Kind::Head, _))) | Err(_) => {
                            stream.close(CLOSE_BAD_REQUEST);
                            return None;
                        }
                        Ok(None) => break,
                    }
                }
            }
            Some(StreamEvent::Closed(_)) | None => return None,
        }
    }
}

/// Whether a request whose body has just arrived is still to be served: judged
/// again on where its device stands now, since a device can open a stream and
/// hold back its `End` for as long as it likes. `None`: it was answered.
async fn still_served(stream: Stream, head: &RequestHead, device: &str, local: &dyn Local) -> Option<Stream> {
    match refusal(&head.method, &head.path, local.standing(device)) {
        None => Some(stream),
        Some(refused) => {
            let route = head.path.split(['?', '#']).next().unwrap_or_default();
            tracing::warn!(%device, method = %head.method, %route, ?refused, "refused a request over a channel as it was dispatched");
            answer(stream, refused.answer()).await;
            None
        }
    }
}

/// One of the agent's own routes.
async fn local_rpc(mut stream: Stream, head: RequestHead, device: &str, local: &dyn Local) {
    let Some(body) = request_body(&mut stream, 64 * 1024).await else {
        return;
    };
    let Some(stream) = still_served(stream, &head, device, local).await else {
        return;
    };
    let path = head.path.split(['?', '#']).next().unwrap_or_default().to_string();
    let (status, json) = local.handle(device, &head.method, &path, &body).await;
    answer(stream, (status, json)).await;
}

async fn rpc(mut stream: Stream, head: RequestHead, device: &str, loopback: &Loopback, local: &dyn Local) {
    let Some(body) = request_body(&mut stream, MAX_BODY).await else {
        return;
    };
    let Some(stream) = still_served(stream, &head, device, local).await else {
        return;
    };
    if push_key_route(&head.method, &head.path) && local.push_renewing() {
        tracing::info!(%device, method = %head.method, "asked for the push key while it is renewed: to ask again");
        return answer(stream, push_renewing_answer()).await;
    }
    let Ok(method) = reqwest::Method::from_bytes(head.method.as_bytes()) else {
        return stream.close(CLOSE_BAD_REQUEST);
    };
    let url = format!("http://{}{}", loopback.addr, head.path);
    let mut request = loopback
        .http
        .request(method, url)
        .header(CREDENTIAL_HEADER, &loopback.credential)
        .header(DEVICE_HEADER, device);
    for (name, value) in &head.headers {
        if FORWARDED.contains(&name.to_ascii_lowercase().as_str()) {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    if !body.is_empty() {
        request = request.body(body);
    }
    let mut response = match request.send().await {
        Ok(r) => r,
        Err(error) => {
            tracing::warn!(%error, "a bridged request failed on loopback");
            return stream.close(CLOSE_UPSTREAM);
        }
    };
    let head = ResponseHead {
        status: response.status().as_u16(),
        headers: response
            .headers()
            .iter()
            .filter(|(name, _)| !DROPPED.contains(&name.as_str()))
            .filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string())))
            .take(bridge::MAX_HEADERS)
            .collect(),
    };
    let writer = stream.writer();
    if !send_head(&writer, &head).await {
        return;
    }
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if writer.send(&bridge::message(Kind::Binary, &chunk)).await.is_err() {
                    return;
                }
            }
            Ok(None) => break,
            Err(_) => return writer.close(CLOSE_UPSTREAM),
        }
    }
    writer.close(CLOSE_DONE);
}

async fn socket(mut stream: Stream, head: RequestHead, device: &str, loopback: &Loopback) {
    let writer = stream.writer();
    let mut request = match format!("ws://{}{}", loopback.addr, head.path).into_client_request() {
        Ok(r) => r,
        Err(_) => return stream.close(CLOSE_BAD_REQUEST),
    };
    let headers = request.headers_mut();
    // Without both headers this would be a socket with no identity at all:
    // refused here rather than handed to the server as nobody.
    match (loopback.credential.parse(), device.parse()) {
        (Ok(credential), Ok(device)) => {
            headers.insert(CREDENTIAL_HEADER, credential);
            headers.insert(DEVICE_HEADER, device);
        }
        _ => return stream.close(CLOSE_BAD_REQUEST),
    }
    let socket = match tokio_tungstenite::connect_async_with_config(request, None, true).await {
        Ok((socket, _)) => socket,
        Err(error) => {
            let status = match &error {
                tokio_tungstenite::tungstenite::Error::Http(response) => response.status().as_u16(),
                _ => 502,
            };
            tracing::info!(status, "a bridged socket was refused on loopback");
            send_head(&writer, &ResponseHead { status, headers: vec![] }).await;
            return writer.close(CLOSE_UPSTREAM);
        }
    };
    // The socket's `open`, as the device sees it.
    if !send_head(
        &writer,
        &ResponseHead {
            status: 101,
            headers: vec![],
        },
    )
    .await
    {
        return;
    }
    let (mut sink, mut source) = socket.split();
    // Loopback → device.
    let down = {
        let writer = writer.clone();
        tokio::spawn(async move {
            while let Some(Ok(message)) = source.next().await {
                let framed = match message {
                    Message::Text(t) => bridge::message(Kind::Text, t.as_bytes()),
                    Message::Binary(b) => bridge::message(Kind::Binary, &b),
                    Message::Close(_) => break,
                    _ => continue,
                };
                if writer.send(&framed).await.is_err() {
                    return;
                }
            }
            writer.close(CLOSE_DONE);
        })
    };
    // Device → loopback.
    let mut reader = MessageReader::new();
    'up: while let Some(StreamEvent::Data(data)) = stream.recv().await {
        reader.push(&data);
        loop {
            let message = match reader.next_message() {
                Ok(Some((Kind::Text, payload))) => match String::from_utf8(payload) {
                    Ok(text) => Message::Text(text.into()),
                    Err(_) => break 'up,
                },
                Ok(Some((Kind::Binary, payload))) => Message::Binary(payload.into()),
                Ok(Some(_)) | Err(_) => break 'up,
                Ok(None) => break,
            };
            if sink.send(message).await.is_err() {
                break 'up;
            }
        }
    }
    let _ = sink.send(Message::Close(None)).await;
    down.abort();
    writer.close(CLOSE_DONE);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Standing = Standing {
        admin: false,
        view: View::Fresh,
        controls: true,
    };
    const ADMIN: Standing = Standing {
        admin: true,
        view: View::Fresh,
        controls: true,
    };
    const STALE_ADMIN: Standing = Standing {
        admin: true,
        view: View::Stale,
        controls: true,
    };

    /// The routes held back while the push keys are renewed: the key and a
    /// subscription against it, however they are written — not forgetting a
    /// subscription, nor anything else.
    #[test]
    fn the_push_key_routes_are_known_however_they_are_written() {
        for (method, path) in [
            ("GET", "/api/push/vapid"),
            ("get", "/api/push/vapid?x=1"),
            ("HEAD", "/api/push/vapid"),
            ("GET", "/api/push/%76apid"),
            ("POST", "/api/push/subscriptions"),
            ("POST", "/api/push/sub%73criptions#x"),
        ] {
            assert!(push_key_route(method, path), "{method} {path}");
        }
        for (method, path) in [
            ("DELETE", "/api/push/subscriptions"),
            ("POST", "/api/push/vapid"),
            ("GET", "/api/push/vapid/x"),
            ("GET", "/api/prefs"),
            ("POST", "/rh/rotate"),
        ] {
            assert!(!push_key_route(method, path), "{method} {path}");
        }
    }

    /// A device that is not an admin reaches every route that *uses* the
    /// computer, and none that changes who signs in, what runs, its
    /// credentials or its push — percent-encoded or not; an admin needs a
    /// twelve-hour-fresh view for those; and nobody reaches the local Droppen
    /// link or gateway over a channel.
    #[test]
    fn devices_that_are_not_admins_reach_only_what_uses_the_computer() {
        for (method, path) in [
            ("GET", "/api/sessions?limit=5"),
            ("POST", "/api/sessions/claude/abc/message"),
            ("POST", "/api/terminals"),
            ("GET", "/api/fs/read?path=%2Ftmp"),
            ("POST", "/api/fs/upload"),
            ("GET", "/api/settings"),
            ("GET", "/api/accounts"),
            ("PUT", "/api/sessions/claude/abc/account"),
            ("GET", "/api/auth/status"),
            ("GET", "/api/notify/config"),
            ("POST", "/api/scheduled-tasks"),
            ("GET", "/rh/info"),
            ("HEAD", "/api/sessions"),
            ("GET", "/api/sessions/claude/a%20b"),
            ("PUT", "/api/screens/order"),
            ("POST", "/api/notify/test"),
            // Which agents there are, by name: no values, nothing to run.
            ("GET", "/api/acp/agents"),
            ("POST", "/api/sessions/acp/abc/message"),
        ] {
            assert_eq!(refusal(method, path, PLAIN), None, "{method} {path}");
        }
        for (method, path) in [
            ("POST", "/api/server/self-upgrade"),
            ("post", "/api/server/self-upgrade"),
            ("GET", "/api/auth/sessions"),
            ("POST", "/api/auth/password"),
            ("DELETE", "/api/auth/sessions/s-1"),
            ("POST", "/api/auth/ticket"),
            ("POST", "/api/accounts"),
            ("PATCH", "/api/accounts/a-1"),
            ("DELETE", "/api/accounts/a-1"),
            ("PUT", "/api/accounts/default"),
            ("POST", "/api/accounts/setup-token"),
            // Its answer carries the token the capture read off the screen.
            ("GET", "/api/accounts/setup-token/c-1"),
            ("HEAD", "/api/accounts/setup-token/c-1"),
            ("head", "/api/accounts/setup-token/c-1"),
            ("PUT", "/api/notify/config"),
            ("PUT", "/api/chat-provider"),
            // They name the programs ACP sessions run.
            ("PUT", "/api/acp/agents/bub"),
            ("DELETE", "/api/acp/agents/bub"),
            ("POST", "/api/acp/agents/bub/check"),
            ("GET", "/api/acp/agents/bub/check"),
            ("POST", "/api/push/subscriptions"),
            ("GET", "/api/push/vapid"),
            ("POST", "/rh/machine-key"),
            ("POST", "/rh/display-name"),
            ("POST", "/rh/rotate"),
            ("post", "/rh/rotate?x=1"),
        ] {
            assert_eq!(refusal(method, path, PLAIN), Some(Refusal::LimitedDevice), "{method} {path}");
            assert_eq!(refusal(method, path, STALE_ADMIN), Some(Refusal::Stale(View::Stale)), "{method} {path}");
            assert_eq!(refusal(method, path, ADMIN), None, "{method} {path}");
        }
        for path in [
            "/api/cloud",
            "/api/cloud/link",
            "/api/cloud/link/return",
            "/api/cloud/unlink",
            "/api/agg/ws?ticket=x",
            "/api/%63loud/link",
        ] {
            assert_eq!(refusal("POST", path, ADMIN), Some(Refusal::NotFromCloud), "{path}");
            assert_eq!(refusal("GET", path, PLAIN), Some(Refusal::NotFromCloud), "{path}");
        }
        // A session's own `rh-notify`: its capability is a bearer only a
        // process this server spawned holds, which no channel carries.
        for path in ["/api/notify", "/api/%6Eotify", "/api/notify?x=1"] {
            assert_eq!(refusal("POST", path, ADMIN), Some(Refusal::NotFromCloud), "{path}");
            assert_eq!(refusal("POST", path, PLAIN), Some(Refusal::NotFromCloud), "{path}");
        }
        // Runtime-installed plugins are local-only (`docs/plugins/runtime.md`
        // §5): no device — admin or plain, whatever the method, however the
        // path is written — reaches one. The whole surface, including the
        // routes of installing and uninstalling and the notices, which this
        // build does not register yet: the rule is the prefix, not a list.
        for (method, path) in [
            ("GET", "/api/plugins"),
            ("GET", "/api/plugins/tasks/enabled"),
            ("PUT", "/api/plugins/tasks/enabled"),
            ("GET", "/api/plugins/tasks/docs"),
            ("GET", "/api/plugins/tasks/docs/state"),
            ("PUT", "/api/plugins/tasks/docs/state"),
            ("GET", "/api/plugins/tasks/docs/events%2F2026-10"),
            ("PUT", "/api/plugins/tasks/docs/events%2f2026-10"),
            ("GET", "/api/plugins/tasks/docs/events%2F2026-10/history"),
            ("POST", "/api/plugins/tasks/batch"),
            ("GET", "/api/plugins/tasks/notices"),
            ("POST", "/api/plugins"),
            ("DELETE", "/api/plugins/tasks"),
            ("GET", "/api/plugins%2Ftasks/docs/state"),
            ("GET", "/api/%70lugins"),
        ] {
            assert_eq!(refusal(method, path, ADMIN), Some(Refusal::NotFromCloud), "{method} {path}");
            assert_eq!(refusal(method, path, PLAIN), Some(Refusal::NotFromCloud), "{method} {path}");
        }
        assert_eq!(Refusal::NotFromCloud.answer().0, 403);
        assert_eq!(Refusal::NotFromCloud.answer().1["error"], "not_from_cloud");
        // An allow-list: what no entry classifies is refused to everybody —
        // a route nobody registered, a method a route does not take, a
        // spelling the router would not match.
        for (method, path) in [
            ("GET", "/api/nope"),
            ("POST", "/api/sessions"),
            ("OPTIONS", "/api/sessions"),
            ("GET", "/api/sessions/"),
            ("GET", "/api/sessions/claude//detail"),
            ("POST", "/api/server/self%2Dupgrade"),
            ("POST", "/rh/direct"),
            ("GET", "/rh/nope"),
            ("GET", "/rh/rotate"),
            ("POST", "/rh/%72otate"),
            // The Tasks routes went into a plugin, and a plugin's prefix is
            // `/api/plugins` exactly: neither is a route the server registers.
            ("GET", "/api/tasks/state"),
            ("GET", "/api/pluginsfoo"),
        ] {
            assert_eq!(refusal(method, path, ADMIN), Some(Refusal::Unlisted), "{method} {path}");
            assert_eq!(refusal(method, path, PLAIN), Some(Refusal::Unlisted), "{method} {path}");
        }
        assert_eq!(Refusal::LimitedDevice.answer().0, 403);
        assert_eq!(Refusal::LimitedDevice.answer().1["error"], "limited_device");
        assert_eq!(Refusal::Stale(View::Stale).answer().0, 412);
        assert_eq!(Refusal::Unlisted.answer(), (403, Refusal::Unlisted.answer().1));
        assert_eq!(Refusal::Unlisted.answer().1["error"], "not_offered");
    }

    /// A 412 says why — `reason` is
    /// `stale` (nobody vouched within twelve hours: attest and retry),
    /// `behind` (an admin vouched for more of the chain: catch up) or `fork`
    /// (an admin vouched for a conflicting chain) — as the agent's freshness
    /// check came out; only `stale` asks for an attestation.
    #[test]
    fn a_stale_answer_says_why() {
        for (view, reason) in [(View::Stale, "stale"), (View::Behind, "behind"), (View::Fork, "fork")] {
            let admin = Standing { admin: true, view, controls: true };
            let refused = refusal("POST", "/rh/display-name", admin);
            assert_eq!(refused, Some(Refusal::Stale(view)));
            let (status, body) = refused.unwrap().answer();
            assert_eq!(
                (status, body["error"].as_str(), body["reason"].as_str()),
                (412, Some("stale"), Some(reason))
            );
            let message = body["message"].as_str().unwrap();
            assert_eq!(message.contains("attest and retry"), view == View::Stale, "{message}");
            assert_eq!(refusal("GET", "/rh/info", admin), None, "what uses the computer needs no fresh view");
        }
        assert_eq!(
            refusal(
                "POST",
                "/rh/display-name",
                Standing {
                    admin: false,
                    view: View::Fork,
                    controls: true,
                }
            ),
            Some(Refusal::LimitedDevice),
            "not an admin comes first"
        );
    }

    /// A device whose control lapsed, or that may no longer control this
    /// machine, has nothing served — what uses the computer included: 412
    /// `stale`, `lapsed`.
    #[test]
    fn a_device_that_no_longer_controls_this_machine_has_nothing_served() {
        let lapsed = Standing { controls: false, ..ADMIN };
        for (method, path) in [("GET", "/rh/info"), ("POST", "/rh/display-name"), ("GET", "/api/sessions")] {
            assert_eq!(refusal(method, path, lapsed), Some(Refusal::Lapsed), "{method} {path}");
        }
        let (status, body) = Refusal::Lapsed.answer();
        assert_eq!((status, body["error"].as_str(), body["reason"].as_str()), (412, Some("stale"), Some("lapsed")));
    }

    /// `(METHOD, path template)` of every route the server's source
    /// registers on its router: `.get("/api/…", handler)` and the other
    /// methods (a path constant resolved in its own file), and
    /// `upload_route("/api/…", …)` (a POST). The undocumented ones — the
    /// websockets — are only found this way.
    fn registered_in_source() -> std::collections::BTreeSet<(String, String)> {
        let call = regex::Regex::new(r#"\.(get|post|put|patch|delete|head)\(\s*(?:"(/api/[A-Za-z0-9_./{}-]*)"|([A-Z][A-Z0-9_]*))\s*,"#).unwrap();
        let upload = regex::Regex::new(r#"upload_route\(\s*"(/api/[A-Za-z0-9_./{}-]*)""#).unwrap();
        let mut found = std::collections::BTreeSet::new();
        let mut dirs = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for c in call.captures_iter(&text) {
                    let route = match (c.get(2), c.get(3)) {
                        (Some(literal), _) => literal.as_str().to_string(),
                        (None, Some(name)) => {
                            let def = regex::Regex::new(&format!(r#"const {}: &str = "([^"]*)";"#, name.as_str())).unwrap();
                            match def.captures(&text) {
                                Some(d) => d[1].to_string(),
                                // Not a route constant of this file (`.get(KEY, …)` on a map).
                                None => continue,
                            }
                        }
                        _ => unreachable!(),
                    };
                    if route.starts_with("/api/") {
                        found.insert((c[1].to_ascii_uppercase(), route));
                    }
                }
                for c in upload.captures_iter(&text) {
                    found.insert(("POST".to_string(), c[1].to_string()));
                }
            }
        }
        found
    }

    /// The routes a device may reach are an allow-list.
    /// Every route the server registers — read off the router it serves
    /// (its OpenAPI document) and off the source (which also finds the
    /// undocumented websockets) — is classified in [`ROUTES`]; a new route
    /// fails here until it is, and is refused to every device meanwhile. The
    /// exception is the plugin routes, which are local-only
    /// (`docs/plugins/runtime.md` §5) and refused by [`plugin_route`] rather
    /// than classified: a new one needs no entry here.
    #[tokio::test]
    async fn every_route_the_server_registers_is_classified() {
        use std::collections::BTreeSet;
        let home = tempfile::tempdir().unwrap();
        let documented = crate::api::test_support::documented_routes(home.path()).await;
        let registered = registered_in_source();
        assert!(documented.len() > 80, "{} documented routes", documented.len());
        let unseen: Vec<_> = documented.difference(&registered).collect();
        assert!(
            unseen.is_empty(),
            "the source scan does not see these routes the router serves: {unseen:?}"
        );
        let classified: BTreeSet<(String, String)> = ROUTES.iter().map(|(m, p, _)| (m.to_string(), p.to_string())).collect();
        assert_eq!(classified.len(), ROUTES.len(), "a route classified twice");
        let unclassified: Vec<_> = registered
            .union(&documented)
            .filter(|r| !classified.contains(*r) && !plugin_route(&r.1))
            .collect();
        assert!(unclassified.is_empty(), "classify these in bridge::ROUTES: {unclassified:?}");
        let stale: Vec<_> = classified
            .iter()
            .filter(|(_, p)| !p.starts_with(LOCAL_PREFIX))
            .filter(|r| !registered.contains(*r))
            .collect();
        assert!(stale.is_empty(), "bridge::ROUTES lists routes the server does not register: {stale:?}");
        // Each, as a device sends it, is judged by its class — never refused as unlisted.
        for (method, template) in &registered {
            let sent: Vec<&str> = template.split('/').map(|s| if s.starts_with('{') { "x-1" } else { s }).collect();
            let sent = sent.join("/");
            let judged = refusal(method, &sent, ADMIN);
            assert_ne!(judged, Some(Refusal::Unlisted), "{method} {sent}");
            let expected = if plugin_route(&sent) || classify(method, &sent) == Some(RouteClass::Never) {
                Some(Refusal::NotFromCloud)
            } else {
                None
            };
            assert_eq!(judged, expected, "{method} {sent}");
        }
    }

    #[test]
    fn each_class_reaches_only_its_routes() {
        assert!(allowed(StreamClass::Rpc, "/api/sessions?limit=5"));
        assert!(allowed(StreamClass::Rpc, "/api/terminals"));
        assert!(!allowed(StreamClass::Rpc, "/openapi.json"));
        assert!(!allowed(StreamClass::Rpc, "/api/../embed/x"));
        assert!(!allowed(StreamClass::Rpc, "/api//x"));
        assert!(!allowed(StreamClass::Rpc, "/api/x y"));
        assert!(allowed(StreamClass::Mux, "/api/events?v=2&z=1"));
        assert!(!allowed(StreamClass::Mux, "/api/events/x"));
        assert!(!allowed(StreamClass::Mux, "/api/sessions"));
        assert!(allowed(StreamClass::Term, "/api/terminals/abc-1_2/ws"));
        assert!(!allowed(StreamClass::Term, "/api/terminals//ws"));
        assert!(!allowed(StreamClass::Term, "/api/terminals/a/b/ws"));
        assert!(!allowed(StreamClass::Term, "/api/events"));
        assert!(!allowed(StreamClass::Blob, "/api/fs/read"));
        assert!(allowed(StreamClass::Blob, "/api/fs/download?path=%2Ftmp%2Fa.png&inline=true"));
        assert!(allowed(StreamClass::Blob, "/api/media/github?url=x"));
        assert!(allowed(StreamClass::Blob, "/api/fs/upload"));
        assert!(!allowed(StreamClass::Blob, "/api/sessions"));
        assert!(allowed(StreamClass::Rpc, "/rh/machine-key"));
        assert!(!allowed(StreamClass::Blob, "/rh/machine-key"));
        assert!(allowed(StreamClass::Rpc, "/rh/rotate"));
        assert!(!allowed(StreamClass::Mux, "/rh/rotate"));
        assert!(!allowed(StreamClass::Term, "/rh/rotate"));
        assert!(!allowed(StreamClass::Mux, "/rh/info"));
        assert!(!allowed(StreamClass::Rpc, "/rh/../api/x"));
        // Percent-encoded dot segments and slashes are refused too.
        for path in [
            "/api/%2e%2e/embed/x",
            "/api/%2E%2E/embed",
            "/api/sessions/%2e",
            "/api/a%2fb",
            "/api/a%5c..%5cb",
            "/rh/%2e%2e/api/x",
        ] {
            assert!(!allowed(StreamClass::Rpc, path), "{path}");
        }
        assert!(
            allowed(StreamClass::Rpc, "/api/fs/download?path=%2Ftmp%2Fa.png"),
            "the query is not the route"
        );
        assert!(allowed(StreamClass::Rpc, "/api/sessions/claude/a%20b"));
        // A plugin document's encoded slash gets no exception, and the
        // plugin routes themselves are refused by [`refusal`], not here
        // (`docs/plugins/runtime.md` §5).
        for path in [
            "/api/plugins/tasks/docs/events%2F2026-10",
            "/api/plugins/tasks/docs/events%2f2026-10/history",
            "/api/plugins/tasks/docs%2Fstate",
            "/api/plugins/tasks/batch%2Fx",
            "/api/plugins/Tasks/docs/a%2Fb",
            "/api/plugins/tasks/docs/a%2F..%2Fb",
            "/api/plugins/tasks/docs/a%2F%2Fb",
            "/api/plugins/tasks/docs/a%2Fb/other",
        ] {
            assert!(!allowed(StreamClass::Rpc, path), "{path}");
        }
    }
}
