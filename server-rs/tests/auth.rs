//! Local accounts, from the outside.
//!
//! This is the boundary that decides whether a stranger who can reach the port
//! can read `$HOME`, open a shell and replace the binary, so the tests are
//! mostly about *refusals*: which faces of the server answer `401`, which two
//! deliberately do not, and what a ticket is and is not good for.
//!
//! The other half is the first account. A server with no user account is in
//! its setup state: without a session nothing answers but the status route,
//! the two ways to a session and the page — for a browser on this device as
//! much as for a stranger a TCP relay makes look like one. The ways in are
//! `remote-hand-server open` (a one-time link only this OS user can get from
//! the running server) and the one-time setup code; `user add` on the device
//! needs neither. Once an account exists, every request needs a login.
//! Switching authentication off (`auth.enabled = false`) is the one way to an
//! open server, and it is a deliberate one.
//!
//! These tests start their servers with authentication as it is by default
//! ([`common::TestServer::start_guarded_at`]); everything else in this suite
//! runs with it switched off.

mod common;

use std::path::Path;

use remote_hand_server::auth::users::UsersStore;
use remote_hand_server::instance::DEFAULT_INSTANCE;
use remote_hand_server::sessions::Paths;
use serde_json::{json, Value};

/// Long enough for the policy, and not on the worst-password list.
const PASSWORD: &str = "correct horse battery";

/// The account store of one instance under a test's home directory — the same
/// one the server built on that home will read, because accounts are
/// per-instance.
fn accounts(home: &Path, instance: &str) -> UsersStore {
    UsersStore::new(&Paths::for_instance(home.to_path_buf(), instance))
}

/// Put an account in the instance a server is about to be started on.
async fn seed_account(home: &Path, name: &str, password: &str) {
    accounts(home, DEFAULT_INSTANCE).add(name, password).await.expect("seed account");
}

/// Case-insensitive header lookup over a raw response head.
fn header(head: &str, name: &str) -> Option<String> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
}

/// A `GET` carrying a bearer token.
async fn get_as(server: &common::TestServer, path: &str, token: &str) -> (u16, String) {
    let authorization = format!("Bearer {token}");
    server.request("GET", path, &[("Authorization", &authorization)], &[]).await
}

/// A JSON request carrying a bearer token.
async fn json_as(server: &common::TestServer, method: &str, path: &str, token: &str, body: &Value) -> (u16, String) {
    let authorization = format!("Bearer {token}");
    let encoded = serde_json::to_vec(body).expect("encode");
    server
        .request(
            method,
            path,
            &[("Content-Type", "application/json"), ("Authorization", &authorization)],
            &encoded,
        )
        .await
}

/// `POST /api/auth/login` over a socket this function owns, so a caller can have
/// many of them in flight at once.
///
/// [`common::TestServer::json`] takes `&self`, which is fine for every other
/// test here and is exactly what the two concurrency tests cannot use: they need
/// N requests genuinely simultaneous, each on its own connection, with nothing
/// of the harness shared between them.
async fn login_at(address: std::net::SocketAddr, password: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let body = serde_json::to_vec(&json!({ "username": "ada", "password": password })).expect("encode");
    let head = format!(
        "POST /api/auth/login HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut stream = tokio::net::TcpStream::connect(address).await.expect("connect");
    let mut raw = head.into_bytes();
    raw.extend_from_slice(&body);
    stream.write_all(&raw).await.expect("write");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");

    let text = String::from_utf8_lossy(&response).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status line in {text}"));
    let payload = text.split_once("\r\n\r\n").map(|(_, rest)| rest.to_string()).unwrap_or_default();
    // `Connection: close` means no chunked framing to unpick — the body is
    // whatever arrived before the socket did.
    (status, payload)
}

/// Log in, asserting it worked, and hand back the token.
async fn login(server: &common::TestServer, name: &str, password: &str) -> String {
    let (status, body) = server
        .json("POST", "/api/auth/login", &json!({ "username": name, "password": password }))
        .await;
    assert_eq!(status, 200, "login -> {status}: {body}");
    let parsed: Value = serde_json::from_str(&body).expect("json");
    parsed["token"].as_str().expect("token").to_string()
}

/// Every face of the server that must be behind the guard.
const GUARDED: [(&str, &str); 7] = [
    ("GET", "/api/settings"),
    ("GET", "/api/sessions"),
    ("GET", "/api/push/vapid"),
    ("GET", "/api/nope"),
    ("GET", "/openapi.json"),
    ("GET", "/redoc"),
    ("POST", "/api/server/self-upgrade"),
];

/// A raw `GET` naming `host` in its one `Host` header — which the harness's own
/// requests cannot, since they always name the server's address. What a TCP
/// relay on this device (`ssh -R`, frp, a LAN-shared proxy) delivers: a
/// stranger's request, from loopback, saying whatever `Host` it likes.
async fn raw_get(server: &common::TestServer, path: &str, host: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(server.addr).await.expect("connect");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    let text = String::from_utf8_lossy(&response).into_owned();
    let status = text.split_whitespace().nth(1).and_then(|code| code.parse().ok()).expect("a status line");
    (status, text)
}

/// No account is not an open door, and connecting from this device is not a
/// key: without a session every guarded face refuses — to a browser on the
/// device, and to a stranger a TCP relay makes look like one. The status route
/// says why, so the page can offer the ways in instead of a login form nobody
/// could use.
#[tokio::test]
async fn with_no_accounts_nobody_gets_in_without_a_session() {
    let server = common::TestServer::start_guarded().await;
    for (method, path) in GUARDED {
        let (status, body) = server.request(method, path, &[], &[]).await;
        assert_eq!(status, 401, "{method} {path} must need a session on a server with no accounts: {body}");
    }
    for (method, path) in [
        ("GET", "/api/cloud"),
        ("POST", "/api/cloud/link"),
        ("GET", "/api/events"),
        ("GET", "/api/terminals"),
        ("POST", "/api/terminals"),
        ("GET", "/api/fs/list?path=%2F"),
        ("GET", "/api/fs/read?path=%2Fetc%2Fhosts"),
        ("GET", "/api/fs/download?path=%2Fetc%2Fhosts"),
        ("GET", "/api/apps"),
        ("POST", "/api/uploads"),
        ("POST", "/api/fs/upload?path=%2Ftmp"),
        ("GET", "/api/media/github?url=https%3A%2F%2Fgithub.com%2Fa.png"),
        ("POST", "/api/notify"),
        ("GET", "/scalar"),
    ] {
        let (status, body) = server.request(method, path, &[], &[]).await;
        assert_eq!(status, 401, "{method} {path} -> {status}: {body}");
    }
    for host in ["localhost", "localhost:8700", "127.0.0.1", "[::1]:8700"] {
        let (status, _) = raw_get(&server, "/api/settings", host).await;
        assert_eq!(status, 401, "Host: {host} through a relay is nobody");
    }
    // A name the server does not answer to gets no further than that
    // (`crate::origin`, "Which names the server answers to").
    let (status, _) = raw_get(&server, "/api/settings", "evil.example").await;
    assert_eq!(status, 403, "Host: evil.example is not this server's name");

    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["authRequired"], true);
    assert_eq!(status["setupRequired"], true);
    assert_eq!(status["authOff"], false);
    assert_eq!(status["user"], Value::Null);
    let mut keys: Vec<&String> = status.as_object().expect("object").keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["authOff", "authRequired", "desktopApp", "droppenEnv", "droppenPath", "setupRequired", "user"],
        "and says nothing else"
    );

    // The page itself still loads, or there would be nowhere to type the code —
    // and no other site may frame it.
    for path in ["/", "/setup", "/assets/index-abc.js"] {
        let (status, head, _) = server.request_full("GET", path, &[], &[]).await;
        assert_ne!(status, 401, "{path}");
        assert_eq!(header(&head, "x-frame-options").as_deref(), Some("SAMEORIGIN"), "{path}");
        assert_eq!(
            header(&head, "content-security-policy").as_deref(),
            Some("frame-ancestors 'self'"),
            "{path}"
        );
    }
    server.stop().await;
}

