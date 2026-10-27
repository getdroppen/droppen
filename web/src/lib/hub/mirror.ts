/*
 * One machine's offline copy, kept on this device: what the page shows when
 * the machine is asleep, and what it draws at once while the machine is being
 * reached.
 *
 * It follows the machine's copies by name through a `CopySource`
 * (`copy-source.ts`; on droppen.app the hub's encrypted streams, opened here)
 * — the index and the status always, and the sessions somebody here opened
 * plus the few most recently updated (kept warm in the background, so that
 * the copy is there before it is needed) — and keeps what they say in
 * IndexedDB (`lib/local-cache.ts`).
 *
 * Every stream's copy is one record: the decrypted state *and* the encoded
 * signed head it stands at, written in one transaction. Never the two apart —
 * a position saved without its data (or the other way round) is how a copy
 * resumes from the wrong place and silently skips or repeats records. And the
 * position is the head the machine signed, not a bare seq,
 * so a copy read back restarts with `LogReader.fromHead` and nothing about it
 * is trusted unless the machine's signature still checks.
 *
 * - Keys: `stream/<account>/<machine>/<stream hex>`, and per machine a
 *   `streammeta/…` record listing the sessions opened on this device (most
 *   recent first). A record names its owner and machine; one that is not this
 *   account's is never shown, and other accounts' copies are swept when an
 *   account opens.
 * - Bounds: the index and the status always; the {@link KEPT_OPENED} sessions
 *   last opened here and the {@link WARM_SESSIONS} most recently updated.
 *   Anything else under the machine's prefix is deleted in the same
 *   transaction that saves a session.
 * - At rest every record is sealed (AES-256-GCM) under a device-local key
 *   that never leaves the browser as bytes — generated here, non-extractable,
 *   kept as a `CryptoKey` in the key store under {@link MIRROR_KEY_NAME}, the
 *   way `keyring.ts` keeps user keys — with an AAD binding it to its account,
 *   machine, store key and {@link MIRROR_SCHEMA}. In the clear: the store key
 *   (account, machine, stream id — what the hub sees anyway) and the owner and
 *   machine again. Sealed: the state, the signed head, and in the opened list
 *   which sessions they are. A record that is not sealed (an older schema's
 *   plaintext), or that does not open (another key, tampered), is treated as
 *   absent and deleted.
 * - Names (relay-and-plans §5): an agent that names its streams per key
 *   epoch keeps the index and status under its newest epoch and each
 *   session's under the epoch its index entry says (`se`; absent: the
 *   first). The index and status are looked for newest epoch first, then
 *   older ones (a machine off since a rotation has not moved them yet), and
 *   followed on to a newer name the moment it has something (the source
 *   does this: `copy-source.ts`); a session whose entry moves to a new epoch
 *   is followed under its new name. The record's place is the source's
 *   (the hub's: the stream id) and so is its position (the signed head).
 * - Signing out clears the whole store and seals it (`cacheClear`), so a save
 *   still scheduled cannot put anything back, and forgets the mirror key
 *   ({@link forgetMirrorKey}): a copy that survived the clear opens for nobody.
 */
import type { components } from "@/api/schema";
import { type Bytes, bytes, copy, equal, fromUtf8, hex, indexedDbKeyStore, type KeyStore, MemoryKeyStore, Serial, utf8, Writer } from "@/lib/e2e";
import { cacheSealed, cacheTransact } from "@/lib/local-cache";
import type { CopyFollow, CopyMark, CopyName, CopySource } from "./copy-source";
import { deviceKey, forgetDeviceKey } from "./device-key";
import type { FeedCopy, IndexState, StatusState } from "./streams";

export { namesOf } from "./copy-source";

type SessionSummary = components["schemas"]["SessionSummary"];

/** Bumped whenever a record changes shape; a record of another version is deleted. 2: sealed at rest. */
export const MIRROR_SCHEMA = 2;
/** Sessions opened on this device whose copies are kept. */
export const KEPT_OPENED = 20;
/** The most recently updated sessions kept warm (followed, and kept). */
export const WARM_SESSIONS = 5;
/** Of the sessions opened in this page, how many stay followed at once. */
export const FOLLOWED_OPENED = 8;
/** How long a reader asking for a session waits for a copy that is not on the device yet. */
export const SESSION_WAIT_MS = 8000;

/** The slice of an `IDBObjectStore` the mirror uses (what `cacheTransact` hands out). */
export interface KvRequest<T> {
  readonly result: T;
  onsuccess: (() => void) | null;
}
export interface KvStore {
  get(key: string): KvRequest<unknown>;
  put(value: unknown, key: string): unknown;
  delete(key: string): unknown;
  /** Every key, or (`prefix`) only those that start with it — read as a key range, not filtered after. */
  getAllKeys(prefix?: string): KvRequest<unknown[]>;
}
/** One transaction: everything `act` starts happens in it. Resolves with what it hands `done`, or `undefined` on any failure. */
export type Transact = <T>(mode: "readonly" | "readwrite", act: (store: KvStore, done: (value: T) => void) => void, ms?: number) => Promise<T | undefined>;

/** `cacheTransact` over the slice of `IDBObjectStore` the mirror uses; a prefix is a key range. */
export const deviceTransact: Transact = (mode, act, ms) => cacheTransact(mode, (store, done) => act(kvOf(store), done), ms);

function kvOf(store: IDBObjectStore): KvStore {
  return {
    get: (key) => store.get(key) as unknown as KvRequest<unknown>,
    put: (value, key) => store.put(value, key),
    delete: (key) => store.delete(key),
    // Callers still check each key's prefix: a store without key ranges (a test's) hands back every key.
    getAllKeys: (prefix) =>
      (prefix === undefined || typeof IDBKeyRange === "undefined"
        ? store.getAllKeys()
        : store.getAllKeys(IDBKeyRange.bound(prefix, `${prefix}\uffff`))) as unknown as KvRequest<unknown[]>,
  };
}

export type StreamKind = "index" | "status" | "session";

/** A stream's copy, as sealed: the decrypted state and the signed head it stands at, together. */
export interface MirrorRecord<S> {
  kind: StreamKind;
  source?: string;
  id?: string;
  /** The encoded signed head the state stands at, hex. */
  head: string;
  state: S;
  freshestMs: number | null;
  savedAt: number;
}

/** A stream's copy at rest: whose it is (as its store key says anyway), and the {@link MirrorRecord} sealed. */
export interface StoredCopy {
  v: number;
  owner: string;
  machine: string;
  iv: Uint8Array;
  ct: Uint8Array;
}

