//! Login sessions: rows in this instance's state database.
//!
//! Per-instance, unlike the account file next door in [`super::users`]. An
//! instance is a server, and a token is a claim on *a server* — so restarting
//! one instance, or stopping it entirely, does not have to invalidate the phone
//! that is logged in to another one on the same machine.
//!
//! **The server never stores a token.** What goes in the store is
//! `sha256(token)`, hex; the token itself exists only in the response to
//! `POST /api/auth/login` and in whatever the client does with it afterwards. A
//! stolen copy of the store therefore buys nothing: an attacker would have to
//! invert sha256 over a 256-bit random string to get a usable bearer token. It
//! also means the "list my sessions" route genuinely cannot leak one, rather
//! than merely being written not to.
//!
//! Expiry is absolute — seven days from the login, never extended by use. A
//! sliding window is the one thing that turns a leaked token into a permanent
//! one, because the attacker's own polling keeps it alive.
//!
//! # Where the sessions live
//!
//! In this instance's [`crate::db`], at `<instance>/state.db` — the database
//! [`crate::sessions::Paths::instance_db`] names.
//!
//! Expiry is the reason this store has a *column* and not just a payload. The
//! file it replaced had to be read whole and filtered in Rust to answer "which
//! of these are still alive", so every login, every revocation and every list
//! rewrote the whole thing to drop the dead ones. `expires_at` makes that
//! `DELETE FROM auth_sessions WHERE expires_at < ?`, and the index makes it a
//! range scan over the ones that are actually dead. It is stored in
//! **milliseconds**, matching the ISO stamps beside it in the payload rather
//! than SQLite's `unixepoch()` convention: a session minted with a zero TTL has
//! to be dead the instant it exists, and a second's rounding would keep it alive
//! for the rest of that second.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use rand::RngCore;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::db::{self, Db};
use crate::sessions::iso8601;

/// How many random bytes a token carries. 256 bits, base64url'd to 43
/// characters — long enough that the digest in the store is a one-way function
/// in practice as well as in principle.
const TOKEN_BYTES: usize = 32;

/// How long a `lastSeenAt` may be stale before it is written down.
///
/// The field exists so a person can recognise "this is the phone I lost"; it is
/// not an audit log. Writing it on every request would turn a read-only API call
/// into a write transaction — and, back when this was a file, into a `rename` of
/// something every other request also read.
const LAST_SEEN_FLUSH: Duration = Duration::from_secs(5 * 60);

/// One live login, as it is stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredSession {
    /// `sha256(token)`, hex. The token itself is never written down.
    pub token_sha256: String,
    /// Which account logged in.
    pub username: String,
    /// When, ISO-8601.
    pub created_at: String,
    /// When it stops working, ISO-8601. Absolute; never extended.
    pub expires_at: String,
    /// Last time a request carried this token, ISO-8601, to within
    /// [`LAST_SEEN_FLUSH`].
    pub last_seen_at: String,
    /// Whatever the client called itself when it logged in (its `User-Agent`),
    /// so the list is recognisable. Absent when it called itself nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A freshly minted login: the one moment the token exists in this process.
#[derive(Debug, Clone)]
pub struct NewSession {
    /// The bearer token, base64url, no padding.
    pub token: String,
    /// When it stops working, ISO-8601.
    pub expires_at: String,
}

/// The session store, owning its database handle and its lock.
///
/// `write_lock` outlived the mirror it was introduced beside, and what it
/// protects now is narrower and worth naming: [`SessionsStore::revoke_user`] and
/// [`SessionsStore::sweep`] read the live sessions, decide which to drop, and
/// delete them in a *second* transaction. "Log every other device out", which is
/// what a password change asks for, must not let a login slip into the gap
/// between those two — the device it minted would survive an eviction that was
/// meant to be total. Holding the lock across [`SessionsStore::issue`] as well
/// is what makes the ordering definite either way.
pub struct SessionsStore {
    db: Arc<Db>,
    ttl: Duration,
    write_lock: tokio::sync::Mutex<()>,
    /// When each token's `lastSeenAt` was last written. Keyed by digest, so it
    /// holds nothing secret and nothing that outlives the process.
    flushed: std::sync::Mutex<HashMap<String, SystemTime>>,
}

