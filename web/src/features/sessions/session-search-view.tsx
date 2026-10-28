/**
 * In-session search: the bar, the shortcut, the jump and the highlights.
 *
 * The decisions live in `session-search.ts`, where they are tested; this is the
 * half that touches the DOM and the network. Three pieces, each owned by one
 * session view:
 *
 * - [`useSessionSearch`]: the query, its results, which match is active, and
 *   the jump — which pages history in through the view's own "load earlier"
 *   until the match is in the window, then scrolls it into view and flashes it.
 * - [`useFindShortcut`]: one `keydown` listener for the whole page, shared by
 *   every mounted session view, that hands Cmd/Ctrl+F to the session the user
 *   is working in.
 * - The highlights, painted with the CSS Custom Highlight API over the rows'
 *   own text nodes. Nothing is wrapped in a `<mark>` inside the transcript:
 *   those text nodes belong to React and to the Markdown renderer, and
 *   splitting them from outside would break the next reconciliation of any row
 *   that changed — a live session's newest rows change all the time. A
 *   highlight paints the same ranges and moves nothing, so links, code blocks
 *   and the Markdown structure are exactly what they were.
 */
import {
  Bot,
  Brain,
  ChevronDown,
  ChevronUp,
  Info,
  List,
  Loader2,
  Search,
  User,
  Wrench,
  X,
  type LucideIcon,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
  type RefObject,
} from "react";
import { useTranslation } from "react-i18next";
import { errorText } from "@/api/errors";
import type { components } from "@/api/schema";
import { cn } from "@/lib/cn";
import { formatInt, formatTime } from "@/lib/format";
import { isImeKeydown } from "@/lib/ime";
import {
  highlightSegments,
  hitRanges,
  JUMP_IDLE,
  jumpStatusReducer,
  newestFirst,
  pageInBudget,
  pickSearchTarget,
  planPageIn,
  positionNear,
  renderedIndexFor,
  routeFindShortcut,
  isFindChord,
  SEARCH_DEBOUNCE_MS,
  SEARCH_LIMIT,
  searchTermsOf,
  stepMatch,
  type JumpOwner,
  type SearchMatch,
} from "@/features/sessions/session-search";
import { useApi } from "@/lib/machines/context";
import { isSessionSource } from "./new-session-flow";

type TranscriptItem = components["schemas"]["TranscriptItem"];

/** Marks a session search box, so a second Cmd+F can tell it has the focus. */
const SEARCH_INPUT_ATTR = "data-session-search-input";
/** How long a jump waits for the live feed to deliver an item newer than the window. */
const AHEAD_WAIT_MS = 6000;
/** How many highlight ranges one view paints, at most. */
const MAX_PAINTED_RANGES = 4000;
/** How long the landed row stays lit. */
const FLASH_MS = 1600;

// ---------------------------------------------------------------- highlights

const HIT_HIGHLIGHT = "rh-search-hit";
const ACTIVE_HIGHLIGHT = "rh-search-active";

function highlightsSupported(): boolean {
  return typeof CSS !== "undefined" && "highlights" in CSS && typeof Highlight === "function";
}

/**
 * Every view's ranges, by owner. The two highlight names are global to the
 * document and every pane on screen paints into them, so each view's share is
 * kept apart and the two highlights are rebuilt from all of them.
 */
const painted = new Map<object, { hits: Range[]; active: Range[] }>();

function paint(owner: object, hits: Range[], active: Range[]) {
  if (!highlightsSupported()) return;
  if (hits.length === 0 && active.length === 0) painted.delete(owner);
  else painted.set(owner, { hits, active });
  const hit = new Highlight();
  const current = new Highlight();
  current.priority = 1;
  for (const share of painted.values()) {
    for (const range of share.hits) hit.add(range);
    for (const range of share.active) current.add(range);
  }
  CSS.highlights.set(HIT_HIGHLIGHT, hit);
  CSS.highlights.set(ACTIVE_HIGHLIGHT, current);
}

