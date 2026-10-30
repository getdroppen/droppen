//! The logins the entry builds, and what each one's launch is.

use crate::provider::claude_code::login::ClaudeCodeLogin;
use droppen_provider_codex::login::{CodexLogin, CODEX_DEFAULT_BASE_URL};
use droppen_accounts::login::{Login, Logins};
use droppen_accounts::store::{AccountError, AccountKind, AccountLaunch, AccountProvider, Credential};
use droppen_accounts::verify::paths_where_no_scratch_can_be_made;
use droppen_base::env_block::parse_env_block;

/// The entry files each login under the CLI it is for: a launch says which
/// CLI it starts, and every spawn trusts that word only after the store has
/// checked it against the account's own.
#[test]
fn each_cli_s_login_is_filed_under_that_cli() {
    let logins = crate::app::logins();
    let block = parse_env_block("OPENAI_API_KEY=k").expect("block");
    for provider in [AccountProvider::Claude, AccountProvider::Codex] {
        let launch = logins.of(provider).spawn_env(AccountKind::Env, "", &block).expect("launch");
        assert_eq!(launch.provider, provider, "{} is served by its own login", provider.label());
    }
}

/// The real logins filed under each other's CLI: a claude block holding an
/// OpenAI key would otherwise be checked, and could pass, as a codex key. The
/// launch Codex's login builds names codex, which is refused before any check
/// runs.
#[tokio::test]
async fn a_login_filed_under_the_other_cli_is_refused_before_its_check() {
    let home = tempfile::tempdir().expect("tempdir");
    let paths = paths_where_no_scratch_can_be_made(home.path());
    let real = crate::app::logins();
    let swapped = Logins {
        claude: real.codex.clone(),
        codex: real.claude.clone(),
    };
    let block = Credential::Env(parse_env_block("OPENAI_API_KEY=codex-test-key").expect("block"));
    let refused = swapped.verify_secret(&paths, AccountProvider::Claude, &block).await;
    assert!(
        matches!(refused, Err(AccountError::ProviderMismatch { account: AccountProvider::Claude, wanted: AccountProvider::Codex })),
        "{refused:?}"
    );
}

/// What each shape actually puts into a child process.
#[test]
fn a_launch_hands_claude_the_block_and_translates_it_for_codex() {
    fn pairs(launch: &AccountLaunch) -> Vec<(&str, &str)> {
        launch.env.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect()
    }
    let block = |text: &str| parse_env_block(text).expect("block");

    let oauth = ClaudeCodeLogin.spawn_env(AccountKind::OauthToken, "tok", &[]).expect("oauth");
    assert_eq!(pairs(&oauth), vec![("CLAUDE_CODE_OAUTH_TOKEN", "tok")]);
    assert!(oauth.codex_args.is_empty());

    // Claude takes the block exactly as typed — including a setup with no
    // recognisable credential variable in it at all, which is what Bedrock
    // and a helper script look like. The CLI decides, not this.
    let claude = ClaudeCodeLogin.spawn_env(
        AccountKind::Env,
        "",
        &block("ANTHROPIC_AUTH_TOKEN=bearer\nANTHROPIC_BASE_URL=https://gw/v1"),
    )
    .expect("claude env");
    assert_eq!(
        pairs(&claude),
        vec![("ANTHROPIC_AUTH_TOKEN", "bearer"), ("ANTHROPIC_BASE_URL", "https://gw/v1")]
    );
    assert!(claude.codex_args.is_empty());
    assert!(ClaudeCodeLogin.spawn_env(
        AccountKind::Env,
        "",
        &block("CLAUDE_CODE_USE_BEDROCK=1\nAWS_REGION=us-east-1")
    )
    .is_ok());

    // Codex gets the block *and* the translation on top of it.
    let codex = CodexLogin.spawn_env(AccountKind::Env, "", &block("OPENAI_API_KEY=sk-codex")).expect("codex");
    assert_eq!(
        pairs(&codex),
        vec![("OPENAI_API_KEY", "sk-codex"), ("DROPPEN_CODEX_API_KEY", "sk-codex")]
    );
    assert_eq!(
        codex.codex_args,
        vec![
            "-c".to_string(),
            "model_providers.remote_hand={name=\"remote-hand\",base_url=\"https://api.openai.com/v1\",env_key=\"DROPPEN_CODEX_API_KEY\",wire_api=\"responses\"}".to_string(),
            "-c".to_string(),
            "model_provider=remote_hand".to_string(),
        ]
    );
    assert!(
        !codex.codex_args.iter().any(|arg| arg.contains("sk-codex")),
        "a command line is world-readable in `ps`"
    );

    // A blank base URL is not a broken one: it is the vendor default, as it
    // was when the URL was a field that could be cleared.
    let blank = CodexLogin.spawn_env(
        AccountKind::Env,
        "",
        &block("OPENAI_API_KEY=sk-1\nOPENAI_BASE_URL=  "),
    )
    .expect("a block that overrides nothing");
    assert!(
        blank.codex_args[1].contains(&format!("base_url=\"{CODEX_DEFAULT_BASE_URL}\"")),
        "{:?}",
        blank.codex_args
    );

    // `CODEX_API_KEY` is the fallback, and `OPENAI_BASE_URL` moves the provider.
    let relay = CodexLogin.spawn_env(
        AccountKind::Env,
        "",
        &block("CODEX_API_KEY=sk-relay\nOPENAI_BASE_URL=https://relay.example.com/v1"),
    )
    .expect("codex relay");
    assert_eq!(
        relay.env.last().expect("translated"),
        &("DROPPEN_CODEX_API_KEY".to_string(), "sk-relay".to_string())
    );
    assert!(
        relay.codex_args[1].contains("base_url=\"https://relay.example.com/v1\""),
        "{:?}",
        relay.codex_args
    );
    // `OPENAI_API_KEY` wins when both are present.
    let both = CodexLogin.spawn_env(
        AccountKind::Env,
        "",
        &block("CODEX_API_KEY=second\nOPENAI_API_KEY=first"),
    )
    .expect("both");
    assert_eq!(both.env.last().expect("translated").1, "first");

    // Nothing to translate, and nothing a CLI could be pointed at.
    let missing = CodexLogin.spawn_env(AccountKind::Env, "", &block("SOME_OTHER_VAR=x")).expect_err("no key");
    assert!(missing.to_string().contains("OPENAI_API_KEY"), "{missing}");
    for bad in [
        "OPENAI_BASE_URL=relay.example.com",
        "OPENAI_BASE_URL=https:///v1",
        "OPENAI_BASE_URL=https://a\"b",
    ] {
        let refused = CodexLogin.spawn_env(AccountKind::Env, "", &block(&format!("OPENAI_API_KEY=k\n{bad}")));
        assert!(matches!(refused, Err(AccountError::Invalid(_))), "{bad} must be refused");
    }
}
