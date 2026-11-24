//! `/a2a/*`: the Agent Card and the two bindings (`docs/a2a.md` §2), as a
//! router of its own that the server nests at `/a2a`.
//!
//! Every request proves an A2A token first, whatever it asks for: no token
//! is `401`, the same on every path; a token from anywhere but this device's
//! loopback is `403`; an entry the token does not reach is `404`, exactly as
//! one that does not exist. A local login counts for nothing here (the login
//! guard lets `/a2a/*` through to this check, and this check reads only an
//! A2A token). Each request then writes one audit line — when, who, which
//! entry, which method, which task — and never what it said.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::stream::{self, Stream, StreamExt as _};
use gotcha::axum::extract::{ConnectInfo, Path, Query, Request, State};
use gotcha::axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use gotcha::axum::response::sse::{Event, KeepAlive, Sse};
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::axum::routing::{any, get, post};
use gotcha::axum::Router;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::backend::ErrorCode;
use crate::engine::{A2a, Caller, Denied, Follow, Sent, MAX_BODY};
use crate::error::{self, a2a_json, json_as, A2aError, ErrorKind};
use crate::wire::{
    AgentCapabilities, AgentCard, AgentInterface, AgentSkill, GetTaskRequest, HttpAuthSecurityScheme, ListPushConfigsRequest, ListTasksRequest,
    PushConfigRequest, SecurityRequirement, SecurityScheme, SendMessageRequest, SendMessageResponse, StreamResponse, TaskIdRequest,
    TaskPushNotificationConfig, TaskState,
};

/// The protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "1.0";

/// The router to nest at `/a2a`.
pub fn router(a2a: Arc<A2a>) -> Router {
    Router::new()
        .route("/agents/{entry}/.well-known/agent-card.json", get(card))
        .route("/agents/{entry}/jsonrpc", post(jsonrpc))
        .route("/agents/{entry}/jsonrpc/", post(jsonrpc))
        .route("/agents/{entry}/rest/{*path}", any(rest))
        .fallback(|| async { error::no_such_agent() })
        .with_state(a2a)
}

/* ── who is calling ──────────────────────────────────────────────────── */

/// The request's caller, or the answer that refuses it.
fn admit(a2a: &A2a, request: &Request, entry: &str, method: &str) -> Result<Caller, Response> {
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0.ip());
    let authorization = request.headers().get(header::AUTHORIZATION).and_then(|value| value.to_str().ok());
    a2a.authorize(authorization, peer, entry).map_err(|denied| {
        audit(None, entry, method, None, denied_outcome(denied));
        match denied {
            Denied::Unauthenticated => error::unauthenticated(),
            Denied::NotLoopback => {
                let body = json!({"error": {
                    "code": 403,
                    "status": "PERMISSION_DENIED",
                    "message": "A2A tokens are taken on this device's loopback only",
                    "details": [],
                }});
                json_as(StatusCode::FORBIDDEN, &body, "application/json")
            }
            Denied::NotFound => error::no_such_agent(),
        }
    })
}

fn denied_outcome(denied: Denied) -> &'static str {
    match denied {
        Denied::Unauthenticated => "unauthenticated",
        Denied::NotLoopback => "not_loopback",
        Denied::NotFound => "not_found",
    }
}

/// One audit line (§6): when (the log's own time), who, which entry, which
/// method, which task, and how it went. Nothing the request said, and no
/// token.
fn audit(caller: Option<&Caller>, entry: &str, method: &str, task: Option<&str>, outcome: &str) {
    tracing::info!(
        target: "droppen_a2a::audit",
        caller = caller.map_or("-", |caller| caller.caller.as_str()),
        entry,
        method,
        task = task.unwrap_or("-"),
        outcome,
        "a2a request"
    );
}

/// Whether a method call without an `A2A-Version` (or with an empty one) is
/// refused. The spec reads it as 0.3 (v1.0 §3.6.2), which this server does
/// not speak, and every client known here (dont-do's `@a2a-js/sdk` 1.3.0, the
/// TCK) sends `1.0` on every call. `false` would read it as 1.0 instead. The
/// Agent Card's `GET` is never checked.
const UNVERSIONED_IS_0_3: bool = true;

