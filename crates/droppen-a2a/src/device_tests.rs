//! A2A proxies as callers (`docs/a2a.md` §7.3), end to end through
//! [`crate::protocol::router`] on a real store and the [`FakeBackend`]: a
//! request bridged in as the guard hands it on — a [`BridgedDevice`], no
//! token — beside a token caller of the same entry; and what the device
//! chain ([`FakeDevices`]) and the entry's `allowedDevices` take back (§6).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use gotcha::axum::body::Body;
use gotcha::axum::extract::ConnectInfo;
use gotcha::axum::http::{Method, Request, StatusCode};
use gotcha::axum::Router;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use droppen_http::identity::BridgedDevice;
use droppen_instance::db::Db;

use crate::backend::event_channel;
use crate::engine::{A2a, Settings};
use crate::store::Store;
use crate::testing::{Call, FakeBackend, FakeDevices};
use crate::tests::{call, calls_of, json_of, message, state_of, Events};
use crate::wire::TaskState;

/// Who sends a request.
#[derive(Clone, Copy)]
enum Who<'a> {
    /// A device bridged in: its id, and whether the guard marked it an A2A
    /// proxy.
    Device(&'a str, bool),
    /// A token, from this device's loopback.
    Token(&'a str),
}

struct Proxies {
    _dir: tempfile::TempDir,
    a2a: Arc<A2a>,
    fake: Arc<FakeBackend>,
    /// The device chain: p-1, p-2 and p-3 admitted.
    devices: Arc<FakeDevices>,
    router: Router,
    _cancel: CancellationToken,
    /// A token for `main`.
    token: String,
}

/// A server whose entry `main` allows A2A proxies p-1 and p-2 (p-3 is one the
/// chain admits but `main` does not list) and pushes to
/// `https://hooks.example.com`, and a token for it.
async fn proxies() -> Proxies {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(Db::open(dir.path().join("state.db")));
    let (events, inbox) = event_channel();
    let fake = FakeBackend::new(events);
    let devices = FakeDevices::admitting(&["p-1", "p-2", "p-3"]);
    let cancel = CancellationToken::new();
    let settings = Settings {
        version: "0.0.0-test".into(),
        public_base: None,
        tck: false,
        tck_streaming_timeout: Duration::from_millis(300),
    };
    let a2a = A2a::new(Store::new(db), fake.clone(), devices.clone(), inbox, settings, cancel.clone());
    let entry = json!({
        "id": "main", "name": "Main", "source": "claude", "cwd": "/work", "enabled": true, "isolation": "none",
        "allowedDevices": ["p-1", "p-2"], "pushOrigins": ["https://hooks.example.com"],
    });
    a2a.create_entry(serde_json::from_value(entry).unwrap()).await.expect("the entry");
    let token = a2a
        .create_token(serde_json::from_value(json!({"name": "dont-do", "entries": ["main"]})).unwrap())
        .expect("a token")
        .secret;
    Proxies {
        _dir: dir,
        router: crate::protocol::router(a2a.clone()),
        a2a,
        fake,
        devices,
        _cancel: cancel,
        token,
    }
}

/// A request as the guard hands it on, from this device's loopback (the
/// bridge is loopback too).
fn request(method: Method, uri: &str, from: Who<'_>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8700")
        .header("a2a-version", "1.0");
    if let Who::Token(token) = from {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40_000))));
    if let Who::Device(id, a2a) = from {
        request.extensions_mut().insert(BridgedDevice { id: id.into(), a2a });
    }
    request
}

