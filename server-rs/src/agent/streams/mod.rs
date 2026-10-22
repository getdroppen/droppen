//! The agent's encrypted streams (M2, architecture doc §5 "加密流"): a copy
//! of this machine's session list, status and session transcripts on the hub,
//! sealed under keys derived from `MK`, so the account's devices can read them
//! while the machine is off.
//!
//! - **index** — the session list's rows ([`content::index_rows`]): the
//!   machine's own sessions of the last 30 days ([`content::Scope`]).
//! - **status** — what the UI shows about the machine (name, versions,
//!   quota readings, account names; no credential), every minute if changed.
//! - **one stream per session** in scope — the feed's normalised items in its
//!   replace-from deltas.
//!
//! The task polls (every [`LIST_EVERY`]) rather than following each feed: what
//! it writes is a copy for offline reading, a few seconds behind is fine, and
//! a delta covering several changes is one record rather than one per change
//! — no keystroke ever becomes an append. Drafts go out only as `EPHEMERAL`
//! frames, never stored, and only for streams the hub says somebody is
//! reading.
//!
//! First enrolment backfills every session in scope, newest first, through a
//! byte budget ([`BACKFILL_BYTES_PER_SEC`]) and at most [`MAX_IN_FLIGHT`]
//! requests at a time; a session with more than [`PAGES_OVER`] of history goes
//! up as snapshot pages and a manifest rather than as records. A stream whose
//! tail grows past its state is compacted the same way ([`writer::Plan`]), and
//! so is one whose tail has been idle for [`COMPACT_IDLE`], whose oldest
//! uncovered record is [`COMPACT_OLDEST`] old (the hub keeps tails in Redis,
//! short), or that the hub refused an append for with `compact`; one
//! the hub disagrees with (a `CONFLICT`), whose transcript was rewritten under
//! it, or that is in an older `MK` epoch, is rebuilt from the transcript with a
//! rebase. [`writer`] has the write-ahead and `ACK` rules, [`store`] the
//! persistence.
//!
//! The account's plan bounds the copy ([`retention`]): sessions outside the
//! retention window are deleted from it, a long live session's oldest pages
//! are trimmed at its checkpoints, the index and status get a write at least
//! daily, and above 90% of the storage quota the least recently updated
//! sessions' copies go to make room.
//!
//! Once a device reads them, streams are named per key epoch ([`naming`]): a
//! rotation moves them to new names, and re-encryption is that move.

pub(crate) mod content;
pub(crate) mod naming;
pub(crate) mod retention;
pub(crate) mod store;
pub(crate) mod writer;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use rh_proto::frame::Frame;
use rh_proto::record::{self, stream_key, StreamId, FMT_FEED_V1, FMT_INDEX_V1, FMT_STATUS_V1};
use rh_proto::stream::{seal_ephemeral, INDEX_SOURCE, STATUS_SOURCE};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::drive::Drive;
use crate::feed::{encode_fp, Transcript};
use crate::sessions::{Paths, SessionSummary, Source};
use content::{Delta, Scope, SessionCursor};
use retention::HubPlan;
use store::{IndexDiff, Kind, PageRef, Row, Store};
use writer::{Answer, PageSpec, Plan, Step, Writer};

use super::bridge::Loopback;
use super::hub::now_ms;

/// How often the session list and transcripts are compared with the copy.
pub const LIST_EVERY: Duration = Duration::from_secs(3);
/// How often the machine status is gathered.
pub const STATUS_EVERY: Duration = Duration::from_secs(60);
/// Heartbeat heads, every stream alike (M2 review: index and status
/// streams beat every minute and sessions every half hour, which told the hub
/// which stream was which). The index's beat is what "as of" means offline,
/// so that is good to five minutes.
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(5 * 60);
/// Requests in flight at once, over all streams.
pub const MAX_IN_FLIGHT: usize = 4;
/// What the copy may send a second (records and pages), the first backfill included.
pub const BACKFILL_BYTES_PER_SEC: u64 = 1024 * 1024;
/// A session with more history than this is backfilled as pages.
pub const PAGES_OVER: usize = 64 * 1024;
/// A delta bigger than this is sent as a rebuild (pages) instead.
pub const REBUILD_OVER: usize = 4 * 1024 * 1024;
/// Compaction: when the tail has this many records (`RH_AGENT_COMPACT_RECORDS`)…
pub const COMPACT_RECORDS: u64 = 128;
/// …or more bytes than this and than the state it describes.
pub const COMPACT_MIN_BYTES: i64 = 512 * 1024;
/// The least time between two checkpoints of one stream (`RH_AGENT_COMPACT_EVERY_SECS`).
pub const COMPACT_EVERY: Duration = Duration::from_secs(60);
/// …and a tail with no append for this long is compacted (`RH_AGENT_COMPACT_IDLE_SECS`)…
pub const COMPACT_IDLE: Duration = Duration::from_secs(5 * 60);
/// …as is one whose oldest uncovered record is this old (`RH_AGENT_COMPACT_OLDEST_SECS`).
pub const COMPACT_OLDEST: Duration = Duration::from_secs(30 * 60);

/// When to compact, overridable for tests and the acceptance run.
#[derive(Debug, Clone, Copy)]
struct Compaction {
    records: u64,
    every: Duration,
    idle: Duration,
    oldest: Duration,
}

fn compaction() -> Compaction {
    static AT: std::sync::OnceLock<Compaction> = std::sync::OnceLock::new();
    *AT.get_or_init(|| {
        let env = |name: &str| crate::env::var(name).and_then(|v| v.parse::<u64>().ok());
        Compaction {
            records: env("DROPPEN_AGENT_COMPACT_RECORDS").unwrap_or(COMPACT_RECORDS).max(1),
            every: env("DROPPEN_AGENT_COMPACT_EVERY_SECS").map_or(COMPACT_EVERY, Duration::from_secs),
            idle: env("DROPPEN_AGENT_COMPACT_IDLE_SECS").map_or(COMPACT_IDLE, Duration::from_secs),
            oldest: env("DROPPEN_AGENT_COMPACT_OLDEST_SECS").map_or(COMPACT_OLDEST, Duration::from_secs),
        }
    })
}

impl Compaction {
    /// Whether `row`'s tail is due a checkpoint at `now` (ms); `asked`: the
    /// hub refused an append with `compact`.
    fn due(&self, row: &Row, now: u64, asked: bool) -> bool {
        if asked {
            return true;
        }
        if row.tail_records == 0 {
            return false;
        }
        let ms = |d: Duration| d.as_millis() as u64;
        let big = row.tail_records >= self.records || row.tail_bytes > COMPACT_MIN_BYTES.max(row.state_bytes);
        let idle = now.saturating_sub(row.updated_ms) >= ms(self.idle);
        let old = row.tail_since_ms.is_some_and(|t| now.saturating_sub(t) >= ms(self.oldest));
        (big || idle || old) && row.checkpoint_ms.is_none_or(|t| now.saturating_sub(t) >= ms(self.every))
    }
}
/// A marked session the list does not name is tried again after this.
const GONE_RETRY: Duration = Duration::from_secs(60);
/// Drafts to live readers: at most one frame per stream this often.
pub const DRAFT_EVERY: Duration = Duration::from_millis(300);
/// Where `streams.db` keeps the account's plan.
const PLAN_KEY: &str = "plan";

/// What the link tells the sync task.
pub enum Event {
    /// Signed in; frames go out through this until [`Event::Disconnected`].
    /// The link is up: where frames and control messages to the hub go.
    Connected(mpsc::UnboundedSender<Frame>, mpsc::UnboundedSender<String>),
    /// The connection went; nothing in flight will be answered.
    Disconnected,
    /// `ACK` or `CONFLICT`.
    Frame(Frame),
    /// `stream_refused` / `stream_watched`.
    Control(Value),
    /// A machine key arrived (in a channel or from the mailbox). With
    /// `reencrypt`, every stream is rebuilt under it and the old ciphertext
    /// goes; otherwise new records simply use it.
    KeysChanged {
        /// Rebuild every stream under the new key and drop the old ciphertext.
        reencrypt: bool,
    },
    /// The name devices show for this machine changed.
    DisplayName(Option<String>),
    /// The machine's chain alarms changed ([`super::state::Alarm`]): every
    /// device reads them in the status stream.
    Alarms(Vec<super::state::Alarm>),
    /// The account changes this machine saw ([`super::state::AgentState::note_change`]):
    /// every device reads them in the status stream.
    Changes(Vec<super::state::Alarm>),
    /// A device's channel hello said it reads streams named per key epoch
    /// (`streams/2`, [`naming`]).
    ReadsEpochNames,
    /// A device opened a channel for the first time, or the chain moved on:
    /// the status's `controllers` ([`Options::controllers`]) changed.
    Controllers,
}

/// The link's handle on the sync task.
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Event>,
}

impl Handle {
    /// A handle with no sync task behind it (tests of the link).
    #[cfg(test)]
    pub fn detached() -> Handle {
        let (tx, _rx) = mpsc::unbounded_channel();
        Handle { tx }
    }

    /// A handle whose events a test reads instead of a sync task.
    #[cfg(test)]
    pub fn captured() -> (Handle, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Handle { tx }, rx)
    }

    /// Tell the sync task about the link.
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}

/// Whether machine key `epoch` is the one the chain committed for it.
pub type KeyCheck = Arc<dyn Fn(u32, &[u8; 32]) -> bool + Send + std::marker::Sync>;

/// Which devices ever opened a channel to this machine, with its chain as it
/// is now ([`super::state::AgentState::controllers`]), read when a status
/// body is built.
pub type ControllersView = Arc<dyn Fn() -> Option<super::state::Controllers> + Send + std::marker::Sync>;

/// What the sync task starts with.
pub struct Options {
    /// What devices call this machine (`None`: its hostname).
    pub display_name: Option<String>,
    /// The machine's chain alarms, for the status stream.
    pub alarms: Vec<super::state::Alarm>,
    /// The account changes it saw, for the status stream.
    pub changes: Vec<super::state::Alarm>,
    /// Which stored machine keys the writer may use: only those the chain
    /// committed ([`committed_only`]).
    pub committed: KeyCheck,
    /// The status's `controllers`, read from the agent state each time.
    pub controllers: ControllersView,
}

/// The machine keys of `keys` that `check` says the chain committed; the
/// rest are skipped, with a warning.
pub fn committed_only(keys: BTreeMap<u32, zeroize::Zeroizing<[u8; 32]>>, check: &KeyCheck) -> BTreeMap<u32, zeroize::Zeroizing<[u8; 32]>> {
    keys.into_iter()
        .filter(|(epoch, key)| {
            let ok = check(*epoch, key);
            if !ok {
                tracing::warn!(epoch, "a stored machine key the chain does not commit (yet): not sealing under it");
            }
            ok
        })
        .collect()
}

