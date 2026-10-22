/*
 * The account's symmetric keys in this browser, and the keyring the account
 * shares through the hub (architecture doc §1, §5 "钥匙投递").
 *
 * - **`UK` epochs** (`UserKeyVault`): 32 random bytes each. At rest in this
 *   browser a `UK` epoch exists only encrypted under a device-local AES-GCM
 *   key — generated here once, non-extractable, kept as a `CryptoKey` in the
 *   key store — with an AAD naming the account and the epoch. For use it is
 *   unwrapped straight into a non-extractable HKDF `CryptoKey`; its bytes
 *   appear only transiently (to seal it to a new device, to the recovery key,
 *   or when a rotation makes a new one) and are zeroed after.
 * - **`MK` epochs** are never kept as bytes at all: an `MK` exists at rest
 *   only as its keyring blob, `mk/<machine>/<mkEpoch>/<ukEpoch>` =
 *   `wrapUnder(UK_ukEpoch, context.machineKeyUnderUserKey(…), MK)`, which any
 *   device holding that `UK` epoch unwraps into a non-extractable HKDF key.
 * - **The keyring** (`Keyring`): the account's opaque blobs at the hub
 *   (`GET/PUT /v1/keys`) — the `mk/…` blobs above and the recovery packages
 *   `rk/<ukEpoch>/<senderDevice>/<rskTag>` (`sealForRecovery`; the tag names
 *   the recovery key it is sealed to, since names are written once). The hub
 *   cannot open any of them, and their AEAD contexts bind each to its
 *   account, machine and epochs. But a device that held a user key — one
 *   removed since — could wrap a key of its own choosing, so nothing read from
 *   the keyring is used unless it is the key the chain committed for its
 *   epoch (`chain.checkMachineKey`): every blob for an epoch is tried and the
 *   one that verifies kept, never "the first by name". A copy is cached in the
 *   key store so the page can read its machines' keys before the hub answers.
 * - Epoch numbers come only from the chain, never from keyring names (a name
 *   is anyone's to write); names are parsed with bounded, overflow-safe
 *   numbers and only to find candidates.
 *
 * - **Host keys** (`HostKeyVault`, chain v3): a device that does not manage
 *   the account holds no `UK`, so it cannot open the keyring's `mk/…` blobs.
 *   An admin seals it each host's committed `MK` epochs instead (mailbox
 *   purpose 5); each is checked against the chain's commitment as it is
 *   taken, then kept exactly as a `UK` epoch is — encrypted under the
 *   device-local key, with an AAD naming the account, the host and the epoch
 *   — and unwrapped only into non-extractable keys.
 *
 * What this buys is no raw key bytes at rest and none lingering in memory —
 * not a wall against a script in this origin. Such a script can use the
 * device-local key just as this code does (it decrypts a `UK` epoch to seal
 * it onwards), and so read every key kept here, as it can the device keys
 * (`lib/e2e/keystore.ts`). Against it the defence is a trusted client
 * (docs/web-integrity.md).
 */
import {
  type Bytes,
  type ChainState,
  context,
  copy,
  equal,
  fromHex,
  hex,
  importMachineKey,
  type KeyStore,
  type MachineKey,
  machineKeyFrom,
  type NoiseKeyPair,
  openFrom,
  openFromRecovery,
  readKept,
  RECOVERY_RECIPIENT,
  requireCommitted,
  type SealContext,
  Serial,
  storeKeepsCryptoKeys,
  underKeyStoreLock,
  unwrapUnder,
  utf8,
  WrapError,
  wrapUnder,
  Writer,
} from "@/lib/e2e";
import type { KeyringEntry } from "./api";

const LOCAL_WRAP = "local-wrap/v1";
const UK_LABEL = utf8("rh/v1 local UK\0");
const HK_LABEL = utf8("rh/v1 local MK\0");

const ukRecord = (account: string, epoch: number) => `uk/${account}/${epoch}`;
const ukIndex = (account: string) => `uk-epochs/${account}`;
const keyringCache = (account: string) => `keyring/${account}`;
const hkRecord = (account: string, host: string, epoch: number) => `hk/${account}/${host}/${epoch}`;
const hkIndex = (account: string) => `hk-index/${account}`;
/** Where M1 and M2a kept a machine key as raw hex (migrated away, see `migrateLegacyMachineKeys`). */
export const legacyMachineKeyName = (machine: string, epoch: number) => `mk/${machine}/${epoch}`;

interface StoredUserKey {
  v: 1;
  iv: Bytes;
  ct: Bytes;
}

function isStoredUserKey(value: unknown): value is StoredUserKey {
  const v = value as Partial<StoredUserKey> | undefined;
  return !!v && v.v === 1 && v.iv instanceof Uint8Array && v.ct instanceof Uint8Array;
}

function isAesKey(value: unknown): value is CryptoKey {
  const k = value as Partial<CryptoKey> | undefined;
  return !!k && typeof k === "object" && k.type === "secret" && (k.algorithm as { name?: string } | undefined)?.name === "AES-GCM";
}

