//! Which sessions are waiting on you.
//!
//! A phone that shows twenty sessions is useless unless it can say which of them
//! wants something. That is one comparison of two clocks:
//!
//! * **`attentionAt`** — when the assistant last actually spoke on the main
//!   chain, derived from the transcript itself ([`unread`], over the per-line
//!   rule in [`derive`]). The transcript *is* the record, and a derived clock
//!   cannot fall out of step with the file the way a mirrored one would after a
//!   crash, a restore, or a session driven from the terminal instead of from
//!   here. What is kept is only a cache of that derivation.
//! * **`readAt`** — when the user last looked, recorded by the client through
//!   `POST /api/sessions/{source}/{id}/seen` and kept in a small sidecar
//!   ([`seen`]). This half cannot be derived from anything: only the browser
//!   knows whether the tab was in front.
//!
//! `unread` is `attentionAt > readAt`, suppressed while the session is
//! `running`. The suppression is what keeps the badge honest — a session mid-turn
//! is not waiting for you even if its last sentence is unread, and it would
//! otherwise light up the moment it started working and stay lit.
//!
//! `unreadCount` is how many messages arrived after `readAt` (capped at 99),
//! and is deliberately *not* suppressed: it is a quantity, not a call to act,
//! and a session that is working can be holding answers you have not read.
//!
//! # Where the work happens
//!
//! [`apply`] is called from [`crate::sessions::list_and_filter`], which is the
//! single place the session list is assembled — so `GET /api/sessions` and the
//! websocket's `sessions:snapshot`/`sessions:delta` rows agree by construction
//! rather than by two implementations happening to match.
//!
//! That path runs every five seconds for every attached client, so the
//! derivation — an [`unread::Ledger`] of the newest message timestamps — is
//! memoised against the row's own `(updatedAt, sizeBytes)` pair, in memory and
//! in `index.db`: a transcript is append-only, so a session that has not been
//! written to since the last pass costs nothing but a map lookup, one that has
//! costs the bytes it grew by, and a restart costs an indexed read rather than
//! a scan. No `stat` is needed — both halves of the key are already in the row.

pub mod derive;
pub mod seen;
pub mod unread;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::extract::ApiPath;
use crate::api::{ApiRouter, AppState};
use crate::db::index::Index;
use crate::openapi_ext::{ExtraResponses, Nullable};
use crate::sessions::util::{ItemKind, Role, TranscriptItem};
use crate::sessions::{Paths, SessionStatus, SessionSummary, Source};
use unread::Ledger;

/// Whether a session is waiting on the user.
///
/// The rule the badge, the sort and the push trigger all read, spelled once.
///
/// Timestamps are compared as strings. Every clock involved is ISO-8601 UTC with
/// milliseconds — the CLIs write theirs that way and [`crate::sessions::iso8601`]
/// writes ours that way — and for one fixed format that is a plain lexicographic
/// compare, which is what the session list already sorts by.
pub fn is_unread(attention_at: Option<&str>, read_at: Option<&str>, status: SessionStatus) -> bool {
    // Mid-turn is not "waiting on you", whatever the clocks say.
    if status == SessionStatus::Running {
        return false;
    }
    let Some(attention_at) = attention_at else {
        // The assistant has never spoken: there is nothing to have missed.
        return false;
    };
    match read_at {
        Some(read_at) => attention_at > read_at,
        // Never opened, and the assistant has said something.
        None => true,
    }
}

/// Where the messages a visit has not read yet begin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct FirstUnread {
    /// Absolute index of the first agent message newer than `after`, in the
    /// feed's own item numbering — the one the history route pages by and a
    /// transcript row's `data-ti` carries — or `null` when there is none.
    pub index: Nullable<usize>,
    /// How many agent messages are newer than `after`, which is what the
    /// client's button says. Not capped at 99 like `unreadCount`: this walks the
    /// whole transcript. Zero exactly when `index` is `null`.
    pub count: usize,
}

