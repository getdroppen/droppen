//! A relay's HTTP face (§2.4):
//!
//! | route | |
//! |---|---|
//! | `GET /healthz` | liveness |
//! | `GET /v1/ping` | 204, `Access-Control-Allow-Origin: *`, `Timing-Allow-Origin: *`: what browsers time to choose a relay; 120 a minute per address |
//! | `GET /v1/relay-id?nonce=…` | the relay's proof that it answers here (the Hub asks at registration) |
//! | `GET /v1/connect` | the websocket devices and agents sign in on ([`crate::client`]); 60 upgrades a minute per address, bursts of 20 |
//!
//! A client's address is the socket's, or the one the proxy in front says
//! (`DROPPEN_RELAY_TRUST_PROXY`); the per-address tables hold at most 50,000,
//! in memory only, and forget an address once its windows have refilled — a
//! minute after its last request, swept every minute ([`UnauthLimits::forget_old`],
//! `crate::run`), a day at the latest whatever the windows
//! ([`crate::limit::FORGET_AFTER`]).
//!
//! Every response says `Permissions-Policy: publickey-credentials-get=(),
//! publickey-credentials-create=()` (`docs/passkeys.md` §3.8 rule 3): a
//! relay's origin may be a subdomain of the app's (`*.relay.droppen.app`),
//! and nothing served from one may use the app's passkeys. A relay serves no
//! page anyway, only JSON and a WebSocket.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Query, State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::config::{hex, unhex, TrustProxy};
use crate::limit::{address_key, Keyed};
use crate::server::Server;

/// The per-address limits on what anyone may ask before signing in.
pub struct UnauthLimits {
    pub ping: Keyed<IpAddr>,
    pub upgrades: Keyed<IpAddr>,
}

impl Default for UnauthLimits {
    fn default() -> Self {
        let minute = Duration::from_secs(60);
        Self {
            ping: Keyed::new(&[(120, minute)]),
            upgrades: Keyed::new(&[(60, minute), (20, Duration::from_secs(20))]),
        }
    }
}

impl UnauthLimits {
    /// Forget the addresses there is nothing more to count against
    /// ([`Keyed::forget_old`]): how many went.
    pub fn forget_old(&self, now: Instant) -> usize {
        self.ping.forget_old(now) + self.upgrades.forget_old(now)
    }
}

pub fn router(server: Arc<Server>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/ping", get(ping))
        .route("/v1/relay-id", get(relay_id))
        .route("/v1/connect", get(connect))
        .layer(axum::middleware::map_response(no_webauthn))
        .with_state(server)
}

/// The `Permissions-Policy` every relay response carries: no WebAuthn.
pub const PERMISSIONS_POLICY: &str = "publickey-credentials-get=(), publickey-credentials-create=()";

/// Forbid WebAuthn to whatever a relay answers ([`PERMISSIONS_POLICY`]).
async fn no_webauthn(mut response: Response) -> Response {
    response.headers_mut().insert(
        axum::http::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(PERMISSIONS_POLICY),
    );
    response
}

/// The client's address: the socket's, or the trusted proxy's header.
pub fn client_ip(trust: TrustProxy, headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<IpAddr> {
    let header = match trust {
        TrustProxy::None => None,
        TrustProxy::Fly => Some("fly-client-ip"),
        TrustProxy::Railway => Some("x-real-ip"),
    };
    header
        .and_then(|h| headers.get(h))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .or(peer.map(|p| p.ip()))
}

fn too_many(wait: Duration) -> Response {
    let mut r = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({"error": "rate", "retry_after_ms": wait.as_millis() as u64})),
    )
        .into_response();
    if let Ok(v) = HeaderValue::from_str(&wait.as_secs().max(1).to_string()) {
        r.headers_mut().insert(header::RETRY_AFTER, v);
    }
    r
}

