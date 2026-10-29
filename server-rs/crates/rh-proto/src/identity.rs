//! A machine's identity keys, key fingerprints, and enrollment codes.
//!
//! # Machine identity
//!
//! An agent has one Ed25519 key (it answers the hub's sign-in challenge and
//! signs stream heads and push payloads) and one X25519 key (its Noise static
//! key, and what deliveries are sealed to). Both are expanded with HKDF from a
//! single random 32-byte seed the agent keeps (`server-rs/crates/droppen-agent`). The seed
//! is the only secret; where it is kept is the agent's concern.
//!
//! # Enrollment codes
//!
//! `RH1-<account fingerprint>-<secret>`, both 16 bytes of base32. `DR1-` is
//! read as the same version (either, in any case); codes are still written
//! `RH1-`. The device that adds the machine generates the 128-bit `secret` and
//! shows the code; the user types it on the machine. The secret never reaches
//! the hub:
//!
//! - the hub routes the enrollment by `token = HMAC(secret, "rh/v1
//!   enroll-token")[..16]`, which the device registered with it;
//! - the agent proves it holds the code with
//!   `HMAC(secret, "rh/v1 enroll-proof\0" ‖ account ‖ machine id ‖ sign key ‖ dh key ‖ token)`,
//!   which the device checks before it signs "add machine" with exactly those keys;
//! - the agent pins the account fingerprint, so a hub cannot substitute its own
//!   trust root either.
//!
//! So a hub that issues codes cannot enroll a machine of its own (it cannot
//! produce the proof), and there is no short code for anyone to grind.

use ed25519_dalek::SigningKey;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::chain::MachineInfo;
use crate::codec::{base32, put_str16};
use crate::keys::DhKeypair;

const MACHINE_SALT: &[u8] = b"rh/v1 machine";
const FINGERPRINT_LABEL: &[u8] = b"rh/v1 fingerprint\0";
const TOKEN_LABEL: &[u8] = b"rh/v1 enroll-token";
const PROOF_LABEL: &[u8] = b"rh/v1 enroll-proof\0";

/// The first 16 bytes of a labelled SHA-256 of a public key.
pub fn fingerprint(key: &[u8; 32]) -> [u8; 16] {
    let mut h = Sha256::new();
    h.update(FINGERPRINT_LABEL);
    h.update(key);
    let digest = h.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// A key's fingerprint as a person reads it: base32 in groups of four.
pub fn fingerprint_text(key: &[u8; 32]) -> String {
    display_fingerprint(&fingerprint(key))
}

/// A fingerprint (from [`fingerprint`] or an enrollment code) in groups of four.
pub fn display_fingerprint(fp: &[u8; 16]) -> String {
    let text = base32::encode(fp);
    text.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("ascii"))
        .collect::<Vec<_>>()
        .join("-")
}

/// A device's two keys as a person compares them on two screens — what the
/// web app shows in the device list (`deviceFingerprint`): base32 of the first
/// 10 bytes of `SHA-256("rh/v1 fingerprint\0" ‖ sign_key ‖ dh_key)`, in groups
/// of four.
pub fn device_fingerprint_text(sign_key: &[u8; 32], dh_key: &[u8; 32]) -> String {
    let mut h = Sha256::new();
    h.update(FINGERPRINT_LABEL);
    h.update(sign_key);
    h.update(dh_key);
    let digest = h.finalize();
    let text = base32::encode(&digest[..10]);
    text.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).expect("ascii"))
        .collect::<Vec<_>>()
        .join("-")
}

/// An agent's keys.
pub struct MachineIdentity {
    signing: SigningKey,
    dh: DhKeypair,
}

