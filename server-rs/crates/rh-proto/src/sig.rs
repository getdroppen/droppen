//! Every Ed25519 signature in the protocol, in one place.
//!
//! A key that signs more than one kind of thing must never be tricked into
//! signing one kind and having it read as another — above all, it must never
//! sign bytes the hub chose. So each signature covers a fixed, NUL-terminated
//! label followed by a fixed binary structure, and the labels are listed here:
//!
//! | label | signer | structure |
//! |---|---|---|
//! | `rh/v1 chain-entry` | `AK` (genesis), an admin device, the recovery key, or a passkey slot | [`crate::chain::Draft::encode`] |
//! | `rh/v1 chain-attest` | an admin device | [`crate::chain::Attestation`] |
//! | `rh/v1 stream-head` | a machine | [`crate::record::StreamHead`] |
//! | `rh/v1 stream-manifest` | a machine | [`crate::stream::Manifest`] |
//! | `rh/v1 hub-login` | a machine or a device | [`HubLogin`] |
//! | `rh/v1 push` | a machine | reserved for Web Push payloads (M3) |
//! | `rh/v1 recovery-proof` | the recovery key `RSK` | [`crate::chain::recovery_proof_body`] |
//! | `rh/v1 stream-draft` | a machine | [`crate::stream::seal_ephemeral`]'s `stream | payload` |
//! | `rh/v1 relay-ticket` | the Hub's ticket key | [`crate::ticket::RelayTicket::encode`] |
//! | `rh/v1 relay-origin` | a relay | [`crate::ticket::origin_proof_body`] |
//! | `rh/v1 passkey-proof` | a passkey slot's `PAK` | [`crate::chain::passkey_proof_body`] |
//! | `rh/v1 host-rotate` | a host's current and its new identity key | [`crate::chain::host_rotate_body`] |
//! | `rh/v1 rotate-ack` | a host's new identity key | [`crate::chain::rotate_ack_body`] |
//! | `rh/v1 rotate-dispute` | the sign key a renewal retired | [`crate::chain::rotate_dispute_body`] |
//!
//! Every label ends in NUL and has none before it, so no label is a prefix of
//! another and no structure can be mistaken for a different label's.

use ed25519_dalek::{Signer as _, SigningKey};

use crate::codec::{check_u64, put_str16, DecodeError};
use crate::keys::ed25519_verify;

pub const CHAIN_ENTRY: &[u8] = b"rh/v1 chain-entry\0";
pub const CHAIN_ATTEST: &[u8] = b"rh/v1 chain-attest\0";
pub const STREAM_HEAD: &[u8] = b"rh/v1 stream-head\0";
pub const STREAM_MANIFEST: &[u8] = b"rh/v1 stream-manifest\0";
pub const HUB_LOGIN: &[u8] = b"rh/v1 hub-login\0";
pub const PUSH_PAYLOAD: &[u8] = b"rh/v1 push\0";
pub const RECOVERY_PROOF: &[u8] = b"rh/v1 recovery-proof\0";
pub const STREAM_DRAFT: &[u8] = b"rh/v1 stream-draft\0";
pub const RELAY_TICKET: &[u8] = b"rh/v1 relay-ticket\0";
pub const RELAY_ORIGIN: &[u8] = b"rh/v1 relay-origin\0";
pub const PASSKEY_PROOF: &[u8] = b"rh/v1 passkey-proof\0";
/// Both of a host's identity keys, the current and the new one, over the
/// renewal's proof (`docs/host-identity-rotation.md` §3.2).
pub const HOST_ROTATE: &[u8] = b"rh/v1 host-rotate\0";
/// A host's new identity key confirming a staged renewal on a connection that
/// signed in with the old one (§3.3).
pub const ROTATE_ACK: &[u8] = b"rh/v1 rotate-ack\0";
/// The sign key a renewal retired, disputing that renewal by its hash
/// (`docs/host-identity-rotation.md` §3.8 「证据：退役钥签的争议声明」).
pub const ROTATE_DISPUTE: &[u8] = b"rh/v1 rotate-dispute\0";

/// Every label; [`sign`] refuses anything else.
pub const LABELS: [&[u8]; 14] = [
    CHAIN_ENTRY,
    CHAIN_ATTEST,
    STREAM_HEAD,
    STREAM_MANIFEST,
    HUB_LOGIN,
    PUSH_PAYLOAD,
    RECOVERY_PROOF,
    STREAM_DRAFT,
    RELAY_TICKET,
    RELAY_ORIGIN,
    PASSKEY_PROOF,
    HOST_ROTATE,
    ROTATE_ACK,
    ROTATE_DISPUTE,
];

fn message(label: &[u8], body: &[u8]) -> Vec<u8> {
    assert!(LABELS.contains(&label), "an unregistered signature label");
    [label, body].concat()
}

