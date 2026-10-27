/*
 * Where this browser keeps, for each account it is a device of, the genesis
 * it pinned and what it verified of the account's chain since (the seen
 * record, `CloudAccount`): shared by every page of the browser, the pin set
 * once and the record moving forward only.
 *
 * That makes every write a read-compare-write, and it has to be one step
 * across pages. IndexedDB `readwrite` transactions whose scopes
 * overlap run one after another across every page of the origin, each seeing
 * what the one before it committed: a compare done inside the transaction is
 * against what is stored, not against a page's own copy of it — which is all
 * `localStorage` gives (each page reads its own copy, brought into step by
 * the browser only after the fact). The pin and the record are one value, so
 * which genesis is pinned and what is accepted under it are decided in the
 * same transaction. The compare runs synchronously inside it, between its
 * read and its write: nothing in it waits on anything else, so it never
 * commits half-way (WebKit commits a transaction once the page yields with no
 * request of it pending).
 *
 * IndexedDB has no change events: a page that writes a value says so on a
 * `BroadcastChannel`, and the pages open on the account hold it against what
 * they verified (`CloudAccount.watchSeenRecord`).
 *
 * Where IndexedDB is not there or will not open at all (some private modes),
 * both stay in `localStorage` as before (`rh.hub.genesis.<account>`,
 * `rh.hub.seen.<account>`): read-compare-written under a Web Lock, which a
 * page's stale copy can still lower, and its pages hear one another through
 * `storage` events. That is said on the console — and refused, as a failure
 * to try again, in a browser that kept them in IndexedDB before.
 */
import { type ChainClock, type ChainState, hex } from "@/lib/e2e";
import { openIdb } from "@/lib/idb";

/** Where older pages keep the seen record (and this one, where IndexedDB will not open): `localStorage`'s key for `account`. */
export const seenKey = (account: string) => `rh.hub.seen.${account}`;
/** Where older pages keep the pinned genesis (hex), likewise. */
export const genesisKey = (account: string) => `rh.hub.genesis.${account}`;
/** Set in `localStorage` once IndexedDB opened in this browser: from then on its `localStorage` copies are never read instead. */
export const SEEN_IN_IDB = "rh.hub.seen-in-idb";
/** The IndexedDB database the values are in (one object store, keyed by account): a database of its own, so the key store's is never upgraded for it. */
export const SEEN_DB = "rh-hub-seen";
/** The `BroadcastChannel` a page says on that it wrote an account's value. */
export const SEEN_CHANNEL = "rh.hub.seen";
/** How long opening the database may take before that open counts as failed (the next call tries again). */
export const SEEN_DB_OPEN_MS = 10_000;
const STORE = "seen";
/** The one object store of {@link SEEN_DB}. */
export const SEEN_STORE = STORE;

/** `localStorage`, or a stand-in. */
export type SeenStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

type EventTargetLike = Pick<EventTarget, "addEventListener" | "removeEventListener">;

const unref = <T>(handle: T): T => {
  (handle as { unref?: () => void }).unref?.();
  return handle;
};

/**
 * The store would not read or write what it keeps, or would not open: a
 * failure that may pass, to be tried again — never taken for a fork, and
 * never for there being nothing stored.
 */
export class SeenStoreError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "SeenStoreError";
  }
}

/** The browser's values, one per account (`{v, genesis, seen}`, as `CloudAccount` writes them and checks what it reads). */
export interface SeenStore {
  /**
   * Whether `update` is one step across every page of the browser (IndexedDB,
   * memory) — or only as good as the Web Lock the caller holds around it and
   * the page's own copy of `localStorage` (the fallback).
   */
  readonly atomic: boolean;
  /** `account`'s value as stored: `undefined` when there is none. */
  get(account: string): Promise<unknown>;
  /**
   * Read `account`'s value and write what `change` makes of it, as one step
   * no other page's update comes between. `change` is handed the value as
   * `get` would give it and runs synchronously, inside the step; it returns
   * what to store — `undefined`: leave it as it is — or throws, and then
   * nothing is written and the update rejects with what it threw. Resolves
   * once what was written is committed (and rejects when it was not); the
   * other pages are told then.
   */
  update(account: string, change: (stored: unknown) => unknown): Promise<void>;
  /** Hear that another page may have written a value: its account, or `null` for any. Returns the stop. */
  watch(heard: (account: string | null) => void): () => void;
}

