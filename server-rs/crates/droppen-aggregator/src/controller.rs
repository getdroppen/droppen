//! The local install as a device that controls other devices (local-aggregator
//! §5, option A; S1-4): with the one identity it runs AI with, it signs in to
//! its hub as a device, takes the other hosts' machine keys an admin sealed
//! for it, and asks the admins for the ones it lacks.
//!
//! It runs inside the hub link (`agent::link`) and stops with it —
//! when the server stops, when this computer is disconnected, and when its
//! own verified chain removes it (the link then deletes `aggregator/`). It
//! works only while:
//!
//! - the chain names this computer as a current device (an enrollment still
//!   waiting for a device, or a removed computer, has no device role), and
//! - the owner has not turned the aggregator off ([`super::Switch`]).
//!
//! **Each round** (on a chain change, every [`GAP_EVERY`] while a key is
//! missing, else every [`IDLE_EVERY`]):
//!
//! 1. a device session, minted with the machine key when there is none or it
//!    expires soon ([`super::hub::DeviceClient::sign_in`]);
//! 2. the mailbox: each host key (purpose 5) is opened with
//!    [`rh_proto::wrap::open_host_key`] — sealed by a current admin, to this
//!    device, exactly the key the chain committed for that host and epoch —
//!    and kept only for a host that accepts this device's control (review
//!    D2); a key the chain has not caught up with yet (its epoch, or its
//!    sender, unknown) stays in the mailbox for [`LATER_FOR`] at most. All
//!    other mail is acknowledged unread: this device manages nothing, so it
//!    is never sent a user key, and it answers no key request;
//! 3. keys of hosts that no longer accept it are dropped;
//! 4. a host that accepts it whose newest committed epoch it lacks for
//!    [`ASK_AFTER`] (the admin's own delivery comes first) makes it ask every
//!    current admin once (purpose 4) — at most every [`ASK_EVERY`], across
//!    restarts. Online admins answer it with host keys only.
//!
//! It never signs a chain entry, never attests, holds no user key, and writes
//! nothing to the hub but key requests and acknowledgements.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gotcha::Schematic;
use rh_proto::chain::{ChainError, ChainState};
use rh_proto::identity::MachineIdentity;
use rh_proto::wrap::{self, HostKeyContext, WrapError};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::hub::{DeviceClient, HubError, Mail};
use super::keys::HostKeys;
use super::{Enrollment, Switch, View};
use droppen_instance::paths::Paths;

/// A round while a key is missing.
pub const GAP_EVERY: Duration = Duration::from_secs(5);
/// A round otherwise.
pub const IDLE_EVERY: Duration = Duration::from_secs(60);
/// A missing key is asked for once it has been missing this long.
pub const ASK_AFTER: Duration = Duration::from_secs(20);
/// At most one key request per this long (as the web's `KEY_REQUEST_EVERY_MS`).
pub const ASK_EVERY: Duration = Duration::from_secs(10 * 60);
/// How long after asking about a gap it asks about that same gap again: 10
/// minutes after the first request, an hour after the second, six hours
/// after every one after that — until the gap closes or changes (a host's
/// newest epoch moves, a host comes or goes), which starts it again
/// (security re-review of S1-4, 2: with the admins offline for long, every
/// install asking every 10 minutes would fill an admin's mailbox, and other
/// devices' requests would bounce).
pub const ASK_BACKOFF: [Duration; 3] = [ASK_EVERY, Duration::from_secs(60 * 60), Duration::from_secs(6 * 60 * 60)];
/// A key that cannot be stored (the disk) is tried this many rounds, then its
/// mail is acknowledged unread — it would sit in the mailbox for good
/// (security re-review of S1-4, 4). The device asks again later.
pub const MAX_PUT_TRIES: u32 = 5;
/// How long a host key the chain has not caught up with stays unanswered.
pub const LATER_FOR: Duration = Duration::from_secs(10 * 60);
/// The hub refusing host devices altogether (`host_devices_off`): asked again
/// this much later.
const REFUSED_EVERY: Duration = Duration::from_secs(10 * 60);
/// The hub saying this device was removed (`device_removed`): its keys go at
/// once, and it asks again only this much later, quietly (security review of
/// S1-4, S2) — the chain, not the hub, is what removes it for good.
const REMOVED_EVERY: Duration = Duration::from_secs(60 * 60);
/// Backoff on an unreachable hub.
const BACKOFF_MIN: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);
/// `/v1/me` (which agents declared `policy/1`) is read at most this often.
const ME_EVERY: Duration = Duration::from_secs(5 * 60);
/// The most mail one round reads.
const MAX_PAGES: usize = 8;

/// One device that runs AI and accepts this computer's control, and the
/// content keys (its machine-key epochs) held of it here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AggregatorHost {
    /// Its id.
    pub id: String,
    /// Its newest committed machine-key epoch.
    pub latest_epoch: Option<u32>,
    /// The epochs held here.
    pub epochs: Vec<u32>,
    /// Whether its agent declared `policy/1` (refuses this computer its admin
    /// routes); `null` until the hub said.
    pub policy: Option<bool>,
    /// What is copied of it here (S1-5); `null` before anything is, and for
    /// a caller who may not see other devices (§4.6: only whoever manages the
    /// link, or a local account the owner let see them).
    pub copy: Option<super::copies::AggregatorCopy>,
}

