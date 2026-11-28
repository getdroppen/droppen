//! Anti-loop guardrails and `<cross-session-message>` envelope formatting.
//!
//! Two autonomous sessions allowed to reply to each other without a ceiling
//! will burn tokens until one hits a rate limit. Each `(caller, target)` pair
//! is therefore bounded to [`RATE_LIMIT_MAX`] messages inside [`RATE_WINDOW`],
//! and consecutive cross-session hops stop at [`MAX_HOPS`] until either a
//! person sends a message directly to the session ([`AgentsBus::reset_hops`])
//! or the chain goes quiet for [`HOP_RESET_AFTER`].

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use droppen_sessions::attention::seen::key_of;
use droppen_sessions::Source;

/// Maximum messages one caller may send to the same target inside [`RATE_WINDOW`].
const RATE_LIMIT_MAX: usize = 6;
/// Sliding window for per-pair rate limiting.
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// Maximum consecutive cross-session hops before human intervention is required.
pub(super) const MAX_HOPS: u32 = 6;
/// Idle interval after which consecutive hop counts reset automatically.
const HOP_RESET_AFTER: Duration = Duration::from_secs(10 * 60);

/// In-memory anti-loop and rate-limit state for the local agent bus.
#[derive(Default)]
pub struct AgentsBus {
    rate_windows: Mutex<HashMap<(String, String), VecDeque<Instant>>>,
    hop_state: Mutex<HashMap<String, (u32, Instant)>>,
}

/// Why a cross-session delivery was throttled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThrottleError {
    /// The caller sent too many messages to this target inside the window.
    RateLimited,
    /// Consecutive agent-to-agent hops reached [`MAX_HOPS`].
    HopLimitExceeded,
}

impl AgentsBus {
    /// Clear the consecutive hop count when a person sends a message directly
    /// to `(source, id)`.
    pub fn reset_hops(&self, source: Source, id: &str) {
        let key = key_of(source, id);
        self.hop_state.lock().unwrap().remove(&key);
    }

    /// Check rate and hop budgets for `from_key -> to_key` and record the
    /// delivery, returning the hop number to stamp on the envelope.
    pub(super) fn check_and_record(&self, from_key: &str, to_key: &str, now: Instant) -> Result<u32, ThrottleError> {
        let next_hop = {
            let hops = self.hop_state.lock().unwrap();
            let caller_hop = hops
                .get(from_key)
                .filter(|(_, at)| now.duration_since(*at) <= HOP_RESET_AFTER)
                .map(|(h, _)| *h)
                .unwrap_or(0);
            caller_hop.saturating_add(1)
        };
        if next_hop > MAX_HOPS {
            return Err(ThrottleError::HopLimitExceeded);
        }

        {
            let mut windows = self.rate_windows.lock().unwrap();
            let slot = windows.entry((from_key.to_string(), to_key.to_string())).or_default();
            while let Some(&front) = slot.front() {
                if now.duration_since(front) >= RATE_WINDOW {
                    slot.pop_front();
                } else {
                    break;
                }
            }
            if slot.len() >= RATE_LIMIT_MAX {
                return Err(ThrottleError::RateLimited);
            }
            slot.push_back(now);
        }

        self.hop_state.lock().unwrap().insert(to_key.to_string(), (next_hop, now));
        Ok(next_hop)
    }
}

fn escape_attr(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' | '\n' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// Wrap `message` in the `<cross-session-message>` envelope that Droppen's
/// transcript title picker and web renderer already recognise.
pub fn wrap_cross_session_message(from_ref: &str, from_name: &str, hop: u32, message: &str) -> String {
    let from = escape_attr(from_ref);
    let name = escape_attr(from_name);
    format!(
        "<cross-session-message from=\"{from}\" from-name=\"{name}\" hop=\"{hop}\">\n\n{}\n</cross-session-message>",
        message.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_attributes_are_escaped_and_title_line_keeps_the_body() {
        let wrapped = wrap_cross_session_message("acp/s1", "agy: fix \"quotes\" & <tags>\nline2", 2, "  hello peer  ");
        assert!(wrapped.starts_with(
            "<cross-session-message from=\"acp/s1\" from-name=\"agy: fix &quot;quotes&quot; &amp; &lt;tags&gt; line2\" hop=\"2\">\n\n"
        ));
        assert!(wrapped.ends_with("\nhello peer\n</cross-session-message>"));
        assert_eq!(droppen_sessions::util::title_line(&wrapped), "hello peer");
    }

    #[test]
    fn ping_pong_stops_at_the_hop_ceiling_until_a_human_turn_resets_it() {
        let bus = AgentsBus::default();
        let start = Instant::now();

        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Ok(1));
        assert_eq!(bus.check_and_record("claude/b", "acp/a", start), Ok(2));
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Ok(3));
        assert_eq!(bus.check_and_record("claude/b", "acp/a", start), Ok(4));
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Ok(5));
        assert_eq!(bus.check_and_record("claude/b", "acp/a", start), Ok(6));
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Err(ThrottleError::HopLimitExceeded));

        bus.reset_hops(Source::Acp, "a");
        assert_eq!(bus.check_and_record("acp/a", "claude/b", start), Ok(1));
    }

    #[test]
    fn a_pair_rate_limit_opens_again_after_the_sliding_window() {
        let bus = AgentsBus::default();
        let start = Instant::now();

        for i in 1..=RATE_LIMIT_MAX {
            assert_eq!(bus.check_and_record("acp/x", "claude/y", start), Ok(1), "slot {i}");
        }
        assert_eq!(bus.check_and_record("acp/x", "claude/y", start), Err(ThrottleError::RateLimited));
        assert_eq!(
            bus.check_and_record("acp/x", "claude/y", start + RATE_WINDOW + Duration::from_secs(1)),
            Ok(1)
        );
    }
}
