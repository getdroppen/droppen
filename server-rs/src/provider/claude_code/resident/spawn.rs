//! How a claude resident is started: its command line, the environment and
//! the account it runs under, for a send, a start and a rotation.

use std::sync::Arc;

use droppen_accounts::store::AccountProvider;
use crate::provider::claude_code::claude_roots;
use crate::provider::common::{new_uuid, session_workdir, CHAT_SYSTEM_PROMPT};
use crate::provider::{claude_prompt, require_provider, claude_code::resident, AccountChoice, InjectError, SendOptions, SendTo, Sent, SpawnGate, StartAt};
use crate::provider::claude_code::sessions::claude;
use droppen_sessions::Source;
use droppen_base::process_env::{claude_env, claude_env_bare, clean_env_bare, Env};
use droppen_instance::paths::{ClaudeRoot, Paths};

fn claude_chat_flags() -> Vec<String> {
    vec![
        "--system-prompt".into(),
        CHAT_SYSTEM_PROMPT.into(),
        "--disallowedTools".into(),
        "*".into(),
    ]
}

/// The tools a resident is started without, whatever else it is given.
///
/// The plan-mode pair, because of what `--permission-prompt-tool stdio` does to
/// them: `EnterPlanMode` runs without asking anyone, and the only way back out
/// is `ExitPlanMode`, which is a permission prompt — and every permission
/// prompt a resident raises is refused (see [`crate::provider::questions`]). A model that
/// entered plan mode would be shut in it, read-only, for the rest of the
/// session. Without the prompt tool the CLI never offered either of them to a
/// `-p` process, so nothing is lost by saying so.
const RESIDENT_DISALLOWED_TOOLS: [&str; 2] = ["EnterPlanMode", "ExitPlanMode"];

/// claude has no image flag; it views every attachment through the Read tool.
pub(super) fn with_claude_attachments(text: &str, options: &SendOptions) -> String {
    claude_prompt(text, &options.images, &options.files)
}

/// The settings half of a resident's command line: everything after the
/// session selector, and nothing that varies per message.
///
/// Two things changed with the resident model, and both are visible here. The
/// prompt is gone — it goes in over stdin as stream-json, which is what lets one
/// process take message after message — and with it the `--` guard, because
/// with no positional argument left there is nothing for a variadic flag to
/// swallow. The variadic flags are still last for the same reason they were
/// before: `--add-dir` and `--disallowedTools` would otherwise eat whatever
/// followed them.
///
/// Everything on this line is fixed for the process' whole life, which is what
/// [`resident::Fingerprint`] exists to compare: a message wanting a different
/// model needs a different process, not a different argument.
fn claude_resident_flags(paths: &Paths, options: &SendOptions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = &options.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    if let Some(effort) = &options.effort {
        args.extend(["--effort".to_string(), effort.clone()]);
    }
    if let Some(permission) = &options.permission {
        args.extend(["--permission-mode".to_string(), permission.clone()]);
    }
    // Where claude sends what it would otherwise have asked a terminal: back
    // down the stream, as a `control_request` the driver answers. This is what
    // makes `AskUserQuestion` exist at all for a `-p` process — and it routes
    // every other permission prompt the same way, each of which the driver
    // refuses exactly as `--permission-mode` used to. See [`crate::provider::questions`].
    args.extend(["--permission-prompt-tool".to_string(), "stdio".to_string()]);
    // Prepare every process for later screenshots, even when its first prompt
    // is text-only. Adding this flag later would require a process restart.
    args.extend(["--add-dir".into(), paths.uploads_dir().to_string_lossy().into_owned()]);
    // One `--disallowedTools`, not two: it is variadic, so it goes last, and a
    // chat's `*` already covers the plan tools.
    if options.chat {
        args.extend(claude_chat_flags());
    } else {
        args.push("--disallowedTools".into());
        args.extend(RESIDENT_DISALLOWED_TOOLS.iter().map(|tool| (*tool).to_string()));
    }
    args
}

