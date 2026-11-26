//! The proxy's HTTP side.
//!
//! `GET /healthz` answers anyone `{"status": "up", "enrolled": …}`: up, and
//! whether the proxy has joined its account (until it has, every protected
//! route is answered 503, "not ready"). Every other request must carry the
//! proxy's token (`Authorization: Bearer <DROPPEN_SESSION_PROXY_TOKEN>`),
//! compared in constant time through a SHA-256 of each side; without it, the
//! same 401 for every path.
//!
//! Routes are the backend's small session surface: start a session
//! (`POST /sessions`, which remembers the id's device), send it a message,
//! interrupt it, answer a question, and follow its events as SSE. `/rpc`
//! carries any other `/api/…` request straight through. Each goes over the
//! device's channel as an `rpc` pipe — or, for events, a `mux` pipe to
//! `/api/events?v=2` — and the answer is relayed as it arrives, for events and
//! messages as a stream. A failure that keeps the request from being answered
//! is the proxy's own ([`crate::errors`]), judged by its chain
//! ([`crate::upstream`]), never a 500.
//!
//! A request is one log line: method, session or device, status or refusal.
//! Never a body, a header's value or the token.

use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use droppen_client::failure::Failure;
use droppen_client::rpc::RpcRequest;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::errors::{self, Refusal};
use crate::route::{clean_api_path, session_id_of, Sessions};
use crate::upstream::{MuxItem, Upstream};

/// The largest request body taken (the same the agent buffers on a pipe).
pub const MAX_BODY: usize = 64 * 1024 * 1024;
/// The largest buffered answer taken.
pub const MAX_ANSWER: usize = 64 * 1024 * 1024;
/// Request headers carried on a `/rpc`.
const MAX_HEADERS: usize = 64;

/// Response headers never passed back (hop-by-hop, or re-framed here).
const DROPPED: [&str; 8] = [
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "proxy-authenticate",
    "proxy-connection",
    "content-length",
    "trailer",
];

/// The proxy: where requests go, and what it checks.
pub struct Proxy {
    /// Set once the proxy's device is in its account; until then every
    /// protected route is answered "not ready".
    upstream: OnceLock<Arc<dyn Upstream>>,
    /// SHA-256 of the token.
    token: [u8; 32],
    /// The sessions this proxy started.
    sessions: Sessions,
}

impl Proxy {
    pub fn new(upstream: Arc<dyn Upstream>, token: &str) -> Proxy {
        let proxy = Proxy::waiting(token);
        proxy.ready(upstream);
        proxy
    }

    /// A proxy whose device is not in its account yet: it answers its health,
    /// and every protected route with "not ready", until [`Proxy::ready`].
    pub fn waiting(token: &str) -> Proxy {
        Proxy {
            upstream: OnceLock::new(),
            token: Sha256::digest(token.as_bytes()).into(),
            sessions: Sessions::new(),
        }
    }

    /// Requests go to `upstream` from now on.
    pub fn ready(&self, upstream: Arc<dyn Upstream>) {
        let _ = self.upstream.set(upstream);
    }

    /// Whether its device is in its account (and requests go through).
    pub fn is_ready(&self) -> bool {
        self.upstream.get().is_some()
    }

