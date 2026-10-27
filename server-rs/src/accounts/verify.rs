//! Does this credential actually work? Asked of Anthropic, not of a regex.
//!
//! Everything else in [`super`] takes a secret on trust: the store checks its
//! *shape* (non-empty, no whitespace, not absurdly long) and files it. That was
//! enough right up until the token wizard handed over a token the terminal had
//! wrapped, whose first 101 characters look exactly like a whole one — and every
//! session started under that account then failed with a 401 the user could only
//! read as "remote-hand is broken".
//!
//! So a credential is now *used* once before it is kept. The probe is the
//! cheapest real thing there is: one `claude -p` with the credential in the
//! environment, in a config directory holding no login of its own, so the only
//! way it can succeed is the credential itself.
//!
//! # Three answers, not two
//!
//! A probe that did not run says nothing about the credential. `claude` may not
//! be installed, the machine may be offline, the account may be rate-limited —
//! refusing a perfectly good token for any of those would be a worse bug than
//! the one this exists to fix. So the verdict is three-valued, and only
//! [`Verdict::Invalid`] — the CLI ran and Anthropic turned the credential down —
//! keeps a secret out of the store.
//!
//! A non-zero exit is not by itself a refusal, and reading it as one is the
//! failure this splits hairs to avoid: an older CLI that does not know one of
//! the probe's flags, a DNS failure behind a proxy, a crash, all exit non-zero
//! on *every* token, and a user whose CLI is one version behind would be unable
//! to add an account at all. So the output has to say something about
//! credentials ([`is_credential_refusal`]) before a refusal is believed.
//!
//! What the caller does with [`Verdict::Unverifiable`] is the caller's business,
//! and the two callers differ: a pasted credential is stored with the reason
//! attached, so the UI can say "saved, but nothing proved this works", while a
//! capture with two possible readings refuses to guess — see
//! [`super::setup_token::choose_reading`].

use std::path::PathBuf;
use std::time::Duration;

use crate::process_env::{apply_env, claude_env, clean_env, clean_env_bare, redact_secrets, redact_values, resolve_on_path};
use crate::quota::{is_rate_limited, mentions_status};
use crate::sessions::Paths;

use super::store::{launch_for, AccountLaunch, AccountProvider, Credential, CLAUDE_BASE_URL_ENV};

/// The prompt the probe sends. Short on purpose: it is billed like any other.
const PROBE_PROMPT: &str = "ping";
/// What a codex probe runs. `--skip-git-repo-check` because the scratch
/// directory is deliberately not a repository, and the account's own provider
/// override is appended by [`AccountLaunch::codex_args`].
const CODEX_PROBE_ARGS: [&str; 2] = ["exec", "--skip-git-repo-check"];
/// `--no-session-persistence` keeps the probe out of the session list, exactly
/// as the quota probe does.
const PROBE_ARGS: [&str; 5] = ["-p", PROBE_PROMPT, "--output-format", "json", "--no-session-persistence"];
/// A cold `claude` start plus one round trip, with room for a slow link.
///
/// Under a minute on purpose: this runs inside a `POST`/`PATCH`, and the reverse
/// proxy in front of a typical deployment gives up on a response at 60 s. A
/// check that outlived the request it belongs to would reach the user as a
/// gateway error rather than as a verdict.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// How much of a failing CLI's output is quoted back to the user.
const MAX_REASON: usize = 300;
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

/// What one probe concluded about a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The CLI ran a real request with this credential and it worked.
    Valid,
    /// The CLI ran and the credential was turned down. Carries the CLI's own
    /// words, redacted, so the UI can say *why*.
    Invalid(String),
    /// The probe did not happen. Says nothing about the credential.
    Unverifiable(String),
}

