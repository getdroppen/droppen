/*
 * The browser key move: this browser's device, as droppen.app keeps it, goes
 * to my.droppen.app without pairing again. This file is the move's interface
 * — what the host modes, the apex interstitial and the pages on `/move` call
 * and get back — and it stays stable; how it works is in `./flows.ts`.
 *
 * Two origins take part, and only these two: the one that holds the keys now
 * (`legacy`, the apex) and the one they go to (`app`). Two ways in, both a
 * popup opened by a click, never a frame:
 *
 * - push, from the apex: the interstitial's "Move this browser" click calls
 *   {@link StartMove} there, which opens `app` + {@link MOVE_PATH} as a popup;
 *   that page ({@link MovePageProps}) receives the keys.
 * - pull, from the app host: "Bring it over" calls {@link StartPull} there,
 *   which opens `legacy` + {@link MOVE_PATH}; that page shows which device it
 *   would send and sends it on one more click.
 *
 * Either way the receiving side writes what it got, signs in afresh with the
 * moved device key and only then says so; the sending side forgets its copy
 * only after that answer, and only if nothing changed meanwhile. Whatever
 * fails before that leaves the keys where they were.
 *
 * Both origins at once: a move cut off after the bundle went (the app host
 * signed in, and the legacy origin did not hear it, or kept its copy because
 * it changed) leaves the same device at both — the app host using it, the
 * legacy origin still holding its keys. Nothing is lost, and the legacy
 * origin knows it ({@link MoveUnfinished}, noted before the bundle is sent):
 * pressing Move again finishes it (the app host takes what it lacks, the
 * legacy origin forgets its copy). Until then "remove this browser" at the
 * legacy origin must delete only its own copy and never take the device off
 * the account — that would remove the device the app host is using.
 */

/** The path the move's popup page is served on, on both hosts. */
export const MOVE_PATH = "/move";

/**
 * The two origins of a move, exactly (`https://droppen.app`,
 * `https://my.droppen.app`): https, one host a subdomain of the other. The
 * host modes learn them from the hub (`/.well-known/droppen-hub`); the move
 * refuses anything else, and refuses to run on a page of neither.
 */
export interface MoveOrigins {
  /** Where the keys are now (the apex, legacy mode). */
  legacy: string;
  /** Where they go (the app host). */
  app: string;
}

/** Where a move is, for a page to say (`onStep`). */
export type MoveStep =
  /** The other window is opening. */
  | "opening"
  /** The other window is open; waiting for it to answer. */
  | "waiting"
  /** Pull: waiting for the click in the droppen.app window that sends the keys. */
  | "confirm"
  /** The keys are on their way. */
  | "sending"
  /** The app host is signing in with them. */
  | "verifying"
  /** The app host signed in with them: the old copy is being removed. */
  | "wiping";

/**
 * How a move ended. Only `moved` means the app host holds the device and
 * signed in with it; with every other outcome the keys are still where they
 * were (the legacy origin wiped nothing).
 */
export type MoveOutcome =
  | {
      kind: "moved";
      /** The account and device the app host signed in as. */
      account: string;
      device: string;
      /** The device whose notifications were on before the move (`null`: none were): the app host offers to turn them back on. */
      pushWasOn: string | null;
      /** The app host held this device already from an earlier move that did not finish: what it lacked was added. */
      resumed: boolean;
      /**
       * Whether the legacy origin removed its copy. `false` only for a pull
       * whose droppen.app window closed before it said so: the app host has
       * the device anyway, and droppen.app offers the move again (it finishes
       * at once, `resumed`).
       */
      senderWiped: boolean;
    }
  /** The browser would not open the other window: nothing happened. */
  | { kind: "popupBlocked" }
  /** The other window was closed before the keys went: nothing happened (after they went, it is `unfinished`). */
  | { kind: "closed" }
  /** Pull: the person cancelled in the droppen.app window. */
  | { kind: "cancelled" }
  /** There is no device to move (push: none here; pull: none at droppen.app). */
  | { kind: "nothingToMove" }
  /** The app host already holds another device: it takes none over it. */
  | { kind: "targetHasDevice" }
  /** The app host could not sign in with the moved keys (or they did not read back intact): it removed what it wrote, and droppen.app kept everything. */
  | { kind: "verifyFailed"; reason: string }
  /**
   * The keys went, and the app host may hold the device now (it said so, or
   * the move was cut off before it could): the legacy origin kept its copy
   * ({@link MoveUnfinished}). Pressing Move again finishes it. `why`: the app
   * host signed in but something here changed meanwhile (another tab), or
   * answered for other keys; or the other window closed or went silent
   * after the keys went.
   */
  | { kind: "unfinished"; why: "changed" | "mismatch" | "closed" | "timeout" }
  /** Another tab of this browser is moving, or renewing a key, right now: try again in a moment. */
  | { kind: "busy" }
  /** The other window did not answer in time, before the keys went: nothing happened. */
  | { kind: "timeout" }
  /** Anything else (a page on neither origin, a refused bundle, a storage error): nothing was removed. */
  | { kind: "failed"; reason: string };

export interface StartOptions {
  origins: MoveOrigins;
  /** Each step as it is reached. */
  onStep?: (step: MoveStep) => void;
}

/**
 * Push, from the legacy origin: move this browser's device to the app host.
 * Call it synchronously from the click handler — it opens the popup before it
 * awaits anything, and a popup needs the click. On `moved` the caller sends
 * the tab on to the app host (`location.replace(app + path)`); the popup
 * closes itself.
 */
export type StartMove = (opts: StartOptions) => Promise<MoveOutcome>;

/**
 * Pull, from the app host: bring the device kept at the legacy origin in this
 * browser over. Call it synchronously from the click handler, as
 * {@link StartMove}. On `moved` this origin holds the device and a fresh
 * session (`rh.hub.session`): reload into the app.
 */
export type StartPull = (opts: StartOptions) => Promise<MoveOutcome>;

/** The `/move` page on either host: it serves whichever side of the move its origin is (`features/move/move-page.tsx`). */
export interface MovePageProps {
  origins: MoveOrigins;
}

/** What this origin holds of a device, read without creating anything (no database is made by asking). */
export interface LocalDevice {
  /**
   * The account and device a move signs in as: the session's, or the only
   * device id this browser noted. `null` when none can be told (no device id
   * noted, or several and no session).
   */
  primary: { account: string; device: string } | null;
  /** Every device id this browser noted, per account. */
  accounts: { account: string; device: string }[];
  /** Whether device keys are stored here. */
  keys: boolean;
  /** Whether a device session is kept here (`rh.hub.session`). */
  signedIn: boolean;
}

/** What one tab tells this origin's other tabs (a `BroadcastChannel`). */
export type MoveEvent =
  /** A tab started sending this browser's device away: hold still. */
  | { t: "moving" }
  /** It went to `to`, and this origin forgot it: reload (the page then finds no device here). */
  | { t: "moved"; to: string }
  /** The move stopped without moving anything. */
  | { t: "stopped" }
  /** This origin (the app host) took a device over: reload to open it. */
  | { t: "arrived"; account: string };

/**
 * What the legacy origin notes of a move whose keys went but which did not
 * finish (`rh.hub.move.out`): `sent` — the app host may or may not hold the
 * device (the move was cut off before it answered); `verified` — it signed in
 * with it. Gone once the move finishes (with the rest of the copy) or the app
 * host refused the keys.
 */
export interface MoveUnfinished {
  state: "sent" | "verified";
  account: string;
  device: string;
  /** The app host the keys went to. */
  app: string;
  atMs: number;
}
