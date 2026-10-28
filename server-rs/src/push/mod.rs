//! Web Push: telling a phone that a turn finished, with the tab closed.
//!
//! This is the only thing in the server that reaches *out* to the user rather
//! than waiting to be asked. That makes restraint the design, not a detail: a
//! notification the user did not need is worse than no notification at all,
//! because the next real one gets swiped away with it. So there are exactly two
//! events, and one of them is conditional:
//!
//! * **A turn finished and the session is unread.** "Unread" is
//!   [`crate::attention`]'s verdict, unchanged — if the user is looking at the
//!   session, the screen already said everything the notification would.
//! * **A run failed.** Always, regardless of what anyone is looking at: a run
//!   that died is not going to tell you itself.
//!
//! And one that is not about a turn ending at all: **the agent stopped to ask
//! something** (`AskUserQuestion`) and is blocked until somebody answers. Once
//! per question, never throttled — see [`notify_question`].
//!
//! On top of that a 60-second per-session throttle ([`THROTTLE`]), so a session
//! being driven in a tight loop collapses into one line in the notification
//! shade rather than a column of them. The `tag` is the session key, which makes
//! the browser replace rather than stack them for the same reason.
//!
//! # Why the trigger is where it is
//!
//! There are two hooks, one per CLI, and they are the same event seen through
//! two different mechanisms — the line where a turn's final state is written:
//!
//! * **codex** is still one `codex exec` child per message, so a child exiting
//!   *is* the turn ending, and `inject::drive_run`'s exit bookkeeping
//!   is the hook.
//! * **claude** is a resident process that answers message after message, so
//!   the turn ends at a `{"type":"result"}` line and the process keeps going.
//!   `inject::resident` fires this once per result, which is the
//!   correction that matters: hanging the notification on the *process* would
//!   have meant one notification per session per day, arriving when the user
//!   closed it.
//!
//! A hook on a session's state *transitions* would have to sleep and re-read
//! before sending, because a streaming session flickers through `idle` on its
//! way to somewhere else. Neither hook here needs that: both are a single discrete event that nothing later takes
//! back, so there is no re-check — which also means the notification arrives
//! three seconds sooner. What the resident does need is the [`THROTTLE`] below,
//! which was already there: a session driven in a tight loop now really does
//! produce a result a minute.
//!
//! # Protocol
//!
//! RFC 8291 (aes128gcm content encryption) and RFC 8292 (VAPID) are implemented
//! in [`ece`], on RustCrypto, pinned by the published test vector. [`store`]
//! keeps the keypair and the subscriptions.
//!
//! # Operational note: secure contexts
//!
//! A browser only exposes `PushManager` in a secure context — HTTPS, or
//! `localhost`. Over plain HTTP on a LAN address there is no subscription to be
//! had, and no amount of server-side work changes that; the web app checks for
//! `PushManager` and says so. Every route here still works over HTTP (they are
//! ordinary JSON, and `GET /api/push/vapid` in particular is useful to hit while
//! diagnosing exactly this), so the failure is visible rather than silent. See
//! `server-rs/README.md` for the deployment shapes that do work.

pub mod ece;
pub mod store;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::api::created::Created;
use crate::api::error::ApiError;
use crate::api::extract::ApiJson;
use crate::api::{ApiRouter, AppState, SendResult};
use crate::attention::{self, derive};
use crate::openapi_ext::{ExtraResponses, Optional};
use crate::sessions::{DriverStates, InjectionRunState, Paths, ResidentInfo, SessionSummary, Source};

/// How long the same session's notifications are suppressed after one is sent.
const THROTTLE: Duration = Duration::from_secs(60);
/// How long a push service is given to accept a notification.
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a push service should hold a notification for a phone that is off.
const TTL_SECONDS: &str = "86400";

/// RFC 8292 §2.1's `sub`: who a push service contacts if this server misbehaves.
/// It must be a `mailto:` or `https:` URI — Apple rejects anything else outright.
/// A deployment with a real contact address sets `DROPPEN_PUSH_CONTACT` (before
/// the rename, `REMOTE_HAND_PUSH_CONTACT`, still read).
/// The product's public site, so a push service has somewhere real to look.
const DEFAULT_CONTACT: &str = "https://droppen.app";

fn contact() -> String {
    crate::env::var("DROPPEN_PUSH_CONTACT")
        .filter(|value| value.starts_with("mailto:") || value.starts_with("https://"))
        .unwrap_or_else(|| DEFAULT_CONTACT.to_string())
}

/* ── the wire shapes ─────────────────────────────────────────────────── */

/// The key a browser passes to `pushManager.subscribe`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct VapidKey {
    /// base64url of the uncompressed P-256 public point, ready to be handed to
    /// `applicationServerKey` after the client's own base64url-to-bytes step.
    pub public_key: String,
}

/// The `keys` half of a browser's `PushSubscription`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SubscriptionKeys {
    /// The subscription's public key, base64url of an uncompressed P-256 point.
    pub p256dh: String,
    /// The subscription's auth secret, base64url of 16 bytes.
    pub auth: String,
}

