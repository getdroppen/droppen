//! The provider registry, over the providers the entry registers.

use std::sync::Arc;

use droppen_sessions::{SessionSummary, Source};

use droppen_provider::*;
use droppen_provider_acp::agents::AcpAgentUpdate;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;

/// Every provider over a fresh instance in `home`, as the entry registers
/// them, with one ACP agent configured: `fake`.
fn providers(home: &std::path::Path) -> Arc<ProviderRegistry> {
    providers_from(home, crate::app::register_providers)
}

/// The providers `register` adds, over a fresh instance in `home` with one
/// ACP agent configured: `fake`.
fn providers_from(home: &std::path::Path, register: fn(&mut ProviderRegistry, &Arc<InjectionRegistry>)) -> Arc<ProviderRegistry> {
    let paths = Arc::new(Paths::new(home.to_path_buf()));
    let dirs = InstanceDirs::new(&paths.data_dir(), "registry-test").expect("valid name");
    dirs.create().expect("instance dirs");
    let agent = AcpAgentUpdate {
        name: None.into(),
        command: Some("/x/agent".to_string()).into(),
        args: None.into(),
        env: None.into(),
    };
    droppen_provider_acp::testing::configure(&paths, "fake", &agent).expect("saved");
    let accounts = Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), crate::app::logins()));
    let quota = Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), crate::app::logins()));
    let decoration = droppen_provider::testing::no_decoration();
    ProviderRegistry::new(InjectionRegistry::new(paths, &dirs, accounts, quota, decoration), register)
}

/// Claude Code alone.
fn claude_only(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
    providers.register(Arc::new(droppen_provider_claude_code::resident::ClaudeCode::new(registry.clone())));
}

/// Codex alone.
fn codex_only(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
    providers.register(Arc::new(droppen_provider_codex::app_server::Codex::new(registry.clone())));
}

/// The ACP agents alone.
fn acp_only(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
    providers.register(Arc::new(droppen_provider_acp::Acp::new(registry.clone())));
}

/// A session row of `source` that ran on the ACP agent `agent`, if any.
fn row(source: &str, agent: Option<&str>) -> SessionSummary {
    serde_json::from_value(serde_json::json!({
        "pinned": false, "id": "s", "source": source, "scope": "owned", "server": "s", "title": "t",
        "cwd": null, "repoKey": null, "gitBranch": null, "updatedAt": "", "createdAt": "",
        "sizeBytes": 1, "filePath": "/x", "status": "idle", "activity": "idle", "resident": null,
        "claudeRoot": null, "accountId": null, "attentionAt": null, "unread": false, "unreadCount": 0,
        "acpAgent": agent,
    }))
    .unwrap_or_else(|error| panic!("a session row: {error}"))
}

/// Why `build` panicked, or `None` when it did not.
fn refusal(build: impl FnOnce()) -> Option<String> {
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)).err()?;
    Some(refused.downcast_ref::<String>().cloned().or_else(|| refused.downcast_ref::<&str>().map(|why| why.to_string())).unwrap_or_default())
}

/// A provider is registered only over the host its runs are on: the
/// registry lists and adopts into the runs its providers hold, and runs on
/// another host would be driven where none of that reaches.
#[test]
fn a_provider_whose_runs_are_on_another_host_is_refused() {
    let home = tempfile::tempdir().expect("tempdir");
    let providers = providers(home.path());
    for runs in providers.runs().values() {
        assert!(Arc::ptr_eq(runs.host(), &providers.injections().host));
    }
    let injections = providers.injections().clone();
    drop(providers);
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let foreign = providers_from(elsewhere.path(), |_, _| {}).injections().clone();
    let why = refusal(|| {
        ProviderRegistry::new(injections, |providers, _| providers.register(Arc::new(droppen_provider_codex::app_server::Codex::new(foreign))));
    });
    assert!(
        why.as_deref().is_some_and(|why| why.contains("the codex provider's runs are on another host")),
        "Codex over another host's injection registry: refused for another reason, or not at all: {why:?}"
    );
}