interface OpenedRef {
  source: string;
  id: string;
  hex: string;
}

/** An opened-list entry at rest: the stream id in the clear (it is the store key's), which session it is sealed. */
interface StoredOpened {
  hex: string;
  iv: Uint8Array;
  ct: Uint8Array;
}

interface StoredMeta {
  v: number;
  owner: string;
  machine: string;
  opened: StoredOpened[];
}

const seg = (part: string) => encodeURIComponent(part);
export const streamPrefix = (account: string, machine: string) => `stream/${seg(account)}/${seg(machine)}/`;
export const streamRecordKey = (account: string, machine: string, streamHex: string) => `${streamPrefix(account, machine)}${streamHex}`;
export const metaKey = (account: string, machine: string) => `streammeta/${seg(account)}/${seg(machine)}`;

/** Whether `key` is one of the mirror's keys and belongs to some account other than `account`. */
export function foreignMirrorKey(key: unknown, account: string): boolean {
  if (typeof key !== "string") return false;
  const m = /^(?:stream|streammeta)\/([^/]+)\//.exec(key);
  return m !== null && m[1] !== seg(account);
}

/** The opened list after `ref` was opened again: most recent first, at most `keep`. */
export function nextOpened(list: readonly OpenedRef[], ref: OpenedRef, keep = KEPT_OPENED): OpenedRef[] {
  return [ref, ...list.filter((o) => o.hex !== ref.hex)].slice(0, keep);
}

/** Merge this page's opened list over the stored one, by stream: this page's first, at most `keep`. */
export function mergeOpened<T extends { hex: string }>(mine: readonly T[], theirs: readonly T[], keep = KEPT_OPENED): T[] {
  const out: T[] = [];
  for (const o of [...mine, ...theirs]) if (!out.some((x) => x.hex === o.hex)) out.push(o);
  return out.slice(0, keep);
}

/** The `n` most recently updated rows. */
export function warmRows(rows: readonly SessionSummary[], n = WARM_SESSIONS): SessionSummary[] {
  return [...rows].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)).slice(0, n);
}

const validState: Record<StreamKind, (state: unknown) => boolean> = {
  index: (s) => typeof s === "object" && s !== null && typeof (s as IndexState).rows === "object",
  status: (s) => typeof s === "object" && s !== null,
  session: (s) => typeof s === "object" && s !== null && Array.isArray((s as FeedCopy).items) && Number.isInteger((s as FeedCopy).startIndex),
};

// ------------------------------------------------------------ at rest

/** Where the mirror key is kept in the key store (one per store, beside `keyring.ts`'s `local-wrap/v1`). */
export const MIRROR_KEY_NAME = "mirror-wrap/v1";
const SEAL_LABEL = utf8("rh/v1 mirror copy\0");

/** What a sealed value is bound to: the account and machine it is a copy of, and its key in the store. */
export interface SealAt {
  account: string;
  machine: string;
  key: string;
}

export interface Sealed {
  iv: Bytes;
  ct: Bytes;
}

/**
 * Seals the mirror's records for the device and opens them again: JSON under
 * the device-local mirror key ({@link mirrorSeal}).
 */
export interface MirrorSeal {
  seal(at: SealAt, value: unknown): Promise<Sealed>;
  /** The value back; `null` when it does not open (another key, tampered). Rejects only when there is no key to try. */
  open(at: SealAt, sealed: { iv: Uint8Array; ct: Uint8Array }): Promise<{ value: unknown } | null>;
}

function sealAad(at: SealAt): Bytes {
  return new Writer().raw(SEAL_LABEL).u16(MIRROR_SCHEMA).str16(at.account).str16(at.machine).str16(at.key).finish();
}

/**
 * Making the key is one at a time across every store: two seals (two key
 * stores over the same database) creating it at once would each keep a key
 * and orphan the other's copies. `forgetMirrorKey` moves the generation on,
 * so every seal reads the key afresh after it.
 */
const keySerial = new Serial();
let keyGeneration = 0;
const mirrorKeys = new WeakMap<KeyStore, { generation: number; key: Promise<CryptoKey> }>();

function mirrorKey(store: KeyStore): Promise<CryptoKey> {
  const held = mirrorKeys.get(store);
  if (held && held.generation === keyGeneration) return held.key;
  const entry = {
    generation: keyGeneration,
    // Kept but handed back as nothing (WebKit, a stored `CryptoKey` it cannot
    // unseal) is read again; it is replaced only when it will never open
    // again (the store keeps new keys fine), else nothing is saved this time.
    key: keySerial.run(() =>
      deviceKey(store, MIRROR_KEY_NAME, {
        usages: ["encrypt", "decrypt"],
        unreadable: "the browser could not read the key the device copies are sealed under",
        make: async (_existing, fresh) => {
          // Signed out (the copies cleared and sealed): nothing can be saved, and
          // a key made now would only outlive the copies it was forgotten with.
          if (cacheSealed()) throw new Error("the device copies are sealed: no new mirror key");
          return fresh();
        },
      }),
    ),
  };
  mirrorKeys.set(store, entry);
  entry.key.catch(() => {
    if (mirrorKeys.get(store) === entry) mirrorKeys.delete(store);
  });
  return entry.key;
}

const seals = new WeakMap<KeyStore, MirrorSeal>();

/** The mirror's seal over `store`'s mirror key (made on first use). One per store. */
export function mirrorSeal(store: KeyStore): MirrorSeal {
  let seal = seals.get(store);
  if (seal) return seal;
  seal = {
    async seal(at, value) {
      const key = await mirrorKey(store);
      const iv = crypto.getRandomValues(bytes(12));
      const plain = utf8(JSON.stringify(value));
      try {
        const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: sealAad(at) }, key, plain));
        return { iv, ct };
      } finally {
        plain.fill(0);
      }
    },
    async open(at, sealed) {
      const key = await mirrorKey(store);
      let plain: Uint8Array;
      try {
        plain = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: copy(sealed.iv), additionalData: sealAad(at) }, key, copy(sealed.ct)));
      } catch {
        return null;
      }
      try {
        return { value: JSON.parse(fromUtf8(plain)) };
      } catch {
        return null;
      } finally {
        plain.fill(0);
      }
    },
  };
  seals.set(store, seal);
  return seal;
}

let deviceStore: KeyStore | null = null;

/** The key store the mirror key is kept in by default: this browser's (`indexedDbKeyStore()`), or memory where there is no IndexedDB (the tests). */
export function deviceMirrorStore(): KeyStore {
  deviceStore ??= typeof indexedDB === "undefined" ? new MemoryKeyStore() : indexedDbKeyStore();
  return deviceStore;
}

