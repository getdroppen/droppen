import { morph, setAttr, setText } from "./motion";

/*
 * The one story every landing diagram tells, as data: a message is a fixed
 * pair, the plaintext written on one end and read on the other, and the
 * ciphertext that is all anything between them ever holds. The sender seals
 * P into C in place; the very same C rides the wire, sits in the Droppen
 * relay that passes it on (or in Droppen Hub, which hands out the sealed
 * pages) and goes into the sealed pages; the receiver opens that C into
 * that P.
 *
 * One message moves at a time. A card still holding the message before keeps
 * it, dimmed, until the next one reaches it: never blank, and never a value
 * that is not the message in flight or the one just delivered. Each diagram
 * computes what its cards show as a pure function of the loop's time (tested
 * frame by frame in test/landing-story.test.ts); the component only draws it.
 */

/** A message: its plaintext (`p`, filled in in the reader's language) and its ciphertext (`c`). */
export interface Message {
  p: string;
  c: string;
}

/** What one row shows of a message. */
export interface Shown {
  /** Which message (an index into the diagram's messages). */
  msg: number;
  /**
   * plain / cipher: the message's plaintext or ciphertext; seal: the
   * plaintext turning into the ciphertext in place (`p` 0 → 1), on the
   * sender; open: the ciphertext turning into the plaintext, on the receiver.
   */
  kind: "plain" | "cipher" | "seal" | "open";
  p?: number;
  /** Plaintext being typed: how much of it is there yet (0 → 1). */
  typed?: number;
  /** Kept from the message before, dimmed, until the next one reaches this card. */
  dim?: boolean;
  /** The padlock at the start of the row: shut while sealed, open as it opens (then fading, by `lockOpacity`). */
  lock?: "shut" | "open";
  lockOpacity?: number;
}

/** A chip of ciphertext on a wire: whose, which wire, and how far along (0 → 1). */
export interface Chip {
  msg: number;
  wire: string;
  at: number;
}

/** The text a row shows. */
export function textOf(s: Shown, messages: readonly Message[], tick = 0): string {
  const m = messages[s.msg]!;
  switch (s.kind) {
    case "plain": {
      if (s.typed === undefined) return m.p;
      const chars = [...m.p];
      return chars.slice(0, Math.max(1, Math.round(s.typed * chars.length))).join("");
    }
    case "cipher":
      return m.c;
    case "seal":
      return morph(m.p, m.c, s.p ?? 0, tick);
    case "open":
      return morph(m.c, m.p, s.p ?? 0, tick);
  }
}

type El = { current: HTMLElement | null };

/** The elements of one message row (`MessageRow` in kit.tsx, through `useRowRefs`). */
export interface RowEls {
  row: El;
  text: El;
  lock?: El;
}

/** Draw `s` into a message row, writing only what changed. */
export function drawRow(els: RowEls, s: Shown, messages: readonly Message[], tick: number): void {
  setText(els.text.current, textOf(s, messages, tick));
  setAttr(els.row.current, "data-dim", s.dim ? "true" : "false");
  setAttr(els.text.current, "data-caret", s.typed !== undefined ? "true" : "false");
  const lock = els.lock?.current;
  if (lock) {
    setAttr(lock, "data-lock", s.lock ?? "none");
    const opacity = s.lock ? (s.lockOpacity ?? 1) : 0;
    // Shut or open at full strength unless fading; the empty slot is hidden by `data-lock` alone.
    const value = !s.lock || opacity >= 1 ? "" : opacity.toFixed(2);
    if (lock.style.opacity !== value) lock.style.opacity = value;
  }
}

/** The message whose turn it is at `time`: the last to start, or (before any has) the last of the loop before. */
export function currentMessage(starts: readonly number[], time: number): number {
  let current = starts.length - 1;
  starts.forEach((start, i) => {
    if (time >= start) current = i;
  });
  return current;
}
