//! The byte-level plumbing every layer shares: a bounds-checked reader, a few
//! big-endian writers, and the one error a malformed input can produce.
//!
//! Everything on this protocol's wire is big-endian and length-prefixed, and
//! nothing is self-describing beyond a leading type byte. That is deliberate:
//! the hub parses envelopes on every frame of every connection, so the encoding
//! is the cheapest one that cannot be misread, and the TypeScript half
//! (`web/src/lib/e2e/`) can implement it with a `DataView` and nothing else.

use thiserror::Error;

/// Why some bytes are not the thing they were supposed to encode.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    #[error("truncated input")]
    Truncated,
    #[error("{0} trailing byte(s) after a complete value")]
    TrailingBytes(usize),
    #[error("{what} is {len} bytes, more than the {max} allowed")]
    TooLarge { what: &'static str, len: usize, max: usize },
    #[error("unknown {what} {value:#04x}")]
    Unknown { what: &'static str, value: u8 },
    #[error("a string field is not valid UTF-8")]
    BadUtf8,
    #[error("invalid {0}")]
    Invalid(&'static str),
}

/// A cursor over a byte slice that refuses to read past its end.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::Truncated);
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    /// A `u64` no larger than [`MAX_SAFE_U64`]: every such field (seqs,
    /// milliseconds, nonces) is far below it, and the browser half reads them as
    /// JavaScript numbers, so both sides refuse the same values.
    pub(crate) fn u64(&mut self) -> Result<u64, DecodeError> {
        let value = u64::from_be_bytes(self.array()?);
        check_u64(value)?;
        Ok(value)
    }

    /// A `u16`-length-prefixed UTF-8 string of at most `max` bytes: invalid or
    /// non-canonical UTF-8 is refused, and a leading byte-order mark is part
    /// of the string (the browser half reads it so too: `vectors` `utf8`).
    pub(crate) fn str16(&mut self, what: &'static str, max: usize) -> Result<String, DecodeError> {
        let len = usize::from(self.u16()?);
        if len > max {
            return Err(DecodeError::TooLarge { what, len, max });
        }
        let raw = self.bytes(len)?;
        String::from_utf8(raw.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }

    /// Everything not yet read.
    pub(crate) fn rest(&mut self) -> &'a [u8] {
        let out = &self.buf[self.pos..];
        self.pos = self.buf.len();
        out
    }

    /// Succeeds only if every byte was consumed: a value with junk after it is
    /// not that value, and accepting it would give one message two encodings.
    pub(crate) fn finish(self) -> Result<(), DecodeError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(DecodeError::TrailingBytes(n)),
        }
    }
}

/// The largest `u64` any field may carry: 2^53 - 1, JavaScript's
/// `Number.MAX_SAFE_INTEGER`.
pub const MAX_SAFE_U64: u64 = (1 << 53) - 1;

/// Refuse a `u64` field beyond [`MAX_SAFE_U64`].
pub(crate) fn check_u64(value: u64) -> Result<(), DecodeError> {
    if value > MAX_SAFE_U64 {
        Err(DecodeError::Invalid("a u64 field beyond 2^53 - 1"))
    } else {
        Ok(())
    }
}

/// Append a `u16`-length-prefixed string. Callers validate the length first;
/// this only guards against the prefix silently wrapping.
pub(crate) fn put_str16(out: &mut Vec<u8>, value: &str) {
    let len = u16::try_from(value.len()).expect("string fields are validated to fit a u16 length");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// Append a `u16` count. As with [`put_str16`], callers bound the count
/// first (every list a decoder takes is far below `u16::MAX`); this never
/// clamps, so a count can never disagree with the items that follow it.
pub(crate) fn put_count(out: &mut Vec<u8>, count: usize) {
    let count = u16::try_from(count).expect("list counts are bounded to fit a u16");
    out.extend_from_slice(&count.to_be_bytes());
}

/// RFC 4648 base32, upper case, no padding: what a person types or reads aloud
/// (enrollment codes, fingerprints). Decoding is case-insensitive.
pub mod base32 {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
        let mut buffer: u32 = 0;
        let mut bits = 0;
        for &byte in data {
            buffer = (buffer << 8) | u32::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
            }
        }
        if bits > 0 {
            out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
        }
        out
    }

    /// `None` for any character outside the alphabet, or for trailing bits that
    /// are not zero (which would give one value two spellings).
    pub fn decode(text: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(text.len() * 5 / 8);
        let mut buffer: u32 = 0;
        let mut bits = 0;
        for c in text.bytes() {
            let value = ALPHABET.iter().position(|&a| a == c.to_ascii_uppercase())? as u32;
            buffer = (buffer << 5) | value;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                out.push(((buffer >> bits) & 0xff) as u8);
            }
        }
        if bits >= 5 || buffer & ((1 << bits) - 1) != 0 {
            return None;
        }
        Some(out)
    }
}

#[cfg(test)]
pub(crate) mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn decode(text: &str) -> Vec<u8> {
        assert!(text.len().is_multiple_of(2), "odd-length hex");
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_refuses_to_run_off_the_end_and_to_ignore_trailing_bytes() {
        let mut r = Reader::new(&[0, 1, 2]);
        assert_eq!(r.u16(), Ok(1));
        assert_eq!(r.u16(), Err(DecodeError::Truncated));
        assert_eq!(Reader::new(&[0, 0, 9]).finish(), Err(DecodeError::TrailingBytes(3)));
    }

    #[test]
    fn str16_enforces_its_bound_and_utf8() {
        let mut ok = Vec::new();
        put_str16(&mut ok, "héllo");
        assert_eq!(Reader::new(&ok).str16("name", 16).as_deref(), Ok("héllo"));
        assert!(matches!(Reader::new(&ok).str16("name", 2), Err(DecodeError::TooLarge { .. })));
        assert_eq!(Reader::new(&[0, 1, 0xff]).str16("name", 16), Err(DecodeError::BadUtf8));
    }

    #[test]
    fn base32_round_trips_and_matches_rfc4648() {
        // RFC 4648 §10 test vectors, unpadded.
        for (plain, encoded) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32::encode(plain.as_bytes()), encoded);
            assert_eq!(base32::decode(encoded).as_deref(), Some(plain.as_bytes()));
            assert_eq!(base32::decode(&encoded.to_lowercase()).as_deref(), Some(plain.as_bytes()));
        }
        assert_eq!(base32::decode("MZ"), None, "non-zero trailing bits are a second spelling");
        assert_eq!(base32::decode("M0"), None, "0 is not in the alphabet");
    }
}
