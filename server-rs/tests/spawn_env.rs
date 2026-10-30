//! The whole environment, and the account's part of the argv, that each CLI is
//! started with, per provider and per kind of account.
//!
//! `tests/inject.rs` asserts on the variables an account adds. This file pins
//! everything else as well: a variable an account should not have set, or one
//! the server's own environment should have kept, fails here too. The expected
//! side is the server's own environment builder (`claude_env` / `clean_env`)
//! with the account's variables spelled out by hand.
//!
//! The server is the real binary, started with credentials of its own planted
//! in its environment ([`PLANTED`]): the harness's `CLAUDE_*` / `ANTHROPIC_*`
//! variables must never reach a child, whichever login it runs on, and only
//! the account's own block may say what key a codex spawn reads.

mod support;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use droppen_base::process_env::{claude_env, clean_env, Env};
use serde_json::{json, Value};
use support::{common, Spawn};

/// A token in the shape the store accepts; the fake CLI takes anything.
const OAUTH_SECRET: &str = "spawn-env-oauth-token";
/// A claude block the way a gateway's documentation writes one.
const CLAUDE_BLOCK: &str = "ANTHROPIC_AUTH_TOKEN=spawn-env-gateway-token\nANTHROPIC_BASE_URL=https://gateway.example.com";
/// A codex block with a key and an endpoint of its own.
const CODEX_BLOCK: &str = "OPENAI_API_KEY=spawn-env-codex-key\nOPENAI_BASE_URL=https://relay.example.com/v1";
/// A codex block whose key is under the fallback name, with no endpoint.
const CODEX_FALLBACK_BLOCK: &str = "CODEX_API_KEY=spawn-env-codex-fallback";
/// The stand-in ACP agent of `tests/acp.rs`.
const FAKE_ACP_AGENT: &str = include_str!("support/fake_acp_agent.py");
/// A session in the root this server owns.
const OWNED_ID: &str = "cccccccc-3333-4333-8333-cccccccccccc";
/// The thread the fake App Server answers under unless told otherwise.
const CODEX_THREAD: &str = "0199c1a1-2b3c-4d5e-8f60-112233445566";

/// What the server is started with on top of this process' environment: the
/// variables an agent harness or a user's shell leaves behind, credentials
/// included. The scrub every child gets ([`claude_env`] / [`clean_env`]) drops
/// the `CLAUDE*` and `ANTHROPIC_*` ones.
const PLANTED: [(&str, &str); 7] = [
    ("CLAUDECODE", "1"),
    ("CLAUDE_CODE_ENTRYPOINT", "planted"),
    ("CLAUDE_CONFIG_DIR", "/planted/claude"),
    ("CLAUDE_CODE_OAUTH_TOKEN", "planted-oauth-token"),
    ("ANTHROPIC_API_KEY", "planted-anthropic-key"),
    ("ANTHROPIC_BASE_URL", "https://planted.invalid"),
    ("OPENAI_API_KEY", "planted-openai-key"),
];
/// The planted variable the scrub leaves, as it leaves any variable of the
/// server's own that is not the harness's: a codex block's own key wins over
/// it, and nothing reads it as an account's.
const INHERITED_OPENAI_KEY: (&str, &str) = ("OPENAI_API_KEY", "planted-openai-key");

/// What the fake CLIs' interpreters add, rewrite or drop on the way in, which
/// no spawn controls: `sh` exports its working directory, its depth and the
/// command it ran; Python coerces a C locale's `LC_CTYPE` to UTF-8; and on
/// macOS the toolchain shims and the dynamic loader adjust the compiler and
/// library search paths of what they start.
const INTERPRETER_OWN: [&str; 9] = ["PWD", "OLDPWD", "SHLVL", "_", "LC_CTYPE", "CPATH", "LIBRARY_PATH", "MANPATH", "SDKROOT"];

/// Whether `sh` can pass this name on at all: it drops a variable whose name
/// is not an identifier (cargo's `CARGO_BIN_EXE_remote-hand-server`).
fn is_shell_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn comparable(env: impl IntoIterator<Item = (String, String)>) -> BTreeMap<String, String> {
    env.into_iter()
        .filter(|(key, _)| !INTERPRETER_OWN.contains(&key.as_str()) && !key.starts_with("DYLD_") && is_shell_name(key))
        .collect()
}

