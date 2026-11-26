//! The errors the proxy answers itself, in each binding's form (`docs/a2a.md`
//! §3.5), as the device's own A2A server writes them: a `google.rpc.ErrorInfo`
//! of the domain `droppen.app` whose `metadata.code` is the droppen code
//! (`droppen.device_offline`, …), beside the protocol's own `ErrorInfo` when
//! the error is one of the protocol's.
//!
//! - JSON-RPC: HTTP 200, `{"jsonrpc": "2.0", "id": <the request's id>,
//!   "error": {"code": -32603, "message": …, "data": [ErrorInfo…]}}` — the id
//!   read from the request's body, `null` when it has none;
//! - HTTP+JSON (and the card): the `google.rpc.Status` JSON, `{"error":
//!   {"code": <HTTP status>, "status": …, "message": …, "details":
//!   [ErrorInfo…]}}`, as `application/a2a+json`.
//!
//! | code | JSON-RPC | HTTP | `status` |
//! |---|---|---|---|
//! | `droppen.device_offline` | -32603 | 503 | `UNAVAILABLE` |
//! | `droppen.attestation_stale` | -32603 | 400 | `FAILED_PRECONDITION` |
//! | `droppen.not_accepted` | -32603 | 403 | `PERMISSION_DENIED` |
//! | `droppen.device_removed` | -32603 | 403 | `PERMISSION_DENIED` |
//! | `droppen.quota_exceeded` (a body over 10 MiB) | -32004 | 400 | `FAILED_PRECONDITION` |
//! | none: not joined its account yet | -32603 | 503 | `UNAVAILABLE` |
//! | none (anything else) | -32603 | 500 | `INTERNAL` |

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::route::Binding;

/// The `@type` of an `ErrorInfo`.
pub const ERROR_INFO: &str = "type.googleapis.com/google.rpc.ErrorInfo";
/// The domain of a droppen code's `ErrorInfo`.
pub const DROPPEN_DOMAIN: &str = "droppen.app";
/// The domain of the protocol's own `ErrorInfo`s.
pub const A2A_DOMAIN: &str = "a2a-protocol.org";
/// HTTP+JSON's media type.
pub const A2A_JSON: &str = "application/a2a+json";

/// The droppen codes the proxy answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// The proxy cannot reach the device.
    DeviceOffline,
    /// No admin device vouched for the device's view within seven days.
    AttestationStale,
    /// The device does not accept this a2a device.
    NotAccepted,
    /// This a2a device was removed from the account.
    DeviceRemoved,
    /// A request body over the limit.
    QuotaExceeded,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::DeviceOffline => "droppen.device_offline",
            Code::AttestationStale => "droppen.attestation_stale",
            Code::NotAccepted => "droppen.not_accepted",
            Code::DeviceRemoved => "droppen.device_removed",
            Code::QuotaExceeded => "droppen.quota_exceeded",
        }
    }

    /// The `ErrorInfo`'s `reason`: the code without `droppen.`, upper snake case.
    pub fn reason(self) -> String {
        self.as_str().trim_start_matches("droppen.").to_ascii_uppercase()
    }

    /// What a person reads.
    pub fn message(self) -> &'static str {
        match self {
            Code::DeviceOffline => "the device cannot be reached now",
            Code::AttestationStale => {
                "the device's view of the account has not been confirmed in the last 7 days: open Droppen on any of your devices"
            }
            Code::NotAccepted => "the device does not accept this A2A proxy",
            Code::DeviceRemoved => "this A2A proxy was removed from the account",
            Code::QuotaExceeded => "a request body is at most 10 MiB",
        }
    }
}

/// One of the proxy's own errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyError {
    /// `None`: an error the proxy has no code for.
    pub code: Option<Code>,
    pub message: String,
    /// The proxy has not joined its account yet (503, no code).
    not_ready: bool,
}

impl ProxyError {
    pub fn new(code: Code) -> Self {
        Self {
            code: Some(code),
            message: code.message().into(),
            not_ready: false,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
            not_ready: false,
        }
    }

    /// The proxy has not joined its account yet: try again later.
    pub fn not_ready() -> Self {
        Self {
            code: None,
            message: "this A2A proxy has not joined its account yet".into(),
            not_ready: true,
        }
    }

    fn jsonrpc_code(&self) -> i64 {
        match self.code {
            Some(Code::QuotaExceeded) => -32004,
            _ => -32603,
        }
    }

