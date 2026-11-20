// In-session search, without a DOM: which key opens which session's bar, how
// stepping wraps, how many history requests a jump costs and when it gives up,
// and where the hits are inside a run of text.
import assert from "node:assert/strict";
import test from "node:test";
import {
  foldText,
  HISTORY_PAGE_SIZE,
  JUMP_MARGIN,
  highlightSegments,
  hitRanges,
  isFindChord,
  mergeRanges,
  newestFirst,
  PAGE_IN_SLACK,
  pageInBudget,
  pagesToReach,
  pickSearchTarget,
  planPageIn,
  positionNear,
  renderedIndexFor,
  routeFindShortcut,
  searchTermsOf,
  stepMatch,
  type FindKey,
  type SearchMatch,
  type SearchTargetInfo,
  JUMP_IDLE,
  jumpStatusReducer,
  type JumpEvent,
  type JumpStatus,
} from "../src/features/sessions/session-search.js";
import { MAX_HISTORY_ITEMS } from "../src/features/sessions/session-feed-state.js";

function key(overrides: Partial<FindKey>): FindKey {
  return { key: "f", code: "KeyF", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...overrides };
}

function match(index: number): SearchMatch {
  return { index, role: "assistant", kind: "text", field: "text", snippet: `item ${index}`, ranges: [], timestamp: null };
}

// ------------------------------------------------------------------ shortcut

test("the find chord is Cmd+F on a Mac and Ctrl+F elsewhere, and nothing else", () => {
  assert.equal(isFindChord(key({ metaKey: true }), true), true);
  assert.equal(isFindChord(key({ ctrlKey: true }), true), false, "Ctrl+F is forward-char on a Mac");
  assert.equal(isFindChord(key({ ctrlKey: true }), false), true);
  assert.equal(isFindChord(key({ metaKey: true }), false), false);
  assert.equal(isFindChord(key({ metaKey: true, key: "F" }), true), true, "caps lock");
  assert.equal(isFindChord(key({ metaKey: true, shiftKey: true, key: "F" }), true), false, "Cmd+Shift+F is not find");
  assert.equal(isFindChord(key({ metaKey: true, altKey: true }), true), false);
  assert.equal(isFindChord(key({ metaKey: true, key: "g", code: "KeyG" }), true), false);
  assert.equal(isFindChord(key({ metaKey: true, isComposing: true }), true), false, "an IME keystroke");
  // A Cyrillic layout: `key` is "а", the physical key is still F.
  assert.equal(isFindChord(key({ metaKey: true, key: "а", code: "KeyF" }), true), true);
  // Dvorak: the key labelled F sits where QWERTY's Y is; the letter wins.
  assert.equal(isFindChord(key({ metaKey: true, key: "f", code: "KeyY" }), true), true);
  assert.equal(isFindChord(key({ metaKey: true, key: "u", code: "KeyF" }), true), false);
});

test("the chord opens the bar once, and a second press goes to the browser's find", () => {
  const press = key({ metaKey: true });
  // First press: nothing of ours has the focus yet.
  assert.equal(routeFindShortcut(press, { isMac: true, searchInputFocused: false, hasTarget: true }), "open");
  // Opening the bar focused its input, so the same chord again is native.
  assert.equal(routeFindShortcut(press, { isMac: true, searchInputFocused: true, hasTarget: true }), "native");
  // No session to search at all: the browser's find, untouched.
  assert.equal(routeFindShortcut(press, { isMac: true, searchInputFocused: false, hasTarget: false }), "native");
  // Anything else is not ours to look at.
  assert.equal(
    routeFindShortcut(key({ metaKey: true, key: "d", code: "KeyD" }), { isMac: true, searchInputFocused: false, hasTarget: true }),
    "ignore",
  );
});

test("the chord searches the session holding the focus, else the one touched last", () => {
  const view = (key: string, lastInteraction: number, extra: Partial<SearchTargetInfo<string>> = {}) => ({
    key,
    visible: true,
    holdsFocus: false,
    lastInteraction,
    ...extra,
  });
  assert.equal(pickSearchTarget([view("only", 1)], false), "only", "a single session view");
  assert.equal(pickSearchTarget([view("left", 5), view("right", 9)], false), "right", "the pane touched last");
  assert.equal(
    pickSearchTarget([view("left", 5, { holdsFocus: true }), view("right", 9)], false),
    "left",
    "focus beats recency — its composer is focused",
  );
  assert.equal(
    pickSearchTarget([view("hidden", 99, { visible: false }), view("shown", 1)], false),
    "shown",
    "a pane in a screen that is switched away cannot be searched",
  );
  assert.equal(
    pickSearchTarget([view("left", 5), view("right", 9)], true),
    null,
    "the focus is in a terminal pane: the browser's find",
  );
  assert.equal(pickSearchTarget([view("hidden", 1, { visible: false })], false), null);
  assert.equal(pickSearchTarget<string>([], false), null);
});

