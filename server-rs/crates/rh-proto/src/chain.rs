//! The device chain: the signed, append-only list of the devices that belong
//! to an account.
//!
//! It is the root of every trust decision the hub cannot be allowed to make:
//! an agent accepts a Noise handshake only from a device key the chain lists
//! (and only from one its host accepts control from), accepts a key delivery
//! only from an admin device the chain lists, and a device encrypts only to a
//! machine key the chain lists. The hub stores and serves the chain and can
//! neither add to it nor (undetectably, see below) take from it.
//!
//! # One kind of member: the device (chain v3)
//!
//! Every endpoint is a **device** with one identity (an Ed25519 signing key and
//! an X25519 key) and two flags (`docs/local-aggregator.md` §3):
//!
//! - **host** (可运行 AI): it runs an agent, can be connected to, and has a
//!   machine key `MK` per epoch for its streams. Declared when it joins and
//!   signed in by an admin; [`Action::SetHost`] only stops or restarts it.
//! - **admin** (可管理账号): it signs entries, holds the user key `UK`, attests
//!   and delivers keys. The one switch ([`Action::SetAdmin`]); nobody grants
//!   it to themselves, and the last admin can neither lose it nor be removed.
//!
//! Every device may *control* (open a channel to a host), subject to the
//! host's [`Accept`] policy: by default a host accepts control only from admin
//! devices; host-to-host (or any other non-admin) control is allowed only when
//! an admin says so with [`Action::SetAccept`] ([`ChainState::may_control`]).
//!
//! # Two encodings, one state
//!
//! Entries carry a version byte, which the signature covers. Version 2 is the
//! older model — devices (`full` or limited) and machines as two kinds — and
//! version 3 the one above. A chain is a v2 prefix and then, once its first v3
//! entry lands, v3 entries only ([`ChainState::is_v3`]). A verifier maps v2
//! entries onto the v3 state deterministically: a full device (genesis, an
//! added full device, a recovered one) is an admin; a limited device is a
//! plain device; a machine is a host that is not an admin. Replaying a v2
//! chain gives the same devices, the same machines (= active hosts) and the
//! same errors as before.
//!
//! **No way back.** Once a chain holds a v3 entry, a verifier that reads only
//! v2 refuses the whole chain: readers must never be rolled back below the
//! first release that reads v3 (the hub's `DROPPEN_HUB_CHAIN_V3` gate exists so
//! that no account gets a v3 entry before all of its hosts read them).
//!
//! # Entries
//!
//! Each entry names its account, its `seq` (0 for genesis, then +1 each), the
//! hash of the entry before it, a timestamp, who signed it and one action. It is
//! a fixed binary layout ([`Draft::encode`]) — never JSON, so there is exactly
//! one byte string to sign and hash — signed with Ed25519 under the
//! `rh/v1 chain-entry` label ([`crate::sig`]).
//!
//! # Who may sign what ([`ChainState::apply`])
//!
//! - **Genesis** (`Genesis`, `Genesis3`) is the only entry the account key `AK`
//!   ever signs. `AK` is the account's root *identity* — the fingerprint every
//!   machine pins at enrollment — not a standing authority: the device that
//!   creates the account signs genesis and then destroys `AK`.
//! - Ordinary entries are signed by a **current, active admin device** with
//!   its own key. Within a batch, entries take effect in order: an admin that
//!   demoted itself signs nothing after that.
//! - The **recovery key** `RSK` signs exactly two things: `RecoveryJoin` (a
//!   new *admin* device, which starts **pending**) and `ActivateRecovered`.
//! - A **passkey slot** (`Passkey(slot)`, registered by an admin with
//!   `PasskeySlotSet`) signs exactly two things: a pending `AddDevice3` with no
//!   flags, and that device's `PasskeyActivate` once the verifier has known
//!   about the join for [`PASSKEY_DELAY_MS`] (`docs/passkeys.md` §4.4).
//! - A pending device can neither sign entries nor be authorised by an agent.
//!   It becomes active when an admin signs `ApproveDevice`, or when it
//!   activates itself after the delay ([`ChainClock`]).
//! - Only a host has machine keys: `KeyEpochs` commits `MK` epochs only for
//!   current hosts, and a host that joins (or restarts hosting) must have its
//!   first new `MK` epoch committed in the very next entry.
//! - Ids are never reused; keys must be canonical, valid and not of small
//!   order; unknown flag bits and non-canonical lists are refused.
//! - A current host renews its identity key with [`Action::RotateHostKey`]
//!   (`docs/host-identity-rotation.md` §3.4): an admin signs the entry, the
//!   host's current key and its new key sign the proof inside it
//!   ([`host_rotate_body`]), the new keys are ones the chain never named, and
//!   its next machine key epoch comes in the very next entry. The id and
//!   everything the chain says about it stay; the old keys are kept as
//!   [`RetiredKeys`] ([`ChainState::keys_at`]) and never name a device again
//!   ([`rules::KEY_RETIRED`]).
//!
//! # Rollback and freezing
//!
//! A verifier remembers the highest `(seq, hash)` it has seen ([`Checkpoint`])
//! and refuses a chain that is shorter or different there. A hub can still
//! *withhold* new entries from one verifier; two things bound that: a device's
//! handshake carries its own chain view and the entries the agent may lack
//! ([`crate::handshake`]), and admin devices sign [`Attestation`]s of the head
//! they see, so an agent can tell whether anyone else has vouched for its view
//! recently ([`ChainState::freshness`]) — within [`CONTROL_FRESHNESS_MS`] to
//! be controlled at all, within [`MANAGE_FRESHNESS_MS`] for anything that
//! changes keys or who it is.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::codec::{check_u64, put_str16, DecodeError, Reader};
use crate::keys::{ed25519_key, x25519_valid};
use crate::sig;

/// The v2 entry encoding's version byte (recovery keys carry their proof, and
/// key epochs are committed in the chain).
pub const VERSION: u8 = 2;
/// The v3 entry encoding's version byte: one kind of device, with flags.
pub const VERSION_3: u8 = 3;
/// What every entry hash covers before the entry.
pub const HASH_LABEL: &[u8] = b"rh/v1 chain-hash\0";
/// The longest account, device, machine or passkey slot id.
pub const MAX_ID_LEN: usize = 64;
/// How long a verifier must have known about a recovery-join before it accepts
/// the recovered device activating itself.
pub const RECOVERY_DELAY_MS: u64 = 24 * 60 * 60 * 1000;
/// How long a verifier must have known about a passkey join before it accepts
/// the device activating itself with its slot.
pub const PASSKEY_DELAY_MS: u64 = 24 * 60 * 60 * 1000;
/// How far in the future an attestation's clock may be.
pub const ATTESTATION_SKEW_MS: u64 = 5 * 60 * 1000;
/// What a key commitment hashes under.
pub const KEY_COMMIT_LABEL: &[u8] = b"rh/v1 key-commit\0";
/// The most machines one `KeyEpochs` entry may move on.
pub const MAX_KEY_COMMITS: usize = 256;
/// The most passkey slots an account may have at once.
pub const MAX_PASSKEY_SLOTS: usize = 8;
/// The most devices one `SetAccept` may name.
pub const MAX_ACCEPT_IDS: usize = 64;
/// How recently an admin must have vouched for a host's chain head for it to
/// accept control at all (a channel, sessions, terminals, files).
pub const CONTROL_FRESHNESS_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// How recently an admin must have vouched for a host's chain head for it to
/// accept a management request (its machine key, its name, re-encryption).
pub const MANAGE_FRESHNESS_MS: u64 = 12 * 60 * 60 * 1000;

/// What a request asks a host to do, for the freshness it needs
/// (`docs/local-aggregator.md` §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Open a channel; use sessions, terminals, files: [`CONTROL_FRESHNESS_MS`].
    Control,
    /// Change keys or the host's identity: [`MANAGE_FRESHNESS_MS`].
    Manage,
}

impl Operation {
    pub fn max_age_ms(self) -> u64 {
        match self {
            Operation::Control => CONTROL_FRESHNESS_MS,
            Operation::Manage => MANAGE_FRESHNESS_MS,
        }
    }
}

/// A recovery key `RSK` as the chain holds it, with `RSK`'s own signature over
/// the account and its root key ([`recovery_proof_body`]). Without the proof
/// a hub could build a chain of its own around the account's (public) `RSK`
/// and pass it off on a device recovering with the words; with it, only
/// someone holding `RK` can put this `RSK` into a chain, and only into the
/// account whose root `AK` it signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryKey {
    pub key: [u8; 32],
    pub proof: [u8; 64],
}

impl RecoveryKey {
    /// Sign the proof for `account` (root key `account_key`) with `rsk`.
    pub fn new(rsk: &SigningKey, account: &str, account_key: &[u8; 32]) -> Self {
        let key = rsk.verifying_key().to_bytes();
        Self {
            key,
            proof: sig::sign(rsk, sig::RECOVERY_PROOF, &recovery_proof_body(account, account_key, &key)),
        }
    }

    /// Whether the proof is `key`'s signature over this account and root.
    pub fn verify(&self, account: &str, account_key: &[u8; 32]) -> bool {
        sig::verify(
            &self.key,
            sig::RECOVERY_PROOF,
            &recovery_proof_body(account, account_key, &self.key),
            &self.proof,
        )
    }
}

/// What a recovery key's proof signs: `str16 account | AK 32 | RSK 32`.
pub fn recovery_proof_body(account: &str, account_key: &[u8; 32], rsk: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + account.len() + 64);
    put_str16(&mut out, account);
    out.extend_from_slice(account_key);
    out.extend_from_slice(rsk);
    out
}

/// A passkey slot as `PasskeySlotSet` registers it (`docs/passkeys.md` §4.2):
/// the passkey authority key `PAK`, derived from the passkey's PRF output, with
/// `PAK`'s own proof over the account, its root `AK`, the slot, the credential
/// and the salt — so only someone who can evaluate the PRF can put this `PAK`
/// into a chain, and only into this account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasskeySlot {
    /// `"p-" + 12 random characters`, never reused.
    pub slot: String,
    /// `PAK`: Ed25519.
    pub key: [u8; 32],
    pub proof: [u8; 64],
    /// `SHA-256(credential id)`.
    pub cred_hash: [u8; 32],
    /// The PRF salt's random part (`docs/passkeys.md` §4.2).
    pub salt_nonce: [u8; 32],
    /// What a device joining with the slot may become: must be 0 (a plain
    /// device; passkeys never grant admin, `docs/passkeys.md` §4.10). Kept so
    /// the encoding need not change if that ever does.
    pub grants: u8,
    /// Whether a device joined with the slot may activate itself after
    /// [`PASSKEY_DELAY_MS`] (else only an admin's approval activates it).
    pub self_activate: bool,
}

impl PasskeySlot {
    /// Whether the proof is `key`'s signature over this account, root, slot,
    /// credential and salt.
    pub fn verify(&self, account: &str, account_key: &[u8; 32]) -> bool {
        sig::verify(
            &self.key,
            sig::PASSKEY_PROOF,
            &passkey_proof_body(account, account_key, &self.key, &self.slot, &self.cred_hash, &self.salt_nonce),
            &self.proof,
        )
    }
}

/// What a passkey slot's proof signs:
/// `str16 account | AK 32 | PAK 32 | str16 slot | cred_hash 32 | salt_nonce 32`.
pub fn passkey_proof_body(account: &str, account_key: &[u8; 32], pak: &[u8; 32], slot: &str, cred_hash: &[u8; 32], salt_nonce: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + account.len() + slot.len() + 160);
    put_str16(&mut out, account);
    out.extend_from_slice(account_key);
    out.extend_from_slice(pak);
    put_str16(&mut out, slot);
    out.extend_from_slice(cred_hash);
    out.extend_from_slice(salt_nonce);
    out
}

/// What the HMAC that proves a host holds its new DH key covers after the key
/// (`docs/host-identity-rotation.md` §3.2): [`rotate_confirm_mac`].
pub const ROTATE_CONFIRM_LABEL: &[u8] = b"rh/v1 rotate-confirm\0";
/// What the high-assurance code hashes under (§3.7): [`rotate_sas`].
pub const ROTATE_SAS_LABEL: &[u8] = b"rh/v1 rotate-sas\0";

/// How a host's identity renewal was confirmed (`RotateHostKey`'s last byte
/// before the signatures; any other value does not decode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Assurance {
    /// 0: automatic — the admin's channel to the host's current key, the
    /// hub's window (§3.3).
    #[default]
    Auto,
    /// 1: confirmed on the device itself — the code it showed was typed in
    /// on the admin device (§3.7).
    OnDevice,
}

impl Assurance {
    pub fn to_byte(self) -> u8 {
        match self {
            Assurance::Auto => 0,
            Assurance::OnDevice => 1,
        }
    }

    pub fn from_byte(b: u8) -> Result<Self, DecodeError> {
        match b {
            0 => Ok(Assurance::Auto),
            1 => Ok(Assurance::OnDevice),
            _ => Err(DecodeError::Invalid("renewal assurance")),
        }
    }
}

/// `RotateHostKey` (chain v3, kind 20): a current host renews its identity
/// key — both halves, sign and DH — and keeps its id and everything the chain
/// says about it (`docs/host-identity-rotation.md` §3.4). An admin signs the
/// entry; the host's current key **and** its new key sign the proof
/// ([`host_rotate_body`]), so neither an admin alone nor a holder of the old
/// key alone can rebind the id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRotation {
    pub host: String,
    /// The new Ed25519 key.
    pub sign_key: [u8; 32],
    /// The new X25519 key.
    pub dh_key: [u8; 32],
    /// The chain view the proof was made against: `staged_head` is the hash
    /// of entry `staged_seq`.
    pub staged_seq: u64,
    pub staged_head: [u8; 32],
    /// The admin's request nonce.
    pub nonce: [u8; 16],
    pub assurance: Assurance,
    /// The host's current sign key over the proof.
    pub old_sig: [u8; 64],
    /// The new sign key over the same proof.
    pub new_sig: [u8; 64],
}

impl HostRotation {
    /// The proof this renewal's two signatures cover, for a host whose keys
    /// on the chain are `old_sign` / `old_dh`.
    pub fn body(&self, account: &str, old_sign: &[u8; 32], old_dh: &[u8; 32]) -> Vec<u8> {
        host_rotate_body(
            account,
            &self.host,
            self.staged_seq,
            &self.staged_head,
            old_sign,
            old_dh,
            &self.sign_key,
            &self.dh_key,
            &self.nonce,
            self.assurance,
        )
    }

    /// Whether `old_sig` is `old_sign`'s and `new_sig` the new key's
    /// signature over the proof: rule 5 of §3.4 (`ROTATE_PROOF`).
    pub fn verify(&self, account: &str, old_sign: &[u8; 32], old_dh: &[u8; 32]) -> bool {
        let body = self.body(account, old_sign, old_dh);
        sig::verify(old_sign, sig::HOST_ROTATE, &body, &self.old_sig) && sig::verify(&self.sign_key, sig::HOST_ROTATE, &body, &self.new_sig)
    }
}

/// What both of a host's identity keys sign when it renews them
/// (`rh/v1 host-rotate`, §3.2):
/// `str16 account | str16 host | u64 staged_seq | staged_head 32 | old_sign 32
/// | old_dh 32 | new_sign 32 | new_dh 32 | nonce 16 | u8 assurance`.
#[allow(clippy::too_many_arguments)]
pub fn host_rotate_body(
    account: &str,
    host: &str,
    staged_seq: u64,
    staged_head: &[u8; 32],
    old_sign: &[u8; 32],
    old_dh: &[u8; 32],
    new_sign: &[u8; 32],
    new_dh: &[u8; 32],
    nonce: &[u8; 16],
    assurance: Assurance,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + account.len() + host.len() + 8 + 32 * 5 + 16 + 1);
    put_str16(&mut out, account);
    put_str16(&mut out, host);
    out.extend_from_slice(&staged_seq.to_be_bytes());
    out.extend_from_slice(staged_head);
    out.extend_from_slice(old_sign);
    out.extend_from_slice(old_dh);
    out.extend_from_slice(new_sign);
    out.extend_from_slice(new_dh);
    out.extend_from_slice(nonce);
    out.push(assurance.to_byte());
    out
}

/// `HMAC-SHA256(shared, "rh/v1 rotate-confirm\0" ‖ body)`: the proof that the
/// host holds the new DH key's private half, which no signature can show
/// (§3.2). `shared` is `X25519(new_dh, eph)` — the host's new DH secret with
/// the admin's ephemeral public key, or the admin's ephemeral secret with the
/// new DH public key ([`host_rotate_confirm`], [`check_rotate_confirm`]).
pub fn rotate_confirm_mac(shared: &[u8; 32], body: &[u8]) -> [u8; 32] {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(shared).expect("HMAC takes any key length");
    mac.update(ROTATE_CONFIRM_LABEL);
    mac.update(body);
    mac.finalize().into_bytes().into()
}

/// The host's side of [`rotate_confirm_mac`]: with its new DH key and the
/// admin's ephemeral public key. `None` when `eph` is of low order.
pub fn host_rotate_confirm(new_dh: &crate::keys::DhKeypair, eph: &[u8; 32], body: &[u8]) -> Option<[u8; 32]> {
    let shared = new_dh.agree(eph)?;
    Some(rotate_confirm_mac(&shared, body))
}

