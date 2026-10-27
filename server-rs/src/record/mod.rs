//! The session flight recorder: everything one claude session lived through,
//! written down so it can be lived through again.
//!
//! The Session page renders a transcript that arrives in a dozen shapes — text,
//! thinking, tool calls, background notifications, subagent drill-downs — and
//! every one of them is the CLI's business rather than ours. Iterating on that
//! rendering against a *live* session is the slowest loop in this repository:
//! the interesting states (a turn that stalls, a fork mid-crash, four subagents
//! writing at once) take minutes to provoke, arrive once, and cannot be asked
//! for again.
//!
//! So the session is recorded. One append-only NDJSON file per claude session:
//!
//! ```text
//! <data>/instances/<name>/session-logs/<session-id>.ndjson
//! ```
//!
//! …holding every line written to claude, every line claude wrote back, the
//! lifecycle around them, and — the part the renderer actually eats — every
//! line the CLI appended to its transcript files, the `subagents/agent-*.jsonl`
//! of child agents included. [`replay`] turns that back into a filesystem on a
//! throwaway `$HOME`, so a second server pointed at it renders a real session
//! through the real pipeline, at the speed it originally happened.
//!
//! # What is *not* in it, and why the rest can be verbatim
//!
//! An account's token reaches claude in exactly one place: the store hands out
//! an [`AccountEnv`](crate::accounts::store::AccountEnv) — a
//! `CLAUDE_CODE_OAUTH_TOKEN` or an `ANTHROPIC_API_KEY` — which
//! [`crate::process_env`] puts in an [`Env`](crate::process_env::Env) that the
//! spawn applies over an `env_clear`. It is never written to a stream, never on
//! a command line, and never in `runs.json`. The recorder cannot record it even
//! by accident — [`SpawnSummary`], the only way a spawn is described to this
//! module, has no field that could carry an environment, and the fingerprint it
//! does carry is narrowed to the three command-line settings (model, effort,
//! permission) on the way in.
//!
//! What is left is stdio, and stdio is recorded **verbatim**, deliberately:
//!
//! * stdin is the user's own prompt and the `control_request` JSON this server
//!   composes. Neither has ever held a credential.
//! * stdout is claude's stream-json, which is the same content the CLI is
//!   concurrently writing — unredacted — into `<project>/<id>.jsonl`. The
//!   recorder copies those very files a few lines further down.
//!
//! So [`redact_secrets`](crate::process_env::redact_secrets) is *not* applied
//! here, and that is a decision rather than an oversight: it would buy nothing
//! (the same bytes sit in the transcript two directories away, which is what a
//! recording is *for*) and it would cost the one property replay depends on —
//! that a rebuilt transcript is byte-for-byte the file the CLI wrote. A
//! transcript with `sk-ant-…[redacted]` spliced into it is a transcript that
//! renders differently from the real one, which defeats the purpose. The
//! redaction still guards every path that puts child output into an *API
//! response*; a file under the instance directory is not one of those.
//!
//! # Failure is always silence
//!
//! Nothing in here may cost a session. Every write goes to a task behind an
//! unbounded channel, so a slow disk cannot back-pressure the driver's pipe
//! loop; every failure — a directory that will not create, a file that will not
//! open, a session id that does not look like one — degrades to
//! [`Recorder::disabled`], which is a handle whose methods do nothing at all.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::instance::InstanceDirs;
use crate::sessions::{iso8601, Paths};

mod tail;

pub mod replay;

/// The schema version every line carries.
///
/// On the line rather than in a header, because the file is *appended to* by
/// every server that ever drives the session: there is no one place a header
/// could live that a later build would not have to migrate.
pub const SCHEMA: u8 = 1;

/// How much of a free-text detail is worth keeping.
///
/// The tombstone of a dead claude carries its stderr tail, which can be a whole
/// stack trace; the point of the field is to say what happened, not to be a
/// second copy of the log.
const DETAIL_CAP: usize = 4_000;

/// How much *replayed* stdout one attach may add to a recording.
///
/// The shim's ring holds a hundred mebibytes of claude's stdout so that a
/// server upgrade cannot truncate a turn ([`crate::inject::shim`]), and every
/// re-attach replays all of it. That history is worth having in a recording
/// that has none of its own — but the usual case is a recording that was
/// written by the server before this one and already holds every one of those
/// lines, and re-recording them per restart would spend the whole ceiling on
/// duplicates and stop the recording for good. Eight mebibytes is enough of the
/// reconstruction to read; past it a `note` says what was dropped.
const REPLAY_BUDGET: u64 = 8 * 1024 * 1024;

