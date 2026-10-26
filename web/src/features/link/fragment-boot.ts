/*
 * The browser hand-off's fragments (`handoff.ts`, docs/local-aggregator.md
 * §4.9), taken out of the address before anything else reads it: `main.tsx`
 * imports this first, so the router, the service worker and every later
 * history entry see the bare path.
 *
 * - `#droppen-link=…` on droppen.app's `/link`: the sign-in a device's local
 *   page asked for. Kept for this tab only (`sessionStorage`) — signing in or
 *   up on the way may reload the page — and forgotten once it is answered or
 *   given up ({@link forgetHandoff}).
 * - `#droppen-return=…` on the local page's `/link/return`: droppen.app's
 *   sealed answer. Kept in memory only, and handed out once ({@link takeReturn}).
 */
import { type HandoffRequest, handoffOf, handoffPayload, HANDOFF_FRAGMENT, parseHandoff, parseReturn, RETURN_FRAGMENT } from "./handoff";

/** Where this tab keeps the hand-off it is answering. */
export const HANDOFF_STORE_KEY = "droppen.link.handoff";

/** The few browser objects this needs (a test passes fakes). */
export interface FragmentEnv {
  location: { pathname: string; search: string; hash: string };
  history: { state: unknown; replaceState(state: unknown, unused: string, url: string): void };
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
}

let returned: { attempt: string; sealed: string } | null = null;
let handoff: HandoffRequest | null = null;

/** Take the fragments out of `env`'s address (see the module notes). */
export function captureLinkFragments(env: FragmentEnv): void {
  const hash = env.location.hash.replace(/^#/, "");
  const isHandoff = hash.startsWith(`${HANDOFF_FRAGMENT}=`);
  const isReturn = hash.startsWith(`${RETURN_FRAGMENT}=`);
  if (!isHandoff && !isReturn) return;
  if (isHandoff) {
    handoff = parseHandoff(hash);
    if (handoff) {
      try {
        env.storage?.setItem(HANDOFF_STORE_KEY, JSON.stringify(handoffPayload(handoff)));
      } catch {
        // No storage: kept for this page only.
      }
    }
  } else {
    returned = parseReturn(hash);
  }
  // Gone from the address either way, well-formed or not.
  env.history.replaceState(env.history.state, "", env.location.pathname + env.location.search);
}

/** The hand-off this tab is answering on droppen.app, if any. */
export function pendingHandoff(env: Pick<FragmentEnv, "storage">): HandoffRequest | null {
  if (handoff) return handoff;
  try {
    const kept = env.storage?.getItem(HANDOFF_STORE_KEY);
    handoff = kept ? handoffOf(JSON.parse(kept)) : null;
  } catch {
    handoff = null;
  }
  return handoff;
}

/** The hand-off is answered, or given up: this tab forgets it. */
export function forgetHandoff(env: Pick<FragmentEnv, "storage">): void {
  handoff = null;
  try {
    env.storage?.removeItem(HANDOFF_STORE_KEY);
  } catch {
    // Nothing kept.
  }
}

/** droppen.app's sealed answer this page was opened with — once. */
export function takeReturn(): { attempt: string; sealed: string } | null {
  const answer = returned;
  returned = null;
  return answer;
}

/**
 * Put `answer` back on `env`'s address, the way droppen.app sent it, for a
 * reload to take it again (`main.tsx`: the page's own chunk did not come).
 * Only for the moment of the reload: the reloaded page takes it off again
 * before anything else reads the address.
 */
export function restoreReturn(env: Pick<FragmentEnv, "location" | "history">, answer: { attempt: string; sealed: string }): void {
  env.history.replaceState(env.history.state, "", `${env.location.pathname}${env.location.search}#${RETURN_FRAGMENT}=${answer.attempt}.${answer.sealed}`);
}

/** The browser's own objects, when there is a browser. */
export function browserEnv(): FragmentEnv | null {
  if (typeof window === "undefined") return null;
  let storage: Storage | null = null;
  try {
    storage = window.sessionStorage;
  } catch {
    storage = null;
  }
  return { location: window.location, history: window.history, storage };
}

const env = browserEnv();
if (env) captureLinkFragments(env);
