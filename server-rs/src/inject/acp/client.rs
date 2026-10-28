//! The client half of ACP: what an agent asks of the program driving it.
//!
//! An ACP agent may run its shell and file tools through the client rather
//! than itself — `terminal/*` and `fs/*` requests — so that an editor can show
//! the terminal and the edit. Some insist on it: an agent may swap its shell
//! and file tools for client-routed versions inside every turn, with no
//! fallback, so a client that cannot answer them leaves it with no way to run
//! a command or touch a file at all.
//!
//! This server answers them on the machine the agent runs on, as the user the
//! agent runs as: the same reach the agent's own built-in tools have, and
//! nothing more. A terminal is a process group of its own, so killing one
//! takes the commands its shell started with it, and every terminal still
//! running when the agent's process goes is killed with it.
//!
//! Each request is answered on a task of its own (see [`super::conn`]),
//! because `terminal/wait_for_exit` is answered when the command ends —
//! minutes later, perhaps.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::watch;

use crate::acp::{CreateTerminalParams, ReadTextFileParams, TerminalParams, WriteTextFileParams};
use crate::process_env::Env;

/// How much of a terminal's output is kept when the agent names no limit.
const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
/// The most a terminal may keep, whatever the agent asks for.
const MAX_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
/// The most terminals one session may hold open.
const MAX_TERMINALS: usize = 32;

/// JSON-RPC's codes for the refusals below, and ACP's for a missing resource.
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const RESOURCE_NOT_FOUND: i64 = -32002;

/// A refusal, as a JSON-RPC error carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Error {
    pub(super) code: i64,
    pub(super) message: String,
}

impl Error {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// A request's parameters, read as `T`.
fn params<T: DeserializeOwned>(params: Value) -> Result<T, Error> {
    serde_json::from_value(params).map_err(|error| Error::new(INVALID_PARAMS, error.to_string()))
}

/// How a terminal's command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Exit {
    code: Option<i32>,
    signal: Option<String>,
}

impl Exit {
    fn to_json(&self) -> Value {
        json!({"exitCode": self.code, "signal": self.signal})
    }
}

/// One command the agent started.
struct Terminal {
    /// The process group, which is the command's own pid.
    group: Option<u32>,
    /// What it printed, newest kept, and whether anything was dropped.
    output: Arc<Mutex<(Vec<u8>, bool)>>,
    exit: watch::Receiver<Option<Exit>>,
}

impl Terminal {
    fn kill(&self) {
        if self.exit.borrow().is_some() {
            return;
        }
        if let Some(group) = self.group {
            // SAFETY: a signal to a process group this server created and has
            // not seen exit; the worst outcome is an `ESRCH`, which is ignored.
            unsafe { libc::kill(-(group as libc::pid_t), libc::SIGKILL) };
        }
    }
}

/// What one agent process may ask of this server.
pub(super) struct Client {
    /// Where a terminal runs when the agent names no directory.
    workdir: PathBuf,
    /// The environment the agent itself was started with, which its commands
    /// inherit.
    env: Env,
    terminals: Mutex<HashMap<String, Arc<Terminal>>>,
    next: AtomicU64,
}

impl Client {
    pub(super) fn new(workdir: PathBuf, env: Env) -> Arc<Self> {
        Arc::new(Self {
            workdir,
            env,
            terminals: Mutex::new(HashMap::new()),
            next: AtomicU64::new(0),
        })
    }

    /// Kill every terminal still running: the agent's process is gone, and
    /// nothing is left to read them or to stop them later.
    pub(super) fn kill_all(&self) {
        for terminal in self.terminals.lock().unwrap().drain().map(|(_, terminal)| terminal) {
            terminal.kill();
        }
    }

    /// Answer one request the agent made.
    pub(super) async fn handle(&self, method: &str, request: Value) -> Result<Value, Error> {
        match method {
            "fs/read_text_file" => self.read_text_file(params(request)?).await,
            "fs/write_text_file" => self.write_text_file(params(request)?).await,
            "terminal/create" => self.create_terminal(params(request)?),
            "terminal/output" => self.terminal_output(&params::<TerminalParams>(request)?.terminal_id),
            "terminal/wait_for_exit" => self.wait_for_terminal_exit(&params::<TerminalParams>(request)?.terminal_id).await,
            "terminal/kill" => {
                self.terminal(&params::<TerminalParams>(request)?.terminal_id)?.kill();
                Ok(json!({}))
            }
            "terminal/release" => {
                let id = params::<TerminalParams>(request)?.terminal_id;
                if let Some(terminal) = self.terminals.lock().unwrap().remove(&id) {
                    terminal.kill();
                }
                Ok(json!({}))
            }
            // There is nobody at this end of the pipe to ask, and approving
            // whatever a model wants to run is not this server's call to make.
            "session/request_permission" => Ok(json!({"outcome": {"outcome": "cancelled"}})),
            other => Err(Error::new(METHOD_NOT_FOUND, format!("{other} is not supported by Droppen"))),
        }
    }

