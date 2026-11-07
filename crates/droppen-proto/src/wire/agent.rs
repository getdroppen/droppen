//! `/v1/agent`: a machine's websocket to the Hub, and the JSON bodies a
//! machine sends the Hub over HTTP.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

/// What a machine sends the Hub. Its `Debug` leaves out the enrollment
/// token, the proof and every signature.
#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AgentToHub {
    /// The sign-in: `sig` (hex) over [`crate::sig::HubLogin`] with role
    /// `Agent` for the Hub's challenge. `role` is always `agent`.
    Login { role: String, account: String, id: String, time_ms: u64, sig: String },
    /// The first message of an enrollment: the enrollment code's token (hex).
    Enroll { token: String },
    /// The machine's keys for the account and machine the Hub assigned:
    /// `sig` signs in as that machine, `proof` (hex) is the enrollment
    /// code's proof over the keys ([`crate::identity::EnrollCode`]).
    EnrollProof { sign_key: String, dh_key: String, time_ms: u64, sig: String, proof: String },
    /// This build's version and what it reads beyond the first protocol.
    AgentInfo { version: String, features: Vec<String> },
    /// Send the device chain from entry `from` on.
    ChainSub { from: u64 },
    /// A mail is taken: `id` is the Hub's own value, echoed as it came.
    MailAck { id: Value },
    /// A mail goes back to the mailbox for the next sign-in, `id` echoed.
    MailRelease { id: Value },
    /// Tickets for these relays.
    RelayTickets { relays: Vec<String> },
    /// The relays this machine measured (round trip, ms) and those it is
    /// signed in on.
    RelayState { rtts: BTreeMap<String, u32>, attached: BTreeSet<String> },
    /// An identity key renewal to stage: the new keys, the chain head they
    /// were staged at, and both keys' signatures over the renewal
    /// ([`crate::chain::host_rotate_body`]).
    RotationStage {
        nonce: String,
        staged_seq: u64,
        staged_head: String,
        sign_key: String,
        dh_key: String,
        assurance: u8,
        old_sig: String,
        new_sig: String,
    },
    /// The staged key confirms the renewal the Hub asked about: `sign_key`
    /// echoed as it came, `sig` ([`crate::chain::rotate_ack_body`]).
    RotationAck { sign_key: Value, sig: String },
    /// This machine holds no staged key `sign_key` (echoed as it came).
    RotationUnknown { sign_key: Value },
    /// The streams the Hub keeps for this machine.
    StreamList,
    /// Delete one of them (`stream`, hex).
    StreamDelete { stream: String },
}

impl std::fmt::Debug for AgentToHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Login { role, account, id, time_ms, .. } => f
                .debug_struct("Login")
                .field("role", role)
                .field("account", account)
                .field("id", id)
                .field("time_ms", time_ms)
                .finish_non_exhaustive(),
            Self::Enroll { .. } => f.debug_struct("Enroll").finish_non_exhaustive(),
            Self::EnrollProof { sign_key, dh_key, time_ms, .. } => f
                .debug_struct("EnrollProof")
                .field("sign_key", sign_key)
                .field("dh_key", dh_key)
                .field("time_ms", time_ms)
                .finish_non_exhaustive(),
            Self::AgentInfo { version, features } => f.debug_struct("AgentInfo").field("version", version).field("features", features).finish(),
            Self::ChainSub { from } => f.debug_struct("ChainSub").field("from", from).finish(),
            Self::MailAck { id } => f.debug_struct("MailAck").field("id", id).finish(),
            Self::MailRelease { id } => f.debug_struct("MailRelease").field("id", id).finish(),
            Self::RelayTickets { relays } => f.debug_struct("RelayTickets").field("relays", relays).finish(),
            Self::RelayState { rtts, attached } => f.debug_struct("RelayState").field("rtts", rtts).field("attached", attached).finish(),
            Self::RotationStage {
                nonce,
                staged_seq,
                staged_head,
                sign_key,
                dh_key,
                assurance,
                ..
            } => f
                .debug_struct("RotationStage")
                .field("nonce", nonce)
                .field("staged_seq", staged_seq)
                .field("staged_head", staged_head)
                .field("sign_key", sign_key)
                .field("dh_key", dh_key)
                .field("assurance", assurance)
                .finish_non_exhaustive(),
            Self::RotationAck { sign_key, .. } => f.debug_struct("RotationAck").field("sign_key", sign_key).finish_non_exhaustive(),
            Self::RotationUnknown { sign_key } => f.debug_struct("RotationUnknown").field("sign_key", sign_key).finish(),
            Self::StreamList => f.write_str("StreamList"),
            Self::StreamDelete { stream } => f.debug_struct("StreamDelete").field("stream", stream).finish(),
        }
    }
}

