//! The server's environment names are its `DROPPEN_*` ones: the names from
//! before the rename to Droppen (`REMOTE_HAND_*`, `RH_*`) are ignored, and the
//! programs the server starts get only the `DROPPEN_*` names. Of the old
//! names, `serve` says something only about the two directory names whose
//! silence would move its files.
//!
//! Checked on the binary, each process started with nothing in its
//! environment but what the case sets (so no case sees another's variables,
//! nor the developer's), its files under a scratch home.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long a server is given to come up, or a supervised child to start.
const BUDGET: Duration = Duration::from_secs(60);

mod common;

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

/// A running `serve`, stopped (and its log kept for the assertions) on drop.
struct Serve {
    child: Child,
    log: PathBuf,
    /// Ends it if the test process dies before it is dropped.
    _reaper: common::Reaper,
}

impl Serve {
    fn start(home: &Path, env: &[(&str, &str)]) -> Serve {
        let log = home.join("serve.log");
        let file = std::fs::File::create(&log).expect("serve.log");
        // A group of its own, which its reaper ends.
        let child = server(home, &["serve"], env)
            .env("APP_SERVER__HOST", "127.0.0.1")
            .env("APP_SERVER__PORT", "0")
            .stdout(file.try_clone().expect("log"))
            .stderr(file)
            .process_group(0)
            .spawn()
            .expect("start the server");
        let reaper = common::Reaper::watch(Some(child.id()), home, "default");
        Serve { child, log, _reaper: reaper }
    }