/** The text nodes under `root`, in document order. */
function textNodes(root: Node, skip?: Node | null): Text[] {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) =>
      skip && skip.contains(node) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
  });
  const out: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) out.push(node as Text);
  return out;
}

function rangesIn(nodes: Text[], terms: readonly string[], budget: number): Range[] {
  const out: Range[] = [];
  for (const node of nodes) {
    for (const [start, end] of hitRanges(node.data, terms)) {
      if (out.length >= budget) return out;
      const range = document.createRange();
      range.setStart(node, start);
      range.setEnd(node, end);
      out.push(range);
    }
  }
  return out;
}

/** The first text node under `el` with a hit in it. */
function firstHitNode(el: Element, terms: readonly string[]): Text | null {
  for (const node of textNodes(el)) {
    if (hitRanges(node.data, terms).length > 0) return node;
  }
  return null;
}

/**
 * Open what hides a hit: the step group the row is folded into, whatever the
 * hit, and inside the row every fold between the row and the first hit —
 * except one whose own summary line holds the hit, which already shows it.
 */
function openFoldsFor(el: HTMLElement, terms: readonly string[], field: SearchMatch["field"]) {
  for (let fold = el.parentElement?.closest("details"); fold; fold = fold.parentElement?.closest("details")) {
    fold.open = true;
  }
  const node = firstHitNode(el, terms);
  if (!node) {
    // Nothing on screen spells the hit (a field this row draws differently),
    // so open the row's own fold when the match was in its body.
    if (field === "detail") {
      const own = el.querySelector("details");
      if (own) own.open = true;
    }
    return;
  }
  for (
    let fold = node.parentElement?.closest("details") ?? null;
    fold && el.contains(fold);
    fold = fold.parentElement?.closest("details") ?? null
  ) {
    const summary = fold.querySelector(":scope > summary");
    if (!summary?.contains(node)) fold.open = true;
  }
}

/** Scroll `scroller` so `rect` sits in its middle, or at its top when taller. */
function centerOn(scroller: HTMLElement, rect: DOMRect) {
  const box = scroller.getBoundingClientRect();
  const top = rect.top - box.top + scroller.scrollTop;
  const desired =
    rect.height >= scroller.clientHeight * 0.8
      ? top - 24
      : top - (scroller.clientHeight - rect.height) / 2;
  scroller.scrollTop = Math.max(0, Math.min(desired, scroller.scrollHeight - scroller.clientHeight));
}

function rowFor(rows: HTMLElement, index: number): HTMLElement | null {
  return rows.querySelector<HTMLElement>(`[data-ti="${index}"]`);
}

// ------------------------------------------------------------------ shortcut

const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);

interface Registered {
  root: HTMLElement;
  lastInteraction: number;
  open: () => void;
}

const registered = new Set<Registered>();
/** A monotonic clock for "touched most recently"; a counter cannot tie. */
let clock = 0;

function isShown(el: HTMLElement): boolean {
  return el.isConnected && el.getClientRects().length > 0;
}

function isEditable(el: Element | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  return el.isContentEditable || el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT";
}

function onFindKey(event: KeyboardEvent) {
  if (event.defaultPrevented || !isFindChord(event, IS_MAC)) return;
  const focus = document.activeElement;
  const entries = [...registered];
  const inside = (entry: Registered) => focus !== null && entry.root.contains(focus);
  const target = pickSearchTarget(
    entries.map((entry) => ({
      key: entry,
      visible: isShown(entry.root),
      holdsFocus: inside(entry),
      lastInteraction: entry.lastInteraction,
    })),
    isEditable(focus) && !entries.some(inside),
  );
  const action = routeFindShortcut(event, {
    isMac: IS_MAC,
    searchInputFocused: focus instanceof HTMLElement && focus.hasAttribute(SEARCH_INPUT_ATTR),
    hasTarget: target !== null,
  });
  if (action !== "open" || !target) return;
  event.preventDefault();
  target.open();
}

/**
 * Put a session view up for the find chord: `root` is the view, `open` what
 * the chord does to it. Pointing at or focusing anything inside the view makes
 * it the most recently touched — the same two events the pane tree uses to
 * decide which pane is focused.
 */
