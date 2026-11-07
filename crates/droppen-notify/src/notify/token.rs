//! The credential an injected `rh-notify` proves itself with.
//!
//! `POST /api/notify` reaches out of this machine and onto the user's lock
//! screen, so it cannot be open — and the caller is a process inside an agent's
//! session, which has no account, no login and no browser to hold one. This is
//! the third kind of credential in the server, beside `droppen_auth::sessions`'
//! bearer tokens and `droppen_auth::tickets`' scoped ones, and it is shaped by
//! two constraints neither of those has:
//!
//! * **It must survive a server restart.** A resident claude outlives the
//!   server that spawned it (the server's `provider::shim` exists for exactly that),
//!   and its environment was fixed at spawn: there is no way to hand a
//!   re-started server's new in-memory table to a process that is already
//!   running. So verification is *stateless* — a keyed MAC recomputed from the
//!   stored secret — and the only thing that has to persist is that secret,
//!   which [`super::store::secret`] already writes `0600`.
//! * **It must name the session.** The whole value of routing a nudge through
//!   the server rather than letting the agent `curl` Bark directly is that the
//!   server can add what the agent does not know and must not be told: the
//!   session's title, and a link that opens it. A token that only said "somebody
//!   this server spawned" would leave the notification anonymous.
//!
//! # What it is not
//!
//! Not an identity: it authorizes one operation on one session and nothing else,
//! and [`super::NotifyCaller`] is deliberately not an `droppen_auth::Identity`.
//! Whoever holds one can put notifications on the user's phone, attributed to
//! that session, and can do nothing else at all — not read a transcript, not
//! send a message, not learn the device key. That is the blast radius of the
//! environment of a process the user themselves is running, which is the level
//! it needs to be at.
//!
//! Not a bearer of the device key either, which is the point of the whole
//! arrangement: the key stays in a `0600` file the agent cannot read, and what
//! reaches the session is a capability to *use* it once per call.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

/// The version tag every token carries, so a future format change is a refusal
/// rather than a misparse.
const PREFIX: &str = "rhn1";

/// base64url without padding, the alphabet every other token in this server uses.
const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// What the MAC is taken over.
///
/// The instance name is in the message rather than in the token because the
/// verifier already knows its own: a token minted by the `staging` instance
/// therefore simply fails to verify on `default`, without either side having to
/// compare a field. The secret is machine-global (see [`super::store`]), so
/// without this the two instances would accept each other's sessions — which
/// would put another instance's session title and deep link on a notification.
fn message(instance: &str, session_key: &str) -> Vec<u8> {
    format!("rh-notify:v1:{instance}:{session_key}").into_bytes()
}

/// The MAC of one session under one instance.
fn mac(secret: &[u8], instance: &str, session_key: &str) -> Vec<u8> {
    // `new_from_slice` only refuses a key length HMAC cannot use, and HMAC
    // accepts every length there is.
    let mut hmac = <Hmac<Sha256>>::new_from_slice(secret).expect("hmac accepts any key length");
    hmac.update(&message(instance, session_key));
    hmac.finalize().into_bytes().to_vec()
}

/// The token a session's `rh-notify` is given.
///
/// Opaque on purpose: the session key is inside it rather than passed beside it,
/// so there is no version of the call where the caller states which session it
/// is and the server believes them.
pub fn mint(secret: &[u8], instance: &str, session_key: &str) -> String {
    format!(
        "{PREFIX}.{}.{}",
        B64.encode(session_key.as_bytes()),
        B64.encode(mac(secret, instance, session_key))
    )
}

/// The session a token names, or `None` for anything this server did not mint.
///
/// The comparison is constant-time for `droppen_auth::sessions`' reason: a
/// byte-by-byte early exit is a timing oracle a caller can walk a forgery
/// through, and the cost of not having one is a fixed 32-byte compare.
pub fn verify(secret: &[u8], instance: &str, token: &str) -> Option<String> {
    let mut parts = token.split('.');
    if parts.next()? != PREFIX {
        return None;
    }
    let session_key = String::from_utf8(B64.decode(parts.next()?).ok()?).ok()?;
    let offered = B64.decode(parts.next()?).ok()?;
    if parts.next().is_some() {
        return None;
    }
    let expected = mac(secret, instance, &session_key);
    bool::from(offered.ct_eq(&expected)).then_some(session_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn a_token_round_trips_and_names_its_session() {
        let token = mint(SECRET, "default", "claude/abc-123");
        assert_eq!(verify(SECRET, "default", &token).as_deref(), Some("claude/abc-123"));
        assert!(token.starts_with("rhn1."), "the version is readable in a log");
        assert!(
            !token.contains("claude/abc-123"),
            "the key is encoded, so a `.` in it cannot split the token"
        );
    }

    #[test]
    fn nothing_this_server_did_not_mint_verifies() {
        let token = mint(SECRET, "default", "claude/abc-123");

        assert_eq!(verify(b"a different secret", "default", &token), None);
        assert_eq!(
            verify(SECRET, "staging", &token),
            None,
            "the secret is machine-global, so the instance is what keeps siblings apart"
        );

        // A forged session key: the attacker knows the format and swaps the
        // middle segment, which is exactly what the MAC is over.
        let forged = format!(
            "{PREFIX}.{}.{}",
            B64.encode(b"claude/somebody-elses"),
            token.split('.').nth(2).expect("mac")
        );
        assert_eq!(verify(SECRET, "default", &forged), None);

        assert_eq!(verify(SECRET, "default", ""), None);
        assert_eq!(verify(SECRET, "default", "rhn1"), None);
        assert_eq!(verify(SECRET, "default", "rhn1.abc"), None);
        assert_eq!(
            verify(SECRET, "default", &format!("rhn0.{}", token.split_once('.').expect("split").1)),
            None
        );
        assert_eq!(verify(SECRET, "default", &format!("{token}.extra")), None);
        assert_eq!(verify(SECRET, "default", "rhn1.!!!.!!!"), None);
    }

    #[test]
    fn two_sessions_never_share_a_token() {
        let one = mint(SECRET, "default", "claude/a");
        let two = mint(SECRET, "default", "claude/b");
        assert_ne!(one, two);
        assert_eq!(verify(SECRET, "default", &one).as_deref(), Some("claude/a"));
        assert_eq!(verify(SECRET, "default", &two).as_deref(), Some("claude/b"));
    }
}
