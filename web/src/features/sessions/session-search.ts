/**
 * In-session search: the decisions, without a DOM.
 *
 * The page only holds a window of a transcript — the newest page plus whatever
 * the reader paged in — so the browser's own find cannot see most of a long
 * session. `GET /api/sessions/{source}/{id}/search` answers over the whole
 * transcript, in the feed's own item numbering, and the view pages in to a
 * match with the history request it already has, asked to reach back to the
 * match in one go. Everything here is the part of that which can be wrong in
 * a way a test can see: which key opens which session's bar, how stepping
 * wraps, how many requests a jump costs and when it gives up, and where the
 * hits are inside a run of text.
 *
 * Matches are kept **newest first**. A transcript is read from the bottom, so
 * the first match is the one nearest to where the reader already is, and
 * "next" walks back in time — up the transcript, which is where the rest of
 * them are.
 */
import type { components } from "../../api/schema.js";

export type SearchMatch = components["schemas"]["SearchMatch"];
export type SessionSearchResult = components["schemas"]["SessionSearchResult"];

/** How long typing has to pause before a query goes out. */
export const SEARCH_DEBOUNCE_MS = 200;
/** How many matches one search asks for. */
export const SEARCH_LIMIT = 200;
/**
 * Items per history page — the server's `SESSION_PAGE_SIZE`. Only used to
 * estimate how many pages a jump will cost; the pages themselves are whatever
 * the server sends.
 */
export const HISTORY_PAGE_SIZE = 200;
/**
 * Items one history request asked to reach back to an item (`from`) brings at
 * most — the server's `MAX_HISTORY_ITEMS`. Only used to estimate how many
 * requests a jump will cost: one, for anything this close.
 */
export const HISTORY_RANGE_MAX = 2000;
/**
 * How many pages past the estimate a jump may spend before it gives up. A
 * history page can be refused (the transcript was rewritten under the window,
 * or a delta raced it) and the window replaced, so the estimate is a floor, not
 * a promise; the slack bounds what a session that keeps changing can cost.
 */
export const PAGE_IN_SLACK = 3;

// ------------------------------------------------------------------ matching

/**
 * Fold a string for matching, with where each folded code unit came from.
 *
 * The server's fold, character by character: lower case, and the final sigma
 * as `σ` (`String.prototype.toLowerCase` writes `ς` at the end of a word, which
 * would make the same letters fold two ways). `from`/`to` are `null` when the
 * fold kept every offset — every lower-case mapping is one unit for one unit
 * except a handful of expansions (`İ` → `i̇`), and those are the only text that
 * pays for the map.
 */
export function foldText(text: string): { folded: string; from: number[] | null; to: number[] | null } {
  const fast = text.toLowerCase().replace(/ς/g, "σ");
  if (fast.length === text.length) return { folded: fast, from: null, to: null };
  let folded = "";
  const from: number[] = [];
  const to: number[] = [];
  let at = 0;
  for (const ch of text) {
    const lower = ch.toLowerCase().replace(/ς/g, "σ");
    for (let unit = 0; unit < lower.length; unit++) {
      from.push(at);
      to.push(at + ch.length);
    }
    folded += lower;
    at += ch.length;
  }
  return { folded, from, to };
}

/** A query's distinct folded terms, in the order they were typed. */
export function searchTermsOf(query: string): string[] {
  const terms: string[] = [];
  for (const word of query.split(/\s+/)) {
    if (!word) continue;
    const folded = foldText(word).folded;
    if (!terms.includes(folded)) terms.push(folded);
  }
  return terms;
}

/** `[start, end)` in UTF-16 code units. */
export type HitRange = [number, number];

/** Sorted, with ranges that overlap or touch merged into one. */
export function mergeRanges(ranges: readonly (readonly number[])[]): HitRange[] {
  const sorted = ranges
    .filter((range) => range.length >= 2 && range[0]! < range[1]!)
    .map((range) => [range[0]!, range[1]!] as HitRange)
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged: HitRange[] = [];
  for (const range of sorted) {
    const last = merged[merged.length - 1];
    if (last && range[0] <= last[1]) last[1] = Math.max(last[1], range[1]);
    else merged.push(range);
  }
  return merged;
}