/// Whether the request speaks A2A 1.0 (`A2A-Version`, in a header or the
/// query): `1.0`, a patch number ignored. An absent or empty one is as
/// [`UNVERSIONED_IS_0_3`] says.
fn check_version(headers: &HeaderMap, query: Option<&str>) -> Result<(), A2aError> {
    let from_query = query.and_then(|query| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find_map(|(key, value)| key.eq_ignore_ascii_case("A2A-Version").then(|| value.to_string()))
    });
    let version = headers
        .get("a2a-version")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or(from_query)
        .unwrap_or_default();
    let version = version.trim();
    if version.is_empty() && !UNVERSIONED_IS_0_3 {
        return Ok(());
    }
    let mut parts = version.split('.');
    let speaks = matches!((parts.next(), parts.next()), (Some("1"), Some("0")));
    if speaks {
        return Ok(());
    }
    let asked = if version.is_empty() {
        "0.3 (no A2A-Version)".to_string()
    } else {
        version.to_string()
    };
    Err(A2aError::new(
        ErrorKind::VersionNotSupported,
        format!("A2A version {asked} is not supported: this agent speaks {PROTOCOL_VERSION}"),
    ))
}

/// The body, at most [`MAX_BODY`] of it.
async fn body_of(request: Request) -> Result<gotcha::axum::body::Bytes, A2aError> {
    gotcha::axum::body::to_bytes(request.into_body(), MAX_BODY)
        .await
        .map_err(|_| A2aError::unsupported(format!("a request body is at most {} MiB", MAX_BODY / (1024 * 1024))).with_code(ErrorCode::QuotaExceeded))
}

/* ── the card ────────────────────────────────────────────────────────── */

/// The base the interfaces' URLs start with: the configured public base, or
/// the request's own `Host` over `http`.
fn base_url(a2a: &A2a, headers: &HeaderMap) -> String {
    if let Some(base) = &a2a.settings().public_base {
        return base.clone();
    }
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b)))
        .unwrap_or("127.0.0.1");
    format!("http://{host}")
}

/// The Agent Card of `caller`'s entry.
pub fn agent_card(a2a: &A2a, caller: &Caller, headers: &HeaderMap) -> AgentCard {
    let base = format!("{}/a2a/agents/{}", base_url(a2a, headers), caller.entry.id);
    let interface = |binding: &str, path: &str| AgentInterface {
        url: format!("{base}/{path}"),
        protocol_binding: binding.into(),
        tenant: String::new(),
        protocol_version: PROTOCOL_VERSION.into(),
    };
    let mut schemes = serde_json::Map::new();
    schemes.insert(
        "bearer".into(),
        serde_json::to_value(SecurityScheme::HttpAuthSecurityScheme(HttpAuthSecurityScheme { scheme: "Bearer".into() })).unwrap_or(Value::Null),
    );
    let mut required = serde_json::Map::new();
    required.insert("bearer".into(), json!({"list": []}));
    let description = if caller.entry.description.is_empty() {
        "A coding session on this device, run by Droppen.".to_string()
    } else {
        caller.entry.description.clone()
    };
    AgentCard {
        name: caller.entry.name.clone(),
        description,
        supported_interfaces: vec![interface("JSONRPC", "jsonrpc"), interface("HTTP+JSON", "rest")],
        version: a2a.settings().version.clone(),
        capabilities: AgentCapabilities {
            streaming: true,
            push_notifications: true,
            extended_agent_card: false,
        },
        security_schemes: schemes,
        security_requirements: vec![SecurityRequirement { schemes: required }],
        default_input_modes: ["text/plain", "application/json", "image/png", "image/jpeg"].map(String::from).to_vec(),
        default_output_modes: ["text/plain", "text/markdown", "application/json"].map(String::from).to_vec(),
        skills: vec![AgentSkill {
            id: "coding_session".into(),
            name: "Coding session".into(),
            description: "Runs the task as a coding session in this agent's configured project, asks when it needs an answer, and reports a summary and what changed.".into(),
            tags: vec!["coding".into()],
        }],
    }
}

