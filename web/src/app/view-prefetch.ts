/*
 * Every view a route loads on demand (`lazy-views.ts`, `router.tsx`) and the
 * terminal, fetched before they are opened.
 *
 * Split by route, the boot is small, but every view's first opening then
 * waits on its chunks: on a weak link, a blank pane for a second or more —
 * the new session view after the list, a session, settings. So once the first
 * page is drawn, the files each view is made of are fetched in the
 * background: one view at a time, its files together, each view when the page
 * is idle, the likeliest next first (`PREFETCH` in vite.config.ts). Which
 * views those are, the build finds (`build/view-prefetch.ts`). Opening a view
 * then finds them cached, by the service worker or the browser.
 *
 * Fetched, not imported: a dynamic import that fails is remembered — by
 * `lazyRouteComponent`, and by the module map of browsers that still keep a
 * failed fetch — so one lost in the background to a dropped link would break
 * the view it was for until a reload. A failed fetch leaves nothing behind:
 * the view loads as it always did when it is opened. The first view that
 * fails ends the run, and a link that asks to save data, or is as slow as
 * 2G, is left alone.
 */

/**
 * Put in by the build (`build/view-prefetch.ts`): per view, in the order to
 * fetch them, the files it needs beyond the boot set and the views before it.
 * Not defined outside a build (`vite dev`, the tests), where nothing is
 * fetched.
 */
declare const __RH_VIEW_FILES__: readonly (readonly string[])[] | undefined;

let started = false;

/**
 * Fetch the views' files, once per page load: when the router has first
 * rendered, and in cloud mode when the signed-in shell shows — the router
 * renders only with a machine in view, and a visit that starts at 「我的设备」
 * opens one from there.
 */
export function prefetchViews(): void {
  if (started) return;
  started = true;
  const views = typeof __RH_VIEW_FILES__ === "undefined" ? [] : __RH_VIEW_FILES__;
  if (views.length === 0 || savesData()) return;
  // The router renders before the browser paints: the next frame is that
  // paint, and the first view waits for idle time after it, so none of this
  // competes with the first page.
  const nextFrame: (run: () => void) => void =
    typeof requestAnimationFrame === "function" ? (run) => void requestAnimationFrame(run) : (run) => void setTimeout(run, 16);
  nextFrame(() => setTimeout(() => void fetchViews(views, { fetchFile, idle, fetched })));
}

/** Each view's files together, one view after another, each after `idle`; none after a view one of whose files failed. */
export async function fetchViews(
  views: readonly (readonly string[])[],
  io: { fetchFile: (path: string) => Promise<boolean>; idle: () => Promise<void>; fetched: (path: string) => boolean },
): Promise<void> {
  for (const files of views) {
    await io.idle();
    const results = await Promise.all(files.filter((path) => !io.fetched(path)).map(io.fetchFile));
    if (results.includes(false)) return;
  }
}

async function fetchFile(path: string): Promise<boolean> {
  try {
    const res = await fetch(path, { priority: "low" });
    // Read to the end: a cache keeps only a whole response.
    if (res.ok) await res.arrayBuffer();
    return res.ok;
  } catch {
    return false;
  }
}

function idle(): Promise<void> {
  return new Promise((resolve) => {
    if (typeof requestIdleCallback === "function") requestIdleCallback(() => resolve(), { timeout: 2000 });
    else setTimeout(resolve, 500);
  });
}

/** Already fetched by this page: a boot chunk, or a view opened before its turn came. */
function fetched(path: string): boolean {
  return performance.getEntriesByName(new URL(path, location.href).href, "resource").length > 0;
}

function savesData(): boolean {
  const link = (navigator as Navigator & { connection?: { saveData?: boolean; effectiveType?: string } }).connection;
  return link?.saveData === true || link?.effectiveType === "2g" || link?.effectiveType === "slow-2g";
}