/// Everything a resident spawn needs, resolved against one send.
///
/// Assembled on the send path rather than in the driver because this is where
/// the account store, the uploads directory and the session's own working
/// directory are reachable; the driver only ever compares it and hands it to
/// [`crate::provider::shim::spawn_via_shim`].
pub(super) fn claude_recipe(paths: &Paths, cwd: Option<&str>, options: &SendOptions, env: Env, login: ClaudeLogin) -> resident::Recipe {
    resident::Recipe {
        args: claude_resident_flags(paths, options),
        env,
        workdir: Some(session_workdir(paths, cwd)),
        fingerprint: resident::Fingerprint {
            model: options.model.clone(),
            // A send asks for a model; only the CLI can say what it will call
            // it. See [`resident::Fingerprint::resolved`].
            resolved: None,
            effort: options.effort.clone(),
            permission: options.permission.clone(),
            account: login.account,
            credential: login.credential,
            chat: options.chat,
            uploads: true,
            questions: true,
        },
    }
}

/// Which login a claude spawn runs under, in the two parts a resident is
/// compared by.
///
/// The id alone was the whole answer until an env account was edited under a
/// running process: the id says who is billed, and says nothing about what is
/// actually in the environment. So the credential is named too — by its hash,
/// which is the most the store will say about one — and a send that resolves to
/// the same account holding a *different* credential no longer looks like the
/// same login. See [`resident::Fingerprint::credential`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ClaudeLogin {
    /// The account whose credential is in the environment; `None` is the
    /// root's own login.
    account: Option<String>,
    /// The hash of the credential that account held when this was resolved.
    credential: Option<String>,
}

// ------------------------------------------------------------ account resolution

impl super::ClaudeCodeRuns {
    /// The environment a spawn against this instance's own Claude root runs with.
    ///
    /// Two independent pieces:
    ///
    /// * **the root.** Every session this server owns lives in
    ///   `<data>/instances/<name>/claude`, so every spawn against one is pointed
    ///   there. That is what makes the session resumable *here* and nowhere else.
    /// * **the token**, only when an account was chosen. Without one the spawn uses
    ///   whatever login the root itself holds — which is exactly what an adopted
    ///   session, or an instance the user logged into directly, needs.
    ///
    /// A `.credentials.json` in the root is only worth a warning in the token case.
    /// Older CLIs let file credentials outrank the environment, which is why this
    /// used to refuse; claude 2.1.266 was checked on 2026-09-23 with a bogus file
    /// next to a valid `CLAUDE_CODE_OAUTH_TOKEN` and used the environment, and the
    /// `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY` tracks sit ahead of the file as
    /// well. Refusing would only make the root's own login (which no-account
    /// sessions need) and account sessions mutually exclusive.
    async fn owned_claude_env(&self, account_id: Option<&str>) -> Result<Env, InjectError> {
        let dir = claude_roots::ClaudeRoots::of(&self.paths).ensure_managed()
            .await
            .map_err(|error| InjectError::Internal(format!("cannot create this instance's Claude root: {error}")))?;
        let mut vars = vec![("CLAUDE_CONFIG_DIR".to_string(), dir.to_string_lossy().into_owned())];
        if let Some(account_id) = account_id {
            if let Some(path) = claude_roots::ClaudeRoots::of(&self.paths).managed_file_credentials().await {
                tracing::warn!(
                    path = %path.display(),
                    account = account_id,
                    "the managed root holds file credentials; the account token in the environment takes precedence on current CLIs"
                );
            }
            let launch = self.accounts.account_env(account_id).await?;
            // A codex key in `ANTHROPIC_API_KEY` would be a 401 under a label that
            // says otherwise; refuse it here, where the account is still nameable.
            require_provider(&launch, AccountProvider::Claude)?;
            vars.extend(launch.env);
        }
        Ok(claude_env(vars))
    }
}

/// A spawn environment with one account's variables swapped for another's.
///
/// Everything else in `env` — the Claude root, the `rh-notify` decoration, the
/// `PATH` it was started with — is what the session already runs with and is
/// kept exactly. Each of `old`'s variables goes; where the server's own
/// environment would have supplied one of those names had no account set it,
/// that value comes back (`inherited`, which never answers for the harness
/// variables [`claude_env`] strips). Then `new`'s go in on top — the same
/// "account last" order [`super::ClaudeCodeRuns::owned_claude_env`] builds in.
///
/// Known limitation: `old` is the old account's block as stored *now*. A key
/// deleted from that block after the process started (say `ANTHROPIC_BASE_URL`)
/// is not in `old`, so its stale value survives into the new environment.
fn swap_account_env(env: &Env, old: &[(String, String)], new: &[(String, String)], inherited: impl Fn(&str) -> Option<String>) -> Env {
    let mut env = env.clone();
    for (key, _) in old {
        match inherited(key) {
            Some(value) => env.insert(key.clone(), value),
            None => env.remove(key),
        };
    }
    for (key, value) in new {
        env.insert(key.clone(), value.clone());
    }
    env
}

