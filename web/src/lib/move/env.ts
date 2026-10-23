/*
 * Everything the move touches of the page, in one place: its origin, its
 * windows, its storage, Web Locks, the cross-tab channel and the hub. The
 * page's own by default (`browserEnv`); tests hand in their own.
 */
import { indexedDbKeyStore, type KeyStore, MemoryKeyStore } from "@/lib/e2e/keystore";
import { hubOrigin } from "@/lib/hub/config";
import type { MoveEvent } from "./types";
import { idbRecordStore, memoryRecordStore, type RecordStore } from "./stores";

/** The other side's window, as this page holds it. */
export interface PeerWindow {
  postMessage(message: unknown, targetOrigin: string): void;
  readonly closed: boolean;
  close(): void;
}

/** A message event, as far as the move reads one. */
export interface PeerMessage {
  origin: string;
  source: unknown;
  data: unknown;
}

/** `localStorage`, listable. */
export type MoveStorage = Pick<Storage, "getItem" | "setItem" | "removeItem" | "key" | "length">;

/** `navigator.locks`, as far as the move uses it. */
export interface MoveLocks {
  request<T>(name: string, options: { mode: "exclusive"; ifAvailable?: boolean }, callback: (lock: unknown) => Promise<T> | T): Promise<T>;
  query?(): Promise<{ held?: { name?: string }[] }>;
}

export interface MoveEnv {
  /** This page's origin (`location.origin`). */
  origin: string;
  storage: MoveStorage;
  /** One moved IndexedDB store. */
  records(db: string, store: string): RecordStore;
  /** The key store, on a fresh connection (the device keys are read back through it). */
  keyStore(): KeyStore;
  locks: MoveLocks | null;
  /** This origin's other tabs. */
  tabs: { post(event: MoveEvent): void; subscribe(fn: (event: MoveEvent) => void): () => void } | null;
  fetch: (input: string, init?: RequestInit) => Promise<Response>;
  /** The hub origin this page's sign-ins name (`wss://…`, the shell's marker). */
  hub: string | null;
  /** `window.open`: `null` when the browser would not open it. */
  open(url: string, name: string): PeerWindow | null;
  /** The window that opened this one (`null`: none, or `noopener`). */
  opener(): PeerWindow | null;
  /** Hear every `message` event of this window. */
  onMessage(fn: (event: PeerMessage) => void): () => void;
  /** Close this window (a popup closes itself once it is done). */
  closeSelf(): void;
  /** Whether a click is being handled now (`null`: the browser does not say). */
  userActive(): boolean | null;
  /** Stop this origin's push subscription, best effort (the legacy origin, after a move). */
  dropPushSubscription(): Promise<void>;
}

/** The channel the tabs of one origin tell each other about a move on. */
export const MOVE_CHANNEL = "rh.hub.move";

function pageTabs(): MoveEnv["tabs"] {
  const Channel = (globalThis as { BroadcastChannel?: typeof BroadcastChannel }).BroadcastChannel;
  if (!Channel) return null;
  let poster: BroadcastChannel | null = null;
  return {
    post(event) {
      try {
        poster ??= new Channel(MOVE_CHANNEL);
        poster.postMessage(event);
      } catch {
        // the others find out as they reload
      }
    },
    subscribe(fn) {
      const channel = new Channel(MOVE_CHANNEL);
      channel.onmessage = (e: MessageEvent) => fn(e.data as MoveEvent);
      return () => channel.close();
    },
  };
}

function pageLocks(): MoveLocks | null {
  try {
    const locks = (globalThis.navigator as { locks?: MoveLocks } | undefined)?.locks;
    return locks && typeof locks.request === "function" ? locks : null;
  } catch {
    return null;
  }
}

function pageStorage(): MoveStorage {
  try {
    if (globalThis.localStorage) return globalThis.localStorage;
  } catch {
    // not allowed here
  }
  const values = new Map<string, string>();
  return {
    getItem: (k) => values.get(k) ?? null,
    setItem: (k, v) => void values.set(k, String(v)),
    removeItem: (k) => void values.delete(k),
    key: (i) => [...values.keys()][i] ?? null,
    get length() {
      return values.size;
    },
  };
}

/** The page's own. */
export function browserEnv(): MoveEnv {
  const idb = (globalThis as { indexedDB?: IDBFactory | null }).indexedDB ?? null;
  return {
    origin: location.origin,
    storage: pageStorage(),
    records: (db, store) => (idb ? idbRecordStore(idb, db, store) : memoryRecordStore(db, store)),
    keyStore: () => (idb ? indexedDbKeyStore(idb) : new MemoryKeyStore()),
    locks: pageLocks(),
    tabs: pageTabs(),
    fetch: (input, init) => globalThis.fetch(input, init),
    hub: hubOrigin(),
    open: (url, name) => window.open(url, name, "popup,width=480,height=640"),
    opener: () => (window.opener as PeerWindow | null) ?? null,
    onMessage(fn) {
      const listener = (e: MessageEvent) => fn({ origin: e.origin, source: e.source, data: e.data });
      window.addEventListener("message", listener);
      return () => window.removeEventListener("message", listener);
    },
    closeSelf: () => window.close(),
    userActive: () => {
      const ua = (navigator as { userActivation?: { isActive: boolean } }).userActivation;
      return ua ? ua.isActive : null;
    },
    async dropPushSubscription() {
      const sw = (navigator as { serviceWorker?: ServiceWorkerContainer }).serviceWorker;
      const reg = await sw?.getRegistration?.();
      const sub = await reg?.pushManager?.getSubscription();
      await sub?.unsubscribe();
    },
  };
}

/** Every `localStorage` entry whose name `keep` keeps. */
export function localEntries(storage: MoveStorage, keep: (name: string) => boolean): { key: string; value: string }[] {
  const out: { key: string; value: string }[] = [];
  const names: string[] = [];
  for (let i = 0; i < storage.length; i++) {
    const name = storage.key(i);
    if (name !== null && keep(name)) names.push(name);
  }
  for (const key of names.sort()) {
    const value = storage.getItem(key);
    if (value !== null) out.push({ key, value });
  }
  return out;
}
