//! Signing in to Droppen through the system browser (docs/local-aggregator.md
//! §4.9, the browser hand-off): what a sign-in attempt carries so droppen.app
//! can answer it without a `window.opener` — the Droppen desktop app's window,
//! or a browser that blocks the popup.
//!
//! Every attempt opened from a page ([`PageOrigin`]) gets a one-off X25519 key
//! and a 16-byte `state`, held in memory with the attempt. The page has the
//! browser open [`Handoff::url`], `<hub>/link#droppen-link=<payload>`: the
//! payload (base64url JSON) holds the attempt's id, the key's public half,
//! the state, the page's origin, when the attempt expires, and the device's
//! host name and public keys — what the consent page shows and checks. A
//! fragment never reaches a server, and the page on droppen.app takes it out
//! of the address at once.
//!
//! droppen.app seals the enrollment code to the key (`rh_proto::link_return`)
//! and sends the browser to `<origin>/link/return#droppen-return=<attempt>.<sealed>`;
//! that page (the web app's, standalone) posts it to `POST
//! /api/cloud/link/return` ([`super::cloud::post_return`]), which opens it and
//! enrolls exactly as the popup's `finish` does. The secret half of the key
//! never leaves this process and goes with the attempt, so what the browser's
//! history keeps of the return address is ciphertext nobody can open.

use std::sync::Arc;

use base64::Engine as _;
use gotcha::axum::extract::FromRequestParts;
use gotcha::axum::http::header;
use gotcha::axum::http::request::Parts;
use rh_proto::keys::DhKeypair;
use rh_proto::link_return::{self, LinkReturnError, ReturnContext, STATE_LEN};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The fragment key of the droppen.app URL.
pub const LINK_FRAGMENT: &str = "droppen-link";
/// Where droppen.app sends the browser back, on the page's origin.
pub const RETURN_PATH: &str = "/link/return";
/// The fragment key of the return address.
pub const RETURN_FRAGMENT: &str = "droppen-return";
/// The payload's version.
pub const VERSION: u8 = 1;
/// The longest sealed reply taken, base64url.
pub const MAX_SEALED_TEXT: usize = 2048;

/// The `Origin` the request came with, when it is a plain `http(s)://host[:port]`
/// origin: where droppen.app sends the browser back. The `/api/*` origin
/// check has already refused one this server does not trust.
///
/// A hand-written extractor, so it adds nothing to the OpenAPI document
/// (`#[api(skip)]`).
#[derive(Debug, Clone)]
pub struct PageOrigin(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for PageOrigin {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let origin = parts
            .headers
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .and_then(plain_origin);
        Ok(PageOrigin(origin))
    }
}

/// `text` when it is an `http(s)://host[:port]` origin and nothing more
/// (no user, path, query or fragment; not `null`), lower-cased scheme.
pub fn plain_origin(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() > 255 {
        return None;
    }
    let (scheme, rest) = text.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let bad = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '/' | '?' | '#' | '@' | '\\' | '"' | '<' | '>');
    if rest.is_empty() || rest.chars().any(bad) {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

/// The device as the consent page shows it.
#[derive(Debug, Clone)]
pub struct HandoffMachine {
    /// The host name it reports (for reference only).
    pub hostname: String,
    /// Its Ed25519 key.
    pub sign_key: [u8; 32],
    /// Its X25519 key.
    pub dh_key: [u8; 32],
    /// `(account key fingerprint, 32 hex digits; machine id)` when enrolled already.
    pub enrolled: Option<(String, String)>,
}

/// An attempt's hand-off material. Its `Debug` shows neither the key nor the state.
#[derive(Clone)]
pub struct Handoff {
    key: Arc<DhKeypair>,
    state: Zeroizing<[u8; STATE_LEN]>,
    /// The page's origin: where the reply comes back.
    pub origin: String,
    sign_key: [u8; 32],
    dh_key: [u8; 32],
    url: String,
}

impl std::fmt::Debug for Handoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handoff").field("origin", &self.origin).finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct Payload {
    v: u8,
    /// The attempt.
    a: String,
    /// The one-off key, hex.
    k: String,
    /// The state, hex.
    s: String,
    /// The page's origin.
    o: String,
    /// When the attempt expires (ms).
    x: u64,
    m: PayloadMachine,
}