function ukAad(account: string, epoch: number): Bytes {
  return new Writer().raw(UK_LABEL).str16(account).u32(epoch).finish();
}

/** One device-local wrap key per key store, however many vaults use it (two creating it at once would orphan one's keys). */
const localKeys = new WeakMap<KeyStore, Promise<CryptoKey>>();

/** The records sealed under the device-local wrap key, every account's: the user keys and the hosts' machine keys, with their lists. */
const SEALED_LOCALLY = ["uk/", "uk-epochs/", "hk/", "hk-index/"];

/**
 * Forget every record sealed under a device-local wrap key that is gone, with
 * the lists that name them: a vault that still listed an epoch would skip it
 * when another device sends it again (`put`), and would not say it lacks keys.
 */
async function forgetSealedLocally(store: KeyStore): Promise<void> {
  if (!store.list) return console.warn("keyring: this key store cannot list its records: what the old local key sealed stays listed");
  for (const prefix of SEALED_LOCALLY) for (const name of await store.list(prefix)) await store.delete(name);
}

function localWrapKey(store: KeyStore): Promise<CryptoKey> {
  let key = localKeys.get(store);
  if (!key) {
    key = (async () => {
      // Kept, but handed back as nothing (WebKit, for a stored `CryptoKey` it
      // cannot unseal) is read again: a new one orphans every key sealed
      // under it. Only when that one will never open again (the store keeps
      // new keys fine) is it replaced — and what it sealed forgotten, so that
      // another device sending those keys again restores them. Otherwise fail
      // this time: the next use reads it again.
      const existing = await readKept(store, LOCAL_WRAP);
      if (isAesKey(existing)) return existing;
      if (existing !== undefined && !(await storeKeepsCryptoKeys(store))) throw new Error("this browser could not read the key its user keys are sealed under; reload the page");
      // Decided under the lock, on one more read: another page may have made it, or this read may work now.
      return underKeyStoreLock("rh-local-wrap", async () => {
        const again = await store.get(LOCAL_WRAP);
        if (isAesKey(again)) return again;
        // Kept but handed back as nothing here (whatever the first read said): replaced only if it provably never opens again.
        if (again !== undefined) {
          if (!(await storeKeepsCryptoKeys(store))) throw new Error("this browser could not read the key its user keys are sealed under; reload the page");
          // The store keeps new keys: one more read before deciding this one never opens again — it may now.
          const last = await store.get(LOCAL_WRAP);
          if (isAesKey(last)) return last;
        }
        // The new key first: a store that will not take it changes nothing (the old key and what it sealed stay).
        const fresh = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt", "unwrapKey"]);
        await store.put(LOCAL_WRAP, fresh);
        if (again !== undefined) {
          console.warn("keyring: the device-local key will never open again: what it sealed is forgotten, and other devices send it again");
          await forgetSealedLocally(store);
        }
        return fresh;
      });
    })();
    key.catch(() => localKeys.delete(store));
    localKeys.set(store, key);
  }
  return key;
}

/** Forget the device-local wrap key (a removed device's wipe). */
export async function forgetLocalWrapKey(store: KeyStore): Promise<void> {
  localKeys.delete(store);
  await store.delete(LOCAL_WRAP);
}

/** This browser's `UK` epochs for one account, encrypted at rest under the device-local key. */
export class UserKeyVault {
  private readonly serial = new Serial();
  private readonly inUse = new Map<number, Promise<CryptoKey>>();

  constructor(
    readonly store: KeyStore,
    readonly account: string,
  ) {}

  /** The epochs held, ascending. */
  async epochs(): Promise<number[]> {
    const list = await this.store.get(ukIndex(this.account));
    return Array.isArray(list) ? list.filter((e): e is number => Number.isInteger(e) && e >= 0).sort((a, b) => a - b) : [];
  }

  async has(epoch: number): Promise<boolean> {
    return (await this.epochs()).includes(epoch);
  }

  async newest(): Promise<number | null> {
    return (await this.epochs()).at(-1) ?? null;
  }