/**
 * The values in memory: one browser's, for the tests. Updates run one after
 * another, as IndexedDB runs `readwrite` transactions on one store, and a
 * read waits for the updates asked for before it; every watcher (each page)
 * is told of every write.
 */
export class MemorySeenStore implements SeenStore {
  readonly atomic = true;
  /** What is stored, by account (each value a copy of what was put). */
  readonly values = new Map<string, unknown>();
  /**
   * Awaited as each update's turn comes, before it reads: holding it holds
   * that update back — and every update and read after it — as a long
   * transaction would.
   */
  gate: ((account: string) => Promise<void> | void) | null = null;
  private turn: Promise<unknown> = Promise.resolve();
  private readonly watchers = new Set<(account: string | null) => void>();

  get(account: string): Promise<unknown> {
    return this.queued(() => structuredClone(this.values.get(account)));
  }

  update(account: string, change: (stored: unknown) => unknown): Promise<void> {
    return this.queued(async () => {
      await this.gate?.(account);
      const next = change(structuredClone(this.values.get(account)));
      if (next === undefined) return;
      this.values.set(account, structuredClone(next));
      for (const heard of [...this.watchers]) queueMicrotask(() => heard(account));
    });
  }

  watch(heard: (account: string | null) => void): () => void {
    const own = (account: string | null) => heard(account);
    this.watchers.add(own);
    return () => void this.watchers.delete(own);
  }

  private queued<T>(work: () => T | Promise<T>): Promise<T> {
    const turn = this.turn.then(work, work);
    this.turn = turn.catch(() => undefined);
    return turn;
  }
}

/**
 * The values in `localStorage` (`storage`), as they were kept before
 * IndexedDB — the pin under {@link genesisKey}, the record under {@link
 * seenKey} — handed out and taken as one value: the fallback where IndexedDB
 * will not open. Not one step across pages: the caller holds the Web Lock
 * around `update`, and a page's stale copy can still lower the record. A pin
 * is never removed. The pages hear one another through `storage` events on
 * `events` (the window).
 */
export function localStorageSeenStore(storage: SeenStorage, events: EventTargetLike | null = defaultEvents()): SeenStore {
  const read = (account: string): unknown => {
    let genesis: string | null = null;
    let seen: unknown = null;
    try {
      genesis = storage.getItem(genesisKey(account));
      const raw = storage.getItem(seenKey(account));
      seen = raw === null ? null : JSON.parse(raw);
    } catch {
      // not readable: as if not there, as before
    }
    return genesis === null && seen === null ? undefined : { v: 1, genesis, seen };
  };
  return {
    atomic: false,
    async get(account) {
      return read(account);
    },
    async update(account, change) {
      const next = change(read(account)) as { genesis?: unknown; seen?: unknown } | undefined;
      if (next === undefined) return;
      try {
        if (typeof next.genesis === "string") storage.setItem(genesisKey(account), next.genesis);
        if (next.seen === null || next.seen === undefined) storage.removeItem(seenKey(account));
        else storage.setItem(seenKey(account), JSON.stringify(next.seen));
      } catch {
        // memory only for this tab
      }
    },
    watch(heard) {
      if (!events) return () => undefined;
      const prefixes = [seenKey(""), genesisKey("")];
      const onStorage = (event: Event) => {
        const key = (event as Event & { key?: string | null }).key;
        if (key === null || key === undefined) return heard(null);
        const prefix = prefixes.find((p) => key.startsWith(p));
        if (prefix) heard(key.slice(prefix.length));
      };
      events.addEventListener("storage", onStorage);
      return () => events.removeEventListener("storage", onStorage);
    },
  };
}

/** A store that will not be had at all here: every read and write fails with `why`, to be tried again (`SeenStoreError`). */
export function refusedSeenStore(why: string): SeenStore {
  const refuse = () => Promise.reject(new SeenStoreError(why));
  return { atomic: true, get: refuse, update: refuse, watch: () => () => undefined };
}

