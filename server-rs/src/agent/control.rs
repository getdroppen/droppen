//! Starting and stopping the hub link while the server runs: [`Control`] is
//! the server's handle on its link.
//!
//! - at boot it starts the link when `agent/state.json` is there (an instance
//!   that never enrolled starts nothing);
//! - [`Control::enroll`] runs the same enrollment `agent enroll` does
//!   ([`super::enroll::begin`]) in this process and starts the link at once,
//!   which then follows the chain until a device adds the machine;
//! - the link stops itself when this machine is removed from the account
//!   ([`super::link`]), and [`Control::status`] says so;
//! - [`Control::unenroll`] stops it and deletes everything under `agent/`.
//!
//! Nothing here ever runs on a request path that is not about the link: the
//! local API's other routes never wait for the hub.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rh_proto::identity::{display_fingerprint, EnrollCode, MachineIdentity};
use serde::Serialize;

use super::enroll::BeginError;
use super::link::{Health, LinkEnv, Running};
use super::state::{AgentState, Alarm};
use crate::sessions::Paths;

/// Where this instance stands with a hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// Never enrolled (no `agent/state.json`): the single-machine server.
    None,
    /// The hub assigned a machine id; no device has added it yet.
    Enrolling,
    /// The device chain names this machine, and it runs AI in the account.
    Linked,
    /// The device chain names this machine, but it no longer runs AI in the
    /// account (chain v3 `SetHost false`): still a device of the account,
    /// with its state kept; it writes no streams and accepts no channels, and
    /// runs AI again once the chain makes it a host again.
    NotHosting,
    /// The account removed this machine; the link does not run.
    Removed,
    /// `agent/state.json` is there but does not load.
    Broken,
}

/// What the local API and `agent status` say about the link. Nothing secret.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    /// Where it stands.
    pub state: Standing,
    /// The hub's origin (`wss://…`).
    pub hub: Option<String>,
    /// The account id.
    pub account: Option<String>,
    /// The account key's fingerprint, as `agent enroll` printed it.
    pub account_fingerprint: Option<String>,
    /// This machine's id in the account.
    pub machine: Option<String>,
    /// Whether the link runs in this process.
    pub running: bool,
    /// Signed in to the hub right now.
    pub connected: bool,
    /// Since when (ms) it has been connected, or not.
    pub since_ms: Option<u64>,
    /// Why the hub connection last failed or ended.
    pub error: Option<String>,
    /// The hub refuses this computer's sign-in (retried; not a removal).
    pub refused: Option<String>,
    /// The verified chain's seq.
    pub chain_seq: Option<u64>,
    /// How many current devices the chain lists.
    pub devices: Option<usize>,
    /// How many current machines the chain lists.
    pub machines: Option<usize>,
    /// The newest machine-key epoch held.
    pub mk_epoch: Option<u32>,
    /// When the enrollment began.
    pub enrolled_ms: Option<u64>,
    /// When this machine learnt it was removed.
    pub removed_ms: Option<u64>,
    /// Chain conflicts this machine saw (newest last).
    pub alarms: Vec<Alarm>,
    /// Account changes this machine saw (newest last).
    pub changes: Vec<Alarm>,
    /// Renewing this machine's identity key: a staged key (and the code to
    /// type, for a high-assurance renewal), a renewal just taken, or one made
    /// elsewhere ([`super::rotation::status`]).
    pub rotation: super::rotation::Status,
}

impl AgentStatus {
    fn none() -> Self {
        Self {
            state: Standing::None,
            hub: None,
            account: None,
            account_fingerprint: None,
            machine: None,
            running: false,
            connected: false,
            since_ms: None,
            error: None,
            refused: None,
            chain_seq: None,
            devices: None,
            machines: None,
            mk_epoch: None,
            enrolled_ms: None,
            removed_ms: None,
            alarms: Vec::new(),
            changes: Vec::new(),
            rotation: super::rotation::Status {
                state: "idle".into(),
                ..Default::default()
            },
        }
    }

    /// [`AgentStatus::of`], with the identity renewal as this instance's
    /// files say ([`super::rotation::status`]).
    pub fn of_instance(paths: &Paths, st: &AgentState) -> Self {
        Self {
            rotation: super::rotation::status(paths, Some(st), super::now_ms()),
            ..Self::of(st)
        }
    }