  /**
   * Keep `raw` as epoch `epoch` (encrypted; `raw` itself is left for the
   * caller to zero). An epoch already held is kept as it is: `false`.
   */
  put(epoch: number, raw: Uint8Array): Promise<boolean> {
    if (!Number.isInteger(epoch) || epoch < 0 || epoch > 0xffff_ffff) return Promise.reject(new Error(`not a key epoch: ${epoch}`));
    if (raw.length !== 32) return Promise.reject(new Error("a user key is 32 bytes"));
    return this.serial.run(async () => {
      // The local key first: one that had to be replaced forgets what it sealed, and the list read after says so.
      const local = await localWrapKey(this.store);
      const epochs = await this.epochs();
      if (epochs.includes(epoch)) return false;
      const iv = crypto.getRandomValues(new Uint8Array(12));
      const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: ukAad(this.account, epoch) }, local, copy(raw)));
      const record: StoredUserKey = { v: 1, iv, ct };
      await this.store.put(ukRecord(this.account, epoch), record);
      await this.store.put(ukIndex(this.account), [...epochs, epoch].sort((a, b) => a - b));
      return true;
    });
  }

  private async record(epoch: number): Promise<StoredUserKey> {
    const record = await this.store.get(ukRecord(this.account, epoch));
    if (!isStoredUserKey(record)) throw new Error(`this device does not hold user key epoch ${epoch}`);
    return record;
  }

  /** Epoch `epoch` as a non-extractable HKDF key (what wraps and unwraps machine keys). */
  key(epoch: number): Promise<CryptoKey> {
    let key = this.inUse.get(epoch);
    if (!key) {
      key = (async () => {
        const local = await localWrapKey(this.store);
        const { iv, ct } = await this.record(epoch);
        return crypto.subtle.unwrapKey("raw", ct, local, { name: "AES-GCM", iv, additionalData: ukAad(this.account, epoch) }, "HKDF", false, [
          "deriveKey",
          "deriveBits",
        ]);
      })();
      key.catch(() => this.inUse.delete(epoch));
      this.inUse.set(epoch, key);
    }
    return key;
  }

  /** Epoch `epoch`'s bytes for the length of `use` only (sealing it to someone), zeroed after. */
  async withRaw<T>(epoch: number, use: (raw: Bytes) => Promise<T>): Promise<T> {
    const local = await localWrapKey(this.store);
    const { iv, ct } = await this.record(epoch);
    const raw = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv, additionalData: ukAad(this.account, epoch) }, local, ct));
    try {
      return await use(raw);
    } finally {
      raw.fill(0);
    }
  }

  /** Every epoch's bytes for the length of `use` (a pairing delivery carries them all), zeroed after. */
  async withAllRaw<T>(use: (keys: { epoch: number; key: Bytes }[]) => Promise<T>): Promise<T> {
    const keys: { epoch: number; key: Bytes }[] = [];
    try {
      const local = await localWrapKey(this.store);
      for (const epoch of await this.epochs()) {
        const { iv, ct } = await this.record(epoch);
        keys.push({ epoch, key: new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv, additionalData: ukAad(this.account, epoch) }, local, ct)) });
      }
      return await use(keys);
    } finally {
      for (const k of keys) k.key.fill(0);
    }
  }

  /** Forget every epoch of this account. */
  wipe(): Promise<void> {
    return this.serial.run(async () => {
      for (const epoch of await this.epochs()) await this.store.delete(ukRecord(this.account, epoch));
      await this.store.delete(ukIndex(this.account));
      this.inUse.clear();
    });
  }
}

function hkAad(account: string, host: string, epoch: number): Bytes {
  return new Writer().raw(HK_LABEL).str16(account).str16(host).u32(epoch).finish();
}

/**
 * The hosts' machine key epochs this device was handed one by one (chain v3:
 * a device that does not manage the account, mailbox purpose 5), encrypted
 * at rest under the device-local key. The caller checks each against the
 * chain's commitment before `put`; nothing here reads the chain.
 */
export class HostKeyVault {
  private readonly serial = new Serial();
  private readonly inUse = new Map<string, Promise<MachineKey>>();

  constructor(
    readonly store: KeyStore,
    readonly account: string,
  ) {}

  private async index(): Promise<Record<string, number[]>> {
    const raw = await this.store.get(hkIndex(this.account));
    if (!raw || typeof raw !== "object") return {};
    const out: Record<string, number[]> = {};
    for (const [host, list] of Object.entries(raw as Record<string, unknown>)) {
      if (Array.isArray(list)) out[host] = list.filter((e): e is number => Number.isInteger(e) && e >= 0).sort((a, b) => a - b);
    }
    return out;
  }

  /** The hosts this device holds a key epoch of. */
  async hosts(): Promise<string[]> {
    return Object.keys(await this.index()).sort();
  }

  /** Host `host`'s epochs held, ascending. */
  async epochs(host: string): Promise<number[]> {
    return (await this.index())[host] ?? [];
  }

  async has(host: string, epoch: number): Promise<boolean> {
    return (await this.epochs(host)).includes(epoch);
  }