    fn upstream(&self) -> Option<Arc<dyn Upstream>> {
        self.upstream.get().cloned()
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
}

/// The proxy's routes: `/healthz` open, the rest behind the token.
pub fn router(proxy: Arc<Proxy>) -> Router {
    let protected = Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/{id}/message", post(message))
        .route("/sessions/{id}/events", get(events))
        .route("/sessions/{id}/interrupt", post(interrupt))
        .route("/sessions/{id}/answer", post(answer))
        .route("/rpc", post(rpc))
        .route_layer(middleware::from_fn_with_state(proxy.clone(), guard));
    Router::new().route("/healthz", get(health)).merge(protected).with_state(proxy)
}

/// The token guard, before any route sees the request.
async fn guard(State(proxy): State<Arc<Proxy>>, request: Request, next: Next) -> Response {
    if !proxy.authorized(request.headers()) {
        return errors::unauthorized();
    }
    next.run(request).await
}

/// `GET /healthz`, with no token: that the proxy is up, and whether it has
/// joined its account (`enrolled`) — nothing else.
async fn health(State(proxy): State<Arc<Proxy>>) -> Response {
    errors::json_response(StatusCode::OK, &serde_json::json!({"status": "up", "enrolled": proxy.is_ready()}))
}

/// Whether the request may go to `device`, and the proxy ready: `Err` is the
/// answer to give, if any.
async fn reach(proxy: &Proxy, device: &str) -> Result<Arc<dyn Upstream>, Response> {
    let Some(upstream) = proxy.upstream() else {
        return Err(errors::not_ready());
    };
    if let Err(refusal) = upstream.admits(device).await {
        return Err(refusal.response());
    }
    Ok(upstream)
}

/// `POST /sessions`: start one on `device` and remember the id it got.
async fn create_session(State(proxy): State<Arc<Proxy>>, request: Request) -> Response {
    let mut body = match json_body(request).await {
        Ok(Value::Object(body)) => body,
        Ok(_) => return errors::bad_request("the body is a JSON object"),
        Err(response) => return response,
    };
    let Some(device) = body.remove("device").and_then(|d| d.as_str().map(String::from)).filter(|d| !d.is_empty()) else {
        return errors::bad_request("`device` is required");
    };
    body.insert("source".into(), Value::String("claude".into()));
    let upstream = match reach(&proxy, &device).await {
        Ok(upstream) => upstream,
        Err(response) => return response,
    };
    let call = RpcRequest {
        method: "POST".into(),
        path: "/api/sessions/new".into(),
        headers: json_headers(),
        body: Value::Object(body).to_string().into_bytes(),
    };
    let answer = match upstream.call(&device, call).await {
        Ok(answer) => answer,
        Err(failure) => return judged(&*upstream, &device, &failure).await,
    };
    let status = answer.status;
    let buffered = match collect(answer.body).await {
        Ok(buffered) => buffered,
        Err(Outcome::TooLarge) => return errors::answer_too_large(MAX_ANSWER),
        Err(Outcome::Failed(failure)) => return judged(&*upstream, &device, &failure).await,
    };
    if (200..300).contains(&status) {
        if let Some(id) = serde_json::from_slice::<Value>(&buffered).ok().as_ref().and_then(|v| v.get("id")).and_then(Value::as_str) {
            proxy.sessions.bind(id, device.clone());
        }
    }
    tracing::info!(device = %device, status, "start a session");
    echo(status, Some("application/json"), buffered)
}

/// `POST /sessions/{id}/message`.
async fn message(State(proxy): State<Arc<Proxy>>, Path(id): Path<String>, request: Request) -> Response {
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY).await {
        Ok(body) => body,
        Err(_) => return errors::bad_request("the body is over 64 MiB"),
    };
    let call = RpcRequest {
        method: "POST".into(),
        path: format!("/api/sessions/claude/{id}/message"),
        headers: json_headers(),
        body: body.to_vec(),
    };
    forward_session(&proxy, &id, call).await
}

/// `POST /sessions/{id}/interrupt`.
async fn interrupt(State(proxy): State<Arc<Proxy>>, Path(id): Path<String>, request: Request) -> Response {
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY).await {
        Ok(body) => body,
        Err(_) => return errors::bad_request("the body is over 64 MiB"),
    };
    let call = RpcRequest {
        method: "POST".into(),
        path: format!("/api/sessions/claude/{id}/interrupt"),
        headers: json_headers(),
        body: body.to_vec(),
    };
    forward_session(&proxy, &id, call).await
}

/// `POST /sessions/{id}/answer`: `{requestId, answers?, notes?, response?}`,
/// or `{requestId, dismiss: true}`; the extras go on as they came.
async fn answer(State(proxy): State<Arc<Proxy>>, Path(id): Path<String>, request: Request) -> Response {
    let mut body = match json_body(request).await {
        Ok(Value::Object(body)) => body,
        Ok(_) => return errors::bad_request("the body is a JSON object"),
        Err(response) => return response,
    };
    let Some(request_id) = body.remove("requestId").and_then(|r| r.as_str().map(String::from)).filter(|r| !r.is_empty()) else {
        return errors::bad_request("`requestId` is required");
    };
    let dismiss = body.remove("dismiss").and_then(|d| d.as_bool()).unwrap_or(false);
    let route = if dismiss { "dismiss" } else { "answer" };
    let call = RpcRequest {
        method: "POST".into(),
        path: format!("/api/sessions/claude/{id}/questions/{request_id}/{route}"),
        headers: json_headers(),
        body: Value::Object(body).to_string().into_bytes(),
    };
    forward_session(&proxy, &id, call).await
}

