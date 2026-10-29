//! The whole environment, and the account's part of the argv, that each CLI is
//! started with, per provider and per kind of account.
//!
//! `tests/inject.rs` asserts on the variables an account adds. This file pins
//! everything else as well: a variable an account should not have set, or one
//! the server's own environment should have kept, fails here too. The expected
//! side is the server's own environment builder (`claude_env` / `clean_env`)
//! with the account's variables spelled out by hand, so the only thing these
//! tests take on trust is the strip rule those builders apply to every child.

mod support;

use std::collections::BTreeMap;

use droppen_base::process_env::{claude_env, clean_env};
use serde_json::{json, Value};
use support::{Probe, Spawn};

/// A token in the shape the store accepts; the fake CLI takes anything.
const OAUTH_SECRET: &str = "spawn-env-oauth-token";
/// A claude block the way a gateway's documentation writes one.
const CLAUDE_BLOCK: &str = "ANTHROPIC_AUTH_TOKEN=spawn-env-gateway-token\nANTHROPIC_BASE_URL=https://gateway.example.com";
/// A codex block with a key and an endpoint of its own.
const CODEX_BLOCK: &str = "OPENAI_API_KEY=spawn-env-codex-key\nOPENAI_BASE_URL=https://relay.example.com/v1";
/// A session in the root this server owns.
const OWNED_ID: &str = "cccccccc-3333-4333-8333-cccccccccccc";

/// What the fake CLIs' interpreters add or rewrite on the way in, which no
/// spawn controls: `sh` exports its working directory, its depth and the
/// command it ran, and Python coerces a C locale's `LC_CTYPE` to UTF-8.
const INTERPRETER_OWN: [&str; 5] = ["PWD", "OLDPWD", "SHLVL", "_", "LC_CTYPE"];

/// Whether `sh` can pass this name on at all: it drops a variable whose name
/// is not an identifier (cargo's `CARGO_BIN_EXE_remote-hand-server`).
fn is_shell_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn without_shell(env: impl IntoIterator<Item = (String, String)>) -> BTreeMap<String, String> {
    env.into_iter()
        .filter(|(key, _)| !INTERPRETER_OWN.contains(&key.as_str()) && is_shell_name(key))
        .collect()
}

/// Assert that the child got exactly `expected`, naming every difference.
fn assert_env(spawn: &Spawn, expected: BTreeMap<String, String>, what: &str) {
    let got = without_shell(spawn.env.clone());
    let expected = without_shell(expected);
    let mut diff = Vec::new();
    for (key, value) in &expected {
        match got.get(key) {
            Some(seen) if seen == value => {}
            Some(seen) => diff.push(format!("{key}: expected {value:?}, got {seen:?}")),
            None => diff.push(format!("{key}: expected {value:?}, missing")),
        }
    }
    for (key, value) in &got {
        if !expected.contains_key(key) {
            diff.push(format!("{key}: not expected, got {value:?}"));
        }
    }
    assert!(diff.is_empty(), "{what}: the child's environment differs:\n{}", diff.join("\n"));
}

fn pairs(vars: &[(&str, &str)]) -> Vec<(String, String)> {
    vars.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

async fn create(probe: &Probe, body: Value) -> String {
    let created = probe.send_ok("POST", "/api/accounts", body, 201).await;
    created["id"].as_str().expect("account id").to_string()
}

async fn start_claude(probe: &Probe, account: Value) -> Spawn {
    let workspace = probe.workspace();
    let mut body = json!({"source": "claude", "cwd": workspace.to_string_lossy(), "text": "start"});
    if let Some(account) = account.as_object() {
        for (key, value) in account {
            body[key] = value.clone();
        }
    }
    probe.send_ok("POST", "/api/sessions/new", body, 200).await;
    let spawn = probe.wait_for_spawn(&workspace).await;
    assert!(spawn.is_resident(), "a claude session is one resident process: {:?}", spawn.argv);
    spawn
}

/// `accountId: null`: the machine's own login, so nothing of the server's
/// managed root and no credential at all.
#[tokio::test]
async fn a_claude_start_on_the_local_login_gets_the_server_environment_and_nothing_else() {
    let probe = Probe::start().await;
    let spawn = start_claude(&probe, json!({"accountId": null})).await;
    assert_env(&spawn, claude_env(Vec::<(String, String)>::new()), "local login");
    probe.stop().await;
}

#[tokio::test]
async fn a_claude_start_on_an_oauth_account_gets_the_managed_root_and_the_token() {
    let probe = Probe::start().await;
    let account = create(&probe, json!({"label": "Work", "kind": "oauth_token", "secret": OAUTH_SECRET})).await;
    let spawn = start_claude(&probe, json!({"accountId": account})).await;
    let root = probe.managed_root().to_string_lossy().into_owned();
    assert_env(
        &spawn,
        claude_env(pairs(&[("CLAUDE_CONFIG_DIR", &root), ("CLAUDE_CODE_OAUTH_TOKEN", OAUTH_SECRET)])),
        "oauth account",
    );
    probe.stop().await;
}

/// A block is handed over as it stands, on top of the managed root.
#[tokio::test]
async fn a_claude_start_on_an_env_account_gets_the_managed_root_and_the_block_as_typed() {
    let probe = Probe::start().await;
    let account = create(&probe, json!({"label": "Gateway", "kind": "env", "env": CLAUDE_BLOCK})).await;
    let spawn = start_claude(&probe, json!({"accountId": account})).await;
    let root = probe.managed_root().to_string_lossy().into_owned();
    assert_env(
        &spawn,
        claude_env(pairs(&[
            ("CLAUDE_CONFIG_DIR", &root),
            ("ANTHROPIC_AUTH_TOKEN", "spawn-env-gateway-token"),
            ("ANTHROPIC_BASE_URL", "https://gateway.example.com"),
        ])),
        "env account",
    );
    probe.stop().await;
}

/// A resume finds the account the session is bound to, and builds the same
/// environment a start under it would.
#[tokio::test]
async fn a_claude_resume_of_a_bound_session_gets_its_accounts_environment() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let account = create(&probe, json!({"label": "Gateway", "kind": "env", "env": CLAUDE_BLOCK})).await;
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    probe
        .send_ok("PUT", &format!("/api/sessions/claude/{OWNED_ID}/account"), json!({"accountId": account}), 200)
        .await;
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{OWNED_ID}/message"), json!({"text": "carry on"}), 200)
        .await;
    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.flag("--resume"), Some(OWNED_ID));
    let root = probe.managed_root().to_string_lossy().into_owned();
    assert_env(
        &spawn,
        claude_env(pairs(&[
            ("CLAUDE_CONFIG_DIR", &root),
            ("ANTHROPIC_AUTH_TOKEN", "spawn-env-gateway-token"),
            ("ANTHROPIC_BASE_URL", "https://gateway.example.com"),
        ])),
        "bound resume",
    );
    probe.stop().await;
}

