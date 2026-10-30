//! Claude Code session discovery.
//!
//! Port of the discovery half of the former TypeScript server's `claude.ts`: scan
//! `~/.claude/projects/*/<uuid>.jsonl`, derive a title/cwd/branch from the head
//! (plus the newest `custom-title` in the tail), then overlay live-process
//! status from `~/.claude/sessions/*.json`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use futures_util::future::BoxFuture;
use serde_json::Value;

use droppen_sessions::context::ContextUsage;
use droppen_sessions::sources::{TokenUsage, UsageFile, UsageFold};
use super::transcript::{content_blocks, user_title_candidate};
use droppen_sessions::util::{self, read_head_lines, read_tail_lines, title_line, try_parse, FileCache};
use droppen_sessions::{SessionActivity, SessionScope, SessionStatus, SessionSummary, Source};
use crate::provider::claude_code::claude_roots;
use droppen_http::openapi_ext::Nullable;
use droppen_instance::paths::{ClaudeRoot, Paths};
use droppen_base::time::{iso8601, mtime_ms};

pub mod adoption_leftovers;
pub mod archive;
pub mod continuation;
pub mod tool_display;

const HEAD_BYTES: usize = 128 * 1024;
const TAIL_BYTES: u64 = 64 * 1024;
/// How far back [`claude_activity_status`] will look for an entry that decides
/// the session's status before giving up and calling it `Running`.
///
/// Generous, because it is only ever reached by a transcript whose whole tail is
/// tool traffic — and cheap, because reaching it at all is rare enough that the
/// budget is a backstop against a pathological file, not a normal cost.
const STATUS_SCAN_BUDGET: u64 = 8 * 1024 * 1024;

/// Whose a claude session is, as far as the root it lives in can say.
///
/// The managed root belongs to this instance and nothing else can be in it, so a
/// transcript's directory settles the common case with no bookkeeping. It cannot
/// settle the other one: a session started on the *local login* is deliberately
/// written to the user's own `~/.claude`, and is still this server's. Those are
/// promoted afterwards, from the registry, by [`mark_owned`].
pub fn scope_of(root: ClaudeRoot) -> SessionScope {
    match root {
        ClaudeRoot::Managed => SessionScope::Owned,
        ClaudeRoot::Local => SessionScope::External,
    }
}

/// Promote the rows this instance's own registry claims, and name the server.
///
/// One read of the registry per listing rather than one per row. It runs after
/// [`summarize_file`] rather than inside it because that answer is cached
/// against the transcript's mtime, and ownership can change — a local session
/// recorded as ours, or its row pruned at boot — without the file moving at all.
/// `server` is filled in here for the same reason: a rename moves it, and the
/// transcript does not.
///
/// Every row gets a server, including the ones this instance does not own —
/// see [`SessionSummary::server`], which says why the two are different claims.
/// So there is no early return on an empty registry any more: the rows still
/// have to be told which server is answering for them.
pub(super) async fn mark_owned(paths: &Paths, summaries: &mut [SessionSummary]) {
    let local = droppen_instance::identity::ServerId::local(paths);
    let owned = droppen_sessions::owned_sessions::read(paths).await;
    for summary in summaries {
        match owned.server_of(Source::Claude, &summary.id) {
            Some(server) => {
                summary.server = server.to_string();
                summary.scope = SessionScope::Owned;
            }
            // Either a transcript in this instance's managed root — owned by the
            // directory it sits in, with no registry row to read — or somebody
            // else's session this server merely found. Both are reached through
            // this server, and that is what the field says.
            None => summary.server = local.to_string(),
        }
    }
}

/// The overlays a listing and a single-row lookup both apply to Claude rows.
trait FinishExt {
    /// Apply live status, the user's own title and ownership.
    ///
    /// None of them is in the cached row, because each can change without the
    /// transcript moving at all — which is why the list and the detail view must
    /// both run this, or a detail that disagreed with the row the user clicked
    /// would read as a different session.
    async fn finish(&mut self, paths: &Paths);
}

impl FinishExt for [SessionSummary] {
    async fn finish(&mut self, paths: &Paths) {
        if self.is_empty() {
            return;
        }
        let live = live_claude_session_ids(paths).await;
        let titles = droppen_sessions::custom_titles(paths, Source::Claude).await;
        for summary in self.iter_mut() {
            if live.contains(&summary.id) {
                let activity = claude_activity(Path::new(&summary.file_path)).await;
                (summary.status, summary.asking) = (activity.status, activity.asking);
            }
            if let Some(custom) = titles.get(&summary.id) {
                summary.title = custom.clone();
            }
        }
        mark_owned(paths, self).await;
    }
}

/// How many transcripts the in-memory tier holds before it is emptied.
///
/// There was no bound at all, and that was a leak rather than a simplification:
/// nothing ever dropped the entry for a transcript that had since been archived,
/// deleted or renamed, so a server that lists every few seconds for a fortnight
/// accumulated a summary for every session it had ever seen. The cap is far
/// above the corpus — 70 transcripts on the machine this was measured on — so
/// the working set never reaches it.
///
/// Emptied wholesale rather than evicted one at a time, following [`droppen_sessions::repo`]
/// which made the same trade for the same reason: an LRU needs a second
/// structure to maintain, and being wrong costs a re-read. It costs less than it
/// used to, at that — a cleared entry is now found again in the durable index by
/// a `stat`, not by re-parsing 192 KiB of transcript.
const CACHE_MAX: usize = 2048;

/// Summaries keyed by transcript path, invalidated by mtime and size — the same
/// module-level cache the TypeScript keeps, so a list refresh only re-reads the
/// files that actually changed.
///
/// The near tier of two. This one is a `HashMap` lookup and dies with the
/// process; behind it sits [`droppen_sessions::index`], which is what stops a restart
/// costing 11.2 MB of head and tail parsing before the first list appears.
static LIST_CACHE: LazyLock<FileCache<SessionSummary>> = LazyLock::new(|| FileCache::new(CACHE_MAX));

/// A summary as `index.db` stored it, read with tolerance for the fields added
/// since.
///
/// A row is stored *before* the overlays run, so every field an overlay fills
/// in is at its default in the payload anyway — which is what makes defaulting
/// one that a previous build never wrote correct rather than merely convenient.
/// `unreadCount` is the one such field that is required on the wire; without
/// this, every summary cached before it existed would fail to parse and the
/// first list after an upgrade would re-read every transcript's head and tail.
fn from_index(mut payload: serde_json::Value) -> Option<SessionSummary> {
    if let Some(row) = payload.as_object_mut() {
        row.entry("unreadCount").or_insert(serde_json::Value::from(0));
    }
    serde_json::from_value(payload).ok()
}

