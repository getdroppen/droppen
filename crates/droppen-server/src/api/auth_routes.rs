use droppen_auth::routes::*;
use droppen_auth::{Identity, MaybeIdentity, LOCAL_OWNER};
use droppen_http::error::ApiError;
use gotcha::axum::extract::FromRef;
use gotcha::{Json, State};
use crate::api::AppState;
use gotcha::axum::body::Body;
use gotcha::axum::extract::ConnectInfo;
use gotcha::axum::http::Request;
use serde_json::{json, Value};
use tower_service::Service;

const PASSWORD: &str = "correct horse battery";

/// Where a request comes from, as a socket and a `Host` show it. None of
/// them is trusted for it: that is the point of most of these tests.
#[derive(Clone, Copy, Debug)]
enum From {
    /// A browser on this device: loopback socket, loopback `Host`.
    ThisDevice,
    /// A TCP relay on this device (`ssh -R`, frp, a LAN-shared proxy)
    /// carrying a stranger, who says `Host: localhost`.
    Relay,
    /// Another device on the network.
    Elsewhere,
}

const EVERYWHERE: [From; 3] = [From::ThisDevice, From::Relay, From::Elsewhere];

/// One request through the router and its guard: `(status, headers, JSON body)`.
async fn send(
    router: &mut gotcha::axum::Router,
    from: From,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
    extra: &[(&str, &str)],
) -> (u16, gotcha::axum::http::HeaderMap, Value) {
    let (peer, host) = match from {
        From::ThisDevice => ("127.0.0.1:50000", "127.0.0.1:8700"),
        From::Relay => ("127.0.0.1:50001", "localhost"),
        From::Elsewhere => ("192.168.1.9:50000", "192.168.1.5:8700"),
    };
    let mut request = Request::builder().method(method).uri(path).header("host", host);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let mut request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).expect("json"))),
        None => request.body(Body::empty()),
    }
    .expect("request");
    request
        .extensions_mut()
        .insert(ConnectInfo(peer.parse::<std::net::SocketAddr>().expect("peer")));
    let response = router.call(request).await.expect("infallible");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.expect("body");
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn call(router: &mut gotcha::axum::Router, from: From, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let (status, _, body) = send(router, from, method, path, token, body, &[]).await;
    (status, body)
}

/// A router as the real server builds it, armed to mint setup codes as the
/// server that owns its instance is.
async fn server(auth_enabled: Option<bool>) -> (tempfile::TempDir, AppState, gotcha::axum::Router) {
    let home = tempfile::tempdir().expect("tempdir");
    let (state, router) = crate::api::test_support::guarded_router(home.path(), auth_enabled).await;
    let auth = &state.drive.auth;
    auth.setup.arm_for(&state.paths, "http://127.0.0.1:1/".into());
    // What the owner does at boot.
    auth.in_setup().await;
    (home, state, router)
}

async fn status_of(router: &mut gotcha::axum::Router, from: From, token: Option<&str>) -> Value {
    let (status, body) = call(router, from, "GET", STATUS_PATH, token, None).await;
    assert_eq!(status, 200, "{body}");
    body
}

/// A local-open token exchanged for a session of the device's owner.
async fn open_as_owner(router: &mut gotcha::axum::Router, state: &AppState) -> String {
    let token = state.drive.auth.local_open.mint().expect("a token");
    let (status, body) = call(router, From::ThisDevice, "POST", LOCAL_OPEN_PATH, None, Some(json!({"token": token}))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["username"], LOCAL_OWNER);
    body["token"].as_str().expect("a session token").to_string()
}

