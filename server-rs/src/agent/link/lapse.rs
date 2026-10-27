use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rh_proto::chain::{Attestation, ChainClock, ChainError, ChainState};
use rh_proto::driver::Channel;
use rh_proto::frame::Frame;
use tokio::sync::mpsc;

use super::admit::refuse;
use super::hub::now_ms;
use super::state::AgentState;
use super::{Routes, Shared, CHANNEL_STALE, LAPSE_POLL};

/// This machine was removed from the account — its own verified chain says
/// so (a hub refusing its sign-in never gets here): mark it (saved, so the
/// link does not start again), say so, and stop the link (the hub
/// connection, its relays and channels, the streams' writer). The server and
/// its local API go on.
pub(in crate::agent) fn on_removed(shared: &Arc<Shared>, why: &str) {
    let news = shared.state.lock().expect("agent state").mark_removed(now_ms());
    // What it held to control the others (their keys) goes with it (§4.7).
    shared.cancel.cancel();
    if let Err(error) = crate::aggregator::forget_all(&shared.paths) {
        tracing::error!(%error, "cannot delete the other devices' keys after this machine's removal");
    }
    if news {
        shared.save();
        tracing::warn!("this machine was removed from the account ({why}): stopping the hub link");
        crate::push::notify_account_change(
            Arc::new(shared.paths.clone()),
            "Droppen: this device was removed".into(),
            format!("this device was removed from the account ({why}); it no longer connects to Droppen"),
        );
    }
    shared.cancel.cancel();
}

/// Close every open channel whose device the chain no longer lists as current
/// (removed, or never active): a removal takes effect here at once, whatever
/// the hub does, and the handshake refuses the device from now on.
///
/// Chain v3: also every channel whose device this machine no longer accepts
/// control from — an admin demoted while the policy is admins only, a device
/// an admin took off this host's accept list — and every channel at all once
/// this machine stops running AI (`SetHost false`): "who may control me" holds
/// for open channels, not only new ones ([`rh_proto::chain::ChainState::may_control`]).
///
/// Host identity renewal (`docs/host-identity-rotation.md` §3.5, §8 item 8):
/// also every channel whose device's static key is not the one the chain
/// names for it now — a host that renewed its identity key opened it with
/// the old one, which somebody else may hold. The device opens it again
/// with its new key.
pub(in crate::agent) fn drop_removed(shared: &Arc<Shared>, routes: &mut Routes, out: &mpsc::UnboundedSender<Frame>) {
    let gone: Vec<(u32, String, Gone)> = {
        let st = shared.state.lock().expect("agent state");
        let Some(chain) = st.chain.as_ref() else { return };
        routes
            .iter()
            .filter_map(|(ch, (device, key, _))| {
                let why = if chain.device(device).is_none() {
                    Gone::Removed
                } else if chain.paused_edges(&st.machine).iter().any(|id| id == device) {
                    Gone::Paused
                } else if chain.may_control(device, &st.machine).is_err() {
                    Gone::NotAccepted
                } else if !chain.holds_named_key(device, key) {
                    Gone::Renewed
                } else {
                    return None;
                };
                Some((*ch, device.clone(), why))
            })
            .collect()
    };
    for (ch, device, why) in gone {
        routes.remove(&ch);
        match why {
            Gone::Removed => {
                tracing::info!(%device, ch, "closing the channel of a device the chain no longer lists");
                refuse(out, ch, 4403, "this device was removed from the account");
            }
            Gone::NotAccepted => {
                tracing::info!(%device, ch, "closing the channel of a device this machine no longer accepts control from");
                refuse(out, ch, 4403, "the device you reach no longer accepts control from this one");
            }
            Gone::Paused => {
                tracing::info!(%device, ch, "closing the channel of a device whose control is paused while identity keys are renewed");
                refuse(out, ch, 4403, PAUSED_REASON);
            }
            Gone::Renewed => {
                tracing::info!(%device, ch, "closing a channel opened with an identity key the chain no longer names");
                refuse(out, ch, 4403, "this device's identity key was renewed: connect again with the new one");
            }
        }
    }
}

/// What one attestation says for a device's control on a channel already
/// open ([`ChannelLapse`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::agent) enum Word {
    /// It counts, from this time.
    Counts(u64),
    /// It is about a head beyond this view (a catch-up under way, or a hub
    /// holding entries back): it counts for nothing, and takes back nothing
    /// its admin said before.
    Ahead,
    /// It conflicts with this view.
    Fork,
    /// It does not count (see [`word_of`]).
    No,
}