export function useFindShortcut(rootRef: RefObject<HTMLElement | null>, open: () => void, ready: boolean) {
  const openRef = useRef(open);
  useLayoutEffect(() => {
    openRef.current = open;
  });
  useEffect(() => {
    const root = rootRef.current;
    if (!ready || !root) return;
    const entry: Registered = { root, lastInteraction: ++clock, open: () => openRef.current() };
    const touch = () => {
      entry.lastInteraction = ++clock;
    };
    root.addEventListener("pointerdown", touch, true);
    root.addEventListener("focusin", touch, true);
    registered.add(entry);
    if (registered.size === 1) window.addEventListener("keydown", onFindKey);
    return () => {
      root.removeEventListener("pointerdown", touch, true);
      root.removeEventListener("focusin", touch, true);
      registered.delete(entry);
      if (registered.size === 0) window.removeEventListener("keydown", onFindKey);
    };
  }, [rootRef, ready]);
}

// -------------------------------------------------------------------- search

/** What the search needs from the session view it lives in. */
export interface SearchHost {
  source: string;
  id: string;
  window: {
    items: TranscriptItem[];
    startIndex: number;
    endIndex: number;
    totalItems: number;
    hasMoreBefore: boolean;
  } | null;
  isLoadingEarlier: boolean;
  historyError: string | null;
  /**
   * Ask for the page before the window exactly as the reader scrolling up
   * does — anchored, so the rows on screen stay put. `null` when no request
   * went out (one is already in flight).
   */
  loadEarlier: () => Promise<void> | null;
  scrollRef: RefObject<HTMLDivElement | null>;
  rowsRef: RefObject<HTMLDivElement | null>;
  /** Take the view off the newest row, so the pin and the trim leave it be. */
  unpin: () => void;
}

/** One search's answer, as the bar walks it. */
interface ResultView {
  query: string;
  terms: string[];
  /** Newest first. */
  ordered: SearchMatch[];
  total: number;
  truncated: boolean;
  /** The transcript's length when it was asked, to know when it is stale. */
  atTotal: number;
}

/** How a jump ended, for the caller that asked for it. */
export type JumpOutcome = "landed" | "failed" | "cancelled";

interface Jump {
  /** The item to land on, and which of its fields holds the hit. */
  match: Pick<SearchMatch, "index" | "field">;
  terms: string[];
  owner: JumpOwner;
  /** Told how it ended, once. */
  onSettled?: (outcome: JumpOutcome) => void;
  pagesLoaded: number;
  budget: number;
  inflight: boolean;
  aheadSince: number | null;
}

export interface SessionSearch {
  open: boolean;
  openBar: () => void;
  close: () => void;
  inputRef: RefObject<HTMLInputElement | null>;
  text: string;
  setText: (value: string, composing: boolean) => void;
  commitText: (value: string) => void;
  searching: boolean;
  error: string | null;
  result: ResultView | null;
  /** Position of the active match in `result.ordered`; -1 for none. */
  position: number;
  /** Enter (`backwards` = Shift+Enter): run the query now, or step. */
  enter: (backwards: boolean) => void;
  step: (delta: number) => void;
  jumpTo: (position: number) => void;
  /**
   * The same jump to any item, for a caller that is not searching: page it
   * in, scroll to it and flash it, with no terms to highlight. The way back
   * to a visit's first unread message (`first-unread-view.tsx`) is this.
   * `onSettled` hears how it ended; the search bar hears nothing of it.
   */
  jumpToItem: (index: number, onSettled?: (outcome: JumpOutcome) => void) => void;
  /** A search's jump paging history in, `loaded` of about `total` pages. */
  paging: { loaded: number; total: number } | null;
  /** The same for a `jumpToItem` jump, for its caller to draw. */
  itemPaging: { loaded: number; total: number } | null;
  /** Stop the jump in flight, whoever asked for it. */
  cancelJump: () => void;
  /** A search's jump could not reach its match. */
  jumpFailed: boolean;
}

