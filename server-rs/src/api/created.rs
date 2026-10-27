//! A `201 Created` success, which gotcha cannot derive.
//!
//! Every success gotcha documents is a `200`
//! (`gotcha_core-0.4.1/src/responsible.rs:25`), and `Responsible` carries a
//! blanket `impl<T: Schematic> Responsible for T` that a downstream crate cannot
//! add to. So [`Created`] borrows `T`'s schema — the derived response is right
//! about the *body*, wrong only about the status — and
//! [`ExtraResponses::restatus`](crate::openapi_ext::ExtraResponses::restatus)
//! moves the entry to `201`. At runtime the
//! [`IntoResponse`] impl below sends the real status.

use gotcha::axum::http::StatusCode;
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::{Json, Schematic};
use gotcha_core::EnhancedSchema;
use serde::Serialize;

/// A body that is sent with `201 Created`.
#[derive(Debug, Clone)]
pub struct Created<T>(pub T);

impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (StatusCode::CREATED, Json(self.0)).into_response()
    }
}

/// Delegates to `T`, so the generated body schema is `T`'s and the `201` entry
/// [`ExtraResponses::restatus`](crate::openapi_ext::ExtraResponses::restatus)
/// moves it into needs no schema of its own.
impl<T: Schematic> Schematic for Created<T> {
    fn name() -> &'static str {
        T::name()
    }

    fn required() -> bool {
        T::required()
    }

    fn nullable() -> Option<bool> {
        T::nullable()
    }

    fn type_() -> &'static str {
        T::type_()
    }

    fn doc() -> Option<String> {
        T::doc()
    }

    fn fields() -> Vec<(&'static str, EnhancedSchema)> {
        T::fields()
    }

    fn generate_schema() -> EnhancedSchema {
        T::generate_schema()
    }
}
