//! The per-session websocket feed.
//!
//! Port of the former TypeScript server's `session-feed.ts`. One [`Feed`] per
//! `source/id`, shared by every socket watching that session; each socket gets
//! a `snapshot` on attach and then `delta` / `reset` / `state` / `draft` /
//! `closed` frames.
//!
//! # Resuming
//!
//! A transcript is an append-only file, so a position in it is a natural resume
//! marker. Every transcript frame (`snapshot`, `reset`, `delta`) carries one:
//!
//! * `endIndex` — how many items the client holds through the end of its
//!   window, in the server's numbering.
//! * `prefixFp` — the fingerprint of the server's items `[0, endIndex - K)`,
//!   with `K` = [`RESUME_OVERLAP`]. The last `K` items are left out on purpose:
//!   they are the ones the parser still rewrites after the fact (a tool result
//!   attaching, a compact summary, a command's output), and a resume re-sends
//!   them anyway.
//!
//! `snapshot` and `reset` also carry `startFp`, the fingerprint of
//! `[0, startIndex)`, which is what "load earlier" proves its window against
//! (see [`history_anchor_holds`]).
//!
//! A client that holds a window re-attaches with `?since=<endIndex>&fp=<prefixFp>`
//! ([`ResumeFrom`]). When the fingerprint still matches the server's own over
//! `[0, since - K)` and the client is not too far behind, the opening frame is a
//! `delta` from `since - K` instead of a fresh `snapshot` ([`resume_point`]).
//! Nothing per-socket goes into that decision: the fingerprint is a pure
//! function of the file's path and its parsed items, so a restarted server
//! resumes a client exactly as the one it replaced would have.
//!
//! Every `delta` has *replace-from* semantics: the client drops whatever it
//! holds at `fromIndex` and after, and puts `items` there. That is what lets a
//! live delta deliver an in-place modification of a recent item rather than
//! lose it ([`live_delta_from`] starts at the first item that changed). A
//! client sent a delta it cannot place — `fromIndex` beyond its window — never
//! drops it silently: it sends `{"type":"resync","since":…,"fp":…}` up the
//! socket and is answered with a delta that fits or a `reset`
//! ([`FeedAttachment::client_frame`]).
//!
//! The fingerprint is a chain: `fp[0]` hashes the file's path, `fp[i + 1]`
//! hashes `fp[i]` and item `i`'s JSON; 128 bits of SHA-256, base64url on the
//! wire. It is computed once per parse and kept with it ([`Transcript`]), so
//! any prefix's fingerprint is a lookup and the first difference between two
//! versions is a binary search ([`common_prefix`]).
//!
//! Two things differ structurally from the TypeScript, both forced by Rust's
//! ownership model rather than by choice:
//!
//! * Fan-out is a `tokio::sync::broadcast` of already-serialized frames instead
//!   of a `Set` of `send` closures. Per-client `delta` tailoring therefore
//!   happens in the client task (which knows what it last sent), not in the
//!   refresh loop.
//! * The watcher and the poll backstop live in one owned task that ends when the
//!   last client detaches, rather than in a mutable `Feed` record.
//!
//! # Drafts
//!
//! A claude session also has a *draft*: the count of the thinking block its
//! resident is generating, which the transcript cannot say (see
//! [`droppen_provider::drafts`]). It goes out as `{"type":"draft","turn","v","blocks"}`,
//! throttled to one frame per [`DRAFT_THROTTLE`], and is a separate thing in
//! every way that matters to the rest of this module: no index, no `sig`, no
//! fingerprint, nothing a resume or a delta is cut against. A client that does
//! not know the frame type ignores it.

pub mod global;
pub mod mux;
pub mod search;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::Drive;
use droppen_provider::drafts::Draft;
use droppen_instance::paths::{ClaudeRoot, Paths};
use droppen_sessions::account::AccountRef;
use droppen_sessions::loader::{encode_fp, Fp, Transcript, RESUME_OVERLAP};
use droppen_sessions::util::TranscriptItem;
use droppen_sessions::{file_signature, CliChoice, Injection, SessionDetail, SessionSummary, Source, TranscriptSources};
use droppen_base::watch;

/// How many items one page of transcript holds.
pub const SESSION_PAGE_SIZE: usize = 200;
/// How many items survive from the front of an over-long transcript.
pub const HEAD_KEEP: usize = 200;
/// How many items survive from the end of an over-long transcript.
pub const TAIL_KEEP: usize = 1200;
/// Past this many re-sent items a live `delta` gives way to a `reset`.
const MAX_DELTA_ITEMS: usize = 500;
/// Past this many items a resume gives way to a `snapshot`.
///
/// Ten pages. Further behind than that, the client's old window is a long way
/// from anything it will look at next, and a page is the cheaper answer.
pub const MAX_RESUME_ITEMS: usize = 2000;
/// Past this many bytes a resume gives way to a `snapshot`, whatever its item
/// count: a phone on a bad network should not be handed megabytes of tool
/// output to keep a scroll position.
pub const MAX_RESUME_BYTES: usize = 4 * 1024 * 1024;
/// How many frames the fan-out buffers before it starts lapping slow clients.
const BROADCAST_CAPACITY: usize = 64;
/// The poll backstop, for filesystem events the watcher misses.
const STATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(2000);
/// How long to coalesce filesystem events before re-reading.
const WATCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);
/// The least time between two `draft` frames.
///
/// A draft changes with every thinking chunk's token count, which during a
/// long think is several times a second.
/// A frame per change would be a render per change on every open page for a
/// number nobody reads that closely; one per 80 ms still looks live, and the
/// latest value is all a frame ever needs to carry.
pub const DRAFT_THROTTLE: std::time::Duration = std::time::Duration::from_millis(80);

/// A window onto a transcript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptPage {
    /// The items in the window.
    pub items: Vec<TranscriptItem>,
    /// Absolute index of the first item.
    pub start_index: usize,
    /// Absolute index one past the last item.
    pub end_index: usize,
    /// Total item count of the transcript.
    pub total_items: usize,
    /// Whether older items exist before `start_index`.
    pub has_more_before: bool,
}

/// The last `SESSION_PAGE_SIZE` items ending at `before`.
pub fn transcript_page(items: &[TranscriptItem], before: usize) -> TranscriptPage {
    let end_index = before.min(items.len());
    let start_index = end_index.saturating_sub(SESSION_PAGE_SIZE);
    TranscriptPage {
        items: items[start_index..end_index].to_vec(),
        start_index,
        end_index,
        total_items: items.len(),
        has_more_before: start_index > 0,
    }
}

/// Keep the head and tail of an over-long transcript, dropping the middle.
pub fn truncate_items(items: &[TranscriptItem]) -> (Vec<TranscriptItem>, usize) {
    if items.len() <= HEAD_KEEP + TAIL_KEEP {
        return (items.to_vec(), 0);
    }
    let mut kept = items[..HEAD_KEEP].to_vec();
    kept.extend_from_slice(&items[items.len() - TAIL_KEEP..]);
    let omitted = items.len() - kept.len();
    (kept, omitted)
}

/// What a client says it holds: `?since=&fp=` on attach, or the same two in a
/// `resync` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeFrom {
    /// The `endIndex` of the client's window.
    pub since: usize,
    /// The `prefixFp` it was last sent: its fingerprint of `[0, since - K)`.
    pub fp: String,
}

impl ResumeFrom {
    /// Read leniently: a malformed marker is no marker, and the client gets a
    /// fresh window rather than a refused socket it would retry forever.
    pub fn parse(since: Option<&str>, fp: Option<&str>) -> Option<Self> {
        let since = since?.trim().parse::<usize>().ok()?;
        let fp = fp?.trim();
        (!fp.is_empty() && fp.len() <= 64).then(|| Self { since, fp: fp.to_string() })
    }
}

/// Where a client holding `resume` can be resumed from, or `None` when it needs
/// a fresh window.
///
/// The decision table:
///
/// * `since - K` past the end of the transcript (the file shrank under the
///   client, or was replaced by a shorter one) — fresh.
/// * the fingerprint of `[0, since - K)` differs (an item the client holds was
///   rewritten, the file was replaced, it is another file) — fresh.
/// * more than [`MAX_RESUME_ITEMS`] items to send — fresh.
/// * otherwise — a delta from `since - K`. That includes a file that shrank by
///   no more than the overlap: the delta then ends before the client's window
///   did, and replace-from semantics truncate it.
///
/// A pure function of the chain, which is a pure function of the file: there is
/// nothing here a restart could lose.
pub fn resume_point(chain: &[Fp], resume: &ResumeFrom) -> Option<usize> {
    let len = chain.len().checked_sub(1)?;
    let at = resume.since.saturating_sub(RESUME_OVERLAP);
    let link = chain.get(at)?;
    (encode_fp(link) == resume.fp && len - at <= MAX_RESUME_ITEMS).then_some(at)
}