/// Start the sync task for this enrolled instance; it ends with `cancel`
/// (the join handle says when its store is closed).
pub fn spawn(
    paths: &Paths,
    drive: Arc<Drive>,
    signing: SigningKey,
    loopback: Arc<Loopback>,
    options: Options,
    cancel: CancellationToken,
) -> (Handle, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let store = Store::open(paths);
    // The plan the hub last told: the window holds across a restart.
    let plan: Option<HubPlan> = store.get_kv(PLAN_KEY).and_then(|p| serde_json::from_str(&p).ok());
    let streams2 = store.get_kv(naming::STREAMS2_KEY).is_some();
    let resign = manifests_to_resign(&store, &signing.verifying_key().to_bytes());
    let env = Scope::from_env();
    let mut sync = Sync {
        paths: paths.clone(),
        drive,
        loopback,
        writer: Writer::new(store, signing),
        out: None,
        scope: Scope {
            days: retention::window_days(env.days, plan.as_ref()),
            ..env
        },
        env_days: env.days,
        plan,
        plan_fresh: false,
        freed: 0,
        retention_since: 0,
        streams2,
        migrate_within: crate::env::var("DROPPEN_AGENT_MIGRATE_WITHIN_SECS")
            .and_then(|v| v.parse().ok())
            .map_or(naming::MIGRATE_WITHIN, Duration::from_secs),
        retry_at: HashMap::new(),
        watched: HashSet::new(),
        drafts: HashMap::new(),
        seen_sig: HashMap::new(),
        last_list: None,
        last_status: None,
        beats: HashMap::new(),
        budget: Budget::new(BACKFILL_BYTES_PER_SEC),
        cli: None,
        names: HashMap::new(),
        display_name: options.display_name,
        alarms: options.alarms,
        changes: options.changes,
        committed: options.committed,
        controllers: options.controllers,
        reencrypt: HashMap::new(),
        text: None,
        compact_now: HashSet::new(),
        resign,
    };
    sync.reencrypt = sync.writer.store.reencrypt_list().into_iter().collect();
    let task = tokio::spawn(async move { sync.run(rx, cancel).await });
    (Handle { tx }, task)
}

/// A byte budget a second, refilled continuously, bursting to four seconds' worth.
struct Budget {
    rate: f64,
    tokens: f64,
    at: Instant,
}

impl Budget {
    fn new(rate: u64) -> Self {
        Self {
            rate: rate as f64,
            tokens: rate as f64 * 4.0,
            at: Instant::now(),
        }
    }

    fn available(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * self.rate).min(self.rate * 4.0);
        self.at = now;
        self.tokens > 0.0
    }

    fn spend(&mut self, bytes: usize) {
        self.tokens -= bytes as f64;
    }
}

struct Sync {
    paths: Paths,
    drive: Arc<Drive>,
    loopback: Arc<Loopback>,
    writer: Writer,
    out: Option<mpsc::UnboundedSender<Frame>>,
    scope: Scope,
    /// Streams to leave alone until then (a refusal's back-off).
    retry_at: HashMap<StreamId, Instant>,
    watched: HashSet<StreamId>,
    drafts: HashMap<StreamId, tokio::task::JoinHandle<()>>,
    /// The transcript signature each session stream was last compared at.
    seen_sig: HashMap<StreamId, String>,
    last_list: Option<Instant>,
    last_status: Option<Instant>,
    beats: HashMap<StreamId, Instant>,
    budget: Budget,
    cli: Option<(Instant, Value)>,
    /// Session streams' names, for drafts: stream → (source, id).
    names: HashMap<StreamId, (Source, String)>,
    /// What devices call this machine (`None`: its hostname).
    display_name: Option<String>,
    /// Chain alarms every device should see (status stream `alarms`).
    alarms: Vec<super::state::Alarm>,
    /// Account changes every device should see (status stream `changes`).
    changes: Vec<super::state::Alarm>,
    /// Which stored machine keys the chain committed.
    committed: KeyCheck,
    /// Which devices ever opened a channel (status stream `controllers`).
    controllers: ControllersView,
    /// Streams to rebuild under the newest key, deleting what was before, and
    /// the epoch each must reach: kept in `streams.db` until the hub
    /// acknowledges the rebuilt copy. Named per epoch ([`naming`]): streams
    /// to move to a name of that epoch, kept until the old one is deleted.
    reencrypt: HashMap<StreamId, u32>,
    /// Control messages to the hub.
    text: Option<mpsc::UnboundedSender<String>>,
    /// Streams the hub refused an append for with `compact`: checkpoint first.
    compact_now: HashSet<StreamId>,
    /// `RH_AGENT_SYNC_DAYS`: the window when the plan keeps longer, or says nothing.
    env_days: u64,
    /// The account's plan as the hub last told it (kept in `streams.db`).
    plan: Option<HubPlan>,
    /// The plan came on this connection: its usage figure is recent enough to
    /// delete copies for (one kept from before a restart is not).
    plan_fresh: bool,
    /// What this agent deleted for space since the hub last said the usage.
    freed: u64,
    /// When the retention last changed: copies checkpointed before it are
    /// trimmed at once rather than within the hour.
    retention_since: u64,
    /// A device reads streams named per key epoch: name them so ([`naming`]).
    /// Until then (`false`), every name is the first epoch's, as before.
    streams2: bool,
    /// A rotation's session migration starts at a random moment within this.
    migrate_within: Duration,
    /// Streams whose manifest another identity key signed — this machine
    /// renewed its key since — to sign again ([`Writer::resign`]); kept in
    /// `streams.db` until the hub took each.
    resign: HashSet<StreamId>,
}

/// Where `streams.db` keeps the identity key (hex) the writer last signed with.
const SIGNER_KEY: &str = "signer";
/// Where it keeps [`Sync::resign`] (a JSON list of hex stream ids).
const RESIGN_KEY: &str = "resign";
/// At most this many manifests are signed again a round.
const RESIGN_PER_ROUND: usize = 4;

/// The manifests to sign again when the writer starts with `signer`: after
/// an identity renewal (`docs/host-identity-rotation.md` §5.4) every stream
/// with a manifest — readers take one from the hub only under the
/// machine's current key — plus whatever an earlier run left unsigned.
/// Remembers `signer` as the key in use.
fn manifests_to_resign(store: &Store, signer: &[u8; 32]) -> HashSet<StreamId> {
    let mut resign: HashSet<StreamId> = store
        .get_kv(RESIGN_KEY)
        .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| crate::agent::hub::unhex32(s).map(StreamId))
        .collect();
    let signer_hex = crate::agent::hub::hex(signer);
    match store.get_kv(SIGNER_KEY) {
        Some(before) if before != signer_hex => {
            let manifests: Vec<StreamId> = store.all().into_iter().filter(|r| r.manifest_upto.is_some()).map(|r| r.stream).collect();
            tracing::info!(
                streams = manifests.len(),
                "this machine's identity key was renewed: signing every stream's manifest again with the new one"
            );
            resign.extend(manifests);
            save_resign(store, &resign);
        }
        _ => {}
    }
    store.set_kv(SIGNER_KEY, &signer_hex);
    resign
}

fn save_resign(store: &Store, resign: &HashSet<StreamId>) {
    let list: Vec<String> = resign.iter().map(|s| crate::agent::hub::hex(&s.0)).collect();
    store.set_kv(RESIGN_KEY, &serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()));
}

