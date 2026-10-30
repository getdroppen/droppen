import type { components } from "../../api/schema.js";

export type TranscriptItem = components["schemas"]["TranscriptItem"];

/*
 * The session websocket's transcript frames. They are not in the OpenAPI
 * contract (the socket route is not documented there), so their shape lives
 * here, next to the code that reads them; the server side is the module docs of
 * `server-rs/crates/droppen-drive/src/feed/mod.rs`.
 *
 * - `snapshot` (attach) and `reset`: a fresh window — `items`, `startIndex`,
 *   `endIndex`, `totalItems`, `hasMoreBefore`, `sig` — plus `prefixFp` and
 *   `startFp`. A snapshot also carries the whole session state.
 * - `delta`: `fromIndex`, `items`, `endIndex`, `totalItems`, `sig`, `prefixFp`.
 *   Replace-from: keep what is held before `fromIndex`, put `items` there; they
 *   always run to the end of the transcript.
 * - A server from before may add `agentUuids` beside the items of all three
 *   (the agent entries they came from, which a provisional bubble was matched
 *   against). Nothing reads it any more.
 *
 * `prefixFp` is the server's fingerprint of items `[0, endIndex - K)` (K is a
 * small overlap the server owns), and it is the resume marker: a reconnect sends
 * `?since=<endIndex>&fp=<prefixFp>` and is answered with a delta from
 * `endIndex - K` — the window, its earlier pages and the scroll position all
 * survive — or, when the transcript under it changed, a snapshot. `startFp` is
 * the fingerprint of `[0, startIndex)`, which "load earlier" proves its page
 * against. A delta that cannot be placed is answered by sending
 * `{"type":"resync","since":…,"fp":…}` up the socket (see `resyncMessage`).
 */

/** A contiguous, absolute-indexed slice of a session transcript. */
export interface TranscriptWindow {
  sig: string;
  items: TranscriptItem[];
  /** Inclusive absolute index of the first loaded item. */
  startIndex: number;
  /** Exclusive absolute index after the last loaded item. */
  endIndex: number;
  /** Newest total reported by any accepted feed message. */
  totalItems: number;
  hasMoreBefore: boolean;
  /**
   * The resume marker: the server's fingerprint of `[0, endIndex - K)`. `null`
   * when the server sent none, which only costs the next reconnect a snapshot.
   */
  prefixFp: string | null;
  /**
   * The server's fingerprint of `[0, startIndex)`; `null` once unknown — the
   * window was trimmed, or replaced from before its start by a delta.
   */
  startFp: string | null;
}

/** One older, contiguous transcript page returned by the server. */
export interface TranscriptHistoryPage {
  sig: string;
  items: TranscriptItem[];
  startIndex: number;
  endIndex: number;
  totalItems: number;
  hasMoreBefore: boolean;
  /** The fingerprint of `[0, startIndex)`; absent from a server that predates it. */
  startFp?: string | null;
}

/** A replace-from update: everything from `fromIndex` on is `items`. */
export interface TranscriptDelta {
  sig: string;
  items: TranscriptItem[];
  fromIndex: number;
  /** Where `items` end, which is the end of the transcript. */
  endIndex: number;
  totalItems: number;
  prefixFp: string | null;
}