  /** Keep `raw` as host `host`'s epoch `epoch` (encrypted; the caller zeroes `raw`). One held already is kept: `false`. */
  put(host: string, epoch: number, raw: Uint8Array): Promise<boolean> {
    if (!Number.isInteger(epoch) || epoch < 0 || epoch > 0xffff_ffff) return Promise.reject(new Error(`not a key epoch: ${epoch}`));
    if (raw.length !== 32) return Promise.reject(new Error("a machine key is 32 bytes"));
    if (!host || host.length > 64) return Promise.reject(new Error("not a host id"));
    return this.serial.run(async () => {
      // The local key first (see UserKeyVault.put).
      const local = await localWrapKey(this.store);
      const index = await this.index();
      const held = index[host] ?? [];
      if (held.includes(epoch)) return false;
      const iv = crypto.getRandomValues(new Uint8Array(12));
      const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: hkAad(this.account, host, epoch) }, local, copy(raw)));
      const record: StoredUserKey = { v: 1, iv, ct };
      await this.store.put(hkRecord(this.account, host, epoch), record);
      await this.store.put(hkIndex(this.account), { ...index, [host]: [...held, epoch].sort((a, b) => a - b) });
      return true;
    });
  }

  /** Host `host`'s epoch `epoch` as the record layer's non-extractable keys. */
  key(host: string, epoch: number): Promise<MachineKey> {
    const id = `${host}/${epoch}`;
    let key = this.inUse.get(id);
    if (!key) {
      key = (async () => {
        const local = await localWrapKey(this.store);
        const record = await this.store.get(hkRecord(this.account, host, epoch));
        if (!isStoredUserKey(record)) throw new Error(`this device does not hold ${host}'s key epoch ${epoch}`);
        const hkdf = await crypto.subtle.unwrapKey(
          "raw",
          record.ct,
          local,
          { name: "AES-GCM", iv: record.iv, additionalData: hkAad(this.account, host, epoch) },
          "HKDF",
          false,
          ["deriveKey", "deriveBits"],
        );
        return machineKeyFrom(hkdf);
      })();
      key.catch(() => this.inUse.delete(id));
      this.inUse.set(id, key);
    }
    return key;
  }

  /** Forget every host key of this account (a removed device's wipe). */
  wipe(): Promise<void> {
    return this.serial.run(async () => {
      for (const [host, epochs] of Object.entries(await this.index())) {
        for (const epoch of epochs) await this.store.delete(hkRecord(this.account, host, epoch));
      }
      await this.store.delete(hkIndex(this.account));
      this.inUse.clear();
    });
  }
}

/** The recovery key a package is sealed to, as its keyring name says it: the first 8 bytes of `RSK`, in hex. */
export function recoveryTag(rsk: Uint8Array): string {
  return hex(rsk.slice(0, 8));
}

/**
 * Keyring names (`PUT /v1/keys/<name>`): what a blob is — its *base* — and,
 * as the last segment, the device that wrote it (`<base>/<writer>`, or
 * `<base>/<writer>.<suffix>` for a second try). M2 re-review: names were
 * free for any full device to take first, so a stolen device could squat the
 * next rotation's names before it was removed; the hub now takes a name only
 * from the device it ends with, and only for epochs the chain has committed.
 */
export const keyName = {
  machine: (machine: string, mkEpoch: number, ukEpoch: number) => `mk/${machine}/${mkEpoch}/${ukEpoch}`,
  recovery: (ukEpoch: number, sender: string, rsk: Uint8Array) => `rk/${ukEpoch}/${sender}/${recoveryTag(rsk)}`,
  /** `base` as written by `writer` (`suffix` for a retry under a new name). */
  written: (base: string, writer: string, suffix?: string) => `${base}/${writer}${suffix ? `.${suffix}` : ""}`,
};

export type KeyNameParts = (
  | { kind: "machine"; machine: string; mkEpoch: number; ukEpoch: number }
  /** `tag`: the recovery key's tag (`null` for a name from before tags). */
  | { kind: "recovery"; ukEpoch: number; sender: string; tag: string | null }
) & {
  /** The blob's base name (the name without its writer). */
  base: string;
  /** The device that wrote it, as its name says (`null`: a name from before writers). */
  writer: string | null;
};

const NAME = /^[a-z0-9._/-]{1,128}$/;

/** What the hub accepts as a key name. */
export function validKeyName(name: string): boolean {
  return NAME.test(name) && !name.includes("..");
}

function epochPart(text: string | undefined): number | null {
  if (!text || !/^\d{1,10}$/.test(text)) return null;
  const n = Number(text);
  return n <= 0xffff_ffff ? n : null;
}

/** A name's last segment as its writer: `<writer>` or `<writer>.<suffix>`. */
function writerPart(text: string | undefined): string | null {
  if (!text) return null;
  const writer = text.split(".")[0]!;
  return writer ? writer : null;
}

