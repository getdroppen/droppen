/**
 * How one browser keeps its copy of each screen honest with the server's.
 *
 * A screen's layout is edited locally first — a split or a drag has to answer
 * at once — and saved behind a short debounce. The local tree is therefore a
 * *working copy*, and the server is where other devices meet it. Two rules
 * keep the two from silently overwriting each other:
 *
 * - Every save names the revision its tree was based on (`baseRevision`). The
 *   server refuses a save based on a revision it has since replaced, with
 *   `409` and the screen as it now is; the client adopts that and says so, and
 *   the local edit is dropped rather than merged. A tree merge would be a guess
 *   about what two people meant, and the edit is one gesture to redo.
 * - A newer server copy is adopted only while there is no local edit waiting.
 *   With one waiting, the save carries the check and the `409` decides.
 *
 * A `409` is not always another device, though, and {@link resolveConflict}
 * tells the cases apart before anything is dropped: a save whose answer was
 * lost on a flaky network has usually landed, and the retry is refused by its
 * own earlier self; and a rename elsewhere moves the revision without moving
 * the layout. Only a server layout this browser never sent costs local edits.
 *
 * Everything here is pure: the provider in `screens.tsx` holds the state, the
 * timers and the requests, and asks this module what each event means.
 */
import { firstLeafId, hasLeaf, newPaneId, type PaneNode, type PaneTreeState } from "./pane-tree";

/** A screen as the server sends it, as far as syncing is concerned. */
export interface ServerScreen {
  id: string;
  revision: number;
  layout?: unknown;
  title?: string;
  emoji?: string;
}

/** A layout save whose answer never arrived, and which may have landed anyway. */
export interface SentLayout {
  /** The `editSeq` the save carried edits up to. */
  seq: number;
  layout: unknown;
}

/** How many unanswered saves are remembered; older ones are superseded anyway. */
const UNANSWERED_KEPT = 4;

/** What this browser knows about one screen's saves. */
export interface ScreenSync {
  /** The server revision the local tree was last known to agree with. */
  revision: number;
  /** The server's layout at {@link ScreenSync.revision}. */
  layout: unknown;
  /** Bumped by every local layout edit. */
  editSeq: number;
  /** The newest `editSeq` the server has taken. */
  savedSeq: number;
  /** Consecutive saves that failed without an answer worth acting on. */
  failures: number;
  /** Why the server refused the last save outright; it is not retried. */
  rejected: string | null;
  /** Saves sent without an answer, newest last; see {@link resolveConflict}. */
  unanswered: readonly SentLayout[];
}

export type SyncEvent =
  /** The user changed the layout. */
  | { type: "edit" }
  /** A save carrying edits up to `seq` landed as `revision`, holding `layout`. */
  | { type: "saved"; revision: number; seq: number; layout: unknown }
  /** A save failed (network, timeout, 5xx); it will be retried. `sent` is what
   *  it carried, in case it landed with the answer lost. */
  | { type: "failed"; sent?: SentLayout }
  /** A save went out that nobody will hear the answer to (`keepalive`). */
  | { type: "unanswered"; sent: SentLayout }
  /** The server refused a save in a way a retry cannot fix. */
  | { type: "rejected"; error: string }
  /** The server's screen replaces the tree; every local edit is dropped. */
  | { type: "conflict"; revision: number; layout: unknown }
  /** A newer server copy replaced a tree that had no edit waiting. */
  | { type: "adopt"; revision: number; layout: unknown }
  /** The revision moved and the layout did not (a rename, ours or another
   *  device's): any waiting edit is still an edit on top of the new revision. */
  | { type: "rebased"; revision: number; layout: unknown };

export const freshSync = (revision: number, layout: unknown = null): ScreenSync => ({
  revision,
  layout,
  editSeq: 0,
  savedSeq: 0,
  failures: 0,
  rejected: null,
  unanswered: [],
});

/** Whether the local tree holds an edit the server has not taken yet. */
export function hasPendingEdit(sync: ScreenSync): boolean {
  return sync.editSeq > sync.savedSeq;
}

/** Whether the screen's tab should say its last change is not saved. */
export function isUnsaved(sync: ScreenSync | undefined): boolean {
  return !!sync && hasPendingEdit(sync) && (sync.failures > 0 || sync.rejected !== null);
}

/** Why the last change will not be saved without another edit, if the server said. */
export function saveError(sync: ScreenSync | undefined): string | null {
  return sync && hasPendingEdit(sync) ? sync.rejected : null;
}

