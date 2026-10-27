//! Joining this instance to an account (`docs/cross-machine-architecture.md`
//! §6): `agent enroll <code> --hub <url>` on the command line, and the local
//! settings' "Sign in to Droppen" in process
//! ([`super::control::Control::enroll`]) — the same [`begin`], then either
//! [`wait_confirmed`] here or the link.
//!
//! 1. Dial the hub and present the code's *token* — `HMAC(secret, …)`, never
//!    the secret. The hub assigns this machine an id in the account the token
//!    was registered to.
//! 2. Prove possession of the code: `HMAC(secret, account | machine id | both
//!    public keys | token)`, alongside a `HubLogin` signature showing this
//!    process holds the signing key. The device that issued the code checks
//!    the proof before it signs "add machine" with exactly these keys, so a
//!    hub cannot enroll a machine of its own.
//! 3. Follow the account's chain until that entry arrives, verifying it from
//!    genesis against the fingerprint in the code — then pin genesis itself.
//!
//! State is saved as soon as the hub has assigned the id, so an interrupted
//! enrollment completes on the next `serve`/`agent run` — and an in-process
//! one is completed by the link, which follows the chain the same way.

use std::time::Duration;

use futures_util::StreamExt;
use rh_proto::chain::{Attestation, Entry};
use rh_proto::frame::Frame;
use rh_proto::identity::{display_fingerprint, EnrollCode, MachineIdentity};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

use super::hub::{self, hex, now_ms};
use super::state::AgentState;
use crate::sessions::Paths;

/// How long to wait for the device to confirm (the code's own lifetime).
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Why an enrollment could not begin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginError {
    /// This instance is enrolled already.
    Enrolled(String),
    /// The hub could not be reached (or went away mid-way).
    Unreachable(String),
    /// The hub refused the code or the proof.
    Refused(String),
    /// This machine's own files, or a hub address that is not one.
    Local(String),
}

impl std::fmt::Display for BeginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BeginError::Enrolled(m) | BeginError::Unreachable(m) | BeginError::Refused(m) | BeginError::Local(m) => f.write_str(m),
        }
    }
}

/// An enrollment the hub has taken: the machine id it assigned, and the
/// socket it was taken on (signed in as that machine). The enrolling state is
/// saved already, so whoever follows the chain from here — [`wait_confirmed`]
/// on this socket, or the link ([`super::link::start`]) on its own — completes it.
pub struct Begun {
    /// The hub's origin, as the machine signs in to it.
    pub origin: String,
    /// The account the code belongs to.
    pub account: String,
    /// The machine id the hub assigned.
    pub machine: String,
    sink: hub::Sink,
    source: hub::Source,
}

/// Steps 1 and 2 (see the module docs): present the code's token, prove the
/// code and the keys, and save the enrolling state. Fast — a few round trips
/// — and the same for the command line and the local API.
pub async fn begin(paths: &Paths, identity: &MachineIdentity, code: &EnrollCode, hub_url: &str) -> Result<Begun, BeginError> {
    if let Some(existing) = AgentState::load(paths).map_err(BeginError::Local)? {
        if existing.enrolled() {
            return Err(BeginError::Enrolled(format!(
                "this instance is already enrolled as {} in account {} at {}.\n\
                 To enroll it again, stop its server and run `remote-hand-server agent unenroll` (a new device id is minted).",
                existing.machine, existing.account, existing.hub
            )));
        }
    }
    let origin = hub::origin(hub_url).map_err(BeginError::Local)?;
    let (mut sink, mut source, nonce) = hub::dial(&origin).await.map_err(BeginError::Unreachable)?;
    hub::send_json(&mut sink, &json!({"t": "enroll", "token": hex(&code.token())}))
        .await
        .map_err(BeginError::Unreachable)?;
    let assigned = hub::next_json(&mut source).await.map_err(BeginError::Unreachable)?;
    if assigned["t"] != "enroll_assigned" {
        return Err(BeginError::Refused(format!(
            "the hub refused the enrollment code: {}",
            assigned["message"]
        )));
    }
    let account = assigned["account"]
        .as_str()
        .ok_or_else(|| BeginError::Refused("no account in the hub's answer".into()))?
        .to_string();
    let machine = assigned["machine"]
        .as_str()
        .ok_or_else(|| BeginError::Refused("no device id in the hub's answer".into()))?
        .to_string();
    let time_ms = now_ms();
    let sig = hub::login_signature(identity, &account, &machine, &origin, nonce, time_ms).map_err(BeginError::Local)?;
    let proof = code.proof(&account, &machine, &identity.sign_public(), &identity.dh_public());
    hub::send_json(
        &mut sink,
        &json!({
            "t": "enroll_proof",
            "sign_key": hex(&identity.sign_public()),
            "dh_key": hex(&identity.dh_public()),
            "time_ms": time_ms,
            "sig": hex(&sig),
            "proof": hex(&proof),
        }),
    )
    .await
    .map_err(BeginError::Unreachable)?;
    let welcome = hub::next_json(&mut source).await.map_err(BeginError::Unreachable)?;
    if welcome["t"] != "welcome" {
        return Err(BeginError::Refused(format!(
            "the hub refused the enrollment proof: {}",
            welcome["message"]
        )));
    }
    let state = AgentState::enrolling(&origin, &account, &machine, code.account_fingerprint, now_ms());
    // A new machine id with new keys: what an earlier enrollment of this
    // instance left (its machine keys, its streams' store) is not this one's.
    super::state::forget_enrollment_leftovers(paths).map_err(BeginError::Local)?;
    state.save(paths).map_err(BeginError::Local)?;
    Ok(Begun {
        origin,
        account,
        machine,
        sink,
        source,
    })
}