function stringOrNull(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

/** The two fingerprints a `snapshot` or `reset` frame carries. */
export function resumeMarksOf(
  frame: Record<string, unknown>,
): Pick<TranscriptWindow, "prefixFp" | "startFp"> {
  return { prefixFp: stringOrNull(frame.prefixFp), startFp: stringOrNull(frame.startFp) };
}

/**
 * The window a `reset` (or `snapshot`) frame replaces the held one with —
 * whatever that held, earlier pages included — or `null` for a frame without a
 * signature, which is not one.
 */
export function transcriptWindowOf(frame: Record<string, unknown>): TranscriptWindow | null {
  if (typeof frame.sig !== "string") return null;
  const items = Array.isArray(frame.items) ? (frame.items as TranscriptItem[]) : [];
  const number = (value: unknown): number | undefined =>
    typeof value === "number" ? value : undefined;
  const totalItems = number(frame.totalItems);
  const startIndex =
    number(frame.startIndex) ??
    number(frame.omittedItems) ??
    Math.max(0, (totalItems ?? items.length) - items.length);
  const endIndex = number(frame.endIndex) ?? startIndex + items.length;
  return {
    sig: frame.sig,
    items,
    startIndex,
    endIndex,
    totalItems: totalItems ?? endIndex,
    hasMoreBefore:
      typeof frame.hasMoreBefore === "boolean" ? frame.hasMoreBefore : startIndex > 0,
    ...resumeMarksOf(frame),
  };
}

/**
 * Read a `delta` frame. A server that predates `endIndex` only ever sent deltas
 * that ran to the end, so `totalItems` stands in for it.
 */
export function transcriptDeltaOf(frame: Record<string, unknown>): TranscriptDelta {
  const fromIndex = typeof frame.fromIndex === "number" ? frame.fromIndex : -1;
  const items = Array.isArray(frame.items) ? (frame.items as TranscriptItem[]) : [];
  const totalItems =
    typeof frame.totalItems === "number" ? frame.totalItems : fromIndex + items.length;
  return {
    sig: typeof frame.sig === "string" ? frame.sig : "",
    items,
    fromIndex,
    endIndex: typeof frame.endIndex === "number" ? frame.endIndex : totalItems,
    totalItems,
    prefixFp: stringOrNull(frame.prefixFp),
  };
}

/** `?since=…&fp=…` for a socket re-attaching while holding `window`, else `""`. */
export function resumeQuery(
  window: Pick<TranscriptWindow, "endIndex" | "prefixFp"> | null | undefined,
): string {
  if (!window?.prefixFp || !Number.isInteger(window.endIndex)) return "";
  return `?since=${window.endIndex}&fp=${encodeURIComponent(window.prefixFp)}`;
}

/**
 * The message that asks the socket for a delta that fits `window`, or — with no
 * window, or none the server could check — for a reset.
 */
export function resyncMessage(
  window: Pick<TranscriptWindow, "endIndex" | "prefixFp"> | null | undefined,
): string {
  return JSON.stringify(
    window?.prefixFp
      ? { type: "resync", since: window.endIndex, fp: window.prefixFp }
      : { type: "resync" },
  );
}

/**
 * How to ask for the page before `window`: proving the prefix it hangs off with
 * the window's start fingerprint when it is known, and with the resume marker —
 * which covers more than it needs to, but is never unknown — when it is not.
 */
export function historyQueryFor(
  window: Pick<TranscriptWindow, "startIndex" | "endIndex" | "startFp" | "prefixFp">,
): { before: number; fp?: string; since?: number } {
  if (window.startFp) return { before: window.startIndex, fp: window.startFp };
  if (window.prefixFp) {
    return { before: window.startIndex, fp: window.prefixFp, since: window.endIndex };
  }
  return { before: window.startIndex };
}

function isValidRange(
  value: Pick<TranscriptWindow, "items" | "startIndex" | "endIndex" | "totalItems">,
): boolean {
  return (
    Number.isInteger(value.startIndex) &&
    Number.isInteger(value.endIndex) &&
    Number.isInteger(value.totalItems) &&
    value.startIndex >= 0 &&
    value.endIndex >= value.startIndex &&
    value.totalItems >= value.endIndex &&
    value.items.length === value.endIndex - value.startIndex
  );
}

/**
 * Put an older page in front of the window.
 *
 * A page with a `startFp` was proven by the server to hang off the very prefix
 * the window does (the request carried the window's fingerprint), so it is
 * placed by index alone and the session growing in the meantime is no reason
 * to refuse it. One from a server that predates fingerprints still has to come
 * from the same whole-file signature.
 */
export function prependHistoryPage(
  current: TranscriptWindow,
  page: TranscriptHistoryPage,
): TranscriptWindow {
  if (
    !isValidRange(page) ||
    (!page.startFp && page.sig !== current.sig) ||
    page.hasMoreBefore !== (page.startIndex > 0) ||
    page.endIndex !== current.startIndex
  ) {
    return current;
  }

  return {
    sig: current.sig,
    items: [...page.items, ...current.items],
    startIndex: page.startIndex,
    endIndex: current.endIndex,
    totalItems: Math.max(current.totalItems, page.totalItems),
    hasMoreBefore: page.hasMoreBefore,
    prefixFp: current.prefixFp,
    startFp: page.startFp ?? null,
  };
}

/** Keep the same visible content after older rows increase the scroll height. */
export function scrollTopAfterPrepend(
  currentScrollTop: number,
  previousScrollHeight: number,
  nextScrollHeight: number,
): number {
  return currentScrollTop + (nextScrollHeight - previousScrollHeight);
}

/**
 * Apply a replace-from delta, or answer `null` when it cannot be placed.
 *
 * `null` is never "ignore it": the caller asks the server for a delta that fits
 * (`resyncMessage`), because a dropped delta is a transcript that silently
 * stops matching the session. It means the delta starts beyond the window's end
 * — a gap the client cannot fill — or is malformed.
 *
 * Placed, everything the window holds before `fromIndex` stays, by reference,
 * and the delta's items replace the rest: an item the server re-sent because it
 * changed in place (the overlap of a resume, a tool result attaching to its
 * call) lands on top of its old self instead of next to it. A delta that starts
 * before the window does replaces the window outright, and the start
 * fingerprint goes unknown with it.
 */
export function applyTranscriptDelta(
  current: TranscriptWindow,
  delta: TranscriptDelta,
): TranscriptWindow | null {
  if (
    !Number.isInteger(delta.fromIndex) ||
    !Number.isInteger(delta.endIndex) ||
    !Number.isInteger(delta.totalItems) ||
    delta.fromIndex < 0 ||
    delta.endIndex < delta.fromIndex ||
    delta.totalItems < delta.endIndex ||
    delta.items.length !== delta.endIndex - delta.fromIndex ||
    delta.fromIndex > current.endIndex
  ) {
    return null;
  }
  const sig = delta.sig || current.sig;
  if (delta.fromIndex < current.startIndex) {
    return {
      sig,
      items: delta.items,
      startIndex: delta.fromIndex,
      endIndex: delta.endIndex,
      totalItems: delta.totalItems,
      hasMoreBefore: delta.fromIndex > 0,
      prefixFp: delta.prefixFp,
      startFp: null,
    };
  }
  const keep = delta.fromIndex - current.startIndex;
  const items =
    keep === current.items.length
      ? delta.items.length === 0
        ? current.items
        : [...current.items, ...delta.items]
      : [...current.items.slice(0, keep), ...delta.items];
  return {
    ...current,
    sig,
    items,
    endIndex: delta.endIndex,
    totalItems: delta.totalItems,
    prefixFp: delta.prefixFp,
  };
}

/**
 * How many items a window that has been trimmed is left holding.
 *
 * The transcript is deliberately not virtualized and its folds keep their
 * bodies in the DOM (see `index.css` and the `transcript-rows` div), so the
 * cost of every render — and of every layout the browser does afterwards —
 * grows linearly with the loaded window. That was fine while the window was the
 * page the server hands over on attach, and stopped being fine once a pane left
 * open on a working session was found holding four thousand items: nothing
 * above trims, so the window walks from 200 towards the session's full length
 * and keeps going.
 *
 * 800 is picked to be far more than a viewport and far less than a session.
 * The tightest row this renders is about 20px, so 800 of them is ~16,000px of
 * content: seven screens on a 4K display, thirty on a phone. The reader
 * following the tail can therefore never scroll far enough to reach the cut
 * before the next trim, which is the requirement — a trim must be invisible.
 * It is also four times `SESSION_PAGE_SIZE`, the page the server considers a
 * reasonable amount of transcript to hand over at once, and ten times
 * `SUBTASK_LOOKBACK`, so the live-work scan behind the floating panel still
 * sees every item it has ever looked at.
 */
export const LIVE_WINDOW_CAP = 800;

/**
 * How far past the cap a window may grow before it is trimmed back to it.
 *
 * Hysteresis, so that trimming is a thing that happens once per page of
 * arrivals rather than once per delta: a delta can be a single streamed line,
 * and re-slicing an 800-element array on each one would be its own steady
 * burn. One `SESSION_PAGE_SIZE` of slack means at most 1000 items are ever in
 * the DOM and at most one array copy per 200 that arrive.
 */
export const LIVE_WINDOW_SLACK = 200;

/**
 * Drop the oldest items from a window that has outgrown {@link LIVE_WINDOW_CAP}.
 *
 * Only ever called on a window whose view is pinned to the newest row, and
 * never while a request for an earlier page is in flight — the caller owns both
 * of those conditions, and both matter. Trimming an unpinned view would yank
 * the scroll out from under a reader who had gone back through the history;
 * trimming during a `loadEarlier` would move `startIndex` out from under the
 * `before` the request was sent with, and `prependHistoryPage` would then
 * refuse the answer as no longer adjacent.
 *
 * What makes this safe under a *pinned* view is the pin's own definition: the
 * content shrinking at the top makes the browser clamp `scrollTop`, the clamp
 * lands exactly on the bottom, and `pinAfterScroll` reads a clamp to the bottom
 * as keeping the pin. The layout effect then re-sticks against the new height,
 * so the newest row does not move on screen at all.
 *
 * The indices stay consistent by construction — the items removed are removed
 * from the front, so `startIndex` moves by exactly as many, `endIndex` and
 * `totalItems` do not move at all, and `items.length === endIndex - startIndex`
 * still holds. Which in turn means "load earlier history" keeps working: it
 * asks for the page ending at `startIndex`, which is now the cut, and
 * `prependHistoryPage` accepts a page that meets it. `endIndex` not moving is
 * also what keeps the unread cursor honest, since that counts growth at the end
 * and is blind to anything that happens at the start.
 *
 * A trim can orphan a `tool_result` whose `tool_use` fell off the front, which
 * renders it as a standalone row instead of folding it into the call. That is
 * not new: the window the server hands over on attach starts mid-session and
 * has always been able to do the same, which is what the "…N earlier steps
 * omitted" line above the transcript is for.
 */
export function trimLiveWindow(current: TranscriptWindow): TranscriptWindow {
  if (current.items.length <= LIVE_WINDOW_CAP + LIVE_WINDOW_SLACK) return current;
  const dropped = current.items.length - LIVE_WINDOW_CAP;
  const startIndex = current.startIndex + dropped;
  return {
    ...current,
    items: current.items.slice(dropped),
    startIndex,
    // Always true, since something was dropped — written as the derivation
    // rather than as `true` so it cannot drift from what the field means.
    hasMoreBefore: startIndex > 0,
    // Nobody told the client the fingerprint at the cut; "load earlier" proves
    // its page with the resume marker instead (`historyQueryFor`).
    startFp: null,
  };
}

/**
 * The state half of a session, exactly as a `state` frame carries it.
 *
 * The server's `SessionState` (`server-rs/crates/droppen-drive/src/feed/mod.rs`) serializes to these
 * fields and nothing else, and a `state` frame is that object with a `type`
 * glued on — so this is also the complete list of what such a frame is allowed
 * to change on the client. `rotation` is the one it may leave out: a server
 * that only reports it on the snapshot and the REST detail keeps the value
 * those established. `questions` and `receipts` may be missing too, from a
 * server older than either.
 */
export type SessionStateFields = Pick<
  components["schemas"]["SessionDetail"],
  | "summary"
  | "injection"
  | "canSend"
  | "sendBlockReason"
  | "cliChoice"
  | "contextTokens"
  | "contextWindow"
  | "claudeRoot"
  | "account"
  | "rotation"
  | "receipts"
  | "questions"
>;

/**
 * Structural equality over two values that came out of `JSON.parse`.
 *
 * Deliberately not a `JSON.stringify` comparison: that answers "were these
 * serialized the same way", which is only the same question while both sides
 * happen to have the same key order, and it allocates two strings the size of
 * the value every time it is asked. This walks and stops at the first
 * difference, so the common case — a heartbeat whose first field already
 * differs, or a value that is identical by reference — costs almost nothing.
 *
 * Only sound for JSON: no cycles, no `Date`, no `Map`, no `undefined` members.
 * Everything it is used on here arrives from `JSON.parse` of a websocket frame
 * or from the generated API client, which is the same shape.
 */
export function sameJson(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((entry, index) => sameJson(entry, b[index]));
  }
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left);
  if (keys.length !== Object.keys(right).length) return false;
  return keys.every(
    (key) => Object.hasOwn(right, key) && sameJson(left[key], right[key]),
  );
}

