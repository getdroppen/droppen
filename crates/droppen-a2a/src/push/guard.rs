//! Where a push may go (`docs/a2a.md` §6).
//!
//! A push configuration's URL comes from the A2A client, so it is the one
//! place where a caller chooses what this device connects to. Three checks
//! hold it, each where it cannot be stepped around:
//!
//! * [`push_target`] reads the URL with the parser the HTTP client reads it
//!   with (`url`, WHATWG), so the two never see different hosts: `https`, no
//!   credentials in it, and an origin (scheme, host in its ASCII form, port
//!   with the scheme's default) that the entry's `pushOrigins` lists exactly.
//!   An address literal is never resolved, so it is held to [`refused`] here.
//! * [`refused`] names the address classes a push never goes to: loopback,
//!   private, link-local, shared (CGNAT, Tailscale's `100.64.0.0/10`), the
//!   unspecified and broadcast addresses, multicast, the documentation and
//!   benchmarking ranges, reserved space, and every form of IPv4 inside IPv6
//!   (mapped, compatible, NAT64, 6to4, Teredo) whatever the address inside.
//!   That last rule is stricter than `droppen_proto::netpolicy`'s on purpose:
//!   an embedded address is a way to reach one of the others.
//! * [`Guarded`] is the client's resolver: what a host name resolves to is
//!   held to [`refused`] when the connection is made, and only the addresses
//!   that pass are handed to the connector. There is no second lookup for an
//!   answer to change in (DNS rebinding): the address checked is the address
//!   connected to.
//!
//! The one exception to the classes is an address `pushOrigins` names
//! literally, at the URL's port (`https://10.0.0.5:8443`): the person who set
//! the entry up said so. The TCK's plain-http webhook is the other
//! ([`tck_plain_http`]).

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use reqwest::Url;

use crate::backend::EntrySource;

/// Why an address is not one a push goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `0.0.0.0/8`, "this network".
    ThisNetwork,
    /// `::`.
    Unspecified,
    /// `127.0.0.0/8`, `::1`.
    Loopback,
    /// `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`.
    Private,
    /// `100.64.0.0/10`: carrier-grade NAT, and Tailscale's addresses.
    Shared,
    /// `169.254.0.0/16` (the cloud metadata services), `fe80::/10`.
    LinkLocal,
    /// `fec0::/10`, deprecated but still routed by some networks.
    SiteLocal,
    /// `fc00::/7`.
    UniqueLocal,
    /// `224.0.0.0/4`, `ff00::/8`.
    Multicast,
    /// `255.255.255.255`.
    Broadcast,
    /// `240.0.0.0/4`.
    Reserved,
    /// `192.0.0.0/24`: the IETF's protocol assignments.
    ProtocolAssignments,
    /// `192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`, `2001:db8::/32`,
    /// `3fff::/20`.
    Documentation,
    /// `198.18.0.0/15`, `2001:2::/48`.
    Benchmarking,
    /// `::ffff:0:0/96`.
    Ipv4Mapped,
    /// `::/96` (`::a.b.c.d`).
    Ipv4Compatible,
    /// `64:ff9b::/96`, `64:ff9b:1::/48`.
    Nat64,
    /// `2002::/16`, and its relays' `192.88.99.0/24`.
    SixToFour,
    /// `2001::/32`.
    Teredo,
    /// `100::/64`, discard-only.
    Discard,
    /// `2001:10::/28`, `2001:20::/28`.
    Orchid,
    /// Anything else outside `2000::/3`, the global unicast space.
    NotGlobal,
}

impl Class {
    /// What it is called in a log line.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::ThisNetwork => "this-network",
            Class::Unspecified => "unspecified",
            Class::Loopback => "loopback",
            Class::Private => "private",
            Class::Shared => "shared (CGNAT)",
            Class::LinkLocal => "link-local",
            Class::SiteLocal => "site-local",
            Class::UniqueLocal => "unique-local",
            Class::Multicast => "multicast",
            Class::Broadcast => "broadcast",
            Class::Reserved => "reserved",
            Class::ProtocolAssignments => "IETF protocol assignments",
            Class::Documentation => "documentation",
            Class::Benchmarking => "benchmarking",
            Class::Ipv4Mapped => "IPv4-mapped",
            Class::Ipv4Compatible => "IPv4-compatible",
            Class::Nat64 => "NAT64",
            Class::SixToFour => "6to4",
            Class::Teredo => "Teredo",
            Class::Discard => "discard-only",
            Class::Orchid => "ORCHID",
            Class::NotGlobal => "not global unicast",
        }
    }
}

