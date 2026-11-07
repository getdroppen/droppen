//! Searching one session's whole transcript.
//!
//! The page only ever holds a window of a transcript — the newest page plus
//! whatever the reader paged in — so the browser's own find cannot see most of
//! a long session. This answers "which items say X" over every item the parse
//! holds, numbered exactly as the feed numbers them (`startIndex`/`endIndex`,
//! `GET …/history`), so a client can page in to any of them with the paging it
//! already has.
//!
//! # Matching
//!
//! Case-insensitive substring, per item. The query is split on whitespace and
//! every term has to occur somewhere in the same item (AND) — not necessarily
//! in the same field. There is no tokenizer, so CJK text, paths and
//! identifiers match the way they read.
//!
//! Case is folded one character at a time ([`fold`]): `char::to_lowercase`,
//! with the final sigma folded to `σ`. Not `str::to_lowercase`, whose final
//! sigma depends on the neighbouring letters — a term typed on its own would
//! then fail to match the same letters inside a word. The web client folds the
//! same way for its highlights, so the two agree on what a hit is.
//!
//! # Cost
//!
//! The items come out of the feed's parse cache, so a search is a scan and
//! never a re-read of the file. The scan runs from the newest item backwards:
//! the first `limit` matches it meets get a snippet, every match after that is
//! only counted, and the count stops at [`TOTAL_CAP`]. A snippet is built from
//! the one field it shows and never from the rest of the item, so a match's
//! cost does not grow with how much output a tool call had.

use std::borrow::Cow;

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use droppen_http::openapi_ext::Nullable;
use droppen_sessions::util::{ItemKind, Role, TranscriptItem};

/// How many matches a search returns when the client does not say.
pub const DEFAULT_LIMIT: usize = 200;
/// The most matches one search returns.
pub const MAX_LIMIT: usize = 1000;
/// Past this many matching items the count stops and `truncated` is set.
pub const TOTAL_CAP: usize = 10_000;
/// The longest query accepted, in characters.
pub const MAX_QUERY_CHARS: usize = 256;
/// The most distinct terms one query may hold.
pub const MAX_TERMS: usize = 16;
/// The longest snippet, in characters, ellipses included.
pub const SNIPPET_CHARS: usize = 160;
/// How much of the text before the first hit a snippet keeps, in characters.
const LEAD_CHARS: usize = 48;
/// Put between a tool's name and its summary in a `tool` snippet.
const TOOL_SEPARATOR: &str = " · ";

/// Which part of an item a match's snippet was cut from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum SearchField {
    /// The message itself: what the user, the agent or remote-hand wrote, a
    /// thinking block, a slash command and its arguments.
    Text,
    /// A tool call's name and its one-line summary (and caption).
    Tool,
    /// An expanded body: a tool call's full input, a tool's output, a compact
    /// summary, a background task's report.
    Detail,
}

/// One item of the transcript that holds every term of the query.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    /// Absolute index of the item, in the numbering of the session feed's
    /// `startIndex`/`endIndex` and of `GET …/history`.
    pub index: usize,
    /// Who produced the item.
    pub role: Role,
    /// What kind of content it carries.
    pub kind: ItemKind,
    /// Which part of the item `snippet` was cut from: the one holding the most
    /// distinct terms, `text` before `tool` before `detail` on a tie.
    pub field: SearchField,
    /// At most 160 characters of that field around its first hit, whitespace
    /// runs collapsed to one space, with `…` where it was cut.
    pub snippet: String,
    /// Every hit inside `snippet` as `[start, end)` offsets in UTF-16 code
    /// units — so `snippet.slice(start, end)` in JavaScript is the hit — sorted
    /// and with overlapping hits merged.
    pub ranges: Vec<Vec<u32>>,
    /// The item's own timestamp, when the transcript has one.
    pub timestamp: Nullable<String>,
}

/// The answer to `GET /api/sessions/{source}/{id}/search`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchResult {
    /// The query as it was searched, trimmed.
    pub query: String,
    /// How many items match. Exact up to 10 000; past that the count stops
    /// there and `truncated` is set.
    pub total: usize,
    /// More items match than `total` counts: the scan stopped at the cap.
    pub truncated: bool,
    /// The newest `limit` matching items, in ascending `index` order. Shorter
    /// than `total` when there were more.
    pub matches: Vec<SearchMatch>,
}