/** The value already held, whenever the new one says the same thing. */
function held<V>(previous: V, next: V): V {
  return sameJson(next, previous) ? previous : next;
}

/**
 * Fold a `state` frame into the state already held, changing nothing it did not.
 *
 * Returns `previous` **by identity** when the frame says nothing new, which is
 * the whole point: `data` is the dependency of every memo in the session view,
 * so a fresh object for an unchanged state re-renders the entire transcript —
 * which is deliberately not virtualized, so that is every row in the window,
 * up to `LIVE_WINDOW_CAP` of them. The server already suppresses a frame whose
 * *serialization* is unchanged (`push_state` in `server-rs/crates/droppen-drive/src/feed/mod.rs`),
 * so what reaches here is frames that differ somewhere; the reason to check
 * again is what they usually differ in. A working session rewrites its
 * transcript file constantly, and `summary.sizeBytes` and `summary.updatedAt`
 * move with every write, so most of those frames carry a different byte count
 * and an identical everything-else. Holding each field's previous value where
 * it did not change keeps the identities the memos below are keyed on — the
 * pairing, the resident task set, the chunking — so a frame that only moved
 * the byte count no longer invalidates any of them.
 *
 * `undefined` and `null` are different answers here, and collapsing them is how
 * the composer used to lock itself: the server always *sends* these fields
 * (they serialize as `null`), so an absent one means a server too old to have
 * it, and the value the snapshot established is the best one left. A `null`
 * that really did arrive still clears the field — that is a run ending, and it
 * must not be sticky. The `??` fields are the ones where the two mean the same
 * thing because `null` is not a value they take.
 */