/** The window, where there is one. */
function defaultEvents(): EventTargetLike | null {
  const g = globalThis as { addEventListener?: unknown; removeEventListener?: unknown };
  return typeof g.addEventListener === "function" && typeof g.removeEventListener === "function" ? (globalThis as unknown as EventTargetLike) : null;
}

/** An open of IndexedDB that threw at once. */
class NotHere extends Error {}

/** An open that says IndexedDB is not to be had here at all (Firefox's private mode before 115, a sandboxed frame), not a failure that may pass. */
function unavailable(error: unknown): boolean {
  const name = (error as { name?: unknown } | null)?.name;
  return error instanceof NotHere || name === "InvalidStateError" || name === "SecurityError";
}

function describe(error: unknown): string {
  return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

export interface IndexedDbSeenOptions {
  factory: IDBFactory;
  /** The database's name ({@link SEEN_DB}). */
  dbName?: string;
  /** The channel other pages are told on ({@link SEEN_CHANNEL}; `null`: none). */
  channel?: string | null;
  /**
   * What to keep the values in instead when IndexedDB is not to be had here
   * at all (its first open says so): the `localStorage` fallback — or `null`,
   * none (the browser kept them in IndexedDB before), and the open's failure
   * stands, to be tried again.
   */
  fallback?: () => SeenStore | null;
  /** Told when the database opened in this page (the first time). */
  onOpen?: () => void;
}

/**
 * The records in IndexedDB (see the header). A connection the browser closes
 * (WebKit, when its storage process goes away), another page upgrading or
 * deleting the database, and an open that fails or never answers are let go
 * of: the next call opens it afresh. Only a first open that says IndexedDB is
 * not to be had here at all turns to `fallback`, for the rest of the page's
 * life — never once a connection opened (a page that read the record here
 * does not go on with an older copy of it).
 */
export function indexedDbSeenStore(opts: IndexedDbSeenOptions): SeenStore {
  const { factory, dbName = SEEN_DB } = opts;
  const channelName = opts.channel === undefined ? SEEN_CHANNEL : opts.channel;
  const canBroadcast = channelName !== null && typeof BroadcastChannel === "function";
  /** This store's own id on the channel: a page does not hear itself. */
  const self = Math.random().toString(36).slice(2);
  let opened = false;
  let fallen: SeenStore | null = null;
  let poster: BroadcastChannel | null = null;
  const watchers = new Set<(account: string | null) => void>();
  let listening: (() => void) | null = null;

  const idb = openIdb({
    factory,
    name: dbName,
    store: STORE,
    timeoutMs: SEEN_DB_OPEN_MS,
    timedOut: `the browser did not open the seen record's database within ${SEEN_DB_OPEN_MS / 1000} s`,
    refused: "the browser would not open the seen record's database",
    unref: true,
    // Thrown at once: not to be had here (an opaque origin, storage turned off).
    notHere: (error) => new NotHere(describe(error)),
  });

  /** The database, or `null` once this page keeps the records in the fallback. */
  const connection = async (): Promise<IDBDatabase | null> => {
    if (fallen) return null;
    try {
      const conn = await idb.open();
      if (!opened) opts.onOpen?.();
      opened = true;
      return conn;
    } catch (error) {
      if (opened || !opts.fallback || !unavailable(error)) throw error;
      if (!fallen) {
        const instead = opts.fallback();
        if (!instead) {
          throw new SeenStoreError(`IndexedDB is not to be had here (${describe(error)}), and this browser kept what it verified of the account there: it is not read from localStorage instead — try again`, { cause: error });
        }
        fallen = instead;
        console.warn(`seen record: IndexedDB is not to be had here (${describe(error)}): this page keeps the record in localStorage, where another page's stale copy can still lower it`);
        listening?.();
        listening = null;
        listen();
      }
      return null;
    }
  };

  /**
   * `run` on a new transaction of the store, handed it the moment it is made:
   * `run` asks for its first request before it awaits anything, so the
   * transaction never sits idle (WebKit commits an idle one). A connection
   * the browser is closing is let go of and opened afresh, once. `null`:
   * this page keeps the records in the fallback.
   */
  const inTransaction = async <T>(mode: IDBTransactionMode, run: (tx: IDBTransaction) => Promise<T>): Promise<{ result: T } | null> => {
    for (let tries = 0; ; tries++) {
      const conn = await connection();
      if (!conn) return null;
      let tx: IDBTransaction;
      try {
        tx = conn.transaction(STORE, mode);
      } catch (error) {
        letGo();
        if (tries > 0) throw error;
        continue;
      }
      return { result: await run(tx) };
    }
  };
  /** Let go of the connection (one that would not make a transaction, or whose request failed): the next call opens the database afresh. */
  const letGo = () => idb.reopen();

  const post = (account: string) => {
    if (!canBroadcast) return;
    try {
      poster ??= unref(new BroadcastChannel(channelName));
      poster.postMessage({ account, from: self });
    } catch (error) {
      // The other pages look again when they come back into view.
      console.warn("seen record: could not tell the other pages:", error);
    }
  };

  const dispatch = (account: string | null) => {
    for (const heard of [...watchers]) heard(account);
  };
  const listen = () => {
    if (listening || watchers.size === 0) return;
    if (fallen) {
      listening = fallen.watch(dispatch);
      return;
    }
    if (!canBroadcast) return;
    const channel = unref(new BroadcastChannel(channelName));
    channel.onmessage = (event: MessageEvent) => {
      const said = event.data as { account?: unknown; from?: unknown } | null;
      if (said && typeof said.account === "string" && said.from !== self) dispatch(said.account);
    };
    listening = () => channel.close();
  };

  return {
    get atomic() {
      return fallen ? fallen.atomic : true;
    },
    async get(account) {
      const done = await inTransaction(
        "readonly",
        (tx) =>
          new Promise<unknown>((resolve, reject) => {
            const req = tx.objectStore(STORE).get(account);
            req.onsuccess = () => resolve(req.result);
            req.onerror = () => {
              letGo();
              reject(req.error ?? new Error("the browser would not read the seen record"));
            };
          }),
      );
      return done ? done.result : fallen!.get(account);
    },
    async update(account, change) {
      // Everything from the read to the commit happens in callbacks of this
      // one transaction, without a wait: the read, `change`, the write.
      const done = await inTransaction(
        "readwrite",
        (tx) =>
          new Promise<boolean>((resolve, reject) => {
            const store = tx.objectStore(STORE);
            let failed: { error: unknown } | null = null;
            let wrote = false;
            const read = store.get(account);
            read.onsuccess = () => {
              try {
                const next = change(read.result);
                if (next === undefined) return;
                store.put(next, account);
                wrote = true;
              } catch (error) {
                failed = { error };
                try {
                  tx.abort();
                } catch {
                  // over already
                }
              }
            };
            tx.oncomplete = () => resolve(wrote);
            tx.onabort = () => {
              if (failed) return reject(failed.error);
              letGo();
              reject(tx.error ?? new Error("the seen record's transaction was aborted"));
            };
          }),
      );
      if (!done) return fallen!.update(account, change);
      if (done.result) post(account);
    },
    watch(heard) {
      const own = (account: string | null) => heard(account);
      watchers.add(own);
      listen();
      return () => {
        watchers.delete(own);
        if (watchers.size === 0) {
          listening?.();
          listening = null;
        }
      };
    },
  };
}

let pageStore: SeenStore | null = null;
let saidNoIndexedDb = false;

/**
 * This page's seen store: IndexedDB (one for the page, however many
 * accounts it opens), or `localStorage` (`storage`) where IndexedDB is not
 * to be had — said once — unless this browser kept its values in IndexedDB
 * before ({@link SEEN_IN_IDB}): then `localStorage`'s copies, older than
 * what IndexedDB holds, are not read instead, and every read fails, to be
 * tried again.
 */
export function pageSeenStore(storage: SeenStorage): SeenStore {
  const keptInIdb = () => {
    try {
      return storage.getItem(SEEN_IN_IDB) !== null;
    } catch {
      return false;
    }
  };
  let factory: IDBFactory | null = null;
  try {
    factory = (globalThis as { indexedDB?: IDBFactory | null }).indexedDB ?? null;
  } catch {
    // not allowed here
  }
  if (!factory) {
    if (keptInIdb()) return refusedSeenStore("IndexedDB is not to be had here, and this browser kept what it verified of the account there: it is not read from localStorage instead — try again");
    if (!saidNoIndexedDb) {
      saidNoIndexedDb = true;
      console.warn("seen record: no IndexedDB here: the record stays in localStorage, where another page's stale copy can still lower it");
    }
    return localStorageSeenStore(storage);
  }
  pageStore ??= indexedDbSeenStore({
    factory,
    fallback: () => (keptInIdb() ? null : localStorageSeenStore(storage)),
    onOpen: () => {
      try {
        storage.setItem(SEEN_IN_IDB, "1");
      } catch {
        // not kept: a later page with no IndexedDB falls back as before
      }
    },
  });
  return pageStore;
}

/** The most first sightings kept per account. */
export const MAX_FIRST_SEEN = 4096;
/** The most entry hashes the seen record keeps (`SeenRecord.heads`): how far behind a tab can still be checked against it. */
export const MAX_SEEN_HEADS = 256;
export interface SeenRecord {
  v: 1;
  seenThrough: number;
  head: string;
  /**
   * `[seq, hash]` of the newest entries up to `seenThrough` (at most
   * {@link MAX_SEEN_HEADS}): what lets a tab that is behind the record check
   * it is on the same chain at its own height. Absent in older records.
   */
  heads?: [number, string][];
  firstSeen: [number, number][];
}

/** `raw` as a seen record, if it is one (`null`: it is not). */
export function parseSeen(raw: unknown): SeenRecord | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Partial<SeenRecord>;
  if (r.v !== 1 || !Number.isSafeInteger(r.seenThrough) || r.seenThrough! < 0 || typeof r.head !== "string" || !Array.isArray(r.firstSeen)) return null;
  const firstSeen = r.firstSeen.filter((p): p is [number, number] => Array.isArray(p) && Number.isSafeInteger(p[0]) && Number.isFinite(p[1]));
  const heads = Array.isArray(r.heads) ? r.heads.filter((p): p is [number, string] => Array.isArray(p) && Number.isSafeInteger(p[0]) && typeof p[1] === "string") : undefined;
  return { v: 1, seenThrough: r.seenThrough!, head: r.head, ...(heads ? { heads } : {}), firstSeen };
}