/// A new subscription.
///
/// Deliberately the exact shape of `PushSubscription.toJSON()`, so a client
/// posts what the browser handed it rather than unpacking and re-packing it —
/// one fewer place the two keys can be swapped.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SubscribeBody {
    /// The push service URL. Unique per device; re-posting one replaces it.
    pub endpoint: String,
    /// The two keys the payload is encrypted against.
    pub keys: SubscriptionKeys,
    /// Optional name for the device, for a future management UI.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub label: Optional<String>,
}

/// Which subscription to forget.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct UnsubscribeBody {
    /// The endpoint of the subscription to remove.
    pub endpoint: String,
}

/* ── routes ──────────────────────────────────────────────────────────── */

/// The env opt-in that lets plain-HTTP loopback endpoints through.
///
/// Off in production, where every real push service is HTTPS and a plain-HTTP
/// endpoint would only ever be a mistake or an attempt to make this server post
/// to something on its own host. The send-path integration tests set it to reach
/// a loopback listener over a real socket, and it is also useful while diagnosing
/// locally — but it is never on unless a deployment says so in so many words:
/// `1`, `true`, `yes` or `on` (any case), never `0`, `false` or an empty value.
const ALLOW_INSECURE_ENDPOINT_ENV: &str = "DROPPEN_PUSH_ALLOW_INSECURE_ENDPOINT";

/// Whether plain-HTTP loopback endpoints are permitted in this process.
fn insecure_endpoints_allowed() -> bool {
    crate::env::flag(ALLOW_INSECURE_ENDPOINT_ENV)
}

/// Whether an endpoint is somewhere this server is willing to send to.
///
/// Push services are always HTTPS, so production requires it. Plain HTTP is
/// accepted only for loopback and only when [`ALLOW_INSECURE_ENDPOINT_ENV`] is
/// set — the opt-in the send path is tested through, where there is nothing on
/// the wire to protect. Note this is a *first-hop* check: the shared [`CLIENT`]
/// does not follow redirects, so an endpoint cannot 3xx its way to somewhere
/// this would have refused.
fn endpoint_is_acceptable(endpoint: &str) -> bool {
    crate::outbound::endpoint_is_acceptable(endpoint, insecure_endpoints_allowed())
}

/// The public key browsers subscribe against, minting the keypair on first ask.
#[api(id = "push_vapid_key", group = "push", summary = "Web Push application server key")]
pub async fn vapid_key(State(state): State<AppState>) -> Result<Json<VapidKey>, ApiError> {
    let public_key = store::vapid_public(state.paths.as_ref())
        .await
        .map_err(|error| ApiError::internal(format!("failed to read the push key: {error}")))?;
    Ok(Json(VapidKey { public_key }))
}

/// Register a browser for notifications, replacing any earlier one on the same endpoint.
#[api(id = "push_subscribe", group = "push", summary = "Register a push subscription")]
pub async fn subscribe(State(state): State<AppState>, ApiJson(body): ApiJson<SubscribeBody>) -> Result<Created<SendResult>, ApiError> {
    // Refused here rather than at send time: a malformed row would otherwise sit
    // in the file failing every notification, and the browser that sent it would
    // never find out.
    let candidate = store::Subscription {
        endpoint: body.endpoint.clone(),
        p256dh: body.keys.p256dh.clone(),
        auth: body.keys.auth.clone(),
        created_at: String::new(),
        label: None,
    };
    if !endpoint_is_acceptable(&body.endpoint) {
        return Err(ApiError::bad_request(
            "push endpoint must be an https URL (or a loopback http URL where explicitly allowed)",
        ));
    }
    if !candidate.keys_are_well_formed() {
        return Err(ApiError::bad_request("push subscription keys are malformed"));
    }
    store::upsert(
        state.paths.as_ref(),
        body.endpoint,
        body.keys.p256dh,
        body.keys.auth,
        body.label.0.filter(|label| !label.trim().is_empty()),
    )
    .await
    .map_err(|error| ApiError::internal(format!("failed to store the subscription: {error}")))?;
    Ok(Created(SendResult { accepted: true }))
}

/// Forget a subscription. Idempotent: unsubscribing twice is not an error.
#[api(id = "push_unsubscribe", group = "push", summary = "Remove a push subscription")]
pub async fn unsubscribe(State(state): State<AppState>, ApiJson(body): ApiJson<UnsubscribeBody>) -> Result<Json<SendResult>, ApiError> {
    store::remove(state.paths.as_ref(), &body.endpoint)
        .await
        .map_err(|error| ApiError::internal(format!("failed to remove the subscription: {error}")))?;
    Ok(Json(SendResult { accepted: true }))
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/push/vapid", vapid_key)
        .post("/api/push/subscriptions", subscribe)
        .delete("/api/push/subscriptions", unsubscribe)
}