/**
 * The search a session view owns. Results are fetched on a pause in typing,
 * never mid-composition, and never because the session moved on: a live
 * session's new items do not move the reader, and the count is refreshed when
 * they press Enter or type again.
 */
export function useSessionSearch(host: SearchHost): SessionSearch {
  const api = useApi();
  const [open, setOpen] = useState(false);
  const [text, setTextState] = useState("");
  // The text as of the last keystroke outside an IME composition: what the
  // debounce below searches. Composition text is the IME's, not the user's.
  const [committed, setCommitted] = useState("");
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ResultView | null>(null);
  const [position, setPosition] = useState(-1);
  const [jumpStatus, dispatchJump] = useReducer(jumpStatusReducer, JUMP_IDLE);
  // Bumped to re-run the jump effect and the painter when nothing they read
  // from React state changed — a page that was refused, a landing.
  const [jumpTick, setJumpTick] = useState(0);
  const [paintTick, setPaintTick] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const hostRef = useRef(host);
  useLayoutEffect(() => {
    hostRef.current = host;
  });
  const abortRef = useRef<AbortController | null>(null);
  /** The query of the newest request: what the debounce need not send again. */
  const requestedRef = useRef<string | null>(null);
  const jumpRef = useRef<Jump | null>(null);
  const resultRef = useRef<ResultView | null>(null);
  const positionRef = useRef(-1);
  useLayoutEffect(() => {
    resultRef.current = result;
    positionRef.current = position;
  });
  /** The item index of the active match, which survives a re-query. */
  const activeIndex = result && position >= 0 ? (result.ordered[position]?.index ?? null) : null;
  const activeIndexRef = useRef<number | null>(null);
  useLayoutEffect(() => {
    activeIndexRef.current = activeIndex;
  });

  /** End the jump in flight, telling whoever asked for it how. */
  const settleJump = useCallback((outcome: JumpOutcome) => {
    const jump = jumpRef.current;
    if (!jump) return;
    jumpRef.current = null;
    dispatchJump({ type: outcome === "landed" ? "land" : outcome === "failed" ? "fail" : "cancel" });
    jump.onSettled?.(outcome);
  }, []);

  const cancelJump = useCallback(() => {
    if (jumpRef.current) settleJump("cancelled");
    else dispatchJump({ type: "cancel" });
  }, [settleJump]);

  const startJump = useCallback(
    (match: Jump["match"], terms: string[], owner: JumpOwner, onSettled?: (outcome: JumpOutcome) => void) => {
      // A jump replaced by another is one its caller is no longer waiting on.
      settleJump("cancelled");
      const held = hostRef.current.window;
      jumpRef.current = {
        match,
        terms,
        owner,
        onSettled,
        pagesLoaded: 0,
        budget: pageInBudget(match.index, held?.startIndex ?? 0),
        inflight: false,
        aheadSince: null,
      };
      dispatchJump({ type: "start", owner });
      setJumpTick((n) => n + 1);
    },
    [settleJump],
  );

  const runSearch = useCallback(
    async (query: string, mode: "jump" | "keep") => {
      const q = query.trim();
      abortRef.current?.abort();
      abortRef.current = null;
      requestedRef.current = q;
      if (!q) {
        setResult(null);
        setSearching(false);
        setError(null);
        setPosition(-1);
        cancelJump();
        return;
      }
      const { source, id } = hostRef.current;
      if (!isSessionSource(source)) return;
      const controller = new AbortController();
      abortRef.current = controller;
      setSearching(true);
      setError(null);
      try {
        const res = await api.GET("/api/sessions/{source}/{id}/search", {
          params: { path: { source, id }, query: { q, limit: SEARCH_LIMIT } },
          signal: controller.signal,
        });
        if (controller.signal.aborted) return;
        setSearching(false);
        if (res.error || !res.data) {
          setError(res.error?.error ?? "empty response");
          return;
        }
        const ordered = newestFirst(res.data.matches);
        const view: ResultView = {
          query: q,
          terms: searchTermsOf(q),
          ordered,
          total: res.data.total,
          truncated: res.data.truncated,
          atTotal: hostRef.current.window?.totalItems ?? 0,
        };
        setResult(view);
        const next = mode === "keep" ? positionNear(ordered, activeIndexRef.current) : ordered.length > 0 ? 0 : -1;
        setPosition(next);
        // A fresh query lands on its newest match, the one nearest the bottom
        // the reader is usually at; a refresh leaves them where they are.
        if (mode === "jump" && next >= 0) startJump(ordered[next]!, view.terms, "search");
        else if (next < 0) cancelJump();
      } catch (err) {
        if (controller.signal.aborted) return;
        setSearching(false);
        setError(errorText(err));
      }
    },
    [cancelJump, startJump],
  );

  // Search on a pause in typing.
  useEffect(() => {
    if (!open) return;
    const handle = setTimeout(() => {
      if (committed.trim() !== requestedRef.current) void runSearch(committed, "jump");
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(handle);
  }, [open, committed, runSearch]);

  // The box is focused (and its text selected, so typing replaces the last
  // query) at once when it is already on screen, and by the effect below on
  // the commit that mounts it when it is not.
  const focusPending = useRef(false);
  const focusInput = useCallback(() => {
    const input = inputRef.current;
    if (!input) {
      focusPending.current = true;
      return;
    }
    focusPending.current = false;
    input.focus();
    input.select();
  }, []);
  useEffect(() => {
    if (open && focusPending.current) focusInput();
  }, [open, focusInput]);

  const openBar = useCallback(() => {
    setOpen(true);
    dispatchJump({ type: "open" });
    focusInput();
  }, [focusInput]);

  const close = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    requestedRef.current = null;
    setOpen(false);
    setResult(null);
    setSearching(false);
    setError(null);
    setPosition(-1);
    cancelJump();
    dispatchJump({ type: "close" });
  }, [cancelJump]);

  const setText = useCallback((value: string, composing: boolean) => {
    setTextState(value);
    if (!composing) setCommitted(value);
  }, []);
  const commitText = useCallback((value: string) => {
    setTextState(value);
    setCommitted(value);
  }, []);

  const step = useCallback(
    (delta: number) => {
      const view = resultRef.current;
      if (!view || view.ordered.length === 0) return;
      const next = stepMatch(positionRef.current, view.ordered.length, delta);
      setPosition(next);
      startJump(view.ordered[next]!, view.terms, "search");
    },
    [startJump],
  );

  const jumpTo = useCallback(
    (at: number) => {
      const view = resultRef.current;
      const match = view?.ordered[at];
      if (!view || !match) return;
      setPosition(at);
      startJump(match, view.terms, "search");
    },
    [startJump],
  );

  const jumpToItem = useCallback(
    (index: number, onSettled?: (outcome: JumpOutcome) => void) => startJump({ index, field: "text" }, [], "item", onSettled),
    [startJump],
  );

  const enter = useCallback(
    (backwards: boolean) => {
      const q = committed.trim();
      const view = resultRef.current;
      if (!q) return;
      // Typed faster than the debounce: this Enter is the search.
      if (!view || view.query !== q) {
        void runSearch(q, "jump");
        return;
      }
      // Newest first, so "next" is older — up the transcript.
      step(backwards ? -1 : 1);
      // A live session may have said it again since; recount, staying put.
      const total = hostRef.current.window?.totalItems ?? view.atTotal;
      if (total !== view.atTotal) void runSearch(q, "keep");
    },
    [committed, runSearch, step],
  );

  // ---------------------------------------------------------------- the jump

  /** Scroll to the jump's row and flash it; false when there is no row to land on. */
  const land = useCallback((jump: Jump): boolean => {
    const { rowsRef, scrollRef, window: held } = hostRef.current;
    const rows = rowsRef.current;
    const scroller = scrollRef.current;
    if (!rows || !scroller || !held) return false;
    const el = rowFor(rows, renderedIndexFor(held.items, held.startIndex, jump.match.index));
    if (!el) return false;
    openFoldsFor(el, jump.terms, jump.match.field);
    hostRef.current.unpin();
    const hit = firstHitNode(el, jump.terms);
    let rect = el.getBoundingClientRect();
    if (hit) {
      const [first] = hitRanges(hit.data, jump.terms);
      if (first) {
        const range = document.createRange();
        range.setStart(hit, first[0]);
        range.setEnd(hit, first[1]);
        const hitRect = range.getBoundingClientRect();
        if (hitRect.height > 0) rect = hitRect;
      }
    }
    centerOn(scroller, rect);
    el.classList.remove("rh-search-flash");
    // Restart the animation on a row that is flashed twice in a row.
    void el.offsetWidth;
    el.classList.add("rh-search-flash");
    setTimeout(() => el.classList.remove("rh-search-flash"), FLASH_MS);
    setPaintTick((n) => n + 1);
    return true;
  }, []);

  const loaded = host.window;
  useEffect(() => {
    const jump = jumpRef.current;
    if (!jump || !loaded || jump.inflight) return;
    const fail = () => settleJump("failed");
    // A page this jump asked for did not come.
    if (host.historyError && jump.pagesLoaded > 0 && !host.isLoadingEarlier) {
      fail();
      return;
    }
    const next = planPageIn({
      target: jump.match.index,
      window: loaded,
      loading: host.isLoadingEarlier,
      pagesLoaded: jump.pagesLoaded,
      budget: jump.budget,
    });
    switch (next.kind) {
      case "ready":
        settleJump(land(jump) ? "landed" : "failed");
        return;
      case "wait":
        return;
      case "ahead": {
        jump.aheadSince ??= Date.now();
        const timer = setTimeout(
          () => {
            if (jumpRef.current === jump) fail();
          },
          Math.max(0, AHEAD_WAIT_MS - (Date.now() - jump.aheadSince)),
        );
        return () => clearTimeout(timer);
      }
      case "load": {
        const request = host.loadEarlier();
        if (!request) return;
        jump.inflight = true;
        jump.pagesLoaded += 1;
        dispatchJump({ type: "page", loaded: jump.pagesLoaded, total: jump.pagesLoaded + next.pagesLeft - 1 });
        void request.finally(() => {
          jump.inflight = false;
          // Re-plan even when the window did not move: a refused page leaves
          // it exactly where it was, and nothing else would wake this up.
          if (jumpRef.current === jump) setJumpTick((n) => n + 1);
        });
        return;
      }
      case "fail":
        fail();
        return;
    }
    // `host.loadEarlier` is read fresh on every run and deliberately not a
    // dependency: it changes identity with every window, which the window
    // fields already say.
  }, [
    jumpTick,
    loaded?.startIndex,
    loaded?.endIndex,
    loaded?.hasMoreBefore,
    loaded?.items,
    host.isLoadingEarlier,
    host.historyError,
    land,
    settleJump,
  ]);

  // ------------------------------------------------------------ highlights

  // One identity per view for the shared highlight registry.
  const [owner] = useState(() => ({}));
  const paintTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const paintNowRef = useRef<() => void>(() => {});
  useLayoutEffect(() => {
    paintNowRef.current = () => {
      const rows = hostRef.current.rowsRef.current;
      const view = resultRef.current;
      const held = hostRef.current.window;
      if (!rows) return;
      for (const marked of rows.querySelectorAll("[data-search-active]")) marked.removeAttribute("data-search-active");
      if (!open || !view || view.terms.length === 0) {
        paint(owner, [], []);
        return;
      }
      const index = activeIndexRef.current;
      const active = index !== null && held ? rowFor(rows, renderedIndexFor(held.items, held.startIndex, index)) : null;
      active?.setAttribute("data-search-active", "");
      // The active row first, so the cap can never cost it its highlight.
      const activeRanges = active ? rangesIn(textNodes(active), view.terms, MAX_PAINTED_RANGES) : [];
      const hits = rangesIn(textNodes(rows, active), view.terms, MAX_PAINTED_RANGES - activeRanges.length);
      paint(owner, hits, activeRanges);
    };
  });
  // Throttled rather than debounced: a live session re-renders the window
  // twice a second, and a debounce would never let the paint happen. The
  // timer reads whatever is current when it fires.
  useEffect(() => {
    if (paintTimer.current !== null) return;
    paintTimer.current = setTimeout(() => {
      paintTimer.current = null;
      paintNowRef.current();
    }, 80);
  }, [open, result, activeIndex, loaded?.items, paintTick]);
  useEffect(
    () => () => {
      if (paintTimer.current !== null) clearTimeout(paintTimer.current);
      paint(owner, [], []);
    },
    [owner],
  );

  return {
    open,
    openBar,
    close,
    inputRef,
    text,
    setText,
    commitText,
    searching,
    error,
    result,
    position,
    enter,
    step,
    jumpTo,
    jumpToItem,
    paging: jumpStatus.owner === "search" ? jumpStatus.paging : null,
    itemPaging: jumpStatus.owner === "item" ? jumpStatus.paging : null,
    cancelJump,
    jumpFailed: jumpStatus.failed,
  };
}

