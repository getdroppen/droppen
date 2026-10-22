/*
 * The guide opening by itself: the first
 * route of a page load in the app, when it is the home and the server says
 * the guide never started on an install with no sessions. Asked from the
 * router's guard, once per page load, so the session list never flashes
 * first; every later navigation, and a first route that is a deep link,
 * goes where it was going.
 */
import { isDesktopApp } from "@/lib/auth";
import { LOCAL } from "@/lib/machines/key";
import { apiFor } from "@/lib/machines/registry";
import { fetchProgress } from "./api";
import { shouldAutoOpen } from "./model";
import type { SetupProgress } from "./types";

/**
 * How long the first paint waits for the answer, and a missing answer is
 * "no". A local server answers in milliseconds however many transcripts the
 * user has: whether this install owns sessions is read from its own records,
 * not from a session listing (`server-rs/src/setup/progress.rs`).
 */
const WAIT_MS = 1500;

/**
 * The once-per-page-load question, over what it needs to know: whether the
 * app runs this page's server, and the stored progress.
 */
export function createFirstRun(deps: { desktopApp: () => boolean; progress: () => Promise<SetupProgress | null> }) {
  let decided = false;
  /** Whether this navigation should go to the guide instead: only ever for the page load's first one. */
  return async function firstRun(pathname: string): Promise<boolean> {
    if (decided) return false;
    decided = true;
    const desktopApp = deps.desktopApp();
    // Not even asked outside the app, or for a deep link.
    if (!shouldAutoOpen({ desktopApp, firstLoad: true, pathname, progress: { autoOpen: true } })) return false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const progress = await Promise.race([
      deps.progress().catch(() => null),
      new Promise<null>((resolve) => (timer = setTimeout(() => resolve(null), WAIT_MS))),
    ]);
    clearTimeout(timer);
    return shouldAutoOpen({ desktopApp, firstLoad: true, pathname, progress });
  };
}

/** The page's own: the app's page is always its local server's, so the question goes to it ({@link LOCAL}). */
export const firstRunRedirect = createFirstRun({ desktopApp: isDesktopApp, progress: () => fetchProgress(apiFor(LOCAL)) });