#[derive(Serialize, Deserialize)]
struct PayloadMachine {
    h: String,
    s: String,
    d: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    e: Option<PayloadEnrolled>,
}

#[derive(Serialize, Deserialize)]
struct PayloadEnrolled {
    f: String,
    i: String,
}

use crate::text::hex;

impl Handoff {
    /// New material for `attempt` (expiring at `expires_ms`), answered at `origin`.
    pub fn new(hub_web: &str, attempt: &str, expires_ms: u64, origin: &str, machine: &HandoffMachine) -> Self {
        let key = DhKeypair::generate(&mut rand::rngs::OsRng);
        let mut state = Zeroizing::new([0u8; STATE_LEN]);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, state.as_mut());
        let payload = Payload {
            v: VERSION,
            a: attempt.to_string(),
            k: hex(&key.public()),
            s: hex(state.as_ref()),
            o: origin.to_string(),
            x: expires_ms,
            m: PayloadMachine {
                h: machine.hostname.clone(),
                s: hex(&machine.sign_key),
                d: hex(&machine.dh_key),
                e: machine.enrolled.as_ref().map(|(f, i)| PayloadEnrolled { f: f.clone(), i: i.clone() }),
            },
        };
        let json = serde_json::to_vec(&payload).expect("a payload serialises");
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
        let url = format!("{hub_web}/link#{LINK_FRAGMENT}={encoded}");
        Self {
            key: Arc::new(key),
            state,
            origin: origin.to_string(),
            sign_key: machine.sign_key,
            dh_key: machine.dh_key,
            url,
        }
    }

    /// The droppen.app address the browser opens. It carries the state:
    /// it goes to the page that opened the attempt, and is never logged.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Open droppen.app's reply for `attempt`: the plaintext it sealed.
    pub fn open(&self, attempt: &str, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, LinkReturnError> {
        let ctx = ReturnContext {
            attempt,
            origin: &self.origin,
            sign_key: &self.sign_key,
            dh_key: &self.dh_key,
        };
        link_return::open(&self.key, &self.state, &ctx, sealed)
    }
}

/// What droppen.app seals: the enrollment code, and the account's e-mail
/// address as droppen.app shows it (display only).
#[derive(Debug, Deserialize)]
pub struct Reply {
    /// The enrollment code (`DR1-…`, or `RH1-…`).
    pub code: String,
    /// The account's e-mail address (display only).
    #[serde(default)]
    pub email: Option<String>,
}

