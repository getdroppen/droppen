//! `GET|HEAD /plugins/<key>/<id>/<build>/<path>`: a plugin's code, to the
//! pages this server serves.
//!
//! # Why the key is a path segment
//!
//! A page cannot put its bearer token on an `import()`, and a query string
//! would not survive the plugin's own relative chunk imports (`./tasks-page-
//! ….js`), so the credential is part of the path. `<key>` is the instance-wide
//! secret `crate::plugins` keeps in `state.db`: 128 random bits as hex, made
//! on first need and stable across restarts. It reaches a page only inside
//! `PluginStatus.entry`, which only a signed-in, non-cloud caller is given
//! (`GET /api/plugins`, the feed's `plugins` frame); [`super::Plugins::asset`]
//! compares it in constant time.
//!
//! # How this sits with the authentication guard
//!
//! Deliberately on the guard's *static* side: `crate::auth::exempt` lets every
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
//! per-subscriber filter (`feed::global`, which hands a `cloud:` subscriber
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

use crate::api::extract::ApiPath;
use crate::api::AppState;
use crate::auth::MaybeIdentity;

/// The route, as gotcha registers it. The build id inside the URL makes the
/// answer immutable, so the path is also its own cache key.
pub const ASSET_PATH: &str = "/plugins/{key}/{id}/{build}/{*path}";