/// The class that keeps a push from `ip`, or `None` for a public address.
pub fn refused(ip: IpAddr) -> Option<Class> {
    match ip {
        IpAddr::V4(v4) => refused_v4(v4),
        IpAddr::V6(v6) => refused_v6(v6),
    }
}

fn refused_v4(ip: Ipv4Addr) -> Option<Class> {
    let n = u32::from(ip);
    let within = |net: [u8; 4], bits: u32| {
        let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
        n & mask == u32::from(Ipv4Addr::from(net)) & mask
    };
    let class = if ip.is_broadcast() {
        Class::Broadcast
    } else if within([0, 0, 0, 0], 8) {
        Class::ThisNetwork
    } else if within([127, 0, 0, 0], 8) {
        Class::Loopback
    } else if within([10, 0, 0, 0], 8) || within([172, 16, 0, 0], 12) || within([192, 168, 0, 0], 16) {
        Class::Private
    } else if within([100, 64, 0, 0], 10) {
        Class::Shared
    } else if within([169, 254, 0, 0], 16) {
        Class::LinkLocal
    } else if within([192, 0, 0, 0], 24) {
        Class::ProtocolAssignments
    } else if within([192, 0, 2, 0], 24) || within([198, 51, 100, 0], 24) || within([203, 0, 113, 0], 24) {
        Class::Documentation
    } else if within([192, 88, 99, 0], 24) {
        Class::SixToFour
    } else if within([198, 18, 0, 0], 15) {
        Class::Benchmarking
    } else if within([224, 0, 0, 0], 4) {
        Class::Multicast
    } else if within([240, 0, 0, 0], 4) {
        Class::Reserved
    } else {
        return None;
    };
    Some(class)
}