async fn card(State(a2a): State<Arc<A2a>>, Path(entry): Path<String>, request: Request) -> Response {
    let who = match admit(&a2a, &request, &entry, "GetAgentCard") {
        Ok(who) => who,
        Err(refused) => return refused,
    };
    audit(Some(&who), &entry, "GetAgentCard", None, "ok");
    let body = serde_json::to_string(&agent_card(&a2a, &who, request.headers())).unwrap_or_default();
    let etag = format!("\"{}\"", &droppen_base::text::sha256_hex(&body)[..32]);
    let matched = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|tags| tags.split(',').any(|tag| tag.trim() == etag));
    let mut response = if matched {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut response = (StatusCode::OK, body).into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    response
}

/* ── streams ─────────────────────────────────────────────────────────── */

/// How each stream item is written.
#[derive(Clone)]
enum Framing {
    /// HTTP+JSON: the `StreamResponse` itself.
    Rest,
    /// JSON-RPC: a response to request `id` whose result is the item.
    JsonRpc(Value),
}

impl Framing {
    fn frame(&self, item: &StreamResponse) -> String {
        match self {
            Framing::Rest => serde_json::to_string(item).unwrap_or_default(),
            Framing::JsonRpc(id) => json!({"jsonrpc": "2.0", "id": id, "result": item}).to_string(),
        }
    }
}

/// An SSE answer: `first`, then whatever `follow` yields.
fn sse(first: StreamResponse, follow: Option<Follow>, framing: Framing) -> Response {
    let items = stream::once(async move { first }).chain(stream::unfold(follow, |follow| async move {
        let mut follow = follow?;
        let item = follow.next().await?;
        Some((item, Some(follow)))
    }));
    let events = items.map(move |item| Ok::<_, Infallible>(Event::default().data(framing.frame(&item))));
    sse_response(events)
}

fn sse_response(events: impl Stream<Item = Result<Event, Infallible>> + Send + 'static) -> Response {
    Sse::new(events).keep_alive(KeepAlive::default()).into_response()
}

/// A sent message as a stream: the message alone, or the task's snapshot
/// and its events.
fn stream_of(sent: Sent, framing: Framing) -> (Response, Option<String>) {
    match sent {
        Sent::Message(message) => (sse(StreamResponse::Message(message), None, framing), None),
        Sent::Task { follow, .. } => {
            let task = follow.task_id().to_string();
            let first = StreamResponse::Task(follow.snapshot());
            (sse(first, Some(*follow), framing), Some(task))
        }
    }
}

/// A sent message as a unary answer: at once, or once the task settles.
async fn answer_of(sent: Sent) -> (SendMessageResponse, Option<String>) {
    match sent {
        Sent::Message(message) => (SendMessageResponse::Message(message), None),
        Sent::Task { follow, blocking } => {
            let task_id = follow.task_id().to_string();
            let task = if blocking && !follow.snapshot().status.state.is_settled() {
                follow.settle().await
            } else {
                follow.snapshot()
            };
            (SendMessageResponse::Task(task), Some(task_id))
        }
    }
}

/* ── JSON-RPC ────────────────────────────────────────────────────────── */

