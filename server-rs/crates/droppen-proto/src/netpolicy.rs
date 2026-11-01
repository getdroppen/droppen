//! Where a relay's origin may point (`docs/relay-and-plans.md` §2.5, §2.6).
//!
//! A relay names its own origin, and the Hub (proving it, probing it) and
//! every agent (attaching, measuring) dial it. A relay is not trusted with
//! the networks behind them: its origin has to be a public name or a public
//! address, checked once on the text and again on every address it resolves
//! to (a name that resolves to a public address when it is registered and to
//! `10.0.0.5` an hour later is refused then).
//!
//! Refused: loopback, private (RFC 1918, unique-local), link-local (with the
//! cloud metadata services on `169.254.169.254` and `fd00:ec2::254`), shared
//! (CGNAT, `100.64.0.0/10`, where Alibaba's metadata lives), unspecified,
//! broadcast, multicast, documentation, benchmarking and reserved ranges,
//! IPv4 hidden inside IPv6 (mapped, compatible, NAT64, 6to4, Teredo) when the
//! address inside is refused; and names that are not public by their form:
//! `localhost`, single labels (`redis`), `*.internal` (`hub.railway.internal`),
//! `*.local`, `*.localhost`, `*.localdomain`, `*.home.arpa`.
//!
//! Local testing (`DROPPEN_HUB_RELAY_INSECURE=1`, `DROPPEN_AGENT_RELAY_INSECURE=1`)
//! allows loopback, and only loopback, besides.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `ip` is a public unicast address a relay may be at.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

/// Whether `ip` is a loopback address (`127.0.0.0/8`, `::1`, or IPv4 loopback
/// mapped into IPv6).
pub fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }
}

/// Whether an address a relay's origin resolved to may be dialled.
pub fn allowed(ip: IpAddr, allow_loopback: bool) -> bool {
    is_public(ip) || (allow_loopback && is_loopback(ip))
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0                                   // "this network"
        || a == 10                             // private
        || a == 127                            // loopback
        || (a == 100 && (64..128).contains(&b)) // shared (CGNAT)
        || (a == 169 && b == 254)              // link-local, metadata
        || (a == 172 && (16..32).contains(&b)) // private
        || (a == 192 && b == 0 && c == 0)      // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)      // documentation
        || (a == 192 && b == 88 && c == 99)    // 6to4 relay anycast
        || (a == 192 && b == 168)              // private
        || (a == 198 && (18..20).contains(&b)) // benchmarking
        || (a == 198 && b == 51 && c == 100)   // documentation
        || (a == 203 && b == 0 && c == 113)    // documentation
        || a >= 224) // multicast, reserved, broadcast
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    // IPv4 inside IPv6: as public as the address inside.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    if s[0..6] == [0, 0, 0, 0, 0, 0] {
        // IPv4-compatible (deprecated): ::a.b.c.d
        return public_v4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    if s[0] == 0x64 && s[1] == 0xff9b {
        // NAT64 (64:ff9b::/96 and the local-use 64:ff9b:1::/48).
        return s[2] == 0 && public_v4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    if s[0] == 0x2002 {
        // 6to4: 2002:a.b.c.d::/48
        return public_v4(Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8));
    }
    !((s[0] & 0xfe00) == 0xfc00           // unique-local (fd00:ec2::254 among them)
        || (s[0] & 0xffc0) == 0xfe80      // link-local
        || (s[0] & 0xffc0) == 0xfec0      // site-local (deprecated)
        || (s[0] == 0x2001 && s[1] < 0x0200) // IETF special-purpose (Teredo, benchmarking, ORCHID)
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        || (s[0] == 0x0100 && s[1..4] == [0, 0, 0])) // discard-only
}

/// The host of a URL's authority, without brackets: `[::1]:8080` → `::1`.
pub fn host_of(authority: &str) -> &str {
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split_once(']').map_or(rest, |(h, _)| h);
    }
    authority.rsplit_once(':').map_or(authority, |(h, _)| h)
}

/// Whether a host name is not a public name by its form alone.
pub fn internal_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') {
        return true; // `localhost`, `redis`, `postgres`: a private network's names
    }
    const SUFFIXES: [&str; 6] = [".internal", ".local", ".localhost", ".localdomain", ".home.arpa", ".lan"];
    SUFFIXES.iter().any(|s| host.ends_with(s))
}

/// Whether the host of an origin may be dialled, judged on the text alone:
/// an address literal must be public (or loopback when allowed); a name
/// must look public (or be `localhost` when loopback is allowed). The
/// addresses a name resolves to are checked again with [`allowed`].
pub fn host_allowed(host: &str, allow_loopback: bool) -> bool {
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return allowed(ip, allow_loopback);
    }
    if allow_loopback && bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    !internal_name(bare)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn internal_and_special_addresses_are_refused() {
        for bad in [
            "10.0.0.5",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "127.0.0.1",
            "127.8.8.8",
            "169.254.169.254",
            "100.100.100.200",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "240.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "::",
            "::1",
            "fd00::1",
            "fd00:ec2::254",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::10.0.0.1",
            "64:ff9b::a00:1",
            "2002:a00:1::",
            "2001:db8::1",
            "2001::1",
        ] {
            assert!(!is_public(ip(bad)), "{bad} is not public");
            assert!(!allowed(ip(bad), false), "{bad}");
        }
    }

    #[test]
    fn public_addresses_pass() {
        for good in [
            "8.8.8.8",
            "1.1.1.1",
            "66.33.22.11",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700::1111",
            "2a00:1450::1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2002:808:808::",
        ] {
            assert!(is_public(ip(good)), "{good} is public");
        }
    }

    #[test]
    fn loopback_only_when_testing_locally() {
        assert!(!allowed(ip("127.0.0.1"), false));
        assert!(allowed(ip("127.0.0.1"), true));
        assert!(allowed(ip("::1"), true));
        assert!(allowed(ip("::ffff:127.0.0.1"), true));
        assert!(!allowed(ip("10.0.0.1"), true), "testing allows loopback, nothing else");
        assert!(!allowed(ip("169.254.169.254"), true));
    }

    #[test]
    fn names_that_are_internal_by_their_form_are_refused() {
        for bad in [
            "localhost",
            "redis",
            "hub.railway.internal",
            "metadata.google.internal",
            "printer.local",
            "a.localhost",
            "box.home.arpa",
            "10.0.0.5",
            "[fd00::1]",
            "169.254.169.254",
        ] {
            assert!(!host_allowed(bad, false), "{bad}");
        }
        for good in ["sin-1-production.up.railway.app", "relay.example.com", "8.8.8.8", "[2606:4700::1111]"] {
            assert!(host_allowed(good, false), "{good}");
        }
        assert!(host_allowed("localhost", true));
        assert!(host_allowed("127.0.0.1", true));
        assert!(!host_allowed("hub.railway.internal", true));
    }

    #[test]
    fn the_host_of_an_authority() {
        assert_eq!(host_of("relay.example:443"), "relay.example");
        assert_eq!(host_of("relay.example"), "relay.example");
        assert_eq!(host_of("[::1]:8080"), "::1");
        assert_eq!(host_of("[fd00::1]"), "fd00::1");
        assert_eq!(host_of("user@10.0.0.1:80"), "10.0.0.1");
    }
}