/** A record as `localStorage` held it (JSON), if it is one. */
export function parseSeenJson(raw: string): SeenRecord | null {
  try {
    return parseSeen(JSON.parse(raw));
  } catch {
    return null;
  }
}

/** Both lists of first sightings as one: the earliest of each entry, the newest {@link MAX_FIRST_SEEN} entries kept. */
export function earliestSightings(a: Iterable<[number, number]>, b: Iterable<[number, number]>): [number, number][] {
  const firstSeen = new Map(a);
  for (const [seq, ms] of b) {
    const at = firstSeen.get(seq);
    if (at === undefined || ms < at) firstSeen.set(seq, ms);
  }
  return [...firstSeen.entries()].sort((x, y) => x[0] - y[0]).slice(-MAX_FIRST_SEEN);
}

/**
 * Two seen records of the account (`a` the store's) as one, where that is
 * proven: they agree where they overlap — the lower one's head is the higher
 * one's hash at that height — and then the higher, with the first sightings
 * of both (the earliest of each). Where it cannot be told (the higher keeps
 * no hash that far back, or predates `heads`), missing overlap is no
 * agreement: `a` stays the record and `b` is kept `apart`, a checkpoint every
 * view is held against as well, until a verified chain holds both. `forkAt`:
 * they do not agree.
 */