    /// From a state, running or loaded from disk.
    pub fn of(st: &AgentState) -> Self {
        let state = if st.removed() {
            Standing::Removed
        } else if st.hosting() {
            Standing::Linked
        } else if st.enrolled() {
            Standing::NotHosting
        } else {
            Standing::Enrolling
        };
        let summary = st.summary();
        Self {
            state,
            hub: Some(st.hub.clone()),
            account: Some(st.account.clone()),
            account_fingerprint: Some(display_fingerprint(&st.root.account_fingerprint)),
            machine: Some(st.machine.clone()),
            chain_seq: summary.map(|s| s.0),
            devices: summary.map(|s| s.1),
            machines: summary.map(|s| s.2),
            mk_epoch: st.mk_epoch,
            enrolled_ms: Some(st.enrolled_ms()),
            removed_ms: st.removed_ms,
            alarms: st.alarms.clone(),
            changes: st.changes.clone(),
            ..Self::none()
        }
    }
}

/// Why [`Control`] could not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// This server cannot run a link: it does not own its instance
    /// (`export-openapi`, a test server), or `DROPPEN_AGENT_LINK=0`.
    Unavailable(String),
    /// The enrollment could not begin.
    Begin(BeginError),
    /// The hub could not be reached.
    Unreachable(String),
    /// Anything else, in words.
    Failed(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::Unavailable(m) | ControlError::Failed(m) | ControlError::Unreachable(m) => f.write_str(m),
            ControlError::Begin(e) => write!(f, "{e}"),
        }
    }
}

/// The server's handle on its hub link. One per server, in
/// [`crate::api::AppState`].
pub struct Control {
    /// Itself, for the link to ask for a restart once it took a renewed
    /// identity key ([`Control::restart`]).
    me: std::sync::Weak<Control>,
    paths: Paths,
    /// Set once the server owns its instance and listens
    /// ([`crate::app::RemoteHandApp::prepare`]); a server without it (no
    /// lock: `export-openapi`, tests) never starts a link.
    env: OnceLock<LinkEnv>,
    running: Mutex<Option<Running>>,
    /// Enrolling, starting, stopping and unenrolling one at a time.
    gate: tokio::sync::Mutex<()>,
    /// Bumped by every enrollment, so a stale watchdog does nothing.
    generation: Mutex<u64>,
    /// The other devices' copies in memory, for the local gateway:
    /// the copies task of whichever link runs publishes into it.
    live: Arc<crate::aggregator::live::LiveCopies>,
}