// ---------------------------------------------------------------- navigation

test("stepping wraps around both ends", () => {
  assert.equal(stepMatch(0, 3, 1), 1);
  assert.equal(stepMatch(2, 3, 1), 0, "past the oldest back to the newest");
  assert.equal(stepMatch(0, 3, -1), 2, "before the newest round to the oldest");
  assert.equal(stepMatch(1, 3, -1), 0);
  assert.equal(stepMatch(0, 1, 1), 0, "one match wraps onto itself");
  assert.equal(stepMatch(-1, 3, 1), 0, "from nowhere, forward is the first");
  assert.equal(stepMatch(-1, 3, -1), 2, "and backward the last");
  assert.equal(stepMatch(7, 3, 1), 0, "a stale position from a longer list");
  assert.equal(stepMatch(0, 0, 1), -1, "nothing to step through");
});

test("matches are walked newest first, and a re-query keeps the reader's place", () => {
  const ordered = newestFirst([match(3), match(10), match(42)]);
  assert.deepEqual(
    ordered.map((m) => m.index),
    [42, 10, 3],
  );
  assert.equal(positionNear(ordered, 10), 1, "the same item");
  assert.equal(positionNear(ordered, 20), 1, "gone: the next older one");
  assert.equal(positionNear(ordered, 1), 2, "nothing older: the oldest");
  assert.equal(positionNear(ordered, null), 0, "a fresh query starts at the newest");
  assert.equal(positionNear([], 10), -1);
  // A live session said it again: the new match is first, the reader stays.
  const grown = newestFirst([match(3), match(10), match(42), match(57)]);
  assert.equal(grown[positionNear(grown, 10)]!.index, 10);
});

// ------------------------------------------------------------------- paging

test("a match inside the window needs no paging", () => {
  const window = { startIndex: 800, endIndex: 1000, hasMoreBefore: true };
  assert.deepEqual(planPageIn({ target: 800, window, loading: false, pagesLoaded: 0, budget: 3 }), { kind: "ready" });
  assert.deepEqual(planPageIn({ target: 999, window, loading: false, pagesLoaded: 0, budget: 3 }), { kind: "ready" });
  assert.equal(pagesToReach(900, 800), 0);
});

type PageWindow = { startIndex: number; endIndex: number; hasMoreBefore: boolean };

/**
 * The window after the server answers a request reaching back to `from`: as
 * far back as `cap` items, and never less than a page — or, from a server
 * that predates ranges (`cap` a page), the one page.
 */
function answered(window: PageWindow, from: number, cap = MAX_HISTORY_ITEMS): PageWindow {
  const start = Math.max(0, Math.min(window.startIndex - HISTORY_PAGE_SIZE, Math.max(from, window.startIndex - cap)));
  return { startIndex: start, endIndex: window.endIndex, hasMoreBefore: start > 0 };
}

/** Walk a jump to `target` the way the view does, counting its requests. */
function jump(start: PageWindow, target: number, cap = MAX_HISTORY_ITEMS) {
  let window = start;
  const budget = pageInBudget(target, window.startIndex);
  let requests = 0;
  const steps: { from: number; requestsLeft: number }[] = [];
  for (;;) {
    const step = planPageIn({ target, window, loading: false, pagesLoaded: requests, budget });
    if (step.kind !== "load") return { step, requests, steps, window };
    steps.push({ from: step.from, requestsLeft: step.requestsLeft });
    // The request is in flight: nothing else goes out meanwhile.
    assert.deepEqual(planPageIn({ target, window, loading: true, pagesLoaded: requests + 1, budget }), { kind: "wait" });
    requests += 1;
    window = answered(window, step.from, cap);
  }
}