    /// The port the server recorded under `data` once it listens there; a
    /// failure at once if anything appears at a path of `not_in`.
    fn port_in(&mut self, data: &Path, not_in: &[&Path]) -> u16 {
        let port_file = data.join("instances/default/server.port");
        let deadline = Instant::now() + BUDGET;
        loop {
            if let Some(wrong) = not_in.iter().find(|path| path.exists()) {
                panic!("the server wrote {} instead of {}: {}", wrong.display(), data.display(), self.said());
            }
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
fn a_server_with_the_new_names_uses_them() {
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
            // Moved over as v0.2.134 asked: both names, one value.
            ("REMOTE_HAND_DATA_DIR", &data_s),
        ],
    );
    let port = serve.port_in(&data, &[&home.join(".local/share/remote-hand")]);
    let status = get(port, "/api/auth/status");
    assert!(status.contains("\"desktopApp\":true"), "DROPPEN_DESKTOP_APP was read: {status}");
    let said = serve.stop();
    assert!(!said.contains("REMOTE_HAND_DATA_DIR"), "nothing to say once the new name is set: {said}");
    assert!(config.is_dir(), "its configuration went to DROPPEN_CONFIG_DIR");
    assert!(!home.join(".local/share/remote-hand").exists(), "not to the default data directory: {said}");
    assert!(!home.join(".config/remote-hand").exists(), "nor the default configuration directory: {said}");
}

#[test]
fn a_server_ignores_the_old_names() {
    let root = tempfile::tempdir().expect("tempdir");
    let (home, data, config) = (root.path().join("home"), root.path().join("data-old"), root.path().join("config-old"));
    std::fs::create_dir_all(&home).expect("home");
    let (data_s, config_s) = (data.to_string_lossy().into_owned(), config.to_string_lossy().into_owned());
    let old = [
        ("REMOTE_HAND_DATA_DIR", data_s.as_str()),
        ("REMOTE_HAND_CONFIG_DIR", config_s.as_str()),
        ("RH_DESKTOP_APP", "1"),
    ];
    let mut serve = Serve::start(&home, &old);
    // The default install's files, under the scratch home.
    let port = serve.port_in(&home.join(".local/share/remote-hand"), &[&data, &config]);
    let status = get(port, "/api/auth/status");
    assert!(status.contains("\"desktopApp\":false"), "RH_DESKTOP_APP was not read: {status}");
    let said = serve.stop();
    assert!(home.join(".config/remote-hand").is_dir(), "its configuration went to the default directory: {said}");
    assert!(!data.exists() && !config.exists(), "the old names' directories were not used: {said}");
    // The two names that silently moved its files are said once each, by
    // name only; the desktop app's old name is nobody's setting.
    for (old, new) in [("REMOTE_HAND_DATA_DIR", "DROPPEN_DATA_DIR"), ("REMOTE_HAND_CONFIG_DIR", "DROPPEN_CONFIG_DIR")] {
        let line = format!("{old} is set but no longer read; set {new}");
        assert_eq!(said.lines().filter(|l| l.contains(&line)).count(), 1, "{line}: {said}");
    }
    assert!(said.lines().filter(|l| l.contains("no longer read")).all(|l| !l.contains(&data_s) && !l.contains(&config_s)), "a name, never a value: {said}");
    assert!(!said.contains("RH_DESKTOP_APP"), "{said}");
}

/// A command reads its files by the `DROPPEN_*` names only, and says nothing
/// about an old name on either stream.
#[test]
fn a_command_ignores_the_old_names() {
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
    assert!(stdout.starts_with("no accounts on instance default") && !stdout.contains("-old"), "the default install's files: {stdout}");
    assert!(stderr.is_empty(), "nothing to say: {stderr}");

    let (ok, stdout, stderr) = run(&home, &["user", "list"], &[("DROPPEN_DATA_DIR", &new_data), ("DROPPEN_CONFIG_DIR", &new_config)]);
    assert!(ok, "{stdout}{stderr}");
    assert!(stdout.contains("-new"), "the new names' files: {stdout}");
    assert!(stderr.is_empty(), "nothing to say: {stderr}");
}

/// A session's `droppen-notify` runs `notify` with the channel the server
/// injected under the `DROPPEN_*` names; the old names are no channel, and
/// they lose to the new ones when both are set.
#[test]
fn notify_reads_only_the_new_names() {
    let root = tempfile::tempdir().expect("tempdir");
    // A port nothing listens on: reaching for it proves the channel was read.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let url = format!("http://127.0.0.1:{closed}/api/notify");

    let (ok, stdout, stderr) = run(root.path(), &["notify", "done"], &[("DROPPEN_NOTIFY_URL", &url), ("DROPPEN_NOTIFY_TOKEN", "t")]);
    assert!(!ok && stderr.contains("cannot reach the Droppen server"), "{stdout}{stderr}");

    let (ok, _, stderr) = run(root.path(), &["notify", "done"], &[("RH_NOTIFY_URL", &url), ("RH_NOTIFY_TOKEN", "t")]);
    assert!(!ok && stderr.contains("no channel was configured"), "{stderr}");

    let (ok, stdout, stderr) = run(
        root.path(),
        &["notify", "done"],
        &[
            ("DROPPEN_NOTIFY_URL", url.as_str()),
            ("DROPPEN_NOTIFY_TOKEN", "t"),
            ("RH_NOTIFY_URL", "http://outer.invalid/api/notify"),
            ("RH_NOTIFY_TOKEN", "outer"),
        ],
    );
    assert!(!ok && stderr.contains("cannot reach the Droppen server"), "{stdout}{stderr}");
    assert!(stderr.contains(&closed.to_string()) && !stderr.contains("outer.invalid"), "the DROPPEN_ channel: {stderr}");
    assert!(!stderr.contains("RH_"), "{stderr}");
}

/// `up` tells the server it runs that it is supervised, and hands on what it
/// was given, under the `DROPPEN_*` names only: nothing is copied to an old
/// name, and an old name set by hand reaches the child as it is, untranslated.
/// The installed binary here is a script standing in for the server: it
/// writes down what it was given.
#[test]
fn up_hands_its_server_only_the_new_names() {
    let root = tempfile::tempdir().expect("tempdir");
    let (home, data) = (root.path().join("home"), root.path().join("data"));
    let bin = data.join("bin");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&bin).expect("bin");
    let seen = root.path().join("child.env");
    let installed = bin.join("remote-hand-server");
    // It waits as a server would, for a minute at most, and goes as soon as
    // `up` does: nothing else would end it if this test process died first.
    std::fs::write(
        &installed,
        format!(
            "#!/bin/sh\nenv > '{}.tmp'\nmv '{0}.tmp' '{0}'\nn=0\nwhile [ \"$n\" -lt 300 ] && kill -0 \"$PPID\" 2>/dev/null; do sleep 0.2; n=$((n + 1)); done\n",
            seen.display()
        ),
    )
    .expect("fake server");
    std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let data_s = data.to_string_lossy().into_owned();
    let config_s = root.path().join("config").to_string_lossy().into_owned();
    let env = [
        ("DROPPEN_DATA_DIR", data_s.as_str()),
        ("DROPPEN_CONFIG_DIR", config_s.as_str()),
        ("APP_INSTANCE", "bob"),
        ("RH_STALL_MS", "5000"),
        ("DROPPEN_AGENT_DIRECT_STUN", ""),
    ];
    // A group of its own, which its reaper ends if the test process dies
    // before it is stopped below.
    let mut up = server(&home, &["up", "--", "serve"], &env)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .expect("start up");
    let _reaper = common::Reaper::watch(Some(up.id()), &home, "bob");
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
    for line in [
        "DROPPEN_SUPERVISED=1".to_string(),
        format!("DROPPEN_DATA_DIR={data_s}"),
        format!("DROPPEN_CONFIG_DIR={config_s}"),
        "APP_INSTANCE=bob".to_string(),
        "RH_STALL_MS=5000".to_string(),
        "DROPPEN_AGENT_DIRECT_STUN=".to_string(),
    ] {
        assert!(seen.lines().any(|l| l == line), "{line} missing from {seen}");
    }
    for name in ["RH_SUPERVISED", "REMOTE_HAND_DATA_DIR", "REMOTE_HAND_CONFIG_DIR", "RH_AGENT_DIRECT_STUN", "DROPPEN_STALL_MS"] {
        assert!(!seen.lines().any(|l| l.starts_with(&format!("{name}="))), "{name} was not added: {seen}");
    }
}
