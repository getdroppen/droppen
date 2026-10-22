/*
 * Where a device keeps its keys: IndexedDB.
 *
 * A `CryptoKey` is structured-cloneable, so IndexedDB can store the key itself
 * — including a non-extractable one — and hand back a key that still cannot be
 * exported. (`localStorage` can only hold strings, which would mean
 * extractable keys.) The device keys were stored that way until WebKit turned
 * out to lose non-extractable X25519 keys: they now sit in IndexedDB as
 * PKCS #8 sealed under a non-extractable AES-GCM key, which WebKit keeps, and
 * are imported non-extractable on load. Their bytes exist only while they are
 * made and while they are opened.
 *
 * A read that does not give the keys is not taken for keys that are gone
 * unless the store answered and holds none (`readDeviceKeys`): on iOS a
 * reload after a while in the background has handed back nothing for keys
 * that were there, and making new ones over them then loses the device for
 * good.
 *
 * None of this keeps them from code running in this origin: it can read the
 * wrap key from the store and open the PKCS #8 exactly as `openV2` does.
 * (Keys still kept the old way, as `CryptoKey`s in `device-keys/v1`, cannot
 * be read out; such code can use them all the same.) The defence against it
 * is trusted delivery of the page (docs/web-integrity.md), not WebCrypto.
 *
 * The store is an interface so tests and the node runner can use memory.
 */
import { type Bytes, utf8 } from "./bytes";
import type { DeviceKeys } from "./keys";
import { isAlgorithmUnsupported, unsupportedFrom } from "./support";

export interface KeyStore {
  get(name: string): Promise<unknown>;
  put(name: string, value: unknown): Promise<void>;
  delete(name: string): Promise<void>;
  /** Let go of a connection that may have gone bad (IndexedDB): the next call opens the store afresh. */
  reopen?(): void;
  /** Whether this page found the store missing and created it empty (IndexedDB: a database made just now). */
  createdAfresh?(): boolean;
  /** The names of the records whose name starts with `prefix`. */
  list?(prefix: string): Promise<string[]>;
}

export class MemoryKeyStore implements KeyStore {
  private readonly values = new Map<string, unknown>();
  async get(name: string): Promise<unknown> {
    return this.values.get(name);
  }
  async put(name: string, value: unknown): Promise<void> {
    this.values.set(name, value);
  }
  async delete(name: string): Promise<void> {
    this.values.delete(name);
  }
  async list(prefix: string): Promise<string[]> {
    return [...this.values.keys()].filter((name) => name.startsWith(prefix));
  }
}

const STORE = "keys";

function request<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

/** How long opening the key store may take before that open counts as failed (the next call tries again). */
export const KEY_STORE_OPEN_MS = 10_000;

/**
 * A key store in IndexedDB database `dbName`. A connection the browser closes
 * (WebKit, when its storage process goes away), another page upgrading or
 * deleting the database, and an open that fails or never answers are let go
 * of: the next call opens the database afresh, instead of every call failing
 * for the rest of the page's life.
 */
export function indexedDbKeyStore(factory: IDBFactory = indexedDB, dbName = "rh-e2e"): KeyStore {
  let db: Promise<IDBDatabase> | null = null;
  let afresh = false;
  const drop = (which: Promise<IDBDatabase>) => {
    if (db === which) db = null;
  };
  const open = (): Promise<IDBDatabase> => {
    if (db) return db;
    const opening: Promise<IDBDatabase> = new Promise((resolve, reject) => {
      let late = false;
      const timer = setTimeout(() => {
        late = true;
        reject(new Error(`the browser did not open its key store within ${KEY_STORE_OPEN_MS / 1000} s`));
      }, KEY_STORE_OPEN_MS);
      const req = factory.open(dbName, 1);
      req.onupgradeneeded = (event) => {
        if (event.oldVersion === 0) afresh = true;
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
        reject(req.error ?? new Error("the browser would not open its key store"));
      };
    });
    opening.catch(() => drop(opening));
    db = opening;
    return opening;
  };
  const tx = async (mode: IDBTransactionMode) => (await open()).transaction(STORE, mode).objectStore(STORE);
  return {
    async get(name) {
      return request((await tx("readonly")).get(name));
    },
    async put(name, value) {
      await request((await tx("readwrite")).put(value, name));
    },
    async delete(name) {
      await request((await tx("readwrite")).delete(name));
    },
    reopen() {
      const current = db;
      db = null;
      void current?.then(
        (conn) => conn.close(),
        () => undefined,
      );
    },
    createdAfresh: () => afresh,
    async list(prefix) {
      const keys = await request((await tx("readonly")).getAllKeys(IDBKeyRange.bound(prefix, `${prefix}\uffff`)));
      return keys.filter((k): k is string => typeof k === "string" && k.startsWith(prefix));
    },
  };
}

