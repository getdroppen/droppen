//! Copying a session's transcript files into its recording as they grow.
//!
//! The stream-json a resident writes is what the *driver* reads; it is not what
//! the Session page renders. That comes off disk — `<project>/<id>.jsonl`, the
//! `subagents/agent-*.jsonl` of every child agent, the `.meta.json` beside each
//! one that links an agent back to the `Task` call that spawned it, and the
//! `tasks/<id>/*.json` checklist. A recording that held only stdio would replay
//! into a Session page with no transcript, no drill-down and no checklist,
//! which is to say into nothing worth looking at.
//!
//! So this task follows those files for as long as a resident is alive, and
//! writes what they gained into the recording. Two shapes, because the CLI
//! writes two:
//!
//! * **Append-only line streams** — the `.jsonl` transcripts. Followed by byte
//!   offset, one [`Event::Transcript`] per complete line. A line without its
//!   terminating newline is the CLI caught mid-write, and is left for the next
//!   sweep rather than recorded in half.
//! * **Rewritten snapshots** — the agent metas (a single line with *no*
//!   trailing newline, so a line-follower would never see them) and the
//!   checklist files, which the CLI rewrites in place as a task's status
//!   changes. Recorded whole, as [`Event::Sidecar`], whenever they change.
//!
//! # Why there is a cursor file
//!
//! A recording is appended to across runs and across servers: the same session
//! is driven by a fresh resident after every settings change, and by a fresh
//! *server* after every upgrade. Each of those opens the recording again, and
//! each would copy the whole transcript from byte zero — a recording holding
//! four copies of a conversation replays into a transcript holding four copies
//! of a conversation. `<session-id>.offsets.json` beside the recording is what
//! remembers how far the last one got.
//!
//! When it is missing but the recording is not — a recorder switched on
//! mid-session, a cursor file somebody deleted — the transcript is picked up at
//! its *end* and a [`Event::Note`] says so. Recording a duplicate would be
//! silently wrong; recording a gap is loudly incomplete, which is the failure a
//! person can act on.
//!
//! # Why history is rationed
//!
//! A *fresh* recording over a session that already exists is the other side of
//! the same question, and it has a different answer: the history is real, and
//! it is right there in the file being read. Copying all of it is what fills a
//! recording's ceiling before it has witnessed anything — see [`MAX_BACKFILL`]
//! for the measurement — so each file this tailer meets for the first time
//! contributes a bounded tail of what it already held, and says in a
//! [`Event::Note`] how much it left behind. `replay` needs no more than that:
//! the bytes it skipped are still on disk, in the transcript they came from.
//!
//! # Timing
//!
//! `notify` plus a poll, exactly as [`crate::feed::watch`] does it and for the
//! same reason: the watcher is an optimisation (it makes a recording's
//! timestamps match the moment the CLI wrote, which is what replay's pacing
//! reads), and the poll is the correctness backstop for every filesystem and
//! sandbox where the watcher says nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc;

use super::{Event, Line, Stop, SCHEMA};
use crate::claude_roots::ClaudeRoot;
use crate::feed::watch::{self, FileEvents};
use crate::sessions::transcript::{SubagentFileKind, SubagentFilesExt};
use crate::sessions::{iso8601, mtime_ms, Paths};

/// How often the files are looked at when the watcher says nothing.
const POLL: Duration = Duration::from_secs(1);

/// The window a burst of writes is coalesced into, matching the feed's.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// The most any one file may contribute to a single sweep.
///
/// A recording that opens on a session with a hundred megabytes of history
/// would otherwise read all of it into memory in one go. Reaching this cap does
/// not delay anything: the sweep loop goes straight round again.
const MAX_CHUNK: u64 = 8 * 1024 * 1024;

/// How much of a file's *existing* content a recording will take on.
///
/// See [`State::start_of`]. Per file, so that the main transcript cannot spend
/// the whole allowance and leave a session's subagents — which is what the
/// drill-down renders — with nothing.
const MAX_FILE_BACKFILL: u64 = 1024 * 1024;

