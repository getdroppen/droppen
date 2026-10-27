/**
 * A device's session list off its offline copy, as a store
 * (`device-sessions.ts` reads every device's): the copy's index, which the
 * page follows anyway, replaced when it changes.
 *
 * While the device is not online its rows are shown as the app shows them
 * from the copy (`restingRow`, `lib/transport/offline.ts`): nothing can be running
 * on it, so nothing on it is working or asking you — what is unread stays.
 * The same goes for a device that says it is online but whose copy has not
 * moved for longer than its heartbeat allows ({@link STALE_COPY_MS}): a
 * stream writer stuck or refused, a key epoch this device lacks — an old
 * question must not sit at the top of the ledger as if it were being asked.
 */
import type { components } from "@/api/schema";
import type { MachineKey } from "@/lib/machines/key";
import { createStore, type Store } from "@/lib/machines/store";
import { restingRow } from "@/lib/transport/offline";
import type { DeviceSessions } from "./needs-you";

type SessionSummary = components["schemas"]["SessionSummary"];

/**
 * How old a copy may be before it is not taken as the device's word any more:
 * the agent signs a fresh head on every stream at least every 5 minutes
 * (`HEARTBEAT_EVERY`, `agent/streams/mod.rs`), so three missed beats.
 */
export const STALE_COPY_MS = 15 * 60_000;

/** What a device's copy offers the lists: `MachineMirror` is one. */
export interface CopyLike {
  /** Its sessions, newest first; `null` with no copy. */
  rows(): SessionSummary[] | null;
  /** How current it is (ms); `null` with none. */
  readonly freshestMs: number | null;
  /** Whether it has been read off the device yet. */
  readonly loaded: boolean;
  onChange(fn: (change: { kind: string }) => void): () => void;
  whenLoaded(): Promise<void>;
}

export interface SessionsStore extends Store<DeviceSessions> {
  /** Look again at whether the device is online (its presence changed). */
  refresh(): void;
  /** Stop following the copy. */
  close(): void;
}

export interface CopySessionsOptions {
  /** Whether the device is up now (default: always). */
  online?: () => boolean;
  /** How old a copy may be before it is shown at rest (default {@link STALE_COPY_MS}). */
  staleMs?: number;
  /** The page's clock (tests). */
  now?: () => number;
}

/**
 * `copy`'s session list as a store, replaced when the copy's index changes
 * (or it is first read off the device), when the device comes or goes, or
 * when its copy goes stale — and only then: a selector comparing by identity
 * sees one change per change.
 */
export function copySessions(machine: MachineKey, copy: CopyLike, opts: CopySessionsOptions = {}): SessionsStore {
  const online = opts.online ?? (() => true);
  const staleMs = opts.staleMs ?? STALE_COPY_MS;
  const now = opts.now ?? Date.now;
  // The copy's rows as last read (`rows()` sorts afresh on every call), and the rows shown for them.
  let raw: SessionSummary[] | null = copy.rows();
  let shown: { raw: SessionSummary[] | null; resting: boolean; rows: readonly SessionSummary[] | null } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let open = true;
  const rowsOf = (resting: boolean): readonly SessionSummary[] | null => {
    if (shown && shown.raw === raw && shown.resting === resting) return shown.rows;
    shown = { raw, resting, rows: raw && resting ? raw.map(restingRow) : raw };
    return shown.rows;
  };
  const read = (): DeviceSessions => {
    const asOf = copy.freshestMs;
    const up = online();
    const stale = up && asOf !== null && now() - asOf > staleMs;
    // Look again the moment it would go stale, if nothing moves it before then.
    clearTimeout(timer);
    timer = undefined;
    if (open && up && !stale && asOf !== null) {
      timer = setTimeout(() => {
        timer = undefined;
        if (open) store.set(read());
      }, asOf + staleMs - now() + 1);
      (timer as { unref?: () => void }).unref?.();
    }
    const resting = !up || stale;
    return {
      machine,
      rows: rowsOf(resting),
      from: "copy",
      asOf,
      ...(copy.loaded ? {} : { pending: true }),
      ...(resting ? { away: true } : {}),
      ...(stale ? { stale: true } : {}),
    };
  };
  const store = createStore<DeviceSessions>(
    read(),
    (a, b) => a.rows === b.rows && a.asOf === b.asOf && a.pending === b.pending && a.away === b.away && a.stale === b.stale,
  );
  const reread = () => {
    raw = copy.rows();
    store.set(read());
  };
  const off = copy.onChange((change) => {
    if (open && change.kind === "index") reread();
  });
  void copy.whenLoaded().then(() => {
    if (open) reread();
  });
  return {
    get: store.get,
    subscribe: store.subscribe,
    refresh() {
      if (open) store.set(read());
    },
    close() {
      open = false;
      clearTimeout(timer);
      off();
    },
  };
}
