//! A standalone device claiming an enrollment code at the hub: an A2A proxy
//! joining its account with a `DA1` code (`docs/a2a.md` §7.2), over the
//! hub's enrollment socket as an agent claims an `RH1` code
//! (`docs/hub-api/websocket.md`, "Enrollment"), the code's kind said:
//!
//! ```text
//! hub    → challenge {nonce}
//! device → enroll {token, kind: "a2a"}
//! hub    → enroll_assigned {account, device}      (or error unknown_enrollment)
//! device → enroll_proof {kind: "a2a", sign_key, dh_key, time_ms, sig, proof}
//! hub    → enrolled {account, device}             (or error …), then it closes
//! ```
//!
//! `sig` is the `HubLogin {role: Device}` signature under the new
//! `sign_key` for the challenge and the assigned ids — the device holds its
//! key; `proof` is the code's proof over the account, the device id and both
//! keys ([`A2aEnrollCode::proof`]), which the page checks before it adds the
//! device to the chain. The device never signs in as an agent there: once
//! claimed, it is a device of the account, and the page adds it.
//!
//! Nothing here is trusted from the hub but the ids it assigns, and those
//! are bound into the proof the page checks and into the chain entry the
//! page signs; whether the device joined is its verified chain's to say
//! ([`crate::device::StandaloneDevice::watch_chain`]).
//!
//! [`claim_machine`] is the same exchange for the other kind of code — an
//! `RH1` code that adds a device which runs AI, rather than an A2A proxy — so
//! it proves with [`EnrollCode`]'s label, signs as an agent and answers
//! `welcome`.

use droppen_proto::frame::AGENT_PATH;
use droppen_proto::identity::{A2aEnrollCode, EnrollCode};
use droppen_proto::keys::DhKeypair;
use droppen_proto::sig::{HubLogin, LoginRole};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::link::next_json;
use crate::{hex, key32, now_ms, tap, HubUrl};

/// The kind an A2A proxy's code is registered and claimed as.
pub const KIND_A2A: &str = "a2a";
/// The kind a code for a device that runs AI is claimed as.
pub const KIND_MACHINE: &str = "machine";

/// What the hub assigned a claimed code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    pub account: String,
    pub device: String,
}

/// Why a claim did not go through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError {
    /// The hub said no (`error {code, message}`): `unknown_enrollment` (no
    /// such code, expired or used), a code of another kind, …
    Refused { code: String, message: String },
    /// No answer, or not one that says.
    Failed(String),
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::Refused { code, message } => write!(f, "the hub refused the code ({code}): {message}"),
            ClaimError::Failed(why) => f.write_str(why),
        }
    }
}

/// Claim `code` at `hub` as a device with these keys.
pub async fn claim_a2a(hub: &HubUrl, code: &A2aEnrollCode, sign: &SigningKey, dh: &DhKeypair) -> Result<Claimed, ClaimError> {
    let failed = ClaimError::Failed;
    let url = format!("{}{AGENT_PATH}", hub.ws);
    let (ws, _) = tokio::time::timeout(std::time::Duration::from_secs(20), tokio_tungstenite::connect_async(&url))
        .await
        .map_err(|_| failed("connect: no answer within 20 s".into()))?
        .map_err(|e| failed(format!("connect: {e}")))?;
    let (mut sink, mut stream) = ws.split();
    let challenge = next_json(&mut stream, AGENT_PATH).await.map_err(failed)?;
    let nonce = key32(challenge["nonce"].as_str().ok_or_else(|| failed("no challenge".into()))?).map_err(failed)?;
    send(&mut sink, json!({"t": "enroll", "token": hex(&code.token()), "kind": KIND_A2A})).await?;
    let assigned = next_json(&mut stream, AGENT_PATH).await.map_err(failed)?;
    let refused = |v: &Value| ClaimError::Refused {
        code: v["code"].as_str().unwrap_or("refused").to_string(),
        message: v["message"].as_str().unwrap_or_default().to_string(),
    };
    if assigned["t"] != "enroll_assigned" {
        return Err(if assigned["t"] == "error" {
            refused(&assigned)
        } else {
            failed(format!("not an answer to the claim: {assigned}"))
        });
    }
    let account = assigned["account"]
        .as_str()
        .ok_or_else(|| failed("no account assigned".into()))?
        .to_string();
    let device = assigned["device"]
        .as_str()
        .or_else(|| assigned["machine"].as_str())
        .ok_or_else(|| failed("no device id assigned".into()))?
        .to_string();
    let time_ms = now_ms();
    let sig = HubLogin {
        role: LoginRole::Device,
        account: account.clone(),
        id: device.clone(),
        hub: hub.origin.clone(),
        nonce,
        time_ms,
    }
    .sign(sign)
    .map_err(|e| failed(format!("sign: {e}")))?;
    let (sign_key, dh_key) = (sign.verifying_key().to_bytes(), dh.public());
    let proof = code.proof(&account, &device, &sign_key, &dh_key);
    let message = json!({
        "t": "enroll_proof",
        "kind": KIND_A2A,
        "sign_key": hex(&sign_key),
        "dh_key": hex(&dh_key),
        "time_ms": time_ms,
        "sig": hex(&sig),
        "proof": hex(&proof),
    });
    send(&mut sink, message).await?;
    let answer = next_json(&mut stream, AGENT_PATH).await.map_err(failed)?;
    match answer["t"].as_str() {
        Some("enrolled") => {
            let _ = sink.close().await;
            Ok(Claimed { account, device })
        }
        Some("error") => Err(refused(&answer)),
        _ => Err(failed(format!("not an answer to the proof: {answer}"))),
    }
}