/** The seal a mirror or status reader uses when it is given none: {@link deviceMirrorStore}'s. */
export function defaultMirrorSeal(): MirrorSeal {
  return mirrorSeal(deviceMirrorStore());
}

/**
 * Forget the mirror key (sign-out, a removed device's wipe): the copies go
 * with it — whatever of them is still on the device opens for nobody. The
 * next copy saved makes a new key.
 */
export function forgetMirrorKey(store: KeyStore = deviceMirrorStore()): Promise<void> {
  keyGeneration++;
  mirrorKeys.delete(store);
  return keySerial.run(() => forgetDeviceKey(store, MIRROR_KEY_NAME));
}

const isIv = (v: unknown): v is Uint8Array => v instanceof Uint8Array && v.length === 12;
const isCt = (v: unknown): v is Uint8Array => v instanceof Uint8Array && v.length >= 16;

/** Whether `value` is a stream copy in this schema's shape (whether it opens is another matter). */
export function isStoredCopy(value: unknown): value is StoredCopy {
  const v = value as Partial<StoredCopy> | undefined;
  return !!v && typeof v === "object" && v.v === MIRROR_SCHEMA && typeof v.owner === "string" && typeof v.machine === "string" && isIv(v.iv) && isCt(v.ct);
}

function isStoredOpened(value: unknown): value is StoredOpened {
  const v = value as Partial<StoredOpened> | undefined;
  return !!v && typeof v === "object" && typeof v.hex === "string" && isIv(v.iv) && isCt(v.ct);
}

function isStoredMeta(value: unknown): value is StoredMeta {
  const v = value as Partial<StoredMeta> | undefined;
  return !!v && typeof v === "object" && v.v === MIRROR_SCHEMA && typeof v.owner === "string" && typeof v.machine === "string" && Array.isArray(v.opened) && v.opened.every(isStoredOpened);
}

/** Where a copy of `account`'s `machine` is bound: its stream's record. */
export const copyAt = (account: string, machine: string, streamHex: string): SealAt => ({ account, machine, key: streamRecordKey(account, machine, streamHex) });
/** Where an opened-list entry is bound: its stream, in the machine's list. */
const openedAt = (account: string, machine: string, streamHex: string): SealAt => ({ account, machine, key: `${metaKey(account, machine)}#${streamHex}` });

/** `record` sealed for the device, at `at`. Rejects (and nothing is written in the clear) when there is no key. */
export async function sealCopy(seal: MirrorSeal, at: SealAt, record: MirrorRecord<unknown>): Promise<StoredCopy> {
  const { iv, ct } = await seal.seal(at, record);
  return { v: MIRROR_SCHEMA, owner: at.account, machine: at.machine, iv, ct };
}

/**
 * A stored copy opened: the record when it is this schema's, opens at `at`
 * and is a well-formed copy of `kind`; otherwise whether to delete it (not
 * when it is simply not there, nor when no key could be had to try it with).
 */
export async function openCopy<S>(seal: MirrorSeal, stored: unknown, at: SealAt, kind: StreamKind): Promise<{ record: MirrorRecord<S> } | { drop: boolean }> {
  if (stored === undefined || stored === null) return { drop: false };
  if (!isStoredCopy(stored) || stored.owner !== at.account || stored.machine !== at.machine) return { drop: true };
  let opened: { value: unknown } | null;
  try {
    opened = await seal.open(at, stored);
  } catch {
    return { drop: false };
  }
  const r = opened?.value as Partial<MirrorRecord<S>> | null | undefined;
  if (!r || typeof r !== "object" || r.kind !== kind || typeof r.head !== "string" || r.state === undefined || r.state === null || !validState[kind](r.state)) {
    return { drop: true };
  }
  return { record: r as MirrorRecord<S> };
}

/** Delete the copy at `key` if it is still the one that would not do (another tab may have saved a good one since). */
export function dropCopy(transact: Transact, key: string, seen: unknown): Promise<void> {
  const ivOf = (v: unknown) => ((v as { iv?: unknown } | null | undefined)?.iv instanceof Uint8Array ? (v as { iv: Uint8Array }).iv : null);
  return transact<void>(
    "readwrite",
    (store) => {
      const req = store.get(key);
      req.onsuccess = () => {
        const now = req.result;
        if (now === undefined) return;
        const a = ivOf(now);
        const b = ivOf(seen);
        if (!isStoredCopy(now) || (a && b && equal(a, b))) store.delete(key);
      };
    },
    2000,
  ).then(() => undefined);
}

const sweeping = new WeakMap<Transact, Map<string, Promise<void>>>();

/**
 * Sweep the device: every copy that is not `account`'s (the device changed
 * hands without a sign-out), and every copy of `account`'s that is not in
 * this schema's sealed shape (plaintext from before it). Copies that are in
 * shape but do not open are deleted as they are read. One sweep at a time per
 * store and account.
 */
export function sweepCopies(transact: Transact, account: string): Promise<void> {
  let byAccount = sweeping.get(transact);
  if (!byAccount) sweeping.set(transact, (byAccount = new Map()));
  const running = byAccount.get(account);
  if (running) return running;
  const copies = `stream/${seg(account)}/`;
  const metas = `streammeta/${seg(account)}/`;
  const work = transact<void>(
    "readwrite",
    (store) => {
      // Only the mirror's own keys: the rest of the device copy (the feed
      // windows, the session lists) is not read at all.
      const seen = new Set<string>();
      for (const range of ["stream/", "streammeta/"]) {
        const keysReq = store.getAllKeys(range);
        keysReq.onsuccess = () => {
          for (const key of keysReq.result) {
            if (typeof key !== "string" || seen.has(key)) continue;
            seen.add(key);
            if (foreignMirrorKey(key, account)) {
              store.delete(key);
              continue;
            }
            const meta = key.startsWith(metas);
            if (!meta && !key.startsWith(copies)) continue;
            const req = store.get(key);
            req.onsuccess = () => {
              if (!(meta ? isStoredMeta(req.result) : isStoredCopy(req.result))) store.delete(key);
            };
          }
        };
      }
    },
    2000,
  )
    .then(() => undefined)
    .finally(() => byAccount.delete(account));
  byAccount.set(account, work);
  return work;
}

export type MirrorChange =
  | { kind: "index" }
  | { kind: "status" }
  /** `fromIndex`: where the items changed from (replace-from), or `null` when the copy was replaced outright. */
  | { kind: "session"; key: string; fromIndex: number | null };

