//! The other devices' copies as this computer holds them in memory, for the
//! local gateway (local-aggregator §6.3, S1-7): what a page on this computer
//! — the local app, a phone on the LAN — reads of the devices it may see.
//!
//! The copies task ([`super::copies`]) folds every stream it follows into a
//! [`Slot`] of its own and **publishes** the slot here once its catch-up is
//! over (the floor checked). From then on every batch it releases is folded
//! into the slot and handed to the slot's subscribers under the same lock,
//! at once: the ten-second throttle on session copies is `copies.db`'s, never
//! the page's. A subscriber registers under that lock too and takes its
//! snapshot there, so it gets the state at some point and then exactly what
//! came after it — nothing twice, nothing missed. A stream read again (an
//! error, new keys, a newer epoch) folds into a new slot and only replaces
//! the published one once it has caught up; until then the page keeps the old
//! one. An index or a status never goes back to an older epoch than the one
//! published (as the store's own rule).
//!
//! What `copies.db` holds is loaded here when the store opens ([`preload`]):
//! a hub that is down at start still leaves the last copies, with how current
//! they are. Nothing on the gateway's path reads the disk.
//!
//! Locks: a slot, then the registry, never the other way round. The
//! directory ([`DirHost`]) is worked out from the registry alone — the status
//! and index fields it needs are noted at publish time — so it never takes a
//! slot.
//!
//! The sessions a page opens are interest ([`LiveCopies::opened`]): the copies
//! task follows them ahead of each host's newest, and for [`LINGER`] after the
//! last page let go.
//!
//! [`preload`]: LiveCopies::preload

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, watch, Notify};

use super::copies::Fold;
use super::store::Kind;

/// How long a session a page let go of stays followed.
pub const LINGER: Duration = Duration::from_secs(5 * 60);
/// The most sessions pages' interest is kept for: past it, the oldest let go
/// of goes first; with none let go of, a new one adds none.
pub const INTEREST_MAX: usize = 64;

/// One followed stream's copy as folded, and where it stands.
pub struct Published {
    pub(super) fold: Fold,
    /// The sequence number the stream's next record will carry.
    pub(super) seq: u64,
    /// How current it is: the newest machine-signed head time seen (ms).
    pub(super) time_ms: u64,
    /// The machine-key epoch its stream is named under (set when published):
    /// with `seq`, where a page's copy stands.
    pub(super) epoch: u32,
    /// The host identity key its stream task verifies heads under, when one
    /// does: never published, nor delivered, once the host
    /// renewed it ([`LiveCopies::set_signers`]).
    pub(super) signer: Option<[u8; 32]>,
}

impl Published {
    pub(super) fn new(fold: Fold, seq: u64, time_ms: u64) -> Self {
        Self {
            fold,
            seq,
            time_ms,
            epoch: 0,
            signer: None,
        }
    }

    /// [`Published::new`], for a stream task verifying under `signer`.
    pub(super) fn signed_by(mut self, signer: [u8; 32]) -> Self {
        self.signer = Some(signer);
        self
    }
}

/// A stream's copy, shared by its follow task and the gateway.
pub type Slot = Arc<Mutex<Published>>;

/// Which copy: a host's index, its status, or one of its sessions (`source/id`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key {
    /// The device it is of.
    pub host: String,
    /// What it copies.
    pub kind: Kind,
    /// `source/id` for a session; empty otherwise.
    pub name: String,
}

impl Key {
    /// `host`'s copy of `kind` (`name`: a session's `source/id`, else empty).
    pub fn new(host: &str, kind: Kind, name: &str) -> Self {
        Self {
            host: host.to_string(),
            kind,
            name: name.to_string(),
        }
    }
}

/// One gateway connection's way out: whole text frames, a queue bounded by
/// count and by bytes. A subscription whose frame does not fit is dropped and
/// said to be overrun ([`Outbox::take_overrun`]): its page subscribes again
/// for a fresh snapshot.
pub struct Outbox {
    tx: mpsc::Sender<String>,
    overrun: Mutex<BTreeSet<u32>>,
    /// Subscriptions the registry ended (their device left, a session too
    /// large): the connection lets go of them ([`Outbox::take_ended`]).
    ended: Mutex<BTreeSet<u32>>,
    wake: Notify,
    /// Bytes queued and not taken yet ([`Outbox::taken`]).
    bytes: std::sync::atomic::AtomicUsize,
    max_bytes: usize,
}

impl Outbox {
    /// An outbox of `capacity` frames and `max_bytes`, and its receiving end.
    pub fn new(capacity: usize, max_bytes: usize) -> (Arc<Self>, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Arc::new(Self {
                tx,
                overrun: Mutex::new(BTreeSet::new()),
                ended: Mutex::new(BTreeSet::new()),
                wake: Notify::new(),
                bytes: std::sync::atomic::AtomicUsize::new(0),
                max_bytes,
            }),
            rx,
        )
    }

    /// Queue `text` for subscription `s`: `false` when it did not fit (or the
    /// connection is gone) — then `s` is overrun.
    pub fn send(&self, s: u32, text: String) -> bool {
        use std::sync::atomic::Ordering;
        let len = text.len();
        // Counted first, then checked (and given back if it does not go): two
        // senders at once cannot both fit in room for one.
        let queued = self.bytes.fetch_add(len, Ordering::AcqRel);
        // One frame larger than the budget still goes when nothing waits.
        let fits = queued == 0 || queued.saturating_add(len) <= self.max_bytes;
        let sent = if fits {
            self.tx.try_send(text)
        } else {
            Err(mpsc::error::TrySendError::Full(text))
        };
        if sent.is_err() {
            self.bytes.fetch_sub(len, Ordering::AcqRel);
        }
        match sent {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.overrun.lock().expect("overrun").insert(s);
                self.wake.notify_one();
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// A frame of `len` bytes was taken off the queue.
    pub fn taken(&self, len: usize) {
        use std::sync::atomic::Ordering;
        let _ = self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |b| Some(b.saturating_sub(len)));
    }

    /// Subscription `s` ended here (`frame` says why): the connection lets go of it.
    fn end(&self, s: u32, frame: String) {
        self.ended.lock().expect("ended").insert(s);
        // A frame that does not fit is an overrun: the page asks again and hears why then.
        self.send(s, frame);
        self.wake.notify_one();
    }

    /// The subscriptions the registry ended since the last call.
    pub fn take_ended(&self) -> Vec<u32> {
        std::mem::take(&mut *self.ended.lock().expect("ended")).into_iter().collect()
    }

    /// The subscriptions dropped for want of room since the last call.
    pub fn take_overrun(&self) -> Vec<u32> {
        std::mem::take(&mut *self.overrun.lock().expect("overrun")).into_iter().collect()
    }

    /// Resolves when a subscription was dropped for want of room, or ended.
    pub async fn overrun(&self) {
        self.wake.notified().await;
    }
}

