//! Relay tickets (`docs/relay-and-plans.md` §2.3): the Hub's signed
//! permission for one device or machine to use one relay for 15 minutes.
//!
//! A relay verifies a ticket offline — it holds the Hub's ticket public keys
//! and nothing else — and then asks the holder to prove it owns the ticket's
//! `sign_key` ([`crate::sig::HubLogin`] with role `RelayDevice`/`RelayAgent`
//! and the relay's own origin), so a stolen ticket is worthless on its own. A
//! device's ticket lists the machines it may open channels to (`peers`); an
//! agent's lists none. Tickets are opaque bytes to the browser.
//!
//! ```text
//! RelayTicket v1 (big-endian, u16-length-prefixed strings)
//!   version     u8        1
//!   kid         u8        which of the Hub's ticket keys signed it
//!   relay       str ≤32
//!   account     str ≤64
//!   role        u8        1 = device, 2 = agent
//!   id          str ≤64   the device or machine
//!   sign_key    [32]      its Ed25519 key
//!   peers       u8 count, str ≤64 each (≤64): machines a device may reach
//!   limits      bytes_per_sec u32, frames_per_sec u32, channels u16, account_conns u16
//!   flags       u8        bit 0: throttled (free transfer used up); bit 1: an emergency ticket
//!   issued_ms   u64
//!   expires_ms  u64
//! signature: Ed25519(ticket key[kid], "rh/v1 relay-ticket\0" ‖ encoding)
//! on the wire: base64url(encoding ‖ signature), unpadded
//! ```

use ed25519_dalek::SigningKey;
use thiserror::Error;

use crate::codec::{check_u64, put_str16, DecodeError, Reader};
use crate::sig::{self, RELAY_TICKET};

/// The encoding's version byte.
const TICKET_VERSION: u8 = 1;
/// How long a ticket is good for.
pub const TICKET_TTL_MS: u64 = 15 * 60 * 1000;
/// The longest a ticket may run, `expires_ms - issued_ms`, for a verifier to
/// take it ([`SignedTicket::verify`]): what the Hub issues, so the 15 minutes
/// hold at the relay too, not only at the issuer. A Hub that ever issues
/// longer tickets needs its relays to take them first.
pub const MAX_TICKET_LIFETIME_MS: u64 = TICKET_TTL_MS;
/// How far ahead of a verifier's clock a ticket's `issued_ms` may lie (the
/// Hub's clock and a relay's are not the same clock). Five times a relay's
/// default sign-in skew of a minute, so a relay that takes sign-ins at all
/// does not refuse a fresh ticket for it.
pub const TICKET_CLOCK_SKEW_MS: u64 = 5 * 60 * 1000;
const MAX_RELAY_ID_LEN: usize = 32;
pub const MAX_PEERS: usize = 64;
/// `flags` bit 0: the account used up its free transfer; connections are throttled.
pub const FLAG_THROTTLED: u8 = 1;
/// `flags` bit 1: an emergency ticket for a relay outside the account's set.
pub const FLAG_EMERGENCY: u8 = 2;

/// Who a ticket is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TicketRole {
    Device = 1,
    Agent = 2,
}

impl TicketRole {
    /// The sign-in role a holder signs with on a relay.
    pub fn login_role(self) -> sig::LoginRole {
        match self {
            TicketRole::Device => sig::LoginRole::RelayDevice,
            TicketRole::Agent => sig::LoginRole::RelayAgent,
        }
    }
}

/// What a relay holds one connection to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TicketLimits {
    pub bytes_per_sec: u32,
    pub frames_per_sec: u32,
    pub channels: u16,
    /// Connections the account may hold on this relay at once.
    pub account_conns: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTicket {
    pub kid: u8,
    pub relay: String,
    pub account: String,
    pub role: TicketRole,
    pub id: String,
    pub sign_key: [u8; 32],
    pub peers: Vec<String>,
    pub limits: TicketLimits,
    pub flags: u8,
    pub issued_ms: u64,
    pub expires_ms: u64,
}