impl Sync {
    async fn run(&mut self, mut rx: mpsc::UnboundedReceiver<Event>, cancel: CancellationToken) {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = rx.recv() => match event {
                    Some(event) => self.on_event(event).await,
                    None => return,
                },
                _ = tick.tick() => self.round().await,
                () = cancel.cancelled() => return,
            }
        }
    }

    fn send(&self, frame: Frame) {
        if let Some(out) = &self.out {
            let _ = out.send(frame);
        }
    }

    fn backing_off(&self, stream: &StreamId) -> bool {
        self.retry_at.get(stream).is_some_and(|t| Instant::now() < *t)
    }

    async fn on_event(&mut self, event: Event) {
        match event {
            Event::Connected(out, text) => {
                self.out = Some(out);
                self.text = Some(text);
                // Re-encryption left unfinished: ask the hub again what it holds.
                if !self.reencrypt.is_empty() {
                    self.ask_stream_list();
                }
                self.writer.disconnected();
                self.beats.clear();
                self.load_keys();
                // Whatever was written ahead goes again, byte for byte.
                for row in self.writer.store.all() {
                    if row.pending.is_some() {
                        if let Some(frame) = self.writer.resend(&row) {
                            self.send(frame);
                        }
                    }
                }
            }
            Event::Disconnected => {
                self.out = None;
                self.text = None;
                self.plan_fresh = false;
                self.writer.disconnected();
                for (_, task) in self.drafts.drain() {
                    task.abort();
                }
                self.watched.clear();
            }
            Event::Frame(Frame::Ack { ch, head, .. }) => {
                let answer = self.writer.on_ack(ch, head, now_ms());
                self.answered(answer).await;
            }
            Event::Frame(Frame::Conflict { ch, head, .. }) => {
                let answer = self.writer.on_conflict(ch, head);
                self.answered(answer).await;
            }
            Event::Frame(_) => {}
            Event::KeysChanged { reencrypt } if self.streams2 => {
                self.load_keys();
                // A new epoch names the index and status anew at once (the
                // list and status rounds), and the sessions at a random moment
                // within hours — at once when it re-encrypts.
                self.schedule_migration(reencrypt);
                if reencrypt {
                    // Whatever the hub holds that this agent no longer knows: deleted.
                    self.ask_stream_list();
                }
                self.last_list = None;
                self.last_status = None;
                for (_, task) in self.drafts.drain() {
                    task.abort();
                }
                self.watch(self.watched.clone());
            }
            Event::KeysChanged { reencrypt } => {
                self.load_keys();
                if let (true, Some((epoch, _))) = (reencrypt, self.mk()) {
                    let streams: Vec<StreamId> = self.writer.store.all().iter().map(|r| r.stream).collect();
                    self.writer.store.mark_reencrypt(&streams, epoch);
                    self.reencrypt = self.writer.store.reencrypt_list().into_iter().collect();
                    tracing::info!(
                        streams = self.reencrypt.len(),
                        epoch,
                        "re-encrypting every stream under the new machine key"
                    );
                    // And whatever the hub holds that this agent no longer
                    // knows (a store reset, a session long gone): deleted.
                    self.ask_stream_list();
                }
                // The status says which epoch it is in by being in it; re-gather now.
                self.last_status = None;
                // Drafts restart sealed under the newest key.
                for (_, task) in self.drafts.drain() {
                    task.abort();
                }
                self.watch(self.watched.clone());
            }
            Event::DisplayName(name) => {
                self.display_name = name;
                self.last_status = None;
            }
            Event::Alarms(alarms) => {
                self.alarms = alarms;
                self.last_status = None;
            }
            Event::Changes(changes) => {
                self.changes = changes;
                self.last_status = None;
            }
            Event::Controllers => self.last_status = None,
            Event::ReadsEpochNames => {
                if self.streams2 {
                    return;
                }
                tracing::info!("a device reads streams named per key epoch (streams/2): naming them so from now on");
                self.streams2 = true;
                self.writer.store.set_kv(naming::STREAMS2_KEY, "1");
                // Names from the first epoch (all of them, so far) move to the
                // newest one's, like after a rotation.
                self.schedule_migration(false);
                self.last_list = None;
                self.last_status = None;
            }
            Event::Control(v) => match v["t"].as_str() {
                Some("plan") => self.on_plan(&v),
                Some("stream_refused") => {
                    let ch = v["ch"].as_u64().and_then(|c| u32::try_from(c).ok()).unwrap_or(0);
                    let code = v["code"].as_str().unwrap_or("");
                    if let Some(stream) = self.writer.on_refused(ch) {
                        if code == "compact" {
                            // The hub's tail is full: the refused append was not
                            // stored, so it goes, and the stream is checkpointed
                            // now; the next round sends the delta again after it.
                            tracing::info!(stream = %short(&stream), "the hub asks for a checkpoint first");
                            self.writer.store.drop_pending(&stream);
                            self.compact_now.insert(stream);
                            return;
                        }
                        let wait = match code {
                            "rate" | "unavailable" => Duration::from_millis(v["retry_ms"].as_u64().unwrap_or(1000).clamp(100, 60_000)),
                            "missing_pages" => Duration::from_secs(5),
                            "quota" => Duration::from_secs(10 * 60),
                            _ => Duration::from_secs(60 * 60),
                        };
                        if !matches!(code, "rate" | "unavailable") {
                            tracing::warn!(stream = %short(&stream), code, "the hub refused a stream write");
                            // A manifest signed again that the hub will not
                            // take: the next checkpoint signs one anyway.
                            self.resigned(&stream);
                        }
                        self.retry_at.insert(stream, Instant::now() + wait);
                    }
                }
                Some("stream_list") => {
                    // What the hub holds for this machine, asked for while
                    // streams move or are re-encrypted. Of what this agent
                    // does not know, only an older name of a stream it does
                    // know goes (a delete that never reached the hub); the
                    // rest — a store reset, a session no longer listed — may be
                    // the only copy left, and is the hub's retention sweep's
                    // to expire.
                    if self.reencrypt.is_empty() {
                        return;
                    }
                    let held: Vec<StreamId> = v["streams"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s["stream"].as_str())
                        .filter_map(crate::agent::hub::unhex32)
                        .map(StreamId)
                        .collect();
                    let (known, entries) = match (self.writer.store.try_all(), self.writer.store.try_index_rows()) {
                        (Ok(known), Ok(entries)) => (known, entries),
                        (Err(error), _) | (_, Err(error)) => {
                            tracing::warn!(%error, "cannot read streams.db: deleting none of the streams the hub holds");
                            return;
                        }
                    };
                    let keys: Vec<(u32, &[u8; 32])> = self.writer.keys().collect();
                    let strays = naming::strays(&held, Some(&known), Some(&entries), &keys, self.first_epoch());
                    let mine: HashSet<StreamId> = known.iter().map(|r| r.stream).collect();
                    let unknown = held.iter().filter(|s| !mine.contains(s)).count();
                    if unknown > strays.len() {
                        tracing::info!(
                            streams = unknown - strays.len(),
                            "the hub holds streams this agent cannot place; keeping them for the hub's retention sweep"
                        );
                    }
                    for stream in strays {
                        tracing::info!(stream = %short(&stream), "the hub still holds an older name of a stream that moved; deleting it");
                        self.delete_on_hub(&stream);
                    }
                }
                Some("stream_watched") => {
                    let streams: HashSet<StreamId> = v["streams"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s.as_str().and_then(super::hub::unhex32).map(StreamId))
                        .collect();
                    self.watch(streams);
                }
                _ => {}
            },
        }
    }

    async fn answered(&mut self, answer: Answer) {
        match answer {
            Answer::Committed { stream, checkpoint } => {
                self.retry_at.remove(&stream);
                // A rebuilt copy under the target epoch is acknowledged: that
                // stream is re-encrypted (and the hub dropped what was before).
                // Named per epoch, re-encryption is a move to a new name
                // instead, done when the old stream is deleted.
                if let (true, false, Some(target)) = (checkpoint, self.streams2, self.reencrypt.get(&stream).copied()) {
                    if self.writer.store.get(&stream).is_some_and(|r| r.epoch >= target) {
                        self.reencrypt.remove(&stream);
                        self.writer.store.clear_reencrypt(&stream);
                        if self.reencrypt.is_empty() {
                            tracing::info!("every stream is re-encrypted under the new machine key");
                        }
                    }
                }
                // More may be waiting behind this one (a backfill, a long index): look again soon.
                self.last_list = None;
                if let Some(row) = self.writer.store.get(&stream) {
                    if let Some(head) = self.writer.head(&row, now_ms()) {
                        self.send(head);
                        self.beats.insert(stream, Instant::now());
                    }
                }
            }
            Answer::Resigned { stream } => {
                self.retry_at.remove(&stream);
                self.resigned(&stream);
                if let Some(row) = self.writer.store.get(&stream) {
                    if let Some(head) = self.writer.head(&row, now_ms()) {
                        self.send(head);
                        self.beats.insert(stream, Instant::now());
                    }
                }
            }
            Answer::Conflict { stream, head } => {
                // A rebuild signs its manifest with the key in use anyway.
                self.resigned(&stream);
                tracing::info!(stream = %short(&stream), head, "the hub's stream is not where this agent left it; rebuilding it");
                self.seen_sig.remove(&stream);
                if let Err(error) = self.rebuild(&stream, head).await {
                    tracing::warn!(stream = %short(&stream), %error, "cannot rebuild a stream yet");
                    self.retry_at.insert(stream, Instant::now() + Duration::from_secs(10));
                }
            }
            Answer::Unknown => {}
        }
    }

    /// The hub said the account's plan (after sign-in, on a change, after a
    /// write refused for the quota, hourly): keep it, and the window it makes.
    fn on_plan(&mut self, v: &Value) {
        let Some(plan) = HubPlan::parse(v, now_ms()) else {
            tracing::warn!("a plan message this agent cannot read; ignoring it");
            return;
        };
        let days = retention::window_days(self.env_days, Some(&plan));
        let changed = self
            .plan
            .as_ref()
            .is_none_or(|p| (p.plan.as_str(), p.retention_days, p.storage_bytes) != (plan.plan.as_str(), plan.retention_days, plan.storage_bytes));
        if self.plan.as_ref().is_none_or(|p| p.retention_days != plan.retention_days) {
            self.retention_since = now_ms();
        }
        if changed || days != self.scope.days {
            tracing::info!(
                plan = %plan.plan,
                retention_days = plan.retention_days,
                storage_bytes = ?plan.storage_bytes,
                usage_bytes = plan.usage_bytes,
                window_days = days,
                "the account's plan"
            );
        }
        self.scope.days = days;
        self.writer.store.set_kv(PLAN_KEY, &serde_json::to_string(&plan).unwrap_or_default());
        self.plan = Some(plan);
        self.plan_fresh = true;
        self.freed = 0;
        // Act on it now rather than at the next tick.
        self.last_list = None;
    }

    /// Entries older than this are trimmed from session copies (none without a plan).
    fn trim_cutoff(&self) -> Option<u64> {
        retention::trim_cutoff(self.plan.as_ref(), now_ms())
    }

    /// The machine keys from `machine-keys.json` the chain committed — never
    /// one it does not (a key file left from an earlier enrollment, or one
    /// written ahead of the chain): new records are sealed under the newest
    /// of these, not the newest in the file (M2 re-review).
    fn load_keys(&mut self) {
        match super::state::load_machine_keys(&self.paths) {
            Ok(keys) => {
                for (epoch, key) in committed_only(keys, &self.committed) {
                    self.writer.add_key(epoch, key);
                }
            }
            Err(error) => tracing::warn!(%error, "cannot read the machine keys"),
        }
    }

    fn mk(&self) -> Option<(u32, [u8; 32])> {
        self.writer.current().map(|(e, k)| (e, *k))
    }

    /// The machine-key epoch new records and pages are sealed under.
    fn epoch(&self) -> Option<u32> {
        self.writer.current().map(|(e, _)| e)
    }

    /// The first epoch held: what every stream was named under before
    /// `streams/2`, and what a stream with no name epoch of its own still is.
    fn first_epoch(&self) -> u32 {
        self.writer.first().map_or(0, |(e, _)| e)
    }

    /// The epoch new streams (and the index and status) are named under: the
    /// newest once a device reads epoch names, else the first.
    fn name_target(&self) -> Option<u32> {
        if self.streams2 {
            self.epoch()
        } else {
            self.writer.first().map(|(e, _)| e)
        }
    }

    /// What a new stream named under `target` records as its name epoch.
    fn name_for_new(&self, target: u32) -> Option<u32> {
        self.streams2.then_some(target)
    }

    /// A stream marked for re-encryption the way it was before epoch names: a
    /// rebuild in place, past its head.
    fn legacy_reencrypt(&self, stream: &StreamId) -> bool {
        !self.streams2 && self.reencrypt.contains_key(stream)
    }

    /// The stream's row, made on first use in the current epoch.
    fn row(&mut self, stream: StreamId, kind: Kind, source: &str, id: &str, name_epoch: Option<u32>) -> Row {
        match self.writer.store.get(&stream) {
            Some(row) => row,
            None => {
                let epoch = self.mk().map_or(0, |(e, _)| e);
                self.writer.store.create(&stream, kind, source, id, epoch, name_epoch, now_ms())
            }
        }
    }

    /// The index's or status's row under `target`'s name. One under an older
    /// name (a rotation, or the first device that reads epoch names) is marked
    /// to go once the new one is on the hub — and while it has a request in
    /// flight, the new one waits (the index's acknowledged rows are one table).
    fn named_row(&mut self, kind: Kind, source: &str, target: u32, mk: &[u8; 32]) -> Option<Row> {
        let stream = record::stream_id(mk, source, "").ok()?;
        if let Some(row) = self.writer.store.get(&stream) {
            return Some(row);
        }
        let older: Vec<Row> = self.writer.store.all().into_iter().filter(|r| r.kind == kind).collect();
        if older.iter().any(|r| self.writer.busy(&r.stream) || r.pending.is_some()) {
            return None;
        }
        let row = self.row(stream, kind, source, "", self.name_for_new(target));
        if !older.is_empty() {
            tracing::info!(?kind, epoch = target, "naming the stream under the newest machine key");
            let streams: Vec<StreamId> = older.iter().map(|r| r.stream).collect();
            self.writer.store.mark_reencrypt(&streams, target);
            self.reencrypt = self.writer.store.reencrypt_list().into_iter().collect();
        }
        Some(row)
    }

    /// Mark every stream named before the newest epoch to move to its name:
    /// the index and status at once, the sessions from a random moment within
    /// [`naming::MIGRATE_WITHIN`] (`immediate`: now — the rotation
    /// re-encrypts, a device was removed). Kept in `streams.db`.
    fn schedule_migration(&mut self, immediate: bool) {
        let Some(current) = self.epoch() else { return };
        let first = self.first_epoch();
        // A migration a newer epoch overtakes goes straight to the newest
        // (host identity renewal, §5.4: a removal's epoch, then the
        // renewal's): a name of an epoch in between that the hub never
        // held — nothing acknowledged, nothing written ahead — is forgotten
        // rather than filled, so nothing is moved twice.
        let never: Vec<StreamId> = self
            .writer
            .store
            .all()
            .iter()
            .filter(|r| naming::never_written(r, first, current) && !self.writer.busy(&r.stream))
            .map(|r| r.stream)
            .collect();
        for stream in never {
            tracing::info!(stream = %short(&stream), epoch = current, "a name the hub never held: skipped for the newest epoch's");
            self.forget(&stream);
        }
        let behind: Vec<StreamId> = self
            .writer
            .store
            .all()
            .iter()
            .filter(|r| naming::name_epoch(r, first) < current)
            .map(|r| r.stream)
            .collect();
        if behind.is_empty() {
            return;
        }
        self.writer.store.mark_reencrypt(&behind, current);
        self.reencrypt = self.writer.store.reencrypt_list().into_iter().collect();
        let now = now_ms();
        let prior = self.writer.store.get_kv(naming::MIGRATE_AT_KEY).and_then(|v| v.parse().ok());
        let at = naming::migrate_at(now, immediate, prior, self.migrate_within, &mut rand::thread_rng());
        self.writer.store.set_kv(naming::MIGRATE_AT_KEY, &at.to_string());
        tracing::info!(
            streams = behind.len(),
            epoch = current,
            sessions_in_secs = at.saturating_sub(now) / 1000,
            "moving the streams to the newest machine key's names"
        );
    }

    fn ready(&self, row: &Row) -> bool {
        !self.writer.busy(&row.stream) && row.pending.is_none() && !self.backing_off(&row.stream)
    }

    async fn round(&mut self) {
        if self.out.is_none() {
            return;
        }
        if self.mk().is_none() {
            self.load_keys();
            if self.mk().is_none() {
                return;
            }
        }
        let now = Instant::now();
        if self.last_list.is_none_or(|t| now.duration_since(t) >= LIST_EVERY) {
            self.last_list = Some(now);
            let listed = self.list_round().await;
            // Named per epoch, re-encryption is the list round's migration.
            if !self.reencrypt.is_empty() && !self.streams2 {
                self.reencrypt_round(&listed).await;
            }
        }
        if self.last_status.is_none_or(|t| now.duration_since(t) >= STATUS_EVERY) {
            self.last_status = Some(now);
            self.status_round().await;
        }
        self.resend_refused();
        self.heartbeats();
        self.compactions().await;
        self.resign_round();
    }

    /// Sign again, a few at a time, the manifests another identity key
    /// signed ([`Sync::resign`]): the same position and pages, the new key's
    /// signatures — the pages are not sent again. A stream with records past
    /// its manifest gets a checkpoint instead (signed with the key in use),
    /// one without a manifest nothing (its heads are signed anew by the
    /// heartbeat).
    fn resign_round(&mut self) {
        if self.resign.is_empty() {
            return;
        }
        let mut sent = 0;
        let mut done = Vec::new();
        for stream in self.resign.iter().copied().collect::<Vec<_>>() {
            if sent >= RESIGN_PER_ROUND || self.writer.in_flight() >= MAX_IN_FLIGHT {
                break;
            }
            let Some(row) = self.writer.store.get(&stream) else {
                done.push(stream);
                continue;
            };
            if !self.ready(&row) {
                continue;
            }
            if row.manifest_upto != Some(row.next_seq) {
                if row.manifest_upto.is_some() {
                    self.compact_now.insert(stream);
                }
                done.push(stream);
                continue;
            }
            match self.writer.resign(&row, now_ms()) {
                Some(frame) => {
                    self.send(frame);
                    sent += 1;
                }
                None => done.push(stream),
            }
        }
        for stream in done {
            self.resigned(&stream);
        }
    }

    /// `stream`'s manifest needs no signing again (any more).
    fn resigned(&mut self, stream: &StreamId) {
        if self.resign.remove(stream) {
            save_resign(&self.writer.store, &self.resign);
            if self.resign.is_empty() {
                tracing::info!("every stream's manifest is signed with this machine's identity key now");
            }
        }
    }

    /// Written-ahead appends the hub refused for now (`rate`, `unavailable`:
    /// Redis restarting) go again, byte for byte, once their back-off is over.
    fn resend_refused(&mut self) {
        for row in self.writer.store.all() {
            if row.pending.is_some() && !self.writer.busy(&row.stream) && !self.backing_off(&row.stream) {
                if let Some(frame) = self.writer.resend(&row) {
                    self.send(frame);
                }
            }
        }
    }

    // ------------------------------------------------------------ index and sessions

    async fn list(&self) -> Vec<SessionSummary> {
        let bindings = self.drive.accounts.session_account_map().await;
        let drivers = self.drive.injections.driver_states();
        let filter = if self.scope.external {
            crate::sessions::ScopeFilter::All
        } else {
            crate::sessions::ScopeFilter::Owned
        };
        crate::sessions::list_and_filter(&self.drive.paths, &bindings, &drivers, None, None, filter).await
    }

    /// Compare the session list with the copy; the sessions it listed
    /// (`source/id`; none when it could not run).
    async fn list_round(&mut self) -> HashSet<String> {
        let Some(target) = self.name_target() else { return HashSet::new() };
        let Some(mk) = self.writer.key(target).copied() else {
            return HashSet::new();
        };
        let rows = self.list().await;
        let now = now_ms();
        let listed: HashMap<String, &SessionSummary> = rows.iter().map(|r| (content::row_key(r), r)).collect();
        // Space first: what it deletes leaves the index in this same round.
        self.space_round(&listed, now);
        let held_out = self.held_out(&listed, now);
        let wanted = self.index_entries(&rows, &held_out, target, now);

        // The index, under the newest name it may have.
        if let Some(index) = self.named_row(Kind::Index, INDEX_SOURCE, target, &mk) {
            if self.ready(&index) && self.writer.in_flight() < MAX_IN_FLIGHT {
                let moving = !naming::acked(&index) && self.writer.store.all().iter().any(|r| r.kind == Kind::Index && r.stream != index.stream);
                if self.legacy_reencrypt(&index.stream) {
                    let _ = self.reencrypt_stream(&index).await;
                } else if moving {
                    // A new name: the whole index at once, as pages — a reader
                    // finding it never sees half of it.
                    if let Err(error) = self.checkpoint(&index, index_plan(&wanted, 0, true)) {
                        tracing::warn!(%error, "cannot name the index anew yet");
                    }
                } else {
                    let held = self.writer.store.index_rows();
                    let (ups, removed) = content::index_diff(&held, &wanted);
                    if !ups.is_empty() || !removed.is_empty() {
                        let steps: Vec<Step> = content::index_bodies(&ups, &removed, false, now)
                            .into_iter()
                            .map(|(body, upserts, removed)| Step {
                                fmt: FMT_INDEX_V1,
                                body,
                                cursor: String::new(),
                                diff: Some(IndexDiff { upserts, removed }),
                            })
                            .collect();
                        self.append(&index, &steps);
                    } else if naming::acked(&index) && retention::refresh_due(index.updated_ms, now) {
                        // Nothing changed for a day: `{"asOf"}`, so the hub (which
                        // expires streams by their last content write) keeps the
                        // index of a machine that is on but quiet.
                        self.append(
                            &index,
                            &[Step {
                                fmt: FMT_INDEX_V1,
                                body: content::as_of_body(now),
                                cursor: String::new(),
                                diff: None,
                            }],
                        );
                    }
                }
            }
        }

        // The sessions, newest first, each to its newest name.
        let scope = self.scope;
        let all = self.writer.store.all();
        let first = self.first_epoch();
        let sessions = naming::by_session(&all);
        let in_scope: Vec<&SessionSummary> = rows
            .iter()
            .filter(|r| scope.admits(r, now) && !held_out.contains(&content::row_key(r)))
            .collect();
        for summary in in_scope {
            if self.writer.in_flight() >= MAX_IN_FLIGHT || !self.budget.available() {
                break;
            }
            let group = sessions.get(&content::row_key(summary)).map_or(&[][..], Vec::as_slice);
            let row = match naming::current(group, first) {
                Some(row) => row.clone(),
                None => {
                    let Ok(stream) = record::stream_id(&mk, summary.source.as_str(), &summary.id) else {
                        continue;
                    };
                    self.row(stream, Kind::Session, summary.source.as_str(), &summary.id, self.name_for_new(target))
                }
            };
            let stream = row.stream;
            self.names.insert(stream, (summary.source, summary.id.clone()));
            if !self.ready(&row) {
                continue;
            }
            let path = Path::new(&summary.file_path);
            let Some(sig) = crate::sessions::file_signature(path).await else {
                continue;
            };
            if self.seen_sig.get(&stream) == Some(&sig) && !self.legacy_reencrypt(&stream) {
                continue;
            }
            // A new name beside an old one (a migration that did not get to
            // rebuild it): pages, so the index never points at half of it.
            let as_pages = group.len() > 1;
            if let Err(error) = self.session_round(&row, summary.source, path, &sig, as_pages).await {
                tracing::warn!(stream = %short(&stream), %error, "cannot copy a session yet");
                self.retry_at.insert(stream, Instant::now() + Duration::from_secs(30));
            }
        }

        self.window_round(&listed, &wanted, now);
        if self.streams2 && !self.reencrypt.is_empty() {
            self.migrate_round(&listed, &wanted, target, now).await;
        }
        listed.into_keys().collect()
    }

    /// The index's entries: the sessions in scope (less those held out for
    /// space), each — once a device reads epoch names — with the epoch its
    /// stream is named under (`se`).
    fn index_entries(&self, rows: &[SessionSummary], held_out: &HashSet<String>, target: u32, now: u64) -> BTreeMap<String, String> {
        let mut wanted = content::index_rows(rows, &self.scope, now);
        wanted.retain(|key, _| !held_out.contains(key));
        if self.streams2 {
            let all = self.writer.store.all();
            let sessions = naming::by_session(&all);
            let first = self.first_epoch();
            for (key, json) in wanted.iter_mut() {
                let group = sessions.get(key).map_or(&[][..], Vec::as_slice);
                *json = naming::with_se(json, naming::entry_epoch(group, first, target));
            }
        }
        wanted
    }

    /// Move streams named before their target epoch (a rotation, or the first
    /// device that reads epoch names) to new names ([`naming`]): an old index
    /// or status goes once its successor is on the hub (the rounds make it);
    /// a session, once its due moment has come, is rebuilt under the new name
    /// from its transcript — a batch at a time, in shuffled order — and its
    /// old stream goes once the new one is acknowledged and the index points
    /// at it. Nothing goes for being missing from this round's list, which
    /// may be short ([`naming::migration_fate`]): a session it does not
    /// name keeps its old stream, for the window round to judge.
    async fn migrate_round(&mut self, listed: &HashMap<String, &SessionSummary>, wanted: &BTreeMap<String, String>, target: u32, now: u64) {
        use rand::seq::SliceRandom;
        let due = self
            .writer
            .store
            .get_kv(naming::MIGRATE_AT_KEY)
            .and_then(|v| v.parse::<u64>().ok())
            .is_none_or(|at| now >= at);
        let all = match self.writer.store.try_all() {
            Ok(all) => all,
            Err(error) => {
                tracing::warn!(%error, "cannot read streams.db: no stream moves this round");
                return;
            }
        };
        let entries = self.writer.store.try_index_rows().ok();
        let round = naming::Round {
            all: &all,
            first: self.first_epoch(),
            entries: entries.as_ref(),
            wanted,
            due,
        };
        let mut marked: Vec<(StreamId, u32)> = self.reencrypt.iter().map(|(s, t)| (*s, *t)).collect();
        marked.shuffle(&mut rand::thread_rng());
        for (old, to) in marked {
            let Some(row) = all.iter().find(|r| r.stream == old) else {
                self.reencrypt.remove(&old);
                self.writer.store.clear_reencrypt(&old);
                continue;
            };
            // A request still in flight for it would land after the delete.
            let idle = !self.writer.busy(&old) && row.pending.is_none();
            match naming::migration_fate(&round, row, to, idle) {
                naming::Fate::Unmark => {
                    // Named there already (it was marked for re-encryption before epoch names).
                    self.reencrypt.remove(&old);
                    self.writer.store.clear_reencrypt(&old);
                }
                naming::Fate::Retire => {
                    tracing::info!(stream = %short(&old), "its successor under the new name is on the hub: deleting the old stream");
                    self.delete_on_hub(&old);
                }
                naming::Fate::Keep => {}
                naming::Fate::Rebuild => {
                    if self.writer.in_flight() >= MAX_IN_FLIGHT || !self.budget.available() {
                        break;
                    }
                    let Some(summary) = listed.get(&format!("{}/{}", row.source, row.id)) else {
                        continue;
                    };
                    if let Err(error) = self.migrate_session(summary, target).await {
                        tracing::warn!(stream = %short(&old), %error, "cannot move a session to its new name yet");
                    }
                }
            }
        }
    }

    /// Rebuild a session under its name in `target` from its transcript:
    /// manifest and pages from seq 0, under a fresh link.
    async fn migrate_session(&mut self, summary: &SessionSummary, target: u32) -> Result<(), String> {
        let mk = self.writer.key(target).copied().ok_or("no key for the target epoch")?;
        let stream = record::stream_id(&mk, summary.source.as_str(), &summary.id).map_err(|e| e.to_string())?;
        let row = self.row(stream, Kind::Session, summary.source.as_str(), &summary.id, Some(target));
        self.names.insert(stream, (summary.source, summary.id.clone()));
        let path = Path::new(&summary.file_path);
        let sig = crate::sessions::file_signature(path).await.ok_or("unreadable")?;
        let transcript = crate::feed::load_transcript(&self.drive.paths, summary.source, path, &sig).await;
        self.rebuild_session(&row, &transcript, &sig, 0, true)
    }

    /// Copies of sessions outside the window go (relay-and-plans §3.4): the
    /// agent first, a day before the hub's sweep would. A session listed and
    /// no longer in scope; one not listed at all only once its copy has not
    /// been written for the whole window (a list that came back short must
    /// not delete what is still there). Out of the index in the same round.
    fn window_round(&mut self, listed: &HashMap<String, &SessionSummary>, wanted: &BTreeMap<String, String>, now: u64) {
        let horizon = now.saturating_sub(self.scope.days.saturating_mul(retention::DAY_MS));
        let mut gone = Vec::new();
        for row in self.writer.store.all() {
            if gone.len() >= retention::DELETES_PER_ROUND {
                break;
            }
            let key = format!("{}/{}", row.source, row.id);
            if row.kind != Kind::Session || wanted.contains_key(&key) || self.writer.busy(&row.stream) {
                continue;
            }
            let outside = match listed.get(&key) {
                Some(summary) => !self.scope.admits(summary, now),
                None => row.updated_ms < horizon,
            };
            if outside {
                gone.push(row.stream);
            }
        }
        if !gone.is_empty() {
            tracing::info!(
                streams = gone.len(),
                window_days = self.scope.days,
                "deleting the copies of sessions outside the window"
            );
        }
        for stream in gone {
            self.delete_on_hub(&stream);
        }
    }

    /// Above 90% of the account's quota (as the hub said on this connection,
    /// less what was freed since), this machine's least recently updated
    /// session copies go — never the index or status, never one updated in
    /// the last day — until the estimate is down to 85% (relay-and-plans
    /// §3.5). Each is remembered, so it is not copied again until it changes.
    fn space_round(&mut self, listed: &HashMap<String, &SessionSummary>, now: u64) {
        if !self.plan_fresh {
            return;
        }
        let Some((plan, limit)) = self.plan.as_ref().and_then(|p| p.storage_bytes.map(|l| (p.clone(), l))) else {
            return;
        };
        let usage = retention::usage_estimate(&plan, self.freed);
        if usage as f64 <= limit as f64 * retention::EVICT_ABOVE {
            return;
        }
        let mut names: HashMap<StreamId, (String, String)> = HashMap::new();
        let mut candidates = Vec::new();
        for row in self.writer.store.all() {
            if row.kind != Kind::Session || self.writer.busy(&row.stream) {
                continue;
            }
            let key = format!("{}/{}", row.source, row.id);
            let updated_at = listed.get(&key).map(|s| s.updated_at.clone());
            let updated_ms = updated_at.as_deref().and_then(iso_ms).unwrap_or(row.updated_ms);
            candidates.push(retention::Candidate {
                stream: row.stream,
                updated_ms,
                bytes: retention::stream_bytes(&row),
            });
            names.insert(row.stream, (key, updated_at.unwrap_or_else(|| ms_iso(row.updated_ms))));
        }
        let chosen = retention::evictions(candidates, usage, limit, now);
        if chosen.is_empty() {
            return;
        }
        let bytes: u64 = chosen.iter().map(|c| c.bytes).sum();
        tracing::info!(
            sessions = chosen.len(),
            bytes,
            usage,
            limit,
            "the account's storage is nearly full: deleting this machine's least recently updated session copies"
        );
        for c in chosen {
            if let Some((key, updated_at)) = names.get(&c.stream) {
                self.writer.store.mark_evicted(key, updated_at, now);
            }
            self.delete_on_hub(&c.stream);
            self.freed = self.freed.saturating_add(c.bytes);
        }
    }

    /// The sessions deleted for space that stay out of the copy: those not
    /// updated since. One updated since comes back while there is room; one
    /// out of the window (or long gone) is forgotten.
    fn held_out(&mut self, listed: &HashMap<String, &SessionSummary>, now: u64) -> HashSet<String> {
        let mut out = HashSet::new();
        let horizon = now.saturating_sub(self.scope.days.saturating_mul(retention::DAY_MS));
        for (key, (updated_at, at_ms)) in self.writer.store.evicted() {
            match listed.get(&key) {
                Some(summary) if self.scope.admits(summary, now) => {
                    let changed = iso_ms(&summary.updated_at) > iso_ms(&updated_at);
                    if changed && retention::space_allows(self.plan.as_ref(), self.freed) {
                        tracing::info!(session = %key, "a session whose copy was deleted for space changed: copying it again");
                        self.writer.store.clear_evicted(&key);
                    } else {
                        out.insert(key);
                    }
                }
                Some(_) => self.writer.store.clear_evicted(&key),
                None if at_ms < horizon => self.writer.store.clear_evicted(&key),
                None => {
                    out.insert(key);
                }
            }
        }
        out
    }

    /// Bring a session's copy up to its transcript; `as_pages`: a first upload goes up as pages whatever its size.
    async fn session_round(&mut self, row: &Row, source: Source, path: &Path, sig: &str, as_pages: bool) -> Result<(), String> {
        let fresh = row.next_seq == 0 && row.manifest_upto.is_none() && row.cursor.is_empty();
        let transcript: Arc<Transcript> = if fresh {
            Arc::new(
                crate::feed::read_transcript_uncached(&self.drive.paths, source, path)
                    .await
                    .ok_or("unreadable")?,
            )
        } else {
            crate::feed::load_transcript(&self.drive.paths, source, path, sig).await
        };
        let chain = transcript.chain().clone();
        let len = transcript.len();
        if self.legacy_reencrypt(&row.stream) && !fresh {
            // Past the head, so the hub drops every older record at once.
            return self.rebuild_session(row, &transcript, sig, row.next_seq + 1, true);
        }
        if fresh {
            let cutoff = self.trim_cutoff();
            let (start, windowed) = copy_start(&transcript.items, cutoff);
            let size: usize = transcript.items[start..]
                .iter()
                .map(|i| serde_json::to_vec(i).map_or(0, |b| b.len()))
                .sum();
            if size > PAGES_OVER || windowed || as_pages {
                // A long history goes up as pages: one upload, no records to
                // compact later — and so does one the retention window trims,
                // so its window-start page comes first. Its link is fresh, not
                // the empty stream's: a reader that subscribed before the
                // stream existed holds the empty state at seq 0 and must not
                // take this for a compaction it is already consistent with.
                let (plan, _) = session_plan(row, &transcript, sig, start, 0, true, false, cutoff, self.epoch());
                self.checkpoint(row, plan)?;
            } else {
                let steps = session_steps(&transcript, start, start, len, &chain, sig);
                self.append(row, &steps);
            }
            self.seen_sig.insert(row.stream, sig.to_string());
            return Ok(());
        }
        let cursor = SessionCursor::parse(&row.cursor).unwrap_or_default();
        match content::session_delta(&cursor, &chain) {
            Delta::None => {}
            Delta::From(from) if from >= cursor.start => {
                let bytes: usize = transcript.items[from..]
                    .iter()
                    .map(|i| serde_json::to_vec(i).map_or(0, |b| b.len()))
                    .sum();
                if bytes > REBUILD_OVER {
                    return self.rebuild_session(row, &transcript, sig, row.next_seq, true);
                }
                let steps = session_steps(&transcript, cursor.start, from, len, &chain, sig);
                self.append(row, &steps);
            }
            Delta::From(_) | Delta::Rebuild => {
                tracing::info!(stream = %short(&row.stream), "a transcript changed under its copy; rebuilding it");
                return self.rebuild_session(row, &transcript, sig, row.next_seq, true);
            }
        }
        self.seen_sig.insert(row.stream, sig.to_string());
        Ok(())
    }

    fn append(&mut self, row: &Row, steps: &[Step]) {
        match self.writer.append(row, steps) {
            Ok(Some(frame)) => {
                if let Frame::Append { records, .. } = &frame {
                    self.budget.spend(records.iter().map(|r| r.ct.len()).sum());
                }
                self.send(frame);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(stream = %short(&row.stream), %error, "cannot seal an append"),
        }
    }

    fn checkpoint(&mut self, row: &Row, plan: Plan) -> Result<(), String> {
        let frames = self.writer.checkpoint(row, plan, now_ms())?;
        for frame in frames {
            if let Frame::Page { data, .. } = &frame {
                self.budget.spend(data.len());
            }
            self.send(frame);
        }
        self.writer.store.touch_checkpoint(&row.stream, now_ms());
        Ok(())
    }

    fn rebuild_session(&mut self, row: &Row, transcript: &Transcript, sig: &str, upto: u64, fresh_link: bool) -> Result<(), String> {
        let start = content::session_start(transcript.len());
        let (plan, _) = session_plan(row, transcript, sig, start, upto, fresh_link, false, self.trim_cutoff(), self.epoch());
        self.checkpoint(row, plan)?;
        self.seen_sig.insert(row.stream, sig.to_string());
        Ok(())
    }

    /// Rebuild `stream` from this machine's truth, past the hub's `head`.
    async fn rebuild(&mut self, stream: &StreamId, head: u64) -> Result<(), String> {
        let row = self.writer.store.get(stream).ok_or("no such stream")?;
        let upto = head.max(row.next_seq);
        match row.kind {
            Kind::Index => {
                let held_out: HashSet<String> = self.writer.store.evicted().into_keys().collect();
                let target = self.name_target().ok_or("no machine key")?;
                let rows = self.index_entries(&self.list().await, &held_out, target, now_ms());
                let plan = index_plan(&rows, upto, true);
                self.checkpoint(&row, plan)
            }
            Kind::Status => {
                let body = self.status_body().await;
                let plan = status_plan(&body, upto, true);
                self.checkpoint(&row, plan)
            }
            Kind::Session => {
                let source = source_of(&row).ok_or("an unknown session stream")?;
                let summary = self
                    .list()
                    .await
                    .into_iter()
                    .find(|s| s.source == source && s.id == row.id)
                    .ok_or("the session is gone")?;
                let path = Path::new(&summary.file_path);
                let sig = crate::sessions::file_signature(path).await.ok_or("unreadable")?;
                let transcript = crate::feed::load_transcript(&self.drive.paths, source, path, &sig).await;
                self.rebuild_session(&row, &transcript, &sig, upto, true)
            }
        }
    }

    /// Rebuild `row` under the newest machine key, past its head, so the hub
    /// deletes every record (and page) sealed under the keys before it. A
    /// session no longer in the list is not deleted for it — the list may be
    /// short — but kept, marked, and tried again later; the window round
    /// deletes its copy once that has not been written for the whole window.
    async fn reencrypt_stream(&mut self, row: &Row) -> Result<(), String> {
        let result = self.rebuild(&row.stream, row.next_seq + 1).await;
        if let Err(error) = &result {
            if error.contains("the session is gone") {
                tracing::info!(stream = %short(&row.stream), "a session not in the list: keeping its copy, to re-encrypt once it is back");
                self.retry_at.insert(row.stream, Instant::now() + GONE_RETRY);
            }
        }
        result
    }

    /// Re-encrypt whatever is still marked, in scope or not (the list round
    /// only visits recent sessions): a session `listed` (`source/id`) this
    /// round names; one it does not name is kept ([`naming::reencrypt_fate`]).
    async fn reencrypt_round(&mut self, listed: &HashSet<String>) {
        let all = match self.writer.store.try_all() {
            Ok(all) => all,
            Err(error) => {
                tracing::warn!(%error, "cannot read streams.db: nothing is re-encrypted this round");
                return;
            }
        };
        let marked: Vec<StreamId> = self.reencrypt.keys().copied().collect();
        for stream in marked {
            if self.writer.in_flight() >= MAX_IN_FLIGHT || !self.budget.available() {
                break;
            }
            let Some(row) = all.iter().find(|r| r.stream == stream) else {
                self.reencrypt.remove(&stream);
                self.writer.store.clear_reencrypt(&stream);
                continue;
            };
            if self.ready(row) && naming::reencrypt_fate(row, listed) == naming::Fate::Rebuild {
                let _ = self.reencrypt_stream(row).await;
            }
        }
    }

    fn ask_stream_list(&self) {
        if let Some(text) = &self.text {
            let _ = text.send(serde_json::json!({"t": "stream_list"}).to_string());
        }
    }

    /// Ask the hub to delete `stream` (this machine's own), and forget it —
    /// so a session back in scope later is copied again from nothing.
    fn delete_on_hub(&mut self, stream: &StreamId) {
        if let Some(text) = &self.text {
            let _ = text.send(serde_json::json!({"t": "stream_delete", "stream": crate::agent::hub::hex(&stream.0)}).to_string());
        }
        self.forget(stream);
    }

    /// Forget `stream` here (what [`Sync::delete_on_hub`] does after telling the hub).
    fn forget(&mut self, stream: &StreamId) {
        self.reencrypt.remove(stream);
        self.resigned(stream);
        self.seen_sig.remove(stream);
        self.retry_at.remove(stream);
        self.compact_now.remove(stream);
        self.beats.remove(stream);
        if let Some(task) = self.drafts.remove(stream) {
            task.abort();
        }
        self.writer.store.forget(stream);
    }

    // ------------------------------------------------------------ status

    async fn status_body(&mut self) -> Vec<u8> {
        let settings = self.loopback.get_json("/api/settings").await;
        let accounts = self.loopback.get_json("/api/accounts").await;
        let quota = self.loopback.get_json("/api/stats/quota").await;
        let cli = self.cli_versions().await;
        let name = self
            .display_name
            .clone()
            .or_else(|| settings.as_ref().and_then(|s| s["hostname"].as_str()).map(str::to_string));
        // The machine key epoch it writes under: a device compares it with the
        // chain's newest and hands the key over again if this machine lags.
        let mk_epoch = self.mk().map(|(e, _)| e);
        content::with_controllers(
            content::with_changes(
                content::with_alarms(
                    content::with_system(
                        content::status_body(now_ms(), name, mk_epoch, settings, accounts, quota, cli),
                        crate::agent::system::os(),
                        crate::api::SERVER_VERSION,
                    ),
                    &self.alarms,
                ),
                &self.changes,
            ),
            (self.controllers)().as_ref(),
        )
    }

    async fn cli_versions(&mut self) -> Value {
        if let Some((at, v)) = &self.cli {
            if at.elapsed() < Duration::from_secs(60 * 60) {
                return v.clone();
            }
        }
        let v = if crate::env::var("DROPPEN_AGENT_STATUS_CLI").as_deref() == Some("0") {
            Value::Null
        } else {
            serde_json::json!({"claude": cli_version("claude").await, "codex": cli_version("codex").await})
        };
        self.cli = Some((Instant::now(), v.clone()));
        v
    }

    async fn status_round(&mut self) {
        let Some(target) = self.name_target() else { return };
        let Some(mk) = self.writer.key(target).copied() else { return };
        // Under the newest name it may have; an old one goes once this is on the hub.
        let Some(row) = self.named_row(Kind::Status, STATUS_SOURCE, target, &mk) else {
            self.last_status = None;
            return;
        };
        if !self.ready(&row) {
            return;
        }
        if self.legacy_reencrypt(&row.stream) {
            let _ = self.reencrypt_stream(&row).await;
            return;
        }
        let body = self.status_body().await;
        // Unchanged: nothing to write — unless a day went by without a write,
        // which the hub would take for a machine gone (it expires streams by
        // their last content write).
        if !row.cursor.is_empty()
            && content::status_fingerprint(row.cursor.as_bytes()) == content::status_fingerprint(&body)
            && !retention::refresh_due(row.updated_ms, now_ms())
        {
            return;
        }
        let cursor = String::from_utf8(body.clone()).unwrap_or_default();
        self.append(
            &row,
            &[Step {
                fmt: FMT_STATUS_V1,
                body,
                cursor,
                diff: None,
            }],
        );
    }

    // ------------------------------------------------------------ upkeep

    fn heartbeats(&mut self) {
        let now = Instant::now();
        let mut sent = 0;
        for row in self.writer.store.all() {
            if sent >= 20 {
                break;
            }
            if row.next_seq == 0 && row.manifest_upto.is_none() {
                continue;
            }
            if self.beats.get(&row.stream).is_some_and(|t| now.duration_since(*t) < HEARTBEAT_EVERY) || !self.ready(&row) {
                continue;
            }
            if let Some(head) = self.writer.head(&row, now_ms()) {
                self.send(head);
                self.beats.insert(row.stream, now);
                sent += 1;
            }
        }
    }

    async fn compactions(&mut self) {
        if self.writer.in_flight() >= MAX_IN_FLIGHT {
            return;
        }
        let now = now_ms();
        let when = compaction();
        let cutoff = self.trim_cutoff();
        let due = self.writer.store.all().into_iter().find(|row| {
            // An index under an old name takes no more appends and goes soon;
            // checkpointing it would rewrite the acknowledged rows, which are
            // the new one's now.
            let retiring = self.streams2 && row.kind == Kind::Index && self.reencrypt.contains_key(&row.stream);
            self.ready(row)
                && !retiring
                && (when.due(row, now, self.compact_now.contains(&row.stream)) || retention::trim_due(row, cutoff, self.retention_since, now))
        });
        let Some(row) = due else { return };
        self.compact_now.remove(&row.stream);
        let result = match row.kind {
            Kind::Index => {
                let rows = self.writer.store.index_rows();
                self.checkpoint(&row, index_plan(&rows, row.next_seq, false))
            }
            Kind::Status => self.checkpoint(&row, status_plan(row.cursor.as_bytes(), row.next_seq, false)),
            Kind::Session => self.compact_session(&row).await,
        };
        if let Err(error) = result {
            tracing::debug!(stream = %short(&row.stream), %error, "no checkpoint this time");
            self.writer.store.touch_checkpoint(&row.stream, now);
        }
    }

    async fn compact_session(&mut self, row: &Row) -> Result<(), String> {
        let source = source_of(row).ok_or("an unknown session stream")?;
        let summary = self
            .list()
            .await
            .into_iter()
            .find(|s| s.source == source && s.id == row.id)
            .ok_or("the session is gone")?;
        let path = Path::new(&summary.file_path);
        let sig = crate::sessions::file_signature(path).await.ok_or("unreadable")?;
        let transcript = crate::feed::load_transcript(&self.drive.paths, source, path, &sig).await;
        let cursor = SessionCursor::parse(&row.cursor).ok_or("no cursor")?;
        // The pages must be exactly what the records fold to: the transcript
        // still agrees with the copy through its end, and the pages stop there.
        let chain = transcript.chain();
        if cursor
            .end_fp()
            .is_none_or(|fp| chain.get(cursor.end).is_none_or(|ours| encode_fp(ours) != fp))
        {
            return Err("the transcript moved on; the next delta first".into());
        }
        let truncated = Transcript::new(path, Arc::new(transcript.items[..cursor.end].to_vec()));
        let (mut plan, base) = session_plan(
            row,
            &truncated,
            &cursor.sig,
            cursor.start,
            row.next_seq,
            false,
            true,
            self.trim_cutoff(),
            self.epoch(),
        );
        // The same cursor, but for where the copy now starts (the retention
        // window may have moved it on).
        plan.cursor = SessionCursor { start: base, ..cursor }.encode();
        self.checkpoint(row, plan)
    }

    // ------------------------------------------------------------ drafts

    fn watch(&mut self, streams: HashSet<StreamId>) {
        for gone in self.watched.difference(&streams).copied().collect::<Vec<_>>() {
            if let Some(task) = self.drafts.remove(&gone) {
                task.abort();
            }
        }
        self.watched = streams;
        let Some((epoch, mk)) = self.mk() else { return };
        let Some(out) = self.out.clone() else { return };
        for stream in self.watched.clone() {
            if self.drafts.contains_key(&stream) {
                continue;
            }
            let Some((source, id)) = self.names.get(&stream).cloned() else {
                continue;
            };
            if self.writer.store.get(&stream).is_none() {
                continue;
            }
            let Some(mut rx) = self.drive.injections.draft_watch(source, &id) else {
                continue;
            };
            let key = stream_key(&mk, &stream);
            let signing = self.writer.signing().clone();
            let out = out.clone();
            self.drafts.insert(
                stream,
                tokio::spawn(async move {
                    let mut last_v = u64::MAX;
                    while rx.changed().await.is_ok() {
                        tokio::time::sleep(DRAFT_EVERY).await;
                        let draft = rx.borrow_and_update().clone();
                        if draft.v == last_v {
                            continue;
                        }
                        last_v = draft.v;
                        let mut body = serde_json::to_value(&draft).unwrap_or(Value::Null);
                        body["type"] = Value::String("draft".into());
                        let Ok(bytes) = serde_json::to_vec(&body) else { continue };
                        if bytes.len() > content::BODY_BUDGET {
                            continue;
                        }
                        let Ok(payload) = seal_ephemeral(&key, &stream, epoch, &bytes, &signing, &mut rand::rngs::OsRng) else {
                            continue;
                        };
                        if out.send(Frame::Ephemeral { ch: 0, stream, payload }).is_err() {
                            break;
                        }
                    }
                }),
            );
        }
    }
}

