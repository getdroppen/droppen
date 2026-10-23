/*
 * Moving this browser's device from the hub's origin to its app host
 * (`droppen.app` → `my.droppen.app`): the seam the move notice
 * (`features/hosts/legacy.tsx`) calls through, and the `/move` page both
 * hosts serve (`main.tsx`).
 *
 * The move itself is `lib/move` (the protocol, its threat analysis and its
 * full outcomes: `lib/move/types.ts`): a window on the app host opened from
 * the notice's click, this origin's stores sent to it, checked there by a
 * fresh device sign-in, and wiped here only after that answer. Here its
 * outcome is said the way the notice takes it.
 */
import i18next from "i18next";
import { type MoveOrigins, type MoveOutcome as Ended, startMove as startPush, startPull as startPullMove } from "@/lib/move";
import { MOVE_OUT_KEY, readMoveOut } from "@/lib/move/sender";
import type { MoveUnfinished } from "@/lib/move/types";
import { outcomeText } from "./move-page";

export { MovePage } from "./move-page";
export { PushMovedNotice } from "./push-moved-notice";

export interface MoveRequest {
  /** The app host's origin, the one end the stores may go to (`https://my.droppen.app`). */
  app: string;
  /** Where this tab goes once moved: the path, query and fragment it loaded with. */
  continueTo: string;
  /**
   * The account this browser keeps a device of, as far as the notice can tell
   * without opening it (`null`: it cannot). The move signs in as the device
   * the session names, or the only one noted; this is for what the notice says.
   */
  account: string | null;
}

export type MoveOutcome =
  /** The app host took the stores and signed in with them; nothing of the device is left here. The notice goes on to `app` + `continueTo`. */
  | { kind: "moved" }
  /** The person closed the window, or said no in it: nothing changed. */
  | { kind: "cancelled" }
  /** The browser would not open the window: nothing changed. */
  | { kind: "blocked" }
  /** It did not go through: nothing was wiped here (`message`, for the person). */
  | { kind: "failed"; message: string }
  /** This build, or this browser, cannot move a device. */
  | { kind: "unavailable" };

/**
 * A move of this browser's device that did not finish (the keys went; the
 * app host may be using the device while this origin still holds it): `null`
 * when there is none. While there is, "remove this browser" here deletes only
 * this copy and never takes the device off the account (`hosts/remove-here.ts`),
 * and Move finishes it.
 */
export function moveUnfinished(storage: Pick<Storage, "getItem"> | undefined = pageStorage()): MoveUnfinished | null {
  return storage ? readMoveOut(storage) : null;
}

/** Forget the note of an unfinished move (this origin's copy is gone). */
export function forgetMoveUnfinished(storage: Pick<Storage, "removeItem"> | undefined = pageStorage()): void {
  try {
    storage?.removeItem(MOVE_OUT_KEY);
  } catch {
    // nothing kept
  }
}

function pageStorage(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

/** Whether this browser can move its device here at all (the notice offers it only then): a secure page with IndexedDB, WebCrypto and windows. */
export function moveSupported(): boolean {
  try {
    const g = globalThis as { isSecureContext?: boolean; indexedDB?: unknown; crypto?: Crypto; window?: Window };
    return g.isSecureContext !== false && !!g.indexedDB && !!g.crypto?.subtle && typeof g.window?.open === "function";
  } catch {
    return false;
  }
}

const hostOf = (origin: string) => {
  try {
    return new URL(origin).host;
  } catch {
    return origin;
  }
};

/** A move's full outcome as the notice takes it. */
export function noticeOutcome(ended: Ended, origins: MoveOrigins): MoveOutcome {
  switch (ended.kind) {
    case "moved":
      return { kind: "moved" };
    case "popupBlocked":
      return { kind: "blocked" };
    case "closed":
    case "cancelled":
      return { kind: "cancelled" };
    default: {
      // The page's language; the English key filled in where i18n is not set up (tests).
      const t = (key: string, vars: Record<string, unknown> = {}) => {
        const said: unknown = i18next.isInitialized ? i18next.t(key, vars) : undefined;
        return typeof said === "string" ? said : key.replace(/\{\{(\w+)\}\}/g, (_, name: string) => String(vars[name] ?? ""));
      };
      return { kind: "failed", message: outcomeText(t, ended, { app: hostOf(origins.app), legacy: hostOf(origins.legacy) }) };
    }
  }
}

/**
 * Move this browser's device to `request.app`. Called straight from the
 * click on "Move this browser": a window may only open from a click, so it
 * opens before anything is waited for. Settles once the move is done, given
 * up or failed; never throws.
 */
export function startMove(request: MoveRequest): Promise<MoveOutcome> {
  if (!moveSupported()) return Promise.resolve({ kind: "unavailable" });
  const origins = { legacy: location.origin, app: request.app };
  return startPush({ origins }).then(
    (ended) => noticeOutcome(ended, origins),
    (error: unknown) => ({ kind: "failed", message: error instanceof Error ? error.message : String(error) }),
  );
}

/**
 * Bring the device this browser keeps at the hub's origin (`legacy`, e.g.
 * `https://droppen.app`) over to this page, the app host. Called straight
 * from the click on "Bring it over", like {@link startMove}. On `moved` this
 * origin holds the device and a fresh session: reload into the app.
 */
export function startPull(request: { legacy: string }): Promise<MoveOutcome> {
  if (!moveSupported()) return Promise.resolve({ kind: "unavailable" });
  const origins = { legacy: request.legacy, app: location.origin };
  return startPullMove({ origins }).then(
    (ended) => noticeOutcome(ended, origins),
    (error: unknown) => ({ kind: "failed", message: error instanceof Error ? error.message : String(error) }),
  );
}