/// Fold one character for matching, handing each folded character to `out`.
fn fold_char(c: char, out: &mut impl FnMut(char)) {
    if c.is_ascii() {
        out(c.to_ascii_lowercase());
        return;
    }
    for lower in c.to_lowercase() {
        out(if lower == 'ς' { 'σ' } else { lower });
    }
}

/// A string folded for matching; see the module docs. Borrowed when there is
/// nothing to fold, which is most ASCII.
pub fn fold(text: &str) -> Cow<'_, str> {
    if text.is_ascii() {
        if text.bytes().any(|b| b.is_ascii_uppercase()) {
            return Cow::Owned(text.to_ascii_lowercase());
        }
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        fold_char(c, &mut |lower| out.push(lower));
    }
    Cow::Owned(out)
}

/// The distinct folded terms of a query, in the order they were typed.
pub fn terms_of(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in query.split_whitespace() {
        let folded = fold(word).into_owned();
        if !terms.contains(&folded) {
            terms.push(folded);
        }
    }
    terms
}

/// Why a query cannot be searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// Nothing but whitespace.
    Empty,
    /// Longer than [`MAX_QUERY_CHARS`].
    TooLong,
    /// More than [`MAX_TERMS`] distinct terms.
    TooManyTerms,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "the query is empty"),
            Self::TooLong => write!(f, "the query is longer than {MAX_QUERY_CHARS} characters"),
            Self::TooManyTerms => write!(f, "the query has more than {MAX_TERMS} terms"),
        }
    }
}

/// The terms of a query a client sent, or why it cannot be searched.
pub fn validate_query(query: &str) -> Result<Vec<String>, QueryError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(QueryError::Empty);
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(QueryError::TooLong);
    }
    let terms = terms_of(query);
    if terms.len() > MAX_TERMS {
        return Err(QueryError::TooManyTerms);
    }
    Ok(terms)
}

/// `name · text`, as a `tool` snippet shows a call.
fn named<'a>(name: Option<&str>, text: &'a str) -> Cow<'a, str> {
    match name {
        Some(name) if !text.trim().is_empty() => Cow::Owned(format!("{name}{TOOL_SEPARATOR}{text}")),
        Some(name) => Cow::Owned(name.to_string()),
        None => Cow::Borrowed(text),
    }
}