impl SessionsStore {
    /// A store over one instance's state database.
    ///
    /// `db` is [`crate::sessions::Paths::instance_db`] — `<instance>/state.db` —
    /// so a store built for an instance, as [`super::cli`] builds one per
    /// instance it finds, reaches that instance's sessions and nobody else's.
    pub fn new(db: PathBuf, ttl: Duration) -> Self {
        Self {
            db: db::at(db),
            ttl,
            write_lock: tokio::sync::Mutex::new(()),
            flushed: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Every session that has not expired, oldest first.
    ///
    /// `ORDER BY rowid` is the order they were minted in, which is what the
    /// list route hands back.
    fn live(&self, now: i64) -> Vec<StoredSession> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare("SELECT payload FROM auth_sessions WHERE expires_at > ?1 ORDER BY rowid")?;
                let rows = statement.query_map((now,), |row| row.get::<_, String>(0))?;
                // A payload that will not parse is a session nobody can be
                // logged in by; it is dropped from the answer rather than
                // failing the request, and the expiry sweep will collect it.
                Ok(rows
                    .filter_map(Result::ok)
                    .filter_map(|raw| serde_json::from_str::<StoredSession>(&raw).ok())
                    .collect::<Vec<_>>())
            })
            .unwrap_or_else(|error| {
                tracing::error!(database = %self.db.path().display(), %error, "cannot read the login sessions out of the state database");
                Vec::new()
            })
    }

    /// Mint a session for `username` and return the one copy of its token.
    pub async fn issue(&self, username: &str, label: Option<String>) -> std::io::Result<NewSession> {
        let token = new_token();
        let digest = digest_of(&token);
        let now = SystemTime::now();
        let created_at = iso8601(now);
        let expires_at = iso8601(now + self.ttl);
        let session = StoredSession {
            token_sha256: digest,
            username: username.to_string(),
            created_at: created_at.clone(),
            expires_at: expires_at.clone(),
            last_seen_at: created_at,
            label,
        };

        let _guard = self.write_lock.lock().await;
        self.db
            .write(|tx| {
                prune(tx, epoch_millis(now))?;
                tx.execute(
                    "INSERT INTO auth_sessions (digest, expires_at, payload) VALUES (?1, ?2, ?3)",
                    (
                        &session.token_sha256,
                        epoch_millis(now + self.ttl),
                        serde_json::to_string(&session).unwrap_or_default(),
                    ),
                )?;
                Ok(())
            })
            .map_err(store_error)?;
        Ok(NewSession { token, expires_at })
    }

    /// Whose session this token is, or `None` for an unknown or expired one.
    ///
    /// The digest lookup is still a linear scan with a constant-time comparison
    /// rather than the primary-key hit the table now makes available, and the
    /// reason it is worth keeping is the one it always was: an index probe leaks
    /// through timing exactly what a `==` on the digest leaks, comparing in
    /// constant time is free to avoid it, and the scan is bounded by how many
    /// devices one person is logged in from. The `WHERE` clause does the work
    /// the index is actually for — not reading the dead rows at all.
    ///
    /// Nothing is deleted here. This runs on every authenticated request, and
    /// turning a read into a write transaction per request is a price the
    /// expiry sweep on the write paths already saves.
    pub async fn resolve(&self, token: &str) -> Option<String> {
        let digest = digest_of(token);
        let found = self
            .live(now_millis())
            .into_iter()
            .find(|session| session.token_sha256.as_bytes().ct_eq(digest.as_bytes()).into())?;
        let username = found.username;
        self.touch(&digest).await;
        Some(username)
    }

    /// Record that a token was used, at most once per [`LAST_SEEN_FLUSH`].
    async fn touch(&self, digest: &str) {
        let now = SystemTime::now();
        {
            let mut flushed = self.flushed.lock().expect("auth sessions flush map poisoned");
            if let Some(last) = flushed.get(digest) {
                if now.duration_since(*last).unwrap_or_default() < LAST_SEEN_FLUSH {
                    return;
                }
            }
            flushed.insert(digest.to_string(), now);
        }
        let _guard = self.write_lock.lock().await;
        let stamp = iso8601(now);
        let millis = epoch_millis(now);
        let outcome = self.db.write(|tx| {
            prune(tx, millis)?;
            let raw: Option<String> = tx
                .query_row("SELECT payload FROM auth_sessions WHERE digest = ?1", (digest,), |row| row.get(0))
                .optional()?;
            let Some(mut session) = raw.and_then(|raw| serde_json::from_str::<StoredSession>(&raw).ok()) else {
                return Ok(());
            };
            session.last_seen_at = stamp;
            tx.execute(
                "UPDATE auth_sessions SET payload = ?2 WHERE digest = ?1",
                (digest, serde_json::to_string(&session).unwrap_or_default()),
            )?;
            Ok(())
        });
        if let Err(error) = outcome {
            tracing::warn!(%error, "could not record a session's last use");
        }
    }

    /// The account a live session (by its token's digest) is for, if it is
    /// still live — signed out, revoked or expired: `None`. Read only: the
    /// local gateway asks this of the login behind a socket it keeps open.
    pub fn alive(&self, digest: &str) -> Option<String> {
        self.live(now_millis())
            .into_iter()
            .find(|session| session.token_sha256.as_bytes().ct_eq(digest.as_bytes()).into())
            .map(|session| session.username)
    }

    /// Every live session, oldest first, with the expired ones dropped.
    ///
    /// The dropping is now a `DELETE` rather than a filter over a copy of the
    /// file, so the dead rows stop existing instead of being skipped by every
    /// reader for ever.
    pub async fn list(&self) -> Vec<StoredSession> {
        let _guard = self.write_lock.lock().await;
        let now = now_millis();
        self.sweep_expired(now);
        self.live(now)
    }

    /// Drop one session by its id (which is its digest). `true` if it was there.
    pub async fn revoke(&self, digest: &str) -> bool {
        let _guard = self.write_lock.lock().await;
        let removed = self.db.write(|tx| {
            prune(tx, now_millis())?;
            Ok(tx.execute("DELETE FROM auth_sessions WHERE digest = ?1", (digest,))? > 0)
        });
        let removed = match removed {
            Ok(removed) => removed,
            Err(error) => {
                tracing::warn!(%error, "could not revoke a session");
                return false;
            }
        };
        self.flushed.lock().expect("auth sessions flush map poisoned").remove(digest);
        removed
    }

    /// Drop every session of `username`, optionally sparing one digest.
    ///
    /// The sparing is what makes "change my password" usable from a phone: the
    /// point is to evict every *other* device, not to log the person out of the
    /// one they are holding.
    pub async fn revoke_user(&self, username: &str, except: Option<&str>) -> usize {
        let _guard = self.write_lock.lock().await;
        // The name is inside the payload rather than in a column of its own, so
        // the rows are read and decided on here. Bounded by the sessions one
        // machine has, which is the number of devices one household owns.
        let doomed: Vec<String> = self
            .live(now_millis())
            .into_iter()
            .filter(|session| session.username == username && Some(session.token_sha256.as_str()) != except)
            .map(|session| session.token_sha256)
            .collect();
        let (removed, _) = match self.delete_all(&doomed) {
            Ok(counts) => counts,
            Err(error) => {
                tracing::warn!(%error, "could not revoke a user's sessions");
                return 0;
            }
        };
        self.flushed.lock().expect("auth sessions flush map poisoned").clear();
        removed
    }

    /// Drop every session whose user is no longer on file, plus every expired
    /// one. Called at boot; the token check makes the same decision per request,
    /// so this is tidying rather than enforcement.
    pub async fn sweep(&self, known_users: &[String]) {
        let _guard = self.write_lock.lock().await;
        let now = now_millis();
        let doomed: Vec<String> = self
            .live(now)
            .into_iter()
            .filter(|session| !known_users.contains(&session.username))
            .map(|session| session.token_sha256)
            .collect();
        if let Err(error) = self.delete_all(&doomed) {
            tracing::warn!(%error, "could not sweep stale login sessions");
        }
    }

    /// Delete a known set of digests, and the expired ones with them.
    ///
    /// Answers with both counts because [`SessionsStore::revoke_user`] reports
    /// the first of them to its caller; the second is what the expiry sweep
    /// collected on the way past.
    fn delete_all(&self, digests: &[String]) -> rusqlite::Result<(usize, usize)> {
        self.db.write(|tx| {
            let pruned = prune(tx, now_millis())?;
            let mut removed = 0;
            for digest in digests {
                removed += tx.execute("DELETE FROM auth_sessions WHERE digest = ?1", (digest,))?;
            }
            Ok((removed, pruned))
        })
    }

    /// The expiry sweep on its own, for the read paths that have nothing else
    /// to change. Returns how many rows it collected.
    fn sweep_expired(&self, now: i64) -> usize {
        self.db.write(|tx| prune(tx, now)).unwrap_or_else(|error| {
            tracing::warn!(%error, "could not drop the expired login sessions");
            0
        })
    }
}

