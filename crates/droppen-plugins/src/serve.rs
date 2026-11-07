//! `GET|HEAD /plugins/<key>/<id>/<build>/<path>`: a plugin's code, to the
//! pages this server serves.
//!
//! # Why the key is a path segment
//!
//! A page cannot put its bearer token on an `import()`, and a query string
//! would not survive the plugin's own relative chunk imports (`./tasks-page-
//! ….js`), so the credential is part of the path. `<key>` is the instance-wide
//! secret [`crate::Plugins`] keeps in `state.db`: 128 random bits as hex, made
//! on first need and stable across restarts. It reaches a page only inside
//! `PluginStatus.entry`, which only a signed-in, non-cloud caller is given
//! (`GET /api/plugins`, the feed's `plugins` frame); [`super::Plugins::asset`]
//! compares it in constant time.
//!
//! # How this sits with the authentication guard
//!
//! Deliberately on the guard's *static* side: `droppen_auth::exempt` lets every
//! `GET`/`HEAD` outside `/api/` through without a session, because the shell
//! and its assets must load before anyone can type a password. This route
//! cannot ask for a session — `import()` has no place to put a header — so it
//! is the path's own capability that authenticates it, exactly once per
//! plugin build. The guard still runs, and still resolves a bridged identity
//! when a request carries the agent's headers; the plugin router's cloud
//! layer refuses that identity before this handler runs, and the handler
//! refuses it again, so a request that came in over droppen.app gets nothing
//! even with the right key. The secret never leaves this server's own pages:
//! it is given to `/api/plugins` callers and to feed subscribers, the feed's
//! per-subscriber filter (`droppen_drive::feed::global`, which hands a `cloud:` subscriber
//! `plugins: []`) keeps it out of the cloud's frames, and `agent::bridge`
//! refuses plugin paths for a device before they are ever forwarded here.
//!
//! # What is answered
//!
//! Only for an installed plugin that is enabled and compatible, at its current
//! build, for a regular file inside its package directory. Everything else is
//! `404` — wrong key, unknown id, an old build, a plugin that is off, a path
//! out of the package, a directory — with a body that says nothing about which
//! of those it was. `GET` and `HEAD` only.

use gotcha::axum::body::Body;
use gotcha::axum::http::{header, HeaderValue, Response, StatusCode};
use gotcha::State;

use super::routes::PluginsState;
use super::CloudIdentityExt;
use droppen_auth::MaybeIdentity;
use droppen_http::extract::ApiPath;

/// The route, as gotcha registers it. The build id inside the URL makes the
/// answer immutable, so the path is also its own cache key.
pub const ASSET_PATH: &str = "/plugins/{key}/{id}/{build}/{*path}";

/// Serve one file of an installed plugin, or a `404`.
pub async fn serve_asset(
    State(state): State<PluginsState>,
    ApiPath((key, id, build, path)): ApiPath<(String, String, String, String)>,
    who: MaybeIdentity,
) -> Response<Body> {
    // The guard resolved a bridged request's identity even on this exempt
    // surface; a device's request is answered nothing, key or no key.
    if who.is_cloud() {
        return not_found();
    }
    let plugins = state.plugins.clone();
    let asset = tokio::task::spawn_blocking(move || plugins.asset(&key, &id, &build, &path))
        .await
        .ok()
        .flatten();
    match asset {
        Some(asset) => {
            let mut response = Response::new(Body::from(asset.bytes));
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
            headers.insert(header::HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
            // The build id is in the URL: these bytes never change under it.
            // `private` because the URL carries a capability, not because the
            // code is personal.
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
            headers.insert(
                header::HeaderName::from_static("cross-origin-resource-policy"),
                HeaderValue::from_static("same-origin"),
            );
            response
        }
        None => not_found(),
    }
}

/// The one answer every refusal gets. It does not say whether the id is
/// installed, the key right or the build current.
fn not_found() -> Response<Body> {
    let mut response = Response::new(Body::from(r#"{"error":"plugin_not_found"}"#));
    *response.status_mut() = StatusCode::NOT_FOUND;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