/// `auth.enabled = false` is the one open server left, and it is explicit.
#[tokio::test]
async fn with_authentication_switched_off_every_face_is_open() {
    let server = common::TestServer::start().await;
    for (method, path) in GUARDED {
        let (status, _) = server.request(method, path, &[], &[]).await;
        assert_ne!(status, 401, "{method} {path} must not need a token with authentication off");
    }
    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["authRequired"], false);
    assert_eq!(status["setupRequired"], false);
    assert_eq!(status["authOff"], true);
    let code_file = server.paths.instance_dir().join("setup-code");
    assert!(!code_file.exists(), "no setup state, so no code");
    let (status, body) = set_up(&server, Some("0000-0000-0000-0000"), "ada", PASSWORD).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"], "auth_off");
    server.stop().await;
}

/// `auth.enabled = true` is the default spelled out.
#[tokio::test]
async fn forced_on_is_the_default_spelled_out() {
    let home = tempfile::tempdir().expect("tempdir");
    let server = common::TestServer::start_forced_at(home.path()).await;
    assert_eq!(server.get("/api/settings").await.0, 401);
    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["authRequired"], true);
    assert_eq!(status["setupRequired"], true);
    let code = setup_code(&server);
    let (status, body) = set_up(&server, Some(&code), "ada", PASSWORD).await;
    assert_eq!(status, 200, "{body}");
    server.stop().await;
}

/// The setup code, as `setup-code` reads it back.
fn setup_code(server: &common::TestServer) -> String {
    let file = server.paths.instance_dir().join("setup-code");
    std::fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("{} should hold the setup code: {error}", file.display()))
        .trim()
        .to_string()
}

/// `POST /api/auth/setup`, with or without a code.
async fn set_up(server: &common::TestServer, code: Option<&str>, name: &str, password: &str) -> (u16, Value) {
    let body = match code {
        Some(code) => json!({ "code": code, "username": name, "password": password }),
        None => json!({ "username": name, "password": password }),
    };
    let (status, body) = server.json("POST", "/api/auth/setup", &body).await;
    (status, serde_json::from_str(&body).unwrap_or(Value::String(body)))
}

/// The code makes the first account once, signs the page in as it, and is
/// gone — memory and file — the moment it has.
#[tokio::test]
async fn the_setup_code_makes_the_first_account_once() {
    let server = common::TestServer::start_guarded().await;
    let file = server.paths.instance_dir().join("setup-code");
    let code = setup_code(&server);
    assert_eq!(code.len(), 19, "four groups of four: {code}");
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).expect("code file").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "readable only by the owner");
    }

    // The rules are checked before the code is spent.
    let (status, body) = set_up(&server, Some(&code), "ada", "short").await;
    assert_eq!((status, body["error"].as_str()), (400, Some("invalid_account")), "{body}");
    let (status, body) = set_up(&server, Some(&code), "Ada!", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("invalid_account")), "{body}");
    assert!(file.exists(), "a refusal on the rules leaves the code alone");

    // No code, no account — from this device as much as from anywhere.
    let (status, body) = set_up(&server, None, "ada", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("code_required")), "{body}");
    let (status, body) = set_up(&server, Some("0000-0000-0000-0000"), "ada", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (403, Some("wrong_setup_code")), "{body}");

    // Typed sloppily — lower case, no dashes — it is still the code.
    let typed = code.replace('-', " ").to_lowercase();
    let (status, body) = set_up(&server, Some(&typed), "ada", PASSWORD).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["username"], "ada");
    let token = body["token"].as_str().expect("a token").to_string();
    assert_eq!(get_as(&server, "/api/settings", &token).await.0, 200, "signed in as the new account");
    assert_eq!(server.get("/api/settings").await.0, 401, "and nobody else is");
    assert!(!file.exists(), "the code file goes with the code");

    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["setupRequired"], false);
    assert_eq!(status["authRequired"], true);

    let (status, body) = set_up(&server, Some(&code), "eve", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("setup_done")), "single use: {body}");
    assert_eq!(accounts(&server.home, DEFAULT_INSTANCE).list().await.len(), 1);
    // The new account logs in like any other.
    login(&server, "ada", PASSWORD).await;
    server.stop().await;
}

