//! The A2A server end to end: requests through [`crate::protocol::router`],
//! both bindings, on a real store and the [`FakeBackend`].

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use gotcha::axum::body::Body;
use gotcha::axum::extract::ConnectInfo;
use gotcha::axum::http::{Method, Request, StatusCode};
use gotcha::axum::response::Response;
use gotcha::axum::Router;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use droppen_instance::db::Db;

use crate::backend::{event_channel, BackendError, Question, QuestionItem, QuestionOption, Resumed, SendKind, TurnOutcome};
use crate::engine::{A2a, Settings};
use crate::store::Store;
use crate::testing::{Call, FakeBackend};
use crate::wire::TaskState;

const LOOPBACK: [u8; 4] = [127, 0, 0, 1];

struct Harness {
    _dir: tempfile::TempDir,
    a2a: Arc<A2a>,
    fake: Arc<FakeBackend>,
    router: Router,
    _cancel: CancellationToken,
    token: String,
}

fn settings() -> Settings {
    Settings {
        version: "0.0.0-test".into(),
        public_base: None,
        tck: true,
        tck_streaming_timeout: Duration::from_millis(300),
    }
}

/// A server over the database in `dir`, with a fresh fake backend.
fn server(dir: &std::path::Path) -> (Arc<A2a>, Arc<FakeBackend>, CancellationToken) {
    let db = Arc::new(Db::open(dir.join("state.db")));
    let (events, inbox) = event_channel();
    let fake = FakeBackend::new(events);
    let cancel = CancellationToken::new();
    let a2a = A2a::new(Store::new(db), fake.clone(), inbox, settings(), cancel.clone());
    (a2a, fake, cancel)
}

async fn entry(a2a: &A2a, body: Value) {
    a2a.create_entry(serde_json::from_value(body).expect("an entry body"))
        .await
        .expect("the entry is made");
}

fn token(a2a: &A2a, name: &str, entries: &[&str]) -> String {
    a2a.create_token(serde_json::from_value(json!({"name": name, "entries": entries})).unwrap())
        .expect("a token")
        .secret
}

/// A server with one enabled Claude entry, `main`, and a token for it.
async fn harness() -> Harness {
    harness_with(json!({})).await
}

async fn harness_with(extra: Value) -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let (a2a, fake, cancel) = server(dir.path());
    let mut body = json!({"id": "main", "name": "Main", "source": "claude", "cwd": "/work", "enabled": true, "isolation": "none"});
    for (key, value) in extra.as_object().unwrap() {
        body[key] = value.clone();
    }
    entry(&a2a, body).await;
    let token = token(&a2a, "dont-do", &["main"]);
    Harness {
        _dir: dir,
        router: crate::protocol::router(a2a.clone()),
        a2a,
        fake,
        _cancel: cancel,
        token,
    }
}

fn request(method: Method, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:8700")
        .header("a2a-version", "1.0");
    if let Some(token) = token {
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
    request.extensions_mut().insert(ConnectInfo(SocketAddr::from((LOOPBACK, 40_000))));
    request
}

async fn call(router: &Router, request: Request<Body>) -> Response {
    tower_service::Service::call(&mut router.clone(), request).await.expect("infallible")
}

async fn json_of(response: Response) -> Value {
    let bytes = gotcha::axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn message(id: &str, text: &str) -> Value {
    json!({"messageId": id, "role": "ROLE_USER", "parts": [{"text": text, "mediaType": "text/plain"}]})
}

impl Harness {
    async fn rpc_as(&self, token: &str, method: &str, params: Value) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params});
        let response = call(&self.router, request(Method::POST, "/agents/main/jsonrpc", Some(token), Some(body))).await;
        assert_eq!(response.status(), StatusCode::OK);
        json_of(response).await
    }

    async fn rpc(&self, method: &str, params: Value) -> Value {
        self.rpc_as(&self.token.clone(), method, params).await
    }

    async fn rest(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let response = call(
            &self.router,
            request(method, &format!("/agents/main/rest/{path}"), Some(&self.token), body),
        )
        .await;
        let status = response.status();
        assert!(
            response.headers()["content-type"].to_str().unwrap().starts_with("application/a2a+json"),
            "HTTP+JSON answers in application/a2a+json"
        );
        (status, json_of(response).await)
    }

    async fn stream(&self, method: &str, params: Value) -> Events {
        let body = json!({"jsonrpc": "2.0", "id": 9, "method": method, "params": params});
        let response = call(&self.router, request(Method::POST, "/agents/main/jsonrpc", Some(&self.token), Some(body))).await;
        assert!(
            response.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"),
            "{:?}",
            response.headers()
        );
        Events::of(response)
    }

    /// A new task, sent without waiting: its id and context.
    async fn new_task(&self, id: &str) -> (String, String) {
        let answer = self
            .rpc(
                "SendMessage",
                json!({"message": message(id, "fix the bug"), "configuration": {"returnImmediately": true}}),
            )
            .await;
        let task = &answer["result"]["task"];
        (
            task["id"].as_str().unwrap_or_else(|| panic!("a task: {answer}")).to_string(),
            task["contextId"].as_str().unwrap().to_string(),
        )
    }

    async fn state(&self, task_id: &str) -> TaskState {
        self.a2a.store().task(task_id).expect("the task").state
    }

    async fn wait_state(&self, task_id: &str, state: TaskState) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while self.state(task_id).await != state {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the task never became {state:?}; it is {:?}",
                self.state(task_id).await
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn started(&self, task_id: &str) -> Call {
        let task_id = task_id.to_string();
        self.fake
            .wait_for(move |call| matches!(call, Call::Start(start) if start.task.task_id == task_id))
            .await
    }

    /// A task taken into its turn and working.
    async fn working(&self, id: &str) -> (String, String) {
        let (task, context) = self.new_task(id).await;
        self.started(&task).await;
        self.fake.turn_started(&task);
        self.wait_state(&task, TaskState::Working).await;
        (task, context)
    }

    fn question(&self) -> Question {
        Question {
            request_id: "req-1".into(),
            questions: vec![QuestionItem {
                question: "Which branch?".into(),
                header: "Branch".into(),
                multi_select: false,
                options: vec![
                    QuestionOption {
                        label: "main".into(),
                        description: "the default".into(),
                    },
                    QuestionOption {
                        label: "dev".into(),
                        description: String::new(),
                    },
                ],
            }],
        }
    }

    /// A task waiting on [`Harness::question`].
    async fn asking(&self, id: &str) -> String {
        let (task, _) = self.working(id).await;
        self.fake.ask(&task, self.question());
        self.wait_state(&task, TaskState::InputRequired).await;
        task
    }
}

/// An SSE answer, read event by event.
struct Events {
    body: gotcha::axum::body::BodyDataStream,
    buf: String,
}

impl Events {
    fn of(response: Response) -> Self {
        Events {
            body: response.into_body().into_data_stream(),
            buf: String::new(),
        }
    }

    async fn next(&mut self) -> Option<Value> {
        loop {
            if let Some(at) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..at + 2).collect();
                let data: Vec<&str> = block.lines().filter_map(|line| line.strip_prefix("data:")).map(str::trim).collect();
                if data.is_empty() {
                    continue;
                }
                return Some(serde_json::from_str(&data.join("\n")).expect("an event is JSON"));
            }
            match tokio::time::timeout(Duration::from_secs(30), self.body.next()).await {
                Ok(Some(Ok(bytes))) => self.buf.push_str(&String::from_utf8_lossy(&bytes)),
                Ok(_) => return None,
                Err(_) => panic!("the stream stalled; it had {:?}", self.buf),
            }
        }
    }

    /// The `result`s of the JSON-RPC events, or the bare items, up to the
    /// stream's end.
    async fn results(mut self) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(event) = self.next().await {
            out.push(event.get("result").cloned().unwrap_or(event));
        }
        out
    }
}

