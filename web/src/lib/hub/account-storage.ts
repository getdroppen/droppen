/*
 * Where this browser keeps an account's small records: the `rh.hub.*` names
 * (`localStorage`, and the seen record's keys in `seen-store.ts`), the storage
 * and lock seams the account is opened with (`AccountStorage`, `AccountLocks`,
 * `CrossTab`, `SeenWatch`), and the read/write helpers that never throw.
 * Nothing here knows the account's state; `account.ts` does.
 */

export const deviceKey = (account: string) => `rh.hub.device.${account}`;
/** The Web Lock moving the raw machine keys from before the keyring is done under (`CloudAccount.migrate`): the key store's, not one account's. */
export const LEGACY_MIGRATION_LOCK = "rh-legacy-machine-keys";
/** The signing key (hex) this browser's device id was registered with: what tells keys that changed under it. */
export const deviceSignKey = (account: string) => `rh.hub.devicekey.${account}`;
/** A device id this browser held but lost the keys of, until it is a device of the account again. */
export const lostDeviceKey = (account: string) => `rh.hub.lost.${account}`;
/** What the key store held instead of that device's keys, in a few words (shown with the notice). */
export const lostWhyKey = (account: string) => `rh.hub.lost-why.${account}`;
/** The device id whose pairing screen asked to sign in again: the next sign-in registers this browser afresh if the chain still does not name it. */
export const afreshKey = (account: string) => `rh.hub.afresh.${account}`;
/** A sign-in this close to the end of its device's code-pairing window registers the browser afresh too. */
export const REGISTER_AFRESH_MARGIN_MS = 60_000;
/** When this browser signed its recovery join (its own clock), for "you can activate from …". */
export const joinedKey = (account: string, device: string) => `rh.hub.recovered.${account}.${device}`;
/** When this device last asked the others for a user key epoch it lacks. */
export const keyRequestKey = (account: string, device: string) => `rh.hub.keyreq.${account}.${device}`;
/** When this device (not an admin) last asked an admin for the hosts' machine keys it lacks. */
export const hostKeyRequestKey = (account: string, device: string) => `rh.hub.hostkeyreq.${account}.${device}`;
/**
 * When this (admin) device last sealed device `to` every host key it may
 * hold, and the newest epoch of each host sealed to it since: the answer to
 * its requests is throttled by it (security review of S1-4, S4/N7).
 */
export const hostKeysSentKey = (account: string, device: string, to: string) => `rh.hub.hostkeysent.${account}.${device}.${to}`;

/** What this admin sealed one device lately ({@link hostKeysSentKey}). */
export interface HostKeysSent {
  /** When it was last sealed every key (a full answer). */
  atMs: number;
  /** Host → the newest epoch sealed to it since. */
  through: Record<string, number>;
}
/** The alerts this device acknowledged. */
export const ackKey = (account: string, device: string) => `rh.hub.ack.${account}.${device}`;
/**
 * Re-enroll warnings (review D3) a person dismissed in this browser — "I'm
 * sure it never connected" — host → device → the loss (chain seq) it was
 * dismissed for: a later loss of the same device warns again.
 */
export const reEnrollDismissedKey = (account: string) => `rh.hub.reenroll.dismissed.${account}`;
/**
 * Host → device → when it first connected, as a host's own record said it,
 * kept so a later record that leaves it out cannot unsay it. Only devices the
 * chain names (removed ones included) are added: the account bounds it, and
 * nothing is ever evicted — a record padded with made-up ids cannot push a
 * real one out. Nothing is deleted on reading either: every tab shares it,
 * and one whose chain is a moment behind does not know a device just added.
 * A host's list goes only with the host's own removal from the account.
 */
export const reEnrollConnectedKey = (account: string) => `rh.hub.reenroll.connected.${account}`;
/**
 * Renewals this browser verified a dispute of (docs/host-identity-rotation.md
 * §3.8), per account: only ever added to — another tab's are merged, never
 * dropped — and cleared by nothing but this page's own verified chain (an
 * assured renewal after it, or the host's removal: `disputeIsolates`).
 */
export const keptDisputesKey = (account: string) => `rh.hub.renewal.disputes.${account}`;
/** This admin device's own 「这台设备只签在设备上确认的换钥」 (§4.4): set, it signs no automatic renewal, whatever the chain or the hub says. */
export const localAssuredKey = (account: string) => `rh.hub.renewal.localAssured.${account}`;
/** A renewal superseded by a loss after it staged is started again at most this many times in all (§4.1 「清除」). */
/** The Web Lock a page holds while it renews `host` (asking, waiting for the window, writing): across this browser's tabs. */
export const renewalLockName = (account: string, host: string) => `rh.hub.renewal.${account}.${host}`;
export interface CrossTab {
  post(message: unknown): void;
  subscribe(fn: (message: unknown) => void): () => void;
}

