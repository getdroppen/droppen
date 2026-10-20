//! Following the other devices' copies (local-aggregator §6.3, S1-5): this
//! computer, as a device of its account, keeps a current copy of every
//! host's index and status and of each one's newest sessions — readable
//! here while that host is off — in [`super::store`] (`copies.db`).
//!
//! It runs inside the hub link, beside [`super::controller`], and stops with
//! it. The controller hands it the keys it holds ([`KeyRing`], in memory
//! only); which streams it follows is [`super::access`]'s.
//!
//! - **One connection**: its own device session (minted with the machine
//!   key, as the controller's) and a `/v1/client` socket, signed in as a
//!   device. The link pings every 20 s; a minute without a word from the hub
//!   is a dead connection. Reconnects back off from 0.5 s to 30 s, with
//!   jitter. The hub refusing the session (401, `denied`) mints a new one;
//!   `device_removed` deletes `aggregator/` (as the controller does) and
//!   tries again only an hour later; `host_devices_off` ten minutes later.
//! - **Each stream** is a task of its own: `SUB` from its cursor — the last
//!   machine-signed head released, kept with the folded copy — through a
//!   [`Follower`]. Nothing a signed head does not cover is folded in. A
//!   record that does not open, a head that does not match, a session body
//!   that does not follow: that stream is read again from the start, with
//!   backoff; the copy it had stays meanwhile.
//! - **Pushes** on the same socket: `mail` wakes the controller (its own
//!   polling is the fallback); `presence` says which hosts are online.
//! - **Removal and narrowing purge**: a host the chain no longer lets this
//!   computer control, or whose keys it no longer holds, loses its rows at
//!   once; the aggregator turned off, the computer disconnected or removed
//!   deletes `aggregator/` — the store refuses every write after that, and
//!   this task drops what it holds.
//!
//! Nothing it logs carries content or keys: host ids, kinds, sequence
//! numbers and error kinds only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use rand::Rng;
use rh_client::follow::{check_page, Floor, FollowError, Follower, Step};
use rh_client::http::HttpError;
use rh_client::link::{ConnectError, Link};
use rh_client::streams::{json_body, FeedCopy, IndexCopy, Pages};
use rh_client::HubUrl;
use rh_proto::frame::Frame;
use rh_proto::identity::MachineIdentity;
use rh_proto::record::{SignedHead, StreamId, FMT_FEED_V1, FMT_INDEX_V1, FMT_STATUS_V1};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, Notify};
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

use super::access::{self, Want};
use super::controller::{view, View};
use super::live::{Key, LiveCopies, Published, Slot};
use super::store::{CopyStore, Kind, Row};
use crate::agent::state::AgentState;
use crate::sessions::Paths;

/// A session copy larger than this (its folded JSON) is not kept: its row
/// says so, and it is not followed again until the next run.
pub const MAX_SESSION_BYTES: usize = 8 * 1024 * 1024;
/// The hub is taken for gone after this long without a word.
const SILENT_FOR: Duration = Duration::from_secs(60);
/// Reconnecting backs off between these.
const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A connection that lasted this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(30);
/// `host_devices_off`: asked again this much later.
const REFUSED_EVERY: Duration = Duration::from_secs(10 * 60);
/// `device_removed`: asked again only this much later, quietly.
const REMOVED_EVERY: Duration = Duration::from_secs(60 * 60);
/// With nothing to follow (or not allowed to), look again this often.
const IDLE_EVERY: Duration = Duration::from_secs(5);
/// Copies are written at most this often (each stream's newest state).
const FLUSH_EVERY: Duration = Duration::from_millis(200);
/// A stream read again after an error waits between these.
const REBUILD_MIN: Duration = Duration::from_secs(2);
const REBUILD_MAX: Duration = Duration::from_secs(5 * 60);
/// A session copy is written at most this often (its first write at once):
/// a busy session would otherwise rewrite its whole copy with every turn.
const SESSION_WRITE_EVERY: Duration = Duration::from_secs(10);
/// A head over nothing new (a heartbeat) refreshes how current a copy is
/// known to be at most this often.
const FRESH_EVERY: Duration = Duration::from_secs(60);
/// Mint a new device session this long before the one held expires.
const RENEW_BEFORE_MS: u64 = 5 * 60 * 1000;

/// Machine-key epochs and keys, zeroed when dropped.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct Keys(pub Vec<(u32, [u8; 32])>);

impl Drop for Keys {
    fn drop(&mut self) {
        for (_, key) in self.0.iter_mut() {
            key.zeroize();
        }
    }
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter().map(|(e, _)| e)).finish()
    }
}

/// The other hosts' machine keys the controller holds, as it hands them over
/// (in memory only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRing {
    /// The account they are of.
    pub account: String,
    /// The device (this computer) they were sealed to.
    pub device: String,
    /// Host → its epochs held.
    pub hosts: BTreeMap<String, Keys>,
}

/// What `GET /api/cloud` shows of one host's copy: counts and times, never
/// content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCopy {
    /// Sessions in its index, as copied.
    pub index_rows: Option<u64>,
    /// When its agent last listed them (ms).
    pub index_as_of_ms: Option<u64>,
    /// When its agent last signed its status (ms).
    pub status_at_ms: Option<u64>,
    /// Sessions copied here.
    pub sessions: u64,
    /// Whether it is online now (`None`: not known).
    pub online: Option<bool>,
}

/// What the copies task reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Its connection is up.
    pub connected: bool,
    /// Every host followed.
    pub hosts: BTreeMap<String, HostCopy>,
    /// Why the last connection failed or ended.
    pub error: Option<String>,
}

/// What it needs from the link it runs in.
pub struct Env {
    /// The instance's paths.
    pub paths: Paths,
    /// The machine key: this device's one identity.
    pub identity: Arc<MachineIdentity>,
    /// The link's agent state, with its verified chain.
    pub state: Arc<Mutex<AgentState>>,
    /// Bumped whenever the chain advanced.
    pub chain: watch::Receiver<u64>,
    /// The keys, from the controller.
    pub keys: watch::Receiver<Option<Arc<KeyRing>>>,
    /// Wakes the controller (mail for this device).
    pub nudge: Arc<Notify>,
    /// The link's token (a child of it).
    pub cancel: CancellationToken,
    /// Where it reports to.
    pub status: Arc<Mutex<Status>>,
    /// Where the copies are published for the local gateway (S1-7).
    pub live: Arc<LiveCopies>,
    /// Asks the link to sync the chain from the hub once: a stream that
    /// stops being readable may be one whose host renewed its identity key
    /// before this computer's chain heard (`docs/host-identity-rotation.md`
    /// §5.4) — the chain moving makes it read again under the new key.
    pub resync: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Whether the device role may sign in now ([`super::SignInGate`]);
    /// `None`: always.
    pub may_sign_in: Option<super::SignInGate>,
}

/// A host followed now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Host {
    /// Its id.
    pub id: String,
    /// Its Ed25519 key now (what signs its heads): the only key anything from
    /// the network is taken under.
    pub machine_key: [u8; 32],
    /// The Ed25519 keys it renewed away from (`RotateHostKey`), oldest first:
    /// a position stored here, verified when it was stored, may carry one
    /// (`docs/host-identity-rotation.md` §5.4).
    pub retired: Vec<[u8; 32]>,
    /// Its epochs held.
    pub keys: Keys,
}

/// The hosts this computer may follow: accepting its control (D2), with
/// keys held — never itself.
pub(super) fn followed(view: &View, ring: &KeyRing) -> Vec<Host> {
    view.chain
        .hosts_admitting(&view.me)
        .filter(|h| h.info.id != view.me)
        .filter_map(|h| {
            let keys = ring.hosts.get(&h.info.id).filter(|k| !k.0.is_empty())?;
            Some(Host {
                id: h.info.id.clone(),
                machine_key: h.info.sign_key,
                retired: h.retired.iter().map(|r| r.sign_key).collect(),
                keys: keys.clone(),
            })
        })
        .collect()
}

/// A copy being folded (also what the local gateway serves: [`super::live`]).
pub(super) enum Fold {
    Index(IndexCopy),
    Status(Value),
    /// A session, and the size (bytes of JSON) of each of its items: it is
    /// counted as it is folded, and folding stops past [`MAX_SESSION_BYTES`].
    Feed(FeedCopy, Vec<usize>),
}

/// Why a body was not folded in.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum FoldError {
    /// The session copy passed [`MAX_SESSION_BYTES`].
    TooLarge,
    /// The body is not what the stream holds.
    Bad(String),
}

/// The bytes an item takes in the copy (its JSON and a separator).
fn item_size(item: &Value) -> usize {
    serde_json::to_vec(item).map_or(0, |v| v.len()) + 1
}

impl Fold {
    pub(super) fn empty(kind: Kind) -> Self {
        match kind {
            Kind::Index => Fold::Index(IndexCopy::default()),
            Kind::Status => Fold::Status(Value::Null),
            Kind::Session => Fold::Feed(FeedCopy::default(), Vec::new()),
        }
    }