/// How [`enroll`] ended without the device being added, as the command line
/// tells the two apart.
#[derive(Debug)]
pub enum EnrollFailure {
    /// The hub took the code and gave this device an id, and the device is
    /// not added yet: the enrolling state is saved, and `serve` finishes the
    /// enrollment once the droppen.app page that showed the code adds it.
    Pending(String),
    /// Nothing that will finish by itself: the code was refused or expired,
    /// the hub could not be reached, the chain did not verify.
    Failed(String),
}

impl From<String> for EnrollFailure {
    fn from(message: String) -> Self {
        EnrollFailure::Failed(message)
    }
}

impl std::fmt::Display for EnrollFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnrollFailure::Pending(m) | EnrollFailure::Failed(m) => f.write_str(m),
        }
    }
}

/// Enroll this instance with `code` at the hub at `hub_url` (see the module
/// docs): `agent enroll` on the command line, which waits here until the
/// page that showed the code has added this device (or the code expired).
pub async fn enroll(paths: &Paths, identity: &MachineIdentity, code: &EnrollCode, hub_url: &str) -> Result<(), EnrollFailure> {
    let begun = begin(paths, identity, code, hub_url)
        .await
        .map_err(|e| EnrollFailure::Failed(e.to_string()))?;
    println!(
        "hub {} assigned this device the id {} in account {}",
        begun.origin, begun.machine, begun.account
    );
    println!(
        "waiting for the droppen.app page that showed the code to add this device (it checks this device's proof first; account key {})…",
        display_fingerprint(&code.account_fingerprint)
    );
    let origin = begun.origin.clone();
    let seq = wait_confirmed(paths, identity, begun, CONFIRM_TIMEOUT).await?;
    println!("enrolled: the device chain (seq {seq}) names this device; genesis pinned");
    println!("start this instance's server (`serve` or `agent run`) and it will connect to {origin}");
    Ok(())
}

/// Step 3 on the enrollment's own socket: follow the account's chain until
/// it names this machine with exactly its keys; the chain's seq then.
pub async fn wait_confirmed(paths: &Paths, identity: &MachineIdentity, begun: Begun, timeout: Duration) -> Result<u64, EnrollFailure> {
    let Begun {
        machine,
        mut sink,
        mut source,
        ..
    } = begun;
    let mut state = AgentState::load(paths)?.ok_or_else(|| "the enrollment's state is gone".to_string())?;
    // Say first that this agent reads chain v3 (`docs/local-aggregator.md` §3.5): the
    // hub then sends it the v3 entry that adds it, and lets an account that
    // writes v3 add it at all.
    hub::send_json(
        &mut sink,
        &json!({"t": "agent_info", "version": crate::api::SERVER_VERSION, "features": [super::CHAIN_V3_FEATURE, super::POLICY_FEATURE]}),
    )
    .await?;
    hub::send_json(&mut sink, &json!({"t": "chain_sub", "from": 0})).await?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    let mut nonce_n = 0u64;
    loop {
        let message = tokio::select! {
            m = source.next() => m,
            _ = ping.tick() => {
                nonce_n += 1;
                use futures_util::SinkExt;
                let ping = Frame::Ping { nonce: nonce_n }.encode().map_err(|e| e.to_string())?;
                sink.send(Message::Binary(ping.into())).await.map_err(|e| e.to_string())?;
                continue;
            }
            () = tokio::time::sleep_until(deadline) => {
                return Err(EnrollFailure::Failed(
                    "the droppen.app page did not add this device in time, and the code has expired. Get a new one (my devices → add a device that runs AI → show a one-time code) and enroll again.".into(),
                ));
            }
        };
        let Some(Ok(message)) = message else {
            return Err(EnrollFailure::Pending(format!(
                "the hub closed the connection before the page added this device; `serve` finishes the enrollment once it does (state saved in {})",
                super::state::state_path(paths).display()
            )));
        };
        let Message::Binary(bytes) = message else { continue };
        match Frame::decode(&bytes) {
            Ok(Frame::Chain { entries, .. }) => {
                let entries = entries
                    .iter()
                    .map(|e| Entry::decode(e))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                let absorbed = state.absorb(&entries, now_ms());
                // Whatever took effect is kept, even when the batch stopped short.
                state.save(paths)?;
                match absorbed.error {
                    // A recovered device's self-activation, too early by this
                    // agent's clock: it waits here, the rest goes on.
                    None | Some(rh_proto::chain::ChainError::TooEarly { .. }) => {}
                    Some(e) => return Err(format!("the account's chain does not verify against the code's fingerprint: {e}").into()),
                }
                if state.enrolled() {
                    if !state.keys_match(&identity.sign_public(), &identity.dh_public()) {
                        return Err(format!("the chain added {machine} with other keys; refusing it").into());
                    }
                    return Ok(state.seq().unwrap_or(0));
                }
            }
            Ok(Frame::Attest { attestation, .. }) => {
                if let Ok(att) = Attestation::decode(&attestation) {
                    state.attest(att);
                }
            }
            _ => {}
        }
    }
}
