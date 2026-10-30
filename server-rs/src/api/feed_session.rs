//! A session's feed (`droppen_drive::feed`) on the drive the server builds: the
//! transcript cache and paging, a reconnecting client's resume, a rotation
//! decision, and the drafts in flight. `test_drive` and `claude_turns` serve
//! `feed_mux` too.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use droppen_instance::paths::Paths;
use droppen_sessions::account::AccountRef;
use droppen_sessions::loader::RESUME_OVERLAP;
use droppen_sessions::Source;

use droppen_drive::Drive;
use droppen_drive::feed::{
    attach_session_feed, global, load_session_detail, transcript_page, Attached, FeedAttachment, FeedRegistry, ResumeFrom, DRAFT_THROTTLE,
    SESSION_PAGE_SIZE,
};

/// A transcript is parsed from byte zero, and every attached client plus
/// the two REST routes ask for the same one at the same signature.
#[tokio::test]
async fn an_unchanged_transcript_is_parsed_once_and_a_changed_one_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cached.jsonl");
    let paths = Paths::new(dir.path().to_path_buf());
    let line = |text: &str| format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#);

    std::fs::write(&path, format!("{}\n{}\n", line("one"), line("two"))).expect("seed");
    let first = crate::app::transcript_sources().load_items(&paths, Source::Claude, &path, "sig-1").await;
    assert_eq!(first.len(), 2);

    // The file changes underneath, but the caller still says `sig-1`: the
    // cached parse is what it gets, no read at all.
    std::fs::write(&path, format!("{}\n{}\n{}\n", line("one"), line("two"), line("three"))).expect("rewrite");
    let cached = crate::app::transcript_sources().load_items(&paths, Source::Claude, &path, "sig-1").await;
    assert_eq!(cached.len(), 2, "the same signature must not cost a re-read");

    // A new signature — which is what a changed length or mtime produces —
    // invalidates it, shrinks included.
    let fresh = crate::app::transcript_sources().load_items(&paths, Source::Claude, &path, "sig-2").await;
    assert_eq!(fresh.len(), 3, "a changed signature must re-read");

    std::fs::write(&path, format!("{}\n", line("one"))).expect("shrink");
    let shrunk = crate::app::transcript_sources().load_items(&paths, Source::Claude, &path, "sig-3").await;
    assert_eq!(shrunk.len(), 1, "a file that shrank must not keep its old tail");
}

/// A rotation is decided inside a session's driver and changes no file the
/// watcher can see, so an open page would only hear of it at the next poll.
/// The driver pokes the feed instead, and the state frame that follows
/// carries the decision — well inside the poll interval, which is what
/// shows it was the poke that sent it.
#[tokio::test]
async fn a_rotation_decision_reaches_an_open_session_feed_without_waiting_for_the_poll() {
    use droppen_accounts::rotation::Decision;
    use droppen_sessions::account::{LimitClass, RotationOutcome, RotationTrigger};

    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
    let instance = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    let auth = droppen_auth::Auth::new(&paths, &instance, &droppen_http::config::AuthSettings::default(), false, std::sync::Arc::new(crate::app::NotifyTokens));
    let drive = Drive::new(paths.clone(), &instance, auth, crate::app::transcript_sources(), crate::app::register_providers, crate::app::logins(), crate::app::NotifyDecoration::new(paths.clone(), instance.clone()));
    let feeds = FeedRegistry::new();
    drive.injections.host.attach_listeners(feeds.clone(), droppen_http::refresh::Refresh::new(global::GlobalFeed::new()));

    let id = "33333333-3333-4333-8333-333333333333";
    let project = crate::provider::claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().join("-w-repo");
    std::fs::create_dir_all(&project).expect("project");
    let line = format!(
        r#"{{"type":"user","message":{{"role":"user","content":"hello"}},"timestamp":"2026-09-29T00:00:00.000Z","cwd":"/tmp","sessionId":"{id}","uuid":"u1"}}"#
    );
    std::fs::write(project.join(format!("{id}.jsonl")), format!("{line}\n")).expect("transcript");

    let mut attached = attach_session_feed(&feeds, &drive, Source::Claude, id, None)
        .await
        .expect("the session is there");
    let snapshot: Value = serde_json::from_str(&attached.opening[0]).expect("json");
    assert!(snapshot["rotation"].is_null(), "present and null before there is one: {snapshot}");

    let account = |id: &str, label: &str| AccountRef {
        id: id.to_string(),
        label: label.to_string(),
    };
    drive
        .accounts
        .rotation_log()
        .record(&Decision {
            session_id: id.to_string(),
            at_ms: 1_790_000_000_000,
            from: account("a", "a@work"),
            to: account("b", "b@team"),
            class: LimitClass::Session,
            scope: "all".into(),
            until_ms: Some(1_790_003_600_000),
            interrupted_tasks: 1,
            outcome: RotationOutcome::Switched,
            trigger: RotationTrigger::Limit,
            used_percent: None,
        })
        .expect("record");
    drive.injections.host.announce(Source::Claude, id);

    let frame = tokio::time::timeout(std::time::Duration::from_millis(1500), attached.attachment.next_frame())
        .await
        .expect("pushed before the next poll")
        .expect("a frame");
    let frame: Value = serde_json::from_str(&frame).expect("json");
    assert_eq!(frame["type"], "state", "{frame}");
    assert_eq!(frame["rotation"]["outcome"], "switched", "{frame}");
    assert_eq!(frame["rotation"]["from"]["label"], "a@work");
    assert_eq!(frame["rotation"]["to"]["label"], "b@team");
    assert_eq!(frame["rotation"]["interruptedTasks"], 1);
    assert_eq!(frame["rotation"]["untilAt"], droppen_accounts::rest::iso_ms(1_790_003_600_000));
    assert_eq!(frame["rotation"]["trigger"], "limit", "always present: {frame}");

    let detail = load_session_detail(&drive, Source::Claude, id).await.expect("detail");
    assert_eq!(
        detail.rotation.map(|rotation| rotation.outcome),
        Some(RotationOutcome::Switched),
        "and the REST detail agrees"
    );
}

