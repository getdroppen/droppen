//! The errors the proxy answers itself, as plain JSON `{"error": code,
//! "message": …}`.
//!
//! The statuses are a contract with the backend and never 500 for a channel
//! that did not carry a request:
//!
//! | code | HTTP | when |
//! |---|---|---|
//! | `host_not_accepting` | 403 | the computer does not take control from this proxy, or its view lapsed (`why` says) |
//! | `host_offline` | 503 | the computer is not reachable now |
//! | `host_stale` | 503 | no admin confirmed the account's view in 7 days |
//! | `device_removed` | 410 | the proxy was removed from the account |
//! | `channel_failed` | 502 | the channel broke in a way none of the above explains |
//! | `unknown_session` | 404 | the id is not one this proxy started |
//! | `not_ready` | 503 | the proxy has not joined its account yet |
//! | `unauthorized` | 401 | no token, or not the proxy's |
//! | `not_found` | 404 | no such route |
//! | `bad_request` | 400 | the body is not what the route takes |
//! | `answer_too_large` | 413 | a buffered answer over 64 MiB |

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Why a request did not get through, as the backend reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The computer does not accept control from this proxy (or its view has
    /// lapsed): `why` is the far end's or the chain's word.
    HostNotAccepting { why: String },
    /// The computer cannot be reached now.
    HostOffline,
    /// No admin vouched for the account's view within seven days.
    HostStale,
    /// The proxy was removed from the account.
    DeviceRemoved,
    /// The channel failed in a way none of the above explains (a fork, a
    /// protocol error).
    ChannelFailed,
}

impl Refusal {
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::HostNotAccepting { .. } => "host_not_accepting",
            Refusal::HostOffline => "host_offline",
            Refusal::HostStale => "host_stale",
            Refusal::DeviceRemoved => "device_removed",
            Refusal::ChannelFailed => "channel_failed",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Refusal::HostNotAccepting { .. } => StatusCode::FORBIDDEN,
            Refusal::HostOffline | Refusal::HostStale => StatusCode::SERVICE_UNAVAILABLE,
            Refusal::DeviceRemoved => StatusCode::GONE,
            Refusal::ChannelFailed => StatusCode::BAD_GATEWAY,
        }
    }

    pub fn message(&self) -> std::borrow::Cow<'static, str> {
        match self {
            Refusal::HostNotAccepting { why } => format!("the computer does not take control from this proxy: {why}").into(),
            Refusal::HostOffline => "the computer cannot be reached now".into(),
            Refusal::HostStale => {
                "the computer's view of the account has not been confirmed in the last 7 days: open Droppen on any of your devices".into()
            }
            Refusal::DeviceRemoved => "this proxy was removed from the account".into(),
            Refusal::ChannelFailed => "the channel to the computer failed".into(),
        }
    }

    /// The `why` a refusal carries, when it has one.
    pub fn why(&self) -> Option<&str> {
        match self {
            Refusal::HostNotAccepting { why } => Some(why),
            _ => None,
        }
    }

    pub fn response(&self) -> Response {
        let mut body = json!({"error": self.code(), "message": self.message()});
        if let Some(why) = self.why() {
            body["why"] = json!(why);
        }
        json_response(self.status(), &body)
    }
}

/// A `body` under `content-type`, with the status.
pub fn json_response(status: StatusCode, body: &serde_json::Value) -> Response {
    let mut response = (status, body.to_string()).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// The same 401 for every protected path, so it says nothing about which
/// devices or sessions exist.
pub fn unauthorized() -> Response {
    let mut response = json_response(
        StatusCode::UNAUTHORIZED,
        &json!({"error": "unauthorized", "message": "the proxy's token is required: Authorization: Bearer <token>"}),
    );
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"droppen-session-proxy\""));
    response
}

/// The proxy has not joined its account yet.
pub fn not_ready() -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        &json!({"error": "not_ready", "message": "this proxy has not joined its account yet"}),
    )
}

/// The path is no route.
pub fn not_found() -> Response {
    json_response(StatusCode::NOT_FOUND, &json!({"error": "not_found", "message": "no such route"}))
}

/// A session id the registry does not hold.
pub fn unknown_session() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        &json!({"error": "unknown_session", "message": "no session of this proxy has that id"}),
    )
}

/// The body is not what the route takes.
pub fn bad_request(message: &str) -> Response {
    json_response(StatusCode::BAD_REQUEST, &json!({"error": "bad_request", "message": message}))
}

/// A buffered answer past the limit.
pub fn answer_too_large(max: usize) -> Response {
    json_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        &json!({"error": "answer_too_large", "message": format!("the device's answer is larger than {max} bytes")}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_refusal_has_its_status_and_code() {
        for (refusal, status, code) in [
            (Refusal::HostNotAccepting { why: "x".into() }, StatusCode::FORBIDDEN, "host_not_accepting"),
            (Refusal::HostOffline, StatusCode::SERVICE_UNAVAILABLE, "host_offline"),
            (Refusal::HostStale, StatusCode::SERVICE_UNAVAILABLE, "host_stale"),
            (Refusal::DeviceRemoved, StatusCode::GONE, "device_removed"),
            (Refusal::ChannelFailed, StatusCode::BAD_GATEWAY, "channel_failed"),
        ] {
            assert_eq!((refusal.status(), refusal.code()), (status, code));
        }
    }
}
