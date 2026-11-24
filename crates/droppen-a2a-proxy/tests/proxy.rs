//! The proxy from the outside: an HTTP client against its listener. First
//! with a stand-in for the devices behind it (the token, the routes, what is
//! carried and what is not, the Agent Card, each error in both bindings),
//! then end to end through a relay to a host's agent, on droppen-client's
//! stand (`tests/support`).

#[path = "../../droppen-client/tests/support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use droppen_a2a_proxy::errors::Code;
use droppen_a2a_proxy::server::Proxy;
use droppen_a2a_proxy::upstream::{classify, Answer, DeviceUpstream, Upstream};
use droppen_client::failure::{Failure, Refuser};
use droppen_client::rpc::RpcRequest;
use droppen_proto::chain::Action;
use futures_util::future::BoxFuture;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

const TOKEN: &str = "0123456789abcdef0123456789abcdef-test";

/// What the stand-in answers a call with.
type Respond = Box<dyn Fn(&str, &RpcRequest) -> Result<Answer, Failure> + Send + Sync>;

/// A stand-in for the devices: answers by `respond`, admits by `admits`,
/// judges as [`classify`] does with no chain, and keeps every call.
struct Fake {
    respond: Respond,
    admits: Mutex<Result<(), Code>>,
    calls: Mutex<Vec<(String, RpcRequest)>>,
}

impl Upstream for Fake {
    fn call<'a>(&'a self, host: &'a str, request: RpcRequest) -> BoxFuture<'a, Result<Answer, Failure>> {
        Box::pin(async move {
            let answer = (self.respond)(host, &request);
            self.calls.lock().unwrap().push((host.to_string(), request));
            answer
        })
    }

    fn admits<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, Result<(), Code>> {
        Box::pin(async move { *self.admits.lock().unwrap() })
    }

    fn judge<'a>(&'a self, host: &'a str, failure: &'a Failure) -> BoxFuture<'a, Option<Code>> {
        Box::pin(async move { classify(None, "proxy-1", host, failure) })
    }
}

