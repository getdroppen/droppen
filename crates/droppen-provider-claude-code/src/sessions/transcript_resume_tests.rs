//! The oracle for a parse that goes on from an earlier one
//! ([`resume_claude_transcript`]): after every append, and every change to
//! what a parse reads besides the transcript, it has to be the parse of the
//! whole file from its start, item for item, chips and fingerprints included,
//! and every item below where it says it changed has to be the very one it
//! went on from.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use super::transcript::{read_claude_transcript, resume_claude_transcript, subagent_dir, ClaudeCheckpoint};
use droppen_instance::paths::Paths;
use droppen_sessions::loader::fingerprint_chain;
use droppen_sessions::util::{ItemKind, TranscriptItem};
use droppen_sessions::Source;

/// How a step was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// On from the last step.
    On,
    /// From the start: the file was not the last step's, grown.
    Whole,
}

/// A transcript written a piece at a time, and read after each.
struct Oracle {
    _dir: tempfile::TempDir,
    home: PathBuf,
    path: PathBuf,
    paths: Paths,
    sidechain: bool,
    held: Option<(Vec<Arc<TranscriptItem>>, ClaudeCheckpoint)>,
}

/// What one step read.
struct Step {
    read: Read,
    items: Vec<Arc<TranscriptItem>>,
    changed_from: usize,
}

impl Oracle {
    fn new() -> Self {
        Self::with(false)
    }

    fn with(sidechain: bool) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().to_path_buf();
        for (file, body) in [("proj/notes.md", "n"), ("proj/src/lib.rs", "l"), ("proj/pic.png", "p")] {
            let path = home.join(file);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        }
        let path = home.join("s/session.jsonl");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(&path, b"").expect("transcript");
        Self {
            paths: Paths::new(home.clone()),
            _dir: dir,
            home,
            path,
            sidechain,
            held: None,
        }
    }

    fn append(&self, bytes: &[u8]) {
        let mut file = std::fs::OpenOptions::new().append(true).open(&self.path).expect("open");
        file.write_all(bytes).expect("append");
    }

    /// Read on from the last step, and hold it to a parse of the whole file.
    async fn step(&mut self) -> Step {
        let whole = read_claude_transcript(&self.paths, &self.path, self.sidechain).await.expect("whole");
        let (parsed, read) = match self.held.take() {
            Some((before, checkpoint)) => match resume_claude_transcript(&self.paths, &self.path, before.clone(), checkpoint).await.expect("resume") {
                Some(parsed) => {
                    assert!(parsed.changed_from <= before.len(), "nothing past the end can be the same");
                    // Below it, an item is the one it went on from, or that
                    // item with other chips: what the chain hashes is the same.
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
                None => (read_claude_transcript(&self.paths, &self.path, self.sidechain).await.expect("whole"), Read::Whole),
            },
            None => (read_claude_transcript(&self.paths, &self.path, self.sidechain).await.expect("whole"), Read::Whole),
        };
        assert_eq!(json(&parsed.items), json(&whole.items), "the items, chips included");
        assert_eq!(
            fingerprint_chain(&self.path, &parsed.items),
            fingerprint_chain(&self.path, &whole.items),
            "the fingerprints"
        );
        let (end, open) = (parsed.checkpoint.point().end(), parsed.checkpoint.point().open_tail());
        assert_eq!((end, open), (whole.checkpoint.point().end(), whole.checkpoint.point().open_tail()), "where it stopped");
        let step = Step {
            read,
            items: parsed.items.clone(),
            changed_from: parsed.changed_from,
        };
        self.held = Some((parsed.items, parsed.checkpoint));
        step
    }
}

fn json(items: &[Arc<TranscriptItem>]) -> Vec<Value> {
    items.iter().map(|item| serde_json::to_value(&**item).expect("json")).collect()
}

fn line(value: Value) -> Vec<u8> {
    format!("{value}\n").into_bytes()
}

fn at(n: usize) -> String {
    format!("2026-11-20T10:{:02}:{:02}.000Z", n / 60, n % 60)
}

