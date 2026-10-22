//! Key wrapping: how `UK` and `MK` reach a device or an agent without the hub
//! ever holding them — and without the hub being able to *substitute* them.
//!
//! - **Sealed from one key pair to another** ([`seal_to`] / [`open_from`]):
//!   sender-authenticated, in the manner of HPKE's Auth mode. A one-off X25519
//!   key and the sender's *static* key both agree a secret with the recipient,
//!   `kek = HKDF(salt = "rh/v1 seal-auth", ikm = X25519(e, R) ‖ X25519(S, R),
//!   info = e ‖ S ‖ R ‖ context)`, and AES-256-GCM seals the secret. Only the
//!   holder of `S` could have produced it, so a hub that seals its own `MK` to
//!   an agent (anonymous ECIES allowed exactly that) is refused: the recipient
//!   names the sender it expects, and [`open_from_chain`] accepts only a
//!   current full device of the verified chain.
//! - **Under a symmetric key** ([`wrap_under`]): `UK` over each machine's `MK`,
//!   and the recovery wrap key over each `UK` epoch.
//!   `kek = HKDF(salt = "rh/v1 wrap-sym", ikm = K, info = context)`.
//! - The **context** ([`SealContext`], [`context`]) names the purpose, the
//!   account, sender and recipient ids and the epoch, and is the AEAD's AAD as
//!   well as part of the KDF input: a sealed `MK` for one machine cannot be
//!   passed off as another's, as another epoch's, or as a `UK`.
//! - **Pairing** ([`pairing_tag`]): a new device shows a QR code with its keys
//!   and a one-time secret `s`; the device that scans it MACs everything it
//!   hands back (the account fingerprint, its own id and key, the sealed `UK`)
//!   with `s`, so the new device — which does not know the account yet — can
//!   tell the delivery came from whoever saw its screen, not from the hub.
//!
//! Prefer delivering `MK` to an agent *inside* the Noise channel from an
//! authorised device; sealing is for the mailbox, when the recipient is offline.
//!
//! Every step is a WebCrypto primitive, so a browser unwraps with `unwrapKey`
//! into a non-extractable `CryptoKey`.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{CryptoRng, RngCore};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::chain::{ChainError, ChainState};
use crate::codec::{put_str16, DecodeError, Reader};
use crate::keys::DhKeypair;

const AUTH_SALT: &[u8] = b"rh/v1 seal-auth";
const SYM_SALT: &[u8] = b"rh/v1 wrap-sym";
const SEAL_LABEL: &[u8] = b"rh/v1 seal\0";
const PAIR_LABEL: &[u8] = b"rh/v1 pair\0";
/// The largest secret worth wrapping; anything bigger is not a key.
pub const MAX_SECRET: usize = 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WrapError {
    #[error("the wrapped key does not open (wrong sender or recipient, wrong context, or tampered)")]
    Open,
    #[error("a key is of low order")]
    LowOrderKey,
    #[error("a {0}-byte secret is too large to wrap")]
    TooLarge(usize),
    #[error("the delivery is for {0:?}, not this recipient")]
    WrongRecipient(String),
    #[error("the delivery belongs to another account")]
    WrongAccount,
    #[error("the sender is not a current full device: {0}")]
    UnknownSender(ChainError),
    #[error("the key is not the one the chain committed: {0}")]
    Uncommitted(ChainError),
    #[error("the delivery is not a {0:?}")]
    WrongPurpose(Purpose),
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

/// What a sealed secret is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Purpose {
    /// One epoch of the user key, for a device.
    UserKey = 1,
    /// One epoch of a machine key, for that machine's agent (or a device).
    MachineKey = 2,
}

/// Who sealed what for whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealContext {
    pub purpose: Purpose,
    pub account: String,
    /// The sending device's id in the chain.
    pub sender: String,
    /// The recipient device's or machine's id.
    pub recipient: String,
    pub epoch: u32,
}

impl SealContext {
    /// `"rh/v1 seal\0" | purpose | str16 account | str16 sender | str16 recipient | u32 epoch`.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = SEAL_LABEL.to_vec();
        out.push(self.purpose as u8);
        put_str16(&mut out, &self.account);
        put_str16(&mut out, &self.sender);
        put_str16(&mut out, &self.recipient);
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out
    }
}