/** Whether a failed save is waiting on a retry (and so is worth retrying now). */
export function isRetrying(sync: ScreenSync | undefined): boolean {
  return !!sync && hasPendingEdit(sync) && sync.failures > 0 && sync.rejected === null;
}

export function reduceSync(sync: ScreenSync, event: SyncEvent): ScreenSync {
  switch (event.type) {
    case "edit":
      return { ...sync, editSeq: sync.editSeq + 1 };
    case "saved":
      return {
        ...sync,
        revision: event.revision,
        layout: event.layout,
        savedSeq: Math.max(sync.savedSeq, event.seq),
        failures: 0,
        rejected: null,
        // Superseded: a later 409 cannot be the doing of a save this one covers.
        unanswered: sync.unanswered.filter((sent) => sent.seq > event.seq),
      };
    case "failed":
      return {
        ...sync,
        failures: sync.failures + 1,
        unanswered: event.sent ? [...sync.unanswered, event.sent].slice(-UNANSWERED_KEPT) : sync.unanswered,
      };
    case "unanswered":
      return { ...sync, unanswered: [...sync.unanswered, event.sent].slice(-UNANSWERED_KEPT) };
    case "rejected":
      return { ...sync, failures: 0, rejected: event.error };
    case "conflict":
      // Everything edited locally is dropped with the tree it was edited on.
      return { ...freshSync(event.revision, event.layout), editSeq: sync.editSeq, savedSeq: sync.editSeq };
    case "adopt":
      // Never over a waiting edit: that edit's save is what finds out.
      return hasPendingEdit(sync) || event.revision <= sync.revision
        ? sync
        : { ...sync, revision: event.revision, layout: event.layout };
    case "rebased":
      return event.revision > sync.revision ? { ...sync, revision: event.revision, layout: event.layout } : sync;
  }
}

/** How long to wait before retrying a failed save: 1 s, doubling, capped at 30 s. */
export function retryDelay(failures: number): number {
  return Math.min(1000 * 2 ** Math.max(0, failures - 1), 30_000);
}

/** What a save's answer means for the screen. */
export type SaveOutcome = "saved" | "conflict" | "gone" | "retry" | "rejected";

/**
 * Only what a later attempt can fix is retried: no answer at all (offline, a
 * dropped connection — `undefined`), a timeout, or a server error, such as the
 * `502` of a server mid-restart. Any other refusal — a `400`, a `401`, a `413`
 * — would be refused again every thirty seconds for ever, so it stops and the
 * screen says why.
 */
export function classifySave(status: number | undefined): SaveOutcome {
  if (status === undefined) return "retry";
  if (status >= 200 && status < 300) return "saved";
  if (status === 409) return "conflict";
  if (status === 404) return "gone";
  if (status === 408 || status >= 500) return "retry";
  return "rejected";
}

/** The server sanitizes tree shape; this only guards the loose contract type. */
export function parseLayout(value: unknown): PaneNode {
  if (typeof value === "object" && value !== null && "type" in value) return value as PaneNode;
  return { type: "leaf", id: newPaneId("pane"), content: { kind: "picker" } };
}

/** Structural equality, blind to key order — the server rebuilds every node. */
export function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every((key) => sameValue((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]));
}

/**
 * A server layout taking over a local tree.
 *
 * The panes render positionally, so a tree that keeps its shape keeps every
 * pane mounted — terminals keep their scrollback and sockets. An identical
 * tree is not replaced at all, and focus stays where it was whenever the
 * focused pane survived.
 */
export function adoptTree(current: PaneTreeState | undefined, layout: unknown): PaneTreeState {
  const root = parseLayout(layout);
  if (current && sameValue(current.root, root)) return current;
  const focusedId = current && hasLeaf(root, current.focusedId) ? current.focusedId : firstLeafId(root);
  return { root, focusedId };
}

/** Every open screen's tree and save state, as one value. */
export interface ScreensWorkingCopy {
  trees: Readonly<Record<string, PaneTreeState>>;
  syncs: Readonly<Record<string, ScreenSync>>;
}

export const EMPTY_COPY: ScreensWorkingCopy = { trees: {}, syncs: {} };

/** What to do with a server copy of a screen this browser may already hold. */
export function serverScreenAction(sync: ScreenSync | undefined, serverRevision: number): "seed" | "adopt" | "keep" {
  if (!sync) return "seed";
  if (serverRevision <= sync.revision) return "keep";
  return hasPendingEdit(sync) ? "keep" : "adopt";
}

