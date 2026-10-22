//! Renewing this device's identity key on request
//! (`docs/host-identity-rotation.md` §3.2–§3.7, §5.6).
//!
//! - **The request** — `POST /rh/rotate {nonce, eph, chainSeq, chainHead,
//!   assurance}` over an end-to-end channel ([`rotate`]): a Posture route
//!   (an admin, with a view of the account vouched for within twelve hours —
//!   checked by the bridge and again here), for a device that is a current
//!   host of its own chain with the keys it runs with, about a chain view
//!   that is on that chain (409 `not_on_chain` otherwise: the page syncs and
//!   asks again). A new seed is staged beside the one in use
//!   (`machine-key.next-<nonce>.json`, [`super::SeedStore::stage`]); the seed
//!   in use is never touched. The answer: both keys' signatures over the
//!   proof ([`rh_proto::chain::host_rotate_body`]) and the DH confirmation
//!   MAC. The same nonce gets the same answer. A new nonce adds a staging:
//!   every staging the hub took is kept until it expires (at most
//!   [`MAX_STAGINGS`]) — a page whose answer was lost asks again, and the hub
//!   may still commit the first. One it refused is forgotten at once; one it
//!   did not answer in time is kept, unshown, for the attempt's ten minutes
//!   ([`ATTEMPT_MS`]), and becomes one it took if its answer comes late.
//! - **The staging** — the proof goes to the hub as `rotation_stage` on this
//!   agent's own hub connection, signed in with the old key, and only on a
//!   connection whose `welcome` offered `rotate/1` (an older hub closes on a
//!   message it does not know: 409 `hub_not_ready` then, and nothing is
//!   staged). The hub's `rotation_staged` / `rotation_refused` is waited for
//!   [`STAGE_WAIT`] at most.
//! - **The window** — every sign-in with the old key while the hub holds the
//!   staging is asked `rotation_pending`: this agent answers with the new
//!   key's `rotation_ack` over that connection's own challenge nonce, or
//!   `rotation_unknown` when it does not hold the key asked about
//!   ([`on_pending`]). Whoever else holds the old key cannot answer.
//! - **Promotion** — once its chain renews this device to exactly one staged
//!   seed's keys (the entry, or a hub closing with 4005, which makes it sign
//!   in with the staged keys and read the chain), it signs in with that key,
//!   sees the entry on the chain it verified, renames that staged seed into
//!   place (the others go), and restarts its link — relays, control end and
//!   the streams' writer with it — in this process ([`promote`]), which then
//!   renews the push keys too (a new VAPID keypair, every subscription
//!   forgotten: R6 review). AI sessions and terminals do not notice. A lying hub cannot make it give
//!   up its seed: only its own verified chain promotes.
//! - **Crash safety** — at start, staged seeds sign in first (the one the
//!   chain names, then the newest); a hub that takes none of them — its
//!   `denied`, nothing else: `unavailable` is its own trouble, retried —
//!   gets the old one, never once a renewal is committed. Every step leaves
//!   one of: the old seed (alone or with staged ones); the new seed (at worst
//!   with staged ones never used). Every rename is flushed (`sync_dir`).
//! - **Expiry** — a staging no renewal should take any more (its day over
//!   before the old key signed in again, or more than [`MAX_STAGINGS`]) is
//!   set aside (`machine-key.expired-<nonce>.json`), never deleted: never
//!   tried at sign-in, and taken back and promoted if the verified chain
//!   names it after all ([`set_aside_expired`], R6 review).
//! - **Replaced elsewhere** — a verified entry on its own chain renewing
//!   this device's id to keys it did not stage (waiting or set aside): this
//!   computer came back from a backup made before a renewal, or another copy
//!   of its old key renewed it. A `foreign_rotation` alarm (logged, in the
//!   status stream every device reads, pushed with Bark), a dispute
//!   statement the retired key signs over that renewal, kept and delivered
//!   to the hub until it takes it ([`super::dispute`]), and the link stops;
//!   the old seed stays ([`Verdict::Foreign`])
//!   until a person adds this device again under a new identity
//!   ([`re_enroll_fresh`]); every sign-up after it uses a fresh seed
//!   ([`identity_for_enrolling`]). Its own renewal (to a key it staged) is
//!   never taken for it, nor one a staged file that does not read may hold
//!   ([`Verdict::Unreadable`]). A hub refusing the key as `rotated` is not
//!   evidence of it: the agent reads the chain the hub then serves, and
//!   without a verified renewal it only waits and retries (`refused`).
//! - **High assurance** — a renewal with `assurance: 1` shows its code
//!   ([`rh_proto::chain::rotate_sas`]) on this device for [`CODE_FOR_MS`]:
//!   the local settings (`GET /api/cloud` `link.rotation`) and
//!   `droppen agent status` ([`status`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::RngCore;
use rh_proto::chain::{self, Assurance};
use rh_proto::identity::MachineIdentity;
use rh_proto::sig;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::hub::{hex, now_ms, unhex32};
use super::link::Shared;
use super::state::AgentState;
use super::{FileSeedStore, SeedStore, StagedSeed};
use crate::sessions::Paths;

/// How long `POST /rh/rotate` waits for the hub to stage the proof.
#[cfg(not(test))]
pub const STAGE_WAIT: Duration = Duration::from_secs(10);
/// [`STAGE_WAIT`] in tests.
#[cfg(test)]
pub const STAGE_WAIT: Duration = Duration::from_secs(1);
/// How long a high-assurance renewal's code is shown (§3.7).
pub const CODE_FOR_MS: u64 = 10 * 60 * 1000;
/// How long `link.rotation` says a renewal just happened.
pub const RENEWED_SHOWN_MS: u64 = 24 * 60 * 60 * 1000;
/// How long a staging the hub took may still be committed: the hub expires
/// it a day after it staged it ([`drop_expired`]).
pub const STAGED_FOR_MS: u64 = 24 * 60 * 60 * 1000;
/// At most this many stagings the hub took are kept at once; one more drops
/// the oldest. A page whose answer was lost asks again with a new nonce, and
/// the hub may still commit the earlier staging: each one it took stays
/// until it expires, so that renewal is this device's, never "elsewhere".
pub const MAX_STAGINGS: usize = 4;
/// How long an attempt the page registered on the hub lives:
/// a staging the hub did not answer within [`STAGE_WAIT`] is kept — unshown —
/// this long from when it was made, in case the hub staged it after all.
pub const ATTEMPT_MS: u64 = 10 * 60 * 1000;
/// How long a connection signed in with the staged key waits for the chain
/// to name it before it signs in with the old key again.
#[cfg(not(test))]
pub const NEW_KEY_GRACE: Duration = Duration::from_secs(20);
/// [`NEW_KEY_GRACE`] in tests.
#[cfg(test)]
pub const NEW_KEY_GRACE: Duration = Duration::from_secs(2);

/// What restarts the link in this process — the server's [`super::control::Control`]
/// — once a renewed key is in place.
pub type Restart = Arc<dyn Fn() + Send + Sync>;

/// A [`Restart`] that does nothing (a link no control runs: tests).
pub fn no_restart() -> Restart {
    Arc::new(|| {})
}

/// The hub connection as renewals need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubSession {
    /// Whether this connection's `welcome` offered `rotate/1`: only then are
    /// the rotation messages sent on it.
    pub offered: bool,
    /// The hub's challenge nonce this connection answered: what a
    /// `rotation_ack` signs over.
    pub nonce: [u8; 32],
    /// Whether it signed in with the staged key (it then only reads the
    /// chain, to promote).
    pub staged_key: bool,
    /// When it signed in (this agent's clock).
    pub since_ms: u64,
}

#[derive(Default)]
struct Inner {
    session: Option<HubSession>,
    /// `rotation_stage`s waiting for the hub's answer, by nonce hash (hex).
    waiting: HashMap<String, oneshot::Sender<Value>>,
    /// The stagings the hub took this run, by nonce, with the `ready_ms` it
    /// said: what the same nonce is answered with again.
    taken: HashMap<[u8; 16], Option<u64>>,
    /// The connection signed in with the old key is to end, so the next
    /// signs in with the staged one (the chain names it now).
    switch: bool,
    /// The next connection is made at once, not after the back-off.
    again: bool,
    /// The renewal to the staged key is committed (the chain names it, or the
    /// hub closed with 4005): the key in use is retired, and never signs in
    /// again — a sign-in with it is the hub's evidence against this very
    /// device (`docs/host-identity-rotation.md` §3.8).
    committed: bool,
    /// When (wall clock, ms) the hub connection last heard from the hub.
    heard_ms: u64,
}

/// How recently (wall clock: a computer's sleep counts) the hub connection
/// must have heard from the hub for it to vouch that the key in use is
/// still current ([`Rotation::key_in_use_confirmed`]). The link pings every
/// 20 s, so a live hub is heard at least that often.
pub const LIVE_MS: u64 = 25_000;

/// Ends a hub connection's [`HubSession`] when dropped, however the
/// connection ends.
pub struct SessionGuard<'a>(pub &'a Rotation);

impl Drop for SessionGuard<'_> {
    fn drop(&mut self) {
        self.0.disconnected();
    }
}

/// What a running link keeps about renewals.
pub struct Rotation {
    inner: Mutex<Inner>,
    /// One `POST /rh/rotate` at a time.
    gate: tokio::sync::Mutex<()>,
    /// Wakes the hub connection when [`Rotation::switch`] is set.
    pub(super) wake: tokio::sync::Notify,
}

impl Default for Rotation {
    fn default() -> Self {
        Self::new()
    }
}

