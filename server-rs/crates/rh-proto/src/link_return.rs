//! Signing a device in to Droppen through the system browser
//! (`docs/local-aggregator.md` §4.9, the browser hand-off): the sealed reply
//! droppen.app hands back to the device's own page, by a top-level
//! navigation, when there is no `window.opener` to post it to — the desktop
//! app's window, or a browser that does not allow popups.
//!
//! The device's local server opens a sign-in attempt with a one-off X25519
//! key `E` and a 16-byte `state`, and puts both — with the attempt's id, the
//! page's own origin and the device's public keys — in the fragment of the
//! droppen.app URL it has the browser open (a fragment never reaches a
//! server). droppen.app, once the person consents, seals the enrollment code
//! to `E` and sends the browser to `<origin>/link/return#…` with the sealed
//! reply in the fragment; the page there hands it to its server, which opens
//! it with `E`'s secret — held in memory only, for that one attempt.
//!
//! `sealed = w ‖ nonce(12) ‖ AES-256-GCM(kek, nonce, aad = context, plaintext)`,
//! with a one-off X25519 key `w` of the sealing page and
//!
//! `kek = HKDF-SHA256(salt = "droppen/v1 link-return", ikm = X25519(w, E) ‖ state,
//!                    info = w ‖ E ‖ context)`,
//!
//! `context = "droppen/v1 link-return\0" | str16 attempt | str16 origin | sign key(32) | dh key(32)`.
//!
//! So a reply opens only:
//!
//! - with `E`'s secret: what lands in the browser's history (the return
//!   address's fragment) is ciphertext, useless once the attempt is over;
//! - with the `state` of the URL the server made: a page that never saw that
//!   URL cannot seal a code of its own choosing to this server (login CSRF);
//! - for the attempt, the origin and the device keys the consent page showed
//!   (the AAD): a reply cannot be moved to another attempt or address.
//!
//! What it does not do, by design: authenticate droppen.app. Anybody holding
//! the URL can seal; the URL is the capability, as the opener's nonce is in
//! the popup hand-off (§4.4), and the enrollment proof still binds the code
//! to the device's own keys.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use hkdf::Hkdf;
use rand_core::{CryptoRng, RngCore};
use sha2::Sha256;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::codec::put_str16;
use crate::keys::DhKeypair;

/// The HKDF salt.
pub const SALT: &[u8] = b"droppen/v1 link-return";
const CONTEXT_LABEL: &[u8] = b"droppen/v1 link-return\0";
/// The `state`'s length.
pub const STATE_LEN: usize = 16;
/// The longest plaintext sealed (a code and an e-mail address fit many times over).
pub const MAX_PLAINTEXT: usize = 1024;
/// The longest attempt id or origin the context takes.
pub const MAX_FIELD: usize = 512;
const HEAD: usize = 32 + 12;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LinkReturnError {
    #[error("the reply does not open (another attempt, state, address or key, or tampered)")]
    Open,
    #[error("a key is of low order")]
    LowOrderKey,
    #[error("the reply is malformed")]
    Malformed,
    #[error("a {0}-byte reply is too large")]
    TooLarge(usize),
}

/// What a reply is bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnContext<'a> {
    /// The sign-in attempt's id.
    pub attempt: &'a str,
    /// The origin of the page the reply goes back to (`http://127.0.0.1:51234`).
    pub origin: &'a str,
    /// The device's Ed25519 key, as the consent page showed it.
    pub sign_key: &'a [u8; 32],
    /// The device's X25519 key, as the consent page showed it.
    pub dh_key: &'a [u8; 32],
}

impl ReturnContext<'_> {
    /// `"droppen/v1 link-return\0" | str16 attempt | str16 origin | sign key | dh key`.
    pub fn encode(&self) -> Result<Vec<u8>, LinkReturnError> {
        if self.attempt.len() > MAX_FIELD || self.origin.len() > MAX_FIELD {
            return Err(LinkReturnError::Malformed);
        }
        let mut out = CONTEXT_LABEL.to_vec();
        put_str16(&mut out, self.attempt);
        put_str16(&mut out, self.origin);
        out.extend_from_slice(self.sign_key);
        out.extend_from_slice(self.dh_key);
        Ok(out)
    }
}