/// `POST /v1/rotations/{host}/disputes`: the retired key disputes the
/// renewal at chain entry `seq` (`entry_hash`, hex); `sig` (hex) over
/// [`crate::chain::rotate_dispute_body`].
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct DisputePost {
    pub seq: u64,
    pub entry_hash: String,
    pub sig: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn machine_messages_have_the_documented_shape() {
        let cases = [
            (
                AgentToHub::Login { role: "agent".into(), account: "a-1".into(), id: "m-1".into(), time_ms: 7, sig: "ab".into() },
                json!({"t": "login", "role": "agent", "account": "a-1", "id": "m-1", "time_ms": 7, "sig": "ab"}),
            ),
            (AgentToHub::Enroll { token: "tk".into() }, json!({"t": "enroll", "token": "tk"})),
            (
                AgentToHub::EnrollProof { sign_key: "s".into(), dh_key: "d".into(), time_ms: 7, sig: "ab".into(), proof: "pf".into() },
                json!({"t": "enroll_proof", "sign_key": "s", "dh_key": "d", "time_ms": 7, "sig": "ab", "proof": "pf"}),
            ),
            (
                AgentToHub::AgentInfo { version: "0.2.0".into(), features: vec!["chain/3".into()] },
                json!({"t": "agent_info", "version": "0.2.0", "features": ["chain/3"]}),
            ),
            (AgentToHub::ChainSub { from: 3 }, json!({"t": "chain_sub", "from": 3})),
            (AgentToHub::MailAck { id: json!(41) }, json!({"t": "mail_ack", "id": 41})),
            (AgentToHub::MailRelease { id: json!("odd") }, json!({"t": "mail_release", "id": "odd"})),
            (AgentToHub::MailAck { id: Value::Null }, json!({"t": "mail_ack", "id": null})),
            (AgentToHub::RelayTickets { relays: vec!["r-1".into()] }, json!({"t": "relay_tickets", "relays": ["r-1"]})),
            (
                AgentToHub::RelayState { rtts: BTreeMap::from([("r-2".into(), 30), ("r-1".into(), 12)]), attached: BTreeSet::from(["r-1".into()]) },
                json!({"t": "relay_state", "rtts": {"r-1": 12, "r-2": 30}, "attached": ["r-1"]}),
            ),
            (
                AgentToHub::RotationStage {
                    nonce: "n".into(),
                    staged_seq: 9,
                    staged_head: "h".into(),
                    sign_key: "s".into(),
                    dh_key: "d".into(),
                    assurance: 1,
                    old_sig: "o".into(),
                    new_sig: "w".into(),
                },
                json!({
                    "t": "rotation_stage", "nonce": "n", "staged_seq": 9, "staged_head": "h", "sign_key": "s", "dh_key": "d", "assurance": 1,
                    "old_sig": "o", "new_sig": "w",
                }),
            ),
            (AgentToHub::RotationAck { sign_key: json!("s"), sig: "ab".into() }, json!({"t": "rotation_ack", "sign_key": "s", "sig": "ab"})),
            (AgentToHub::RotationUnknown { sign_key: Value::Null }, json!({"t": "rotation_unknown", "sign_key": null})),
            (AgentToHub::StreamList, json!({"t": "stream_list"})),
            (AgentToHub::StreamDelete { stream: "00".into() }, json!({"t": "stream_delete", "stream": "00"})),
        ];
        for (message, want) in cases {
            assert_eq!(to_value(&message).unwrap(), want);
        }
    }

    #[test]
    fn debug_leaves_out_the_token_the_proof_and_the_signatures() {
        let secret = || "secret".to_string();
        let messages = [
            AgentToHub::Login { role: "agent".into(), account: "a-1".into(), id: "m-1".into(), time_ms: 7, sig: secret() },
            AgentToHub::Enroll { token: secret() },
            AgentToHub::EnrollProof { sign_key: "s".into(), dh_key: "d".into(), time_ms: 7, sig: secret(), proof: secret() },
            AgentToHub::RotationStage {
                nonce: "n".into(),
                staged_seq: 9,
                staged_head: "h".into(),
                sign_key: "s".into(),
                dh_key: "d".into(),
                assurance: 1,
                old_sig: secret(),
                new_sig: secret(),
            },
            AgentToHub::RotationAck { sign_key: json!("s"), sig: secret() },
        ];
        for message in messages {
            let text = format!("{message:?}");
            assert!(!text.contains("secret"), "{text}");
            assert!(text.ends_with(".. }"), "{text}");
        }
    }

    #[test]
    fn a_dispute_has_the_documented_shape() {
        let dispute = DisputePost { seq: 12, entry_hash: "eh".into(), sig: "ab".into() };
        assert_eq!(to_value(&dispute).unwrap(), json!({"seq": 12, "entry_hash": "eh", "sig": "ab"}));
    }
}
