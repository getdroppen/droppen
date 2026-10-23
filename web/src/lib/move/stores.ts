/*
 * The move's own access to the two IndexedDB stores it copies: everything in
 * one, read without ever creating the database (a page that only asks must
 * not leave an empty one behind, the interstitial tells "a device here" by
 * it); a bundle written in one transaction, what it replaced handed back so a
 * failed move puts it back; a store emptied, a database deleted.
 *
 * Each call opens its own connection and closes it when done, and lets go at
 * once when another page wants the database upgraded or deleted.
 */
import { Unmovable } from "./digest";

/** One record: its name (always a string here) and its value as stored. */
export interface StoredRecord {
  key: string;
  value: unknown;
}

/** What a write replaced: the record as it was (`existed: false`: there was none). */
export interface PriorRecord {
  key: string;
  existed: boolean;
  value?: unknown;
}

/** One moved object store (see `allowlist.ts`). */
export interface RecordStore {
  readonly db: string;
  readonly store: string;
  /** Every record, by name; `null` when the database is not there (nothing is created). */
  readAll(): Promise<StoredRecord[] | null>;
  /** The named records as they are now (`existed: false`: none of that name), the database created if it is missing. */
  read(keys: readonly string[]): Promise<PriorRecord[]>;
  /**
   * Write `records` in one transaction (creating the database at `version`
   * if it is missing): over what is there (`overwrite`), or only where there
   * is no record of that name (`ifAbsent`). Resolves, once committed, with
   * every record it touched as it was before.
   */
  write(records: readonly StoredRecord[], mode: "overwrite" | "ifAbsent", version: number): Promise<PriorRecord[]>;
  /** Put back what a {@link write} replaced, in one transaction. */
  restore(prior: readonly PriorRecord[]): Promise<void>;
  /** Remove every record, in one transaction (nothing when the database is not there). */
  clear(): Promise<void>;
  /** Delete the database, best effort: never waits more than `withinMs` on another page that keeps it open. */
  destroy(withinMs: number): Promise<void>;
}

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("the browser's store failed"));
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onabort = () => reject(tx.error ?? new Error("the browser aborted the write"));
    tx.onerror = () => undefined;
  });
}

function watch(conn: IDBDatabase): IDBDatabase {
  conn.onversionchange = () => conn.close();
  return conn;
}

/** The database if it is there (`null` if not: the open that would create it is aborted, so nothing is made). */
function openExisting(factory: IDBFactory, db: string): Promise<IDBDatabase | null> {
  return new Promise((resolve, reject) => {
    let missing = false;
    const req = factory.open(db);
    req.onupgradeneeded = (event) => {
      if (event.oldVersion === 0) {
        missing = true;
        req.transaction?.abort();
      }
    };
    req.onsuccess = () => resolve(watch(req.result));
    req.onerror = (event) => {
      if (missing) {
        event.preventDefault();
        resolve(null);
      } else reject(req.error ?? new Error(`the browser would not open ${db}`));
    };
    req.onblocked = () => undefined;
  });
}

/** The database at `version` at least, its store created where it is missing. */
function openForWrite(factory: IDBFactory, db: string, store: string, version: number): Promise<IDBDatabase> {
  return openExisting(factory, db).then((conn) => {
    if (conn && conn.objectStoreNames.contains(store)) return conn;
    const at = conn ? conn.version + 1 : version;
    conn?.close();
    return new Promise<IDBDatabase>((resolve, reject) => {
      const req = factory.open(db, at);
      req.onupgradeneeded = () => {
        if (!req.result.objectStoreNames.contains(store)) req.result.createObjectStore(store);
      };
      req.onsuccess = () => resolve(watch(req.result));
      req.onerror = () => reject(req.error ?? new Error(`the browser would not open ${db}`));
    });
  });
}

function checkShape(os: IDBObjectStore): void {
  if (os.keyPath !== null || os.autoIncrement || os.indexNames.length !== 0) throw new Unmovable(`the store ${os.name} is not one the move knows (key path, generator or indexes)`);
}