/// One provider registry per host, and one provider per source in it: a
/// second registry over the same injection registry is refused while the
/// first lasts and builds once it has gone, and a second provider for a
/// source is refused, whichever source it is.
#[test]
fn a_second_live_provider_registry_on_one_host_is_refused() {
    let home = tempfile::tempdir().expect("tempdir");
    let first = providers(home.path());
    let injections = first.injections().clone();
    let why = refusal(|| {
        ProviderRegistry::new(injections.clone(), crate::app::register_providers);
    });
    assert!(
        why.as_deref().is_some_and(|why| why.contains("a provider registry is over this host already")),
        "a second live provider registry on one host: refused for another reason, or not at all: {why:?}"
    );
    drop(first);
    let again = ProviderRegistry::new(injections.clone(), crate::app::register_providers);
    assert!(again.provider::<droppen_provider_acp::Acp>().is_some());
    drop(again);
    let register: [(Source, fn(&mut ProviderRegistry, &Arc<InjectionRegistry>)); 3] = [(Source::Claude, claude_only), (Source::Codex, codex_only), (Source::Acp, acp_only)];
    for (source, register) in register {
        let why = refusal(|| {
            ProviderRegistry::new(injections.clone(), |providers, injections| {
                register(providers, injections);
                register(providers, injections);
            });
        });
        assert!(
            why.as_deref().is_some_and(|why| why.contains(&format!("a {source} provider is registered already"))),
            "a second {source} provider in one registry: refused for another reason, or not at all: {why:?}"
        );
    }
}

/// Only Claude Code carries a session onto an account, and each source's
/// request reaches its own provider: Codex and the ACP agents refuse
/// before anything is read, and a registry without Claude Code refuses
/// Claude's too.
#[tokio::test]
async fn only_claude_code_continues_a_session_on_an_account() {
    let home = tempfile::tempdir().expect("tempdir");
    let providers = providers(home.path());
    let paths = Paths::new(home.path().to_path_buf());
    let local = droppen_provider_claude_code::claude_roots::ClaudeRoots::of(&paths).local_projects().join("-w");
    std::fs::create_dir_all(&local).expect("project dir");
    const STUCK: &str = "11111111-1111-4111-8111-111111111111";
    const NEW: &str = "22222222-2222-4222-8222-222222222222";
    std::fs::write(local.join(format!("{STUCK}.jsonl")), format!("{{\"sessionId\":\"{STUCK}\"}}\n")).expect("transcript");

    for source in [Source::Codex, Source::Acp] {
        let refused = providers.continue_on_account(source, STUCK, NEW).await;
        assert!(matches!(refused, Err(droppen_provider::ContinueError::Unsupported)), "{source}: {refused:?}");
    }
    let other = tempfile::tempdir().expect("tempdir");
    let none = providers_from(other.path(), |_, _| {});
    assert!(matches!(none.continue_on_account(Source::Claude, STUCK, NEW).await, Err(droppen_provider::ContinueError::Unsupported)));

    let copy = providers.continue_on_account(Source::Claude, STUCK, NEW).await.expect("Claude Code continues it");
    assert_eq!(copy, droppen_provider_claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().join("-w").join(format!("{NEW}.jsonl")));
}

/// A source no provider runs is refused at the start and blocked at a send,
/// whatever the request names: nothing could start or drive its session.
#[test]
fn a_source_no_provider_runs_is_refused() {
    let home = tempfile::tempdir().expect("tempdir");
    let none = providers_from(home.path(), |_, _| {});
    let other = tempfile::tempdir().expect("tempdir");
    let claude = providers_from(other.path(), claude_only);
    let refusal = |providers: &ProviderRegistry, source, agent| {
        providers.check_start(source, agent).err().map(|error| {
            assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
            error.body()["error"].as_str().unwrap_or_default().to_string()
        })
    };
    for source in [Source::Claude, Source::Codex, Source::Acp] {
        for agent in [None, Some("fake")] {
            assert_eq!(refusal(&none, source, agent), Some(format!("no provider runs {source} sessions")), "{source} {agent:?}");
        }
    }
    assert_eq!(refusal(&claude, Source::Claude, None), None);
    assert_eq!(refusal(&claude, Source::Acp, Some("fake")).as_deref(), Some("no provider runs acp sessions"));
    assert_eq!(refusal(&claude, Source::Codex, None).as_deref(), Some("no provider runs codex sessions"));

    assert_eq!(none.send_block(&row("claude", None)).as_deref(), Some("no provider runs claude sessions"));
    assert_eq!(none.send_block(&row("acp", Some("fake"))).as_deref(), Some("no provider runs acp sessions"));
    assert_eq!(claude.send_block(&row("claude", None)), None);
    assert_eq!(claude.send_block(&row("codex", None)).as_deref(), Some("no provider runs codex sessions"));
    assert_eq!(claude.send_block(&row("acp", Some("fake"))).as_deref(), Some("no provider runs acp sessions"));
}

