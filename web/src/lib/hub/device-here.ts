/*
 * Whether this browser keeps a device of a Droppen account on this origin —
 * told without making anything: no database is created and none is
 * upgraded, so a browser that never used the app here leaves no trace of
 * having been asked (the move notice, `features/hosts/legacy.tsx`, asks
 * before it sends such a browser on to the app host).
 *
 * A device is:
 * - a device id this browser noted for an account
 *   (`rh.hub.device.<account>`), or a kept device session
 *   (`rh.hub.session`), in localStorage; or
 * - device keys in the key store (`rh-e2e`, records `device-keys/v2` or the
 *   older `device-keys/v1`), read only where `indexedDB.databases()` lists
 *   the database, and in a transaction that is aborted should the database
 *   turn out not to exist after all.
 *
 * What "remove this browser" leaves behind does not count: an emptied key
 * store, and the account's seen record (`rh-hub-seen`, which keeps the
 * chain's pin and outlives a removal).
 */
import { type HubSession, loadSession } from "./api";

export const KEY_DB = "rh-e2e";
const KEY_STORE = "keys";
const DEVICE_RECORDS = ["device-keys/v2", "device-keys/v1"];
const DEVICE_NOTE = "rh.hub.device.";
const SESSION_NOTE = "rh.hub.session";

/** How long the key store may take to say whether it holds device keys. */
const IDB_WAIT_MS = 3_000;

export interface DeviceHere {
  /** Whether this browser keeps a device here. */
  held: boolean;
  /** The accounts it noted a device id for, the kept session's first. */
  accounts: string[];
  /** The kept device session, if any (it may have expired). */
  session: HubSession | null;
}

type NoteStorage = Pick<Storage, "getItem" | "key" | "length">;

export interface DeviceHereEnv {
  storage: NoteStorage | null;
  idb: Pick<IDBFactory, "open"> & { databases?: () => Promise<IDBDatabaseInfo[]> } | null;
}

function browserEnv(): DeviceHereEnv {
  let storage: NoteStorage | null = null;
  try {
    storage = globalThis.localStorage ?? null;
  } catch {
    storage = null;
  }
  let idb: DeviceHereEnv["idb"] = null;
  try {
    idb = globalThis.indexedDB ?? null;
  } catch {
    idb = null;
  }
  return { storage, idb };
}

/** The device id this browser noted for `account`, if any. */
export function notedDevice(account: string, storage: Pick<Storage, "getItem"> | null = browserEnv().storage): string | null {
  try {
    return storage?.getItem(`${DEVICE_NOTE}${account}`) ?? null;
  } catch {
    return null;
  }
}

function notes(storage: NoteStorage | null): { accounts: string[]; session: HubSession | null; sessionKept: boolean } {
  const accounts: string[] = [];
  let sessionKept = false;
  let session: HubSession | null = null;
  if (!storage) return { accounts, session, sessionKept };
  try {
    for (let i = 0; i < storage.length; i++) {
      const key = storage.key(i);
      if (key === null) continue;
      if (key === SESSION_NOTE) sessionKept = true;
      else if (key.startsWith(DEVICE_NOTE) && key.length > DEVICE_NOTE.length) accounts.push(key.slice(DEVICE_NOTE.length));
    }
    session = loadSession(storage as Storage);
  } catch {
    // storage not readable: what was read so far
  }
  if (session) {
    const at = accounts.indexOf(session.account);
    if (at > 0) accounts.splice(at, 1);
    if (at !== 0) accounts.unshift(session.account);
  }
  return { accounts, session, sessionKept };
}

/** Whether the key store holds device keys, opening it only if the browser lists it, and never creating it. */
async function keysStored(idb: DeviceHereEnv["idb"]): Promise<boolean> {
  if (!idb || typeof idb.databases !== "function") return false;
  let listed: IDBDatabaseInfo[];
  try {
    listed = await idb.databases();
  } catch {
    return false;
  }
  if (!listed.some((db) => db.name === KEY_DB)) return false;
  return await new Promise<boolean>((resolve) => {
    let done = false;
    const settle = (value: boolean, db?: IDBDatabase) => {
      db?.close();
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolve(value);
    };
    const timer = setTimeout(() => settle(false), IDB_WAIT_MS);
    let req: IDBOpenDBRequest;
    try {
      req = idb.open(KEY_DB);
    } catch {
      return settle(false);
    }
    // Gone since it was listed: the open would create it. It is not.
    req.onupgradeneeded = () => {
      try {
        req.transaction?.abort();
      } catch {
        // the request fails all the same
      }
    };
    req.onerror = () => settle(false);
    req.onblocked = () => settle(false);
    req.onsuccess = () => {
      const db = req.result;
      if (done) return db.close();
      if (!db.objectStoreNames.contains(KEY_STORE)) return settle(false, db);
      try {
        const tx = db.transaction(KEY_STORE, "readonly");
        const store = tx.objectStore(KEY_STORE);
        let found = false;
        let pending = DEVICE_RECORDS.length;
        for (const name of DEVICE_RECORDS) {
          const count = store.count(name);
          count.onsuccess = () => {
            if (count.result > 0) found = true;
            if (--pending === 0) settle(found, db);
          };
          count.onerror = () => {
            if (--pending === 0) settle(found, db);
          };
        }
      } catch {
        settle(false, db);
      }
    };
  });
}

/** Whether this browser keeps a device here ({@link DeviceHere}). Never throws. */
export async function deviceHere(env: DeviceHereEnv = browserEnv()): Promise<DeviceHere> {
  const { accounts, session, sessionKept } = notes(env.storage);
  if (accounts.length > 0 || sessionKept) return { held: true, accounts, session };
  return { held: await keysStored(env.idb), accounts, session };
}
