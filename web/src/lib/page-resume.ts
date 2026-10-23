/*
 * The page coming back: in view again (`visibilitychange`), shown from the
 * back-forward cache (`pageshow`), focused again (`focus`), online again
 * (`online`). A phone holds a page's timers while it sleeps or the page is in
 * the background, and may drop its sockets meanwhile: what waits on a timer —
 * a reconnect, a retry at the end of a backoff — is better done now. The
 * first three often come together (a phone unlocked): told once a second at
 * most. `online` is always told: the network may come back just after.
 */

type Target = { addEventListener?: (type: string, fn: () => void) => void; removeEventListener?: (type: string, fn: () => void) => void };

const COALESCE_MS = 1000;

export function onPageResume(fn: () => void): () => void {
  const win = globalThis as Target;
  const doc = (globalThis as { document?: Target & { visibilityState?: string } }).document;
  let last = Number.NEGATIVE_INFINITY;
  const back = () => {
    const now = Date.now();
    if (now - last < COALESCE_MS) return;
    last = now;
    fn();
  };
  const visible = () => {
    if (doc?.visibilityState !== "hidden") back();
  };
  const online = () => {
    last = Date.now();
    fn();
  };
  win.addEventListener?.("pageshow", back);
  win.addEventListener?.("focus", back);
  win.addEventListener?.("online", online);
  doc?.addEventListener?.("visibilitychange", visible);
  return () => {
    win.removeEventListener?.("pageshow", back);
    win.removeEventListener?.("focus", back);
    win.removeEventListener?.("online", online);
    doc?.removeEventListener?.("visibilitychange", visible);
  };
}
