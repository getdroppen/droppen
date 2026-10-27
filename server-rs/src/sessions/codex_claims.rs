//! Which instance made a codex thread, written where every instance can read it.
//!
//! A claude session made under an account lands in the Claude root that instance
//! owns, so a sibling server does not merely refuse to drive it — it never sees
//! it at all. Codex has one `~/.codex` and no documented way to move it: every
//! rollout file is visible to every server on the machine, and
//! [`super::owned_sessions`] cannot close that gap, because a per-instance
//! registry is deliberately never read across instances. It can say "mine". It
//! can never say "someone else's", and so instance A's threads list in instance
//! B, browsable, indistinguishable from the user's own `codex` runs.
//!
//! The claim is therefore kept in the shared data directory, beside the inject
//! locks and for the reason [`crate::inject::lock`] gives: what is being divided
//! is shared, so the bookkeeping has to be shared too.
//!
//! ```text
//! <data>/codex-claims/<thread-id>   -> the name of the instance that made it
//! ```
//!
//! Three verdicts follow, and they are exhaustive:
//!
//! * claimed by **another** instance — [`Verdict::Hidden`], the same silence an
//!   account-backed claude session in a root this server does not own gets;
//! * claimed by **this** instance — [`Verdict::Owned`], and drivable;
//! * **unclaimed** — [`Verdict::External`]. Threads the user started with their
//!   own `codex` belong to the user and stay browsable from everywhere, which is
//!   what stops this from being a land grab.
//!
//! # What the file may be trusted to say
//!
//! First writer wins, and a claim is never rewritten. That is what makes it safe
//! to read with no lock: the content cannot change under a reader, it can only
//! appear and disappear. `create_new` is the whole of the exclusion, so two
//! servers racing on one id cannot both succeed.
//!
//! The registry remains the source of truth for what *this* instance owns; a
//! claim only adds the half a registry structurally cannot hold, which is what
//! the *others* own. Both directions of that split are load-bearing:
//!
//! * writing a claim is best effort, because a claim that never lands costs a
//!   sibling only the knowledge that the thread is taken — it lists there as
//!   external, which is the behaviour from before claims existed and is safe;
//! * a thread this instance's own registry records is never hidden, whatever a
//!   claim says. A bug here should cost a sibling a row it did not need, never
//!   this server the session it is holding.
//!
//! A claim that is empty or unparseable reads as no claim at all, and for the
//! same reason: unclaimed is the direction that shows rather than hides, and the
//! writer's next boot mirrors it back out of the registry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use super::{Paths, Source};

/// What one instance may conclude about one codex thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// This instance made it. Listed, and drivable.
    Owned,
    /// Nobody claimed it: the user's own `codex` thread. Listed, browse-only.
    External,
    /// A sibling instance made it. Not this server's to show.
    Hidden,
}

/// `<data>/codex-claims/<id>`, or `None` when the id is not a bare name.
fn claim_file(paths: &Paths, id: &str) -> Option<PathBuf> {
    super::reference::is_path_segment_id(id).then(|| paths.codex_claims_dir().join(id))
}

/// The instance named by one claim file, or `None` when it names nothing usable.
async fn owner_at(path: &Path) -> Option<String> {
    named(&tokio::fs::read_to_string(path).await.ok()?)
}

/// The instance a claim file's contents name, or `None` when they name nothing
/// usable.
fn named(raw: &str) -> Option<String> {
    let name = raw.trim();
    crate::instance::valid_name(name).then(|| name.to_string())
}

/// Record that this instance made `id`, for the other instances to read.
///
/// Called wherever [`super::owned_sessions::record`] records a codex thread —
/// from inside it, in fact, so the two cannot drift: a registry entry with no
/// claim is a thread a sibling still lists, which is the bug this exists to fix.
///
/// Best effort. Every failure is logged and none is propagated, because the run
/// that just succeeded must not be reported as failed over bookkeeping a sibling
/// reads. An existing claim is left exactly as it is — including one naming
/// another instance, which is logged as the conflict it is rather than resolved
/// by force.
///
/// The file is created and then filled, so a reader can catch it empty. That
/// window costs one listing refresh in a sibling, where the thread reads as
/// external; the alternative — writing elsewhere and renaming over — would trade
/// it for losing `create_new`, which is the exclusion itself.
pub async fn claim(paths: &Paths, id: &str) {
    let Some(path) = claim_file(paths, id) else {
        tracing::warn!(thread = %id, "not a codex thread id; no claim written");
        return;
    };
    let dir = paths.codex_claims_dir();
    if let Err(error) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(path = %dir.display(), %error, "cannot create the codex claim directory; siblings will list this thread");
        return;
    }
    write_claim(&path, paths.instance()).await;
    invalidate().await;
}

