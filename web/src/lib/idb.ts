/*
 * Opening one IndexedDB database the way the stores that must not silently
 * lose what they keep open theirs (`lib/e2e/keystore.ts` and
 * `lib/hub/seen-store.ts`): a connection the browser closes (WebKit, when its
 * storage process goes away), another page upgrading or deleting the
 * database, and an open that fails or never answers are let go of, so the
 * next call opens the database afresh instead of every call failing for the
 * rest of the page's life.
 *
 * `lib/local-cache.ts` keeps its own best-effort policy: a cache that cannot
 * be opened is a cache that misses, not a store that fails.
 */

/** How one database is opened, and the words its failures use. */
export interface OpenIdbOptions {
  factory: IDBFactory;
  /** The database's name. */
  name: string;
  /** The object store made when the database is first created. */
  store: string;
  /** How long opening may take before that open counts as failed (the next call tries again). */
  timeoutMs: number;
  /** The error when the open does not answer in time. */
  timedOut: string;
  /** The error when the browser refuses the open (`req.error` absent). */
  refused: string;
  /** The database was made for the first time (nothing was there before). */
  onFirstOpen?: () => void;
  /**
   * An open that throws at once is made this error: a database that is not to
   * be had here at all (an opaque origin, storage turned off), which a caller
   * may treat differently from a failure that can be tried again.
   */
  notHere?: (error: unknown) => Error;
  /** Do not let the timeout hold a test process open. */
  unref?: boolean;
}

/** A database opened once and kept; `reopen` drops it, closing the connection. */
export interface IdbOpener {
  /** The connection, opened on first ask and kept until it closes or another page upgrades or deletes the database. */
  open(): Promise<IDBDatabase>;
  /** Drop the kept connection (a later open starts afresh), closing it. */
  reopen(): void;
}

export function openIdb(opts: OpenIdbOptions): IdbOpener {
  let db: Promise<IDBDatabase> | null = null;
  const drop = (which: Promise<IDBDatabase>) => {
    if (db === which) db = null;
  };
  const open = (): Promise<IDBDatabase> => {
    if (db) return db;
    const opening: Promise<IDBDatabase> = new Promise((resolve, reject) => {
      let late = false;
      const timer = setTimeout(() => {
        late = true;
        reject(new Error(opts.timedOut));
      }, opts.timeoutMs);
      if (opts.unref) (timer as { unref?: () => void }).unref?.();
      let req: IDBOpenDBRequest;
      try {
        req = opts.factory.open(opts.name, 1);
      } catch (error) {
        clearTimeout(timer);
        reject(opts.notHere ? opts.notHere(error) : error);
        return;
      }
      req.onupgradeneeded = (event) => {
        if (event.oldVersion === 0) opts.onFirstOpen?.();
        req.result.createObjectStore(opts.store);
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
        reject(req.error ?? new Error(opts.refused));
      };
    });
    opening.catch(() => drop(opening));
    db = opening;
    return opening;
  };
  return {
    open,
    reopen() {
      const current = db;
      db = null;
      void current?.then(
        (conn) => conn.close(),
        () => undefined,
      );
    },
  };
}