struct Subscriber {
    id: u64,
    s: u32,
    out: Arc<Outbox>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    /// Not known yet.
    #[default]
    Pending,
    /// The host has no such stream (yet).
    Missing,
    /// A session over the size limit: not kept.
    TooLarge,
}

/// What the directory needs of a host's status, noted when it is published.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StatusInfo {
    name: Option<String>,
    os: Option<String>,
    agent_version: Option<String>,
}

#[derive(Default)]
struct Entry {
    slot: Option<Slot>,
    /// The epoch the published slot's stream is named under.
    epoch: u32,
    state: State,
    subs: Vec<Subscriber>,
    /// The published slot's time (an index: how current the host's copy is).
    time_ms: Option<u64>,
    /// A status's directory fields.
    info: Option<StatusInfo>,
}

#[derive(Debug, Clone, Copy)]
struct Presence {
    online: Option<bool>,
    since_ms: u64,
}

struct Interest {
    /// The subscriptions holding it (their ids): a handle the registry has
    /// forgotten (its device left, everything was cleared) takes nothing away
    /// from a later one.
    subs: BTreeSet<u64>,
    /// When it was last subscribed to or let go of (ms).
    last_ms: u64,
}

#[derive(Default)]
struct Inner {
    hosts: BTreeMap<String, Presence>,
    entries: HashMap<Key, Entry>,
    interest: BTreeMap<(String, String), Interest>,
    next_id: u64,
    /// Each directory made is numbered, in the order the changes happened
    /// (under this lock): one made earlier never goes out after a later one.
    dir_version: u64,
    /// Each followed host's identity key now ([`LiveCopies::set_signers`]):
    /// what a stream task's copy must be verified under to be shown.
    signers: BTreeMap<String, [u8; 32]>,
}

/// Whether a copy verified under `signer` (`None`: no task's — read back
/// from the store) may be shown for `host`: only under the host's identity
/// key now. A stream task of a host that renewed it since — cancelled, but a
/// ready frame may have won its race with the cancel — shows nothing, and
/// neither does one before the copies task said which key that is.
fn signer_holds(inner: &Inner, host: &str, signer: Option<[u8; 32]>) -> bool {
    match signer {
        Some(signer) => inner.signers.get(host) == Some(&signer),
        None => true,
    }
}

/// A directory made under the registry's lock, and its place in line.
struct Dir {
    version: u64,
    hosts: Vec<DirHost>,
}

/// A device in the gateway's directory (`dir`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirHost {
    /// Its device id.
    pub id: String,
    /// Its name, as its status says it (`null`: it said none).
    pub name: Option<String>,
    /// `online`, `offline`, or `unknown` (the hub has not said, or this
    /// computer is not connected to it).
    pub state: &'static str,
    /// Since when (ms, this computer's clock) it has been in `state`.
    pub since_ms: Option<u64>,
    /// How current its copy is: the newest time it signed its index (ms).
    pub copy_as_of_ms: Option<u64>,
    /// Its system, as its status says (`macOS 15.1 arm64`).
    pub os: Option<String>,
    /// Its Droppen version.
    pub agent_version: Option<String>,
}

/// Why a subscription is refused or ended (`copy_error`'s `code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The device is not (any more) one this computer follows.
    Gone,
    /// A session over the size limit: not kept here.
    TooLarge,
}

impl Ended {
    /// As `copy_error` says it.
    pub fn code(self) -> &'static str {
        match self {
            Ended::Gone => "gone",
            Ended::TooLarge => "too_large",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Ended::Gone => "this device no longer shows that device's copies",
            Ended::TooLarge => "this session's copy is too large to keep on this device",
        }
    }
}

/// `copy_error` for subscription `s`.
pub fn error_frame(s: u32, code: &str, message: &str) -> String {
    serde_json::json!({"t": "copy_error", "s": s, "code": code, "message": message}).to_string()
}

fn missing_frame(s: u32) -> String {
    format!(r#"{{"t":"missing","s":{s}}}"#)
}

/// `copy` for subscription `s`: `bodies` is a JSON array's text; `epoch` and
/// `seq` say where the copy stands (a page resubscribing says them back).
fn copy_frame(s: u32, reset: bool, bodies: &str, published: &Published) -> String {
    let reset = if reset { r#""reset":true,"# } else { "" };
    let (seq, epoch, time_ms) = (published.seq, published.epoch, published.time_ms);
    format!(r#"{{"t":"copy","s":{s},{reset}"bodies":{bodies},"headSeq":{seq},"epoch":{epoch},"headTimeMs":{time_ms}}}"#)
}

/// Released bodies as a JSON array's text (each is JSON its fold already took).
fn bodies_text(bodies: &[(u16, Vec<u8>)]) -> String {
    let mut out = String::from("[");
    for (i, (_, body)) in bodies.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&String::from_utf8_lossy(body));
    }
    out.push(']');
    out
}

/// A system as the status stream says it (S1-5's `os`), or the settings' platform.
fn os_of(status: &Value) -> Option<String> {
    let os = &status["os"];
    let pretty = match os["name"].as_str() {
        Some("macos") => Some("macOS"),
        Some("linux") => os["distro"].as_str().or(Some("Linux")),
        Some("windows") => Some("Windows"),
        Some(other) => Some(other),
        None => None,
    };
    if let Some(name) = pretty {
        let parts: Vec<&str> = [Some(name), os["version"].as_str(), os["arch"].as_str()]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty())
            .collect();
        return Some(parts.join(" "));
    }
    match status["settings"]["platform"].as_str() {
        Some("darwin") => Some("macOS".into()),
        Some(p) if !p.is_empty() => Some(p.to_string()),
        _ => None,
    }
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(200).collect())
}

fn status_info(status: &Value) -> StatusInfo {
    StatusInfo {
        name: text(&status["displayName"]),
        os: os_of(status),
        agent_version: text(&status["agentVersion"]).or_else(|| text(&status["settings"]["serverVersion"])),
    }
}

/// The registry. One per server, on its link's handle
/// ([`crate::agent::control::Control`]): it outlives a link that stops and
/// starts again.
pub struct LiveCopies {
    inner: Mutex<Inner>,
    dir: watch::Sender<Arc<Vec<DirHost>>>,
    /// The version of the directory last sent (read and moved only inside
    /// the watch's own lock, [`LiveCopies::send_dir`]).
    dir_sent: std::sync::atomic::AtomicU64,
    interest: watch::Sender<u64>,
}

