//! Download chips for files an assistant message points at.
//!
//! Port of the former TypeScript server's `mentions.ts`. A file the agent
//! produced or referenced should be one click away instead of a hunt through
//! the file browser, so path-like tokens in assistant prose are resolved and —
//! when they name a file that actually exists under `$HOME` — attached to the
//! item.
//!
//! Existence-gating is what keeps precision high; the two caps
//! ([`MAX_FILES_PER_ITEM`], [`MAX_STATS_PER_TRANSCRIPT`]) are what keeps the
//! `stat` cost of a very long transcript bounded.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::util::{FileRef, ItemKind, Role, TranscriptItem};

/// How many chips one message may carry.
pub const MAX_FILES_PER_ITEM: usize = 6;
/// How many `stat` calls one transcript may cost.
pub const MAX_STATS_PER_TRANSCRIPT: usize = 600;

/// A file a message names in its CLI's own markup rather than in prose: where
/// the reference sits in the text, and the path it names.
pub struct FileCitation {
    /// Where the reference sits in the message's text: what its chip's label replaces.
    pub range: Range<usize>,
    /// The path it names, as written.
    pub path: String,
}

/// How a CLI marks the files its messages name, if it has a way: its reader's
/// to say, as the crate knows no CLI's markup.
pub type FileCitations = fn(&str) -> Vec<FileCitation>;

/// A CLI with no markup of its own: every file is found in the prose.
pub fn no_citations(_text: &str) -> Vec<FileCitation> {
    Vec::new()
}

fn clean_token(token: &str) -> &str {
    token
        .trim_start_matches(['(', '\'', '"', '`', '['])
        .trim_end_matches([')', '\'', '"', '`', ']', '.', ',', ';', ':', '!', '?'])
        .trim()
}

/// A token worth resolving: has a path separator, or is a bare `filename.ext`.
fn looks_pathy(token: &str) -> bool {
    static BARE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // `\w` is ASCII-only in JavaScript, so it is spelled that way here too.
    let bare = BARE.get_or_init(|| regex::Regex::new(r"^(?-u:[\w.\-])+\.[A-Za-z0-9]{1,8}$").expect("valid bare-filename pattern"));
    if token.is_empty() || token.len() > 400 || token.chars().any(char::is_whitespace) {
        return false;
    }
    token.contains('/') || bare.is_match(token)
}

/// The files `citations` finds, followed by backtick spans and prose paths.
pub fn candidate_paths(text: &str, citations: FileCitations) -> Vec<String> {
    candidate_paths_with_citations(text, &citations(text))
}

fn candidate_paths_with_citations(text: &str, citations: &[FileCitation]) -> Vec<String> {
    static SPAN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static BARE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let span = SPAN.get_or_init(|| regex::Regex::new(r"`([^`\n]{1,240})`").expect("valid code-span pattern"));
    let bare = BARE.get_or_init(|| regex::Regex::new(r"(?-u:(?:~/|\.{1,2}/|/)[\w@./+\-]*[\w@+\-])").expect("valid path pattern"));

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for citation in citations {
        if seen.insert(citation.path.clone()) {
            out.push(citation.path.clone());
        }
    }
    // Never rediscover fragments of a quoted citation as unrelated files.
    let in_citation = |start, end| citations.iter().any(|c| start < c.range.end && end > c.range.start);
    let mut push = |token: &str| {
        let token = clean_token(token);
        if looks_pathy(token) && seen.insert(token.to_string()) {
            out.push(token.to_string());
        }
    };
    for caps in span.captures_iter(text) {
        let found = caps.get(0).expect("whole span");
        if !in_citation(found.start(), found.end()) {
            push(&caps[1]);
        }
    }
    for found in bare.find_iter(text) {
        if !in_citation(found.start(), found.end()) {
            push(found.as_str());
        }
    }
    out
}