    fn http(&self) -> (StatusCode, &'static str) {
        match self.code {
            Some(Code::DeviceOffline) => (StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE"),
            Some(Code::AttestationStale | Code::QuotaExceeded) => (StatusCode::BAD_REQUEST, "FAILED_PRECONDITION"),
            Some(Code::NotAccepted | Code::DeviceRemoved) => (StatusCode::FORBIDDEN, "PERMISSION_DENIED"),
            None if self.not_ready => (StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE"),
            None => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL"),
        }
    }

    /// The `ErrorInfo` list both bindings carry.
    pub fn details(&self) -> Vec<Value> {
        let mut details = Vec::new();
        if self.code == Some(Code::QuotaExceeded) {
            details.push(json!({"@type": ERROR_INFO, "reason": "UNSUPPORTED_OPERATION", "domain": A2A_DOMAIN}));
        }
        if let Some(code) = self.code {
            details.push(json!({
                "@type": ERROR_INFO,
                "reason": code.reason(),
                "domain": DROPPEN_DOMAIN,
                "metadata": {"code": code.as_str()},
            }));
        }
        details
    }

    /// The JSON-RPC answer to request `id`.
    pub fn jsonrpc(&self, id: &Value) -> Value {
        let mut error = json!({"code": self.jsonrpc_code(), "message": self.message});
        let details = self.details();
        if !details.is_empty() {
            error["data"] = Value::Array(details);
        }
        json!({"jsonrpc": "2.0", "id": id, "error": error})
    }

    /// The HTTP+JSON `google.rpc.Status` body.
    pub fn status_body(&self) -> Value {
        let (status, name) = self.http();
        json!({"error": {"code": status.as_u16(), "status": name, "message": self.message, "details": self.details()}})
    }

    /// The answer, in `binding`'s form (`id`: a JSON-RPC request's).
    pub fn response(&self, binding: Binding, id: &Value) -> Response {
        match binding {
            Binding::JsonRpc => json_response(StatusCode::OK, &self.jsonrpc(id), "application/json"),
            _ => json_response(self.http().0, &self.status_body(), A2A_JSON),
        }
    }
}

/// A JSON-RPC request's `id`, read from its body: a string, a number or
/// `null`; `null` for a body that is not a request.
pub fn jsonrpc_id(body: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(request)) => match request.get("id") {
            Some(id @ (Value::String(_) | Value::Number(_))) => id.clone(),
            _ => Value::Null,
        },
        _ => Value::Null,
    }
}

/// `body` under `media_type`.
pub fn json_response(status: StatusCode, body: &Value, media_type: &'static str) -> Response {
    let mut response = (status, body.to_string()).into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    response
}

/// The answer to a request without the proxy's token: the same for every
/// path, so it says nothing about which devices or entries exist.
pub fn unauthenticated() -> Response {
    let body = json!({"error": {"code": 401, "status": "UNAUTHENTICATED", "message": "the proxy's token is required: Authorization: Bearer <token>", "details": []}});
    let mut response = json_response(StatusCode::UNAUTHORIZED, &body, "application/json");
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"droppen-a2a-proxy\""));
    response
}

/// The answer for a path that is no route.
pub fn not_found() -> Response {
    let body = json!({"error": {"code": 404, "status": "NOT_FOUND", "message": "no such agent", "details": []}});
    json_response(StatusCode::NOT_FOUND, &body, "application/json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_error_reads_the_same_in_both_bindings() {
        let error = ProxyError::new(Code::DeviceOffline);
        let rpc = error.jsonrpc(&json!(7));
        assert_eq!(
            rpc,
            json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32603, "message": "the device cannot be reached now", "data": [
                {"@type": ERROR_INFO, "reason": "DEVICE_OFFLINE", "domain": "droppen.app", "metadata": {"code": "droppen.device_offline"}},
            ]}})
        );
        let status = error.status_body();
        assert_eq!(status["error"]["code"], 503);
        assert_eq!(status["error"]["status"], "UNAVAILABLE");
        assert_eq!(status["error"]["details"], rpc["error"]["data"], "the same details in both");
        for (code, http, name) in [
            (Code::AttestationStale, 400, "FAILED_PRECONDITION"),
            (Code::NotAccepted, 403, "PERMISSION_DENIED"),
            (Code::DeviceRemoved, 403, "PERMISSION_DENIED"),
        ] {
            let body = ProxyError::new(code).status_body();
            assert_eq!(
                (body["error"]["code"].as_u64(), body["error"]["status"].as_str()),
                (Some(http), Some(name)),
                "{code:?}"
            );
            assert_eq!(body["error"]["details"][0]["metadata"]["code"], code.as_str());
        }
        let quota = ProxyError::new(Code::QuotaExceeded).jsonrpc(&Value::Null);
        assert_eq!(quota["error"]["code"], -32004);
        assert_eq!(quota["error"]["data"][0]["reason"], "UNSUPPORTED_OPERATION");
        assert_eq!(quota["error"]["data"][1]["metadata"]["code"], "droppen.quota_exceeded");
        let internal = ProxyError::internal("the device chain forked").jsonrpc(&json!("a"));
        assert_eq!(internal["error"], json!({"code": -32603, "message": "the device chain forked"}));
    }

    #[test]
    fn a_json_rpc_errors_id_is_the_requests() {
        assert_eq!(jsonrpc_id(br#"{"jsonrpc":"2.0","id":"req-1","method":"SendMessage"}"#), json!("req-1"));
        assert_eq!(jsonrpc_id(br#"{"jsonrpc":"2.0","id":42,"method":"GetTask"}"#), json!(42));
        assert_eq!(jsonrpc_id(br#"{"jsonrpc":"2.0","id":{"x":1}}"#), Value::Null);
        assert_eq!(jsonrpc_id(b"[1,2]"), Value::Null);
        assert_eq!(jsonrpc_id(b"not json"), Value::Null);
    }
}
