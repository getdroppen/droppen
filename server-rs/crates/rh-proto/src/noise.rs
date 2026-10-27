//! The end-to-end channel: Noise IK between a device (initiator) and an agent
//! (responder), carried in the payloads of the hub's `DATA` frames.
//!
//! # Suite
//!
//! `Noise_IK_25519_AESGCM_SHA256`. Every primitive in it is native to WebCrypto
//! (X25519, AES-256-GCM, HMAC-SHA-256), so the browser half needs no wasm and
//! keeps its static key a non-extractable `CryptoKey` while in use (at rest
//! it is sealed PKCS #8 that code in the page's origin can open); the agent
//! uses `snow`.
//!
//! # Handshake
//!
//! IK is one round trip: the device already knows the agent's static key (from
//! the device chain), so its first message is encrypted to it. The **prologue**
//! binds the session to `rh/v1 | account | device | machine` ([`Binding`]), so a
//! hub cannot splice sessions between the wrong parties. The chain state is
//! *not* in the prologue — an exact-equality binding let a hub that withheld a
//! removal from the agent lock out the honest device that signed it, while the
//! removed one, sharing the stale view, got in. Instead each side's chain view
//! travels inside the encrypted handshake payloads, and the agent catches up or
//! raises the alarm before it authorises anyone ([`crate::handshake`]).
//!
//! The first message's payload has no forward secrecy and is open to key
//! compromise impersonation: anyone holding the *agent's* static key can forge a
//! first message from any device. It is also replayable by anyone who saw it.
//! So it carries no request at all — only the chain view and client metadata
//! ([`crate::handshake::HelloBody`]); anything that changes state waits for the
//! first transport message, which requires the device's own static key. The
//! payload starts with the sender's clock, and the agent keeps a
//! [`ReplayGuard`].
//!
//! # Transport
//!
//! After the handshake each direction has its own key and an implicit nonce, so
//! a dropped, replayed or reordered message fails to decrypt — and any failure
//! poisons the channel, since a peer that has seen one forgery is talking to an
//! attacker and has nothing to gain from carrying on.
//!
//! Each transport plaintext starts with a flags byte. [`FLAG_REKEY`] says "I
//! rekey my sending key after this message": the sender sets it after
//! [`RekeyPolicy::max_messages`] messages or [`RekeyPolicy::max_age_ms`]
//! (2^20 messages or an hour by default), and the receiver rekeys the matching
//! receiving key once it has decrypted it. A receiver that has taken
//! `max_messages` without one refuses the next, so the policy is enforced rather
//! than merely suggested. Rekeying is Noise's own `REKEY()` (§4.2), so it gives
//! forward secrecy *within* a long-lived channel.

use std::collections::HashMap;

use snow::{Builder, HandshakeState, TransportState};
use thiserror::Error;

use crate::codec::{put_str16, MAX_ID_LEN};
use crate::frame::MAX_PAYLOAD_LEN;
use crate::keys::{x25519_is_low_order, DhKeypair};

