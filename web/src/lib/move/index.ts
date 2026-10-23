/*
 * The browser key move (droppen.app → my.droppen.app): what the rest of the
 * app calls. The interface is in `./types.ts`; the page on `/move` is
 * `features/move/move-page.tsx`.
 */
import { browserEnv, type MoveEnv, sharedTabs } from "./env";
import { startMove as start, startPull as pull } from "./flows";
import { readLocalDevice } from "./local";
import type { LocalDevice, MoveEvent, StartMove, StartPull } from "./types";

export * from "./types";
export { movesLocal, MOVED_DATABASES } from "./allowlist";
export { servePullSender, servePushReceiver, type PullSender, type ReceiverView, type SenderView } from "./flows";

/** Push, from the legacy origin (see {@link StartMove}). */
export const startMove: StartMove = (opts) => start(opts);

/** Pull, from the app host (see {@link StartPull}). */
export const startPull: StartPull = (opts) => pull(opts);

/** What this origin holds of a device, read without creating anything (`null`: nothing). */
export function localDevice(env: MoveEnv = browserEnv()): Promise<LocalDevice | null> {
  return readLocalDevice(env);
}

/** Hear what this origin's other tabs say about a move (`MoveEvent`): returns a stop. */
export function watchMoves(fn: (event: MoveEvent) => void, env: Pick<MoveEnv, "tabs"> = { tabs: sharedTabs() }): () => void {
  return env.tabs?.subscribe(fn) ?? (() => undefined);
}
