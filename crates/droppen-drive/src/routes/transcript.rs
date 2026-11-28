//! Transcript history, search, unread, subagent, and checklist routes.

use super::*;

/// The transcript page immediately before an index the client already holds,
/// or — with `from` — the range back to that index, capped.
#[api(
    id = "session_history",
    group = "sessions",
    summary = "Older transcript",
    responses(200 = "The contiguous transcript page immediately before the requested index, or with `from` as much of the range `[from, before)` as the caps allow"),
    errors(
        404 = "Session not found",
        409 = "`transcript_changed`: the items before `before` are no longer the ones `fp` fingerprinted; the client's window needs replacing"
    )
)]
pub async fn session_history(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
) -> Result<Json<SessionHistoryPage>, ApiError> {
    let drive = &state.drive;
    let not_found = || ApiError::not_found("session not found");
    let state_now = feed::load_session_state(drive, source, &id).await.ok_or_else(not_found)?;
    let file_path = PathBuf::from(&state_now.summary.file_path);
    let sig = file_signature(&file_path).await.ok_or_else(not_found)?;
    let before = query.before as usize;
    let since = query.since.map(|since| since as usize);
    // Only the prefix the page depends on is checked, so a session that grew
    // since the client's window was cut is no reason to refuse it — nor to
    // parse it again: the newest parse there is, at whatever signature, holds
    // the same page whenever it holds the client's fingerprint. Its `sig` and
    // `totalItems` are that parse's: a client places the page by `startFp`,
    // and keeps the larger of its own total and the page's.
    let cached = match query.fp.as_deref() {
        Some(fp) => {
            feed::newest_parse_holding(&drive.transcripts, &file_path, &sig, |_, transcript| {
                feed::history_anchor_holds(transcript, before, fp, since)
            })
            .await
        }
        None => None,
    };
    let (sig, transcript) = match cached {
        Some(cached) => cached,
        None => {
            let transcript = drive.transcripts.load(&drive.paths, source, &file_path, &sig).await;
            if let Some(fp) = query.fp.as_deref() {
                if !feed::history_anchor_holds(&transcript, before, fp, since) {
                    return Err(ApiError::conflict("transcript_changed"));
                }
            }
            (sig, transcript)
        }
    };
    // A range depends on nothing past `before` either, so the anchor that
    // proves a page proves it.
    let page = match query.from {
        Some(from) => feed::transcript_range(&transcript.items, from as usize, before),
        None => feed::transcript_page(&transcript.items, before),
    };
    Ok(Json(SessionHistoryPage {
        sig,
        start_fp: transcript.fp_at(page.start_index).unwrap_or_default(),
        items: page.items,
        start_index: page.start_index,
        end_index: page.end_index,
        total_items: page.total_items,
        has_more_before: page.has_more_before,
    }))
}