    async fn read_text_file(&self, request: ReadTextFileParams) -> Result<Value, Error> {
        let path = absolute(&request.path)?;
        let text = tokio::fs::read(path)
            .await
            .map_err(|error| Error::new(RESOURCE_NOT_FOUND, format!("cannot read {}: {error}", path.display())))?;
        let text = String::from_utf8_lossy(&text);
        // `line` is 1-based; both it and `limit` count lines.
        let start = request.line.map_or(0, |line| line.saturating_sub(1) as usize);
        let content = if start == 0 && request.limit.is_none() {
            text.into_owned()
        } else {
            let lines = text.split_inclusive('\n').skip(start);
            match request.limit {
                Some(limit) => lines.take(limit as usize).collect(),
                None => lines.collect(),
            }
        };
        Ok(json!({ "content": content }))
    }

    async fn write_text_file(&self, request: WriteTextFileParams) -> Result<Value, Error> {
        let path = absolute(&request.path)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| Error::new(INTERNAL_ERROR, format!("cannot create {}: {error}", parent.display())))?;
        }
        tokio::fs::write(path, &request.content)
            .await
            .map_err(|error| Error::new(INTERNAL_ERROR, format!("cannot write {}: {error}", path.display())))?;
        Ok(json!({}))
    }

    fn create_terminal(&self, request: CreateTerminalParams) -> Result<Value, Error> {
        if request.command.is_empty() {
            return Err(Error::new(INVALID_PARAMS, "no command"));
        }
        if self.terminals.lock().unwrap().len() >= MAX_TERMINALS {
            return Err(Error::new(
                INTERNAL_ERROR,
                format!("more than {MAX_TERMINALS} terminals are open; release one first"),
            ));
        }
        let cwd = request.cwd.clone().unwrap_or_else(|| self.workdir.clone());
        let mut env = self.env.clone();
        for var in &request.env {
            env.insert(var.name.clone(), var.value.clone());
        }
        let limit = request.output_byte_limit.map_or(DEFAULT_OUTPUT_LIMIT, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX).min(MAX_OUTPUT_LIMIT)
        });
        let mut child = tokio::process::Command::new(&request.command)
            .args(&request.args)
            .current_dir(&cwd)
            .env_clear()
            .envs(&env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|error| Error::new(INTERNAL_ERROR, format!("cannot run {}: {error}", request.command)))?;
        let output = Arc::new(Mutex::new((Vec::new(), false)));
        let (exit_tx, exit) = watch::channel(None);
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let group = child.id();
        let collected = output.clone();
        tokio::spawn(async move {
            tokio::join!(collect(stdout, &collected, limit), collect(stderr, &collected, limit));
            let exit = match child.wait().await {
                Ok(status) => {
                    use std::os::unix::process::ExitStatusExt;
                    Exit {
                        code: status.code(),
                        signal: status.signal().map(super::super::signal_name),
                    }
                }
                Err(_) => Exit { code: None, signal: None },
            };
            let _ = exit_tx.send(Some(exit));
        });
        let id = format!("term-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        self.terminals
            .lock()
            .unwrap()
            .insert(id.clone(), Arc::new(Terminal { group, output, exit }));
        Ok(json!({ "terminalId": id }))
    }

    fn terminal_output(&self, id: &str) -> Result<Value, Error> {
        let terminal = self.terminal(id)?;
        let (bytes, truncated) = &*terminal.output.lock().unwrap();
        Ok(json!({
            "output": String::from_utf8_lossy(bytes),
            "truncated": truncated,
            "exitStatus": terminal.exit.borrow().as_ref().map(Exit::to_json),
        }))
    }

    async fn wait_for_terminal_exit(&self, id: &str) -> Result<Value, Error> {
        let mut exit = self.terminal(id)?.exit.clone();
        let ended = exit.wait_for(Option::is_some).await.map(|exit| exit.clone());
        match ended {
            Ok(Some(exit)) => Ok(exit.to_json()),
            _ => Err(Error::new(INTERNAL_ERROR, "the terminal is gone")),
        }
    }

    fn terminal(&self, id: &str) -> Result<Arc<Terminal>, Error> {
        self.terminals
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new(INVALID_PARAMS, format!("no terminal {id}")))
    }
}