fn rpc_result(id: &Value, result: Value) -> Response {
    json_as(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": id, "result": result}), "application/json")
}

fn rpc_error(id: &Value, error: &A2aError) -> Response {
    json_as(
        StatusCode::OK,
        &json!({"jsonrpc": "2.0", "id": id, "error": error.jsonrpc()}),
        "application/json",
    )
}

fn params<T: DeserializeOwned>(params: &Value) -> Result<T, A2aError> {
    serde_json::from_value(params.clone()).map_err(|error| A2aError::invalid(format!("invalid params: {error}")))
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

async fn jsonrpc(State(a2a): State<Arc<A2a>>, Path(entry): Path<String>, request: Request) -> Response {
    let who = match admit(&a2a, &request, &entry, "jsonrpc") {
        Ok(who) => who,
        Err(refused) => return refused,
    };
    let headers = request.headers().clone();
    let query = request.uri().query().map(str::to_string);
    let null = Value::Null;
    let body = match body_of(request).await {
        Ok(body) => body,
        Err(error) => {
            audit(Some(&who), &entry, "jsonrpc", None, "too_large");
            return rpc_error(&null, &error);
        }
    };
    let call: Value = match serde_json::from_slice(&body) {
        Ok(call) => call,
        Err(_) => {
            audit(Some(&who), &entry, "jsonrpc", None, "parse_error");
            return rpc_error(&null, &A2aError::new(ErrorKind::Parse, "Invalid JSON payload"));
        }
    };
    let Some(object) = call.as_object() else {
        audit(Some(&who), &entry, "jsonrpc", None, "invalid_request");
        let message = if call.is_array() {
            "batch requests are not supported"
        } else {
            "Request payload validation error"
        };
        return rpc_error(&null, &A2aError::new(ErrorKind::InvalidRequest, message));
    };
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    let id = if matches!(id, Value::String(_) | Value::Number(_) | Value::Null) {
        id
    } else {
        Value::Null
    };
    let method = object.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || method.is_empty() {
        audit(Some(&who), &entry, "jsonrpc", None, "invalid_request");
        return rpc_error(
            &id,
            &A2aError::new(
                ErrorKind::InvalidRequest,
                "Request payload validation error: a JSON-RPC 2.0 request with a method",
            ),
        );
    }
    let params_value = object.get("params").cloned().unwrap_or_else(|| json!({}));
    if !params_value.is_object() {
        audit(Some(&who), &entry, &method, None, "invalid_params");
        return rpc_error(&id, &A2aError::invalid("params is an object"));
    }
    if let Err(error) = check_version(&headers, query.as_deref()) {
        audit(Some(&who), &entry, &method, None, "version_not_supported");
        return rpc_error(&id, &error);
    }
    let (outcome, task) = call_method(&a2a, &who, &method, &params_value, &id).await;
    let (response, task, result) = match outcome {
        Ok(response) => (response, task, "ok"),
        Err(error) => (rpc_error(&id, &error), task, error_outcome(&error)),
    };
    audit(Some(&who), &entry, &method, task.as_deref(), result);
    response
}

fn error_outcome(error: &A2aError) -> &'static str {
    match error.kind {
        ErrorKind::Parse => "parse_error",
        ErrorKind::InvalidRequest => "invalid_request",
        ErrorKind::MethodNotFound => "method_not_found",
        ErrorKind::InvalidParams => "invalid_params",
        ErrorKind::Internal => "internal_error",
        ErrorKind::TaskNotFound => "task_not_found",
        ErrorKind::TaskNotCancelable => "task_not_cancelable",
        ErrorKind::UnsupportedOperation => "unsupported_operation",
        ErrorKind::ContentTypeNotSupported => "content_type_not_supported",
        ErrorKind::VersionNotSupported => "version_not_supported",
    }
}

/// One JSON-RPC method: its answer, and the task it touched.
async fn call_method(a2a: &Arc<A2a>, who: &Caller, method: &str, value: &Value, id: &Value) -> (Result<Response, A2aError>, Option<String>) {
    let task_of = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    match method {
        "SendMessage" | "SendStreamingMessage" => {
            let request: SendMessageRequest = match params(value) {
                Ok(request) => request,
                Err(error) => return (Err(error), None),
            };
            let streaming = method == "SendStreamingMessage";
            match a2a.send(who, request, streaming).await {
                Ok(sent) if streaming => {
                    let (response, task) = stream_of(sent, Framing::JsonRpc(id.clone()));
                    (Ok(response), task)
                }
                Ok(sent) => {
                    let (answer, task) = answer_of(sent).await;
                    (Ok(rpc_result(id, to_value(answer))), task)
                }
                Err(error) => (Err(error), None),
            }
        }
        "GetTask" => {
            let request: GetTaskRequest = match params(value) {
                Ok(request) => request,
                Err(error) => return (Err(error), None),
            };
            let task = Some(request.id.clone());
            (
                a2a.get_task(who, &request.id, request.history_length)
                    .map(|t| rpc_result(id, to_value(t))),
                task,
            )
        }
        "ListTasks" => {
            let request: ListTasksRequest = match params(value) {
                Ok(request) => request,
                Err(error) => return (Err(error), None),
            };
            (a2a.list_tasks(who, &request).map(|list| rpc_result(id, to_value(list))), None)
        }
        "CancelTask" => {
            let request: TaskIdRequest = match params(value) {
                Ok(request) => request,
                Err(error) => return (Err(error), None),
            };
            let task = Some(request.id.clone());
            (a2a.cancel_task(who, &request.id).await.map(|t| rpc_result(id, to_value(t))), task)
        }
        "SubscribeToTask" => {
            let request: TaskIdRequest = match params(value) {
                Ok(request) => request,
                Err(error) => return (Err(error), None),
            };
            let task = Some(request.id.clone());
            let follow = a2a.subscribe(who, &request.id);
            (
                follow.map(|follow| {
                    let first = StreamResponse::Task(follow.snapshot());
                    sse(first, Some(follow), Framing::JsonRpc(id.clone()))
                }),
                task,
            )
        }
        "CreateTaskPushNotificationConfig" => {
            let config: TaskPushNotificationConfig = match params(value) {
                Ok(config) => config,
                Err(error) => return (Err(error), None),
            };
            let task_id = config.task_id.clone();
            (a2a.create_push(who, &task_id, config).map(|c| rpc_result(id, to_value(c))), Some(task_id))
        }
        "GetTaskPushNotificationConfig" => {
            let request: PushConfigRequest = match params(value) {
                Ok(request) => request,
                Err(error) => return (Err(error), None),
            };
            (
                a2a.get_push(who, &request.task_id, &request.id).map(|c| rpc_result(id, to_value(c))),
                Some(request.task_id),
            )
        }
        "ListTaskPushNotificationConfigs" | "ListTaskPushNotificationConfig" => {
            let request: ListPushConfigsRequest = match params(value) {
                Ok(request) => request,
                Err(error) => return (Err(error), None),
            };
            (
                a2a.list_push(who, &request.task_id).map(|list| rpc_result(id, to_value(list))),
                Some(request.task_id),
            )
        }
        "DeleteTaskPushNotificationConfig" => {
            let request: PushConfigRequest = match params(value) {
                Ok(request) => request,
                Err(error) => return (Err(error), None),
            };
            (
                a2a.delete_push(who, &request.task_id, &request.id).map(|()| rpc_result(id, json!({}))),
                Some(request.task_id),
            )
        }
        "GetExtendedAgentCard" => (Err(A2aError::unsupported("this agent has no extended card")), None),
        _ => (Err(A2aError::new(ErrorKind::MethodNotFound, "Method not found")), task_of(value, "id")),
    }
}

/* ── HTTP+JSON ───────────────────────────────────────────────────────── */

/// The REST operation a method and a path (below `/rest/`) name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Operation {
    Send,
    Stream,
    List,
    Get(String),
    Cancel(String),
    Subscribe(String),
    CreatePush(String),
    ListPush(String),
    GetPush(String, String),
    DeletePush(String, String),
    ExtendedCard,
}

