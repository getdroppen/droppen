//! How much of the model's context window a session is currently occupying.
//!
//! Distinct from [`crate::stats::usage`], which totals what a session has *spent*
//! over its whole life. This is the instantaneous reading behind the context
//! ring: how full the window was on the most recent request, which only ever
//! reflects the last turn and falls back down when the CLI compacts.

use std::path::Path;

use serde_json::Value;

use super::util::{read_tail_lines, try_parse};

/// Both CLIs write the reading on every turn, so the tail is always enough.
const TAIL_BYTES: u64 = 128 * 1024;

/// The window assumed for an ordinary model.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
/// The window assumed for a model advertising the long-context variant.
pub const LARGE_CONTEXT_WINDOW: u64 = 1_000_000;

/// Whether an id names a generation whose 1M window is the default rather than
/// an opt-in.
///
/// `fable` and `mythos` are long-context throughout, and so is `opus-5`: for
/// that generation the maximum window *is* the default, with no `[1m]` variant
/// to ask for. Older opus and sonnet generations are the opposite — 1M is a
/// variant the CLI only runs when the id says so — so they are deliberately
/// absent, and the `[1m]` marker plus [`resolve_window`] speak for them.
///
/// The `opus-5` test is written against the segment rather than a prefix so a
/// later `opus-5x` cannot be read as this generation, and so `opus-4-5` — a
/// different model whose name ends in the same digit — is not.
fn long_context_family(model: &str) -> bool {
    if model.contains("fable") || model.contains("mythos") {
        return true;
    }
    model.split("opus-5").skip(1).any(|rest| !rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// The window a model id implies, for a CLI that does not report its own.
///
/// Two things earn the long window: a model advertising the long-context
/// variant with a `[1m]` suffix, and a member of a generation that is
/// long-context by default (see [`long_context_family`]). Everything else is
/// the standard window. The match is case-insensitive because the id reaches us
/// verbatim from the transcript, where its casing is the CLI's business.
///
/// This rule alone is not enough for Claude, and deliberately so: Claude Code
/// writes `message.model` as the bare id, with the `[1m]` marker stripped, so a
/// session running the 1M window is recorded as plain `claude-opus-4-8` and
/// reads here as an ordinary 200k one. [`resolve_window`] is what corrects
/// that, using the reading itself as evidence.
pub fn context_window(model: Option<&str>) -> u64 {
    let Some(model) = model else {
        return DEFAULT_CONTEXT_WINDOW;
    };
    let model = model.to_ascii_lowercase();
    if model.contains("[1m]") || long_context_family(&model) {
        LARGE_CONTEXT_WINDOW
    } else {
        DEFAULT_CONTEXT_WINDOW
    }
}

/// What a transcript's tail says about the window's occupancy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsage {
    /// Tokens that occupied the window on the last request.
    pub tokens: u64,
    /// The window size the CLI reported itself, when it does. Codex records one
    /// per turn, which beats inferring the window from the model id.
    pub window: Option<u64>,
    /// The model recorded on the very entry the reading came from.
    ///
    /// Not the session's current choice: a session can be switched between
    /// turns, and only the model that was in force for *this* request says
    /// anything about the window it was measured against.
    pub model: Option<String>,
}

/// The window a reading was taken against.
///
/// Three sources, in falling order of authority:
///
/// 1. The window the CLI stated for that turn. Codex records one, and a figure
///    the CLI computed itself is never second-guessed here — including when the
///    reading overruns it, since we have no better number to replace it with.
/// 2. The model recorded on the entry the reading came from, falling back to the
///    session's current choice when the entry named none.
/// 3. The reading itself. A request the API accepted carrying more than the
///    standard window cannot have been made against the standard window, so an
///    inferred window that the observation does not fit is proof the id was
///    under-specified — which is exactly what a stripped `[1m]` looks like.
///
/// Note what this does *not* do: it never touches `tokens`. The occupancy is
/// reported as the transcript recorded it, and it is the window that gets
/// corrected, because the window is the part that was wrong.
pub fn resolve_window(usage: Option<&ContextUsage>, cli_model: Option<&str>) -> u64 {
    let Some(usage) = usage else {
        return context_window(cli_model);
    };
    if let Some(window) = usage.window {
        return window;
    }
    let inferred = context_window(usage.model.as_deref().or(cli_model));
    if usage.tokens > inferred {
        // The largest window we know of. A reading beyond even this has no
        // better answer available, so it stays visibly over-full rather than
        // inventing a size to make the ring look tidy.
        LARGE_CONTEXT_WINDOW.max(inferred)
    } else {
        inferred
    }
}

/// Claude's occupancy: the last main-chain assistant entry that carries a usage.
///
/// Everything the request carried *in* counts — fresh input, what was replayed
/// out of the cache, and what was written into it — because all three occupy the
/// window. Sidechain entries are a subagent's own context, not this session's.
pub async fn read_claude_context(file_path: &Path) -> Option<ContextUsage> {
    let lines = read_tail_lines(file_path, TAIL_BYTES).await.ok()?;
    for line in lines.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        if entry.get("type").and_then(Value::as_str) != Some("assistant") || entry.get("isSidechain") == Some(&Value::Bool(true)) {
            continue;
        }
        let Some(message) = entry.get("message").and_then(Value::as_object) else {
            continue;
        };
        let Some(usage) = message.get("usage").and_then(Value::as_object) else {
            continue;
        };
        let field = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let tokens = field("input_tokens") + field("cache_read_input_tokens") + field("cache_creation_input_tokens");
        if tokens > 0 {
            let model = message.get("model").and_then(Value::as_str).map(str::to_owned);
            return Some(ContextUsage { tokens, window: None, model });
        }
    }
    None
}

