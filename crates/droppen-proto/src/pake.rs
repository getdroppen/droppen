//! CPace over X25519, for pairing a device by a short numeric code (architecture
//! doc §1 "口令配对").
//!
//! The new device shows an 8-digit code; the user types it on a device that is
//! already in the account. The two run CPace through the hub, which relays the
//! messages and so gets exactly **one online guess per run** and nothing it can
//! attack offline. Both ends enforce that: the new device answers one run per
//! code and then burns it, the old one tries one code per entry.
//!
//! Shaped after draft-irtf-cfrg-cpace (CPace-X25519):
//!
//! - the generator is `G = elligator2(SHA-512(DSI ‖ lv(prs) ‖ lv(ci) ‖ lv(sid))[..32])`
//!   on Curve25519's u-line (RFC 9380 §6.7.1; the top bit masked), where `prs`
//!   is the code, `ci` binds the account and the new device's id, and `sid` is
//!   16 random bytes the initiator picks; `lv` is a u32 length then the bytes;
//! - each side picks a scalar `y` and sends `Y = X25519(y, G)`;
//! - `K = X25519(y, Y_peer)` (all-zero, a low-order share, is refused);
//! - `ISK = SHA-512("rh/v1 cpace isk\0" ‖ lv(sid) ‖ K ‖ Y_initiator ‖ Y_responder)[..32]`,
//!   expanded with HKDF into the two confirmation keys and the pairing secret
//!   that plays the QR code's `s` ([`crate::pairing`]).
//!
//! X25519 with an arbitrary point is exactly WebCrypto's `deriveBits`, so the
//! browser needs only Elligator 2 written out (in `BigInt`). Elligator here is
//! not constant-time (`num-bigint`); its input is derived from the one-time
//! code, which a local timing observer would have to recover within its single
//! use — accepted for this prototype and noted in the architecture doc.

use hkdf::Hkdf;
use num_bigint::BigUint;
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha512};
use thiserror::Error;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// The domain separation for the generator.
pub const DSI: &[u8] = b"rh/v1 cpace";
const ISK_LABEL: &[u8] = b"rh/v1 cpace isk\0";
/// The length of a run's session id.
pub const SID_LEN: usize = 16;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PakeError {
    #[error("the other side's share is of low order")]
    LowOrder,
}

/// Which end of a run this is: the initiator picks `sid` and speaks first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

/// What a finished run gives both ends.
pub struct Keys {
    /// The initiator's confirmation key.
    pub confirm_initiator: Zeroizing<[u8; 32]>,
    /// The responder's confirmation key.
    pub confirm_responder: Zeroizing<[u8; 32]>,
    /// The secret that MACs the key delivery (the QR code's `s`).
    pub pair_secret: Zeroizing<[u8; 32]>,
}

fn p() -> BigUint {
    (BigUint::from(1u8) << 255u32) - BigUint::from(19u8)
}