/// Drop everything that has already expired.
///
/// The one query the `expires_at` column exists for. `<` rather than `<=`
/// because a session is alive up to and including the instant it names; the
/// readers use `expires_at > now`, which is the same boundary from the other
/// side.
fn prune(tx: &rusqlite::Transaction<'_>, now: i64) -> rusqlite::Result<usize> {
    tx.execute("DELETE FROM auth_sessions WHERE expires_at < ?1", (now,))
}

fn store_error(error: rusqlite::Error) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

/// Milliseconds since the epoch, the unit `expires_at` is kept in.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH).map(|since| since.as_millis() as i64).unwrap_or_default()
}

fn now_millis() -> i64 {
    epoch_millis(SystemTime::now())
}

/// A fresh token: 32 bytes from the OS, base64url without padding.
fn new_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `sha256(token)` as lower-case hex — the only form of a token this server keeps.
pub fn digest_of(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(ttl: Duration) -> (tempfile::TempDir, SessionsStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionsStore::new(dir.path().join("state.db"), ttl);
        (dir, store)
    }

    const WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

    #[tokio::test]
    async fn a_token_resolves_to_its_user_and_a_stranger_does_not() {
        let (_dir, store) = store(WEEK);
        let issued = store.issue("ada", None).await.expect("issue");
        assert_eq!(store.resolve(&issued.token).await.as_deref(), Some("ada"));
        assert_eq!(store.resolve("not-a-token").await, None);
    }

    #[tokio::test]
    async fn the_token_is_43_url_safe_characters_and_never_repeats() {
        let (_dir, store) = store(WEEK);
        let first = store.issue("ada", None).await.expect("issue").token;
        let second = store.issue("ada", None).await.expect("issue").token;
        assert_eq!(first.len(), 43, "32 bytes, base64url, unpadded");
        assert!(first.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(first, second);
    }

    /// The stored row holds the digest and the label, never the token.
    #[tokio::test]
    async fn what_is_stored_is_the_digest_and_never_the_token() {
        let (_dir, store) = store(WEEK);
        let issued = store.issue("ada", Some("curl/8".into())).await.expect("issue");

        let payload: String = store
            .db
            .read(|conn| conn.query_row("SELECT payload FROM auth_sessions", [], |row| row.get(0)))
            .expect("the row is there");
        assert!(!payload.contains(&issued.token), "the token must exist nowhere but the response");
        assert!(payload.contains(&digest_of(&issued.token)));
        assert!(payload.contains("curl/8"));
    }

    /// Absolute expiry, and no sliding window: a token that has been used every
    /// second since it was minted still dies on schedule.
    #[tokio::test]
    async fn a_session_expires_on_its_own_clock() {
        let (_dir, store) = store(Duration::from_millis(0));
        let issued = store.issue("ada", None).await.expect("issue");
        assert_eq!(store.resolve(&issued.token).await, None, "already past its expiry");
    }

    #[tokio::test]
    async fn an_expired_session_is_swept_out_by_the_next_write() {
        let (dir, store) = store(Duration::from_millis(0));
        store.issue("ada", None).await.expect("issue");
        assert_eq!(store.list().await.len(), 0, "the list never shows a dead session");
        // The sweep removes rows strictly before now; cross the expiry millisecond.
        tokio::time::sleep(Duration::from_millis(2)).await;
        let live = SessionsStore::new(dir.path().join("state.db"), WEEK);
        live.issue("ada", None).await.expect("issue");
        assert_eq!(rows(&live), 1, "the write that added one dropped the dead one");
    }

    #[tokio::test]
    async fn revoking_one_leaves_the_others() {
        let (_dir, store) = store(WEEK);
        let first = store.issue("ada", None).await.expect("issue");
        let second = store.issue("ada", None).await.expect("issue");
        assert!(store.revoke(&digest_of(&first.token)).await);
        assert_eq!(store.resolve(&first.token).await, None);
        assert_eq!(store.resolve(&second.token).await.as_deref(), Some("ada"));
        assert!(!store.revoke(&digest_of(&first.token)).await, "twice is not an error, just a no-op");
    }

    #[tokio::test]
    async fn revoking_a_user_can_spare_the_device_asking() {
        let (_dir, store) = store(WEEK);
        let phone = store.issue("ada", None).await.expect("issue");
        let laptop = store.issue("ada", None).await.expect("issue");
        let other = store.issue("grace", None).await.expect("issue");

        assert_eq!(store.revoke_user("ada", Some(&digest_of(&phone.token))).await, 1);
        assert_eq!(store.resolve(&phone.token).await.as_deref(), Some("ada"), "the one that asked survives");
        assert_eq!(store.resolve(&laptop.token).await, None);
        assert_eq!(store.resolve(&other.token).await.as_deref(), Some("grace"), "and so does everyone else");
    }

    #[tokio::test]
    async fn a_session_whose_user_is_gone_is_swept() {
        let (_dir, store) = store(WEEK);
        let ada = store.issue("ada", None).await.expect("issue");
        let grace = store.issue("grace", None).await.expect("issue");
        store.sweep(&["grace".to_string()]).await;
        assert_eq!(store.resolve(&ada.token).await, None);
        assert_eq!(store.resolve(&grace.token).await.as_deref(), Some("grace"));
    }

    /// The stamp is only written every few minutes; what must always be true is
    /// that it exists and that a resolve does not write anything down every
    /// time. An authenticated request is the commonest thing this server does,
    /// and turning each one into a write transaction is the cost the flush
    /// window exists to refuse.
    #[tokio::test]
    async fn last_seen_is_not_rewritten_on_every_request() {
        let (_dir, store) = store(WEEK);
        let issued = store.issue("ada", None).await.expect("issue");
        let changes = || -> i64 {
            store
                .db
                .read(|conn| conn.query_row("SELECT total_changes()", [], |row| row.get(0)))
                .expect("total_changes")
        };
        // The first use of a token is always written down: nothing has recorded
        // a flush for it yet, and a `lastSeenAt` still saying "when it was
        // minted" is the one value that would be wrong rather than merely stale.
        assert!(store.resolve(&issued.token).await.is_some());
        let before = changes();
        for _ in 0..5 {
            assert!(store.resolve(&issued.token).await.is_some());
        }
        assert_eq!(changes(), before, "and every use inside the flush window after it changes no row");
        assert_eq!(store.list().await.len(), 1);
        assert!(!store.list().await[0].last_seen_at.is_empty(), "the stamp is there all the same");
    }

    /// How many rows there actually are, expired or not.
    fn rows(store: &SessionsStore) -> i64 {
        store
            .db
            .read(|conn| conn.query_row("SELECT count(*) FROM auth_sessions", [], |row| row.get(0)))
            .expect("count")
    }

    /// What the `expires_at` column is for: the dead row stops existing, rather
    /// than being skipped by every reader from now until the end of time.
    #[tokio::test]
    async fn an_expired_session_is_deleted_rather_than_merely_filtered_out() {
        let (_dir, store) = store(Duration::from_millis(0));
        store.issue("ada", None).await.expect("issue");
        assert_eq!(rows(&store), 1, "it is a row to begin with");

        // A session is alive up to and including the instant it names, so the
        // sweep only collects it once the clock is past that instant. Waiting
        // for the millisecond to turn over is the difference between testing
        // the boundary and testing how fast this machine is.
        tokio::time::sleep(Duration::from_millis(2)).await;
        assert!(store.list().await.is_empty(), "the list never shows a dead session");
        assert_eq!(rows(&store), 0, "and the read that skipped it took it out of the table");
    }

    /// A live session is not collateral damage of the sweep.
    #[tokio::test]
    async fn the_sweep_takes_the_expired_and_leaves_the_rest() {
        let (dir, expiring) = store(Duration::from_millis(0));
        expiring.issue("ada", None).await.expect("issue");
        tokio::time::sleep(Duration::from_millis(2)).await;
        let live = SessionsStore::new(dir.path().join("state.db"), WEEK);
        let kept = live.issue("grace", None).await.expect("issue");

        assert_eq!(live.list().await.len(), 1);
        assert_eq!(rows(&live), 1, "one row left, and it is the one that has not expired");
        assert_eq!(live.resolve(&kept.token).await.as_deref(), Some("grace"));
    }

    #[test]
    fn expiry_ordering_is_string_ordering() {
        let now = SystemTime::now();
        let earlier = iso8601(now);
        let later = iso8601(now + Duration::from_secs(1));
        assert!(later > earlier, "iso8601 is fixed-width UTC, so lexical order is chronological");
    }
}