/** The keys as `CryptoKey` objects: what every build wrote until WebKit was found to lose them. Still read. */
const DEVICE_KEYS = "device-keys/v1";
/**
 * The keys as PKCS #8, each sealed under {@link DEVICE_WRAP}. WebKit (Safari
 * 26, and so every iOS browser) hands back `null` for a record holding a
 * non-extractable X25519 `CryptoKey` (https://bugs.webkit.org/show_bug.cgi?id=312279),
 * while non-extractable AES-GCM keys and plain bytes come back intact.
 */
const DEVICE_KEYS_V2 = "device-keys/v2";
/**
 * The non-extractable AES-GCM key {@link DEVICE_KEYS_V2} is sealed under —
 * unless the record names another (`wrapName`): keys made over keys that
 * would not open, when this one was handed back as nothing, get a seal key
 * of their own, so the old one is never overwritten.
 */
const DEVICE_WRAP = "device-keys/wrap";
/** Every record the device keys are kept in starts so. */
const DEVICE_KEYS_PREFIX = "device-keys/";
/** Keys replaced at the person's word, kept aside (`<prefix><time>`), at most {@link KEPT_ASIDE} of them. */
const PREV_PREFIX = "device-keys/v2.prev-";
const KEPT_ASIDE = 2;

interface StoredDeviceKeys extends DeviceKeys {
  version: 1;
  createdAt: number;
}

interface Sealed {
  iv: Bytes;
  sealed: Bytes;
}

interface StoredDeviceKeysV2 {
  version: 2;
  createdAt: number;
  signPublic: Bytes;
  dhPublic: Bytes;
  sign: Sealed;
  dh: Sealed;
  /** The record of the key they are sealed under, when not {@link DEVICE_WRAP}. */
  wrapName?: string;
}

function isStored(value: unknown): value is StoredDeviceKeys {
  const v = value as Partial<StoredDeviceKeys> | undefined;
  return !!v && v.version === 1 && !!v.sign?.privateKey && !!v.dh?.privateKey && v.signPublic instanceof Uint8Array && v.dhPublic instanceof Uint8Array;
}

function isSealed(value: unknown): value is Sealed {
  const v = value as Partial<Sealed> | undefined;
  return !!v && v.iv instanceof Uint8Array && v.iv.length === 12 && v.sealed instanceof Uint8Array;
}

function isStoredV2(value: unknown): value is StoredDeviceKeysV2 {
  const v = value as Partial<StoredDeviceKeysV2> | undefined;
  return (
    !!v &&
    v.version === 2 &&
    v.signPublic instanceof Uint8Array &&
    v.dhPublic instanceof Uint8Array &&
    isSealed(v.sign) &&
    isSealed(v.dh) &&
    (v.wrapName === undefined || (typeof v.wrapName === "string" && v.wrapName.startsWith(DEVICE_KEYS_PREFIX)))
  );
}

/** Whether two stored records hold the same: their bytes, numbers and strings, field by field. */
function sameRecord(a: unknown, b: unknown): boolean {
  if (a instanceof Uint8Array || b instanceof Uint8Array) return a instanceof Uint8Array && b instanceof Uint8Array && a.length === b.length && a.every((x, i) => x === b[i]);
  if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return Object.is(a, b);
  const ka = Object.keys(a).sort();
  const kb = Object.keys(b).sort();
  return ka.length === kb.length && ka.every((k, i) => k === kb[i] && sameRecord((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]));
}

/** The record of the key `stored` is sealed under. */
const wrapNameOf = (stored: StoredDeviceKeysV2) => stored.wrapName ?? DEVICE_WRAP;