impl Rotation {
    /// Nothing known yet.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            gate: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("rotation")
    }

    /// A hub connection signed in.
    pub fn connected(&self, session: HubSession) {
        let mut inner = self.lock();
        inner.session = Some(session);
        inner.switch = false;
        inner.heard_ms = now_ms();
    }

    /// The hub connection heard from the hub just now.
    pub fn heard(&self) {
        self.lock().heard_ms = now_ms();
    }

    /// It went: whatever waited for it gets no answer.
    pub fn disconnected(&self) {
        let mut inner = self.lock();
        inner.session = None;
        inner.waiting.clear();
    }

    /// The hub connection, while it is up.
    pub fn session(&self) -> Option<HubSession> {
        self.lock().session
    }

    /// Whether the link is signed in to the hub with the key in use and has
    /// heard from the hub within [`LIVE_MS`] by the wall clock at `now` — the
    /// hub took the key and still talks on that connection, so no renewal has
    /// retired it: only then may this computer's device role sign in while a
    /// staged seed is kept ([`device_role_may_sign_in`]). A connection that
    /// merely looks open — a laptop waking from sleep, its socket dead and
    /// not yet noticed — vouches for nothing.
    pub fn key_in_use_confirmed(&self, now: u64) -> bool {
        let inner = self.lock();
        // A renewal committed (the chain names a staged key, or the hub closed
        // with 4005) retired the key in use, whatever that connection says.
        !inner.committed && inner.session.is_some_and(|s| !s.staged_key) && now.saturating_sub(inner.heard_ms) <= LIVE_MS
    }

    /// Whether the connection signed in with the old key should end now (and
    /// the next sign in with the staged key, at once); taken once.
    pub fn take_switch(&self) -> bool {
        let mut inner = self.lock();
        let switch = std::mem::take(&mut inner.switch);
        if switch {
            inner.again = true;
        }
        switch
    }

    /// The hub closed with 4005 (it committed a renewal of this device): with
    /// a staged seed, sign in with it next, at once.
    pub fn renewed_by_hub(&self) {
        let mut inner = self.lock();
        inner.again = true;
        inner.committed = true;
    }

    /// Whether the renewal to the staged key is committed: only the staged
    /// key signs in from now on, however often the hub turns it away.
    pub fn committed(&self) -> bool {
        self.lock().committed
    }

    /// Whether the next connection is made at once; taken once.
    pub fn take_again(&self) -> bool {
        std::mem::take(&mut self.lock().again)
    }

    /// The hub's `rotation_staged` / `rotation_refused`, to whoever waits for
    /// it; `false` when nobody does.
    pub fn answer(&self, value: &Value) -> bool {
        let Some(hash) = value["nonce_hash"].as_str() else { return false };
        match self.lock().waiting.remove(hash) {
            Some(tx) => tx.send(value.clone()).is_ok(),
            None => false,
        }
    }
}

/// `SHA-256(nonce)`, hex: how the hub names a request.
pub fn nonce_hash(nonce: &[u8; 16]) -> [u8; 32] {
    Sha256::digest(nonce).into()
}

/// The proof both keys sign (§3.2) for this device renewing from `old` to
/// `new` as `staged` says.
fn proof(account: &str, machine: &str, staged: &StagedSeed, old: &MachineIdentity, new: &MachineIdentity) -> Vec<u8> {
    chain::host_rotate_body(
        account,
        machine,
        staged.chain_seq,
        &staged.chain_head,
        &old.sign_public(),
        &old.dh_public(),
        &new.sign_public(),
        &new.dh_public(),
        &staged.nonce,
        staged.assurance,
    )
}

/// What the chain says now about this device's own keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The keys it runs with (or the chain does not name it: an enrollment,
    /// a removal — handled elsewhere).
    Unchanged,
    /// Renewed to exactly its staged seed: promote it.
    Renewed,
    /// Renewed to keys that are neither the ones it runs with nor its staged
    /// seed's: somebody else renewed it ("renewed elsewhere").
    Foreign {
        /// The latest renewal of this device on the chain.
        seq: u64,
        /// The keys the chain names now, as the device list shows them.
        fingerprint: String,
    },
    /// Renewed to keys that are neither the ones it runs with nor any staged
    /// seed's that reads — while a staged seed's file does not read: the
    /// renewal may be to that one. Never taken for a replacement elsewhere
    /// (R6 review): alarmed, and the link stops until the file is put right.
    Unreadable,
}

/// [`Verdict`] of `st`'s chain for a device running with `current`, with
/// `staged` staged (and, when `unreadable`, a staged seed whose file does not
/// read). Only a renewal makes a device's keys differ from what it holds (an
/// enrollment with other keys is refused where it completes,
/// [`super::link`]), so keys after one that are neither `current` nor any
/// staged seed's are somebody else's — unless a staged seed it cannot read
/// may hold them.
pub fn verdict(st: &AgentState, current: &MachineIdentity, staged: &[MachineIdentity], unreadable: bool) -> Verdict {
    let Some(record) = st.chain.as_ref().and_then(|c| c.device_record(&st.machine)) else {
        return Verdict::Unchanged;
    };
    let named = (record.info.sign_key, record.info.dh_key);
    if named == (current.sign_public(), current.dh_public()) {
        return Verdict::Unchanged;
    }
    if staged.iter().any(|s| named == (s.sign_public(), s.dh_public())) {
        return Verdict::Renewed;
    }
    match record.rotated_seq {
        Some(_) if unreadable => Verdict::Unreadable,
        Some(seq) => Verdict::Foreign {
            seq,
            fingerprint: rh_proto::identity::device_fingerprint_text(&named.0, &named.1),
        },
        None => Verdict::Unchanged,
    }
}

/// Every staging of this instance this device keeps — waiting, and set aside
/// as expired — but the key in use; read only, nothing forgotten (another
/// process may call it).
fn kept_stagings(store: &FileSeedStore, current: &MachineIdentity) -> Vec<StagedSeed> {
    let uuid = store.load().ok().flatten().map(|s| s.uuid);
    store
        .stagings()
        .unwrap_or_default()
        .into_iter()
        .chain(store.expired())
        .filter(|s| uuid.as_deref() == Some(s.uuid.as_str()) && s.identity().sign_public() != current.sign_public())
        .collect()
}

/// [`verdict`] from this instance's files, changing none: every staging it
/// keeps, waiting or set aside as expired (R6 review).
fn stored_verdict(paths: &Paths, st: &AgentState, current: &MachineIdentity) -> Verdict {
    let store = FileSeedStore::for_instance(paths);
    let kept = kept_stagings(&store, current);
    verdict(st, current, &identities(&kept), !store.unreadable().is_empty())
}

/// The keys `st`'s chain names for this device now, if it names it.
pub(super) fn named_keys(st: &AgentState) -> Option<([u8; 32], [u8; 32])> {
    let record = st.chain.as_ref()?.device_record(&st.machine)?;
    Some((record.info.sign_key, record.info.dh_key))
}

/// A staging set aside as expired whose keys are the ones the chain names
/// (`keys`) after all — a renewal committed later than this device judged
/// possible: taken back among the stagings, to be signed in with and
/// promoted (R6 review).
fn restore_named(store: &FileSeedStore, keys: ([u8; 32], [u8; 32]), current: &MachineIdentity) {
    let uuid = store.load().ok().flatten().map(|s| s.uuid);
    let staged = store.expired().into_iter().find(|s| {
        let k = s.identity();
        uuid.as_deref() == Some(s.uuid.as_str()) && k.sign_public() != current.sign_public() && (k.sign_public(), k.dh_public()) == keys
    });
    if let Some(staged) = staged {
        tracing::warn!("the device chain names a staged identity key this device had set aside as expired: taking it back");
        if let Err(error) = store.restore(&staged.nonce) {
            tracing::error!(%error, "cannot take back a staged identity key the device chain names");
        }
    }
}

/// Of `stagings`, the one `st`'s chain names for this device, if any.
pub fn named<'a>(st: &AgentState, stagings: &'a [StagedSeed]) -> Option<&'a StagedSeed> {
    let record = st.chain.as_ref()?.device_record(&st.machine)?;
    stagings.iter().find(|s| {
        let k = s.identity();
        (record.info.sign_key, record.info.dh_key) == (k.sign_public(), k.dh_public())
    })
}

/// Why this device holds only a retired identity key (`link.rotation.reason`
/// of state `foreign`): a renewal on its own verified chain gave its id keys
/// it does not hold. Nothing else — a hub's word least of all — says so.
pub const FOREIGN_RENEWED_ELSEWHERE: &str = "renewed_elsewhere";
/// `link.rotation.reason` of state `refused`: the hub refuses the key in
/// use as `rotated`, and no chain it served shows a renewal (yet).
pub const REFUSED_ROTATED: &str = "rotated";
/// `link.rotation.reason` of state `unreadable` (R6 review).
pub const UNREADABLE_REASON: &str = "staged_key_unreadable";

/// What this device says about holding only a retired key — in the alarm,
/// the logs and `agent status` — worded for the likeliest case too: this
/// computer's own backup from before a renewal.
pub const REPLACED_ELSEWHERE: &str = "This device's identity key was replaced elsewhere (for example, it was restored from a \
     backup made before a renewal, or another copy of its old key renewed it). To keep using it here, add it again under a \
     new identity.";

/// The staged seeds this device may use, oldest first: this instance's, and
/// not the key in use (any other is forgotten here). Whether one expired is
/// judged only where that is safe ([`drop_expired`], [`rotate`]): a renewal
/// committed while this device was away may name any staging the hub took.
pub(super) fn staged_seeds(paths: &Paths, current: &MachineIdentity) -> Vec<StagedSeed> {
    let store = FileSeedStore::for_instance(paths);
    let uuid = store.load().ok().flatten().map(|s| s.uuid);
    let mut out = Vec::new();
    for staged in store.stagings().unwrap_or_default() {
        let why = if uuid.as_deref() != Some(staged.uuid.as_str()) {
            Some("it belongs to another instance")
        } else if staged.identity().sign_public() == current.sign_public() {
            Some("it is the key in use")
        } else {
            None
        };
        match why {
            Some(why) => {
                tracing::warn!(why, "forgetting a staged identity key");
                if let Err(error) = store.discard_staged(&staged.nonce) {
                    tracing::error!(%error, "cannot forget a staged identity key");
                }
            }
            None => out.push(staged),
        }
    }
    out
}

/// The keys of `stagings`.
pub(super) fn identities(stagings: &[StagedSeed]) -> Vec<MachineIdentity> {
    stagings.iter().map(StagedSeed::identity).collect()
}

/// When no renewal can name `staged` any more: 24 h after the hub took it
/// (the hub's staging expires) — or, for one the hub never confirmed, once
/// the attempt it was made for is over ([`ATTEMPT_MS`]).
fn expiry(staged: &StagedSeed) -> u64 {
    match staged.accepted_ms {
        Some(at) => at.saturating_add(STAGED_FOR_MS),
        None => staged.created_ms.saturating_add(ATTEMPT_MS),
    }
}