/// What the local settings show (`GET /api/cloud`'s `aggregator`). Nothing secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The controller runs (the link runs, the chain names this device, and
    /// the aggregator is on).
    pub running: bool,
    /// Signed in to the hub as a device now.
    pub signed_in: bool,
    /// Why the last round failed, in words.
    pub error: Option<String>,
    /// The hosts that accept this device's control.
    pub hosts: Vec<AggregatorHost>,
    /// When it last asked the admins for keys.
    pub asked_ms: Option<u64>,
}

/// How often it does what (the constants above; shorter in tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// A round while a key is missing ([`GAP_EVERY`]).
    pub gap_every: Duration,
    /// A round otherwise ([`IDLE_EVERY`]).
    pub idle_every: Duration,
    /// How long a key is missing before it is asked for ([`ASK_AFTER`]).
    pub ask_after: Duration,
    /// After the n-th request in a row about the same gap, the next one only
    /// this long after it ([`ASK_BACKOFF`]); between different gaps the first
    /// step at least.
    pub ask_backoff: [Duration; 3],
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            gap_every: GAP_EVERY,
            idle_every: IDLE_EVERY,
            ask_after: ASK_AFTER,
            ask_backoff: ASK_BACKOFF,
        }
    }
}

/// What the controller needs from the link it runs in.
pub struct Env {
    /// The instance's paths.
    pub paths: Paths,
    /// The machine key: this device's one identity.
    pub identity: Arc<MachineIdentity>,
    /// The link's enrollment, with its verified chain.
    pub enrollment: Arc<dyn Enrollment>,
    /// The owner's switch, asked each round and before each write.
    pub switch: Arc<dyn Switch>,
    /// Bumped whenever the chain advanced.
    pub chain: watch::Receiver<u64>,
    /// The link's token (a child of it): the controller stops with the link.
    pub cancel: CancellationToken,
    /// Where it reports to.
    pub status: Arc<Mutex<Status>>,
    /// How often.
    pub timing: Timing,
    /// Where it hands the keys it holds to the copies task (S1-5): in
    /// memory only; `None` while it does not run.
    pub keys_out: watch::Sender<Option<Arc<super::copies::KeyRing>>>,
    /// Woken when mail for this device was pushed (the copies task hears it).
    pub nudge: Arc<tokio::sync::Notify>,
    /// Whether the device role may sign in now ([`super::SignInGate`]);
    /// `None`: always.
    pub may_sign_in: Option<super::SignInGate>,
}

/// What to do with one mail. Its `Debug` never prints a key (security
/// review of S1-4, N5).
#[derive(Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A host key, opened and checked: keep it, then acknowledge.
    Keep {
        /// The host whose key it is.
        host: String,
        /// Its epoch.
        epoch: u32,
        /// The key.
        key: zeroize::Zeroizing<Vec<u8>>,
    },
    /// Acknowledge it unread (or refused for good).
    Drop(&'static str),
    /// The chain may be behind it: leave it for now.
    Later,
}

impl std::fmt::Debug for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Keep { host, epoch, .. } => f
                .debug_struct("Keep")
                .field("host", host)
                .field("epoch", epoch)
                .field("key", &"[redacted]")
                .finish(),
            Verdict::Drop(why) => f.debug_tuple("Drop").field(why).finish(),
            Verdict::Later => f.write_str("Later"),
        }
    }
}

/// Judge one mail against the chain (pure, for tests): a host key (purpose 5)
/// from a current admin, to `me`, of a host that accepts `me`'s control,
/// exactly as the chain committed it — else dropped, or left for later when
/// the chain may simply be behind it.
pub fn judge(chain: &ChainState, account: &str, me: &str, recipient: &rh_proto::keys::DhKeypair, mail: &Mail) -> Verdict {
    if mail.purpose != wrap::MAIL_HOST_KEY {
        return Verdict::Drop("not a host key: this device manages nothing");
    }
    let Ok((host, sealed)) = wrap::decode_host_key_mail(&mail.sealed) else {
        return Verdict::Drop("a host key that does not decode");
    };
    if chain.may_control(me, &host).is_err() {
        // Not (or no longer) a host that accepts this device: none of its business (D2).
        if chain.device_record(&host).is_none() {
            return Verdict::Later;
        }
        return Verdict::Drop("a host that does not accept this device's control");
    }
    let ctx = HostKeyContext {
        account: account.to_string(),
        sender: mail.sender.clone(),
        recipient: me.to_string(),
        host: host.clone(),
        epoch: mail.epoch,
    };
    match wrap::open_host_key(recipient, me, chain, &ctx, &sealed) {
        Ok(key) => Verdict::Keep {
            host,
            epoch: mail.epoch,
            key,
        },
        // An epoch the chain has not committed yet, or a sender it does not
        // know yet: the hub delivers mail before the chain catches up.
        Err(WrapError::Uncommitted(ChainError::UncommittedKey { .. })) if mail.epoch >= chain.machine_key_epochs(&host) => Verdict::Later,
        Err(WrapError::UnknownSender(_)) if chain.device_record(&mail.sender).is_none() => Verdict::Later,
        Err(_) => Verdict::Drop("a host key that does not open as the chain committed it"),
    }
}