function isWrapKey(value: unknown): value is CryptoKey {
  return typeof CryptoKey !== "undefined" && value instanceof CryptoKey && value.algorithm.name === "AES-GCM";
}

/** What each sealed key is bound to, so one cannot be swapped for the other. */
function sealContext(which: "sign" | "dh"): Bytes {
  return utf8(`rh/v1 device key\0${which}`);
}

async function importPrivate(which: "sign" | "dh", pkcs8: Bytes): Promise<CryptoKey> {
  return which === "sign"
    ? crypto.subtle.importKey("pkcs8", pkcs8, { name: "Ed25519" }, false, ["sign"])
    : crypto.subtle.importKey("pkcs8", pkcs8, { name: "X25519" }, false, ["deriveBits"]);
}

async function importPublic(which: "sign" | "dh", raw: Bytes): Promise<CryptoKey> {
  return which === "sign" ? crypto.subtle.importKey("raw", raw, { name: "Ed25519" }, true, ["verify"]) : crypto.subtle.importKey("raw", raw, { name: "X25519" }, true, []);
}

/** The stored keys opened under `wrap`, private halves non-extractable; `null` when they do not open. */
async function openV2(wrap: CryptoKey, stored: StoredDeviceKeysV2): Promise<DeviceKeys | null> {
  try {
    const open = async (which: "sign" | "dh") => {
      const sealed = stored[which];
      const pkcs8 = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: sealed.iv, additionalData: sealContext(which) }, wrap, sealed.sealed));
      try {
        return await importPrivate(which, pkcs8);
      } finally {
        pkcs8.fill(0);
      }
    };
    return {
      sign: { privateKey: await open("sign"), publicKey: await importPublic("sign", stored.signPublic) },
      dh: { privateKey: await open("dh"), publicKey: await importPublic("dh", stored.dhPublic) },
      signPublic: stored.signPublic,
      dhPublic: stored.dhPublic,
    };
  } catch (error) {
    // A browser that does not know the algorithms: not keys that did not open (`readDeviceKeys` says so).
    if (isAlgorithmUnsupported(error)) throw error;
    return null;
  }
}

/**
 * Whether the private halves are the ones the public halves name: a signature
 * verifies, and an X25519 agreement with a throwaway key comes out the same
 * from both sides. Cheap, and the only way to tell keys a store mangled from
 * keys it kept.
 */