/// What the server's own environment gives a spawn under `key`, after the strip
/// [`claude_env`] makes.
fn inherited_var(key: &str) -> Option<String> {
    clean_env_bare().remove(key)
}

impl super::ClaudeCodeRuns {
    /// Which login a resume runs under.
    ///
    /// A session is this server's own when its transcript sits in the root this
    /// instance owns, or when it is already bound to an account — the binding is
    /// written before the first spawn, so it is authoritative even before the CLI
    /// has created the transcript.
    ///
    /// A session that is *not* this server's own reaches here only from a caller
    /// that has already checked (the send route refuses it with `external_session`),
    /// so the local-login branch survives for the account-mismatch report and for
    /// anything internal that drives a session directly.
    /// The account is returned beside the environment because a resident is
    /// fingerprinted by it: two messages billed to different logins cannot share a
    /// process, and the resolution — binding, explicit choice, default — happens
    /// only here.
    async fn resume_claude_env(&self, id: &str, options: &SendOptions) -> Result<(Env, ClaudeLogin), InjectError> {
        let found = claude::find_claude_session(&self.paths, id).await;
        let bound = self.accounts.session_account_id(id).await;
        let owned = found.map(|f| f.root) == Some(ClaudeRoot::Managed) || bound.is_some();

        if !owned {
            // An explicit account on a session in the user's own root is a mistake
            // worth reporting; `accountId: null` on one is simply what it already does.
            if options.account.named().is_some() {
                return Err(InjectError::LocalSessionAccount);
            }
            return Ok((claude_env_bare(), ClaudeLogin::default()));
        }

        // `accountId: null` means "no account token", which for an owned session is
        // the root's own login rather than the user's `~/.claude`; absent means
        // "whatever this session is already bound to".
        let account_id = match &options.account {
            AccountChoice::Local => None,
            // Only a new start carries a rule's account; on a resume it is a name.
            AccountChoice::Account(id) | AccountChoice::Rule(id) => Some(id.clone()),
            AccountChoice::Unset => bound.clone(),
        };
        let env = self.owned_claude_env(account_id.as_deref()).await?;
        if let Some(chosen) = options.account.named() {
            if Some(chosen) != bound.as_deref() {
                self.accounts.bind_session_account(id, Some(chosen), AccountProvider::Claude).await?;
                // Somebody's choice: nothing moves it ahead of a limit for a while.
                self.accounts.rotation_log().note_hand_binding(id, droppen_accounts::rest::now_ms());
            }
        }
        let login = self.login_of(account_id).await;
        Ok((env, login))
    }
}

impl super::ClaudeCodeRuns {
    /// Give a claude spawn the means to notify its user, when there is a channel.
    ///
    /// Claude only, for now, and the reason is the skill rather than the program:
    /// `rh-notify` would work perfectly well on a `codex exec` command line, but
    /// nothing would ever tell codex that it exists — skills are a Claude Code
    /// mechanism, and a `codex` thread's id is not even known until after the spawn
    /// that would have carried the token. See [`crate::provider::SpawnDecoration`].
    ///
    /// Deliberately infallible. A session that cannot notify is a session; a session
    /// that would not start because a notification channel could not be set up is a
    /// bug with a very unhappy user attached.
    async fn notify_decorate(&self, id: &str, env: &mut Env) {
        self.decoration.decorate(Source::Claude, id, env).await;
    }
}

impl super::ClaudeCodeRuns {
    /// Hand a message to claude for an existing session.
    pub(super) async fn claude_send(self: &Arc<Self>, gate: &SpawnGate, to: SendTo<'_>) -> Result<Sent, InjectError> {
        let SendTo { id, cwd, text, options } = to;
        // Resolved before anything is built: an unusable account must not spawn.
        let (mut env, login) = self.resume_claude_env(id, options).await?;
        self.notify_decorate(id, &mut env).await;
        let recipe = claude_recipe(&self.paths, cwd, options, env, login);
        // claude has no image flag, so every attachment reaches it as a path in
        // the prompt, which travels over stdin.
        let prompt = with_claude_attachments(text, options);
        let delivery = self.deliver(gate, id, &prompt, recipe, options.message_id.clone()).await?;
        Ok(Sent { queued: delivery.queued })
    }