/// Search one session's whole transcript, not just the window a client holds.
///
/// Read-only like the detail route, so an external session is as searchable as
/// an owned one.
#[api(
    id = "session_search",
    group = "sessions",
    summary = "Search a session's transcript",
    responses(200 = "Every item of the whole transcript holding all the query's terms: the count, and the newest `limit` with snippets"),
    errors(400 = "Empty or whitespace-only query, longer than 256 characters, or more than 16 terms", 404 = "Session not found")
)]
pub async fn session_search(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<Json<feed::search::SessionSearchResult>, ApiError> {
    search_session(&state.drive, source, &id, &query).await.map(Json)
}

/// [`session_search`] without the extractors, for the tests.
pub(crate) async fn search_session(
    drive: &crate::Drive,
    source: Source,
    id: &str,
    query: &SearchQuery,
) -> Result<feed::search::SessionSearchResult, ApiError> {
    // Before anything is read: a refused query costs nothing.
    let terms = feed::search::validate_query(&query.q).map_err(|error| ApiError::bad_request(error.to_string()))?;
    let limit = query
        .limit
        .map_or(feed::search::DEFAULT_LIMIT, |limit| limit as usize)
        .min(feed::search::MAX_LIMIT);
    // The feed's own parse, cached: a search per keystroke never re-reads the
    // file, and its indices are the ones the feed and the history route use.
    let (items, _) = whole_transcript(drive, source, id).await?;
    let text = query.q.clone();
    // A scan of a long transcript is tens of milliseconds of plain CPU work,
    // which does not belong on a runtime thread serving sockets.
    tokio::task::spawn_blocking(move || feed::search::search_items(&items, &text, &terms, limit, feed::search::TOTAL_CAP))
        .await
        .map_err(|error| ApiError::internal(format!("search failed: {error}")))
}

/// Every item of one session's transcript, by the feed's own cached parse, and
/// the file's mtime (ISO-8601).
///
/// Only the file is looked up, not the row: a claude row's live status costs a
/// read of every live registration and a walk back through the transcript's
/// tail, and nothing that reads the items needs it. The scope is deliberately
/// not checked either — reading is never gated.
async fn whole_transcript(drive: &crate::Drive, source: Source, id: &str) -> Result<(Arc<Vec<Arc<TranscriptItem>>>, String), ApiError> {
    let file = transcript_file(drive, source, id).await?;
    Ok((drive.transcripts.load(&drive.paths, source, &file.path, &file.sig).await.items.clone(), file.mtime))
}

/// Where one session's transcript is and what it is now, without reading it.
struct TranscriptFile {
    path: PathBuf,
    /// Its [`file_signature`].
    sig: String,
    /// Its mtime (ISO-8601).
    mtime: String,
}

/// [`TranscriptFile`] for session `id`, looked up the way
/// [`whole_transcript`] says.
async fn transcript_file(drive: &crate::Drive, source: Source, id: &str) -> Result<TranscriptFile, ApiError> {
    let not_found = || ApiError::not_found("session not found");
    let path = drive.transcripts.find_file(&drive.paths, source, id).await.ok_or_else(not_found)?;
    let sig = file_signature(&path).await.ok_or_else(not_found)?;
    // In the form it is on disk: a codex rollout may be compressed.
    let mtime = droppen_sessions::compressed::locate(&path)
        .await
        .and_then(|(_, metadata)| metadata.modified())
        .map(droppen_base::time::iso8601)
        .map_err(|_| not_found())?;
    Ok(TranscriptFile { path, sig, mtime })
}

/// Where the messages a visit has not read yet begin.
///
/// The client asks once per arrival at a session, with the read marker its
/// arrival's seen report replaced (`previousReadAt` on that report's answer) —
/// so the answer is about what was unread *before* the reader got there, and
/// no later report can move it. Read-only and ungated like search, and answered
/// over the same cached parse, so the index is one the history route can page
/// to. With the client's resume marker (`since`, `fp`) that may be an older
/// parse than the file now has, when it provably gives the same index.
/// See `sessions::attention::first_unread`.
#[api(
    id = "session_first_unread",
    group = "sessions",
    summary = "Where unread messages begin",
    responses(200 = "The index of the first agent message newer than `after`, and how many there are"),
    errors(404 = "Session not found")
)]
pub async fn session_first_unread(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
    ApiQuery(query): ApiQuery<FirstUnreadQuery>,
) -> Result<Json<droppen_sessions::attention::FirstUnread>, ApiError> {
    first_unread_of(&state.drive, source, &id, &query).await.map(Json)
}

/// [`session_first_unread`] without the extractors, for the tests.
pub(crate) async fn first_unread_of(
    drive: &crate::Drive,
    source: Source,
    id: &str,
    query: &FirstUnreadQuery,
) -> Result<droppen_sessions::attention::FirstUnread, ApiError> {
    let file = transcript_file(drive, source, id).await?;
    let after = query.after.as_deref();
    let cached = match (query.since, query.fp.as_deref()) {
        (Some(since), Some(fp)) => cached_first_unread(drive, &file, since as usize, fp, after).await,
        _ => None,
    };
    if let Some(found) = cached {
        return Ok(found);
    }
    let items = drive.transcripts.load(&drive.paths, source, &file.path, &file.sig).await.items.clone();
    Ok(droppen_sessions::attention::first_unread(&items, after, &file.mtime))
}

