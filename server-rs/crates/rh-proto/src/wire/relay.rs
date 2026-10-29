//! `/v1/relay`: a relay's control link to the Hub, and the proof of origin
//! the Hub fetches from a relay before it registers it.

use serde::Serialize;

/// What a relay sends the Hub. Its `Debug` leaves out the registration
/// token and the signatures.
#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RelayToHub {
    /// The sign-in: `sig` (hex) over [`crate::sig::HubLogin`] with role
    /// `Relay` for the Hub's challenge; `features`, what the relay takes from
    /// the Hub beyond the first control link.
    Login { id: String, time_ms: u64, sig: String, version: String, features: Vec<String> },
    /// A first sign-in with the operator's registration token: the key the
    /// Hub pins, and the origin it checks the relay answers at
    /// ([`RelayIdAnswer`]).
    Register { token: String, sign_key: String, origin: String, version: String, time_ms: u64, sig: String, features: Vec<String> },
    /// The relay's state, every interval while signed in.
    Health {
        conns: u64,
        channels: u64,
        bytes_in_per_s: u64,
        bytes_out_per_s: u64,
        /// Connections over the limit, 0 to 1. The relay computes it as an
        /// `f32` and widens it with `f64::from`, which writes the digits of
        /// the widened value (`0.10000000149011612`, where an `f32` field
        /// would write `0.1`).
        load: f64,
        version: String,
    },
    /// Per-account transfer, sent until the Hub acknowledges it.
    Usage(UsageReport),
    /// The relay stops taking sign-ins and goes away at `until_ms`.
    Draining { until_ms: u64 },
    /// Retire this relay.
    Deregister { reason: String },
    /// Pin `new_key` instead of the signed-in key: `sig_new`, the new key's
    /// sign-in signature for the same challenge.
    Rotate { new_key: String, time_ms: u64, sig_new: String },
}

impl std::fmt::Debug for RelayToHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Login { id, time_ms, version, features, .. } => f
                .debug_struct("Login")
                .field("id", id)
                .field("time_ms", time_ms)
                .field("version", version)
                .field("features", features)
                .finish_non_exhaustive(),
            Self::Register { sign_key, origin, version, time_ms, features, .. } => f
                .debug_struct("Register")
                .field("sign_key", sign_key)
                .field("origin", origin)
                .field("version", version)
                .field("time_ms", time_ms)
                .field("features", features)
                .finish_non_exhaustive(),
            Self::Health { conns, channels, bytes_in_per_s, bytes_out_per_s, load, version } => f
                .debug_struct("Health")
                .field("conns", conns)
                .field("channels", channels)
                .field("bytes_in_per_s", bytes_in_per_s)
                .field("bytes_out_per_s", bytes_out_per_s)
                .field("load", load)
                .field("version", version)
                .finish(),
            Self::Usage(report) => f.debug_tuple("Usage").field(report).finish(),
            Self::Draining { until_ms } => f.debug_struct("Draining").field("until_ms", until_ms).finish(),
            Self::Deregister { reason } => f.debug_struct("Deregister").field("reason", reason).finish(),
            Self::Rotate { new_key, time_ms, .. } => f.debug_struct("Rotate").field("new_key", new_key).field("time_ms", time_ms).finish_non_exhaustive(),
        }
    }
}

/// One usage report: `boot` is random per relay process and `seq` counts up
/// within it, so the Hub records each report once however often it comes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageReport {
    pub boot: String,
    pub seq: u64,
    pub from_ms: u64,
    pub to_ms: u64,
    pub accounts: Vec<UsageLine>,
}

/// The `DATA` payload bytes one account had relayed in a report's interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageLine {
    pub account: String,
    pub bytes: u64,
}

