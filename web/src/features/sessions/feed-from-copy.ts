import type { components } from "@/api/schema";
import { hubShell } from "@/features/machines/shells";
import { OPENING_ITEMS, type FeedState } from "@/features/sessions/session-feed-pool";
import type { MachineMirror, SessionCopy } from "@/lib/hub/mirror";
import type { MachineKey } from "@/lib/machines/key";

type SessionSummary = components["schemas"]["SessionSummary"];

/*
 * A session's window drawn from the machine's offline copy (`lib/hub/mirror.ts`)
 * while droppen.app asks the machine itself for it: what the local app does
 * with the windows it keeps (`feed-persist.ts`), from the copy this device
 * holds of the sessions opened here and the most recently updated ones,
 * rather than from a second copy of every transcript. Nothing is written.
 *
 * Only while the page is on the machine (`live`). On its copy the copy is
 * what the feed socket answers with already (`lib/transport/offline.ts`),
 * and the local app's gateway shows other devices from their copies alone,
 * with no machine to wait for.
 *
 * The window is the one the copy's own snapshot would be: its newest
 * {@link OPENING_ITEMS} items, with the resume marker the agent signed beside
 * them — `endIndex` and `prefixFp`, the marks the live server sends. So the
 * live socket opens with a delta from there (`?since=&fp=`), or, when the
 * session under it is no longer what the copy holds, with a snapshot that
 * replaces it. The state half says only what the copy can vouch for, the
 * list's row, and is marked as the copy's (`fromCopy`). The copy cannot say
 * what the session last ran with: the machine reads its model, effort and
 * permission (`cliChoice`) off fields of the transcript file the copy's items
 * do not carry. So until the live state replaces this one, which follows the
 * delta, the composer sends nothing, typed or queued (`composer.tsx`): a
 * message sent now would go out under the defaults rather than the session's
 * own choice. Nor does anything show what only that state can say — the
 * composer's model, effort and permission, its quota and context, and the
 * header's account (`session-detail.tsx`) — rather than the defaults.
 */

/** What of a session's copy its window is cut from: `SessionCopy`, `lib/hub/mirror.ts`. */
export type CopyOfSession = Pick<SessionCopy, "state" | "seq">;

/** An item the agent cut down to fit its copy: the machine still has it whole. */
const cutDown = (item: unknown) => (item as { rhOfflineTruncated?: unknown }).rhOfflineTruncated === true;

/**
 * `copy`'s newest `size` items as a window, under `row`, or `null` when there
 * is nothing to draw (an empty copy, or one whose numbers do not add up).
 *
 * The page is on the machine, which holds the whole session: anything before
 * the window is a "load earlier" away, past where the copy starts too — also
 * a copy the plan's retention window trimmed, whose earlier entries are only
 * on the machine anyway.
 */
export function copyFeedState(copy: CopyOfSession, row: SessionSummary, size: number = OPENING_ITEMS): FeedState | null {
  const { started, startIndex: from, endIndex, items: held, prefixFp } = copy.state;
  if (!started || !Number.isInteger(from) || !Number.isInteger(endIndex) || held.length !== endIndex - from) return null;
  const startIndex = Math.max(from, endIndex - size);
  const items = held.slice(startIndex - from);
  if (items.length === 0) return null;
  return {
    // The copy's version, as its own snapshot names it (`offlineSig`).
    sig: `offline:${copy.seq}`,
    summary: row,
    injection: null,
    canSend: false,
    sendBlockReason: null,
    cliChoice: null,
    contextTokens: null,
    contextWindow: 200_000,
    claudeRoot: row.claudeRoot ?? null,
    account: null,
    rotation: null,
    receipts: [],
    questions: [],
    items,
    startIndex,
    endIndex,
    totalItems: endIndex,
    hasMoreBefore: startIndex > 0,
    omittedItems: startIndex,
    // A resume keeps every item before the last few as it is held, and one
    // the agent cut down would stay cut down on the live page. Without the
    // marker the live socket sends the window whole.
    prefixFp: items.some(cutDown) ? null : prefixFp,
    startFp: null,
    fromCopy: true,
  };
}

/** `machine`'s copy while droppen.app reaches the machine itself rather than its copy; else `null`. */
function liveCopy(machine: MachineKey): MachineMirror | null {
  const held = hubShell()?.machine(machine);
  return held && held.transport.current === "live" ? held.mirror : null;
}

/** A feed's key (`source/id`) as the copy names the session. */
function sessionOf(key: string): { source: string; id: string } | null {
  const at = key.indexOf("/");
  return at > 0 ? { source: key.slice(0, at), id: key.slice(at + 1) } : null;
}

/** The window the copy in memory gives the session `key`, for the feed's very first frame. */
export function copyFeedNow(machine: MachineKey, key: string): FeedState | null {
  const mirror = liveCopy(machine);
  const ref = sessionOf(key);
  if (!mirror || !ref) return null;
  const row = mirror.row(ref.source, ref.id);
  const copy = row ? mirror.sessionNow(ref.source, ref.id) : null;
  return row && copy ? copyFeedState(copy, row) : null;
}

/**
 * The same, read back from the device when the copy is not in memory: what
 * the feed waits `HYDRATE_WAIT_MS` for before it connects without it.
 */
export async function copyFeedRead(machine: MachineKey, key: string): Promise<FeedState | null> {
  const mirror = liveCopy(machine);
  const ref = sessionOf(key);
  if (!mirror || !ref) return null;
  await mirror.whenLoaded();
  // A session the list does not have is no window to draw, and no copy worth reading.
  if (!mirror.row(ref.source, ref.id)) return null;
  const copy = await mirror.sessionHeld(ref.source, ref.id);
  const row = mirror.row(ref.source, ref.id);
  // Gone back to the copy meanwhile: its feed socket answers from the copy itself.
  return copy && row && liveCopy(machine) === mirror ? copyFeedState(copy, row) : null;
}
