//! Curve25519 key helpers shared by the channel, the chain and key wrapping.
//!
//! Two checks here exist so that the Rust half and the browser half agree on
//! what a *valid* key is, rather than each accepting whatever its library does:
//!
//! - an X25519 public key of low order is refused everywhere (WebCrypto throws
//!   when such a key yields the all-zero secret; `snow` would silently use it);
//! - Ed25519 verification is `verify_strict` — canonical `S`, no small-order `A`
//!   or `R` — and the TypeScript half adds the same checks in front of
//!   WebCrypto's `verify`.

use ed25519_dalek::{Signature, VerifyingKey};
use rand_core::{CryptoRng, RngCore};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// An X25519 key pair: a device's or machine's Noise static key, or a one-off
/// key for wrapping.
pub struct DhKeypair {
    secret: Zeroizing<[u8; 32]>,
    public: [u8; 32],
}

impl DhKeypair {
    pub fn from_secret(secret: [u8; 32]) -> Self {
        let public = PublicKey::from(&StaticSecret::from(secret)).to_bytes();
        Self {
            secret: Zeroizing::new(secret),
            public,
        }
    }

    pub fn generate<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let mut secret = [0u8; 32];
        rng.fill_bytes(&mut secret);
        let pair = Self::from_secret(secret);
        secret.fill(0);
        pair
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    /// The private scalar, for handing to `snow`. Keep the borrow short.
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// `X25519(self, peer)`, or `None` when `peer` is of low order (the result
    /// would be all zeros, i.e. no secret at all).
    pub fn agree(&self, peer: &[u8; 32]) -> Option<Zeroizing<[u8; 32]>> {
        let shared = StaticSecret::from(*self.secret).diffie_hellman(&PublicKey::from(*peer));
        shared.was_contributory().then(|| Zeroizing::new(shared.to_bytes()))
    }
}

impl std::fmt::Debug for DhKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DhKeypair").field("public", &self.public).finish_non_exhaustive()
    }
}

/// The field prime 2^255 - 19, little-endian.
const P: [u8; 32] = [
    0xed, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f,
];

/// Whether a little-endian field element, top bit ignored, is below p: the one
/// canonical spelling. Libraries differ on whether they accept the other
/// spellings (and WebCrypto implementations differ among themselves), so every
/// key that enters the protocol must be canonical and both halves say so.
fn canonical_field(bytes: &[u8; 32]) -> bool {
    let mut b = *bytes;
    b[31] &= 0x7f;
    for i in (0..32).rev() {
        if b[i] != P[i] {
            return b[i] < P[i];
        }
    }
    false
}

/// Whether `key` is an X25519 public key of low order (it would make every
/// shared secret with it all zeros).
pub fn x25519_is_low_order(key: &[u8; 32]) -> bool {
    // Any clamped scalar is a multiple of the cofactor, so it sends exactly the
    // low-order points to the identity.
    !StaticSecret::from([0u8; 32]).diffie_hellman(&PublicKey::from(*key)).was_contributory()
}

/// An X25519 public key the protocol accepts: canonical (top bit clear, `u < p`)
/// and not of low order.
pub fn x25519_valid(key: &[u8; 32]) -> bool {
    key[31] & 0x80 == 0 && canonical_field(key) && !x25519_is_low_order(key)
}

/// A public key that is canonically encoded, decodes, and is not of small order.
pub fn ed25519_key(key: &[u8; 32]) -> Option<VerifyingKey> {
    if !canonical_field(key) {
        return None;
    }
    VerifyingKey::from_bytes(key).ok().filter(|k| !k.is_weak())
}

/// Strict Ed25519 verification: canonical `S`, and neither the key nor `R` of
/// small order.
pub fn ed25519_verify(key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    match ed25519_key(key) {
        Some(key) => key.verify_strict(message, &Signature::from_bytes(signature)).is_ok(),
        None => false,
    }
}

/// The DER prefix of a PKCS#8 v1 `OneAsymmetricKey` holding an Ed25519 seed
/// (RFC 8410). WebCrypto can only import a private Ed25519 key as PKCS#8 or
/// JWK, so a wrapped account key travels in this form, which lets a browser
/// `unwrapKey` it straight into a non-extractable `CryptoKey`.
pub const ED25519_PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];
/// The same for an X25519 private key.
pub const X25519_PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04, 0x22, 0x04, 0x20,
];

#[cfg(test)]
fn ed25519_pkcs8(seed: &[u8; 32]) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(48));
    out.extend_from_slice(&ED25519_PKCS8_PREFIX);
    out.extend_from_slice(seed);
    out
}

pub fn ed25519_seed_from_pkcs8(der: &[u8]) -> Option<Zeroizing<[u8; 32]>> {
    if der.len() != 48 || der[..16] != ED25519_PKCS8_PREFIX {
        return None;
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    seed.copy_from_slice(&der[16..]);
    Some(seed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn low_order_x25519_keys_are_recognised() {
        assert!(x25519_is_low_order(&[0; 32]));
        let mut one = [0u8; 32];
        one[0] = 1;
        assert!(x25519_is_low_order(&one));
        assert!(!x25519_is_low_order(&DhKeypair::from_secret([7; 32]).public()));
        assert!(DhKeypair::from_secret([7; 32]).agree(&[0; 32]).is_none());
    }

    #[test]
    fn strict_verification_refuses_a_non_canonical_s() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let public = key.verifying_key().to_bytes();
        let mut sig = key.sign(b"m").to_bytes();
        assert!(ed25519_verify(&public, b"m", &sig));
        // S + L: the same signature, spelt a second way.
        const L: [u8; 32] = [
            0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0x10,
        ];
        let mut carry = 0u16;
        for i in 0..32 {
            let sum = u16::from(sig[32 + i]) + u16::from(L[i]) + carry;
            sig[32 + i] = sum as u8;
            carry = sum >> 8;
        }
        assert!(!ed25519_verify(&public, b"m", &sig));
    }

    #[test]
    fn small_order_ed25519_keys_are_refused() {
        // The identity point.
        let mut identity = [0u8; 32];
        identity[0] = 1;
        assert!(ed25519_key(&identity).is_none());
    }

    #[test]
    fn pkcs8_round_trips() {
        let der = ed25519_pkcs8(&[9; 32]);
        assert_eq!(der.len(), 48);
        assert_eq!(*ed25519_seed_from_pkcs8(&der).unwrap(), [9; 32]);
        assert!(ed25519_seed_from_pkcs8(&der[..47]).is_none());
    }
}
