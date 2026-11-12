//! Which of the machine's session directories are no longer there.
//!
//! A session's transcript outlives the directory it ran in: a worktree is
//! removed once its branch merges, and a harness that runs Claude Code in
//! temporary repositories deletes them when it is done. Their sessions stay
//! readable, but nothing can be started in them again, so the new-session
//! picker leaves them out (`GET /api/sessions/gone-cwds`).
//!
//! The answer takes a look at every directory a session ever ran in, so two
//! things keep it cheap however often it is asked:
//!
//! * **one answer per half minute.** An answer is given again for
//!   [`ANSWER_TTL`], and whoever asks while one is being made waits for it
//!   rather than making another.
//! * **one look per directory at a time.** A `stat` runs on a blocking thread,
//!   and on a stalled network mount it may not come back: the timeout stops
//!   the waiting, not the thread. A directory whose last look has not come
//!   back is not looked at again until it has, and counts as there meanwhile,
//!   so a picker opened again and again cannot pile threads up behind a mount.
//!
//! Only a definite answer counts as gone (not found, not a directory). A look
//! that is refused or does not come back in time leaves the directory listed.
//! [`crate::repo::repo_key`] never forgets a directory it has seen, which is
//! why this does not go through it.

use std::collections::{BTreeSet, HashSet};
use std::future::Future;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

/// How long an answer is given again before the directories are looked at anew.
const ANSWER_TTL: Duration = Duration::from_secs(30);

/// How long one directory's look is waited for before it counts as there.
const LOOK_TIMEOUT: Duration = Duration::from_secs(1);

/// How many directories are looked at at once.
const LOOK_CONCURRENCY: usize = 16;

/// Whether a path is a directory, as the disk says now. Blocking.
type IsDir = Arc<dyn Fn(&Path) -> io::Result<bool> + Send + Sync>;

/// The directories still being looked at.
type Looking = Arc<Mutex<HashSet<String>>>;

/// One server's answer to which of its session directories are gone.
pub struct GoneDirs {
    /// The last answer and when it was made. Held while a new one is made, so
    /// whoever asks meanwhile waits for it and takes it.
    answer: tokio::sync::Mutex<Option<(Instant, Vec<String>)>>,
    /// The directories whose look has not come back yet.
    looking: Looking,
    ttl: Duration,
    timeout: Duration,
    is_dir: IsDir,
}

impl Default for GoneDirs {
    fn default() -> Self {
        Self::new(
            ANSWER_TTL,
            LOOK_TIMEOUT,
            Arc::new(|path: &Path| std::fs::metadata(path).map(|meta| meta.is_dir())),
        )
    }
}

impl GoneDirs {
    fn new(ttl: Duration, timeout: Duration, is_dir: IsDir) -> Self {
        Self {
            answer: tokio::sync::Mutex::new(None),
            looking: Arc::default(),
            ttl,
            timeout,
            is_dir,
        }
    }

    /// Which of the directories `listed` gives are no longer directories,
    /// sorted. `listed` is called only when the last answer is too old to give
    /// again.
    pub async fn answer<F, Fut>(&self, listed: F) -> Vec<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = BTreeSet<String>>,
    {
        let mut kept = self.answer.lock().await;
        if let Some((_, gone)) = kept.as_ref().filter(|(at, _)| at.elapsed() < self.ttl) {
            return gone.clone();
        }
        let mut gone: Vec<String> = futures_util::stream::iter(listed().await)
            .map(|cwd| self.gone(cwd))
            .buffer_unordered(LOOK_CONCURRENCY)
            .filter_map(|cwd| async move { cwd })
            .collect()
            .await;
        gone.sort();
        *kept = Some((Instant::now(), gone.clone()));
        gone
    }

