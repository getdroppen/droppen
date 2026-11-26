//! Who a request is, as the authentication guard decided it.
//!
//! The type sits in the HTTP kit rather than with the guard (the server's `auth`)
//! because code that may not depend on the guard reads it too: the
//! desktop-window gate (`app_window`) asks whether a request's caller is
//! signed in. [`NotifyGrant`] sits here for the same reason: the guard inserts
//! it and notify reads it back, and neither may name the other. So does
//! [`BridgedDevice`], which the A2A server reads.

use std::any::Any;
use std::sync::Arc;

/// Who a request is, once the guard has decided.
///
/// Put in the request extensions by the guard (the server's `auth::guard`) and read
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

/// The device a request was bridged from (`crate::bridge`): the device id
/// the agent's end-to-end channel was authorised for, and whether that device
/// is an A2A proxy (`docs/a2a.md` §7.3).
///
/// Put in the request extensions by the guard (the server's `auth::guard`)
/// for every bridged request, beside the [`AuthIdentity`] `cloud:<id>` of any
/// other device; an A2A proxy gets this alone — it is no login, and reaches
/// nothing but `/a2a/*` — so code that asks which device a request came
/// from, or whether it is an A2A proxy, reads this rather than a string
/// prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgedDevice {
    /// Its device id, as the account's device chain names it.
    pub id: String,
    /// It is an A2A proxy: its channel was admitted for A2A calls only.
    pub a2a: bool,
}

/// What the notify capability proved about a request, which the guard (the
/// server's `auth::guard`) carries to the notify handler in the request
/// extensions.
///
/// The one type the guard inserts for that capability: what it holds is
/// opaque to the guard, so whatever the capability's check proves stays
/// inside it and can never stand in the extensions as an [`AuthIdentity`] or
/// anything else a handler reads. Only code that can name the type it holds
/// reads it back ([`NotifyGrant::get`]).
#[derive(Clone)]
pub struct NotifyGrant(Arc<dyn Any + Send + Sync>);

impl NotifyGrant {
    /// A grant holding what the capability proved.
    pub fn new<T: Any + Send + Sync>(proof: T) -> Self {
        Self(Arc::new(proof))
    }

    /// What the grant holds, if it is a `T`.
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}