/// The Noise protocol name.
pub const PATTERN: &str = "Noise_IK_25519_AESGCM_SHA256";
/// The prologue's leading label.
const PROLOGUE_LABEL: &[u8] = b"rh/v1\0";
/// The AEAD tag.
pub const TAG_LEN: usize = 16;
/// The flags byte of a transport plaintext.
pub const FLAG_REKEY: u8 = 0x01;
/// The most application data one transport message carries: a Noise message
/// must fit in a hub `DATA` frame, and spends a tag and the flags byte.
pub const MAX_TRANSPORT_DATA: usize = MAX_PAYLOAD_LEN - TAG_LEN - 1;
/// The most a handshake payload may carry (the first message also spends the
/// ephemeral key, the encrypted static key and the timestamp).
pub const MAX_HELLO_PAYLOAD: usize = MAX_PAYLOAD_LEN - 32 - (32 + TAG_LEN) - 8 - TAG_LEN;
/// How far a first message's clock may be from the responder's, either way.
pub const DEFAULT_PAST_WINDOW_MS: u64 = 10 * 60 * 1000;
/// How far ahead of the agent's clock a first message's timestamp may be.
pub const DEFAULT_FUTURE_WINDOW_MS: u64 = 60 * 1000;
/// How many first messages one device may have in the guard at once.
pub const DEFAULT_PER_DEVICE: usize = 256;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChannelError {
    #[error("noise: {0}")]
    Noise(String),
    #[error("a message failed to decrypt (forged, replayed, reordered or dropped)")]
    Decrypt,
    #[error("the channel already failed and must be re-established")]
    Poisoned,
    #[error("a binding id is empty or longer than {MAX_ID_LEN} bytes")]
    BadBinding,
    #[error("the peer offered a low-order X25519 key")]
    LowOrderKey,
    #[error("malformed message: {0}")]
    Malformed(&'static str),
    #[error("{0} bytes is more than one message can carry")]
    TooLarge(usize),
    #[error("handshake replayed or stale")]
    Replay,
    #[error("the peer went past the rekey limit without rekeying")]
    RekeyOverdue,
}

impl From<snow::Error> for ChannelError {
    fn from(error: snow::Error) -> Self {
        match error {
            snow::Error::Decrypt => ChannelError::Decrypt,
            other => ChannelError::Noise(other.to_string()),
        }
    }
}

/// Who a channel is between, as both ends must agree it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub account: String,
    pub device: String,
    pub machine: String,
}

impl Binding {
    /// `"rh/v1\0" | str16 account | str16 device | str16 machine`.
    pub fn prologue(&self) -> Result<Vec<u8>, ChannelError> {
        for id in [&self.account, &self.device, &self.machine] {
            if id.is_empty() || id.len() > MAX_ID_LEN {
                return Err(ChannelError::BadBinding);
            }
        }
        let mut out = Vec::with_capacity(PROLOGUE_LABEL.len() + 3 * 66 + 40);
        out.extend_from_slice(PROLOGUE_LABEL);
        put_str16(&mut out, &self.account);
        put_str16(&mut out, &self.device);
        put_str16(&mut out, &self.machine);
        Ok(out)
    }
}

/// When a sender must rekey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyPolicy {
    pub max_messages: u64,
    pub max_age_ms: u64,
}

impl Default for RekeyPolicy {
    fn default() -> Self {
        Self {
            max_messages: 1 << 20,
            max_age_ms: 60 * 60 * 1000,
        }
    }
}

fn builder<'a>(params: &'a snow::params::NoiseParams, local: &'a DhKeypair, prologue: &'a [u8]) -> Result<Builder<'a>, ChannelError> {
    Ok(Builder::new(params.clone()).local_private_key(local.secret())?.prologue(prologue)?)
}

fn params() -> snow::params::NoiseParams {
    PATTERN.parse().expect("a valid noise pattern")
}

/// The device's half of the handshake.
pub struct Initiator {
    hs: HandshakeState,
}

impl Initiator {
    /// Start a handshake with the agent whose static key is `responder`.
    pub fn new(local: &DhKeypair, responder: &[u8; 32], binding: &Binding) -> Result<Self, ChannelError> {
        Self::build(local, responder, &binding.prologue()?, None)
    }

    /// The same handshake under another prologue (the direct path's,
    /// [`crate::path::DirectBinding::prologue`]).
    pub fn with_prologue(local: &DhKeypair, responder: &[u8; 32], prologue: &[u8]) -> Result<Self, ChannelError> {
        Self::build(local, responder, prologue, None)
    }

    /// Only for producing test vectors another implementation must match byte for byte.
    #[cfg(test)]
    pub(crate) fn with_fixed_ephemeral(local: &DhKeypair, responder: &[u8; 32], binding: &Binding, e: &[u8; 32]) -> Result<Self, ChannelError> {
        Self::build(local, responder, &binding.prologue()?, Some(e))
    }

    /// [`Initiator::with_prologue`] with a fixed ephemeral, for vectors.
    #[cfg(test)]
    pub(crate) fn with_prologue_and_ephemeral(local: &DhKeypair, responder: &[u8; 32], prologue: &[u8], e: &[u8; 32]) -> Result<Self, ChannelError> {
        Self::build(local, responder, prologue, Some(e))
    }