    /// `cwd`, when it is definitely not a directory now.
    async fn gone(&self, cwd: String) -> Option<String> {
        let first = self.looking.lock().expect("gone-dirs looking set poisoned").insert(cwd.clone());
        if !first {
            // Its last look has not come back: there, as far as anyone knows.
            return None;
        }
        let mark = LookMark {
            looking: Arc::clone(&self.looking),
            cwd: cwd.clone(),
        };
        let is_dir = Arc::clone(&self.is_dir);
        // The mark goes with the look, and comes off when the look ends, however
        // long after the timeout below that is.
        let look = tokio::task::spawn_blocking(move || {
            let answer = is_dir(Path::new(&mark.cwd));
            drop(mark);
            answer
        });
        let gone = match tokio::time::timeout(self.timeout, look).await {
            Ok(Ok(Ok(is_dir))) => !is_dir,
            Ok(Ok(Err(error))) => matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory),
            Ok(Err(_)) | Err(_) => false,
        };
        gone.then_some(cwd)
    }
}

/// A directory's place in the looking set, taken off when its look ends,
/// panicked or never started.
struct LookMark {
    looking: Looking,
    cwd: String,
}

impl Drop for LookMark {
    fn drop(&mut self) {
        self.looking.lock().expect("gone-dirs looking set poisoned").remove(&self.cwd);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    use super::*;

    fn dirs(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[tokio::test]
    async fn a_directory_whose_look_has_not_come_back_counts_as_there_and_is_not_looked_at_again() {
        // `/stuck`'s first look hangs, as a stat on a stalled mount does, until
        // it is released; by its next look the directory is gone.
        let (release, hung) = std::sync::mpsc::channel::<()>();
        let hung = Mutex::new(hung);
        let looks = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&looks);
        let gone = GoneDirs::new(
            Duration::ZERO,
            Duration::from_millis(250),
            Arc::new(move |path: &Path| match path.to_str() {
                Some("/stuck") if counted.fetch_add(1, SeqCst) == 0 => {
                    let _ = hung.lock().expect("hung").recv();
                    Ok(true)
                }
                Some("/stuck" | "/removed") => Err(io::ErrorKind::NotFound.into()),
                Some("/file") => Ok(false),
                _ => Ok(true),
            }),
        );
        let listed = || async { dirs(&["/here", "/stuck", "/removed", "/file"]) };

        assert_eq!(
            gone.answer(listed).await,
            ["/file", "/removed"],
            "a look that does not come back counts as there"
        );
        assert_eq!(gone.answer(listed).await, ["/file", "/removed"]);
        assert_eq!(looks.load(SeqCst), 1, "not looked at again while its look hangs");

        release.send(()).expect("the hung look is waiting");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !gone.looking.lock().expect("looking").is_empty() {
            assert!(Instant::now() < deadline, "the released look came back");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            gone.answer(listed).await,
            ["/file", "/removed", "/stuck"],
            "looked at again once it came back"
        );
        assert_eq!(looks.load(SeqCst), 2);
    }

    #[tokio::test]
    async fn an_answer_is_given_again_for_a_while_and_whoever_asks_meanwhile_waits_for_it() {
        let lists = AtomicUsize::new(0);
        let counted = &lists;
        let listed = move || async move {
            counted.fetch_add(1, SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            dirs(&["/removed"])
        };
        let removed: IsDir = Arc::new(|_: &Path| Err(io::ErrorKind::NotFound.into()));

        let kept = GoneDirs::new(Duration::from_secs(3600), Duration::from_secs(5), Arc::clone(&removed));
        let (first, second) = tokio::join!(kept.answer(listed), kept.answer(listed));
        assert_eq!(first, ["/removed"]);
        assert_eq!(second, ["/removed"]);
        assert_eq!(lists.load(SeqCst), 1, "the second caller took the answer the first was making");
        assert_eq!(kept.answer(listed).await, ["/removed"]);
        assert_eq!(lists.load(SeqCst), 1, "and so does the next, while the answer is fresh");

        let stale = GoneDirs::new(Duration::ZERO, Duration::from_secs(5), removed);
        stale.answer(listed).await;
        stale.answer(listed).await;
        assert_eq!(lists.load(SeqCst), 3, "an answer past its time is made again");
    }
}