    pub(super) fn restore(kind: Kind, state: &str) -> Option<Self> {
        Some(match kind {
            Kind::Index => Fold::Index(serde_json::from_str(state).ok()?),
            Kind::Status => Fold::Status(serde_json::from_str(state).ok()?),
            Kind::Session => {
                let copy: FeedCopy = serde_json::from_str(state).ok()?;
                let sizes = copy.items.iter().map(item_size).collect();
                Fold::Feed(copy, sizes)
            }
        })
    }

    pub(super) fn apply(&mut self, body: &(u16, Vec<u8>)) -> Result<(), FoldError> {
        match self {
            Fold::Index(copy) => copy.apply(&json_body(FMT_INDEX_V1, body).map_err(FoldError::Bad)?),
            Fold::Status(copy) => *copy = json_body(FMT_STATUS_V1, body).map_err(FoldError::Bad)?,
            Fold::Feed(copy, sizes) => {
                let body = json_body(FMT_FEED_V1, body).map_err(FoldError::Bad)?;
                copy.apply(&body).map_err(FoldError::Bad)?;
                // Replace-from: the items kept are a prefix, the body's come after them.
                let added = body["items"].as_array().map_or(0, Vec::len);
                let kept = copy.items.len().saturating_sub(added);
                if sizes.len() < kept {
                    sizes.clear();
                    sizes.extend(copy.items.iter().map(item_size));
                } else {
                    sizes.truncate(kept);
                    sizes.extend(copy.items[kept..].iter().map(item_size));
                }
                if sizes.iter().sum::<usize>() > MAX_SESSION_BYTES {
                    return Err(FoldError::TooLarge);
                }
            }
        }
        Ok(())
    }

    fn to_json(&self) -> String {
        match self {
            Fold::Index(copy) => serde_json::to_string(copy),
            Fold::Status(copy) => serde_json::to_string(copy),
            Fold::Feed(copy, _) => serde_json::to_string(copy),
        }
        .unwrap_or_default()
    }

    /// The bodies that fold, from nothing, into this copy, as a JSON array's
    /// text (the web's folds take them as the agent's own): the gateway's
    /// snapshot (S1-7). A status is its last body; nothing for none yet.
    pub(super) fn snapshot_bodies(&self) -> String {
        use rh_client::streams::Snapshot;
        match self {
            Fold::Index(copy) => copy.snapshot_bodies(),
            Fold::Status(Value::Null) => "[]".into(),
            Fold::Status(copy) => format!("[{copy}]"),
            Fold::Feed(copy, _) => copy.snapshot_bodies(),
        }
    }
}

/// A copy as it stands (after the catch-up, then after each head).
struct Copied {
    want: Want,
    head: Option<SignedHead>,
    at_ms: Option<u64>,
    state: String,
    index: Option<IndexCopy>,
}

/// What a stream's task tells the copies task.
enum Update {
    /// The copy as it stands.
    State(Box<Copied>),
    /// The hub has no such stream (yet).
    Missing { want: Want },
    /// It stopped: read it again later.
    Failed { want: Want, error: String, unauthorized: bool },
    /// A session copy over [`MAX_SESSION_BYTES`].
    TooLarge { want: Want },
    /// A heartbeat: the copy is unchanged, and current as of this head.
    Fresh { want: Want, head: SignedHead, at_ms: Option<u64> },
}

/// Where a stream's task fetches pages: the device session, renewed in place.
#[derive(Clone)]
struct PageSource {
    http: reqwest::Client,
    hub: HubUrl,
    token: Arc<RwLock<Zeroizing<String>>>,
}

impl PageSource {
    async fn fetch(&self, stream: &StreamId, hash: &[u8; 32]) -> Result<Vec<u8>, String> {
        let token = Zeroizing::new(self.token.read().expect("token").to_string());
        Pages {
            http: &self.http,
            hub: &self.hub,
            token: &token,
        }
        .fetch(stream, hash)
        .await
    }
}

/// How a stream's task starts: from the start (never below the copy's
/// floor, when it had one), or from a stored cursor.
enum Start {
    Fresh { floor: Option<Floor> },
    Resume { head: SignedHead, state: String },
}

/// One stream to follow, and where to.
struct Task {
    want: Want,
    /// The host's Ed25519 key.
    machine_key: [u8; 32],
    /// The keys it renewed away from: a stored cursor may carry one.
    retired: Vec<[u8; 32]>,
    keys: Keys,
    start: Start,
    link: Arc<Link>,
    pages: PageSource,
    tx: mpsc::UnboundedSender<Update>,
    /// Where the copy is published for the local gateway once caught up.
    live: Arc<LiveCopies>,
}

