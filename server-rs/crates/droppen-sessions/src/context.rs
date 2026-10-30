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
