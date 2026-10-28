//! Who a request is, as the authentication guard decided it.
//!
//! The type sits in the HTTP kit rather than with the guard (`crate::auth`)
//! because code that may not depend on the guard reads it too: the
//! desktop-window gate (`app_window`) asks whether a request's caller is
//! signed in.

/// Who a request is, once the guard has decided.
///
/// Put in the request extensions by the guard (`crate::auth::guard`) and read
/// back out by its `Identity` extractor, so a handler never parses a header
/// itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthIdentity {
    /// The account name.
    pub username: String,
    /// The digest of the session token this request carried, when it carried
    /// one — or, for a request authorized by a ticket, of the session that
    /// minted the ticket (`None` for a ticket no session minted).
    pub token_sha256: Option<String>,
}