export function applyStateFrame<T extends SessionStateFields>(
  previous: T,
  frame: Partial<SessionStateFields>,
): T {
  const summary = held(previous.summary, frame.summary ?? previous.summary);
  const injection = held(
    previous.injection,
    frame.injection !== undefined ? frame.injection : previous.injection,
  );
  const canSend = frame.canSend !== undefined ? frame.canSend : previous.canSend;
  const sendBlockReason =
    frame.sendBlockReason !== undefined ? frame.sendBlockReason : previous.sendBlockReason;
  const cliChoice = held(previous.cliChoice, frame.cliChoice ?? previous.cliChoice);
  // A state frame from a server that predates the field must not wipe a reading
  // the snapshot already established.
  const contextTokens =
    frame.contextTokens !== undefined ? frame.contextTokens : previous.contextTokens;
  const contextWindow = frame.contextWindow ?? previous.contextWindow;
  const claudeRoot = frame.claudeRoot !== undefined ? frame.claudeRoot : previous.claudeRoot;
  const account = held(
    previous.account,
    frame.account !== undefined ? frame.account : previous.account,
  );
  const rotation = held(
    previous.rotation,
    frame.rotation !== undefined ? frame.rotation : previous.rotation,
  );
  // What became of the messages this session was sent under a client id. A
  // server too old to send the field leaves whatever the snapshot said.
  const receipts = held(previous.receipts, frame.receipts ?? previous.receipts);
  // The questions the agent asked and what became of them. Held like the
  // receipts, and for a sharper reason: the question cards in the transcript
  // look themselves up in this list through a context, so a fresh array on
  // every frame would wake every one of them twice a second. A server too old
  // to send it leaves what the snapshot said — an empty list, there.
  const questions = held(previous.questions, frame.questions ?? previous.questions);

  if (
    summary === previous.summary &&
    injection === previous.injection &&
    canSend === previous.canSend &&
    sendBlockReason === previous.sendBlockReason &&
    cliChoice === previous.cliChoice &&
    contextTokens === previous.contextTokens &&
    contextWindow === previous.contextWindow &&
    claudeRoot === previous.claudeRoot &&
    account === previous.account &&
    rotation === previous.rotation &&
    receipts === previous.receipts &&
    questions === previous.questions
  ) {
    return previous;
  }

  // The cast is what a spread over a generic costs: TypeScript types
  // `{ ...previous, summary }` as a fresh object rather than as `T`, and every
  // field named here is one of `T`'s own, carrying `T`'s own type.
  return {
    ...previous,
    summary,
    injection,
    canSend,
    sendBlockReason,
    cliChoice,
    contextTokens,
    contextWindow,
    claudeRoot,
    account,
    // Only written when there is one to write, so a state that never had a
    // rotation does not grow an `undefined` member it did not have.
    ...(rotation !== undefined ? { rotation } : {}),
    ...(receipts !== undefined ? { receipts } : {}),
    ...(questions !== undefined ? { questions } : {}),
  } as T;
}