/// Assert that the child got exactly `expected`, naming every difference by
/// variable only: half of these values are credentials, and a failing test's
/// output is a log like any other.
fn assert_env(spawn: &Spawn, expected: Env, what: &str) {
    let got = comparable(spawn.env.clone());
    let expected = comparable(expected);
    let mut diff = Vec::new();
    for (key, value) in &expected {
        match got.get(key) {
            Some(seen) if seen == value => {}
            Some(_) => diff.push(format!("{key}: a different value")),
            None => diff.push(format!("{key}: missing")),
        }
    }
    for key in got.keys() {
        if !expected.contains_key(key) {
            diff.push(format!("{key}: not expected"));
        }
    }
    assert!(diff.is_empty(), "{what}: the child's environment differs:\n{}", diff.join("\n"));
}

fn pairs(vars: &[(&str, &str)]) -> Vec<(String, String)> {
    vars.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

/// The real server binary on a home directory of its own, with [`PLANTED`] in
/// its environment.
///
/// It kills what it started on the way out ([`common::Spawned`]): the server,
/// and then the residents and app servers the journal names, which lead
/// process groups of their own so that they outlive a server.
struct Server {
    addr: SocketAddr,
    /// Declared before `home`, so the server and its CLIs are gone before it is.
    _server: common::Spawned,
    home: tempfile::TempDir,
}

impl Server {
    async fn start() -> Server {
        support::ensure_fake_clis_on_path();
        let home = tempfile::tempdir().expect("tempdir");
        // The marker the fake CLIs walk up to find the test's own root.
        std::fs::write(home.path().join(".rh-test-root"), "").expect("marker");
        let spawned = common::spawn_server(home.path(), droppen_instance::instance::DEFAULT_INSTANCE, &PLANTED).await;
        Server {
            addr: spawned.addr,
            _server: spawned,
            home,
        }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn workspace(&self) -> PathBuf {
        let dir = self.home().join("workspace");
        std::fs::create_dir_all(&dir).expect("workspace");
        dir
    }

    fn managed_root(&self) -> String {
        support::owned_claude_root(self.home()).to_string_lossy().into_owned()
    }

    /// What every child of this server inherits on top of this process' own
    /// environment: the variables `spawn_server` starts it with, and the one
    /// planted variable the scrub leaves.
    fn inherited(&self) -> Vec<(String, String)> {
        let home = self.home().to_string_lossy().into_owned();
        pairs(&[
            ("APP_HOME_DIR", &home),
            ("APP_AUTH__ENABLED", "false"),
            ("APP_SERVER__PORT", "0"),
            ("APP_SERVER__HOST", "127.0.0.1"),
            INHERITED_OPENAI_KEY,
        ])
    }

    /// The environment a claude child is expected to get: this process'
    /// environment through the scrub, what this server inherits, and the
    /// account's variables last.
    fn claude_env(&self, account: &[(&str, &str)]) -> Env {
        let mut env = claude_env(self.inherited());
        env.extend(pairs(account));
        env
    }

    /// The same for a codex child, which gets no sandbox marker.
    fn clean_env(&self, account: &[(&str, &str)]) -> Env {
        let mut env = clean_env(self.inherited());
        env.extend(pairs(account));
        env
    }

    async fn send(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
        let bytes = serde_json::to_vec(&body).expect("json");
        let (status, _, raw) = common::http(self.addr, method, path, &[("Content-Type", "application/json")], &bytes).await;
        let value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        (status, value)
    }

    async fn send_ok(&self, method: &str, path: &str, body: Value, expect: u16) -> Value {
        let (status, value) = self.send(method, path, body).await;
        assert_eq!(status, expect, "{method} {path} -> {status}: {value}");
        value
    }

    async fn create(&self, body: Value) -> String {
        let created = self.send_ok("POST", "/api/accounts", body, 201).await;
        created["id"].as_str().expect("account id").to_string()
    }

    /// Make `account` what `provider`'s new sessions run on when they name none.
    async fn make_default(&self, account: &str, provider: &str) {
        self.send_ok("PUT", "/api/accounts/default", json!({"accountId": account, "provider": provider}), 200)
            .await;
    }

    /// Rest `account` under `scope` until cleared, written where the limit log
    /// writes a limit its CLI reported.
    fn rest(&self, account: &str, scope: &str) {
        let db = self.home().join(".local/share/remote-hand/instances/default/state.db");
        let conn = rusqlite::Connection::open(&db).expect("the instance database");
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("after 1970").as_millis() as i64;
        conn.execute(
            "INSERT INTO account_rest (account_id, scope, class, until_ms, reason, source, observed_ms)
             VALUES (?1, ?2, 'session', NULL, 'a limit the test reports', 'test', ?3)",
            rusqlite::params![account, scope, now],
        )
        .expect("rest the account");
    }

    /// Turn the notify channel on, as the settings page does.
    async fn notify_on(&self) {
        self.send_ok("PUT", "/api/notify/config", json!({"enabled": true, "endpoint": "https://api.day.app/FAKEBARKDEVICEKEY0Wc31"}), 200)
            .await;
    }

    /// `env` with what the notify channel gives claude session `id`: the
    /// `droppen-notify` program's directory ahead of `PATH`, this server's
    /// address, and a token that names the session.
    async fn with_notify(&self, mut env: Env, id: &str) -> Env {
        let bin = self.home().join(".local/share/remote-hand/instances/default/session-bin");
        let path = format!("{}:{}", bin.display(), env.get("PATH").expect("a PATH"));
        let paths = droppen_instance::paths::Paths::new(self.home().to_path_buf());
        let secret = droppen_notify::notify::store::secret(&paths).await.expect("the channel's secret");
        let key = droppen_sessions::attention::seen::key_of(droppen_sessions::Source::Claude, id);
        let token = droppen_notify::notify::token::mint(&secret, droppen_instance::instance::DEFAULT_INSTANCE, &key);
        env.insert("PATH".to_string(), path);
        env.insert("DROPPEN_NOTIFY_URL".to_string(), format!("http://127.0.0.1:{}/api/notify", self.addr.port()));
        env.insert("DROPPEN_NOTIFY_TOKEN".to_string(), token);
        env
    }

    /// Where a chat's session runs.
    fn chats_dir(&self) -> PathBuf {
        self.home().join(".local/share/remote-hand/chats")
    }

    /// Bring every scheduled task's next run to a second ago, so the runner
    /// fires it on its next tick rather than at the top of the next minute.
    fn make_tasks_due(&self) {
        let db = self.home().join(".local/share/remote-hand/instances/default/state.db");
        let conn = rusqlite::Connection::open(&db).expect("the instance database");
        let due = droppen_base::time::iso8601(SystemTime::now() - Duration::from_secs(1));
        let rows: Vec<(i64, String)> = conn
            .prepare("SELECT rowid, payload FROM scheduled_tasks")
            .expect("the task table")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("the tasks")
            .collect::<Result<_, _>>()
            .expect("the tasks");
        assert!(!rows.is_empty(), "no task to bring forward");
        for (rowid, payload) in rows {
            let mut task: Value = serde_json::from_str(&payload).expect("a task");
            task["nextRunAt"] = json!(due);
            conn.execute("UPDATE scheduled_tasks SET payload = ?1 WHERE rowid = ?2", rusqlite::params![task.to_string(), rowid])
                .expect("bring the task forward");
        }
    }

    /// Wait until no run is going on for this session, so the next message is
    /// taken by an idle process.
    async fn wait_idle(&self, source: &str, id: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let (_, detail) = self.send("GET", &format!("/api/sessions/{source}/{id}"), Value::Null).await;
            if detail["injection"]["state"].as_str().is_some_and(|state| state != "running") {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{source}/{id} never finished its turn: {detail}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// The `nth` spawn of the fake `claude` in `dir`, once its dumps are whole.
async fn claude_spawn(dir: &Path, nth: usize) -> Spawn {
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    loop {
        if support::call_count(dir) >= nth {
            if let Some(spawn) = Spawn::read(dir) {
                return spawn;
            }
        }
        assert!(std::time::Instant::now() < deadline, "spawn {nth} was never recorded in {}", dir.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn start_claude(server: &Server, account: Value) -> Spawn {
    let workspace = server.workspace();
    let mut body = json!({"source": "claude", "cwd": workspace.to_string_lossy(), "text": "start"});
    if let Some(account) = account.as_object() {
        for (key, value) in account {
            body[key] = value.clone();
        }
    }
    server.send_ok("POST", "/api/sessions/new", body, 200).await;
    let spawn = claude_spawn(&workspace, 1).await;
    assert!(spawn.is_resident(), "a claude session is one resident process: {:?}", spawn.argv);
    spawn
}

/// `accountId: null`: the machine's own login, so nothing of the server's
/// managed root and no credential at all — none of the planted ones either.
#[tokio::test]
async fn a_claude_start_on_the_local_login_gets_the_server_environment_and_nothing_else() {
    let server = Server::start().await;
    let spawn = start_claude(&server, json!({"accountId": null})).await;
    assert_env(&spawn, server.claude_env(&[]), "local login");
}

#[tokio::test]
async fn a_claude_start_on_an_oauth_account_gets_the_managed_root_and_the_token() {
    let server = Server::start().await;
    let account = server.create(json!({"label": "Work", "kind": "oauth_token", "secret": OAUTH_SECRET})).await;
    let spawn = start_claude(&server, json!({"accountId": account})).await;
    let root = server.managed_root();
    assert_env(
        &spawn,
        server.claude_env(&[("CLAUDE_CONFIG_DIR", &root), ("CLAUDE_CODE_OAUTH_TOKEN", OAUTH_SECRET)]),
        "oauth account",
    );
}

/// A block is handed over as it stands, on top of the managed root.
#[tokio::test]
async fn a_claude_start_on_an_env_account_gets_the_managed_root_and_the_block_as_typed() {
    let server = Server::start().await;
    let account = server.create(json!({"label": "Gateway", "kind": "env", "env": CLAUDE_BLOCK})).await;
    let spawn = start_claude(&server, json!({"accountId": account})).await;
    let root = server.managed_root();
    assert_env(
        &spawn,
        server.claude_env(&[
            ("CLAUDE_CONFIG_DIR", &root),
            ("ANTHROPIC_AUTH_TOKEN", "spawn-env-gateway-token"),
            ("ANTHROPIC_BASE_URL", "https://gateway.example.com"),
        ]),
        "env account",
    );
}

/// Seed a session in the server's own root and bind it to `account`.
async fn bound_session(server: &Server, account: &str) -> PathBuf {
    let workspace = server.workspace();
    support::common::seed_transcript(
        &support::owned_claude_root(server.home()),
        "-workspace",
        OWNED_ID,
        &[json!({
            "type": "user",
            "cwd": workspace.to_string_lossy(),
            "timestamp": "2024-05-01T10:00:00.000Z",
            "message": {"content": [{"type": "text", "text": "hello"}]},
        })],
    );
    server
        .send_ok("PUT", &format!("/api/sessions/claude/{OWNED_ID}/account"), json!({"accountId": account}), 200)
        .await;
    workspace
}

async fn resume_claude(server: &Server, workspace: &Path) -> Spawn {
    server
        .send_ok("POST", &format!("/api/sessions/claude/{OWNED_ID}/message"), json!({"text": "carry on"}), 200)
        .await;
    let spawn = claude_spawn(workspace, 1).await;
    assert_eq!(spawn.flag("--resume"), Some(OWNED_ID));
    spawn
}

/// A resume finds the account the session is bound to, and builds the same
/// environment a start under it would.
#[tokio::test]
async fn a_claude_resume_of_a_session_bound_to_an_env_account_gets_its_block() {
    let server = Server::start().await;
    let account = server.create(json!({"label": "Gateway", "kind": "env", "env": CLAUDE_BLOCK})).await;
    let workspace = bound_session(&server, &account).await;
    let spawn = resume_claude(&server, &workspace).await;
    let root = server.managed_root();
    assert_env(
        &spawn,
        server.claude_env(&[
            ("CLAUDE_CONFIG_DIR", &root),
            ("ANTHROPIC_AUTH_TOKEN", "spawn-env-gateway-token"),
            ("ANTHROPIC_BASE_URL", "https://gateway.example.com"),
        ]),
        "env-bound resume",
    );
}

#[tokio::test]
async fn a_claude_resume_of_a_session_bound_to_an_oauth_account_gets_its_token() {
    let server = Server::start().await;
    let account = server.create(json!({"label": "Work", "kind": "oauth_token", "secret": OAUTH_SECRET})).await;
    let workspace = bound_session(&server, &account).await;
    let spawn = resume_claude(&server, &workspace).await;
    let root = server.managed_root();
    assert_env(
        &spawn,
        server.claude_env(&[("CLAUDE_CONFIG_DIR", &root), ("CLAUDE_CODE_OAUTH_TOKEN", OAUTH_SECRET)]),
        "oauth-bound resume",
    );
}

/// An owned session with no account runs under the managed root's own login.
#[tokio::test]
async fn a_claude_resume_of_an_owned_session_with_no_account_gets_the_managed_root_only() {
    let server = Server::start().await;
    let workspace = server.workspace();
    support::common::seed_transcript(
        &support::owned_claude_root(server.home()),
        "-workspace",
        OWNED_ID,
        &[json!({
            "type": "user",
            "cwd": workspace.to_string_lossy(),
            "timestamp": "2024-05-01T10:00:00.000Z",
            "message": {"content": [{"type": "text", "text": "hello"}]},
        })],
    );
    let spawn = resume_claude(&server, &workspace).await;
    let root = server.managed_root();
    assert_env(&spawn, server.claude_env(&[("CLAUDE_CONFIG_DIR", &root)]), "owned resume");
}

/// The `nth` App Server's command line and environment, and the
/// `thread/start` or `thread/resume` request it was sent.
async fn codex_spawn(dir: &Path, nth: usize) -> (Spawn, Value) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let spawns = std::fs::read_to_string(dir.join("codex-calls.txt")).unwrap_or_default().lines().count();
        let requests: Vec<Value> = std::fs::read_to_string(dir.join("codex-stdin.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|request: &Value| request["method"] == "thread/start" || request["method"] == "thread/resume")
            .collect();
        if spawns >= nth && requests.len() >= nth {
            let spawn = Spawn::read_codex(dir).expect("codex spawn");
            return (spawn, requests[nth - 1].clone());
        }
        assert!(std::time::Instant::now() < deadline, "app server {nth} never started in {}", dir.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn start_codex(server: &Server, account: Option<&str>) -> (Spawn, Value) {
    let workspace = server.workspace();
    let mut body = json!({"source": "codex", "cwd": workspace.to_string_lossy(), "text": "start", "permission": "read-only"});
    if let Some(account) = account {
        body["accountId"] = json!(account);
    }
    server.send_ok("POST", "/api/sessions/new", body, 200).await;
    codex_spawn(&workspace, 1).await
}

/// The `-c` overrides an account's model provider adds after `app-server`.
fn provider_argv(base_url: &str) -> Vec<String> {
    vec![
        "app-server".to_string(),
        "-c".to_string(),
        format!(
            "model_providers.remote_hand={{name=\"remote-hand\",base_url=\"{base_url}\",env_key=\"DROPPEN_CODEX_API_KEY\",wire_api=\"responses\"}}"
        ),
        "-c".to_string(),
        "model_provider=remote_hand".to_string(),
    ]
}

/// With no account, exactly the spawn there was before accounts existed:
/// `codex app-server`, the server's environment, and codex's own login.
#[tokio::test]
async fn a_codex_start_with_no_account_is_the_bare_app_server() {
    let server = Server::start().await;
    let (spawn, start) = start_codex(&server, None).await;
    assert_eq!(spawn.argv, ["app-server"]);
    assert_env(&spawn, server.clean_env(&[]), "codex, no account");
    assert_eq!(start["method"], "thread/start");
    assert_eq!(start["params"], json!({"cwd": server.workspace(), "approvalPolicy": "never"}));
}

/// An account adds a model provider whose key is read from a variable of the
/// server's, because `app-server` reads no key variable of codex's own. The
/// block's own `OPENAI_API_KEY` wins over the one the server inherited.
#[tokio::test]
async fn a_codex_start_on_an_env_account_gets_the_block_its_key_and_a_provider_of_its_own() {
    let server = Server::start().await;
    let account = server.create(json!({"label": "Relay", "kind": "env", "env": CODEX_BLOCK, "provider": "codex"})).await;
    let (spawn, start) = start_codex(&server, Some(&account)).await;
    assert_eq!(spawn.argv, provider_argv("https://relay.example.com/v1"));
    assert_env(
        &spawn,
        server.clean_env(&[
            ("OPENAI_API_KEY", "spawn-env-codex-key"),
            ("OPENAI_BASE_URL", "https://relay.example.com/v1"),
            ("DROPPEN_CODEX_API_KEY", "spawn-env-codex-key"),
        ]),
        "codex env account",
    );
    assert_eq!(
        start["params"],
        json!({"cwd": server.workspace(), "approvalPolicy": "never", "modelProvider": "remote_hand"})
    );
}

/// `CODEX_API_KEY` is the fallback key, read out of the block and never out of
/// the server's own `OPENAI_API_KEY`; a block naming no endpoint gets the
/// vendor's.
#[tokio::test]
async fn a_codex_block_with_only_the_fallback_key_gets_it_and_the_default_endpoint() {
    let server = Server::start().await;
    let account = server
        .create(json!({"label": "Fallback", "kind": "env", "env": CODEX_FALLBACK_BLOCK, "provider": "codex"}))
        .await;
    let (spawn, _) = start_codex(&server, Some(&account)).await;
    assert_eq!(spawn.argv, provider_argv("https://api.openai.com/v1"));
    assert_env(
        &spawn,
        server.clean_env(&[
            ("CODEX_API_KEY", "spawn-env-codex-fallback"),
            ("DROPPEN_CODEX_API_KEY", "spawn-env-codex-fallback"),
        ]),
        "codex fallback key",
    );
}

/// The rollout codex keeps of a thread, which is how the server finds the
/// thread again and the directory it runs in; the fake App Server writes none.
fn seed_rollout(home: &Path, id: &str, cwd: &Path) {
    let dir = home.join(".codex/sessions/2026/05/09");
    std::fs::create_dir_all(&dir).expect("rollout dir");
    let body = [
        json!({"type": "session_meta", "timestamp": "2026-05-09T16:39:49.000Z", "payload": {"id": id, "cwd": cwd.to_string_lossy()}}),
        json!({
            "type": "response_item",
            "timestamp": "2026-05-09T16:39:52.000Z",
            "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hello"}]},
        }),
    ]
    .iter()
    .map(|line| format!("{line}\n"))
    .collect::<String>();
    std::fs::write(dir.join(format!("rollout-2026-05-09T16-39-49-{id}.jsonl")), body).expect("seed rollout");
}

/// A message naming an account on a thread whose App Server runs without one
/// retires that process and resumes the thread under the account.
#[tokio::test]
async fn a_codex_resume_on_an_env_account_restarts_the_app_server_under_it() {
    let server = Server::start().await;
    let account = server.create(json!({"label": "Relay", "kind": "env", "env": CODEX_BLOCK, "provider": "codex"})).await;
    let workspace = server.workspace();
    seed_rollout(server.home(), CODEX_THREAD, &workspace);
    let (first, _) = start_codex(&server, None).await;
    assert_eq!(first.argv, ["app-server"]);
    server.wait_idle("codex", CODEX_THREAD).await;

    server
        .send_ok(
            "POST",
            &format!("/api/sessions/codex/{CODEX_THREAD}/message"),
            json!({"text": "carry on", "accountId": account}),
            200,
        )
        .await;
    let (spawn, resume) = codex_spawn(&workspace, 2).await;
    assert_eq!(spawn.argv, provider_argv("https://relay.example.com/v1"));
    assert_env(
        &spawn,
        server.clean_env(&[
            ("OPENAI_API_KEY", "spawn-env-codex-key"),
            ("OPENAI_BASE_URL", "https://relay.example.com/v1"),
            ("DROPPEN_CODEX_API_KEY", "spawn-env-codex-key"),
        ]),
        "codex resume",
    );
    assert_eq!(resume["method"], "thread/resume");
    assert_eq!(resume["params"]["threadId"], CODEX_THREAD);
    assert_eq!(resume["params"]["modelProvider"], "remote_hand");
}

/// A chat names no account, so it runs on the default one, with the
/// environment an explicit start under that account gets. The default is made
/// after a pool account, so the first account is not mistaken for it.
#[tokio::test]
async fn a_claude_chat_runs_on_the_default_account() {
    let server = Server::start().await;
    server.create(json!({"label": "Gateway", "kind": "env", "env": CLAUDE_BLOCK, "rotation": true})).await;
    let account = server.create(json!({"label": "Work", "kind": "oauth_token", "secret": OAUTH_SECRET})).await;
    server.make_default(&account, "claude").await;
    server.send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi"}), 200).await;
    let spawn = claude_spawn(&server.chats_dir(), 1).await;
    let root = server.managed_root();
    assert_env(
        &spawn,
        server.claude_env(&[("CLAUDE_CONFIG_DIR", &root), ("CLAUDE_CODE_OAUTH_TOKEN", OAUTH_SECRET)]),
        "claude chat on the default account",
    );
}

#[tokio::test]
async fn a_codex_chat_runs_on_the_default_account() {
    let server = Server::start().await;
    server.create(json!({"label": "Fallback", "kind": "env", "env": CODEX_FALLBACK_BLOCK, "provider": "codex"})).await;
    let account = server.create(json!({"label": "Relay", "kind": "env", "env": CODEX_BLOCK, "provider": "codex"})).await;
    server.make_default(&account, "codex").await;
    server.send_ok("POST", "/api/chats/new", json!({"backend": "codex", "text": "hi"}), 200).await;
    let (spawn, _) = codex_spawn(&server.chats_dir(), 1).await;
    assert_eq!(spawn.argv, provider_argv("https://relay.example.com/v1"));
    assert_env(
        &spawn,
        server.clean_env(&[
            ("OPENAI_API_KEY", "spawn-env-codex-key"),
            ("OPENAI_BASE_URL", "https://relay.example.com/v1"),
            ("DROPPEN_CODEX_API_KEY", "spawn-env-codex-key"),
        ]),
        "codex chat on the default account",
    );
}

/// A scheduled task names no account either, and runs on the default one.
#[tokio::test]
async fn a_scheduled_claude_task_runs_on_the_default_account() {
    let server = Server::start().await;
    server.create(json!({"label": "Work", "kind": "oauth_token", "secret": OAUTH_SECRET})).await;
    let account = server.create(json!({"label": "Gateway", "kind": "env", "env": CLAUDE_BLOCK})).await;
    server.make_default(&account, "claude").await;
    let workspace = server.workspace();
    server
        .send_ok(
            "POST",
            "/api/scheduled-tasks",
            json!({"source": "claude", "expr": "* * * * *", "cwd": workspace.to_string_lossy(), "prompt": "tick"}),
            200,
        )
        .await;
    server.make_tasks_due();
    let spawn = claude_spawn(&workspace, 1).await;
    let root = server.managed_root();
    assert_env(
        &spawn,
        server.claude_env(&[
            ("CLAUDE_CONFIG_DIR", &root),
            ("ANTHROPIC_AUTH_TOKEN", "spawn-env-gateway-token"),
            ("ANTHROPIC_BASE_URL", "https://gateway.example.com"),
        ]),
        "scheduled task on the default account",
    );
}

/// `accountFrom: "start"`: an account the start rule named, checked again at
/// the spawn. The default rests on opus, so a start naming no model goes to
/// the pool account, and one asking for sonnet stays on the default.
///
/// The rule names the pool account, so a start it named runs there, even
/// asking for sonnet. A third account it does not name, so a start it named
/// there goes where sonnet lands now: the default.
#[tokio::test]
async fn a_claude_start_the_rule_named_runs_where_the_rule_puts_it() {
    let server = Server::start().await;
    let pool = server.create(json!({"label": "Gateway", "kind": "env", "env": CLAUDE_BLOCK, "rotation": true})).await;
    let default = server.create(json!({"label": "Work", "kind": "oauth_token", "secret": OAUTH_SECRET})).await;
    let other = server
        .create(json!({"label": "Other", "kind": "env", "env": "ANTHROPIC_AUTH_TOKEN=other-gateway-token\nANTHROPIC_BASE_URL=https://other.example.com"}))
        .await;
    server.make_default(&default, "claude").await;
    server.rest(&default, "opus");
    let root = server.managed_root();
    let on_pool = server.claude_env(&[
        ("CLAUDE_CONFIG_DIR", &root),
        ("ANTHROPIC_AUTH_TOKEN", "spawn-env-gateway-token"),
        ("ANTHROPIC_BASE_URL", "https://gateway.example.com"),
    ]);

    let spawn = start_claude(&server, json!({"accountId": pool, "accountFrom": "start", "model": "sonnet"})).await;
    assert_env(&spawn, on_pool, "the account the rule still names");

    let elsewhere = server.home().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("elsewhere");
    server
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({
                "source": "claude", "cwd": elsewhere.to_string_lossy(), "text": "start", "model": "sonnet",
                "accountId": other, "accountFrom": "start",
            }),
            200,
        )
        .await;
    assert_env(
        &claude_spawn(&elsewhere, 1).await,
        server.claude_env(&[("CLAUDE_CONFIG_DIR", &root), ("CLAUDE_CODE_OAUTH_TOKEN", OAUTH_SECRET)]),
        "an account the rule does not name",
    );
}

#[tokio::test]
async fn a_codex_start_the_rule_named_runs_on_it() {
    let server = Server::start().await;
    server.create(json!({"label": "Fallback", "kind": "env", "env": CODEX_FALLBACK_BLOCK, "provider": "codex"})).await;
    let account = server.create(json!({"label": "Relay", "kind": "env", "env": CODEX_BLOCK, "provider": "codex"})).await;
    server.make_default(&account, "codex").await;
    let workspace = server.workspace();
    server
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({
                "source": "codex", "cwd": workspace.to_string_lossy(), "text": "start", "permission": "read-only",
                "accountId": account, "accountFrom": "start",
            }),
            200,
        )
        .await;
    let (spawn, _) = codex_spawn(&workspace, 1).await;
    assert_eq!(spawn.argv, provider_argv("https://relay.example.com/v1"));
    assert_env(
        &spawn,
        server.clean_env(&[
            ("OPENAI_API_KEY", "spawn-env-codex-key"),
            ("OPENAI_BASE_URL", "https://relay.example.com/v1"),
            ("DROPPEN_CODEX_API_KEY", "spawn-env-codex-key"),
        ]),
        "codex, the account the rule named",
    );
}

/// What the stand-in ACP agent in `dir` was started with.
async fn acp_spawn(dir: &Path) -> Spawn {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(raw) = std::fs::read_to_string(dir.join("agent-spawn.json")) {
            let spawned: Value = serde_json::from_str(&raw).expect("the agent's dump");
            let strings = |value: &Value| value.as_str().expect("a string").to_string();
            return Spawn {
                argv: spawned["argv"].as_array().expect("argv").iter().map(strings).collect(),
                env: spawned["env"].as_object().expect("env").iter().map(|(key, value)| (key.clone(), strings(value))).collect(),
            };
        }
        assert!(std::time::Instant::now() < deadline, "the agent never started in {}", dir.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// An ACP agent gets the server's environment and the variables its settings
/// give it, and no account's.
#[tokio::test]
async fn an_acp_start_gets_the_server_environment_and_the_agents_own_variables() {
    let server = Server::start().await;
    let program = server.home().join("bin/fake-agent");
    std::fs::create_dir_all(program.parent().expect("bin")).expect("bin dir");
    std::fs::write(&program, FAKE_ACP_AGENT).expect("fake agent");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    server
        .send_ok(
            "PUT",
            "/api/acp/agents/fake",
            json!({"command": program.to_string_lossy(), "args": ["--acp", "--stdio"], "env": "FAKE_AGENT_MARK=pinned"}),
            200,
        )
        .await;
    let workspace = server.workspace();
    server
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "acp", "acpAgent": "fake", "cwd": workspace.to_string_lossy(), "text": "start"}),
            200,
        )
        .await;
    let spawn = acp_spawn(&workspace).await;
    assert_eq!(spawn.argv, ["--acp", "--stdio"]);
    assert_env(&spawn, server.clean_env(&[("FAKE_AGENT_MARK", "pinned")]), "acp agent");
}

/// With the notify channel on, a claude start gets `droppen-notify` ahead of
/// its `PATH`, this server's address and a token for the session, and the
/// skill goes into the managed root it runs in. A start on the local login gets
/// the same variables and no skill: that root is the user's own. Codex is not
/// decorated.
#[tokio::test]
async fn a_start_with_the_notify_channel_on_gets_the_channel_on_claude_only() {
    let server = Server::start().await;
    server.notify_on().await;
    let account = server.create(json!({"label": "Work", "kind": "oauth_token", "secret": OAUTH_SECRET})).await;
    let workspace = server.workspace();
    let started = server
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "claude", "cwd": workspace.to_string_lossy(), "text": "start", "accountId": account}),
            200,
        )
        .await;
    let id = started["id"].as_str().expect("the session id");
    let spawn = claude_spawn(&workspace, 1).await;
    let root = server.managed_root();
    let expected = server.claude_env(&[("CLAUDE_CONFIG_DIR", &root), ("CLAUDE_CODE_OAUTH_TOKEN", OAUTH_SECRET)]);
    assert_env(&spawn, server.with_notify(expected, id).await, "claude start, notify on");
    assert!(Path::new(&root).join("skills/droppen-notify/SKILL.md").exists(), "the skill is in the managed root");

    let local = server.home().join("local");
    std::fs::create_dir_all(&local).expect("local");
    let started = server
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "claude", "cwd": local.to_string_lossy(), "text": "start", "accountId": null}),
            200,
        )
        .await;
    let id = started["id"].as_str().expect("the session id");
    let spawn = claude_spawn(&local, 1).await;
    assert_env(&spawn, server.with_notify(server.claude_env(&[]), id).await, "claude local login, notify on");
    assert!(!server.home().join(".claude/skills/droppen-notify").exists(), "nothing is written into the user's own root");

    let codex = server.home().join("codex");
    std::fs::create_dir_all(&codex).expect("codex");
    server
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "codex", "cwd": codex.to_string_lossy(), "text": "start", "permission": "read-only"}),
            200,
        )
        .await;
    let (spawn, _) = codex_spawn(&codex, 1).await;
    assert_env(&spawn, server.clean_env(&[]), "codex, notify on");
}

/// A resume is decorated as a start is, with a token for the session it
/// resumes.
#[tokio::test]
async fn a_claude_resume_with_the_notify_channel_on_gets_the_channel() {
    let server = Server::start().await;
    server.notify_on().await;
    let account = server.create(json!({"label": "Work", "kind": "oauth_token", "secret": OAUTH_SECRET})).await;
    let workspace = bound_session(&server, &account).await;
    let spawn = resume_claude(&server, &workspace).await;
    let root = server.managed_root();
    let expected = server.claude_env(&[("CLAUDE_CONFIG_DIR", &root), ("CLAUDE_CODE_OAUTH_TOKEN", OAUTH_SECRET)]);
    assert_env(&spawn, server.with_notify(expected, OWNED_ID).await, "claude resume, notify on");
    assert!(Path::new(&root).join("skills/droppen-notify/SKILL.md").exists(), "the skill is in the managed root");
}
