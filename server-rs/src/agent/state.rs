//! What an enrolled agent remembers between runs: which hub and account, the
//! trust root it pinned, the device chain it has verified (with its own
//! first sightings of each entry), the replay guard, the attestations it holds,
//! which machine-key epochs it has and which devices ever opened a channel to
//! it ([`Controllers`]).
//!
//! Two files in `<instance dir>/agent/`, both 0600 in a 0700 directory and
//! written atomically:
//!
//! - `state.json` — everything above except key material. The chain is kept
//!   whole and **re-verified from genesis on every load** against the pinned
//!   root, so a tampered file is refused, not trusted.
//! - `machine-keys.json` — the `MK` epochs, hex. Secret, like the machine seed
//!   next to it ([`super::FileSeedStore`]).
//!
//! A missing `state.json` means "not enrolled", and then nothing in this
//! module runs: the server is exactly today's single-machine server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rh_proto::chain::{Attestation, ChainClock, ChainState, Entry, TrustRoot};
use rh_proto::noise::ReplayGuard;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::hub::hex;
use super::link::unhex;
use crate::sessions::Paths;

const VERSION: u32 = 1;

/// `<instance dir>/agent/state.json`.
pub fn state_path(paths: &Paths) -> PathBuf {
    paths.instance_dir().join("agent").join("state.json")
}

/// `<instance dir>/agent/machine-keys.json`.
pub fn keys_path(paths: &Paths) -> PathBuf {
    paths.instance_dir().join("agent").join("machine-keys.json")
}

/// The file's shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StateFile {
    version: u32,
    hub: String,
    account: String,
    machine: String,
    fingerprint: String,
    genesis: Option<String>,
    chain: Vec<String>,
    first_seen: BTreeMap<u64, u64>,
    live_from: u64,
    replay: Vec<(String, String, u64)>,
    /// Whether `replay` is the whole guard (it is saved before every
    /// handshake reply leaves); an older file without it starts guarded.
    #[serde(default)]
    replay_complete: bool,
    attestations: Vec<String>,
    mk_epoch: Option<u32>,
    enrolled_ms: u64,
    /// What devices call this machine (encrypted in its status stream; the hub
    /// only ever knows the machine's id). `None`: the hostname.
    #[serde(default)]
    display_name: Option<String>,
    /// Chain alarms to tell every device about ([`Alarm`]).
    #[serde(default)]
    alarms: Vec<Alarm>,
    /// The newest changes to who is in the account this machine saw
    /// ([`AgentState::note_change`]), for the status stream.
    #[serde(default)]
    changes: Vec<Alarm>,
    /// When this machine learnt from its own chain (`RemoveMachine` naming
    /// it) that it was removed. Informational: [`AgentState::removed`] reads
    /// the chain itself.
    #[serde(default)]
    removed_ms: Option<u64>,
    /// Every device whose handshake this agent accepted, with when it first
    /// did (its own clock): [`AgentState::note_controller`].
    #[serde(default)]
    controllers: BTreeMap<String, u64>,
    /// The chain seq from which `controllers` is complete. `None` in a file
    /// from before the record was kept: it starts at the chain seq of the
    /// load that upgraded it.
    #[serde(default)]
    controllers_from: Option<u64>,
    /// When (this agent's clock) the record became complete.
    #[serde(default)]
    controllers_from_ms: Option<u64>,
    /// This machine's identity renewals as it saw them ([`RotationNote`]).
    #[serde(default)]
    rotation: RotationNote,
}

/// What this machine remembers of renewing its own identity key
/// (`docs/host-identity-rotation.md` §3.5): what the local settings show.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationNote {
    /// When it last took a renewed key (its own clock).
    #[serde(default)]
    pub renewed_ms: Option<u64>,
    /// The fingerprint of the key it renewed away from then.
    #[serde(default)]
    pub previous_fingerprint: Option<String>,
    /// When its own verified chain renewed this machine's identity to keys it
    /// does not hold (`foreign_rotation`): it holds only a retired key.
    #[serde(default)]
    pub foreign_ms: Option<u64>,
    /// Why (`renewed_elsewhere`).
    #[serde(default)]
    pub foreign_reason: Option<String>,
    /// When the hub last refused the key in use as `rotated` with no chain
    /// to show for it (not evidence: it waits to sync, and retries).
    #[serde(default)]
    pub refused_ms: Option<u64>,
    /// When the hub took this machine's dispute statement — its retired
    /// key's signature over the renewal that replaced it elsewhere
    /// ([`super::dispute`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_ms: Option<u64>,
    /// When its own verified chain renewed this machine to keys only a staged
    /// seed that does not read may hold (R6 review): never taken for a
    /// replacement elsewhere; the link stops until the file is put right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable_ms: Option<u64>,
    /// A renewed key was promoted and the push keys have not been renewed
    /// with it yet (R6 review): the next link start makes a new VAPID
    /// keypair and forgets every stored push subscription.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub push_renew: bool,
}

/// What [`AgentState::absorb`] did with a batch of chain entries.
#[derive(Debug)]
pub struct Absorbed {
    /// How many entries it took (the first batch's history included).
    pub applied: usize,
    /// The entries it took that are news — past what it had — to alert about.
    /// Never the first batch, which is the history that let this machine in.
    pub news: Vec<Entry>,
    /// Why it stopped short of the batch's end; what it took before stays.
    pub error: Option<rh_proto::chain::ChainError>,
}

/// The most alarms kept (the newest).
pub const MAX_ALARMS: usize = 8;
/// The longest alarm text kept.
pub const MAX_ALARM_TEXT: usize = 300;
/// The most account changes kept for the status stream (the newest).
pub const MAX_CHANGES: usize = 8;
/// The same alarm again within this long is not raised again.
pub const ALARM_REPEAT_MS: u64 = 60 * 60 * 1000;
/// At most this many alarms of any kind within [`ALARM_REPEAT_MS`]: alarms
/// whose text varies (another device id, another seq) cannot flood every
/// device's notifications (M2 re-review N3).
pub const MAX_ALARMS_PER_WINDOW: usize = 4;

/// The most devices [`AgentState::note_controller`] keeps; one more evicts
/// the one seen first, and the record starts again at the chain seq of then.
pub const MAX_CONTROLLERS: usize = 256;

