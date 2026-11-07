//! What the wizards do on a machine with no `claude` installed.
//!
//! A binary of its own, holding exactly one test, because the only way to say
//! "the CLI is not installed" is to empty the process's `PATH` — and `PATH` is
//! process-global, so a second test running beside this one would see it too.
//! Both wizards are asserted here rather than in two binaries, since they share
//! the one precondition the file exists to set up.

use serde_json::{json, Value};

#[path = "common/mod.rs"]
mod common;

#[tokio::test]
async fn a_missing_cli_fails_the_request_instead_of_opening_a_dead_terminal() {
    let empty = tempfile::tempdir().expect("empty bin dir");
    std::env::set_var("PATH", empty.path());
    let server = common::TestServer::start().await;

    let (status, body) = server.json("POST", "/api/accounts/setup-token", &json!({ "label": "Nowhere" })).await;
    assert_eq!(status, 500, "{body}");
    let error = serde_json::from_str::<Value>(&body).expect("json");
    assert_eq!(
        error,
        json!({ "error": "the `claude` CLI is not on PATH, so the token wizard cannot start" })
    );

    let secret = format!("sk-ant-oat01-{}", "A".repeat(70));
    let account = common::create_account(&server, "Work", &secret).await;
    let id = account["id"].as_str().expect("id").to_string();
    let (status, body) = server.json("POST", &format!("/api/accounts/{id}/usage-login"), &json!({})).await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json"),
        json!({ "error": "the `claude` CLI is not on PATH, so the login wizard cannot start" })
    );

    // The failure came before any pty was opened, so there is nothing on screen
    // that printed "command not found" and died.
    let terminals = server.get_json("/api/terminals").await;
    assert_eq!(terminals["terminals"].as_array().expect("terminals").len(), 0, "{terminals}");
    server.stop().await;
}
