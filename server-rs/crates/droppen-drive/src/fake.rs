//! A provider and a transcript reader of the tests' own, for the tests of the
//! drive's own rules: the attachment routes, an answer's length, the feeds'
//! frames and back-pressure. No CLI's rule is pinned through them, so none is
//! built: each port is answered by a stand-in that writes nothing and starts
//! nothing.
//!
//! A fake session is a file under the data directory's `fake-sessions`, one
//! line per transcript item: each line is read back as a user's text
//! ([`TranscriptItem::user_text`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use droppen_accounts::login::Logins;
use droppen_auth::NotifyCredential;
use droppen_http::identity::NotifyGrant;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::{ClaudeRoot, Paths};
use droppen_provider::models::{ModelList, Models, ModelsQuery};
use droppen_provider::{
    Capabilities, ContinueError, DriverStatesBuilder, InjectError, InjectionRegistry, Provider, ProviderRegistry, ProviderRuns, QueueOutcome,
    ResidentState, Run, RunHost, SendTo, Sent, SpawnGate, StartAt,
};
use droppen_sessions::context::ContextUsage;
use droppen_sessions::util::TranscriptItem;
use droppen_sessions::{CliChoice, Injection, SessionStatus, SessionSummary, Source, TranscriptSources, Transcripts};
use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use super::Drive;

/// The source the fakes answer for. Any would do; the drive asks the
/// provider of a session's source, whichever it is.
pub(crate) const SOURCE: Source = Source::Claude;

/// A drive over a fresh instance at `dir`: the fake provider and the fake
/// reader for [`SOURCE`], the accounts on stand-in logins, and no notify
/// channel: no notify credential, and spawns decorated with nothing.
pub(crate) fn drive(dir: &Path) -> (Arc<Paths>, Arc<Drive>) {
    let paths = Arc::new(Paths::new(dir.to_path_buf()));
    let instance = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    let auth = droppen_auth::Auth::new(&paths, &instance, &droppen_http::config::AuthSettings::default(), false, Arc::new(NoNotify));
    let mut sources = TranscriptSources::default();
    sources.register(Arc::new(FakeReader));
    let drive = Drive::new(paths.clone(), &instance, auth, sources, register, Logins::stand_ins(), droppen_provider::testing::no_decoration());
    (paths, drive)
}

/// The fake provider, and no other.
fn register(providers: &mut ProviderRegistry, registry: &Arc<InjectionRegistry>) {
    providers.register(Arc::new(FakeProvider {
        runs: Arc::new(FakeRuns { host: registry.host.clone() }),
    }));
}

/// Where fake session `id`'s transcript is under `paths`.
pub(crate) fn session_file(paths: &Paths, id: &str) -> PathBuf {
    paths.data_dir().join("fake-sessions").join(format!("{id}.jsonl"))
}

/// Write fake session `id` with `lines`, one transcript item each; its file.
pub(crate) fn session(paths: &Paths, id: &str, lines: &str) -> PathBuf {
    let file = session_file(paths, id);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("fake sessions dir");
    std::fs::write(&file, lines).expect("fake transcript");
    file
}