/// Which devices ever opened a channel to this machine, as this agent saw it
/// (review D3), published in the encrypted status stream as evidence: when a
/// device that could control this machine no longer can, the page asks for
/// the machine to be enrolled again and says whether that device ever
/// connected. Evidence, never a reason to stay quiet: a device that did
/// connect controlled this computer and could have edited this record.
///
/// Complete for every handshake this agent accepted while its chain was at
/// seq `from` or later: a fresh enrollment's record starts at 0 (nothing is
/// accepted before it), one from before the record was kept starts at the
/// seq it was loaded at. `seq` is this agent's chain when the record was
/// read: it refuses a device the chain no longer lets control it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Controllers {
    /// The chain seq from which `devices` is complete.
    pub from: u64,
    /// When (this agent's clock) it became complete.
    pub from_ms: u64,
    /// This agent's chain seq when the record was read.
    pub seq: u64,
    /// Every device it accepted a handshake from (since `from` at least), by id.
    pub devices: Vec<Controller>,
}

/// One device in [`Controllers`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    /// Its device id.
    pub id: String,
    /// When this agent first accepted its handshake (its own clock).
    pub first_ms: u64,
}

/// What [`AgentState::note_controller`] changed, to take back.
#[derive(Debug)]
struct Noted {
    device: String,
    evicted: Vec<(String, u64)>,
    from: u64,
    from_ms: u64,
}

/// What [`AgentState::record_controller`] did with a device let in by a
/// handshake.
#[derive(Debug, PartialEq, Eq)]
pub enum Recorded {
    /// New to the record, and saved: the status should say so.
    New,
    /// On the record already, and saved.
    Known,
    /// New to the record, and the save failed: taken back — it is not let
    /// in, and its next handshake records (and saves) it again.
    Unsaved(String),
    /// On the (saved) record already; this save failed: let in all the same.
    KnownUnsaved(String),
}

/// Something every device must hear about: this agent was served a device
/// chain that conflicts with its own (a fork, a rollback, another genesis).
/// Kept in `state.json`, published in the encrypted status stream and on
/// `/rh/info`, and pushed (M2 re-review: it used to be logged only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alarm {
    /// When this agent raised it (its own clock).
    pub at_ms: u64,
    /// What it saw, in words.
    pub what: String,
}

