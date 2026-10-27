//! The same-origin guard and CORS answer on `/api/*`.
//!
//! The fs endpoints can write into `$HOME` and read files back, so a
//! cross-origin page must never be able to reach them: a browser request whose
//! `Origin` is not trusted is refused outright, and only a trusted one is
//! echoed back in `Access-Control-Allow-Origin`. Tools that send no `Origin` at
//! all (curl, the integration tests) are unaffected.
//!
//! Same-host origins are trusted, for any name the server answers to (below);
//! extra proxy origins come from `DROPPEN_ALLOWED_ORIGINS`. Nothing else is:
//! a development origin is another site's page as far as the browser is
//! concerned (`pnpm dev` proxies the API, so it needs no exemption).
//!
//! # Which names the server answers to
//!
//! A same-origin request says nothing about where it was made — a page's own
//! `GET` carries no `Origin`, and `Sec-Fetch-Site` says `same-origin` — so a
//! page served under some other host name that resolves to this device would
//! pass both checks above. [`host_layer`] therefore looks at the name the
//! request was addressed to (`Host`, and the request target's authority when it
//! has one), on every path, before anything else runs. It accepts:
//!
//! * an IP address (`127.0.0.1:8700`, `[::1]:8700`, `192.168.1.20:8700`): the
//!   desktop app's window, the local page, the CLI, `droppen-notify` and the
//!   Droppen bridge all address the server this way;
//! * `localhost` and `*.localhost`, which browsers keep on this device;
//! * names only a local network gives out: a single label (`mymac`) and
//!   `*.local`, `*.lan`, `*.home.arpa`, `*.internal`, and a tailnet's
//!   `*.ts.net` (the web app's `privateHost` in
//!   `web/src/features/link/handoff.ts` takes the same suffixes);
//! * the name the server is configured to bind (`APP_SERVER__HOST`), the hosts
//!   of `DROPPEN_ALLOWED_ORIGINS`, and the names in `DROPPEN_ALLOWED_HOSTS`
//!   (comma-separated; a port is ignored). A reverse proxy or tunnel that
//!   passes another public name on (Caddy, Traefik, a Cloudflare Tunnel) needs
//!   that name here. `DROPPEN_ALLOWED_HOSTS=*` accepts any name.
//!
//! A request with no `Host` at all (an HTTP/1.0 tool) passes; a browser always
//! sends one.
//!
//! What this leaves: the local-network names are whatever the network's own
//! name resolution says they are, so a device on the same network that
//! controls that resolution can still reach the routes that answer without a
//! session (sign-in, first-run setup, status); everything else needs a
//! session ([`crate::auth`]).
//!
//! Before the `Origin` is looked at, a browser's own word on where a request
//! came from is: `Sec-Fetch-Site: cross-site` — another site's page, an
//! `<img src>`, a form, a WebSocket from somebody else's tab — is refused on
//! `/api/*` whatever `Origin` says or does not say, and so is `same-site`,
//! which on a server that is a single origin means another port or another
//! subdomain of this one (`http://localhost:5173` reaching `localhost:8700`).
//! The page itself sends `same-origin`, a typed URL `none`, and tools nothing
//! at all. This is what keeps a request that carries no `Origin` — a simple
//! `GET` — from being somebody else's page acting with this device's
//! permissions, which matters most where this device's own browser needs no
//! login ([`crate::auth`]). WebSocket handshakes are `GET`s under `/api/` and
//! pass through both checks like every other request.
//!
//! It is a layer over the whole router rather than a route-scoped one, because
//! gotcha only exposes `GotchaRouter::layer` (`gotcha-0.4.1/src/router.rs:207`)
//! and nesting would rewrite every documented path. The `/api/` prefix test is
//! therefore made explicitly, in the first line of the guard.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use gotcha::axum::body::Body;
use gotcha::axum::extract::{Request, State};
use gotcha::axum::http::{header, HeaderValue, Method, StatusCode};
use gotcha::axum::middleware::{FromFnLayer, Next};
use gotcha::axum::response::{IntoResponse, Response};

use crate::api::error::ApiError;

/// The methods the CORS preflight advertises: every method the API answers.
const ALLOW_METHODS: &str = "GET,HEAD,PUT,POST,DELETE,PATCH";

