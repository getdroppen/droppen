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
//!   (`machine-key.next.json`, [`super::SeedStore::stage`]); the seed in use
//!   is never touched. The answer: both keys' signatures over the proof
//!   ([`rh_proto::chain::host_rotate_body`]) and the DH confirmation MAC. The
//!   same nonce gets the same answer; a new nonce replaces the staged seed.
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
//! - **Promotion** — once its chain renews this device to exactly the staged
//!   keys (the entry, or a hub closing with 4005, which makes it sign in
//!   with the new key and read the chain), it signs in with the new key,
//!   sees the entry on the chain it verified, renames the staged seed into
//!   place, and restarts its link — relays, control end and the streams'
//!   writer with it — in this process ([`promote`]). AI sessions and
//!   terminals do not notice. A lying hub cannot make it give up its seed:
//!   only its own verified chain promotes.
//! - **Crash safety** — at start, a staged seed is tried first; a hub that
//!   does not take it yet (`denied`) gets the old one. Every step leaves one
//!   of: the old seed; the old seed and a staged one; the new seed.
//! - **Replaced elsewhere** — a verified entry on its own chain renewing
//!   this device's id to keys it did not stage: this computer came back from
//!   a backup made before a renewal, or another copy of its old key renewed
//!   it. A `foreign_rotation` alarm (logged, in the status stream every
//!   device reads, pushed with Bark), the link stops and the old seed stays
//!   ([`Verdict::Foreign`]) until a person adds this device again under a new
//!   identity ([`re_enroll_fresh`]); every sign-up after it uses a fresh seed
//!   ([`identity_for_enrolling`]). A hub refusing the key as `rotated` is not
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
pub const STAGE_WAIT: Duration = Duration::from_secs(10);
/// How long a high-assurance renewal's code is shown (§3.7).
pub const CODE_FOR_MS: u64 = 10 * 60 * 1000;
/// How long `link.rotation` says a renewal just happened.
pub const RENEWED_SHOWN_MS: u64 = 24 * 60 * 60 * 1000;
/// A staged seed older than this that the chain does not name is forgotten:
/// the hub expires a staging after a day.
pub const STAGED_FOR_MS: u64 = 48 * 60 * 60 * 1000;
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
}

/// A staging the hub took: what the same nonce is answered with again.
struct Taken {
    nonce: [u8; 16],
    ready_ms: Option<u64>,
}

#[derive(Default)]
struct Inner {
    session: Option<HubSession>,
    /// `rotation_stage`s waiting for the hub's answer, by nonce hash (hex).
    waiting: HashMap<String, oneshot::Sender<Value>>,
    taken: Option<Taken>,
    /// Sign in with the staged key first: at start, after the chain renewed
    /// this device to it, after a 4005.
    try_new: bool,
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
    /// At start, a staged seed is tried first.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                try_new: true,
                ..Inner::default()
            }),
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

    /// Whether the next sign-in tries the staged key first.
    pub fn try_new(&self) -> bool {
        self.lock().try_new
    }

    /// Set whether the next sign-in tries the staged key first.
    pub fn set_try_new(&self, on: bool) {
        self.lock().try_new = on;
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
        inner.try_new = true;
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
}

/// [`Verdict`] of `st`'s chain for a device running with `current`, with
/// `staged` staged (if any). Only a renewal makes a device's keys differ
/// from what it holds (an enrollment with other keys is refused where it
/// completes, [`super::link`]), so other keys after one are somebody else's.
pub fn verdict(st: &AgentState, current: &MachineIdentity, staged: Option<&MachineIdentity>) -> Verdict {
    let Some(record) = st.chain.as_ref().and_then(|c| c.device_record(&st.machine)) else {
        return Verdict::Unchanged;
    };
    let named = (record.info.sign_key, record.info.dh_key);
    if named == (current.sign_public(), current.dh_public()) {
        return Verdict::Unchanged;
    }
    if staged.is_some_and(|s| named == (s.sign_public(), s.dh_public())) {
        return Verdict::Renewed;
    }
    match record.rotated_seq {
        Some(seq) => Verdict::Foreign {
            seq,
            fingerprint: rh_proto::identity::device_fingerprint_text(&named.0, &named.1),
        },
        None => Verdict::Unchanged,
    }
}

/// Why this device holds only a retired identity key (`link.rotation.reason`
/// of state `foreign`): a renewal on its own verified chain gave its id keys
/// it does not hold. Nothing else — a hub's word least of all — says so.
pub const FOREIGN_RENEWED_ELSEWHERE: &str = "renewed_elsewhere";
/// `link.rotation.reason` of state `refused`: the hub refuses the key in
/// use as `rotated`, and no chain it served shows a renewal (yet).
pub const REFUSED_ROTATED: &str = "rotated";