test("a jump to an item many pages back asks for the whole way in one request, with a page above the item", () => {
  // Nine pages back: the first unread message after a long absence.
  const { step, requests, steps, window } = jump({ startIndex: 9800, endIndex: 10000, hasMoreBefore: true }, 8000);
  assert.equal(step.kind, "ready");
  assert.equal(requests, 1, "one request, not nine");
  assert.deepEqual(steps, [{ from: 8000 - JUMP_MARGIN, requestsLeft: 1 }], "one request draws no n/m");
  // Not the item as the window's first row, which the view cannot centre: it
  // would land at the top, where "load earlier" fires under it, and a tool
  // result's call would be left outside.
  assert.equal(8000 - window.startIndex, JUMP_MARGIN);
});

test("further back than the cap, each request reaches as far and the next goes on from where it stopped", () => {
  const { step, requests, steps } = jump({ startIndex: 9000, endIndex: 9200, hasMoreBefore: true }, 100);
  assert.equal(step.kind, "ready");
  assert.equal(requests, 5, "9000 → 7000 → 5000 → 3000 → 1000 → 0");
  assert.deepEqual(steps.map((s) => s.from), [0, 0, 0, 0, 0], "a page above the item, as far as there is one");
  assert.deepEqual(steps.map((s) => s.requestsLeft), [5, 4, 3, 2, 1], "the estimate counts down");
});

test("a range the cap cut short of the rows above the item is topped up by one more request", () => {
  // The cap stops the first answer ten items above the target.
  const { step, requests, steps, window } = jump({ startIndex: 4300, endIndex: 4500, hasMoreBefore: true }, 2310);
  assert.equal(step.kind, "ready");
  assert.equal(requests, 2);
  assert.deepEqual(steps.map((s) => s.from), [2110, 2110]);
  assert.ok(2310 - window.startIndex >= JUMP_MARGIN, `${window.startIndex}`);
});

test("a server that answers a page at a time still gets the jump there, rows above and all, within its budget", () => {
  // One from before ranges, or ranges the byte cap cut down to a page.
  const { step, requests, steps, window } = jump({ startIndex: 1000, endIndex: 1200, hasMoreBefore: true }, 437, HISTORY_PAGE_SIZE);
  assert.equal(step.kind, "ready");
  assert.equal(requests, 4, "1000 → 800 → 600 → 400, which holds the item, → 200, which holds the rows above it");
  assert.ok(requests <= pageInBudget(437, 1000));
  assert.equal(pageInBudget(437, 1000), 4 + PAGE_IN_SLACK, "the budget is counted in pages to the rows above");
  assert.deepEqual(steps.map((s) => s.from), [237, 237, 237, 237]);
  assert.deepEqual([window.startIndex, window.hasMoreBefore], [200, true]);
});

test("a jump short of the rows above lands anyway when there are no more, or its budget is spent", () => {
  const held = { startIndex: 400, endIndex: 1200, hasMoreBefore: true };
  assert.deepEqual(planPageIn({ target: 437, window: held, loading: false, pagesLoaded: 2, budget: 6 }), { kind: "load", from: 237, requestsLeft: 1 });
  assert.deepEqual(planPageIn({ target: 437, window: held, loading: true, pagesLoaded: 2, budget: 6 }), { kind: "wait" });
  assert.deepEqual(planPageIn({ target: 437, window: held, loading: false, pagesLoaded: 6, budget: 6 }), { kind: "ready" });
  assert.deepEqual(planPageIn({ target: 437, window: { ...held, startIndex: 0, hasMoreBefore: false }, loading: false, pagesLoaded: 2, budget: 6 }), { kind: "ready" });
  assert.deepEqual(planPageIn({ target: 10, window: { ...held, startIndex: 5, hasMoreBefore: false }, loading: false, pagesLoaded: 1, budget: 6 }), { kind: "ready" });
});

test("paging in to the very first item ends at the start of the transcript", () => {
  const start = { startIndex: 450, endIndex: 650, hasMoreBefore: true };
  const { step, requests, window } = jump(start, 0);
  assert.equal(step.kind, "ready");
  assert.equal(requests, 1);
  assert.deepEqual([window.startIndex, window.hasMoreBefore], [0, false]);
  assert.equal(jump(start, 0, HISTORY_PAGE_SIZE).requests, 3, "a page at a time: 450 → 250 → 50 → 0");
});

