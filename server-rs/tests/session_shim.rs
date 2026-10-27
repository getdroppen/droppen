//! A resident claude, across a server that goes away.
//!
//! The unit tests in `inject::shim` run the shim inside the test process, which
//! proves the protocol and nothing about the deployment. These run the **real
//! binary** under its `session-shim` subcommand, which is the only way to find
//! out whether the subcommand is wired up at all — and then kill the client and
//! come back to it, because a shim that does not survive that is not a shim.
//!
//! `/bin/cat` stands in for claude: it reads lines from stdin, writes them to
//! stdout, and exits when its stdin closes, which is the whole of the contract
//! this module depends on.

use std::collections::BTreeMap;
use std::time::Duration;

use remote_hand_server::inject::shim::{self, LinkError, Output, Sidecars, Spawn};

mod common;

/// Point `shim_program` at the binary cargo just built for this test.
///
/// Set once for the whole process rather than per test: it is a process-global,
/// and every test in this file wants the same value.
fn use_the_real_binary() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("DROPPEN_SHIM_BIN", env!("CARGO_BIN_EXE_remote-hand-server"));
    });
}

/// Enough environment for `/bin/cat` to run.
fn env() -> BTreeMap<String, String> {
    ["PATH", "HOME"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_string(), value)))
        .collect()
}

/// The next line, or a panic rather than a test that hangs forever.
async fn next(link: &mut shim::Link) -> Output {
    tokio::time::timeout(Duration::from_secs(10), link.recv())
        .await
        .expect("a line within ten seconds")
        .expect("the link is still open")
}

/// Wait for the tombstone the shim writes before it closes the stream.
async fn wait_for_tombstone(sidecars: &Sidecars) -> shim::Exit {
    for _ in 0..200 {
        if let Some(exit) = sidecars.read_tombstone() {
            return exit;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no tombstone after five seconds");
}

#[tokio::test]
async fn claude_outlives_the_server_that_started_it() {
    use_the_real_binary();
    let dir = tempfile::tempdir().expect("tempdir");
    let sidecars = Sidecars::new(dir.path().join("session-shims"), "0011223344556677");
    let env = env();

    let mut link = shim::spawn_via_shim(Spawn {
        sidecars: &sidecars,
        program: "/bin/cat",
        args: &[],
        workdir: Some(dir.path()),
        env: &env,
        buffer_bytes: None,
    })
    .await
    .expect("the binary knows the session-shim subcommand");

    let claude = link.claude_pid();
    assert!(claude > 1, "the hello carries a real pid");
    assert!(link.shim_pid().is_some(), "a shim we spawned is a child we know");
    link.send_line("{\"type\":\"user\",\"n\":1}").await.expect("send");
    assert_eq!(next(&mut link).await, Output::Line("{\"type\":\"user\",\"n\":1}".into()));

    // The server is upgraded: this process's end of everything goes away.
    drop(link);

    // The new server finds the socket and picks the session back up. There is
    // no "old link is gone" signal in the protocol to wait for: `accept_loop`
    // serves every connection on its own, and a completed handshake takes a new
    // generation, which supersedes the dropped link's tasks. Nothing below
    // depends on the shim having noticed the old link, so this retries only the
    // new handshake itself.
    let sidecars_ref = &sidecars;
    let mut link = common::wait_for_async(
        "the shim to accept a reattach",
        Duration::from_secs(5),
        move || async move { shim::attach(sidecars_ref).await.ok() },
    )
    .await;
    assert_eq!(link.claude_pid(), claude, "the same claude, still mid-session");
    assert!(link.shim_pid().is_none(), "the shim belongs to a server that is gone");
    assert_eq!(
        next(&mut link).await,
        Output::Line("{\"type\":\"user\",\"n\":1}".into()),
        "the reattach replays from zero"
    );

    // And it still works, which is the difference between surviving and being a
    // readable corpse.
    link.send_line("{\"type\":\"user\",\"n\":2}").await.expect("send");
    assert_eq!(next(&mut link).await, Output::Line("{\"type\":\"user\",\"n\":2}".into()));

    link.close_stdin().await.expect("close");
    assert_eq!(wait_for_tombstone(&sidecars).await.status, Some(0));
    drop(link);
}

#[tokio::test]
async fn a_shim_whose_claude_never_started_reports_a_corpse_rather_than_a_session() {
    use_the_real_binary();
    let dir = tempfile::tempdir().expect("tempdir");
    let sidecars = Sidecars::new(dir.path().join("session-shims"), "8899aabbccddeeff");
    let env = env();

    // A program that exits immediately: the shim comes up, buries it, and
    // lingers with a live socket over a dead claude.
    let link = shim::spawn_via_shim(Spawn {
        sidecars: &sidecars,
        program: "/usr/bin/false",
        args: &[],
        workdir: Some(dir.path()),
        env: &env,
        buffer_bytes: None,
    })
    .await
    .expect("the shim binds even for a program that is already gone");
    drop(link);

    let exit = wait_for_tombstone(&sidecars).await;
    assert_eq!(exit.status, Some(1));

    let refused = shim::attach(&sidecars).await;
    assert!(
        matches!(refused, Err(LinkError::Corpse(_))),
        "a tombstone means spawn a fresh one, never attach: {refused:?}"
    );
}