/// Read one transcript's head (and `custom-title` tail) into a list row.
pub async fn summarize_file(file_path: &Path, id: &str, root: ClaudeRoot) -> std::io::Result<SessionSummary> {
    let metadata = tokio::fs::metadata(file_path).await?;
    let mtime = mtime_ms(&metadata);
    let mark = droppen_sessions::index::Watermark::of(file_path, &metadata);
    // Size as well as mtime, so the near tier agrees with the durable one
    // about what "unchanged" means.
    if let Some(summary) = LIST_CACHE.get(file_path, mtime, metadata.len()) {
        return Ok(summary);
    }

    // What some previous boot worked out about this exact file. The watermark is
    // the entire check: same path, same length, same mtime, or it is a miss and
    // the transcript gets read like there was no index at all.
    let index = droppen_sessions::index::current();
    if let Some(summary) = index
        .as_ref()
        .and_then(|index| index.summary::<serde_json::Value>(Source::Claude, id, mark))
        .and_then(from_index)
        // The root is an input to the answer — it decides `scope` — and it is
        // the one input that is not the file, so the watermark cannot vouch for
        // it. A row that was written under the other root is refused rather than
        // trusted: an external session that came back from a restart marked
        // owned would be offered a send button it must not have.
        .filter(|summary: &SessionSummary| summary.claude_root == Some(root))
    {
        LIST_CACHE.put(file_path, mtime, summary.size_bytes, summary.clone());
        return Ok(summary);
    }

    let lines = read_head_lines(file_path, HEAD_BYTES).await?;
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut title: Option<String> = None;
    let mut summary_title: Option<String> = None;
    let mut fallback_title: Option<String> = None;
    let mut custom_title: Option<String> = None;
    let mut created_at: Option<String> = None;

    for line in &lines {
        let Some(entry) = try_parse(line) else { continue };
        // The session's own idea of when it began, which survives copying the
        // file around in a way the filesystem's birth time does not.
        if created_at.is_none() {
            created_at = entry
                .get("timestamp")
                .and_then(Value::as_str)
                .filter(|stamp| !stamp.is_empty())
                .map(str::to_owned);
        }
        if cwd.is_none() {
            cwd = entry.get("cwd").and_then(Value::as_str).map(str::to_owned);
        }
        if git_branch.is_none() {
            git_branch = entry
                .get("gitBranch")
                .and_then(Value::as_str)
                .filter(|b| !b.is_empty())
                .map(str::to_owned);
        }
        let entry_type = entry.get("type").and_then(Value::as_str);
        if entry_type == Some("custom-title") {
            if let Some(value) = entry.get("customTitle").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                custom_title = Some(value.to_string());
            }
        }
        if summary_title.is_none() && entry_type == Some("summary") {
            summary_title = entry.get("summary").and_then(Value::as_str).map(str::to_owned);
        }
        if title.is_none() {
            title = user_title_candidate(&entry);
        }
        if fallback_title.is_none() && entry_type == Some("queue-operation") {
            fallback_title = entry
                .get("content")
                .and_then(Value::as_str)
                .filter(|c| !c.trim().is_empty())
                .map(str::to_owned);
        }
    }

    // Claude Code appends its generated/edited session name near the end of the
    // file; the newest custom-title line wins over anything found in the head.
    // An unreadable tail leaves the head-derived titles in place.
    if let Ok(tail) = read_tail_lines(file_path, TAIL_BYTES).await {
        for line in tail.iter().rev() {
            if !line.contains("\"custom-title\"") {
                continue;
            }
            let Some(entry) = try_parse(line) else { continue };
            if entry.get("type").and_then(Value::as_str) != Some("custom-title") {
                continue;
            }
            if let Some(value) = entry.get("customTitle").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                custom_title = Some(value.to_string());
                break;
            }
        }
    }

    let picked = custom_title.or(summary_title).or(title).or(fallback_title);
    let summary = SessionSummary {
        pinned: false,
        id: id.to_string(),
        source: Source::Claude,
        scope: scope_of(root),
        // Both overlays, applied by `mark_owned` on the copy that goes out: the
        // row cached here is keyed on the transcript, and neither whose the
        // session is nor what this server is called is written in the file.
        server: String::new(),
        title: picked.as_deref().map(title_line).unwrap_or_else(|| "(untitled)".to_string()),
        cwd: Nullable(cwd),
        // Filled in by `droppen_sessions::repo::apply`, which spawns one `git`
        // per distinct directory rather than one per row.
        repo_key: Nullable(None),
        git_branch: Nullable(git_branch),
        updated_at: metadata.modified().map(iso8601).unwrap_or_default(),
        // An undated transcript falls back to the file's own birth time, and to
        // its mtime on a filesystem that does not keep one.
        created_at: created_at.unwrap_or_else(|| metadata.created().or_else(|_| metadata.modified()).map(iso8601).unwrap_or_default()),
        size_bytes: metadata.len(),
        file_path: file_path.to_string_lossy().into_owned(),
        status: SessionStatus::Idle,
        // Both refined by `droppen_sessions::DriverStates::apply` for the
        // sessions this server drives; a row nobody drives keeps these.
        activity: SessionActivity::Idle,
        resident: Nullable(None),
        claude_root: Some(root),
        acp_agent: None,
        account_id: None,
        // Filled in by `droppen_sessions::attention::apply`, which memoises the transcript
        // read this cache would otherwise repeat for it.
        attention_at: Nullable(None),
        unread: false,
        unread_count: 0,
        // Live status, applied by the callers below with `status`.
        asking: false,
        children: Vec::new(),
        parent_id: Nullable(None),
    };
    LIST_CACHE.put(file_path, mtime, summary.size_bytes, summary.clone());
    // Both tiers hold the row *before* the overlays — ownership, live status,
    // the user's own title, the account binding. Those are applied to the copy
    // that goes out, by the callers below, because each of them can change
    // without the transcript being touched at all.
    if let Some(index) = index {
        index.remember_summary(Source::Claude, id, mark, &summary);
    }
    Ok(summary)
}

/// Session ids that have a live Claude Code process attached right now.
///
/// Both roots are scanned: a managed session registers its pid under the managed
/// root, and a client that cannot see it would happily send into a live CLI.
pub async fn live_claude_session_ids(paths: &Paths) -> HashSet<String> {
    let mut out = HashSet::new();
    for root in ClaudeRoot::ALL {
        collect_live_ids(&claude_roots::ClaudeRoots::of(paths).live_dir(root), &mut out).await;
    }
    out
}

async fn collect_live_ids(dir: &Path, out: &mut HashSet<String>) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_name().to_string_lossy().ends_with(".json") {
            continue;
        }
        // Stale or unreadable registration: skip it.
        let Ok(raw) = tokio::fs::read_to_string(entry.path()).await else {
            continue;
        };
        let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let (Some(pid), Some(session_id)) = (data.get("pid").and_then(Value::as_i64), data.get("sessionId").and_then(Value::as_str)) else {
            continue;
        };
        // `exists`, not `alive`: the pid is Claude Code's own, not a child of
        // this server, so pid 1 (Claude as a container's init) counts.
        if u32::try_from(pid as i32).is_ok_and(droppen_base::procs::exists) {
            out.insert(session_id.to_string());
        }
    }
}

/// What one transcript entry says about whether the session is still working.
///
/// `None` is the important value: the entry says *nothing*, and the walk goes on
/// past it however large it was. Everything the CLI writes that is not the
/// conversation itself lands here — `queue-operation`, `attachment`,
/// `last-prompt`, `ai-title`, `system` lines, a subagent's sidechain, and a
/// `user` entry that only carries tool results.
fn decisive(entry: &serde_json::Map<String, Value>) -> Option<SessionStatus> {
    if entry.get("isSidechain") == Some(&Value::Bool(true)) {
        return None;
    }
    match entry.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            let blocks = content_blocks(entry.get("message"));
            let last = blocks.last()?;
            Some(match last.get("type").and_then(Value::as_str) {
                Some("text") => SessionStatus::Waiting,
                Some("tool_use") if last.get("name").and_then(Value::as_str) == Some("AskUserQuestion") => SessionStatus::Waiting,
                _ => SessionStatus::Running,
            })
        }
        // A compact writes its summary as a `user` entry, so the plain
        // "someone spoke last" rule reads a finished compact as a turn
        // starting and pins the session to Running with nothing running.
        Some("user") if entry.get("isMeta") != Some(&Value::Bool(true)) && entry.get("isCompactSummary") != Some(&Value::Bool(true)) => {
            // A tool's result is not a person speaking. It always follows the
            // `tool_use` that asked for it, which decides the same way, so
            // skipping it changes no answer — except that these are the entries
            // that reach 1.35 MB, and stepping over them is what lets the walk
            // reach the assistant text above.
            let blocks = content_blocks(entry.get("message"));
            if !blocks.is_empty() && blocks.iter().all(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")) {
                return None;
            }
            Some(SessionStatus::Running)
        }
        _ => None,
    }
}

/// The tool calls a `user` entry answers: the `tool_use_id` of each
/// `tool_result` it carries.
fn answered_calls(entry: &serde_json::Map<String, Value>) -> impl Iterator<Item = String> {
    let blocks = match entry.get("type").and_then(Value::as_str) {
        Some("user") => content_blocks(entry.get("message")),
        _ => Vec::new(),
    };
    blocks
        .into_iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|block| block.get("tool_use_id").and_then(Value::as_str).map(str::to_owned))
}

/// Whether a deciding entry is a question the user has not answered yet: an
/// `AskUserQuestion` call as its last block, with no result for it among the
/// entries after it.
///
/// [`decisive`] reads a question as a wait whether or not it was answered,
/// because it steps over the `tool_result` that answers it; `answered` is what
/// that walk stepped over.
fn unanswered_question(entry: &serde_json::Map<String, Value>, answered: &HashSet<String>) -> bool {
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return false;
    }
    let blocks = content_blocks(entry.get("message"));
    let Some(last) = blocks.last() else { return false };
    last.get("type").and_then(Value::as_str) == Some("tool_use")
        && last.get("name").and_then(Value::as_str) == Some("AskUserQuestion")
        && !last.get("id").and_then(Value::as_str).is_some_and(|id| answered.contains(id))
}

/// What a live session's transcript says: [`SessionStatus`], and whether the
/// agent stopped to ask the user something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeActivity {
    /// Waiting or running, as [`claude_activity_status`] has always said.
    pub status: SessionStatus,
    /// The turn ended on an `AskUserQuestion` nobody has answered: the one wait
    /// that is genuinely on the user. A turn that ended on text is `Waiting`
    /// too, and is not this — a resident keeps every finished session's
    /// process alive, so `Waiting` alone is true of nearly all of them.
    pub asking: bool,
}

/// For a live session: waiting for user input, or still working?
///
/// [`claude_activity`]'s status alone, for the callers that ask nothing else.
pub async fn claude_activity_status(file_path: &Path) -> SessionStatus {
    claude_activity(file_path).await.status
}

/// For a live session: waiting for user input, or still working — and if
/// waiting, on a question?
///
/// Walks the transcript backwards to the first entry that settles it rather than
/// over a fixed tail window. The window was 64 KiB, and the entries the CLI
/// writes around a queued message are not small: two `queue-operation` entries
/// of 30–44 KB evict everything decisive from it, at which point a session that
/// was plainly working fell through to `Waiting` and wore the "input?" badge.
/// Measured at 37 minutes of that in one real session — one stretch of 24 —
/// triggered precisely by the user queueing messages.
///
/// The cost is unchanged for the common case: the first read is the same 64 KiB
/// as before and holds the deciding entry in every ordinary session, so the
/// five-second poll over every live claude session pays exactly what it did.
/// Only a transcript whose tail is genuinely all indecisive pays for more, in
/// doubling chunks, and [`STATUS_SCAN_BUDGET`] is where that stops.
pub async fn claude_activity(file_path: &Path) -> ClaudeActivity {
    let settled = |status| ClaudeActivity { status, asking: false };
    let Ok(mut scan) = util::BackwardLines::open(file_path, STATUS_SCAN_BUDGET).await else {
        return settled(SessionStatus::Running);
    };
    // The results the walk has stepped over: what tells an answered question
    // from an open one once the walk reaches it.
    let mut answered = HashSet::new();
    loop {
        match scan.next_line().await {
            Ok(Some(line)) => {
                let Some(entry) = try_parse(&line) else { continue };
                if let Some(status) = decisive(&entry) {
                    let asking = status == SessionStatus::Waiting && unanswered_question(&entry, &answered);
                    return ClaudeActivity { status, asking };
                }
                answered.extend(answered_calls(&entry));
            }
            // Megabytes of transcript with nothing decisive in them is heavy
            // tool traffic, not an idle session: claiming it wants input is the
            // one direction that actually costs the user something.
            Ok(None) if scan.capped() => {
                tracing::debug!(path = %file_path.display(), "no decisive transcript entry within the status scan budget");
                return settled(SessionStatus::Running);
            }
            // The whole file, and nobody has spoken: nothing is running.
            Ok(None) => return settled(SessionStatus::Waiting),
            Err(_) => return settled(SessionStatus::Running),
        }
    }
}

