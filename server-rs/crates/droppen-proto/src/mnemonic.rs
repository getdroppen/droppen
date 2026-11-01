//! The recovery key as words: BIP-39's encoding and its English list of 2048
//! words (`data/bip39-english.txt`, from bitcoin/bips, SHA-256
//! `2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda`).
//!
//! `RK` is client-generated random bytes (16 by default: 128 bits, twelve
//! words); the words are those bytes followed by the first `bits / 32` bits of
//! their SHA-256 as a checksum, cut into 11-bit indices. Only the encoding is
//! BIP-39's: the bytes are `RK` itself (no PBKDF2 seed), and what they derive
//! is [`crate::chain::recovery_keys`].

use sha2::{Digest, Sha256};
use thiserror::Error;
use zeroize::Zeroizing;

const LIST: &str = include_str!("data/bip39-english.txt");

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MnemonicError {
    #[error("a recovery key is 12, 15, 18, 21 or 24 words, not {0}")]
    WordCount(usize),
    #[error("{0:?} is not a recovery word")]
    UnknownWord(String),
    #[error("the words do not check out: one is wrong or out of place")]
    Checksum,
    #[error("a recovery key is 16 to 32 bytes, a multiple of 4")]
    Length,
}

/// The 2048 words, in order.
pub fn words() -> Vec<&'static str> {
    LIST.lines().collect()
}

/// `entropy` (16–32 bytes, a multiple of 4) as words.
pub fn encode(entropy: &[u8]) -> Result<Vec<&'static str>, MnemonicError> {
    if !(16..=32).contains(&entropy.len()) || !entropy.len().is_multiple_of(4) {
        return Err(MnemonicError::Length);
    }
    let list = words();
    let checksum_bits = entropy.len() * 8 / 32;
    let hash = Sha256::digest(entropy);
    let mut bits: Vec<bool> = Vec::with_capacity(entropy.len() * 8 + checksum_bits);
    for byte in entropy.iter().chain(hash.iter().take(1)) {
        for i in (0..8).rev() {
            bits.push(byte >> i & 1 == 1);
        }
    }
    bits.truncate(entropy.len() * 8 + checksum_bits);
    Ok(bits
        .chunks(11)
        .map(|chunk| list[chunk.iter().fold(0usize, |acc, &b| acc << 1 | usize::from(b))])
        .collect())
}

/// Words (any case, any whitespace between them) back to the entropy, with
/// the checksum checked.
pub fn decode(phrase: &str) -> Result<Zeroizing<Vec<u8>>, MnemonicError> {
    let list = words();
    let given: Vec<String> = phrase.split_whitespace().map(|w| w.to_lowercase()).collect();
    if ![12, 15, 18, 21, 24].contains(&given.len()) {
        return Err(MnemonicError::WordCount(given.len()));
    }
    let mut bits: Vec<bool> = Vec::with_capacity(given.len() * 11);
    for word in &given {
        let index = list.binary_search(&word.as_str()).map_err(|_| MnemonicError::UnknownWord(word.clone()))?;
        for i in (0..11).rev() {
            bits.push(index >> i & 1 == 1);
        }
    }
    let checksum_bits = given.len() * 11 / 33;
    let entropy_bits = bits.len() - checksum_bits;
    let entropy: Zeroizing<Vec<u8>> = Zeroizing::new(
        bits[..entropy_bits]
            .chunks(8)
            .map(|c| c.iter().fold(0u8, |acc, &b| acc << 1 | u8::from(b)))
            .collect(),
    );
    let hash = Sha256::digest(&*entropy);
    let expected = (0..checksum_bits).map(|i| hash[0] >> (7 - i) & 1 == 1);
    if !expected.eq(bits[entropy_bits..].iter().copied()) {
        return Err(MnemonicError::Checksum);
    }
    Ok(entropy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_bip39s() {
        let list = words();
        assert_eq!(list.len(), 2048);
        assert_eq!((list[0], list[2047]), ("abandon", "zoo"));
        assert!(list.windows(2).all(|w| w[0] < w[1]), "sorted, for lookup");
        let digest: [u8; 32] = Sha256::digest(LIST.as_bytes()).into();
        assert_eq!(
            hex::encode(digest),
            "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda"
        );
    }

    #[test]
    fn bip39_test_vectors() {
        // From the BIP-39 reference vectors (the entropy → words part).
        let cases: [(&str, &str); 3] = [
            ("00000000000000000000000000000000", "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"),
            ("7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f", "legal winner thank year wave sausage worth useful legal winner thank yellow"),
            (
                "0000000000000000000000000000000000000000000000000000000000000000",
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
            ),
        ];
        for (entropy, phrase) in cases {
            let entropy = hex::decode(entropy).expect("hex");
            assert_eq!(encode(&entropy).unwrap().join(" "), phrase);
            assert_eq!(*decode(phrase).unwrap(), entropy);
        }
    }

    #[test]
    fn typos_and_swaps_are_caught() {
        let rk = [0x5au8; 16];
        let words = encode(&rk).unwrap();
        let phrase = words.join(" ");
        assert_eq!(
            *decode(&format!("  {}  ", phrase.to_uppercase())).unwrap(),
            rk.to_vec(),
            "case and spacing do not matter"
        );
        let mut swapped = words.clone();
        swapped.swap(0, 1);
        if swapped != words {
            assert_eq!(decode(&swapped.join(" ")), Err(MnemonicError::Checksum));
        }
        assert!(matches!(
            decode(&phrase.replace(words[3], "notaword")),
            Err(MnemonicError::UnknownWord(_))
        ));
        assert_eq!(decode("abandon abandon"), Err(MnemonicError::WordCount(2)));
        assert_eq!(encode(&[0; 15]), Err(MnemonicError::Length));
    }
}
