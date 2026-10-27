//! `read_at`: when the user last looked at a session.
//!
//! One row per session in the machine-wide state database ([`crate::db`]), under
//! [`scope::SEEN`] and keyed `"<source>/<id>"` — the same key the global feed
//! diffs its rows by (`crate::feed::global`) and the same one a push
//! notification's `tag` carries, so the three never have to be translated into
//! each other.
//!
//! `seen.json` was a mirror for one version, so that a [`crate::supervisor`]
//! demotion onto a `.prev` binary that had never heard of this database would
//! still find its marks; the rollback floor has since risen to a build that
//! reads the database, so nothing reads the file any more.
//!
//! It is deliberately *not* a subsystem with its own state struct. There is
//! nothing to cache: the store is a few hundred bytes, every reader wants the
//! newest value, and a write-through cache is one more thing that can disagree
//! with what was committed. The same reasoning [`crate::drive::titles`] gives
//! for going straight to the store applies here, only more so — this one is
//! written on every tab focus.
//!
//! Two properties survive the move, and one of them changes shape:
//!
//! * **Bounded.** A client marks anything it is shown as seen, and sessions are
//!   created far faster than they are deleted, so an unbounded map grows for the
//!   life of the install. [`MAX_ENTRIES`] still caps it and the oldest
//!   timestamps still go first — which is the right eviction, because an entry
//!   only matters while its session is recent enough to still be in the list.
//!   *Writing* no longer needs a cap: a row is an upsert, not a rewrite of every
//!   mark ever taken, so a hundred thousand of them would cost the
//!   hundred-thousand-and-first write nothing. Reading does. [`read`] hands back
//!   the whole scope and the global feed asks for it on every diff, so the cap
//!   is what keeps that read a short one for the life of the install.
//! * **Never half-written.** A mark is one committed row, so there is no state
//!   in which half of it landed. That used to be true of the table and only
//!   nearly true of the file beside it, which is one more reason the file is
//!   gone.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::db::{scope, Db};
use crate::sessions::{iso8601, Paths, Source};

/// How many sessions are remembered. Well past what any one list shows, and
/// small enough that the scope stays a single short read.
pub const MAX_ENTRIES: usize = 500;

/// The key both this store and a notification's `tag` use for a session.
///
/// Deliberately server-free, which is why it goes through
/// [`crate::sessions::reference::local_key`] rather than
/// [`crate::sessions::SessionRef::scoped_key`]: a session marked read is read
/// on every server on this machine, the `tag` two servers push under has to
/// collapse to one notification, and `crate::notify::NotifyCaller::parse` reads
/// this key back out of tokens minted by processes that are still running.
pub fn key_of(source: Source, id: &str) -> String {
    crate::sessions::reference::local_key(source, id)
}

/// Serializes the *trim*, which is the one read-modify-write left here.
///
/// It does not guard the marks. Each one is a single upserted row and two of
/// them cannot lose each other, which is the bug this lock used to be the only
/// defence against. Past [`MAX_ENTRIES`], though, eviction reads the whole scope
/// and writes back what survives, and two of those interleaved would throw away
/// a mark that had already been committed between them.
///
/// Process-global rather than a field on a store struct because the store *is*
/// the path: any number of `Paths` values in one process (two servers in one
/// test binary) name different databases, and taking one lock across all of
/// them costs a few microseconds on a route that runs at human speed.
static WRITE_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The machine-wide database the marks live in.
fn store(paths: &Paths) -> Arc<Db> {
    crate::db::at(paths.config_db())
}

/// Every remembered `read_at`. A store with nothing in it reads as empty, which
/// marks everything unread — the safe direction.
pub async fn read(paths: &Paths) -> BTreeMap<String, String> {
    store(paths).kv_scope(scope::SEEN)
}

/// Drop the oldest entries until at most [`MAX_ENTRIES`] remain.
///
/// Oldest by the timestamp itself rather than by insertion order: the map has no
/// order of its own, and the timestamp is the thing that decides whether an
/// entry is still worth keeping.
fn evict(entries: &mut BTreeMap<String, String>) {
    if entries.len() <= MAX_ENTRIES {
        return;
    }
    let mut by_age: Vec<(&String, &String)> = entries.iter().collect();
    // Ties broken by key so eviction is deterministic; ISO-8601 in one timezone
    // compares lexicographically, which is what the session list already sorts on.
    by_age.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)));
    let doomed: Vec<String> = by_age.iter().take(entries.len() - MAX_ENTRIES).map(|(key, _)| (*key).clone()).collect();
    for key in doomed {
        entries.remove(&key);
    }
}

