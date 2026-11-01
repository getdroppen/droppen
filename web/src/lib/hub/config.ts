/*
 * Cloud mode: whether this page was served by a remote-hand hub.
 *
 * The hub writes `<meta name="droppen-hub" content="<its origin>">` into the
 * shell it serves (docs/hub-api/http.md); a remote-hand server's shell has no such
 * tag, so a page it serves never takes any of the cloud paths and behaves
 * exactly as it always has. The marker was named `rh-hub` before the rename;
 * a hub that has not switched yet still writes it, so both are read and a
 * shell it serves (or one the browser cached) still enters cloud mode.
 */

/** The marker names, newest first: `rh-hub` may go once every hub writes `droppen-hub`. */
const MARKERS = ['meta[name="droppen-hub"]', 'meta[name="rh-hub"]'] as const;

/** The hub's origin (`wss://…`) when the page runs in cloud mode, else `null`. */
export function hubOrigin(doc: Pick<Document, "querySelector"> | undefined = globalThis.document): string | null {
  for (const marker of MARKERS) {
    const content = doc?.querySelector(marker)?.getAttribute("content");
    if (content && /^wss?:\/\/[^/]+$/.test(content)) return content;
  }
  return null;
}

/**
 * The hub's HTTP API origin: the `https://` form of the marker
 * (`wss://api.droppen.app` → `https://api.droppen.app`, `ws://…` → `http://…`),
 * or `null` on a page no hub serves. The marker stays the one source of truth
 * for cloud mode; nothing else says where the page sends its requests.
 */
export function hubHttpOrigin(doc: Pick<Document, "querySelector"> | undefined = globalThis.document): string | null {
  const origin = hubOrigin(doc);
  return origin && origin.replace(/^ws(s?):\/\//, "http$1://");
}

export function isCloud(): boolean {
  return hubOrigin() !== null;
}