/** A `BroadcastChannel` and the `storage` event, in a browser whose `localStorage` the account keeps to; else none. */
export function defaultCrossTab(storage: AccountStorage): CrossTab | null {
  let local: Storage | undefined;
  try {
    local = globalThis.localStorage;
  } catch {
    return null;
  }
  if (!local || storage !== local) return null;
  const Channel = (globalThis as { BroadcastChannel?: typeof BroadcastChannel }).BroadcastChannel;
  const channel = Channel ? new Channel("rh.hub.renewal") : null;
  const target = globalThis as unknown as { addEventListener?: (type: string, fn: (e: unknown) => void) => void; removeEventListener?: (type: string, fn: (e: unknown) => void) => void };
  return {
    post: (message) => {
      try {
        channel?.postMessage(message);
      } catch {
        // the others read storage when they look
      }
    },
    subscribe: (fn) => {
      const onMessage = (e: MessageEvent) => fn(e.data);
      const onStorage = (e: unknown) => {
        const key = (e as { key?: string | null }).key;
        if (key === null || key === undefined || key.startsWith("rh.hub.renewal.disputes.")) fn({ storage: key ?? null });
      };
      channel?.addEventListener("message", onMessage);
      target.addEventListener?.("storage", onStorage);
      return () => {
        channel?.removeEventListener("message", onMessage);
        target.removeEventListener?.("storage", onStorage);
      };
    },
  };
}
/** Where this browser keeps the account's small records (`localStorage`, or a stand-in). */
export type AccountStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/**
 * Web Locks (`navigator.locks`), as far as the seen record uses them: one
 * exclusive lock per account, across the browser's pages — around a pin and
 * the acknowledgements it starts (those in `localStorage`), and around the
 * pin and the record themselves where they are there too (the fallback,
 * `seen-store.ts`).
 */
export interface AccountLocks {
  /**
   * With `ifAvailable`, the callback is handed `null` (and runs at once) when
   * another page holds the lock — as `navigator.locks` does; the renewal
   * this browser is finishing in another tab (`CloudAccount.resumeRenewal`).
   */
  request<T>(name: string, options: { mode: "exclusive"; ifAvailable?: boolean }, callback: (lock?: unknown) => T | Promise<T>): Promise<T>;
}

export type EventTargetLike = Pick<EventTarget, "addEventListener" | "removeEventListener">;

/**
 * Where a page hears that the seen record may have changed under it
 * (`CloudAccount.watchSeenRecord`), besides the store saying another page
 * wrote it (`SeenStore.watch`: a `BroadcastChannel`).
 */
export interface SeenWatch {
  /** `visibilitychange`: the page is back in view (the document). */
  visibility?: EventTargetLike & { visibilityState?: string };
}

/** The document, where there is one. */
export function defaultSeenWatch(): SeenWatch {
  const g = globalThis as { document?: SeenWatch["visibility"] };
  return { visibility: g.document && typeof g.document.addEventListener === "function" ? g.document : undefined };
}

/** `navigator.locks` where the browser has them. */
export function defaultLocks(): AccountLocks | null {
  try {
    const locks = (globalThis.navigator as { locks?: AccountLocks } | undefined)?.locks;
    return locks && typeof locks.request === "function" ? locks : null;
  } catch {
    return null;
  }
}

/** `localStorage`, or (where there is none) memory for this page. */
export function defaultStorage(): AccountStorage {
  try {
    if (globalThis.localStorage) return globalThis.localStorage;
  } catch {
    // not allowed here
  }
  const values = new Map<string, string>();
  return { getItem: (k) => values.get(k) ?? null, setItem: (k, v) => void values.set(k, v), removeItem: (k) => void values.delete(k) };
}

export function readFrom(storage: AccountStorage, key: string): string | null {
  try {
    return storage.getItem(key) ?? null;
  } catch {
    return null;
  }
}

export function writeTo(storage: AccountStorage, key: string, value: string): void {
  try {
    storage.setItem(key, value);
  } catch {
    // memory only for this tab
  }
}

export function forgetIn(storage: AccountStorage, key: string): void {
  try {
    storage.removeItem(key);
  } catch {
    // nothing stored
  }
}

/** Forget this browser's device id `id` for `account`, with what it noted under it: the next sign-in registers it anew. */
export function forgetDeviceIdIn(storage: AccountStorage, account: string, id: string): void {
  for (const key of [deviceKey(account), deviceSignKey(account), afreshKey(account), joinedKey(account, id), keyRequestKey(account, id), hostKeyRequestKey(account, id), ackKey(account, id)]) {
    forgetIn(storage, key);
  }
}
