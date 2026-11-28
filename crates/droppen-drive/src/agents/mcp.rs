//! Pure-Rust stdio JSON-RPC 2.0 MCP server (`droppen mcp-agents`).
//!
//! Spawned as a child by ACP agents, Claude Code, or Codex to expose Droppen's
//! local cross-session agent bus (`list_agents`, `send_message`,
//! `read_session`). Stdout carries nothing but line-delimited JSON-RPC 2.0
//! frames; no logs or diagnostics are ever written there.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use droppen_base::process_env::{AGENTS_TOKEN_ENV, AGENTS_URL_ENV};
use droppen_sessions::Source;

use super::routes::{AGENTS_LIST_PATH, AGENTS_SEND_PATH};
use super::token::MCP_SERVER_NAME;

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";
/// Maximum bytes accepted for a single JSON-RPC line on stdin (1 MiB).
const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum StdinLine {
    Line(String),
    TooLong,
    InvalidUtf8,
}

fn deserialize_present_id<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default, deserialize_with = "deserialize_present_id")]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

impl RpcResponse {
    fn ok(id: Value, result: impl Serialize) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(serde_json::to_value(result).expect("serializable RPC result")),
            error: None,
        }
    }

    fn error(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }

    fn parse_error(message: impl Into<String>) -> Self {
        Self::error(Value::Null, -32700, message)
    }

    fn invalid_request(message: impl Into<String>) -> Self {
        Self::error(Value::Null, -32600, message)
    }

    fn method_not_found(id: Value, method: &str) -> Self {
        Self::error(id, -32601, format!("method not found: {method}"))
    }

    fn invalid_params(id: Value, message: impl Into<String>) -> Self {
        Self::error(id, -32602, message)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    protocol_version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    protocol_version: String,
    capabilities: ToolsCapability,
    server_info: ServerInfo,
}

#[derive(Debug, Serialize)]
struct ToolsCapability {
    tools: serde_json::Map<String, Value>,
}