/// Extra trusted origins, comma-separated, from `DROPPEN_ALLOWED_ORIGINS`.
/// Its old name, `RH_ALLOWED_ORIGINS`, is still read until v0.2.135
/// ([`crate::env`]).
fn configured_origins() -> &'static [String] {
    static ORIGINS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    ORIGINS.get_or_init(|| {
        crate::env::var("DROPPEN_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

/// The `host` of an origin, as a browser's `URL.host` computes it: lowercase,
/// without userinfo, and without a port that is the scheme's default.
///
/// `None` means the string is not a URL at all, so it can never match a host.
fn origin_host(origin: &str) -> Option<String> {
    let (scheme, rest) = origin.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map(|(_, host)| host).unwrap_or(authority);
    if authority.is_empty() {
        return None;
    }
    let host = authority.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" | "ws" => ":80",
        "https" | "wss" => ":443",
        _ => return Some(host),
    };
    Some(host.strip_suffix(default_port).map(str::to_owned).unwrap_or(host))
}

/// Whether a browser sending this `Origin` to this `Host` is trusted.
pub fn origin_allowed(origin: &str, request_host: Option<&str>) -> bool {
    if configured_origins().iter().any(|o| o == origin) {
        return true;
    }
    match (origin_host(origin), request_host) {
        (Some(host), Some(request_host)) => host == request_host,
        _ => false,
    }
}

/// What a `Host` value names: an IP address, or a host name (lowercase,
/// without a trailing dot).
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostName {
    Ip,
    Name(String),
}

/// The name in a `Host` value (or an authority, or a `DROPPEN_ALLOWED_HOSTS`
/// entry), without its port. `None` when it is not one: an empty value, a bad
/// port, a bare IPv6 address, a character no host name has.
fn host_name(value: &str) -> Option<HostName> {
    let value = value.trim();
    let port_ok = |port: &str| !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = value.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        inside.parse::<Ipv6Addr>().ok()?;
        return match after {
            "" => Some(HostName::Ip),
            _ => after.strip_prefix(':').filter(|port| port_ok(port)).map(|_| HostName::Ip),
        };
    }
    let name = match value.split_once(':') {
        Some((name, port)) if port_ok(port) => name,
        Some(_) => return None,
        None => value,
    };
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    if name.parse::<Ipv4Addr>().is_ok() {
        return Some(HostName::Ip);
    }
    let label_ok = |label: &str| {
        !label.is_empty() && label.len() <= 63 && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    (name.len() <= 253 && name.split('.').all(label_ok)).then_some(HostName::Name(name))
}

/// The suffixes of names only this device or its local network gives out:
/// browsers keep `localhost` on this device, mDNS answers `.local` on the local
/// network only, `.lan`, `.home.arpa` and `.internal` are what home and office
/// networks name their devices under, never names public DNS hands out, and
/// `.ts.net` names are a tailnet's own (Tailscale's MagicDNS and `tailscale
/// serve`). The web app's `privateHost` (`web/src/features/link/handoff.ts`)
/// takes the same ones.
const LOCAL_SUFFIXES: [&str; 6] = ["localhost", "local", "lan", "home.arpa", "internal", "ts.net"];

/// Whether `name` is one only this device or its local network resolves: a
/// single label (`mymac`, reached through the network's own DNS) or a name
/// under one of [`LOCAL_SUFFIXES`] (not the suffix itself).
fn local_name(name: &str) -> bool {
    !name.contains('.')
        || LOCAL_SUFFIXES
            .iter()
            .any(|suffix| name.strip_suffix(suffix).is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')))
}

/// The host names this server answers to besides IP addresses and local names
/// (see the module documentation).
#[derive(Debug, Clone, Default)]
pub struct HostPolicy {
    /// `DROPPEN_ALLOWED_HOSTS=*`: any name at all.
    any: bool,
    names: Vec<String>,
}

impl HostPolicy {
    /// From `DROPPEN_ALLOWED_HOSTS`'s value, the trusted origins
    /// (`DROPPEN_ALLOWED_ORIGINS`) and the host the server binds.
    pub fn new(allowed_hosts: &str, allowed_origins: &[String], bind_host: &str) -> Self {
        let mut policy = HostPolicy::default();
        let add = |policy: &mut HostPolicy, value: &str| {
            if let Some(HostName::Name(name)) = host_name(value) {
                if !policy.names.contains(&name) {
                    policy.names.push(name);
                }
            }
        };
        for entry in allowed_hosts.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            if entry == "*" {
                policy.any = true;
            } else if host_name(entry).is_none() {
                tracing::warn!(entry, "DROPPEN_ALLOWED_HOSTS: not a host name; ignored");
            } else {
                add(&mut policy, entry);
            }
        }
        for origin in allowed_origins {
            if let Some(host) = origin_host(origin) {
                add(&mut policy, &host);
            }
        }
        add(&mut policy, bind_host);
        policy
    }

    /// [`HostPolicy::new`] from this process's environment.
    pub fn from_env(bind_host: &str) -> Self {
        Self::new(&crate::env::var("DROPPEN_ALLOWED_HOSTS").unwrap_or_default(), configured_origins(), bind_host)
    }

    /// Whether a request addressed to `host` (a `Host` value) is answered.
    pub fn allows(&self, host: &str) -> bool {
        if self.any {
            return true;
        }
        match host_name(host) {
            Some(HostName::Ip) => true,
            Some(HostName::Name(name)) => local_name(&name) || self.names.contains(&name),
            None => false,
        }
    }
}

/// The already-boxed future [`host_layer`] hands to axum.
type HostFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;
/// A plain function pointer, so the layer's type can be written down
/// ([`layer`]'s trick).
type HostFn = fn(State<Arc<HostPolicy>>, Request, Next) -> HostFuture;

/// [`host_guard`] as a router layer, for the whole router (`crate::app`).
pub fn host_layer(policy: Arc<HostPolicy>) -> FromFnLayer<HostFn, Arc<HostPolicy>, (State<Arc<HostPolicy>>, Request)> {
    fn boxed(State(policy): State<Arc<HostPolicy>>, request: Request, next: Next) -> HostFuture {
        Box::pin(host_guard(policy, request, next))
    }
    gotcha::axum::middleware::from_fn_with_state(policy, boxed as HostFn)
}

/// Refuse a request addressed to a name this server does not answer to, on
/// every path, saying which setting would let it through.
pub async fn host_guard(policy: Arc<HostPolicy>, request: Request, next: Next) -> Response {
    let named = request.headers().get(header::HOST).map(|value| value.to_str().unwrap_or(""));
    let authority = request.uri().authority().map(|authority| authority.as_str());
    if let Some(refused) = named.into_iter().chain(authority).find(|host| !policy.allows(host)) {
        let said = match host_name(refused) {
            Some(HostName::Name(name)) => format!(
                "this server does not answer to the name {name}: add it to DROPPEN_ALLOWED_HOSTS (comma-separated) to reach it by that name"
            ),
            _ => "bad Host header".to_string(),
        };
        tracing::debug!(host = refused, "refused a request addressed to another host name");
        if request.uri().path().starts_with("/api/") {
            return ApiError::new(StatusCode::FORBIDDEN, said).into_response();
        }
        let mut response = Response::new(Body::from(format!("{said}\n")));
        *response.status_mut() = StatusCode::FORBIDDEN;
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
        headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return response;
    }
    next.run(request).await
}

/// The already-boxed future [`layer`] hands to axum.
type GuardFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;
/// A plain function pointer, so the layer's type can be written down.
type GuardFn = fn(Request, Next) -> GuardFuture;

/// [`guard`] as a router layer.
///
/// `from_fn` leaves its extractor-tuple parameter to be inferred from the
/// `Service` bound at the call site, which rustc will not do backwards through
/// the impl (`axum-0.8.9/src/middleware/from_fn.rs:259`). Naming the return type
/// here pins it once, and keeps the registration in `api::routes` to one line.
pub fn layer() -> gotcha::axum::middleware::FromFnLayer<GuardFn, (), (Request,)> {
    fn boxed(request: Request, next: Next) -> GuardFuture {
        Box::pin(guard(request, next))
    }
    gotcha::axum::middleware::from_fn(boxed as GuardFn)
}

/// Refuse untrusted origins on `/api/*`, and answer the preflight for trusted ones.
pub async fn guard(request: Request, next: Next) -> Response {
    if !request.uri().path().starts_with("/api/") {
        return next.run(request).await;
    }

    if let Some(site) = request.headers().get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        if foreign_fetch(site) {
            return ApiError::new(StatusCode::FORBIDDEN, "cross-site request").into_response();
        }
    }
    // Read everything out of the request first: a borrow of it (the request body
    // is `Send` but not `Sync`) may not be held across the `next.run` await.
    let (origin, host, requested_headers) = {
        let text = |name: header::HeaderName| request.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        (text(header::ORIGIN), text(header::HOST), text(header::ACCESS_CONTROL_REQUEST_HEADERS))
    };
    let preflight_request = request.method() == Method::OPTIONS;

    let allowed = origin.as_deref().map(|origin| origin_allowed(origin, host.as_deref())).unwrap_or(true);
    if !allowed {
        return ApiError::new(StatusCode::FORBIDDEN, "forbidden origin").into_response();
    }

    // hono's cors() answers every OPTIONS itself, before routing, so a
    // preflight never depends on the route existing.
    if preflight_request {
        return preflight(origin.as_deref(), requested_headers.as_deref());
    }

    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if let Some(value) = origin.as_deref().and_then(|o| HeaderValue::from_str(o).ok()) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    // No other site's page may embed an answer of this API as an image, a
    // script or a frame — which is how a cross-site `<img src>` would learn a
    // private file exists, and how big it is, without reading it.
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    response
}

/// Whether a browser says this request was started by another origin's page:
/// `cross-site`, or `same-site` (this site, another port or subdomain).
/// `same-origin`, `none` (typed, bookmarked) and anything unknown pass.
fn foreign_fetch(sec_fetch_site: &str) -> bool {
    let site = sec_fetch_site.trim();
    site.eq_ignore_ascii_case("cross-site") || site.eq_ignore_ascii_case("same-site")
}

/// The preflight answer: `204`, no body, and the request's own header list
/// echoed back, because the browser asks whether it may send those headers.
fn preflight(origin: Option<&str>, request_headers: Option<&str>) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    let headers = response.headers_mut();
    if let Some(value) = origin.and_then(|o| HeaderValue::from_str(o).ok()) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static(ALLOW_METHODS));
    if let Some(value) = request_headers.filter(|h| !h.is_empty()).and_then(|h| HeaderValue::from_str(h).ok()) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
        headers.append(header::VARY, HeaderValue::from_static("Access-Control-Request-Headers"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_same_host_origin_is_trusted_whatever_the_domain() {
        assert!(origin_allowed("https://remote.example.com", Some("remote.example.com")));
        assert!(origin_allowed("http://remote.example.com:8700", Some("remote.example.com:8700")));
        assert!(!origin_allowed("https://evil.example.com", Some("remote.example.com")));
    }

    /// A development origin is not trusted for a `Host` it does not name: a
    /// page at `localhost:5180` is another site to a server at
    /// `127.0.0.1:8700`, and `pnpm dev` proxies the API anyway.
    #[test]
    fn no_origin_is_trusted_for_a_host_it_does_not_name() {
        assert!(!origin_allowed("http://localhost:5180", None));
        assert!(!origin_allowed("http://127.0.0.1:8700", Some("elsewhere")));
        assert!(origin_allowed("http://127.0.0.1:8700", Some("127.0.0.1:8700")));
    }

    #[test]
    fn only_another_origins_fetch_is_foreign() {
        assert!(foreign_fetch("cross-site"));
        assert!(foreign_fetch("Cross-Site"));
        assert!(foreign_fetch("same-site"), "another port on this host, or another subdomain");
        assert!(!foreign_fetch("same-origin"));
        assert!(!foreign_fetch("none"), "typed or bookmarked");
        assert!(!foreign_fetch("something-new"));
    }

    #[test]
    fn a_non_url_origin_is_never_trusted() {
        // `new URL("null")` throws, which is what the browser sends for a
        // sandboxed or file:// document.
        assert!(!origin_allowed("null", Some("null")));
        assert!(!origin_allowed("", Some("")));
    }

    fn policy(allowed_hosts: &str, allowed_origins: &[&str], bind_host: &str) -> HostPolicy {
        let origins: Vec<String> = allowed_origins.iter().map(|o| o.to_string()).collect();
        HostPolicy::new(allowed_hosts, &origins, bind_host)
    }

    #[test]
    fn ip_addresses_are_always_answered() {
        let p = policy("", &[], "127.0.0.1");
        for host in ["127.0.0.1:8700", "127.0.0.1", "[::1]:8700", "[::1]", "192.168.1.20:8700", "100.64.0.7:8700", "127.0.0.1.:8700"] {
            assert!(p.allows(host), "{host}");
        }
    }

    #[test]
    fn localhost_and_local_network_names_are_answered() {
        let p = policy("", &[], "127.0.0.1");
        for host in [
            "localhost:8700",
            "LOCALHOST:8700",
            "localhost.:8700",
            "app.localhost:8700",
            "mymac:8700",
            "mymac.local:8700",
            "nas.lan",
            "box.home.arpa:8700",
            "dev.internal:8700",
            "mymac.tail1234.ts.net:8700",
            "MyMac.Tail1234.TS.NET.",
        ] {
            assert!(p.allows(host), "{host}");
        }
        for host in ["ts.net", "ts.net:8700", "evilts.net", "x.ts.net.example.com", "home.arpa", ".local"] {
            assert!(!p.allows(host), "{host}: a suffix alone, or one that is not at the end, is not a local name");
        }
    }

    #[test]
    fn a_public_name_is_refused_unless_configured() {
        let p = policy("", &[], "127.0.0.1");
        for host in ["remote.example.com", "remote.example.com:8700", "127.0.0.1.example.com:8700", "localhost.example.com:8700", "local.example.com"] {
            assert!(!p.allows(host), "{host}");
        }
        let p = policy(" Remote.Example.com:443 , other.example.org ", &[], "127.0.0.1");
        assert!(p.allows("remote.example.com"), "a configured name, its port and case aside");
        assert!(p.allows("REMOTE.example.com.:8443"));
        assert!(p.allows("other.example.org:8700"));
        assert!(!p.allows("sub.remote.example.com"), "a name, not its subdomains");
    }

    #[test]
    fn the_trusted_origins_and_the_bound_name_are_answered() {
        let p = policy("", &["https://proxy.example.com", "http://dev.example.net:5180"], "mymac.example.org");
        assert!(p.allows("proxy.example.com"));
        assert!(p.allows("dev.example.net:8700"));
        assert!(p.allows("mymac.example.org:8700"));
        assert!(!p.allows("elsewhere.example.com"));
    }

    #[test]
    fn a_star_answers_any_name() {
        let p = policy("*", &[], "127.0.0.1");
        assert!(p.allows("remote.example.com:8700"));
        assert!(p.allows("anything.example"));
    }

    #[test]
    fn a_malformed_host_is_refused() {
        let p = policy("", &[], "127.0.0.1");
        for host in ["", ":8700", "localhost:", "localhost:http", "localhost:123456", "::1", "[::1", "[::1]x", "[nope]:80", "a b", "user@localhost", "a..b", "localhost/x"] {
            assert!(!p.allows(host), "{host:?}");
        }
        assert!(policy("not a name, ok.example", &[], "127.0.0.1").allows("ok.example"), "a bad entry is skipped, the rest kept");
    }

    #[test]
    fn a_host_names_an_ip_or_a_lowercase_name() {
        assert_eq!(host_name("127.0.0.1:8700"), Some(HostName::Ip));
        assert_eq!(host_name("[::1]:8700"), Some(HostName::Ip));
        assert_eq!(host_name("[fe80::1%25en0]:8700"), None, "a zone is no Host a browser sends");
        assert_eq!(host_name("MyMac.Local.:8700"), Some(HostName::Name("mymac.local".into())));
        assert!(local_name("x.local") && !local_name("xlocal.example") && !local_name("x.localx"));
    }

    #[test]
    fn a_default_port_is_not_part_of_the_host() {
        assert_eq!(origin_host("http://example.com:80").as_deref(), Some("example.com"));
        assert_eq!(origin_host("https://example.com:443").as_deref(), Some("example.com"));
        assert_eq!(origin_host("http://EXAMPLE.com:8080").as_deref(), Some("example.com:8080"));
        assert_eq!(origin_host("http://user:pw@example.com/x").as_deref(), Some("example.com"));
    }
}