/// The snapshot a websocket opens with is a page over whatever
/// [`TranscriptSources::load_items`] returned, so a strict decode failure would show the
/// client an empty session rather than the one it asked for.
#[tokio::test]
async fn a_truncated_character_at_eof_still_pages_the_whole_transcript() {
    use std::io::Write;

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let mut file = std::fs::File::create(&path).expect("create");
    writeln!(file, r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"one"}}]}}}}"#).expect("write");
    writeln!(file, r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"two"}}]}}}}"#).expect("write");
    // A CLI mid-write: the last character is cut in half.
    file.write_all(&[0xe4, 0xbd]).expect("write");
    drop(file);

    let items = crate::app::transcript_sources()
        .load_items(&Paths::new(dir.path().to_path_buf()), Source::Claude, &path, "truncated")
        .await;
    let page = transcript_page(&items, items.len());
    assert_eq!(page.total_items, 2, "one bad byte must not blank the snapshot");
}

/// A synthetic claude session of `turns` × 4 items — prompt, prose, a tool
/// call and its output — at roughly the sizes a real one has.
pub(crate) fn claude_turns(id: &str, from: usize, turns: usize) -> String {
    let mut out = String::new();
    for turn in from..from + turns {
        let stamp = format!("2026-09-29T00:{:02}:{:02}.000Z", (turn / 60) % 60, turn % 60);
        let prose = format!("Turn {turn}: ") + &"Here is what I found while reading the code, and what I intend to do next. ".repeat(6);
        let output = format!("output of turn {turn}\n") + &"-rw-r--r--  1 user  staff  4096 Sep 29 00:00 some/path/to/a/file.rs\n".repeat(12);
        let lines = [
            serde_json::json!({"type":"user","message":{"role":"user","content":format!("please look at part {turn} of the codebase")},"timestamp":stamp,"cwd":"/tmp","sessionId":id,"uuid":format!("u{turn}")}),
            serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":prose}]},"timestamp":stamp,"cwd":"/tmp","sessionId":id,"uuid":format!("a{turn}")}),
            serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":format!("toolu_{turn}"),"name":"Bash","input":{"command":format!("ls -la part{turn}")}}]},"timestamp":stamp,"cwd":"/tmp","sessionId":id,"uuid":format!("t{turn}")}),
            serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("toolu_{turn}"),"content":output}]},"timestamp":stamp,"cwd":"/tmp","sessionId":id,"uuid":format!("r{turn}")}),
        ];
        for line in lines {
            out.push_str(&line.to_string());
            out.push('\n');
        }
    }
    out
}

