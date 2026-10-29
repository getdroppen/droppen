//! Every program the first-run guide runs, behind one seam.
//!
//! Detection is nothing but asking CLIs about themselves — `claude --version`,
//! `codex login status`, `xcode-select -p`, a login shell — and every one of
//! those answers depends on the machine the test happens to run on. So none of
//! them is spawned directly: each is described as an [`Invocation`] (or, for
//! the one conversation that is not a single command, an [`RpcCall`]) and
//! handed to a [`Runner`]. The server's runner is [`SystemRunner`]; the unit
//! tests hand in a fixture that answers from canned text and records what it
//! was asked, so the whole of detection is tested without a single process
//! being started — and so a test can say exactly which programs detection
//! runs, which is how "it never reads a credential file" is checked at the
//! level of behaviour rather than of good intentions.
//!
//! What the seam deliberately does *not* carry is a shell. An invocation is a
//! program and its argv; the only shell the guide ever starts is the user's
//! login shell, by its absolute path, with a fixed script ([`super::shell`]).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::accounts::login::Login;
use droppen_base::process_env::{apply_env, Env};

/// The most output kept from one stream of one invocation. A version string or
/// a login status is a line; this only stops something unexpected from growing
/// the server's memory.
const MAX_OUTPUT: usize = 256 * 1024;
/// How long a program that printed its `until` marker is given to exit on its
/// own before it is killed.
const UNTIL_GRACE: Duration = Duration::from_millis(500);

/// One program to run, start to finish.
#[derive(Clone)]
pub struct Invocation {
    /// The program: an absolute path, or a bare name looked up on `env`'s `PATH`.
    pub program: String,
    /// Its arguments, not including `argv[0]`.
    pub args: Vec<String>,
    /// The complete environment; nothing is inherited around it.
    pub env: Env,
    /// Working directory; the server's own when `None`.
    pub cwd: Option<PathBuf>,
    /// How long it may take before it is killed and reported as timed out.
    pub timeout: Duration,
    /// Bytes for its standard input, which is closed after them. `None` closes
    /// it at once. The only way a secret reaches a child in this module.
    pub stdin: Option<Vec<u8>>,
    /// Stop reading once standard output contains this, rather than at EOF.
    ///
    /// For the login shell, whose start-up files can leave something running
    /// in the background that holds the pipe open long after the answer was
    /// printed (the same reason the desktop app's own read stops at its marker).
    pub until: Option<String>,
}

impl Invocation {
    /// A program and its arguments, with an empty environment and no input.
    pub fn new(program: impl Into<String>, args: &[&str], timeout: Duration) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            env: Env::new(),
            cwd: None,
            timeout,
            stdin: None,
            until: None,
        }
    }

    /// The program's file name: what a fixture keys on and what a log may name.
    pub fn name(&self) -> &str {
        self.program.rsplit('/').next().unwrap_or(&self.program)
    }
}

/// Never the environment or the input: either can hold a credential.
impl std::fmt::Debug for Invocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invocation")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("timeout", &self.timeout)
            .field("stdin", &self.stdin.as_ref().map(|bytes| bytes.len()))
            .finish_non_exhaustive()
    }
}

/// What one [`Invocation`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOutput {
    /// Its exit code; `None` when it was killed, timed out, or never started.
    pub code: Option<i32>,
    /// Standard output, lossily decoded.
    pub stdout: String,
    /// Standard error, lossily decoded.
    pub stderr: String,
    /// It was still running at [`Invocation::timeout`] and was killed.
    pub timed_out: bool,
    /// It could not be started at all (no such program, not executable).
    pub spawn_failed: bool,
}

impl RunOutput {
    /// A program that ran and exited with `code`.
    pub fn exited(code: i32, stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
            ..Self::default()
        }
    }

    /// A program that never answered.
    pub fn timeout() -> Self {
        Self {
            timed_out: true,
            ..Self::default()
        }
    }

    /// A program that could not be started.
    pub fn failed_to_start() -> Self {
        Self {
            spawn_failed: true,
            ..Self::default()
        }
    }

    /// Whether it ran to completion and said yes.
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }
}