/// The agent's state, verified.
pub struct AgentState {
    /// The hub's origin, as enrolled with.
    pub hub: String,
    /// The account this machine belongs to.
    pub account: String,
    /// This machine's id in the account.
    pub machine: String,
    /// The pinned trust root: the code's fingerprint, then genesis too.
    pub root: TrustRoot,
    /// `None` only while an enrollment waits for its "add machine" entry.
    pub chain: Option<ChainState>,
    /// This agent's own clock: first sightings of chain entries.
    pub clock: ChainClock,
    /// The newest attestation from each device.
    pub attestations: BTreeMap<String, Attestation>,
    /// The newest machine-key epoch held, if any.
    pub mk_epoch: Option<u32>,
    /// The name devices show for this machine, if a device set one.
    pub display_name: Option<String>,
    /// The newest alarms ([`Alarm`]), oldest first.
    pub alarms: Vec<Alarm>,
    /// The newest changes to who is in the account (machines and devices
    /// added or removed, recovery joins…), oldest first; published in the
    /// status stream so every device reads what this machine saw.
    pub changes: Vec<Alarm>,
    /// When this machine learnt it was removed from the account (see
    /// [`AgentState::removed`]).
    pub removed_ms: Option<u64>,
    /// Its identity renewals ([`RotationNote`]).
    pub rotation: RotationNote,
    /// Every device whose handshake it accepted → when it first did.
    controllers: BTreeMap<String, u64>,
    /// The chain seq from which `controllers` is complete ([`Controllers`]),
    /// and when.
    controllers_from: u64,
    controllers_from_ms: u64,
    /// The first seq this agent saw arrive live; history before it is taken
    /// as it is (see [`ChainClock`]).
    live_from: u64,
    replay: Vec<(String, [u8; 32], u64)>,
    replay_complete: bool,
    enrolled_ms: u64,
    /// The account's only admin as of each chain seq where an open channel
    /// asked (`agent::link`), with the hash of the entry it was worked out on:
    /// entries do not change under a seq, so it is worked out once. Not saved.
    pub(crate) sole_admins: std::sync::Mutex<BTreeMap<u64, ([u8; 32], Option<String>)>>,
    /// A test's hook between noting a device and saving it
    /// ([`AgentState::record_controller`]): what a concurrent handshake can
    /// see in between, if anything.
    #[cfg(test)]
    pub(crate) between_note_and_save: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl AgentState {
    /// A new enrollment: nothing verified yet but the pinned fingerprint.
    pub fn enrolling(hub: &str, account: &str, machine: &str, fingerprint: [u8; 16], now_ms: u64) -> Self {
        Self {
            hub: hub.into(),
            account: account.into(),
            machine: machine.into(),
            root: TrustRoot::from_fingerprint(fingerprint),
            chain: None,
            clock: ChainClock::live(now_ms, 0),
            attestations: BTreeMap::new(),
            mk_epoch: None,
            display_name: None,
            alarms: Vec::new(),
            changes: Vec::new(),
            removed_ms: None,
            rotation: RotationNote::default(),
            controllers: BTreeMap::new(),
            // Nothing can have been accepted before enrollment: the record is whole.
            controllers_from: 0,
            controllers_from_ms: now_ms,
            live_from: 0,
            replay: Vec::new(),
            // Nothing can have been accepted before enrollment.
            replay_complete: true,
            enrolled_ms: now_ms,
            sole_admins: Default::default(),
            #[cfg(test)]
            between_note_and_save: None,
        }
    }

    /// Whether the chain names this machine as a current device of the
    /// account (enrollment is complete) — whether or not it runs AI now: a
    /// member that stopped running AI (chain v3 `SetHost false`) is still
    /// enrolled ([`AgentState::not_hosting`]), and nothing takes it for an
    /// enrollment waiting for a device (chain v3 review, finding 12).
    pub fn enrolled(&self) -> bool {
        self.chain.as_ref().is_some_and(|c| c.device(&self.machine).is_some())
    }

    /// Whether this machine runs AI in its account now (a current host): only
    /// then does it write its encrypted streams and accept channels.
    pub fn hosting(&self) -> bool {
        self.chain.as_ref().is_some_and(|c| c.machine(&self.machine).is_some())
    }

    /// Whether this machine's own chain removed it (`RemoveMachine` or
    /// `RemoveDevice3` naming it).
    pub fn removed_in_chain(&self) -> bool {
        self.chain
            .as_ref()
            .and_then(|c| c.device_record(&self.machine))
            .is_some_and(|d| d.status == rh_proto::chain::DeviceStatus::Removed)
    }

    /// Whether this machine is in its chain but no longer runs AI (chain v3
    /// `SetHost false`): it writes no streams and accepts no channels, but
    /// is still a device of the account — its state stays, and it runs AI
    /// again once the chain makes it a host again.
    pub fn not_hosting(&self) -> bool {
        self.enrolled() && !self.hosting()
    }

    /// Whether this machine was removed from the account: only ever by its
    /// own verified chain (`RemoveMachine` naming it). A hub refusing its
    /// sign-in is not a removal — a hub that lies, or is wrong, could
    /// otherwise turn a healthy machine off for good.
    pub fn removed(&self) -> bool {
        self.removed_in_chain()
    }

    /// Mark this machine removed (once); `true` when that is news.
    pub fn mark_removed(&mut self, now_ms: u64) -> bool {
        if self.removed_ms.is_some() {
            return false;
        }
        self.removed_ms = Some(now_ms);
        true
    }

    /// Whether the chain added this machine with exactly `sign`/`dh` (an
    /// enrollment completing with other keys is refused, not served).
    pub fn keys_match(&self, sign: &[u8; 32], dh: &[u8; 32]) -> bool {
        self.chain
            .as_ref()
            .and_then(|c| c.device(&self.machine))
            .is_some_and(|m| &m.info.sign_key == sign && &m.info.dh_key == dh)
    }

    /// Keep an account change (newest last, at most [`MAX_CHANGES`]).
    pub fn note_change(&mut self, now_ms: u64, what: &str) {
        self.changes.push(Alarm {
            at_ms: now_ms,
            what: what.chars().take(MAX_ALARM_TEXT).collect(),
        });
        let excess = self.changes.len().saturating_sub(MAX_CHANGES);
        self.changes.drain(..excess);
    }

    /// Record that `device`'s handshake was accepted (a channel opens, on the
    /// hub, a relay or a direct path) and save the state — its replay guard
    /// with it — before the reply leaves, so a device that got in is never
    /// missing from the record on disk. Noting, saving and (when the save
    /// fails) taking the note back are one step under whatever lock holds
    /// this state: what is in memory is only ever what was saved, so "on the
    /// record already" never means another handshake's note that is not on
    /// disk (review: concurrent hub, relay and direct admissions).
    pub fn record_controller(&mut self, paths: &Paths, device: &str, now_ms: u64) -> Recorded {
        let noted = self.note_controller(device, now_ms);
        #[cfg(test)]
        if let Some(hook) = self.between_note_and_save.clone() {
            hook();
        }
        match (self.save(paths), noted) {
            (Ok(()), Some(_)) => Recorded::New,
            (Ok(()), None) => Recorded::Known,
            (Err(error), Some(noted)) => {
                self.unnote_controller(noted);
                Recorded::Unsaved(error)
            }
            (Err(error), None) => Recorded::KnownUnsaved(error),
        }
    }

    /// Note `device` among those that opened a channel; what changed when
    /// that is news — a device not seen before — to take back with
    /// [`AgentState::unnote_controller`]. At [`MAX_CONTROLLERS`] the device
    /// seen first makes room, and the record says it is complete only from
    /// the chain's seq now on. Only [`AgentState::record_controller`] calls
    /// it, with the save.
    fn note_controller(&mut self, device: &str, now_ms: u64) -> Option<Noted> {
        if self.controllers.contains_key(device) {
            return None;
        }
        let mut noted = Noted {
            device: device.to_string(),
            evicted: Vec::new(),
            from: self.controllers_from,
            from_ms: self.controllers_from_ms,
        };
        while self.controllers.len() >= MAX_CONTROLLERS {
            let first = self.controllers.iter().min_by_key(|(id, at)| (**at, (*id).clone())).map(|(id, at)| (id.clone(), *at));
            let Some(first) = first else { break };
            self.controllers.remove(&first.0);
            noted.evicted.push(first);
            self.controllers_from = self.controllers_from.max(self.seq().unwrap_or(0));
            self.controllers_from_ms = now_ms;
        }
        self.controllers.insert(device.to_string(), now_ms);
        Some(noted)
    }

    /// Take back a [`AgentState::note_controller`] that could not be saved
    /// (in the same step: nothing else changed the record in between).
    fn unnote_controller(&mut self, noted: Noted) {
        self.controllers.remove(&noted.device);
        for (id, at) in noted.evicted {
            self.controllers.entry(id).or_insert(at);
        }
        self.controllers_from = noted.from;
        self.controllers_from_ms = noted.from_ms;
    }

    /// Which devices ever opened a channel to this machine, with this
    /// agent's chain as it is now; `None` before it has a chain.
    pub fn controllers(&self) -> Option<Controllers> {
        let chain = self.chain.as_ref()?;
        Some(Controllers {
            from: self.controllers_from,
            from_ms: self.controllers_from_ms,
            seq: chain.seq(),
            devices: self
                .controllers
                .iter()
                .map(|(id, at)| Controller {
                    id: id.clone(),
                    first_ms: *at,
                })
                .collect(),
        })
    }

    /// The device chain's seq and how many current devices and machines it
    /// lists, for `agent status` and the local API.
    pub fn summary(&self) -> Option<(u64, usize, usize)> {
        let chain = self.chain.as_ref()?;
        let devices = chain.devices().filter(|d| d.is_active() && !d.host).count();
        let machines = chain.devices().filter(|d| d.is_current_host()).count();
        Some((chain.seq(), devices, machines))
    }

    /// When the enrollment began (the hub assigned the machine id).
    pub fn enrolled_ms(&self) -> u64 {
        self.enrolled_ms
    }

    /// Apply chain entries from the hub (or a device). The first batch must
    /// start at genesis and verify against the pinned fingerprint, which then
    /// pins the genesis hash too; later batches apply as far as they verify
    /// ([`ChainState::merge_partial`]).
    ///
    /// What took effect is reported even when the batch stops short (M2
    /// re-review): the caller alerts about every entry in `news`, closes the
    /// channels of devices it removed and saves — a join followed by a spoiled
    /// entry, or by a self-activation that is too early here, is still a join.
    pub fn absorb(&mut self, entries: &[Entry], now_ms: u64) -> Absorbed {
        self.clock.set_now(now_ms);
        match &mut self.chain {
            Some(chain) => {
                let before = chain.seq();
                let merged = chain.merge_partial(entries, &mut self.clock);
                Absorbed {
                    applied: merged.applied,
                    news: chain.entries_after(Some(before)).to_vec(),
                    error: merged.error,
                }
            }
            None => {
                let refused = |error| Absorbed {
                    applied: 0,
                    news: Vec::new(),
                    error: Some(error),
                };
                // The history before enrollment is the account that let this
                // machine in; only what arrives from now on is gated by time.
                let live_from = entries.last().map_or(0, |e| e.seq() + 1);
                let mut clock = ChainClock::live(now_ms, live_from);
                let chain = match ChainState::verify_rooted(entries, &self.root, &mut clock) {
                    Ok(chain) => chain,
                    Err(error) => return refused(error),
                };
                if chain.account() != self.account {
                    return refused(rh_proto::chain::ChainError::WrongAccount { seq: 0 });
                }
                self.clock = clock;
                self.live_from = live_from;
                self.root = self.root.pinned_to(&chain);
                let n = chain.entries().len();
                self.chain = Some(chain);
                // History, not news: nothing in it is an alert.
                Absorbed {
                    applied: n,
                    news: Vec::new(),
                    error: None,
                }
            }
        }
    }

    /// The chain entries after seq `seq` (`None`: all), as this view has them.
    pub fn entries_after(&self, seq: Option<u64>) -> Vec<Entry> {
        self.chain.as_ref().map(|c| c.entries_after(seq).to_vec()).unwrap_or_default()
    }

    /// The view's seq, if it has a chain.
    pub fn seq(&self) -> Option<u64> {
        self.chain.as_ref().map(ChainState::seq)
    }

    /// Note an alarm (a chain that conflicts with this agent's) to tell every
    /// device about. `false` when the same alarm was noted within
    /// [`ALARM_REPEAT_MS`] (a hub serving a fork on every reconnect raises it
    /// once an hour, not once a reconnect).
    pub fn note_alarm(&mut self, now_ms: u64, what: &str) -> bool {
        let recent = |a: &&Alarm| now_ms.saturating_sub(a.at_ms) < ALARM_REPEAT_MS;
        if self.alarms.iter().filter(recent).any(|a| a.what == what) {
            return false;
        }
        if self.alarms.iter().filter(recent).count() >= MAX_ALARMS_PER_WINDOW {
            tracing::warn!("ALARM (not raised again: {MAX_ALARMS_PER_WINDOW} alarms within the hour already): {what}");
            return false;
        }
        self.alarms.push(Alarm {
            at_ms: now_ms,
            what: what.chars().take(MAX_ALARM_TEXT).collect(),
        });
        let excess = self.alarms.len().saturating_sub(MAX_ALARMS);
        self.alarms.drain(..excess);
        true
    }

    /// Keep the newest attestation per device that checks out against the
    /// chain (or is from beyond it — catching up will tell).
    pub fn attest(&mut self, att: Attestation) -> bool {
        let Some(chain) = &self.chain else { return false };
        match chain.check_attestation(&att) {
            Ok(()) | Err(rh_proto::chain::ChainError::Behind { .. }) => {}
            Err(_) => return false,
        }
        let newer = self.attestations.get(&att.device).is_none_or(|old| old.time_ms <= att.time_ms);
        if newer {
            self.attestations.insert(att.device.clone(), att);
        }
        newer
    }

    /// Every attestation held, for a handshake's freshness check.
    pub fn attestation_list(&self) -> Vec<Attestation> {
        self.attestations.values().cloned().collect()
    }

    /// The replay guard for this run. The saved one is complete — the link
    /// saves it before a handshake reply leaves — so it needs none of the
    /// start-up refusal window a guard with no memory has
    /// ([`ReplayGuard::after_restart`]), which only state from before that
    /// rule gets.
    pub fn replay_guard(&self, now_ms: u64) -> ReplayGuard {
        if !self.replay_complete {
            return ReplayGuard::after_restart(now_ms);
        }
        let mut guard = ReplayGuard::new(
            rh_proto::noise::DEFAULT_PAST_WINDOW_MS,
            rh_proto::noise::DEFAULT_FUTURE_WINDOW_MS,
            rh_proto::noise::DEFAULT_PER_DEVICE,
            0,
        );
        guard.restore(self.replay.iter().cloned());
        guard
    }

    /// Remember the replay guard's contents (saved with the rest).
    pub fn save_guard(&mut self, guard: &ReplayGuard) {
        self.replay = guard.snapshot();
        self.replay_complete = true;
    }

    /// The saved state, re-verified; `None` when this instance was never enrolled.
    pub fn load(paths: &Paths) -> Result<Option<Self>, String> {
        let path = state_path(paths);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        super::check_private(&path)?;
        let file: StateFile = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        if file.version != VERSION {
            return Err(format!("{}: unsupported version {}", path.display(), file.version));
        }
        let bad = |what: &str| format!("{}: bad {what}", path.display());
        let fingerprint: [u8; 16] = unhex(&file.fingerprint)
            .and_then(|f| f.try_into().ok())
            .ok_or_else(|| bad("fingerprint"))?;
        let genesis = match &file.genesis {
            Some(g) => Some(unhex(g).and_then(|g| <[u8; 32]>::try_from(g).ok()).ok_or_else(|| bad("genesis pin"))?),
            None => None,
        };
        let root = TrustRoot {
            account_fingerprint: fingerprint,
            genesis_hash: genesis,
        };
        let mut clock = ChainClock::live(crate::agent::now_ms(), file.live_from);
        clock.restore(file.first_seen.clone());
        let chain = if file.chain.is_empty() {
            None
        } else {
            let entries = file
                .chain
                .iter()
                .map(|e| {
                    unhex(e)
                        .ok_or_else(|| bad("chain entry"))
                        .and_then(|b| Entry::decode(&b).map_err(|e| format!("{}: {e}", path.display())))
                })
                .collect::<Result<Vec<_>, _>>()?;
            // Re-verified from genesis with the agent's own clock and pins.
            let chain = ChainState::verify_rooted(&entries, &root, &mut clock)
                .map_err(|e| format!("{}: the saved chain does not verify: {e}", path.display()))?;
            Some(chain)
        };
        let attestations = file
            .attestations
            .iter()
            .filter_map(|a| unhex(a).and_then(|b| Attestation::decode(&b).ok()))
            .map(|a| (a.device.clone(), a))
            .collect();
        let replay = file
            .replay
            .iter()
            .filter_map(|(d, e, x)| Some((d.clone(), unhex(e).and_then(|e| <[u8; 32]>::try_from(e).ok())?, *x)))
            .collect();
        // A file from before the record was kept: it is complete from now on.
        let (controllers_from, controllers_from_ms) = match file.controllers_from {
            Some(from) => (from, file.controllers_from_ms.unwrap_or(file.enrolled_ms)),
            None => (chain.as_ref().map_or(0, ChainState::seq), crate::agent::now_ms()),
        };
        Ok(Some(Self {
            hub: file.hub,
            account: file.account,
            machine: file.machine,
            root,
            chain,
            clock,
            attestations,
            mk_epoch: file.mk_epoch,
            display_name: file.display_name,
            alarms: file.alarms,
            changes: file.changes,
            removed_ms: file.removed_ms,
            rotation: file.rotation,
            controllers: file.controllers,
            controllers_from,
            controllers_from_ms,
            live_from: file.live_from,
            replay,
            replay_complete: file.replay_complete,
            enrolled_ms: file.enrolled_ms,
            sole_admins: Default::default(),
            #[cfg(test)]
            between_note_and_save: None,
        }))
    }

    /// Write the state (0600, atomically).
    pub fn save(&self, paths: &Paths) -> Result<(), String> {
        let file = StateFile {
            version: VERSION,
            hub: self.hub.clone(),
            account: self.account.clone(),
            machine: self.machine.clone(),
            fingerprint: hex(&self.root.account_fingerprint),
            genesis: self.root.genesis_hash.map(|g| hex(&g)),
            chain: self.chain.iter().flat_map(|c| c.entries().iter().map(|e| hex(&e.encode()))).collect(),
            first_seen: self.clock.first_seen().clone(),
            live_from: self.live_from,
            replay: self.replay.iter().map(|(d, e, x)| (d.clone(), hex(e), *x)).collect(),
            replay_complete: self.replay_complete,
            attestations: self.attestations.values().map(|a| hex(&a.encode())).collect(),
            mk_epoch: self.mk_epoch,
            enrolled_ms: self.enrolled_ms,
            display_name: self.display_name.clone(),
            alarms: self.alarms.clone(),
            changes: self.changes.clone(),
            removed_ms: self.removed_ms,
            controllers: self.controllers.clone(),
            controllers_from: Some(self.controllers_from),
            controllers_from_ms: Some(self.controllers_from_ms),
            rotation: self.rotation.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        super::write_private(&state_path(paths), &bytes)
    }
}

/// The machine keys this agent holds, by epoch. Secret.
#[derive(Default, Serialize, Deserialize)]
struct KeysFile {
    epochs: BTreeMap<u32, String>,
}

/// Store machine-key epoch `epoch`.
pub fn store_machine_key(paths: &Paths, epoch: u32, key: &[u8]) -> Result<(), String> {
    let path = keys_path(paths);
    let mut file: KeysFile = match std::fs::read(&path) {
        Ok(bytes) => {
            super::check_private(&path)?;
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => KeysFile::default(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    file.epochs.insert(epoch, hex(key));
    let bytes = Zeroizing::new(serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?);
    super::write_private(&path, &bytes)
}

/// Every machine-key epoch this agent holds (empty until a device delivered one).
pub fn load_machine_keys(paths: &Paths) -> Result<BTreeMap<u32, Zeroizing<[u8; 32]>>, String> {
    let path = keys_path(paths);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    super::check_private(&path)?;
    let file: KeysFile = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    file.epochs
        .iter()
        .map(|(epoch, key)| {
            let raw = Zeroizing::new(unhex(key).ok_or_else(|| format!("{}: a key that is not hex", path.display()))?);
            let key: [u8; 32] = raw
                .as_slice()
                .try_into()
                .map_err(|_| format!("{}: a key that is not 32 bytes", path.display()))?;
            Ok((*epoch, Zeroizing::new(key)))
        })
        .collect()
}

/// What an earlier enrollment of this instance left that is not the new
/// one's: its machine keys (`machine-keys.json`) and its streams' store
/// (`streams.db`). A new enrollment mints a new machine id with new keys; the
/// old file's epochs must never be what the writer seals under (M2 re-review).
/// Gone files are fine.
pub fn forget_enrollment_leftovers(paths: &Paths) -> Result<(), String> {
    let store = super::streams::store::path(paths);
    // The process's connection to the old store goes with it, or the next
    // open would hand it back (the file deleted, its rows not).
    crate::db::forget(&store);
    let mut leftovers = vec![keys_path(paths), store.clone()];
    for suffix in ["-wal", "-shm"] {
        let mut name = store.clone().into_os_string();
        name.push(suffix);
        leftovers.push(PathBuf::from(name));
    }
    for path in leftovers {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!(path = %path.display(), "removed what an earlier enrollment left"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(())
}

/// Whether `path`'s parent exists — enrollment state is per instance.
pub fn exists(paths: &Paths) -> bool {
    Path::new(&state_path(paths)).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rh_proto::chain::{genesis_draft, Action, DeviceInfo, Signer};
    use rh_proto::identity::{fingerprint, MachineIdentity};

    /// genesis (AK, device) → add machine; and the machine's identity.
    fn chain() -> (Vec<Entry>, [u8; 16], MachineIdentity) {
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let device = MachineIdentity::from_seed(&[2; 32]);
        let machine = MachineIdentity::from_seed(&[3; 32]);
        let info = DeviceInfo {
            id: "d-1".into(),
            sign_key: device.sign_public(),
            dh_key: device.dh_public(),
            full: true,
        };
        let genesis = genesis_draft("a-1", ak.sign_public(), info, None, 1_000).sign(ak.signing_key());
        let mut state = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let add = state
            .draft(Signer::Device("d-1".into()), Action::AddMachine(machine.info("m-1")), 2_000)
            .sign(device.signing_key());
        state.apply(&add, &mut ChainClock::history()).unwrap();
        (vec![genesis, add], fingerprint(&ak.sign_public()), machine)
    }

    #[test]
    fn an_enrolled_state_round_trips_and_is_re_verified_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentstate");
        let (entries, fp, _) = chain();
        assert!(AgentState::load(&paths).unwrap().is_none(), "never enrolled");
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 5_000);
        assert_eq!(st.absorb(&entries[..1], 5_000).applied, 1);
        assert!(
            !st.enrolled() && st.root.genesis_hash == Some(entries[0].hash()),
            "genesis pinned at once"
        );
        let second = st.absorb(&entries, 6_000);
        assert_eq!((second.applied, second.news.len(), second.error), (1, 1, None));
        assert!(st.enrolled());
        let mut guard = st.replay_guard(6_000);
        guard.forget("nobody");
        st.save_guard(&guard);
        st.save(&paths).unwrap();
        let loaded = AgentState::load(&paths).unwrap().unwrap();
        assert!(loaded.enrolled());
        assert_eq!(loaded.root, st.root);
        assert_eq!(loaded.clock.first_seen().get(&1), Some(&6_000), "the agent's own first sighting survives");

        // A saved chain that no longer verifies is refused, not trusted.
        let path = state_path(&paths);
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let tampered = v["chain"][1].as_str().unwrap().replacen('0', "1", 1);
        v["chain"][1] = serde_json::Value::String(tampered);
        super::super::write_private(&path, &serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(AgentState::load(&paths).is_err());
    }

    #[test]
    fn a_chain_from_another_account_key_or_genesis_is_refused() {
        let (entries, fp, _) = chain();
        let mut wrong_fp = AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 0);
        assert!(wrong_fp.absorb(&entries, 0).error.is_some());
        let mut pinned = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 0);
        assert!(pinned.absorb(&entries, 0).error.is_none());
        // The same AK (as if it had leaked) signing a second genesis.
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let intruder = MachineIdentity::from_seed(&[7; 32]);
        let other = DeviceInfo {
            id: "d-evil".into(),
            sign_key: intruder.sign_public(),
            dh_key: intruder.dh_public(),
            full: true,
        };
        let other_genesis = genesis_draft("a-1", ak.sign_public(), other, None, 1_000).sign(ak.signing_key());
        let mut unpinned = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 0);
        assert!(
            unpinned.absorb(std::slice::from_ref(&other_genesis), 0).error.is_none(),
            "the fingerprint alone lets it through"
        );
        let mut fresh = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 0);
        fresh.root = pinned.root;
        assert!(fresh.absorb(&[other_genesis], 0).error.is_some(), "the genesis hash is pinned too");
    }

    /// The partial-merge PoC, at the agent: the hub sends `[RecoveryJoin,
    /// <a spoiled entry>]`. The join took effect, so it is news — alerted
    /// about, saved, with this agent's own first sighting — though the batch
    /// stopped short. (It used to come back as a bare `Err`, and the link
    /// raised nothing, closed nothing and saved nothing.)
    #[test]
    fn a_batch_that_stops_short_still_reports_and_keeps_what_took_effect() {
        use rh_proto::chain::{recovery_keys, RecoveryKey};
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentpartial");
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let device = MachineIdentity::from_seed(&[2; 32]);
        let machine = MachineIdentity::from_seed(&[3; 32]);
        let thief = MachineIdentity::from_seed(&[4; 32]);
        let rk = recovery_keys(&[7; 16]);
        let info = |id: &str, k: &MachineIdentity| DeviceInfo {
            id: id.into(),
            sign_key: k.sign_public(),
            dh_key: k.dh_public(),
            full: true,
        };
        let genesis = genesis_draft(
            "a-1",
            ak.sign_public(),
            info("d-1", &device),
            Some(RecoveryKey::new(&rk.signing, "a-1", &ak.sign_public())),
            1_000,
        )
        .sign(ak.signing_key());
        let mut hub = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let add = hub
            .draft(Signer::Device("d-1".into()), Action::AddMachine(machine.info("m-1")), 2_000)
            .sign(device.signing_key());
        hub.apply(&add, &mut ChainClock::history()).unwrap();
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fingerprint(&ak.sign_public()), 5_000);
        assert!(st.absorb(&[genesis, add], 5_000).news.is_empty(), "enrollment's history is no alert");

        let join = hub
            .draft(Signer::Recovery, Action::RecoveryJoin(info("d-thief", &thief)), 3_000)
            .sign(&rk.signing);
        hub.apply(&join, &mut ChainClock::history()).unwrap();
        let mut bogus = hub
            .draft(Signer::Device("d-1".into()), Action::RemoveMachine("m-1".into()), 4_000)
            .sign(device.signing_key());
        bogus.signature[0] ^= 1;
        let absorbed = st.absorb(&[join.clone(), bogus], 9_000);
        assert_eq!(absorbed.applied, 1);
        assert_eq!(absorbed.news, vec![join], "the join is news to act on");
        assert!(matches!(absorbed.error, Some(rh_proto::chain::ChainError::BadSignature { seq: 3 })));
        st.save(&paths).unwrap();
        let loaded = AgentState::load(&paths).unwrap().unwrap();
        assert!(loaded.chain.as_ref().unwrap().device_record("d-thief").is_some(), "kept");
        assert_eq!(loaded.clock.first_seen().get(&2), Some(&9_000), "first seen by this agent, then");
    }

    /// A host enrolled into an account made on chain v3, as the web app makes
    /// every new account (`Genesis3`, then user key 0 as v3): the fingerprint
    /// and the genesis pin it, its own enrollment (`AddDevice3{host}` and its
    /// first key) is history, not news, and it hosts; the devices added after
    /// are news; it admits the admin and the devices its accept list names,
    /// nobody else; and the state is verified again on load.
    #[test]
    fn a_host_of_an_account_made_on_chain_v3_follows_it_and_admits_its_devices() {
        use rh_proto::chain::{genesis3_draft, machine_key_commit, user_key_commit, Accept, DeviceV3, Flags, KeyCommit, KeyCommits};
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentv3genesis");
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let admin = MachineIdentity::from_seed(&[2; 32]);
        let machine = MachineIdentity::from_seed(&[3; 32]);
        let phone = MachineIdentity::from_seed(&[4; 32]);
        let other = MachineIdentity::from_seed(&[5; 32]);
        let device = |id: &str, k: &MachineIdentity, host: bool, admin: bool| DeviceV3 {
            id: id.into(),
            sign_key: k.sign_public(),
            dh_key: k.dh_public(),
            flags: Flags { host, admin },
        };
        let genesis = genesis3_draft("a-1", ak.sign_public(), device("d-1", &admin, false, true), None, 1_000).sign(ak.signing_key());
        let mut hub = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let mut entries = vec![genesis];
        let push = |hub: &mut ChainState, entries: &mut Vec<Entry>, action: Action| {
            let e = hub
                .draft(Signer::Device("d-1".into()), action, 2_000 + hub.seq())
                .sign(admin.signing_key());
            hub.apply(&e, &mut ChainClock::history()).unwrap();
            entries.push(e);
        };
        let user_key = KeyCommit {
            epoch: 0,
            hash: user_key_commit("a-1", 0, &[7; 32]),
        };
        push(
            &mut hub,
            &mut entries,
            Action::KeyEpochs(KeyCommits {
                user: Some(user_key),
                machines: Vec::new(),
            }),
        );
        push(&mut hub, &mut entries, Action::AddDevice3(device("m-1", &machine, true, false)));
        let first_mk = KeyCommit {
            epoch: 0,
            hash: machine_key_commit("a-1", "m-1", 0, &[8; 32]),
        };
        push(
            &mut hub,
            &mut entries,
            Action::KeyEpochs(KeyCommits {
                user: None,
                machines: vec![("m-1".into(), first_mk)],
            }),
        );
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fingerprint(&ak.sign_public()), 5_000);
        let enrolled = st.absorb(&entries, 5_000);
        assert_eq!((enrolled.applied, enrolled.news.len(), enrolled.error), (4, 0, None), "history, not news");
        assert!(st.enrolled() && st.hosting());
        assert_eq!(st.root.genesis_hash, Some(entries[0].hash()), "the v3 genesis pinned");
        let at = entries.len();
        push(&mut hub, &mut entries, Action::AddDevice3(device("d-2", &phone, false, false)));
        push(&mut hub, &mut entries, Action::AddDevice3(device("d-3", &other, false, false)));
        push(
            &mut hub,
            &mut entries,
            Action::SetAccept {
                host: "m-1".into(),
                from: Accept::Devices(vec!["d-2".into()]),
            },
        );
        let later = st.absorb(&entries[at..], 6_000);
        assert_eq!((later.applied, later.news.len(), later.error), (3, 3, None));
        assert!(entries.iter().all(Entry::is_v3), "nothing but v3");
        let chain = st.chain.as_ref().unwrap();
        assert_eq!(chain.v3_from(), Some(0));
        chain.authorize_control("d-1", &admin.dh_public(), "m-1").expect("the admin");
        chain.authorize_control("d-2", &phone.dh_public(), "m-1").expect("a device it accepts");
        assert!(chain.authorize_control("d-3", &other.dh_public(), "m-1").is_err(), "one it does not");
        assert!(chain.authorize_control("m-1", &machine.dh_public(), "m-1").is_err(), "never itself");
        st.save(&paths).unwrap();
        let loaded = AgentState::load(&paths).unwrap().unwrap();
        assert!(loaded.enrolled() && loaded.hosting());
        assert_eq!(loaded.seq(), Some(entries.len() as u64 - 1));
    }

