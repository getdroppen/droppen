// The jump to an item further back than the window, through the hook itself
// (`useSessionSearch` in `session-search-view.tsx`): how many history requests
// it makes and what each asks for, what it says of its progress, and the row
// it lands on — against a server that answers a range in one go, and one that
// answers a page at a time. The view around it is a stand-in that renders a
// row per item and folds a tool result into its call's card the way the
// transcript does (`pairToolResults`); jsdom has no layout, so where the row
// ends up on screen is not what is checked here, only which row it is and
// how many items the window holds above it.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, type TestContext } from "node:test";
import { installJsdom } from "./jsdom.js";
import { transcriptItem } from "./session-fixtures.js";

const { window, container } = installJsdom();
// No layout in jsdom: a range measures as the empty box an element does.
(window as unknown as { Range: { prototype: Record<string, unknown> } }).Range.prototype.getBoundingClientRect = () =>
  ({ top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0 }) as DOMRect;
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

/**
 * The search's typed client builds a `Request` from a root-relative URL, which
 * Node's own `Request` refuses to parse, and reads `Request` and `fetch` once,
 * when it is first imported: both are stubbed before anything loads it, and
 * the server's answer is whatever the test puts in `answer`.
 */
class StubRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Headers;
  constructor(url: string, init: RequestInit = {}) {
    this.url = url;
    this.method = init.method ?? "GET";
    this.headers = new Headers(init.headers ?? {});
  }
}
let answer: (url: string) => unknown = () => null;
globalThis.Request = StubRequest as unknown as typeof Request;
globalThis.fetch = (async (request: StubRequest) =>
  new Response(JSON.stringify(answer(request.url)), {
    status: 200,
    headers: { "content-type": "application/json" },
  })) as unknown as typeof fetch;

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useSessionSearch } = await import("../src/features/sessions/session-search-view.js");
const { JUMP_MARGIN } = await import("../src/features/sessions/session-search.js");
const { prependHistoryPage, MAX_HISTORY_ITEMS } = await import("../src/features/sessions/session-feed-state.js");
const { pairToolResults } = await import("../src/features/sessions/transcript-groups.js");
type TranscriptItem = import("../src/features/sessions/session-feed-state.js").TranscriptItem;
type TranscriptWindow = import("../src/features/sessions/session-feed-state.js").TranscriptWindow;
type SessionSearch = import("../src/features/sessions/session-search-view.js").SessionSearch;
type JumpOutcome = import("../src/features/sessions/session-search-view.js").JumpOutcome;

const PAGE = 200;

/** `n` items, with a tool call at `call` and its result, holding `output`, right after it. */
function transcript(n: number, call: number, output: string): TranscriptItem[] {
  return Array.from({ length: n }, (_, i) =>
    i === call
      ? transcriptItem({ kind: "tool_use", toolName: "Bash", toolUseId: "call-1", text: "run it" })
      : i === call + 1
        ? transcriptItem({ kind: "tool_result", resultOf: "call-1", text: output })
        : transcriptItem({ text: `item ${i}` }),
  );
}

/** The window a snapshot hands over: the last page. */
function lastPage(all: TranscriptItem[]): TranscriptWindow {
  const start = all.length - PAGE;
  return {
    sig: "v1",
    items: all.slice(start),
    startIndex: start,
    endIndex: all.length,
    totalItems: all.length,
    hasMoreBefore: start > 0,
    prefixFp: "pfx",
    startFp: `start@${start}`,
  };
}

/**
 * The server's `GET …/history`: the page before `before`, or with `from` the
 * range back to it, `cap` items at most — a page, for a server that predates
 * ranges.
 */
function serve(all: TranscriptItem[], before: number, from: number | undefined, cap: number) {
  const page = Math.max(0, before - PAGE);
  const startIndex = from === undefined ? page : Math.min(page, Math.max(from, before - cap));
  return {
    sig: "v1",
    items: all.slice(startIndex, before),
    startIndex,
    endIndex: before,
    totalItems: all.length,
    hasMoreBefore: startIndex > 0,
    startFp: `start@${startIndex}`,
  };
}

interface Probe {
  search: SessionSearch | null;
  window: TranscriptWindow | null;
  /** The `from` of every history request, in order. */
  asked: (number | undefined)[];
  /** Every progress the jump reported, in order, each once. */
  paging: string[];
}

/**
 * A session view, as far as the jump sees it. `fails` says which of its
 * history requests (the first is 1) do not come back: the window stays as it
 * was and the view says why, as the feed does (`SessionFeed.loadEarlier`).
 */