/// What this device says about holding only a retired key — in the alarm,
/// the logs and `agent status` — worded for the likeliest case too: this
/// computer's own backup from before a renewal.
pub const REPLACED_ELSEWHERE: &str = "This device's identity key was replaced elsewhere (for example, it was restored from a \
     backup made before a renewal, or another copy of its old key renewed it). To keep using it here, add it again under a \
     new identity.";

/// The staged seed as this device may use it: the same instance's, its keys
/// not the ones in use, and either younger than [`STAGED_FOR_MS`] or named
/// by the chain (a device off for days after a renewal still takes it). One
/// past its time is forgotten here.
pub(super) fn staged_seed(paths: &Paths, st: &AgentState, current: &MachineIdentity, now: u64) -> Option<StagedSeed> {
    let store = FileSeedStore::for_instance(paths);
    let staged = match store.staged() {
        Ok(Some(staged)) => staged,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(%error, "a staged identity key that does not read: ignoring it");
            return None;
        }
    };
    match usable(&store, &staged, st, current, now) {
        Ok(()) => Some(staged),
        Err(why) => {
            tracing::warn!(why, "forgetting the staged identity key");
            if let Err(error) = store.discard_staged() {
                tracing::error!(%error, "cannot forget the staged identity key");
            }
            None
        }
    }
}

fn usable(store: &FileSeedStore, staged: &StagedSeed, st: &AgentState, current: &MachineIdentity, now: u64) -> Result<(), &'static str> {
    let uuid = store.load().ok().flatten().map(|s| s.uuid);
    if uuid.as_deref() != Some(staged.uuid.as_str()) {
        return Err("it belongs to another instance");
    }
    let new = staged.identity();
    if new.sign_public() == current.sign_public() {
        return Err("it is the key in use");
    }
    let named = verdict(st, current, Some(&new)) == Verdict::Renewed;
    if !named && now.saturating_sub(staged.created_ms) > STAGED_FOR_MS {
        return Err("it is older than any renewal the hub would still take");
    }
    Ok(())
}

