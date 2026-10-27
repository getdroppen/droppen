//! The command lines the server answers to, run against the real binary.
//!
//! These pin what the parsers accept and refuse, and the exit status and stream
//! each outcome uses. Help and error *text* is clap's; these tests only look at
//! text that is behaviour rather than parsing (e.g. the agent's own refusals).
//! Program-to-program argv (`scripts/install.sh`'s `agent enroll - --hub …`,
//! `up -- serve`, the shims) is pinned by its own modules; this file covers the
//! dispatch above them.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use remote_hand_server::api::SERVER_VERSION;

struct Invocation {
    home: tempfile::TempDir,
}

impl Invocation {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(home.path().join("data")).expect("data");
        std::fs::create_dir_all(home.path().join("config")).expect("config");
        Self { home }
    }

    fn run(&self, argv: &[&str]) -> Output {
        self.run_with_stdin(argv, b"")
    }

    fn run_with_stdin(&self, argv: &[&str], stdin: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_remote-hand-server"))
            .args(argv)
            .env("HOME", self.home.path())
            .env("DROPPEN_DATA_DIR", self.home.path().join("data"))
            .env("DROPPEN_CONFIG_DIR", self.home.path().join("config"))
            .env("APP_INSTANCE", "probe")
            .env("APP_SERVER__PORT", "0")
            .env("APP_SERVER__HOST", "127.0.0.1")
            .env_remove("APP_AUTH__ENABLED")
            .env_remove("DROPPEN_NOTIFY_URL")
            .env_remove("DROPPEN_NOTIFY_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the server binary");
        child.stdin.as_mut().expect("stdin").write_all(stdin).expect("write stdin");
        child.wait_with_output().expect("wait")
    }
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("a normal exit, not a signal")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `--help` and `-h` are the first argument only, print to stdout, and exit 0.
#[test]
fn help_is_stdout_and_success() {
    let invocation = Invocation::new();
    for argv in [vec!["--help"], vec!["-h"], vec!["--help", "extra"], vec!["-h", "--bogus"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
        assert!(stderr(&output).is_empty(), "{argv:?}");
        let text = stdout(&output);
        assert!(text.contains("USAGE:"), "{argv:?} keeps the usage text: {text}");
        assert!(text.contains("remote-hand-server"), "{argv:?} names the binary it was run as: {text}");
    }
}

/// `--version` and `-V` print the bare version to stdout — the self-upgrade check reads all of stdout —
/// are the first argument only, and ignore what follows.
#[test]
fn version_is_the_bare_string_on_stdout() {
    let invocation = Invocation::new();
    for argv in [vec!["--version"], vec!["-V"], vec!["--version", "extra"], vec!["-V", "--bogus"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
        assert!(stderr(&output).is_empty(), "{argv:?}");
        assert_eq!(stdout(&output), format!("{SERVER_VERSION}\n"), "{argv:?}");
    }
}

/// Anything else as the first token is the unknown-command path: stderr, exit 1.
#[test]
fn unknown_commands_are_stderr_and_failure() {
    let invocation = Invocation::new();
    for argv in [
        vec!["bogus"],
        vec!["help"],
        vec!["--bogus"],
        vec!["-x"],
        vec!["-hV"],
        vec!["-Vh"],
        vec!["--help=1"],
        vec!["--version=1"],
        // clap would read this as its terminator and then serve; it has always
        // been an unknown command.
        vec!["--"],
        vec!["--", "serve"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?} writes nothing to stdout");
        assert!(!stderr(&output).is_empty(), "{argv:?} says why on stderr");
    }
}

/// A subcommand's `--help` is not help: every subcommand refuses it exactly as it refuses any other
/// argument it does not know (today: usage on stderr, exit 1). `serve`'s tail is ignored entirely.
#[test]
fn only_the_top_level_has_help() {
    let invocation = Invocation::new();
    for argv in [
        vec!["user", "--help"],
        vec!["user", "-h"],
        vec!["plugin", "--help"],
        vec!["notify", "--help"],
        vec!["replay", "--help"],
        vec!["pty-shim", "--help"],
        vec!["session-shim", "--help"],
        vec!["agent", "--help"],
        vec!["agent", "-h"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
}

/// The accepted `agent` shapes, including `--hub`/`--instance` before the subcommand and either spelling
/// of their values. The refusals run before any network or file work.
#[test]
fn agent_argv_keeps_its_shape() {
    let invocation = Invocation::new();

    for argv in [
        vec!["agent", "status"],
        vec!["agent", "--instance", "probe", "status"],
        vec!["agent", "status", "--instance=probe"],
        vec!["agent", "--instance=probe", "status"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
        assert!(stdout(&output).contains("instance probe"), "{argv:?}: {}", stdout(&output));
    }

    // `--hub` before the subcommand and both spellings reach the same check.
    for argv in [
        vec!["agent", "enroll", "-", "--hub", "wss://hub.example"],
        vec!["agent", "enroll", "--hub=wss://hub.example", "-"],
        vec!["agent", "--hub", "wss://hub.example", "enroll", "-"],
        vec!["agent", "enroll", "-", "--hub", "wss://a.example", "--hub", "wss://b.example"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(
            stderr(&output).contains("no local account"),
            "{argv:?} parses and reaches the enrolment refusal: {}",
            stderr(&output)
        );
    }

    // A bad code is refused, and `--` is never a separator here.
    for argv in [
        vec!["agent", "enroll", "nonsense", "--hub", "wss://hub.example"],
        vec!["agent", "enroll", "--", "-", "--hub", "wss://hub.example"],
        vec!["agent", "enroll", "-", "--", "--hub", "wss://hub.example"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(stderr(&output).contains("USAGE"), "{argv:?}: {}", stderr(&output));
    }

    // Every --instance is validated, and a --hub immediately before one still gets its URL.
    for argv in [
        vec!["agent", "status", "--instance", "Bad", "--instance", "probe"],
        vec!["agent", "status", "--instance=Bad", "--instance=probe"],
        vec!["agent", "--instance", "Bad", "--instance", "probe", "status"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(stderr(&output).contains("must be 1-32 characters"), "{argv:?}: {}", stderr(&output));
    }
    for argv in [
        vec!["agent", "enroll", "-", "--hub", "--instance", "probe", "wss://hub.example"],
        vec!["agent", "enroll", "-", "--hub", "--instance=probe", "wss://hub.example"],
        vec!["agent", "--hub", "--instance", "probe", "wss://hub.example", "enroll", "-"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(
            stderr(&output).contains("no local account"),
            "{argv:?} parsed the hub URL: {}",
            stderr(&output)
        );
    }

    // The reason the hub is checked even for a bad code: the URL is validated first.
    let output = invocation.run(&["agent", "enroll", "nonsense", "--hub", "ftp://hub.example"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("wss://"), "{}", stderr(&output));

    // --hub is only for enroll.
    for argv in [
        vec!["agent", "status", "--hub", "wss://hub.example"],
        vec!["agent", "run", "--hub=wss://hub.example"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stderr(&output).contains("--hub is only for enroll"), "{argv:?}: {}", stderr(&output));
    }

    // The shapes with no action at all.
    for argv in [vec!["agent"], vec!["agent", "enroll"], vec!["agent", "bogus"], vec!["agent", "status", "extra"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
}

/// `user`, `setup-code` and `open` keep their word shapes, including names that look like flags and the
/// bare `--` that clap reserves.
#[test]
fn auth_argv_keeps_its_shape() {
    let invocation = Invocation::new();

    for argv in [
        vec!["user", "list"],
        vec!["user", "list", "--app"],
        vec!["user", "--instance", "probe", "list"],
        vec!["user", "list", "--instance=probe"],
        vec!["user", "--app", "list"],
        vec!["user", "list", "--app", "--app"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
    }

    // A word that looks like a flag is a user name: `--help` here is not help.
    let output = invocation.run(&["user", "add", "--help"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("at least 12 characters"), "{}", stderr(&output));

    // `--` is a word to this grammar (it can be a name), never a separator.
    for argv in [
        vec!["user", "add", "--", "ada"],
        vec!["user", "--", "list"],
        vec!["user", "add", "ada", "--yes"],
        vec!["user", "passwd", "ada", "--app"],
        vec!["user", "rm", "ada", "--app"],
        vec!["user", "mark-app", "ada", "--yes"],
        vec!["user", "--instance", "Bad Name", "list"],
        vec!["user", "rm", "--help"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }

    // Every --instance is validated before clap: an invalid earlier name is refused even when a good
    // one follows, and nothing is created for the good one.
    let fresh = Invocation::new();
    for argv in [
        vec!["user", "list", "--instance", "Bad", "--instance", "probe"],
        vec!["user", "list", "--instance=Bad", "--instance=probe"],
        vec!["setup-code", "--instance", "Bad", "--instance", "probe"],
        vec!["setup-code", "--instance=Bad", "--instance=probe"],
        vec!["open", "--print", "--instance", "Bad", "--instance", "probe"],
        vec!["open", "--instance", "Bad", "--instance", "probe"],
    ] {
        let output = fresh.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(stderr(&output).contains("must be 1-32 characters"), "{argv:?}: {}", stderr(&output));
    }
    assert!(!fresh.home.path().join("data/instances/probe/state.db").exists(), "no files for the good name");

    for argv in [
        vec!["setup-code", "--"],
        vec!["setup-code", "junk"],
        vec!["setup-code", "--help"],
        vec!["open", "--"],
        vec!["open", "--print", "--print"],
        vec!["open", "--print", "extra"],
        vec!["open", "--help"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
}

/// `plugin` keeps its literal `--out`/`--yes` places and its `--` as a word; `notify` keeps its
/// two-word option spelling and its own help answer.
#[test]
fn plugin_and_notify_argv_keep_their_shape() {
    let invocation = Invocation::new();

    let output = invocation.run(&["plugin", "list"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));

    // Every --instance is validated before anything else.
    let fresh = Invocation::new();
    for argv in [
        vec!["plugin", "list", "--instance", "Bad", "--instance", "probe"],
        vec!["plugin", "list", "--instance=Bad", "--instance=probe"],
    ] {
        let output = fresh.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stderr(&output).contains("must be 1-32 characters"), "{argv:?}: {}", stderr(&output));
    }
    assert!(!fresh.home.path().join("data/instances/probe/state.db").exists());

    // `--` is a word here: every shape refuses a bare one (clap would drop it and run the command).
    for argv in [vec!["plugin", "--", "list"], vec!["plugin", "install", "--", "file"], vec!["plugin", "purge", "--", "tasks", "--yes"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }

    // A bare `--` is an `--out` value, and reaches the store (which does not exist here).
    let output = invocation.run(&["plugin", "export", "tasks", "--out", "--"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no store"), "{}", stderr(&output));

    for argv in [
        vec!["plugin", "export", "tasks", "--out=x"],
        vec!["plugin", "export", "tasks", "--out"],
        vec!["plugin", "purge", "tasks", "--yes", "--yes"],
        vec!["plugin", "purge", "--yes", "tasks"],
        vec!["plugin", "import", "tasks", "--replace"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
    let output = invocation.run(&["plugin", "purge", "tasks"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("--yes"), "{}", stderr(&output));

    // notify reaches the missing channel only when the argv parsed.
    for argv in [
        vec!["notify", "T"],
        vec!["notify", "T", "--level", "active"],
        vec!["notify", "--", "--level=active"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stderr(&output).contains("no channel was configured"), "{argv:?}: {}", stderr(&output));
    }
    // The token after a value-taking flag is its value, even when it looks like help.
    let output = invocation.run(&["notify", "T", "--level", "--help"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("unknown level"), "{}", stderr(&output));

    for argv in [
        vec!["notify", "T", "--level=active"],
        vec!["notify", "T", "-lpassive"],
        vec!["notify", "T", "--help"],
        vec!["notify", "T", "-h"],
        vec!["notify", "T", "--nope"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(!stderr(&output).contains("no channel"), "{argv:?} was parsed: {}", stderr(&output));
    }
}

/// The unconverted commands still receive their exact tail, including a `--help` that their own parser
/// refuses, and the two whose tails are ignored stay permissive.
#[test]
fn the_legacy_tails_are_untouched() {
    let invocation = Invocation::new();

    let output = invocation.run(&["preflight", "junk"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("ok (instance probe)"), "{}", stdout(&output));

    let output = invocation.run(&["user", "list"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let output = invocation.run(&["plugin", "list"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));

    // Every --instance is validated before anything else.
    let fresh = Invocation::new();
    for argv in [
        vec!["plugin", "list", "--instance", "Bad", "--instance", "probe"],
        vec!["plugin", "list", "--instance=Bad", "--instance=probe"],
    ] {
        let output = fresh.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stderr(&output).contains("must be 1-32 characters"), "{argv:?}: {}", stderr(&output));
    }
    assert!(!fresh.home.path().join("data/instances/probe/state.db").exists());

    // Accepted by the notify parser, refused by its behaviour (no channel in this environment).
    let output = invocation.run(&["notify", "T", "B", "--level", "active"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no channel was configured"), "{}", stderr(&output));

    // A bare `--` is not a separator in any of these tails today.
    for argv in [vec!["user", "--", "list"], vec!["plugin", "--", "list"], vec!["replay", "--", "--log", "x"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
}
