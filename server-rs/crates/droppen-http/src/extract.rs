//! Extractors that answer a malformed request the way the TypeScript server does.
//!
//! axum's own rejections are `text/plain`, and the status depends on how the
//! body failed: `422` for a well-formed JSON document of the wrong shape, `415`
//! for a missing content type, `400` for a path segment that will not parse.
//! `@hono/zod-validator` answers **every** one of those with
//! `c.json(result, 400)` (`@hono/zod-validator/dist/index.js:24`), and the web
//! client reads `.error` off a failed response.
//!
//! So each extractor here wraps its axum counterpart, keeps the same OpenAPI
//! contribution by delegating [`ParameterProvider`], and turns any rejection
//! into an [`ApiError`] — a `400` whose body is `{ "error": … }`.
//!
//! They are drop-in: `Json` → `ApiJson`, `Path` → `ApiPath`, `Query` → `ApiQuery`.

use gotcha::axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use gotcha::axum::extract::{FromRequest, FromRequestParts, Request};
use gotcha::axum::http::request::Parts;
use gotcha::oas::{Parameter, RequestBody};
use gotcha::{Either, Json, ParameterProvider, Path, Query};
use serde::de::DeserializeOwned;

use super::error::ApiError;

/// A JSON body whose rejection is a `400 {"error": …}`.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(ApiError::bad_request(body_text(&rejection))),
        }
    }
}

impl<T> ParameterProvider for ApiJson<T>
where
    Json<T>: ParameterProvider,
{
    fn generate(url: String) -> Either<Vec<Parameter>, RequestBody> {
        <Json<T> as ParameterProvider>::generate(url)
    }
}

/// Path segments whose rejection is a `400 {"error": …}`.
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(ApiPath(value)),
            Err(rejection) => Err(ApiError::bad_request(body_text(&rejection))),
        }
    }
}

impl<T> ParameterProvider for ApiPath<T>
where
    Path<T>: ParameterProvider,
{
    fn generate(url: String) -> Either<Vec<Parameter>, RequestBody> {
        <Path<T> as ParameterProvider>::generate(url)
    }
}

/// A query string whose rejection is a `400 {"error": …}`.
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(ApiQuery(value)),
            Err(rejection) => Err(ApiError::bad_request(body_text(&rejection))),
        }
    }
}

impl<T> ParameterProvider for ApiQuery<T>
where
    Query<T>: ParameterProvider,
{
    fn generate(url: String) -> Either<Vec<Parameter>, RequestBody> {
        <Query<T> as ParameterProvider>::generate(url)
    }
}

/// What went wrong, in one line, without the status axum would have used.
trait RejectionText {
    fn text(&self) -> String;
}

macro_rules! rejection_text {
    ($($rejection:ty),+) => {$(
        impl RejectionText for $rejection {
            fn text(&self) -> String {
                self.body_text()
            }
        }
    )+};
}
rejection_text!(JsonRejection, PathRejection, QueryRejection);

fn body_text(rejection: &impl RejectionText) -> String {
    // axum's text is multi-line for some rejections; the API's `error` is one.
    rejection.text().replace('\n', " ")
}
