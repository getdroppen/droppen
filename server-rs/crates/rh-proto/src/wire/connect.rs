//! `/v1/connect`: a relay's websocket. An agent or a device signs in on it
//! with a ticket from the Hub ([`crate::ticket`]), and the relay copies its
//! channels' frames to the other side.

use serde::{Deserialize, Serialize};

/// What an agent or a device sends a relay. Its `Debug` leaves out the
/// ticket and the signature.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum PeerToRelay {
    /// The sign-in: the Hub's ticket, and the holder's signature (hex) over
    /// [`crate::sig::HubLogin`] for the relay's challenge.
    Login { ticket: String, time_ms: u64, sig: String },
    /// A renewed ticket for the signed-in connection.
    Ticket { ticket: String },
}

impl std::fmt::Debug for PeerToRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Login { time_ms, .. } => f.debug_struct("Login").field("time_ms", time_ms).finish_non_exhaustive(),
            Self::Ticket { .. } => f.debug_struct("Ticket").finish_non_exhaustive(),
        }
    }
}

/// What a relay sends an agent or a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RelayToPeer {
    /// The first message on a connection: the nonce a sign-in signs (hex).
    Challenge { nonce: String, relay: String, origin: String, version: u32 },
    /// Signed in: the ticket's holder and expiry.
    Welcome { relay: String, id: String, expires_ms: u64 },
    /// To a device: every machine its ticket names after its `welcome`, then
    /// each one that comes or goes.
    Presence { machines: Vec<MachinePresence> },
    /// A renewed ticket was taken: the connection's new expiry.
    TicketOk { expires_ms: u64 },
    /// The relay is going away: sign in elsewhere after `retry_ms`.
    Goaway { retry_ms: u64, reason: String },
    /// A refusal, mostly followed by a close.
    Error {
        code: String,
        message: String,
        /// Only when the relay is shutting down.
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_ms: Option<u64>,
    },
}

/// One machine in a [`RelayToPeer::Presence`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MachinePresence {
    pub id: String,
    pub state: PresenceState,
    /// When the machine came or went: in a change only, not in the list a
    /// device gets after its `welcome`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PresenceState {
    Online,
    Offline,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn peer_messages_have_the_documented_shape() {
        let login = PeerToRelay::Login { ticket: "tk".into(), time_ms: 7, sig: "ab".into() };
        assert_eq!(to_value(&login).unwrap(), json!({"t": "login", "ticket": "tk", "time_ms": 7, "sig": "ab"}));
        let ticket = PeerToRelay::Ticket { ticket: "tk".into() };
        assert_eq!(to_value(&ticket).unwrap(), json!({"t": "ticket", "ticket": "tk"}));
    }

    #[test]
    fn a_relay_reads_peer_messages_in_any_key_order_and_skips_unknown_keys() {
        let read = |text: &str| serde_json::from_str::<PeerToRelay>(text).ok();
        assert_eq!(
            read(r#"{"sig":"ab","time_ms":7,"extra":[1],"ticket":"tk","t":"login"}"#),
            Some(PeerToRelay::Login { ticket: "tk".into(), time_ms: 7, sig: "ab".into() })
        );
        assert_eq!(read(r#"{"t":"ticket","ticket":"tk"}"#), Some(PeerToRelay::Ticket { ticket: "tk".into() }));
        assert_eq!(read(r#"{"t":"login","ticket":"tk","sig":"ab"}"#), None, "time_ms is required");
        assert_eq!(read(r#"{"t":"challenge","nonce":"00"}"#), None, "not a peer's message");
    }

    #[test]
    fn relay_messages_have_the_documented_shape() {
        let cases = [
            (
                RelayToPeer::Challenge { nonce: "00".into(), relay: "r-1".into(), origin: "wss://r".into(), version: 1 },
                json!({"t": "challenge", "nonce": "00", "relay": "r-1", "origin": "wss://r", "version": 1}),
            ),
            (
                RelayToPeer::Welcome { relay: "r-1".into(), id: "m-1".into(), expires_ms: 9 },
                json!({"t": "welcome", "relay": "r-1", "id": "m-1", "expires_ms": 9}),
            ),
            (
                RelayToPeer::Presence {
                    machines: vec![
                        MachinePresence { id: "m-1".into(), state: PresenceState::Online, since_ms: None },
                        MachinePresence { id: "m-2".into(), state: PresenceState::Offline, since_ms: Some(5) },
                    ],
                },
                json!({"t": "presence", "machines": [{"id": "m-1", "state": "online"}, {"id": "m-2", "state": "offline", "since_ms": 5}]}),
            ),
            (RelayToPeer::TicketOk { expires_ms: 9 }, json!({"t": "ticket_ok", "expires_ms": 9})),
            (
                RelayToPeer::Goaway { retry_ms: 1000, reason: "drain".into() },
                json!({"t": "goaway", "retry_ms": 1000, "reason": "drain"}),
            ),
            (
                RelayToPeer::Error { code: "denied".into(), message: "no".into(), retry_ms: None },
                json!({"t": "error", "code": "denied", "message": "no"}),
            ),
            (
                RelayToPeer::Error { code: "busy".into(), message: "later".into(), retry_ms: Some(1000) },
                json!({"t": "error", "code": "busy", "message": "later", "retry_ms": 1000}),
            ),
        ];
        for (message, want) in cases {
            assert_eq!(to_value(&message).unwrap(), want);
        }
    }

    #[test]
    fn debug_leaves_out_the_ticket_and_the_signature() {
        let login = PeerToRelay::Login { ticket: "secret-ticket".into(), time_ms: 7, sig: "secret-sig".into() };
        let ticket = PeerToRelay::Ticket { ticket: "secret-ticket".into() };
        for text in [format!("{login:?}"), format!("{ticket:?}")] {
            assert!(!text.contains("secret"), "{text}");
        }
        assert_eq!(format!("{login:?}"), "Login { time_ms: 7, .. }");
    }

    #[test]
    fn the_tag_is_written_first() {
        let text = serde_json::to_string(&RelayToPeer::TicketOk { expires_ms: 9 }).unwrap();
        assert_eq!(text, r#"{"t":"ticket_ok","expires_ms":9}"#);
    }
}
