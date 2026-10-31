//! How much of the model's context window a session is currently occupying.
//!
//! Distinct from [`crate::stats::usage`], which totals what a session has *spent*
//! over its whole life. This is the instantaneous reading behind the context
//! ring: how full the window was on the most recent request, which only ever
//! reflects the last turn and falls back down when the CLI compacts.

/// How far back each reader looks for the reading: every CLI writes one on
/// every turn, so the tail is always enough.
pub const TAIL_BYTES: u64 = 128 * 1024;

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
/// reads here as an ordinary 200k one. Two things correct that:
/// [`ContextUsage::run_by`], which puts the marker back from the name of the
/// process the session runs in, and [`resolve_window`], which uses the reading
/// itself as evidence when no such process is there to ask.
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

/// The suffix that asks Claude Code for a model's long-context variant
/// (`claude-opus-4-8[1m]`), in any case, as the CLI reads it.
const LONG_CONTEXT_MARKER: &str = "[1m]";

/// What the window rules read off a model id.
pub trait ModelIdExt {
    /// The id without the long-context marker at its end, in whatever case it
    /// is written: `deepseek-v4.1-flash[1M]` is `deepseek-v4.1-flash`, the name
    /// the CLI sends to the API and records.
    fn without_long_context_marker(&self) -> &str;

    /// `asked`, when it is this recorded id asked for with the marker.
    ///
    /// Claude Code takes the long-context variant of a model as `<model>[1m]`,
    /// runs it against the 1M window, and writes `<model>` on every transcript
    /// entry. What the CLI is asked for keeps the marker (the process' own
    /// name for its model in its `system:init`, or what the next message asks
    /// for), so a recorded id that is that name less the marker was run with
    /// the marker. `None` when `asked` names another model or no variant, and
    /// when the recorded id already says which variant it is.
    fn long_context_as(&self, asked: &str) -> Option<String>;
}

impl ModelIdExt for str {
    fn without_long_context_marker(&self) -> &str {
        let mut id = self;
        while let Some(head) = id
            .len()
            .checked_sub(LONG_CONTEXT_MARKER.len())
            .filter(|&at| id.get(at..).is_some_and(|tail| tail.eq_ignore_ascii_case(LONG_CONTEXT_MARKER)))
            .map(|at| &id[..at])
        {
            id = head;
        }
        id
    }

    fn long_context_as(&self, asked: &str) -> Option<String> {
        let model = asked.without_long_context_marker();
        let marked = model.len() < asked.len();
        let unmarked = self.without_long_context_marker().len() == self.len();
        (marked && unmarked && model.eq_ignore_ascii_case(self)).then(|| asked.to_string())
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

impl ContextUsage {
    /// The reading with its model named as the session's process runs it or
    /// the next message asks for it (`asked`): the long-context marker the
    /// transcript leaves out is put back when the two name the same model
    /// ([`ModelIdExt::long_context_as`]).
    ///
    /// A session asked for `deepseek-v4.1-flash[1m]` records
    /// `deepseek-v4.1-flash`, which [`context_window`] reads as a 200k model.
    /// The marker is only in what the CLI is asked for, so that is what
    /// restores it. A reading of another model, taken before the session was
    /// switched, keeps its own name and its own window.
    pub fn run_by(mut self, asked: Option<&str>) -> Self {
        if let Some(named) = self.model.as_deref().zip(asked).and_then(|(model, asked)| model.long_context_as(asked)) {
            self.model = Some(named);
        }
        self
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// #446: a third-party model asked for with `[1m]`. Claude Code records
    /// `deepseek-v4.1-flash` on the entry (claude 2.1.312, `--model` or
    /// `ANTHROPIC_MODEL` alike), so the reading alone says 200k; the process'
    /// own name for its model says 1M.
    #[test]
    fn a_reading_of_a_long_context_ask_is_measured_against_the_long_window() {
        let recorded = ContextUsage {
            tokens: 30_000,
            window: None,
            model: Some("deepseek-v4.1-flash".to_string()),
        };
        assert_eq!(resolve_window(Some(&recorded), None), DEFAULT_CONTEXT_WINDOW, "the transcript alone cannot tell");
        let run = recorded.clone().run_by(Some("deepseek-v4.1-flash[1m]"));
        assert_eq!(run.model.as_deref(), Some("deepseek-v4.1-flash[1m]"));
        assert_eq!(resolve_window(Some(&run), None), LARGE_CONTEXT_WINDOW);
        assert_eq!(run.tokens, 30_000, "the occupancy is the reading's");

        for running in ["deepseek-v4.1-flash[1M]", "DeepSeek-V4.1-Flash[1m]"] {
            assert_eq!(
                resolve_window(Some(&recorded.clone().run_by(Some(running))), None),
                LARGE_CONTEXT_WINDOW,
                "{running}: the marker and the id in any case, as the CLI reads them"
            );
        }
    }

    /// Only the marker comes from the process, and only for its own model: a
    /// reading taken before a switch, or by a process asked for no variant,
    /// keeps the window its own name gives it.
    #[test]
    fn the_marker_is_restored_only_on_the_model_that_was_asked_for_with_it() {
        let reading = |model: &str| ContextUsage {
            tokens: 30_000,
            window: None,
            model: Some(model.to_string()),
        };
        for (recorded, running) in [
            ("glm-5", Some("deepseek-v4.1-flash[1m]")),
            ("deepseek-v4.1", Some("deepseek-v4.1-flash[1m]")),
            ("deepseek-v4.1-flash", Some("deepseek-v4.1-flash")),
            ("deepseek-v4.1-flash", None),
        ] {
            let run = reading(recorded).run_by(running);
            assert_eq!(run.model.as_deref(), Some(recorded), "{recorded} run by {running:?}");
            assert_eq!(resolve_window(Some(&run), None), DEFAULT_CONTEXT_WINDOW, "{recorded} run by {running:?}");
        }
        let marked = reading("claude-opus-4-8[1M]").run_by(Some("claude-opus-4-8[1m]"));
        assert_eq!(marked.model.as_deref(), Some("claude-opus-4-8[1M]"), "an id that says its variant is left as it says it");
        let unnamed = ContextUsage {
            tokens: 30_000,
            window: None,
            model: None,
        };
        assert_eq!(unnamed.clone().run_by(Some("deepseek-v4.1-flash[1m]")), unnamed);
    }

    #[test]
    fn the_marker_comes_off_the_end_in_any_case() {
        assert_eq!("deepseek-v4.1-flash[1m]".without_long_context_marker(), "deepseek-v4.1-flash");
        assert_eq!("claude-opus-4-8[1M]".without_long_context_marker(), "claude-opus-4-8");
        assert_eq!("opus[1m][1m]".without_long_context_marker(), "opus", "as the CLI strips a repeated one");
        assert_eq!("claude-opus-4-8".without_long_context_marker(), "claude-opus-4-8");
        assert_eq!("[1m]x".without_long_context_marker(), "[1m]x", "the end only");
        assert_eq!("éabc".without_long_context_marker(), "éabc", "a cut inside a character is no panic");
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
}
