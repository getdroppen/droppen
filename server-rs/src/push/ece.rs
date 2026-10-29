//! Web Push cryptography: RFC 8291 content encryption and RFC 8292 VAPID.
//!
//! Both protocols are implemented here rather than taken from the `web-push`
//! crate, for one reason: that crate reaches the `ece` crate and from there
//! `openssl-sys`, and this server links nothing but rustls so that it can be
//! built static against musl. Everything below is RustCrypto — `p256`, `hkdf`,
//! `sha2`, `aes-gcm` — all of which are already pure Rust.
//!
//! Hand-rolled cryptography is worth exactly as much as the vectors that pin it,
//! so [`tests::rfc8291_appendix_a_vector`] runs the published example through
//! [`encrypt_with`] and compares the output byte for byte. That single assertion
//! covers the whole construction at once: the ECDH, both HKDF layers and their
//! `info` strings, the record delimiter, the AES-128-GCM call and the header
//! encoding. Any one of them wrong and it fails.
//!
//! # The shape of an aes128gcm body
//!
//! ```text
//! salt (16) | rs (4, big-endian) | idlen (1) | as_public (65) | ciphertext
//! ```
//!
//! The plaintext is a single record: the payload followed by the `0x02`
//! last-record delimiter, sealed with AES-128-GCM. `rs` is nominal here — one
//! record always fits — but it is part of the header the receiver parses.

use p256::ecdsa::SigningKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;

/// An uncompressed SEC1 P-256 point: the `0x04` tag plus two 32-byte coordinates.
pub const POINT_LEN: usize = 65;
/// The length of a subscription's `auth` secret, fixed by RFC 8291 §3.2.
pub const AUTH_SECRET_LEN: usize = 16;
/// Salt length, fixed by RFC 8188.
pub const SALT_LEN: usize = 16;
/// The record size the header advertises. One record always fits under it.
const RECORD_SIZE: u32 = 4096;
/// RFC 8188's last-record delimiter.
const LAST_RECORD: u8 = 0x02;
/// How long a VAPID JWT stays valid. RFC 8292 §2 caps this at 24 hours; half of
/// that leaves room for a clock that is out by hours rather than seconds.
const VAPID_TTL_SECS: u64 = 12 * 3600;

/// Something in a subscription or a payload was not what the protocol requires.
#[derive(Debug, thiserror::Error)]
pub enum EceError {
    /// The subscription's `p256dh` is not a point on P-256.
    #[error("subscription key is not a P-256 point: {0}")]
    BadPublicKey(String),
    /// One of the two HKDF expansions asked for an impossible length.
    #[error("hkdf: {0}")]
    Hkdf(&'static str),
    /// AES-128-GCM refused.
    #[error("aes-128-gcm: {0}")]
    Aead(&'static str),
    /// A body that is too short, or whose header does not parse.
    #[error("malformed aes128gcm body: {0}")]
    Malformed(&'static str),
}

/// base64url without padding — the encoding every Web Push field uses.
pub fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The inverse of [`b64url`], forgiving about how the sender spelled it.
///
/// Browsers agree on unpadded base64url, but subscriptions arrive through
/// clients that have re-encoded them, and the two common accidents are padding
/// and the standard `+/` alphabet. Neither changes the bytes, so neither is
/// worth rejecting a real subscription over.
pub fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let normalized: String = text
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(normalized).ok()
}

/// Seconds since the epoch, for the JWT's `exp`.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Derive the content-encryption key and nonce from a shared secret.
///
/// Both callers — encryption and decryption — need the identical derivation, and
/// getting the two halves to agree by writing them twice is exactly the kind of
/// thing that fails silently, so they share this.
fn derive_keys(shared: &[u8], auth: &[u8], salt: &[u8], ua_pub: &[u8], as_pub: &[u8]) -> Result<([u8; 16], [u8; 12]), EceError> {
    use hkdf::Hkdf;
    use sha2::Sha256;

    // IKM = HKDF(salt = auth secret, ikm = ECDH output,
    //            info = "WebPush: info" 0x00 ua_public as_public)
    // The two public keys in the info string are what bind the derived key to
    // this exact pair of parties (RFC 8291 §3.3).
    let mut key_info = Vec::with_capacity(14 + POINT_LEN * 2);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_pub);
    key_info.extend_from_slice(as_pub);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared)
        .expand(&key_info, &mut ikm)
        .map_err(|_| EceError::Hkdf("input keying material"))?;