/// The admin's side: whether `mac` is [`rotate_confirm_mac`] under its
/// ephemeral key `eph` and the host's new DH public key `new_dh`.
pub fn check_rotate_confirm(eph: &crate::keys::DhKeypair, new_dh: &[u8; 32], body: &[u8], mac: &[u8; 32]) -> bool {
    use hmac::{Hmac, Mac};
    let Some(shared) = eph.agree(new_dh) else { return false };
    let mut check = <Hmac<Sha256> as Mac>::new_from_slice(&shared[..]).expect("HMAC takes any key length");
    check.update(ROTATE_CONFIRM_LABEL);
    check.update(body);
    check.verify_slice(mac).is_ok()
}

/// The code a host shows for a high-assurance renewal (§3.7): RFC 4648
/// base32 (upper case, no padding) of the first 5 bytes of
/// `SHA-256("rh/v1 rotate-sas\0" ‖ body)` — 40 bits, 8 characters — written
/// `XXXX-XXXX`.
pub fn rotate_sas(body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(ROTATE_SAS_LABEL);
    h.update(body);
    let digest = h.finalize();
    let text = crate::codec::base32::encode(&digest[..5]);
    format!("{}-{}", &text[..4], &text[4..])
}

/// What a host's new key signs to confirm a staged renewal on a connection
/// that signed in with the old one (`rh/v1 rotate-ack`, §3.3):
/// `str16 account | str16 host | SHA-256(proof body) 32 | login nonce 32` —
/// the nonce being the hub's challenge that connection answered
/// ([`crate::sig::HubLogin::nonce`]).
pub fn rotate_ack_body(account: &str, host: &str, proof_body: &[u8], login_nonce: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + account.len() + host.len() + 64);
    put_str16(&mut out, account);
    put_str16(&mut out, host);
    out.extend_from_slice(&Sha256::digest(proof_body));
    out.extend_from_slice(login_nonce);
    out
}

/// The new key's [`rotate_ack_body`] signature.
pub fn sign_rotate_ack(new_key: &SigningKey, account: &str, host: &str, proof_body: &[u8], login_nonce: &[u8; 32]) -> [u8; 64] {
    sig::sign(new_key, sig::ROTATE_ACK, &rotate_ack_body(account, host, proof_body, login_nonce))
}

/// Whether `signature` is `new_sign`'s [`rotate_ack_body`] signature.
pub fn verify_rotate_ack(new_sign: &[u8; 32], account: &str, host: &str, proof_body: &[u8], login_nonce: &[u8; 32], signature: &[u8; 64]) -> bool {
    sig::verify(new_sign, sig::ROTATE_ACK, &rotate_ack_body(account, host, proof_body, login_nonce), signature)
}

/// What a host's retired sign key signs to dispute the renewal that retired
/// it (`rh/v1 rotate-dispute`, `docs/host-identity-rotation.md` §3.8
/// 「证据：退役钥签的争议声明」): `str16 account | str16 host | u64 seq |
/// entry_hash 32 | new_sign 32` — `seq` the disputed `RotateHostKey`,
/// `entry_hash` its chain hash ([`Entry::hash`]), `new_sign` the sign key it
/// put in.
///
/// Bound to the entry's hash, a statement can only be signed once that entry
/// exists: no comparing of two parties' clocks, and nothing signed before the
/// renewal (a login, say) can be passed off as one.
pub fn rotate_dispute_body(account: &str, host: &str, seq: u64, entry_hash: &[u8; 32], new_sign: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + account.len() + host.len() + 8 + 64);
    put_str16(&mut out, account);
    put_str16(&mut out, host);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(entry_hash);
    out.extend_from_slice(new_sign);
    out
}

/// The retired key's [`rotate_dispute_body`] signature (`key`: the sign key
/// the renewal at `seq` retired, `keys_at(host, seq - 1)`).
pub fn sign_rotate_dispute(key: &SigningKey, account: &str, host: &str, seq: u64, entry_hash: &[u8; 32], new_sign: &[u8; 32]) -> [u8; 64] {
    sig::sign(key, sig::ROTATE_DISPUTE, &rotate_dispute_body(account, host, seq, entry_hash, new_sign))
}

/// One key epoch committed in the chain: the epoch and its key's hash
/// ([`user_key_commit`], [`machine_key_commit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCommit {
    pub epoch: u32,
    pub hash: [u8; 32],
}

/// A `KeyEpochs` entry: a new user key epoch, and/or a new key epoch for each
/// of some hosts, each exactly one past the last committed. Every key a
/// device or an agent accepts — whoever delivered it, however it was wrapped —
/// must hash to what the chain committed for its epoch: a removed device (or
/// the hub, or both) can neither plant a key of its choosing nor push an
/// epoch out of reach.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyCommits {
    pub user: Option<KeyCommit>,
    pub machines: Vec<(String, KeyCommit)>,
}

fn key_commit(kind: u8, account: &str, id: &str, epoch: u32, key: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(KEY_COMMIT_LABEL);
    h.update([kind]);
    let mut ids = Vec::new();
    put_str16(&mut ids, account);
    put_str16(&mut ids, id);
    h.update(ids);
    h.update(epoch.to_be_bytes());
    h.update(key);
    h.finalize().into()
}

/// The commitment to user key epoch `epoch`.
pub fn user_key_commit(account: &str, epoch: u32, key: &[u8]) -> [u8; 32] {
    key_commit(1, account, "", epoch, key)
}

/// The commitment to host `machine`'s key epoch `epoch`.
pub fn machine_key_commit(account: &str, machine: &str, epoch: u32, key: &[u8]) -> [u8; 32] {
    key_commit(2, account, machine, epoch, key)
}

/// A device as a **v2** entry names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    /// Ed25519: signs chain entries and attestations.
    pub sign_key: [u8; 32],
    /// X25519: the Noise static key, and what keys are sealed to and from.
    pub dh_key: [u8; 32],
    /// v2's "full device": an admin in the v3 state. A limited one is not.
    pub full: bool,
}

/// A machine as a **v2** `AddMachine` names it: a host in the v3 state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineInfo {
    pub id: String,
    pub sign_key: [u8; 32],
    pub dh_key: [u8; 32],
}

/// A device's two flags (`DeviceV3`'s last byte: bit 0 host, bit 1 admin;
/// every other bit must be 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    /// 可运行 AI: runs an agent, can be controlled, has machine keys.
    pub host: bool,
    /// 可管理账号: signs entries, holds `UK`, attests.
    pub admin: bool,
}

impl Flags {
    pub const HOST: u8 = 0b01;
    pub const ADMIN: u8 = 0b10;
    pub const NONE: Flags = Flags { host: false, admin: false };

    pub fn to_byte(self) -> u8 {
        (if self.host { Self::HOST } else { 0 }) | (if self.admin { Self::ADMIN } else { 0 })
    }

    /// Unknown bits are refused: one byte, one meaning.
    pub fn from_byte(b: u8) -> Result<Self, DecodeError> {
        if b & !(Self::HOST | Self::ADMIN) != 0 {
            return Err(DecodeError::Invalid("device flags"));
        }
        Ok(Self {
            host: b & Self::HOST != 0,
            admin: b & Self::ADMIN != 0,
        })
    }
}

/// A device as a **v3** entry names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceV3 {
    pub id: String,
    pub sign_key: [u8; 32],
    pub dh_key: [u8; 32],
    pub flags: Flags,
}

/// A device's identity in the state: its id and its two public keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKeys {
    pub id: String,
    pub sign_key: [u8; 32],
    pub dh_key: [u8; 32],
}

/// Whom a host accepts control from (`SetAccept`), besides admin devices,
/// which it always accepts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Accept {
    /// Admin devices only: the default.
    #[default]
    Admins,
    /// Every current device of the account.
    All,
    /// Admin devices and these (ascending, no repeats).
    Devices(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    // ---- v2 only
    Genesis {
        account_key: [u8; 32],
        device: DeviceInfo,
        recovery_key: Option<RecoveryKey>,
    },
    AddDevice(DeviceInfo),
    RemoveDevice(String),
    AddMachine(MachineInfo),
    RemoveMachine(String),
    RecoveryJoin(DeviceInfo),
    // ---- either version (the draft's version says which)
    ApproveDevice(String),
    ActivateRecovered(String),
    SetRecoveryKey(Option<RecoveryKey>),
    KeyEpochs(KeyCommits),
    // ---- v3 only
    /// Kind 6 in a v3 entry: the recovered device must be an admin, and may
    /// be a host.
    RecoveryJoin3(DeviceV3),
    Genesis3 {
        account_key: [u8; 32],
        device: DeviceV3,
        recovery_key: Option<RecoveryKey>,
    },
    AddDevice3(DeviceV3),
    SetAdmin {
        id: String,
        admin: bool,
    },
    SetHost {
        id: String,
        host: bool,
    },
    RemoveDevice3(String),
    PasskeySlotSet(PasskeySlot),
    PasskeySlotRemove(String),
    PasskeyActivate(String),
    SetAccept {
        host: String,
        from: Accept,
    },
    /// Kind 20: a current host renews its identity key
    /// (`docs/host-identity-rotation.md` §3.4).
    RotateHostKey(HostRotation),
}

impl Action {
    pub fn kind(&self) -> u8 {
        match self {
            Action::Genesis { .. } => 1,
            Action::AddDevice(_) => 2,
            Action::RemoveDevice(_) => 3,
            Action::AddMachine(_) => 4,
            Action::RemoveMachine(_) => 5,
            Action::RecoveryJoin(_) | Action::RecoveryJoin3(_) => 6,
            Action::ApproveDevice(_) => 7,
            Action::ActivateRecovered(_) => 8,
            Action::SetRecoveryKey(_) => 9,
            Action::KeyEpochs(_) => 10,
            Action::Genesis3 { .. } => 11,
            Action::AddDevice3(_) => 12,
            Action::SetAdmin { .. } => 13,
            Action::SetHost { .. } => 14,
            Action::RemoveDevice3(_) => 15,
            Action::PasskeySlotSet(_) => 16,
            Action::PasskeySlotRemove(_) => 17,
            Action::PasskeyActivate(_) => 18,
            Action::SetAccept { .. } => 19,
            Action::RotateHostKey(_) => 20,
        }
    }

    /// The one version this action exists in, or `None` for the four that
    /// both versions share (the draft's version byte decides those).
    pub fn fixed_version(&self) -> Option<u8> {
        match self {
            Action::Genesis { .. }
            | Action::AddDevice(_)
            | Action::RemoveDevice(_)
            | Action::AddMachine(_)
            | Action::RemoveMachine(_)
            | Action::RecoveryJoin(_) => Some(VERSION),
            Action::ApproveDevice(_) | Action::ActivateRecovered(_) | Action::SetRecoveryKey(_) | Action::KeyEpochs(_) => None,
            _ => Some(VERSION_3),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signer {
    /// The account key `AK`: genesis only.
    Account,
    /// A device, by id.
    Device(String),
    /// The recovery key `RSK`: recovery-join and activate-recovered only.
    Recovery,
    /// A passkey slot's `PAK` (v3): its own pending join and activation only.
    Passkey(String),
}

/// An entry before it is signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// [`VERSION`] (2) or [`VERSION_3`]; covered by the signature.
    pub version: u8,
    pub account: String,
    pub seq: u64,
    pub prev: [u8; 32],
    pub created_ms: u64,
    pub signer: Signer,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub draft: Draft,
    pub signature: [u8; 64],
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChainError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("unsupported chain entry version {0}")]
    Version(u8),
    #[error("entry {seq}: bad signature")]
    BadSignature { seq: u64 },
    #[error("entry {seq}: belongs to another account")]
    WrongAccount { seq: u64 },
    #[error("expected entry {expected}, got {got}")]
    OutOfOrder { expected: u64, got: u64 },
    #[error("entry {seq}: does not follow the entry before it")]
    BrokenLink { seq: u64 },
    #[error("the first entry is not a genesis")]
    NotGenesis,
    #[error("entry {seq}: a genesis after the first entry")]
    LateGenesis { seq: u64 },
    #[error("the account key does not match the pinned fingerprint")]
    PinMismatch,
    #[error("the genesis entry is not the one pinned at enrollment")]
    GenesisMismatch,
    #[error("entry {seq}: {reason}")]
    NotAllowed { seq: u64, reason: &'static str },
    #[error("entry {seq}: id {id:?} was already used")]
    DuplicateId { seq: u64, id: String },
    #[error("entry {seq}: no current device or machine {id:?}")]
    UnknownId { seq: u64, id: String },
    #[error("entry {seq}: would remove the last full device")]
    LastFullDevice { seq: u64 },
    #[error("entry {seq}: would leave the account without an admin device")]
    LastAdmin { seq: u64 },
    #[error("entry {seq}: no current passkey slot {slot:?}")]
    UnknownSlot { seq: u64, slot: String },
    #[error("entry {seq}: invalid or small-order key")]
    WeakKey { seq: u64 },
    #[error("entry {seq}: invalid id")]
    BadId { seq: u64 },
    #[error("entry {seq}: a pending device may activate itself only from {ready_ms} on")]
    TooEarly { seq: u64, ready_ms: u64 },
    #[error("the chain was rolled back: seen up to {seen}, offered up to {offered}")]
    Rollback { seen: u64, offered: u64 },
    #[error("the chain forked at entry {seq}")]
    Fork { seq: u64 },
    #[error("the other side has seen the chain up to {seq}, beyond this view")]
    Behind { seq: u64 },
    #[error("device {0:?} is not a current, active device of this account, or its key does not match")]
    UnknownDevice(String),
    #[error("bad attestation: {0}")]
    BadAttestation(&'static str),
    #[error("no key epoch {epoch} is committed for {id:?}")]
    UncommittedKey { id: String, epoch: u32 },
    #[error("the key for {id:?} epoch {epoch} is not the one the chain committed")]
    KeyMismatch { id: String, epoch: u32 },
    #[error("device {device:?} may not control {host:?}: {reason}")]
    NotAccepted { device: String, host: String, reason: &'static str },
    #[error("a device does not control itself")]
    SelfControl,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && id.bytes().all(|b| b.is_ascii_graphic())
}

fn put_recovery_key(out: &mut Vec<u8>, key: &Option<RecoveryKey>) {
    match key {
        None => out.push(0),
        Some(k) => {
            out.push(1);
            out.extend_from_slice(&k.key);
            out.extend_from_slice(&k.proof);
        }
    }
}

fn get_recovery_key(r: &mut Reader<'_>) -> Result<Option<RecoveryKey>, DecodeError> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(RecoveryKey {
            key: r.array()?,
            proof: r.array()?,
        })),
        _ => Err(DecodeError::Invalid("key presence flag")),
    }
}

fn get_bool(r: &mut Reader<'_>, what: &'static str) -> Result<bool, DecodeError> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DecodeError::Invalid(what)),
    }
}

fn put_commit(out: &mut Vec<u8>, c: &KeyCommit) {
    out.extend_from_slice(&c.epoch.to_be_bytes());
    out.extend_from_slice(&c.hash);
}

fn get_commit(r: &mut Reader<'_>) -> Result<KeyCommit, DecodeError> {
    Ok(KeyCommit {
        epoch: r.u32()?,
        hash: r.array()?,
    })
}

/// `u8 has_user [commit] | u16 n | (str16 machine | commit)*`, a commit
/// being `u32 epoch | hash 32`.
fn put_key_commits(out: &mut Vec<u8>, k: &KeyCommits) {
    match &k.user {
        None => out.push(0),
        Some(c) => {
            out.push(1);
            put_commit(out, c);
        }
    }
    out.extend_from_slice(&(k.machines.len().min(usize::from(u16::MAX)) as u16).to_be_bytes());
    for (machine, c) in &k.machines {
        put_str16(out, machine);
        put_commit(out, c);
    }
}

fn get_key_commits(r: &mut Reader<'_>) -> Result<KeyCommits, DecodeError> {
    let user = match r.u8()? {
        0 => None,
        1 => Some(get_commit(r)?),
        _ => return Err(DecodeError::Invalid("user key presence flag")),
    };
    let n = usize::from(r.u16()?);
    if n > MAX_KEY_COMMITS {
        return Err(DecodeError::TooLarge {
            what: "machine key commits",
            len: n,
            max: MAX_KEY_COMMITS,
        });
    }
    let mut machines = Vec::with_capacity(n);
    for _ in 0..n {
        let machine = r.str16("machine id", MAX_ID_LEN)?;
        machines.push((machine, get_commit(r)?));
    }
    Ok(KeyCommits { user, machines })
}

fn put_device(out: &mut Vec<u8>, device: &DeviceInfo) {
    put_str16(out, &device.id);
    out.extend_from_slice(&device.sign_key);
    out.extend_from_slice(&device.dh_key);
    out.push(u8::from(device.full));
}

fn get_device(r: &mut Reader<'_>) -> Result<DeviceInfo, DecodeError> {
    let id = r.str16("device id", MAX_ID_LEN)?;
    let sign_key = r.array()?;
    let dh_key = r.array()?;
    let full = match r.u8()? {
        0 => false,
        1 => true,
        _ => return Err(DecodeError::Invalid("device flags")),
    };
    Ok(DeviceInfo { id, sign_key, dh_key, full })
}

/// `str16 id | sign_key 32 | dh_key 32 | u8 flags`.
fn put_device3(out: &mut Vec<u8>, device: &DeviceV3) {
    put_str16(out, &device.id);
    out.extend_from_slice(&device.sign_key);
    out.extend_from_slice(&device.dh_key);
    out.push(device.flags.to_byte());
}