/// The pages of a session's copy, standing at `upto`: its items from
/// `start` on — or from later, past the entries older than `cutoff` (the
/// retention window), after a window-start page
/// ([`content::window_body`]) — reusing the last manifest's unchanged
/// pages when `reuse`. A page all of whose entries the window leaves out
/// is dropped from the head; every later one is kept as it is (pages are
/// content-addressed: nothing is uploaded again), so the copy may start a
/// little before `cutoff`. Also: the item the copy now starts at.
/// `epoch`: the machine key new pages are sealed under (pages of another
/// are not reused).
#[allow(clippy::too_many_arguments)]
fn session_plan(
    row: &Row,
    transcript: &Transcript,
    sig: &str,
    start: usize,
    upto: u64,
    fresh_link: bool,
    reuse: bool,
    cutoff: Option<u64>,
    epoch: Option<u32>,
) -> (Plan, usize) {
    let chain = transcript.chain();
    let items = &transcript.items;
    let len = transcript.len();
    let times = content::item_times(items);
    let k = cutoff.map_or(0, |c| retention::window_start(&times, c)).max(start).min(len);
    let same_epoch = epoch == Some(row.epoch);
    let old: Vec<&PageRef> = if reuse && same_epoch {
        row.pages.iter().filter(|p| !p.window).collect()
    } else {
        Vec::new()
    };
    let first = retention::first_kept(&old, k);
    let base = match old.get(first) {
        Some(p) if p.from >= start && p.from <= k => p.from,
        _ => k,
    };
    let mut pages = Vec::new();
    let mut state_bytes = 0i64;
    if base > content::session_start(len) {
        // The retention window, not the item cap, says where the copy
        // starts: the window-start page goes first.
        let fp = chain.get(base).map(encode_fp).unwrap_or_default();
        match row.pages.iter().find(|p| p.window) {
            Some(p) if reuse && same_epoch && p.from == base && p.fp == fp => pages.push(PageSpec::Reuse(p.clone())),
            _ => {
                let body = content::window_body(base, retention::newest(&times[..base]), chain);
                state_bytes += body.len() as i64;
                pages.push(PageSpec::New {
                    body,
                    from: base,
                    to: base,
                    fp,
                    last_ms: None,
                    window: true,
                });
            }
        }
    }
    let mut at = base;
    for page in &old[first..] {
        let unchanged = page.from == at && page.to <= len && page.to > page.from && chain.get(page.to).is_some_and(|fp| encode_fp(fp) == page.fp);
        if !unchanged {
            break;
        }
        pages.push(PageSpec::Reuse((*page).clone()));
        at = page.to;
    }
    let mut from = at;
    for (to, body) in content::session_bodies(items, at, len, chain) {
        state_bytes += body.len() as i64;
        pages.push(PageSpec::New {
            body,
            from,
            to,
            fp: chain.get(to).map(encode_fp).unwrap_or_default(),
            last_ms: retention::newest(&times[from.min(to)..to]),
            window: false,
        });
        from = to;
    }
    let plan = Plan {
        fmt: FMT_FEED_V1,
        pages,
        upto,
        fresh_link,
        cursor: SessionCursor::at(base, len, chain, sig).encode(),
        state_bytes,
        rows: None,
    };
    (plan, base)
}