async fn login_as(router: &mut gotcha::axum::Router, name: &str) -> String {
    let (status, body) = call(
        router,
        From::Elsewhere,
        "POST",
        LOGIN_PATH,
        None,
        Some(json!({"username": name, "password": PASSWORD})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    body["token"].as_str().expect("token").to_string()
}

/// Nobody is let in for where they connect from — not a browser on this
/// device, and not a stranger whose TCP relay makes them look like one.
#[tokio::test]
async fn without_a_session_nobody_gets_in_whatever_the_socket_and_host_say() {
    let (_home, state, mut router) = server(None).await;
    for from in EVERYWHERE {
        for (method, path) in [
            ("GET", "/api/settings"),
            ("GET", "/api/sessions"),
            ("GET", "/api/cloud"),
            ("POST", "/api/cloud/link"),
            ("GET", "/api/terminals"),
            ("GET", "/api/fs/read?path=%2Fetc%2Fhosts"),
            ("GET", "/api/auth/sessions"),
            ("POST", "/api/auth/ticket"),
            ("GET", "/openapi.json"),
        ] {
            assert_eq!(call(&mut router, from, method, path, None, None).await.0, 401, "{from:?} {method} {path}");
        }
        let mut body = status_of(&mut router, from, None).await;
        // Where this binary is, for the commands the page shows: the same
        // for everyone, and no secret.
        assert!(body["droppenPath"].as_str().is_some_and(|path| !path.is_empty()), "{body}");
        assert!(body["droppenEnv"].is_array(), "{body}");
        body.as_object_mut().expect("object").remove("droppenPath");
        body.as_object_mut().expect("object").remove("droppenEnv");
        assert_eq!(
            body,
            json!({"authRequired": true, "setupRequired": true, "authOff": false, "user": null, "desktopApp": false}),
            "{from:?}: the same answer for everyone, and nothing else"
        );
    }
    assert!(state.drive.auth.setup.live().is_some(), "a code for the first account");
}

/// The setup code makes the first account once and signs the caller in;
/// after that the server is an ordinary one, this device included.
#[tokio::test]
async fn the_setup_code_makes_the_first_account_once() {
    let (_home, state, mut router) = server(None).await;
    let auth = state.drive.auth.clone();
    let file = droppen_auth::setup::code_file(&state.paths);
    let code = auth.setup.live().expect("minted by the server that owns the instance");
    assert!(file.exists());

    let attempt = |code: Option<&str>, name: &str| match code {
        Some(code) => json!({"code": code, "username": name, "password": PASSWORD}),
        None => json!({"username": name, "password": PASSWORD}),
    };
    let (status, body) = call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(attempt(None, "ada"))).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("code_required")), "{body}");
    let (status, body) = call(&mut router, From::ThisDevice, "POST", SETUP_PATH, None, Some(attempt(None, "ada"))).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (400, Some("code_required")),
        "this device needs it too: {body}"
    );
    let (status, body) = call(
        &mut router,
        From::Elsewhere,
        "POST",
        SETUP_PATH,
        None,
        Some(attempt(Some("ZZZZ-ZZZZ-ZZZZ-ZZZZ"), "ada")),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (403, Some("wrong_setup_code")), "{body}");
    let (status, body) = call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(attempt(Some(&code), "ada"))).await;
    assert_eq!(status, 200, "{body}");
    let token = body["token"].as_str().expect("token").to_string();
    assert!(!file.exists(), "spent");

    assert_eq!(
        call(&mut router, From::Elsewhere, "GET", "/api/settings", Some(&token), None).await.0,
        200
    );
    for from in EVERYWHERE {
        assert_eq!(call(&mut router, from, "GET", "/api/settings", None, None).await.0, 401, "{from:?}");
    }
    let body = status_of(&mut router, From::Elsewhere, Some(&token)).await;
    assert_eq!(body["setupRequired"], false);
    assert_eq!(body["user"], "ada");
    let (status, body) = call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(attempt(Some(&code), "eve"))).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("setup_done")), "{body}");
}

/// A local-open token: once, within its minute, for a session of the
/// device's owner — which is the owner of the Droppen link too.
#[tokio::test]
async fn a_local_open_token_signs_the_page_in_as_the_owner_once() {
    let (_home, state, mut router) = server(None).await;
    let (status, body) = call(
        &mut router,
        From::ThisDevice,
        "POST",
        LOCAL_OPEN_PATH,
        None,
        Some(json!({"token": "not-a-token"})),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (403, Some("invalid_local_open")), "{body}");

    let token = state.drive.auth.local_open.mint().expect("a token");
    let (status, body) = call(&mut router, From::Relay, "POST", LOCAL_OPEN_PATH, None, Some(json!({"token": token}))).await;
    assert_eq!(status, 200, "a token is a token wherever it comes from: {body}");
    let session = body["token"].as_str().unwrap().to_string();
    let (status, body) = call(
        &mut router,
        From::ThisDevice,
        "POST",
        LOCAL_OPEN_PATH,
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (403, Some("invalid_local_open")), "single use: {body}");

    assert_eq!(
        call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&session), None).await.0,
        200
    );
    let body = status_of(&mut router, From::ThisDevice, Some(&session)).await;
    assert_eq!(body["user"], LOCAL_OWNER);
    assert_eq!(body["setupRequired"], true);
    let (status, cloud) = call(&mut router, From::ThisDevice, "GET", "/api/cloud", Some(&session), None).await;
    assert_eq!(status, 200, "{cloud}");
    assert_eq!(cloud["localAuth"], true, "{cloud}");
    assert_eq!(cloud["canManage"], true, "the owner manages the link: {cloud}");
}