fn get_device3(r: &mut Reader<'_>) -> Result<DeviceV3, DecodeError> {
    Ok(DeviceV3 {
        id: r.str16("device id", MAX_ID_LEN)?,
        sign_key: r.array()?,
        dh_key: r.array()?,
        flags: Flags::from_byte(r.u8()?)?,
    })
}

/// `str16 slot | PAK 32 | proof 64 | cred_hash 32 | salt_nonce 32 | u8 grants | u8 self_activate`.
fn put_slot(out: &mut Vec<u8>, s: &PasskeySlot) {
    put_str16(out, &s.slot);
    out.extend_from_slice(&s.key);
    out.extend_from_slice(&s.proof);
    out.extend_from_slice(&s.cred_hash);
    out.extend_from_slice(&s.salt_nonce);
    out.push(s.grants);
    out.push(u8::from(s.self_activate));
}

fn get_slot(r: &mut Reader<'_>) -> Result<PasskeySlot, DecodeError> {
    Ok(PasskeySlot {
        slot: r.str16("passkey slot", MAX_ID_LEN)?,
        key: r.array()?,
        proof: r.array()?,
        cred_hash: r.array()?,
        salt_nonce: r.array()?,
        grants: r.u8()?,
        self_activate: get_bool(r, "self-activate flag")?,
    })
}

/// `u8 kind (0 admins, 1 all, 2 list) [| u16 n | str16 id × n]`, the list
/// non-empty, at most [`MAX_ACCEPT_IDS`], strictly ascending.
fn put_accept(out: &mut Vec<u8>, from: &Accept) {
    match from {
        Accept::Admins => out.push(0),
        Accept::All => out.push(1),
        Accept::Devices(ids) => {
            out.push(2);
            out.extend_from_slice(&(ids.len().min(usize::from(u16::MAX)) as u16).to_be_bytes());
            for id in ids {
                put_str16(out, id);
            }
        }
    }
}

fn get_accept(r: &mut Reader<'_>) -> Result<Accept, DecodeError> {
    match r.u8()? {
        0 => Ok(Accept::Admins),
        1 => Ok(Accept::All),
        2 => {
            let n = usize::from(r.u16()?);
            if n == 0 {
                return Err(DecodeError::Invalid("an empty accept list"));
            }
            if n > MAX_ACCEPT_IDS {
                return Err(DecodeError::TooLarge {
                    what: "accept list",
                    len: n,
                    max: MAX_ACCEPT_IDS,
                });
            }
            let mut ids: Vec<String> = Vec::with_capacity(n);
            for _ in 0..n {
                let id = r.str16("device id", MAX_ID_LEN)?;
                if ids.last().is_some_and(|last| last.as_bytes() >= id.as_bytes()) {
                    return Err(DecodeError::Invalid("accept list order"));
                }
                ids.push(id);
            }
            Ok(Accept::Devices(ids))
        }
        other => Err(DecodeError::Unknown {
            what: "accept policy",
            value: other,
        }),
    }
}

/// `str16 host | sign_key 32 | dh_key 32 | u64 staged_seq | staged_head 32 |
/// nonce 16 | u8 assurance (0 or 1) | old_sig 64 | new_sig 64`.
fn put_rotation(out: &mut Vec<u8>, r: &HostRotation) {
    put_str16(out, &r.host);
    out.extend_from_slice(&r.sign_key);
    out.extend_from_slice(&r.dh_key);
    out.extend_from_slice(&r.staged_seq.to_be_bytes());
    out.extend_from_slice(&r.staged_head);
    out.extend_from_slice(&r.nonce);
    out.push(r.assurance.to_byte());
    out.extend_from_slice(&r.old_sig);
    out.extend_from_slice(&r.new_sig);
}

fn get_rotation(r: &mut Reader<'_>) -> Result<HostRotation, DecodeError> {
    Ok(HostRotation {
        host: r.str16("device id", MAX_ID_LEN)?,
        sign_key: r.array()?,
        dh_key: r.array()?,
        staged_seq: r.u64()?,
        staged_head: r.array()?,
        nonce: r.array()?,
        assurance: Assurance::from_byte(r.u8()?)?,
        old_sig: r.array()?,
        new_sig: r.array()?,
    })
}

impl Draft {
    /// The bytes that are signed (after the label) and that, with the
    /// signature, are the entry.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(192);
        out.push(self.version);
        put_str16(&mut out, &self.account);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.prev);
        out.extend_from_slice(&self.created_ms.to_be_bytes());
        match &self.signer {
            Signer::Account => out.push(0),
            Signer::Device(id) => {
                out.push(1);
                put_str16(&mut out, id);
            }
            Signer::Recovery => out.push(2),
            Signer::Passkey(slot) => {
                out.push(3);
                put_str16(&mut out, slot);
            }
        }
        out.push(self.action.kind());
        match &self.action {
            Action::Genesis {
                account_key,
                device,
                recovery_key,
            } => {
                out.extend_from_slice(account_key);
                put_device(&mut out, device);
                put_recovery_key(&mut out, recovery_key);
            }
            Action::Genesis3 {
                account_key,
                device,
                recovery_key,
            } => {
                out.extend_from_slice(account_key);
                put_device3(&mut out, device);
                put_recovery_key(&mut out, recovery_key);
            }
            Action::AddDevice(device) | Action::RecoveryJoin(device) => put_device(&mut out, device),
            Action::AddDevice3(device) | Action::RecoveryJoin3(device) => put_device3(&mut out, device),
            Action::RemoveDevice(id)
            | Action::RemoveMachine(id)
            | Action::ApproveDevice(id)
            | Action::ActivateRecovered(id)
            | Action::RemoveDevice3(id)
            | Action::PasskeySlotRemove(id)
            | Action::PasskeyActivate(id) => put_str16(&mut out, id),
            Action::AddMachine(machine) => {
                put_str16(&mut out, &machine.id);
                out.extend_from_slice(&machine.sign_key);
                out.extend_from_slice(&machine.dh_key);
            }
            Action::SetRecoveryKey(key) => put_recovery_key(&mut out, key),
            Action::KeyEpochs(k) => put_key_commits(&mut out, k),
            Action::SetAdmin { id, admin: flag } | Action::SetHost { id, host: flag } => {
                put_str16(&mut out, id);
                out.push(u8::from(*flag));
            }
            Action::PasskeySlotSet(slot) => put_slot(&mut out, slot),
            Action::SetAccept { host, from } => {
                put_str16(&mut out, host);
                put_accept(&mut out, from);
            }
            Action::RotateHostKey(r) => put_rotation(&mut out, r),
        }
        out
    }

    pub fn sign(self, key: &SigningKey) -> Entry {
        let signature = sig::sign(key, sig::CHAIN_ENTRY, &self.encode());
        Entry { draft: self, signature }
    }
}

impl Entry {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.draft.encode();
        out.extend_from_slice(&self.signature);
        out
    }

    /// Decode one entry. The version byte decides which actions and signers
    /// exist: a v3 action (or a passkey signer) in a v2 entry, or a v2-only
    /// action in a v3 entry, does not decode — one byte string, one reading.
    pub fn decode(bytes: &[u8]) -> Result<Entry, ChainError> {
        let mut r = Reader::new(bytes);
        let version = r.u8()?;
        if version != VERSION && version != VERSION_3 {
            return Err(ChainError::Version(version));
        }
        let v3 = version == VERSION_3;
        let account = r.str16("account id", MAX_ID_LEN)?;
        let seq = r.u64()?;
        let prev = r.array()?;
        let created_ms = r.u64()?;
        let signer = match r.u8()? {
            0 => Signer::Account,
            1 => Signer::Device(r.str16("signer id", MAX_ID_LEN)?),
            2 => Signer::Recovery,
            3 if v3 => Signer::Passkey(r.str16("passkey slot", MAX_ID_LEN)?),
            other => {
                return Err(DecodeError::Unknown {
                    what: "signer",
                    value: other,
                }
                .into())
            }
        };
        let id = |r: &mut Reader<'_>, what: &'static str| r.str16(what, MAX_ID_LEN);
        let action = match (v3, r.u8()?) {
            (false, 1) => Action::Genesis {
                account_key: r.array()?,
                device: get_device(&mut r)?,
                recovery_key: get_recovery_key(&mut r)?,
            },
            (false, 2) => Action::AddDevice(get_device(&mut r)?),
            (false, 3) => Action::RemoveDevice(id(&mut r, "device id")?),
            (false, 4) => Action::AddMachine(MachineInfo {
                id: id(&mut r, "machine id")?,
                sign_key: r.array()?,
                dh_key: r.array()?,
            }),
            (false, 5) => Action::RemoveMachine(id(&mut r, "machine id")?),
            (false, 6) => Action::RecoveryJoin(get_device(&mut r)?),
            (true, 6) => Action::RecoveryJoin3(get_device3(&mut r)?),
            (_, 7) => Action::ApproveDevice(id(&mut r, "device id")?),
            (_, 8) => Action::ActivateRecovered(id(&mut r, "device id")?),
            (_, 9) => Action::SetRecoveryKey(get_recovery_key(&mut r)?),
            (_, 10) => Action::KeyEpochs(get_key_commits(&mut r)?),
            (true, 11) => Action::Genesis3 {
                account_key: r.array()?,
                device: get_device3(&mut r)?,
                recovery_key: get_recovery_key(&mut r)?,
            },
            (true, 12) => Action::AddDevice3(get_device3(&mut r)?),
            (true, 13) => Action::SetAdmin {
                id: id(&mut r, "device id")?,
                admin: get_bool(&mut r, "admin flag")?,
            },
            (true, 14) => Action::SetHost {
                id: id(&mut r, "device id")?,
                host: get_bool(&mut r, "host flag")?,
            },
            (true, 15) => Action::RemoveDevice3(id(&mut r, "device id")?),
            (true, 16) => Action::PasskeySlotSet(get_slot(&mut r)?),
            (true, 17) => Action::PasskeySlotRemove(id(&mut r, "passkey slot")?),
            (true, 18) => Action::PasskeyActivate(id(&mut r, "device id")?),
            (true, 19) => Action::SetAccept {
                host: id(&mut r, "device id")?,
                from: get_accept(&mut r)?,
            },
            (true, 20) => Action::RotateHostKey(get_rotation(&mut r)?),
            (_, other) => {
                return Err(DecodeError::Unknown {
                    what: "chain action",
                    value: other,
                }
                .into())
            }
        };
        let signature = r.array()?;
        r.finish()?;
        Ok(Entry {
            draft: Draft {
                version,
                account,
                seq,
                prev,
                created_ms,
                signer,
                action,
            },
            signature,
        })
    }

    /// What the next entry's `prev` must be.
    pub fn hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(HASH_LABEL);
        h.update(self.encode());
        h.finalize().into()
    }

    pub fn seq(&self) -> u64 {
        self.draft.seq
    }

    /// Whether this is a v3 entry.
    pub fn is_v3(&self) -> bool {
        self.draft.version == VERSION_3
    }

    fn verify_with(&self, key: &[u8; 32]) -> bool {
        sig::verify(key, sig::CHAIN_ENTRY, &self.draft.encode(), &self.signature)
    }
}

/// The keys derived from a recovery secret `RK` (≥ 16 random bytes the user
/// keeps as words): `RSK`, which signs recovery entries, and the key that wraps
/// the recovery package of `UK`s. Nothing else derives from `RK`, and no
/// device stores it.
pub struct RecoveryKeys {
    pub signing: SigningKey,
    pub wrap: Zeroizing<[u8; 32]>,
    /// The X25519 twin of `RSK` (the same scalar on the Montgomery curve):
    /// every new `UK` epoch is sealed to its public half, which any device can
    /// compute from the `RSK` the chain holds ([`recovery_seal_public`]) — so
    /// the recovery package keeps up with rotations though no device stores `RK`.
    pub seal: crate::keys::DhKeypair,
}

pub fn recovery_keys(rk: &[u8]) -> RecoveryKeys {
    assert!(rk.len() >= 16, "a recovery secret is at least 128 bits");
    let hk = Hkdf::<Sha256>::new(Some(b"rh/v1 recovery"), rk);
    let mut seed = Zeroizing::new([0u8; 32]);
    let mut wrap = Zeroizing::new([0u8; 32]);
    hk.expand(b"signing key", seed.as_mut()).expect("32 bytes");
    hk.expand(b"wrap key", wrap.as_mut()).expect("32 bytes");
    // Ed25519's secret scalar is clamp(SHA-512(seed)[..32]); X25519 clamps the
    // same bytes, so this key pair's public half is RSK's on the u-line.
    let mut expanded = Zeroizing::new([0u8; 64]);
    expanded.copy_from_slice(&sha2::Sha512::digest(*seed));
    let mut scalar = Zeroizing::new([0u8; 32]);
    scalar.copy_from_slice(&expanded[..32]);
    RecoveryKeys {
        signing: SigningKey::from_bytes(&seed),
        wrap,
        seal: crate::keys::DhKeypair::from_secret(*scalar),
    }
}

/// The X25519 public key a `UK` epoch is sealed to for recovery, from the
/// recovery key `RSK` the chain names (Edwards to Montgomery).
pub fn recovery_seal_public(rsk: &[u8; 32]) -> Option<[u8; 32]> {
    let key = ed25519_key(rsk)?;
    Some(key.to_montgomery().to_bytes())
}

/// What [`ChainState::merge_partial`] did with a batch.
#[derive(Debug, PartialEq, Eq)]
pub struct Merged {
    /// How many entries it applied: the longest run, past what the view had,
    /// that verifies. These stay applied whatever `error` says.
    pub applied: usize,
    /// Why it stopped before the end of the batch, if it did.
    pub error: Option<ChainError>,
}

/// The highest point of a chain a verifier has seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
    pub seq: u64,
    pub head: [u8; 32],
}

/// What an agent trusts an account by, fixed when it was enrolled: the account
/// key's fingerprint from the code the user typed, and — once a chain has
/// verified against that — the hash of the genesis entry itself.
///
/// `AK` signs only genesis and is destroyed after it, so a second genesis under
/// the same key should never exist. Pinning the hash as well means that even if
/// `AK` had leaked (a device that failed to destroy it, say), a different
/// genesis could not later be passed off on an enrolled agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustRoot {
    pub account_fingerprint: [u8; 16],
    /// `None` only between reading the code and first verifying a chain.
    pub genesis_hash: Option<[u8; 32]>,
}

impl TrustRoot {
    /// The root for an enrollment code's fingerprint, before any chain was seen.
    pub fn from_fingerprint(account_fingerprint: [u8; 16]) -> Self {
        Self {
            account_fingerprint,
            genesis_hash: None,
        }
    }

    /// Pin the genesis of a chain verified against this root.
    pub fn pinned_to(self, chain: &ChainState) -> Self {
        Self {
            genesis_hash: Some(chain.genesis_hash()),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    Active,
    /// Joined at `join_seq` with the recovery key or a passkey slot; not yet
    /// approved or activated.
    Pending {
        join_seq: u64,
    },
    Removed,
}

/// A device as the chain has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub info: DeviceKeys,
    /// Runs an agent and has machine keys (v2: a machine).
    pub host: bool,
    /// Manages the account (v2: a full device).
    pub admin: bool,
    /// Has run AI here at some point — been a host, so its agent held its
    /// key in a file (review of chain v3: such a device never becomes an
    /// admin, [`rules::RAN_AI_NOT_ADMIN`]).
    pub ran_ai: bool,
    pub status: DeviceStatus,
    pub via_recovery: bool,
    /// The recovery key it came back with — the account's at its recovery
    /// join — so that only the words it knows go with it (v3; write gate).
    pub via_recovery_key: Option<[u8; 32]>,
    /// The passkey slot it joined with, if it did.
    pub via_passkey: Option<String>,
    pub added_seq: u64,
    /// The entry that made it active: its addition, or (for a pending
    /// device) the approval or self-activation.
    pub activated_seq: Option<u64>,
    /// The identity keys it renewed away from, oldest first
    /// (`RotateHostKey`; `docs/host-identity-rotation.md` §3.4).
    pub retired: Vec<RetiredKeys>,
    /// The entry of its latest renewal, if it renewed.
    pub rotated_seq: Option<u64>,
}

/// Identity keys a device held before a renewal (`RotateHostKey`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetiredKeys {
    pub sign_key: [u8; 32],
    pub dh_key: [u8; 32],
    /// The renewal that retired them: they were the device's keys for every
    /// entry before this one ([`ChainState::keys_at`]).
    pub until_seq: u64,
    /// Whether that renewal was confirmed on the device itself
    /// ([`Assurance::OnDevice`]): a key it retired showing up again is
    /// recorded, not a reason to isolate the host (§3.8).
    pub by_assured: bool,
}

impl DeviceRecord {
    pub fn is_active(&self) -> bool {
        self.status == DeviceStatus::Active
    }

    pub fn flags(&self) -> Flags {
        Flags {
            host: self.host,
            admin: self.admin,
        }
    }

    /// An active host: what v2 called a current machine.
    pub fn is_current_host(&self) -> bool {
        self.is_active() && self.host
    }

    /// An active admin: what v2 called a current full device.
    pub fn is_current_admin(&self) -> bool {
        self.is_active() && self.admin
    }
}

/// A passkey slot as the chain has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotRecord {
    pub slot: PasskeySlot,
    /// The admin that registered it: removing that device voids the slot.
    pub set_by: String,
    pub added_seq: u64,
    /// Removed, or voided by a removal; a slot id is never used again.
    pub removed: bool,
}