/**
 * Fold a server list into the working copy: seed screens seen for the first
 * time, adopt newer copies of screens with nothing waiting, keep the rest, and
 * drop screens the server no longer has. Returns `copy` itself when nothing
 * moved, so a poll that brings nothing new renders nothing.
 */
export function reconcileScreens(copy: ScreensWorkingCopy, screens: readonly ServerScreen[]): ScreensWorkingCopy {
  let changed = false;
  const trees: Record<string, PaneTreeState> = {};
  const syncs: Record<string, ScreenSync> = {};
  for (const screen of screens) {
    const tree = copy.trees[screen.id];
    const sync = tree ? copy.syncs[screen.id] : undefined;
    switch (serverScreenAction(sync, screen.revision)) {
      case "seed": {
        trees[screen.id] = adoptTree(undefined, screen.layout);
        syncs[screen.id] = freshSync(screen.revision, screen.layout);
        changed = true;
        break;
      }
      case "adopt": {
        trees[screen.id] = adoptTree(tree, screen.layout);
        syncs[screen.id] = reduceSync(sync!, { type: "adopt", revision: screen.revision, layout: screen.layout });
        changed = true;
        break;
      }
      case "keep":
        trees[screen.id] = tree!;
        syncs[screen.id] = sync!;
    }
  }
  if (Object.keys(copy.trees).some((id) => !(id in trees))) changed = true;
  return changed ? { trees, syncs } : copy;
}

/** A screen this browser just made, held at once rather than when a list next says so. */
export function seedScreen(copy: ScreensWorkingCopy, screen: ServerScreen): ScreensWorkingCopy {
  if (copy.syncs[screen.id]) return copy;
  return {
    trees: { ...copy.trees, [screen.id]: adoptTree(undefined, screen.layout) },
    syncs: { ...copy.syncs, [screen.id]: freshSync(screen.revision, screen.layout) },
  };
}

/** A local edit to one tree, counted so its save can be told apart from later ones. */
export function applyLocalEdit(
  copy: ScreensWorkingCopy,
  id: string,
  edit: (state: PaneTreeState) => PaneTreeState,
): { copy: ScreensWorkingCopy; edited: boolean } {
  const tree = copy.trees[id];
  const sync = copy.syncs[id];
  if (!tree || !sync) return { copy, edited: false };
  const next = edit(tree);
  if (next === tree) return { copy, edited: false };
  // Focus moves are local: they change the state but not what is saved.
  const edited = next.root !== tree.root;
  return {
    copy: {
      trees: { ...copy.trees, [id]: next },
      syncs: edited ? { ...copy.syncs, [id]: reduceSync(sync, { type: "edit" }) } : copy.syncs,
    },
    edited,
  };
}

/** Apply a sync event to one screen; a screen this browser no longer holds is left alone. */
export function withSync(copy: ScreensWorkingCopy, id: string, event: SyncEvent): ScreensWorkingCopy {
  const sync = copy.syncs[id];
  if (!sync) return copy;
  const next = reduceSync(sync, event);
  return next === sync ? copy : { ...copy, syncs: { ...copy.syncs, [id]: next } };
}

/**
 * A `409`: the server's screen replaces the local tree. `notice` says whether
 * a local edit was actually lost, which is when the user should be told — not
 * when the server already holds exactly this tree (a save that landed while
 * its answer did not, say a `keepalive` one as the page was hidden).
 */
export function adoptConflict(copy: ScreensWorkingCopy, screen: ServerScreen): { copy: ScreensWorkingCopy; notice: boolean } {
  const sync = copy.syncs[screen.id];
  if (!sync) return { copy, notice: false };
  const tree = copy.trees[screen.id];
  const adopted = adoptTree(tree, screen.layout);
  return {
    copy: {
      trees: { ...copy.trees, [screen.id]: adopted },
      syncs: {
        ...copy.syncs,
        [screen.id]: reduceSync(sync, { type: "conflict", revision: screen.revision, layout: screen.layout }),
      },
    },
    notice: hasPendingEdit(sync) && adopted !== tree,
  };
}

/** How a `409` was resolved; see {@link resolveConflict}. */
export type ConflictResolution =
  /** The server holds a layout this browser sent: that save landed after all. */
  | "landed"
  /** The revision moved, the layout did not: waiting edits stay, on the new base. */
  | "rebased"
  /** The server holds a layout from somewhere else: it replaces the local tree. */
  | "adopted";