/// An owned session with no account runs under the managed root's own login.
#[tokio::test]
async fn a_claude_resume_of_an_owned_session_with_no_account_gets_the_managed_root_only() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.managed_root(), OWNED_ID, &workspace);
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{OWNED_ID}/message"), json!({"text": "carry on"}), 200)
        .await;
    let spawn = probe.wait_for_spawn(&workspace).await;
    let root = probe.managed_root().to_string_lossy().into_owned();
    assert_env(&spawn, claude_env(pairs(&[("CLAUDE_CONFIG_DIR", &root)])), "owned resume");
    probe.stop().await;
}

/// The App Server's command line, environment and `thread/start` parameters.
async fn start_codex(probe: &Probe, account: Option<&str>) -> (Spawn, Value) {
    let workspace = probe.workspace();
    let mut body = json!({"source": "codex", "cwd": workspace.to_string_lossy(), "text": "start", "permission": "read-only"});
    if let Some(account) = account {
        body["accountId"] = json!(account);
    }
    probe.send_ok("POST", "/api/sessions/new", body, 200).await;
    let spawn = Spawn::read_codex(&workspace).expect("codex spawn");
    let requests = std::fs::read_to_string(workspace.join("codex-stdin.jsonl")).expect("RPC log");
    let start = requests
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("RPC line"))
        .find(|request| request["method"] == "thread/start")
        .expect("thread/start");
    (spawn, start["params"].clone())
}

/// With no account, exactly the spawn there was before accounts existed:
/// `codex app-server`, the server's environment, and codex's own login.
#[tokio::test]
async fn a_codex_start_with_no_account_is_the_bare_app_server() {
    let probe = Probe::start().await;
    let (spawn, params) = start_codex(&probe, None).await;
    assert_eq!(spawn.argv, ["app-server"]);
    assert_env(&spawn, clean_env(Vec::<(String, String)>::new()), "codex, no account");
    assert_eq!(params, json!({"cwd": probe.workspace(), "approvalPolicy": "never"}));
    probe.stop().await;
}

/// An account adds a model provider whose key is read from a variable of the
/// server's, because `app-server` reads no key variable of codex's own.
#[tokio::test]
async fn a_codex_start_on_an_env_account_gets_the_block_its_key_and_a_provider_of_its_own() {
    let probe = Probe::start().await;
    let account = create(&probe, json!({"label": "Relay", "kind": "env", "env": CODEX_BLOCK, "provider": "codex"})).await;
    let (spawn, params) = start_codex(&probe, Some(&account)).await;
    assert_eq!(
        spawn.argv,
        [
            "app-server",
            "-c",
            "model_providers.remote_hand={name=\"remote-hand\",base_url=\"https://relay.example.com/v1\",env_key=\"DROPPEN_CODEX_API_KEY\",wire_api=\"responses\"}",
            "-c",
            "model_provider=remote_hand",
        ]
    );
    assert_env(
        &spawn,
        clean_env(pairs(&[
            ("OPENAI_API_KEY", "spawn-env-codex-key"),
            ("OPENAI_BASE_URL", "https://relay.example.com/v1"),
            ("DROPPEN_CODEX_API_KEY", "spawn-env-codex-key"),
        ])),
        "codex env account",
    );
    assert_eq!(
        params,
        json!({"cwd": probe.workspace(), "approvalPolicy": "never", "modelProvider": "remote_hand"})
    );
    probe.stop().await;
}