fn refused_v6(ip: Ipv6Addr) -> Option<Class> {
    let n = u128::from(ip);
    let within = |net: [u16; 8], bits: u32| {
        let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
        let [a, b, c, d, e, f, g, h] = net;
        n & mask == u128::from(Ipv6Addr::new(a, b, c, d, e, f, g, h)) & mask
    };
    let class = if ip.is_unspecified() {
        Class::Unspecified
    } else if ip.is_loopback() {
        Class::Loopback
    } else if within([0, 0, 0, 0, 0, 0xffff, 0, 0], 96) {
        Class::Ipv4Mapped
    } else if within([0; 8], 96) {
        Class::Ipv4Compatible
    } else if within([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96) || within([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48) {
        Class::Nat64
    } else if within([0x100, 0, 0, 0, 0, 0, 0, 0], 64) {
        Class::Discard
    } else if within([0x2001, 0, 0, 0, 0, 0, 0, 0], 32) {
        Class::Teredo
    } else if within([0x2001, 0x2, 0, 0, 0, 0, 0, 0], 48) {
        Class::Benchmarking
    } else if within([0x2001, 0x10, 0, 0, 0, 0, 0, 0], 28) || within([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28) {
        Class::Orchid
    } else if within([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32) || within([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20) {
        Class::Documentation
    } else if within([0x2002, 0, 0, 0, 0, 0, 0, 0], 16) {
        Class::SixToFour
    } else if within([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7) {
        Class::UniqueLocal
    } else if within([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10) {
        Class::LinkLocal
    } else if within([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10) {
        Class::SiteLocal
    } else if within([0xff00, 0, 0, 0, 0, 0, 0, 0], 8) {
        Class::Multicast
    } else if !within([0x2000, 0, 0, 0, 0, 0, 0, 0], 3) {
        Class::NotGlobal
    } else {
        return None;
    };
    Some(class)
}

/// Whether a push may connect to `ip`: a public address, or one in `named`
/// (those `pushOrigins` names literally at the URL's port).
fn admit(ip: IpAddr, named: &BTreeSet<IpAddr>) -> Result<(), Class> {
    if named.contains(&ip) {
        return Ok(());
    }
    refused(ip).map_or(Ok(()), Err)
}

/// Why a push URL is not one this entry's pushes go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not a URL, or one without a host.
    Unparsable,
    /// Credentials in it (`https://user:pw@…`).
    Userinfo,
    /// Not `https` (and not the TCK's plain http to loopback).
    Scheme,
    /// An origin the entry's `pushOrigins` does not list.
    Origin,
    /// An address literal of a class a push never goes to.
    Address(IpAddr, Class),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unparsable => f.write_str("the push url is not a URL with a host"),
            Refusal::Userinfo => f.write_str("the push url carries credentials"),
            Refusal::Scheme => f.write_str("the push url is not https"),
            Refusal::Origin => f.write_str("the push url's origin is not in the entry's pushOrigins"),
            Refusal::Address(ip, class) => write!(f, "the push url's address {ip} is {}", class.as_str()),
        }
    }
}

/// A push URL that passed [`push_target`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    /// The URL, as the HTTP client will read it.
    pub url: Url,
    /// The addresses its host may resolve to whatever their class: those
    /// `pushOrigins` names literally at its port.
    pub named: BTreeSet<IpAddr>,
    /// Plain http to this device's loopback ([`tck_plain_http`]).
    pub plain_http: bool,
}

impl PushTarget {
    /// `scheme://host:port`, for a log line: never the path or query, which
    /// may be the webhook's capability.
    pub fn origin(&self) -> String {
        origin_of(&self.url).map_or_else(|| "<no origin>".into(), |origin| origin.to_string())
    }
}

/// The part of a URL a push is held to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl std::fmt::Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}:{}", self.scheme, self.host, self.port)
    }
}

/// A URL's origin: its host as the parser normalised it (lower case, an
/// IDN in its `xn--` form, an IPv4 address in dotted decimal, an IPv6 one in
/// brackets) and its port, the scheme's default when it names none.
fn origin_of(url: &Url) -> Option<Origin> {
    Some(Origin {
        scheme: url.scheme().to_string(),
        host: url.host_str()?.to_string(),
        port: url.port_or_known_default()?,
    })
}

/// The address a URL's host is, when it is one rather than a name. The
/// parser has already turned every spelling of an IPv4 address (`0x7f.1`,
/// `2130706433`) into dotted decimal, so a name is never one.
fn literal(url: &Url) -> Option<IpAddr> {
    let host = url.host_str()?;
    match host.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        Some(v6) => v6.parse::<Ipv6Addr>().ok().map(IpAddr::V6),
        None => host.parse::<Ipv4Addr>().ok().map(IpAddr::V4),
    }
}

fn has_userinfo(url: &Url) -> bool {
    !url.username().is_empty() || url.password().is_some()
}

/// The TCK's exception (`docs/a2a.md` §5): plain http to this device's
/// loopback, `http://127.0.0.1` or `http://localhost` at any port, which is
/// where the A2A TCK's webhook listens. Only for a `tck` entry, and only on a
/// server started with `DROPPEN_A2A_TCK=1` (`tck`); nothing else ever goes
/// out in plain http.
pub fn tck_plain_http(url: &Url, source: EntrySource, tck: bool) -> bool {
    tck && source == EntrySource::Tck && url.scheme() == "http" && !has_userinfo(url) && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
}

/// Where a push to `url` goes, for an entry whose `pushOrigins` are
/// `push_origins`, of `source`, on a server whose `DROPPEN_A2A_TCK` is `tck`;
/// or why it does not. The one rule for a push URL: the sender asks it
/// before every attempt, and a configuration may be checked with it when it
/// is made.
pub fn push_target(url: &str, push_origins: &[String], source: EntrySource, tck: bool) -> Result<PushTarget, Refusal> {
    let url = Url::parse(url).map_err(|_| Refusal::Unparsable)?;
    if has_userinfo(&url) {
        return Err(Refusal::Userinfo);
    }
    let origin = origin_of(&url).ok_or(Refusal::Unparsable)?;
    if tck_plain_http(&url, source, tck) {
        let named = [IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)].into();
        return Ok(PushTarget {
            url,
            named,
            plain_http: true,
        });
    }
    if url.scheme() != "https" {
        return Err(Refusal::Scheme);
    }
    let mut listed = false;
    let mut named = BTreeSet::new();
    for allowed in push_origins {
        let Ok(allowed) = Url::parse(allowed) else { continue };
        if has_userinfo(&allowed) {
            continue;
        }
        let Some(allowed_origin) = origin_of(&allowed) else { continue };
        listed |= allowed_origin == origin;
        if allowed_origin.scheme == "https" && allowed_origin.port == origin.port {
            named.extend(literal(&allowed));
        }
    }
    if !listed {
        return Err(Refusal::Origin);
    }
    if let Some(ip) = literal(&url) {
        admit(ip, &named).map_err(|class| Refusal::Address(ip, class))?;
    }
    Ok(PushTarget {
        url,
        named,
        plain_http: false,
    })
}

