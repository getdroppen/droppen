import type { components } from "@/api/schema";
import { authStatus, currentUser } from "@/lib/auth";
import { isCloud } from "@/lib/hub/config";
import { cacheClear, cacheTransact } from "@/lib/local-cache";
import type { FeedPersist, FeedState } from "@/features/sessions/session-feed-pool";
import { copyFeedNow, copyFeedRead } from "@/features/sessions/feed-from-copy";
import { nextFeedIndex, orphanFeedKeys, readCopy, COPY_SCHEMA, feedCopyKey, type CopyRecord } from "@/features/sessions/feed-copy";
import { LOCAL, type MachineKey } from "@/lib/machines/key";
import { machineCacheKey } from "@/lib/machines/storage";

type SessionSummary = components["schemas"]["SessionSummary"];

/*
 * What this page keeps on the device for its next cold start (`lib/local-cache.ts`):
 * the session list and the last few windows read. Both are drawn at once on the
 * next load and replaced by the socket as soon as it answers — the windows by a
 * resume from their own marker, which is usually a handful of items. The
 * record format, and the rules for which records count, are in `feed-copy.ts`.
 *
 * Each machine's records are its own (`machineCacheKey`): the local
 * machine's under the very keys from before machines had keys, any other's
 * under `m/<machine>/…`, each with its own index and sweep.
 *
 * Not in cloud mode. There the page draws from the machine's offline copy
 * (`lib/hub/mirror.ts`) — per machine, verified against what the machine
 * signed — and this copy would only keep a second copy of every transcript.
 * Its windows are read from that copy instead, and never written
 * (`feed-from-copy.ts`). Nor, in the local app, for another device its gateway
 * shows (S1-7): that device's copy is the local install's, and the page keeps
 * nothing of it.
 */

/**
 * Whether `machine`'s list and windows are kept on this device: only the local
 * app's own server's. In cloud mode a machine's windows are read, but never
 * written, from its offline copy (`feed-from-copy.ts`).
 */
function keptHere(machine: MachineKey): boolean {
  return !isCloud() && machine === LOCAL;
}

const FEEDS_INDEX = "feeds";
const SESSIONS = "sessions";

/** Who the copy is for, once the page knows; `undefined` until it does. */
async function owner(): Promise<string | null | undefined> {
  const known = currentUser();
  if (known !== undefined) return known;
  try {
    return (await authStatus()).user;
  } catch {
    return undefined;
  }
}

async function load<T>(key: string): Promise<T | null> {
  const [record, who] = await Promise.all([
    cacheTransact<unknown>("readonly", (store, done) => {
      const request = store.get(key);
      request.onsuccess = () => done(request.result);
    }),
    owner(),
  ]);
  if (record === undefined || who === undefined) return null;
  const read = readCopy<T>(record, who);
  if ("value" in read) return read.value;
  // Another user's copy on this device: none of it is ours to show.
  if ("foreign" in read) void cacheClear();
  return null;
}

/**
 * The last few windows of `machine`'s sessions read on this device (`key` is a
 * session's `source/id`); in cloud mode, the windows its offline copy holds.
 */
export function devicePersistFor(machine: MachineKey): FeedPersist {
  const index = machineCacheKey(machine, FEEDS_INDEX);
  return {
    peek: (key) => (keptHere(machine) ? null : copyFeedNow(machine, key)),
    load: (key) =>
      (keptHere(machine) ? load<FeedState>(machineCacheKey(machine, feedCopyKey(key))) : copyFeedRead(machine, key)).then((data) =>
        data && typeof data === "object" && Array.isArray(data.items) && data.summary ? data : null,
      ),
    save(key, data) {
      const who = currentUser();
      if (who === undefined || !keptHere(machine)) return;
      const record: CopyRecord<FeedState> = { v: COPY_SCHEMA, owner: who, value: data };
      // One transaction: the copy, the index naming it, and the sweep of every
      // copy the index no longer names — so two tabs saving at once cannot leave
      // more than the kept few behind.
      void cacheTransact<void>("readwrite", (store) => {
        store.put(record, machineCacheKey(machine, feedCopyKey(key)));
        const read = store.get(index);
        read.onsuccess = () => {
          const { kept } = nextFeedIndex(read.result, key);
          store.put(kept, index);
          const keys = store.getAllKeys();
          keys.onsuccess = () => {
            for (const orphan of orphanFeedKeys(keys.result, kept, machine)) store.delete(orphan);
          };
        };
      });
    },
  };
}

/** `machine`'s session list as last seen, or `null`. */
export function loadSessionList(machine: MachineKey): Promise<SessionSummary[] | null> {
  if (!keptHere(machine)) return Promise.resolve(null);
  return load<SessionSummary[]>(machineCacheKey(machine, SESSIONS)).then((rows) => (Array.isArray(rows) ? rows : null));
}

/** A machine's list waiting to be saved, and the timer that will. */
const pendingLists = new Map<MachineKey, SessionSummary[]>();

/** Keep a machine's list a few seconds behind its feed: it moves several times a second while anything works. */
export function saveSessionList(machine: MachineKey, rows: SessionSummary[]): void {
  if (!keptHere(machine)) return;
  const waiting = pendingLists.has(machine);
  pendingLists.set(machine, rows);
  if (waiting) return;
  setTimeout(() => {
    const value = pendingLists.get(machine);
    pendingLists.delete(machine);
    const who = currentUser();
    if (!value || who === undefined) return;
    const record: CopyRecord<SessionSummary[]> = { v: COPY_SCHEMA, owner: who, value };
    void cacheTransact<void>("readwrite", (store) => {
      store.put(record, machineCacheKey(machine, SESSIONS));
    });
  }, 5000);
}
