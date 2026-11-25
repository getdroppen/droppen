//! The A2A proxy end to end, in this process (`docs/a2a.md` §7): an A2A
//! client's request to `droppen a2a-proxy` (`droppen_a2a_proxy`), over the
//! proxy's own device (`droppen_client`) and a relay (`droppen-relay-core`),
//! to this server's real cloud agent — its link to the hub
//! (`droppen_cloud::control`), its `A2aOnly` admission and its bridge — then
//! the auth guard and the A2A server (`droppen_a2a`) over a fake session
//! backend. The hub is the test hub of droppen-client's stand
//! (`tests/support`): the claim protocol, an A2A proxy's session routes, and
//! tickets by `may_call_a2a` and `a2a/1` (`docs/hub-api/`).
//!
//! What is not real here: the hub, the session backend (`FakeBackend`), and
//! time (a stale view is one whose newest attestation is eight days old when
//! the agent starts; a view that lapses while a channel is open is the
//! agent's own tests').

#[path = "../../droppen-client/tests/support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use droppen_a2a::backend::event_channel;
use droppen_a2a::testing::{Call, FakeBackend};
use droppen_a2a_proxy::server::Proxy;
use droppen_a2a_proxy::upstream::DeviceUpstream;
use droppen_agent::link::LinkEnv;
use droppen_agent::state::AgentState;
use droppen_agent::AgentFiles;
use droppen_client::device::StandaloneDevice;
use droppen_client::failure::Failure;
use droppen_client::rpc::RpcRequest;
use droppen_cloud::control::Control;
use droppen_instance::paths::Paths;
use droppen_proto::chain::Action;
use droppen_proto::mux::StreamClass;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "0123456789abcdef0123456789abcdef-e2e";
const SECS: Duration = Duration::from_secs(10);
/// The least time between two chain syncs the proxy's requests ask for
/// (`droppen_a2a_proxy::upstream`).
const SYNC_EVERY: Duration = Duration::from_secs(5);
/// What [`Events::next`] says when nothing came within [`SECS`].
const NO_EVENT: &str = "no event within 10 s";

/// The whole path: the hub and a relay, this server's agent on the relay,
/// the guard and the A2A server behind its bridge, and the proxy in front.
struct E2e {
    account: support::Account,
    _relay: support::TestRelay,
    hub: support::TestHub,
    control: Arc<Control>,
    a2a: Arc<droppen_a2a::A2a>,
    fake: Arc<FakeBackend>,
    device: Arc<StandaloneDevice>,
    proxy: SocketAddr,
    http: reqwest::Client,
    cancel: CancellationToken,
    _dir: tempfile::TempDir,
}

impl Drop for E2e {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The agent's [`LinkEnv`] for a server listening on `addr` with
/// `credential`: the streams, push host and switch the server attaches.
fn link_env(paths: &Paths, addr: SocketAddr, credential: String, cancel: CancellationToken) -> LinkEnv {
    let instance = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).unwrap();
    let auth = droppen_auth::Auth::new(
        paths,
        &instance,
        &droppen_http::config::AuthSettings::default(),
        false,
        Arc::new(crate::app::NotifyTokens),
    );
    let drive = droppen_drive::Drive::new(
        Arc::new(paths.clone()),
        &instance,
        auth,
        crate::app::transcript_sources(),
        crate::app::register_providers,
        crate::app::logins(),
        crate::app::NotifyDecoration::new(Arc::new(paths.clone()), instance.clone()),
    );
    LinkEnv {
        addr,
        credential,
        streams: Arc::new(crate::stream_source::SessionStreams::new(
            drive.paths.clone(),
            drive.transcripts.clone(),
            drive,
        )),
        host: Arc::new(crate::app::PushLinkHost),
        cancel,
        aggregator_switch: Arc::new(droppen_cloud::LinkFile::of(paths)),
    }
}