export function parseKeyName(name: string): KeyNameParts | null {
  const parts = name.split("/");
  if (parts[0] === "mk" && (parts.length === 4 || parts.length === 5) && parts[1]) {
    const mkEpoch = epochPart(parts[2]);
    const ukEpoch = epochPart(parts[3]);
    const writer = parts.length === 5 ? writerPart(parts[4]) : null;
    if (mkEpoch !== null && ukEpoch !== null && (parts.length === 4 || writer)) {
      return { kind: "machine", machine: parts[1], mkEpoch, ukEpoch, base: parts.slice(0, 4).join("/"), writer };
    }
  }
  if (parts[0] === "rk" && parts.length >= 3 && parts.length <= 5 && parts[2]) {
    const ukEpoch = epochPart(parts[1]);
    const tag = parts.length >= 4 ? (/^[0-9a-f]{16}$/.test(parts[3]!) ? parts[3]! : undefined) : null;
    const writer = parts.length === 5 ? writerPart(parts[4]) : null;
    // A package's sender is the device that wrote it.
    if (parts.length === 5 && writer !== parts[2]) return null;
    if (ukEpoch !== null && tag !== undefined) return { kind: "recovery", ukEpoch, sender: parts[2], tag, base: parts.slice(0, 4).join("/"), writer };
  }
  return null;
}

/** `MK` epoch `mkEpoch` of `machine` as its keyring blob, under `UK` epoch `ukEpoch`. */
export function wrapMachineKey(uk: CryptoKey, account: string, machine: string, mkEpoch: number, ukEpoch: number, mk: Uint8Array): Promise<Bytes> {
  return wrapUnder(uk, context.machineKeyUnderUserKey(account, machine, mkEpoch, ukEpoch), mk);
}

/** A keyring blob's `MK` as bytes (the caller checks and zeroes it). */
export function unwrapMachineKeyBlob(uk: CryptoKey, account: string, machine: string, mkEpoch: number, ukEpoch: number, blob: Uint8Array): Promise<Bytes> {
  return unwrapUnder(uk, context.machineKeyUnderUserKey(account, machine, mkEpoch, ukEpoch), blob);
}

/**
 * A keyring blob back into the record layer's non-extractable keys — only if
 * it is the key `check` says the chain committed for that epoch (it rejects
 * otherwise). The bytes exist only while they are checked and imported.
 */
export async function openMachineKeyBlob(
  uk: CryptoKey,
  account: string,
  machine: string,
  mkEpoch: number,
  ukEpoch: number,
  blob: Uint8Array,
  check: MachineKeyCheck,
): Promise<MachineKey> {
  const raw = await requireCommitted(await unwrapMachineKeyBlob(uk, account, machine, mkEpoch, ukEpoch, blob), (k) => check(mkEpoch, k));
  try {
    return await importMachineKey(raw);
  } finally {
    raw.fill(0);
  }
}

/**
 * A recovery package's `UK` (`rk/<ukEpoch>/<sender>/…`), opened with the
 * recovery words' seal key: sealed by any device `chain` names — a current
 * admin, or one demoted or removed since — and exactly the key the
 * chain committed for its epoch (`chain.checkUserKey`); rejects otherwise.
 *
 * The commitment, not the sender, is what makes it the account's key: a
 * device removed since still holds old user keys and the public `RSK`, but
 * it can seal only a key the chain already names — never one of its own, nor
 * an epoch out of reach (both fail the check). And a package sealed by an
 * admin no longer current can be the only way back in for its epoch (a
 * demoted admin writes no new one, a removed one took its user keys along),
 * so the hub keeps one per sender and epoch and this opens it — as
 * `openFromRecovery` (and Rust's `wrap::open_from_recovery`) do.
 */
export async function openRecoveryPackage(seal: NoiseKeyPair, chain: ChainState, ctx: SealContext, sealed: Uint8Array): Promise<Bytes> {
  return openFromRecovery(seal, chain, ctx, sealed);
}

/** Whether `key` is machine key epoch `mkEpoch` as the chain committed it (`chain.checkMachineKey`): rejects if not. */
export type MachineKeyCheck = (mkEpoch: number, key: Bytes) => Promise<void>;

/** How the keyring reaches the hub (`HubApi.keys`/`putKey`, or a fake). */
export interface KeyringBackend {
  list(): Promise<KeyringEntry[]>;
  /** Write once: `stored: false` when the name held these very bytes already; another blob under it rejects (409 `exists`). */
  put(name: string, blob: Uint8Array): Promise<{ stored: boolean } | void>;
}

/** How many new names a blob is tried under after a 409 on its own name. */
export const KEYRING_RETRIES = 2;

/** Whether `error` is the hub's 409 for a name already holding other bytes. */
function nameTaken(error: unknown): boolean {
  return (error as { status?: unknown } | null)?.status === 409;
}

interface CachedKeyring {
  v: 1;
  entries: { name: string; blob: string; updatedMs: number }[];
}

/** The account's keyring: the hub's copy, cached in the key store (it holds only wrapped blobs). */
export class Keyring {
  private entries = new Map<string, KeyringEntry>();
  private loaded = false;
  private readonly serial = new Serial();

  constructor(
    private readonly backend: KeyringBackend,
    private readonly store: KeyStore,
    readonly account: string,
    /** This device: the writer every name it puts ends with. */
    readonly writer: string,
  ) {}