/// What `att` says for `device`'s control of this machine on a channel
/// already open (local-aggregator §3.4). It counts — as
/// [`rh_proto::chain::ChainState::freshness`] counts words for admission,
/// but for a head before this view's too — when all of these hold:
/// - it is by a current admin other than `device`, or by `device` itself
///   while it is the sole admin — or was, as of the head it vouched for (a
///   sole admin pairing a second one keeps its channels until the new one
///   vouches);
/// - it is dated no further ahead than the clock skew allows;
/// - it is of this view's head, or of an earlier head and dated no later
///   (within the skew) than this agent applied the entry after that head. A
///   word about a head this agent had moved past by then only says its
///   admin is behind: a hub that keeps one admin frozen at an old head
///   cannot keep refreshing its word. The history this agent took at
///   enrollment was first seen then (enrollment verifies on a live clock),
///   so a word about a head from before it counts only if dated by then;
///   where no sighting of the entry is kept at all, only the head counts.
pub(in crate::agent) fn word_of(st: &AgentState, chain: &ChainState, att: &Attestation, device: &str, now_ms: u64) -> Word {
    match chain.check_attestation(att) {
        Ok(()) => {}
        Err(ChainError::Behind { .. }) => return Word::Ahead,
        Err(ChainError::Fork { .. }) => return Word::Fork,
        Err(_) => return Word::No,
    }
    let skew = rh_proto::chain::ATTESTATION_SKEW_MS;
    if att.time_ms > now_ms.saturating_add(skew) {
        return Word::No;
    }
    if att.seq < chain.seq() {
        let moved_on = st.clock.first_seen().get(&(att.seq + 1));
        if !moved_on.is_some_and(|&at| att.time_ms <= at.saturating_add(skew)) {
            return Word::No;
        }
    }
    if att.device == device && chain.admins().count() != 1 && cached_sole_admin_at(st, chain, att.seq).as_deref() != Some(device) {
        return Word::No;
    }
    Word::Counts(att.time_ms)
}

/// [`ChainStateExt::sole_admin_at`], worked out once per seq ([`AgentState::sole_admins`]).
pub(in crate::agent) fn cached_sole_admin_at(st: &AgentState, chain: &ChainState, seq: u64) -> Option<String> {
    let hash = chain.hash_at(seq)?;
    let mut known = st.sole_admins.lock().expect("sole admins");
    if let Some((at, sole)) = known.get(&seq) {
        if *at == hash {
            return sole.clone();
        }
    }
    let sole = chain.sole_admin_at(seq);
    known.insert(seq, (hash, sole.clone()));
    sole
}

/// The chain views the control rules judge.
pub(in crate::agent) trait ChainStateExt {
    /// The account's only admin as of entry `seq` of this chain, if it had
    /// exactly one then (this view's entries up to there, verified again).
    fn sole_admin_at(&self, seq: u64) -> Option<String>;

    /// Whether `key` — the static key a channel of `device` was opened with — is
    /// the DH key the chain names for `device` now.
    fn holds_named_key(&self, device: &str, key: &[u8; 32]) -> bool;
}

impl ChainStateExt for ChainState {
    fn sole_admin_at(&self, seq: u64) -> Option<String> {
        let upto = usize::try_from(seq).ok()?.checked_add(1)?;
        let view = ChainState::verify(self.entries().get(..upto)?, None, &mut ChainClock::history()).ok()?;
        let mut admins = view.admins();
        let only = admins.next()?;
        admins.next().is_none().then(|| only.info.id.clone())
    }

    fn holds_named_key(&self, device: &str, key: &[u8; 32]) -> bool {
        self.device_record(device).is_some_and(|d| d.info.dh_key == *key)
    }
}

/// When an open channel's control lapses (local-aggregator §3.4):
/// [`rh_proto::chain::CONTROL_FRESHNESS_MS`] after the newest word that
/// counts for its device ([`word_of`]), kept per admin. A channel is
/// admitted only on a word about exactly this view's head
/// ([`handshake::accept`]); once open, the head moving on before an admin
/// vouches for the new one does not end it, but a view no admin vouched for
/// in seven days does — however long the hub keeps the connection up and a
/// removal to itself. An admin's newer attestation that does not count (of
/// a head beyond this view, or one it was left behind at) does not take
/// back its earlier word; its being an admin no longer does, and so does a
/// fork. Worked out again whenever the chain moved since, and once the
/// lapse it knew has passed — so a stream that races a chain change is
/// judged on the chain as it is.
pub(in crate::agent) struct ChannelLapse {
    device: String,
    /// Admin → the newest time its word counted.
    words: BTreeMap<String, u64>,
    /// The chain seq it was last worked out on.
    seq: Option<u64>,
}

