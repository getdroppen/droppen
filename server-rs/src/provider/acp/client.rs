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
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol_schema::v1::{
    CreateTerminalResponse, KillTerminalResponse, ReadTextFileResponse, ReleaseTerminalResponse, RequestPermissionOutcome,
    RequestPermissionResponse, TerminalExitStatus, TerminalOutputResponse, WaitForTerminalExitResponse, WriteTextFileResponse,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::watch;

use crate::provider::acp::protocol::{CreateTerminalParams, ReadTextFileParams, TerminalParams, WriteTextFileParams};
use droppen_base::process_env::Env;

/// How much of a terminal's output is kept when the agent names no limit.
const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
/// The most a terminal may keep, whatever the agent asks for.
const MAX_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
/// The most terminals one session may hold open.
const MAX_TERMINALS: usize = 32;
/// How long a kill waits for the group to go before signalling it again.
const KILL_RETRY: Duration = Duration::from_millis(5);
/// How long a kill keeps signalling a group with a member still alive.
const KILL_PATIENCE: Duration = Duration::from_secs(1);

/// JSON-RPC's codes for the refusals below, and ACP's for a missing resource.
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;
const RESOURCE_NOT_FOUND: i32 = -32002;

/// A refusal, as a JSON-RPC error carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Error {
    pub(super) code: i32,
    pub(super) message: String,
}

impl Error {
    fn new(code: i32, message: impl Into<String>) -> Self {
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

/// An answer, as the JSON its reply carries.
fn answer(answer: impl Serialize) -> Result<Value, Error> {
    serde_json::to_value(answer).map_err(|error| Error::new(INTERNAL_ERROR, error.to_string()))
}

/// How a terminal's command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Exit {
    code: Option<i32>,
    signal: Option<String>,
}

impl Exit {
    /// As the protocol has it. Its exit code is unsigned, so a negative one is
    /// left out, beside the signal where there is one, rather than wrapped. No
    /// Unix exit status is negative, but some APIs report -1 for a kill.
    fn status(&self) -> TerminalExitStatus {
        TerminalExitStatus::new()
            .exit_code(self.code.and_then(|code| u32::try_from(code).ok()))
            .signal(self.signal.clone())
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
    /// Kill the command and every process in its group.
    ///
    /// One `SIGKILL` to the group can miss one: on macOS a child the shell is
    /// forking at that moment is not yet among the members the signal reaches,
    /// and lives on in the group with the terminal's pipes open (#430). So the
    /// group is signalled again every [`KILL_RETRY`] until no live member is
    /// left to take the signal, the command's exit is recorded, or
    /// [`KILL_PATIENCE`] has passed.
    ///
    /// The first signal goes out when this is called, so a caller that does
    /// not wait for the rest has still sent it; the future is the rest.
    fn kill(self: Arc<Self>) -> impl Future<Output = ()> + Send + 'static {
        let delivered = self.signal();
        async move {
            if !delivered {
                return;
            }
            let deadline = tokio::time::Instant::now() + KILL_PATIENCE;
            let mut exit = self.exit.clone();
            loop {
                // Woken early when the exit is recorded. A closed channel is a
                // reaper that is gone, with nothing more to report.
                if let Ok(Err(_)) = tokio::time::timeout(KILL_RETRY, exit.changed()).await {
                    return;
                }
                if tokio::time::Instant::now() >= deadline || !self.signal() {
                    return;
                }
            }
        }
    }

    /// One `SIGKILL` to the group while the command's exit is unrecorded, and
    /// whether a live member took it. A group whose only member left is its
    /// leader, dead but not yet reaped, answers `EPERM` on macOS; a group with
    /// no member at all answers `ESRCH`.
    fn signal(&self) -> bool {
        if self.exit.borrow().is_some() {
            return false;
        }
        let Some(group) = self.group else {
            return false;
        };
        // SAFETY: a signal to a process group this server created, whose
        // leader it has not reaped (the exit is recorded right after), so the
        // group's id is still this group's; a failure is only reported.
        unsafe { libc::kill(-(group as libc::pid_t), libc::SIGKILL) == 0 }
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
    /// nothing is left to read them or to stop them later. Each is signalled
    /// here; the signals after the first, for a child it missed, go on a task
    /// of their own.
    pub(super) fn kill_all(&self) {
        for terminal in self.terminals.lock().unwrap().drain().map(|(_, terminal)| terminal) {
            tokio::spawn(terminal.kill());
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
                self.terminal(&params::<TerminalParams>(request)?.terminal_id)?.kill().await;
                answer(KillTerminalResponse::new())
            }
            "terminal/release" => {
                let id = params::<TerminalParams>(request)?.terminal_id;
                let terminal = self.terminals.lock().unwrap().remove(&id);
                if let Some(terminal) = terminal {
                    terminal.kill().await;
                }
                answer(ReleaseTerminalResponse::new())
            }
            // There is nobody at this end of the pipe to ask, and approving
            // whatever a model wants to run is not this server's call to make.
            "session/request_permission" => answer(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)),
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
        answer(ReadTextFileResponse::new(content))
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
        answer(WriteTextFileResponse::new())
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
        answer(CreateTerminalResponse::new(id))
    }