/**
 * Resolve a `409` without throwing away anything the server already has.
 *
 * `inFlight` is the layout save that was refused, if it was one. In order:
 *
 * 1. The server's layout is one this browser sent — the refused save, or an
 *    earlier one whose answer never came. The classic case is a phone losing
 *    the response to a save that landed: its retry still names the old
 *    revision and is refused by its own earlier self. That save is marked
 *    taken at the server's revision, and every edit made after it stays
 *    waiting, to be saved on the right base. Nothing is shown.
 * 2. The server's layout is the one this browser's revision already had: the
 *    revision moved for a rename (ours with its answer lost, or another
 *    device's). Waiting edits are still edits on top of it; rebase and keep.
 * 3. Otherwise another device really did change the layout: adopt it and drop
 *    the local edits ({@link adoptConflict}); `notice` says whether any were.
 */
export function resolveConflict(
  copy: ScreensWorkingCopy,
  screen: ServerScreen,
  inFlight?: SentLayout,
): { copy: ScreensWorkingCopy; notice: boolean; resolution: ConflictResolution } {
  const sync = copy.syncs[screen.id];
  if (!sync) return { copy, notice: false, resolution: "adopted" };
  const sent = [...sync.unanswered, ...(inFlight ? [inFlight] : [])]
    .filter((candidate) => sameValue(candidate.layout, screen.layout))
    .reduce<SentLayout | null>((newest, candidate) => (!newest || candidate.seq > newest.seq ? candidate : newest), null);
  if (sent) {
    const event: SyncEvent = { type: "saved", revision: screen.revision, seq: sent.seq, layout: screen.layout };
    return { copy: withSync(copy, screen.id, event), notice: false, resolution: "landed" };
  }
  if (sameValue(screen.layout, sync.layout)) {
    const event: SyncEvent = { type: "rebased", revision: screen.revision, layout: screen.layout };
    return { copy: withSync(copy, screen.id, event), notice: false, resolution: "rebased" };
  }
  return { ...adoptConflict(copy, screen), resolution: "adopted" };
}

/**
 * Whether the server already shows the name and icon a refused rename or icon
 * change asked for — its answer was lost and it landed anyway, or another
 * device made the same choice. Either way there is nothing left to send. The
 * server trims both, and stores a blank icon as none.
 */
export function metadataLanded(screen: ServerScreen, sent: { title?: string; emoji?: string }): boolean {
  if (sent.title !== undefined && sent.title.trim() !== screen.title) return false;
  if (sent.emoji !== undefined && (sent.emoji.trim() || undefined) !== (screen.emoji || undefined)) return false;
  return true;
}

/** Forget one screen. */
export function dropScreen(copy: ScreensWorkingCopy, id: string): ScreensWorkingCopy {
  if (!(id in copy.trees) && !(id in copy.syncs)) return copy;
  const trees = { ...copy.trees };
  const syncs = { ...copy.syncs };
  delete trees[id];
  delete syncs[id];
  return { trees, syncs };
}

/** What the global feed says about screens; see `crates/droppen-server/src/screens`. */
export type ScreensFeedEvent =
  | { type: "screens:changed"; id: string; revision: number; deleted?: boolean }
  | { type: "screens:list"; listRevision: number }
  /** The feed (re)connected or fell behind; anything may have been missed. */
  | { type: "screens:resync" };

/** A feed frame, if it is one of the screens'. */
export function screensFeedEvent(frame: Record<string, unknown>): ScreensFeedEvent | null {
  if (frame.type === "screens:changed" && typeof frame.id === "string" && typeof frame.revision === "number") {
    return { type: "screens:changed", id: frame.id, revision: frame.revision, ...(frame.deleted === true ? { deleted: true } : {}) };
  }
  if (frame.type === "screens:list" && typeof frame.listRevision === "number") {
    return { type: "screens:list", listRevision: frame.listRevision };
  }
  return null;
}

/**
 * Whether a feed event means the list should be fetched again. Our own writes
 * come back as events too, and are recognised by revision: by the time one
 * arrives the answer to the request has usually already moved us there.
 */
export function needsRefetch(
  event: ScreensFeedEvent,
  listRevision: number | undefined,
  revisionOf: (id: string) => number | undefined,
): boolean {
  switch (event.type) {
    case "screens:resync":
      return true;
    case "screens:list":
      return listRevision === undefined || event.listRevision > listRevision;
    case "screens:changed": {
      const known = revisionOf(event.id);
      if (event.deleted) return known !== undefined;
      return known === undefined || event.revision > known;
    }
  }
}