/// The key in use signed in at `since`: the hub had not retired it then — a
/// commit retires it in the same step — so no staging that expired before
/// `since` was ever committed, nor can be by an honest hub. But that rests
/// on the hub's word (it took the sign-in), so those are set aside, never
/// deleted (R6 review): never tried at sign-in again, and taken back if
/// this device's own verified chain names one after all ([`on_chain`]). One
/// whose keys that chain names now (`named`, [`named_keys`]) is never set aside.
pub(super) fn set_aside_expired(paths: &Paths, named: Option<([u8; 32], [u8; 32])>, current: &MachineIdentity, since: u64) {
    let store = FileSeedStore::for_instance(paths);
    for staged in &staged_seeds(paths, current) {
        let keys = staged.identity();
        if expiry(staged) < since && named != Some((keys.sign_public(), keys.dh_public())) {
            tracing::info!("setting aside a staged identity key no renewal should take any more");
            if let Err(error) = store.expire(&staged.nonce) {
                tracing::error!(%error, "cannot set aside a staged identity key");
            }
        }
    }
}

/// `POST /rh/rotate` (see the module docs). 200 `{signKey, dhKey, oldSig,
/// newSig, mac, stagedSeq, stagedHead, readyMs}`; 400 `bad_request`; 403
/// `forbidden`/`limited_device`; 412 `stale`; 409 `not_current_host`,
/// `not_v3`, `not_on_chain`, `assurance_required`, `hub_not_ready`,
/// `hub_refused {reason}`; 504
/// `hub_timeout` (the hub said nothing in [`STAGE_WAIT`]: the new seed is
/// kept, unshown, for the attempt's ten minutes in case the hub staged it
/// after all — [`late_answer`]).
pub(super) async fn rotate(shared: &Arc<Shared>, device: &str, body: &[u8]) -> (u16, Value) {
    let req = match Request::parse(body) {
        Ok(req) => req,
        Err(message) => return (400, json!({"error": "bad_request", "message": message})),
    };
    let _one = shared.rotation.gate.lock().await;
    // The bridge refused already; the chain (and who vouched for it) may
    // have moved since, and this route changes who this device is.
    if let Some(refused) = super::bridge::refusal("POST", "/rh/rotate", super::link::standing_of(shared, device)) {
        return refused.answer();
    }
    let (account, machine) = match check_chain(shared, &req) {
        Ok(found) => found,
        Err(answer) => return answer,
    };
    let session = shared.rotation.session().filter(|s| s.offered && !s.staged_key);
    let text = shared.hub_text.lock().expect("hub text").clone();
    let (Some(session), Some(text)) = (session, text) else {
        return (
            409,
            json!({"error": "hub_not_ready", "message": "this device's hub connection is down, or its hub does not stage identity renewals yet"}),
        );
    };
    let store = FileSeedStore::for_instance(&shared.paths);
    let uuid = match store.load() {
        Ok(Some(seed)) => seed.uuid,
        _ => return (500, json!({"error": "unavailable", "message": "this device's identity key does not read"})),
    };
    let stagings = staged_seeds(&shared.paths, &shared.identity);
    let existing = stagings.iter().find(|s| s.nonce == req.nonce).cloned();
    if let Some(s) = &existing {
        if (s.chain_seq, s.chain_head, s.assurance) != (req.chain_seq, req.chain_head, req.assurance) {
            return (
                400,
                json!({"error": "bad_request", "message": "this nonce came before with another chain view or assurance"}),
            );
        }
    }
    let is_new = existing.is_none();
    let staged = match existing {
        Some(staged) => staged,
        None => {
            let mut seed = Zeroizing::new([0u8; 32]);
            rand::rngs::OsRng.fill_bytes(seed.as_mut());
            StagedSeed {
                uuid,
                seed,
                nonce: req.nonce,
                chain_seq: req.chain_seq,
                chain_head: req.chain_head,
                assurance: req.assurance,
                created_ms: now_ms(),
                accepted_ms: None,
            }
        }
    };
    let new = staged.identity();
    let body = proof(&account, &machine, &staged, &shared.identity, &new);
    let Some(mac) = chain::host_rotate_confirm(new.dh(), &req.eph, &body) else {
        return (400, json!({"error": "bad_request", "message": "eph is not a usable X25519 key"}));
    };
    let old_sig = sig::sign(shared.identity.signing_key(), sig::HOST_ROTATE, &body);
    let new_sig = sig::sign(new.signing_key(), sig::HOST_ROTATE, &body);
    let answer = |ready_ms: Option<u64>| {
        json!({
            "signKey": hex(&new.sign_public()),
            "dhKey": hex(&new.dh_public()),
            "oldSig": hex(&old_sig),
            "newSig": hex(&new_sig),
            "mac": hex(&mac),
            "stagedSeq": staged.chain_seq,
            "stagedHead": hex(&staged.chain_head),
            "readyMs": ready_ms,
        })
    };
    // The same nonce, staged on the hub already: the same answer.
    let taken = shared.rotation.lock().taken.get(&req.nonce).copied();
    if let Some(ready_ms) = taken {
        return (200, answer(ready_ms));
    }
    if is_new {
        make_room(&store, &stagings, session.since_ms, now_ms());
    }
    // On disk before the hub hears of it: a renewal the hub stages is one
    // this device can always finish.
    if let Err(error) = store.stage(&staged) {
        tracing::error!(%error, "cannot stage a new identity key");
        return (500, json!({"error": "unavailable", "message": "cannot store the new identity key"}));
    }
    let hash = hex(&nonce_hash(&req.nonce));
    let (tx, rx) = oneshot::channel();
    shared.rotation.lock().waiting.insert(hash.clone(), tx);
    let stage = json!({
        "t": "rotation_stage",
        "nonce": hex(&req.nonce),
        "staged_seq": staged.chain_seq,
        "staged_head": hex(&staged.chain_head),
        "sign_key": hex(&new.sign_public()),
        "dh_key": hex(&new.dh_public()),
        "assurance": staged.assurance.to_byte(),
        "old_sig": hex(&old_sig),
        "new_sig": hex(&new_sig),
    });
    if text.send(stage.to_string()).is_err() {
        shared.rotation.lock().waiting.remove(&hash);
        return (409, json!({"error": "hub_not_ready", "message": "this device's hub connection went"}));
    }
    tracing::info!(%device, assurance = staged.assurance.to_byte(), "staged a new identity key; the hub is staging the renewal");
    let heard = tokio::time::timeout(STAGE_WAIT, rx).await;
    shared.rotation.lock().waiting.remove(&hash);
    match heard {
        Ok(Ok(v)) if v["t"] == "rotation_staged" => {
            let ready_ms = v["ready_ms"].as_u64();
            // Taken by the hub: kept until it expires, whatever is asked next.
            let accepted = StagedSeed {
                accepted_ms: Some(staged.accepted_ms.unwrap_or_else(now_ms)),
                ..staged.clone()
            };
            if let Err(error) = store.stage(&accepted) {
                tracing::error!(%error, "cannot note that the hub staged the renewal");
            }
            shared.rotation.lock().taken.insert(req.nonce, ready_ms);
            if staged.assurance == Assurance::OnDevice {
                log_code(&chain::rotate_sas(&body));
            }
            (200, answer(ready_ms))
        }
        Ok(Ok(v)) => {
            let reason = v["reason"].as_str().unwrap_or("unknown").to_string();
            tracing::warn!(%reason, "the hub refused to stage the renewal; forgetting the staged key");
            // Only one the hub never took: a staging it took once may still
            // be committed (the same nonce asked again, and refused now).
            if staged.accepted_ms.is_none() {
                if let Err(error) = store.discard_staged(&req.nonce) {
                    tracing::error!(%error, "cannot forget the staged identity key");
                }
            }
            (
                409,
                json!({"error": "hub_refused", "reason": reason, "message": "the hub refused to stage this renewal"}),
            )
        }
        Ok(Err(_)) | Err(_) => {
            // Not answered in time — but the hub may still have staged it
            // (its answer late, the page's withdrawal lost): kept, unshown,
            // for the attempt's ten minutes, so a later sign-in asked to
            // confirm it still can; a late `rotation_staged` makes it one the
            // hub took ([`late_answer`]).
            if staged.accepted_ms.is_none() {
                tracing::warn!("the hub did not answer the staging in time; keeping the staged key for the attempt's ten minutes");
            }
            (
                504,
                json!({"error": "hub_timeout", "message": "the hub did not answer the staging in time: ask again"}),
            )
        }
    }
}

/// A high-assurance renewal waits for its code to be typed on the admin
/// device: said at WARN without the code — the code is in the local settings
/// and `droppen agent status` ([`status`]) — and the code itself at DEBUG at
/// most (R6 review: logs are kept, and read by more than the person here).
fn log_code(code: &str) {
    tracing::warn!(
        "an admin device renews this device's identity key and asks you to confirm it: its code is in this computer's Droppen settings (and `droppen agent status`) for 10 minutes"
    );
    tracing::debug!(%code, "the renewal's confirmation code");
}

/// A `rotation_staged` / `rotation_refused` nobody waits for any more: the
/// answer to a staging whose wait timed out. Taken: the staging is kept as
/// one the hub took (it may be committed); refused: forgotten. An answer
/// for a staging no longer kept is ignored.
pub(super) fn late_answer(shared: &Shared, value: &Value) {
    let hash = value["nonce_hash"].as_str().and_then(unhex32);
    let staged = hash.and_then(|hash| {
        staged_seeds(&shared.paths, &shared.identity)
            .into_iter()
            .find(|s| nonce_hash(&s.nonce) == hash && s.accepted_ms.is_none())
    });
    let Some(staged) = staged else {
        tracing::info!(t = %value["t"], nonce_hash = %value["nonce_hash"], "a staging answer for no staged key waiting for one: ignored");
        return;
    };
    let store = FileSeedStore::for_instance(&shared.paths);
    match value["t"].as_str() {
        Some("rotation_staged") => {
            tracing::info!("the hub's late answer: it staged the renewal; keeping the staged key");
            let nonce = staged.nonce;
            let accepted = StagedSeed {
                accepted_ms: Some(now_ms()),
                ..staged
            };
            if let Err(error) = store.stage(&accepted) {
                tracing::error!(%error, "cannot note that the hub staged the renewal");
            }
            shared.rotation.lock().taken.insert(nonce, value["ready_ms"].as_u64());
        }
        _ => {
            tracing::info!(reason = %value["reason"], "the hub's late answer: it refused the staging; forgetting the staged key");
            if let Err(error) = store.discard_staged(&staged.nonce) {
                tracing::error!(%error, "cannot forget the staged identity key");
            }
        }
    }
}

