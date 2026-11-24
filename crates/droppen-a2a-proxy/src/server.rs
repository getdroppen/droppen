//! The proxy's HTTP side (`docs/a2a.md` §7.4).
//!
//! Every request but `GET /healthz` must carry the proxy's token
//! (`Authorization: Bearer <DROPPEN_A2A_PROXY_TOKEN>`), compared in constant
//! time through a SHA-256 of each side; without it, the same 401 for every
//! path. A route ([`crate::route`]) goes to its device as an `rpc` pipe: the
//! method, the path under `/a2a/agents/<entry>/` with its query, an allowlist
//! of headers (the client's `Authorization` never), and the body, read whole
//! (at most 10 MiB, §5). The answer comes back as it arrives — its head at
//! once, its body streamed (an SSE answer for as long as it runs) — but for:
//!
//! - the Agent Card: its interface addresses become the proxy's own
//!   ([`crate::card`]), so it is read whole (at most 1 MiB), and asked for
//!   whole (no `If-None-Match`, no `ETag` back: the device's would name
//!   another body);
//! - a `412`: read whole (at most 64 KiB) to tell the bridge's own `stale`
//!   refusal from the route's answer.
//!
//! When the device cannot be reached, or refuses, the proxy answers itself
//! ([`crate::errors`]): in JSON-RPC's form for `/jsonrpc` (to the request's
//! id), in HTTP+JSON's otherwise, the code judged by the proxy's own chain
//! ([`crate::upstream`]). A stream that breaks after its head is cut off: the
//! client follows up with `SubscribeToTask` or `GetTask` (the proxy does not
//! reconnect for it, §7.4).
//!
//! Each request is one log line: method, device, entry, binding, status (or
//! the failure). Never a body, a header's value or the token.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use droppen_client::failure::Failure;
use droppen_client::rpc::{refusal_of, RpcRequest};
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::card;
use crate::errors::{jsonrpc_id, not_found, unauthenticated, Code, ProxyError};
use crate::route::{Binding, Route};
use crate::upstream::Upstream;

/// The largest request body taken (`docs/a2a.md` §5).
pub const MAX_BODY: usize = 10 * 1024 * 1024;
/// The largest Agent Card read.
const MAX_CARD: usize = 1024 * 1024;
/// The most of a `412` answer read to judge it.
const MAX_REFUSAL: usize = 64 * 1024;

/// Request headers a client may send on; everything else is dropped.
const FORWARDED: [&str; 8] = [
    "content-type",
    "accept",
    "accept-language",
    "a2a-version",
    "a2a-extensions",
    "if-none-match",
    "if-match",
    "cache-control",
];

/// Response headers never passed back (hop-by-hop, or re-framed here).
const DROPPED: [&str; 9] = [
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "proxy-authenticate",
    "proxy-connection",
    "content-length",
    "set-cookie",
    "trailer",
];

/// The proxy: where requests go, and what it checks.
pub struct Proxy {
    upstream: Arc<dyn Upstream>,
    /// SHA-256 of the token.
    token: [u8; 32],
    public_base: Option<String>,
    aliases: BTreeMap<String, String>,
}

impl Proxy {
    pub fn new(upstream: Arc<dyn Upstream>, token: &str, public_base: Option<String>, aliases: BTreeMap<String, String>) -> Proxy {
        Proxy {
            upstream,
            token: Sha256::digest(token.as_bytes()).into(),
            public_base,
            aliases,
        }
    }

    /// Whether `headers` carry the token (`Bearer`, any case).
    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some((scheme, token)) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().split_once(' '))
        else {
            return false;
        };
        let given: [u8; 32] = Sha256::digest(token.trim().as_bytes()).into();
        scheme.eq_ignore_ascii_case("bearer") & bool::from(given.ct_eq(&self.token))
    }

    /// The base the card's addresses start with: the configured one, or the
    /// request's `Host` over `http`.
    fn base(&self, headers: &HeaderMap) -> String {
        if let Some(base) = &self.public_base {
            return base.clone();
        }
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b)))
            .unwrap_or("localhost");
        format!("http://{host}")
    }
}

/// The proxy's routes.
pub fn router(proxy: Arc<Proxy>) -> Router {
    Router::new().route("/healthz", get(|| async { "ok" })).fallback(handle).with_state(proxy)
}

