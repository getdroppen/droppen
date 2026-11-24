//! Push delivery (`docs/a2a.md` §5, §6): every status change of a task (not
//! a working task's progress messages) and every artifact, POSTed to each
//! webhook the task's caller set, as the `StreamResponse` a stream carries.
//!
//! # In order, at least once, across a restart
//!
//! Each configuration has a cursor over its task's kept events (`delivered_seq`,
//! [`Store::push_cursor`]). One worker per configuration reads the events
//! after it, in order, and moves it past each once the webhook answered
//! `2xx`, or once it gave up on it: tried at once, then again at once, after
//! 10 s and after 60 s ([`RETRIES`]), then logged and passed over. A webhook
//! that is down holds up its own configuration only. After a restart
//! ([`TaskEventSink::resume`]) the workers start again where the cursors
//! stand; an event that was on its way when the server stopped goes again.
//!
//! # Asked again before every attempt
//!
//! The configuration must still be there, the task's caller must still reach
//! its entry ([`A2a::caller_reaches`]), and the URL must still pass
//! [`guard::push_target`] against the entry as it stands. A revoked caller is
//! told nothing more; a deleted configuration gets nothing more.
//!
//! # A client of its own
//!
//! [`guard`] says where a push may go; the client holds it there: the guarded
//! resolver, no redirects, no proxy (`HTTPS_PROXY` would resolve the name
//! elsewhere, unchecked), 15 seconds for the whole exchange, and no more than
//! [`BODY_CAP`] of the answer read. `droppen_base::outbound` is not that
//! client and is not made into it: Web Push and Bark go through it, and a
//! Bark server is often one the user runs on their own network, which these
//! rules would cut off.

pub mod guard;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use futures_util::future::BoxFuture;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::engine::{A2a, EventKind, TaskEvent, TaskEventSink};
use crate::store::{EventRow, PushRow, Store};
use guard::{push_target, Guarded, Lookup, PushTarget, SystemLookup};

/// How long one attempt may take, connection to the last byte read (§6).
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// The pauses before each retry of a failed attempt (§5): three retries, the
/// first at once.
pub const RETRIES: [Duration; 3] = [Duration::ZERO, Duration::from_secs(10), Duration::from_secs(60)];
/// The most of a webhook's answer read; the rest is dropped unread.
pub const BODY_CAP: usize = 64 * 1024;
/// The media type of a push's body.
const MEDIA_TYPE: &str = "application/a2a+json";

/// A pause, as the sender takes one: the runtime's timer, or a test's.
pub(crate) type Sleep = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// How the sender works; [`Default`] is how the server runs it.
pub(crate) struct PushSettings {
    /// [`TIMEOUT`].
    pub(crate) timeout: Duration,
    /// [`RETRIES`].
    pub(crate) retries: Vec<Duration>,
    /// How a host name is resolved before the guard sees it.
    pub(crate) lookup: Arc<dyn Lookup>,
    /// Roots trusted besides the built-in ones: a test's.
    pub(crate) roots: Vec<reqwest::Certificate>,
    /// How a pause is taken.
    pub(crate) sleep: Sleep,
}

impl Default for PushSettings {
    fn default() -> Self {
        Self {
            timeout: TIMEOUT,
            retries: RETRIES.to_vec(),
            lookup: Arc::new(SystemLookup),
            roots: Vec::new(),
            sleep: Arc::new(|pause| Box::pin(tokio::time::sleep(pause))),
        }
    }
}

/// A configuration: its task, and its id.
type Key = (String, String);

/// What a worker is asked to look at.
enum Wake {
    /// The configurations of a task.
    Task(String),
    /// Every configuration with events it has not been told of.
    All,
}

/// How one event's delivery to one configuration ended.
enum Outcome {
    /// The webhook took it.
    Delivered,
    /// Every attempt failed, or the URL is refused: passed over.
    GaveUp,
    /// The configuration is gone, its caller no longer reaches the entry, or
    /// the server stops: nothing more goes to it, and its cursor stays.
    Stop,
}

/// What [`Pusher::check`] found before an attempt.
enum Check {
    /// Send to the target, as the configuration says.
    Go(Box<(PushRow, PushTarget)>),
    /// The URL is refused.
    Refused(guard::Refusal),
    /// Stop: see [`Outcome::Stop`].
    Stop,
}