impl Default for LiveCopies {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            dir: watch::channel(Arc::new(Vec::new())).0,
            dir_sent: std::sync::atomic::AtomicU64::new(0),
            interest: watch::channel(0).0,
        }
    }
}

impl LiveCopies {
    /// An empty registry.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // ------------------------------------------------------------ the directory

    /// The directory, as it changes.
    pub fn dir(&self) -> watch::Receiver<Arc<Vec<DirHost>>> {
        self.dir.subscribe()
    }

    /// Whether `host` is in the directory now.
    pub fn has_host(&self, host: &str) -> bool {
        self.lock().hosts.contains_key(host)
    }

    /// The directory as `inner` has it, numbered — sent with
    /// [`Self::send_dir`] once the registry's lock is let go: a gateway
    /// reading the directory may be letting go of subscriptions (which takes
    /// that lock) meanwhile.
    fn dir_of(inner: &mut Inner) -> Dir {
        inner.dir_version += 1;
        let hosts = inner
            .hosts
            .iter()
            .map(|(id, presence)| {
                let entry = |kind| inner.entries.get(&Key::new(id, kind, ""));
                let info = entry(Kind::Status).and_then(|e| e.info.clone()).unwrap_or_default();
                DirHost {
                    id: id.clone(),
                    name: info.name,
                    state: match presence.online {
                        Some(true) => "online",
                        Some(false) => "offline",
                        None => "unknown",
                    },
                    since_ms: (presence.since_ms > 0).then_some(presence.since_ms),
                    copy_as_of_ms: entry(Kind::Index).and_then(|e| e.time_ms),
                    os: info.os,
                    agent_version: info.agent_version,
                }
            })
            .collect();
        Dir {
            version: inner.dir_version,
            hosts,
        }
    }

    /// Say the directory, if it changed — and only if no later one went out
    /// already (made under the registry's lock after this one but sent
    /// before it: a device that just left must not come back). Never with
    /// the registry's lock held.
    fn send_dir(&self, dir: Dir) {
        use std::sync::atomic::Ordering;
        let Dir { version, hosts } = dir;
        self.dir.send_if_modified(|held| {
            // Inside the watch's lock: one sender at a time reads and moves the version.
            if version <= self.dir_sent.load(Ordering::Acquire) {
                return false;
            }
            self.dir_sent.store(version, Ordering::Release);
            if **held == hosts {
                return false;
            }
            *held = Arc::new(hosts);
            true
        });
    }

    /// The hosts followed now: the others' copies go, and their subscribers
    /// are told so.
    pub fn set_hosts(&self, ids: &[String]) {
        let mut inner = self.lock();
        inner.hosts.retain(|id, _| ids.contains(id));
        for id in ids {
            inner.hosts.entry(id.clone()).or_insert(Presence { online: None, since_ms: 0 });
        }
        let gone: Vec<Key> = inner.entries.keys().filter(|k| !ids.contains(&k.host)).cloned().collect();
        for key in gone {
            if let Some(entry) = inner.entries.remove(&key) {
                end_all(entry.subs, Ended::Gone);
            }
        }
        inner.interest.retain(|(host, _), _| ids.contains(host));
        let dir = Self::dir_of(&mut inner);
        drop(inner);
        self.send_dir(dir);
    }

    /// Each followed host's identity key now: set before
    /// the stream tasks of a host that renewed it are cancelled, so a task
    /// verifying under the old key shows nothing from then on — not by its
    /// own publication, nor by delivering to one published before.
    pub fn set_signers(&self, keys: &[(String, [u8; 32])]) {
        let mut inner = self.lock();
        inner.signers = keys.iter().cloned().collect();
    }

    /// Whether `host` is online (`None`: not known).
    pub fn set_presence(&self, host: &str, online: Option<bool>) {
        let mut inner = self.lock();
        let Some(presence) = inner.hosts.get_mut(host) else { return };
        if presence.online == online {
            return;
        }
        *presence = Presence { online, since_ms: crate::agent::now_ms() };
        let dir = Self::dir_of(&mut inner);
        drop(inner);
        self.send_dir(dir);
    }

    /// Nothing is known of anyone's presence now (the connection to the hub went).
    pub fn presence_unknown(&self) {
        let mut inner = self.lock();
        let now = crate::agent::now_ms();
        let mut changed = false;
        for presence in inner.hosts.values_mut() {
            if presence.online.is_some() {
                *presence = Presence { online: None, since_ms: now };
                changed = true;
            }
        }
        if changed {
            let dir = Self::dir_of(&mut inner);
            drop(inner);
            self.send_dir(dir);
        }
    }

