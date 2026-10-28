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

use std::time::Duration;

/// Whether an endpoint is somewhere this server is willing to send to.
///
/// HTTPS always; plain HTTP only for loopback, and only when the caller's
/// channel opted in. Parsing the endpoint rather than prefix-matching it is
/// what makes `https://` and an unknown scheme fall on the right side.
pub fn endpoint_is_acceptable(endpoint: &str, insecure_endpoints_allowed: bool) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    match url.scheme() {
        "https" => true,
        "http" if insecure_endpoints_allowed => {
            matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1" | "[::1]"))
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
/// `version` is the server's own (its `CARGO_PKG_VERSION`), which the user
/// agent `remote-hand/<version>` carries: this crate's version is not the
/// release's.
pub fn no_redirect_client(timeout: Duration, version: &str) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("remote-hand/{version}"))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()
}