/// How much existing content a whole recording will take on, across every file.
///
/// The number that stops a long-lived session from filling its ceiling in one
/// sweep. The case it is measured against is real: switching recording on over
/// a session with a hundred and seventy subagent transcripts copied **228 MB in
/// the first minute** and then averaged 0.1 MB a minute for the next forty —
/// that is, 99% of a 256 MiB budget spent on history before the recording had
/// witnessed anything at all.
const MAX_BACKFILL: u64 = 16 * 1024 * 1024;

/// Where to start reading a file this recording has never seen.
///
/// Split out from [`State::start_of`] because it is the whole of the policy and
/// none of the I/O: everything else there is opening files and finding line
/// boundaries. `0` means the file is short enough to take whole.
fn budgeted_start(len: u64, remaining: u64) -> u64 {
    len.saturating_sub(remaining.min(MAX_FILE_BACKFILL))
}

/// What the tailer is started with.
pub(super) struct Tail {
    /// The recording's writer.
    pub tx: mpsc::UnboundedSender<Line>,
    /// Where transcripts are discovered.
    pub paths: Arc<Paths>,
    /// The session id to follow, which `adopt` moves under us.
    pub follow: Arc<Mutex<String>>,
    /// Raised when the last [`super::Recorder`] handle goes.
    pub stop: Arc<Stop>,
    /// Where the cursors are kept between runs.
    pub offsets: PathBuf,
    /// Whether the recording was empty when it was opened — that is, whether
    /// there is any history worth catching up on.
    pub fresh: bool,
}

/// How far each file has been recorded.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cursors {
    /// Byte offsets into the append-only transcripts, keyed by `<root>/<path>`.
    #[serde(default)]
    lines: HashMap<String, u64>,
    /// `<len>:<mtime-ms>` of each snapshot file, keyed the same way.
    ///
    /// A fingerprint rather than a hash, so a sweep costs one `stat` per
    /// sidecar instead of a read: the CLI rewrites these files at distinct
    /// moments, and two rewrites in the same millisecond that also leave the
    /// file exactly as long is not a case worth a per-sweep read of every task
    /// file in the session.
    #[serde(default)]
    snapshots: HashMap<String, String>,
}

