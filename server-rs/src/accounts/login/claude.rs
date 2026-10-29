//! Claude Code's part of the accounts: how a credential starts `claude`, and
//! the check that proves one works before it is kept.

use futures_util::future::BoxFuture;

use super::{LocalPlan, Login};
use crate::accounts::local_login::LocalLoginState;
use crate::accounts::quota::readings::SharedReading;
use crate::accounts::quota::{claude_plan_label, is_rate_limited, mentions_status, CodexQuota, ProbeRoots, QuotaProbeError, UsageProbe};
use crate::accounts::store::{AccountError, AccountKind, AccountLaunch, AccountProvider};
use droppen_base::env_block::EnvVar;
use crate::accounts::verify::{reason, Scratch, Verdict, PROBE_PROMPT, PROBE_TIMEOUT};
use crate::claude_roots::ClaudeRoots;
use droppen_base::process_env::{apply_env, claude_env, clean_env_bare, resolve_on_path, Env};
use droppen_instance::paths::Paths;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// What `ANTHROPIC_BASE_URL` is called, for a claude account that names one.
pub const CLAUDE_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";

/// `--no-session-persistence` keeps the probe out of the session list, exactly
/// as the quota probe does.
const PROBE_ARGS: [&str; 5] = ["-p", PROBE_PROMPT, "--output-format", "json", "--no-session-persistence"];

/// The one misconfiguration whose symptom names the wrong thing entirely.
///
/// `ANTHROPIC_BASE_URL` is a *root*: the SDK inside claude appends
/// `/v1/messages` to it. A gateway's documentation almost always shows the
/// endpoint as `https://host/v1`, so pasting that in is the obvious move and
/// produces `https://host/v1/v1/messages` — a 404, which the CLI reports as
/// "There's an issue with the selected model". The user then spends their time
/// on the model picker, which is the one place the problem is not.
///
/// So it is said here, where the block is in hand, and said as a *prefix* to
/// whatever the CLI actually complained about rather than instead of it: this
/// is a guess about a plausible mistake, not a verdict, and the CLI's own words
/// are still the evidence.
const CLAUDE_BASE_URL_HINT: &str = "ANTHROPIC_BASE_URL ends with /v1 — claude appends /v1/messages itself, so drop the trailing /v1. ";

/// The hint, as what a launch can say about its own `ANTHROPIC_BASE_URL`.
///
/// An extension trait rather than inherent methods: the launch is the
/// accounts' type, and the hint is Claude Code's.
trait BaseUrlHintExt {
    /// Whether this launch's `ANTHROPIC_BASE_URL` is a root with `/v1` already on it.
    ///
    /// The scheme is stripped first so that `https://v1` — a host called `v1`, which
    /// is legal and would otherwise end in the same three characters — is not read
    /// as a path. A trailing slash is the same URL, so it goes too.
    fn base_url_has_v1(&self) -> bool;

    /// Put the hint in front of a verdict that did not go well.
    ///
    /// Never a refusal of its own: a `/v1` root is unusual rather than wrong — a
    /// relay is free to serve `/v1/v1/messages` — and this method only ever
    /// changes what a failure *says*. A credential that works is left alone, hint
    /// or no hint, because there is nothing to explain.
    fn with_base_url_hint(&self, verdict: Verdict) -> Verdict;
}

impl BaseUrlHintExt for AccountLaunch {
    fn base_url_has_v1(&self) -> bool {
        let Some((_, value)) = self.env.iter().find(|(key, _)| key == CLAUDE_BASE_URL_ENV) else {
            return false;
        };
        let url = value.trim().trim_end_matches('/');
        let path = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
        path.ends_with("/v1")
    }

    fn with_base_url_hint(&self, verdict: Verdict) -> Verdict {
        if !self.base_url_has_v1() {
            return verdict;
        }
        match verdict {
            Verdict::Valid => Verdict::Valid,
            Verdict::Invalid(reason) => Verdict::Invalid(format!("{CLAUDE_BASE_URL_HINT}{reason}")),
            Verdict::Unverifiable(why) => Verdict::Unverifiable(format!("{CLAUDE_BASE_URL_HINT}{why}")),
        }
    }
}