/// An instant chat turns attachments away when its CLI's chats cannot read
/// them (Claude Code's), and when no provider runs its source. Codex's chats
/// read them, and ACP runs no instant chats to refuse them in.
#[test]
fn a_chat_refuses_the_attachments_its_cli_cannot_read() {
    let home = tempfile::tempdir().expect("tempdir");
    let providers = providers(home.path());
    assert!(providers.chat_refuses_attachments(Source::Claude));
    assert!(!providers.chat_refuses_attachments(Source::Codex));
    assert!(!providers.chat_refuses_attachments(Source::Acp));
    let other = tempfile::tempdir().expect("tempdir");
    let claude = providers_from(other.path(), claude_only);
    assert!(claude.chat_refuses_attachments(Source::Claude));
    assert!(claude.chat_refuses_attachments(Source::Codex), "no provider runs codex here");
    assert!(claude.chat_refuses_attachments(Source::Acp), "no provider runs acp here");
}

/// An ACP model list names its agent by an id that can be a directory name,
/// checked before the agent is looked up, and refused in these words
/// otherwise; the other CLIs' lists ignore the field. A source no provider
/// runs has no list.
#[tokio::test]
async fn an_acp_model_list_names_its_agent_by_an_id_that_can_be_a_directory_name() {
    let home = tempfile::tempdir().expect("tempdir");
    let providers = providers(home.path());
    let query = |source, agent: Option<&str>| droppen_provider::models::ModelsQuery {
        source,
        account_id: None,
        acp_agent: agent.map(str::to_owned),
    };
    let acp = providers.models(Source::Acp).expect("the ACP provider");
    let too_long = "x".repeat(droppen_provider_acp::sessions::MAX_AGENT_ID + 1);
    for agent in [None, Some("../fake"), Some("Fake"), Some(""), Some("a/b"), Some("a\0b"), Some(too_long.as_str())] {
        assert_eq!(
            acp.list(&query(Source::Acp, agent)).await.err().as_deref(),
            Some("acpAgent names the ACP agent to list models for"),
            "{agent:?}"
        );
    }
    let unconfigured = acp.list(&query(Source::Acp, Some("other"))).await.expect("a list");
    assert!(unconfigured.models.is_empty() && unconfigured.hidden.is_none());
    assert_eq!(unconfigured.source, droppen_provider::models::ModelListSource::Fallback, "an agent that is not configured has no list");
    let codex = providers.models(Source::Codex).expect("the Codex provider");
    assert!(codex.list(&query(Source::Codex, Some("../fake"))).await.is_ok(), "only an ACP list reads acpAgent");

    let other = tempfile::tempdir().expect("tempdir");
    assert!(providers_from(other.path(), |_, _| {}).models(Source::Acp).is_none());
}

/// Each CLI's model ids are held to its own spelling: names for Claude Code
/// and Codex, `provider:model` paths for an ACP agent, and nothing at all
/// for a source no provider runs.
#[test]
fn each_cli_spells_its_model_ids_its_own_way() {
    use droppen_provider::models::ModelRule;
    let home = tempfile::tempdir().expect("tempdir");
    let providers = providers(home.path());
    assert_eq!(providers.model_rule(Source::Claude), ModelRule::NAMED);
    assert_eq!(providers.model_rule(Source::Codex), ModelRule::NAMED);
    let acp = providers.model_rule(Source::Acp);
    assert!(acp.admits("openrouter:vendor/name") && acp.admits(&"m".repeat(200)));
    assert!(!acp.admits(&"m".repeat(201)) && !acp.admits("--add-dir") && !acp.admits("a b"));
    assert!(!ModelRule::NAMED.admits("openrouter:vendor/name"), "a path is not a name");

    let other = tempfile::tempdir().expect("tempdir");
    let none = providers_from(other.path(), |_, _| {});
    assert_eq!(none.model_rule(Source::Claude), ModelRule::NONE);
    assert!(!ModelRule::NONE.admits("opus"), "a source no provider runs takes no model");
}