/// The default per-session ceiling, in mebibytes.
///
/// A recording is roughly the size of the transcript it mirrors plus the
/// stream-json that produced it, so a long session is tens of megabytes and a
/// runaway one — an agent in a tool loop — is unbounded. 256 MiB is far past
/// any session worth replaying and far short of filling a disk.
const DEFAULT_MAX_MB: u64 = 256;

// ------------------------------------------------------------------ the schema

/// One line of a recording.
///
/// Internally tagged on `kind` and flattened under [`Line`], so the file reads
/// as one flat object per line — which is what makes `jq` and a human eye the
/// two tools you need to debug one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A `claude` was started for this session.
    Spawned {
        /// `fresh`, `resume` or `fork` — which session selector was on the
        /// command line.
        selector: String,
        /// The CLI's pid.
        pid: u32,
        /// The shim holding its pipes.
        shim: String,
        /// Where claude runs.
        ///
        /// The project directory a transcript lands in is derived from this by
        /// the CLI, which is what makes it worth recording: it is how a person
        /// reading a recording knows which repository they are looking at.
        /// Replay does *not* re-derive anything from it — every
        /// [`Event::Transcript`] carries the path the CLI actually chose, which
        /// is the only version that survives a fork.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// `--model`, as asked for at spawn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// `--effort`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        /// `--permission-mode`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission: Option<String>,
    },
    /// This server took over a `claude` it did not start.
    Attached {
        /// The CLI's pid, as the journal recorded it.
        pid: u32,
        /// The shim socket the attach went through.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shim: Option<String>,
        /// The journal's turn mark: how far into claude's stdout the prompt of
        /// an unfinished turn was written, or absent if none was in flight.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_at: Option<u64>,
        /// How many messages the record says were still waiting.
        queued: usize,
        /// How much stdout the shim is about to replay.
        ///
        /// The count is here rather than left to be counted off the
        /// [`Event::Stdout`] lines that follow, because only the first
        /// [`REPLAY_BUDGET`] of them are recorded a second time.
        replay_bytes: u64,
    },
    /// A line written to claude's stdin: a user message, or a control request.
    Stdin {
        /// The line, exactly as claude read it.
        line: String,
    },
    /// A line claude wrote, JSON or not.
    Stdout {
        /// The line, exactly as claude wrote it.
        line: String,
        /// True while the shim is replaying stdout this server did not witness.
        /// Such a line already happened; nothing acted on it the second time.
        replayed: bool,
    },
    /// The shim's replay ring overflowed: stdout between these two offsets is
    /// gone for good.
    Gap {
        /// The offset the lost run starts at.
        from: u64,
        /// The offset it ends at.
        to: u64,
    },
    /// Something happened to the session that is not a line of stdio.
    Lifecycle {
        /// What happened.
        event: Stage,
        /// Which rung of the interrupt ladder, for [`Stage::Escalate`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rung: Option<u8>,
        /// Which selector a restart used, for [`Stage::Recover`] and
        /// [`Stage::Respawn`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        /// One short line of why.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// One line the CLI appended to a transcript file on disk.
    ///
    /// This is the payload replay exists for: the renderer reads transcripts,
    /// not stream-json, and a session's subagents write files of their own that
    /// the Session page drills into.
    Transcript {
        /// `main` for the session's own transcript, `agent:<id>` for a
        /// subagent's.
        file: String,
        /// Which Claude config root it lives under: `local` or `managed`.
        root: String,
        /// The path relative to that root —
        /// `projects/<project-dir>/<id>.jsonl`, or
        /// `projects/<project-dir>/<id>/subagents/agent-<id>.jsonl`.
        ///
        /// Recorded rather than derived, because the derivation is the CLI's:
        /// which project directory a session lands in, and which id it is
        /// written under after a fork, are decisions replay has no way to
        /// reproduce from a working directory alone.
        path: String,
        /// The line, without its newline.
        line: String,
    },
    /// A whole file the CLI rewrites rather than appends to.
    ///
    /// The agent metas — a single line with no trailing newline, so a
    /// line-follower would never see one — and the `tasks/<id>/*.json`
    /// checklist, which is rewritten in place as a task's status changes.
    /// Replay writes these whole, which makes re-recording one harmless.
    Sidecar {
        /// `agent-meta:<agent-id>` or `task:<n>`.
        file: String,
        /// Which Claude config root it lives under: `local` or `managed`.
        root: String,
        /// The path relative to that root.
        path: String,
        /// The file, verbatim.
        content: String,
    },
    /// The recorder talking about itself.
    Note {
        /// What it has to say.
        text: String,
    },
}