/// The first few lines of whatever the CLI complained about, fit to be shown.
///
/// A failing CLI happily echoes the credential it was handed ("Authentication
/// failed for sk-ant-…"), so the redaction is not optional: this string ends up
/// in an API response and in the capture status the browser polls.
fn reason(stderr: &str, stdout: &str, secrets: &[&str]) -> String {
    let raw = if stderr.trim().is_empty() { stdout } else { stderr };
    // The credential being checked is known *here*, before anything has been
    // stored, so it is taken out by value rather than guessed at by shape —
    // every value of the block, because any of them can be the one the CLI
    // quotes back. It has to be done this way: a gateway's bearer token and a
    // relay key are arbitrary strings, and the pattern below can only find the
    // ones that look like credentials. The pattern still runs, for the ones this probe
    // was never handed — a `CLAUDE_CONFIG_DIR` login the CLI mentions, a key in
    // the CLI's own diagnostics.
    let raw = redact_values(raw, secrets.iter().copied());
    let text = redact_secrets(&raw)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if text.is_empty() {
        return "the claude CLI rejected the token without saying why".to_string();
    }
    if text.len() <= MAX_REASON {
        return text;
    }
    let mut cut = MAX_REASON;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_string()
}

/// Where a probe runs: a config directory with no login of its own, plus an
/// empty working directory so the CLI adopts no project of the user's.
///
/// Fresh per probe, and removed afterwards. It must not be the managed root: the
/// probe must answer for this token alone, with no login of the root's own in
/// reach and no session state left behind in a root that real sessions use.
struct Scratch(PathBuf);

impl Scratch {
    async fn make(paths: &Paths) -> std::io::Result<Self> {
        let root = paths.data_dir().join("verify").join(uuid::Uuid::new_v4().to_string());
        tokio::fs::create_dir_all(root.join("claude")).await?;
        tokio::fs::create_dir_all(root.join("cwd")).await?;
        tokio::fs::create_dir_all(root.join("codex")).await?;
        Ok(Self(root))
    }

    fn config_dir(&self) -> PathBuf {
        self.0.join("claude")
    }

    fn cwd(&self) -> PathBuf {
        self.0.join("cwd")
    }

    /// `CODEX_HOME` for a codex probe. Created by [`Scratch::make`], never left
    /// to the CLI: `codex` exits with "Error finding codex home" rather than
    /// creating a `CODEX_HOME` that is not there.
    fn codex_home(&self) -> PathBuf {
        self.0.join("codex")
    }
}

/// The cleanup is a `Drop` rather than a call at the end of the happy path,
/// because there is no happy path to put it on: every early return above is a
/// probe that failed, and those are exactly the runs that would leave a
/// directory behind. Synchronous removal inside an async fn is fine here — the
/// tree is two empty directories plus whatever the CLI wrote into a throwaway
/// config, so it is a handful of syscalls, not a walk of a project.
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Spend one real request proving the credential works.
///
/// Never panics and never fails: everything that is not a clean run or a clean
/// refusal comes back as [`Verdict::Unverifiable`].
pub async fn verify_secret(paths: &Paths, provider: AccountProvider, credential: &Credential) -> Verdict {
    let (secret, env) = match credential {
        Credential::Oauth(secret) => (secret.as_str(), Vec::new()),
        Credential::Env(env) => ("", env.clone()),
    };
    // A block that cannot be turned into a spawn is refused outright rather
    // than probed: there is nothing to run. Only codex can say this, and only
    // about a block with no key variable in it.
    let launch = match launch_for(provider, credential.kind(), secret, &env) {
        Ok(launch) => launch,
        Err(error) => return Verdict::Invalid(error.to_string()),
    };
    let values = credential.values();
    match provider {
        AccountProvider::Claude => with_base_url_hint(verify_claude(paths, &launch, &values).await, &launch),
        AccountProvider::Codex => verify_codex(paths, &launch, &values).await,
    }
}

/// Whether this launch's `ANTHROPIC_BASE_URL` is a root with `/v1` already on it.
///
/// The scheme is stripped first so that `https://v1` — a host called `v1`, which
/// is legal and would otherwise end in the same three characters — is not read
/// as a path. A trailing slash is the same URL, so it goes too.
fn base_url_has_v1(env: &[(String, String)]) -> bool {
    let Some((_, value)) = env.iter().find(|(key, _)| key == CLAUDE_BASE_URL_ENV) else {
        return false;
    };
    let url = value.trim().trim_end_matches('/');
    let path = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
    path.ends_with("/v1")
}