/// Wrong codes cost the caller time: a few are free, then each one makes the
/// next wait — even for the right code.
#[tokio::test]
async fn wrong_setup_codes_are_rate_limited() {
    let server = common::TestServer::start_guarded().await;
    let code = setup_code(&server);
    for _ in 0..3 {
        let (status, body) = set_up(&server, Some("0000-0000-0000-0000"), "ada", PASSWORD).await;
        assert_eq!(status, 403, "{body}");
    }
    let (status, body) = set_up(&server, Some(&code), "ada", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (429, Some("too_many_attempts")), "{body}");
    assert!(body["retryAfterSeconds"].as_u64().is_some_and(|s| s >= 1), "{body}");
    assert!(server.paths.instance_dir().join("setup-code").exists(), "waiting spends nothing");
    server.stop().await;
}

/// Each start in the setup state makes a new code; the old one dies with the
/// server that minted it.
#[tokio::test]
async fn every_start_in_the_setup_state_makes_a_new_code() {
    let home = tempfile::tempdir().expect("tempdir");
    let first = common::TestServer::start_guarded_at(home.path(), None).await;
    let old = setup_code(&first);
    first.stop().await;
    assert!(
        !home.path().join(".local/share/remote-hand/instances/default/setup-code").exists(),
        "the code file does not outlive its server"
    );

    let second = common::TestServer::start_guarded_at(home.path(), None).await;
    let new = setup_code(&second);
    assert_ne!(old, new);
    let (status, body) = set_up(&second, Some(&old), "ada", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (403, Some("wrong_setup_code")), "{body}");
    let (status, body) = set_up(&second, Some(&new), "ada", PASSWORD).await;
    assert_eq!(status, 200, "{body}");
    second.stop().await;
}