async fn ping(State(server): State<Arc<Server>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    let ip = address_key(client_ip(server.config.trust_proxy, &headers, Some(peer)));
    if let Err(wait) = server.unauth.ping.allow(&ip, Instant::now()) {
        return too_many(wait);
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    let h = r.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    h.insert("timing-allow-origin", HeaderValue::from_static("*"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

#[derive(Deserialize)]
struct NonceQuery {
    nonce: String,
}

/// The proof the Hub asks for at registration: this relay's id and key, and
/// its signature over its origin and the Hub's nonce.
async fn relay_id(
    State(server): State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(q): Query<NonceQuery>,
) -> Response {
    let ip = address_key(client_ip(server.config.trust_proxy, &headers, Some(peer)));
    if let Err(wait) = server.unauth.ping.allow(&ip, Instant::now()) {
        return too_many(wait);
    }
    let Some(nonce) = unhex(&q.nonce).and_then(|n| <[u8; 32]>::try_from(n).ok()) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "bad_request", "message": "nonce: 64 hex"}))).into_response();
    };
    let Ok(sig) = rh_proto::ticket::sign_origin_proof(&server.config.key, &server.config.origin, &nonce) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    Json(json!({
        "relay_id": server.config.id,
        "sign_key": hex(&server.config.key.verifying_key().to_bytes()),
        "origin": server.config.origin,
        "sig": hex(&sig),
    }))
    .into_response()
}

async fn connect(
    State(server): State<Arc<Server>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let ip = address_key(client_ip(server.config.trust_proxy, &headers, Some(peer)));
    if let Err(wait) = server.unauth.upgrades.allow(&ip, Instant::now()) {
        return too_many(wait);
    }
    ws.max_message_size(rh_proto::frame::MAX_FRAME_LEN)
        .max_frame_size(rh_proto::frame::MAX_FRAME_LEN)
        .on_upgrade(move |socket| {
            let tasks = server.tasks.clone();
            tasks.track_future(crate::client::connection(server, socket))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_response_forbids_webauthn() {
        for status in [
            StatusCode::OK,
            StatusCode::NO_CONTENT,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::NOT_FOUND,
        ] {
            let r = no_webauthn(status.into_response()).await;
            assert_eq!(
                r.headers()["permissions-policy"],
                "publickey-credentials-get=(), publickey-credentials-create=()",
                "{status}"
            );
        }
    }

    /// droppen.app/privacy: requests are counted per address in memory for a
    /// day at most. Here an address goes once its windows refilled — a
    /// minute after its last request at most — and not while it still counts.
    #[test]
    fn an_address_is_forgotten_once_it_is_done_counting() {
        let limits = UnauthLimits::default();
        let ip: IpAddr = "198.51.100.7".parse().unwrap();
        let t = Instant::now();
        while limits.ping.allow(&ip, t).is_ok() {}
        while limits.upgrades.allow(&ip, t).is_ok() {}
        assert_eq!(limits.forget_old(t + Duration::from_secs(10)), 0, "both still counting");
        assert_eq!((limits.ping.len(), limits.upgrades.len()), (1, 1));
        assert_eq!(limits.forget_old(t + Duration::from_secs(30)), 1, "the upgrades' windows refilled");
        assert_eq!(limits.ping.len(), 1);
        assert_eq!(limits.forget_old(t + Duration::from_secs(61)), 1, "then the pings'");
        assert!(limits.ping.is_empty() && limits.upgrades.is_empty());
    }

    #[test]
    fn the_address_comes_from_the_trusted_header_only() {
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", HeaderValue::from_static("203.0.113.9"));
        h.insert("fly-client-ip", HeaderValue::from_static("198.51.100.7"));
        let sock: SocketAddr = "127.0.0.1:5".parse().unwrap();
        assert_eq!(client_ip(TrustProxy::None, &h, Some(sock)), Some(sock.ip()));
        assert_eq!(client_ip(TrustProxy::Railway, &h, Some(sock)), Some("203.0.113.9".parse().unwrap()));
        assert_eq!(client_ip(TrustProxy::Fly, &h, Some(sock)), Some("198.51.100.7".parse().unwrap()));
        assert_eq!(client_ip(TrustProxy::Fly, &HeaderMap::new(), Some(sock)), Some(sock.ip()));
    }
}