    /// Start a brand-new claude session in `cwd` and resolve once its id is known.
    pub(super) async fn claude_start(self: &Arc<Self>, gate: &SpawnGate, at: StartAt<'_>) -> Result<String, InjectError> {
        let StartAt { cwd, text, options } = at;
        // The local-login default for a claude spawn; an account overwrites it
        // below with a managed environment, which carries the marker too.
        let mut env = claude_env_bare();
        let id = new_uuid();
        let chosen = match &options.account {
            // The configured default (moved off a rest), or — with none, and
            // the local login known to be signed out — an account in the pool:
            // a chat or a scheduled task started there would fail its first turn.
            AccountChoice::Unset if options.use_default_account => {
                self.start_account(AccountProvider::Claude, options.model.as_deref()).await
            }
            AccountChoice::Unset | AccountChoice::Local => None,
            AccountChoice::Account(id) => Some(id.clone()),
            AccountChoice::Rule(named) => self.rule_account(AccountProvider::Claude, named, options.model.as_deref()).await,
        };
        match &chosen {
            Some(account_id) => {
                env = self.owned_claude_env(Some(account_id)).await?;
                // Bound before the spawn: a crash mid-launch must not leave a
                // managed session nobody can resume.
                self
                    .accounts
                    .bind_session_account(&id, Some(account_id), AccountProvider::Claude)
                    .await?;
            }
            // The local login writes into the user's own `~/.claude`, so this
            // session's own directory will say it belongs to nobody. It belongs
            // to us: we just made it, and refusing to send the *second* message
            // to a session this server started would be absurd. Recorded before
            // the spawn, like the binding above and for the same reason.
            None => {
                if let Err(error) = crate::provider::claude_code::sessions::claude::ClaudeCodeSessions.record_owned(&self.paths, &id).await {
                    tracing::error!(session = %id, %error, "cannot record this claude session as ours; it will list as external");
                }
            }
        }
        // Unlike a resume, a new session waits for the launch: there is no
        // session yet for a failure to be reported against. The resident's own
        // spawn is what is awaited — the id is ours either way, because a
        // session with no transcript is started with `--session-id`.
        self.notify_decorate(&id, &mut env).await;
        let login = self.login_of(chosen).await;
        let recipe = claude_recipe(&self.paths, Some(cwd), options, env, login);
        let prompt = with_claude_attachments(text, options);
        self.deliver(gate, &id, &prompt, recipe, options.message_id.clone()).await?;
        Ok(id)
    }
}

impl super::ClaudeCodeRuns {
    /// `recipe`, rebuilt to bill `to` instead of `from`.
    ///
    /// The command line and the working directory are the session's and stay as
    /// they are; the environment has `from`'s variables swapped for `to`'s (see
    /// [`swap_account_env`]) and the fingerprint names `to` and the credential
    /// it holds now, so the process this recipe starts is the one a message for
    /// `to` fits. A recipe that was not `from`'s to begin with, or a `from` the
    /// store no longer holds, is rebuilt from scratch the way the send path
    /// builds one instead.
    pub(super) async fn retarget_recipe(&self, session: &str, recipe: &resident::Recipe, from: &str, to: &str) -> Result<resident::Recipe, InjectError> {
        let new = self.accounts.account_env(to).await?;
        require_provider(&new, AccountProvider::Claude)?;
        let old = match recipe.fingerprint.account.as_deref() {
            Some(account) if account == from => self.accounts.account_env(from).await.ok(),
            _ => None,
        };
        let env = match old {
            Some(old) => swap_account_env(&recipe.env, &old.env, &new.env, inherited_var),
            None => self.account_spawn_env(session, to).await?,
        };
        let login = self.login_of(Some(to.to_string())).await;
        let mut fingerprint = recipe.fingerprint.clone();
        fingerprint.account = login.account;
        fingerprint.credential = login.credential;
        fingerprint.resolved = None;
        Ok(resident::Recipe {
            args: recipe.args.clone(),
            env,
            workdir: recipe.workdir.clone(),
            fingerprint,
        })
    }