/// A verifier's own sense of time for the rules that depend on it: a device
/// that joined with the recovery key (or a passkey) may activate itself only
/// once the verifier has known about its join for [`RECOVERY_DELAY_MS`]
/// ([`PASSKEY_DELAY_MS`]).
///
/// "Known about" means the verifier's own clock when it first applied the join
/// — never a timestamp anyone else wrote — so neither the hub nor the device
/// can backdate it. A verifier records first sightings as it goes and persists
/// them ([`ChainClock::first_seen`], [`ChainClock::restore`]), and comes back
/// with [`ChainClock::live`] from the seq after the last one it had seen — not
/// with [`ChainClock::history`], or a hub could hand it a join and its
/// activation together at the next start.
///
/// An activation that is itself history (before `live_from_seq`) is taken as
/// it is: a device that joins an account trusts the account that let it in. A
/// *pending* join in that history is not: its delay runs from when this
/// verifier first applied it, so an agent enrolled (or a browser opened) just
/// after a join still waits the full delay before it accepts the activation.
#[derive(Debug, Clone)]
pub struct ChainClock {
    now_ms: u64,
    live_from_seq: u64,
    first_seen: BTreeMap<u64, u64>,
}

impl ChainClock {
    /// For verifying history the verifier was not present for: no time gates.
    pub fn history() -> Self {
        Self {
            now_ms: 0,
            live_from_seq: u64::MAX,
            first_seen: BTreeMap::new(),
        }
    }

    /// For a verifier that has followed the chain live since `live_from_seq`.
    pub fn live(now_ms: u64, live_from_seq: u64) -> Self {
        Self {
            now_ms,
            live_from_seq,
            first_seen: BTreeMap::new(),
        }
    }

    pub fn set_now(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    /// Whether this clock gates anything: a verifier that follows the chain
    /// live (an agent, a device) must use a live clock, never
    /// [`ChainClock::history`], or a recovered device could activate itself the
    /// moment its join arrived.
    pub fn is_live(&self) -> bool {
        self.live_from_seq != u64::MAX
    }

    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// Note that entry `seq` was first seen now, unless it was seen before.
    /// [`ChainState::apply`] does this itself for every entry it accepts, so a
    /// verifier cannot forget to; it is public for restoring and for tests. A
    /// history clock records nothing (it has no "now").
    pub fn observe(&mut self, seq: u64) {
        if self.is_live() {
            self.first_seen.entry(seq).or_insert(self.now_ms);
        }
    }

    pub fn first_seen(&self) -> &BTreeMap<u64, u64> {
        &self.first_seen
    }

    pub fn restore(&mut self, first_seen: BTreeMap<u64, u64>) {
        self.first_seen = first_seen;
    }

    /// `None` when the join at `join_seq` may be activated by the entry at
    /// `activation_seq` now (after `delay_ms`), or the time it may.
    fn ready_at(&self, join_seq: u64, activation_seq: u64, delay_ms: u64) -> Option<u64> {
        if activation_seq < self.live_from_seq {
            return None;
        }
        let seen = self.first_seen.get(&join_seq).copied().unwrap_or(self.now_ms);
        let ready = seen.saturating_add(delay_ms);
        (self.now_ms < ready).then_some(ready)
    }
}

/// An admin device's signed statement that, at `time_ms`, the chain it saw
/// ended at `seq` with `head`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    pub account: String,
    pub device: String,
    pub seq: u64,
    pub head: [u8; 32],
    pub time_ms: u64,
    pub signature: [u8; 64],
}

impl Attestation {
    fn body(account: &str, device: &str, seq: u64, head: &[u8; 32], time_ms: u64) -> Vec<u8> {
        let mut out = Vec::new();
        put_str16(&mut out, account);
        put_str16(&mut out, device);
        out.extend_from_slice(&seq.to_be_bytes());
        out.extend_from_slice(head);
        out.extend_from_slice(&time_ms.to_be_bytes());
        out
    }

    /// Attest to `state` as device `device`.
    pub fn create(state: &ChainState, device: &str, key: &SigningKey, time_ms: u64) -> Attestation {
        let body = Self::body(state.account(), device, state.seq(), &state.head(), time_ms);
        Attestation {
            account: state.account().into(),
            device: device.into(),
            seq: state.seq(),
            head: state.head(),
            time_ms,
            signature: sig::sign(key, sig::CHAIN_ATTEST, &body),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Self::body(&self.account, &self.device, self.seq, &self.head, self.time_ms);
        out.extend_from_slice(&self.signature);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ChainError> {
        let mut r = Reader::new(bytes);
        let account = r.str16("account id", MAX_ID_LEN)?;
        let device = r.str16("device id", MAX_ID_LEN)?;
        let seq = r.u64()?;
        let head = r.array()?;
        let time_ms = r.u64()?;
        let signature = r.array()?;
        r.finish()?;
        Ok(Self {
            account,
            device,
            seq,
            head,
            time_ms,
            signature,
        })
    }
}

/// How current a verifier's view of the chain is, by others' attestations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Another admin device vouched for exactly this head within the window.
    Fresh,
    /// Nobody else has vouched for this head recently; `newest_ms` is the
    /// newest usable attestation's time, if there was one.
    Stale { newest_ms: Option<u64> },
    /// A device has seen entries beyond this view: fetch them first.
    Behind { seq: u64 },
}

/// A chain verified from genesis: who is in the account right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainState {
    account: String,
    account_key: [u8; 32],
    recovery_key: Option<[u8; 32]>,
    /// The device that put the current recovery key in place (the genesis
    /// device for genesis's): removing it clears the key.
    recovery_set_by: Option<String>,
    /// Committed user key epochs, by epoch: their [`user_key_commit`]s.
    user_keys: Vec<[u8; 32]>,
    /// Committed key epochs per host.
    machine_keys: BTreeMap<String, Vec<[u8; 32]>>,
    entries: Vec<Entry>,
    hashes: Vec<[u8; 32]>,
    /// Every device the chain ever named, hosts included.
    devices: BTreeMap<String, DeviceRecord>,
    /// Every passkey slot the chain ever named.
    slots: BTreeMap<String, SlotRecord>,
    /// Hosts whose accept policy is not the default ([`Accept::Admins`]).
    accept: BTreeMap<String, Accept>,
    /// The first v3 entry's seq: no v2 entry may follow it.
    v3_from: Option<u64>,
    /// A host that joined or restarted hosting in the last entry: the next
    /// entry must commit its next machine key epoch.
    host_key_due: Option<String>,
}

/// Who signed an entry, as [`ChainState::apply`] established.
enum Authority {
    Recovery,
    Device(String),
    Passkey(String),
}

/// The reasons of [`ChainError::NotAllowed`] that chain v3 added, by rule:
/// the interop vectors name them, so both halves refuse for the same rule.
pub mod rules {
    /// D1 (review of chain v3): a device that runs AI does not manage the
    /// account — its agent never attests nor signs, so an admin host would
    /// make every view stale and count as an admin nobody can act as.
    /// Deferred until hosts can attest and sign (the Tauri app).
    pub const HOST_NOT_ADMIN: &str = "a device that runs AI cannot manage the account";
    /// D1 extended (re-review): nor does one that ran AI and stopped — its
    /// agent kept its signing key in a file all along.
    pub const RAN_AI_NOT_ADMIN: &str = "a device that has run AI cannot manage the account";
    pub const SELF_ADMIN: &str = "a device cannot make itself an admin";
    pub const SELF_HOST: &str = "a device cannot make itself a host";
    pub const NO_CHANGE: &str = "the device already is what this entry says";
    pub const HOST_KEY_NEXT: &str = "a new host's machine key epoch must come next";
    pub const ONLY_ADMIN_SIGNS: &str = "only an admin device may sign";
    pub const RECOVERED_ADMIN: &str = "a recovered device is an admin device";
    pub const PASSKEY_PLAIN: &str = "a passkey adds only a device that neither runs AI nor manages the account";
    pub const SLOT_GRANTS: &str = "a passkey slot grants nothing: its devices neither run AI nor manage the account";
    pub const V2_AFTER_V3: &str = "a v2 entry after the chain moved to v3";
    pub const ACCEPT_SELF: &str = "a host does not control itself";
    pub const ACCEPT_SAME: &str = "the host already accepts exactly that";
    pub const ACCEPT_NOT_HOST: &str = "only a host accepts control";
    /// `RotateHostKey` (`docs/host-identity-rotation.md` §3.4, rule 2): only
    /// a current, active host renews its identity key.
    pub const ROTATE_NOT_HOST: &str = "only a current device that runs AI renews its identity key";
    /// Rule 3: neither new key is one any device of the chain holds or held
    /// (which also keeps a host's identity off another device's key).
    pub const ROTATE_KEY_REUSED: &str = "a renewed identity key must be one the chain never named";
    /// Rule 4: the proof names an entry before this one, by its hash.
    pub const ROTATE_STAGED: &str = "the renewal was proved against another chain";
    /// Rule 5: the host's current key and its new key both signed the proof.
    pub const ROTATE_PROOF: &str = "the renewal is not signed by both the old and the new identity key";
    /// A key a renewal retired never names a device again — no device added,
    /// enrolled or joining with it, and no pending device holding it approved
    /// or activated — so whoever kept a copy of it cannot bring it back under
    /// a new id. (A key a current device holds is not refused: chains from
    /// before may name one twice.)
    pub const KEY_RETIRED: &str = "a retired identity key never comes back";
}

/// Which removal voids the recovery key: v2's `RemoveDevice` (unchanged, so
/// v2 chains replay as they always did) or v3's `RemoveDevice3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryRule {
    V2,
    V3,
}

fn not_allowed(seq: u64, reason: &'static str) -> ChainError {
    ChainError::NotAllowed { seq, reason }
}

impl ChainState {
    /// Start from a genesis entry (`Genesis` or `Genesis3`). `pinned_fingerprint`
    /// is the account-key fingerprint an agent was enrolled with
    /// ([`crate::identity::fingerprint`]).
    pub fn genesis(entry: &Entry, pinned_fingerprint: Option<&[u8; 16]>) -> Result<Self, ChainError> {
        let d = &entry.draft;
        let (account_key, keys, flags, recovery_key) = match &d.action {
            Action::Genesis {
                account_key,
                device,
                recovery_key,
            } => (
                account_key,
                DeviceKeys {
                    id: device.id.clone(),
                    sign_key: device.sign_key,
                    dh_key: device.dh_key,
                },
                Flags {
                    host: false,
                    admin: device.full,
                },
                recovery_key,
            ),
            Action::Genesis3 {
                account_key,
                device,
                recovery_key,
            } => (
                account_key,
                DeviceKeys {
                    id: device.id.clone(),
                    sign_key: device.sign_key,
                    dh_key: device.dh_key,
                },
                device.flags,
                recovery_key,
            ),
            _ => return Err(ChainError::NotGenesis),
        };
        if d.action.fixed_version() != Some(d.version) {
            return Err(ChainError::Version(d.version));
        }
        if d.seq != 0 {
            return Err(ChainError::OutOfOrder { expected: 0, got: d.seq });
        }
        if d.prev != [0; 32] {
            return Err(ChainError::BrokenLink { seq: 0 });
        }
        if !valid_id(&d.account) {
            return Err(ChainError::BadId { seq: 0 });
        }
        if d.signer != Signer::Account {
            return Err(not_allowed(0, "genesis must be signed by the account key"));
        }
        if let Some(pin) = pinned_fingerprint {
            if crate::identity::fingerprint(account_key) != *pin {
                return Err(ChainError::PinMismatch);
            }
        }
        if ed25519_key(account_key).is_none() || recovery_key.is_some_and(|k| ed25519_key(&k.key).is_none()) {
            return Err(ChainError::WeakKey { seq: 0 });
        }
        if recovery_key.is_some_and(|k| !k.verify(&d.account, account_key)) {
            return Err(not_allowed(0, "the recovery key's proof does not hold"));
        }
        if !entry.verify_with(account_key) {
            return Err(ChainError::BadSignature { seq: 0 });
        }
        if !flags.admin {
            return Err(not_allowed(
                0,
                if d.version == VERSION {
                    "the first device must be a full device"
                } else {
                    "the first device must be an admin device"
                },
            ));
        }
        if flags.host && flags.admin {
            return Err(not_allowed(0, rules::HOST_NOT_ADMIN));
        }
        let mut state = Self {
            account: d.account.clone(),
            account_key: *account_key,
            recovery_key: recovery_key.map(|k| k.key),
            recovery_set_by: recovery_key.map(|_| keys.id.clone()),
            user_keys: Vec::new(),
            machine_keys: BTreeMap::new(),
            entries: vec![entry.clone()],
            hashes: vec![entry.hash()],
            devices: BTreeMap::new(),
            slots: BTreeMap::new(),
            accept: BTreeMap::new(),
            v3_from: (d.version == VERSION_3).then_some(0),
            host_key_due: flags.host.then(|| keys.id.clone()),
        };
        state.check_new(0, &keys)?;
        state.devices.insert(
            keys.id.clone(),
            DeviceRecord {
                info: keys,
                host: flags.host,
                admin: flags.admin,
                ran_ai: flags.host,
                status: DeviceStatus::Active,
                via_recovery: false,
                via_recovery_key: None,
                via_passkey: None,
                added_seq: 0,
                activated_seq: Some(0),
                retired: Vec::new(),
                rotated_seq: None,
            },
        );
        Ok(state)
    }

    /// Verify a whole chain from genesis.
    pub fn verify(entries: &[Entry], pinned_fingerprint: Option<&[u8; 16]>, clock: &mut ChainClock) -> Result<Self, ChainError> {
        let (first, rest) = entries.split_first().ok_or(ChainError::NotGenesis)?;
        let mut state = Self::genesis(first, pinned_fingerprint)?;
        for entry in rest {
            state.apply(entry, clock)?;
        }
        Ok(state)
    }

    /// Verify a whole chain against an enrolled agent's [`TrustRoot`]: the
    /// account key's fingerprint, and the genesis hash once it is pinned.
    pub fn verify_rooted(entries: &[Entry], root: &TrustRoot, clock: &mut ChainClock) -> Result<Self, ChainError> {
        let first = entries.first().ok_or(ChainError::NotGenesis)?;
        if root.genesis_hash.is_some_and(|pinned| pinned != first.hash()) {
            return Err(ChainError::GenesisMismatch);
        }
        Self::verify(entries, Some(&root.account_fingerprint), clock)
    }

    /// Who may sign `entry`, checked (signature included); `v2` entries see
    /// the v2 view of the state (a machine is no signer).
    fn authority(&self, entry: &Entry) -> Result<Authority, ChainError> {
        let d = &entry.draft;
        let seq = d.seq;
        let recovery_action = matches!(
            d.action,
            Action::RecoveryJoin(_) | Action::RecoveryJoin3(_) | Action::ActivateRecovered(_)
        );
        match (&d.signer, &d.action) {
            (_, Action::Genesis { .. } | Action::Genesis3 { .. }) => Err(ChainError::LateGenesis { seq }),
            (Signer::Account, _) => Err(not_allowed(seq, "the account key signs genesis and nothing else")),
            (Signer::Recovery, _) if recovery_action => {
                let key = self.recovery_key.ok_or(not_allowed(seq, "this account has no recovery key"))?;
                if !entry.verify_with(&key) {
                    return Err(ChainError::BadSignature { seq });
                }
                Ok(Authority::Recovery)
            }
            (Signer::Recovery, _) => Err(not_allowed(seq, "the recovery key signs only recovery entries")),
            (Signer::Passkey(slot), Action::AddDevice3(_) | Action::PasskeyActivate(_)) => {
                let record = self.slot(slot).ok_or_else(|| ChainError::UnknownSlot { seq, slot: slot.clone() })?;
                if !entry.verify_with(&record.slot.key) {
                    return Err(ChainError::BadSignature { seq });
                }
                Ok(Authority::Passkey(slot.clone()))
            }
            (Signer::Passkey(_), _) => Err(not_allowed(seq, "a passkey slot signs only its own joins and activations")),
            (Signer::Device(_), _) if recovery_action => Err(not_allowed(seq, "recovery entries are signed by the recovery key")),
            (Signer::Device(_), Action::PasskeyActivate(_)) => Err(not_allowed(seq, "a passkey activation is signed by its slot")),
            (Signer::Device(id), _) => {
                let v2 = d.version == VERSION;
                let signer = self
                    .devices
                    .get(id)
                    // v2 knew no signing machines: a host is no v2 signer.
                    .filter(|r| r.is_active() && !(v2 && r.host))
                    .ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                if !signer.admin {
                    return Err(not_allowed(seq, if v2 { "only a full device may sign" } else { rules::ONLY_ADMIN_SIGNS }));
                }
                if !entry.verify_with(&signer.info.sign_key) {
                    return Err(ChainError::BadSignature { seq });
                }
                Ok(Authority::Device(id.clone()))
            }
        }
    }

