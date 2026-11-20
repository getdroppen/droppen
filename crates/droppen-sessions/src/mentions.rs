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
//!
//! The chips are the filesystem's state when the transcript was parsed, not
//! the transcript's, so they stay out of its fingerprint chain
//! ([`TranscriptItem::as_transcribed`]).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use super::item_log::ItemLog;
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

impl TranscriptItem {
    /// The item as its transcript wrote it, before [`annotate_mentioned_files`]
    /// gave it chips or turned its citations into their labels: what the
    /// fingerprint chain hashes. The item itself unless the annotation changed
    /// it.
    ///
    /// Whether a mentioned file exists is the filesystem's answer at parse
    /// time, and so is which mentions the `stat` budget reached: both move
    /// while the transcript stays as it was. A fingerprint over them moved too,
    /// for every later prefix, and refused each history page and resume
    /// anchored past the item as a changed transcript.
    pub fn as_transcribed(&self) -> &TranscriptItem {
        self.transcribed.as_deref().unwrap_or(self)
    }

    /// Whether [`annotate_mentioned_files`] changed the item, so that it is
    /// served otherwise than it is fingerprinted.
    pub fn is_annotated(&self) -> bool {
        self.transcribed.is_some()
    }

    /// Whether the annotation looks at the item for files: an assistant's
    /// text, and not an empty one.
    pub(crate) fn is_annotatable(&self) -> bool {
        self.role == Role::Assistant && self.kind == ItemKind::Text && !self.text.is_empty()
    }

    /// Keep the item as it is, before the annotation's first change to it.
    fn keep_transcribed(&mut self) {
        if self.transcribed.is_none() {
            self.transcribed = Some(Box::new(self.clone()));
        }
    }
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
///
/// Each item it changes keeps a copy of itself from before, for its
/// fingerprint ([`TranscriptItem::as_transcribed`]).
pub async fn annotate_mentioned_files(home: &Path, items: &mut [TranscriptItem], cwd: Option<&str>, citations_of: FileCitations) {
    let mut mentions = Mentions::new();
    for item in items.iter_mut() {
        if let Some(change) = mentions.look(home, item, cwd, citations_of).await {
            change.apply(item);
        }
    }
}

/// What annotating a transcript's items has found out about the filesystem,
/// kept with a parse so that one going on from it annotates only what is new.
///
/// Whatever it keeps, [`Mentions::annotate`] leaves every item as
/// [`annotate_mentioned_files`] would leave it, walking all of them from the
/// first, now. So the order the [`MAX_STATS_PER_TRANSCRIPT`] budget is spent
/// in, and how a walk goes on, are this type's business alone: a parse only
/// carries it.
#[derive(Debug)]
pub struct Mentions {
    /// Every path looked at, and whether it was a file.
    exists: HashMap<PathBuf, bool>,
    /// How many paths may still be looked at.
    budget: usize,
    /// How many items, from the first, have been annotated.
    upto: usize,
    /// The directory relative paths were resolved against, which a
    /// transcript can name only after an item that mentions one.
    cwd: Option<String>,
}

impl Default for Mentions {
    fn default() -> Self {
        Self::new()
    }
}

impl Mentions {
    /// Nothing looked at: a walk from the first item.
    pub fn new() -> Self {
        Self {
            exists: HashMap::new(),
            budget: MAX_STATS_PER_TRANSCRIPT,
            upto: 0,
            cwd: None,
        }
    }

    /// Annotate the items of `log` that are not yet, so that every item is
    /// as a walk of all of them from the first, now, would leave it.
    ///
    /// The budget is spent in item order, so an item's chips depend on what
    /// the paths looked at before it turned out to be. While every one of
    /// them is still what it was, and relative paths resolve against the
    /// same `cwd`, the items annotated already are exactly as such a walk
    /// leaves them, and it goes on from the first one not yet annotated.
    /// Otherwise (a file a message named was written since, or deleted, or
    /// the transcript has only now said where it runs) the walk starts over
    /// from the items as written. Either way the chain does not move: the
    /// chips are not the transcript's.
    ///
    /// A parser never changes an annotatable item after making it
    /// ([`ItemLog::get_mut`]), so the items annotated already are the ones
    /// below where the last walk stopped. One that took items out or put
    /// some in below there ([`ItemLog::moved_from`]) moved what the walk saw:
    /// while none of those it took out or moved along could be chipped, the
    /// walk saw nothing of them, and goes on from the lowest one moved; when
    /// one could, it starts over.
    pub async fn annotate(&mut self, home: &Path, log: &mut ItemLog, cwd: Option<&str>, citations_of: FileCitations) {
        let mut over = false;
        if log.moved_from() < self.upto {
            match log.moved_annotatable() {
                true => over = true,
                false => self.upto = log.moved_from(),
            }
        }
        if over || self.upto > log.len() || self.cwd.as_deref() != cwd || self.answers_moved().await {
            for index in 0..log.len() {
                if let Some(written) = log.get(index).filter(|item| item.is_annotated()).map(|item| item.as_transcribed().clone()) {
                    *log.annotate_mut(index) = written;
                }
            }
            *self = Self::new();
            self.cwd = cwd.map(str::to_owned);
        }
        for index in self.upto..log.len() {
            let Some(item) = log.get(index) else { break };
            if let Some(change) = self.look(home, item, cwd, citations_of).await {
                change.apply(log.annotate_mut(index));
            }
        }
        self.upto = log.len();
    }