/// `remote-hand-server setup-code` shows the running server's code, and where
/// to type it.
#[tokio::test]
async fn the_setup_code_command_shows_the_running_servers_code() {
    let server = common::TestServer::start_guarded().await;
    let code = setup_code(&server);
    let out = cli(&server.home, &["setup-code"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(printed.contains(&code), "{printed}");
    assert!(
        printed.contains(&format!(":{}/", server.addr.port())),
        "and the page's address: {printed}"
    );

    accounts(&server.home, DEFAULT_INSTANCE).add("ada", PASSWORD).await.expect("user add");
    let out = cli(&server.home, &["setup-code"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already has an account"));
    server.stop().await;
}

/// The local capability end to end: `remote-hand-server open` asks the running
/// server over a socket only this user can reach for a one-time link, which
/// the page exchanges for a session of the device's owner — once.
#[tokio::test]
async fn remote_hand_server_open_signs_the_page_in_as_the_owner() {
    use std::os::unix::fs::PermissionsExt;
    let server = common::TestServer::start_guarded().await;
    let control = server.paths.instance_dir().join("control");
    assert_eq!(std::fs::metadata(&control).expect("control dir").permissions().mode() & 0o777, 0o700);
    let socket = remote_hand_server::auth::local_open::socket_path(&server.paths);
    assert_eq!(std::fs::metadata(&socket).expect("control socket").permissions().mode() & 0o777, 0o600);

    let out = cli_beside(&server, &["open", "--print"]).await;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    let url = printed.lines().next().expect("a link").to_string();
    assert!(url.starts_with(&format!("http://127.0.0.1:{}/#local-open=", server.addr.port())), "{url}");
    let token = url.split_once("#local-open=").expect("a token").1.to_string();

    let (status, body) = server.json("POST", "/api/auth/local-open", &json!({ "token": token })).await;
    assert_eq!(status, 200, "{body}");
    let owner: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(owner["username"], "local:owner");
    let session = owner["token"].as_str().expect("a session").to_string();
    assert_eq!(get_as(&server, "/api/settings", &session).await.0, 200);
    let status: Value = serde_json::from_str(&get_as(&server, "/api/auth/status", &session).await.1).expect("json");
    assert_eq!(status["user"], "local:owner");

    let (status, body) = server.json("POST", "/api/auth/local-open", &json!({ "token": token })).await;
    assert_eq!(status, 403, "single use: {body}");

    // The owner makes the first account without a code; its sessions end with it.
    let (status, body) = json_as(
        &server,
        "POST",
        "/api/auth/setup",
        &session,
        &json!({ "username": "ada", "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(get_as(&server, "/api/settings", &session).await.0, 401, "the owner's session is over");
    let out = cli_beside(&server, &["open", "--print"]).await;
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        !printed.contains("local-open="),
        "with an account there is only the login page: {printed}"
    );
    assert!(printed.contains("has an account"), "{printed}");
    server.stop().await;
}

/// The server binary as a command, on a test's home directory.
fn cli(home: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_remote-hand-server"))
        .args(args)
        .env("APP_HOME_DIR", home)
        .env("HOME", home)
        .env_remove("APP_INSTANCE")
        .env_remove("APP_AUTH__ENABLED")
        .env_remove("DROPPEN_CONFIG_DIR")
        .env_remove("DROPPEN_DATA_DIR")
        .env_remove("REMOTE_HAND_CONFIG_DIR")
        .env_remove("REMOTE_HAND_DATA_DIR")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run the server binary")
}

/// [`cli`] for a command that talks to the running test server: on a blocking
/// thread, so the server — on this test's runtime — can answer it.
async fn cli_beside(server: &common::TestServer, args: &[&str]) -> std::process::Output {
    let home = server.home.clone();
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        cli(&home, &args)
    })
    .await
    .expect("the command ran")
}

/// `agent enroll` will not link an instance that has no user account: a linked
/// device's page reaches other devices.
#[tokio::test]
async fn enrolling_without_a_user_account_is_refused() {
    let home = tempfile::tempdir().expect("tempdir");
    let code = rh_proto::identity::EnrollCode {
        account_fingerprint: [9; 16],
        secret: [7; 16],
    }
    .to_string();
    let enroll = ["agent", "enroll", code.as_str(), "--hub", "wss://hub.invalid", "--instance", "default"];
    let out = cli(home.path(), &enroll);
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("has no local account"), "{said}");
    assert!(said.contains("user add"), "{said}");
    assert!(said.contains("setup-code"), "{said}");
    assert!(
        !home.path().join(".local/share/remote-hand/instances/default/agent").exists(),
        "refused before anything was made"
    );

    // The desktop app's own account is not a user account either.
    accounts(home.path(), DEFAULT_INSTANCE)
        .add_app("droppen-app", PASSWORD)
        .await
        .expect("app account");
    let out = cli(home.path(), &enroll);
    assert!(String::from_utf8_lossy(&out.stderr).contains("has no local account"));
    // And `-` refuses before it asks for a code.
    let stdin = ["agent", "enroll", "-", "--hub", "wss://hub.invalid", "--instance", "default"];
    let out = cli(home.path(), &stdin);
    assert!(String::from_utf8_lossy(&out.stderr).contains("has no local account"));
}

/// An account made on the device — `user add` — ends the setup state just the
/// same, and the code that was showing stops working.
#[tokio::test]
async fn an_account_made_on_the_device_kills_the_live_code() {
    let server = common::TestServer::start_guarded().await;
    let code = setup_code(&server);
    accounts(&server.home, DEFAULT_INSTANCE).add("ada", PASSWORD).await.expect("user add");

    let (status, body) = set_up(&server, Some(&code), "eve", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("setup_done")), "{body}");
    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["setupRequired"], false);
    assert!(!server.paths.instance_dir().join("setup-code").exists());
    login(&server, "ada", PASSWORD).await;
    server.stop().await;
}

/// The desktop app's instance never leaves its setup state (its own account
/// is app-internal), so nothing a process of the user can reach by itself
/// hands out a way in: no setup code on file, and the control socket refuses.
/// The app's own login asks for the page link over HTTP, and the page that
/// trades it is the device's owner — who makes the first account.
#[tokio::test]
async fn the_desktop_apps_instance_opens_only_for_the_apps_own_login() {
    let home = tempfile::tempdir().expect("tempdir");
    accounts(home.path(), "droppen-app")
        .add_app("droppen-app", PASSWORD)
        .await
        .expect("app account");
    // A code an earlier build left on file: it goes at boot, and none takes
    // its place.
    let leftover = Paths::for_instance(home.path().to_path_buf(), "droppen-app").instance_dir().join("setup-code");
    std::fs::create_dir_all(leftover.parent().expect("the instance")).expect("instance dir");
    std::fs::write(&leftover, "AAAA-AAAA-AAAA-AAAA\n").expect("a leftover code");
    let server = common::TestServer::start_desktop_app_at(home.path(), "droppen-app").await;
    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!((status["setupRequired"].as_bool(), status["desktopApp"].as_bool()), (Some(true), Some(true)));
    assert_eq!(server.paths.instance_dir().join("setup-code"), leftover);
    assert!(!leftover.exists(), "no code on file: the leftover is gone, and none was minted");
    let (status, body) = set_up(&server, Some("AAAA-AAAA-AAAA-AAAA"), "eve", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("no_setup_code")), "{body}");

    // The control socket refuses, and mints nothing.
    let out = cli_beside(&server, &["open", "--print", "--instance", "droppen-app"]).await;
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("Open in Browser"), "{said}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("local-open="));

    // Nobody without the app's login gets a link; the app's own login does.
    assert_eq!(server.request("POST", "/api/auth/local-open/grant", &[], &[]).await.0, 401);
    let app = login(&server, "droppen-app", PASSWORD).await;
    let (status, body) = json_as(&server, "POST", "/api/auth/local-open/grant", &app, &json!({})).await;
    assert_eq!(status, 200, "{body}");
    let token = serde_json::from_str::<Value>(&body).expect("json")["token"]
        .as_str()
        .expect("a token")
        .to_string();
    let (status, body) = server.json("POST", "/api/auth/local-open", &json!({ "token": token })).await;
    assert_eq!(status, 200, "{body}");
    let owner: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(owner["username"], "local:owner");
    let owner = owner["token"].as_str().expect("a session").to_string();

    // The owner makes the first account without a code; then there is only
    // the login page to open.
    let (status, body) = json_as(
        &server,
        "POST",
        "/api/auth/setup",
        &owner,
        &json!({ "username": "ada", "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = json_as(&server, "POST", "/api/auth/local-open/grant", &app, &json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["token"], Value::Null);
    server.stop().await;
}

/// The desktop app's own account signs its window in, and is not a user
/// account: the app's server is never open without a credential, and the
/// instance stays in its setup state.
#[tokio::test]
async fn the_desktop_apps_own_account_opens_nothing_by_itself() {
    let home = tempfile::tempdir().expect("tempdir");
    accounts(home.path(), DEFAULT_INSTANCE)
        .add_app("droppen-app", PASSWORD)
        .await
        .expect("app account");
    let server = common::TestServer::start_guarded_at(home.path(), None).await;
    assert_eq!(server.get("/api/settings").await.0, 401);
    assert_eq!(raw_get(&server, "/api/settings", "localhost").await.0, 401);
    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["setupRequired"], true);
    let token = login(&server, "droppen-app", PASSWORD).await;
    assert_eq!(get_as(&server, "/api/settings", &token).await.0, 200);
    server.stop().await;
}

/// An instance that already has accounts starts exactly as it always did: no
/// setup state, no code, a login for every route — this device included.
#[tokio::test]
async fn an_instance_with_accounts_starts_as_it_always_did() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    // A file a crashed earlier run left behind is not a live code.
    let dir = home.path().join(".local/share/remote-hand/instances/default");
    std::fs::create_dir_all(&dir).expect("instance dir");
    std::fs::write(dir.join("setup-code"), "AAAA-AAAA-AAAA-AAAA\n").expect("stale code");
    let server = common::TestServer::start_guarded_at(home.path(), None).await;
    assert!(!dir.join("setup-code").exists(), "removed at boot");
    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["authRequired"], true);
    assert_eq!(status["setupRequired"], false);
    assert_eq!(server.get("/api/settings").await.0, 401, "this device too");
    let (status, body) = set_up(&server, Some("AAAA-AAAA-AAAA-AAAA"), "eve", PASSWORD).await;
    assert_eq!((status, body["error"].as_str()), (409, Some("setup_done")), "{body}");
    let token = login(&server, "ada", PASSWORD).await;
    assert_eq!(get_as(&server, "/api/settings", &token).await.0, 200);
    server.stop().await;
}

