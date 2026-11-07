//! A CLI-backed instant chat, from the route down to the argv.
//!
//! Separate from `tests/chats.rs` because this half needs the fake CLIs on
//! `PATH` (`tests/support`) while that half needs a fake HTTP provider, and the
//! two fixtures have nothing to say to each other.
//!
//! What is asserted is the whole reason a chat is not just a session: the spawn
//! happens in the anchor directory, with the coding persona replaced and every
//! tool disabled.

mod support;

use serde_json::json;

#[tokio::test]
async fn a_claude_chat_spawns_in_the_anchor_directory_with_the_persona() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");

    let result = probe
        .send_ok("POST", "/api/chats/new", json!({ "backend": "claude", "text": "hi" }), 200)
        .await;
    assert_eq!(result["kind"], "claude");
    assert!(result["id"].as_str().expect("id").len() == 36, "a fresh session uuid: {result}");

    let spawn = probe.wait_for_spawn(&anchor).await;
    assert!(spawn.is_resident(), "a chat is a resident like any other session: {:?}", spawn.argv);
    assert_eq!(
        support::wait_for_prompts(&anchor, 1).await,
        vec!["hi".to_string()],
        "the prompt reaches the CLI over its stdin, not in its argv"
    );
    assert_eq!(spawn.flag("--disallowedTools"), Some("*"), "a chat has no tools");
    assert!(
        spawn
            .flag("--system-prompt")
            .expect("a persona")
            .starts_with("You are a friendly general-purpose assistant in a casual chat app."),
        "the chat persona replaces the coding one"
    );

    // codex has no persona flag, so the anchor directory carries one for it.
    let persona = std::fs::read_to_string(anchor.join("AGENTS.md")).expect("AGENTS.md");
    assert!(persona.starts_with("# Instant chat\n"), "{persona}");
    assert!(persona.contains("NOT a coding session"), "{persona}");
    probe.stop().await;
}

/// A chat, too, can carry an account the start rule named (`accountFrom:
/// "start"`): checked again at the spawn, and placed afresh when the rule no
/// longer names it — never billed to an account only a stale answer named.
/// The same id without the marker is a person's pick.
#[tokio::test]
async fn a_chat_with_an_account_the_start_rule_named_is_checked_again_at_the_spawn() {
    let probe = support::Probe::start().await;
    let anchor = probe.data_dir().join("chats");
    let create = |label: &'static str, secret: &'static str| {
        let probe = &probe;
        async move { probe.create_account_id(label, secret).await }
    };
    const DEFAULT_SECRET: &str = "sk-ant-oat01-TESTCHATDEFAULT0123456789";
    const SPARE_SECRET: &str = "sk-ant-oat01-TESTCHATSPARE00123456789";
    let default = create("Default", DEFAULT_SECRET).await;
    let spare = create("Spare", SPARE_SECRET).await;
    probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": default}), 200).await;

    // A stale answer named the spare: the rule places a chat on the default now.
    probe
        .send_ok(
            "POST",
            "/api/chats/new",
            json!({"backend": "claude", "text": "hi", "accountId": spare, "accountFrom": "start"}),
            200,
        )
        .await;
    let stale = probe.wait_for_spawn_number(&anchor, 1).await;
    assert_eq!(
        stale.env("CLAUDE_CODE_OAUTH_TOKEN"),
        Some(DEFAULT_SECRET),
        "placed afresh, on the default — not on the account the stale answer named"
    );

    // The same id as a person's pick: theirs.
    probe
        .send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "accountId": spare}), 200)
        .await;
    let picked = probe.wait_for_spawn_number(&anchor, 2).await;
    assert_eq!(picked.env("CLAUDE_CODE_OAUTH_TOKEN"), Some(SPARE_SECRET));

    // A malformed marker is a bad request, not a pick.
    let (status, _) = probe
        .send("POST", "/api/chats/new", json!({"backend": "claude", "text": "hi", "accountId": spare, "accountFrom": "guess"}))
        .await;
    assert_eq!(status, 400);
    probe.stop().await;
}