/// Room for a new staging among `stagings`: one the hub never answered goes
/// once its attempt's ten minutes are over by `now`; one it took stays until
/// it expired before `since` — when the key in use signed in, so nothing
/// committed it — and beyond [`MAX_STAGINGS`] the oldest the hub took goes.
/// "Goes" is set aside, never deleted (R6 review, [`set_aside_expired`]):
/// only asked while the chain names the key in use, so it names none of them.
fn make_room(store: &FileSeedStore, stagings: &[StagedSeed], since: u64, now: u64) {
    let mut kept: Vec<&StagedSeed> = Vec::new();
    for staged in stagings {
        let expired = match staged.accepted_ms {
            None => expiry(staged) < now,
            Some(_) => expiry(staged) < since,
        };
        if staged.accepted_ms.is_none() && !expired {
            continue;
        }
        if expired {
            if let Err(error) = store.expire(&staged.nonce) {
                tracing::error!(%error, "cannot set aside a staged identity key");
            }
        } else {
            kept.push(staged);
        }
    }
    kept.sort_by_key(|s| s.accepted_ms);
    while kept.len() >= MAX_STAGINGS {
        let oldest = kept.remove(0);
        tracing::warn!("more renewals staged on the hub than kept: setting aside the oldest staged identity key");
        if let Err(error) = store.expire(&oldest.nonce) {
            tracing::error!(%error, "cannot set aside a staged identity key");
        }
    }
}

/// The account and this machine's id, if the chain lets it renew now:
/// a current host whose chain names the keys it runs with, asked about a
/// view on that chain.
fn check_chain(shared: &Shared, req: &Request) -> Result<(String, String), (u16, Value)> {
    let not_host = |message: &str| (409, json!({"error": "not_current_host", "message": message}));
    let st = shared.state.lock().expect("agent state");
    let Some(chain) = st.chain.as_ref() else {
        return Err(not_host("this device is not in the account yet"));
    };
    let Some(me) = chain.machine(&st.machine) else {
        return Err(not_host("this device does not run AI in the account now"));
    };
    // A renewal is a chain v3 entry: a v2 account re-enrolls instead.
    if !chain.is_v3() {
        return Err((
            409,
            json!({"error": "not_v3", "message": "this account's device chain is not at version 3: identity keys are renewed on v3 accounts only"}),
        ));
    }
    if (me.info.sign_key, me.info.dh_key) != (shared.identity.sign_public(), shared.identity.dh_public()) {
        return Err(not_host(
            "the device chain names other keys for this device than the ones it runs with: a renewal it has not taken yet",
        ));
    }
    if req.chain_seq > chain.seq() || chain.hash_at(req.chain_seq) != Some(req.chain_head) {
        return Err((
            409,
            json!({"error": "not_on_chain", "message": "that chain view is not on this device's chain: sync and retry", "chainSeq": chain.seq()}),
        ));
    }
    // The account's renewal policy (`SetRenewalPolicy`, §3.4 rule 9): every
    // renewal confirmed on the device. The chain would never take another
    // (R6 re-review): nothing is staged for one.
    if chain.always_assured() && req.assurance != Assurance::OnDevice {
        return Err((
            409,
            json!({"error": "assurance_required", "message": "this account renews identity keys only when confirmed on the device (assurance 1)"}),
        ));
    }
    Ok((st.account.clone(), st.machine.clone()))
}

/// A `POST /rh/rotate` body.
struct Request {
    nonce: [u8; 16],
    eph: [u8; 32],
    chain_seq: u64,
    chain_head: [u8; 32],
    assurance: Assurance,
}

impl Request {
    fn parse(body: &[u8]) -> Result<Self, &'static str> {
        let v: Value = serde_json::from_slice(body).map_err(|_| "JSON expected")?;
        let nonce = v["nonce"]
            .as_str()
            .and_then(super::link::unhex)
            .and_then(|n| <[u8; 16]>::try_from(n).ok())
            .ok_or("nonce: 16 bytes of hex expected")?;
        let eph = v["eph"].as_str().and_then(unhex32).ok_or("eph: 32 bytes of hex expected")?;
        let chain_seq = v["chainSeq"].as_u64().ok_or("chainSeq: a number expected")?;
        let chain_head = v["chainHead"].as_str().and_then(unhex32).ok_or("chainHead: 32 bytes of hex expected")?;
        let assurance = v["assurance"]
            .as_u64()
            .and_then(|a| u8::try_from(a).ok())
            .and_then(|a| Assurance::from_byte(a).ok())
            .ok_or("assurance: 0 or 1 expected")?;
        Ok(Self {
            nonce,
            eph,
            chain_seq,
            chain_head,
            assurance,
        })
    }
}

/// `rotation_pending` (§3.3): the hub holds a staging for this device and
/// asks this connection, signed in with the old key, to confirm it with the
/// new one. `rotation_ack` when the key asked about is the staged seed's —
/// the new key's signature over this connection's own challenge nonce —
/// else `rotation_unknown`. Nothing at all on a connection whose `welcome`
/// did not offer `rotate/1`.
pub(super) fn on_pending(shared: &Shared, value: &Value, text: &mpsc::UnboundedSender<String>) {
    let Some(session) = shared.rotation.session().filter(|s| s.offered && !s.staged_key) else {
        return;
    };
    let asked = value["sign_key"].as_str().and_then(unhex32);
    let ack = asked.and_then(|asked| {
        let stagings = staged_seeds(&shared.paths, &shared.identity);
        let staged = stagings.iter().find(|s| {
            let new = s.identity();
            new.sign_public() == asked
                && value["dh_key"].as_str().and_then(unhex32) == Some(new.dh_public())
                && value["nonce_hash"].as_str().and_then(unhex32) == Some(nonce_hash(&s.nonce))
        })?;
        let new = staged.identity();
        let st = shared.state.lock().expect("agent state");
        let body = proof(&st.account, &st.machine, staged, &shared.identity, &new);
        Some(chain::sign_rotate_ack(new.signing_key(), &st.account, &st.machine, &body, &session.nonce))
    });
    let reply = match ack {
        Some(sig) => {
            tracing::info!("the hub asks this sign-in to confirm the staged identity key: confirmed");
            json!({"t": "rotation_ack", "sign_key": value["sign_key"], "sig": hex(&sig)})
        }
        None => {
            tracing::warn!(sign_key = %value["sign_key"], "the hub asks to confirm an identity key this device does not hold");
            json!({"t": "rotation_unknown", "sign_key": value["sign_key"]})
        }
    };
    let _ = text.send(reply.to_string());
}

/// What the chain says about this device's own keys now, acted on: renewed
/// to its staged seed — the connection signed in with the old key ends and
/// the next signs in with the new one, to promote; renewed to keys it does
/// not hold — [`foreign`].
pub(super) fn on_chain(shared: &Arc<Shared>) -> Verdict {
    let store = FileSeedStore::for_instance(&shared.paths);
    // One set aside as expired that the chain names after all: back among
    // the stagings first, to be signed in with and promoted.
    let keys = named_keys(&shared.state.lock().expect("agent state"));
    if let Some(keys) = keys.filter(|k| *k != (shared.identity.sign_public(), shared.identity.dh_public())) {
        restore_named(&store, keys, &shared.identity);
    }
    let staged = identities(&staged_seeds(&shared.paths, &shared.identity));
    let any_unreadable = !store.unreadable().is_empty();
    let verdict = verdict(&shared.state.lock().expect("agent state"), &shared.identity, &staged, any_unreadable);
    match &verdict {
        Verdict::Unchanged => {}
        Verdict::Unreadable => unreadable(shared, &store),
        Verdict::Renewed => {
            let mut inner = shared.rotation.lock();
            inner.committed = true;
            if inner.session.is_some_and(|s| !s.staged_key) && !inner.switch {
                tracing::info!("the device chain renewed this device's identity key to the staged one: signing in with it");
                inner.switch = true;
                drop(inner);
                shared.rotation.wake.notify_one();
            }
        }
        Verdict::Foreign { seq, fingerprint } => foreign(shared, *seq, fingerprint),
    }
    verdict
}

/// `foreign_rotation` (§3.5, §3.8 「只剩退役钥时」): this device's own verified
/// chain renewed its id (entry `seq`) to keys it never staged — it holds
/// only a retired identity key: this computer came back from a backup made
/// before a renewal, or another copy of its old key renewed it. No rule lets
/// a retired key renew again: alarmed (kept, in the status stream every
/// device reads, pushed — Web Push and Bark), noted for the local settings
/// (which offer to add this device again under a new identity,
/// [`re_enroll_fresh`]), and the link stops; the old seed stays until then.
///
/// The evidence: a statement the retired key signs over
/// the hash of the renewal that retired it, kept here and delivered to the
/// hub until it takes it ([`super::dispute`]). No sign-in with the retired
/// key is made for it.
fn foreign(shared: &Arc<Shared>, seq: u64, fingerprint: &str) {
    let statement = {
        let mut st = shared.state.lock().expect("agent state");
        st.rotation.foreign_ms.get_or_insert(now_ms());
        st.rotation.foreign_reason = Some(FOREIGN_RENEWED_ELSEWHERE.to_string());
        st.rotation.refused_ms = None;
        st.rotation.unreadable_ms = None;
        // Once the hub took it (its file is gone then), not again.
        if st.rotation.evidence_ms.is_some() {
            Err(())
        } else {
            Ok(super::dispute::statement(&st, &shared.identity, now_ms()))
        }
    };
    tracing::warn!(
        alarm = "foreign_rotation",
        seq,
        %fingerprint,
        "the device chain gave this device's id keys it does not hold; the link stops"
    );
    super::link::raise_alarm(shared, REPLACED_ELSEWHERE.to_string());
    shared.save();
    shared.cancel.cancel();
    match statement {
        Ok(Some(statement)) => {
            match super::dispute::keep(&shared.paths, &statement) {
                Ok(true) => tracing::warn!(seq = statement.seq, "signed a dispute of that renewal with the retired key; delivering it to the hub"),
                Ok(false) => {}
                Err(error) => tracing::error!(%error, "cannot keep the dispute statement"),
            }
            super::dispute::deliver(&shared.paths);
        }
        Ok(None) => tracing::warn!("the device chain retired no key this device holds: no dispute statement to sign"),
        Err(()) => tracing::info!("the hub took this device's dispute statement already"),
    }
}

