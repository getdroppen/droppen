//! What a task's turn said, read off its session's transcript: the reply the
//! `summary` artifact carries, and the pull requests the `result` names
//! (`docs/a2a.md` §3.3).
//!
//! A session's transcript holds every turn it ever ran, the Droppen page's
//! own messages among them, so a task's part of it is found by its own
//! messages: from the last user message that is its first one, up to the
//! first user message after that which is none of its own.

use droppen_sessions::util::{ItemKind, Role, TranscriptItem};

/// How much of a message's text finds it in the transcript again.
const PROBE_CHARS: usize = 512;

/// How long a progress line may be: what the protocol side cuts its status
/// messages to (§3.2).
pub(crate) const PROGRESS_CHARS: usize = 280;

/// What a message is found by in a transcript: the start of its text,
/// whitespace collapsed, as the transcript's own user item reads.
///
/// A prefix because the CLIs add to a prompt (an attachment list after it)
/// and the parser can cut a huge one; the task's own random fence early in
/// the prompt keeps two tasks' prefixes apart.
pub(crate) fn probe(text: &str) -> String {
    collapse(text).chars().take(PROBE_CHARS).collect()
}

/// `text` with every run of whitespace one space, trimmed.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A line of progress: the agent's latest sentence, on one line, cut to
/// [`PROGRESS_CHARS`].
pub(crate) fn progress_line(text: &str) -> String {
    let flat = collapse(text);
    if flat.chars().count() <= PROGRESS_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(PROGRESS_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// What a task's turns said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Said {
    /// The last reply: the assistant text at the end of the task's part, its
    /// consecutive blocks joined by a blank line.
    pub summary: String,
    /// Every assistant text of the task's part, oldest first.
    pub texts: Vec<String>,
}

/// What the turns of the task whose messages `probes` find said: `probes[0]`
/// is its first message, the rest the ones added to it.
pub(crate) fn said(items: &[TranscriptItem], probes: &[String]) -> Said {
    let user_text = |item: &TranscriptItem| item.role == Role::User && item.kind == ItemKind::Text;
    let finds = |item: &TranscriptItem, probe: &String| !probe.is_empty() && collapse(&item.text).contains(probe.as_str());
    let first = probes.first();
    // The last user message that is the task's first one, else (a parser
    // that rewrote it past recognition) the last user message there is.
    let start = items
        .iter()
        .rposition(|item| user_text(item) && first.is_some_and(|probe| finds(item, probe)))
        .or_else(|| items.iter().rposition(user_text));
    let Some(start) = start else {
        return Said::default();
    };
    let part = &items[start + 1..];
    // Up to the first user message that is none of the task's own: whatever
    // follows it is another turn's.
    let end = part
        .iter()
        .position(|item| user_text(item) && !probes.iter().any(|probe| finds(item, probe)))
        .unwrap_or(part.len());
    let part = &part[..end];
    let assistant_text = |item: &TranscriptItem| item.role == Role::Assistant && item.kind == ItemKind::Text && !item.text.trim().is_empty();
    let texts: Vec<String> = part
        .iter()
        .filter(|item| assistant_text(item))
        .map(|item| item.text.trim().to_string())
        .collect();
    // The reply at the end: back past whatever trails it that says nothing
    // (a notice, a tool the turn ended in), then every text block of that
    // reply, thinking between them aside.
    let mut reply = Vec::new();
    for item in part.iter().rev() {
        if assistant_text(item) {
            reply.push(item.text.trim());
        } else if reply.is_empty() || (item.role == Role::Assistant && item.kind == ItemKind::Thinking) {
            continue;
        } else {
            break;
        }
    }
    reply.reverse();
    Said {
        summary: reply.join("\n\n"),
        texts,
    }
}

/// The GitHub pull requests `texts` link to, each once, in the order first
/// named: `https://github.com/<owner>/<repo>/pull/<number>`, whatever
/// follows the number (`/files`, `#issuecomment-…`) dropped.
pub(crate) fn pr_urls<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    const PREFIX: &str = "https://github.com/";
    let mut found: Vec<String> = Vec::new();
    for text in texts {
        let mut rest = text;
        while let Some(at) = rest.find(PREFIX) {
            rest = &rest[at + PREFIX.len()..];
            if let Some(url) = pull_request(rest) {
                if !found.contains(&url) {
                    found.push(url);
                }
            }
        }
    }
    found
}

