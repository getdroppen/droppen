//! Nudges: the notification an *agent* decides to send, rather than one the
//! server infers.
//!
//! [`crate::push`] is the other half of this and stays exactly as it is. The two
//! answer different questions and are deliberately not merged:
//!
//! * **push** watches the machine. A turn ended, a run failed — the server can
//!   see those, so it decides, and its whole design is restraint (one throttle,
//!   an unread gate, two events and no more).
//! * **notify** is the agent's own judgement. Whether the thing that just
//!   happened is worth a person's attention is a question the transcript answers
//!   and the filesystem does not: "I need a decision before I can continue",
//!   "the migration is half-applied and I have stopped", "this is done". Nothing
//!   outside the conversation can tell those from an ordinary pause.
//!
//! So a session can nudge, and the skill in [`inject`] is where the judgement
//! about *when* is written down.
//!
//! # The shape of the call, and why it is not a `curl`
//!
//! The obvious implementation is to put the Bark device key in the session's
//! environment and let the agent post to Bark directly. That is the one thing
//! this module exists to avoid. A session's environment is readable by
//! everything the agent runs, its value ends up quoted in transcripts and
//! summaries, and an agent that has been asked to debug a network problem will
//! cheerfully print it. The key would be one `env` away from anywhere the
//! transcript goes.
//!
//! The chain is therefore:
//!
//! ```text
//! agent runs `rh-notify` ─POST /api/notify─> this server ──> Bark ──> phone
//!        (holds a session token)              (holds the device key)
//! ```
//!
//! and the server, being the only party that knows both, is also the only one
//! that can add what the agent could not: the session's title, and a link that
//! opens it.
//!
//! # Layout
//!
//! * [`store`] — the `0600` configuration: endpoint, base URL, token secret;
//! * [`token`] — the capability an injected `rh-notify` proves itself with;
//! * [`bark`] — the one channel, and the shape a second would take;
//! * [`inject`] — the `rh-notify` program and the skill, written into a session;
//! * [`cli`] — `remote-hand-server notify`, which is what `rh-notify` runs.

pub mod bark;
pub mod cli;
pub mod inject;
pub mod store;
pub mod token;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use gotcha::axum::extract::FromRequestParts;
use gotcha::axum::http::request::Parts;
use gotcha::axum::http::{header, HeaderMap, Method, StatusCode};
use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::extract::ApiJson;
use crate::api::{ApiRouter, AppState, SendResult};
use crate::openapi_ext::{ExtraResponses, Optional};
use crate::sessions::{Paths, SessionSummary, Source};

/// The one route an agent's `rh-notify` calls.
pub const NOTIFY_PATH: &str = "/api/notify";

/// How long a session's nudges are counted over.
const WINDOW: Duration = Duration::from_secs(10 * 60);
/// How many one session may send inside [`WINDOW`].
///
/// Generous for the use this is for — a blocked agent, a milestone, a finish —
/// and small enough that a loop which has started calling `rh-notify` after
/// every tool call stops being felt on the phone within seconds. The limit is a
/// refusal rather than a silent drop, because the agent is the one who can act
/// on being told to stop.
const BURST: usize = 6;

/// The longest title and body a nudge may carry.
///
/// A lock-screen notification shows perhaps two lines; a model asked for a short
/// one will occasionally write a paragraph. Truncating rather than refusing:
/// there is nothing the agent could usefully do about a body four characters
/// over, and losing the notification would cost more than losing the tail.
const TITLE_CAP: usize = 120;
const BODY_CAP: usize = 800;

/* ── the wire shapes ─────────────────────────────────────────────────── */

/// How loudly a nudge arrives, under Bark's own names.
///
/// Bark has a fourth, `critical`, which bypasses silent mode and Do Not Disturb.
/// It is deliberately absent: a model deciding on its own that something is
/// worth waking a person at four in the morning is exactly the failure this
/// feature would be remembered for, and the user who really wants that can set
/// it on the Bark side per group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// Added to the notification list without lighting the screen.
    Passive,
    /// An ordinary notification. The default, and right for almost everything.
    #[default]
    Active,
    /// Breaks through a Focus mode. For "I am blocked and cannot continue".
    TimeSensitive,
}

impl Level {
    /// The name Bark expects on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Passive => "passive",
            Level::Active => "active",
            Level::TimeSensitive => "timeSensitive",
        }
    }
}