/// And the moment one exists, every one of those faces closes — including the
/// three that describe the API rather than being it.
#[tokio::test]
async fn the_first_account_closes_every_face_of_the_server() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    for (method, path) in GUARDED {
        let (status, body) = server.request(method, path, &[], &[]).await;
        assert_eq!(status, 401, "{method} {path} -> {status}: {body}");
        assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "unauthorized");
    }
    // `/scalar` too — it is the same document with a different renderer.
    assert_eq!(server.get("/scalar").await.0, 401);
    server.stop().await;
}

/// The one thing an unauthenticated browser must still be able to do: load the
/// page it is going to type the password into.
#[tokio::test]
async fn the_spa_shell_stays_reachable_so_the_login_page_can_be_drawn() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    for path in ["/", "/sessions/claude/x", "/assets/index-abc.js", "/sw.js"] {
        let (status, _) = server.get(path).await;
        assert!(status != 401, "{path} -> 401; there would be nowhere to log in");
    }
    // And the status route, which is how the page knows to draw a login form.
    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["authRequired"], true);
    assert_eq!(status["user"], Value::Null);
    server.stop().await;
}

#[tokio::test]
async fn logging_in_returns_a_token_that_works() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let (status, body) = server
        .json("POST", "/api/auth/login", &json!({ "username": "ada", "password": PASSWORD }))
        .await;
    assert_eq!(status, 200, "{body}");
    let parsed: Value = serde_json::from_str(&body).expect("json");
    let token = parsed["token"].as_str().expect("token");
    assert_eq!(token.len(), 43, "32 random bytes, base64url");
    assert_eq!(parsed["username"], "ada");
    assert!(parsed["expiresAt"].as_str().is_some_and(|when| when.ends_with('Z')));

    assert_eq!(get_as(&server, "/api/settings", token).await.0, 200);
    assert_eq!(get_as(&server, "/openapi.json", token).await.0, 200);

    let status: Value = serde_json::from_str(&get_as(&server, "/api/auth/status", token).await.1).expect("json");
    assert_eq!(status["user"], "ada");

    // The store never holds the token, only its digest — the property the whole
    // session design rests on, and the reason "list my sessions" cannot leak one
    // even by accident. Both columns, because both are derived from the same
    // record and a token in either would be the same disclosure.
    let stored = common::rows(&server.paths.instance_db(), "SELECT digest || ' ' || payload FROM auth_sessions", ());
    assert_eq!(stored.len(), 1, "one login, one row");
    assert!(!stored[0].contains(token), "a bearer token reached the store: {}", stored[0]);
    assert!(
        !server.paths.instance_dir().join("auth-sessions.json").exists(),
        "and nothing wrote a second copy of the digests beside it"
    );
    server.stop().await;
}

/// No such account, wrong password, and locked out are one answer with one
/// wording. Anything else is a way to enumerate the accounts on the machine.
#[tokio::test]
async fn every_refusal_on_the_login_path_is_the_same_refusal() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let attempts = [
        json!({ "username": "grace", "password": PASSWORD }),
        json!({ "username": "ada", "password": "the wrong long one" }),
        json!({ "username": "NOT A NAME", "password": PASSWORD }),
    ];
    for body in attempts {
        let (status, text) = server.json("POST", "/api/auth/login", &body).await;
        assert_eq!(status, 401, "{body} -> {text}");
        assert_eq!(serde_json::from_str::<Value>(&text).expect("json")["error"], "invalid credentials");
    }
    server.stop().await;
}

/// Five wrong ones and the right one stops working too — with, again, the same
/// wording, so the lockout is not itself an oracle.
#[tokio::test]
async fn five_wrong_passwords_lock_the_account_even_for_the_right_one() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    for _ in 0..5 {
        let (status, _) = server
            .json("POST", "/api/auth/login", &json!({ "username": "ada", "password": "wrong but long" }))
            .await;
        assert_eq!(status, 401);
    }
    let (status, body) = server
        .json("POST", "/api/auth/login", &json!({ "username": "ada", "password": PASSWORD }))
        .await;
    assert_eq!(status, 401, "the correct password is refused while the account is locked");
    assert_eq!(serde_json::from_str::<Value>(&body).expect("json")["error"], "invalid credentials");
    // That the lock lets go by itself is a clock question, pinned down in
    // `auth::lockout`'s own tests where the clock is an argument.
    server.stop().await;
}

/// The allowance is five guesses, not five *sequential* guesses.
///
/// Checking the lock, hashing the password and recording what the hash said is
/// a check-then-act with ~100 ms of argon2id in the middle, so without a gate
/// across all three every request that arrived inside that window passed a
/// check none of them had yet had the chance to fail — twenty at once got
/// twenty attempts. This fires them simultaneously and counts how many the
/// server was willing to check.
#[tokio::test]
async fn twenty_simultaneous_guesses_still_only_get_the_five() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    // Nineteen wrong and one right, all in flight together. If the guesses were
    // not serialized against the counter, the right one would be checked and
    // answered with a token despite the allowance having run out.
    let mut attempts = tokio::task::JoinSet::new();
    for index in 0..19 {
        let address = server.addr;
        attempts.spawn(async move { login_at(address, &format!("wrong but long {index}")).await });
    }
    let address = server.addr;
    attempts.spawn(async move { login_at(address, PASSWORD).await });
    let mut accepted = 0;
    while let Some(result) = attempts.join_next().await {
        if result.expect("join").0 == 200 {
            accepted += 1;
        }
    }
    assert_eq!(accepted, 0, "the right password was inside the burst and the allowance had run out");

    // And the account really is locked now, rather than merely having answered
    // twenty times.
    let (status, _) = server
        .json("POST", "/api/auth/login", &json!({ "username": "ada", "password": PASSWORD }))
        .await;
    assert_eq!(status, 401);
    server.stop().await;
}

/// The other half of the same gate: simultaneous *correct* logins queue, they
/// do not eat the allowance. Counting an attempt up front instead of gating
/// would have locked an account out for six honest logins at once.
#[tokio::test]
async fn simultaneous_correct_logins_all_succeed() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let mut logins = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let address = server.addr;
        logins.spawn(async move { login_at(address, PASSWORD).await });
    }
    let mut tokens = Vec::new();
    while let Some(result) = logins.join_next().await {
        let (status, body) = result.expect("join");
        assert_eq!(status, 200, "an honest login was refused: {body}");
        tokens.push(
            serde_json::from_str::<Value>(&body).expect("json")["token"]
                .as_str()
                .expect("token")
                .to_string(),
        );
    }
    // Eight logins, eight distinct sessions, and every one of them works.
    tokens.sort();
    tokens.dedup();
    assert_eq!(tokens.len(), 8);
    for token in &tokens {
        assert_eq!(get_as(&server, "/api/settings", token).await.0, 200);
    }
    server.stop().await;
}

