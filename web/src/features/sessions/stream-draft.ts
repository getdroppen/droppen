/*
 * The session websocket's `draft` frame: how far the think claude is in the
 * middle of has run, which the transcript cannot say until the think is over
 * and its API response has ended.
 *
 * The server keeps the CLI's running estimate (`server-rs/src/provider/drafts.rs`)
 * and sends it as `{"type":"draft","turn","v","blocks":[{"index":0,"kind":"thinking","thinkingTokens":N}]}`,
 * or `"blocks":[]` once nothing is being thought. A page shows it in the
 * activity bar above the composer (`turn-status.tsx`), and nowhere else: the
 * transcript is append-only — nothing is drawn in it until it is written, and
 * nothing drawn in it changes height while the agent works.
 *
 * Drafts used to carry the agent's finished blocks as well — its words and
 * tool calls before the file had them — for a provisional bubble below the
 * last message, which appeared, grew, shrank and went away several times a
 * turn and moved everything above the reader's eye with it. An agent from
 * then still sends those blocks, beside the counter; [`thinkingOf`] reads the
 * counter out of such a frame and nothing else.
 *
 * A draft is never part of the transcript window. It carries no index and no
 * fingerprint, so nothing the window is resumed, paged, searched or counted
 * unread by can see it — it lives in its own store beside the window.
 */

/**
 * The count a `draft` frame carries: the CLI's running estimate of the
 * thinking block it is generating, in tokens, or `null` when nothing is being
 * thought — an empty draft, or a frame this client cannot read, which is never
 * an error.
 *
 * The counter is the thinking block with no transcript entry behind it yet
 * (no `uuid`) that is not finished; every other block — a finished think, a
 * paragraph, a tool call, which only an agent from before sends — is skipped.
 */
export function thinkingOf(frame: Record<string, unknown>): number | null {
  if (frame.type !== "draft" || !Array.isArray(frame.blocks)) return null;
  for (let at = frame.blocks.length - 1; at >= 0; at--) {
    const block: unknown = frame.blocks[at];
    if (typeof block !== "object" || block === null) continue;
    const { kind, uuid, done, thinkingTokens } = block as Record<string, unknown>;
    if (kind !== "thinking" || typeof uuid === "string" || done === true) continue;
    return typeof thinkingTokens === "number" && thinkingTokens >= 0 ? thinkingTokens : 0;
  }
  return null;
}

/** A session's think, as far as this page has heard. */
export interface Thinking {
  /** How far the think in flight has run, in tokens; `null` while there is none. */
  tokens: number | null;
  /**
   * When the count last moved — it started, grew, or ended — by this page's
   * clock; `null` until it has. Activity in its own right: a long think writes
   * nothing to the transcript, and without this the bar would call it silence.
   */
  movedAt: number | null;
}

const NOTHING: Thinking = { tokens: null, movedAt: null };

/**
 * One session's draft, outside React's state.
 *
 * The count moves up to a dozen times a second during a long think, and in
 * React state each change would re-render the whole session view: header,
 * composer and all. Held here, a change wakes the one component that reads it
 * (the activity bar's words, `useSyncExternalStore`) and nothing else, and a
 * frame that says what the last one did wakes nobody.
 */
export interface DraftStore {
  /** The same object until something in it changes. */
  get(): Thinking;
  subscribe(onChange: () => void): () => void;
  /** A `draft` frame's count ([`thinkingOf`]), heard at `at`. */
  receive(tokens: number | null, at?: number): void;
  /**
   * The socket is gone: whatever is in flight is the next attach's to say.
   * When the count last moved stays true, and stays.
   */
  clear(): void;
}

export function createDraftStore(): DraftStore {
  let held: Thinking = NOTHING;
  const listeners = new Set<() => void>();
  const set = (next: Thinking) => {
    held = next;
    for (const listener of listeners) listener();
  };
  return {
    get: () => held,
    subscribe(onChange) {
      listeners.add(onChange);
      return () => listeners.delete(onChange);
    },
    receive(tokens, at = Date.now()) {
      if (tokens === held.tokens) return;
      set({ tokens, movedAt: at });
    },
    clear() {
      if (held.tokens !== null) set({ ...held, tokens: null });
    },
  };
}

/** A token count as the activity bar shows it: `~850`, `~1.2k`; at most five characters below a million. */
export function approxTokens(tokens: number): string {
  if (tokens < 1000) return `~${Math.max(0, Math.round(tokens))}`;
  const thousands = tokens / 1000;
  return `~${thousands < 10 ? thousands.toFixed(1).replace(/\.0$/, "") : Math.round(thousands)}k`;
}