/// The lifecycle moments worth a line of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// A message was written and a turn opened for it.
    TurnOpen,
    /// A turn ended, however it ended.
    Result,
    /// A turn has produced nothing for long enough to be marked. Nothing is
    /// killed for it — see [`crate::inject::resident`].
    Stalled,
    /// Stop was pressed: claude was asked to end the turn.
    Interrupt,
    /// A queued message was written into claude's stdin ahead of its turn, so
    /// that it runs the moment the turn in flight ends.
    ///
    /// The `stdin` line beside this one is the prompt itself; this says *why* a
    /// prompt appears mid-turn, which nothing else in the recording could.
    Steer,
    /// What became of a steered message once the turn it was handed to ended.
    ///
    /// The CLI has two answers and does not say which it chose, so the driver
    /// has to watch for it — see [`crate::inject::resident::Driver::settle_delivered`].
    /// Either claude opened a turn of its own for the message, or it had already
    /// folded the message into the turn that was running and nothing more is
    /// coming. This is the line that says which happened, and it is the one to
    /// read first when a session appears to be working on nothing.
    Absorb,
    /// The polite request went unanswered and a signal was sent.
    Escalate,
    /// The process is being replaced because the settings changed.
    Respawn,
    /// The process died and is being brought back.
    Recover,
    /// A limit ended the turn and the session was moved to another account —
    /// or had nowhere to go, and its queue is held. See
    /// [`crate::accounts::rotation`].
    Rotate,
    /// claude answered under a different session id and everything followed it.
    Adopt,
    /// claude stopped to ask the user something (`AskUserQuestion`), or what
    /// became of it: answered, dismissed, withdrawn, or left behind by a process
    /// that went. See [`crate::inject::questions`].
    Question,
    /// claude asked for a permission and was refused. Every prompt but a
    /// question is: remote-hand grants nothing interactively.
    Permission,
    /// A re-attached driver finished reading the history and decided what it
    /// had inherited — in particular, which of the background tasks the replay
    /// reconstructed it is willing to believe in.
    Settle,
    /// The process is gone and the driver is winding up.
    Exit,
}

/// One line of the file: the envelope every event shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// [`SCHEMA`].
    pub v: u8,
    /// When it was recorded, ISO-8601 UTC with milliseconds.
    pub at: String,
    /// What happened.
    #[serde(flatten)]
    pub event: Event,
}

/// What a spawn tells the recorder about itself.
///
/// A struct rather than a pile of arguments for one reason worth stating: it is
/// the type boundary that makes leaking an account token impossible. The caller
/// holds a `Recipe`, which carries the whole environment; this holds the three
/// command-line settings and nothing that could contain a credential, so the
/// narrowing happens at the call site and is visible there.
#[derive(Debug, Clone, Copy)]
pub struct SpawnSummary<'a> {
    /// `fresh`, `resume` or `fork`.
    pub selector: &'a str,
    /// The CLI's pid.
    pub pid: u32,
    /// The shim id holding its pipes.
    pub shim: &'a str,
    /// Where claude runs.
    pub cwd: Option<&'a Path>,
    /// `--model`.
    pub model: Option<&'a str>,
    /// `--effort`.
    pub effort: Option<&'a str>,
    /// `--permission-mode`.
    pub permission: Option<&'a str>,
}

// ------------------------------------------------------------- the factory

/// Where recordings go, and whether they are made at all.
///
/// Built once per server from the instance directory, and read from the
/// environment there rather than per session: `DROPPEN_SESSION_LOG=0` turns
/// the whole thing off, and `DROPPEN_SESSION_LOG_MAX_MB` moves the ceiling (`0`
/// removes it). Their old names, `RH_SESSION_LOG*`, are still read.
#[derive(Debug, Clone)]
pub struct Recorders {
    /// `None` when recording is switched off.
    dir: Option<PathBuf>,
    /// The per-session ceiling in bytes; `None` for no ceiling.
    cap: Option<u64>,
    /// Where transcripts are discovered, for the tailer.
    paths: Arc<Paths>,
}