    /// Forget everything: the aggregator is off, the link stopped, this
    /// computer left its account. Every subscriber is told its device is gone.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.signers.clear();
        if inner.hosts.is_empty() && inner.entries.is_empty() && inner.interest.is_empty() {
            return;
        }
        for (_, entry) in inner.entries.drain() {
            end_all(entry.subs, Ended::Gone);
        }
        inner.hosts.clear();
        inner.interest.clear();
        let dir = Self::dir_of(&mut inner);
        drop(inner);
        self.send_dir(dir);
    }

    // ------------------------------------------------------------ the copies task's side

    /// A copy read back from `copies.db` when the store opens: published
    /// unless something newer already is.
    pub fn preload(&self, key: &Key, epoch: u32, slot: &Slot) -> bool {
        self.put(key, epoch, slot, true)
    }

    /// A stream's catch-up is over: `slot` is what the page sees of `key` from
    /// now on, and every subscriber starts again from it. Refused (`false`)
    /// for a host not followed, and — an index or a status — for an older
    /// epoch than the one published.
    pub fn publish(&self, key: &Key, epoch: u32, slot: &Slot) -> bool {
        self.put(key, epoch, slot, false)
    }

    fn put(&self, key: &Key, epoch: u32, slot: &Slot, only_if_empty: bool) -> bool {
        // The slot is held throughout: nobody subscribes to it, and nothing is
        // delivered on it, until its subscribers have their snapshot.
        let mut published = slot.lock().expect("slot");
        let waiting = {
            let mut inner = self.lock();
            if !inner.hosts.contains_key(&key.host) || !signer_holds(&inner, &key.host, published.signer) {
                return false;
            }
            let entry = inner.entries.entry(key.clone()).or_default();
            if only_if_empty && (entry.slot.is_some() || entry.state == State::TooLarge) {
                return false;
            }
            if key.kind != Kind::Session && entry.slot.is_some() && entry.epoch > epoch {
                return false;
            }
            published.epoch = epoch;
            entry.slot = Some(slot.clone());
            entry.epoch = epoch;
            entry.state = State::Pending;
            note(entry, key.kind, &published);
            let waiting = !entry.subs.is_empty();
            let dir = (key.kind != Kind::Session).then(|| Self::dir_of(&mut inner));
            drop(inner);
            if let Some(dir) = dir {
                self.send_dir(dir);
            }
            waiting
        };
        if waiting {
            // Made without the registry's lock: a session's may be megabytes.
            let bodies = published.fold.snapshot_bodies();
            let mut inner = self.lock();
            if let Some(entry) = inner
                .entries
                .get_mut(key)
                .filter(|e| e.slot.as_ref().is_some_and(|x| Arc::ptr_eq(x, slot)))
            {
                entry.subs.retain(|sub| sub.out.send(sub.s, copy_frame(sub.s, true, &bodies, &published)));
            }
        }
        true
    }

    /// The copy `slot` holds is not one to show after all (a batch that did
    /// not fold, half applied): it is not shown any more, its subscribers
    /// keep what they have, and the next read publishes again.
    pub fn retract(&self, key: &Key, slot: &Slot) {
        let mut inner = self.lock();
        if let Some(entry) = inner.entries.get_mut(key) {
            if entry.slot.as_ref().is_some_and(|x| Arc::ptr_eq(x, slot)) {
                entry.slot = None;
                entry.state = State::Pending;
            }
        }
    }

    /// `bodies` were folded into `published` (held: the guard of `slot`):
    /// hand them to the subscribers, if `slot` is the one published. `reset`:
    /// the copy was rebuilt from nothing, so they get a snapshot instead.
    /// Empty bodies: a heartbeat (how current the copy is moved).
    pub fn deliver(&self, key: &Key, slot: &Slot, published: &Published, reset: bool, bodies: &[(u16, Vec<u8>)]) {
        let waiting = {
            let mut inner = self.lock();
            if !signer_holds(&inner, &key.host, published.signer) {
                return;
            }
            let Some(entry) = inner.entries.get_mut(key) else { return };
            if !entry.slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, slot)) {
                return;
            }
            let before = (entry.time_ms, entry.info.clone());
            note(entry, key.kind, published);
            let waiting = !entry.subs.is_empty();
            let dir = (key.kind != Kind::Session && before != (entry.time_ms, entry.info.clone())).then(|| Self::dir_of(&mut inner));
            drop(inner);
            if let Some(dir) = dir {
                self.send_dir(dir);
            }
            waiting
        };
        if !waiting {
            return;
        }
        // Made without the registry's lock (the slot's is held by the caller,
        // so nothing else is delivered, or subscribes, meanwhile).
        let text = if reset { published.fold.snapshot_bodies() } else { bodies_text(bodies) };
        let mut inner = self.lock();
        if let Some(entry) = inner
            .entries
            .get_mut(key)
            .filter(|e| e.slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, slot)))
        {
            entry.subs.retain(|sub| sub.out.send(sub.s, copy_frame(sub.s, reset, &text, published)));
        }
    }

    /// A `Missing` update from a stream task verifying under `signer`.
    pub fn missing_as(&self, key: &Key, signer: Option<[u8; 32]>) {
        let mut inner = self.lock();
        if !inner.hosts.contains_key(&key.host) || !signer_holds(&inner, &key.host, signer) {
            return;
        }
        let entry = inner.entries.entry(key.clone()).or_default();
        if entry.slot.is_some() || entry.state == State::Missing {
            return;
        }
        entry.state = State::Missing;
        entry.subs.retain(|sub| sub.out.send(sub.s, missing_frame(sub.s)));
    }

    /// A session over the size limit: not kept; its subscribers are told and let go.
    pub fn too_large(&self, key: &Key) {
        self.too_large_as(key, None);
    }

    /// [`LiveCopies::too_large`], from a stream task verifying under `signer`.
    pub fn too_large_as(&self, key: &Key, signer: Option<[u8; 32]>) {
        let mut inner = self.lock();
        if !inner.hosts.contains_key(&key.host) || !signer_holds(&inner, &key.host, signer) {
            return;
        }
        let entry = inner.entries.entry(key.clone()).or_default();
        entry.slot = None;
        entry.state = State::TooLarge;
        end_all(std::mem::take(&mut entry.subs), Ended::TooLarge);
    }

    /// Let go of the session copies no longer followed (`keep` says which
    /// are) that no page is subscribed to: what they held leaves memory.
    pub fn retain_sessions(&self, keep: impl Fn(&Key) -> bool) {
        let mut inner = self.lock();
        inner
            .entries
            .retain(|key, entry| key.kind != Kind::Session || keep(key) || !entry.subs.is_empty());
    }

    /// The sessions pages opened, most recently first: those subscribed to
    /// now, and those let go of within [`LINGER`].
    pub fn opened(&self) -> Vec<(String, String)> {
        let now = crate::agent::now_ms();
        let linger = LINGER.as_millis() as u64;
        let mut inner = self.lock();
        inner.interest.retain(|_, i| !i.subs.is_empty() || now.saturating_sub(i.last_ms) < linger);
        // Those subscribed to now first, then those let go of; the most recent first in each.
        let mut out: Vec<(bool, u64, &(String, String))> = inner.interest.iter().map(|(k, i)| (!i.subs.is_empty(), i.last_ms, k)).collect();
        out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)).then_with(|| a.2.cmp(b.2)));
        out.into_iter().map(|(_, _, k)| k.clone()).collect()
    }

    /// Bumped whenever the sessions pages opened change.
    pub fn interest(&self) -> watch::Receiver<u64> {
        self.interest.subscribe()
    }

    // ------------------------------------------------------------ the gateway's side

    /// Subscribe `out`'s subscription `s` to `key`. It gets the copy as it
    /// stands now (or nothing, while there is none yet; or `missing`), then
    /// every change. `from`: where the page's copy stands (`epoch`, `seq`) —
    /// when the copy still stands there, no snapshot, only an empty `copy`
    /// saying how current it is. Refused for a host not followed, or a
    /// session too large. A subscription whose first frame does not fit the
    /// outbox is not registered: it is overrun, and the page asks again.
    /// `listed`: a session its device's index names (or no index yet) —
    /// one it does not is not followed for the page (no interest), and with
    /// no copy kept it is `missing` at once.
    pub fn subscribe(self: &Arc<Self>, key: Key, s: u32, out: &Arc<Outbox>, from: Option<(u32, u64)>, listed: bool) -> Result<Subscription, Ended> {
        loop {
            let slot = {
                let inner = self.lock();
                if !inner.hosts.contains_key(&key.host) {
                    return Err(Ended::Gone);
                }
                inner.entries.get(&key).and_then(|e| e.slot.clone())
            };
            let Some(slot) = slot else {
                let mut inner = self.lock();
                if !inner.hosts.contains_key(&key.host) {
                    return Err(Ended::Gone);
                }
                let id = next_id(&mut inner);
                let entry = inner.entries.entry(key.clone()).or_default();
                if entry.slot.is_some() {
                    // Published meanwhile: take its snapshot.
                    continue;
                }
                let fits = match entry.state {
                    State::TooLarge => return Err(Ended::TooLarge),
                    State::Missing => out.send(s, missing_frame(s)),
                    State::Pending if !listed => out.send(s, missing_frame(s)),
                    State::Pending => true,
                };
                if fits {
                    entry.subs.push(Subscriber { id, s, out: out.clone() });
                }
                drop(inner);
                return Ok(self.subscribed(key, id, listed));
            };
            let published = slot.lock().expect("slot");
            // Made under the slot's lock only (a session's may be megabytes).
            let frame = if from == Some((published.epoch, published.seq)) {
                copy_frame(s, false, "[]", &published)
            } else {
                copy_frame(s, true, &published.fold.snapshot_bodies(), &published)
            };
            let mut inner = self.lock();
            let id = next_id(&mut inner);
            let Some(entry) = inner
                .entries
                .get_mut(&key)
                .filter(|e| e.slot.as_ref().is_some_and(|x| Arc::ptr_eq(x, &slot)))
            else {
                // Replaced meanwhile: again, with the new one.
                continue;
            };
            if out.send(s, frame) {
                entry.subs.push(Subscriber { id, s, out: out.clone() });
            }
            drop(inner);
            drop(published);
            return Ok(self.subscribed(key, id, listed));
        }
    }

    /// The handle of a subscription just made; a session's adds interest
    /// (`interest`: its device's index lists it, or is not known yet).
    fn subscribed(self: &Arc<Self>, key: Key, id: u64, interest: bool) -> Subscription {
        if key.kind == Kind::Session && interest {
            let mut inner = self.lock();
            let name = (key.host.clone(), key.name.clone());
            let room = inner.hosts.contains_key(&key.host) && (inner.interest.contains_key(&name) || make_room(&mut inner.interest));
            let mut first = false;
            if room {
                let held = inner.interest.entry(name).or_insert(Interest {
                    subs: BTreeSet::new(),
                    last_ms: 0,
                });
                first = held.subs.is_empty();
                held.subs.insert(id);
                held.last_ms = crate::agent::now_ms();
            }
            drop(inner);
            if first {
                self.interest.send_modify(|n| *n += 1);
            }
        }
        Subscription {
            live: Arc::downgrade(self),
            key,
            id,
        }
    }

    fn unsubscribe(&self, key: &Key, id: u64) {
        let mut inner = self.lock();
        if let Some(entry) = inner.entries.get_mut(key) {
            entry.subs.retain(|sub| sub.id != id);
        }
        if key.kind != Kind::Session {
            return;
        }
        let mut last = false;
        if let Some(interest) = inner.interest.get_mut(&(key.host.clone(), key.name.clone())) {
            if interest.subs.remove(&id) && interest.subs.is_empty() {
                interest.last_ms = crate::agent::now_ms();
                last = true;
            }
        }
        drop(inner);
        if last {
            self.interest.send_modify(|n| *n += 1);
        }
    }

    /// The index of `host` as published, if it has one and it names `key`
    /// (`source/id`): `Some(true)`; known not to: `Some(false)`; no index yet: `None`.
    pub fn index_names(&self, host: &str, key: &str) -> Option<bool> {
        let slot = self.lock().entries.get(&Key::new(host, Kind::Index, "")).and_then(|e| e.slot.clone())?;
        let published = slot.lock().expect("slot");
        match &published.fold {
            Fold::Index(index) => Some(index.rows.contains_key(key)),
            _ => None,
        }
    }
}