/// A sealed reply's text (base64url, unpadded), or `None`.
pub fn sealed_bytes(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() > MAX_SEALED_TEXT {
        return None;
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// What droppen.app does with a hand-off URL: read the payload and seal `reply` to it.
    pub(crate) fn answer(url: &str, reply: &serde_json::Value, state_override: Option<[u8; STATE_LEN]>) -> (String, String) {
        let fragment = url.split_once('#').expect("a fragment").1;
        let encoded = fragment.strip_prefix(&format!("{LINK_FRAGMENT}=")).expect("the payload");
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded).unwrap();
        let payload: Payload = serde_json::from_slice(&json).unwrap();
        let unhex = |s: &str| -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect() };
        let key: [u8; 32] = unhex(&payload.k).try_into().unwrap();
        let state: [u8; STATE_LEN] = state_override.unwrap_or_else(|| unhex(&payload.s).try_into().unwrap());
        let sign: [u8; 32] = unhex(&payload.m.s).try_into().unwrap();
        let dh: [u8; 32] = unhex(&payload.m.d).try_into().unwrap();
        let ctx = ReturnContext {
            attempt: &payload.a,
            origin: &payload.o,
            sign_key: &sign,
            dh_key: &dh,
        };
        let sealed = link_return::seal(&key, &state, &ctx, reply.to_string().as_bytes(), &mut rand::rngs::OsRng).unwrap();
        (payload.a, base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sealed))
    }

    fn machine() -> HandoffMachine {
        HandoffMachine {
            hostname: "laptop".into(),
            sign_key: [1; 32],
            dh_key: [2; 32],
            enrolled: None,
        }
    }

    #[test]
    fn only_a_plain_http_origin_is_a_place_to_come_back_to() {
        assert_eq!(plain_origin("http://127.0.0.1:51234").as_deref(), Some("http://127.0.0.1:51234"));
        assert_eq!(plain_origin("HTTPS://box.example").as_deref(), Some("https://box.example"));
        assert_eq!(plain_origin("http://[::1]:8700").as_deref(), Some("http://[::1]:8700"));
        for bad in [
            "null",
            "",
            "file://x",
            "http://",
            "http://a/b",
            "http://a?b",
            "http://a#b",
            "http://user@a",
            "http://a b",
            "chrome-extension://abc",
        ] {
            assert_eq!(plain_origin(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_url_carries_public_keys_and_the_state_in_its_fragment_only() {
        let handoff = Handoff::new("https://droppen.app", "a1", 99, "http://127.0.0.1:8700", &machine());
        let url = handoff.url();
        assert!(url.starts_with("https://droppen.app/link#droppen-link="), "{url}");
        assert!(!url.contains('?'), "nothing in a query");
        let encoded = url.split_once('=').unwrap().1;
        let payload: Payload = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
        assert_eq!(
            (payload.v, payload.a.as_str(), payload.o.as_str(), payload.x),
            (1, "a1", "http://127.0.0.1:8700", 99)
        );
        assert_eq!(payload.k, hex(&handoff.key.public()));
        assert_eq!(payload.m.s, hex(&[1; 32]));
        assert_eq!(payload.m.h, "laptop");
        assert!(payload.m.e.is_none());
        assert!(!format!("{handoff:?}").contains(&payload.s), "Debug never shows the state");
        // Another attempt: another key and state.
        let again = Handoff::new("https://droppen.app", "a1", 99, "http://127.0.0.1:8700", &machine());
        assert_ne!(again.url(), handoff.url());
    }

    #[test]
    fn a_reply_opens_for_its_own_attempt_and_state_only() {
        let handoff = Handoff::new("https://droppen.app", "a1", 99, "http://127.0.0.1:8700", &machine());
        let reply = serde_json::json!({"code": "DR1-AAAA", "email": "me@example.com"});
        let (attempt, sealed) = answer(handoff.url(), &reply, None);
        let bytes = sealed_bytes(&sealed).unwrap();
        let opened = handoff.open(&attempt, &bytes).unwrap();
        let parsed: Reply = serde_json::from_slice(&opened).unwrap();
        assert_eq!((parsed.code.as_str(), parsed.email.as_deref()), ("DR1-AAAA", Some("me@example.com")));
        assert!(handoff.open("a2", &bytes).is_err(), "another attempt");
        let (_, forged) = answer(handoff.url(), &reply, Some([0; STATE_LEN]));
        assert!(
            handoff.open(&attempt, &sealed_bytes(&forged).unwrap()).is_err(),
            "a page that never saw the state"
        );
        let other = Handoff::new("https://droppen.app", "a1", 99, "http://127.0.0.1:8700", &machine());
        assert!(other.open(&attempt, &bytes).is_err(), "another attempt's key");
        assert!(sealed_bytes("").is_none());
        assert!(sealed_bytes(&"A".repeat(MAX_SEALED_TEXT + 1)).is_none());
        assert!(sealed_bytes("not base64!").is_none());
    }
}
