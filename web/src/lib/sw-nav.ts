/**
 * The one message the service worker posts to the page: a request to navigate
 * inside the running SPA when a notification is tapped.
 *
 * The URL is attacker-influenceable in principle — it originates in a push
 * payload — so only a same-origin, absolute *path* is ever handed to the router.
 * Anything else (an absolute URL to another origin, a `javascript:` URI, a
 * scheme-relative `//host`, or a non-string) yields `null` and is ignored.
 */
export function internalNavPath(data: unknown): string | null {
  if (typeof data !== "object" || data === null) return null;
  const message = data as Record<string, unknown>;
  const url = message["url"];
  if (message["type"] !== "nav" || typeof url !== "string") return null;
  // A leading single slash is a same-origin path; "//host" is scheme-relative
  // and would leave the origin, so it must be rejected — and so must what a
  // URL parser reads as one: "/\host", or a tab or line break the parser drops.
  if (!url.startsWith("/") || url.startsWith("//") || /[\\\u0000-\u001f\u007f]/.test(url)) return null;
  return sameOriginPath(url) ? url : null;
}

/** Whether `path`, resolved as a browser would against this origin, stays on it (the service worker's rule for `openWindow`, `public/sw.js`). */
function sameOriginPath(path: string): boolean {
  const base = "https://droppen.invalid";
  try {
    return new URL(path, base).origin === base;
  } catch {
    return false;
  }
}

/**
 * The window event `sw-register.ts` raises with the path a tapped notification
 * navigated to. Routing to the page already on screen changes nothing, so a
 * session view listens for this to know the reader came back to it.
 */
export const ARRIVE_EVENT = "rh:arrive";

/** Whether an arrival event's path is this session's own page. */
export function arrivesAt(path: unknown, source: string, id: string): boolean {
  if (typeof path !== "string") return false;
  const bare = path.split(/[?#]/, 1)[0]!.replace(/\/+$/, "");
  return bare === `/sessions/${source}/${id}`;
}
