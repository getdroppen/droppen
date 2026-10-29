//! What each CLI that runs under stored accounts brings to them.
//!
//! The store keeps the credentials and the rotation chooses between them in the
//! same way for every CLI. How a credential reaches a child differs: claude
//! reads it out of its environment as it stands, while codex has to be handed a
//! model provider of the server's own on its command line, and each is proved
//! to work by a run of its own CLI. Each CLI says how through its [`Login`],
//! and the entry hands the store one per [`AccountProvider`] ([`Logins`]), so
//! the accounts name no CLI's own code.

pub mod claude;
pub mod codex;

use std::path::Path;
use std::sync::Arc;

use futures_util::future::BoxFuture;

use super::local_login::LocalLoginState;
use super::quota::readings::SharedReading;
use super::quota::{CodexQuota, ProbeRoots, UsageProbe};
use super::store::{AccountError, AccountKind, AccountLaunch, AccountProvider};
use droppen_base::env_block::EnvVar;
use super::verify::Verdict;
use droppen_instance::paths::Paths;

/// One CLI's part of the accounts: how a credential starts it, and how one
/// is proved to work.
pub trait Login: Send + Sync {
    /// What one credential puts into a spawn of this CLI, without it having to
    /// be stored first.
    ///
    /// The same answer serves the spawn and the check before a credential is
    /// kept ([`super::verify`]): a check that ran with a different environment
    /// from the spawn would be a check of something else. An `Err` is a
    /// credential this CLI could never be started under, which the store
    /// refuses to keep.
    fn spawn_env(&self, kind: AccountKind, secret: &str, env: &[EnvVar]) -> Result<AccountLaunch, AccountError>;

    /// Spend one real request proving the credential `launch` was built from
    /// works: one run of this CLI with it, in a scratch home of its own, so the
    /// only login in reach is the one being checked.
    ///
    /// `secrets` are the credential's values: whatever the CLI says back is
    /// redacted of them before it reaches the verdict. Never fails: anything
    /// that is not a clean run or a clean refusal is [`Verdict::Unverifiable`].
    fn verify<'a>(&'a self, paths: &'a Paths, launch: &'a AccountLaunch, secrets: &'a [&'a str]) -> BoxFuture<'a, Verdict>;

    /// Whether the machine's own login of this CLI can start a session, from
    /// the evidence this server has of it.
    ///
    /// `refused_at` is when an active `auth` rest on that login was seen.
    /// `local_reading` is Claude Code's reading of its local login
    /// (`<data>/local-usage.json`), whichever CLI is asked: no other CLI keeps
    /// one, so every other login ignores it and weighs the refusal alone.
    fn local_state(&self, refused_at: Option<i64>, local_reading: &SharedReading) -> LocalLoginState;

    /// Run this CLI's usage probe against one of its logins: `config_dir` is an
    /// account's usage login, `None` the user's own. `None` back is a CLI with
    /// no such probe: only claude's logins keep readings for it to refresh.
    fn probe<'a>(&'a self, roots: &'a ProbeRoots, config_dir: Option<&'a Path>) -> Option<BoxFuture<'a, UsageProbe>>;

    /// The plan the user's own login of this CLI is on, as the quota panel
    /// names it, and the raw tier it was read from. `None` back is a CLI whose
    /// plan arrives with its quota reading instead.
    fn local_plan<'a>(&'a self, paths: &'a Paths) -> Option<BoxFuture<'a, (Option<String>, Option<String>)>>;

    /// The quota of the user's own login of this CLI, read whole: its plan
    /// and windows, live or out of the CLI's own logs. `None` back is a CLI
    /// whose quota is read per login instead ([`Login::probe`]).
    fn local_quota<'a>(&'a self, roots: &'a ProbeRoots) -> Option<BoxFuture<'a, Option<CodexQuota>>>;
}

/// Every CLI's [`Login`], one per [`AccountProvider`].
///
/// A field per provider rather than a map: the providers are a closed set, so
/// every account's CLI has a login and finding it cannot fail. The fields are
/// named where the entry fills them in, so which login serves which CLI is
/// written down there rather than implied by an argument's position; the store
/// still refuses a launch that claims another CLI than its account's
/// ([`super::store::AccountsStore::account_env`]).
#[derive(Clone)]
pub struct Logins {
    /// Claude Code's login.
    pub claude: Arc<dyn Login>,
    /// Codex's login.
    pub codex: Arc<dyn Login>,
}

impl Logins {
    /// The login of the CLI `provider` names.
    pub fn of(&self, provider: AccountProvider) -> &dyn Login {
        match provider {
            AccountProvider::Claude => self.claude.as_ref(),
            AccountProvider::Codex => self.codex.as_ref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::claude::ClaudeCodeLogin;
    use super::codex::{CodexLogin, CODEX_DEFAULT_BASE_URL};
    use super::*;
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
}