fn kek(shared: &[u8; 32], state: &[u8; STATE_LEN], sender: &[u8; 32], recipient: &[u8; 32], ctx: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut ikm = Zeroizing::new([0u8; 32 + STATE_LEN]);
    ikm[..32].copy_from_slice(shared);
    ikm[32..].copy_from_slice(state);
    let mut out = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(SALT), ikm.as_ref())
        .expand_multi_info(&[sender, recipient, ctx], out.as_mut())
        .expect("32 bytes is a valid HKDF-SHA256 length");
    out
}

/// Seal `plaintext` to the attempt's key `recipient` (what droppen.app does).
pub fn seal<R: RngCore + CryptoRng>(
    recipient: &[u8; 32],
    state: &[u8; STATE_LEN],
    ctx: &ReturnContext<'_>,
    plaintext: &[u8],
    rng: &mut R,
) -> Result<Vec<u8>, LinkReturnError> {
    let ephemeral = DhKeypair::generate(rng);
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    seal_with(&ephemeral, nonce, recipient, state, ctx, plaintext)
}

/// [`seal`] with a given one-off key and nonce (test vectors).
fn seal_with(
    ephemeral: &DhKeypair,
    nonce: [u8; 12],
    recipient: &[u8; 32],
    state: &[u8; STATE_LEN],
    ctx: &ReturnContext<'_>,
    plaintext: &[u8],
) -> Result<Vec<u8>, LinkReturnError> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(LinkReturnError::TooLarge(plaintext.len()));
    }
    let context = ctx.encode()?;
    let shared = ephemeral.agree(recipient).ok_or(LinkReturnError::LowOrderKey)?;
    let key = kek(&shared, state, &ephemeral.public(), recipient, &context);
    let ct = Aes256Gcm::new_from_slice(key.as_ref())
        .expect("32-byte key")
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &context,
            },
        )
        .map_err(|_| LinkReturnError::TooLarge(plaintext.len()))?;
    Ok([&ephemeral.public()[..], &nonce, &ct].concat())
}