    #[test]
    fn alarms_are_kept_bounded_and_not_repeated_within_the_hour() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentalarms");
        let (entries, fp, _) = chain();
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 0);
        st.absorb(&entries, 0);
        assert!(st.note_alarm(1_000, "the hub served a fork"));
        assert!(!st.note_alarm(2_000, "the hub served a fork"), "the same alarm within the hour");
        assert!(st.note_alarm(1_000 + ALARM_REPEAT_MS, "the hub served a fork"), "again an hour on");
        // M2 re-review N3: alarms whose text varies are still few an hour.
        let later = 10 * ALARM_REPEAT_MS;
        let raised = (0..20)
            .filter(|i| st.note_alarm(later, &format!("device d-{i}'s view conflicts")))
            .count();
        assert_eq!(raised, MAX_ALARMS_PER_WINDOW);
        for i in 0..MAX_ALARMS {
            st.note_alarm(later + (i as u64 + 1) * ALARM_REPEAT_MS, &format!("alarm {i}"));
        }
        assert_eq!(st.alarms.len(), MAX_ALARMS);
        assert_eq!(st.alarms.last().unwrap().what, format!("alarm {}", MAX_ALARMS - 1));
        st.save(&paths).unwrap();
        assert_eq!(AgentState::load(&paths).unwrap().unwrap().alarms, st.alarms, "they survive a restart");
    }

    /// Review D3: which devices ever opened a channel survives a restart, and
    /// a fresh enrollment's record is whole (it starts at seq 0).
    #[test]
    fn the_devices_that_opened_a_channel_are_kept_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentcontrollers");
        let (entries, fp, _) = chain();
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 500);
        assert_eq!(st.controllers(), None, "nothing to say before a chain");
        st.absorb(&entries, 0);
        let none = st.controllers().unwrap();
        assert_eq!(
            (none.from, none.from_ms, none.seq, none.devices.len()),
            (0, 500, 1, 0),
            "whole from enrollment on, nobody yet"
        );
        assert!(st.note_controller("d-1", 7_000).is_some(), "news");
        assert!(st.note_controller("d-1", 8_000).is_none(), "seen before");
        assert!(st.note_controller("d-0", 9_000).is_some());
        st.save(&paths).unwrap();
        let loaded = AgentState::load(&paths).unwrap().unwrap();
        let record = loaded.controllers().unwrap();
        assert_eq!((record.from, record.from_ms, record.seq), (0, 500, 1));
        let seen = |id: &str, first_ms| Controller { id: id.into(), first_ms };
        assert_eq!(
            record.devices,
            vec![seen("d-0", 9_000), seen("d-1", 7_000)],
            "by id, each first seen by this agent's clock"
        );
    }

    /// A state file from before the record was kept: whoever opened a channel
    /// before is unknown, so the record starts at the chain seq it was loaded
    /// at — and stays there once saved.
    #[test]
    fn a_state_from_before_the_record_starts_it_at_the_seq_it_was_loaded_at() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentcontrollersold");
        let (entries, fp, _) = chain();
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 0);
        st.absorb(&entries, 0);
        st.save(&paths).unwrap();
        let path = state_path(&paths);
        let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let map = v.as_object_mut().unwrap();
        for key in ["controllers", "controllers_from", "controllers_from_ms"] {
            assert!(map.remove(key).is_some(), "{key}");
        }
        super::super::write_private(&path, &serde_json::to_vec(&v).unwrap()).unwrap();
        let before = crate::agent::now_ms();
        let mut old = AgentState::load(&paths).unwrap().unwrap();
        let record = old.controllers().unwrap();
        assert_eq!((record.from, record.seq, record.devices.len()), (1, 1, 0), "complete from the upgrade on");
        assert!(record.from_ms >= before, "from the load, by this agent's clock");
        assert!(old.note_controller("d-1", 5).is_some());
        old.save(&paths).unwrap();
        let again = AgentState::load(&paths).unwrap().unwrap().controllers().unwrap();
        assert_eq!((again.from, again.from_ms), (1, record.from_ms), "kept where it started");
        assert_eq!(again.devices.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["d-1"]);
    }

    /// At [`MAX_CONTROLLERS`] the device seen first makes room, and the record
    /// is then complete only from the chain's seq of then.
    #[test]
    fn a_full_record_evicts_the_first_seen_and_starts_again_at_the_seq_of_then() {
        let (entries, fp, _) = chain();
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 0);
        st.absorb(&entries, 0);
        for i in 0..MAX_CONTROLLERS {
            assert!(st.note_controller(&format!("d-{i:03}"), 1_000 + i as u64).is_some());
        }
        assert_eq!(st.controllers().unwrap().from, 0, "full, not over");
        let before = st.controllers().unwrap();
        // Not saved: taken back, eviction and all.
        let noted = st.note_controller("d-new", 9_999).unwrap();
        st.unnote_controller(noted);
        assert_eq!(st.controllers().unwrap(), before, "as it was");
        assert!(st.note_controller("d-new", 9_999).is_some());
        let record = st.controllers().unwrap();
        let ids: Vec<&str> = record.devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids.len(), MAX_CONTROLLERS);
        assert!(!ids.contains(&"d-000") && ids.contains(&"d-new"), "the first seen went");
        assert_eq!((record.from, record.from_ms), (1, 9_999), "complete only from the seq of then");
    }

    /// A device new to the record whose save fails is taken back, eviction
    /// and all; one on the saved record stays, the save notwithstanding.
    #[test]
    fn recording_a_device_saves_it_or_takes_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentrecord");
        let (entries, fp, _) = chain();
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", fp, 0);
        st.absorb(&entries, 0);
        assert_eq!(st.record_controller(&paths, "d-1", 1_000), Recorded::New);
        assert_eq!(st.record_controller(&paths, "d-1", 2_000), Recorded::Known);
        let before = st.controllers().unwrap();
        // Saves fail: the file's place is taken.
        let path = state_path(&paths);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir_all(path.join("in-the-way")).unwrap();
        assert!(matches!(st.record_controller(&paths, "d-2", 3_000), Recorded::Unsaved(_)));
        assert_eq!(st.controllers().unwrap(), before, "taken back");
        assert!(matches!(st.record_controller(&paths, "d-1", 4_000), Recorded::KnownUnsaved(_)));
        std::fs::remove_dir_all(&path).unwrap();
        assert_eq!(st.record_controller(&paths, "d-2", 5_000), Recorded::New, "saved this time");
        let ids: Vec<String> = AgentState::load(&paths).unwrap().unwrap().controllers().unwrap().devices.into_iter().map(|d| d.id).collect();
        assert_eq!(ids, vec!["d-1".to_string(), "d-2".to_string()]);
    }

    #[test]
    fn machine_keys_round_trip_by_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentkeys");
        assert!(load_machine_keys(&paths).unwrap().is_empty());
        store_machine_key(&paths, 0, &[1; 32]).unwrap();
        store_machine_key(&paths, 2, &[2; 32]).unwrap();
        let keys = load_machine_keys(&paths).unwrap();
        assert_eq!(keys.keys().copied().collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(*keys[&2], [2; 32]);
    }

    /// M2 re-review: `agent enroll` after a re-enrollment kept the old
    /// `machine-keys.json`, and the writer sealed under its newest epoch.
    #[test]
    fn a_new_enrollment_forgets_the_last_ones_keys_and_streams() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "agentreenroll");
        store_machine_key(&paths, 7, &[7; 32]).unwrap();
        let store = super::super::streams::store::path(&paths);
        std::fs::write(&store, b"old").unwrap();
        std::fs::write(format!("{}-wal", store.display()), b"old").unwrap();
        forget_enrollment_leftovers(&paths).unwrap();
        assert!(load_machine_keys(&paths).unwrap().is_empty());
        assert!(!store.exists());
        assert!(!std::path::Path::new(&format!("{}-wal", store.display())).exists());
        forget_enrollment_leftovers(&paths).unwrap();
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(super::unhex(&super::hex(&[1, 2, 0xfe])).unwrap(), vec![1, 2, 0xfe]);
        assert!(super::unhex("0").is_none());
    }
}