/// Claim `code` at `hub` as a machine (`KIND_MACHINE`) with these keys — the
/// way a device that runs AI joins (`droppen agent enroll`), as an `RH1` code
/// asks.
///
/// It is [`claim_a2a`]'s exchange with a machine's own differences: the code
/// is an [`EnrollCode`], the proof is the machine's label
/// ([`EnrollCode::proof`]), the assigned id comes back as `machine`, and the
/// signature is a `HubLogin {role: Agent}` — the hub signs the device in as
/// its agent and answers `welcome`. The page that added the code then writes
/// the device into the chain with the host flag (`AddDevice3 {host}`), which
/// is what lets it control the account's computers; an A2A proxy stays
/// `A2aOnly` for good, a machine does not.
pub async fn claim_machine(hub: &HubUrl, code: &EnrollCode, sign: &SigningKey, dh: &DhKeypair) -> Result<Claimed, ClaimError> {
    let failed = ClaimError::Failed;
    let url = format!("{}{AGENT_PATH}", hub.ws);
    let (ws, _) = tokio::time::timeout(std::time::Duration::from_secs(20), tokio_tungstenite::connect_async(&url))
        .await
        .map_err(|_| failed("connect: no answer within 20 s".into()))?
        .map_err(|e| failed(format!("connect: {e}")))?;
    let (mut sink, mut stream) = ws.split();
    let challenge = next_json(&mut stream, AGENT_PATH).await.map_err(failed)?;
    let nonce = key32(challenge["nonce"].as_str().ok_or_else(|| failed("no challenge".into()))?).map_err(failed)?;
    send(&mut sink, json!({"t": "enroll", "token": hex(&code.token()), "kind": KIND_MACHINE})).await?;
    let assigned = next_json(&mut stream, AGENT_PATH).await.map_err(failed)?;
    let refused = |v: &Value| ClaimError::Refused {
        code: v["code"].as_str().unwrap_or("refused").to_string(),
        message: v["message"].as_str().unwrap_or_default().to_string(),
    };
    if assigned["t"] != "enroll_assigned" {
        return Err(if assigned["t"] == "error" {
            refused(&assigned)
        } else {
            failed(format!("not an answer to the claim: {assigned}"))
        });
    }
    let account = assigned["account"]
        .as_str()
        .ok_or_else(|| failed("no account assigned".into()))?
        .to_string();
    let machine = assigned["machine"]
        .as_str()
        .or_else(|| assigned["device"].as_str())
        .ok_or_else(|| failed("no machine id assigned".into()))?
        .to_string();
    let time_ms = now_ms();
    let sig = HubLogin {
        role: LoginRole::Agent,
        account: account.clone(),
        id: machine.clone(),
        hub: hub.origin.clone(),
        nonce,
        time_ms,
    }
    .sign(sign)
    .map_err(|e| failed(format!("sign: {e}")))?;
    let (sign_key, dh_key) = (sign.verifying_key().to_bytes(), dh.public());
    let proof = code.proof(&account, &machine, &sign_key, &dh_key);
    let message = json!({
        "t": "enroll_proof",
        "kind": KIND_MACHINE,
        "sign_key": hex(&sign_key),
        "dh_key": hex(&dh_key),
        "time_ms": time_ms,
        "sig": hex(&sig),
        "proof": hex(&proof),
    });
    send(&mut sink, message).await?;
    let answer = next_json(&mut stream, AGENT_PATH).await.map_err(failed)?;
    match answer["t"].as_str() {
        // `welcome` is the machine's: it is also its sign-in as an agent.
        Some("welcome" | "enrolled") => {
            let _ = sink.close().await;
            Ok(Claimed {
                account,
                device: machine,
            })
        }
        Some("error") => Err(refused(&answer)),
        _ => Err(failed(format!("not an answer to the proof: {answer}"))),
    }
}

/// Send `message` on the enrollment socket.
async fn send<S>(sink: &mut S, message: Value) -> Result<(), ClaimError>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    let text = tap::sent(AGENT_PATH, message.to_string());
    sink.send(Message::Text(text.into()))
        .await
        .map_err(|e| ClaimError::Failed(format!("send: {e}")))
}