impl MachineIdentity {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(MACHINE_SALT), seed);
        let mut ed = Zeroizing::new([0u8; 32]);
        let mut x = Zeroizing::new([0u8; 32]);
        hk.expand(b"ed25519", ed.as_mut()).expect("32 bytes");
        hk.expand(b"x25519", x.as_mut()).expect("32 bytes");
        Self {
            signing: SigningKey::from_bytes(&ed),
            dh: DhKeypair::from_secret(*x),
        }
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }

    pub fn sign_public(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    pub fn dh(&self) -> &DhKeypair {
        &self.dh
    }

    pub fn dh_public(&self) -> [u8; 32] {
        self.dh.public()
    }

    /// What a device puts in the chain's add-machine entry for this agent.
    pub fn info(&self, machine_id: &str) -> MachineInfo {
        MachineInfo {
            id: machine_id.to_string(),
            sign_key: self.sign_public(),
            dh_key: self.dh_public(),
        }
    }
}

impl std::fmt::Debug for MachineIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineIdentity")
            .field("sign", &fingerprint_text(&self.sign_public()))
            .finish_non_exhaustive()
    }
}

/// `RH1-<account fingerprint>-<secret>` (read as well with `DR1-`).
#[derive(Clone, PartialEq, Eq)]
pub struct EnrollCode {
    pub account_fingerprint: [u8; 16],
    pub secret: [u8; 16],
}

impl std::fmt::Debug for EnrollCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnrollCode")
            .field("account", &display_fingerprint(&self.account_fingerprint))
            .finish_non_exhaustive()
    }
}

fn hmac(secret: &[u8; 16]) -> Hmac<Sha256> {
    <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC takes any key length")
}

impl EnrollCode {
    /// A fresh code for the account whose key has fingerprint `account_fingerprint`.
    pub fn generate<R: RngCore + CryptoRng>(account_fingerprint: [u8; 16], rng: &mut R) -> Self {
        let mut secret = [0u8; 16];
        rng.fill_bytes(&mut secret);
        Self { account_fingerprint, secret }
    }

    /// What the hub knows the enrollment by.
    pub fn token(&self) -> [u8; 16] {
        let mut mac = hmac(&self.secret);
        mac.update(TOKEN_LABEL);
        let out = mac.finalize().into_bytes();
        out[..16].try_into().expect("16 bytes")
    }

    fn proof_mac(&self, account: &str, machine: &str, sign_key: &[u8; 32], dh_key: &[u8; 32]) -> Hmac<Sha256> {
        let mut body = Vec::with_capacity(160);
        put_str16(&mut body, account);
        put_str16(&mut body, machine);
        body.extend_from_slice(sign_key);
        body.extend_from_slice(dh_key);
        body.extend_from_slice(&self.token());
        let mut mac = hmac(&self.secret);
        mac.update(PROOF_LABEL);
        mac.update(&body);
        mac
    }

    /// The agent's proof that it holds this code, bound to the keys it enrolls.
    pub fn proof(&self, account: &str, machine: &str, sign_key: &[u8; 32], dh_key: &[u8; 32]) -> [u8; 32] {
        self.proof_mac(account, machine, sign_key, dh_key).finalize().into_bytes().into()
    }

    /// The device's constant-time check of that proof.
    pub fn verify_proof(&self, account: &str, machine: &str, sign_key: &[u8; 32], dh_key: &[u8; 32], proof: &[u8; 32]) -> bool {
        self.proof_mac(account, machine, sign_key, dh_key).verify_slice(proof).is_ok()
    }
}

/// The version tags an enrollment code may start with, in any case. The first
/// is what [`EnrollCode`]'s `Display` writes.
const ENROLL_CODE_VERSIONS: [&str; 2] = ["RH1", "DR1"];

impl std::fmt::Display for EnrollCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RH1-{}-{}", base32::encode(&self.account_fingerprint), base32::encode(&self.secret))
    }
}

impl std::str::FromStr for EnrollCode {
    type Err = &'static str;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut parts = text.trim().split('-');
        let (Some(version), Some(fp), Some(secret), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            return Err("an enrollment code looks like <version>-<fingerprint>-<secret>");
        };
        if !ENROLL_CODE_VERSIONS.iter().any(|v| version.eq_ignore_ascii_case(v)) {
            return Err("this is not an enrollment code of a version this agent understands");
        }
        let account_fingerprint: [u8; 16] = base32::decode(fp)
            .and_then(|b| b.try_into().ok())
            .ok_or("the fingerprint part is not 16 bytes of base32")?;
        let secret: [u8; 16] = base32::decode(secret)
            .and_then(|b| b.try_into().ok())
            .ok_or("the secret part is not 16 bytes of base32")?;
        Ok(Self { account_fingerprint, secret })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::rng;