async function keysAgree(keys: DeviceKeys): Promise<boolean> {
  try {
    const msg = crypto.getRandomValues(new Uint8Array(32));
    const sig = await crypto.subtle.sign({ name: "Ed25519" }, keys.sign.privateKey, msg);
    if (!(await crypto.subtle.verify({ name: "Ed25519" }, await importPublic("sign", keys.signPublic), sig, msg))) return false;
    const other = (await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    const ours = new Uint8Array(await crypto.subtle.deriveBits({ name: "X25519", public: other.publicKey }, keys.dh.privateKey, 256));
    const theirs = new Uint8Array(await crypto.subtle.deriveBits({ name: "X25519", public: await importPublic("dh", keys.dhPublic) }, other.privateKey, 256));
    return ours.length === theirs.length && ours.every((b, i) => b === theirs[i]);
  } catch (error) {
    if (isAlgorithmUnsupported(error)) throw error;
    return false;
  }
}

/**
 * New keys, stored as {@link DEVICE_KEYS_V2}: generated extractable only to be
 * sealed, then imported back non-extractable for use. Their PKCS #8 bytes live
 * in this function alone and are zeroed before it returns.
 *
 * Over keys that would not open (`replacing`), nothing is destroyed: the old
 * record is copied aside first (`device-keys/v2.prev-<time>`), and when its
 * seal key is handed back as nothing the new keys get a seal key under a
 * record of their own (`device-keys/wrap-<time>`), the old one left as it is.
 */
async function createV2(store: KeyStore, now: () => number, replacing = false): Promise<DeviceKeys> {
  const at = now();
  // Made before anything is written or set aside: a browser that cannot make
  // them (no Ed25519 or X25519, `support.ts`) leaves the store as it was.
  const signPair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const dhPair = (await crypto.subtle.generateKey({ name: "X25519" }, true, ["deriveBits"])) as CryptoKeyPair;
  const old = replacing ? await store.get(DEVICE_KEYS_V2) : undefined;
  // Kept but handed back as nothing: it cannot be kept aside, so it is not overwritten either.
  if (old === null) throw new DeviceKeysUnreadable("stored", "the browser handed back nothing for the stored device keys, so they cannot be kept aside and are not replaced");
  if (old !== undefined) {
    const byAge = (names: string[]) => names.sort((a, b) => Number(b.slice(PREV_PREFIX.length)) - Number(a.slice(PREV_PREFIX.length)));
    const aside = byAge((await store.list?.(PREV_PREFIX)) ?? []);
    // The same keys kept aside already (replaced again before they ever opened): not copied twice.
    const newest = aside.length ? await store.get(aside[0]!) : undefined;
    if (!sameRecord(newest, old)) {
      await store.put(`${PREV_PREFIX}${at}`, old);
      aside.unshift(`${PREV_PREFIX}${at}`);
    }
    // The newest two are kept aside; older ones go.
    for (const name of aside.slice(KEPT_ASIDE)) await store.delete(name);
  }
  const oldWrapName = isStoredV2(old) ? wrapNameOf(old) : DEVICE_WRAP;
  let wrapName = oldWrapName;
  let wrap = await store.get(oldWrapName);
  if (!isWrapKey(wrap)) {
    // Handed back as nothing: kept, so never overwritten.
    if (wrap !== undefined) wrapName = `${DEVICE_WRAP}-${at}`;
    wrap = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
    await store.put(wrapName, wrap);
  }
  const wrapKey = wrap as CryptoKey;
  const signPublic = new Uint8Array(await crypto.subtle.exportKey("raw", signPair.publicKey));
  const dhPublic = new Uint8Array(await crypto.subtle.exportKey("raw", dhPair.publicKey));
  const seal = async (which: "sign" | "dh", key: CryptoKey) => {
    const pkcs8 = new Uint8Array(await crypto.subtle.exportKey("pkcs8", key));
    try {
      const iv = crypto.getRandomValues(new Uint8Array(12));
      const sealed = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: sealContext(which) }, wrapKey, pkcs8));
      return { sealed: { iv, sealed }, key: await importPrivate(which, pkcs8) };
    } finally {
      pkcs8.fill(0);
    }
  };
  const sign = await seal("sign", signPair.privateKey);
  const dh = await seal("dh", dhPair.privateKey);
  const stored: StoredDeviceKeysV2 = { version: 2, createdAt: at, signPublic, dhPublic, sign: sign.sealed, dh: dh.sealed, ...(wrapName === DEVICE_WRAP ? {} : { wrapName }) };
  await store.put(DEVICE_KEYS_V2, stored);
  // Whatever was here before was unusable (or there was nothing): it must not be read again instead of these.
  await store.delete(DEVICE_KEYS);
  return {
    sign: { privateKey: sign.key, publicKey: await importPublic("sign", signPublic) },
    dh: { privateKey: dh.key, publicKey: await importPublic("dh", dhPublic) },
    signPublic,
    dhPublic,
  };
}

/** Run `work` holding this origin's Web Lock `name`, so two tabs never do it at once (no Web Locks: run it). */
export async function underKeyStoreLock<T>(name: string, work: () => Promise<T>): Promise<T> {
  const locks = (globalThis.navigator as Navigator | undefined)?.locks;
  if (!locks?.request) return work();
  return locks.request(name, work) as Promise<T>;
}

/**
 * What reading the device keys found:
 * - `found`: keys that open and agree with their public halves;
 * - `none`: the store answered and holds no device keys (never made here, or
 *   the browser emptied the store: `afresh` when this page found the whole
 *   store missing and created it);
 * - `unreadable`: something is stored that did not open (`stored`), or the
 *   store did not answer. That may pass — WebKit hands back `null` for a
 *   stored `CryptoKey` it cannot read back at that moment, a connection its
 *   storage process dropped fails every read — so it is never taken for keys
 *   that are gone.
 */