impl Recorders {
    /// Read the configuration and name the directory recordings live in.
    pub fn new(paths: Arc<Paths>, instance: &InstanceDirs) -> Self {
        let on = crate::env::flag_or("DROPPEN_SESSION_LOG", true);
        let max_mb = crate::env::var("DROPPEN_SESSION_LOG_MAX_MB")
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_MB);
        Self {
            dir: on.then(|| instance.session_logs()),
            cap: (max_mb > 0).then(|| max_mb * 1024 * 1024),
            paths,
        }
    }

    /// A factory that records nothing, for tests and for callers with no
    /// instance directory.
    pub fn off(paths: Arc<Paths>) -> Self {
        Self { dir: None, cap: None, paths }
    }

    /// A factory with the configuration supplied rather than read.
    ///
    /// [`Recorders::new`] reads the process environment, which every test in
    /// this binary shares; a test that wants a ceiling of two kilobytes says so
    /// here instead of racing every other test for `DROPPEN_SESSION_LOG_MAX_MB`.
    #[cfg(test)]
    pub(crate) fn new_at(dir: PathBuf, cap: Option<u64>, paths: Arc<Paths>) -> Self {
        Self { dir: Some(dir), cap, paths }
    }

    /// Start recording `session`, appending to whatever is already there.
    ///
    /// Never fails: a recorder that cannot be opened is one that does nothing,
    /// and the caller is a driver that has a session to run either way.
    pub fn open(&self, session: &str) -> Recorder {
        let Some(dir) = &self.dir else { return Recorder::disabled() };
        // The id becomes a file name, and after a `--fork-session` it is
        // whatever the CLI announced rather than anything this server minted.
        // Refusing the odd ones is cheaper than sanitising them, and a session
        // whose id is not id-shaped has bigger problems than its recording.
        if !crate::sessions::reference::is_path_segment_id(session) {
            tracing::warn!(session = %session, "not recording a session whose id is not a plain identifier");
            return Recorder::disabled();
        }
        if let Err(error) = std::fs::create_dir_all(dir) {
            tracing::warn!(path = %dir.display(), %error, "cannot create the session-log directory; not recording");
            return Recorder::disabled();
        }
        let path = dir.join(format!("{session}.ndjson"));
        let existing = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if self.cap.is_some_and(|cap| existing >= cap) {
            tracing::debug!(path = %path.display(), "the recording for this session is already at its ceiling; not recording");
            return Recorder::disabled();
        }

        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(writer(path.clone(), rx, existing, self.cap));
        let stop = Arc::new(Stop::default());
        let follow = Arc::new(Mutex::new(session.to_string()));
        tokio::spawn(tail::run(tail::Tail {
            tx: tx.clone(),
            paths: self.paths.clone(),
            follow: follow.clone(),
            stop: stop.clone(),
            offsets: dir.join(format!("{session}.offsets.json")),
            // A recording that starts empty may copy the transcript from byte
            // zero; one that is being appended to must not, or every server
            // restart would duplicate the whole file. See [`tail::Tail::first`].
            fresh: existing == 0,
        }));
        Recorder(Some(Arc::new(Inner {
            tx,
            follow,
            stop,
            replayed: AtomicU64::new(0),
            replay_noted: AtomicBool::new(false),
        })))
    }
}

// -------------------------------------------------------------- the handle

/// The handle a driver holds. Cloning it is cloning a reference.
///
/// Every method is infallible and returns nothing: recording is a side effect
/// of running a session, never a step in it.
#[derive(Debug, Clone)]
pub struct Recorder(Option<Arc<Inner>>);

#[derive(Debug)]
struct Inner {
    tx: mpsc::UnboundedSender<Line>,
    /// The session id the transcript tailer follows, which [`Recorder::follow`]
    /// moves when claude renames the session under us.
    follow: Arc<Mutex<String>>,
    stop: Arc<Stop>,
    /// How much replayed stdout this attach has already contributed, against
    /// [`REPLAY_BUDGET`].
    replayed: AtomicU64,
    /// Whether the budget's `note` has been written; it is written once.
    replay_noted: AtomicBool,
}

impl Drop for Inner {
    /// The last handle going is what ends the tailer.
    ///
    /// Cooperative rather than an `abort`, so the tailer gets one final sweep:
    /// the most interesting lines of a session are the last ones, and they are
    /// written in the moment between claude's `result` and the driver letting go.
    fn drop(&mut self) {
        self.stop.stop();
    }
}

/// A flag with a doorbell: the tailer waits on the bell and reads the flag.
#[derive(Debug, Default)]
struct Stop {
    stopped: AtomicBool,
    wake: tokio::sync::Notify,
}

impl Stop {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        // `notify_one` rather than `notify_waiters`, because the tailer spends
        // most of its life sweeping rather than waiting, and a permit that is
        // stored is a wake-up that cannot be missed.
        self.wake.notify_one();
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

impl Recorder {
    /// A recorder that records nothing.
    pub fn disabled() -> Self {
        Self(None)
    }

    /// Whether anything is actually being written.
    pub fn is_recording(&self) -> bool {
        self.0.is_some()
    }

    fn emit(&self, event: Event) {
        let Some(inner) = &self.0 else { return };
        // The writer having gone is the ceiling having been reached, which is
        // already on record as a `note`. Dropping the line is the whole of the
        // response.
        let _ = inner.tx.send(Line {
            v: SCHEMA,
            at: iso8601(SystemTime::now()),
            event,
        });
    }