  /** The cached copy, if nothing newer is held. */
  async load(): Promise<void> {
    if (this.loaded) return;
    const cached = (await this.store.get(keyringCache(this.account))) as Partial<CachedKeyring> | undefined;
    if (!this.loaded && cached?.v === 1 && Array.isArray(cached.entries)) {
      for (const e of cached.entries) {
        if (typeof e?.name === "string" && typeof e.blob === "string") this.entries.set(e.name, { name: e.name, blob: fromHex(e.blob), updatedMs: e.updatedMs ?? 0 });
      }
    }
    this.loaded = true;
  }

  private save(): Promise<void> {
    const value: CachedKeyring = { v: 1, entries: [...this.entries.values()].map((e) => ({ name: e.name, blob: hex(e.blob), updatedMs: e.updatedMs })) };
    return this.store.put(keyringCache(this.account), value);
  }

  /** The hub's keyring, replacing the cached copy. */
  refresh(): Promise<void> {
    return this.serial.run(async () => {
      const list = await this.backend.list();
      this.entries = new Map(list.filter((e) => validKeyName(e.name)).map((e) => [e.name, e]));
      this.loaded = true;
      await this.save();
    });
  }

  /**
   * Put `blob` as `base`, written by this device (`<base>/<writer>`, written
   * once at the hub), and resolve with the name it went under — the hub holds
   * exactly these bytes there, just now or from an earlier try.
   *
   * Only this device writes names ending in its id, so a 409 (other bytes
   * there already) is not another device's claim: it is this device's own
   * earlier blob (another tab) or the hub tampering. Either way the blob goes
   * under a new name (`<writer>.<random>`) rather than being skipped (M2
   * re-review: a rotation used to skip the machine whose name was taken).
   */
  put(base: string, blob: Uint8Array): Promise<string> {
    const first = keyName.written(base, this.writer);
    if (!validKeyName(first)) return Promise.reject(new Error(`not a keyring name: ${first}`));
    return this.serial.run(async () => {
      let name = first;
      for (let attempt = 0; ; attempt++) {
        try {
          await this.backend.put(name, blob);
          break;
        } catch (error) {
          if (!nameTaken(error) || attempt >= KEYRING_RETRIES) throw error;
          console.warn(`cloud keyring: the hub says ${name} holds other bytes — this device's own earlier blob, or tampering; trying a new name`);
          name = keyName.written(base, this.writer, hex(crypto.getRandomValues(new Uint8Array(4))));
        }
      }
      this.entries.set(name, { name, blob: copy(blob), updatedMs: Date.now() });
      await this.save();
      return name;
    });
  }

  /** Whether some blob is held as `base` (by `writer`, if given). */
  hasBase(base: string, writer?: string): boolean {
    return this.list().some((e) => {
      const p = parseKeyName(e.name);
      return p?.base === base && (writer === undefined || p.writer === writer);
    });
  }

  /** The blobs held as `base`, by whoever wrote them. */
  byBase(base: string): KeyringEntry[] {
    return this.list().filter((e) => parseKeyName(e.name)?.base === base);
  }

  /** The blob held under `name` (the cached copy), if any. */
  get(name: string): Bytes | undefined {
    return this.entries.get(name)?.blob;
  }

  list(): KeyringEntry[] {
    return [...this.entries.values()];
  }

  has(name: string): boolean {
    return this.entries.has(name);
  }

  /** Machine `machine`'s blobs: one per (`MK` epoch, `UK` epoch it is wrapped under). */
  machineBlobs(machine: string): { mkEpoch: number; ukEpoch: number; blob: Bytes }[] {
    return this.list().flatMap((e) => {
      const p = parseKeyName(e.name);
      return p?.kind === "machine" && p.machine === machine ? [{ mkEpoch: p.mkEpoch, ukEpoch: p.ukEpoch, blob: e.blob }] : [];
    });
  }

  /** The recovery packages: `UK` epochs sealed to a recovery key, by whichever device sealed them. */
  recoveryBlobs(): { name: string; ukEpoch: number; sender: string; tag: string | null; blob: Bytes; updatedMs: number }[] {
    return this.list().flatMap((e) => {
      const p = parseKeyName(e.name);
      return p?.kind === "recovery" ? [{ name: e.name, ukEpoch: p.ukEpoch, sender: p.sender, tag: p.tag, blob: e.blob, updatedMs: e.updatedMs }] : [];
    });
  }

  /** Forget the cached copy (a removed device's wipe). */
  async wipe(): Promise<void> {
    this.entries.clear();
    await this.store.delete(keyringCache(this.account));
  }
}

/** Machine `machine`'s blobs for epochs `0..epochs-1` under `UK` epochs held, newest `UK` first. */
async function candidates(vault: UserKeyVault, keyring: Keyring, machine: string, epochs: number): Promise<Map<number, { ukEpoch: number; blob: Bytes }[]>> {
  const held = new Set(await vault.epochs());
  const byEpoch = new Map<number, { ukEpoch: number; blob: Bytes }[]>();
  for (const b of keyring.machineBlobs(machine)) {
    if (!held.has(b.ukEpoch) || b.mkEpoch >= epochs) continue;
    const list = byEpoch.get(b.mkEpoch) ?? [];
    list.push(b);
    byEpoch.set(b.mkEpoch, list);
  }
  for (const list of byEpoch.values()) list.sort((x, y) => y.ukEpoch - x.ukEpoch);
  return byEpoch;
}