/// Why a ticket was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TicketError {
    #[error("malformed ticket: {0}")]
    Malformed(String),
    #[error("signed by an unknown ticket key ({0})")]
    UnknownKey(u8),
    #[error("the ticket's signature does not verify")]
    BadSignature,
    #[error("the ticket is for relay {0:?}")]
    OtherRelay(String),
    #[error("the ticket expired")]
    Expired,
    #[error("the ticket runs longer than a ticket may")]
    TooLong,
    #[error("the ticket was issued ahead of this clock")]
    IssuedAhead,
}

fn too_large(what: &'static str, len: usize, max: usize) -> DecodeError {
    DecodeError::TooLarge { what, len, max }
}

impl RelayTicket {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        check_u64(self.issued_ms)?;
        check_u64(self.expires_ms)?;
        for (what, value, max) in [
            ("relay id", &self.relay, MAX_RELAY_ID_LEN),
            ("account id", &self.account, 64),
            ("id", &self.id, 64),
        ] {
            if value.len() > max {
                return Err(too_large(what, value.len(), max));
            }
        }
        if self.peers.len() > MAX_PEERS {
            return Err(too_large("peers", self.peers.len(), MAX_PEERS));
        }
        let mut out = vec![TICKET_VERSION, self.kid];
        put_str16(&mut out, &self.relay);
        put_str16(&mut out, &self.account);
        out.push(self.role as u8);
        put_str16(&mut out, &self.id);
        out.extend_from_slice(&self.sign_key);
        out.push(self.peers.len() as u8);
        for peer in &self.peers {
            if peer.len() > 64 {
                return Err(too_large("peer id", peer.len(), 64));
            }
            put_str16(&mut out, peer);
        }
        out.extend_from_slice(&self.limits.bytes_per_sec.to_be_bytes());
        out.extend_from_slice(&self.limits.frames_per_sec.to_be_bytes());
        out.extend_from_slice(&self.limits.channels.to_be_bytes());
        out.extend_from_slice(&self.limits.account_conns.to_be_bytes());
        out.push(self.flags);
        out.extend_from_slice(&self.issued_ms.to_be_bytes());
        out.extend_from_slice(&self.expires_ms.to_be_bytes());
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        let version = r.u8()?;
        if version != TICKET_VERSION {
            return Err(DecodeError::Unknown {
                what: "ticket version",
                value: version,
            });
        }
        let kid = r.u8()?;
        let relay = r.str16("relay id", MAX_RELAY_ID_LEN)?;
        let account = r.str16("account id", 64)?;
        let role = match r.u8()? {
            1 => TicketRole::Device,
            2 => TicketRole::Agent,
            value => return Err(DecodeError::Unknown { what: "ticket role", value }),
        };
        let id = r.str16("id", 64)?;
        let sign_key = r.array::<32>()?;
        let count = usize::from(r.u8()?);
        if count > MAX_PEERS {
            return Err(too_large("peers", count, MAX_PEERS));
        }
        let mut peers = Vec::with_capacity(count);
        for _ in 0..count {
            peers.push(r.str16("peer id", 64)?);
        }
        let limits = TicketLimits {
            bytes_per_sec: r.u32()?,
            frames_per_sec: r.u32()?,
            channels: r.u16()?,
            account_conns: r.u16()?,
        };
        let flags = r.u8()?;
        let issued_ms = r.u64()?;
        let expires_ms = r.u64()?;
        r.finish()?;
        Ok(Self {
            kid,
            relay,
            account,
            role,
            id,
            sign_key,
            peers,
            limits,
            flags,
            issued_ms,
            expires_ms,
        })
    }

    /// The ticket as it travels: base64url(encoding ‖ signature).
    pub fn sign(&self, key: &SigningKey) -> Result<String, DecodeError> {
        let body = self.encode()?;
        let signature = sig::sign(key, RELAY_TICKET, &body);
        Ok(base64url::encode(&[body.as_slice(), &signature].concat()))
    }

    pub fn throttled(&self) -> bool {
        self.flags & FLAG_THROTTLED != 0
    }

    pub fn emergency(&self) -> bool {
        self.flags & FLAG_EMERGENCY != 0
    }
}

