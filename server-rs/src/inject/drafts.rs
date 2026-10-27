//! What a resident claude is thinking right now, which its transcript cannot
//! say yet.
//!
//! The feed renders a session from its transcript file, and the CLI writes that
//! file late: a finished content block reaches it only once the whole API
//! response has ended (measured against claude 2.1.295: a text block on stdout
//! at 7.89 s was in the file at 9.43 s). The page's transcript only ever grows
//! by what the file holds — it used to draw the finished blocks early, as a
//! provisional bubble below the last message, and that bubble came and went and
//! changed height under the reader several times a turn. What the page does
//! show of the gap is one piece of live state: that the agent is thinking, and
//! how far it has got, as a count in the activity bar above the composer
//! (`web/src/features/sessions/turn-status.tsx`).
//!
//! The CLI writes `system:thinking_tokens` with every chunk of a thinking block
//! it is generating, carrying its running estimate of the block's length. The
//! resident reads every stdout line anyway (`resident.rs`), so this keeps that
//! estimate per session, and the session feed sends it beside the transcript as
//! a `draft` frame (see [`crate::feed`]); the cloud agent relays the same body
//! to the hub as an ephemeral record. A draft is never part of the transcript —
//! no index, no fingerprint, nothing a resume, an unread count or a search is
//! computed from.
//!
//! # The frame
//!
//! `{"type":"draft","turn","v","blocks":[{"index":0,"kind":"thinking","thinkingTokens":N}]}`
//! while a think is being counted, and `"blocks":[]` once it is not. That is
//! the shape drafts had when they also carried finished text and tool calls,
//! cut down to the one block that was a counter then too, so both sides of a
//! version gap read it:
//!
//! * a page from before draws it as its bubble's "thinking…" line and has
//!   nothing else to draw — no uuid to wait for, nothing to replace;
//! * a page from now reads only the counter out of a frame, which is all an
//!   agent from before sends besides finished blocks it then ignores.
//!
//! # Lifetime
//!
//! * The first `thinking_tokens` line of a block starts the count, and each
//!   one after moves it.
//! * The model being done with the block ends it: the session's next
//!   `assistant` line with a content block in it — the thinking block itself,
//!   printed finished, or whatever the model went on to say.
//! * A `result` ends it with the turn, and so does a reset, and a process going
//!   away: an exit, a respawn, the relink an account rotation does.
//! * A driver re-attached after a server restart starts empty; the next chunk
//!   of thinking starts it again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Serialize, Serializer};
use serde_json::Value;
use tokio::sync::watch;

/// One session's draft: the count of the think in flight, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    /// The newest turn that thought, counted by this process from one: the
    /// first count after a close starts the next. Informational.
    pub turn: u64,
    /// Bumped on every change a page can see, so two frames that say
    /// different things are never the same string.
    pub v: u64,
    /// The CLI's running estimate of the thinking block being generated, in
    /// tokens; `None` while nothing is being thought.
    pub thinking: Option<u64>,
    /// Whether a turn is still going. Not on the wire.
    pub open: bool,
}

impl Draft {
    /// Nothing to show: no think in flight.
    pub fn is_empty(&self) -> bool {
        self.thinking.is_none()
    }
}

/// The frame's body, in the shape every page since drafts began can read: see
/// the module docs.
impl Serialize for Draft {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Counter {
            index: usize,
            kind: &'static str,
            thinking_tokens: u64,
        }
        #[derive(Serialize)]
        struct Wire {
            turn: u64,
            v: u64,
            blocks: Vec<Counter>,
        }
        Wire {
            turn: self.turn,
            v: self.v,
            blocks: self
                .thinking
                .map(|tokens| Counter {
                    index: 0,
                    kind: "thinking",
                    thinking_tokens: tokens,
                })
                .into_iter()
                .collect(),
        }
        .serialize(serializer)
    }
}

// ---------------------------------------------------------------- reading

/// What one stdout line says about the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    /// The running token estimate of the thinking block being generated.
    Thinking(u64),
    /// A finished content block: the model is done with whatever it was
    /// thinking.
    Finished,
    /// The conversation was discarded (`/clear` and its kin).
    Reset,
}

