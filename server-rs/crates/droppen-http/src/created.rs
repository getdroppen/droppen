//! Success statuses other than `200`, which gotcha cannot derive: `201
//! Created` and `202 Accepted`.
//!
//! Each wrapper borrows `T`'s schema — the documented response is right about
//! the *body*, wrong only about the status — and
//! [`ExtraResponses::restatus`](crate::openapi_ext::ExtraResponses::restatus)
//! moves the entry to its real status. At runtime the [`IntoResponse`] impl
//! below sends that status.

use gotcha::axum::http::StatusCode;
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::oas::Responses;
use gotcha::response::response as response_contract;
use gotcha::{Json, Responsible, Schematic};
use gotcha_core::EnhancedSchema;
use serde::Serialize;

/// One success wrapper: its body sent with `$code`, and a `Schematic` that
/// delegates to `T` so the generated body schema is `T`'s and the entry
/// [`ExtraResponses::restatus`](crate::openapi_ext::ExtraResponses::restatus)
/// moves it into needs no schema of its own.
macro_rules! status_body {
    ($(#[$doc:meta])* $name:ident, $code:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone)]
        pub struct $name<T>(pub T);

        impl<T: Serialize> IntoResponse for $name<T> {
            fn into_response(self) -> Response {
                (StatusCode::$code, Json(self.0)).into_response()
            }
        }

        /// The `200` the wrapper'd body would document on its own;
        /// [`ExtraResponses::restatus`](crate::openapi_ext::ExtraResponses::restatus)
        /// moves it to the status this wrapper really sends.
        impl<T: Schematic> Responsible for $name<T> {
            fn response() -> Responses {
                response_contract::<T>(200, "application/json", "default return")
            }
        }

        impl<T: Schematic> Schematic for $name<T> {
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
    };
}

status_body!(
    /// A body that is sent with `201 Created`.
    Created,
    CREATED
);
status_body!(
    /// A body that is sent with `202 Accepted` (enrolling: a device of the
    /// account completes it).
    Accepted,
    ACCEPTED
);
