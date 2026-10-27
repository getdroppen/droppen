//! The `PATH` the server runs with.
//!
//! An app opened from the Finder, the Dock or a login item is started by
//! launchd with `/usr/bin:/bin:/usr/sbin:/sbin` and nothing else. The server
//! needs more: every session it starts runs `claude` or `codex`, and those —
//! and the `node` and `git` they call — live in `~/.local/bin`, Homebrew, a
//! Node version manager or wherever their installer put them.
//!
//! So the PATH is built the way the launchd agent of a command-line install
//! builds it (`scripts/install.sh` `service_path`): whatever the user's login
//! shell says first, read once at startup ([`login_shell_path`]) — which is
//! what finds a version manager's directory, since only the shell's own
//! start-up files know it — then a fixed list of the usual places, as a
//! fallback for what the shell did not say (or when it would not answer).
//! The shell first, so the server runs the `node` and `claude` the user's
//! terminal runs: nvm's, say, rather than Homebrew's. The server appends the
//! folders adopted in Settings after all of it (`server-rs/src/setup`).
//!
//! The rule itself ([`base_dirs`], [`build`]) lives in the dependency-free
//! `droppen-path` crate, shared with the server
//! (`server-rs/src/setup/locate.rs`), so the two cannot drift apart.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

// The PATH rule itself lives in `droppen-path`, shared with the server's
// setup guide (`server-rs/src/setup/locate.rs`).
pub use droppen_path::{base_dirs, build};

/// What `$SHELL -ilc` says `PATH` is, or `None` if it would not say in time.
///
/// Interactive *and* login, as VS Code and most terminal-adjacent apps do it,
/// because people set PATH in either `.zprofile` or `.zshrc` and a login-only
/// shell misses the second. The value is fenced with markers so whatever the
/// start-up files print does not end up in it. Standard input is closed and the
/// shell is stopped (by pid) after `timeout`, so a start-up file that waits for
/// something cannot hold the app up.
pub fn login_shell_path(shell: &Path, home: &Path, timeout: Duration) -> Option<String> {
    const MARK: &str = "__DROPPEN_PATH__";
    let mut child = Command::new(shell)
        .args(["-ilc", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Read on a thread and stop at the closing marker rather than at EOF:
    // something a start-up file left running in the background can hold the
    // pipe open long after the shell has printed its answer and gone.
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => text.extend_from_slice(&chunk[..read]),
            }
            if let Some(path) = fenced(&String::from_utf8_lossy(&text), MARK) {
                let _ = send.send(path);
                return;
            }
        }
    });
    let path = receive.recv_timeout(timeout).ok();
    // By pid, and only the shell this started; a no-op once it has exited.
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
    }
    let _ = child.wait();
    path.filter(|path| !path.is_empty())
}

/// The text between the first two `mark`s, trimmed.
fn fenced(text: &str, mark: &str) -> Option<String> {
    let start = text.find(mark)? + mark.len();
    let end = start + text[start..].find(mark)?;
    Some(text[start..end].trim().to_string())
}

/// The first executable called `name` on `path`.
pub fn find_executable(path: &str, name: &str) -> Option<PathBuf> {
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

/// Which AI CLIs the server will find.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiClis {
    /// Claude Code's `claude`.
    pub claude: Option<PathBuf>,
    /// OpenAI's `codex`.
    pub codex: Option<PathBuf>,
}

impl AiClis {
    /// Look both up on `path`.
    pub fn on(path: &str) -> Self {
        Self {
            claude: find_executable(path, "claude"),
            codex: find_executable(path, "codex"),
        }
    }