/// R6 review: the chain renewed this device to keys it does not hold
/// while a staged seed's file does not read — the renewal may be to that
/// one. Not "replaced elsewhere": nothing archived, nothing offered; alarmed
/// ("staged key unreadable", the files named in the log), noted for the
/// settings, and the link stops until the file is put right.
fn unreadable(shared: &Arc<Shared>, store: &FileSeedStore) {
    let files: Vec<String> = store.unreadable().iter().map(|p| p.display().to_string()).collect();
    tracing::error!(
        alarm = "staged_key_unreadable",
        ?files,
        "the device chain renewed this device to keys only a staged identity key that does not read may hold; the link stops"
    );
    let news = {
        let mut st = shared.state.lock().expect("agent state");
        let news = st.rotation.unreadable_ms.is_none();
        st.rotation.unreadable_ms.get_or_insert(now_ms());
        news
    };
    if news {
        super::link::raise_alarm(shared, STAGED_KEY_UNREADABLE.to_string());
    }
    shared.save();
    shared.cancel.cancel();
}

/// What this device says when a staged key it may have been renewed to does
/// not read (R6 review).
pub const STAGED_KEY_UNREADABLE: &str = "Staged key unreadable: this device's identity key was renewed, and the staged key it may \
     have been renewed to does not read (a damaged file beside its key). It stopped connecting: put the file right, or remove \
     this device and add it again.";

/// Whether `st`'s chain names this device with other keys than `identity`
/// (a renewal the link has not taken yet, or one made elsewhere): nothing
/// signs in with `identity` then — the hub keeps a sign-in with a retired
/// key as evidence against this very device.
pub fn renewed_away(st: &AgentState, identity: &MachineIdentity) -> bool {
    st.chain
        .as_ref()
        .and_then(|c| c.device_record(&st.machine))
        .is_some_and(|d| (d.info.sign_key, d.info.dh_key) != (identity.sign_public(), identity.dh_public()))
}

/// Whether this computer's device role (and its relay connections) may sign
/// in with the key in use now (`docs/host-identity-rotation.md` §3.5
/// 「不拿旧钥误伤自己」): always while no staged seed is kept; while one is,
/// only while the link is signed in with the key in use on a connection that
/// heard from the hub within [`LIVE_MS`] by the wall clock — a renewal
/// committed while this device slept, its chain still stale, would make
/// that key retired, and the hub keeps a retired key's sign-in as evidence
/// against this very device. Never once its own chain names other keys for
/// it ([`renewed_away`]: a renewal the link has not taken yet, or one made
/// elsewhere) — R6 re-review.
pub(super) fn device_role_may_sign_in(shared: &Shared) -> bool {
    device_role_may_sign_in_at(shared, now_ms())
}

/// [`device_role_may_sign_in`] at wall-clock time `now`.
pub(super) fn device_role_may_sign_in_at(shared: &Shared, now: u64) -> bool {
    if renewed_away(&shared.state.lock().expect("agent state"), &shared.identity) {
        return false;
    }
    let staged = FileSeedStore::for_instance(&shared.paths).stagings().is_ok_and(|s| !s.is_empty());
    !staged || shared.rotation.key_in_use_confirmed(now)
}

/// Whether `identity` is a seed this device must never use again: one it
/// archived ([`re_enroll_fresh`]), or one the chain it knows (`state.json`'s)
/// renewed away to keys it holds in no staging, waiting or set aside
/// ([`Verdict::Foreign`]). Never one the chain renewed to a key it staged —
/// its own renewal, to be taken, not wiped (R6 review) — nor one whose
/// renewal a staged seed that does not read may hold ([`Verdict::Unreadable`]).
pub fn seed_retired(paths: &Paths, identity: &MachineIdentity) -> bool {
    if archived(paths, identity) {
        return true;
    }
    let Ok(Some(st)) = AgentState::load(paths) else { return false };
    matches!(stored_verdict(paths, &st, identity), Verdict::Foreign { .. })
}

/// Whether `identity`'s seed is one [`re_enroll_fresh`] archived.
pub fn archived(paths: &Paths, identity: &MachineIdentity) -> bool {
    let key = super::key_path(paths);
    let Some(Ok(entries)) = key.parent().map(std::fs::read_dir) else { return false };
    entries.filter_map(Result::ok).any(|e| {
        let name = e.file_name();
        name.to_str().is_some_and(|n| n.starts_with("machine-key.retired-") && n.ends_with(".json"))
            && FileSeedStore::at(e.path())
                .load()
                .ok()
                .flatten()
                .is_some_and(|s| MachineIdentity::from_seed(&s.seed).sign_public() == identity.sign_public())
    })
}

/// The identity an enrollment signs up with (every path: `agent enroll`, the
/// local settings' sign-in): the seed in use — or, when it is one this
/// device must never use again ([`seed_retired`]), a fresh one, the old
/// archived and its enrollment forgotten ([`re_enroll_fresh`]). A retired key
/// never joins again (the chain refuses it, `KEY_RETIRED`).
pub fn identity_for_enrolling(paths: &Paths) -> Result<MachineIdentity, String> {
    if let Some(identity) = finish_re_enroll(paths)? {
        return Ok(identity);
    }
    if let Some(stored) = FileSeedStore::for_instance(paths).load()? {
        let current = MachineIdentity::from_seed(&stored.seed);
        if seed_retired(paths, &current) {
            tracing::warn!("the identity key in use was replaced elsewhere (or archived): this enrollment uses a fresh one");
            return re_enroll_fresh(paths, now_ms());
        }
    }
    super::load_or_create(paths).map(|(identity, _)| identity)
}

/// `<instance dir>/agent/re-enroll-fresh.json`: [`re_enroll_fresh`] began and
/// has not finished (R6 review): whatever start comes next finishes it
/// ([`finish_re_enroll`]).
fn re_enroll_marker(paths: &Paths) -> std::path::PathBuf {
    super::key_path(paths).with_file_name("re-enroll-fresh.json")
}

/// "Add this device again with a new identity" (§3.8 「只剩退役钥时」): for a
/// device left holding only a retired key, which no rule lets renew in
/// place. The link must be stopped first ([`super::control::Control::re_enroll_fresh`]).
///
/// Refused when this device's own chain renewed it to a key it staged
/// (waiting, or set aside as expired) — that is its own renewal, which it
/// takes; nothing is archived or forgotten — or when a staged key that does
/// not read may be the one (R6 review).
///
/// Otherwise the seed in use is archived as `machine-key.retired-<ms>.json` —
/// kept, never loaded again — and the staged ones dropped; the old enrollment
/// is forgotten (the other devices' keys and copies, the machine keys, the
/// streams' store, `state.json` last); and a fresh seed is made: the instance
/// is back at "enter a registration code", and joins under a new id. A
/// marker is written first and removed last, so a crash anywhere in between
/// is finished at the next start ([`finish_re_enroll`]) — never the old seed
/// in use without its enrollment, never an enrollment without its seed.
pub fn re_enroll_fresh(paths: &Paths, now: u64) -> Result<MachineIdentity, String> {
    if let Some(identity) = finish_re_enroll(paths)? {
        return Ok(identity);
    }
    let store = FileSeedStore::for_instance(paths);
    if let (Ok(Some(seed)), Ok(Some(st))) = (store.load(), AgentState::load(paths)) {
        match stored_verdict(paths, &st, &MachineIdentity::from_seed(&seed.seed)) {
            Verdict::Renewed => {
                return Err(
                    "this device's own chain renewed it to an identity key it staged: it takes that key, and is not added again"
                        .into(),
                )
            }
            Verdict::Unreadable => {
                return Err(
                    "a staged identity key of this device does not read, and may be the one its chain renewed it to: put the file right first"
                        .into(),
                )
            }
            Verdict::Unchanged | Verdict::Foreign { .. } => {}
        }
    }
    let marker = re_enroll_marker(paths);
    super::write_private(&marker, b"{}")?;
    super::sync_dir(&marker);
    forget_for_a_fresh_identity(paths, now)
}

/// A [`re_enroll_fresh`] a crash interrupted, finished (at the link's start,
/// the control's, and before any sign-up): the new identity, or `None` when
/// none was interrupted.
pub fn finish_re_enroll(paths: &Paths) -> Result<Option<MachineIdentity>, String> {
    if !re_enroll_marker(paths).exists() {
        return Ok(None);
    }
    tracing::warn!("adding this device again under a new identity was interrupted: finishing it");
    forget_for_a_fresh_identity(paths, now_ms()).map(Some)
}

#[cfg(test)]
thread_local! {
    /// Tests: [`forget_for_a_fresh_identity`] stops right after archiving, as
    /// a crash there would.
    static CRASH_AFTER_ARCHIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`re_enroll_fresh`]'s steps after its marker; each may run again.