    /// Apply the next entry, or refuse it and leave the state as it was. An
    /// accepted entry is recorded in `clock` as first seen now, so the
    /// recovery and passkey delays always run from the moment *this* verifier
    /// applied the join.
    pub fn apply(&mut self, entry: &Entry, clock: &mut ChainClock) -> Result<(), ChainError> {
        let d = &entry.draft;
        let seq = d.seq;
        let expected = self.hashes.len() as u64;
        if seq != expected {
            return Err(ChainError::OutOfOrder { expected, got: seq });
        }
        check_u64(d.created_ms)?;
        if d.account != self.account {
            return Err(ChainError::WrongAccount { seq });
        }
        if d.prev != self.head() {
            return Err(ChainError::BrokenLink { seq });
        }
        match d.action.fixed_version() {
            Some(v) if v != d.version => return Err(ChainError::Version(d.version)),
            None if d.version != VERSION && d.version != VERSION_3 => return Err(ChainError::Version(d.version)),
            _ => {}
        }
        if matches!(d.signer, Signer::Passkey(_)) && d.version != VERSION_3 {
            return Err(ChainError::Version(d.version));
        }
        if d.version == VERSION && self.v3_from.is_some() {
            return Err(not_allowed(seq, rules::V2_AFTER_V3));
        }
        // A host that just joined (or restarted hosting) gets its next machine
        // key epoch in the very next entry, before anything else.
        if let Some(host) = &self.host_key_due {
            let covered = matches!(&d.action, Action::KeyEpochs(k) if k.machines.iter().any(|(m, _)| m == host));
            if !covered {
                return Err(not_allowed(seq, rules::HOST_KEY_NEXT));
            }
        }
        let authority = self.authority(entry)?;
        let signer_id = match &authority {
            Authority::Device(id) => Some(id.as_str()),
            _ => None,
        };
        // What the action may do. Every check happens before any change.
        let mut host_key_due = None;
        match &d.action {
            Action::Genesis { .. } | Action::Genesis3 { .. } => unreachable!("refused above"),
            Action::AddDevice(device) => {
                let keys = keys_of(&device.id, &device.sign_key, &device.dh_key);
                self.check_new(seq, &keys)?;
                self.insert(
                    keys,
                    Flags {
                        host: false,
                        admin: device.full,
                    },
                    DeviceStatus::Active,
                    seq,
                );
            }
            Action::AddMachine(machine) => {
                let keys = keys_of(&machine.id, &machine.sign_key, &machine.dh_key);
                self.check_new(seq, &keys)?;
                self.insert(keys, Flags { host: true, admin: false }, DeviceStatus::Active, seq);
            }
            Action::RecoveryJoin(device) => {
                let keys = keys_of(&device.id, &device.sign_key, &device.dh_key);
                self.check_new(seq, &keys)?;
                if !device.full {
                    return Err(not_allowed(seq, "a recovered device is a full device"));
                }
                self.insert(keys, Flags { host: false, admin: true }, DeviceStatus::Pending { join_seq: seq }, seq);
                let record = self.devices.get_mut(&device.id).expect("inserted");
                record.via_recovery = true;
                record.via_recovery_key = self.recovery_key;
            }
            Action::RecoveryJoin3(device) => {
                let keys = keys_of(&device.id, &device.sign_key, &device.dh_key);
                self.check_new(seq, &keys)?;
                if !device.flags.admin {
                    return Err(not_allowed(seq, rules::RECOVERED_ADMIN));
                }
                if device.flags.host {
                    return Err(not_allowed(seq, rules::HOST_NOT_ADMIN));
                }
                self.insert(keys, device.flags, DeviceStatus::Pending { join_seq: seq }, seq);
                let record = self.devices.get_mut(&device.id).expect("inserted");
                record.via_recovery = true;
                record.via_recovery_key = self.recovery_key;
            }
            Action::AddDevice3(device) => {
                let keys = keys_of(&device.id, &device.sign_key, &device.dh_key);
                self.check_new(seq, &keys)?;
                match &authority {
                    Authority::Passkey(slot) => {
                        if device.flags != Flags::NONE {
                            return Err(not_allowed(seq, rules::PASSKEY_PLAIN));
                        }
                        // One pending join per slot: a newer one replaces it.
                        for record in self.devices.values_mut() {
                            if record.via_passkey.as_deref() == Some(slot.as_str()) && matches!(record.status, DeviceStatus::Pending { .. }) {
                                record.status = DeviceStatus::Removed;
                            }
                        }
                        self.insert(keys, Flags::NONE, DeviceStatus::Pending { join_seq: seq }, seq);
                        self.devices.get_mut(&device.id).expect("inserted").via_passkey = Some(slot.clone());
                    }
                    _ => {
                        if device.flags.host && device.flags.admin {
                            return Err(not_allowed(seq, rules::HOST_NOT_ADMIN));
                        }
                        if device.flags.host {
                            host_key_due = Some(device.id.clone());
                        }
                        self.insert(keys, device.flags, DeviceStatus::Active, seq);
                    }
                }
            }
            Action::ApproveDevice(id) => {
                let record = self.devices.get(id).ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                if !matches!(record.status, DeviceStatus::Pending { .. }) {
                    return Err(not_allowed(seq, "only a pending device can be approved"));
                }
                self.check_not_retired(seq, &record.info)?;
                let record = self.devices.get_mut(id).expect("present");
                record.status = DeviceStatus::Active;
                record.activated_seq = Some(seq);
            }
            Action::ActivateRecovered(id) => {
                let record = self.devices.get(id).ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                let DeviceStatus::Pending { join_seq } = record.status else {
                    return Err(not_allowed(seq, "only a pending device can be activated"));
                };
                self.check_not_retired(seq, &record.info)?;
                if !record.via_recovery {
                    return Err(not_allowed(
                        seq,
                        "only a device that came back with the recovery words activates with them",
                    ));
                }
                if let Some(ready_ms) = clock.ready_at(join_seq, seq, RECOVERY_DELAY_MS) {
                    return Err(ChainError::TooEarly { seq, ready_ms });
                }
                let record = self.devices.get_mut(id).expect("present");
                record.status = DeviceStatus::Active;
                record.activated_seq = Some(seq);
            }
            Action::PasskeyActivate(id) => {
                let Authority::Passkey(slot) = &authority else {
                    unreachable!("only a slot signs an activation")
                };
                let record = self.devices.get(id).ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                let DeviceStatus::Pending { join_seq } = record.status else {
                    return Err(not_allowed(seq, "only a pending device can be activated"));
                };
                self.check_not_retired(seq, &record.info)?;
                if record.via_passkey.as_deref() != Some(slot.as_str()) {
                    return Err(not_allowed(seq, "a passkey slot activates only the device it added"));
                }
                if !self.slot(slot).is_some_and(|s| s.slot.self_activate) {
                    return Err(not_allowed(seq, "this passkey slot does not let its devices activate themselves"));
                }
                if let Some(ready_ms) = clock.ready_at(join_seq, seq, PASSKEY_DELAY_MS) {
                    return Err(ChainError::TooEarly { seq, ready_ms });
                }
                let record = self.devices.get_mut(id).expect("present");
                record.status = DeviceStatus::Active;
                record.activated_seq = Some(seq);
            }
            Action::RemoveDevice(id) => {
                // v2: a device (never a machine).
                let record = self
                    .devices
                    .get(id)
                    .filter(|r| r.status != DeviceStatus::Removed && !r.host)
                    .ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                if record.is_current_admin() && self.admins_except(id) == 0 {
                    return Err(ChainError::LastFullDevice { seq });
                }
                self.remove(id, RecoveryRule::V2);
            }
            Action::RemoveMachine(id) => {
                // v2: a current machine.
                self.devices
                    .get(id)
                    .filter(|r| r.is_current_host())
                    .ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                self.devices.get_mut(id).expect("present").status = DeviceStatus::Removed;
            }
            Action::RemoveDevice3(id) => {
                let record = self
                    .devices
                    .get(id)
                    .filter(|r| r.status != DeviceStatus::Removed)
                    .ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                if record.is_current_admin() && self.admins_except(id) == 0 {
                    return Err(ChainError::LastAdmin { seq });
                }
                self.remove(id, RecoveryRule::V3);
            }
            Action::SetAdmin { id, admin } => {
                let record = self
                    .devices
                    .get(id)
                    .filter(|r| r.is_active())
                    .ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                if *admin && signer_id == Some(id.as_str()) {
                    return Err(not_allowed(seq, rules::SELF_ADMIN));
                }
                if record.admin == *admin {
                    return Err(not_allowed(seq, rules::NO_CHANGE));
                }
                if *admin && record.host {
                    return Err(not_allowed(seq, rules::HOST_NOT_ADMIN));
                }
                if *admin && (record.ran_ai || self.machine_key_epochs(id) > 0) {
                    return Err(not_allowed(seq, rules::RAN_AI_NOT_ADMIN));
                }
                if !*admin && self.admins_except(id) == 0 {
                    return Err(ChainError::LastAdmin { seq });
                }
                // A demoted admin that knows the *current* recovery words —
                // it set them, or came back with them — keeps that knowledge:
                // as with its removal, the key stops counting (every new user
                // key is sealed for it, and any device reads those packages)
                // until an admin sets a new one. Words set after it came back
                // with older ones stand.
                let knows_rk = self.knows_current_recovery_key(id);
                self.devices.get_mut(id).expect("present").admin = *admin;
                // A demoted admin computed the PRF of the slots it registered:
                // they go with its admin standing (as with its removal).
                if !*admin {
                    for slot in self.slots.values_mut() {
                        if !slot.removed && slot.set_by == *id {
                            slot.removed = true;
                        }
                    }
                    if knows_rk {
                        self.recovery_key = None;
                        self.recovery_set_by = None;
                    }
                }
            }
            Action::SetHost { id, host } => {
                let record = self
                    .devices
                    .get(id)
                    .filter(|r| r.is_active())
                    .ok_or_else(|| ChainError::UnknownId { seq, id: id.clone() })?;
                if *host && signer_id == Some(id.as_str()) {
                    return Err(not_allowed(seq, rules::SELF_HOST));
                }
                if record.host == *host {
                    return Err(not_allowed(seq, rules::NO_CHANGE));
                }
                if *host && record.admin {
                    return Err(not_allowed(seq, rules::HOST_NOT_ADMIN));
                }
                if *host {
                    host_key_due = Some(id.clone());
                } else {
                    // Its accept policy goes with its hosting: restarting
                    // starts from admins only, never silently from before.
                    self.accept.remove(id);
                }
                let record = self.devices.get_mut(id).expect("present");
                record.host = *host;
                record.ran_ai |= *host;
            }
            Action::SetRecoveryKey(key) => {
                if key.is_some_and(|k| ed25519_key(&k.key).is_none()) {
                    return Err(ChainError::WeakKey { seq });
                }
                if key.is_some_and(|k| !k.verify(&self.account, &self.account_key)) {
                    return Err(not_allowed(seq, "the recovery key's proof does not hold"));
                }
                self.recovery_key = key.map(|k| k.key);
                self.recovery_set_by = match (signer_id, key) {
                    (Some(id), Some(_)) => Some(id.to_string()),
                    _ => None,
                };
            }
            Action::KeyEpochs(k) => {
                if k.user.is_none() && k.machines.is_empty() {
                    return Err(not_allowed(seq, "a key epoch entry moves no key"));
                }
                if k.machines.len() > MAX_KEY_COMMITS {
                    return Err(not_allowed(seq, "too many machine keys in one entry"));
                }
                if k.user.is_some_and(|c| c.epoch as usize != self.user_keys.len()) {
                    return Err(not_allowed(seq, "a user key epoch must be the one after the last"));
                }
                let mut seen = BTreeSet::new();
                for (machine, c) in &k.machines {
                    if self.machine(machine).is_none() {
                        return Err(ChainError::UnknownId { seq, id: machine.clone() });
                    }
                    if !seen.insert(machine.as_str()) {
                        return Err(not_allowed(seq, "a machine twice in one entry"));
                    }
                    let next = self.machine_keys.get(machine).map_or(0, Vec::len);
                    if c.epoch as usize != next {
                        return Err(not_allowed(seq, "a machine key epoch must be the one after the last"));
                    }
                }
                if let Some(c) = k.user {
                    self.user_keys.push(c.hash);
                }
                for (machine, c) in &k.machines {
                    self.machine_keys.entry(machine.clone()).or_default().push(c.hash);
                }
            }
            Action::PasskeySlotSet(slot) => {
                if !valid_id(&slot.slot) {
                    return Err(ChainError::BadId { seq });
                }
                if self.slots.contains_key(&slot.slot) {
                    return Err(ChainError::DuplicateId { seq, id: slot.slot.clone() });
                }
                if slot.grants != 0 {
                    return Err(not_allowed(seq, rules::SLOT_GRANTS));
                }
                if ed25519_key(&slot.key).is_none() {
                    return Err(ChainError::WeakKey { seq });
                }
                if !slot.verify(&self.account, &self.account_key) {
                    return Err(not_allowed(seq, "the passkey slot's proof does not hold"));
                }
                let current: Vec<&SlotRecord> = self.slots.values().filter(|s| !s.removed).collect();
                if current.len() >= MAX_PASSKEY_SLOTS {
                    return Err(not_allowed(seq, "too many passkey slots"));
                }
                if current.iter().any(|s| s.slot.cred_hash == slot.cred_hash) {
                    return Err(not_allowed(seq, "that passkey already has a current slot"));
                }
                self.slots.insert(
                    slot.slot.clone(),
                    SlotRecord {
                        slot: slot.clone(),
                        set_by: signer_id.expect("an admin signs a slot").to_string(),
                        added_seq: seq,
                        removed: false,
                    },
                );
            }
            Action::PasskeySlotRemove(slot) => {
                if self.slot(slot).is_none() {
                    return Err(ChainError::UnknownSlot { seq, slot: slot.clone() });
                }
                self.slots.get_mut(slot).expect("present").removed = true;
            }
            Action::SetAccept { host, from } => {
                let record = self
                    .devices
                    .get(host)
                    .filter(|r| r.is_active())
                    .ok_or_else(|| ChainError::UnknownId { seq, id: host.clone() })?;
                if !record.host {
                    return Err(not_allowed(seq, rules::ACCEPT_NOT_HOST));
                }
                if let Accept::Devices(ids) = from {
                    for id in ids {
                        if !valid_id(id) {
                            return Err(ChainError::BadId { seq });
                        }
                        if self.device(id).is_none() {
                            return Err(ChainError::UnknownId { seq, id: id.clone() });
                        }
                        if id == host {
                            return Err(not_allowed(seq, rules::ACCEPT_SELF));
                        }
                    }
                }
                if self.accept_policy(host) == *from {
                    return Err(not_allowed(seq, rules::ACCEPT_SAME));
                }
                // Machine keys follow the accept policy (review D2): a device
                // the host stops accepting may have its MK, so a narrowing
                // takes a new MK epoch for the host in the very next entry.
                let before = self.extra_controllers(host, &self.accept_policy(host));
                let after = self.extra_controllers(host, from);
                if before.iter().any(|d| !after.contains(d)) {
                    host_key_due = Some(host.clone());
                }
                match from {
                    Accept::Admins => {
                        self.accept.remove(host);
                    }
                    other => {
                        self.accept.insert(host.clone(), other.clone());
                    }
                }
            }
            Action::RotateHostKey(r) => {
                // `docs/host-identity-rotation.md` §3.4. Rule 1 (v3 only, an
                // active admin signs) and rule 7 (a host that owes a machine
                // key epoch gets that first) are checked above, as for every
                // entry.
                let old = self
                    .machine(&r.host)
                    .map(|d| (d.info.sign_key, d.info.dh_key))
                    .ok_or_else(|| not_allowed(seq, rules::ROTATE_NOT_HOST))?;
                if ed25519_key(&r.sign_key).is_none() || !x25519_valid(&r.dh_key) {
                    return Err(ChainError::WeakKey { seq });
                }
                if self.ever_named(&r.sign_key) || self.ever_named(&r.dh_key) {
                    return Err(not_allowed(seq, rules::ROTATE_KEY_REUSED));
                }
                if r.staged_seq >= seq || self.hash_at(r.staged_seq) != Some(r.staged_head) {
                    return Err(not_allowed(seq, rules::ROTATE_STAGED));
                }
                if !r.verify(&self.account, &old.0, &old.1) {
                    return Err(not_allowed(seq, rules::ROTATE_PROOF));
                }
                // Rule 6: its next machine key epoch — sealed to the new key
                // only — comes in the very next entry.
                host_key_due = Some(r.host.clone());
                let record = self.devices.get_mut(&r.host).expect("a current host");
                record.retired.push(RetiredKeys {
                    sign_key: old.0,
                    dh_key: old.1,
                    until_seq: seq,
                    by_assured: r.assurance == Assurance::OnDevice,
                });
                record.info.sign_key = r.sign_key;
                record.info.dh_key = r.dh_key;
                record.rotated_seq = Some(seq);
            }
        }
        self.host_key_due = host_key_due;
        if d.version == VERSION_3 && self.v3_from.is_none() {
            self.v3_from = Some(seq);
        }
        self.entries.push(entry.clone());
        self.hashes.push(entry.hash());
        clock.observe(seq);
        Ok(())
    }

    /// Mark `id` removed, with what removing it takes along: a recovery key it
    /// set or came back with (it knows `RK`), and a passkey slot it registered
    /// or joined with (it computed the slot's PRF).
    /// Whether `id` knows the account's current recovery words: it set them,
    /// or came back with them (chain v3; write gate).
    fn knows_current_recovery_key(&self, id: &str) -> bool {
        self.recovery_key.is_some()
            && (self.recovery_set_by.as_deref() == Some(id) || self.devices.get(id).and_then(|d| d.via_recovery_key) == self.recovery_key)
    }