/// Absolute expiry. The stored expiry is dragged into the past rather than
/// waiting a week for it, which also proves the check reads the store on every
/// request rather than trusting something it cached at login.
///
/// The one test here that reaches into a row, and deliberately: no route can
/// move an expiry backwards — that is the entire point of an absolute one — so
/// there is no window onto this from the outside.
#[tokio::test]
async fn a_token_stops_working_when_its_session_expires() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;
    let token = login(&server, "ada", PASSWORD).await;
    assert_eq!(get_as(&server, "/api/settings", &token).await.0, 200);

    // The `expires_at` column rather than the ISO `expiresAt` in the payload
    // beside it: the column is what every read filters on, in milliseconds, and
    // the stamp in the payload is what a client is shown.
    let changed = common::edit(&server.paths.instance_db(), "UPDATE auth_sessions SET expires_at = 0", ());
    assert_eq!(changed, 1, "one login to expire");

    assert_eq!(get_as(&server, "/api/settings", &token).await.0, 401);
    server.stop().await;
}

#[tokio::test]
async fn logging_out_revokes_the_token_it_arrived_with() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let phone = login(&server, "ada", PASSWORD).await;
    let laptop = login(&server, "ada", PASSWORD).await;
    let (status, _) = json_as(&server, "POST", "/api/auth/logout", &phone, &json!({})).await;
    assert_eq!(status, 200);

    assert_eq!(get_as(&server, "/api/settings", &phone).await.0, 401);
    assert_eq!(
        get_as(&server, "/api/settings", &laptop).await.0,
        200,
        "one device logging out is not all of them"
    );
    server.stop().await;
}

/// The list is how a person finds the device they left on a train, and the
/// `DELETE` is how they deal with it. Neither may reach anybody else's.
#[tokio::test]
async fn a_session_can_be_revoked_by_id_and_only_by_its_owner() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    seed_account(home.path(), "grace", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let here = login(&server, "ada", PASSWORD).await;
    let train = login(&server, "ada", PASSWORD).await;
    let someone_else = login(&server, "grace", PASSWORD).await;

    let listed: Value = serde_json::from_str(&get_as(&server, "/api/auth/sessions", &here).await.1).expect("json");
    let sessions = listed["sessions"].as_array().expect("array");
    assert_eq!(sessions.len(), 2, "ada's two, and not grace's");
    assert!(sessions.iter().all(|session| session["username"] == "ada"));
    assert_eq!(sessions.iter().filter(|session| session["current"] == true).count(), 1);
    let lost = sessions
        .iter()
        .find(|session| session["current"] == false)
        .and_then(|session| session["id"].as_str())
        .expect("the other session");
    assert!(!listed.to_string().contains(&train), "a listing must never contain a token");

    // grace cannot revoke it, and is told nothing about whether it exists.
    let authorization = format!("Bearer {someone_else}");
    let (status, _) = server
        .request("DELETE", &format!("/api/auth/sessions/{lost}"), &[("Authorization", &authorization)], &[])
        .await;
    assert_eq!(status, 404);
    assert_eq!(get_as(&server, "/api/settings", &train).await.0, 200, "and it still works");

    let authorization = format!("Bearer {here}");
    let (status, _) = server
        .request("DELETE", &format!("/api/auth/sessions/{lost}"), &[("Authorization", &authorization)], &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(get_as(&server, "/api/settings", &train).await.0, 401);
    assert_eq!(get_as(&server, "/api/settings", &here).await.0, 200);
    server.stop().await;
}

#[tokio::test]
async fn changing_the_password_evicts_every_other_device() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let here = login(&server, "ada", PASSWORD).await;
    let elsewhere = login(&server, "ada", PASSWORD).await;

    let body = json!({ "currentPassword": PASSWORD, "newPassword": "a different long one" });
    let (status, text) = json_as(&server, "POST", "/api/auth/password", &here, &body).await;
    assert_eq!(status, 200, "{text}");

    assert_eq!(
        get_as(&server, "/api/settings", &here).await.0,
        200,
        "the device that asked stays logged in"
    );
    assert_eq!(get_as(&server, "/api/settings", &elsewhere).await.0, 401);

    // And the password really changed.
    let (status, _) = server
        .json("POST", "/api/auth/login", &json!({ "username": "ada", "password": PASSWORD }))
        .await;
    assert_eq!(status, 401);
    login(&server, "ada", "a different long one").await;
    server.stop().await;
}

/// The password rules reach the API, one refusal per rule, and the *current*
/// password is required even from an authenticated caller.
#[tokio::test]
async fn a_password_change_enforces_every_rule() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;
    let token = login(&server, "ada", PASSWORD).await;

    let refused = [("short", "at least"), (" leading whitespace ", "whitespace"), ("123456789012", "common")];
    for (candidate, expected) in refused {
        let body = json!({ "currentPassword": PASSWORD, "newPassword": candidate });
        let (status, text) = json_as(&server, "POST", "/api/auth/password", &token, &body).await;
        assert_eq!(status, 400, "{candidate:?} -> {status}: {text}");
        assert!(text.contains(expected), "{candidate:?} -> {text}");
    }

    let body = json!({ "currentPassword": "not the current one", "newPassword": "a fine new password" });
    let (status, text) = json_as(&server, "POST", "/api/auth/password", &token, &body).await;
    assert_eq!(status, 401, "a token on an unlocked laptop is not enough to take the account over");
    assert_eq!(serde_json::from_str::<Value>(&text).expect("json")["error"], "invalid credentials");
    server.stop().await;
}