/// The pull request `after` (what follows `https://github.com/`) names, if
/// it names one.
fn pull_request(after: &str) -> Option<String> {
    let name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    let mut parts = after.splitn(4, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let pull = parts.next()?;
    let tail = parts.next()?;
    let number: String = tail.chars().take_while(char::is_ascii_digit).collect();
    let plain = |part: &str| !part.is_empty() && part.chars().all(name) && !part.starts_with('.');
    (plain(owner) && plain(repo) && pull == "pull" && !number.is_empty() && number.len() <= 12)
        .then(|| format!("https://github.com/{owner}/{repo}/pull/{number}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(role: Role, kind: ItemKind, text: &str) -> TranscriptItem {
        serde_json::from_value(serde_json::json!({
            "role": role, "kind": kind, "text": text, "detail": null, "toolName": null, "meta": null,
            "isError": false, "question": null, "timestamp": null, "truncated": false,
        }))
        .expect("an item")
    }

    fn user(text: &str) -> TranscriptItem {
        item(Role::User, ItemKind::Text, text)
    }

    fn says(text: &str) -> TranscriptItem {
        item(Role::Assistant, ItemKind::Text, text)
    }

    #[test]
    fn a_task_s_part_runs_from_its_message_to_the_next_message_not_its_own() {
        let items = vec![
            user("an earlier task"),
            says("earlier reply"),
            user("fence-1 fix the bug"),
            says("looking"),
            item(Role::Assistant, ItemKind::ToolUse, "Bash"),
            item(Role::User, ItemKind::ToolResult, "ok"),
            user("fence-1 also the docs"),
            says("Done."),
            item(Role::Assistant, ItemKind::Thinking, "hm"),
            says("PR: https://github.com/o/r/pull/7"),
            user("the page's own message"),
            says("something else"),
        ];
        let probes = vec![probe("fence-1 fix the bug"), probe("fence-1 also the docs")];
        let said = said(&items, &probes);
        assert_eq!(said.summary, "Done.\n\nPR: https://github.com/o/r/pull/7");
        assert_eq!(said.texts, ["looking", "Done.", "PR: https://github.com/o/r/pull/7"]);
    }

    #[test]
    fn a_reply_followed_by_a_tool_is_still_the_last_reply() {
        let items = vec![
            user("go"),
            says("first"),
            item(Role::Assistant, ItemKind::ToolUse, "Bash"),
            says("last"),
            item(Role::System, ItemKind::Text, "notice"),
        ];
        assert_eq!(said(&items, &[probe("go")]).summary, "last");
    }

    #[test]
    fn a_message_the_parser_rewrote_falls_back_to_the_last_user_message() {
        let items = vec![user("one"), says("a"), user("two"), says("b")];
        assert_eq!(said(&items, &[probe("not in the transcript")]).summary, "b");
        assert_eq!(said(&[], &[probe("x")]), Said::default());
    }

    #[test]
    fn pull_requests_are_named_once_without_what_follows_the_number() {
        let urls = pr_urls([
            "Opened https://github.com/acme/web.app/pull/12 and https://github.com/acme/web.app/pull/12/files.",
            "See https://github.com/acme/api/pull/3#issuecomment-1, not https://github.com/acme/api/issues/4",
            "nor https://github.com/acme/pull/5 or https://github.com/../x/pull/6",
        ]);
        assert_eq!(urls, ["https://github.com/acme/web.app/pull/12", "https://github.com/acme/api/pull/3"]);
    }

    #[test]
    fn a_progress_line_is_one_line_and_cut() {
        assert_eq!(progress_line("  two\nlines  "), "two lines");
        let long = progress_line(&"x".repeat(500));
        assert_eq!(long.chars().count(), PROGRESS_CHARS);
        assert!(long.ends_with('…'));
    }
}