/// A ticket off the wire, its signature not yet checked.
#[derive(Debug, Clone)]
pub struct SignedTicket {
    pub ticket: RelayTicket,
    body: Vec<u8>,
    signature: [u8; 64],
}

/// The longest ticket text a relay reads.
const MAX_TICKET_TEXT: usize = 8 * 1024;

impl SignedTicket {
    pub fn parse(text: &str) -> Result<Self, TicketError> {
        if text.len() > MAX_TICKET_TEXT {
            return Err(TicketError::Malformed("too long".into()));
        }
        let raw = base64url::decode(text).ok_or_else(|| TicketError::Malformed("not base64url".into()))?;
        if raw.len() < 64 {
            return Err(TicketError::Malformed("too short".into()));
        }
        let (body, signature) = raw.split_at(raw.len() - 64);
        let ticket = RelayTicket::decode(body).map_err(|e| TicketError::Malformed(e.to_string()))?;
        Ok(Self {
            ticket,
            body: body.to_vec(),
            signature: signature.try_into().expect("64 bytes"),
        })
    }

    /// Check the signature under the key `key_of(kid)` returns, that the
    /// ticket is for `relay`, that it has not expired at `now_ms`, and that
    /// it runs no longer than [`MAX_TICKET_LIFETIME_MS`] from an issue time
    /// no further than [`TICKET_CLOCK_SKEW_MS`] past `now_ms`: whatever its
    /// issuer wrote, a ticket is good for at most their sum from now.
    pub fn verify(self, key_of: impl Fn(u8) -> Option<[u8; 32]>, relay: &str, now_ms: u64) -> Result<RelayTicket, TicketError> {
        let key = key_of(self.ticket.kid).ok_or(TicketError::UnknownKey(self.ticket.kid))?;
        if !sig::verify(&key, RELAY_TICKET, &self.body, &self.signature) {
            return Err(TicketError::BadSignature);
        }
        if self.ticket.relay != relay {
            return Err(TicketError::OtherRelay(self.ticket.relay));
        }
        if now_ms >= self.ticket.expires_ms {
            return Err(TicketError::Expired);
        }
        if self.ticket.expires_ms.saturating_sub(self.ticket.issued_ms) > MAX_TICKET_LIFETIME_MS {
            return Err(TicketError::TooLong);
        }
        if self.ticket.issued_ms > now_ms.saturating_add(TICKET_CLOCK_SKEW_MS) {
            return Err(TicketError::IssuedAhead);
        }
        Ok(self.ticket)
    }
}

/// The body a relay signs to prove it answers at `origin`: `GET
/// <origin>/v1/relay-id?nonce=…` (§2.5), under [`sig::RELAY_ORIGIN`].
pub fn origin_proof_body(origin: &str, nonce: &[u8; 32]) -> Result<Vec<u8>, DecodeError> {
    if origin.len() > 256 {
        return Err(too_large("relay origin", origin.len(), 256));
    }
    let mut out = Vec::with_capacity(origin.len() + 34);
    put_str16(&mut out, origin);
    out.extend_from_slice(nonce);
    Ok(out)
}

/// A relay's proof that it answers at `origin`.
pub fn sign_origin_proof(key: &SigningKey, origin: &str, nonce: &[u8; 32]) -> Result<[u8; 64], DecodeError> {
    Ok(sig::sign(key, sig::RELAY_ORIGIN, &origin_proof_body(origin, nonce)?))
}

