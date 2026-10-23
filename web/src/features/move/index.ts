/*
 * Moving this browser's device from the hub's origin to its app host
 * (`droppen.app` → `my.droppen.app`): the seam the move notice
 * (`features/hosts/legacy.tsx`) calls through.
 *
 * The move itself (the push flow: a window on the app host opened from the
 * notice's click, this origin's stores sent to it, checked there by a fresh
 * device sign-in, and wiped here only after its acknowledgement) is a module
 * of its own; until it is in, these stubs say it is not available, and the
 * notice offers removing this browser or going on without it.
 */

export interface MoveRequest {
  /** The app host's origin, the one end the stores may go to (`https://my.droppen.app`). */
  app: string;
  /** Where this tab goes once moved: the path, query and fragment it loaded with. */
  continueTo: string;
  /** The account this browser keeps a device of, as far as the notice can tell without opening it (`null`: it cannot). */
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

/** Whether this browser can move its device here at all (the notice offers it only then). */
export function moveSupported(): boolean {
  return false;
}

/**
 * Move this browser's device to `request.app`. Called straight from the
 * click on "Move this browser": a window may only open from a click, so it
 * opens before anything is waited for. Settles once the move is done, given
 * up or failed; never throws.
 */
export async function startMove(request: MoveRequest): Promise<MoveOutcome> {
  void request;
  return { kind: "unavailable" };
}