/// A websocket cannot carry a header, so it carries a ticket — one that is spent
/// by the handshake and worth nothing anywhere else.
#[tokio::test]
async fn a_ws_ticket_opens_one_socket_and_only_one() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;
    let token = login(&server, "ada", PASSWORD).await;

    // Without one, the upgrade is refused before it is an upgrade.
    let plain = tokio_tungstenite::connect_async(server.ws_url("/api/events")).await;
    assert!(plain.is_err(), "an unauthenticated upgrade must not succeed");

    let (status, body) = json_as(&server, "POST", "/api/auth/ticket", &token, &json!({ "scope": "ws" })).await;
    assert_eq!(status, 200, "{body}");
    let ticket = serde_json::from_str::<Value>(&body).expect("json")["ticket"]
        .as_str()
        .expect("ticket")
        .to_string();

    let url = server.ws_url(&format!("/api/events?ticket={ticket}"));
    let (socket, _) = tokio_tungstenite::connect_async(&url).await.expect("the ticket opens the socket");
    drop(socket);

    let again = tokio_tungstenite::connect_async(&url).await;
    assert!(again.is_err(), "a ws ticket is spent by the handshake that used it");
    server.stop().await;
}

/// Scopes are what keep a thumbnail URL from being a way to open a shell.
#[tokio::test]
async fn a_download_ticket_is_worthless_anywhere_but_a_download() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    std::fs::write(home.path().join("note.txt"), "hello").expect("write");
    let server = common::TestServer::start_guarded_at(home.path(), None).await;
    let token = login(&server, "ada", PASSWORD).await;

    let (_, body) = json_as(&server, "POST", "/api/auth/ticket", &token, &json!({ "scope": "download" })).await;
    let ticket = serde_json::from_str::<Value>(&body).expect("json")["ticket"]
        .as_str()
        .expect("ticket")
        .to_string();

    let file = home.path().join("note.txt");
    let file = file.to_string_lossy();
    let (status, body) = server.get(&format!("/api/fs/download?path={file}&ticket={ticket}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "hello");
    // Reusable, unlike a ws ticket: a page of thumbnails is many requests.
    assert_eq!(server.get(&format!("/api/fs/download?path={file}&ticket={ticket}")).await.0, 200);

    // Everywhere else the parameter is simply not looked at.
    assert_eq!(server.get(&format!("/api/fs/read?path={file}&ticket={ticket}")).await.0, 401);
    assert_eq!(server.get(&format!("/api/settings?ticket={ticket}")).await.0, 401);
    let ws = tokio_tungstenite::connect_async(server.ws_url(&format!("/api/events?ticket={ticket}"))).await;
    assert!(ws.is_err(), "a download ticket is not a websocket ticket");
    server.stop().await;
}

/// An inline preview renders and does nothing else. Without the sandbox, an SVG
/// an agent wrote would run on this origin and read the token out of it — which
/// is the direct cost of keeping a bearer token in the page.
#[tokio::test]
async fn an_inline_preview_is_sandboxed() {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("logo.svg"), "<svg xmlns='http://www.w3.org/2000/svg'/>").expect("write");
    // About the answer, not about who may ask: authentication off.
    let server = common::TestServer::start_at(home.path(), None).await;

    let file = home.path().join("logo.svg");
    let file = file.to_string_lossy();
    let (status, head, _) = server
        .request_full("GET", &format!("/api/fs/download?path={file}&inline=true"), &[], &[])
        .await;
    assert_eq!(status, 200);
    assert_eq!(header(&head, "content-type").as_deref(), Some("image/svg+xml"));
    let policy = header(&head, "content-security-policy").expect("a sandbox");
    assert!(policy.contains("sandbox"), "{policy}");
    assert_eq!(header(&head, "x-content-type-options").as_deref(), Some("nosniff"));
    server.stop().await;
}

/// The exemption is decided on `uri.path()` *verbatim* — axum normalises
/// nothing, and neither does this — so the question that matters is not "does
/// the guard normalise" but "can the exemption and the router ever disagree
/// about what a path is". They cannot, and in one direction only: every variant
/// below dodges the `/api/` prefix and is therefore exempt as a static asset,
/// and every one of them also fails to match an API route, so it lands on the
/// SPA shell — which is public by design and is what an unauthenticated browser
/// is *supposed* to get.
///
/// This is the load-bearing property of the whole guard, so it is pinned rather
/// than reasoned about: if a future router ever grew path normalisation, one of
/// these would start returning JSON and this test would say so.
#[tokio::test]
async fn a_path_that_dodges_the_api_prefix_reaches_the_shell_and_never_the_api() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    // Every way of spelling `/api/settings` (and `/openapi.json`) that is not
    // literally that string. A `200` here is the SPA; what must never appear is
    // the JSON the real route answers with.
    let variants = [
        "//api/settings",
        "///api/settings",
        "/%61pi/settings",
        "/API/settings",
        "/./api/settings",
        "/embed/../api/settings",
        "/api",
        "//openapi.json",
        "/openapi.json/",
        "/%6fpenapi.json",
    ];
    for path in variants {
        let (status, body) = server.get(path).await;
        assert!(status == 401 || body.starts_with("<!doctype html>"), "GET {path} -> {status}: {body}");
        assert!(
            !body.contains("\"instance\"") && !body.contains("\"openapi\""),
            "GET {path} answered with the document it was spelled around: {body}"
        );
    }

    // The percent-encoding only ever loses: an encoded byte *inside* an
    // otherwise-real API path is still not that path, so it is guarded.
    assert_eq!(server.get("/api/%73ettings").await.0, 401);
    // `HEAD` is exempt for the same reason `GET` is, and carved back out for the
    // documentation for the same reason too.
    for path in ["/api/settings", "/openapi.json", "/redoc"] {
        assert_eq!(server.request("HEAD", path, &[], &[]).await.0, 401, "HEAD {path}");
    }
    // And a method that is not `GET`/`HEAD` is guarded wherever it lands,
    // including on the two fallbacks.
    for (method, path) in [("POST", "/api/nonexistent"), ("POST", "/nonexistent"), ("PUT", "/nonexistent")] {
        assert_eq!(server.request(method, path, &[], &[]).await.0, 401, "{method} {path}");
    }
    server.stop().await;
}

