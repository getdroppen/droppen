//! Pseudo-terminals that outlive the server.
//!
//! A pty owned by the server process dies with it: restart the server — to
//! upgrade it, because it crashed, because a container was rescheduled — and
//! every shell dies with it, along with whatever was running in it. So the pty
//! lives one process out.
//!
//! ```text
//!   browser  ──websocket──▶  server  ──unix socket──▶  shim  ──pty──▶  shell
//!                            (dies and comes back)     (does not)
//! ```
//!
//! The shim is this same binary under a hidden subcommand ([`child`]), spawned
//! detached and in its own process group. It owns the pty, records every byte in
//! a bounded ring, and keeps a `vt100` model of the screen so a client that
//! joins late can be repainted rather than replayed. The server holds nothing
//! but a socket and a byte offset, both of which it can rebuild from the
//! instance directory at boot.
//!
//! * [`proto`] — the frozen frame format, and the sidecar shapes;
//! * [`ring`] — the ring and the screen model, updated under one lock;
//! * [`child`] — the shim process;
//! * [`client`] — the server's end of the link;
//! * [`sidecar`] — the four files one surviving terminal leaves on disk.
//!
//! Everything lives under `<data>/instances/<name>/pty-shims/`, which is the
//! [`crate::instance`] directory one server exclusively owns. That is not
//! decoration: adoption is "attach to whatever is in this directory", and two
//! servers doing that to one directory would fight over every shell in it.

pub mod child;
pub mod client;
pub mod proto;
pub mod ring;
pub mod sidecar;

use std::path::PathBuf;

/// How much output the server may consume before it records how far it got.
///
/// The cursor is deliberately allowed to lag by this much, and is deliberately
/// *not* written on a clean shutdown either. Both follow from the same
/// asymmetry: output re-delivered after a restart is painted over milliseconds
/// later by the repaint that follows it, and nobody notices; output skipped
/// because a cursor ran ahead of what actually reached a browser is gone. So the
/// cursor is always a little behind the truth, on purpose, and a restart is the
/// same shape whether the server exited or was killed.
pub const CURSOR_FLUSH_EVERY: u64 = 64 * 1024;

/// What the browser is shown where output was lost.
///
/// Visible, counted, and in the stream at the point the loss happened. The
/// alternative — splicing the two halves together and saying nothing — produces
/// a scrollback that reads as continuous and is not, which is the one thing a
/// terminal must never do.
pub fn gap_note(lost: u64) -> String {
    format!("\r\n[Droppen] output gap: {lost} bytes lost\r\n")
}

/// The binary to spawn a shim from.
///
/// Normally this process's own executable: the shim is a subcommand of the
/// server, so there is nothing else to find. Two escapes exist for tests, which
/// run inside a test harness rather than inside the server:
///
/// * `DROPPEN_SHIM_BIN` (or its old name, `REMOTE_HAND_SHIM_BIN`) names the
///   binary outright, which is what the cross-process survival test uses;
/// * failing that, a `current_exe` inside cargo's `deps/` directory is a test
///   binary, and the server it links sits one level up under its own name.
///
/// Neither path is reachable from a real deployment, where `current_exe` is the
/// server itself.
pub fn shim_program() -> PathBuf {
    let override_path = crate::env::var("DROPPEN_SHIM_BIN").map(PathBuf::from);
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("remote-hand-server"));
    resolve_shim_program(override_path, exe, |path| path.is_file())
}

/// The decision [`shim_program`] makes, with the environment supplied.
///
/// Split out so it can be asserted on without writing to a process-global
/// variable that every other test in this binary also reads.
fn resolve_shim_program(override_path: Option<PathBuf>, exe: PathBuf, exists: impl Fn(&std::path::Path) -> bool) -> PathBuf {
    if let Some(path) = override_path {
        return path;
    }
    if exe.parent().and_then(|dir| dir.file_name()) == Some(std::ffi::OsStr::new("deps")) {
        if let Some(target) = exe.parent().and_then(|deps| deps.parent()) {
            let candidate = target.join("remote-hand-server");
            if exists(&candidate) {
                return candidate;
            }
        }
    }
    exe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gap_note_says_how_much_was_lost_on_its_own_line() {
        let note = gap_note(4096);
        assert_eq!(note, "\r\n[Droppen] output gap: 4096 bytes lost\r\n");
        assert!(
            note.starts_with("\r\n") && note.ends_with("\r\n"),
            "it never runs into the surrounding output"
        );
    }

    #[test]
    fn an_explicit_shim_binary_wins_and_a_test_binary_falls_back_to_the_server() {
        let named = Some(PathBuf::from("/opt/remote-hand/bin/server"));
        assert_eq!(
            resolve_shim_program(named.clone(), PathBuf::from("/anything"), |_| true),
            PathBuf::from("/opt/remote-hand/bin/server")
        );

        // A cargo integration-test binary: the server sits one level above it.
        let test_binary = PathBuf::from("/w/target/debug/deps/pty_shim-1a2b3c");
        assert_eq!(
            resolve_shim_program(None, test_binary.clone(), |_| true),
            PathBuf::from("/w/target/debug/remote-hand-server")
        );
        assert_eq!(
            resolve_shim_program(None, test_binary, |_| false),
            PathBuf::from("/w/target/debug/deps/pty_shim-1a2b3c"),
            "with nothing to fall back to, the running executable stands"
        );

        // A real deployment: `current_exe` is the server, and is used as it is.
        assert_eq!(
            resolve_shim_program(None, PathBuf::from("/usr/local/bin/remote-hand-server"), |_| true),
            PathBuf::from("/usr/local/bin/remote-hand-server")
        );
    }
}