/// [`FirstUnread`] over one transcript's items.
///
/// The messages are the ones `unreadCount` counts: an agent text item is
/// exactly one of [`derive::spoken_texts`]' texts — a non-empty text block of a
/// main-chain claude `assistant` entry, a non-wrapper text part of a codex
/// assistant `message` — stamped with its entry's timestamp. They are read off
/// the feed's parse rather than the [`unread::Ledger`] for two reasons: the
/// answer has to be an index a client can page to, which only that parse
/// numbers; and the ledger stops at [`unread::CAP`], while a reader coming back
/// to `99+` wants the first message they missed, not the 99th newest. The parse
/// is the one search already caches, so walking all of it is cheap.
///
/// `after` is the read marker the visit began with, compared as a string like
/// every clock in this module; `None` counts from the start, as the count does
/// for a session never looked at. A message with no timestamp of its own stands
/// at `fallback_at`, the transcript's mtime — the stand-in the ledger gives it
/// too ([`unread::refresh`]), so the two count it alike.
pub fn first_unread(items: &[TranscriptItem], after: Option<&str>, fallback_at: &str) -> FirstUnread {
    let mut first = None;
    let mut count = 0;
    for (index, item) in items.iter().enumerate() {
        if item.role != Role::Assistant || item.kind != ItemKind::Text {
            continue;
        }
        let at = item.timestamp.0.as_deref().unwrap_or(fallback_at);
        if after.is_none_or(|after| at > after) {
            first.get_or_insert(index);
            count += 1;
        }
    }
    FirstUnread {
        index: Nullable(first),
        count,
    }
}

/// One ledger, and the row state it was brought up to date against.
///
/// `updated_at` and `size_bytes` are a change *detector*, not an identity: a
/// transcript only ever grows, so a pair that still matches means nothing has
/// been appended and the previous answer still stands. Both are already on the
/// row, which is why nothing here has to `stat`.
struct Cached {
    updated_at: String,
    size_bytes: u64,
    /// An empty ledger is remembered too — a session whose assistant has not
    /// spoken is exactly the case that would otherwise re-read the same file
    /// every five seconds forever.
    ledger: Ledger,
}

/// Ledgers already brought up to date, keyed by transcript path.
static CACHE: LazyLock<Mutex<HashMap<String, Cached>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// How many ledgers are memoised at once.
///
/// One entry is added per distinct transcript ever listed, and the seed from
/// `recent_cwds` means a long-lived server touches far more than any one list
/// shows. Well past a working set, and bounded so the map cannot grow for the
/// life of the install. The same rows are kept durably in `index.db`, so an
/// eviction costs one indexed read rather than a scan.
const MAX_CACHE_ENTRIES: usize = 1000;

/// Drop the coldest entries until at most [`MAX_CACHE_ENTRIES`] remain.
///
/// Cold by the row's own `updated_at`: a transcript that has not been written to
/// in the longest is the one least likely to be asked about again, the same
/// eviction shape [`seen::evict`] uses. Ties broken by key so it is deterministic.
fn evict(cache: &mut HashMap<String, Cached>) {
    if cache.len() <= MAX_CACHE_ENTRIES {
        return;
    }
    let mut by_age: Vec<(&String, &String)> = cache.iter().map(|(key, entry)| (key, &entry.updated_at)).collect();
    by_age.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)));
    let doomed: Vec<String> = by_age
        .iter()
        .take(cache.len() - MAX_CACHE_ENTRIES)
        .map(|(key, _)| (*key).clone())
        .collect();
    for key in doomed {
        cache.remove(&key);
    }
}