/// The texts of an item a search looks at, each with the field it counts as,
/// in tie-break order. Blank ones are left out.
fn haystacks(item: &TranscriptItem) -> Vec<(SearchField, Cow<'_, str>)> {
    let name = item.tool_name.as_ref().map(String::as_str).filter(|name| !name.trim().is_empty());
    let detail = item.detail.as_ref().map(|detail| Cow::Borrowed(detail.as_str()));
    let meta = item.meta.as_ref().map(|meta| Cow::Borrowed(meta.as_str()));
    let body = Cow::Borrowed(item.text.as_str());
    let candidates: [Option<(SearchField, Cow<'_, str>)>; 3] = match item.kind {
        ItemKind::ToolUse => [
            Some((SearchField::Tool, named(name, &item.text))),
            meta.map(|meta| (SearchField::Tool, meta)),
            detail.map(|detail| (SearchField::Detail, detail)),
        ],
        // Its `toolName` is the call's, inherited: searching it would match
        // every result of every call to a tool whose name was typed.
        ItemKind::ToolResult => [
            Some((SearchField::Detail, body)),
            detail.map(|detail| (SearchField::Detail, detail)),
            meta.map(|meta| (SearchField::Detail, meta)),
        ],
        ItemKind::Command => [
            Some((SearchField::Text, named(name, &item.text))),
            detail.map(|detail| (SearchField::Detail, detail)),
            None,
        ],
        ItemKind::Text | ItemKind::Thinking | ItemKind::Compact => {
            [Some((SearchField::Text, body)), detail.map(|detail| (SearchField::Detail, detail)), None]
        }
    };
    candidates.into_iter().flatten().filter(|(_, text)| !text.trim().is_empty()).collect()
}

/// Sort `[start, end)` ranges and merge the ones that overlap or touch.
fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.retain(|(start, end)| start < end);
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// A snippet of `text` around its first hit, and the hits inside it in UTF-16
/// offsets; see [`SearchMatch::snippet`] and [`SearchMatch::ranges`].
pub fn snippet_of(text: &str, terms: &[String]) -> (String, Vec<Vec<u32>>) {
    // Whitespace runs collapse first, so the snippet is one line and every
    // offset below is into what is shown. A term holds no whitespace, so this
    // cannot make or break a hit.
    let mut norm: Vec<char> = Vec::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_whitespace() {
            gap = !norm.is_empty();
            continue;
        }
        if gap {
            norm.push(' ');
            gap = false;
        }
        norm.push(c);
    }

    // Folded, with the character of `norm` every folded byte came from.
    let mut folded = String::with_capacity(norm.len());
    let mut owner: Vec<usize> = Vec::with_capacity(norm.len());
    for (at, &c) in norm.iter().enumerate() {
        fold_char(c, &mut |lower| {
            folded.push(lower);
            owner.resize(folded.len(), at);
        });
    }

    let mut hits: Vec<(usize, usize)> = Vec::new();
    for term in terms.iter().filter(|term| !term.is_empty()) {
        for (byte, found) in folded.match_indices(term.as_str()) {
            hits.push((owner[byte], owner[byte + found.len() - 1] + 1));
        }
    }
    let hits = merge_ranges(hits);

    let len = norm.len();
    let first = hits.first().map_or(0, |hit| hit.0);
    let (start, end) = if len <= SNIPPET_CHARS {
        (0, len)
    } else {
        let start = first.saturating_sub(LEAD_CHARS);
        let room = SNIPPET_CHARS - usize::from(start > 0);
        if start + room < len {
            // Cut at both ends, or at the end only: one ellipsis per cut.
            (start, start + room - 1)
        } else {
            // The end of the text is in reach, so fill the budget backwards.
            // `len > SNIPPET_CHARS` keeps the start above zero, so there is a
            // leading ellipsis and nothing else.
            (len - (SNIPPET_CHARS - 1), len)
        }
    };
    let lead = start > 0;
    let tail = end < len;

    let mut snippet = String::new();
    if lead {
        snippet.push('…');
    }
    // UTF-16 offset of every window position, the end included.
    let base = usize::from(lead);
    let mut utf16 = Vec::with_capacity(end - start + 1);
    let mut at = base;
    for &c in &norm[start..end] {
        utf16.push(at);
        at += c.len_utf16();
        snippet.push(c);
    }
    utf16.push(at);
    if tail {
        snippet.push('…');
    }

    let ranges = hits
        .into_iter()
        .filter(|&(hit_start, hit_end)| hit_start < end && hit_end > start)
        .map(|(hit_start, hit_end)| {
            let from = utf16[hit_start.max(start) - start];
            let to = utf16[hit_end.min(end) - start];
            vec![from as u32, to as u32]
        })
        .collect();
    (snippet, ranges)
}