/** One session's copy, as the mirror holds it. */
export interface SessionCopy {
  source: string;
  id: string;
  /**
   * The folded transcript. When the account's retention window trimmed the
   * copy, `copyWindow(state)` (`streams.ts`) says from which item it starts
   * and that the earlier ones are only on the machine.
   */
  state: FeedCopy;
  /** The confirmed seq of its stream (the copy's version). */
  seq: number;
  freshestMs: number | null;
}

/** A copy read back from the device, still good: its state, where it stands, how current. */
export interface Saved<S> {
  state: S;
  at: CopyMark;
  freshestMs: number | null;
}

interface StreamSlot<S> {
  kind: StreamKind;
  /** Where its copy is kept, as the source names it (the hub's: the stream id, hex). */
  place: string;
  state: S | null;
  /** Where the state stands, as the source resumes it. */
  at: CopyMark | null;
  freshestMs: number | null;
  sub: CopyFollow | null;
  /** The hub has said the stream does not exist (yet). */
  missing: boolean;
  /** Its catch-up on the current subscription is over. */
  synced: boolean;
  saveTimer: ReturnType<typeof setTimeout> | undefined;
}

/** How one kind of copy is kept and read back: the device store it is in, and where it comes from. */
export interface CopySlotStore {
  account: string;
  machine: string;
  kind: StreamKind;
  /** `null`, or a source with no keys, when this device holds none: nothing is read back or saved then. */
  source: CopySource | null;
  transact: Transact;
  seal: MirrorSeal;
}

/** A copy as it stood when its save was asked for: the record, and where it goes. */
export interface PendingCopy {
  record: MirrorRecord<unknown>;
  at: SealAt;
}

/**
 * One small copy (the index or the status) as this device keeps it: the state
 * it holds and where it stands, read back from a sealed record and written as
 * one — the mechanics the mirror and the machine list's status reader share.
 * Sessions keep more (an opened list, sweeping) and are the mirror's own.
 */
export class CopySlot<S> implements StreamSlot<S> {
  readonly kind: StreamKind;
  state: S | null = null;
  at: CopyMark | null = null;
  freshestMs: number | null = null;
  sub: CopyFollow | null = null;
  /** The hub has said the stream does not exist (yet). */
  missing = false;
  /** Its catch-up on the current subscription is over. */
  synced = false;
  saveTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    private readonly store: CopySlotStore,
    /** Where its copy is kept, as the source names it (the hub's: the stream id, hex). */
    public place: string,
    saved: Saved<S> | null = null,
  ) {
    this.kind = store.kind;
    if (saved) {
      this.state = saved.state;
      this.at = saved.at;
      this.freshestMs = saved.freshestMs;
    }
  }

  /** The newest of `places` whose stored record (`stored`, by index) opens and still holds; `at` its index (`0` when none does). */
  read(places: readonly string[], stored: readonly unknown[], ref?: { source: string; id: string }): Promise<{ at: number; saved: Saved<S> | null }> {
    return newestSavedCopy<S>(this.store, places, stored, ref);
  }

  /** This copy as it stands now, for a save (`null`: nothing to save). */
  pending(savedAt: number): PendingCopy | null {
    if (!this.state || !this.at || !this.place) return null;
    return {
      record: { kind: this.kind, head: this.at.mark, state: this.state, freshestMs: this.freshestMs, savedAt },
      at: copyAt(this.store.account, this.store.machine, this.place),
    };
  }

  /** Seal `pending` and write it in one transaction. `beforeWrite` is asked just before the write (`false`: nothing is written). */
  async write(pending: PendingCopy, beforeWrite?: () => boolean, warning = "mirror: a copy could not be sealed, so it was not saved:"): Promise<void> {
    let stored: StoredCopy;
    try {
      stored = await sealCopy(this.store.seal, pending.at, pending.record);
    } catch (error) {
      console.warn(warning, error);
      return;
    }
    if (beforeWrite && !beforeWrite()) return;
    await this.store.transact<void>(
      "readwrite",
      (store) => {
        store.put(stored, pending.at.key);
      },
      2000,
    );
  }
}

/**
 * A stored record as a starting point, if it opens, is ours and its mark
 * still holds (the hub's: the head is still the machine's). One that is not
 * sealed, or does not open, is deleted.
 */
async function savedCopy<S>(store: CopySlotStore, stored: unknown, place: string, ref?: { source: string; id: string }): Promise<Saved<S> | null> {
  const source = store.source;
  if (!source) return null;
  const at = copyAt(store.account, store.machine, place);
  const opened = await openCopy<S>(store.seal, stored, at, store.kind);
  if (!("record" in opened)) {
    if (opened.drop) void dropCopy(store.transact, at.key, stored);
    return null;
  }
  const r = opened.record;
  if (ref && (r.source !== ref.source || r.id !== ref.id)) return null;
  const mark = await source.resume(place, r.head);
  // How current: the machine's own time on the mark, not the number stored beside it.
  return mark ? { at: mark, state: r.state, freshestMs: mark.timeMs } : null;
}

/** The newest of `places` with a copy on this device that still holds, and that copy; else the first and none. */
async function newestSavedCopy<S>(
  store: CopySlotStore,
  places: readonly string[],
  stored: readonly unknown[],
  ref?: { source: string; id: string },
): Promise<{ at: number; saved: Saved<S> | null }> {
  for (let i = 0; i < places.length; i++) {
    if (stored[i] === undefined) continue;
    const saved = await savedCopy<S>(store, stored[i], places[i]!, ref);
    if (saved) return { at: i, saved };
  }
  return { at: 0, saved: null };
}

interface SessionSlot extends StreamSlot<FeedCopy> {
  source: string;
  id: string;
  key: string;
  /** The index entry's `se` the stream was named from (`undefined`: none, the first epoch). */
  se: unknown;
  /** Read back from the device (or found absent). */
  loaded: Promise<void>;
  /** `loaded` has settled, and nobody asked for it since that is still waiting on it. */
  settled: boolean;
  waiters: Set<() => void>;
}