/// [`droppen_sessions::attention::first_unread`] off the newest parse there
/// is, when it is provably the index a fresh read would answer; `None` when
/// the transcript has to be read.
///
/// The answer depends on the transcript's tail — an unread message may have
/// been written after any parse — so the parse has to hold more than a page's
/// anchor would. All four of:
///
/// * the client's resume marker: the parse's items `[0, since - K)` are the
///   ones the client's window was cut from ([`feed::newest_parse_holding`]),
///   and therefore the ones any parse still holding that marker has;
/// * the client's whole window (`since` items), so that `count`, which counts
///   to the end of the parse, misses nothing the client already holds;
/// * an unread message inside `[0, since - K)`. Every item up to it is then
///   the same in a fresh parse holding the marker: none before it is unread,
///   and it is, whatever was written after. A message without a timestamp
///   stands at the file's mtime as it is now, as on a fresh read;
/// * a parse no further behind the file than a live session's feed is
///   ([`FIRST_UNREAD_MAX_LAG_MS`], [`FIRST_UNREAD_MAX_LAG_BYTES`]). The marker
///   proves the parse is the client's, not that it is the file's: a client
///   back at a session it held a window of while nobody re-read it — its
///   socket closed, the agent writing on — holds a marker the last visit's
///   parse still matches, and that parse is missing everything written since.
///   Read fresh, it joins the parse the resumed feed has started. The lag is
///   measured against the file as it is, looked at again when `file.sig` is
///   behind the parse ([`feed::newest_parse_holding`]).
async fn cached_first_unread(
    drive: &crate::Drive,
    file: &TranscriptFile,
    since: usize,
    fp: &str,
    after: Option<&str>,
) -> Option<droppen_sessions::attention::FirstUnread> {
    let proven = since.checked_sub(droppen_sessions::loader::RESUME_OVERLAP)?;
    let (_, transcript) = feed::newest_parse_holding(&drive.transcripts, &file.path, &file.sig, |lag, transcript| {
        lags_like_a_live_feed(lag) && transcript.len() >= since && feed::history_anchor_holds(transcript, 0, fp, Some(since))
    })
    .await?;
    let found = droppen_sessions::attention::first_unread(&transcript.items, after, &file.mtime);
    found.index.0.is_some_and(|index| index < proven).then_some(found)
}

/// How much later than the parse's own signature the file may have been
/// written for [`cached_first_unread`] to answer off it: what the feed of a
/// session being watched lags by — its 150 ms debounce, then a whole parse,
/// about a second for a 500 MB transcript, while the agent goes on appending —
/// several times over. A session written to while nobody re-read it shows the
/// whole time it was written to.
pub(super) const FIRST_UNREAD_MAX_LAG_MS: f64 = 10_000.0;

/// How much text the file may have gained over the parse for
/// [`cached_first_unread`] to answer off it: a few seconds of a busy agent's
/// tool output, which averages tens of kilobytes a call. What it bounds is how
/// much `count` can miss when the writes all landed within
/// [`FIRST_UNREAD_MAX_LAG_MS`] of the parse.
pub(super) const FIRST_UNREAD_MAX_LAG_BYTES: u64 = 8 << 20;

/// Whether a parse that far behind the file is no further behind than a
/// watched session's feed would be.
fn lags_like_a_live_feed(lag: droppen_sessions::Growth) -> bool {
    lag.bytes <= FIRST_UNREAD_MAX_LAG_BYTES && lag.millis <= FIRST_UNREAD_MAX_LAG_MS
}

