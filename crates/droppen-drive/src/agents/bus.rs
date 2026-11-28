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
/// Closing tag prefix neutralized in cross-session message bodies.
const CLOSING_TAG_PREFIX: &str = "</cross-session-message";

#[derive(Default)]
struct BusState {
    rate_windows: HashMap<(String, String), VecDeque<Instant>>,
    hop_state: HashMap<String, (u32, Instant)>,
}

impl BusState {
    fn prune(&mut self, now: Instant) {
        self.hop_state
            .retain(|_, (_, at)| now.saturating_duration_since(*at) <= HOP_RESET_AFTER);
        self.rate_windows.retain(|_, slot| {
            while let Some(&front) = slot.front() {
                if now.saturating_duration_since(front) >= RATE_WINDOW {
                    slot.pop_front();
                } else {
                    break;
                }
            }
            !slot.is_empty()
        });
    }
}

/// In-memory anti-loop and rate-limit state for the local agent bus.
#[derive(Default)]
pub struct AgentsBus {
    state: Mutex<BusState>,
}

/// A reserved rate-limit slot and hop count awaiting provider delivery.
///
/// Call [`Self::commit`] once the target accepts the message, or
/// [`Self::rollback`] (also invoked automatically on drop) if delivery fails so
/// failed sends neither burn rate quota nor advance the target's hop streak.
pub(super) struct DeliveryReservation<'a> {
    bus: &'a AgentsBus,
    from_key: String,
    to_key: String,
    reserved_at: Instant,
    hop: u32,
    active: bool,
}

impl DeliveryReservation<'_> {
    /// Consecutive hop count to stamp on the `<cross-session-message>` envelope.
    pub(super) fn hop(&self) -> u32 {
        self.hop
    }

    /// Finalize the delivery, monotonically advancing the target's hop streak.
    pub(super) fn commit(self) {
        self.commit_at(Instant::now());
    }

    /// Finalize the delivery using an explicit timestamp.
    pub(super) fn commit_at(mut self, now: Instant) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut state = self.bus.state.lock().unwrap_or_else(|e| e.into_inner());
        state.prune(now);
        let existing = state
            .hop_state
            .get(&self.to_key)
            .filter(|(_, at)| now.saturating_duration_since(*at) <= HOP_RESET_AFTER)
            .map(|(h, _)| *h)
            .unwrap_or(0);
        state.hop_state.insert(self.to_key.clone(), (existing.max(self.hop), now));
    }

    /// Release the reserved rate-limit slot without advancing target hop state.
    pub(super) fn rollback(mut self) {
        self.rollback_inner();
    }

    fn rollback_inner(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut state = self.bus.state.lock().unwrap_or_else(|e| e.into_inner());
        let key = (self.from_key.clone(), self.to_key.clone());
        let empty = if let Some(slot) = state.rate_windows.get_mut(&key) {
            if let Some(idx) = slot.iter().rposition(|&t| t == self.reserved_at) {
                slot.remove(idx);
            }
            slot.is_empty()
        } else {
            false
        };
        if empty {
            state.rate_windows.remove(&key);
        }
    }
}

impl Drop for DeliveryReservation<'_> {
    fn drop(&mut self) {
        self.rollback_inner();
    }
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
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.hop_state.remove(&key);
    }

    /// Validate rate and hop budgets for `from_key -> to_key` and reserve a
    /// rate-limit slot without holding any lock across an async delivery.
    pub(super) fn reserve<'a>(
        &'a self,
        from_key: &str,
        to_key: &str,
        now: Instant,
    ) -> Result<DeliveryReservation<'a>, ThrottleError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.prune(now);

        let caller_hop = state
            .hop_state
            .get(from_key)
            .filter(|(_, at)| now.saturating_duration_since(*at) <= HOP_RESET_AFTER)
            .map(|(h, _)| *h)
            .unwrap_or(0);
        let next_hop = caller_hop.saturating_add(1);
        if next_hop > MAX_HOPS {
            return Err(ThrottleError::HopLimitExceeded);
        }

        let slot = state
            .rate_windows
            .entry((from_key.to_string(), to_key.to_string()))
            .or_default();
        if slot.len() >= RATE_LIMIT_MAX {
            return Err(ThrottleError::RateLimited);
        }
        slot.push_back(now);

        Ok(DeliveryReservation {
            bus: self,
            from_key: from_key.to_string(),
            to_key: to_key.to_string(),
            reserved_at: now,
            hop: next_hop,
            active: true,
        })
    }

    #[cfg(test)]
    pub(crate) fn hop_for(&self, key: &str) -> Option<u32> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.hop_state.get(key).map(|(h, _)| *h)
    }

    #[cfg(test)]
    pub(crate) fn record_hop_for_test(&self, source: Source, id: &str, hop: u32, at: Instant) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.hop_state.insert(key_of(source, id), (hop, at));
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