function Session({ all, cap, probe, fails }: { all: TranscriptItem[]; cap: number; probe: Probe; fails?: (request: number) => boolean }) {
  const [win, setWin] = React.useState(() => lastPage(all));
  const [loading, setLoading] = React.useState(false);
  const [historyError, setHistoryError] = React.useState<string | null>(null);
  const busy = React.useRef(false);
  const scrollRef = React.useRef<HTMLDivElement>(null);
  const rowsRef = React.useRef<HTMLDivElement>(null);
  const loadEarlier = (from?: number) => {
    if (busy.current || !win.hasMoreBefore) return null;
    busy.current = true;
    setLoading(true);
    setHistoryError(null);
    probe.asked.push(from);
    const failed = fails?.(probe.asked.length) ?? false;
    const before = win.startIndex;
    return new Promise((resolve) => setTimeout(resolve, 0))
      .then(() => {
        if (failed) setHistoryError("history failed");
        else setWin((prev) => prependHistoryPage(prev, serve(all, before, from, cap)));
      })
      .finally(() => {
        busy.current = false;
        setLoading(false);
      });
  };
  const search = useSessionSearch({
    source: "claude",
    id: "a",
    window: win,
    isLoadingEarlier: loading,
    historyError,
    loadEarlier,
    scrollRef,
    rowsRef,
    unpin: () => undefined,
  });
  probe.search = search;
  probe.window = win;
  const paging = search.paging ?? search.itemPaging;
  const said = paging ? `${paging.loaded}/${paging.total}` : null;
  if (said && probe.paging.at(-1) !== said) probe.paging.push(said);
  // A result whose call is in the window is drawn in the call's card, folded;
  // one whose call is not is a row of its own.
  const pairing = pairToolResults(win.items);
  return (
    <div ref={scrollRef}>
      <div ref={rowsRef}>
        {win.items.map((item, i) => {
          const index = win.startIndex + i;
          if (pairing.consumed.has(item)) return <div key={index} data-ti={index} />;
          const result = item.kind === "tool_use" && item.toolUseId ? pairing.resultFor.get(item.toolUseId) : undefined;
          return (
            <div key={index} data-ti={index}>
              {item.kind === "tool_use" ? (
                <details>
                  <summary>{item.toolName}</summary>
                  <pre>{result?.text ?? ""}</pre>
                </details>
              ) : (
                <p>{item.text}</p>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** A view of its own for the test, taken down when the test ends however it ends. */
async function mount(t: TestContext, all: TranscriptItem[], cap: number, fails?: (request: number) => boolean) {
  const probe: Probe = { search: null, window: null, asked: [], paging: [] };
  const host = container.appendChild(window.document.createElement("div"));
  const root = createRoot(host);
  t.after(async () => {
    await act(async () => root.unmount());
    host.remove();
  });
  await act(async () => root.render(<Session all={all} cap={cap} probe={probe} fails={fails} />));
  const flashed = () => [...host.querySelectorAll(".rh-search-flash")].map((el) => Number(el.getAttribute("data-ti")));
  return { probe, host, flashed };
}

/** Let the requests answer and the jump re-plan, until it has said how it ended. */
async function settled(outcomes: JumpOutcome[], probe: Probe) {
  for (let i = 0; i < 200 && outcomes.length === 0 && !probe.search?.jumpFailed; i++) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  }
}

test("a search hit in a tool's output far back is one request, lands on its call's card, and opens it", async (t) => {
  // The output sits 1500 items above the window, its call just before it.
  const all = transcript(10_000, 8299, "the needle is here");
  const hit = 8300;
  const searched: string[] = [];
  answer = (url) => {
    searched.push(url);
    return {
      matches: [{ index: hit, role: "assistant", kind: "tool_result", field: "detail", snippet: "the needle is here", ranges: [[4, 10]], timestamp: null }],
      total: 1,
      truncated: false,
    };
  };
  try {
    const { probe, host, flashed } = await mount(t, all, MAX_HISTORY_ITEMS);
    await act(async () => probe.search!.openBar());
    await act(async () => probe.search!.setText("needle", false));
    await act(async () => probe.search!.enter(false));
    for (let i = 0; i < 200 && flashed().length === 0 && !probe.search!.jumpFailed; i++) {
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 0));
      });
    }
    assert.deepEqual([probe.search!.error, probe.search!.result?.total, probe.search!.jumpFailed], [null, 1, false]);
    assert.deepEqual(probe.asked, [hit - JUMP_MARGIN], "one request, reaching a page above the hit");
    assert.ok(hit - probe.window!.startIndex >= JUMP_MARGIN, "rows above it to centre it by, clear of the scroll-top load");
    assert.deepEqual(flashed(), [hit - 1], "the call's card, which draws the output");
    const card = host.querySelector<HTMLElement>(`[data-ti="${hit - 1}"]`)!;
    assert.equal(card.querySelector("details")!.open, true, "opened to the hit");
    assert.equal(host.querySelector(`[data-ti="${hit}"]`)!.textContent, "", "not a row of its own");
    assert.deepEqual(probe.paging, ["1/1"], "one request, which the bar draws without a count");
    assert.deepEqual(searched, ["/api/sessions/claude/a/search?q=needle&limit=200"]);
  } finally {
    answer = () => null;
  }
});

test("a jump within the cap is one request and lands with a page above it", async (t) => {
  const all = transcript(10_000, 10, "x");
  const { probe, flashed } = await mount(t, all, MAX_HISTORY_ITEMS);
  const outcomes: JumpOutcome[] = [];
  await act(async () => probe.search!.jumpToItem(8000, (outcome) => outcomes.push(outcome)));
  await settled(outcomes, probe);
  assert.deepEqual(outcomes, ["landed"]);
  assert.deepEqual(probe.asked, [8000 - JUMP_MARGIN]);
  assert.equal(probe.window!.startIndex, 8000 - JUMP_MARGIN);
  assert.deepEqual(flashed(), [8000]);
  assert.deepEqual(probe.paging, ["1/1"], "said as one request, which the view draws without a count");
});

test("against a server that answers a page at a time, the count follows what each answer brought", async (t) => {
  const all = transcript(10_000, 10, "x");
  const { probe, flashed } = await mount(t, all, PAGE);
  const outcomes: JumpOutcome[] = [];
  await act(async () => probe.search!.jumpToItem(8437, (outcome) => outcomes.push(outcome)));
  await settled(outcomes, probe);
  assert.deepEqual(outcomes, ["landed"]);
  // 9800 → 9600 → … → 8400, which holds the item, → 8200, which holds a page above it.
  assert.equal(probe.asked.length, 8);
  assert.ok(probe.asked.every((from) => from === 8437 - JUMP_MARGIN));
  assert.equal(probe.window!.startIndex, 8200);
  // The first request is estimated at the server's cap; its answer of a page
  // is what the rest are estimated at.
  assert.deepEqual(probe.paging, ["1/1", "2/8", "3/8", "4/8", "5/8", "6/8", "7/8", "8/8"]);
  assert.deepEqual(flashed(), [8437]);
});

test("an item the window already holds is landed on at once, with no request", async (t) => {
  const all = transcript(10_000, 10, "x");
  const { probe, flashed } = await mount(t, all, MAX_HISTORY_ITEMS);
  const outcomes: JumpOutcome[] = [];
  await act(async () => probe.search!.jumpToItem(9805, (outcome) => outcomes.push(outcome)));
  await settled(outcomes, probe);
  assert.deepEqual(outcomes, ["landed"]);
  assert.deepEqual(probe.asked, []);
  assert.deepEqual(flashed(), [9805]);
});

test("a request for the rows above an item already brought in that fails still lands on the item", async (t) => {
  const all = transcript(10_000, 10, "x");
  // A page at a time: the first answer brings the item in 37 rows from the
  // top, and the second, for the rows above it, does not come back.
  const { probe, flashed } = await mount(t, all, PAGE, (request) => request === 2);
  const outcomes: JumpOutcome[] = [];
  await act(async () => probe.search!.jumpToItem(9637, (outcome) => outcomes.push(outcome)));
  await settled(outcomes, probe);
  assert.deepEqual(probe.asked, [9437, 9437]);
  assert.equal(probe.window!.startIndex, 9600);
  assert.deepEqual(outcomes, ["landed"]);
  assert.deepEqual(flashed(), [9637]);
});

test("a request that fails before the item is in fails the jump", async (t) => {
  const all = transcript(10_000, 10, "x");
  const { probe, flashed } = await mount(t, all, MAX_HISTORY_ITEMS, () => true);
  const outcomes: JumpOutcome[] = [];
  await act(async () => probe.search!.jumpToItem(8000, (outcome) => outcomes.push(outcome)));
  await settled(outcomes, probe);
  assert.deepEqual(probe.asked, [7800]);
  assert.deepEqual(outcomes, ["failed"]);
  assert.deepEqual(flashed(), []);
});