/// Ids the user archived out of the list.
pub async fn archived_ids(paths: &Paths) -> HashSet<String> {
    archive::archived_ids(paths)
}

/// Every `<uuid>.jsonl` under one root's projects directory.
async fn root_transcripts(paths: &Paths, root: ClaudeRoot) -> Vec<(PathBuf, String)> {
    let Ok(mut project_dirs) = tokio::fs::read_dir(claude_roots::ClaudeRoots::of(paths).projects_dir(root)).await else {
        return vec![];
    };
    let mut files = Vec::new();
    while let Ok(Some(project)) = project_dirs.next_entry().await {
        let Ok(mut entries) = tokio::fs::read_dir(project.path()).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".jsonl") else { continue };
            files.push((entry.path(), id.to_string()));
        }
    }
    files
}

/// Every non-archived Claude session in either root, with live-process status
/// and account bindings applied.
///
/// `bindings` is the `sessionAccounts` map from the account store; it is passed
/// in rather than read here so session discovery keeps no dependency on the
/// account subsystem's caching.
pub async fn list_claude_sessions(paths: &Paths, bindings: &BTreeMap<String, String>) -> Vec<SessionSummary> {
    // [`summarize_file`] takes a path and no instance, because a transcript is a
    // file on this machine and belongs to nobody here. This is where the index
    // it writes into is named — on the way past, by the caller that does hold a
    // `Paths`. See [`droppen_sessions::index::current`].
    droppen_sessions::index::install(paths);
    let archived = archived_ids(paths).await;

    // Ids are uuids, so a collision across roots means one file was copied by
    // hand. The managed copy wins: it is the one remote-hand knows how to spawn.
    let mut by_id: HashMap<String, SessionSummary> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for root in ClaudeRoot::ALL {
        for (path, id) in root_transcripts(paths, root).await {
            if archived.contains(&id) {
                continue;
            }
            let Ok(summary) = summarize_file(&path, &id, root).await else { continue };
            if let Some(previous) = by_id.get(&id) {
                warn_duplicate(&id, &previous.file_path, &summary.file_path);
            } else {
                order.push(id.clone());
            }
            by_id.insert(id, summary);
        }
    }

    let mut summaries: Vec<SessionSummary> = order.into_iter().filter_map(|id| by_id.remove(&id)).collect();
    for summary in &mut summaries {
        summary.account_id = bindings.get(&summary.id).cloned();
    }
    summaries.finish(paths).await;
    summaries
}

/// Ids already reported as living in both roots, so the warning stays a one-off.
static WARNED_DUPLICATES: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn warn_duplicate(id: &str, previous: &str, winner: &str) {
    // The global feed re-lists every few seconds; one line per id is plenty.
    if WARNED_DUPLICATES.lock().expect("duplicate warnings poisoned").insert(id.to_string()) {
        tracing::warn!(%id, %winner, %previous, "claude session exists in both roots; using the managed copy");
    }
}

/// A located transcript and the config root it belongs to.
#[derive(Debug, Clone)]
pub struct ClaudeSessionFile {
    /// Absolute path of the transcript.
    pub file_path: PathBuf,
    /// Which root it was found in.
    pub root: ClaudeRoot,
}

/// Locate a session's transcript by id, managed root first.
///
/// Anything that is not a plain file name is rejected outright, so an id can
/// never walk out of the projects directory. Managed wins ties for the same
/// reason it wins in the list: it is the copy remote-hand knows how to spawn.
pub async fn find_claude_session(paths: &Paths, id: &str) -> Option<ClaudeSessionFile> {
    if archived_ids(paths).await.contains(id) {
        return None;
    }
    for root in [ClaudeRoot::Managed, ClaudeRoot::Local] {
        if let Some(file_path) = find_in_root(paths, root, id).await {
            return Some(ClaudeSessionFile { file_path, root });
        }
    }
    None
}