fn session_steps(transcript: &Transcript, start: usize, from: usize, len: usize, chain: &[crate::feed::Fp], sig: &str) -> Vec<Step> {
    content::session_bodies(&transcript.items, from, len, chain)
        .into_iter()
        .map(|(end, body)| Step {
            fmt: FMT_FEED_V1,
            body,
            cursor: SessionCursor::at(start, end, chain, sig).encode(),
            diff: None,
        })
        .collect()
}

fn index_plan(rows: &BTreeMap<String, String>, upto: u64, fresh_link: bool) -> Plan {
    let all: Vec<(String, String)> = rows.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let bodies = content::index_bodies(&all, &[], true, now_ms());
    let state_bytes = bodies.iter().map(|(b, ..)| b.len() as i64).sum();
    Plan {
        fmt: FMT_INDEX_V1,
        pages: bodies.into_iter().map(|(body, ..)| PageSpec::body(body)).collect(),
        upto,
        fresh_link,
        cursor: String::new(),
        state_bytes,
        rows: Some(all),
    }
}

fn status_plan(body: &[u8], upto: u64, fresh_link: bool) -> Plan {
    Plan {
        fmt: FMT_STATUS_V1,
        pages: if body.is_empty() {
            Vec::new()
        } else {
            vec![PageSpec::body(body.to_vec())]
        },
        upto,
        fresh_link,
        cursor: String::from_utf8_lossy(body).into_owned(),
        state_bytes: body.len() as i64,
        rows: None,
    }
}