/// Whether the CLI's complaint is about the *credential* rather than about
/// itself, the network or its arguments.
///
/// Deliberately the narrow direction: a miss stores an unchecked credential and
/// says so, while a false positive refuses a working token with no way around it.
fn is_credential_refusal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: [&str; 12] = [
        "authentication",
        "authenticate",
        "unauthorized",
        "forbidden",
        "invalid api key",
        "invalid bearer",
        "invalid token",
        "invalid_api_key",
        "expired",
        "oauth",
        "/login",
        "not logged in",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker)) || mentions_status(&lower, "401", None) || mentions_status(&lower, "403", None)
}

/// Claude Code's [`Login`].
pub struct ClaudeCodeLogin;

impl ClaudeCodeLogin {
    /// One `claude -p ping` with the credential in the environment.
    async fn check(&self, paths: &Paths, launch: &AccountLaunch, secrets: &[&str]) -> Verdict {
        if resolve_on_path("claude", &clean_env_bare()).is_none() {
            return Verdict::Unverifiable("the `claude` CLI is not on PATH, so the token cannot be checked".to_string());
        }
        let scratch = match Scratch::make(paths).await {
            Ok(scratch) => scratch,
            Err(error) => return Verdict::Unverifiable(format!("cannot prepare the check's config directory: {error}")),
        };

        let mut command = tokio::process::Command::new("claude");
        command
            .args(PROBE_ARGS)
            .current_dir(scratch.cwd())
            .stdin(std::process::Stdio::null())
            // Dropping the future on timeout must take the CLI with it, or a probe
            // outlives the request that started it.
            .kill_on_drop(true);
        apply_env(
            &mut command,
            &claude_env(
                [("CLAUDE_CONFIG_DIR".to_string(), scratch.config_dir().to_string_lossy().into_owned())]
                    .into_iter()
                    .chain(launch.env.iter().cloned()),
            ),
        );

        let output = match tokio::time::timeout(PROBE_TIMEOUT, command.output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Verdict::Unverifiable(format!("the `claude` CLI could not be started: {error}")),
            Err(_) => return Verdict::Unverifiable("the check timed out before the CLI answered".to_string()),
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        self.verdict(output.status.success(), &stdout, &stderr, secrets)
    }

    /// What one finished `claude -p ping` says about the credential it ran
    /// with: a clean exit is a request that went through, and a failure is
    /// read for what the CLI says about it.
    fn verdict(&self, success: bool, stdout: &str, stderr: &str, secrets: &[&str]) -> Verdict {
        if success {
            return Verdict::Valid;
        }
        // An account over its request limit is a working account, and a refusal here
        // would send the user back through a wizard that cannot help.
        if is_rate_limited(stdout) || is_rate_limited(stderr) {
            return Verdict::Unverifiable("the account is rate-limited, so the token could not be checked".to_string());
        }
        let quoted = reason(stderr, stdout, secrets);
        // A non-zero exit is not a verdict on the credential unless the CLI said so.
        // An older CLI that does not know one of these flags, a DNS failure behind a
        // proxy, a crash — all exit non-zero, and turning any of them into a refusal
        // would lock the user out of adding an account at all.
        if is_credential_refusal(stdout) || is_credential_refusal(stderr) {
            return Verdict::Invalid(format!("the token was refused: {quoted}"));
        }
        Verdict::Unverifiable(format!("the check itself failed, so the token was not tested: {quoted}"))
    }
}

impl Login for ClaudeCodeLogin {
    /// The credential as it stands, and nothing on the command line: claude
    /// reads a token or a block out of its environment by itself.
    ///
    /// Never refused. A block is handed over as it stands — a token, a
    /// gateway's four variables, a Bedrock setup with no token at all: the CLI
    /// is the thing that knows which of those work, and second-guessing it here
    /// would refuse configurations that are fine.
    fn spawn_env(&self, kind: AccountKind, secret: &str, env: &[EnvVar]) -> Result<AccountLaunch, AccountError> {
        Ok(AccountLaunch {
            provider: AccountProvider::Claude,
            env: kind.vars(secret, env),
            codex_args: Vec::new(),
        })
    }

