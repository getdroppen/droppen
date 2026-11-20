//! The oracle for a Codex parse that goes on from an earlier one, through the
//! path the loader takes ([`Resumable::resume`] after
//! [`ResumableLines::reopen`]): after every append, and every change to the
//! files a message names, it has to be the parse of the whole rollout from
//! its start, item for item, chips and fingerprints included, and every item
//! below where it says it changed has to be the very one it went on from (or
//! that item with other chips).

use std::any::Any;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use super::codex::{read_codex_transcript, CodexSessions};
use droppen_instance::paths::Paths;
use droppen_sessions::loader::fingerprint_chain;
use droppen_sessions::resume::{ReadPoint, Resumable, ResumableLines, Resumed, READ_ON_MAX};
use droppen_sessions::util::{ItemKind, TranscriptItem};
use droppen_sessions::Source;

/// What a step left to go on from: its items, where it stopped, its state.
type Held = (Vec<Arc<TranscriptItem>>, ReadPoint, Box<dyn Any + Send>);

/// Go on from `held` as the loader does; `None` when the file or the reader
/// refuses.
async fn resume(paths: &Paths, path: &Path, held: Held) -> Option<Resumed> {
    let (items, point, state) = held;
    let lines = ResumableLines::reopen(path, &point, READ_ON_MAX).await.expect("reopen")?;
    CodexSessions.resume(paths, path, lines, items, state).await.expect("resume")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    On,
    Whole,
}

/// A rollout written a piece at a time, and read after each.
struct Oracle {
    _dir: tempfile::TempDir,
    home: PathBuf,
    path: PathBuf,
    paths: Paths,
    held: Option<Held>,
}

struct Step {
    read: Read,
    items: Vec<Arc<TranscriptItem>>,
    changed_from: usize,
}

impl Oracle {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().to_path_buf();
        for (file, body) in [("proj/notes.md", "n"), ("proj/src/lib.rs", "l"), ("proj/pic.png", "p"), ("other/src/lib.rs", "o")] {
            let path = home.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        }
        let path = home.join("sessions/rollout-2026-10-04T23-54-59-11111111-2222-3333-4444-555555555555.jsonl");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(&path, b"").expect("rollout");
        Self {
            paths: Paths::new(home.clone()),
            _dir: dir,
            home,
            path,
            held: None,
        }
    }

    fn append(&self, bytes: &[u8]) {
        let mut file = std::fs::OpenOptions::new().append(true).open(&self.path).expect("open");
        file.write_all(bytes).expect("append");
    }

    /// Read on from the last step, and hold it to a parse of the whole file.
    async fn step(&mut self) -> Step {
        let whole = read_codex_transcript(&self.paths, &self.path).await.expect("whole");
        let before = self.held.as_ref().map(|(items, _, _)| items.clone());
        let resumed = match self.held.take() {
            Some(held) => resume(&self.paths, &self.path, held).await,
            None => None,
        };
        let (parsed, read) = match (resumed, before) {
            (Some(parsed), Some(before)) => {
                assert!(parsed.changed_from <= before.len(), "nothing past the end can be the same");
                for (index, (then, now)) in before.iter().zip(&parsed.items).take(parsed.changed_from).enumerate() {
                    assert!(
                        Arc::ptr_eq(then, now)
                            || ((now.is_annotated() || then.is_annotated())
                                && serde_json::to_value(then.as_transcribed()).expect("json") == serde_json::to_value(now.as_transcribed()).expect("json")),
                        "item {index}, below where it says it changed, is not the one it went on from"
                    );
                }
                (parsed, Read::On)
            }
            _ => (read_codex_transcript(&self.paths, &self.path).await.expect("whole"), Read::Whole),
        };
        assert_eq!(json(&parsed.items), json(&whole.items), "the items, chips included");
        assert_eq!(
            fingerprint_chain(&self.path, &parsed.items),
            fingerprint_chain(&self.path, &whole.items),
            "the fingerprints"
        );
        assert_eq!(
            (parsed.point.end(), parsed.point.open_tail()),
            (whole.point.end(), whole.point.open_tail()),
            "where it stopped"
        );
        let step = Step {
            read,
            items: parsed.items.clone(),
            changed_from: parsed.changed_from,
        };
        self.held = Some((parsed.items, parsed.point, parsed.state));
        step
    }
}

