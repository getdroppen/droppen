//! The run journal, as Claude Code's and Codex's rows are written in it.

use droppen_provider::runs::*;
use droppen_sessions::Source;
use serde::Serialize;
use crate::provider::codex::app_server::CodexRun;
use crate::provider::claude_code::resident::{ClaudeJournal, ClaudeRun, Queued};

fn record(run_id: &str, session_id: &str, pid: u32) -> RunRecord<ClaudeRun> {
    RunRecord {
        run_id: run_id.to_string(),
        source: Source::Claude,
        session_id: session_id.to_string(),
        cwd: Some("/workspace".to_string()),
        cli_pid: pid,
        started_at: "2026-01-01T00:00:00.000Z".to_string(),
        pid_token: Some("123.000000".to_string()),
        resident: false,
        shim: None,
        payload: ClaudeRun::default(),
    }
}

/// Every run in `file`, read as Claude Code's.
fn claude_runs(file: &RunsFile) -> Vec<RunRecord<ClaudeRun>> {
    file.read().into_iter().map(|run| run.decode().expect("a Claude Code run")).collect()
}

#[test]
fn a_run_is_recorded_until_it_finishes_and_no_longer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = RunsFile::new(dir.path().join("runs.json"));

    assert!(claude_runs(&file).is_empty(), "an empty database is an empty journal, not an error");

    file.add(record("run-1", "session-a", 4242));
    file.add(record("run-2", "session-b", 4243));
    let runs = claude_runs(&file);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].session_id, "session-a");
    assert_eq!(runs[0].cli_pid, 4242);
    assert_eq!(runs[0].source, Source::Claude);
    assert_eq!(runs[0].pid_token.as_deref(), Some("123.000000"));

    // Re-adding one run replaces it rather than duplicating it, so a retry
    // cannot leave two records pointing at one process.
    file.add(record("run-1", "session-a", 5555));
    assert_eq!(claude_runs(&file).len(), 2);
    assert_eq!(claude_runs(&file)[1].cli_pid, 5555, "the newest wins and moves to the end");

    file.repoint("run-2", "0199c1a1-2b3c-4d5e-8f60-112233445566");
    assert_eq!(claude_runs(&file)[0].session_id, "0199c1a1-2b3c-4d5e-8f60-112233445566");

    // The turn mark and the queue go on and come off the record they name,
    // and only it: a resident answering one session must not mark another's
    // turn open or inherit its backlog.
    assert!(claude_runs(&file).iter().all(|run| run.payload.turn_at.is_none()), "no turn is in flight to begin with");
    let waiting = vec![Queued {
        id: "q-1".to_string(),
        text: "next please".to_string(),
        fingerprint: Default::default(),
        steered: false,
        rotation_attempt: 0,
        continuation: None,
    }];
    file.mark_progress("run-1", Some(4096), &waiting);
    let runs = claude_runs(&file);
    let marked = runs.iter().find(|run| run.run_id == "run-1").expect("run-1");
    assert_eq!(marked.payload.turn_at, Some(4096));
    assert_eq!(marked.payload.queued, waiting);
    let untouched = runs.iter().find(|run| run.run_id == "run-2").expect("run-2");
    assert_eq!(untouched.payload.turn_at, None);
    assert!(untouched.payload.queued.is_empty());
    file.mark_progress("run-1", None, &[]);
    let runs = claude_runs(&file);
    assert!(runs.iter().all(|run| run.payload.turn_at.is_none()), "and the turn ending takes it off again");
    assert!(runs.iter().all(|run| run.payload.queued.is_empty()), "and the delivered message with it");
    // Marking a run that is not there is not an error: a turn can end after
    // the process that was running it has already been retired.
    file.mark_progress("run-gone", Some(1), &waiting);

    file.remove("run-2");
    assert_eq!(claude_runs(&file).len(), 1);
    file.remove("run-1");
    assert!(claude_runs(&file).is_empty(), "a finished run leaves nothing behind");
    // Removing what is not there is not an error: completion paths overlap.
    file.remove("run-1");
}