/// Read one stdout line for what it says about the draft, or `None`.
///
/// Tolerant like everything else that reads this stream: a shape it does not
/// know says nothing and is never an error. Only the session's own thread is
/// read — a background agent's stream is forwarded with the `parent_tool_use_id`
/// of the call that started it, and its thinking is not the session's.
pub fn read(line: &Value) -> Option<Said> {
    if !line.get("parent_tool_use_id").is_none_or(Value::is_null) {
        return None;
    }
    let subtype = line.get("subtype").and_then(Value::as_str);
    match line.get("type").and_then(Value::as_str)? {
        "assistant" => finishes(line).then_some(Said::Finished),
        "system" if subtype == Some("thinking_tokens") => line.get("estimated_tokens").and_then(Value::as_u64).map(Said::Thinking),
        "conversation_reset" => Some(Said::Reset),
        _ => None,
    }
}

/// Whether an `assistant` line carries a content block — the CLI prints one
/// finished block per line — and so says the model has moved on. A text
/// block with nothing in it is not one: the CLI prints those and they say
/// nothing.
fn finishes(line: &Value) -> bool {
    match line.pointer("/message/content") {
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Array(blocks)) => blocks.iter().any(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => block.get("text").and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty()),
            Some(_) => true,
            None => false,
        }),
        _ => false,
    }
}

// ---------------------------------------------------------------- folding

/// Fold what a line said into `draft`; whether a page would see a change.
pub fn fold(draft: &mut Draft, said: Said) -> bool {
    match said {
        // Its `estimated_tokens` is the total *for the current block*, so a
        // new block starts again from zero — which is fine, because the
        // previous one's finished line has already ended its count.
        Said::Thinking(tokens) => {
            if !draft.open {
                draft.turn += 1;
                draft.open = true;
            }
            if draft.thinking == Some(tokens) {
                return false;
            }
            draft.thinking = Some(tokens);
            true
        }
        Said::Finished => draft.thinking.take().is_some(),
        Said::Reset => end(draft),
    }
}

/// Read and fold in one go.
pub fn apply(draft: &mut Draft, line: &Value) -> bool {
    read(line).is_some_and(|said| fold(draft, said))
}

/// The turn is over, or the process is gone: nothing is being thought. Answers
/// whether a page sees a change — `open` is not on the wire, so a turn that
/// ends with no think in flight costs no frame.
pub fn end(draft: &mut Draft) -> bool {
    draft.open = false;
    draft.thinking.take().is_some()
}

// ------------------------------------------------------------------ store

/// Every session's draft, as a `watch` per session.
///
/// A `watch` because a draft is a value rather than a log: a reader wants the
/// latest one, a slow reader may skip any number in between, and a socket that
/// attaches mid-think gets the current count for free. Created by whichever
/// side asks first — a feed may start following a session before any claude is
/// running in it, and has to be holding the same channel the driver later
/// writes into.
#[derive(Default)]
pub struct Drafts {
    channels: Mutex<HashMap<String, Arc<watch::Sender<Draft>>>>,
}

impl Drafts {
    /// The channel for `key`, made if this is the first ask.
    ///
    /// Entries nobody holds any more are swept on the way: a feed that stopped
    /// watching a session nothing is running in leaves one behind, and a
    /// server that runs for months sees a great many sessions.
    pub fn channel(&self, key: &str) -> Arc<watch::Sender<Draft>> {
        let mut channels = self.channels.lock().expect("drafts poisoned");
        if let Some(channel) = channels.get(key) {
            return channel.clone();
        }
        channels.retain(|_, channel| Arc::strong_count(channel) > 1 || channel.receiver_count() > 0 || !channel.borrow().is_empty());
        let channel = Arc::new(watch::Sender::new(Draft::default()));
        channels.insert(key.to_string(), channel.clone());
        channel
    }

    /// A reader of `key`'s draft, which has seen the current value.
    pub fn subscribe(&self, key: &str) -> watch::Receiver<Draft> {
        self.channel(key).subscribe()
    }

