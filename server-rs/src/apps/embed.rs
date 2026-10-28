//! The `/embed/{id}/*` reverse proxy, for HTTP and for websockets.
//!
//! The proxy is the router's fallback, composed in [`crate::app`] outside the
//! gotcha app, so nothing about it is in the OpenAPI document.
//!
//! # Where it hangs
//!
//! gotcha offers two seams — [`fallback_service`](gotcha::GotchaRouter::fallback_service)
//! and [`layer`](gotcha::GotchaRouter::layer) — and the SPA already owns the
//! fallback (see [`crate::static_files`]). It also assembles routes in
//! `GotchaApp::routes`, which runs *before* the state exists and so cannot be
//! handed the [`Registry`]. So the composition happens one level up, in
//! [`crate::app`], on the axum router `GotchaApp::build_router` returns: this
//! module's service takes the fallback, and delegates every path that is not an
//! embed to the SPA service it wraps. A layer would work too, but would put a
//! path test in front of every `/api/*` request to serve a route that
//! by definition never matches one.
//!
//! Sitting on the fallback also leaves `/embed/*` outside the `/api/*` origin
//! guard; what guards it is the unguessable id ([`crate::auth`], "Who has to
//! log in").
//!
//! # What it does to the traffic
//!
//! * **path** — `^/embed/([\w-]+)(/.*)?$` over the full request target,
//!   query string included, so `/embed/x/y?z` proxies to `/y?z` and
//!   `/embed/x?z` matches nothing; group 2 (or `/`) becomes the upstream path.
//! * **headers** — forwarded verbatim in both directions minus the hop-by-hop
//!   set, so the client's `Host` survives and `code-server` builds its URLs
//!   against the outer origin. `Content-Length` is left exactly as the upstream set it: the
//!   body is streamed through untouched, so the number is still true.
//! * **framing** — the response's `Content-Security-Policy` is replaced with
//!   `frame-ancestors 'self'` and `X-Frame-Options` removed, so the app can be
//!   framed by our own UI and by nothing else.
//! * **websockets** — a byte passthrough. The client's handshake is written to
//!   the upstream verbatim (which is what preserves `Sec-WebSocket-Protocol`
//!   and any extension negotiation), the upstream's `101` is returned verbatim,
//!   and the two sockets are then spliced. No frame is ever parsed.

use std::collections::HashSet;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use gotcha::axum::body::Body;
use gotcha::axum::http::{header, HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, Uri, Version};
use hyper::upgrade::OnUpgrade;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower_service::Service;

use super::Registry;

/// The client the proxy (and the readiness probe) make upstream requests with.
pub type HttpClient = hyper_util::client::legacy::Client<HttpConnector, Body>;

/// A pooled loopback HTTP client.
pub fn client() -> HttpClient {
    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build_http()
}

/// Only our own UI may frame an embedded app.
const FRAME_ANCESTORS: &str = "frame-ancestors 'self'";
/// What a client is told when the app cannot be reached.
const UNREACHABLE: &str = "app unreachable";
/// What a client is told for an id no instance answers to.
const NOT_FOUND: &str = "app not found";
/// A handshake response head past this is not one.
const MAX_HEAD: usize = 64 * 1024;

/// Headers that belong to one hop and must not be forwarded.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// The `/embed/{id}/*` proxy, in front of whatever owns every other path.
#[derive(Clone)]
pub struct Embed<Inner> {
    apps: Arc<Registry>,
    client: HttpClient,
    inner: Inner,
}

/// The fallback service: embeds go to the apps, everything else to `inner`.
pub fn fallback<Inner>(apps: Arc<Registry>, inner: Inner) -> Embed<Inner> {
    Embed {
        client: apps.client.clone(),
        apps,
        inner,
    }
}

impl<Inner> Service<Request<Body>> for Embed<Inner>
where
    Inner: Service<Request<Body>, Response = Response<Body>, Error = Infallible> + Clone + Send + 'static,
    Inner::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let target = request.uri().path_and_query().map(|target| target.as_str().to_owned());
        let embed = target.as_deref().and_then(split).map(|(id, path)| (id.to_owned(), path.to_owned()));
        let Some((id, path)) = embed else {
            // `poll_ready` ran on `self.inner`, so that is the copy that must
            // serve this request; the clone takes its place for the next.
            let spare = self.inner.clone();
            let mut ready = std::mem::replace(&mut self.inner, spare);
            return Box::pin(ready.call(request));
        };
        let apps = self.apps.clone();
        let client = self.client.clone();
        Box::pin(async move { Ok(proxy(&apps, &client, request, &id, &path).await) })
    }
}