    /// A `claude` was started.
    pub fn spawned(&self, spawn: SpawnSummary<'_>) {
        self.emit(Event::Spawned {
            selector: spawn.selector.to_string(),
            pid: spawn.pid,
            shim: spawn.shim.to_string(),
            cwd: spawn.cwd.map(|dir| dir.to_string_lossy().into_owned()),
            model: spawn.model.map(str::to_owned),
            effort: spawn.effort.map(str::to_owned),
            permission: spawn.permission.map(str::to_owned),
        });
    }

    /// A `claude` from before this server was taken over.
    pub fn attached(&self, pid: u32, shim: Option<&str>, turn_at: Option<u64>, queued: usize, replay_bytes: u64) {
        self.emit(Event::Attached {
            pid,
            shim: shim.map(str::to_owned),
            turn_at,
            queued,
            replay_bytes,
        });
    }

    /// A line written to claude.
    pub fn stdin(&self, line: &str) {
        self.emit(Event::Stdin { line: line.to_string() });
    }

    /// A line claude wrote.
    ///
    /// `replayed` marks the history a shim hands back when this server attaches
    /// to a process it did not start. Those lines are recorded too — a
    /// recording that begins mid-session would otherwise have no idea what came
    /// before — but only up to [`REPLAY_BUDGET`], because the usual case is a
    /// recording that already holds every one of them from the server before
    /// this one.
    pub fn stdout(&self, line: &str, replayed: bool) {
        if replayed && !self.affordable(line.len()) {
            return;
        }
        self.emit(Event::Stdout {
            line: line.to_string(),
            replayed,
        });
    }

    /// Whether another line of replayed history is still within budget, noting
    /// the moment it stops being.
    fn affordable(&self, bytes: usize) -> bool {
        let Some(inner) = &self.0 else { return false };
        let spent = inner.replayed.fetch_add(bytes as u64, Ordering::Relaxed).saturating_add(bytes as u64);
        if spent <= REPLAY_BUDGET {
            return true;
        }
        // Exactly one note, however many lines are still to come.
        if !inner.replay_noted.swap(true, Ordering::Relaxed) {
            self.note(format!(
                "more than {REPLAY_BUDGET} bytes of replayed history arrived on attach; the rest is not being recorded again \
                 (the server that recorded it the first time already did)"
            ));
        }
        false
    }

    /// Stdout that the shim's ring buffer lost.
    pub fn gap(&self, from: u64, to: u64) {
        self.emit(Event::Gap { from, to });
    }

    /// Something happened to the session.
    pub fn lifecycle(&self, event: Stage, detail: Option<&str>) {
        self.lifecycle_with(event, None, None, detail);
    }

    /// The same, for the two stages that carry more than a sentence.
    pub fn lifecycle_with(&self, event: Stage, rung: Option<u8>, selector: Option<&str>, detail: Option<&str>) {
        self.emit(Event::Lifecycle {
            event,
            rung,
            selector: selector.map(str::to_owned),
            detail: detail.map(|text| clip(text, DETAIL_CAP)),
        });
    }

    /// The recorder's own voice.
    pub fn note(&self, text: impl Into<String>) {
        self.emit(Event::Note { text: text.into() });
    }