pub(crate) fn test_drive(dir: &Path) -> (Arc<Paths>, Arc<Drive>) {
    let paths = Arc::new(Paths::new(dir.to_path_buf()));
    let instance = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    let auth = droppen_auth::Auth::new(&paths, &instance, &droppen_http::config::AuthSettings::default(), false, std::sync::Arc::new(crate::app::NotifyTokens));
    let drive = Drive::new(paths.clone(), &instance, auth, crate::app::transcript_sources(), crate::app::register_providers, crate::app::logins(), crate::app::NotifyDecoration::new(paths.clone(), instance.clone()));
    (paths, drive)
}

fn opening_bytes(attached: &Attached) -> usize {
    attached.opening.iter().map(String::len).sum()
}

/// The whole point, end to end and across a restart: a client that
/// reconnects with the marker it was sent is handed what it is missing —
/// the overlap and the new items — instead of a fresh page, by a server
/// that has never seen it before. Also where the saving is measured.
#[tokio::test]
async fn a_restarted_server_resumes_a_reconnecting_client_from_its_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = "44444444-4444-4444-8444-444444444444";
    let (paths, drive) = test_drive(dir.path());
    let project = crate::provider::claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().join("-w-repo");
    std::fs::create_dir_all(&project).expect("project");
    let file = project.join(format!("{id}.jsonl"));
    std::fs::write(&file, claude_turns(id, 0, 250)).expect("transcript");

    // First attach: a snapshot, which is what every reconnect used to cost.
    let feeds = FeedRegistry::new();
    let attached = attach_session_feed(&feeds, &drive, Source::Claude, id, None).await.expect("attached");
    let snapshot_bytes = opening_bytes(&attached);
    let snapshot: Value = serde_json::from_str(&attached.opening[0]).expect("json");
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["endIndex"], 1000, "a ~1000-item transcript");
    assert_eq!(snapshot["items"].as_array().map(Vec::len), Some(SESSION_PAGE_SIZE));
    assert!(snapshot["startFp"].is_string(), "{}", snapshot["startFp"]);
    let marker = ResumeFrom::parse(Some("1000"), snapshot["prefixFp"].as_str()).expect("a marker");

    // A reconnect with nothing new: the overlap alone.
    let idle = attach_session_feed(&feeds, &drive, Source::Claude, id, Some(&marker))
        .await
        .expect("attached");
    let idle_bytes = opening_bytes(&idle);
    let frame: Value = serde_json::from_str(&idle.opening[0]).expect("json");
    assert_eq!(frame["type"], "delta", "{}", &idle.opening[0][..200.min(idle.opening[0].len())]);
    assert_eq!(frame["fromIndex"], 1000 - RESUME_OVERLAP);
    assert_eq!(frame["items"].as_array().map(Vec::len), Some(RESUME_OVERLAP));
    let state: Value = serde_json::from_str(&idle.opening[1]).expect("json");
    assert_eq!(state["type"], "state", "state follows as usual");
    drop((attached, idle));

    // The session moves on while the client is away, and the server
    // restarts: a new registry, a new drive, nothing cached.
    let mut handle = std::fs::OpenOptions::new().append(true).open(&file).expect("open");
    std::io::Write::write_all(&mut handle, claude_turns(id, 250, 2).as_bytes()).expect("append");
    drop(handle);
    droppen_sessions::loader::forget_transcript(&file);
    let (_, restarted) = test_drive(dir.path());
    let feeds = FeedRegistry::new();

    let resumed = attach_session_feed(&feeds, &restarted, Source::Claude, id, Some(&marker))
        .await
        .expect("attached");
    let resumed_bytes = opening_bytes(&resumed);
    let frame: Value = serde_json::from_str(&resumed.opening[0]).expect("json");
    assert_eq!(frame["type"], "delta");
    assert_eq!(frame["fromIndex"], 1000 - RESUME_OVERLAP, "the overlap comes again");
    assert_eq!(frame["items"].as_array().map(Vec::len), Some(RESUME_OVERLAP + 8));
    assert_eq!(frame["endIndex"], 1008);
    assert_eq!(frame["items"][0]["text"], snapshot["items"][SESSION_PAGE_SIZE - RESUME_OVERLAP]["text"]);

    // A marker from some other version of the file is a snapshot again.
    let stale = ResumeFrom {
        fp: snapshot["startFp"].as_str().expect("fp").to_string(),
        ..marker.clone()
    };
    let fresh = attach_session_feed(&feeds, &restarted, Source::Claude, id, Some(&stale))
        .await
        .expect("attached");
    let frame: Value = serde_json::from_str(&fresh.opening[0]).expect("json");
    assert_eq!(frame["type"], "snapshot");

    eprintln!(
        "reconnect opening bytes for a 1000-item transcript: snapshot {snapshot_bytes}, \
         resume with nothing new {idle_bytes}, resume with 8 new items {resumed_bytes}"
    );
    assert!(idle_bytes * 10 < snapshot_bytes, "{idle_bytes} vs {snapshot_bytes}");
    assert!(resumed_bytes * 5 < snapshot_bytes, "{resumed_bytes} vs {snapshot_bytes}");
}