impl Proxies {
    /// One JSON-RPC call to `main`: its status and answer.
    async fn rpc_status(&self, from: Who<'_>, method: &str, params: Value) -> (StatusCode, Value) {
        let body = json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params});
        let response = call(&self.router, request(Method::POST, "/agents/main/jsonrpc", from, Some(body))).await;
        (response.status(), json_of(response).await)
    }

    async fn rpc(&self, from: Who<'_>, method: &str, params: Value) -> Value {
        let (status, answer) = self.rpc_status(from, method, params).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        answer
    }

    async fn stream(&self, from: Who<'_>, method: &str, params: Value) -> Events {
        let body = json!({"jsonrpc": "2.0", "id": 9, "method": method, "params": params});
        let response = call(&self.router, request(Method::POST, "/agents/main/jsonrpc", from, Some(body))).await;
        assert!(
            response.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"),
            "{:?}",
            response.headers()
        );
        Events::of(response)
    }

    /// A new task sent by `from`, without waiting: its id.
    async fn new_task(&self, from: Who<'_>, id: &str) -> String {
        let answer = self
            .rpc(
                from,
                "SendMessage",
                json!({"message": message(id, "fix the bug"), "configuration": {"returnImmediately": true}}),
            )
            .await;
        answer["result"]["task"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("a task: {answer}"))
            .to_string()
    }

    async fn wait_state(&self, task_id: &str, state: TaskState) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let now = self.a2a.store().task(task_id).expect("the task").state;
            if now == state {
                return;
            }
            assert!(tokio::time::Instant::now() < deadline, "the task never became {state:?}; it is {now:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A task of `from`'s taken into its turn and working.
    async fn working(&self, from: Who<'_>, id: &str) -> String {
        let task = self.new_task(from, id).await;
        let wanted = task.clone();
        self.fake
            .wait_for(move |call| matches!(call, Call::Start(start) if start.task.task_id == wanted))
            .await;
        self.fake.turn_started(&task);
        self.wait_state(&task, TaskState::Working).await;
        task
    }

    /// The `droppen.*` code in a task's status message, if any.
    fn code_of(&self, task_id: &str) -> Value {
        let row = self.a2a.store().task(task_id).unwrap();
        row.status_message
            .and_then(|message| message.metadata)
            .map_or(Value::Null, |metadata| metadata["errorInfo"]["metadata"]["code"].clone())
    }
}

const P1: Who<'static> = Who::Device("p-1", true);
const P2: Who<'static> = Who::Device("p-2", true);

/// An A2A proxy the chain admits, on an entry that allows it, is a caller
/// of its own: it starts a task (kept under `a2a:device:<id>`, and named so
/// to the agent), reads it, follows it, and cancels it — with no token at
/// all, on both bindings.
#[tokio::test]
async fn an_a2a_proxy_creates_follows_and_cancels_its_own_tasks() {
    let h = proxies().await;
    let card = call(&h.router, request(Method::GET, "/agents/main/.well-known/agent-card.json", P1, None)).await;
    assert_eq!(card.status(), StatusCode::OK);

    let task = h.working(P1, "m-1").await;
    let row = h.a2a.store().task(&task).unwrap();
    assert_eq!(row.caller, "a2a:device:p-1");
    let Call::Start(start) = h.fake.wait_for(|call| matches!(call, Call::Start(_))).await else {
        unreachable!()
    };
    assert_eq!(start.task.caller, "a2a:device:p-1");
    assert!(start.prompt.text.contains("a2a:device:p-1"), "the agent is told who asks");

    let got = h.rpc(P1, "GetTask", json!({"id": task})).await;
    assert_eq!(got["result"]["status"]["state"], "TASK_STATE_WORKING", "{got}");
    let rest = call(&h.router, request(Method::GET, &format!("/agents/main/rest/tasks/{task}"), P1, None)).await;
    assert_eq!(rest.status(), StatusCode::OK);
    let listed = h.rpc(P1, "ListTasks", json!({})).await;
    assert_eq!(listed["result"]["tasks"].as_array().map(Vec::len), Some(1), "{listed}");

    let mut stream = h.stream(P1, "SubscribeToTask", json!({"id": task})).await;
    let first = stream.next().await.expect("the snapshot");
    assert_eq!(first["result"]["task"]["id"], task.as_str());
    let canceled = h.rpc(P1, "CancelTask", json!({"id": task})).await;
    assert_eq!(canceled["result"]["status"]["state"], "TASK_STATE_CANCELED", "{canceled}");
    let mut last = Value::Null;
    while let Some(event) = stream.next().await {
        last = event["result"].clone();
    }
    assert_eq!(state_of(&last), "TASK_STATE_CANCELED", "the stream follows it to its end");
    let cancels = calls_of(&h.fake, |call| matches!(call, Call::Cancel(t) if t.task_id == task).then_some(()));
    assert_eq!(cancels.len(), 1, "its turn is interrupted");
}

