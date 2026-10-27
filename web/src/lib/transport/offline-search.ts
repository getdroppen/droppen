/*
 * In-session search and the first unread message, answered from the offline
 * copy while the machine is away (`offline.ts`) — the server's own
 * answers, computed in the page over the decrypted window the copy holds.
 *
 * A port of `server-rs/src/feed/search.rs` (`search_items`, `snippet_of`, the
 * fold, the haystacks of an item) and `attention::first_unread`, rule for
 * rule, so a match, its field, its snippet and the hit ranges in it are the
 * ones the machine would have sent for the same items, and the view above
 * cannot tell which side answered. Indices are the feed's absolute numbering:
 * a copy that starts past item 0 (the agent caps long sessions) numbers its
 * items from where it starts, and nothing before that is searched.
 */
import type { components } from "@/api/schema";
import type { FeedCopy } from "@/lib/hub/streams";

type TranscriptItem = components["schemas"]["TranscriptItem"];
type SearchField = components["schemas"]["SearchField"];
type SearchMatch = components["schemas"]["SearchMatch"];
type SessionSearchResult = components["schemas"]["SessionSearchResult"];
type SessionSummary = components["schemas"]["SessionSummary"];

/** `search.rs`'s bounds, the same numbers. */
export const DEFAULT_LIMIT = 200;
export const MAX_LIMIT = 1000;
export const TOTAL_CAP = 10_000;
export const MAX_QUERY_CHARS = 256;
export const MAX_TERMS = 16;
export const SNIPPET_CHARS = 160;
const LEAD_CHARS = 48;
const TOOL_SEPARATOR = " · ";

/** Rust's `char::is_whitespace` (Unicode `White_Space`), which `\s` is not quite. */
function isWhitespace(ch: string): boolean {
  const cp = ch.codePointAt(0)!;
  return (
    (cp >= 0x09 && cp <= 0x0d) ||
    cp === 0x20 ||
    cp === 0x85 ||
    cp === 0xa0 ||
    cp === 0x1680 ||
    (cp >= 0x2000 && cp <= 0x200a) ||
    cp === 0x2028 ||
    cp === 0x2029 ||
    cp === 0x202f ||
    cp === 0x205f ||
    cp === 0x3000
  );
}

function trimWs(text: string): string {
  const chars = [...text];
  let start = 0;
  let end = chars.length;
  while (start < end && isWhitespace(chars[start]!)) start++;
  while (end > start && isWhitespace(chars[end - 1]!)) end--;
  return chars.slice(start, end).join("");
}

const isBlank = (text: string) => trimWs(text) === "";

/** One character folded for matching: lower case, the final sigma as `σ`. */
function foldChar(ch: string): string {
  return ch.toLowerCase().replace(/ς/g, "σ");
}

/**
 * A string folded for matching — the server's `fold`. Lower-casing a whole
 * string differs from lower-casing it a character at a time only at a final
 * sigma (`ς`), which is folded to `σ` either way.
 */
export function fold(text: string): string {
  return text.toLowerCase().replace(/ς/g, "σ");
}

/** The distinct folded terms of a query, in the order they were typed. */
export function termsOf(query: string): string[] {
  const terms: string[] = [];
  let word = "";
  const take = () => {
    if (!word) return;
    const folded = fold(word);
    if (!terms.includes(folded)) terms.push(folded);
    word = "";
  };
  for (const ch of query) {
    if (isWhitespace(ch)) take();
    else word += ch;
  }
  take();
  return terms;
}

/** The terms of a query, or the sentence the server refuses it with. */
export function validateQuery(query: string): { terms: string[] } | { error: string } {
  const trimmed = trimWs(query);
  if (!trimmed) return { error: "the query is empty" };
  if ([...trimmed].length > MAX_QUERY_CHARS) return { error: `the query is longer than ${MAX_QUERY_CHARS} characters` };
  const terms = termsOf(trimmed);
  if (terms.length > MAX_TERMS) return { error: `the query has more than ${MAX_TERMS} terms` };
  return { terms };
}