/// Search `items` for every item holding all of `terms`; see the module docs.
///
/// `terms` are folded already ([`terms_of`]); none at all matches nothing.
pub fn search_items(items: &[TranscriptItem], query: &str, terms: &[String], limit: usize, cap: usize) -> SessionSearchResult {
    let mut total = 0;
    let mut truncated = false;
    let mut matches = Vec::new();
    if !terms.is_empty() {
        for (index, item) in items.iter().enumerate().rev() {
            let stacks = haystacks(item);
            let folded: Vec<Cow<'_, str>> = stacks.iter().map(|(_, text)| fold(text)).collect();
            if !terms.iter().all(|term| folded.iter().any(|text| text.contains(term.as_str()))) {
                continue;
            }
            if total == cap {
                truncated = true;
                break;
            }
            total += 1;
            if matches.len() >= limit {
                continue;
            }
            // The field that holds the most of the query; `text` before
            // `tool` before `detail` on a tie, which is the order they were
            // pushed in.
            let best = (0..stacks.len())
                .max_by_key(|&at| {
                    let held = terms.iter().filter(|term| folded[at].contains(term.as_str())).count();
                    (held, std::cmp::Reverse(at))
                })
                .unwrap_or(0);
            let (field, text) = &stacks[best];
            let (snippet, ranges) = snippet_of(text, terms);
            matches.push(SearchMatch {
                index,
                role: item.role,
                kind: item.kind,
                field: *field,
                snippet,
                ranges,
                timestamp: Nullable(item.timestamp.0.clone()),
            });
        }
    }
    matches.reverse();
    SessionSearchResult {
        query: query.trim().to_string(),
        total,
        truncated,
        matches,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use droppen_sessions::util::{make_item, make_tool_item, ToolDisplay};

    fn text(role: Role, body: &str) -> TranscriptItem {
        make_item(role, ItemKind::Text, body, Some("2026-10-01T10:00:00.000Z"))
    }

    fn tool_use(name: &str, summary: &str, detail: Option<&str>) -> TranscriptItem {
        make_tool_item(
            Role::Assistant,
            ItemKind::ToolUse,
            ToolDisplay {
                tool_name: Some(name.to_string()),
                text: summary.to_string(),
                detail: detail.map(str::to_string),
                ..ToolDisplay::default()
            },
            None,
        )
    }

    fn tool_result(name: &str, summary: &str, detail: &str) -> TranscriptItem {
        make_tool_item(
            Role::User,
            ItemKind::ToolResult,
            ToolDisplay {
                tool_name: Some(name.to_string()),
                text: summary.to_string(),
                detail: Some(detail.to_string()),
                ..ToolDisplay::default()
            },
            None,
        )
    }

    fn search(items: &[TranscriptItem], query: &str) -> SessionSearchResult {
        search_items(items, query, &terms_of(query), DEFAULT_LIMIT, TOTAL_CAP)
    }

    fn indices(result: &SessionSearchResult) -> Vec<usize> {
        result.matches.iter().map(|m| m.index).collect()
    }

    /// The hit a range names, sliced the way the web client slices it.
    fn utf16_slice(snippet: &str, range: &[u32]) -> String {
        let units: Vec<u16> = snippet.encode_utf16().collect();
        String::from_utf16(&units[range[0] as usize..range[1] as usize]).expect("a range never splits a character")
    }

    #[test]
    fn every_term_has_to_be_in_the_same_item_but_not_the_same_field() {
        let items = vec![
            text(Role::User, "please fix the parser"),
            text(Role::Assistant, "the parser is fixed"),
            tool_use("Bash", "$ cargo test parser", Some("{\"command\":\"cargo test parser\"}")),
            tool_result("Bash", "test result: ok", "running 3 tests\nparser::works ... ok"),
            text(Role::Assistant, "all green"),
        ];
        assert_eq!(indices(&search(&items, "parser")), vec![0, 1, 2, 3]);
        assert_eq!(indices(&search(&items, "parser fixed")), vec![1], "AND, not OR");
        assert_eq!(indices(&search(&items, "  fixed   parser ")), vec![1], "order and spacing do not matter");
        // `cargo` is in the tool field and `ok` only in the detail of the
        // result, so neither item alone holds both terms…
        assert!(search(&items, "cargo ok").matches.is_empty());
        // …but the result holds `result` in its summary and `works` in its
        // output, and that is one item.
        assert_eq!(indices(&search(&items, "works result")), vec![3]);
        assert!(search(&items, "nowhere").matches.is_empty());
    }

    #[test]
    fn matching_ignores_case_both_ways() {
        let items = vec![text(Role::User, "Run The TESTS"), text(Role::Assistant, "ΟΔΟΣ, οδος and Straße")];
        assert_eq!(indices(&search(&items, "tests")), vec![0]);
        assert_eq!(indices(&search(&items, "rUn THE")), vec![0]);
        assert_eq!(indices(&search(&items, "οδοσ")), vec![1], "a final sigma folds like any other");
        assert_eq!(indices(&search(&items, "ΟΔΟΣ")), vec![1]);
        let both = &search(&items, "οδος").matches[0];
        assert_eq!(both.ranges.len(), 2, "upper case and the written final sigma alike: {:?}", both.ranges);
        assert_eq!(indices(&search(&items, "STRASSE")), Vec::<usize>::new(), "folding is not expansion");
        assert_eq!(indices(&search(&items, "STRAßE")), vec![1]);
        let hit = &search(&items, "tests").matches[0];
        assert_eq!(utf16_slice(&hit.snippet, &hit.ranges[0]), "TESTS", "the hit is the original text");
    }

    #[test]
    fn cjk_matches_as_a_plain_substring() {
        let items = vec![
            text(Role::User, "请帮我修复登录页面的样式问题"),
            text(Role::Assistant, "我已经修改了登录页面，现在样式正常了"),
            text(Role::User, "ログイン画面を直してください"),
        ];
        assert_eq!(indices(&search(&items, "登录页面")), vec![0, 1]);
        assert_eq!(indices(&search(&items, "登录 正常")), vec![1]);
        assert_eq!(indices(&search(&items, "画面")), vec![2]);
        let hit = &search(&items, "页面").matches[0];
        assert_eq!(hit.snippet, "请帮我修复登录页面的样式问题");
        assert_eq!(hit.ranges, vec![vec![7, 9]]);
    }

    #[test]
    fn a_match_names_the_field_its_snippet_came_from() {
        let items = vec![
            text(Role::User, "deploy the server"),
            tool_use(
                "Bash",
                "$ ./deploy.sh staging",
                Some("{\"command\":\"./deploy.sh staging\",\"timeout\":600000}"),
            ),
            tool_result("Bash", "done", "uploading artifacts\nrestarting the service on staging"),
            tool_use("Read", "src/main.rs", Some("{\"file_path\":\"/w/src/main.rs\",\"limit\":40}")),
            make_item(Role::Assistant, ItemKind::Thinking, "maybe the staging box is down", None),
        ];
        let result = search(&items, "staging");
        let fields: Vec<(usize, SearchField)> = result.matches.iter().map(|m| (m.index, m.field)).collect();
        assert_eq!(
            fields,
            vec![(1, SearchField::Tool), (2, SearchField::Detail), (4, SearchField::Text)],
            "the summary beats the detail on a tie; output is detail; thinking is text"
        );
        assert_eq!(
            result.matches[0].snippet, "Bash · $ ./deploy.sh staging",
            "the tool's name leads its summary"
        );
        assert_eq!(result.matches[0].kind, ItemKind::ToolUse);
        assert_eq!(result.matches[1].role, Role::User);
        assert_eq!(result.matches[1].kind, ItemKind::ToolResult);

        // Only the input knows about `limit`, so the detail is the snippet.
        let input = search(&items, "limit");
        assert_eq!(input.matches.len(), 1);
        assert_eq!((input.matches[0].index, input.matches[0].field), (3, SearchField::Detail));
        // The tool's name is searched on the call…
        assert_eq!(indices(&search(&items, "read")), vec![3]);
        // …and not on its result, which only inherits it.
        assert_eq!(indices(&search(&items, "bash")), vec![1]);
        // The field holding more of the query wins over an earlier one.
        let both = search(&items, "restarting staging");
        assert_eq!((both.matches[0].index, both.matches[0].field), (2, SearchField::Detail));
    }

    #[test]
    fn a_snippet_is_cut_around_the_first_hit_and_says_where_it_was_cut() {
        let before = "a".repeat(300);
        let after = "b".repeat(300);
        let items = vec![text(Role::Assistant, &format!("{before} needle {after}"))];
        let hit = &search(&items, "needle").matches[0];
        assert_eq!(hit.snippet.chars().count(), SNIPPET_CHARS);
        assert!(hit.snippet.starts_with('…') && hit.snippet.ends_with('…'), "{}", hit.snippet);
        assert_eq!(hit.ranges.len(), 1);
        assert_eq!(utf16_slice(&hit.snippet, &hit.ranges[0]), "needle");
        assert_eq!(hit.ranges[0][0] as usize, 1 + LEAD_CHARS, "the lead it promises, after the ellipsis");

        // Near the start: no leading ellipsis, and the budget runs on.
        let items = vec![text(Role::Assistant, &format!("needle {after}"))];
        let hit = &search(&items, "needle").matches[0];
        assert!(!hit.snippet.starts_with('…') && hit.snippet.ends_with('…'));
        assert_eq!(hit.snippet.chars().count(), SNIPPET_CHARS);
        assert_eq!(hit.ranges, vec![vec![0, 6]]);

        // Near the end: the window slides back to fill the budget.
        let items = vec![text(Role::Assistant, &format!("{before} needle"))];
        let hit = &search(&items, "needle").matches[0];
        assert!(hit.snippet.starts_with('…') && !hit.snippet.ends_with('…'));
        assert_eq!(hit.snippet.chars().count(), SNIPPET_CHARS);
        assert!(hit.snippet.ends_with(" needle"));

        // Short text: whole, whitespace runs collapsed into one space.
        let items = vec![text(Role::User, "  look\n\n\tat   this  needle\n")];
        let hit = &search(&items, "needle look").matches[0];
        assert_eq!(hit.snippet, "look at this needle");
        assert_eq!(hit.ranges, vec![vec![0, 4], vec![13, 19]]);
    }

    #[test]
    fn ranges_count_utf16_units_through_multibyte_text_and_emoji() {
        let items = vec![text(Role::User, "🎉 发布 v2 🚀 réussi: déploiement OK 🎉")];
        let result = search(&items, "déploiement 发布 ok");
        let hit = &result.matches[0];
        let hits: Vec<String> = hit.ranges.iter().map(|range| utf16_slice(&hit.snippet, range)).collect();
        assert_eq!(hits, vec!["发布", "déploiement", "OK"]);
        // 🎉 is two UTF-16 units, so 发布 starts at 3, not at 2 (chars) or 5 (bytes).
        assert_eq!(hit.ranges[0], vec![3, 5]);

        // A long text cut with an emoji inside the window and before the hit.
        let long = format!("{}🚀🚀 target {}", "x".repeat(200), "y".repeat(200));
        let hit = &search(&[text(Role::User, &long)], "TARGET").matches[0];
        assert_eq!(utf16_slice(&hit.snippet, &hit.ranges[0]), "target");
        assert_eq!(
            hit.ranges[0][0] as usize,
            1 + (LEAD_CHARS - 3) + 4 + 1,
            "ellipsis, x's, two 2-unit emoji, space"
        );
    }

    #[test]
    fn overlapping_hits_of_different_terms_are_one_range() {
        let items = vec![text(Role::User, "abcdef and cd")];
        let hit = &search(&items, "abc bcd").matches[0];
        assert_eq!(hit.ranges, vec![vec![0, 4]], "abc and bcd overlap into abcd");
        let hit = &search(&items, "ab cd").matches[0];
        assert_eq!(hit.ranges, vec![vec![0, 4], vec![11, 13]], "touching hits merge too");
    }

    #[test]
    fn the_newest_matches_are_returned_in_order_and_the_rest_only_counted() {
        let items: Vec<TranscriptItem> = (0..50)
            .map(|n| text(Role::Assistant, &format!("step {n}: {}", if n % 2 == 0 { "even" } else { "odd" })))
            .collect();
        let limited = search_items(&items, "even", &terms_of("even"), 5, TOTAL_CAP);
        assert_eq!(limited.total, 25, "every match is counted");
        assert!(!limited.truncated, "the count is exact");
        assert_eq!(indices(&limited), vec![40, 42, 44, 46, 48], "the newest five, ascending");

        let capped = search_items(&items, "step", &terms_of("step"), 5, 20);
        assert_eq!(capped.total, 20);
        assert!(capped.truncated, "more than the cap match");
        assert_eq!(indices(&capped), vec![45, 46, 47, 48, 49]);

        let exactly = search_items(&items, "even", &terms_of("even"), 5, 25);
        assert_eq!((exactly.total, exactly.truncated), (25, false), "reaching the cap is not passing it");

        let count_only = search_items(&items, "odd", &terms_of("odd"), 0, TOTAL_CAP);
        assert_eq!((count_only.total, count_only.matches.len()), (25, 0));
    }

    #[test]
    fn a_query_is_validated_before_anything_is_read() {
        assert_eq!(validate_query(""), Err(QueryError::Empty));
        assert_eq!(validate_query(" \t\n "), Err(QueryError::Empty));
        assert_eq!(validate_query(&"x".repeat(MAX_QUERY_CHARS + 1)), Err(QueryError::TooLong));
        assert!(validate_query(&"界".repeat(MAX_QUERY_CHARS)).is_ok(), "characters, not bytes");
        let many: Vec<String> = (0..=MAX_TERMS).map(|n| format!("t{n}")).collect();
        assert_eq!(validate_query(&many.join(" ")), Err(QueryError::TooManyTerms));
        assert_eq!(
            validate_query(" Foo foo  BAR "),
            Ok(vec!["foo".to_string(), "bar".to_string()]),
            "folded and deduplicated"
        );
        assert!(
            search(&[text(Role::User, "anything")], "   ").matches.is_empty(),
            "no terms match nothing"
        );
    }

    #[test]
    fn a_timestamp_and_the_item_shape_come_with_a_match() {
        let items = vec![
            text(Role::User, "hello"),
            make_item(Role::System, ItemKind::Text, "hello from remote-hand", None),
        ];
        let result = search(&items, "hello");
        assert_eq!(result.matches[0].timestamp.as_ref().map(String::as_str), Some("2026-10-01T10:00:00.000Z"));
        assert_eq!(result.matches[1].timestamp.as_ref(), None);
        assert_eq!(result.matches[1].role, Role::System);
        let json = serde_json::to_value(&result).expect("json");
        assert_eq!(json["query"], "hello");
        assert_eq!(json["matches"][0]["field"], "text");
        assert_eq!(json["matches"][0]["kind"], "text");
        assert_eq!(json["matches"][0]["ranges"], serde_json::json!([[0, 5]]));
        assert!(json["matches"][1]["timestamp"].is_null(), "null, not absent");
    }
}