// ----------------------------------------------------------------------- bar

function MatchIcon({ match }: { match: SearchMatch }) {
  const { t } = useTranslation();
  const [Icon, label]: [LucideIcon, string] =
    match.kind === "tool_use" || match.kind === "tool_result"
      ? [Wrench, t("tool")]
      : match.kind === "thinking"
        ? [Brain, t("thinking")]
        : match.role === "user"
          ? [User, t("user message")]
          : match.role === "system"
            ? [Info, t("system message")]
            : [Bot, t("agent message")];
  return (
    <span role="img" aria-label={label} title={label} className="mt-0.5 shrink-0 text-faint">
      <Icon aria-hidden className="size-3" />
    </span>
  );
}

/** A snippet with its hits in `<mark>`, cut where the server said they are. */
export function Snippet({ text, ranges }: { text: string; ranges: readonly (readonly number[])[] }) {
  return (
    <>
      {highlightSegments(text, ranges).map((segment, i) =>
        segment.hit ? (
          <mark key={i} className="rounded-sm bg-warn/30 text-fg">
            {segment.text}
          </mark>
        ) : (
          <span key={i}>{segment.text}</span>
        ),
      )}
    </>
  );
}

const ICON_BUTTON =
  "flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg disabled:pointer-events-none disabled:opacity-40";