/// Whether an id can name a transcript file at all.
///
/// Anything that is not a plain file name is rejected outright, so an id can
/// never walk out of a projects directory. Dots are tolerated *inside* a name —
/// the CLIs have never minted one, but a lookup that refused would 404 a session
/// the user can see — while a name made only of dots is refused, because
/// `<project>/<id>` for `id = ".."` is the projects directory itself. That is
/// not a hypothetical: everything that strips `.jsonl` off a located path and
/// treats the remainder as this session's own directory would then be handed a
/// directory holding every session there is.
///
/// An id that becomes a *directory* of its own is held to the stricter
/// [`droppen_sessions::reference::is_path_segment_id`] instead.
pub fn is_session_id(id: &str) -> bool {
    !id.is_empty() && !id.bytes().all(|b| b == b'.') && id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// A session's transcript in one *named* root, ignoring the other.
///
/// The archive list is deliberately not consulted: this answers "is the file
/// there", which is what the registry's boot prune needs. Discovery layers the
/// hiding rule on top.
pub async fn find_in_root(paths: &Paths, root: ClaudeRoot, id: &str) -> Option<PathBuf> {
    if !is_session_id(id) {
        return None;
    }
    let mut project_dirs = tokio::fs::read_dir(claude_roots::ClaudeRoots::of(paths).projects_dir(root)).await.ok()?;
    while let Ok(Some(project)) = project_dirs.next_entry().await {
        let candidate = project.path().join(format!("{id}.jsonl"));
        if tokio::fs::metadata(&candidate).await.is_ok() {
            return Some(candidate);
        }
    }
    None
}

/// Just the path of [`find_claude_session`].
pub async fn find_claude_session_file(paths: &Paths, id: &str) -> Option<PathBuf> {
    Some(find_claude_session(paths, id).await?.file_path)
}

/// One session's row, with live status applied, or `None` when it is gone.
pub async fn load_claude_summary(paths: &Paths, id: &str) -> Option<SessionSummary> {
    droppen_sessions::index::install(paths);
    let found = find_claude_session(paths, id).await?;
    let mut summary = summarize_file(&found.file_path, id, found.root).await.ok()?;
    std::slice::from_mut(&mut summary).finish(paths).await;
    Some(summary)
}

/// Claude Code's transcripts, for [`droppen_sessions::TranscriptSources`]: the sessions in
/// both roots' projects directories, and the parser of their main chain.
pub struct ClaudeCodeSessions;

impl ClaudeCodeSessions {
    /// Record that this instance created claude session `id`, in its ownership
    /// registry ([`droppen_sessions::owned_sessions::OwnedSessions::record`]): for a session
    /// whose transcript lands in a root this server does not own.
    pub async fn record_owned(&self, paths: &Paths, id: &str) -> std::io::Result<()> {
        droppen_sessions::owned_sessions::OwnedSessions::record(paths, self, id).await
    }

    /// Drop claude entries whose transcript is not in any root this server reads.
    ///
    /// Two things put one there. A claude id is recorded *before* the spawn, so a
    /// launch that failed leaves an entry for a session that never existed; and a
    /// transcript the user deletes leaves one for a session that no longer does.
    /// Neither is harmful — nothing lists an id with no file — but the registry is
    /// the one piece of ownership state that cannot be rebuilt from the filesystem,
    /// so it should not accumulate forever either.
    ///
    /// **Boot only, and under the instance lock**, rather than on every load: the
    /// check is a directory scan per id, and the read path runs once per list
    /// refresh for every attached client. There is a narrow race — a server
    /// restarted in the moment between a claude id being recorded and its CLI
    /// writing the first line would prune it — and the cost of losing it is that the
    /// session lists as external: still readable here, and still the user's own to
    /// continue with `claude --resume`, but no longer sendable from this server.
    ///
    /// Codex entries are deliberately left alone. `~/.codex` hides archived and
    /// subagent threads from every lookup cheap enough to run here, so "not found"
    /// there does not mean "gone", and disowning a thread the user merely archived
    /// would be worse than keeping a stale line.
    pub async fn prune_owned(&self, paths: &Paths) {
        let owned = droppen_sessions::owned_sessions::read(paths).await;
        let ids: Vec<String> = owned.ids(Source::Claude).map(str::to_owned).collect();
        if ids.is_empty() {
            return;
        }
        let mut stale: Vec<String> = Vec::new();
        for id in ids {
            let found = find_in_root(paths, ClaudeRoot::Managed, &id).await.is_some() || find_in_root(paths, ClaudeRoot::Local, &id).await.is_some();
            if !found {
                stale.push(id);
            }
        }
        if stale.is_empty() {
            return;
        }
        for id in &stale {
            if let Err(error) = droppen_sessions::owned_sessions::forget(paths, Source::Claude, id).await {
                tracing::warn!(session = %id, %error, "cannot drop a stale ownership entry");
                return;
            }
        }
        tracing::info!(
            dropped = stale.len(),
            "dropped ownership entries for claude sessions that no longer exist"
        );
    }
}

/// What a Claude root's `projects` directory says about the sessions in it.
trait ProjectsDirExt {
    /// Whether it holds a session transcript (`<project>/<id>.jsonl`).
    fn has_transcript(&self) -> bool;
}

impl ProjectsDirExt for Path {
    fn has_transcript(&self) -> bool {
        let Ok(projects) = std::fs::read_dir(self) else {
            return false;
        };
        projects.flatten().any(|project| {
            std::fs::read_dir(project.path()).is_ok_and(|mut files| {
                files.any(|file| file.is_ok_and(|file| file.path().extension().is_some_and(|ext| ext == "jsonl")))
            })
        })
    }
}

impl droppen_sessions::Transcripts for ClaudeCodeSessions {
    fn source(&self) -> Source {
        Source::Claude
    }

    fn list<'a>(&'a self, paths: &'a Paths, bindings: &'a BTreeMap<String, String>) -> BoxFuture<'a, Vec<SessionSummary>> {
        Box::pin(list_claude_sessions(paths, bindings))
    }

    /// Any transcript in the instance's own Claude root, without reading one.
    fn has_own_sessions<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, bool> {
        let projects = claude_roots::ClaudeRoots::of(paths).projects_dir(ClaudeRoot::Managed);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || projects.has_transcript())
                .await
                .unwrap_or(true)
        })
    }

    fn find<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<SessionSummary>> {
        Box::pin(load_claude_summary(paths, id))
    }

    // Found without building the row: the row summarizes the file and applies
    // the live overlay, and reading the transcript needs neither.
    fn find_file<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<PathBuf>> {
        Box::pin(find_claude_session_file(paths, id))
    }

    // Managed first and archived hidden, as every lookup by id finds it.
    fn find_root<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<ClaudeRoot>> {
        Box::pin(async move { Some(find_claude_session(paths, id).await?.root) })
    }

    // The main chain only: a subagent's sidechain is read as its own transcript.
    fn parse<'a>(&'a self, paths: &'a Paths, file_path: &'a Path) -> BoxFuture<'a, std::io::Result<Vec<util::TranscriptItem>>> {
        Box::pin(super::transcript::parse_claude_transcript(paths, file_path, false))
    }

    fn cli_choice<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<droppen_sessions::CliChoice>> {
        Box::pin(read_cli_choice(file_path))
    }

    fn external_reason(&self) -> String {
        "this session was not started by this instance; continue it where it was started".to_string()
    }

    // The live claude the probe sees may be one the user has open in their own
    // terminal, whatever it is doing.
    fn live_block(&self, status: SessionStatus) -> Option<String> {
        (status != SessionStatus::Idle).then(|| "attached to a live local Claude Code process".into())
    }

    // A session with no recorded root predates multi-root discovery, so it can
    // only be in the local one.
    fn detail_root(&self, summary: &SessionSummary) -> Option<ClaudeRoot> {
        Some(summary.claude_root.unwrap_or(ClaudeRoot::Local))
    }

    fn foreign_sessions(&self) -> bool {
        true
    }

    // The row is all a claude session needs: a sibling lists one on the local
    // login as external, which is what it is to that sibling.
    fn on_owned<'a>(&'a self, _paths: &'a Paths, _id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(std::future::ready(()))
    }

    // Both roots' projects directories, made first: on a fresh install the
    // managed root does not exist yet, and without a watch on it account
    // sessions would wait for a restart. An unwritable home falls back to the
    // poll interval.
    fn watch_dirs<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, Vec<PathBuf>> {
        Box::pin(async move {
            let dirs: Vec<PathBuf> = claude_roots::ClaudeRoots::of(paths).projects_dirs().into_iter().map(|(_, dir)| dir).collect();
            for dir in &dirs {
                let _ = tokio::fs::create_dir_all(dir).await;
            }
            dirs
        })
    }

    fn subagents(&self) -> Option<&dyn droppen_sessions::sidecars::Subagents> {
        Some(self)
    }

    fn checklists(&self) -> Option<&dyn droppen_sessions::sidecars::Checklists> {
        Some(self)
    }

    fn children<'a>(&'a self, _paths: &'a Paths, sessions: Vec<&'a mut SessionSummary>, marks: &'a BTreeMap<String, String>) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            for session in sessions {
                session.children = self.subagent_tree(session, marks).await;
            }
        })
    }

    fn context<'a>(&'a self, file_path: &'a Path) -> BoxFuture<'a, Option<droppen_sessions::context::ContextUsage>> {
        Box::pin(read_claude_context(file_path))
    }

    // Each non-empty `text` block of a main-chain `assistant` entry.
    // `isSidechain` is a subagent talking to itself, and `thinking` / `tool_use`
    // are the model working. A `<synthetic>` entry — the CLI writing in the
    // model's voice: a rate-limit wall, an API error — *does* count: it is the
    // last thing the session said, and a session that stopped on a limit is
    // exactly one the user needs to see has stopped.
    fn spoken_texts(&self, entry: &serde_json::Map<String, Value>) -> Vec<String> {
        let keep = |text: &str| {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        };
        if entry.get("isSidechain") == Some(&Value::Bool(true)) || entry.get("type").and_then(Value::as_str) != Some("assistant") {
            return Vec::new();
        }
        content_blocks(entry.get("message"))
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str).and_then(keep))
            .collect()
    }

    // Claude keeps one flat `<uuid>.jsonl` per session under each project dir, and
    // both config roots contribute: usage is per machine, not per login.
    fn usage_files<'a>(&'a self, paths: &'a Paths, cutoff_ms: f64) -> BoxFuture<'a, Vec<UsageFile>> {
        Box::pin(async move {
            let mut out = Vec::new();
            for (_, projects) in crate::provider::claude_code::claude_roots::ClaudeRoots::of(&paths).projects_dirs() {
                let Ok(mut roots) = tokio::fs::read_dir(&projects).await else { continue };
                while let Ok(Some(project)) = roots.next_entry().await {
                    let Ok(mut files) = tokio::fs::read_dir(project.path()).await else {
                        continue;
                    };
                    while let Ok(Some(file)) = files.next_entry().await {
                        let path = file.path();
                        if !util::is_jsonl(&path) {
                            continue;
                        }
                        if let Some(reference) = UsageFile::stat(path, cutoff_ms).await {
                            out.push(reference);
                        }
                    }
                }
            }
            out
        })
    }

    fn usage_fold(&self) -> Box<dyn UsageFold> {
        Box::new(ClaudeUsage)
    }
}

/// Claude assistant entries carry per-message `message.usage` + `message.model`.
struct ClaudeUsage;

impl UsageFold for ClaudeUsage {
    fn fold(&mut self, entry: &serde_json::Map<String, Value>) -> Option<TokenUsage> {
        if entry.get("type").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        let Some(message) = entry.get("message").and_then(Value::as_object) else {
            return None;
        };
        let Some(usage) = message.get("usage").and_then(Value::as_object) else {
            return None;
        };
        let Some(date) = util::date_of(entry) else { return None };
        let model = message.get("model").and_then(Value::as_str).unwrap_or("unknown");
        let delta = TokenUsage {
            date,
            model: model.to_string(),
            input: util::num(usage.get("input_tokens")),
            output: util::num(usage.get("output_tokens")),
            cache_read: util::num(usage.get("cache_read_input_tokens")),
            cache_write: util::num(usage.get("cache_creation_input_tokens")),
        };
        if delta.total() == 0.0 {
            return None;
        }
        Some(delta)
    }
}