/// The largest `i` at which two chains agree — the two versions share items
/// `[0, i)` — or `None` when they are fingerprints of different files.
///
/// A binary search, which is sound because a link covers everything before it:
/// once two chains disagree they disagree from there on.
pub fn common_prefix(a: &[Fp], b: &[Fp]) -> Option<usize> {
    if a.is_empty() || b.is_empty() || a[0] != b[0] {
        return None;
    }
    let (mut agree, mut upper) = (0, a.len().min(b.len()) - 1);
    while agree < upper {
        let mid = agree + (upper - agree).div_ceil(2);
        if a[mid] == b[mid] {
            agree = mid;
        } else {
            upper = mid - 1;
        }
    }
    Some(agree)
}

/// Where a live `delta` from the version a client holds (`sent`) to `next`
/// starts, or `None` when a `reset` is the answer.
///
/// The first item that differs, not the old end: an item the parser rewrote in
/// place — a tool result attaching to its call — is re-sent rather than lost.
/// A reset when the versions are of different files, or when the change reaches
/// so far back that the delta would be bigger than a page several times over.
pub fn live_delta_from(sent: &[Fp], next: &[Fp]) -> Option<usize> {
    let from = common_prefix(sent, next)?;
    let len = next.len() - 1;
    (len - from <= MAX_DELTA_ITEMS).then_some(from)
}

/// Whether a client may be served the history page that ends at `before`.
///
/// `fp` is the client's fingerprint of a prefix it holds: of `[0, before)` when
/// `since` is absent (its window's `startFp`), or of `[0, since - K)` when it is
/// there (the resume marker, for a window whose start fingerprint the client no
/// longer knows because it trimmed). Either way it has to cover `[0, before)`,
/// which is all a page before `before` depends on — so a session that grew, or
/// rewrote its tail, no longer fails a page that is still exactly right.
pub fn history_anchor_holds(transcript: &Transcript, before: usize, fp: &str, since: Option<usize>) -> bool {
    let at = since.map_or(before, |since| since.saturating_sub(RESUME_OVERLAP));
    at >= before && transcript.fp_at(at).is_some_and(|ours| ours == fp)
}

/// The state half of a session, without any transcript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    /// The list row.
    pub summary: SessionSummary,
    /// A remote-hand-injected run, when one exists.
    ///
    /// This is the seam onto `provider`: the feed reads it through the
    /// [`droppen_provider::InjectionRegistry`] handle inside [`Drive`] and knows
    /// nothing else about the injection subsystem.
    pub injection: Option<Injection>,
    /// Whether the composer may send.
    pub can_send: bool,
    /// Why it may not.
    pub send_block_reason: Option<String>,
    /// Model/effort the session last actually ran with.
    pub cli_choice: Option<CliChoice>,
    /// How much of the context window the last request occupied.
    ///
    /// Carried on the state, not just the REST detail, so the ring follows a
    /// live session instead of only moving when the page is reloaded.
    pub context_tokens: Option<u64>,
    /// The window `context_tokens` is measured against.
    pub context_window: u64,
    /// Which Claude config root the session runs out of; null for codex.
    pub claude_root: Option<ClaudeRoot>,
    /// Account a managed claude session runs under.
    pub account: Option<AccountRef>,
    /// The latest automatic account rotation of a claude session, or `null`.
    ///
    /// Always present on a frame, so a client can treat an absent field as
    /// "unchanged" and `null` as "none" — the frame is the whole state.
    /// Written by the session's driver the moment a rotation is decided, and
    /// the driver pokes this feed right after, so it arrives without waiting
    /// for the next poll.
    pub rotation: Option<droppen_sessions::account::SessionRotation>,
    /// The receipts of the messages clients named for this session, from the
    /// last two hours; see [`droppen_provider::receipts`]. Always present, like
    /// `rotation`, so an absent field means a server too old to send it.
    pub receipts: Vec<droppen_sessions::delivery::MessageReceipt>,
    /// The questions the agent stopped to ask, oldest first: the one it is
    /// blocked on (`pending`), and the recent ones with what became of them —
    /// who answered, and when. See [`droppen_provider::questions`]. Always
    /// present; empty for codex, which never asks.
    pub questions: Vec<droppen_sessions::delivery::SessionQuestion>,
}

/// Read one session's live state, or `None` when it no longer exists.
pub async fn load_session_state(drive: &Arc<Drive>, source: Source, id: &str) -> Option<SessionState> {
    let paths = drive.paths.as_ref();
    let mut summary = drive.transcripts.find_summary(paths, source, id).await?;
    let injection = drive.providers.injection_for(source, id);
    let running = injection
        .as_ref()
        .map(|i| i.state == droppen_sessions::InjectionRunState::Running)
        .unwrap_or(false);
    let finished_at = injection
        .as_ref()
        .and_then(|i| {
            (i.state != droppen_sessions::InjectionRunState::Running)
                .then(|| i.finished_at.as_ref().map(String::as_str))
                .flatten()
        })
        .filter(|_| drive.transcripts.settles_reaped_runs(source));
    // The mirror whole, once: the API's half of it, and the model its process
    // runs, which the API does not carry.
    let resident_state = drive.providers.resident_state(source, id);
    let running_model = resident_state.as_ref().and_then(|state| state.model.clone());
    let resident: Option<droppen_sessions::ResidentInfo> = resident_state.map(Into::into);
    // Before `sendability`, and before `attention::apply` further down, for the
    // same reason the list applies it before both: `status` is what they read,
    // and the driver's answer is the one that is not a guess. Sendability is
    // unaffected in practice — a session with a resident is sendable whatever
    // its status says — but a rule that only holds because of a short-circuit
    // three functions away is a rule waiting to be broken.
    droppen_sessions::apply_driver(&mut summary, resident.as_ref(), running, finished_at);
    droppen_sessions::pins::apply(paths, std::slice::from_mut(&mut summary));
    // An ACP session whose agent was removed cannot be continued, whatever is
    // still running for it.
    let (can_send, send_block_reason) = match drive.providers.send_block(&summary) {
        Some(reason) => (false, Some(reason)),
        None => drive.transcripts.sendability(source, summary.scope, summary.status, running, resident.is_some()),
    };
    // One binding table, both CLIs: a codex thread started under a stored key
    // is pinned exactly as a claude session is, and the payload says so in the
    // same field.
    let account = drive.accounts.session_account_ref(id).await;
    // The transcript names the model without the `[1m]` a long-context ask
    // carries. What was asked is the process this server runs for the session
    // or, with none, what the next message will ask for (`Drive::asked_model`):
    // both readings of the model are named as that.
    let asked_model = match running_model.clone() {
        Some(model) => Some(model),
        None => drive.asked_model(source, id, account.as_ref().map(|a| a.id.as_str())).await,
    };
    let cli_choice = drive
        .transcripts
        .cli_choice(source, Path::new(&summary.file_path))
        .await
        .map(|choice| choice.run_by(asked_model.as_deref()));
    let context = drive
        .transcripts
        .context(source, Path::new(&summary.file_path))
        .await
        .map(|usage| usage.run_by(asked_model.as_deref()));
    // The CLI's own figure wins, then the model on the entry the reading came
    // from, then the reading itself; see `context::resolve_window`. With no
    // reading, or one that names no model, the process' model is the
    // session's choice.
    let context_window = droppen_sessions::context::resolve_window(
        context.as_ref(),
        running_model.as_deref().or(cli_choice.as_ref().and_then(|c| c.model.as_ref()).map(String::as_str)),
    );
    let context_tokens = context.as_ref().map(|usage| usage.tokens);
    // The list route reports the binding on the summary too; keep both payloads
    // shaped the same so a client can read it from either.
    summary.account_id = account.as_ref().map(|a| a.id.clone());
    let claude_root = drive.transcripts.detail_root(source, &summary);
    // A row is a row wherever it is served from: the list route, the detail
    // route and this feed's `state` frames must not disagree about whether a
    // session is unread, or which repository it groups under.
    // `TranscriptSources::list_and_filter` says the same for the list; this is the
    // single-session path.
    droppen_sessions::children::apply(&drive.paths, &drive.transcripts, std::slice::from_mut(&mut summary)).await;
    droppen_sessions::attention::apply(&drive.paths, &drive.transcripts, std::slice::from_mut(&mut summary)).await;
    droppen_sessions::repo::apply(std::slice::from_mut(&mut summary)).await;
    // Only claude sessions rotate, and only this instance's log knows about it.
    let rotation = match source {
        Source::Claude => drive.accounts.rotation_log().latest(id),
        Source::Codex | Source::Acp => None,
    };
    let receipts = drive.injections.host.receipts().for_session(source, id, droppen_provider::receipts::now_ms());
    let questions = if drive.providers.capabilities(source).questions {
        drive.injections.host.questions().for_session(id, droppen_provider::questions::now_ms())
    } else {
        Vec::new()
    };
    Some(SessionState {
        summary,
        injection,
        can_send,
        send_block_reason,
        cli_choice,
        context_tokens,
        context_window,
        claude_root,
        account,
        rotation,
        receipts,
        questions,
    })
}