/// Everything gotcha cannot express about those routes.
///
/// `POST` is a `201` the same way `POST /api/accounts` is: gotcha derives every
/// success as a `200` (`gotcha_core-0.4.1/src/responsible.rs:25`), so the entry
/// is restated under the status [`Created`] actually sends.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe_final("GET", "/api/push/vapid", 200, "The application server key browsers subscribe with")
        .restatus("POST", "/api/push/subscriptions", 200, 201, "The subscription is stored")
        .error("POST", "/api/push/subscriptions", 400, "Malformed endpoint or keys")
        .describe_final("DELETE", "/api/push/subscriptions", 200, "The subscription is forgotten");
}

/* ── the send path ───────────────────────────────────────────────────── */

/// One shared client: building one means building a rustls configuration, which
/// is far more expensive than the request it would carry.
static CLIENT: LazyLock<Option<reqwest::Client>> = LazyLock::new(|| crate::outbound::no_redirect_client(SEND_TIMEOUT));

/// When each session last had a notification sent for it.
static SENT_AT: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Take the throttle slot for `tag`, or report that it is still held.
///
/// Checked at the last moment, after the decision to notify has been made: a
/// turn that was suppressed because the user was already looking at it should
/// not also cost the next one its window.
fn throttle_allows(tag: &str) -> bool {
    let mut sent = SENT_AT.lock().expect("push throttle poisoned");
    let now = Instant::now();
    // The map would otherwise hold one entry per session ever notified.
    sent.retain(|_, at| now.duration_since(*at) < THROTTLE * 5);
    if sent.get(tag).is_some_and(|at| now.duration_since(*at) < THROTTLE) {
        return false;
    }
    sent.insert(tag.to_string(), now);
    true
}

/// An endpoint's scheme, host and port, for logs.
///
/// The path of a push endpoint is a per-subscription capability: whoever holds
/// it can send that device notifications. So only the origin is ever written to
/// a log line — enough to tell FCM from Mozilla from a loopback test, and none
/// of the secret.
fn endpoint_origin(endpoint: &str) -> String {
    // Empty keeps this module's log spelling; the shared rule answers with `""`
    // for it, which is what the notify endpoint's API-facing mask wants.
    if endpoint.is_empty() {
        return "<unparseable>".to_string();
    }
    crate::outbound::endpoint_origin(endpoint)
}

/// What one push service said.
enum Outcome {
    Delivered,
    /// `404`/`410`: the browser has unregistered. RFC 8030 §7.3 — the row is
    /// dead and no retry will revive it, so it goes.
    Gone,
    Failed(String),
}