/// The hosts that accept `me`'s control, and the newest committed epoch of
/// each that `keys` lacks.
pub fn gaps(chain: &ChainState, me: &str, keys: &HostKeys) -> Vec<(String, u32)> {
    chain
        .hosts_admitting(me)
        .filter_map(|h| {
            let latest = chain.latest_machine_epoch(&h.info.id)?;
            (!keys.has(&h.info.id, latest)).then(|| (h.info.id.clone(), latest))
        })
        .collect()
}

/// The name of a gap (`host/epoch,…`, in host order): a request backs off
/// for as long as it is the same.
pub fn gap_name(gaps: &[(String, u32)]) -> String {
    gaps.iter().map(|(h, e)| format!("{h}/{e}")).collect::<Vec<_>>().join(",")
}

/// Whether to ask the admins about `gap` now (`Some(n)`: the n-th request in
/// a row about it), given since when (ms) a key has been missing and the last
/// request `(at, gap, tries)`. Pure, for tests: missing for
/// [`Timing::ask_after`] first (the admin's own delivery comes first); then
/// at once for a gap never asked about — but never sooner than the first
/// backoff step after any request — and for the same gap only after the step
/// its count says.
pub fn ask_due(now: u64, since: u64, last: (Option<u64>, &str, u32), gap: &str, timing: &Timing) -> Option<u32> {
    let ms = |d: Duration| d.as_millis() as u64;
    if now.saturating_sub(since) < ms(timing.ask_after) {
        return None;
    }
    let (at, asked_gap, tries) = last;
    let tries = if asked_gap == gap { tries } else { 0 };
    let wait = match tries {
        0 => ms(timing.ask_backoff[0]),
        n => ms(timing.ask_backoff[(n as usize - 1).min(timing.ask_backoff.len() - 1)]),
    };
    // A clock that went back counts as long ago.
    let elapsed = at.map(|a| now.checked_sub(a).unwrap_or(u64::MAX));
    if elapsed.is_some_and(|e| e < wait) {
        return None;
    }
    Some(tries + 1)
}

/// The controller's state across rounds.
struct Controller {
    env: Env,
    client: Option<DeviceClient>,
    keys: Option<HostKeys>,
    /// Since when (ms) a key has been missing.
    gap_since: Option<u64>,
    /// Mail left for the chain to catch up with: id → when first seen (ms).
    later: HashMap<i64, u64>,
    /// `policy/1` by host, and when it was read.
    policy: BTreeMap<String, bool>,
    policy_ms: u64,
    policy_seq: Option<u64>,
    backoff: Duration,
    /// The enrollment `(account, device)` the hub said was removed, and when
    /// (ms): no more rounds for it until [`REMOVED_EVERY`] has passed.
    hub_removed: Option<(String, String, u64)>,
    /// Mail whose key could not be stored: id → rounds tried ([`MAX_PUT_TRIES`]).
    put_failures: HashMap<i64, u32>,
    /// Parked (R6 review): the hub suspended the device role while this
    /// computer's identity renewal is pending (`rotation_pending`) at this
    /// chain version; no round until the chain moves, or until (ms).
    suspended: Option<(u64, u64)>,
}

/// Run the controller until `env.cancel`.
pub async fn run(env: Env) {
    let mut c = Controller {
        env,
        client: None,
        keys: None,
        gap_since: None,
        later: HashMap::new(),
        policy: BTreeMap::new(),
        policy_ms: 0,
        policy_seq: None,
        backoff: BACKOFF_MIN,
        hub_removed: None,
        put_failures: HashMap::new(),
        suspended: None,
    };
    loop {
        if c.env.cancel.is_cancelled() {
            break;
        }
        let wait = c.round().await;
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            changed = c.env.chain.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            // Mail was pushed: read it now (the rounds above are the fallback).
            () = c.env.nudge.notified() => {}
            () = c.env.cancel.cancelled() => break,
        }
    }
    c.env.keys_out.send_replace(None);
    let mut status = c.env.status.lock().expect("aggregator status");
    status.running = false;
    status.signed_in = false;
}