/// Record that the user has now seen this session, as of `at`, and say what
/// the mark was before — `None` for a session never marked (or evicted).
///
/// The mark it replaces is the one thing a client arriving at a session cannot
/// learn any other way: its own report is what moves it, and anything it read
/// beforehand — a feed row, say — may be stale. Read just before the write, not
/// in one transaction with it: two marks racing on one session both answer
/// with what was there before either, which is still a mark from before the
/// visit that sent them.
///
/// Still `io::Result` because every caller's signature is built on it. Nothing
/// in here does I/O that can fail any more — the mark is a committed row and
/// there is no file to write beside it — so the result is unconditionally `Ok`.
pub async fn mark_at(paths: &Paths, source: Source, id: &str, at: String) -> std::io::Result<Option<String>> {
    let db = store(paths);
    let key = key_of(source, id);
    let previous = db.kv_get(scope::SEEN, &key);
    // One row, and no lock over it. Two marks in flight — from two tabs, or
    // from the two servers sharing this directory — cannot lose each other,
    // which is the whole reason this store moved.
    db.kv_set(scope::SEEN, &key, &at);

    // The trim is the exception, and the only thing [`WRITE_LOCK`] is still for:
    // it reads the whole scope and writes back what survives.
    let _guard = WRITE_LOCK.lock().await;
    let mut entries = db.kv_scope(scope::SEEN);
    if entries.len() > MAX_ENTRIES {
        // Trimming is the rare path rather than part of every write, and it is
        // the one change here that touches the whole scope — so it goes through
        // the transaction that replaces it all at once.
        evict(&mut entries);
        db.kv_replace_scope(scope::SEEN, entries.iter().map(|(key, at)| (key.as_str(), at.as_str())));
    }
    Ok(previous)
}

/// [`mark_at`] with the current time.
pub async fn mark(paths: &Paths, source: Source, id: &str) -> std::io::Result<Option<String>> {
    mark_at(paths, source, id, iso8601(std::time::SystemTime::now())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        (dir, paths)
    }

    #[tokio::test]
    async fn a_mark_round_trips_under_the_shared_key() {
        let (_dir, paths) = paths();
        assert!(read(&paths).await.is_empty(), "a missing file is an empty map");

        mark_at(&paths, Source::Claude, "abc", "2026-01-01T00:00:00.000Z".into())
            .await
            .expect("mark");
        mark_at(&paths, Source::Codex, "abc", "2026-01-02T00:00:00.000Z".into())
            .await
            .expect("mark");

        let entries = read(&paths).await;
        assert_eq!(entries.get("claude/abc").map(String::as_str), Some("2026-01-01T00:00:00.000Z"));
        assert_eq!(
            entries.get("codex/abc").map(String::as_str),
            Some("2026-01-02T00:00:00.000Z"),
            "the same id under two CLIs is two sessions"
        );
    }

    #[tokio::test]
    async fn marking_again_moves_the_clock_forward() {
        let (_dir, paths) = paths();
        let first = mark_at(&paths, Source::Claude, "abc", "2026-01-01T00:00:00.000Z".into())
            .await
            .expect("mark");
        assert_eq!(first, None, "never marked before");
        let second = mark_at(&paths, Source::Claude, "abc", "2026-01-05T00:00:00.000Z".into())
            .await
            .expect("mark");
        assert_eq!(second.as_deref(), Some("2026-01-01T00:00:00.000Z"), "the mark it replaced");
        assert_eq!(read(&paths).await.get("claude/abc").map(String::as_str), Some("2026-01-05T00:00:00.000Z"));
        assert_eq!(read(&paths).await.len(), 1, "a re-mark replaces, it does not accumulate");
    }

    /// A client marks everything it is shown, and sessions outlive the interest
    /// in them. Without a cap the file grows for the life of the install.
    #[test]
    fn the_oldest_entries_are_evicted_past_the_cap() {
        // Written in one go rather than through `mark_at` MAX_ENTRIES+2 times:
        // this is about the eviction rule, not about the write path.
        let mut entries: BTreeMap<String, String> = (0..MAX_ENTRIES + 2)
            .map(|n| (format!("claude/{n:04}"), format!("2026-01-01T00:00:{:02}.000Z", n % 60)))
            .collect();
        // Give two entries an unambiguously ancient clock so the assertion does
        // not depend on how the modulo above happened to land.
        entries.insert("claude/ancient-a".into(), "1999-01-01T00:00:00.000Z".into());
        entries.insert("claude/ancient-b".into(), "1999-01-02T00:00:00.000Z".into());
        let before = entries.len();

        evict(&mut entries);

        assert_eq!(entries.len(), MAX_ENTRIES, "evicted down to the cap, not below it");
        assert!(before > MAX_ENTRIES, "the fixture actually overflowed");
        assert!(!entries.contains_key("claude/ancient-a"), "the oldest goes first");
        assert!(!entries.contains_key("claude/ancient-b"));
    }

    /// The point of the move: a mark outlives the process that took it, and
    /// outlives the connection it was written through.
    #[tokio::test]
    async fn a_mark_is_still_there_through_a_fresh_connection() {
        let (_dir, paths) = paths();
        mark_at(&paths, Source::Claude, "abc", "2026-01-01T00:00:00.000Z".into())
            .await
            .expect("mark");

        let reopened = crate::db::Db::open(paths.config_db());
        assert_eq!(
            reopened.kv_scope(scope::SEEN).get("claude/abc").map(String::as_str),
            Some("2026-01-01T00:00:00.000Z"),
            "the row is committed, not sitting in a cache that dies with the connection"
        );
    }
}