/// An A2A proxy and a token on the same entry — and two proxies — see none
/// of each other's tasks: another caller's task is not found, exactly as
/// one that does not exist (§6), whatever is asked of it.
#[tokio::test]
async fn a_proxy_and_a_token_see_none_of_each_others_tasks() {
    let h = proxies().await;
    let token = Who::Token(&h.token);
    let theirs = h.new_task(token, "t-1").await;
    let mine = h.new_task(P1, "p-1").await;
    for (from, foreign) in [(P1, &theirs), (token, &mine), (P2, &mine)] {
        for method in ["GetTask", "CancelTask", "SubscribeToTask"] {
            let (_, answer) = h.rpc_status(from, method, json!({"id": foreign})).await;
            assert_eq!(answer["error"]["code"], -32001, "{method}: {answer}");
        }
        let (_, answer) = h.rpc_status(from, "ListTaskPushNotificationConfigs", json!({"taskId": foreign})).await;
        assert_eq!(answer["error"]["code"], -32001, "{answer}");
        let rest = call(&h.router, request(Method::GET, &format!("/agents/main/rest/tasks/{foreign}"), from, None)).await;
        assert_eq!(rest.status(), StatusCode::NOT_FOUND);
    }
    let ids = |answer: Value| -> Vec<String> {
        answer["result"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["id"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(h.rpc(P1, "ListTasks", json!({})).await), [mine.as_str()]);
    assert_eq!(ids(h.rpc(token, "ListTasks", json!({})).await), [theirs.as_str()]);
    assert!(ids(h.rpc(P2, "ListTasks", json!({})).await).is_empty());
    assert_eq!(h.a2a.store().task(&theirs).unwrap().state, TaskState::Submitted, "untouched");
}

/// A request bridged in is its device and nothing else (§7.3): a token it
/// carries counts for nothing; a device the chain does not admit, or that
/// the guard did not mark an A2A proxy, is unauthenticated, as a revoked
/// token is; an entry that does not list the proxy is not found, as one
/// that does not exist.
#[tokio::test]
async fn a_bridged_request_is_its_device_and_nothing_else() {
    let h = proxies().await;
    let list = json!({});
    // p-3: the chain admits it, `main` does not list it — a token for `main`
    // beside it changes nothing.
    let mut with_token = request(
        Method::POST,
        "/agents/main/jsonrpc",
        Who::Device("p-3", true),
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ListTasks", "params": list})),
    );
    with_token
        .headers_mut()
        .insert("authorization", format!("Bearer {}", h.token).parse().unwrap());
    let response = call(&h.router, with_token).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "the token is not its");
    let (status, _) = h.rpc_status(Who::Device("p-3", true), "ListTasks", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let card = call(
        &h.router,
        request(Method::GET, "/agents/main/.well-known/agent-card.json", Who::Device("p-3", true), None),
    )
    .await;
    assert_eq!(card.status(), StatusCode::NOT_FOUND, "nor its card");
    let (status, _) = h.rpc_status(P1, "ListTasks", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let other = call(&h.router, request(Method::GET, "/agents/other/.well-known/agent-card.json", P1, None)).await;
    assert_eq!(other.status(), StatusCode::NOT_FOUND, "no such entry");
    // Not admitted by the chain: unauthenticated, as a revoked token.
    for from in [Who::Device("p-9", true), Who::Device("bad id", true), Who::Device("p-1", false)] {
        let (status, _) = h.rpc_status(from, "ListTasks", json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let mut marked_off = request(
        Method::POST,
        "/agents/main/jsonrpc",
        Who::Device("d-1", false),
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ListTasks", "params": {}})),
    );
    marked_off
        .headers_mut()
        .insert("authorization", format!("Bearer {}", h.token).parse().unwrap());
    assert_eq!(
        call(&h.router, marked_off).await.status(),
        StatusCode::UNAUTHORIZED,
        "a device bridged in that is no A2A proxy is nobody here, token or not"
    );
    h.devices.forget("p-1");
    let (status, _) = h.rpc_status(P1, "ListTasks", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "once the chain no longer admits it");
}

/// Taking a proxy out of the entry's `allowedDevices` takes back what it has
/// there, as revoking a token does (§6): its tasks cancelled with
/// `droppen.revoked`, its push configurations deleted, its streams closed;
/// and it reaches the entry no more. The other proxy and the token keep
/// theirs.
#[tokio::test]
async fn taking_a_proxy_out_of_allowed_devices_takes_back_its_tasks() {
    let h = proxies().await;
    let working = h.working(P1, "m-1").await;
    let other = h.new_task(P2, "m-2").await;
    let push = h
        .rpc(
            P1,
            "CreateTaskPushNotificationConfig",
            json!({"taskId": working, "url": "https://hooks.example.com/a2a", "token": "t"}),
        )
        .await;
    assert!(push["result"]["id"].is_string(), "{push}");
    let mut stream = h.stream(P1, "SubscribeToTask", json!({"id": working})).await;
    stream.next().await.unwrap();

    h.a2a
        .update_entry("main", serde_json::from_value(json!({"allowedDevices": ["p-2"]})).unwrap())
        .await
        .unwrap();
    assert!(stream.next().await.is_none(), "its open stream is closed");
    assert_eq!(h.a2a.store().task(&working).unwrap().state, TaskState::Canceled);
    assert_eq!(h.code_of(&working), "droppen.revoked");
    assert!(h.a2a.store().pushes(&working).is_empty(), "its push configurations are gone");
    assert!(!h.a2a.caller_reaches("a2a:device:p-1", "main"));
    let (status, _) = h.rpc_status(P1, "ListTasks", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_ne!(h.a2a.store().task(&other).unwrap().state, TaskState::Canceled, "p-2's stays");
    assert!(h.a2a.caller_reaches("a2a:device:p-2", "main"));
}

/// The device chain moving so that it no longer admits a proxy — removed from
/// the account, taken off this device's A2A list, this device no longer
/// running AI — takes back what that proxy has here, on every entry, as a
/// token revoke does: its unfinished tasks cancelled (queued ones withdrawn,
/// running ones interrupted) with `droppen.revoked`, its push
/// configurations deleted, its streams closed; and a push asks again and is
/// told no. Another proxy's and the token's tasks stay.
#[tokio::test]
async fn the_chain_no_longer_admitting_a_proxy_takes_back_its_tasks() {
    let h = proxies().await;
    let token = Who::Token(&h.token);
    let working = h.working(P1, "m-1").await;
    let queued = h.new_task(P1, "m-2").await;
    let others = h.new_task(P2, "m-3").await;
    let tokens = h.new_task(token, "m-4").await;
    h.rpc(
        P1,
        "CreateTaskPushNotificationConfig",
        json!({"taskId": working, "url": "https://hooks.example.com/a2a", "token": "t"}),
    )
    .await;
    let mut stream = h.stream(P1, "SubscribeToTask", json!({"id": working})).await;
    stream.next().await.unwrap();

    h.devices.forget("p-1");
    assert!(stream.next().await.is_none(), "its open stream is closed");
    for task in [&working, &queued] {
        h.wait_state(task, TaskState::Canceled).await;
        assert_eq!(h.code_of(task), "droppen.revoked");
    }
    assert!(h.a2a.store().pushes(&working).is_empty(), "its push configurations are gone");
    let cancels = calls_of(&h.fake, |call| match call {
        Call::Cancel(task) => Some(task.task_id.clone()),
        _ => None,
    });
    assert_eq!(
        cancels,
        [working.as_str()],
        "the running one is interrupted; the queued one was never handed over"
    );
    assert!(!h.a2a.caller_reaches("a2a:device:p-1", "main"), "a push asks again, and is told no");
    for task in [&others, &tokens] {
        assert_ne!(h.a2a.store().task(task).unwrap().state, TaskState::Canceled, "{task} stays");
    }
    // Admitted again, it is a caller again, with nothing of before.
    h.devices.admit("p-1");
    let listed = h.rpc(P1, "ListTasks", json!({})).await;
    let states: Vec<&str> = listed["result"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["status"]["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["TASK_STATE_CANCELED", "TASK_STATE_CANCELED"]);
}

/// §6: an entry A2A proxies may call never runs in a bypass permission mode
/// — Claude's `bypassPermissions`, Codex's `full-access`,
/// `danger-full-access` and `bypass-all` — whichever of the two is set
/// first, on creation and on every change; and an entry stored with both
/// (saved before the rule) lets no proxy in.
#[tokio::test]
async fn an_entry_proxies_may_call_runs_in_no_bypass_mode() {
    let h = proxies().await;
    for (source, permission) in [
        ("claude", "bypassPermissions"),
        ("codex", "danger-full-access"),
        ("codex", "full-access"),
        ("codex", "bypass-all"),
    ] {
        let refused = h
            .a2a
            .create_entry(
                serde_json::from_value(json!({
                    "id": "risky", "name": "Risky", "source": source, "cwd": "/work",
                    "permission": permission, "allowedDevices": ["p-1"],
                }))
                .unwrap(),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{source} {permission}");
    }
    // Bypass without devices: a token's entry, as before.
    h.a2a
        .create_entry(
            serde_json::from_value(json!({"id": "risky", "name": "Risky", "source": "claude", "cwd": "/work", "permission": "bypassPermissions"}))
                .unwrap(),
        )
        .await
        .expect("no device may call it");
    let refused = h
        .a2a
        .update_entry("risky", serde_json::from_value(json!({"allowedDevices": ["p-1"]})).unwrap())
        .await
        .unwrap_err();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "adding a device to a bypass entry");
    let refused = h
        .a2a
        .update_entry("main", serde_json::from_value(json!({"permission": "bypassPermissions"})).unwrap())
        .await
        .unwrap_err();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "bypass on an entry devices may call");
    h.a2a
        .update_entry("main", serde_json::from_value(json!({"permission": "acceptEdits"})).unwrap())
        .await
        .expect("a mode with its guard rails on");
    // Stored with both, as before the rule: no proxy gets in.
    let mut legacy = h.a2a.entry("main").unwrap();
    legacy.permission = droppen_http::openapi_ext::Nullable(Some("bypassPermissions".into()));
    h.a2a.store().replace_entry(&legacy, 1).unwrap();
    let (status, _) = h.rpc_status(P1, "ListTasks", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!h.a2a.caller_reaches("a2a:device:p-1", "main"));
}

/// The device list is the gate's: what the chain admits, for the entry
/// form's `allowedDevices`.
#[tokio::test]
async fn the_device_list_is_what_the_chain_admits() {
    let h = proxies().await;
    let ids = |h: &Proxies| h.a2a.admitted_devices().into_iter().map(|device| device.id).collect::<Vec<_>>();
    assert_eq!(ids(&h), ["p-1", "p-2", "p-3"]);
    h.devices.forget("p-2");
    assert_eq!(ids(&h), ["p-1", "p-3"]);
}