/// A record can be less than the truth, and saying so is the point of being
/// able to rewrite it.
///
/// Two ways it happens, both to a *running* process: the record predates the
/// field, so it says nothing at all; or the CLI resolved the model alias the
/// command line asked for into the name it answers under. Either way the
/// next server reads this journal and nothing else, so what it says has to
/// become what is true — a record left saying "unknown" is one every later
/// boot has to guess at again.
#[test]
fn a_fingerprint_learned_after_the_spawn_replaces_what_the_record_said() {
    use super::super::claude_code::resident::Fingerprint;

    let dir = tempfile::tempdir().expect("tempdir");
    let file = RunsFile::new(dir.path().join("runs.json"));
    file.add(record("run-1", "session-a", 4242));
    file.add(record("run-2", "session-b", 4243));
    assert!(claude_runs(&file)[0].payload.fingerprint.is_none(), "a record from before the field says nothing");

    let learned = Fingerprint {
        model: Some("claude-opus-4-5".to_string()),
        account: Some("work".to_string()),
        ..Default::default()
    };
    file.mark_fingerprint("run-1", &learned);
    let runs = claude_runs(&file);
    let healed = runs.iter().find(|run| run.run_id == "run-1").expect("run-1");
    assert_eq!(healed.payload.fingerprint.as_ref(), Some(&learned));
    let untouched = runs.iter().find(|run| run.run_id == "run-2").expect("run-2");
    assert!(untouched.payload.fingerprint.is_none(), "and only the run it names");

    // A correction of a correction is the model alias case, and it wins:
    // the newest thing the driver was told is the one the process answers to.
    let resolved = Fingerprint {
        model: Some("claude-opus-4-5-20260101".to_string()),
        ..learned.clone()
    };
    file.mark_fingerprint("run-1", &resolved);
    assert_eq!(claude_runs(&file)[0].payload.fingerprint.as_ref(), Some(&resolved));
    // Naming a run that is not there is not an error, for the same reason
    // marking progress on one is not: a driver can outlive its record.
    file.mark_fingerprint("run-gone", &resolved);
}

/// The table is the journal, and `runs.json` is neither written nor read.
///
/// This is the whole of what the move bought, stated as a test. Before, this
/// file *was* the journal, and a truncated one meant every in-flight run was
/// orphaned — the composer unblocked on sessions that still had a CLI
/// attached. Then it was a mirror, rewritten whole on every step of every
/// turn for a rolled-back binary's sake. Now nothing touches it: a turn costs
/// no whole-file encode, no `fsync` and no rename, and whatever the file
/// happens to contain neither helps nor hurts.
#[test]
fn the_file_the_table_replaced_is_neither_written_nor_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runs.json");
    let file = RunsFile::new(&path);
    file.add(record("run-1", "session-a", 4242));
    file.add(record("run-2", "session-b", 4243));
    assert!(!path.exists(), "recording a run writes the table and nothing else");

    // A file that appears afterwards — a rollback's leavings, a restore done
    // by hand — is left exactly as found. Nothing reads it, so it is not
    // adopted, and there is no writer left to overwrite it.
    let planted = b"{\"runs\": [{\"run_id\"".to_vec();
    std::fs::write(&path, &planted).expect("write");
    file.remove("run-2");
    file.mark_progress("run-1", Some(4096), &[]);
    assert_eq!(claude_runs(&file).len(), 1, "the journal is the table, not the file beside it");
    assert_eq!(std::fs::read(&path).expect("read"), planted, "and the file is byte for byte what it was");
    assert!(
        !dir.path().join("runs.json.tmp").exists(),
        "no temporary either: there is nothing left that writes through one"
    );
}

/// What a restarting server sees is what the last one committed.
#[test]
fn a_run_survives_the_server_that_recorded_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runs.json");
    {
        let file = RunsFile::new(&path);
        file.add(record("run-1", "session-a", 4242));
        file.mark_progress("run-1", Some(4096), &[]);
    }
    // A second `RunsFile` over the same directory is the next boot.
    let next = RunsFile::new(&path);
    let runs = claude_runs(&next);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].cli_pid, 4242);
    assert_eq!(runs[0].payload.turn_at, Some(4096), "including what it was in the middle of");
}

