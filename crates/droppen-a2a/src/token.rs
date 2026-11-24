//! A2A tokens: `Authorization: Bearer dpa_<id>_<secret>` (`docs/a2a.md` §2).
//!
//! The id finds the row; the secret proves it. Only `sha256(secret)` is kept,
//! compared in constant time, and the whole token is shown once, when it is
//! made. A token reaches the entries its scope lists and no other: an entry
//! outside it answers exactly as one that does not exist.

use base64::Engine as _;
use rand::RngCore as _;

use crate::store::TokenRow;

/// Every token starts so.
pub const PREFIX: &str = "dpa_";

/// Random bytes in a token's id: 64 bits, hex — enough not to collide, and
/// not a secret.
const ID_BYTES: usize = 8;

/// Random bytes in a token's secret: 256 bits, base64url.
const SECRET_BYTES: usize = 32;

/// A new token: its id, the whole `dpa_<id>_<secret>` to show once, and the
/// digest to keep.
pub fn mint() -> (String, String, String) {
    let mut id = [0u8; ID_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut id);
    let mut secret = [0u8; SECRET_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let id = droppen_base::text::hex(&id);
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret);
    let digest = droppen_base::text::sha256_hex(&secret);
    (id.clone(), format!("{PREFIX}{id}_{secret}"), digest)
}

/// The `(id, secret)` a bearer value spells, if it is a token at all.
pub fn parse(bearer: &str) -> Option<(&str, &str)> {
    let rest = bearer.strip_prefix(PREFIX)?;
    let (id, secret) = rest.split_once('_')?;
    let id_ok = id.len() == ID_BYTES * 2 && id.bytes().all(|b| b.is_ascii_hexdigit());
    (id_ok && !secret.is_empty() && secret.len() <= 128).then_some((id, secret))
}

/// Whether `secret` is the one `row` was made with, and the row still stands.
pub fn proves(row: &TokenRow, secret: &str) -> bool {
    let offered = droppen_base::text::sha256_hex(secret);
    let matches = bool::from(subtle::ConstantTimeEq::ct_eq(offered.as_bytes(), row.secret_hash.as_bytes()));
    matches && row.revoked_ms.is_none()
}

/// Whether `name` (trimmed) may name a token: a label for a person, at most
/// 100 characters and none of them a control character. Two tokens may share
/// one; it identifies nothing ([`caller_of`]).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= 100 && !name.chars().any(char::is_control)
}

/// The caller a token speaks as: `a2a:token:<id>`. By its id, never its
/// name, so a token made later under the name of a revoked one is another
/// caller and sees none of its tasks (§6).
pub fn caller_of(token_id: &str) -> String {
    format!("a2a:token:{token_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(digest: String) -> TokenRow {
        TokenRow {
            id: "0011223344556677".into(),
            name: "dont-do".into(),
            entries: vec!["main".into()],
            secret_hash: digest,
            created_ms: 0,
            last_used_ms: None,
            revoked_ms: None,
        }
    }

    #[test]
    fn a_minted_token_parses_back_and_proves_only_its_own_secret() {
        let (id, token, digest) = mint();
        let (parsed_id, secret) = parse(&token).expect("a token");
        assert_eq!(parsed_id, id);
        assert!(!token.contains(&digest), "the digest is not in the token");
        let mut kept = row(digest);
        assert!(proves(&kept, secret));
        assert!(!proves(&kept, "another"));
        kept.revoked_ms = Some(1);
        assert!(!proves(&kept, secret), "a revoked token proves nothing");
    }

    #[test]
    fn only_a_dpa_token_parses() {
        assert!(
            parse("dpa_0011223344556677_abc_def").is_some(),
            "a base64url secret may hold an underscore"
        );
        assert!(parse("dpa_0011223344556677_").is_none());
        assert!(parse("dpa_short_abc").is_none());
        assert!(parse("Bearer dpa_0011223344556677_abc").is_none());
        assert!(parse("some-login-token").is_none());
    }
}