/// Room for one more interest under [`INTEREST_MAX`]: the oldest let go of
/// goes; `false` when every one is still subscribed to.
fn make_room(interest: &mut BTreeMap<(String, String), Interest>) -> bool {
    if interest.len() < INTEREST_MAX {
        return true;
    }
    let oldest = interest
        .iter()
        .filter(|(_, i)| i.subs.is_empty())
        .min_by_key(|(_, i)| i.last_ms)
        .map(|(k, _)| k.clone());
    match oldest {
        Some(k) => {
            interest.remove(&k);
            true
        }
        None => false,
    }
}

fn next_id(inner: &mut Inner) -> u64 {
    inner.next_id += 1;
    inner.next_id
}

/// Note what the directory needs of a published copy.
fn note(entry: &mut Entry, kind: Kind, published: &Published) {
    match (kind, &published.fold) {
        (Kind::Index, _) => entry.time_ms = (published.time_ms > 0).then_some(published.time_ms),
        (Kind::Status, Fold::Status(status)) => entry.info = Some(status_info(status)),
        _ => {}
    }
}

fn end_all(subs: Vec<Subscriber>, why: Ended) {
    for sub in subs {
        sub.out.end(sub.s, error_frame(sub.s, why.code(), why.message()));
    }
}

/// A gateway subscription: dropped, it ends.
pub struct Subscription {
    live: std::sync::Weak<LiveCopies>,
    key: Key,
    id: u64,
}

impl Subscription {
    /// What it follows.
    pub fn key(&self) -> &Key {
        &self.key
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(live) = self.live.upgrade() {
            live.unsubscribe(&self.key, self.id);
        }
    }
}

impl LiveCopies {
    /// How many subscriptions `key` has (tests, diagnostics).
    pub fn subscribers(&self, key: &Key) -> usize {
        self.lock().entries.get(key).map_or(0, |e| e.subs.len())
    }