/// A new session names an ACP agent exactly when it is an ACP session, and
/// then a configured one, by an id that can be a directory name: the id is
/// checked before it is looked up, and every refusal is a `400` in these
/// words.
#[test]
fn a_new_session_names_an_acp_agent_exactly_when_it_is_one() {
    let home = tempfile::tempdir().expect("tempdir");
    let providers = providers(home.path());
    let refusal = |source, agent| {
        providers.check_start(source, agent).err().map(|error| {
            assert_eq!(error.status(), gotcha::axum::http::StatusCode::BAD_REQUEST);
            error.body()["error"].as_str().unwrap_or_default().to_string()
        })
    };
    assert_eq!(refusal(Source::Acp, None).as_deref(), Some("an ACP session needs acpAgent: the agent it runs on"));
    assert_eq!(refusal(Source::Acp, Some("../fake")).as_deref(), Some("invalid ACP agent id: ../fake"));
    assert_eq!(refusal(Source::Acp, Some("Fake")).as_deref(), Some("invalid ACP agent id: Fake"));
    assert_eq!(refusal(Source::Acp, Some("other")).as_deref(), Some("no ACP agent \"other\" is configured"));
    assert_eq!(refusal(Source::Acp, Some("fake")), None);
    for source in [Source::Claude, Source::Codex] {
        assert_eq!(refusal(source, None), None, "{source}");
        for agent in ["fake", "../fake"] {
            assert_eq!(refusal(source, Some(agent)).as_deref(), Some("acpAgent is only for ACP sessions"), "{source} {agent}");
        }
    }
}

/// Only an ACP session is ever blocked by its agent: one whose agent is
/// gone, or that names none.
#[test]
fn a_session_is_blocked_only_when_its_acp_agent_is_gone() {
    let home = tempfile::tempdir().expect("tempdir");
    let providers = providers(home.path());
    assert_eq!(providers.send_block(&row("acp", Some("fake"))), None);
    assert_eq!(providers.send_block(&row("acp", Some("gone"))), Some(droppen_provider_acp::agents::removed("gone")));
    assert_eq!(
        providers.send_block(&row("acp", None)).as_deref(),
        Some("this ACP session does not say which agent it ran on")
    );
    for source in ["claude", "codex"] {
        assert_eq!(providers.send_block(&row(source, Some("gone"))), None, "{source}");
        assert_eq!(providers.send_block(&row(source, None)), None, "{source}");
    }
}

/// Each provider puts its own sessions away: Claude Code onto this server's
/// hidden list, and ACP refuses, in its archive's own words, a session that
/// is not there. A source no provider runs is refused.
#[tokio::test]
async fn each_provider_archives_its_own_sessions() {
    let home = tempfile::tempdir().expect("tempdir");
    let providers = providers(home.path());
    let paths = Paths::new(home.path().to_path_buf());
    providers.archive(Source::Claude, "c1").await.expect("archived");
    assert!(droppen_provider_claude_code::sessions::claude::archive::archived_ids(&paths).contains("c1"));
    assert_eq!(
        providers.archive(Source::Acp, "a1").await,
        Err(droppen_sessions::archive::ArchiveError("cannot archive the ACP session: it is gone".to_string()).to_string())
    );

    let other = tempfile::tempdir().expect("tempdir");
    let none = providers_from(other.path(), |_, _| {});
    for source in [Source::Claude, Source::Codex, Source::Acp] {
        assert_eq!(none.archive(source, "x").await, Err(format!("no provider runs {source} sessions")), "{source}");
    }
    assert!(droppen_provider_claude_code::sessions::claude::archive::archived_ids(&Paths::new(other.path().to_path_buf())).is_empty(), "nothing was archived");
}