/// How many times [`StateAndTranscript::read`] reads a session whose
/// transcript keeps moving before it serves the last read as it is.
const STATE_READS: usize = 3;

/// A session's state and transcript, of one transcript signature.
struct StateAndTranscript {
    state: SessionState,
    file_path: PathBuf,
    sig: String,
    transcript: Arc<Transcript>,
}

impl StateAndTranscript {
    /// Read a session's state and its transcript while the transcript
    /// stands still, or `None` when the session no longer exists.
    ///
    /// The state is read off the transcript too (the row, `cliChoice`, the
    /// context), so a turn written between the two reads would pair its items
    /// with the state of the turn before. The signature is taken before the
    /// state and again after the transcript; while it moved, both are read
    /// again, [`STATE_READS`] times at most. A state that names another file
    /// than the one signed (the session moved in between) has the file
    /// looked up and read again too; the last read takes the file its state
    /// names. A transcript gone by the second signature is served as read.
    async fn read(drive: &Arc<Drive>, source: Source, id: &str) -> Option<Self> {
        let mut file_path = drive.transcripts.find_file(&drive.paths, source, id).await?;
        let mut sig = file_signature(&file_path).await?;
        let mut reads = 0;
        loop {
            reads += 1;
            let last = reads == STATE_READS;
            let state = load_session_state(drive, source, id).await?;
            if Path::new(&state.summary.file_path) != file_path {
                file_path = if last {
                    PathBuf::from(&state.summary.file_path)
                } else {
                    drive.transcripts.find_file(&drive.paths, source, id).await?
                };
                sig = file_signature(&file_path).await?;
                if !last {
                    continue;
                }
            }
            let transcript = drive.transcripts.load(&drive.paths, source, &file_path, &sig).await;
            match file_signature(&file_path).await {
                Some(now) if now != sig && !last => sig = now,
                _ => {
                    return Some(Self {
                        state,
                        file_path,
                        sig,
                        transcript,
                    })
                }
            }
        }
    }
}

/// Load a session's state plus a full first page of its transcript.
pub async fn load_session_detail(drive: &Arc<Drive>, source: Source, id: &str) -> Option<SessionDetail> {
    let StateAndTranscript { state, sig, transcript, .. } = StateAndTranscript::read(drive, source, id).await?;
    let items = &transcript.items;
    // A changed signature may have rewritten an existing normalized item, so
    // only a snapshot is safe.
    let page = transcript_page(items, items.len());
    Some(SessionDetail {
        summary: state.summary,
        injection: droppen_http::openapi_ext::Nullable(state.injection),
        can_send: state.can_send,
        send_block_reason: droppen_http::openapi_ext::Nullable(state.send_block_reason),
        cli_choice: droppen_http::openapi_ext::Nullable(state.cli_choice),
        context_tokens: state.context_tokens,
        context_window: state.context_window,
        claude_root: state.claude_root,
        account: state.account,
        rotation: state.rotation,
        receipts: state.receipts,
        questions: state.questions,
        sig,
        items_changed: true,
        reset: true,
        items: page.items,
        start_index: page.start_index,
        end_index: page.end_index,
        total_items: page.total_items,
        has_more_before: page.has_more_before,
        omitted_items: page.start_index,
    })
}

/// A frame as it goes out on the wire, before per-client tailoring.
#[derive(Debug, Clone)]
enum Broadcast {
    /// The transcript changed: everyone re-derives their own delta or reset
    /// against the version they hold.
    Transcript { sig: String, transcript: Arc<Transcript> },
    /// The session's state changed.
    State(Arc<str>),
    /// The session's draft changed: a whole `draft` frame, the same for
    /// everyone, since nothing in it depends on what a client holds.
    Draft(Arc<str>),
    /// The session is gone; every socket closes.
    Closed,
}

/// The newest transcript the watcher has read, shared with every client.
///
/// The broadcast alone cannot serve a client that the buffer lapped: by
/// definition it no longer knows what it missed. This is what it resyncs from,
/// and what a client's own `resync` request is answered from.
type Latest = Arc<Mutex<(String, Arc<Transcript>)>>;

/// The last `draft` frame the watcher sent, for a client the broadcast lapped:
/// the transcript it catches up from [`Latest`], and the draft from here.
type LatestDraft = Arc<Mutex<Option<Arc<str>>>>;

/// The shared per-session watcher, alive while at least one socket is attached.
struct Feed {
    tx: broadcast::Sender<Broadcast>,
    cancel: CancellationToken,
    clients: usize,
    latest: Latest,
    latest_draft: LatestDraft,
    /// Asks the watcher to push the session's state now rather than at the
    /// next poll; see [`FeedRegistry::poke`].
    poke: Arc<tokio::sync::Notify>,
}

/// Every live feed, keyed by `source/id`.
///
/// This lives in the application state rather than in a `static`, unlike the
/// module-level `feeds` map in the former TypeScript server's
/// `session-feed.ts`. A `static` would be keyed only by session id, so two
/// servers in one process — which is exactly what the integration tests run,
/// each pointed at its own synthetic home — would share and tear down each
/// other's watchers.
#[derive(Default)]
pub struct FeedRegistry {
    feeds: Mutex<HashMap<String, Feed>>,
}

impl FeedRegistry {
    /// An empty registry, ready to be shared by every handler.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// How many feeds are currently watching. Zero once every socket detaches.
    pub fn len(&self) -> usize {
        self.feeds.lock().expect("feed registry poisoned").len()
    }

    /// Whether no feed is watching.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Cancel every live watcher and tell every socket the feed is closing.
    /// Called by the shutdown supervisor; returns how many were stopped.
    pub fn shutdown_all(&self) -> usize {
        let mut feeds = self.feeds.lock().expect("feed registry poisoned");
        let count = feeds.len();
        for feed in feeds.values() {
            feed.cancel.cancel();
            let _ = feed.tx.send(Broadcast::Closed);
        }
        feeds.clear();
        count
    }

    /// Push one session's state to its sockets now, if anybody is watching.
    ///
    /// For a change no file shows and the watcher therefore cannot see — an
    /// automatic account rotation, decided in a session's driver — which would
    /// otherwise wait for the next poll. `key` is the session's
    /// [`droppen_sessions::SessionRef::scoped_key`]. A permit is stored when the
    /// watcher is busy, so a poke is never lost, only merged with the next.
    pub fn poke(&self, key: &str) {
        let feeds = self.feeds.lock().expect("feed registry poisoned");
        if let Some(feed) = feeds.get(key) {
            feed.poke.notify_one();
        }
    }

    fn detach(&self, key: &str) {
        let mut feeds = self.feeds.lock().expect("feed registry poisoned");
        let Some(feed) = feeds.get_mut(key) else { return };
        feed.clients = feed.clients.saturating_sub(1);
        if feed.clients == 0 {
            feed.cancel.cancel();
            feeds.remove(key);
        }
    }

    /// Forget a feed whose watcher has stopped.
    ///
    /// Without this a watcher that ends on its own — the session file went away
    /// under it, so `refresh_items` gave up — would leave its entry behind, and
    /// every later attach would join a feed that can never speak again.
    ///
    /// The sender identity is what makes it safe to call late: by the time a
    /// cancelled watcher notices, the key may already have been re-taken by a
    /// fresh feed, which must not be torn down in its place.
    fn stop(&self, key: &str, tx: &broadcast::Sender<Broadcast>) {
        let mut feeds = self.feeds.lock().expect("feed registry poisoned");
        if feeds.get(key).is_some_and(|feed| feed.tx.same_channel(tx)) {
            feeds.remove(key);
        }
    }
}

/// The drivers wake a session's open views through this
/// ([`droppen_provider::SessionFeeds`]): the same call, at the same moment.
impl droppen_provider::SessionFeeds for FeedRegistry {
    fn poke(&self, key: &str) {
        FeedRegistry::poke(self, key);
    }
}

/// A socket's handle on a feed; dropping it detaches and may stop the watcher.
pub struct FeedAttachment {
    registry: Arc<FeedRegistry>,
    key: String,
    rx: broadcast::Receiver<Broadcast>,
    /// What the watcher last read, for resyncing after a lag.
    latest: Latest,
    /// The signature this client last received a page for.
    sent_sig: String,
    /// The fingerprint chain of the version this client holds, through the end
    /// of its window: what its next delta is cut against.
    ///
    /// Only for live tailoring. An attach does not need it — the client's own
    /// `since`/`fp` say what it holds — which is why a restart loses nothing.
    sent: Arc<[Fp]>,
    /// The watcher's last `draft` frame, for after a lag.
    latest_draft: LatestDraft,
    /// A frame owed to this client before anything the broadcast has: the
    /// draft a lag skipped, which follows the catch-up that replaced it.
    owed: Option<String>,
}