/// Serve one file of an installed plugin, or a `404`.
pub async fn serve_asset(
    State(state): State<AppState>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support;
    use crate::plugins::tests::package_bytes;
    use gotcha::axum::http::Request;
    use tower_service::Service;

    async fn call(router: &mut gotcha::axum::Router, uri: &str) -> Response<Body> {
        let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        std::future::poll_fn(|cx| <gotcha::axum::Router as Service<Request<Body>>>::poll_ready(router, cx))
            .await
            .unwrap();
        router.call(request).await.unwrap()
    }

    /// A served router over a scratch home with authentication off, and the
    /// entry URL and key of one installed plugin.
    async fn served(home: &std::path::Path) -> (gotcha::axum::Router, String, String) {
        let (state, router) = test_support::guarded_router(home, Some(false)).await;
        state.plugins.install(&package_bytes("tasks", "0.1.0"), None).unwrap();
        let status = state.plugins.status_of("tasks").unwrap();
        let entry = status.entry.expect("an entry");
        let key = entry.strip_prefix("/plugins/").unwrap().split('/').next().unwrap().to_string();
        (router, entry, key)
    }

    #[tokio::test]
    async fn the_entry_and_its_relative_chunk_are_served_with_the_contract_s_headers() {
        let home = tempfile::tempdir().unwrap();
        let (mut router, entry, _key) = served(home.path()).await;
        let response = call(&mut router, &entry).await;
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(headers[header::CONTENT_TYPE], "text/javascript");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers[header::CACHE_CONTROL], "private, max-age=31536000, immutable");
        assert_eq!(headers["cross-origin-resource-policy"], "same-origin");
        let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"export default {};\n");

        let chunk = entry.replace("dist/index.js", "dist/page-abc.js");
        let response = call(&mut router, &chunk).await;
        assert_eq!(response.status(), StatusCode::OK, "a relative chunk resolves under the same key and build");
        let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"export const page = 1;\n");

        let package = entry.replace("dist/index.js", "package.json");
        let response = call(&mut router, &package).await;
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    }

    /// The entry URL is percent-encoded segment by segment, and the route's
    /// wildcard comes back decoded: a package whose entry has a space in its
    /// name still loads.
    #[tokio::test]
    async fn a_percent_encoded_entry_is_decoded_before_the_file_is_looked_for() {
        use crate::plugins::testing::{package, tarball};
        let home = tempfile::tempdir().unwrap();
        let (state, mut router) = test_support::guarded_router(home.path(), Some(false)).await;
        let mut pkg = package("tasks", "0.1.0");
        pkg["exports"] = serde_json::json!({ ".": "./dist/a b.js" });
        state
            .plugins
            .install(&tarball(&pkg, &[("dist/a b.js", b"export default {};\n")]), None)
            .unwrap();
        let entry = state.plugins.status_of("tasks").unwrap().entry.unwrap();
        assert!(entry.ends_with("/dist/a%20b.js"), "{entry}");
        let response = call(&mut router, &entry).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"export default {};\n");
    }

    #[tokio::test]
    async fn head_is_answered_with_the_headers_and_no_body() {
        let home = tempfile::tempdir().unwrap();
        let (state, mut router) = test_support::guarded_router(home.path(), Some(false)).await;
        state.plugins.install(&package_bytes("tasks", "0.1.0"), None).unwrap();
        let entry = state.plugins.status_of("tasks").unwrap().entry.unwrap();
        let request = Request::builder().method("HEAD").uri(&entry).body(Body::empty()).unwrap();
        let response = router.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/javascript");
        let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(body.is_empty(), "HEAD carries no body");
    }

    #[tokio::test]
    async fn every_other_key_build_path_and_switch_is_a_404_before_the_spa_fallback() {
        let home = tempfile::tempdir().unwrap();
        let (state, mut router) = test_support::guarded_router(home.path(), Some(false)).await;
        state.plugins.install(&package_bytes("tasks", "0.1.0"), None).unwrap();
        let status = state.plugins.status_of("tasks").unwrap();
        let entry = status.entry.clone().unwrap();
        let key = entry.strip_prefix("/plugins/").unwrap().split('/').next().unwrap().to_string();
        let build = status.build.clone();

        let wrong_key = format!("/plugins/{}/tasks/{build}/dist/index.js", "0".repeat(32));
        let old_build = format!("/plugins/{key}/tasks/0.1.0-000000000000/dist/index.js");
        let traversal = format!("/plugins/{key}/tasks/{build}/../package.json");
        let unknown = format!("/plugins/{key}/notes/{build}/dist/index.js");
        let directory = format!("/plugins/{key}/tasks/{build}/dist");
        let short = format!("/plugins/{key}/tasks");
        for uri in [wrong_key, old_build, traversal, unknown, directory, short] {
            let response = call(&mut router, &uri).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            let kind = response.headers().get(header::CONTENT_TYPE).map(|value| value.to_str().unwrap().to_string());
            assert_eq!(kind.as_deref(), Some("application/json"), "{uri}: never the SPA shell");
        }

        state.plugins.set_enabled("tasks", false, None, super::super::DecisionReason::Settings).unwrap();
        let response = call(&mut router, &entry).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "a plugin that is off");
    }

    #[tokio::test]
    async fn a_bridged_identity_gets_nothing_even_with_the_right_key() {
        use gotcha::axum::http::Response as HttpResponse;
        use gotcha::axum::middleware::Next;
        use gotcha::axum::extract::Request;

        async fn as_cloud(mut request: Request, next: Next) -> HttpResponse<Body> {
            request.extensions_mut().insert(crate::auth::AuthIdentity {
                username: "cloud:device-1".into(),
                token_sha256: None,
            });
            next.run(request).await
        }

        let home = tempfile::tempdir().unwrap();
        let (state, router) = test_support::guarded_router(home.path(), Some(false)).await;
        state.plugins.install(&package_bytes("tasks", "0.1.0"), None).unwrap();
        let entry = state.plugins.status_of("tasks").unwrap().entry.unwrap();
        // Outside the guard, so the injected identity is what the handler sees;
        // that is exactly the shape `crate::auth::guard` leaves for the bridge.
        let mut router = router.layer(gotcha::axum::middleware::from_fn(as_cloud));
        let response = call(&mut router, &entry).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).expect("a JSON refusal");
        assert_eq!(body["error"], serde_json::json!("plugin_not_found"));
        assert!(body.get("entry").is_none(), "nothing about the plugin comes back");
    }
}