fn assistant(n: usize, content: Value) -> Value {
    json!({"type": "assistant", "timestamp": at(n), "message": {"model": "claude-opus-4-5", "content": content}})
}

fn user(n: usize, content: Value) -> Value {
    json!({"type": "user", "timestamp": at(n), "message": {"role": "user", "content": content}})
}

/// A transcript with every kind of entry the parser reads, and every way it
/// changes an item it made earlier, a line each; and a subagent's meta for
/// its `Task` call.
fn fixture(oracle: &Oracle) -> Vec<Vec<u8>> {
    let cwd = oracle.home.join("proj");
    let dir = subagent_dir(&oracle.path);
    std::fs::create_dir_all(&dir).expect("subagents");
    std::fs::write(dir.join("agent-a1.meta.json"), json!({"toolUseId": "task1", "agentType": "Explore"}).to_string()).expect("meta");
    let mut lines = vec![
        line(json!({"type": "user", "cwd": cwd, "timestamp": at(0), "message": {"role": "user", "content": "start"}})),
        line(assistant(1, json!([{"type": "thinking", "thinking": "hmm"}, {"type": "text", "text": "see `~/proj/notes.md`, `src/lib.rs` and `gone.md`"}]))),
        line(assistant(2, json!([{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]))),
        line(user(3, json!([{"type": "tool_result", "tool_use_id": "t1", "content": "a\nb"}]))),
        line(assistant(4, json!([{"type": "tool_use", "id": "q1", "name": "AskUserQuestion", "input": {"questions": [{"question": "Which?", "options": [{"label": "a"}]}]}}]))),
        line(assistant(5, json!([{"type": "text", "text": "waiting for you"}]))),
        line(user(6, json!([{"type": "tool_result", "tool_use_id": "q1", "content": "a"}]))),
        line(json!({"type": "system", "subtype": "compact_boundary", "timestamp": at(1), "compactMetadata": {"trigger": "auto", "preTokens": 9, "postTokens": 1}})),
        line(json!({"type": "user", "isCompactSummary": true, "timestamp": at(8), "message": {"content": "the summary"}})),
        line(user(9, json!("<command-name>/model</command-name>\n<command-args>opus</command-args>"))),
        line(user(10, json!("<local-command-stdout>set to opus</local-command-stdout>"))),
        line(assistant(11, json!([{"type": "tool_use", "id": "task1", "name": "Task", "input": {"description": "explore"}}]))),
        line(json!({"type": "assistant", "isSidechain": true, "timestamp": at(12), "message": {"content": [{"type": "text", "text": "inside the agent"}]}})),
        line(user(13, json!([{"type": "tool_result", "tool_use_id": "task1", "content": "found it"}]))),
        line(json!({"type": "attachment", "timestamp": at(14), "attachment": {"type": "queued_command", "prompt": "also this", "commandMode": "prompt"}})),
        line(user(15, json!("<task-notification><task-id>b1</task-id><status>completed</status><summary>done</summary><result>all good</result></task-notification>"))),
        line(assistant(16, json!([{"type": "fallback", "from": {"model": "a"}, "to": {"model": "b"}}]))),
        format!("{}\r\n", assistant(17, json!([{"type": "text", "text": "written on Windows"}]))).into_bytes(),
        b"{not json\n".to_vec(),
        [&b"{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"bad \xff byte\"}]}}"[..], b"\n"].concat(),
        line(assistant(19, json!([{"type": "tool_use", "id": "r1", "name": "Read", "input": {"file_path": "~/proj/pic.png"}}]))),
        line(user(20, json!([{"type": "tool_result", "tool_use_id": "r1", "content": [{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "cA=="}}]}]))),
        line(user(21, json!("[Request interrupted by user]"))),
        line(json!({"type": "assistant", "timestamp": at(22), "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "limit reached"}]}})),
        line(json!({"type": "user", "isMeta": true, "timestamp": at(23), "message": {"content": "meta"}})),
        b"\n".to_vec(),
        line(assistant(24, json!([{"type": "tool_use", "id": "bg", "name": "Bash", "input": {"command": "sleep 9", "run_in_background": true}}]))),
        line(assistant(25, json!([{"type": "text", "text": "last words, about `~/proj/notes.md`"}]))),
    ];
    // Ended without a newline: a whole entry the CLI has not finished the
    // line of.
    lines.push(assistant(26, json!([{"type": "text", "text": "no newline yet"}])).to_string().into_bytes());
    lines
}

/// A small xorshift, so a failing seed is the same split every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// The whole fixture, written in pieces cut at random bytes (inside lines
/// and between them alike), read on after every piece: each read is the
/// whole file's parse, and each goes on from the last.
#[tokio::test]
async fn a_transcript_appended_in_random_pieces_reads_as_its_whole_parse_at_every_step() {
    for sidechain in [false, true] {
        for seed in 1..=60u64 {
            let mut oracle = Oracle::with(sidechain);
            let bytes = fixture(&oracle).concat();
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut cuts: Vec<usize> = (0..1 + rng.below(16)).map(|_| 1 + rng.below(bytes.len() - 1)).collect();
            cuts.push(bytes.len());
            cuts.sort_unstable();
            cuts.dedup();
            let mut from = 0;
            for (step, cut) in cuts.into_iter().enumerate() {
                let piece = &bytes[from..cut];
                oracle.append(piece);
                from = cut;
                let read = oracle.step().await.read;
                // A carriage return alone after an entry taken without its
                // newline cannot say yet whether the line ended.
                if step > 0 && piece != b"\r" {
                    assert_eq!(read, Read::On, "seed {seed}, step {step}: an append is read on from the last step");
                }
            }
        }
    }
}

/// The same, a line at a time.
#[tokio::test]
async fn a_transcript_appended_a_line_at_a_time_reads_as_its_whole_parse() {
    let mut oracle = Oracle::new();
    for (step, piece) in fixture(&oracle).iter().enumerate() {
        oracle.append(piece);
        let read = oracle.step().await.read;
        assert_eq!(read, if step == 0 { Read::Whole } else { Read::On });
    }
}

/// An answer settles the question it answers, wherever that is, and the
/// count of what changed goes down to it and no further.
#[tokio::test]
async fn an_answer_changes_its_question_and_nothing_before_it() {
    let mut oracle = Oracle::new();
    let lines = fixture(&oracle);
    oracle.append(&lines[..6].concat());
    let asked = oracle.step().await;
    let question = asked.items.iter().position(|item| item.question.0.is_some()).expect("a pending question");
    oracle.append(&lines[6]);
    let answered = oracle.step().await;
    assert_eq!((answered.read, answered.changed_from), (Read::On, question));
    assert!(answered.items[question].question.0.is_none());
}

/// A compact's summary and a command's output fold into the item before
/// them, which is all that changes.
#[tokio::test]
async fn a_summary_and_a_commands_output_change_only_the_item_they_fold_into() {
    let mut oracle = Oracle::new();
    let lines = fixture(&oracle);
    oracle.append(&lines[..8].concat());
    let boundary = oracle.step().await;
    oracle.append(&lines[8]);
    let summary = oracle.step().await;
    assert_eq!((summary.changed_from, summary.items.len()), (boundary.items.len() - 1, boundary.items.len()));
    assert_eq!(summary.items.last().and_then(|item| item.detail.0.as_deref()), Some("the summary"));

    oracle.append(&lines[9]);
    let command = oracle.step().await;
    oracle.append(&lines[10]);
    let output = oracle.step().await;
    assert_eq!((output.changed_from, output.items.len()), (command.items.len() - 1, command.items.len()));
    assert_eq!(output.items.last().and_then(|item| item.detail.0.as_deref()), Some("set to opus"));
}

/// A subagent's meta can land after its launch was read, go, or name
/// another agent: with no line written, the tags follow it, and only the
/// items they move on count as changed.
#[tokio::test]
async fn a_subagents_tags_follow_its_meta_with_no_line_written() {
    let mut oracle = Oracle::new();
    let lines = fixture(&oracle);
    let meta = subagent_dir(&oracle.path).join("agent-a1.meta.json");
    std::fs::remove_file(&meta).expect("no meta yet");
    oracle.append(&lines[..14].concat());
    let untagged = oracle.step().await;
    let call = untagged.items.iter().position(|item| item.tool_use_id.as_deref() == Some("task1")).expect("the call");
    let result = untagged.items.iter().position(|item| item.result_of.as_deref() == Some("task1")).expect("the result");
    assert!(untagged.items[call].agent.is_none());

    std::fs::write(&meta, json!({"toolUseId": "task1"}).to_string()).expect("meta");
    let tagged = oracle.step().await;
    assert_eq!((tagged.read, tagged.changed_from), (Read::On, call));
    assert_eq!(tagged.items[result].agent.as_deref(), Some("a1"));

    let unchanged = oracle.step().await;
    assert_eq!(unchanged.changed_from, unchanged.items.len(), "nothing moved");

    std::fs::remove_file(&meta).expect("gone");
    std::fs::write(subagent_dir(&oracle.path).join("agent-b2.meta.json"), json!({"toolUseId": "task1"}).to_string()).expect("another");
    let retagged = oracle.step().await;
    assert_eq!((retagged.changed_from, retagged.items[call].agent.as_deref()), (call, Some("b2")));

    std::fs::remove_file(subagent_dir(&oracle.path).join("agent-b2.meta.json")).expect("gone");
    let gone = oracle.step().await;
    assert!(gone.items[call].agent.is_none() && gone.items[result].agent.is_none());
}

/// A file a message named, written after the message was read, gets its
/// chip on the next read, and loses it when deleted. The chain does not
/// move: nothing counts as changed.
#[tokio::test]
async fn a_file_written_after_its_mention_was_read_gets_its_chip() {
    let mut oracle = Oracle::new();
    oracle.append(&line(json!({"type": "user", "cwd": oracle.home.join("proj"), "timestamp": at(0), "message": {"content": "write it"}})));
    oracle.append(&line(assistant(1, json!([{"type": "text", "text": "I will write `~/report.md`"}]))));
    let before = oracle.step().await;
    assert!(before.items[1].files.is_none());

    std::fs::write(oracle.home.join("report.md"), "r").expect("write");
    let written = oracle.step().await;
    assert_eq!((written.read, written.changed_from), (Read::On, written.items.len()));
    assert_eq!(written.items[1].files.as_ref().map(Vec::len), Some(1));

    std::fs::remove_file(oracle.home.join("report.md")).expect("delete");
    let deleted = oracle.step().await;
    assert!(deleted.items[1].files.is_none());
}

/// A relative path resolves against the session's directory, which a
/// transcript can name only after a message that mentions one: once it is
/// named, that message gets its chip, read whole or read on.
#[tokio::test]
async fn a_mention_read_before_the_session_named_its_directory_gets_its_chip_once_it_does() {
    let mut oracle = Oracle::new();
    oracle.append(&line(assistant(0, json!([{"type": "text", "text": "see `notes.md`"}]))));
    let before = oracle.step().await;
    assert!(before.items[0].files.is_none(), "nothing to resolve it against yet");
    oracle.append(&line(json!({"type": "user", "cwd": oracle.home.join("proj"), "timestamp": at(1), "message": {"content": "go on"}})));
    let named = oracle.step().await;
    assert_eq!(named.read, Read::On);
    assert_eq!(named.items[0].files.as_ref().map(Vec::len), Some(1));
}

/// The `stat` budget is spent from the first message on, read whole or in
/// pieces: a message after it is spent gets no chip, until a file an
/// earlier message named turns up and the walk starts over.
#[tokio::test]
async fn the_stat_budget_is_spent_as_a_parse_from_the_start_spends_it() {
    let mut oracle = Oracle::new();
    let drafts: Vec<String> = (0..droppen_sessions::mentions::MAX_STATS_PER_TRANSCRIPT).map(|n| format!("`~/d{n}.md`")).collect();
    oracle.append(&line(assistant(0, json!([{"type": "text", "text": format!("drafts: {}", drafts.join(" "))}]))));
    oracle.step().await;
    oracle.append(&line(assistant(1, json!([{"type": "text", "text": "and `~/proj/notes.md`"}]))));
    let spent = oracle.step().await;
    assert!(spent.items[1].files.is_none(), "every stat went on the drafts");

    std::fs::write(oracle.home.join("d0.md"), "d").expect("write");
    let again = oracle.step().await;
    assert_eq!(again.items[0].files.as_ref().map(Vec::len), Some(1));
}

/// A whole entry with no newline after it is read, and the next read goes
/// on once the newline comes; a line that turns out to go on past it is read
/// from the start.
#[tokio::test]
async fn an_entry_waiting_for_its_newline_is_read_and_the_newline_awaited() {
    let mut oracle = Oracle::new();
    let entry = assistant(0, json!([{"type": "text", "text": "one"}])).to_string();
    oracle.append(entry.as_bytes());
    assert_eq!(oracle.step().await.items.len(), 1);
    oracle.append(b"\n");
    oracle.append(&line(assistant(1, json!([{"type": "text", "text": "two"}]))));
    let on = oracle.step().await;
    assert_eq!((on.read, on.items.len()), (Read::On, 2));

    let mut longer = Oracle::new();
    longer.append(entry.as_bytes());
    longer.step().await;
    longer.append(b" trailing words\n");
    let whole = longer.step().await;
    assert_eq!((whole.read, whole.items.len()), (Read::Whole, 0), "one line, and not an entry");
}

/// A line still being written is left until it ends.
#[tokio::test]
async fn a_half_written_line_is_read_once_it_ends() {
    let mut oracle = Oracle::new();
    let entry = line(assistant(0, json!([{"type": "text", "text": "slowly"}])));
    let (head, tail) = entry.split_at(entry.len() / 2);
    oracle.append(head);
    assert!(oracle.step().await.items.is_empty());
    oracle.append(tail);
    let ended = oracle.step().await;
    assert_eq!((ended.read, ended.items.len()), (Read::On, 1));
}

/// A transcript rewritten, longer or shorter, or replaced by another, is
/// read from its start.
#[tokio::test]
async fn a_rewritten_transcript_is_read_from_its_start() {
    let mut oracle = Oracle::new();
    let lines = fixture(&oracle);
    oracle.append(&lines[..10].concat());
    oracle.step().await;

    std::fs::write(&oracle.path, [&lines[..4].concat()[..], &line(assistant(40, json!([{"type": "text", "text": "rewritten"}])))[..]].concat())
        .expect("rewrite");
    oracle.append(&lines[10..20].concat());
    assert_eq!(oracle.step().await.read, Read::Whole, "the bytes before the stop changed");

    std::fs::write(&oracle.path, lines[..3].concat()).expect("truncate");
    assert_eq!(oracle.step().await.read, Read::Whole, "shorter");

    std::fs::write(&oracle.path, [line(user(0, json!("another session"))), lines[1..].concat()].concat()).expect("replace");
    assert_eq!(oracle.step().await.read, Read::Whole, "another first line");
}

/// A transcript read compressed goes on in the plain file written back,
/// which is the same text grown; one still compressed is read whole.
#[tokio::test]
async fn a_transcript_read_compressed_goes_on_in_the_plain_file_written_back() {
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

/// A chat's delivery records are each matched once, in file order, whether
/// the entries are read in one parse or in several.
#[tokio::test]
async fn a_chats_deliveries_are_each_matched_once_across_reads() {
    let mut oracle = Oracle::new();
    let chats = droppen_sessions::chats_cwd(&oracle.paths);
    let png = |name: &str| {
        let path = oracle.paths.uploads_dir().join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("uploads");
        std::fs::write(&path, b"png").expect("upload");
        path.to_string_lossy().into_owned()
    };
    let (first, second) = (png("u/1.png"), png("u/2.png"));
    for (id, image) in [("q-1", &first), ("q-2", &second)] {
        droppen_sessions::chat_delivery::record(&oracle.paths, Source::Claude, "session", id, "look", std::slice::from_ref(image), &[]);
    }
    let said = |n: usize| line(json!({"type": "user", "cwd": chats, "timestamp": format!("2999-01-01T00:00:0{n}.000Z"), "message": {"content": "look"}}));
    oracle.append(&said(1));
    let one = oracle.step().await;
    oracle.append(&said(2));
    let two = oracle.step().await;
    assert_eq!(two.read, Read::On);
    let images: Vec<Option<Vec<String>>> = two.items.iter().map(|item| item.images.clone()).collect();
    assert_eq!(images, [Some(vec![second.clone()]), Some(vec![first])], "the latest row first, then the one left");
    assert_eq!(one.items[0].images, Some(vec![second]));
}

/// Through the loader: a chat's delivery recorded after its entry was read
/// (the driver records one after it sends the message) has the next version
/// read whole, which finds it; while the rows stand still, versions are read
/// on.
#[tokio::test]
async fn a_delivery_recorded_after_its_entry_was_read_has_the_session_read_whole() {
    use droppen_sessions::loader::{reads_of, resume_from};

    let oracle = Oracle::new();
    let mut sources = droppen_sessions::TranscriptSources::default();
    sources.register(Arc::new(super::claude::ClaudeCodeSessions));
    resume_from(&oracle.path, 0);
    let load = || async {
        let sig = droppen_sessions::file_signature(&oracle.path).await.expect("a signature");
        sources.load(&oracle.paths, Source::Claude, &oracle.path, &sig).await
    };
    let image = oracle.paths.uploads_dir().join("u/1.png");
    std::fs::create_dir_all(image.parent().expect("parent")).expect("uploads");
    std::fs::write(&image, b"png").expect("upload");
    let image = image.to_string_lossy().into_owned();

    let chats = droppen_sessions::chats_cwd(&oracle.paths);
    oracle.append(&line(json!({"type": "user", "cwd": chats, "timestamp": "2999-01-01T00:00:01.000Z", "message": {"content": "look"}})));
    assert!(load().await.items[0].images.is_none(), "read before its delivery was recorded");

    droppen_sessions::chat_delivery::record(&oracle.paths, Source::Claude, "session", "q-1", "look", std::slice::from_ref(&image), &[]);
    oracle.append(&line(assistant(2, json!([{"type": "text", "text": "a fine picture"}]))));
    assert_eq!(load().await.items[0].images, Some(vec![image]), "read whole, the delivery found");
    oracle.append(&line(assistant(3, json!([{"type": "text", "text": "anything else?"}]))));
    load().await;
    let reads = reads_of(&oracle.path);
    assert_eq!((reads.full, reads.fallback, reads.resumed), (1, 1, 1));
}

/// Items that are not the parse's own are no place to go on from.
#[tokio::test]
async fn items_that_are_not_the_checkpoints_are_refused() {
    let mut oracle = Oracle::new();
    let lines = fixture(&oracle);
    oracle.append(&lines[..5].concat());
    oracle.step().await;
    let (mut items, checkpoint) = oracle.held.take().expect("held");
    items.pop();
    let refused = resume_claude_transcript(&oracle.paths, &oracle.path, items, checkpoint).await.expect("resume");
    assert!(refused.is_none());
}

/// The fixture reads as it should from the start, so the oracle's steps
/// hold it to something: every kind of item, the folds, the tag and the
/// chips.
#[tokio::test]
async fn the_fixture_reads_as_it_should() {
    let mut oracle = Oracle::new();
    oracle.append(&fixture(&oracle).concat());
    let items = oracle.step().await.items;
    let kinds: Vec<ItemKind> = items.iter().map(|item| item.kind).collect();
    for kind in [ItemKind::Text, ItemKind::Thinking, ItemKind::ToolUse, ItemKind::ToolResult, ItemKind::Compact, ItemKind::Command] {
        assert!(kinds.contains(&kind), "a {kind:?} item");
    }
    assert!(items.iter().any(|item| item.agent.as_deref() == Some("a1")), "the subagent's tag");
    assert!(items.iter().any(|item| item.files.as_ref().is_some_and(|files| files.len() == 2)), "chips for the two files that exist");
    assert!(items.iter().any(|item| item.images.is_some()), "the image result's thumbnail");
    assert_eq!(items.last().map(|item| item.text.as_str()), Some("no newline yet"));
    assert!(!items.iter().any(|item| item.text == "inside the agent"), "the sidechain is the agent's");
}