/** {@link RecordStore} over IndexedDB `factory`. */
export function idbRecordStore(factory: IDBFactory, db: string, store: string): RecordStore {
  return {
    db,
    store,
    async readAll() {
      const conn = await openExisting(factory, db);
      if (!conn) return null;
      try {
        if (!conn.objectStoreNames.contains(store)) return [];
        const os = conn.transaction(store, "readonly").objectStore(store);
        checkShape(os);
        const [keys, values] = await Promise.all([request(os.getAllKeys()), request(os.getAll())]);
        if (keys.length !== values.length) throw new Error(`the store ${store} changed while it was read`);
        return keys.map((key, i) => {
          if (typeof key !== "string") throw new Unmovable(`a record of ${store} not named by a string`);
          return { key, value: values[i] };
        });
      } finally {
        conn.close();
      }
    },
    async read(keys) {
      const conn = await openForWrite(factory, db, store, 1);
      try {
        const os = conn.transaction(store, "readonly").objectStore(store);
        return await Promise.all(
          keys.map(async (key) => {
            const [count, value] = await Promise.all([request(os.count(key)), request(os.get(key))]);
            return count > 0 ? { key, existed: true, value } : { key, existed: false };
          }),
        );
      } finally {
        conn.close();
      }
    },
    async write(records, mode, version) {
      const conn = await openForWrite(factory, db, store, version);
      try {
        // On disk before it resolves: the app host answers "verified" only after this, and the sender then deletes its copy.
        const tx = conn.transaction(store, "readwrite", { durability: "strict" });
        const os = tx.objectStore(store);
        checkShape(os);
        const prior: PriorRecord[] = [];
        const finished = done(tx);
        // Every request is made inside the transaction's own callbacks: it never commits half-way.
        for (const r of records) {
          const count = os.count(r.key);
          count.onsuccess = () => {
            const existed = count.result > 0;
            if (!existed) {
              prior.push({ key: r.key, existed: false });
              os.put(r.value, r.key);
              return;
            }
            const get = os.get(r.key);
            get.onsuccess = () => {
              if (mode === "ifAbsent") return;
              prior.push({ key: r.key, existed: true, value: get.result });
              os.put(r.value, r.key);
            };
          };
        }
        await finished;
        return prior;
      } finally {
        conn.close();
      }
    },
    async restore(prior) {
      if (!prior.length) return;
      const conn = await openForWrite(factory, db, store, 1);
      try {
        const tx = conn.transaction(store, "readwrite", { durability: "strict" });
        const os = tx.objectStore(store);
        const finished = done(tx);
        for (const p of prior) {
          if (p.existed) os.put(p.value, p.key);
          else os.delete(p.key);
        }
        await finished;
      } finally {
        conn.close();
      }
    },
    async clear() {
      const conn = await openExisting(factory, db);
      if (!conn) return;
      try {
        if (!conn.objectStoreNames.contains(store)) return;
        const tx = conn.transaction(store, "readwrite");
        const finished = done(tx);
        tx.objectStore(store).clear();
        await finished;
      } finally {
        conn.close();
      }
    },
    destroy(withinMs) {
      return new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, withinMs);
        try {
          const req = factory.deleteDatabase(db);
          req.onsuccess = req.onerror = () => {
            clearTimeout(timer);
            resolve();
          };
        } catch {
          clearTimeout(timer);
          resolve();
        }
      });
    },
  };
}

/** {@link RecordStore} in memory: tests, and a page without IndexedDB (which then has nothing to move). */
export function memoryRecordStore(db: string, store: string, initial?: Map<string, unknown>): RecordStore & { data: Map<string, unknown> | null } {
  const self = {
    db,
    store,
    data: initial ?? (null as Map<string, unknown> | null),
    async readAll() {
      return self.data ? [...self.data.entries()].sort(([a], [b]) => (a < b ? -1 : 1)).map(([key, value]) => ({ key, value })) : null;
    },
    async read(keys: readonly string[]) {
      self.data ??= new Map();
      const data = self.data;
      return keys.map((key) => (data.has(key) ? { key, existed: true, value: data.get(key) } : { key, existed: false }));
    },
    async write(records: readonly StoredRecord[], mode: "overwrite" | "ifAbsent") {
      self.data ??= new Map();
      const prior: PriorRecord[] = [];
      for (const r of records) {
        const existed = self.data.has(r.key);
        if (existed && mode === "ifAbsent") continue;
        prior.push(existed ? { key: r.key, existed, value: self.data.get(r.key) } : { key: r.key, existed });
        self.data.set(r.key, r.value);
      }
      return prior;
    },
    async restore(prior: readonly PriorRecord[]) {
      self.data ??= new Map();
      for (const p of prior) {
        if (p.existed) self.data.set(p.key, p.value);
        else self.data.delete(p.key);
      }
    },
    async clear() {
      self.data?.clear();
    },
    async destroy() {
      self.data = null;
    },
  };
  return self;
}
