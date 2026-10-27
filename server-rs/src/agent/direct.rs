//! The agent's side of direct paths (`docs/cross-machine-architecture.md`
//! §9, "直连通道"): a device that reached this machine through the hub may
//! ask, inside that end-to-end channel, for a WebRTC DataChannel straight to
//! it; when one comes up, the channel's frames move onto it and fall back to
//! the hub when it fails ([`rh_proto::path`], [`rh_proto::direct`]).
//!
//! - **Off switch**: `DROPPEN_AGENT_DIRECT=0` turns direct paths off for this
//!   instance: the agent then answers the signaling stream with 403, binds no
//!   UDP socket and offers no `direct/1` feature. Channels still negotiate
//!   sequenced frames (`paths/1`), which costs nothing and gives the
//!   device's path indicator the hub's round-trip time.
//! - **Sockets**: bound per attempt by [`rh_proto::rtc`] — on the IPv4 and
//!   IPv6 default-route addresses, never loopback or link-local — and closed
//!   when the attempt fails or the direct path ends. Only an enrolled
//!   instance runs the link, so an instance that was never enrolled binds
//!   nothing at all.
//! - **STUN**: Cloudflare and Google by default; `DROPPEN_AGENT_DIRECT_STUN` is a
//!   comma-separated list to use instead (empty: none, host candidates only).
//!   No TURN: the hub is the fallback.
//! - **Admission**: the direct hello is admitted by `handshake::accept_direct`
//!   with this agent's live chain, clock, replay guard and attestations —
//!   the rules every hub channel is admitted by — see [`super::link`].
//! - At most [`MAX_PEERS`] peers at once across all channels.

use std::time::Duration;

use rh_proto::rtc::{RtcOptions, DEFAULT_STUN};

/// Peers (each with its own sockets) at once, across every channel.
pub const MAX_PEERS: usize = 16;

/// Whether direct paths are on for this instance (`DROPPEN_AGENT_DIRECT=0` turns them off).
pub fn enabled() -> bool {
    crate::env::flag_or("DROPPEN_AGENT_DIRECT", true)
}

/// The peers' options: default-route host candidates and STUN.
pub fn options() -> RtcOptions {
    RtcOptions {
        stun: stun_from(crate::env::var_or_empty("DROPPEN_AGENT_DIRECT_STUN").as_deref()),
        default_route: true,
        loopback: false,
        blackhole: None,
    }
}

fn stun_from(value: Option<&str>) -> Vec<String> {
    match value {
        None => DEFAULT_STUN.iter().map(|s| s.to_string()).collect(),
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.contains(':'))
            .map(str::to_string)
            .collect(),
    }
}

/// How long an attempt may take from offer to attached.
pub const ATTEMPT_TIMEOUT: Duration = rh_proto::direct::ATTEMPT_TIMEOUT;

/// The features this agent offers devices in its handshake reply.
pub fn features(direct: bool) -> Vec<&'static str> {
    let mut f = vec![
        "rpc",
        "mux",
        "term",
        "blob",
        "local",
        rh_proto::path::CLIENT_FEATURE,
        rh_proto::stream::STREAMS_FEATURE,
        super::CHAIN_V3_FEATURE,
        super::POLICY_FEATURE,
    ];
    if direct {
        f.push("direct/1");
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stun_list_parses() {
        assert_eq!(stun_from(None), DEFAULT_STUN.map(String::from).to_vec());
        assert!(stun_from(Some("")).is_empty(), "empty: host candidates only");
        assert_eq!(stun_from(Some("stun.example:3478, junk ,")), ["stun.example:3478"]);
    }

    #[test]
    fn direct_is_offered_only_when_on() {
        assert!(features(true).contains(&"direct/1"));
        assert!(!features(false).contains(&"direct/1"));
        assert!(features(false).contains(&"paths/1"), "sequenced frames either way");
    }

    #[test]
    fn production_peers_never_offer_loopback() {
        let o = options();
        assert!(o.default_route && !o.loopback && o.blackhole.is_none());
    }
}
