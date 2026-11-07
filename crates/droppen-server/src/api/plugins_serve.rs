use crate::api::test_support;
use droppen_plugins::testing::package_bytes;
use gotcha::axum::body::Body;
use gotcha::axum::http::{header, Request, Response, StatusCode};
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
    use droppen_plugins::testing::{package, tarball};
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

    state.plugins.set_enabled("tasks", false, None, droppen_plugins::DecisionReason::Settings).unwrap();
    let response = call(&mut router, &entry).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "a plugin that is off");
}

#[tokio::test]
async fn a_bridged_identity_gets_nothing_even_with_the_right_key() {
    use gotcha::axum::http::Response as HttpResponse;
    use gotcha::axum::middleware::Next;
    use gotcha::axum::extract::Request;

    async fn as_cloud(mut request: Request, next: Next) -> HttpResponse<Body> {
        request.extensions_mut().insert(droppen_http::identity::AuthIdentity {
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
    // that is exactly the shape `droppen_auth::guard` leaves for the bridge.
    let mut router = router.layer(gotcha::axum::middleware::from_fn(as_cloud));
    let response = call(&mut router, &entry).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).expect("a JSON refusal");
    assert_eq!(body["error"], serde_json::json!("plugin_not_found"));
    assert!(body.get("entry").is_none(), "nothing about the plugin comes back");
}