/// Create one claim file, or explain why the existing one stays.
///
/// Returns once the name is in the file, not merely on its way there. A tokio
/// `write_all` only hands the bytes to a blocking task and returns; without the
/// `flush` that waits for that task, the very next read — [`migrate`] checking
/// its own work, or a sibling listing — can find the file still empty, and a
/// write that fails is dropped with nobody told.
async fn write_claim(path: &Path, me: &str) {
    match tokio::fs::OpenOptions::new().write(true).create_new(true).open(path).await {
        Ok(mut file) => {
            let landed = match file.write_all(me.as_bytes()).await {
                Ok(()) => file.flush().await,
                Err(error) => Err(error),
            };
            if let Err(error) = landed {
                tracing::warn!(path = %path.display(), %error, "cannot write the codex claim");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => match owner_at(path).await {
            Some(holder) if holder == me => {}
            Some(holder) => tracing::warn!(
                path = %path.display(),
                %holder,
                instance = %me,
                "this codex thread is already claimed by another instance; leaving that claim in place"
            ),
            None => tracing::warn!(path = %path.display(), "a codex claim file names no instance; leaving it alone"),
        },
        Err(error) => tracing::warn!(path = %path.display(), %error, "cannot write the codex claim"),
    }
}

/// One cached directory read: which directory it was, the signature it was read
/// at, and the claims it held. Named because the tuple inside the `Mutex` is
/// otherwise deep enough for `clippy::type_complexity` to refuse the file.
type CachedClaims = Option<(PathBuf, String, HashMap<String, String>)>;

/// The last directory read, keyed by what the directory weighed and when it
/// changed.
///
/// A listing runs every five seconds for every attached client, and a claim file
/// is never rewritten — only created or removed, both of which move the
/// directory's own mtime. So one `stat` stands in for reading every file in it,
/// exactly as [`super::owned_sessions`] caches its registry parse. The one change
/// the `stat` cannot see is a new claim being filled, which is why
/// [`Claims::all`] never caches a read that caught one still empty.
static PARSED: Mutex<CachedClaims> = Mutex::const_new(None);

/// Forget the cached directory, because this process just changed it.
///
/// Belt and braces over the signature, which would catch the change anyway: two
/// writes inside one filesystem timestamp tick are the case it cannot see.
async fn invalidate() {
    *PARSED.lock().await = None;
}

/// The claims one pass has to consult.
///
/// Two constructors rather than one, because the two callers ask different
/// questions: a listing needs the whole directory, and resolving a single
/// session needs one file and should not pay for the rest.
#[derive(Debug, Default, Clone)]
pub struct Claims(HashMap<String, String>);

impl Claims {
    /// Every claim on the machine, thread id to instance name.
    pub async fn all(paths: &Paths) -> Self {
        let dir = paths.codex_claims_dir();
        let signature = super::file_signature(&dir).await;

        if let Some(signature) = signature.as_deref() {
            let cached = PARSED.lock().await;
            if let Some((cached_dir, cached_signature, claims)) = cached.as_ref() {
                if cached_dir == &dir && cached_signature == signature {
                    return Self(claims.clone());
                }
            }
        }

        let mut claims = HashMap::new();
        let mut settled = true;
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let id = entry.file_name().to_string_lossy().into_owned();
                if !super::reference::is_path_segment_id(&id) {
                    continue;
                }
                match tokio::fs::read_to_string(entry.path()).await {
                    Ok(raw) if raw.is_empty() => settled = false,
                    Ok(raw) => {
                        if let Some(holder) = named(&raw) {
                            claims.insert(id, holder);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        // An empty file is a claim caught between being created and being filled,
        // and filling it moves nothing the signature sees: the directory's mtime
        // changed when the file appeared, not when its name was written. Cached,
        // this read would leave the thread unclaimed here until some unrelated
        // claim came or went; uncached, the next listing reads it again, which is
        // the one refresh [`claim`] promises the window costs. A file left empty
        // for good, by a claimer that died between the two, costs each listing
        // the directory read the cache would have saved — slower, never wrong.
        if let Some(signature) = signature.filter(|_| settled) {
            *PARSED.lock().await = Some((dir, signature, claims.clone()));
        }
        Self(claims)
    }

    /// The one claim a single session lookup needs.
    pub async fn just(paths: &Paths, id: &str) -> Self {
        let Some(path) = claim_file(paths, id) else {
            return Self::default();
        };
        match owner_at(&path).await {
            Some(holder) => Self(HashMap::from([(id.to_string(), holder)])),
            None => Self::default(),
        }
    }

    /// Which instance made `id`, if any said so.
    pub fn owner(&self, id: &str) -> Option<&str> {
        self.0.get(id).map(String::as_str)
    }

    /// What the instance named `instance` should do with a row for `id`.
    ///
    /// `owned_here` is that instance's own registry answer, and it is checked
    /// first on purpose: the registry is the source of truth for what this
    /// server made, so no claim — stale, conflicting or corrupt — can hide a
    /// session this server is holding.
    pub fn verdict(&self, instance: &str, id: &str, owned_here: bool) -> Verdict {
        if owned_here {
            return Verdict::Owned;
        }
        match self.owner(id) {
            Some(holder) if holder == instance => Verdict::Owned,
            Some(_) => Verdict::Hidden,
            None => Verdict::External,
        }
    }
}

/// Mirror this instance's recorded codex threads into the shared directory.
///
/// Every thread created before claims existed has a registry entry and no claim,
/// which is precisely a thread that still lists in a sibling. One pass at boot
/// turns each of them into a claim, so each instance's own boot is what removes
/// its threads from everybody else's list.
///
/// Idempotent by construction — a claim that is already there is left alone, so
/// the second boot writes nothing — and never destructive: a claim naming
/// another instance is a conflict from before this bookkeeping existed and is
/// logged, not overwritten. Returns how many claims were newly written so a test
/// can assert the decision rather than a directory listing.
///
/// **Boot only, and under the instance lock**, like every other owner's move
/// there: it writes into a directory the sibling servers are reading.
pub async fn migrate(paths: &Paths) -> usize {
    let owned = super::owned_sessions::read(paths).await;
    let ids: Vec<String> = owned.ids(Source::Codex).map(str::to_owned).collect();
    if ids.is_empty() {
        return 0;
    }
    let dir = paths.codex_claims_dir();
    if let Err(error) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(path = %dir.display(), %error, "cannot create the codex claim directory; siblings will list this instance's threads");
        return 0;
    }

    let mut written = 0usize;
    for id in ids {
        let Some(path) = claim_file(paths, &id) else { continue };
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            continue;
        }
        write_claim(&path, paths.instance()).await;
        if owner_at(&path).await.as_deref() == Some(paths.instance()) {
            written += 1;
        }
    }
    if written > 0 {
        tracing::info!(
            claimed = written,
            "claimed this instance's existing codex threads so sibling instances stop listing them"
        );
    }
    invalidate().await;
    written
}

/// Drop this instance's claims whose rollout file is gone.
///
/// A claim outlives the thread it names — the user deletes a rollout, or a
/// handshake recorded an id whose file never appeared — and the directory is
/// shared, so it should not accumulate one dead file per thread forever.
///
/// Two restrictions make this safe to run at boot. Only claims naming **this**
/// instance are ever removed: a sibling's claim is the sibling's to retire, and
/// deleting one would un-hide its thread here. And the absence checked is the
/// rollout *file*, not the thread's visibility — `~/.codex` hides archived and
/// subagent threads from every cheap lookup, which is why
/// [`super::owned_sessions::prune`] leaves codex registry entries alone, but an
/// archived thread still has its file. No file is the one form of "gone" that is
/// not a lie. A missing sessions directory says nothing at all, so nothing is
/// dropped.
///
/// Returns how many claims were removed. **Boot only, under the instance lock.**
pub async fn prune(paths: &Paths) -> usize {
    let dir = paths.codex_claims_dir();
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return 0;
    };
    let me = paths.instance();
    let mut mine: Vec<(String, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let id = entry.file_name().to_string_lossy().into_owned();
        if !super::reference::is_path_segment_id(&id) {
            continue;
        }
        let path = entry.path();
        if owner_at(&path).await.as_deref() == Some(me) {
            mine.push((id, path));
        }
    }
    if mine.is_empty() {
        return 0;
    }
    // The scan of `~/.codex` is paid for only once there is something it could
    // answer for, and skipped entirely when there is no corpus to scan.
    let Some(alive) = super::codex::rollout_ids(paths).await else {
        return 0;
    };

    let mut dropped = 0usize;
    for (id, path) in mine {
        if alive.contains(&id) {
            continue;
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => dropped += 1,
            Err(error) => tracing::warn!(path = %path.display(), %error, "cannot drop a stale codex claim"),
        }
    }
    if dropped > 0 {
        tracing::info!(dropped, "dropped codex claims whose rollout file no longer exists");
        invalidate().await;
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instances(dir: &tempfile::TempDir) -> (Paths, Paths) {
        (
            Paths::for_instance(dir.path().to_path_buf(), "a"),
            Paths::for_instance(dir.path().to_path_buf(), "b"),
        )
    }

    /// The whole point, in one test: A's thread is A's everywhere, and B is told
    /// so rather than left to guess.
    #[tokio::test]
    async fn a_thread_one_instance_claimed_is_hidden_from_the_other() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b) = instances(&dir);
        claim(&a, "thread-a").await;

        let seen_by_a = Claims::all(&a).await;
        let seen_by_b = Claims::all(&b).await;
        assert_eq!(seen_by_a.owner("thread-a"), Some("a"));
        assert_eq!(seen_by_b.owner("thread-a"), Some("a"), "the claim is shared, not private");

        assert_eq!(seen_by_a.verdict("a", "thread-a", true), Verdict::Owned);
        assert_eq!(seen_by_b.verdict("b", "thread-a", false), Verdict::Hidden);
        // And a thread nobody claimed stays the user's own, everywhere.
        assert_eq!(seen_by_a.verdict("a", "unclaimed", false), Verdict::External);
        assert_eq!(seen_by_b.verdict("b", "unclaimed", false), Verdict::External);
    }

    /// A claim this server's registry contradicts must not cost it a session.
    #[tokio::test]
    async fn the_registry_outranks_a_claim_that_would_hide_our_own_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, _b) = instances(&dir);
        claim(&a, "contested").await;
        let claims = Claims::all(&a).await;
        assert_eq!(claims.verdict("b", "contested", true), Verdict::Owned, "our registry wins");
        assert_eq!(claims.verdict("b", "contested", false), Verdict::Hidden);
    }

    #[tokio::test]
    async fn the_first_claim_wins_and_is_never_rewritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b) = instances(&dir);
        claim(&a, "thread-a").await;
        claim(&b, "thread-a").await;
        assert_eq!(
            Claims::all(&b).await.owner("thread-a"),
            Some("a"),
            "the second writer must not take the first one's thread"
        );
        // Claiming one's own thread twice is a no-op, not a conflict.
        claim(&a, "thread-a").await;
        assert_eq!(Claims::all(&a).await.owner("thread-a"), Some("a"));
    }

    #[tokio::test]
    async fn a_claim_naming_nothing_usable_reads_as_unclaimed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, _b) = instances(&dir);
        std::fs::create_dir_all(a.codex_claims_dir()).expect("claims dir");
        std::fs::write(a.codex_claims_dir().join("empty"), "").expect("empty claim");
        std::fs::write(a.codex_claims_dir().join("junk"), "Not A Name!").expect("junk claim");

        let claims = Claims::all(&a).await;
        assert_eq!(claims.verdict("a", "empty", false), Verdict::External);
        assert_eq!(claims.verdict("a", "junk", false), Verdict::External, "unreadable never hides");
    }

    /// A sibling that lists in the instant between a claim being created and
    /// being filled reads it as unclaimed, and that must cost it one refresh:
    /// filling the file leaves the directory's signature where it was, so a
    /// cached miss would otherwise outlive the write.
    #[tokio::test]
    async fn a_claim_caught_before_it_is_filled_is_read_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b) = instances(&dir);
        let claims_dir = a.codex_claims_dir();
        std::fs::create_dir_all(&claims_dir).expect("claims dir");
        std::fs::write(claims_dir.join("thread-a"), "").expect("created, not yet filled");
        assert_eq!(Claims::all(&b).await.owner("thread-a"), None);

        let signature = super::super::file_signature(&claims_dir).await;
        std::fs::write(claims_dir.join("thread-a"), "a").expect("filled");
        assert_eq!(
            super::super::file_signature(&claims_dir).await,
            signature,
            "filling moves nothing the cache sees"
        );
        assert_eq!(Claims::all(&b).await.owner("thread-a"), Some("a"), "the empty read was not remembered");
    }

    /// Resolving one session must reach the same verdict as listing them all,
    /// or a row the list hides would still open by URL.
    #[tokio::test]
    async fn one_lookup_agrees_with_the_whole_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b) = instances(&dir);
        claim(&a, "thread-a").await;
        assert_eq!(Claims::just(&b, "thread-a").await.verdict("b", "thread-a", false), Verdict::Hidden);
        assert_eq!(Claims::just(&b, "missing").await.verdict("b", "missing", false), Verdict::External);
        assert_eq!(
            Claims::just(&b, "../../escape").await.verdict("b", "../../escape", false),
            Verdict::External,
            "a traversing id claims nothing"
        );
    }

    #[tokio::test]
    async fn a_traversing_id_never_becomes_a_claim_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, _b) = instances(&dir);
        claim(&a, "../escape").await;
        assert!(
            !dir.path().join("escape").exists() && !a.codex_claims_dir().join("../escape").exists(),
            "nothing may be written outside the claims directory"
        );
    }

    #[tokio::test]
    async fn the_boot_migration_mirrors_the_registry_once_and_never_steals() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b) = instances(&dir);
        super::super::owned_sessions::record(&a, Source::Claude, "session-a")
            .await
            .expect("record");
        assert_eq!(migrate(&a).await, 0, "a claude session is not a codex thread");

        // Simulate a registry written before claims existed: the entry is there,
        // the claim is not.
        super::super::owned_sessions::record(&a, Source::Codex, "old-thread")
            .await
            .expect("record");
        std::fs::remove_file(a.codex_claims_dir().join("old-thread")).expect("unclaim");

        assert_eq!(migrate(&a).await, 1, "the pre-existing thread is claimed at boot");
        assert_eq!(Claims::all(&b).await.owner("old-thread"), Some("a"));
        assert_eq!(migrate(&a).await, 0, "and the second boot writes nothing");

        // A conflict is reported by leaving the other instance's claim alone.
        super::super::owned_sessions::record(&b, Source::Codex, "old-thread")
            .await
            .expect("record");
        assert_eq!(migrate(&b).await, 0);
        assert_eq!(Claims::all(&b).await.owner("old-thread"), Some("a"), "never overwritten");
    }

    #[tokio::test]
    async fn pruning_drops_only_our_own_claims_and_only_with_no_rollout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, b) = instances(&dir);
        let day = a.codex_sessions().join("2026").join("05").join("09");
        std::fs::create_dir_all(&day).expect("sessions dir");
        let alive = "11111111-2222-3333-4444-555555555555";
        std::fs::write(day.join(format!("rollout-2026-05-09T16-39-49-{alive}.jsonl")), "{}\n").expect("rollout");

        claim(&a, alive).await;
        claim(&a, "22222222-2222-3333-4444-555555555555").await;
        claim(&b, "33333333-2222-3333-4444-555555555555").await;

        assert_eq!(prune(&a).await, 1, "only the one with no rollout file");
        let claims = Claims::all(&a).await;
        assert_eq!(claims.owner(alive), Some("a"), "a live thread keeps its claim");
        assert_eq!(claims.owner("22222222-2222-3333-4444-555555555555"), None);
        assert_eq!(
            claims.owner("33333333-2222-3333-4444-555555555555"),
            Some("b"),
            "a sibling's stale claim is the sibling's to retire"
        );
        assert_eq!(prune(&a).await, 0, "and pruning again finds nothing");
    }

    /// No `~/.codex/sessions` is "no answer", not "nothing exists": a fresh
    /// machine, or a home directory that has not been written yet, must not
    /// disown every thread this instance made.
    #[tokio::test]
    async fn a_missing_sessions_directory_drops_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (a, _b) = instances(&dir);
        claim(&a, "thread-a").await;
        assert_eq!(prune(&a).await, 0);
        assert_eq!(Claims::all(&a).await.owner("thread-a"), Some("a"));
    }
}