export type DeviceKeysRead =
  | { kind: "found"; keys: DeviceKeys }
  | { kind: "none"; afresh: boolean }
  /** `nullKey`: what did not come back is the seal key, a stored `CryptoKey`, handed back as nothing (see {@link storeKeepsCryptoKeys}). */
  | { kind: "unreadable"; why: string; stored: boolean; nullKey?: boolean };

function failure(error: unknown): string {
  return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

/** One read of the device keys, nothing written (see {@link DeviceKeysRead}). */
export async function readDeviceKeys(store: KeyStore): Promise<DeviceKeysRead> {
  try {
    const v2 = await store.get(DEVICE_KEYS_V2);
    // Nothing here is ever stored as `null`: a `null` is a record the browser
    // kept but could not hand back (WebKit's answer for a value it cannot
    // deserialize), not one that is missing (`undefined`).
    if (v2 !== undefined) {
      if (!isStoredV2(v2)) return { kind: "unreadable", stored: true, why: v2 === null ? "the browser handed back nothing for the stored device keys" : "the stored device keys are not in a form this page knows" };
      const wrap = await store.get(wrapNameOf(v2));
      if (wrap === null) return { kind: "unreadable", stored: true, why: "the browser handed back nothing for the key that seals the device keys", nullKey: true };
      if (!isWrapKey(wrap)) return { kind: "unreadable", stored: true, why: wrap === undefined ? "the key that seals the device keys is missing" : "the key that seals the device keys is not an AES-GCM key" };
      const keys = await openV2(wrap, v2);
      if (!keys) return { kind: "unreadable", stored: true, why: "the stored device keys did not open under the key that seals them" };
      if (!(await keysAgree(keys))) return { kind: "unreadable", stored: true, why: "the stored device keys do not agree with their public halves" };
      return { kind: "found", keys };
    }
    const v1 = await store.get(DEVICE_KEYS);
    if (isStored(v1) && (await keysAgree(v1))) return { kind: "found", keys: v1 };
    // Keys kept the old way that WebKit handed back as `null` are gone for
    // good (https://bugs.webkit.org/show_bug.cgi?id=312279: it always does),
    // as are ones that do not agree: there is nothing to wait for.
    return { kind: "none", afresh: store.createdAfresh?.() ?? false };
  } catch (error) {
    // Not the store: the browser does not know the keys' algorithms, and no read ever opens them (`support.ts`).
    if (isAlgorithmUnsupported(error)) throw await unsupportedFrom(error);
    return { kind: "unreadable", stored: false, why: `the browser's key store failed: ${failure(error)}` };
  }
}

const PROBE = "device-keys/probe";

/**
 * Whether `store` keeps a non-extractable AES-GCM `CryptoKey` across a write
 * and a read right now: one is made, stored, read back, used and deleted.
 *
 * WebKit seals every `CryptoKey` it stores in IndexedDB under a master key of
 * the app's, kept in the app's keychain, and hands back `null` for one it
 * cannot unseal. When that happens to a stored key while a new key goes in
 * and comes back fine, the master key it was sealed under is gone (the app
 * made a new one, as after the simulator's `keychain reset`): it will never
 * open again. When the new key does not come back either, the master key is
 * out of reach for now — a read later may well work.
 */
export async function storeKeepsCryptoKeys(store: KeyStore): Promise<boolean> {
  try {
    const key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
    await store.put(PROBE, key);
    let back: unknown;
    try {
      back = await store.get(PROBE);
    } finally {
      await store.delete(PROBE).catch(() => undefined);
    }
    if (!isWrapKey(back)) return false;
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const msg = crypto.getRandomValues(new Uint8Array(16));
    const opened = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv }, back, await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, msg)));
    return opened.length === msg.length && opened.every((b, i) => b === msg[i]);
  } catch {
    return false;
  }
}

/**
 * Why the device keys did not come: `stored`, keys are stored but would not
 * open (they may yet); `gone`, their seal key will never open again (handed
 * back as nothing while the store keeps new keys fine: the browser's key for
 * stored keys changed); `store`, the browser's key store did not answer, or
 * would not take new keys.
 */
export type UnreadableReason = "stored" | "gone" | "store";

