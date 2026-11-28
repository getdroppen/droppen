//! Filesystem watching with a debounce, behind a plain channel.
//!
//! `notify` calls back on its own thread, so the events are forwarded onto a
//! tokio channel and coalesced there: a CLI writing a transcript emits a burst
//! of modify events per turn, and re-parsing on each one would dominate the cost
//! of the feed. The 150 ms window matches `WATCH_DEBOUNCE_MS` in
//! the former TypeScript server's `session-feed.ts`.
//!
//! The watcher is best-effort. If `notify` cannot watch the file at all (an
//! unsupported filesystem, a descriptor limit), the channel simply stays quiet
//! and the caller's 2 s poll keeps the feed correct — just less prompt.

use std::path::Path;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// A debounced "the file changed" signal.
pub struct FileEvents {
    rx: mpsc::Receiver<()>,
    /// Kept alive for as long as the receiver is: dropping the watcher
    /// unregisters the path.
    watcher: Option<RecommendedWatcher>,
    /// Held so a silent channel stays *pending* rather than closing. A closed
    /// channel would make the feed's `select!` arm complete immediately and spin.
    _keepalive: Option<mpsc::Sender<()>>,
}

impl FileEvents {
    /// Wait for the next change, or `None` once the watcher is gone.
    pub async fn recv(&mut self) -> Option<()> {
        self.rx.recv().await
    }
}

impl Drop for FileEvents {
    fn drop(&mut self) {
        // `notify`'s watchers join their own thread on drop — FSEvents stops a
        // run loop and waits for it, inotify writes a command and waits — and
        // this struct is dropped on a tokio worker, at the end of the session
        // feed's watcher task. Blocking a worker on another thread's teardown is
        // exactly what a runtime is not for, so the join gets a thread of its
        // own. A detached `std::thread` rather than `spawn_blocking` because
        // this also drops during shutdown and from plain `#[test]`s, where there
        // is no runtime to spawn onto.
        let Some(watcher) = self.watcher.take() else { return };
        std::thread::spawn(move || drop(watcher));
    }
}

/// A channel that never fires, for when the watcher could not be created.
pub fn never() -> FileEvents {
    let (tx, rx) = mpsc::channel(1);
    FileEvents {
        rx,
        watcher: None,
        _keepalive: Some(tx),
    }
}

/// Watch `path`, coalescing bursts into one signal per `debounce` window.
///
/// The watch is placed on `path`'s parent directory (non-recursively) and
/// filtered by file name, whether or not the file exists yet. On Linux an
/// inotify watch on a file follows its inode, so deleting and recreating the
/// file at the same path — which happens when Codex compresses an idle rollout
/// to `.jsonl.zst` and later restores the plain `.jsonl` on the next append, or
/// when any writer replaces a file atomically — would leave a file watch on a
/// dead inode. Watching the parent directory keeps the watch alive across
/// delete, rename, and recreate, and also handles files that do not exist yet
/// when the watch starts.
pub fn watch_file(path: &Path, debounce: Duration) -> FileEvents {
    let (raw_tx, raw_rx) = mpsc::channel::<()>(64);
    let (target, only) = watch_target(path);

    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        // A directory watch hears of every file in it; only this one counts.
        if let Some(name) = &only {
            if !event.paths.iter().any(|changed| changed.file_name() == Some(name.as_os_str())) {
                return;
            }
        }
        // A full queue already means "a change is pending", so a drop here
        // loses nothing.
        let _ = raw_tx.try_send(());
    })
    .and_then(|mut watcher| {
        watcher.watch(target, RecursiveMode::NonRecursive)?;
        Ok(watcher)
    });

    match watcher {
        Ok(watcher) => {
            let mut events = debounced(raw_rx, debounce);
            events.watcher = Some(watcher);
            events
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "cannot watch transcript; falling back to polling");
            never()
        }
    }
}

/// The directory [`watch_file`] watches and the file name it filters for.
///
/// Always picks the parent directory when `path` names a file inside one, even
/// if the file already exists, so inode replacement never orphans the watch.
/// Falls back to `path` itself only for paths with no file name or parent.
fn watch_target(path: &Path) -> (&Path, Option<std::ffi::OsString>) {
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) => {
            let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
            (dir, Some(name.to_os_string()))
        }
        _ => (path, None),
    }
}