/// Turns `from..from + turns` of a fake session: four items each, the
/// prompt, a reply, a tool call and its output, each a few hundred bytes.
pub(crate) fn turns(from: usize, turns: usize) -> String {
    let mut out = String::new();
    for turn in from..from + turns {
        let reply = format!("Turn {turn}: ") + &"Here is what I found while reading the code, and what I intend to do next. ".repeat(6);
        let output = format!("output of turn {turn}: ") + &"-rw-r--r--  1 user  staff  4096 Sep 29 00:00 some/path/to/a/file.rs ".repeat(12);
        for line in [format!("please look at part {turn} of the codebase"), reply, format!("ls -la part{turn}"), output] {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// The notify channel's credential, for a server that has none.
struct NoNotify;

impl NotifyCredential for NoNotify {
    fn is_notify_request(&self, _method: &gotcha::axum::http::Method, _path: &str) -> bool {
        false
    }

    fn caller<'a>(
        &'a self,
        _paths: &'a Paths,
        _instance: &'a str,
        _headers: &'a gotcha::axum::http::HeaderMap,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<NotifyGrant>> + Send + 'a>> {
        Box::pin(std::future::ready(None))
    }
}

/// The fake reader: a session is its file under `fake-sessions`, found by
/// id, owned by this server, each line one user's text.
struct FakeReader;

impl FakeReader {
    /// Session `id`'s row, when its file is there.
    fn row(paths: &Paths, id: &str) -> Option<SessionSummary> {
        let file = session_file(paths, id);
        let size = std::fs::metadata(&file).ok()?.len();
        let row = serde_json::json!({
            "pinned": false, "id": id, "source": SOURCE.as_str(), "scope": "owned", "server": "fake", "title": "a fake session",
            "cwd": "/tmp", "repoKey": null, "gitBranch": null, "updatedAt": "2026-10-10T00:00:00.000Z", "createdAt": "2026-10-10T00:00:00.000Z",
            "sizeBytes": size, "filePath": file.to_string_lossy(), "status": "idle", "activity": "idle", "resident": null,
            "claudeRoot": null, "accountId": null, "attentionAt": null, "unread": false, "unreadCount": 0, "acpAgent": null,
        });
        Some(serde_json::from_value(row).expect("a fake session row"))
    }
}

impl Transcripts for FakeReader {
    fn source(&self) -> Source {
        SOURCE
    }

    fn list<'a>(&'a self, paths: &'a Paths, _bindings: &'a BTreeMap<String, String>) -> BoxFuture<'a, Vec<SessionSummary>> {
        Box::pin(async move {
            let Ok(entries) = std::fs::read_dir(paths.data_dir().join("fake-sessions")) else {
                return Vec::new();
            };
            entries
                .flatten()
                .filter_map(|entry| entry.path().file_stem().and_then(|stem| Self::row(paths, &stem.to_string_lossy())))
                .collect()
        })
    }

    fn find<'a>(&'a self, paths: &'a Paths, id: &'a str) -> BoxFuture<'a, Option<SessionSummary>> {
        Box::pin(std::future::ready(Self::row(paths, id)))
    }

    // A fake session sits in no Claude root.
    fn find_root<'a>(&'a self, _paths: &'a Paths, _id: &'a str) -> BoxFuture<'a, Option<ClaudeRoot>> {
        Box::pin(std::future::ready(None))
    }

    fn parse<'a>(&'a self, _paths: &'a Paths, file_path: &'a Path) -> BoxFuture<'a, std::io::Result<Vec<TranscriptItem>>> {
        Box::pin(async move {
            let text = tokio::fs::read_to_string(file_path).await?;
            Ok(text.lines().map(|line| TranscriptItem::user_text(line, None)).collect())
        })
    }

    fn cli_choice<'a>(&'a self, _file_path: &'a Path) -> BoxFuture<'a, Option<CliChoice>> {
        Box::pin(std::future::ready(None))
    }

    fn context<'a>(&'a self, _file_path: &'a Path) -> BoxFuture<'a, Option<ContextUsage>> {
        Box::pin(std::future::ready(None))
    }

    fn spoken_texts(&self, _entry: &Map<String, Value>) -> Vec<String> {
        Vec::new()
    }

    fn external_reason(&self) -> String {
        "this fake session was started elsewhere".to_string()
    }

    fn live_block(&self, _status: SessionStatus) -> Option<String> {
        None
    }

    fn foreign_sessions(&self) -> bool {
        false
    }

    fn on_owned<'a>(&'a self, _paths: &'a Paths, _id: &'a str) -> BoxFuture<'a, ()> {
        Box::pin(std::future::ready(()))
    }

    fn watch_dirs<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, Vec<PathBuf>> {
        Box::pin(std::future::ready(vec![paths.data_dir().join("fake-sessions")]))
    }

    fn has_own_sessions<'a>(&'a self, paths: &'a Paths) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(paths.data_dir().join("fake-sessions").is_dir()))
    }
}