    fn remove(&mut self, id: &str, rule: RecoveryRule) {
        let record = self.devices.get(id).expect("present").clone();
        // A device that set the recovery key, or came back with it, knows
        // `RK`: once it is removed the key is no longer the user's alone, so
        // it stops counting (architecture doc §1 "吊销与轮换") until an admin
        // sets a new one. v2 voids whatever words the account has; v3 only
        // the words the device knows — newer ones set since stand.
        let knows_rk = match rule {
            RecoveryRule::V2 => record.via_recovery || self.recovery_set_by.as_deref() == Some(id),
            RecoveryRule::V3 => self.knows_current_recovery_key(id),
        };
        if knows_rk {
            self.recovery_key = None;
            self.recovery_set_by = None;
        }
        for slot in self.slots.values_mut() {
            if !slot.removed && (slot.set_by == id || record.via_passkey.as_deref() == Some(slot.slot.slot.as_str())) {
                slot.removed = true;
            }
        }
        self.devices.get_mut(id).expect("present").status = DeviceStatus::Removed;
    }

    fn insert(&mut self, info: DeviceKeys, flags: Flags, status: DeviceStatus, seq: u64) {
        let activated_seq = (status == DeviceStatus::Active).then_some(seq);
        self.devices.insert(
            info.id.clone(),
            DeviceRecord {
                info,
                host: flags.host,
                admin: flags.admin,
                ran_ai: flags.host,
                status,
                via_recovery: false,
                via_recovery_key: None,
                via_passkey: None,
                added_seq: seq,
                activated_seq,
                retired: Vec::new(),
                rotated_seq: None,
            },
        );
    }

    /// The current devices other than admins and the host itself that
    /// policy `from` lets control `host`.
    fn extra_controllers(&self, host: &str, from: &Accept) -> BTreeSet<String> {
        self.devices
            .values()
            .filter(|d| d.is_active() && !d.admin && d.info.id != host)
            .filter(|d| match from {
                Accept::Admins => false,
                Accept::All => true,
                Accept::Devices(ids) => ids.contains(&d.info.id),
            })
            .map(|d| d.info.id.clone())
            .collect()
    }

    /// Active admins other than `id`.
    fn admins_except(&self, id: &str) -> usize {
        self.devices.values().filter(|d| d.is_current_admin() && d.info.id != id).count()
    }

    /// Apply whatever of `entries` this view lacks. Entries it already has must
    /// be the ones it has (anything else is a fork); entries beyond must follow
    /// on. Returns how many were applied.
    ///
    /// **Not atomic** ([`ChainState::merge_partial`]): on an error, the entries
    /// before the one refused stay applied. A verifier acts on everything that
    /// took effect — its seq before and after — never only on `Ok` (M2
    /// re-review: a join followed by a spoiled entry in one batch was applied,
    /// the batch "failed", and nobody was told about the join).
    pub fn merge(&mut self, entries: &[Entry], clock: &mut ChainClock) -> Result<usize, ChainError> {
        let merged = self.merge_partial(entries, clock);
        match merged.error {
            Some(error) => Err(error),
            None => Ok(merged.applied),
        }
    }

    /// [`ChainState::merge`], saying both how far it got and why it stopped.
    ///
    /// Partial on purpose rather than all-or-nothing: a recovery join followed
    /// by a self-activation this verifier's clock says is too early must leave
    /// the join applied — first seen now, alerted about, and vetoable — or the
    /// verifier would never see the join at all, and so never be able to take
    /// the activation either (the delay runs from its first sighting).
    pub fn merge_partial(&mut self, entries: &[Entry], clock: &mut ChainClock) -> Merged {
        let mut applied = 0;
        for entry in entries {
            let seq = entry.seq();
            if seq <= self.seq() {
                if self.hashes[seq as usize] != entry.hash() {
                    return Merged {
                        applied,
                        error: Some(ChainError::Fork { seq }),
                    };
                }
                continue;
            }
            if let Err(error) = self.apply(entry, clock) {
                return Merged { applied, error: Some(error) };
            }
            applied += 1;
        }
        Merged { applied, error: None }
    }

    /// The entries after seq `seq` (all of them for `None`): what a verifier
    /// learnt since it stood at `seq`.
    pub fn entries_after(&self, seq: Option<u64>) -> &[Entry] {
        let from = seq.map_or(0, |s| usize::try_from(s.saturating_add(1)).unwrap_or(usize::MAX));
        &self.entries[from.min(self.entries.len())..]
    }

    /// A new device's id and keys: a valid id never used before, canonical
    /// keys of full order, and neither key one a renewal retired
    /// ([`rules::KEY_RETIRED`]). Every entry that names a device's keys —
    /// genesis, `AddDevice`, `AddMachine`, `RecoveryJoin`, `RecoveryJoin3`,
    /// `AddDevice3` (an admin's or a passkey's) — comes through here; an
    /// approval or activation checks the pending device's keys again
    /// ([`ChainState::check_not_retired`]).
    fn check_new(&self, seq: u64, keys: &DeviceKeys) -> Result<(), ChainError> {
        if !valid_id(&keys.id) {
            return Err(ChainError::BadId { seq });
        }
        if self.devices.contains_key(&keys.id) {
            return Err(ChainError::DuplicateId { seq, id: keys.id.clone() });
        }
        if ed25519_key(&keys.sign_key).is_none() || !x25519_valid(&keys.dh_key) {
            return Err(ChainError::WeakKey { seq });
        }
        self.check_not_retired(seq, keys)
    }

    /// Neither of `keys` is one a renewal retired ([`rules::KEY_RETIRED`]):
    /// checked when keys name a new device ([`ChainState::check_new`]) and
    /// again when a pending device becomes active (`ApproveDevice`,
    /// `ActivateRecovered`, `PasskeyActivate`) — a join made with a host's
    /// key before its renewal does not come alive after it.
    fn check_not_retired(&self, seq: u64, keys: &DeviceKeys) -> Result<(), ChainError> {
        if self.is_retired_key(&keys.sign_key) || self.is_retired_key(&keys.dh_key) {
            return Err(not_allowed(seq, rules::KEY_RETIRED));
        }
        Ok(())
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn account_key(&self) -> [u8; 32] {
        self.account_key
    }

    pub fn recovery_key(&self) -> Option<[u8; 32]> {
        self.recovery_key
    }

    /// Whether the chain holds a v3 entry (and so takes no more v2 ones).
    pub fn is_v3(&self) -> bool {
        self.v3_from.is_some()
    }

    /// The seq of the chain's first v3 entry.
    pub fn v3_from(&self) -> Option<u64> {
        self.v3_from
    }

    /// A host that joined or restarted hosting in the last entry and still
    /// awaits its machine key epoch: a batch must not end here.
    pub fn host_key_due(&self) -> Option<&str> {
        self.host_key_due.as_deref()
    }

    /// How many user key epochs are committed (the next one's number).
    pub fn user_key_epochs(&self) -> u32 {
        self.user_keys.len() as u32
    }

    /// The newest committed user key epoch.
    pub fn latest_user_epoch(&self) -> Option<u32> {
        self.user_key_epochs().checked_sub(1)
    }

    /// How many key epochs are committed for `machine`.
    pub fn machine_key_epochs(&self, machine: &str) -> u32 {
        self.machine_keys.get(machine).map_or(0, |v| v.len() as u32)
    }

    /// The newest committed key epoch of `machine`.
    pub fn latest_machine_epoch(&self, machine: &str) -> Option<u32> {
        self.machine_key_epochs(machine).checked_sub(1)
    }

    /// `key` is user key epoch `epoch` as the chain committed it.
    pub fn check_user_key(&self, epoch: u32, key: &[u8]) -> Result<(), ChainError> {
        let committed = self
            .user_keys
            .get(epoch as usize)
            .ok_or(ChainError::UncommittedKey { id: String::new(), epoch })?;
        if *committed != user_key_commit(&self.account, epoch, key) {
            return Err(ChainError::KeyMismatch { id: String::new(), epoch });
        }
        Ok(())
    }

    /// `key` is `machine`'s key epoch `epoch` as the chain committed it.
    pub fn check_machine_key(&self, machine: &str, epoch: u32, key: &[u8]) -> Result<(), ChainError> {
        let committed = self
            .machine_keys
            .get(machine)
            .and_then(|v| v.get(epoch as usize))
            .ok_or_else(|| ChainError::UncommittedKey { id: machine.into(), epoch })?;
        if *committed != machine_key_commit(&self.account, machine, epoch, key) {
            return Err(ChainError::KeyMismatch { id: machine.into(), epoch });
        }
        Ok(())
    }

    /// The seq of the last entry.
    pub fn seq(&self) -> u64 {
        self.hashes.len() as u64 - 1
    }

    /// The hash of the last entry.
    pub fn head(&self) -> [u8; 32] {
        *self.hashes.last().expect("a state always has its genesis")
    }

    /// The hash of the genesis entry: what an enrolled agent pins.
    pub fn genesis_hash(&self) -> [u8; 32] {
        self.hashes[0]
    }

    /// Every entry, genesis first: what a verifier persists to re-verify on boot.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The hash of entry `seq`, if this view has it.
    pub fn hash_at(&self, seq: u64) -> Option<[u8; 32]> {
        self.hashes.get(usize::try_from(seq).ok()?).copied()
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            seq: self.seq(),
            head: self.head(),
        }
    }

    /// The last `n` entries: what a device sends an agent that may lag behind.
    pub fn suffix(&self, n: usize) -> &[Entry] {
        &self.entries[self.entries.len().saturating_sub(n)..]
    }

    /// Refuse this chain if it does not extend what was seen before.
    pub fn check_extends(&self, seen: &Checkpoint) -> Result<(), ChainError> {
        if self.seq() < seen.seq {
            return Err(ChainError::Rollback {
                seen: seen.seq,
                offered: self.seq(),
            });
        }
        if self.hashes[seen.seq as usize] != seen.head {
            return Err(ChainError::Fork { seq: seen.seq });
        }
        Ok(())
    }

    /// A current, active device (any flags: hosts are devices too).
    pub fn device(&self, id: &str) -> Option<&DeviceRecord> {
        self.devices.get(id).filter(|d| d.is_active())
    }

    /// Any device the chain has ever named, in whatever state.
    pub fn device_record(&self, id: &str) -> Option<&DeviceRecord> {
        self.devices.get(id)
    }

    /// The identity keys device `id` renewed away from, oldest first (none
    /// for a device the chain does not name).
    pub fn retired_keys(&self, id: &str) -> &[RetiredKeys] {
        self.devices.get(id).map_or(&[], |d| d.retired.as_slice())
    }

    /// Device `id`'s keys as they stood once entry `seq` had taken effect:
    /// the old keys before a renewal, the new ones from it on. `None` before
    /// the device joined, or past this view. What a stored stream head or a
    /// dispute's signed login is checked against
    /// (`docs/host-identity-rotation.md` §3.8, §5.4).
    pub fn keys_at(&self, id: &str, seq: u64) -> Option<DeviceKeys> {
        let d = self.devices.get(id)?;
        if seq < d.added_seq || seq > self.seq() {
            return None;
        }
        let (sign_key, dh_key) = d
            .retired
            .iter()
            .find(|r| seq < r.until_seq)
            .map_or((d.info.sign_key, d.info.dh_key), |r| (r.sign_key, r.dh_key));
        Some(DeviceKeys {
            id: id.to_string(),
            sign_key,
            dh_key,
        })
    }

    /// Whether `signature` disputes the renewal at `seq` of `host`
    /// ([`rotate_dispute_body`]): entry `seq` of this view is a
    /// `RotateHostKey` for `host`, the statement is over that entry's hash and
    /// the sign key it put in, and it is signed by the sign key that entry
    /// retired (`keys_at(host, seq - 1)`). A hub cannot forge one; it can only
    /// withhold it.
    pub fn verify_rotate_dispute(&self, host: &str, seq: u64, signature: &[u8; 64]) -> bool {
        let Some(entry) = usize::try_from(seq).ok().and_then(|i| self.entries.get(i)) else {
            return false;
        };
        let Action::RotateHostKey(r) = &entry.draft.action else {
            return false;
        };
        if r.host != host {
            return false;
        }
        let (Some(hash), Some(retired)) = (self.hash_at(seq), seq.checked_sub(1).and_then(|s| self.keys_at(host, s))) else {
            return false;
        };
        sig::verify(
            &retired.sign_key,
            sig::ROTATE_DISPUTE,
            &rotate_dispute_body(&self.account, host, seq, &hash, &r.sign_key),
            signature,
        )
    }

    /// Whether `key` is one a renewal retired — any device's, sign or DH
    /// ([`RetiredKeys`]): such a key never names a device again
    /// ([`rules::KEY_RETIRED`]).
    pub fn is_retired_key(&self, key: &[u8; 32]) -> bool {
        self.devices
            .values()
            .any(|d| d.retired.iter().any(|r| r.sign_key == *key || r.dh_key == *key))
    }

    /// Whether any device the chain ever named — in whatever state — holds
    /// or held `key`, as its sign or its DH key.
    pub fn ever_named(&self, key: &[u8; 32]) -> bool {
        self.devices.values().any(|d| {
            d.info.sign_key == *key || d.info.dh_key == *key || d.retired.iter().any(|r| r.sign_key == *key || r.dh_key == *key)
        })
    }

    /// A current host: an active device that runs AI (v2: a current machine).
    pub fn machine(&self, id: &str) -> Option<&DeviceRecord> {
        self.devices.get(id).filter(|d| d.is_current_host())
    }

    /// A current admin device (v2: a current full device).
    pub fn admin(&self, id: &str) -> Option<&DeviceRecord> {
        self.devices.get(id).filter(|d| d.is_current_admin())
    }

    /// Every device the chain ever named, hosts included, by id.
    pub fn devices(&self) -> impl Iterator<Item = &DeviceRecord> {
        self.devices.values()
    }

    /// Every device the chain ever named that is (or was, when removed) a
    /// host, by id.
    pub fn hosts(&self) -> impl Iterator<Item = &DeviceRecord> {
        self.devices.values().filter(|d| d.host)
    }

    /// The active admin devices, by id.
    pub fn admins(&self) -> impl Iterator<Item = &DeviceRecord> {
        self.devices.values().filter(|d| d.is_current_admin())
    }

    /// A current passkey slot.
    pub fn slot(&self, slot: &str) -> Option<&SlotRecord> {
        self.slots.get(slot).filter(|s| !s.removed)
    }

    /// Every passkey slot the chain ever named, by id.
    pub fn slots(&self) -> impl Iterator<Item = &SlotRecord> {
        self.slots.values()
    }

    /// Whom host `host` accepts control from besides admin devices.
    pub fn accept_policy(&self, host: &str) -> Accept {
        self.accept.get(host).cloned().unwrap_or_default()
    }

    /// Devices that joined with the recovery key and are not yet active: what
    /// every device and agent should be alerting about.
    pub fn pending_recoveries(&self) -> Vec<(&str, u64)> {
        self.pending(|d| d.via_recovery)
    }

    /// Devices that joined with a passkey slot and are not yet active.
    pub fn pending_passkey_joins(&self) -> Vec<(&str, u64)> {
        self.pending(|d| d.via_passkey.is_some())
    }

    fn pending(&self, which: impl Fn(&DeviceRecord) -> bool) -> Vec<(&str, u64)> {
        self.devices
            .values()
            .filter(|d| which(d))
            .filter_map(|d| match d.status {
                DeviceStatus::Pending { join_seq } => Some((d.info.id.as_str(), join_seq)),
                _ => None,
            })
            .collect()
    }

    /// The check behind a handshake: `dh_key` is the key of current, active
    /// device `id`. A pending device is refused. (Whether the host accepts
    /// control from it is [`ChainState::may_control`].)
    pub fn authorize_device(&self, id: &str, dh_key: &[u8; 32]) -> Result<(), ChainError> {
        match self.device(id) {
            Some(d) if &d.info.dh_key == dh_key => Ok(()),
            _ => Err(ChainError::UnknownDevice(id.to_string())),
        }
    }

    /// Whether device `device` may control host `host` — open a channel to
    /// it at all (`docs/local-aggregator.md` §3.1, "谁可以控制我"): never
    /// itself; only a current device to a current host; an admin device
    /// always; any other device only when the host's [`Accept`] policy names
    /// it (or all). Agents enforce this at admission; the hub trims tickets by
    /// it as defence in depth.
    pub fn may_control(&self, device: &str, host: &str) -> Result<(), ChainError> {
        if device == host {
            return Err(ChainError::SelfControl);
        }
        let requester = self.device(device).ok_or_else(|| ChainError::UnknownDevice(device.to_string()))?;
        let refuse = |reason| ChainError::NotAccepted {
            device: device.to_string(),
            host: host.to_string(),
            reason,
        };
        if self.machine(host).is_none() {
            return Err(refuse("not a current host of this account"));
        }
        if requester.admin {
            return Ok(());
        }
        match self.accept.get(host) {
            None | Some(Accept::Admins) => Err(refuse("the host accepts control only from admin devices")),
            Some(Accept::All) => Ok(()),
            Some(Accept::Devices(ids)) if ids.iter().any(|i| i == device) => Ok(()),
            Some(Accept::Devices(_)) => Err(refuse("the host does not accept control from this device")),
        }
    }