/// One notification, with nothing channel-specific about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nudge {
    /// The headline: what the agent wants said.
    pub title: String,
    /// The detail under it.
    pub body: String,
    /// Which session this came from. The server's contribution, never the
    /// agent's — an agent that could name itself could name somebody else.
    pub subtitle: Option<String>,
    /// How the phone stacks these. Absent means Bark's own default.
    pub group: Option<String>,
    /// What tapping the notification opens.
    pub url: Option<String>,
    /// How loudly it arrives.
    pub level: Level,
}

/// What a client is allowed to learn about the configuration.
///
/// The endpoint is not in it. Its last path segment is the Bark device key,
/// which is the entirety of Bark's authorization, so what comes back is a mask:
/// enough to recognise the endpoint you configured, and not enough to send to
/// it. Same rule as [`crate::chats::provider::ChatProviderInfo`]'s `hasKey`, one
/// step friendlier because there is only ever one of these and a user with two
/// phones should be able to tell which one is armed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct NotifyConfigInfo {
    /// Whether sessions may nudge at all.
    pub enabled: bool,
    /// The endpoint with its device key masked, or `""` when there is none.
    pub endpoint_mask: String,
    /// Whether an endpoint is stored.
    pub has_endpoint: bool,
    /// How a phone reaches this server, for the link a notification carries.
    /// Not a secret, and returned in full so the settings page can edit it.
    pub base_url: String,
}

/// The body of `PUT /api/notify/config`; every field is a patch.
///
/// A patch rather than a whole document because the endpoint is write-only from
/// the client's side: a settings page that has never been shown the device key
/// cannot send it back, and a `PUT` of everything it *can* see would erase it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct NotifyConfigUpdate {
    /// Turn nudges on or off without forgetting the endpoint.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub enabled: Optional<bool>,
    /// A new endpoint — a full URL or a bare Bark device key. An explicit empty
    /// string forgets the stored one.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub endpoint: Optional<String>,
    /// A new base URL for notification links; an empty string drops the links.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub base_url: Optional<String>,
}

/// The body of `POST /api/notify`, which is what `rh-notify` sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct NotifyBody {
    /// The headline. Required, and the one thing the user reads first.
    pub title: String,
    /// The detail under it.
    #[serde(default)]
    pub body: String,
    /// How loudly it should arrive; `active` when absent.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub level: Optional<Level>,
    /// How the phone stacks these; the session's own title when absent.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub group: Optional<String>,
}

/* ── who is calling ──────────────────────────────────────────────────── */

/// The session a verified [`token`] names.
///
/// Put in the request extensions by [`crate::auth::guard`] and read back out by
/// the extractor below, so the handler never parses a header itself — the same
/// arrangement [`crate::auth::Identity`] has, and deliberately *not* the same
/// type. This is not an identity: it authorizes one operation on one session and
/// carries no account, so nothing that asks "who is this" can be satisfied by it
/// by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyCaller {
    /// Which CLI the session belongs to.
    pub source: Source,
    /// The session id, as it was when the process was spawned.
    pub id: String,
}

impl NotifyCaller {
    /// The `<source>/<id>` key a token is minted against.
    pub fn key(&self) -> String {
        crate::attention::seen::key_of(self.source, &self.id)
    }

    /// Split a session key back into its two halves.
    fn parse(key: &str) -> Option<Self> {
        let (source, id) = key.split_once('/')?;
        let source = match source {
            "claude" => Source::Claude,
            "codex" => Source::Codex,
            _ => return None,
        };
        (!id.is_empty()).then(|| Self { source, id: id.to_string() })
    }
}