function named(name: string | null, text: string): string {
  if (name === null) return text;
  return isBlank(text) ? name : `${name}${TOOL_SEPARATOR}${text}`;
}

/** The texts of an item a search looks at, each with the field it counts as, in tie-break order. */
function haystacks(item: TranscriptItem): [SearchField, string][] {
  const name = item.toolName && !isBlank(item.toolName) ? item.toolName : null;
  const detail = item.detail ?? null;
  const meta = item.meta ?? null;
  const body = item.text ?? "";
  let candidates: ([SearchField, string] | null)[];
  switch (item.kind) {
    case "tool_use":
      candidates = [["tool", named(name, body)], meta !== null ? ["tool", meta] : null, detail !== null ? ["detail", detail] : null];
      break;
    // Its `toolName` is the call's, inherited: not searched here.
    case "tool_result":
      candidates = [["detail", body], detail !== null ? ["detail", detail] : null, meta !== null ? ["detail", meta] : null];
      break;
    case "command":
      candidates = [["text", named(name, body)], detail !== null ? ["detail", detail] : null];
      break;
    default:
      candidates = [["text", body], detail !== null ? ["detail", detail] : null];
  }
  return candidates.filter((c): c is [SearchField, string] => c !== null && !isBlank(c[1]));
}

function mergeRanges(ranges: [number, number][]): [number, number][] {
  const sorted = ranges.filter(([s, e]) => s < e).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged: [number, number][] = [];
  for (const [s, e] of sorted) {
    const last = merged[merged.length - 1];
    if (last && s <= last[1]) last[1] = Math.max(last[1], e);
    else merged.push([s, e]);
  }
  return merged;
}

/** A snippet of `text` around its first hit, and the hits in it in UTF-16 offsets — the server's `snippet_of`. */
export function snippetOf(text: string, terms: readonly string[]): { snippet: string; ranges: number[][] } {
  // Whitespace runs collapse to one space (none leading or trailing), so
  // every offset below is into what is shown. Positions are characters.
  const norm: string[] = [];
  let gap = false;
  for (const ch of text) {
    if (isWhitespace(ch)) {
      gap = norm.length > 0;
      continue;
    }
    if (gap) {
      norm.push(" ");
      gap = false;
    }
    norm.push(ch);
  }

  // Folded, with the character of `norm` every folded code unit came from.
  let folded = "";
  const owner: number[] = [];
  norm.forEach((ch, at) => {
    const lower = foldChar(ch);
    folded += lower;
    for (let unit = 0; unit < lower.length; unit++) owner.push(at);
  });

  const hits: [number, number][] = [];
  for (const term of terms) {
    if (!term) continue;
    for (let at = folded.indexOf(term); at !== -1; at = folded.indexOf(term, at + term.length)) {
      hits.push([owner[at]!, owner[at + term.length - 1]! + 1]);
    }
  }
  const merged = mergeRanges(hits);

  const len = norm.length;
  const first = merged[0]?.[0] ?? 0;
  let start: number;
  let end: number;
  if (len <= SNIPPET_CHARS) {
    start = 0;
    end = len;
  } else {
    start = Math.max(0, first - LEAD_CHARS);
    const room = SNIPPET_CHARS - (start > 0 ? 1 : 0);
    if (start + room < len) {
      // Cut at both ends, or at the end only: one ellipsis per cut.
      end = start + room - 1;
    } else {
      // The end is in reach: fill the budget backwards, one leading ellipsis.
      start = len - (SNIPPET_CHARS - 1);
      end = len;
    }
  }
  const lead = start > 0;
  const tail = end < len;

  let snippet = lead ? "…" : "";
  const utf16: number[] = [];
  let at = lead ? 1 : 0;
  for (const ch of norm.slice(start, end)) {
    utf16.push(at);
    at += ch.length;
    snippet += ch;
  }
  utf16.push(at);
  if (tail) snippet += "…";

  const ranges = merged
    .filter(([s, e]) => s < end && e > start)
    .map(([s, e]) => [utf16[Math.max(s, start) - start]!, utf16[Math.min(e, end) - start]!]);
  return { snippet, ranges };
}

