import { useEffect, useRef, useState } from "react";

/**
 * `active`, once it has held for `delayMs`, and kept true for at least
 * `minMs` once it is: what lets a page say what a wait is about only when
 * there is one worth saying, and not blink when it ends just after. The cloud
 * bar's catch-up and a session's opening words both say theirs this way, so
 * it lives here rather than with either: the views every mode shares reach
 * cloud mode's modules only through `cloudParts()`.
 */
export function useShownAfter(active: boolean, delayMs: number, minMs: number): boolean {
  const [shown, setShown] = useState(false);
  const shownAt = useRef(0);
  useEffect(() => {
    if (active === shown) return;
    if (active) {
      const timer = setTimeout(() => {
        shownAt.current = Date.now();
        setShown(true);
      }, delayMs);
      return () => clearTimeout(timer);
    }
    const timer = setTimeout(() => setShown(false), Math.max(0, minMs - (Date.now() - shownAt.current)));
    return () => clearTimeout(timer);
  }, [active, shown, delayMs, minMs]);
  return shown;
}
