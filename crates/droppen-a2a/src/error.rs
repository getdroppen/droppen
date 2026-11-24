//! The errors of the protocol, and how each binding writes them.
//!
//! One [`A2aError`] is written two ways (v1.0 §5.4, §9.5, §11.6):
//!
//! * JSON-RPC: `{"code": <-32xxx>, "message": …, "data": [ErrorInfo…]}`;
//! * HTTP+JSON: the `google.rpc.Status` JSON, `{"error": {"code": <HTTP
//!   status>, "status": "NOT_FOUND", "message": …, "details": [ErrorInfo…]}}`.
//!
//! The details are the same list in both: a `google.rpc.ErrorInfo` of the
//! domain `a2a-protocol.org` for an A2A error, and, for a droppen one, a second
//! `ErrorInfo` of the domain `droppen.app` whose `metadata.code` is the code
//! (`docs/a2a.md` §3.5).

use gotcha::axum::http::{header, HeaderValue, StatusCode};
use gotcha::axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::backend::ErrorCode;

/// The `@type` of an `ErrorInfo` detail.
pub const ERROR_INFO: &str = "type.googleapis.com/google.rpc.ErrorInfo";

/// The domain of the protocol's own `ErrorInfo`s.
pub const A2A_DOMAIN: &str = "a2a-protocol.org";

/// The domain of a droppen code's `ErrorInfo`.
pub const DROPPEN_DOMAIN: &str = "droppen.app";

/// The media type of every HTTP+JSON answer (v1.0.1).
pub const A2A_JSON: &str = "application/a2a+json";

/// Which error, as the protocol names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The body is not JSON.
    Parse,
    /// The body is JSON but not a request.
    InvalidRequest,
    /// No such method, or no such REST path.
    MethodNotFound,
    /// The parameters are wrong.
    InvalidParams,
    /// Something went wrong here.
    Internal,
    /// No such task, or not this caller's.
    TaskNotFound,
    /// The task is finished already.
    TaskNotCancelable,
    /// What was asked is not done (here, or for a task in that state).
    UnsupportedOperation,
    /// A part's media type is not one the card lists.
    ContentTypeNotSupported,
    /// The `A2A-Version` is not 1.0.
    VersionNotSupported,
}

impl ErrorKind {
    /// The JSON-RPC code.
    pub fn jsonrpc_code(self) -> i64 {
        match self {
            ErrorKind::Parse => -32700,
            ErrorKind::InvalidRequest => -32600,
            ErrorKind::MethodNotFound => -32601,
            ErrorKind::InvalidParams => -32602,
            ErrorKind::Internal => -32603,
            ErrorKind::TaskNotFound => -32001,
            ErrorKind::TaskNotCancelable => -32002,
            ErrorKind::UnsupportedOperation => -32004,
            ErrorKind::ContentTypeNotSupported => -32005,
            ErrorKind::VersionNotSupported => -32009,
        }
    }

    /// The HTTP status of the HTTP+JSON binding.
    pub fn http_status(self) -> StatusCode {
        match self {
            ErrorKind::TaskNotFound | ErrorKind::MethodNotFound => StatusCode::NOT_FOUND,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    /// The `google.rpc.Code` name the `Status` carries.
    fn rpc_status(self) -> &'static str {
        match self {
            ErrorKind::TaskNotFound | ErrorKind::MethodNotFound => "NOT_FOUND",
            ErrorKind::Internal => "INTERNAL",
            ErrorKind::TaskNotCancelable | ErrorKind::UnsupportedOperation | ErrorKind::VersionNotSupported => "FAILED_PRECONDITION",
            ErrorKind::Parse | ErrorKind::InvalidRequest | ErrorKind::InvalidParams | ErrorKind::ContentTypeNotSupported => "INVALID_ARGUMENT",
        }
    }

    /// The `reason` of the protocol's `ErrorInfo`, for an A2A error.
    fn reason(self) -> Option<&'static str> {
        match self {
            ErrorKind::TaskNotFound => Some("TASK_NOT_FOUND"),
            ErrorKind::TaskNotCancelable => Some("TASK_NOT_CANCELABLE"),
            ErrorKind::UnsupportedOperation => Some("UNSUPPORTED_OPERATION"),
            ErrorKind::ContentTypeNotSupported => Some("CONTENT_TYPE_NOT_SUPPORTED"),
            ErrorKind::VersionNotSupported => Some("VERSION_NOT_SUPPORTED"),
            _ => None,
        }
    }
}