pub fn verify_origin_proof(key: &[u8; 32], origin: &str, nonce: &[u8; 32], signature: &[u8; 64]) -> bool {
    origin_proof_body(origin, nonce)
        .map(|body| sig::verify(key, sig::RELAY_ORIGIN, &body, signature))
        .unwrap_or(false)
}

/// RFC 4648 §5 base64url without padding.
pub mod base64url {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            let chars = chunk.len() + 1;
            for i in 0..chars {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            }
        }
        out
    }

    /// `None` for padding, any character outside the alphabet, a length no
    /// encoding has, or non-zero trailing bits (a second spelling).
    pub fn decode(text: &str) -> Option<Vec<u8>> {
        if text.len() % 4 == 1 {
            return None;
        }
        let mut out = Vec::with_capacity(text.len() * 3 / 4);
        let mut buffer: u32 = 0;
        let mut bits = 0;
        for c in text.bytes() {
            let value = ALPHABET.iter().position(|&a| a == c)? as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buffer >> bits) & 0xff) as u8);
            }
        }
        if buffer & ((1 << bits) - 1) != 0 {
            return None;
        }
        Some(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> RelayTicket {
        RelayTicket {
            kid: 1,
            relay: "sin-1".into(),
            account: "a-1".into(),
            role: TicketRole::Device,
            id: "d-1".into(),
            sign_key: [9; 32],
            peers: vec!["m-1".into(), "m-2".into()],
            limits: TicketLimits {
                bytes_per_sec: 4 * 1024 * 1024,
                frames_per_sec: 1000,
                channels: 64,
                account_conns: 48,
            },
            flags: 0,
            issued_ms: 1_760_000_000_000,
            expires_ms: 1_760_000_000_000 + TICKET_TTL_MS,
        }
    }

    #[test]
    fn a_ticket_round_trips_and_refuses_junk() {
        let t = sample();
        let bytes = t.encode().unwrap();
        assert_eq!(RelayTicket::decode(&bytes).unwrap(), t);
        let mut long = bytes.clone();
        long.push(0);
        assert!(RelayTicket::decode(&long).is_err(), "trailing bytes");
        let mut v2 = bytes.clone();
        v2[0] = 2;
        assert!(RelayTicket::decode(&v2).is_err(), "unknown version");
        assert!(RelayTicket::decode(&bytes[..bytes.len() - 1]).is_err());
        let many = RelayTicket {
            peers: (0..65).map(|i| format!("m-{i}")).collect(),
            ..t.clone()
        };
        assert!(many.encode().is_err());
    }

    #[test]
    fn a_ticket_verifies_only_under_its_key_for_its_relay_until_it_expires() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let public = key.verifying_key().to_bytes();
        let t = sample();
        let text = t.sign(&key).unwrap();
        let keys = |kid: u8| (kid == 1).then_some(public);
        let now = t.issued_ms + 1;
        assert_eq!(SignedTicket::parse(&text).unwrap().verify(keys, "sin-1", now).unwrap(), t);
        assert_eq!(
            SignedTicket::parse(&text).unwrap().verify(keys, "nrt-1", now),
            Err(TicketError::OtherRelay("sin-1".into()))
        );
        assert_eq!(
            SignedTicket::parse(&text).unwrap().verify(keys, "sin-1", t.expires_ms),
            Err(TicketError::Expired)
        );
        assert_eq!(
            SignedTicket::parse(&text).unwrap().verify(|_| None, "sin-1", now),
            Err(TicketError::UnknownKey(1))
        );
        let other = SigningKey::from_bytes(&[4; 32]).verifying_key().to_bytes();
        assert_eq!(
            SignedTicket::parse(&text).unwrap().verify(|_| Some(other), "sin-1", now),
            Err(TicketError::BadSignature)
        );
        // A flipped bit anywhere in the body breaks the signature.
        let mut raw = base64url::decode(&text).unwrap();
        raw[5] ^= 1;
        let tampered = base64url::encode(&raw);
        assert!(SignedTicket::parse(&tampered).map_or(true, |s| s.verify(keys, "sin-1", now).is_err()));
        // A ticket signature is no other signature: the same body under the
        // hub-login label does not verify.
        let body = t.encode().unwrap();
        let as_login = sig::sign(&key, sig::HUB_LOGIN, &body);
        let forged = base64url::encode(&[body.as_slice(), &as_login].concat());
        assert_eq!(
            SignedTicket::parse(&forged).unwrap().verify(keys, "sin-1", now),
            Err(TicketError::BadSignature)
        );
    }

    #[test]
    fn a_ticket_runs_no_longer_than_the_issuers_ttl_from_no_later_than_now() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let public = key.verifying_key().to_bytes();
        let keys = |kid: u8| (kid == 1).then_some(public);
        let check = |t: &RelayTicket, now: u64| SignedTicket::parse(&t.sign(&key).unwrap()).unwrap().verify(keys, "sin-1", now);
        // What the Hub issues: `issued_ms = now`, `expires_ms = now + TICKET_TTL_MS`.
        let t = sample();
        assert_eq!(t.expires_ms - t.issued_ms, TICKET_TTL_MS);
        assert_eq!(check(&t, t.issued_ms), Ok(t.clone()));
        assert_eq!(check(&t, t.expires_ms - 1), Ok(t.clone()));
        // A relay whose clock is behind the Hub's, by up to the skew, takes it too.
        assert_eq!(check(&t, t.issued_ms - TICKET_CLOCK_SKEW_MS), Ok(t.clone()));
        assert_eq!(check(&t, t.issued_ms - TICKET_CLOCK_SKEW_MS - 1), Err(TicketError::IssuedAhead));
        // A millisecond longer than the Hub issues is refused, at any time in it.
        let long = RelayTicket {
            expires_ms: t.issued_ms + MAX_TICKET_LIFETIME_MS + 1,
            ..t.clone()
        };
        for now in [long.issued_ms, long.issued_ms + 1, long.expires_ms - 1] {
            assert_eq!(check(&long, now), Err(TicketError::TooLong), "{now}");
        }
        // A distant expiry does not pass by moving the issue time along with it.
        let far = RelayTicket {
            issued_ms: t.issued_ms + 30 * 24 * 60 * 60 * 1000,
            expires_ms: t.expires_ms + 30 * 24 * 60 * 60 * 1000,
            ..t.clone()
        };
        assert_eq!(check(&far, t.issued_ms), Err(TicketError::IssuedAhead));
        // A ticket that ran out is still told apart as expired.
        assert_eq!(check(&long, long.expires_ms), Err(TicketError::Expired));
    }

    #[test]
    fn an_origin_proof_binds_origin_and_nonce() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let public = key.verifying_key().to_bytes();
        let sig = sign_origin_proof(&key, "wss://sin-1.relay.example", &[1; 32]).unwrap();
        assert!(verify_origin_proof(&public, "wss://sin-1.relay.example", &[1; 32], &sig));
        assert!(!verify_origin_proof(&public, "wss://evil.example", &[1; 32], &sig));
        assert!(!verify_origin_proof(&public, "wss://sin-1.relay.example", &[2; 32], &sig));
    }

    #[test]
    fn base64url_round_trips_and_has_one_spelling() {
        for n in 0..40 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            let text = base64url::encode(&data);
            assert!(!text.contains('='));
            assert_eq!(base64url::decode(&text).unwrap(), data);
        }
        assert_eq!(base64url::encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url::encode(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64url::decode("Zm9vYg"), Some(b"foob".to_vec()));
        assert_eq!(base64url::decode("Zm9vYh"), None, "non-zero trailing bits");
        assert_eq!(base64url::decode("Zm9vY"), None, "no encoding is 5 characters");
        assert_eq!(base64url::decode("Zm9v+"), None);
        assert_eq!(base64url::decode("Zm9vYg=="), None, "unpadded only");
    }
}