impl Operation {
    fn name(&self) -> &'static str {
        match self {
            Operation::Send => "SendMessage",
            Operation::Stream => "SendStreamingMessage",
            Operation::List => "ListTasks",
            Operation::Get(_) => "GetTask",
            Operation::Cancel(_) => "CancelTask",
            Operation::Subscribe(_) => "SubscribeToTask",
            Operation::CreatePush(_) => "CreateTaskPushNotificationConfig",
            Operation::ListPush(_) => "ListTaskPushNotificationConfigs",
            Operation::GetPush(..) => "GetTaskPushNotificationConfig",
            Operation::DeletePush(..) => "DeleteTaskPushNotificationConfig",
            Operation::ExtendedCard => "GetExtendedAgentCard",
        }
    }

    fn task(&self) -> Option<&str> {
        match self {
            Operation::Get(task)
            | Operation::Cancel(task)
            | Operation::Subscribe(task)
            | Operation::CreatePush(task)
            | Operation::ListPush(task)
            | Operation::GetPush(task, _)
            | Operation::DeletePush(task, _) => Some(task),
            _ => None,
        }
    }
}

/// `%XX` escapes decoded; `None` for a segment that does not decode to UTF-8.
fn decoded(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The operation `method path` names, if any.
fn operation(method: &Method, path: &str) -> Option<Operation> {
    let segments: Vec<String> = path.trim_end_matches('/').split('/').map(decoded).collect::<Option<_>>()?;
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let id_ok = |id: &str| !id.is_empty() && id.len() <= 256;
    let op = match (method.as_str(), segments.as_slice()) {
        ("POST", ["message:send"]) => Operation::Send,
        ("POST", ["message:stream"]) => Operation::Stream,
        ("GET", ["tasks"]) => Operation::List,
        ("GET", ["extendedAgentCard"]) => Operation::ExtendedCard,
        ("POST", ["tasks", last]) if last.ends_with(":cancel") => Operation::Cancel(last.trim_end_matches(":cancel").to_string()),
        ("GET" | "POST", ["tasks", last]) if last.ends_with(":subscribe") => Operation::Subscribe(last.trim_end_matches(":subscribe").to_string()),
        ("GET", ["tasks", id]) => Operation::Get((*id).to_string()),
        ("POST", ["tasks", id, "pushNotificationConfigs"]) => Operation::CreatePush((*id).to_string()),
        ("GET", ["tasks", id, "pushNotificationConfigs"]) => Operation::ListPush((*id).to_string()),
        ("GET", ["tasks", id, "pushNotificationConfigs", config]) => Operation::GetPush((*id).to_string(), (*config).to_string()),
        ("DELETE", ["tasks", id, "pushNotificationConfigs", config]) => Operation::DeletePush((*id).to_string(), (*config).to_string()),
        _ => return None,
    };
    op.task().is_none_or(id_ok).then_some(op)
}

/// `ListTasks` out of a query string (camelCase, or the proto's names).
fn list_request(query: &std::collections::HashMap<String, String>) -> Result<ListTasksRequest, A2aError> {
    let get = |camel: &str, snake: &str| query.get(camel).or_else(|| query.get(snake)).cloned();
    let number = |camel: &str, snake: &str| -> Result<Option<i32>, A2aError> {
        get(camel, snake)
            .map(|value| value.trim().parse::<i32>().map_err(|_| A2aError::invalid(format!("{camel} is a number"))))
            .transpose()
    };
    let status = match get("status", "status") {
        None => None,
        Some(name) if name.is_empty() => None,
        Some(name) => Some(TaskState::parse(&name).ok_or_else(|| A2aError::invalid("status is a TASK_STATE_* name"))?),
    };
    let include_artifacts = match get("includeArtifacts", "include_artifacts").as_deref() {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => return Err(A2aError::invalid("includeArtifacts is true or false")),
    };
    Ok(ListTasksRequest {
        tenant: String::new(),
        context_id: get("contextId", "context_id").unwrap_or_default(),
        status,
        page_size: number("pageSize", "page_size")?,
        page_token: get("pageToken", "page_token").unwrap_or_default(),
        history_length: number("historyLength", "history_length")?,
        status_timestamp_after: get("statusTimestampAfter", "status_timestamp_after"),
        include_artifacts,
    })
}

fn rest_ok(value: Value) -> Response {
    a2a_json(StatusCode::OK, &value)
}

fn json_body<T: DeserializeOwned + Default>(body: &[u8]) -> Result<T, A2aError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|error| A2aError::invalid(format!("invalid body: {error}")))
}