#[derive(Debug, Serialize)]
struct ServerInfo {
    name: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct ToolListResult {
    tools: Vec<ToolDef>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolDef {
    name: &'static str,
    description: &'static str,
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallResult {
    content: Vec<TextContent>,
    is_error: bool,
}

#[derive(Debug, Serialize)]
struct TextContent {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

impl ToolCallResult {
    fn text(text: String, is_error: bool) -> Self {
        Self {
            content: vec![TextContent { kind: "text", text }],
            is_error,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAgentsArgs {
    cwd: Option<String>,
    source: Option<String>,
    active_only: Option<bool>,
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SendMessageArgs {
    target: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionArgs {
    target: String,
    limit: Option<u64>,
    include_tools: Option<bool>,
}

/// Read one `\n`-terminated line up to `limit` bytes. Oversized lines are
/// drained cleanly through `\n` (or EOF) without further allocation so the next
/// frame starts at a clean boundary.
fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> std::io::Result<Option<StdinLine>> {
    let mut buf = Vec::new();
    let mut too_long = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            if too_long {
                return Ok(Some(StdinLine::TooLong));
            }
            if buf.is_empty() {
                return Ok(None);
            }
            return Ok(Some(match String::from_utf8(buf) {
                Ok(line) => StdinLine::Line(line),
                Err(_) => StdinLine::InvalidUtf8,
            }));
        }
        if let Some(newline_pos) = available.iter().position(|&b| b == b'\n') {
            let take = newline_pos + 1;
            if !too_long {
                if buf.len() + take > limit {
                    too_long = true;
                    buf.clear();
                } else {
                    buf.extend_from_slice(&available[..take]);
                }
            }
            reader.consume(take);
            if too_long {
                return Ok(Some(StdinLine::TooLong));
            }
            return Ok(Some(match String::from_utf8(buf) {
                Ok(line) => StdinLine::Line(line),
                Err(_) => StdinLine::InvalidUtf8,
            }));
        }
        let len = available.len();
        if !too_long {
            if buf.len() + len > limit {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(available);
            }
        }
        reader.consume(len);
    }
}

fn write_response<W: Write>(writer: &mut W, response: &RpcResponse) -> Result<(), String> {
    let encoded = serde_json::to_string(response).map_err(|error| format!("encode failed: {error}"))?;
    writer
        .write_all(encoded.as_bytes())
        .map_err(|error| format!("stdout write failed: {error}"))?;
    writer
        .write_all(b"\n")
        .map_err(|error| format!("stdout write failed: {error}"))?;
    writer
        .flush()
        .map_err(|error| format!("stdout flush failed: {error}"))?;
    Ok(())
}

/// Run the stdio MCP server using `DROPPEN_AGENTS_URL` and `DROPPEN_AGENTS_TOKEN`.
pub async fn run() -> Result<(), String> {
    let base_url = std::env::var(AGENTS_URL_ENV)
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{AGENTS_URL_ENV} is not set"))?;
    let token = std::env::var(AGENTS_TOKEN_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{AGENTS_TOKEN_ENV} is not set"))?;

    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|error| format!("cannot initialize HTTP client: {error}"))?;

    // Read stdin on a dedicated OS thread so blocking reads never stall the
    // tokio runtime while an HTTP call or timeout is in flight.
    let (tx, rx) = mpsc::channel::<std::io::Result<StdinLine>>(16);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        loop {
            match read_bounded_line(&mut lock, MAX_LINE_BYTES) {
                Ok(None) => break,
                Ok(Some(line)) => {
                    if tx.blocking_send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.blocking_send(Err(error));
                    break;
                }
            }
        }
    });

    let stdout = std::io::stdout();
    serve_lines(rx, stdout, &client, &base_url, &token).await
}

async fn serve_lines<W: Write>(
    mut rx: mpsc::Receiver<std::io::Result<StdinLine>>,
    mut writer: W,
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<(), String> {
    while let Some(item) = rx.recv().await {
        let line = match item.map_err(|error| format!("stdin read failed: {error}"))? {
            StdinLine::Line(line) => line,
            StdinLine::TooLong => {
                write_response(
                    &mut writer,
                    &RpcResponse::parse_error(format!("request exceeds {MAX_LINE_BYTES} byte limit")),
                )?;
                continue;
            }
            StdinLine::InvalidUtf8 => {
                write_response(&mut writer, &RpcResponse::parse_error("invalid UTF-8 in request"))?;
                continue;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let raw_value: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(error) => {
                write_response(&mut writer, &RpcResponse::parse_error(format!("parse error: {error}")))?;
                continue;
            }
        };
        if !raw_value.is_object() {
            write_response(&mut writer, &RpcResponse::invalid_request("invalid request"))?;
            continue;
        }
        let req: RpcRequest = match serde_json::from_value(raw_value) {
            Ok(req) => req,
            Err(error) => {
                write_response(
                    &mut writer,
                    &RpcResponse::invalid_request(format!("invalid request: {error}")),
                )?;
                continue;
            }
        };
        let Some(id) = req.id else {
            // Notifications (omitted `id`, such as `notifications/initialized`) expect no reply.
            continue;
        };
        if !matches!(id, Value::Null | Value::Number(_) | Value::String(_)) {
            write_response(&mut writer, &RpcResponse::invalid_request("invalid request id"))?;
            continue;
        }

        let response = dispatch(client, base_url, token, id, &req.method, req.params).await;
        write_response(&mut writer, &response)?;
    }
    Ok(())
}

async fn dispatch(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    id: Value,
    method: &str,
    params: Value,
) -> RpcResponse {
    match method {
        "initialize" => {
            let parsed: InitializeParams = if params.is_null() {
                InitializeParams::default()
            } else {
                match serde_json::from_value(params) {
                    Ok(parsed) => parsed,
                    Err(error) => return RpcResponse::invalid_params(id, format!("invalid initialize params: {error}")),
                }
            };
            let protocol_version = parsed
                .protocol_version
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| DEFAULT_PROTOCOL_VERSION.to_string());
            RpcResponse::ok(
                id,
                InitializeResult {
                    protocol_version,
                    capabilities: ToolsCapability {
                        tools: serde_json::Map::new(),
                    },
                    server_info: ServerInfo {
                        name: MCP_SERVER_NAME,
                        version: droppen_http::SERVER_VERSION,
                    },
                },
            )
        }
        "ping" | "server/discover" => RpcResponse::ok(id, serde_json::Map::<String, Value>::new()),
        "tools/list" => RpcResponse::ok(id, ToolListResult { tools: tool_definitions() }),
        "tools/call" => {
            let parsed: ToolCallParams = match serde_json::from_value(params) {
                Ok(parsed) => parsed,
                Err(error) => return RpcResponse::invalid_params(id, format!("invalid tools/call params: {error}")),
            };
            if parsed.name.trim().is_empty() {
                return RpcResponse::invalid_params(id, "tool name must not be empty");
            }
            if !matches!(parsed.arguments, Value::Null | Value::Object(_)) {
                return RpcResponse::invalid_params(id, "tool arguments must be an object");
            }
            let result = call_tool(client, base_url, token, &parsed.name, parsed.arguments).await;
            RpcResponse::ok(id, result)
        }
        _ => RpcResponse::method_not_found(id, method),
    }
}

fn parse_read_target(raw: &str) -> Result<(Source, &str), String> {
    let target = raw.trim();
    let Some((source_raw, id)) = target.split_once('/') else {
        return Err(format!("target must be '<source>/<id>' (from list_agents), got '{target}'"));
    };
    let Some(source) = Source::parse(source_raw) else {
        return Err(format!("invalid target source: '{source_raw}'"));
    };
    if !is_valid_session_id_segment(id) {
        return Err(format!("invalid target ref: '{target}'"));
    }
    Ok((source, id))
}

fn is_valid_session_id_segment(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && !id.contains("..")
        && !id
            .chars()
            .any(|c| matches!(c, '/' | '\\' | '?' | '#' | '%') || c.is_whitespace() || c.is_ascii_control())
}

fn transcript_url(base_url: &str, source: Source, id: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(base_url).map_err(|error| format!("invalid base URL: {error}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| "base URL cannot accept path segments".to_string())?;
        segments.pop_if_empty();
        segments.extend(["api", "agents", source.as_str(), id, "transcript"]);
    }
    Ok(url)
}

fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_agents",
            description: "List other AI agent sessions (Claude Code, Codex, and ACP agents such as Antigravity) running on this Droppen instance so you can coordinate with them.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "Optional working directory prefix filter." },
                    "source": { "type": "string", "enum": ["claude", "codex", "acp"], "description": "Optional provider filter." },
                    "activeOnly": { "type": "boolean", "description": "Only return currently active/running sessions." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum number of sessions to return (default 20)." }
                }
            }),
        },
        ToolDef {
            name: "send_message",
            description: "Send a cross-session message to another Droppen agent session (Claude Code, Codex, or ACP). Delivery is asynchronous; use read_session to inspect progress or wait for a reply.",
            input_schema: json!({
                "type": "object",
                "required": ["target", "message"],
                "properties": {
                    "target": { "type": "string", "description": "Target session ref from list_agents (e.g. 'claude/<id>', 'acp/<id>', 'codex/<id>') or a unique Claude agentName." },
                    "message": { "type": "string", "description": "Plain-text message to send (max 16 KiB)." }
                }
            }),
        },
        ToolDef {
            name: "read_session",
            description: "Read recent trimmed transcript messages and live status (including whether it is waiting on a user permission or question) from another Droppen agent session.",
            input_schema: json!({
                "type": "object",
                "required": ["target"],
                "properties": {
                    "target": { "type": "string", "description": "Target session ref from list_agents (e.g. 'claude/<id>', 'acp/<id>', 'codex/<id>')." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 30, "description": "Maximum recent messages to return (default 10)." },
                    "includeTools": { "type": "boolean", "description": "Include compact one-line tool call summaries (default false)." }
                }
            }),
        },
    ]
}

async fn call_tool(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    name: &str,
    args: Value,
) -> ToolCallResult {
    match name {
        "list_agents" => {
            let args: ListAgentsArgs = if args.is_null() {
                ListAgentsArgs::default()
            } else {
                match serde_json::from_value(args) {
                    Ok(parsed) => parsed,
                    Err(error) => return ToolCallResult::text(format!("invalid arguments: {error}"), true),
                }
            };
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(cwd) = args.cwd.filter(|s| !s.is_empty()) {
                params.push(("cwd", cwd));
            }
            if let Some(source) = args.source.filter(|s| !s.is_empty()) {
                params.push(("source", source));
            }
            if let Some(active) = args.active_only {
                params.push(("activeOnly", active.to_string()));
            }
            if let Some(limit) = args.limit {
                params.push(("limit", limit.to_string()));
            }
            let url = format!("{base_url}{AGENTS_LIST_PATH}");
            execute_request(client.get(&url).bearer_auth(token).query(&params)).await
        }
        "send_message" => {
            let args: SendMessageArgs = match serde_json::from_value(args) {
                Ok(parsed) => parsed,
                Err(error) => return ToolCallResult::text(format!("invalid arguments: {error}"), true),
            };
            let url = format!("{base_url}{AGENTS_SEND_PATH}");
            execute_request(
                client
                    .post(&url)
                    .bearer_auth(token)
                    .json(&json!({ "target": args.target, "message": args.message })),
            )
            .await
        }
        "read_session" => {
            let args: ReadSessionArgs = match serde_json::from_value(args) {
                Ok(parsed) => parsed,
                Err(error) => return ToolCallResult::text(format!("invalid arguments: {error}"), true),
            };
            let (source, id) = match parse_read_target(&args.target) {
                Ok(parts) => parts,
                Err(message) => return ToolCallResult::text(message, true),
            };
            let url = match transcript_url(base_url, source, id) {
                Ok(url) => url,
                Err(message) => return ToolCallResult::text(message, true),
            };
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(limit) = args.limit {
                params.push(("limit", limit.to_string()));
            }
            if let Some(include_tools) = args.include_tools {
                params.push(("includeTools", include_tools.to_string()));
            }
            execute_request(client.get(url).bearer_auth(token).query(&params)).await
        }
        other => ToolCallResult::text(format!("unknown tool: {other}"), true),
    }
}

async fn execute_request(req: reqwest::RequestBuilder) -> ToolCallResult {
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                ToolCallResult::text(text, false)
            } else {
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.get("error")
                            .or_else(|| v.get("message"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| format!("HTTP {}: {text}", status.as_u16()));
                ToolCallResult::text(message, true)
            }
        }
        Err(error) => ToolCallResult::text(format!("cannot reach Droppen server: {error}"), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gotcha::axum::{routing::get, Json, Router};
    use std::io::Cursor;

    async fn run_framed_lines(raw_input: &[u8], limit: usize, base_url: &str) -> Vec<Value> {
        let mut cursor = Cursor::new(raw_input);
        let (tx, rx) = mpsc::channel(32);
        while let Some(line) = read_bounded_line(&mut cursor, limit).expect("read") {
            tx.send(Ok(line)).await.expect("send");
        }
        drop(tx);

        let mut output = Vec::new();
        let client = reqwest::Client::new();
        serve_lines(rx, &mut output, &client, base_url, "rha1.test-token")
            .await
            .expect("serve");
        String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect()
    }

    #[tokio::test]
    async fn stdio_mcp_handles_handshake_and_proxies_tools_over_loopback() {
        let app = Router::new().route(
            "/api/agents",
            get(|headers: gotcha::axum::http::HeaderMap| async move {
                assert_eq!(
                    headers.get("authorization").and_then(|v| v.to_str().ok()),
                    Some("Bearer rha1.test-token")
                );
                Json(json!({ "self": { "ref": "acp/s1" }, "agents": [{ "ref": "claude/c1" }] }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let _ = gotcha::axum::serve(listener, app).await;
        });

        let inputs = [
            json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}).to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}).to_string(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}).to_string(),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}).to_string(),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_agents","arguments":{}}}).to_string(),
        ];
        let joined = format!("{}\n", inputs.join("\n"));
        let lines = run_framed_lines(joined.as_bytes(), MAX_LINE_BYTES, &format!("http://{addr}")).await;
        server.abort();

        assert_eq!(lines.len(), 4, "notification produces no response");
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[1]["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(lines[1]["result"]["serverInfo"]["name"], "droppen-agents");
        let tools = lines[2]["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 3);
        assert_eq!(lines[3]["result"]["isError"], false);
        assert!(lines[3]["result"]["content"][0]["text"].as_str().unwrap().contains("claude/c1"));
    }

    #[tokio::test]
    async fn jsonrpc_distinguishes_null_id_parse_errors_and_invalid_params() {
        let inputs = [
            // Omitted id -> notification (no response)
            json!({"jsonrpc":"2.0","method":"ping","params":{}}).to_string(),
            // Explicit null id -> must respond with id: null
            json!({"jsonrpc":"2.0","id":null,"method":"ping","params":{}}).to_string(),
            // Malformed JSON -> -32700 parse error with id: null
            "{not valid json".to_string(),
            // Malformed tools/call params (missing name) -> -32602
            json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{}}).to_string(),
            // Malformed tools/call arguments (non-object) -> -32602
            json!({"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"list_agents","arguments":"bad"}}).to_string(),
            // Wrong tool argument types -> tool error (isError: true), not silently defaulted
            json!({"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"list_agents","arguments":{"limit":"ten"}}}).to_string(),
        ];
        let joined = format!("{}\n", inputs.join("\n"));
        let lines = run_framed_lines(joined.as_bytes(), MAX_LINE_BYTES, "http://127.0.0.1:1").await;

        assert_eq!(lines.len(), 5, "{lines:#?}");
        assert_eq!(lines[0]["id"], Value::Null);
        assert!(lines[0]["result"].is_object());

        assert_eq!(lines[1]["id"], Value::Null);
        assert_eq!(lines[1]["error"]["code"], -32700);

        assert_eq!(lines[2]["id"], 10);
        assert_eq!(lines[2]["error"]["code"], -32602);

        assert_eq!(lines[3]["id"], 11);
        assert_eq!(lines[3]["error"]["code"], -32602);

        assert_eq!(lines[4]["id"], 12);
        assert_eq!(lines[4]["result"]["isError"], true);
        assert!(lines[4]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("invalid arguments"));
    }

    #[tokio::test]
    async fn oversized_stdin_line_returns_parse_error_and_drains_cleanly() {
        let oversized = format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{}\"}}\n", "x".repeat(256));
        let valid = format!("{}\n", json!({"jsonrpc":"2.0","id":2,"method":"ping","params":{}}));
        let combined = format!("{oversized}{valid}");

        let lines = run_framed_lines(combined.as_bytes(), 128, "http://127.0.0.1:1").await;
        assert_eq!(lines.len(), 2, "{lines:#?}");
        assert_eq!(lines[0]["id"], Value::Null);
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["id"], 2);
        assert!(lines[1]["result"].is_object());
    }

    #[test]
    fn read_session_target_validation_rejects_traversal_and_url_metachars() {
        assert_eq!(parse_read_target("acp/sess-123_abc"), Ok((Source::Acp, "sess-123_abc")));
        assert_eq!(parse_read_target("claude/uuid-1"), Ok((Source::Claude, "uuid-1")));
        assert_eq!(parse_read_target("codex/thread-1"), Ok((Source::Codex, "thread-1")));

        for bad in [
            "unknown/sess-1",
            "acp/",
            "/sess-1",
            "acp/.",
            "acp/..",
            "acp/a..b",
            "acp/a/b",
            "acp/a\\b",
            "acp/id?limit=999",
            "acp/id#frag",
            "acp/id%2f..",
            "acp/id with space",
            "acp/id\tbad",
            "acp/id\x07bell",
        ] {
            assert!(parse_read_target(bad).is_err(), "expected rejection for {bad:?}");
        }

        let built = transcript_url("http://127.0.0.1:4321", Source::Acp, "sess-1").expect("url");
        assert_eq!(built.as_str(), "http://127.0.0.1:4321/api/agents/acp/sess-1/transcript");
    }

    #[test]
    fn read_bounded_line_retries_eintr() {
        struct InterruptOnceReader<'a> {
            interrupted: bool,
            data: &'a [u8],
            pos: usize,
        }
        impl std::io::Read for InterruptOnceReader<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let rem = &self.data[self.pos..];
                let n = rem.len().min(buf.len());
                buf[..n].copy_from_slice(&rem[..n]);
                self.pos += n;
                Ok(n)
            }
        }
        impl BufRead for InterruptOnceReader<'_> {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "EINTR"));
                }
                Ok(&self.data[self.pos..])
            }
            fn consume(&mut self, amt: usize) {
                self.pos = (self.pos + amt).min(self.data.len());
            }
        }

        let mut reader = InterruptOnceReader {
            interrupted: false,
            data: b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n",
            pos: 0,
        };
        let line = read_bounded_line(&mut reader, 1024).expect("should retry EINTR");
        assert_eq!(
            line,
            Some(StdinLine::Line("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n".to_string()))
        );
    }
}