/// Follow a session's files until the recorder is dropped.
pub(super) async fn run(tail: Tail) {
    let mut state = State {
        cursors: load(&tail.offsets).await,
        seeded: tokio::fs::try_exists(&tail.offsets).await.unwrap_or(false),
        first: true,
        dirty: false,
        events: watch::never(),
        watching: None,
        resolved: None,
        misses: 0,
        backfill: MAX_BACKFILL,
        tail,
    };

    loop {
        if state.sweep().await.is_err() {
            // The writer has gone: the ceiling was reached, or the file could
            // not be opened. There is nothing left for this task to write to.
            return;
        }
        state.persist().await;
        if state.tail.stop.stopped() {
            return;
        }
        let wait = state.poll_after();
        tokio::select! {
            _ = state.tail.stop.wake.notified() => {}
            _ = state.events.recv() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// The tailer's own state, kept out of [`Tail`] so the inputs stay obvious.
struct State {
    tail: Tail,
    cursors: Cursors,
    /// Whether a cursor file was there to load. See the module docs: a
    /// recording with history and no cursors starts at the end.
    seeded: bool,
    /// Whether this is the first sweep, which is the only one allowed to skip
    /// past a file's existing contents.
    first: bool,
    /// Whether the cursors have moved since they were last written.
    dirty: bool,
    events: FileEvents,
    /// The directories the watcher was pointed at, so it is only rebuilt when
    /// they change.
    watching: Option<Vec<PathBuf>>,
    /// The session id and transcript the last resolve found, so the projects
    /// directories are not re-scanned once per second for the whole session.
    resolved: Option<(String, ClaudeRoot, PathBuf)>,
    /// How many sweeps in a row have failed to find a transcript at all.
    misses: u32,
    /// How much of [`MAX_BACKFILL`] this recording has left to spend on the
    /// content its files already held when it first saw them.
    backfill: u64,
}

/// The writer is gone; there is nothing to record into.
struct Gone;

impl State {
    /// How long to wait before looking again.
    ///
    /// The poll interval, until a session turns out to have no transcript to
    /// follow at all — an id that was archived out of the lookup, or a
    /// `--session-id` claude never wrote under. Resolving costs a scan of both
    /// roots' project directories, so a session in that state is backed off to
    /// once every ten seconds rather than left scanning for the rest of its
    /// life. A resident's transcript normally appears within a turn, and one
    /// sweep of the ordinary interval catches it.
    fn poll_after(&self) -> Duration {
        POLL * self.misses.clamp(1, 10)
    }

    /// Copy whatever the session's files have gained.
    async fn sweep(&mut self) -> Result<(), Gone> {
        let Some((root, transcript)) = self.locate().await else {
            self.first = false;
            self.misses = self.misses.saturating_add(1);
            return Ok(());
        };
        self.misses = 0;
        self.rewatch(&transcript).await;

        self.follow_lines(root, &transcript, "main").await?;

        // The subagent directory: one transcript and one meta per child agent,
        // and both matter — the transcript is what the drill-down renders, the
        // meta is what links it to the `Task` call it hangs off. The walk is the
        // one the session page uses, so a workflow's agents (one level down)
        // are recorded too rather than silently skipped.
        for file in transcript.subagent_files().await {
            match file.kind {
                SubagentFileKind::Transcript => {
                    let label = format!("agent:{}", file.agent_id);
                    self.follow_lines(root, &file.path, &label).await?;
                }
                SubagentFileKind::Meta => {
                    let label = format!("agent-meta:{}", file.agent_id);
                    self.snapshot(root, &file.path, &label).await?;
                }
            }
        }

        // The checklist, which lives beside the config root rather than beside
        // the transcript, and is rewritten in place as tasks change state.
        let session = self.session();
        if let Some(tasks) = crate::sessions::checklist::tasks_dir(&self.tail.paths, root, &session) {
            for entry in list(&tasks).await {
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(stem) = name.strip_suffix(".json") {
                    let label = format!("task:{stem}");
                    self.snapshot(root, &entry.path(), &label).await?;
                }
            }
        }
        self.first = false;
        Ok(())
    }

    /// The id the recorder is currently following.
    fn session(&self) -> String {
        self.tail.follow.lock().expect("recorder follow poisoned").clone()
    }

    /// Where this session's transcript is, re-resolving only when it has to.
    ///
    /// The lookup walks both roots' project directories, which is far too much
    /// to do once a second per live session — but it has to be redone when
    /// claude renames the session under us, and when the file it found has gone
    /// (a transcript that has not been written yet is the everyday case: a
    /// fresh session has no file until its first turn).
    ///
    /// It is [`crate::sessions::claude::find_claude_session`] rather than a
    /// scan of its own, so that the recorder follows exactly the file the rest
    /// of the server calls this session's transcript. One consequence worth
    /// knowing: an *archived* session is invisible to that lookup, so archiving
    /// a session that still has a resident stops its recording gaining
    /// transcript lines until it is unarchived.
    async fn locate(&mut self) -> Option<(ClaudeRoot, PathBuf)> {
        let session = self.session();
        if let Some((known, root, path)) = &self.resolved {
            if *known == session && tokio::fs::try_exists(path).await.unwrap_or(false) {
                return Some((*root, path.clone()));
            }
        }
        let found = crate::sessions::claude::find_claude_session(&self.tail.paths, &session).await?;
        self.resolved = Some((session, found.root, found.file_path.clone()));
        Some((found.root, found.file_path))
    }

    /// Point the watcher at the directories this session writes under.
    ///
    /// Both are watched recursively, which over-reports — every other session
    /// in the same project wakes this one — and that is the right trade: a
    /// spurious wake costs a sweep that finds nothing changed, while a missed
    /// one costs up to a second of timestamp accuracy in the recording.
    async fn rewatch(&mut self, transcript: &Path) {
        let Some(project) = transcript.parent().map(Path::to_path_buf) else {
            return;
        };
        let mut dirs = vec![project];
        if let Some(tasks) = self
            .resolved
            .as_ref()
            .and_then(|(session, root, _)| crate::sessions::checklist::tasks_dir(&self.tail.paths, *root, session))
        {
            if tokio::fs::try_exists(&tasks).await.unwrap_or(false) {
                dirs.push(tasks);
            }
        }
        if self.watching.as_deref() == Some(dirs.as_slice()) {
            return;
        }
        self.events = watch::watch_dirs(&dirs, DEBOUNCE);
        self.watching = Some(dirs);
    }

    /// Record every complete line `path` has gained since the last sweep.
    async fn follow_lines(&mut self, root: ClaudeRoot, path: &Path, label: &str) -> Result<(), Gone> {
        let Some(rel) = self.relative(root, path) else { return Ok(()) };
        let key = format!("{}/{rel}", root.as_str());
        let mut offset = match self.cursors.lines.get(&key).copied() {
            Some(offset) => offset,
            None => self.start_of(path).await,
        };

        let Ok(mut file) = tokio::fs::File::open(path).await else {
            return Ok(());
        };
        let len = file.metadata().await.map(|meta| meta.len()).unwrap_or(0);
        // Written down before anything is read, and on every path out of this
        // function. Leaving a resolved offset unstored is how a file that was
        // deliberately picked up at its *end* gets picked up from the start on
        // the very next sweep — recording the history the cursor existed to
        // skip, twice.
        self.cursors.lines.insert(key.clone(), offset);
        self.dirty = true;
        if len < offset {
            // Shorter than it was: the file was replaced or truncated, which no
            // CLI does to a transcript but a restore or a manual edit will.
            // Starting again is the only reading that cannot silently splice
            // two different files together.
            self.emit(Event::Note {
                text: format!("{rel} shrank from {offset} to {len} bytes; following it from the start again"),
            })?;
            offset = 0;
            self.cursors.lines.insert(key.clone(), offset);
        }
        if len == offset {
            return Ok(());
        }
        if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
            return Ok(());
        }
        let mut buffer = Vec::new();
        if (&mut file).take(MAX_CHUNK).read_to_end(&mut buffer).await.is_err() {
            return Ok(());
        }
        // Only whole lines: the tail of a transcript being written right now is
        // half an object, and recording it would make a recording that cannot
        // be parsed as one.
        let Some(end) = buffer.iter().rposition(|byte| *byte == b'\n') else {
            return Ok(());
        };
        let consumed = end + 1;
        for line in buffer[..consumed].split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            // Lossy, because a recording is JSON and JSON is UTF-8. A
            // transcript line that is not valid UTF-8 is already unrenderable
            // — [`crate::sessions::util::LossyLines`] reads it exactly this way
            // — so this loses nothing the Session page would have shown.
            self.emit(Event::Transcript {
                file: label.to_string(),
                root: root.as_str().to_string(),
                path: rel.clone(),
                line: String::from_utf8_lossy(line).into_owned(),
            })?;
        }
        self.cursors.lines.insert(key, offset + consumed as u64);
        Ok(())
    }

    /// Record a whole file, if it is not the one already recorded.
    async fn snapshot(&mut self, root: ClaudeRoot, path: &Path, label: &str) -> Result<(), Gone> {
        let Some(rel) = self.relative(root, path) else { return Ok(()) };
        let key = format!("{}/{rel}", root.as_str());
        let Ok(meta) = tokio::fs::metadata(path).await else { return Ok(()) };
        let stamp = format!("{}:{}", meta.len(), mtime_ms(&meta));
        if self.cursors.snapshots.get(&key) == Some(&stamp) {
            return Ok(());
        }
        // Skipped rather than seeded: a snapshot is idempotent — replay writes
        // the file whole — so re-recording one that a previous run already had
        // costs a duplicate line and nothing else. That is a far better trade
        // than a checklist or an agent meta that never makes it into the
        // recording at all.
        let Ok(content) = tokio::fs::read(path).await else { return Ok(()) };
        self.emit(Event::Sidecar {
            file: label.to_string(),
            root: root.as_str().to_string(),
            path: rel,
            content: String::from_utf8_lossy(&content).into_owned(),
        })?;
        self.cursors.snapshots.insert(key, stamp);
        self.dirty = true;
        Ok(())
    }

    /// Where a file this tailer has never seen should be picked up.
    ///
    /// Two rules, and the first is the older one. A recording that already has
    /// content but no cursors cannot know what it holds, so its first sweep
    /// starts every file at the *end*: recording a duplicate would be silently
    /// wrong, and recording a gap is loudly incomplete.
    ///
    /// The second is a budget, and it is what stops a recorder switched on over
    /// a long session from spending its whole ceiling before it has witnessed
    /// anything. A recording's job is what happens from now on; the history is
    /// already on disk in the very file being read, and `replay` can be pointed
    /// at it. But a recording that begins in the middle of a sentence is hard
    /// to read, so a bounded tail of each file comes along — [`MAX_FILE_BACKFILL`]
    /// per file and [`MAX_BACKFILL`] over all of them — and everything older
    /// than that is announced rather than copied.
    ///
    /// The cut is moved forward to the next line boundary. A transcript is one
    /// JSON object per line and a recording that began mid-object would not
    /// parse as one.
    async fn start_of(&mut self, path: &Path) -> u64 {
        let len = tokio::fs::metadata(path).await.map(|meta| meta.len()).unwrap_or(0);
        if len == 0 {
            return 0;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if self.first && !self.tail.fresh && !self.seeded {
            let _ = self.emit(Event::Note {
                text: format!("no cursor file for a recording that already has content; {len} bytes of {name} were not recorded"),
            });
            return len;
        }
        let cut = budgeted_start(len, self.backfill);
        if cut == 0 {
            self.backfill = self.backfill.saturating_sub(len);
            return 0;
        }
        let start = line_start(path, cut, len).await;
        self.backfill = self.backfill.saturating_sub(len - start);
        let _ = self.emit(Event::Note {
            text: format!(
                "{name} already held {len} bytes; recording its last {} and leaving the {start} before them on disk",
                len - start
            ),
        });
        start
    }

    /// A file's path relative to its Claude config root.
    ///
    /// Root-relative rather than absolute, because that is exactly what replay
    /// needs: it rebuilds the same tree under a different `$HOME`, and a path
    /// carrying this machine's home directory would be useless there — as well
    /// as a login name in a file meant to be shared with a bug report.
    fn relative(&self, root: ClaudeRoot, path: &Path) -> Option<String> {
        let base = self.tail.paths.claude_root(root);
        let rel = path.strip_prefix(&base).ok()?;
        Some(rel.to_string_lossy().replace('\\', "/"))
    }

    fn emit(&self, event: Event) -> Result<(), Gone> {
        self.tail
            .tx
            .send(Line {
                v: SCHEMA,
                at: iso8601(SystemTime::now()),
                event,
            })
            .map_err(|_| Gone)
    }

    /// Write the cursors, if they moved.
    ///
    /// Best effort and unsynced: losing them costs a note and a gap in one
    /// recording, which is a far smaller thing than an `fsync` on every sweep
    /// of every live session.
    async fn persist(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.seeded = true;
        match serde_json::to_vec(&self.cursors) {
            Ok(encoded) => {
                if let Err(error) = tokio::fs::write(&self.tail.offsets, encoded).await {
                    tracing::debug!(path = %self.tail.offsets.display(), %error, "cannot write a recording's cursors");
                }
            }
            Err(error) => tracing::warn!(%error, "cannot encode a recording's cursors"),
        }
    }
}

/// The start of the first whole line at or after `cut`.
///
/// `len` — the end of the file — is the answer when there is no line boundary
/// to be had, which means a single line longer than [`MAX_CHUNK`]. Recording
/// half of it would be worse than recording none of it, and the next sweep
/// picks the file up whole from there.
async fn line_start(path: &Path, cut: u64, len: u64) -> u64 {
    if cut == 0 {
        return 0;
    }
    let Ok(mut file) = tokio::fs::File::open(path).await else { return len };
    if file.seek(std::io::SeekFrom::Start(cut)).await.is_err() {
        return len;
    }
    let mut buffer = Vec::new();
    if file.take(MAX_CHUNK).read_to_end(&mut buffer).await.is_err() {
        return len;
    }
    match buffer.iter().position(|byte| *byte == b'\n') {
        Some(offset) => cut + offset as u64 + 1,
        None => len,
    }
}

/// Read the cursors, treating anything unreadable as none.
async fn load(path: &Path) -> Cursors {
    let Ok(raw) = tokio::fs::read(path).await else {
        return Cursors::default();
    };
    serde_json::from_slice(&raw).unwrap_or_default()
}

/// The entries of a directory that may not exist.
async fn list(dir: &Path) -> Vec<tokio::fs::DirEntry> {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return vec![];
    };
    let mut found = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        found.push(entry);
    }
    // Stable order, so a recording of two agents that appeared in one sweep
    // reads the same on every machine.
    found.sort_by_key(|entry| entry.file_name());
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{read_log, Recorders};
    use crate::sessions::transcript::{subagent_dir, subagent_path};
    use std::io::Write;

    /// A session tree under a temporary home, as the CLI would leave it.
    struct Session {
        home: tempfile::TempDir,
        paths: Arc<Paths>,
        dirs: crate::instance::InstanceDirs,
        transcript: PathBuf,
        id: String,
    }

    fn session(id: &str) -> Session {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let dirs = crate::instance::InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
        dirs.create().expect("instance dirs");
        let project = paths.claude_managed_projects().join("-w-repo");
        std::fs::create_dir_all(&project).expect("project dir");
        let transcript = project.join(format!("{id}.jsonl"));
        std::fs::write(&transcript, b"").expect("transcript");
        Session {
            home,
            paths,
            dirs,
            transcript,
            id: id.to_string(),
        }
    }

    impl Session {
        fn append(&self, line: &str) {
            let mut file = std::fs::OpenOptions::new().append(true).open(&self.transcript).expect("open");
            writeln!(file, "{line}").expect("write");
        }

        fn log(&self) -> PathBuf {
            self.dirs.session_logs().join(format!("{}.ndjson", self.id))
        }

        fn recorded(&self) -> Vec<Event> {
            read_log(&self.log()).unwrap_or_default().into_iter().map(|line| line.event).collect()
        }
    }

    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..600 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    }

    /// The whole point: what the CLI writes to disk lands in the recording,
    /// subagents and their metas included, and a half-written line waits.
    #[tokio::test]
    async fn transcripts_subagents_and_metas_all_reach_the_recording() {
        let session = session("11111111-1111-4111-8111-111111111111");
        let recorders = Recorders::new_at(session.dirs.session_logs(), None, session.paths.clone());
        // Held for the whole test: dropping the handle is what stops the tailer.
        let _recorder = recorders.open(&session.id);

        session.append(r#"{"type":"user","n":1}"#);
        until("the first line", || {
            session
                .recorded()
                .iter()
                .any(|event| matches!(event, Event::Transcript { line, .. } if line.contains("\"n\":1")))
        })
        .await;

        // A subagent appearing mid-session is picked up without anything
        // telling the tailer it exists.
        let agent = subagent_path(&session.transcript, "abc123");
        std::fs::create_dir_all(agent.parent().expect("subagents dir")).expect("mkdir");
        std::fs::write(&agent, "{\"type\":\"assistant\",\"agent\":true}\n").expect("agent transcript");
        std::fs::write(
            agent.with_file_name("agent-abc123.meta.json"),
            br#"{"agentType":"Explore","toolUseId":"toolu_1"}"#,
        )
        .expect("agent meta");

        until("the subagent's transcript and meta", || {
            let events = session.recorded();
            events
                .iter()
                .any(|event| matches!(event, Event::Transcript { file, .. } if file == "agent:abc123"))
                && events
                    .iter()
                    .any(|event| matches!(event, Event::Sidecar { file, .. } if file == "agent-meta:abc123"))
        })
        .await;

        // The meta has no trailing newline, which is exactly why it is a
        // snapshot rather than a line stream.
        let events = session.recorded();
        let meta = events
            .iter()
            .find_map(|event| match event {
                Event::Sidecar { file, content, path, .. } if file == "agent-meta:abc123" => Some((content.clone(), path.clone())),
                _ => None,
            })
            .expect("the meta");
        assert!(meta.0.ends_with('}'), "a snapshot is the file, verbatim: {:?}", meta.0);
        assert!(
            meta.1.starts_with("projects/-w-repo/"),
            "paths are relative to the config root: {}",
            meta.1
        );

        // An agent a workflow filed one level down is followed too: the
        // recorder walks the directory exactly as the session page does.
        let nested = subagent_dir(&session.transcript).join("workflows").join("agent-wf1.jsonl");
        std::fs::create_dir_all(nested.parent().expect("workflows dir")).expect("mkdir");
        std::fs::write(&nested, "{\"type\":\"assistant\",\"workflow\":true}\n").expect("nested transcript");
        std::fs::write(nested.with_file_name("agent-wf1.meta.json"), br#"{"agentType":"Explore"}"#).expect("nested meta");
        until("the nested agent's transcript and meta", || {
            let events = session.recorded();
            events
                .iter()
                .any(|event| matches!(event, Event::Transcript { file, .. } if file == "agent:wf1"))
                && events
                    .iter()
                    .any(|event| matches!(event, Event::Sidecar { file, .. } if file == "agent-meta:wf1"))
        })
        .await;

        // A line the CLI has not finished writing is not recorded in half.
        let before = session.recorded().len();
        {
            let mut file = std::fs::OpenOptions::new().append(true).open(&session.transcript).expect("open");
            file.write_all(br#"{"type":"assistant","half":"#).expect("write");
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(session.recorded().len(), before, "a half-written line waits for its newline");

        // …and arrives whole once it is finished.
        session.append(r#"true}"#);
        until("the completed line", || {
            session
                .recorded()
                .iter()
                .any(|event| matches!(event, Event::Transcript { line, .. } if line.contains("\"half\":true")))
        })
        .await;
        drop(session.home);
    }

    /// The reason the cursor file exists: a second recorder over the same
    /// recording must not copy the transcript again.
    #[tokio::test]
    async fn a_second_run_over_one_recording_does_not_repeat_the_transcript() {
        let session = session("22222222-2222-4222-8222-222222222222");
        let recorders = Recorders::new_at(session.dirs.session_logs(), None, session.paths.clone());

        let first = recorders.open(&session.id);
        session.append(r#"{"n":1}"#);
        until("the first run to record", || {
            session
                .recorded()
                .iter()
                .filter(|event| matches!(event, Event::Transcript { .. }))
                .count()
                == 1
        })
        .await;
        drop(first);
        // The tailer stops with the last handle; give it the sweep it is owed.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let _second = recorders.open(&session.id);
        session.append(r#"{"n":2}"#);
        until("the second run to record", || {
            session
                .recorded()
                .iter()
                .filter(|event| matches!(event, Event::Transcript { .. }))
                .count()
                == 2
        })
        .await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        let lines: Vec<String> = session
            .recorded()
            .into_iter()
            .filter_map(|event| match event {
                Event::Transcript { line, .. } => Some(line),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 2, "the second run carried on rather than starting again: {lines:?}");
        assert!(lines[0].contains("\"n\":1") && lines[1].contains("\"n\":2"), "{lines:?}");
        drop(session.home);
    }

    /// The budget's arithmetic, without any files.
    #[test]
    fn the_history_budget_is_spent_per_file_and_then_shared() {
        assert_eq!(budgeted_start(500, MAX_BACKFILL), 0, "a short file is taken whole");
        assert_eq!(
            budgeted_start(MAX_FILE_BACKFILL * 4, MAX_BACKFILL),
            MAX_FILE_BACKFILL * 3,
            "and a long one contributes only its last {MAX_FILE_BACKFILL} bytes"
        );
        // The shared pool binds before the per-file one when it is nearly gone:
        // the hundred-and-seventieth subagent of a session gets what is left,
        // which may be nothing at all.
        assert_eq!(budgeted_start(MAX_FILE_BACKFILL * 4, 1024), MAX_FILE_BACKFILL * 4 - 1024);
        assert_eq!(
            budgeted_start(MAX_FILE_BACKFILL * 4, 0),
            MAX_FILE_BACKFILL * 4,
            "an exhausted budget starts at the end"
        );
    }

    /// A recorder switched on over a session that already has a large
    /// transcript takes a readable tail of it and leaves the rest on disk.
    ///
    /// The measured failure this replaces: 228 MB copied in the first minute of
    /// a recording whose ceiling is 256 MiB, for a session that then wrote
    /// 0.1 MB a minute. The history was never lost — it is in the transcript
    /// this very function reads — but the recording had spent its whole life
    /// on it before witnessing a single turn.
    #[tokio::test]
    async fn a_fresh_recording_over_a_long_transcript_keeps_a_tail_and_says_what_it_left() {
        let session = session("44444444-4444-4444-8444-444444444444");
        // Comfortably past the per-file allowance, in lines big enough that the
        // cut lands inside one and the boundary logic has to do something.
        let filler = "x".repeat(64 * 1024);
        let lines = (MAX_FILE_BACKFILL / (64 * 1024) * 2) as usize;
        for n in 0..lines {
            session.append(&format!(r#"{{"n":{n},"pad":"{filler}"}}"#));
        }
        let recorders = Recorders::new_at(session.dirs.session_logs(), None, session.paths.clone());
        let _recorder = recorders.open(&session.id);

        until("the note about the history left behind", || {
            session
                .recorded()
                .iter()
                .any(|event| matches!(event, Event::Note { text } if text.contains("before them on disk")))
        })
        .await;
        // A line written after the recorder started must arrive whatever the
        // budget did to the history in front of it.
        session.append(r#"{"n":"live"}"#);
        until("the line after the cut", || {
            session
                .recorded()
                .iter()
                .any(|event| matches!(event, Event::Transcript { line, .. } if line.contains("\"live\"")))
        })
        .await;

        let events = session.recorded();
        let recorded: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                Event::Transcript { line, .. } => Some(line.clone()),
                _ => None,
            })
            .collect();
        let copied: usize = recorded.iter().map(|line| line.len() + 1).sum();
        assert!(
            copied <= MAX_FILE_BACKFILL as usize + 64 * 1024 + 64,
            "the tail is bounded by the allowance plus the line the cut fell inside: {copied}"
        );
        assert!(
            recorded.iter().all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()),
            "the cut moved forward to a line boundary, so nothing was recorded in half"
        );
        assert!(
            !recorded.iter().any(|line| line.starts_with(r#"{"n":0,"#)),
            "the oldest history was left on disk"
        );
        assert!(
            recorded.iter().any(|line| line.starts_with(&format!(r#"{{"n":{},"#, lines - 1))),
            "and the newest history came along, so the recording opens mid-conversation rather than mid-sentence"
        );
        drop(session.home);
    }

    /// A recording with history and no cursors cannot know what was recorded,
    /// so it says so rather than duplicating.
    #[tokio::test]
    async fn a_lost_cursor_file_costs_a_note_rather_than_a_duplicate() {
        let session = session("33333333-3333-4333-8333-333333333333");
        let recorders = Recorders::new_at(session.dirs.session_logs(), None, session.paths.clone());
        session.append(r#"{"already":"there"}"#);
        // A recording that exists, with no cursors beside it.
        std::fs::create_dir_all(session.dirs.session_logs()).expect("dir");
        std::fs::write(
            session.log(),
            b"{\"v\":1,\"at\":\"2026-01-01T00:00:00.000Z\",\"kind\":\"note\",\"text\":\"an earlier run\"}\n",
        )
        .expect("seed");

        let _recorder = recorders.open(&session.id);
        // The gap is announced by the first sweep, and only what the CLI writes
        // *after* that sweep can be recorded — so the test waits for it rather
        // than racing it.
        until("the gap to be announced", || {
            session
                .recorded()
                .iter()
                .any(|event| matches!(event, Event::Note { text } if text.contains("were not recorded")))
        })
        .await;
        session.append(r#"{"after":"the gap"}"#);
        until("the line after the gap", || {
            session
                .recorded()
                .iter()
                .any(|event| matches!(event, Event::Transcript { line, .. } if line.contains("the gap")))
        })
        .await;

        let events = session.recorded();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::Note { text } if text.contains("were not recorded"))),
            "the gap is announced: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Transcript { line, .. } if line.contains("already"))),
            "and the history that could not be placed is not recorded twice"
        );
        drop(session.home);
    }
}