/// An error of the protocol, with the droppen code that refines it, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A2aError {
    /// Which error.
    pub kind: ErrorKind,
    /// What a person reads.
    pub message: String,
    /// The droppen code, for an error a droppen rule caused.
    pub code: Option<ErrorCode>,
}

impl A2aError {
    /// An error of `kind`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: None,
        }
    }

    /// This error, refined by a droppen code.
    pub fn with_code(mut self, code: ErrorCode) -> Self {
        self.code = Some(code);
        self
    }

    /// `TaskNotFoundError`, for a task that does not exist and for one this
    /// caller may not see alike.
    pub fn task_not_found() -> Self {
        Self::new(ErrorKind::TaskNotFound, "the task does not exist or is not accessible")
    }

    /// `InvalidParamsError`.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidParams, message)
    }

    /// `UnsupportedOperationError`.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::UnsupportedOperation, message)
    }

    /// `InternalError`.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// The `ErrorInfo` list both bindings carry.
    pub fn details(&self) -> Vec<Value> {
        let mut details = Vec::new();
        if let Some(reason) = self.kind.reason() {
            details.push(json!({"@type": ERROR_INFO, "reason": reason, "domain": A2A_DOMAIN}));
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

    /// The JSON-RPC `error` object.
    pub fn jsonrpc(&self) -> Value {
        let mut error = json!({"code": self.kind.jsonrpc_code(), "message": self.message});
        let details = self.details();
        if !details.is_empty() {
            error["data"] = Value::Array(details);
        }
        error
    }

    /// The HTTP+JSON `google.rpc.Status` body.
    pub fn status_body(&self) -> Value {
        json!({"error": {
            "code": self.kind.http_status().as_u16(),
            "status": self.kind.rpc_status(),
            "message": self.message,
            "details": self.details(),
        }})
    }

    /// The HTTP+JSON answer.
    pub fn rest_response(&self) -> Response {
        a2a_json(self.kind.http_status(), &self.status_body())
    }
}

impl std::fmt::Display for A2aError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// `body` as `application/a2a+json`.
pub fn a2a_json(status: StatusCode, body: &Value) -> Response {
    json_as(status, body, A2A_JSON)
}

/// `body`, serialized, under `media_type`.
pub fn json_as(status: StatusCode, body: &Value, media_type: &'static str) -> Response {
    let mut response = (status, body.to_string()).into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    response
}

/// The answer to a request that proved no A2A token: the same for every path,
/// so it says nothing about which entries exist.
pub fn unauthenticated() -> Response {
    let body = json!({"error": {
        "code": 401,
        "status": "UNAUTHENTICATED",
        "message": "an A2A token is required: Authorization: Bearer dpa_…",
        "details": [],
    }});
    let mut response = json_as(StatusCode::UNAUTHORIZED, &body, "application/json");
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer realm=\"droppen-a2a\""));
    response
}

/// The answer for an agent that does not exist, and for one the token may not
/// reach: the two are not told apart.
pub fn no_such_agent() -> Response {
    let body = json!({"error": {
        "code": 404,
        "status": "NOT_FOUND",
        "message": "no such agent",
        "details": [],
    }});
    json_as(StatusCode::NOT_FOUND, &body, "application/json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_droppen_code_rides_beside_the_protocols_own_error_info() {
        let error = A2aError::unsupported("no answers over A2A").with_code(ErrorCode::RemoteAnswersDisabled);
        assert_eq!(
            error.jsonrpc(),
            json!({"code": -32004, "message": "no answers over A2A", "data": [
                {"@type": ERROR_INFO, "reason": "UNSUPPORTED_OPERATION", "domain": "a2a-protocol.org"},
                {"@type": ERROR_INFO, "reason": "REMOTE_ANSWERS_DISABLED", "domain": "droppen.app", "metadata": {"code": "droppen.remote_answers_disabled"}},
            ]})
        );
        let status = error.status_body();
        assert_eq!(status["error"]["code"], 400);
        assert_eq!(status["error"]["status"], "FAILED_PRECONDITION");
        assert_eq!(status["error"]["details"], error.jsonrpc()["data"], "the same details in both bindings");
    }

    #[test]
    fn a_plain_json_rpc_error_has_no_data() {
        let error = A2aError::invalid("message is required");
        assert_eq!(error.jsonrpc(), json!({"code": -32602, "message": "message is required"}));
        assert_eq!(error.status_body()["error"]["details"], json!([]));
    }
}
