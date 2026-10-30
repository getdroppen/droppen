//! Codex's part of the accounts: how a credential starts `codex`, and the
//! check that proves one works before it is kept.

use futures_util::future::BoxFuture;

use droppen_accounts::login::{LocalPlan, Login};
use droppen_accounts::local_login::LocalLoginState;
use crate::accounts::quota::codex::{app_server_call, CodexReadingExt};
use droppen_accounts::quota::readings::SharedReading;
use droppen_accounts::quota::{CliTextExt, CodexQuota, ProbeRoots, UsageProbe};
use droppen_accounts::store::{AccountError, AccountKind, AccountLaunch, AccountProvider, CODEX_BASE_URL_VAR, CODEX_KEY_CANDIDATES};
use droppen_base::env_block::EnvVar;
use droppen_accounts::verify::{CliOutput, Scratch, Verdict, PROBE_PROMPT, PROBE_TIMEOUT};
use droppen_base::process_env::{apply_env, clean_env, clean_env_bare, resolve_on_path, Env};
use droppen_instance::paths::Paths;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// The environment variable a codex account's key is read from.
///
/// Not `OPENAI_API_KEY` and not `CODEX_API_KEY`, because `codex app-server`
/// reads neither: it authenticates from `CODEX_HOME/auth.json` and nothing
/// else. The one lever left that keeps `~/.codex` — and therefore every session
/// this server discovers — exactly where it is, is a *custom model provider*
/// whose key comes from an environment variable of our choosing. This is that
/// variable, and [`CodexBaseUrl::provider_args`] is what tells codex to look at it.
///
/// `REMOTE_HAND_CODEX_API_KEY` before the rename to Droppen, and renamed
/// outright, with no fallback: the spawn that sets it is the spawn that names
/// it (`env_key`), so nothing else ever reads it, and a `codex` started under
/// the old name keeps the configuration it was started with.
pub const CODEX_API_KEY_ENV: &str = "DROPPEN_CODEX_API_KEY";
/// The id the injected provider is registered under in codex's config.
pub const CODEX_PROVIDER_ID: &str = "remote_hand";
/// Where a codex account talks when it names no base URL of its own.
pub const CODEX_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

const MAX_BASE_URL_LENGTH: usize = 512;

/// A base URL fit to be interpolated into codex's inline TOML.
///
/// Its own type so that the only way to the command line is through
/// [`CodexBaseUrl::parse`]: a value that could close the TOML string would be
/// an injection into the CLI's own configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexBaseUrl(String);

impl CodexBaseUrl {
    /// Where a codex account talks when it names no base URL of its own
    /// ([`CODEX_DEFAULT_BASE_URL`]).
    pub fn vendor_default() -> Self {
        Self(CODEX_DEFAULT_BASE_URL.to_string())
    }

    /// `raw`, if it is fit to be interpolated into codex's inline TOML, or a refusal.
    ///
    /// The rules are narrow on purpose: this string is put inside a quoted TOML
    /// value on a command line, so a quote, a backslash or a control character in
    /// it is a way to say something to the child that nobody typed.
    pub fn parse(raw: &str) -> Result<Self, AccountError> {
        let url = raw.trim();
        if url.len() > MAX_BASE_URL_LENGTH {
            return Err(AccountError::Invalid(format!("{CODEX_BASE_URL_VAR} is implausibly long")));
        }
        let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else {
            return Err(AccountError::Invalid(format!("{CODEX_BASE_URL_VAR} must start with http:// or https://")));
        };
        // `https:///v1` parses as a URL with no host, and a CLI pointed at one
        // fails somewhere far away from the field that was typed wrong.
        if rest.is_empty() || rest.starts_with('/') {
            return Err(AccountError::Invalid(format!("{CODEX_BASE_URL_VAR} has no host")));
        }
        if url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\') {
            return Err(AccountError::Invalid(format!(
                "{CODEX_BASE_URL_VAR} cannot contain whitespace, a quote or a backslash"
            )));
        }
        Ok(Self(url.to_string()))
    }

    /// The `-c` overrides that point one `codex` spawn at an account's own key.
    ///
    /// Inline TOML on the command line, which is why [`CodexBaseUrl::parse`]
    /// refuses a quote, a backslash and anything that is not printable: the base
    /// URL is interpolated into the table below.
    ///
    /// `wire_api = "responses"` because that is the API the OpenAI-compatible
    /// endpoints codex speaks to expose; `name` is only ever shown to a human.
    pub fn provider_args(&self) -> Vec<String> {
        let base_url = &self.0;
        vec![
            "-c".to_string(),
            format!(
                "model_providers.{CODEX_PROVIDER_ID}={{name=\"remote-hand\",base_url=\"{base_url}\",env_key=\"{CODEX_API_KEY_ENV}\",wire_api=\"responses\"}}"
            ),
            "-c".to_string(),
            format!("model_provider={CODEX_PROVIDER_ID}"),
        ]
    }
}