    /// The current hosts that accept control from `device` — each one
    /// [`ChainState::may_control`] allows — in id order: what it may reach,
    /// and so whose machine keys it may hold (review D2: machine keys follow
    /// the accept policy; `docs/local-aggregator.md` S1-4). Never the device
    /// itself; every other current host for an admin; none for a device that
    /// is not current.
    pub fn hosts_admitting<'a>(&'a self, device: &'a str) -> impl Iterator<Item = &'a DeviceRecord> + 'a {
        self.devices
            .values()
            .filter(move |d| d.is_current_host() && self.may_control(device, &d.info.id).is_ok())
    }

    /// [`ChainState::authorize_device`] and [`ChainState::may_control`]: what
    /// an agent checks before it lets a device in.
    pub fn authorize_control(&self, device: &str, dh_key: &[u8; 32], host: &str) -> Result<(), ChainError> {
        if device == host {
            return Err(ChainError::SelfControl);
        }
        self.authorize_device(device, dh_key)?;
        self.may_control(device, host)
    }

    /// The X25519 key of `id` if it is an active *admin* device: the only
    /// kind of sender whose sealed keys a recipient accepts ([`crate::wrap`]).
    pub fn full_device_key(&self, id: &str) -> Result<[u8; 32], ChainError> {
        match self.admin(id) {
            Some(d) => Ok(d.info.dh_key),
            None => Err(ChainError::UnknownDevice(id.to_string())),
        }
    }

    /// Check one attestation against this view: only an admin attests.
    pub fn check_attestation(&self, att: &Attestation) -> Result<(), ChainError> {
        if att.account != self.account {
            return Err(ChainError::BadAttestation("another account"));
        }
        let device = self.admin(&att.device).ok_or(ChainError::BadAttestation("not a current full device"))?;
        let body = Attestation::body(&att.account, &att.device, att.seq, &att.head, att.time_ms);
        if !sig::verify(&device.info.sign_key, sig::CHAIN_ATTEST, &body, &att.signature) {
            return Err(ChainError::BadAttestation("bad signature"));
        }
        match self.hash_at(att.seq) {
            None => Err(ChainError::Behind { seq: att.seq }),
            Some(hash) if hash != att.head => Err(ChainError::Fork { seq: att.seq }),
            Some(_) => Ok(()),
        }
    }

    /// Whether some admin device has vouched for this exact head within
    /// `max_age_ms` — what an agent requires before it opens any channel to
    /// `requester` ([`CONTROL_FRESHNESS_MS`]) and again before each management
    /// request ([`MANAGE_FRESHNESS_MS`]), so that a hub withholding a removal
    /// cannot keep a removed device in business indefinitely.
    ///
    /// Who counts: any current admin device other than the requester, or the
    /// requester itself when it is the sole active admin. (M2 had added a
    /// day's distrust of self-attestations after recoveries and removals; a
    /// thief defeats it in seconds by adding a puppet device, while it locked
    /// the honest user out, so it went in favour of alerting every device and
    /// machine about each change to who is in the account — architecture doc
    /// §7.)
    ///
    /// A fork is an error (raise the alarm); an attestation from beyond this
    /// view is `Behind`.
    pub fn freshness(&self, requester: &str, attestations: &[Attestation], now_ms: u64, max_age_ms: u64) -> Result<Freshness, ChainError> {
        let sole = self.admins().count() == 1;
        let self_counts = sole;
        let mut newest: Option<u64> = None;
        let mut behind: Option<u64> = None;
        for att in attestations {
            match self.check_attestation(att) {
                Ok(()) => {}
                Err(ChainError::Behind { seq }) => {
                    behind = Some(behind.map_or(seq, |b| b.max(seq)));
                    continue;
                }
                Err(fork @ ChainError::Fork { .. }) => return Err(fork),
                Err(_) => continue,
            }
            let counts = att.device != requester || self_counts;
            if !counts || att.seq != self.seq() || att.time_ms > now_ms.saturating_add(ATTESTATION_SKEW_MS) {
                continue;
            }
            newest = Some(newest.map_or(att.time_ms, |n| n.max(att.time_ms)));
        }
        if let Some(seq) = behind {
            return Ok(Freshness::Behind { seq });
        }
        Ok(match newest {
            Some(t) if now_ms.saturating_sub(t) <= max_age_ms => Freshness::Fresh,
            other => Freshness::Stale { newest_ms: other },
        })
    }

    /// The next entry, unsigned. Its version is the action's own, or — for
    /// the four actions both versions share — the chain's (v3 once it holds a
    /// v3 entry).
    pub fn draft(&self, signer: Signer, action: Action, created_ms: u64) -> Draft {
        let version = action.fixed_version().unwrap_or(if self.is_v3() { VERSION_3 } else { VERSION });
        Draft {
            version,
            account: self.account.clone(),
            seq: self.seq() + 1,
            prev: self.head(),
            created_ms,
            signer,
            action,
        }
    }
}

fn keys_of(id: &str, sign_key: &[u8; 32], dh_key: &[u8; 32]) -> DeviceKeys {
    DeviceKeys {
        id: id.to_string(),
        sign_key: *sign_key,
        dh_key: *dh_key,
    }
}

/// The genesis draft for a new v2 account. The creating device signs it with
/// `AK` and then destroys `AK`.
pub fn genesis_draft(account: &str, account_key: [u8; 32], device: DeviceInfo, recovery_key: Option<RecoveryKey>, created_ms: u64) -> Draft {
    Draft {
        version: VERSION,
        account: account.into(),
        seq: 0,
        prev: [0; 32],
        created_ms,
        signer: Signer::Account,
        action: Action::Genesis {
            account_key,
            device,
            recovery_key,
        },
    }
}

