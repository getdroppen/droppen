//! The JSON-RPC connection to one agent process: one message per line, both
//! ways, over the agent's stdio.
//!
//! One task reads everything the agent writes, in order, and turns it into an
//! [`Event`] on one channel — so a turn's last `session/update` is always read
//! before the answer that ends the turn:
//!
//! * `session/update` notifications become [`Event::Update`];
//! * the answer to `session/prompt` becomes [`Event::Prompted`];
//! * the answers to the other requests go to whoever is awaiting them;
//! * requests the agent makes of this client go to [`super::client`], each on
//!   a task of its own, because `terminal/wait_for_exit` is answered only when
//!   the command ends;
//! * the end of the agent's output becomes [`Event::Closed`].
//!
//! Anything on stdout that is not a JSON object is skipped: it is a plugin
//! printing to the wrong stream, not the protocol.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};

use super::client::Client;
use super::{absolute, InjectError, Replay};
use crate::acp::{config_options, ConfigOption, PromptResponse, SessionNotification, PROTOCOL_VERSION};

/// How long a write to the agent's stdin may block.
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Something the agent said, in the order it said it.
pub(super) enum Event {
    /// A `session/update`.
    Update(SessionNotification),
    /// The answer to the `session/prompt` in flight: how the turn ended, or the
    /// agent's error.
    Prompted(Result<PromptResponse, String>),
    /// The agent's output ended: it exited, or closed it.
    Closed,
}

/// The agent's stdin, shared by the driver and the tasks answering its
/// requests; `None` once closed, which is how an agent is told to exit.
#[derive(Clone)]
struct Writer(Arc<tokio::sync::Mutex<Option<ChildStdin>>>);

impl Writer {
    /// Write one message. `false` when the input is closed or the write failed.
    async fn send(&self, value: &Value) -> bool {
        let mut line = value.to_string();
        line.push('\n');
        let mut stdin = self.0.lock().await;
        let Some(pipe) = stdin.as_mut() else { return false };
        let written = tokio::time::timeout(WRITE_TIMEOUT, async { pipe.write_all(line.as_bytes()).await.and(pipe.flush().await) }).await;
        matches!(written, Ok(Ok(())))
    }

    async fn close(&self) {
        drop(self.0.lock().await.take());
    }
}

/// Requests awaiting an answer, by id, and the one `session/prompt` whose
/// answer is an [`Event`] rather than a reply.
#[derive(Default)]
struct Pending {
    replies: HashMap<String, oneshot::Sender<Result<Value, String>>>,
    prompt: Option<String>,
}

/// A session [`Conn::open`] created or reopened.
pub(super) struct Opened {
    /// Its id.
    pub(super) id: String,
    /// The history the agent replayed while reopening it.
    pub(super) replay: Replay,
    /// Its configuration options as they stand.
    pub(super) options: Vec<ConfigOption>,
}

/// One agent's connection.
pub(super) struct Conn {
    writer: Writer,
    pending: Arc<Mutex<Pending>>,
    next: AtomicU64,
    /// The agent can close a session (`session/close`).
    can_close: bool,
    /// What the agent says; see [`Event`].
    pub(super) events: mpsc::UnboundedReceiver<Event>,
}

