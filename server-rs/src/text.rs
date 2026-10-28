//! Text helpers that more than one subsystem needs.
//!
//! Hex is re-exported from [`rh_client`], which spells it the way every binary
//! field in the hub's control messages does; `unhex` and [`unhex32`] read it
//! back, answering `None` for anything that is not hex; [`sha256_hex`] is the one
//! digest-to-hex spelling this crate uses. [`strip_ansi`] is the terminal
//! cleaner the setup wizard and the installer-output reader share — every escape
//! becomes a space, never nothing, so two words the escape sat between do not
//! splice together.

use sha2::{Digest, Sha256};

/// Lower-case hex, as `sha256sum` and the hub's wire fields spell it.
pub use rh_client::hex;

pub(crate) fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// 32 bytes from 64 hex characters.
pub fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Lower-case hex of the SHA-256 digest of `data`.
pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    hex(&Sha256::digest(data.as_ref()))
}

/// Every escape is replaced by a space, never deleted.
///
/// Deleting it would splice the trailing "Store this token…" hint onto the
/// token itself and produce a silently unusable account, so the separator has
/// to survive the strip.
pub fn strip_ansi(text: &str) -> String {
    ansi_re().replace_all(text, " ").into_owned()
}

fn ansi_re() -> &'static regex::Regex {
    static ANSI: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ANSI.get_or_init(|| {
        regex::Regex::new(concat!(
            r"[\x1b\u{9b}]",
            r"(?:",
            r"\[[0-?]*[ -/]*[@-~]",                  // CSI
            r"|\][^\x07\x1b]{0,512}(?:\x07|\x1b\\)", // OSC, terminated by BEL or ST
            r"|[@-Z\\-_]",                           // Fe, two bytes
            r")",
        ))
        .expect("valid ANSI pattern")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_is_lowercase_hex() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&[0x00, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn an_escape_becomes_one_space_whatever_its_kind() {
        const ESC: char = '\u{1b}';
        assert_eq!(strip_ansi(&format!("red{ESC}[31mgreen")), "red green");
        assert_eq!(strip_ansi(&format!("{ESC}[2J{ESC}[Hplain")), "  plain");
        assert_eq!(strip_ansi(&format!("{ESC}]0;a title\u{7}after")), " after");
    }
}