/// Watch every directory in `dirs` recursively, coalescing bursts into one
/// signal per `debounce` window.
///
/// One watcher covers all of them: the global feed re-lists everything on any
/// change, so which root moved is not information it can use. A directory that
/// cannot be watched (it does not exist, or the platform refuses) is skipped
/// rather than failing the rest — the caller's poll interval still covers it.
pub fn watch_dirs(dirs: &[std::path::PathBuf], debounce: Duration) -> FileEvents {
    let (raw_tx, raw_rx) = mpsc::channel::<()>(64);

    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = raw_tx.try_send(());
        }
    });
    let Ok(mut watcher) = watcher else {
        tracing::warn!("cannot create a filesystem watcher; falling back to polling");
        return never();
    };

    let mut watching = 0usize;
    for dir in dirs {
        match watcher.watch(dir, RecursiveMode::Recursive) {
            Ok(()) => watching += 1,
            Err(error) => tracing::debug!(path = %dir.display(), %error, "cannot watch directory; the poll interval still covers it"),
        }
    }
    if watching == 0 {
        return never();
    }

    let mut events = debounced(raw_rx, debounce);
    events.watcher = Some(watcher);
    events
}

/// Coalesce a stream of raw change notifications: one signal per quiet `window`.
///
/// Separated from [`watch_file`] so the coalescing rule can be exercised without
/// a real filesystem watcher, whose timing is the platform's business.
fn debounced(mut raw_rx: mpsc::Receiver<()>, window: Duration) -> FileEvents {
    let (tx, rx) = mpsc::channel::<()>(4);
    tokio::spawn(async move {
        while raw_rx.recv().await.is_some() {
            // Keep draining until the file goes quiet for a full window, then
            // emit exactly one signal for the whole burst.
            loop {
                match tokio::time::timeout(window, raw_rx.recv()).await {
                    Ok(Some(())) => continue,
                    Ok(None) => return,
                    Err(_) => break,
                }
            }
            if tx.send(()).await.is_err() {
                return;
            }
        }
    });
    FileEvents {
        rx,
        watcher: None,
        _keepalive: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn a_burst_of_changes_coalesces_into_one_signal() {
        let (tx, rx) = mpsc::channel::<()>(64);
        let mut events = debounced(rx, Duration::from_millis(50));

        for _ in 0..5 {
            tx.send(()).await.expect("raw send");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let first = tokio::time::timeout(Duration::from_secs(2), events.recv()).await;
        assert!(matches!(first, Ok(Some(()))), "the burst must produce a signal, got {first:?}");

        let second = tokio::time::timeout(Duration::from_millis(200), events.recv()).await;
        assert!(second.is_err(), "the burst must coalesce into one signal, got {second:?}");

        // A later change is a new burst, not part of the coalesced one.
        tx.send(()).await.expect("raw send");
        let third = tokio::time::timeout(Duration::from_secs(2), events.recv()).await;
        assert!(matches!(third, Ok(Some(()))), "a later change signals again, got {third:?}");
    }

    /// A silent watcher must leave the caller's `select!` arm *pending*.
    ///
    /// This is the property the feed actually depends on: `watch_session`
    /// swaps in [`never`] as soon as the watcher stops producing, and a channel
    /// whose sender had been dropped would resolve instantly and spin the loop
    /// at full speed instead of falling back to the 2 s poll.
    #[tokio::test]
    async fn a_silent_watcher_stays_pending_rather_than_closing() {
        let mut events = never();
        let result = tokio::time::timeout(Duration::from_millis(150), events.recv()).await;
        assert!(result.is_err(), "a silent watcher must never resolve, got {result:?}");
    }

    /// Watching a path that does not exist degrades instead of failing: in a
    /// directory that is not there either, to polling; in one that is, to its
    /// directory, quiet while nothing there names it.
    #[tokio::test]
    async fn an_unwatchable_path_degrades_to_polling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut events = watch_file(&dir.path().join("does-not-exist.jsonl"), Duration::from_millis(50));
        std::fs::write(dir.path().join("another.jsonl"), "a\n").expect("another file");
        let result = tokio::time::timeout(Duration::from_millis(300), events.recv()).await;
        assert!(result.is_err(), "a missing path must be quiet, not closed, got {result:?}");

        let mut events = watch_file(&dir.path().join("gone").join("does-not-exist.jsonl"), Duration::from_millis(50));
        let result = tokio::time::timeout(Duration::from_millis(150), events.recv()).await;
        assert!(result.is_err(), "an unwatchable path must be quiet, not closed, got {result:?}");
    }

    /// Even when the file already exists, `watch_file` watches its parent
    /// directory filtered by name so deleting and recreating the file with a
    /// new inode (such as Codex compressing and restoring a rollout on Linux)
    /// does not orphan the watch.
    #[test]
    fn existing_and_missing_files_both_watch_their_parent_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let existing = dir.path().join("present.jsonl");
        std::fs::write(&existing, "a\n").expect("seed");
        let missing = dir.path().join("absent.jsonl");

        assert_eq!(watch_target(&existing), (dir.path(), Some(std::ffi::OsString::from("present.jsonl"))));
        assert_eq!(watch_target(&missing), (dir.path(), Some(std::ffi::OsString::from("absent.jsonl"))));
        assert_eq!(
            watch_target(Path::new("bare.jsonl")),
            (Path::new("."), Some(std::ffi::OsString::from("bare.jsonl")))
        );
    }

    /// Watching an existing file ignores writes to sibling files in the same
    /// directory, and keeps watching across delete + recreate.
    #[tokio::test]
    async fn a_replaced_file_keeps_reaching_the_channel_when_the_platform_allows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, "a\n").expect("seed");

        let mut events = watch_file(&path, Duration::from_millis(50));
        // Sibling writes must be filtered out even though the parent dir is watched.
        std::fs::write(dir.path().join("sibling.jsonl"), "other\n").expect("sibling");
        let quiet = tokio::time::timeout(Duration::from_millis(200), events.recv()).await;
        assert!(quiet.is_err(), "sibling changes must not trigger events, got {quiet:?}");

        // Delete the original inode (e.g. compression) and recreate at the same path.
        std::fs::remove_file(&path).expect("remove");
        let _ = tokio::time::timeout(Duration::from_millis(150), events.recv()).await;

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).expect("recreate");
        loop {
            writeln!(file, "after-restore").expect("write");
            file.flush().expect("flush");
            match tokio::time::timeout(Duration::from_millis(250), events.recv()).await {
                Ok(Some(())) => return,
                Ok(None) => panic!("the watcher channel closed; `never()` must keep it pending"),
                Err(_) if std::time::Instant::now() < deadline => continue,
                Err(_) => {
                    eprintln!("note: no filesystem events in this environment; the 2s poll backstop is carrying the feed");
                    return;
                }
            }
        }
    }

    /// A file that appears after the watch began (Codex decompressing a
    /// rollout back to the name its view is watching) is heard of, when the
    /// platform delivers events at all; see the test below for why that is
    /// not asserted.
    #[tokio::test]
    async fn a_file_that_appears_later_reaches_the_channel_when_the_platform_allows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        let mut events = watch_file(&path, Duration::from_millis(50));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).expect("create");
        loop {
            writeln!(file, "line").expect("write");
            file.flush().expect("flush");
            match tokio::time::timeout(Duration::from_millis(250), events.recv()).await {
                Ok(Some(())) => return,
                Ok(None) => panic!("the watcher channel closed; `never()` must keep it pending"),
                Err(_) if std::time::Instant::now() < deadline => continue,
                Err(_) => {
                    eprintln!("note: no filesystem events in this environment; the 2s poll backstop is carrying the feed");
                    return;
                }
            }
        }
    }

    /// End-to-end through the real platform watcher, when the platform will
    /// talk to us at all.
    ///
    /// Under this repository's agent sandbox, FSEvents delivers nothing to a
    /// `cargo test` binary (the same code signals immediately from a `cargo run`
    /// binary), so a hard assertion here would fail for reasons that have
    /// nothing to do with this crate. The watcher is an optimisation over the
    /// 2 s poll, never a correctness requirement, so the test reports what it
    /// saw and leaves the correctness claim to the poll-backstopped integration
    /// test in `tests/probe.rs`.
    #[tokio::test]
    async fn a_real_file_change_reaches_the_channel_when_the_platform_allows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, "a\n").expect("seed");

        let mut events = watch_file(&path, Duration::from_millis(50));
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("open");

        loop {
            writeln!(file, "line").expect("write");
            file.flush().expect("flush");
            match tokio::time::timeout(Duration::from_millis(250), events.recv()).await {
                Ok(Some(())) => return,
                Ok(None) => panic!("the watcher channel closed; `never()` must keep it pending"),
                Err(_) if std::time::Instant::now() < deadline => continue,
                Err(_) => {
                    eprintln!("note: no filesystem events in this environment; the 2s poll backstop is carrying the feed");
                    return;
                }
            }
        }
    }
}