/// Follow one stream until the link closes, `stop`, or an error.
///
/// The copy is folded into a [`Slot`] of its own, published for the local
/// gateway once the catch-up is over; from then on each released batch is
/// folded and handed to the page under the slot's lock ([`super::live`]).
async fn follow_stream(task: Task, stop: CancellationToken) {
    let Task {
        want,
        machine_key,
        retired,
        keys,
        start,
        link,
        pages,
        tx,
        live,
    } = task;
    let key = Key::new(&want.host, want.kind, &want.name);
    let fresh = |floor| (Follower::new(want.stream, machine_key, &keys.0).with_floor(floor), Fold::empty(want.kind));
    let (mut follower, fold) = match start {
        Start::Fresh { floor } => fresh(floor),
        Start::Resume { head, state } => {
            // Stored here, verified when it was: under any key the host held
            // (`keys_at`) — it may have renewed its identity key since.
            let held = std::iter::once(&machine_key).chain(&retired).any(|k| head.verify(k));
            let floor = held.then(|| Floor::of(&head));
            match (
                Follower::resume_stored(want.stream, machine_key, &retired, &keys.0, head),
                Fold::restore(want.kind, &state),
            ) {
                (Ok(f), Some(fold)) => (f, fold),
                _ => fresh(floor),
            }
        }
    };
    let slot: Slot = Arc::new(Mutex::new(Published::new(fold, follower.from_seq(), 0)));
    // Whether the slot is the one the gateway shows (once caught up).
    let mut published = false;
    let (ch, mut rx) = link.reserve();
    if link
        .send(Frame::Sub {
            ch,
            stream: want.stream,
            from_seq: follower.from_seq(),
        })
        .is_err()
    {
        link.release(ch);
        return;
    }
    let too_large = || {
        live.too_large(&key);
        let _ = tx.send(Update::TooLarge { want: want.clone() });
    };
    let emit = |follower: &Follower| -> Result<(), ()> {
        let (state, index) = {
            let held = slot.lock().expect("slot");
            let index = match &held.fold {
                Fold::Index(copy) => Some(copy.clone()),
                _ => None,
            };
            (held.fold.to_json(), index)
        };
        if want.kind == Kind::Session && state.len() > MAX_SESSION_BYTES {
            too_large();
            return Err(());
        }
        let _ = tx.send(Update::State(Box::new(Copied {
            want: want.clone(),
            head: follower.head().cloned(),
            at_ms: follower.freshest_ms(),
            state,
            index,
        })));
        Ok(())
    };
    let failed = |error: String, unauthorized: bool| {
        let _ = tx.send(Update::Failed {
            want: want.clone(),
            error,
            unauthorized,
        });
    };
    // Where the follower stands, on the slot.
    let stand = |held: &mut Published, follower: &Follower| {
        held.seq = follower.from_seq();
        held.time_ms = follower.freshest_ms().unwrap_or(held.time_ms);
    };
    // A session's copy is serialised and sent at most every
    // SESSION_WRITE_EVERY (the end of the catch-up at once): `dirty` holds
    // what is folded and not sent yet. The gateway is never held back.
    let mut last_emit: Option<tokio::time::Instant> = None;
    let mut dirty = false;
    let mut last_fresh: Option<tokio::time::Instant> = None;
    loop {
        let due = last_emit.map_or_else(tokio::time::Instant::now, |at| at + SESSION_WRITE_EVERY);
        let frame = tokio::select! {
            frame = rx.recv() => match frame {
                Some(frame) => frame,
                None => break,
            },
            () = tokio::time::sleep_until(due), if dirty => {
                dirty = false;
                last_emit = Some(tokio::time::Instant::now());
                if emit(&follower).is_err() {
                    break;
                }
                continue;
            }
            () = stop.cancelled() => break,
        };
        let mut step = match follower.on_frame(frame) {
            Ok(step) => step,
            Err(FollowError::Refused(code)) => {
                failed(format!("refused: {code}"), false);
                break;
            }
            Err(FollowError::Broken(why)) => {
                failed(why, false);
                break;
            }
        };
        if let Step::Checkpoint(manifest) = step {
            // The manifest is the machine's (the follower checked its
            // signature); each page is checked as it arrives, the fetches
            // stop with the task, and a session's stop at its size limit.
            let mut data: Vec<Vec<u8>> = Vec::with_capacity(manifest.pages.len());
            let mut fetched = 0usize;
            let mut outcome = Ok(());
            for (i, hash) in manifest.pages.iter().enumerate() {
                let page = tokio::select! {
                    page = pages.fetch(&want.stream, hash) => page,
                    () = stop.cancelled() => {
                        outcome = Err(None);
                        break;
                    }
                };
                match page
                    .map_err(Some)
                    .and_then(|page| check_page(&manifest, i, &page).map(|()| page).map_err(|e| Some(e.to_string())))
                {
                    Ok(page) => {
                        fetched += page.len();
                        data.push(page);
                    }
                    Err(error) => {
                        outcome = Err(error);
                        break;
                    }
                }
                if want.kind == Kind::Session && fetched > MAX_SESSION_BYTES {
                    outcome = Err(Some(String::new()));
                    break;
                }
            }
            match outcome {
                Ok(()) => {}
                // Stopped.
                Err(None) => break,
                Err(Some(error)) if error.is_empty() => {
                    too_large();
                    break;
                }
                Err(Some(error)) => {
                    let unauthorized = error.contains(" 401");
                    failed(error, unauthorized);
                    break;
                }
            }
            step = match follower.checkpoint(&manifest, &data) {
                Ok(step) => step,
                Err(error) => {
                    failed(error.to_string(), false);
                    break;
                }
            };
        }
        match step {
            Step::Nothing | Step::Checkpoint(_) => {}
            Step::Missing => {
                live.missing(&key);
                let _ = tx.send(Update::Missing { want: want.clone() });
            }
            Step::Synced => {
                dirty = false;
                last_emit = Some(tokio::time::Instant::now());
                if emit(&follower).is_err() {
                    break;
                }
                // Caught up: this is what the page sees from now on.
                stand(&mut slot.lock().expect("slot"), &follower);
                published = live.publish(&key, want.epoch, &slot);
            }
            Step::Released { reset, bodies } => {
                // A heartbeat (a head over nothing new): the copy is unchanged
                // and current as of it — said now and then, never rewritten;
                // the page hears of it at once.
                if bodies.is_empty() && !reset {
                    if follower.synced() {
                        let mut held = slot.lock().expect("slot");
                        stand(&mut held, &follower);
                        if published {
                            live.deliver(&key, &slot, &held, false, &[]);
                        } else {
                            drop(held);
                            // Missing at first and there since: shown from now on.
                            published = live.publish(&key, want.epoch, &slot);
                        }
                    }
                    let quiet = last_fresh.is_none_or(|at| at.elapsed() >= FRESH_EVERY);
                    if follower.synced() && !dirty && quiet {
                        if let Some(head) = follower.head().cloned() {
                            last_fresh = Some(tokio::time::Instant::now());
                            let _ = tx.send(Update::Fresh {
                                want: want.clone(),
                                head,
                                at_ms: follower.freshest_ms(),
                            });
                        }
                    }
                    continue;
                }
                let folded = {
                    let mut held = slot.lock().expect("slot");
                    if reset {
                        held.fold = Fold::empty(want.kind);
                    }
                    let folded = bodies.iter().try_for_each(|b| held.fold.apply(b));
                    match &folded {
                        Ok(()) => {
                            stand(&mut held, &follower);
                            if published && follower.synced() {
                                live.deliver(&key, &slot, &held, reset, &bodies);
                            }
                        }
                        // Half applied: not one to show — taken back while the
                        // slot is still held, so no page subscribes to it meanwhile.
                        Err(FoldError::Bad(_)) if published => live.retract(&key, &slot),
                        Err(_) => {}
                    }
                    folded
                };
                match folded {
                    // Missing at first and created since (no `Synced` on the
                    // way): caught up, it is shown from now on.
                    Ok(()) if !published && follower.synced() => published = live.publish(&key, want.epoch, &slot),
                    Ok(()) => {}
                    Err(FoldError::TooLarge) => {
                        too_large();
                        break;
                    }
                    Err(FoldError::Bad(error)) => {
                        // (Retracted above, under the slot's lock; the next read publishes again.)
                        failed(error, false);
                        break;
                    }
                }
                if !follower.synced() {
                    continue;
                }
                let now = tokio::time::Instant::now();
                let held_back = want.kind == Kind::Session && last_emit.is_some_and(|at| now < at + SESSION_WRITE_EVERY);
                if held_back {
                    dirty = true;
                    continue;
                }
                last_emit = Some(now);
                if emit(&follower).is_err() {
                    break;
                }
            }
        }
    }
    // Stopped by the link or the copies task: what was folded and not sent goes now.
    if dirty && !stop.is_cancelled() {
        let _ = emit(&follower);
    }
    let _ = link.send_control(&json!({"t": "stream_unsub", "ch": ch}));
    link.release(ch);
}

/// A stream being followed.
struct Sub {
    want: Want,
    stop: CancellationToken,
}

/// What is stored of a row (no content).
#[derive(Clone)]
struct Stored {
    stream: StreamId,
    epoch: u32,
    at_ms: Option<u64>,
    too_large: bool,
    /// The last released head's place: the stream read again never goes below it.
    floor: Option<Floor>,
}

/// Why a connection ended.
enum Ended {
    /// Reconnect (after the backoff).
    Again(String),
    /// The session was refused: mint a new one, then reconnect.
    Unauthorized,
    /// The enrollment, the hub or the keys changed under it: start over now.
    Rebind,
    /// Writing was refused (off, stopped, deleted): drop everything.
    Off,
    /// The link's token.
    Cancelled,
}

/// The copies task's state across connections.
struct Copies {
    env: Env,
    store: Option<CopyStore>,
    backoff: Duration,
    /// No connection before this (ms): `device_removed`, `host_devices_off`.
    quiet_until: Option<u64>,
    /// The device session and when (ms) it expires.
    session: Option<(Arc<RwLock<Zeroizing<String>>>, u64)>,
    /// Streams that failed: when they may be read again (ms), and how often they failed.
    failures: HashMap<StreamId, (u64, u32)>,
    /// Sessions over [`MAX_SESSION_BYTES`]: not followed again this run.
    too_large: HashSet<StreamId>,
    http: Option<reqwest::Client>,
    /// The sessions pages opened through the gateway changed.
    interest: watch::Receiver<u64>,
}

fn now_ms() -> u64 {
    crate::agent::now_ms()
}

fn jitter(d: Duration) -> Duration {
    let ms = d.as_millis() as u64;
    Duration::from_millis(ms / 2 + rand::thread_rng().gen_range(0..=ms / 2))
}

/// Run the copies task until `env.cancel`.
pub async fn run(env: Env) {
    let env_interest = env.live.interest();
    let mut c = Copies {
        env,
        store: None,
        backoff: BACKOFF_MIN,
        quiet_until: None,
        session: None,
        failures: HashMap::new(),
        too_large: HashSet::new(),
        http: None,
        interest: env_interest,
    };
    let cancel = c.env.cancel.clone();
    loop {
        if cancel.is_cancelled() {
            break;
        }
        // A round may wait on the hub (signing in, connecting): the link's
        // token ends it there too.
        let wait = tokio::select! {
            wait = c.round() => wait,
            () = cancel.cancelled() => break,
        };
        if c.env.cancel.is_cancelled() {
            break;
        }
        if wait.is_zero() {
            continue;
        }
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            changed = c.env.keys.changed() => if changed.is_err() { break },
            changed = c.env.chain.changed() => if changed.is_err() { break },
            () = c.env.cancel.cancelled() => break,
        }
    }
    c.drop_all();
}