/// Split a request target into `(app id, upstream path)`.
///
/// The pattern is `/^\/embed\/([\w-]+)(\/.*)?$/` over the full request target,
/// which carries the query string — so `/embed/x/y?z` proxies to `/y?z`, while
/// `/embed/x?z` matches nothing at all and falls through to the SPA.
fn split(target: &str) -> Option<(&str, &str)> {
    let rest = target.strip_prefix("/embed/")?;
    let (id, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-') {
        return None;
    }
    Some((id, if path.is_empty() { "/" } else { path }))
}

/// Forward one request to the app `id` names.
async fn proxy(apps: &Registry, client: &HttpClient, request: Request<Body>, id: &str, path: &str) -> Response<Body> {
    let Some(port) = apps.port(id) else {
        return text(StatusCode::NOT_FOUND, NOT_FOUND);
    };
    if is_upgrade(request.headers()) {
        websocket(request, port, path).await
    } else {
        http(client, request, port, path).await
    }
}

/// Stream a plain request through, and the response back.
async fn http(client: &HttpClient, request: Request<Body>, port: u16, path: &str) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let Some(uri) = upstream_uri(port, path) else {
        return text(StatusCode::BAD_GATEWAY, UNREACHABLE);
    };

    let mut outgoing = Request::new(body);
    *outgoing.method_mut() = parts.method;
    *outgoing.uri_mut() = uri;
    *outgoing.version_mut() = Version::HTTP_11;
    *outgoing.headers_mut() = forwarded(&parts.headers);
    // An HTTP/1.1 client always sent one; an HTTP/2 one put it in the authority.
    if !outgoing.headers().contains_key(header::HOST) {
        if let Ok(host) = HeaderValue::from_str(&format!("127.0.0.1:{port}")) {
            outgoing.headers_mut().insert(header::HOST, host);
        }
    }

    let response = match client.request(outgoing).await {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(%error, port, "embedded app unreachable");
            return text(StatusCode::BAD_GATEWAY, UNREACHABLE);
        }
    };

    let (parts, body) = response.into_parts();
    let mut out = Response::new(Body::new(body));
    *out.status_mut() = parts.status;
    *out.headers_mut() = forwarded(&parts.headers);
    reframe(out.headers_mut());
    out
}

/// Splice a websocket to the app, byte for byte.
async fn websocket(mut request: Request<Body>, port: u16, path: &str) -> Response<Body> {
    // hyper parks this on the request for exactly this purpose; taking it is
    // what `axum`'s own `WebSocketUpgrade` does, and returning a `101` below is
    // what makes the connection hand itself over.
    let Some(upgrade) = request.extensions_mut().remove::<OnUpgrade>() else {
        tracing::debug!("a websocket upgrade arrived on a connection that cannot be upgraded");
        return text(StatusCode::BAD_GATEWAY, UNREACHABLE);
    };

    let mut upstream = match TcpStream::connect(("127.0.0.1", port)).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(%error, port, "cannot reach the embedded app's websocket");
            return text(StatusCode::BAD_GATEWAY, UNREACHABLE);
        }
    };

    let head = request_head(request.method().as_str(), path, request.headers());
    if let Err(error) = upstream.write_all(&head).await {
        tracing::debug!(%error, port, "cannot forward the websocket handshake");
        return text(StatusCode::BAD_GATEWAY, UNREACHABLE);
    }

    let (status, headers, pending) = match read_head(&mut upstream).await {
        Ok(head) => head,
        Err(error) => {
            tracing::debug!(%error, port, "the embedded app did not answer the handshake");
            return text(StatusCode::BAD_GATEWAY, UNREACHABLE);
        }
    };
    if status != StatusCode::SWITCHING_PROTOCOLS {
        tracing::debug!(%status, port, "the embedded app refused the websocket upgrade");
        return text(StatusCode::BAD_GATEWAY, UNREACHABLE);
    }

    tokio::spawn(async move {
        let Ok(upgraded) = upgrade.await else { return };
        let mut client = TokioIo::new(upgraded);
        // Anything the upstream sent in the same packet as its `101` was read
        // into our buffer already, and has to lead the stream.
        if !pending.is_empty() && client.write_all(&pending).await.is_err() {
            return;
        }
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    });

    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    // Verbatim: `Sec-WebSocket-Accept` is computed over the key we forwarded,
    // and any negotiated subprotocol or extension is the upstream's answer.
    *response.headers_mut() = headers;
    response
}

