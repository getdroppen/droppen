//! The desktop app's own window, and the gate that tells a request from it
//! apart from every other caller.
//!
//! The routes it guards are in more than one subsystem: the first-run guide's
//! installs and sign-ins (the server's `setup`) and Claude Code's setup-token wizard
//! (the server's `accounts::setup_token`). So the gate sits in the HTTP kit, below
//! both.

use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use gotcha::axum::extract::FromRequestParts;
use gotcha::axum::http::request::Parts;
use gotcha::axum::http::{header, StatusCode};

use super::error::ApiError;
use super::identity::AuthIdentity;

/// Who sent a request and what it says about where it was made: what the
/// gate on the app-only routes reads ([`AppWindow::app_caller`]).
///
/// Annotate the parameter `#[api(skip)]`: it contributes nothing to the
/// OpenAPI document.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The peer's address, when the listener recorded one (the server's `app`
    /// serves with connect info; a router driven without a socket has none).
    pub peer: Option<SocketAddr>,
    /// Who it is signed in as, if anyone.
    pub identity: Option<AuthIdentity>,
    /// Its `Origin` header.
    pub origin: Option<String>,
    /// Its `Host` header (or the request target's authority).
    pub host: Option<String>,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let text = |name| parts.headers.get(name).and_then(|value: &header::HeaderValue| value.to_str().ok()).map(str::to_string);
        Ok(Caller {
            peer: parts
                .extensions
                .get::<gotcha::axum::extract::ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
            identity: parts.extensions.get::<AuthIdentity>().cloned(),
            origin: text(header::ORIGIN),
            host: text(header::HOST).or_else(|| parts.uri.authority().map(|authority| authority.to_string())),
        })
    }
}

/// The port a server records once its listener is bound, for the gate below.
///
/// A handle rather than the shared cell itself: every request builds a window,
/// and a window that could write the cell would let a request make itself look
/// like the app's own ([`AppWindow::from_app_window`]). Only the server's
/// entry, which binds the listener, writes it.
#[derive(Clone)]
pub struct ListenPort(Arc<OnceLock<u16>>);

impl ListenPort {
    /// A handle on `cell`, which the server fills once.
    pub fn new(cell: Arc<OnceLock<u16>>) -> Self {
        Self(cell)
    }

    /// The port, once it is recorded.
    pub fn get(&self) -> Option<&u16> {
        self.0.get()
    }
}

/// The desktop app's window as this server knows it: the port the server
/// listens on, and whether the app runs it.
///
/// Made for each request from the server's state. The port cell is the one
/// the server fills once its listener is bound, shared rather than copied, so
/// a request made after that sees the port.
#[derive(Clone)]
pub struct AppWindow {
    /// The port this server listens on, once it does.
    port: ListenPort,
    /// Whether the Droppen desktop app runs this server.
    desktop_app: bool,
}

impl AppWindow {
    /// The window of a server whose listen port is recorded in `port`, run by
    /// the desktop app when `desktop_app` is set.
    pub fn new(port: ListenPort, desktop_app: bool) -> Self {
        Self { port, desktop_app }
    }

    /// Whether a request is the desktop app's own window talking to its own
    /// server. Every one of these must hold:
    ///
    /// * the app runs this server;
    /// * the peer is this device (loopback);
    /// * the caller is signed in — the app's window always is, with the login the
    ///   app owns — and is not a device linked through the Droppen bridge
    ///   (`cloud:<device>`, which also arrives over loopback);
    /// * it was addressed to `127.0.0.1:<this server's port>` (`Host`) and, when it
    ///   says where it was made (`Origin`), was made by a page at exactly
    ///   `http://127.0.0.1:<port>`, the address the app opens its window at.
    ///
    /// The last one is what loopback alone cannot tell: a page on another site
    /// whose name was re-pointed at 127.0.0.1 (DNS rebinding) names its own host,
    /// and so does a browser elsewhere reaching this port through a tunnel or a
    /// reverse proxy (`tailscale serve`, `cloudflared`, `ssh -R`), whose requests
    /// arrive over loopback too. `require_origin` is for the routes that act: a
    /// browser always sends `Origin` on a `POST` from a page.
    fn from_app_window(&self, caller: &Caller, require_origin: bool) -> bool {
        let Some(port) = self.port.get().copied() else {
            return false;
        };
        let local = caller.peer.is_some_and(|peer| match peer.ip() {
            std::net::IpAddr::V4(ip) => ip.is_loopback(),
            std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or(ip.is_loopback(), |ip| ip.is_loopback()),
        });
        let signed_in = caller
            .identity
            .as_ref()
            .is_some_and(|identity| !identity.username.starts_with("cloud:"));
        let own = format!("127.0.0.1:{port}");
        let addressed_here = caller.host.as_deref() == Some(own.as_str());
        let made_here = match caller.origin.as_deref() {
            Some(origin) => origin == format!("http://{own}"),
            None => !require_origin,
        };
        self.desktop_app && local && signed_in && addressed_here && made_here
    }

    /// Whether this caller may install and sign in from here — what the tools
    /// report says as `canInstall`. A page's own `GET` carries no `Origin`, so
    /// one is not required here; the routes that act require it ([`Self::app_caller`]).
    pub fn local_app_request(&self, caller: &Caller) -> bool {
        self.from_app_window(caller, false)
    }

    /// The gate on every route that installs, updates, signs in or rewrites the
    /// `PATH`: `403 {"error":"app_only"}` unless the request is the app's own
    /// window, `Origin` included ([`Self::from_app_window`]).
    ///
    /// A browser on another device — a phone, a laptop on the LAN, a device linked
    /// through Droppen, a page reaching this port through a tunnel — must not be
    /// able to start an installer on this one. What this cannot yet tell apart is
    /// the app's window from another browser on the same Mac signed in to the same
    /// server at the same address. The local web access work adds the
    /// app-internal account (`user add --app`, `droppen-app`) the app signs in
    /// with, and once it exists this check also requires that identity.
    pub fn app_caller(&self, caller: &Caller) -> Result<(), ApiError> {
        if self.from_app_window(caller, true) {
            Ok(())
        } else {
            Err(ApiError::new(StatusCode::FORBIDDEN, "app_only"))
        }
    }
}
