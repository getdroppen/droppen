//! The rename to Droppen: the server reads
//! `DROPPEN_*` first and the old `REMOTE_HAND_*` / `RH_*` names when those are
//! unset, saying once per old name that it is deprecated and stops working in
//! v0.2.135 — by name, never by value, and never on stdout; and what one
//! Droppen program hands another is written under both names and read under
//! either without a word.
//!
//! Checked on the binary, each process started with nothing in its
//! environment but what the case sets (so no case sees another's variables,
//! nor the developer's), its files under a scratch home.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long a server is given to come up, or a supervised child to start.
const BUDGET: Duration = Duration::from_secs(60);

/// The server binary with nothing in its environment but a scratch home, a
/// `PATH` and `env`.
fn server(home: &Path, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_remote-hand-server"));
    command
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("RUST_LOG", "warn")
        .envs(env.iter().copied())
        .stdin(Stdio::null());
    command
}

/// Run a command to its end: whether it succeeded, its stdout and its stderr.
fn run(home: &Path, args: &[&str], env: &[(&str, &str)]) -> (bool, String, String) {
    let out = server(home, args, env).output().expect("run the server binary");
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    (out.status.success(), text(&out.stdout), text(&out.stderr))
}

/// How many lines say `old` is deprecated.
fn deprecations(said: &str, old: &str) -> usize {
    said.lines().filter(|l| l.contains(&format!("{old} is deprecated"))).count()
}

/// The lines about an old name, which must never carry a value.
fn notices(said: &str) -> Vec<&str> {
    said.lines().filter(|l| l.contains("deprecated")).collect()
}

/// A running `serve`, stopped (and its log kept for the assertions) on drop.
struct Serve {
    child: Child,
    log: PathBuf,
}

impl Serve {
    fn start(home: &Path, env: &[(&str, &str)]) -> Serve {
        let log = home.join("serve.log");
        let file = std::fs::File::create(&log).expect("serve.log");
        let child = server(home, &["serve"], env)
            .env("APP_SERVER__HOST", "127.0.0.1")
            .env("APP_SERVER__PORT", "0")
            .stdout(file.try_clone().expect("log"))
            .stderr(file)
            .spawn()
            .expect("start the server");
        Serve { child, log }
    }

