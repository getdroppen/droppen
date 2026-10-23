/**
 * A small key-value store in IndexedDB, for what the page draws before the
 * network has answered: the session list, and the last few transcripts read.
 *
 * Everything in it is a copy of something the server will send again, so every
 * failure here — no IndexedDB (some private modes), a quota, a blocked upgrade,
 * a database that never answers — reads as a miss within {@link CACHE_WAIT_MS}
 * and costs nothing but the instant first paint.
 *
 * It holds transcript text, so it is the signed-in user's and nobody else's:
 * {@link cacheClear} (on logout, on a dead token) empties it and *seals* it, so
 * that a save already scheduled by a feed or the session list — which carry on
 * until the page has actually unloaded — cannot put anything back. Only a new
 * login unseals it ({@link cacheUnseal}). {@link cacheWipe} is the same clear
 * for a sign-out that must know it happened (cloud mode): never cut short, and
 * a failure is an error rather than a miss.
 */

const DB = "rh-local-cache";
const STORE = "kv";
/** The database and store the cache is kept in (the key move empties them where the keys left). */
export const LOCAL_CACHE_DB = DB;
export const LOCAL_CACHE_STORE = STORE;

/**
 * How long any one operation may take before it counts as a miss. An IndexedDB
 * that hangs — a stuck upgrade in another tab, a browser bug — must never hold
 * up what is waiting on it, which is a transcript's first paint.
 */
export const CACHE_WAIT_MS = 300;

let opening: Promise<IDBDatabase | null> | null = null;
let sealed = false;

/** The store could not be emptied. */
export class CacheClearError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "CacheClearError";
  }
}

function within<T>(promise: Promise<T>, fallback: T, ms: number): Promise<T> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(fallback), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      () => {
        clearTimeout(timer);
        resolve(fallback);
      },
    );
  });
}

function db(): Promise<IDBDatabase | null> {
  if (opening) return opening;
  opening = new Promise((resolve) => {
    try {
      if (typeof indexedDB === "undefined") return resolve(null);
      const request = indexedDB.open(DB, 1);
      request.onupgradeneeded = () => {
        if (!request.result.objectStoreNames.contains(STORE)) request.result.createObjectStore(STORE);
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => resolve(null);
      request.onblocked = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
  return opening;
}

/**
 * Run `act` in one transaction and resolve with what it hands to `done`, or
 * `undefined` on any failure. A read-modify-write is one call: everything `act`
 * starts happens in the same transaction.
 */
export function cacheTransact<T>(
  mode: IDBTransactionMode,
  act: (store: IDBObjectStore, done: (value: T) => void) => void,
  ms: number = CACHE_WAIT_MS,
): Promise<T | undefined> {
  if (mode === "readwrite" && sealed) return Promise.resolve(undefined);
  const work = db().then(
    (database) =>
      new Promise<T | undefined>((resolve) => {
        if (!database || (mode === "readwrite" && sealed)) return resolve(undefined);
        try {
          let result: T | undefined;
          const tx = database.transaction(STORE, mode);
          tx.oncomplete = () => resolve(result);
          tx.onerror = () => resolve(undefined);
          tx.onabort = () => resolve(undefined);
          act(tx.objectStore(STORE), (value) => {
            result = value;
          });
        } catch {
          resolve(undefined);
        }
      }),
  );
  return within(work, undefined, ms);
}

export function cacheGet<T>(key: string): Promise<T | undefined> {
  return cacheTransact<T>("readonly", (store, done) => {
    const request = store.get(key);
    request.onsuccess = () => done(request.result as T);
  });
}

export function cacheSet(key: string, value: unknown): Promise<void> {
  return cacheTransact<void>("readwrite", (store) => {
    store.put(value, key);
  }).then(() => undefined);
}

/**
 * Empty the store and refuse every write until {@link cacheUnseal}. The seal is
 * immediate; the promise is the clear itself landing, which a caller about to
 * navigate away should wait for (it is bounded like everything else here, and
 * never rejects: see {@link cacheWipe} for a clear that says when it failed).
 */
export function cacheClear(): Promise<void> {
  return within(
    cacheWipe().catch(() => undefined),
    undefined,
    1000,
  );
}

/**
 * {@link cacheClear}, for a sign-out that must know: sealed at once, then the
 * whole store emptied — waited for however long it takes, with no deadline —
 * and a {@link CacheClearError} if it could not be. A page with no IndexedDB
 * at all has nothing to clear.
 */
export async function cacheWipe(): Promise<void> {
  sealed = true;
  if (typeof indexedDB === "undefined") return;
  let database = await db();
  if (!database) {
    // The open failed before (or was blocked): one fresh attempt, for this.
    opening = null;
    database = await db();
  }
  if (!database) throw new CacheClearError("the device cache could not be opened");
  const open = database;
  await new Promise<void>((resolve, reject) => {
    try {
      const tx = open.transaction(STORE, "readwrite");
      tx.objectStore(STORE).clear();
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(new CacheClearError(`the device cache could not be cleared${tx.error ? `: ${tx.error.message}` : ""}`, { cause: tx.error }));
      tx.onabort = () => reject(new CacheClearError(`clearing the device cache was aborted${tx.error ? `: ${tx.error.message}` : ""}`, { cause: tx.error }));
    } catch (error) {
      reject(new CacheClearError(`the device cache could not be cleared: ${error instanceof Error ? error.message : String(error)}`, { cause: error }));
    }
  });
}

/** A new login: the store is the new user's to fill. */
export function cacheUnseal(): void {
  sealed = false;
}

/** Whether writes are being refused. */
export function cacheSealed(): boolean {
  return sealed;
}