/// Every subagent that ever wrote under a session.
///
/// The fallback entry point for work the transcript cannot point at. The
/// drill-down in the transcript hangs off a `Task` `tool_use` whose id a meta
/// names, and two real shapes never match one: a Skill-spawned agent, whose meta
/// carries no `toolUseId`, and any agent at `spawnDepth` 2 or deeper, whose
/// `toolUseId` names a call in its parent *agent's* transcript. Everything such
/// an agent did — 2.5 MB across eleven agents in `b16232e3`, 116 of them in
/// another session — was unreachable from the UI. This lists them all, anchored
/// or not, so there is always a way in.
#[api(
    id = "session_agents",
    group = "sessions",
    summary = "Subagents of a session",
    responses(200 = "Every subagent that wrote a transcript under this session, anchored or not"),
    errors(404 = "Session not found, or the source has no subagents")
)]
pub async fn session_agents(
    State(state): State<DriveState>,
    ApiPath((source, id)): ApiPath<(Source, String)>,
) -> Result<Json<SessionAgents>, ApiError> {
    let drive = &state.drive;
    let Some(subagents) = drive.transcripts.subagents(source) else {
        return Err(ApiError::not_found("subagents exist only for claude sessions"));
    };
    let Some(file_path) = drive.transcripts.find_file(&drive.paths, source, &id).await else {
        return Err(ApiError::not_found("session not found"));
    };
    // Which ids the session's own transcript can point at. Parsed through the
    // shared cache, so the common case — a client opening the panel on a session
    // it is already watching — costs nothing.
    let anchors: HashSet<String> = match file_signature(&file_path).await {
        Some(sig) => drive
            .transcripts
            .load_items(&drive.paths, source, &file_path, &sig)
            .await
            .iter()
            .filter_map(|item| item.tool_use_id.clone())
            .collect(),
        None => HashSet::new(),
    };
    let agents = subagents
        .agents(&file_path)
        .await
        .into_iter()
        .map(|agent| SessionAgent {
            anchored: agent.tool_use_id.as_deref().is_some_and(|id| anchors.contains(id)),
            agent_id: agent.agent_id,
            agent_type: Nullable(agent.agent_type),
            description: Nullable(agent.description),
            tool_use_id: Nullable(agent.tool_use_id),
            parent_agent_id: Nullable(agent.parent_agent_id),
            spawn_depth: Nullable(agent.spawn_depth),
            model: Nullable(agent.model),
            bytes: agent.bytes,
            updated_at: Nullable(agent.updated_at),
        })
        .collect();
    Ok(Json(SessionAgents { agents }))
}

/// The transcript of a subagent this session's Task call spawned.
#[api(
    id = "agent_transcript",
    group = "sessions",
    summary = "Subagent transcript",
    responses(200 = "Transcript of a subagent spawned by this session's Task call"),
    errors(404 = "Session or subagent not found")
)]
pub async fn agent_transcript(
    State(state): State<DriveState>,
    ApiPath(AgentPath { source, id, agent_id }): ApiPath<AgentPath>,
) -> Result<Json<AgentTranscript>, ApiError> {
    let drive = &state.drive;
    let Some(subagents) = drive.transcripts.subagents(source) else {
        return Err(ApiError::not_found("subagents exist only for claude sessions"));
    };
    if !is_agent_id(&agent_id) {
        return Err(ApiError::bad_request("invalid agent id"));
    }
    let Some(file_path) = drive.transcripts.find_file(&drive.paths, source, &id).await else {
        return Err(ApiError::not_found("session not found"));
    };
    // Searched rather than composed: an agent the CLI filed under a
    // subdirectory of `subagents/` is still this session's, and answering 404
    // for it is how a whole class of children stayed unreadable.
    let Some(sub_path) = subagents.agent_file(&file_path, &agent_id).await else {
        return Err(ApiError::not_found("subagent not found"));
    };
    // The meta sits beside the transcript, which for a nested agent is not the
    // top-level directory — reading only that one described it as anonymous.
    let meta = match sub_path.parent() {
        Some(dir) => subagents.agent_metas(dir).await.into_iter().find(|meta| meta.agent_id == agent_id),
        None => None,
    };

    let items = subagents.agent_items(&drive.paths, &sub_path).await.unwrap_or_default();
    let (kept, omitted) = feed::truncate_items(&items);
    Ok(Json(AgentTranscript {
        agent_type: Nullable(meta.as_ref().and_then(|m| m.agent_type.clone())),
        description: Nullable(meta.and_then(|m| m.description)),
        total_items: items.len(),
        items: kept,
        omitted_items: omitted,
    }))
}

/// The task checklist Claude Code keeps beside a session.
///
/// Claude-only, like the subagent route above: Codex has no equivalent sidecar,
/// so asking for one is a `404` rather than an empty list that would imply the
/// session simply has not planned anything yet.
#[api(
    id = "session_checklist",
    group = "sessions",
    summary = "Session checklist",
    responses(200 = "The task checklist Claude Code keeps beside this session"),
    errors(404 = "Session not found, or the source has no checklists")
)]
pub async fn session_checklist(State(state): State<DriveState>, ApiPath((source, id)): ApiPath<(Source, String)>) -> Result<Json<Checklist>, ApiError> {
    let Some(checklists) = state.drive.transcripts.checklists(source) else {
        return Err(ApiError::not_found("checklists exist only for claude sessions"));
    };
    let paths = &state.drive.paths;
    let Some(checklist) = checklists.read(paths, &id).await else {
        return Err(ApiError::not_found("session not found"));
    };
    Ok(Json(checklist))
}
