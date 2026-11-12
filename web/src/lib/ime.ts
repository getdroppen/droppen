import { useRef } from "react";

/**
 * True when this keydown belongs to an IME composition session — e.g. the
 * Enter that confirms a candidate — and must not trigger send/submit.
 * keyCode 229 covers Safari, where the confirming keydown can arrive after
 * compositionend has already fired.
 */
export function isImeKeydown(e: React.KeyboardEvent<Element>): boolean {
  return e.nativeEvent.isComposing || e.keyCode === 229;
}

/** The send chord: Enter, no IME, and the modifier the prefs ask for (otherwise Shift+Enter is a newline). */
export function isSendKeydown(e: React.KeyboardEvent<Element>, enterSendsModifier: boolean): boolean {
  if (e.key !== "Enter" || isImeKeydown(e)) return false;
  return enterSendsModifier ? e.metaKey || e.ctrlKey : !e.shiftKey;
}

/**
 * Tracks an active IME composition on a textarea. While composing, a
 * controlled value must only mirror the DOM (no persistence, no fan-out to
 * other subscribers): any externally-driven value rewrite aborts the
 * composition mid-way and commits the segmented preedit (e.g. macOS Pinyin's
 * "ba sh" for "bash") as literal text. onCommit fires with the final value
 * once the IME session ends.
 */
export function useImeComposition(onCommit: (value: string) => void) {
  const composingRef = useRef(false);
  return {
    isComposing: composingRef,
    props: {
      onCompositionStart: () => {
        composingRef.current = true;
      },
      onCompositionEnd: (e: React.CompositionEvent<HTMLTextAreaElement>) => {
        composingRef.current = false;
        onCommit(e.currentTarget.value);
      },
    },
  };
}