/// A row that will not parse costs that run, not the whole adoption.
#[test]
fn a_corrupt_journal_forgets_runs_instead_of_refusing_to_boot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runs.json");
    let db = droppen_instance::db::at(path.with_file_name("state.db"));
    db.write(|tx| {
        tx.execute(
            "INSERT INTO runs (run_id, session_id, ordinal, payload) VALUES ('half-written', 'session-a', 0, ?1)",
            (r#"{"run_id":"half-writ"#,),
        )
    })
    .expect("a half-written row");

    let file = RunsFile::new(&path);
    assert!(claude_runs(&file).is_empty());

    // And it costs only the run in the rubbish: the table came up otherwise
    // whole, so the next run is recorded normally.
    file.add(record("run-1", "session-a", 7));
    assert_eq!(claude_runs(&file).len(), 1);
    assert!(!path.exists(), "and nothing here creates a file");
}

// ---- the rows on disk, byte for byte ---------------------------------
//
// Every other version reads these rows back, so each is pinned as main
// writes it: read as the run of the CLI that wrote it and written again,
// a row comes out as it went in.

/// A Claude Code resident with everything a row can say.
const CLAUDE_RESIDENT: &str = r#"{"run_id":"run-claude","source":"claude","session_id":"11111111-1111-4111-8111-111111111111","cwd":"/work/claude","cli_pid":4242,"started_at":"2026-10-30T01:02:03.004Z","pid_token":"start-4242","resident":true,"shim":"shim-claude","fingerprint":{"model":"opus","resolved":"claude-opus-4","effort":"high","permission":"default","account":"acct-1","credential":"oauth","chat":false,"uploads":true,"questions":false},"turn_at":1024,"queued":[{"id":"q-1","text":"first","fingerprint":{"model":"opus","resolved":null,"effort":null,"permission":null,"account":null,"credential":null,"chat":false,"uploads":false,"questions":false},"steered":true},{"id":"q-2","text":"the continuation","fingerprint":{"model":"sonnet","resolved":null,"effort":null,"permission":null,"account":"acct-2","credential":null,"chat":false,"uploads":false,"questions":false},"steered":false,"rotation_attempt":2,"continuation":{"record":7,"from":"acct-1","to":"acct-2","reason":"limit","until":"2026-10-30T05:00:00.000Z"}}],"hold":{"account":"acct-1","label":"Work","class":"session","scope":"all","untilMs":1761800000000,"switches":1,"continueFailed":false}}"#;
/// A Claude Code resident with nothing in flight and nothing known.
const CLAUDE_RESIDENT_BARE: &str = r#"{"run_id":"run-claude-bare","source":"claude","session_id":"22222222-2222-4222-8222-222222222222","cwd":null,"cli_pid":4343,"started_at":"2026-10-30T01:02:03.004Z","pid_token":null,"resident":true,"shim":"shim-claude-bare","fingerprint":null,"turn_at":null,"queued":[]}"#;
/// A one-shot `claude --print` run.
const CLAUDE_PRINT: &str = r#"{"run_id":"run-print","source":"claude","session_id":"33333333-3333-4333-8333-333333333333","cwd":"/work/print","cli_pid":4444,"started_at":"2026-10-30T01:02:03.004Z","pid_token":"start-4444","resident":false,"shim":null,"fingerprint":null,"turn_at":null,"queued":[]}"#;
/// A Codex app server on a stored account, mid-turn with a message queued.
const CODEX_APP_SERVER: &str = r#"{"run_id":"run-codex","source":"codex","session_id":"0199c1a1-0000-4000-8000-000000000001","cwd":"/work/codex","cli_pid":4545,"started_at":"2026-10-30T01:02:03.004Z","pid_token":"start-4545","resident":false,"shim":"shim-codex","fingerprint":null,"turn_at":null,"queued":[],"codex":{"cursor":2048,"active":"turn-2","completed_turn":"turn-1","queue":[{"id":"m-1","text":"next","params":{"model":"gpt-5"},"request":"req-1","uncertain":true}],"settings":{"model":"gpt-5","effort":"high","sandbox_policy":null},"paused":true,"injection":{"state":"running","error":null,"interrupted":false,"startedAt":"2026-10-30T01:02:03.004Z","finishedAt":null},"interrupt_requested":false,"archive_request":null,"archived":false},"codex_account":"acct-codex"}"#;
/// A Codex app server on the machine's own login, idle.
const CODEX_APP_SERVER_LOCAL: &str = r#"{"run_id":"run-codex-local","source":"codex","session_id":"0199c1a1-0000-4000-8000-000000000002","cwd":"/work/codex","cli_pid":4646,"started_at":"2026-10-30T01:02:03.004Z","pid_token":"start-4646","resident":false,"shim":"shim-codex-local","fingerprint":null,"turn_at":null,"queued":[],"codex":{"cursor":0,"active":null,"completed_turn":null,"queue":[],"settings":null,"paused":false,"injection":null,"interrupt_requested":false,"archive_request":null,"archived":false}}"#;
/// A one-shot `codex exec` run.
const CODEX_EXEC: &str = r#"{"run_id":"run-exec","source":"codex","session_id":"0199c1a1-0000-4000-8000-000000000003","cwd":"/work/exec","cli_pid":4747,"started_at":"2026-10-30T01:02:03.004Z","pid_token":"start-4747","resident":false,"shim":null,"fingerprint":null,"turn_at":null,"queued":[]}"#;

