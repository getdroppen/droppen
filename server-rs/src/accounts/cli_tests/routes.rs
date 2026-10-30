//! The account routes, against the whole server state and the logins the
//! entry builds.

use std::sync::Arc;

use gotcha::axum::extract::FromRef;
use gotcha::State;

use droppen_accounts::store::{Account, AccountKind, AccountProvider, Credential};
use droppen_accounts::*;
use crate::api::AppState;
use droppen_drive::routes::{bind_session_account, continue_session_on_account, ContinuedSession, SessionAccount, SessionContinue};
use crate::provider::claude_code::sessions::claude;
use droppen_http::error::ApiError;
use droppen_http::extract::{ApiJson, ApiPath};
use droppen_http::openapi_ext::{Nullable, Optional};
use droppen_instance::paths::ClaudeRoot;
use droppen_sessions::Source;

/// A credential its login would check as the other CLI's is refused with the
/// mismatch body, hint included. `PATCH` checks a replacement through the same
/// step as `POST`.
#[tokio::test]
async fn a_credential_checked_as_the_other_clis_gets_the_mismatch_body() {
    // Nothing on `PATH`, so no CLI could start even if the refusal did not
    // come first. The override is this test thread's own (`process_env`).
    let empty = tempfile::tempdir().expect("a PATH directory with nothing in it");
    droppen_base::process_env::set_child_path(Some(empty.path().to_string_lossy().into_owned()));
    let home = tempfile::tempdir().expect("tempdir");
    let app = crate::api::test_support::state(home.path()).await;
    let mut state = AccountsState::from_ref(&app);
    let real = crate::app::logins();
    let swapped = login::Logins {
        claude: real.codex.clone(),
        codex: real.claude.clone(),
    };
    state.accounts = Arc::new(store::AccountsStore::new(state.paths.clone(), swapped));
    let body = AccountCreate {
        label: "Work".into(),
        kind: AccountKind::Env,
        secret: Optional(None),
        env: Optional(Some("OPENAI_API_KEY=not-a-real-openai-key".into())),
        provider: Optional(Some(AccountProvider::Claude)),
        rotation: Optional(None),
    };

    let error = create_account(State(state), ApiJson(body)).await.err().expect("refused");
    let pinned = provider_mismatch();
    assert_eq!(error.status(), pinned.status());
    assert_eq!(error.body().to_string(), pinned.body().to_string(), "the hint included");
}

const STUCK: &str = "11111111-1111-4111-8111-111111111111";

/// A claude session this server started on the local login, in the user's
/// own `~/.claude`, with a subagent transcript beside it.
async fn local_session(paths: &droppen_instance::paths::Paths, id: &str, recorded: bool) -> std::path::PathBuf {
    let project = crate::provider::claude_code::claude_roots::ClaudeRoots::of(paths).projects_dir(ClaudeRoot::Local).join("-w");
    std::fs::create_dir_all(project.join(id).join("subagents")).expect("sidecar");
    let path = project.join(format!("{id}.jsonl"));
    std::fs::write(
        &path,
        format!(
            "{{\"type\":\"user\",\"cwd\":\"/w\",\"sessionId\":\"{id}\",\"timestamp\":\"2026-10-19T05:11:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"fix the inbox\"}}}}\n\
             {{\"type\":\"assistant\",\"sessionId\":\"{id}\",\"isApiErrorMessage\":true,\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"Failed to authenticate\"}}]}}}}\n"
        ),
    )
    .expect("transcript");
    std::fs::write(
        project.join(id).join("subagents").join("agent-1.jsonl"),
        format!("{{\"sessionId\":\"{id}\"}}\n"),
    )
    .expect("subagent");
    if recorded {
        crate::provider::claude_code::sessions::claude::ClaudeCodeSessions.record_owned(paths, id).await.expect("record");
    }
    path
}

async fn claude_account(state: &AppState, label: &str) -> Account {
    state
        .drive
        .accounts
        .create(
            label,
            AccountProvider::Claude,
            Credential::Oauth(format!("sk-ant-oat01-{}", "a".repeat(80))),
        )
        .await
        .expect("account")
}

