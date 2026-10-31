//! The wall clock. The protocol's functions take the time as an argument, so a
//! test can pin it; the code that runs them (the direct path, the relay, the
//! device client) reads it here.

/// Milliseconds since the Unix epoch, or zero for a clock that reads before it.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