impl Copies {
    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.env.status.lock().expect("copies status"));
    }

    /// Drop the store and everything held of it; report nothing; the
    /// gateway shows nothing of the others any more.
    fn drop_all(&mut self) {
        self.store = None;
        self.session = None;
        self.failures.clear();
        self.too_large.clear();
        self.set_status(|s| *s = Status::default());
        self.env.live.clear();
    }

    /// What the store holds, into memory for the gateway (the hub may be
    /// down: the last copies, as current as they are). Once per open.
    fn preload(&self) {
        let Some(store) = self.store.as_ref() else { return };
        let rows = match store.rows() {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, "cannot read the other devices' copies back");
                return;
            }
        };
        let mut held = 0usize;
        for row in rows {
            let key = Key::new(&row.host, row.kind, &row.name);
            if row.too_large {
                self.env.live.too_large(&key);
                continue;
            }
            let (Some(state), Some(head)) = (row.state.as_deref(), row.head.as_ref()) else {
                continue;
            };
            let Some(fold) = Fold::restore(row.kind, state) else { continue };
            let slot: Slot = Arc::new(Mutex::new(Published::new(fold, head.head.next_seq, head.head.time_ms)));
            if self.env.live.preload(&key, row.epoch, &slot) {
                held += 1;
            }
        }
        tracing::debug!(held, "the other devices' copies read back for the gateway");
    }

    /// What a round needs: the enrollment as the chain has it, the keys, and
    /// the hosts to follow — `None` while it may not run.
    fn ready(&self) -> Option<(View, Arc<KeyRing>, Vec<Host>)> {
        if !super::wanted(&self.env.paths) {
            return None;
        }
        let view = view(&self.env.state)?;
        let ring = self.env.keys.borrow().clone()?;
        if ring.account != view.account || ring.device != view.me {
            return None;
        }
        let hosts = followed(&view, &ring);
        Some((view, ring, hosts))
    }

    fn guard(&self) -> super::keys::Guard {
        let (paths, cancel) = (self.env.paths.clone(), self.env.cancel.clone());
        Box::new(move || !cancel.is_cancelled() && super::wanted(&paths))
    }

    /// One connection, or the reason there is none: how long to wait after.
    async fn round(&mut self) -> Duration {
        let Some((view, _ring, hosts)) = self.ready() else {
            self.drop_all();
            return IDLE_EVERY;
        };
        self.env.live.set_hosts(&hosts.iter().map(|h| h.id.clone()).collect::<Vec<_>>());
        if let Some(until) = self.quiet_until {
            let now = now_ms();
            if now < until {
                return Duration::from_millis(until - now).min(IDLE_EVERY);
            }
            self.quiet_until = None;
        }
        if self.store.is_none() {
            // The store is created only once there is something to keep.
            if hosts.is_empty() && !super::store::path(&self.env.paths).exists() {
                self.set_status(|s| *s = Status::default());
                return IDLE_EVERY;
            }
            match CopyStore::open(&self.env.paths, &view.account, &view.me, self.guard()) {
                Ok(store) => {
                    self.store = Some(store);
                    if self.purge(&hosts).is_err() {
                        self.drop_all();
                        return IDLE_EVERY;
                    }
                    self.preload();
                }
                Err(error) => {
                    if error != super::store::OFF {
                        tracing::warn!(%error, "cannot open the other devices' copies");
                    }
                    self.drop_all();
                    return IDLE_EVERY;
                }
            }
        }
        if self.purge(&hosts).is_err() {
            self.drop_all();
            return IDLE_EVERY;
        }
        if hosts.is_empty() {
            self.session = None;
            self.set_status(|s| *s = Status::default());
            return IDLE_EVERY;
        }
        // This computer's identity key was renewed and the link has not
        // restarted with the new one yet: no sign-in with the retired key
        // (`docs/host-identity-rotation.md` §3.8).
        if !super::holds_named_keys(&view.chain, &view.me, &self.env.identity) {
            self.session = None;
            self.set_status(|s| s.error = Some("waiting for this device's renewed identity key".into()));
            return IDLE_EVERY;
        }
        // A staged identity key is kept and the link has not shown the key in
        // use is still current: the chain here may be stale.
        if self.env.may_sign_in.as_ref().is_some_and(|may| !may()) {
            self.session = None;
            self.set_status(|s| s.error = Some("waiting for the link to sign in (an identity key renewal is staged)".into()));
            return IDLE_EVERY.min(Duration::from_secs(5));
        }
        let hub = match HubUrl::parse(&view.hub) {
            Ok(hub) => hub,
            Err(error) => {
                self.set_status(|s| s.error = Some(error));
                return IDLE_EVERY;
            }
        };
        let http = self
            .http
            .get_or_insert_with(|| {
                crate::agent::traffic::install_client_tap();
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(15))
                    .no_proxy()
                    .build()
                    .unwrap_or_default()
            })
            .clone();
        let token = match self.signed_in(&http, &hub, &view).await {
            Ok(token) => token,
            Err(wait) => return wait,
        };
        let session = Zeroizing::new(token.read().expect("token").to_string());
        let link = match rh_client::link::connect_device(&hub, &view.account, &view.me, self.env.identity.signing_key(), &session).await {
            Ok(link) => Arc::new(link),
            Err(ConnectError::Refused { code, .. }) => return self.refused(&view, &code),
            Err(ConnectError::Failed(error)) => return self.failed(error),
        };
        drop(session);
        let started = std::time::Instant::now();
        self.set_status(|s| {
            s.connected = true;
            s.error = None;
        });
        tracing::info!(hosts = hosts.len(), "following the other devices' copies");
        let pages = PageSource { http, hub, token };
        let ended = self.live(&view, hosts, link.clone(), pages).await;
        self.set_status(|s| s.connected = false);
        if started.elapsed() >= STABLE_AFTER {
            self.backoff = BACKOFF_MIN;
        }
        match ended {
            Ended::Cancelled => Duration::ZERO,
            Ended::Rebind => Duration::ZERO,
            Ended::Off => {
                tracing::info!("the aggregator was turned off, or its copies deleted: dropping what it held");
                self.drop_all();
                IDLE_EVERY
            }
            Ended::Unauthorized => {
                // A new session, after the backoff: a hub that keeps refusing is not hammered.
                self.session = None;
                self.failed("the hub did not take this device's session for a page".into())
            }
            Ended::Again(why) => self.failed(why),
        }
    }

    /// A connection that failed: back off.
    fn failed(&mut self, why: String) -> Duration {
        let wait = jitter(self.backoff);
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
        tracing::debug!(error = %why, "the copies' connection to the hub failed; trying again in {wait:?}");
        self.set_status(|s| s.error = Some(why));
        wait
    }

    /// The hub refused this device (`code`).
    fn refused(&mut self, view: &View, code: &str) -> Duration {
        match code {
            "denied" => {
                self.session = None;
                self.failed("the hub did not take this device's session".into())
            }
            "removed" | "device_removed" => self.removed(view),
            "host_devices_off" => {
                self.quiet_until = Some(now_ms() + REFUSED_EVERY.as_millis() as u64);
                self.set_status(|s| s.error = Some("the hub lets no device that runs AI act as a device now".into()));
                REFUSED_EVERY.min(IDLE_EVERY)
            }
            other => self.failed(format!("the hub refused: {other}")),
        }
    }

    /// The hub says this device was removed: what it held of the others goes
    /// now (the chain, not the hub, removes it for good; security review of
    /// S1-4, S2).
    fn removed(&mut self, view: &View) -> Duration {
        tracing::warn!(device = %view.me, "the hub says this device was removed: deleting the other devices' copies here");
        self.drop_all();
        if let Err(error) = super::forget_all(&self.env.paths) {
            tracing::error!(%error, "cannot delete the other devices' copies");
        }
        self.quiet_until = Some(now_ms() + REMOVED_EVERY.as_millis() as u64);
        IDLE_EVERY
    }

    /// The device session, minted when there is none or it expires soon.
    async fn signed_in(&mut self, http: &reqwest::Client, hub: &HubUrl, view: &View) -> Result<Arc<RwLock<Zeroizing<String>>>, Duration> {
        let now = now_ms();
        if let Some((token, expires)) = &self.session {
            if *expires > now + RENEW_BEFORE_MS {
                return Ok(token.clone());
            }
        }
        match rh_client::session::mint(http, hub, &view.account, &view.me, self.env.identity.signing_key(), now).await {
            Ok(minted) => {
                let expires = minted.expires_ms.unwrap_or(now + 60 * 60 * 1000);
                let token = match self.session.take() {
                    Some((held, _)) => {
                        *held.write().expect("token") = minted.token;
                        held
                    }
                    None => Arc::new(RwLock::new(minted.token)),
                };
                self.session = Some((token.clone(), expires));
                Ok(token)
            }
            Err(HttpError::Status { status, body, .. }) if status.as_u16() == 403 => {
                Err(self.refused(view, body["error"].as_str().unwrap_or("forbidden")))
            }
            Err(error) => Err(self.failed(match error {
                HttpError::Network(e) => e.without_url().to_string(),
                other => other.to_string(),
            })),
        }
    }

    /// Delete the rows of every host not followed now: `Err` when writing
    /// was refused.
    fn purge(&mut self, hosts: &[Host]) -> Result<(), ()> {
        let Some(store) = self.store.as_mut() else { return Ok(()) };
        match store.retain_hosts(|h| hosts.iter().any(|f| f.id == h)) {
            Ok(gone) => {
                if !gone.is_empty() {
                    tracing::info!(?gone, "deleted the copies of devices this one no longer follows");
                }
                Ok(())
            }
            Err(error) if error == super::store::OFF => Err(()),
            Err(error) => {
                tracing::warn!(%error, "cannot delete the copies of devices this one no longer follows");
                Ok(())
            }
        }
    }

    /// Follow on `link` until it ends.
    async fn live(&mut self, view: &View, hosts: Vec<Host>, link: Arc<Link>, pages: PageSource) -> Ended {
        let mut live = match Live::load(self, view, hosts, link, pages) {
            Ok(live) => live,
            Err(ended) => return ended,
        };
        // The chain or the keys may have moved while it connected: no host
        // narrowed meanwhile is subscribed with its keys.
        if let Err(ended) = live.rehost(self) {
            return ended;
        }
        let mut flush = tokio::time::interval(FLUSH_EVERY);
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut ticks = 0u64;
        let ended = loop {
            live.replan(self);
            if let Err(ended) = live.report(self) {
                break ended;
            }
            tokio::select! {
                update = live.rx.recv() => {
                    let Some(update) = update else { break Ended::Again("closed".into()) };
                    if let Err(ended) = live.on_update(self, update) {
                        break ended;
                    }
                }
                control = live.link.next_control(Duration::from_secs(3600)) => {
                    if let Some(control) = control {
                        live.on_control(self, &control);
                    }
                }
                () = live.link.closed() => break Ended::Again("the hub closed the connection".into()),
                _ = flush.tick() => {
                    if let Err(ended) = live.flush(self, false) {
                        break ended;
                    }
                    ticks += 1;
                    if ticks.is_multiple_of(5) {
                        if let Err(ended) = live.tick(self).await {
                            break ended;
                        }
                    }
                }
                changed = self.env.keys.changed() => {
                    if changed.is_err() {
                        break Ended::Cancelled;
                    }
                    if let Err(ended) = live.rehost(self) {
                        break ended;
                    }
                }
                changed = self.env.chain.changed() => {
                    if changed.is_err() {
                        break Ended::Cancelled;
                    }
                    if let Err(ended) = live.rehost(self) {
                        break ended;
                    }
                }
                // A page opened a session, or let one go: the plan again (at the top).
                changed = self.interest.changed() => {
                    if changed.is_err() {
                        break Ended::Cancelled;
                    }
                }
                () = self.env.cancel.cancelled() => break Ended::Cancelled,
            }
        };
        // Nobody's presence is known without the connection.
        self.env.live.presence_unknown();
        // What was folded and not written yet: written now, if it still may be.
        if !matches!(ended, Ended::Off | Ended::Cancelled) {
            let _ = live.flush(self, true);
        }
        live.stop_all();
        ended
    }
}

