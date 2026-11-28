//! Exponential backoff with full jitter for reconnecting network links.

use std::time::Duration;

/// Exponential backoff with full jitter: each wait is drawn uniformly from
/// `0..=current` and the ceiling doubles up to `max`. A connection that lasted
/// longer than `stable` resets the ceiling back to `min`.
#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    stable: Duration,
    current: Duration,
}

impl Backoff {
    /// Construct a new backoff schedule starting at `min`, doubling up to `max`,
    /// and resetting after a session that stayed up longer than `stable`.
    pub fn new(min: Duration, max: Duration, stable: Duration) -> Self {
        let max = max.max(min);
        Self {
            min,
            max,
            stable,
            current: min,
        }
    }

    /// Return the jittered wait before the next attempt after a run that
    /// lasted `lasted`, advancing the ceiling for the subsequent attempt.
    pub fn after(&mut self, lasted: Duration) -> Duration {
        if lasted > self.stable {
            self.current = self.min;
        }
        let max_nanos = self.current.as_nanos().min(u64::MAX as u128) as u64;
        let wait = if max_nanos == 0 {
            Duration::ZERO
        } else {
            let bytes = uuid::Uuid::new_v4();
            let sample = u64::from_le_bytes(bytes.as_bytes()[..8].try_into().expect("8 bytes"));
            let nanos = if max_nanos == u64::MAX {
                sample
            } else {
                sample % (max_nanos + 1)
            };
            Duration::from_nanos(nanos)
        };
        self.current = self.current.saturating_mul(2).min(self.max);
        wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_respects_ceiling_and_resets_after_stable_run() {
        let mut backoff = Backoff::new(
            Duration::from_millis(100),
            Duration::from_millis(400),
            Duration::from_secs(10),
        );
        for ceiling_ms in [100u64, 200, 400, 400] {
            let wait = backoff.after(Duration::from_secs(1));
            assert!(wait <= Duration::from_millis(ceiling_ms), "{wait:?} exceeded {ceiling_ms}ms");
        }
        let reset_wait = backoff.after(Duration::from_secs(30));
        assert!(reset_wait <= Duration::from_millis(100), "stable run must reset ceiling: {reset_wait:?}");
    }
}
