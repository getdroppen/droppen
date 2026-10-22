//! Claude Code session discovery.
//!
//! Port of the discovery half of the former TypeScript server's `claude.ts`: scan
//! `~/.claude/projects/*/<uuid>.jsonl`, derive a title/cwd/branch from the head
//! (plus the newest `custom-title` in the tail), then overlay live-process
//! status from `~/.claude/sessions/*.json`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde_json::Value;

use super::transcript::{content_blocks, title_line, user_title_candidate};
use super::util::{self, read_head_lines, read_tail_lines, try_parse};
use super::{iso8601, mtime_ms, Paths, SessionActivity, SessionScope, SessionStatus, SessionSummary, Source};
use crate::claude_roots::{self, ClaudeRoot};
use crate::openapi_ext::Nullable;

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
    let local = super::ServerId::local(paths);
    let owned = super::owned_sessions::read(paths).await;
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

/// How many transcripts the in-memory tier holds before it is emptied.
///
/// There was no bound at all, and that was a leak rather than a simplification:
/// nothing ever dropped the entry for a transcript that had since been archived,
/// deleted or renamed, so a server that lists every few seconds for a fortnight
/// accumulated a summary for every session it had ever seen. The cap is far
/// above the corpus — 70 transcripts on the machine this was measured on — so
/// the working set never reaches it.
///
/// Emptied wholesale rather than evicted one at a time, following [`super::repo`]
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
/// process; behind it sits [`crate::db::index`], which is what stops a restart
/// costing 11.2 MB of head and tail parsing before the first list appears.
static LIST_CACHE: LazyLock<Mutex<HashMap<PathBuf, (f64, SessionSummary)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Hold a summary in the near tier, keeping that tier bounded.
fn remember(file_path: &Path, mtime: f64, summary: &SessionSummary) {
    let mut cache = LIST_CACHE.lock().expect("list cache poisoned");
    // Refreshing a transcript already in the map must never trigger the clear:
    // at the cap, a list of live sessions would otherwise empty the cache on
    // every single refresh and never hit it again.
    if cache.len() >= CACHE_MAX && !cache.contains_key(file_path) {
        cache.clear();
    }
    cache.insert(file_path.to_path_buf(), (mtime, summary.clone()));
}

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
    let mark = crate::db::index::Watermark::of(file_path, &metadata);
    if let Some((cached_mtime, summary)) = LIST_CACHE.lock().expect("list cache poisoned").get(file_path) {
        // Size as well as mtime, so the near tier agrees with the durable one
        // about what "unchanged" means. It is free — the summary already carries
        // the length it was built from.
        if *cached_mtime == mtime && summary.size_bytes == metadata.len() {
            return Ok(summary.clone());
        }
    }

    // What some previous boot worked out about this exact file. The watermark is
    // the entire check: same path, same length, same mtime, or it is a miss and
    // the transcript gets read like there was no index at all.
    let index = crate::db::index::current();
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
        remember(file_path, mtime, &summary);
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
        // Filled in by `crate::sessions::repo::apply`, which spawns one `git`
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
        // Both refined by `crate::sessions::DriverStates::apply` for the
        // sessions this server drives; a row nobody drives keeps these.
        activity: SessionActivity::Idle,
        resident: Nullable(None),
        claude_root: Some(root),
        account_id: None,
        // Filled in by `crate::attention::apply`, which memoises the transcript
        // read this cache would otherwise repeat for it.
        attention_at: Nullable(None),
        unread: false,
        unread_count: 0,
        // Live status, applied by the callers below with `status`.
        asking: false,
    };
    remember(file_path, mtime, &summary);
    // Both tiers hold the row *before* the overlays — ownership, live status,
    // the user's own title, the account binding. Those are applied to the copy
    // that goes out, by the callers below, because each of them can change
    // without the transcript being touched at all.
    if let Some(index) = index {
        index.remember_summary(Source::Claude, id, mark, &summary);
    }
    Ok(summary)
}