async fn rest(State(a2a): State<Arc<A2a>>, Path((entry, path)): Path<(String, String)>, request: Request) -> Response {
    let method = request.method().clone();
    let op = operation(&method, &path);
    let name = op.as_ref().map_or("rest", Operation::name);
    let who = match admit(&a2a, &request, &entry, name) {
        Ok(who) => who,
        Err(refused) => return refused,
    };
    let Some(op) = op else {
        audit(Some(&who), &entry, name, None, "method_not_found");
        return A2aError::new(ErrorKind::MethodNotFound, format!("no such operation: {method} {path}")).rest_response();
    };
    let task = op.task().map(str::to_string);
    let (response, outcome) = match rest_call(&a2a, &who, op, request).await {
        Ok((response, sent_task)) => (response, (sent_task.or(task), "ok")),
        Err(error) => (error.rest_response(), (task, error_outcome(&error))),
    };
    audit(Some(&who), &entry, name, outcome.0.as_deref(), outcome.1);
    response
}

async fn rest_call(a2a: &Arc<A2a>, who: &Caller, op: Operation, request: Request) -> Result<(Response, Option<String>), A2aError> {
    check_version(request.headers(), request.uri().query())?;
    let query: std::collections::HashMap<String, String> = Query::try_from_uri(request.uri()).map(|Query(query)| query).unwrap_or_default();
    match op {
        Operation::Send | Operation::Stream => {
            let body = body_of(request).await?;
            if body.iter().all(u8::is_ascii_whitespace) {
                return Err(A2aError::invalid("a SendMessageRequest is required"));
            }
            let send: SendMessageRequest = json_body(&body)?;
            let streaming = op == Operation::Stream;
            let sent = a2a.send(who, send, streaming).await?;
            if streaming {
                let (response, task) = stream_of(sent, Framing::Rest);
                return Ok((response, task));
            }
            let (answer, task) = answer_of(sent).await;
            Ok((rest_ok(to_value(answer)), task))
        }
        Operation::List => Ok((rest_ok(to_value(a2a.list_tasks(who, &list_request(&query)?)?)), None)),
        Operation::Get(id) => {
            let history_length = list_request(&query)?.history_length;
            Ok((rest_ok(to_value(a2a.get_task(who, &id, history_length)?)), None))
        }
        Operation::Cancel(id) => Ok((rest_ok(to_value(a2a.cancel_task(who, &id).await?)), None)),
        Operation::Subscribe(id) => {
            let follow = a2a.subscribe(who, &id)?;
            let first = StreamResponse::Task(follow.snapshot());
            Ok((sse(first, Some(follow), Framing::Rest), None))
        }
        Operation::CreatePush(id) => {
            let body = body_of(request).await?;
            let mut config: TaskPushNotificationConfig = json_body(&body)?;
            if !config.task_id.is_empty() && config.task_id != id {
                return Err(A2aError::invalid("the body's taskId is not the path's"));
            }
            config.task_id = id.clone();
            Ok((rest_ok(to_value(a2a.create_push(who, &id, config)?)), None))
        }
        Operation::ListPush(id) => Ok((rest_ok(to_value(a2a.list_push(who, &id)?)), None)),
        Operation::GetPush(id, config) => Ok((rest_ok(to_value(a2a.get_push(who, &id, &config)?)), None)),
        Operation::DeletePush(id, config) => {
            a2a.delete_push(who, &id, &config)?;
            Ok((rest_ok(json!({})), None))
        }
        Operation::ExtendedCard => Err(A2aError::unsupported("this agent has no extended card")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rest_path_names_its_operation() {
        let op = |method: Method, path: &str| operation(&method, path);
        assert_eq!(op(Method::POST, "message:send"), Some(Operation::Send));
        assert_eq!(op(Method::POST, "tasks/t1:cancel"), Some(Operation::Cancel("t1".into())));
        assert_eq!(
            op(Method::POST, "tasks/t1%3Acancel"),
            Some(Operation::Cancel("t1".into())),
            "an escaped colon is the colon"
        );
        assert_eq!(op(Method::GET, "tasks/t1:subscribe"), Some(Operation::Subscribe("t1".into())));
        assert_eq!(op(Method::POST, "tasks/t1:subscribe"), Some(Operation::Subscribe("t1".into())));
        assert_eq!(
            op(Method::GET, "tasks/t1/pushNotificationConfigs/c1"),
            Some(Operation::GetPush("t1".into(), "c1".into()))
        );
        assert_eq!(op(Method::GET, "tasks"), Some(Operation::List));
        assert_eq!(op(Method::GET, "tasks/"), Some(Operation::List));
        assert_eq!(op(Method::DELETE, "tasks/t1"), None);
        assert_eq!(op(Method::POST, "tasks/:cancel"), None, "no task id");
        assert_eq!(op(Method::GET, "elsewhere"), None);
    }

    #[test]
    fn only_1_0_is_spoken() {
        let with = |version: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("a2a-version", HeaderValue::from_str(version).unwrap());
            check_version(&headers, None)
        };
        assert!(with("1.0").is_ok());
        assert!(with("1.0.1").is_ok(), "a patch number is ignored");
        assert_eq!(with("0.3").unwrap_err().kind, ErrorKind::VersionNotSupported);
        assert_eq!(with("99.0").unwrap_err().kind, ErrorKind::VersionNotSupported);
        assert_eq!(with("").unwrap_err().kind, ErrorKind::VersionNotSupported, "empty is 0.3");
        assert_eq!(check_version(&HeaderMap::new(), None).unwrap_err().kind, ErrorKind::VersionNotSupported);
        assert!(check_version(&HeaderMap::new(), Some("A2A-Version=1.0")).is_ok(), "or in the query");
    }
}