/// A run's record as Claude Code reads its rows, and as Codex reads its.
type ClaudeRecord = RunRecord<ClaudeRun>;
type CodexRecord = RunRecord<CodexRun>;

/// `record` written to a journal of its own, as it then is on disk.
fn on_disk<P: Serialize>(record: RunRecord<P>) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = RunsFile::new(dir.path().join("runs.json"));
    journal.add(record);
    journal.raw().pop().expect("the row")
}

/// `row` read as a Claude Code run and written again.
fn rewritten_as_claude(row: &str) -> String {
    on_disk(serde_json::from_str::<ClaudeRecord>(row).expect("a Claude Code row"))
}

/// `row` read as a Codex run and written again.
fn rewritten_as_codex(row: &str) -> String {
    on_disk(serde_json::from_str::<CodexRecord>(row).expect("a Codex row"))
}

#[test]
fn each_clis_rows_are_written_as_they_always_were() {
    for row in [CLAUDE_RESIDENT, CLAUDE_RESIDENT_BARE, CLAUDE_PRINT] {
        assert_eq!(rewritten_as_claude(row), row);
    }
    for row in [CODEX_APP_SERVER, CODEX_APP_SERVER_LOCAL, CODEX_EXEC] {
        assert_eq!(rewritten_as_codex(row), row);
    }
}