/**
 * Every hit of every term in `text`, merged: two terms that overlap
 * (`abc` and `bcd` in `abcd`) are one highlight, not two that fight over the
 * middle.
 */
export function hitRanges(text: string, terms: readonly string[]): HitRange[] {
  if (!text || terms.length === 0) return [];
  const { folded, from, to } = foldText(text);
  const hits: HitRange[] = [];
  for (const term of terms) {
    if (!term) continue;
    for (let at = folded.indexOf(term); at !== -1; at = folded.indexOf(term, at + term.length)) {
      const end = at + term.length;
      hits.push(from && to ? [from[at]!, to[end - 1]!] : [at, end]);
    }
  }
  return mergeRanges(hits);
}

/** One run of a highlighted string: a hit, or the text between two. */
export interface Segment {
  text: string;
  hit: boolean;
}

/**
 * Cut `text` at `ranges`, which may come from anywhere — the server's snippet
 * ranges or [`hitRanges`] — so they are clamped, sorted and merged first.
 */
export function highlightSegments(text: string, ranges: readonly (readonly number[])[]): Segment[] {
  const clamped = ranges.map((range) => [
    Math.max(0, Math.min(text.length, range[0] ?? 0)),
    Math.max(0, Math.min(text.length, range[1] ?? 0)),
  ]);
  const segments: Segment[] = [];
  let at = 0;
  for (const [start, end] of mergeRanges(clamped)) {
    if (start > at) segments.push({ text: text.slice(at, start), hit: false });
    segments.push({ text: text.slice(start, end), hit: true });
    at = end;
  }
  if (at < text.length || segments.length === 0) segments.push({ text: text.slice(at), hit: false });
  return segments;
}

// ------------------------------------------------------------------ shortcut

/** The parts of a `KeyboardEvent` the shortcut reads. */
export interface FindKey {
  key: string;
  code?: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing?: boolean;
}

/**
 * Cmd+F on a Mac, Ctrl+F elsewhere — and only that: Ctrl+F on a Mac is the
 * Emacs "forward a character" every text field there honours.
 *
 * The letter is read off `key` on a Latin layout and off the physical key
 * elsewhere, which is what the browser's own find does on a Cyrillic layout.
 */
export function isFindChord(event: FindKey, isMac: boolean): boolean {
  if (event.isComposing || event.altKey || event.shiftKey) return false;
  const latin = /^[a-z]$/i.test(event.key);
  if (latin ? event.key.toLowerCase() !== "f" : event.code !== "KeyF") return false;
  return isMac ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
}

/**
 * What one press of the find chord does.
 *
 * - `ignore`: it was not the chord.
 * - `native`: leave it to the browser. Pressed again while a search box
 *   already has the focus — which is what the first press did — it is the
 *   browser's find the user is after, not the same box a second time; and with
 *   no session to search there is nothing better than the browser's.
 * - `open`: take it, and open (or focus) the target's bar.
 */
export function routeFindShortcut(
  event: FindKey,
  context: { isMac: boolean; searchInputFocused: boolean; hasTarget: boolean },
): "ignore" | "native" | "open" {
  if (!isFindChord(event, context.isMac)) return "ignore";
  if (context.searchInputFocused || !context.hasTarget) return "native";
  return "open";
}

/** One mounted session view, as the shortcut sees it. */
export interface SearchTargetInfo<K> {
  key: K;
  /** Laid out at all: a pane in a screen that is switched away is not. */
  visible: boolean;
  /** The focused element is inside it — its composer, its own search box. */
  holdsFocus: boolean;
  /** When it was last pointed at or focused, on any monotonic clock. */
  lastInteraction: number;
}

