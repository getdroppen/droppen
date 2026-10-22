/*
 * Where this browser keeps the seen record of each account it is a device
 * of: what it verified of the account's chain (`CloudAccount`), shared by
 * every page of the browser, and moving forward only (#172, #182).
 *
 * Moving forward only makes every write a read-compare-write, and that has to
 * be one step across pages (#183). IndexedDB `readwrite` transactions whose
 * scopes overlap run one after another across every page of the origin, each
 * seeing what the one before it committed: a compare done inside the
 * transaction is against the record as it is, not against a page's own copy
 * of it — which is all `localStorage` gives (each page reads its own copy,
 * brought into step by the browser only after the fact). The compare runs
 * synchronously inside the transaction, between its read and its write:
 * nothing in it waits on anything else, so it never commits half-way (WebKit
 * commits a transaction once the page yields with no request of it pending).
 *
 * IndexedDB has no change events: a page that writes a record says so on a
 * `BroadcastChannel`, and the pages open on the account hold it against what
 * they verified (`CloudAccount.watchSeenRecord`).
 *
 * Where IndexedDB is not there or will not open at all (some private modes),
 * the record stays in `localStorage` as before: read-compare-written under a
 * Web Lock, which a page's stale copy can still lower, and its pages hear one
 * another through `storage` events. That is said on the console.
 */

/** Where the record was kept before (and still is, where IndexedDB will not open): `localStorage`'s key for `account`. */
export const seenKey = (account: string) => `rh.hub.seen.${account}`;
/** The IndexedDB database the records are in (one object store, keyed by account): a database of its own, so the key store's is never upgraded for it. */
export const SEEN_DB = "rh-hub-seen";
/** The `BroadcastChannel` a page says on that it wrote an account's record. */
export const SEEN_CHANNEL = "rh.hub.seen";
/** How long opening the database may take before that open counts as failed (the next call tries again). */
export const SEEN_DB_OPEN_MS = 10_000;
const STORE = "seen";

/** `localStorage`, or a stand-in. */
export type SeenStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

type EventTargetLike = Pick<EventTarget, "addEventListener" | "removeEventListener">;

/** The browser's seen records, one per account, as stored (`CloudAccount` checks what it reads). */
export interface SeenStore {
  /**
   * Whether `update` is one step across every page of the browser (IndexedDB,
   * memory) — or only as good as the Web Lock the caller holds around it and
   * the page's own copy of `localStorage` (the fallback).
   */
  readonly atomic: boolean;
  /** `account`'s record as stored: `undefined` when there is none. */
  get(account: string): Promise<unknown>;
  /**
   * Read `account`'s record and write what `change` makes of it, as one step
   * no other page's update comes between. `change` is handed the record as
   * `get` would give it and runs synchronously, inside the step; it returns
   * what to store — `undefined`: leave it as it is; `null`: delete it — or
   * throws, and then nothing is written and the update rejects with what it
   * threw. Resolves once what was written is committed; the other pages are
   * told then.
   */
  update(account: string, change: (stored: unknown) => unknown): Promise<void>;
  /** Hear that another page may have written a record: its account, or `null` for any. Returns the stop. */
  watch(heard: (account: string | null) => void): () => void;
}

const unref = <T>(handle: T): T => {
  (handle as { unref?: () => void }).unref?.();
  return handle;
};

/**
 * The records in memory: one browser's, for the tests. Updates run one after
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
      if (next === null) this.values.delete(account);
      else this.values.set(account, structuredClone(next));
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
 * The records in `localStorage` (`storage`), as they were kept before
 * IndexedDB: the fallback where it will not open. Not one step across pages
 * — the caller holds the Web Lock around `update`, and a page's stale copy
 * can still lower the record — and the pages hear one another through
 * `storage` events on `events` (the window).
 */