/**
 * Machine `machine`'s key epochs this device can open — only the `epochs`
 * the chain committed — each from whichever of its blobs (under a `UK` epoch
 * the vault holds) unwraps to the committed key. Blobs that do not verify are
 * ignored.
 */
export async function openMachineKeys(
  vault: UserKeyVault,
  keyring: Keyring,
  account: string,
  machine: string,
  epochs: number,
  check: MachineKeyCheck,
): Promise<Map<number, MachineKey>> {
  const keys = new Map<number, MachineKey>();
  for (const [mkEpoch, blobs] of [...(await candidates(vault, keyring, machine, epochs)).entries()].sort((a, b) => a[0] - b[0])) {
    for (const b of blobs) {
      try {
        keys.set(mkEpoch, await openMachineKeyBlob(await vault.key(b.ukEpoch), account, machine, mkEpoch, b.ukEpoch, b.blob, check));
        break;
      } catch {
        // spoiled, or not the committed key: the next
      }
    }
  }
  return keys;
}

/**
 * `MK` epoch `mkEpoch` of `machine` as bytes (to hand to its agent; the
 * caller zeroes them), from a blob that unwraps to the committed key — under
 * `UK` epoch `ukEpoch` only, when given. `null` when none does.
 */
export async function machineKeyBytes(
  vault: UserKeyVault,
  keyring: Keyring,
  account: string,
  machine: string,
  mkEpoch: number,
  check: MachineKeyCheck,
  ukEpoch?: number,
): Promise<Bytes | null> {
  const blobs = (await candidates(vault, keyring, machine, mkEpoch + 1)).get(mkEpoch) ?? [];
  for (const b of blobs) {
    if (ukEpoch !== undefined && b.ukEpoch !== ukEpoch) continue;
    try {
      return await requireCommitted(await unwrapMachineKeyBlob(await vault.key(b.ukEpoch), account, machine, mkEpoch, b.ukEpoch, b.blob), (k) => check(mkEpoch, k));
    } catch {
      // the next
    }
  }
  return null;
}

/** A machine-key wrap a rotation writes again, under the new user key and this device's own name. */
export interface Rewrap {
  machine: string;
  mkEpoch: number;
}

/**
 * The machine-key wraps a rotation to `UK` epoch `ukEpoch` rewrites under
 * this device's own name (relay-and-plans §6): for every machine and every
 * committed `MK` epoch (`epochs`) the keyring holds a blob of under a `UK`
 * epoch this device holds — so it can read it — `mk/<m>/<e>/<ukEpoch>`,
 * unless this device wrote that already. With these in, the hub reclaims
 * this device's own older wraps (a removed or demoted writer keeps one copy
 * per epoch): `MK_e` stays readable under the newest `UK`, which every
 * current device holds.
 */
export function rewrapTargets(
  keyring: Keyring,
  machines: { id: string; epochs: number }[],
  held: ReadonlySet<number>,
  ukEpoch: number,
  opts: { skipOwn?: boolean } = {},
): Rewrap[] {
  const out: Rewrap[] = [];
  for (const { id, epochs } of machines) {
    const readable = new Set<number>();
    for (const b of keyring.machineBlobs(id)) {
      if (b.mkEpoch < epochs && held.has(b.ukEpoch)) readable.add(b.mkEpoch);
    }
    for (const mkEpoch of [...readable].sort((a, b) => a - b)) {
      if (opts.skipOwn !== false && keyring.hasBase(keyName.machine(id, mkEpoch, ukEpoch), keyring.writer)) continue;
      out.push({ machine: id, mkEpoch });
    }
  }
  return out;
}

/** Whether this device's own blob of `MK_mkEpoch` under `UK_ukEpoch` is there and opens to the committed key. */
async function ownWrapOpens(vault: UserKeyVault, keyring: Keyring, account: string, machine: string, mkEpoch: number, ukEpoch: number, check: MachineKeyCheck): Promise<boolean> {
  const base = keyName.machine(machine, mkEpoch, ukEpoch);
  for (const e of keyring.byBase(base)) {
    if (parseKeyName(e.name)?.writer !== keyring.writer) continue;
    try {
      const raw = await requireCommitted(await unwrapMachineKeyBlob(await vault.key(ukEpoch), account, machine, mkEpoch, ukEpoch, e.blob), (k) => check(mkEpoch, k));
      raw.fill(0);
      return true;
    } catch {
      // another tab's garbage, or the hub's: written again below
    }
  }
  return false;
}