/// One request to a short-lived `codex app-server` (see
/// [`crate::accounts::quota::codex::app_server_call`]).
#[derive(Clone)]
pub struct RpcCall {
    /// The `codex` binary to start.
    pub program: PathBuf,
    /// Its working directory.
    pub cwd: PathBuf,
    /// Its complete environment.
    pub env: Env,
    /// The JSON-RPC method, e.g. `account/read`.
    pub method: &'static str,
    /// How long the conversation may take.
    pub timeout: Duration,
}

impl std::fmt::Debug for RpcCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcCall")
            .field("program", &self.program)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

/// How the guide runs things. See the module docs.
pub trait Runner: Send + Sync {
    /// Run one program to completion (or to its timeout).
    fn run(&self, invocation: Invocation) -> BoxFuture<'static, RunOutput>;
    /// Ask a `codex app-server` one question; the `result` of its answer, or
    /// `None` for any failure.
    fn rpc(&self, call: RpcCall) -> BoxFuture<'static, Option<serde_json::Value>>;
}

/// The runner the server uses: real processes, killed on timeout and on drop,
/// and codex's app server asked through codex's own login.
#[derive(Clone)]
pub struct SystemRunner {
    /// Codex's login, which is what knows how to talk to its app server.
    pub codex: Arc<dyn Login>,
}

impl Runner for SystemRunner {
    fn run(&self, invocation: Invocation) -> BoxFuture<'static, RunOutput> {
        Box::pin(run_process(invocation))
    }

    fn rpc(&self, call: RpcCall) -> BoxFuture<'static, Option<serde_json::Value>> {
        let codex = self.codex.clone();
        Box::pin(async move {
            match codex.app_server_call(&call.program, &call.cwd, &call.env, call.method, call.timeout) {
                Some(ask) => ask.await,
                None => None,
            }
        })
    }
}

/// Spawn, feed, read and reap one program.
async fn run_process(invocation: Invocation) -> RunOutput {
    let mut command = tokio::process::Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .stdin(if invocation.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A future dropped mid-read (a request that went away) must not leave
        // the child behind; the timeout path below kills it explicitly too.
        .kill_on_drop(true);
    if let Some(cwd) = &invocation.cwd {
        command.current_dir(cwd);
    }
    apply_env(&mut command, &invocation.env);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return RunOutput::failed_to_start(),
    };
    // Written and closed before anything is read: the only input this module
    // ever sends is one line, far below a pipe's buffer, so this cannot block
    // on a child that is not reading yet.
    if let (Some(bytes), Some(mut stdin)) = (invocation.stdin.as_deref(), child.stdin.take()) {
        let _ = stdin.write_all(bytes).await;
        let _ = stdin.shutdown().await;
        drop(stdin);
    }
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill().await;
        return RunOutput::failed_to_start();
    };
    let until = invocation.until.clone();

    let read = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (mut out_open, mut err_open) = (true, true);
        let mut out_chunk = [0u8; 8192];
        let mut err_chunk = [0u8; 8192];
        let mut marked = false;
        while out_open || err_open {
            tokio::select! {
                read = stdout.read(&mut out_chunk), if out_open => match read {
                    Ok(0) | Err(_) => out_open = false,
                    Ok(count) => {
                        keep(&mut out, &out_chunk[..count]);
                        if until.as_deref().is_some_and(|mark| String::from_utf8_lossy(&out).contains(mark)) {
                            marked = true;
                            break;
                        }
                    }
                },
                read = stderr.read(&mut err_chunk), if err_open => match read {
                    Ok(0) | Err(_) => err_open = false,
                    Ok(count) => keep(&mut err, &err_chunk[..count]),
                },
            }
        }
        (out, err, marked)
    };

    let finished = tokio::time::timeout(invocation.timeout, async {
        let (out, err, marked) = read.await;
        let code = if marked {
            // The answer is in; the program gets a moment to finish politely
            // and is stopped (by its own handle, so by pid) if it does not.
            match tokio::time::timeout(UNTIL_GRACE, child.wait()).await {
                Ok(Ok(status)) => status.code(),
                _ => {
                    let _ = child.kill().await;
                    None
                }
            }
        } else {
            child.wait().await.ok().and_then(|status| status.code())
        };
        (out, err, code)
    })
    .await;

    match finished {
        Ok((out, err, code)) => RunOutput {
            code,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            timed_out: false,
            spawn_failed: false,
        },
        Err(_) => {
            let _ = child.kill().await;
            RunOutput::timeout()
        }
    }
}