/// The unread ledger for one row: from the memo, from `index.db`, or from the
/// file — in that order, and reading only what the first two cannot answer.
///
/// A ledger whose watermark still matches the row is the answer as it stands,
/// provided it can count against this read marker (see [`Ledger::floor`]).
/// One that is merely *behind* the row is resumed from where it stopped. The
/// result goes back into both tiers whenever it changed.
async fn ledger_for(index: Option<&Index>, summary: &SessionSummary, read_at: Option<&str>) -> Option<Ledger> {
    let current = |updated_at: &str, size_bytes: u64, ledger: &Ledger| {
        updated_at == summary.updated_at && size_bytes == summary.size_bytes && ledger.count(read_at).is_some()
    };
    let memo = || {
        CACHE
            .lock()
            .expect("attention cache poisoned")
            .get(&summary.file_path)
            .map(|cached| (cached.updated_at.clone(), cached.size_bytes, cached.ledger.clone()))
    };
    if let Some((updated_at, size_bytes, ledger)) = memo() {
        if current(&updated_at, size_bytes, &ledger) {
            return Some(ledger);
        }
    }
    // One derivation per transcript at a time. The global feed's re-list and an
    // HTTP `GET /api/sessions` routinely land together — on the first list
    // after an upgrade that is two cold scans of every transcript — so the
    // second caller waits for the first and then finds its answer in the memo.
    let _turn = single_flight(&summary.file_path).await;
    let memo = memo();
    if let Some((updated_at, size_bytes, ledger)) = &memo {
        if current(updated_at, *size_bytes, ledger) {
            return Some(ledger.clone());
        }
    }
    // The durable tier, for a restart or an eviction. Only consulted when the
    // memo has nothing to say: a memo entry is never older than the index row
    // it was written alongside.
    let known = match memo {
        Some((_, _, ledger)) => Some(ledger),
        None => match index.and_then(|index| index.unread_ledger::<Ledger>(&summary.file_path)) {
            Some((updated_at, size_bytes, ledger)) if current(&updated_at, size_bytes, &ledger) => {
                remember(None, summary, &ledger);
                return Some(ledger);
            }
            Some((_, _, ledger)) => Some(ledger),
            None => None,
        },
    };
    let ledger = unread::refresh(
        summary.source,
        Path::new(&summary.file_path),
        known,
        summary.size_bytes,
        read_at,
        &summary.updated_at,
    )
    .await?;
    remember(index, summary, &ledger);
    Some(ledger)
}

/// Per-transcript turns at deriving a ledger; see [`ledger_for`].
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Wait for this transcript's turn; the turn ends when the guard drops.
async fn single_flight(path: &str) -> FlightGuard {
    let lock = IN_FLIGHT
        .lock()
        .expect("attention in-flight map poisoned")
        .entry(path.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    FlightGuard {
        path: path.to_string(),
        guard: Some(lock.lock_owned().await),
    }
}

/// A transcript's turn, which also tidies its entry away when nobody else is
/// waiting — so the map holds only what is in flight, not every path ever seen.
struct FlightGuard {
    path: String,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
        let mut map = IN_FLIGHT.lock().expect("attention in-flight map poisoned");
        // The map's own reference is the only one left: no holder, no waiter.
        if map.get(&self.path).is_some_and(|lock| Arc::strong_count(lock) == 1) {
            map.remove(&self.path);
        }
    }
}

/// Keep a ledger in the memo, and in the index when one is given.
fn remember(index: Option<&Index>, summary: &SessionSummary, ledger: &Ledger) {
    if let Some(index) = index {
        index.remember_unread_ledger(&summary.file_path, &summary.updated_at, summary.size_bytes, ledger);
    }
    let mut cache = CACHE.lock().expect("attention cache poisoned");
    cache.insert(
        summary.file_path.clone(),
        Cached {
            updated_at: summary.updated_at.clone(),
            size_bytes: summary.size_bytes,
            ledger: ledger.clone(),
        },
    );
    evict(&mut cache);
}

/// Fill in `attentionAt`, `unread` and `unreadCount` on a page of session rows.
///
/// The seen store is read once for the whole page rather than per row: it is
/// one small scope and every row asks it the same question.
///
/// `attentionAt` is the newest message in the same ledger the count comes
/// from, so the three can never disagree: `unread` implies a non-zero count,
/// and a count of zero means `unread` is false. The count is *not* suppressed
/// while a turn runs — a working session can be holding three answers you
/// have not read — but `unread` still is, exactly as before.
pub async fn apply(paths: &Paths, sessions: &mut [SessionSummary]) {
    if sessions.is_empty() {
        return;
    }
    let seen = seen::read(paths).await;
    let index = crate::db::index::current();
    for summary in sessions.iter_mut() {
        let read_at = seen.get(&seen::key_of(summary.source, &summary.id)).map(String::as_str);
        let ledger = ledger_for(index.as_ref(), summary, read_at).await;
        fill(summary, ledger.as_ref(), read_at);
    }
}