    /// Follow claude to the session id it chose instead.
    ///
    /// Only the *tailer* moves: the file keeps the name it was opened under, so
    /// one recording still holds one continuous session however many times the
    /// CLI renames it, and the [`Stage::Adopt`] line in it says when the rename
    /// happened.
    pub fn follow(&self, id: &str) {
        let Some(inner) = &self.0 else { return };
        *inner.follow.lock().expect("recorder follow poisoned") = id.to_string();
        // The tailer resolves the id at the top of each sweep; nudging it here
        // is what makes the switch immediate rather than one poll late.
        inner.stop.wake.notify_one();
    }
}

/// A string, shortened enough to put on one line.
fn clip(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    text.chars().take(cap).chain(['…']).collect()
}

// -------------------------------------------------------------- the writer

/// Append events to one file until the channel closes or the ceiling is hit.
///
/// Batched on purpose: a turn's stdout arrives as a burst of small lines, and a
/// `write` per line with a `flush` per line would turn a recording into the
/// session's bottleneck. Draining whatever is queued and flushing once per
/// batch keeps the file readable by a `tail -f` in the common case — where
/// events arrive one at a time and each batch is one line — without paying for
/// it in the case that is actually hot. Nothing is `fsync`ed: a recording that
/// loses its last lines to a power cut has lost the least valuable part of
/// itself, and a session that waited for a disk to say so would be paying for
/// that with latency the user can feel.
async fn writer(path: PathBuf, mut rx: mpsc::UnboundedReceiver<Line>, mut written: u64, cap: Option<u64>) {
    let file = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await;
    let mut file = match file {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "cannot open a session log; not recording");
            // Draining rather than returning: a dropped receiver makes every
            // later `send` fail, which is harmless, but the driver would then
            // hold a handle that looks alive. Closing is tidier.
            rx.close();
            return;
        }
    };

    let mut batch = String::new();
    while let Some(first) = rx.recv().await {
        batch.clear();
        encode_into(&mut batch, &first);
        // The ceiling is checked *while* the batch is built, not after it is
        // written: a session that produced ten thousand lines faster than this
        // task could drain them would otherwise write all of them and only then
        // notice, which turns a 256 MiB ceiling into whatever one burst happens
        // to be.
        let mut full = cap.is_some_and(|cap| written + batch.len() as u64 >= cap);
        while !full {
            let Ok(next) = rx.try_recv() else { break };
            encode_into(&mut batch, &next);
            full = cap.is_some_and(|cap| written + batch.len() as u64 >= cap);
        }
        if let Err(error) = file.write_all(batch.as_bytes()).await {
            tracing::warn!(path = %path.display(), %error, "cannot append to a session log; giving up on it");
            return;
        }
        let _ = file.flush().await;
        written += batch.len() as u64;

        if full {
            // The ceiling is a stop, not a rotation. A recording is only worth
            // anything from its first line — replay rebuilds a transcript by
            // appending — so throwing away the beginning to keep the end would
            // leave a file that cannot be replayed at all.
            let note = Line {
                v: SCHEMA,
                at: iso8601(SystemTime::now()),
                event: Event::Note {
                    text: format!("this recording reached its {written}-byte ceiling and stopped; raise DROPPEN_SESSION_LOG_MAX_MB to record more"),
                },
            };
            let mut last = String::new();
            encode_into(&mut last, &note);
            let _ = file.write_all(last.as_bytes()).await;
            let _ = file.flush().await;
            tracing::warn!(path = %path.display(), written, "a session recording reached its ceiling and stopped");
            rx.close();
            return;
        }
    }
    let _ = file.flush().await;
}

/// Encode one line, or drop it if it cannot be encoded.
///
/// `serde_json` fails on exactly one thing here — a string that is not valid
/// UTF-8 cannot exist in Rust — so this is unreachable in practice and written
/// as a skip rather than an `expect` on principle: nothing in the recorder may
/// panic a driver's task.
fn encode_into(batch: &mut String, line: &Line) {
    match serde_json::to_string(line) {
        Ok(encoded) => {
            batch.push_str(&encoded);
            batch.push('\n');
        }
        Err(error) => tracing::warn!(%error, "cannot encode a session-log line"),
    }
}