    /// Whether any path looked at is not what it was then: one `stat` each,
    /// all on one blocking thread.
    async fn answers_moved(&self) -> bool {
        if self.exists.is_empty() {
            return false;
        }
        let looked: Vec<(PathBuf, bool)> = self.exists.iter().map(|(path, was)| (path.clone(), *was)).collect();
        tokio::task::spawn_blocking(move || {
            looked
                .iter()
                .any(|(path, was)| std::fs::metadata(path).map(|meta| meta.is_file()).unwrap_or(false) != *was)
        })
        .await
        .unwrap_or(true)
    }

    /// The chips `item` gets, and the citations their labels replace, when it
    /// gets any. The paths it looks at are spent from the budget.
    async fn look(&mut self, home: &Path, item: &TranscriptItem, cwd: Option<&str>, citations_of: FileCitations) -> Option<Change> {
        if !item.is_annotatable() {
            return None;
        }
        let cwd = cwd.map(Path::new);
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
            let is_file = match self.exists.get(&absolute) {
                Some(known) => *known,
                None => {
                    if self.budget == 0 {
                        continue;
                    }
                    self.budget -= 1;
                    let is_file = tokio::fs::metadata(&absolute).await.map(|m| m.is_file()).unwrap_or(false);
                    self.exists.insert(absolute.clone(), is_file);
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
        // Only replace a directive once its real file has a chip. Missing,
        // out-of-home and over-budget references remain visible, not erased.
        let mut labels = Vec::new();
        for citation in citations.into_iter().rev() {
            let Some(absolute) = resolve_within_home(home, &citation.path, cwd) else {
                continue;
            };
            let Some(file) = item.files.iter().flatten().chain(&found).find(|file| Path::new(&file.path) == absolute) else {
                continue;
            };
            let mut label = String::new();
            for c in file.name.chars() {
                if c.is_ascii_punctuation() {
                    label.push('\\');
                }
                label.push(c);
            }
            labels.push((citation.range, label));
        }
        (!found.is_empty() || !labels.is_empty()).then_some(Change { found, labels })
    }
}

/// What the annotation does to one item: the chips it adds, and the citations
/// their labels replace, the last first.
struct Change {
    found: Vec<FileRef>,
    labels: Vec<(Range<usize>, String)>,
}

impl Change {
    fn apply(self, item: &mut TranscriptItem) {
        if !self.found.is_empty() {
            item.keep_transcribed();
            item.files.get_or_insert_with(Vec::new).extend(self.found);
        }
        for (range, label) in self.labels {
            item.keep_transcribed();
            item.text.replace_range(range, &label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::fingerprint_chain;
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

    /// A transcript's items as its reader makes them, before the annotation:
    /// a user turn, an assistant message naming `named`, and a reply.
    fn naming(named: &Path) -> Vec<TranscriptItem> {
        vec![
            make_item(Role::User, ItemKind::Text, "write the report", None),
            make_item(Role::Assistant, ItemKind::Text, &format!("wrote `{}`", named.display()), None),
            make_item(Role::Assistant, ItemKind::Text, "anything else?", None),
        ]
    }

    async fn annotated(home: &Path, mut items: Vec<TranscriptItem>, cwd: Option<&str>) -> Vec<TranscriptItem> {
        annotate_mentioned_files(home, &mut items, cwd, no_citations).await;
        items
    }

    /// A file created or deleted between two parses of the same transcript
    /// changes its chips, and not one fingerprint: the chain names what the
    /// transcript wrote, which is also what it was before chips existed.
    #[tokio::test]
    async fn a_file_created_or_deleted_between_parses_moves_the_chips_not_the_chain() {
        let home = tempfile::tempdir().expect("tempdir");
        let report = home.path().join("report.md");
        let file = Path::new("/t/s.jsonl");
        let chain = fingerprint_chain(file, &naming(&report));

        let absent = annotated(home.path(), naming(&report), None).await;
        assert!(absent[1].files.is_none());
        assert_eq!(fingerprint_chain(file, &absent), chain);

        std::fs::write(&report, "hi").expect("write");
        let present = annotated(home.path(), naming(&report), None).await;
        let chips: Vec<&str> = present[1].files.iter().flatten().map(|file| file.name.as_str()).collect();
        assert_eq!(chips, ["report.md"], "the chip follows the file");
        assert_eq!(fingerprint_chain(file, &present), chain);
        let again = annotated(home.path(), absent.clone(), None).await;
        assert!(again[1].files.is_some());
        assert_eq!(fingerprint_chain(file, &again), chain, "a second annotation is left out too");

        std::fs::remove_file(&report).expect("remove");
        let deleted = annotated(home.path(), naming(&report), None).await;
        assert!(deleted[1].files.is_none(), "and goes with it");
        assert_eq!(fingerprint_chain(file, &deleted), chain);
    }

    /// The `stat` budget is spent in order, so which later mentions it reaches
    /// depends on the files an earlier message names: here six of them
    /// existing gives a later message its chip. The chain does not move.
    #[tokio::test]
    async fn the_stat_budget_spent_differently_moves_no_fingerprint() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(home.path().join("report.md"), "hi").expect("write");
        let drafts: Vec<String> = (0..MAX_STATS_PER_TRANSCRIPT).map(|n| format!("`d{n}.md`")).collect();
        let items = || {
            vec![
                make_item(Role::Assistant, ItemKind::Text, &format!("drafts: {}", drafts.join(" ")), None),
                make_item(Role::Assistant, ItemKind::Text, "wrote `report.md`", None),
            ]
        };
        let cwd = home.path().to_str();
        let file = Path::new("/t/s.jsonl");

        let spent = annotated(home.path(), items(), cwd).await;
        assert!(spent[0].files.is_none());
        assert!(spent[1].files.is_none(), "every stat went on the drafts");

        for n in 0..MAX_FILES_PER_ITEM {
            std::fs::write(home.path().join(format!("d{n}.md")), "x").expect("write");
        }
        let left = annotated(home.path(), items(), cwd).await;
        assert_eq!(left[0].files.as_ref().map(Vec::len), Some(MAX_FILES_PER_ITEM));
        assert_eq!(left[1].files.as_ref().map(Vec::len), Some(1), "six stats found six drafts; the report got one too");

        assert_eq!(fingerprint_chain(file, &left), fingerprint_chain(file, &spent));
        assert_eq!(fingerprint_chain(file, &left), fingerprint_chain(file, &items()));
    }

    /// Annotating a transcript's items a few at a time, as a parse that goes
    /// on from an earlier one does, leaves every item as one walk of all of
    /// them leaves it, a file written in between included.
    #[tokio::test]
    async fn annotating_on_from_where_the_last_walk_stopped_is_annotating_all_of_it() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(home.path().join("a.md"), "a").expect("write");
        let texts = ["`~/a.md` and `~/b.md`", "nothing", "`~/b.md` again", "`~/a.md`"];
        let mut log = ItemLog::new();
        let mut mentions = Mentions::new();
        for (n, text) in texts.iter().enumerate() {
            log.push(make_item(Role::Assistant, ItemKind::Text, text, None));
            if n == 2 {
                std::fs::write(home.path().join("b.md"), "b").expect("write");
            }
            mentions.annotate(home.path(), &mut log, None, no_citations).await;
            let mut whole: Vec<TranscriptItem> = log.items().iter().map(|item| item.as_transcribed().clone()).collect();
            annotate_mentioned_files(home.path(), &mut whole, None, no_citations).await;
            let served: Vec<&TranscriptItem> = log.items().iter().map(|item| &**item).collect();
            assert_eq!(serde_json::to_value(served).expect("json"), serde_json::to_value(&whole).expect("json"), "after item {n}");
        }
        assert_eq!(log.items()[0].files.as_ref().map(Vec::len), Some(2), "the file written later has its chip");
    }

    /// Items taken out or put in below where the last walk stopped: a tool
    /// call's row taken out and a message written in its place is walked
    /// from there; a message moved along by a row put in ahead of it has the
    /// walk start over. Either way, as one walk of all of them leaves them.
    #[tokio::test]
    async fn items_moved_below_the_last_walk_are_walked_as_a_whole_walk_would() {
        let home = tempfile::tempdir().expect("tempdir");
        for file in ["a.md", "b.md"] {
            std::fs::write(home.path().join(file), "x").expect("write");
        }
        let whole = |log: &ItemLog| {
            let items: Vec<TranscriptItem> = log.items().iter().map(|item| item.as_transcribed().clone()).collect();
            let home = home.path().to_path_buf();
            async move {
                let mut items = items;
                annotate_mentioned_files(&home, &mut items, None, no_citations).await;
                serde_json::to_value(items).expect("json")
            }
        };
        let served = |log: &ItemLog| serde_json::to_value(log.items().iter().map(|item| &**item).collect::<Vec<_>>()).expect("json");
        let said = |text: &str| make_item(Role::Assistant, ItemKind::Text, text, None);
        let tool = |text: &str| make_item(Role::Assistant, ItemKind::ToolUse, text, None);

        let mut mentions = Mentions::new();
        let mut log = ItemLog::new();
        log.push(said("`~/a.md`"));
        log.push(tool("cell"));
        mentions.annotate(home.path(), &mut log, None, no_citations).await;
        let (items, _) = log.into_parts();
        let mut log = ItemLog::resume(items);
        log.pop();
        log.push(said("`~/b.md`"));
        mentions.annotate(home.path(), &mut log, None, no_citations).await;
        assert_eq!(served(&log), whole(&log).await, "the row's place walked");
        assert_eq!(log.items()[1].files.as_ref().map(Vec::len), Some(1));

        let (items, _) = log.into_parts();
        let mut log = ItemLog::resume(items);
        log.insert(0, [said("`~/b.md` first")]);
        mentions.annotate(home.path(), &mut log, None, no_citations).await;
        assert_eq!(served(&log), whole(&log).await, "started over");
    }

    /// Where starting over matters: a message that spent the whole budget is
    /// moved along by one put in ahead of it, which a walk from the first
    /// item looks at first, with the budget whole. Walking on from where it
    /// was put in, the budget would be spent already.
    #[tokio::test]
    async fn a_message_put_in_ahead_of_the_one_that_spent_the_budget_is_walked_with_it_whole() {
        let home = tempfile::tempdir().expect("tempdir");
        std::fs::write(home.path().join("notes.md"), "x").expect("write");
        let spender: Vec<String> = (0..MAX_STATS_PER_TRANSCRIPT + 100).map(|n| format!("`~/gone{n}.md`")).collect();
        let mut mentions = Mentions::new();
        let mut log = ItemLog::new();
        log.push(make_item(Role::Assistant, ItemKind::Text, &spender.join(" "), None));
        mentions.annotate(home.path(), &mut log, None, no_citations).await;

        let (items, _) = log.into_parts();
        let mut log = ItemLog::resume(items);
        log.insert(0, [make_item(Role::Assistant, ItemKind::Text, "see `~/notes.md`", None)]);
        mentions.annotate(home.path(), &mut log, None, no_citations).await;
        let mut whole: Vec<TranscriptItem> = log.items().iter().map(|item| item.as_transcribed().clone()).collect();
        annotate_mentioned_files(home.path(), &mut whole, None, no_citations).await;
        assert_eq!(whole[0].files.as_ref().map(Vec::len), Some(1), "a walk from the first item chips it");
        assert_eq!(log.items()[0].files, whole[0].files, "and so does this one");
    }

    /// A file the item already named keeps counting, as the transcript's own:
    /// only the chip added beside it is left out.
    #[tokio::test]
    async fn a_file_the_item_already_named_stays_in_its_fingerprint() {
        let home = tempfile::tempdir().expect("tempdir");
        let report = home.path().join("report.md");
        std::fs::write(&report, "hi").expect("write");
        let named = |path: &str| {
            let mut items = naming(&report);
            items[1].files = Some(vec![FileRef {
                name: "draft.md".into(),
                path: path.into(),
            }]);
            items
        };
        let file = Path::new("/t/s.jsonl");

        let chipped = annotated(home.path(), named("/u/draft.md"), None).await;
        let files: Vec<&str> = chipped[1].files.iter().flatten().map(|file| file.name.as_str()).collect();
        assert_eq!(files, ["draft.md", "report.md"]);
        let kept: Vec<&str> = chipped[1].as_transcribed().files.iter().flatten().map(|file| file.name.as_str()).collect();
        assert_eq!(kept, ["draft.md"]);
        assert_eq!(fingerprint_chain(file, &chipped), fingerprint_chain(file, &named("/u/draft.md")));
        assert_ne!(
            fingerprint_chain(file, &chipped),
            fingerprint_chain(file, &named("/u/other.md")),
            "the file it named is the transcript's"
        );
    }
}