impl<S: Send + Sync> FromRequestParts<S> for NotifyCaller {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<NotifyCaller>()
            .cloned()
            .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

/// Whether this request is the one an agent's `rh-notify` makes.
///
/// Asked by [`crate::auth::guard`] before it does any work, so the extra file
/// read below is paid on one route and not on every request.
pub fn is_notify_request(method: &Method, path: &str) -> bool {
    method == Method::POST && path == NOTIFY_PATH
}

/// The session this request's `Authorization: Bearer` proves it is, if any.
///
/// A miss is `None` on every path — an unparseable header, a secret that has
/// never been minted, a MAC that does not match — because there is exactly one
/// thing the caller may learn from a refusal, and that is that it was refused.
pub async fn caller(paths: &Paths, instance: &str, headers: &HeaderMap) -> Option<NotifyCaller> {
    let offered = bearer(headers)?;
    // Read rather than minted: `store::secret` creates one on first use, and a
    // request must never be the thing that writes a file. A machine that has
    // never configured notifications has no secret and therefore no valid token.
    let stored = store::read(paths).await.secret;
    if stored.is_empty() {
        return None;
    }
    let secret = {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(stored).ok()?
    };
    NotifyCaller::parse(&token::verify(&secret, instance, &offered)?)
}

/// The token out of `Authorization: Bearer <token>`.
///
/// Copied from [`crate::auth::RequestCredentialsExt::bearer`] rather than
/// shared with it, because that one is private to a module whose job is
/// accounts and this one's job is not.
/// The rule is what matters: a long-lived credential is read from this header
/// and from nowhere else — never a query parameter, which ends up in proxy logs,
/// and never a cookie, which would make the route forgeable cross-site.
fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/* ── the rate limit ──────────────────────────────────────────────────── */

/// When each session's recent nudges went out.
///
/// Process-global, like [`crate::push`]'s throttle and for the same reason: the
/// thing being protected is one phone, and two instances in one process notify
/// the same one. Keyed by session, so a session in a tight loop cannot spend
/// another session's budget.
static RECENT: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Take a slot for `key`, or report that the window is full.
///
/// Split from the handler and pure but for the clock, so the rule can be
/// asserted without a socket or a device key.
fn take_slot(key: &str, now: Instant) -> bool {
    let mut recent = RECENT.lock().expect("notify rate limiter poisoned");
    // The map would otherwise hold one entry per session ever notified.
    recent.retain(|_, at| at.iter().any(|sent| now.duration_since(*sent) < WINDOW));
    let slots = recent.entry(key.to_string()).or_default();
    slots.retain(|sent| now.duration_since(*sent) < WINDOW);
    if slots.len() >= BURST {
        return false;
    }
    slots.push(now);
    true
}

/* ── routes ──────────────────────────────────────────────────────────── */

/// What a nudge is worth telling the caller about the session it came from.
async fn summary_of(paths: &Paths, source: Source, id: &str) -> Option<SessionSummary> {
    crate::sessions::find_summary(paths, source, id).await
}

/// Trim, collapse and cap one field of a nudge.
///
/// Newlines are collapsed because a lock-screen notification renders them as
/// spaces anyway, and a body that arrived with its layout intact would only look
/// broken on the phone.
fn tidy(raw: &str, cap: usize) -> String {
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= cap {
        return flat;
    }
    flat.chars().take(cap.saturating_sub(1)).collect::<String>() + "\u{2026}"
}

/// Where the stored configuration currently points.
#[api(id = "notify_config", group = "notify", summary = "Read the notification channel")]
pub async fn notify_config(State(state): State<AppState>) -> Json<NotifyConfigInfo> {
    Json(store::read(state.paths.as_ref()).await.info())
}

/// Change it. Every field is optional; an empty string clears one.
#[api(id = "update_notify_config", group = "notify", summary = "Configure the notification channel")]
pub async fn update_notify_config(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<NotifyConfigUpdate>,
) -> Result<Json<NotifyConfigInfo>, ApiError> {
    // Normalised — and refused — before anything is written, so a mistyped
    // endpoint is a message under the field rather than a notification that
    // never arrives.
    let endpoint = match body.endpoint.as_deref() {
        Some(raw) if raw.trim().is_empty() => Some(String::new()),
        Some(raw) => Some(
            bark::normalize_endpoint(raw)
                .ok_or_else(|| ApiError::bad_request("that is neither a Bark device key nor a Bark server URL ending in one"))?,
        ),
        None => None,
    };
    if let Some(endpoint) = endpoint.as_deref().filter(|value| !value.is_empty()) {
        if !bark::endpoint_is_acceptable(endpoint) {
            return Err(ApiError::bad_request("the notification endpoint must be an https URL"));
        }
    }
    let base_url = body.base_url.as_deref().map(|raw| raw.trim().trim_end_matches('/').to_string());

    let stored = store::update(state.paths.as_ref(), |data| {
        if let Some(enabled) = body.enabled.0 {
            data.enabled = enabled;
        }
        if let Some(endpoint) = endpoint {
            data.endpoint = endpoint;
        }
        if let Some(base_url) = base_url {
            data.base_url = base_url;
        }
    })
    .await
    .map_err(|error| ApiError::internal(format!("failed to store the notification settings: {error}")))?;
    Ok(Json(stored.info()))
}

/// Send one notification to the configured channel, from the settings page.
///
/// The only way to find out whether a device key is right is to use it, and
/// finding out by waiting for an agent to be blocked is not finding out.
#[api(id = "notify_test", group = "notify", summary = "Send a test notification")]
pub async fn notify_test(State(state): State<AppState>) -> Result<Json<SendResult>, ApiError> {
    let stored = store::read(state.paths.as_ref()).await;
    if stored.endpoint.is_empty() {
        return Err(ApiError::bad_request("no notification endpoint is configured"));
    }
    let nudge = Nudge {
        title: "Droppen".to_string(),
        body: format!("Test notification from the {} instance.", state.instance.name()),
        subtitle: None,
        group: Some("Droppen".to_string()),
        url: (!stored.base_url.is_empty()).then(|| stored.base_url.clone()),
        level: Level::Active,
    };
    bark::send(&stored.endpoint, &nudge)
        .await
        .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, format!("the notification channel refused it: {error}")))?;
    Ok(Json(SendResult { accepted: true }))
}