impl E2e {
    /// The path up, the host's view vouched for `vouched_ago` before now.
    async fn start(vouched_ago: Duration) -> E2e {
        // Relays on loopback, over plain `ws://`: local testing only.
        std::env::set_var("DROPPEN_AGENT_RELAY_INSECURE", "1");
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(dir.path().to_path_buf(), "a2ae2e");
        let control = Control::new(paths.clone());
        let machine = control.identity().unwrap();
        let account = support::Account::with_host(machine.sign_public(), machine.dh_public());
        let relay = support::TestRelay::start("rly-1", support::timing(60_000)).await;
        let hub = support::TestHub::start(&account, &[&relay]).await;

        // This server enrolled, its view vouched for by the admin.
        let now = droppen_client::now_ms();
        let origin = hub.state.lock().unwrap().origin.clone();
        let mut state = AgentState::enrolling(&origin, support::ACCOUNT, support::HOST, account.fingerprint, now);
        assert!(state.absorb(&account.entries(), now).error.is_none());
        assert!(state.attest(account.attestation_at(now - vouched_ago.as_millis() as u64)));
        state.save(&AgentFiles::of(&paths)).unwrap();

        // Its loopback listener: the guard, and the A2A server over a fake
        // backend, asking the link which proxies it takes.
        let instance = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).unwrap();
        let auth = droppen_auth::Auth::new(
            &paths,
            &instance,
            &droppen_http::config::AuthSettings::default(),
            false,
            Arc::new(crate::app::NotifyTokens),
        );
        auth.users.add("ada", "correct horse battery staple").await.unwrap();
        let db = Arc::new(droppen_instance::db::Db::open(dir.path().join("a2a.db")));
        let (events, inbox) = event_channel();
        let fake = FakeBackend::new(events);
        let cancel = CancellationToken::new();
        let a2a = droppen_a2a::A2a::new(
            droppen_a2a::store::Store::new(db),
            fake.clone(),
            crate::a2a_backend::LinkDevices::new(control.clone()),
            inbox,
            droppen_a2a::Settings::from_env("0.0.0-test", None),
            cancel.clone(),
        );
        for (id, allowed) in [("main", json!([support::PROXY])), ("other", json!([]))] {
            let entry =
                json!({"id": id, "name": id, "source": "claude", "cwd": "/work", "enabled": true, "isolation": "none", "allowedDevices": allowed});
            a2a.create_entry(serde_json::from_value(entry).unwrap()).await.unwrap();
        }
        let router = gotcha::axum::Router::new()
            .nest_service("/a2a", droppen_a2a::protocol::router(a2a.clone()))
            .layer(droppen_auth::layer(auth.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                gotcha::axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
                    .with_graceful_shutdown(async move { cancel.cancelled().await })
                    .await
            });
        }

        // The agent: signed in at the hub, then on the relay with `a2a/1` said.
        control.attach(link_env(&paths, addr, auth.internal_credential().to_string(), cancel.clone()));
        assert!(control.start_if_enrolled().await);
        let attached = support::eventually(SECS, || {
            let st = hub.state.lock().unwrap();
            st.agent_tickets > 0 && st.agent_features.get(support::HOST).is_some_and(|f| f.iter().any(|f| f == "a2a/1"))
        })
        .await;
        assert!(attached, "the agent declared a2a/1 and asked for a relay ticket");

        // The proxy: its device, and its listener.
        let device = Arc::new(support::device_in(
            &support::scratch("e2e-proxy"),
            &hub,
            &account,
            support::device_options(),
        ));
        device.sync_chain().await.unwrap();
        let aliases = BTreeMap::from([("mac".to_string(), support::HOST.to_string())]);
        let proxy = Arc::new(Proxy::new(Arc::new(DeviceUpstream::new(device.clone())), TOKEN, None, aliases));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        {
            let cancel = cancel.clone();
            tokio::spawn(droppen_a2a_proxy::serve(listener, proxy, async move { cancel.cancelled().await }));
        }
        let e2e = E2e {
            account,
            _relay: relay,
            hub,
            control,
            a2a,
            fake,
            device,
            proxy: proxy_addr,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            cancel,
            _dir: dir,
        };
        e2e.wait_for_the_relay().await;
        e2e
    }

    /// Until the agent is signed in on the relay: a first channel opens
    /// (a stale view is a channel refused, which says the same).
    async fn wait_for_the_relay(&self) {
        let deadline = tokio::time::Instant::now() + SECS;
        loop {
            match self.device.channel(support::HOST).await {
                Ok(_) | Err(Failure::Stale(_)) => return,
                Err(failure) if tokio::time::Instant::now() > deadline => panic!("the agent never took a channel: {failure:?}"),
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}/a2a/mac/agents/{path}", self.proxy)
    }

    /// A JSON-RPC call to entry `main`.
    fn rpc(&self, id: &str, method: &str, params: Value) -> reqwest::RequestBuilder {
        self.http
            .post(self.url("main/jsonrpc"))
            .bearer_auth(TOKEN)
            .header("a2a-version", "1.0")
            .header("content-type", "application/json")
            .body(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string())
    }

    /// An HTTP+JSON call to entry `main`.
    fn rest(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> reqwest::RequestBuilder {
        let request = self
            .http
            .request(method, self.url(&format!("main/rest/{path}")))
            .bearer_auth(TOKEN)
            .header("a2a-version", "1.0");
        match body {
            Some(body) => request.header("content-type", "application/json").body(body.to_string()),
            None => request,
        }
    }

    /// The task the backend was asked to start for message `id`.
    async fn started(&self, message: &str) -> String {
        let wanted = format!(":{message}");
        let call = self
            .fake
            .wait_for(|call| matches!(call, Call::Start(start) if start.client_message_id.ends_with(&wanted)))
            .await;
        call.task_id().expect("a task").to_string()
    }

    /// The admin appends `actions`; the hub serves them, and hands the agent
    /// the admin's word on the new head.
    fn append(&mut self, actions: Vec<Action>) {
        self.account.append(actions);
        self.hub.set_entries(self.account.entries());
        self.hub.attest(&self.account.attestation());
    }
}

fn message(id: &str, text: &str) -> Value {
    json!({"messageId": id, "role": "ROLE_USER", "parts": [{"text": text, "mediaType": "text/plain"}]})
}

/// An SSE answer read event by event.
struct Events {
    response: reqwest::Response,
    buffer: String,
}

impl Events {
    fn new(response: reqwest::Response) -> Events {
        assert_eq!(response.status(), 200);
        assert!(
            response.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"),
            "{:?}",
            response.headers()
        );
        Events {
            response,
            buffer: String::new(),
        }
    }

    /// The next event's JSON; `Ok(None)` once the stream ended cleanly, an
    /// error when it broke.
    async fn next(&mut self) -> Result<Option<Value>, String> {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let data: Vec<&str> = block.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim).collect();
                if data.is_empty() {
                    continue;
                }
                return Ok(Some(serde_json::from_str(&data.join("\n")).map_err(|e| e.to_string())?));
            }
            match tokio::time::timeout(SECS, self.response.chunk()).await {
                Ok(Ok(Some(chunk))) => self.buffer.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(Ok(None)) => return Ok(None),
                Ok(Err(e)) => return Err(e.to_string()),
                Err(_) => return Err(NO_EVENT.into()),
            }
        }
    }
}