fn json(items: &[Arc<TranscriptItem>]) -> Vec<Value> {
    items.iter().map(|item| serde_json::to_value(&**item).expect("json")).collect()
}

const AT: &str = "2026-10-04T14:55:07.760Z";

/// One rollout line, with its newline.
fn line(kind: &str, payload: Value) -> Vec<u8> {
    format!("{}\n", json!({"timestamp": AT, "type": kind, "payload": payload})).into_bytes()
}

fn message(role: &str, text: &str) -> Vec<u8> {
    let part = if role == "user" { "input_text" } else { "output_text" };
    line("response_item", json!({"type": "message", "role": role, "content": [{"type": part, "text": text}]}))
}

fn reasoning(text: &str) -> Vec<u8> {
    line("response_item", json!({"type": "reasoning", "summary": [{"type": "summary_text", "text": text}]}))
}

fn cell(call_id: &str, source: &str) -> Vec<u8> {
    line(
        "response_item",
        json!({"type": "custom_tool_call", "status": "completed", "call_id": call_id, "name": "exec", "input": source}),
    )
}

fn cell_output(call_id: &str, header: &str, printed: &[&str]) -> Vec<u8> {
    let mut parts = vec![json!({"type": "input_text", "text": header})];
    parts.extend(printed.iter().map(|text| json!({"type": "input_text", "text": text})));
    line("response_item", json!({"type": "custom_tool_call_output", "call_id": call_id, "output": parts}))
}

fn completed(item: Value) -> Vec<u8> {
    line(
        "event_msg",
        json!({"type": "item_completed", "thread_id": "t", "turn_id": "u", "item": item,
               "started_at_ms": 1_791_125_707_887u64, "completed_at_ms": 1_791_125_708_387u64}),
    )
}

fn command(id: &str, script: &str, output: &str) -> Value {
    json!({
        "type": "CommandExecution", "id": id, "process_id": "98684",
        "command": ["/bin/zsh", "-lc", script], "cwd": "file:///w",
        "parsed_cmd": [{"type": "unknown", "cmd": script}], "source": "unified_exec_startup",
        "status": "completed", "stdout": output, "stderr": "", "aggregated_output": output,
        "exit_code": 0, "duration": {"secs": 0, "nanos": 412_000_000}, "formatted_output": output,
    })
}

fn function_call(call_id: &str, name: &str, args: Value) -> Vec<u8> {
    line("response_item", json!({"type": "function_call", "name": name, "arguments": args.to_string(), "call_id": call_id}))
}

fn function_output(call_id: &str, output: Value) -> Vec<u8> {
    line("response_item", json!({"type": "function_call_output", "call_id": call_id, "output": output}))
}

const TWO_COMMANDS: &str = "const r = await Promise.allSettled([\n  tools.exec_command({cmd:\"pwd\"}),\n  tools.exec_command({cmd:\"ls\"}),\n]);\nr.forEach((r,i)=>text({i,...r}));";

/// The pieces of the rollout the targeted tests take apart, by name.
struct Pieces {
    meta: Vec<u8>,
    cell: Vec<u8>,
    first_command: Vec<u8>,
    second_command: Vec<u8>,
    cell_done: Vec<u8>,
}