    /// One `claude -p ping`, with a hint in front of a failure when the block's
    /// `ANTHROPIC_BASE_URL` already ends in `/v1`.
    fn verify<'a>(&'a self, paths: &'a Paths, launch: &'a AccountLaunch, secrets: &'a [&'a str]) -> BoxFuture<'a, Verdict> {
        Box::pin(async move { launch.with_base_url_hint(self.check(paths, launch, secrets).await) })
    }

    /// The local login's state from its evidence, newest first: a refused
    /// credential (`refused_at`, when an active `auth` rest on it was seen), a
    /// `/usage` probe that found nobody signed in, and every reading a session
    /// or a probe made (`local_reading`, Claude Code's own reading of that
    /// login). Ties go to the refusal, the direction that costs a click rather
    /// than a failed session.
    fn local_state(&self, refused_at: Option<i64>, local_reading: &SharedReading) -> LocalLoginState {
        let refused_by_probe = match (local_reading.probe_error, local_reading.probe_error_at_ms) {
            // `probe_failed` only says the reading did not happen.
            (Some(QuotaProbeError::NotLoggedIn), Some(at)) => Some(at),
            _ => None,
        };
        let refused = refused_at.max(refused_by_probe);
        let worked = local_reading.newest().map(|window| window.read_at_ms);
        match (refused, worked) {
            (Some(refused), Some(worked)) if worked > refused => LocalLoginState::SignedIn,
            (Some(_), _) => LocalLoginState::SignedOut,
            (None, Some(_)) => LocalLoginState::SignedIn,
            (None, None) => LocalLoginState::Unknown,
        }
    }

    /// `claude -p /usage` against that config root ([`ProbeRoots::claude_usage`]).
    fn probe<'a>(&'a self, roots: &'a ProbeRoots, config_dir: Option<&'a Path>) -> Option<BoxFuture<'a, UsageProbe>> {
        Some(Box::pin(roots.claude_usage(config_dir)))
    }

    /// The two fields of the user's own `~/.claude/.credentials.json` that
    /// name the plan.
    fn local_plan<'a>(&'a self, paths: &'a Paths) -> Option<BoxFuture<'a, LocalPlan>> {
        Some(Box::pin(async move {
            let mut plan = None;
            let mut tier = None;
            // Read-only, and only these two fields: the credentials file is the
            // user's own login and nothing here may touch the rest of it.
            if let Ok(raw) = tokio::fs::read_to_string(ClaudeRoots::of(paths).local_credentials()).await {
                if let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&raw) {
                    if let Some(Value::Object(oauth)) = root.get("claudeAiOauth") {
                        let subscription_type = oauth.get("subscriptionType").and_then(Value::as_str).filter(|s| !s.is_empty());
                        tier = oauth
                            .get("rateLimitTier")
                            .and_then(Value::as_str)
                            .filter(|t| !t.is_empty())
                            .map(str::to_owned);
                        if subscription_type.is_some() || tier.is_some() {
                            plan = Some(claude_plan_label(subscription_type, tier.as_deref()));
                        }
                    }
                }
            }
            LocalPlan { plan, tier }
        }))
    }

    /// None: claude's quota is read per login, by [`Login::probe`] and by its
    /// sessions.
    fn local_quota<'a>(&'a self, _roots: &'a ProbeRoots) -> Option<BoxFuture<'a, Option<CodexQuota>>> {
        None
    }

    /// None: claude has no app server; its login is asked on its command line.
    fn app_server_call<'a>(
        &'a self,
        _program: &'a Path,
        _cwd: &'a Path,
        _env: &'a Env,
        _method: &'a str,
        _timeout: Duration,
    ) -> Option<BoxFuture<'a, Option<Value>>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use droppen_base::env_block::parse_env_block;
    use crate::accounts::verify::StubCli;
    use droppen_base::process_env::set_child_path;
    use crate::accounts::quota::{QuotaReadingSource, QuotaWindow};

    /// A CLI that fell over is not a CLI that turned the credential down.
    ///
    /// The failure mode being kept out: an older `claude` that does not know one
    /// of the probe's flags exits non-zero on *every* token, and reading that as
    /// a refusal would lock the user out of adding an account at all.
    #[test]
    fn only_the_cli_talking_about_credentials_counts_as_a_refusal() {
        assert!(is_credential_refusal("Invalid bearer token"));
        assert!(is_credential_refusal("API Error: 401 {\"type\":\"error\"}"));
        assert!(is_credential_refusal("Authentication failed"));
        assert!(is_credential_refusal("OAuth token has expired · please run /login"));
        assert!(is_credential_refusal("403 Forbidden"));

        assert!(!is_credential_refusal("error: unknown option '--no-session-persistence'"));
        assert!(!is_credential_refusal("getaddrinfo ENOTFOUND api.anthropic.com"));
        assert!(!is_credential_refusal("Killed: 9"));
        assert!(
            !is_credential_refusal("wrote 2401 tokens to the transcript"),
            "a status code is three digits standing on their own, not any three digits"
        );
    }

    /// Each marker is enough on its own: every line here is matched by exactly
    /// one of them, so a marker that goes missing turns its line into a miss.
    #[test]
    fn each_refusal_marker_counts_on_its_own() {
        for text in [
            "authentication_error",
            "could not authenticate",
            "Request failed: Unauthorized",
            "Forbidden",
            "Invalid API key",
            "invalid bearer",
            "Invalid token",
            "invalid_api_key",
            "Session expired",
            "OAuth error",
            "Please run /login",
            "Not logged in",
            "API Error: 401",
            "API Error: 403",
        ] {
            assert!(is_credential_refusal(text), "{text}");
        }
    }

    /// A finished check is judged by its exit status first, then, on a
    /// failure, by its words: over its limit is a working account, a refusal
    /// is quoted without the credential, and anything else proves nothing.
    #[test]
    fn a_finished_check_is_judged_by_its_exit_and_then_its_words() {
        let login = ClaudeCodeLogin;
        assert_eq!(login.verdict(true, "{\"result\":\"pong\"}", "", &[]), Verdict::Valid);
        assert_eq!(
            login.verdict(true, "", "unauthorized", &[]),
            Verdict::Valid,
            "a clean exit is a request that went through, whatever was printed beside it"
        );

        let refused = login.verdict(false, "", "API Error: 401 Unauthorized for not-a-real-token", &["not-a-real-token"]);
        let Verdict::Invalid(why) = refused else { panic!("a refusal: {refused:?}") };
        assert!(why.starts_with("the token was refused: "), "{why}");
        assert!(!why.contains("not-a-real-token"), "the credential is taken out: {why}");

        assert_eq!(
            login.verdict(false, "", "HTTP 429 Too Many Requests: unauthorized", &[]),
            Verdict::Unverifiable("the account is rate-limited, so the token could not be checked".to_string()),
            "a rate limit outranks words that would otherwise be a refusal"
        );
        let failed = login.verdict(false, "", "getaddrinfo ENOTFOUND api.anthropic.com", &[]);
        let Verdict::Unverifiable(why) = failed else { panic!("no verdict: {failed:?}") };
        assert!(why.starts_with("the check itself failed, so the token was not tested: getaddrinfo ENOTFOUND"), "{why}");
    }

    /// The hint goes in front of whatever the check concludes, including the
    /// verdicts it reaches without running the CLI, and only for a `/v1` root.
    #[tokio::test]
    async fn a_check_that_cannot_run_proves_nothing_and_still_carries_the_hint() {
        // An empty `PATH`, whatever this machine has installed, so the words
        // can only be claude's own check stopping. The override is this test
        // thread's own (`process_env`).
        let empty = tempfile::tempdir().expect("a PATH directory with nothing in it");
        set_child_path(Some(empty.path().to_string_lossy().into_owned()));
        let home = tempfile::tempdir().expect("tempdir");
        let paths = crate::accounts::verify::paths_where_no_scratch_can_be_made(home.path());
        let not_on_path = "the `claude` CLI is not on PATH, so the token cannot be checked";

        let verdict = ClaudeCodeLogin.verify(&paths, &launch("https://gw.example.com/v1"), &[]).await;
        assert_eq!(verdict, Verdict::Unverifiable(format!("{CLAUDE_BASE_URL_HINT}{not_on_path}")), "the hint goes first");

        let verdict = ClaudeCodeLogin.verify(&paths, &launch("https://gw.example.com"), &[]).await;
        assert_eq!(verdict, Verdict::Unverifiable(not_on_path.to_string()), "a root gets no hint");
    }

    /// The check is one `claude -p ping` with the probe's own arguments, in a
    /// fresh config directory and an empty working directory under the
    /// instance's data, both gone once it answers, with the credential in its
    /// environment — and a refusal is quoted back without that credential.
    #[tokio::test]
    async fn the_check_runs_claude_in_a_scratch_of_its_own() {
        let claude = StubCli::new(
            "claude",
            &["CLAUDE_CONFIG_DIR", "ANTHROPIC_AUTH_TOKEN"],
            "printf 'API Error: 401 Unauthorized for %s\\n' \"$ANTHROPIC_AUTH_TOKEN\" >&2\nexit 1\n",
        );
        set_child_path(Some(claude.path()));
        let home = tempfile::tempdir().expect("tempdir");
        // Canonical, because the stub reports its working directory physically.
        let paths = Paths::new(home.path().canonicalize().expect("canonical home"));
        let token = "not-a-real-gateway-token";
        let block = parse_env_block(&format!("ANTHROPIC_AUTH_TOKEN={token}\nANTHROPIC_BASE_URL=https://gw.example.com")).expect("block");
        let launch = ClaudeCodeLogin.spawn_env(AccountKind::Env, "", &block).expect("a launch");

        let verdict = ClaudeCodeLogin.verify(&paths, &launch, &[token]).await;
        assert_eq!(
            verdict,
            Verdict::Invalid("the token was refused: API Error: 401 Unauthorized for [redacted]".to_string())
        );
        assert_eq!(
            claude.recorded("argv").lines().collect::<Vec<_>>(),
            ["-p", "ping", "--output-format", "json", "--no-session-persistence"]
        );
        assert!(claude.recorded("ANTHROPIC_AUTH_TOKEN") == token, "the credential reached the CLI");
        let config = std::path::PathBuf::from(claude.recorded("CLAUDE_CONFIG_DIR"));
        let scratch = config.parent().expect("the scratch");
        assert_eq!(scratch.parent(), Some(paths.data_dir().join("verify").as_path()));
        assert_eq!(config, scratch.join("claude"));
        assert_eq!(std::path::PathBuf::from(claude.recorded("cwd").trim_end()), scratch.join("cwd"));
        assert!(!scratch.exists(), "the scratch goes once the check has answered");
    }

    /// A block whose base URL already carries the path claude appends.
    fn launch(base_url: &str) -> AccountLaunch {
        AccountLaunch {
            provider: AccountProvider::Claude,
            env: vec![
                ("ANTHROPIC_AUTH_TOKEN".to_string(), "hunter2-gateway-issued-value".to_string()),
                (CLAUDE_BASE_URL_ENV.to_string(), base_url.to_string()),
            ],
            codex_args: Vec::new(),
        }
    }

    /// The 404 that reads as a model problem: `<base>/v1` + `/v1/messages`.
    #[test]
    fn a_base_url_that_already_ends_in_v1_is_pointed_out_before_the_clis_own_words() {
        for url in [
            "https://gw.example.com/v1",
            "https://gw.example.com/v1/",
            " https://gw.example.com/api/v1 ",
            "http://10.0.0.2:8000/v1",
        ] {
            assert!(launch(url).base_url_has_v1(), "{url} needs the hint");
        }
        let refused = launch("https://gw/v1").with_base_url_hint(Verdict::Invalid("the token was refused: 401".to_string()));
        assert_eq!(
            refused,
            Verdict::Invalid(format!("{CLAUDE_BASE_URL_HINT}the token was refused: 401")),
            "a prefix, not a replacement: the CLI's own words are still the evidence"
        );
        let unchecked = launch("https://gw/v1/").with_base_url_hint(Verdict::Unverifiable("the check itself failed".to_string()));
        assert_eq!(unchecked, Verdict::Unverifiable(format!("{CLAUDE_BASE_URL_HINT}the check itself failed")));
        // And it is a hint, never a refusal: a relay is free to serve
        // /v1/v1/messages, and one that works is one that works.
        assert_eq!(launch("https://gw/v1").with_base_url_hint(Verdict::Valid), Verdict::Valid);
    }

    #[test]
    fn a_base_url_that_is_already_a_root_is_left_to_speak_for_itself() {
        for url in [
            "https://gw.example.com",
            "https://gw.example.com/",
            "https://gw.example.com/anthropic",
            "https://gw.example.com/v1/messages",
            "https://v1",
            "https://v2",
        ] {
            assert!(!launch(url).base_url_has_v1(), "{url} must not be second-guessed");
        }
        // An OAuth account names no base URL at all.
        let oauth = AccountLaunch {
            provider: AccountProvider::Claude,
            env: vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-ant-oat01-whatever".to_string())],
            codex_args: Vec::new(),
        };
        assert!(!oauth.base_url_has_v1());
        let verdict = Verdict::Invalid("the token was refused".to_string());
        assert_eq!(oauth.with_base_url_hint(verdict.clone()), verdict);
    }

    const MINUTE: i64 = 60_000;

    fn window(label: &str) -> QuotaWindow {
        QuotaWindow {
            label: label.to_string(),
            used_percent: 10.0,
            remaining_percent: 90.0,
            resets_at: droppen_http::openapi_ext::Nullable(None),
            window_minutes: 300,
            updated_at: droppen_http::openapi_ext::Optional(None),
        }
    }

    fn read_at(at: i64) -> SharedReading {
        let mut reading = SharedReading::default();
        reading.merge(vec![window("5h")], at, QuotaReadingSource::Session);
        reading
    }

    fn not_logged_in_at(mut reading: SharedReading, at: i64) -> SharedReading {
        reading.probe_error = Some(QuotaProbeError::NotLoggedIn);
        reading.probe_error_at_ms = Some(at);
        reading
    }

    #[test]
    fn nothing_known_is_unknown_and_a_reading_is_signed_in() {
        assert_eq!(ClaudeCodeLogin.local_state(None, &SharedReading::default()), LocalLoginState::Unknown);
        assert_eq!(ClaudeCodeLogin.local_state(None, &read_at(5 * MINUTE)), LocalLoginState::SignedIn);
    }

    /// A login whose OAuth token expired weeks ago: the probe found nobody
    /// signed in, a turn on it was refused, and nothing has worked since.
    #[test]
    fn a_refused_credential_or_an_empty_probe_is_signed_out() {
        let old_reading = read_at(MINUTE);
        assert_eq!(ClaudeCodeLogin.local_state(Some(10 * MINUTE), &old_reading), LocalLoginState::SignedOut);
        assert_eq!(
            ClaudeCodeLogin.local_state(None, &not_logged_in_at(old_reading.clone(), 10 * MINUTE)),
            LocalLoginState::SignedOut,
            "the probe's verdict alone is enough"
        );
        assert_eq!(ClaudeCodeLogin.local_state(Some(10 * MINUTE), &SharedReading::default()), LocalLoginState::SignedOut);
        assert_eq!(
            ClaudeCodeLogin.local_state(Some(10 * MINUTE), &read_at(10 * MINUTE)),
            LocalLoginState::SignedOut,
            "a tie goes to the refusal"
        );
    }

    /// Signing in again and running one turn is the whole fix: the reading that
    /// turn makes is newer than the rest, and nobody has to clear anything.
    #[test]
    fn a_reading_newer_than_the_refusal_heals_it() {
        assert_eq!(ClaudeCodeLogin.local_state(Some(MINUTE), &read_at(10 * MINUTE)), LocalLoginState::SignedIn);
        assert_eq!(
            ClaudeCodeLogin.local_state(None, &not_logged_in_at(read_at(10 * MINUTE), MINUTE)),
            LocalLoginState::SignedIn
        );
    }

    /// A probe that failed to run says nothing about the login.
    #[test]
    fn a_probe_that_failed_to_run_is_not_a_verdict() {
        let reading = SharedReading {
            probe_error: Some(QuotaProbeError::ProbeFailed),
            probe_error_at_ms: Some(MINUTE),
            ..Default::default()
        };
        assert_eq!(ClaudeCodeLogin.local_state(None, &reading), LocalLoginState::Unknown);
    }
}
