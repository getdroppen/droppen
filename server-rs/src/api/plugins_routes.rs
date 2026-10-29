use droppen_plugins::routes::LIST;
use gotcha::axum::body::Body;
use gotcha::axum::http::{Request, StatusCode};
use serde_json::json;
use tower_service::Service;

async fn call(router: &mut gotcha::axum::Router, uri: &str) -> (StatusCode, String) {
    let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    std::future::poll_fn(|cx| <gotcha::axum::Router as Service<Request<Body>>>::poll_ready(router, cx))
        .await
        .unwrap();
    let response = router.call(request).await.unwrap();
    let status = response.status();
    let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// The whole HTTP surface of an install: the tarball goes up, the status
/// comes back with its entry, the list agrees, and the delete removes it.
#[tokio::test]
async fn an_install_is_listed_with_its_entry_and_a_delete_uninstalls_it() {
    use droppen_plugins::testing::package_bytes;
    use droppen_plugins::testing::{package, tarball};

    let home = tempfile::tempdir().unwrap();
    let (state, mut router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
    let send = |router: &mut gotcha::axum::Router, method: &'static str, uri: &str, body: Vec<u8>| {
        let request = Request::builder().method(method).uri(uri).body(Body::from(body)).unwrap();
        let fut = router.call(request);
        async move {
            let response = fut.await.unwrap();
            let status = response.status();
            let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
            (status, serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default())
        }
    };

    let (status, body) = send(&mut router, "POST", LIST, package_bytes("tasks", "0.1.0")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], json!("tasks"));
    assert_eq!(body["build"].as_str().unwrap().len(), 18);
    assert_eq!(body["aliases"], json!(["/tasks"]));
    assert_eq!(body["permissions"], json!(["storage"]));
    let entry = body["entry"].as_str().expect("an entry").to_string();

    let (status, list) = send(&mut router, "GET", LIST, Vec::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["plugins"][0]["entry"], json!(entry));

    // The entry answers over the same router, to a caller with no session:
    // the key in the path is the credential.
    let (status, _) = call(&mut router, &entry).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&mut router, "DELETE", "/api/plugins/tasks", Vec::new()).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, serde_json::Value::Null));
    let (status, list) = send(&mut router, "GET", LIST, Vec::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["plugins"], json!([]));
    let (status, body) = send(&mut router, "DELETE", "/api/plugins/tasks", Vec::new()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], json!("plugin_not_found"));

    // A refused package is 400 invalid_package and changes nothing.
    let mut broken = package("notes", "0.1.0");
    broken["droppen"]["id"] = json!("Notes");
    let (status, body) = send(&mut router, "POST", LIST, tarball(&broken, &[("dist/index.js", b"x")])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], json!("invalid_package"));
    assert!(body["message"].as_str().unwrap().contains("is not a plugin id"));
    let _ = state;
}

#[tokio::test]
async fn a_tarball_over_the_limit_is_413_too_large() {
    use droppen_plugins::install::MAX_TARBALL_BYTES;
    let home = tempfile::tempdir().unwrap();
    let (_state, mut router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
    let request = Request::builder()
        .method("POST")
        .uri(LIST)
        .body(Body::from(vec![0u8; MAX_TARBALL_BYTES + 1]))
        .unwrap();
    let response = router.call(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"], json!("too_large"));
    assert_eq!(body["limit"], json!(MAX_TARBALL_BYTES));
}

#[tokio::test]
async fn a_cloud_identity_is_refused_everywhere() {
    use gotcha::axum::extract::Request as ExtractRequest;
    use gotcha::axum::middleware::Next;
    use gotcha::axum::response::Response as HttpResponse;

    async fn as_cloud(mut request: ExtractRequest, next: Next) -> HttpResponse<Body> {
        request.extensions_mut().insert(droppen_http::identity::AuthIdentity {
            username: "cloud:device-1".into(),
            token_sha256: None,
        });
        next.run(request).await
    }

    let home = tempfile::tempdir().unwrap();
    let (state, router) = crate::api::test_support::guarded_router(home.path(), Some(false)).await;
    state.plugins.install(&droppen_plugins::testing::package_bytes("tasks", "0.1.0"), None).unwrap();
    let mut router = router.layer(gotcha::axum::middleware::from_fn(as_cloud));
    // Empty or malformed bodies on purpose: the refusal must come before
    // the extractors, so these are 404 and never 400.
    for (method, uri, body) in [
        ("GET", LIST, Body::empty()),
        ("POST", LIST, Body::empty()),
        ("PUT", "/api/plugins/tasks/enabled", Body::empty()),
        ("PUT", "/api/plugins/tasks/docs/state", Body::empty()),
        ("POST", "/api/plugins/tasks/batch", Body::empty()),
        ("GET", "/api/plugins/tasks/docs", Body::empty()),
        ("GET", "/api/plugins/tasks/docs/state", Body::empty()),
        ("DELETE", "/api/plugins/tasks", Body::empty()),
    ] {
        let request = Request::builder().method(method).uri(uri).body(body).unwrap();
        let response = router.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {uri}");
        let body = gotcha::axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        assert_eq!(body["error"], json!("plugin_not_found"), "{method} {uri}");
    }
}
