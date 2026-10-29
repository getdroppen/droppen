//! Where this instance stands with its hub link, as the local API and
//! `agent status` say it: read from the agent's state and files.

use rh_proto::identity::display_fingerprint;
use serde::Serialize;

use super::state::{AgentState, Alarm};
use super::AgentFiles;

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
    /// elsewhere ([`AgentFiles::rotation_status`]).
    pub rotation: super::rotation::Status,
}

impl AgentStatus {
    /// The status of an instance with no agent state: not enrolled, nothing
    /// running.
    pub fn none() -> Self {
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
    /// files say ([`AgentFiles::rotation_status`]).
    pub fn of_instance(files: &AgentFiles, st: &AgentState) -> Self {
        Self {
            rotation: files.rotation_status(Some(st), droppen_base::time::now_ms_u64()),
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