/// When the assistant last spoke in one session, read from the same ledger
/// [`apply`] fills the list row from — for a caller outside the list that must
/// agree with the row's badge (the push gate in [`crate::push`]).
pub async fn clock_of(summary: &SessionSummary, read_at: Option<&str>) -> Option<String> {
    let index = crate::db::index::current();
    let ledger = ledger_for(index.as_ref(), summary, read_at).await?;
    ledger.newest().map(str::to_owned)
}

/// Write one row's three attention fields from its ledger and read marker.
fn fill(summary: &mut SessionSummary, ledger: Option<&Ledger>, read_at: Option<&str>) {
    let at = ledger.and_then(Ledger::newest).map(str::to_owned);
    summary.unread_count = ledger.and_then(|ledger| ledger.count(read_at)).unwrap_or(0);
    summary.unread = summary.unread_count > 0 && is_unread(at.as_deref(), read_at, summary.status);
    summary.attention_at = Nullable(at);
}

/// Mark a session as read, as of now.
///
/// Deliberately does no existence check. The client calls this on every focus
/// and visibility change, and a lookup would mean scanning a projects tree to
/// answer a request whose whole job is to write eight words to a small file. An
/// id that names nothing costs one entry, which the store's own cap reclaims.
#[api(id = "mark_session_seen", group = "sessions", summary = "Mark a session read")]
pub async fn mark_seen(State(state): State<AppState>, ApiPath((source, id)): ApiPath<(Source, String)>) -> Result<Json<SeenResult>, ApiError> {
    let previous = seen::mark(state.paths.as_ref(), source, &id)
        .await
        .map_err(|error| ApiError::internal(format!("failed to record seen: {error}")))?;
    // The badge lives on the session-list rows, and those only change when the
    // feed re-lists — without this the badge would linger for up to the poll
    // interval after the user has plainly already looked at the session.
    state.global.request_refresh(None);
    Ok(Json(SeenResult {
        accepted: true,
        previous_read_at: Nullable(previous),
    }))
}

/// Mark one of a session's subagents as read, as of now.
///
/// For a subagent that is not a session of its own (Claude's), whose read mark
/// lives within its parent's: the tree under a session shows it unread until
/// it, or the session, is opened after it last wrote. A subagent that is a
/// session (a Codex thread) is marked through the session route.
#[api(id = "mark_agent_seen", group = "sessions", summary = "Mark a subagent read")]
pub async fn mark_agent_seen(
    State(state): State<AppState>,
    ApiPath(crate::drive::routes::AgentPath { source, id, agent_id }): ApiPath<crate::drive::routes::AgentPath>,
) -> Result<Json<SeenResult>, ApiError> {
    if !crate::drive::routes::is_agent_id(&agent_id) {
        return Err(ApiError::bad_request("invalid agent id"));
    }
    let previous = seen::mark(state.paths.as_ref(), source, &crate::sessions::subagents::agent_seen_id(&id, &agent_id))
        .await
        .map_err(|error| ApiError::internal(format!("failed to record seen: {error}")))?;
    state.global.request_refresh(None);
    Ok(Json(SeenResult {
        accepted: true,
        previous_read_at: Nullable(previous),
    }))
}

/// What marking a session read answers.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SeenResult {
    /// Always true; the shape every other accept-only route answers with.
    pub accepted: bool,
    /// The mark this one replaced: when the user had last looked before now,
    /// ISO-8601, or `null` for a session never marked (or whose mark was
    /// evicted).
    ///
    /// The reader arriving at a session reports it seen at once, and that
    /// report is what moves the mark — so this is the only reliable place to
    /// learn where their unread messages began. A client hands it to
    /// `GET /api/sessions/{source}/{id}/first-unread?after=` to offer the way
    /// back to the first of them.
    pub previous_read_at: Nullable<String>,
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .post("/api/sessions/{source}/{id}/seen", mark_seen)
        .post("/api/sessions/{source}/{id}/agents/{agentId}/seen", mark_agent_seen)
}