/// The genesis draft for a new v3 account (its device must be an admin).
pub fn genesis3_draft(account: &str, account_key: [u8; 32], device: DeviceV3, recovery_key: Option<RecoveryKey>, created_ms: u64) -> Draft {
    Draft {
        version: VERSION_3,
        account: account.into(),
        seq: 0,
        prev: [0; 32],
        created_ms,
        signer: Signer::Account,
        action: Action::Genesis3 {
            account_key,
            device,
            recovery_key,
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::keys::DhKeypair;
    use crate::testing::bytes32;

    pub(crate) struct Fixture {
        pub ak: SigningKey,
        /// The recovery secret the user keeps; `recovery.signing` is RSK.
        pub rk: [u8; 16],
        pub recovery: RecoveryKeys,
        pub devices: Vec<(SigningKey, DhKeypair)>,
        pub machine: (SigningKey, DhKeypair),
    }

    pub(crate) fn fixture() -> Fixture {
        let rk: [u8; 16] = bytes32(150)[..16].try_into().unwrap();
        Fixture {
            ak: SigningKey::from_bytes(&bytes32(100)),
            rk,
            recovery: recovery_keys(&rk),
            devices: (0..4)
                .map(|i| (SigningKey::from_bytes(&bytes32(200 + i)), DhKeypair::from_secret(bytes32(300 + i))))
                .collect(),
            machine: (SigningKey::from_bytes(&bytes32(400)), DhKeypair::from_secret(bytes32(401))),
        }
    }

    pub(crate) fn device_info(f: &Fixture, i: usize, full: bool) -> DeviceInfo {
        DeviceInfo {
            id: format!("dev-{i}"),
            sign_key: f.devices[i].0.verifying_key().to_bytes(),
            dh_key: f.devices[i].1.public(),
            full,
        }
    }

    pub(crate) fn machine_info(f: &Fixture) -> MachineInfo {
        MachineInfo {
            id: "m-1".into(),
            sign_key: f.machine.0.verifying_key().to_bytes(),
            dh_key: f.machine.1.public(),
        }
    }

    pub(crate) fn h() -> ChainClock {
        ChainClock::history()
    }

    /// genesis(dev-0, RSK) → add dev-1 (full) → add m-1 → add dev-2 (limited) → remove dev-1.
    pub(crate) fn sample_chain(f: &Fixture) -> Vec<Entry> {
        let ak = f.ak.verifying_key().to_bytes();
        let rsk = RecoveryKey::new(&f.recovery.signing, "acct-1", &ak);
        let mut entries = vec![genesis_draft("acct-1", ak, device_info(f, 0, true), Some(rsk), 1_000).sign(&f.ak)];
        let mut state = ChainState::verify(&entries, None, &mut h()).unwrap();
        let by_dev0 = Signer::Device("dev-0".into());
        for (action, ms) in [
            (Action::AddDevice(device_info(f, 1, true)), 2_000),
            (Action::AddMachine(machine_info(f)), 3_000),
            (Action::AddDevice(device_info(f, 2, false)), 4_000),
            (Action::RemoveDevice("dev-1".into()), 5_000),
        ] {
            let entry = state.draft(by_dev0.clone(), action, ms).sign(&f.devices[0].0);
            state.apply(&entry, &mut h()).unwrap();
            entries.push(entry);
        }
        entries
    }

    pub(crate) fn recovered_device(f: &Fixture) -> DeviceInfo {
        DeviceInfo {
            id: "dev-r".into(),
            ..device_info(f, 3, true)
        }
    }

    #[test]
    fn a_valid_chain_verifies_and_round_trips() {
        let f = fixture();
        let entries = sample_chain(&f);
        for entry in &entries {
            assert_eq!(&Entry::decode(&entry.encode()).unwrap(), entry);
        }
        let pin = crate::identity::fingerprint(&f.ak.verifying_key().to_bytes());
        let state = ChainState::verify(&entries, Some(&pin), &mut h()).unwrap();
        assert_eq!(state.seq(), 4);
        assert!(state.device("dev-0").is_some());
        assert!(state.device("dev-1").is_none(), "removed");
        assert!(state.device("dev-2").is_some());
        assert!(state.machine("m-1").is_some());
        state.authorize_device("dev-0", &f.devices[0].1.public()).unwrap();
        assert!(
            state.authorize_device("dev-1", &f.devices[1].1.public()).is_err(),
            "a removed device cannot handshake"
        );
        assert!(
            state.authorize_device("dev-0", &f.devices[2].1.public()).is_err(),
            "the key must match the id"
        );
        assert_eq!(state.full_device_key("dev-0"), Ok(f.devices[0].1.public()));
        assert!(state.full_device_key("dev-2").is_err(), "a limited device cannot deliver keys");
        assert_eq!(state.suffix(2), &entries[3..]);
    }

    #[test]
    fn the_pinned_account_key_is_enforced() {
        let f = fixture();
        assert_eq!(
            ChainState::verify(&sample_chain(&f), Some(&[0; 16]), &mut h()),
            Err(ChainError::PinMismatch)
        );
    }

    #[test]
    fn an_enrolled_agent_pins_the_genesis_entry_itself() {
        let f = fixture();
        let entries = sample_chain(&f);
        let fp = crate::identity::fingerprint(&f.ak.verifying_key().to_bytes());
        let root = TrustRoot::from_fingerprint(fp);
        let first = ChainState::verify_rooted(&entries, &root, &mut h()).unwrap();
        let pinned = root.pinned_to(&first);
        assert_eq!(pinned.genesis_hash, Some(entries[0].hash()));
        ChainState::verify_rooted(&entries, &pinned, &mut h()).unwrap();
        // A second genesis under the same AK (say it leaked): same fingerprint,
        // different entry. The fingerprint alone would let it through.
        let other = genesis_draft("acct-1", f.ak.verifying_key().to_bytes(), device_info(&f, 3, true), None, 1_000).sign(&f.ak);
        ChainState::verify_rooted(std::slice::from_ref(&other), &root, &mut h()).unwrap();
        assert_eq!(
            ChainState::verify_rooted(std::slice::from_ref(&other), &pinned, &mut h()),
            Err(ChainError::GenesisMismatch)
        );
    }

    #[test]
    fn applying_an_entry_records_when_this_verifier_first_saw_it() {
        let f = fixture();
        let entries = sample_chain(&f);
        let mut clock = ChainClock::live(5_000, 0);
        let mut state = ChainState::verify(&entries[..2], None, &mut clock).unwrap();
        clock.set_now(9_000);
        state.merge(&entries, &mut clock).unwrap();
        assert_eq!(
            clock.first_seen().iter().map(|(&s, &t)| (s, t)).collect::<Vec<_>>(),
            vec![(1, 5_000), (2, 9_000), (3, 9_000), (4, 9_000)]
        );
        clock.set_now(10_000);
        state.merge(&entries, &mut clock).unwrap();
        assert_eq!(clock.first_seen()[&2], 9_000, "a second sighting changes nothing");
        assert!(clock.is_live() && !ChainClock::history().is_live());
    }

    #[test]
    fn tampering_with_any_entry_is_detected() {
        let f = fixture();
        let entries = sample_chain(&f);
        for i in 0..entries.len() {
            let mut bytes = entries[i].encode();
            for at in [bytes.len() - 70, bytes.len() - 1] {
                bytes[at] ^= 1;
                let mut tampered = entries.clone();
                if let Ok(entry) = Entry::decode(&bytes) {
                    tampered[i] = entry;
                    assert!(ChainState::verify(&tampered, None, &mut h()).is_err(), "entry {i} byte {at}");
                }
                bytes[at] ^= 1;
            }
        }
    }

    #[test]
    fn reordering_dropping_and_splicing_are_detected() {
        let f = fixture();
        let entries = sample_chain(&f);
        let mut swapped = entries.clone();
        swapped.swap(1, 2);
        assert!(matches!(ChainState::verify(&swapped, None, &mut h()), Err(ChainError::OutOfOrder { .. })));
        let mut dropped = entries.clone();
        dropped.remove(2);
        assert!(matches!(ChainState::verify(&dropped, None, &mut h()), Err(ChainError::OutOfOrder { .. })));
        let mut state = ChainState::verify(&entries[..2], None, &mut h()).unwrap();
        let mut other = state.draft(Signer::Device("dev-0".into()), Action::AddMachine(machine_info(&f)), 0);
        other.prev = [7; 32];
        assert_eq!(
            state.apply(&other.sign(&f.devices[0].0), &mut h()),
            Err(ChainError::BrokenLink { seq: 2 })
        );
    }

    #[test]
    fn the_account_key_signs_genesis_and_nothing_else() {
        let f = fixture();
        let mut state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        for action in [
            Action::RecoveryJoin(recovered_device(&f)),
            Action::RemoveMachine("m-1".into()),
            Action::AddDevice(recovered_device(&f)),
        ] {
            let by_ak = state.draft(Signer::Account, action, 0).sign(&f.ak);
            assert!(matches!(state.apply(&by_ak, &mut h()), Err(ChainError::NotAllowed { .. })));
        }
    }

    #[test]
    fn only_current_active_full_devices_may_sign() {
        let f = fixture();
        let mut state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let by_removed = state
            .draft(Signer::Device("dev-1".into()), Action::RemoveDevice("dev-0".into()), 0)
            .sign(&f.devices[1].0);
        assert!(matches!(state.apply(&by_removed, &mut h()), Err(ChainError::UnknownId { .. })));
        let by_limited = state
            .draft(Signer::Device("dev-2".into()), Action::RemoveMachine("m-1".into()), 0)
            .sign(&f.devices[2].0);
        assert!(matches!(state.apply(&by_limited, &mut h()), Err(ChainError::NotAllowed { .. })));
        let forged = state
            .draft(Signer::Device("dev-0".into()), Action::RemoveMachine("m-1".into()), 0)
            .sign(&f.devices[1].0);
        assert!(matches!(state.apply(&forged, &mut h()), Err(ChainError::BadSignature { .. })));
        let by_rsk = state
            .draft(Signer::Recovery, Action::RemoveMachine("m-1".into()), 0)
            .sign(&f.recovery.signing);
        assert!(matches!(state.apply(&by_rsk, &mut h()), Err(ChainError::NotAllowed { .. })));
    }

    #[test]
    fn removing_the_device_that_set_the_recovery_key_or_came_back_with_it_clears_it() {
        let f = fixture();
        // Genesis (dev-0) set it; the sample chain removes only dev-1.
        let mut state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        assert!(state.recovery_key().is_some(), "dev-1 neither set it nor recovered with it");
        // A recovered device, approved, then removed: it knew RK, so RK stops counting.
        let join = state
            .draft(Signer::Recovery, Action::RecoveryJoin(recovered_device(&f)), 6_000)
            .sign(&f.recovery.signing);
        state.apply(&join, &mut h()).unwrap();
        let ok = state
            .draft(Signer::Device("dev-0".into()), Action::ApproveDevice("dev-r".into()), 6_001)
            .sign(&f.devices[0].0);
        state.apply(&ok, &mut h()).unwrap();
        let mut removed = state.clone();
        let rm = removed
            .draft(Signer::Device("dev-0".into()), Action::RemoveDevice("dev-r".into()), 6_002)
            .sign(&f.devices[0].0);
        removed.apply(&rm, &mut h()).unwrap();
        assert_eq!(removed.recovery_key(), None);
        // With RK cleared, the recovery key can no longer bring anyone in.
        let again = removed
            .draft(
                Signer::Recovery,
                Action::RecoveryJoin(DeviceInfo {
                    id: "dev-q".into(),
                    ..recovered_device(&f)
                }),
                6_003,
            )
            .sign(&f.recovery.signing);
        assert!(matches!(removed.apply(&again, &mut h()), Err(ChainError::NotAllowed { .. })));
        // dev-r sets a new key, then dev-0 removes dev-r: the key it set goes too.
        let new_rsk = SigningKey::from_bytes(&bytes32(160));
        let ak = f.ak.verifying_key().to_bytes();
        let set = state
            .draft(
                Signer::Device("dev-r".into()),
                Action::SetRecoveryKey(Some(RecoveryKey::new(&new_rsk, "acct-1", &ak))),
                6_004,
            )
            .sign(&f.devices[3].0);
        state.apply(&set, &mut h()).unwrap();
        assert_eq!(state.recovery_key(), Some(new_rsk.verifying_key().to_bytes()));
        let rm = state
            .draft(Signer::Device("dev-0".into()), Action::RemoveDevice("dev-r".into()), 6_005)
            .sign(&f.devices[0].0);
        state.apply(&rm, &mut h()).unwrap();
        assert_eq!(state.recovery_key(), None);
        // A key set by a device that stays is kept when another is removed.
        let f2 = fixture();
        let mut s2 = ChainState::verify(&sample_chain(&f2)[..4], None, &mut h()).unwrap();
        let set = s2
            .draft(
                Signer::Device("dev-0".into()),
                Action::SetRecoveryKey(Some(RecoveryKey::new(&new_rsk, "acct-1", &ak))),
                7_000,
            )
            .sign(&f2.devices[0].0);
        s2.apply(&set, &mut h()).unwrap();
        let rm = s2
            .draft(Signer::Device("dev-0".into()), Action::RemoveDevice("dev-1".into()), 7_001)
            .sign(&f2.devices[0].0);
        s2.apply(&rm, &mut h()).unwrap();
        assert!(s2.recovery_key().is_some());
    }

    #[test]
    fn a_sole_full_device_vouches_for_itself_and_any_other_full_device_counts() {
        let f = fixture();
        let entries = sample_chain(&f);
        let mut clock = ChainClock::live(1_000, 0);
        let mut state = ChainState::verify(&entries[..4], None, &mut clock).unwrap();
        let day = RECOVERY_DELAY_MS;
        // dev-0 removes dev-1 and is the sole full device: its own word counts
        // at once (the M2 day of distrust is gone; alerts replace it).
        clock.set_now(10_000);
        state.apply(&entries[4], &mut clock).unwrap();
        let own = Attestation::create(&state, "dev-0", &f.devices[0].0, 10_000);
        assert_eq!(state.freshness("dev-0", std::slice::from_ref(&own), 10_000, day), Ok(Freshness::Fresh));
        // With a second full device (a recovered one, approved), each needs the other.
        let join = state
            .draft(Signer::Recovery, Action::RecoveryJoin(recovered_device(&f)), 0)
            .sign(&f.recovery.signing);
        state.apply(&join, &mut clock).unwrap();
        let ok = state
            .draft(Signer::Device("dev-0".into()), Action::ApproveDevice("dev-r".into()), 0)
            .sign(&f.devices[0].0);
        state.apply(&ok, &mut clock).unwrap();
        let t = clock.now_ms();
        let by_r = Attestation::create(&state, "dev-r", &f.devices[3].0, t);
        let by_0 = Attestation::create(&state, "dev-0", &f.devices[0].0, t);
        assert_eq!(state.freshness("dev-0", std::slice::from_ref(&by_r), t, day), Ok(Freshness::Fresh));
        assert_eq!(state.freshness("dev-r", std::slice::from_ref(&by_0), t, day), Ok(Freshness::Fresh));
        assert_eq!(
            state.freshness("dev-r", std::slice::from_ref(&by_r), t, day),
            Ok(Freshness::Stale { newest_ms: None })
        );
        // Too old an attestation does not count.
        assert_eq!(
            state.freshness("dev-0", std::slice::from_ref(&by_r), t + day + 1, day),
            Ok(Freshness::Stale { newest_ms: Some(t) })
        );
    }

    #[test]
    fn key_epochs_go_one_at_a_time_and_keys_are_checked_against_them() {
        let f = fixture();
        let mut state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let by0 = Signer::Device("dev-0".into());
        let uk0 = [1u8; 32];
        let mk0 = [2u8; 32];
        let commit = |user: Option<(u32, [u8; 32])>, machines: Vec<(&str, u32, [u8; 32])>| KeyCommits {
            user: user.map(|(epoch, key)| KeyCommit {
                epoch,
                hash: user_key_commit("acct-1", epoch, &key),
            }),
            machines: machines
                .into_iter()
                .map(|(m, epoch, key)| {
                    (
                        m.to_string(),
                        KeyCommit {
                            epoch,
                            hash: machine_key_commit("acct-1", m, epoch, &key),
                        },
                    )
                })
                .collect(),
        };
        let first = state
            .draft(by0.clone(), Action::KeyEpochs(commit(Some((0, uk0)), vec![("m-1", 0, mk0)])), 0)
            .sign(&f.devices[0].0);
        state.apply(&first, &mut h()).unwrap();
        assert_eq!(Entry::decode(&first.encode()).unwrap(), first);
        assert_eq!((state.latest_user_epoch(), state.latest_machine_epoch("m-1")), (Some(0), Some(0)));
        state.check_user_key(0, &uk0).unwrap();
        state.check_machine_key("m-1", 0, &mk0).unwrap();
        assert_eq!(
            state.check_user_key(0, &[9; 32]),
            Err(ChainError::KeyMismatch { id: String::new(), epoch: 0 })
        );
        assert_eq!(
            state.check_machine_key("m-1", 1, &mk0),
            Err(ChainError::UncommittedKey { id: "m-1".into(), epoch: 1 })
        );
        assert!(state.check_machine_key("m-1", 0, &uk0).is_err(), "a user key is not a machine key");
        // Skipping an epoch, repeating one, an unknown machine, a removed signer, nothing at all: refused.
        for bad in [
            commit(Some((2, uk0)), vec![]),
            commit(Some((0, uk0)), vec![]),
            commit(None, vec![("m-1", u32::MAX, mk0)]),
            commit(None, vec![("m-1", 1, mk0), ("m-1", 2, mk0)]),
            commit(None, vec![("m-9", 0, mk0)]),
            KeyCommits::default(),
        ] {
            let e = state.draft(by0.clone(), Action::KeyEpochs(bad), 0).sign(&f.devices[0].0);
            assert!(state.clone().apply(&e, &mut h()).is_err());
        }
        let by_removed = state
            .draft(Signer::Device("dev-1".into()), Action::KeyEpochs(commit(Some((1, uk0)), vec![])), 0)
            .sign(&f.devices[1].0);
        assert!(matches!(state.clone().apply(&by_removed, &mut h()), Err(ChainError::UnknownId { .. })));
        let next = state
            .draft(by0, Action::KeyEpochs(commit(Some((1, [3; 32])), vec![("m-1", 1, [4; 32])])), 0)
            .sign(&f.devices[0].0);
        state.apply(&next, &mut h()).unwrap();
        assert_eq!((state.user_key_epochs(), state.machine_key_epochs("m-1")), (2, 2));
    }

    #[test]
    fn a_recovery_key_needs_its_proof_over_this_account() {
        let f = fixture();
        let ak = f.ak.verifying_key().to_bytes();
        let good = RecoveryKey::new(&f.recovery.signing, "acct-1", &ak);
        // Another account's proof, or another root's, does not hold here.
        for bad in [
            RecoveryKey::new(&f.recovery.signing, "acct-2", &ak),
            RecoveryKey::new(&f.recovery.signing, "acct-1", &[7; 32]),
            RecoveryKey { proof: [0; 64], ..good },
        ] {
            let g = genesis_draft("acct-1", ak, device_info(&f, 0, true), Some(bad), 1_000).sign(&f.ak);
            assert!(matches!(ChainState::genesis(&g, None), Err(ChainError::NotAllowed { seq: 0, .. })));
        }
        let mut state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let bad = RecoveryKey::new(&SigningKey::from_bytes(&bytes32(161)), "acct-1", &[7; 32]);
        let set = state
            .draft(Signer::Device("dev-0".into()), Action::SetRecoveryKey(Some(bad)), 0)
            .sign(&f.devices[0].0);
        assert!(matches!(state.apply(&set, &mut h()), Err(ChainError::NotAllowed { .. })));
    }

    #[test]
    fn a_recovered_device_stays_pending_until_approved_and_cannot_act() {
        let f = fixture();
        let mut state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let join = state
            .draft(Signer::Recovery, Action::RecoveryJoin(recovered_device(&f)), 6_000)
            .sign(&f.recovery.signing);
        state.apply(&join, &mut h()).unwrap();
        assert_eq!(state.pending_recoveries(), vec![("dev-r", 5)]);
        assert!(
            state.authorize_device("dev-r", &f.devices[3].1.public()).is_err(),
            "pending: no handshakes"
        );
        let kick = state
            .draft(Signer::Device("dev-r".into()), Action::RemoveDevice("dev-0".into()), 6_001)
            .sign(&f.devices[3].0);
        assert!(
            matches!(state.apply(&kick, &mut h()), Err(ChainError::UnknownId { .. })),
            "pending: no signing"
        );
        let fake = state
            .draft(
                Signer::Device("dev-0".into()),
                Action::RecoveryJoin(DeviceInfo {
                    id: "dev-z".into(),
                    ..recovered_device(&f)
                }),
                0,
            )
            .sign(&f.devices[0].0);
        assert!(
            matches!(state.apply(&fake, &mut h()), Err(ChainError::NotAllowed { .. })),
            "only RSK signs recovery entries"
        );
        state
            .apply(
                &state
                    .draft(Signer::Device("dev-0".into()), Action::ApproveDevice("dev-r".into()), 6_002)
                    .sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        state.authorize_device("dev-r", &f.devices[3].1.public()).unwrap();
        assert!(state.device("dev-r").unwrap().via_recovery);
    }

    #[test]
    fn self_activation_waits_out_the_delay_on_the_verifiers_own_clock() {
        let f = fixture();
        let entries = sample_chain(&f);
        let base = ChainState::verify(&entries, None, &mut h()).unwrap();
        let join = base
            .draft(Signer::Recovery, Action::RecoveryJoin(recovered_device(&f)), 6_000)
            .sign(&f.recovery.signing);
        let mut after_join = base.clone();
        after_join.apply(&join, &mut h()).unwrap();
        let activate = after_join
            .draft(Signer::Recovery, Action::ActivateRecovered("dev-r".into()), 6_001)
            .sign(&f.recovery.signing);

        // A live verifier (an agent) first sees the join at t = 1000.
        let mut clock = ChainClock::live(1_000, 5);
        let mut state = base.clone();
        clock.observe(5);
        state.apply(&join, &mut clock).unwrap();
        clock.set_now(1_000 + RECOVERY_DELAY_MS - 1);
        assert_eq!(
            state.apply(&activate, &mut clock),
            Err(ChainError::TooEarly {
                seq: 6,
                ready_ms: 1_000 + RECOVERY_DELAY_MS
            })
        );
        assert!(state.device("dev-r").is_none(), "nothing changed");
        clock.set_now(1_000 + RECOVERY_DELAY_MS);
        state.apply(&activate, &mut clock).unwrap();
        assert!(state.device("dev-r").is_some());

        // A device bootstrapping later takes the history as it is.
        let mut history = entries.clone();
        history.extend([join.clone(), activate.clone()]);
        assert!(ChainState::verify(&history, None, &mut h()).unwrap().device("dev-r").is_some());

        // Within the window, any full device vetoes the join.
        let mut vetoed = after_join.clone();
        vetoed
            .apply(
                &vetoed
                    .draft(Signer::Device("dev-0".into()), Action::RemoveDevice("dev-r".into()), 7_000)
                    .sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        assert!(vetoed.pending_recoveries().is_empty());
        let late = vetoed
            .draft(Signer::Recovery, Action::ActivateRecovered("dev-r".into()), 8_000)
            .sign(&f.recovery.signing);
        assert!(matches!(vetoed.apply(&late, &mut h()), Err(ChainError::NotAllowed { .. })));
    }

    #[test]
    fn the_recovery_key_can_be_replaced_or_removed_by_a_full_device() {
        let f = fixture();
        let mut state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let new_rk = recovery_keys(&[9; 16]);
        state
            .apply(
                &state
                    .draft(
                        Signer::Device("dev-0".into()),
                        Action::SetRecoveryKey(Some(RecoveryKey::new(&new_rk.signing, "acct-1", &f.ak.verifying_key().to_bytes()))),
                        0,
                    )
                    .sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        let old = state
            .draft(Signer::Recovery, Action::RecoveryJoin(recovered_device(&f)), 0)
            .sign(&f.recovery.signing);
        assert!(
            matches!(state.apply(&old, &mut h()), Err(ChainError::BadSignature { .. })),
            "the old RK is dead"
        );
        state
            .apply(
                &state
                    .draft(Signer::Device("dev-0".into()), Action::SetRecoveryKey(None), 0)
                    .sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        let any = state
            .draft(Signer::Recovery, Action::RecoveryJoin(recovered_device(&f)), 0)
            .sign(&new_rk.signing);
        assert!(matches!(state.apply(&any, &mut h()), Err(ChainError::NotAllowed { .. })));
    }

    #[test]
    fn ids_are_never_reused_and_the_last_full_device_stays() {
        let f = fixture();
        let mut state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let reuse = state
            .draft(Signer::Device("dev-0".into()), Action::AddDevice(device_info(&f, 1, true)), 0)
            .sign(&f.devices[0].0);
        assert!(matches!(state.apply(&reuse, &mut h()), Err(ChainError::DuplicateId { .. })));
        let last = state
            .draft(Signer::Device("dev-0".into()), Action::RemoveDevice("dev-0".into()), 0)
            .sign(&f.devices[0].0);
        assert_eq!(state.apply(&last, &mut h()), Err(ChainError::LastFullDevice { seq: 5 }));
        let late = state
            .draft(
                Signer::Account,
                Action::Genesis {
                    account_key: [1; 32],
                    device: device_info(&f, 0, true),
                    recovery_key: None,
                },
                0,
            )
            .sign(&f.ak);
        assert!(matches!(state.apply(&late, &mut h()), Err(ChainError::LateGenesis { .. })));
    }

    #[test]
    fn weak_and_non_canonical_keys_are_refused() {
        let f = fixture();
        let mut state = ChainState::verify(&sample_chain(&f)[..1], None, &mut h()).unwrap();
        let mut identity = [0u8; 32];
        identity[0] = 1;
        let mut high_bit = f.devices[1].1.public();
        high_bit[31] |= 0x80;
        for device in [
            DeviceInfo {
                id: "w1".into(),
                sign_key: identity,
                dh_key: f.devices[1].1.public(),
                full: true,
            },
            DeviceInfo {
                id: "w2".into(),
                sign_key: f.devices[1].0.verifying_key().to_bytes(),
                dh_key: [0; 32],
                full: true,
            },
            DeviceInfo {
                id: "w3".into(),
                sign_key: f.devices[1].0.verifying_key().to_bytes(),
                dh_key: high_bit,
                full: true,
            },
        ] {
            let entry = state
                .draft(Signer::Device("dev-0".into()), Action::AddDevice(device), 0)
                .sign(&f.devices[0].0);
            assert!(matches!(state.apply(&entry, &mut h()), Err(ChainError::WeakKey { .. })));
        }
    }

    #[test]
    fn rollback_forks_and_merges() {
        let f = fixture();
        let entries = sample_chain(&f);
        let full = ChainState::verify(&entries, None, &mut h()).unwrap();
        let seen = full.checkpoint();
        let old = ChainState::verify(&entries[..4], None, &mut h()).unwrap();
        assert_eq!(old.check_extends(&seen), Err(ChainError::Rollback { seen: 4, offered: 3 }));
        let mut fork = old.clone();
        fork.apply(
            &fork
                .draft(Signer::Device("dev-0".into()), Action::RemoveMachine("m-1".into()), 9)
                .sign(&f.devices[0].0),
            &mut h(),
        )
        .unwrap();
        assert_eq!(fork.check_extends(&seen), Err(ChainError::Fork { seq: 4 }));
        let mut lagging = old.clone();
        assert_eq!(lagging.merge(full.suffix(3), &mut h()), Ok(1));
        assert_eq!(lagging.checkpoint(), seen);
        assert_eq!(lagging.clone().merge(fork.suffix(1), &mut h()), Err(ChainError::Fork { seq: 4 }));
    }

    #[test]
    fn attestations_say_whether_someone_else_vouched_for_this_head() {
        let f = fixture();
        let entries = sample_chain(&f);
        let mut state = ChainState::verify(&entries, None, &mut h()).unwrap();
        // dev-0 is the only full device left, so its own attestation counts for it.
        let att0 = Attestation::create(&state, "dev-0", &f.devices[0].0, 10_000);
        assert_eq!(Attestation::decode(&att0.encode()).unwrap(), att0);
        assert_eq!(
            state.freshness("dev-0", std::slice::from_ref(&att0), 11_000, 3_600_000),
            Ok(Freshness::Fresh)
        );
        assert_eq!(
            state.freshness("dev-0", std::slice::from_ref(&att0), 10_000 + 3_600_001, 3_600_000),
            Ok(Freshness::Stale { newest_ms: Some(10_000) })
        );
        // With a second full device, a requester's own attestation does not count.
        state
            .apply(
                &state
                    .draft(Signer::Device("dev-0".into()), Action::AddDevice(device_info(&f, 3, true)), 0)
                    .sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        let own = Attestation::create(&state, "dev-0", &f.devices[0].0, 20_000);
        let other = Attestation::create(&state, "dev-3", &f.devices[3].0, 20_000);
        assert_eq!(
            state.freshness("dev-0", std::slice::from_ref(&own), 20_001, 3_600_000),
            Ok(Freshness::Stale { newest_ms: None })
        );
        assert_eq!(state.freshness("dev-0", &[own, other], 20_001, 3_600_000), Ok(Freshness::Fresh));
        // An attestation of entries this view lacks means: catch up first.
        let stale = ChainState::verify(&entries, None, &mut h()).unwrap();
        let behind = Attestation::create(&state, "dev-0", &f.devices[0].0, 20_000);
        assert_eq!(stale.freshness("dev-2", &[behind], 20_001, 3_600_000), Ok(Freshness::Behind { seq: 5 }));
        let mut forged = Attestation::create(&state, "dev-3", &f.devices[3].0, 20_000);
        forged.signature[0] ^= 1;
        assert_eq!(state.check_attestation(&forged), Err(ChainError::BadAttestation("bad signature")));
    }
}