/// `POST /rh/rotate` (see the module docs). 200 `{signKey, dhKey, oldSig,
/// newSig, mac, stagedSeq, stagedHead, readyMs}`; 400 `bad_request`; 403
/// `forbidden`/`limited_device`; 412 `stale`; 409 `not_current_host`,
/// `not_on_chain`, `hub_not_ready`, `hub_refused {reason}`; 504
/// `hub_timeout` (the hub said nothing in [`STAGE_WAIT`]: the seed stays
/// staged, and a new request replaces it).
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
    let (Some(_), Some(text)) = (session, text) else {
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
    let existing = store.staged().ok().flatten().filter(|s| s.nonce == req.nonce && s.uuid == uuid);
    if let Some(s) = &existing {
        if (s.chain_seq, s.chain_head, s.assurance) != (req.chain_seq, req.chain_head, req.assurance) {
            return (
                400,
                json!({"error": "bad_request", "message": "this nonce came before with another chain view or assurance"}),
            );
        }
    }
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
    let taken = shared.rotation.lock().taken.as_ref().filter(|t| t.nonce == req.nonce).map(|t| t.ready_ms);
    if let Some(ready_ms) = taken {
        return (200, answer(ready_ms));
    }
    // On disk before the hub hears of it: a renewal the hub stages is one
    // this device can always finish.
    if let Err(error) = store.stage(&staged) {
        tracing::error!(%error, "cannot stage a new identity key");
        return (500, json!({"error": "unavailable", "message": "cannot store the new identity key"}));
    }
    shared.rotation.lock().taken = None;
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
            shared.rotation.lock().taken = Some(Taken {
                nonce: req.nonce,
                ready_ms,
            });
            if staged.assurance == Assurance::OnDevice {
                tracing::warn!(
                    code = %chain::rotate_sas(&body),
                    "an admin device renews this device's identity key and asks you to confirm it: type this code there (10 minutes)"
                );
            }
            (200, answer(ready_ms))
        }
        Ok(Ok(v)) => {
            let reason = v["reason"].as_str().unwrap_or("unknown").to_string();
            tracing::warn!(%reason, "the hub refused to stage the renewal; forgetting the staged key");
            if store.staged().ok().flatten().is_some_and(|s| s.nonce == req.nonce) {
                if let Err(error) = store.discard_staged() {
                    tracing::error!(%error, "cannot forget the staged identity key");
                }
            }
            (
                409,
                json!({"error": "hub_refused", "reason": reason, "message": "the hub refused to stage this renewal"}),
            )
        }
        Ok(Err(_)) | Err(_) => (
            504,
            json!({"error": "hub_timeout", "message": "the hub did not answer the staging in time: ask again"}),
        ),
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
        let st = shared.state.lock().expect("agent state");
        let staged = staged_seed(&shared.paths, &st, &shared.identity, now_ms())?;
        let new = staged.identity();
        let ours = new.sign_public() == asked
            && value["dh_key"].as_str().and_then(unhex32) == Some(new.dh_public())
            && value["nonce_hash"].as_str().and_then(unhex32) == Some(nonce_hash(&staged.nonce));
        ours.then(|| {
            let body = proof(&st.account, &st.machine, &staged, &shared.identity, &new);
            chain::sign_rotate_ack(new.signing_key(), &st.account, &st.machine, &body, &session.nonce)
        })
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
    let verdict = {
        let st = shared.state.lock().expect("agent state");
        let staged = staged_seed(&shared.paths, &st, &shared.identity, now_ms()).map(|s| s.identity());
        verdict(&st, &shared.identity, staged.as_ref())
    };
    match &verdict {
        Verdict::Unchanged => {}
        Verdict::Renewed => {
            let mut inner = shared.rotation.lock();
            inner.try_new = true;
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
fn foreign(shared: &Arc<Shared>, seq: u64, fingerprint: &str) {
    {
        let mut st = shared.state.lock().expect("agent state");
        st.rotation.foreign_ms.get_or_insert(now_ms());
        st.rotation.foreign_reason = Some(FOREIGN_RENEWED_ELSEWHERE.to_string());
        st.rotation.refused_ms = None;
    }
    tracing::warn!(
        alarm = "foreign_rotation",
        seq,
        %fingerprint,
        "the device chain gave this device's id keys it does not hold; the link stops"
    );
    super::link::raise_alarm(shared, REPLACED_ELSEWHERE.to_string());
    shared.save();
    shared.cancel.cancel();
}

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

/// Whether `identity` is a seed this device must never use again: one a
/// renewal retired on the chain it knows (`state.json`'s), or one it
/// archived ([`re_enroll_fresh`]).
pub fn seed_retired(paths: &Paths, identity: &MachineIdentity) -> bool {
    let on_chain = AgentState::load(paths)
        .ok()
        .flatten()
        .and_then(|st| st.chain)
        .is_some_and(|c| c.is_retired_key(&identity.sign_public()) || c.is_retired_key(&identity.dh_public()));
    on_chain || archived(paths, identity)
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
/// local settings' sign-in): the seed in use — or, when it is retired on
/// the chain this device knows or one it archived, a fresh one, the old
/// archived and its enrollment forgotten ([`re_enroll_fresh`]). A retired key
/// never joins again (the chain refuses it, `KEY_RETIRED`).
pub fn identity_for_enrolling(paths: &Paths) -> Result<MachineIdentity, String> {
    if let Some(stored) = FileSeedStore::for_instance(paths).load()? {
        let current = MachineIdentity::from_seed(&stored.seed);
        if seed_retired(paths, &current) {
            tracing::warn!("the identity key in use was retired by a renewal: this enrollment uses a fresh one");
            return re_enroll_fresh(paths, now_ms());
        }
    }
    super::load_or_create(paths).map(|(identity, _)| identity)
}

/// "Add this device again with a new identity" (§3.8 「只剩退役钥时」): for a
/// device left holding only a retired key, which no rule lets renew in
/// place. The link must be stopped first ([`super::control::Control::re_enroll_fresh`]).
///
/// The seed in use is archived as `machine-key.retired-<ms>.json` — kept,
/// never loaded again — and a staged one dropped; the old enrollment is
/// forgotten (`state.json`, the machine keys, the streams' store, the other
/// devices' keys); and a fresh seed is made: the instance is back at "enter
/// a registration code", and joins under a new id. Archived first, so a
/// crash anywhere never leaves the old seed in use without its enrollment.
pub fn re_enroll_fresh(paths: &Paths, now: u64) -> Result<MachineIdentity, String> {
    let current = super::key_path(paths);
    if current.exists() {
        let archived = archived_key_path(paths, now);
        std::fs::rename(&current, &archived).map_err(|e| format!("{} → {}: {e}", current.display(), archived.display()))?;
        super::sync_dir(&current);
        tracing::warn!(archived = %archived.display(), "archived this device's retired identity key; it is never used again");
    }
    FileSeedStore::for_instance(paths).discard_staged()?;
    super::state::forget_enrollment_leftovers(paths)?;
    match std::fs::remove_file(super::state::state_path(paths)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    crate::aggregator::forget_all(paths)?;
    let (identity, created) = super::load_or_create(paths)?;
    if !created {
        return Err("a seed was in place after the old one was archived".into());
    }
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

/// Take the staged seed (see the module docs): only once this device's own
/// verified chain names exactly its keys. Renamed into place atomically,
/// noted for the local settings, and the link restarted with it.
pub(super) fn promote(shared: &Arc<Shared>) -> Result<(), String> {
    let store = FileSeedStore::for_instance(&shared.paths);
    let staged = store.staged()?.ok_or("nothing staged")?;
    let new = staged.identity();
    {
        let st = shared.state.lock().expect("agent state");
        if verdict(&st, &shared.identity, Some(&new)) != Verdict::Renewed {
            return Err("the device chain does not name the staged identity key".into());
        }
    }
    let previous = super::control::machine_fingerprint(&shared.identity);
    store.promote()?;
    {
        let mut st = shared.state.lock().expect("agent state");
        st.rotation.renewed_ms = Some(now_ms());
        st.rotation.previous_fingerprint = Some(previous.clone());
        st.rotation.foreign_ms = None;
        st.rotation.foreign_reason = None;
    }
    shared.save();
    {
        let mut inner = shared.rotation.lock();
        inner.taken = None;
        inner.try_new = false;
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
    /// the link stopped, and it may be added again under a new identity).
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
    /// Why: `renewed_elsewhere` for `foreign`, `rotated` for `refused`.
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
    if st.rotation.refused_ms.is_some() {
        return Status {
            state: "refused".into(),
            reason: Some(REFUSED_ROTATED.into()),
            ..Status::default()
        };
    }
    let store = FileSeedStore::for_instance(paths);
    let current = store.load().ok().flatten().map(|s| MachineIdentity::from_seed(&s.seed));
    let staged = store.staged().ok().flatten();
    if let (Some(current), Some(staged)) = (current, staged) {
        if usable(&store, &staged, st, &current, now).is_ok() {
            let until = staged.created_ms.saturating_add(CODE_FOR_MS);
            let shown = staged.assurance == Assurance::OnDevice && now < until;
            let code = shown.then(|| chain::rotate_sas(&proof(&st.account, &st.machine, &staged, &current, &staged.identity())));
            return Status {
                state: "staged".into(),
                code_expires_ms: code.as_ref().map(|_| until),
                code,
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
        }
    }

    /// §3.8 「只剩退役钥时」 (contract §9): "add this device again with a new
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

    /// Contract §10: an archived (retired) seed is never loaded again — not
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
        let other = MachineIdentity::from_seed(&[66; 32]);
        let mut r = chain::HostRotation {
            host: "m-1".into(),
            sign_key: other.sign_public(),
            dh_key: other.dh_public(),
            staged_seq: hub.seq(),
            staged_head: hub.head(),
            nonce: [1; 16],
            assurance: Assurance::Auto,
            old_sig: [0; 64],
            new_sig: [0; 64],
        };
        let body = r.body("a-1", &current.sign_public(), &current.dh_public());
        r.old_sig = sig::sign(current.signing_key(), sig::HOST_ROTATE, &body);
        r.new_sig = sig::sign(other.signing_key(), sig::HOST_ROTATE, &body);
        push(&mut hub, chain::Action::RotateHostKey(r));
        push(&mut hub, epoch(1));
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", rh_proto::identity::fingerprint(&ak.sign_public()), 5);
        assert!(st.absorb(&entries, now_ms()).error.is_none());
        st.save(&paths).unwrap();
        assert!(seed_retired(&paths, &current), "retired on the chain it knows");
        let fresh = identity_for_enrolling(&paths).unwrap();
        assert_ne!(fresh.sign_public(), current.sign_public());
        assert!(archived(&paths, &current), "the retired seed is archived, kept");
        assert!(AgentState::load(&paths).unwrap().is_none(), "the old enrollment is forgotten");
        assert_eq!(identity_for_enrolling(&paths).unwrap().sign_public(), fresh.sign_public(), "and the fresh one stays");
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
        // …and is forgotten where the link reads it.
        assert!(staged_seed(&paths, &st, &current, now + STAGED_FOR_MS).is_none());
        assert!(store.staged().unwrap().is_none());
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