/// What a resident's driver writes into its row as it goes: each change
/// rewrites the row whole, as it always has.
#[test]
fn a_residents_row_is_rewritten_as_it_always_was() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = RunsFile::new(dir.path().join("runs.json"));
    journal.add(serde_json::from_str::<ClaudeRecord>(CLAUDE_RESIDENT_BARE).expect("a Claude Code row"));
    let run = "run-claude-bare";
    let queued = serde_json::from_str(r#"{"id":"q-9","text":"behind it","fingerprint":{"model":"haiku"},"steered":false}"#).expect("a queued message");
    let hold = serde_json::from_str(r#"{"account":"acct-1","label":"Work","class":"session","scope":"all","untilMs":null,"switches":0,"continueFailed":true}"#).expect("a hold");
    let fingerprint = serde_json::from_str(r#"{"model":"opus","resolved":"claude-opus-4","account":"acct-1"}"#).expect("a fingerprint");
    let mut seen = Vec::new();
    journal.mark_progress(run, Some(2048), &[queued]);
    seen.extend(journal.raw());
    journal.mark_hold(run, Some(hold));
    seen.extend(journal.raw());
    journal.mark_fingerprint(run, &fingerprint);
    seen.extend(journal.raw());
    journal.repoint(run, "66666666-6666-4666-8666-666666666666");
    seen.extend(journal.raw());
    journal.mark_progress(run, None, &[]);
    journal.mark_hold(run, None);
    seen.extend(journal.raw());
    assert_eq!(seen, RESIDENT_REWRITES);
}

/// The row after each change: a turn and a queued message, a hold, what
/// the process runs, the session the CLI chose, and the turn and hold over.
const RESIDENT_REWRITES: [&str; 5] = [
r#"{"run_id":"run-claude-bare","source":"claude","session_id":"22222222-2222-4222-8222-222222222222","cwd":null,"cli_pid":4343,"started_at":"2026-10-30T01:02:03.004Z","pid_token":null,"resident":true,"shim":"shim-claude-bare","fingerprint":null,"turn_at":2048,"queued":[{"id":"q-9","text":"behind it","fingerprint":{"model":"haiku","resolved":null,"effort":null,"permission":null,"account":null,"credential":null,"chat":false,"uploads":false,"questions":false},"steered":false}]}"#,
r#"{"run_id":"run-claude-bare","source":"claude","session_id":"22222222-2222-4222-8222-222222222222","cwd":null,"cli_pid":4343,"started_at":"2026-10-30T01:02:03.004Z","pid_token":null,"resident":true,"shim":"shim-claude-bare","fingerprint":null,"turn_at":2048,"queued":[{"id":"q-9","text":"behind it","fingerprint":{"model":"haiku","resolved":null,"effort":null,"permission":null,"account":null,"credential":null,"chat":false,"uploads":false,"questions":false},"steered":false}],"hold":{"account":"acct-1","label":"Work","class":"session","scope":"all","untilMs":null,"switches":0,"continueFailed":true}}"#,
r#"{"run_id":"run-claude-bare","source":"claude","session_id":"22222222-2222-4222-8222-222222222222","cwd":null,"cli_pid":4343,"started_at":"2026-10-30T01:02:03.004Z","pid_token":null,"resident":true,"shim":"shim-claude-bare","fingerprint":{"model":"opus","resolved":"claude-opus-4","effort":null,"permission":null,"account":"acct-1","credential":null,"chat":false,"uploads":false,"questions":false},"turn_at":2048,"queued":[{"id":"q-9","text":"behind it","fingerprint":{"model":"haiku","resolved":null,"effort":null,"permission":null,"account":null,"credential":null,"chat":false,"uploads":false,"questions":false},"steered":false}],"hold":{"account":"acct-1","label":"Work","class":"session","scope":"all","untilMs":null,"switches":0,"continueFailed":true}}"#,
r#"{"run_id":"run-claude-bare","source":"claude","session_id":"66666666-6666-4666-8666-666666666666","cwd":null,"cli_pid":4343,"started_at":"2026-10-30T01:02:03.004Z","pid_token":null,"resident":true,"shim":"shim-claude-bare","fingerprint":{"model":"opus","resolved":"claude-opus-4","effort":null,"permission":null,"account":"acct-1","credential":null,"chat":false,"uploads":false,"questions":false},"turn_at":2048,"queued":[{"id":"q-9","text":"behind it","fingerprint":{"model":"haiku","resolved":null,"effort":null,"permission":null,"account":null,"credential":null,"chat":false,"uploads":false,"questions":false},"steered":false}],"hold":{"account":"acct-1","label":"Work","class":"session","scope":"all","untilMs":null,"switches":0,"continueFailed":true}}"#,
r#"{"run_id":"run-claude-bare","source":"claude","session_id":"66666666-6666-4666-8666-666666666666","cwd":null,"cli_pid":4343,"started_at":"2026-10-30T01:02:03.004Z","pid_token":null,"resident":true,"shim":"shim-claude-bare","fingerprint":{"model":"opus","resolved":"claude-opus-4","effort":null,"permission":null,"account":"acct-1","credential":null,"chat":false,"uploads":false,"questions":false},"turn_at":null,"queued":[]}"#,
];

/// Rows another version wrote, before a field existed or after one this
/// version does not know: each is read with what its absence always meant,
/// and written again whole, without what this version does not know.
#[test]
fn rows_written_by_other_versions_are_read_and_written_whole() {
    // Before the pid-reuse guard, residents, shims, fingerprints, turns and queues.
    assert_eq!(
        rewritten_as_claude(r#"{"run_id":"run-old-print","source":"claude","session_id":"44444444-4444-4444-8444-444444444444","cwd":"/tmp","cli_pid":4848,"started_at":"2026-01-01T00:00:00.000Z"}"#),
        OLD_ROWS_REWRITTEN[0]
    );
    // A resident whose queued message predates ids, steering and fingerprints,
    // written by a version that knew one more field.
    assert_eq!(
        rewritten_as_claude(r#"{"run_id":"run-old-resident","source":"claude","session_id":"55555555-5555-4555-8555-555555555555","cwd":"/tmp","cli_pid":4949,"started_at":"2026-01-01T00:00:00.000Z","pid_token":"start-4949","resident":true,"shim":"shim-old","queued":[{"text":"from before ids"}],"from_a_later_version":true}"#),
        OLD_ROWS_REWRITTEN[1]
    );
    // An app server whose state predates most of its fields.
    assert_eq!(
        rewritten_as_codex(r#"{"run_id":"run-old-codex","source":"codex","session_id":"0199c1a1-0000-4000-8000-000000000004","cwd":"/tmp","cli_pid":5050,"started_at":"2026-01-01T00:00:00.000Z","pid_token":"start-5050","resident":false,"shim":"shim-old-codex","codex":{"cursor":7,"queue":[{"id":"m-old","text":"queued long ago","params":{},"request":null}]}}"#),
        OLD_ROWS_REWRITTEN[2]
    );
}

/// Each of those rows as this version writes it again.
const OLD_ROWS_REWRITTEN: [&str; 3] = [
r#"{"run_id":"run-old-print","source":"claude","session_id":"44444444-4444-4444-8444-444444444444","cwd":"/tmp","cli_pid":4848,"started_at":"2026-01-01T00:00:00.000Z","pid_token":null,"resident":false,"shim":null,"fingerprint":null,"turn_at":null,"queued":[]}"#,
r#"{"run_id":"run-old-resident","source":"claude","session_id":"55555555-5555-4555-8555-555555555555","cwd":"/tmp","cli_pid":4949,"started_at":"2026-01-01T00:00:00.000Z","pid_token":"start-4949","resident":true,"shim":"shim-old","fingerprint":null,"turn_at":null,"queued":[{"id":"","text":"from before ids","fingerprint":{"model":null,"resolved":null,"effort":null,"permission":null,"account":null,"credential":null,"chat":false,"uploads":false,"questions":false},"steered":false}]}"#,
r#"{"run_id":"run-old-codex","source":"codex","session_id":"0199c1a1-0000-4000-8000-000000000004","cwd":"/tmp","cli_pid":5050,"started_at":"2026-01-01T00:00:00.000Z","pid_token":"start-5050","resident":false,"shim":"shim-old-codex","fingerprint":null,"turn_at":null,"queued":[],"codex":{"cursor":7,"active":null,"completed_turn":null,"queue":[{"id":"m-old","text":"queued long ago","params":{},"request":null,"uncertain":false}],"settings":null,"paused":false,"injection":null,"interrupt_requested":false,"archive_request":null,"archived":false}}"#,
];

/// A row whose source this version does not know, or that names none, is
/// one it cannot read: it is not handed to anyone, and it stays where it is.
#[test]
fn a_row_with_an_unknown_source_or_none_is_not_read_and_stays() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = RunsFile::new(dir.path().join("runs.json"));
    journal.insert_raw("run-unknown", "77777777-7777-4777-8777-777777777777", &CLAUDE_PRINT.replace("\"run-print\"", "\"run-unknown\"").replace("\"source\":\"claude\"", "\"source\":\"gemini\""));
    journal.insert_raw("run-nameless", "88888888-8888-4888-8888-888888888888", &CLAUDE_PRINT.replace("\"run-print\"", "\"run-nameless\"").replace("\"source\":\"claude\",", ""));
    journal.add(serde_json::from_str::<ClaudeRecord>(CLAUDE_PRINT).expect("a Claude Code row"));
    let read: Vec<String> = journal.read().into_iter().map(|run| run.run_id).collect();
    assert_eq!(read, ["run-print"]);
    assert_eq!(journal.raw().len(), 3, "the rows it cannot read stay as they were");
}