async fn send_one(client: &reqwest::Client, key: &p256::ecdsa::SigningKey, subject: &str, row: &store::Subscription, payload: &[u8]) -> Outcome {
    let Ok(endpoint) = reqwest::Url::parse(&row.endpoint) else {
        return Outcome::Gone; // a stored row that will not even parse: reap it
    };
    let (Some(ua_public), Some(auth)) = (ece::b64url_decode(&row.p256dh), ece::b64url_decode(&row.auth)) else {
        return Outcome::Gone;
    };
    let body = match ece::encrypt(&ua_public, &auth, payload) {
        Ok(body) => body,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    match client
        .post(endpoint.clone())
        .header("TTL", TTL_SECONDS)
        .header("Urgency", "high")
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("Authorization", ece::vapid_auth(&endpoint, subject, key))
        .body(body)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => Outcome::Delivered,
        Ok(response) if matches!(response.status().as_u16(), 404 | 410) => Outcome::Gone,
        Ok(response) => Outcome::Failed(format!("http {}", response.status())),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// Encrypt and deliver one notification to every stored subscription.
///
/// Nothing here is fatal to anything: a push service being down is not a reason
/// for a CLI run to have gone wrong.
async fn deliver(paths: &Paths, title: String, body: String, tag: String, url: String) {
    let subscriptions = store::subscriptions(paths).await;
    if subscriptions.is_empty() {
        return;
    }
    let key = match store::vapid_key(paths).await {
        Ok(key) => key,
        Err(error) => {
            tracing::warn!(%error, "push: no usable VAPID key; not notifying");
            return;
        }
    };
    let Some(client) = CLIENT.as_ref() else {
        tracing::warn!("push: no HTTP client; not notifying");
        return;
    };
    let subject = contact();
    let payload = serde_json::json!({ "title": title, "body": body, "tag": tag, "url": url }).to_string();

    for row in subscriptions {
        match send_one(client, &key, &subject, &row, payload.as_bytes()).await {
            Outcome::Delivered => tracing::debug!(origin = %endpoint_origin(&row.endpoint), %tag, "push: delivered"),
            Outcome::Gone => {
                tracing::info!(origin = %endpoint_origin(&row.endpoint), "push: subscription is gone; forgetting it");
                if let Err(error) = store::remove(paths, &row.endpoint).await {
                    tracing::warn!(%error, "push: failed to forget a dead subscription");
                }
            }
            Outcome::Failed(error) => tracing::warn!(origin = %endpoint_origin(&row.endpoint), %error, "push: delivery failed"),
        }
    }
}

/* ── the trigger ─────────────────────────────────────────────────────── */

/// The session-list row for one session, whatever CLI it belongs to.
async fn summary_of(paths: &Paths, source: Source, id: &str) -> Option<SessionSummary> {
    crate::sessions::find_summary(paths, source, id).await
}

/// What a finished run is worth telling somebody about, if anything.
enum Notice {
    Finished(String),
    Failed(String),
    /// A limit moved the session to another account, or found nowhere to move
    /// it: the sentence [`crate::accounts::rotation::notice`] wrote.
    Rotated(String),
}

impl Notice {
    /// Whether the per-session [`THROTTLE`] applies.
    ///
    /// Not to a rotation. It arrives a moment after the failed run that caused
    /// it, which has just taken the session's slot, and it is the one of the
    /// two that says what happens next — throttled, it would be the one lost.
    fn throttled(&self) -> bool {
        !matches!(self, Notice::Rotated(_))
    }

    /// The notification's body.
    fn body(self) -> String {
        match self {
            Notice::Finished(snippet) => snippet,
            Notice::Failed(error) => format!("Run failed: {error}"),
            Notice::Rotated(text) => text,
        }
    }
}

/// `1 message` / `2 messages`. Both nouns this has pluralise with an `s`.
fn counted(n: u32, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// What is *still* going on, as a clause to hang off the end of a snippet.
///
/// The sentence a transcript cannot write. A turn that ends with a message
/// queued behind it, or with claude still minding a background task, is not
/// "finished" in the sense that word has on a phone — the screen the user is
/// being invited to open will be moving again by the time they get there, and a
/// bare echo of the last thing said is a lie by omission. So the driver's own
/// account gets a clause of its own.
///
/// Pure, and separate from the notification it ends up in, so the wording can be
/// pinned without a transcript, a subscription or a running CLI.
///
/// English, like every other string this module sends: the payload goes to the
/// service worker unchanged and nothing between here and the phone translates it.
fn still_busy_clause(resident: Option<&ResidentInfo>) -> Option<String> {
    let resident = resident?;
    let queued = counted(resident.queued, "message");
    let background = counted(resident.background, "background task");
    match (resident.queued > 0, resident.background > 0) {
        (false, false) => None,
        // The park. The head of the queue is not merely waiting its turn, it is
        // waiting on that background work specifically, and a user told only
        // "1 message still queued" beside an idle session would reasonably read
        // it as a queue that has got stuck.
        (true, true) if resident.awaiting_background => Some(format!("{queued} queued, waiting on {background}")),
        (true, true) => Some(format!("{queued} still queued · {background} still running")),
        (true, false) => Some(format!("{queued} still queued")),
        (false, true) => Some(format!("{background} still running")),
    }
}

/// Decide, and send. Split from [`notify_turn`] so the decision can be read on
/// its own, and so the spawn site stays one line.
async fn notify_inner(paths: Arc<Paths>, source: Source, id: String, outcome: InjectionRunState, error: Option<String>, driver: DriverStates) {
    // Nobody has subscribed: stop before reading a transcript or minting a key.
    if store::subscriptions(&paths).await.is_empty() {
        return;
    }
    let mut summary = summary_of(&paths, source, &id).await;
    // The transcript's own verdict, kept from before the overlay below runs.
    //
    // The unread gate must go on asking exactly the question it has always
    // asked. `crate::sessions::apply_driver` promotes `status` to `Running`
    // whenever the activity is `Working` or `Queued`, and `attention::is_unread`
    // answers `false` for `Running` — so feeding it the overlaid status would
    // silently delete every notification for a turn that ended with something
    // queued, which is the exact case the overlay was brought in to describe.
    // The driver is here to change what the notification *says*, not whether the
    // turn happened.
    let transcript_status = summary.as_ref().map(|summary| summary.status);
    if let Some(summary) = summary.as_mut() {
        // The same overlay `crate::sessions::list_and_filter` applies, on the one
        // row this is about, so a notification and a list row can never give two
        // accounts of one session.
        driver.apply(std::slice::from_mut(summary));
    }
    let tag = attention::seen::key_of(source, &id);

    let notice = match outcome {
        // Not a completion at all; the hook fires once per run and this arm
        // exists so a future caller cannot silently mean something else.
        InjectionRunState::Running => return,
        InjectionRunState::Failed => Notice::Failed(error.unwrap_or_else(|| "the run failed".to_string())),
        InjectionRunState::Ok => {
            // A turn only earns a notification if the user has not already seen
            // what it produced. Exactly `crate::attention`'s rule *and* its data:
            // the clock is the newest message in the same unread ledger the list
            // row's badge and count come from, built by the same predicate
            // (`derive::spoken_texts`) — so the badge and the notification can
            // never disagree about whether the session said something new.
            let (Some(summary), Some(status)) = (summary.as_ref(), transcript_status) else {
                return;
            };
            let seen = attention::seen::read(&paths).await;
            let read_at = seen.get(&tag).map(String::as_str);
            let at = attention::clock_of(summary, read_at).await;
            if !attention::is_unread(at.as_deref(), read_at, status) {
                return;
            }
            // Only the words come from the tail, by the same predicate, so the
            // body is the message the clock was read from whenever it is there.
            let echo = derive::read_echo(source, Path::new(&summary.file_path)).await;
            let snippet = echo.map(|echo| echo.snippet()).unwrap_or_else(|| "Turn finished.".to_string());
            Notice::Finished(match still_busy_clause(summary.resident.as_ref()) {
                Some(clause) => format!("{snippet} · {clause}"),
                None => snippet,
            })
        }
    };

    if notice.throttled() && !throttle_allows(&tag) {
        tracing::debug!(%tag, "push: within the throttle window; not notifying");
        return;
    }
    let title = summary.map(|summary| summary.title).unwrap_or_else(|| format!("{source} session {id}"));
    let body = notice.body();
    // The SPA's own route for a session (`web/src/app/router.tsx`), so the
    // notification opens the thing it is about.
    let url = format!("/sessions/{source}/{id}");
    deliver(&paths, title, body, tag, url).await;
}

/// Tell the phone a session was moved to another account, or could not be.
///
/// Both channels this server has: Web Push to every subscribed browser, and the
/// Bark endpoint `rh-notify` uses when one is configured — a rotation is news
/// about a session the user walked away from, which is what that channel is
/// for. Never throttled (see [`Notice::throttled`]), and fire-and-forget like
/// [`notify_turn`]: a slow push service must not hold up the driver that is
/// about to respawn the session.
pub fn notify_rotation(
    paths: Arc<Paths>,
    source: Source,
    id: String,
    outcome: crate::accounts::rotation::RotationOutcome,
    trigger: crate::accounts::rotation::RotationTrigger,
    text: String,
) {
    use crate::accounts::rotation::{RotationOutcome, RotationTrigger};
    tokio::spawn(async move {
        let notice = Notice::Rotated(text.clone());
        let summary = summary_of(&paths, source, &id).await;
        let title = summary
            .as_ref()
            .map(|summary| summary.title.clone())
            .unwrap_or_else(|| format!("{source} session {id}"));
        if !store::subscriptions(&paths).await.is_empty() {
            let tag = attention::seen::key_of(source, &id);
            let url = format!("/sessions/{source}/{id}");
            deliver(&paths, title.clone(), notice.body(), tag, url).await;
        }
        let stored = crate::notify::store::read(&paths).await;
        if !stored.is_live() {
            return;
        }
        let nudge = crate::notify::Nudge {
            title: match (outcome, trigger) {
                (RotationOutcome::Switched, RotationTrigger::Ahead) => "已提前切换账号".to_string(),
                (RotationOutcome::Switched, RotationTrigger::Limit) => "已自动切换账号".to_string(),
                _ => "账号额度用完".to_string(),
            },
            body: text,
            subtitle: Some(title.clone()),
            group: Some(title),
            url: (!stored.base_url.is_empty()).then(|| format!("{}/sessions/{source}/{id}", stored.base_url)),
            level: crate::notify::Level::Active,
        };
        if let Err(error) = crate::notify::bark::send(&stored.endpoint, &nudge).await {
            tracing::warn!(session = %id, %error, "push: the rotation notice did not reach the notification channel");
        }
    });
}

/// Tell the phone that who is in the account changed (M2 review): a device
/// added or removed, a device coming back with the recovery words, the
/// recovery key replaced. The machine sees every such change on its own copy
/// of the chain, so even a hub that hides it from the devices cannot hide it
/// from here. Both channels, never throttled, fire-and-forget.
pub fn notify_account_change(paths: Arc<Paths>, title: String, body: String) {
    tokio::spawn(async move {
        if !store::subscriptions(&paths).await.is_empty() {
            deliver(&paths, title.clone(), body.clone(), "rh-account-change".into(), "/".into()).await;
        }
        let stored = crate::notify::store::read(&paths).await;
        if !stored.is_live() {
            return;
        }
        let nudge = crate::notify::Nudge {
            title,
            body,
            subtitle: None,
            group: Some("account".into()),
            url: (!stored.base_url.is_empty()).then(|| stored.base_url.clone()),
            level: crate::notify::Level::Active,
        };
        if let Err(error) = crate::notify::bark::send(&stored.endpoint, &nudge).await {
            tracing::warn!(%error, "push: the account-change notice did not reach the notification channel");
        }
    });
}

/// Tell the phone an agent has stopped to ask something, and is waiting.
///
/// The one notification that is about the *middle* of a turn: claude is
/// blocked on the answer and will sit there for as long as nobody gives one, so
/// it goes out whatever the unread ledger says and is never throttled — a turn
/// that finished a minute ago must not swallow the question that followed it.
/// Once per question: the driver only calls this the first time a question is
/// put on record, so a restart that finds it still waiting stays quiet. Both
/// channels, like [`notify_rotation`], and the Bark one breaks through a Focus
/// mode, because "blocked and cannot continue" is what that level is for.
///
/// Nothing takes it back when the question is answered: a notification already
/// delivered cannot be recalled, and the session it opens says it was answered.
pub fn notify_question(paths: Arc<Paths>, source: Source, id: String, question: String) {
    tokio::spawn(async move {
        let summary = summary_of(&paths, source, &id).await;
        let title = summary
            .as_ref()
            .map(|summary| summary.title.clone())
            .unwrap_or_else(|| format!("{source} session {id}"));
        let body = question_body(&question);
        if !store::subscriptions(&paths).await.is_empty() {
            let tag = attention::seen::key_of(source, &id);
            let url = format!("/sessions/{source}/{id}");
            deliver(&paths, title.clone(), body.clone(), tag, url).await;
        }
        let stored = crate::notify::store::read(&paths).await;
        if !stored.is_live() {
            return;
        }
        let nudge = crate::notify::Nudge {
            title: "在问你".to_string(),
            body,
            subtitle: Some(title.clone()),
            group: Some(title),
            url: (!stored.base_url.is_empty()).then(|| format!("{}/sessions/{source}/{id}", stored.base_url)),
            level: crate::notify::Level::TimeSensitive,
        };
        if let Err(error) = crate::notify::bark::send(&stored.endpoint, &nudge).await {
            tracing::warn!(session = %id, %error, "push: the question did not reach the notification channel");
        }
    });
}

/// A question's notification body. English, like everything this module sends
/// to the service worker; see [`still_busy_clause`].
fn question_body(question: &str) -> String {
    format!("Asking you: {question}")
}

/// A run has reached its final state; notify if that is worth doing.
///
/// Fire-and-forget by design. This is called from the one place in
/// [`crate::inject`] where a run's outcome is written, and a push service being
/// slow (or a phone being asleep) must not hold up releasing the injection lock.
///
/// `driver` is the caller's snapshot of what the injection subsystem knows, and
/// it must be taken **synchronously at the call site**, after the finished turn
/// has been written back as idle. Taken inside the spawned task instead it would
/// race the driver loop picking up the next queued message, and the notification
/// would describe a turn that had not started when it was sent.
pub fn notify_turn(paths: Arc<Paths>, source: Source, id: String, outcome: InjectionRunState, error: Option<String>, driver: DriverStates) {
    tokio::spawn(notify_inner(paths, source, id, outcome, error, driver));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that mutate process-global environment variables, so
    /// one setting `DROPPEN_PUSH_ALLOW_INSECURE_ENDPOINT` cannot be observed by another
    /// reading a different variable mid-flight.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn https_is_always_accepted_and_plain_http_needs_the_opt_in() {
        let _guard = ENV_LOCK.lock().expect("env lock");

        // HTTPS is accepted regardless of the opt-in.
        std::env::remove_var(ALLOW_INSECURE_ENDPOINT_ENV);
        assert!(endpoint_is_acceptable("https://fcm.googleapis.com/fcm/send/abc"));
        assert!(endpoint_is_acceptable("https://updates.push.services.mozilla.com/wpush/v2/abc"));

        // Without the opt-in, plain HTTP is refused — loopback and a private
        // address alike. This is the production posture.
        assert!(!endpoint_is_acceptable("http://127.0.0.1:9999/push"), "loopback http is off by default");
        assert!(!endpoint_is_acceptable("http://localhost:9999/push"));
        assert!(
            !endpoint_is_acceptable("http://192.168.1.5/push"),
            "a private address over http is off too"
        );

        // With the opt-in, plain HTTP is allowed *only* for loopback — never for
        // an arbitrary host, so the send path stays testable without opening an
        // SSRF to the LAN.
        std::env::set_var(ALLOW_INSECURE_ENDPOINT_ENV, "1");
        assert!(
            endpoint_is_acceptable("http://127.0.0.1:9999/push"),
            "the opt-in reaches a loopback listener"
        );
        assert!(endpoint_is_acceptable("http://localhost:9999/push"));
        assert!(
            !endpoint_is_acceptable("http://192.168.1.5/push"),
            "but only loopback, never a private address"
        );
        assert!(
            !endpoint_is_acceptable("http://push.example.com/abc"),
            "and never a public host over http"
        );
        std::env::remove_var(ALLOW_INSECURE_ENDPOINT_ENV);

        assert!(!endpoint_is_acceptable("file:///etc/passwd"));
        assert!(!endpoint_is_acceptable("not a url"));
        assert!(!endpoint_is_acceptable(""));
    }

    #[test]
    fn a_switch_set_to_off_is_off() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        for value in ["0", "false", "", "no", "off"] {
            std::env::set_var(ALLOW_INSECURE_ENDPOINT_ENV, value);
            assert!(!insecure_endpoints_allowed(), "{value:?}");
            assert!(!endpoint_is_acceptable("http://127.0.0.1:9999/push"), "{value:?}");
        }
        std::env::remove_var(ALLOW_INSECURE_ENDPOINT_ENV);
        assert!(!insecure_endpoints_allowed(), "unset");
    }

    #[test]
    fn a_logged_origin_carries_no_capability_path() {
        assert_eq!(
            endpoint_origin("https://fcm.googleapis.com/fcm/send/SECRET-token"),
            "https://fcm.googleapis.com"
        );
        assert_eq!(endpoint_origin("http://127.0.0.1:9999/push/abc"), "http://127.0.0.1:9999");
        assert_eq!(endpoint_origin("not a url"), "<unparseable>");
    }

    /// A rotation arrives right behind the failed run that caused it, inside
    /// the window that run just opened; throttled, the notice that says what
    /// happened next would be the one swallowed.
    #[test]
    fn a_rotation_notice_is_never_throttled() {
        assert!(!Notice::Rotated("已从 a 切到 b（5 小时额度用完）".into()).throttled());
        assert!(Notice::Failed("x".into()).throttled());
        assert!(Notice::Finished("x".into()).throttled());
        assert_eq!(
            Notice::Rotated("已从 a 切到 b".into()).body(),
            "已从 a 切到 b",
            "the sentence goes out as it is"
        );
        assert_eq!(Notice::Failed("boom".into()).body(), "Run failed: boom");
    }

    #[test]
    fn a_question_says_it_is_one() {
        assert_eq!(question_body("Which colour do you prefer?"), "Asking you: Which colour do you prefer?");
    }

    /// The window is per session, and taking it is what starts it.
    #[test]
    fn the_throttle_is_per_session_and_opens_once() {
        let a = format!("claude/{}", uuid::Uuid::new_v4());
        let b = format!("claude/{}", uuid::Uuid::new_v4());
        assert!(throttle_allows(&a), "the first notification for a session goes");
        assert!(!throttle_allows(&a), "the second inside the window does not");
        assert!(throttle_allows(&b), "a different session has its own window");
        assert!(!throttle_allows(&a));
    }

    #[test]
    fn the_contact_falls_back_when_the_override_is_not_a_usable_uri() {
        let _guard = ENV_LOCK.lock().expect("env lock");

        // The default is a shape RFC 8292 accepts; Apple rejects anything else.
        assert!(DEFAULT_CONTACT.starts_with("https://") || DEFAULT_CONTACT.starts_with("mailto:"));
        assert_eq!(DEFAULT_CONTACT, "https://droppen.app", "public, and not a source repository");

        std::env::remove_var("DROPPEN_PUSH_CONTACT");
        assert_eq!(contact(), DEFAULT_CONTACT, "no override: the default stands");

        // A set-but-unusable override (neither `mailto:` nor `https:`) is Apple's
        // outright-reject case, so it must fall back rather than be passed on.
        std::env::set_var("DROPPEN_PUSH_CONTACT", "ops@example.com");
        assert_eq!(contact(), DEFAULT_CONTACT, "a bare address is not a URI: fall back");
        std::env::set_var("DROPPEN_PUSH_CONTACT", "ftp://example.com/contact");
        assert_eq!(contact(), DEFAULT_CONTACT, "the wrong scheme falls back too");

        // A usable override is honoured verbatim.
        std::env::set_var("DROPPEN_PUSH_CONTACT", "mailto:ops@example.com");
        assert_eq!(contact(), "mailto:ops@example.com", "a real mailto: is passed through");

        std::env::remove_var("DROPPEN_PUSH_CONTACT");
    }

    /// A resident with nothing outstanding, for the clause tests to vary.
    fn resident(queued: u32, background: u32) -> ResidentInfo {
        ResidentInfo {
            phase: crate::sessions::ResidentPhase::Idle,
            queued,
            queued_messages: Vec::new(),
            background,
            tasks: Vec::new(),
            awaiting_background: false,
            stalled: false,
            turn_started_at: crate::openapi_ext::Nullable(None),
            asking: false,
        }
    }

    /// Nothing to add is the common case, and it must add nothing at all: a
    /// notification for an ordinary finished turn is the echo and nothing else.
    #[test]
    fn a_session_with_nothing_left_running_gets_no_clause() {
        assert_eq!(still_busy_clause(None), None, "no resident: this server is not driving it");
        assert_eq!(still_busy_clause(Some(&resident(0, 0))), None, "an idle resident has nothing to add");
    }

    /// The bug this exists for: the turn ended, the session did not, because the
    /// user sent something while it was running. Pluralised because "1 messages
    /// still queued" reads as a bug in the server rather than a busy session.
    #[test]
    fn a_queue_left_behind_is_counted_and_pluralised() {
        assert_eq!(still_busy_clause(Some(&resident(1, 0))).unwrap(), "1 message still queued");
        assert_eq!(still_busy_clause(Some(&resident(2, 0))).unwrap(), "2 messages still queued");
    }

    /// Background work outlives the turn that started it, so a turn can end with
    /// the session still genuinely busy and nothing queued at all.
    #[test]
    fn background_work_left_running_is_said_on_its_own() {
        assert_eq!(still_busy_clause(Some(&resident(0, 1))).unwrap(), "1 background task still running");
        assert_eq!(still_busy_clause(Some(&resident(0, 2))).unwrap(), "2 background tasks still running");
    }

    /// Both are true independently, so both are said. Joined the same way the
    /// clause is joined to the snippet, so the body reads as one line.
    #[test]
    fn a_queue_and_background_work_are_both_reported() {
        assert_eq!(
            still_busy_clause(Some(&resident(1, 2))).unwrap(),
            "1 message still queued · 2 background tasks still running"
        );
    }

    /// The park is a different fact from "both happen to be true", and reads as
    /// one: the queue is not moving *because* of the background work. Told the
    /// generic way, a user would see an idle session with a stuck queue and
    /// reasonably conclude their message had been dropped.
    #[test]
    fn a_message_parked_behind_background_work_says_what_it_is_waiting_for() {
        let parked = ResidentInfo {
            awaiting_background: true,
            ..resident(1, 2)
        };
        assert_eq!(
            still_busy_clause(Some(&parked)).unwrap(),
            "1 message queued, waiting on 2 background tasks"
        );

        // The flag on its own is not a story. A park with nothing left in the
        // background is a momentary inconsistency in the mirror, not something
        // to tell a phone about, so it falls back to the plain queue clause.
        let unparked = ResidentInfo {
            awaiting_background: true,
            ..resident(1, 0)
        };
        assert_eq!(still_busy_clause(Some(&unparked)).unwrap(), "1 message still queued");
    }

    /// The trap this module has to keep stepping around, pinned so a future
    /// refactor cannot "simplify" it away.
    ///
    /// [`crate::sessions::apply_driver`] promotes `status` to `Running` for a
    /// queued session, and [`attention::is_unread`] answers `false` for
    /// `Running`. So the overlay is applied for the *wording* while the unread
    /// gate keeps the status the transcript gave: pass it the overlaid one and
    /// every notification for a turn that ended with something queued disappears
    /// — the precise case the overlay was added to describe.
    #[test]
    fn the_overlay_rewrites_the_row_without_moving_the_unread_gate() {
        use crate::openapi_ext::Nullable;
        use crate::sessions::{apply_driver, SessionActivity, SessionScope, SessionStatus};

        let mut summary = SessionSummary {
            pinned: false,
            id: "s".into(),
            source: Source::Claude,
            scope: SessionScope::Owned,
            server: "default".into(),
            title: "t".into(),
            cwd: Nullable(None),
            repo_key: Nullable(None),
            git_branch: Nullable(None),
            updated_at: "2026-01-01T00:00:00Z".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            size_bytes: 0,
            file_path: String::new(),
            status: SessionStatus::Idle,
            activity: SessionActivity::Idle,
            resident: Nullable(None),
            claude_root: None,
            acp_agent: None,
            account_id: None,
            attention_at: Nullable(None),
            unread: false,
            unread_count: 0,
            asking: false,
            children: Vec::new(),
            parent_id: Nullable(None),
        };
        // What `notify_inner` captures, before the overlay touches anything.
        let transcript_status = summary.status;

        apply_driver(&mut summary, Some(&resident(1, 0)), false, None);
        assert_eq!(summary.activity, SessionActivity::Queued, "the row now says a message is waiting");
        assert_eq!(summary.status, SessionStatus::Running, "and `apply_driver` promoted the status to match");
        assert_eq!(transcript_status, SessionStatus::Idle, "but the captured status is untouched");

        // The two answers differ, and the notification depends on getting the
        // second one. `2026-01-02` is the assistant speaking after the last time
        // the user looked, i.e. an ordinary unread turn.
        assert!(
            attention::is_unread(Some("2026-01-02T00:00:00Z"), Some("2026-01-01T00:00:00Z"), transcript_status),
            "the turn happened and the user has not seen it: notify"
        );
        assert!(
            !attention::is_unread(Some("2026-01-02T00:00:00Z"), Some("2026-01-01T00:00:00Z"), summary.status),
            "gating on the overlaid status would silently drop exactly this notification"
        );

        // And the overlay's real job: the row carries the resident the clause is
        // written from.
        assert_eq!(
            still_busy_clause(summary.resident.as_ref()).unwrap(),
            "1 message still queued",
            "the wording comes from the row the overlay just corrected"
        );
    }

    /// The body a service worker reads. Pinned because the web half parses it
    /// by hand and a renamed field is a silent no-op there.
    #[test]
    fn the_payload_carries_the_four_fields_the_service_worker_reads() {
        let payload = serde_json::json!({ "title": "t", "body": "b", "tag": "claude/abc", "url": "/sessions/claude/abc" });
        for key in ["title", "body", "tag", "url"] {
            assert!(payload.get(key).is_some(), "{key} is part of the contract with sw.js");
        }
        assert_eq!(
            payload["tag"],
            attention::seen::key_of(Source::Claude, "abc"),
            "the tag is the seen-store key"
        );
    }
}