    /// A recipe for a session's process under `account`, from the settings it
    /// was running with, for a driver that was never handed one: a resident
    /// inherited across a restart that no message has described yet. Built the
    /// way the send path builds one — see [`claude_recipe`].
    pub(super) async fn recipe_from_settings(
        &self,
        session: &str,
        live: &resident::Fingerprint,
        cwd: Option<&str>,
        account: &str,
    ) -> Result<resident::Recipe, InjectError> {
        let options = SendOptions {
            model: live.model.clone(),
            effort: live.effort.clone(),
            permission: live.permission.clone(),
            chat: live.chat,
            ..SendOptions::default()
        };
        let env = self.account_spawn_env(session, account).await?;
        let login = self.login_of(Some(account.to_string())).await;
        Ok(claude_recipe(&self.paths, cwd, &options, env, login))
    }

    /// The whole environment a spawn of `session` under `account` gets, built
    /// from nothing: the managed root, the account, the `rh-notify` decoration.
    async fn account_spawn_env(&self, session: &str, account: &str) -> Result<Env, InjectError> {
        let mut env = self.owned_claude_env(Some(account)).await?;
        self.notify_decorate(session, &mut env).await;
        Ok(env)
    }

    /// That account, plus a name for the credential it holds right now.
    ///
    /// Read here rather than carried out of [`super::ClaudeCodeRuns::owned_claude_env`] because it is
    /// wanted on the path that does not build an environment at all — a local-login
    /// send — and because the store is the only thing that may answer it.
    async fn login_of(&self, account: Option<String>) -> ClaudeLogin {
        let credential = match account.as_deref() {
            Some(id) => self.accounts.credential_hash(id).await,
            None => None,
        };
        ClaudeLogin { account, credential }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rotation keeps everything a session's environment holds but the
    /// account: the old account's variables go (back to the server's own value
    /// where it has one), the new one's come in, the rest is untouched.
    #[test]
    fn swapping_accounts_changes_only_the_account_in_an_environment() {
        let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
        let env: Env = [
            pair("PATH", "/session-bin:/usr/bin"),
            pair("CLAUDE_CONFIG_DIR", "/data/instances/default/claude"),
            pair("DROPPEN_NOTIFY_TOKEN", "minted-for-this-session"),
            pair("ANTHROPIC_AUTH_TOKEN", "relay-token"),
            pair("ANTHROPIC_BASE_URL", "https://relay.example"),
            pair("HTTPS_PROXY", "http://account-proxy"),
        ]
        .into_iter()
        .collect();
        let old = [
            pair("ANTHROPIC_AUTH_TOKEN", "relay-token"),
            pair("ANTHROPIC_BASE_URL", "https://relay.example"),
            pair("HTTPS_PROXY", "http://account-proxy"),
        ];
        let new = [pair("CLAUDE_CODE_OAUTH_TOKEN", "oauth-token")];
        let server = |key: &str| (key == "HTTPS_PROXY").then(|| "http://server-proxy".to_string());

        let swapped = swap_account_env(&env, &old, &new, server);
        let expected: Env = [
            pair("PATH", "/session-bin:/usr/bin"),
            pair("CLAUDE_CONFIG_DIR", "/data/instances/default/claude"),
            pair("DROPPEN_NOTIFY_TOKEN", "minted-for-this-session"),
            pair("HTTPS_PROXY", "http://server-proxy"),
            pair("CLAUDE_CODE_OAUTH_TOKEN", "oauth-token"),
        ]
        .into_iter()
        .collect();
        assert_eq!(swapped, expected);
    }

    fn options() -> SendOptions {
        SendOptions::default()
    }

    fn paths() -> Paths {
        Paths::new(std::path::PathBuf::from("/home/me"))
    }

    /// The prompt is no longer on the command line at all — it goes in over
    /// stdin — and what is left is exactly the settings a resident is pinned
    /// to. Both halves matter: a prompt that leaked back into argv would make
    /// every message a new process again, and a settings flag that went missing
    /// would silently run the turn under the wrong model.
    #[test]
    fn a_resident_carries_its_settings_and_never_the_prompt() {
        let mut options = options();
        options.model = Some("opus".into());
        options.effort = Some("high".into());
        options.permission = Some("acceptEdits".into());
        options.images = vec!["/uploads/a.png".into()];
        let args = claude_resident_flags(&paths(), &options);
        assert_eq!(
            args,
            [
                "--model",
                "opus",
                "--effort",
                "high",
                "--permission-mode",
                "acceptEdits",
                "--permission-prompt-tool",
                "stdio",
                "--add-dir",
                "/home/me/.local/share/remote-hand/uploads",
                "--disallowedTools",
                "EnterPlanMode",
                "ExitPlanMode",
            ]
        );
        assert!(!args.iter().any(|arg| arg == "--"), "nothing positional is left to guard");
        assert!(!args.iter().any(|arg| arg.contains("hello")), "the prompt goes over stdin");
        // And the attachment still reaches claude, through the prompt text the
        // driver writes rather than through a flag.
        assert!(with_claude_attachments("hello", &options).starts_with("hello\n\n[1 attached file(s)"));
    }

    #[test]
    fn chat_flags_replace_the_persona_and_disable_every_tool() {
        let mut options = options();
        options.chat = true;
        let args = claude_resident_flags(&paths(), &options);
        assert!(args.contains(&"--disallowedTools".to_string()));
        assert_eq!(args.iter().filter(|a| *a == "*").count(), 1);
        assert!(args.contains(&CHAT_SYSTEM_PROMPT.to_string()));
        // The variadic flags are last, which is what replaces the `--` guard:
        // with no positional argument there is nothing left for them to eat.
        assert_eq!(args.last().expect("a flag"), "*");
        // One `--disallowedTools`, not a second for the plan tools: `*` covers
        // them, and two of a variadic flag is a question for the CLI's parser
        // nobody needs to ask.
        assert_eq!(args.iter().filter(|a| *a == "--disallowedTools").count(), 1);
    }

    /// The prompt tool is what lets claude ask a question at all, and the plan
    /// tools go with it: `EnterPlanMode` runs unasked and only a permission
    /// prompt — which a resident always refuses — leads back out.
    ///
    /// Recorded in the fingerprint, and never the reason for a restart: a
    /// resident started before the flag existed takes a message that wants it,
    /// and picks the flag up the next time it is replaced for a reason of its
    /// own.
    #[test]
    fn every_resident_can_ask_and_none_can_shut_itself_in_plan_mode() {
        for chat in [false, true] {
            let recipe = claude_recipe(&paths(), None, &SendOptions { chat, ..options() }, Env::new(), ClaudeLogin::default());
            assert!(recipe.args.windows(2).any(|pair| pair == ["--permission-prompt-tool", "stdio"]));
            assert!(recipe.fingerprint.questions);
            let disallowed = recipe.args.iter().position(|arg| arg == "--disallowedTools").expect("a deny list");
            let listed = &recipe.args[disallowed + 1..];
            assert!(
                listed.iter().all(|arg| !arg.starts_with("--")),
                "the variadic list is the end of the line: {listed:?}"
            );
            if chat {
                assert_eq!(listed, ["*"]);
            } else {
                assert_eq!(listed, ["EnterPlanMode", "ExitPlanMode"]);
            }
        }
        let wants = claude_recipe(&paths(), None, &options(), Env::new(), ClaudeLogin::default()).fingerprint;
        let older = resident::Fingerprint {
            questions: false,
            ..wants.clone()
        };
        assert!(wants.satisfied_by(&older), "a resident from before the flag is not restarted for it");
    }

    #[test]
    fn uploads_are_a_startup_capability_not_a_per_message_setting() {
        let paths = paths();
        for chat in [false, true] {
            let plain = SendOptions { chat, ..options() };
            let first = claude_recipe(&paths, None, &plain, Env::new(), ClaudeLogin::default());
            assert!(first.fingerprint.uploads);
            assert!(first
                .args
                .windows(2)
                .any(|pair| pair == ["--add-dir", "/home/me/.local/share/remote-hand/uploads"]));
            for attached in [
                SendOptions {
                    images: vec!["/uploads/a.png".into()],
                    ..plain.clone()
                },
                SendOptions {
                    files: vec!["/uploads/a.txt".into()],
                    ..plain.clone()
                },
            ] {
                let next = claude_recipe(&paths, None, &attached, Env::new(), ClaudeLogin::default());
                assert_eq!(first.args, next.args);
                assert_eq!(first.fingerprint, next.fingerprint);
            }
        }
    }
}