async fn continue_on(state: &AppState, source: Source, id: &str, account_id: &str) -> Result<ContinuedSession, ApiError> {
    continue_session_on_account(
        State(droppen_drive::DriveState::from_ref(state)),
        ApiPath((source, id.to_string())),
        ApiJson(SessionContinue {
            account_id: account_id.to_string(),
        }),
    )
    .await
    .map(|json| json.0)
}

/// The trap this route exists for: a session on a signed-out local login
/// cannot be bound to an account, so it is continued on one instead — a new
/// session in this instance's root, bound to the account, holding the
/// conversation, while the original stays exactly where and what it was.
#[tokio::test]
async fn a_local_login_session_continues_on_an_account_as_a_new_session() {
    let _guard = droppen_sessions::index::exclusive().await;
    let home = tempfile::tempdir().expect("tempdir");
    let state = crate::api::test_support::state(home.path()).await;
    let paths = state.drive.paths.clone();
    let original = local_session(&paths, STUCK, true).await;
    let before = std::fs::read(&original).expect("read");
    let work = claude_account(&state, "work").await;

    // Binding is still refused, and now says what to do instead.
    let refused = bind_session_account(
        State(droppen_drive::DriveState::from_ref(&state)),
        ApiPath((Source::Claude, STUCK.to_string())),
        ApiJson(SessionAccount {
            account_id: Nullable(Some(work.id.clone())),
        }),
    )
    .await
    .expect_err("a local session cannot be bound");
    assert_eq!(refused.body()["error"], "local_session_cannot_use_account");
    assert!(refused.body()["hint"].as_str().unwrap_or_default().contains("Continue it on the account"));

    let continued = continue_on(&state, Source::Claude, STUCK, &work.id).await.expect("continued");
    assert_eq!(continued.source, Source::Claude);
    assert_ne!(continued.id, STUCK, "a new session, not the old one moved");
    assert!(droppen_sessions::reference::is_path_segment_id(&continued.id));

    assert_eq!(std::fs::read(&original).expect("the original"), before, "the original is untouched");
    let copy = crate::provider::claude_code::sessions::claude::find_in_root(&paths, ClaudeRoot::Managed, &continued.id)
        .await
        .expect("the copy is in this instance's root");
    let text = std::fs::read_to_string(&copy).expect("copy");
    assert!(
        text.contains(&format!("\"sessionId\":\"{}\"", continued.id)) && !text.contains(STUCK),
        "{text}"
    );
    assert!(copy
        .parent()
        .expect("project")
        .join(&continued.id)
        .join("subagents")
        .join("agent-1.jsonl")
        .is_file());
    assert_eq!(
        state
            .drive
            .accounts
            .session_account_id_for(&continued.id, AccountProvider::Claude)
            .await
            .as_deref(),
        Some(work.id.as_str()),
        "bound to the account, so its first message resumes there"
    );

    // Both list, each as what it is.
    let new_row = claude::load_claude_summary(&paths, &continued.id).await.expect("the new session lists");
    assert_eq!(
        (new_row.scope, new_row.claude_root),
        (droppen_sessions::SessionScope::Owned, Some(ClaudeRoot::Managed))
    );
    assert_eq!(new_row.title, "fix the inbox", "the same conversation");
    let old_row = claude::load_claude_summary(&paths, STUCK).await.expect("the original still lists");
    assert_eq!(
        (old_row.scope, old_row.claude_root),
        (droppen_sessions::SessionScope::Owned, Some(ClaudeRoot::Local))
    );

    // The copy is in the managed root: it switches accounts in place, and
    // continuing it again is refused.
    let again = continue_on(&state, Source::Claude, &continued.id, &work.id)
        .await
        .expect_err("already managed");
    assert_eq!(
        (again.status().as_u16(), again.body()["error"].as_str()),
        (409, Some("session_not_on_local_login"))
    );
}