/// A secret sealed from one key pair to another: `ephemeral ‖ nonce ‖ ct`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedKey {
    pub ephemeral: [u8; 32],
    pub nonce: [u8; 12],
    pub ct: Vec<u8>,
}

/// A secret wrapped under a symmetric key: `nonce ‖ ct`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedKey {
    pub nonce: [u8; 12],
    pub ct: Vec<u8>,
}

impl SealedKey {
    pub fn encode(&self) -> Vec<u8> {
        [&self.ephemeral[..], &self.nonce, &self.ct].concat()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WrapError> {
        let mut r = Reader::new(bytes);
        let ephemeral = r.array()?;
        let nonce = r.array()?;
        let ct = r.rest().to_vec();
        if ct.len() < 16 {
            return Err(DecodeError::Truncated.into());
        }
        Ok(Self { ephemeral, nonce, ct })
    }
}

impl WrappedKey {
    pub fn encode(&self) -> Vec<u8> {
        [&self.nonce[..], &self.ct].concat()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WrapError> {
        let mut r = Reader::new(bytes);
        let nonce = r.array()?;
        let ct = r.rest().to_vec();
        if ct.len() < 16 {
            return Err(DecodeError::Truncated.into());
        }
        Ok(Self { nonce, ct })
    }
}

fn hkdf32(salt: &[u8], ikm: &[u8], info: &[&[u8]]) -> Zeroizing<[u8; 32]> {
    let mut out = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand_multi_info(info, out.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 length");
    out
}

fn aead_seal(kek: &[u8; 32], nonce: &[u8; 12], aad: &[u8], secret: &[u8]) -> Vec<u8> {
    Aes256Gcm::new_from_slice(kek)
        .expect("32-byte key")
        .encrypt(Nonce::from_slice(nonce), Payload { msg: secret, aad })
        .expect("AES-GCM encryption of a small buffer cannot fail")
}

fn aead_open(kek: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ct: &[u8]) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    Aes256Gcm::new_from_slice(kek)
        .expect("32-byte key")
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| WrapError::Open)
}

fn auth_kek(dh_e: &[u8; 32], dh_s: &[u8; 32], ephemeral: &[u8; 32], sender: &[u8; 32], recipient: &[u8; 32], ctx: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut ikm = Zeroizing::new([0u8; 64]);
    ikm[..32].copy_from_slice(dh_e);
    ikm[32..].copy_from_slice(dh_s);
    hkdf32(AUTH_SALT, ikm.as_ref(), &[ephemeral, sender, recipient, ctx])
}

/// Seal `secret` from `sender` (a device's static X25519 key) to `recipient`.
pub fn seal_to<R: RngCore + CryptoRng>(
    sender: &DhKeypair,
    recipient: &[u8; 32],
    ctx: &SealContext,
    secret: &[u8],
    rng: &mut R,
) -> Result<SealedKey, WrapError> {
    seal_raw(sender, recipient, &ctx.encode(), secret, rng)
}

fn seal_raw<R: RngCore + CryptoRng>(
    sender: &DhKeypair,
    recipient: &[u8; 32],
    ctx: &[u8],
    secret: &[u8],
    rng: &mut R,
) -> Result<SealedKey, WrapError> {
    if secret.len() > MAX_SECRET {
        return Err(WrapError::TooLarge(secret.len()));
    }
    let ephemeral = DhKeypair::generate(rng);
    let dh_e = ephemeral.agree(recipient).ok_or(WrapError::LowOrderKey)?;
    let dh_s = sender.agree(recipient).ok_or(WrapError::LowOrderKey)?;
    let kek = auth_kek(&dh_e, &dh_s, &ephemeral.public(), &sender.public(), recipient, ctx);
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    Ok(SealedKey {
        ephemeral: ephemeral.public(),
        nonce,
        ct: aead_seal(&kek, &nonce, ctx, secret),
    })
}

/// Open a secret that `sender` sealed to `recipient`.
pub fn open_from(recipient: &DhKeypair, sender: &[u8; 32], ctx: &SealContext, sealed: &SealedKey) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    open_raw(recipient, sender, &ctx.encode(), sealed)
}