/// The route an agent's `rh-notify` calls.
#[api(id = "notify", group = "notify", summary = "Send a notification from a session")]
pub async fn notify(
    State(state): State<AppState>,
    // `#[api(skip)]` for [`crate::auth::Identity`]'s reason: the caller is
    // proved by a header the guard has already read, and gotcha would otherwise
    // ask this parameter for an OpenAPI contribution it has none of.
    #[api(skip)] caller: NotifyCaller,
    ApiJson(body): ApiJson<NotifyBody>,
) -> Result<Json<SendResult>, ApiError> {
    let stored = store::read(state.paths.as_ref()).await;
    // Reachable even though the skill is only injected when this is live: the
    // user may have turned notifications off while the session was running, and
    // the process was handed its environment at spawn.
    if !stored.is_live() {
        return Err(ApiError::bad_request("notifications are turned off for this server"));
    }
    let title = tidy(&body.title, TITLE_CAP);
    if title.is_empty() {
        return Err(ApiError::bad_request("a notification needs a title"));
    }
    if !take_slot(&caller.key(), Instant::now()) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "this session has sent {BURST} notifications in the last {} minutes; save the next one for something that needs it",
                WINDOW.as_secs() / 60
            ),
        ));
    }

    let summary = summary_of(state.paths.as_ref(), caller.source, &caller.id).await;
    let nudge = Nudge {
        title,
        body: tidy(&body.body, BODY_CAP),
        // The session's own name, and never anything the caller sent: an agent
        // that could write this field could put another session's name on its
        // own notification.
        subtitle: summary.as_ref().map(|summary| summary.title.clone()),
        group: body
            .group
            .0
            .map(|group| tidy(&group, TITLE_CAP))
            .filter(|group| !group.is_empty())
            .or_else(|| summary.as_ref().map(|summary| summary.title.clone())),
        // The SPA's own route for a session (`web/src/app/router.tsx`), so the
        // notification opens the thing it is about. Absent when nobody has said
        // how a phone reaches this server, which is better than a dead link.
        url: (!stored.base_url.is_empty()).then(|| format!("{}/sessions/{}/{}", stored.base_url, caller.source, caller.id)),
        level: body.level.0.unwrap_or_default(),
    };
    bark::send(&stored.endpoint, &nudge).await.map_err(|error| {
        // Logged as well as returned: `rh-notify` prints the message to an agent
        // that may or may not repeat it, and a channel that has started refusing
        // everything should be visible in the server's own log.
        tracing::warn!(session = %caller.key(), %error, "notify: delivery failed");
        ApiError::new(StatusCode::BAD_GATEWAY, format!("the notification channel refused it: {error}"))
    })?;
    tracing::info!(session = %caller.key(), "notify: delivered a nudge");
    Ok(Json(SendResult { accepted: true }))
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/notify/config", notify_config)
        .put("/api/notify/config", update_notify_config)
        .post("/api/notify/test", notify_test)
        .post(NOTIFY_PATH, notify)
}