/// The owner makes the first account without a code; with it every
/// local-owner session ends, and logging in is the only way in.
#[tokio::test]
async fn the_owner_makes_the_first_account_without_a_code_and_its_sessions_end() {
    let (_home, state, mut router) = server(None).await;
    let owner = open_as_owner(&mut router, &state).await;
    let other = open_as_owner(&mut router, &state).await;
    let (status, body) = call(
        &mut router,
        From::ThisDevice,
        "POST",
        SETUP_PATH,
        Some(&owner),
        Some(json!({"username": "ada", "password": PASSWORD})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let ada = body["token"].as_str().unwrap().to_string();
    assert_eq!(call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&ada), None).await.0, 200);
    for session in [&owner, &other] {
        assert_eq!(
            call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(session), None).await.0,
            401
        );
    }
    let late = state.drive.auth.local_open.mint().expect("a token");
    let (status, body) = call(&mut router, From::ThisDevice, "POST", LOCAL_OPEN_PATH, None, Some(json!({"token": late}))).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("accounts_exist")), "{body}");
    assert!(!state.drive.auth.local_open.redeem(&late), "spent on the refusal");
}

/// An account made from the shell ends the owner's sessions just the same.
#[tokio::test]
async fn an_account_from_the_shell_ends_the_owners_sessions_too() {
    let (_home, state, mut router) = server(None).await;
    let owner = open_as_owner(&mut router, &state).await;
    state.drive.auth.users.add("ada", PASSWORD).await.expect("user add");
    assert_eq!(
        call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&owner), None).await.0,
        401
    );
    assert_eq!(status_of(&mut router, From::ThisDevice, Some(&owner)).await["user"], Value::Null);
}

/// The desktop app's own account signs its window in and is not a user
/// account: the app's server is never open without a credential, and a
/// user may not take the app account's name.
#[tokio::test]
async fn the_desktop_apps_own_account_opens_nothing_by_itself() {
    let (_home, state, mut router) = server(None).await;
    state.drive.auth.users.add_app("droppen-app", PASSWORD).await.expect("app account");
    for from in EVERYWHERE {
        assert_eq!(call(&mut router, from, "GET", "/api/settings", None, None).await.0, 401, "{from:?}");
        assert_eq!(status_of(&mut router, from, None).await["setupRequired"], true, "{from:?}");
    }
    let app = login_as(&mut router, "droppen-app").await;
    assert_eq!(call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&app), None).await.0, 200);
    let (status, body) = call(
        &mut router,
        From::ThisDevice,
        "POST",
        "/api/auth/password",
        Some(&app),
        Some(json!({"currentPassword": PASSWORD, "newPassword": "another long password"})),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (403, Some("app_account")), "{body}");

    let code = state.drive.auth.setup.live().expect("a code");
    let (status, body) = call(
        &mut router,
        From::Elsewhere,
        "POST",
        SETUP_PATH,
        None,
        Some(json!({"code": code, "username": "droppen-app", "password": PASSWORD})),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (409, Some("name_taken")), "{body}");
    let (status, body) = call(
        &mut router,
        From::Elsewhere,
        "POST",
        SETUP_PATH,
        None,
        Some(json!({"code": code, "username": "ada", "password": PASSWORD})),
    )
    .await;
    assert_eq!(status, 200, "the code was not spent on the taken name: {body}");
}