    /// The port the server recorded under `data` once it listens there.
    fn port_in(&mut self, data: &Path) -> u16 {
        let port_file = data.join("instances/default/server.port");
        let deadline = Instant::now() + BUDGET;
        loop {
            if let Some(port) = std::fs::read_to_string(&port_file).ok().and_then(|t| t.trim().parse::<u16>().ok()) {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    return port;
                }
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("the server exited ({status}) before listening: {}", self.said());
            }
            assert!(Instant::now() < deadline, "no server.port under {}: {}", data.display(), self.said());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Everything the server has said so far.
    fn said(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Stop it, and everything it said.
    fn stop(mut self) -> String {
        // SAFETY: a signal to the one process this test started.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.try_wait().ok().flatten().is_none() {
            if Instant::now() > deadline {
                let _ = self.child.kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.wait();
        self.said()
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `GET path` on the server at `port`: the whole response, headers and all.
fn get(port: u16, path: &str) -> String {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").expect("request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response");
    response
}

#[test]
fn a_server_with_only_the_new_names_works() {
    let root = tempfile::tempdir().expect("tempdir");
    let (home, data, config) = (root.path().join("home"), root.path().join("data-new"), root.path().join("config-new"));
    std::fs::create_dir_all(&home).expect("home");
    let (data_s, config_s) = (data.to_string_lossy().into_owned(), config.to_string_lossy().into_owned());
    let mut serve = Serve::start(
        &home,
        &[
            ("DROPPEN_DATA_DIR", &data_s),
            ("DROPPEN_CONFIG_DIR", &config_s),
            ("DROPPEN_DESKTOP_APP", "1"),
            ("DROPPEN_SESSION_LOG", "0"),
        ],
    );
    let port = serve.port_in(&data);
    let status = get(port, "/api/auth/status");
    assert!(status.contains("\"desktopApp\":true"), "DROPPEN_DESKTOP_APP was read: {status}");
    let said = serve.stop();
    assert!(notices(&said).is_empty(), "nothing deprecated: {said}");
    assert!(config.is_dir(), "its configuration went to DROPPEN_CONFIG_DIR");
    assert!(!home.join(".local/share/remote-hand").exists(), "not to the default data directory: {said}");
    assert!(!home.join(".config/remote-hand").exists(), "nor the default configuration directory: {said}");
}

#[test]
fn a_server_with_only_the_old_names_still_works_and_says_so_once() {
    let root = tempfile::tempdir().expect("tempdir");
    let (home, data, config) = (root.path().join("home"), root.path().join("data-old"), root.path().join("config-old"));
    std::fs::create_dir_all(&home).expect("home");
    let (data_s, config_s) = (data.to_string_lossy().into_owned(), config.to_string_lossy().into_owned());
    let mut serve = Serve::start(
        &home,
        &[
            ("REMOTE_HAND_DATA_DIR", &data_s),
            ("REMOTE_HAND_CONFIG_DIR", &config_s),
            ("RH_SESSION_LOG", "0"),
            // What an app or an `up` from before the rename sets: read, and
            // nothing said about it — nobody wrote it into a configuration.
            ("RH_DESKTOP_APP", "1"),
            ("RH_SUPERVISED", "1"),
        ],
    );
    let port = serve.port_in(&data);
    let status = get(port, "/api/auth/status");
    assert!(status.contains("\"desktopApp\":true"), "RH_DESKTOP_APP was read: {status}");
    let said = serve.stop();
    // `Paths` is built more than once at boot; each old name is said once.
    for (old, new) in [
        ("REMOTE_HAND_DATA_DIR", "DROPPEN_DATA_DIR"),
        ("REMOTE_HAND_CONFIG_DIR", "DROPPEN_CONFIG_DIR"),
        ("RH_SESSION_LOG", "DROPPEN_SESSION_LOG"),
    ] {
        assert_eq!(deprecations(&said, old), 1, "{old}: {said}");
        assert!(
            said.contains(&format!("{old} is deprecated: add {new} with the same value; {old} stops working in v0.2.135")),
            "{said}"
        );
    }
    for handoff in ["RH_DESKTOP_APP", "RH_SUPERVISED"] {
        assert!(!said.contains(handoff), "{handoff} is a handoff, not a setting: {said}");
    }
    for line in notices(&said) {
        assert!(!line.contains(&data_s) && !line.contains(&config_s), "a name, never a value: {line}");
    }
    assert!(config.is_dir(), "its configuration went to REMOTE_HAND_CONFIG_DIR");
    assert!(!home.join(".local/share/remote-hand").exists(), "not to the default data directory: {said}");
}

#[test]
fn the_new_name_wins_when_both_are_set_to_different_values() {
    let root = tempfile::tempdir().expect("tempdir");
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let (new, old) = (root.path().join("data-new"), root.path().join("data-old"));
    let (new_s, old_s) = (new.to_string_lossy().into_owned(), old.to_string_lossy().into_owned());
    let config_s = root.path().join("config").to_string_lossy().into_owned();
    let mut serve = Serve::start(
        &home,
        &[
            ("DROPPEN_DATA_DIR", &new_s),
            ("REMOTE_HAND_DATA_DIR", &old_s),
            ("DROPPEN_CONFIG_DIR", &config_s),
            // The switch-over: both names, one value. Nothing to say.
            ("REMOTE_HAND_CONFIG_DIR", &config_s),
        ],
    );
    serve.port_in(&new);
    let said = serve.stop();
    assert!(!old.exists(), "the old name's directory was not used: {said}");
    let both = "DROPPEN_DATA_DIR and the deprecated REMOTE_HAND_DATA_DIR are both set, to different values: DROPPEN_DATA_DIR is used";
    assert_eq!(said.lines().filter(|l| l.contains(both)).count(), 1, "{said}");
    // (The setup banner does name it: the commands it suggests carry both.)
    assert!(
        notices(&said).iter().all(|line| !line.contains("REMOTE_HAND_CONFIG_DIR")),
        "the same value under both names says nothing: {said}"
    );
    assert_eq!(notices(&said).len(), 1, "{said}");
}

/// A command that prints its answer on stdout says what it has to say about an
/// old name on stderr: a script reading the answer gets the answer.
#[test]
fn a_command_says_it_on_stderr_and_answers_on_stdout() {
    let root = tempfile::tempdir().expect("tempdir");
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let dir = |name: &str| root.path().join(name).to_string_lossy().into_owned();
    let (new_data, new_config, old_data, old_config) = (dir("data-new"), dir("config-new"), dir("data-old"), dir("config-old"));

    let (ok, stdout, stderr) = run(
        &home,
        &["user", "list"],
        &[("REMOTE_HAND_DATA_DIR", &old_data), ("REMOTE_HAND_CONFIG_DIR", &old_config)],
    );
    assert!(ok, "{stdout}{stderr}");
    assert!(stdout.starts_with("no accounts on instance default") && stdout.contains("-old"), "the old names' files: {stdout}");
    assert!(notices(&stdout).is_empty(), "nothing but the answer on stdout: {stdout}");
    assert_eq!(deprecations(&stderr, "REMOTE_HAND_DATA_DIR"), 1, "{stderr}");
    assert_eq!(deprecations(&stderr, "REMOTE_HAND_CONFIG_DIR"), 1, "{stderr}");
    assert!(
        stderr.lines().all(|l| l.starts_with("remote-hand: warning: ")),
        "prefixed like this binary's errors: {stderr}"
    );

    let (ok, stdout, stderr) = run(&home, &["user", "list"], &[("DROPPEN_DATA_DIR", &new_data), ("DROPPEN_CONFIG_DIR", &new_config)]);
    assert!(ok, "{stdout}{stderr}");
    assert!(stdout.contains("-new") && !stdout.contains("-old"), "the new names' files: {stdout}");
    assert!(stderr.is_empty(), "nothing to say: {stderr}");

    // `--version` reads nothing renamed and says nothing, whatever is set: a
    // self-upgrade reads all of its stdout as the version.
    let (ok, stdout, stderr) = run(&home, &["--version"], &[("REMOTE_HAND_DATA_DIR", &old_data)]);
    assert!(ok && stdout.lines().count() == 1 && stderr.is_empty(), "{stdout:?} {stderr:?}");
}

/// A session's `droppen-notify` runs `notify` with what the server that
/// started the session injected: both names from a server since the rename,
/// only the old ones from a server before it. Either reaches the server, and
/// an agent reading the answer is not told about names it did not choose.
#[test]
fn notify_reads_what_a_server_old_or_new_handed_its_session() {
    let root = tempfile::tempdir().expect("tempdir");
    // A port nothing listens on: reaching for it proves the channel was read.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let url = format!("http://127.0.0.1:{closed}/api/notify");
    for env in [
        vec![("RH_NOTIFY_URL", url.as_str()), ("RH_NOTIFY_TOKEN", "t")],
        vec![("DROPPEN_NOTIFY_URL", url.as_str()), ("DROPPEN_NOTIFY_TOKEN", "t")],
        vec![("DROPPEN_NOTIFY_URL", url.as_str()), ("DROPPEN_NOTIFY_TOKEN", "t"), ("RH_NOTIFY_URL", url.as_str()), ("RH_NOTIFY_TOKEN", "t")],
    ] {
        let (ok, stdout, stderr) = run(root.path(), &["notify", "done"], &env);
        assert!(!ok && stderr.contains("cannot reach the Droppen server"), "{env:?}: {stdout}{stderr}");
        assert!(!stderr.contains("deprecated") && !stderr.contains("RH_"), "{env:?}: {stderr}");
    }
    let (ok, _, stderr) = run(root.path(), &["notify", "done"], &[]);
    assert!(!ok && stderr.contains("no channel was configured"), "{stderr}");

    // A session started by a server from before the rename, itself running in
    // a newer server's session: the old names are its own channel, written
    // over the new ones it inherited from outside. The old names win.
    let (ok, stdout, stderr) = run(
        root.path(),
        &["notify", "done"],
        &[
            ("DROPPEN_NOTIFY_URL", "http://outer.invalid/api/notify"),
            ("DROPPEN_NOTIFY_TOKEN", "outer"),
            ("RH_NOTIFY_URL", url.as_str()),
            ("RH_NOTIFY_TOKEN", "inner"),
        ],
    );
    assert!(!ok && stderr.contains("cannot reach the Droppen server"), "{stdout}{stderr}");
    assert!(stderr.contains(&closed.to_string()) && !stderr.contains("outer.invalid"), "the inner channel: {stderr}");
    assert!(!stderr.contains("deprecated") && !stderr.contains("RH_"), "{stderr}");
}

/// `up` tells the server it runs that it is supervised under both names, and
/// hands on each setting it was given under a new name under the old one
/// too, so a server from before the rename — which `up` runs after a rollback
/// to `.prev` — still knows it may restart itself, and still runs on this
/// install's files rather than the default install's. The installed binary
/// here is a script standing in for such a server: it writes down what it was
/// given.
#[test]
fn up_hands_its_server_both_names() {
    let root = tempfile::tempdir().expect("tempdir");
    let (home, data) = (root.path().join("home"), root.path().join("data"));
    let bin = data.join("bin");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&bin).expect("bin");
    let seen = root.path().join("child.env");
    let installed = bin.join("remote-hand-server");
    std::fs::write(&installed, format!("#!/bin/sh\nenv > '{}.tmp'\nmv '{0}.tmp' '{0}'\nexec sleep 60\n", seen.display())).expect("fake server");
    std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let data_s = data.to_string_lossy().into_owned();
    let config_s = root.path().join("config").to_string_lossy().into_owned();
    let env = [
        ("DROPPEN_DATA_DIR", data_s.as_str()),
        ("DROPPEN_CONFIG_DIR", config_s.as_str()),
        ("APP_INSTANCE", "bob"),
        // Only under the old name: the child inherits it as it is.
        ("RH_STALL_MS", "5000"),
        // Empty, which for this one is a value ("no STUN servers").
        ("DROPPEN_AGENT_DIRECT_STUN", ""),
    ];
    let mut up = server(&home, &["up", "--", "serve"], &env)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start up");
    let deadline = Instant::now() + BUDGET;
    while !seen.exists() {
        if let Ok(Some(status)) = up.try_wait() {
            let mut said = String::new();
            let _ = up.stderr.take().map(|mut e| e.read_to_string(&mut said));
            panic!("up exited ({status}) before its server started: {said}");
        }
        assert!(Instant::now() < deadline, "up never started the installed binary");
        std::thread::sleep(Duration::from_millis(50));
    }
    // `up` passes a stop on to its server and then exits.
    // SAFETY: a signal to the one process this test started.
    unsafe { libc::kill(up.id() as libc::pid_t, libc::SIGTERM) };
    let stopped = Instant::now() + Duration::from_secs(10);
    while up.try_wait().ok().flatten().is_none() {
        if Instant::now() > stopped {
            let _ = up.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = up.wait();

    let seen = std::fs::read_to_string(&seen).expect("the child's environment");
    let expected = [
        "DROPPEN_SUPERVISED=1".to_string(),
        "RH_SUPERVISED=1".to_string(),
        format!("DROPPEN_DATA_DIR={data_s}"),
        format!("REMOTE_HAND_DATA_DIR={data_s}"),
        format!("DROPPEN_CONFIG_DIR={config_s}"),
        format!("REMOTE_HAND_CONFIG_DIR={config_s}"),
        "APP_INSTANCE=bob".to_string(),
        "RH_STALL_MS=5000".to_string(),
        "DROPPEN_AGENT_DIRECT_STUN=".to_string(),
        "RH_AGENT_DIRECT_STUN=".to_string(),
    ];
    for line in &expected {
        assert!(seen.lines().any(|l| l == line), "{line} missing from {seen}");
    }
    assert!(!seen.lines().any(|l| l.starts_with("DROPPEN_STALL_MS=")), "nothing added under a new name: {seen}");
}