    /// How many copies are held in memory (tests, diagnostics).
    pub fn held(&self) -> usize {
        self.lock().entries.values().filter(|e| e.slot.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rh_client::streams::{FeedCopy, IndexCopy};
    use serde_json::json;

    fn index_slot(rows: &[(&str, bool)], seq: u64, time_ms: u64) -> Slot {
        let mut copy = IndexCopy::default();
        for (id, asking) in rows {
            copy.rows
                .insert(format!("claude/{id}"), json!({"source": "claude", "id": id, "asking": asking}));
        }
        copy.as_of = Some(time_ms);
        Arc::new(Mutex::new(Published::new(Fold::Index(copy), seq, time_ms)))
    }

    fn drain(rx: &mut mpsc::Receiver<String>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(text) = rx.try_recv() {
            out.push(serde_json::from_str(&text).unwrap());
        }
        out
    }

    fn body(v: Value) -> (u16, Vec<u8>) {
        (rh_proto::record::FMT_INDEX_V1, serde_json::to_vec(&v).unwrap())
    }

    #[tokio::test]
    async fn a_subscriber_gets_the_copy_then_exactly_what_came_after() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let key = Key::new("m-2", Kind::Index, "");
        let (out, mut rx) = Outbox::new(64, 1 << 30);
        // Before anything is published: nothing yet.
        let early = live.subscribe(key.clone(), 1, &out, None, true).unwrap();
        assert!(drain(&mut rx).is_empty());
        let slot = index_slot(&[("a", false)], 3, 1000);
        assert!(live.publish(&key, 0, &slot));
        let got = drain(&mut rx);
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0]["t"].as_str(), got[0]["reset"].as_bool(), got[0]["headSeq"].as_u64()),
            (Some("copy"), Some(true), Some(3))
        );
        assert_eq!(got[0]["bodies"][0]["upserts"][0]["id"], "a");
        // A second subscriber after it, then a change: each gets it once.
        let late = live.subscribe(key.clone(), 2, &out, None, true).unwrap();
        {
            let mut held = slot.lock().unwrap();
            let change = body(json!({"upserts": [{"source": "claude", "id": "b", "asking": true}], "removed": []}));
            held.fold.apply(&change).unwrap();
            held.seq = 4;
            held.time_ms = 2000;
            live.deliver(&key, &slot, &held, false, &[change]);
        }
        let got = drain(&mut rx);
        assert_eq!(got.len(), 3, "the late one's snapshot, then the change to both: {got:?}");
        assert_eq!(
            (got[0]["s"].as_u64(), got[0]["reset"].as_bool(), got[0]["headSeq"].as_u64()),
            (Some(2), Some(true), Some(3))
        );
        for (frame, s) in got[1..].iter().zip([1, 2]) {
            assert_eq!(frame["s"], s);
            assert!(frame["reset"].is_null());
            assert_eq!(frame["bodies"][0]["upserts"][0]["asking"], true);
            assert_eq!((frame["headSeq"].as_u64(), frame["headTimeMs"].as_u64()), (Some(4), Some(2000)));
        }
        drop(late);
        assert_eq!(live.subscribers(&key), 1);
        // Holding the newest seq already: no snapshot again.
        let same = live.subscribe(key.clone(), 3, &out, Some((0, 4)), true).unwrap();
        let got = drain(&mut rx);
        assert_eq!((got[0]["bodies"].as_array().map(Vec::len), got[0]["reset"].is_null()), (Some(0), true));
        drop((early, same));
        assert_eq!(live.subscribers(&key), 0);
        // The directory says how current the index is.
        assert_eq!(live.dir().borrow()[0].copy_as_of_ms, Some(2000));
    }

    #[tokio::test]
    async fn the_change_reaches_every_subscriber_in_order_with_its_snapshot() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let key = Key::new("m-2", Kind::Index, "");
        let slot = index_slot(&[], 1, 10);
        live.publish(&key, 0, &slot);
        let (out, mut rx) = Outbox::new(1024, 1 << 30);
        let subs: Vec<_> = (0..5).map(|s| live.subscribe(key.clone(), s, &out, None, true).unwrap()).collect();
        for n in 0..20u64 {
            let mut held = slot.lock().unwrap();
            let change = body(json!({"upserts": [{"source": "claude", "id": format!("r{n}")}], "removed": []}));
            held.fold.apply(&change).unwrap();
            held.seq += 1;
            live.deliver(&key, &slot, &held, false, &[change]);
        }
        let got = drain(&mut rx);
        for s in 0..5 {
            let mine: Vec<&Value> = got.iter().filter(|v| v["s"] == s).collect();
            assert_eq!(mine.len(), 21);
            assert_eq!(mine[0]["reset"], true);
            let seqs: Vec<u64> = mine.iter().map(|v| v["headSeq"].as_u64().unwrap()).collect();
            assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
        }
        drop(subs);
    }

    #[test]
    fn an_index_or_status_never_goes_back_to_an_older_epoch() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let key = Key::new("m-2", Kind::Index, "");
        assert!(live.publish(&key, 2, &index_slot(&[("new", false)], 1, 5)));
        assert!(!live.publish(&key, 1, &index_slot(&[("old", false)], 9, 9)), "an older epoch");
        assert!(live.publish(&key, 3, &index_slot(&[("newer", false)], 1, 6)));
        // A host not followed: nothing.
        assert!(!live.publish(&Key::new("m-9", Kind::Index, ""), 0, &index_slot(&[], 1, 1)));
        assert_eq!(live.held(), 1);
    }

    #[tokio::test]
    async fn a_device_that_leaves_ends_its_subscriptions_and_its_copies() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into(), "m-3".into()]);
        let key = Key::new("m-2", Kind::Index, "");
        live.publish(&key, 0, &index_slot(&[("a", false)], 1, 1));
        live.publish(&Key::new("m-3", Kind::Index, ""), 0, &index_slot(&[("b", false)], 1, 1));
        let (out, mut rx) = Outbox::new(64, 1 << 30);
        let _sub = live.subscribe(key.clone(), 7, &out, None, true).unwrap();
        drain(&mut rx);
        live.set_hosts(&["m-3".into()]);
        let got = drain(&mut rx);
        assert_eq!(
            (got[0]["t"].as_str(), got[0]["code"].as_str(), got[0]["s"].as_u64()),
            (Some("copy_error"), Some("gone"), Some(7))
        );
        assert_eq!(live.held(), 1);
        assert_eq!(live.subscribe(key, 8, &out, None, true).err(), Some(Ended::Gone));
        assert_eq!(live.dir().borrow().iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["m-3"]);
        // Cleared: nothing left, the directory empty.
        let _b = live.subscribe(Key::new("m-3", Kind::Index, ""), 9, &out, None, true).unwrap();
        drain(&mut rx);
        live.clear();
        assert_eq!(drain(&mut rx)[0]["code"], "gone");
        assert_eq!(live.held(), 0);
        assert!(live.dir().borrow().is_empty());
    }

    #[tokio::test]
    async fn a_session_subscribed_to_is_interest_that_lingers_once_let_go() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let mut changed = live.interest();
        let (out, _rx) = Outbox::new(64, 1 << 30);
        let a = live.subscribe(Key::new("m-2", Kind::Session, "claude/a"), 1, &out, None, true).unwrap();
        let a2 = live.subscribe(Key::new("m-2", Kind::Session, "claude/a"), 2, &out, None, true).unwrap();
        let b = live.subscribe(Key::new("m-2", Kind::Session, "claude/b"), 3, &out, None, true).unwrap();
        assert!(changed.has_changed().unwrap());
        changed.borrow_and_update();
        assert_eq!(live.opened().len(), 2);
        drop(a);
        assert!(!changed.has_changed().unwrap(), "still subscribed once");
        drop(a2);
        assert!(changed.has_changed().unwrap());
        // Let go of, it lingers, after the ones still subscribed.
        assert_eq!(
            live.opened(),
            vec![("m-2".to_string(), "claude/b".to_string()), ("m-2".to_string(), "claude/a".to_string())]
        );
        drop(b);
        assert_eq!(live.opened().len(), 2);
        // The host leaves: its interest with it.
        live.set_hosts(&[]);
        assert!(live.opened().is_empty());
    }

    /// Once the copies task says a host's identity key is a
    /// new one, a stream task still verifying under the old key — cancelled,
    /// but a ready frame won its race with the cancel — shows nothing: it
    /// publishes nothing, delivers nothing to a copy it published before, and
    /// says nothing missing or too large. One under the new key does.
    #[tokio::test]
    async fn a_copy_verified_under_a_key_its_host_renewed_away_from_is_never_shown() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let (old, new) = ([7; 32], [9; 32]);
        let key = Key::new("m-2", Kind::Session, "claude/a");
        let (out, mut rx) = Outbox::new(64, 1 << 30);
        let _sub = live.subscribe(key.clone(), 1, &out, None, true).unwrap();
        drain(&mut rx);
        let feed = |text: &str| {
            let mut feed = FeedCopy::default();
            feed.apply(&json!({"fromIndex": 0, "items": [{"text": text}], "endIndex": 1, "fp": "f", "prefixFp": "p"}))
                .unwrap();
            Fold::Feed(feed, vec![10])
        };
        // Before the copies task said which key: nothing a task verified is shown.
        let early: Slot = Arc::new(Mutex::new(Published::new(feed("EARLY"), 1, 5).signed_by(old)));
        assert!(!live.publish(&key, 0, &early));
        // Under the key now: shown.
        live.set_signers(&[("m-2".into(), old)]);
        let mine: Slot = Arc::new(Mutex::new(Published::new(feed("OLD"), 1, 5).signed_by(old)));
        assert!(live.publish(&key, 0, &mine));
        assert!(drain(&mut rx).iter().any(|m| m.to_string().contains("OLD")));
        // The host renewed: the old task's frame that won the race shows nothing.
        live.set_signers(&[("m-2".into(), new)]);
        {
            let held = mine.lock().unwrap();
            live.deliver(
                &key,
                &mine,
                &held,
                false,
                &[(rh_proto::record::FMT_FEED_V1, br#"{"fromIndex":1,"items":[{"text":"LATE"}],"endIndex":2}"#.to_vec())],
            );
        }
        assert!(!live.publish(&key, 0, &Arc::new(Mutex::new(Published::new(feed("STALE"), 2, 6).signed_by(old)))));
        live.missing_as(&key, Some(old));
        live.too_large_as(&key, Some(old));
        assert!(drain(&mut rx).is_empty(), "nothing from the old key's task");
        // The new key's task: shown.
        let theirs: Slot = Arc::new(Mutex::new(Published::new(feed("NEW"), 1, 7).signed_by(new)));
        assert!(live.publish(&key, 0, &theirs));
        assert!(drain(&mut rx).iter().any(|m| m.to_string().contains("NEW")));
        // A copy read back from the store (no task's) is as before.
        assert!(!live.preload(&key, 0, &Arc::new(Mutex::new(Published::new(feed("STORED"), 1, 1)))), "one is shown already");
    }

    #[tokio::test]
    async fn a_session_is_missing_or_too_large_and_a_slow_page_is_overrun() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let key = Key::new("m-2", Kind::Session, "claude/a");
        let (out, mut rx) = Outbox::new(2, 1 << 30);
        let _sub = live.subscribe(key.clone(), 1, &out, None, true).unwrap();
        live.missing_as(&key, None);
        assert_eq!(drain(&mut rx)[0]["t"], "missing");
        // A late subscriber hears it at once.
        let _late = live.subscribe(key.clone(), 2, &out, None, true).unwrap();
        assert_eq!(drain(&mut rx)[0]["t"], "missing");
        // Then it appears.
        let mut feed = FeedCopy::default();
        feed.apply(&json!({"fromIndex": 0, "items": [{"text": "hi"}], "endIndex": 1, "fp": "f", "prefixFp": "p"}))
            .unwrap();
        let slot: Slot = Arc::new(Mutex::new(Published::new(Fold::Feed(feed, vec![10]), 2, 5)));
        live.publish(&key, 0, &slot);
        let got = drain(&mut rx);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["bodies"][0]["items"][0]["text"], "hi");
        assert_eq!(got[0]["bodies"][0]["prefixFp"], "p");
        // A page that does not read: its queue fills, the subscription is dropped as overrun.
        for n in 0..3u64 {
            let held = slot.lock().unwrap();
            live.deliver(
                &key,
                &slot,
                &held,
                false,
                &[(rh_proto::record::FMT_FEED_V1, br#"{"fromIndex":1,"items":[],"endIndex":1}"#.to_vec())],
            );
            let _ = n;
        }
        let overrun = out.take_overrun();
        assert_eq!(overrun, vec![1, 2]);
        assert_eq!(live.subscribers(&key), 0, "both dropped");
        drain(&mut rx);
        // Too large: refused from then on.
        live.too_large(&key);
        assert_eq!(live.subscribe(key, 3, &out, None, true).err(), Some(Ended::TooLarge));
    }

    /// Review of S1-7 (deadlock): a gateway reading the directory while it
    /// lets go of a subscription, and the registry saying a new directory at
    /// the same time, must not wait on each other.
    #[test]
    fn reading_the_directory_while_letting_go_never_waits_on_a_new_directory() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let (done, finished) = std::sync::mpsc::channel();
        let reader = {
            let (live, done) = (live.clone(), done.clone());
            std::thread::spawn(move || {
                let (out, _rx) = Outbox::new(16, 1 << 20);
                let dir = live.dir();
                for s in 0..20_000u32 {
                    let sub = live.subscribe(Key::new("m-2", Kind::Status, ""), s, &out, None, true).unwrap();
                    let held = dir.borrow();
                    drop(sub);
                    drop(held);
                }
                done.send("reader").unwrap();
            })
        };
        let writer = {
            let live = live.clone();
            std::thread::spawn(move || {
                for n in 0..20_000u32 {
                    live.set_presence("m-2", Some(n % 2 == 0));
                }
                done.send("writer").unwrap();
            })
        };
        for _ in 0..2 {
            finished
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("both finish: neither waits on the other for ever");
        }
        reader.join().unwrap();
        writer.join().unwrap();
    }

    /// Review of S1-7, round 3: a directory made before a later one (under
    /// the registry's lock) but sent after it does not go out: a device that
    /// just left does not come back.
    #[test]
    fn an_older_directory_never_goes_out_over_a_newer_one() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into(), "m-3".into()]);
        // Made under the lock (a copy delivered, say), then held up before it is sent…
        let older = LiveCopies::dir_of(&mut live.lock());
        // …while m-2 leaves, and that goes out first.
        live.set_hosts(&["m-3".into()]);
        live.send_dir(older);
        let ids: Vec<String> = live.dir().borrow().iter().map(|h| h.id.clone()).collect();
        assert_eq!(ids, vec!["m-3".to_string()]);
        // A later change still goes out.
        live.set_presence("m-3", Some(true));
        assert_eq!(live.dir().borrow()[0].state, "online");
    }

    /// Review of S1-7, round 3: the byte budget is taken atomically — of
    /// many senders at once, no more fit than the budget holds.
    #[test]
    fn many_senders_at_once_never_overfill_the_outbox() {
        let (out, rx) = Outbox::new(1 << 16, 1000);
        let threads: Vec<_> = (0..8u32)
            .map(|t| {
                let out = out.clone();
                std::thread::spawn(move || (0..50).filter(|_| out.send(t, "x".repeat(100))).count())
            })
            .collect();
        let sent: usize = threads.into_iter().map(|t| t.join().unwrap()).sum();
        assert!((1..=10).contains(&sent), "{sent} frames of 100 bytes in a budget of 1000");
        drop(rx);
    }

    /// Review of S1-7 (memory): what waits on one connection is bounded in
    /// bytes, not only in frames; one frame over the budget still goes when
    /// nothing else waits.
    #[test]
    fn an_outbox_holds_so_many_bytes_and_no_more() {
        let (out, mut rx) = Outbox::new(64, 100);
        assert!(out.send(1, "x".repeat(60)));
        assert!(!out.send(2, "y".repeat(60)), "60 + 60 > 100");
        assert_eq!(out.take_overrun(), vec![2]);
        let taken = rx.try_recv().unwrap();
        out.taken(taken.len());
        assert!(out.send(2, "y".repeat(60)));
        out.taken(rx.try_recv().unwrap().len());
        assert!(out.send(3, "z".repeat(500)), "alone, a frame over the budget goes");
        assert!(!out.send(4, "w".to_string()), "but nothing after it until it is taken");
    }

    /// The registry ending a subscription (its device left) tells the
    /// connection to let go of it as well.
    #[test]
    fn a_subscription_the_registry_ends_is_let_go_of_by_its_connection() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let (out, _rx) = Outbox::new(16, 1 << 20);
        let _sub = live.subscribe(Key::new("m-2", Kind::Index, ""), 5, &out, None, true).unwrap();
        assert!(out.take_ended().is_empty());
        live.set_hosts(&[]);
        assert_eq!(out.take_ended(), vec![5]);
    }

    /// Review of S1-7 (memory): the sessions pages opened are at most
    /// [`INTEREST_MAX`]; the one let go of longest ago makes room, and none
    /// still subscribed to is pushed out.
    #[test]
    fn interest_is_bounded_and_the_oldest_let_go_of_makes_room() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let (out, _rx) = Outbox::new(16, 1 << 20);
        let key = |n: usize| Key::new("m-2", Kind::Session, &format!("claude/s{n}"));
        let held: Vec<_> = (0..INTEREST_MAX)
            .map(|n| live.subscribe(key(n), n as u32, &out, None, true).unwrap())
            .collect();
        assert_eq!(live.opened().len(), INTEREST_MAX);
        let over = live.subscribe(key(INTEREST_MAX), 999, &out, None, true).unwrap();
        assert_eq!(live.opened().len(), INTEREST_MAX, "no room: every one is still subscribed to");
        assert!(!live.opened().contains(&("m-2".into(), "claude/s64".into())));
        drop(over);
        // Let go of the first one, then ask for a new one: it takes its place.
        let mut held = held.into_iter();
        drop(held.next());
        let _new = live.subscribe(key(100), 100, &out, None, true).unwrap();
        let opened = live.opened();
        assert_eq!(opened.len(), INTEREST_MAX);
        assert!(opened.contains(&("m-2".into(), "claude/s100".into())));
        assert!(!opened.contains(&("m-2".into(), "claude/s0".into())));
        drop(held);
    }

    #[test]
    fn the_directory_reads_a_devices_name_system_and_version_off_its_status() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into(), "m-3".into()]);
        let status = json!({"displayName": " studio ", "os": {"name": "macos", "version": "15.1", "arch": "arm64"}, "agentVersion": "0.2.122", "settings": {"platform": "darwin", "serverVersion": "0.2.120"}});
        live.publish(
            &Key::new("m-2", Kind::Status, ""),
            0,
            &Arc::new(Mutex::new(Published::new(Fold::Status(status), 1, 1))),
        );
        let old = json!({"settings": {"platform": "linux", "serverVersion": "0.2.110"}});
        live.publish(
            &Key::new("m-3", Kind::Status, ""),
            0,
            &Arc::new(Mutex::new(Published::new(Fold::Status(old), 1, 1))),
        );
        live.set_presence("m-2", Some(true));
        live.set_presence("m-3", Some(false));
        let dir = live.dir().borrow().clone();
        assert_eq!(dir[0].name.as_deref(), Some("studio"));
        assert_eq!(dir[0].os.as_deref(), Some("macOS 15.1 arm64"));
        assert_eq!(dir[0].agent_version.as_deref(), Some("0.2.122"));
        assert_eq!(dir[0].state, "online");
        assert_eq!(
            (dir[1].name.as_deref(), dir[1].os.as_deref(), dir[1].agent_version.as_deref()),
            (None, Some("linux"), Some("0.2.110"))
        );
        assert_eq!(dir[1].state, "offline");
        assert!(dir[1].since_ms.is_some());
        live.presence_unknown();
        assert!(live.dir().borrow().iter().all(|h| h.state == "unknown"));
        let wire = serde_json::to_value(&**live.dir().borrow()).unwrap();
        assert!(wire[0].get("copyAsOfMs").is_some() && wire[0].get("agentVersion").is_some());
    }
}