/// The push sender: the server's [`TaskEventSink`].
pub struct Pusher {
    a2a: Weak<A2a>,
    store: Store,
    settings: PushSettings,
    user_agent: String,
    cancel: CancellationToken,
    wake: mpsc::UnboundedSender<Wake>,
    /// Wakes not yet looked at, for a test to wait on.
    queued: AtomicUsize,
    /// The running workers, each with whether it is to look again.
    workers: Mutex<HashMap<Key, bool>>,
    /// A client per set of named addresses (and plain http or not): the
    /// resolver is the client's.
    clients: Mutex<HashMap<(BTreeSet<IpAddr>, bool), reqwest::Client>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Pusher {
    /// Deliver `a2a`'s pushes from now on. Delivery of what was left
    /// undelivered when the server last stopped starts at [`A2a::resume`],
    /// which only the instance's owner calls.
    pub fn attach(a2a: &Arc<A2a>) -> Arc<Pusher> {
        Self::attach_with(a2a, PushSettings::default())
    }

    pub(crate) fn attach_with(a2a: &Arc<A2a>, settings: PushSettings) -> Arc<Pusher> {
        let (wake, mut woken) = mpsc::unbounded_channel();
        let cancel = a2a.cancelled().clone();
        let pusher = Arc::new(Pusher {
            a2a: Arc::downgrade(a2a),
            store: a2a.store().clone(),
            settings,
            user_agent: format!("droppen/{}", a2a.settings().version),
            cancel: cancel.clone(),
            wake,
            queued: AtomicUsize::new(0),
            workers: Mutex::new(HashMap::new()),
            clients: Mutex::new(HashMap::new()),
        });
        let weak = Arc::downgrade(&pusher);
        tokio::spawn(async move {
            loop {
                let wake = tokio::select! {
                    () = cancel.cancelled() => return,
                    wake = woken.recv() => wake,
                };
                let (Some(wake), Some(pusher)) = (wake, weak.upgrade()) else { return };
                pusher.dispatch(wake);
                pusher.queued.fetch_sub(1, Ordering::SeqCst);
            }
        });
        a2a.set_event_sink(pusher.clone());
        pusher
    }

    fn send(&self, wake: Wake) {
        self.queued.fetch_add(1, Ordering::SeqCst);
        if self.wake.send(wake).is_err() {
            self.queued.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Start a worker for each configuration `wake` names, or tell a running
    /// one to look again.
    fn dispatch(self: &Arc<Self>, wake: Wake) {
        let keys: Vec<Key> = match wake {
            Wake::Task(task_id) => self.store.pushes(&task_id).into_iter().map(|push| (push.task_id, push.id)).collect(),
            Wake::All => self.store.pending_pushes(),
        };
        for key in keys {
            let start = {
                let mut workers = locked(&self.workers);
                match workers.get_mut(&key) {
                    Some(again) => {
                        *again = true;
                        false
                    }
                    None => {
                        workers.insert(key.clone(), false);
                        true
                    }
                }
            };
            if start {
                tokio::spawn(self.clone().run(key));
            }
        }
    }

    /// One configuration's worker: delivers until nothing is left, and again
    /// while it was woken meanwhile.
    async fn run(self: Arc<Self>, key: Key) {
        loop {
            self.drain(&key).await;
            let again = {
                let mut workers = locked(&self.workers);
                let again = workers.get(&key).copied().unwrap_or(false) && !self.cancel.is_cancelled();
                if again {
                    workers.insert(key.clone(), false);
                } else {
                    workers.remove(&key);
                }
                again
            };
            if !again {
                return;
            }
        }
    }

    /// Deliver, in order, every event after the configuration's cursor.
    async fn drain(&self, (task_id, config_id): &Key) {
        let Some(cursor) = self.store.push_cursor(task_id, config_id) else {
            return;
        };
        let events = self.store.events_after(task_id, cursor);
        let Some(last) = events.last().map(|event| event.seq) else { return };
        for event in &events {
            if event.kind == EventKind::Progress.as_str() {
                continue;
            }
            match self.deliver(task_id, config_id, event).await {
                Outcome::Delivered | Outcome::GaveUp => self.advance(task_id, config_id, event.seq),
                Outcome::Stop => return,
            }
        }
        // Past the progress messages after the last one delivered, too.
        self.advance(task_id, config_id, last);
    }

    fn advance(&self, task_id: &str, config_id: &str, seq: i64) {
        if let Err(error) = self.store.advance_push(task_id, config_id, seq) {
            tracing::warn!(task = %task_id, config = %config_id, %error, "cannot write where a push's delivery stands");
        }
    }

    /// One event to one configuration: the first attempt and its retries.
    async fn deliver(&self, task_id: &str, config_id: &str, event: &EventRow) -> Outcome {
        let attempts = self.settings.retries.len() + 1;
        for attempt in 0..attempts {
            if attempt > 0 {
                let pause = (self.settings.sleep)(self.settings.retries[attempt - 1]);
                tokio::select! {
                    () = self.cancel.cancelled() => return Outcome::Stop,
                    () = pause => {}
                }
            }
            let (config, target) = match self.check(task_id, config_id) {
                Check::Go(go) => *go,
                Check::Stop => return Outcome::Stop,
                Check::Refused(refusal) => {
                    tracing::warn!(task = %task_id, config = %config_id, seq = event.seq, %refusal, "a push is refused; passing over the event");
                    return Outcome::GaveUp;
                }
            };
            let sent = tokio::select! {
                () = self.cancel.cancelled() => return Outcome::Stop,
                sent = self.post(&target, &config, &event.payload) => sent,
            };
            match sent {
                Ok(()) => return Outcome::Delivered,
                Err(error) if attempt + 1 < attempts => {
                    tracing::debug!(task = %task_id, config = %config_id, seq = event.seq, origin = %target.origin(), attempt = attempt + 1, %error, "a push failed; it is tried again");
                }
                Err(error) => {
                    tracing::warn!(task = %task_id, config = %config_id, seq = event.seq, origin = %target.origin(), attempts, %error, "a push failed every attempt; passing over the event");
                }
            }
        }
        Outcome::GaveUp
    }

    /// What has to hold before an attempt (§6).
    fn check(&self, task_id: &str, config_id: &str) -> Check {
        if self.cancel.is_cancelled() {
            return Check::Stop;
        }
        let Some(a2a) = self.a2a.upgrade() else { return Check::Stop };
        let Some(config) = self.store.push(task_id, config_id) else {
            return Check::Stop;
        };
        let Some(task) = self.store.task(task_id) else { return Check::Stop };
        if !a2a.caller_reaches(&task.caller, &task.entry) {
            tracing::info!(task = %task_id, config = %config_id, "the task's caller no longer reaches its entry; no more pushes");
            return Check::Stop;
        }
        let Some(entry) = self.store.entry(&task.entry) else {
            return Check::Stop;
        };
        match push_target(&config.url, &entry.push_origins, entry.source, a2a.settings().tck) {
            Ok(target) => Check::Go(Box::new((config, target))),
            Err(refusal) => Check::Refused(refusal),
        }
    }

    /// The client for a target: one per set of addresses its resolver lets
    /// through besides the public ones.
    fn client(&self, target: &PushTarget) -> Result<reqwest::Client, String> {
        let key = (target.named.clone(), target.plain_http);
        let cached = locked(&self.clients).get(&key).cloned();
        if let Some(client) = cached {
            return Ok(client);
        }
        let mut builder = reqwest::Client::builder()
            .user_agent(self.user_agent.clone())
            .timeout(self.settings.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .https_only(!target.plain_http)
            .dns_resolver(Arc::new(Guarded::new(self.settings.lookup.clone(), target.named.clone())));
        for root in &self.settings.roots {
            builder = builder.add_root_certificate(root.clone());
        }
        let client = builder.build().map_err(|error| format!("cannot build the push client: {error}"))?;
        locked(&self.clients).insert(key, client.clone());
        Ok(client)
    }

    /// One attempt: `Ok` when the webhook answered `2xx`.
    async fn post(&self, target: &PushTarget, config: &PushRow, body: &str) -> Result<(), String> {
        let client = self.client(target)?;
        let response = client
            .post(target.url.clone())
            .headers(headers(config)?)
            .body(body.to_owned())
            .send()
            .await
            .map_err(|error| error_chain(&error.without_url()))?;
        let status = response.status();
        let mut response = response;
        let mut read = 0;
        while read < BODY_CAP {
            match response.chunk().await {
                Ok(Some(chunk)) => read += chunk.len(),
                _ => break,
            }
        }
        if status.is_success() {
            Ok(())
        } else {
            Err(format!("the webhook answered {status}"))
        }
    }

    /// Whether no wake is waiting and no worker running.
    #[cfg(test)]
    pub(crate) fn idle(&self) -> bool {
        self.queued.load(Ordering::SeqCst) == 0 && locked(&self.workers).is_empty()
    }

    /// Wait until [`Pusher::idle`]; fails the test after ten seconds.
    #[cfg(test)]
    pub(crate) async fn settled(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !self.idle() {
            assert!(tokio::time::Instant::now() < deadline, "the pushes never settled");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

/// A push's headers (§5): the media type, `Authorization` from the
/// configuration's authentication, and its token under both names clients
/// read. The secrets are marked sensitive, so nothing prints them.
fn headers(config: &PushRow) -> Result<HeaderMap, String> {
    let secret = |value: &str, what: &str| {
        HeaderValue::from_str(value)
            .map(|mut value| {
                value.set_sensitive(true);
                value
            })
            .map_err(|_| format!("the push configuration's {what} is not a header value"))
    };
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(MEDIA_TYPE));
    if let Some(scheme) = config.auth_scheme.as_deref().map(str::trim).filter(|scheme| !scheme.is_empty()) {
        let value = match config.auth_credentials.as_deref().filter(|credentials| !credentials.is_empty()) {
            Some(credentials) => format!("{scheme} {credentials}"),
            None => scheme.to_string(),
        };
        headers.insert(AUTHORIZATION, secret(&value, "authentication")?);
    }
    if let Some(token) = config.token.as_deref().filter(|token| !token.is_empty()) {
        let token = secret(token, "token")?;
        headers.insert(HeaderName::from_static("a2a-notification-token"), token.clone());
        headers.insert(HeaderName::from_static("x-a2a-notification-token"), token);
    }
    Ok(headers)
}

/// An error and its causes, one line: a refused address shows up as the
/// resolver's error under the connector's.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

impl TaskEventSink for Pusher {
    fn task_event(&self, event: &TaskEvent) {
        if event.kind != EventKind::Progress {
            self.send(Wake::Task(event.task_id.clone()));
        }
    }

    fn push_added(&self, task_id: &str) {
        self.send(Wake::Task(task_id.to_string()));
    }

    fn resume(&self) {
        self.send(Wake::All);
    }
}