// ------------------------------------------------------------- drafts

/// The stdout line a resident claude prints for one finished text block.
fn said(uuid: &str, text: &str) -> Value {
    serde_json::json!({"type": "assistant", "uuid": uuid, "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "text", "text": text}]}})
}

/// The `thinking_tokens` line claude prints with each chunk of a think.
fn thinking(tokens: u64) -> Value {
    serde_json::json!({"type": "system", "subtype": "thinking_tokens", "estimated_tokens": tokens})
}

/// The transcript entry a block becomes, a moment later.
fn entry(id: &str, uuid: &str, text: &str) -> String {
    serde_json::json!({"type": "assistant", "uuid": uuid, "message": {"id": "msg_1", "role": "assistant", "content": [{"type": "text", "text": text}]}, "timestamp": "2026-10-10T00:00:01.000Z", "cwd": "/tmp", "sessionId": id})
        .to_string()
        + "\n"
}

/// A claude session on disk with one prompt in it, and a drive to serve it.
fn drafted_session(dir: &Path, id: &str) -> (Arc<Drive>, PathBuf) {
    let (paths, drive) = test_drive(dir);
    let project = crate::provider::claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().join("-w-repo");
    std::fs::create_dir_all(&project).expect("project");
    let file = project.join(format!("{id}.jsonl"));
    let prompt = serde_json::json!({"type": "user", "uuid": "u-prompt", "message": {"role": "user", "content": "write the module"}, "timestamp": "2026-10-10T00:00:00.000Z", "cwd": "/tmp", "sessionId": id});
    std::fs::write(&file, format!("{prompt}\n")).expect("transcript");
    (drive, file)
}

/// Stand in for the driver: feed the draft a stdout line the way its
/// writer does.
fn draft_hears(drive: &Drive, id: &str, line: &Value) {
    let claude = drive.providers.provider::<crate::provider::claude_code::resident::ClaudeCode>().expect("Claude Code");
    claude.runs().draft_channel(id).send_if_modified(|draft| {
        let changed = droppen_provider::drafts::apply(draft, line);
        if changed {
            draft.v += 1;
        }
        changed
    });
}

fn append(file: &Path, text: &str) {
    let mut handle = std::fs::OpenOptions::new().append(true).open(file).expect("open");
    std::io::Write::write_all(&mut handle, text.as_bytes()).expect("append");
}

/// Every frame up to and including the first `done` accepts, in order.
async fn frames_until(attachment: &mut FeedAttachment, mut done: impl FnMut(&Value) -> bool) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(6), attachment.next_frame())
            .await
            .unwrap_or_else(|_| panic!("no frame in time; so far {seen:?}"))
            .expect("the feed is open");
        let frame: Value = serde_json::from_str(&frame).expect("json");
        let stop = done(&frame);
        seen.push(frame);
        if stop {
            return seen;
        }
    }
}

fn is_draft(frame: &Value) -> bool {
    frame["type"] == "draft"
}

/// The count a draft frame carries, or `None` for one that says nothing is
/// being thought.
fn count(frame: &Value) -> Option<u64> {
    frame["blocks"][0]["thinkingTokens"].as_u64()
}

fn delta_with(frame: &Value, text: &str) -> bool {
    frame["type"] == "delta"
        && frame["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["text"] == text))
}

/// An attach mid-think opens with the count, after the snapshot and the
/// state; one with nothing in flight opens without a draft, and a codex
/// session never has one.
#[tokio::test]
async fn the_opening_frames_carry_the_count_of_a_think_in_flight() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = "66666666-6666-4666-8666-666666666666";
    let (drive, _file) = drafted_session(dir.path(), id);
    let feeds = FeedRegistry::new();

    let quiet = attach_session_feed(&feeds, &drive, Source::Claude, id, None).await.expect("attached");
    assert_eq!(quiet.opening.len(), 2, "snapshot and state, no draft: {:?}", quiet.opening);
    drop(quiet);

    // Words said before the think are not the draft's: only the count is.
    draft_hears(&drive, id, &said("u-said", "said, and not drafted"));
    draft_hears(&drive, id, &thinking(300));
    let attached = attach_session_feed(&feeds, &drive, Source::Claude, id, None).await.expect("attached");
    let types: Vec<Value> = attached
        .opening
        .iter()
        .map(|frame| serde_json::from_str::<Value>(frame).expect("json")["type"].clone())
        .collect();
    assert_eq!(types, ["snapshot", "state", "draft"]);
    let draft: Value = serde_json::from_str(&attached.opening[2]).expect("json");
    assert_eq!(draft["blocks"], serde_json::json!([{"index": 0, "kind": "thinking", "thinkingTokens": 300}]));

    assert!(drive.providers.draft_watch(Source::Codex, id).is_none(), "codex has no draft");
}