/// The fake provider: it runs [`SOURCE`]'s sessions on nothing. A message or
/// a new session is refused, so nothing is ever started.
struct FakeProvider {
    runs: Arc<FakeRuns>,
}

impl Provider for FakeProvider {
    fn source(&self) -> Source {
        SOURCE
    }

    /// Its sessions ask questions, so the drive's rules for answering one
    /// can be tested; nothing else.
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            questions: true,
            ..Capabilities::default()
        }
    }

    fn run(&self) -> &dyn Run {
        self
    }

    fn runs(&self) -> Arc<dyn ProviderRuns> {
        self.runs.clone()
    }

    fn models(&self) -> &dyn Models {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn continue_on_account<'a>(&'a self, _id: &'a str, _new_id: &'a str) -> BoxFuture<'a, Result<PathBuf, ContinueError>> {
        Box::pin(std::future::ready(Err(ContinueError::Unsupported)))
    }
}

impl Run for FakeProvider {
    fn send<'a>(&'a self, _gate: &'a SpawnGate, _to: SendTo<'a>) -> BoxFuture<'a, Result<Sent, InjectError>> {
        Box::pin(std::future::ready(Err(InjectError::message("the fake provider starts nothing"))))
    }

    fn start<'a>(&'a self, _gate: &'a SpawnGate, _at: StartAt<'a>) -> BoxFuture<'a, Result<String, InjectError>> {
        Box::pin(std::future::ready(Err(InjectError::message("the fake provider starts nothing"))))
    }

    fn injection(&self, _id: &str) -> Option<Injection> {
        None
    }

    fn resident(&self, _id: &str) -> Option<ResidentState> {
        None
    }

    fn schedule_busy(&self, _id: &str) -> Option<&'static str> {
        None
    }

    fn drop_queued<'a>(&'a self, _id: &'a str, _qid: &'a str) -> BoxFuture<'a, QueueOutcome> {
        Box::pin(std::future::ready(QueueOutcome::NoResident))
    }

    fn steer_queued<'a>(&'a self, _id: &'a str, _qid: &'a str) -> BoxFuture<'a, QueueOutcome> {
        Box::pin(std::future::ready(QueueOutcome::NoResident))
    }

    fn interrupt(&self, _id: &str) -> bool {
        false
    }

    fn archive<'a>(&'a self, _gate: &'a SpawnGate, _id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

impl Models for FakeProvider {
    fn list<'a>(&'a self, _query: &'a ModelsQuery) -> BoxFuture<'a, Result<ModelList, String>> {
        Box::pin(std::future::ready(Err("the fake provider lists no models".to_string())))
    }

    fn ids<'a>(&'a self, _account_id: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
        Box::pin(std::future::ready(Vec::new()))
    }

    // Nothing here is entitled to an opinion on an effort.
    fn accepted_efforts<'a>(&'a self, _model: Option<&'a str>) -> BoxFuture<'a, Option<Vec<String>>> {
        Box::pin(std::future::ready(None))
    }

    fn permission_is_known(&self, _permission: &str) -> bool {
        false
    }
}

/// The fake provider's runs, on the registry's host: there are never any.
struct FakeRuns {
    host: Arc<RunHost>,
}

impl ProviderRuns for FakeRuns {
    fn source(&self) -> Source {
        SOURCE
    }

    fn host(&self) -> &Arc<RunHost> {
        &self.host
    }

    fn add_to(&self, _states: &mut DriverStatesBuilder) {}

    fn cli_name(&self) -> &'static str {
        "the fake CLI"
    }

    fn readable(&self, _record: &droppen_provider::runs::RunRecord) -> bool {
        false
    }

    // A fake run is never journalled, so none is ever adopted.
    fn attach(self: Arc<Self>, _record: droppen_provider::runs::RunRecord, _sidecars: droppen_provider::shim::Sidecars, _link: droppen_provider::shim::Link, _lease: droppen_provider::gate::SessionLease) {}

    fn was_cut_short<'a>(&'a self, _record: &'a droppen_provider::runs::RunRecord) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(false))
    }

    fn last_write<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Option<std::time::Instant>> {
        Box::pin(std::future::ready(None))
    }
}