    fn build(local: &DhKeypair, responder: &[u8; 32], prologue: &[u8], e: Option<&[u8; 32]>) -> Result<Self, ChannelError> {
        if x25519_is_low_order(responder) {
            return Err(ChannelError::LowOrderKey);
        }
        let params = params();
        let mut b = builder(&params, local, prologue)?.remote_public_key(responder)?;
        if let Some(e) = e {
            b = b.fixed_ephemeral_key_for_testing_only(e);
        }
        Ok(Self { hs: b.build_initiator()? })
    }

    /// The first message: `e, es, s, ss` and `timestamp_ms ‖ payload`.
    pub fn write_hello(&mut self, timestamp_ms: u64, payload: &[u8]) -> Result<Vec<u8>, ChannelError> {
        if payload.len() > MAX_HELLO_PAYLOAD {
            return Err(ChannelError::TooLarge(payload.len()));
        }
        let mut body = Vec::with_capacity(8 + payload.len());
        body.extend_from_slice(&timestamp_ms.to_be_bytes());
        body.extend_from_slice(payload);
        let mut out = vec![0u8; MAX_PAYLOAD_LEN];
        let n = self.hs.write_message(&body, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// The agent's reply: `e, ee, se` and its payload. Completes the handshake.
    pub fn read_reply(mut self, message: &[u8], policy: RekeyPolicy, now_ms: u64) -> Result<(Transport, Vec<u8>), ChannelError> {
        check_ephemeral(message, 32 + TAG_LEN)?;
        let mut payload = vec![0u8; message.len()];
        let n = self.hs.read_message(message, &mut payload)?;
        payload.truncate(n);
        Ok((Transport::new(self.hs, policy, now_ms)?, payload))
    }
}

/// A first message, as the agent sees it once it has decrypted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The device's static key. The caller must check it against the device
    /// chain ([`crate::chain::ChainState::authorize_device`]) before trusting
    /// anything else here.
    pub initiator_static: [u8; 32],
    pub ephemeral: [u8; 32],
    pub timestamp_ms: u64,
    pub payload: Vec<u8>,
}

/// The agent's half of the handshake.
pub struct Responder {
    hs: HandshakeState,
}

impl Responder {
    pub fn new(local: &DhKeypair, binding: &Binding) -> Result<Self, ChannelError> {
        Self::build(local, &binding.prologue()?, None)
    }

    /// The same handshake under another prologue (the direct path's).
    pub fn with_prologue(local: &DhKeypair, prologue: &[u8]) -> Result<Self, ChannelError> {
        Self::build(local, prologue, None)
    }

    #[cfg(test)]
    pub(crate) fn with_fixed_ephemeral(local: &DhKeypair, binding: &Binding, e: &[u8; 32]) -> Result<Self, ChannelError> {
        Self::build(local, &binding.prologue()?, Some(e))
    }

    /// [`Responder::with_prologue`] with a fixed ephemeral, for vectors.
    #[cfg(test)]
    pub(crate) fn with_prologue_and_ephemeral(local: &DhKeypair, prologue: &[u8], e: &[u8; 32]) -> Result<Self, ChannelError> {
        Self::build(local, prologue, Some(e))
    }

    fn build(local: &DhKeypair, prologue: &[u8], e: Option<&[u8; 32]>) -> Result<Self, ChannelError> {
        let params = params();
        let mut b = builder(&params, local, prologue)?;
        if let Some(e) = e {
            b = b.fixed_ephemeral_key_for_testing_only(e);
        }
        Ok(Self { hs: b.build_responder()? })
    }

    pub fn read_hello(&mut self, message: &[u8]) -> Result<Hello, ChannelError> {
        check_ephemeral(message, 32 + 32 + TAG_LEN + 8 + TAG_LEN)?;
        let mut body = vec![0u8; message.len()];
        let n = self.hs.read_message(message, &mut body)?;
        if n < 8 {
            return Err(ChannelError::Malformed("first message without a timestamp"));
        }
        let initiator_static: [u8; 32] = self
            .hs
            .get_remote_static()
            .and_then(|s| s.try_into().ok())
            .ok_or(ChannelError::Malformed("no static key"))?;
        if x25519_is_low_order(&initiator_static) {
            return Err(ChannelError::LowOrderKey);
        }
        let mut ephemeral = [0u8; 32];
        ephemeral.copy_from_slice(&message[..32]);
        Ok(Hello {
            initiator_static,
            ephemeral,
            timestamp_ms: u64::from_be_bytes(body[..8].try_into().expect("8 bytes")),
            payload: body[8..n].to_vec(),
        })
    }