    #[test]
    fn the_seed_determines_the_keys() {
        let a = MachineIdentity::from_seed(&[1; 32]);
        let again = MachineIdentity::from_seed(&[1; 32]);
        let other = MachineIdentity::from_seed(&[2; 32]);
        assert_eq!(a.sign_public(), again.sign_public());
        assert_eq!(a.dh_public(), again.dh_public());
        assert_ne!(a.dh_public(), other.dh_public());
        assert_ne!(a.sign_public(), a.dh_public());
    }

    #[test]
    fn enrollment_codes_round_trip_and_reject_junk() {
        let code = EnrollCode::generate(fingerprint(&[9; 32]), &mut rng(1));
        let text = code.to_string();
        assert!(text.starts_with("RH1-"));
        assert_eq!(text.len(), 4 + 26 + 1 + 26);
        assert_eq!(text.parse::<EnrollCode>(), Ok(code.clone()));
        assert_eq!(text.to_lowercase().parse::<EnrollCode>(), Ok(code.clone()));
        // `DR1-` names the same version, in any case; `RH1-` is still what is written.
        let body = &text[4..];
        let dr = format!("DR1-{body}");
        assert_eq!(dr.parse::<EnrollCode>(), Ok(code.clone()));
        assert_eq!(dr.to_lowercase().parse::<EnrollCode>(), Ok(code.clone()));
        assert_eq!(format!(" Dr1-{body} \n").parse::<EnrollCode>(), Ok(code.clone()));
        assert_eq!(dr.parse::<EnrollCode>().unwrap().to_string(), text);
        for bad in [
            "",
            "RH1",
            "DR1",
            "RH2-AAAA-BBBB",
            "RH1-AAAA-BBBB",
            "DR1-AAAA-BBBB",
            &format!("{text}-X"),
            &format!("{dr}-X"),
            &text.replace('A', "1"),
            &format!("DR2-{body}"),
            &format!("DRP1-{body}"),
            &format!("RHP1-{body}"),
            &format!("DRR1-{body}"),
            &format!("RHR1-{body}"),
            &format!("D1-{body}"),
            &format!("RH1DR1-{body}"),
        ] {
            assert!(bad.parse::<EnrollCode>().is_err(), "{bad}");
        }
    }

    #[test]
    fn only_the_holder_of_the_code_can_prove_an_enrollment_for_given_keys() {
        let code = EnrollCode::generate([3; 16], &mut rng(2));
        let id = MachineIdentity::from_seed(&[4; 32]);
        let proof = code.proof("acct", "m-1", &id.sign_public(), &id.dh_public());
        assert!(code.verify_proof("acct", "m-1", &id.sign_public(), &id.dh_public(), &proof));
        // The hub knows the token and the fingerprint, not the secret.
        let hub_guess = EnrollCode {
            secret: [0; 16],
            ..code.clone()
        };
        assert!(!code.verify_proof(
            "acct",
            "m-1",
            &id.sign_public(),
            &id.dh_public(),
            &hub_guess.proof("acct", "m-1", &id.sign_public(), &id.dh_public())
        ));
        // The proof binds the keys and the machine id.
        let other = MachineIdentity::from_seed(&[5; 32]);
        assert!(!code.verify_proof("acct", "m-1", &other.sign_public(), &id.dh_public(), &proof));
        assert!(!code.verify_proof("acct", "m-2", &id.sign_public(), &id.dh_public(), &proof));
        assert_ne!(code.token(), code.secret, "the token is not the secret");
    }

    #[test]
    fn fingerprints_are_grouped_for_reading() {
        let text = fingerprint_text(&[3; 32]);
        assert_eq!(text.split('-').count(), 7);
        assert!(text.split('-').all(|g| g.len() <= 4));
    }
}