/**
 * Which session the find chord is about, or `null` to leave it to the browser.
 *
 * The one holding the focus, then — unless the focus is in some other text
 * field (a terminal pane, a form) — the visible one touched most recently,
 * which is how the pane tree decides which leaf is focused too. A view that
 * was never touched counts from when it mounted, so the session just opened is
 * the one a bare Cmd+F searches.
 */
export function pickSearchTarget<K>(
  targets: readonly SearchTargetInfo<K>[],
  foreignEditableFocused: boolean,
): K | null {
  const shown = targets.filter((target) => target.visible);
  const focused = shown.find((target) => target.holdsFocus);
  if (focused) return focused.key;
  if (foreignEditableFocused) return null;
  let best: SearchTargetInfo<K> | null = null;
  for (const target of shown) {
    if (!best || target.lastInteraction > best.lastInteraction) best = target;
  }
  return best?.key ?? null;
}

// ---------------------------------------------------------------- navigation

/** The server's matches (ascending) as the bar walks them: newest first. */
export function newestFirst(matches: readonly SearchMatch[]): SearchMatch[] {
  return [...matches].sort((a, b) => b.index - a.index);
}

/**
 * The position `delta` steps from `position` among `count` matches, wrapping
 * at both ends; `-1` when there is nothing to step through. From no position
 * at all, forward lands on the first and backward on the last.
 */
export function stepMatch(position: number, count: number, delta: number): number {
  if (count <= 0) return -1;
  if (position < 0 || position >= count) return delta >= 0 ? 0 : count - 1;
  return (((position + delta) % count) + count) % count;
}

/**
 * Where an item index sits in a fresh list of matches (newest first) — the
 * match the reader was on survives a re-query — or, when it no longer matches,
 * the next older one, so stepping carries on from where they were.
 */
export function positionNear(ordered: readonly SearchMatch[], index: number | null): number {
  if (ordered.length === 0) return -1;
  if (index === null) return 0;
  const exact = ordered.findIndex((match) => match.index === index);
  if (exact >= 0) return exact;
  const older = ordered.findIndex((match) => match.index < index);
  return older >= 0 ? older : ordered.length - 1;
}

// ------------------------------------------------------------------- paging

/** History pages a window starting at `startIndex` needs to hold `target`. */
export function pagesToReach(target: number, startIndex: number, pageSize = HISTORY_PAGE_SIZE): number {
  return target >= startIndex ? 0 : Math.ceil((startIndex - target) / pageSize);
}

/**
 * What a jump may spend on requests before it gives up; see [`PAGE_IN_SLACK`].
 * Counted in pages, not in ranges: a server from before ranges, or a range
 * the byte cap cut short, answers a page at a time, and the jump still gets
 * there.
 */
export function pageInBudget(target: number, startIndex: number, pageSize = HISTORY_PAGE_SIZE): number {
  return pagesToReach(target, startIndex, pageSize) + PAGE_IN_SLACK;
}

/** The next move of a jump to an item that may not be loaded yet. */
export type PageInStep =
  /** The item is in the window: scroll to it. */
  | { kind: "ready" }
  /**
   * Ask for everything from `from` (the item itself) to the window's start,
   * which the server sends in one answer up to its cap. `requestsLeft` is the
   * estimate, this one included.
   */
  | { kind: "load"; from: number; requestsLeft: number }
  /** A page is already on its way; wait for it. */
  | { kind: "wait" }
  /**
   * Newer than the window's end: the search read the file a moment after the
   * live feed last did, and the feed will deliver it. Wait, but not forever.
   */
  | { kind: "ahead" }
  | { kind: "fail"; reason: "unreachable" | "budget" };

/**
 * One step of paging in to `target`, re-planned from scratch every time the
 * window changes.
 *
 * Re-planned rather than counted down, because a page is not guaranteed to
 * land where it was asked for: a rewritten transcript answers `409` and the
 * window is replaced by a fresh tail, after which the right number of pages is
 * whatever it is *now*. `budget` is what stops a transcript that keeps changing
 * from being paged for ever.
 *
 * Every request asks for the whole way back to `target` at once, since the
 * target is known: a jump within the server's cap is one request, not one per
 * page, and over a relay each of those was a round trip and a server answer
 * before the view could move. Further back than the cap, each answer is the
 * newest part of the range and the next request goes on from where it ends.
 */