/// The model/effort a session last ran with, read from its transcript tail.
pub async fn read_cli_choice(file_path: &Path) -> Option<droppen_sessions::CliChoice> {
    // Claude writes model+effort on every assistant entry, so a small tail is enough.
    droppen_sessions::CliChoice::read_tail(file_path, 128 * 1024, |entry| {
        if entry.get("type").and_then(Value::as_str) != Some("assistant") || entry.get("isSidechain") == Some(&Value::Bool(true)) {
            return None;
        }
        let model = entry
            .get("message")
            .and_then(Value::as_object)
            .and_then(|message| message.get("model"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let effort = entry.get("effort").and_then(Value::as_str).map(str::to_owned);
        (model.is_some() || effort.is_some()).then_some((model, effort))
    })
    .await
}

/// Claude's occupancy: the last main-chain assistant entry that carries a usage.
///
/// Everything the request carried *in* counts — fresh input, what was replayed
/// out of the cache, and what was written into it — because all three occupy the
/// window. Sidechain entries are a subagent's own context, not this session's.
async fn read_claude_context(file_path: &Path) -> Option<ContextUsage> {
    let lines = read_tail_lines(file_path, droppen_sessions::context::TAIL_BYTES).await.ok()?;
    for line in lines.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        if entry.get("type").and_then(Value::as_str) != Some("assistant") || entry.get("isSidechain") == Some(&Value::Bool(true)) {
            continue;
        }
        let Some(message) = entry.get("message").and_then(Value::as_object) else {
            continue;
        };
        let Some(usage) = message.get("usage").and_then(Value::as_object) else {
            continue;
        };
        let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let tokens = field("input_tokens") + field("cache_read_input_tokens") + field("cache_creation_input_tokens");
        if tokens > 0 {
            let model = message.get("model").and_then(Value::as_str).map(str::to_owned);
            return Some(ContextUsage { tokens, window: None, model });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use droppen_sessions::context::{context_window, resolve_window, DEFAULT_CONTEXT_WINDOW, LARGE_CONTEXT_WINDOW};

    /// A claude id is recorded *before* its spawn, so a launch that failed
    /// leaves an entry for a session that never existed — and a transcript the
    /// user deletes leaves one for a session that no longer does.
    #[tokio::test]
    async fn the_prune_drops_claude_ids_with_no_transcript_and_keeps_the_rest() {
        use crate::provider::claude_code::claude_roots;
        use droppen_sessions::owned_sessions;

        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        let project = claude_roots::ClaudeRoots::of(&paths).projects_dir(ClaudeRoot::Local).join("-w");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(project.join("alive.jsonl"), "{}\n").expect("a session that exists");

        ClaudeCodeSessions.record_owned(&paths, "alive").await.expect("record");
        ClaudeCodeSessions.record_owned(&paths, "never-launched").await.expect("record");
        // Codex is left alone whatever the filesystem says: `~/.codex` hides
        // archived threads from every lookup cheap enough to run at boot, so
        // "not found" there is not "gone".
        crate::provider::codex::sessions::codex::CodexSessions.record_owned(&paths, "archived-thread").await.expect("record");

        ClaudeCodeSessions.prune_owned(&paths).await;

        let owned = owned_sessions::read(&paths).await;
        assert!(owned.contains(Source::Claude, "alive"));
        assert!(!owned.contains(Source::Claude, "never-launched"), "the failed launch is dropped");
        assert!(owned.contains(Source::Codex, "archived-thread"), "codex entries are never pruned");

        // Idempotent, and a no-op on a registry with nothing to drop.
        ClaudeCodeSessions.prune_owned(&paths).await;
        assert_eq!(owned_sessions::read(&paths).await, owned);
    }

    #[test]
    fn a_transcript_is_a_jsonl_file_in_a_project() {
        let root = tempfile::tempdir().expect("tempdir");
        let projects = root.path().join("projects");
        assert!(!projects.has_transcript(), "no root yet");
        std::fs::create_dir_all(projects.join("-w")).expect("project dir");
        std::fs::write(projects.join("-w/notes.txt"), "x").expect("write");
        std::fs::write(projects.join("stray.jsonl"), "x").expect("write");
        assert!(!projects.has_transcript(), "an empty project, a stray file");
        std::fs::write(projects.join("-w/1c2d3e4f-5061-4723-9b8c-0d1e2f3a4b5c.jsonl"), "{}").expect("write");
        assert!(projects.has_transcript());
    }

    fn session(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
        (dir, path)
    }

    #[tokio::test]
    async fn a_custom_title_in_the_tail_outranks_everything_in_the_head() {
        let (_dir, path) = session(&[
            r#"{"type":"summary","summary":"from summary","cwd":"/w","gitBranch":"main"}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"first message"}]}}"#,
            r#"{"type":"custom-title","customTitle":"renamed later"}"#,
        ]);
        let summary = summarize_file(&path, "abc", ClaudeRoot::Local).await.expect("summarize");
        assert_eq!(summary.title, "renamed later");
        assert_eq!(summary.cwd.as_ref().map(String::as_str), Some("/w"));
        assert_eq!(summary.git_branch.as_ref().map(String::as_str), Some("main"));
        assert_eq!(summary.status, SessionStatus::Idle);
    }

    #[tokio::test]
    async fn the_first_user_message_is_the_fallback_title() {
        let (_dir, path) = session(&[
            r#"{"type":"user","isMeta":true,"message":{"content":[{"type":"text","text":"meta noise"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"<system-reminder>x</system-reminder>"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"real question\nsecond line"}]}}"#,
        ]);
        let summary = summarize_file(&path, "abc", ClaudeRoot::Local).await.expect("summarize");
        assert_eq!(summary.title, "real question", "only the first line, and never an XML wrapper");
    }

    #[tokio::test]
    async fn an_empty_transcript_is_untitled() {
        let (_dir, path) = session(&[]);
        assert_eq!(
            summarize_file(&path, "abc", ClaudeRoot::Local).await.expect("summarize").title,
            "(untitled)"
        );
    }

    /// `Waiting` is every finished turn of a live process, and a resident keeps
    /// every finished session's process alive — so only a question nobody has
    /// answered is the agent actually asking. The reported bug was the amber
    /// "waiting for you" on every session that had stopped.
    #[tokio::test]
    async fn only_an_unanswered_question_is_asking() {
        let ask =
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_q","name":"AskUserQuestion","input":{"questions":[]}}]}}"#;
        let answer = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_q","content":"The user answered: a"}]}}"#;

        let (_dir, finished) = session(&[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#]);
        assert_eq!(
            claude_activity(&finished).await,
            ClaudeActivity {
                status: SessionStatus::Waiting,
                asking: false
            },
            "a finished turn waits, and asks nothing"
        );

        let (_dir2, open) = session(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"one thing first"}]}}"#,
            ask,
        ]);
        assert_eq!(
            claude_activity(&open).await,
            ClaudeActivity {
                status: SessionStatus::Waiting,
                asking: true
            }
        );

        // Answered, and the model not yet back: `status` reads as it always has,
        // but the question is no longer open.
        let (_dir3, answered) = session(&[ask, answer]);
        assert!(!claude_activity(&answered).await.asking, "the result answers it");

        let (_dir4, moved_on) = session(&[
            ask,
            answer,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"going with a"}]}}"#,
        ]);
        assert_eq!(
            claude_activity(&moved_on).await,
            ClaudeActivity {
                status: SessionStatus::Waiting,
                asking: false
            }
        );

        // A result for some other call is not an answer to this one, and what
        // the CLI writes around a waiting question decides nothing.
        let (_dir5, still_open) = session(&[
            ask,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_other","content":"x"}]}}"#,
            r#"{"type":"queue-operation","operation":"add","content":"later"}"#,
        ]);
        assert!(claude_activity(&still_open).await.asking);

        let (_dir6, working) = session(&[
            ask,
            answer,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_b","name":"Bash","input":{}}]}}"#,
        ]);
        assert_eq!(
            claude_activity(&working).await,
            ClaudeActivity {
                status: SessionStatus::Running,
                asking: false
            }
        );
    }

    #[tokio::test]
    async fn status_is_waiting_after_assistant_text_and_running_mid_tool() {
        let (_dir, waiting) = session(&[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#]);
        assert_eq!(claude_activity_status(&waiting).await, SessionStatus::Waiting);

        let (_dir2, running) = session(&[r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#]);
        assert_eq!(claude_activity_status(&running).await, SessionStatus::Running);

        let (_dir3, asked) = session(&[r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{}}]}}"#]);
        assert_eq!(
            claude_activity_status(&asked).await,
            SessionStatus::Waiting,
            "a question is a wait, not work"
        );

        let (_dir4, replied) = session(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"next"}]}}"#,
        ]);
        assert_eq!(
            claude_activity_status(&replied).await,
            SessionStatus::Running,
            "a fresh user turn means work is starting"
        );
    }

    /// Fixture lines from the real compacted session `76f842ae` (2026-08-30T07:20).
    #[tokio::test]
    async fn a_finished_compact_is_a_wait_and_a_typed_command_is_work() {
        let (_dir, compacted) = session(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"v0.2.3 shipped"}]}}"#,
            r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted","isMeta":false}"#,
            r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":"This session is being continued from a previous conversation."}}"#,
        ]);
        assert_eq!(
            claude_activity_status(&compacted).await,
            SessionStatus::Waiting,
            "the compact wrote its own summary; nobody asked for anything"
        );

        let (_dir2, commanded) = session(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>"}}"#,
        ]);
        assert_eq!(
            claude_activity_status(&commanded).await,
            SessionStatus::Running,
            "a command the user typed is still a turn starting"
        );
    }

    /// Fixture lines from the real session `3c002577` (GhostLens, 2026-08-30T16:54).
    /// The transcript now renders an absorbed steer as a user message, but status
    /// reads the raw file: the steer was answered inside the turn it interrupted,
    /// so the finished turn is a wait, not a fresh turn starting.
    #[tokio::test]
    async fn a_steer_absorbed_into_a_finished_turn_is_not_work() {
        let (_dir, steered) = session(&[
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"text":"所以结论呢？","type":"text"}],"commandMode":"prompt"}}"#,
            r#"{"type":"queue-operation","operation":"remove","timestamp":"2026-08-30T16:55:13.648Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"结论一句话"}]}}"#,
        ]);
        assert_eq!(
            claude_activity_status(&steered).await,
            SessionStatus::Waiting,
            "the turn that absorbed the steer already answered it"
        );

        let (_dir2, mid_turn) = session(&[
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":[{"text":"所以结论呢？","type":"text"}],"commandMode":"prompt"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
        ]);
        assert_eq!(
            claude_activity_status(&mid_turn).await,
            SessionStatus::Running,
            "the turn carrying the steer is still working"
        );
    }

    /// One `queue-operation` entry in the real session `3c002577` is 30–44 KB,
    /// and two of them are more than the 64 KiB window the status heuristic used
    /// to read. The session was working; the badge said it wanted input. 37
    /// minutes of that in one session, one stretch of it 24 minutes long.
    #[tokio::test]
    async fn queued_messages_cannot_evict_the_entry_that_decides_status() {
        let bulky = |n: usize| format!(r#"{{"type":"queue-operation","operation":"add","content":"{}"}}"#, "咕".repeat(n));
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            &bulky(14_000),
            &bulky(14_000),
        ]);
        assert_eq!(
            claude_activity_status(&path).await,
            SessionStatus::Running,
            "the queue entries say nothing; the tool_use above them says everything"
        );
    }

    /// The other size that broke the window, from the other direction: a single
    /// `tool_result` entry of 1.35 MB, which is a wait mistaken for work.
    #[tokio::test]
    async fn a_huge_tool_result_does_not_hide_the_answer_above_it() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"here is the answer"}]}}"#,
            &format!(
                r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"{}"}}]}}}}"#,
                "z".repeat(1_350_000)
            ),
        ]);
        assert_eq!(
            claude_activity_status(&path).await,
            SessionStatus::Waiting,
            "a tool's result is not a person speaking"
        );
    }

    /// Megabytes of tail with nothing decisive in it is a session doing heavy
    /// tool work, so the budget's fallthrough is `Running`. Under-report the
    /// need for attention; never invent it.
    #[tokio::test]
    async fn an_indecisive_tail_past_the_budget_reads_as_working() {
        let filler: Vec<String> = (0..9)
            .map(|n| {
                format!(
                    r#"{{"type":"queue-operation","operation":"add","n":{n},"content":"{}"}}"#,
                    "q".repeat(1_048_576)
                )
            })
            .collect();
        let mut lines = vec![r#"{"type":"assistant","message":{"content":[{"type":"text","text":"finished"}]}}"#.to_string()];
        lines.extend(filler);
        let (_dir, path) = session(&lines.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(
            claude_activity_status(&path).await,
            SessionStatus::Running,
            "the assistant text is past the budget, and guessing 'waiting' is the harmful guess"
        );
    }

    #[tokio::test]
    async fn sidechain_entries_do_not_decide_status() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent said something"}]}}"#,
        ]);
        assert_eq!(claude_activity_status(&path).await, SessionStatus::Running);
    }

    #[tokio::test]
    async fn a_traversing_id_never_resolves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        assert!(find_claude_session(&paths, "../../etc/passwd").await.is_none());
        assert!(find_claude_session(&paths, "").await.is_none());
    }

    /// A name made only of dots is the projects directory, not a session; a
    /// dot inside a name is merely unusual. The directory rule allows no dots.
    #[test]
    fn an_id_of_dots_is_never_a_session() {
        for id in ["..", ".", "...", "a/b", "a\\b", ""] {
            assert!(!is_session_id(id), "{id:?} must not name a transcript");
            assert!(!droppen_sessions::reference::is_path_segment_id(id), "{id:?} must not name a directory");
        }
        assert!(is_session_id("a..b"), "a dot inside a name is merely unusual");
        assert!(!droppen_sessions::reference::is_path_segment_id("a..b"), "but never part of a directory name");
    }

    /// What an adoption by an earlier build left behind: a copy in this
    /// instance's root beside the user's untouched original, under one id. The
    /// feature that made it is gone; the copy must still be this server's own
    /// session, listed once, and resolved to the copy a resume runs against.
    #[tokio::test]
    async fn a_session_adopted_by_an_earlier_build_is_still_this_servers_own() {
        let _guard = droppen_sessions::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(home.path().to_path_buf(), "alice");
        let id = "0b1c2d3e-4f50-4612-8a7b-9c0d1e2f3a4b";
        for root in ClaudeRoot::ALL {
            let project = claude_roots::ClaudeRoots::of(&paths).projects_dir(root).join("-w");
            std::fs::create_dir_all(&project).expect("project dir");
            std::fs::write(
                project.join(format!("{id}.jsonl")),
                "{\"type\":\"user\",\"cwd\":\"/w\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n",
            )
            .expect("transcript");
        }

        let rows = list_claude_sessions(&paths, &BTreeMap::new()).await;
        let rows: Vec<&SessionSummary> = rows.iter().filter(|row| row.id == id).collect();
        assert_eq!(rows.len(), 1, "one session, not two: {rows:?}");
        assert_eq!(rows[0].scope, SessionScope::Owned, "the copy is this server's own");

        let found = find_claude_session(&paths, id).await.expect("found");
        assert_eq!(found.root, ClaudeRoot::Managed, "the copy, which is what a resume runs against");
        let summary = load_claude_summary(&paths, id).await.expect("the row");
        assert_eq!(summary.scope, SessionScope::Owned);
    }

    /// Every row says which server it came from, owned or not — and the two
    /// are different claims. See [`SessionSummary::server`].
    #[tokio::test]
    async fn a_discovered_session_is_reachable_through_this_server_and_says_so() {
        let _guard = droppen_sessions::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(home.path().to_path_buf(), "alice");

        // A transcript in the user's own `~/.claude`: nobody here made it, and
        // this server can nonetheless see it and serve it.
        let project = claude_roots::ClaudeRoots::of(&paths).projects_dir(ClaudeRoot::Local).join("-w");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::write(
            project.join("discovered.jsonl"),
            "{\"type\":\"summary\",\"summary\":\"someone else's\"}\n",
        )
        .expect("transcript");

        let found = load_claude_summary(&paths, "discovered").await.expect("the row");
        assert_eq!(found.scope, SessionScope::External, "not this server's to drive");
        assert_eq!(found.server, "alice", "but this server is the one it is reachable through");

        // The same file, once this instance records having made it: the value is
        // now read out of the ownership row rather than defaulted, which is the
        // claim that survives a restart.
        ClaudeCodeSessions.record_owned(&paths, "discovered").await.expect("record");
        let owned = load_claude_summary(&paths, "discovered").await.expect("the row");
        assert_eq!(owned.scope, SessionScope::Owned);
        assert_eq!(owned.server, "alice");

        // And a sibling instance, which did not make it and does not own it,
        // still answers for the copy it can see.
        let sibling = Paths::for_instance(home.path().to_path_buf(), "bob");
        let theirs = load_claude_summary(&sibling, "discovered").await.expect("the row");
        assert_eq!(theirs.scope, SessionScope::External);
        assert_eq!(theirs.server, "bob");
    }

    /// The durable index, end to end and in the two directions that matter: a
    /// summary that outlives the process which derived it, and a file that moved
    /// under one, which must not.
    ///
    /// The trick throughout is to make the index and the transcript *disagree* —
    /// a planted title the file could not produce — because that is the only way
    /// to tell "read out of the index" from "read the file again and got the
    /// same answer", which is what every other assertion here cannot distinguish.
    #[tokio::test]
    async fn a_summary_outlives_the_process_and_a_moved_file_does_not() {
        let _guard = droppen_sessions::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        droppen_sessions::index::install(&paths);

        let id = "durable-index-fixture";
        let (_dir, path) = session(&[r#"{"type":"summary","summary":"from the transcript","cwd":"/w"}"#]);
        let first = summarize_file(&path, id, ClaudeRoot::Local).await.expect("summarize");
        assert_eq!(first.title, "from the transcript");

        // The restart: the in-memory tier is precisely the part that dies.
        LIST_CACHE.clear();

        let index = droppen_sessions::index::open(&paths);
        let metadata = std::fs::metadata(&path).expect("stat");
        let planted = SessionSummary {
            title: "read out of the index".to_string(),
            ..first.clone()
        };
        index.remember_summary(Source::Claude, id, droppen_sessions::index::Watermark::of(&path, &metadata), &planted);
        assert_eq!(
            summarize_file(&path, id, ClaudeRoot::Local).await.expect("summarize").title,
            "read out of the index",
            "the head and tail are not read again for a file that has not moved"
        );

        // A row cached by a build that predates `unreadCount` still parses: the
        // upgrade costs the unread scan, not a re-summarise of every transcript.
        let mut older = serde_json::to_value(SessionSummary {
            title: "cached before unreadCount".to_string(),
            ..first.clone()
        })
        .expect("JSON");
        older.as_object_mut().expect("an object").remove("unreadCount");
        index.remember_summary(Source::Claude, id, droppen_sessions::index::Watermark::of(&path, &metadata), &older);
        LIST_CACHE.clear();
        assert_eq!(
            summarize_file(&path, id, ClaudeRoot::Local).await.expect("summarize").title,
            "cached before unreadCount"
        );

        // A transcript that grew: the watermark disagrees, so the planted row is
        // dead and the file decides. This is the assertion that makes the cache
        // safe to keep on disk at all.
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).expect("append");
            writeln!(file, r#"{{"type":"custom-title","customTitle":"renamed since"}}"#).expect("write");
        }
        LIST_CACHE.clear();
        assert_eq!(
            summarize_file(&path, id, ClaudeRoot::Local).await.expect("summarize").title,
            "renamed since",
            "a file that changed is read again, whatever the index remembers"
        );

        // And an index that is a file of rubbish is a slow list, not a failed
        // one: `Db` sets the wreckage aside, the lookup misses, the transcript
        // answers.
        let other = tempfile::tempdir().expect("tempdir");
        let wrecked = Paths::new(other.path().to_path_buf());
        std::fs::create_dir_all(wrecked.index_db().parent().expect("parent")).expect("mkdir");
        std::fs::write(wrecked.index_db(), b"this is not a database").expect("write");
        droppen_sessions::index::install(&wrecked);
        LIST_CACHE.clear();
        assert_eq!(
            summarize_file(&path, id, ClaudeRoot::Local).await.expect("summarize").title,
            "renamed since",
            "a damaged index costs a re-read and nothing else"
        );
    }

    /// Discovery hides what the archive holds: it reads the table
    /// `sessions::archive` writes.
    #[tokio::test]
    async fn discovery_reads_the_archive_s_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        archive::archive_claude_session(&paths, "one").await.expect("archive");
        archive::archive_claude_session(&paths, "two").await.expect("archive");
        assert_eq!(
            archived_ids(&paths).await,
            ["one".to_string(), "two".to_string()].into_iter().collect(),
            "discovery asks the same table"
        );
    }

    /// The root a session is bound by is found as every lookup by id finds
    /// it: managed first, an archived session not at all, and only by Claude
    /// Code's reader, the one whose sessions live in a Claude root.
    #[tokio::test]
    async fn a_session_s_root_is_found_managed_first_and_never_when_archived() {
        use droppen_sessions::Transcripts;

        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        for (root, ids) in [(ClaudeRoot::Local, ["local", "both", "hidden"].as_slice()), (ClaudeRoot::Managed, ["both"].as_slice())] {
            let project = claude_roots::ClaudeRoots::of(&paths).projects_dir(root).join("-w");
            std::fs::create_dir_all(&project).expect("project dir");
            for id in ids {
                std::fs::write(project.join(format!("{id}.jsonl")), "{}\n").expect("transcript");
            }
        }
        archive::archive_claude_session(&paths, "hidden").await.expect("archive");

        assert_eq!(ClaudeCodeSessions.find_root(&paths, "local").await, Some(ClaudeRoot::Local));
        assert_eq!(ClaudeCodeSessions.find_root(&paths, "both").await, Some(ClaudeRoot::Managed), "managed wins");
        assert!(find_in_root(&paths, ClaudeRoot::Local, "hidden").await.is_some(), "the file is there");
        assert_eq!(ClaudeCodeSessions.find_root(&paths, "hidden").await, None, "but archived");
        assert_eq!(ClaudeCodeSessions.find_root(&paths, "nothing").await, None);
        assert_eq!(crate::provider::codex::sessions::codex::CodexSessions.find_root(&paths, "local").await, None, "codex has no Claude root");
        assert_eq!(crate::provider::acp::sessions::AcpSessions.find_root(&paths, "local").await, None, "nor an ACP agent");
    }

    /// What Claude Code says to the user: the speech the attention clock, the
    /// unread count and the push read out of its transcripts
    /// ([`Transcripts::spoken_texts`]). The sessions core's own tests read a
    /// stand-in; these pin Claude Code's rules.
    mod speech {
        use super::*;
        use droppen_sessions::attention::derive::{echo, spoken, AssistantEcho};
        use droppen_sessions::attention::unread::{refresh, Ledger};
        use droppen_sessions::attention::{first_unread, FirstUnread};
        use droppen_sessions::Transcripts;

        const FALLBACK: &str = "2026-01-01T09:59:59.000Z";

        fn claude_echo(tail: &[String]) -> Option<AssistantEcho> {
            echo(&ClaudeCodeSessions, tail)
        }

        fn lines(raw: &[&str]) -> Vec<String> {
            raw.iter().map(|l| (*l).to_string()).collect()
        }

        fn at(n: u32) -> String {
            format!("2026-01-01T10:{:02}:{:02}.000Z", n / 60, n % 60)
        }

        fn assistant(n: u32, text: &str) -> String {
            format!(
                r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#,
                at(n)
            )
        }

        fn user(n: u32) -> String {
            format!(
                r#"{{"type":"user","timestamp":"{}","message":{{"content":[{{"type":"text","text":"go"}}]}}}}"#,
                at(n)
            )
        }

        fn write(dir: &Path, lines: &[String]) -> std::path::PathBuf {
            let path = dir.join("t.jsonl");
            let mut body = lines.join("\n");
            body.push('\n');
            std::fs::write(&path, body).expect("write");
            path
        }

        fn size(path: &Path) -> u64 {
            std::fs::metadata(path).expect("stat").len()
        }

        async fn ledger(reader: &dyn Transcripts, path: &Path, read_at: Option<&str>) -> Ledger {
            refresh(reader, path, None, size(path), read_at, FALLBACK).await.expect("a ledger")
        }

        fn said(n: u32) -> String {
            format!(r#"{{"type":"assistant","timestamp":"2026-01-01T10:00:{n:02}.000Z","message":{{"content":[{{"type":"text","text":"m{n}"}}]}}}}"#)
        }

        fn stamp(n: u32) -> String {
            format!("2026-01-01T10:00:{n:02}.000Z")
        }

        /// The clock is about the assistant *speaking*. A turn that ends mid-tool is
        /// the session working, and the newest text before it is still the last
        /// thing the user was told.
        #[test]
        fn thinking_and_tool_entries_do_not_move_the_clock() {
            let tail = lines(&[
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"working on it"}]}}"#,
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:06.000Z","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:07.000Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
                r#"{"type":"user","timestamp":"2026-01-01T00:00:08.000Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            ]);
            let echo = claude_echo(&tail).expect("an echo");
            assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:05.000Z"));
            assert_eq!(echo.text, "working on it");
        }

        /// A Task subagent talks for as long as it runs, and the user never sees a
        /// word of it. A clock that moved on those would mark a busy session unread
        /// the whole time it was busy.
        #[test]
        fn a_sidechain_never_moves_the_clock() {
            let tail = lines(&[
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"delegating"}]}}"#,
                r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-01-01T00:00:09.000Z","message":{"content":[{"type":"text","text":"subagent chatter"}]}}"#,
            ]);
            let echo = claude_echo(&tail).expect("an echo");
            assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:05.000Z"));
            assert_eq!(echo.text, "delegating");
        }

        /// The CLI writing in the model's voice — a rate-limit wall, an API error —
        /// is the last thing the session said, and both the echo and the count
        /// read it as such. They share one predicate, so they cannot drift apart.
        #[test]
        fn a_synthetic_notice_is_what_the_session_last_said() {
            let tail = lines(&[
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"on it"}]}}"#,
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:06.000Z","message":{"model":"<synthetic>","content":[{"type":"text","text":"You've hit your limit"}]}}"#,
            ]);
            let echo = claude_echo(&tail).expect("an echo");
            assert_eq!(echo.at.as_deref(), Some("2026-01-01T00:00:06.000Z"));
            assert_eq!(echo.text, "You've hit your limit");
            let entry = try_parse(&tail[1]).expect("parses");
            assert_eq!(spoken(&ClaudeCodeSessions, &entry).map(|said| said.messages), Some(1));
        }

        #[test]
        fn an_empty_text_block_is_not_speech() {
            let tail = lines(&[
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05.000Z","message":{"content":[{"type":"text","text":"real"}]}}"#,
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:06.000Z","message":{"content":[{"type":"text","text":"   "}]}}"#,
            ]);
            assert_eq!(claude_echo(&tail).expect("an echo").text, "real");
        }


        /// The unit is a message the user reads. Thinking, tool calls, tool results
        /// and a subagent's chatter are not; the CLI's own `<synthetic>` notice — a
        /// rate-limit wall — is, because it is how the session stopped.
        #[tokio::test]
        async fn only_assistant_text_counts() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = write(
                dir.path(),
                &[
                    user(0),
                    assistant(1, "first"),
                    format!(
                        r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"thinking","thinking":"hmm"}},{{"type":"tool_use","name":"Bash","input":{{}}}}]}}}}"#,
                        at(2)
                    ),
                    format!(
                        r#"{{"type":"user","timestamp":"{}","message":{{"content":[{{"type":"tool_result","content":"{{\"type\":\"assistant\"}}"}}]}}}}"#,
                        at(3)
                    ),
                    format!(
                        r#"{{"type":"assistant","isSidechain":true,"timestamp":"{}","message":{{"content":[{{"type":"text","text":"subagent"}}]}}}}"#,
                        at(4)
                    ),
                    format!(
                        r#"{{"type":"assistant","timestamp":"{}","message":{{"model":"<synthetic>","content":[{{"type":"text","text":"You've hit your limit"}}]}}}}"#,
                        at(5)
                    ),
                    format!(
                        r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"text","text":"   "}}]}}}}"#,
                        at(6)
                    ),
                    format!(
                        r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"text","text":"two"}},{{"type":"text","text":"blocks"}}]}}}}"#,
                        at(7)
                    ),
                ],
            );
            let ledger = ledger(&ClaudeCodeSessions, &path, None).await;
            assert_eq!(
                ledger.stamps,
                vec![at(1), at(5), at(7), at(7)],
                "one per text block, the limit notice included"
            );
            assert_eq!(ledger.count(None), Some(4), "never read: every message counts");
            assert_eq!(ledger.newest(), Some(at(7).as_str()));
        }

        /// A session that stopped on a rate limit or an API error has said its last
        /// word, and the badge must show it — the push notification already does.
        #[tokio::test]
        async fn a_synthetic_notice_at_the_end_is_unread() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = write(
                dir.path(),
                &[
                    assistant(1, "working on it"),
                    format!(
                        r#"{{"type":"assistant","timestamp":"{}","message":{{"model":"<synthetic>","content":[{{"type":"text","text":"API Error: 529 overloaded"}}]}}}}"#,
                        at(2)
                    ),
                ],
            );
            let ledger = ledger(&ClaudeCodeSessions, &path, Some(&at(1))).await;
            assert_eq!(ledger.count(Some(&at(1))), Some(1));
            assert_eq!(ledger.newest(), Some(at(2).as_str()));
        }

        /// Claude's string shorthand for a one-block message is still a message.
        #[tokio::test]
        async fn a_plain_string_message_counts() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = write(
                dir.path(),
                &[format!(
                    r#"{{"type":"assistant","timestamp":"{}","message":{{"content":"just a string"}}}}"#,
                    at(1)
                )],
            );
            assert_eq!(ledger(&ClaudeCodeSessions, &path, None).await.stamps, vec![at(1)]);
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
            let items = crate::provider::claude_code::sessions::transcript::parse_claude_transcript(&paths, &file, false)
                .await
                .expect("parse");
            // The row's `updatedAt` is the transcript's mtime: the ledger's stand-in.
            let mtime = "2026-01-01T10:00:04.000Z";
            let full = refresh(&ClaudeCodeSessions, &file, None, size(&file), None, mtime)
                .await
                .expect("a ledger");

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

    }

    /// Claude Code's token usage, one assistant entry at a time
    /// ([`Transcripts::usage_fold`]). The sessions core adds the folds up by day
    /// and model; these pin what each entry is.
    mod usage {
        use super::*;
        use droppen_sessions::Transcripts;
        use serde_json::json;

        /// What each entry of a literal transcript folds to, in order.
        fn folds(entries: Value) -> Vec<Option<TokenUsage>> {
            let mut fold = ClaudeCodeSessions.usage_fold();
            entries
                .as_array()
                .expect("array")
                .iter()
                .map(|entry| fold.fold(entry.as_object().expect("object")))
                .collect()
        }

        fn tokens(date: &str, model: &str, [input, output, cache_read, cache_write]: [f64; 4]) -> Option<TokenUsage> {
            Some(TokenUsage {
                date: date.to_string(),
                model: model.to_string(),
                input,
                output,
                cache_read,
                cache_write,
            })
        }

        #[test]
        fn an_assistant_entry_s_usage_is_its_tokens_and_nothing_else_counts() {
            let folded = folds(json!([
                {
                    "type": "assistant",
                    "timestamp": "2026-07-20T10:00:00.000Z",
                    "message": {
                        "model": "claude-fable-5",
                        "usage": { "input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 20 }
                    }
                },
                {
                    "type": "assistant",
                    "timestamp": "2026-07-20T11:00:00.000Z",
                    "message": {
                        "model": "claude-fable-5",
                        "usage": { "input_tokens": 2, "output_tokens": 3, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 }
                    }
                },
                {
                    "type": "assistant",
                    "timestamp": "2026-07-20T12:00:00.000Z",
                    "message": { "model": "claude-opus-4-8", "usage": { "input_tokens": 7, "output_tokens": 1 } }
                },
                // A user turn and a usage-less assistant entry are ignored.
                { "type": "user", "timestamp": "2026-07-20T10:30:00.000Z", "message": { "content": "hi" } },
                { "type": "assistant", "timestamp": "2026-07-20T13:00:00.000Z", "message": { "model": "claude-fable-5" } }
            ]));
            assert_eq!(
                folded,
                vec![
                    tokens("2026-07-20", "claude-fable-5", [10.0, 5.0, 100.0, 20.0]),
                    tokens("2026-07-20", "claude-fable-5", [2.0, 3.0, 0.0, 0.0]),
                    tokens("2026-07-20", "claude-opus-4-8", [7.0, 1.0, 0.0, 0.0]),
                    None,
                    None,
                ]
            );
        }

        #[test]
        fn the_day_is_the_entry_s_utc_calendar_day() {
            let folded = folds(json!([
                { "type": "assistant", "timestamp": "2026-07-20T23:59:00.000Z", "message": { "model": "m", "usage": { "input_tokens": 1 } } },
                { "type": "assistant", "timestamp": "2026-07-21T00:01:00.000Z", "message": { "model": "m", "usage": { "input_tokens": 4 } } }
            ]));
            assert_eq!(
                folded,
                vec![tokens("2026-07-20", "m", [1.0, 0.0, 0.0, 0.0]), tokens("2026-07-21", "m", [4.0, 0.0, 0.0, 0.0])]
            );
        }

        #[test]
        fn a_usage_less_or_zero_total_assistant_entry_is_skipped() {
            let folded = folds(json!([
                { "type": "assistant", "timestamp": "2026-07-20T10:00:00.000Z", "message": { "model": "m", "usage": { "input_tokens": 0 } } }
            ]));
            assert_eq!(folded, vec![None], "a zero-total delta adds nothing");
        }

        #[test]
        fn an_entry_without_a_model_is_attributed_to_unknown() {
            let folded = folds(json!([
                { "type": "assistant", "timestamp": "2026-07-20T10:00:00.000Z", "message": { "usage": { "input_tokens": 3 } } }
            ]));
            assert_eq!(folded[0].as_ref().map(|usage| usage.model.as_str()), Some("unknown"));
        }
    }

    /// Everything the request carried in occupies the window, not just the
    /// uncached part — a session running almost entirely on cache reads is still
    /// nearly full, and summing only `input_tokens` would read as empty.
    #[tokio::test]
    async fn claude_sums_fresh_cached_and_written_input() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"model":"claude-fable-5","usage":{"input_tokens":2,"cache_read_input_tokens":878645,"cache_creation_input_tokens":259,"output_tokens":40}}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 878_906);
        assert_eq!(usage.window, None, "claude never states its own window");
        assert_eq!(
            usage.model.as_deref(),
            Some("claude-fable-5"),
            "the model on the entry comes back with the reading"
        );
    }

    /// Taken verbatim from a live session that reported 512,720 tokens against a
    /// 200k window — 256% full, which no request could have been.
    ///
    /// Claude Code writes `message.model` with the `[1m]` marker stripped, so
    /// the id says `claude-opus-4-8` whether the session runs the 200k window or
    /// the 1M one, and the id rule alone can only ever guess the short one. The
    /// reading settles it: the API accepted this prompt, so the window held it.
    #[tokio::test]
    async fn a_reading_the_standard_window_could_not_have_held_proves_the_long_one() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":2,"cache_creation_input_tokens":1634,"cache_read_input_tokens":511084,"output_tokens":644}}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 512_720);
        assert_eq!(
            context_window(usage.model.as_deref()),
            DEFAULT_CONTEXT_WINDOW,
            "the id alone still reads as an ordinary model"
        );
        assert_eq!(resolve_window(Some(&usage), Some("claude-opus-4-8")), LARGE_CONTEXT_WINDOW);
        assert_eq!(
            usage.tokens, 512_720,
            "the occupancy is reported as recorded; it is the window that was wrong"
        );
    }

    /// The reading and the session's current choice can disagree — a session
    /// switched models between turns — and only the model that was in force for
    /// the measured request says anything about the window it was measured
    /// against.
    #[tokio::test]
    async fn the_window_follows_the_model_on_the_entry_the_reading_came_from() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"model":"claude-fable-5","usage":{"input_tokens":10,"cache_read_input_tokens":40000,"cache_creation_input_tokens":0}}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(
            resolve_window(Some(&usage), Some("claude-opus-4-8")),
            LARGE_CONTEXT_WINDOW,
            "the entry outranks the current choice"
        );

        let entryless = ContextUsage {
            tokens: 40_010,
            window: None,
            model: None,
        };
        assert_eq!(
            resolve_window(Some(&entryless), Some("claude-fable-5")),
            LARGE_CONTEXT_WINDOW,
            "an entry naming no model falls back"
        );
    }

    /// After a compact the window empties, and the entries above the compaction
    /// point describe a context that no longer exists. Reading the *last* entry
    /// rather than the largest is what keeps the ring falling back down — and
    /// keeps a stale 512k reading from widening the window of a session that is
    /// now a tenth as full.
    #[tokio::test]
    async fn a_compacted_session_is_measured_by_its_newest_turn_not_its_biggest() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":2,"cache_read_input_tokens":511084,"cache_creation_input_tokens":1634}}}"#,
            r#"{"type":"user","isCompactSummary":true,"message":{"content":[{"type":"text","text":"summary"}]}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":3,"cache_read_input_tokens":0,"cache_creation_input_tokens":29997}}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 30_000, "the turn after the compact, not the one before it");
        assert_eq!(
            resolve_window(Some(&usage), Some("claude-opus-4-8")),
            DEFAULT_CONTEXT_WINDOW,
            "a reading that fits is not evidence of a wider window"
        );
    }

    #[tokio::test]
    async fn claude_reads_the_newest_main_chain_entry() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":500,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":9999,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"next"}]}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 500, "a subagent's context is not this session's");
    }

    #[tokio::test]
    async fn a_transcript_with_no_usage_has_no_reading() {
        let (_dir, path) = session(&[r#"{"type":"user","message":{"content":[{"type":"text","text":"hi"}]}}"#]);
        assert_eq!(read_claude_context(&path).await, None);
    }

    /// The reading is looked for as far back as the context budget
    /// (`droppen_sessions::context::TAIL_BYTES`), further than this reader's own
    /// tail for discovery.
    #[tokio::test]
    async fn the_context_reading_is_found_further_back_than_the_discovery_tail() {
        let reading = r#"{"type":"assistant","message":{"usage":{"input_tokens":500,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let filler = format!(r#"{{"type":"user","message":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#, "x".repeat(1000));
        let lines: Vec<&str> = std::iter::once(reading).chain(std::iter::repeat_n(filler.as_str(), 90)).collect();
        let (_dir, path) = session(&lines);
        let behind = std::fs::metadata(&path).expect("size").len() - reading.len() as u64;
        assert!(TAIL_BYTES < behind && behind < droppen_sessions::context::TAIL_BYTES, "{behind}");
        assert_eq!(read_claude_context(&path).await.map(|usage| usage.tokens), Some(500));
    }
}
