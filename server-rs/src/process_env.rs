//! The environment a spawned CLI runs with, and the redaction every captured
//! byte of its output passes through.
//!
//! Port of the former TypeScript server's `env.ts`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The environment for a child process, ordered so a spawn is reproducible.
pub type Env = BTreeMap<String, String>;

/// Variables the harness may have set that break a spawned CLI.
///
/// The server may itself run inside an agent harness whose `CLAUDE`- and
/// `ANTHROPIC`-prefixed variables break spawned CLIs (nested-session guard,
/// proxied base URL), so they are stripped before launching anything.
fn is_harness_variable(key: &str) -> bool {
    key.starts_with("CLAUDECODE") || key.starts_with("CLAUDE_") || key.starts_with("ANTHROPIC_")
}

/// Variables that describe *this server's* situation, not a child's.
///
/// `DROPPEN_DESKTOP_APP` (`crate::desktop_app`), or its old name
/// `RH_DESKTOP_APP`, says the Droppen desktop app runs this server; a terminal
/// or a CLI it starts is not run by the app, and a `remote-hand-server`
/// somebody starts by hand from such a terminal must not believe it is (it
/// would refuse to self-upgrade and hide its own login). Both names go.
/// `DROPPEN_DESKTOP_SHELL_READ` ([`crate::setup::SHELL_READ_ENV`]) is the
/// app's report on how *its* read of the login shell went; it describes how
/// this server was started and means nothing to anything the server starts.
fn is_server_only_variable(key: &str) -> bool {
    crate::env::is(key, crate::desktop_app::ENV) || key == crate::setup::SHELL_READ_ENV
}

/// The `PATH` every child gets instead of the inherited one, once something has
/// set it.
///
/// A process-wide value rather than `std::env::set_var`, which is unsound in a
/// process with more than one thread (another thread may be reading the
/// environment block while it is rewritten). Every child this server starts
/// gets its environment from [`clean_env`], and `Command` looks a bare program
/// name up on the `PATH` it is *given*, so overriding here is overriding
/// everywhere: sessions, terminals, probes and wizards alike.
///
/// Set by the first-run guide ([`crate::setup`]) when it re-reads the login
/// shell or adopts the directory of a CLI found off the `PATH`, so a tool
/// installed while the server runs is found without a restart. Children that
/// are already running keep the `PATH` they were started with.
#[cfg(not(test))]
static CHILD_PATH: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

// In the unit tests the override is the setting thread's own: a test that
// sets it is the only one that sees it, and every other test, running
// alongside, keeps reading the inherited `PATH`. (A test's async code runs on
// its own thread: `#[tokio::test]` is a current-thread runtime.)
#[cfg(test)]
thread_local! {
    static CHILD_PATH: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Replace the `PATH` children are started with; `None` goes back to the
/// inherited one. Returns what was there before.
pub fn set_child_path(path: Option<String>) -> Option<String> {
    #[cfg(not(test))]
    {
        let mut slot = CHILD_PATH.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::replace(&mut *slot, path)
    }
    #[cfg(test)]
    {
        CHILD_PATH.with(|slot| slot.replace(path))
    }
}

/// The override [`set_child_path`] installed, if any.
pub fn child_path_override() -> Option<String> {
    #[cfg(not(test))]
    {
        CHILD_PATH.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
    #[cfg(test)]
    {
        CHILD_PATH.with(|slot| slot.borrow().clone())
    }
}

/// The `PATH` this process was started with, whatever the override says.
pub fn inherited_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

/// The `PATH` a child started now would get: the override, or the inherited one.
pub fn child_path() -> String {
    child_path_override().unwrap_or_else(inherited_path)
}

/// The process environment minus the harness variables, plus `extra`.
///
/// `extra` is applied *after* the strip, which is how the account token and
/// `CLAUDE_CONFIG_DIR` get through: they are ours, not the harness's. The
/// child-`PATH` override ([`set_child_path`]) goes in before `extra`, so a
/// caller that needs a `PATH` of its own (the setup-token wizard's no-op
/// browser openers) still has the last word.
pub fn clean_env<I, K, V>(extra: I) -> Env
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let mut env: Env = std::env::vars()
        .filter(|(key, _)| !is_harness_variable(key) && !is_server_only_variable(key))
        .collect();
    if let Some(path) = child_path_override() {
        env.insert("PATH".to_string(), path);
    }
    for (key, value) in extra {
        env.insert(key.into(), value.into());
    }
    env
}

/// [`clean_env`] with nothing added.
pub fn clean_env_bare() -> Env {
    clean_env(std::iter::empty::<(String, String)>())
}

/// What tells the Claude CLI it is already inside a sandbox.
///
/// The CLI refuses `--permission-mode bypassPermissions` outright when it is
/// running as root, on the reasoning that root plus "approve everything" on a
/// developer's own machine is not a combination anybody meant to ask for. The
/// escape hatch is this variable, which says the blast radius is already walled
/// off by something outside the CLI.
pub const SANDBOX_MARKER: &str = "IS_SANDBOX";

/// The marker a spawn needs, given the effective uid it will run under.
///
/// Split out from [`claude_env`] purely so the branch is testable: no test
/// process runs as root, so the only honest way to cover the root side is to
/// hand the uid in.
fn sandbox_marker(euid: u32) -> Option<(&'static str, &'static str)> {
    (euid == 0).then_some((SANDBOX_MARKER, "1"))
}

/// The uid a child spawned right now would run under.
#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: `geteuid` reads an attribute of the calling process, takes no
    // arguments and is documented as always succeeding.
    unsafe { libc::geteuid() }
}