/// Everything gotcha cannot express about those routes.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe_final("GET", "/api/notify/config", 200, "Where notifications go, with the device key masked")
        .describe("PUT", "/api/notify/config", 200, "The updated channel, with the device key masked")
        .error("PUT", "/api/notify/config", 400, "The endpoint is not a Bark device key or https URL")
        .describe("POST", "/api/notify/test", 200, "A test notification was accepted by the channel")
        .error("POST", "/api/notify/test", 400, "No endpoint is configured")
        .error("POST", "/api/notify/test", 502, "The channel refused it")
        .describe("POST", NOTIFY_PATH, 200, "The notification was accepted by the channel")
        .error("POST", NOTIFY_PATH, 400, "Notifications are off, or the title is empty")
        .error("POST", NOTIFY_PATH, 401, "No valid session token")
        .error("POST", NOTIFY_PATH, 429, "This session has nudged too often")
        .error("POST", NOTIFY_PATH, 502, "The channel refused it");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_key_round_trips_through_the_caller() {
        let caller = NotifyCaller::parse("claude/abc-123").expect("parsed");
        assert_eq!(caller.source, Source::Claude);
        assert_eq!(caller.id, "abc-123");
        assert_eq!(caller.key(), "claude/abc-123");
        assert_eq!(NotifyCaller::parse("codex/019a").expect("parsed").source, Source::Codex);

        assert_eq!(NotifyCaller::parse("claude/"), None, "a source with no session is not a session");
        assert_eq!(NotifyCaller::parse("gemini/abc"), None);
        assert_eq!(NotifyCaller::parse("abc-123"), None);
    }

    #[test]
    fn only_this_route_pays_for_the_lookup() {
        assert!(is_notify_request(&Method::POST, NOTIFY_PATH));
        assert!(!is_notify_request(&Method::GET, NOTIFY_PATH));
        assert!(!is_notify_request(&Method::POST, "/api/notify/test"));
        assert!(!is_notify_request(&Method::POST, "/api/notify/config"));
    }

    #[test]
    fn a_nudge_is_flattened_and_capped_rather_than_refused() {
        assert_eq!(tidy("  hello   world \n", 40), "hello world");
        assert_eq!(tidy("one\ntwo\nthree", 40), "one two three", "a lock screen renders newlines as spaces");
        assert_eq!(tidy("", 40), "");
        assert_eq!(tidy("     ", 40), "");

        let long = "x".repeat(200);
        let capped = tidy(&long, 10);
        assert_eq!(capped.chars().count(), 10);
        assert!(capped.ends_with('\u{2026}'), "the cut is visible: {capped}");

        // Counted in characters, not bytes: a cap that split a multi-byte
        // character would panic on the slice.
        let cjk = "需要你决定用哪个方案".repeat(30);
        assert_eq!(tidy(&cjk, 10).chars().count(), 10);
    }

    #[test]
    fn the_window_lets_a_session_speak_up_and_then_stops_a_loop() {
        let key = "claude/rate-limit-test";
        let start = Instant::now();
        for n in 0..BURST {
            assert!(take_slot(key, start), "nudge {n} is inside the burst");
        }
        assert!(!take_slot(key, start), "the {}th is not", BURST + 1);
        // Another session has its own budget: one agent in a loop must not
        // silence the one that is actually blocked.
        assert!(take_slot("claude/some-other-session", start), "the limit is per session");
        // And the window really is a window.
        assert!(take_slot(key, start + WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn a_bearer_token_is_read_out_of_the_one_header_that_carries_one() {
        let header_map = |value: &str| {
            let mut map = HeaderMap::new();
            map.insert(header::AUTHORIZATION, value.parse().expect("header value"));
            map
        };
        assert_eq!(bearer(&header_map("Bearer rhn1.a.b")).as_deref(), Some("rhn1.a.b"));
        assert_eq!(bearer(&header_map("bearer rhn1.a.b")).as_deref(), Some("rhn1.a.b"));
        assert_eq!(bearer(&header_map("Basic rhn1.a.b")), None);
        assert_eq!(bearer(&header_map("Bearer  ")), None);
        assert_eq!(bearer(&HeaderMap::new()), None);
    }

    #[test]
    fn the_level_names_are_barks_own() {
        assert_eq!(Level::default(), Level::Active);
        assert_eq!(Level::Passive.as_str(), "passive");
        assert_eq!(Level::TimeSensitive.as_str(), "timeSensitive");
        // The wire form and the string Bark is sent must not drift apart.
        for level in [Level::Passive, Level::Active, Level::TimeSensitive] {
            let wire = serde_json::to_value(level).expect("json");
            assert_eq!(wire.as_str(), Some(level.as_str()));
        }
    }
}