/// Sign `body` under `label`.
pub fn sign(key: &SigningKey, label: &'static [u8], body: &[u8]) -> [u8; 64] {
    key.sign(&message(label, body)).to_bytes()
}

/// Strictly verify a signature over `body` under `label`.
pub fn verify(key: &[u8; 32], label: &'static [u8], body: &[u8], signature: &[u8; 64]) -> bool {
    ed25519_verify(key, &message(label, body), signature)
}

/// Who is signing in to the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LoginRole {
    Agent = 1,
    Device = 2,
    /// A device trading its signature for an HTTP session (`POST
    /// /v1/session`): kept apart from the WebSocket sign-in's role.
    DeviceSession = 3,
    /// A device signing in on a relay with its ticket (`hub` is the relay's
    /// origin): worthless at the Hub and at any other relay.
    RelayDevice = 4,
    /// A machine signing in on a relay with its ticket.
    RelayAgent = 5,
    /// A relay signing in on the Hub's control link (`account` empty, `id`
    /// the relay's).
    Relay = 6,
}

/// The answer to a hub's sign-in challenge. The hub chooses `nonce`; the
/// signer chooses everything else and signs the structure, never the nonce
/// alone, so a challenge cannot be a chain entry, a stream head or another
/// hub's login in disguise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubLogin {
    pub role: LoginRole,
    pub account: String,
    /// The machine or device id.
    pub id: String,
    /// The hub's origin, e.g. `wss://hub.example`: a login for one hub is
    /// worthless at another.
    pub hub: String,
    pub nonce: [u8; 32],
    pub time_ms: u64,
}

impl HubLogin {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        check_u64(self.time_ms)?;
        for (what, value, max) in [("account id", &self.account, 64), ("id", &self.id, 64), ("hub origin", &self.hub, 256)] {
            if value.len() > max {
                return Err(DecodeError::TooLarge { what, len: value.len(), max });
            }
        }
        let mut out = vec![self.role as u8];
        put_str16(&mut out, &self.account);
        put_str16(&mut out, &self.id);
        put_str16(&mut out, &self.hub);
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.time_ms.to_be_bytes());
        Ok(out)
    }

    pub fn sign(&self, key: &SigningKey) -> Result<[u8; 64], DecodeError> {
        Ok(sign(key, HUB_LOGIN, &self.encode()?))
    }

    pub fn verify(&self, key: &[u8; 32], signature: &[u8; 64]) -> bool {
        self.encode().map(|body| verify(key, HUB_LOGIN, &body, signature)).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_distinct_nul_terminated_and_prefix_free() {
        for (i, a) in LABELS.iter().enumerate() {
            assert_eq!(a.last(), Some(&0), "NUL-terminated");
            assert_eq!(a.iter().filter(|&&b| b == 0).count(), 1, "one NUL");
            for (j, b) in LABELS.iter().enumerate() {
                if i != j {
                    assert!(
                        !b.starts_with(a),
                        "{:?} is a prefix of {:?}",
                        String::from_utf8_lossy(a),
                        String::from_utf8_lossy(b)
                    );
                }
            }
        }
    }

    #[test]
    fn a_signature_under_one_label_does_not_verify_under_another() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let public = key.verifying_key().to_bytes();
        let body = b"same bytes";
        let sig = sign(&key, STREAM_HEAD, body);
        assert!(verify(&public, STREAM_HEAD, body, &sig));
        for label in [
            CHAIN_ENTRY,
            CHAIN_ATTEST,
            HUB_LOGIN,
            PUSH_PAYLOAD,
            RECOVERY_PROOF,
            STREAM_DRAFT,
            RELAY_TICKET,
            RELAY_ORIGIN,
            PASSKEY_PROOF,
            HOST_ROTATE,
            ROTATE_ACK,
            ROTATE_DISPUTE,
        ] {
            assert!(!verify(&public, label, body, &sig));
        }
    }

    #[test]
    fn a_hub_login_binds_role_hub_nonce_and_time() {
        let key = SigningKey::from_bytes(&[6; 32]);
        let public = key.verifying_key().to_bytes();
        let login = HubLogin {
            role: LoginRole::Agent,
            account: "a".into(),
            id: "m-1".into(),
            hub: "wss://hub.example".into(),
            nonce: [1; 32],
            time_ms: 5,
        };
        let sig = login.sign(&key).unwrap();
        assert!(login.verify(&public, &sig));
        for other in [
            HubLogin {
                role: LoginRole::Device,
                ..login.clone()
            },
            HubLogin {
                role: LoginRole::RelayAgent,
                ..login.clone()
            },
            HubLogin {
                hub: "wss://evil.example".into(),
                ..login.clone()
            },
            HubLogin {
                nonce: [2; 32],
                ..login.clone()
            },
            HubLogin { time_ms: 6, ..login.clone() },
        ] {
            assert!(!other.verify(&public, &sig));
        }
    }
}