/// Append everything `pipe` yields to `output`, keeping the newest `limit` bytes.
async fn collect(mut pipe: impl AsyncRead + Unpin, output: &Mutex<(Vec<u8>, bool)>, limit: usize) {
    let mut buffer = [0u8; 8192];
    while let Ok(read) = pipe.read(&mut buffer).await {
        if read == 0 {
            return;
        }
        let mut output = output.lock().unwrap();
        output.0.extend_from_slice(&buffer[..read]);
        if output.0.len() > limit {
            // Cut on a character boundary, so what is kept is still text.
            let mut cut = output.0.len() - limit;
            while cut < output.0.len() && (output.0[cut] & 0b1100_0000) == 0b1000_0000 {
                cut += 1;
            }
            output.0.drain(..cut);
            output.1 = true;
        }
    }
}

/// The path a file request names: absolute, as ACP requires.
fn absolute(path: &Path) -> Result<&Path, Error> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(Error::new(INVALID_PARAMS, "the path must be absolute"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(dir: &Path) -> Arc<Client> {
        Client::new(dir.to_path_buf(), crate::process_env::clean_env_bare())
    }

    #[tokio::test]
    async fn files_are_read_by_line_and_written_whole() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub/notes.txt");
        let client = client(dir.path());
        client
            .handle(
                "fs/write_text_file",
                json!({"sessionId": "s", "path": file, "content": "one\ntwo\nthree\n"}),
            )
            .await
            .expect("written");
        let all = client.handle("fs/read_text_file", json!({"path": file})).await.expect("read");
        assert_eq!(all["content"], "one\ntwo\nthree\n");
        let part = client
            .handle("fs/read_text_file", json!({"path": file, "line": 2, "limit": 1}))
            .await
            .expect("read");
        assert_eq!(part["content"], "two\n");
        assert_eq!(
            client
                .handle("fs/read_text_file", json!({"path": "relative.txt"}))
                .await
                .unwrap_err()
                .code,
            INVALID_PARAMS
        );
        assert_eq!(client.handle("fs/read_text_file", json!({})).await.unwrap_err().code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn a_terminal_runs_reports_and_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path());
        let created = client
            .handle(
                "terminal/create",
                json!({"command": "bash", "args": ["-c", "echo out; echo err >&2; exit 3"]}),
            )
            .await
            .expect("created");
        let id = created["terminalId"].clone();
        let exit = client.handle("terminal/wait_for_exit", json!({"terminalId": id})).await.expect("exited");
        assert_eq!(exit["exitCode"], 3);
        let output = client.handle("terminal/output", json!({"terminalId": id})).await.expect("output");
        let text = output["output"].as_str().unwrap();
        assert!(text.contains("out") && text.contains("err"), "{text}");
        assert_eq!(output["exitStatus"]["exitCode"], 3);
        client.handle("terminal/release", json!({"terminalId": id})).await.expect("released");
        assert!(client.handle("terminal/output", json!({"terminalId": id})).await.is_err());
    }

    #[tokio::test]
    async fn a_killed_terminal_takes_its_children_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path());
        let created = client
            .handle(
                "terminal/create",
                json!({"command": "bash", "args": ["-c", "sleep 30 & wait"], "cwd": dir.path()}),
            )
            .await
            .expect("created");
        let id = created["terminalId"].clone();
        client.handle("terminal/kill", json!({"terminalId": id})).await.expect("killed");
        let exit = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.handle("terminal/wait_for_exit", json!({"terminalId": id})),
        )
        .await
        .expect("the group died, so its pipes closed")
        .expect("exited");
        assert_eq!(exit["signal"], "SIGKILL");
    }

    #[tokio::test]
    async fn output_keeps_the_newest_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path());
        let created = client
            .handle(
                "terminal/create",
                json!({"command": "bash", "args": ["-c", "printf 'abcdefghij'"], "outputByteLimit": 4}),
            )
            .await
            .expect("created");
        let id = created["terminalId"].clone();
        client.handle("terminal/wait_for_exit", json!({"terminalId": id})).await.expect("exited");
        let output = client.handle("terminal/output", json!({"terminalId": id})).await.expect("output");
        assert_eq!((output["output"].as_str(), output["truncated"].as_bool()), (Some("ghij"), Some(true)));
    }

    #[tokio::test]
    async fn permission_is_declined_and_the_unknown_refused() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path());
        let answer = client.handle("session/request_permission", json!({})).await.expect("answered");
        assert_eq!(answer["outcome"]["outcome"], "cancelled");
        assert_eq!(client.handle("nope", json!({})).await.unwrap_err().code, METHOD_NOT_FOUND);
    }
}