/// A ticket is read on three websocket paths and one download path, and the
/// websocket three only when the request really is a handshake. Neither half is
/// forgeable from the other: an `Upgrade: websocket` header on a route that is
/// not one of the three buys nothing, because the path list is checked first.
#[tokio::test]
async fn a_forged_upgrade_header_does_not_open_the_ticket_branch() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let token = login(&server, "ada", PASSWORD).await;
    let minted: Value = serde_json::from_str(&json_as(&server, "POST", "/api/auth/ticket", &token, &json!({ "scope": "ws" })).await.1).expect("json");
    let ticket = minted["ticket"].as_str().expect("ticket");

    // A real ws ticket, on a real route, with the handshake header — and still
    // refused, because the route is not one of the three that take one.
    for path in ["/api/settings", "/api/fs/read?path=%2Fetc%2Fhosts", "/openapi.json"] {
        let target = format!("{path}{}ticket={ticket}", if path.contains('?') { "&" } else { "?" });
        let (status, body) = server.request("GET", &target, &[("Upgrade", "websocket")], &[]).await;
        assert_eq!(status, 401, "GET {target} with a forged Upgrade -> {status}: {body}");
    }
    // And the ticket is still unspent, because none of that reached the table.
    let (status, _) = server.request("GET", &format!("/api/events?ticket={ticket}"), &[], &[]).await;
    assert_eq!(status, 401, "a plain GET of a websocket path is not a handshake either");
    server.stop().await;
}

/// The `/embed/{id}/*` proxy is a capability URL and is left outside the guard;
/// what proves the exemption is that an unknown id is a `404` from the proxy
/// rather than a `401` from the guard.
#[tokio::test]
async fn the_embed_proxy_answers_without_a_token() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let (status, _) = server.get("/embed/00000000-0000-4000-8000-000000000000/index.html").await;
    assert_eq!(status, 404, "the proxy answered, so the guard let it through");
    server.stop().await;
}

/// A browser strips credentials from a preflight, so a `401` there would break
/// every cross-origin request before the real one was ever sent.
#[tokio::test]
async fn a_preflight_is_answered_without_a_token() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let origin = format!("http://{}", server.addr);
    let (status, _, _) = server
        .request_full(
            "OPTIONS",
            "/api/fs/list",
            &[("Origin", &origin), ("Access-Control-Request-Headers", "authorization")],
            &[],
        )
        .await;
    assert_eq!(status, 204, "the CORS layer behind the guard still answers it");
    server.stop().await;
}

#[tokio::test]
async fn every_response_says_not_to_leak_the_url_it_came_from() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;
    let token = login(&server, "ada", PASSWORD).await;

    let authorization = format!("Bearer {token}");
    for (path, headers) in [("/api/settings", &[("Authorization", authorization.as_str())][..]), ("/", &[][..])] {
        let (_, head, _) = server.request_full("GET", path, headers, &[]).await;
        assert_eq!(header(&head, "referrer-policy").as_deref(), Some("no-referrer"), "{path}");
    }
    // Including the refusal itself.
    let (status, head, _) = server.request_full("GET", "/api/settings", &[], &[]).await;
    assert_eq!(status, 401);
    assert_eq!(header(&head, "referrer-policy").as_deref(), Some("no-referrer"));
    server.stop().await;
}

/// Deleting an account takes its logins with it: the token check re-confirms
/// the name against this instance's store on every request, so the sessions are
/// dead the moment the row is.
#[tokio::test]
async fn removing_an_account_kills_its_sessions_immediately() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    seed_account(home.path(), "grace", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;

    let ada = login(&server, "ada", PASSWORD).await;
    let grace = login(&server, "grace", PASSWORD).await;
    assert_eq!(get_as(&server, "/api/settings", &ada).await.0, 200);

    accounts(home.path(), DEFAULT_INSTANCE).remove("ada").await.expect("remove");

    assert_eq!(get_as(&server, "/api/settings", &ada).await.0, 401);
    assert_eq!(get_as(&server, "/api/settings", &grace).await.0, 200);
    server.stop().await;
}

/// Removing the *last* account takes the instance back to where it started:
/// the setup state, with a fresh code, and no way in for a dead token.
#[tokio::test]
async fn removing_the_last_account_puts_the_server_back_in_its_setup_state() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_account(home.path(), "ada", PASSWORD).await;
    let server = common::TestServer::start_guarded_at(home.path(), None).await;
    let token = login(&server, "ada", PASSWORD).await;
    assert_eq!(server.get("/api/settings").await.0, 401);
    assert!(!server.paths.instance_dir().join("setup-code").exists());

    accounts(home.path(), DEFAULT_INSTANCE).remove("ada").await.expect("remove");

    assert_eq!(server.get("/api/settings").await.0, 401);
    assert_eq!(
        get_as(&server, "/api/settings", &token).await.0,
        401,
        "the removed account's token is dead"
    );
    let status: Value = server.get_json("/api/auth/status").await;
    assert_eq!(status["authRequired"], true);
    assert_eq!(status["setupRequired"], true);
    // The running server notices within a tick and prints a fresh code.
    let file = server.paths.instance_dir().join("setup-code");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !file.exists() {
        assert!(std::time::Instant::now() < deadline, "no setup code appeared after the last account went");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let code = setup_code(&server);
    let (status, body) = set_up(&server, Some(&code), "grace", PASSWORD).await;
    assert_eq!(status, 200, "{body}");
    server.stop().await;
}

/// Two servers, one account inherited by both, and no token that crosses
/// between them.
///
/// The account is seeded machine-wide — where every build before this one kept
/// it — so this is also the upgrade path end to end: two instances that both
/// take the password the machine had, and still hand each other nothing.
#[tokio::test]
async fn a_token_is_a_claim_on_one_instance() {
    let home = tempfile::tempdir().expect("tempdir");
    UsersStore::at(home.path().join(".config/remote-hand/users.json"))
        .add("ada", PASSWORD)
        .await
        .expect("seed the machine-wide accounts an upgrade would find");
    let first = common::TestServer::start_guarded_at(home.path(), None).await;
    let second = common::TestServer::start_guarded_at(home.path(), Some("staging")).await;

    let token = login(&first, "ada", PASSWORD).await;
    assert_eq!(get_as(&first, "/api/settings", &token).await.0, 200);
    assert_eq!(
        get_as(&second, "/api/settings", &token).await.0,
        401,
        "the same account, a different server"
    );

    // And the second instance inherited the same account, so the same password
    // still opens it — with a token of its own.
    let other = login(&second, "ada", PASSWORD).await;
    assert_eq!(get_as(&second, "/api/settings", &other).await.0, 200);
    second.stop().await;
    first.stop().await;
}