    /// How many sessions have a channel, for the test of the sweep.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.channels.lock().expect("drafts poisoned").len()
    }
}

/// A driver's hold on its session's draft. The only writer.
pub(super) struct DraftWriter {
    channel: Arc<watch::Sender<Draft>>,
}

impl DraftWriter {
    /// Take `channel` over for a new driver.
    ///
    /// Emptied first: whatever a previous driver of this session left in it is
    /// about a process that is not this one, and a re-attached driver starts
    /// empty on purpose (see the module docs).
    pub(super) fn new(channel: Arc<watch::Sender<Draft>>) -> Self {
        let writer = Self { channel };
        writer.end();
        writer
    }

    /// One live line of claude's stdout.
    pub(super) fn observe(&self, line: &Value) {
        if let Some(said) = read(line) {
            modify(&self.channel, |draft| fold(draft, said));
        }
    }

    /// Nothing more is coming: the turn ended, or the process is gone.
    pub(super) fn end(&self) {
        modify(&self.channel, end);
    }

    /// Follow the session to another id: claude forked it. The old id's draft
    /// is emptied, since nothing will ever move it there again.
    pub(super) fn retarget(&mut self, channel: Arc<watch::Sender<Draft>>) {
        self.end();
        self.channel = channel;
        self.end();
    }
}