/// Whether a pid still names a live process.
///
/// `kill(pid, 0)` succeeds for a live process we own and fails with `EPERM` for
/// a live process we do not — both mean "alive", which is what the TypeScript's
/// `process.kill(pid, 0)` / `EPERM` check encodes.
fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the permission/existence check and never
    // delivers a signal, so this cannot affect the target process.
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Session ids that have a live Claude Code process attached right now.
///
/// Both roots are scanned: a managed session registers its pid under the managed
/// root, and a client that cannot see it would happily send into a live CLI.
pub async fn live_claude_session_ids(paths: &Paths) -> HashSet<String> {
    let mut out = HashSet::new();
    for root in ClaudeRoot::ALL {
        collect_live_ids(&claude_roots::live_dir(paths, root), &mut out).await;
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
        if is_process_alive(pid as i32) {
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
    crate::drive::archive::archived_ids(paths)
}

/// User-set titles, which outrank anything derived from the transcript.
///
/// Keyed `source:id` in one shared sidecar, so Codex discovery reads it too.
pub(super) async fn custom_titles(paths: &Paths, source: Source) -> HashMap<String, String> {
    let prefix = format!("{source}:");
    crate::drive::titles::all(paths)
        .into_iter()
        .filter_map(|(key, value)| {
            let id = key.strip_prefix(&prefix)?;
            Some((id.to_string(), value))
        })
        .collect()
}

/// Every `<uuid>.jsonl` under one root's projects directory.
async fn root_transcripts(paths: &Paths, root: ClaudeRoot) -> Vec<(PathBuf, String)> {
    let Ok(mut project_dirs) = tokio::fs::read_dir(claude_roots::projects_dir(paths, root)).await else {
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
    // `Paths`. See [`crate::db::index::current`].
    crate::db::index::install(paths);
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
    let live = live_claude_session_ids(paths).await;
    for summary in &mut summaries {
        summary.account_id = bindings.get(&summary.id).cloned();
        if live.contains(&summary.id) {
            let activity = claude_activity(Path::new(&summary.file_path)).await;
            (summary.status, summary.asking) = (activity.status, activity.asking);
        }
    }

    let titles = custom_titles(paths, Source::Claude).await;
    if !titles.is_empty() {
        for summary in &mut summaries {
            if let Some(custom) = titles.get(&summary.id) {
                summary.title = custom.clone();
            }
        }
    }
    mark_owned(paths, &mut summaries).await;
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
pub fn is_session_id(id: &str) -> bool {
    !id.is_empty() && !id.bytes().all(|b| b == b'.') && id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// Whether an id may name a *directory* beside its transcript.
///
/// Strictly narrower than [`is_session_id`], and used wherever an id becomes a
/// path segment of its own rather than part of a file name, such as the task
/// sidecars. Real ids are uuids, so nothing
/// legitimate is turned away — and a rule with no dots in it at all cannot be
/// argued into traversing.
pub fn is_session_dir_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
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
    let mut project_dirs = tokio::fs::read_dir(claude_roots::projects_dir(paths, root)).await.ok()?;
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
    crate::db::index::install(paths);
    let found = find_claude_session(paths, id).await?;
    let mut summary = summarize_file(&found.file_path, id, found.root).await.ok()?;
    if live_claude_session_ids(paths).await.contains(id) {
        let activity = claude_activity(&found.file_path).await;
        (summary.status, summary.asking) = (activity.status, activity.asking);
    }
    let titles = custom_titles(paths, Source::Claude).await;
    if let Some(custom) = titles.get(id) {
        summary.title = custom.clone();
    }
    // And the same for ownership: the composer's verdict is read off this row,
    // so it has to say what the list said.
    mark_owned(paths, std::slice::from_mut(&mut summary)).await;
    Some(summary)
}

/// The model/effort a session last ran with, read from its transcript tail.
pub async fn read_cli_choice(file_path: &Path) -> Option<super::CliChoice> {
    // Claude writes model+effort on every assistant entry, so a small tail is enough.
    let lines = read_tail_lines(file_path, 128 * 1024).await.ok()?;
    for line in lines.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        if entry.get("type").and_then(Value::as_str) != Some("assistant") || entry.get("isSidechain") == Some(&Value::Bool(true)) {
            continue;
        }
        let model = entry
            .get("message")
            .and_then(Value::as_object)
            .and_then(|m| m.get("model"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let effort = entry.get("effort").and_then(Value::as_str).map(str::to_owned);
        if model.is_none() && effort.is_none() {
            continue;
        }
        return Some(super::CliChoice {
            model: Nullable(model),
            effort: Nullable(effort),
            at: Nullable(entry.get("timestamp").and_then(Value::as_str).map(str::to_owned)),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
            assert!(!is_session_dir_id(id), "{id:?} must not name a directory");
        }
        assert!(is_session_id("a..b"), "a dot inside a name is merely unusual");
        assert!(!is_session_dir_id("a..b"), "but never part of a directory name");
    }

    /// What an adoption by an earlier build left behind: a copy in this
    /// instance's root beside the user's untouched original, under one id. The
    /// feature that made it is gone; the copy must still be this server's own
    /// session, listed once, and resolved to the copy a resume runs against.
    #[tokio::test]
    async fn a_session_adopted_by_an_earlier_build_is_still_this_servers_own() {
        let _guard = crate::db::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(home.path().to_path_buf(), "alice");
        let id = "0b1c2d3e-4f50-4612-8a7b-9c0d1e2f3a4b";
        for root in ClaudeRoot::ALL {
            let project = claude_roots::projects_dir(&paths, root).join("-w");
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
        let _guard = crate::db::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(home.path().to_path_buf(), "alice");

        // A transcript in the user's own `~/.claude`: nobody here made it, and
        // this server can nonetheless see it and serve it.
        let project = claude_roots::projects_dir(&paths, ClaudeRoot::Local).join("-w");
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
        crate::sessions::owned_sessions::record(&paths, Source::Claude, "discovered")
            .await
            .expect("record");
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
        let _guard = crate::db::index::exclusive().await;
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        crate::db::index::install(&paths);

        let id = "durable-index-fixture";
        let (_dir, path) = session(&[r#"{"type":"summary","summary":"from the transcript","cwd":"/w"}"#]);
        let first = summarize_file(&path, id, ClaudeRoot::Local).await.expect("summarize");
        assert_eq!(first.title, "from the transcript");

        // The restart: the in-memory tier is precisely the part that dies.
        LIST_CACHE.lock().expect("list cache poisoned").clear();

        let index = crate::db::index::open(&paths);
        let metadata = std::fs::metadata(&path).expect("stat");
        let planted = SessionSummary {
            title: "read out of the index".to_string(),
            ..first.clone()
        };
        index.remember_summary(Source::Claude, id, crate::db::index::Watermark::of(&path, &metadata), &planted);
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
        index.remember_summary(Source::Claude, id, crate::db::index::Watermark::of(&path, &metadata), &older);
        LIST_CACHE.lock().expect("list cache poisoned").clear();
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
        LIST_CACHE.lock().expect("list cache poisoned").clear();
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
        crate::db::index::install(&wrecked);
        LIST_CACHE.lock().expect("list cache poisoned").clear();
        assert_eq!(
            summarize_file(&path, id, ClaudeRoot::Local).await.expect("summarize").title,
            "renamed since",
            "a damaged index costs a re-read and nothing else"
        );
    }
}