/// Lexically resolve `path` against `base`, as Node's `path.resolve` does.
fn resolve_lexically(base: &Path, path: &Path) -> PathBuf {
    let mut resolved = if path.is_absolute() { PathBuf::new() } else { base.to_path_buf() };
    for component in path.components() {
        match component {
            std::path::Component::RootDir => resolved = PathBuf::from("/"),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    resolved
}

/// Resolve a candidate (absolute, `~`, or relative to `cwd`) and keep it inside `$HOME`.
///
/// Shared with the transcript parser, which offers a tool result's image as a
/// thumbnail: the only thing that can serve one is `/api/fs/download`, and that
/// route refuses everything outside the home directory — so a path this returns
/// `None` for must not become a chip that could only ever 403.
pub fn resolve_within_home(home: &Path, candidate: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let raw = if candidate == "~" {
        home.to_path_buf()
    } else if let Some(rest) = candidate.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(candidate)
    };
    let absolute = if raw.is_absolute() {
        resolve_lexically(Path::new("/"), &raw)
    } else {
        resolve_lexically(cwd?, &raw)
    };
    (absolute == home || absolute.starts_with(home)).then_some(absolute)
}

/// Attach download chips for files an assistant message points at, the ones
/// its CLI's markup names (`citations`, from the reader) first.
pub async fn annotate_mentioned_files(home: &Path, items: &mut [TranscriptItem], cwd: Option<&str>, citations_of: FileCitations) {
    let cwd = cwd.map(Path::new);
    let mut exists: HashMap<PathBuf, bool> = HashMap::new();
    let mut stat_budget = MAX_STATS_PER_TRANSCRIPT;

    for item in items.iter_mut() {
        if item.role != Role::Assistant || item.kind != ItemKind::Text || item.text.is_empty() {
            continue;
        }
        let citations = citations_of(&item.text);
        let candidates = candidate_paths_with_citations(&item.text, &citations);
        let mut seen: HashSet<PathBuf> = item.files.iter().flatten().map(|file| PathBuf::from(&file.path)).collect();
        let existing_count = seen.len();
        let mut found: Vec<FileRef> = Vec::new();
        for candidate in candidates {
            if existing_count + found.len() >= MAX_FILES_PER_ITEM {
                break;
            }
            let Some(absolute) = resolve_within_home(home, &candidate, cwd) else {
                continue;
            };
            if !seen.insert(absolute.clone()) {
                continue;
            }
            let is_file = match exists.get(&absolute) {
                Some(known) => *known,
                None => {
                    if stat_budget == 0 {
                        continue;
                    }
                    stat_budget -= 1;
                    let is_file = tokio::fs::metadata(&absolute).await.map(|m| m.is_file()).unwrap_or(false);
                    exists.insert(absolute.clone(), is_file);
                    is_file
                }
            };
            if is_file {
                found.push(FileRef {
                    name: absolute.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    path: absolute.to_string_lossy().into_owned(),
                });
            }
        }
        if !found.is_empty() {
            item.files.get_or_insert_with(Vec::new).extend(found);
        }
        // Only replace a directive once its real file has a chip. Missing,
        // out-of-home and over-budget references remain visible, not erased.
        for citation in citations.into_iter().rev() {
            let Some(absolute) = resolve_within_home(home, &citation.path, cwd) else {
                continue;
            };
            let Some(file) = item.files.iter().flatten().find(|file| Path::new(&file.path) == absolute) else {
                continue;
            };
            let mut label = String::new();
            for c in file.name.chars() {
                if c.is_ascii_punctuation() {
                    label.push('\\');
                }
                label.push(c);
            }
            item.text.replace_range(citation.range, &label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::make_item;

    #[test]
    fn code_spans_and_bare_paths_are_both_candidates() {
        let candidates = candidate_paths("see `src/app.rs` and /etc/hosts, plus ./notes.md (and `not a path`)", no_citations);
        assert!(candidates.contains(&"src/app.rs".to_string()));
        assert!(candidates.contains(&"/etc/hosts".to_string()), "the trailing comma is trimmed");
        assert!(candidates.contains(&"./notes.md".to_string()));
        assert!(!candidates.iter().any(|c| c.contains(' ')), "a token with a space is not a path");
    }

    #[test]
    fn a_bare_filename_needs_an_extension_to_count() {
        assert!(looks_pathy("README.md"));
        assert!(looks_pathy("a/b"));
        assert!(!looks_pathy("README"), "a bare word is prose, not a file");
        assert!(!looks_pathy(&"x".repeat(500)));
    }

    #[test]
    fn nothing_outside_the_home_directory_resolves() {
        let home = Path::new("/home/me");
        assert_eq!(resolve_within_home(home, "~/notes.md", None), Some(PathBuf::from("/home/me/notes.md")));
        assert_eq!(
            resolve_within_home(home, "b.txt", Some(Path::new("/home/me/proj"))),
            Some(PathBuf::from("/home/me/proj/b.txt"))
        );
        assert_eq!(resolve_within_home(home, "/etc/passwd", None), None);
        assert_eq!(
            resolve_within_home(home, "../../etc/passwd", Some(Path::new("/home/me/proj"))),
            None,
            "a traversal out of $HOME is refused after normalisation"
        );
        assert_eq!(resolve_within_home(home, "b.txt", None), None, "a relative path needs a cwd");
    }

    #[tokio::test]
    async fn only_existing_files_under_home_become_chips() {
        let home = tempfile::tempdir().expect("tempdir");
        let real = home.path().join("notes.md");
        std::fs::write(&real, "hi").expect("write");

        let mut items = vec![
            make_item(
                Role::Assistant,
                ItemKind::Text,
                &format!("wrote `{}` and `{}`", real.display(), home.path().join("ghost.md").display()),
                None,
            ),
            make_item(Role::User, ItemKind::Text, &format!("`{}`", real.display()), None),
        ];
        annotate_mentioned_files(home.path(), &mut items, None, no_citations).await;

        let files = items[0].files.as_ref().expect("one chip");
        assert_eq!(files.len(), 1, "the file that does not exist is not offered");
        assert_eq!(files[0].name, "notes.md");
        assert!(items[1].files.is_none(), "only assistant text is annotated");
    }

    #[tokio::test]
    async fn one_message_carries_at_most_six_chips() {
        let home = tempfile::tempdir().expect("tempdir");
        let mut text = String::from("made:");
        for n in 0..10 {
            let path = home.path().join(format!("f{n}.txt"));
            std::fs::write(&path, "x").expect("write");
            text.push_str(&format!(" `{}`", path.display()));
        }
        let mut items = vec![make_item(Role::Assistant, ItemKind::Text, &text, None)];
        annotate_mentioned_files(home.path(), &mut items, None, no_citations).await;
        assert_eq!(items[0].files.as_ref().expect("chips").len(), MAX_FILES_PER_ITEM);
    }
}