/// `GET /sessions/{id}/events`, as SSE: one mux socket to the device, `sub`
/// this session, each frame down as an event, then `closed` when the socket
/// ends.
async fn events(State(proxy): State<Arc<Proxy>>, Path(id): Path<String>, request: Request) -> Response {
    let _ = request;
    let Some(device) = proxy.sessions.device_of(&id) else {
        return errors::unknown_session();
    };
    let upstream = match reach(&proxy, &device).await {
        Ok(upstream) => upstream,
        Err(response) => return response,
    };
    let sub = sub_frame(&id);
    let stream = match upstream.events(&device, sub).await {
        Ok(stream) => stream,
        Err(failure) => return judged(&*upstream, &device, &failure).await,
    };
    tracing::info!(device = %device, session = %id, "follow a session");
    let body = futures_util::stream::unfold((stream, false), |(mut stream, done)| async move {
        if done {
            return None;
        }
        match stream.next().await {
            Some(Ok(MuxItem::Frame(frame))) => {
                let (event, closed) = sse_frame(&frame);
                Some((Ok::<_, std::convert::Infallible>(event.into_bytes()), (stream, closed)))
            }
            Some(Ok(MuxItem::Closed)) | None => Some((Ok(closed_event(None).into_bytes()), (stream, true))),
            Some(Err(failure)) => Some((Ok(closed_event(Some(&failure)).into_bytes()), (stream, true))),
        }
    });
    let mut response = Response::new(Body::from_stream(body));
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response.headers_mut().insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

/// The `sub` frame that attaches the one subscription: `sub` 1, source
/// `claude`, `id` the session, a 200-item opening snapshot.
fn sub_frame(id: &str) -> String {
    serde_json::json!({
        "type": "sub",
        "feeds": [{"sub": 1, "source": "claude", "id": id, "limit": 200}],
    })
    .to_string()
}

/// A frame down the mux socket as SSE: `event: <type>`, `data: <json>`. The
/// second value says whether it was the socket's `closed`.
fn sse_frame(frame: &[u8]) -> (String, bool) {
    let value = serde_json::from_slice::<Value>(frame).ok();
    let kind = value
        .as_ref()
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("message")
        .to_string();
    let data = match value {
        Some(value) => value.to_string(),
        None => String::from_utf8_lossy(frame).into_owned(),
    };
    let mut event = format!("event: {kind}\n");
    for line in data.split('\n') {
        event.push_str("data: ");
        event.push_str(line);
        event.push('\n');
    }
    event.push('\n');
    (event, kind == "closed")
}

/// The `closed` event that ends a stream: the failure's code and message
/// when there was one.
fn closed_event(failure: Option<&Failure>) -> String {
    let mut data = serde_json::json!({"type": "closed"});
    if let Some(failure) = failure {
        data["error"] = serde_json::json!("channel_failed");
        data["message"] = serde_json::json!(failure.to_string());
    }
    format!("event: closed\ndata: {data}\n\n")
}

/// `POST /rpc`: `{device, method, path, headers?, body?}` straight through.
async fn rpc(State(proxy): State<Arc<Proxy>>, request: Request) -> Response {
    let body = match json_body(request).await {
        Ok(Value::Object(body)) => body,
        Ok(_) => return errors::bad_request("the body is a JSON object"),
        Err(response) => return response,
    };
    let Some(device) = body.get("device").and_then(Value::as_str).filter(|d| !d.is_empty()).map(String::from) else {
        return errors::bad_request("`device` is required");
    };
    let method = match body.get("method").and_then(Value::as_str).filter(|m| is_method(m)) {
        Some(method) => method.to_string(),
        None => return errors::bad_request("`method` is an HTTP method"),
    };
    let Some(path) = body.get("path").and_then(Value::as_str) else {
        return errors::bad_request("`path` is required");
    };
    if !clean_api_path(path) {
        return errors::bad_request("`path` must be a clean `/api/…` path");
    }
    if let Some(id) = session_id_of(path) {
        if proxy.sessions.device_of(id).is_none() {
            return errors::unknown_session();
        }
    }
    let headers = match rpc_headers(body.get("headers")) {
        Ok(headers) => headers,
        Err(response) => return response,
    };
    let body = match body.get("body") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) => text.clone().into_bytes(),
        Some(other) => other.to_string().into_bytes(),
    };
    let upstream = match reach(&proxy, &device).await {
        Ok(upstream) => upstream,
        Err(response) => return response,
    };
    let call = RpcRequest {
        method,
        path: path.to_string(),
        headers,
        body,
    };
    let answer = match upstream.call(&device, call).await {
        Ok(answer) => answer,
        Err(failure) => return judged(&*upstream, &device, &failure).await,
    };
    let status = answer.status;
    let content_type = answer
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone());
    let buffered = match collect(answer.body).await {
        Ok(buffered) => buffered,
        Err(Outcome::TooLarge) => return errors::answer_too_large(MAX_ANSWER),
        Err(Outcome::Failed(failure)) => return judged(&*upstream, &device, &failure).await,
    };
    tracing::info!(device = %device, %path, status, "rpc");
    echo(status, content_type.as_deref(), buffered)
}

