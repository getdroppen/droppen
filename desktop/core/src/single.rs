//! One app per user: a second launch hands over to the first and exits.
//!
//! Per user, in the app's own support directory (0700): an exclusive `flock`
//! on `app.lock` says who is first, and a Unix socket beside it, `app.sock`,
//! carries "show your window" from a second launch to the first. This replaces
//! Tauri's single-instance plugin, whose socket is one fixed path in `/tmp`
//! shared by every account on the Mac: a second macOS user would either find
//! the first user's app there (and quietly exit) or be unable to create their
//! own, depending on who launched first.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

/// The message a second launch sends.
const SHOW: &str = "show";

/// The outcome of [`claim`].
pub enum Claim {
    /// This is the only app for this user. Keep `lock` for the life of the
    /// process; `listener` (when the socket could be made) hears later
    /// launches.
    First {
        /// The held lock.
        lock: File,
        /// The socket later launches connect to.
        listener: Option<UnixListener>,
    },
    /// Another app of this user's is running.
    Running,
}

/// Claim the app for this user, in `dir` (created 0700 if missing).
pub fn claim(dir: &Path) -> io::Result<Claim> {
    crate::dirs::create_private_dir(dir)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("app.lock"))?;
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(Claim::Running)
        } else {
            Err(error)
        };
    }
    let _ = lock.set_len(0);
    let _ = writeln!(&lock, "{}", std::process::id());
    // Holding the lock makes any socket file left here stale: remove and
    // rebind. A path too long for a Unix socket just means no hand-over.
    let socket = dir.join("app.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).ok();
    Ok(Claim::First { lock, listener })
}

/// [`claim`], waiting up to `wait` for a running app to go: what the new
/// version does when the one it updates relaunches it — the old one may still
/// be on its way out, and must not be asked to show a window it is closing.
pub fn claim_waiting(dir: &Path, wait: Duration) -> io::Result<Claim> {
    let deadline = std::time::Instant::now() + wait;
    loop {
        match claim(dir)? {
            Claim::Running if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            claimed => return Ok(claimed),
        }
    }
}

/// Ask the running app to show its window. Whether it heard. Retries briefly
/// in case the first launch just took the lock and has not bound `app.sock` yet.
pub fn ask_to_show(dir: &Path) -> bool {
    let socket = dir.join("app.sock");
    let mut stream = None;
    for attempt in 0..5 {
        match UnixStream::connect(&socket) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error)
                if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
                    && attempt < 4 =>
            {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return false,
        }
    }
    let Some(mut stream) = stream else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    stream.write_all(format!("{SHOW}\n").as_bytes()).is_ok()
}

/// Hear later launches on `listener`, calling `on_show` for each, on a thread
/// of its own for the life of the process.
pub fn listen(listener: UnixListener, on_show: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("droppen-single-instance".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok() && line.trim() == SHOW {
                    on_show();
                }
            }
        })
        .expect("spawn the single-instance listener");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;

    #[test]
    fn the_second_claim_finds_the_first_and_hands_over() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("support");
        let Claim::First { lock, listener } = claim(&dir).expect("first") else {
            panic!("the first claim is first");
        };
        assert_eq!(std::fs::metadata(&dir).expect("dir").permissions().mode() & 0o777, 0o700);
        let (send, shown) = mpsc::channel();
        listen(listener.expect("socket"), move || {
            let _ = send.send(());
        });
        assert!(matches!(claim(&dir).expect("second"), Claim::Running));
        assert!(ask_to_show(&dir));
        shown.recv_timeout(Duration::from_secs(5)).expect("the first heard it");
        drop(lock);
        assert!(matches!(claim(&dir).expect("after the first is gone"), Claim::First { .. }));
    }

    #[test]
    fn a_relaunch_waits_for_the_app_it_replaces_to_go() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("support");
        let Claim::First { lock, .. } = claim(&dir).expect("first") else {
            panic!("the first claim is first");
        };
        assert!(
            matches!(claim_waiting(&dir, Duration::from_millis(300)).expect("waited"), Claim::Running),
            "still there after the wait: running"
        );
        let leaving = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(lock);
        });
        assert!(matches!(
            claim_waiting(&dir, Duration::from_secs(10)).expect("waited"),
            Claim::First { .. }
        ));
        leaving.join().expect("join");
    }

    #[test]
    fn asking_with_nobody_there_is_just_false() {
        let root = tempfile::tempdir().expect("tempdir");
        assert!(!ask_to_show(root.path()));
    }
}