fn forget_for_a_fresh_identity(paths: &Paths, now: u64) -> Result<MachineIdentity, String> {
    let current = super::key_path(paths);
    if current.exists() {
        // Whatever seed is in place: the old one, or (a crash after the
        // fresh one was made) one never enrolled anywhere.
        let archived = free_archive_path(paths, now);
        std::fs::rename(&current, &archived).map_err(|e| format!("{} → {}: {e}", current.display(), archived.display()))?;
        super::sync_dir(&current);
        tracing::warn!(archived = %archived.display(), "archived this device's retired identity key; it is never used again");
    }
    #[cfg(test)]
    if CRASH_AFTER_ARCHIVE.with(|c| c.replace(false)) {
        return Err("crashed after archiving (test)".into());
    }
    FileSeedStore::for_instance(paths).discard_all_staged()?;
    super::state::forget_enrollment_leftovers(paths)?;
    crate::aggregator::forget_all(paths)?;
    match std::fs::remove_file(super::state::state_path(paths)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    let (identity, created) = super::load_or_create(paths)?;
    if !created {
        return Err("a seed was in place after the old one was archived".into());
    }
    let marker = re_enroll_marker(paths);
    std::fs::remove_file(&marker).map_err(|e| format!("{}: {e}", marker.display()))?;
    super::sync_dir(&marker);
    tracing::warn!(
        fingerprint = %super::control::machine_fingerprint(&identity),
        "made a fresh identity for this device: add it to the account again with a new registration code"
    );
    Ok(identity)
}

/// Where [`re_enroll_fresh`] archives a retired seed:
/// `<instance dir>/agent/machine-key.retired-<ms>.json`.
pub fn archived_key_path(paths: &Paths, now: u64) -> std::path::PathBuf {
    super::key_path(paths).with_file_name(format!("machine-key.retired-{now}.json"))
}

/// [`archived_key_path`], or beside it when that is taken (an archive is
/// never written over: it is what keeps its seed from being used again).
fn free_archive_path(paths: &Paths, now: u64) -> std::path::PathBuf {
    let first = archived_key_path(paths, now);
    if !first.exists() {
        return first;
    }
    (1u32..)
        .map(|n| first.with_file_name(format!("machine-key.retired-{now}-{n}.json")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
thread_local! {
    /// Tests: what happens in [`promote`] between saving the state and
    /// renaming the staged seed into place.
    pub(super) static BETWEEN_SAVE_AND_SWAP: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// Take the staged seed the chain names (see the module docs): only once
/// this device's own verified chain names exactly its keys, and only the key
/// that just signed in to the hub (`signed_in`). Renamed into place
/// atomically — the other stagings go — noted for the local settings, and
/// the link restarted with it.
pub(super) fn promote(shared: &Arc<Shared>, signed_in: &MachineIdentity) -> Result<(), String> {
    let store = FileSeedStore::for_instance(&shared.paths);
    let stagings = staged_seeds(&shared.paths, &shared.identity);
    let new = {
        let st = shared.state.lock().expect("agent state");
        let staged = named(&st, &stagings).ok_or("the device chain names none of the staged identity keys")?;
        if staged.identity().sign_public() != signed_in.sign_public() {
            return Err("the device chain names another staged identity key than the one that signed in".into());
        }
        staged.clone()
    };
    let previous = super::control::machine_fingerprint(&shared.identity);
    // The push keys follow the identity key (R6 review): noted before the
    // rename, so a crash after it still renews them at the next start.
    let push_renew = std::mem::replace(&mut shared.state.lock().expect("agent state").rotation.push_renew, true);
    // Saved before the rename, or nothing is renamed (final review of the
    // agent's renewal): the state on disk must hold the chain that names the
    // new key before that key is the one in use. Else, after an earlier
    // renewal, the next start reads a chain naming keys this device holds
    // nowhere any more, with a renewal on it, and takes its own renewal for
    // one made elsewhere. The staged seed is kept: the next sign-in with it
    // tries again.
    if let Err(error) = shared.try_save() {
        shared.state.lock().expect("agent state").rotation.push_renew = push_renew;
        return Err(format!("cannot save the agent state, so the renewed identity key is not taken yet: {error}"));
    }
    #[cfg(test)]
    if let Some(between) = BETWEEN_SAVE_AND_SWAP.with(|b| b.borrow_mut().take()) {
        between();
    }
    // Not renamed (the rename is the swap): the key in use stays, and so do
    // its push keys — not refused as being renewed (review of the renewal
    // fixes), in memory and on disk.
    if let Err(error) = store.promote(&new.nonce) {
        shared.state.lock().expect("agent state").rotation.push_renew = push_renew;
        shared.save();
        return Err(error);
    }
    let new = new.identity();
    {
        let mut st = shared.state.lock().expect("agent state");
        st.rotation.renewed_ms = Some(now_ms());
        st.rotation.previous_fingerprint = Some(previous.clone());
        st.rotation.foreign_ms = None;
        st.rotation.foreign_reason = None;
        st.rotation.unreadable_ms = None;
    }
    shared.save();
    {
        shared.rotation.lock().taken.clear();
    }
    tracing::warn!(
        %previous,
        fingerprint = %super::control::machine_fingerprint(&new),
        "renewed this device's identity key: restarting its link with it"
    );
    (shared.restart)();
    Ok(())
}

/// Where this device stands with renewing its identity key (`GET /api/cloud`
/// `link.rotation`, `droppen agent status`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `idle`; `staged` (a new key waits for the renewal to commit);
    /// `renewed` (within the last day); `refused` (the hub refuses the key in
    /// use as renewed but showed no renewal: it retries, nothing else
    /// changes); `foreign` (its own chain gave its id keys it does not hold:
    /// the link stopped, and it may be added again under a new identity);
    /// `unreadable` (renewed, and a staged key it may have been renewed to
    /// does not read: the link stopped until the file is put right).
    pub state: String,
    /// The code to type on the admin device (`XXXX-XXXX`), while a
    /// high-assurance renewal waits.
    pub code: Option<String>,
    /// Until when the code is shown (ms).
    pub code_expires_ms: Option<u64>,
    /// When this device last took a renewed key (ms).
    pub renewed_ms: Option<u64>,
    /// The fingerprint it had before.
    pub previous_fingerprint: Option<String>,
    /// Why: `renewed_elsewhere` for `foreign`, `rotated` for `refused`,
    /// `staged_key_unreadable` for `unreadable`.
    pub reason: Option<String>,
}

/// [`Status`] from this instance's files: the staged seed beside the key in
/// use, and `st` (none when not enrolled). Reads only; nothing is made or
/// forgotten here (another process — `agent status` — may call it).
pub fn status(paths: &Paths, st: Option<&AgentState>, now: u64) -> Status {
    let idle = || Status {
        state: "idle".into(),
        ..Status::default()
    };
    let Some(st) = st else { return idle() };
    if st.rotation.foreign_ms.is_some() {
        return Status {
            state: "foreign".into(),
            reason: st.rotation.foreign_reason.clone(),
            ..Status::default()
        };
    }
    if st.rotation.unreadable_ms.is_some() {
        return Status {
            state: "unreadable".into(),
            reason: Some(UNREADABLE_REASON.into()),
            ..Status::default()
        };
    }
    if st.rotation.refused_ms.is_some() {
        return Status {
            state: "refused".into(),
            reason: Some(REFUSED_ROTATED.into()),
            ..Status::default()
        };
    }
    let store = FileSeedStore::for_instance(paths);
    let seed = store.load().ok().flatten();
    if let Some(seed) = seed {
        let current = MachineIdentity::from_seed(&seed.seed);
        // This instance's stagings, not the key in use, that a renewal may
        // still take (not expired, or named by the chain).
        let stagings: Vec<StagedSeed> = store
            .stagings()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.uuid == seed.uuid && s.identity().sign_public() != current.sign_public())
            .collect();
        let named = named(st, &stagings).map(|s| s.nonce);
        // Only one the hub took (or the chain names): one it never answered
        // is kept in case, not shown — nor its code.
        let waiting: Vec<&StagedSeed> = stagings
            .iter()
            .filter(|s| (s.accepted_ms.is_some() && now <= expiry(s)) || Some(s.nonce) == named)
            .collect();
        if !waiting.is_empty() {
            // The code of the newest high-assurance renewal, for ten minutes.
            let code = waiting
                .iter()
                .filter(|s| s.assurance == Assurance::OnDevice && now < s.created_ms.saturating_add(CODE_FOR_MS))
                .max_by_key(|s| s.created_ms)
                .map(|s| {
                    (
                        chain::rotate_sas(&proof(&st.account, &st.machine, s, &current, &s.identity())),
                        s.created_ms.saturating_add(CODE_FOR_MS),
                    )
                });
            return Status {
                state: "staged".into(),
                code_expires_ms: code.as_ref().map(|(_, until)| *until),
                code: code.map(|(code, _)| code),
                ..Status::default()
            };
        }
    }
    match st.rotation.renewed_ms {
        Some(at) if now.saturating_sub(at) < RENEWED_SHOWN_MS => Status {
            state: "renewed".into(),
            renewed_ms: Some(at),
            previous_fingerprint: st.rotation.previous_fingerprint.clone(),
            ..Status::default()
        },
        _ => idle(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::state::RotationNote;

    fn staged(paths: &Paths, seed: u8, assurance: Assurance, created_ms: u64) -> StagedSeed {
        StagedSeed {
            uuid: crate::identity::ensure(paths).unwrap().uuid,
            seed: Zeroizing::new([seed; 32]),
            nonce: [3; 16],
            chain_seq: 2,
            chain_head: [4; 32],
            assurance,
            created_ms,
            // Taken by the hub as it was made.
            accepted_ms: Some(created_ms),
        }
    }

    /// The hub phase's review: a page whose `/rh/rotate` answer was lost asks
    /// again with a new nonce. A staging the hub took stays — the hub may
    /// still commit it — until it expires (only judged once the key in use
    /// signed in after it, which no commit allows), at most
    /// [`MAX_STAGINGS`]; one the hub never answered stays, unshown, for its
    /// attempt's ten minutes ([`ATTEMPT_MS`]).
    #[test]
    fn stagings_the_hub_took_stay_until_they_expire() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rotroom");
        let (current, _) = super::super::load_or_create(&paths).unwrap();
        let store = FileSeedStore::for_instance(&paths);
        let at = |seed: u8, nonce: u8, created: u64, accepted: Option<u64>| StagedSeed {
            nonce: [nonce; 16],
            accepted_ms: accepted,
            ..staged(&paths, seed, Assurance::Auto, created)
        };
        let now = 100 * STAGED_FOR_MS;
        let st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5);
        let nonces = |store: &FileSeedStore| -> Vec<u8> { store.stagings().unwrap().iter().map(|s| s.nonce[0]).collect() };
        // What "goes" is set aside, never deleted (R6 review).
        let aside = |store: &FileSeedStore| -> Vec<u8> {
            let mut n: Vec<u8> = store.expired().iter().map(|s| s.nonce[0]).collect();
            n.sort_unstable();
            n
        };
        store.stage(&at(10, 1, now - 10, Some(now - 9))).unwrap(); // taken
        store.stage(&at(11, 2, now - 8, None)).unwrap(); // not answered yet: within its attempt
        store.stage(&at(13, 4, now - ATTEMPT_MS - 1, None)).unwrap(); // never answered, its attempt over
        store.stage(&at(12, 3, now - 2 * STAGED_FOR_MS, Some(now - 2 * STAGED_FOR_MS))).unwrap(); // taken, expired
        make_room(&store, &staged_seeds(&paths, &current), now, now);
        assert_eq!(nonces(&store), vec![1, 2], "the expired ones make room; the taken one and the young unanswered one stay");
        assert_eq!(aside(&store), vec![3, 4], "set aside, not deleted");
        assert!(store.expired_path(&[3; 16]).exists() && !store.staging_path(&[3; 16]).exists());
        assert!(staged_seeds(&paths, &current).iter().all(|s| s.nonce[0] != 3), "never tried at sign-in again");
        // The unanswered one goes once its attempt is over.
        set_aside_expired(&paths, named_keys(&st), &current, now - 8 + ATTEMPT_MS + 1);
        assert_eq!(nonces(&store), vec![1]);
        assert_eq!(aside(&store), vec![2, 3, 4]);
        // Expired, but the key in use last signed in before it expired: kept.
        store.restore(&[3; 16]).unwrap();
        make_room(&store, &staged_seeds(&paths, &current), now - 2 * STAGED_FOR_MS, now);
        assert_eq!(nonces(&store), vec![3, 1]);
        set_aside_expired(&paths, named_keys(&st), &current, now - STAGED_FOR_MS - 1);
        assert_eq!(nonces(&store), vec![3, 1], "not expired then");
        set_aside_expired(&paths, named_keys(&st), &current, now);
        assert_eq!(nonces(&store), vec![1], "the key in use signed in after it expired: never committed");
        // At most four the hub took: the oldest goes for a fifth.
        for n in 4..7u8 {
            store.stage(&at(20 + n, n, now + u64::from(n), Some(now + u64::from(n)))).unwrap();
        }
        assert_eq!(nonces(&store).len(), 4);
        make_room(&store, &staged_seeds(&paths, &current), now, now);
        assert_eq!(nonces(&store), vec![4, 5, 6], "room for one more, the oldest gone");
        assert!(aside(&store).contains(&1), "set aside too");
        // A staging of the key in use, or of another instance, is not one.
        let in_use = store.load().unwrap().unwrap().seed;
        store.stage(&StagedSeed { seed: in_use, ..at(0, 9, now, None) }).unwrap();
        store.stage(&StagedSeed { uuid: "another".into(), ..at(30, 8, now, None) }).unwrap();
        assert_eq!(staged_seeds(&paths, &current).len(), 3);
        assert_eq!(nonces(&store), vec![4, 5, 6]);
    }

    /// §3.8 「只剩退役钥时」: "add this device again with a new
    /// identity" archives the seed in use — kept, never loaded again —
    /// forgets the enrollment and whatever was staged, and makes a fresh seed.
    #[test]
    fn re_enrolling_fresh_archives_the_retired_seed_and_forgets_the_enrollment() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rotfresh");
        let (old, _) = super::super::load_or_create(&paths).unwrap();
        AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5).save(&paths).unwrap();
        crate::agent::state::store_machine_key(&paths, 0, &[7; 32]).unwrap();
        FileSeedStore::for_instance(&paths).stage(&staged(&paths, 8, Assurance::Auto, 5)).unwrap();
        let fresh = re_enroll_fresh(&paths, 1234).unwrap();
        assert_ne!(fresh.sign_public(), old.sign_public(), "a fresh seed");
        assert_ne!(fresh.sign_public(), MachineIdentity::from_seed(&[8; 32]).sign_public(), "not the staged one");
        let archived = archived_key_path(&paths, 1234);
        assert!(archived.ends_with("agent/machine-key.retired-1234.json"));
        let kept = FileSeedStore::at(archived).load().unwrap().unwrap();
        assert_eq!(MachineIdentity::from_seed(&kept.seed).sign_public(), old.sign_public(), "the old seed is kept, archived");
        // Never used again: the seed in use is the fresh one, for good.
        let (now, created) = super::super::load_or_create(&paths).unwrap();
        assert!(!created);
        assert_eq!(now.sign_public(), fresh.sign_public());
        // The enrollment is forgotten: no state, no machine keys, nothing staged.
        assert!(!crate::agent::state::exists(&paths));
        assert!(AgentState::load(&paths).unwrap().is_none());
        assert!(!crate::agent::state::keys_path(&paths).exists());
        assert!(FileSeedStore::for_instance(&paths).staged().unwrap().is_none());
    }

    /// An archived (retired) seed is never loaded again — not
    /// by `load_or_create`, not after a crash left only the archive — and
    /// every enrollment signs up with a fresh seed when the one in use is
    /// retired on the chain this device knows, or one it archived.
    #[test]
    fn a_retired_or_archived_seed_never_signs_up_again() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rotenrol");
        // Not retired: the seed in use enrolls.
        let (old, _) = super::super::load_or_create(&paths).unwrap();
        assert!(!seed_retired(&paths, &old));
        assert_eq!(identity_for_enrolling(&paths).unwrap().sign_public(), old.sign_public());
        // Archived, then a crash put the archive back in place (or a copy of it):
        // never loaded, and enrolling makes a fresh seed.
        let archive = archived_key_path(&paths, 5);
        std::fs::copy(super::super::key_path(&paths), &archive).unwrap();
        assert!(archived(&paths, &old));
        let refused = super::super::load_or_create(&paths).err().expect("an archived seed is not loaded");
        assert!(refused.contains("archived"), "{refused}");
        let fresh = identity_for_enrolling(&paths).unwrap();
        assert_ne!(fresh.sign_public(), old.sign_public());
        assert_eq!(super::super::load_or_create(&paths).unwrap().0.sign_public(), fresh.sign_public());
        // Only the archive left (a crash right after archiving): a fresh seed, never the archive.
        std::fs::remove_file(super::super::key_path(&paths)).unwrap();
        let (made, created) = super::super::load_or_create(&paths).unwrap();
        assert!(created && made.sign_public() != old.sign_public() && made.sign_public() != fresh.sign_public());

        // Retired on the chain it knows: a v3 account renewed this seed away.
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rotenrol2");
        let (current, _) = super::super::load_or_create(&paths).unwrap();
        renewed_to(&current, &MachineIdentity::from_seed(&[66; 32])).save(&paths).unwrap();
        assert!(seed_retired(&paths, &current), "retired on the chain it knows");
        let fresh = identity_for_enrolling(&paths).unwrap();
        assert_ne!(fresh.sign_public(), current.sign_public());
        assert!(archived(&paths, &current), "the retired seed is archived, kept");
        assert!(AgentState::load(&paths).unwrap().is_none(), "the old enrollment is forgotten");
        assert_eq!(identity_for_enrolling(&paths).unwrap().sign_public(), fresh.sign_public(), "and the fresh one stays");
    }

    /// m-1's state with a v3 chain that added it with `current`'s keys, then
    /// renewed it to `to`'s.
    fn renewed_to(current: &MachineIdentity, to: &MachineIdentity) -> AgentState {
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let d1 = MachineIdentity::from_seed(&[2; 32]);
        let genesis = chain::genesis_draft(
            "a-1",
            ak.sign_public(),
            chain::DeviceInfo {
                id: "d-1".into(),
                sign_key: d1.sign_public(),
                dh_key: d1.dh_public(),
                full: true,
            },
            None,
            1_000,
        )
        .sign(ak.signing_key());
        let mut hub = chain::ChainState::verify(std::slice::from_ref(&genesis), None, &mut chain::ChainClock::history()).unwrap();
        let mut entries = vec![genesis];
        let mut push = |hub: &mut chain::ChainState, action: chain::Action| {
            let e = hub.draft(chain::Signer::Device("d-1".into()), action, 2_000 + hub.seq()).sign(d1.signing_key());
            hub.apply(&e, &mut chain::ChainClock::history()).unwrap();
            entries.push(e);
        };
        let epoch = |e: u32| {
            chain::Action::KeyEpochs(chain::KeyCommits {
                user: None,
                machines: vec![("m-1".into(), chain::KeyCommit { epoch: e, hash: [e as u8; 32] })],
            })
        };
        push(
            &mut hub,
            chain::Action::AddDevice3(chain::DeviceV3 {
                id: "m-1".into(),
                sign_key: current.sign_public(),
                dh_key: current.dh_public(),
                flags: chain::Flags { host: true, admin: false },
            }),
        );
        push(&mut hub, epoch(0));
        let mut r = chain::HostRotation {
            host: "m-1".into(),
            sign_key: to.sign_public(),
            dh_key: to.dh_public(),
            staged_seq: hub.seq(),
            staged_head: hub.head(),
            nonce: [1; 16],
            assurance: Assurance::Auto,
            old_sig: [0; 64],
            new_sig: [0; 64],
        };
        let body = r.body("a-1", &current.sign_public(), &current.dh_public());
        r.old_sig = sig::sign(current.signing_key(), sig::HOST_ROTATE, &body);
        r.new_sig = sig::sign(to.signing_key(), sig::HOST_ROTATE, &body);
        push(&mut hub, chain::Action::RotateHostKey(r));
        push(&mut hub, epoch(1));
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", rh_proto::identity::fingerprint(&ak.sign_public()), 5);
        assert!(st.absorb(&entries, now_ms()).error.is_none());
        st
    }

    /// R6 review: an honest host whose own renewal was committed — its
    /// chain names a key it staged, waiting or set aside as expired — is
    /// never taken for one replaced elsewhere: no sign-in path (the local
    /// settings' sign-in, `agent enroll`) and no "add again" wipes it.
    #[tokio::test]
    async fn an_honest_hosts_own_renewal_is_never_wiped_for_a_fresh_identity() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rothonest");
        let (current, _) = super::super::load_or_create(&paths).unwrap();
        let store = FileSeedStore::for_instance(&paths);
        let mine = staged(&paths, 8, Assurance::Auto, 5);
        store.stage(&mine).unwrap();
        renewed_to(&current, &mine.identity()).save(&paths).unwrap();
        let untouched = |why: &str| {
            assert!(!seed_retired(&paths, &current), "{why}: its own renewal");
            assert!(!archived(&paths, &current), "{why}: nothing archived");
            assert!(crate::agent::state::exists(&paths), "{why}: the enrollment stays");
            assert_eq!(
                MachineIdentity::from_seed(&store.load().unwrap().unwrap().seed).sign_public(),
                current.sign_public(),
                "{why}: the seed in use stays (the link promotes the staged one)"
            );
            assert_eq!(kept_stagings(&store, &current).len(), 1, "{why}: the staged seed stays");
        };
        let st = AgentState::load(&paths).unwrap().unwrap();
        assert_eq!(stored_verdict(&paths, &st, &current), Verdict::Renewed);
        assert_eq!(identity_for_enrolling(&paths).unwrap().sign_public(), current.sign_public());
        untouched("signing up");
        let refused = re_enroll_fresh(&paths, 77).err().expect("refused");
        assert!(refused.contains("staged"), "{refused}");
        untouched("adding it again");
        let control = super::super::control::Control::new(paths.clone());
        assert!(!control.renew_retired_seed().await.unwrap(), "the local settings' sign-in");
        untouched("the local settings' sign-in");
        // However long ago it expired, one the chain names is never set aside (A2).
        set_aside_expired(&paths, named_keys(&st), &current, u64::MAX);
        assert_eq!(store.stagings().unwrap().len(), 1);
        // Set aside as expired: still its own.
        store.expire(&mine.nonce).unwrap();
        assert!(store.stagings().unwrap().is_empty());
        assert!(identity_for_enrolling(&paths).is_ok() && re_enroll_fresh(&paths, 78).is_err());
        assert!(!control.renew_retired_seed().await.unwrap());
        untouched("set aside");
        // No staging of it at all: replaced elsewhere — only then a fresh one.
        store.discard_all_staged().unwrap();
        assert!(seed_retired(&paths, &current));
        assert!(control.renew_retired_seed().await.unwrap());
        assert!(archived(&paths, &current));
    }

    /// R6 review: a staged seed whose file does not read, while the chain
    /// renewed this device to keys it holds in no other: its own verdict,
    /// never "replaced elsewhere" — the seed in use is not retired, and
    /// adding the device again is refused until the file is put right.
    #[test]
    fn an_unreadable_staging_is_never_taken_for_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rotunread");
        let (current, _) = super::super::load_or_create(&paths).unwrap();
        let store = FileSeedStore::for_instance(&paths);
        let mine = staged(&paths, 8, Assurance::Auto, 5);
        let st = renewed_to(&current, &mine.identity());
        st.save(&paths).unwrap();
        // The staged file is damaged.
        super::super::write_private(&store.staging_path(&mine.nonce), b"{\"version\":1,\"seed\":").unwrap();
        assert_eq!(store.unreadable().len(), 1);
        assert_eq!(verdict(&st, &current, &[], true), Verdict::Unreadable);
        assert_eq!(stored_verdict(&paths, &st, &current), Verdict::Unreadable);
        assert!(!seed_retired(&paths, &current));
        assert!(re_enroll_fresh(&paths, 5).err().is_some_and(|e| e.contains("does not read")));
        assert!(!archived(&paths, &current) && crate::agent::state::exists(&paths));
        // One set aside as expired that does not read counts the same.
        std::fs::rename(store.staging_path(&mine.nonce), store.expired_path(&mine.nonce)).unwrap();
        assert_eq!(stored_verdict(&paths, &st, &current), Verdict::Unreadable);
        // Put right: its own renewal.
        std::fs::remove_file(store.expired_path(&mine.nonce)).unwrap();
        store.stage(&mine).unwrap();
        assert_eq!(stored_verdict(&paths, &st, &current), Verdict::Renewed);
    }

    /// R6 review: a crash inside "add this device again" — the seed
    /// archived, `state.json` still there — is finished at the next start
    /// (or sign-up): never the old enrollment without its seed.
    #[test]
    fn adding_again_cut_short_by_a_crash_is_finished_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rotcrash");
        let (old, _) = super::super::load_or_create(&paths).unwrap();
        renewed_to(&old, &MachineIdentity::from_seed(&[66; 32])).save(&paths).unwrap();
        crate::agent::state::store_machine_key(&paths, 0, &[7; 32]).unwrap();
        FileSeedStore::for_instance(&paths).stage(&staged(&paths, 8, Assurance::Auto, 5)).unwrap();
        CRASH_AFTER_ARCHIVE.with(|c| c.set(true));
        assert!(re_enroll_fresh(&paths, 1234).is_err(), "crashed");
        // What the crash left: the seed archived, the enrollment still there.
        assert!(!super::super::key_path(&paths).exists());
        assert!(archived(&paths, &old));
        assert!(crate::agent::state::exists(&paths));
        assert!(re_enroll_marker(&paths).exists());
        // The next start finishes it.
        let fresh = finish_re_enroll(&paths).unwrap().expect("finished");
        assert_ne!(fresh.sign_public(), old.sign_public());
        assert!(!crate::agent::state::exists(&paths), "the old enrollment is forgotten");
        assert!(!crate::agent::state::keys_path(&paths).exists());
        assert!(FileSeedStore::for_instance(&paths).staged().unwrap().is_none());
        assert!(!re_enroll_marker(&paths).exists());
        assert_eq!(super::super::load_or_create(&paths).unwrap().0.sign_public(), fresh.sign_public());
        assert!(finish_re_enroll(&paths).unwrap().is_none(), "nothing left to finish");
        // Cut short again, and a sign-up comes first: it finishes it too.
        renewed_to(&fresh, &MachineIdentity::from_seed(&[67; 32])).save(&paths).unwrap();
        CRASH_AFTER_ARCHIVE.with(|c| c.set(true));
        assert!(re_enroll_fresh(&paths, 1235).is_err());
        let again = identity_for_enrolling(&paths).unwrap();
        assert!(again.sign_public() != fresh.sign_public() && again.sign_public() != old.sign_public());
        assert!(!crate::agent::state::exists(&paths) && !re_enroll_marker(&paths).exists());
        assert!(archived(&paths, &old) && archived(&paths, &fresh), "both archived, neither written over");
    }

    /// A log line, kept for a test.
    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// R6 review: the confirmation code is never logged at WARN (nor
    /// INFO) — the person reads it in the settings — only at DEBUG.
    #[test]
    fn the_confirmation_code_is_logged_at_debug_at_most() {
        let at = |level: tracing::Level| {
            let logs = Logs::default();
            let writer = logs.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || writer.clone())
                .with_ansi(false)
                .with_max_level(level)
                .finish();
            tracing::subscriber::with_default(subscriber, || log_code("ABCD-1234"));
            let text = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
            text
        };
        let info = at(tracing::Level::INFO);
        assert!(info.contains("asks you to confirm it"), "{info}");
        assert!(!info.contains("ABCD-1234"), "{info}");
        assert!(at(tracing::Level::DEBUG).contains("ABCD-1234"));
    }

    /// `link.rotation` from the files: idle; staged (a code only for high
    /// assurance, ten minutes); renewed for a day; foreign over everything.
    #[test]
    fn the_status_reads_what_the_files_say() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rotstatus");
        let (current, _) = super::super::load_or_create(&paths).unwrap();
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5);
        let now = 10_000_000;
        assert_eq!(status(&paths, None, now).state, "idle");
        assert_eq!(status(&paths, Some(&st), now).state, "idle");
        let store = FileSeedStore::for_instance(&paths);
        store.stage(&staged(&paths, 8, Assurance::Auto, now - 1_000)).unwrap();
        let shown = status(&paths, Some(&st), now);
        assert_eq!((shown.state.as_str(), shown.code.as_ref()), ("staged", None), "no code without high assurance");
        let high = staged(&paths, 8, Assurance::OnDevice, now - 1_000);
        store.stage(&high).unwrap();
        let shown = status(&paths, Some(&st), now);
        let body = proof("a-1", "m-1", &high, &current, &high.identity());
        assert_eq!(shown.code.as_deref(), Some(chain::rotate_sas(&body).as_str()));
        assert_eq!(shown.code_expires_ms, Some(now - 1_000 + CODE_FOR_MS));
        // A staged seed older than any staging the hub keeps (and not on the chain) is not shown…
        assert_eq!(status(&paths, Some(&st), now + STAGED_FOR_MS).state, "idle");
        // …and is set aside once the key in use signed in after it expired.
        assert_eq!(staged_seeds(&paths, &current).len(), 1, "not on reading alone");
        set_aside_expired(&paths, named_keys(&st), &current, now + STAGED_FOR_MS);
        assert!(store.staged().unwrap().is_none());
        assert_eq!(status(&paths, Some(&st), now).state, "idle", "one set aside is not shown");
        st.rotation = RotationNote {
            renewed_ms: Some(now),
            previous_fingerprint: Some("aaaa".into()),
            ..RotationNote::default()
        };
        let shown = status(&paths, Some(&st), now + 1);
        assert_eq!(
            (shown.state.as_str(), shown.renewed_ms, shown.previous_fingerprint.as_deref()),
            ("renewed", Some(now), Some("aaaa"))
        );
        assert_eq!(status(&paths, Some(&st), now + RENEWED_SHOWN_MS).state, "idle", "a day later");
        st.rotation.refused_ms = Some(now);
        let shown = status(&paths, Some(&st), now + 1);
        assert_eq!((shown.state.as_str(), shown.reason.as_deref()), ("refused", Some(REFUSED_ROTATED)));
        st.rotation.unreadable_ms = Some(now);
        let shown = status(&paths, Some(&st), now + 1);
        assert_eq!((shown.state.as_str(), shown.reason.as_deref()), ("unreadable", Some(UNREADABLE_REASON)));
        st.rotation.foreign_ms = Some(now);
        st.rotation.foreign_reason = Some(FOREIGN_RENEWED_ELSEWHERE.into());
        let shown = status(&paths, Some(&st), now + 1);
        assert_eq!((shown.state.as_str(), shown.reason.as_deref()), ("foreign", Some(FOREIGN_RENEWED_ELSEWHERE)));
        // A staged seed of another instance is not this device's.
        let mut other = staged(&paths, 8, Assurance::Auto, now);
        other.uuid = "someone-else".into();
        store.stage(&other).unwrap();
        st.rotation = RotationNote::default();
        assert_eq!(status(&paths, Some(&st), now).state, "idle");
    }
}