export interface MirrorOptions {
  account: string;
  machine: string;
  /**
   * Where the copies come from (`copy-source.ts`: the hub's streams, opened
   * here with the machine's keys); `null`, or a source with no keys, when
   * this device holds no key for the machine. A source whose first keys
   * arrive later starts the copy then.
   */
  source: CopySource | null;
  transact?: Transact;
  /** How the copies are sealed at rest; default {@link defaultMirrorSeal} (pass `mirrorSeal(store)` with the page's key store). */
  seal?: MirrorSeal;
  now?: () => number;
  /** How far a save trails the state it saves: index and status, sessions. */
  saveDelayMs?: { small: number; session: number };
  sessionWaitMs?: number;
  /**
   * Whether the warm sessions and the last few opened are followed (default
   * yes); off, only the index, the status and a session somebody is asking
   * for right now are ({@link MachineMirror.setFollowSessions}).
   */
  followSessions?: boolean;
  /** Whether the copy sweeps the device once it has started (default yes); a page with many copies sweeps once for all of them. */
  sweep?: boolean;
  /**
   * Whether the copy is kept on this device at all (default yes). Off, it is
   * memory only: nothing read from or written to IndexedDB, nothing sealed —
   * the local app's view of another device through its gateway (S1-7), where
   * the copy lives on the local install and the page may not even be a
   * secure context (a phone at the computer's LAN address has no WebCrypto).
   */
  persist?: boolean;
}

export class MachineMirror {
  /** The index and status copies; `null` until there is one. */
  index: CopySlot<IndexState> | null = null;
  status: CopySlot<StatusState> | null = null;
  /** Whether the device copy has been read (instant or not, it is what it is now). */
  loaded = false;
  private readonly sessions = new Map<string, SessionSlot>();
  /** Sessions opened on this device, most recent first (this page's, merged with the stored list). */
  private opened: OpenedRef[] = [];
  private readonly listeners = new Set<(change: MirrorChange) => void>();
  private closed = false;
  private readonly transact: Transact;
  private readonly seal: MirrorSeal;
  /** Saves one at a time: sealing is asynchronous, and a later state must not be overwritten by an earlier one. */
  private readonly saves = new Serial();
  /** The opened list's entries as sealed (by stream), so a save does not seal them all again. */
  private readonly sealedOpened = new Map<string, StoredOpened>();
  /** Opened-list entries read back that did not open (by IV): not carried on. */
  private readonly unreadableOpened = new Set<string>();
  private readonly ready: Promise<void>;
  private readyResolve!: () => void;
  /** The index and the status, each followed under the newest name it has. */
  private indexFollow: CopyFollow | null = null;
  private statusFollow: CopyFollow | null = null;
  /** Waiting for the source's first keys (it had none when the copy started). */
  private offKeys: (() => void) | null = null;
  /** Asking again for sessions the source refused for want of room, once there is room. */
  private offRoom: (() => void) | null = null;
  private started = false;
  /** See {@link MirrorOptions.followSessions}. */
  private followingSessions: boolean;

  constructor(private readonly opts: MirrorOptions) {
    this.transact = opts.transact ?? deviceTransact;
    this.seal = opts.seal ?? defaultMirrorSeal();
    this.followingSessions = opts.followSessions ?? true;
    this.ready = new Promise((resolve) => {
      this.readyResolve = resolve;
    });
  }

  get account(): string {
    return this.opts.account;
  }

  get machine(): string {
    return this.opts.machine;
  }

  private now(): number {
    return (this.opts.now ?? Date.now)();
  }

  /** Resolves once the device copy has been read. */
  whenLoaded(): Promise<void> {
    return this.ready;
  }