/// Nothing to gate on: there is no root, and no CLI refusal to work around.
#[cfg(not(unix))]
fn effective_uid() -> u32 {
    u32::MAX
}

/// The environment a `claude` spawn runs with: [`clean_env`] plus the sandbox
/// marker when this server is root.
///
/// A root server is ordinary in a container, and the composer offers
/// `bypassPermissions` like any other permission mode. Without the marker that
/// combination does not produce a warning, it produces a CLI that will not
/// start, which reaches the user as a run that failed for no visible reason.
///
/// The marker is set on the *uid*, not on "am I in a container", because the uid
/// is the thing the CLI actually checks and the only one this process can read
/// honestly. That does mean a server deliberately run as root on a bare host
/// gets it too; that is the widening, and it is the side to err on: a CLI which
/// refuses to launch would teach the user to reach for `--dangerously-…`
/// somewhere we cannot see it.
pub fn claude_env<I, K, V>(extra: I) -> Env
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let mut env = clean_env(extra);
    if let Some((key, value)) = sandbox_marker(effective_uid()) {
        env.insert(key.to_string(), value.to_string());
    }
    env
}

/// [`claude_env`] with nothing added.
pub fn claude_env_bare() -> Env {
    claude_env(std::iter::empty::<(String, String)>())
}

/// Apply an [`Env`] to a command, replacing the inherited environment wholesale.
pub fn apply_env(command: &mut tokio::process::Command, env: &Env) {
    command.env_clear();
    command.envs(env);
}

/// Every `sk-…` credential, whoever issued it.
///
/// Captured CLI output is stored in injection state and served over the API, and
/// a failing CLI happily echoes the token we injected ("Authentication failed
/// for credential sk-…"), so every path that surfaces child output runs through
/// this first.
///
/// Not `sk-ant-` alone any more. A codex account's key is an OpenAI-shaped
/// `sk-…` or `sk-proj-…`, and `codex exec` quotes it back verbatim on a 401 —
/// so a pattern that only knew Anthropic's prefix would have put somebody's key
/// in a session recording the first time it was typed wrong. The prefix is kept
/// in the replacement, because "which vendor refused this" is the useful half
/// of the line and is not itself a secret.
///
/// Two alternatives rather than one widened pattern, because the two prefixes
/// carry different amounts of evidence. `sk-ant-` is Anthropic's and nothing
/// else's, so eight characters behind it are already a credential — the
/// threshold this has always used, and lowering the bar for an unprefixed
/// `sk-` must not raise it here. A bare `sk-` is a prefix half the world uses,
/// so it needs twenty before the guess is safe: longer than every `sk-`-word
/// that is not a credential, far shorter than the shortest real key.
///
/// This is only ever the *second* layer. It can catch a credential nobody told
/// us about, which is why it exists; it cannot catch one that does not look
/// like a credential at all, which is why
/// [`crate::accounts::store::AccountsStore::redact_known_secrets`] runs first
/// on every path that shows a user what a CLI said.
pub fn redact_secrets(text: &str) -> String {
    static SECRET: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re =
        SECRET.get_or_init(|| regex::Regex::new(r"(?P<anthropic>sk-ant-[A-Za-z0-9_-]{8,})|sk-[A-Za-z0-9_-]{20,}").expect("valid secret pattern"));
    re.replace_all(text, |caps: &regex::Captures| {
        let prefix = if caps.name("anthropic").is_some() { "sk-ant-" } else { "sk-" };
        format!("{prefix}\u{2026}[redacted]")
    })
    .into_owned()
}

