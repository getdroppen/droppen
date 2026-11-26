//! The proxy from the outside, end to end: an HTTP client against its
//! listener, a real relay, hub and host's agent behind it (droppen-client's
//! stand). A session is started, remembered, messaged and interrupted; a
//! straight-through `/rpc` goes on; and each failure is judged by the proxy's
//! own chain.

#[path = "../../droppen-client/tests/support/mod.rs"]
mod support;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use droppen_proto::chain::{machine_key_commit, Accept, Action, DeviceV3, Flags, KeyCommit, KeyCommits};
use droppen_proto::identity::EnrollCode;
use droppen_session_proxy::server::Proxy;
use droppen_session_proxy::upstream::{DeviceUpstream, Upstream};
use droppen_session_proxy::Config;
use serde_json::{json, Value};

const TOKEN: &str = "0123456789abcdef0123456789abcdef-test";

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// A proxy in front of `upstream` on a free port.
async fn proxy(upstream: Arc<dyn Upstream>) -> SocketAddr {
    let proxy = Arc::new(Proxy::new(upstream, TOKEN));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(droppen_session_proxy::serve(listener, proxy, std::future::pending()));
    addr
}

/// The device under test is added to `stand`'s account as a normal device the
/// host accepts, and its view brought up.
async fn join_as_device(stand: &mut support::Stand) {
    let (sign, dh) = support::keys(3);
    stand.append(
        vec![Action::AddDevice3(DeviceV3 {
            id: support::PROXY.into(),
            sign_key: sign.verifying_key().to_bytes(),
            dh_key: dh.public(),
            flags: Flags::NONE,
        })],
        true,
    );
    stand.append(
        vec![
            Action::SetAccept {
                host: support::HOST.into(),
                from: Accept::Devices(vec![support::PROXY.into()]),
            },
            support::Account::host_key(1),
        ],
        true,
    );
    stand.device.sync_chain().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_is_started_remembered_and_carried() {
    let seen: Arc<Mutex<Vec<(String, String, String)>>> = Arc::default();
    let seen_in_handler = seen.clone();
    let serve = support::handler(move |request: support::Request, responder: support::Responder| {
        let seen = seen_in_handler.clone();
        async move {
            let body = String::from_utf8_lossy(&request.body).into_owned();
            seen.lock().unwrap().push((request.head.method.clone(), request.head.path.clone(), body));
            if request.head.path == "/api/sessions/new" {
                responder.json(200, &json!({"id": "sess-1"})).await;
            } else {
                responder.json(200, &json!({"ok": true})).await;
            }
        }
    });
    let mut config = support::StandConfig::new(serve);
    config.proxy_in_chain = false;
    let mut stand = support::Stand::with(config).await;
    join_as_device(&mut stand).await;
    let addr = proxy(Arc::new(DeviceUpstream::new(stand.device.clone()))).await;
    let http = client();

    // Start: `device` is taken off and `source` forced.
    let response = http
        .post(format!("http://{addr}/sessions"))
        .bearer_auth(TOKEN)
        .json(&json!({"device": support::HOST, "cwd": "/tmp", "text": "hi", "clientMessageId": "c-1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap(), json!({"id": "sess-1"}));
    {
        let seen = seen.lock().unwrap();
        let (method, path, body) = &seen[0];
        assert_eq!((method.as_str(), path.as_str()), ("POST", "/api/sessions/new"));
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["source"], "claude");
        assert!(body.get("device").is_none(), "{body}");
        assert_eq!(body["cwd"], "/tmp");
    }

    // Message and interrupt: both reach the session's device.
    let response = http
        .post(format!("http://{addr}/sessions/sess-1/message"))
        .bearer_auth(TOKEN)
        .json(&json!({"text": "next"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = http
        .post(format!("http://{addr}/sessions/sess-1/interrupt"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // An answer is re-shaped into the question route.
    let response = http
        .post(format!("http://{addr}/sessions/sess-1/answer"))
        .bearer_auth(TOKEN)
        .json(&json!({"requestId": "q-1", "answers": {"Which?": "this"}, "notes": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    {
        let seen = seen.lock().unwrap();
        let paths: Vec<&str> = seen.iter().map(|(_, path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/api/sessions/new",
                "/api/sessions/claude/sess-1/message",
                "/api/sessions/claude/sess-1/interrupt",
                "/api/sessions/claude/sess-1/questions/q-1/answer",
            ]
        );
        let body: Value = serde_json::from_str(&seen[3].2).unwrap();
        assert_eq!(body, json!({"answers": {"Which?": "this"}, "notes": {}}), "requestId and dismiss do not go on");
    }

    // A dismiss names the dismiss route.
    let response = http
        .post(format!("http://{addr}/sessions/sess-1/answer"))
        .bearer_auth(TOKEN)
        .json(&json!({"requestId": "q-2", "dismiss": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(seen.lock().unwrap()[4].1, "/api/sessions/claude/sess-1/questions/q-2/dismiss");

    // `/rpc` straight through, and an id it never started is refused.
    let response = http
        .post(format!("http://{addr}/rpc"))
        .bearer_auth(TOKEN)
        .json(&json!({"device": support::HOST, "method": "GET", "path": "/api/sessions/claude/sess-1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(seen.lock().unwrap()[5].1, "/api/sessions/claude/sess-1");
    let response = http
        .post(format!("http://{addr}/rpc"))
        .bearer_auth(TOKEN)
        .json(&json!({"device": support::HOST, "method": "GET", "path": "/api/sessions/claude/never/message"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.json::<Value>().await.unwrap()["error"], "unknown_session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_token_is_required_and_a_bad_path_is_refused() {
    let stand = support::Stand::new(support::echo()).await;
    let addr = proxy(Arc::new(DeviceUpstream::new(stand.device.clone()))).await;
    let http = client();
    assert_eq!(http.get(format!("http://{addr}/healthz")).send().await.unwrap().status(), 200);
    assert_eq!(http.get(format!("http://{addr}/sessions")).send().await.unwrap().status(), 401);
    for path in ["/a2a/x", "/api/../x", "/api//sessions", "/api/sess%2fions"] {
        let response = http
            .post(format!("http://{addr}/rpc"))
            .bearer_auth(TOKEN)
            .json(&json!({"device": support::HOST, "method": "GET", "path": path}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{path}");
    }
    let response = http
        .post(format!("http://{addr}/rpc"))
        .bearer_auth(TOKEN)
        .json(&json!({"device": support::HOST, "method": "get", "path": "/api/sessions"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400, "a lowercase method");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_failure_is_judged_by_the_proxys_own_chain() {
    // The host on no relay: offline.
    let mut config = support::StandConfig::new(support::echo());
    config.proxy_in_chain = false;
    config.attach = false;
    let mut stand = support::Stand::with(config).await;
    join_as_device(&mut stand).await;
    let addr = proxy(Arc::new(DeviceUpstream::new(stand.device.clone()))).await;
    let http = client();
    let start = |addr: SocketAddr| {
        http.post(format!("http://{addr}/sessions"))
            .bearer_auth(TOKEN)
            .json(&json!({"device": support::HOST, "cwd": "/tmp", "text": "hi"}))
    };
    let response = start(addr).send().await.unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.json::<Value>().await.unwrap()["error"], "host_offline");

    // The host's accept list no longer names the proxy: not accepting.
    stand.append(
        vec![
            Action::SetAccept {
                host: support::HOST.into(),
                from: Accept::Admins,
            },
            support::Account::host_key(2),
        ],
        true,
    );
    stand.device.sync_chain().await.unwrap();
    let response = start(addr).send().await.unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(response.json::<Value>().await.unwrap()["error"], "host_not_accepting");

    // The proxy removed from the account: gone.
    stand.append(vec![Action::RemoveDevice3(support::PROXY.into())], true);
    stand.device.sync_chain().await.unwrap();
    let response = start(addr).send().await.unwrap();
    assert_eq!(response.status(), 410);
    assert_eq!(response.json::<Value>().await.unwrap()["error"], "device_removed");
}

/// The proxy's configuration: data in `dir`, at `hub`, maybe a code.
fn config(dir: &std::path::Path, hub: &str, code: Option<&str>) -> Config {
    let vars = [
        ("DROPPEN_DATA_DIR", dir.display().to_string()),
        ("DROPPEN_SESSION_PROXY_TOKEN", TOKEN.to_string()),
        ("DROPPEN_HUB", hub.to_string()),
        ("PORT", "0".to_string()),
        ("DROPPEN_ENROLL_CODE", code.unwrap_or_default().to_string()),
    ];
    Config::from_vars(|name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone())).unwrap()
}

/// The page's part of adding a device, for one that proved `code`: the proof
/// checked, then added as a host the existing host accepts.
async fn page_adds(stand: &mut support::Stand, code: &EnrollCode) -> String {
    let token = code.token();
    let mut proved = None;
    assert!(
        support::eventually(std::time::Duration::from_secs(10), || {
            proved = stand.hub.proved(&token);
            proved.is_some()
        })
        .await,
        "the proxy proved the code"
    );
    let p = proved.unwrap();
    assert!(
        code.verify_proof(support::ACCOUNT, &p.device, &p.sign_key, &p.dh_key, &p.proof),
        "the proof holds"
    );
    stand.append(
        vec![
            Action::AddDevice3(DeviceV3 {
                id: p.device.clone(),
                sign_key: p.sign_key,
                dh_key: p.dh_key,
                flags: Flags {
                    host: true,
                    admin: false,
                    a2a: false,
                },
            }),
            // A new host's machine key epoch must come next.
            Action::KeyEpochs(KeyCommits {
                user: None,
                machines: vec![(
                    p.device.clone(),
                    KeyCommit {
                        epoch: 0,
                        hash: machine_key_commit(support::ACCOUNT, &p.device, 0, &[8; 32]),
                    },
                )],
            }),
        ],
        true,
    );
    stand.append(
        vec![
            Action::SetAccept {
                host: support::HOST.into(),
                from: Accept::Devices(vec![p.device.clone()]),
            },
            support::Account::host_key(1),
        ],
        true,
    );
    p.device
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_to_end_the_proxy_joins_with_an_rh1_code_as_a_host() {
    let mut cfg = support::StandConfig::new(support::echo());
    cfg.proxy_in_chain = false;
    let mut stand = support::Stand::with(cfg).await;
    let code = EnrollCode {
        account_fingerprint: stand.account.fingerprint,
        secret: [7; 16],
    };
    stand.hub.register_code(&code.token(), "machine");
    let dir = support::scratch("join");
    let (hub, text) = (stand.hub.url.clone(), code.to_string());
    let joining = {
        let dir = dir.clone();
        tokio::spawn(async move { droppen_session_proxy::enroll::open_or_join(&config(&dir, &hub, Some(&text)), support::device_options()).await })
    };
    let added = page_adds(&mut stand, &code).await;
    let device = tokio::time::timeout(std::time::Duration::from_secs(20), joining).await.unwrap().unwrap().unwrap();
    assert_eq!(device.identity().device, added);
    assert!(device.with_chain(|c| c.unwrap().may_control(&added, support::HOST).is_ok()));
    #[cfg(unix)]
    for file in ["identity.json", "chain.json"] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join(file)).unwrap().permissions().mode() & 0o777, 0o600, "{file}");
    }
    // It carries a request through the relay to the host now.
    let addr = proxy(Arc::new(DeviceUpstream::new(Arc::new(device)))).await;
    let response = client()
        .post(format!("http://{addr}/rpc"))
        .bearer_auth(TOKEN)
        .json(&json!({"device": support::HOST, "method": "GET", "path": "/api/sessions"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    // A restart with the code still set: the same device, the code ignored.
    let again = droppen_session_proxy::enroll::open_or_join(&config(&dir, &stand.hub.url, Some(&code.to_string())), support::device_options())
        .await
        .unwrap();
    assert_eq!(again.identity().device, added);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_to_end_the_proxy_refuses_a_device_added_without_the_host_flag() {
    let mut cfg = support::StandConfig::new(support::echo());
    cfg.proxy_in_chain = false;
    let mut stand = support::Stand::with(cfg).await;
    let code = EnrollCode {
        account_fingerprint: stand.account.fingerprint,
        secret: [9; 16],
    };
    stand.hub.register_code(&code.token(), "machine");
    let dir = support::scratch("join-plain");
    let (hub, text) = (stand.hub.url.clone(), code.to_string());
    let joining = {
        let dir = dir.clone();
        tokio::spawn(async move { droppen_session_proxy::enroll::open_or_join(&config(&dir, &hub, Some(&text)), support::device_options()).await })
    };
    let token = code.token();
    let mut proved = None;
    assert!(support::eventually(std::time::Duration::from_secs(10), || {
        proved = stand.hub.proved(&token);
        proved.is_some()
    })
    .await);
    let p = proved.unwrap();
    // A plain device (no host flag).
    stand.append(
        vec![Action::AddDevice3(DeviceV3 {
            id: p.device.clone(),
            sign_key: p.sign_key,
            dh_key: p.dh_key,
            flags: Flags::NONE,
        })],
        true,
    );
    let error = tokio::time::timeout(std::time::Duration::from_secs(20), joining)
        .await
        .unwrap()
        .unwrap()
        .err()
        .unwrap();
    assert!(error.contains("without the host flag"), "{error}");
}
