//! Deterministic randomness for tests and test vectors.
//!
//! Vectors shared with the TypeScript half have to come out byte-identical on
//! every run, so every nonce and one-off key in them is drawn from this rather
//! than from the OS: SHA-256 in counter mode over a seed. Never used outside
//! `#[cfg(test)]`.

use rand_core::{CryptoRng, Error, RngCore};
use sha2::{Digest, Sha256};

pub(crate) struct TestRng {
    seed: u64,
    counter: u64,
    buf: [u8; 32],
    pos: usize,
}

pub(crate) fn rng(seed: u64) -> TestRng {
    TestRng {
        seed,
        counter: 0,
        buf: [0; 32],
        pos: 32,
    }
}

impl RngCore for TestRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }

    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for byte in dest {
            if self.pos == 32 {
                let mut h = Sha256::new();
                h.update(b"rh-proto test rng");
                h.update(self.seed.to_be_bytes());
                h.update(self.counter.to_be_bytes());
                self.buf = h.finalize().into();
                self.counter += 1;
                self.pos = 0;
            }
            *byte = self.buf[self.pos];
            self.pos += 1;
        }
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for TestRng {}

/// 32 deterministic bytes, for fixed keys.
pub(crate) fn bytes32(seed: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    rng(seed).fill_bytes(&mut out);
    out
}