/// Whether this request is asking to leave HTTP behind.
fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE)
        && headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
}

/// `http://127.0.0.1:{port}{path}`, or `None` if `path` is not a valid target.
fn upstream_uri(port: u16, path: &str) -> Option<Uri> {
    Uri::builder()
        .scheme("http")
        .authority(format!("127.0.0.1:{port}"))
        .path_and_query(path)
        .build()
        .ok()
}

/// Every header worth forwarding: the hop-by-hop set and anything `Connection`
/// nominates are this connection's business, not the next one's.
fn forwarded(source: &HeaderMap) -> HeaderMap {
    let nominated: HashSet<String> = source
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();

    let mut out = HeaderMap::with_capacity(source.len());
    for (name, value) in source {
        let name_str = name.as_str();
        if HOP_BY_HOP.contains(&name_str) || nominated.contains(name_str) {
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    out
}

/// Replace the app's framing policy with ours.
fn reframe(headers: &mut HeaderMap) {
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(FRAME_ANCESTORS));
    headers.remove(header::X_FRAME_OPTIONS);
}

/// A plain-text response, the shape both error paths answer with.
fn text(status: StatusCode, body: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    response
}

/// Serialize a request head for the upstream, headers untouched.
fn request_head(method: &str, path: &str, headers: &HeaderMap) -> Vec<u8> {
    let mut head = Vec::with_capacity(512);
    head.extend_from_slice(method.as_bytes());
    head.push(b' ');
    head.extend_from_slice(path.as_bytes());
    head.extend_from_slice(b" HTTP/1.1\r\n");
    for (name, value) in headers {
        head.extend_from_slice(name.as_str().as_bytes());
        head.extend_from_slice(b": ");
        head.extend_from_slice(value.as_bytes());
        head.extend_from_slice(b"\r\n");
    }
    head.extend_from_slice(b"\r\n");
    head
}

/// Read one response head off `stream`, returning it plus whatever bytes of the
/// body (here: the first websocket frames) came along with it.
async fn read_head(stream: &mut TcpStream) -> std::io::Result<(StatusCode, HeaderMap, Vec<u8>)> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let (status, headers) = parse_head(&buffer[..end])?;
            return Ok((status, headers, buffer[end + 4..].to_vec()));
        }
        if buffer.len() > MAX_HEAD {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "response head too large"));
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the upstream closed before answering",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