/// `GET /v1/relay-id?nonce=…`, the relay's answer: its id and key, and `sig`
/// (hex), [`crate::ticket::sign_origin_proof`] over its origin and the Hub's
/// nonce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelayIdAnswer {
    pub relay_id: String,
    pub sign_key: String,
    pub origin: String,
    pub sig: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn relay_messages_have_the_documented_shape() {
        let features = || vec!["revoke-role/1".to_string()];
        let cases = [
            (
                RelayToHub::Login { id: "r-1".into(), time_ms: 7, sig: "ab".into(), version: "0.1.0".into(), features: features() },
                json!({"t": "login", "id": "r-1", "time_ms": 7, "sig": "ab", "version": "0.1.0", "features": ["revoke-role/1"]}),
            ),
            (
                RelayToHub::Register {
                    token: "tok".into(),
                    sign_key: "cd".into(),
                    origin: "wss://r".into(),
                    version: "0.1.0".into(),
                    time_ms: 7,
                    sig: "ab".into(),
                    features: features(),
                },
                json!({
                    "t": "register", "token": "tok", "sign_key": "cd", "origin": "wss://r", "version": "0.1.0", "time_ms": 7, "sig": "ab",
                    "features": ["revoke-role/1"],
                }),
            ),
            (
                RelayToHub::Health { conns: 3, channels: 1, bytes_in_per_s: 10, bytes_out_per_s: 10, load: 0.5, version: "0.1.0".into() },
                json!({"t": "health", "conns": 3, "channels": 1, "bytes_in_per_s": 10, "bytes_out_per_s": 10, "load": 0.5, "version": "0.1.0"}),
            ),
            (
                RelayToHub::Usage(UsageReport {
                    boot: "b0".into(),
                    seq: 2,
                    from_ms: 1,
                    to_ms: 5,
                    accounts: vec![UsageLine { account: "a-1".into(), bytes: 64 }],
                }),
                json!({"t": "usage", "boot": "b0", "seq": 2, "from_ms": 1, "to_ms": 5, "accounts": [{"account": "a-1", "bytes": 64}]}),
            ),
            (RelayToHub::Draining { until_ms: 9 }, json!({"t": "draining", "until_ms": 9})),
            (RelayToHub::Deregister { reason: "moved".into() }, json!({"t": "deregister", "reason": "moved"})),
            (
                RelayToHub::Rotate { new_key: "ef".into(), time_ms: 7, sig_new: "ab".into() },
                json!({"t": "rotate", "new_key": "ef", "time_ms": 7, "sig_new": "ab"}),
            ),
        ];
        for (message, want) in cases {
            assert_eq!(to_value(&message).unwrap(), want);
        }
    }

    #[test]
    fn debug_leaves_out_the_token_and_the_signatures() {
        let secret = || "secret".to_string();
        let messages = [
            RelayToHub::Login { id: "r-1".into(), time_ms: 7, sig: secret(), version: "0.1.0".into(), features: vec![] },
            RelayToHub::Register {
                token: secret(),
                sign_key: "cd".into(),
                origin: "wss://r".into(),
                version: "0.1.0".into(),
                time_ms: 7,
                sig: secret(),
                features: vec![],
            },
            RelayToHub::Rotate { new_key: "ef".into(), time_ms: 7, sig_new: secret() },
        ];
        for message in messages {
            let text = format!("{message:?}");
            assert!(!text.contains("secret"), "{text}");
            assert!(text.ends_with(", .. }"), "{text}");
        }
    }

    #[test]
    fn health_load_keeps_the_widened_f32_digits() {
        let load = 1.0f32 / 10.0;
        let health = RelayToHub::Health { conns: 1, channels: 0, bytes_in_per_s: 0, bytes_out_per_s: 0, load: f64::from(load), version: String::new() };
        assert_eq!(to_value(&health).unwrap()["load"], json!(load));
        assert!(serde_json::to_string(&health).unwrap().contains(r#""load":0.10000000149011612"#));
    }

    #[test]
    fn the_relay_id_answer_has_the_documented_shape() {
        let answer = RelayIdAnswer { relay_id: "r-1".into(), sign_key: "cd".into(), origin: "wss://r".into(), sig: "ab".into() };
        assert_eq!(to_value(&answer).unwrap(), json!({"relay_id": "r-1", "sign_key": "cd", "origin": "wss://r", "sig": "ab"}));
    }
}