/// Open a reply with the attempt's key pair and state (what the device's
/// local server does).
pub fn open(recipient: &DhKeypair, state: &[u8; STATE_LEN], ctx: &ReturnContext<'_>, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, LinkReturnError> {
    if sealed.len() > HEAD + MAX_PLAINTEXT + 16 {
        return Err(LinkReturnError::TooLarge(sealed.len()));
    }
    if sealed.len() < HEAD + 16 {
        return Err(LinkReturnError::Malformed);
    }
    let context = ctx.encode()?;
    let sender: [u8; 32] = sealed[..32].try_into().expect("32 bytes");
    let nonce: [u8; 12] = sealed[32..HEAD].try_into().expect("12 bytes");
    let shared = recipient.agree(&sender).ok_or(LinkReturnError::LowOrderKey)?;
    let key = kek(&shared, state, &sender, &recipient.public(), &context);
    Aes256Gcm::new_from_slice(key.as_ref())
        .expect("32-byte key")
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &sealed[HEAD..],
                aad: &context,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| LinkReturnError::Open)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::codec::hex;
    use crate::testing::rng;

    /// The fixed inputs of the interop vector (`web/test/link-browser-handoff.test.ts`
    /// seals the same and must get the same bytes).
    pub(crate) struct Vector {
        pub recipient_secret: [u8; 32],
        pub ephemeral_secret: [u8; 32],
        pub nonce: [u8; 12],
        pub state: [u8; STATE_LEN],
        pub sign_key: [u8; 32],
        pub dh_key: [u8; 32],
        pub attempt: &'static str,
        pub origin: &'static str,
        pub plaintext: &'static str,
    }

    pub(crate) const VECTOR: Vector = Vector {
        recipient_secret: [0x11; 32],
        ephemeral_secret: [0x22; 32],
        nonce: [0x33; 12],
        state: [0x44; STATE_LEN],
        sign_key: [0x55; 32],
        dh_key: [0x66; 32],
        attempt: "0123456789abcdef0123456789abcdef",
        origin: "http://127.0.0.1:51234",
        plaintext: r#"{"code":"DR1-AAAA-BBBB","email":"me@example.com"}"#,
    };

    fn ctx(v: &Vector) -> ReturnContext<'_> {
        ReturnContext {
            attempt: v.attempt,
            origin: v.origin,
            sign_key: &v.sign_key,
            dh_key: &v.dh_key,
        }
    }

    /// The sealed bytes of [`VECTOR`], as hex: pinned here and in the web test.
    pub(crate) const SEALED_HEX: &str = include_str!("../vectors/link-return.hex");

    #[test]
    fn the_vector_is_current_and_opens() {
        let v = &VECTOR;
        let recipient = DhKeypair::from_secret(v.recipient_secret);
        let ephemeral = DhKeypair::from_secret(v.ephemeral_secret);
        let sealed = seal_with(&ephemeral, v.nonce, &recipient.public(), &v.state, &ctx(v), v.plaintext.as_bytes()).unwrap();
        if std::env::var_os("RH_PROTO_WRITE_VECTORS").is_some() {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors/link-return.hex");
            std::fs::write(path, hex::encode(&sealed)).unwrap();
        } else {
            assert_eq!(hex::encode(&sealed), SEALED_HEX.trim(), "RH_PROTO_WRITE_VECTORS=1 rewrites it");
        }
        let opened = open(&recipient, &v.state, &ctx(v), &sealed).unwrap();
        assert_eq!(opened.as_slice(), v.plaintext.as_bytes());
        // And the recipient's public key is what the URL carries.
        assert_eq!(recipient.public(), DhKeypair::from_secret(v.recipient_secret).public());
    }

    #[test]
    fn a_reply_opens_only_with_its_key_state_and_context() {
        let mut r = rng(7);
        let recipient = DhKeypair::generate(&mut r);
        let state = [9u8; STATE_LEN];
        let (sign, dh) = ([1u8; 32], [2u8; 32]);
        let good = ReturnContext {
            attempt: "a1",
            origin: "http://127.0.0.1:8700",
            sign_key: &sign,
            dh_key: &dh,
        };
        let sealed = seal(&recipient.public(), &state, &good, b"DR1-CODE", &mut r).unwrap();
        assert_eq!(open(&recipient, &state, &good, &sealed).unwrap().as_slice(), b"DR1-CODE");

        let other = DhKeypair::generate(&mut r);
        assert_eq!(open(&other, &state, &good, &sealed), Err(LinkReturnError::Open), "another attempt's key");
        assert_eq!(
            open(&recipient, &[8u8; STATE_LEN], &good, &sealed),
            Err(LinkReturnError::Open),
            "another state"
        );
        for moved in [
            ReturnContext { attempt: "a2", ..good },
            ReturnContext {
                origin: "http://127.0.0.1:8701",
                ..good
            },
            ReturnContext { sign_key: &dh, ..good },
            ReturnContext { dh_key: &sign, ..good },
        ] {
            assert_eq!(open(&recipient, &state, &moved, &sealed), Err(LinkReturnError::Open), "{moved:?}");
        }
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(open(&recipient, &state, &good, &tampered), Err(LinkReturnError::Open));
        assert_eq!(open(&recipient, &state, &good, &sealed[..40]), Err(LinkReturnError::Malformed));
        let mut low = sealed.clone();
        low[..32].fill(0);
        assert_eq!(open(&recipient, &state, &good, &low), Err(LinkReturnError::LowOrderKey));
        assert_eq!(
            open(&recipient, &state, &good, &vec![7u8; HEAD + MAX_PLAINTEXT + 17]),
            Err(LinkReturnError::TooLarge(HEAD + MAX_PLAINTEXT + 17))
        );
        assert_eq!(
            seal(&recipient.public(), &state, &good, &[0u8; MAX_PLAINTEXT + 1], &mut r),
            Err(LinkReturnError::TooLarge(MAX_PLAINTEXT + 1))
        );
    }
}