/// Append up to [`MAX_OUTPUT`] bytes.
fn keep(buffer: &mut Vec<u8>, chunk: &[u8]) {
    let room = MAX_OUTPUT.saturating_sub(buffer.len());
    buffer.extend_from_slice(&chunk[..chunk.len().min(room)]);
}

/// A runner that answers from canned text and records every question.
#[cfg(test)]
pub(crate) mod fixture {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// One recorded question: what was asked, with what, and what was piped in.
    #[derive(Debug, Clone)]
    pub(crate) struct Asked {
        /// The program's file name.
        pub name: String,
        /// The program as given (a path, usually).
        pub program: String,
        /// Its arguments.
        pub args: Vec<String>,
        /// What went to its standard input.
        pub stdin: Option<Vec<u8>>,
        /// Its environment's `PATH`.
        pub path: Option<String>,
    }

    impl Asked {
        /// `name arg arg…`, the key answers are filed under.
        pub fn line(&self) -> String {
            std::iter::once(self.name.as_str())
                .chain(self.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        }
    }

    /// Canned answers keyed by `name arg arg…`; anything unknown fails to start.
    #[derive(Default)]
    pub(crate) struct Fixture {
        answers: Mutex<HashMap<String, RunOutput>>,
        rpc: Mutex<HashMap<String, serde_json::Value>>,
        asked: Mutex<Vec<Asked>>,
        held: Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
    }

    impl Fixture {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// Answer `line` (`name arg arg…`) with `output`.
        pub fn answer(&self, line: &str, output: RunOutput) -> &Self {
            self.answers.lock().unwrap().insert(line.to_string(), output);
            self
        }

        /// Keep `line`'s answer back until the returned signal is given: a
        /// program that is still running.
        pub fn hold(&self, line: &str) -> Arc<tokio::sync::Notify> {
            let signal = Arc::new(tokio::sync::Notify::new());
            self.held.lock().unwrap().insert(line.to_string(), signal.clone());
            signal
        }

        /// Answer an app-server `method` with `result`.
        pub fn answer_rpc(&self, method: &str, result: serde_json::Value) -> &Self {
            self.rpc.lock().unwrap().insert(method.to_string(), result);
            self
        }

        /// Everything asked so far, in order.
        pub fn asked(&self) -> Vec<Asked> {
            self.asked.lock().unwrap().clone()
        }

        /// Everything asked so far, as `name arg…` lines.
        pub fn lines(&self) -> Vec<String> {
            self.asked().iter().map(Asked::line).collect()
        }
    }

    impl Runner for Fixture {
        fn run(&self, invocation: Invocation) -> BoxFuture<'static, RunOutput> {
            let asked = Asked {
                name: invocation.name().to_string(),
                program: invocation.program.clone(),
                args: invocation.args.clone(),
                stdin: invocation.stdin.clone(),
                path: invocation.env.get("PATH").cloned(),
            };
            let line = asked.line();
            self.asked.lock().unwrap().push(asked);
            // The login shell's script is long and fixed; it is filed under the
            // shell's name and its first argument.
            let shell_key = format!("{} {}", invocation.name(), invocation.args.first().cloned().unwrap_or_default());
            let answers = self.answers.lock().unwrap();
            let output = answers
                .get(&line)
                .or_else(|| answers.get(&shell_key))
                .cloned()
                .unwrap_or_else(RunOutput::failed_to_start);
            let held = self.held.lock().unwrap().get(&line).cloned();
            Box::pin(async move {
                if let Some(signal) = held {
                    signal.notified().await;
                }
                output
            })
        }

        fn rpc(&self, call: RpcCall) -> BoxFuture<'static, Option<serde_json::Value>> {
            let name = call
                .program
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.asked.lock().unwrap().push(Asked {
                name,
                program: call.program.to_string_lossy().into_owned(),
                args: vec!["app-server".to_string(), call.method.to_string()],
                stdin: None,
                path: call.env.get("PATH").cloned(),
            });
            let answer = self.rpc.lock().unwrap().get(call.method).cloned();
            Box::pin(async move { answer })
        }
    }
}
