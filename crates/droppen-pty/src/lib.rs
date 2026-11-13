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
//! instance directory one server exclusively owns (the server's `instance`).
//! That is not decoration: adoption is "attach to whatever is in this
//! directory", and two servers doing that to one directory would fight over
//! every shell in it.

pub mod child;
pub mod client;
pub mod proto;
pub mod ring;
pub mod sidecar;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

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
/// This process's own executable: the shim is a subcommand of the server, so
/// there is nothing else to find. A test, which runs inside a test harness
/// rather than inside the server, names the server binary instead
/// (`testing::shims_run_from`, with `test-support`).
///
/// Never worked out from where the executable is. A test binary in cargo's
/// `deps/` used to be taken to have the server one level up, and cargo 1.100
/// moved test binaries to `build/<package>/<hash>/out/`: the guess fell
/// through to the test binary, every shim a test started was libtest refusing
/// `--socket`, and no terminal in any test could open. Where cargo keeps its
/// intermediate files is not something it promises to keep.
pub fn shim_program() -> PathBuf {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(program) = testing::shim_program() {
        return program;
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from(droppen_base::program::DROPPEN_SERVER))
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

    /// Nothing is read off where the executable is: this test binary is in
    /// cargo's build directory, with a server built near it or not, and until
    /// a test names another program the shim is this executable.
    #[test]
    fn with_no_program_named_the_shim_is_this_executable() {
        assert_eq!(shim_program(), std::env::current_exe().expect("this test binary"));
    }
}