export function planPageIn(input: {
  target: number;
  window: { startIndex: number; endIndex: number; hasMoreBefore: boolean };
  /** A history request is out — this jump's or anybody's. */
  loading: boolean;
  /** Requests this jump has made so far. */
  pagesLoaded: number;
  budget: number;
  /** What one request brings at most, for the estimate. */
  rangeSize?: number;
}): PageInStep {
  const { target, window } = input;
  if (target >= window.startIndex && target < window.endIndex) return { kind: "ready" };
  if (target >= window.endIndex) return { kind: "ahead" };
  if (input.loading) return { kind: "wait" };
  if (!window.hasMoreBefore) return { kind: "fail", reason: "unreachable" };
  if (input.pagesLoaded >= input.budget) return { kind: "fail", reason: "budget" };
  return {
    kind: "load",
    from: target,
    requestsLeft: pagesToReach(target, window.startIndex, input.rangeSize ?? HISTORY_RANGE_MAX),
  };
}

type WindowItem = Pick<components["schemas"]["TranscriptItem"], "kind" | "toolUseId" | "resultOf" | "toolName">;

/**
 * The index of the row that draws `index`.
 *
 * Itself, except a tool result whose call is in the same window: the call's
 * card renders the result and the result's own row is empty (see
 * `pairToolResults`), so the call is what has to be scrolled to — and opened.
 */
export function renderedIndexFor(items: readonly WindowItem[], startIndex: number, index: number): number {
  const item = items[index - startIndex];
  if (!item || item.kind !== "tool_result" || !item.resultOf) return index;
  const call = items.findIndex(
    (other) =>
      other.kind === "tool_use" && other.toolUseId === item.resultOf && other.toolName !== "AskUserQuestion",
  );
  return call >= 0 ? startIndex + call : index;
}

// ------------------------------------------------------------- jump status

/**
 * Who asked for a jump: the search bar stepping to a match, or a caller that
 * is not searching at all (the way back to a visit's first unread message).
 */
export type JumpOwner = "search" | "item";

/** What the view shows about the jump in flight, and the last one that failed. */
export interface JumpStatus {
  owner: JumpOwner | null;
  /**
   * History requests made so far, of about `total`, while a jump pages history
   * in. A jump within the server's cap is one request, and its view draws no
   * count for it.
   */
  paging: { loaded: number; total: number } | null;
  /** The search bar's "could not reach this match". Only ever a search's own. */
  failed: boolean;
}

export const JUMP_IDLE: JumpStatus = { owner: null, paging: null, failed: false };

export type JumpEvent =
  | { type: "start"; owner: JumpOwner }
  | { type: "page"; loaded: number; total: number }
  | { type: "land" }
  | { type: "fail" }
  | { type: "cancel" }
  /** The bar opens: whatever failed before it is not this search's news. */
  | { type: "open" }
  | { type: "close" };

/**
 * The jump's status, one event at a time.
 *
 * The failure flag belongs to the search bar and only a search's own jump may
 * raise it: an item jump that fails is reported by whoever asked for it, and
 * used to leave the flag up behind a closed bar — so the next Cmd+F opened on
 * a red "could not reach this match" for a search nobody had run.
 */
export function jumpStatusReducer(status: JumpStatus, event: JumpEvent): JumpStatus {
  switch (event.type) {
    case "start":
      return { owner: event.owner, paging: null, failed: false };
    case "page":
      return status.owner === null ? status : { ...status, paging: { loaded: event.loaded, total: event.total } };
    case "land":
      return JUMP_IDLE;
    case "fail":
      return { owner: null, paging: null, failed: status.owner === "search" };
    case "cancel":
      return status.owner === null && status.paging === null ? status : { ...status, owner: null, paging: null };
    case "open":
      return status.failed ? { ...status, failed: false } : status;
    case "close":
      return JUMP_IDLE;
  }
}