/// What a stream item says of its task's state (JSON-RPC wraps it in `result`).
fn state_of(item: &Value) -> String {
    let item = item.get("result").unwrap_or(item);
    item["task"]["status"]["state"]
        .as_str()
        .or_else(|| item["statusUpdate"]["status"]["state"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The droppen code of an error in either binding's form.
fn code_of(body: &Value) -> String {
    let details = body["error"]["data"].as_array().or_else(|| body["error"]["details"].as_array());
    details
        .into_iter()
        .flatten()
        .find(|d| d["domain"] == "droppen.app")
        .and_then(|d| d["metadata"]["code"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a2a_calls_go_from_the_proxy_through_a_relay_and_the_agent_to_the_a2a_server() {
    let e2e = E2e::start(Duration::from_secs(60)).await;

    // The Agent Card, through the proxy: its addresses are the proxy's.
    let card: Value = e2e
        .http
        .get(e2e.url("main/.well-known/agent-card.json"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let urls: Vec<&str> = card["supportedInterfaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["url"].as_str().unwrap())
        .collect();
    assert_eq!(urls, [e2e.url("main/jsonrpc"), e2e.url("main/rest")], "{card}");

    // SendMessage over JSON-RPC, waiting for the turn: completed.
    let blocking = tokio::spawn(
        e2e.rpc("rpc-1", "SendMessage", json!({"message": message("m-rpc", "fix the bug")}))
            .send(),
    );
    let task = e2e.started("m-rpc").await;
    e2e.fake.complete(&task, "fixed");
    let answer: Value = blocking.await.unwrap().unwrap().json().await.unwrap();
    assert_eq!(answer["id"], "rpc-1");
    assert_eq!(state_of(&answer), "TASK_STATE_COMPLETED", "{answer}");

    // SendMessage over HTTP+JSON, returned at once; GetTask once it is done.
    let sent: Value = e2e
        .rest(
            reqwest::Method::POST,
            "message:send",
            Some(json!({"message": message("m-rest", "add a test"), "configuration": {"returnImmediately": true}})),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rest_task = sent["task"]["id"].as_str().expect("a task").to_string();
    e2e.fake.complete(&e2e.started("m-rest").await, "added");
    let got: Value = e2e
        .rest(reqwest::Method::GET, &format!("tasks/{rest_task}"), None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["status"]["state"], "TASK_STATE_COMPLETED", "{got}");

    // message:stream: the task's first event arrives while its turn still
    // runs, the rest as it happens, and the stream ends with the turn.
    let response = e2e
        .rest(
            reqwest::Method::POST,
            "message:stream",
            Some(json!({"message": message("m-sse", "refactor")})),
        )
        .send()
        .await
        .unwrap();
    let mut events = Events::new(response);
    let first = events.next().await.unwrap().expect("a first event");
    let sse_task = e2e.started("m-sse").await;
    assert_eq!(first["task"]["id"], sse_task.as_str());
    assert_ne!(state_of(&first), "TASK_STATE_COMPLETED", "before the turn ended");
    e2e.fake.turn_started(&sse_task);
    e2e.fake.progress(&sse_task, "reading the code");
    let mut states = vec![state_of(&first)];
    let mut saw_progress = false;
    while !states.last().is_some_and(|s| s == "TASK_STATE_WORKING") || !saw_progress {
        let item = events.next().await.unwrap().expect("an event while the turn runs");
        saw_progress |= item["statusUpdate"]["status"]["message"]["parts"][0]["text"] == "reading the code";
        states.push(state_of(&item));
    }
    e2e.fake.complete(&sse_task, "refactored");
    let mut last = String::new();
    while let Some(item) = events.next().await.unwrap() {
        if !state_of(&item).is_empty() {
            last = state_of(&item);
        }
    }
    assert_eq!(last, "TASK_STATE_COMPLETED", "the stream ends with the turn: {states:?}");

    // SubscribeToTask on a task that runs, then cancel it.
    let sent: Value = e2e
        .rpc(
            "rpc-2",
            "SendMessage",
            json!({"message": message("m-sub", "long job"), "configuration": {"returnImmediately": true}}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sub_task = sent["result"]["task"]["id"].as_str().expect("a task").to_string();
    e2e.started("m-sub").await;
    let response = e2e
        .rest(reqwest::Method::GET, &format!("tasks/{sub_task}:subscribe"), None)
        .send()
        .await
        .unwrap();
    let mut subscribed = Events::new(response);
    let snapshot = subscribed.next().await.unwrap().expect("the task first");
    assert_eq!(snapshot["task"]["id"], sub_task.as_str());
    let canceled: Value = e2e
        .rest(reqwest::Method::POST, &format!("tasks/{sub_task}:cancel"), None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(canceled["status"]["state"], "TASK_STATE_CANCELED", "{canceled}");
    e2e.fake
        .wait_for(|call| matches!(call, Call::Cancel(task) if task.task_id == sub_task))
        .await;
    let mut last = String::new();
    while let Some(item) = subscribed.next().await.unwrap() {
        if !state_of(&item).is_empty() {
            last = state_of(&item);
        }
    }
    assert_eq!(last, "TASK_STATE_CANCELED", "the subscription follows the task to its end");

    // A body over 10 MiB: refused at the proxy, as the device would.
    let big = format!(
        r#"{{"jsonrpc":"2.0","id":"big","method":"SendMessage","params":{{"pad":"{}"}}}}"#,
        "x".repeat(10 * 1024 * 1024)
    );
    let answer: Value = e2e
        .http
        .post(e2e.url("main/jsonrpc"))
        .bearer_auth(TOKEN)
        .body(big)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(code_of(&answer), "droppen.quota_exceeded", "{answer}");
    // …and at the device's bridge too, should a proxy send one.
    let over = RpcRequest {
        method: "POST".into(),
        path: "/a2a/agents/main/jsonrpc".into(),
        headers: vec![("content-type".into(), "application/json".into())],
        body: vec![b' '; 10 * 1024 * 1024 + 1],
    };
    assert!(
        e2e.device.rpc(support::HOST, over).await.is_err(),
        "the bridge takes no more than 10 MiB from a proxy"
    );

    // An entry that does not list this proxy: no such agent.
    let other = e2e
        .http
        .get(e2e.url("other/.well-known/agent-card.json"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 404);

    // The channel is A2A only: nothing of /api, no direct path.
    let api = e2e.device.rpc(support::HOST, support::Stand::get("/api/sessions")).await.unwrap();
    assert_eq!(api.status, 403);
    let body: Value = serde_json::from_slice(&api.body.read_to_end(1 << 16).await.unwrap()).unwrap();
    assert_eq!(body["error"], "a2a_only");
    let channel = e2e.device.channel(support::HOST).await.unwrap();
    let signaling = droppen_client::rpc::rpc(channel, StreamClass::Mux, support::Stand::get(droppen_proto::path::SIGNAL_PATH)).await;
    match signaling {
        Ok(answer) => assert_eq!(answer.status, 403, "no direct path for an A2A proxy"),
        Err(failure) => assert!(matches!(failure, Failure::Refused { .. }), "{failure:?}"),
    }
    assert!(e2e.control.takes_a2a_calls_from(support::PROXY));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_that_stops_taking_the_proxy_and_its_removal_are_answered_from_the_proxys_chain() {
    let mut e2e = E2e::start(Duration::from_secs(60)).await;
    // A task that runs, followed over SSE.
    let sent: Value = e2e
        .rpc(
            "rpc-1",
            "SendMessage",
            json!({"message": message("m-1", "long job"), "configuration": {"returnImmediately": true}}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let task = sent["result"]["task"]["id"].as_str().expect("a task").to_string();
    e2e.started("m-1").await;
    let response = e2e
        .rest(reqwest::Method::GET, &format!("tasks/{task}:subscribe"), None)
        .send()
        .await
        .unwrap();
    let mut events = Events::new(response);
    events.next().await.unwrap().expect("the task first");

    // The host stops taking the proxy's calls (and renews its machine key).
    e2e.append(vec![
        Action::SetA2aAccept {
            host: support::HOST.into(),
            devices: Vec::new(),
        },
        support::Account::host_key(1),
    ]);
    // Its open task is taken back, and the stream ends.
    let revoked = support::eventually(SECS, || {
        e2e.a2a
            .store()
            .task(&task)
            .is_some_and(|t| t.state == droppen_a2a::wire::TaskState::Canceled)
    })
    .await;
    assert!(revoked, "the host's chain took the proxy's task back");
    let status = e2e.a2a.store().task(&task).unwrap().status_message.unwrap().metadata.unwrap();
    assert_eq!(status["errorInfo"]["metadata"]["code"], "droppen.revoked");
    loop {
        match events.next().await {
            Ok(Some(_)) => continue,
            Err(why) if why == NO_EVENT => panic!("the stream stayed open after the host stopped taking the proxy"),
            Ok(None) | Err(_) => break,
        }
    }
    // The next call: not accepted, by the proxy's own chain.
    let answer: Value = e2e
        .rpc("rpc-2", "GetTask", json!({"id": task}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(answer["id"], "rpc-2");
    assert_eq!(code_of(&answer), "droppen.not_accepted", "{answer}");

    // Removed from the account: device_removed, once the proxy synced its
    // chain again (a refused request syncs it, at most every 5 s).
    e2e.append(vec![Action::RemoveDevice3(support::PROXY.into())]);
    let deadline = tokio::time::Instant::now() + SYNC_EVERY + SECS;
    let mut code = String::new();
    while tokio::time::Instant::now() < deadline {
        let response = e2e.rest(reqwest::Method::GET, "tasks", None).send().await.unwrap();
        code = code_of(&response.json::<Value>().await.unwrap());
        if code == "droppen.device_removed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(code, "droppen.device_removed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_whose_view_nobody_vouched_for_in_seven_days_is_attestation_stale() {
    let e2e = E2e::start(Duration::from_secs(8 * 24 * 60 * 60)).await;
    let answer: Value = e2e
        .rpc("rpc-1", "SendMessage", json!({"message": message("m-1", "fix it")}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(answer["id"], "rpc-1");
    assert_eq!(code_of(&answer), "droppen.attestation_stale", "{answer}");
    let response = e2e.rest(reqwest::Method::GET, "tasks", None).send().await.unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(code_of(&response.json::<Value>().await.unwrap()), "droppen.attestation_stale");
    assert!(
        !e2e.fake.calls().iter().any(|call| matches!(call, Call::Start(_) | Call::Send(_))),
        "nothing reached the A2A server"
    );
}