/// The kind of a `StreamResponse`.
fn kind(item: &Value) -> &str {
    ["task", "message", "statusUpdate", "artifactUpdate"]
        .into_iter()
        .find(|key| item.get(*key).is_some())
        .unwrap_or("?")
}

fn state_of(item: &Value) -> &str {
    item["statusUpdate"]["status"]["state"]
        .as_str()
        .or_else(|| item["task"]["status"]["state"].as_str())
        .unwrap_or("")
}

fn calls_of<T>(fake: &FakeBackend, pick: impl Fn(&Call) -> Option<T>) -> Vec<T> {
    fake.calls().iter().filter_map(pick).collect()
}

/* ── the card and who may call ───────────────────────────────────────── */

#[tokio::test]
async fn the_card_needs_a_token_and_says_nothing_of_entries_it_does_not_reach() {
    let h = harness().await;
    entry(
        &h.a2a,
        json!({"id": "other", "name": "Other", "source": "codex", "cwd": "/elsewhere", "enabled": true}),
    )
    .await;
    let card = |token: Option<&str>, entry: &str| request(Method::GET, &format!("/agents/{entry}/.well-known/agent-card.json"), token, None);

    let none = call(&h.router, card(None, "main")).await;
    assert_eq!(none.status(), StatusCode::UNAUTHORIZED);
    assert!(none.headers().contains_key("www-authenticate"));
    let wrong = call(&h.router, card(Some("dpa_0011223344556677_wrong"), "main")).await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let unknown = call(&h.router, card(Some(&h.token), "nowhere")).await;
    let unreached = call(&h.router, card(Some(&h.token), "other")).await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(unreached.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        json_of(unknown).await,
        json_of(unreached).await,
        "not reached and not there are told apart by nothing"
    );

    let response = call(&h.router, card(Some(&h.token), "main")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()["etag"].to_str().unwrap().to_string();
    assert!(response.headers().contains_key("cache-control"));
    let card_json = json_of(response).await;
    assert_eq!(card_json["name"], "Main");
    assert_eq!(card_json["version"], "0.0.0-test");
    assert_eq!(
        card_json["supportedInterfaces"],
        json!([
            {"url": "http://127.0.0.1:8700/a2a/agents/main/jsonrpc", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
            {"url": "http://127.0.0.1:8700/a2a/agents/main/rest", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"},
        ])
    );
    assert_eq!(
        card_json["capabilities"],
        json!({"streaming": true, "pushNotifications": true, "extendedAgentCard": false})
    );
    assert_eq!(
        card_json["securitySchemes"],
        json!({"bearer": {"httpAuthSecurityScheme": {"scheme": "Bearer"}}})
    );
    assert_eq!(
        card_json["defaultInputModes"],
        json!(["text/plain", "application/json", "image/png", "image/jpeg"])
    );
    assert_eq!(
        card_json["defaultOutputModes"],
        json!(["text/plain", "text/markdown", "application/json"])
    );
    assert_eq!(card_json["skills"][0]["id"], "coding_session");
    assert!(!card_json.to_string().contains("/work"), "the card names no directory");

    let mut again = card(Some(&h.token), "main");
    again.headers_mut().insert("if-none-match", etag.parse().unwrap());
    assert_eq!(call(&h.router, again).await.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn a_token_is_taken_over_loopback_only() {
    let h = harness().await;
    let mut from_lan = request(Method::GET, "/agents/main/.well-known/agent-card.json", Some(&h.token), None);
    from_lan
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([192, 168, 1, 20], 40_000))));
    assert_eq!(call(&h.router, from_lan).await.status(), StatusCode::FORBIDDEN);

    let mut no_peer = request(Method::GET, "/agents/main/.well-known/agent-card.json", Some(&h.token), None);
    no_peer.extensions_mut().remove::<ConnectInfo<SocketAddr>>();
    assert_eq!(
        call(&h.router, no_peer).await.status(),
        StatusCode::FORBIDDEN,
        "an unknown peer is not loopback"
    );

    let mut mapped = request(Method::GET, "/agents/main/.well-known/agent-card.json", Some(&h.token), None);
    mapped
        .extensions_mut()
        .insert(ConnectInfo("[::ffff:127.0.0.1]:40000".parse::<SocketAddr>().unwrap()));
    assert_eq!(
        call(&h.router, mapped).await.status(),
        StatusCode::OK,
        "IPv4 loopback in an IPv6 socket is loopback"
    );
}

#[tokio::test]
async fn every_binding_refuses_a_missing_wrong_revoked_or_unscoped_token() {
    let h = harness().await;
    entry(
        &h.a2a,
        json!({"id": "other", "name": "Other", "source": "claude", "cwd": "/w", "enabled": true}),
    )
    .await;
    let revoked = token(&h.a2a, "old", &["main"]);
    let id = crate::token::parse(&revoked).unwrap().0.to_string();
    h.a2a.revoke_token(&id).await.unwrap();
    let rpc = json!({"jsonrpc": "2.0", "id": 1, "method": "ListTasks", "params": {}});
    let (token_id, secret) = crate::token::parse(&h.token).unwrap();
    let wrong = format!("dpa_{token_id}_{}", &secret[1..]);
    for token in [None, Some(wrong.as_str()), Some(revoked.as_str()), Some("a-login-token")] {
        let response = call(&h.router, request(Method::POST, "/agents/main/jsonrpc", token, Some(rpc.clone()))).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{token:?} on JSON-RPC");
        let response = call(&h.router, request(Method::GET, "/agents/main/rest/tasks", token, None)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{token:?} on HTTP+JSON");
    }
    let response = call(&h.router, request(Method::POST, "/agents/other/jsonrpc", Some(&h.token), Some(rpc))).await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "an entry outside the token's scope is not there"
    );
    let response = call(&h.router, request(Method::GET, "/agents/other/rest/tasks", Some(&h.token), None)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_request_without_a2a_version_1_0_is_refused() {
    let h = harness().await;
    let mut old = request(
        Method::POST,
        "/agents/main/jsonrpc",
        Some(&h.token),
        Some(json!({"jsonrpc": "2.0", "id": 3, "method": "ListTasks"})),
    );
    old.headers_mut().remove("a2a-version");
    let body = json_of(call(&h.router, old).await).await;
    assert_eq!(body["id"], 3);
    assert_eq!(body["error"]["code"], -32009, "no version is 0.3, which is not spoken: {body}");
    assert_eq!(body["error"]["data"][0]["reason"], "VERSION_NOT_SUPPORTED");
    assert_eq!(h.rpc("NoSuchMethod", json!({})).await["error"]["code"], -32601);
}

/* ── new tasks, idempotency, contexts ────────────────────────────────── */

#[tokio::test]
async fn a_new_task_starts_one_session_however_often_its_message_is_sent() {
    let h = harness().await;
    let (task, context) = h.new_task("m-1").await;
    let Call::Start(start) = h.started(&task).await else { unreachable!() };
    assert_eq!(start.task.context_id, context);
    let caller = format!("a2a:token:{}", crate::token::parse(&h.token).unwrap().0);
    assert_eq!(start.task.caller, caller, "the caller is the token's id, never its name");
    assert_eq!(start.client_message_id, format!("a2a:{caller}:m-1"));
    assert_eq!(start.run.cwd, "/work");
    assert!(start.prompt.text.contains("fix the bug"));
    assert!(
        start.prompt.text.contains(&format!("dont-do ({caller})")),
        "the agent is told the token's name beside its id"
    );

    let (again, again_context) = h.new_task("m-1").await;
    assert_eq!(
        (again.as_str(), again_context.as_str()),
        (task.as_str(), context.as_str()),
        "a retry finds the first task"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    let starts = calls_of(&h.fake, |call| matches!(call, Call::Start(_)).then_some(()));
    assert_eq!(starts.len(), 1, "a retry starts no second session");
}

#[tokio::test]
async fn the_request_is_fenced_with_a_fence_it_cannot_know() {
    let h = harness().await;
    let parts = json!([
        {"text": "do this", "mediaType": "text/plain"},
        {"data": {"todo": {"id": 1}}, "mediaType": "application/json"},
    ]);
    let answer = h
        .rpc(
            "SendMessage",
            json!({"message": {"messageId": "f-1", "role": "ROLE_USER", "parts": parts}, "configuration": {"returnImmediately": true}}),
        )
        .await;
    let task = answer["result"]["task"]["id"].as_str().unwrap().to_string();
    let Call::Start(start) = h.started(&task).await else { unreachable!() };
    let text = start.prompt.text;
    let open = text.find("<<<A2A-REQUEST-").expect("a fence");
    let fence: String = text[open + 3..].chars().take_while(|c| !c.is_whitespace()).collect();
    assert!(
        text.contains(&format!("<<<{fence}\ndo this\n\n{{\"todo\":{{\"id\":1}}}}\n{fence}>>>")),
        "{text}"
    );
}

#[tokio::test]
async fn a_follow_up_goes_to_its_context_and_no_other_callers_context_is_taken() {
    let h = harness().await;
    let (first, context) = h.working("m-1").await;
    h.fake.complete(&first, "done");
    h.wait_state(&first, TaskState::Completed).await;

    let follow_up = json!({"messageId": "m-2", "contextId": context, "role": "ROLE_USER", "parts": [{"text": "and the tests"}]});
    let answer = h
        .rpc("SendMessage", json!({"message": follow_up, "configuration": {"returnImmediately": true}}))
        .await;
    let second = answer["result"]["task"]["id"].as_str().unwrap().to_string();
    assert_ne!(second, first);
    assert_eq!(answer["result"]["task"]["contextId"], context);
    let id = second.clone();
    let Call::Send(send) = h
        .fake
        .wait_for(move |call| matches!(call, Call::Send(send) if send.task.task_id == id))
        .await
    else {
        unreachable!()
    };
    assert_eq!(send.kind, SendKind::NewTurn);
    assert_eq!(send.task.session.session_id.as_deref(), Some("session-1"), "the context's session");

    let stranger = token(&h.a2a, "stranger", &["main"]);
    let foreign = json!({"messageId": "s-1", "contextId": context, "role": "ROLE_USER", "parts": [{"text": "mine now"}]});
    let unknown = json!({"messageId": "s-2", "contextId": "no-such-context", "role": "ROLE_USER", "parts": [{"text": "x"}]});
    let foreign = h.rpc_as(&stranger, "SendMessage", json!({"message": foreign})).await;
    let unknown = h.rpc_as(&stranger, "SendMessage", json!({"message": unknown})).await;
    assert_eq!(foreign["error"]["code"], -32602);
    assert_eq!(foreign["error"], unknown["error"], "another caller's context is not told from none");
}

/* ── answers, appends, terminal tasks ────────────────────────────────── */

#[tokio::test]
async fn a_question_is_text_and_data_and_takes_an_answer_of_either() {
    let h = harness().await;
    let task = h.asking("m-1").await;
    let got = h.rpc("GetTask", json!({"id": task})).await;
    let status = &got["result"]["status"];
    assert_eq!(status["state"], "TASK_STATE_INPUT_REQUIRED");
    assert!(status["message"]["parts"][0]["text"].as_str().unwrap().contains("Which branch?"));
    assert_eq!(
        status["message"]["parts"][1]["data"],
        json!({"kind": "question", "requestId": "req-1", "questions": [{"question": "Which branch?", "header": "Branch", "multiSelect": false,
            "options": [{"label": "main", "description": "the default"}, {"label": "dev", "description": ""}]}]})
    );

    let data = json!({"messageId": "a-1", "taskId": task, "role": "ROLE_USER",
        "parts": [{"data": {"answers": {"Which branch?": "main"}, "notes": {"Which branch?": "please"}, "requestId": "req-1"}}]});
    let answered = h
        .rpc("SendMessage", json!({"message": data, "configuration": {"returnImmediately": true}}))
        .await;
    assert_eq!(answered["result"]["task"]["status"]["state"], "TASK_STATE_WORKING", "{answered}");
    let answers = calls_of(&h.fake, |call| {
        if let Call::Answer(_, answer) = call {
            Some(answer.clone())
        } else {
            None
        }
    });
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].request_id, "req-1");
    assert_eq!(serde_json::to_value(&answers[0].answers).unwrap(), json!({"Which branch?": "main"}));
    assert_eq!(answers[0].notes.get("Which branch?").map(String::as_str), Some("please"));
    assert_eq!(answers[0].response, None);

    h.fake.ask(
        &task,
        Question {
            request_id: "req-2".into(),
            ..h.question()
        },
    );
    h.wait_state(&task, TaskState::InputRequired).await;
    let text = json!({"messageId": "a-2", "taskId": task, "role": "ROLE_USER", "parts": [{"text": "use dev, and rebase"}]});
    h.rpc("SendMessage", json!({"message": text, "configuration": {"returnImmediately": true}}))
        .await;
    let answers = calls_of(&h.fake, |call| {
        if let Call::Answer(_, answer) = call {
            Some(answer.clone())
        } else {
            None
        }
    });
    let last = answers.last().unwrap();
    assert_eq!(
        (last.request_id.as_str(), last.response.as_deref()),
        ("req-2", Some("use dev, and rebase"))
    );
    assert!(last.answers.is_empty());

    h.fake.ask(
        &task,
        Question {
            request_id: "req-3".into(),
            ..h.question()
        },
    );
    h.wait_state(&task, TaskState::InputRequired).await;
    let stale = json!({"messageId": "a-3", "taskId": task, "role": "ROLE_USER", "parts": [{"data": {"answers": {"q": "x"}, "requestId": "req-1"}}]});
    let refused = h.rpc("SendMessage", json!({"message": stale})).await;
    assert_eq!(refused["error"]["code"], -32602, "an answer to a question that is not open: {refused}");
}

#[tokio::test]
async fn an_entry_that_takes_no_answers_says_so_with_its_code() {
    let h = harness_with(json!({"allowRemoteAnswers": false})).await;
    let task = h.asking("m-1").await;
    let answer = json!({"messageId": "a-1", "taskId": task, "role": "ROLE_USER", "parts": [{"text": "main"}]});
    let refused = h.rpc("SendMessage", json!({"message": answer})).await;
    assert_eq!(refused["error"]["code"], -32004);
    assert_eq!(
        refused["error"]["data"][1],
        json!({"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "REMOTE_ANSWERS_DISABLED", "domain": "droppen.app",
            "metadata": {"code": "droppen.remote_answers_disabled"}})
    );
    let (status, body) = h.rest(Method::POST, "message:send", Some(json!({"message": answer}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["details"], refused["error"]["data"], "the same details in both bindings");
    assert!(!h.fake.calls().iter().any(|call| matches!(call, Call::Answer(..))));
}

#[tokio::test]
async fn more_for_a_working_task_goes_into_its_turn() {
    let h = harness().await;
    let (task, _) = h.working("m-1").await;
    let more = json!({"messageId": "m-2", "taskId": task, "role": "ROLE_USER", "parts": [{"text": "also update the docs"}]});
    let answer = h
        .rpc("SendMessage", json!({"message": more, "configuration": {"returnImmediately": true}}))
        .await;
    assert_eq!(answer["result"]["task"]["id"], task);
    let Call::Send(send) = h.fake.wait_for(|call| matches!(call, Call::Send(_))).await else {
        unreachable!()
    };
    assert_eq!(send.kind, SendKind::Append);
    assert!(send.prompt.text.contains("also update the docs"));
    let history = h.rpc("GetTask", json!({"id": task})).await["result"]["history"].clone();
    assert_eq!(history.as_array().unwrap().len(), 2);
    let one = h.rpc("GetTask", json!({"id": task, "historyLength": 1})).await["result"]["history"].clone();
    assert_eq!(one.as_array().unwrap().len(), 1, "historyLength cuts it");
    assert!(h.rpc("GetTask", json!({"id": task, "historyLength": 0})).await["result"]
        .get("history")
        .is_none());
}

#[tokio::test]
async fn a_finished_task_takes_no_message_no_cancel_and_no_subscription() {
    let h = harness().await;
    let (task, _) = h.working("m-1").await;
    h.fake.complete(&task, "done");
    h.wait_state(&task, TaskState::Completed).await;
    let more = json!({"messageId": "m-2", "taskId": task, "role": "ROLE_USER", "parts": [{"text": "again"}]});
    assert_eq!(h.rpc("SendMessage", json!({"message": more})).await["error"]["code"], -32004);
    let cancel = h.rpc("CancelTask", json!({"id": task})).await;
    assert_eq!(cancel["error"]["code"], -32002);
    assert_eq!(cancel["error"]["data"][0]["reason"], "TASK_NOT_CANCELABLE");
    assert_eq!(h.rpc("SubscribeToTask", json!({"id": task})).await["error"]["code"], -32004);
    let (status, body) = h.rest(Method::POST, &format!("tasks/{task}:cancel"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["details"][0]["reason"], "TASK_NOT_CANCELABLE");
}

/* ── cancel and slots ────────────────────────────────────────────────── */

#[tokio::test]
async fn a_task_waiting_for_a_slot_is_withdrawn_and_a_working_one_interrupted() {
    let h = harness().await;
    let (first, _) = h.working("m-1").await;
    let (queued, _) = h.new_task("m-2").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!h.a2a.store().task(&queued).unwrap().dispatched, "one slot, taken: the second waits");

    let canceled = h.rpc("CancelTask", json!({"id": queued})).await;
    assert_eq!(canceled["result"]["status"]["state"], "TASK_STATE_CANCELED");
    assert!(
        h.fake.calls().iter().all(|call| call.task_id() != Some(queued.as_str())),
        "it was never handed over"
    );

    let canceled = h.rpc("CancelTask", json!({"id": first})).await;
    assert_eq!(canceled["result"]["status"]["state"], "TASK_STATE_CANCELED");
    let cancels = calls_of(&h.fake, |call| {
        if let Call::Cancel(task) = call {
            Some(task.task_id.clone())
        } else {
            None
        }
    });
    assert_eq!(cancels, [first.clone()]);
    h.fake.complete(&first, "too late");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        h.state(&first).await,
        TaskState::Canceled,
        "what the backend says of a canceled task is dropped"
    );
}

#[tokio::test]
async fn a_slot_frees_when_a_task_ends_or_asks() {
    let h = harness().await;
    let (first, _) = h.working("m-1").await;
    let (second, _) = h.new_task("m-2").await;
    h.fake.ask(&first, h.question());
    h.started(&second).await;
}

/* ── streams ─────────────────────────────────────────────────────────── */

#[tokio::test]
async fn a_stream_opens_with_the_task_and_closes_when_it_is_done() {
    let h = harness().await;
    let events = h.stream("SendStreamingMessage", json!({"message": message("m-1", "go")})).await;
    let fake = h.fake.clone();
    let a2a = h.a2a.clone();
    let drive = tokio::spawn(async move {
        let Call::Start(start) = fake.wait_for(|call| matches!(call, Call::Start(_))).await else {
            unreachable!()
        };
        let task = start.task.task_id;
        fake.turn_started(&task);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while a2a.store().task(&task).unwrap().state != TaskState::Working {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        fake.progress(&task, "reading the code");
        fake.complete(&task, "Fixed it. https://github.com/o/r/pull/1");
    });
    let items = events.results().await;
    drive.await.unwrap();
    let kinds: Vec<&str> = items.iter().map(kind).collect();
    assert_eq!(
        kinds,
        ["task", "statusUpdate", "statusUpdate", "artifactUpdate", "artifactUpdate", "statusUpdate"],
        "{items:#?}"
    );
    assert_eq!(state_of(&items[0]), "TASK_STATE_SUBMITTED");
    assert_eq!(state_of(&items[1]), "TASK_STATE_WORKING");
    assert_eq!(items[2]["statusUpdate"]["status"]["message"]["parts"][0]["text"], "reading the code");
    assert_eq!(items[3]["artifactUpdate"]["artifact"]["artifactId"], "summary");
    assert_eq!(items[3]["artifactUpdate"]["artifact"]["parts"][0]["mediaType"], "text/markdown");
    assert_eq!(items[3]["artifactUpdate"]["lastChunk"], true);
    assert!(items[3]["artifactUpdate"].get("append").is_none(), "append = false is left out");
    assert_eq!(items[4]["artifactUpdate"]["artifact"]["artifactId"], "result");
    assert_eq!(items[4]["artifactUpdate"]["artifact"]["parts"][0]["data"]["host"], "test-host");
    assert_eq!(state_of(&items[5]), "TASK_STATE_COMPLETED");
    let task = items[0]["task"]["id"].as_str().unwrap();
    let got = h.rpc("GetTask", json!({"id": task})).await;
    assert_eq!(
        got["result"]["artifacts"].as_array().unwrap().len(),
        2,
        "the final task carries both artifacts"
    );
}

#[tokio::test]
async fn a_stream_closes_when_the_task_asks() {
    let h = harness().await;
    let (task, _) = h.working("m-1").await;
    let mut events = h.stream("SubscribeToTask", json!({"id": task})).await;
    let first = events.next().await.unwrap();
    assert_eq!(kind(&first["result"]), "task", "a subscription opens with the task");
    assert_eq!(first["id"], 9, "each event answers the request's id");
    h.fake.ask(&task, h.question());
    let asked = events.next().await.unwrap();
    assert_eq!(state_of(&asked["result"]), "TASK_STATE_INPUT_REQUIRED");
    assert!(events.next().await.is_none(), "the stream closes at INPUT_REQUIRED");
}

#[tokio::test]
async fn every_stream_of_a_task_sees_the_same_events() {
    let h = harness().await;
    let (task, _) = h.working("m-1").await;
    let one = h.stream("SubscribeToTask", json!({"id": task})).await;
    let two = h.stream("SubscribeToTask", json!({"id": task})).await;
    h.fake.progress(&task, "step one");
    h.fake.fail(&task, "the CLI crashed");
    let (one, two) = (one.results().await, two.results().await);
    let rest = |items: &[Value]| -> Vec<String> { items[1..].iter().map(Value::to_string).collect() };
    assert_eq!(rest(&one), rest(&two), "the same events, in the same order");
    assert_eq!(state_of(one.last().unwrap()), "TASK_STATE_FAILED");
}

#[tokio::test]
async fn a_rest_stream_carries_the_bare_stream_responses() {
    let h = harness().await;
    let (task, _) = h.working("m-1").await;
    let response = call(
        &h.router,
        request(Method::GET, &format!("/agents/main/rest/tasks/{task}:subscribe"), Some(&h.token), None),
    )
    .await;
    assert!(response.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let mut events = Events::of(response);
    let first = events.next().await.unwrap();
    assert_eq!(first["task"]["id"], task);
    h.fake.complete(&task, "done");
    let items = events.results().await;
    assert_eq!(state_of(items.last().unwrap()), "TASK_STATE_COMPLETED");
}

#[tokio::test(start_paused = true)]
async fn progress_is_one_status_message_per_five_seconds_and_the_latest_one() {
    let h = harness().await;
    let (task, _) = h.working("m-1").await;
    let mut events = h.stream("SubscribeToTask", json!({"id": task})).await;
    events.next().await.unwrap();
    h.fake.progress(&task, "one");
    h.fake.progress(&task, "two");
    h.fake.progress(&task, &"three ".repeat(100));
    let first = events.next().await.unwrap();
    assert_eq!(first["result"]["statusUpdate"]["status"]["message"]["parts"][0]["text"], "one");
    let second = events.next().await.unwrap();
    let text = second["result"]["statusUpdate"]["status"]["message"]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text.starts_with("three"), "the latest wins: {text}");
    assert!(text.chars().count() <= 280);
    h.fake.complete(&task, "done");
    let rest = events.results().await;
    assert_eq!(
        rest.iter().filter(|item| kind(item) == "statusUpdate").count(),
        1,
        "nothing of \"two\": {rest:?}"
    );
}

/* ── lists, isolation, restarts ──────────────────────────────────────── */

#[tokio::test]
async fn a_caller_sees_only_its_own_tasks() {
    let h = harness().await;
    let (mine, _) = h.new_task("m-1").await;
    let other = token(&h.a2a, "other", &["main"]);
    let theirs = h
        .rpc_as(
            &other,
            "SendMessage",
            json!({"message": message("o-1", "theirs"), "configuration": {"returnImmediately": true}}),
        )
        .await;
    let theirs = theirs["result"]["task"]["id"].as_str().unwrap().to_string();

    let list = h.rpc("ListTasks", json!({})).await["result"].clone();
    let ids: Vec<&str> = list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [mine.as_str()]);
    assert_eq!(list["totalSize"], 1);
    assert_eq!(list["nextPageToken"], "");
    assert!(list["tasks"][0].get("artifacts").is_none(), "no artifacts unless asked for");

    let foreign = h.rpc("GetTask", json!({"id": theirs})).await;
    let none = h.rpc("GetTask", json!({"id": "no-such-task"})).await;
    assert_eq!(foreign["error"]["code"], -32001);
    assert_eq!(foreign["error"], none["error"], "another caller's task is not told from none");
    assert_eq!(h.rpc("CancelTask", json!({"id": theirs})).await["error"]["code"], -32001);
    let answer = json!({"messageId": "x-1", "taskId": theirs, "role": "ROLE_USER", "parts": [{"text": "hijack"}]});
    assert_eq!(h.rpc("SendMessage", json!({"message": answer})).await["error"]["code"], -32001);
    assert_eq!(
        h.rpc("ListTaskPushNotificationConfigs", json!({"taskId": theirs})).await["error"]["code"],
        -32001
    );
    assert_eq!(h.state(&theirs).await, TaskState::Submitted, "and it stays theirs, untouched");
}

#[tokio::test]
async fn a_list_pages_newest_first() {
    let h = harness_with(json!({"maxConcurrent": 8})).await;
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(h.new_task(&format!("m-{n}")).await.0);
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    let first = h.rpc("ListTasks", json!({"pageSize": 2})).await["result"].clone();
    assert_eq!(first["pageSize"], 2);
    assert_eq!(first["totalSize"], 5);
    let token = first["nextPageToken"].as_str().unwrap().to_string();
    assert!(!token.is_empty());
    let second = h.rpc("ListTasks", json!({"pageSize": 2, "page_token": token})).await["result"].clone();
    let seen: Vec<String> = first["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["tasks"].as_array().unwrap())
        .map(|task| task["id"].as_str().unwrap().to_string())
        .collect();
    let newest_first: Vec<String> = ids.iter().rev().cloned().collect();
    assert_eq!(seen, newest_first[..4]);
    assert_eq!(h.rpc("ListTasks", json!({"pageSize": 0})).await["error"]["code"], -32602);
    assert_eq!(h.rpc("ListTasks", json!({"pageToken": "nonsense"})).await["error"]["code"], -32602);
}

#[tokio::test]
async fn tasks_survive_a_restart_and_are_followed_again() {
    let dir = tempfile::tempdir().unwrap();
    let (done, working, waiting) = {
        let (a2a, fake, cancel) = server(dir.path());
        let body =
            json!({"id": "main", "name": "Main", "source": "claude", "cwd": "/work", "enabled": true, "isolation": "none", "maxConcurrent": 2});
        entry(&a2a, body).await;
        let secret = token(&a2a, "dont-do", &["main"]);
        let router = crate::protocol::router(a2a.clone());
        let mut ids = Vec::new();
        for id in ["m-1", "m-2", "m-3"] {
            let params = json!({"message": message(id, "x"), "configuration": {"returnImmediately": true}});
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": "SendMessage", "params": params});
            let answer = json_of(call(&router, request(Method::POST, "/agents/main/jsonrpc", Some(&secret), Some(body))).await).await;
            ids.push(answer["result"]["task"]["id"].as_str().unwrap().to_string());
        }
        for id in &ids[..2] {
            let id = id.clone();
            fake.wait_for(move |call| matches!(call, Call::Start(start) if start.task.task_id == id))
                .await;
        }
        cancel.cancel();
        (ids[0].clone(), ids[1].clone(), ids[2].clone())
    };

    let (a2a, fake, _cancel) = server(dir.path());
    fake.on_resume(
        &done,
        Ok(Resumed::Ended(TurnOutcome::Completed(FakeBackend::result("finished while down")))),
    );
    fake.on_resume(&working, Ok(Resumed::Working));
    a2a.resume().await;
    assert_eq!(a2a.store().task(&done).unwrap().state, TaskState::Completed);
    assert_eq!(a2a.store().task(&done).unwrap().artifacts.len(), 2);
    assert_eq!(a2a.store().task(&working).unwrap().state, TaskState::Working);
    let id = waiting.clone();
    fake.wait_for(move |call| matches!(call, Call::Start(start) if start.task.task_id == id))
        .await;
    let resumed = calls_of(&fake, |call| {
        if let Call::Resume(task) = call {
            Some(task.task_id.clone())
        } else {
            None
        }
    });
    assert_eq!(resumed.len(), 2, "only what had been handed over is resumed: {resumed:?}");
}

/* ── the two bindings ────────────────────────────────────────────────── */

/// A task as JSON, without what differs between two tasks of the same history.
fn shape(mut task: Value) -> Value {
    for key in ["id", "contextId"] {
        task[key] = json!("*");
    }
    task["status"]["timestamp"] = json!("*");
    if task["status"].get("message").is_some() {
        for key in ["messageId", "taskId", "contextId"] {
            task["status"]["message"][key] = json!("*");
        }
    }
    if let Some(history) = task["history"].as_array_mut() {
        for message in history {
            for key in ["messageId", "taskId", "contextId"] {
                message[key] = json!("*");
            }
        }
    }
    task
}

#[tokio::test]
async fn both_bindings_answer_alike() {
    let h = harness_with(json!({"maxConcurrent": 8})).await;
    let send = |id: &str| json!({"message": message(id, "same request"), "configuration": {"returnImmediately": true}});
    let by_rpc = h.rpc("SendMessage", send("r-1")).await["result"]["task"].clone();
    let (status, by_rest) = h.rest(Method::POST, "message:send", Some(send("r-2"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shape(by_rpc.clone()), shape(by_rest["task"].clone()));
    let rest_id = by_rest["task"]["id"].as_str().unwrap().to_string();

    let rpc_get = h.rpc("GetTask", json!({"id": by_rpc["id"]})).await["result"].clone();
    let (_, rest_get) = h.rest(Method::GET, &format!("tasks/{rest_id}"), None).await;
    assert_eq!(shape(rpc_get), shape(rest_get));

    let rpc_list = h.rpc("ListTasks", json!({"pageSize": 10})).await["result"].clone();
    let (_, rest_list) = h.rest(Method::GET, "tasks?pageSize=10", None).await;
    assert_eq!(rpc_list, rest_list);

    let rpc_missing = h.rpc("GetTask", json!({"id": "nope"})).await["error"].clone();
    let (status, rest_missing) = h.rest(Method::GET, "tasks/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(rpc_missing["code"], -32001);
    assert_eq!(rest_missing["error"]["code"], 404);
    assert_eq!(rest_missing["error"]["status"], "NOT_FOUND");
    assert_eq!(rpc_missing["data"], rest_missing["error"]["details"], "the same ErrorInfo");

    let rpc_cancel = h.rpc("CancelTask", json!({"id": by_rpc["id"]})).await["result"].clone();
    let (_, rest_cancel) = h.rest(Method::POST, &format!("tasks/{rest_id}:cancel"), None).await;
    assert_eq!(shape(rpc_cancel), shape(rest_cancel));
}

#[tokio::test]
async fn a_blocking_send_waits_for_the_task_to_settle() {
    let h = harness().await;
    let fake = h.fake.clone();
    tokio::spawn(async move {
        let Call::Start(start) = fake.wait_for(|call| matches!(call, Call::Start(_))).await else {
            unreachable!()
        };
        fake.turn_started(&start.task.task_id);
        fake.complete(&start.task.task_id, "all done");
    });
    let (status, answer) = h.rest(Method::POST, "message:send", Some(json!({"message": message("b-1", "go")}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["task"]["status"]["state"], "TASK_STATE_COMPLETED");
    assert_eq!(answer["task"]["artifacts"][0]["parts"][0]["text"], "all done");
}

/* ── what a message may carry ────────────────────────────────────────── */

#[tokio::test]
async fn parts_outside_the_cards_input_modes_are_refused() {
    let h = harness().await;
    let with = |part: Value| json!({"message": {"messageId": uuid::Uuid::new_v4().to_string(), "role": "ROLE_USER", "parts": [part]}});
    let unsupported = h
        .rpc(
            "SendMessage",
            with(json!({"raw": "dGNr", "mediaType": "application/x-unsupported-tck-type"})),
        )
        .await;
    assert_eq!(unsupported["error"]["code"], -32005);
    assert_eq!(unsupported["error"]["data"][0]["reason"], "CONTENT_TYPE_NOT_SUPPORTED");
    let by_url = h
        .rpc(
            "SendMessage",
            with(json!({"url": "http://169.254.169.254/latest", "mediaType": "image/png"})),
        )
        .await;
    assert_eq!(by_url["error"]["code"], -32005, "a file by URL is never fetched");
    assert_eq!(h.rpc("SendMessage", json!({})).await["error"]["code"], -32602);
    assert_eq!(
        h.rpc("SendMessage", json!({"message": {"role": "ROLE_USER", "parts": [{"text": "x"}]}}))
            .await["error"]["code"],
        -32602
    );

    let image = json!({"raw": "iVBORw0KGgo=", "mediaType": "image/png", "filename": "../../etc/passwd"});
    let params = json!({"message": {"messageId": "i-1", "role": "ROLE_USER", "parts": [{"text": "look"}, image]}, "configuration": {"returnImmediately": true}});
    let answer = h.rpc("SendMessage", params).await;
    let task = answer["result"]["task"]["id"].as_str().unwrap().to_string();
    let Call::Start(start) = h.started(&task).await else { unreachable!() };
    assert_eq!(start.prompt.images.len(), 1);
    assert_eq!(
        start.prompt.images[0].filename.as_deref(),
        Some("passwd"),
        "a file name is a label, never a path"
    );
}

/* ── revocation and quotas ───────────────────────────────────────────── */

#[tokio::test]
async fn revoking_a_token_takes_back_its_tasks_pushes_and_streams() {
    let h = harness_with(json!({"pushOrigins": ["https://hooks.example.com"]})).await;
    let (working, _) = h.working("m-1").await;
    let (queued, _) = h.new_task("m-2").await;
    let push = h
        .rpc(
            "CreateTaskPushNotificationConfig",
            json!({"taskId": working, "url": "https://hooks.example.com/a2a", "token": "t"}),
        )
        .await;
    assert!(push["result"]["id"].is_string(), "{push}");
    let mut stream = h.stream("SubscribeToTask", json!({"id": working})).await;
    stream.next().await.unwrap();

    let id = crate::token::parse(&h.token).unwrap().0.to_string();
    h.a2a.revoke_token(&id).await.unwrap();
    assert!(stream.next().await.is_none(), "the open stream is closed, with nothing more on it");
    for task in [&working, &queued] {
        let row = h.a2a.store().task(task).unwrap();
        assert_eq!(row.state, TaskState::Canceled);
        assert_eq!(
            row.status_message.unwrap().metadata.unwrap()["errorInfo"]["metadata"]["code"],
            "droppen.revoked"
        );
    }
    let cancels = calls_of(&h.fake, |call| {
        if let Call::Cancel(task) = call {
            Some(task.task_id.clone())
        } else {
            None
        }
    });
    assert_eq!(
        cancels,
        [working.clone()],
        "the running one is interrupted; the queued one was never handed over"
    );
    assert!(h.a2a.store().pushes(&working).is_empty(), "its push configurations are gone");
    assert!(
        !h.a2a.caller_reaches(&format!("a2a:token:{id}"), "main"),
        "a push asks again, and is told no"
    );
}

#[tokio::test]
async fn switching_an_entry_off_cancels_its_tasks() {
    let h = harness().await;
    let (task, _) = h.working("m-1").await;
    h.a2a
        .update_entry("main", serde_json::from_value(json!({"enabled": false})).unwrap())
        .await
        .unwrap();
    assert_eq!(h.state(&task).await, TaskState::Canceled);
    let refused = h.rpc("SendMessage", json!({"message": message("m-2", "x")})).await;
    assert_eq!(refused["error"]["data"][1]["metadata"]["code"], "droppen.entry_disabled");
}

#[tokio::test]
async fn taking_a_device_out_of_an_entry_cancels_that_devices_tasks() {
    let h = harness_with(json!({"allowedDevices": ["d-1", "d-2"]})).await;
    assert!(h.a2a.caller_reaches("a2a:device:d-1", "main"));
    h.a2a
        .update_entry("main", serde_json::from_value(json!({"allowedDevices": ["d-2"]})).unwrap())
        .await
        .unwrap();
    assert!(!h.a2a.caller_reaches("a2a:device:d-1", "main"));
    assert!(h.a2a.caller_reaches("a2a:device:d-2", "main"));
}

#[tokio::test]
async fn a_callers_queue_and_its_new_contexts_are_capped() {
    let h = harness().await;
    h.working("q-0").await;
    for n in 1..=20 {
        h.new_task(&format!("q-{n}")).await;
    }
    let over = h
        .rpc(
            "SendMessage",
            json!({"message": message("q-21", "x"), "configuration": {"returnImmediately": true}}),
        )
        .await;
    assert_eq!(over["error"]["code"], -32004, "{over}");
    assert_eq!(over["error"]["data"][1]["metadata"]["code"], "droppen.quota_exceeded");
    let starts = calls_of(&h.fake, |call| matches!(call, Call::Start(_)).then_some(()));
    assert_eq!(starts.len(), 1, "nothing over the cap opens a session");

    let fresh = harness().await;
    let who = fresh
        .a2a
        .authorize(Some(&format!("Bearer {}", fresh.token)), Some(LOOPBACK.into()), "main")
        .unwrap();
    for n in 0..50 {
        let (task, _) = fresh.new_task(&format!("c-{n}")).await;
        fresh.a2a.cancel_task(&who, &task).await.unwrap();
    }
    let over = fresh.rpc("SendMessage", json!({"message": message("c-50", "x")})).await;
    assert_eq!(
        over["error"]["data"][1]["metadata"]["code"], "droppen.quota_exceeded",
        "50 new contexts a day: {over}"
    );
}

#[tokio::test]
async fn a_body_over_ten_mib_is_over_quota() {
    let h = harness().await;
    let text = "x".repeat(crate::engine::MAX_BODY);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "SendMessage", "params": {"message": message("big", &text)}});
    let answer = json_of(call(&h.router, request(Method::POST, "/agents/main/jsonrpc", Some(&h.token), Some(body))).await).await;
    assert_eq!(answer["error"]["data"][1]["metadata"]["code"], "droppen.quota_exceeded");
    assert!(
        h.fake.calls().iter().all(|call| matches!(call, Call::CheckEntry(_))),
        "nothing reached the backend"
    );
}

/* ── push configurations ─────────────────────────────────────────────── */

#[tokio::test]
async fn push_configurations_are_kept_listed_and_deleted_and_held_to_the_entrys_origins() {
    let h = harness_with(json!({"pushOrigins": ["https://hooks.example.com"]})).await;
    let (task, _) = h.new_task("m-1").await;
    let config = json!({"id": "p-1", "taskId": task, "url": "https://hooks.example.com/x", "token": "t",
        "authentication": {"scheme": "Bearer", "credentials": "c"}});
    let created = h.rpc("CreateTaskPushNotificationConfig", config.clone()).await["result"].clone();
    assert_eq!(created, config);
    let (_, by_rest) = h.rest(Method::GET, &format!("tasks/{task}/pushNotificationConfigs/p-1"), None).await;
    assert_eq!(by_rest, config);
    let listed = h.rpc("ListTaskPushNotificationConfigs", json!({"taskId": task})).await["result"].clone();
    assert_eq!(listed, json!({"configs": [config], "nextPageToken": ""}));

    for url in [
        "http://hooks.example.com/x",
        "https://evil.example.com/x",
        "https://hooks.example.com.evil.net/",
    ] {
        let refused = h.rpc("CreateTaskPushNotificationConfig", json!({"taskId": task, "url": url})).await;
        assert_eq!(refused["error"]["code"], -32602, "{url}");
    }
    for n in 2..=3 {
        h.rpc(
            "CreateTaskPushNotificationConfig",
            json!({"taskId": task, "id": format!("p-{n}"), "url": "https://hooks.example.com/y"}),
        )
        .await;
    }
    let over = h
        .rpc(
            "CreateTaskPushNotificationConfig",
            json!({"taskId": task, "id": "p-4", "url": "https://hooks.example.com/z"}),
        )
        .await;
    assert_eq!(over["error"]["data"][1]["metadata"]["code"], "droppen.quota_exceeded", "three a task");

    let (status, deleted) = h.rest(Method::DELETE, &format!("tasks/{task}/pushNotificationConfigs/p-1"), None).await;
    assert_eq!((status, deleted), (StatusCode::OK, json!({})));
    assert!(
        h.rpc("DeleteTaskPushNotificationConfig", json!({"taskId": task, "id": "p-1"})).await["result"].is_object(),
        "deleting twice is no error"
    );
    assert_eq!(
        h.rpc("GetTaskPushNotificationConfig", json!({"taskId": task, "id": "p-1"})).await["error"]["code"],
        -32001
    );
}

#[tokio::test]
async fn the_push_seam_hears_every_event_in_order() {
    struct Sink(std::sync::Mutex<Vec<(i64, crate::EventKind)>>);
    impl crate::TaskEventSink for Sink {
        fn task_event(&self, event: &crate::TaskEvent) {
            self.0.lock().unwrap().push((event.seq, event.kind));
        }
    }
    let h = harness().await;
    let sink = Arc::new(Sink(std::sync::Mutex::new(Vec::new())));
    h.a2a.set_event_sink(sink.clone());
    let (task, _) = h.working("m-1").await;
    h.fake.progress(&task, "busy");
    h.fake.complete(&task, "done");
    h.wait_state(&task, TaskState::Completed).await;
    let heard = sink.0.lock().unwrap().clone();
    use crate::EventKind::{Artifact, Progress, Status};
    assert_eq!(
        heard,
        [(1, Status), (2, Status), (3, Progress), (4, Artifact), (5, Artifact), (6, Status)]
    );
    assert_eq!(h.a2a.store().events_after(&task, 0).len(), 6, "and every one is kept for a retry");
}

/* ── failures the backend reports ────────────────────────────────────── */

#[tokio::test]
async fn a_start_the_entry_cannot_run_is_rejected_and_a_lost_session_failed() {
    let h = harness_with(json!({"maxConcurrent": 4})).await;
    h.fake.next_start(Err(BackendError::Rejected {
        code: Some(crate::backend::ErrorCode::IsolationFailed),
        message: "not a git repository".into(),
    }));
    let (rejected, _) = h.new_task("m-1").await;
    h.wait_state(&rejected, TaskState::Rejected).await;
    let row = h.a2a.store().task(&rejected).unwrap();
    assert_eq!(
        row.status_message.unwrap().metadata.unwrap()["errorInfo"]["metadata"]["code"],
        "droppen.isolation_failed"
    );

    let (gone, context) = h.working("m-2").await;
    h.fake.report(crate::backend::BackendEvent::SessionGone {
        context_id: context,
        message: "the session was deleted".into(),
    });
    h.wait_state(&gone, TaskState::Failed).await;
}

/* ── the TCK's entry ─────────────────────────────────────────────────── */

#[tokio::test]
async fn the_tck_entry_plays_its_scenarios() {
    let h = harness().await;
    entry(&h.a2a, json!({"id": "tck", "name": "TCK", "source": "tck", "enabled": true})).await;
    let secret = token(&h.a2a, "tck", &["tck"]);
    let router = crate::protocol::router(h.a2a.clone());
    let rpc = |method: &str, params: Value| {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let request = request(Method::POST, "/agents/tck/jsonrpc/", Some(&secret), Some(body));
        let router = router.clone();
        async move { json_of(call(&router, request).await).await }
    };
    let done = rpc("SendMessage", json!({"message": message("tck-complete-task-1", "x")})).await;
    assert_eq!(done["result"]["task"]["status"]["state"], "TASK_STATE_COMPLETED", "{done}");
    let direct = rpc("SendMessage", json!({"message": message("tck-message-response-1", "x")})).await;
    assert_eq!(direct["result"]["message"]["parts"][0]["text"], "Direct message response");
    let again = rpc("SendMessage", json!({"message": message("tck-complete-task-1", "x")})).await;
    assert_ne!(
        again["result"]["task"]["id"], done["result"]["task"]["id"],
        "the TCK reuses message ids; its entry makes a task each time"
    );

    let waiting = rpc("SendMessage", json!({"message": message("tck-input-required-1", "x")})).await;
    let task = waiting["result"]["task"]["id"].as_str().unwrap().to_string();
    assert_eq!(waiting["result"]["task"]["status"]["state"], "TASK_STATE_INPUT_REQUIRED");
    let follow = json!({"messageId": "tck-complete-task-1", "taskId": task, "role": "ROLE_USER", "parts": [{"text": "x"}]});
    let finished = rpc("SendMessage", json!({"message": follow})).await;
    assert_eq!(finished["result"]["task"]["status"]["state"], "TASK_STATE_COMPLETED", "{finished}");
    assert_eq!(finished["result"]["task"]["history"].as_array().unwrap().len(), 2);

    let params = json!({"message": message("tck-stream-artifact-chunked-1", "x")});
    let body = json!({"jsonrpc": "2.0", "id": 2, "method": "SendStreamingMessage", "params": params});
    let items = Events::of(call(&router, request(Method::POST, "/agents/tck/jsonrpc", Some(&secret), Some(body))).await)
        .results()
        .await;
    let kinds: Vec<&str> = items.iter().map(kind).collect();
    assert_eq!(kinds, ["task", "statusUpdate", "artifactUpdate", "artifactUpdate", "statusUpdate"]);
    assert_eq!(items[3]["artifactUpdate"]["append"], true);

    let long = rpc(
        "SendMessage",
        json!({"message": message("test-resubscribe-message-id-1", "x"), "configuration": {"returnImmediately": true}}),
    )
    .await;
    let task = long["result"]["task"]["id"].as_str().unwrap().to_string();
    let canceled = rpc("CancelTask", json!({"id": task})).await;
    assert_eq!(canceled["result"]["status"]["state"], "TASK_STATE_CANCELED");
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(h.state(&task).await, TaskState::Canceled, "a canceled scenario plays no further");
}

#[tokio::test]
async fn a_tck_entry_needs_the_tck_switch() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::open(dir.path().join("state.db")));
    let (events, inbox) = event_channel();
    let a2a = A2a::new(
        Store::new(db),
        FakeBackend::new(events),
        inbox,
        Settings { tck: false, ..settings() },
        CancellationToken::new(),
    );
    let refused = a2a
        .create_entry(serde_json::from_value(json!({"id": "tck", "name": "TCK", "source": "tck"})).unwrap())
        .await
        .unwrap_err();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
}

/* ── what the backend may say back ───────────────────────────────────── */

#[tokio::test]
async fn a_turn_stopped_in_droppen_is_canceled() {
    let h = harness().await;
    let (task, _) = h.working("m-1").await;
    h.fake.stopped_in_droppen(&task);
    h.wait_state(&task, TaskState::Canceled).await;
    let got = h.rpc("GetTask", json!({"id": task})).await;
    assert_eq!(got["result"]["status"]["message"]["parts"][0]["text"], "stopped in Droppen");
}

#[tokio::test]
async fn an_answer_the_backend_will_not_take_leaves_the_task_waiting() {
    let h = harness().await;
    let task = h.asking("m-1").await;
    h.fake
        .next_answer(Err(BackendError::Invalid("\"blue\" is not an option of \"Which branch?\"".into())));
    let answer = json!({"messageId": "a-1", "taskId": task, "role": "ROLE_USER", "parts": [{"data": {"answers": {"Which branch?": "blue"}}}]});
    let refused = h.rpc("SendMessage", json!({"message": answer.clone()})).await;
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap().contains("blue"));
    assert_eq!(h.state(&task).await, TaskState::InputRequired, "the task still waits; it never fails");

    let fixed = json!({"messageId": "a-1", "taskId": task, "role": "ROLE_USER", "parts": [{"data": {"answers": {"Which branch?": "main"}}}]});
    let taken = h
        .rpc("SendMessage", json!({"message": fixed, "configuration": {"returnImmediately": true}}))
        .await;
    assert_eq!(
        taken["result"]["task"]["status"]["state"], "TASK_STATE_WORKING",
        "the refused message id may be sent again: {taken}"
    );
}

#[tokio::test]
async fn an_entry_runs_under_the_default_account_unless_it_names_another() {
    let h = harness_with(json!({"maxConcurrent": 4})).await;
    let (task, _) = h.new_task("m-1").await;
    let Call::Start(start) = h.started(&task).await else { unreachable!() };
    assert_eq!(start.run.account, crate::backend::RunAccount::Default);

    for (account, expected) in [
        (json!({"kind": "local"}), crate::backend::RunAccount::Local),
        (json!({"kind": "id", "id": "acc-1"}), crate::backend::RunAccount::Id("acc-1".into())),
        (json!({"kind": "default"}), crate::backend::RunAccount::Default),
    ] {
        let entry = h
            .a2a
            .update_entry("main", serde_json::from_value(json!({"account": account})).unwrap())
            .await
            .unwrap();
        assert_eq!(entry.account, expected);
        assert_eq!(serde_json::to_value(&entry).unwrap()["account"], account, "written as it was given");
    }
    let refused = h
        .a2a
        .update_entry(
            "main",
            serde_json::from_value(json!({"account": {"kind": "id", "id": "--flag"}})).unwrap(),
        )
        .await;
    assert!(refused.is_err(), "an account id is checked like a flag");
}

#[tokio::test]
async fn a_new_token_under_a_revoked_ones_name_is_another_caller() {
    let h = harness().await;
    let (task, _) = h.new_task("m-1").await;
    let id = crate::token::parse(&h.token).unwrap().0.to_string();
    h.a2a.revoke_token(&id).await.unwrap();
    let again = token(&h.a2a, "dont-do", &["main"]);
    let twin = token(&h.a2a, "dont-do", &["main"]);
    assert_ne!(
        crate::token::parse(&again).unwrap().0,
        crate::token::parse(&twin).unwrap().0,
        "two live tokens may share a name"
    );
    assert_eq!(
        h.rpc_as(&again, "GetTask", json!({"id": task})).await["error"]["code"],
        -32001,
        "the old caller's task is not the new one's"
    );
    assert_eq!(h.rpc_as(&again, "ListTasks", json!({})).await["result"]["totalSize"], 0);
    let mine = h
        .rpc_as(
            &again,
            "SendMessage",
            json!({"message": message("m-1", "x"), "configuration": {"returnImmediately": true}}),
        )
        .await;
    assert_ne!(
        mine["result"]["task"]["id"],
        json!(task),
        "the same message id from another caller is a new task"
    );
    assert_eq!(
        h.rpc_as(&twin, "ListTasks", json!({})).await["result"]["totalSize"],
        0,
        "nor does a twin see its sibling's"
    );
}

#[tokio::test]
async fn only_the_tck_entry_of_a_tck_instance_pushes_to_loopback_http() {
    let h = harness().await;
    let refused = h
        .a2a
        .update_entry("main", serde_json::from_value(json!({"pushOrigins": ["http://127.0.0.1:9000"]})).unwrap())
        .await;
    assert!(refused.is_err(), "§6 holds for every other entry");
    entry(
        &h.a2a,
        json!({"id": "tck", "name": "TCK", "source": "tck", "enabled": true, "pushOrigins": ["http://localhost:9000", "https://example.com"]}),
    )
    .await;
    let secret = token(&h.a2a, "tck", &["tck"]);
    let router = crate::protocol::router(h.a2a.clone());
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "SendMessage", "params": {"message": message("tck-input-required-1", "x")}});
    let sent = json_of(call(&router, request(Method::POST, "/agents/tck/jsonrpc", Some(&secret), Some(body))).await).await;
    let task = sent["result"]["task"]["id"].as_str().unwrap().to_string();
    let create = |url: &str| json!({"jsonrpc": "2.0", "id": 2, "method": "CreateTaskPushNotificationConfig", "params": {"taskId": task, "url": url}});
    let ok = json_of(
        call(
            &router,
            request(
                Method::POST,
                "/agents/tck/jsonrpc",
                Some(&secret),
                Some(create("http://localhost:9000/hook")),
            ),
        )
        .await,
    )
    .await;
    assert!(ok["result"]["id"].is_string(), "{ok}");
    let lan = json_of(
        call(
            &router,
            request(
                Method::POST,
                "/agents/tck/jsonrpc",
                Some(&secret),
                Some(create("http://192.168.1.9:9000/hook")),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(lan["error"]["code"], -32602);
    let switched = h
        .a2a
        .update_entry("tck", serde_json::from_value(json!({"source": "claude", "cwd": "/w"})).unwrap())
        .await;
    assert!(switched.is_err(), "the loopback origins do not come along to another kind of entry");
}