fn pieces(oracle: &Oracle) -> Pieces {
    Pieces {
        meta: [
            line("session_meta", json!({"id": "x", "cwd": oracle.home.join("proj")})),
            line("turn_context", json!({"model": "gpt-5"})),
        ]
        .concat(),
        cell: cell("call_A", TWO_COMMANDS),
        first_command: completed(command("exec-1", "pwd", "/w\n")),
        second_command: completed(command("exec-2", "ls", "a\n")),
        cell_done: cell_output("call_A", "Script completed\nWall time 0.1 seconds\nOutput:\n", &[r#"{"i":0,"status":"fulfilled"}"#]),
    }
}

/// A rollout with every kind of line the parser reads, and every way it
/// changes, takes out or puts in an item it made earlier.
fn fixture(oracle: &Oracle) -> Vec<Vec<u8>> {
    let pieces = pieces(oracle);
    let patch = "*** Begin Patch\n*** Update File: MEMORY.md\n@@\n ## Active Context\n-- First startup.\n+- Replied in #all.\n*** End Patch\n";
    let mut lines = vec![
        pieces.meta,
        message("user", "start"),
        message("assistant", "see `~/proj/notes.md` and :codex-file-citation{path=\"~/proj/src/lib.rs\"}"),
        message("assistant", "and `src/lib.rs`, where the session runs"),
        function_call("call_R", "view_image", json!({"path": "pic.png"})),
        function_output("call_R", json!("viewed")),
        reasoning("Weighing it"),
        reasoning("Weighing it up"),
        function_call("call_X", "exec_command", json!({"cmd": "pwd"})),
        function_output("call_X", json!("Chunk ID: ab\nWall time: 0.0 seconds\nProcess exited with code 0\nOutput:\n/w\n")),
        completed(command("call_X", "pwd", "/w\n")),
        function_call("call_Q", "request_user_input", json!({"questions": [{"question": "Which?", "options": [{"label": "a"}]}]})),
        message("assistant", "waiting for you"),
        function_output("call_Q", json!("a")),
        function_call("call_S", "exec_command", json!({"cmd": "sleep 30"})),
        function_output("call_S", json!("Chunk ID: ab\nWall time: 10.0 seconds\nProcess running with session ID 5\nOriginal token count: 0\nOutput:\n")),
        completed(command("call_S", "sleep 30", "")),
        pieces.cell,
        pieces.first_command,
        pieces.second_command,
        pieces.cell_done,
        message("assistant", "and `~/proj/notes.md` again"),
        line(
            "response_item",
            json!({"type": "custom_tool_call", "status": "completed", "call_id": "call_P", "name": "apply_patch", "input": patch}),
        ),
        completed(json!({
            "type": "FileChange", "id": "call_P",
            "changes": {"/w/MEMORY.md": {"type": "update", "move_path": null,
                "unified_diff": "@@ -9,2 +9,2 @@\n ## Active Context\n-- First startup.\n+- Replied in #all.\n"}},
            "status": "completed", "stdout": "", "stderr": "",
        })),
        line(
            "response_item",
            json!({"type": "custom_tool_call_output", "call_id": "call_P",
                   "output": "Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nM MEMORY.md\n"}),
        ),
        cell("call_B", "text(await tools.exec_command({cmd:\"python build.py\",workdir:\"/w\",yield_time_ms:1000}))"),
        cell_output(
            "call_B",
            "Script completed\nWall time 1.4 seconds\nOutput:\n",
            &[r#"{"chunk_id":"6da718","wall_time_seconds":1.0,"session_id":26552,"original_token_count":0,"output":""}"#],
        ),
        completed(command("exec-9", "python build.py", "Pages rendered\n")),
        function_call("call_V", "view_image", json!({"path": "~/proj/pic.png"})),
        function_output("call_V", json!("viewed")),
        line("response_item", json!({"type": "web_search_call", "id": "ws_1", "status": "completed", "action": {"type": "search", "query": "rust"}})),
        line("turn_context", json!({"model": "gpt-5-mini"})),
        line("session_meta", json!({"id": "x", "cwd": oracle.home.join("other")})),
        message("assistant", "now `src/lib.rs` is the other one"),
        function_call("call_W", "view_image", json!({"path": "pic.png"})),
        function_output("call_W", json!("viewed")),
        format!("{}\r\n", json!({"timestamp": AT, "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "written on Windows"}]}})).into_bytes(),
        b"{not json\n".to_vec(),
        b"\n".to_vec(),
    ];
    // A whole entry the CLI has not ended the line of.
    let last = message("assistant", "no newline yet");
    lines.push(last[..last.len() - 1].to_vec());
    lines
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// The whole fixture, written in pieces cut at random bytes (inside lines and
/// between them), read on after every piece.
#[tokio::test]
async fn a_rollout_appended_in_random_pieces_reads_as_its_whole_parse_at_every_step() {
    for seed in 1..=80u64 {
        let mut oracle = Oracle::new();
        let bytes = fixture(&oracle).concat();
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut cuts: Vec<usize> = (0..1 + rng.below(20)).map(|_| 1 + rng.below(bytes.len() - 1)).collect();
        cuts.push(bytes.len());
        cuts.sort_unstable();
        cuts.dedup();
        let mut from = 0;
        for (step, cut) in cuts.into_iter().enumerate() {
            let piece = &bytes[from..cut];
            oracle.append(piece);
            from = cut;
            let read = oracle.step().await.read;
            if step > 0 && piece != b"\r" {
                assert_eq!(read, Read::On, "seed {seed}, step {step}: an append is read on from the last step");
            }
        }
    }
}

/// The same, a line at a time.
#[tokio::test]
async fn a_rollout_appended_a_line_at_a_time_reads_as_its_whole_parse() {
    let mut oracle = Oracle::new();
    for (step, piece) in fixture(&oracle).iter().enumerate() {
        oracle.append(piece);
        let read = oracle.step().await.read;
        assert_eq!(read, if step == 0 { Read::Whole } else { Read::On });
    }
}

/// A cell's own row, taken out once its calls said everything, is the one
/// item that changes; and a message written into its place gets its chips,
/// though the walk had passed that place before.
#[tokio::test]
async fn a_cells_row_taken_out_changes_nothing_before_it_and_its_place_is_walked_again() {
    let mut oracle = Oracle::new();
    let pieces = pieces(&oracle);
    oracle.append(&[pieces.meta, pieces.cell, pieces.first_command, pieces.second_command].concat());
    let open = oracle.step().await;
    let row = open.items.len() - 1;
    assert_eq!(open.items[row].tool_use_id.as_deref(), Some("call_A"), "the cell's row, last");
    oracle.append(&[pieces.cell_done, message("assistant", "made `~/proj/notes.md`")].concat());
    let done = oracle.step().await;
    assert_eq!((done.read, done.changed_from), (Read::On, row));
    assert_eq!(done.items[row].text, "made `~/proj/notes.md`", "the message in the row's place");
    assert_eq!(done.items[row].files.as_ref().map(Vec::len), Some(1), "and its chip");
}

/// What a cell's call did is put in ahead of the cell's own row, which is
/// where the items change from.
#[tokio::test]
async fn a_calls_rows_put_in_ahead_of_the_cell_change_from_there() {
    let mut oracle = Oracle::new();
    let pieces = pieces(&oracle);
    oracle.append(&[pieces.meta, pieces.cell].concat());
    let open = oracle.step().await;
    let row = open.items.len() - 1;
    oracle.append(&pieces.first_command);
    let ran = oracle.step().await;
    assert_eq!((ran.read, ran.changed_from), (Read::On, row));
    assert_eq!(ran.items.last().and_then(|item| item.tool_use_id.as_deref()), Some("call_A"), "the cell's row stays last");
    assert_eq!(ran.items.len(), open.items.len() + 2);
}

/// Reasoning re-emitted at greater length replaces the last item, and only
/// that one.
#[tokio::test]
async fn longer_reasoning_replaces_the_last_item() {
    let mut oracle = Oracle::new();
    let pieces = pieces(&oracle);
    oracle.append(&[pieces.meta, message("user", "go"), reasoning("Weighing it")].concat());
    let short = oracle.step().await;
    oracle.append(&reasoning("Weighing it up"));
    let long = oracle.step().await;
    assert_eq!((long.read, long.changed_from, long.items.len()), (Read::On, short.items.len() - 1, short.items.len()));
    assert_eq!(long.items.last().map(|item| item.text.as_str()), Some("Weighing it up"));
}

/// A patch's own echo, read in one piece, gives its result the line numbers
/// in the next; and a question asked in one is answered in the next, under
/// its own name.
#[tokio::test]
async fn an_echo_and_a_question_carry_across_reads() {
    let mut oracle = Oracle::new();
    let pieces = pieces(&oracle);
    let patch = "*** Begin Patch\n*** Update File: MEMORY.md\n@@\n-- a\n+- b\n*** End Patch\n";
    oracle.append(&pieces.meta);
    oracle.append(&line(
        "response_item",
        json!({"type": "custom_tool_call", "status": "completed", "call_id": "call_P", "name": "apply_patch", "input": patch}),
    ));
    oracle.step().await;
    oracle.append(&completed(json!({
        "type": "FileChange", "id": "call_P",
        "changes": {"/w/MEMORY.md": {"type": "update", "move_path": null, "unified_diff": "@@ -4,1 +4,1 @@\n-- a\n+- b\n"}},
        "status": "completed", "stdout": "", "stderr": "",
    })));
    oracle.step().await;
    oracle.append(&line(
        "response_item",
        json!({"type": "custom_tool_call_output", "call_id": "call_P", "output": "Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess.\n"}),
    ));
    let patched = oracle.step().await;
    let result = patched.items.last().expect("the result");
    assert_eq!(result.structured_patch.as_ref().map(|hunks| hunks[0].old_start), Some(4), "the echo's line numbers");

    oracle.append(&function_call("call_Q", "request_user_input", json!({"questions": [{"question": "Which?"}]})));
    oracle.step().await;
    oracle.append(&function_output("call_Q", json!("a")));
    let answered = oracle.step().await;
    let answer = answered.items.last().expect("the answer");
    assert_eq!((answer.kind, answer.tool_name.0.as_deref()), (ItemKind::ToolResult, Some("request_user_input")));
}

/// A citation whose file exists reads as the file's label, and a file named
/// in a message read earlier gets its chip once it is written, and loses it
/// once deleted: the chain does not move.
#[tokio::test]
async fn citations_and_files_written_or_deleted_between_reads_keep_to_the_whole_parse() {
    let mut oracle = Oracle::new();
    let pieces = pieces(&oracle);
    oracle.append(&[pieces.meta, message("assistant", "see :codex-file-citation{path=\"~/report.md\"} and `~/proj/notes.md`")].concat());
    let absent = oracle.step().await;
    let said = absent.items.len() - 1;
    assert!(absent.items[said].text.contains(":codex-file-citation"), "no file, no label");

    std::fs::write(oracle.home.join("report.md"), "r").expect("write");
    let written = oracle.step().await;
    assert_eq!((written.read, written.changed_from), (Read::On, written.items.len()));
    assert!(!written.items[said].text.contains(":codex-file-citation"), "the citation reads as its label");
    assert_eq!(written.items[said].files.as_ref().map(Vec::len), Some(2));

    std::fs::remove_file(oracle.home.join("report.md")).expect("delete");
    let deleted = oracle.step().await;
    assert!(deleted.items[said].text.contains(":codex-file-citation"));
}

/// The session's directory, named in an earlier read, resolves a relative
/// mention and a relative image read on in a later one; and one named again
/// mid-rollout resolves what comes after it.
#[tokio::test]
async fn the_sessions_directory_carries_across_reads() {
    let mut oracle = Oracle::new();
    let pieces = pieces(&oracle);
    oracle.append(&pieces.meta);
    oracle.step().await;
    oracle.append(
        &[
            message("assistant", "see `src/lib.rs`"),
            function_call("call_V", "view_image", json!({"path": "pic.png"})),
            function_output("call_V", json!("viewed")),
        ]
        .concat(),
    );
    let read = oracle.step().await;
    assert_eq!(read.read, Read::On);
    let proj = oracle.home.join("proj");
    let said = read.items.iter().find(|item| item.text == "see `src/lib.rs`").expect("the message");
    let chips: Vec<String> = said.files.iter().flatten().map(|file| file.path.clone()).collect();
    assert_eq!(chips, [proj.join("src/lib.rs").to_string_lossy().into_owned()]);
    let shown = read.items.iter().find(|item| item.images.is_some()).expect("the thumbnail");
    assert_eq!(shown.images, Some(vec![proj.join("pic.png").to_string_lossy().into_owned()]));

    oracle.append(&[line("session_meta", json!({"id": "x", "cwd": oracle.home.join("other")})), message("assistant", "and `src/lib.rs` here")].concat());
    let moved = oracle.step().await;
    assert_eq!(moved.read, Read::On);
    let here = moved.items.last().expect("the message");
    let chips: Vec<String> = here.files.iter().flatten().map(|file| file.path.clone()).collect();
    assert_eq!(chips, [oracle.home.join("other/src/lib.rs").to_string_lossy().into_owned()]);
}

/// A rollout read compressed goes on in the plain file Codex writes back
/// before it appends; one still compressed is read whole.
#[tokio::test]
async fn a_rollout_read_compressed_goes_on_in_the_plain_file_written_back() {
    use droppen_sessions::compressed::{compress, compressed_path};

    let mut oracle = Oracle::new();
    let lines = fixture(&oracle);
    let first = lines[..12].concat();
    std::fs::remove_file(&oracle.path).expect("plain gone");
    std::fs::write(compressed_path(&oracle.path), compress(&first)).expect("compressed");
    assert_eq!(oracle.step().await.read, Read::Whole);
    assert_eq!(oracle.step().await.read, Read::Whole, "a compressed file is not read on from a point");
    std::fs::write(&oracle.path, [&first[..], &lines[12..].concat()[..]].concat()).expect("written back, grown");
    std::fs::remove_file(compressed_path(&oracle.path)).expect("compressed gone");
    assert_eq!(oracle.step().await.read, Read::On);
}

/// A rollout rewritten, truncated or replaced is read whole.
#[tokio::test]
async fn a_rewritten_rollout_is_read_whole() {
    let mut oracle = Oracle::new();
    let lines = fixture(&oracle);
    oracle.append(&lines[..10].concat());
    oracle.step().await;
    std::fs::write(&oracle.path, [lines[..4].concat(), message("assistant", "rewritten"), lines[10..20].concat()].concat()).expect("rewrite");
    assert_eq!(oracle.step().await.read, Read::Whole, "the bytes before the stop changed");
    std::fs::write(&oracle.path, lines[..3].concat()).expect("truncate");
    assert_eq!(oracle.step().await.read, Read::Whole, "shorter");
    std::fs::write(&oracle.path, [line("session_meta", json!({"id": "other"})), lines[1..].concat()].concat()).expect("replace");
    assert_eq!(oracle.step().await.read, Read::Whole, "another first line");
}

/// Items or a state that are not the parse's own are no place to go on from.
#[tokio::test]
async fn items_or_a_state_that_are_not_the_parses_are_refused() {
    let mut oracle = Oracle::new();
    let lines = fixture(&oracle);
    oracle.append(&lines[..8].concat());
    oracle.step().await;
    let (mut items, point, state) = oracle.held.take().expect("held");
    items.pop();
    assert!(resume(&oracle.paths, &oracle.path, (items.clone(), point.clone(), state)).await.is_none(), "an item short");
    assert!(resume(&oracle.paths, &oracle.path, (items, point, Box::new(()))).await.is_none(), "another reader's state");
}

/// The fixture reads as it should from the start, so the steps hold it to
/// something: every kind of item, the folds, the chips and the label.
#[tokio::test]
async fn the_fixture_reads_as_it_should() {
    let mut oracle = Oracle::new();
    oracle.append(&fixture(&oracle).concat());
    let items = oracle.step().await.items;
    let kinds: Vec<ItemKind> = items.iter().map(|item| item.kind).collect();
    for kind in [ItemKind::Text, ItemKind::Thinking, ItemKind::ToolUse, ItemKind::ToolResult] {
        assert!(kinds.contains(&kind), "a {kind:?} item");
    }
    assert_eq!(items.iter().filter(|item| item.kind == ItemKind::Thinking).count(), 1, "the reasoning, replaced");
    assert!(!items.iter().any(|item| item.tool_use_id.as_deref() == Some("call_A")), "the cell's row, taken out");
    assert!(items.iter().any(|item| item.tool_use_id.as_deref() == Some("call_S#finished")), "the command finishing later");
    assert!(items.iter().any(|item| item.structured_patch.is_some()), "the patch's echo");
    assert!(items.iter().any(|item| item.images.is_some()), "the image's thumbnail");
    assert!(items.iter().any(|item| item.files.as_ref().is_some_and(|files| files.len() == 2)), "chips for the file and the citation");
    assert_eq!(items.last().map(|item| item.text.as_str()), Some("no newline yet"));
}

/// Through the loader: a rollout appended to is read on from its last parse,
/// and what it answers is the whole rollout's parse, items and fingerprints.
#[tokio::test]
async fn through_the_loader_an_appended_rollout_is_read_on() {
    use droppen_sessions::loader::{reads_of, resume_from};

    let oracle = Oracle::new();
    let lines = fixture(&oracle);
    let mut sources = droppen_sessions::TranscriptSources::default();
    sources.register(Arc::new(CodexSessions));
    resume_from(&oracle.path, 0);
    let load = || async {
        let sig = droppen_sessions::file_signature(&oracle.path).await.expect("a signature");
        sources.load(&oracle.paths, Source::Codex, &oracle.path, &sig).await
    };
    oracle.append(&lines[..16].concat());
    let first = load().await;
    first.chain();
    oracle.append(&lines[16..].concat());
    let grown = load().await;
    let whole = read_codex_transcript(&oracle.paths, &oracle.path).await.expect("whole");
    assert_eq!(json(&grown.items), json(&whole.items));
    assert_eq!(&grown.chain()[..], fingerprint_chain(&oracle.path, &whole.items).as_slice());
    let reads = reads_of(&oracle.path);
    assert_eq!((reads.full, reads.resumed), (1, 1));
}