/// Live: the count goes out as it moves, and the model moving on — a
/// finished block, then the transcript getting it — goes out as a draft
/// with no blocks, which is how a page is told the think is over.
#[tokio::test]
async fn a_count_goes_out_as_it_moves_and_its_end_as_an_empty_draft() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = "77777777-7777-4777-8777-777777777777";
    let (drive, file) = drafted_session(dir.path(), id);
    let feeds = FeedRegistry::new();
    let mut attached = attach_session_feed(&feeds, &drive, Source::Claude, id, None).await.expect("attached");

    draft_hears(&drive, id, &thinking(40));
    frames_until(&mut attached.attachment, |frame| is_draft(frame) && count(frame) == Some(40)).await;
    draft_hears(&drive, id, &thinking(90));
    frames_until(&mut attached.attachment, |frame| is_draft(frame) && count(frame) == Some(90)).await;

    draft_hears(&drive, id, &said("u-1", "early words"));
    let over = frames_until(&mut attached.attachment, |frame| is_draft(frame) && count(frame).is_none()).await;
    assert_eq!(over.last().expect("a frame")["blocks"], serde_json::json!([]));
    assert!(over.iter().all(|frame| !frame.to_string().contains("early words")), "{over:?}");

    // The words reach the page as an item, from the file, and no draft
    // follows them: there is nothing left to retire.
    append(&file, &entry(id, "u-1", "early words"));
    let mut frames = frames_until(&mut attached.attachment, |frame| delta_with(frame, "early words")).await;
    while let Ok(Some(frame)) = tokio::time::timeout(std::time::Duration::from_millis(400), attached.attachment.next_frame()).await {
        frames.push(serde_json::from_str(&frame).expect("json"));
    }
    assert!(!frames.iter().any(is_draft), "{frames:?}");
}

/// A session with nothing in flight — no resident, or one between turns —
/// is never sent a draft at all, however much its transcript moves.
#[tokio::test]
async fn a_session_with_nothing_in_flight_is_never_sent_a_draft() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let (drive, file) = drafted_session(dir.path(), id);
    let feeds = FeedRegistry::new();
    let mut attached = attach_session_feed(&feeds, &drive, Source::Claude, id, None).await.expect("attached");
    append(&file, &entry(id, "u-1", "written straight to the file"));
    let mut frames = frames_until(&mut attached.attachment, |frame| delta_with(frame, "written straight to the file")).await;
    while let Ok(Some(frame)) = tokio::time::timeout(std::time::Duration::from_millis(400), attached.attachment.next_frame()).await {
        frames.push(serde_json::from_str(&frame).expect("json"));
    }
    assert!(!frames.iter().any(is_draft), "{frames:?}");
}

/// A burst of changes is a handful of frames, and the last says the last
/// thing: the throttle reads the newest value rather than queueing.
#[tokio::test]
async fn a_burst_of_draft_changes_is_coalesced_and_ends_on_the_latest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = "88888888-8888-4888-8888-888888888888";
    let (drive, _file) = drafted_session(dir.path(), id);
    let feeds = FeedRegistry::new();
    let mut attached = attach_session_feed(&feeds, &drive, Source::Claude, id, None).await.expect("attached");

    let started = std::time::Instant::now();
    for n in 1..=20u64 {
        draft_hears(&drive, id, &thinking(n * 10));
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let burst = started.elapsed();
    let frames = frames_until(&mut attached.attachment, |frame| is_draft(frame) && count(frame) == Some(200)).await;
    let drafts = frames.iter().filter(|frame| is_draft(frame)).count();
    let allowed = 2 + (burst.as_millis() / DRAFT_THROTTLE.as_millis()) as usize;
    assert!(drafts <= allowed, "{drafts} frames for a {burst:?} burst, at most {allowed}");
    assert!(drafts < 20, "twenty changes must not be twenty frames");
    assert_eq!(frames.last().expect("a frame")["blocks"][0]["kind"], "thinking");
}