/// Change a draft through `change`, publishing — with a new version — only
/// when it says something changed.
fn modify(channel: &watch::Sender<Draft>, change: impl FnOnce(&mut Draft) -> bool) {
    channel.send_if_modified(|draft| {
        let changed = change(draft);
        if changed {
            draft.v += 1;
        }
        changed
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tokens(n: u64) -> Value {
        json!({"type": "system", "subtype": "thinking_tokens", "estimated_tokens": n, "estimated_tokens_delta": 10})
    }

    fn assistant(content: Value) -> Value {
        json!({"type": "assistant", "uuid": "u-1", "message": {"id": "msg_1", "role": "assistant", "content": content}})
    }

    fn fed(lines: &[Value]) -> Draft {
        let mut draft = Draft::default();
        for value in lines {
            apply(&mut draft, value);
        }
        draft
    }

    /// The count follows `thinking_tokens` while the block is generated, and
    /// the block's own finished line — or anything the model says after it —
    /// ends it.
    #[test]
    fn thinking_tokens_move_a_count_a_finished_block_ends() {
        let mut draft = fed(&[tokens(40), tokens(250)]);
        assert_eq!(draft.thinking, Some(250));
        assert!(draft.open);
        assert!(!apply(&mut draft, &tokens(250)), "the same count again is no change");

        assert!(apply(&mut draft, &assistant(json!([{"type": "thinking", "thinking": "so then"}]))));
        assert_eq!(draft.thinking, None, "the think, printed finished");

        apply(&mut draft, &tokens(5));
        assert_eq!(draft.thinking, Some(5), "the next block counts from its own start");
        for moved_on in [
            assistant(json!([{"type": "text", "text": "done thinking"}])),
            assistant(json!([{"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {}}])),
            assistant(json!([{"type": "redacted_thinking", "data": "…"}])),
            assistant(json!("a string is the one text block it stands for")),
        ] {
            apply(&mut draft, &tokens(7));
            assert!(apply(&mut draft, &moved_on), "{moved_on}");
            assert_eq!(draft.thinking, None);
        }
        assert_eq!(draft.turn, 1, "all one turn");
    }

    /// The frame a page reads: the counter as the one block drafts always
    /// called it, or no blocks at all — never a uuid or a word of text.
    #[test]
    fn the_frame_is_the_counter_in_the_shape_pages_have_always_read() {
        let mut draft = fed(&[tokens(1240)]);
        draft.v = 3;
        assert_eq!(
            serde_json::to_value(&draft).expect("json"),
            json!({"turn": 1, "v": 3, "blocks": [{"index": 0, "kind": "thinking", "thinkingTokens": 1240}]})
        );
        end(&mut draft);
        assert_eq!(serde_json::to_value(&draft).expect("json"), json!({"turn": 1, "v": 3, "blocks": []}));
    }

    /// A background agent's thinking and speech are forwarded on the same
    /// stream, and are not the session's.
    #[test]
    fn a_subagents_forwarded_lines_are_not_the_sessions() {
        let mut draft = fed(&[tokens(42)]);
        let mut child = assistant(json!([{"type": "text", "text": "a subagent"}]));
        child["parent_tool_use_id"] = json!("toolu_child");
        let mut child_tokens = tokens(9);
        child_tokens["parent_tool_use_id"] = json!("toolu_child");
        assert!(!apply(&mut draft, &child));
        assert!(!apply(&mut draft, &child_tokens));
        assert_eq!(draft.thinking, Some(42));
    }

    /// Lines that say nothing about a think leave it alone: a tool result, the
    /// CLI taking a refused partial back, an empty text block, an init.
    #[test]
    fn nothing_else_moves_or_ends_it() {
        let mut draft = fed(&[tokens(42)]);
        for line in [
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}}),
            json!({"type": "system", "subtype": "model_refusal_fallback", "retracted_message_uuids": ["u-1"]}),
            json!({"type": "tombstone", "message": {"uuid": "u-1"}}),
            assistant(json!([{"type": "text", "text": "  "}])),
            assistant(json!([])),
            json!({"type": "assistant", "uuid": "u-2", "message": {}}),
            json!({"type": "system", "subtype": "init"}),
            json!({"type": "system", "subtype": "thinking_tokens"}),
            json!("not even an object"),
        ] {
            assert!(!apply(&mut draft, &line), "{line}");
        }
        assert_eq!(draft.thinking, Some(42));
    }

    /// A result ends the count with the turn, and the next count is the next
    /// turn; a reset ends it as well.
    #[test]
    fn the_turn_ending_ends_it() {
        let mut draft = fed(&[tokens(12)]);
        assert!(end(&mut draft));
        assert!(!draft.open && draft.thinking.is_none());
        assert!(!end(&mut draft), "ending twice is ending once");
        apply(&mut draft, &tokens(3));
        assert_eq!((draft.turn, draft.thinking), (2, Some(3)));
        assert!(apply(&mut draft, &json!({"type": "conversation_reset", "new_conversation_id": "c", "uuid": "r", "session_id": "s"})));
        assert!(draft.is_empty() && !draft.open);
    }

    /// The writer publishes every change with a new version and nothing for a
    /// line that changes nothing, and clears what a previous driver left.
    #[test]
    fn a_writer_publishes_changes_only() {
        let drafts = Drafts::default();
        drafts.channel("claude/s").send_modify(|draft| draft.thinking = Some(99));
        let mut rx = drafts.subscribe("claude/s");
        let writer = DraftWriter::new(drafts.channel("claude/s"));
        assert!(rx.has_changed().expect("open"), "a new driver clears what the last one left");
        assert!(rx.borrow_and_update().is_empty());

        writer.observe(&tokens(40));
        assert!(rx.has_changed().expect("open"));
        let seen = rx.borrow_and_update().clone();
        assert_eq!(seen.thinking, Some(40));
        writer.observe(&json!({"type": "system", "subtype": "init"}));
        writer.observe(&tokens(40));
        assert!(!rx.has_changed().expect("open"), "a line that changes nothing publishes nothing");

        writer.end();
        assert!(rx.has_changed().expect("open"));
        let closed = rx.borrow_and_update().clone();
        assert!(closed.is_empty() && !closed.open);
        assert!(closed.v > seen.v);
        writer.end();
        assert!(!rx.has_changed().expect("open"), "nothing left to clear is no frame");
    }

    /// The store keeps a channel while anybody holds it, and sweeps the rest.
    #[test]
    fn idle_channels_are_swept() {
        let drafts = Drafts::default();
        let held = drafts.channel("claude/held");
        let watched = drafts.subscribe("claude/watched");
        drop(drafts.channel("claude/idle"));
        assert_eq!(drafts.len(), 3);
        drop(drafts.channel("claude/next"));
        assert_eq!(drafts.len(), 3, "the idle one went, the new one came: held, watched, next");
        drop((held, watched));
    }
}