/// One connection's state.
struct Live {
    view: View,
    hosts: Vec<Host>,
    link: Arc<Link>,
    pages: PageSource,
    tx: mpsc::UnboundedSender<Update>,
    rx: mpsc::UnboundedReceiver<Update>,
    subs: HashMap<StreamId, Sub>,
    /// Which names exist (from `stream_synced`).
    exists: HashMap<StreamId, bool>,
    /// What is stored, by (host, kind, name).
    stored: HashMap<(String, Kind, String), Stored>,
    /// Each host's index as copied, for its sessions.
    indexes: BTreeMap<String, IndexCopy>,
    /// Written at the next flush, by (host, kind, name).
    pending: HashMap<(String, Kind, String), Row>,
    /// When (ms) each row was last written in this connection.
    written: HashMap<(String, Kind, String), u64>,
    /// Heads of unchanged copies to write (a heartbeat: how current they are).
    touches: HashMap<(String, Kind, String), (StreamId, SignedHead)>,
    online: BTreeMap<String, Option<bool>>,
    reported: Option<BTreeMap<String, HostCopy>>,
}

impl Live {
    fn load(c: &mut Copies, view: &View, hosts: Vec<Host>, link: Arc<Link>, pages: PageSource) -> Result<Self, Ended> {
        let rows = c.store.as_ref().ok_or(Ended::Off)?.rows().map_err(|e| Ended::Again(e.to_string()))?;
        let (tx, rx) = mpsc::unbounded_channel();
        let mut live = Live {
            view: view.clone(),
            hosts,
            link,
            pages,
            tx,
            rx,
            subs: HashMap::new(),
            exists: HashMap::new(),
            stored: HashMap::new(),
            indexes: BTreeMap::new(),
            pending: HashMap::new(),
            written: HashMap::new(),
            touches: HashMap::new(),
            online: BTreeMap::new(),
            reported: None,
        };
        for row in rows {
            if row.kind == Kind::Index {
                if let Some(index) = row.state.as_deref().and_then(|s| serde_json::from_str::<IndexCopy>(s).ok()) {
                    live.indexes.insert(row.host.clone(), index);
                }
            }
            if row.too_large {
                // Not fetched again after a restart.
                c.too_large.insert(row.stream);
            } else {
                live.exists.insert(row.stream, true);
            }
            live.stored.insert(
                (row.host.clone(), row.kind, row.name.clone()),
                Stored {
                    stream: row.stream,
                    epoch: row.epoch,
                    at_ms: row.head.as_ref().map(|h| h.head.time_ms),
                    too_large: row.too_large,
                    floor: row.head.as_ref().map(Floor::of),
                },
            );
        }
        Ok(live)
    }