fn open_raw(recipient: &DhKeypair, sender: &[u8; 32], ctx: &[u8], sealed: &SealedKey) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    let dh_e = recipient.agree(&sealed.ephemeral).ok_or(WrapError::LowOrderKey)?;
    let dh_s = recipient.agree(sender).ok_or(WrapError::LowOrderKey)?;
    let kek = auth_kek(&dh_e, &dh_s, &sealed.ephemeral, sender, &recipient.public(), ctx);
    aead_open(&kek, &sealed.nonce, ctx, &sealed.ct)
}

/// The seal-context purpose byte of a host's machine key sealed for *another*
/// device (chain v3: a device that is not an admin holds no `UK`, so an admin
/// hands it each host's committed `MK` epochs instead,
/// `docs/local-aggregator.md` §3.3).
pub const HOST_KEY_PURPOSE: u8 = 3;

/// Who sealed which host's machine key epoch for whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyContext {
    pub account: String,
    /// The sealing admin device.
    pub sender: String,
    /// The device it is for.
    pub recipient: String,
    /// The host whose `MK` this is.
    pub host: String,
    pub epoch: u32,
}

impl HostKeyContext {
    /// `"rh/v1 seal\0" | 3 | str16 account | str16 sender | str16 recipient | u32 epoch | str16 host`:
    /// [`SealContext`]'s layout with the host after it, under a purpose byte
    /// of its own, so it can be neither a `UK` nor the host's own `MK` delivery.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = SEAL_LABEL.to_vec();
        out.push(HOST_KEY_PURPOSE);
        put_str16(&mut out, &self.account);
        put_str16(&mut out, &self.sender);
        put_str16(&mut out, &self.recipient);
        out.extend_from_slice(&self.epoch.to_be_bytes());
        put_str16(&mut out, &self.host);
        out
    }
}

/// Seal host `ctx.host`'s machine key epoch `ctx.epoch` from admin device
/// `sender` to device `recipient` (its X25519 key).
pub fn seal_host_key<R: RngCore + CryptoRng>(
    sender: &DhKeypair,
    recipient: &[u8; 32],
    ctx: &HostKeyContext,
    key: &[u8],
    rng: &mut R,
) -> Result<SealedKey, WrapError> {
    seal_raw(sender, recipient, &ctx.encode(), key, rng)
}

/// Open a host's machine key sealed for device `me`: from a current *admin*
/// device of `chain`, and exactly the key the chain committed for that host
/// and epoch ([`ChainState::check_machine_key`]) — so neither the hub nor a
/// non-admin (nor a device removed since) can plant a key of its own.
pub fn open_host_key(
    recipient: &DhKeypair,
    me: &str,
    chain: &ChainState,
    ctx: &HostKeyContext,
    sealed: &SealedKey,
) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    if ctx.recipient != me {
        return Err(WrapError::WrongRecipient(ctx.recipient.clone()));
    }
    if ctx.account != chain.account() {
        return Err(WrapError::WrongAccount);
    }
    let sender = chain.full_device_key(&ctx.sender).map_err(WrapError::UnknownSender)?;
    let key = open_raw(recipient, &sender, &ctx.encode(), sealed)?;
    chain.check_machine_key(&ctx.host, ctx.epoch, &key).map_err(WrapError::Uncommitted)?;
    Ok(key)
}

/// A host key as the mailbox carries it (purpose [`MAIL_HOST_KEY`]):
/// `str16 host | sealed` — the mail row says the epoch, the blob which host.
pub fn encode_host_key_mail(host: &str, sealed: &SealedKey) -> Vec<u8> {
    let mut out = Vec::new();
    put_str16(&mut out, host);
    out.extend_from_slice(&sealed.encode());
    out
}