/**
 * Write each of {@link rewrapTargets} — the key read from a blob that
 * unwraps to what the chain committed (`check`), wrapped under `UK` epoch
 * `ukEpoch` — through {@link Keyring.put} (write-once names: the same bytes
 * again are a retry, other bytes go under a new suffix). One failing leaves
 * the others: the old wraps stay readable until the hub sees the new one.
 */
export async function rewrapMachineKeys(
  vault: UserKeyVault,
  keyring: Keyring,
  account: string,
  machines: { id: string; epochs: number }[],
  ukEpoch: number,
  check: (machine: string) => MachineKeyCheck,
): Promise<{ rewrapped: Rewrap[]; failed: (Rewrap & { error: string })[] }> {
  // This device's own wraps count only if they open (another tab's, or bytes the hub put there, may not).
  const targets = rewrapTargets(keyring, machines, new Set(await vault.epochs()), ukEpoch, { skipOwn: false });
  const rewrapped: Rewrap[] = [];
  const failed: (Rewrap & { error: string })[] = [];
  if (!targets.length) return { rewrapped, failed };
  const uk = await vault.key(ukEpoch);
  for (const target of targets) {
    let mk: Bytes | null = null;
    try {
      if (await ownWrapOpens(vault, keyring, account, target.machine, target.mkEpoch, ukEpoch, check(target.machine))) continue;
      mk = await machineKeyBytes(vault, keyring, account, target.machine, target.mkEpoch, check(target.machine));
      if (!mk) throw new Error("no blob of it opens to the committed key");
      await keyring.put(keyName.machine(target.machine, target.mkEpoch, ukEpoch), await wrapMachineKey(uk, account, target.machine, target.mkEpoch, ukEpoch, mk));
      rewrapped.push(target);
    } catch (error) {
      failed.push({ ...target, error: (error as { code?: unknown })?.code ? String((error as { code: unknown }).code) : error instanceof Error ? error.message : String(error) });
    } finally {
      mk?.fill(0);
    }
  }
  return { rewrapped, failed };
}

/**
 * Move M1/M2a's raw `mk/<machine>/<epoch>` entries into the keyring, wrapped
 * under `UK` epoch `ukEpoch`, and delete them — each only once the keyring,
 * read back from the hub, holds a blob under its name that unwraps to exactly
 * the raw key (a blob someone else put under that name first, garbage or
 * another key, leaves the raw copy where it is). Returns how many moved.
 */
export async function migrateLegacyMachineKeys(
  store: KeyStore,
  vault: UserKeyVault,
  keyring: Keyring,
  account: string,
  machines: string[],
  ukEpoch: () => Promise<number | null>,
  maxEpochs = 64,
): Promise<number> {
  const found: { machine: string; epoch: number; hex: string }[] = [];
  for (const machine of machines) {
    for (let epoch = 0; epoch < maxEpochs; epoch++) {
      const stored = await store.get(legacyMachineKeyName(machine, epoch));
      if (typeof stored !== "string") break;
      found.push({ machine, epoch, hex: stored });
    }
  }
  if (!found.length) return 0;
  const epoch = await ukEpoch();
  if (epoch === null) return 0;
  const uk = await vault.key(epoch);
  /** Whether a blob held as the base of `f` (by anyone) unwraps to exactly its raw key. */
  const held = async (f: { machine: string; epoch: number; hex: string }): Promise<boolean> => {
    for (const { blob } of keyring.byBase(keyName.machine(f.machine, f.epoch, epoch))) {
      let raw: Bytes | null = null;
      let back: Bytes | null = null;
      try {
        raw = fromHex(f.hex);
        back = await unwrapMachineKeyBlob(uk, account, f.machine, f.epoch, epoch, blob);
        if (raw.length === 32 && equal(back, raw)) return true;
      } catch {
        // not the key it was meant to hold
      } finally {
        raw?.fill(0);
        back?.fill(0);
      }
    }
    return false;
  };
  for (const f of found) {
    const name = keyName.machine(f.machine, f.epoch, epoch);
    // A blob already there counts only if it holds this very key (another
    // blob there — garbage, or someone else's key — is no reason to skip).
    if (await held(f)) continue;
    let raw: Bytes | null = null;
    try {
      raw = fromHex(f.hex);
      if (raw.length !== 32) throw new Error("not a machine key");
      await keyring.put(name, await wrapMachineKey(uk, account, f.machine, f.epoch, epoch, raw));
    } catch {
      // unreadable, taken, or the hub would not take it: checked below, and left if need be
    } finally {
      raw?.fill(0);
    }
  }
  // What the hub holds now, not what this page believes it wrote.
  try {
    await keyring.refresh();
  } catch {
    return 0;
  }
  let moved = 0;
  for (const f of found) {
    // Not held by any blob: the raw copy stays.
    if (!(await held(f))) continue;
    await store.delete(legacyMachineKeyName(f.machine, f.epoch));
    moved++;
  }
  return moved;
}