impl Drop for FeedAttachment {
    fn drop(&mut self) {
        self.registry.detach(&self.key);
    }
}

/// The opening frames plus the handle that yields every later one.
pub struct Attached {
    /// Frames to send before the socket starts following the feed: the
    /// `snapshot` — or, for a client that resumed, a `delta` — then a `state`
    /// replay, then the session's `draft` when it has one.
    pub opening: Vec<String>,
    /// The live handle.
    pub attachment: FeedAttachment,
}

/// Attach a socket to a session's feed, starting the watcher if this is the first.
///
/// `resume` is what the client says it already holds; see [`resume_point`] for
/// when that earns it a `delta` instead of a `snapshot`.
pub async fn attach_session_feed(
    registry: &Arc<FeedRegistry>,
    drive: &Arc<Drive>,
    source: Source,
    id: &str,
    resume: Option<&ResumeFrom>,
) -> Option<Attached> {
    attach_session_feed_with(registry, drive, source, id, resume, SESSION_PAGE_SIZE).await
}

/// [`attach_session_feed`] whose opening `snapshot`, when it has to send one,
/// holds the last `page_size` items rather than a full page.
///
/// For a client that knows how much it is about to draw: a screen of
/// transcript is a fraction of a page, and on a weak link the rest of the page
/// is the wait. Earlier items are still one `history` request away, and the
/// window is proved by the same `startFp` whatever its size. Clamped to a page.
pub async fn attach_session_feed_with(
    registry: &Arc<FeedRegistry>,
    drive: &Arc<Drive>,
    source: Source,
    id: &str,
    resume: Option<&ResumeFrom>,
    page_size: usize,
) -> Option<Attached> {
    let page_size = page_size.clamp(1, SESSION_PAGE_SIZE);
    let StateAndTranscript {
        state,
        file_path,
        sig,
        transcript,
    } = StateAndTranscript::read(drive, source, id).await?;
    let chain = transcript.chain().clone();

    // A resumed client is sent what it is missing and nothing else; its state
    // arrives in the replay below, as it would on any `state` frame.
    let opening = resume
        .and_then(|resume| resume_frame(&sig, &transcript, resume))
        .unwrap_or_else(|| snapshot_frame(&sig, &state, &transcript, page_size));
    // The registry is this process' own memory, so the key carries the server:
    // see `droppen_sessions::SessionRef::scoped_key`. Nothing here is written
    // down or sent anywhere — the frames a socket receives never contain it.
    let key = droppen_sessions::SessionRef::new(droppen_instance::identity::ServerId::local(&drive.paths), source, id).scoped_key();

    let (rx, latest, latest_draft) = {
        let mut feeds = registry.feeds.lock().expect("feed registry poisoned");
        match feeds.get_mut(&key) {
            Some(feed) => {
                feed.clients += 1;
                (feed.tx.subscribe(), feed.latest.clone(), feed.latest_draft.clone())
            }
            None => {
                // The draft is subscribed here rather than in the task, so that
                // a change made the moment this returns is one the watcher
                // sees: a receiver only hears of changes after it was made.
                // And the watcher starts out believing it sent what that
                // receiver has seen — which is what a client attaching now is
                // opened with, or older — so that nothing newer is ever held
                // back.
                let drafts = drive.providers.draft_watch(source, id);
                let sent_draft = drafts.as_ref().map(|rx| draft_frame(&rx.borrow())).unwrap_or_default();
                let (tx, rx) = broadcast::channel(BROADCAST_CAPACITY);
                let cancel = CancellationToken::new();
                let latest: Latest = Arc::new(Mutex::new((sig.clone(), transcript.clone())));
                let latest_draft: LatestDraft = Arc::default();
                let poke = Arc::new(tokio::sync::Notify::new());
                feeds.insert(
                    key.clone(),
                    Feed {
                        tx: tx.clone(),
                        cancel: cancel.clone(),
                        clients: 1,
                        latest: latest.clone(),
                        latest_draft: latest_draft.clone(),
                        poke: poke.clone(),
                    },
                );
                tokio::spawn(watch_session(
                    WatchTarget {
                        drive: drive.clone(),
                        registry: Arc::downgrade(registry),
                        key: key.clone(),
                        source,
                        id: id.to_string(),
                        file_path,
                        latest: latest.clone(),
                        latest_draft: latest_draft.clone(),
                        drafts,
                        sent_draft,
                        poke,
                    },
                    sig.clone(),
                    tx,
                    cancel,
                ));
                (rx, latest, latest_draft)
            }
        }
    };

    // The handle exists from here on, before anything else is awaited: the
    // client is counted on the feed now, and a caller that gives up on this
    // attach part-way — a subscription dropped while it was still opening —
    // must take that count back with it, which is what dropping the handle does.
    let attachment = FeedAttachment {
        registry: registry.clone(),
        key,
        rx,
        latest,
        sent_sig: sig,
        sent: chain,
        latest_draft,
        owed: None,
    };

    // State was read before the transcript was parsed, so a transition that
    // completed during a slow attach would otherwise be missed. Replaying the
    // current state closes that window.
    let replay = load_session_state(drive, source, id).await.as_ref().map(state_frame);
    // Read after subscribing, for the same reason: a draft that changes from
    // here on is on the broadcast, and one that changed before is in this.
    // Sent only when there is something in it — a client starts with none.
    let draft = drive.providers.draft_watch(source, id).and_then(|draft| {
        let draft = draft.borrow();
        (!draft.is_empty()).then(|| draft_frame(&draft))
    });

    Some(Attached {
        opening: std::iter::once(opening).chain(replay).chain(draft).collect(),
        attachment,
    })
}

impl FeedAttachment {
    /// Wait for the next frame this client should receive.
    ///
    /// Returns `None` once the feed is finished (the session went away, or the
    /// server is shutting down).
    pub async fn next_frame(&mut self) -> Option<String> {
        // Before any await, so a `select!` that drops this future between two
        // frames never loses the second.
        if let Some(frame) = self.owed.take() {
            return Some(frame);
        }
        loop {
            match self.rx.recv().await {
                Ok(Broadcast::Closed) | Err(broadcast::error::RecvError::Closed) => return None,
                // Lagged past the buffer: this client missed transcript frames,
                // and must be caught up from the watcher's latest read *now* —
                // the alternative was leaving a stale window on screen until the
                // transcript happened to change again, which for a finished
                // session is never. What it holds is still known exactly
                // (`sent`), so that is a delta wherever one is small enough.
                //
                // It may have missed a draft as well, and a draft is not caught
                // up by the next one if there is no next one — a closed turn's
                // last frame empties it. So the latest is owed, after the
                // catch-up, in the order the watcher sent the two.
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    let (sig, transcript) = self.latest.lock().expect("feed latest poisoned").clone();
                    tracing::debug!(missed, key = %self.key, "session feed client lagged; catching it up");
                    self.owed = self
                        .latest_draft
                        .lock()
                        .expect("feed latest draft poisoned")
                        .as_deref()
                        .map(str::to_owned);
                    return Some(self.catch_up(&sig, &transcript));
                }
                Ok(Broadcast::State(json)) | Ok(Broadcast::Draft(json)) => return Some(json.to_string()),
                Ok(Broadcast::Transcript { sig, transcript }) => {
                    if sig == self.sent_sig {
                        continue;
                    }
                    return Some(self.catch_up(&sig, &transcript));
                }
            }
        }
    }

    /// The frame that takes this client from the version it holds to
    /// `transcript`: a delta from the first item that differs, or a reset.
    fn catch_up(&mut self, sig: &str, transcript: &Transcript) -> String {
        let chain = transcript.chain().clone();
        let frame = match live_delta_from(&self.sent, &chain) {
            Some(from) => delta_frame(sig, Some(&self.sent_sig), transcript, from),
            None => reset_frame(sig, transcript),
        };
        self.sent_sig = sig.to_string();
        self.sent = chain;
        frame
    }

    /// Answer something the client sent up the socket, if it is anything this
    /// feed understands.
    ///
    /// One message: `{"type":"resync","since":…,"fp":…}`, sent by a client that
    /// was handed a delta it could not place. It is answered exactly as an
    /// attach would be — a delta from `since - K` when the marker still holds,
    /// otherwise a `reset` — from the watcher's latest read, and what the
    /// client holds is re-based on the answer. A `resync` with no marker asks
    /// for the `reset` outright.
    pub fn client_frame(&mut self, text: &str) -> Option<String> {
        let message: Value = serde_json::from_str(text).ok()?;
        if message.get("type").and_then(Value::as_str) != Some("resync") {
            return None;
        }
        let since = message.get("since").map(|since| match since {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        });
        let resume = ResumeFrom::parse(since.as_deref(), message.get("fp").and_then(Value::as_str));
        let (sig, transcript) = self.latest.lock().expect("feed latest poisoned").clone();
        let frame = resume
            .as_ref()
            .and_then(|resume| resume_frame(&sig, &transcript, resume))
            .unwrap_or_else(|| reset_frame(&sig, &transcript));
        self.sent_sig = sig;
        self.sent = transcript.chain().clone();
        Some(frame)
    }
}

