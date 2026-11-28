//! The guards every outbound webhook in this server shares.
//!
//! Both channels — Web Push (the server's `push`) and Bark
//! (`notify::bark`) — take an endpoint from stored configuration and
//! POST to it. That endpoint is the one URL in a request path that ends in an
//! outbound connection, so the two rules that keep it from becoming an SSRF
//! jump live here, in one audited place:
//!
//! * [`endpoint_is_acceptable`] — HTTPS, or plain HTTP only for loopback and
//!   only under the channel's own opt-in. It is a *first-hop* check.
//! * [`no_redirect_client`] — a client that refuses to follow a redirect, the
//!   only way an endpoint that passed the first-hop check could still reach a
//!   loopback-only service on this host.
//!
//! The opt-in variables stay separate per channel on purpose: turning on
//! insecure *push* endpoints to run a test must not also turn on insecure
//! notification endpoints. Each caller reads its own switch and passes the
//! answer in.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

/// Whether `ip` is a loopback address (`127.0.0.0/8`, `::1`, or IPv4-mapped loopback).
fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }
}

/// Whether `ip` is a globally routable IPv4 address (refusing loopback, private,
/// link-local/metadata, CGNAT, unspecified, documentation, multicast and reserved).
fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..128).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (18..20).contains(&b))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

/// Whether `ip` is a globally routable IPv6 address (also unwrapping embedded IPv4).
fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    if s[0..6] == [0, 0, 0, 0, 0, 0] {
        return is_public_v4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    if s[0] == 0x64 && s[1] == 0xff9b {
        return s[2] == 0 && is_public_v4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    if s[0] == 0x2002 {
        return is_public_v4(Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8));
    }
    !((s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xffc0) == 0xfec0
        || (s[0] == 0x2001 && s[1] < 0x0200)
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        || (s[0] == 0x0100 && s[1..4] == [0, 0, 0]))
}

/// Whether `host` is a non-public internal name (`localhost`, single label, or private suffix).
fn is_internal_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') {
        return true;
    }
    const SUFFIXES: [&str; 6] = [".internal", ".local", ".localhost", ".localdomain", ".home.arpa", ".lan"];
    SUFFIXES.iter().any(|s| host.ends_with(s))
}

/// Whether `host` may be targeted by an outbound webhook.
fn host_is_acceptable(host: &str, allow_loopback: bool) -> bool {
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        let public = match ip {
            IpAddr::V4(v4) => is_public_v4(v4),
            IpAddr::V6(v6) => is_public_v6(v6),
        };
        return public || (allow_loopback && is_loopback(ip));
    }
    if allow_loopback && bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    !is_internal_name(bare)
}

/// Whether an endpoint is somewhere this server is willing to send to.
///
/// HTTPS to a public host; loopback (HTTP or HTTPS) only when the caller's
/// channel opted in for local testing. Private, link-local, metadata, and
/// internal hostnames are refused to block SSRF.
pub fn endpoint_is_acceptable(endpoint: &str, insecure_endpoints_allowed: bool) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    match url.scheme() {
        "https" => host_is_acceptable(host, insecure_endpoints_allowed),
        "http" if insecure_endpoints_allowed => {
            let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
            bare.eq_ignore_ascii_case("localhost") || bare.parse::<IpAddr>().is_ok_and(is_loopback)
        }
        _ => false,
    }
}

/// An endpoint's scheme, host and port, and never its path.
///
/// The path of a webhook endpoint is a per-device capability — whoever holds it
/// can send that device notifications — so only the origin is ever written to a
/// log line or shown in an API response. It is enough to tell a hosted service
/// from a self-hosted one, and none of the secret.
pub fn endpoint_origin(endpoint: &str) -> String {
    if endpoint.is_empty() {
        return String::new();
    }
    match reqwest::Url::parse(endpoint) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
            (Some(host), None) => format!("{}://{host}", url.scheme()),
            (None, _) => format!("{}://<no-host>", url.scheme()),
        },
        Err(_) => "<unparseable>".to_string(),
    }
}

/// A client for an outbound webhook that never follows a redirect.
///
/// Both channels share the rule and differ only in the timeout: a push goes out
/// on a background task, while a Bark nudge has an agent's `rh-notify` waiting
/// on the answer. Building a client means building a rustls configuration, so
/// each channel keeps one shared instance rather than one per request.
///
/// `version` is the server's own (its `CARGO_PKG_VERSION`, which the crates it
/// is made of read as `droppen_http::PACKAGE_VERSION`), which the user agent
/// `remote-hand/<version>` carries: this crate's version is not the release's.
pub fn no_redirect_client(timeout: Duration, version: &str) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("remote-hand/{version}"))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_blocks_loopback_private_metadata_and_internal_hosts() {
        assert!(endpoint_is_acceptable("https://api.day.app/AbCd1234", false));
        assert!(endpoint_is_acceptable("https://fcm.googleapis.com/fcm/send/abc", false));
        for blocked in [
            "https://127.0.0.1/hook",
            "https://[::1]/hook",
            "https://[::ffff:127.0.0.1]/hook",
            "https://localhost/hook",
            "https://169.254.169.254/latest/meta-data/",
            "https://[fd00:ec2::254]/latest/meta-data/",
            "https://10.0.0.5/hook",
            "https://172.16.0.1/hook",
            "https://192.168.1.5/hook",
            "https://100.64.0.1/hook",
            "https://0.0.0.0/hook",
            "https://redis/hook",
            "https://service.internal/hook",
            "https://printer.local/hook",
        ] {
            assert!(!endpoint_is_acceptable(blocked, false), "must reject {blocked}");
        }
        assert!(endpoint_is_acceptable("https://127.0.0.1:8443/hook", true));
        assert!(endpoint_is_acceptable("http://127.0.0.1:8443/hook", true));
        assert!(!endpoint_is_acceptable("https://169.254.169.254/latest/meta-data/", true));
    }
}