export function higherSeen(a: SeenRecord | null, b: SeenRecord | null): { record: SeenRecord | null; apart: SeenRecord | null } | { forkAt: number } {
  if (!a || !b) return { record: a ?? b, apart: null };
  const [lo, hi] = a.seenThrough < b.seenThrough ? [a, b] : [b, a];
  const theirs = lo.seenThrough === hi.seenThrough ? hi.head : hi.heads?.find(([seq]) => seq === lo.seenThrough)?.[1];
  if (theirs === undefined) return { record: a, apart: b };
  if (theirs !== lo.head) return { forkAt: lo.seenThrough };
  const heads = hi.heads ?? lo.heads;
  return { record: { v: 1, seenThrough: hi.seenThrough, head: hi.head, ...(heads ? { heads } : {}), firstSeen: earliestSightings(hi.firstSeen, lo.firstSeen) }, apart: null };
}

/** What the seen store keeps for an account: the genesis this browser pinned (hex), and what it verified of the chain under it. */
export interface StoredTrust {
  v: 1;
  genesis: string | null;
  seen: SeenRecord | null;
}

/** What the store handed back, as a {@link StoredTrust}. A `null` is a value the browser kept and would not hand back (nothing here stores `null`): never taken for nothing stored. */
export function parseStored(raw: unknown): StoredTrust {
  if (raw === undefined) return { v: 1, genesis: null, seen: null };
  if (raw === null) throw new SeenStoreError("the browser handed back nothing for what it verified of the account: it is left as it is — try again");
  const r = (typeof raw === "object" ? raw : {}) as { genesis?: unknown; seen?: unknown };
  return { v: 1, genesis: typeof r.genesis === "string" ? r.genesis : null, seen: parseSeen(r.seen) };
}