/** The header button that opens the bar — the way in without a keyboard. */
export function SearchButton({ onClick }: { onClick: () => void }) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      title={t("search in this session")}
      aria-label={t("search in this session")}
      onClick={onClick}
      className={ICON_BUTTON}
    >
      <Search className="size-3.5" />
    </button>
  );
}

export function SessionSearchBar({ search }: { search: SessionSearch }) {
  const { t } = useTranslation();
  const [listOpen, setListOpen] = useState(false);
  const composing = useRef(false);
  const listRef = useRef<HTMLUListElement>(null);
  const { result, position } = search;
  const count = result?.ordered.length ?? 0;

  // The active entry in sight whenever the list opens or the reader steps.
  useEffect(() => {
    if (!listOpen) return;
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [listOpen, position]);

  if (!search.open) return null;

  const counter = result
    ? count > 0
      ? `${position + 1}/${formatInt(result.total)}${result.truncated ? "+" : ""}`
      : "0/0"
    : "";
  return (
    <div role="search" className="relative shrink-0 border-b border-border bg-panel px-2 py-1">
      <div className="flex min-w-0 items-center gap-1">
        <Search aria-hidden className="size-3.5 shrink-0 text-faint" />
        <input
          ref={search.inputRef}
          data-session-search-input=""
          value={search.text}
          onChange={(e) => search.setText(e.currentTarget.value, composing.current)}
          onCompositionStart={() => {
            composing.current = true;
          }}
          onCompositionEnd={(e) => {
            composing.current = false;
            search.commitText(e.currentTarget.value);
          }}
          onKeyDown={(e) => {
            if (isImeKeydown(e)) return;
            if (e.key === "Enter") {
              e.preventDefault();
              search.enter(e.shiftKey);
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              search.close();
            }
          }}
          placeholder={t("search in this session")}
          aria-label={t("search in this session")}
          enterKeyHint="search"
          autoComplete="off"
          spellCheck={false}
          className="h-6 min-w-0 flex-1 bg-transparent font-mono text-xs text-fg outline-none placeholder:text-faint"
        />
        {search.searching && <Loader2 aria-hidden className="size-3 shrink-0 animate-spin text-faint" />}
        {counter && (
          <span
            aria-live="polite"
            aria-label={count > 0 ? t("match {{n}} of {{total}}", { n: position + 1, total: result!.total }) : t("no matches")}
            className="shrink-0 font-mono text-2xs tabular-nums text-faint"
          >
            {counter}
          </span>
        )}
        <button
          type="button"
          title={t("older match (Enter)")}
          aria-label={t("older match (Enter)")}
          disabled={count === 0}
          onClick={() => search.step(1)}
          className={ICON_BUTTON}
        >
          <ChevronUp className="size-3.5" />
        </button>
        <button
          type="button"
          title={t("newer match (Shift+Enter)")}
          aria-label={t("newer match (Shift+Enter)")}
          disabled={count === 0}
          onClick={() => search.step(-1)}
          className={ICON_BUTTON}
        >
          <ChevronDown className="size-3.5" />
        </button>
        <button
          type="button"
          title={t("list of matches")}
          aria-label={t("list of matches")}
          aria-expanded={listOpen}
          disabled={count === 0}
          onClick={() => setListOpen((v) => !v)}
          className={cn(ICON_BUTTON, listOpen && "bg-hover text-fg")}
        >
          <List className="size-3.5" />
        </button>
        <button
          type="button"
          title={t("close search")}
          aria-label={t("close search")}
          onClick={search.close}
          className={ICON_BUTTON}
        >
          <X className="size-3.5" />
        </button>
      </div>
      {search.paging && (
        <div className="flex items-center gap-1.5 px-1 pt-0.5 font-mono text-2xs text-faint">
          <Loader2 aria-hidden className="size-3 animate-spin" />
          <span className="min-w-0 truncate">{t("loading earlier messages…")}</span>
          {search.paging.total > 1 && (
            <span className="shrink-0 tabular-nums">
              {search.paging.loaded}/{search.paging.total}
            </span>
          )}
          <button
            type="button"
            onClick={search.cancelJump}
            className="shrink-0 rounded px-1 text-muted hover:bg-hover hover:text-fg"
          >
            {t("cancel")}
          </button>
        </div>
      )}
      {(search.error || search.jumpFailed || (result && (count === 0 || count < result.total || result.truncated))) && (
        <div
          className={cn(
            "px-1 pt-0.5 font-mono text-2xs break-words",
            search.error || search.jumpFailed ? "text-danger" : "text-faint",
          )}
        >
          {search.error
            ? t("search failed: {{msg}}", { msg: search.error })
            : search.jumpFailed
              ? t("could not reach this match")
              : count === 0
                ? t("no matches")
                : t("too many matches, only the first {{n}} are shown", { n: count })}
        </div>
      )}
      {listOpen && count > 0 && (
        <ul
          ref={listRef}
          role="listbox"
          aria-label={t("list of matches")}
          className="absolute inset-x-0 top-full z-40 max-h-[min(18rem,50vh)] overflow-y-auto border-b border-border-strong bg-bg"
        >
          {result!.ordered.map((match, at) => (
            <li key={match.index} role="option" aria-selected={at === position}>
              <button
                type="button"
                onClick={() => {
                  setListOpen(false);
                  search.jumpTo(at);
                }}
                className={cn(
                  "flex w-full min-w-0 items-start gap-1.5 border-b border-border px-2 py-1 text-left hover:bg-hover",
                  at === position && "bg-accent/10",
                )}
              >
                <MatchIcon match={match} />
                <span className="min-w-0 flex-1 font-mono text-2xs leading-4 break-words text-muted">
                  <Snippet text={match.snippet} ranges={match.ranges} />
                </span>
                {match.timestamp && (
                  <span className="shrink-0 font-mono text-[10px] whitespace-nowrap text-faint/80">
                    {formatTime(match.timestamp)}
                  </span>
                )}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