/// A setup request dropped with its connection after the right code and
/// before its account exists gives the code back — and while it was being
/// spent, nothing minted another (the server's 3 s check asked meanwhile).
#[tokio::test]
async fn a_setup_dropped_before_its_account_exists_gives_the_code_back() {
    let (_home, state, mut router) = server(None).await;
    let auth = &state.drive.auth;
    let code = auth.setup.live().expect("a code");
    let body = json!({"code": code, "username": "ada", "password": PASSWORD});

    // The store takes no write: the request gets past the code, and waits.
    let writes = auth.users.hold_writes().await;
    {
        let mut request = Box::pin(call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(body.clone())));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while auth.setup.live().is_some() {
            tokio::select! {
                biased;
                answer = &mut request => panic!("answered while the store took no write: {answer:?}"),
                () = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
            }
            assert!(std::time::Instant::now() < deadline, "the request never got past the code");
        }
        // The code is being spent; whatever asks mints nothing over it.
        assert!(auth.in_setup().await);
        assert_eq!(auth.setup.live(), None);
        // The connection goes away, and the request with it.
    }
    drop(writes);
    assert_eq!(auth.setup.live().as_deref(), Some(code.as_str()), "given back, the same code");
    assert!(!auth.users.has_users().await);

    // And it still makes the account.
    let (status, answer) = call(&mut router, From::Elsewhere, "POST", SETUP_PATH, None, Some(body)).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(auth.setup.live(), None, "used up by the account it made");
}

/// The desktop app's *Open in Browser*: its own login asks for a page
/// link, on its own server only — nobody else gets one, and once the
/// instance has a user account there is only the login page to open.
#[tokio::test]
async fn only_the_desktop_apps_own_login_gets_a_page_link() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut state = crate::api::test_support::state(home.path()).await;
    state.drive.auth.users.add_app("droppen-app", PASSWORD).await.expect("app account");
    let who = |name: &str| {
        Identity(droppen_http::identity::AuthIdentity {
            username: name.into(),
            token_sha256: None,
        })
    };
    let code_of = |error: ApiError| (error.status().as_u16(), error.body()["error"].as_str().unwrap_or_default().to_string());

    // Not a server the desktop app runs: `open` is the way there.
    let refused = local_open_grant(State(AuthState::from_ref(&state)), who("droppen-app")).await.unwrap_err();
    assert_eq!(code_of(refused), (403, "not_desktop_app".into()));

    state.desktop_app = true;
    let Json(grant) = local_open_grant(State(AuthState::from_ref(&state)), who("droppen-app")).await.expect("a link");
    let token = grant.token.0.expect("a token");
    assert!(state.drive.auth.local_open.redeem(&token), "a live one-time token");
    // Anybody else signed in to this server — the owner it opened, a
    // device through Droppen — gets none.
    for other in [LOCAL_OWNER, "cloud:d-1"] {
        let refused = local_open_grant(State(AuthState::from_ref(&state)), who(other)).await.unwrap_err();
        assert_eq!(code_of(refused), (403, "not_app_account".into()), "{other}");
    }

    // With a person's account there is only the login page to open.
    state.drive.auth.users.add("ada", PASSWORD).await.expect("account");
    let refused = local_open_grant(State(AuthState::from_ref(&state)), who("ada")).await.unwrap_err();
    assert_eq!(code_of(refused), (403, "not_app_account".into()));
    let Json(grant) = local_open_grant(State(AuthState::from_ref(&state)), who("droppen-app")).await.expect("an answer");
    assert_eq!(grant.token.0, None);
}

/// An instance with a user account asks for a login everywhere, with
/// `auth.enabled` unset or `true`: the account ends the setup state, so no
/// code is offered and only a login gets in.
#[tokio::test]
async fn with_a_user_account_everyone_logs_in() {
    for enabled in [None, Some(true)] {
        let (_home, state, mut router) = server(enabled).await;
        state.drive.auth.users.add("ada", PASSWORD).await.expect("account");
        // The owner's next tick.
        state.drive.auth.in_setup().await;
        for from in EVERYWHERE {
            let body = status_of(&mut router, from, None).await;
            assert_eq!(body["authRequired"], true, "{enabled:?} {from:?}");
            assert_eq!(body["setupRequired"], false);
            assert_eq!(
                call(&mut router, from, "GET", "/api/settings", None, None).await.0,
                401,
                "{enabled:?} {from:?}"
            );
        }
        assert_eq!(state.drive.auth.setup.live(), None, "no code for an instance with an account");
        let token = login_as(&mut router, "ada").await;
        assert_eq!(
            call(&mut router, From::ThisDevice, "GET", "/api/settings", Some(&token), None).await.0,
            200
        );
    }
}

