/*
 * Everything the move touches of the page, in one place: its origin, its
 * windows, its storage, Web Locks, the cross-tab channel and the hub. The
 * page's own by default (`browserEnv`); tests hand in their own.
 */
import { indexedDbKeyStore, type KeyStore, MemoryKeyStore } from "@/lib/e2e/keystore";
import { hubOrigin } from "@/lib/hub/config";
import { moveOrigins } from "@/lib/hub/hosts";
import type { MoveEvent, MoveOrigins } from "./types";
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
  /** The move's two origins as this page's hub pinned them (its own origin, its app host); `null`: it names no app host. */
  pinned: MoveOrigins | null;
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

/**
 * This origin's other tabs, over {@link MOVE_CHANNEL}. A page does not hear
 * itself: what it says is marked with an id of its own, and every page's
 * listeners skip it (one page may listen more than once).
 */
export function pageTabs(): MoveEnv["tabs"] {
  const Channel = (globalThis as { BroadcastChannel?: typeof BroadcastChannel }).BroadcastChannel;
  if (!Channel) return null;
  const self = Math.random().toString(36).slice(2);
  let poster: BroadcastChannel | null = null;
  return {
    post(event) {
      try {
        if (!poster) {
          poster = new Channel(MOVE_CHANNEL);
          (poster as { unref?: () => void }).unref?.();
        }
        poster.postMessage({ from: self, event });
      } catch {
        // the others find out as they reload
      }
    },
    subscribe(fn) {
      const channel = new Channel(MOVE_CHANNEL);
      // Node's channels keep a process alive (tests); a browser's have no such thing.
      (channel as { unref?: () => void }).unref?.();
      channel.onmessage = (e: MessageEvent) => {
        const m = e.data as { from?: unknown; event?: MoveEvent } | null;
        if (m && m.from !== self && m.event && typeof m.event.t === "string") fn(m.event);
      };
      return () => channel.close();
    },
  };
}

/** One per page: its own tabs channel, so its own words are known as its own. */
let pageTabsOnce: MoveEnv["tabs"] | undefined;

/** This page's {@link pageTabs}, the same every time it is asked for. */
export function sharedTabs(): MoveEnv["tabs"] {
  return (pageTabsOnce ??= pageTabs());
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

/**
 * The hub's own origin and its app host as `/.well-known/droppen-hub` said
 * them, held to the apex this page came from (`lib/hub/hosts.ts`
 * `moveOrigins`): `null` on any other page, or when the hub names no app host.
 */
export function pinnedOrigins(): MoveOrigins | null {
  return moveOrigins();
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
    tabs: sharedTabs(),
    fetch: (input, init) => globalThis.fetch(input, init),
    hub: hubOrigin(),
    pinned: pinnedOrigins(),
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
