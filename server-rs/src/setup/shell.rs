//! What the user's login shell says, read the way a terminal would see it.
//!
//! An app started from the Finder gets launchd's environment, not the one the
//! user's shell builds: the `PATH` a version manager adds, the proxy a company
//! requires, the API key somebody exported in `.zshrc`. The desktop app reads
//! the `PATH` once at launch; the guide reads it again here, server-side, when
//! that one was not good enough — a `.zshrc` too slow for the app's 5 s, or a
//! CLI that was just installed somewhere only the shell knows about.
//!
//! One `$SHELL -ilc` call answers everything the guide needs, fenced so
//! whatever the start-up files print cannot end up in it:
//!
//! * `PATH`;
//! * the proxy variables, both spellings — handed to
//!   install terminals, never reported, never logged;
//! * whether `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` are set — `set` or `unset`,
//!   computed *inside* the shell, so the value never leaves it.
//!
//! Nothing else is read. The script runs under `/bin/sh` as a child of the
//! login shell, so it means the same thing whether that shell is zsh, bash or
//! fish, and it only sees what the shell *exports* — which is exactly what a
//! CLI started from that shell would see.

use std::path::Path;
use std::time::Duration;

use super::runner::{Invocation, RunOutput};
use super::PathRead;
use crate::process_env::Env;

/// The opening fence.
pub const BEGIN: &str = "__DROPPEN_SHELL_BEGIN__";
/// The closing fence; reading stops as soon as it has been seen.
pub const END: &str = "__DROPPEN_SHELL_END__";
/// How long the read normally gets — the desktop app's own budget.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// How long it gets when the user asked to wait ("look again, slowly").
pub const PATIENT_TIMEOUT: Duration = Duration::from_secs(20);
/// The proxy variables carried to install terminals, in both cases.
pub const PROXY_VARIABLES: [&str; 8] = [
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];
/// The keys whose presence — never value — is reported.
pub const KEY_VARIABLES: [&str; 2] = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"];
/// The `PATH` the read starts from: launchd's, so what comes back is what the
/// user's own start-up files produce and nothing Droppen added.
pub const MINIMAL_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// The user's login shell: `$SHELL` when it is an absolute path, else zsh.
pub fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| shell.starts_with('/'))
        .unwrap_or_else(|| "/bin/zsh".to_string())
}

/// The POSIX script the shell runs, one line.
///
/// It is passed inside single quotes to a shell that may be fish, where the
/// only escapes inside single quotes are a backslash before a quote or before
/// another backslash. So it contains no single quote and no doubled backslash
/// (a test holds it to that); the `\n` in `printf`'s format is a backslash
/// before an `n`, which every one of those shells passes through untouched.
pub fn script() -> String {
    let mut parts = vec![format!("echo {BEGIN}"), "printf \"PATH=%s\\n\" \"$PATH\"".to_string()];
    for name in PROXY_VARIABLES {
        parts.push(format!("printf \"{name}=%s\\n\" \"${name}\""));
    }
    for name in KEY_VARIABLES {
        parts.push(format!("if [ -n \"${name}\" ]; then echo {name}=set; else echo {name}=unset; fi"));
    }
    parts.push(format!("echo {END}"));
    parts.join("; ")
}

/// The one invocation: `$SHELL -ilc "/bin/sh -c '<script>'"` in `home`.
///
/// Interactive *and* login, as the desktop app and most terminal-adjacent apps
/// do it, because people set things in `.zprofile` and in `.zshrc` and a
/// login-only shell misses the second. The environment it starts from is the
/// server's, scrubbed (`clean`), minus everything it is about to be asked:
/// `PATH` is launchd's and the proxies and keys are absent, so whatever comes
/// back came from the user's own files.
pub fn invocation(shell: &str, home: &Path, clean: &Env, patient: bool) -> Invocation {
    let mut env = clean.clone();
    for name in PROXY_VARIABLES.iter().chain(KEY_VARIABLES.iter()) {
        env.remove(*name);
    }
    env.insert("PATH".to_string(), MINIMAL_PATH.to_string());
    Invocation {
        program: shell.to_string(),
        args: vec!["-ilc".to_string(), format!("/bin/sh -c '{}'", script())],
        env,
        cwd: Some(home.to_path_buf()),
        timeout: if patient { PATIENT_TIMEOUT } else { TIMEOUT },
        stdin: None,
        until: Some(END.to_string()),
    }
}

/// What one read found.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ShellSnapshot {
    /// How the read went.
    pub status: PathRead,
    /// The shell's `PATH`, when it said one.
    pub path: Option<String>,
    /// The proxy variables it has set, by name. Handed to install terminals;
    /// never reported, never logged — a proxy URL can carry a password.
    pub proxies: Vec<(String, String)>,
    /// `ANTHROPIC_API_KEY` is set (its value was never read).
    pub anthropic_key: bool,
    /// `OPENAI_API_KEY` is set (its value was never read).
    pub openai_key: bool,
}

/// No proxy values: they can hold credentials.
impl std::fmt::Debug for ShellSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellSnapshot")
            .field("status", &self.status)
            .field("path", &self.path)
            .field("proxies", &self.proxies.iter().map(|(name, _)| name).collect::<Vec<_>>())
            .field("anthropic_key", &self.anthropic_key)
            .field("openai_key", &self.openai_key)
            .finish()
    }
}