/// Parse a status line and the headers under it.
///
/// Hand-rolled rather than reached for a parser: the only response that ever
/// reaches it is a websocket handshake, whose head is a status line and a
/// handful of single-line fields.
fn parse_head(head: &[u8]) -> std::io::Result<(StatusCode, HeaderMap)> {
    let malformed = || std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed response head");
    let text = std::str::from_utf8(head).map_err(|_| malformed())?;
    let mut lines = text.split("\r\n");

    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| StatusCode::from_bytes(code.as_bytes()).ok())
        .ok_or_else(malformed)?;

    let mut headers = HeaderMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let (Ok(name), Ok(value)) = (HeaderName::from_bytes(name.trim().as_bytes()), HeaderValue::from_str(value.trim())) else {
            continue;
        };
        headers.append(name, value);
    }
    Ok((status, headers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_split_is_the_typescripts_regex() {
        assert_eq!(split("/embed/abc"), Some(("abc", "/")), "a bare id is the app's root");
        assert_eq!(split("/embed/abc/"), Some(("abc", "/")));
        assert_eq!(split("/embed/abc/stable/out/main.js"), Some(("abc", "/stable/out/main.js")));
        assert_eq!(
            split("/embed/a-b_1/x?y=1&z=2"),
            Some(("a-b_1", "/x?y=1&z=2")),
            "node matches over `req.url`, so the query rides along"
        );

        assert_eq!(split("/embed/"), None, "the id is `[\\w-]+`, never empty");
        assert_eq!(split("/embed/abc?y=1"), None, "no slash after the id: not a match, so the SPA gets it");
        assert_eq!(split("/embed/a.b/x"), None, "a dot is not a word character");
        assert_eq!(split("/api/apps"), None);
        assert_eq!(split("/embedded/abc/x"), None);
    }

    #[test]
    fn our_framing_policy_replaces_the_apps() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("frame-ancestors 'none'"));
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));

        reframe(&mut headers);

        assert_eq!(headers[header::CONTENT_SECURITY_POLICY], FRAME_ANCESTORS);
        assert!(!headers.contains_key(header::X_FRAME_OPTIONS), "it would out-vote the CSP");
        assert_eq!(headers[header::CONTENT_TYPE], "text/html", "nothing else is touched");

        let mut absent = HeaderMap::new();
        reframe(&mut absent);
        assert_eq!(
            absent[header::CONTENT_SECURITY_POLICY],
            FRAME_ANCESTORS,
            "an app that sets none still gets ours"
        );
    }

    #[test]
    fn hop_by_hop_headers_stop_at_the_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade, x-private"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(header::TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        headers.insert("x-private", HeaderValue::from_static("1"));
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8700"));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("12"));

        let out = forwarded(&headers);

        assert!(!out.contains_key(header::CONNECTION));
        assert!(!out.contains_key(header::UPGRADE));
        assert!(!out.contains_key(header::TRANSFER_ENCODING));
        assert!(!out.contains_key("x-private"), "`Connection` nominated it");
        assert_eq!(out[header::HOST], "localhost:8700", "the client's host is what the app builds URLs from");
        assert_eq!(out[header::CONTENT_LENGTH], "12", "the body is forwarded whole, so the count still holds");
    }

    #[test]
    fn an_upgrade_needs_both_headers() {
        let with = |pairs: &[(HeaderName, &'static str)]| {
            let mut headers = HeaderMap::new();
            for (name, value) in pairs {
                headers.insert(name.clone(), HeaderValue::from_static(value));
            }
            is_upgrade(&headers)
        };
        assert!(with(&[(header::UPGRADE, "websocket"), (header::CONNECTION, "Upgrade")]));
        assert!(with(&[(header::UPGRADE, "websocket"), (header::CONNECTION, "keep-alive, Upgrade")]));
        assert!(!with(&[(header::UPGRADE, "websocket"), (header::CONNECTION, "keep-alive")]));
        assert!(!with(&[(header::CONNECTION, "Upgrade")]));
        assert!(!with(&[]));
    }

    #[test]
    fn a_handshake_head_parses_and_the_frames_behind_it_survive() {
        let raw = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                    Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\nSec-WebSocket-Protocol: chat\r\n\r\n\x81\x03abc";
        let end = raw.windows(4).position(|window| window == b"\r\n\r\n").expect("head end");
        let (status, headers) = parse_head(&raw[..end]).expect("parse");

        assert_eq!(status, StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(headers["sec-websocket-accept"], "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        assert_eq!(headers["sec-websocket-protocol"], "chat", "a negotiated subprotocol is passed back");
        assert_eq!(&raw[end + 4..], b"\x81\x03abc", "the first frame came in the same packet");

        assert!(parse_head(b"not a status line").is_err());
    }

    #[test]
    fn the_request_head_goes_out_untouched() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8700"));
        headers.insert("sec-websocket-protocol", HeaderValue::from_static("chat, superchat"));

        let head = String::from_utf8(request_head("GET", "/socket?x=1", &headers)).expect("utf8");

        assert!(head.starts_with("GET /socket?x=1 HTTP/1.1\r\n"), "{head}");
        assert!(head.contains("host: localhost:8700\r\n"), "{head}");
        assert!(
            head.contains("sec-websocket-protocol: chat, superchat\r\n"),
            "subprotocols are the client's to offer: {head}"
        );
        assert!(head.ends_with("\r\n\r\n"));
    }
}