/// Read a recording back, skipping lines that do not parse.
///
/// Tolerant for the same reason the transcript parser is: the tail of a file
/// being appended to right now is half a line, and a recording made by an older
/// build may hold events this one has never heard of.
pub fn read_log(path: &Path) -> std::io::Result<Vec<Line>> {
    let raw = std::fs::read_to_string(path)?;
    Ok(raw.lines().filter_map(|line| serde_json::from_str::<Line>(line).ok()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait for the writer task to have caught up, without a sleep that has to
    /// be long enough for the slowest machine.
    async fn until(what: &str, mut check: impl FnMut() -> bool) {
        for _ in 0..400 {
            if check() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn recorders(home: &Path) -> (Recorders, InstanceDirs) {
        let paths = Arc::new(Paths::new(home.to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
        dirs.create().expect("instance directories");
        let recorders = Recorders::new_at(dirs.session_logs(), Some(64 * 1024), paths);
        (recorders, dirs)
    }

    #[tokio::test]
    async fn every_event_lands_as_one_flat_line_that_says_what_it_is() {
        let home = tempfile::tempdir().expect("tempdir");
        let (recorders, dirs) = recorders(home.path());
        let recorder = recorders.open("session-one");
        let log = dirs.session_logs().join("session-one.ndjson");

        recorder.spawned(SpawnSummary {
            selector: "resume",
            pid: 4242,
            shim: "shim-1",
            cwd: Some(Path::new("/w/repo")),
            model: Some("opus"),
            effort: None,
            permission: None,
        });
        recorder.stdin(r#"{"type":"user"}"#);
        recorder.stdout(r#"{"type":"result"}"#, false);
        recorder.gap(10, 20);
        recorder.lifecycle_with(Stage::Escalate, Some(2), None, Some("SIGTERM"));
        recorder.note("hello");
        recorder.attached(99, Some("shim-2"), Some(1234), 2, 5678);

        until("the writer to catch up", || read_log(&log).map(|lines| lines.len() == 7).unwrap_or(false)).await;

        let lines = read_log(&log).expect("the log parses");
        assert!(lines.iter().all(|line| line.v == SCHEMA));
        assert!(
            lines.iter().all(|line| line.at.ends_with('Z') && line.at.len() == 24),
            "every line is stamped in ISO-8601 with milliseconds: {:?}",
            lines.iter().map(|l| &l.at).collect::<Vec<_>>()
        );
        assert!(matches!(&lines[0].event, Event::Spawned { pid: 4242, .. }));
        assert!(matches!(&lines[3].event, Event::Gap { from: 10, to: 20 }));
        assert!(matches!(
            &lines[4].event,
            Event::Lifecycle {
                event: Stage::Escalate,
                rung: Some(2),
                ..
            }
        ));

        // The wire shape, rather than only the round trip: a `jq` one-liner and
        // the replay tool both read `kind` off a flat object.
        let raw = std::fs::read_to_string(&log).expect("read");
        let first: serde_json::Value = serde_json::from_str(raw.lines().next().expect("a line")).expect("json");
        assert_eq!(first["kind"], "spawned");
        assert_eq!(first["v"], 1);
        assert_eq!(first["cwd"], "/w/repo");
        assert_eq!(first["effort"], serde_json::Value::Null, "an absent setting is absent, not empty");
        let ladder: serde_json::Value = serde_json::from_str(raw.lines().nth(4).expect("a line")).expect("json");
        assert_eq!(ladder["kind"], "lifecycle");
        assert_eq!(ladder["event"], "escalate", "the stage is kebab-case on the wire");

        // An attach carries the journal's turn mark. It is the one thing about
        // an inherited session that claude's own stdout can never show — a
        // prompt that went in and has not been answered looks, on the stream,
        // exactly like a session that has been quiet — so a recording without
        // it cannot be replayed into the state the driver was actually in.
        let attach: serde_json::Value = serde_json::from_str(raw.lines().nth(6).expect("a line")).expect("json");
        assert_eq!(attach["kind"], "attached");
        assert_eq!(
            attach["turnAt"],
            serde_json::Value::Null,
            "the field is snake_case on the wire, like its neighbours"
        );
        assert_eq!(attach["turn_at"], 1234);
        assert_eq!(attach["queued"], 2);
        assert_eq!(attach["replay_bytes"], 5678);
    }

    /// The one thing that must never be in a recording. The environment is
    /// where an account's token lives, and no method on a recorder accepts one
    /// — this pins that the spawn line stays that way as the struct grows.
    #[tokio::test]
    async fn a_spawn_line_carries_settings_and_never_a_credential() {
        let home = tempfile::tempdir().expect("tempdir");
        let (recorders, dirs) = recorders(home.path());
        let recorder = recorders.open("session-secret");
        let log = dirs.session_logs().join("session-secret.ndjson");

        // The recipe a real send builds, with the credential the account store
        // puts in it under exactly these names. Only the narrow summary ever
        // reaches the recorder — this environment is here to be *absent* from
        // the file below.
        let mut env = crate::process_env::Env::new();
        env.insert(
            crate::accounts::store::CLAUDE_OAUTH_ENV.into(),
            "sk-ant-oat01-SUPERSECRETVALUE-000".into(),
        );
        env.insert("ANTHROPIC_API_KEY".into(), "sk-ant-api03-ALSOSECRET-111".into());
        recorder.spawned(SpawnSummary {
            selector: "fresh",
            pid: 1,
            shim: "shim",
            cwd: Some(Path::new("/w")),
            model: Some("opus"),
            effort: Some("high"),
            permission: Some("acceptEdits"),
        });
        // And the stdio around it, which is recorded verbatim by design.
        recorder.stdin(r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#);

        until("the writer", || read_log(&log).map(|lines| lines.len() == 2).unwrap_or(false)).await;
        let raw = std::fs::read_to_string(&log).expect("read");
        for secret in ["SUPERSECRETVALUE", "ALSOSECRET", "sk-ant-"] {
            assert!(!raw.contains(secret), "an environment must never reach a recording: {raw}");
        }
        for name in [crate::accounts::store::CLAUDE_OAUTH_ENV, "ANTHROPIC_API_KEY"] {
            assert!(!raw.contains(name), "not even the names of the variables: {raw}");
        }
        assert!(raw.contains("acceptEdits"), "the settings that decide rendering are kept: {raw}");
        // The environment is what was never given to the recorder; nothing was
        // stripped out of it afterwards. Spelled out because the whole "stdio is
        // recorded verbatim" decision rests on it.
        assert!(!env.is_empty(), "the environment this spawn would really have carried");
    }

    /// The ceiling stops the recording rather than rotating it, and says so in
    /// the file — a silent stop would look exactly like a session that ended.
    #[tokio::test]
    async fn a_recording_that_reaches_its_ceiling_stops_and_leaves_a_note() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
        dirs.create().expect("dirs");
        let recorders = Recorders::new_at(dirs.session_logs(), Some(2_000), paths);
        let recorder = recorders.open("session-cap");
        let log = dirs.session_logs().join("session-cap.ndjson");

        for index in 0..200 {
            recorder.stdout(&format!("{{\"line\":{index},\"pad\":\"{}\"}}", "x".repeat(60)), false);
        }
        until("the ceiling", || {
            read_log(&log)
                .map(|lines| lines.iter().any(|line| matches!(line.event, Event::Note { .. })))
                .unwrap_or(false)
        })
        .await;

        let lines = read_log(&log).expect("the log parses");
        let last = lines.last().expect("a last line");
        match &last.event {
            Event::Note { text } => assert!(text.contains("DROPPEN_SESSION_LOG_MAX_MB"), "{text}"),
            other => panic!("the ceiling must be the last thing recorded, got {other:?}"),
        }
        assert!(lines.len() < 200, "the recording stopped rather than writing everything: {}", lines.len());
        let size = std::fs::metadata(&log).expect("stat").len();
        assert!(size < 4_000, "and it stopped near its ceiling rather than far past it: {size}");

        // Re-opening a session that is already at its ceiling records nothing
        // at all rather than pushing it further past.
        let again = recorders.open("session-cap");
        assert!(!again.is_recording());
        again.note("this goes nowhere");
        let after = std::fs::metadata(&log).expect("stat").len();
        assert_eq!(size, after);
    }

    /// Live stdout is always recorded; replayed history is recorded until it
    /// stops being worth the ceiling it would spend.
    #[tokio::test]
    async fn replayed_history_is_recorded_up_to_a_budget_and_then_owned_up_to() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
        dirs.create().expect("dirs");
        // No ceiling here: the budget under test is the *replay* one, and a
        // ceiling low enough to keep this test quick would stop the recording
        // before the budget ever came into it.
        let recorders = Recorders::new_at(dirs.session_logs(), None, paths);
        let recorder = recorders.open("session-replay");
        let log = dirs.session_logs().join("session-replay.ndjson");

        let line = "x".repeat(64 * 1024);
        // Twice the budget of history, and one live line after it.
        for _ in 0..(2 * REPLAY_BUDGET / line.len() as u64) {
            recorder.stdout(&line, true);
        }
        recorder.stdout("{\"type\":\"result\"}", false);

        until("the writer", || {
            read_log(&log)
                .map(|lines| lines.iter().any(|line| matches!(&line.event, Event::Stdout { replayed: false, .. })))
                .unwrap_or(false)
        })
        .await;

        let lines = read_log(&log).expect("the log parses");
        let replayed: u64 = lines
            .iter()
            .filter_map(|line| match &line.event {
                Event::Stdout { line, replayed: true } => Some(line.len() as u64),
                _ => None,
            })
            .sum();
        assert!(replayed > 0, "history is worth recording when there is room for it");
        assert!(
            replayed <= REPLAY_BUDGET + line.len() as u64,
            "but not all of it: {replayed} bytes got in"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| matches!(&line.event, Event::Note { text } if text.contains("replayed history")))
                .count(),
            1,
            "and the truncation is owned up to exactly once"
        );
        assert!(
            lines
                .iter()
                .any(|line| matches!(&line.event, Event::Stdout { line, replayed: false } if line.contains("result"))),
            "a live line after the budget is spent is still recorded: it is not history"
        );
    }

    #[tokio::test]
    async fn a_disabled_factory_and_an_unusable_id_both_record_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let off = Recorders::off(paths.clone());
        assert!(!off.open("session-one").is_recording());

        let (recorders, dirs) = recorders(home.path());
        for bad in ["", "../escape", "with/slash", "-leading-dash", &"x".repeat(129)] {
            assert!(!recorders.open(bad).is_recording(), "{bad:?} must not become a file name");
        }
        assert_eq!(
            std::fs::read_dir(dirs.session_logs()).expect("read dir").count(),
            0,
            "and nothing was created for one"
        );
    }

    #[test]
    fn the_environment_decides_whether_anything_is_recorded_at_all() {
        // The parse rules rather than the process environment, which is shared
        // with every other test in this binary.
        assert!(crate::sessions::reference::is_path_segment_id("11111111-1111-4111-8111-111111111111"));
        assert!(crate::sessions::reference::is_path_segment_id("agent_1"));
        assert!(!crate::sessions::reference::is_path_segment_id("a b"));
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("0123456789abc", 10).chars().count(), 11);
    }
}
