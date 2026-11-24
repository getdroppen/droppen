//! The A2A server (`droppen_a2a`) behind the guards the server puts in front
//! of it: `/a2a/*` answers to an A2A token from loopback and to nothing else,
//! a login included, and `/api/a2a/*` to a login.

use gotcha::axum::body::Body;
use gotcha::axum::extract::ConnectInfo;
use gotcha::axum::http::Request;
use serde_json::{json, Value};
use tower_service::Service;

/// One request through the router and its guard: its status and JSON body.
async fn send(
    router: &mut gotcha::axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
    extra: &[(&str, &str)],
) -> (u16, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:8700")
        .header("a2a-version", "1.0");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let mut request = match body {
        Some(body) => request.header("content-type", "application/json").body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .expect("request");
    request
        .extensions_mut()
        .insert(ConnectInfo("127.0.0.1:50000".parse::<std::net::SocketAddr>().expect("peer")));
    let response = router.call(request).await.expect("infallible");
    let status = response.status().as_u16();
    let bytes = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("body");
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A server with authentication on, an entry `main`, an A2A token for it,
/// and a login of the device's owner.
async fn server() -> (tempfile::TempDir, crate::api::AppState, gotcha::axum::Router, String, String) {
    let home = tempfile::tempdir().expect("tempdir");
    let (state, router) = crate::api::test_support::guarded_router(home.path(), Some(true)).await;
    // A directory the real backend accepts: in the home, and no repository,
    // so no worktree.
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).expect("the entry's directory");
    let entry = json!({"id": "main", "name": "Main", "source": "claude", "cwd": work, "enabled": true, "isolation": "none"});
    state.a2a.create_entry(serde_json::from_value(entry).unwrap()).await.expect("an entry");
    let token = state
        .a2a
        .create_token(serde_json::from_value(json!({"name": "dont-do", "entries": ["main"]})).unwrap())
        .expect("a token")
        .secret;
    let login = state
        .drive
        .auth
        .sessions
        .issue(droppen_auth::LOCAL_OWNER, None)
        .await
        .expect("an owner login")
        .token;
    (home, state, router, token, login)
}

#[tokio::test]
async fn a2a_answers_an_a2a_token_on_every_method_and_a_login_on_none() {
    let (_home, _state, mut router, token, login) = server().await;
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "ListTasks", "params": {}});

    let (status, body) = send(&mut router, "POST", "/a2a/agents/main/jsonrpc", Some(&token), Some(list.clone()), &[]).await;
    assert_eq!(status, 200, "a POST gets past the login guard to the A2A check: {body}");
    assert_eq!(body["result"]["tasks"], json!([]));
    let (status, _) = send(
        &mut router,
        "GET",
        "/a2a/agents/main/.well-known/agent-card.json",
        Some(&token),
        None,
        &[],
    )
    .await;
    assert_eq!(status, 200);

    for credential in [None, Some(login.as_str())] {
        let (status, body) = send(&mut router, "POST", "/a2a/agents/main/jsonrpc", credential, Some(list.clone()), &[]).await;
        assert_eq!(status, 401, "{credential:?}");
        assert_eq!(
            body["error"]["status"], "UNAUTHENTICATED",
            "the A2A check answered, not the login guard: {body}"
        );
        let (status, _) = send(&mut router, "GET", "/a2a/agents/main/.well-known/agent-card.json", credential, None, &[]).await;
        assert_eq!(status, 401, "a login is no A2A caller");
    }
}

#[tokio::test]
async fn a_device_bridged_in_reaches_no_a2a_route() {
    let (_home, state, mut router, token, _login) = server().await;
    let credential = state.drive.auth.internal_credential().to_string();
    let bridged = [
        (droppen_http::bridge::CREDENTIAL_HEADER, credential.as_str()),
        (droppen_http::bridge::DEVICE_HEADER, "d-1"),
    ];
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "ListTasks", "params": {}});
    let (status, _) = send(&mut router, "POST", "/a2a/agents/main/jsonrpc", Some(&token), Some(list), &bridged).await;
    assert_eq!(status, 403, "not even with a token");
}

#[tokio::test]
async fn the_management_routes_answer_a_login_only() {
    let (_home, _state, mut router, token, login) = server().await;
    let (status, _) = send(&mut router, "GET", "/api/a2a/entries", None, None, &[]).await;
    assert_eq!(status, 401);
    let (status, _) = send(&mut router, "GET", "/api/a2a/entries", Some(&token), None, &[]).await;
    assert_eq!(status, 401, "an A2A token opens no management route");
    let (status, body) = send(&mut router, "GET", "/api/a2a/entries", Some(&login), None, &[]).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["entries"][0]["id"], "main");
    assert!(body["defaultTemplate"].as_str().unwrap().contains("{{fence}}"));
    assert_eq!(body["publicBase"], Value::Null, "no [a2a] public_base: the card uses the request's Host");

    let (status, created) = send(
        &mut router,
        "POST",
        "/api/a2a/tokens",
        Some(&login),
        Some(json!({"name": "other", "entries": ["main"]})),
        &[],
    )
    .await;
    assert_eq!(status, 200, "{created}");
    assert!(created["secret"].as_str().unwrap().starts_with("dpa_"));
    let (_, tokens) = send(&mut router, "GET", "/api/a2a/tokens", Some(&login), None, &[]).await;
    assert!(
        !tokens.to_string().contains(created["secret"].as_str().unwrap()),
        "a secret is shown once"
    );
    let (status, _) = send(&mut router, "GET", "/api/a2a/tasks?state=working", Some(&login), None, &[]).await;
    assert_eq!(status, 200);
    let (status, contexts) = send(&mut router, "GET", "/api/a2a/contexts?pageSize=10", Some(&login), None, &[]).await;
    assert_eq!((status, contexts), (200, json!({"contexts": [], "nextPageToken": null})));
    let (status, _) = send(&mut router, "GET", "/api/a2a/contexts?pageSize=0", Some(&login), None, &[]).await;
    assert_eq!(status, 400);
    let (status, _) = send(&mut router, "GET", "/api/a2a/contexts", Some(&token), None, &[]).await;
    assert_eq!(status, 401, "an A2A token opens no management route");
}