/// Only this instance's own local-login sessions, only claude, only a real
/// claude account — and nothing written on any refusal.
#[tokio::test]
async fn continuing_refuses_what_is_not_this_servers_to_copy() {
    let _guard = droppen_sessions::index::exclusive().await;
    let home = tempfile::tempdir().expect("tempdir");
    let state = crate::api::test_support::state(home.path()).await;
    let paths = state.drive.paths.clone();
    let work = claude_account(&state, "work").await;
    let codex = state
        .drive
        .accounts
        .create(
            "codex",
            AccountProvider::Codex,
            Credential::Env(droppen_base::env_block::parse_env_block("OPENAI_API_KEY=sk-test-abcdefghijklmnop").expect("env block")),
        )
        .await
        .expect("codex account");

    // The user's own session, started in a terminal: external, browse-only.
    let theirs = "33333333-3333-4333-8333-333333333333";
    local_session(&paths, theirs, false).await;
    let error = continue_on(&state, Source::Claude, theirs, &work.id).await.expect_err("external");
    assert_eq!((error.status().as_u16(), error.body()["error"].as_str()), (409, Some("external_session")));

    local_session(&paths, STUCK, true).await;
    let error = continue_on(&state, Source::Codex, STUCK, &work.id).await.expect_err("codex");
    assert_eq!(
        (error.status().as_u16(), error.body()["error"].as_str()),
        (400, Some("continue_not_supported"))
    );
    let error = continue_on(&state, Source::Claude, STUCK, &codex.id)
        .await
        .expect_err("the other CLI's account");
    assert_eq!(
        (error.status().as_u16(), error.body()["error"].as_str()),
        (400, Some("provider_mismatch"))
    );
    for account in ["not-a-uuid", "99999999-9999-4999-8999-999999999999"] {
        let error = continue_on(&state, Source::Claude, STUCK, account).await.expect_err("no such account");
        assert_eq!(error.status().as_u16(), 404, "{account}");
    }
    for id in ["44444444-4444-4444-8444-444444444444", "..", "../../etc/passwd", "-w/../x"] {
        let error = continue_on(&state, Source::Claude, id, &work.id).await.expect_err("no such session");
        assert_eq!(error.status().as_u16(), 404, "{id}");
    }
    assert!(
        !crate::provider::claude_code::claude_roots::ClaudeRoots::of(&paths).projects_dir(ClaudeRoot::Managed).join("-w").exists(),
        "not one copy was written"
    );
}

/// `GET /api/accounts` says what the local login is worth, so the picker can
/// stop offering a broken one as the default.
#[tokio::test]
async fn the_account_list_reports_the_local_logins_state() {
    use droppen_accounts::local_login::LocalLoginState;
    let home = tempfile::tempdir().expect("tempdir");
    let state = crate::api::test_support::state(home.path()).await;
    let listed = list_accounts(State(AccountsState::from_ref(&state))).await.0;
    assert_eq!(listed.local.claude.state, LocalLoginState::Unknown, "nothing known yet");

    state
        .drive
        .accounts
        .limit_log()
        .record(
            &rest::Observation {
                account_id: rest::LOCAL_CLAUDE.to_string(),
                session_id: Some(STUCK.into()),
                source: "claude:result",
                model: None,
                verdict: limits::Verdict {
                    class: droppen_sessions::account::LimitClass::Auth,
                    scope: "all".into(),
                    until_ms: None,
                },
                raw: "Failed to authenticate: OAuth session expired and could not be refreshed".into(),
                reason: String::new(),
                may_rest: true,
            },
            rest::now_ms(),
        )
        .expect("record");
    let listed = list_accounts(State(AccountsState::from_ref(&state))).await.0;
    assert_eq!(listed.local.claude.state, LocalLoginState::SignedOut);
    assert_eq!(listed.local.claude.rest.len(), 1);
    assert_eq!(listed.local.codex.state, LocalLoginState::Unknown);
    let wire = serde_json::to_value(&listed).expect("json");
    assert_eq!(wire["local"]["claude"]["state"], "signed_out", "spelled as the client reads it");
}