/// `<cli> --version`, first line, if it answers within a few seconds.
async fn cli_version(cli: &str) -> Option<String> {
    let run = tokio::process::Command::new(cli)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(5), run).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?.trim();
    (!line.is_empty() && line.len() < 100).then(|| line.to_string())
}

/// Where a session's copy starts: past the newest [`content::MAX_ITEMS`],
/// and past the entries older than `cutoff` (the retention window) — and
/// whether it is the window that says so.
fn copy_start(items: &[crate::sessions::util::TranscriptItem], cutoff: Option<u64>) -> (usize, bool) {
    let cap = content::session_start(items.len());
    let k = cutoff.map_or(0, |c| retention::window_start(&content::item_times(items), c));
    (cap.max(k), k > cap)
}

/// An ISO-8601 time as ms since the epoch.
fn iso_ms(stamp: &str) -> Option<u64> {
    crate::schedule::parse_iso(stamp)
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

fn ms_iso(ms: u64) -> String {
    crate::sessions::iso8601(std::time::UNIX_EPOCH + Duration::from_millis(ms))
}

/// The CLI a session stream's row is for.
fn source_of(row: &Row) -> Option<Source> {
    serde_json::from_value(Value::String(row.source.clone())).ok()
}

fn short(stream: &StreamId) -> String {
    super::hub::hex(&stream.0[..4])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Host identity renewal (§5.4): the writer remembers which key it signs
    /// with; started with another — a renewed key — every stream with a
    /// manifest is to be signed again, and that list outlives a restart
    /// until each is done.
    #[test]
    fn a_renewed_signing_key_marks_every_manifest_to_be_signed_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(&dir.path().join("streams.db"));
        let with = StreamId([1; 32]);
        let without = StreamId([2; 32]);
        store.create(&with, Kind::Session, "claude", "a", 0, None, 1);
        store.create(&without, Kind::Session, "claude", "b", 0, None, 1);
        store.commit_checkpoint(&with, 0, 2, &[0; 32], "c", &[], 0, None, 1).unwrap();
        let (old, new) = ([7; 32], [8; 32]);
        assert!(manifests_to_resign(&store, &old).is_empty(), "the first key it signs with");
        assert!(manifests_to_resign(&store, &old).is_empty(), "the same key again");
        let marked = manifests_to_resign(&store, &new);
        assert_eq!(marked, HashSet::from([with]), "only the stream with a manifest");
        assert_eq!(manifests_to_resign(&store, &new), marked, "kept across a restart");
        save_resign(&store, &HashSet::new());
        assert!(manifests_to_resign(&store, &new).is_empty(), "done");
    }

    fn row(tail_records: u64, updated_ms: u64, tail_since_ms: Option<u64>, checkpoint_ms: Option<u64>) -> Row {
        Row {
            stream: StreamId([1; 32]),
            kind: Kind::Session,
            source: "claude".into(),
            id: "s".into(),
            epoch: 0,
            name_epoch: None,
            next_seq: tail_records,
            link: [0; 32],
            cursor: String::new(),
            pending: None,
            manifest_upto: None,
            pages: Vec::new(),
            tail_records,
            tail_bytes: 100,
            state_bytes: 1000,
            checkpoint_ms,
            tail_since_ms,
            updated_ms,
        }
    }

    /// M2 re-review: the writer sealed under the newest epoch in
    /// `machine-keys.json`, whatever the chain said; now only committed keys count.
    #[test]
    fn only_keys_the_chain_committed_reach_the_writer() {
        let keys: BTreeMap<u32, zeroize::Zeroizing<[u8; 32]>> = [(0, [1u8; 32]), (1, [2; 32]), (5, [9; 32])]
            .into_iter()
            .map(|(e, k)| (e, zeroize::Zeroizing::new(k)))
            .collect();
        let check: KeyCheck = Arc::new(|epoch, key: &[u8; 32]| (epoch == 0 && key == &[1; 32]) || (epoch == 1 && key == &[2; 32]));
        let kept = committed_only(keys, &check);
        assert_eq!(
            kept.keys().copied().collect::<Vec<_>>(),
            vec![0, 1],
            "epoch 5 (a leftover) is not sealed under"
        );
        let dir = tempfile::tempdir().unwrap();
        let mut writer = Writer::new(Store::at(&dir.path().join("streams.db")), ed25519_dalek::SigningKey::from_bytes(&[3; 32]));
        for (epoch, key) in kept {
            writer.add_key(epoch, key);
        }
        assert_eq!(writer.current().map(|(e, _)| e), Some(1));
    }

    const DAY: u64 = retention::DAY_MS;

    /// `n` items of about 8 KB, the first `old` dated `old_ms`, the rest `new_ms`.
    fn dated_items(n: usize, old: usize, old_ms: u64, new_ms: u64) -> Vec<crate::sessions::util::TranscriptItem> {
        (0..n)
            .map(|i| {
                let at = ms_iso(if i < old { old_ms } else { new_ms });
                serde_json::from_value(serde_json::json!({
                    "role": "assistant", "kind": "text", "text": format!("{i}:{}", "x".repeat(8000)),
                    "detail": null, "toolName": null, "meta": null, "isError": false, "question": null,
                    "timestamp": at, "truncated": false,
                }))
                .unwrap()
            })
            .collect()
    }

    /// The page refs a writer would keep for `plan` (a new page named
    /// `p<from>-<to>`, a window page `w<from>`).
    fn refs(plan: &Plan) -> Vec<PageRef> {
        plan.pages
            .iter()
            .map(|spec| match spec {
                PageSpec::Reuse(r) => r.clone(),
                PageSpec::New {
                    body,
                    from,
                    to,
                    fp,
                    last_ms,
                    window,
                } => PageRef {
                    hash: if *window { format!("w{from}") } else { format!("p{from}-{to}") },
                    from: *from,
                    to: *to,
                    fp: fp.clone(),
                    epoch: 0,
                    window: *window,
                    last_ms: *last_ms,
                    size: body.len() as u64,
                },
            })
            .collect()
    }

    fn kinds(plan: &Plan) -> Vec<String> {
        plan.pages
            .iter()
            .map(|p| match p {
                PageSpec::Reuse(r) => format!("reuse {}", r.hash),
                PageSpec::New { from, to, window: true, .. } => format!("window {from}..{to}"),
                PageSpec::New { from, to, .. } => format!("new {from}..{to}"),
            })
            .collect()
    }

    fn body_of(plan: &Plan, i: usize) -> Value {
        match &plan.pages[i] {
            PageSpec::New { body, .. } => serde_json::from_slice(body).unwrap(),
            PageSpec::Reuse(r) => panic!("page {i} is reused ({})", r.hash),
        }
    }

    /// Relay-and-plans §3.4.2: a long live session's checkpoint drops the
    /// pages all of whose entries are older than the window, keeps the rest
    /// as they were, and starts with a window-start page.
    #[test]
    fn a_long_live_session_drops_its_old_pages_and_starts_with_a_window_page() {
        let now = 100 * DAY;
        let old_ms = now - 20 * DAY;
        let path = std::path::Path::new("/tmp/long.jsonl");
        // 30 items, 0..20 three weeks old, 20..30 an hour old; 7 to a page.
        let transcript = Transcript::new(path, Arc::new(dated_items(30, 20, old_ms, now - 3_600_000)));
        let chain = transcript.chain().clone();
        let mut r = row(0, 1, None, None);
        r.manifest_upto = Some(0);

        // Before any plan: the whole of it, no window page.
        let (plan, base) = session_plan(&r, &transcript, "sig", 0, 0, true, false, None, Some(0));
        assert_eq!(base, 0);
        assert_eq!(kinds(&plan), ["new 0..7", "new 7..14", "new 14..21", "new 21..28", "new 28..30"]);
        r.pages = refs(&plan);
        assert_eq!(r.pages[0].last_ms, Some(old_ms), "each page knows its newest entry");
        assert_eq!(r.pages[2].last_ms, Some(now - 3_600_000));
        assert!(retention::trim_due(&r, retention::trim_cutoff(Some(&plan_of(7)), now), 0, now));

        // A 7-day plan: the two pages wholly older go, the page holding item 20
        // stays whole (so the copy starts at 14), nothing kept is uploaded again.
        let cutoff = Some(now - 7 * DAY);
        let (plan, base) = session_plan(&r, &transcript, "sig", 0, 3, false, true, cutoff, Some(0));
        assert_eq!(base, 14);
        assert_eq!(
            kinds(&plan),
            ["window 14..14", "reuse p14-21", "reuse p21-28", "reuse p28-30", "new 30..30"]
        );
        let window = body_of(&plan, 0);
        assert_eq!(window["fromIndex"], 14);
        assert_eq!(window["endIndex"], 14);
        assert_eq!(window["items"], serde_json::json!([]));
        assert_eq!(window["base"], 14);
        assert_eq!(window["before"], ms_iso(old_ms), "the newest entry left out");
        assert_eq!(window["fp"], encode_fp(&chain[14]));
        assert_eq!(SessionCursor::parse(&plan.cursor).unwrap().start, 14);

        // The next checkpoint reuses the window page too; so does one without
        // a plan any more (the copy does not grow back by itself).
        r.pages = refs(&plan);
        for cutoff in [cutoff, None] {
            let (again, base) = session_plan(&r, &transcript, "sig", 14, 3, false, true, cutoff, Some(0));
            assert_eq!(base, 14);
            assert_eq!(kinds(&again)[..4], ["reuse w14", "reuse p14-21", "reuse p21-28", "reuse p28-30"]);
        }
        // A new epoch reuses nothing.
        let (other, _) = session_plan(&r, &transcript, "sig", 14, 3, false, true, cutoff, Some(1));
        assert!(other.pages.iter().all(|p| matches!(p, PageSpec::New { .. })));

        // A rebuild (or a first upload) trims to the item: the copy starts at 20.
        let (rebuilt, base) = session_plan(&r, &transcript, "sig", 0, 9, true, false, cutoff, Some(0));
        assert_eq!(base, 20);
        assert_eq!(kinds(&rebuilt), ["window 20..20", "new 20..27", "new 27..30"]);
        assert_eq!(body_of(&rebuilt, 0)["base"], 20);
        assert_eq!(copy_start(&transcript.items, cutoff), (20, true));
        assert_eq!(copy_start(&transcript.items, None), (0, false));
        // A session all within the window is not touched.
        let young = Transcript::new(path, Arc::new(dated_items(10, 0, 0, now)));
        let (plan, base) = session_plan(&r, &young, "sig", 0, 0, true, false, cutoff, Some(0));
        assert_eq!(base, 0);
        assert!(!kinds(&plan).iter().any(|k| k.starts_with("window")));
    }

    fn plan_of(days: u64) -> HubPlan {
        HubPlan {
            plan: "free".into(),
            retention_days: days,
            storage_bytes: None,
            usage_bytes: 0,
            machine_usage_bytes: None,
            grace_until_ms: None,
            received_ms: 0,
        }
    }

    #[test]
    fn tails_are_compacted_when_big_idle_old_or_asked_for() {
        let min = 60_000;
        let when = Compaction {
            records: 128,
            every: COMPACT_EVERY,
            idle: COMPACT_IDLE,
            oldest: COMPACT_OLDEST,
        };
        let now = 100 * min;
        // A small tail written to a minute ago: not yet.
        assert!(!when.due(&row(3, now - min, Some(now - 2 * min), None), now, false));
        // Idle five minutes.
        assert!(when.due(&row(3, now - 5 * min, Some(now - 6 * min), None), now, false));
        // Busy, but its oldest record is half an hour old.
        assert!(when.due(&row(3, now - 1000, Some(now - 30 * min), None), now, false));
        // Big.
        assert!(when.due(&row(128, now, Some(now), None), now, false));
        // Not twice within a minute (a failing checkpoint is not retried every tick)…
        assert!(!when.due(&row(3, now - 5 * min, Some(now - 6 * min), Some(now - 1000)), now, false));
        // …unless the hub refused an append for it.
        assert!(when.due(&row(3, now, Some(now), Some(now - 1000)), now, true));
        // Nothing to compact.
        assert!(!when.due(&row(0, now - 60 * min, None, None), now, false));
    }
}