/// An ACP agent's occupancy: the newest `usage` record, which is the agent's
/// own `usage_update` with the window it reported beside it.
pub async fn read_acp_context(file_path: &Path) -> Option<ContextUsage> {
    let lines = read_tail_lines(file_path, TAIL_BYTES).await.ok()?;
    lines.iter().rev().filter_map(|line| try_parse(line)).find_map(|entry| {
        if entry.get("type").and_then(Value::as_str) != Some("usage") {
            return None;
        }
        Some(ContextUsage {
            tokens: entry.get("used").and_then(Value::as_u64)?,
            window: entry.get("size").and_then(Value::as_u64).filter(|size| *size > 0),
            model: None,
        })
    })
}

/// Codex's occupancy: the newest `token_count` event's `last_token_usage`.
///
/// Deliberately *not* `total_token_usage`, which is cumulative over the session —
/// a long thread reports tens of millions against a 258k window, which is spend,
/// not occupancy. `last_token_usage.input_tokens` is the per-turn figure and
/// already includes whatever was replayed out of the cache, matching what the
/// Claude arm sums by hand.
pub async fn read_codex_context(file_path: &Path) -> Option<ContextUsage> {
    let lines = read_tail_lines(file_path, TAIL_BYTES).await.ok()?;
    for line in lines.iter().rev() {
        let Some(entry) = try_parse(line) else { continue };
        if entry.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        let Some(payload) = entry.get("payload").and_then(Value::as_object) else {
            continue;
        };
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(info) = payload.get("info").and_then(Value::as_object) else {
            continue;
        };
        let window = info.get("model_context_window").and_then(Value::as_u64).filter(|w| *w > 0);
        let tokens = info
            .get("last_token_usage")
            .and_then(Value::as_object)
            .and_then(|last| last.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if tokens > 0 {
            // No model is recorded here on purpose: codex states its own window
            // on the same event, so nothing downstream needs to infer one.
            return Some(ContextUsage { tokens, window, model: None });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn session(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
        (dir, path)
    }

    #[test]
    fn the_long_context_models_are_the_ones_that_say_so() {
        assert_eq!(context_window(Some("claude-opus-4-8[1m]")), LARGE_CONTEXT_WINDOW);
        assert_eq!(context_window(Some("claude-fable-5")), LARGE_CONTEXT_WINDOW);
        assert_eq!(context_window(Some("claude-opus-4-8")), DEFAULT_CONTEXT_WINDOW);
        assert_eq!(context_window(Some("gpt-5-codex")), DEFAULT_CONTEXT_WINDOW);
        assert_eq!(context_window(None), DEFAULT_CONTEXT_WINDOW, "an unknown model is the safe assumption");
    }

    /// The generations whose 1M window is the default, not a `[1m]` opt-in.
    /// Their transcripts carry no marker to read, so the id alone has to answer
    /// — otherwise every one of their sessions reads as 200k until it overruns.
    #[test]
    fn a_generation_that_is_long_context_by_default_needs_no_marker() {
        assert_eq!(context_window(Some("claude-opus-5")), LARGE_CONTEXT_WINDOW);
        assert_eq!(context_window(Some("claude-opus-5-20260401")), LARGE_CONTEXT_WINDOW);
        assert_eq!(
            context_window(Some("CLAUDE-OPUS-5")),
            LARGE_CONTEXT_WINDOW,
            "casing is the CLI's business"
        );
        assert_eq!(context_window(Some("claude-mythos-5")), LARGE_CONTEXT_WINDOW);
        assert_eq!(
            context_window(Some("claude-opus-4-5")),
            DEFAULT_CONTEXT_WINDOW,
            "a name that merely ends in the same digit is a different model"
        );
        assert_eq!(
            context_window(Some("claude-opus-50")),
            DEFAULT_CONTEXT_WINDOW,
            "and a later generation is not this one"
        );
        assert_eq!(context_window(Some("claude-sonnet-5")), DEFAULT_CONTEXT_WINDOW, "sonnet still opts in");
    }

    /// Everything the request carried in occupies the window, not just the
    /// uncached part — a session running almost entirely on cache reads is still
    /// nearly full, and summing only `input_tokens` would read as empty.
    #[tokio::test]
    async fn claude_sums_fresh_cached_and_written_input() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"model":"claude-fable-5","usage":{"input_tokens":2,"cache_read_input_tokens":878645,"cache_creation_input_tokens":259,"output_tokens":40}}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 878_906);
        assert_eq!(usage.window, None, "claude never states its own window");
        assert_eq!(
            usage.model.as_deref(),
            Some("claude-fable-5"),
            "the model on the entry comes back with the reading"
        );
    }

    /// Taken verbatim from a live session that reported 512,720 tokens against a
    /// 200k window — 256% full, which no request could have been.
    ///
    /// Claude Code writes `message.model` with the `[1m]` marker stripped, so
    /// the id says `claude-opus-4-8` whether the session runs the 200k window or
    /// the 1M one, and the id rule alone can only ever guess the short one. The
    /// reading settles it: the API accepted this prompt, so the window held it.
    #[tokio::test]
    async fn a_reading_the_standard_window_could_not_have_held_proves_the_long_one() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":2,"cache_creation_input_tokens":1634,"cache_read_input_tokens":511084,"output_tokens":644}}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 512_720);
        assert_eq!(
            context_window(usage.model.as_deref()),
            DEFAULT_CONTEXT_WINDOW,
            "the id alone still reads as an ordinary model"
        );
        assert_eq!(resolve_window(Some(&usage), Some("claude-opus-4-8")), LARGE_CONTEXT_WINDOW);
        assert_eq!(
            usage.tokens, 512_720,
            "the occupancy is reported as recorded; it is the window that was wrong"
        );
    }

    /// The reading and the session's current choice can disagree — a session
    /// switched models between turns — and only the model that was in force for
    /// the measured request says anything about the window it was measured
    /// against.
    #[tokio::test]
    async fn the_window_follows_the_model_on_the_entry_the_reading_came_from() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"model":"claude-fable-5","usage":{"input_tokens":10,"cache_read_input_tokens":40000,"cache_creation_input_tokens":0}}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(
            resolve_window(Some(&usage), Some("claude-opus-4-8")),
            LARGE_CONTEXT_WINDOW,
            "the entry outranks the current choice"
        );

        let entryless = ContextUsage {
            tokens: 40_010,
            window: None,
            model: None,
        };
        assert_eq!(
            resolve_window(Some(&entryless), Some("claude-fable-5")),
            LARGE_CONTEXT_WINDOW,
            "an entry naming no model falls back"
        );
    }

    /// A window the CLI computed itself is the authority, even when the reading
    /// overruns it: there is no better number to put in its place, and quietly
    /// widening it would hide a real disagreement.
    #[test]
    fn a_window_the_cli_stated_is_never_second_guessed() {
        let stated = ContextUsage {
            tokens: 400_000,
            window: Some(258_400),
            model: None,
        };
        assert_eq!(resolve_window(Some(&stated), Some("claude-fable-5")), 258_400);
    }

    /// After a compact the window empties, and the entries above the compaction
    /// point describe a context that no longer exists. Reading the *last* entry
    /// rather than the largest is what keeps the ring falling back down — and
    /// keeps a stale 512k reading from widening the window of a session that is
    /// now a tenth as full.
    #[tokio::test]
    async fn a_compacted_session_is_measured_by_its_newest_turn_not_its_biggest() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":2,"cache_read_input_tokens":511084,"cache_creation_input_tokens":1634}}}"#,
            r#"{"type":"user","isCompactSummary":true,"message":{"content":[{"type":"text","text":"summary"}]}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":3,"cache_read_input_tokens":0,"cache_creation_input_tokens":29997}}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 30_000, "the turn after the compact, not the one before it");
        assert_eq!(
            resolve_window(Some(&usage), Some("claude-opus-4-8")),
            DEFAULT_CONTEXT_WINDOW,
            "a reading that fits is not evidence of a wider window"
        );
    }

    /// A session that has not made a request yet has nothing to correct with, so
    /// the model id is all there is.
    #[test]
    fn without_a_reading_the_session_choice_is_the_only_evidence() {
        assert_eq!(resolve_window(None, Some("claude-fable-5")), LARGE_CONTEXT_WINDOW);
        assert_eq!(resolve_window(None, Some("claude-opus-5")), LARGE_CONTEXT_WINDOW);
        assert_eq!(resolve_window(None, Some("claude-opus-4-8")), DEFAULT_CONTEXT_WINDOW);
        assert_eq!(resolve_window(None, None), DEFAULT_CONTEXT_WINDOW);
    }

    #[tokio::test]
    async fn claude_reads_the_newest_main_chain_entry() {
        let (_dir, path) = session(&[
            r#"{"type":"assistant","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":500,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":9999,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"next"}]}}"#,
        ]);
        let usage = read_claude_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 500, "a subagent's context is not this session's");
    }

    #[tokio::test]
    async fn a_transcript_with_no_usage_has_no_reading() {
        let (_dir, path) = session(&[r#"{"type":"user","message":{"content":[{"type":"text","text":"hi"}]}}"#]);
        assert_eq!(read_claude_context(&path).await, None);
    }

    /// The cumulative total is spend, not occupancy: this entry reports 24M
    /// against a 258k window, and reading it would peg the ring forever.
    #[tokio::test]
    async fn codex_reads_the_turn_not_the_running_total() {
        let (_dir, path) = session(&[
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":258400,"last_token_usage":{"input_tokens":202004,"cached_input_tokens":11008,"output_tokens":629},"total_token_usage":{"input_tokens":24141990,"total_tokens":24251793}}}}"#,
        ]);
        let usage = read_codex_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 202_004);
        assert_eq!(usage.window, Some(258_400), "codex states its own window, so nothing is inferred");
    }

    #[tokio::test]
    async fn codex_without_a_stated_window_leaves_it_to_the_model_id() {
        let (_dir, path) = session(&[
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1200,"output_tokens":10}}}}"#,
        ]);
        let usage = read_codex_context(&path).await.expect("a reading");
        assert_eq!(usage.tokens, 1_200);
        assert_eq!(usage.window, None);
    }
}