/// What a codex probe runs. `--skip-git-repo-check` because the scratch
/// directory is deliberately not a repository, and the account's own provider
/// override is appended by [`AccountLaunch::codex_args`].
const CODEX_PROBE_ARGS: [&str; 2] = ["exec", "--skip-git-repo-check"];

/// Whether codex is complaining about the *key* rather than about itself.
///
/// The same narrow direction as claude's `is_credential_refusal`: a miss stores an
/// unchecked key and says so, a false positive refuses a working one.
fn is_codex_refusal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: [&str; 5] = [
        "invalid_api_key",
        "incorrect api key",
        "unauthorized",
        "missing bearer",
        "invalid api key",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker)) || lower.mentions_status("401", None)
}

/// Codex's [`Login`].
pub struct CodexLogin;

impl CodexLogin {
    /// One `codex exec ping` through the account's own model provider.
    ///
    /// Two deliberate differences from the claude probe, both about *not touching
    /// the user's codex state*:
    ///
    /// * a scratch `CODEX_HOME`, so the run's rollout lands in a directory that is
    ///   deleted afterwards rather than appearing in the session list — and so the
    ///   probe cannot fall back on the user's own `auth.json` and answer for a login
    ///   that is not the one being checked;
    /// * the provider override from [`crate::accounts::login::codex::CodexBaseUrl::provider_args`],
    ///   which is the whole of how a key reaches codex: `app-server` reads neither
    ///   `OPENAI_API_KEY` nor `CODEX_API_KEY`, so a probe that only set a variable
    ///   would be checking nothing at all.
    async fn check(&self, paths: &Paths, launch: &AccountLaunch, secrets: &[&str]) -> Verdict {
        if resolve_on_path("codex", &clean_env_bare()).is_none() {
            return Verdict::Unverifiable("the `codex` CLI is not on PATH, so the key cannot be checked".to_string());
        }
        let scratch = match Scratch::make(paths).await {
            Ok(scratch) => scratch,
            Err(error) => return Verdict::Unverifiable(format!("cannot prepare the check's config directory: {error}")),
        };

        let mut command = tokio::process::Command::new("codex");
        command
            .args(
                CODEX_PROBE_ARGS
                    .iter()
                    .map(|arg| arg.to_string())
                    .chain(launch.codex_args.iter().cloned()),
            )
            .arg(PROBE_PROMPT)
            .current_dir(scratch.cwd())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        apply_env(
            &mut command,
            &clean_env(
                [("CODEX_HOME".to_string(), scratch.codex_home().to_string_lossy().into_owned())]
                    .into_iter()
                    .chain(launch.env.iter().cloned()),
            ),
        );

        let output = match tokio::time::timeout(PROBE_TIMEOUT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Verdict::Unverifiable(format!("the `codex` CLI could not be started: {error}")),
            Err(_) => return Verdict::Unverifiable("the check timed out before the CLI answered".to_string()),
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        self.verdict(output.status.success(), &stdout, &stderr, secrets)
    }

    /// What one finished `codex exec ping` says about the key it ran with:
    /// its words first, then its exit status.
    fn verdict(&self, success: bool, stdout: &str, stderr: &str, secrets: &[&str]) -> Verdict {
        // `codex exec` reports a refusal on stderr and still exits zero often
        // enough that the exit code cannot be the test; the words are.
        if is_codex_refusal(stdout) || is_codex_refusal(stderr) {
            return Verdict::Invalid(format!("the key was refused: {}", CliOutput { stdout, stderr }.reason(secrets)));
        }
        if success {
            return Verdict::Valid;
        }
        if stdout.is_rate_limited() || stderr.is_rate_limited() {
            return Verdict::Unverifiable("the account is rate-limited, so the key could not be checked".to_string());
        }
        Verdict::Unverifiable(format!(
            "the check itself failed, so the key was not tested: {}",
            CliOutput { stdout, stderr }.reason(secrets)
        ))
    }
}

impl Login for CodexLogin {
    /// The block, plus the translation `app-server` needs to read a key at all.
    ///
    /// Fallible, unlike claude's: codex cannot be handed a block as it stands,
    /// because `app-server` reads no key variable of its own, so the block has
    /// to be *translated* — and a translation needs something to translate.
    fn spawn_env(&self, kind: AccountKind, secret: &str, env: &[EnvVar]) -> Result<AccountLaunch, AccountError> {
        let mut vars = kind.vars(secret, env);
        let key = CODEX_KEY_CANDIDATES
            .iter()
            .find_map(|name| env.iter().find(|var| &var.key == name))
            .ok_or_else(|| {
                AccountError::Invalid(format!(
                    "a codex account needs {} (or {}) in its environment block: `codex app-server` reads neither, so Droppen has to hand the key to a model provider of its own",
                    CODEX_KEY_CANDIDATES[0], CODEX_KEY_CANDIDATES[1]
                ))
            })?;
        // On top of the block rather than instead of it: the variables the user
        // typed still reach the child, and this is the one the injected provider
        // is told to read.
        vars.push((CODEX_API_KEY_ENV.to_string(), key.value.clone()));
        // A blank one is "the vendor default", as it was when the base URL was
        // a field of its own: `OPENAI_BASE_URL=` is how a pasted block says it
        // is not overriding anything, and v0.2.58 read it that way.
        let named = env.iter().find(|var| var.key == CODEX_BASE_URL_VAR && !var.value.trim().is_empty());
        let base_url = match named {
            Some(var) => CodexBaseUrl::parse(&var.value)?,
            None => CodexBaseUrl::vendor_default(),
        };
        let codex_args = base_url.provider_args();
        Ok(AccountLaunch {
            provider: AccountProvider::Codex,
            env: vars,
            codex_args,
        })
    }