test("a jump gives up rather than paging for ever", () => {
  const window = { startIndex: 1000, endIndex: 1200, hasMoreBefore: true };
  const budget = pageInBudget(900, window.startIndex);
  // Every page refused (the window never moves): the budget runs out.
  let pages = 0;
  let step = planPageIn({ target: 900, window, loading: false, pagesLoaded: pages, budget });
  while (step.kind === "load") {
    pages += 1;
    step = planPageIn({ target: 900, window, loading: false, pagesLoaded: pages, budget });
  }
  assert.deepEqual(step, { kind: "fail", reason: "budget" });
  assert.equal(pages, 2 + PAGE_IN_SLACK, "two pages to the rows above the item, and the slack");
  // A window that has nothing before it cannot hold an older item.
  assert.deepEqual(
    planPageIn({ target: 10, window: { startIndex: 50, endIndex: 250, hasMoreBefore: false }, loading: false, pagesLoaded: 0, budget: 9 }),
    { kind: "fail", reason: "unreachable" },
  );
});

test("a reset in the middle of paging is planned from where the window is now", () => {
  // Two requests in, the transcript was rewritten and the window replaced by
  // a fresh tail: the jump asks again, from where the window is now.
  const reset = { startIndex: 1100, endIndex: 1300, hasMoreBefore: true };
  const step = planPageIn({ target: 437, window: reset, loading: false, pagesLoaded: 2, budget: 6 });
  assert.deepEqual(step, { kind: "load", from: 237, requestsLeft: 1 });
  assert.deepEqual(
    planPageIn({ target: 437, window: reset, loading: false, pagesLoaded: 2, budget: 6, rangeSize: HISTORY_PAGE_SIZE }),
    { kind: "load", from: 237, requestsLeft: 5 },
    "estimated a page at a time, it is the pages left",
  );
});

test("a match newer than the window waits for the live feed", () => {
  const window = { startIndex: 800, endIndex: 1000, hasMoreBefore: true };
  assert.deepEqual(planPageIn({ target: 1000, window, loading: false, pagesLoaded: 0, budget: 3 }), { kind: "ahead" });
  assert.deepEqual(planPageIn({ target: 1003, window, loading: true, pagesLoaded: 0, budget: 3 }), { kind: "ahead" });
});

test("a tool result folded into its call is found on the call's row", () => {
  const items = [
    { kind: "text", toolUseId: undefined, resultOf: undefined, toolName: null },
    { kind: "tool_use", toolUseId: "toolu_1", resultOf: undefined, toolName: "Bash" },
    { kind: "tool_result", toolUseId: undefined, resultOf: "toolu_1", toolName: "Bash" },
    { kind: "tool_result", toolUseId: undefined, resultOf: "toolu_gone", toolName: "Bash" },
    { kind: "tool_use", toolUseId: "toolu_q", resultOf: undefined, toolName: "AskUserQuestion" },
    { kind: "tool_result", toolUseId: undefined, resultOf: "toolu_q", toolName: "AskUserQuestion" },
  ] as const;
  const start = 500;
  assert.equal(renderedIndexFor([...items], start, 502), 501, "drawn by its call");
  assert.equal(renderedIndexFor([...items], start, 503), 503, "its call scrolled out: its own row");
  assert.equal(renderedIndexFor([...items], start, 505), 505, "a question draws its own body and not its answer");
  assert.equal(renderedIndexFor([...items], start, 500), 500);
  assert.equal(renderedIndexFor([...items], start, 99), 99, "outside the window: unchanged");
});

// --------------------------------------------------------------- highlights

test("every term is found, whatever its case", () => {
  assert.deepEqual(searchTermsOf("  Foo   BAR foo "), ["foo", "bar"]);
  assert.deepEqual(hitRanges("Foo bar FOO", ["foo"]), [
    [0, 3],
    [8, 11],
  ]);
  assert.deepEqual(hitRanges("run the tests", ["tests", "run"]), [
    [0, 3],
    [8, 13],
  ]);
  assert.deepEqual(hitRanges("nothing here", ["absent"]), []);
  assert.deepEqual(hitRanges("", ["x"]), []);
  assert.deepEqual(hitRanges("abc", []), []);
});

test("overlapping and touching hits of different terms are one highlight", () => {
  assert.deepEqual(hitRanges("abcdef", ["abc", "bcd"]), [[0, 4]]);
  assert.deepEqual(hitRanges("abcdef", ["ab", "cd"]), [[0, 4]], "touching");
  assert.deepEqual(hitRanges("abcdef", ["bcd", "abcdef"]), [[0, 6]], "one inside another");
  assert.deepEqual(hitRanges("aaaa", ["aa"]), [[0, 4]], "repeats of one term");
  assert.deepEqual(mergeRanges([[5, 7], [0, 2], [1, 3], [9, 9]]), [
    [0, 3],
    [5, 7],
  ]);
});