    pub fn write_reply(mut self, payload: &[u8], policy: RekeyPolicy, now_ms: u64) -> Result<(Vec<u8>, Transport), ChannelError> {
        if payload.len() > MAX_HELLO_PAYLOAD {
            return Err(ChannelError::TooLarge(payload.len()));
        }
        let mut out = vec![0u8; MAX_PAYLOAD_LEN];
        let n = self.hs.write_message(payload, &mut out)?;
        out.truncate(n);
        Ok((out, Transport::new(self.hs, policy, now_ms)?))
    }
}

fn check_ephemeral(message: &[u8], min_len: usize) -> Result<(), ChannelError> {
    if message.len() < min_len {
        return Err(ChannelError::Malformed("handshake message too short"));
    }
    if message.len() > MAX_PAYLOAD_LEN {
        return Err(ChannelError::TooLarge(message.len()));
    }
    let e: [u8; 32] = message[..32].try_into().expect("32 bytes");
    if x25519_is_low_order(&e) {
        return Err(ChannelError::LowOrderKey);
    }
    Ok(())
}

/// Refuses first messages that are stale, from the future, from before this
/// process could know what it had accepted, or seen before.
///
/// - An entry is kept until its *timestamp* is too old to be accepted again
///   (`timestamp + past window`), not merely for a window after it arrived — a
///   future-dated message must not become replayable when its entry expires.
/// - A guard starts with no memory, so a fresh one refuses every timestamp a
///   previous process could have accepted: anything before `not_before`
///   ([`ReplayGuard::after_restart`]). Persisting the guard
///   ([`ReplayGuard::snapshot`] / [`ReplayGuard::restore`]) avoids the short
///   refusal window that costs.
/// - Entries are kept per device and checked only *after* the device has been
///   authorised against the chain, so a device can fill only its own quota.
pub struct ReplayGuard {
    past_ms: u64,
    future_ms: u64,
    per_device: usize,
    not_before_ms: u64,
    seen: HashMap<String, HashMap<[u8; 32], u64>>,
}

impl ReplayGuard {
    pub fn new(past_ms: u64, future_ms: u64, per_device: usize, not_before_ms: u64) -> Self {
        Self {
            past_ms,
            future_ms,
            per_device,
            not_before_ms,
            seen: HashMap::new(),
        }
    }

    /// A guard for a process that started at `now_ms` with no saved state: a
    /// previous process may have accepted anything stamped up to `now_ms +
    /// future window`, so nothing before that is accepted here.
    pub fn after_restart(now_ms: u64) -> Self {
        Self::new(
            DEFAULT_PAST_WINDOW_MS,
            DEFAULT_FUTURE_WINDOW_MS,
            DEFAULT_PER_DEVICE,
            now_ms.saturating_add(DEFAULT_FUTURE_WINDOW_MS),
        )
    }

    /// Accept `hello` from (already authorised) `device` once.
    pub fn check(&mut self, device: &str, hello: &Hello, now_ms: u64) -> Result<(), ChannelError> {
        let ts = hello.timestamp_ms;
        if ts < self.not_before_ms || ts.saturating_add(self.past_ms) < now_ms || ts > now_ms.saturating_add(self.future_ms) {
            return Err(ChannelError::Replay);
        }
        let seen = self.seen.entry(device.to_string()).or_default();
        seen.retain(|_, &mut expires| expires >= now_ms);
        if seen.contains_key(&hello.ephemeral) || seen.len() >= self.per_device {
            return Err(ChannelError::Replay);
        }
        seen.insert(hello.ephemeral, ts.saturating_add(self.past_ms));
        Ok(())
    }

    /// Forget a device (it was removed).
    pub fn forget(&mut self, device: &str) {
        self.seen.remove(device);
    }

    /// Everything still remembered, as `(device, ephemeral, expires_ms)`, to persist.
    pub fn snapshot(&self) -> Vec<(String, [u8; 32], u64)> {
        self.seen
            .iter()
            .flat_map(|(d, m)| m.iter().map(move |(e, &x)| (d.clone(), *e, x)))
            .collect()
    }