    /// One `codex exec ping` through the account's own model provider.
    fn verify<'a>(&'a self, paths: &'a Paths, launch: &'a AccountLaunch, secrets: &'a [&'a str]) -> BoxFuture<'a, Verdict> {
        Box::pin(self.check(paths, launch, secrets))
    }

    /// Codex has no reading of its login (its readings are another protocol's
    /// and say nothing about who is signed in), so a refused credential is
    /// the only evidence: signed out on one, unknown otherwise.
    fn local_state(&self, refused_at: Option<i64>, _local_reading: &SharedReading) -> LocalLoginState {
        if refused_at.is_some() {
            LocalLoginState::SignedOut
        } else {
            LocalLoginState::Unknown
        }
    }

    /// None: codex keeps no reading per login to refresh.
    fn probe<'a>(&'a self, _roots: &'a ProbeRoots, _config_dir: Option<&'a Path>) -> Option<BoxFuture<'a, UsageProbe>> {
        None
    }

    /// None: codex's plan arrives with its own login's quota reading.
    fn local_plan<'a>(&'a self, _paths: &'a Paths) -> Option<BoxFuture<'a, LocalPlan>> {
        None
    }

    /// None: codex names a login's plan itself (`planType`), so there is no
    /// pair to make a label of.
    fn plan_label(&self, _subscription: Option<&str>, _tier: Option<&str>) -> Option<String> {
        None
    }

    /// The app server's `account/rateLimits/read` when it can be reached, the
    /// newest reading in the session logs when not ([`CodexReadingExt::codex`]).
    fn local_quota<'a>(&'a self, roots: &'a ProbeRoots) -> Option<BoxFuture<'a, Option<CodexQuota>>> {
        Some(Box::pin(roots.codex()))
    }

    /// `codex app-server`, one handshake and one question ([`app_server_call`]).
    fn app_server_call<'a>(
        &'a self,
        program: &'a Path,
        cwd: &'a Path,
        env: &'a Env,
        method: &'a str,
        timeout: Duration,
    ) -> Option<BoxFuture<'a, Option<Value>>> {
        Some(Box::pin(app_server_call(program, cwd, env, method, timeout)))
    }

    /// A codex session stays on the account it started on.
    fn rotates(&self) -> bool {
        false
    }

    /// Codex's models name no family the limits know.
    fn model_family(&self, _model: Option<&str>) -> Option<&'static str> {
        None
    }

    /// Codex keeps one reading, of the user's own login ([`Login::local_quota`]).
    fn reads_per_login(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use droppen_base::env_block::parse_env_block;
    use droppen_accounts::verify::StubCli;
    use droppen_base::process_env::set_child_path;

    /// A codex session stays on its account, whatever its model, and its
    /// accounts carry no reading each.
    #[test]
    fn codex_sessions_never_rotate_and_name_no_family() {
        assert!(!CodexLogin.rotates());
        assert_eq!(CodexLogin.model_family(Some("opus")), None, "not even a model that looks like claude's");
        assert!(!CodexLogin.reads_per_login());
    }

    /// Each marker is enough on its own, and words about codex itself, the
    /// network or its arguments count for nothing.
    #[test]
    fn only_codex_talking_about_the_key_counts_as_a_refusal() {
        // One marker each, so a marker that goes missing turns its line into a miss.
        for text in ["invalid_api_key", "Incorrect API key", "Unauthorized", "Missing bearer", "Invalid API key", "status 401"] {
            assert!(is_codex_refusal(text), "{text}");
        }
        assert!(!is_codex_refusal("getaddrinfo ENOTFOUND api.openai.com"));
        assert!(!is_codex_refusal("error: unexpected argument '--skip-git-repo-check' found"));
        assert!(
            !is_codex_refusal("wrote 2401 tokens"),
            "a status code is three digits standing on their own, not any three digits"
        );
    }

    /// A finished check is judged by its words first, because `codex exec`
    /// can print a refusal and still exit zero; then by its exit status.
    #[test]
    fn a_finished_check_is_judged_by_its_words_and_then_its_exit() {
        let login = CodexLogin;
        let refused = login.verdict(true, "", "ERROR: 401 Unauthorized: invalid_api_key for not-a-real-key", &["not-a-real-key"]);
        let Verdict::Invalid(why) = refused else { panic!("a clean exit with a refusal is a refusal: {refused:?}") };
        assert!(why.starts_with("the key was refused: "), "{why}");
        assert!(!why.contains("not-a-real-key"), "the key is taken out: {why}");
        assert!(matches!(login.verdict(false, "", "Incorrect API key provided", &[]), Verdict::Invalid(_)));

        assert_eq!(login.verdict(true, "pong", "", &[]), Verdict::Valid);
        assert_eq!(
            login.verdict(false, "", "HTTP 429 Too Many Requests", &[]),
            Verdict::Unverifiable("the account is rate-limited, so the key could not be checked".to_string())
        );
        let failed = login.verdict(false, "", "getaddrinfo ENOTFOUND api.openai.com", &[]);
        let Verdict::Unverifiable(why) = failed else { panic!("no verdict: {failed:?}") };
        assert!(why.starts_with("the check itself failed, so the key was not tested: getaddrinfo ENOTFOUND"), "{why}");
    }

    /// A check that cannot run proves nothing, and says it in codex's words.
    #[tokio::test]
    async fn a_check_that_cannot_run_proves_nothing() {
        // An empty `PATH`, whatever this machine has installed, so the words
        // can only be codex's own check stopping. The override is this test
        // thread's own (`process_env`).
        let empty = tempfile::tempdir().expect("a PATH directory with nothing in it");
        set_child_path(Some(empty.path().to_string_lossy().into_owned()));
        let home = tempfile::tempdir().expect("tempdir");
        let paths = droppen_accounts::verify::paths_where_no_scratch_can_be_made(home.path());
        let launch = AccountLaunch {
            provider: AccountProvider::Codex,
            env: vec![("OPENAI_API_KEY".to_string(), "not-a-real-key".to_string())],
            codex_args: CodexBaseUrl::vendor_default().provider_args(),
        };
        let verdict = CodexLogin.verify(&paths, &launch, &[]).await;
        assert_eq!(
            verdict,
            Verdict::Unverifiable("the `codex` CLI is not on PATH, so the key cannot be checked".to_string())
        );
    }

    /// The check is one `codex exec ping` through the account's own model
    /// provider, with the key in the variable that provider reads, in a fresh
    /// `CODEX_HOME` and an empty working directory under the instance's data,
    /// both gone once it answers — and a refusal printed beside a clean exit
    /// is still a refusal, quoted back without the key.
    #[tokio::test]
    async fn the_check_runs_codex_through_the_accounts_own_provider_in_a_scratch_home() {
        let codex = StubCli::new(
            "codex",
            &["CODEX_HOME", "DROPPEN_CODEX_API_KEY"],
            "printf 'ERROR: 401 Unauthorized: invalid_api_key %s\\n' \"$DROPPEN_CODEX_API_KEY\" >&2\nexit 0\n",
        );
        set_child_path(Some(codex.path()));
        let home = tempfile::tempdir().expect("tempdir");
        // Canonical, because the stub reports its working directory physically.
        let paths = Paths::new(home.path().canonicalize().expect("canonical home"));
        let key = "not-a-real-openai-key";
        let block = parse_env_block(&format!("OPENAI_API_KEY={key}")).expect("block");
        let launch = CodexLogin.spawn_env(AccountKind::Env, "", &block).expect("a launch");

        let verdict = CodexLogin.verify(&paths, &launch, &[key]).await;
        assert_eq!(
            verdict,
            Verdict::Invalid("the key was refused: ERROR: 401 Unauthorized: invalid_api_key [redacted]".to_string())
        );
        assert_eq!(
            codex.recorded("argv").lines().collect::<Vec<_>>(),
            [
                "exec",
                "--skip-git-repo-check",
                "-c",
                "model_providers.remote_hand={name=\"remote-hand\",base_url=\"https://api.openai.com/v1\",env_key=\"DROPPEN_CODEX_API_KEY\",wire_api=\"responses\"}",
                "-c",
                "model_provider=remote_hand",
                "ping",
            ]
        );
        assert!(codex.recorded("DROPPEN_CODEX_API_KEY") == key, "the key reached the provider's variable");
        let codex_home = std::path::PathBuf::from(codex.recorded("CODEX_HOME"));
        let scratch = codex_home.parent().expect("the scratch");
        assert_eq!(scratch.parent(), Some(paths.data_dir().join("verify").as_path()));
        assert_eq!(codex_home, scratch.join("codex"));
        assert_eq!(std::path::PathBuf::from(codex.recorded("cwd").trim_end()), scratch.join("cwd"));
        assert!(!scratch.exists(), "the scratch goes once the check has answered");
    }
}
