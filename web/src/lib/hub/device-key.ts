/*
 * The device-local AES-GCM keys this browser keeps for the page's own use:
 * the keyring's wrap key (`keyring.ts`, what the user and host keys are
 * sealed under) and the offline copies' mirror key (`mirror.ts`, what every
 * copy is sealed under). Each is kept as a non-extractable `CryptoKey` in the
 * key store, read back and checked, or made once and replaced the same way.
 *
 * The two keys stay separate: different record names and different AAD
 * labels at their seal sites, so one being lost or forgotten does not open or
 * orphan what the other sealed. What is shared here is only the management
 * state machine — read, check, and make if there is none — with each caller's
 * own cross-tab creation policy and error wording.
 */
import { type KeyStore, readKept, storeKeepsCryptoKeys } from "@/lib/e2e";

/** What the key store handed back, if it is a key both managers use: a non-extractable AES-GCM `CryptoKey` (WebKit may hand back nothing instead). */
export function isAesKey(value: unknown): value is CryptoKey {
  const k = value as Partial<CryptoKey> | undefined;
  return !!k && typeof k === "object" && k.type === "secret" && (k.algorithm as { name?: string } | undefined)?.name === "AES-GCM";
}

export interface DeviceKeyOptions {
  /** What a new key may do (the keyring's wrap key unwraps; a copy only encrypts and decrypts). */
  usages: KeyUsage[];
  /** The error when the store keeps a key it cannot read and cannot be trusted to keep a new one. */
  unreadable: string;
  /**
   * How the key is made when there is none, or the one kept never opens:
   * `fresh` makes and stores one. Each caller's policy decides the cross-tab
   * race, whether the unreadable key is replaced, and what replacing forgets.
   */
  make(existing: unknown, fresh: () => Promise<CryptoKey>): Promise<CryptoKey>;
}

/**
 * The device-local key kept as `name`: the one there if it reads, else one
 * made through `opts.make`. A key that is kept but cannot be read is only
 * replaced when the store can keep a new one — otherwise replacing it would
 * orphan whatever it sealed with no way back.
 */
export async function deviceKey(store: KeyStore, name: string, opts: DeviceKeyOptions): Promise<CryptoKey> {
  const existing = await readKept(store, name);
  if (isAesKey(existing)) return existing;
  if (existing !== undefined && !(await storeKeepsCryptoKeys(store))) throw new Error(opts.unreadable);
  const fresh = async (): Promise<CryptoKey> => {
    const key = (await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, opts.usages)) as CryptoKey;
    await store.put(name, key);
    return key;
  };
  return opts.make(existing, fresh);
}

/** Forget a device-local key's record (a removed device's wipe). The caller drops its cache first. */
export function forgetDeviceKey(store: KeyStore, name: string): Promise<void> {
  return store.delete(name);
}