async fn handle(State(proxy): State<Arc<Proxy>>, request: Request) -> Response {
    if !proxy.authorized(request.headers()) {
        return unauthenticated();
    }
    let uri = request.uri().clone();
    let Some(route) = Route::parse(uri.path(), uri.query()) else {
        return not_found();
    };
    let host = proxy.aliases.get(&route.device).cloned().unwrap_or_else(|| route.device.clone());
    let method = request.method().clone();
    let base = proxy.base(request.headers());
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .filter(|(name, _)| FORWARDED.contains(&name.as_str()) && !(route.binding == Binding::Card && *name == header::IF_NONE_MATCH))
        .filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string())))
        .collect();
    let log = |outcome: &dyn std::fmt::Display| {
        tracing::info!(method = %method, device = %route.device, entry = %route.entry, binding = ?route.binding, %outcome, "a2a request");
    };
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY).await {
        Ok(body) => body,
        Err(_) => {
            log(&"refused: the body is over 10 MiB");
            return ProxyError::new(Code::QuotaExceeded).response(route.binding, &Value::Null);
        }
    };
    let id = if route.binding == Binding::JsonRpc {
        jsonrpc_id(&body)
    } else {
        Value::Null
    };
    if let Err(code) = proxy.upstream.admits(&host).await {
        log(&code.as_str());
        return ProxyError::new(code).response(route.binding, &id);
    }
    let call = RpcRequest {
        method: method.as_str().to_string(),
        path: route.upstream_path(),
        headers,
        body: body.to_vec(),
    };
    let answer = match proxy.upstream.call(&host, call).await {
        Ok(answer) => answer,
        Err(failure) => return failed(&proxy, &host, &route, &failure, &id, &log).await,
    };
    let status = answer.status;
    let mut body = answer.body;
    if status == 412 {
        let buffered = match collect(&mut body, MAX_REFUSAL).await {
            Ok(buffered) => buffered,
            Err(failure) => return failed(&proxy, &host, &route, &failure, &id, &log).await,
        };
        if let Some(failure) = refusal_of(status, &buffered) {
            return failed(&proxy, &host, &route, &failure, &id, &log).await;
        }
        log(&status);
        return respond(status, &answer.headers, false, Body::from(buffered));
    }
    if route.binding == Binding::Card && status == 200 {
        let buffered = match collect(&mut body, MAX_CARD).await {
            Ok(buffered) => buffered,
            Err(failure) => return failed(&proxy, &host, &route, &failure, &id, &log).await,
        };
        let Ok(mut card) = serde_json::from_slice::<Value>(&buffered) else {
            log(&"the device's card is not JSON");
            return ProxyError::internal("the device answered with a card that is not JSON").response(route.binding, &id);
        };
        card::rewrite(&mut card, &base, &route.device, &route.entry);
        log(&status);
        return respond(status, &answer.headers, true, Body::from(card.to_string()));
    }
    log(&status);
    let device = route.device.clone();
    let streamed = body.inspect(move |piece| {
        if let Err(failure) = piece {
            tracing::warn!(%device, kind = failure.kind(), %failure, "an answer broke off as it streamed");
        }
    });
    respond(status, &answer.headers, false, Body::from_stream(streamed))
}

/// The proxy's own answer to a failed call: the code judged by its chain.
async fn failed(proxy: &Proxy, host: &str, route: &Route, failure: &Failure, id: &Value, log: &impl Fn(&dyn std::fmt::Display)) -> Response {
    let error = match proxy.upstream.judge(host, failure).await {
        Some(code) => ProxyError::new(code),
        None => ProxyError::internal(format!("the proxy could not reach the device: {failure}")),
    };
    log(&format_args!(
        "{} ({}: {failure})",
        error.code.map_or("internal", Code::as_str),
        failure.kind()
    ));
    error.response(route.binding, id)
}

/// The whole of `body`, up to `max` bytes.
async fn collect(body: &mut BoxStream<'static, Result<Vec<u8>, Failure>>, max: usize) -> Result<Vec<u8>, Failure> {
    let mut out = Vec::new();
    while let Some(piece) = body.next().await {
        out.extend_from_slice(&piece?);
        if out.len() > max {
            return Err(Failure::Other(format!("the device's answer is larger than {max} bytes")));
        }
    }
    Ok(out)
}

/// A response with the device's `headers` (less the hop-by-hop ones, and its
/// `ETag` when the body was rewritten).
fn respond(status: u16, headers: &[(String, String)], rewritten: bool, body: Body) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (name, value) in headers {
        let lower = name.to_ascii_lowercase();
        if DROPPED.contains(&lower.as_str()) || (rewritten && lower == "etag") {
            continue;
        }
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(lower.as_bytes()), HeaderValue::from_str(value)) {
            response.headers_mut().append(name, value);
        }
    }
    if rewritten {
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    response
}