/// The resume delta for a client holding `resume`, or `None` when it needs a
/// fresh window — see [`resume_point`] for the rule, and [`MAX_RESUME_BYTES`]
/// for the one thing it cannot see from the chain.
fn resume_frame(sig: &str, transcript: &Transcript, resume: &ResumeFrom) -> Option<String> {
    let from = resume_point(transcript.chain(), resume)?;
    let frame = delta_frame(sig, None, transcript, from);
    (frame.len() <= MAX_RESUME_BYTES).then_some(frame)
}

/// `{"type":"delta", sig, previousSig?, fromIndex, items, endIndex, totalItems, prefixFp}`.
///
/// Replace-from: the client keeps what it holds before `fromIndex` and puts
/// `items` from there, which always run to the end of the transcript.
/// `previousSig` is the signature the client was last sent, for clients that
/// still chain on it; a resume has none to name.
fn delta_frame(sig: &str, previous_sig: Option<&str>, transcript: &Transcript, from: usize) -> String {
    let len = transcript.len();
    let from = from.min(len);
    let mut object = serde_json::Map::new();
    object.insert("type".into(), Value::String("delta".into()));
    object.insert("sig".into(), Value::String(sig.into()));
    if let Some(previous_sig) = previous_sig {
        object.insert("previousSig".into(), Value::String(previous_sig.into()));
    }
    object.insert("fromIndex".into(), Value::from(from));
    object.insert(
        "items".into(),
        serde_json::to_value(&transcript.items[from..]).unwrap_or(Value::Array(vec![])),
    );
    object.insert("endIndex".into(), Value::from(len));
    object.insert("totalItems".into(), Value::from(len));
    object.insert("prefixFp".into(), Value::String(transcript.prefix_fp()));
    Value::Object(object).to_string()
}

/// The last page of `transcript`, as the fields a `snapshot` or `reset` frame
/// carries: the page itself, `omittedItems`, and the two fingerprints.
fn page_fields(object: &mut serde_json::Map<String, Value>, transcript: &Transcript) {
    page_fields_sized(object, transcript, SESSION_PAGE_SIZE);
}

/// [`page_fields`] for a page of `size` items rather than a full one.
fn page_fields_sized(object: &mut serde_json::Map<String, Value>, transcript: &Transcript, size: usize) {
    let mut page = transcript_page(&transcript.items, transcript.len());
    if page.items.len() > size {
        let cut = page.items.len() - size;
        page.items.drain(..cut);
        page.start_index += cut;
        page.has_more_before = page.start_index > 0;
    }
    let start_fp = transcript.fp_at(page.start_index).unwrap_or_default();
    if let Ok(Value::Object(page)) = serde_json::to_value(&page) {
        let start_index = page.get("startIndex").cloned().unwrap_or(Value::from(0));
        object.extend(page);
        object.insert("omittedItems".into(), start_index);
    }
    object.insert("prefixFp".into(), Value::String(transcript.prefix_fp()));
    object.insert("startFp".into(), Value::String(start_fp));
}

/// `{"type":"snapshot", sig, ...state, ...page, omittedItems, prefixFp, startFp}`
/// — the flattened shape `useSessionFeed` in
/// `web/src/features/sessions/session-detail.tsx` reads.
fn snapshot_frame(sig: &str, state: &SessionState, transcript: &Transcript, page_size: usize) -> String {
    let mut object = serde_json::Map::new();
    object.insert("type".into(), Value::String("snapshot".into()));
    object.insert("sig".into(), Value::String(sig.into()));
    if let Ok(Value::Object(state)) = serde_json::to_value(state) {
        object.extend(state);
    }
    page_fields_sized(&mut object, transcript, page_size);
    Value::Object(object).to_string()
}

/// `{"type":"reset", sig, ...page, omittedItems, prefixFp, startFp}`: the
/// client's window is replaced, whatever it held.
fn reset_frame(sig: &str, transcript: &Transcript) -> String {
    let mut object = serde_json::Map::new();
    object.insert("type".into(), Value::String("reset".into()));
    object.insert("sig".into(), Value::String(sig.into()));
    page_fields(&mut object, transcript);
    Value::Object(object).to_string()
}

fn state_frame(state: &SessionState) -> String {
    let mut object = serde_json::Map::new();
    object.insert("type".into(), Value::String("state".into()));
    if let Ok(Value::Object(state)) = serde_json::to_value(state) {
        object.extend(state);
    }
    Value::Object(object).to_string()
}

/// `{"type":"draft", turn, v, blocks}`. An empty `blocks` is a draft with no
/// think in flight, which is how a page is told the count is over.
fn draft_frame(draft: &Draft) -> String {
    let mut object = serde_json::Map::new();
    object.insert("type".into(), Value::String("draft".into()));
    if let Ok(Value::Object(draft)) = serde_json::to_value(draft) {
        object.extend(draft);
    }
    Value::Object(object).to_string()
}

/// Everything the watcher task needs to identify and serve one session.
struct WatchTarget {
    drive: Arc<Drive>,
    /// Weak so a watcher never keeps the registry (and therefore the whole
    /// application state) alive past shutdown.
    registry: std::sync::Weak<FeedRegistry>,
    key: String,
    source: Source,
    id: String,
    file_path: PathBuf,
    latest: Latest,
    latest_draft: LatestDraft,
    /// The session's draft, or `None` for a session that cannot have one.
    drafts: Option<tokio::sync::watch::Receiver<Draft>>,
    /// The `draft` frame the watcher starts out believing it has sent.
    sent_draft: String,
    /// See [`FeedRegistry::poke`].
    poke: Arc<tokio::sync::Notify>,
}

/// The one task per session that watches the file and pushes frames.
async fn watch_session(target: WatchTarget, mut last_sig: String, tx: broadcast::Sender<Broadcast>, cancel: CancellationToken) {
    let WatchTarget {
        drive,
        registry,
        key,
        source,
        id,
        file_path,
        latest,
        latest_draft,
        mut drafts,
        mut sent_draft,
        poke,
    } = target;
    let mut events = watch::watch_file(&file_path, WATCH_DEBOUNCE);
    let mut poll = tokio::time::interval(STATE_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    poll.tick().await; // the first tick is immediate
    let mut last_state_json = String::new();
    // The session's draft, for a claude session. A change arms a send at most
    // [`DRAFT_THROTTLE`] after the last one, and the send reads whatever the
    // draft is by then: the `watch` keeps only the latest value, so a burst of
    // changes is one frame without anything here having to merge them.
    let mut draft_due: Option<tokio::time::Instant> = None;
    let mut draft_sent_at: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = poll.tick() => {
                if !push_state(&drive, source, &id, &tx, &mut last_state_json).await {
                    break;
                }
            }
            _ = poke.notified() => {
                if !push_state(&drive, source, &id, &tx, &mut last_state_json).await {
                    break;
                }
                continue;
            }
            changed = draft_changed(&mut drafts), if draft_due.is_none() => {
                if !changed {
                    // The store let the channel go, which it does not while a
                    // receiver is held; stop listening rather than spin on it.
                    drafts = None;
                    continue;
                }
                let now = tokio::time::Instant::now();
                draft_due = Some(draft_sent_at.map_or(now, |at| (at + DRAFT_THROTTLE).max(now)));
                continue;
            }
            () = until(draft_due) => {
                draft_due = None;
                draft_sent_at = Some(tokio::time::Instant::now());
                let draft = drafts.as_mut().map(|rx| rx.borrow_and_update().clone()).unwrap_or_default();
                push_draft(&draft, &tx, &latest_draft, &mut sent_draft);
                continue;
            }
            event = events.recv() => {
                if event.is_none() {
                    // The watcher died; the poll backstop still refreshes.
                    events = watch::never();
                    continue;
                }
            }
        }

        match refresh_items(&drive.paths, &drive.transcripts, source, &file_path, &tx, &latest, &mut last_sig).await {
            None => break,
            Some(true) => {
                // What landed may be a steered message claude folded into its
                // running turn. Its resident takes it off the queue and pokes
                // this feed, so the state frame saying so follows the transcript
                // frame that shows it rather than a poll later.
                drive.providers.transcript_grew(source, &id);
            }
            Some(false) => {}
        }
    }

    // Whatever ended the loop, this feed can no longer produce a frame. Drop it
    // from the registry before the sockets are told, so an attach racing the
    // teardown starts a fresh watcher instead of joining this dead one.
    if let Some(registry) = registry.upgrade() {
        registry.stop(&key, &tx);
    }
    let _ = tx.send(Broadcast::Closed);
    tracing::debug!(%source, %id, "session feed watcher stopped");
}