/// How a host name becomes addresses: the system's resolver, or a test's.
pub trait Lookup: Send + Sync + 'static {
    /// Every address `host` resolves to.
    fn lookup(&self, host: String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>>;
}

/// The system's resolver (`getaddrinfo`, off the runtime's threads).
pub struct SystemLookup;

impl Lookup for SystemLookup {
    fn lookup(&self, host: String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
        Box::pin(async move { Ok(tokio::net::lookup_host((host.as_str(), 0)).await?.map(|address| address.ip()).collect()) })
    }
}

/// A host name that resolved to no address a push may go to.
#[derive(Debug)]
pub struct NoAdmissibleAddress {
    /// The name.
    pub host: String,
    /// What it resolved to, and why each was refused.
    pub refused: Vec<(IpAddr, Class)>,
}

impl std::fmt::Display for NoAdmissibleAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} resolves to no address a push may go to", self.host)?;
        for (index, (ip, class)) in self.refused.iter().enumerate() {
            write!(f, "{} {ip} ({})", if index == 0 { ":" } else { "," }, class.as_str())?;
        }
        Ok(())
    }
}

impl std::error::Error for NoAdmissibleAddress {}

/// The push client's resolver: [`Lookup`], with every address held to
/// [`refused`] (but those `named`), and only the ones that pass handed to the
/// connector, which connects to nothing else.
pub(crate) struct Guarded {
    lookup: Arc<dyn Lookup>,
    named: BTreeSet<IpAddr>,
}

impl Guarded {
    pub(crate) fn new(lookup: Arc<dyn Lookup>, named: BTreeSet<IpAddr>) -> Self {
        Self { lookup, named }
    }

    /// What `host` may be connected at: its admissible addresses, or why it
    /// has none.
    pub(crate) async fn admissible(&self, host: String) -> Result<Vec<IpAddr>, Box<dyn std::error::Error + Send + Sync>> {
        let found = self.lookup.lookup(host.clone()).await?;
        let mut kept = Vec::new();
        let mut refused = Vec::new();
        for ip in found {
            match admit(ip, &self.named) {
                Ok(()) => kept.push(ip),
                Err(class) => refused.push((ip, class)),
            }
        }
        if kept.is_empty() {
            return Err(Box::new(NoAdmissibleAddress { host, refused }));
        }
        Ok(kept)
    }
}

