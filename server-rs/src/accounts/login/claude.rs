//! Claude Code's part of the accounts: how a credential starts `claude`, and
//! the check that proves one works before it is kept.

use futures_util::future::BoxFuture;

use super::Login;
use crate::accounts::quota::{is_rate_limited, mentions_status};
use crate::accounts::store::{AccountError, AccountKind, AccountLaunch, AccountProvider};
use droppen_base::env_block::EnvVar;
use crate::accounts::verify::{reason, Scratch, Verdict, PROBE_PROMPT, PROBE_TIMEOUT};
use droppen_instance::paths::Paths;
use droppen_base::process_env::{apply_env, claude_env, clean_env_bare, resolve_on_path};

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
    /// relay is free to serve `/v1/v1/messages` — and this function only ever
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let home = tempfile::tempdir().expect("tempdir");
        let paths = crate::accounts::verify::paths_where_no_scratch_can_be_made(home.path());
        // It stops before starting anything either way: claude is not on
        // `PATH` here, or it is and the scratch directory cannot be made.
        let claudes_own = |why: &str| {
            why == "the `claude` CLI is not on PATH, so the token cannot be checked"
                || why.starts_with("cannot prepare the check's config directory: ")
        };

        let verdict = ClaudeCodeLogin.verify(&paths, &launch("https://gw.example.com/v1"), &[]).await;
        let Verdict::Unverifiable(why) = verdict else { panic!("a check that cannot run proves nothing: {verdict:?}") };
        let rest = why.strip_prefix(CLAUDE_BASE_URL_HINT).unwrap_or_else(|| panic!("the hint goes first: {why}"));
        assert!(claudes_own(rest), "{rest}");

        let verdict = ClaudeCodeLogin.verify(&paths, &launch("https://gw.example.com"), &[]).await;
        let Verdict::Unverifiable(why) = verdict else { panic!("a check that cannot run proves nothing: {verdict:?}") };
        assert!(claudes_own(&why), "a root gets no hint: {why}");
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
}