/// A session route's call to its device, relayed as it streams.
async fn forward_session(proxy: &Proxy, id: &str, call: RpcRequest) -> Response {
    let route = call.path.clone();
    let Some(device) = proxy.sessions.device_of(id) else {
        return errors::unknown_session();
    };
    let upstream = match reach(proxy, &device).await {
        Ok(upstream) => upstream,
        Err(response) => return response,
    };
    let answer = match upstream.call(&device, call).await {
        Ok(answer) => answer,
        Err(failure) => return judged(&*upstream, &device, &failure).await,
    };
    let status = answer.status;
    tracing::info!(device = %device, session = %id, %route, status, "session request");
    let mut response = Response::new(Body::from_stream(answer.body));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (name, value) in &answer.headers {
        let lower = name.to_ascii_lowercase();
        if DROPPED.contains(&lower.as_str()) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (HeaderName::from_bytes(lower.as_bytes()), HeaderValue::from_str(value)) {
            response.headers_mut().append(name, value);
        }
    }
    response
}

/// The proxy's answer to a failed call: the refusal its chain judges.
async fn judged(upstream: &dyn Upstream, device: &str, failure: &Failure) -> Response {
    let refusal: Refusal = upstream.judge(device, failure).await;
    tracing::info!(device = %device, code = refusal.code(), kind = failure.kind(), "failed");
    refusal.response()
}

/// A buffered `body` under `content_type`.
fn echo(status: u16, content_type: Option<&str>, body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = content_type.and_then(|v| HeaderValue::from_str(v).ok()).unwrap_or_else(|| HeaderValue::from_static("application/json"));
    response.headers_mut().insert(header::CONTENT_TYPE, content_type);
    response
}

enum Outcome {
    TooLarge,
    Failed(Failure),
}

/// The whole of `body`, up to [`MAX_ANSWER`] bytes.
async fn collect(mut body: BoxStream<'static, Result<Vec<u8>, Failure>>) -> Result<Vec<u8>, Outcome> {
    let mut out = Vec::new();
    while let Some(piece) = body.next().await {
        match piece {
            Ok(piece) => out.extend_from_slice(&piece),
            Err(failure) => return Err(Outcome::Failed(failure)),
        }
        if out.len() > MAX_ANSWER {
            return Err(Outcome::TooLarge);
        }
    }
    Ok(out)
}

/// The request's body as JSON, or the 400/413 answer.
async fn json_body(request: Request) -> Result<Value, Response> {
    let bytes = axum::body::to_bytes(request.into_body(), MAX_BODY)
        .await
        .map_err(|_| errors::bad_request("the body is over 64 MiB or was cut off"))?;
    serde_json::from_slice(&bytes).map_err(|_| errors::bad_request("the body is not JSON"))
}

fn is_method(method: &str) -> bool {
    (1..=16).contains(&method.len()) && method.bytes().all(|b| b.is_ascii_uppercase())
}

/// `content-type: application/json` alone.
fn json_headers() -> Vec<(String, String)> {
    vec![("content-type".into(), "application/json".into())]
}