/**
 * Every item of the copy holding all of `terms`, newest first up to `limit`
 * described and the rest counted to `cap`, returned in ascending index — the
 * server's `search_items`, over `[copy.startIndex, copy.endIndex)`.
 */
export function searchCopy(copy: Pick<FeedCopy, "items" | "startIndex">, query: string, terms: readonly string[], limit = DEFAULT_LIMIT, cap = TOTAL_CAP): SessionSearchResult {
  let total = 0;
  let truncated = false;
  const matches: SearchMatch[] = [];
  if (terms.length > 0) {
    for (let i = copy.items.length - 1; i >= 0; i--) {
      const item = copy.items[i]!;
      const stacks = haystacks(item);
      const folded = stacks.map(([, text]) => fold(text));
      if (!terms.every((term) => folded.some((text) => text.includes(term)))) continue;
      if (total === cap) {
        truncated = true;
        break;
      }
      total++;
      if (matches.length >= limit) continue;
      // The field holding the most of the query; the earliest on a tie.
      let best = 0;
      let bestHeld = -1;
      folded.forEach((text, at) => {
        const held = terms.filter((term) => text.includes(term)).length;
        if (held > bestHeld) {
          best = at;
          bestHeld = held;
        }
      });
      const [field, text] = stacks[best]!;
      const { snippet, ranges } = snippetOf(text, terms);
      matches.push({ index: copy.startIndex + i, role: item.role, kind: item.kind, field, snippet, ranges, timestamp: item.timestamp ?? null });
    }
  }
  matches.reverse();
  return { query: trimWs(query), total, truncated, matches };
}

/** An agent message the unread count counts: assistant text. */
const isAnswer = (item: TranscriptItem) => item.role === "assistant" && item.kind === "text";

/**
 * Where the agent messages newer than `after` begin in the copy, and how many
 * there are — `attention::first_unread`. A message with no timestamp of its
 * own stands at `fallbackAt` (the row's `updatedAt`, the copy's stand-in for
 * the transcript's mtime).
 */
export function firstUnreadIn(copy: Pick<FeedCopy, "items" | "startIndex">, after: string | null, fallbackAt: string): { index: number | null; count: number } {
  let index: number | null = null;
  let count = 0;
  copy.items.forEach((item, i) => {
    if (!isAnswer(item)) return;
    const at = item.timestamp ?? fallbackAt;
    if (after === null || at > after) {
      index ??= copy.startIndex + i;
      count++;
    }
  });
  return { index, count };
}

/**
 * The read mark a seen report would have replaced, as the copy can tell it:
 * the row's unread ledger (`unreadCount`, as of the machine's last list) says
 * how many of the newest answers are unread, so the mark is the time of the
 * answer just before them — the last one read. Nothing unread: the last
 * answer's time (`attentionAt`). More unread than the copy holds answers:
 * `null`, from the start. `unreadCount` stops at 99, so past that this is the
 * 99th newest, not the very first.
 */
export function readMarkOf(row: Pick<SessionSummary, "unreadCount" | "attentionAt" | "updatedAt">, copy: Pick<FeedCopy, "items"> | null): string | null {
  const unread = Math.max(0, row.unreadCount ?? 0);
  if (unread === 0) return row.attentionAt ?? null;
  if (!copy) return null;
  let seen = 0;
  for (let i = copy.items.length - 1; i >= 0; i--) {
    const item = copy.items[i]!;
    if (!isAnswer(item)) continue;
    seen++;
    if (seen === unread + 1) return item.timestamp ?? row.updatedAt;
  }
  return null;
}