/// Below this, a "secret" is more likely to be a word than a credential, and
/// replacing every occurrence of it would shred the text it appears in.
///
/// Only reachable through a hand-edited row or a caller passing something odd:
/// every credential a person can type in goes through `normalize_secret`, and
/// the shortest real one is far longer.
const MIN_REDACTABLE_SECRET: usize = 8;

/// Take every known value out of `text`, longest first.
///
/// The exact-value layer of [`redact_secrets`]: a caller that holds the
/// credential itself — the account store, the probe that is about to refuse
/// one — does not have to guess at its shape. Longest first so a value that
/// contains another is not left behind as a readable tail.
pub fn redact_values<'a>(text: &str, values: impl IntoIterator<Item = &'a str>) -> String {
    let mut values: Vec<&str> = values.into_iter().filter(|value| value.len() >= MIN_REDACTABLE_SECRET).collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let mut redacted = text.to_string();
    for value in values {
        if redacted.contains(value) {
            redacted = redacted.replace(value, "[redacted]");
        }
    }
    redacted
}

/// Whether the file at `path` is executable by this process.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(metadata) = std::fs::metadata(path) else { return false };
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
    }
}

/// Where a command would be found, or `None`.
///
/// Wizards check this before spawning: a missing CLI must fail the request
/// loudly rather than leave the user staring at a terminal that printed
/// "command not found" and died.
pub fn resolve_on_path(command: &str, env: &Env) -> Option<PathBuf> {
    if command.contains('/') {
        let path = PathBuf::from(command);
        return is_executable(&path).then_some(path);
    }
    for dir in env.get("PATH").map(String::as_str).unwrap_or("").split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(command);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_variables_are_stripped_and_extras_survive() {
        assert!(is_harness_variable("CLAUDECODE"));
        assert!(is_harness_variable("CLAUDE_CONFIG_DIR"));
        assert!(is_harness_variable("ANTHROPIC_API_KEY"));
        assert!(!is_harness_variable("CLAUDIA"), "only the documented prefixes go");
        assert!(!is_harness_variable("PATH"));

        // `clean_env` reads the live process environment, so assert on the
        // contract it can guarantee: an extra always wins over the strip.
        let env = clean_env([("CLAUDE_CONFIG_DIR", "/managed"), ("ANTHROPIC_API_KEY", "sk-ant-abcdefgh")]);
        assert_eq!(env.get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/managed"));
        assert_eq!(env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("sk-ant-abcdefgh"));
        assert!(!env.keys().any(|k| k == "CLAUDECODE"));
    }

    /// The root branch, proved without being root.
    ///
    /// No test process runs as uid 0, so the marker's own rule is asserted
    /// against a handed-in uid, and [`claude_env`] is then held to that rule
    /// under whatever uid the test really has. Between them the two halves say
    /// what matters: the marker appears exactly when the spawn would be root's.
    #[test]
    fn the_desktop_app_switch_stays_with_the_server() {
        assert!(is_server_only_variable("DROPPEN_DESKTOP_APP"));
        let old = crate::env::old_name("DROPPEN_DESKTOP_APP").expect("an old name");
        assert!(is_server_only_variable(old), "{old}, which an app from before the rename sets, goes too");
        assert!(
            is_server_only_variable("DROPPEN_DESKTOP_SHELL_READ"),
            "the app's report on its own shell read is the server's alone"
        );
        assert!(!is_server_only_variable("DROPPEN_NOTIFY_URL"), "what a child is meant to have stays");
        assert!(!is_server_only_variable("PATH"));
    }

    #[test]
    fn the_sandbox_marker_appears_for_root_and_for_nobody_else() {
        assert_eq!(sandbox_marker(0), Some((SANDBOX_MARKER, "1")), "root needs the escape hatch");
        assert_eq!(sandbox_marker(1), None);
        assert_eq!(sandbox_marker(501), None, "an ordinary login is refused nothing");

        // `claude_env` reads the live uid, so hold it to the rule rather than to
        // a fixed answer: on an ordinary test run this asserts the absence, and
        // in a root container the very same line asserts the presence.
        let expected = sandbox_marker(effective_uid())
            .map(|(_, value)| value.to_string())
            .or_else(|| std::env::var(SANDBOX_MARKER).ok());
        let env = claude_env([("CLAUDE_CONFIG_DIR", "/managed")]);
        assert_eq!(env.get(SANDBOX_MARKER), expected.as_ref(), "a claude spawn follows the marker's rule");
        assert_eq!(
            env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some("/managed"),
            "and still carries the extras"
        );
        assert_eq!(claude_env_bare().get(SANDBOX_MARKER), env.get(SANDBOX_MARKER));

        // The plain environment adds nothing of its own: a `codex` or a shell
        // has no use for a variable that means something to one CLI only.
        assert_eq!(
            clean_env_bare().get(SANDBOX_MARKER).map(String::as_str),
            std::env::var(SANDBOX_MARKER).ok().as_deref()
        );
    }

    #[test]
    fn a_token_never_survives_redaction() {
        let raw = "Authentication failed for credential sk-ant-oat01-AbCdEf_012-345 (see docs)";
        let redacted = redact_secrets(raw);
        assert!(!redacted.contains("AbCdEf"), "the secret body must be gone: {redacted}");
        assert!(redacted.contains("sk-ant-\u{2026}[redacted]"));
        assert!(redacted.ends_with("(see docs)"), "the surrounding text is untouched");
        // Too short to be a credential: left alone, as in the TypeScript.
        assert_eq!(redact_secrets("sk-ant-1234"), "sk-ant-1234");

        // A codex account's key is an OpenAI-shaped `sk-…`, and `codex exec`
        // prints it back on a 401. The prefix survives; the body does not.
        let codex = redact_secrets("Incorrect API key provided: sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789 (docs)");
        assert!(!codex.contains("AbCdEfGh"), "an OpenAI key must not reach a recording either: {codex}");
        assert_eq!(codex, "Incorrect API key provided: sk-\u{2026}[redacted] (docs)");
        assert_eq!(redact_secrets("sk-short"), "sk-short", "not every sk- word is a credential");

        // The two thresholds are not the same number, and widening the bare
        // `sk-` one must never have raised Anthropic's: `sk-ant-` is nobody
        // else's prefix, so eight characters behind it are already a token.
        assert_eq!(redact_secrets("sk-ant-api03-abcdefgh"), "sk-ant-\u{2026}[redacted]");
        assert_eq!(
            redact_secrets("sk-ant-Abcdefgh"),
            "sk-ant-\u{2026}[redacted]",
            "eight characters behind sk-ant- is the threshold this has always used"
        );
        assert_eq!(
            redact_secrets("sk-abcdefghijklmnopqrs"),
            "sk-abcdefghijklmnopqrs",
            "nineteen is below the bare-prefix bar"
        );
        assert_eq!(redact_secrets("sk-abcdefghijklmnopqrst"), "sk-\u{2026}[redacted]", "twenty is at it");

        // What the pattern cannot do, and why the store's by-value pass exists:
        // a gateway's bearer token looks like nothing at all.
        assert_eq!(
            redact_secrets("401 for token hunter2-gateway-issued-value"),
            "401 for token hunter2-gateway-issued-value",
            "a pattern can only find credentials that look like credentials"
        );
    }

    #[test]
    fn resolve_on_path_walks_path_and_checks_the_bit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("faketool");
        std::fs::write(&bin, "#!/bin/sh\n").expect("write");
        let env: Env = [("PATH".to_string(), dir.path().to_string_lossy().into_owned())].into_iter().collect();

        assert_eq!(resolve_on_path("faketool", &env), None, "a non-executable file is not a command");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            assert_eq!(resolve_on_path("faketool", &env), Some(bin.clone()));
            assert_eq!(resolve_on_path(&bin.to_string_lossy(), &env), Some(bin), "an explicit path skips PATH");
        }
        assert_eq!(resolve_on_path("definitely-not-here", &env), None);
    }
}