/// Read the fenced answer out of whatever the shell printed.
///
/// `ok` needs both fences: an answer cut off by the timeout is a `timeout`, an
/// answer that never came (the shell exited, or would not start) a `failed`.
/// A shell that exited non-zero *after* printing a whole answer is still `ok`
/// — a start-up file with an error in it is common, and the answer is whole.
pub fn parse(output: &RunOutput) -> ShellSnapshot {
    let fenced = output
        .stdout
        .find(BEGIN)
        .map(|start| &output.stdout[start + BEGIN.len()..])
        .and_then(|rest| rest.find(END).map(|end| &rest[..end]));
    let Some(body) = fenced else {
        let status = if output.timed_out { PathRead::Timeout } else { PathRead::Failed };
        return ShellSnapshot {
            status,
            ..ShellSnapshot::default()
        };
    };
    let mut snapshot = ShellSnapshot {
        status: PathRead::Ok,
        ..ShellSnapshot::default()
    };
    for line in body.lines() {
        let Some((name, value)) = line.split_once('=') else { continue };
        let value = value.trim_end_matches('\r');
        match name {
            "PATH" if !value.is_empty() => snapshot.path = Some(value.to_string()),
            "ANTHROPIC_API_KEY" => snapshot.anthropic_key = value == "set",
            "OPENAI_API_KEY" => snapshot.openai_key = value == "set",
            name if PROXY_VARIABLES.contains(&name) && !value.is_empty() => snapshot.proxies.push((name.to_string(), value.to_string())),
            _ => {}
        }
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fenced(body: &str) -> String {
        format!("welcome to zsh!\n{BEGIN}\n{body}\n{END}\nbye\n")
    }

    #[test]
    fn the_script_survives_any_shell_s_single_quotes() {
        let script = script();
        assert!(!script.contains('\''), "a single quote would end the quoting: {script}");
        assert!(
            !script.contains("\\\\") && !script.contains("\\'"),
            "fish's two escapes inside single quotes: {script}"
        );
        assert!(!script.contains('\n'), "one line: {script}");
        for name in PROXY_VARIABLES {
            assert!(script.contains(&format!("\"${name}\"")), "{name}");
        }
        for name in KEY_VARIABLES {
            assert!(script.contains(&format!("{name}=set")), "{name} is reported as set/unset");
            assert!(!script.contains(&format!("{name}=%s")), "{name}'s value is never printed");
        }
    }

    #[test]
    fn the_read_starts_from_launchd_s_path_with_nothing_it_is_about_to_ask() {
        let clean: Env = [
            ("PATH", "/Users/ada/.local/bin:/opt/homebrew/bin:/usr/bin"),
            ("HTTPS_PROXY", "http://inherited:8080"),
            ("OPENAI_API_KEY", "sk-inherited"),
            ("HOME", "/Users/ada"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let read = invocation("/bin/zsh", Path::new("/Users/ada"), &clean, false);
        assert_eq!(read.program, "/bin/zsh");
        assert_eq!(read.args[0], "-ilc");
        assert!(read.args[1].starts_with("/bin/sh -c '") && read.args[1].ends_with('\''));
        assert_eq!(
            read.env.get("PATH").map(String::as_str),
            Some(MINIMAL_PATH),
            "no Droppen directories (H5)"
        );
        assert!(!read.env.contains_key("HTTPS_PROXY") && !read.env.contains_key("OPENAI_API_KEY"));
        assert_eq!(read.env.get("HOME").map(String::as_str), Some("/Users/ada"));
        assert_eq!(read.timeout, TIMEOUT);
        assert_eq!(read.until.as_deref(), Some(END));
        assert_eq!(invocation("/bin/zsh", Path::new("/"), &clean, true).timeout, PATIENT_TIMEOUT);
    }

    #[test]
    fn the_answer_is_read_between_the_fences_whatever_surrounds_it() {
        let output = RunOutput::exited(
            1,
            fenced(
                "PATH=/Users/ada/.nvm/versions/node/v22/bin:/usr/bin\nHTTPS_PROXY=http://user:pw@proxy:3128\nhttps_proxy=\n\
                 NO_PROXY=localhost\nANTHROPIC_API_KEY=set\nOPENAI_API_KEY=unset",
            ),
            "zsh: command not found: compinit",
        );
        let read = parse(&output);
        assert_eq!(read.status, PathRead::Ok, "a start-up file's error does not spoil a whole answer");
        assert_eq!(read.path.as_deref(), Some("/Users/ada/.nvm/versions/node/v22/bin:/usr/bin"));
        assert_eq!(
            read.proxies,
            vec![
                ("HTTPS_PROXY".to_string(), "http://user:pw@proxy:3128".to_string()),
                ("NO_PROXY".to_string(), "localhost".to_string())
            ]
        );
        assert!(read.anthropic_key && !read.openai_key);
        assert!(!format!("{read:?}").contains("pw@proxy"), "a proxy's password never reaches a log");
    }

    #[test]
    fn a_cut_off_answer_is_a_timeout_and_no_answer_a_failure() {
        let mut cut = RunOutput::timeout();
        cut.stdout = format!("{BEGIN}\nPATH=/usr/bin\n");
        assert_eq!(parse(&cut).status, PathRead::Timeout);
        assert_eq!(parse(&cut).path, None, "half an answer is no answer");
        assert_eq!(parse(&RunOutput::exited(127, "", "no such file")).status, PathRead::Failed);
        assert_eq!(parse(&RunOutput::failed_to_start()).status, PathRead::Failed);
    }
}