/// The host and the sealed key of a [`MAIL_HOST_KEY`] mail.
pub fn decode_host_key_mail(bytes: &[u8]) -> Result<(String, SealedKey), WrapError> {
    let mut r = Reader::new(bytes);
    let host = r.str16("host id", crate::chain::MAX_ID_LEN)?;
    let sealed = SealedKey::decode(r.rest())?;
    Ok((host, sealed))
}

/// Mailbox purposes (`docs/hub-api/`): a `UK` epoch for an admin device.
pub const MAIL_USER_KEY: u8 = 1;
/// A host's own `MK` epoch, for its agent.
pub const MAIL_MACHINE_KEY: u8 = 2;
/// A pairing's delivery.
pub const MAIL_PAIRING: u8 = 3;
/// A request for keys this device lacks (no secret in it). From an admin it
/// asks for `UK` epochs; from any other device, for `MK` epochs only — and is
/// answered with [`MAIL_HOST_KEY`] mail, never with a `UK`.
pub const MAIL_KEY_REQUEST: u8 = 4;
/// Another host's `MK` epoch for a device that is not an admin
/// ([`encode_host_key_mail`]).
pub const MAIL_HOST_KEY: u8 = 5;

/// Open a delivery addressed to `me`, accepting it only if its sender is a
/// current full device of `chain` — what an agent does with an `MK` from the
/// mailbox, and a device with a `UK`.
pub fn open_from_chain(
    recipient: &DhKeypair,
    me: &str,
    chain: &ChainState,
    ctx: &SealContext,
    sealed: &SealedKey,
) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    if ctx.recipient != me {
        return Err(WrapError::WrongRecipient(ctx.recipient.clone()));
    }
    if ctx.account != chain.account() {
        return Err(WrapError::WrongAccount);
    }
    let sender = chain.full_device_key(&ctx.sender).map_err(WrapError::UnknownSender)?;
    open_from(recipient, &sender, ctx, sealed)
}

/// Open a machine key delivered to agent `me`: from a current full device of
/// `chain` (as [`open_from_chain`]), sealed for [`Purpose::MachineKey`], and
/// exactly the key the chain committed for that epoch
/// ([`ChainState::check_machine_key`]). The commitment, not the sender, is
/// what makes it the account's key: a device removed since it sealed this, or
/// the hub replaying old mail, can hand over only a key the chain already
/// names — never one of its own, nor an epoch the chain has not reached.
pub fn open_machine_key(
    recipient: &DhKeypair,
    me: &str,
    chain: &ChainState,
    ctx: &SealContext,
    sealed: &SealedKey,
) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    if ctx.purpose != Purpose::MachineKey {
        return Err(WrapError::WrongPurpose(Purpose::MachineKey));
    }
    let key = open_from_chain(recipient, me, chain, ctx, sealed)?;
    chain.check_machine_key(me, ctx.epoch, &key).map_err(WrapError::Uncommitted)?;
    Ok(key)
}

/// The recipient a `UK` epoch is sealed to for the recovery package.
pub const RECOVERY_RECIPIENT: &str = "recovery";

/// Seal `UK` epoch `epoch` for the account's recovery package: from device
/// `sender_id` (its static key `sender`) to the X25519 twin of the recovery
/// key `rsk` the chain names ([`crate::chain::recovery_seal_public`]). Only the
/// holder of `RK` can open it; the sender needs neither `RK` nor `RSK`'s secret.
pub fn seal_for_recovery<R: RngCore + CryptoRng>(
    sender: &DhKeypair,
    sender_id: &str,
    account: &str,
    rsk: &[u8; 32],
    epoch: u32,
    user_key: &[u8],
    rng: &mut R,
) -> Result<SealedKey, WrapError> {
    let recipient = crate::chain::recovery_seal_public(rsk).ok_or(WrapError::LowOrderKey)?;
    let ctx = SealContext {
        purpose: Purpose::UserKey,
        account: account.into(),
        sender: sender_id.into(),
        recipient: RECOVERY_RECIPIENT.into(),
        epoch,
    };
    seal_to(sender, &recipient, &ctx, user_key, rng)
}