    /// Whether neither is installed.
    pub fn none(&self) -> bool {
        self.claude.is_none() && self.codex.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn executable(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, "#!/bin/sh\n").expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    #[test]
    fn without_the_login_shell_the_base_is_the_path() {
        let path = build(Path::new("/Users/ada"), None);
        let dirs: Vec<&str> = path.split(':').collect();
        let base: Vec<String> = base_dirs(Path::new("/Users/ada"))
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        assert_eq!(dirs, base);
        assert_eq!(dirs[0], "/Users/ada/.local/bin");
        let homebrew = dirs.iter().position(|dir| *dir == "/opt/homebrew/bin").expect("homebrew");
        let usr_local = dirs.iter().position(|dir| *dir == "/usr/local/bin").expect("usr/local");
        let usr_bin = dirs.iter().position(|dir| *dir == "/usr/bin").expect("usr/bin");
        assert!(homebrew < usr_local && usr_local < usr_bin);
        assert_eq!(*dirs.last().expect("last"), "/sbin");
    }

    #[test]
    fn the_login_shell_comes_first_and_the_base_fills_in_after_it() {
        let root = tempfile::tempdir().expect("tempdir");
        let nvm = root.path().join("nvm/versions/node/v22/bin");
        std::fs::create_dir_all(&nvm).expect("nvm");
        let nvm = nvm.to_string_lossy().into_owned();
        // nvm's node before Homebrew's, as the user's terminal has it.
        let shell = format!("{nvm}:/usr/bin:/bin");
        let path = build(Path::new("/Users/ada"), Some(&shell));
        let dirs: Vec<&str> = path.split(':').collect();
        assert_eq!(&dirs[..3], [nvm.as_str(), "/usr/bin", "/bin"], "the shell's own order, first");
        let homebrew = dirs.iter().position(|dir| *dir == "/opt/homebrew/bin").expect("homebrew");
        assert!(homebrew > 2, "Homebrew is a fallback after the shell: {path}");
        // The rest of the base, in its own order, once each.
        let rest: Vec<&str> = dirs[3..].to_vec();
        let base: Vec<String> = base_dirs(Path::new("/Users/ada"))
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .filter(|dir| dir != "/usr/bin" && dir != "/bin")
            .collect();
        assert_eq!(rest, base);
    }

    #[test]
    fn the_login_shell_adds_existing_absolute_directories_once() {
        let root = tempfile::tempdir().expect("tempdir");
        let nvm = root.path().join("nvm/versions/node/v22/bin");
        std::fs::create_dir_all(&nvm).expect("nvm");
        let extra = format!(
            "{}:/usr/bin:relative/bin::{}:/does/not/exist:{}:/usr/bin",
            nvm.display(),
            nvm.display(),
            "/opt/homebrew/bin"
        );
        let path = build(Path::new("/Users/ada"), Some(&extra));
        let dirs: Vec<&str> = path.split(':').collect();
        let nvm = nvm.to_string_lossy();
        assert_eq!(dirs.iter().filter(|dir| **dir == nvm).count(), 1);
        assert_eq!(dirs.iter().filter(|dir| **dir == "/usr/bin").count(), 1);
        assert_eq!(dirs.iter().filter(|dir| **dir == "/bin").count(), 1);
        assert!(!dirs.contains(&"relative/bin"));
        assert!(!dirs.contains(&"/does/not/exist"));
        assert!(!dirs.contains(&""));
        assert_eq!(dirs[0], nvm, "the shell's own directories come first");
        // Each where it first appears: /usr/bin where the shell put it, not
        // again where the base would have.
        assert_eq!(dirs[1], "/usr/bin");
        if Path::new("/opt/homebrew/bin").is_dir() {
            assert_eq!(dirs[2], "/opt/homebrew/bin");
        }
        assert_eq!(*dirs.last().expect("last"), "/sbin", "the base comes after the shell's");
    }

    #[test]
    fn the_command_line_install_builds_the_same_path() {
        // `scripts/install.sh` `service_path`, run on its own, against the same
        // HOME and shell PATH: the launchd agent of a command-line install and
        // the app give the server the same directories, in the same order.
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/install.sh");
        let script = std::fs::read_to_string(&script).expect("install.sh");
        let start = script.find("\nservice_path() {").expect("service_path in install.sh");
        let end = start + script[start..].find("\n}\n").expect("end of service_path") + 3;
        let function = &script[start..end];

        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let nvm = home.join(".nvm/versions/node/v22/bin");
        let session_bin = home.join(".local/share/remote-hand/instances/other/session-bin");
        std::fs::create_dir_all(&nvm).expect("nvm");
        std::fs::create_dir_all(&session_bin).expect("session-bin");
        let shell = format!(
            "{}:{}:/usr/bin:relative:{}/.local/bin:/does/not/exist::{}:/bin",
            nvm.display(),
            session_bin.display(),
            home.display(),
            nvm.display()
        );
        let output = Command::new("/bin/sh")
            .args(["-c", &format!("{function}\nPATH=\"$1\" service_path"), "sh", &shell])
            .env_clear()
            .env("HOME", &home)
            .output()
            .expect("sh");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let installed = String::from_utf8(output.stdout).expect("utf-8");
        assert_eq!(installed.trim_end(), build(&home, Some(&shell)));
    }

    #[test]
    fn without_a_home_the_path_has_no_relative_directory() {
        for home in ["", ".", "relative/home"] {
            let path = build(Path::new(home), Some("relative/bin:/usr/bin"));
            assert!(path.split(':').all(|dir| dir.starts_with('/')), "{home:?}: {path}");
            assert!(path.starts_with("/usr/bin:/opt/homebrew/bin:/usr/local/bin:"), "{home:?}: {path}");
            assert!(base_dirs(Path::new(home)).iter().all(|dir| dir.is_absolute()));
        }
        assert_eq!(base_dirs(Path::new("/Users/ada"))[0], Path::new("/Users/ada/.local/bin"));
    }

    #[test]
    fn a_session_bin_directory_never_leaks_in() {
        let root = tempfile::tempdir().expect("tempdir");
        let session_bin = root.path().join(".local/share/remote-hand/instances/another/session-bin");
        std::fs::create_dir_all(&session_bin).expect("dir");
        let path = build(Path::new("/Users/ada"), Some(&session_bin.to_string_lossy()));
        assert!(!path.contains("session-bin"), "{path}");
    }

    #[test]
    fn executables_are_found_in_path_order_and_plain_files_are_not() {
        let root = tempfile::tempdir().expect("tempdir");
        let first = root.path().join("a");
        let second = root.path().join("b");
        executable(&second.join("claude"));
        std::fs::create_dir_all(&first).expect("dir");
        std::fs::write(first.join("claude"), "not executable").expect("write");
        executable(&first.join("codex"));
        let path = format!("{}:{}", first.display(), second.display());
        let clis = AiClis::on(&path);
        assert_eq!(clis.claude, Some(second.join("claude")));
        assert_eq!(clis.codex, Some(first.join("codex")));
        assert!(!clis.none());
        assert!(AiClis::on(&root.path().join("empty").to_string_lossy()).none());
    }

    #[test]
    fn the_login_shell_path_is_read_between_the_markers() {
        let root = tempfile::tempdir().expect("tempdir");
        // A stand-in "shell" that prints noise around the real answer, the way
        // a chatty .zshrc would.
        let shell = root.path().join("fake-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\necho 'welcome!'\nPATH=/opt/fake/bin:/usr/bin\neval \"$2\"\necho 'bye'\n",
        )
        .expect("write");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let path = login_shell_path(&shell, root.path(), Duration::from_secs(5));
        assert_eq!(path.as_deref(), Some("/opt/fake/bin:/usr/bin"));
    }

    #[test]
    fn a_login_shell_that_hangs_is_given_up_on() {
        let root = tempfile::tempdir().expect("tempdir");
        let shell = root.path().join("slow-shell");
        std::fs::write(&shell, "#!/bin/sh\nexec sleep 30\n").expect("write");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let started = Instant::now();
        assert_eq!(login_shell_path(&shell, root.path(), Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