fn answer(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Answer {
    let body = body.to_vec();
    Answer {
        status,
        headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
        body: futures_util::stream::once(async move { Ok(body) }).boxed(),
    }
}

/// The proxy on a free port in front of `upstream`.
async fn proxy(upstream: Arc<dyn Upstream>, aliases: &[(&str, &str)]) -> SocketAddr {
    let aliases: BTreeMap<String, String> = aliases.iter().map(|(a, d)| (a.to_string(), d.to_string())).collect();
    let proxy = Arc::new(Proxy::new(upstream, TOKEN, None, aliases));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(droppen_a2a_proxy::serve(listener, proxy, std::future::pending()));
    addr
}

fn fake(respond: Respond) -> Arc<Fake> {
    Arc::new(Fake {
        respond,
        admits: Mutex::new(Ok(())),
        calls: Mutex::new(Vec::new()),
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test]
async fn only_the_proxys_token_gets_in_and_its_health_is_open() {
    let upstream = fake(Box::new(|_, _| Ok(answer(200, &[("content-type", "application/json")], b"{}"))));
    let addr = proxy(upstream.clone(), &[]).await;
    let url = format!("http://{addr}/a2a/host-1/agents/main/rest/tasks");
    let http = client();
    for auth in [
        None,
        Some("Bearer wrong"),
        Some(&format!("Basic {TOKEN}")[..]),
        Some(&format!("Bearer {TOKEN}x")[..]),
        Some("Bearer"),
    ] {
        let mut request = http.get(&url);
        if let Some(auth) = auth {
            request = request.header("authorization", auth);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 401, "{auth:?}");
        assert!(response.headers()["www-authenticate"].to_str().unwrap().starts_with("Bearer"));
        assert_eq!(response.json::<Value>().await.unwrap()["error"]["status"], "UNAUTHENTICATED");
    }
    // Not a route: the same answer whatever was asked, once the token is in.
    let response = http.get(format!("http://{addr}/api/sessions")).send().await.unwrap();
    assert_eq!(response.status(), 401, "no token: 401 before anything else");
    assert!(upstream.calls.lock().unwrap().is_empty(), "nothing reached a device");
    let response = http.get(format!("http://{addr}/api/sessions")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(response.status(), 404);
    let response = http.get(&url).header("authorization", format!("bearer {TOKEN}")).send().await.unwrap();
    assert_eq!(response.status(), 200, "the scheme in any case");
    assert_eq!(http.get(format!("http://{addr}/healthz")).send().await.unwrap().status(), 200);
}

#[tokio::test]
async fn a_request_goes_on_with_its_path_query_and_body_and_only_the_headers_it_may_carry() {
    let upstream = fake(Box::new(|_, _| {
        Ok(answer(
            201,
            &[
                ("content-type", "application/a2a+json"),
                ("content-length", "2"),
                ("set-cookie", "x=1"),
                ("x-kept", "yes"),
            ],
            b"{}",
        ))
    }));
    let addr = proxy(upstream.clone(), &[("mac", "host-1")]).await;
    let response = client()
        .post(format!("http://{addr}/a2a/mac/agents/main/rest/message:send?x=1"))
        .bearer_auth(TOKEN)
        .header("a2a-version", "1.0")
        .header("a2a-extensions", "https://ext.example/v1")
        .header("content-type", "application/a2a+json")
        .header("cookie", "secret=1")
        .header("x-forwarded-for", "1.2.3.4")
        .body(r#"{"message":{}}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(response.headers()["x-kept"], "yes");
    assert!(response.headers().get("set-cookie").is_none());
    let calls = upstream.calls.lock().unwrap();
    let (host, request) = &calls[0];
    assert_eq!(host, "host-1", "the alias names the device");
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/a2a/agents/main/rest/message:send?x=1");
    assert_eq!(request.body, br#"{"message":{}}"#);
    let mut names: Vec<&str> = request.headers.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    // `accept` is the client library's own (`*/*`).
    assert_eq!(
        names,
        ["a2a-extensions", "a2a-version", "accept", "content-type"],
        "never the token, a cookie or a forwarded address"
    );
}

#[tokio::test]
async fn the_agent_cards_addresses_become_the_proxys_and_it_is_asked_for_whole() {
    let card = json!({
        "name": "main",
        "supportedInterfaces": [
            {"url": "http://127.0.0.1:7777/a2a/agents/main/jsonrpc", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
            {"url": "http://127.0.0.1:7777/a2a/agents/main/rest", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"},
        ],
    });
    let body = card.to_string();
    let upstream = fake(Box::new(move |_, _| {
        Ok(answer(
            200,
            &[
                ("content-type", "application/json"),
                ("etag", "\"abc\""),
                ("cache-control", "private, max-age=300"),
            ],
            body.as_bytes(),
        ))
    }));
    let addr = proxy(upstream.clone(), &[]).await;
    let response = client()
        .get(format!("http://{addr}/a2a/host-1/agents/main/.well-known/agent-card.json"))
        .bearer_auth(TOKEN)
        .header("if-none-match", "\"abc\"")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("etag").is_none(), "the device's tag names another body");
    assert_eq!(response.headers()["cache-control"], "private, max-age=300");
    let got: Value = response.json().await.unwrap();
    let urls: Vec<&str> = got["supportedInterfaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["url"].as_str().unwrap())
        .collect();
    assert_eq!(
        urls,
        [
            format!("http://{addr}/a2a/host-1/agents/main/jsonrpc"),
            format!("http://{addr}/a2a/host-1/agents/main/rest")
        ]
    );
    let calls = upstream.calls.lock().unwrap();
    assert_eq!(calls[0].1.path, "/a2a/agents/main/.well-known/agent-card.json");
    assert!(calls[0].1.headers.iter().all(|(n, _)| n != "if-none-match"));
}

#[tokio::test]
async fn a_failure_is_answered_in_each_bindings_form_with_its_droppen_code() {
    let fail = Arc::new(Mutex::new(Failure::Offline("presence: offline".into())));
    let upstream = {
        let fail = fail.clone();
        fake(Box::new(move |_, _| Err(fail.lock().unwrap().clone())))
    };
    let addr = proxy(upstream.clone(), &[]).await;
    let http = client();
    let rpc = |body: &'static str| {
        http.post(format!("http://{addr}/a2a/host-1/agents/main/jsonrpc"))
            .bearer_auth(TOKEN)
            .header("content-type", "application/json")
            .body(body)
            .send()
    };
    let rest = || {
        http.get(format!("http://{addr}/a2a/host-1/agents/main/rest/tasks/t-1"))
            .bearer_auth(TOKEN)
            .send()
    };
    for (failure, code, http_status, status) in [
        (Failure::Offline("x".into()), "droppen.device_offline", 503, "UNAVAILABLE"),
        (Failure::Stale("x".into()), "droppen.attestation_stale", 400, "FAILED_PRECONDITION"),
        (
            Failure::Refused {
                by: Refuser::Agent,
                code: 4401,
                why: "x".into(),
            },
            "droppen.not_accepted",
            403,
            "PERMISSION_DENIED",
        ),
        (Failure::Removed("x".into()), "droppen.device_removed", 403, "PERMISSION_DENIED"),
    ] {
        *fail.lock().unwrap() = failure;
        let response = rpc(r#"{"jsonrpc":"2.0","id":"req-7","method":"SendMessage","params":{}}"#).await.unwrap();
        assert_eq!(response.status(), 200, "JSON-RPC answers its errors in a 200");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], "req-7", "the error answers the request");
        assert_eq!(body["error"]["code"], -32603);
        let info = &body["error"]["data"][0];
        assert_eq!(info["@type"], "type.googleapis.com/google.rpc.ErrorInfo");
        assert_eq!(
            (info["domain"].as_str(), info["metadata"]["code"].as_str()),
            (Some("droppen.app"), Some(code))
        );
        let response = rest().await.unwrap();
        assert_eq!(response.status(), http_status, "{code}");
        assert_eq!(response.headers()["content-type"], "application/a2a+json");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["status"], status);
        assert_eq!(body["error"]["details"][0]["metadata"]["code"], code);
    }
    // A request whose id is a number, or none at all.
    *fail.lock().unwrap() = Failure::Offline("x".into());
    let body: Value = rpc(r#"{"jsonrpc":"2.0","id":12,"method":"GetTask"}"#)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["id"], 12);
    let body: Value = rpc("not json").await.unwrap().json().await.unwrap();
    assert_eq!(body["id"], Value::Null);
    // Refused by the proxy's own chain before anything goes out.
    *upstream.admits.lock().unwrap() = Err(Code::DeviceRemoved);
    let calls = upstream.calls.lock().unwrap().len();
    let body: Value = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"GetTask"}"#).await.unwrap().json().await.unwrap();
    assert_eq!(body["error"]["data"][0]["metadata"]["code"], "droppen.device_removed");
    assert_eq!(upstream.calls.lock().unwrap().len(), calls, "nothing sent to the device");
}

#[tokio::test]
async fn the_bridges_lapsed_answer_is_judged_and_a_body_over_the_limit_is_refused() {
    let upstream = fake(Box::new(|_, request| {
        Ok(match request.path.as_str() {
            "/a2a/agents/main/rest/lapsed" => answer(
                412,
                &[("content-type", "application/json")],
                br#"{"error":"stale","reason":"lapsed","message":"m"}"#,
            ),
            _ => answer(412, &[("content-type", "application/json")], br#"{"error":"precondition"}"#),
        })
    }));
    let addr = proxy(upstream.clone(), &[]).await;
    let http = client();
    let response = http
        .get(format!("http://{addr}/a2a/host-1/agents/main/rest/lapsed"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["details"][0]["metadata"]["code"], "droppen.attestation_stale");
    // A route's own 412 goes back as it was.
    let response = http
        .get(format!("http://{addr}/a2a/host-1/agents/main/rest/other"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 412);
    assert_eq!(response.json::<Value>().await.unwrap(), json!({"error": "precondition"}));
    let response = http
        .post(format!("http://{addr}/a2a/host-1/agents/main/jsonrpc"))
        .bearer_auth(TOKEN)
        .body(vec![b' '; 10 * 1024 * 1024 + 1])
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["data"][1]["metadata"]["code"], "droppen.quota_exceeded");
}

#[tokio::test]
async fn an_answer_streams_through_as_the_device_sends_it() {
    let (tx, rx) = mpsc::unbounded_channel::<Result<Vec<u8>, Failure>>();
    let rx = Arc::new(Mutex::new(Some(rx)));
    let upstream = fake(Box::new(move |_, _| {
        let rx = rx.lock().unwrap().take().expect("one call");
        Ok(Answer {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            body: futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|piece| (piece, rx)) }).boxed(),
        })
    }));
    let addr = proxy(upstream, &[]).await;
    tx.send(Ok(b"data: 1\n\n".to_vec())).unwrap();
    let mut response = client()
        .post(format!("http://{addr}/a2a/host-1/agents/main/rest/message:stream"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let first = tokio::time::timeout(Duration::from_secs(5), response.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(&first[..], b"data: 1\n\n", "the first event before the second exists");
    tx.send(Ok(b"data: 2\n\n".to_vec())).unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), response.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(&second[..], b"data: 2\n\n");
    // The device's channel goes mid-stream: the answer is cut off.
    tx.send(Err(Failure::Offline("the other end disconnected".into()))).unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), response.chunk()).await.unwrap();
    assert!(end.is_err(), "a broken stream is not a clean end: {end:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_proxy_starts_only_from_a_data_directory_its_joining_made() {
    let config = |dir: &std::path::Path, hub: &str, code: Option<&str>| {
        let vars = [
            ("DROPPEN_DATA_DIR", dir.display().to_string()),
            ("DROPPEN_A2A_PROXY_TOKEN", TOKEN.to_string()),
            ("DROPPEN_HUB", hub.to_string()),
            ("PORT", "0".to_string()),
            ("DROPPEN_ENROLL_CODE", code.unwrap_or_default().to_string()),
        ];
        droppen_a2a_proxy::Config::from_vars(|name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone())).unwrap()
    };
    let empty = support::scratch("proxy-empty");
    let error = droppen_a2a_proxy::run(config(&empty, "hub.example", None), "test").await.unwrap_err();
    assert!(error.contains("has not joined an account"), "{error}");
    let error = droppen_a2a_proxy::run(config(&empty, "hub.example", Some("RH1-x")), "test")
        .await
        .unwrap_err();
    assert!(error.contains("not in this version"), "{error}");
    // Joined at one hub, pointed at another: refused, nothing served.
    let stand = support::Stand::new(support::echo()).await;
    let error = droppen_a2a_proxy::run(config(&stand.dir, "https://elsewhere.example", None), "test")
        .await
        .unwrap_err();
    assert!(error.contains("joined its account at"), "{error}");
}

// ------------------------------------------------------------- end to end

/// The proxy in front of the stand's device.
async fn proxy_for(stand: &support::Stand) -> SocketAddr {
    proxy(Arc::new(DeviceUpstream::new(stand.device.clone())), &[("mac", support::HOST)]).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_to_end_a_request_and_a_stream_go_through_a_relay_to_the_host() {
    let serve = support::handler(|request: support::Request, responder: support::Responder| async move {
        if request.head.path.ends_with("/message:stream") {
            responder.head(200, &[("content-type", "text/event-stream")]).await;
            for n in 0..3 {
                responder.chunk(format!("data: {n}\n\n").as_bytes()).await;
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            responder.close(0);
        } else {
            let body = json!({"path": request.head.path, "headers": request.head.headers, "body": String::from_utf8_lossy(&request.body)});
            responder.json(200, &body).await;
        }
    });
    let stand = support::Stand::new(serve).await;
    let addr = proxy_for(&stand).await;
    let http = client();
    let body: Value = http
        .post(format!("http://{addr}/a2a/mac/agents/main/jsonrpc"))
        .bearer_auth(TOKEN)
        .header("a2a-version", "1.0")
        .header("content-type", "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"GetTask","params":{"id":"t"}}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["path"], "/a2a/agents/main/jsonrpc");
    assert_eq!(body["body"], r#"{"jsonrpc":"2.0","id":1,"method":"GetTask","params":{"id":"t"}}"#);
    assert!(body["headers"].as_array().unwrap().iter().any(|h| h == &json!(["a2a-version", "1.0"])));
    let mut response = http
        .post(format!("http://{addr}/a2a/{}/agents/main/rest/message:stream", support::HOST))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    let mut seen = Vec::new();
    while let Some(piece) = tokio::time::timeout(Duration::from_secs(10), response.chunk()).await.unwrap().unwrap() {
        seen.extend_from_slice(&piece);
    }
    assert_eq!(String::from_utf8(seen).unwrap(), "data: 0\n\ndata: 1\n\ndata: 2\n\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_to_end_each_failure_is_judged_by_the_proxys_own_chain() {
    let rpc_body = r#"{"jsonrpc":"2.0","id":"x-1","method":"SendMessage","params":{}}"#;
    let code_of = |body: &Value| body["error"]["data"][0]["metadata"]["code"].as_str().map(String::from);

    // The host on no relay: offline.
    let mut config = support::StandConfig::new(support::echo());
    config.attach = false;
    let stand = support::Stand::with(config).await;
    let addr = proxy_for(&stand).await;
    let body: Value = client()
        .post(format!("http://{addr}/a2a/mac/agents/main/jsonrpc"))
        .bearer_auth(TOKEN)
        .body(rpc_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (body["id"].as_str(), code_of(&body).as_deref()),
        (Some("x-1"), Some("droppen.device_offline"))
    );

    // Nobody vouched for the host's view: stale.
    let mut config = support::StandConfig::new(support::echo());
    config.vouched = false;
    let stand = support::Stand::with(config).await;
    let addr = proxy_for(&stand).await;
    let body: Value = client()
        .get(format!("http://{addr}/a2a/mac/agents/main/rest/tasks"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["error"]["details"][0]["metadata"]["code"], "droppen.attestation_stale");

    // The host stops accepting the proxy: not accepted — before anything is
    // sent, by the chain the proxy syncs.
    let mut stand = support::Stand::new(support::echo()).await;
    let addr = proxy_for(&stand).await;
    let ok = client()
        .get(format!("http://{addr}/a2a/mac/agents/main/rest/tasks"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    stand.append(
        vec![Action::SetA2aAccept {
            host: support::HOST.into(),
            devices: Vec::new(),
        }],
        true,
    );
    stand.device.sync_chain().await.unwrap();
    let body: Value = client()
        .post(format!("http://{addr}/a2a/mac/agents/main/jsonrpc"))
        .bearer_auth(TOKEN)
        .body(rpc_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(code_of(&body).as_deref(), Some("droppen.not_accepted"));

    // The proxy removed from the account: removed, whatever the far end says.
    stand.append(vec![Action::RemoveDevice3(support::PROXY.into())], true);
    stand.device.sync_chain().await.unwrap();
    let body: Value = client()
        .post(format!("http://{addr}/a2a/mac/agents/main/jsonrpc"))
        .bearer_auth(TOKEN)
        .body(rpc_body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(code_of(&body).as_deref(), Some("droppen.device_removed"));
}