/// Neutralize any case variant of `</cross-session-message` inside a peer body
/// so a message cannot break out of the outer XML envelope.
fn neutralize_closing_tags(raw: &str) -> String {
    let needle_len = CLOSING_TAG_PREFIX.len();
    if raw.len() < needle_len {
        return raw.to_string();
    }
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut cursor = 0;
    let mut i = 0;
    while i + needle_len <= bytes.len() {
        if bytes[i] == b'<' && bytes[i..i + needle_len].eq_ignore_ascii_case(CLOSING_TAG_PREFIX.as_bytes()) {
            out.push_str(&raw[cursor..i]);
            out.push_str("&lt;/cross-session-message");
            i += needle_len;
            cursor = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&raw[cursor..]);
    out
}

/// Wrap `message` in the `<cross-session-message>` envelope that Droppen's
/// transcript title picker and web renderer already recognise.
pub fn wrap_cross_session_message(from_ref: &str, from_name: &str, hop: u32, message: &str) -> String {
    let from = escape_attr(from_ref);
    let name = escape_attr(from_name);
    let body = neutralize_closing_tags(message.trim());
    format!(
        "<cross-session-message from=\"{from}\" from-name=\"{name}\" hop=\"{hop}\">\n\n{body}\n</cross-session-message>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deliver(bus: &AgentsBus, from: &str, to: &str, now: Instant) -> Result<u32, ThrottleError> {
        let reservation = bus.reserve(from, to, now)?;
        let hop = reservation.hop();
        reservation.commit_at(now);
        Ok(hop)
    }

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
    fn closing_envelope_tags_in_body_are_neutralized_case_insensitively() {
        let body = "first </cross-session-message> middle </CROSS-SESSION-MESSAGE > and </Cross-Session-Message> 日本語";
        let wrapped = wrap_cross_session_message("acp/s1", "peer", 1, body);
        let inner = wrapped
            .strip_prefix("<cross-session-message from=\"acp/s1\" from-name=\"peer\" hop=\"1\">\n\n")
            .and_then(|s| s.strip_suffix("\n</cross-session-message>"))
            .expect("outer envelope intact");
        assert!(
            !inner.to_ascii_lowercase().contains("</cross-session-message"),
            "closing tag escaped in inner body: {inner}"
        );
        assert_eq!(
            inner,
            "first &lt;/cross-session-message> middle &lt;/cross-session-message > and &lt;/cross-session-message> 日本語"
        );
    }

    #[test]
    fn ping_pong_stops_at_the_hop_ceiling_until_a_human_turn_resets_it() {
        let bus = AgentsBus::default();
        let start = Instant::now();

        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Ok(1));
        assert_eq!(deliver(&bus, "claude/b", "acp/a", start), Ok(2));
        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Ok(3));
        assert_eq!(deliver(&bus, "claude/b", "acp/a", start), Ok(4));
        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Ok(5));
        assert_eq!(deliver(&bus, "claude/b", "acp/a", start), Ok(6));
        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Err(ThrottleError::HopLimitExceeded));

        bus.reset_hops(Source::Acp, "a");
        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Ok(1));
    }

    #[test]
    fn a_third_agent_cannot_reset_a_sessions_higher_hop_count() {
        let bus = AgentsBus::default();
        let start = Instant::now();

        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Ok(1));
        assert_eq!(deliver(&bus, "claude/b", "acp/a", start), Ok(2));
        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Ok(3));
        assert_eq!(deliver(&bus, "claude/b", "acp/a", start), Ok(4));
        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Ok(5));
        assert_eq!(deliver(&bus, "claude/b", "acp/a", start), Ok(6));

        // A fresh third peer sends at hop 1 to `acp/a`; `acp/a` must stay at hop 6.
        assert_eq!(deliver(&bus, "codex/c", "acp/a", start), Ok(1));
        assert_eq!(bus.hop_for("acp/a"), Some(6));
        assert_eq!(deliver(&bus, "acp/a", "claude/b", start), Err(ThrottleError::HopLimitExceeded));
    }

    #[test]
    fn a_pair_rate_limit_opens_again_after_the_sliding_window() {
        let bus = AgentsBus::default();
        let start = Instant::now();

        for i in 1..=RATE_LIMIT_MAX {
            assert_eq!(deliver(&bus, "acp/x", "claude/y", start), Ok(1), "slot {i}");
        }
        assert_eq!(deliver(&bus, "acp/x", "claude/y", start), Err(ThrottleError::RateLimited));
        assert_eq!(
            deliver(&bus, "acp/x", "claude/y", start + RATE_WINDOW + Duration::from_secs(1)),
            Ok(1)
        );
    }

    #[test]
    fn failed_delivery_rollback_and_drop_preserve_quota_and_hops() {
        let bus = AgentsBus::default();
        let start = Instant::now();

        for _ in 0..(RATE_LIMIT_MAX + 3) {
            let res = bus.reserve("acp/x", "claude/y", start).expect("rollback keeps quota free");
            assert_eq!(res.hop(), 1);
            res.rollback();
            assert_eq!(bus.hop_for("claude/y"), None);
        }

        // Dropping without committing also rolls back automatically.
        {
            let res = bus.reserve("acp/x", "claude/y", start).expect("slot");
            assert_eq!(res.hop(), 1);
        }
        assert_eq!(bus.hop_for("claude/y"), None);
        let state = bus.state.lock().unwrap();
        assert!(state.rate_windows.is_empty(), "rolled-back slots leave no empty keys");
    }

    #[test]
    fn expired_entries_are_pruned_opportunistically() {
        let bus = AgentsBus::default();
        let start = Instant::now();
        assert_eq!(deliver(&bus, "acp/old1", "claude/old2", start), Ok(1));

        let later = start + HOP_RESET_AFTER + Duration::from_secs(1);
        assert_eq!(deliver(&bus, "acp/new1", "claude/new2", later), Ok(1));

        let state = bus.state.lock().unwrap();
        assert!(!state.hop_state.contains_key("claude/old2"));
        assert!(!state.rate_windows.contains_key(&("acp/old1".to_string(), "claude/old2".to_string())));
        assert_eq!(state.hop_state.len(), 1);
        assert_eq!(state.rate_windows.len(), 1);
    }
}