impl reqwest::dns::Resolve for Guarded {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let guarded = Guarded {
            lookup: self.lookup.clone(),
            named: self.named.clone(),
        };
        let host = name.as_str().to_string();
        Box::pin(async move {
            let kept = guarded.admissible(host).await?;
            // Port 0: the connector puts the URL's port in.
            let addresses: reqwest::dns::Addrs = Box::new(kept.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addresses)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn every_refused_class_is_refused_and_public_addresses_are_not() {
        let cases = [
            ("0.1.2.3", Class::ThisNetwork),
            ("0.0.0.0", Class::ThisNetwork),
            ("127.0.0.1", Class::Loopback),
            ("127.255.0.9", Class::Loopback),
            ("10.1.2.3", Class::Private),
            ("172.16.0.1", Class::Private),
            ("172.31.255.255", Class::Private),
            ("192.168.1.10", Class::Private),
            ("100.64.0.1", Class::Shared),
            ("100.100.100.100", Class::Shared),
            ("100.127.255.254", Class::Shared),
            ("169.254.169.254", Class::LinkLocal),
            ("192.0.0.170", Class::ProtocolAssignments),
            ("192.0.2.1", Class::Documentation),
            ("198.51.100.7", Class::Documentation),
            ("203.0.113.9", Class::Documentation),
            ("198.18.0.1", Class::Benchmarking),
            ("198.19.255.1", Class::Benchmarking),
            ("192.88.99.1", Class::SixToFour),
            ("224.0.0.1", Class::Multicast),
            ("239.255.255.250", Class::Multicast),
            ("240.0.0.1", Class::Reserved),
            ("255.255.255.255", Class::Broadcast),
            ("::", Class::Unspecified),
            ("::1", Class::Loopback),
            ("::ffff:127.0.0.1", Class::Ipv4Mapped),
            ("::ffff:8.8.8.8", Class::Ipv4Mapped),
            ("::10.0.0.1", Class::Ipv4Compatible),
            ("::8.8.8.8", Class::Ipv4Compatible),
            ("64:ff9b::a00:1", Class::Nat64),
            ("64:ff9b::808:808", Class::Nat64),
            ("64:ff9b:1::1", Class::Nat64),
            ("100::1", Class::Discard),
            ("2001::1", Class::Teredo),
            ("2001:0:4136:e378:8000:63bf:3fff:fdd2", Class::Teredo),
            ("2001:2::1", Class::Benchmarking),
            ("2001:10::1", Class::Orchid),
            ("2001:20::1", Class::Orchid),
            ("2001:db8::1", Class::Documentation),
            ("3fff::1", Class::Documentation),
            ("2002:a00:1::1", Class::SixToFour),
            ("2002:808:808::1", Class::SixToFour),
            ("fc00::1", Class::UniqueLocal),
            ("fd00:ec2::254", Class::UniqueLocal),
            ("fe80::1", Class::LinkLocal),
            ("fec0::1", Class::SiteLocal),
            ("ff02::1", Class::Multicast),
            ("4000::1", Class::NotGlobal),
            ("::ffff:0:a00:1", Class::NotGlobal),
        ];
        for (address, class) in cases {
            assert_eq!(refused(ip(address)), Some(class), "{address}");
        }
        for public in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "100.128.0.1",
            "172.32.0.1",
            "2606:4700:4700::1111",
            "2a00:1450:4001::1",
        ] {
            assert_eq!(refused(ip(public)), None, "{public} is public");
        }
    }

    fn origins(list: &[&str]) -> Vec<String> {
        list.iter().map(|origin| origin.to_string()).collect()
    }

    fn target(url: &str, list: &[&str]) -> Result<PushTarget, Refusal> {
        push_target(url, &origins(list), EntrySource::Claude, false)
    }

    #[test]
    fn an_origin_matches_exactly_by_scheme_host_and_port() {
        let list = ["https://hooks.example.com"];
        assert!(target("https://hooks.example.com/a2a?x=1", &list).is_ok());
        assert!(target("https://HOOKS.Example.com:443/a2a", &list).is_ok(), "case and the default port");
        assert_eq!(target("https://hooks.example.com:8443/a2a", &list), Err(Refusal::Origin), "another port");
        assert_eq!(target("http://hooks.example.com/a2a", &list), Err(Refusal::Scheme));
        assert_eq!(target("wss://hooks.example.com/a2a", &list), Err(Refusal::Scheme));
        assert_eq!(target("https://hooks.example.com.evil.test/", &list), Err(Refusal::Origin));
        assert_eq!(target("https://evil.test#@hooks.example.com", &list), Err(Refusal::Origin));
        assert_eq!(target("https://evil.test?@hooks.example.com", &list), Err(Refusal::Origin));
        assert_eq!(
            target("https://hooks.example.com./", &list),
            Err(Refusal::Origin),
            "a trailing dot is another host"
        );
        assert_eq!(target("not a url", &list), Err(Refusal::Unparsable));

        let on_8443 = ["https://hooks.example.com:8443"];
        assert_eq!(
            target("https://hooks.example.com/a2a", &on_8443),
            Err(Refusal::Origin),
            "the default port is not 8443"
        );
        assert!(target("https://hooks.example.com:8443/a2a", &on_8443).is_ok());
        assert_eq!(
            target("https://hooks.example.com:443/", &["https://hooks.example.com:443"]).map(|t| t.origin()),
            Ok("https://hooks.example.com:443".into())
        );
    }

    #[test]
    fn credentials_in_the_url_are_refused() {
        let list = ["https://hooks.example.com"];
        for url in [
            "https://user@hooks.example.com/",
            "https://user:pw@hooks.example.com/",
            "https://:pw@hooks.example.com/",
            "https://evil.test@hooks.example.com/",
        ] {
            assert_eq!(target(url, &list), Err(Refusal::Userinfo), "{url}");
        }
        assert_eq!(
            target("https://hooks.example.com/", &["https://user@hooks.example.com"]),
            Err(Refusal::Origin),
            "nor does an origin with credentials list anything"
        );
    }

    #[test]
    fn an_idn_matches_in_either_spelling_and_a_lookalike_does_not() {
        assert!(target("https://Bücher.example/hook", &["https://xn--bcher-kva.example"]).is_ok());
        assert!(target("https://xn--bcher-kva.example/hook", &["https://bücher.example"]).is_ok());
        assert!(target("https://BÜCHER.example:443/hook", &["https://bücher.example"]).is_ok());
        assert_eq!(target("https://bucher.example/hook", &["https://bücher.example"]), Err(Refusal::Origin));
        assert_eq!(
            target("https://xn--bcher-kva.example/", &["https://bücher.example"]).unwrap().origin(),
            "https://xn--bcher-kva.example:443"
        );
    }

    #[test]
    fn an_address_literal_is_held_to_the_classes_unless_named() {
        // Named literally: the exception.
        let named = target("https://127.0.0.1:8443/hook", &["https://127.0.0.1:8443"]).unwrap();
        assert_eq!(named.named, [ip("127.0.0.1")].into());
        // Every spelling the parser reads as that address is that address.
        assert!(target("https://0x7f.0.0.1:8443/hook", &["https://127.0.0.1:8443"]).is_ok());
        assert!(target("https://2130706433:8443/hook", &["https://127.0.0.1:8443"]).is_ok());
        // Another port is another origin.
        assert_eq!(target("https://127.0.0.1/hook", &["https://127.0.0.1:8443"]), Err(Refusal::Origin));
        assert!(target("https://[::1]:8443/x", &["https://[::1]:8443"]).is_ok());
        // A public literal needs no naming beyond the origin.
        assert!(target("https://93.184.216.34/x", &["https://93.184.216.34"]).is_ok());
    }

    #[test]
    fn a_name_may_resolve_to_an_address_named_at_its_port_only() {
        let lan = target("https://hooks.lan/x", &["https://hooks.lan", "https://192.168.1.10"]).unwrap();
        assert_eq!(lan.named, [ip("192.168.1.10")].into());
        let other_port = target("https://hooks.lan/x", &["https://hooks.lan", "https://192.168.1.10:8443"]).unwrap();
        assert!(other_port.named.is_empty());
        let plain = target("https://hooks.example.com/x", &["https://hooks.example.com"]).unwrap();
        assert!(plain.named.is_empty() && !plain.plain_http);
    }

    #[test]
    fn plain_http_to_loopback_is_the_tcks_alone() {
        let none: Vec<String> = Vec::new();
        for url in [
            "http://127.0.0.1:41234/webhook",
            "http://localhost:41234/webhook",
            "http://LOCALHOST:41234/webhook",
        ] {
            let target = push_target(url, &none, EntrySource::Tck, true).unwrap();
            assert!(target.plain_http, "{url}");
            assert_eq!(target.named, [ip("127.0.0.1"), ip("::1")].into());
            assert_eq!(
                push_target(url, &none, EntrySource::Tck, false),
                Err(Refusal::Scheme),
                "{url} without the switch"
            );
            assert_eq!(
                push_target(url, &none, EntrySource::Claude, true),
                Err(Refusal::Scheme),
                "{url} for another entry"
            );
            assert_eq!(push_target(url, &none, EntrySource::Codex, false), Err(Refusal::Scheme));
        }
        for url in [
            "http://10.0.0.1:41234/webhook",
            "http://example.com/webhook",
            "http://[::1]:41234/webhook",
            "http://127.0.0.2:41234/webhook",
            "http://localhost.evil.test/webhook",
        ] {
            assert_eq!(push_target(url, &none, EntrySource::Tck, true), Err(Refusal::Scheme), "{url}");
        }
        assert_eq!(
            push_target("http://u:p@localhost:1/webhook", &none, EntrySource::Tck, true),
            Err(Refusal::Userinfo)
        );
        // A tck entry's https pushes follow the ordinary rule.
        assert!(push_target(
            "https://hooks.example.com/",
            &origins(&["https://hooks.example.com"]),
            EntrySource::Tck,
            true
        )
        .is_ok());
    }

    struct Answers(std::sync::Mutex<std::collections::VecDeque<Vec<IpAddr>>>);

    impl Lookup for Answers {
        fn lookup(&self, _host: String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
            let mut answers = self.0.lock().unwrap();
            let answer = if answers.len() > 1 {
                answers.pop_front()
            } else {
                answers.front().cloned()
            };
            Box::pin(async move { Ok(answer.unwrap_or_default()) })
        }
    }

    fn guarded(answers: &[&[&str]], named: &[&str]) -> Guarded {
        let answers = answers.iter().map(|answer| answer.iter().map(|a| ip(a)).collect()).collect();
        Guarded::new(Arc::new(Answers(std::sync::Mutex::new(answers))), named.iter().map(|a| ip(a)).collect())
    }

    #[tokio::test]
    async fn the_resolver_hands_on_only_what_passes_and_checks_every_lookup() {
        let private = guarded(&[&["10.0.0.7"]], &[]);
        let error = private.admissible("hooks.test".into()).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "hooks.test resolves to no address a push may go to: 10.0.0.7 (private)"
        );

        let mixed = guarded(&[&["10.0.0.7", "93.184.216.34", "::1"]], &[]);
        assert_eq!(mixed.admissible("hooks.test".into()).await.unwrap(), [ip("93.184.216.34")]);

        let named = guarded(&[&["192.168.1.10"]], &["192.168.1.10"]);
        assert_eq!(named.admissible("hooks.lan".into()).await.unwrap(), [ip("192.168.1.10")]);

        // Rebinding: public on the first lookup, private on the next. Each
        // connection's lookup is checked, so the second finds nothing.
        let rebinding = guarded(&[&["93.184.216.34"], &["127.0.0.1"]], &[]);
        assert_eq!(rebinding.admissible("hooks.test".into()).await.unwrap(), [ip("93.184.216.34")]);
        assert!(rebinding.admissible("hooks.test".into()).await.is_err());

        // Through reqwest's own seam.
        use reqwest::dns::Resolve as _;
        let resolved: Vec<SocketAddr> = guarded(&[&["93.184.216.34"]], &[])
            .resolve("hooks.test".parse().unwrap())
            .await
            .unwrap()
            .collect();
        assert_eq!(resolved, [SocketAddr::new(ip("93.184.216.34"), 0)]);
        assert!(guarded(&[&["169.254.169.254"]], &[])
            .resolve("hooks.test".parse().unwrap())
            .await
            .is_err());
    }
}