/// Put the hint in front of a verdict that did not go well.
///
/// Never a refusal of its own: a `/v1` root is unusual rather than wrong — a
/// relay is free to serve `/v1/v1/messages` — and this function only ever
/// changes what a failure *says*. A credential that works is left alone, hint
/// or no hint, because there is nothing to explain.
fn with_base_url_hint(verdict: Verdict, launch: &AccountLaunch) -> Verdict {
    if !base_url_has_v1(&launch.env) {
        return verdict;
    }
    match verdict {
        Verdict::Valid => Verdict::Valid,
        Verdict::Invalid(reason) => Verdict::Invalid(format!("{CLAUDE_BASE_URL_HINT}{reason}")),
        Verdict::Unverifiable(why) => Verdict::Unverifiable(format!("{CLAUDE_BASE_URL_HINT}{why}")),
    }
}

/// One `claude -p ping` with the credential in the environment.
async fn verify_claude(paths: &Paths, launch: &AccountLaunch, secrets: &[&str]) -> Verdict {
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
    if output.status.success() {
        return Verdict::Valid;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // An account over its request limit is a working account, and a refusal here
    // would send the user back through a wizard that cannot help.
    if is_rate_limited(&stdout) || is_rate_limited(&stderr) {
        return Verdict::Unverifiable("the account is rate-limited, so the token could not be checked".to_string());
    }
    let quoted = reason(&stderr, &stdout, secrets);
    // A non-zero exit is not a verdict on the credential unless the CLI said so.
    // An older CLI that does not know one of these flags, a DNS failure behind a
    // proxy, a crash — all exit non-zero, and turning any of them into a refusal
    // would lock the user out of adding an account at all.
    if is_credential_refusal(&stdout) || is_credential_refusal(&stderr) {
        return Verdict::Invalid(format!("the token was refused: {quoted}"));
    }
    Verdict::Unverifiable(format!("the check itself failed, so the token was not tested: {quoted}"))
}

/// One `codex exec ping` through the account's own model provider.
///
/// Two deliberate differences from the claude probe, both about *not touching
/// the user's codex state*:
///
/// * a scratch `CODEX_HOME`, so the run's rollout lands in a directory that is
///   deleted afterwards rather than appearing in the session list — and so the
///   probe cannot fall back on the user's own `auth.json` and answer for a login
///   that is not the one being checked;
/// * the provider override from [`crate::accounts::store::codex_provider_args`],
///   which is the whole of how a key reaches codex: `app-server` reads neither
///   `OPENAI_API_KEY` nor `CODEX_API_KEY`, so a probe that only set a variable
///   would be checking nothing at all.
async fn verify_codex(paths: &Paths, launch: &AccountLaunch, secrets: &[&str]) -> Verdict {
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
    // `codex exec` reports a refusal on stderr and still exits zero often
    // enough that the exit code cannot be the test; the words are.
    if is_codex_refusal(&stdout) || is_codex_refusal(&stderr) {
        return Verdict::Invalid(format!("the key was refused: {}", reason(&stderr, &stdout, secrets)));
    }
    if output.status.success() {
        return Verdict::Valid;
    }
    if is_rate_limited(&stdout) || is_rate_limited(&stderr) {
        return Verdict::Unverifiable("the account is rate-limited, so the key could not be checked".to_string());
    }
    Verdict::Unverifiable(format!(
        "the check itself failed, so the key was not tested: {}",
        reason(&stderr, &stdout, secrets)
    ))
}

/// Whether codex is complaining about the *key* rather than about itself.
///
/// The same narrow direction as [`is_credential_refusal`]: a miss stores an
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
    MARKERS.iter().any(|marker| lower.contains(marker)) || mentions_status(&lower, "401", None)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_is_quoted_back_without_the_credential_in_it() {
        let text = reason("Authentication failed for sk-ant-oat01-AbCdEfGhIjKlMnOp\n\nrun /login\n", "", &[]);
        assert!(!text.contains("AbCdEfGhIjKlMnOp"), "the secret must never reach a response: {text}");
        assert!(text.contains("[redacted]"), "{text}");
        assert_eq!(text, "Authentication failed for sk-ant-\u{2026}[redacted] · run /login");
    }

    /// The credential being checked is not in the store yet — a refusal never
    /// stores anything — and an `auth_token` or a relay key looks like nothing
    /// a pattern could find. So the probe redacts the one value it was handed.
    #[test]
    fn the_credential_being_probed_is_removed_by_value_before_it_is_ever_stored() {
        let secret = "hunter2-gateway-issued-value";
        let text = reason(&format!("401 unauthorized for {secret} at https://gw/v1"), "", &[secret]);
        assert!(!text.contains("hunter2"), "the value must not reach a 400 body: {text}");
        assert_eq!(text, "401 unauthorized for [redacted] at https://gw/v1");

        // And the pattern still runs underneath, for credentials this probe was
        // never handed — a login the CLI found for itself, a key in its output.
        let both = reason(
            "bad token hunter2-gateway-issued-value; config has sk-ant-oat01-AbCdEfGhIj",
            "",
            &[secret],
        );
        assert!(!both.contains("hunter2") && !both.contains("AbCdEfGhIj"), "{both}");

        // A short "secret" is a word, and replacing every occurrence of one
        // would shred the sentence it appears in.
        assert_eq!(redact_values("the key is bad", ["key"]), "the key is bad");
        assert_eq!(redact_values("nothing here", [secret]), "nothing here");
    }

    #[test]
    fn stdout_is_used_when_the_cli_said_nothing_on_stderr() {
        assert_eq!(reason("  \n", "{\"error\":\"invalid token\"}", &[]), "{\"error\":\"invalid token\"}");
        assert_eq!(reason("", "", &[]), "the claude CLI rejected the token without saying why");
    }

    #[test]
    fn a_wall_of_output_is_cut_to_something_showable() {
        let text = reason(&"x".repeat(4096), "", &[]);
        assert!(text.len() <= MAX_REASON, "cut to {} characters", text.len());
    }

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
            assert!(base_url_has_v1(&launch(url).env), "{url} needs the hint");
        }
        let refused = with_base_url_hint(Verdict::Invalid("the token was refused: 401".to_string()), &launch("https://gw/v1"));
        assert_eq!(
            refused,
            Verdict::Invalid(format!("{CLAUDE_BASE_URL_HINT}the token was refused: 401")),
            "a prefix, not a replacement: the CLI's own words are still the evidence"
        );
        let unchecked = with_base_url_hint(Verdict::Unverifiable("the check itself failed".to_string()), &launch("https://gw/v1/"));
        assert_eq!(unchecked, Verdict::Unverifiable(format!("{CLAUDE_BASE_URL_HINT}the check itself failed")));
        // And it is a hint, never a refusal: a relay is free to serve
        // /v1/v1/messages, and one that works is one that works.
        assert_eq!(with_base_url_hint(Verdict::Valid, &launch("https://gw/v1")), Verdict::Valid);
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
            assert!(!base_url_has_v1(&launch(url).env), "{url} must not be second-guessed");
        }
        // An OAuth account names no base URL at all.
        let oauth = AccountLaunch {
            provider: AccountProvider::Claude,
            env: vec![("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "sk-ant-oat01-whatever".to_string())],
            codex_args: Vec::new(),
        };
        assert!(!base_url_has_v1(&oauth.env));
        let verdict = Verdict::Invalid("the token was refused".to_string());
        assert_eq!(with_base_url_hint(verdict.clone(), &oauth), verdict);
    }

    /// `codex` refuses to start at all when `CODEX_HOME` names a path that is
    /// not there — "Error finding codex home: … but that path does not exist",
    /// verified against codex-cli 0.154.0. A scratch that left the directory to
    /// the CLI would therefore make *every* codex check answer `Unverifiable`
    /// without ever testing the key, which is the one thing a probe exists not
    /// to do.
    #[tokio::test]
    async fn a_scratch_has_both_clis_homes_on_disk_before_either_is_started() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let scratch = Scratch::make(&paths).await.expect("scratch");
        assert!(scratch.config_dir().is_dir(), "CLAUDE_CONFIG_DIR");
        assert!(scratch.cwd().is_dir(), "the working directory");
        assert!(scratch.codex_home().is_dir(), "CODEX_HOME: codex will not start without it");

        let root = scratch.codex_home().parent().expect("root").to_path_buf();
        drop(scratch);
        assert!(!root.exists(), "and the whole probe directory goes when it does");
    }
}