    /// A guard restored from a snapshot needs no `not_before` beyond the old process's.
    pub fn restore(&mut self, entries: impl IntoIterator<Item = (String, [u8; 32], u64)>) {
        for (device, e, expires) in entries {
            self.seen.entry(device).or_default().insert(e, expires);
        }
    }
}

/// An established channel.
pub struct Transport {
    state: TransportState,
    policy: RekeyPolicy,
    sent: u64,
    received: u64,
    last_rekey_ms: u64,
    poisoned: bool,
    handshake_hash: [u8; 32],
    remote_static: [u8; 32],
}

impl Transport {
    fn new(hs: HandshakeState, policy: RekeyPolicy, now_ms: u64) -> Result<Self, ChannelError> {
        let handshake_hash: [u8; 32] = hs
            .get_handshake_hash()
            .try_into()
            .map_err(|_| ChannelError::Malformed("handshake hash"))?;
        let remote_static: [u8; 32] = hs
            .get_remote_static()
            .and_then(|s| s.try_into().ok())
            .ok_or(ChannelError::Malformed("no remote static"))?;
        Ok(Self {
            state: hs.into_transport_mode()?,
            policy,
            sent: 0,
            received: 0,
            last_rekey_ms: now_ms,
            poisoned: false,
            handshake_hash,
            remote_static,
        })
    }

    /// Unique to this handshake; what a later short-code comparison would bind to.
    pub fn handshake_hash(&self) -> [u8; 32] {
        self.handshake_hash
    }

    pub fn remote_static(&self) -> [u8; 32] {
        self.remote_static
    }

    /// Encrypt one message.
    pub fn seal(&mut self, now_ms: u64, data: &[u8]) -> Result<Vec<u8>, ChannelError> {
        if self.poisoned {
            return Err(ChannelError::Poisoned);
        }
        if data.len() > MAX_TRANSPORT_DATA {
            return Err(ChannelError::TooLarge(data.len()));
        }
        let rekey = self.sent + 1 >= self.policy.max_messages || now_ms.saturating_sub(self.last_rekey_ms) >= self.policy.max_age_ms;
        let mut plaintext = Vec::with_capacity(1 + data.len());
        plaintext.push(if rekey { FLAG_REKEY } else { 0 });
        plaintext.extend_from_slice(data);
        let mut out = vec![0u8; plaintext.len() + TAG_LEN];
        let n = self.state.write_message(&plaintext, &mut out).inspect_err(|_| self.poisoned = true)?;
        out.truncate(n);
        if rekey {
            self.state.rekey_outgoing();
            self.sent = 0;
            self.last_rekey_ms = now_ms;
        } else {
            self.sent += 1;
        }
        Ok(out)
    }