/// `auth.enabled = false`: no setup state, no code, nothing refused.
#[tokio::test]
async fn with_authentication_off_nothing_is_asked() {
    let (_home, state, mut router) = server(Some(false)).await;
    for from in EVERYWHERE {
        assert_eq!(call(&mut router, from, "GET", "/api/settings", None, None).await.0, 200, "{from:?}");
        let body = status_of(&mut router, from, None).await;
        assert_eq!(body["authRequired"], false);
        assert_eq!(body["setupRequired"], false);
        assert_eq!(body["authOff"], true);
    }
    assert_eq!(state.drive.auth.setup.live(), None);
    let (status, body) = call(
        &mut router,
        From::Elsewhere,
        "POST",
        SETUP_PATH,
        None,
        Some(json!({"code": "x", "username": "ada", "password": PASSWORD})),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (409, Some("auth_off")));
    let token = state.drive.auth.local_open.mint().unwrap();
    let (status, body) = call(
        &mut router,
        From::ThisDevice,
        "POST",
        LOCAL_OPEN_PATH,
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (409, Some("auth_off")));
}

/// Another site's page in a browser that holds a session: `Sec-Fetch-Site`
/// is the browser's word on who started the request, and anything but this
/// page itself is refused on `/api/*` — a WebSocket handshake included.
#[tokio::test]
async fn another_sites_page_is_refused_even_with_a_session() {
    let (_home, state, mut router) = server(None).await;
    let owner = open_as_owner(&mut router, &state).await;
    for site in ["cross-site", "same-site"] {
        for (method, path) in [
            ("GET", "/api/settings"),
            ("POST", "/api/terminals"),
            ("GET", STATUS_PATH),
            ("POST", SETUP_PATH),
        ] {
            let (status, _, body) = send(
                &mut router,
                From::ThisDevice,
                method,
                path,
                Some(&owner),
                None,
                &[("sec-fetch-site", site)],
            )
            .await;
            assert_eq!(status, 403, "{site} {method} {path}: {body}");
        }
    }
    for site in ["same-origin", "none"] {
        let (status, _, _) = send(
            &mut router,
            From::ThisDevice,
            "GET",
            "/api/settings",
            Some(&owner),
            None,
            &[("sec-fetch-site", site)],
        )
        .await;
        assert_eq!(status, 200, "{site}");
    }
    let upgrade = [
        ("connection", "upgrade"),
        ("upgrade", "websocket"),
        ("sec-websocket-version", "13"),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    let mut foreign = upgrade.to_vec();
    foreign.push(("origin", "https://evil.example"));
    assert_eq!(
        send(&mut router, From::ThisDevice, "GET", "/api/events", Some(&owner), None, &foreign)
            .await
            .0,
        403
    );
    let mut dev = upgrade.to_vec();
    dev.push(("origin", "http://localhost:5180"));
    assert_eq!(
        send(&mut router, From::ThisDevice, "GET", "/api/events", Some(&owner), None, &dev)
            .await
            .0,
        403,
        "a development origin is not trusted for any host"
    );
}

async fn mint_ticket(router: &mut gotcha::axum::Router, token: &str) -> String {
    let (status, body) = call(
        router,
        From::ThisDevice,
        "POST",
        "/api/auth/ticket",
        Some(token),
        Some(json!({"scope": "download"})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    body["ticket"].as_str().unwrap().to_string()
}

/// A ticket is worth no more than the login that minted it: logging out or
/// removing the account kills it, and only a login mints one.
#[tokio::test]
async fn a_ticket_dies_with_its_session_and_its_account() {
    let (_home, state, mut router) = server(None).await;
    state.drive.auth.users.add("ada", PASSWORD).await.expect("account");
    let download = |ticket: &str| format!("/api/fs/download?path=%2Fnonexistent&ticket={ticket}");
    let first = login_as(&mut router, "ada").await;
    let ticket = mint_ticket(&mut router, &first).await;
    assert_ne!(
        call(&mut router, From::Elsewhere, "GET", &download(&ticket), None, None).await.0,
        401,
        "live"
    );
    let (status, _) = call(&mut router, From::ThisDevice, "POST", "/api/auth/logout", Some(&first), None).await;
    assert_eq!(status, 200);
    assert_eq!(
        call(&mut router, From::Elsewhere, "GET", &download(&ticket), None, None).await.0,
        401,
        "its session is gone"
    );

    let second = login_as(&mut router, "ada").await;
    let ticket = mint_ticket(&mut router, &second).await;
    assert_ne!(call(&mut router, From::Elsewhere, "GET", &download(&ticket), None, None).await.0, 401);
    state.drive.auth.users.remove("ada").await.expect("user rm");
    assert_eq!(
        call(&mut router, From::Elsewhere, "GET", &download(&ticket), None, None).await.0,
        401,
        "its account is gone"
    );
}

/// The page, and every other answer but an embedded app's, may not be put
/// in another site's frame.
#[tokio::test]
async fn no_other_site_may_frame_the_page() {
    let (_home, _state, mut router) = server(None).await;
    for path in ["/", "/sessions", STATUS_PATH] {
        let (_, headers, _) = send(&mut router, From::ThisDevice, "GET", path, None, None, &[]).await;
        assert_eq!(headers.get("x-frame-options").map(|v| v.to_str().unwrap()), Some("SAMEORIGIN"), "{path}");
        assert_eq!(
            headers.get("content-security-policy").map(|v| v.to_str().unwrap()),
            Some("frame-ancestors 'self'"),
            "{path}"
        );
    }
}

/// Another device of the account, bridged in by the agent, is itself — and
/// the bridge keeps working on a device with no user account.
#[tokio::test]
async fn a_bridged_device_is_itself() {
    let (_home, state, mut router) = server(None).await;
    let credential = state.drive.auth.internal_credential().to_string();
    let bridged = [
        (droppen_http::bridge::CREDENTIAL_HEADER, credential.as_str()),
        (droppen_http::bridge::DEVICE_HEADER, "d-1"),
    ];
    let (status, _, body) = send(&mut router, From::ThisDevice, "GET", STATUS_PATH, None, None, &bridged).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["user"], "cloud:d-1");
    assert_eq!(
        send(&mut router, From::ThisDevice, "GET", "/api/settings", None, None, &bridged).await.0,
        200
    );
    let (status, _, body) = send(&mut router, From::Elsewhere, "GET", "/api/settings", None, None, &bridged).await;
    assert_eq!(status, 401, "only from loopback: {body}");
    let (status, _, body) = send(
        &mut router,
        From::ThisDevice,
        "POST",
        LOCAL_OPEN_PATH,
        None,
        Some(json!({"token": "x"})),
        &bridged,
    )
    .await;
    assert_eq!((status, body["error"].as_str()), (403, Some("not_from_cloud")));
}

#[tokio::test]
async fn the_status_says_whether_the_desktop_app_owns_the_login() {
    let home = tempfile::tempdir().expect("tempdir");
    let mut state = crate::api::test_support::state(home.path()).await;
    let Json(ordinary) = status(State(AuthState::from_ref(&state)), MaybeIdentity(None)).await;
    assert!(!ordinary.desktop_app);
    assert!(ordinary.auth_required, "on by default");
    assert!(ordinary.setup_required, "and no account yet");
    state.desktop_app = true;
    let Json(under_the_app) = status(State(AuthState::from_ref(&state)), MaybeIdentity(None)).await;
    assert!(under_the_app.desktop_app);
    let wire = serde_json::to_value(&under_the_app).expect("json");
    assert_eq!(wire["desktopApp"], true, "the web app reads it as desktopApp: {wire}");
}

#[tokio::test]
async fn the_status_says_how_to_run_this_server_on_its_device() {
    let home = tempfile::tempdir().expect("tempdir");
    let state = crate::api::test_support::state(home.path()).await;
    let state_paths = state.paths.clone();
    let Json(answer) = status(State(AuthState::from_ref(&state)), MaybeIdentity(None)).await;
    assert_eq!(answer.droppen_path, droppen_base::program::droppen_path());
    assert!(!answer.droppen_path.is_empty());
    // The test binary has no `droppen` beside it: the binary itself.
    let exe = std::env::current_exe().expect("exe");
    let name = exe.file_name().expect("name").to_string_lossy().into_owned();
    assert!(answer.droppen_path.ends_with(&name), "{} vs {name}", answer.droppen_path);
    let wire = serde_json::to_value(&answer).expect("json");
    assert_eq!(
        wire["droppenPath"],
        answer.droppen_path.as_str(),
        "the web app reads it as droppenPath: {wire}"
    );
    // And what such a command needs to reach this instance: this test's
    // server keeps its files under a home of its own, not the process's.
    let expected: Vec<serde_json::Value> = state_paths.droppen_env()
        .into_iter()
        .map(|(name, value)| json!({"name": name, "value": value}))
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(wire["droppenEnv"], serde_json::Value::Array(expected), "{wire}");
}
