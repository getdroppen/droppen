//! Pure-Rust stdio JSON-RPC 2.0 MCP server (`droppen mcp-agents`).
//!
//! Spawned by ACP agents (via `session/new` / `session/load` `mcpServers`),
//! Claude Code (`--mcp-config`), and Codex (`mcp_servers`) to expose Droppen's
//! local cross-session agent bus (`list_agents`, `send_message`, `read_session`).
//!
//! Stdout is reserved exclusively for line-delimited JSON-RPC 2.0 responses;
//! no logs or diagnostics may ever be written to stdout.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use droppen_base::process_env::{AGENTS_TOKEN_ENV, AGENTS_URL_ENV};

use super::{AGENTS_LIST_PATH, AGENTS_SEND_PATH, MCP_SERVER_NAME};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";

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

    let (tx, rx) = mpsc::channel::<std::io::Result<String>>(16);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        loop {
            let mut line = String::new();
            match lock.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
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
    mut rx: mpsc::Receiver<std::io::Result<String>>,
    mut writer: W,
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<(), String> {
    while let Some(item) = rx.recv().await {
        let line = item.map_err(|error| format!("stdin read failed: {error}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            // Notification (e.g. `notifications/initialized`): no response.
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        let response = dispatch(client, base_url, token, id, method, params).await;
        let encoded = serde_json::to_string(&response).map_err(|error| format!("encode failed: {error}"))?;
        writer
            .write_all(encoded.as_bytes())
            .map_err(|error| format!("stdout write failed: {error}"))?;
        writer
            .write_all(b"\n")
            .map_err(|error| format!("stdout write failed: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("stdout flush failed: {error}"))?;
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
) -> Value {
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL_VERSION);
            ok_response(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": MCP_SERVER_NAME,
                        "version": droppen_http::SERVER_VERSION
                    }
                }),
            )
        }
        "ping" | "server/discover" => ok_response(id, json!({})),
        "tools/list" => ok_response(id, json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let result = call_tool(client, base_url, token, name, &args).await;
            ok_response(id, result)
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("method not found: {method}")
            }
        }),
    }
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "list_agents",
            "description": "List other AI agent sessions (Claude Code, Codex, and ACP agents such as Antigravity) running on this Droppen instance so you can coordinate with them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cwd": { "type": "string", "description": "Optional working directory prefix filter." },
                    "source": { "type": "string", "enum": ["claude", "codex", "acp"], "description": "Optional provider filter." },
                    "activeOnly": { "type": "boolean", "description": "Only return currently active/running sessions." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum number of sessions to return (default 20)." }
                }
            }
        },
        {
            "name": "send_message",
            "description": "Send a cross-session message to another Droppen agent session (Claude Code, Codex, or ACP). Delivery is asynchronous; use read_session to inspect progress or wait for a reply.",
            "inputSchema": {
                "type": "object",
                "required": ["target", "message"],
                "properties": {
                    "target": { "type": "string", "description": "Target session ref from list_agents (e.g. 'claude/<id>', 'acp/<id>', 'codex/<id>') or a unique Claude agentName." },
                    "message": { "type": "string", "description": "Plain-text message to send (max 16 KiB)." }
                }
            }
        },
        {
            "name": "read_session",
            "description": "Read recent trimmed transcript messages and live status (including whether it is waiting on a user permission or question) from another Droppen agent session.",
            "inputSchema": {
                "type": "object",
                "required": ["target"],
                "properties": {
                    "target": { "type": "string", "description": "Target session ref from list_agents (e.g. 'claude/<id>', 'acp/<id>', 'codex/<id>')." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 30, "description": "Maximum recent messages to return (default 10)." },
                    "includeTools": { "type": "boolean", "description": "Include compact one-line tool call summaries (default false)." }
                }
            }
        }
    ])
}

async fn call_tool(client: &reqwest::Client, base_url: &str, token: &str, name: &str, args: &Value) -> Value {
    match name {
        "list_agents" => {
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(cwd) = args.get("cwd").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                params.push(("cwd", cwd.to_string()));
            }
            if let Some(source) = args.get("source").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                params.push(("source", source.to_string()));
            }
            if let Some(active) = args.get("activeOnly").and_then(Value::as_bool) {
                params.push(("activeOnly", active.to_string()));
            }
            if let Some(limit) = args.get("limit").and_then(Value::as_u64) {
                params.push(("limit", limit.to_string()));
            }
            let url = format!("{base_url}{AGENTS_LIST_PATH}");
            execute_request(client.get(&url).bearer_auth(token).query(&params)).await
        }
        "send_message" => {
            let Some(target) = args.get("target").and_then(Value::as_str) else {
                return tool_result("missing required argument: target".to_string(), true);
            };
            let Some(message) = args.get("message").and_then(Value::as_str) else {
                return tool_result("missing required argument: message".to_string(), true);
            };
            let url = format!("{base_url}{AGENTS_SEND_PATH}");
            execute_request(
                client
                    .post(&url)
                    .bearer_auth(token)
                    .json(&json!({ "target": target, "message": message })),
            )
            .await
        }
        "read_session" => {
            let Some(target) = args.get("target").and_then(Value::as_str).map(str::trim) else {
                return tool_result("missing required argument: target".to_string(), true);
            };
            let Some((source, id)) = target.split_once('/') else {
                return tool_result(
                    format!("target must be '<source>/<id>' (from list_agents), got '{target}'"),
                    true,
                );
            };
            if source.is_empty() || id.is_empty() || id.contains('/') || id.contains("..") {
                return tool_result(format!("invalid target ref: '{target}'"), true);
            }
            let mut params: Vec<(&str, String)> = Vec::new();
            if let Some(limit) = args.get("limit").and_then(Value::as_u64) {
                params.push(("limit", limit.to_string()));
            }
            if let Some(include_tools) = args.get("includeTools").and_then(Value::as_bool) {
                params.push(("includeTools", include_tools.to_string()));
            }
            let url = format!("{base_url}/api/agents/{source}/{id}/transcript");
            execute_request(client.get(&url).bearer_auth(token).query(&params)).await
        }
        other => tool_result(format!("unknown tool: {other}"), true),
    }
}

async fn execute_request(req: reqwest::RequestBuilder) -> Value {
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                tool_result(text, false)
            } else {
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("error").or_else(|| v.get("message")).and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_else(|| format!("HTTP {}: {text}", status.as_u16()));
                tool_result(message, true)
            }
        }
        Err(error) => tool_result(format!("cannot reach Droppen server: {error}"), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gotcha::axum::{routing::get, Json, Router};

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

        let (tx, rx) = mpsc::channel(16);
        for line in inputs {
            tx.send(Ok(format!("{line}\n"))).await.expect("send");
        }
        drop(tx);

        let mut output = Vec::new();
        let client = reqwest::Client::new();
        serve_lines(
            rx,
            &mut output,
            &client,
            &format!("http://{addr}"),
            "rha1.test-token",
        )
        .await
        .expect("serve");
        server.abort();

        let lines: Vec<Value> = String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        assert_eq!(lines.len(), 4, "notification produces no response");
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[1]["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(lines[1]["result"]["serverInfo"]["name"], "droppen-agents");
        let tools = lines[2]["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 3);
        assert_eq!(lines[3]["result"]["isError"], false);
        assert!(lines[3]["result"]["content"][0]["text"].as_str().unwrap().contains("claude/c1"));
    }
}