impl ChannelLapse {
    pub(in crate::agent) fn new(device: &str) -> Self {
        Self {
            device: device.to_string(),
            words: BTreeMap::new(),
            seq: None,
        }
    }

    /// When control lapses, as last worked out (`None`: nothing counts).
    pub(in crate::agent) fn at(&self) -> Option<u64> {
        self.words.values().max().map(|t| t.saturating_add(rh_proto::chain::CONTROL_FRESHNESS_MS))
    }

    /// Work it out again on `st` at `now`.
    pub(in crate::agent) fn update(&mut self, st: &AgentState, now: u64) {
        self.seq = st.chain.as_ref().map(ChainState::seq);
        let Some(chain) = st.chain.as_ref() else {
            self.words.clear();
            return;
        };
        self.words.retain(|admin, _| chain.admin(admin).is_some());
        for att in st.attestations.values() {
            match word_of(st, chain, att, &self.device, now) {
                Word::Counts(t) => {
                    let word = self.words.entry(att.device.clone()).or_insert(t);
                    *word = (*word).max(t);
                }
                Word::Fork => {
                    self.words.clear();
                    return;
                }
                Word::Ahead | Word::No => {}
            }
        }
    }

    /// Note what one attestation that just arrived says, before the state
    /// keeps it — where it may replace a word that counted (a sole admin's
    /// renewal, then its first word on a head where it is one of two).
    pub(in crate::agent) fn note(&mut self, st: &AgentState, att: &Attestation, now: u64) {
        let Some(chain) = st.chain.as_ref() else { return };
        if let Word::Counts(t) = word_of(st, chain, att, &self.device, now) {
            let word = self.words.entry(att.device.clone()).or_insert(t);
            *word = (*word).max(t);
        }
    }

    /// Whether control holds at `now`: worked out again first if the chain
    /// moved since, or the lapse it knew has passed.
    pub(in crate::agent) fn holds(&mut self, st: &AgentState, now: u64) -> bool {
        if st.chain.as_ref().map(ChainState::seq) != self.seq || !self.at().is_some_and(|at| now <= at) {
            self.update(st, now);
        }
        self.at().is_some_and(|at| now <= at)
    }
}

/// Whether the device of `lapse` may still control this machine at `now`:
/// the chain lets it ([`rh_proto::chain::ChainState::may_control`]) and its
/// control has not lapsed.
pub(in crate::agent) fn still_controls(shared: &Shared, lapse: &Mutex<ChannelLapse>, now: u64) -> bool {
    let st = shared.state.lock().expect("agent state");
    let mut lapse = lapse.lock().expect("channel lapse");
    let may = st.chain.as_ref().is_some_and(|c| c.may_control(&lapse.device, &st.machine).is_ok());
    lapse.holds(&st, now) && may
}

/// How long an open channel sleeps before it looks at its lapse again: just
/// past it, never more than [`LAPSE_POLL`].
pub(in crate::agent) fn lapse_wait(lapses_at: Option<u64>, now: u64) -> Duration {
    match lapses_at {
        Some(at) => Duration::from_millis(at.saturating_sub(now).saturating_add(1)).min(LAPSE_POLL),
        None => Duration::ZERO,
    }
}

/// End an open channel whose control lapsed: its streams and its direct path
/// with it; the device hears [`CHANNEL_STALE`] and may come back once an admin
/// vouches for this machine's view again.
pub(in crate::agent) fn close_lapsed(channel: &Channel, device: &str, ch: u32) {
    tracing::warn!(%device, ch, "closing a channel: no admin device vouched for this machine's view of the account in seven days");
    channel.close(CHANNEL_STALE, "stale chain view: no admin device vouched for it in seven days; attest and reconnect");
}

/// Forget the routes of channels that ended here (a lapse, a protocol
/// error): neither the hub nor a relay echoes the `CLOSE` this agent sent.
pub(in crate::agent) fn drop_ended(routes: &mut Routes) {
    routes.retain(|_, (_, _, route)| !route.is_closed());
}

/// Why [`drop_removed`] closes a channel.
pub(in crate::agent) enum Gone {
    Removed,
    NotAccepted,
    /// An open renewal pause removes it from this host's effective accept
    /// (§3.4 rule 10): it comes back once the pause ends.
    Paused,
    Renewed,
}

/// The close reason of a channel whose control a renewal pause took away.
pub(in crate::agent) const PAUSED_REASON: &str =
    "control is paused while identity keys are renewed: connect again once the renewal is done";