/// A line the agent wrote that is a JSON-RPC message.
fn parse(line: &str) -> Option<Map<String, Value>> {
    if !line.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// The message an error answer carries.
fn error_text(error: &Value) -> String {
    error["message"].as_str().map(str::to_owned).unwrap_or_else(|| error.to_string())
}

impl Conn {
    /// Connect to the agent behind these pipes.
    pub(super) async fn start(stdin: ChildStdin, stdout: ChildStdout, client: Arc<Client>) -> Result<Conn, InjectError> {
        let writer = Writer(Arc::new(tokio::sync::Mutex::new(Some(stdin))));
        let pending = Arc::new(Mutex::new(Pending::default()));
        let (events_tx, events) = mpsc::unbounded_channel();
        tokio::spawn(read(stdout, writer.clone(), pending.clone(), events_tx, client));
        Ok(Conn {
            writer,
            pending,
            next: AtomicU64::new(0),
            can_close: false,
            events,
        })
    }

    fn next_id(&self) -> String {
        format!("droppen-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Send a request and wait for its answer.
    async fn call(&self, method: &str, params: Value) -> Result<Value, InjectError> {
        let id = self.next_id();
        let (reply, answer) = oneshot::channel();
        self.pending.lock().unwrap().replies.insert(id.clone(), reply);
        if !self
            .writer
            .send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await
        {
            self.pending.lock().unwrap().replies.remove(&id);
            return Err(InjectError::message("the agent closed its input"));
        }
        match answer.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(InjectError::message(format!("{method} failed: {error}"))),
            Err(_) => Err(InjectError::message("the agent exited")),
        }
    }

    /// Say who this client is and what it can do; the agent's answer.
    pub(super) async fn initialize(&mut self) -> Result<Value, InjectError> {
        let init = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": true},
                    "clientInfo": {"name": "droppen", "title": "Droppen", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        self.can_close = init["agentCapabilities"]["sessionCapabilities"]["close"].is_object();
        Ok(init)
    }

    /// Initialize, then create or reopen the session.
    pub(super) async fn open(&mut self, id: Option<&str>, workdir: &Path) -> Result<Opened, InjectError> {
        let init = self.initialize().await?;
        let cwd = absolute(workdir);
        match id {
            None => {
                let created = self.call("session/new", json!({"cwd": cwd, "mcpServers": []})).await?;
                let id = created["sessionId"]
                    .as_str()
                    .filter(|id| crate::sessions::acp_transcript::valid_id(id))
                    .ok_or_else(|| InjectError::message("the agent returned no usable session id"))?;
                Ok(Opened {
                    id: id.to_string(),
                    replay: Vec::new(),
                    options: config_options(&created),
                })
            }
            Some(id) => {
                if init["agentCapabilities"]["loadSession"] != json!(true) {
                    return Err(InjectError::message("this agent cannot reopen a session"));
                }
                let loaded = self.call("session/load", json!({"sessionId": id, "cwd": cwd, "mcpServers": []})).await?;
                // Every update the replay sent was read before the answer
                // that ended it, so it is already on the channel.
                let mut replayed = Vec::new();
                while let Ok(event) = self.events.try_recv() {
                    match event {
                        Event::Update(notification) if notification.session_id == id => replayed.push(notification.update),
                        Event::Closed => return Err(InjectError::message("the agent exited")),
                        _ => {}
                    }
                }
                Ok(Opened {
                    id: id.to_string(),
                    replay: replayed,
                    options: config_options(&loaded),
                })
            }
        }
    }

    /// Set one configuration option; the session's options as they now stand.
    pub(super) async fn set_config_option(&self, id: &str, config_id: &str, value: &str) -> Result<Vec<ConfigOption>, InjectError> {
        let answer = self
            .call(
                "session/set_config_option",
                json!({"sessionId": id, "configId": config_id, "value": value}),
            )
            .await?;
        Ok(config_options(&answer))
    }

    /// Forget a session, where the agent can: what a probe opened only to read
    /// its options is not a conversation anybody should find later.
    pub(super) async fn close_session(&self, id: &str) {
        if self.can_close {
            if self.call("session/close", json!({ "sessionId": id })).await.is_err() {
                // Not what it said: the agent's own words are not logged.
                tracing::debug!("the agent did not close a probe session");
            }
        }
    }

    /// Start a turn. Its end arrives as [`Event::Prompted`], after its updates.
    pub(super) async fn prompt(&self, id: &str, text: String) -> Result<(), InjectError> {
        let request = self.next_id();
        self.pending.lock().unwrap().prompt = Some(request.clone());
        let sent = self
            .writer
            .send(&json!({
                "jsonrpc": "2.0", "id": request, "method": "session/prompt",
                "params": {"sessionId": id, "prompt": [{"type": "text", "text": text}]},
            }))
            .await;
        if sent {
            Ok(())
        } else {
            Err(InjectError::message("the agent closed its input"))
        }
    }

    /// Ask the agent to end the turn in flight.
    pub(super) async fn cancel(&self, id: &str) -> Result<(), InjectError> {
        if self
            .writer
            .send(&json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": id}}))
            .await
        {
            Ok(())
        } else {
            Err(InjectError::message("the agent closed its input"))
        }
    }

    /// Close the agent's stdin: how an ACP agent over stdio is told to exit.
    pub(super) async fn close(&mut self) {
        self.writer.close().await;
    }
}

/// The longest line read from an agent: far more than a message carries — a
/// whole file the agent writes through the client is one line — and a bound on
/// what a program that is not speaking ACP can make this server hold.
const MAX_LINE: usize = 64 * 1024 * 1024;
/// What the line buffer keeps between lines: one long line is not held on to.
const KEEP_LINE: usize = 64 * 1024;

/// The agent's stdout, a line at a time, none longer than `max`.
struct Lines<R> {
    reader: BufReader<R>,
    max: usize,
    line: Vec<u8>,
}

impl<R: tokio::io::AsyncRead + Unpin> Lines<R> {
    fn new(output: R, max: usize) -> Self {
        Self {
            reader: BufReader::new(output),
            max,
            line: Vec::new(),
        }
    }

    /// The next line, without its line ending; `None` at the end of the
    /// output. A line longer than `max` ends it too: nothing after it can be
    /// trusted to be in step, and holding it would be unbounded. A line that is
    /// not UTF-8 is skipped, as anything else that is not the protocol is.
    async fn next(&mut self) -> Option<String> {
        loop {
            self.line.clear();
            self.line.shrink_to(KEEP_LINE);
            let read = (&mut self.reader)
                .take(self.max as u64 + 1)
                .read_until(b'\n', &mut self.line)
                .await
                .ok()?;
            if read == 0 {
                return None;
            }
            if self.line.len() > self.max && !self.line.ends_with(b"\n") {
                tracing::warn!("an ACP agent wrote a line longer than {} bytes; reading no more of it", self.max);
                return None;
            }
            let end = self.line.strip_suffix(b"\n").unwrap_or(&self.line);
            let end = end.strip_suffix(b"\r").unwrap_or(end);
            if let Ok(line) = std::str::from_utf8(end) {
                return Some(line.to_string());
            }
        }
    }
}

/// Read everything the agent writes; see the module note.
async fn read(stdout: ChildStdout, writer: Writer, pending: Arc<Mutex<Pending>>, events: mpsc::UnboundedSender<Event>, client: Arc<Client>) {
    let mut lines = Lines::new(stdout, MAX_LINE);
    while let Some(line) = lines.next().await {
        let Some(message) = parse(&line) else { continue };
        match (message.get("method").and_then(Value::as_str), message.get("id")) {
            // A request this client is to answer.
            (Some(method), Some(id)) => {
                let (method, id, params) = (method.to_string(), id.clone(), message.get("params").cloned().unwrap_or(Value::Null));
                let (client, writer) = (client.clone(), writer.clone());
                tokio::spawn(async move {
                    let reply = match client.handle(&method, params).await {
                        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": error.code, "message": error.message}}),
                    };
                    writer.send(&reply).await;
                });
            }
            (Some("session/update"), None) => match serde_json::from_value(message.get("params").cloned().unwrap_or(Value::Null)) {
                Ok(notification) => {
                    let _ = events.send(Event::Update(notification));
                }
                // Where it failed, not the message: serde quotes the values it saw.
                Err(error) => tracing::debug!(
                    category = ?error.classify(),
                    column = error.column(),
                    "skipping a session/update this build cannot read"
                ),
            },
            (Some(_), None) => {}
            // An answer to one of this client's requests.
            (None, Some(id)) => {
                let id = id.as_str().map(str::to_owned).unwrap_or_else(|| id.to_string());
                let outcome = match (message.get("error"), message.get("result")) {
                    (Some(error), _) => Err(error_text(error)),
                    (None, result) => Ok(result.cloned().unwrap_or(Value::Null)),
                };
                let mut pending = pending.lock().unwrap();
                if pending.prompt.as_deref() == Some(id.as_str()) {
                    pending.prompt = None;
                    let ended = outcome.and_then(|result| serde_json::from_value(result).map_err(|error| error.to_string()));
                    let _ = events.send(Event::Prompted(ended));
                } else if let Some(reply) = pending.replies.remove(&id) {
                    let _ = reply.send(outcome);
                }
            }
            (None, None) => {}
        }
    }
    // Whatever was still waiting will not be answered now.
    pending.lock().unwrap().replies.clear();
    let _ = events.send(Event::Closed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_json_objects_reach_the_protocol() {
        assert!(parse(r#"{"jsonrpc":"2.0","method":"x"}"#).is_some());
        assert!(parse(r#"  {"jsonrpc":"2.0","id":1,"result":{}}"#).is_some());
        assert!(parse("Loading plugins...").is_none());
        assert!(parse("[1, 2]").is_none());
        assert!(parse("{not json").is_none());
    }

    #[tokio::test]
    async fn a_line_too_long_ends_the_output_and_one_that_is_not_text_is_skipped() {
        let output: &[u8] = b"one\r\n\xff\xfe\ntwo\nlast";
        let mut lines = Lines::new(output, 8);
        assert_eq!(lines.next().await.as_deref(), Some("one"));
        assert_eq!(lines.next().await.as_deref(), Some("two"), "not UTF-8: skipped");
        assert_eq!(lines.next().await.as_deref(), Some("last"), "the end of the output ends a line");
        assert_eq!(lines.next().await, None);

        let output: &[u8] = b"12345678\n123456789\nafter\n";
        let mut lines = Lines::new(output, 8);
        assert_eq!(lines.next().await.as_deref(), Some("12345678"), "at the bound: kept");
        assert_eq!(lines.next().await, None, "past it: the end");
    }
}
