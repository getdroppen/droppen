//! Clocks for the protocol. The protocol's functions take the time as an
//! argument, so a test can pin it; the code that runs them (the direct path,
//! the relay, the device client) reads it here.

use std::sync::OnceLock;
use std::time::Instant;

/// Milliseconds since the Unix epoch, or zero for a clock that reads before it.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Monotonic microseconds anchored to the wall clock at first call.
///
/// Immune to NTP step-backs and wall-clock jumps (for path RTT, pacing, and
/// direct-path timeouts), while keeping `monotonic_us() / 1000` aligned with
/// Unix epoch milliseconds for Noise rekey-age comparisons.
pub fn monotonic_us() -> u64 {
    static ANCHOR: OnceLock<(Instant, u64)> = OnceLock::new();
    let (start, base_us) = ANCHOR.get_or_init(|| {
        let wall_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        (Instant::now(), wall_us)
    });
    base_us.saturating_add(start.elapsed().as_micros() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_us_is_non_decreasing_and_epoch_aligned() {
        let a = monotonic_us();
        let b = monotonic_us();
        assert!(b >= a);
        assert!(a / 1000 > 1_700_000_000_000);
    }
}