    // CEK and nonce from the record salt, with RFC 8188's own info strings.
    let hk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    hk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .map_err(|_| EceError::Hkdf("content encryption key"))?;
    let mut nonce = [0u8; 12];
    hk.expand(b"Content-Encoding: nonce\0", &mut nonce).map_err(|_| EceError::Hkdf("nonce"))?;
    Ok((cek, nonce))
}

/// Encrypt with a caller-supplied ephemeral key and salt.
///
/// Split out from [`encrypt`] for one reason: it is the only way to run the
/// RFC 8291 published vector, which fixes both. Nothing in production calls it.
fn encrypt_with(eph: &p256::SecretKey, salt: &[u8; SALT_LEN], ua_pub: &[u8], auth: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, EceError> {
    use aes_gcm::aead::{Aead, KeyInit};

    let ua = p256::PublicKey::from_sec1_bytes(ua_pub).map_err(|e| EceError::BadPublicKey(e.to_string()))?;
    let shared = p256::ecdh::diffie_hellman(eph.to_nonzero_scalar(), ua.as_affine());
    let eph_pub = eph.public_key().to_encoded_point(false);
    let (cek, nonce) = derive_keys(shared.raw_secret_bytes(), auth, salt, ua_pub, eph_pub.as_bytes())?;

    // One record: the payload plus the delimiter that says no more follow.
    let mut record = Vec::with_capacity(plaintext.len() + 1);
    record.extend_from_slice(plaintext);
    record.push(LAST_RECORD);
    let cipher = aes_gcm::Aes128Gcm::new_from_slice(&cek).map_err(|_| EceError::Aead("key length"))?;
    let sealed = cipher
        .encrypt(aes_gcm::Nonce::from_slice(&nonce), record.as_slice())
        .map_err(|_| EceError::Aead("seal"))?;

    let mut out = Vec::with_capacity(SALT_LEN + 4 + 1 + POINT_LEN + sealed.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(POINT_LEN as u8);
    out.extend_from_slice(eph_pub.as_bytes());
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Encrypt a payload for one subscription: the body of the `POST`.
///
/// A fresh ephemeral keypair and salt per message, which is what makes two
/// notifications with the same text produce different ciphertext.
pub fn encrypt(ua_pub: &[u8], auth: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, EceError> {
    use rand::RngCore;
    let eph = p256::SecretKey::random(&mut rand::rngs::OsRng);
    let mut salt = [0u8; SALT_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    encrypt_with(&eph, &salt, ua_pub, auth, plaintext)
}

/// Recover a payload from an aes128gcm body, given the subscription's private key.
///
/// The inverse of [`encrypt`], and the only way to assert that what this server
/// puts on the wire is what a browser would read back out of it — the integration
/// test stands up a fake push endpoint, catches the body and runs it through
/// here. It is public because that test lives outside the crate; nothing in the
/// server calls it.
pub fn decrypt(ua_private: &p256::SecretKey, auth: &[u8], body: &[u8]) -> Result<Vec<u8>, EceError> {
    use aes_gcm::aead::{Aead, KeyInit};

    if body.len() < SALT_LEN + 4 + 1 {
        return Err(EceError::Malformed("shorter than a header"));
    }
    let salt = &body[..SALT_LEN];
    let id_len = body[SALT_LEN + 4] as usize;
    let key_end = SALT_LEN + 5 + id_len;
    if id_len != POINT_LEN || body.len() <= key_end {
        return Err(EceError::Malformed("key id is not a P-256 point"));
    }
    let as_pub = &body[SALT_LEN + 5..key_end];
    let ciphertext = &body[key_end..];

    let as_key = p256::PublicKey::from_sec1_bytes(as_pub).map_err(|e| EceError::BadPublicKey(e.to_string()))?;
    let shared = p256::ecdh::diffie_hellman(ua_private.to_nonzero_scalar(), as_key.as_affine());
    let ua_pub = ua_private.public_key().to_encoded_point(false);
    let (cek, nonce) = derive_keys(shared.raw_secret_bytes(), auth, salt, ua_pub.as_bytes(), as_pub)?;

    let cipher = aes_gcm::Aes128Gcm::new_from_slice(&cek).map_err(|_| EceError::Aead("key length"))?;
    let mut record = cipher
        .decrypt(aes_gcm::Nonce::from_slice(&nonce), ciphertext)
        .map_err(|_| EceError::Aead("open"))?;
    // Strip the delimiter and any zero padding that followed the payload.
    while record.last() == Some(&0) {
        record.pop();
    }
    match record.pop() {
        Some(LAST_RECORD) | Some(0x01) => Ok(record),
        _ => Err(EceError::Malformed("no record delimiter")),
    }
}

/// The `applicationServerKey` a browser subscribes with: the uncompressed public
/// point, base64url.
pub fn vapid_public_b64(key: &SigningKey) -> String {
    b64url(key.verifying_key().to_encoded_point(false).as_bytes())
}

/// The `Authorization` header for one push endpoint (RFC 8292 §3).
///
/// An ES256 JWT whose `aud` is the push service's origin — not the full endpoint
/// URL, which would leak the subscription id into the header — plus the public
/// key the service verifies it against. `p256`'s ECDSA is RFC 6979 deterministic
/// and produces a fixed 64-byte `r || s`, which is the encoding JWS wants and
/// not the DER one.
pub fn vapid_auth(endpoint: &reqwest::Url, subject: &str, key: &SigningKey) -> String {
    use p256::ecdsa::signature::Signer;

    let header = b64url(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = serde_json::json!({
        "aud": endpoint.origin().ascii_serialization(),
        "exp": now_unix() + VAPID_TTL_SECS,
        "sub": subject,
    });
    let payload = b64url(&serde_json::to_vec(&claims).unwrap_or_default());
    let signing_input = format!("{header}.{payload}");
    let signature: p256::ecdsa::Signature = key.sign(signing_input.as_bytes());
    format!("vapid t={signing_input}.{}, k={}", b64url(&signature.to_bytes()), vapid_public_b64(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(text: &str) -> Vec<u8> {
        b64url_decode(text).expect("a test vector decodes")
    }

    /// The published RFC 8291 Appendix A example, run forwards.
    ///
    /// The vector fixes the application server's private key and the salt, which
    /// is the only reason a hand-rolled implementation can be compared byte for
    /// byte against a known-good one at all. It exercises every step in one go:
    /// get the ECDH, either HKDF `info` string, the record delimiter, the GCM
    /// call or the header layout wrong and the output diverges.
    #[test]
    fn rfc8291_appendix_a_vector() {
        let as_private = b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw");
        let ua_public = b64("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let auth_secret = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; SALT_LEN] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().expect("16-byte salt");
        let plaintext = b"When I grow up, I want to be a watermelon";

        let eph = p256::SecretKey::from_slice(&as_private).expect("the vector's private key");
        let got = encrypt_with(&eph, &salt, &ua_public, &auth_secret, plaintext).expect("encrypt");

        let mut expected = b64("DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
        expected.extend_from_slice(&b64("8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ"));
        assert_eq!(got, expected, "RFC 8291 Appendix A");
    }

    /// The same vector run backwards, using the receiver's private key from the
    /// RFC. This is what pins [`decrypt`] — without it the integration test
    /// could be asserting that two of our own bugs cancel out.
    #[test]
    fn rfc8291_appendix_a_vector_decrypts() {
        let ua_private = b64("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94");
        let auth_secret = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let mut body = b64("DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
        body.extend_from_slice(&b64("8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ"));

        let key = p256::SecretKey::from_slice(&ua_private).expect("the vector's receiver key");
        // Cross-check the key against the vector's own public half first, so a
        // mistyped scalar fails here rather than as an opaque GCM tag mismatch.
        assert_eq!(
            b64url(key.public_key().to_encoded_point(false).as_bytes()),
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let plaintext = decrypt(&key, &auth_secret, &body).expect("decrypt");
        assert_eq!(plaintext, b"When I grow up, I want to be a watermelon");
    }

    /// A round trip through a freshly generated subscription keypair, which is
    /// the path production actually takes (random ephemeral key, random salt).
    #[test]
    fn a_random_message_round_trips() {
        let ua = p256::SecretKey::random(&mut rand::rngs::OsRng);
        let ua_public = ua.public_key().to_encoded_point(false);
        let auth = [7u8; AUTH_SECRET_LEN];

        let a = encrypt(ua_public.as_bytes(), &auth, b"hello").expect("encrypt");
        let b = encrypt(ua_public.as_bytes(), &auth, b"hello").expect("encrypt");
        assert_ne!(a, b, "a fresh ephemeral key and salt per message");
        assert_eq!(
            a.len(),
            SALT_LEN + 4 + 1 + POINT_LEN + 5 + 1 + 16,
            "header + payload + delimiter + GCM tag"
        );
        assert_eq!(decrypt(&ua, &auth, &a).expect("decrypt"), b"hello");
    }

    #[test]
    fn a_subscription_key_that_is_not_on_the_curve_is_refused() {
        let auth = [7u8; AUTH_SECRET_LEN];
        let error = encrypt(&[0x04; POINT_LEN], &auth, b"hi").expect_err("not a point");
        assert!(matches!(error, EceError::BadPublicKey(_)), "got {error:?}");
    }

    #[test]
    fn a_truncated_body_is_refused_rather_than_panicking() {
        let ua = p256::SecretKey::random(&mut rand::rngs::OsRng);
        for len in [0usize, 10, SALT_LEN + 4, SALT_LEN + 5, SALT_LEN + 5 + POINT_LEN] {
            let body = vec![0u8; len];
            assert!(decrypt(&ua, &[7u8; AUTH_SECRET_LEN], &body).is_err(), "len {len}");
        }
    }

    /// The header a push service parses: `vapid t=<JWT>, k=<public key>`, with
    /// the three claims RFC 8292 requires and a signature the named key verifies.
    #[test]
    fn the_vapid_header_carries_a_verifiable_es256_jwt() {
        use p256::ecdsa::signature::Verifier;

        let key = SigningKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).expect("a P-256 scalar");
        let endpoint = reqwest::Url::parse("https://fcm.googleapis.com/fcm/send/abc:def").expect("a URL");
        let header = vapid_auth(&endpoint, "mailto:x@y.z", &key);

        let token = header
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split(", k=").next())
            .expect("a `vapid t=` header");
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "header.payload.signature");

        let jose: serde_json::Value = serde_json::from_slice(&b64(parts[0])).expect("JOSE header");
        assert_eq!(jose["alg"], "ES256", "the only algorithm RFC 8292 defines");
        assert_eq!(jose["typ"], "JWT");

        let claims: serde_json::Value = serde_json::from_slice(&b64(parts[1])).expect("claims");
        // The signed bytes: `aud`, `exp`, `sub`, in that order.
        assert_eq!(
            String::from_utf8(b64(parts[1])).expect("utf-8"),
            format!(r#"{{"aud":"https://fcm.googleapis.com","exp":{},"sub":"mailto:x@y.z"}}"#, claims["exp"])
        );
        assert_eq!(claims["aud"], "https://fcm.googleapis.com", "the origin, not the subscription URL");
        assert_eq!(claims["sub"], "mailto:x@y.z");
        let exp = claims["exp"].as_u64().expect("exp");
        assert!(exp > now_unix(), "not already expired");
        assert!(exp <= now_unix() + 24 * 3600, "RFC 8292 caps a token at 24 hours");

        let signature = p256::ecdsa::Signature::from_slice(&b64(parts[2])).expect("a 64-byte r||s");
        key.verifying_key()
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("the signature verifies under the advertised key");

        let advertised = header.split(", k=").nth(1).expect("a `k=` parameter");
        assert_eq!(b64(advertised).len(), POINT_LEN, "an uncompressed point");
        assert_eq!(advertised, vapid_public_b64(&key), "and it is this key's");
    }

    /// A subscription that arrived through a client which re-encoded it must
    /// still work: the bytes are the same, only the spelling changed.
    #[test]
    fn base64url_decoding_forgives_padding_and_the_standard_alphabet() {
        let bytes: Vec<u8> = (0u8..32).collect();
        let canonical = b64url(&bytes);
        assert_eq!(b64url_decode(&canonical).expect("canonical"), bytes);

        let padded = format!("{canonical}==");
        assert_eq!(b64url_decode(&padded).expect("padded"), bytes);

        let standard: String = canonical
            .chars()
            .map(|c| match c {
                '-' => '+',
                '_' => '/',
                other => other,
            })
            .collect();
        assert_eq!(b64url_decode(&standard).expect("standard alphabet"), bytes);

        assert_eq!(b64url_decode("not base64 at all!"), None);
    }
}