/** The device keys did not come, and nothing was replaced (see {@link UnreadableReason}). */
export class DeviceKeysUnreadable extends Error {
  constructor(
    readonly reason: UnreadableReason,
    readonly why: string,
  ) {
    super(`this browser could not open its device keys: ${why}`);
    this.name = "DeviceKeysUnreadable";
  }
}

/** The waits before each further read of a store that did not give the keys (each on a fresh connection). */
export const DEVICE_KEYS_RETRY_MS: readonly number[] = [250, 1_000, 2_500];

/**
 * Record `name` of `store`, read again on a fresh connection (after each of
 * `retryMs`) while it is kept but handed back as nothing (`null`) or the read
 * fails: what the last read gave (it throws if that one failed too).
 */
export async function readKept(store: KeyStore, name: string, retryMs: readonly number[] = DEVICE_KEYS_RETRY_MS): Promise<unknown> {
  const once = () => store.get(name).then((value) => ({ value }), (error: unknown) => ({ error }));
  let read = await once();
  for (const ms of retryMs) {
    if ("value" in read && read.value !== null) break;
    store.reopen?.();
    await new Promise((resolve) => setTimeout(resolve, ms));
    read = await once();
  }
  if ("error" in read) throw read.error;
  return read.value;
}

export interface LoadDeviceKeysOptions {
  now?: () => number;
  /**
   * This browser holds a device id registered with keys it made: a store that
   * reads empty is read again, on a fresh connection, before new keys are
   * made (an unreadable one always is).
   */
  expected?: boolean;
  /** Keys that are stored but will not open are replaced with new ones (the person chose to use this browser as a new device). */
  replaceUnreadable?: boolean;
  /** The waits between reads ({@link DEVICE_KEYS_RETRY_MS}). */
  retryMs?: readonly number[];
}

export interface LoadedDeviceKeys {
  keys: DeviceKeys;
  /** Made just now: there were none (or, asked to, the ones stored would not open). */
  created: boolean;
  /** With `created`: what the store held instead, in a few words (for the console and the "lost the keys" notice). */
  instead?: string;
}

/**
 * `read` (unreadable) as the error to say it with — `gone` when its seal key
 * provably never opens again — or, read once more (`reread`) after the store
 * showed it keeps new keys, the keys after all.
 */
async function diagnose(
  store: KeyStore,
  read: Extract<DeviceKeysRead, { kind: "unreadable" }>,
  reread: () => Promise<DeviceKeysRead>,
): Promise<{ keys: DeviceKeys } | DeviceKeysUnreadable> {
  if (read.nullKey && (await storeKeepsCryptoKeys(store))) {
    // The store keeps new keys: one more read (under the lock) before saying these never open again — it may give them.
    const again = await reread();
    if (again.kind === "found") return { keys: again.keys };
    if (again.kind === "unreadable" && again.nullKey) return new DeviceKeysUnreadable("gone", GONE_WHY);
    if (again.kind === "unreadable") return new DeviceKeysUnreadable(again.stored ? "stored" : "store", again.why);
    return new DeviceKeysUnreadable("store", "the device keys went away while they were being read");
  }
  return new DeviceKeysUnreadable(read.stored ? "stored" : "store", read.why);
}

const GONE_WHY = "the browser no longer opens the key that seals the device keys, though it keeps new keys: the key it protects stored keys with changed, so they will not come back";

/**
 * This device's keys: the stored ones when they open and agree with their
 * public halves; new ones (`created`) only when the store answers and holds
 * none. A caller holding a device id for the old keys must not go on signing
 * for it with new ones — the hub refuses such a device for good
 * (`CloudAccount.open` registers a new device instead).
 *
 * Keys that are stored but do not open are never replaced on their own: the
 * store is read again on a fresh connection (`retryMs`), and if they still do
 * not open a {@link DeviceKeysUnreadable} says why — even when they never will
 * (`gone`): a person decides. Only `replaceUnreadable` makes new keys over
 * them, and even then the old record is kept aside (`createV2`). New keys the
 * store will not take are a `DeviceKeysUnreadable` too (`store`). A browser
 * that does not know Ed25519 or X25519 is neither: an `E2eUnsupportedError`
 * (`support.ts`), since no retry and no new device ever helps it.
 */