/// Everything gotcha cannot express about those routes.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe_final("POST", "/api/sessions/{source}/{id}/seen", 200, "The session is now marked read")
        .describe_final(
            "POST",
            "/api/sessions/{source}/{id}/agents/{agentId}/seen",
            200,
            "The subagent is now marked read",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "2026-01-01T00:00:00.000Z";
    const NEW: &str = "2026-01-02T00:00:00.000Z";

    #[test]
    fn unread_is_an_assistant_who_spoke_after_the_last_look() {
        assert!(is_unread(Some(NEW), Some(OLD), SessionStatus::Idle), "spoke since the last look");
        assert!(!is_unread(Some(OLD), Some(NEW), SessionStatus::Idle), "already looked");
        assert!(!is_unread(Some(OLD), Some(OLD), SessionStatus::Idle), "looked at that very moment");
    }

    #[test]
    fn a_session_never_opened_is_unread_once_the_assistant_speaks() {
        assert!(is_unread(Some(NEW), None, SessionStatus::Idle));
        assert!(!is_unread(None, None, SessionStatus::Idle), "nothing has been said yet");
        assert!(!is_unread(None, Some(OLD), SessionStatus::Idle));
    }

    /// A session mid-turn is not waiting on anyone. Without this every session
    /// would light up the moment it started working — its last sentence is
    /// genuinely unread, and that is genuinely not the user's problem yet.
    #[test]
    fn a_running_session_is_never_unread() {
        assert!(!is_unread(Some(NEW), Some(OLD), SessionStatus::Running));
        assert!(!is_unread(Some(NEW), None, SessionStatus::Running));
    }

    /// `waiting` is the CLI sitting on a question or a finished answer, which is
    /// the single most important case for the badge to catch.
    #[test]
    fn a_waiting_session_is_unread_like_an_idle_one() {
        assert!(is_unread(Some(NEW), Some(OLD), SessionStatus::Waiting));
        assert!(!is_unread(Some(OLD), Some(NEW), SessionStatus::Waiting));
    }

    fn cached(updated_at: &str) -> Cached {
        Cached {
            updated_at: updated_at.to_string(),
            size_bytes: 0,
            ledger: Ledger::default(),
        }
    }

    /// One entry is added per transcript ever listed; without a cap the memo
    /// grows for the life of the install. Eviction keeps the hottest and drops
    /// the coldest by `updated_at`.
    #[test]
    fn the_derivation_cache_is_bounded_and_keeps_the_hottest() {
        let mut cache: HashMap<String, Cached> = (0..MAX_CACHE_ENTRIES + 50)
            .map(|n| {
                (
                    format!("/p/{n:05}.jsonl"),
                    cached(&format!("2026-01-01T00:{:02}:{:02}.000Z", (n / 60) % 60, n % 60)),
                )
            })
            .collect();
        // Two unambiguously ancient entries that must be the first to go.
        cache.insert("/p/ancient-a.jsonl".into(), cached("1999-01-01T00:00:00.000Z"));
        cache.insert("/p/ancient-b.jsonl".into(), cached("1999-01-02T00:00:00.000Z"));
        let before = cache.len();

        evict(&mut cache);

        assert_eq!(cache.len(), MAX_CACHE_ENTRIES, "evicted down to the cap, not below it");
        assert!(before > MAX_CACHE_ENTRIES, "the fixture actually overflowed");
        assert!(!cache.contains_key("/p/ancient-a.jsonl"), "the coldest updatedAt goes first");
        assert!(!cache.contains_key("/p/ancient-b.jsonl"));
    }

    fn row(file_path: &Path, updated_at: &str, status: SessionStatus) -> SessionSummary {
        SessionSummary {
            pinned: false,
            id: "s".into(),
            source: Source::Claude,
            scope: crate::sessions::SessionScope::Owned,
            server: "default".into(),
            title: "t".into(),
            cwd: Nullable(None),
            repo_key: Nullable(None),
            git_branch: Nullable(None),
            updated_at: updated_at.into(),
            created_at: updated_at.into(),
            size_bytes: std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0),
            file_path: file_path.to_string_lossy().into_owned(),
            status,
            activity: crate::sessions::SessionActivity::from_status(status),
            resident: Nullable(None),
            claude_root: None,
            acp_agent: None,
            account_id: None,
            attention_at: Nullable(None),
            unread: false,
            unread_count: 0,
            asking: false,
            children: Vec::new(),
            parent_id: Nullable(None),
        }
    }

    fn said(n: u32) -> String {
        format!(r#"{{"type":"assistant","timestamp":"2026-01-01T10:00:{n:02}.000Z","message":{{"content":[{{"type":"text","text":"m{n}"}}]}}}}"#)
    }

    fn stamp(n: u32) -> String {
        format!("2026-01-01T10:00:{n:02}.000Z")
    }

    /// The point of keeping ledgers in `index.db`: a restart (or a memo
    /// eviction) whose row still matches the stored watermark reads the answer
    /// back instead of the transcript. The file here holds no message at all, so
    /// the only place three could come from is the index.
    #[tokio::test]
    async fn an_indexed_ledger_is_believed_without_reading_the_transcript() {
        let home = tempfile::tempdir().expect("tempdir");
        let index = crate::db::index::open(&Paths::new(home.path().to_path_buf()));
        let file = home.path().join("quiet.jsonl");
        std::fs::write(&file, "{\"type\":\"user\"}\n").expect("write");
        let summary = row(&file, "2026-01-01T10:00:09.000Z", SessionStatus::Idle);
        let stored = Ledger {
            stamps: vec![stamp(1), stamp(2), stamp(3)],
            floor: None,
            end: summary.size_bytes,
            head: unread::head_of_file(&file).await.expect("head"),
        };
        index.remember_unread_ledger(&summary.file_path, &summary.updated_at, summary.size_bytes, &stored);

        let ledger = ledger_for(Some(&index), &summary, None).await.expect("a ledger");
        assert_eq!(ledger, stored, "same watermark: no re-parse");
        assert_eq!(ledger.count(None), Some(3));
    }

    /// A transcript that grew since its ledger was stored resumes from the
    /// stored `end` — two messages from the index, one read off the new bytes —
    /// rather than scanning from scratch (which would find only one).
    #[tokio::test]
    async fn a_grown_transcript_resumes_from_its_indexed_ledger() {
        let home = tempfile::tempdir().expect("tempdir");
        let index = crate::db::index::open(&Paths::new(home.path().to_path_buf()));
        let file = home.path().join("grown.jsonl");
        std::fs::write(&file, "{\"type\":\"user\"}\n").expect("write");
        let before = row(&file, "2026-01-01T10:00:05.000Z", SessionStatus::Idle);
        index.remember_unread_ledger(
            &before.file_path,
            &before.updated_at,
            before.size_bytes,
            &Ledger {
                stamps: vec![stamp(1), stamp(2)],
                floor: None,
                end: before.size_bytes,
                head: unread::head_of_file(&file).await.expect("head"),
            },
        );

        let mut body = std::fs::read_to_string(&file).expect("read");
        body.push_str(&said(7));
        body.push('\n');
        std::fs::write(&file, body).expect("append");
        let after = row(&file, "2026-01-01T10:00:07.000Z", SessionStatus::Idle);
        let ledger = ledger_for(Some(&index), &after, None).await.expect("a ledger");
        assert_eq!(ledger.stamps, vec![stamp(1), stamp(2), stamp(7)]);

        // …and the up-to-date ledger went back into the index, against the new row.
        let (updated_at, size, stored) = index.unread_ledger::<Ledger>(&after.file_path).expect("stored");
        assert_eq!((updated_at.as_str(), size), (after.updated_at.as_str(), after.size_bytes));
        assert_eq!(stored, ledger);
    }

    #[tokio::test]
    async fn a_row_carries_the_count_beside_the_flag() {
        let home = tempfile::tempdir().expect("tempdir");
        let file = home.path().join("t.jsonl");
        std::fs::write(&file, format!("{}\n{}\n{}\n", said(1), said(2), said(3))).expect("write");

        let mut idle = row(&file, "2026-01-01T10:00:03.000Z", SessionStatus::Idle);
        let ledger = ledger_for(None, &idle, Some(&stamp(1))).await.expect("a ledger");
        fill(&mut idle, Some(&ledger), Some(&stamp(1)));
        assert_eq!((idle.unread, idle.unread_count), (true, 2), "two answers since the last look");
        assert_eq!(idle.attention_at.0.as_deref(), Some(stamp(3).as_str()));

        // Mid-turn the flag is suppressed as it always was; the count is not.
        let mut running = row(&file, "2026-01-01T10:00:03.000Z", SessionStatus::Running);
        fill(&mut running, Some(&ledger), Some(&stamp(1)));
        assert_eq!((running.unread, running.unread_count), (false, 2));

        // A read marker newer than everything: nothing unread.
        let mut read = row(&file, "2026-01-01T10:00:03.000Z", SessionStatus::Idle);
        fill(&mut read, Some(&ledger), Some("2026-01-01T11:00:00.000Z"));
        assert_eq!((read.unread, read.unread_count), (false, 0));

        // Never opened: every message counts.
        let mut never = row(&file, "2026-01-01T10:00:03.000Z", SessionStatus::Waiting);
        let full = ledger_for(None, &never, None).await.expect("a ledger");
        fill(&mut never, Some(&full), None);
        assert_eq!((never.unread, never.unread_count), (true, 3));
    }

    /// The jump and the count are one rule: the first unread item is the oldest
    /// message the ledger counts, against any marker — thinking, tool calls and
    /// a subagent's words count in neither, and a message with no timestamp
    /// stands at the transcript's mtime in both.
    #[tokio::test]
    async fn the_first_unread_is_the_oldest_message_the_count_counts() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let file = home.path().join("first.jsonl");
        let ask = r#"{"type":"user","timestamp":"2026-01-01T09:59:00.000Z","message":{"content":"go"}}"#;
        let tool = r#"{"type":"assistant","timestamp":"2026-01-01T10:00:02.500Z","message":{"content":[{"type":"thinking","thinking":"hmm"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{}}]}}"#;
        let side = r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-01-01T10:00:02.600Z","message":{"content":[{"type":"text","text":"a subagent"}]}}"#;
        let undated = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"undated"}]}}"#;
        std::fs::write(
            &file,
            format!("{ask}\n{}\n{}\n{tool}\n{side}\n{}\n{undated}\n", said(1), said(2), said(3)),
        )
        .expect("write");
        let items = crate::sessions::transcript::parse_claude_transcript(&paths, &file, false)
            .await
            .expect("parse");
        // The row's `updatedAt` is the transcript's mtime: the ledger's stand-in.
        let mtime = "2026-01-01T10:00:04.000Z";
        let summary = row(&file, mtime, SessionStatus::Idle);
        let full = ledger_for(None, &summary, None).await.expect("a ledger");

        let markers = [None, Some(stamp(1)), Some(stamp(2)), Some(stamp(3)), Some(mtime.to_string())];
        for read_at in &markers {
            let first = first_unread(&items, read_at.as_deref(), mtime);
            assert_eq!(
                Some(first.count as u32),
                full.count(read_at.as_deref()),
                "the same count against {read_at:?}"
            );
            assert_eq!(first.index.0.is_some(), first.count > 0, "an index exactly when there is a count");
        }
        let text_at = |found: FirstUnread| found.index.0.map(|index| items[index].text.clone());
        assert_eq!(
            text_at(first_unread(&items, None, mtime)).as_deref(),
            Some("m1"),
            "never read: the very first answer"
        );
        assert_eq!(text_at(first_unread(&items, Some(&stamp(1)), mtime)).as_deref(), Some("m2"));
        assert_eq!(
            text_at(first_unread(&items, Some(&stamp(2)), mtime)).as_deref(),
            Some("m3"),
            "past the tool call"
        );
        assert_eq!(
            text_at(first_unread(&items, Some(&stamp(3)), mtime)).as_deref(),
            Some("undated"),
            "counted, as the badge counts it"
        );
        assert_eq!(
            first_unread(&items, Some(mtime), mtime),
            FirstUnread {
                index: Nullable(None),
                count: 0
            }
        );
    }

    /// The count stops at 99; the walk to the first unread does not, because a
    /// reader coming back to `99+` wants the first message they missed.
    #[test]
    fn the_first_unread_walks_past_the_cap() {
        use crate::sessions::util::make_item;
        let mut items = vec![make_item(Role::User, ItemKind::Text, "go", Some("2026-01-01T09:00:00.000Z"))];
        for n in 0..150 {
            let at = format!("2026-01-01T10:{:02}:{:02}.000Z", n / 60, n % 60);
            items.push(make_item(Role::Assistant, ItemKind::Text, &format!("m{n}"), Some(&at)));
        }
        assert_eq!(
            first_unread(&items, Some("2026-01-01T09:30:00.000Z"), "2026-01-01T11:00:00.000Z"),
            FirstUnread {
                index: Nullable(Some(1)),
                count: 150
            },
            "uncapped"
        );
    }

    /// The global feed and an HTTP list arrive together; the second must wait
    /// for the first's cold scan and read its answer, not scan again.
    #[tokio::test]
    async fn concurrent_lists_share_one_cold_scan() {
        let home = tempfile::tempdir().expect("tempdir");
        let file = home.path().join("both.jsonl");
        std::fs::write(&file, format!("{}\n{}\n", said(1), said(2))).expect("write");
        let summary = row(&file, "2026-01-01T10:00:02.000Z", SessionStatus::Idle);
        let (a, b) = tokio::join!(ledger_for(None, &summary, None), ledger_for(None, &summary, None));
        assert_eq!(a, b);
        assert_eq!(a.expect("a ledger").count(None), Some(2));
        let scans = unread::COLD_SCANS.lock().expect("scan counter").get(&file).copied();
        assert_eq!(scans, Some(1), "one scan between them");
        assert!(
            !IN_FLIGHT.lock().expect("in-flight").contains_key(&summary.file_path),
            "and the turn is tidied away once nobody holds it"
        );
    }

    /// The push gate and the badge read one ledger: a turn that ended on a
    /// rate-limit notice is unread to both, at the same instant.
    #[tokio::test]
    async fn the_push_clock_is_the_badge_clock() {
        let home = tempfile::tempdir().expect("tempdir");
        let file = home.path().join("limited.jsonl");
        let notice = r#"{"type":"assistant","timestamp":"2026-01-01T10:00:09.000Z","message":{"model":"<synthetic>","content":[{"type":"text","text":"You've hit your limit"}]}}"#;
        std::fs::write(&file, format!("{}\n{notice}\n", said(1))).expect("write");
        let mut summary = row(&file, "2026-01-01T10:00:09.000Z", SessionStatus::Waiting);
        let read_at = Some("2026-01-01T10:00:05.000Z");
        let clock = clock_of(&summary, read_at).await;
        assert_eq!(clock.as_deref(), Some("2026-01-01T10:00:09.000Z"));
        let echo = derive::read_echo(Source::Claude, &file).await.expect("an echo");
        assert_eq!(echo.at, clock, "the notification's words come from the message the clock was read from");
        let ledger = ledger_for(None, &summary, read_at).await;
        fill(&mut summary, ledger.as_ref(), read_at);
        assert!(
            summary.unread && is_unread(clock.as_deref(), read_at, SessionStatus::Waiting),
            "badge and push agree"
        );
        assert_eq!(summary.unread_count, 1);
    }

    /// A map already at or under the cap is left exactly as it is.
    #[test]
    fn eviction_is_a_no_op_under_the_cap() {
        let mut cache: HashMap<String, Cached> = (0..10).map(|n| (format!("/p/{n}.jsonl"), cached("2026-01-01T00:00:00.000Z"))).collect();
        evict(&mut cache);
        assert_eq!(cache.len(), 10, "nothing is dropped while there is room");
    }
}