/** Whether two stored values say the same (both as `parseStored`, `seenNext` or `higherSeen` lay them out). */
export function sameStored(a: StoredTrust, b: StoredTrust): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * What this browser holds for the account now: the genesis it pinned, the
 * seen record, and an older page's record that could not be proven one chain
 * with it (`apart`, see {@link higherSeen}) — every view is held against both.
 */
export interface Held {
  genesis: string | null;
  record: SeenRecord | null;
  apart: SeenRecord | null;
}

/** The checkpoints `held` holds: the record, and the one kept apart. */
export function checkpoints(held: Held): SeenRecord[] {
  return [held.record, held.apart].filter((c): c is SeenRecord => c !== null);
}

export function describeError(error: unknown): string {
  return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

/**
 * What `state` makes of the seen record `stored`, which only moves forward:
 * a view behind it (a page left open while another went on) keeps the
 * newer checkpoint and never lowers it — else the next load of the newer
 * page would take a chain without the entries only it had verified (a
 * removal, a renewal) as all this device saw. Where this view and the record
 * overlap they must be one chain: the record's head is in this view, or
 * (this view behind it) this view's head is the record's hash at that height
 * — else they fork (`forkAt`). `checked`: whether they could be held against
 * each other at all (not for a view more than {@link MAX_SEEN_HEADS} behind,
 * or a record from before `heads`).
 */
export function compareSeen(state: ChainState, stored: SeenRecord | null): { record: Omit<SeenRecord, "firstSeen">; checked: boolean } | { forkAt: number } {
  if (stored && stored.seenThrough > state.seq) {
    const theirs = stored.heads?.find(([seq]) => seq === state.seq)?.[1];
    if (theirs !== undefined && theirs !== hex(state.head)) return { forkAt: state.seq };
    return { record: { v: 1, seenThrough: stored.seenThrough, head: stored.head, ...(stored.heads ? { heads: stored.heads } : {}) }, checked: theirs !== undefined };
  }
  if (stored && hex(state.hashAt(stored.seenThrough)!) !== stored.head) return { forkAt: stored.seenThrough };
  const heads: [number, string][] = [];
  for (let seq = Math.max(0, state.seq - MAX_SEEN_HEADS + 1); seq <= state.seq; seq++) heads.push([seq, hex(state.hashAt(seq)!)]);
  return { record: { v: 1, seenThrough: state.seq, head: hex(state.head), heads }, checked: true };
}

/**
 * The record to write for `record`, with a view's first sightings (`clock`)
 * merged into `stored`'s — the earliest of each entry kept, never a later
 * page's — when the view and the record were held against each other
 * (`checked`; else the record's own are kept as they are).
 */
export function seenNext({ record, checked }: { record: Omit<SeenRecord, "firstSeen">; checked: boolean }, stored: SeenRecord | null, clock: ChainClock): SeenRecord {
  return { ...record, firstSeen: earliestSightings(stored?.firstSeen ?? [], checked ? clock.firstSeen : []) };
}

export interface AckRecord {
  through: number;
  extra: number[];
}