    /// Decrypt one message, which must be the very next one the peer sent.
    pub fn open(&mut self, message: &[u8]) -> Result<Vec<u8>, ChannelError> {
        if self.poisoned {
            return Err(ChannelError::Poisoned);
        }
        let result = self.open_inner(message);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn open_inner(&mut self, message: &[u8]) -> Result<Vec<u8>, ChannelError> {
        if message.len() < 1 + TAG_LEN {
            return Err(ChannelError::Malformed("transport message too short"));
        }
        let mut plaintext = vec![0u8; message.len()];
        let n = self.state.read_message(message, &mut plaintext)?;
        plaintext.truncate(n);
        let flags = plaintext[0];
        if flags & !FLAG_REKEY != 0 {
            return Err(ChannelError::Malformed("unknown transport flags"));
        }
        if flags & FLAG_REKEY != 0 {
            self.state.rekey_incoming();
            self.received = 0;
        } else {
            self.received += 1;
            if self.received >= self.policy.max_messages {
                return Err(ChannelError::RekeyOverdue);
            }
        }
        plaintext.remove(0);
        Ok(plaintext)
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::bytes32;

    pub(crate) fn binding() -> Binding {
        Binding {
            account: "acct-1".into(),
            device: "dev-1".into(),
            machine: "m-1".into(),
        }
    }

    fn keys() -> (DhKeypair, DhKeypair) {
        (DhKeypair::from_secret(bytes32(1)), DhKeypair::from_secret(bytes32(2)))
    }

    fn handshake(policy: RekeyPolicy) -> (Transport, Transport) {
        let (device, agent) = keys();
        let mut i = Initiator::new(&device, &agent.public(), &binding()).unwrap();
        let mut r = Responder::new(&agent, &binding()).unwrap();
        let m1 = i.write_hello(1_000, b"first request").unwrap();
        let hello = r.read_hello(&m1).unwrap();
        assert_eq!(hello.payload, b"first request");
        assert_eq!(hello.initiator_static, device.public());
        assert_eq!(hello.timestamp_ms, 1_000);
        let (m2, rt) = r.write_reply(b"features", policy, 1_000).unwrap();
        let (it, reply) = i.read_reply(&m2, policy, 1_000).unwrap();
        assert_eq!(reply, b"features");
        assert_eq!(it.handshake_hash(), rt.handshake_hash());
        (it, rt)
    }

    #[test]
    fn a_handshake_completes_and_messages_flow_both_ways() {
        let (mut i, mut r) = handshake(RekeyPolicy::default());
        for n in 0..5 {
            let m = i.seal(2_000, format!("ping {n}").as_bytes()).unwrap();
            assert_eq!(r.open(&m).unwrap(), format!("ping {n}").as_bytes());
            let m = r.seal(2_000, b"pong").unwrap();
            assert_eq!(i.open(&m).unwrap(), b"pong");
        }
    }

    #[test]
    fn a_mismatched_binding_fails_the_handshake() {
        let (device, agent) = keys();
        for other in [
            Binding {
                device: "dev-2".into(),
                ..binding()
            },
            Binding {
                machine: "m-2".into(),
                ..binding()
            },
            Binding {
                account: "acct-2".into(),
                ..binding()
            },
        ] {
            let mut i = Initiator::new(&device, &agent.public(), &binding()).unwrap();
            let mut r = Responder::new(&agent, &other).unwrap();
            let m1 = i.write_hello(1, b"x").unwrap();
            assert_eq!(r.read_hello(&m1), Err(ChannelError::Decrypt), "{other:?}");
        }
    }

    #[test]
    fn the_wrong_responder_key_cannot_read_the_first_message() {
        let (device, agent) = keys();
        let impostor = DhKeypair::from_secret(bytes32(3));
        let mut i = Initiator::new(&device, &agent.public(), &binding()).unwrap();
        let mut r = Responder::new(&impostor, &binding()).unwrap();
        assert_eq!(r.read_hello(&i.write_hello(1, b"secret").unwrap()), Err(ChannelError::Decrypt));
    }

    #[test]
    fn tampered_replayed_reordered_and_dropped_messages_are_rejected() {
        // Tamper.
        let (mut i, mut r) = handshake(RekeyPolicy::default());
        let mut m = i.seal(0, b"a").unwrap();
        m[3] ^= 0x40;
        assert_eq!(r.open(&m), Err(ChannelError::Decrypt));
        assert_eq!(
            r.open(&i.seal(0, b"b").unwrap()),
            Err(ChannelError::Poisoned),
            "one forgery ends the channel"
        );

        // Replay.
        let (mut i, mut r) = handshake(RekeyPolicy::default());
        let m = i.seal(0, b"a").unwrap();
        r.open(&m).unwrap();
        assert_eq!(r.open(&m), Err(ChannelError::Decrypt));

        // Reorder.
        let (mut i, mut r) = handshake(RekeyPolicy::default());
        let a = i.seal(0, b"a").unwrap();
        let b = i.seal(0, b"b").unwrap();
        assert_eq!(r.open(&b), Err(ChannelError::Decrypt));
        let _ = a;

        // Drop.
        let (mut i, mut r) = handshake(RekeyPolicy::default());
        let _dropped = i.seal(0, b"a").unwrap();
        assert_eq!(r.open(&i.seal(0, b"b").unwrap()), Err(ChannelError::Decrypt));

        // A message from one direction cannot be reflected back.
        let (mut i, _r) = handshake(RekeyPolicy::default());
        let m = i.seal(0, b"a").unwrap();
        assert_eq!(i.open(&m), Err(ChannelError::Decrypt));
    }

    #[test]
    fn rekeying_by_count_and_by_age_keeps_both_sides_in_step() {
        let policy = RekeyPolicy {
            max_messages: 3,
            max_age_ms: 10_000,
        };
        let (mut i, mut r) = handshake(policy);
        let mut flags = Vec::new();
        for n in 0..10u64 {
            let now = if n >= 7 { 20_000 + n } else { 1_000 + n };
            let m = i.seal(now, &[n as u8]).unwrap();
            flags.push(i.sent == 0);
            assert_eq!(r.open(&m).unwrap(), vec![n as u8]);
        }
        // The 3rd and 6th by count, the 8th by age (the clock jumped), and the
        // count restarts from each.
        assert_eq!(flags, [false, false, true, false, false, true, false, true, false, false]);
    }

    #[test]
    fn a_peer_that_never_rekeys_is_cut_off() {
        let policy = RekeyPolicy {
            max_messages: 3,
            max_age_ms: u64::MAX,
        };
        let (mut i, mut r) = handshake(RekeyPolicy {
            max_messages: 100,
            max_age_ms: u64::MAX,
        });
        r.policy = policy;
        r.open(&i.seal(0, b"1").unwrap()).unwrap();
        r.open(&i.seal(0, b"2").unwrap()).unwrap();
        assert_eq!(r.open(&i.seal(0, b"3").unwrap()), Err(ChannelError::RekeyOverdue));
    }

    fn hello_at(ts: u64) -> Hello {
        let (device, agent) = keys();
        let m1 = Initiator::new(&device, &agent.public(), &binding())
            .unwrap()
            .write_hello(ts, b"")
            .unwrap();
        Responder::new(&agent, &binding()).unwrap().read_hello(&m1).unwrap()
    }

    #[test]
    fn the_replay_guard_refuses_copies_stale_and_future_clocks_and_overflowing_devices() {
        let mut guard = ReplayGuard::new(60_000, 10_000, 2, 0);
        let hello = hello_at(100_000);
        guard.check("dev-1", &hello, 100_500).unwrap();
        assert_eq!(guard.check("dev-1", &hello, 101_000), Err(ChannelError::Replay), "a second copy");
        assert_eq!(guard.check("dev-1", &hello_at(1), 100_000), Err(ChannelError::Replay), "stale");
        assert_eq!(
            guard.check("dev-1", &hello_at(120_000), 100_000),
            Err(ChannelError::Replay),
            "too far ahead"
        );
        guard.check("dev-1", &hello_at(100_001), 100_500).unwrap();
        assert_eq!(
            guard.check("dev-1", &hello_at(100_002), 100_500),
            Err(ChannelError::Replay),
            "dev-1's quota is full"
        );
        guard.check("dev-2", &hello_at(100_002), 100_500).unwrap();
        // Entries live until their timestamp is too old to accept again.
        assert_eq!(guard.check("dev-1", &hello, 160_000), Err(ChannelError::Replay));
        // Persisted and restored, the memory survives a restart.
        let mut restored = ReplayGuard::new(60_000, 10_000, 2, 0);
        restored.restore(guard.snapshot());
        assert_eq!(restored.check("dev-1", &hello, 101_000), Err(ChannelError::Replay));
    }

    #[test]
    fn oversize_messages_and_low_order_keys_are_refused() {
        let (mut i, _r) = handshake(RekeyPolicy::default());
        assert!(i.seal(0, &vec![0; MAX_TRANSPORT_DATA]).is_ok());
        assert_eq!(
            i.seal(0, &vec![0; MAX_TRANSPORT_DATA + 1]),
            Err(ChannelError::TooLarge(MAX_TRANSPORT_DATA + 1))
        );
        let (device, agent) = keys();
        assert!(matches!(Initiator::new(&device, &[0; 32], &binding()), Err(ChannelError::LowOrderKey)));
        let mut forged = Initiator::new(&device, &agent.public(), &binding()).unwrap().write_hello(1, b"").unwrap();
        forged[..32].fill(0);
        assert_eq!(
            Responder::new(&agent, &binding()).unwrap().read_hello(&forged),
            Err(ChannelError::LowOrderKey)
        );
    }
}