export function localStorageSeenStore(storage: SeenStorage, events: EventTargetLike | null = defaultEvents()): SeenStore {
  const read = (account: string): unknown => {
    try {
      const raw = storage.getItem(seenKey(account));
      return raw === null ? undefined : JSON.parse(raw);
    } catch {
      return undefined;
    }
  };
  return {
    atomic: false,
    async get(account) {
      return read(account);
    },
    async update(account, change) {
      const next = change(read(account));
      if (next === undefined) return;
      try {
        if (next === null) storage.removeItem(seenKey(account));
        else storage.setItem(seenKey(account), JSON.stringify(next));
      } catch {
        // memory only for this tab
      }
    },
    watch(heard) {
      if (!events) return () => undefined;
      const prefix = seenKey("");
      const onStorage = (event: Event) => {
        const key = (event as Event & { key?: string | null }).key;
        if (key === null || key === undefined) heard(null);
        else if (key.startsWith(prefix)) heard(key.slice(prefix.length));
      };
      events.addEventListener("storage", onStorage);
      return () => events.removeEventListener("storage", onStorage);
    },
  };
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
  /** What to keep the records in instead when IndexedDB is not to be had here at all (its first open says so): the `localStorage` fallback. */
  fallback?: () => SeenStore;
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
  let db: Promise<IDBDatabase> | null = null;
  let opened = false;
  let fallen: SeenStore | null = null;
  let poster: BroadcastChannel | null = null;
  const watchers = new Set<(account: string | null) => void>();
  let listening: (() => void) | null = null;

  const drop = (which: Promise<IDBDatabase>) => {
    if (db === which) db = null;
  };
  const open = (): Promise<IDBDatabase> => {
    if (db) return db;
    const opening = new Promise<IDBDatabase>((resolve, reject) => {
      let late = false;
      const timer = unref(
        setTimeout(() => {
          late = true;
          reject(new Error(`the browser did not open the seen record's database within ${SEEN_DB_OPEN_MS / 1000} s`));
        }, SEEN_DB_OPEN_MS),
      );
      let req: IDBOpenDBRequest;
      try {
        req = factory.open(dbName, 1);
      } catch (error) {
        clearTimeout(timer);
        // Thrown at once: not to be had here (an opaque origin, storage turned off).
        reject(new NotHere(describe(error)));
        return;
      }
      req.onupgradeneeded = () => {
        req.result.createObjectStore(STORE);
      };
      req.onsuccess = () => {
        clearTimeout(timer);
        const conn = req.result;
        if (late) return conn.close();
        conn.onversionchange = () => {
          conn.close();
          drop(opening);
        };
        conn.onclose = () => drop(opening);
        resolve(conn);
      };
      req.onerror = () => {
        clearTimeout(timer);
        reject(req.error ?? new Error("the browser would not open the seen record's database"));
      };
    });
    opening.catch(() => drop(opening));
    db = opening;
    return opening;
  };

  /** The database, or `null` once this page keeps the records in the fallback. */
  const connection = async (): Promise<IDBDatabase | null> => {
    if (fallen) return null;
    try {
      const conn = await open();
      opened = true;
      return conn;
    } catch (error) {
      if (opened || !opts.fallback || !unavailable(error)) throw error;
      if (!fallen) {
        fallen = opts.fallback();
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
  const letGo = () => {
    const current = db;
    db = null;
    void current?.then(
      (conn) => conn.close(),
      () => undefined,
    );
  };

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
                if (next === null) store.delete(account);
                else store.put(next, account);
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
 * to be had — said once.
 */
export function pageSeenStore(storage: SeenStorage): SeenStore {
  let factory: IDBFactory | null = null;
  try {
    factory = (globalThis as { indexedDB?: IDBFactory | null }).indexedDB ?? null;
  } catch {
    // not allowed here
  }
  if (!factory) {
    if (!saidNoIndexedDb) {
      saidNoIndexedDb = true;
      console.warn("seen record: no IndexedDB here: the record stays in localStorage, where another page's stale copy can still lower it");
    }
    return localStorageSeenStore(storage);
  }
  pageStore ??= indexedDbSeenStore({ factory, fallback: () => localStorageSeenStore(storage) });
  return pageStore;
}