fn lv(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

fn decode_le(bytes: &[u8; 32]) -> BigUint {
    BigUint::from_bytes_le(bytes)
}

fn encode_le(x: &BigUint) -> [u8; 32] {
    let mut out = [0u8; 32];
    let bytes = x.to_bytes_le();
    out[..bytes.len()].copy_from_slice(&bytes);
    out
}

/// RFC 9380 §6.7.1 `map_to_curve_elligator2` for curve25519 (`J = 486662`,
/// `K = 1`, `Z = 2`), returning the u-coordinate.
pub fn elligator2(u: &[u8; 32]) -> [u8; 32] {
    let p = p();
    let a = BigUint::from(486_662u32);
    let z = BigUint::from(2u8);
    let one = BigUint::from(1u8);
    let mut masked = *u;
    masked[31] &= 0x7f;
    let u = decode_le(&masked) % &p;
    let neg = |x: &BigUint| (&p - (x % &p)) % &p;
    let inv0 = |x: &BigUint| x.modpow(&(&p - BigUint::from(2u8)), &p);
    let tv1 = (&z * &u * &u) % &p;
    let mut x1 = (neg(&a) * inv0(&((&one + tv1) % &p))) % &p;
    if x1 == BigUint::from(0u8) {
        x1 = neg(&a);
    }
    let gx = |x: &BigUint| (x * x * x + &a * x * x + x) % &p;
    let exp = (&p - &one) >> 1u32;
    let is_square = |v: &BigUint| v == &BigUint::from(0u8) || v.modpow(&exp, &p) == one;
    let x = if is_square(&gx(&x1)) { x1 } else { (neg(&x1) + neg(&a)) % &p };
    encode_le(&x)
}

/// The generator for code `prs`, channel identifier `ci` and session `sid`.
pub fn generator(prs: &[u8], ci: &[u8], sid: &[u8]) -> [u8; 32] {
    let mut input = DSI.to_vec();
    lv(&mut input, prs);
    lv(&mut input, ci);
    lv(&mut input, sid);
    let hash = Sha512::digest(&input);
    let mut u = [0u8; 32];
    u.copy_from_slice(&hash[..32]);
    elligator2(&u)
}

/// One end of a run.
pub struct Cpace {
    role: Role,
    sid: Vec<u8>,
    secret: StaticSecret,
    share: [u8; 32],
}

impl Cpace {
    /// Start a run: `(self, the share to send)`.
    pub fn start<R: RngCore + CryptoRng>(role: Role, prs: &[u8], ci: &[u8], sid: &[u8], rng: &mut R) -> Result<(Self, [u8; 32]), PakeError> {
        let mut scalar = Zeroizing::new([0u8; 32]);
        rng.fill_bytes(scalar.as_mut());
        Self::start_with(role, prs, ci, sid, &scalar)
    }

    /// [`Cpace::start`] with a given scalar (test vectors).
    pub fn start_with(role: Role, prs: &[u8], ci: &[u8], sid: &[u8], scalar: &[u8; 32]) -> Result<(Self, [u8; 32]), PakeError> {
        let g = PublicKey::from(generator(prs, ci, sid));
        let secret = StaticSecret::from(*scalar);
        let share = secret.diffie_hellman(&g).to_bytes();
        if share == [0; 32] {
            return Err(PakeError::LowOrder);
        }
        Ok((
            Self {
                role,
                sid: sid.to_vec(),
                secret,
                share,
            },
            share,
        ))
    }

    /// The other side's share in: the run's keys.
    pub fn finish(self, peer: &[u8; 32]) -> Result<Keys, PakeError> {
        let k = Zeroizing::new(self.secret.diffie_hellman(&PublicKey::from(*peer)).to_bytes());
        if *k == [0; 32] {
            return Err(PakeError::LowOrder);
        }
        let (ya, yb) = match self.role {
            Role::Initiator => (self.share, *peer),
            Role::Responder => (*peer, self.share),
        };
        let mut input = ISK_LABEL.to_vec();
        lv(&mut input, &self.sid);
        input.extend_from_slice(k.as_ref());
        input.extend_from_slice(&ya);
        input.extend_from_slice(&yb);
        let mut hash = Zeroizing::new([0u8; 64]);
        hash.copy_from_slice(&Sha512::digest(&input));
        let hk = Hkdf::<sha2::Sha256>::new(Some(b"rh/v1 cpace keys"), &hash[..32]);
        let expand = |info: &[u8]| {
            let mut out = Zeroizing::new([0u8; 32]);
            hk.expand(info, out.as_mut()).expect("32 bytes");
            out
        };
        Ok(Keys {
            confirm_initiator: expand(b"confirm initiator"),
            confirm_responder: expand(b"confirm responder"),
            pair_secret: expand(b"pair secret"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::rng;

    fn on_curve(x: &[u8; 32]) -> bool {
        let p = p();
        let xv = BigUint::from_bytes_le(x);
        let a = BigUint::from(486_662u32);
        let g = (&xv * &xv * &xv + &a * &xv * &xv + &xv) % &p;
        let exp = (&p - BigUint::from(1u8)) >> 1u32;
        g == BigUint::from(0u8) || g.modpow(&exp, &p) == BigUint::from(1u8)
    }

    #[test]
    fn elligator2_lands_on_the_curve_for_any_input() {
        let mut r = rng(9);
        for _ in 0..64 {
            let mut u = [0u8; 32];
            RngCore::fill_bytes(&mut r, &mut u);
            let x = elligator2(&u);
            assert!(on_curve(&x));
            assert!(BigUint::from_bytes_le(&x) < p(), "canonical");
            // The top bit of the input is ignored.
            let mut flipped = u;
            flipped[31] ^= 0x80;
            assert_eq!(elligator2(&flipped), x);
        }
        // The exceptional input (1 + Z u^2 = 0 has no solution mod p for Z = 2,
        // but u = 0 is the other edge): still a point.
        assert!(on_curve(&elligator2(&[0; 32])));
    }

    #[test]
    fn the_same_code_agrees_and_another_does_not() {
        let ci = b"rh/v1 pair-ci\0acct\0d-new";
        let sid = [7u8; SID_LEN];
        let (a, ya) = Cpace::start(Role::Initiator, b"12345678", ci, &sid, &mut rng(1)).unwrap();
        let (b, yb) = Cpace::start(Role::Responder, b"12345678", ci, &sid, &mut rng(2)).unwrap();
        let ka = a.finish(&yb).unwrap();
        let kb = b.finish(&ya).unwrap();
        assert_eq!(*ka.pair_secret, *kb.pair_secret);
        assert_eq!(*ka.confirm_responder, *kb.confirm_responder);
        assert_ne!(*ka.confirm_initiator, *ka.confirm_responder);
        // A wrong code (an attacker's one guess) gives other keys.
        let (c, yc) = Cpace::start(Role::Initiator, b"12345679", ci, &sid, &mut rng(3)).unwrap();
        let (d, yd) = Cpace::start(Role::Responder, b"12345678", ci, &sid, &mut rng(4)).unwrap();
        assert_ne!(*c.finish(&yd).unwrap().pair_secret, *d.finish(&yc).unwrap().pair_secret);
        // So do another account, device or session.
        let (e, ye) = Cpace::start(Role::Initiator, b"12345678", b"rh/v1 pair-ci\0acct\0d-other", &sid, &mut rng(5)).unwrap();
        let (f, yf) = Cpace::start(Role::Responder, b"12345678", ci, &sid, &mut rng(6)).unwrap();
        assert_ne!(*e.finish(&yf).unwrap().pair_secret, *f.finish(&ye).unwrap().pair_secret);
        // A low-order share is refused.
        let (g, _) = Cpace::start(Role::Initiator, b"12345678", ci, &sid, &mut rng(7)).unwrap();
        assert!(matches!(g.finish(&[0; 32]), Err(PakeError::LowOrder)));
    }

    #[test]
    fn generators_differ_by_every_input() {
        let g = generator(b"1", b"c", b"s");
        assert_ne!(g, generator(b"2", b"c", b"s"));
        assert_ne!(g, generator(b"1", b"d", b"s"));
        assert_ne!(g, generator(b"1", b"c", b"t"));
        // Length-prefixed: moving a byte between fields changes it.
        assert_ne!(generator(b"12", b"c", b"s"), generator(b"1", b"2c", b"s"));
    }
}
