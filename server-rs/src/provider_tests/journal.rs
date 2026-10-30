//! The run journal with each CLI's rows side by side: Claude Code's and
//! Codex's, as each reads and writes them. Claude Code's driver's own rows are
//! its tests' (`droppen_provider_claude_code::resident::journal_tests`).

use droppen_provider::runs::*;
use droppen_provider_claude_code::resident::ClaudeRowsExt;
use droppen_provider_codex::app_server::CodexRowsExt;

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

/// The one row `add` puts in a journal of its own, as it then is on disk.
fn on_disk(add: impl FnOnce(&RunsFile)) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = RunsFile::new(dir.path().join("runs.json"));
    add(&journal);
    journal.raw().pop().expect("the row")
}

/// `row` read as a Claude Code run and written again.
fn rewritten_as_claude(row: &str) -> String {
    on_disk(|journal| journal.add_claude_row(row))
}

/// `row` read as a Codex run and written again.
fn rewritten_as_codex(row: &str) -> String {
    on_disk(|journal| journal.add_codex_row(row))
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
    journal.add_claude_row(CLAUDE_PRINT);
    let read: Vec<String> = journal.read().into_iter().map(|run| run.run_id).collect();
    assert_eq!(read, ["run-print"]);
    assert_eq!(journal.raw().len(), 3, "the rows it cannot read stay as they were");
}