  onChange(fn: (change: MirrorChange) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(change: MirrorChange): void {
    for (const fn of [...this.listeners]) {
      try {
        fn(change);
      } catch {
        // one listener failing must not starve the others
      }
    }
  }

  /** Whether there is any copy of this machine to show. */
  get hasCopy(): boolean {
    return this.index?.state != null;
  }

  /** The session list, newest first; `null` with no copy. */
  rows(): SessionSummary[] | null {
    const state = this.index?.state;
    if (!state) return null;
    return Object.values(state.rows).sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }

  /** One row of the list. */
  row(source: string, id: string): SessionSummary | undefined {
    return this.index?.state?.rows[`${source}/${id}`];
  }

  /** The hub has said this machine has no index stream (nothing was ever synced), and there is no copy. */
  get indexMissing(): boolean {
    return this.index?.missing === true;
  }

  /** How current the copy is known to be: the newest machine-signed head time of the index. */
  get freshestMs(): number | null {
    return this.index?.freshestMs ?? null;
  }

  // ------------------------------------------------------------ start

  /**
   * Read the device copy, then follow the index and the status. With a
   * source that has no keys yet, the copy is empty (and loaded) until its
   * first keys arrive, and starts then.
   */
  async start(): Promise<void> {
    try {
      const source = this.opts.source;
      if (!source) return;
      await source.ready();
      if (this.closed) return;
      if (source.keyed) return await this.begin(source);
      this.offKeys = source.onKeys(() => {
        if (!source.keyed || this.started || this.closed) return;
        this.offKeys?.();
        this.offKeys = null;
        void this.begin(source);
      });
    } finally {
      this.loaded = true;
      this.readyResolve();
    }
  }

  private async begin(source: CopySource): Promise<void> {
    if (this.started) return;
    this.started = true;
    const [indexPlaces, statusPlaces] = await Promise.all([source.places({ kind: "index" }), source.places({ kind: "status" })]);
    if (!indexPlaces.length || this.closed) return;
    const { account, machine } = this.opts;
    const stored = this.opts.persist === false ? undefined : await this.transact<{ index: unknown[]; status: unknown[]; meta: unknown }>(
      "readonly",
      (store, done) => {
        const got = { index: [] as unknown[], status: [] as unknown[], meta: undefined as unknown };
        let left = indexPlaces.length + statusPlaces.length + 1;
        const one = () => {
          if (--left === 0) done(got);
        };
        for (const [list, into] of [
          [indexPlaces, got.index],
          [statusPlaces, got.status],
        ] as const) {
          list.forEach((place, i) => {
            const req = store.get(streamRecordKey(account, machine, place));
            req.onsuccess = () => {
              into[i] = req.result;
              one();
            };
          });
        }
        const c = store.get(metaKey(account, machine));
        c.onsuccess = () => {
          got.meta = c.result;
          one();
        };
      },
      3000,
    );
    const [index, status, opened] = await Promise.all([
      this.newestSaved<IndexState>(indexPlaces, stored?.index ?? [], "index"),
      this.newestSaved<StatusState>(statusPlaces, stored?.status ?? [], "status"),
      this.openedOf(stored?.meta),
    ]);
    this.opened = mergeOpened(this.opened, opened);
    if (this.closed) return;
    this.index = this.slot("index", indexPlaces[index.at]!, index.saved);
    this.status = this.slot("status", statusPlaces[status.at] ?? "", status.saved);
    this.loaded = true;
    // The first paint waits for this much and no more.
    this.readyResolve();
    if (index.saved) this.emit({ kind: "index" });
    if (status.saved) this.emit({ kind: "status" });
    this.indexFollow = this.followNewest(this.index, { kind: "index" }, indexPlaces, index);
    this.statusFollow = this.followNewest(this.status, { kind: "status" }, statusPlaces, status);
    this.offRoom = source.onRoom?.(() => this.reconcile()) ?? null;
    this.reconcile();
    // Other accounts' copies on this device are not ours to keep (the
    // device changed hands without a sign-out), and plaintext from before
    // the copies were sealed is not kept at all. After the first paint.
    if (this.opts.sweep !== false) await sweepCopies(this.transact, account);
  }

  /** How a copy of `kind` is kept here, for the shared slot machinery. */
  private slotStore(kind: StreamKind): CopySlotStore {
    return { account: this.opts.account, machine: this.opts.machine, kind, source: this.opts.source, transact: this.transact, seal: this.seal };
  }

  /** Of `places` (the one to show first first), the first with a copy on this device that is still good, and that copy; else the first and none. */
  private newestSaved<S>(places: readonly string[], stored: readonly unknown[], kind: StreamKind): Promise<{ at: number; saved: Saved<S> | null }> {
    return newestSavedCopy<S>(this.slotStore(kind), places, stored);
  }

  /** Follow the index or the status into `slot`, under whichever of its places the source says (the slot moves with it). */
  private followNewest<S>(slot: StreamSlot<S>, name: CopyName, places: readonly string[], start: { at: number; saved: Saved<S> | null }): CopyFollow | null {
    const source = this.opts.source;
    if (!source || this.closed) return null;
    return source.follow<S>(
      name,
      places,
      { at: start.at, from: start.saved },
      {
        onMoved: (place) => {
          slot.place = place;
          slot.synced = false;
        },
        onState: (change) => {
          slot.state = change.state;
          slot.at = change.at;
          slot.freshestMs = change.freshestMs;
          slot.missing = false;
          this.scheduleSave(slot);
          this.emit({ kind: slot.kind as "index" | "status" });
          if (slot.kind === "index") {
            this.reconcile();
            void this.renameSessions();
          }
        },
        onSynced: (exists) => {
          slot.synced = true;
          slot.missing = !exists && slot.state === null;
          if (slot.kind === "index") this.emit({ kind: "index" });
        },
      },
    );
  }

  /**
   * Sessions whose index entry now names their stream under another epoch
   * (the machine moved it): followed under the new name from here on — the
   * copy under the old one is let go (and swept), and whoever waits on it
   * asks again.
   */
  private async renameSessions(): Promise<void> {
    for (const [key, slot] of [...this.sessions]) {
      const se = (this.row(slot.source, slot.id) as { se?: unknown } | undefined)?.se;
      if (se === slot.se || !slot.settled || this.closed) continue;
      const place = await this.placeOf(slot.source, slot.id, se);
      if (this.closed || this.sessions.get(key) !== slot) continue;
      if (place && place === slot.place) {
        slot.se = se;
        continue;
      }
      const followed = slot.sub !== null;
      slot.sub?.close();
      slot.sub = null;
      clearTimeout(slot.saveTimer);
      slot.saveTimer = undefined;
      this.sessions.delete(key);
      this.wake(slot);
      const next = this.sessionSlot(slot.source, slot.id);
      if (!next) continue;
      if (this.opened.some((o) => o.source === slot.source && o.id === slot.id)) {
        void next.loaded.then(() => {
          if (!next.place || this.closed) return;
          this.opened = nextOpened(
            this.opened.filter((o) => !(o.source === slot.source && o.id === slot.id)),
            { source: slot.source, id: slot.id, hex: next.place },
          );
        });
      }
      if (followed) void next.loaded.then(() => this.startFollowing(next));
      this.emit({ kind: "session", key, fromIndex: null });
    }
  }

  /** Where a session's copy is: named under its index entry's epoch (`se`; none: the first). `null`: no key for it. */
  private async placeOf(source: string, id: string, se: unknown): Promise<string | null> {
    const from = this.opts.source;
    if (!from) return null;
    return (await from.places({ kind: "session", source, id, se }))[0] ?? null;
  }

  private slot<S>(kind: StreamKind, place: string, saved: Saved<S> | null): CopySlot<S> {
    return new CopySlot<S>(this.slotStore(kind), place, saved);
  }

  /**
   * A stored record as a starting point, if it opens, is ours and its mark
   * still holds (the hub's: the head is still the machine's). One that is
   * not sealed, or does not open, is deleted.
   */
  private savedOf<S>(stored: unknown, kind: StreamKind, place: string, ref?: { source: string; id: string }): Promise<Saved<S> | null> {
    return savedCopy<S>(this.slotStore(kind), stored, place, ref);
  }

  /** The opened list read back: the entries that open, in order (each kept as sealed, to be saved again as it is). */
  private async openedOf(stored: unknown): Promise<OpenedRef[]> {
    const { account, machine } = this.opts;
    if (!isStoredMeta(stored) || stored.owner !== account || stored.machine !== machine) return [];
    const refs = await Promise.all(
      stored.opened.map(async (entry): Promise<OpenedRef | null> => {
        let opened: { value: unknown } | null;
        try {
          opened = await this.seal.open(openedAt(account, machine, entry.hex), entry);
        } catch {
          return null;
        }
        const v = opened?.value as Partial<OpenedRef> | null | undefined;
        if (!v || typeof v.source !== "string" || typeof v.id !== "string") {
          this.unreadableOpened.add(hex(entry.iv));
          return null;
        }
        this.sealedOpened.set(entry.hex, entry);
        return { source: v.source, id: v.id, hex: entry.hex };
      }),
    );
    return refs.filter((r): r is OpenedRef => r !== null);
  }

  /** This page's opened list as it is stored. */
  private async sealOpened(list: readonly OpenedRef[]): Promise<StoredOpened[]> {
    const { account, machine } = this.opts;
    return Promise.all(
      list.map(async (ref) => {
        const known = this.sealedOpened.get(ref.hex);
        if (known) return known;
        const { iv, ct } = await this.seal.seal(openedAt(account, machine, ref.hex), { source: ref.source, id: ref.id });
        const entry: StoredOpened = { hex: ref.hex, iv, ct };
        this.sealedOpened.set(ref.hex, entry);
        return entry;
      }),
    );
  }

  /** Follow a session into `slot`, from the copy it holds. */
  private follow(slot: SessionSlot, onUpdate: (change: { reset: boolean; bodies: unknown[] }) => void): CopyFollow | null {
    const source = this.opts.source;
    if (!source || this.closed) return null;
    const from = slot.state && slot.at ? { state: slot.state, at: slot.at } : null;
    return source.follow<FeedCopy>(
      { kind: "session", source: slot.source, id: slot.id, se: slot.se },
      [slot.place],
      { at: 0, from },
      {
        onState: (change) => {
          slot.state = change.state;
          slot.at = change.at;
          slot.freshestMs = change.freshestMs;
          slot.missing = false;
          this.scheduleSave(slot);
          onUpdate(change);
        },
        onSynced: (exists) => {
          slot.synced = true;
          slot.missing = !exists && slot.state === null;
          this.wake(slot);
        },
      },
    );
  }

  // ------------------------------------------------------------ sessions

  private sessionSlot(source: string, id: string): SessionSlot | null {
    if (!this.opts.source?.keyed || this.closed) return null;
    const key = `${source}/${id}`;
    const existing = this.sessions.get(key);
    if (existing) return existing;
    const slot: SessionSlot = {
      kind: "session",
      source,
      id,
      key,
      place: "",
      state: null,
      at: null,
      freshestMs: null,
      sub: null,
      missing: false,
      synced: false,
      saveTimer: undefined,
      waiters: new Set(),
      loaded: Promise.resolve(),
      settled: false,
      se: (this.row(source, id) as { se?: unknown } | undefined)?.se,
    };
    slot.loaded = (async () => {
      try {
        const place = await this.placeOf(source, id, slot.se);
        if (!place) {
          // An epoch this device holds no key for (yet): no copy to be had.
          slot.missing = true;
          return;
        }
        slot.place = place;
      } catch {
        // a name the stream layer refuses: no stream, and no copy
        slot.missing = true;
        return;
      }
      const record =
        this.opts.persist === false
          ? undefined
          : await this.transact<unknown>(
              "readonly",
              (store, done) => {
                const req = store.get(streamRecordKey(this.opts.account, this.opts.machine, slot.place));
                req.onsuccess = () => done(req.result);
              },
              3000,
            );
      const saved = await this.savedOf<FeedCopy>(record, "session", slot.place, { source, id });
      if (saved && slot.state === null) {
        slot.state = saved.state;
        slot.at = saved.at;
        slot.freshestMs = saved.freshestMs;
      }
    })().finally(() => {
      slot.settled = true;
    });
    this.sessions.set(key, slot);
    return slot;
  }

  private startFollowing(slot: SessionSlot): void {
    if (slot.sub || slot.missing || this.closed) return;
    slot.sub = this.follow(slot, (change) => {
      this.wake(slot);
      // A heartbeat moves no item.
      if (!change.reset && change.bodies.length === 0) return;
      const fromIndex = change.reset ? null : change.bodies.reduce<number | null>((min, body) => {
        const from = (body as { fromIndex?: unknown }).fromIndex;
        return typeof from === "number" && (min === null || from < min) ? from : min;
      }, null);
      this.emit({ kind: "session", key: slot.key, fromIndex: change.reset ? null : (fromIndex ?? slot.state?.endIndex ?? 0) });
    });
  }

  private wake(slot: SessionSlot): void {
    for (const fn of [...slot.waiters]) fn();
  }

  /** The copy of a session if one is in memory now. */
  sessionNow(source: string, id: string): SessionCopy | null {
    const slot = this.sessions.get(`${source}/${id}`);
    return slot?.state ? { source, id, state: slot.state, seq: slot.at?.seq ?? 0, freshestMs: slot.freshestMs } : null;
  }

  /**
   * A session's copy: read back from the device at once if it is there, and
   * followed from there (or fetched from the hub, waiting up to `waitMs` for
   * the first state). `null` when there is none to be had.
   */
  async session(source: string, id: string, waitMs = this.opts.sessionWaitMs ?? SESSION_WAIT_MS): Promise<SessionCopy | null> {
    let slot = this.sessionSlot(source, id);
    if (!slot) return null;
    await slot.loaded;
    // Let go of meanwhile (it was neither followed nor kept): ask again.
    if (this.sessions.get(slot.key) !== slot) {
      slot = this.sessionSlot(source, id);
      if (!slot) return null;
      await slot.loaded;
    }
    this.startFollowing(slot);
    // Nothing on the device: worth a wait for the hub, if the hub is there —
    // and if the session is being followed (there may be no room for it).
    const linkUp = this.opts.source?.reachable === true;
    if (!slot.state && !slot.missing && slot.sub && !slot.synced && linkUp) {
      await new Promise<void>((resolve) => {
        const timer = setTimeout(finish, waitMs);
        const waiting = slot!;
        function finish() {
          clearTimeout(timer);
          waiting.waiters.delete(finish);
          resolve();
        }
        waiting.waiters.add(finish);
      });
    }
    return this.sessionNow(source, id);
  }

  /** A session was opened on this device: keep its copy, and follow it while it is among the last few. */
  touch(source: string, id: string): void {
    const slot = this.sessionSlot(source, id);
    if (!slot) return;
    void slot.loaded.then(() => {
      if (!slot.place || this.closed) return;
      this.opened = nextOpened(this.opened, { source, id, hex: slot.place });
      this.reconcile();
      // The opened list is saved with the next copy of this session, in the
      // same transaction; a session with a copy already gets one now.
      if (slot.state && slot.at) this.scheduleSave(slot);
    });
  }

  /**
   * Follow the warm sessions and the last few opened in this page, or (off)
   * none of them; let go of the rest (their copies stay on the device). A
   * session somebody is waiting on is followed either way.
   */
  setFollowSessions(on: boolean): void {
    if (this.followingSessions === on) return;
    this.followingSessions = on;
    this.reconcile();
  }

  /**
   * Follow exactly the warm sessions and the last few opened in this page
   * (when sessions are followed at all), and let go of the rest (their
   * copies stay on the device).
   */
  private reconcile(): void {
    if (this.closed || !this.opts.source?.keyed) return;
    const rows = this.rows() ?? [];
    const want = new Set<string>();
    if (this.followingSessions) {
      for (const row of warmRows(rows)) want.add(`${row.source}/${row.id}`);
      for (const o of this.opened.slice(0, FOLLOWED_OPENED)) want.add(`${o.source}/${o.id}`);
    }
    for (const key of want) {
      const [source, ...rest] = key.split("/");
      const slot = this.sessionSlot(source!, rest.join("/"));
      if (slot) void slot.loaded.then(() => this.startFollowing(slot));
    }
    const opened = new Set(this.opened.map((o) => `${o.source}/${o.id}`));
    for (const [key, slot] of this.sessions) {
      if (want.has(key) || slot.waiters.size > 0) continue;
      slot.sub?.close();
      slot.sub = null;
      slot.synced = false;
      // Not followed and not one of the kept: its copy (if any) is on the
      // device, and memory holds only what may be shown next.
      if (slot.settled && !opened.has(key) && slot.saveTimer === undefined) this.sessions.delete(key);
    }
  }

  /**
   * The places whose copies are kept (everything else under the machine is
   * swept): the index, the status, the sessions opened here, and the warm
   * sessions — named from the index, whether or not their slot is in memory
   * right now. Memory follows what is being followed (`reconcile` lets go of
   * a slot it stops following, as when the machine leaves view); what stays
   * on the device must not, or a save landing just after a switch would
   * sweep away the warm copies the switch had just let go of.
   */
  private async kept(): Promise<Set<string>> {
    const keep = new Set<string>();
    if (this.index) keep.add(this.index.place);
    if (this.status) keep.add(this.status.place);
    for (const o of this.opened) keep.add(o.hex);
    for (const row of warmRows(this.rows() ?? [])) {
      const slot = this.sessions.get(`${row.source}/${row.id}`);
      let place = slot?.place || null;
      if (!place) {
        try {
          place = await this.placeOf(row.source, row.id, (row as { se?: unknown }).se);
        } catch {
          place = null;
        }
      }
      if (place) keep.add(place);
    }
    return keep;
  }

  // ------------------------------------------------------------ saving

  private scheduleSave(slot: StreamSlot<unknown> | SessionSlot | StreamSlot<IndexState> | StreamSlot<StatusState>): void {
    if (this.closed || this.opts.persist === false || slot.saveTimer !== undefined) return;
    const delays = this.opts.saveDelayMs ?? { small: 2000, session: 5000 };
    slot.saveTimer = setTimeout(
      () => {
        slot.saveTimer = undefined;
        void this.save(slot as StreamSlot<unknown>, false);
      },
      slot.kind === "session" ? delays.session : delays.small,
    );
  }

  /** Save every copy that has a save pending, now (the page is going away). */
  flush(): Promise<void> {
    const pending: StreamSlot<unknown>[] = [];
    for (const slot of [this.index, this.status, ...this.sessions.values()]) {
      if (slot?.saveTimer !== undefined) {
        clearTimeout(slot.saveTimer);
        slot.saveTimer = undefined;
        pending.push(slot as StreamSlot<unknown>);
      }
    }
    return Promise.all(pending.map((slot) => this.save(slot, true))).then(() => undefined);
  }

  /**
   * Write one copy: its state and the signed head it stands at, together,
   * read off the slot at one instant and sealed as one record. A session's
   * save also carries the opened list and sweeps what is no longer kept —
   * the same transaction. Nothing is written when it cannot be sealed.
   */
  private save(slot: StreamSlot<unknown>, flushed: boolean): Promise<void> {
    const { account, machine } = this.opts;
    if (!slot.state || !slot.at || !slot.place) return Promise.resolve();
    const session = slot.kind === "session" ? (slot as unknown as SessionSlot) : null;
    const record: MirrorRecord<unknown> = {
      kind: slot.kind,
      ...(session ? { source: session.source, id: session.id } : {}),
      head: slot.at.mark,
      state: slot.state,
      freshestMs: slot.freshestMs,
      savedAt: this.now(),
    };
    const at = copyAt(account, machine, slot.place);
    if (!session) {
      const pending = { record, at };
      return this.saves.run(async () => {
        // Closed meanwhile (the machine is gone, or the account signed out):
        // a save its timer queued is dropped; one `flush` asked for is kept.
        if (this.closed && !flushed) return;
        await (slot as CopySlot<unknown>).write(pending, () => !this.closed || flushed);
      });
    }
    const mine = this.opened;
    return this.saves.run(async () => {
      // Closed meanwhile (the machine is gone, or the account signed out):
      // a save its timer queued is dropped; one `flush` asked for is kept.
      if (this.closed && !flushed) return;
      let stored: StoredCopy;
      let mineSealed: StoredOpened[];
      try {
        stored = await sealCopy(this.seal, at, record);
        mineSealed = await this.sealOpened(mine);
      } catch (error) {
        console.warn("mirror: a copy could not be sealed, so it was not saved:", error);
        return;
      }
      if (this.closed && !flushed) return;
      const kept = await this.kept();
      if (this.closed && !flushed) return;
      const prefix = streamPrefix(account, machine);
      const unreadable = this.unreadableOpened;
      await this.transact<void>(
        "readwrite",
        (store) => {
          const metaReq = store.get(metaKey(account, machine));
          metaReq.onsuccess = () => {
            const prior = metaReq.result;
            const theirs = isStoredMeta(prior) && prior.owner === account && prior.machine === machine ? prior.opened.filter((o) => !unreadable.has(hex(o.iv))) : [];
            const opened = mergeOpened(mineSealed, theirs);
            for (const o of opened) kept.add(o.hex);
            // Only a session that is kept is written at all.
            if (kept.has(slot.place)) store.put(stored, at.key);
            const meta: StoredMeta = { v: MIRROR_SCHEMA, owner: account, machine, opened };
            store.put(meta, metaKey(account, machine));
            // This machine's keys only, as a key range: not every key every machine keeps.
            const keysReq = store.getAllKeys(prefix);
            keysReq.onsuccess = () => {
              for (const k of keysReq.result) {
                if (typeof k === "string" && k.startsWith(prefix) && !kept.has(k.slice(prefix.length))) store.delete(k);
              }
            };
          };
        },
        2000,
      );
    });
  }

  /** Stop following everything (a save still pending is dropped; call {@link flush} first to keep it). */
  close(): void {
    this.closed = true;
    this.indexFollow?.close();
    this.statusFollow?.close();
    this.offKeys?.();
    this.offKeys = null;
    this.offRoom?.();
    this.offRoom = null;
    for (const slot of [this.index, this.status, ...this.sessions.values()]) {
      if (!slot) continue;
      clearTimeout(slot.saveTimer);
      slot.saveTimer = undefined;
      slot.sub?.close();
      slot.sub = null;
    }
    for (const slot of this.sessions.values()) this.wake(slot);
    this.listeners.clear();
  }
}