    fn host(&self, id: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// Whether an update about `want` still counts: its stream is followed
    /// now, as that very want, for a host followed now.
    fn admits(&self, want: &Want) -> bool {
        self.subs.get(&want.stream).is_some_and(|s| &s.want == want) && self.host(&want.host).is_some()
    }

    /// The chain or the keys changed: the same enrollment carries on with
    /// the hosts it may follow now; another starts over.
    fn rehost(&mut self, c: &mut Copies) -> Result<(), Ended> {
        let Some((view, _ring, hosts)) = c.ready() else {
            return Err(Ended::Rebind);
        };
        if view.account != self.view.account || view.me != self.view.me || view.hub != self.view.hub {
            return Err(Ended::Rebind);
        }
        let gone: Vec<String> = self
            .hosts
            .iter()
            .filter(|h| !hosts.iter().any(|n| n.id == h.id))
            .map(|h| h.id.clone())
            .collect();
        let rekeyed: Vec<String> = hosts
            .iter()
            .filter(|n| self.host(&n.id).is_some_and(|h| h.keys != n.keys || h.machine_key != n.machine_key))
            .map(|n| n.id.clone())
            .collect();
        self.view = view;
        self.hosts = hosts;
        c.env.live.set_hosts(&self.hosts.iter().map(|h| h.id.clone()).collect::<Vec<_>>());
        // Presence of devices no longer in the account goes.
        let chain = &self.view.chain;
        self.online.retain(|id, _| chain.machine(id).is_some());
        // A device followed from now on: its presence as the hub last said it.
        for host in &self.hosts {
            if let Some(online) = self.online.get(&host.id) {
                c.env.live.set_presence(&host.id, *online);
            }
        }
        if !gone.is_empty() {
            // Narrowed, removed, no longer running AI: its copies go now.
            self.subs.retain(|_, sub| {
                let keep = !gone.contains(&sub.want.host);
                if !keep {
                    sub.stop.cancel();
                }
                keep
            });
            self.pending.retain(|(host, ..), _| !gone.contains(host));
            self.stored.retain(|(host, ..), _| !gone.contains(host));
            for host in &gone {
                self.indexes.remove(host);
            }
            self.written.retain(|(host, ..), _| !gone.contains(host));
            let hosts = self.hosts.clone();
            c.purge(&hosts).map_err(|()| Ended::Off)?;
        }
        // New keys (a rotation): its streams again, under them.
        for host in rekeyed {
            for sub in self.subs.values().filter(|s| s.want.host == host) {
                sub.stop.cancel();
            }
            self.subs.retain(|_, s| s.want.host != host);
        }
        Ok(())
    }

    /// Start what is wanted and not followed; stop what is followed and not
    /// wanted; delete the session rows no longer among the newest.
    fn replan(&mut self, c: &mut Copies) {
        let hosts: Vec<access::Host<'_>> = self.hosts.iter().map(|h| access::Host { id: &h.id, keys: &h.keys.0 }).collect();
        let wants = access::plan(&hosts, &self.indexes, &self.exists, &c.env.live.opened());
        let wanted: HashSet<StreamId> = wants.iter().map(|w| w.stream).collect();
        // What is known of streams no longer followed goes.
        c.failures.retain(|stream, _| wanted.contains(stream));
        let stored_streams: HashSet<StreamId> = self.stored.values().map(|s| s.stream).collect();
        self.exists.retain(|stream, _| wanted.contains(stream) || stored_streams.contains(stream));
        self.subs.retain(|stream, sub| {
            let keep = wanted.contains(stream);
            if !keep {
                sub.stop.cancel();
            }
            keep
        });
        // Sessions no longer among a host's newest (once its index is known): their rows go.
        let keep_sessions: HashSet<(&str, &str)> = wants
            .iter()
            .filter(|w| w.kind == Kind::Session)
            .map(|w| (w.host.as_str(), w.name.as_str()))
            .collect();
        // What memory holds of them goes too, unless a page is reading it.
        c.env
            .live
            .retain_sessions(|k| !self.indexes.contains_key(&k.host) || keep_sessions.contains(&(k.host.as_str(), k.name.as_str())));
        let stale: Vec<(String, Kind, String)> = self
            .stored
            .keys()
            .filter(|(host, kind, name)| {
                *kind == Kind::Session && self.indexes.contains_key(host) && !keep_sessions.contains(&(host.as_str(), name.as_str()))
            })
            .cloned()
            .collect();
        for key in stale {
            // Whatever the store says, nothing of it is written any more.
            self.pending.remove(&key);
            self.touches.remove(&key);
            if let Some(store) = c.store.as_mut() {
                match store.delete(&key.0, key.1, &key.2) {
                    Ok(_) => {
                        self.stored.remove(&key);
                    }
                    Err(error) => tracing::debug!(%error, host = %key.0, "cannot delete a session copy"),
                }
            }
        }
        let now = now_ms();
        for want in wants {
            if self.subs.contains_key(&want.stream) || c.too_large.contains(&want.stream) {
                continue;
            }
            if c.failures.get(&want.stream).is_some_and(|(at, _)| *at > now) {
                continue;
            }
            let Some(host) = self.host(&want.host) else { continue };
            let (machine_key, retired, keys) = (host.machine_key, host.retired.clone(), host.keys.clone());
            // A failed stream is read again from the start; any other from its cursor.
            let key = (want.host.clone(), want.kind, want.name.clone());
            let stored = self.stored.get(&key).filter(|s| s.stream == want.stream && !s.too_large);
            // Never below what was released, also when read again from the start.
            let floor = stored.and_then(|s| s.floor);
            // After a failure the stream is read again from its cursor first
            // (once), and only then from the start, under the floor.
            let tries = c.failures.get(&want.stream).map_or(0, |(_, tries)| *tries);
            let start = match (tries < 2, stored, c.store.as_ref()) {
                (true, Some(_), Some(store)) => match store.row(&key.0, key.1, &key.2) {
                    Ok(Some(Row {
                        head: Some(head),
                        state: Some(state),
                        stream,
                        ..
                    })) if stream == want.stream => Start::Resume { head, state },
                    _ => Start::Fresh { floor },
                },
                _ => Start::Fresh { floor },
            };
            let stop = c.env.cancel.child_token();
            tokio::spawn(follow_stream(
                Task {
                    want: want.clone(),
                    machine_key,
                    retired,
                    keys,
                    start,
                    link: self.link.clone(),
                    pages: self.pages.clone(),
                    tx: self.tx.clone(),
                    live: c.env.live.clone(),
                },
                stop.clone(),
            ));
            self.subs.insert(want.stream, Sub { want, stop });
        }
    }

    fn on_update(&mut self, c: &mut Copies, update: Update) -> Result<(), Ended> {
        match update {
            Update::State(copied) => {
                let Copied {
                    want,
                    head,
                    at_ms,
                    state,
                    index,
                } = *copied;
                if !self.admits(&want) {
                    return Ok(());
                }
                self.exists.insert(want.stream, true);
                c.failures.remove(&want.stream);
                let key = (want.host.clone(), want.kind, want.name.clone());
                // An index or status under an older name than the one kept: not kept.
                if want.kind != Kind::Session && self.stored.get(&key).is_some_and(|s| s.epoch > want.epoch && !s.too_large) {
                    return Ok(());
                }
                if let Some(index) = index {
                    self.indexes.insert(want.host.clone(), index);
                }
                self.stored.insert(
                    key.clone(),
                    Stored {
                        stream: want.stream,
                        epoch: want.epoch,
                        at_ms,
                        too_large: false,
                        floor: head.as_ref().map(Floor::of),
                    },
                );
                self.pending.insert(
                    key,
                    Row {
                        host: want.host,
                        kind: want.kind,
                        name: want.name,
                        stream: want.stream,
                        epoch: want.epoch,
                        head,
                        state: Some(state),
                        too_large: false,
                        updated_ms: now_ms(),
                    },
                );
            }
            Update::Missing { want } => {
                if self.admits(&want) {
                    self.exists.insert(want.stream, false);
                }
            }
            Update::Fresh { want, head, at_ms } => {
                if !self.admits(&want) {
                    return Ok(());
                }
                let key = (want.host.clone(), want.kind, want.name.clone());
                let Some(stored) = self.stored.get_mut(&key).filter(|s| s.stream == want.stream && !s.too_large) else {
                    return Ok(());
                };
                stored.at_ms = at_ms.or(stored.at_ms);
                let floor = Floor::of(&head);
                stored.floor = Some(stored.floor.map_or(floor, |f| Floor {
                    next_seq: f.next_seq.max(floor.next_seq),
                    time_ms: f.time_ms.max(floor.time_ms),
                }));
                // A copy waiting to be written takes the newer head; one
                // already written gets only its head and time moved.
                match self.pending.get_mut(&key) {
                    // Only at the place the waiting state stands.
                    Some(row) => {
                        if row.head.as_ref().is_some_and(|h| h.head.next_seq == head.head.next_seq) {
                            row.head = Some(head);
                        }
                    }
                    None => {
                        self.touches.insert(key, (want.stream, head));
                    }
                }
            }
            Update::TooLarge { want } => {
                // From a stream still followed for a host still followed only
                // (a late update must not bring back a purged host's row).
                if !self.admits(&want) {
                    return Ok(());
                }
                self.subs.remove(&want.stream);
                c.too_large.insert(want.stream);
                tracing::info!(host = %want.host, "a session copy is too large to keep");
                let key = (want.host.clone(), want.kind, want.name.clone());
                self.stored.insert(
                    key.clone(),
                    Stored {
                        stream: want.stream,
                        epoch: want.epoch,
                        at_ms: None,
                        too_large: true,
                        floor: None,
                    },
                );
                self.pending.insert(
                    key,
                    Row {
                        host: want.host,
                        kind: want.kind,
                        name: want.name,
                        stream: want.stream,
                        epoch: want.epoch,
                        head: None,
                        state: None,
                        too_large: true,
                        updated_ms: now_ms(),
                    },
                );
            }
            Update::Failed { want, error, unauthorized } => {
                if self.subs.get(&want.stream).is_some_and(|s| s.want == want) {
                    self.subs.remove(&want.stream);
                }
                let (_, tries) = c.failures.get(&want.stream).copied().unwrap_or((0, 0));
                if tries == 0 {
                    // Its host may have renewed its identity key: the chain
                    // is synced once, and the stream read again under it.
                    if let Some(resync) = &c.env.resync {
                        resync();
                    }
                }
                let wait = (REBUILD_MIN * 2u32.saturating_pow(tries.min(10))).min(REBUILD_MAX);
                c.failures.insert(want.stream, (now_ms() + wait.as_millis() as u64, tries + 1));
                tracing::warn!(host = %want.host, kind = want.kind.as_str(), %error, "a copy stopped; reading it again in {wait:?}");
                if unauthorized {
                    return Err(Ended::Unauthorized);
                }
            }
        }
        Ok(())
    }

    fn on_control(&mut self, c: &Copies, v: &Value) {
        match v["t"].as_str() {
            Some("mail") => c.env.nudge.notify_one(),
            Some("presence") => {
                for m in v["machines"].as_array().into_iter().flatten() {
                    let Some(id) = m["id"].as_str() else { continue };
                    let online = match m["state"].as_str() {
                        Some("online") => Some(true),
                        Some("offline") => Some(false),
                        _ => None,
                    };
                    self.online.insert(id.to_string(), online);
                    c.env.live.set_presence(id, online);
                }
            }
            _ => {}
        }
    }

    /// Write what changed: a session at most every [`SESSION_WRITE_EVERY`]
    /// (unless `all`), anything else at once; never a row of a host no
    /// longer followed.
    fn flush(&mut self, c: &mut Copies, all: bool) -> Result<(), Ended> {
        if !self.touches.is_empty() {
            let hosts: HashSet<&str> = self.hosts.iter().map(|h| h.id.as_str()).collect();
            let store = c.store.as_mut().ok_or(Ended::Off)?;
            for ((host, kind, name), (stream, head)) in self.touches.drain() {
                if !hosts.contains(host.as_str()) {
                    continue;
                }
                match store.touch(&host, kind, &name, &stream, &head, now_ms()) {
                    Ok(_) => {}
                    Err(error) if error == super::store::OFF => return Err(Ended::Off),
                    Err(error) => tracing::debug!(%error, %host, "cannot note how current a copy is"),
                }
            }
        }
        if self.pending.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let every = SESSION_WRITE_EVERY.as_millis() as u64;
        let hosts: HashSet<&str> = self.hosts.iter().map(|h| h.id.as_str()).collect();
        let mut later = HashMap::new();
        let mut due = Vec::new();
        for (key, row) in self.pending.drain() {
            if !hosts.contains(row.host.as_str()) {
                continue;
            }
            let recent = self.written.get(&key).is_some_and(|at| now.saturating_sub(*at) < every);
            if row.kind == Kind::Session && recent && !all {
                later.insert(key, row);
            } else {
                due.push((key, row));
            }
        }
        self.pending = later;
        let store = c.store.as_mut().ok_or(Ended::Off)?;
        for (key, row) in due {
            match store.put(&row) {
                Ok(()) => {
                    self.written.insert(key, now);
                }
                Err(error) if error == super::store::OFF => return Err(Ended::Off),
                Err(error) => tracing::warn!(%error, host = %row.host, kind = row.kind.as_str(), "cannot keep a copy"),
            }
        }
        Ok(())
    }

    /// Every second: still allowed, still connected, still signed in.
    async fn tick(&mut self, c: &mut Copies) -> Result<(), Ended> {
        if !c.store.as_ref().is_some_and(CopyStore::usable) {
            return Err(Ended::Off);
        }
        if self.link.silent_ms() > SILENT_FOR.as_millis() as u64 {
            return Err(Ended::Again("the hub has not answered for a minute".into()));
        }
        let http = self.pages.http.clone();
        let hub = self.pages.hub.clone();
        match c.signed_in(&http, &hub, &self.view.clone()).await {
            Ok(token) => self.pages.token = token,
            Err(_) => return Err(Ended::Again("the device session could not be renewed".into())),
        }
        Ok(())
    }

    /// Report what is held, when it changed.
    fn report(&mut self, c: &Copies) -> Result<(), Ended> {
        let mut hosts: BTreeMap<String, HostCopy> = BTreeMap::new();
        for host in &self.hosts {
            let index = self.indexes.get(&host.id);
            let status = self.stored.get(&(host.id.clone(), Kind::Status, String::new()));
            hosts.insert(
                host.id.clone(),
                HostCopy {
                    index_rows: index.map(|i| i.rows.len() as u64),
                    index_as_of_ms: index.and_then(|i| i.as_of),
                    status_at_ms: status.and_then(|s| s.at_ms),
                    sessions: self
                        .stored
                        .iter()
                        .filter(|((h, k, _), s)| h == &host.id && *k == Kind::Session && !s.too_large)
                        .count() as u64,
                    online: self.online.get(&host.id).copied().flatten(),
                },
            );
        }
        if self.reported.as_ref() != Some(&hosts) {
            c.set_status(|s| s.hosts = hosts.clone());
            self.reported = Some(hosts);
        }
        Ok(())
    }

    fn stop_all(&mut self) {
        for sub in self.subs.values() {
            sub.stop.cancel();
        }
        self.subs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_say_their_epochs_and_nothing_else() {
        let keys = Keys(vec![(0, [0x42; 32]), (3, [0x43; 32])]);
        let shown = format!("{keys:?}");
        assert_eq!(shown, "[0, 3]");
        assert!(!shown.contains("66"));
    }

    #[test]
    fn a_fold_takes_bodies_of_its_own_format_only() {
        let index = (
            FMT_INDEX_V1,
            br#"{"upserts":[{"source":"claude","id":"a"}],"removed":[],"asOf":5}"#.to_vec(),
        );
        let mut fold = Fold::empty(Kind::Index);
        fold.apply(&index).unwrap();
        let state = fold.to_json();
        let back = Fold::restore(Kind::Index, &state).unwrap();
        let Fold::Index(copy) = back else { panic!() };
        assert_eq!((copy.rows.len(), copy.as_of), (1, Some(5)));
        let mut status = Fold::empty(Kind::Status);
        assert!(status.apply(&index).is_err(), "an index body on a status stream");
        status.apply(&(FMT_STATUS_V1, br#"{"asOf":1,"os":{"name":"macos"}}"#.to_vec())).unwrap();
        status.apply(&(FMT_STATUS_V1, br#"{"asOf":2}"#.to_vec())).unwrap();
        assert_eq!(status.to_json(), r#"{"asOf":2}"#, "the newest status replaces the last");
        let mut feed = Fold::empty(Kind::Session);
        feed.apply(&(FMT_FEED_V1, br#"{"fromIndex":0,"items":[{"t":1}],"endIndex":1}"#.to_vec()))
            .unwrap();
        assert!(
            feed.apply(&(FMT_FEED_V1, br#"{"fromIndex":5,"items":[],"endIndex":5}"#.to_vec()))
                .is_err(),
            "a body past the items held"
        );
    }

    /// F3: a session's size is counted as it is folded: past
    /// [`MAX_SESSION_BYTES`] it stops, before the whole copy is built.
    #[test]
    fn a_session_too_large_to_keep_is_stopped_while_it_is_folded() {
        let mut feed = Fold::empty(Kind::Session);
        let line = "x".repeat(64 * 1024);
        let mut from = 0usize;
        let mut stopped = None;
        for round in 0..200 {
            let items: Vec<Value> = (0..4).map(|_| json!({"text": line})).collect();
            let body = json!({"fromIndex": from, "items": items, "endIndex": from + 4}).to_string();
            from += 4;
            match feed.apply(&(FMT_FEED_V1, body.into_bytes())) {
                Ok(()) => {}
                Err(e) => {
                    stopped = Some((round, e));
                    break;
                }
            }
        }
        let (round, error) = stopped.expect("stopped");
        assert_eq!(error, FoldError::TooLarge);
        assert!(round < 40, "stopped right past 8 MiB (round {round})");
        // Replace-from shrinks what is counted: a copy that starts over is small again.
        let Fold::Feed(_, sizes) = &feed else { panic!() };
        assert!(sizes.iter().sum::<usize>() > MAX_SESSION_BYTES);
        feed.apply(&(FMT_FEED_V1, br#"{"fromIndex":0,"items":[{"t":1}],"endIndex":1}"#.to_vec()))
            .unwrap();
        let Fold::Feed(copy, sizes) = &feed else { panic!() };
        assert_eq!((copy.items.len(), sizes.len()), (1, 1));
    }

    /// A connection's state for tests: a store under `paths`, following `hosts`.
    fn live(paths: &Paths, hosts: &[&str]) -> (Copies, Live) {
        use rh_proto::chain::{ChainState, DeviceV3, Flags};
        let ak = ed25519_dalek::SigningKey::from_bytes(&[1; 32]);
        let admin = ed25519_dalek::SigningKey::from_bytes(&[2; 32]);
        let genesis = rh_proto::chain::genesis3_draft(
            "a-1",
            ak.verifying_key().to_bytes(),
            DeviceV3 {
                id: "d-a".into(),
                sign_key: admin.verifying_key().to_bytes(),
                dh_key: [3; 32],
                flags: Flags { host: false, admin: true },
            },
            None,
            1,
        )
        .sign(&ak);
        let chain = ChainState::genesis(&genesis, None).unwrap();
        let env = Env {
            paths: paths.clone(),
            identity: Arc::new(MachineIdentity::from_seed(&[9; 32])),
            state: Arc::new(Mutex::new(AgentState::enrolling("ws://127.0.0.1:1", "a-1", "m-me", [0; 16], 1))),
            chain: watch::channel(0).1,
            keys: watch::channel(None).1,
            nudge: Arc::default(),
            cancel: CancellationToken::new(),
            status: Arc::default(),
            live: LiveCopies::new(),
            resync: None,
            may_sign_in: None,
        };
        env.live.set_hosts(&hosts.iter().map(|h| h.to_string()).collect::<Vec<_>>());
        let c = Copies {
            store: Some(CopyStore::open(paths, "a-1", "m-me", Box::new(|| true)).unwrap()),
            interest: env.live.interest(),
            env,
            backoff: BACKOFF_MIN,
            quiet_until: None,
            session: None,
            failures: HashMap::new(),
            too_large: HashSet::new(),
            http: None,
        };
        let link = Link::spawn(
            futures_util::sink::drain(),
            futures_util::stream::pending::<Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>>(),
            "/v1/client",
        );
        let (tx, rx) = mpsc::unbounded_channel();
        let live = Live {
            view: View {
                hub: "ws://127.0.0.1:1".into(),
                account: "a-1".into(),
                me: "m-me".into(),
                chain,
            },
            hosts: hosts
                .iter()
                .map(|id| Host {
                    id: id.to_string(),
                    machine_key: [7; 32],
                    retired: Vec::new(),
                    keys: Keys(vec![(0, [8; 32])]),
                })
                .collect(),
            link: Arc::new(link),
            pages: PageSource {
                http: reqwest::Client::new(),
                hub: HubUrl::parse("ws://127.0.0.1:1").unwrap(),
                token: Arc::default(),
            },
            tx,
            rx,
            subs: HashMap::new(),
            exists: HashMap::new(),
            stored: HashMap::new(),
            indexes: BTreeMap::new(),
            pending: HashMap::new(),
            written: HashMap::new(),
            touches: HashMap::new(),
            online: BTreeMap::new(),
            reported: None,
        };
        (c, live)
    }

    /// Host identity renewal (the hub phase's constraint): while this
    /// computer's renewal is pending the hub suspends its device role —
    /// `rotation_pending` on `/v1/session` or `/v1/client`. A refusal like
    /// any other, retried with backoff: nothing is deleted, nothing goes
    /// quiet for an hour as for a removal.
    #[tokio::test]
    async fn a_suspended_device_role_is_retried_not_taken_for_a_removal() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "pendingtest");
        let (mut c, live) = live(&paths, &["m-2"]);
        let wait = c.refused(&live.view, "rotation_pending");
        assert!(wait <= BACKOFF_MAX, "{wait:?}");
        assert!(c.quiet_until.is_none(), "not quiet as after a removal");
        assert!(super::super::store::path(&paths).exists(), "the copies stay");
        assert!(c.store.is_some());
        assert!(c.env.status.lock().unwrap().error.as_deref().is_some_and(|e| e.contains("rotation_pending")));
    }

    fn want(host: &str, name: &str) -> Want {
        Want {
            host: host.into(),
            kind: Kind::Session,
            name: name.into(),
            epoch: 0,
            stream: rh_proto::record::stream_id(&[8; 32], "claude", name).unwrap(),
        }
    }

    fn state(want: &Want, text: &str) -> Update {
        Update::State(Box::new(Copied {
            want: want.clone(),
            head: None,
            at_ms: None,
            state: json!({"items": [text]}).to_string(),
            index: None,
        }))
    }

    /// F4: an update that arrives after its host was purged (narrowed,
    /// removed) brings nothing back — not a copy, not a "too large" mark —
    /// and no row of a host no longer followed is written.
    #[tokio::test]
    async fn a_late_update_never_brings_back_a_purged_hosts_row() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "latetest");
        let (mut c, mut live) = live(&paths, &["m-2"]);
        let kept = want("m-2", "a");
        live.subs.insert(
            kept.stream,
            Sub {
                want: kept.clone(),
                stop: CancellationToken::new(),
            },
        );
        // m-9 was followed and purged: its updates come late.
        let purged = want("m-9", "b");
        live.on_update(&mut c, Update::TooLarge { want: purged.clone() }).ok().unwrap();
        live.on_update(&mut c, state(&purged, "PURGED")).ok().unwrap();
        assert!(live.pending.is_empty() && live.stored.is_empty(), "nothing of m-9 comes back");
        assert!(!c.too_large.contains(&purged.stream));
        // Even with its sub still listed (the race), a host no longer followed is not admitted.
        live.subs.insert(
            purged.stream,
            Sub {
                want: purged.clone(),
                stop: CancellationToken::new(),
            },
        );
        live.on_update(&mut c, Update::TooLarge { want: purged.clone() }).ok().unwrap();
        assert!(live.pending.is_empty());
        // m-2's updates are kept; a row already pending for m-9 is never written.
        live.on_update(&mut c, state(&kept, "KEPT")).ok().unwrap();
        live.pending.insert(
            ("m-9".into(), Kind::Session, "b".into()),
            Row {
                host: "m-9".into(),
                kind: Kind::Session,
                name: "b".into(),
                stream: purged.stream,
                epoch: 0,
                head: None,
                state: Some("{}".into()),
                too_large: false,
                updated_ms: 1,
            },
        );
        live.flush(&mut c, true).ok().unwrap();
        let rows = c.store.as_ref().unwrap().rows().unwrap();
        assert_eq!(rows.iter().map(|r| r.host.as_str()).collect::<Vec<_>>(), vec!["m-2"]);
    }

    /// F3: a session copy is written at once the first time, then at most
    /// every [`SESSION_WRITE_EVERY`]; everything is written when the
    /// connection ends.
    #[tokio::test]
    async fn a_busy_session_is_written_at_most_every_ten_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "throttletest");
        let (mut c, mut live) = live(&paths, &["m-2"]);
        let w = want("m-2", "a");
        live.subs.insert(
            w.stream,
            Sub {
                want: w.clone(),
                stop: CancellationToken::new(),
            },
        );
        let text = |c: &Copies| {
            c.store
                .as_ref()
                .unwrap()
                .rows()
                .unwrap()
                .into_iter()
                .find(|r| r.name == "a")
                .and_then(|r| r.state)
                .unwrap_or_default()
        };
        live.on_update(&mut c, state(&w, "FIRST")).ok().unwrap();
        live.flush(&mut c, false).ok().unwrap();
        assert!(text(&c).contains("FIRST"), "the first write is at once");
        live.on_update(&mut c, state(&w, "SECOND")).ok().unwrap();
        live.flush(&mut c, false).ok().unwrap();
        assert!(text(&c).contains("FIRST") && !live.pending.is_empty(), "the next waits");
        live.flush(&mut c, true).ok().unwrap();
        assert!(text(&c).contains("SECOND"), "and is written when the connection ends");
    }

    /// N2: a heartbeat on an unchanged copy moves how current it is known to
    /// be — its time and its cursor — without writing its state again.
    #[tokio::test]
    async fn a_heartbeat_keeps_a_quiet_copy_current_without_rewriting_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "freshtest");
        let (mut c, mut live) = live(&paths, &["m-2"]);
        let w = Want {
            host: "m-2".into(),
            kind: Kind::Status,
            name: String::new(),
            epoch: 0,
            stream: rh_proto::stream::status_stream(&[8; 32]),
        };
        live.subs.insert(
            w.stream,
            Sub {
                want: w.clone(),
                stop: CancellationToken::new(),
            },
        );
        let machine = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
        let head_at = |time_ms| {
            rh_proto::record::StreamHead {
                stream: w.stream,
                next_seq: 3,
                epoch: 0,
                link: [5; 32],
                time_ms,
            }
            .sign(&machine)
        };
        live.on_update(
            &mut c,
            Update::State(Box::new(Copied {
                want: w.clone(),
                head: Some(head_at(1_000)),
                at_ms: Some(1_000),
                state: r#"{"asOf":1}"#.into(),
                index: None,
            })),
        )
        .ok()
        .unwrap();
        live.flush(&mut c, true).ok().unwrap();
        // Hours later, a heartbeat: the same place, signed later.
        live.on_update(
            &mut c,
            Update::Fresh {
                want: w.clone(),
                head: head_at(9_000_000),
                at_ms: Some(9_000_000),
            },
        )
        .ok()
        .unwrap();
        live.flush(&mut c, false).ok().unwrap();
        let row = c.store.as_ref().unwrap().row("m-2", Kind::Status, "").unwrap().unwrap();
        assert_eq!(row.head.unwrap().head.time_ms, 9_000_000, "the cursor moved");
        assert_eq!(row.state.as_deref(), Some(r#"{"asOf":1}"#), "the state was not rewritten");
        live.report(&c).ok().unwrap();
        assert_eq!(
            c.env.status.lock().unwrap().hosts["m-2"].status_at_ms,
            Some(9_000_000),
            "statusAtMs moves"
        );
    }

    /// S1-7: what `copies.db` holds is in memory for the gateway as soon as the
    /// store opens — a hub that is down still leaves the last copies — and a
    /// session a page opens is followed ahead of the host's newest.
    #[tokio::test]
    async fn the_gateway_has_the_stored_copies_at_once_and_what_a_page_opens_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "gwtest");
        let (mut c, mut live) = live(&paths, &["m-2"]);
        let machine = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        let head = |stream: StreamId, seq: u64, time_ms: u64| {
            rh_proto::record::StreamHead {
                stream,
                next_seq: seq,
                epoch: 0,
                link: [5; 32],
                time_ms,
            }
            .sign(&machine)
        };
        // Seven sessions in m-2's index, the newest last; the stored index.
        let mut index = IndexCopy::default();
        for i in 0..7 {
            index.rows.insert(
                format!("claude/s{i}"),
                json!({"source": "claude", "id": format!("s{i}"), "updatedAt": format!("2026-10-19T0{i}:00:00.000Z"), "asking": i == 6}),
            );
        }
        let index_stream = rh_proto::stream::index_stream(&[8; 32]);
        c.store
            .as_mut()
            .unwrap()
            .put(&Row {
                host: "m-2".into(),
                kind: Kind::Index,
                name: String::new(),
                stream: index_stream,
                epoch: 0,
                head: Some(head(index_stream, 12, 1_700_000_000_000)),
                state: Some(serde_json::to_string(&index).unwrap()),
                too_large: false,
                updated_ms: 1,
            })
            .unwrap();
        c.preload();
        let gateway = c.env.live.clone();
        assert_eq!(gateway.held(), 1);
        assert_eq!(gateway.dir().borrow()[0].copy_as_of_ms, Some(1_700_000_000_000));
        let (out, mut rx) = super::super::live::Outbox::new(16, 1 << 20);
        let _index = gateway.subscribe(Key::new("m-2", Kind::Index, ""), 1, &out, None, true).unwrap();
        let frame: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!((frame["reset"].as_bool(), frame["headSeq"].as_u64()), (Some(true), Some(12)));
        assert_eq!(frame["bodies"][0]["upserts"].as_array().map(Vec::len), Some(7));
        // The newest five are planned; a page opens the oldest: it comes first.
        live.indexes.insert("m-2".into(), index);
        live.replan(&mut c);
        let followed = |live: &Live| {
            let mut names: Vec<String> = live
                .subs
                .values()
                .filter(|s| s.want.kind == Kind::Session)
                .map(|s| s.want.name.clone())
                .collect();
            names.sort();
            names
        };
        assert_eq!(followed(&live), vec!["claude/s2", "claude/s3", "claude/s4", "claude/s5", "claude/s6"]);
        let opened = gateway
            .subscribe(Key::new("m-2", Kind::Session, "claude/s0"), 2, &out, None, true)
            .unwrap();
        live.replan(&mut c);
        assert!(followed(&live).contains(&"claude/s0".to_string()), "{:?}", followed(&live));
        // Let go of, it lingers (followed still).
        drop(opened);
        live.replan(&mut c);
        assert!(followed(&live).contains(&"claude/s0".to_string()));
        live.stop_all();
    }
}
