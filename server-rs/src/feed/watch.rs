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

// `::notify` rather than `notify`: this crate has a module of that name
// (`crate::notify`, the agent-initiated notifications), and an unqualified path
// would be ambiguous between the two.
use ::notify::{RecommendedWatcher, RecursiveMode, Watcher};
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
pub fn watch_file(path: &Path, debounce: Duration) -> FileEvents {
    let (raw_tx, raw_rx) = mpsc::channel::<()>(64);

    let watcher = ::notify::recommended_watcher(move |event: ::notify::Result<::notify::Event>| {
        if event.is_ok() {
            // A full queue already means "a change is pending", so a drop here
            // loses nothing.
            let _ = raw_tx.try_send(());
        }
    })
    .and_then(|mut watcher| {
        watcher.watch(path, RecursiveMode::NonRecursive)?;
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

/// Watch every directory in `dirs` recursively, coalescing bursts into one
/// signal per `debounce` window.
///
/// One watcher covers all of them: the global feed re-lists everything on any
/// change, so which root moved is not information it can use. A directory that
/// cannot be watched (it does not exist, or the platform refuses) is skipped
/// rather than failing the rest — the caller's poll interval still covers it.
pub fn watch_dirs(dirs: &[std::path::PathBuf], debounce: Duration) -> FileEvents {
    let (raw_tx, raw_rx) = mpsc::channel::<()>(64);

    let watcher = ::notify::recommended_watcher(move |event: ::notify::Result<::notify::Event>| {
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

    /// Watching a path that does not exist degrades instead of failing.
    #[tokio::test]
    async fn an_unwatchable_path_degrades_to_polling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut events = watch_file(&dir.path().join("does-not-exist.jsonl"), Duration::from_millis(50));
        let result = tokio::time::timeout(Duration::from_millis(150), events.recv()).await;
        assert!(result.is_err(), "an unwatchable path must be quiet, not closed, got {result:?}");
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