/// Returns `false` once the session is gone and the feed should close.
async fn push_state(drive: &Arc<Drive>, source: Source, id: &str, tx: &broadcast::Sender<Broadcast>, last_state_json: &mut String) -> bool {
    let Some(state) = load_session_state(drive, source, id).await else {
        return false;
    };
    let json = state_frame(&state);
    if json == *last_state_json {
        return true;
    }
    *last_state_json = json.clone();
    let _ = tx.send(Broadcast::State(Arc::from(json.as_str())));
    true
}

/// Wait for the session's draft to change: `false` once its channel is gone,
/// never for a session that has none.
async fn draft_changed(drafts: &mut Option<tokio::sync::watch::Receiver<Draft>>) -> bool {
    match drafts {
        Some(rx) => rx.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}

/// Sleep until `at`, or for ever when there is nothing to wait for.
async fn until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Send `draft`, unless it is exactly what was sent last.
///
/// The version is part of what is compared, so a frame is only ever
/// suppressed when it is the very same draft — and never when a socket that
/// attached in between was handed something else.
fn push_draft(draft: &Draft, tx: &broadcast::Sender<Broadcast>, latest_draft: &LatestDraft, sent: &mut String) {
    let next = draft_frame(draft);
    if next == *sent {
        return;
    }
    *sent = next;
    let frame: Arc<str> = Arc::from(sent.as_str());
    *latest_draft.lock().expect("feed latest draft poisoned") = Some(frame.clone());
    let _ = tx.send(Broadcast::Draft(frame));
}

/// Read the transcript again if it changed: `Some(true)` when it did and
/// everyone has been told, `Some(false)` when it had not, and `None` once it is
/// gone and the feed should close.
async fn refresh_items(
    paths: &Paths,
    transcripts: &TranscriptSources,
    source: Source,
    file_path: &Path,
    tx: &broadcast::Sender<Broadcast>,
    latest: &Latest,
    last_sig: &mut String,
) -> Option<bool> {
    let sig = file_signature(file_path).await?;
    if sig == *last_sig {
        return Some(false);
    }
    let transcript = transcripts.load(paths, source, file_path, &sig).await;
    // Fingerprinted here, once, rather than by whichever client gets to it
    // first while the others wait on the lock. Each client then cuts its own
    // delta against the version it holds (`FeedAttachment::catch_up`): a file
    // that shrank or was rewritten shows up there as an early first
    // difference, and a different file as no common prefix at all.
    transcript.chain();
    *last_sig = sig.clone();
    *latest.lock().expect("feed latest poisoned") = (sig.clone(), transcript.clone());
    let _ = tx.send(Broadcast::Transcript { sig, transcript });
    Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use droppen_sessions::loader::fingerprint_chain;
    use droppen_sessions::util::{make_item, ItemKind, Role};

    fn items(n: usize) -> Vec<TranscriptItem> {
        (0..n)
            .map(|i| make_item(Role::User, ItemKind::Text, &format!("item {i}"), None))
            .collect()
    }

    #[test]
    fn a_page_is_the_last_two_hundred_items() {
        let all = items(250);
        let page = transcript_page(&all, all.len());
        assert_eq!(page.start_index, 50);
        assert_eq!(page.end_index, 250);
        assert_eq!(page.items.len(), SESSION_PAGE_SIZE);
        assert!(page.has_more_before);

        let earlier = transcript_page(&all, 50);
        assert_eq!((earlier.start_index, earlier.end_index), (0, 50));
        assert!(!earlier.has_more_before);
    }

    #[test]
    fn truncation_keeps_the_head_and_the_tail() {
        let (kept, omitted) = truncate_items(&items(HEAD_KEEP + TAIL_KEEP + 7));
        assert_eq!(kept.len(), HEAD_KEEP + TAIL_KEEP);
        assert_eq!(omitted, 7);
        assert_eq!(kept[0].text, "item 0");
        assert_eq!(kept[HEAD_KEEP].text, format!("item {}", 7 + HEAD_KEEP));

        let (kept, omitted) = truncate_items(&items(10));
        assert_eq!((kept.len(), omitted), (10, 0));
    }

    fn transcript_at(path: &str, items: Vec<TranscriptItem>) -> Transcript {
        Transcript::new(Path::new(path), Arc::new(items))
    }

    /// What a client holding everything through the end of `transcript` would
    /// reconnect with.
    fn marker_of(transcript: &Transcript) -> ResumeFrom {
        ResumeFrom {
            since: transcript.len(),
            fp: transcript.prefix_fp(),
        }
    }

    fn rewritten(mut all: Vec<TranscriptItem>, index: usize) -> Vec<TranscriptItem> {
        all[index].text = format!("{} (rewritten)", all[index].text);
        all
    }

    /// The resume decision, row by row.
    #[test]
    fn a_resume_is_a_delta_from_the_overlap_only_when_the_prefix_still_matches() {
        let held = transcript_at("/t/s.jsonl", items(100));
        let marker = marker_of(&held);

        // Same file, grown: a delta that re-sends the overlap.
        let grown = transcript_at("/t/s.jsonl", items(110));
        assert_eq!(resume_point(grown.chain(), &marker), Some(100 - RESUME_OVERLAP));
        // Same file, unchanged: still the overlap, so a tail item rewritten
        // while the client was away is delivered.
        let tail_rewritten = transcript_at("/t/s.jsonl", rewritten(items(100), 99));
        assert_eq!(resume_point(tail_rewritten.chain(), &marker), Some(100 - RESUME_OVERLAP));

        // An item the client holds outside the overlap was rewritten.
        let prefix_rewritten = transcript_at("/t/s.jsonl", rewritten(items(110), 50));
        assert_eq!(resume_point(prefix_rewritten.chain(), &marker), None, "fp mismatch");
        // Another file with the very same items: an adoption's copy.
        let elsewhere = transcript_at("/t/other.jsonl", items(110));
        assert_eq!(resume_point(elsewhere.chain(), &marker), None, "file replaced");
        // Shortened past the overlap: the fingerprinted prefix is gone.
        let shortened = transcript_at("/t/s.jsonl", items(90));
        assert_eq!(resume_point(shortened.chain(), &marker), None, "file shortened");
        // Shortened within it: still exactly placeable, and the delta ends
        // before the client's window did, which truncates it.
        let trimmed_tail = transcript_at("/t/s.jsonl", items(99));
        assert_eq!(resume_point(trimmed_tail.chain(), &marker), Some(100 - RESUME_OVERLAP));
        // Too far behind.
        let far = transcript_at("/t/s.jsonl", items(100 + MAX_RESUME_ITEMS));
        assert_eq!(resume_point(far.chain(), &marker), None, "too far behind");
        let just = transcript_at("/t/s.jsonl", items(100 - RESUME_OVERLAP + MAX_RESUME_ITEMS));
        assert_eq!(resume_point(just.chain(), &marker), Some(100 - RESUME_OVERLAP));
        // A garbled marker.
        let garbled = ResumeFrom {
            fp: "nope".into(),
            ..marker.clone()
        };
        assert_eq!(resume_point(grown.chain(), &garbled), None);
        // A window shorter than the overlap resumes from the very start.
        let tiny = transcript_at("/t/s.jsonl", items(2));
        assert_eq!(resume_point(grown.chain(), &marker_of(&tiny)), Some(0));
    }

    /// The marker parses leniently: bad input is no marker, never an error.
    #[test]
    fn a_malformed_resume_marker_is_no_marker() {
        assert_eq!(
            ResumeFrom::parse(Some("12"), Some("abc")),
            Some(ResumeFrom { since: 12, fp: "abc".into() })
        );
        assert_eq!(ResumeFrom::parse(Some("-1"), Some("abc")), None);
        assert_eq!(ResumeFrom::parse(Some("x"), Some("abc")), None);
        assert_eq!(ResumeFrom::parse(Some("12"), Some("")), None);
        assert_eq!(ResumeFrom::parse(Some("12"), None), None);
        assert_eq!(ResumeFrom::parse(None, Some("abc")), None);
        assert_eq!(ResumeFrom::parse(Some("12"), Some(&"a".repeat(65))), None);
    }

    /// A live delta starts at the first item that changed, not at the old end,
    /// so an item the parser rewrote in place is re-sent instead of lost.
    #[test]
    fn a_live_delta_starts_at_the_first_changed_item() {
        let sent = transcript_at("/t/s.jsonl", items(50));
        let appended = transcript_at("/t/s.jsonl", items(53));
        assert_eq!(live_delta_from(sent.chain(), appended.chain()), Some(50), "a pure append");

        let mut modified = rewritten(items(50), 49);
        modified.extend(items(53).drain(50..));
        let modified = transcript_at("/t/s.jsonl", modified);
        assert_eq!(live_delta_from(sent.chain(), modified.chain()), Some(49), "the tail item comes again");

        let shrunk = transcript_at("/t/s.jsonl", items(40));
        assert_eq!(
            live_delta_from(sent.chain(), shrunk.chain()),
            Some(40),
            "a truncation is an empty delta from the new end"
        );

        let elsewhere = transcript_at("/t/other.jsonl", items(53));
        assert_eq!(live_delta_from(sent.chain(), elsewhere.chain()), None, "another file is a reset");

        let deep = transcript_at("/t/s.jsonl", rewritten(items(50 + MAX_DELTA_ITEMS), 2));
        assert_eq!(live_delta_from(sent.chain(), deep.chain()), None, "a change too far back is a reset");
    }

    #[test]
    fn the_common_prefix_is_exact_at_every_split() {
        let base = items(37);
        for split in 0..=37 {
            let mut other = base.clone();
            if split < 37 {
                other[split].text.push('!');
            }
            let a = fingerprint_chain(Path::new("/t/s.jsonl"), &base);
            let b = fingerprint_chain(Path::new("/t/s.jsonl"), &other);
            assert_eq!(common_prefix(&a, &b), Some(split), "split at {split}");
            assert_eq!(common_prefix(&a, &b[..=split.min(20)]), Some(split.min(20)));
        }
    }

    /// The frame a live client is handed for a modified tail item: the item
    /// again, then what was appended, with the new resume marker.
    #[tokio::test]
    async fn a_live_delta_with_a_modified_tail_item_carries_it_again() {
        let registry = FeedRegistry::new();
        let (tx, rx) = broadcast::channel(8);
        let held = transcript_at("/t/live.jsonl", items(10));
        let latest: Latest = Arc::new(Mutex::new(("sig-1".to_string(), Arc::new(transcript_at("/t/live.jsonl", items(10))))));
        let mut attachment = FeedAttachment {
            registry,
            key: "claude/live".to_string(),
            rx,
            latest,
            sent_sig: "sig-1".to_string(),
            sent: held.chain().clone(),
            latest_draft: Arc::default(),
            owed: None,
        };

        let mut next = rewritten(items(10), 9);
        next.extend(items(12).drain(10..));
        let next = Arc::new(transcript_at("/t/live.jsonl", next));
        tx.send(Broadcast::Transcript {
            sig: "sig-2".to_string(),
            transcript: next.clone(),
        })
        .expect("send");

        let frame: Value = serde_json::from_str(&attachment.next_frame().await.expect("a frame")).expect("json");
        assert_eq!(frame["type"], "delta", "{frame}");
        assert_eq!(frame["fromIndex"], 9);
        assert_eq!(frame["items"][0]["text"], "item 9 (rewritten)");
        assert_eq!(frame["items"].as_array().map(Vec::len), Some(3));
        assert_eq!(frame["endIndex"], 12);
        assert_eq!(frame["totalItems"], 12);
        assert_eq!(frame["previousSig"], "sig-1");
        assert_eq!(frame["prefixFp"], next.prefix_fp());
        assert_eq!(attachment.sent.len(), 13, "the client now holds the new version");
    }

    /// History paging checks the prefix it depends on and nothing else.
    #[test]
    fn history_tolerates_growth_but_not_a_changed_prefix() {
        let held = transcript_at("/t/h.jsonl", items(300));
        let start_fp = held.fp_at(100).expect("in range");
        let marker = marker_of(&held);

        let grown = transcript_at("/t/h.jsonl", items(350));
        assert!(history_anchor_holds(&grown, 100, &start_fp, None), "the session grew");
        let tail_rewritten = transcript_at("/t/h.jsonl", rewritten(items(350), 299));
        assert!(history_anchor_holds(&tail_rewritten, 100, &start_fp, None));
        let window_rewritten = transcript_at("/t/h.jsonl", rewritten(items(350), 150));
        assert!(
            history_anchor_holds(&window_rewritten, 100, &start_fp, None),
            "a change inside the client's window is the socket's business, not the page's"
        );
        let prefix_rewritten = transcript_at("/t/h.jsonl", rewritten(items(350), 50));
        assert!(
            !history_anchor_holds(&prefix_rewritten, 100, &start_fp, None),
            "the page's own prefix changed"
        );
        let shrunk = transcript_at("/t/h.jsonl", items(80));
        assert!(!history_anchor_holds(&shrunk, 100, &start_fp, None));

        // The resume marker as the anchor, for a window that trimmed its start.
        assert!(history_anchor_holds(&grown, 100, &marker.fp, Some(marker.since)));
        assert!(!history_anchor_holds(&prefix_rewritten, 100, &marker.fp, Some(marker.since)));
        assert!(
            !history_anchor_holds(&grown, marker.since, &marker.fp, Some(marker.since)),
            "a marker that does not reach `before` proves nothing about it"
        );
    }

    fn lapped_attachment(latest: &Latest, held: &Transcript) -> (broadcast::Sender<Broadcast>, FeedAttachment) {
        let (tx, rx) = broadcast::channel(2);
        let attachment = FeedAttachment {
            registry: FeedRegistry::new(),
            key: "claude/lapped".to_string(),
            rx,
            latest: latest.clone(),
            sent_sig: "sig-1".to_string(),
            sent: held.chain().clone(),
            latest_draft: Arc::default(),
            owed: None,
        };
        (tx, attachment)
    }

    /// A client the broadcast buffer lapped missed frames, and waiting for the
    /// next transcript change to catch it up means waiting forever on a session
    /// that has finished. What it holds is still known exactly, so that is a
    /// delta when it fits and a reset when it does not.
    #[tokio::test]
    async fn a_lapped_client_is_caught_up_at_once_rather_than_on_the_next_change() {
        let held = transcript_at("/t/lapped.jsonl", items(3));
        let latest: Latest = Arc::new(Mutex::new(("sig-1".to_string(), Arc::new(transcript_at("/t/lapped.jsonl", items(3))))));
        let (tx, mut attachment) = lapped_attachment(&latest, &held);

        // Five frames through a two-frame buffer: the receiver is lapped.
        for n in 0..5 {
            let _ = tx.send(Broadcast::State(Arc::from(format!(r#"{{"type":"state","n":{n}}}"#).as_str())));
        }
        // Meanwhile the watcher has read a longer transcript.
        *latest.lock().expect("latest") = ("sig-2".to_string(), Arc::new(transcript_at("/t/lapped.jsonl", items(7))));

        let frame = attachment.next_frame().await.expect("a frame");
        let value: Value = serde_json::from_str(&frame).expect("json");
        assert_eq!(value["type"], "delta", "what it missed is known, got {frame}");
        assert_eq!(value["sig"], "sig-2");
        assert_eq!(value["fromIndex"], 3);
        assert_eq!(value["endIndex"], 7);
        assert_eq!(attachment.sent.len(), 8, "the client is now up to date");

        // And when the file under it was replaced, nothing it holds is reusable.
        let (tx, mut attachment) = lapped_attachment(&latest, &held);
        for n in 0..5 {
            let _ = tx.send(Broadcast::State(Arc::from(format!(r#"{{"type":"state","n":{n}}}"#).as_str())));
        }
        *latest.lock().expect("latest") = ("sig-3".to_string(), Arc::new(transcript_at("/t/replaced.jsonl", items(7))));
        let value: Value = serde_json::from_str(&attachment.next_frame().await.expect("a frame")).expect("json");
        assert_eq!(value["type"], "reset", "{value}");
        assert_eq!(value["totalItems"], 7);
        assert!(value["startFp"].is_string() && value["prefixFp"].is_string(), "{value}");
    }

    /// A client handed a delta it could not place asks again over the socket;
    /// the answer is a delta that fits when its marker holds, a reset otherwise.
    #[tokio::test]
    async fn a_resync_request_is_answered_with_a_fitting_delta_or_a_reset() {
        let client_holds = transcript_at("/t/resync.jsonl", items(20));
        let latest: Latest = Arc::new(Mutex::new(("sig-9".to_string(), Arc::new(transcript_at("/t/resync.jsonl", items(25))))));
        // What the server *thinks* the client holds is wrong — that is the
        // whole reason it asked — so it must not matter.
        let (_tx, mut attachment) = lapped_attachment(&latest, &transcript_at("/t/resync.jsonl", items(24)));

        let marker = marker_of(&client_holds);
        let request = serde_json::json!({"type": "resync", "since": marker.since, "fp": marker.fp}).to_string();
        let value: Value = serde_json::from_str(&attachment.client_frame(&request).expect("answered")).expect("json");
        assert_eq!(value["type"], "delta", "{value}");
        assert_eq!(value["fromIndex"], 20 - RESUME_OVERLAP);
        assert_eq!(value["endIndex"], 25);
        assert_eq!(attachment.sent_sig, "sig-9");
        assert_eq!(attachment.sent.len(), 26, "re-based on the answer");

        let stale = serde_json::json!({"type": "resync", "since": 20, "fp": "stale"}).to_string();
        let value: Value = serde_json::from_str(&attachment.client_frame(&stale).expect("answered")).expect("json");
        assert_eq!(value["type"], "reset", "{value}");

        let bare = r#"{"type":"resync"}"#;
        let value: Value = serde_json::from_str(&attachment.client_frame(bare).expect("answered")).expect("json");
        assert_eq!(value["type"], "reset", "no marker asks for a fresh window");

        assert_eq!(attachment.client_frame(r#"{"type":"hello"}"#), None, "anything else is ignored");
        assert_eq!(attachment.client_frame("not json"), None);
    }

    /// A watcher that gives up must not leave its entry behind: the next attach
    /// would subscribe to a sender nobody is holding any more.
    #[tokio::test]
    async fn a_stopped_watcher_forgets_its_registry_entry() {
        let registry = FeedRegistry::new();
        let (tx, _rx) = broadcast::channel(4);
        let key = "claude/gone".to_string();
        registry.feeds.lock().expect("registry").insert(
            key.clone(),
            Feed {
                tx: tx.clone(),
                cancel: CancellationToken::new(),
                clients: 1,
                latest: Arc::new(Mutex::new((String::new(), Arc::new(transcript_at("/t/gone.jsonl", vec![]))))),
                latest_draft: Arc::default(),
                poke: Arc::new(tokio::sync::Notify::new()),
            },
        );

        registry.stop(&key, &tx);
        assert!(registry.is_empty(), "the dead feed must be gone");

        // A feed that re-took the key in the meantime is left alone.
        let (fresh, _rx) = broadcast::channel(4);
        registry.feeds.lock().expect("registry").insert(
            key.clone(),
            Feed {
                tx: fresh.clone(),
                cancel: CancellationToken::new(),
                clients: 1,
                latest: Arc::new(Mutex::new((String::new(), Arc::new(transcript_at("/t/gone.jsonl", vec![]))))),
                latest_draft: Arc::default(),
                poke: Arc::new(tokio::sync::Notify::new()),
            },
        );
        registry.stop(&key, &tx);
        assert_eq!(registry.len(), 1, "a late teardown must not take the replacement with it");
    }

    #[test]
    fn a_fingerprint_covers_exactly_its_prefix() {
        let all = items(5);
        let path = Path::new("/t/fp.jsonl");
        let chain = fingerprint_chain(path, &all);
        assert_eq!(chain.len(), 6, "one link per prefix, the empty one included");
        assert_eq!(chain[3], fingerprint_chain(path, &all[..3])[3]);
        assert_ne!(chain[3], chain[4]);
        assert_ne!(
            chain[3],
            fingerprint_chain(Path::new("/t/other.jsonl"), &all)[3],
            "the file is part of it"
        );
        assert_eq!(encode_fp(&chain[3]).len(), 22, "short enough for a query string");
    }

    /// The `thinking_tokens` line claude prints with each chunk of a think.
    fn thinking(tokens: u64) -> Value {
        serde_json::json!({"type": "system", "subtype": "thinking_tokens", "estimated_tokens": tokens})
    }

    fn append(file: &Path, text: &str) {
        let mut handle = std::fs::OpenOptions::new().append(true).open(file).expect("open");
        std::io::Write::write_all(&mut handle, text.as_bytes()).expect("append");
    }

    /// A draft rides beside the transcript and is never part of it: the frame
    /// carries none of the transcript's markers, and the window a client holds
    /// — what its next delta is cut against — is exactly what it was.
    #[tokio::test]
    async fn a_draft_frame_never_moves_the_window_a_client_holds() {
        let held = transcript_at("/t/drafted.jsonl", items(10));
        let latest: Latest = Arc::new(Mutex::new(("sig-1".to_string(), Arc::new(transcript_at("/t/drafted.jsonl", items(10))))));
        let (tx, mut attachment) = lapped_attachment(&latest, &held);

        let mut draft = Draft::default();
        droppen_provider::drafts::apply(&mut draft, &thinking(42));
        let frame = draft_frame(&draft);
        tx.send(Broadcast::Draft(Arc::from(frame.as_str()))).expect("send");
        let got: Value = serde_json::from_str(&attachment.next_frame().await.expect("a frame")).expect("json");
        assert_eq!(got["type"], "draft");
        assert_eq!(got["blocks"], serde_json::json!([{"index": 0, "kind": "thinking", "thinkingTokens": 42}]));
        for marker in ["sig", "endIndex", "totalItems", "prefixFp", "startFp", "items", "fromIndex"] {
            assert!(got.get(marker).is_none(), "a draft must not carry `{marker}`: {got}");
        }
        assert_eq!(attachment.sent_sig, "sig-1");
        assert_eq!(attachment.sent.len(), 11, "the window held is untouched");

        // The next transcript frame is the delta it would have been anyway.
        let next = Arc::new(transcript_at("/t/drafted.jsonl", items(12)));
        tx.send(Broadcast::Transcript {
            sig: "sig-2".to_string(),
            transcript: next.clone(),
        })
        .expect("send");
        let delta: Value = serde_json::from_str(&attachment.next_frame().await.expect("a frame")).expect("json");
        assert_eq!(delta["type"], "delta");
        assert_eq!(
            (delta["fromIndex"].clone(), delta["endIndex"].clone()),
            (Value::from(10), Value::from(12))
        );
        assert_eq!(delta["prefixFp"], next.prefix_fp());
        assert_eq!(delta["previousSig"], "sig-1");
    }

    /// A lapped client is owed the draft as well as the transcript, after it.
    #[tokio::test]
    async fn a_lapped_client_gets_the_latest_draft_after_its_catch_up() {
        let held = transcript_at("/t/lapped-draft.jsonl", items(3));
        let latest: Latest = Arc::new(Mutex::new((
            "sig-1".to_string(),
            Arc::new(transcript_at("/t/lapped-draft.jsonl", items(5))),
        )));
        let (tx, mut attachment) = lapped_attachment(&latest, &held);
        *attachment.latest_draft.lock().expect("latest draft") = Some(Arc::from(r#"{"type":"draft","turn":1,"v":7,"blocks":[]}"#));
        for n in 0..5 {
            let _ = tx.send(Broadcast::State(Arc::from(format!(r#"{{"type":"state","n":{n}}}"#).as_str())));
        }
        let first: Value = serde_json::from_str(&attachment.next_frame().await.expect("a frame")).expect("json");
        assert_eq!(first["type"], "delta", "the transcript first: {first}");
        let second: Value = serde_json::from_str(&attachment.next_frame().await.expect("a frame")).expect("json");
        assert_eq!((second["type"].clone(), second["v"].clone()), (Value::from("draft"), Value::from(7)));
    }

    /// Transcript frames are the items and their markers, and nothing beside
    /// them for a draft to be matched against: the count has no entry to wait
    /// for.
    #[tokio::test]
    async fn transcript_frames_name_no_agent_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = "55555555-5555-4555-8555-555555555555";
        let (paths, drive) = crate::fake::drive(dir.path());
        let file = crate::fake::session(&paths, id, "write the module\n");
        append(&file, "said\n");
        let sig = file_signature(&file).await.expect("sig");
        let transcript = drive.transcripts.load(&drive.paths, crate::fake::SOURCE, &file, &sig).await;
        let delta: Value = serde_json::from_str(&delta_frame(&sig, None, &transcript, 0)).expect("json");
        let reset: Value = serde_json::from_str(&reset_frame(&sig, &transcript)).expect("json");
        for frame in [&delta, &reset] {
            assert!(frame.get("agentUuids").is_none(), "{frame}");
        }
        assert_eq!(delta["prefixFp"], transcript.prefix_fp());
    }

    /// A turn written while a session's state is read is not served beside
    /// the state of the turn before: the detail and the feed's opening
    /// snapshot each read one transcript's state and items.
    #[tokio::test]
    async fn the_state_and_the_items_are_of_one_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = "66666666-6666-4666-8666-666666666666";
        let (paths, drive) = crate::fake::drive(dir.path());
        crate::fake::session(&paths, id, "the first turn\n");
        crate::fake::writes_next(&paths, id, "the second turn\n");
        let detail = load_session_detail(&drive, crate::fake::SOURCE, id).await.expect("detail");
        let model = detail.cli_choice.0.and_then(|choice| choice.model.0);
        assert_eq!((model.as_deref(), detail.items.len()), (Some("the second turn"), 2), "the detail");

        crate::fake::writes_next(&paths, id, "the third turn\n");
        let registry = FeedRegistry::new();
        let attached = attach_session_feed_with(&registry, &drive, crate::fake::SOURCE, id, None, SESSION_PAGE_SIZE)
            .await
            .expect("attached");
        let snapshot: Value = serde_json::from_str(&attached.opening[0]).expect("json");
        assert_eq!(
            (snapshot["cliChoice"]["model"].as_str(), snapshot["items"].as_array().map(Vec::len)),
            (Some("the third turn"), Some(3)),
            "the snapshot"
        );
    }
}