/// A `/rpc` request's headers: pairs of strings, at most [`MAX_HEADERS`].
fn rpc_headers(value: Option<&Value>) -> Result<Vec<(String, String)>, Response> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Value::Array(pairs) = value else {
        return Err(errors::bad_request("`headers` is an array of [name, value] pairs"));
    };
    if pairs.len() > MAX_HEADERS {
        return Err(errors::bad_request("`headers` has over 64 entries"));
    }
    pairs
        .iter()
        .map(|pair| match pair {
            Value::Array(pair) if pair.len() == 2 => match (pair[0].as_str(), pair[1].as_str()) {
                (Some(name), Some(value)) => Ok((name.to_string(), value.to_string())),
                _ => Err(errors::bad_request("a header is a pair of strings")),
            },
            _ => Err(errors::bad_request("a header is a [name, value] pair")),
        })
        .collect()
}

/// The names in `headers`, for tests.
#[cfg(test)]
fn header_names(headers: &[(String, String)]) -> std::collections::BTreeSet<&str> {
    headers.iter().map(|(name, _)| name.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::upstream::classify;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn proxy_with(headers: &[(&str, &str)]) -> (Proxy, bool) {
        let proxy = Proxy::waiting(TOKEN);
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let authorized = proxy.authorized(&map);
        (proxy, authorized)
    }

    #[test]
    fn only_the_exact_bearer_gets_in() {
        assert!(proxy_with(&[("authorization", &format!("Bearer {TOKEN}"))]).1);
        assert!(proxy_with(&[("authorization", &format!("bearer {TOKEN}"))]).1, "scheme in any case");
        for (name, value) in [
            ("authorization", "Bearer wrong"),
            ("authorization", "Basic 0123456789abcdef0123456789abcdef"),
            ("authorization", "Bearer 0123456789abcdef0123456789abcdefx"),
            ("authorization", "Bearer"),
            ("x-token", TOKEN),
        ] {
            assert!(!proxy_with(&[(name, value)]).1, "{name}: {value}");
        }
    }

    #[test]
    fn the_sub_frame_asks_for_the_one_session() {
        assert_eq!(
            serde_json::from_str::<Value>(&sub_frame("s-1")).unwrap(),
            serde_json::json!({"type": "sub", "feeds": [{"sub": 1, "source": "claude", "id": "s-1", "limit": 200}]})
        );
    }

    #[test]
    fn a_frame_is_an_event_named_by_its_type() {
        let (event, closed) = sse_frame(br#"{"sub":1,"type":"snapshot","items":[]}"#);
        assert_eq!(event, "event: snapshot\ndata: {\"sub\":1,\"type\":\"snapshot\",\"items\":[]}\n\n");
        assert!(!closed);
        let (event, closed) = sse_frame(br#"{"sub":1,"type":"closed"}"#);
        assert_eq!(event, "event: closed\ndata: {\"sub\":1,\"type\":\"closed\"}\n\n");
        assert!(closed);
        let (event, closed) = sse_frame(b"not json");
        assert_eq!(event, "event: message\ndata: not json\n\n");
        assert!(!closed);
    }

    #[test]
    fn a_failure_is_a_closed_event_with_its_word() {
        let event = closed_event(Some(&Failure::Offline("the channel ended".into())));
        assert!(event.starts_with("event: closed\ndata: {"));
        assert!(event.contains("\"error\":\"channel_failed\""));
        assert!(event.ends_with("\n\n"));
        assert_eq!(closed_event(None), "event: closed\ndata: {\"type\":\"closed\"}\n\n");
    }

    #[test]
    fn headers_are_pairs_of_strings() {
        assert_eq!(rpc_headers(None).unwrap(), Vec::new());
        let value = serde_json::json!([["accept", "application/json"]]);
        assert_eq!(header_names(&rpc_headers(Some(&value)).unwrap()), BTreeSet::from(["accept"]));
        for bad in [serde_json::json!("x"), serde_json::json!([["a"]]), serde_json::json!([[1, 2]])] {
            assert!(rpc_headers(Some(&bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_failure_table_is_the_backends() {
        // The mapping itself lives in `upstream`; this is the status side the
        // backend depends on.
        for (failure, status, code) in [
            (Failure::Offline("x".into()), 503, "host_offline"),
            (Failure::Stale("x".into()), 503, "host_stale"),
            (Failure::Lapsed("x".into()), 403, "host_not_accepting"),
            (Failure::Removed("x".into()), 410, "device_removed"),
            (Failure::Chain("x".into()), 502, "channel_failed"),
            (Failure::Other("x".into()), 502, "channel_failed"),
        ] {
            let refusal = classify(None, "me", "host", &failure);
            assert_eq!((refusal.status().as_u16(), refusal.code()), (status, code));
        }
    }
}