impl Control {
    /// The handle for one server.
    pub fn new(paths: Paths) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            paths,
            env: OnceLock::new(),
            running: Mutex::new(None),
            gate: tokio::sync::Mutex::new(()),
            generation: Mutex::new(0),
            live: crate::aggregator::live::LiveCopies::new(),
        })
    }

    /// What a link calls once it renewed its identity key: this control
    /// restarts it ([`Control::restart`]) on a task of its own — the link
    /// that asks is the one stopped.
    fn restart_hook(&self) -> super::rotation::Restart {
        let me = self.me.clone();
        Arc::new(move || {
            let Some(control) = me.upgrade() else { return };
            tokio::spawn(async move {
                if let Err(error) = control.restart().await {
                    tracing::error!(%error, "cannot restart the hub link with the renewed identity key");
                }
            });
        })
    }

    /// Stop the link and start it again from what is on disk — after an
    /// identity renewal, with the new key: its hub connection, relays,
    /// channels, this computer's device role and the streams' writer. The
    /// server, its AI sessions and terminals go on untouched (they never ran
    /// under the link).
    pub async fn restart(&self) -> Result<(), ControlError> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
        tracing::info!("restarting the hub link");
        self.start_locked().await
    }

    /// The other devices' copies as the gateway serves them.
    pub fn live(&self) -> &Arc<crate::aggregator::live::LiveCopies> {
        &self.live
    }

    /// Give the handle what a link needs (the owner, once listening). First,
    /// whatever deletion of the other devices' copies an earlier run left
    /// unfinished is finished — whether the aggregator is on or not
    /// ([`crate::aggregator::sweep_deleted`]).
    pub fn attach(&self, env: LinkEnv) {
        crate::aggregator::sweep_deleted(&self.paths);
        // Adding this device again under a new identity, cut short by a
        // crash: finished before anything starts.
        if let Err(error) = super::rotation::finish_re_enroll(&self.paths) {
            tracing::error!(%error, "cannot finish adding this device again under a new identity");
        }
        // A dispute statement the hub has not taken yet: delivered again
        // (it outlives the enrollment it disputes).
        super::dispute::deliver(&self.paths);
        let _ = self.env.set(env);
    }

    /// Whether this server can run a link at all.
    pub fn available(&self) -> Result<&LinkEnv, ControlError> {
        if !super::link::enabled() {
            return Err(ControlError::Unavailable("the hub link is turned off here (DROPPEN_AGENT_LINK=0)".into()));
        }
        self.env
            .get()
            .ok_or_else(|| ControlError::Unavailable("this server does not own its instance, so it runs no hub link".into()))
    }

    /// At boot: start the link if this instance is enrolled (or enrolling)
    /// and not removed. `false`, and nothing started, otherwise — the server
    /// is then exactly the single-machine server.
    pub async fn start_if_enrolled(&self) -> bool {
        if self.available().is_err() || !self.paths.has_agent_state() {
            return false;
        }
        let _gate = self.gate.lock().await;
        match self.start_locked().await {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(%error, "not connecting to the hub");
                false
            }
        }
    }

    async fn start_locked(&self) -> Result<(), ControlError> {
        let env = self.available()?;
        let old = {
            let mut running = self.running.lock().expect("running");
            if running.as_ref().is_some_and(|r| !r.finished()) {
                return Ok(());
            }
            running.take()
        };
        // A link that ended on its own (a removal, other keys) is stopped
        // before another starts: its device role, or its streams' writer, may
        // still be going.
        if let Some(old) = old {
            old.stop().await;
        }
        // An install still enrolled at the legacy apex hub moves to the
        // canonical `api.` origin once, if the hub takes its sign-in
        // ([`super::migrate_hub`]).
        super::migrate_hub(&self.paths).await.map_err(ControlError::Failed)?;
        let started = super::link::start(&self.paths, env, self.live.clone(), self.restart_hook()).map_err(ControlError::Failed)?;
        *self.running.lock().expect("running") = Some(started);
        Ok(())
    }

    /// Stop the link, if it runs, and wait for it.
    pub async fn stop(&self) {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
    }

    async fn stop_locked(&self) {
        let running = self.running.lock().expect("running").take();
        if let Some(running) = running {
            running.stop().await;
        }
        // Whatever the copies task left in memory goes with it (it clears
        // it itself when it ends, unless it had to be cut short).
        self.live.clear();
    }

    /// Where the link stands, without waiting for anything but the local
    /// disk: the running link's own view, or the saved state.
    pub fn status(&self) -> AgentStatus {
        let running = self.running.lock().expect("running");
        if let Some(run) = running.as_ref() {
            let mut status = run.with_state(|st| AgentStatus::of_instance(&self.paths, st));
            let finished = run.finished();
            let Health {
                connected,
                since_ms,
                error,
                refused,
            } = run.health();
            status.running = !finished;
            status.connected = connected && !finished;
            status.since_ms = (since_ms > 0).then_some(since_ms);
            status.error = error;
            status.refused = refused;
            return status;
        }
        drop(running);
        match AgentState::load(&self.paths) {
            Ok(Some(st)) => AgentStatus::of_instance(&self.paths, &st),
            Ok(None) => AgentStatus::none(),
            Err(error) => AgentStatus {
                state: Standing::Broken,
                error: Some(error),
                ..AgentStatus::none()
            },
        }
    }

    /// What this computer's device role reports: `None` when the
    /// link does not run here.
    pub fn control_status(&self) -> Option<crate::aggregator::controller::Status> {
        let running = self.running.lock().expect("running");
        running.as_ref().filter(|r| !r.finished()).map(Running::control_status)
    }

    /// This instance's machine identity (made on first use, kept in
    /// `agent/machine-key.json`): what a device checks an enrollment against.
    pub fn identity(&self) -> Result<MachineIdentity, String> {
        super::load_or_create(&self.paths).map(|(identity, _)| identity)
    }

    /// Enroll in this process: present `code` at `hub_url`, prove it, save the
    /// enrolling state and start the link, which completes the enrollment as
    /// soon as a device adds this machine — no restart. The account and the
    /// machine id the hub assigned. If no device adds it within the code's
    /// lifetime, the link stops and the enrolling state is forgotten (the
    /// machine key stays, for the next try).
    pub async fn enroll(self: &Arc<Self>, code: &EnrollCode, hub_url: &str) -> Result<(String, String), ControlError> {
        self.available()?;
        let _gate = self.gate.lock().await;
        // Whatever ran (an enrollment that went nowhere) makes way for this
        // one — and comes back if this one cannot begin (the hub unreachable,
        // the code refused): a failed sign-in never leaves the link stopped.
        let was_running = self.running.lock().expect("running").as_ref().is_some_and(|r| !r.finished());
        self.stop_locked().await;
        // A seed a renewal retired (or one archived) never signs up again:
        // a fresh one then (`docs/host-identity-rotation.md` §3.8).
        let begun = match super::rotation::identity_for_enrolling(&self.paths) {
            Ok(identity) => super::enroll::begin(&self.paths, &identity, code, hub_url)
                .await
                .map_err(ControlError::Begin),
            Err(error) => Err(ControlError::Failed(error)),
        };
        let begun = match begun {
            Ok(begun) => begun,
            Err(error) => {
                if was_running {
                    if let Err(restart) = self.start_locked().await {
                        tracing::error!(%restart, "cannot restart the hub link after a failed sign-in");
                    }
                }
                return Err(error);
            }
        };
        let (account, machine) = (begun.account.clone(), begun.machine.clone());
        tracing::info!(hub = %begun.origin, %account, %machine, "enrolled in process; starting the hub link to finish");
        // The link signs in again on its own socket and follows the chain.
        drop(begun);
        self.start_locked().await?;
        let generation = {
            let mut g = self.generation.lock().expect("generation");
            *g += 1;
            *g
        };
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(super::enroll::CONFIRM_TIMEOUT + Duration::from_secs(5)).await;
            this.expire_enrollment(generation).await;
        });
        Ok((account, machine))
    }

    /// The code's lifetime is over: an enrollment still waiting for a device
    /// is forgotten (unless another one began since).
    async fn expire_enrollment(&self, generation: u64) {
        let _gate = self.gate.lock().await;
        if *self.generation.lock().expect("generation") != generation {
            return;
        }
        let waiting = match AgentState::load(&self.paths) {
            Ok(Some(st)) => waiting_for_a_device(&st),
            _ => false,
        };
        if !waiting {
            return;
        }
        tracing::warn!("no device added this machine within the enrollment code's lifetime; forgetting the enrollment");
        self.stop_locked().await;
        if let Err(error) = forget_enrollment(&self.paths) {
            tracing::error!(%error, "cannot forget the expired enrollment");
        }
    }

    /// Forget an enrollment no device has completed (cancelled from the
    /// local settings): stop the link and delete the enrolling state; the
    /// machine key stays. Does nothing to an enrolled (running AI or not) or
    /// removed state.
    pub async fn cancel_enrollment(&self) -> Result<bool, ControlError> {
        let _gate = self.gate.lock().await;
        let waiting = match AgentState::load(&self.paths) {
            Ok(Some(st)) => waiting_for_a_device(&st),
            Ok(None) => return Ok(false),
            Err(error) => return Err(ControlError::Failed(error)),
        };
        if !waiting {
            return Ok(false);
        }
        self.stop_locked().await;
        forget_enrollment(&self.paths).map_err(ControlError::Failed)?;
        Ok(true)
    }

    /// Delete this machine's encrypted copy on the hub (every stream it
    /// holds for the machine), on a connection of its own: the link is
    /// stopped first, so the hub has one connection for the machine. The
    /// number of streams deleted — none for a machine that does not run AI
    /// in the account now (the hub signs in hosts only; an admin device
    /// deletes such a copy).
    pub async fn delete_copies(&self) -> Result<usize, ControlError> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
        let st = match AgentState::load(&self.paths) {
            Ok(Some(st)) if st.hosting() => st,
            Ok(_) => return Ok(0),
            Err(error) => return Err(ControlError::Failed(error)),
        };
        let identity = self.identity().map_err(ControlError::Failed)?;
        tokio::time::timeout(Duration::from_secs(30), delete_streams(&st, &identity))
            .await
            .map_err(|_| ControlError::Unreachable("the hub did not answer in time".into()))?
    }

    /// Before a sign-in shows this computer's keys: when the seed in use is
    /// one it must never use again — archived, or replaced elsewhere on the
    /// chain it knows ([`super::rotation::seed_retired`]: never its own
    /// renewal to a key it staged) — it never signs up again:
    /// the link stops, the seed is archived, the old enrollment forgotten and
    /// a fresh seed made ([`Control::re_enroll_fresh`]). `true` when that happened.
    pub async fn renew_retired_seed(&self) -> Result<bool, ControlError> {
        let _gate = self.gate.lock().await;
        let current = match super::FileSeedStore::for_instance(&self.paths).load() {
            Ok(Some(seed)) => MachineIdentity::from_seed(&seed.seed),
            Ok(None) => return Ok(false),
            Err(error) => return Err(ControlError::Failed(error)),
        };
        if !super::rotation::seed_retired(&self.paths, &current) {
            return Ok(false);
        }
        self.stop_locked().await;
        super::rotation::re_enroll_fresh(&self.paths, super::now_ms()).map_err(ControlError::Failed)?;
        Ok(true)
    }

    /// "Add this device again with a new identity": for a device left holding
    /// only a retired identity key (`link.rotation.state` `foreign`), stop
    /// the link, archive the seed, forget the enrollment and make a fresh
    /// seed ([`super::rotation::re_enroll_fresh`]). The new identity; the
    /// instance is then not enrolled, waiting for a registration code.
    pub async fn re_enroll_fresh(&self) -> Result<MachineIdentity, ControlError> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
        super::rotation::re_enroll_fresh(&self.paths, super::now_ms()).map_err(ControlError::Failed)
    }

    /// Stop the link and delete everything under `agent/`: the state, the
    /// machine keys, the streams' store and the machine key itself. The
    /// instance is the single-machine server again.
    pub async fn unenroll(&self) -> Result<(), ControlError> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await;
        unenroll_files(&self.paths).map_err(ControlError::Failed)
    }
}