impl Controller {
    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.env.status.lock().expect("aggregator status"));
    }

    fn idle(&mut self, why: Option<String>) -> Duration {
        self.client = None;
        self.keys = None;
        self.env.keys_out.send_replace(None);
        self.set_status(|s| {
            *s = Status {
                error: why,
                ..Status::default()
            }
        });
        self.env.timing.idle_every
    }

    async fn round(&mut self) -> Duration {
        if !self.env.switch.is_on() {
            return self.idle(None);
        }
        let Some(view) = self.env.enrollment.view() else {
            return self.idle(None);
        };
        if let Some((account, device, at)) = &self.hub_removed {
            if *account == view.account && *device == view.me {
                let left = (REMOVED_EVERY.as_millis() as u64).saturating_sub(droppen_base::time::now_ms_u64().saturating_sub(*at));
                if left > 0 {
                    return Duration::from_millis(left);
                }
            }
            self.hub_removed = None;
        }
        if let Some(wait) = self.parked() {
            return wait;
        }
        // The client and the keys are bound to this enrollment.
        if self
            .client
            .as_ref()
            .is_none_or(|c| c.account != view.account || c.device != view.me || c.origin() != view.hub)
        {
            self.client = match DeviceClient::new(&view.hub, &view.account, &view.me) {
                Ok(client) => Some(client),
                Err(error) => return self.idle(Some(error)),
            };
            self.keys = None;
            self.later.clear();
        }
        if self.keys.is_none() {
            // Every write asks again, under the directory's lock: still on, the link still running (S1).
            let (switch, cancel) = (self.env.switch.clone(), self.env.cancel.clone());
            let guard: super::keys::Guard = Box::new(move || !cancel.is_cancelled() && switch.is_on());
            match HostKeys::load(&self.env.paths, &view.account, &view.me) {
                Ok(keys) => self.keys = Some(keys.guarded(guard)),
                Err(error) => {
                    tracing::error!(%error, "cannot read the other devices' keys");
                    return self.idle(Some(error));
                }
            }
        }
        self.set_status(|s| s.running = true);
        match self.work(&view).await {
            Ok(missing) => {
                self.backoff = BACKOFF_MIN;
                self.report(&view, None);
                if missing {
                    self.env.timing.gap_every
                } else {
                    self.env.timing.idle_every
                }
            }
            Err(HubError::Removed) => {
                // The hub says this device was removed (while this computer was
                // offline, say, so its chain never showed it): what it held to
                // control the others goes now, and it stops asking (S2).
                self.forget_removed(&view);
                self.env.timing.idle_every.max(Duration::from_millis(1))
            }
            Err(HubError::Suspended) => {
                // Parked until the chain moves — the renewal committed (the
                // link then restarts with the new key) — or REFUSED_EVERY:
                // not retried on the back-off meanwhile (R6 review).
                tracing::info!(device = %view.me, "the hub suspends the device role while this computer's identity renewal is pending: parked until the chain moves");
                self.suspended = Some((*self.env.chain.borrow(), droppen_base::time::now_ms_u64() + REFUSED_EVERY.as_millis() as u64));
                self.report(&view, Some(HubError::Suspended.to_string()));
                REFUSED_EVERY
            }
            Err(error) => {
                let wait = match &error {
                    HubError::Removed | HubError::Off => REFUSED_EVERY,
                    HubError::NotInChain => self.env.timing.gap_every,
                    _ => {
                        let wait = self.backoff;
                        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
                        wait
                    }
                };
                tracing::warn!(%error, "the device role could not reach the hub; trying again in {wait:?}");
                self.report(&view, Some(error.to_string()));
                wait
            }
        }
    }

    /// One round with the hub: `true` while a key is missing.
    async fn work(&mut self, view: &View) -> Result<bool, HubError> {
        // A host that no longer accepts this device loses its keys before
        // anything that needs the hub (D2: narrowing purges at once).
        self.retain(view);
        if self.keys.is_none() {
            return Ok(false);
        }
        // This computer's identity key was renewed and the link has not
        // restarted with the new one yet: nothing that signs in with the
        // retired key (`docs/host-identity-rotation.md` §3.8).
        if !super::holds_named_keys(&view.chain, &view.me, &self.env.identity) {
            tracing::info!(device = %view.me, "the chain names a renewed identity key for this device: the device role waits for the link to take it");
            return Ok(false);
        }
        // A staged identity key is kept and the link has not shown the key
        // in use is still current: the chain here may be stale.
        if self.env.may_sign_in.as_ref().is_some_and(|may| !may()) {
            tracing::debug!(device = %view.me, "a staged identity key is kept: the device role waits for the link's sign-in");
            return Ok(false);
        }
        self.publish(view);
        let now = droppen_base::time::now_ms_u64();
        let client = self.client.as_mut().expect("a client");
        if !client.signed_in(now) {
            client.sign_in(&self.env.identity, now).await?;
            tracing::info!(device = %view.me, "signed in to the hub as a device");
        }
        self.read_mail(view).await?;
        self.retain(view);
        // A write refused because the aggregator went off, or its keys were
        // deleted, dropped them from memory: nothing more this round.
        if self.keys.is_none() {
            return Ok(false);
        }
        self.read_policy(view).await;
        let Some(keys) = self.keys.as_ref() else {
            return Ok(false);
        };
        let gaps = gaps(&view.chain, &view.me, keys);
        if gaps.is_empty() {
            self.gap_since = None;
            // The next gap starts its backoff again.
            let closed = self.keys.as_mut().expect("keys").gap_closed();
            if !self.refused(&closed) {
                if let Err(error) = closed {
                    tracing::warn!(%error, "cannot note that no key is missing");
                }
            }
            return Ok(false);
        }
        let since = *self.gap_since.get_or_insert(now);
        let name = gap_name(&gaps);
        if let Some(tries) = ask_due(now, since, keys.asked(), &name, &self.env.timing) {
            self.ask(view, &gaps, &name, tries).await?;
        }
        Ok(true)
    }

    async fn read_mail(&mut self, view: &View) -> Result<(), HubError> {
        let now = droppen_base::time::now_ms_u64();
        let mut after = 0i64;
        for _ in 0..MAX_PAGES {
            let page = self.client.as_mut().expect("a client").mailbox(after).await?;
            let full = page.len() >= crate::hub::MAIL_PAGE;
            for mail in page {
                after = after.max(mail.id);
                if self.keys.is_none() {
                    // Turned off, or deleted, mid-round: leave the rest in the mailbox.
                    return Ok(());
                }
                match judge(&view.chain, &view.account, &view.me, self.env.identity.dh(), &mail) {
                    Verdict::Keep { host, epoch, key } => {
                        let stored = self.keys.as_mut().expect("keys").put(&host, epoch, &key);
                        if self.refused(&stored) {
                            return Ok(());
                        }
                        match stored {
                            Ok(fresh) => {
                                if fresh {
                                    tracing::info!(%host, epoch, sender = %mail.sender, "took another device's content key from the mailbox");
                                }
                            }
                            // Another key for an epoch held: definitive — acknowledged, never tried again.
                            Err(error) if error.starts_with(super::keys::CONFLICT) => {
                                tracing::warn!(%error, %host, epoch, sender = %mail.sender, "dropped a host key: {error}");
                            }
                            // The disk: kept in the mailbox and tried again next round — a few rounds only.
                            Err(error) => {
                                let tries = self.put_failures.entry(mail.id).or_insert(0);
                                *tries += 1;
                                if *tries < MAX_PUT_TRIES {
                                    tracing::warn!(%error, %host, epoch, tries = *tries, "cannot keep another device's key; trying again");
                                    continue;
                                }
                                tracing::error!(%error, %host, epoch, "cannot keep another device's key; dropping its mail (it is asked for again)");
                            }
                        }
                        self.put_failures.remove(&mail.id);
                        self.later.remove(&mail.id);
                        self.client.as_mut().expect("a client").ack(mail.id).await?;
                    }
                    Verdict::Drop(why) => {
                        if mail.purpose == wrap::MAIL_HOST_KEY {
                            tracing::warn!(id = mail.id, sender = %mail.sender, epoch = mail.epoch, "dropped a host key: {why}");
                        }
                        self.later.remove(&mail.id);
                        self.client.as_mut().expect("a client").ack(mail.id).await?;
                    }
                    Verdict::Later => {
                        let first = *self.later.entry(mail.id).or_insert(now);
                        if now.saturating_sub(first) >= LATER_FOR.as_millis() as u64 {
                            tracing::warn!(id = mail.id, sender = %mail.sender, "dropped a host key the chain never caught up with");
                            self.later.remove(&mail.id);
                            self.client.as_mut().expect("a client").ack(mail.id).await?;
                        }
                    }
                }
            }
            if !full {
                break;
            }
        }
        Ok(())
    }

    /// Drop the keys of hosts that no longer accept this device (D2).
    fn retain(&mut self, view: &View) {
        let Some(keys) = self.keys.as_mut() else { return };
        let admitting: Vec<String> = view.chain.hosts_admitting(&view.me).map(|h| h.info.id.clone()).collect();
        let dropped = keys.retain(|h| admitting.iter().any(|a| a == h));
        if self.refused(&dropped) {
            return;
        }
        match dropped {
            Ok(gone) if !gone.is_empty() => tracing::info!(?gone, "dropped the keys of devices that no longer accept this one"),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "cannot drop the keys of devices that no longer accept this one"),
        }
    }

    /// Which hosts declared `policy/1` (for the status; S2 checks it before
    /// opening a channel). Best effort.
    async fn read_policy(&mut self, view: &View) {
        let now = droppen_base::time::now_ms_u64();
        let seq = Some(view.chain.seq());
        if self.policy_seq == seq && now.saturating_sub(self.policy_ms) < ME_EVERY.as_millis() as u64 {
            return;
        }
        match self.client.as_mut().expect("a client").me().await {
            Ok(me) => {
                self.policy = me["machines"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| Some((m["id"].as_str()?.to_string(), m["reads_v3"] == true)))
                    .collect();
                self.policy_ms = now;
                self.policy_seq = seq;
            }
            Err(error) => tracing::debug!(%error, "cannot read which devices declared policy/1"),
        }
    }

    /// Ask every current admin for the keys this device lacks (purpose 4):
    /// the `tries`-th time in a row about gap `name`.
    async fn ask(&mut self, view: &View, gaps: &[(String, u32)], name: &str, tries: u32) -> Result<(), HubError> {
        let admins: Vec<String> = view.chain.admins().map(|d| d.info.id.clone()).filter(|id| id != &view.me).collect();
        let mut asked = Vec::new();
        for admin in admins {
            match self.client.as_mut().expect("a client").ask(&admin).await {
                Ok(()) => asked.push(admin),
                Err(error @ (HubError::Unauthorized | HubError::Unreachable(_) | HubError::Suspended)) => return Err(error),
                Err(error) => tracing::warn!(%error, %admin, "could not ask an admin for keys"),
            }
        }
        let missing: Vec<&str> = gaps.iter().map(|(h, _)| h.as_str()).collect();
        tracing::info!(
            ?asked,
            ?missing,
            tries,
            "asked the account's admins for the content keys this device lacks"
        );
        let now = droppen_base::time::now_ms_u64();
        // The requests took a while: the aggregator may be off, or its keys deleted, by now (S1).
        let Some(keys) = self.keys.as_mut() else { return Ok(()) };
        let noted = keys.set_asked(now, name, tries);
        if !self.refused(&noted) {
            if let Err(error) = noted {
                tracing::warn!(%error, "cannot note the key request");
            }
        }
        Ok(())
    }

    /// While parked after `rotation_pending` ([`Controller::suspended`]) at the
    /// chain version now: how long still. Unparked once the chain moved or
    /// the time is over.
    fn parked(&mut self) -> Option<Duration> {
        let (version, until) = self.suspended?;
        let now = droppen_base::time::now_ms_u64();
        if *self.env.chain.borrow() == version && now < until {
            return Some(Duration::from_millis(until - now));
        }
        self.suspended = None;
        None
    }

    /// Whether a write was refused because the aggregator is off, its link
    /// stopped, or its keys were deleted ([`super::keys::OFF`]) — then the
    /// keys held in memory are dropped: they belong to a directory that is
    /// gone, and must never be written back (security review of S1-4, S1).
    fn refused<T>(&mut self, result: &Result<T, String>) -> bool {
        if matches!(result, Err(e) if e == super::keys::OFF) {
            tracing::info!("the aggregator was turned off, or its keys deleted: dropping what it held");
            self.keys = None;
            return true;
        }
        false
    }

    /// The hub says this device was removed: delete what it held to control
    /// the others, forget its session, and ask again only after
    /// [`REMOVED_EVERY`], quietly (security review of S1-4, S2).
    fn forget_removed(&mut self, view: &View) {
        if self.hub_removed.is_none() {
            tracing::warn!(device = %view.me, "the hub says this device was removed: deleting the other devices' keys here");
        }
        self.keys = None;
        self.client = None;
        self.later.clear();
        self.env.keys_out.send_replace(None);
        if let Err(error) = super::forget_all(&self.env.paths) {
            tracing::error!(%error, "cannot delete the other devices' keys");
        }
        self.hub_removed = Some((view.account.clone(), view.me.clone(), droppen_base::time::now_ms_u64()));
        self.set_status(|s| {
            *s = Status {
                running: true,
                error: Some(HubError::Removed.to_string()),
                ..Status::default()
            }
        });
    }

    /// Hand the keys held to the copies task, when they changed.
    fn publish(&self, view: &View) {
        let ring = self.keys.as_ref().map(|keys| {
            Arc::new(super::copies::KeyRing {
                account: view.account.clone(),
                device: view.me.clone(),
                hosts: keys
                    .hosts()
                    .into_iter()
                    .map(|host| {
                        let held = keys
                            .epochs(&host)
                            .into_iter()
                            .filter_map(|e| keys.key(&host, e).map(|k| (e, *k)))
                            .collect();
                        (host, super::copies::Keys(held))
                    })
                    .collect(),
            })
        });
        self.env.keys_out.send_if_modified(|held| {
            if *held == ring {
                return false;
            }
            *held = ring;
            true
        });
    }

    fn report(&self, view: &View, error: Option<String>) {
        self.publish(view);
        let keys = self.keys.as_ref();
        let hosts = view
            .chain
            .hosts_admitting(&view.me)
            .map(|h| AggregatorHost {
                id: h.info.id.clone(),
                latest_epoch: view.chain.latest_machine_epoch(&h.info.id),
                epochs: keys.map(|k| k.epochs(&h.info.id)).unwrap_or_default(),
                policy: self.policy.get(&h.info.id).copied(),
                copy: None,
            })
            .collect();
        let signed_in = self.client.as_ref().is_some_and(|c| c.signed_in(droppen_base::time::now_ms_u64()));
        let asked_ms = keys.and_then(HostKeys::asked_ms);
        self.set_status(|s| {
            *s = Status {
                running: true,
                signed_in,
                error,
                hosts,
                asked_ms,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rh_proto::chain::{Accept, Action, ChainClock, Flags, KeyCommit, KeyCommits, Signer};
    use rh_proto::keys::DhKeypair;
    use rh_proto::wrap::{encode_host_key_mail, seal_host_key};

    /// An account: admin `d-a`; hosts `m-me` (this computer, not an admin),
    /// `m-2` (its key epoch 0 committed as `[0x22; 32]`) and `m-3` (epoch 0
    /// as `[0x33; 32]`); `m-2` accepts every device.
    struct Account {
        state: ChainState,
        admin: (ed25519_dalek::SigningKey, DhKeypair),
        me: MachineIdentity,
    }

    fn sk(b: u8) -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[b; 32])
    }

    fn commit(host: &str, epoch: u32, key: &[u8; 32]) -> Action {
        Action::KeyEpochs(KeyCommits {
            user: None,
            machines: vec![(
                host.into(),
                KeyCommit {
                    epoch,
                    hash: rh_proto::chain::machine_key_commit("a-1", host, epoch, key),
                },
            )],
        })
    }

    fn account() -> Account {
        let ak = sk(1);
        let admin = (sk(2), DhKeypair::from_secret([3; 32]));
        let device = |id: &str, sign: &ed25519_dalek::SigningKey, dh: [u8; 32], host: bool, admin: bool| rh_proto::chain::DeviceV3 {
            id: id.into(),
            sign_key: sign.verifying_key().to_bytes(),
            dh_key: dh,
            flags: Flags { host, admin },
        };
        let genesis = rh_proto::chain::genesis3_draft(
            "a-1",
            ak.verifying_key().to_bytes(),
            device("d-a", &admin.0, admin.1.public(), false, true),
            None,
            1,
        )
        .sign(&ak);
        let mut clock = ChainClock::history();
        let mut state = ChainState::genesis(&genesis, None).unwrap();
        let me = MachineIdentity::from_seed(&[9; 32]);
        let m2 = MachineIdentity::from_seed(&[10; 32]);
        let m3 = MachineIdentity::from_seed(&[11; 32]);
        let mut push = |state: &mut ChainState, action: Action| {
            let e = state.draft(Signer::Device("d-a".into()), action, 2 + state.seq()).sign(&admin.0);
            state.apply(&e, &mut clock).unwrap();
        };
        for (id, m, key) in [("m-me", &me, [0x11; 32]), ("m-2", &m2, [0x22; 32]), ("m-3", &m3, [0x33; 32])] {
            push(&mut state, Action::AddDevice3(device(id, m.signing_key(), m.dh_public(), true, false)));
            push(&mut state, commit(id, 0, &key));
        }
        push(
            &mut state,
            Action::SetAccept {
                host: "m-2".into(),
                from: Accept::All,
            },
        );
        Account { state, admin, me }
    }

    fn mail(a: &Account, sender: &str, sender_dh: &DhKeypair, host: &str, epoch: u32, key: &[u8; 32]) -> Mail {
        let ctx = HostKeyContext {
            account: "a-1".into(),
            sender: sender.into(),
            recipient: "m-me".into(),
            host: host.into(),
            epoch,
        };
        let sealed = seal_host_key(sender_dh, &a.me.dh_public(), &ctx, key, &mut rand::rngs::OsRng).unwrap();
        Mail {
            id: 1,
            sender: sender.into(),
            purpose: wrap::MAIL_HOST_KEY,
            epoch,
            sealed: encode_host_key_mail(host, &sealed),
        }
    }

    #[test]
    fn a_host_key_is_kept_only_from_an_admin_as_committed_for_a_host_that_accepts_this_device() {
        let a = account();
        let judge = |m: &Mail| judge(&a.state, "a-1", "m-me", a.me.dh(), m);
        // The admin seals m-2's epoch 0: kept.
        match judge(&mail(&a, "d-a", &a.admin.1, "m-2", 0, &[0x22; 32])) {
            Verdict::Keep { host, epoch, key } => assert_eq!((host.as_str(), epoch, key.as_slice()), ("m-2", 0, &[0x22; 32][..])),
            other => panic!("{other:?}"),
        }
        // Another key than the committed one: dropped.
        assert!(matches!(judge(&mail(&a, "d-a", &a.admin.1, "m-2", 0, &[0x99; 32])), Verdict::Drop(_)));
        // m-3 does not accept this device: dropped, even from the admin (D2).
        assert!(matches!(judge(&mail(&a, "d-a", &a.admin.1, "m-3", 0, &[0x33; 32])), Verdict::Drop(_)));
        // From a host (not an admin): dropped.
        let m3 = MachineIdentity::from_seed(&[11; 32]);
        assert!(matches!(judge(&mail(&a, "m-3", m3.dh(), "m-2", 0, &[0x22; 32])), Verdict::Drop(_)));
        // An epoch the chain has not committed yet, or a sender it does not know yet: later.
        assert_eq!(judge(&mail(&a, "d-a", &a.admin.1, "m-2", 1, &[0x23; 32])), Verdict::Later);
        assert_eq!(judge(&mail(&a, "d-new", &a.admin.1, "m-2", 0, &[0x22; 32])), Verdict::Later);
        assert_eq!(judge(&mail(&a, "d-a", &a.admin.1, "m-9", 0, &[0x22; 32])), Verdict::Later);
        // Any other purpose: dropped unread (a user key never comes to a device that manages nothing).
        for purpose in [wrap::MAIL_USER_KEY, wrap::MAIL_PAIRING, wrap::MAIL_KEY_REQUEST, 200] {
            let m = Mail {
                purpose,
                ..mail(&a, "d-a", &a.admin.1, "m-2", 0, &[0x22; 32])
            };
            assert!(matches!(judge(&m), Verdict::Drop(_)), "purpose {purpose}");
        }
    }

    #[test]
    fn gaps_are_the_newest_epochs_of_the_hosts_that_accept_this_device() {
        let a = account();
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "gaps");
        let mut keys = HostKeys::load(&paths, "a-1", "m-me").unwrap();
        assert_eq!(
            gaps(&a.state, "m-me", &keys),
            vec![("m-2".to_string(), 0)],
            "m-3 does not accept it; never itself"
        );
        keys.put("m-2", 0, &[0x22; 32]).unwrap();
        assert!(gaps(&a.state, "m-me", &keys).is_empty());
    }

    /// S1-5: the copies task follows only hosts that accept this device
    /// (D2) and whose keys it holds — never itself, whatever keys it is handed.
    #[test]
    fn only_hosts_that_accept_this_device_and_whose_keys_are_held_are_followed() {
        let a = account();
        let view = View {
            hub: "wss://hub.example".into(),
            account: "a-1".into(),
            me: "m-me".into(),
            chain: a.state.clone(),
        };
        let keys = |e: u32, k: u8| super::super::copies::Keys(vec![(e, [k; 32])]);
        let ring = super::super::copies::KeyRing {
            account: "a-1".into(),
            device: "m-me".into(),
            hosts: [("m-me", keys(0, 1)), ("m-2", keys(0, 0x22)), ("m-3", keys(0, 0x33))]
                .into_iter()
                .map(|(h, k)| (h.to_string(), k))
                .collect(),
        };
        let followed = super::super::copies::followed(&view, &ring);
        assert_eq!(followed.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["m-2"]);
        assert_eq!(followed[0].machine_key, MachineIdentity::from_seed(&[10; 32]).sign_public());
        // No keys of it: not followed either.
        let empty = super::super::copies::KeyRing {
            hosts: Default::default(),
            ..ring.clone()
        };
        assert!(super::super::copies::followed(&view, &empty).is_empty());
    }

    #[test]
    fn a_verdict_never_prints_its_key() {
        let keep = Verdict::Keep {
            host: "m-2".into(),
            epoch: 3,
            key: zeroize::Zeroizing::new(vec![0xab; 32]),
        };
        let printed = format!("{keep:?}");
        assert!(printed.contains("m-2") && printed.contains("[redacted]"), "{printed}");
        assert!(!printed.contains("171") && !printed.to_lowercase().contains("abab"), "{printed}");
    }

    /// Security re-review of S1-4, 2: requests about the same gap back off —
    /// 10 minutes, then an hour, then six hours — and a changed gap starts
    /// again, never sooner than the first step after any request.
    #[test]
    fn key_requests_back_off_per_gap() {
        let m = 60_000u64;
        let t = Timing {
            gap_every: GAP_EVERY,
            idle_every: IDLE_EVERY,
            ask_after: ASK_AFTER,
            ask_backoff: ASK_BACKOFF,
        };
        let gap = "m-2/1";
        assert_eq!(ask_due(10_000, 0, (None, "", 0), gap, &t), None, "the admin's own delivery comes first");
        assert_eq!(ask_due(20_000, 0, (None, "", 0), gap, &t), Some(1));
        let at = 1_000_000;
        let next = |tries: u32, after: u64| ask_due(at + after, 0, (Some(at), gap, tries), gap, &t);
        assert_eq!(next(1, 10 * m - 1), None);
        assert_eq!(next(1, 10 * m), Some(2));
        assert_eq!(next(2, 60 * m - 1), None);
        assert_eq!(next(2, 60 * m), Some(3));
        assert_eq!(next(3, 360 * m - 1), None);
        assert_eq!(next(3, 360 * m), Some(4));
        assert_eq!(next(9, 360 * m), Some(10), "six hours from then on");
        // A day of a gap nobody answers: a handful of requests, not 144.
        let (mut now, mut tries, mut asked, mut count) = (at, 0u32, None, 0);
        while now < at + 24 * 60 * m {
            if let Some(n) = ask_due(now, 0, (asked, gap, tries), gap, &t) {
                tries = n;
                asked = Some(now);
                count += 1;
            }
            now += 5_000;
        }
        assert_eq!(count, 6, "at 0, 10 min, 1 h 10, 7 h 10, 13 h 10, 19 h 10");
        // Another gap (the epoch moved): asked again — but not within the first step of the last request.
        assert_eq!(ask_due(at + 5 * m, 0, (Some(at), gap, 5), "m-2/2", &t), None);
        assert_eq!(ask_due(at + 10 * m, 0, (Some(at), gap, 5), "m-2/2", &t), Some(1));
        // A clock that went back is not a reason to stay silent for good.
        assert_eq!(ask_due(at, 0, (Some(at + 600 * m), gap, 3), gap, &t), Some(4));
        assert_eq!(gap_name(&[("m-2".into(), 1), ("m-3".into(), 4)]), "m-2/1,m-3/4");
    }
}