/// Open a recovery-package `UK` with the keys derived from `RK`: sealed by a
/// device the chain names — a current admin, or one demoted or removed since —
/// and exactly the key the chain committed for that epoch
/// ([`ChainState::check_user_key`]). The commitment, not the sender, makes it
/// the account's key: a device removed since still holds old user keys and
/// the public `RSK`, but it can seal only a key the chain already names,
/// never one of its own. And a package an admin no longer current sealed can
/// be the only way back in for its epoch, so the hub keeps one per sender and
/// epoch when it sweeps the keyring, and this opens it.
pub fn open_from_recovery(
    keys: &crate::chain::RecoveryKeys,
    chain: &ChainState,
    ctx: &SealContext,
    sealed: &SealedKey,
) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    if ctx.recipient != RECOVERY_RECIPIENT || ctx.purpose != Purpose::UserKey {
        return Err(WrapError::WrongRecipient(ctx.recipient.clone()));
    }
    if ctx.account != chain.account() {
        return Err(WrapError::WrongAccount);
    }
    let sender = chain
        .device_record(&ctx.sender)
        .map(|d| d.info.dh_key)
        .ok_or_else(|| WrapError::UnknownSender(crate::chain::ChainError::UnknownDevice(ctx.sender.clone())))?;
    let key = open_from(&keys.seal, &sender, ctx, sealed)?;
    chain.check_user_key(ctx.epoch, &key).map_err(WrapError::Uncommitted)?;
    Ok(key)
}

/// Open a `UK` epoch mailed to device `me` (purpose 1): from a current full
/// device ([`open_from_chain`]) and exactly the key the chain committed.
pub fn open_user_key(
    recipient: &DhKeypair,
    me: &str,
    chain: &ChainState,
    ctx: &SealContext,
    sealed: &SealedKey,
) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    if ctx.purpose != Purpose::UserKey {
        return Err(WrapError::WrongPurpose(Purpose::UserKey));
    }
    let key = open_from_chain(recipient, me, chain, ctx, sealed)?;
    chain.check_user_key(ctx.epoch, &key).map_err(WrapError::Uncommitted)?;
    Ok(key)
}

/// Wrap `secret` under the symmetric key `key`.
pub fn wrap_under<R: RngCore + CryptoRng>(key: &[u8; 32], context: &[u8], secret: &[u8], rng: &mut R) -> Result<WrappedKey, WrapError> {
    if secret.len() > MAX_SECRET {
        return Err(WrapError::TooLarge(secret.len()));
    }
    let kek = hkdf32(SYM_SALT, key, &[context]);
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    Ok(WrappedKey {
        nonce,
        ct: aead_seal(&kek, &nonce, context, secret),
    })
}

pub fn unwrap_under(key: &[u8; 32], context: &[u8], wrapped: &WrappedKey) -> Result<Zeroizing<Vec<u8>>, WrapError> {
    let kek = hkdf32(SYM_SALT, key, &[context]);
    aead_open(&kek, &wrapped.nonce, context, &wrapped.ct)
}

/// Contexts for symmetric wrapping. Every field is NUL-separated or fixed-width;
/// ids are printable ASCII ([`crate::chain`]).
pub mod context {
    /// One epoch of a machine's `MK`, wrapped under one epoch of `UK`.
    pub fn machine_key_under_user_key(account: &str, machine: &str, mk_epoch: u32, uk_epoch: u32) -> Vec<u8> {
        [
            &b"rh/v1 MK under UK\0"[..],
            account.as_bytes(),
            b"\0",
            machine.as_bytes(),
            b"\0",
            &mk_epoch.to_be_bytes(),
            &uk_epoch.to_be_bytes(),
        ]
        .concat()
    }

    /// One epoch of `UK`, wrapped under the recovery wrap key.
    pub fn user_key_under_recovery(account: &str, uk_epoch: u32) -> Vec<u8> {
        [&b"rh/v1 UK under RK\0"[..], account.as_bytes(), b"\0", &uk_epoch.to_be_bytes()].concat()
    }
}

