//! The API's error body and its response mapping.
//!
//! `GotchaError` renders every failure as a bare `500` with a text body
//! (`gotcha-0.4.1/src/error.rs:57`), which neither carries a status nor matches
//! the `{ "error": string }` shape the web client reads. So the API brings its
//! own.

use gotcha::axum::http::StatusCode;
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::{Json, Schematic};
use gotcha_core::EnhancedSchema;
use serde::{Deserialize, Serialize};

/// The error body every failing route returns.
///
/// Named `Error` because that is the schema name in `openapi/openapi.json` —
/// `Schematic::name()` is the Rust identifier, so the two have to agree.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct Error {
    /// Human-readable failure text.
    pub error: String,
}

/// A failure with the status code to render it at.
///
/// The body is held as JSON rather than as [`Error`] because a few routes
/// answer with a widened shape — `POST /api/accounts` returns
/// `{ "error": "duplicate_account", "existingLabel": … }` — and the TypeScript
/// bodies are the contract.
#[derive(Debug, Clone)]
pub struct ApiError {
    status: StatusCode,
    body: serde_json::Value,
}

impl ApiError {
    /// `{ "error": message }` at the given status.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: serde_json::json!({ "error": message.into() }),
        }
    }

    /// A body that carries more than `error`, at the given status.
    pub fn with_body(status: StatusCode, body: serde_json::Value) -> Self {
        Self { status, body }
    }

    /// `400` with the given message.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// `404` with the given message.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    /// `409` with the given message.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    /// `500` with the given message.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// The status this renders at.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The body this renders.
    pub fn body(&self) -> &serde_json::Value {
        &self.body
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// Delegates to [`Error`] so a `Result<_, ApiError>` handler documents the same
/// schema the `404` entry in [`crate::openapi_ext`] registers.
impl Schematic for ApiError {
    fn name() -> &'static str {
        <Error as Schematic>::name()
    }

    fn required() -> bool {
        <Error as Schematic>::required()
    }

    fn type_() -> &'static str {
        <Error as Schematic>::type_()
    }

    fn doc() -> Option<String> {
        <Error as Schematic>::doc()
    }

    fn fields() -> Vec<(&'static str, EnhancedSchema)> {
        <Error as Schematic>::fields()
    }

    fn generate_schema() -> EnhancedSchema {
        <Error as Schematic>::generate_schema()
    }
}
