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
//! cheapest real thing there is: one `claude -p` (or `codex exec`) with the
//! credential in the environment, in a config directory holding no login of
//! its own, so the only way it can succeed is the credential itself. Each CLI's
//! probe is its login's [`super::login::Login::verify`]; this module holds what
//! they share.
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
//! credentials (claude's `is_credential_refusal`, codex's `is_codex_refusal`)
//! before a refusal is believed.
//!
//! What the caller does with [`Verdict::Unverifiable`] is the caller's business,
//! and the two callers differ: a pasted credential is stored with the reason
//! attached, so the UI can say "saved, but nothing proved this works", while a
//! capture with two possible readings refuses to guess — see
//! [`super::setup_token::choose_reading`].

use std::path::PathBuf;
use std::time::Duration;

use droppen_instance::paths::Paths;
use droppen_base::process_env::{redact_secrets, redact_values};

use super::login::Logins;
use super::store::{AccountProvider, Credential};

/// The prompt the probe sends. Short on purpose: it is billed like any other.
pub(crate) const PROBE_PROMPT: &str = "ping";
/// A cold `claude` start plus one round trip, with room for a slow link.
///
/// Under a minute on purpose: this runs inside a `POST`/`PATCH`, and the reverse
/// proxy in front of a typical deployment gives up on a response at 60 s. A
/// check that outlived the request it belongs to would reach the user as a
/// gateway error rather than as a verdict.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// How much of a failing CLI's output is quoted back to the user.
const MAX_REASON: usize = 300;

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
pub(crate) fn reason(stderr: &str, stdout: &str, secrets: &[&str]) -> String {
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
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) async fn make(paths: &Paths) -> std::io::Result<Self> {
        let root = paths.data_dir().join("verify").join(uuid::Uuid::new_v4().to_string());
        tokio::fs::create_dir_all(root.join("claude")).await?;
        tokio::fs::create_dir_all(root.join("cwd")).await?;
        tokio::fs::create_dir_all(root.join("codex")).await?;
        Ok(Self(root))
    }

    pub(crate) fn config_dir(&self) -> PathBuf {
        self.0.join("claude")
    }

    pub(crate) fn cwd(&self) -> PathBuf {
        self.0.join("cwd")
    }

    /// `CODEX_HOME` for a codex probe. Created by [`Scratch::make`], never left
    /// to the CLI: `codex` exits with "Error finding codex home" rather than
    /// creating a `CODEX_HOME` that is not there.
    pub(crate) fn codex_home(&self) -> PathBuf {
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
pub async fn verify_secret(logins: &Logins, paths: &Paths, provider: AccountProvider, credential: &Credential) -> Verdict {
    let (secret, env) = match credential {
        Credential::Oauth(secret) => (secret.as_str(), Vec::new()),
        Credential::Env(env) => ("", env.clone()),
    };
    // A block that cannot be turned into a spawn is refused outright rather
    // than probed: there is nothing to run. Only codex can say this, and only
    // about a block with no key variable in it.
    let login = logins.of(provider);
    let launch = match login.spawn_env(credential.kind(), secret, &env) {
        Ok(launch) => launch,
        Err(error) => return Verdict::Invalid(error.to_string()),
    };
    let values = credential.values();
    login.verify(paths, &launch, &values).await
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
