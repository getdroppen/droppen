//! The command lines the server answers to, run against the real binary.
//!
//! Program-to-program argv is pinned byte-for-byte: `scripts/install.sh`'s `agent enroll - --hub … --instance …`
//! and the two shims the server spawns. Every other command keeps the meaning and exit code of what it
//! accepted before, but clap may now take more spellings (`--x=v`, `-xv`) and owns `--help`/`--version`.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use remote_hand_server::http::SERVER_VERSION;

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

/// Help is clap's, on stdout, exit 0. `-h` is the short form.
#[test]
fn help_is_stdout_and_success() {
    let invocation = Invocation::new();
    for argv in [
        vec!["--help"],
        vec!["-h"],
        vec!["--help", "extra"],
        vec!["-h", "--bogus"],
        vec!["-hV"],
        vec!["--help", "--version"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
        assert!(stderr(&output).is_empty(), "{argv:?}");
        let text = stdout(&output);
        assert!(text.contains("Usage:"), "{argv:?} prints help: {text}");
        assert!(text.contains("remote-hand-server"), "{argv:?} names the binary it was run as: {text}");
    }
}

/// `--version` and `-V` print the bare version to stdout — the self-upgrade check reads all of stdout.
#[test]
fn version_is_the_bare_string_on_stdout() {
    let invocation = Invocation::new();
    for argv in [
        vec!["--version"],
        vec!["-V"],
        vec!["--version", "extra"],
        vec!["-V", "--bogus"],
        vec!["-Vh"],
        vec!["--version", "--help"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
        assert!(stderr(&output).is_empty(), "{argv:?}");
        assert_eq!(stdout(&output), format!("{SERVER_VERSION}\n"), "{argv:?}");
    }
}

/// Unknown commands and malformed flags are clap's usage errors: stderr, exit 2.
#[test]
fn unknown_commands_are_stderr_and_failure() {
    let invocation = Invocation::new();
    for argv in [
        vec!["bogus"],
        vec!["help"],
        vec!["--bogus"],
        vec!["-x"],
        vec!["--help=1"],
        vec!["--version=1"],
        vec!["--", "serve"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?} writes nothing to stdout");
        assert!(!stderr(&output).is_empty(), "{argv:?} says why on stderr");
    }
}

/// Every command's `--help` is clap's.
#[test]
fn subcommand_help_is_claps() {
    let invocation = Invocation::new();
    for argv in [
        vec!["user", "--help"],
        vec!["user", "-h"],
        vec!["plugin", "--help"],
        vec!["notify", "--help"],
        vec!["replay", "--help"],
        vec!["agent", "--help"],
        vec!["agent", "-h"],
        vec!["open", "--help"],
        vec!["setup-code", "--help"],
        vec!["export-openapi", "--help"],
        vec!["pty-shim", "--help"],
        vec!["session-shim", "--help"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
        assert!(stderr(&output).is_empty(), "{argv:?}");
        assert!(!stdout(&output).is_empty(), "{argv:?} prints help");
    }
}

/// The accepted `agent` shapes, including `--hub`/`--instance` before the subcommand, and the exact
/// `scripts/install.sh` invocation. The refusals run before any network or file work.
#[test]
fn agent_argv_keeps_its_shape() {
    let invocation = Invocation::new();

    for argv in [
        vec!["agent", "status"],
        vec!["agent", "--instance", "probe", "status"],
        vec!["agent", "status", "--instance=probe"],
        vec!["agent", "--instance=probe", "status"],
        vec!["agent", "status", "--instance", "--"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
        let expected = if argv.contains(&"--") { "instance --" } else { "instance probe" };
        assert!(stdout(&output).contains(expected), "{argv:?}: {}", stdout(&output));
    }

    // `--hub` before the subcommand and both spellings reach the same check; this is the installer's form.
    for argv in [
        vec!["agent", "enroll", "-", "--hub", "wss://hub.example"],
        vec!["agent", "enroll", "-", "--hub", "wss://hub.example", "--instance", "probe"],
        vec!["agent", "enroll", "--hub=wss://hub.example", "-"],
        vec!["agent", "--hub", "wss://hub.example", "enroll", "-"],
        vec!["agent", "--hub", "wss://hub.example", "--instance", "probe", "enroll", "-"],
        vec!["agent", "--instance", "probe", "enroll", "-", "--hub", "wss://hub.example"],
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

    // A bad code is refused; `--` is clap's terminator, so `--hub` after it is an extra word.
    for (argv, expected, says) in [
        (vec!["agent", "enroll", "nonsense", "--hub", "wss://hub.example"], 1, "an enrollment code looks like"),
        (vec!["agent", "enroll", "--", "-", "--hub", "wss://hub.example"], 2, "Usage:"),
        (vec!["agent", "enroll", "-", "--", "--hub", "wss://hub.example"], 2, "Usage:"),
        (vec!["agent", "enroll", "--", "-"], 1, "enroll needs the hub"),
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), expected, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(stderr(&output).contains(says), "{argv:?}: {}", stderr(&output));
    }

    // Every --instance is validated, and `--hub --instance …` is now refused (--hub takes --instance as
    // its value; no caller uses that form).
    for argv in [
        vec!["agent", "status", "--instance", "Bad", "--instance", "probe"],
        vec!["agent", "status", "--instance=Bad", "--instance=probe"],
        vec!["agent", "--instance", "Bad", "--instance", "probe", "status"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(stderr(&output).contains("must be 1-32 characters"), "{argv:?}: {}", stderr(&output));
    }
    for argv in [
        vec!["agent", "enroll", "-", "--hub", "--instance", "probe", "wss://hub.example"],
        vec!["agent", "enroll", "-", "--hub", "--instance=probe", "wss://hub.example"],
        vec!["agent", "--hub", "--instance", "probe", "wss://hub.example", "enroll", "-"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stderr(&output).contains("error:"), "{argv:?}: {}", stderr(&output));
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
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
}

/// `user`, `setup-code` and `open`: the accepted shapes, the flags each action refuses, and the
/// every-`--instance`-is-validated rule. Named hyphens work; `--help` is clap's.
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
        vec!["user", "list", "--instance", "--x"],
        vec!["user", "--instance", "--", "list"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
    }

    // A word that looks like a flag is a user name: `--help` here is not help, and with no
    // password on stdin no account is made.
    let output = invocation.run(&["user", "add", "--help"]);
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("at least 12 characters"), "{}", stderr(&output));
    let listed = invocation.run(&["user", "list"]);
    assert_eq!(code(&listed), 0, "{}", stderr(&listed));
    assert!(!stdout(&listed).contains("--help"), "no account was made: {}", stdout(&listed));

    // Flags are still only meaningful to the actions that read them.
    for (argv, expected) in [
        (vec!["user", "add", "ada", "--yes"], 1),
        (vec!["user", "passwd", "ada", "--app"], 1),
        (vec!["user", "rm", "ada", "--app"], 1),
        (vec!["user", "mark-app", "ada", "--yes"], 1),
        (vec!["user", "--instance", "Bad Name", "list"], 2),
        (vec!["user", "--", "list"], 2),
        // A name of exactly `--` ends clap's options in these forms.
        (vec!["user", "add", "--", "--instance", "probe"], 2),
        (vec!["user", "rm", "--", "--yes", "--instance", "probe"], 2),
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), expected, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }

    // Every --instance is validated; an invalid earlier name is refused even when a good one follows,
    // and nothing is created for the good one.
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
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(stderr(&output).contains("must be 1-32 characters"), "{argv:?}: {}", stderr(&output));
    }
    assert!(!fresh.home.path().join("data/instances/probe/state.db").exists(), "no files for the good name");

    // `setup-code --` is now clap's terminator (an accepted spelling) and runs the command.
    let output = invocation.run(&["setup-code", "--"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no setup code"), "{}", stderr(&output));

    for (argv, expected) in [
        (vec!["setup-code", "junk"], 2),
        (vec!["setup-code", "--", "x"], 2),
        (vec!["open", "--print", "--print"], 1),
        (vec!["open", "--print", "extra"], 2),
        (vec!["open", "--", "--print"], 2),
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), expected, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}: {}", stdout(&output));
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
    // `open --` is an accepted spelling (it opens the page; with no server here it refuses at runtime).
    let output = invocation.run(&["open", "--"]);
    assert_eq!(code(&output), 1);
    assert!(!stderr(&output).is_empty(), "{}", stderr(&output));
}

/// `plugin` and `notify`: clap parses their options now (`--out=…`, `-lactive`, repeats), while the
/// grammar's refusals (an action that does not take a flag, a missing option value) remain.
#[test]
fn plugin_and_notify_argv_keep_their_shape() {
    let invocation = Invocation::new();

    let output = invocation.run(&["plugin", "list"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let output = invocation.run(&["plugin", "list", "--instance", "--hub"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));

    // Every --instance is validated before anything else.
    let fresh = Invocation::new();
    for argv in [
        vec!["plugin", "list", "--instance", "Bad", "--instance", "probe"],
        vec!["plugin", "list", "--instance=Bad", "--instance=probe"],
    ] {
        let output = fresh.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stderr(&output).contains("must be 1-32 characters"), "{argv:?}: {}", stderr(&output));
    }
    assert!(!fresh.home.path().join("data/instances/probe/state.db").exists());

    // A bare `--` is clap's terminator: `plugin -- list` has no positional at the root, so it is refused;
    // `install -- file` and `uninstall -- tasks` are accepted.
    for argv in [vec!["plugin", "--", "list"], vec!["plugin", "purge", "--", "tasks", "--yes"], vec!["plugin", "export", "tasks", "--", "--out", "f"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
    for argv in [vec!["plugin", "install", "--", "file"], vec!["plugin", "uninstall", "--", "tasks"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}, which reaches the store and fails there");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }

    // A bare `--` is an `--out` value, and reaches the store (which does not exist here).
    let output = invocation.run(&["plugin", "export", "tasks", "--out", "--"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("no store"), "{}", stderr(&output));

    // New spellings clap accepts, with the same meaning.
    for argv in [
        vec!["plugin", "export", "tasks", "--out=x"],
        vec!["plugin", "export", "--out=x", "tasks"],
        vec!["plugin", "import", "tasks", "--replace", "f.json"],
        vec!["plugin", "import", "--replace", "tasks", "f.json"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(!stderr(&output).contains("Usage:"), "{argv:?} parsed: {}", stderr(&output));
    }
    let output = invocation.run(&["plugin", "purge", "tasks", "--yes", "--yes"]);
    assert_eq!(code(&output), 0, "repeated --yes counts: {}", stderr(&output));
    let output = invocation.run(&["plugin", "purge", "--yes", "tasks"]);
    assert_eq!(code(&output), 0, "--yes before the id: {}", stderr(&output));

    for argv in [
        vec!["plugin", "export", "tasks", "--out"],
        vec!["plugin", "import", "tasks", "--replace"],
        vec!["plugin", "list", "extra"],
        vec!["plugin", "purge", "tasks", "--yes=x"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
    let output = invocation.run(&["plugin", "purge", "tasks"]);
    assert_eq!(code(&output), 1);
    assert!(stderr(&output).contains("--yes"), "{}", stderr(&output));

    // notify reaches the missing channel only when the argv parsed.
    for argv in [
        vec!["notify", "T"],
        vec!["notify", "T", "--level", "active"],
        vec!["notify", "T", "--level=active"],
        vec!["notify", "T", "-lpassive"],
        vec!["notify", "--level=active", "T", "B"],
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

    for argv in [vec!["notify", "T", "--nope"], vec!["notify", "T", "--level"], vec!["notify", "-x"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(!stderr(&output).contains("no channel"), "{argv:?} was parsed: {}", stderr(&output));
    }
}

/// `replay` takes clap spellings now; the shims keep the mandatory separator and the values that look
/// like flags (they are spawned by the server).
#[test]
fn replay_and_the_shims_keep_their_grammar() {
    let invocation = Invocation::new();

    // These parse and reach the file/spawn work (which fails in this empty home).
    for argv in [
        vec!["replay", "--log", "/nonexistent/a.ndjson", "--home", "/tmp/probe-replay-home"],
        vec!["replay", "--home", "/tmp/probe-replay-home", "--log", "--"],
        vec!["replay", "--log", "/nonexistent/a.ndjson", "--home", "/tmp/probe-replay-home", "--speed", "4", "--instant"],
        vec!["replay", "--log=/nonexistent/a.ndjson", "--home", "/tmp/probe-replay-home"],
        vec!["replay", "--log", "/nonexistent/a.ndjson", "--home", "/tmp/probe-replay-home", "--"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(!stderr(&output).to_lowercase().contains("usage"), "{argv:?} was parsed: {}", stderr(&output));
    }
    for argv in [
        vec!["replay", "--log", "a", "--home", "b", "--nope"],
        vec!["replay", "--log", "a", "--home", "b", "extra"],
        vec!["replay", "--log", "a", "--home", "b", "--speed", "0"],
        vec!["replay", "--log"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }

    // A value may be `--`, and the command after the separator is never parsed as a flag.
    for argv in [
        vec!["pty-shim", "--socket", "/tmp/s", "--exit-file", "/tmp/e", "--cols", "x", "--rows", "y", "--", "/nonexistent-cmd"],
        vec!["pty-shim", "--socket", "--", "--exit-file", "/tmp/e", "--", "/nonexistent-cmd", "--socket", "-c"],
        vec!["session-shim", "--socket", "/tmp/s", "--stderr-file", "/tmp/err", "--exit-file", "/tmp/e", "--buffer-bytes", "x", "--", "/nonexistent-cmd"],
        vec!["pty-shim", "--socket=/tmp/s", "--exit-file", "/tmp/e", "--", "/nonexistent-cmd"],
        vec!["session-shim", "--socket", "/tmp/s", "--stderr-file=/tmp/err", "--exit-file", "/tmp/e", "--", "/nonexistent-cmd"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 1, "{argv:?}");
        assert!(stderr(&output).contains("cannot spawn"), "{argv:?} was parsed: {}", stderr(&output));
    }
    for argv in [
        vec!["pty-shim", "--socket", "/tmp/s", "--exit-file", "/tmp/e", "/bin/true"],
        vec!["pty-shim", "--socket", "/tmp/s", "--exit-file", "/tmp/e", "--"],
        vec!["session-shim", "--socket", "/tmp/s", "--stderr-file", "/tmp/err", "--exit-file", "/tmp/e"],
    ] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
}

/// `export-openapi` writes to its `--out` (either spelling, the last one when repeated) and refuses
/// any other argument.
#[test]
fn export_openapi_takes_one_out() {
    let invocation = Invocation::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first.json");
    let second = dir.path().join("second.json");
    let third = dir.path().join("third.json");
    let refused = dir.path().join("refused.json");

    let output = invocation.run(&["export-openapi", "--out", first.to_str().expect("utf8")]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let spec = std::fs::read_to_string(&first).expect("the spec was written");
    assert!(spec.trim_start().starts_with('{'), "{}", &spec[..spec.len().min(80)]);

    let output = invocation.run(&["export-openapi", &format!("--out={}", second.display())]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert_eq!(std::fs::read_to_string(&second).expect("the spec was written"), spec);

    let output = invocation.run(&["export-openapi", "--out", "/nonexistent/never.json", "--out", third.to_str().expect("utf8")]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(third.exists(), "the last --out wins");

    let output = invocation.run(&["export-openapi", "--out", refused.to_str().expect("utf8"), "junk"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(!refused.exists(), "a refused command line writes nothing");
}

/// `preflight` still ignores its tail; the ignored/forwarded tails are not clap grammars.
#[test]
fn the_legacy_tails_are_untouched() {
    let invocation = Invocation::new();

    for argv in [vec!["preflight", "junk"], vec!["preflight", "--help"], vec!["preflight", "junk", "--x"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 0, "{argv:?}: {}", stderr(&output));
        assert!(stdout(&output).contains("ok (instance probe)"), "{argv:?}: {}", stdout(&output));
    }

    // A bare `--` before a root's positional words is still not a command here.
    for argv in [vec!["user", "--", "list"], vec!["plugin", "--", "list"], vec!["replay", "--", "--log", "x"]] {
        let output = invocation.run(&argv);
        assert_eq!(code(&output), 2, "{argv:?}");
        assert!(stdout(&output).is_empty(), "{argv:?}");
        assert!(!stderr(&output).is_empty(), "{argv:?}");
    }
}
