//! The same-origin guard and CORS answer on `/api/*`.
//!
//! Port of the two `app.use("/api/*", ...)` middlewares in the former
//! TypeScript server's `app.ts` plus the `cors()` they wrap
//! (`hono/dist/middleware/cors/index.js`). The fs endpoints can write into
//! `$HOME` and read files back, so a cross-origin page must never be able to
//! reach them: a browser request whose `Origin` is not trusted is refused
//! outright, and only a trusted one is echoed back in
//! `Access-Control-Allow-Origin`. Tools that send no `Origin` at all (curl,
//! the integration tests) are unaffected, exactly as in the TypeScript.
//!
//! Same-host origins are always trusted so any deployment domain works; extra
//! proxy origins come from `DROPPEN_ALLOWED_ORIGINS`. Nothing else is: the
//! development origins this once trusted for any `Host` were another site's
//! page as far as the browser was concerned (`pnpm dev` proxies the API, so it
//! never needed them).
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
//! and nesting would rewrite every documented path. The `/api/` prefix test the
//! TypeScript gets from its route pattern is therefore made explicitly, in the
//! first line of the guard.

use gotcha::axum::body::Body;
use gotcha::axum::extract::Request;
use gotcha::axum::http::{header, HeaderValue, Method, StatusCode};
use gotcha::axum::middleware::Next;
use gotcha::axum::response::{IntoResponse, Response};

use crate::api::error::ApiError;

/// The methods the CORS preflight advertises — hono's defaults, which the
/// TypeScript does not override.
const ALLOW_METHODS: &str = "GET,HEAD,PUT,POST,DELETE,PATCH";

/// Extra trusted origins, comma-separated, from `DROPPEN_ALLOWED_ORIGINS`
/// (before the rename, `RH_ALLOWED_ORIGINS`, still read).
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

/// The `host` of an origin, as `new URL(origin).host` computes it: lowercase,
/// without userinfo, and without a port that is the scheme's default.
///
/// `None` means the string is not a URL at all, which is what makes the
/// TypeScript's `try { new URL(origin) } catch { return false }` reject it.
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

/// hono's preflight answer: `204`, no body, and the request's own header list
/// echoed back (its `allowHeaders` option is left empty by the TypeScript).
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

    /// The development origins once trusted for any `Host` are not trusted for
    /// any `Host` they do not name: a page at `localhost:5180` is another site
    /// to a server at `127.0.0.1:8700`, and `pnpm dev` proxies the API anyway.
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

    #[test]
    fn a_default_port_is_not_part_of_the_host() {
        assert_eq!(origin_host("http://example.com:80").as_deref(), Some("example.com"));
        assert_eq!(origin_host("https://example.com:443").as_deref(), Some("example.com"));
        assert_eq!(origin_host("http://EXAMPLE.com:8080").as_deref(), Some("example.com:8080"));
        assert_eq!(origin_host("http://user:pw@example.com/x").as_deref(), Some("example.com"));
    }
}