test("CJK, emoji and Greek fold and slice in UTF-16 units", () => {
  const text = "🎉 修复登录页面 ΟΔΟΣ οδος";
  const hits = hitRanges(text, searchTermsOf("登录 οδοσ"));
  assert.deepEqual(
    hits.map(([start, end]) => text.slice(start, end)),
    ["登录", "ΟΔΟΣ", "οδος"],
    "both sigmas fold alike",
  );
  // An expanding fold: İ lower-cases to two units, and the hit still lands on
  // the original letters.
  const turkish = "İstanbul and istanbul";
  assert.equal(foldText(turkish).folded.length, turkish.length + 1);
  assert.deepEqual(
    hitRanges(turkish, searchTermsOf("stanbul")).map(([start, end]) => turkish.slice(start, end)),
    ["stanbul", "stanbul"],
  );
  assert.deepEqual(
    hitRanges(turkish, ["i"]).map(([start, end]) => turkish.slice(start, end)),
    ["İ", "i"],
    "the hit covers the whole letter it came from",
  );
});

test("a text node is split into runs around its hits", () => {
  assert.deepEqual(highlightSegments("fix the parser now", hitRanges("fix the parser now", ["parser", "fix"])), [
    { text: "fix", hit: true },
    { text: " the ", hit: false },
    { text: "parser", hit: true },
    { text: " now", hit: false },
  ]);
  // Overlapping, unsorted and out-of-range ranges, as a careless caller might pass.
  assert.deepEqual(
    highlightSegments("abcdefgh", [
      [6, 99],
      [1, 3],
      [2, 4],
    ]),
    [
      { text: "a", hit: false },
      { text: "bcd", hit: true },
      { text: "ef", hit: false },
      { text: "gh", hit: true },
    ],
  );
  assert.deepEqual(highlightSegments("plain", []), [{ text: "plain", hit: false }]);
  assert.deepEqual(highlightSegments("", []), [{ text: "", hit: false }]);
  assert.deepEqual(highlightSegments("all", [[0, 3]]), [{ text: "all", hit: true }]);
});

test("the server's snippet ranges slice the snippet directly", () => {
  // As `GET …/search` sends it: UTF-16 offsets, the emoji counting two.
  const snippet = "…🚀 deploy to 预发 环境 done";
  const ranges = [
    [4, 10],
    [14, 16],
  ];
  assert.deepEqual(
    highlightSegments(snippet, ranges).filter((segment) => segment.hit).map((segment) => segment.text),
    ["deploy", "预发"],
  );
});

/* ── whose jump failed ─────────────────────────────────────────────────── */

const jumps = (...events: JumpEvent[]): JumpStatus => events.reduce(jumpStatusReducer, JUMP_IDLE);

test("only a search's own jump can leave the bar saying it could not reach the match", () => {
  assert.equal(jumps({ type: "start", owner: "search" }, { type: "fail" }).failed, true);
  // The first-unread button's jump fails behind a closed bar: that is the
  // button's news, and the next Cmd+F must not open on it.
  assert.equal(jumps({ type: "start", owner: "item" }, { type: "fail" }).failed, false);
});

test("a failure is cleared by the next jump, by opening the bar and by closing it", () => {
  const failed = jumps({ type: "start", owner: "search" }, { type: "fail" });
  assert.equal(jumpStatusReducer(failed, { type: "open" }).failed, false, "a stale failure does not greet the next search");
  assert.equal(jumpStatusReducer(failed, { type: "close" }).failed, false);
  assert.equal(jumpStatusReducer(failed, { type: "start", owner: "item" }).failed, false);
});

test("progress belongs to the jump in flight, and ends with it however it ends", () => {
  const paging = jumps({ type: "start", owner: "item" }, { type: "page", loaded: 2, total: 5 });
  assert.deepEqual(paging, { owner: "item", paging: { loaded: 2, total: 5 }, failed: false });
  for (const end of [{ type: "land" }, { type: "fail" }, { type: "cancel" }] as JumpEvent[]) {
    const after = jumpStatusReducer(paging, end);
    assert.equal(after.owner, null, end.type);
    assert.equal(after.paging, null, end.type);
  }
  assert.equal(jumpStatusReducer(JUMP_IDLE, { type: "page", loaded: 1, total: 1 }), JUMP_IDLE, "no jump, no progress");
});