/// The label [`machine_fingerprint`] hashes under (the web's
/// `machineFingerprint` in `web/src/features/link/handoff.ts` uses the same).
pub const MACHINE_FINGERPRINT_LABEL: &[u8] = b"rh/v1 machine fingerprint\0";

/// The fingerprint of this machine's keys as the local settings and the
/// droppen.app consent page both show it: the first 16 bytes of
/// SHA-256(label ‖ signing key ‖ channel key), in 8 groups of 4 hex digits.
pub fn machine_fingerprint(identity: &MachineIdentity) -> String {
    machine_fingerprint_of(&identity.sign_public(), &identity.dh_public())
}

/// [`machine_fingerprint`] of two public keys.
pub fn machine_fingerprint_of(sign: &[u8; 32], dh: &[u8; 32]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(MACHINE_FINGERPRINT_LABEL);
    h.update(sign);
    h.update(dh);
    let digest = h.finalize();
    digest[..16]
        .chunks(2)
        .map(|pair| pair.iter().map(|b| format!("{b:02x}")).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sign in as the machine, ask for the streams the hub holds for it, delete
/// each, and ask again until none is left.
async fn delete_streams(st: &AgentState, identity: &MachineIdentity) -> Result<usize, ControlError> {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    let hub = super::hub::origin(&st.hub).map_err(ControlError::Failed)?;
    let (mut sink, mut source, nonce) = super::hub::dial(&hub).await.map_err(ControlError::Unreachable)?;
    super::hub::login(&mut sink, &mut source, identity, &st.account, &st.machine, &hub, nonce)
        .await
        .map_err(|e| ControlError::Unreachable(e.to_string()))?;
    let mut deleted = 0;
    for _round in 0..3 {
        super::hub::send_json(&mut sink, &serde_json::json!({"t": "stream_list"}))
            .await
            .map_err(ControlError::Unreachable)?;
        let streams: Vec<String> = loop {
            let message = tokio::time::timeout(Duration::from_secs(15), source.next())
                .await
                .map_err(|_| ControlError::Unreachable("the hub did not list this device's streams".into()))?;
            let Some(Ok(message)) = message else {
                return Err(ControlError::Unreachable("the hub closed the connection".into()));
            };
            let Message::Text(text) = message else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(text.as_str()) else {
                continue;
            };
            if v["t"] == "stream_list" {
                break v["streams"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["stream"].as_str().map(str::to_string))
                    .collect();
            }
        };
        if streams.is_empty() {
            break;
        }
        for stream in &streams {
            super::hub::send_json(&mut sink, &serde_json::json!({"t": "stream_delete", "stream": stream}))
                .await
                .map_err(ControlError::Unreachable)?;
        }
        deleted += streams.len();
    }
    tracing::info!(deleted, "deleted this machine's encrypted copy on the hub");
    Ok(deleted)
}

/// Whether `st` is an enrollment still waiting for a device to add this
/// machine: the chain does not name it (yet), and has not removed it. Never
/// a member that merely stopped running AI: counting one as waiting would let the
/// code's expiry, or cancel, delete its state.
fn waiting_for_a_device(st: &AgentState) -> bool {
    !st.enrolled() && !st.removed()
}

/// Delete an enrollment's state (and what it left), keeping the machine key.
fn forget_enrollment(paths: &Paths) -> Result<(), String> {
    paths.forget_enrollment_leftovers()?;
    match std::fs::remove_file(paths.agent_state_path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Delete `agent/` and everything in it (no-op when it is not there) — but
/// a dispute statement the hub has not taken yet ([`super::dispute`]): the
/// only evidence that this device's key was replaced elsewhere, signed and
/// holding no secret, kept and still delivered until the hub has it. The
/// link — and with it the streams' writer — must be stopped first; the
/// process's connection to `streams.db` is dropped here, before the file.
pub fn unenroll_files(paths: &Paths) -> Result<(), String> {
    crate::db::forget(&paths.streams_db_path());
    let dir = paths.instance_dir().join("agent");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut kept = 0usize;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let is_dir = entry.file_type().map_err(|e| format!("{}: {e}", path.display()))?.is_dir();
        if !is_dir && super::dispute::is_pending_statement(&path) {
            kept += 1;
            continue;
        }
        let removed = if is_dir { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        match removed {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    if kept == 0 {
        match std::fs::remove_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
        tracing::info!(dir = %dir.display(), "unenrolled: the agent's files are gone");
    } else {
        super::sync_dir(&dir.join("."));
        tracing::warn!(dir = %dir.display(), kept, "unenrolled: the agent's files are gone but the dispute statements the hub does not have yet");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_fingerprint_is_eight_groups_of_four_hex_digits() {
        let identity = MachineIdentity::from_seed(&[3; 32]);
        let fp = machine_fingerprint(&identity);
        assert_eq!(fp.split(' ').count(), 8);
        assert!(fp.split(' ').all(|g| g.len() == 4 && g.bytes().all(|b| b.is_ascii_hexdigit())));
        assert_eq!(fp, machine_fingerprint(&MachineIdentity::from_seed(&[3; 32])), "stable");
        assert_ne!(fp, machine_fingerprint(&MachineIdentity::from_seed(&[4; 32])));
        // The vector the web's `machineFingerprint` is tested against.
        assert_eq!(machine_fingerprint_of(&[1; 32], &[2; 32]), MACHINE_FINGERPRINT_VECTOR);
    }

    /// `machine_fingerprint_of(&[1; 32], &[2; 32])`, also in `web/test/link-handoff.test.ts`.
    const MACHINE_FINGERPRINT_VECTOR: &str = "d4c7 841d 009c f90b 2395 fcd4 4a5a 52fd";

    #[tokio::test]
    async fn a_server_that_does_not_own_its_instance_runs_no_link() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctltest");
        let control = Control::new(paths.clone());
        assert!(matches!(control.available(), Err(ControlError::Unavailable(_))));
        assert!(!control.start_if_enrolled().await);
        assert_eq!(control.status().state, Standing::None);
        let code = EnrollCode {
            account_fingerprint: [1; 16],
            secret: [2; 16],
        };
        assert!(matches!(
            control.enroll(&code, "ws://127.0.0.1:9").await,
            Err(ControlError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn status_reads_the_saved_state_and_unenroll_leaves_nothing_under_agent() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctltest");
        let control = Control::new(paths.clone());
        let identity = control.identity().unwrap();
        let mut st = AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5_000);
        st.save(&paths).unwrap();
        paths.store_machine_key(0, &[7; 32]).unwrap();
        let status = control.status();
        assert_eq!(status.state, Standing::Enrolling);
        assert_eq!(status.machine.as_deref(), Some("m-1"));
        assert!(!status.running);
        st.mark_removed(6_000);
        st.save(&paths).unwrap();
        assert_eq!(
            control.status().state,
            Standing::Enrolling,
            "a removal mark the chain does not back is no removal"
        );
        control.unenroll().await.unwrap();
        assert!(!paths.instance_dir().join("agent").exists(), "agent/ is gone");
        assert_eq!(control.status().state, Standing::None);
        assert_ne!(
            control.identity().unwrap().sign_public(),
            identity.sign_public(),
            "a new machine key after unenroll"
        );
    }

    /// The process keeps one connection per database file: after unenroll
    /// deletes `streams.db`, the next enrollment's store would be the old one
    /// — its rows (re-sent to the new account) in an unlinked file, every
    /// write lost on restart. The cached connection goes with the file.
    #[tokio::test]
    async fn a_new_enrollment_never_reopens_the_old_streams_store() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlstore");
        let control = Control::new(paths.clone());
        let _ = control.identity().unwrap();
        let old = crate::agent::streams::store::Store::open(&paths);
        old.set_kv("plan", "old-account-plan");
        drop(old);
        control.unenroll().await.unwrap();
        let file = paths.streams_db_path();
        assert!(!file.exists(), "agent/ is gone on disk");
        let _ = control.identity().unwrap(); // a new machine key, as a new sign-in makes
        let fresh = crate::agent::streams::store::Store::open(&paths);
        assert_eq!(fresh.get_kv("plan"), None, "the new enrollment starts from an empty store");
        fresh.set_kv("plan", "new-account-plan");
        assert!(file.exists(), "and it is the file on disk");
        drop(fresh);

        // The same for an enrollment that replaces one that never completed.
        paths.forget_enrollment_leftovers().unwrap();
        assert_eq!(crate::agent::streams::store::Store::open(&paths).get_kv("plan"), None);
    }

    /// What a link needs, for a server of these paths that listens nowhere.
    fn link_env(paths: &Paths) -> LinkEnv {
        let instance = crate::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).unwrap();
        let auth = crate::auth::Auth::new(paths, &instance, &crate::config::AuthSettings::default(), false);
        LinkEnv {
            addr: "127.0.0.1:9".parse().unwrap(),
            credential: "test".into(),
            drive: crate::drive::Drive::new(std::sync::Arc::new(paths.clone()), &instance, auth),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// A port nothing listens on.
    fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    /// Re-review of S1-5, M1: a deletion of the other devices' copies the
    /// process did not finish (it stopped right after turning the aggregator
    /// off) is finished at the next start, with the aggregator off.
    #[tokio::test]
    async fn an_unfinished_deletion_of_the_copies_is_finished_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlsweep");
        std::fs::create_dir_all(paths.instance_dir()).unwrap();
        std::fs::write(crate::api::cloud::file_path(&paths), br#"{"aggregatorOff": true}"#).unwrap();
        assert!(!crate::aggregator::wanted(&paths));
        let left = paths.instance_dir().join(".aggregator-deleted-1-2-3");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::write(left.join("copies.db"), b"STARTUP-SWEEP-CANARY").unwrap();
        let mut held = std::fs::File::open(left.join("copies.db")).unwrap();
        let control = Control::new(paths.clone());
        control.attach(link_env(&paths));
        crate::aggregator::finish_deletions(Duration::from_secs(5)).await;
        assert!(!left.exists(), "swept at start");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut held, &mut bytes).unwrap();
        assert!(bytes.iter().all(|b| *b == 0), "and zeroed before it went");
    }

    /// A sign-in that cannot begin (the hub unreachable, or the code
    /// refused) leaves the link that was running as it was: a failed sign-in
    /// never leaves it stopped.
    #[tokio::test]
    async fn a_sign_in_that_cannot_begin_leaves_the_link_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlrestore");
        let control = Control::new(paths.clone());
        let _ = control.identity().unwrap();
        let hub = format!("ws://127.0.0.1:{}", closed_port());
        AgentState::enrolling(&hub, "a-1", "m-1", [9; 16], 5_000).save(&paths).unwrap();
        control.attach(link_env(&paths));
        assert!(control.start_if_enrolled().await);
        assert!(control.status().running);
        let code = EnrollCode {
            account_fingerprint: [1; 16],
            secret: [2; 16],
        };
        let failed = control.enroll(&code, &hub).await;
        assert!(matches!(failed, Err(ControlError::Begin(BeginError::Unreachable(_)))), "{failed:?}");
        assert!(control.status().running, "the link that ran before runs again");
        assert_eq!(control.status().machine.as_deref(), Some("m-1"), "the same enrollment");
        control.stop().await;
    }

    /// A member that stopped running AI (`SetHost false`) is its own standing,
    /// not an enrollment still waiting for a device: the settings do not say
    /// "enrolling", and neither cancel nor the code's expiry deletes its
    /// `state.json`.
    #[tokio::test]
    async fn a_member_that_stopped_running_ai_keeps_its_state() {
        use rh_proto::chain::{genesis_draft, Action, ChainClock, ChainState, DeviceInfo, Signer};
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlhost");
        let control = Control::new(paths.clone());
        let machine = control.identity().unwrap();
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let device = MachineIdentity::from_seed(&[2; 32]);
        let info = DeviceInfo {
            id: "d-1".into(),
            sign_key: device.sign_public(),
            dh_key: device.dh_public(),
            full: true,
        };
        let genesis = genesis_draft("a-1", ak.sign_public(), info, None, 1_000).sign(ak.signing_key());
        let mut chain = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let mut entries = vec![genesis];
        let stop = Action::SetHost {
            id: "m-1".into(),
            host: false,
        };
        for action in [Action::AddMachine(machine.info("m-1")), stop] {
            let e = chain.draft(Signer::Device("d-1".into()), action, 2_000).sign(device.signing_key());
            chain.apply(&e, &mut ChainClock::history()).unwrap();
            entries.push(e);
        }
        let mut st = AgentState::enrolling(
            "wss://hub.example",
            "a-1",
            "m-1",
            rh_proto::identity::fingerprint(&ak.sign_public()),
            5_000,
        );
        assert!(st.absorb(&entries, 5_000).error.is_none());
        assert!(st.enrolled() && st.not_hosting());
        st.save(&paths).unwrap();
        let state_file = paths.agent_state_path();
        assert_eq!(control.status().state, Standing::NotHosting, "not \"enrolling\"");
        assert!(!control.cancel_enrollment().await.unwrap(), "nothing to cancel");
        assert!(state_file.exists());
        control.expire_enrollment(0).await;
        assert!(state_file.exists(), "the code's expiry leaves a member alone");
        assert_eq!(control.delete_copies().await.unwrap(), 0, "no copy to delete while it runs no AI");
        assert_eq!(control.status().state, Standing::NotHosting);
    }

    /// Host identity renewal (§3.5): once a renewed key is in place the link
    /// is restarted in this process — through the hook a link calls, on a
    /// task of its own — and runs with the new key; the server's token (what
    /// its AI sessions and terminals live under) is never touched.
    #[tokio::test]
    async fn a_restart_runs_the_link_with_the_renewed_key_and_leaves_the_server_alone() {
        use crate::agent::{FileSeedStore, StagedSeed};
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlrestart");
        let control = Control::new(paths.clone());
        let old = control.identity().unwrap();
        let hub = format!("ws://127.0.0.1:{}", closed_port());
        AgentState::enrolling(&hub, "a-1", "m-1", [9; 16], 5_000).save(&paths).unwrap();
        let env = link_env(&paths);
        let server = env.cancel.clone();
        let session = server.child_token();
        control.attach(env);
        assert!(control.start_if_enrolled().await);
        let key_in_use = |control: &Control| {
            let running = control.running.lock().unwrap();
            running.as_ref().filter(|r| !r.finished()).map(|r| r.shared.identity.sign_public())
        };
        assert_eq!(key_in_use(&control), Some(old.sign_public()));
        // The renewed seed in place, as a promotion leaves it.
        let store = FileSeedStore::for_instance(&paths);
        store
            .stage(&StagedSeed {
                uuid: crate::identity::ensure(&paths).unwrap().uuid,
                seed: zeroize::Zeroizing::new([9; 32]),
                nonce: [1; 16],
                chain_seq: 0,
                chain_head: [0; 32],
                assurance: rh_proto::chain::Assurance::Auto,
                created_ms: 1,
                accepted_ms: Some(2),
            })
            .unwrap();
        store.promote(&[1; 16]).unwrap();
        let new = MachineIdentity::from_seed(&[9; 32]);
        // The promotion noted that the push keys follow; a
        // device subscribed against the old VAPID key.
        let old_vapid = crate::push::store::vapid_public(&paths).await.unwrap();
        let point = {
            use p256::elliptic_curve::sec1::ToEncodedPoint as _;
            let device = p256::SecretKey::random(&mut rand::rngs::OsRng);
            crate::push::ece::b64url(device.public_key().to_encoded_point(false).as_bytes())
        };
        crate::push::store::upsert(&paths, "https://push.example/a".into(), point, crate::push::ece::b64url(&[9; 16]), None)
            .await
            .unwrap();
        {
            let running = control.running.lock().unwrap();
            let shared = &running.as_ref().unwrap().shared;
            shared.state.lock().unwrap().rotation.push_renew = true;
            shared.save();
        }
        (control.restart_hook())();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while key_in_use(&control) != Some(new.sign_public()) {
            assert!(std::time::Instant::now() < deadline, "the link restarts with the new key");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while AgentState::load(&paths).unwrap().unwrap().rotation.push_renew {
            assert!(std::time::Instant::now() < deadline, "the restarted link renews the push keys");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(crate::push::store::subscriptions(&paths).await.is_empty(), "every push subscription forgotten");
        assert_ne!(crate::push::store::vapid_public(&paths).await.unwrap(), old_vapid, "a new VAPID keypair");
        assert!(control.status().running);
        assert!(!server.is_cancelled() && !session.is_cancelled(), "the server and what runs under it go on");
        control.restart().await.unwrap();
        assert_eq!(key_in_use(&control), Some(new.sign_public()));
        control.stop().await;
        assert!(!server.is_cancelled() && !session.is_cancelled());
    }

    /// "Add this device again" cut short by a crash after
    /// the seed was archived (its `state.json` still there) is finished when
    /// the control comes up: the instance is not enrolled, with a fresh seed,
    /// and the link does not start as the old enrollment.
    #[tokio::test]
    async fn adding_again_cut_short_by_a_crash_is_finished_when_the_control_comes_up() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctlfresh");
        let control = Control::new(paths.clone());
        let old = control.identity().unwrap();
        AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5_000).save(&paths).unwrap();
        // What the crash left: the marker, the seed archived, the state kept.
        let agent = paths.machine_key_path().parent().unwrap().to_path_buf();
        crate::agent::write_private(&agent.join("re-enroll-fresh.json"), b"{}").unwrap();
        std::fs::rename(paths.machine_key_path(), super::super::rotation::archived_key_path(&paths, 5)).unwrap();
        control.attach(link_env(&paths));
        assert!(!paths.has_agent_state(), "the old enrollment is forgotten");
        assert!(!agent.join("re-enroll-fresh.json").exists());
        let fresh = control.identity().unwrap();
        assert_ne!(fresh.sign_public(), old.sign_public());
        assert!(super::super::rotation::archived(&paths, &old));
        assert!(!control.start_if_enrolled().await, "nothing to start");
        assert_eq!(control.status().state, Standing::None);
    }

    #[tokio::test]
    async fn cancelling_an_enrollment_keeps_the_machine_key() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "ctltest");
        let control = Control::new(paths.clone());
        let identity = control.identity().unwrap();
        AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5_000)
            .save(&paths)
            .unwrap();
        assert!(control.cancel_enrollment().await.unwrap());
        assert_eq!(control.status().state, Standing::None);
        assert_eq!(control.identity().unwrap().sign_public(), identity.sign_public(), "the machine key stays");
    }
}
