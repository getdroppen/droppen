//! `/v1/client`: a device's websocket to the Hub, and the JSON bodies a
//! device sends the Hub over HTTP.

use serde::Serialize;

/// What a device sends the Hub. Its `Debug` leaves out the signature and
/// the session.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DeviceToHub {
    /// The sign-in under a device session: `sig` (hex) over
    /// [`crate::sig::HubLogin`] with role `Device` for the Hub's challenge.
    /// `role` is always `device`.
    Login { role: String, account: String, id: String, time_ms: u64, sig: String, session: String },
    /// Stop the subscription on channel `ch`.
    StreamUnsub { ch: u32 },
}

impl std::fmt::Debug for DeviceToHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Login { role, account, id, time_ms, .. } => f
                .debug_struct("Login")
                .field("role", role)
                .field("account", account)
                .field("id", id)
                .field("time_ms", time_ms)
                .finish_non_exhaustive(),
            Self::StreamUnsub { ch } => f.debug_struct("StreamUnsub").field("ch", ch).finish(),
        }
    }
}

/// `POST /v1/session/challenge`: the challenge a device session signs.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SessionChallengeRequest {
    pub account: String,
    pub device: String,
}

/// `POST /v1/session`: a device session for the Hub's challenge, `signature`
/// (hex) over [`crate::sig::HubLogin`] with role `DeviceSession`.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SessionRequest {
    pub account: String,
    pub device: String,
    pub time_ms: u64,
    pub nonce: String,
    pub signature: String,
}

/// `POST /v1/mailbox`: mail for another device of the account. `purpose` is
/// one of [`crate::wrap`]'s `MAIL_*`, `sealed` (hex) what only the recipient
/// opens.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct MailboxPost {
    pub recipient: String,
    pub sender: String,
    pub purpose: u8,
    pub epoch: u32,
    pub sealed: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn device_messages_have_the_documented_shape() {
        let login = DeviceToHub::Login {
            role: "device".into(),
            account: "a-1".into(),
            id: "d-1".into(),
            time_ms: 7,
            sig: "ab".into(),
            session: "sess".into(),
        };
        assert_eq!(
            to_value(&login).unwrap(),
            json!({"t": "login", "role": "device", "account": "a-1", "id": "d-1", "time_ms": 7, "sig": "ab", "session": "sess"})
        );
        assert_eq!(to_value(DeviceToHub::StreamUnsub { ch: 3 }).unwrap(), json!({"t": "stream_unsub", "ch": 3}));
    }

    #[test]
    fn debug_leaves_out_the_signature_and_the_session() {
        let login = DeviceToHub::Login {
            role: "device".into(),
            account: "a-1".into(),
            id: "d-1".into(),
            time_ms: 7,
            sig: "secret-sig".into(),
            session: "secret-session".into(),
        };
        let text = format!("{login:?}");
        assert!(!text.contains("secret"), "{text}");
        assert!(text.ends_with(", .. }"), "{text}");
    }

    #[test]
    fn device_requests_have_the_documented_shape() {
        let challenge = SessionChallengeRequest { account: "a-1".into(), device: "d-1".into() };
        assert_eq!(to_value(&challenge).unwrap(), json!({"account": "a-1", "device": "d-1"}));
        let session = SessionRequest {
            account: "a-1".into(),
            device: "d-1".into(),
            time_ms: 7,
            nonce: "00".into(),
            signature: "ab".into(),
        };
        assert_eq!(
            to_value(&session).unwrap(),
            json!({"account": "a-1", "device": "d-1", "time_ms": 7, "nonce": "00", "signature": "ab"})
        );
        let mail = MailboxPost {
            recipient: "d-2".into(),
            sender: "d-1".into(),
            purpose: crate::wrap::MAIL_KEY_REQUEST,
            epoch: 0,
            sealed: String::new(),
        };
        assert_eq!(
            to_value(&mail).unwrap(),
            json!({"recipient": "d-2", "sender": "d-1", "purpose": 4, "epoch": 0, "sealed": ""})
        );
    }
}