    fn terminal_output(&self, id: &str) -> Result<Value, Error> {
        let terminal = self.terminal(id)?;
        let (bytes, truncated) = &*terminal.output.lock().unwrap();
        let status = terminal.exit.borrow().as_ref().map(Exit::status);
        answer(TerminalOutputResponse::new(String::from_utf8_lossy(bytes), *truncated).exit_status(status))
    }

    async fn wait_for_terminal_exit(&self, id: &str) -> Result<Value, Error> {
        let mut exit = self.terminal(id)?.exit.clone();
        let ended = exit.wait_for(Option::is_some).await.map(|exit| exit.clone());
        match ended {
            Ok(Some(exit)) => answer(WaitForTerminalExitResponse::new(exit.status())),
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
    use serde_json::json;

    fn client(dir: &Path) -> Arc<Client> {
        Client::new(dir.to_path_buf(), droppen_base::process_env::clean_env_bare())
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

    /// A kill can land while the shell is still forking its children, and on
    /// macOS one signal to the group then misses the child being forked, which
    /// lives on holding the terminal's pipes (#430). Each round kills a shell
    /// that starts eight children, at a delay that sweeps its first four
    /// milliseconds, and every one must end with its pipes closed.
    #[tokio::test]
    async fn a_terminal_killed_while_its_shell_forks_takes_every_child() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path());
        for round in 0..68u64 {
            let created = client
                .handle(
                    "terminal/create",
                    json!({"command": "bash", "args": ["-c", "for _ in 1 2 3 4 5 6 7 8; do sleep 30 & done; wait"], "cwd": dir.path()}),
                )
                .await
                .expect("created");
            let id = created["terminalId"].clone();
            // 0 to 4 ms in steps of 250 µs: across bash starting and forking.
            std::thread::sleep(std::time::Duration::from_micros(round % 17 * 250));
            client.handle("terminal/kill", json!({"terminalId": id})).await.expect("killed");
            let ended = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client.handle("terminal/wait_for_exit", json!({"terminalId": id})),
            )
            .await;
            if ended.is_err() {
                // Signal what is left again, so a failed run leaves no child behind.
                client.kill_all();
            }
            assert!(ended.is_ok(), "round {round}: a child outlived the kill, holding the pipes");
            client.handle("terminal/release", json!({"terminalId": id})).await.expect("released");
        }
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

    /// Each answer as the agent reads it, byte for byte: what is left out is
    /// left out, not `null`.
    #[tokio::test]
    async fn each_answer_in_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        let client = client(dir.path());
        let ask = |method: &'static str, params: Value| {
            let client = client.clone();
            async move { client.handle(method, params).await.expect(method).to_string() }
        };
        let written = ask("fs/write_text_file", json!({"sessionId": "s", "path": file, "content": "one\ntwo\n"})).await;
        assert_eq!(written, "{}");
        let read = ask("fs/read_text_file", json!({"sessionId": "s", "path": file, "line": 2})).await;
        assert_eq!(read, r#"{"content":"two\n"}"#);
        let created = ask("terminal/create", json!({"command": "/bin/sh", "args": ["-c", "printf done; exit 3"]})).await;
        assert_eq!(created, r#"{"terminalId":"term-1"}"#);
        let exited = ask("terminal/wait_for_exit", json!({"terminalId": "term-1"})).await;
        assert_eq!(exited, r#"{"exitCode":3}"#);
        let output = ask("terminal/output", json!({"terminalId": "term-1"})).await;
        assert_eq!(output, r#"{"output":"done","truncated":false,"exitStatus":{"exitCode":3}}"#);
        assert_eq!(ask("terminal/kill", json!({"terminalId": "term-1"})).await, "{}");
        assert_eq!(ask("terminal/release", json!({"terminalId": "term-1"})).await, "{}");

        let running = ask("terminal/create", json!({"command": "/bin/sh", "args": ["-c", "sleep 30"]})).await;
        assert_eq!(running, r#"{"terminalId":"term-2"}"#);
        let early = ask("terminal/output", json!({"terminalId": "term-2"})).await;
        assert_eq!(early, r#"{"output":"","truncated":false}"#);
        assert_eq!(ask("terminal/kill", json!({"terminalId": "term-2"})).await, "{}");
        let killed = ask("terminal/wait_for_exit", json!({"terminalId": "term-2"})).await;
        assert_eq!(killed, r#"{"signal":"SIGKILL"}"#);

        let permission = ask("session/request_permission", json!({})).await;
        assert_eq!(permission, r#"{"outcome":{"outcome":"cancelled"}}"#);
        let refused = client.handle("nope", json!({})).await.unwrap_err();
        assert_eq!((refused.code, refused.message.as_str()), (METHOD_NOT_FOUND, "nope is not supported by Droppen"));
    }

    /// An exit code the protocol cannot hold is left out, never wrapped; the
    /// signal is kept.
    #[test]
    fn a_negative_exit_code_is_left_out_and_the_signal_kept() {
        let status = |code: Option<i32>, signal: Option<&str>| {
            let exit = Exit {
                code,
                signal: signal.map(str::to_owned),
            };
            serde_json::to_string(&exit.status()).unwrap()
        };
        assert_eq!(status(Some(-1), Some("SIGKILL")), r#"{"signal":"SIGKILL"}"#);
        assert_eq!(status(Some(-1), None), "{}");
        assert_eq!(status(Some(0), None), r#"{"exitCode":0}"#);
        assert_eq!(status(Some(i32::MAX), None), r#"{"exitCode":2147483647}"#);
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