/// The pairing MAC: `HMAC-SHA256(s, "rh/v1 pair\0" ‖ (u32 len ‖ field)*)`.
pub fn pairing_tag(s: &[u8; 32], fields: &[&[u8]]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(s).expect("HMAC takes any key length");
    mac.update(PAIR_LABEL);
    for field in fields {
        mac.update(&(field.len() as u32).to_be_bytes());
        mac.update(field);
    }
    mac.finalize().into_bytes().into()
}

/// Constant-time check of a pairing MAC.
pub fn verify_pairing_tag(s: &[u8; 32], fields: &[&[u8]], tag: &[u8; 32]) -> bool {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(s).expect("HMAC takes any key length");
    mac.update(PAIR_LABEL);
    for field in fields {
        mac.update(&(field.len() as u32).to_be_bytes());
        mac.update(field);
    }
    mac.verify_slice(tag).is_ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::chain::tests::{fixture, h, sample_chain};
    use crate::testing::{bytes32, rng};

    fn ctx() -> SealContext {
        SealContext {
            purpose: Purpose::MachineKey,
            account: "acct-1".into(),
            sender: "dev-0".into(),
            recipient: "m-1".into(),
            epoch: 3,
        }
    }

    #[test]
    fn a_sealed_key_opens_only_from_its_sender_for_its_recipient_and_context() {
        let device = DhKeypair::from_secret(bytes32(1));
        let agent = DhKeypair::from_secret(bytes32(2));
        let other = DhKeypair::from_secret(bytes32(3));
        let mk = [0x5a; 32];
        let sealed = seal_to(&device, &agent.public(), &ctx(), &mk, &mut rng(1)).unwrap();
        assert_eq!(*open_from(&agent, &device.public(), &ctx(), &sealed).unwrap(), mk.to_vec());
        assert_eq!(
            open_from(&other, &device.public(), &ctx(), &sealed),
            Err(WrapError::Open),
            "another recipient"
        );
        assert_eq!(
            open_from(&agent, &other.public(), &ctx(), &sealed),
            Err(WrapError::Open),
            "another claimed sender"
        );
        for changed in [
            SealContext { epoch: 4, ..ctx() },
            SealContext {
                recipient: "m-2".into(),
                ..ctx()
            },
            SealContext {
                sender: "dev-1".into(),
                ..ctx()
            },
            SealContext {
                purpose: Purpose::UserKey,
                ..ctx()
            },
            SealContext {
                account: "acct-2".into(),
                ..ctx()
            },
        ] {
            assert_eq!(
                open_from(&agent, &device.public(), &changed, &sealed),
                Err(WrapError::Open),
                "{changed:?}"
            );
        }
        let mut tampered = sealed.clone();
        tampered.ct[0] ^= 1;
        assert_eq!(open_from(&agent, &device.public(), &ctx(), &tampered), Err(WrapError::Open));
        assert_eq!(SealedKey::decode(&sealed.encode()).unwrap(), sealed);
        assert_eq!(seal_to(&device, &[0; 32], &ctx(), &mk, &mut rng(1)), Err(WrapError::LowOrderKey));
    }

    #[test]
    fn an_agent_accepts_deliveries_only_from_current_full_devices_of_its_chain() {
        let f = fixture();
        let chain = crate::chain::ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let agent = &f.machine.1;
        let mk = [7u8; 32];
        let good = seal_to(&f.devices[0].1, &agent.public(), &ctx(), &mk, &mut rng(2)).unwrap();
        assert_eq!(*open_from_chain(agent, "m-1", &chain, &ctx(), &good).unwrap(), mk.to_vec());
        assert!(matches!(
            open_from_chain(agent, "m-2", &chain, &ctx(), &good),
            Err(WrapError::WrongRecipient(_))
        ));
        // dev-1 was removed and dev-2 is a limited device.
        for (i, id) in [(1, "dev-1"), (2, "dev-2")] {
            let c = SealContext { sender: id.into(), ..ctx() };
            let sealed = seal_to(&f.devices[i].1, &agent.public(), &c, &mk, &mut rng(3)).unwrap();
            assert!(
                matches!(open_from_chain(agent, "m-1", &chain, &c, &sealed), Err(WrapError::UnknownSender(_))),
                "{id}"
            );
        }
        // A delivery claiming dev-0 but sealed by someone else.
        let impostor = DhKeypair::from_secret(bytes32(66));
        let forged = seal_to(&impostor, &agent.public(), &ctx(), &mk, &mut rng(4)).unwrap();
        assert_eq!(open_from_chain(agent, "m-1", &chain, &ctx(), &forged), Err(WrapError::Open));
    }

    /// The sample chain with user key epochs `uks` and m-1 key epochs `mks` committed.
    pub(crate) fn committed(f: &crate::chain::tests::Fixture, uks: &[[u8; 32]], mks: &[[u8; 32]]) -> ChainState {
        use crate::chain::{machine_key_commit, user_key_commit, Action, KeyCommit, KeyCommits, Signer};
        let mut chain = ChainState::verify(&sample_chain(f), None, &mut h()).unwrap();
        for i in 0..uks.len().max(mks.len()) {
            let k = KeyCommits {
                user: uks.get(i).map(|k| KeyCommit {
                    epoch: i as u32,
                    hash: user_key_commit("acct-1", i as u32, k),
                }),
                machines: mks
                    .get(i)
                    .map(|k| {
                        (
                            "m-1".to_string(),
                            KeyCommit {
                                epoch: i as u32,
                                hash: machine_key_commit("acct-1", "m-1", i as u32, k),
                            },
                        )
                    })
                    .into_iter()
                    .collect(),
            };
            let e = chain.draft(Signer::Device("dev-0".into()), Action::KeyEpochs(k), 0).sign(&f.devices[0].0);
            chain.apply(&e, &mut h()).unwrap();
        }
        chain
    }

    #[test]
    fn an_agent_takes_only_machine_keys_the_chain_committed() {
        let f = fixture();
        let chain = committed(&f, &[], &[[0; 32], [1; 32]]);
        let agent = &f.machine.1;
        let deliver = |from: usize, sender: &str, epoch: u32, key: [u8; 32]| {
            let c = SealContext {
                epoch,
                sender: sender.into(),
                ..ctx()
            };
            (
                c.clone(),
                seal_to(&f.devices[from].1, &agent.public(), &c, &key, &mut rng(epoch.into())).unwrap(),
            )
        };
        for (epoch, key) in [(0, [0; 32]), (1, [1; 32])] {
            let (c, s) = deliver(0, "dev-0", epoch, key);
            assert_eq!(*open_machine_key(agent, "m-1", &chain, &c, &s).unwrap(), key.to_vec());
        }
        // A key of the sender's choosing, an epoch out of reach: refused.
        let (c, s) = deliver(0, "dev-0", 1, [7; 32]);
        assert!(matches!(
            open_machine_key(agent, "m-1", &chain, &c, &s),
            Err(WrapError::Uncommitted(ChainError::KeyMismatch { .. }))
        ));
        let (c, s) = deliver(0, "dev-0", u32::MAX, [7; 32]);
        assert!(matches!(
            open_machine_key(agent, "m-1", &chain, &c, &s),
            Err(WrapError::Uncommitted(ChainError::UncommittedKey { .. }))
        ));
        // The removed dev-1, even with the committed key.
        let (c, s) = deliver(1, "dev-1", 1, [1; 32]);
        assert!(matches!(open_machine_key(agent, "m-1", &chain, &c, &s), Err(WrapError::UnknownSender(_))));
        let uk = SealContext {
            purpose: Purpose::UserKey,
            ..ctx()
        };
        let sealed = seal_to(&f.devices[0].1, &agent.public(), &uk, &[1; 32], &mut rng(7)).unwrap();
        assert_eq!(
            open_machine_key(agent, "m-1", &chain, &uk, &sealed),
            Err(WrapError::WrongPurpose(Purpose::MachineKey))
        );
    }

    #[test]
    fn a_wrapped_key_opens_only_under_its_key_and_context() {
        let uk = [0x11; 32];
        let c = context::machine_key_under_user_key("acct-1", "m-1", 0, 2);
        let wrapped = wrap_under(&uk, &c, &[0x22; 32], &mut rng(2)).unwrap();
        assert_eq!(*unwrap_under(&uk, &c, &wrapped).unwrap(), vec![0x22; 32]);
        assert_eq!(unwrap_under(&[0x12; 32], &c, &wrapped), Err(WrapError::Open));
        assert_eq!(
            unwrap_under(&uk, &context::machine_key_under_user_key("acct-1", "m-1", 0, 3), &wrapped),
            Err(WrapError::Open)
        );
        assert_eq!(WrappedKey::decode(&wrapped.encode()).unwrap(), wrapped);
    }

    #[test]
    fn the_recovery_package_opens_only_with_rk_from_a_current_device_and_the_committed_key() {
        let f = fixture();
        let uk = [0x44u8; 32];
        let chain = committed(&f, &[[0x40; 32], uk], &[]);
        let rsk = chain.recovery_key().unwrap();
        // Any device can compute where to seal from the chain's RSK alone.
        assert_eq!(crate::chain::recovery_seal_public(&rsk), Some(f.recovery.seal.public()));
        let sealed = seal_for_recovery(&f.devices[0].1, "dev-0", "acct-1", &rsk, 1, &uk, &mut rng(1)).unwrap();
        let ctx = SealContext {
            purpose: Purpose::UserKey,
            account: "acct-1".into(),
            sender: "dev-0".into(),
            recipient: RECOVERY_RECIPIENT.into(),
            epoch: 1,
        };
        assert_eq!(*open_from_recovery(&f.recovery, &chain, &ctx, &sealed).unwrap(), uk);
        // Another RK, another epoch, a device that never was full: refused.
        let other = crate::chain::recovery_keys(&[9u8; 16]);
        assert_eq!(open_from_recovery(&other, &chain, &ctx, &sealed), Err(WrapError::Open));
        assert_eq!(
            open_from_recovery(&f.recovery, &chain, &SealContext { epoch: 0, ..ctx.clone() }, &sealed),
            Err(WrapError::Open)
        );
        // Named as another device of the chain: its key does not open dev-0's seal.
        assert_eq!(
            open_from_recovery(
                &f.recovery,
                &chain,
                &SealContext {
                    sender: "dev-2".into(),
                    ..ctx.clone()
                },
                &sealed
            ),
            Err(WrapError::Open)
        );
        // A sender the chain never named.
        assert!(matches!(
            open_from_recovery(
                &f.recovery,
                &chain,
                &SealContext {
                    sender: "dev-9".into(),
                    ..ctx.clone()
                },
                &sealed
            ),
            Err(WrapError::UnknownSender(_))
        ));
        // A current device sealing a key the chain never committed.
        let planted = seal_for_recovery(&f.devices[0].1, "dev-0", "acct-1", &rsk, 1, &[0xAA; 32], &mut rng(3)).unwrap();
        assert!(matches!(
            open_from_recovery(&f.recovery, &chain, &ctx, &planted),
            Err(WrapError::Uncommitted(_))
        ));
        // M2 review: the removed dev-1 cannot plant a key of its
        // own; a package of the committed key it sealed before still opens.
        let open_by_removed = |key: [u8; 32]| {
            let by_removed = seal_for_recovery(&f.devices[1].1, "dev-1", "acct-1", &rsk, 1, &key, &mut rng(2)).unwrap();
            open_from_recovery(
                &f.recovery,
                &chain,
                &SealContext {
                    sender: "dev-1".into(),
                    ..ctx.clone()
                },
                &by_removed,
            )
        };
        assert!(matches!(open_by_removed([0xAA; 32]), Err(WrapError::Uncommitted(_))));
        assert_eq!(*open_by_removed(uk).unwrap(), uk);
    }

    #[test]
    fn the_pairing_tag_binds_every_field_and_their_boundaries() {
        let s = [9u8; 32];
        let tag = pairing_tag(&s, &[b"acct", b"dev-0"]);
        assert!(verify_pairing_tag(&s, &[b"acct", b"dev-0"], &tag));
        assert!(!verify_pairing_tag(&s, &[b"acctd", b"ev-0"], &tag), "field boundaries are bound");
        assert!(!verify_pairing_tag(&[8; 32], &[b"acct", b"dev-0"], &tag));
    }
}