export async function loadDeviceKeys(store: KeyStore, opts: LoadDeviceKeysOptions = {}): Promise<LoadedDeviceKeys> {
  const { now = Date.now, expected = false, replaceUnreadable = false, retryMs = DEVICE_KEYS_RETRY_MS } = opts;
  const settled = (read: DeviceKeysRead) => read.kind === "found" || (read.kind === "none" && !expected);
  let read = await readDeviceKeys(store);
  for (const ms of retryMs) {
    if (settled(read)) break;
    store.reopen?.();
    await new Promise((resolve) => setTimeout(resolve, ms));
    read = await readDeviceKeys(store);
  }
  if (read.kind === "found") return { keys: read.keys, created: false };
  if (read.kind === "unreadable" && !replaceUnreadable) {
    const said = await diagnose(store, read, () => underKeyStoreLock("rh-device-keys", () => readDeviceKeys(store)));
    if (said instanceof DeviceKeysUnreadable) throw said;
    return { keys: said.keys, created: false };
  }
  return underKeyStoreLock("rh-device-keys", async () => {
    // Another tab may have made them while this one waited for the lock.
    const last = await readDeviceKeys(store);
    if (last.kind === "found") return { keys: last.keys, created: false };
    let instead =
      last.kind === "none" && last.afresh ? "this browser's key store was gone: the browser had emptied the site's storage" : "no device keys were stored in this browser";
    if (last.kind === "unreadable") {
      // Already under the lock: read once more as is.
      const said = await diagnose(store, last, () => readDeviceKeys(store));
      if (!(said instanceof DeviceKeysUnreadable)) return { keys: said.keys, created: false };
      if (!replaceUnreadable) throw said;
      instead = `replaced keys that would not open (${said.why})`;
    }
    let keys: DeviceKeys;
    try {
      keys = await createV2(store, now, last.kind === "unreadable");
    } catch (error) {
      if (error instanceof DeviceKeysUnreadable) throw error;
      // A browser that does not know the algorithms makes no keys, now or on a retry: not the store's failure (`support.ts`).
      if (isAlgorithmUnsupported(error)) throw await unsupportedFrom(error);
      // An outage takes no new keys either (WebKit: DataCloneError): the same screen, not a bare error.
      throw new DeviceKeysUnreadable("store", `the browser would not store new keys: ${failure(error)}`);
    }
    return { keys, created: true, instead };
  });
}

/** This device's keys, generated on first use and kept ({@link loadDeviceKeys} without saying which). */
export async function loadOrCreateDeviceKeys(store: KeyStore, now: () => number = Date.now): Promise<DeviceKeys> {
  return (await loadDeviceKeys(store, { now })).keys;
}

/**
 * Ask the browser to keep this site's storage — the device keys among it —
 * out of its eviction under storage pressure (`navigator.storage.persist()`).
 * Best effort: whether it was granted (`null`: the browser has no such thing,
 * would not say, or is not asked). WebKit decides by itself (it grants a site
 * added to the home screen) and, granted or not, still clears a site's
 * storage after seven days without a visit unless it runs from the home
 * screen; Chromium decides by itself too. Firefox asks the person in a prompt
 * of its own, so it is never asked (the owner's call): only engines whose
 * user agent names AppleWebKit — WebKit, every iOS browser, Chromium.
 */
export async function keepStorage(
  storage: StorageManager | undefined = globalThis.navigator?.storage,
  userAgent: string = globalThis.navigator?.userAgent ?? "",
): Promise<boolean | null> {
  if (!storage?.persist || !/\bAppleWebKit\//.test(userAgent)) return null;
  try {
    if (storage.persisted && (await storage.persisted())) return true;
    return await storage.persist();
  } catch {
    return null;
  }
}

/**
 * Forget this device's keys, in every form — the copies kept aside and their
 * seal keys too (a reset; the chain still lists the old ones until a device
 * removes them).
 */
export async function forgetDeviceKeys(store: KeyStore): Promise<void> {
  const names = new Set([DEVICE_KEYS, DEVICE_KEYS_V2, DEVICE_WRAP, ...((await store.list?.(DEVICE_KEYS_PREFIX)) ?? [])]);
  for (const name of names) await store.delete(name);
}