/// End to end through a real session feed: the page is sent the count as
/// the one block of a `draft` frame — never what the agent said, which
/// reaches it as an item once the transcript has it — then an empty draft
/// once the think is over; and the turn's close is pushed rather than left
/// for the poll.
#[tokio::test]
async fn a_page_is_sent_the_count_and_never_the_words() {
    use crate::provider::claude_code::resident::test_harness::{thinks_until_released, until, Harness, SESSION};
    use droppen_instance::instance::InstanceDirs;
    use droppen_provider::common::Phase;
    use std::time::Duration;
    let harness = Harness::ordinary_with(&thinks_until_released()).await;
    let prompt = format!(
        r#"{{"type":"user","uuid":"u-prompt","message":{{"role":"user","content":"think about it"}},"timestamp":"2026-10-10T00:00:00.000Z","cwd":"/tmp","sessionId":"{SESSION}"}}"#
    );
    std::fs::write(harness.transcript(), format!("{prompt}\n")).expect("transcript");
    let paths = Arc::new(Paths::new(harness._home.path().to_path_buf()));
    let instance = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    let drive = Arc::new(droppen_drive::Drive {
        auth: droppen_auth::Auth::new(&paths, &instance, &droppen_http::config::AuthSettings::default(), false, std::sync::Arc::new(crate::app::NotifyTokens)),
        injections: harness.injections.clone(),
        accounts: Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), crate::app::logins())),
        quota: Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), crate::app::logins())),
        composer: droppen_drive::composer_prefs::ComposerPrefsStore::new(paths.clone()),
        titles: droppen_sessions::titles::TitlesStore::new(paths.clone()),
        paths,
        transcripts: crate::app::transcript_sources(),
        providers: harness.providers(),
    });
    let feeds = droppen_drive::feed::FeedRegistry::new();
    harness.injections.host.attach_listeners(feeds.clone(), droppen_http::refresh::Refresh::new(droppen_drive::feed::global::GlobalFeed::new()));
    until("the init", || harness.state().phase == Phase::Idle).await;
    let mut attached = droppen_drive::feed::attach_session_feed(&feeds, &drive, Source::Claude, SESSION, None)
        .await
        .expect("attached");

    let mut frames: Vec<Value> = Vec::new();
    let mut next = async |frames: &mut Vec<Value>| {
        let frame = tokio::time::timeout(Duration::from_secs(8), attached.attachment.next_frame())
            .await
            .unwrap_or_else(|_| panic!("no frame in time; so far {frames:?}"))
            .expect("the feed is open");
        let frame: Value = serde_json::from_str(&frame).expect("json");
        frames.push(frame.clone());
        frame
    };
    let carries_item = |frame: &Value| {
        matches!(frame["type"].as_str(), Some("delta" | "reset"))
            && frame["items"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["text"] == "said once the think was done"))
    };
    let counting = |frame: &Value| frame["type"] == "draft" && frame["blocks"].as_array().is_some_and(|blocks| !blocks.is_empty());

    harness.send("think about it");
    loop {
        let frame = next(&mut frames).await;
        if counting(&frame) && frame["blocks"][0]["thinkingTokens"] == 42 {
            assert_eq!(frame["blocks"], serde_json::json!([{"index": 0, "kind": "thinking", "thinkingTokens": 42}]));
            break;
        }
    }
    let released = std::time::Instant::now();
    harness.release();
    let ended =
        |frame: &Value| frame["type"] == "state" && frame["summary"]["resident"]["phase"] == "idle" && frame["injection"]["state"] == "ok";
    let mut ended_after = None;
    let (mut item, mut over) = (false, false);
    while !(item && over && ended_after.is_some()) {
        let frame = next(&mut frames).await;
        if ended(&frame) && ended_after.is_none() {
            ended_after = Some(released.elapsed());
        }
        item |= carries_item(&frame);
        over |= frame["type"] == "draft" && frame["blocks"] == serde_json::json!([]);
    }
    for frame in frames.iter().filter(|frame| frame["type"] == "draft") {
        assert!(!frame.to_string().contains("said once"), "a draft carries no words: {frame}");
    }

    // The close reaches the page as a state frame of its own, pushed by the
    // driver the moment the turn ends: the 2 s poll would get there too,
    // but a second and more later.
    let ended_after = ended_after.expect("the turn's end was pushed");
    assert!(
        ended_after < Duration::from_millis(1500),
        "the close took {ended_after:?} to reach the page"
    );
}
